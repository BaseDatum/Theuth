//! The meeting app's audio (01 §4.1) through `flexaudio` on its own thread —
//! `SourceKind::ProcessLoopback` + `target_pid`, or `SystemLoopback` for
//! [`crate::SYSTEM_AUDIO`]: a Core Audio process tap on macOS (its ObjC
//! objects are `!Send`), WASAPI (process) loopback on Windows, PipeWire on
//! Linux (58 §6.1). flexaudio has no enumeration, so the process list is per
//! OS here: `kAudioHardwarePropertyProcessObjectList`, the default render
//! endpoint's audio sessions, the PipeWire registry.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Feed, Report, Reports, watchdog};
use crate::{CaptureFault, Degradation, Permission, ProcessInfo, Track};

/// What the system thread taps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// `ProcessLoopback` on this pid.
    Process(u32),
    /// `SystemLoopback` minus our own process ([`crate::SYSTEM_AUDIO`], or the fallback).
    System,
}

impl Mode {
    fn pid(self) -> u32 {
        match self {
            Mode::Process(pid) => pid,
            Mode::System => crate::SYSTEM_AUDIO,
        }
    }

    /// The stream config: 48 kHz mono for every mode (the rings, probe and encoder assume it).
    fn config(self) -> flexaudio::StreamConfig {
        use flexaudio::{OutputFormat, SourceKind, StreamConfig};
        let output = OutputFormat {
            sample_rate: super::RATE,
            channels: 1,
        };
        match self {
            Mode::System => StreamConfig {
                kind: SourceKind::SystemLoopback,
                exclude_self: true,
                output,
                ..Default::default()
            },
            Mode::Process(pid) => StreamConfig {
                kind: SourceKind::ProcessLoopback,
                target_pid: Some(pid),
                output,
                ..Default::default()
            },
        }
    }

    fn open(self) -> flexaudio::Result<flexaudio::Stream> {
        flexaudio::open(self.config()).and_then(|mut s| s.start().map(|()| s))
    }
}

/// What a flexaudio event means to the watchdog (`None`: logged / counted only).
fn signal_of(ev: &flexaudio::Event) -> Option<watchdog::Signal> {
    use flexaudio::Event;
    use watchdog::Signal;
    match ev {
        Event::StreamStalled | Event::RecoverableError { .. } => Some(Signal::Stalled),
        Event::StreamRecovered => Some(Signal::Recovered),
        Event::SilenceWhileSourceActive { .. } => Some(Signal::SilentWhileActive),
        Event::DeviceLost | Event::Error(_) => Some(Signal::Lost),
        Event::PermissionDenied { .. } => Some(Signal::PermissionDenied),
        Event::TerminalError { .. } => Some(Signal::Terminal),
        _ => None,
    }
}

