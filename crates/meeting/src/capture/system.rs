//! The meeting app's audio (01 §4.1) through `flexaudio` on its own thread —
//! `SourceKind::ProcessLoopback` + `target_pid`, or `SystemLoopback` for
//! [`crate::SYSTEM_AUDIO`]: a Core Audio process tap on macOS (its ObjC
//! objects are `!Send`), WASAPI (process) loopback on Windows, PipeWire on
//! Linux (58 §6.1). flexaudio has no enumeration, so the process list is per
//! OS here: `kAudioHardwarePropertyProcessObjectList`, the default render
//! endpoint's audio sessions, the PipeWire registry.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use super::Feed;
use crate::ProcessInfo;

/// Start the tap on `pid` — or, for [`crate::SYSTEM_AUDIO`] (pid 0), a global
/// tap on the default output device minus our own process; returns once the
/// stream runs (the TCC prompt fires here on first use) or with the open
/// error. The thread pumps chunks into the rings until stopped. If the
/// process goes away the thread logs and exits; the encoder keeps writing
/// silence for the track (01 §4.1).
// ponytail: the global tap follows the *default* output device; an app routed to a non-default
// headset is silent to it. Per-app pick remains the escape hatch; `device_id` would be the upgrade.
pub fn spawn(pid: u32, mut feed: Feed, stop: Arc<AtomicBool>) -> anyhow::Result<JoinHandle<()>> {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::Duration;

    use flexaudio::{Event, OutputFormat, SourceKind, StreamConfig};

    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let thread = std::thread::Builder::new()
        .name("meeting-system".into())
        .spawn(move || {
            let cfg = if pid == crate::SYSTEM_AUDIO {
                StreamConfig {
                    kind: SourceKind::SystemLoopback,
                    exclude_self: true,
                    output: OutputFormat {
                        sample_rate: super::RATE,
                        channels: 1,
                    },
                    ..Default::default()
                }
            } else {
                StreamConfig {
                    kind: SourceKind::ProcessLoopback,
                    target_pid: Some(pid),
                    output: OutputFormat {
                        sample_rate: super::RATE,
                        channels: 1,
                    },
                    ..Default::default()
                }
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

/// 58 §3.3. macOS: `Full`. Windows/Linux: `Partial` (no echo cancellation) when
/// the loopback backend answers ([`processes`] reaches it), else `None(why)`.
pub fn supported() -> crate::Support {
    if cfg!(target_os = "macos") {
        return crate::Support::Full;
    }
    match processes() {
        Ok(_) => crate::Support::Partial("no echo cancellation; headphones recommended".into()),
        Err(e) => crate::Support::None(format!("{e:#}")),
    }
}

/// Active (playing) first, then by name; our own pid skipped.
fn sort(out: &mut [ProcessInfo]) {
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
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
    sort(&mut out);
    Ok(out)
}

/// Processes with an audio session on the default render endpoint
/// (`IAudioSessionManager2`), de-duplicated by pid; `Err` when there is no
/// render device. `bundle_id` is `None` on Windows.
#[cfg(target_os = "windows")]
pub fn processes() -> anyhow::Result<Vec<ProcessInfo>> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    // SAFETY: COM init/uninit are balanced on this thread; `RPC_E_CHANGED_MODE`
    // (already initialized as STA) still leaves COM usable, so only our own
    // successful init is undone.
    unsafe {
        let ours = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let out = sessions();
        if ours {
            CoUninitialize();
        }
        out
    }
}

#[cfg(target_os = "windows")]
unsafe fn sessions() -> anyhow::Result<Vec<ProcessInfo>> {
    use anyhow::Context as _;
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::core::Interface as _;

    // SAFETY: every interface pointer comes from a successful COM call on this thread.
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .context("MMDeviceEnumerator")?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .context("no default render device")?;
        let manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .context("IAudioSessionManager2")?;
        let sessions = manager
            .GetSessionEnumerator()
            .context("session enumerator")?;
        let me = std::process::id();
        let mut out: Vec<ProcessInfo> = Vec::new();
        for i in 0..sessions.GetCount().unwrap_or(0) {
            let Ok(control) = sessions.GetSession(i) else {
                continue;
            };
            let Ok(pid) = control
                .cast::<IAudioSessionControl2>()
                .and_then(|c| c.GetProcessId())
            else {
                continue;
            };
            // Pid 0 is the system-sounds session.
            if pid == 0 || pid == me || out.iter().any(|p| p.pid == pid) {
                continue;
            }
            out.push(ProcessInfo {
                pid,
                name: image_name(pid).unwrap_or_else(|| pid.to_string()),
                bundle_id: None,
                active: control.GetState().is_ok_and(|s| s == AudioSessionStateActive),
            });
        }
        sort(&mut out);
        Ok(out)
    }
}

/// The executable's file stem (`QueryFullProcessImageNameW`).
#[cfg(target_os = "windows")]
fn image_name(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    let mut buf = [0u16; MAX_PATH as usize];
    let mut len = buf.len() as u32;
    // SAFETY: `buf`/`len` are valid locals; the handle is closed after one query.
    let path = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len)
            .is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))?
    };
    std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

/// PipeWire nodes with `media.class = Stream/Output/Audio` (an app playing or
/// ready to play), one entry per `application.process.id`; `Err` when no
/// PipeWire daemon answers. `bundle_id` carries `application.name`.
#[cfg(target_os = "linux")]
pub fn processes() -> anyhow::Result<Vec<ProcessInfo>> {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use anyhow::Context as _;
    use pipewire as pw;

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("pipewire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("pipewire context")?;
    let core = context
        .connect_rc(None)
        .context("no PipeWire daemon (is pipewire running for this user?)")?;
    let registry = core.get_registry().context("pipewire registry")?;
    let me = std::process::id();
    let found: Rc<RefCell<Vec<ProcessInfo>>> = Rc::default();
    let sink = found.clone();
    let _reg = registry
        .add_listener_local()
        .global(move |global| {
            let Some(props) = global.props else {
                return;
            };
            if global.type_ != pw::types::ObjectType::Node
                || props.get(*pw::keys::MEDIA_CLASS) != Some("Stream/Output/Audio")
            {
                return;
            }
            let Some(pid) = props
                .get(*pw::keys::APP_PROCESS_ID)
                .and_then(|p| p.parse::<u32>().ok())
                .filter(|&p| p != me)
            else {
                return;
            };
            let mut sink = sink.borrow_mut();
            if sink.iter().any(|p| p.pid == pid) {
                return;
            }
            let app = props.get(*pw::keys::APP_NAME).map(str::to_owned);
            let name = app
                .clone()
                .or_else(|| props.get(*pw::keys::NODE_NAME).map(str::to_owned))
                .unwrap_or_else(|| pid.to_string());
            sink.push(ProcessInfo {
                pid,
                name,
                bundle_id: app,
                // Every node here is a live output stream; PipeWire has no
                // cheap "currently playing" bit at the registry level.
                active: true,
            });
        })
        .register();
    // The daemon replays the registry, then answers the sync: that `done` is
    // the end of the list. The timer is the guard against a wedged daemon.
    let pending = core.sync(0).context("pipewire sync")?;
    let quit = mainloop.clone();
    let _core = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                quit.quit();
            }
        })
        .register();
    let quit = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| quit.quit());
    timer
        .update_timer(Some(Duration::from_millis(500)), None)
        .into_result()
        .map_err(|e| anyhow::anyhow!("pipewire timer: {e:?}"))?;
    mainloop.run();
    let mut out = std::mem::take(&mut *found.borrow_mut());
    sort(&mut out);
    Ok(out)
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
