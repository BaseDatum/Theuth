//! Envelope encryption for the vault key (design doc 225). One random 32-byte data key (DEK)
//! seals the vault exactly as before (it is the "engine key" of 37 §3.5 / 67 §5.4: it seals the
//! mesh keyring and the engine-local `meta` rows). On disk the DEK exists only *wrapped*, once
//! per unlock method, in `<data_dir>/vault-keys.json` (0600):
//!
//! ```json
//! {"version": 1, "methods": [
//!   {"kind": "password", "id": "password", "kdf": {"alg": "argon2id", "m_kib": 65536, "t": 3, "p": 1, "salt": "…"},
//!    "wrapped": "<base64 nonce[24] ‖ XChaCha20-Poly1305(KEK, DEK, aad)>", "created_ms": …},
//!   {"kind": "keychain", "id": "keychain", "wrapped": "…", "created_ms": …},
//!   {"kind": "device", "id": "<owner endpoint id>", "label": "sky-mac", "wrapped": "…", "created_ms": …},
//!   {"kind": "file", "id": "file", "created_ms": …}]}
//! ```
//!
//! `aad = "theuth-vault-dek-1\0<kind>\0<id>"`, so a blob cannot be moved to another method.
//! Adding or removing a method rewraps the DEK only; the vault is never re-encrypted.
//!
//! KEKs: `password` = Argon2id(password, salt, params stored with the blob); `keychain` =
//! SHA-256(domain ‖ the 32-byte keychain item) — the item is the pre-225 key on a migrated
//! desktop, so an older build still opens it; `device` = the 32 bytes a paired owner engine
//! derives for this node and releases over the mesh ([`crate::seal`]); `file` is the legacy
//! plaintext `secrets.key` (no blob: the file *is* the DEK), kept only until a protected
//! method replaces it.

use std::path::Path;

use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::XChaCha20Poly1305;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::vault::KeyStore;

/// The wrapped-key file in the data dir.
pub const KEYS_FILE: &str = "vault-keys.json";
/// Shortest master password accepted (characters).
pub const MIN_PASSWORD: usize = 12;
const NONCE: usize = 24;
const AAD: &str = "theuth-vault-dek-1";
const KEYCHAIN_DOMAIN: &[u8] = b"theuth-vault-keychain-kek-1\0";

/// An unlock method's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "VaultMethodKind"))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// OS keychain (desktops).
    Keychain,
    /// Master password (Argon2id).
    Password,
    /// A paired owner engine releases the KEK over the mesh.
    Device,
    /// Legacy plaintext `secrets.key` (migrated Linux engines); unprotected.
    File,
}

impl Kind {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Keychain => "keychain",
            Kind::Password => "password",
            Kind::Device => "device",
            Kind::File => "file",
        }
    }
}

/// Argon2id parameters, stored with the blob so they can change without breaking old blobs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    /// Always `argon2id`.
    pub alg: String,
    /// Memory, KiB.
    pub m_kib: u32,
    /// Passes.
    pub t: u32,
    /// Lanes.
    pub p: u32,
    /// 16 random bytes, base64.
    pub salt: String,
}

impl Kdf {
    /// Tuned for a 1-vCPU / 2 GB cloud box: 64 MiB, 3 passes, 1 lane (≈0.3-0.8 s there).
    /// Tests use the minimum so the suite stays fast.
    pub fn fresh() -> Self {
        let (m_kib, t) = if cfg!(test) { (8, 1) } else { (64 * 1024, 3) };
        Kdf {
            alg: "argon2id".into(),
            m_kib,
            t,
            p: 1,
            salt: B64.encode(rand::random::<[u8; 16]>()),
        }
    }

    /// The 32-byte KEK for `password`. CPU- and memory-heavy: call off the async runtime.
    pub fn derive(&self, password: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        anyhow::ensure!(self.alg == "argon2id", "unknown kdf {}", self.alg);
        let salt = B64.decode(&self.salt).context("kdf salt")?;
        let params = argon2::Params::new(self.m_kib, self.t, self.p, Some(32))
            .map_err(|e| anyhow::anyhow!("kdf params: {e}"))?;
        let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut out = Zeroizing::new([0u8; 32]);
        a.hash_password_into(password.as_bytes(), &salt, &mut *out)
            .map_err(|e| anyhow::anyhow!("kdf: {e}"))?;
        Ok(out)
    }
}

