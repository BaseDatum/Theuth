//! The encoder thread (01 §4.3): every 20 ms drain both rings, emit the frames
//! the recording clock says are due — a source with nothing buffered gets
//! zeros (53 §3.1, silence when a source stalls) — resample each track to
//! 24 kHz once, mix with a soft limiter, encode three Opus streams, write Ogg
//! pages (one page per second), report RMS at 10 Hz. Only this thread touches
//! the files.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use rtrb::Consumer;
use rubato::Resampler as _;
use tokio::sync::watch;

use super::{RATE, Sample};
use crate::ogg::{self, Writer};
use crate::state::RecordingClock;
use crate::store::Stem;

/// Samples per frame at [`RATE`]: 20 ms.
const IN_FRAME: usize = RATE as usize / 50;
/// Emit frame `n` once the clock passes `(n + 1) * 20 ms + LAG`: chunks up to
/// this late still land in their own frame.
const LAG: Duration = Duration::from_millis(60);
/// A source buffering more than this is running behind the clock; the oldest
/// samples are dropped so the tracks stay aligned.
// ponytail: fixed cap, no drift estimation; a rate-adaptive resampler if two device clocks ever drift past this
const MAX_BACKLOG: usize = IN_FRAME * 10;
/// Frames per level report: 100 ms.
const LEVEL_EVERY: u32 = 5;
/// Biggest packet libopus produces at any bitrate for 20 ms mono.
const MAX_PACKET: usize = 1500;

/// One 48 kHz source as the encoder sees it.
struct Source {
    ring: Consumer<Sample>,
    buf: Vec<f32>,
    /// Sum of squares of the samples emitted since the last level report.
    energy: f64,
}

impl Source {
    fn new(ring: Consumer<Sample>) -> Self {
        Self {
            ring,
            buf: Vec::with_capacity(MAX_BACKLOG + IN_FRAME),
            energy: 0.0,
        }
    }

    /// Move everything the producer pushed into `buf`; the first receipt seen, if any.
    fn drain(&mut self) -> Option<Instant> {
        let mut first = None;
        while let Ok((at, pcm)) = self.ring.pop() {
            first.get_or_insert(at);
            self.buf.extend_from_slice(&pcm);
        }
        first
    }

    fn discard(&mut self) {
        while self.ring.pop().is_ok() {}
        self.buf.clear();
    }

    /// Fill `out` with the next 20 ms, zeros where the source has nothing.
    fn take(&mut self, out: &mut [f32; IN_FRAME]) {
        let n = self.buf.len().min(IN_FRAME);
        out[..n].copy_from_slice(&self.buf[..n]);
        out[n..].fill(0.0);
        self.buf.drain(..n);
        if self.buf.len() > MAX_BACKLOG {
            let excess = self.buf.len() - MAX_BACKLOG;
            self.buf.drain(..excess);
        }
        self.energy += out
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>();
    }

    fn rms(&mut self, frames: u32) -> f32 {
        let rms = (self.energy / f64::from(frames * IN_FRAME as u32)).sqrt() as f32;
        self.energy = 0.0;
        rms
    }
}

/// One Opus stream: encoder, pending 24 kHz samples, file.
struct Track {
    enc: opus::Encoder,
    pending: Vec<f32>,
    out: Writer<File>,
}

impl Track {
    fn open(dir: &Path, stem: Stem, title: &str) -> anyhow::Result<Self> {
        let mut enc = opus::Encoder::new(ogg::RATE, opus::Channels::Mono, opus::Application::Voip)
            .context("opus encoder")?;
        enc.set_bitrate(opus::Bitrate::Bits(32_000))
            .context("opus bitrate")?;
        // Lookahead is at the encoder rate; pre-skip is at 48 kHz.
        let pre_skip = (enc.get_lookahead().unwrap_or(0) as u32 * 48_000 / ogg::RATE) as u16;
        let path = dir.join(stem.file());
        let file = File::create(&path).with_context(|| format!("create {}", path.display()))?;
        let track = match stem {
            Stem::Mic => "mic",
            Stem::System => "system",
            Stem::Mix => "mix",
        };
        let serial = u32::from_le_bytes(*b"wth\0") + stem as u32;
        let out = Writer::new(
            file,
            serial,
            pre_skip,
            &[("TITLE", title), ("TRACK", track)],
        )
        .with_context(|| format!("write headers of {}", path.display()))?;
        Ok(Self {
            enc,
            pending: Vec::with_capacity(ogg::FRAME * 4),
            out,
        })
    }

    /// Encode every full frame in `pending`.
    fn encode(&mut self, buf: &mut [u8]) -> anyhow::Result<()> {
        while self.pending.len() >= ogg::FRAME {
            let n = self
                .enc
                .encode_float(&self.pending[..ogg::FRAME], buf)
                .context("opus encode")?;
            self.pending.drain(..ogg::FRAME);
            self.out.packet(buf[..n].to_vec(), false)?;
        }
        Ok(())
    }

    /// Zero-pad the tail to a frame and write it with EOS.
    fn finish(mut self, buf: &mut [u8]) -> anyhow::Result<()> {
        self.encode(buf)?;
        self.pending.resize(ogg::FRAME, 0.0);
        let n = self
            .enc
            .encode_float(&self.pending, buf)
            .context("opus encode")?;
        self.out.packet(buf[..n].to_vec(), true)?;
        Ok(())
    }
}

