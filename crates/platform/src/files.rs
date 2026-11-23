//! Owner-only files and atomic replacement as portable primitives (58 §0.8):
//! mode `0600`/`0700` on Unix, a protected owner-only DACL on Windows.

use std::io;
use std::path::Path;

/// Write `bytes` to `<path>.tmp` readable by the owner only, then rename it
/// over `path`. On Windows the rename retries briefly when an indexer or AV
/// holds the destination (a sharing violation), instead of failing.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = Path::new(&tmp);
    imp::create(tmp, false, bytes)?;
    imp::rename(tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(tmp);
    })
}

/// Write `bytes` to a unique `.<file name>.<pid>.<n>.tmp` beside `path`, then
/// rename it over `path` (the same rename as [`write_private`]); the temp is
/// removed on error. No directory creation, no fsync, no chmod: the umask
/// default, as `fs::write` plus `rename`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, false)
}

/// [`write_atomic`] with `sync_data` on the temp before the rename.
pub fn write_atomic_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, true)
}

fn write_atomic_with(path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_file_name(name);
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        if sync {
            f.sync_data()?;
        }
        Ok(())
    })();
    written
        .and_then(|()| imp::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

/// Create `path` owner-only with `bytes`; `AlreadyExists` when it is there
/// (the caller decides whose wins).
pub fn create_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    imp::create(path, true, bytes)
}

/// Make an existing file owner-only.
pub fn set_private(path: &Path) -> io::Result<()> {
    imp::restrict(path, false)
}

/// `create_dir_all`, the created directories owner-only.
pub fn private_dir(path: &Path) -> io::Result<()> {
    imp::private_dir(path)
}

/// Mark a fresh binary runnable (`0755`); nothing to do on Windows.
pub fn set_executable(path: &Path) -> io::Result<()> {
    imp::set_executable(path)
}

/// Move `new` over `dest`, where `dest` may be the running executable:
/// a rename on Unix (the old image stays mapped), `self-replace` on Windows
/// (a running `.exe` cannot be overwritten, only renamed aside; 58 §0.5).
pub fn replace_executable(new: &Path, dest: &Path) -> io::Result<()> {
    imp::replace_executable(new, dest)
}

/// Whether this process may create and rename entries in `dir`.
pub fn writable(dir: &Path) -> bool {
    imp::writable(dir)
}

#[cfg(unix)]
mod imp {
    use std::io::{self, Write as _};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    use std::path::Path;

    pub fn create(path: &Path, new: bool, bytes: &[u8]) -> io::Result<()> {
        let mut open = std::fs::OpenOptions::new();
        open.write(true).mode(0o600);
        if new {
            open.create_new(true);
        } else {
            open.create(true).truncate(true);
        }
        open.open(path)?.write_all(bytes)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub fn restrict(path: &Path, dir: bool) -> io::Result<()> {
        let mode = if dir { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }

    pub fn private_dir(path: &Path) -> io::Result<()> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }

    pub fn set_executable(path: &Path) -> io::Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }

    pub fn replace_executable(new: &Path, dest: &Path) -> io::Result<()> {
        std::fs::rename(new, dest)
    }

