//! The meeting app's audio (01 §4.1): a Core Audio process tap through
//! `flexaudio` (`SourceKind::ProcessLoopback` + `target_pid`) on its own
//! thread — the tap's ObjC objects are `!Send` — and the process list from
//! `kAudioHardwarePropertyProcessObjectList` (flexaudio has no enumeration).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use super::Feed;
use crate::ProcessInfo;

/// Start the tap on `pid`; returns once the stream runs (the TCC prompt fires
/// here on first use) or with the open error. The thread pumps chunks into
/// the rings until stopped. If the process goes away the thread logs and
/// exits; the encoder keeps writing silence for the track (01 §4.1).
#[cfg(target_os = "macos")]
pub fn spawn(pid: u32, mut feed: Feed, stop: Arc<AtomicBool>) -> anyhow::Result<JoinHandle<()>> {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::Duration;

    use flexaudio::{Event, OutputFormat, SourceKind, StreamConfig};

    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let thread = std::thread::Builder::new()
        .name("meeting-system".into())
        .spawn(move || {
            let cfg = StreamConfig {
                kind: SourceKind::ProcessLoopback,
                target_pid: Some(pid),
                output: OutputFormat {
                    sample_rate: super::RATE,
                    channels: 1,
                },
                ..Default::default()
            };
            let mut stream = match flexaudio::open(cfg).and_then(|mut s| s.start().map(|()| s)) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            while !stop.load(Ordering::SeqCst) {
                while let Some(ev) = stream.poll_event() {
                    match ev {
                        Event::PermissionDenied | Event::DeviceLost | Event::Error(_) => {
                            tracing::warn!(pid, "process tap stopped: {ev:?}");
                            return;
                        }
                        _ => {}
                    }
                }
                let Some(chunk) = stream.poll_chunk() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // Mono was asked for; downmix defensively if a frame is wider.
                let ch = (chunk.data.len() / chunk.frames.max(1)).max(1);
                let pcm = if ch == 1 {
                    chunk.data
                } else {
                    chunk
                        .data
                        .chunks_exact(ch)
                        .map(|f| f.iter().sum::<f32>() / ch as f32)
                        .collect()
                };
                feed.push(pcm);
            }
            // `stream` drops here: flexaudio joins its threads and closes the tap.
        })?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(e)) => {
            let _ = thread.join();
            anyhow::bail!("process tap on pid {pid}: {e}")
        }
        Err(_) => {
            let _ = thread.join();
            anyhow::bail!("process tap on pid {pid}: capture thread exited before starting")
        }
    }
}

/// Not macOS: no taps.
#[cfg(not(target_os = "macos"))]
pub fn spawn(pid: u32, _feed: Feed, _stop: Arc<AtomicBool>) -> anyhow::Result<JoinHandle<()>> {
    anyhow::bail!("process tap on pid {pid}: meetings need macOS 15")
}

/// Processes Core Audio knows about, active (playing) first, then by name;
/// our own pid skipped.
#[cfg(target_os = "macos")]
pub fn processes() -> anyhow::Result<Vec<ProcessInfo>> {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
        AudioObjectPropertyAddress, kAudioHardwarePropertyProcessObjectList,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
        kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningOutput,
        kAudioProcessPropertyPID,
    };
    use objc2_core_foundation::{CFRetained, CFString};

    fn addr(selector: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        }
    }

    /// Read a fixed-size property into `T`; `None` on any error.
    fn get<T: Copy>(object: AudioObjectID, selector: u32) -> Option<T> {
        let mut addr = addr(selector);
        let mut out = std::mem::MaybeUninit::<T>::uninit();
        let mut size = std::mem::size_of::<T>() as u32;
        // SAFETY: `addr`/`size` are valid locals; `out` has room for one `T`.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&mut addr),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                NonNull::new_unchecked(out.as_mut_ptr().cast::<c_void>()),
            )
        };
        if status != 0 || size as usize != std::mem::size_of::<T>() {
            return None;
        }
        // SAFETY: the HAL wrote `size == size_of::<T>()` bytes.
        Some(unsafe { out.assume_init() })
    }

    /// A `CFString` property (+1 retained by the HAL) as a `String`.
    fn get_string(object: AudioObjectID, selector: u32) -> Option<String> {
        let cf: *const CFString = get(object, selector)?;
        let cf = unsafe { CFRetained::from_raw(NonNull::new(cf as *mut CFString)?) };
        let s = cf.to_string();
        (!s.is_empty()).then_some(s)
    }

    let system = kAudioObjectSystemObject as AudioObjectID;
    let mut list_addr = addr(kAudioHardwarePropertyProcessObjectList);
    let mut size = 0u32;
    // SAFETY: valid locals, no qualifier.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            system,
            NonNull::from(&mut list_addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    anyhow::ensure!(status == 0, "process list size: OSStatus {status}");
    let mut objects =
        vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    if !objects.is_empty() {
        // SAFETY: `objects` has `size` bytes of room.
        let status = unsafe {
            AudioObjectGetPropertyData(
                system,
                NonNull::from(&mut list_addr),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                NonNull::new_unchecked(objects.as_mut_ptr().cast::<c_void>()),
            )
        };
        anyhow::ensure!(status == 0, "process list: OSStatus {status}");
        objects.truncate(size as usize / std::mem::size_of::<AudioObjectID>());
    }
    let me = std::process::id();
    let mut out: Vec<ProcessInfo> = objects
        .into_iter()
        .filter_map(|obj| {
            let pid: i32 = get(obj, kAudioProcessPropertyPID)?;
            let pid = u32::try_from(pid).ok().filter(|&p| p != me)?;
            let bundle_id = get_string(obj, kAudioProcessPropertyBundleID);
            let active = get::<u32>(obj, kAudioProcessPropertyIsRunningOutput).unwrap_or(0) != 0;
            let name = proc_name(pid)
                .or_else(|| bundle_id.as_deref()?.rsplit('.').next().map(str::to_owned))
                .unwrap_or_else(|| pid.to_string());
            Some(ProcessInfo {
                pid,
                name,
                bundle_id,
                active,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Not macOS: nothing to tap.
#[cfg(not(target_os = "macos"))]
pub fn processes() -> anyhow::Result<Vec<ProcessInfo>> {
    Ok(Vec::new())
}

/// `libc::proc_name`.
#[cfg(target_os = "macos")]
fn proc_name(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a valid writable buffer of the stated size.
    let n = unsafe { libc::proc_name(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..n as usize])
        .trim_end_matches('\0')
        .to_owned();
    (!s.is_empty()).then_some(s)
}