/// One wrapped copy of the DEK.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wrapped {
    /// Method kind.
    pub kind: Kind,
    /// `keychain` | `password` | `file` | the owner engine's endpoint id (`device`).
    pub id: String,
    /// Display name (a device's engine name); empty = derived from the kind.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Argon2id parameters (`password` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<Kdf>,
    /// base64 `nonce[24] ‖ ciphertext`; empty for `file`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wrapped: String,
    /// ms since epoch.
    pub created_ms: u64,
}

/// `vault-keys.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeyFile {
    /// Format version (1).
    pub version: u32,
    /// Every method that can open the DEK.
    #[serde(default)]
    pub methods: Vec<Wrapped>,
}

impl KeyFile {
    /// The method with this id.
    pub fn find(&self, id: &str) -> Option<&Wrapped> {
        self.methods.iter().find(|m| m.id == id)
    }

    /// Any method that does not leave the DEK readable from the disk alone.
    pub fn protected(&self) -> bool {
        self.methods
            .iter()
            .any(|m| matches!(m.kind, Kind::Password | Kind::Device | Kind::Keychain))
    }

    /// Insert or replace (same id).
    pub fn put(&mut self, w: Wrapped) {
        self.methods.retain(|m| m.id != w.id);
        self.methods.push(w);
    }
}

fn aad(kind: Kind, id: &str) -> Vec<u8> {
    format!("{AAD}\0{}\0{id}", kind.as_str()).into_bytes()
}

/// Wrap `dek` under `kek` for method `(kind, id)`.
pub fn wrap(kek: &[u8; 32], dek: &[u8; 32], kind: Kind, id: &str) -> String {
    let nonce: [u8; NONCE] = rand::random();
    let ct = XChaCha20Poly1305::new(kek.into())
        .encrypt(&nonce.into(), Payload { msg: dek, aad: &aad(kind, id) })
        .expect("XChaCha20-Poly1305 encrypt cannot fail on in-memory data");
    let mut out = Vec::with_capacity(NONCE + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    B64.encode(out)
}

/// Inverse of [`wrap`]; `None` = wrong KEK, another method's blob, or damaged bytes.
pub fn unwrap(kek: &[u8; 32], w: &Wrapped) -> Option<LockedKey> {
    let bytes = B64.decode(&w.wrapped).ok()?;
    if bytes.len() <= NONCE {
        return None;
    }
    let (nonce, ct) = bytes.split_at(NONCE);
    let nonce: [u8; NONCE] = nonce.try_into().ok()?;
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new(kek.into())
            .decrypt(&nonce.into(), Payload { msg: ct, aad: &aad(w.kind, &w.id) })
            .ok()?,
    );
    let mut k: [u8; 32] = plain.as_slice().try_into().ok()?;
    let locked = LockedKey::new(&k);
    k.zeroize();
    Some(locked)
}

/// KEK of the `keychain` method from the item's 32 bytes.
pub fn keychain_kek(item: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut h = Sha256::new();
    h.update(KEYCHAIN_DOMAIN);
    h.update(item);
    Zeroizing::new(h.finalize().into())
}

/// Constant-time equality of two keys.
pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The DEK in memory: heap-pinned, `mlock`ed where the OS allows (best effort), zeroed on drop.
pub struct LockedKey(Box<[u8; 32]>);

impl LockedKey {
    /// Copy `k` into a locked buffer (the caller zeroes its copy).
    pub fn new(k: &[u8; 32]) -> Self {
        let b = Box::new(*k);
        #[cfg(unix)]
        // SAFETY: `b` points to 32 live bytes owned by this box for its whole life; mlock only
        // pins the pages, a failure (RLIMIT_MEMLOCK) leaves them pageable.
        unsafe {
            libc::mlock(b.as_ptr().cast(), 32);
        }
        LockedKey(b)
    }

    /// A fresh random key.
    pub fn random() -> Self {
        let mut k: [u8; 32] = rand::random();
        let l = Self::new(&k);
        k.zeroize();
        l
    }
}

