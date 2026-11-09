//! The vault (design doc 37 §3.5, replicated per 67 §5.4): rows in
//! `<data_dir>/secrets.sqlite`, one per `(name, version)`, each value sealed
//! with XChaCha20-Poly1305 under the **mesh key** — one key every paired
//! engine shares, so a row written anywhere opens everywhere. The mesh key
//! itself sits in the local `meta` table sealed under this engine's own key
//! (OS keychain or `secrets.key`), so a stolen `secrets.sqlite` is exactly as
//! useful as a stolen `secrets.vault` was. Reads are served from an in-memory
//! copy that [`Vault::refresh`] rebuilds after every own write, when the
//! plugin sees the file move (a peer's rows, `t --secret-set`), or on demand.
//! Values never leave this module except through [`Vault::get`] and
//! [`Vault::expand`]; [`Vault::redact`] turns them back into placeholders.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, PoisonError, RwLock, RwLockReadGuard};
use std::time::SystemTime;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use zeroize::Zeroizing;

use crate::store;

mod rotation;
pub use rotation::{
    LEGACY_CHECK_MS, MAX_EPOCH_JUMP, REMOVED_MACHINE, RETIRE_GRACE_MS, SWEEP_BATCH, SWEEP_GRACE_MS,
    SWEEP_HOLD_MS, key_id,
};

/// Versions kept per name. Not config: a sixth is the one nobody asked for.
pub const KEEP: usize = 5;

/// Values shorter than this (in chars) are not scrubbed from tool output:
/// too many false positives, and the receipt says so.
pub const MIN_REDACT: usize = 8;

/// Regex source for a placeholder: group 1 = name, group 2 = versions back (1..=4).
pub const PLACEHOLDER: &str = r"\{\{secret:([A-Za-z0-9_.\-]+)(?::-([1-4]))?\}\}";

/// Header of the pre-67 single-file vault, still read once for migration.
const MAGIC: &[u8] = b"theuth-secrets-1\n";
/// The pre-67 file; renamed to `secrets.vault.migrated` once its rows are in.
const LEGACY_FILE: &str = "secrets.vault";
const NONCE: usize = 24;
/// `meta` key of the sealed mesh key; its aad.
const MESH_KEY: &str = "mesh_key";
const MESH_KEY_AAD: &[u8] = b"theuth-mesh-key-1";

/// Where the vault key lives (`secrets.key_store`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStore {
    /// Keychain, else file (logged).
    #[default]
    Auto,
    /// OS keychain only; fail if unavailable.
    Keychain,
    /// `secrets.key` only.
    File,
}

/// One kept version. Wire-visible: never a value, never a length.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VersionInfo {
    /// Per-name monotonic number; never reused.
    pub v: u32,
    /// RFC 3339 UTC.
    pub created: String,
    /// Who wrote it: `user`, `cli`, `shell`, `import`, `restore v3`, `provider add`, …
    pub source: String,
}

/// One secret as the model and the UIs see it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretInfo {
    /// `[A-Za-z0-9_.-]{1,64}`.
    pub name: String,
    /// Per-name description, shown in lists.
    pub note: String,
    /// Backend; always `local` in v0.
    pub source: String,
    /// `v` of the current version.
    pub current: u32,
    /// Newest first, at most [`KEEP`].
    pub versions: Vec<VersionInfo>,
    /// `created` of the current version.
    pub updated: String,
}

/// `secret_list` / `list_secrets` result.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretsList {
    /// `keychain` or `file`.
    pub key_store: String,
    /// Sorted by name.
    pub secrets: Vec<SecretInfo>,
}

/// 183 §14.7 (D7): one mesh key's identity and order. Keys order by `(epoch, initiator)`;
/// the legacy key is `(0, "")`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct KeyInfo {
    /// `hex(sha256("theuth-mesh-key-id-1" ‖ key)[0..8])` (W-29); rows carry it in `vault.key_id`.
    pub id: String,
    /// `0` = the legacy key.
    pub epoch: u32,
    /// The engine that minted it; `""` for the legacy key.
    pub initiator: String,
    /// Engines it was minted for or granted to (W-34).
    pub members: Vec<String>,
    /// When it was minted (ms since epoch; 0 for the legacy key).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub minted_ms: u64,
}

/// 183 §14.7: a key as it travels on `theuth/mesh/1` (`PairHello`/`PairAck.keyring`,
/// `KeyGrant`). Engine-internal: carries key bytes, never ts-exported, never logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireKey {
    /// Which key.
    pub info: KeyInfo,
    /// The 32 key bytes, base32 (no padding), as `PairHello.vault_key`.
    pub key: Zeroizing<String>,
}

impl std::fmt::Debug for WireKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireKey")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// 183 §14.7: where a rotation is (§12.9 banner).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum RotationPhase {
    /// The new key is being minted and stored.
    Minting,
    /// Rows are being re-sealed under it.
    Sweeping,
    /// Every local row is swept; waiting for offline machines and the retire grace.
    Waiting,
    /// The old key is retired.
    Done,
    /// Stopped; the old key stays current (`error`, Retry = `vault_rotate`).
    Failed,
}

/// 183 §14.7: one remaining machine as the rotation sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RotationPeer {
    /// Engine id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Connected now.
    pub online: bool,
    /// It acknowledged the new key (`KeyAck`).
    pub has_key: bool,
    /// Connected 5 s without sending `KeyEpoch`: a pre-183 build (§6.9.10).
    pub old_build: bool,
}

/// 183 §14.7: the newest rotation (`meta["rotation"]`, `vault_rotation`,
/// `SecretsEvent::Rotation`). Names only, never values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RotationState {
    /// The key rotated to.
    pub key: KeyInfo,
    /// Engines whose unpair started it; empty for a manual rotation.
    pub removed: Vec<String>,
    /// Where it is.
    pub phase: RotationPhase,
    /// When it started (ms since epoch).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub started_ms: u64,
    /// `vault` rows to re-seal.
    pub rows_total: u32,
    /// Rows already under the new key.
    pub rows_done: u32,
    /// Secret names whose rows open under no held key (§13 row 41).
    pub rows_stuck: Vec<String>,
    /// Every remaining machine.
    pub peers: Vec<RotationPeer>,
    /// The previous key is retired here.
    pub old_destroyed: bool,
    /// Why it failed.
    pub error: Option<String>,
    /// 183 R1: the unpair asked for a device-key reset that is not finished yet
    /// ([`Vault::finish_device_reset`]).
    #[serde(default)]
    pub device_reset_pending: bool,
    /// 183 R2: the removed machines' names, in `removed` order ("a removed machine" when none
    /// was known): what the UI shows, never an engine id.
    #[serde(default)]
    pub removed_names: Vec<String>,
    /// 183 R4: the sweep hold's end (ms; [`Vault::rotate`]), kept so a restart still holds.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub sweep_hold_until_ms: u64,
}

/// 183 §14.7 (§6.9.11): while one is held, no non-current key is retired. Drop releases it.
#[derive(Debug)]
pub struct RetireHold {
    count: Arc<AtomicUsize>,
    _who: &'static str,
}

