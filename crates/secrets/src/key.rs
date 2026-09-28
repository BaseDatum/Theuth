//! The 32-byte vault key (design doc 37 §3.5): an OS keychain item (service
//! `theuth`, account `secrets:<absolute data_dir>`) or `<data_dir>/secrets.key`
//! (base64, mode 0600). `Auto` tries the keychain and falls back to the file
//! on any keyring error, saying so — backend and raw error — in the log.
//!
//! macOS and Linux go through the `keyring` crate. Windows talks to the
//! Credential Manager directly ([`cred`]): keyring 3 hard-codes
//! `CRED_PERSIST_ENTERPRISE`, which `CredWriteW` refuses with
//! `ERROR_NO_SUCH_LOGON_SESSION` for local (non-domain) accounts;
//! `CRED_PERSIST_LOCAL_MACHINE` is what a per-machine key wants anyway.

use std::path::Path;

use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use zeroize::Zeroizing;

use crate::vault::KeyStore;

const SERVICE: &str = "theuth";
const FILE: &str = "secrets.key";

/// Load the key for `data_dir`, creating it on first use. Returns the key
/// and where it lives: `"keychain"` or `"file"`.
pub fn load_or_create(data_dir: &Path, ks: KeyStore) -> anyhow::Result<([u8; 32], &'static str)> {
    match ks {
        KeyStore::File => Ok((file(data_dir)?, "file")),
        KeyStore::Keychain => Ok((keychain(data_dir)?, "keychain")),
        KeyStore::Auto => match keychain(data_dir) {
            Ok(k) => Ok((k, "keychain")),
            Err(e) => match fallback_reason(&e) {
                Some(reason) => {
                    tracing::warn!(
                        "{reason}; vault key in {} (mode 0600)",
                        data_dir.join(FILE).display()
                    );
                    Ok((file(data_dir)?, "file"))
                }
                None => Err(e),
            },
        },
    }
}

/// The keychain backend this build talks to, for the log.
const BACKEND: &str = if cfg!(target_os = "macos") {
    "macOS Keychain (keyring apple-native)"
} else if cfg!(windows) {
    "Windows Credential Manager (CredReadW/CredWriteW, CRED_PERSIST_LOCAL_MACHINE)"
} else {
    "Secret Service (keyring sync-secret-service)"
};

/// `Auto` falls back to the file only when the *store* failed (no keychain,
/// no logon session, platform error): `Some(log line)`. A corrupt item is
/// `None` — that is an error to surface, not a reason to mint a second key.
fn fallback_reason(e: &anyhow::Error) -> Option<String> {
    let store = e.downcast_ref::<keyring::Error>()?;
    Some(format!("no keychain available: {BACKEND}: {store}"))
}

#[cfg(not(windows))]
fn keychain(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let account = account(data_dir);
    let entry = keyring::Entry::new(SERVICE, &account)?;
    match entry.get_password() {
        Ok(b64) => decode(&Zeroizing::new(b64)).with_context(|| {
            format!("keychain item {SERVICE}/{account} is not a 32-byte base64 key")
        }),
        Err(keyring::Error::NoEntry) => {
            let key: [u8; 32] = rand::random();
            entry.set_password(&B64.encode(key))?;
            Ok(key)
        }
        Err(e) => Err(e.into()),
    }
}

/// Same item keyring would address (`<account>.<service>`), so a key written
/// by either is read by the other.
#[cfg(windows)]
fn keychain(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let account = account(data_dir);
    let target = format!("{account}.{SERVICE}");
    match cred::read(&target) {
        Ok(b64) => {
            decode(&b64).with_context(|| format!("credential {target} is not a 32-byte base64 key"))
        }
        Err(keyring::Error::NoEntry) => {
            let key: [u8; 32] = rand::random();
            cred::write(&target, &account, &B64.encode(key))?;
            Ok(key)
        }
        Err(e) => Err(e.into()),
    }
}

fn account(data_dir: &Path) -> String {
    let abs = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    format!("secrets:{}", abs.display())
}