impl std::ops::Deref for LockedKey {
    type Target = [u8; 32];
    fn deref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for LockedKey {
    fn drop(&mut self) {
        self.0.zeroize();
        #[cfg(unix)]
        // SAFETY: the same 32 bytes `new` locked, still owned by the box.
        unsafe {
            libc::munlock(self.0.as_ptr().cast(), 32);
        }
    }
}

impl std::fmt::Debug for LockedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LockedKey(<redacted>)")
    }
}

/// 225 §5: while a DEK is in memory, no core dumps and (Linux) no ptrace by same-uid processes
/// that lack CAP_SYS_PTRACE. Best effort; once set it stays for the process.
pub fn harden_process() {
    #[cfg(unix)]
    // SAFETY: plain syscalls with constant, valid arguments.
    unsafe {
        let zero = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &zero);
        #[cfg(target_os = "linux")]
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}

/// The OS keychain, behind a trait so tests never touch the real one.
pub trait Keychain: Send + Sync {
    /// The item's 32 bytes; `Ok(None)` = no item; `Err` = store failure or a corrupt item.
    fn read(&self, data_dir: &Path) -> anyhow::Result<Option<[u8; 32]>>;
    /// Create or replace the item.
    fn write(&self, data_dir: &Path, key: &[u8; 32]) -> anyhow::Result<()>;
}

/// The real keychain (key.rs: macOS Keychain, Secret Service, Windows Credential Manager).
pub struct OsKeychain;

impl Keychain for OsKeychain {
    fn read(&self, data_dir: &Path) -> anyhow::Result<Option<[u8; 32]>> {
        crate::key::keychain_read(data_dir)
    }
    fn write(&self, data_dir: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
        crate::key::keychain_write(data_dir, key)
    }
}

/// `vault-keys.json`, `None` when absent (pre-225 engine, or a fresh data dir).
pub fn load(data_dir: &Path) -> anyhow::Result<Option<KeyFile>> {
    let path = data_dir.join(KEYS_FILE);
    match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .with_context(|| format!("{}: not a vault key file", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| path.display().to_string()),
    }
}

