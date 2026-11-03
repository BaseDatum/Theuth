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
use tokio::sync::{mpsc, watch};

use crate::{CaptureFault, Degradation, ProcessInfo, Track};

pub mod encode;
pub mod mic;
pub mod system;
pub mod watchdog;

/// 173: what a capture thread's watchdog tells the actor (drained on its 1 s tick and at stop).
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// The system track switched to the global tap at `start` (R2-3: until [`Report::Returned`]).
    Fallback {
        /// When.
        start: Instant,
        /// The episode's reason.
        reason: CaptureFault,
    },
    /// A return probe found the app's tap live: back on it at `end`, tapping `pid`.
    Returned {
        /// When.
        end: Instant,
        /// The pid now tapped.
        pid: u32,
    },
    /// `track` was silence-padded over `start..end` (a manifest capture gap).
    Gap {
        /// Which track.
        track: Track,
        /// First silent moment.
        start: Instant,
        /// Audio back, or the meeting ended.
        end: Instant,
        /// Why.
        reason: CaptureFault,
        /// The pid the system tap moved to.
        retarget: Option<u32>,
    },
    /// `track`'s user-facing health changed.
    Health {
        /// Which track.
        track: Track,
        /// The new health.
        health: watchdog::Health,
    },
    /// Something degraded for the rest of the meeting.
    Degraded(Degradation),
}

/// The capture threads' end of the report channel.
pub type Reports = mpsc::UnboundedSender<Report>;

/// The watchdog's notes for `track` → reports.
fn forward(track: Track, wd: &mut watchdog::Watchdog, reports: &Reports) {
    for n in wd.take_notes() {
        let r = match n {
            watchdog::Note::Gap {
                start,
                end,
                reason,
                retarget,
            } => Report::Gap {
                track,
                start,
                end,
                reason,
                retarget,
            },
            watchdog::Note::Health(health) => Report::Health { track, health },
        };
        let _ = reports.send(r);
    }
}

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
    /// 173: the capture threads' watchdog reports.
    reports: mpsc::UnboundedReceiver<Report>,
}

impl Capture {
    /// Open the files, the tap on `process` and the mic; blocking (TCC prompts
    /// included). Returns what degraded on the way (`MicNoAec`). On `Err` the
    /// files that were opened are finalized and left in `dir`. The capture
    /// threads log under a `meeting` span.
    pub fn start(
        process: &ProcessInfo,
        meeting: crate::MeetingId,
        dir: &Path,
        title: &str,
    ) -> anyhow::Result<(Capture, Vec<Degradation>)> {
        let span = tracing::info_span!("capture", %meeting);
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
        let (reports_tx, reports) = mpsc::unbounded_channel();
        let mut cap = Capture {
            stop,
            paused,
            devices: std::sync::Mutex::new(Devices {
                mic: None,
                threads: vec![encoder],
                stt: Some((mic_stt, sys_stt)),
                diar: Some(sys_diar),
                reports,
            }),
            level,
        };
        let system = match system::spawn(
            process.pid,
            process.bundle_id.clone(),
            sys_feed,
            cap.stop.clone(),
            reports_tx.clone(),
            span.clone(),
        ) {
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
        let (mic, degraded) = match mic::open(mic_feed, cap.stop.clone(), reports_tx, span) {
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

    /// 173: the next pending watchdog report, if any.
    pub fn try_report(&self) -> Option<Report> {
        self.devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reports
            .try_recv()
            .ok()
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
    /// The reports not yet drained (the episodes the stop closed among them).
    pub fn stop(self) -> Vec<Report> {
        self.stop.store(true, Ordering::SeqCst);
        let mut d = self.devices.into_inner().unwrap_or_else(|e| e.into_inner());
        drop(d.mic.take());
        for t in d.threads.drain(..) {
            let _ = t.join();
        }
        std::iter::from_fn(|| d.reports.try_recv().ok()).collect()
    }
}