impl Drop for RetireHold {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A local name: `[A-Za-z0-9_.-]{1,64}`. `/` is reserved for backends (37 §11).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// Prefix of the OAuth grant rows (67 §5.4): `oauth/<ns>`.
pub const OAUTH_PREFIX: &str = "oauth/";

/// Prefix of the mesh's own rows (69 decision 1): `mesh/<name>`, e.g.
/// `mesh/device_key`. Like `oauth/`, writable only through
/// [`Vault::set_if_version`], never listed, never expandable by placeholder.
pub const MESH_PREFIX: &str = "mesh/";

/// Prefix of a sandbox VM's rows (79 decision 4): `sandbox/<vm>/vnc`. Same
/// rules as `mesh/`; the segments after the prefix are each a `valid_name`.
pub const SANDBOX_PREFIX: &str = "sandbox/";

/// `valid_name`, or one of the `/` families a local caller may write
/// ([`Vault::set_if_version`]): `oauth/<ns>`, `mesh/<name>`, `sandbox/<vm>/<key>`.
fn writable_name(name: &str) -> bool {
    valid_name(name)
        || [OAUTH_PREFIX, MESH_PREFIX]
            .iter()
            .any(|p| name.strip_prefix(p).is_some_and(valid_name))
        || name
            .strip_prefix(SANDBOX_PREFIX)
            .is_some_and(|rest| rest.split('/').all(valid_name))
}

#[derive(Deserialize)]
struct Version {
    v: u32,
    value: Zeroizing<String>,
    created: String,
    source: String,
    /// `updated_ms` of the row (183 W-36: the Unpair preview's window).
    #[serde(default)]
    ms: u64,
}

#[derive(Deserialize)]
struct Entry {
    note: String,
    /// Newest first, never empty.
    versions: Vec<Version>,
}

#[derive(Deserialize)]
struct Plain {
    secrets: BTreeMap<String, Entry>,
}

/// Every kept value ≥ [`MIN_REDACT`] chars and its base64, each paired with
/// the placeholder that names it.
struct Scrubber {
    ac: AhoCorasick,
    placeholders: Vec<String>,
    /// Longest pattern, in bytes.
    max_len: usize,
}

struct Inner {
    entries: BTreeMap<String, Entry>,
    scrubber: Option<Scrubber>,
}

/// The encrypted, versioned store. `Send + Sync`; reads are sync from the
/// in-memory copy, writes are async into sqlite.
pub struct Vault {
    pool: SqlitePool,
    path: PathBuf,
    /// This engine's key: seals the mesh keys at rest.
    key: Zeroizing<[u8; 32]>,
    /// 183 §6.9: the mesh-wide data keys: the current one seals every new row, the retained
    /// ones still open rows not yet swept (W-31).
    ring: RwLock<rotation::Ring>,
    /// Key ids rows name that this engine does not hold (W-38): the mesh asks peers for them.
    missing: RwLock<std::collections::BTreeSet<String>>,
    /// Open [`RetireHold`]s.
    holds: Arc<AtomicUsize>,
    /// 183 R1: no sweep before this time (ms; 0 = none), set by `rotate`.
    sweep_until: std::sync::atomic::AtomicU64,
    /// 183 R1: when `retire` last trial-decrypted the `''` rows, and the retained key ids they opened.
    legacy_cache: std::sync::Mutex<Option<(u64, std::collections::BTreeSet<String>)>>,
    /// `meta["rotation"]`, watched by the plugin (`SecretsEvent::Rotation`).
    rotation: tokio::sync::watch::Sender<Option<RotationState>>,
    /// This engine's mesh id ([`Vault::set_engine`]); the initiator sweeps at once.
    me: RwLock<String>,
    /// Serializes keyring changes.
    keylock: tokio::sync::Mutex<()>,
    key_store: &'static str,
    inner: RwLock<Inner>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("key_store", &self.key_store)
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// Open or create `<data_dir>/secrets.sqlite`. Moves a pre-67
    /// `secrets.vault` into rows (renaming it `.migrated`), then imports and
    /// unlinks `<data_dir>/credentials.yaml` (flat `name: value` YAML) if
    /// present, source `import`. Err only when the key store or the file fails.
    pub async fn open(data_dir: &Path, key_store: KeyStore) -> anyhow::Result<Vault> {
        std::fs::create_dir_all(data_dir).with_context(|| data_dir.display().to_string())?;
        let (key, key_store) = crate::key::load_or_create(data_dir, key_store)?;
        let key = Zeroizing::new(key);
        let path = data_dir.join(store::DB);
        let pool = store::open(&path).await?;
        let mesh_key = match store::meta_get(&pool, MESH_KEY).await? {
            Some(sealed) => open_bytes(&key, MESH_KEY_AAD, &sealed)
                .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
                .map(Zeroizing::new)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}: the mesh key does not open with this engine's key",
                        path.display()
                    )
                })?,
            None => {
                let k = Zeroizing::new(rand::random::<[u8; 32]>());
                store::meta_put(&pool, MESH_KEY, &seal(&key, MESH_KEY_AAD, &*k)).await?;
                k
            }
        };
        let (ring, rotation) = rotation::load(&pool, &key, mesh_key).await?;
        let vault = Vault {
            pool,
            path,
            key,
            ring: RwLock::new(ring),
            missing: RwLock::default(),
            holds: Arc::default(),
            // 183 R4: a release (an ack) is not persisted; after a restart the hold runs out.
            sweep_until: rotation
                .as_ref()
                .map_or(0, |r| r.sweep_hold_until_ms)
                .into(),
            legacy_cache: Default::default(),
            rotation: tokio::sync::watch::Sender::new(rotation),
            me: RwLock::default(),
            keylock: tokio::sync::Mutex::new(()),
            key_store,
            inner: RwLock::new(Inner {
                entries: BTreeMap::new(),
                scrubber: None,
            }),
        };
        vault.refresh().await?;
        vault.migrate_legacy(&data_dir.join(LEGACY_FILE)).await?;
        vault.import(&data_dir.join("credentials.yaml")).await;
        Ok(vault)
    }

    /// `"keychain"` or `"file"`.
    pub fn key_store(&self) -> &'static str {
        self.key_store
    }

    /// The pool, for `replica::attach`.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The current mesh data key (travels inside the pairing string, 67 §5.4).
    pub fn mesh_key(&self) -> Zeroizing<[u8; 32]> {
        self.ring().current.key.clone()
    }

    /// Vault name of the mesh's Noise static key (69 decision 1).
    pub const MESH_DEVICE_KEY: &'static str = "mesh/device_key";

    /// The `mesh/device_key` row when it exists — no minting (69 decision 1: a hub *loads* the
    /// mesh identity; only a pairing creates it, so two engines that ran alone before pairing do
    /// not each mint one and collide). `Ok(None)` = no row yet.
    pub fn mesh_device_key_if_set(&self) -> anyhow::Result<Option<Zeroizing<[u8; 32]>>> {
        self.get(Self::MESH_DEVICE_KEY, 0)
            .map(|v| Self::decode_mesh_key(&v))
            .transpose()
    }

    fn decode_mesh_key(v: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(v)
                .context("mesh/device_key: not base64url")?,
        );
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .context("mesh/device_key: expected 32 bytes")?;
        Ok(Zeroizing::new(key))
    }

    /// The mesh's own X25519 static private key (69 decision 1), 32 bytes:
    /// the row `mesh/device_key` (value = base64url, no padding), created by
    /// the first caller with `set_if_version(expected = None)`. A race loser
    /// (`Ok(None)`) re-reads and uses the winner's key, so every engine of the
    /// mesh derives the same device id.
    pub async fn mesh_device_key(&self) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        let decode = Self::decode_mesh_key;
        if let Some(k) = self.mesh_device_key_if_set()? {
            return Ok(k);
        }
        let fresh: Zeroizing<[u8; 32]> = Zeroizing::new(rand::random());
        let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(*fresh));
        match self
            .set_if_version(Self::MESH_DEVICE_KEY, &encoded, "link", None)
            .await
        {
            Ok(Some(_)) => Ok(fresh),
            Ok(None) => {
                // Lost the race: another engine (or task) wrote first; its
                // replicated row is what `refresh` just loaded.
                self.refresh().await?;
                let v = self
                    .get(Self::MESH_DEVICE_KEY, 0)
                    .ok_or_else(|| anyhow::anyhow!("mesh/device_key: lost the race but no row"))?;
                decode(&v)
            }
            Err(e) => Err(anyhow::anyhow!("mesh/device_key: {e}")),
        }
    }

    /// `nonce ‖ XChaCha20-Poly1305(mesh key, aad)` — for callers that keep a
    /// value in their own replicated table but want it sealed like a vault
    /// row (69: link's `pending.psk_sealed`).
    pub fn seal_with_mesh_key(&self, aad: &[u8], plain: &[u8]) -> Vec<u8> {
        seal(&self.mesh_key(), aad, plain)
    }

    /// Inverse of [`Vault::seal_with_mesh_key`]: the current key, then the retained ones
    /// (183 §6.9.11); `None` = no held key, wrong aad or damaged bytes.
    pub fn open_with_mesh_key(&self, aad: &[u8], bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        self.ring().open("", aad, bytes)
    }

    /// 183 §6.9.11: `bytes` re-sealed under the current key when they open only under a
    /// retained one; `None` when they already open under the current key, or under none.
    pub fn reseal_with_mesh_key(&self, aad: &[u8], bytes: &[u8]) -> Option<Vec<u8>> {
        let ring = self.ring();
        if open_bytes(&ring.current.key, aad, bytes).is_some() {
            return None;
        }
        let plain = ring.open("", aad, bytes)?;
        Some(seal(&ring.current.key, aad, &plain))
    }

    /// Newest mtime of the sqlite file or its WAL: moves on every write by
    /// anyone (a peer's rows applied here, another process). The plugin polls
    /// it to know when to [`Vault::refresh`].
    pub fn disk_stamp(&self) -> Option<SystemTime> {
        let mut wal = self.path.as_os_str().to_owned();
        wal.push("-wal");
        mtime(&self.path)
            .into_iter()
            .chain(mtime(Path::new(&wal)))
            .max()
    }

    /// Rebuild the in-memory copy from the tables: per name the current
    /// version and the [`KEEP`] − 1 below it. Each row opens with the key its `key_id`
    /// names (`''`: by trial, 183 §6.9.4). A row under a key this engine does not hold keeps
    /// its previous in-memory value (W-38) and its key id goes to [`Vault::missing_keys`];
    /// any other row that does not open is skipped with a warning.
    pub async fn refresh(&self) -> anyhow::Result<()> {
        let (rows, current) = store::rows(&self.pool).await?;
        let ring = self.ring();
        let mut missing = std::collections::BTreeSet::new();
        let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
        let prev = self.read();
        for r in rows {
            let Some(&cur) = current.get(&r.name) else {
                continue;
            };
            if r.version > cur {
                continue;
            }
            let entry = entries.entry(r.name.clone()).or_insert_with(|| Entry {
                note: r.note.clone(),
                versions: Vec::new(),
            });
            if entry.versions.len() >= KEEP {
                continue;
            }
            let id = store::id(&r.name, r.version);
            let value = match ring.open(&r.key_id, id.as_bytes(), &r.ciphertext) {
                Some(plain) => match String::from_utf8(plain.to_vec()).map(Zeroizing::new) {
                    Ok(v) => v,
                    Err(_) => {
                        tracing::warn!("secrets: {id} is not UTF-8; skipped");
                        continue;
                    }
                },
                None if !r.key_id.is_empty() && ring.get(&r.key_id).is_none() => {
                    missing.insert(r.key_id.clone());
                    let kept = prev
                        .entries
                        .get(&r.name)
                        .and_then(|e| e.versions.iter().find(|v| v.v == r.version));
                    match kept {
                        Some(v) => v.value.clone(),
                        None => {
                            tracing::warn!(
                                "secrets: {id}: key {} not held; waiting for the vault key from the mesh",
                                r.key_id
                            );
                            continue;
                        }
                    }
                }
                None => {
                    tracing::warn!("secrets: {id} does not open under any held mesh key; skipped");
                    continue;
                }
            };
            if r.version == cur {
                entry.note = r.note;
            }
            entry.versions.push(Version {
                v: r.version,
                value,
                created: rfc3339(r.updated_ms),
                source: r.source,
                ms: r.updated_ms,
            });
        }
        drop(prev);
        drop(ring);
        *self.missing.write().unwrap_or_else(PoisonError::into_inner) = missing;
        entries.retain(|_, e| !e.versions.is_empty());
        let mut g = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        g.scrubber = scrubber(&entries);
        g.entries = entries;
        Ok(())
    }

    /// `back` = 0 current, 1 previous, …; `None` when the name or version
    /// does not exist.
    pub fn get(&self, name: &str, back: u8) -> Option<Zeroizing<String>> {
        self.read()
            .entries
            .get(name)?
            .versions
            .get(usize::from(back))
            .map(|v| v.value.clone())
    }

    /// The current version number and value of `name`, for a
    /// [`Vault::set_if_version`] precondition.
    pub fn get_versioned(&self, name: &str) -> Option<(u32, Zeroizing<String>)> {
        let g = self.read();
        let cur = g.entries.get(name)?.versions.first()?;
        Some((cur.v, cur.value.clone()))
    }

    /// Every user-facing secret, sorted by name. Never a value; never a
    /// backend row (`oauth/*`, `mesh/*` — anything with a `/`).
    pub fn list(&self) -> Vec<SecretInfo> {
        self.read()
            .entries
            .iter()
            .filter(|(name, _)| !name.contains('/'))
            .map(|(name, e)| info(name, e))
            .collect()
    }

    /// One name's info, backend rows included.
    fn info_of(&self, name: &str) -> Option<SecretInfo> {
        self.read().entries.get(name).map(|e| info(name, e))
    }

    /// Push a new version (`v` monotonic per name, ≤ [`KEEP`] kept). The same
    /// value as the current version returns the current info and writes
    /// nothing. `note: None` keeps the old note (`""` for a new name).
    pub async fn set(
        &self,
        name: &str,
        value: &str,
        note: Option<&str>,
        source: &str,
    ) -> Result<SecretInfo, String> {
        if !valid_name(name) {
            return Err(format!(
                "invalid secret name {name:?}; names are [A-Za-z0-9_.-]{{1,64}}"
            ));
        }
        self.write(name, value, note, source, None)
            .await?
            .ok_or_else(|| format!("{name}: concurrent write; retry"))
    }

    /// [`Vault::set`] only if the current version is `expected` (`None` = the
    /// name does not exist yet): `Ok(None)` when it is not — the caller
    /// re-reads (67 §4.4, the OAuth refresher). Accepts `oauth/<ns>` names.
    pub async fn set_if_version(
        &self,
        name: &str,
        value: &str,
        source: &str,
        expected: Option<u32>,
    ) -> Result<Option<SecretInfo>, String> {
        if !writable_name(name) {
            return Err(format!("invalid secret name {name:?}"));
        }
        self.write(name, value, None, source, Some(expected)).await
    }

    /// Copy version `back` (1..=4) to the top as a new version, source `restore vN`.
    pub async fn restore(&self, name: &str, back: u8) -> Result<SecretInfo, String> {
        if !(1..=4).contains(&back) {
            return Err(format!("back: {back} is not in 1..=4"));
        }
        let (value, source) = {
            let g = self.read();
            let entry = g.entries.get(name).ok_or_else(|| unknown(name))?;
            let old = entry.versions.get(usize::from(back)).ok_or_else(|| {
                format!(
                    "{name} has {} versions; back: {back} does not exist",
                    entry.versions.len()
                )
            })?;
            (old.value.clone(), format!("restore v{}", old.v))
        };
        // A restore always writes: the same-value short cut is skipped by design.
        self.write_row(name, &value, None, &source, None, true)
            .await?
            .ok_or_else(|| format!("{name}: concurrent write; retry"))
    }

    /// Remove the name and every version.
    pub async fn delete(&self, name: &str) -> Result<(), String> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|e| e.to_string())?;
        let gone = sqlx::query("DELETE FROM vault_current WHERE name = ?")
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
        if gone == 0 {
            return Err(unknown(name));
        }
        sqlx::query("DELETE FROM vault WHERE name = ?")
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        self.refresh().await.map_err(|e| format!("{e:#}"))
    }

    /// Replace every `{{secret:NAME}}` / `{{secret:NAME:-N}}` in every string
    /// leaf of `args` (keys untouched); returns the count. Err on the first
    /// unknown name or version — `args` may then be partially expanded, so
    /// the caller must not dispatch it.
    pub fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(PLACEHOLDER).expect("PLACEHOLDER is a valid regex"));
        let g = self.read();
        let mut n = 0;
        strings(args, &mut |s| {
            if !s.contains("{{secret:") {
                return Ok(());
            }
            let mut err = None;
            let out = RE.replace_all(s, |c: &regex::Captures| -> String {
                let back = c.get(2).map_or(0, |m| m.as_str().parse().unwrap_or(0));
                match lookup(&g.entries, &c[1], back) {
                    Ok(value) => {
                        n += 1;
                        value.to_owned()
                    }
                    Err(e) => {
                        err.get_or_insert(e);
                        String::new()
                    }
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
            if let Cow::Owned(o) = out {
                *s = o;
            }
            Ok(())
        })?;
        Ok(n)
    }

    /// Every kept version of every value ≥ [`MIN_REDACT`] chars, and its
    /// standard base64, → `{{secret:NAME}}` (current) or `{{secret:NAME:-N}}`.
    /// One leftmost-longest pass; borrowed when nothing matches.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        scrub(self.read().scrubber.as_ref(), text)
    }

    /// [`Vault::redact`] over every string in a JSON value.
    pub fn redact_value(&self, v: &mut serde_json::Value) {
        let g = self.read();
        let Some(sc) = g.scrubber.as_ref() else {
            return;
        };
        let _ = strings(v, &mut |s| {
            if let Cow::Owned(o) = scrub(Some(sc), s) {
                *s = o;
            }
            Ok(())
        });
    }

    /// Byte ranges [`Vault::redact`] would replace, ascending and
    /// non-overlapping (51 §7.2).
    pub fn redact_spans(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        self.read().scrubber.as_ref().map_or_else(Vec::new, |sc| {
            sc.ac.find_iter(text).map(|m| m.start()..m.end()).collect()
        })
    }

    /// Longest value [`Vault::redact_spans`] can match, in bytes; `0` = none.
    pub fn max_literal_len(&self) -> usize {
        self.read().scrubber.as_ref().map_or(0, |sc| sc.max_len)
    }

    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// [`Vault::write_row`] with the same-value short cut.
    async fn write(
        &self,
        name: &str,
        value: &str,
        note: Option<&str>,
        source: &str,
        expected: Option<Option<u32>>,
    ) -> Result<Option<SecretInfo>, String> {
        self.write_row(name, value, note, source, expected, false)
            .await
    }

    /// One transaction: read the pointer, check `expected` (`Some(x)` = the
    /// current version must be `x`, `None` = any), insert the row, move the
    /// pointer with the old version as the `WHERE`, prune below [`KEEP`].
    /// `Ok(None)` = precondition failed or the pointer moved underneath.
    ///
    /// ponytail: two engines writing the same name in one sync gap mint the
    /// same `(name, version)` id; per-column LWW then picks one ciphertext and
    /// the `vault_current` pointer picks one winner. Rare, and a re-set fixes it.
    async fn write_row(
        &self,
        name: &str,
        value: &str,
        note: Option<&str>,
        source: &str,
        expected: Option<Option<u32>>,
        force: bool,
    ) -> Result<Option<SecretInfo>, String> {
        let e = |e: sqlx::Error| e.to_string();
        let cached = {
            let g = self.read();
            g.entries.get(name).map(|e| {
                (
                    e.versions.first().map(|v| v.v),
                    e.versions
                        .first()
                        .is_some_and(|cur| cur.value.as_str() == value),
                    e.note.clone(),
                )
            })
        };
        if !force
            && let Some((cur, true, _)) = &cached
            && expected.is_none_or(|x| x == *cur)
        {
            return Ok(self.info_of(name));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(e)?;
        let cur: Option<u32> =
            sqlx::query_scalar::<_, i64>("SELECT version FROM vault_current WHERE name = ?")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await
                .map_err(e)?
                .map(|v| v as u32);
        if let Some(exp) = expected
            && exp != cur
        {
            return Ok(None);
        }
        let note = match note {
            Some(n) => n.to_owned(),
            None => cached.map(|c| c.2).unwrap_or_default(),
        };
        let version = cur.map_or(1, |c| c + 1);
        let id = store::id(name, version);
        let now = now_ms();
        let (key, key_id) = self.current();
        sqlx::query(
            "INSERT INTO vault (id, name, version, ciphertext, note, source, updated_ms, key_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(i64::from(version))
        .bind(seal(&key, id.as_bytes(), value.as_bytes()))
        .bind(&note)
        .bind(source)
        .bind(now as i64)
        .bind(&key_id)
        .execute(&mut *tx)
        .await
        .map_err(e)?;
        let moved = match cur {
            Some(c) => {
                sqlx::query("UPDATE vault_current SET version = ? WHERE name = ? AND version = ?")
                    .bind(i64::from(version))
                    .bind(name)
                    .bind(i64::from(c))
                    .execute(&mut *tx)
                    .await
                    .map_err(e)?
                    .rows_affected()
            }
            None => {
                sqlx::query("INSERT OR IGNORE INTO vault_current (name, version) VALUES (?, ?)")
                    .bind(name)
                    .bind(i64::from(version))
                    .execute(&mut *tx)
                    .await
                    .map_err(e)?
                    .rows_affected()
            }
        };
        if moved != 1 {
            return Ok(None);
        }
        sqlx::query("DELETE FROM vault WHERE name = ? AND version <= ?")
            .bind(name)
            .bind(i64::from(version) - KEEP as i64)
            .execute(&mut *tx)
            .await
            .map_err(e)?;
        tx.commit().await.map_err(e)?;
        self.refresh().await.map_err(|e| format!("{e:#}"))?;
        Ok(self.info_of(name))
    }

    /// First start after 67: the single-file vault becomes rows (names the
    /// table already has are left alone), then the file is renamed
    /// `secrets.vault.migrated`.
    async fn migrate_legacy(&self, path: &Path) -> anyhow::Result<()> {
        let Some(entries) = load_legacy(path, &self.key)? else {
            return Ok(());
        };
        let (mesh_key, key_id) = self.current();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut n = 0;
        for (name, e) in &entries {
            if self.read().entries.contains_key(name) {
                continue;
            }
            let Some(cur) = e.versions.first() else {
                continue;
            };
            for v in &e.versions {
                let id = store::id(name, v.v);
                let ms = chrono::DateTime::parse_from_rfc3339(&v.created)
                    .map_or_else(|_| now_ms(), |t| t.timestamp_millis() as u64);
                sqlx::query(
                    "INSERT OR IGNORE INTO vault (id, name, version, ciphertext, note, source, updated_ms, key_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(name)
                .bind(i64::from(v.v))
                .bind(seal(&mesh_key, id.as_bytes(), v.value.as_bytes()))
                .bind(&e.note)
                .bind(&v.source)
                .bind(ms as i64)
                .bind(&key_id)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("INSERT OR IGNORE INTO vault_current (name, version) VALUES (?, ?)")
                .bind(name)
                .bind(i64::from(cur.v))
                .execute(&mut *tx)
                .await?;
            n += 1;
        }
        tx.commit().await?;
        let mut moved = path.as_os_str().to_owned();
        moved.push(".migrated");
        std::fs::rename(path, &moved).with_context(|| format!("rename {}", path.display()))?;
        tracing::info!(
            "migrated {n} secrets from {} into secrets.sqlite",
            path.display()
        );
        self.refresh().await
    }

    /// 37 §3.6: move `credentials.yaml` into the vault and unlink it. A parse
    /// or write failure leaves the file where it is.
    async fn import(&self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path).map(Zeroizing::new) else {
            return;
        };
        let rows = match serde_yaml::from_str::<Option<HashMap<String, String>>>(&text) {
            Ok(rows) => rows.unwrap_or_default(),
            Err(e) => {
                tracing::warn!("{}: {e}; not imported", path.display());
                return;
            }
        };
        let (mut n, mut failed) = (0, false);
        for (name, value) in rows.iter().filter(|(_, v)| !v.trim().is_empty()) {
            match self
                .set(
                    name,
                    value.trim(),
                    Some("imported from credentials.yaml"),
                    "import",
                )
                .await
            {
                Ok(_) => n += 1,
                Err(e) => {
                    failed = true;
                    tracing::warn!("credentials.yaml: {name}: {e}");
                }
            }
        }
        if failed {
            tracing::warn!(
                "{}: some keys were not imported; the file was kept",
                path.display()
            );
            return;
        }
        match std::fs::remove_file(path) {
            Ok(()) => tracing::info!(
                "imported {n} keys from credentials.yaml into the vault; the file was removed"
            ),
            Err(e) => tracing::warn!(
                "imported {n} keys from credentials.yaml but could not remove it: {e}"
            ),
        }
    }
}

fn unknown(name: &str) -> String {
    format!("unknown secret {name}; secret_list shows the names")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn rfc3339(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `nonce ‖ XChaCha20-Poly1305(key, aad)`.
fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let nonce: [u8; NONCE] = rand::random();
    let sealed = XChaCha20Poly1305::new(key.into())
        .encrypt(&nonce.into(), Payload { msg: plain, aad })
        .expect("XChaCha20-Poly1305 encrypt cannot fail on in-memory data");
    let mut out = Vec::with_capacity(NONCE + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// Inverse of [`seal`]; `None` = wrong key, wrong aad or damaged bytes.
fn open_bytes(key: &[u8; 32], aad: &[u8], bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if bytes.len() <= NONCE {
        return None;
    }
    let (nonce, sealed) = bytes.split_at(NONCE);
    let nonce: [u8; NONCE] = nonce.try_into().ok()?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(&nonce.into(), Payload { msg: sealed, aad })
        .ok()
        .map(Zeroizing::new)
}

/// The value `back` versions behind the current, with the placeholder-style error texts.
fn lookup<'a>(
    entries: &'a BTreeMap<String, Entry>,
    name: &str,
    back: usize,
) -> Result<&'a str, String> {
    let e = entries.get(name).ok_or_else(|| unknown(name))?;
    e.versions
        .get(back)
        .map(|v| v.value.as_str())
        .ok_or_else(|| {
            format!(
                "{name} has {} versions; :-{back} does not exist",
                e.versions.len()
            )
        })
}

fn info(name: &str, e: &Entry) -> SecretInfo {
    let cur = e.versions.first();
    SecretInfo {
        name: name.to_owned(),
        note: e.note.clone(),
        source: "local".to_owned(),
        current: cur.map_or(0, |c| c.v),
        versions: e
            .versions
            .iter()
            .map(|v| VersionInfo {
                v: v.v,
                created: v.created.clone(),
                source: v.source.clone(),
            })
            .collect(),
        updated: cur.map(|c| c.created.clone()).unwrap_or_default(),
    }
}

/// Visit every string leaf (object keys excluded); stops at the first Err.
fn strings(
    v: &mut serde_json::Value,
    f: &mut impl FnMut(&mut String) -> Result<(), String>,
) -> Result<(), String> {
    match v {
        serde_json::Value::String(s) => f(s),
        serde_json::Value::Array(a) => a.iter_mut().try_for_each(|v| strings(v, f)),
        serde_json::Value::Object(o) => o.values_mut().try_for_each(|v| strings(v, f)),
        _ => Ok(()),
    }
}

fn scrub<'a>(sc: Option<&Scrubber>, text: &'a str) -> Cow<'a, str> {
    match sc {
        Some(sc) if sc.ac.find(text).is_some() => {
            Cow::Owned(sc.ac.replace_all(text, &sc.placeholders))
        }
        _ => Cow::Borrowed(text),
    }
}

/// Newest version first per name and each text once, so a value two
/// versions share (a restore) reports the current placeholder.
fn scrubber(entries: &BTreeMap<String, Entry>) -> Option<Scrubber> {
    let mut patterns: Vec<Zeroizing<String>> = Vec::new();
    let mut placeholders = Vec::new();
    for (name, e) in entries {
        for (i, ver) in e.versions.iter().enumerate() {
            if ver.value.chars().count() < MIN_REDACT {
                continue;
            }
            let ph = if i == 0 {
                format!("{{{{secret:{name}}}}}")
            } else {
                format!("{{{{secret:{name}:-{i}}}}}")
            };
            for text in [
                ver.value.clone(),
                Zeroizing::new(B64.encode(ver.value.as_bytes())),
            ] {
                if !patterns.contains(&text) {
                    patterns.push(text);
                    placeholders.push(ph.clone());
                }
            }
        }
    }
    if patterns.is_empty() {
        return None;
    }
    let max_len = patterns.iter().map(|p| p.len()).max().unwrap_or(0);
    let ac = AhoCorasickBuilder::new()
        .match_kind(MatchKind::LeftmostLongest)
        .build(&patterns);
    match ac {
        Ok(ac) => Some(Scrubber {
            ac,
            placeholders,
            max_len,
        }),
        Err(e) => {
            tracing::warn!("secrets: building the redaction automaton failed: {e}");
            None
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The pre-67 `secrets.vault` file. `None` = no file.
fn load_legacy(path: &Path, key: &[u8; 32]) -> anyhow::Result<Option<BTreeMap<String, Entry>>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let Some(rest) = bytes.strip_prefix(MAGIC).filter(|r| r.len() > NONCE) else {
        anyhow::bail!("{} is not a theuth secrets vault", path.display());
    };
    let plain = open_bytes(key, MAGIC, rest).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot decrypt {}: wrong key or corrupt file",
            path.display()
        )
    })?;
    let plain: Plain = serde_json::from_slice(&plain)
        .with_context(|| format!("{}: bad vault contents", path.display()))?;
    Ok(Some(plain.secrets))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open(dir: &Path) -> Vault {
        Vault::open(dir, KeyStore::File).await.expect("open")
    }

    #[tokio::test]
    async fn round_trip_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("gh", "ghp_0123456789", Some("GitHub PAT"), "user")
            .await
            .unwrap();
        drop(v);
        let v = open(dir.path()).await;
        assert_eq!(v.get("gh", 0).unwrap().as_str(), "ghp_0123456789");
        assert_eq!(v.list()[0].note, "GitHub PAT");
        assert_eq!(v.key_store(), "file");

        // Rows are sealed under the mesh key: another key opens nothing.
        let (rows, _) = store::rows(v.pool()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(open_bytes(&[7; 32], b"gh#1", &rows[0].ciphertext).is_none());
        assert!(open_bytes(&v.mesh_key(), b"gh#2", &rows[0].ciphertext).is_none());
        assert_eq!(
            open_bytes(&v.mesh_key(), b"gh#1", &rows[0].ciphertext)
                .unwrap()
                .as_slice(),
            b"ghp_0123456789"
        );
    }

    #[tokio::test]
    async fn versions_keep_five_and_v_is_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        for i in 1..=10 {
            let info = v
                .set("k", &format!("value-number-{i}"), None, "shell")
                .await
                .unwrap();
            assert_eq!(info.current, i);
        }
        let info = &v.list()[0];
        assert_eq!(info.versions.len(), KEEP);
        assert_eq!(
            info.versions.iter().map(|x| x.v).collect::<Vec<_>>(),
            [10, 9, 8, 7, 6]
        );
        assert_eq!(v.get("k", 4).unwrap().as_str(), "value-number-6");
        assert!(v.get("k", 5).is_none());
        assert!(v.get("nope", 0).is_none());
        // Pruned rows are gone from the table too.
        let (rows, _) = store::rows(v.pool()).await.unwrap();
        assert_eq!(rows.len(), KEEP);

        // Same value: no new version.
        let same = v.set("k", "value-number-10", None, "shell").await.unwrap();
        assert_eq!(same.current, 10);
        assert_eq!(v.list()[0].versions.len(), KEEP);

        // Restore copies, keeps history, never reuses a number.
        let r = v.restore("k", 2).await.unwrap();
        assert_eq!(r.current, 11);
        assert_eq!(r.versions[0].source, "restore v8");
        assert_eq!(v.get("k", 0).unwrap().as_str(), "value-number-8");
        assert_eq!(v.get("k", 1).unwrap().as_str(), "value-number-10");
        assert_eq!(
            v.restore("k", 5).await.unwrap_err(),
            "back: 5 is not in 1..=4"
        );
        v.delete("k").await.unwrap();
        assert_eq!(
            v.restore("k", 1).await.unwrap_err(),
            "unknown secret k; secret_list shows the names"
        );
        v.set("two", "a", None, "u").await.unwrap();
        v.set("two", "b", None, "u").await.unwrap();
        assert_eq!(
            v.restore("two", 3).await.unwrap_err(),
            "two has 2 versions; back: 3 does not exist"
        );
        assert!(v.delete("two").await.is_ok());
        assert!(v.delete("two").await.is_err());
        assert!(v.list().is_empty());
    }

    #[tokio::test]
    async fn set_if_version_is_a_precondition() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        assert!(
            v.set_if_version("oauth/gh", "{\"a\":1}", "login", Some(3))
                .await
                .unwrap()
                .is_none()
        );
        let i = v
            .set_if_version("oauth/gh", "{\"a\":1}", "login", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(i.current, 1);
        assert!(
            v.set_if_version("oauth/gh", "{\"a\":2}", "refresh", None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            v.set_if_version("oauth/gh", "{\"a\":2}", "refresh", Some(2))
                .await
                .unwrap()
                .is_none()
        );
        let i = v
            .set_if_version("oauth/gh", "{\"a\":2}", "refresh", Some(1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(i.current, 2);
        assert_eq!(v.get_versioned("oauth/gh").unwrap().0, 2);
        assert!(
            v.set("oauth/gh", "x", None, "u").await.is_err(),
            "`/` names only via set_if_version"
        );
        assert!(v.set_if_version("a/b", "x", "u", None).await.is_err());
    }

    #[tokio::test]
    async fn expand_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("tok", "old-token-value", None, "user").await.unwrap();
        v.set("tok", "new-token-value", None, "user").await.unwrap();
        let mut args = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer {{secret:tok}}' -H 'X-Old: {{secret:tok:-1}}'",
            "nested": {"list": ["{{secret:tok}}", 7, null], "{{secret:tok}}": "key untouched"},
            "plain": "nothing here"
        });
        assert_eq!(v.expand(&mut args).unwrap(), 3);
        assert_eq!(
            args["command"],
            "curl -H 'Authorization: Bearer new-token-value' -H 'X-Old: old-token-value'"
        );
        assert_eq!(args["nested"]["list"][0], "new-token-value");
        assert!(args["nested"].get("{{secret:tok}}").is_some());

        let mut bad = serde_json::json!({"a": "{{secret:nope}}"});
        assert_eq!(
            v.expand(&mut bad).unwrap_err(),
            "unknown secret nope; secret_list shows the names"
        );
        let mut bad = serde_json::json!({"a": "{{secret:tok:-3}}"});
        assert_eq!(
            v.expand(&mut bad).unwrap_err(),
            "tok has 2 versions; :-3 does not exist"
        );
        let mut none = serde_json::json!({"a": "{{secret:}}", "b": 1});
        assert_eq!(v.expand(&mut none).unwrap(), 0);
    }

    #[tokio::test]
    async fn redact_all_versions_and_base64() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("tok", "first-secret-value", None, "user")
            .await
            .unwrap();
        v.set("tok", "second-secret-value", None, "user")
            .await
            .unwrap();
        v.set("pin", "1234", None, "user").await.unwrap();
        let text = format!(
            "cur=second-secret-value old=first-secret-value b64={} pin=1234",
            B64.encode("second-secret-value")
        );
        assert_eq!(
            v.redact(&text),
            "cur={{secret:tok}} old={{secret:tok:-1}} b64={{secret:tok}} pin=1234"
        );
        assert!(matches!(v.redact("nothing to see"), Cow::Borrowed(_)));

        let mut val = serde_json::json!({"out": ["second-secret-value"], "n": 1});
        v.redact_value(&mut val);
        assert_eq!(val["out"][0], "{{secret:tok}}");

        // A restored value lives in two versions; the current placeholder wins.
        v.restore("tok", 1).await.unwrap();
        assert_eq!(v.redact("first-secret-value"), "{{secret:tok}}");
    }

    #[tokio::test]
    async fn imports_credentials_yaml_once() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = dir.path().join("credentials.yaml");
        std::fs::write(
            &yaml,
            "# keys\nanthropic: sk-ant-abc\nopenai: \"\"\nbrave: brv-1\n",
        )
        .unwrap();
        let v = open(dir.path()).await;
        assert!(!yaml.exists());
        let list = v.list();
        assert_eq!(
            list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["anthropic", "brave"]
        );
        assert_eq!(list[0].versions[0].source, "import");
        assert_eq!(list[0].note, "imported from credentials.yaml");
        assert_eq!(v.get("anthropic", 0).unwrap().as_str(), "sk-ant-abc");
        drop(v);
        let v = open(dir.path()).await;
        assert_eq!(v.list()[0].versions.len(), 1);

        // Unparsable: left alone, nothing imported.
        std::fs::write(&yaml, "not: [valid\n").unwrap();
        let v2 = open(dir.path()).await;
        assert!(yaml.exists());
        assert_eq!(v2.list().len(), 2);
    }

    #[tokio::test]
    async fn legacy_file_becomes_rows_once() {
        let dir = tempfile::tempdir().unwrap();
        let (key, _) = crate::key::load_or_create(dir.path(), KeyStore::File).unwrap();
        let plain = serde_json::json!({"secrets": {"gh": {"note": "old", "versions": [
            {"v": 3, "value": "third-value-long", "created": "2025-01-03T00:00:00Z", "source": "user"},
            {"v": 2, "value": "second-value-long", "created": "2025-01-02T00:00:00Z", "source": "cli"}
        ]}}});
        let mut bytes = MAGIC.to_vec();
        bytes.extend(seal(&key, MAGIC, &serde_json::to_vec(&plain).unwrap()));
        let legacy = dir.path().join(LEGACY_FILE);
        std::fs::write(&legacy, bytes).unwrap();

        let v = open(dir.path()).await;
        assert!(!legacy.exists());
        assert!(dir.path().join("secrets.vault.migrated").exists());
        let info = &v.list()[0];
        assert_eq!((info.current, info.note.as_str()), (3, "old"));
        assert_eq!(
            info.versions.iter().map(|x| x.v).collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(info.versions[0].created, "2025-01-03T00:00:00Z");
        assert_eq!(v.get("gh", 1).unwrap().as_str(), "second-value-long");
        assert_eq!(v.redact("x third-value-long y"), "x {{secret:gh}} y");
        let next = v
            .set("gh", "fourth-value-long", None, "user")
            .await
            .unwrap();
        assert_eq!(next.current, 4);
    }

    #[test]
    fn names() {
        assert!(valid_name("GITHUB_TOKEN"));
        assert!(valid_name("a.b-c"));
        assert!(!valid_name(""));
        assert!(!valid_name("bitwarden/github"));
        assert!(!valid_name(&"x".repeat(65)));
        assert!(valid_name(&"x".repeat(64)));
        assert!(writable_name("oauth/github"));
        assert!(!writable_name("oauth/"));
        assert!(!writable_name("oauth/a/b"));
        assert!(writable_name("mesh/device_key"));
        assert!(!writable_name("mesh/"));
        assert!(!writable_name("mesh/a/b"));
        assert!(writable_name("sandbox/lab/vnc"));
        assert!(!writable_name("sandbox/lab/"));
    }

    /// 69 decision 1: `mesh/*` rows are written only through `set_if_version`,
    /// hidden from every list (as `oauth/*` now is too) and no placeholder can
    /// name them.
    #[tokio::test]
    async fn backend_rows_are_hidden_and_unexpandable() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("tok", "user-token-value", None, "user")
            .await
            .unwrap();
        assert!(
            v.set("mesh/device_key", "x", None, "u").await.is_err(),
            "`/` names only via set_if_version"
        );
        let i = v
            .set_if_version("mesh/device_key", "device-key-bytes-b64", "link", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((i.name.as_str(), i.current), ("mesh/device_key", 1));
        v.set_if_version("oauth/gh", "{}", "oauth", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            v.get("mesh/device_key", 0).unwrap().as_str(),
            "device-key-bytes-b64"
        );
        assert_eq!(
            v.list().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["tok"]
        );

        let mut args =
            serde_json::json!({"a": "{{secret:mesh/device_key}}", "b": "{{secret:oauth/gh}}"});
        assert_eq!(
            v.expand(&mut args).unwrap(),
            0,
            "the placeholder grammar has no `/`"
        );
        assert_eq!(args["a"], "{{secret:mesh/device_key}}");
    }

    /// 69 decision 1: two callers racing for the first `mesh/device_key`
    /// end with the same key; a reopen reads the row back; the sealing
    /// helpers round-trip under the mesh key and refuse a wrong aad.
    #[tokio::test]
    async fn mesh_device_key_is_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        assert!(
            v.mesh_device_key_if_set().unwrap().is_none(),
            "a hub loads, never mints"
        );
        assert!(v.info_of(Vault::MESH_DEVICE_KEY).is_none());
        let (a, b) = tokio::join!(v.mesh_device_key(), v.mesh_device_key());
        assert_eq!(
            *v.mesh_device_key_if_set().unwrap().unwrap(),
            **a.as_ref().unwrap()
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(*a, *b, "the race loser adopts the winner's key");
        assert_eq!(
            v.info_of(Vault::MESH_DEVICE_KEY).unwrap().current,
            1,
            "exactly one version written"
        );
        assert!(v.list().is_empty(), "never listed");
        drop(v);
        let v = open(dir.path()).await;
        assert_eq!(*v.mesh_device_key().await.unwrap(), *a);

        let sealed = v.seal_with_mesh_key(b"pending#1", b"psk-bytes");
        assert_eq!(
            v.open_with_mesh_key(b"pending#1", &sealed)
                .unwrap()
                .as_slice(),
            b"psk-bytes"
        );
        assert!(v.open_with_mesh_key(b"pending#2", &sealed).is_none());
    }

    #[tokio::test]
    async fn second_instance_sees_writes_after_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let a = open(dir.path()).await;
        a.set("seed", "seed-value", None, "u").await.unwrap();
        let b = open(dir.path()).await;
        assert!(b.get("fresh", 0).is_none());
        let before = b.disk_stamp();
        a.set("fresh", "fresh-value-123", None, "u").await.unwrap();
        assert!(b.disk_stamp() >= before);
        assert!(b.get("fresh", 0).is_none(), "reads are from the copy");
        b.refresh().await.unwrap();
        assert_eq!(b.get("fresh", 0).unwrap().as_str(), "fresh-value-123");
        assert_eq!(b.redact("x fresh-value-123 y"), "x {{secret:fresh}} y");
    }

    /// Ship everything pending on `from` to `to` (the pattern of `replica::tests::pump`).
    async fn pump(from: &replica::engine::ReplicaHandle, to: &replica::engine::Engine) {
        while let Some(batch) = from.flush().await.unwrap() {
            to.apply(&batch).await.unwrap();
        }
    }

    fn list_names(v: &Vault) -> Vec<String> {
        v.list().into_iter().map(|s| s.name).collect()
    }

    #[tokio::test]
    async fn two_replicas_converge() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path()).await;
        let b = open(db.path()).await;
        b.adopt_keyring(a.keyring()).await.unwrap();
        let ea = replica::engine::Engine::open(&da.path().join("replica.sqlite"))
            .await
            .unwrap();
        let eb = replica::engine::Engine::open(&db.path().join("replica.sqlite"))
            .await
            .unwrap();
        let ha = ea.attach(a.pool(), store::TABLES).await.unwrap();
        let hb = eb.attach(b.pool(), store::TABLES).await.unwrap();

        // Set on A, resolve on B.
        a.set("gh", "first-value-long", Some("GitHub"), "user")
            .await
            .unwrap();
        pump(&ha, &eb).await;
        b.refresh().await.unwrap();
        assert_eq!(b.get("gh", 0).unwrap().as_str(), "first-value-long");
        assert_eq!(b.list()[0].note, "GitHub");
        let mut args = serde_json::json!({"h": "Bearer {{secret:gh}}"});
        b.expand(&mut args).unwrap();
        assert_eq!(args["h"], "Bearer first-value-long");

        // A restore replicates as a new version with its source.
        a.set("gh", "second-value-long", None, "user")
            .await
            .unwrap();
        a.restore("gh", 1).await.unwrap();
        pump(&ha, &eb).await;
        b.refresh().await.unwrap();
        let info = &b.list()[0];
        assert_eq!(info.current, 3);
        assert_eq!(info.versions[0].source, "restore v1");
        assert_eq!(b.get("gh", 0).unwrap().as_str(), "first-value-long");
        assert_eq!(b.get("gh", 1).unwrap().as_str(), "second-value-long");

        // Both directions; `secret_list` shows the same names on both.
        b.set("slack", "xoxb-long-token-value", None, "cli")
            .await
            .unwrap();
        pump(&hb, &ea).await;
        a.refresh().await.unwrap();
        assert_eq!(list_names(&a), ["gh", "slack"]);
        assert_eq!(list_names(&a), list_names(&b));
        assert_eq!(a.get("slack", 0).unwrap().as_str(), "xoxb-long-token-value");

        // A delete replicates too.
        a.delete("gh").await.unwrap();
        pump(&ha, &eb).await;
        b.refresh().await.unwrap();
        assert_eq!(list_names(&b), ["slack"]);
    }

    #[tokio::test]
    async fn adopting_a_mesh_key_reseals_rows() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("gh", "ghp_0123456789", None, "user").await.unwrap();
        let other = tempfile::tempdir().unwrap();
        let o = open(other.path()).await;
        assert_ne!(*v.mesh_key(), *o.mesh_key());
        v.adopt_keyring(o.keyring()).await.unwrap();
        assert_eq!(*v.mesh_key(), *o.mesh_key());
        assert_eq!(v.get("gh", 0).unwrap().as_str(), "ghp_0123456789");
        let (rows, _) = store::rows(v.pool()).await.unwrap();
        assert!(
            rows.iter().all(|r| r.key_id == key_id(&o.mesh_key())),
            "pairing sweeps at once"
        );
        drop(v);
        let v = open(dir.path()).await;
        assert_eq!(
            *v.mesh_key(),
            *o.mesh_key(),
            "the adopted key is what reopens"
        );
        assert_eq!(v.get("gh", 0).unwrap().as_str(), "ghp_0123456789");
    }

    /// `fixtures/vault-pre-dep-w2b`: a data dir written by the pre-bump code
    /// (chacha20poly1305 0.10, rand 0.9, base64 0.22) under a dummy file key:
    /// `secrets.sqlite` rows plus an unmigrated pre-67 `secrets.vault`. The
    /// current crypto stack must open every byte of it.
    #[tokio::test]
    async fn vault_written_by_old_crypto_stack_still_opens() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/vault-pre-dep-w2b");
        let dir = tempfile::tempdir().unwrap();
        for f in ["secrets.key", "secrets.sqlite", LEGACY_FILE] {
            std::fs::copy(fixture.join(f), dir.path().join(f)).unwrap();
        }
        let v = open(dir.path()).await;
        assert_eq!(v.get("gh", 0).unwrap().as_str(), "dummy-old-value-2");
        assert_eq!(v.get("gh", 1).unwrap().as_str(), "dummy-old-value-1");
        assert_eq!(
            v.get("legacy_tok", 0).unwrap().as_str(),
            "legacy-dummy-value-1"
        );
    }

    // --- 183 §6.9: the keyring ------------------------------------------------

    async fn replica(
        dir: &Path,
        v: &Vault,
    ) -> (Arc<replica::engine::Engine>, replica::engine::ReplicaHandle) {
        let e = replica::engine::Engine::open(&dir.join("replica.sqlite"))
            .await
            .unwrap();
        let h = e.attach(v.pool(), store::TABLES).await.unwrap();
        (e, h)
    }

    async fn key_ids(v: &Vault) -> BTreeMap<String, usize> {
        let (rows, _) = store::rows(v.pool()).await.unwrap();
        let mut by = BTreeMap::new();
        for r in rows {
            *by.entry(r.key_id).or_insert(0) += 1;
        }
        by
    }

    async fn meta_keys(v: &Vault) -> Vec<serde_json::Value> {
        store::meta_get(v.pool(), "mesh_keys")
            .await
            .unwrap()
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap_or_default()
    }

    /// §6.9.6, W-33, §16: the sweep re-seals every row (all kept versions, `oauth/*`, `mesh/*`)
    /// in place; reads, `:-N` and restore keep working; the old key goes only after the grace,
    /// with no hold open, and once nothing names it.
    #[tokio::test]
    async fn rotation_reseals_every_row_and_retires_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        for n in ["gh", "slack", "aws"] {
            v.set(n, &format!("{n}-first-value"), None, "user")
                .await
                .unwrap();
            v.set(n, &format!("{n}-second-value"), None, "user")
                .await
                .unwrap();
        }
        v.set_if_version("oauth/github", "{\"t\":1}", "oauth", None)
            .await
            .unwrap();
        let old = v.current_key();
        let sealed = v.seal_with_mesh_key(b"blob", b"psk-bytes");
        assert!(v.rotation().is_none());

        let k1 = v
            .rotate(
                "a",
                vec!["a".into(), "b".into()],
                vec!["c".into()],
                0,
                false,
            )
            .await
            .unwrap();
        assert_eq!((k1.epoch, k1.initiator.as_str()), (1, "a"));
        assert_eq!(k1.id, key_id(&v.mesh_key()));
        assert_eq!(v.held_keys(), [k1.id.clone(), old.id.clone()]);
        // Both keys valid before the sweep: old rows read, new rows seal under K1.
        assert_eq!(v.get("gh", 1).unwrap().as_str(), "gh-first-value");
        v.set("new", "written-after-mint", None, "user")
            .await
            .unwrap();
        assert_eq!(key_ids(&v).await.get(&k1.id), Some(&1));

        let t = now_ms();
        v.release_sweep();
        v.maintain(t).await.unwrap();
        assert_eq!(
            key_ids(&v).await.into_iter().collect::<Vec<_>>(),
            [(k1.id.clone(), 8)],
            "every row and kept version, oauth/* included"
        );
        let r = v.rotation().unwrap();
        assert_eq!(
            (r.phase, r.rows_total, r.rows_done),
            (RotationPhase::Waiting, 8, 8)
        );
        assert_eq!(r.removed, ["c"]);
        assert_eq!(v.get("gh", 0).unwrap().as_str(), "gh-second-value");
        let mut args = serde_json::json!({"h": "{{secret:gh:-1}}"});
        v.expand(&mut args).unwrap();
        assert_eq!(args["h"], "gh-first-value");
        // secret_restore across the rotation: v1 re-sealed in place, v3 under K1.
        let info = v.restore("gh", 1).await.unwrap();
        assert_eq!(
            (info.current, v.get("gh", 0).unwrap().as_str()),
            (3, "gh-first-value")
        );
        assert!(key_ids(&v).await.keys().all(|k| *k == k1.id));
        // A blob sealed before the rotation (the link PSK) opens by trial, and re-seals.
        assert_eq!(
            v.open_with_mesh_key(b"blob", &sealed).unwrap().as_slice(),
            b"psk-bytes"
        );
        let resealed = v.reseal_with_mesh_key(b"blob", &sealed).unwrap();
        assert!(open_bytes(&v.mesh_key(), b"blob", &resealed).is_some());
        assert!(v.reseal_with_mesh_key(b"blob", &resealed).is_none());

        // Not before 10 min; not while a hold is open; then destroyed.
        v.maintain(t + RETIRE_GRACE_MS - 1_000).await.unwrap();
        assert_eq!(v.held_keys().len(), 2);
        let hold = v.retire_hold("test");
        v.maintain(t + RETIRE_GRACE_MS + 1_000).await.unwrap();
        assert_eq!(v.held_keys().len(), 2, "a RetireHold keeps the old key");
        drop(hold);
        v.maintain(t + RETIRE_GRACE_MS + 1_000).await.unwrap();
        assert_eq!(v.held_keys(), vec![k1.id.clone()]);
        assert!(meta_keys(&v).await.is_empty(), "no mesh_keys entry is left");
        let r = v.rotation().unwrap();
        assert!(r.old_destroyed);
        assert_eq!(r.phase, RotationPhase::Done);
        assert!(v.open_with_mesh_key(b"blob", &sealed).is_none());

        // A restart reopens the new key and the record.
        drop(v);
        let v = open(dir.path()).await;
        assert_eq!(v.current_key(), k1);
        assert_eq!(v.get("gh", 1).unwrap().as_str(), "gh-second-value");
        assert_eq!(v.rotation().unwrap().phase, RotationPhase::Done);
    }

    /// 183 Z2 (found live): with two older keys held, retiring the oldest does not report the
    /// rotation's own old key destroyed; that waits until the newest retained key goes too.
    #[tokio::test]
    async fn old_destroyed_waits_for_the_rotations_own_old_key() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("gh", "x", None, "user").await.unwrap();
        v.rotate("a", vec!["a".into()], vec!["b".into()], 0, false)
            .await
            .unwrap();
        v.release_sweep();
        v.maintain(now_ms()).await.unwrap();
        let t1 = now_ms();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        v.rotate("a", vec!["a".into()], vec!["c".into()], 0, false)
            .await
            .unwrap();
        let t2 = now_ms();
        v.release_sweep();
        v.maintain(t2).await.unwrap();
        v.maintain(t1 + RETIRE_GRACE_MS + 100).await.unwrap();
        assert_eq!(v.held_keys().len(), 2, "K0 retired, K1 still held");
        let r = v.rotation().unwrap();
        assert_eq!((r.old_destroyed, r.phase), (false, RotationPhase::Waiting));
        v.maintain(t2 + RETIRE_GRACE_MS + 100).await.unwrap();
        let r = v.rotation().unwrap();
        assert_eq!((r.old_destroyed, r.phase), (true, RotationPhase::Done));
    }

    /// W-32, §13 row 33: a crash after one batch of 64 leaves both keys valid and resumes on
    /// the next start; a holder that did not mint sweeps only after the 60 s grace.
    #[tokio::test]
    async fn a_crash_mid_sweep_resumes_and_another_holder_takes_over_after_60_s() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path()).await;
        let b = open(db.path()).await;
        b.adopt_keyring(a.keyring()).await.unwrap();
        // Both hold the same 70 rows under K0 (five versions of fourteen names).
        for v in [&a, &b] {
            for n in 0..14 {
                for i in 0..KEEP {
                    v.set(&format!("k{n:02}"), &format!("value-{n}-{i}"), None, "user")
                        .await
                        .unwrap();
                }
            }
        }
        assert_eq!(key_ids(&b).await.values().sum::<usize>(), 70);

        let k1 = a
            .rotate("a", vec!["a".into(), "b".into()], vec![], 0, false)
            .await
            .unwrap();
        let mut stuck = std::collections::BTreeSet::new();
        assert!(a.sweep_batch("", &mut stuck).await.unwrap().is_some());
        assert_eq!(key_ids(&a).await.get(&k1.id), Some(&(SWEEP_BATCH as usize)));
        drop(a); // crash: the open batch is all that is lost
        let a = open(da.path()).await;
        a.set_engine("a");
        assert_eq!(
            a.get("k13", 4).unwrap().as_str(),
            "value-13-0",
            "both keys valid"
        );
        // 183 R4: the sweep hold survives the crash; past it, the sweep resumes.
        a.maintain(now_ms() + SWEEP_HOLD_MS + 1).await.unwrap();
        assert_eq!(
            key_ids(&a).await.into_iter().collect::<Vec<_>>(),
            [(k1.id.clone(), 70)]
        );

        // B holds K1 but did not mint it: no sweep for 60 s, then it takes over.
        b.set_engine("b");
        assert!(b.adopt_key(a.keyring().remove(0)).await.unwrap());
        let t = b.rotation().unwrap().started_ms;
        b.maintain(t + 30_000).await.unwrap();
        assert!(!key_ids(&b).await.contains_key(&k1.id));
        assert_eq!(
            key_ids(&b).await.values().sum::<usize>(),
            70,
            "still every row"
        );
        b.maintain(t + SWEEP_GRACE_MS + 1_000).await.unwrap();
        assert_eq!(
            key_ids(&b).await.into_iter().collect::<Vec<_>>(),
            [(k1.id, 70)]
        );
        assert_eq!(b.get("k00", 4).unwrap().as_str(), "value-0-0");
    }

    /// W-34: two rotations at epoch 1; `(epoch, initiator)` makes the same key current on both,
    /// and the other is retained until no row names it.
    #[tokio::test]
    async fn two_rotations_at_once_the_higher_key_wins_on_both() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path()).await;
        let b = open(db.path()).await;
        b.adopt_keyring(a.keyring()).await.unwrap();
        let ka = a
            .rotate("aaa", vec![], vec!["c".into()], 0, false)
            .await
            .unwrap();
        let wa = a.keyring().remove(0);
        let kb = b
            .rotate("bbb", vec![], vec!["d".into()], 0, false)
            .await
            .unwrap();
        assert_eq!((ka.epoch, kb.epoch), (1, 1));
        assert!(
            a.adopt_key(b.keyring().remove(0)).await.unwrap(),
            "bbb orders above aaa"
        );
        assert!(!b.adopt_key(wa).await.unwrap(), "aaa is retained");
        assert_eq!(a.current_key().id, kb.id);
        assert_eq!(b.current_key().id, kb.id);
        assert!(a.held_keys().contains(&ka.id) && b.held_keys().contains(&ka.id));
        // A grant of a held key only adds members.
        let mut again = b.keyring().remove(0);
        again.info.members.push("e".into());
        assert!(!a.adopt_key(again).await.unwrap());
        assert!(a.current_key().members.contains(&"e".to_owned()));
        // A key whose id does not match its bytes is refused.
        let mut forged = b.keyring().remove(0);
        forged.info.id = "0000000000000000".into();
        assert!(a.adopt_key(forged).await.is_err());
    }

    /// W-38, §13 row 32: rows re-sealed under a key this engine does not hold yet keep their
    /// in-memory values, and the key is named for a `KeyRequest`; after a restart in that
    /// window they wait; the grant opens them. A removed engine never opens them.
    #[tokio::test]
    async fn rows_under_a_key_not_held_keep_their_last_value_until_the_key_comes() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path()).await;
        let b = open(db.path()).await;
        b.adopt_keyring(a.keyring()).await.unwrap();
        let ((_ea, ha), (eb, _hb)) = (replica(da.path(), &a).await, replica(db.path(), &b).await);
        a.set("gh", "ghp-value-0123", None, "user").await.unwrap();
        pump(&ha, &eb).await;
        b.refresh().await.unwrap();
        let k1 = a.rotate("a", vec![], vec![], 0, false).await.unwrap();
        a.set_engine("a");
        a.release_sweep();
        a.maintain(now_ms()).await.unwrap();
        a.set("after", "written-after-rotation", None, "user")
            .await
            .unwrap();
        pump(&ha, &eb).await;
        b.refresh().await.unwrap();
        assert_eq!(
            b.get("gh", 0).unwrap().as_str(),
            "ghp-value-0123",
            "never blank"
        );
        assert!(b.get("after", 0).is_none());
        assert_eq!(b.missing_keys(), vec![k1.id.clone()]);
        let fresh = open(db.path()).await;
        assert!(
            fresh.get("gh", 0).is_none(),
            "after a restart: waiting for the key"
        );
        b.adopt_key(a.keyring().remove(0)).await.unwrap();
        assert!(b.missing_keys().is_empty());
        assert_eq!(
            b.get("after", 0).unwrap().as_str(),
            "written-after-rotation"
        );
    }

    /// §6.9.9, W-35: a joiner during a sweep gets the retained keys too, so rows not yet swept
    /// open on it; its own rows move under the owner's current key at once.
    #[tokio::test]
    async fn pairing_during_a_rotation_carries_the_retained_keys() {
        let (da, dd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path()).await;
        let d = open(dd.path()).await;
        let ((_ea, ha), (ed, _hd)) = (replica(da.path(), &a).await, replica(dd.path(), &d).await);
        a.set("gh", "ghp-old-key-row", None, "user").await.unwrap();
        let k1 = a.rotate("a", vec![], vec![], 0, false).await.unwrap(); // not swept yet
        d.set("mine", "joiner-own-row", None, "user").await.unwrap();
        let mut ring = a.keyring();
        assert_eq!(ring.len(), 2);
        ring.iter_mut()
            .for_each(|k| k.info.members.push("d".into()));
        d.adopt_keyring(ring).await.unwrap();
        assert_eq!(d.current_key().id, k1.id);
        assert_eq!(
            d.held_keys().len(),
            3,
            "K1, the retained K0, its own old key"
        );
        pump(&ha, &ed).await;
        d.refresh().await.unwrap();
        assert_eq!(d.get("gh", 0).unwrap().as_str(), "ghp-old-key-row");
        assert_eq!(d.get("mine", 0).unwrap().as_str(), "joiner-own-row");
        assert_eq!(key_ids(&d).await.get(&k1.id), Some(&1), "its own row swept");
    }

    /// §6.9.10, §13 row 40: a pre-183 peer's `''` row (sealed under the legacy key) opens by
    /// trial, is swept while the legacy key is held, and keeps that key until it is swept.
    #[tokio::test]
    async fn a_legacy_row_opens_by_trial_and_holds_the_legacy_key() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        let legacy = v.mesh_key();
        v.rotate("a", vec![], vec![], 0, false).await.unwrap();
        sqlx::query(
            "INSERT INTO vault (id, name, version, ciphertext, note, source, updated_ms) VALUES ('old#1', 'old', 1, ?, '', 'cli', 1)",
        )
        .bind(seal(&legacy, b"old#1", b"from-a-pre-183-peer"))
        .execute(v.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO vault_current (name, version) VALUES ('old', 1)")
            .execute(v.pool())
            .await
            .unwrap();
        v.refresh().await.unwrap();
        assert_eq!(v.get("old", 0).unwrap().as_str(), "from-a-pre-183-peer");
        let t = now_ms() + RETIRE_GRACE_MS + 1_000;
        v.retire(t).await.unwrap();
        assert_eq!(
            v.held_keys().len(),
            2,
            "a '' row still opens with the legacy key"
        );
        v.set_engine("a");
        v.maintain(t).await.unwrap();
        assert_eq!(v.held_keys().len(), 1);
        assert_eq!(v.get("old", 0).unwrap().as_str(), "from-a-pre-183-peer");
    }

    /// W-36: names with a kept version written no later than the cutoff (all when `None`);
    /// OAuth grants apart; `mesh/*` and `sandbox/*` never.
    #[tokio::test]
    async fn unpair_preview_names_respect_the_window_and_the_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        for (n, val) in [
            ("early", "early-value-1"),
            ("late", "late-value-12"),
            ("oauth/github", "{}"),
            ("oauth/google", "{}"),
            ("mesh/device_key", "AAAA"),
            ("sandbox/vm1/vnc", "secret-vnc"),
        ] {
            v.set_if_version(n, val, "user", None).await.unwrap();
        }
        sqlx::query(
            "UPDATE vault SET updated_ms = 1000 WHERE name IN ('early', 'oauth/github', 'mesh/device_key', 'sandbox/vm1/vnc')",
        )
        .execute(v.pool())
        .await
        .unwrap();
        v.refresh().await.unwrap();
        let (names, grants) = v.names_written_by(Some(5_000));
        assert_eq!(
            (names, grants),
            (vec!["early".to_owned()], vec!["github".to_owned()])
        );
        let (names, grants) = v.names_written_by(None);
        assert_eq!(names, ["early", "late"]);
        assert_eq!(grants, ["github", "google"]);
    }

    /// D11: the device key gets a new value, sealed under the current key; none = nothing.
    #[tokio::test]
    async fn rotate_device_key_writes_a_new_value() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        assert!(!v.rotate_device_key().await.unwrap(), "no device key yet");
        let before = v.mesh_device_key().await.unwrap();
        assert!(v.rotate_device_key().await.unwrap());
        let after = v.mesh_device_key_if_set().unwrap().unwrap();
        assert_ne!(*before, *after);
        assert_eq!(v.get_versioned(Vault::MESH_DEVICE_KEY).unwrap().0, 2);
    }

    /// 183 R1: an epoch of `u32::MAX` adopted by a pairing does not brick later rotations.
    #[tokio::test]
    async fn rotate_after_an_exhausted_epoch_is_an_error_not_a_panic() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (open(da.path()).await, open(db.path()).await);
        a.rotate("a", vec![], vec![], 0, false).await.unwrap();
        let mut ring = a.keyring();
        ring[0].info.epoch = u32::MAX;
        b.adopt_keyring(ring).await.unwrap();
        let e = b.rotate("b", vec![], vec![], 0, false).await.unwrap_err();
        assert!(format!("{e:#}").contains("epoch exhausted"));
    }

    /// 183 R1 (S4): a granted key far above ours is refused; a near one is taken.
    #[tokio::test]
    async fn adopt_key_refuses_an_implausible_epoch() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (open(da.path()).await, open(db.path()).await);
        b.adopt_keyring(a.keyring()).await.unwrap();
        a.rotate("a", vec![], vec![], 0, false).await.unwrap();
        let mut far = a.keyring().remove(0);
        far.info.epoch = b.current_key().epoch + MAX_EPOCH_JUMP + 1;
        assert!(b.adopt_key(far).await.is_err());
        assert!(b.adopt_key(a.keyring().remove(0)).await.unwrap());
    }

    /// 183 R1: `floor` (a peer's epoch) lifts the new epoch.
    #[tokio::test]
    async fn rotate_honours_the_epoch_floor() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        assert_eq!(
            v.rotate("a", vec![], vec![], 7, false).await.unwrap().epoch,
            8
        );
    }

    /// 183 R1, R4: rows stay under the old key until a peer acks (`release_sweep`) or the hold
    /// ends, a restart included.
    #[tokio::test]
    async fn rotation_holds_the_first_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.set("gh", "ghp-value-0123", None, "user").await.unwrap();
        let k1 = v.rotate("a", vec![], vec![], 0, false).await.unwrap();
        let t = now_ms();
        v.maintain(t).await.unwrap();
        assert!(!key_ids(&v).await.contains_key(&k1.id), "held");
        drop(v);
        let v = open(dir.path()).await;
        v.set_engine("a");
        v.maintain(t).await.unwrap();
        assert!(
            !key_ids(&v).await.contains_key(&k1.id),
            "held after a restart"
        );
        v.release_sweep();
        v.maintain(t).await.unwrap();
        assert!(key_ids(&v).await.contains_key(&k1.id), "released");

        let dw = tempfile::tempdir().unwrap();
        let w = open(dw.path()).await;
        w.set("gh", "ghp-value-0123", None, "user").await.unwrap();
        let k = w.rotate("a", vec![], vec![], 0, false).await.unwrap();
        w.maintain(now_ms() + SWEEP_HOLD_MS + 1).await.unwrap();
        assert!(key_ids(&w).await.contains_key(&k.id), "hold expired");
    }

    /// 183 R1 (D11): the pending reset survives a restart; finishing it writes a new device
    /// key version once and clears the flag.
    #[tokio::test]
    async fn device_reset_pending_survives_a_restart_and_finishes_once() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        v.mesh_device_key().await.unwrap();
        v.rotate("a", vec![], vec![], 0, true).await.unwrap();
        drop(v);
        let v = open(dir.path()).await;
        assert!(v.rotation().unwrap().device_reset_pending);
        assert_eq!(v.get_versioned(Vault::MESH_DEVICE_KEY).unwrap().0, 1);
        assert!(v.finish_device_reset().await.unwrap());
        assert_eq!(v.get_versioned(Vault::MESH_DEVICE_KEY).unwrap().0, 2);
        assert!(!v.rotation().unwrap().device_reset_pending);
        assert!(!v.finish_device_reset().await.unwrap());
        // No device key: the flag clears without a write.
        let dw = tempfile::tempdir().unwrap();
        let w = open(dw.path()).await;
        w.rotate("a", vec![], vec![], 0, true).await.unwrap();
        assert!(!w.finish_device_reset().await.unwrap());
        assert!(!w.rotation().unwrap().device_reset_pending);
    }

    /// 183 R1 (S3): the `''` trial decrypt is reused for `LEGACY_CHECK_MS`.
    #[tokio::test]
    async fn the_legacy_trial_is_throttled() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path()).await;
        let legacy = v.mesh_key();
        v.rotate("a", vec![], vec![], 0, false).await.unwrap();
        sqlx::query(
            "INSERT INTO vault (id, name, version, ciphertext, note, source, updated_ms) VALUES ('old#1', 'old', 1, ?, '', 'cli', 1)",
        )
        .bind(seal(&legacy, b"old#1", b"from-a-pre-183-peer"))
        .execute(v.pool())
        .await
        .unwrap();
        let t = now_ms() + RETIRE_GRACE_MS + 1_000;
        v.retire(t).await.unwrap();
        assert_eq!(v.held_keys().len(), 2, "the '' row opens with it");
        sqlx::query("DELETE FROM vault")
            .execute(v.pool())
            .await
            .unwrap();
        v.retire(t + 1_000).await.unwrap();
        assert_eq!(v.held_keys().len(), 2, "answer reused");
        v.retire(t + LEGACY_CHECK_MS + 1).await.unwrap();
        assert_eq!(v.held_keys().len(), 1, "rechecked");
    }
}
