//! Which shell runs a script for us (58 §0.3): the user's login shell on
//! macOS/Linux (it exists to inherit the login environment), brush
//! in-process on Windows (the caller that owns `brush_core` runs it).
//! Interactive PTY jobs are the *user's* terminal and get [`pty_command`].

use std::path::{Path, PathBuf};

/// How `$(cmd)`-style scripts run on this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellKind {
    /// `$SHELL` (else `/bin/sh`): spawn it with `-c`.
    Login(PathBuf),
    /// No login shell to inherit from: run the script in-process with brush.
    Brush,
}

/// The shell for scripts on this host.
pub fn shell() -> ShellKind {
    if cfg!(windows) {
        ShellKind::Brush
    } else {
        ShellKind::Login(
            std::env::var_os("SHELL")
                .filter(|s| !s.is_empty())
                .map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from),
        )
    }
}

impl ShellKind {
    /// `<shell> -c script` (`-lc` with `login`), stdin closed; `None` for
    /// [`ShellKind::Brush`], whose caller runs `script` in-process.
    pub fn command(&self, script: &str, login: bool) -> Option<tokio::process::Command> {
        let Self::Login(path) = self else {
            return None;
        };
        let mut cmd = tokio::process::Command::new(path);
        cmd.arg(if login { "-lc" } else { "-c" })
            .arg(script)
            .stdin(std::process::Stdio::null());
        Some(cmd)
    }
}

/// The program and arguments that run `command` on a PTY (22 §4.1):
/// `$SHELL -lc <command>` (else `/bin/sh`) on Unix; `pwsh.exe` when it is on
/// `PATH`, else `powershell.exe`, with `-NoLogo -Command` on Windows.
/// `override` is `tools.pty_shell`, taken as the program on either.
pub fn pty_command(command: &str, override_shell: Option<&Path>) -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        let program = override_shell
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(if on_path("pwsh.exe") { "pwsh.exe" } else { "powershell.exe" }));
        (program, vec!["-NoLogo".into(), "-Command".into(), command.to_owned()])
    } else {
        let program = override_shell.map(Path::to_path_buf).unwrap_or_else(|| {
            std::env::var_os("SHELL")
                .filter(|s| !s.is_empty())
                .map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from)
        });
        (program, vec!["-lc".into(), command.to_owned()])
    }
}

/// Whether `exe` resolves through `PATH`.
fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_is_the_pty_program_everywhere() {
        let (program, args) = pty_command("ls", Some(Path::new("/opt/fish")));
        assert_eq!(program, PathBuf::from("/opt/fish"));
        assert_eq!(args.last().map(String::as_str), Some("ls"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_runs_the_login_shell_with_lc() {
        let (program, args) = pty_command("ls", None);
        let login = std::env::var_os("SHELL").filter(|s| !s.is_empty()).map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from);
        assert_eq!(program, login);
        assert_eq!(args, ["-lc", "ls"]);
        assert_eq!(shell(), ShellKind::Login(login.clone()));
        let cmd = ShellKind::Login(login).command("echo hi", true).unwrap();
        assert_eq!(cmd.as_std().get_args().collect::<Vec<_>>(), ["-lc", "echo hi"]);
    }
}
