//! The daemon's speaker (76 §0.3, §4.3): the default output through `cpal`,
//! fed 24 kHz mono `f32` straight from the speak task, one bounded ring
//! between them — the run-ahead cap the webview lacks. [`Ring`] is the pure
//! part (ring, ledger, position arithmetic) the device callback drives;
//! [`Player`] owns the device on its own `std::thread`, like `mic::Capture`,
//! opened per use and closed on drop, never at prewarm.
//!
//! Position: `read` counts samples the callback has taken (plus what a flush
//! skipped); `position()` is that minus the output latency the backend
//! reports through `OutputCallbackInfo` (`playback - callback`). What cpal
//! 0.16 reports, read from its source: CoreAudio and WASAPI = one callback
//! buffer's duration (a fixed estimate, not the device's own latency
//! property); ALSA = `snd_pcm_delay`. `0`/unknown → 40 ms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::SpeechId;

/// What the speak task writes: 24 kHz mono.
pub const RATE: u32 = crate::speak::SAMPLE_RATE;
/// One Mimi frame at [`RATE`]; `Played::seq` counts these, like `SpeechAudio::seq`.
const FRAME: u64 = 1920;
/// Resampler input block when the device refuses 24 kHz: 20 ms.
const BLOCK: usize = RATE as usize / 50;
/// Output latency assumed when the backend reports none.
const DEFAULT_LATENCY: Duration = Duration::from_millis(40);
/// How long a blocked `write` sleeps before rechecking the ring.
const WAIT: Duration = Duration::from_millis(2);
/// Ledger entries older than this behind the read cursor are pruned.
const KEEP: Duration = Duration::from_secs(2);

/// Where playback is (76 §4.3), in [`RATE`] samples.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Played {
    /// Utterance heard at this position; `None` when the speaker is idle.
    pub id: Option<SpeechId>,
    /// 80 ms frame of `id` heard at this position (0-based).
    pub seq: u32,
    /// Output samples heard since open: data, silence between utterances, and
    /// flushed samples (counted as passed, not heard — their entries are `dropped`).
    pub samples: u64,
}

/// One utterance's span in the ring's sample space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Utterance.
    pub id: SpeechId,
    /// First sample written.
    pub first: u64,
    /// One past the last sample written so far.
    pub last: u64,
    /// A flush cut it before `last` was heard.
    pub dropped: bool,
    /// Flush generation it was written under; a later generation retires it.
    epoch: u64,
}

struct Ledger {
    epoch: u64,
    entries: Vec<Entry>,
}

/// The ring, the ledger and the counters, in device-rate samples. One
/// producer (`push`), one consumer (`fill`), any number of observers.
pub struct Ring {
    /// `f32` bits; the callback reads, the writer writes, never the same slot at once.
    buf: Box<[AtomicU32]>,
    /// Samples ever pushed.
    written: AtomicU64,
    /// Samples the callback has taken, plus what a flush skipped. Monotone.
    read: AtomicU64,
    /// Silence frames the callback emitted on an empty ring (not while paused):
    /// with `read` they place the ledger on the output timeline.
    silence: AtomicU64,
    paused: AtomicBool,
    /// Output latency in samples, from the last callback; 0 = not reported.
    latency: AtomicU64,
    /// Frames of the last callback, for the bench.
    callback_frames: AtomicU32,
    rate: u32,
    /// Producer-side; the callback never locks it.
    ledger: Mutex<Ledger>,
}

/// `push` for an utterance a flush already retired.
#[derive(Debug, PartialEq, Eq)]
pub struct Retired;

impl Ring {
    /// `capacity` samples at `rate`.
    pub fn new(rate: u32, capacity: usize) -> Ring {
        Ring {
            buf: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            silence: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            latency: AtomicU64::new(0),
            callback_frames: AtomicU32::new(0),
            rate,
            ledger: Mutex::new(Ledger {
                epoch: 0,
                entries: Vec::new(),
            }),
        }
    }

