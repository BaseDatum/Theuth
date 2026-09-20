//! The 32-byte vault key (design doc 37 §3.5): an OS keychain item (service
//! `theuth`, account `secrets:<absolute data_dir>`) or `<data_dir>/secrets.key`
//! (base64, mode 0600). `Auto` tries the keychain and falls back to the file
//! on any keyring error, saying so in the log.

use std::io::Write as _;
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
            Err(e) if e.downcast_ref::<keyring::Error>().is_some() => {
                tracing::warn!(
                    "no keychain available ({e}); vault key in {} (mode 0600)",
                    data_dir.join(FILE).display()
                );
                Ok((file(data_dir)?, "file"))
            }
            Err(e) => Err(e),
        },
    }
}

fn keychain(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let abs = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let account = format!("secrets:{}", abs.display());
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
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        open.mode(0o600);
    }
    match open.open(&path) {
        Ok(mut f) => {
            f.write_all(B64.encode(key).as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            Ok(key)
        }
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
