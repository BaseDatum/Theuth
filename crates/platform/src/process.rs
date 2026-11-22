//! Process trees addressed as one (58 §0.4): a Unix process group, a Windows
//! Job Object. The spawner (brush, portable-pty) puts the leader in its own
//! group/session; the tree is adopted from its pid.

use std::io;

/// A signal a job's `send` delivers to its tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Signal {
    Int,
    Term,
    Hup,
    Quit,
    Kill,
}

/// The processes one job started. Clones address the same tree; on Windows
/// the Job Object closes (and kills what is left in it) with the last clone.
#[derive(Clone, Debug)]
pub struct ProcessTree(imp::Tree);

impl ProcessTree {
    /// The tree rooted at `leader`, already running. Unix: its process
    /// group (the leader was spawned with `setpgid`/`setsid`, so pgid ==
    /// pid). Windows: a new Job Object with `KILL_ON_JOB_CLOSE` the leader
    /// is assigned to.
    pub fn adopt(leader: u32) -> io::Result<Self> {
        imp::adopt(leader).map(Self)
    }

    /// Another running member. Unix: already in the group by the way it
    /// was spawned (no-op). Windows: assigned to the job.
    pub fn add(&self, pid: u32) -> io::Result<()> {
        imp::add(&self.0, pid)
    }

    /// The leader's id (the pgid on Unix).
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    /// `SIGKILL` the group / terminate the job. A tree that is already
    /// gone is not an error.
    pub fn kill(&self) {
        let _ = self.signal(Signal::Kill);
    }

    /// `SIGINT` / Ctrl-C.
    pub fn interrupt(&self) -> io::Result<()> {
        self.signal(Signal::Int)
    }

    /// Deliver `sig` to every member.
    pub fn signal(&self, sig: Signal) -> io::Result<()> {
        imp::signal(&self.0, sig)
    }

    /// Any member left. On Unix an unreaped zombie still counts, so this
    /// also waits out the reaper.
    pub fn alive(&self) -> bool {
        imp::alive(&self.0)
    }

    /// 155 §6.2: the tree's size and summed CPU time now — Windows from the
    /// Job Object accounting, Unix by walking the leader's descendants
    /// (bounded at [`TREE_MAX`] members). `None` when it cannot be read: the
    /// leader is gone, the list call fails or the tree is over the bound; a
    /// descendant that vanishes mid-walk is just not counted.
    pub fn sample(&self) -> Option<TreeSample> {
        imp::sample(&self.0)
    }
}

/// One reading of a [`ProcessTree`] (155 §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeSample {
    /// Live members, the leader included.
    pub processes: u32,
    /// User + system CPU time of every member since it started.
    pub cpu: std::time::Duration,
}

/// The most members `ProcessTree::sample` walks before giving up (155 §6.2).
pub const TREE_MAX: usize = 256;

/// Whether the process `pid` still exists (signal 0 / a live exit code).
pub fn pid_alive(pid: u32) -> bool {
    imp::pid_alive(pid)
}

/// Resolves when the OS asks this process to stop: `SIGTERM` on Unix, a
/// console close or system shutdown on Windows. Ctrl-C is
/// `tokio::signal::ctrl_c` on both.
pub async fn stop_signal() -> io::Result<()> {
    imp::stop_signal().await
}

/// Run `cmd` in its own session: no controlling terminal to steal (Unix
/// `setsid` in the child; a no-op on Windows, which has no sessions).
pub fn own_session(cmd: &mut tokio::process::Command) {
    imp::own_session(cmd);
}

/// Give a console-subsystem child no console. Windows allocates a fresh, visible
/// console for a console child whose parent has none — `t.exe --headless` under
/// the desktop app is such a parent, so a plain `Command::new` there put a blank
/// window on the desktop for every helper (`theuth-app-parser-*.exe`, git, tar,
/// hostname) and kept it there for as long as the helper lived. `CREATE_NO_WINDOW`
/// (0x08000000) skips the allocation; pipes still work. A no-op elsewhere. Not for
/// PTY children (ConPTY brings its own console) or a child meant to show a window.
/// std's `creation_flags` replaces rather than ORs, so call this last if the
/// command sets flags of its own. tokio: `hide_console(cmd.as_std_mut())`.
pub fn hide_console(cmd: &mut std::process::Command) {
    imp::hide_console(cmd);
}