    /// Ring size in samples.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Append up to the free space of `pcm` for `id`; the count taken, or
    /// [`Retired`] when a flush cut `id` (nothing is written then).
    pub fn push(&self, id: SpeechId, pcm: &[f32]) -> Result<usize, Retired> {
        let mut ledger = self.ledger.lock();
        let epoch = ledger.epoch;
        let written = self.written.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        // Output-timeline offset of the next sample: data played + silence emitted.
        let at = written + self.silence.load(Ordering::Acquire);
        let horizon = (read + self.silence.load(Ordering::Acquire))
            .saturating_sub(u64::from(self.rate) * KEEP.as_secs());
        ledger.entries.retain(|e| e.last > horizon || e.id == id);
        let entry = match ledger.entries.iter_mut().find(|e| e.id == id) {
            Some(e) if e.dropped || e.epoch != epoch => return Err(Retired),
            Some(e) => e,
            None => {
                ledger.entries.push(Entry {
                    id,
                    first: at,
                    last: at,
                    dropped: false,
                    epoch,
                });
                ledger.entries.last_mut().expect("pushed")
            }
        };
        let cap = self.buf.len() as u64;
        let free = cap - (written - read);
        let n = pcm.len().min(free as usize);
        for (i, &s) in pcm[..n].iter().enumerate() {
            self.buf[((written + i as u64) % cap) as usize].store(s.to_bits(), Ordering::Relaxed);
        }
        self.written.store(written + n as u64, Ordering::Release);
        entry.last = at + n as u64;
        Ok(n)
    }

    /// The device callback: `out` is `channels`-interleaved; the mono sample
    /// is copied to every channel. Silence while paused or empty.
    pub fn fill(&self, out: &mut [f32], channels: usize, latency: Option<Duration>) {
        let channels = channels.max(1);
        let frames = out.len() / channels;
        self.callback_frames.store(frames as u32, Ordering::Relaxed);
        if let Some(l) = latency {
            self.latency.store(self.samples(l), Ordering::Relaxed);
        }
        if self.paused.load(Ordering::Relaxed) {
            out.fill(0.0);
            return;
        }
        let read = self.read.load(Ordering::Acquire);
        let avail = self.written.load(Ordering::Acquire) - read;
        let n = frames.min(avail as usize);
        let cap = self.buf.len() as u64;
        for (i, frame) in out.chunks_exact_mut(channels).enumerate() {
            let s = if i < n {
                f32::from_bits(self.buf[((read + i as u64) % cap) as usize].load(Ordering::Relaxed))
            } else {
                0.0
            };
            frame.fill(s);
        }
        // `fetch_max`, not `store`: a concurrent flush may already have moved the cursor past us.
        self.read.fetch_max(read + n as u64, Ordering::AcqRel);
        if n < frames {
            self.silence
                .fetch_add((frames - n) as u64, Ordering::AcqRel);
        }
    }

    /// Zero-fill from the next callback on; the cursor holds.
    pub fn pause(&self) -> Played {
        self.paused.store(true, Ordering::Relaxed);
        self.played()
    }

    /// Continue from the paused sample.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    /// Drop everything buffered, retire every utterance in the ring (their
    /// later `push`es fail) and unpause. Returns the position before the cut.
    pub fn flush(&self) -> Played {
        let mut ledger = self.ledger.lock();
        let p = self.position_dev();
        let before = self.played_at(&ledger, p);
        ledger.epoch += 1;
        let written = self.written.load(Ordering::Acquire);
        self.read.fetch_max(written, Ordering::AcqRel);
        for e in ledger.entries.iter_mut().filter(|e| e.last > p) {
            e.dropped = true;
        }
        self.paused.store(false, Ordering::Relaxed);
        before
    }

    /// Output-timeline position heard: data played + silence emitted − output latency (device rate).
    fn position_dev(&self) -> u64 {
        let latency = match self.latency.load(Ordering::Relaxed) {
            0 => self.samples(DEFAULT_LATENCY),
            l => l,
        };
        (self.read.load(Ordering::Acquire) + self.silence.load(Ordering::Acquire))
            .saturating_sub(latency)
    }

    /// Where playback is, with the utterance and frame heard there.
    pub fn played(&self) -> Played {
        self.played_at(&self.ledger.lock(), self.position_dev())
    }

