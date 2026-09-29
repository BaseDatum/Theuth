//! The microphone (01 §4.2): `voice::mic::Capture` opened with
//! `Aec::Vpio` — Apple's VoiceProcessingIO unit (echo cancellation, AGC and
//! noise suppression against whatever the default output device plays, 53
//! §0.4) where it opens, the plain default input (no AEC) where it does not;
//! the wrapper lives in `voice` since 76 §0.5. Either delivers 48 kHz mono
//! chunks over an mpsc; one pump thread stamps them into the rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use voice::mic::Aec;

use super::Feed;
use crate::{Degradation, PermissionState};

/// The open microphone. Dropping it closes the device and joins the pump.
pub struct Mic {
    capture: Option<voice::mic::Capture>,
    pump: Option<JoinHandle<()>>,
}

impl Drop for Mic {
    fn drop(&mut self) {
        drop(self.capture.take());
        // The sender is gone now; the pump sees Disconnected.
        if let Some(t) = self.pump.take() {
            let _ = t.join();
        }
    }
}

/// Open the mic; blocking (the TCC prompt fires here on first use). `Err`
/// when neither path opens.
pub fn open(feed: Feed, stop: Arc<AtomicBool>) -> anyhow::Result<(Mic, Vec<Degradation>)> {
    let (cap, rx, aec) =
        voice::mic::Capture::open_with(Aec::Vpio).map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let degraded = match aec {
        Aec::Vpio => Vec::new(),
        Aec::None => vec![Degradation::MicNoAec],
    };
    Ok((
        Mic {
            capture: Some(cap),
            pump: Some(pump(feed, rx, stop)?),
        },
        degraded,
    ))
}

/// Chunks over the mpsc → rings, until stopped or the device is gone.
fn pump(
    mut feed: Feed,
    rx: Receiver<voice::mic::Chunk>,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<JoinHandle<()>> {
    Ok(std::thread::Builder::new()
        .name("meeting-mic".into())
        .spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(Ok(pcm)) => feed.push(pcm),
                    Ok(Err(e)) => {
                        tracing::warn!("microphone stopped: {e}");
                        return;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })?)
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