/// This machine's hostname, read once (`hostname`, a hidden console child);
/// `"theuth"` when that fails or prints nothing.
pub fn hostname() -> &'static str {
    static HOSTNAME: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut cmd = std::process::Command::new("hostname");
        hide_console(&mut cmd);
        cmd.output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "theuth".to_owned())
    });
    &HOSTNAME
}

/// This machine's facts (183 §5.6): `name` is the mesh name once the mesh
/// has started (`set_machine_name`), the hostname before.
pub struct Machine {
    /// This engine's mesh name (hostname until the mesh starts).
    pub name: String,
    /// `hostname()`.
    pub host: String,
    /// `std::env::consts::OS`.
    pub os: &'static str,
    /// Human OS name (`/etc/os-release` NAME on Linux).
    pub os_name: String,
    /// `std::env::consts::ARCH`.
    pub arch: &'static str,
}

static MACHINE: std::sync::OnceLock<Machine> = std::sync::OnceLock::new();

fn build_machine(name: &str) -> Machine {
    let os = std::env::consts::OS;
    let os_name = match os {
        "linux" => std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| os_release_name(&t)),
        "macos" => Some("macOS".to_owned()),
        "windows" => Some("Windows".to_owned()),
        _ => None,
    };
    Machine {
        name: name.to_owned(),
        host: hostname().to_owned(),
        os,
        os_name: os_name.unwrap_or_else(|| os.to_owned()),
        arch: std::env::consts::ARCH,
    }
}

/// `NAME` from os-release text, quotes stripped.
fn os_release_name(text: &str) -> Option<String> {
    let v = text.lines().find_map(|l| l.strip_prefix("NAME="))?;
    let v = v.trim().trim_matches(['"', '\'']);
    (!v.is_empty()).then(|| v.to_owned())
}

/// Computed once per process (the name fixed by the first of `machine` / `set_machine_name`).
pub fn machine() -> &'static Machine {
    MACHINE.get_or_init(|| build_machine(hostname()))
}

