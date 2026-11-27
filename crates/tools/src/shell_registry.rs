//! In-memory interactive shell registry for engine/node terminals.
//!
//! Completely independent of [`crate::Jobs`], sessions, sqlite, `<job_result>`,
//! and `THEUTH_AGENT_PTY`: a node terminal is a plain interactive login shell
//! keyed by a short shell id (default `"main"`). Closing the terminal window
//! removes the entry and sends `SIGHUP` to its process group (escalating if a
//! child ignores `SIGHUP`) so no shell leaks.

use std::path::{Path, PathBuf};

/// Default shell id for a node's interactive terminal.
pub const DEFAULT_SHELL_ID: &str = "main";

/// Maximum length of a shell id.
const MAX_ID_LEN: usize = 64;

/// `$SHELL` when it names an existing absolute path, else `/bin/bash`, else `/bin/sh`.
pub fn login_shell(shell: Option<PathBuf>, exists: impl Fn(&Path) -> bool) -> PathBuf {
    shell
        .filter(|s| s.is_absolute() && exists(s))
        .or_else(|| Some(PathBuf::from("/bin/bash")).filter(|b| exists(b)))
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// The data dir's parent (the `engine` user's home on a node), else `$HOME`.
pub fn shell_cwd(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.to_path_buf())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(format!("shell id must be 1..={MAX_ID_LEN} characters"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid shell id `{id}`"));
    }
    Ok(())
}

#[cfg(not(target_os = "ios"))]
mod imp {
    use std::collections::HashMap;
    use std::io::Write;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::Duration;

    use platform::{ProcessTree, Signal};
    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
    use tokio::sync::{broadcast, mpsc};

    use crate::jobs::{JobExit, lock};
    use crate::pty::{
        AGENT_PTY_ENV, COLS, COLS_MAX, HISTORY, Key, PtyAttached, PtyOut, ROWS, ROWS_MAX,
        STREAM_CHUNKS, dims, exit_of, nz, read_loop, replay, respond_writer,
    };

    use super::validate_id;

    struct LiveShell {
        writer: Mutex<Option<Box<dyn Write + Send>>>,
        master: Mutex<Option<Box<dyn MasterPty + Send>>>,
        parser: Mutex<vt100::Parser>,
        stream: broadcast::Sender<PtyOut>,
        tree: ProcessTree,
        #[cfg_attr(not(test), allow(dead_code))]
        pid: u32,
        exit: Mutex<Option<JobExit>>,
        alive: AtomicBool,
    }

    impl LiveShell {
        fn is_alive(&self) -> bool {
            self.alive.load(SeqCst)
        }

        fn release(&self) {
            drop(lock(&self.master).take());
            drop(lock(&self.writer).take());
        }

        fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
            if !(2..=ROWS_MAX).contains(&rows) || !(10..=COLS_MAX).contains(&cols) {
                return Err(format!(
                    "size {cols}x{rows} out of range (10..={COLS_MAX} x 2..={ROWS_MAX})"
                ));
            }
            let master = lock(&self.master);
            let Some(master) = master.as_ref() else {
                return Err("shell exited".into());
            };
            {
                let mut p = lock(&self.parser);
                if dims(p.screen()) == (rows, cols) {
                    return Ok(());
                }
                p.screen_mut().set_size(nz(rows), nz(cols));
            }
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| format!("resize shell: {e:#}"))
        }

        fn input(&self, text: Option<&str>, keys: &[Key]) -> Result<(), String> {
            let mut out = text.map(|t| t.as_bytes().to_vec()).unwrap_or_default();
            if !keys.is_empty() {
                let p = lock(&self.parser);
                for k in keys {
                    k.encode(&mut out, p.screen())?;
                }
            }
            if out.is_empty() {
                return Ok(());
            }
            let mut w = lock(&self.writer);
            let Some(w) = w.as_mut() else {
                return Err("shell exited".into());
            };
            w.write_all(&out)
                .and_then(|()| w.flush())
                .map_err(|e| format!("write shell: {e}"))
        }

        fn attach(&self, size: Option<(u16, u16)>) -> Result<PtyAttached, String> {
            if let Some((rows, cols)) = size
                && self.is_alive()
            {
                let _ = self.resize(rows, cols);
            }
            let (screen, rx) = {
                let p = lock(&self.parser);
                (p.screen().clone(), self.stream.subscribe())
            };
            let (rows, cols) = dims(&screen);
            let exit = lock(&self.exit).clone();
            Ok(PtyAttached {
                replay: replay(screen),
                rows,
                cols,
                rx,
                exit,
            })
        }

        fn hangup(&self) {
            self.alive.store(false, SeqCst);
            self.release();
            let _ = self.tree.signal(Signal::Hup);
        }
    }

    /// Process-local in-memory registry of interactive node shells.
    pub struct ShellRegistry {
        shells: Mutex<HashMap<String, Arc<LiveShell>>>,
    }

    impl Default for ShellRegistry {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ShellRegistry {
        /// Create an empty registry.
        pub fn new() -> Self {
            Self {
                shells: Mutex::new(HashMap::new()),
            }
        }

        /// The engine's process-wide in-memory shell registry.
        pub fn global() -> &'static Self {
            static REGISTRY: LazyLock<ShellRegistry> = LazyLock::new(ShellRegistry::new);
            &REGISTRY
        }

        /// Open a shell for `id`, or reuse the running one already registered under `id`.
        pub fn open_or_reuse(
            &self,
            id: &str,
            program: (&Path, &[String]),
            cwd: &Path,
            cols: Option<u16>,
            rows: Option<u16>,
        ) -> Result<String, String> {
            validate_id(id)?;
            let rows = rows.unwrap_or(ROWS);
            let cols = cols.unwrap_or(COLS);
            if !(2..=ROWS_MAX).contains(&rows) || !(10..=COLS_MAX).contains(&cols) {
                return Err(format!(
                    "size {cols}x{rows} out of range (10..={COLS_MAX} x 2..={ROWS_MAX})"
                ));
            }

            let mut guard = lock(&self.shells);
            if let Some(existing) = guard.get(id)
                && existing.is_alive()
            {
                return Ok(id.to_owned());
            }

            let (shell, child, rx) = spawn_shell(program, cwd, rows, cols)?;
            guard.insert(id.to_owned(), Arc::clone(&shell));
            drop(guard);

            tokio::spawn(reap_shell(shell, child, rx));
            Ok(id.to_owned())
        }

        /// Attach to shell `id`, optionally resizing it before taking the replay snapshot.
        pub fn attach(&self, id: &str, size: Option<(u16, u16)>) -> Result<PtyAttached, String> {
            let shell = self.get(id)?;
            shell.attach(size)
        }

        /// Write `text` and/or encoded `keys` to shell `id`.
        pub fn input(&self, id: &str, text: Option<&str>, keys: &[Key]) -> Result<(), String> {
            let shell = self.get(id)?;
            shell.input(text, keys)
        }

        /// Resize shell `id` (sends `SIGWINCH`).
        pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), String> {
            let shell = self.get(id)?;
            shell.resize(rows, cols)
        }

        /// Close shell `id`: remove it from the registry and `SIGHUP` its process group
        /// (escalating to `SIGTERM` / `SIGKILL` if a descendant ignores `SIGHUP`).
        /// Returns `true` if a shell was present.
        pub fn close(&self, id: &str) -> bool {
            let Some(shell) = lock(&self.shells).remove(id) else {
                return false;
            };
            terminate_shell(shell);
            true
        }

        fn get(&self, id: &str) -> Result<Arc<LiveShell>, String> {
            lock(&self.shells)
                .get(id)
                .cloned()
                .ok_or_else(|| format!("no such shell `{id}`"))
        }

        #[cfg(test)]
        pub(crate) fn pid(&self, id: &str) -> Option<u32> {
            lock(&self.shells).get(id).map(|s| s.pid)
        }
    }

    impl Drop for ShellRegistry {
        fn drop(&mut self) {
            let shells: Vec<Arc<LiveShell>> = lock(&self.shells).drain().map(|(_, s)| s).collect();
            for shell in shells {
                shell.hangup();
                if shell.tree.alive() {
                    shell.tree.kill();
                }
            }
        }
    }

    fn spawn_shell(
        program: (&Path, &[String]),
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<
        (
            Arc<LiveShell>,
            Box<dyn Child + Send + Sync>,
            mpsc::Receiver<Vec<u8>>,
        ),
        String,
    > {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("openpty: {e:#}"))?;
        let mut cmd = CommandBuilder::new(program.0);
        cmd.args(program.1);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        // Invariant: a human's node terminal is never an agent-driven PTY.
        cmd.env_remove(AGENT_PTY_ENV);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawn {}: {e:#}", program.0.display()))?;
        drop(pair.slave);

        let pid = child
            .process_id()
            .ok_or_else(|| "shell child has no pid".to_owned())?;
        let tree = ProcessTree::adopt(pid).map_err(|e| format!("adopt process group: {e}"))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("clone pty reader: {e:#}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("take pty writer: {e:#}"))?;

        let (chunk_tx, rx) = mpsc::channel(64);
        std::thread::spawn(move || read_loop(reader, &chunk_tx));

        let shell = Arc::new(LiveShell {
            writer: Mutex::new(Some(writer)),
            master: Mutex::new(Some(pair.master)),
            parser: Mutex::new(vt100::Parser::new(nz(rows), nz(cols), HISTORY)),
            stream: broadcast::channel(STREAM_CHUNKS).0,
            tree,
            pid,
            exit: Mutex::new(None),
            alive: AtomicBool::new(true),
        });
        Ok((shell, child, rx))
    }

    async fn reap_shell(
        shell: Arc<LiveShell>,
        mut child: Box<dyn Child + Send + Sync>,
        mut rx: mpsc::Receiver<Vec<u8>>,
    ) {
        let mut wait = tokio::task::spawn_blocking(move || child.wait());
        let mut carry = Vec::new();
        let exit = loop {
            tokio::select! {
                res = &mut wait => {
                    // Drain any trailing bytes flushed as the shell exited.
                    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
                    while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline, rx.recv()).await {
                        process_chunk(&shell, &mut carry, &chunk);
                    }
                    break exit_of(res);
                }
                chunk = rx.recv() => match chunk {
                    Some(chunk) => process_chunk(&shell, &mut carry, &chunk),
                    None => break exit_of((&mut wait).await),
                }
            }
        };
        shell.alive.store(false, SeqCst);
        *lock(&shell.exit) = Some(exit.clone());
        shell.release();
        let _ = shell.stream.send(PtyOut::Exit(exit));
    }

    fn process_chunk(shell: &LiveShell, carry: &mut Vec<u8>, chunk: &[u8]) {
        let cursor = {
            let mut p = lock(&shell.parser);
            p.process(chunk);
            let _ = shell.stream.send(PtyOut::Data(Arc::from(chunk)));
            p.screen().cursor_position()
        };
        respond_writer(&shell.writer, carry, chunk, cursor);
    }

    fn terminate_shell(shell: Arc<LiveShell>) {
        shell.hangup();
        let tree = shell.tree.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                escalate_kill(tree).await;
            });
        } else {
            std::thread::spawn(move || {
                for _ in 0..30 {
                    if !tree.alive() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = tree.signal(Signal::Term);
                for _ in 0..10 {
                    if !tree.alive() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if tree.alive() {
                    tree.kill();
                }
            });
        }
    }

    async fn escalate_kill(tree: ProcessTree) {
        for _ in 0..30 {
            if !tree.alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = tree.signal(Signal::Term);
        for _ in 0..10 {
            if !tree.alive() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if tree.alive() {
            tree.kill();
        }
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::path::Path;
    use std::sync::LazyLock;

    use crate::pty::{Key, PtyAttached};

    /// Stub shell registry on iOS (interactive local PTYs are unavailable).
    #[derive(Default)]
    pub struct ShellRegistry;

    impl ShellRegistry {
        /// Create an empty registry.
        pub fn new() -> Self {
            Self
        }

        /// Process-wide registry stub.
        pub fn global() -> &'static Self {
            static REGISTRY: LazyLock<ShellRegistry> = LazyLock::new(ShellRegistry::new);
            &REGISTRY
        }

        /// Unavailable on iOS.
        pub fn open_or_reuse(
            &self,
            _id: &str,
            _program: (&Path, &[String]),
            _cwd: &Path,
            _cols: Option<u16>,
            _rows: Option<u16>,
        ) -> Result<String, String> {
            Err("interactive shells are not supported on iOS".into())
        }

        /// Unavailable on iOS.
        pub fn attach(&self, _id: &str, _size: Option<(u16, u16)>) -> Result<PtyAttached, String> {
            Err("interactive shells are not supported on iOS".into())
        }

        /// Unavailable on iOS.
        pub fn input(&self, _id: &str, _text: Option<&str>, _keys: &[Key]) -> Result<(), String> {
            Err("interactive shells are not supported on iOS".into())
        }

        /// Unavailable on iOS.
        pub fn resize(&self, _id: &str, _rows: u16, _cols: u16) -> Result<(), String> {
            Err("interactive shells are not supported on iOS".into())
        }

        /// No-op on iOS.
        pub fn close(&self, _id: &str) -> bool {
            false
        }
    }
}

pub use imp::ShellRegistry;

/// Convenience access to the process-wide [`ShellRegistry`].
pub fn global() -> &'static ShellRegistry {
    ShellRegistry::global()
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::pty::{Key, PtyOut};

    #[test]
    fn login_shell_prefers_existing_absolute_shell() {
        let all = |_: &Path| true;
        let none = |_: &Path| false;
        let zsh = Some(PathBuf::from("/bin/zsh"));
        assert_eq!(login_shell(zsh.clone(), all), PathBuf::from("/bin/zsh"));
        assert_eq!(login_shell(Some("zsh".into()), all), PathBuf::from("/bin/bash"));
        assert_eq!(login_shell(None, all), PathBuf::from("/bin/bash"));
        assert_eq!(login_shell(zsh, none), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn invalid_shell_ids_are_rejected() {
        let reg = ShellRegistry::new();
        let cwd = std::env::temp_dir();
        let sh = Path::new("/bin/sh");
        for bad in ["", "a/b", "a b", &"x".repeat(65)] {
            assert!(
                reg.open_or_reuse(bad, (sh, &[]), &cwd, None, None).is_err(),
                "expected `{bad}` to be rejected"
            );
        }
    }

    #[tokio::test]
    async fn open_input_output_reuse_resize_and_close_kills_child() {
        let reg = ShellRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let sh = Path::new("/bin/sh");

        let id = reg
            .open_or_reuse(DEFAULT_SHELL_ID, (sh, &[]), dir.path(), Some(80), Some(24))
            .expect("open shell");
        assert_eq!(id, DEFAULT_SHELL_ID);
        let pid = reg.pid(DEFAULT_SHELL_ID).expect("pid recorded");
        assert!(platform::process::pid_alive(pid));

        // Opening the same id reuses the live shell rather than spawning a second one.
        let reused = reg
            .open_or_reuse(DEFAULT_SHELL_ID, (sh, &[]), dir.path(), Some(80), Some(24))
            .expect("reuse shell");
        assert_eq!(reused, DEFAULT_SHELL_ID);
        assert_eq!(reg.pid(DEFAULT_SHELL_ID), Some(pid));

        let mut at = reg
            .attach(DEFAULT_SHELL_ID, Some((30, 100)))
            .expect("attach");
        assert_eq!((at.rows, at.cols), (30, 100));

        // Verify input/output and that THEUTH_AGENT_PTY is NOT set.
        reg.input(
            DEFAULT_SHELL_ID,
            Some("printf 'NODE_OK:%s\\n' \"${THEUTH_AGENT_PTY:-unset}\""),
            &[Key::Enter],
        )
        .expect("input");

        let mut buf = String::from_utf8_lossy(&at.replay).into_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !buf.contains("NODE_OK:unset") {
            let chunk = tokio::time::timeout_at(deadline, at.rx.recv())
                .await
                .expect("timed out waiting for shell output")
                .expect("broadcast open");
            if let PtyOut::Data(bytes) = chunk {
                buf.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        assert!(buf.contains("NODE_OK:unset"), "output was: {buf}");

        // Resize updates the grid.
        reg.resize(DEFAULT_SHELL_ID, 35, 110).expect("resize");
        let at2 = reg.attach(DEFAULT_SHELL_ID, None).expect("reattach");
        assert_eq!((at2.rows, at2.cols), (35, 110));

        // Close removes the entry and kills the child process group.
        assert!(reg.close(DEFAULT_SHELL_ID));
        assert!(!reg.close(DEFAULT_SHELL_ID), "close is idempotent");
        assert!(reg.attach(DEFAULT_SHELL_ID, None).is_err());

        // Wait up to 3s for the child process to terminate and be reaped.
        let gone_by = tokio::time::Instant::now() + Duration::from_secs(3);
        while platform::process::pid_alive(pid) && tokio::time::Instant::now() < gone_by {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !platform::process::pid_alive(pid),
            "shell child pid {pid} leaked after close"
        );
    }
}
