//! The mic sensor (158 §4.1): which processes hold an input stream right now. Per-OS code
//! beside `capture::system::processes()` — macOS reads `kAudioProcessPropertyIsRunningInput`
//! on the same process objects, Windows walks the default capture endpoints' sessions, Linux
//! lists PipeWire `Stream/Input/Audio` nodes. The sensor never opens a device (§4.6).

use crate::ProcessInfo;

/// Siri's helper holds the mic on every "Hey Siri"-enabled Mac; never a meeting.
#[cfg(target_os = "macos")]
const CORE_SPEECH: &str = "com.apple.CoreSpeech";

/// Processes holding an input stream, by name; `active` is always `true` here ("input
/// running"). Skips our own pid and `com.apple.CoreSpeech` like `processes()` does.
#[cfg(target_os = "macos")]
pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>> {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
        AudioObjectPropertyAddress, kAudioHardwarePropertyProcessObjectList,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
        kAudioProcessPropertyIsRunningInput, kAudioProcessPropertyPID,
    };

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
    let holding: Vec<u32> = objects
        .into_iter()
        .filter(|&obj| get::<u32>(obj, kAudioProcessPropertyIsRunningInput).unwrap_or(0) != 0)
        .filter_map(|obj| u32::try_from(get::<i32>(obj, kAudioProcessPropertyPID)?).ok())
        .filter(|&pid| pid != me)
        .collect();
    if holding.is_empty() {
        return Ok(Vec::new());
    }
    // Names and bundle ids come from the same object list, read once over there.
    let mut out: Vec<ProcessInfo> = crate::capture::system::processes()?
        .into_iter()
        .filter(|p| holding.contains(&p.pid) && p.bundle_id.as_deref() != Some(CORE_SPEECH))
        .map(|p| ProcessInfo { active: true, ..p })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

/// Processes with an active session on the default capture endpoint — `eConsole` and
/// `eCommunications`, which can differ — de-duplicated by pid; own pid skipped.
#[cfg(target_os = "windows")]
pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    // SAFETY: COM init/uninit are balanced on this thread (see `capture::system::processes`).
    unsafe {
        let ours = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let out = capture_sessions();
        if ours {
            CoUninitialize();
        }
        out
    }
}

#[cfg(target_os = "windows")]
unsafe fn capture_sessions() -> anyhow::Result<Vec<ProcessInfo>> {
    use anyhow::Context as _;
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        MMDeviceEnumerator, eCapture, eCommunications, eConsole,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::core::Interface as _;

    // SAFETY: every interface pointer comes from a successful COM call on this thread.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("MMDeviceEnumerator")?;
        let me = std::process::id();
        let mut out: Vec<ProcessInfo> = Vec::new();
        for role in [eConsole, eCommunications] {
            let Ok(device) = enumerator.GetDefaultAudioEndpoint(eCapture, role) else {
                continue; // no capture device for this role
            };
            let manager: IAudioSessionManager2 = device
                .Activate(CLSCTX_ALL, None)
                .context("IAudioSessionManager2")?;
            let sessions = manager
                .GetSessionEnumerator()
                .context("session enumerator")?;
            for i in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(control) = sessions.GetSession(i) else {
                    continue;
                };
                if !control
                    .GetState()
                    .is_ok_and(|s| s == AudioSessionStateActive)
                {
                    continue;
                }
                let Ok(pid) = control
                    .cast::<IAudioSessionControl2>()
                    .and_then(|c| c.GetProcessId())
                else {
                    continue;
                };
                if pid == 0 || pid == me || out.iter().any(|p| p.pid == pid) {
                    continue;
                }
                out.push(ProcessInfo {
                    pid,
                    name: image_name(pid).unwrap_or_else(|| pid.to_string()),
                    bundle_id: None,
                    active: true,
                });
            }
        }
        out.sort_by_key(|p| p.name.to_lowercase());
        Ok(out)
    }
}

/// The executable's file stem (`QueryFullProcessImageNameW`); same as `capture::system`'s.
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
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))?
    };
    std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

/// PipeWire nodes with `media.class = Stream/Input/Audio`, one per `application.process.id`;
/// a node's existence is "holding the mic" (no cheap running bit). `bundle_id` carries
/// `application.name`. Same 500 ms sync guard as `processes()`.
#[cfg(target_os = "linux")]
pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>> {
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
                || props.get(*pw::keys::MEDIA_CLASS) != Some("Stream/Input/Audio")
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
                active: true,
            });
        })
        .register();
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
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>> {
    anyhow::bail!("no mic sensor on this platform")
}

#[cfg(test)]
mod tests {
    /// §4.7: the probe returns on this host (empty is fine); its failure mode is `Err`, not a
    /// panic or a hang.
    #[test]
    fn mic_holders_returns() {
        match super::mic_holders() {
            Ok(list) => assert!(list.iter().all(|p| p.active && p.pid != std::process::id())),
            Err(e) => eprintln!("mic_holders: {e:#} (no audio HAL on this host)"),
        }
    }
}