    fn played_at(&self, ledger: &Ledger, p: u64) -> Played {
        let hit = ledger
            .entries
            .iter()
            .find(|e| !e.dropped && e.first <= p && p < e.last);
        let frame = FRAME * u64::from(self.rate) / u64::from(RATE);
        Played {
            id: hit.map(|e| e.id),
            seq: hit.map_or(0, |e| ((p - e.first) / frame) as u32),
            samples: p * u64::from(RATE) / u64::from(self.rate),
        }
    }

    /// Every utterance still in the ledger, oldest first (device-rate positions).
    pub fn ledger(&self) -> Vec<Entry> {
        self.ledger.lock().entries.clone()
    }

    /// Output latency the last callback reported; `None` until one did or when the backend reports 0.
    pub fn latency(&self) -> Option<Duration> {
        match self.latency.load(Ordering::Relaxed) {
            0 => None,
            l => Some(Duration::from_secs_f64(l as f64 / f64::from(self.rate))),
        }
    }

    /// Frames per callback, as last observed (0 before the first).
    pub fn callback_frames(&self) -> u32 {
        self.callback_frames.load(Ordering::Relaxed)
    }

    /// Samples ever written (device rate).
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    fn samples(&self, d: Duration) -> u64 {
        (d.as_secs_f64() * f64::from(self.rate)).round() as u64
    }
}

/// 24 kHz → device rate, producer side, when the device refused 24 kHz.
struct Resample {
    inner: rubato::Fft<f32>,
    /// Input samples short of a block, waiting for the next `write`.
    pending: Vec<f32>,
}

/// An open speaker. Dropping it stops the thread and closes the device.
pub struct Player {
    ring: Arc<Ring>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    resample: Mutex<Option<Resample>>,
    /// Device name.
    pub device: String,
    /// Rate the device runs at (24 000 unless it refused).
    pub rate: u32,
    /// Output channels; the mono signal goes to all of them.
    pub channels: u16,
}

impl Player {
    /// Open `device` (by name; `None` = the default output) with a ring of
    /// `buffer_ms`. Returns once the stream is playing.
    pub fn open(device: Option<&str>, buffer_ms: u32) -> anyhow::Result<Player> {
        let host = cpal::default_host();
        let dev = match device {
            Some(name) => host
                .output_devices()?
                .find(|d| d.name().is_ok_and(|n| n == name))
                .ok_or_else(|| anyhow::anyhow!("no output device named {name:?}"))?,
            None => host
                .default_output_device()
                .ok_or_else(|| anyhow::anyhow!("no default output device"))?,
        };
        let name = dev.name().unwrap_or_else(|_| "?".into());
        let cfg = pick(&dev)?;
        let rate = cfg.sample_rate.0;
        let channels = cfg.channels;
        let ring = Arc::new(Ring::new(
            rate,
            (u64::from(rate) * u64::from(buffer_ms.max(20)) / 1000) as usize,
        ));
        let resample = if rate == RATE {
            None
        } else {
            Some(Resample {
                inner: rubato::Fft::new(
                    RATE as usize,
                    rate as usize,
                    BLOCK,
                    1,
                    rubato::FixedSync::Input,
                )?,
                pending: Vec::with_capacity(BLOCK),
            })
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let thread = std::thread::Builder::new()
            .name("voice-out".into())
            .spawn({
                let (ring, stop) = (ring.clone(), stop.clone());
                move || run(&dev, &cfg, ring, &stop, &ready_tx)
            })?;
        let mut player = Player {
            ring,
            stop,
            thread: Some(thread),
            resample: Mutex::new(resample),
            device: name,
            rate,
            channels,
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(player),
            Ok(Err(e)) => {
                player.join();
                anyhow::bail!("speaker: {e}")
            }
            Err(_) => {
                player.join();
                anyhow::bail!("speaker: output thread exited before starting")
            }
        }
    }

    /// Queue `pcm` (24 kHz mono) for `id`, blocking while the ring is full.
    /// `false`: a flush retired `id` — stop writing it.
    pub fn write(&self, id: SpeechId, pcm: &[f32]) -> bool {
        let mut guard = self.resample.lock();
        let dev: Vec<f32>;
        let pcm = match guard.as_mut() {
            None => pcm,
            Some(r) => {
                r.pending.extend_from_slice(pcm);
                let mut out =
                    Vec::with_capacity(r.pending.len() * self.rate as usize / RATE as usize + 8);
                let mut at = 0;
                while r.pending.len() - at >= BLOCK {
                    match crate::stt::resample_block(&mut r.inner, &r.pending[at..at + BLOCK]) {
                        Ok(b) => out.extend_from_slice(&b),
                        Err(e) => {
                            tracing::warn!("speaker resampler: {e}");
                            return false;
                        }
                    }
                    at += BLOCK;
                }
                r.pending.drain(..at);
                dev = out;
                &dev
            }
        };
        drop(guard);
        let mut off = 0;
        while off < pcm.len() {
            match self.ring.push(id, &pcm[off..]) {
                Err(Retired) => return false,
                Ok(0) => std::thread::sleep(WAIT),
                Ok(n) => off += n,
            }
        }
        true
    }

    /// Silence from the next callback; where playback was.
    pub fn pause(&self) -> Played {
        self.ring.pause()
    }

    /// Continue from the paused sample.
    pub fn resume(&self) {
        self.ring.resume()
    }

    /// Empty the ring and retire every utterance in it; where playback was.
    pub fn flush(&self) -> Played {
        if let Some(r) = self.resample.lock().as_mut() {
            r.pending.clear();
        }
        self.ring.flush()
    }

    /// Where playback is now (76 §4.3).
    pub fn position(&self) -> Played {
        self.ring.played()
    }

    /// The ring, for the bench and for callers that want the ledger.
    pub fn ring(&self) -> &Ring {
        &self.ring
    }

    fn join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.join();
    }
}

