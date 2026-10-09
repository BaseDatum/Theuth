//! The daemon's microphone (54 §0.2): the default input through `flexaudio`,
//! 48 kHz mono `f32`, on its own `std::thread`, chunks over an mpsc. Opened per
//! dictation and dropped with it, so the OS mic indicator is lit only while
//! dictating. No `flexaudio` type leaves here.
//!
//! Echo cancellation (76 §0.5): [`Capture::open_with`]`(Aec::Vpio)` wraps the
//! mic in Apple's VoiceProcessingIO unit (AEC + AGC + NS against whatever the
//! default output plays, whichever process plays it) and falls back to the
//! plain mic where the unit is unavailable, reporting which one opened. Same
//! chunk shape out either way.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use flexaudio::{Event, OutputFormat, SourceKind, StreamConfig};
use serde::{Deserialize, Serialize};

/// Capture rate. The dictation task resamples to the models' 16 kHz.
pub const RATE: u32 = 48_000;
/// Samples per chunk: 20 ms at [`RATE`].
const CHUNK: usize = RATE as usize / 50;
/// Poll interval when the device ring is empty (a chunk lands every 20 ms).
const IDLE: Duration = Duration::from_millis(5);

/// One mic delivery: 960 mono samples, or the device's error (sent once, then the channel closes).
pub type Chunk = Result<Vec<f32>, String>;

/// Echo cancellation on the mic: what was asked for, and what was opened.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Aec {
    /// Apple's VoiceProcessingIO unit (macOS). As a request: preferred, plain mic if it fails.
    Vpio,
    /// The plain default input; the caller lives with the echo (`Degradation::MicNoAec`).
    #[default]
    None,
}

/// An open microphone. Dropping it stops the thread and closes the device.
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The VPIO unit, when that is what opened; dropped first so its sender closes.
    #[cfg(target_os = "macos")]
    unit: Option<coreaudio::audio_unit::AudioUnit>,
}

impl Capture {
    /// Open the default input and start capturing. Returns once the device is
    /// running (permission prompt included) or fails with the device's error.
    /// The receiver yields mono `f32` chunks of 960 samples; on a device
    /// failure it yields `Err(message)` once and then closes.
    pub fn open() -> anyhow::Result<(Capture, Receiver<Chunk>)> {
        Self::open_with(Aec::None).map(|(cap, rx, _)| (cap, rx))
    }

    /// 153 §8.1: walk `prefs` in order (names the host does not enumerate are
    /// skipped), then the system default, then the first input; the third
    /// value is the name that opened (`"default"` when the host names none).
    pub fn open_on(prefs: &[String]) -> anyhow::Result<(Capture, Receiver<Chunk>, String)> {
        let devices = list_input_devices();
        for name in prefs {
            if !devices.iter().any(|d| &d.name == name) {
                continue;
            }
            match Self::open_named(Some(name.clone())) {
                Ok((cap, rx)) => return Ok((cap, rx, name.clone())),
                Err(e) => tracing::info!("voice: microphone {name:?}: {e:#}"),
            }
        }
        let default = devices
            .iter()
            .find(|d| d.default)
            .or(devices.first())
            .map(|d| d.name.clone());
        let err = match Self::open_named(None) {
            Ok((cap, rx)) => return Ok((cap, rx, default.unwrap_or_else(|| "default".into()))),
            Err(e) => e,
        };
        match devices.first().filter(|d| !d.default) {
            Some(first) => Self::open_named(Some(first.name.clone()))
                .map(|(cap, rx)| (cap, rx, first.name.clone())),
            None => Err(err),
        }
    }

    /// A capture that owns no thread (tests feed the receiver themselves).
    #[cfg(test)]
    pub(crate) fn fake() -> Capture {
        Capture {
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            #[cfg(target_os = "macos")]
            unit: None,
        }
    }