/// Start the tap on `pid` — or, for [`crate::SYSTEM_AUDIO`] (pid 0), a global
/// tap on the default output device minus our own process; returns once the
/// stream runs (the TCC prompt fires here on first use) or with the open
/// error. The thread pumps chunks into the rings until stopped, under the 173
/// watchdog ([`watchdog`]): every flexaudio event is logged; a stall, or exact
/// zeros while a concurrent global-tap probe hears sound, reopen the tap
/// (retargeted within the app's family, [`watchdog::in_family`]); three failed
/// reopens fall back to the global tap, and a return probe every minute goes
/// back to the app's tap once it is live. Every episode goes to `reports`. It
/// never exits before `stop`.
// ponytail: the global tap follows the *default* output device; an app routed to a non-default
// headset is silent to it. Per-app pick remains the escape hatch; `device_id` would be the upgrade.
pub fn spawn(
    pid: u32,
    bundle: Option<String>,
    mut feed: Feed,
    stop: Arc<AtomicBool>,
    reports: Reports,
    span: tracing::Span,
) -> anyhow::Result<JoinHandle<()>> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let thread = std::thread::Builder::new()
        .name("meeting-system".into())
        .spawn(move || {
            let _span = span.entered();
            let (mode, target) = if pid == crate::SYSTEM_AUDIO {
                (Mode::System, None)
            } else {
                (
                    Mode::Process(pid),
                    Some(watchdog::Target::new(pid, bundle, exe_path(pid))),
                )
            };
            let stream = match mode.open() {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            run(stream, mode, target, &mut feed, &stop, &reports);
            // `stream` drops in `run`: flexaudio joins its threads and closes the tap.
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

/// Mono samples of a chunk (mono was asked for; downmix defensively if a frame is wider).
fn mono(chunk: flexaudio::AudioChunk) -> Vec<f32> {
    let ch = (chunk.data.len() / chunk.frames.max(1)).max(1);
    if ch == 1 {
        chunk.data
    } else {
        chunk
            .data
            .chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect()
    }
}

/// The probe floor: [`watchdog::AUDIBLE_DBFS`], or `THEUTH_MEETING_PROBE_DBFS` — a calibration
/// knob (an app whose idle output sits above −70 dBFS; `examples/tap_repro`'s −80 dBFS tone),
/// read once per meeting, clamped to −140…0; a non-finite or unparsable value is logged and ignored.
fn probe_floor() -> f32 {
    let raw = std::env::var("THEUTH_MEETING_PROBE_DBFS").ok();
    let (floor, complaint) = watchdog::probe_floor(raw.as_deref());
    if let Some(c) = complaint {
        tracing::warn!("meeting capture: {c}");
    }
    floor
}

/// Listen on `mode` for [`watchdog::PROBE_WINDOW`] on a thread of its own (its flexaudio
/// objects stay there); the answer arrives on the receiver (disconnected = the thread died).
fn probe(mode: Mode, floor: f32) -> mpsc::Receiver<Result<watchdog::ProbeMeter, String>> {
    let (tx, rx) = mpsc::channel();
    let span = tracing::Span::current();
    let _ = std::thread::Builder::new()
        .name("meeting-probe".into())
        .spawn(move || {
            let _span = span.entered();
            let r = mode.open().map_err(|e| e.to_string()).map(|mut s| {
                let mut m = watchdog::ProbeMeter::new(floor);
                let end = Instant::now() + watchdog::PROBE_WINDOW;
                while Instant::now() < end {
                    while let Some(ev) = s.poll_event() {
                        tracing::debug!(?mode, "meeting capture: probe event: {ev:?}");
                    }
                    match s.poll_chunk() {
                        Some(c) => m.add(&mono(c)),
                        None => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
                m
            });
            let _ = tx.send(r);
        });
    rx
}

/// The activity the watchdog's activity-gated rules see: the app's family on a per-app tap,
/// any process on the global tap (pid 0 and the fallback).
fn activity_of(mode: Mode, target: Option<&watchdog::Target>) -> watchdog::Activity {
    match processes() {
        Err(_) => watchdog::Activity::Unknown,
        Ok(ps) => watchdog::activity(
            target.filter(|_| mode != Mode::System),
            &ps,
            &exe_path,
            std::process::id(),
        ),
    }
}

/// The capture loop: events → log + watchdog, chunks → rings + watchdog, probe answers →
/// watchdog, a watchdog tick every 250 ms, the health line every minute.
fn run(
    stream: flexaudio::Stream,
    mut mode: Mode,
    target: Option<watchdog::Target>,
    feed: &mut Feed,
    stop: &AtomicBool,
    reports: &Reports,
) {
    use std::sync::atomic::Ordering;

    use flexaudio::Event;
    use watchdog::{Action, Activity, Signal};

    const TICK: Duration = Duration::from_millis(250);
    const LINE: Duration = Duration::from_secs(60);
    /// The process list is re-read at most this often…
    const ACTIVITY_TTL: Duration = Duration::from_secs(1);
    /// …or this often while nothing plays (R3 #20: a PipeWire registry walk on Linux).
    const IDLE_ACTIVITY_TTL: Duration = Duration::from_secs(5);
    /// A probe that has not answered by then failed (R3 #4: a hung open or drop).
    const PROBE_DEADLINE: Duration = Duration::from_secs(14);

    let me = std::process::id();
    let floor = probe_floor();
    if floor != watchdog::AUDIBLE_DBFS {
        tracing::warn!(
            floor,
            "meeting capture: probe floor overridden (THEUTH_MEETING_PROBE_DBFS)"
        );
    }
    let mut stream = Some(stream);
    let start = Instant::now();
    let per_app = target.is_some();
    let mut wd = watchdog::Watchdog::new(
        start,
        watchdog::Rules {
            zeros: true,
            fallback: per_app,
            probe: per_app,
        },
    );
    let mut meter = watchdog::Meter::default();
    let (mut last_tick, mut last_line) = (start, start);
    let mut cached: Option<(Instant, Activity)> = None;
    // A running probe: what for, the pid a return probe taps, when it started, its answer.
    type Answer = mpsc::Receiver<Result<watchdog::ProbeMeter, String>>;
    let mut probing: Option<(Action, u32, Instant, Answer)> = None;
    let mut probe_failure_logged = false;
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        let mut gone = false;
        while let Some(ev) = stream.as_mut().and_then(|s| s.poll_event()) {
            let pid = mode.pid();
            match &ev {
                Event::ChunkDropped { count } => meter.chunk_dropped(*count),
                Event::Clipped => meter.clipped += 1,
                _ => tracing::warn!(pid, ?mode, "meeting capture: system audio event: {ev:?}"),
            }
            if let Some(s) = signal_of(&ev) {
                gone |= matches!(
                    s,
                    Signal::Lost | Signal::PermissionDenied | Signal::Terminal
                );
                if s == Signal::PermissionDenied {
                    let _ = reports.send(Report::Degraded(Degradation::PermissionDenied(
                        Permission::AudioCapture,
                    )));
                }
                wd.signal(now, s);
            }
        }
        if gone {
            drop(stream.take());
            meter.new_stream();
        }
        match stream.as_mut().and_then(|s| s.poll_chunk()) {
            Some(chunk) => super::deliver(now, mono(chunk), &mut meter, &mut wd, feed),
            None => std::thread::sleep(Duration::from_millis(5)),
        }
        let answer = probing
            .as_ref()
            .and_then(|(_, _, asked, rx)| match rx.try_recv() {
                Ok(r) => Some(r),
                Err(mpsc::TryRecvError::Empty) if now.duration_since(*asked) < PROBE_DEADLINE => {
                    None
                }
                Err(mpsc::TryRecvError::Empty) => Some(Err("no answer within 14 s".into())),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("probe thread gone".into())),
            });
        if let Some(answer) = answer
            && let Some((kind, pid, _, _)) = probing.take()
        {
            let now = Instant::now();
            match (kind, answer) {
                (Action::Return, Ok(m)) if m.nonzero() >= watchdog::RETURN_FOR => {
                    match Mode::Process(pid).open() {
                        Ok(s) => {
                            tracing::warn!(
                                pid,
                                nonzero_ms = m.nonzero().as_millis() as u64,
                                "meeting capture: the app's tap is live again; back from the fallback"
                            );
                            // R3 #14: what the global tap already buffered is recorded, not dropped.
                            while let Some(c) = stream.as_mut().and_then(|s| s.poll_chunk()) {
                                super::deliver(now, mono(c), &mut meter, &mut wd, feed);
                            }
                            drop(stream.replace(s));
                            meter.new_stream();
                            mode = Mode::Process(pid);
                            wd.returned(now, true);
                            let _ = reports.send(Report::Returned { end: now, pid });
                        }
                        Err(e) => {
                            tracing::warn!(
                                pid,
                                "meeting capture: return to the app's tap failed: {e}"
                            );
                            wd.returned(now, false);
                        }
                    }
                }
                (Action::Return, r) => {
                    tracing::info!(
                        pid,
                        nonzero_ms = r.as_ref().map_or(0, |m| m.nonzero().as_millis() as u64),
                        error = r.as_ref().err().map(String::as_str),
                        "meeting capture: return probe: the app's tap is still quiet"
                    );
                    wd.returned(now, false);
                }
                (_, r) => {
                    let verdict = r.as_ref().map_or(watchdog::Probe::Failed, |m| m.verdict());
                    match &r {
                        Ok(m) => tracing::info!(
                            loud_ms = m.loud().as_millis() as u64,
                            nonzero_ms = m.nonzero().as_millis() as u64,
                            floor,
                            ?verdict,
                            "meeting capture: the app's tap is exact zeros; probe of all system audio"
                        ),
                        Err(e) if !probe_failure_logged => {
                            probe_failure_logged = true;
                            tracing::warn!(
                                "meeting capture: probe of all system audio failed ({e}); \
                                 zeros are judged by IsRunningOutput instead"
                            );
                        }
                        Err(_) => {}
                    }
                    wd.probed(now, verdict);
                }
            }
        }
        if now.duration_since(last_tick) < TICK {
            continue;
        }
        last_tick = now;
        let mut activity = || {
            if let Some((at, a)) = cached {
                let ttl = if a == Activity::Inactive {
                    IDLE_ACTIVITY_TTL
                } else {
                    ACTIVITY_TTL
                };
                if now.duration_since(at) < ttl {
                    return a;
                }
            }
            let a = activity_of(mode, target.as_ref());
            cached = Some((now, a));
            a
        };
        match wd.tick(now, &mut activity) {
            None => {}
            Some(Action::Probe) => {
                probing = Some((Action::Probe, 0, now, probe(Mode::System, floor)));
            }
            Some(Action::Return) => {
                // R3 #8: a family member running output beats a merely-alive original.
                let pid = target.as_ref().and_then(|t| {
                    let ps = processes().ok()?;
                    let pid = watchdog::retarget(t, t.pid, &ps, &exe_path, me);
                    ps.iter().any(|p| p.pid == pid).then_some(pid)
                });
                match pid {
                    Some(pid) => {
                        probing =
                            Some((Action::Return, pid, now, probe(Mode::Process(pid), floor)));
                    }
                    None => wd.returned(now, false),
                }
            }
            Some(action) => {
                drop(stream.take());
                meter.new_stream();
                let mut retarget = None;
                match (action, mode, target.as_ref()) {
                    (Action::Fallback, Mode::Process(pid), _) => {
                        let reason = wd.fell_back().unwrap_or(CaptureFault::Silent);
                        tracing::warn!(
                            pid,
                            ?reason,
                            "meeting capture: the app tap stayed quiet through {} reopens while sound \
                             played; falling back to all system audio",
                            watchdog::FALLBACK_AFTER
                        );
                        mode = Mode::System;
                        let _ = reports.send(Report::Fallback { start: now, reason });
                    }
                    (_, Mode::Process(pid), Some(t)) => {
                        let to = processes()
                            .map(|ps| watchdog::retarget(t, pid, &ps, &exe_path, me))
                            .unwrap_or(pid);
                        if to != pid {
                            tracing::warn!(
                                pid,
                                to,
                                bundle = ?t.bundle,
                                app = ?t.app,
                                "meeting capture: the app plays from another process; retargeting the tap"
                            );
                            mode = Mode::Process(to);
                            retarget = Some(to);
                        }
                    }
                    _ => {}
                }
                cached = None;
                match mode.open() {
                    Ok(s) => {
                        tracing::warn!(
                            pid = mode.pid(),
                            ?mode,
                            reopens = wd.reopens,
                            "meeting capture: system audio reopened"
                        );
                        stream = Some(s);
                        wd.reopened(Instant::now(), true, retarget);
                    }
                    Err(e) => {
                        tracing::warn!(
                            pid = mode.pid(),
                            ?mode,
                            "meeting capture: system audio reopen failed: {e}"
                        );
                        wd.reopened(Instant::now(), false, retarget);
                    }
                }
            }
        }
        super::forward(Track::System, &mut wd, reports);
        if now.duration_since(last_line) >= LINE {
            last_line = now;
            let active = format!("{:?}", activity_of(mode, target.as_ref()));
            health_line(
                "system",
                &mode_label(mode),
                &active,
                &meter,
                wd.reopens,
                wd.troubled(),
            );
            meter.next_minute();
        }
    }
    drop(stream);
    wd.finish(Instant::now());
    super::forward(Track::System, &mut wd, reports);
}

fn mode_label(mode: Mode) -> String {
    match mode {
        Mode::Process(pid) => format!("process {pid}"),
        Mode::System => "system".into(),
    }
}

/// 173 §4: one line per capture track per minute, for forensics (`active`: the target family's
/// `IsRunningOutput`); a warn line too when flexaudio dropped or clipped chunks in that minute
/// (its per-event noise aggregated).
pub(super) fn health_line(
    track: &str,
    target: &str,
    active: &str,
    m: &watchdog::Meter,
    reopens: u32,
    troubled: bool,
) {
    tracing::info!(
        "meeting capture health: {track} chunks={} frames={} zero={:.1}% peak={:.1}dBFS target={target} active={active} reopens={reopens} episode={troubled}",
        m.chunks,
        m.frames,
        m.zero_pct(),
        m.peak_dbfs(),
    );
    if m.dropped > 0 || m.clipped > 0 {
        tracing::warn!(
            "meeting capture: {track} lost {} chunks (ChunkDropped) and clipped {} in the last minute",
            m.dropped,
            m.clipped,
        );
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
    process_objects(objc2_core_audio::kAudioProcessPropertyIsRunningOutput)
}

/// The Core Audio process objects with `active` read from `running`
/// (`kAudioProcessPropertyIsRunningOutput` for the picker, `…IsRunningInput` for the
/// 158 §4.1 mic sensor); own pid skipped, active first, then by name.
#[cfg(target_os = "macos")]
pub(crate) fn process_objects(running: u32) -> anyhow::Result<Vec<ProcessInfo>> {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
        AudioObjectPropertyAddress, kAudioHardwarePropertyProcessObjectList,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
        kAudioProcessPropertyBundleID, kAudioProcessPropertyPID,
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
            let active = get::<u32>(obj, running).unwrap_or(0) != 0;
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
    use windows::Win32::Media::Audio::{eMultimedia, eRender};
    sessions_on(eRender, &[eMultimedia])
}

/// Processes with an audio session on the default endpoint of `flow` for each of `roles`
/// (158 §4.1: the capture side asks `eConsole` and `eCommunications`, which can differ),
/// de-duplicated by pid (`active` if any role's session is); `Err` only when no role has a
/// default device. Own pid and the system-sounds session (pid 0) skipped.
#[cfg(target_os = "windows")]
pub(crate) fn sessions_on(
    flow: windows::Win32::Media::Audio::EDataFlow,
    roles: &[windows::Win32::Media::Audio::ERole],
) -> anyhow::Result<Vec<ProcessInfo>> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    // SAFETY: COM init/uninit are balanced on this thread; `RPC_E_CHANGED_MODE`
    // (already initialized as STA) still leaves COM usable, so only our own
    // successful init is undone.
    unsafe {
        let ours = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let out = sessions(flow, roles);
        if ours {
            CoUninitialize();
        }
        out
    }
}

#[cfg(target_os = "windows")]
unsafe fn sessions(
    flow: windows::Win32::Media::Audio::EDataFlow,
    roles: &[windows::Win32::Media::Audio::ERole],
) -> anyhow::Result<Vec<ProcessInfo>> {
    use anyhow::Context as _;
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        MMDeviceEnumerator,
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
        let mut no_device = None;
        for role in roles {
            let device = match enumerator.GetDefaultAudioEndpoint(flow, *role) {
                Ok(d) => d,
                Err(e) => {
                    no_device.get_or_insert(e);
                    continue;
                }
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
                let Ok(pid) = control
                    .cast::<IAudioSessionControl2>()
                    .and_then(|c| c.GetProcessId())
                else {
                    continue;
                };
                // Pid 0 is the system-sounds session.
                if pid == 0 || pid == me {
                    continue;
                }
                let active = control
                    .GetState()
                    .is_ok_and(|s| s == AudioSessionStateActive);
                if let Some(p) = out.iter_mut().find(|p| p.pid == pid) {
                    p.active |= active;
                    continue;
                }
                out.push(ProcessInfo {
                    pid,
                    name: image_name(pid).unwrap_or_else(|| pid.to_string()),
                    bundle_id: None,
                    active,
                });
            }
        }
        if let Some(e) = no_device.filter(|_| out.is_empty() && roles.len() == 1) {
            return Err(anyhow::Error::from(e).context("no default audio device"));
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

/// PipeWire nodes with `media.class = Stream/Output/Audio` (an app playing or
/// ready to play), one entry per `application.process.id`; `Err` when no
/// PipeWire daemon answers. `bundle_id` carries `application.name`.
#[cfg(target_os = "linux")]
pub fn processes() -> anyhow::Result<Vec<ProcessInfo>> {
    nodes("Stream/Output/Audio")
}

/// PipeWire registry nodes of `media.class`, one per `application.process.id`, own pid
/// skipped; every node counts as `active` (158 §4.1: no cheap running bit). 500 ms guard.
#[cfg(target_os = "linux")]
pub(crate) fn nodes(class: &'static str) -> anyhow::Result<Vec<ProcessInfo>> {
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
                || props.get(*pw::keys::MEDIA_CLASS) != Some(class)
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
                // Every node here is a live stream of `class`; PipeWire has no
                // cheap "currently running" bit at the registry level.
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

/// 173 R2-2: the executable's path (`libc::proc_pidpath`), for [`watchdog::in_family`].
#[cfg(target_os = "macos")]
fn exe_path(pid: u32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is a valid writable buffer of the stated size.
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// Not macOS: no `.app` bundles; the family is the bundle-id rule alone.
#[cfg(not(target_os = "macos"))]
fn exe_path(_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 173 R3: every mode asks flexaudio for what the rings assume: 48 kHz mono.
    #[test]
    fn every_mode_opens_48k_mono() {
        for mode in [Mode::System, Mode::Process(4242)] {
            let c = mode.config();
            assert_eq!(
                (c.output.sample_rate, c.output.channels),
                (super::super::RATE, 1)
            );
        }
        let sys = Mode::System.config();
        assert_eq!(sys.kind, flexaudio::SourceKind::SystemLoopback);
        assert!(sys.exclude_self, "never records theuth itself");
        let app = Mode::Process(4242).config();
        assert_eq!(app.kind, flexaudio::SourceKind::ProcessLoopback);
        assert_eq!(app.target_pid, Some(4242));
    }

    /// 173 R3: what each flexaudio event tells the watchdog.
    #[test]
    fn events_map_to_signals() {
        use flexaudio::Event;
        use watchdog::Signal;
        assert_eq!(signal_of(&Event::StreamStalled), Some(Signal::Stalled));
        assert_eq!(signal_of(&Event::StreamRecovered), Some(Signal::Recovered));
        assert_eq!(signal_of(&Event::DeviceLost), Some(Signal::Lost));
        assert_eq!(signal_of(&Event::Error("x".into())), Some(Signal::Lost));
        assert_eq!(
            signal_of(&Event::SilenceWhileSourceActive {
                detail: String::new()
            }),
            Some(Signal::SilentWhileActive)
        );
        assert_eq!(signal_of(&Event::ChunkDropped { count: 3 }), None);
        assert_eq!(signal_of(&Event::Clipped), None);
        assert_eq!(signal_of(&Event::SilenceProbeStarted), None);
    }
}