/// Write `vault-keys.json` atomically (tmp 0600, fsync, rename).
pub fn save(data_dir: &Path, f: &KeyFile) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir).with_context(|| data_dir.display().to_string())?;
    let path = data_dir.join(KEYS_FILE);
    let tmp = data_dir.join(format!("{KEYS_FILE}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let json = serde_json::to_vec_pretty(f)?;
    platform::files::create_private(&tmp, &json)
        .with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(h) = std::fs::File::open(&tmp) {
        let _ = h.sync_all();
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming to {}", path.display()))
}

/// What boot found.
pub enum Boot {
    /// The DEK opened; `by` = `keychain` | `file` | `memory` (a fresh `key_store: sealed`
    /// vault no method protects yet).
    Unsealed {
        /// The key.
        dek: LockedKey,
        /// How.
        by: &'static str,
    },
    /// No method opens without the user: start sealed.
    Sealed {
        /// Plain text for status and logs.
        reason: String,
    },
}

/// 225 §4: open the DEK without a human, migrating a pre-225 data dir on first start:
/// - `vault-keys.json` exists: try `file`, then `keychain`; otherwise [`Boot::Sealed`].
/// - none, `secrets.key` exists (Linux, pre-225): the file *is* the DEK → method `file`.
/// - none, a pre-225 keychain item exists: the item *is* the DEK → method `keychain` wrapping it
///   under `SHA-256(domain ‖ item)` (no keychain write, no prompt; an older build still opens it).
/// - fresh: `auto`/`keychain` mint a keychain item and a separate DEK; `auto` without a keychain
///   falls back to `file` (logged, as before); `file` → `file`; `sealed` → a DEK in memory only,
///   no method yet (the creator adds a password or device method at once).
pub fn boot(data_dir: &Path, ks: KeyStore, kc: &dyn Keychain) -> anyhow::Result<Boot> {
    if let Some(f) = load(data_dir)? {
        for m in &f.methods {
            match m.kind {
                Kind::File => match crate::key::file_read(data_dir)? {
                    Some(mut k) => {
                        let dek = LockedKey::new(&k);
                        k.zeroize();
                        return Ok(Boot::Unsealed { dek, by: "file" });
                    }
                    None => tracing::warn!("secrets: vault-keys.json names a key file but secrets.key is gone"),
                },
                Kind::Keychain if !matches!(ks, KeyStore::File | KeyStore::Sealed) => {
                    match kc.read(data_dir) {
                        Ok(Some(mut item)) => {
                            let kek = keychain_kek(&item);
                            item.zeroize();
                            match unwrap(&kek, m) {
                                Some(dek) => return Ok(Boot::Unsealed { dek, by: "keychain" }),
                                None => tracing::warn!("secrets: the keychain item does not open the vault key"),
                            }
                        }
                        Ok(None) => tracing::warn!("secrets: the vault's keychain item is gone"),
                        Err(e) => tracing::warn!("secrets: keychain: {e:#}"),
                    }
                }
                _ => {}
            }
        }
        if f.methods.is_empty() && !data_dir.join(crate::store::DB).exists() {
            // A `sealed` vault restarted before anything was stored or protected: start over.
            return Ok(Boot::Unsealed { dek: LockedKey::random(), by: "memory" });
        }
        let reason = if f.methods.is_empty() {
            "no unlock method was ever added to this vault; its secrets cannot be opened".to_owned()
        } else {
            "the vault is sealed: unlock it from an owner device or with the master password".to_owned()
        };
        return Ok(Boot::Sealed { reason });
    }
    if let Some(mut k) = crate::key::file_read(data_dir)? {
        let dek = LockedKey::new(&k);
        k.zeroize();
        save(data_dir, &file_only())?;
        tracing::warn!("secrets: vault key stored unprotected on disk (secrets.key) — add a master password (`t vault add-password`)");
        return Ok(Boot::Unsealed { dek, by: "file" });
    }
    match ks {
        KeyStore::File => fresh_file(data_dir),
        KeyStore::Sealed => {
            save(data_dir, &KeyFile { version: 1, methods: Vec::new() })?;
            Ok(Boot::Unsealed { dek: LockedKey::random(), by: "memory" })
        }
        KeyStore::Keychain | KeyStore::Auto => match keychain_boot(data_dir, kc) {
            Ok(b) => Ok(b),
            Err(e) if ks == KeyStore::Auto => match crate::key::fallback_reason(&e) {
                Some(reason) => {
                    tracing::warn!("{reason}; vault key in {} (mode 0600)", data_dir.join(crate::key::FILE).display());
                    fresh_file(data_dir)
                }
                None => Err(e),
            },
            Err(e) => Err(e),
        },
    }
}

fn file_only() -> KeyFile {
    KeyFile {
        version: 1,
        methods: vec![Wrapped {
            kind: Kind::File,
            id: "file".into(),
            label: String::new(),
            kdf: None,
            wrapped: String::new(),
            created_ms: platform::time::now_ms(),
        }],
    }
}

fn fresh_file(data_dir: &Path) -> anyhow::Result<Boot> {
    let mut k = crate::key::file_create(data_dir)?;
    let dek = LockedKey::new(&k);
    k.zeroize();
    save(data_dir, &file_only())?;
    Ok(Boot::Unsealed { dek, by: "file" })
}

fn keychain_boot(data_dir: &Path, kc: &dyn Keychain) -> anyhow::Result<Boot> {
    let (dek, item) = match kc.read(data_dir)? {
        // Pre-225: the item is the engine key itself.
        Some(item) => (LockedKey::new(&item), Zeroizing::new(item)),
        None => {
            let item = Zeroizing::new(rand::random::<[u8; 32]>());
            kc.write(data_dir, &item)?;
            (LockedKey::random(), item)
        }
    };
    let w = Wrapped {
        kind: Kind::Keychain,
        id: "keychain".into(),
        label: String::new(),
        kdf: None,
        wrapped: wrap(&keychain_kek(&item), &dek, Kind::Keychain, "keychain"),
        created_ms: platform::time::now_ms(),
    };
    save(data_dir, &KeyFile { version: 1, methods: vec![w] })?;
    Ok(Boot::Unsealed { dek, by: "keychain" })
}

/// A `password` method for `dek` (Argon2id; slow — off the async runtime).
pub fn password_method(dek: &[u8; 32], password: &str) -> anyhow::Result<Wrapped> {
    anyhow::ensure!(
        password.chars().count() >= MIN_PASSWORD,
        "the master password needs at least {MIN_PASSWORD} characters"
    );
    let kdf = Kdf::fresh();
    let kek = kdf.derive(password)?;
    Ok(Wrapped {
        kind: Kind::Password,
        id: "password".into(),
        label: String::new(),
        wrapped: wrap(&kek, dek, Kind::Password, "password"),
        kdf: Some(kdf),
        created_ms: platform::time::now_ms(),
    })
}

/// Open the `password` method of `f` (slow — off the async runtime). `Ok(None)` = wrong password.
pub fn open_password(f: &KeyFile, password: &str) -> anyhow::Result<Option<LockedKey>> {
    let w = f
        .find("password")
        .filter(|w| w.kind == Kind::Password)
        .context("no master password is set for this vault")?;
    let kek = w.kdf.as_ref().context("password method without kdf")?.derive(password)?;
    Ok(unwrap(&kek, w))
}

/// Methods on disk without unsealing (offline `t vault methods`).
pub fn methods_offline(data_dir: &Path) -> anyhow::Result<Vec<crate::seal::MethodView>> {
    Ok(load(data_dir)?
        .unwrap_or_default()
        .methods
        .iter()
        .map(crate::seal::MethodView::of)
        .collect())
}

/// 225 provisioning (`t vault init`): a fresh data dir only — mint a DEK, wrap it under
/// `password`, write `vault-keys.json`. The DEK never touches the disk unwrapped; the engine
/// then boots sealed and opens with the password.
pub fn init_password(data_dir: &Path, password: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        load(data_dir)?.is_none() && !data_dir.join(crate::store::DB).exists() && !data_dir.join(crate::key::FILE).exists(),
        "{} already has a vault; use `t vault add-password` on the running engine",
        data_dir.display()
    );
    let dek = LockedKey::random();
    let w = password_method(&dek, password)?;
    save(data_dir, &KeyFile { version: 1, methods: vec![w] })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An in-memory keychain; `fail` = the store is unavailable.
    #[derive(Default)]
    pub(crate) struct MemKeychain {
        pub item: Mutex<Option<[u8; 32]>>,
        pub fail: bool,
    }

    impl Keychain for MemKeychain {
        fn read(&self, _: &Path) -> anyhow::Result<Option<[u8; 32]>> {
            if self.fail {
                return Err(keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("no keychain"))).into());
            }
            Ok(*self.item.lock().unwrap())
        }
        fn write(&self, _: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
            if self.fail {
                return Err(keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("no keychain"))).into());
            }
            *self.item.lock().unwrap() = Some(*key);
            Ok(())
        }
    }

    fn unsealed(b: Boot) -> (LockedKey, &'static str) {
        match b {
            Boot::Unsealed { dek, by } => (dek, by),
            Boot::Sealed { reason } => panic!("sealed: {reason}"),
        }
    }

    #[test]
    fn wrap_round_trips_and_binds_the_method() {
        let kek = [7u8; 32];
        let dek = [9u8; 32];
        let blob = wrap(&kek, &dek, Kind::Device, "peer-a");
        let w = |kind, id: &str| Wrapped {
            kind,
            id: id.into(),
            label: String::new(),
            kdf: None,
            wrapped: blob.clone(),
            created_ms: 0,
        };
        assert_eq!(*unwrap(&kek, &w(Kind::Device, "peer-a")).unwrap(), dek);
        assert!(unwrap(&[8u8; 32], &w(Kind::Device, "peer-a")).is_none(), "wrong kek");
        assert!(unwrap(&kek, &w(Kind::Device, "peer-b")).is_none(), "moved to another id");
        assert!(unwrap(&kek, &w(Kind::Password, "peer-a")).is_none(), "moved to another kind");
    }

    #[test]
    fn password_method_opens_only_with_its_password() {
        let dek = [3u8; 32];
        assert!(password_method(&dek, "short").is_err());
        let w = password_method(&dek, "correct horse battery").unwrap();
        let f = KeyFile { version: 1, methods: vec![w] };
        assert_eq!(*open_password(&f, "correct horse battery").unwrap().unwrap(), dek);
        assert!(open_password(&f, "wrong horse battery!").unwrap().is_none());
        // Params ride with the blob.
        assert_eq!(f.methods[0].kdf.as_ref().unwrap().alg, "argon2id");
    }

    #[test]
    fn migrates_a_file_key_without_changing_it() {
        let dir = tempfile::tempdir().unwrap();
        let k = crate::key::file_create(dir.path()).unwrap();
        let (dek, by) = unsealed(boot(dir.path(), KeyStore::Auto, &MemKeychain::default()).unwrap());
        assert_eq!((*dek, by), (k, "file"));
        let f = load(dir.path()).unwrap().unwrap();
        assert_eq!(f.methods.len(), 1);
        assert_eq!(f.methods[0].kind, Kind::File);
        assert!(!f.protected());
        // Second boot: same key from the same file.
        let (again, _) = unsealed(boot(dir.path(), KeyStore::Auto, &MemKeychain::default()).unwrap());
        assert_eq!(*again, k);
    }

    #[test]
    fn migrates_a_keychain_key_without_a_keychain_write() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = [5u8; 32];
        let kc = MemKeychain { item: Mutex::new(Some(legacy)), fail: false };
        let (dek, by) = unsealed(boot(dir.path(), KeyStore::Auto, &kc).unwrap());
        assert_eq!((*dek, by), (legacy, "keychain"));
        assert_eq!(*kc.item.lock().unwrap(), Some(legacy), "item untouched (older builds still open it)");
        let (again, by) = unsealed(boot(dir.path(), KeyStore::Auto, &kc).unwrap());
        assert_eq!((*again, by), (legacy, "keychain"));
        assert!(!dir.path().join(crate::key::FILE).exists());
    }

    #[test]
    fn fresh_keychain_dek_differs_from_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let kc = MemKeychain::default();
        let (dek, by) = unsealed(boot(dir.path(), KeyStore::Auto, &kc).unwrap());
        assert_eq!(by, "keychain");
        assert_ne!(Some(*dek), *kc.item.lock().unwrap());
        let (again, _) = unsealed(boot(dir.path(), KeyStore::Auto, &kc).unwrap());
        assert_eq!(*again, *dek);
    }

    #[test]
    fn no_keychain_falls_back_to_the_file_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let kc = MemKeychain { fail: true, ..Default::default() };
        let (_, by) = unsealed(boot(dir.path(), KeyStore::Auto, &kc).unwrap());
        assert_eq!(by, "file");
        assert!(boot(tempfile::tempdir().unwrap().path(), KeyStore::Keychain, &kc).is_err());
    }

    #[test]
    fn password_only_vault_boots_sealed_and_never_writes_a_key_file() {
        let dir = tempfile::tempdir().unwrap();
        init_password(dir.path(), "a long enough password").unwrap();
        assert!(init_password(dir.path(), "a long enough password").is_err(), "only once");
        match boot(dir.path(), KeyStore::Auto, &MemKeychain::default()).unwrap() {
            Boot::Sealed { reason } => assert!(reason.contains("sealed"), "{reason}"),
            Boot::Unsealed { .. } => panic!("must start sealed"),
        }
        assert!(!dir.path().join(crate::key::FILE).exists());
        let f = load(dir.path()).unwrap().unwrap();
        assert!(open_password(&f, "a long enough password").unwrap().is_some());
        let raw = std::fs::read_to_string(dir.path().join(KEYS_FILE)).unwrap();
        assert!(!raw.contains("a long enough password"));
    }

    #[test]
    fn sealed_store_keeps_the_dek_in_memory_only() {
        let dir = tempfile::tempdir().unwrap();
        let (_, by) = unsealed(boot(dir.path(), KeyStore::Sealed, &MemKeychain::default()).unwrap());
        assert_eq!(by, "memory");
        assert!(!dir.path().join(crate::key::FILE).exists());
        assert!(load(dir.path()).unwrap().unwrap().methods.is_empty());
    }
}