/// The mesh calls this at start with its effective name. First call wins:
/// a no-op once `machine()` has been read.
pub fn set_machine_name(name: &str) {
    let _ = MACHINE.set(build_machine(name));
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::time::Duration;

    use super::{Signal, TREE_MAX, TreeSample};

    #[derive(Clone, Copy, Debug)]
    pub struct Tree(i32);

    impl Tree {
        pub fn id(&self) -> u32 {
            self.0 as u32
        }
    }

    pub fn adopt(leader: u32) -> io::Result<Tree> {
        Ok(Tree(leader as i32))
    }

    pub fn add(_: &Tree, _: u32) -> io::Result<()> {
        Ok(())
    }

    fn signo(sig: Signal) -> i32 {
        match sig {
            Signal::Int => libc::SIGINT,
            Signal::Term => libc::SIGTERM,
            Signal::Hup => libc::SIGHUP,
            Signal::Quit => libc::SIGQUIT,
            Signal::Kill => libc::SIGKILL,
        }
    }

    pub fn signal(t: &Tree, sig: Signal) -> io::Result<()> {
        // SAFETY: killpg takes two integers and touches no memory.
        if unsafe { libc::killpg(t.0, signo(sig)) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn alive(t: &Tree) -> bool {
        // SAFETY: as above; signal 0 only checks for members.
        unsafe { libc::killpg(t.0, 0) == 0 }
    }

    pub fn pid_alive(pid: u32) -> bool {
        // SAFETY: as above.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    pub async fn stop_signal() -> io::Result<()> {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
            .recv()
            .await;
        Ok(())
    }

    pub fn own_session(cmd: &mut tokio::process::Command) {
        // SAFETY: `setsid` is async-signal-safe and touches no parent state.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    pub fn hide_console(_: &mut std::process::Command) {}

    /// Unix walk: the leader plus every descendant reachable through live
    /// parent links (an orphan reparented to init is no longer ours).
    pub fn sample(t: &Tree) -> Option<TreeSample> {
        let mut pids = vec![t.0];
        let mut cpu = Duration::ZERO;
        let mut processes = 0u32;
        let mut i = 0;
        while let Some(&pid) = pids.get(i) {
            match os::cpu_of(pid) {
                Some(c) => {
                    cpu += c;
                    processes += 1;
                }
                // The leader unreadable is the tree gone; a child that
                // exited between the listing and the read is just over.
                None if i == 0 => return None,
                None => {
                    i += 1;
                    continue;
                }
            }
            os::children(pid, &mut pids)?;
            if pids.len() > TREE_MAX {
                return None;
            }
            i += 1;
        }
        Some(TreeSample { processes, cpu })
    }

    #[cfg(target_os = "macos")]
    mod os {
        use std::time::Duration;

        use super::TREE_MAX;

        /// `pti_total_user + pti_total_system`: `task_absolutetime_info`, so
        /// mach absolute time units (24 MHz on Apple silicon, not nanoseconds).
        pub fn cpu_of(pid: i32) -> Option<Duration> {
            let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
            let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
            // SAFETY: `info` is a `proc_taskinfo` the flavour names, with its exact size.
            let n = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTASKINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if n != size {
                return None;
            }
            // SAFETY: the kernel filled all `size` bytes.
            let info = unsafe { info.assume_init() };
            let ticks = u128::from(info.pti_total_user + info.pti_total_system);
            let (numer, denom) = *TIMEBASE;
            Some(Duration::from_nanos(
                (ticks * numer / denom).try_into().ok()?,
            ))
        }

        /// `<mach/mach_time.h>`; libc deprecates its copy in favour of the
        /// `mach2` crate, which nothing here depends on.
        #[repr(C)]
        struct MachTimebase {
            numer: u32,
            denom: u32,
        }
        unsafe extern "C" {
            fn mach_timebase_info(info: *mut MachTimebase) -> libc::c_int;
        }

        /// The timebase as `(numer, denom)`, read once.
        static TIMEBASE: std::sync::LazyLock<(u128, u128)> = std::sync::LazyLock::new(|| {
            let mut tb = MachTimebase { numer: 0, denom: 0 };
            // SAFETY: a plain out-pointer to the struct the call fills.
            if unsafe { mach_timebase_info(&raw mut tb) } != libc::KERN_SUCCESS || tb.denom == 0 {
                return (1, 1);
            }
            (u128::from(tb.numer), u128::from(tb.denom))
        });

        /// Append `pid`'s children; `None` when the call fails or there are
        /// more than the bound (the buffer would have been truncated).
        pub fn children(pid: i32, out: &mut Vec<i32>) -> Option<()> {
            let mut buf = [0 as libc::pid_t; TREE_MAX + 1];
            let size = std::mem::size_of_val(&buf) as libc::c_int;
            // SAFETY: `buf` is `size` bytes of writable pids.
            // Unlike `proc_listpids`, this returns the number of pids (observed
            // on macOS 26: 2 for two children where `proc_listpids` says 8).
            let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), size) };
            let n = usize::try_from(n).ok()?;
            if n > TREE_MAX {
                return None;
            }
            out.extend_from_slice(&buf[..n]);
            Some(())
        }
    }

    #[cfg(not(target_os = "macos"))]
    mod os {
        //! Linux (and other procfs unices): `/proc/<pid>/stat` fields 4
        //! (ppid), 14 and 15 (utime, stime in clock ticks).
        use std::time::Duration;

        /// `(ppid, utime + stime)` of `pid`; `None` for a zombie or a gone pid.
        fn stat(pid: i32) -> Option<(i32, Duration)> {
            let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // The command name may contain spaces; fields resume after its ')'.
            let rest = s.get(s.rfind(')')? + 2..)?;
            let mut f = rest.split_ascii_whitespace();
            let state = f.next()?;
            if state == "Z" {
                return None;
            }
            let ppid: i32 = f.next()?.parse().ok()?;
            let utime: u64 = f.nth(9)?.parse().ok()?;
            let stime: u64 = f.next()?.parse().ok()?;
            // SAFETY: a plain query with no memory effects.
            let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            let hz = u64::try_from(hz).ok().filter(|h| *h > 0)?;
            Some((
                ppid,
                Duration::from_nanos((utime + stime) * 1_000_000_000 / hz),
            ))
        }

        pub fn cpu_of(pid: i32) -> Option<Duration> {
            stat(pid).map(|(_, cpu)| cpu)
        }

        /// One pass over `/proc` for the pids whose parent is `pid`.
        pub fn children(pid: i32, out: &mut Vec<i32>) -> Option<()> {
            for entry in std::fs::read_dir("/proc").ok()? {
                let Ok(entry) = entry else { continue };
                let Some(child) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<i32>().ok())
                else {
                    continue;
                };
                if stat(child).is_some_and(|(ppid, _)| ppid == pid) {
                    out.push(child);
                }
            }
            Some(())
        }
    }
}

