//! The microphone (01 §4.2): Apple's VoiceProcessingIO unit via `coreaudio-rs`
//! — echo cancellation, AGC and noise suppression against whatever the default
//! output device plays (53 §0.4) — or, when it cannot be opened,
//! `voice::mic::Capture` (plain default input, no AEC). Either delivers
//! 48 kHz mono chunks over an mpsc; one pump thread stamps them into the rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::Feed;
use crate::{Degradation, PermissionState};

/// The open microphone. Dropping it closes the device and joins the pump.
pub struct Mic {
    #[cfg(target_os = "macos")]
    unit: Option<coreaudio::audio_unit::AudioUnit>,
    fallback: Option<voice::mic::Capture>,
    pump: Option<JoinHandle<()>>,
}

impl Drop for Mic {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        drop(self.unit.take());
        drop(self.fallback.take());
        // Both senders are gone now; the pump sees Disconnected.
        if let Some(t) = self.pump.take() {
            let _ = t.join();
        }
    }
}

/// Open the mic; blocking (the TCC prompt fires here on first use). `Err`
/// when neither path opens.
pub fn open(feed: Feed, stop: Arc<AtomicBool>) -> anyhow::Result<(Mic, Vec<Degradation>)> {
    #[cfg(target_os = "macos")]
    match vpio::open() {
        Ok((unit, rx)) => {
            return Ok((
                Mic {
                    unit: Some(unit),
                    fallback: None,
                    pump: Some(pump(feed, rx, stop)?),
                },
                Vec::new(),
            ));
        }
        Err(e) => tracing::warn!("VoiceProcessingIO unavailable, plain mic without AEC: {e:#}"),
    }
    let (cap, rx) =
        voice::mic::Capture::open().map_err(|e| anyhow::anyhow!("microphone: {e:#}"))?;
    Ok((
        Mic {
            #[cfg(target_os = "macos")]
            unit: None,
            fallback: Some(cap),
            pump: Some(pump(feed, rx, stop)?),
        },
        vec![Degradation::MicNoAec],
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

#[cfg(target_os = "macos")]
mod vpio {
    //! Configured the way Chromium configures the voice processing unit
    //! (`media/audio/apple/audio_low_latency_input.cc`, read 2026-09-20):
    //! `kAudioOutputUnitProperty_EnableIO` is not writable on VPIO (IO is
    //! always on), the output bus needs a render callback (silence) for the
    //! unit to run, both bus formats are set before `AudioUnitInitialize`.

    use std::sync::mpsc::{self, Receiver};

    use coreaudio::audio_unit::audio_format::LinearPcmFlags;
    use coreaudio::audio_unit::render_callback::{self, data};
    use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};

    use crate::capture::RATE;

    /// `kAUVoiceIOProperty_BypassVoiceProcessing` (AudioUnitProperties.h); 0 = AEC + AGC + NS on.
    const BYPASS_VOICE_PROCESSING: u32 = 2100;
    /// `kAUVoiceIOProperty_OtherAudioDuckingConfiguration` (macOS 14+): how much
    /// the unit turns down everything else the device plays while it runs.
    const OTHER_AUDIO_DUCKING: u32 = 2108;
    /// `kAUVoiceIOOtherAudioDuckingLevelMin`.
    const DUCKING_MIN: u32 = 10;

    /// `AUVoiceIOOtherAudioDuckingConfiguration`: `Boolean` (u8) + level.
    #[repr(C)]
    struct Ducking {
        enable_advanced: u8,
        level: u32,
    }

    /// Open, configure and start the unit; chunks arrive on the receiver.
    pub fn open() -> anyhow::Result<(AudioUnit, Receiver<voice::mic::Chunk>)> {
        let mut au =
            AudioUnit::new(IOType::VoiceProcessingIO).map_err(|e| anyhow::anyhow!("open: {e}"))?;
        // `new` initializes; formats and callbacks belong on an uninitialized unit.
        au.uninitialize()
            .map_err(|e| anyhow::anyhow!("uninitialize: {e}"))?;
        let fmt = StreamFormat {
            sample_rate: f64::from(RATE),
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: 1,
        };
        // Mic side: what bus 1 hands us. Playout side: what we hand bus 0 (silence).
        au.set_stream_format(fmt, Scope::Output, Element::Input)
            .map_err(|e| anyhow::anyhow!("input format: {e}"))?;
        au.set_stream_format(fmt, Scope::Input, Element::Output)
            .map_err(|e| anyhow::anyhow!("output format: {e}"))?;
        if let Err(e) = au.set_property(
            BYPASS_VOICE_PROCESSING,
            Scope::Global,
            Element::Output,
            Some(&0u32),
        ) {
            // The default is already "not bypassed".
            tracing::debug!("BypassVoiceProcessing=0: {e}");
        }
        // The default ducking makes the call itself hard to hear (the user's
        // report on the first real run): least ducking, no voice-activity ducking.
        if let Err(e) = au.set_property(
            OTHER_AUDIO_DUCKING,
            Scope::Global,
            Element::Output,
            Some(&Ducking {
                enable_advanced: 0,
                level: DUCKING_MIN,
            }),
        ) {
            tracing::warn!("OtherAudioDuckingConfiguration=min: {e}");
        }
        au.set_render_callback(|mut args: render_callback::Args<data::Interleaved<f32>>| {
            args.data.buffer.fill(0.0);
            args.flags
                .insert(render_callback::ActionFlags::OUTPUT_IS_SILENCE);
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("render callback: {e}"))?;
        let (tx, rx) = mpsc::channel();
        au.set_input_callback(move |args: render_callback::Args<data::Interleaved<f32>>| {
            // A gone receiver means the recording ended; nothing to report.
            let _ = tx.send(Ok(args.data.buffer.to_vec()));
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("input callback: {e}"))?;
        au.initialize()
            .map_err(|e| anyhow::anyhow!("initialize: {e}"))?;
        au.start().map_err(|e| anyhow::anyhow!("start: {e}"))?;
        Ok((au, rx))
    }
}