/// Generic credentials in the Windows Credential Manager, `CRED_PERSIST_LOCAL_MACHINE`,
/// UTF-16LE blob as keyring writes it. Errors are keyring's so the fallback
/// classification is one code path.
#[cfg(windows)]
mod cred {
    use windows_sys::Win32::Foundation::{
        ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND, FILETIME, GetLastError,
    };
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW, CredWriteW,
    };
    use zeroize::Zeroizing;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// keyring's mapping (its `decode_error`): not found → `NoEntry`, no logon
    /// session → `NoStorageAccess`, else `PlatformFailure`; the io::Error keeps the code.
    fn last_error() -> keyring::Error {
        // SAFETY: reads this thread's last-error value; no pointers.
        let code = unsafe { GetLastError() };
        let e = Box::new(std::io::Error::from_raw_os_error(code as i32));
        match code {
            ERROR_NOT_FOUND => keyring::Error::NoEntry,
            ERROR_NO_SUCH_LOGON_SESSION => keyring::Error::NoStorageAccess(e),
            _ => keyring::Error::PlatformFailure(e),
        }
    }

    pub fn read(target: &str) -> keyring::Result<Zeroizing<String>> {
        let target = wide(target);
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated and `p` a valid out pointer; a zero result is checked before `p` is used.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &raw mut p) } == 0 {
            return Err(last_error());
        }
        // SAFETY: CredReadW succeeded, so `p` is a credential we own until the one CredFree below.
        let units: Zeroizing<Vec<u16>> = unsafe {
            let c = &*p;
            let blob = if c.CredentialBlobSize == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
            };
            let units = blob
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            CredFree(p.cast());
            Zeroizing::new(units)
        };
        Ok(Zeroizing::new(String::from_utf16_lossy(&units)))
    }

    pub fn write(target: &str, username: &str, password: &str) -> keyring::Result<()> {
        let mut target = wide(target);
        let mut username = wide(username);
        let mut comment = wide("theuth secrets vault key");
        let mut blob: Zeroizing<Vec<u8>> =
            Zeroizing::new(password.encode_utf16().flat_map(u16::to_le_bytes).collect());
        let c = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            LastWritten: FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: username.as_mut_ptr(),
        };
        // SAFETY: every pointer in `c` is into a local that outlives the call.
        if unsafe { CredWriteW(&raw const c, 0) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }
}

fn file(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join(FILE);
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map(Zeroizing::new)
            .map_err(anyhow::Error::from)
            .and_then(|s| decode(&s))
            .with_context(|| format!("{}: not a 32-byte base64 key", path.display()))
    };
    if path.exists() {
        return read(&path);
    }
    std::fs::create_dir_all(data_dir).with_context(|| data_dir.display().to_string())?;
    let key: [u8; 32] = rand::random();
    match platform::files::create_private(&path, B64.encode(key).as_bytes()) {
        Ok(()) => Ok(key),
        // Another process created it between our check and ours: theirs is the key.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read(&path),
        Err(e) => Err(e).with_context(|| format!("creating {}", path.display())),
    }
}

fn decode(b64: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = Zeroizing::new(B64.decode(b64.trim())?);
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| anyhow::anyhow!("key is {} bytes, not 32", bytes.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store failure falls back (and names the backend and the raw error);
    /// a corrupt item does not — that would silently mint a second key.
    #[test]
    fn fallback_only_for_store_errors() {
        let store: anyhow::Error =
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::from_raw_os_error(1312)))
                .into();
        let reason = fallback_reason(&store).expect("store error falls back");
        assert!(reason.contains(BACKEND), "{reason}");
        assert!(reason.contains("1312"), "{reason}");
        // keyring::Error under an anyhow context is still the store's error.
        let wrapped = store.context("keychain");
        assert!(fallback_reason(&wrapped).is_some());

        let corrupt = decode("not base64!").expect_err("bad key");
        assert!(fallback_reason(&corrupt).is_none());
        assert!(fallback_reason(&decode(&B64.encode([0u8; 7])).unwrap_err()).is_none());
    }
}