// ponytail: unverified on Windows — step 2's check: `shell("timeout 30")`
// tree dead after cancel; `interrupt()` on a ConPTY child (it shares no
// console with us, so `GenerateConsoleCtrlEvent` is expected to fail: the
// honest Ctrl-C there is 0x03 on the pty input).
#[cfg(windows)]
mod imp {
    use std::io;
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
    use windows_sys::Win32::System::Console::{CTRL_C_EVENT, GenerateConsoleCtrlEvent};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    use super::{Signal, TreeSample};

    /// An owned kernel handle, closed on drop.
    #[derive(Debug)]
    struct Handle(HANDLE);

    // SAFETY: a HANDLE is an opaque kernel reference usable from any thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a handle this struct owns exclusively.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn check(ok: windows_sys::core::BOOL) -> io::Result<()> {
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[derive(Clone, Debug)]
    pub struct Tree {
        job: Arc<Handle>,
        leader: u32,
    }

    impl Tree {
        pub fn id(&self) -> u32 {
            self.leader
        }
    }

    pub fn adopt(leader: u32) -> io::Result<Tree> {
        // SAFETY: null attributes and name create an anonymous job.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Arc::new(Handle(job));
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is the struct the class names, with its exact size.
        check(unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        let t = Tree { job, leader };
        add(&t, leader)?;
        Ok(t)
    }

    pub fn add(t: &Tree, pid: u32) -> io::Result<()> {
        // SAFETY: OpenProcess with a pid; a null result is checked.
        let h = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let h = Handle(h);
        // SAFETY: both handles are live and owned.
        check(unsafe { AssignProcessToJobObject(t.job.0, h.0) })
    }

    pub fn signal(t: &Tree, sig: Signal) -> io::Result<()> {
        match sig {
            // SAFETY: two integers; the target group is the leader's pid.
            Signal::Int => check(unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, t.leader) }),
            // SAFETY: the job handle is live; 137 mirrors a SIGKILL exit.
            _ => check(unsafe { TerminateJobObject(t.job.0, 137) }),
        }
    }

    /// The job's accounting, `None` when the query fails.
    fn accounting(t: &Tree) -> Option<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `info` is the struct the class names, with its exact size.
        let ok = unsafe {
            QueryInformationJobObject(
                t.job.0,
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(info)
    }

    pub fn alive(t: &Tree) -> bool {
        accounting(t).is_some_and(|info| info.ActiveProcesses > 0)
    }

    /// `ActiveProcesses` and the job's user + kernel time (100 ns units,
    /// terminated members included, so it never decreases).
    pub fn sample(t: &Tree) -> Option<TreeSample> {
        let info = accounting(t)?;
        let ticks =
            u64::try_from(info.TotalUserTime).ok()? + u64::try_from(info.TotalKernelTime).ok()?;
        Some(TreeSample {
            processes: info.ActiveProcesses,
            cpu: std::time::Duration::from_nanos(ticks * 100),
        })
    }

    pub fn pid_alive(pid: u32) -> bool {
        // SAFETY: OpenProcess with a pid; a null result is checked.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return false;
        }
        let h = Handle(h);
        let mut code = 0u32;
        // SAFETY: `h` is live; `code` is a valid out pointer.
        unsafe { GetExitCodeProcess(h.0, &raw mut code) != 0 && code == STILL_ACTIVE as u32 }
    }

    pub async fn stop_signal() -> io::Result<()> {
        let mut close = tokio::signal::windows::ctrl_close()?;
        let mut shutdown = tokio::signal::windows::ctrl_shutdown()?;
        tokio::select! {
            _ = close.recv() => {}
            _ = shutdown.recv() => {}
        }
        Ok(())
    }

    pub fn own_session(_: &mut tokio::process::Command) {}

    pub fn hide_console(cmd: &mut std::process::Command) {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn os_release_name_strips_quotes() {
        assert_eq!(
            os_release_name("PRETTY_NAME=\"x y\"\nNAME=\"Ubuntu\"\nID=ubuntu\n").as_deref(),
            Some("Ubuntu")
        );
        assert_eq!(os_release_name("NAME=Arch\n").as_deref(), Some("Arch"));
        assert_eq!(os_release_name("ID=x\n"), None);
    }

    use std::time::Duration;

    use super::*;

    #[test]
    fn own_pid_is_alive_and_a_dead_pid_is_not() {
        assert!(pid_alive(std::process::id()));
        // Above every OS's pid_max (and, as an i32, positive: not a group).
        assert!(!pid_alive(i32::MAX as u32));
    }

    #[cfg(unix)]
    #[test]
    fn tree_of_a_child_group_dies_with_kill() {
        use std::os::unix::process::CommandExt as _;
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let tree = ProcessTree::adopt(child.id()).unwrap();
        assert_eq!(tree.id(), child.id());
        assert!(tree.alive());
        tree.kill();
        child.wait().unwrap();
        assert!(!tree.alive());
        assert!(!pid_alive(child.id()));
    }

    /// 155 §6.2: `sample` counts the leader's descendants and sums a CPU
    /// that only grows; a tree whose leader is reaped reads `None`.
    #[cfg(unix)]
    #[test]
    fn sample_counts_descendants_and_cpu_grows() {
        use std::os::unix::process::CommandExt as _;
        /// Kills the tree however the test ends (a panic must not leak `yes`).
        struct Reap(ProcessTree);
        impl Drop for Reap {
            fn drop(&mut self) {
                self.0.kill();
            }
        }
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30 & yes > /dev/null & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let reap = Reap(ProcessTree::adopt(child.id()).unwrap());
        let tree = &reap.0;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        // sh + two sleeps + yes.
        let first = loop {
            let s = tree.sample().unwrap();
            if s.processes >= 4 {
                break s;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "tree never filled: {s:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        // `yes` burns a core: within 2 s the summed CPU grows by ≥ 100 ms.
        let grown = loop {
            std::thread::sleep(Duration::from_millis(100));
            let s = tree.sample().unwrap();
            assert!(s.cpu >= first.cpu, "cpu went backwards: {first:?} → {s:?}");
            if s.cpu - first.cpu >= Duration::from_millis(100) {
                break s;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no cpu growth: {first:?} → {s:?}"
            );
        };
        assert!(grown.processes >= 4, "{grown:?}");
        tree.kill();
        child.wait().unwrap();
        assert_eq!(tree.sample(), None);
    }

    /// 155 §6.2 on Windows: the Job Object accounting behind `sample`.
    #[cfg(windows)]
    #[test]
    fn sample_reads_the_job_accounting() {
        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep 30"])
            .spawn()
            .unwrap();
        let tree = ProcessTree::adopt(child.id()).unwrap();
        let first = tree.sample().unwrap();
        assert!(first.processes >= 1, "{first:?}");
        std::thread::sleep(Duration::from_millis(200));
        let second = tree.sample().unwrap();
        assert!(second.cpu >= first.cpu, "{first:?} → {second:?}");
        tree.kill();
        child.wait().unwrap();
    }

    #[test]
    fn hide_console_is_callable_everywhere() {
        let mut cmd = std::process::Command::new("true");
        hide_console(&mut cmd);
        let mut tokio_cmd = tokio::process::Command::new("true");
        hide_console(tokio_cmd.as_std_mut());
    }

    #[cfg(windows)]
    #[test]
    fn hide_console_spawns_a_console_child_without_a_window() {
        // `Command` has no flags getter, so the contract is observed from the child:
        // under `CREATE_NO_WINDOW` a console process has no console window to report.
        let mut ps = std::process::Command::new("powershell");
        ps.args([
            "-NoProfile",
            "-Command",
            "Add-Type -Name W -Namespace K -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern System.IntPtr GetConsoleWindow();'; [K.W]::GetConsoleWindow()",
        ]);
        hide_console(&mut ps);
        let out = ps.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "0",
            "child reports a console window"
        );
    }
}
