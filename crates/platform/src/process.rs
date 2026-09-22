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
}

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

#[cfg(unix)]
mod imp {
    use std::io;

    use super::Signal;

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
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
        PROCESS_TERMINATE,
    };

    use super::Signal;

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
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
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

    pub fn alive(t: &Tree) -> bool {
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
        ok != 0 && info.ActiveProcesses > 0
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
}

#[cfg(test)]
mod tests {
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
}
