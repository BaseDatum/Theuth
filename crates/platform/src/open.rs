//! Hand something to the OS default handler, detached (58 §4): `open` on
//! macOS, `xdg-open` on Linux, `ShellExecuteW` on Windows.

use std::ffi::OsStr;
use std::io;

/// Open `url` in the user's default browser (their real, signed-in one).
pub fn open_url(url: &str) -> io::Result<()> {
    imp::open(OsStr::new(url))
}

/// Open a local file or directory with its default application.
pub fn open_path(path: &std::path::Path) -> io::Result<()> {
    imp::open(path.as_os_str())
}

/// Launch or bring forward an application by name (`Safari`, `TextEdit`).
pub fn open_app(name: &str) -> io::Result<()> {
    imp::open_app(name)
}

#[cfg(all(unix, not(target_os = "ios")))]
mod imp {
    use std::ffi::OsStr;
    use std::io;
    use std::process::{Command, Stdio};

    fn detached(mut cmd: Command) -> io::Result<()> {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
    }

    pub fn open(target: &OsStr) -> io::Result<()> {
        let mut cmd = Command::new(if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        });
        cmd.arg(target);
        detached(cmd)
    }

    pub fn open_app(name: &str) -> io::Result<()> {
        if cfg!(target_os = "macos") {
            let mut cmd = Command::new("open");
            cmd.arg("-a").arg(name);
            detached(cmd)
        } else {
            detached(Command::new(name))
        }
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::ffi::OsStr;
    use std::io;

    pub fn open(target: &OsStr) -> io::Result<()> {
        tracing::debug!(
            target = %target.to_string_lossy(),
            "platform::open::open is a no-op on iOS"
        );
        Ok(())
    }

    pub fn open_app(name: &str) -> io::Result<()> {
        tracing::debug!(app = name, "platform::open::open_app is a no-op on iOS");
        Ok(())
    }
}

// ponytail: unverified on Windows — step 4's check: OAuth login opens the default browser.
#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    pub fn open(target: &OsStr) -> io::Result<()> {
        let file: Vec<u16> = target.encode_wide().chain(std::iter::once(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: both strings are NUL-terminated; the rest are documented nulls.
        let h = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // Documented: a value above 32 is success; below, an error code.
        if h as usize > 32 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn open_app(name: &str) -> io::Result<()> {
        open(OsStr::new(name))
    }
}
