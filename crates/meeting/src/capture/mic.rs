//! The microphone (01 §4.2): `voice::mic::Capture` opened with
//! `Aec::Vpio` — Apple's VoiceProcessingIO unit (echo cancellation, AGC and
//! noise suppression against whatever the default output device plays, 53
//! §0.4) where it opens, the plain default input (no AEC) where it does not;
//! the wrapper lives in `voice` since 76 §0.5. Either delivers 48 kHz mono
//! chunks over an mpsc; one pump thread stamps them into the rings. 173: the
//! pump owns the device and, under the watchdog's stall rule (no exact-zero
//! rule: a muted mic is legitimately zero), reopens it when chunks stop or the
//! device fails, for as long as the meeting runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use voice::mic::Aec;

use super::{Feed, Report, Reports, watchdog};
use crate::{Degradation, PermissionState, Track};

/// The open microphone: its pump thread, which owns the device. Dropping it
/// (after the meeting's stop flag is set) joins the pump, which closes the device.
pub struct Mic {
    pump: Option<JoinHandle<()>>,
}

impl Drop for Mic {
    fn drop(&mut self) {
        if let Some(t) = self.pump.take() {
            let _ = t.join();
        }
    }
}

/// Open the mic; blocking (the TCC prompt fires here on first use). `Err`
/// when neither path opens. The pump logs under `span` and reports to `reports`.
pub fn open(
    feed: Feed,
    stop: Arc<AtomicBool>,
    reports: Reports,
    span: tracing::Span,
) -> anyhow::Result<(Mic, Vec<Degradation>)> {
    let (cap, rx, aec) =
        voice::mic::Capture::open_with(Aec::Vpio).map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let degraded = match aec {
        Aec::Vpio => Vec::new(),
        Aec::None => vec![Degradation::MicNoAec],
    };
    let pump = std::thread::Builder::new()
        .name("meeting-mic".into())
        .spawn(move || {
            let _span = span.entered();
            pump((cap, rx), aec, feed, &stop, &reports);
        })?;
    Ok((Mic { pump: Some(pump) }, degraded))
}

/// Chunks over the mpsc → rings until stopped; the device is reopened whenever
/// the watchdog says so (stall, device error).
fn pump(
    device: (voice::mic::Capture, Receiver<voice::mic::Chunk>),
    mut aec: Aec,
    mut feed: Feed,
    stop: &AtomicBool,
    reports: &Reports,
) {
    const TICK: Duration = Duration::from_millis(250);
    const LINE: Duration = Duration::from_secs(60);

    let mut device = Some(device);
    let start = Instant::now();
    let mut wd = watchdog::Watchdog::new(
        start,
        watchdog::Rules {
            zeros: false,
            fallback: false,
            probe: false,
        },
    );
    let mut meter = watchdog::Meter::default();
    let (mut last_tick, mut last_line) = (start, start);
    while !stop.load(Ordering::SeqCst) {
        let got = match &device {
            Some((_, rx)) => rx.recv_timeout(Duration::from_millis(50)),
            None => {
                std::thread::sleep(Duration::from_millis(50));
                Err(RecvTimeoutError::Timeout)
            }
        };
        let now = Instant::now();
        match got {
            Ok(Ok(pcm)) => {
                meter.add(&pcm);
                wd.chunk(now, false);
                feed.push(pcm);
            }
            Ok(Err(e)) => {
                tracing::warn!("meeting capture: microphone stopped: {e}");
                drop(device.take());
                wd.signal(now, watchdog::Signal::Lost);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                tracing::warn!("meeting capture: microphone thread gone");
                drop(device.take());
                wd.signal(now, watchdog::Signal::Lost);
            }
        }
        if now.duration_since(last_tick) < TICK {
            continue;
        }
        last_tick = now;
        // Mic rules ask for reopens only (no zero rule, no probe, no fallback).
        if wd.tick(now, &mut || watchdog::Activity::Unknown) == Some(watchdog::Action::Reopen) {
            drop(device.take());
            match voice::mic::Capture::open_with(Aec::Vpio) {
                Ok((cap, rx, opened)) => {
                    tracing::warn!(
                        ?opened,
                        reopens = wd.reopens,
                        "meeting capture: microphone reopened"
                    );
                    if opened == Aec::None && aec == Aec::Vpio {
                        let _ = reports.send(Report::Degraded(Degradation::MicNoAec));
                    }
                    aec = opened;
                    device = Some((cap, rx));
                    wd.reopened(Instant::now(), true, None);
                }
                Err(e) => {
                    tracing::warn!("meeting capture: microphone reopen failed: {e:#}");
                    wd.reopened(Instant::now(), false, None);
                }
            }
        }
        super::forward(Track::Mic, &mut wd, reports);
        if now.duration_since(last_line) >= LINE {
            last_line = now;
            let target = format!("{aec:?}");
            super::system::health_line("mic", &target, "-", &meter, wd.reopens, wd.troubled());
            meter = watchdog::Meter::default();
        }
    }
    drop(device);
    wd.finish(Instant::now());
    super::forward(Track::Mic, &mut wd, reports);
}

/// Microphone TCC state without prompting (`AVCaptureDevice`).
#[cfg(target_os = "macos")]
pub fn permission() -> PermissionState {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    // SAFETY: `AVMediaTypeAudio` is a framework constant; the class method has no preconditions.
    let status = unsafe {
        match AVMediaTypeAudio {
            Some(t) => AVCaptureDevice::authorizationStatusForMediaType(t),
            None => return PermissionState::NotDetermined,
        }
    };
    match status {
        AVAuthorizationStatus::Authorized => PermissionState::Granted,
        AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => {
            PermissionState::Denied
        }
        _ => PermissionState::NotDetermined,
    }
}

/// Not macOS: no TCC.
#[cfg(not(target_os = "macos"))]
pub fn permission() -> PermissionState {
    PermissionState::Granted
}