    /// The plain (no AEC) open of one named input, or the default when `None`.
    fn open_named(device: Option<String>) -> anyhow::Result<(Capture, Receiver<Chunk>)> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("voice-mic".into())
            .spawn({
                let stop = stop.clone();
                move || run(&stop, &tx, &ready_tx, device)
            })?;
        let mut cap = Capture {
            stop,
            thread: Some(thread),
            #[cfg(target_os = "macos")]
            unit: None,
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok((cap, rx)),
            Ok(Err(e)) => {
                cap.join();
                anyhow::bail!("microphone: {e}")
            }
            Err(_) => {
                cap.join();
                anyhow::bail!("microphone: capture thread exited before starting")
            }
        }
    }

    /// [`Capture::open`] with echo cancellation when `want` asks for it and
    /// the host has it; the third value is what actually opened.
    pub fn open_with(want: Aec) -> anyhow::Result<(Capture, Receiver<Chunk>, Aec)> {
        #[cfg(target_os = "macos")]
        if want == Aec::Vpio {
            match vpio::open() {
                Ok((unit, raw)) => {
                    let (tx, rx) = mpsc::channel();
                    let stop = Arc::new(AtomicBool::new(false));
                    let thread = std::thread::Builder::new()
                        .name("voice-mic".into())
                        .spawn({
                            let stop = stop.clone();
                            move || rechunk(&stop, &raw, &tx)
                        })?;
                    return Ok((
                        Capture {
                            stop,
                            thread: Some(thread),
                            unit: Some(unit),
                        },
                        rx,
                        Aec::Vpio,
                    ));
                }
                Err(e) => {
                    tracing::warn!("VoiceProcessingIO unavailable, plain mic without AEC: {e:#}")
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = want;
        Self::open_named(None).map(|(cap, rx)| (cap, rx, Aec::None))
    }

    fn join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(target_os = "macos")]
        drop(self.unit.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.join();
    }
}

/// 153 §8.1: the host's input devices, the system default flagged. Empty when
/// the host enumerates nothing (no audio backend); never an error.
pub fn list_input_devices() -> Vec<crate::InputDevice> {
    flexaudio::devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.source_kind == SourceKind::Mic)
        .map(|d| crate::InputDevice {
            name: d.name,
            default: d.is_default,
        })
        .collect()
}

/// The capture thread: open + start the device, then pump chunks until stopped,
/// the receiver is gone, or the device fails.
fn run(
    stop: &AtomicBool,
    tx: &Sender<Chunk>,
    ready: &Sender<Result<(), String>>,
    device_id: Option<String>,
) {
    let cfg = StreamConfig {
        kind: SourceKind::Mic,
        device_id,
        output: OutputFormat {
            sample_rate: RATE,
            channels: 1,
        },
        ..Default::default()
    };
    let mut stream = match flexaudio::open(cfg).and_then(|mut s| s.start().map(|()| s)) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut buf: Vec<f32> = Vec::with_capacity(CHUNK);
    while !stop.load(Ordering::SeqCst) {
        while let Some(ev) = stream.poll_event() {
            let msg = match ev {
                Event::PermissionDenied => "permission denied".to_owned(),
                Event::DeviceLost => "device lost".to_owned(),
                Event::Error(m) => m,
                // Stalls are retried by flexaudio's watchdog; drops are audible, not fatal.
                _ => continue,
            };
            let _ = tx.send(Err(msg));
            return;
        }
        let Some(chunk) = stream.poll_chunk() else {
            std::thread::sleep(IDLE);
            continue;
        };
        // The output format asks for mono; downmix defensively if a frame is wider.
        let ch = (chunk.data.len() / chunk.frames.max(1)).max(1);
        for frame in chunk.data.chunks_exact(ch) {
            buf.push(frame.iter().sum::<f32>() / ch as f32);
            if buf.len() == CHUNK {
                let full = std::mem::replace(&mut buf, Vec::with_capacity(CHUNK));
                if tx.send(Ok(full)).is_err() {
                    return;
                }
            }
        }
    }
    // `stream` drops here: flexaudio joins its threads and closes the device.
}

/// The VPIO thread: the unit's callback-sized buffers → 960-sample chunks,
/// until stopped, the unit is gone (its sender closed) or the receiver is.
#[cfg(target_os = "macos")]
fn rechunk(stop: &AtomicBool, raw: &Receiver<Chunk>, tx: &Sender<Chunk>) {
    let mut buf: Vec<f32> = Vec::with_capacity(CHUNK);
    while !stop.load(Ordering::SeqCst) {
        let pcm = match raw.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(pcm)) => pcm,
            Ok(Err(e)) => {
                let _ = tx.send(Err(e));
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        for s in pcm {
            buf.push(s);
            if buf.len() == CHUNK {
                let full = std::mem::replace(&mut buf, Vec::with_capacity(CHUNK));
                if tx.send(Ok(full)).is_err() {
                    return;
                }
            }
        }
    }
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

    use super::{Chunk, RATE};

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

    /// Open, configure and start the unit; callback-sized buffers arrive on the receiver.
    pub fn open() -> anyhow::Result<(AudioUnit, Receiver<Chunk>)> {
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