/// Open the three files (headers written) and start the thread. `Err` when a
/// file or encoder cannot be created; after that the thread never fails, it
/// logs and drops what it cannot write.
pub fn spawn(
    dir: &Path,
    title: &str,
    mic: Consumer<Sample>,
    system: Consumer<Sample>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    level: watch::Sender<(f32, f32)>,
) -> anyhow::Result<JoinHandle<()>> {
    let tracks = [
        Track::open(dir, Stem::Mic, title)?,
        Track::open(dir, Stem::System, title)?,
        Track::open(dir, Stem::Mix, title)?,
    ];
    let mk = || rubato::FftFixedInOut::<f32>::new(RATE as usize, ogg::RATE as usize, IN_FRAME, 1);
    let resamplers = [mk().context("resampler")?, mk().context("resampler")?];
    let thread = std::thread::Builder::new()
        .name("meeting-encode".into())
        .spawn(move || {
            let mut e = Encoder {
                sources: [Source::new(mic), Source::new(system)],
                resamplers,
                tracks: Some(tracks),
                clock: None,
                emitted: 0,
                level,
                packet: vec![0u8; MAX_PACKET],
            };
            if let Err(err) = e.run(&stop, &paused) {
                tracing::error!("meeting encoder: {err:#}");
            }
        })
        .context("spawn encoder thread")?;
    Ok(thread)
}

struct Encoder {
    sources: [Source; 2],
    resamplers: [rubato::FftFixedInOut<f32>; 2],
    tracks: Option<[Track; 3]>,
    /// Starts at the first chunk received from either source.
    clock: Option<RecordingClock>,
    emitted: u64,
    level: watch::Sender<(f32, f32)>,
    packet: Vec<u8>,
}

impl Encoder {
    fn run(&mut self, stop: &AtomicBool, paused: &AtomicBool) -> anyhow::Result<()> {
        let tick = Duration::from_millis(20);
        let mut next = Instant::now() + tick;
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            next += tick;
            let now = Instant::now();
            if paused.load(Ordering::SeqCst) {
                for s in &mut self.sources {
                    s.discard();
                }
                if let Some(c) = &mut self.clock
                    && !c.is_paused()
                {
                    c.pause_at(now);
                    let _ = self.level.send((0.0, 0.0));
                }
                continue;
            }
            if let Some(c) = &mut self.clock {
                c.resume_at(now);
            }
            self.pump(now)?;
        }
        // Producers are gone: what is in the rings is the tail.
        let now = Instant::now();
        self.pump(now)?;
        let tracks = self.tracks.take().expect("finished once");
        for t in tracks {
            t.finish(&mut self.packet)?;
        }
        Ok(())
    }

    /// Drain, then emit every frame due at `now`.
    fn pump(&mut self, now: Instant) -> anyhow::Result<()> {
        let mut first = None;
        for s in &mut self.sources {
            if let Some(at) = s.drain() {
                first = Some(first.map_or(at, |f: Instant| f.min(at)));
            }
        }
        if self.clock.is_none() {
            match first {
                Some(at) => self.clock = Some(RecordingClock::start_at(at)),
                None => return Ok(()),
            }
        }
        let clock = self.clock.as_ref().expect("set above");
        let elapsed = clock.elapsed_at(now).saturating_sub(LAG);
        let due = elapsed.as_millis() as u64 / 20;
        let mut m = [0f32; IN_FRAME];
        let mut s = [0f32; IN_FRAME];
        while self.emitted < due {
            self.sources[0].take(&mut m);
            self.sources[1].take(&mut s);
            let tracks = self.tracks.as_mut().expect("tracks live until finish");
            let m24 = self.resamplers[0]
                .process(&[&m], None)
                .context("resample mic")?;
            let s24 = self.resamplers[1]
                .process(&[&s], None)
                .context("resample system")?;
            tracks[0].pending.extend_from_slice(&m24[0]);
            tracks[1].pending.extend_from_slice(&s24[0]);
            tracks[2]
                .pending
                .extend(m24[0].iter().zip(&s24[0]).map(|(a, b)| {
                    let x = a + b;
                    x / (1.0 + x.abs())
                }));
            for t in tracks.iter_mut() {
                t.encode(&mut self.packet)?;
            }
            self.emitted += 1;
            if self.emitted.is_multiple_of(u64::from(LEVEL_EVERY)) {
                let lv = (
                    self.sources[0].rms(LEVEL_EVERY),
                    self.sources[1].rms(LEVEL_EVERY),
                );
                let _ = self.level.send(lv);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    /// A source that stalls gets zeros for the gap and its later audio lands
    /// where it arrived, not shifted; a source that runs ahead is capped.
    #[test]
    fn stalled_source_gets_silence_and_stays_aligned() {
        let (mut p, c) = RingBuffer::new(64);
        let mut src = Source::new(c);
        let t0 = Instant::now();
        p.push((t0, vec![0.5; IN_FRAME])).unwrap();
        src.drain();
        let mut out = [0f32; IN_FRAME];
        src.take(&mut out);
        assert!(out.iter().all(|&x| x == 0.5));
        // Nothing arrived: a frame of silence.
        src.take(&mut out);
        assert!(out.iter().all(|&x| x == 0.0));
        // The source is back: its audio is emitted now, not delayed behind the gap.
        p.push((t0 + Duration::from_millis(80), vec![0.25; IN_FRAME / 2]))
            .unwrap();
        src.drain();
        src.take(&mut out);
        assert!(out[..IN_FRAME / 2].iter().all(|&x| x == 0.25));
        assert!(
            out[IN_FRAME / 2..].iter().all(|&x| x == 0.0),
            "short chunk padded"
        );
        assert!(src.buf.is_empty());
        // A backlog past the cap is dropped from the front.
        for i in 0..14 {
            p.push((t0, vec![i as f32; IN_FRAME])).unwrap();
        }
        src.drain();
        src.take(&mut out);
        assert_eq!(out[0], 0.0);
        assert_eq!(src.buf.len(), MAX_BACKLOG);
        assert_eq!(
            src.buf[0], 4.0,
            "the newest ten frames survive; 1..=3 dropped"
        );
    }
}
