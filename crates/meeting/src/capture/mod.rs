//! Capture (01 §4): three OS threads, none async. The system thread owns the
//! `flexaudio` process tap; the mic is the VoiceProcessingIO unit (its
//! callback pushes directly) or, as a fallback, `voice::mic` pumped by a
//! thread; the encoder thread drains both rings and is the only writer of
//! the three files. Rings are `rtrb`; every entry is stamped on receipt —
//! the one clock (01 §4.2).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};
use tokio::sync::watch;

use crate::Degradation;

pub mod encode;
pub mod mic;
pub mod system;

/// One ring entry: receipt time and 48 kHz mono samples.
pub type Sample = (Instant, Vec<f32>);
/// Capture rate asked of both devices.
pub const RATE: u32 = 48_000;
/// Ring capacity in chunks: 5 s at 20 ms.
const RING: usize = 250;

/// One source's producers: the encoder ring and the taps (STT, and the live diariser
/// on the system feed). `try_push`, drop on full: a tap without a consumer costs nothing.
pub struct Feed {
    enc: Producer<Sample>,
    taps: Vec<Producer<Sample>>,
    paused: Arc<AtomicBool>,
}

impl Feed {
    /// Stamp and push one chunk to every ring; nothing while paused.
    pub fn push(&mut self, pcm: Vec<f32>) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        for tap in &mut self.taps {
            if tap.slots() > 0 {
                let _ = tap.push((now, pcm.clone()));
            }
        }
        let _ = self.enc.push((now, pcm));
    }

    /// One more ring fed with every chunk; its consumer.
    pub fn tap(&mut self) -> Consumer<Sample> {
        let (p, c) = RingBuffer::new(RING);
        self.taps.push(p);
        c
    }
}

/// One source's feed and its two consumers `(encoder, stt)`; `pub` for `examples/capture_probe`.
pub fn rings(paused: &Arc<AtomicBool>) -> (Feed, Consumer<Sample>, Consumer<Sample>) {
    let (enc_p, enc_c) = RingBuffer::new(RING);
    let mut feed = Feed {
        enc: enc_p,
        taps: Vec::with_capacity(2),
        paused: paused.clone(),
    };
    let stt_c = feed.tap();
    (feed, enc_c, stt_c)
}

/// The running devices and threads of one meeting. Lives inside the actor,
/// so the non-`Sync` handles sit behind a mutex that only `stop` opens.
pub struct Capture {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    devices: std::sync::Mutex<Devices>,
    /// `(mic_rms, system_rms)` at 10 Hz from the encoder.
    pub level: watch::Receiver<(f32, f32)>,
}

struct Devices {
    mic: Option<mic::Mic>,
    /// System thread, then encoder; joined in that order.
    threads: Vec<JoinHandle<()>>,
    /// Phase 2's consumers (mic, system); taken once by `StartMeeting`.
    stt: Option<(Consumer<Sample>, Consumer<Sample>)>,
    /// 07: the live diariser's system consumer; taken once.
    diar: Option<Consumer<Sample>>,
}

impl Capture {
    /// Open the files, the tap on `pid` and the mic; blocking (TCC prompts
    /// included). Returns what degraded on the way (`MicNoAec`). On `Err` the
    /// files that were opened are finalized and left in `dir`.
    pub fn start(pid: u32, dir: &Path, title: &str) -> anyhow::Result<(Capture, Vec<Degradation>)> {
        let stop = Arc::new(AtomicBool::new(false));
        // Paused until every device is open: the file timeline then starts
        // with the actor's recording clock, not with the first device up.
        let paused = Arc::new(AtomicBool::new(true));
        let (mic_feed, mic_enc, mic_stt) = rings(&paused);
        let (mut sys_feed, sys_enc, sys_stt) = rings(&paused);
        let sys_diar = sys_feed.tap();
        let (level_tx, level) = watch::channel((0.0, 0.0));
        // Encoder first: the files exist before any device opens.
        let encoder = encode::spawn(
            dir,
            title,
            mic_enc,
            sys_enc,
            stop.clone(),
            paused.clone(),
            level_tx,
        )?;
        let mut cap = Capture {
            stop,
            paused,
            devices: std::sync::Mutex::new(Devices {
                mic: None,
                threads: vec![encoder],
                stt: Some((mic_stt, sys_stt)),
                diar: Some(sys_diar),
            }),
            level,
        };
        let system = match system::spawn(pid, sys_feed, cap.stop.clone()) {
            Ok(t) => t,
            Err(e) => {
                cap.stop();
                return Err(e);
            }
        };
        cap.devices
            .get_mut()
            .expect("unshared")
            .threads
            .insert(0, system);
        let (mic, degraded) = match mic::open(mic_feed, cap.stop.clone()) {
            Ok(m) => m,
            Err(e) => {
                cap.stop();
                return Err(e);
            }
        };
        cap.devices.get_mut().expect("unshared").mic = Some(mic);
        cap.resume();
        Ok((cap, degraded))
    }

    /// Producers stop pushing, the encoder stops draining; devices stay open.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume pushing and draining.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// The two STT ring consumers `(mic, system)`, once; `None` after.
    pub fn take_stt(&self) -> Option<(Consumer<Sample>, Consumer<Sample>)> {
        self.devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stt
            .take()
    }

    /// The live diariser's system ring consumer, once; `None` after.
    pub fn take_diar(&self) -> Option<Consumer<Sample>> {
        self.devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .diar
            .take()
    }

    /// The flag `stop` sets; the STT tasks exit on it.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// The flag `pause`/`resume` flip; the STT tasks stop feeding on it.
    pub fn paused_flag(&self) -> Arc<AtomicBool> {
        self.paused.clone()
    }

    /// Close the devices and join the threads; the encoder writes the EOS pages.
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let mut d = self.devices.into_inner().unwrap_or_else(|e| e.into_inner());
        drop(d.mic.take());
        for t in d.threads.drain(..) {
            let _ = t.join();
        }
    }
}