/// An `f32` config at 24 kHz if the device offers one, else its default
/// config's rate and channel count (resampled producer-side).
fn pick(dev: &cpal::Device) -> anyhow::Result<cpal::StreamConfig> {
    let default = dev.default_output_config()?;
    let mut ranges: Vec<_> = dev
        .supported_output_configs()?
        .filter(|r| r.sample_format() == cpal::SampleFormat::F32)
        .collect();
    // Prefer the default channel count so the OS mixer path stays the plain one.
    ranges.sort_by_key(|r| r.channels() != default.channels());
    let at_rate = ranges
        .iter()
        .find_map(|r| r.try_with_sample_rate(cpal::SampleRate(RATE)));
    let cfg = match at_rate {
        Some(c) => c,
        None if default.sample_format() == cpal::SampleFormat::F32 => default,
        None => anyhow::bail!(
            "output device offers no f32 format (default is {:?})",
            default.sample_format()
        ),
    };
    Ok(cpal::StreamConfig {
        channels: cfg.channels(),
        sample_rate: cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    })
}

/// The output thread: build + play the stream, hold it until stopped.
fn run(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    ring: Arc<Ring>,
    stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
) {
    let channels = usize::from(cfg.channels);
    let stream = dev.build_output_stream(
        cfg,
        move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            ring.fill(out, channels, ts.playback.duration_since(&ts.callback));
        },
        |e| tracing::warn!("speaker stream: {e}"),
        None,
    );
    let stream = match stream
        .map_err(|e| e.to_string())
        .and_then(|s| s.play().map(|()| s).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 24 kHz ring, 200 ms: writes block at the cap, the callback drains in order.
    #[test]
    fn ring_bounds_and_order() {
        let ring = Ring::new(RATE, 4_800);
        let id = SpeechId::new();
        let pcm: Vec<f32> = (0..6_000).map(|i| i as f32).collect();
        assert_eq!(ring.push(id, &pcm), Ok(4_800));
        assert_eq!(ring.push(id, &pcm[4_800..]), Ok(0));
        let mut out = vec![9.0; 2 * 1_000];
        ring.fill(&mut out, 2, None);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[1_998], 999.0);
        assert_eq!(ring.push(id, &pcm[4_800..]), Ok(1_000));
        let mut out = vec![9.0; 6_000];
        ring.fill(&mut out, 1, None);
        assert_eq!(out[0], 1_000.0);
        assert_eq!(out[4_799], 5_799.0);
        assert_eq!(out[4_800], 0.0, "silence past the data");
        assert_eq!(ring.written(), 5_800);
    }

    /// Position = read − latency (40 ms default, or what the callback reports);
    /// the ledger names the utterance and 80 ms frame at that position.
    #[test]
    fn position_and_ledger() {
        let ring = Ring::new(RATE, 48_000);
        let (a, b) = (SpeechId::new(), SpeechId::new());
        ring.push(a, &vec![0.1; 3_840]).unwrap(); // two frames
        ring.push(b, &vec![0.2; 1_920]).unwrap();
        let mut out = vec![0.0; 3_000];
        ring.fill(&mut out, 1, None);
        // read 3000 − 960 (40 ms) = 2040 → frame 1 of `a`.
        assert_eq!(
            ring.played(),
            Played {
                id: Some(a),
                seq: 1,
                samples: 2_040
            }
        );
        // The remaining 2760 data samples, with the backend now reporting 10 ms.
        ring.fill(&mut out[..2_760], 1, Some(Duration::from_millis(10)));
        // read 5760 − 240 = 5520 → 1680 into `b`, frame 0.
        assert_eq!(
            ring.played(),
            Played {
                id: Some(b),
                seq: 0,
                samples: 5_520
            }
        );
        assert_eq!(ring.latency(), Some(Duration::from_millis(10)));
        // Empty ring: 3000 frames of silence advance the timeline past `b`.
        ring.fill(&mut out, 1, Some(Duration::from_millis(10)));
        assert_eq!(
            ring.played(),
            Played {
                id: None,
                seq: 0,
                samples: 8_520
            },
            "past the last sample: idle"
        );
        let ledger = ring.ledger();
        assert_eq!(ledger.len(), 2);
        assert_eq!((ledger[0].first, ledger[0].last), (0, 3_840));
        assert_eq!((ledger[1].first, ledger[1].last), (3_840, 5_760));
    }

    /// Pause holds the cursor and zero-fills; resume continues from the same
    /// sample; flush drops the buffer, retires the utterance and reports the
    /// pre-cut position.
    #[test]
    fn pause_resume_flush() {
        let ring = Ring::new(RATE, 48_000);
        let a = SpeechId::new();
        let pcm: Vec<f32> = (1..=9_600).map(|i| i as f32).collect();
        ring.push(a, &pcm).unwrap();
        let mut out = vec![7.0; 1_920];
        ring.fill(&mut out, 1, None);
        let at = ring.pause();
        assert_eq!(at.samples, 1_920 - 960);
        ring.fill(&mut out, 1, None);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(ring.played(), at, "paused: nothing moved");
        ring.resume();
        ring.fill(&mut out, 1, None);
        assert_eq!(out[0], 1_921.0, "resumed from the paused sample");
        let before = ring.flush();
        assert_eq!(before.samples, 3_840 - 960);
        assert_eq!(before.id, Some(a));
        ring.fill(&mut out, 1, None);
        assert!(out.iter().all(|&s| s == 0.0), "flushed: silence");
        assert_eq!(
            ring.push(a, &pcm),
            Err(Retired),
            "the cut utterance may not continue"
        );
        assert!(ring.ledger()[0].dropped);
        let b = SpeechId::new();
        assert_eq!(
            ring.push(b, &pcm[..3_000]),
            Ok(3_000),
            "a new utterance writes after the flush"
        );
        assert_eq!(
            ring.ledger()[1].first,
            9_600 + 1_920,
            "placed after the silence"
        );
        ring.fill(&mut out[..100], 1, None);
        assert_eq!(out[0], 1.0);
        assert_eq!(
            ring.played().id,
            None,
            "100 samples in, still inside the 40 ms latency window"
        );
        ring.fill(&mut out, 1, None);
        // read 11 620 + silence 1 920 − 960 = 12 580, inside `b` [11 520, 14 520).
        assert_eq!(
            ring.played(),
            Played {
                id: Some(b),
                seq: 0,
                samples: 12_580
            },
            "heard once the latency window passed"
        );
    }

    /// Pausing then flushing unpauses (a barge-in that commits after a hold).
    #[test]
    fn flush_unpauses() {
        let ring = Ring::new(RATE, 4_800);
        ring.pause();
        ring.flush();
        ring.push(SpeechId::new(), &[1.0; 10]).unwrap();
        let mut out = [0.0; 10];
        ring.fill(&mut out, 1, None);
        assert_eq!(out[0], 1.0);
    }
}