    pub fn writable(dir: &Path) -> bool {
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
}

// ponytail: unverified on Windows — step 5's check: `icacls` on
// `secrets.key`, `link/identity` and the rpc token shows the user only.
#[cfg(windows)]
mod imp {
    use std::io::{self, Write as _};
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_SHARING_VIOLATION, ERROR_SUCCESS, HANDLE,
    };
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, InitializeAcl,
        OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn check(ok: windows_sys::core::BOOL) -> io::Result<()> {
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// The current user's SID as `TOKEN_USER` bytes (the SID points into them).
    fn user_sid() -> io::Result<Vec<u8>> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: the pseudo-handle needs no close; `token` is a valid out pointer.
        check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) })?;
        let mut len = 0u32;
        // SAFETY: a size query; the buffer-too-small error is expected.
        unsafe {
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &raw mut len);
        }
        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` has the length the first call asked for.
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &raw mut len)
        };
        // SAFETY: `token` came from OpenProcessToken above.
        unsafe {
            CloseHandle(token);
        }
        check(ok)?;
        Ok(buf)
    }

    /// Replace `path`'s DACL with one ACE: the current user, full control,
    /// no inheritance from the parent (`PROTECTED`). Directories pass the
    /// ACE on to what is created inside them.
    pub fn restrict(path: &Path, dir: bool) -> io::Result<()> {
        let user = user_sid()?;
        // SAFETY: `user` holds a TOKEN_USER whose SID pointer targets the same buffer.
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        // SAFETY: `sid` is a valid SID from the token.
        let sid_len = unsafe { GetLengthSid(sid) } as usize;
        let acl_len = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            - std::mem::size_of::<u32>()
            + sid_len;
        let mut acl = vec![0u8; acl_len];
        let flags = if dir {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        };
        // SAFETY: `acl` is `acl_len` zeroed bytes, sized for one ACE with this SID.
        unsafe {
            check(InitializeAcl(
                acl.as_mut_ptr().cast(),
                acl_len as u32,
                ACL_REVISION,
            ))?;
            check(AddAccessAllowedAceEx(
                acl.as_mut_ptr().cast(),
                ACL_REVISION,
                flags,
                FILE_ALL_ACCESS,
                sid,
            ))?;
        }
        let name = wide(path);
        // SAFETY: `name` is NUL-terminated; `acl` is an initialised ACL.
        let err = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl.as_ptr().cast(),
                std::ptr::null(),
            )
        };
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(err as i32))
        }
    }

    pub fn create(path: &Path, new: bool, bytes: &[u8]) -> io::Result<()> {
        let mut open = std::fs::OpenOptions::new();
        open.write(true);
        if new {
            open.create_new(true);
        } else {
            open.create(true).truncate(true);
        }
        let mut f = open.open(path)?;
        // Restricted before the contents land; the empty file was briefly inherited-ACL.
        restrict(path, false)?;
        f.write_all(bytes)
    }

    /// `MoveFileExW(REPLACE_EXISTING)` under the hood; a destination held
    /// open by an indexer or AV is a sharing violation for a moment.
    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let mut tries = 0;
        loop {
            match std::fs::rename(from, to) {
                Err(e)
                    if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) && tries < 10 =>
                {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                r => return r,
            }
        }
    }

    pub fn private_dir(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)?;
        restrict(path, true)
    }

    pub fn set_executable(_: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn replace_executable(new: &Path, dest: &Path) -> io::Result<()> {
        let running = std::env::current_exe()
            .ok()
            .and_then(|e| e.canonicalize().ok());
        if dest.canonicalize().ok() == running && running.is_some() {
            self_replace::self_replace(new)?;
            std::fs::remove_file(new)
        } else {
            rename(new, dest)
        }
    }

    pub fn writable(dir: &Path) -> bool {
        std::fs::metadata(dir).is_ok_and(|m| m.is_dir() && !m.permissions().readonly())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("platform-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn write_private_replaces_and_leaves_no_temp() {
        let d = scratch("write");
        private_dir(&d).unwrap();
        let p = d.join("secret");
        write_private(&p, b"one").unwrap();
        write_private(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&p), 0o600);
            assert_eq!(mode(&d), 0o700);
        }
    }

    #[test]
    fn write_atomic_replaces_and_leaves_no_temp() {
        let d = scratch("atomic");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("f.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic_synced(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn write_atomic_concurrent_writers_never_tear() {
        let d = scratch("atomic-race");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("f");
        let handles: Vec<_> = (*b"ab")
            .into_iter()
            .map(|c| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        write_atomic(&p, &vec![c; 4096]).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let got = std::fs::read(&p).unwrap();
        assert_eq!(got.len(), 4096);
        assert!(got.iter().all(|b| *b == got[0]));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn create_private_refuses_an_existing_file() {
        let d = scratch("create");
        private_dir(&d).unwrap();
        let p = d.join("key");
        create_private(&p, b"first").unwrap();
        let e = create_private(&p, b"second").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).unwrap(), b"first");
        assert!(writable(&d));
    }
}
