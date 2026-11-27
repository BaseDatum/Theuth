//! The seal gate (design doc 225): whether this engine's vault is open, the unlock methods, and
//! the device-unlock protocol between a sealed node and its owner engines.
//!
//! An engine whose DEK no method opens without a human boots **sealed**: [`Gate`] holds no
//! [`Vault`], every secret read answers [`SealError::Sealed`], and the plugin finishes its start
//! once [`Gate::wait_open`] resolves. Everything else (mesh, sync, sessions) runs.
//!
//! Device unlock rides the existing peer-call path (`theuth/rpc/1`, `agent::rpc_forward::to_peer`,
//! installed here as [`PeerCall`]); the receiving side learns the caller's authenticated endpoint
//! id from the forwarded marker ([`install_caller`]). Two one-way calls, each checked:
//!
//! 1. node → owner `vault_peer_ask(nonce, purpose, name)`: `purpose` = `unlock` | `enroll`. The
//!    owner answers `{"key": …}` at once (an `unlock` for a node set to unlock automatically; an
//!    `enroll` within [`PAIRING_GRACE_MS`] of pairing), `{"pending": true}` after showing a prompt,
//!    or an error (not an unlock device for that node, owner sealed).
//! 2. owner → node `vault_peer_grant(nonce, purpose, key)` after the human approved.
//!
//! The node accepts a key only for an outstanding nonce it minted, from the owner it sent that
//! nonce to, for the same purpose, within [`ASK_TTL_MS`]; the nonce is consumed by the first
//! grant (a replay finds nothing). The key is `K_dev = SHA-256(domain ‖ S ‖ node id)` where `S`
//! is a 32-byte secret in the *owner's* own vault (`meta` row `unseal/device-secret`, sealed by the
//! owner's DEK): neither side ever stores the other's DEK, and the node keeps only
//! `wrap(DEK, K_dev)`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use platform::time::now_ms;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::envelope::{self, Kind, KeyFile, LockedKey, Wrapped};
use crate::vault::{KeyStore, Vault};

/// JSON-RPC error code of [`SealError::Sealed`].
pub const VAULT_SEALED: i32 = -32041;
/// JSON-RPC error code of [`SealError::WrongPassword`] and [`SealError::Throttled`].
pub const WRONG_PASSWORD: i32 = -32042;
/// An unanswered ask (and its nonce) lives this long.
pub const ASK_TTL_MS: u64 = 10 * 60 * 1000;
/// An `enroll` ask within this long after pairing is approved without a prompt: the pairing
/// ceremony was the human's approval.
pub const PAIRING_GRACE_MS: u64 = 15 * 60 * 1000;
/// Free password attempts before the back-off starts.
const FREE_ATTEMPTS: u32 = 5;
const DEVICE_SECRET: &str = "unseal/device-secret";
const NODES: &str = "unseal/nodes";
const KDEV_DOMAIN: &[u8] = b"theuth-unseal-kdev-1\0";

/// Why a vault operation did not happen. `Display` is the user-facing text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SealError {
    /// The vault is sealed (the text names the engine and how to unlock it).
    Sealed(String),
    /// The master password did not open the vault.
    WrongPassword,
    /// Too many wrong passwords: retry after this many seconds.
    Throttled(u64),
    /// Refused (a replayed nonce, an unknown method, the last method, a short password, …).
    Refused(String),
    /// I/O or another failure.
    Failed(String),
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealError::Sealed(m) | SealError::Refused(m) | SealError::Failed(m) => f.write_str(m),
            SealError::WrongPassword => f.write_str("wrong password"),
            SealError::Throttled(s) => write!(f, "too many wrong passwords; try again in {s}s"),
        }
    }
}

impl std::error::Error for SealError {}

impl SealError {
    /// The JSON-RPC error code.
    pub fn code(&self) -> i32 {
        match self {
            SealError::Sealed(_) => VAULT_SEALED,
            SealError::WrongPassword | SealError::Throttled(_) => WRONG_PASSWORD,
            SealError::Refused(_) => -32602,
            SealError::Failed(_) => -32603,
        }
    }
}

fn failed(e: impl std::fmt::Display) -> SealError {
    SealError::Failed(e.to_string())
}

/// One unlock method, as lists show it. Never key material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "VaultMethod"))]
pub struct MethodView {
    /// Kind.
    pub kind: Kind,
    /// `keychain` | `password` | `file` | the owner endpoint id.
    pub id: String,
    /// Human text.
    pub label: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
}

impl MethodView {
    /// The view of a stored method.
    pub fn of(w: &Wrapped) -> Self {
        let label = match w.kind {
            Kind::Keychain => "This machine's keychain".to_owned(),
            Kind::Password => "Master password".to_owned(),
            Kind::Device if w.label.is_empty() => format!("Unlock from {}", short(&w.id)),
            Kind::Device => format!("Unlock from {}", w.label),
            Kind::File => "Key file on disk (unprotected)".to_owned(),
        };
        MethodView {
            kind: w.kind,
            id: w.id.clone(),
            label,
            created_ms: w.created_ms,
        }
    }
}

fn short(id: &str) -> &str {
    id.get(..10).unwrap_or(id)
}

/// `vault_status`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VaultStatus {
    /// This engine's mesh id, when known.
    pub engine: Option<String>,
    /// This engine's name, when known.
    pub name: Option<String>,
    /// `unsealed` | `sealed`.
    pub state: String,
    /// `keychain` | `file` | `password` | `device` | `memory`; `None` while sealed.
    pub unsealed_by: Option<String>,
    /// Every method.
    pub methods: Vec<MethodView>,
    /// A `password` method exists.
    pub password_set: bool,
    /// Loud text when the key is not protected.
    pub warning: Option<String>,
    /// Owner engines a sealed engine is asking right now.
    pub asking: Vec<String>,
    /// Why it is sealed.
    pub error: Option<String>,
}

/// A prompt waiting on this (owner) engine.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UnsealRequestView {
    /// Prompt id.
    pub id: String,
    /// The asking node's endpoint id.
    pub node: String,
    /// Its name.
    pub node_name: String,
    /// `unlock` | `enroll`.
    pub purpose: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
}

/// A node this engine unlocks.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UnlockDevice {
    /// Its endpoint id.
    pub node: String,
    /// Its name.
    pub name: String,
    /// Approve its unlock requests without a prompt.
    pub auto: bool,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub enrolled_ms: u64,
}

/// What `vault_peer_ask` answers.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AskReply {
    /// base64 K_dev, when approved at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A prompt is showing on the owner.
    #[serde(default)]
    pub pending: bool,
    /// The owner's name (labels the node's device method).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One call to a paired engine over `theuth/rpc/1`. `Ok(None)`: not connected.
pub trait PeerCall: Send + Sync {
    /// Call `method` with positional `params` on `peer`.
    fn call<'a>(
        &'a self,
        peer: &'a str,
        method: &'static str,
        params: serde_json::Value,
    ) -> futures::future::BoxFuture<'a, Result<Option<serde_json::Value>, String>>;
}

static PEER: OnceLock<Arc<dyn PeerCall>> = OnceLock::new();
/// The caller's authenticated endpoint id of a forwarded request (the agent's `Forwarded`).
pub type CallerOf = fn(&jsonrpsee::Extensions) -> Option<String>;
static CALLER: OnceLock<CallerOf> = OnceLock::new();
static GATE: OnceLock<Arc<Gate>> = OnceLock::new();

/// The composition installs the peer call (`agent::rpc_forward::to_peer`) once.
pub fn install_peer_call(p: Arc<dyn PeerCall>) {
    let _ = PEER.set(p);
}

/// The composition installs how to read a forwarded request's caller once.
pub fn install_caller(f: CallerOf) {
    let _ = CALLER.set(f);
}

/// The forwarded caller of a request; `None` = not forwarded over the mesh.
pub fn caller(ext: &jsonrpsee::Extensions) -> Option<String> {
    CALLER.get().and_then(|f| f(ext))
}

/// This process's gate (the plugin installs it); `None` before the plugin ran.
pub fn installed() -> Option<Arc<Gate>> {
    GATE.get().cloned()
}

/// Install the process gate.
pub fn install(g: Arc<Gate>) {
    let _ = GATE.set(g);
}

#[derive(Clone, Debug)]
struct Ask {
    owner: String,
    purpose: String,
    at_ms: u64,
}

#[derive(Clone, Debug)]
struct Prompt {
    view: UnsealRequestView,
    nonce: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Node {
    name: String,
    auto: bool,
    enrolled_ms: u64,
}

/// The seal gate of one engine.
pub struct Gate {
    dir: PathBuf,
    me: RwLock<String>,
    name: RwLock<String>,
    vault: RwLock<Option<Arc<Vault>>>,
    boot_error: RwLock<Option<String>>,
    by: RwLock<Option<&'static str>>,
    opened: tokio::sync::watch::Sender<Option<Arc<Vault>>>,
    /// Serializes `vault-keys.json` rewrites and unseals.
    keys: tokio::sync::Mutex<()>,
    asks: Mutex<HashMap<String, Ask>>,
    failures: Mutex<(u32, u64)>,
    prompts: Mutex<BTreeMap<String, Prompt>>,
    paired: Mutex<HashMap<String, u64>>,
    peers: RwLock<Option<Arc<dyn PeerCall>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn rd<T: Clone>(m: &RwLock<T>) -> T {
    m.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn wr<T>(m: &RwLock<T>, v: T) {
    *m.write().unwrap_or_else(PoisonError::into_inner) = v;
}

impl Gate {
    fn new(dir: &Path) -> Self {
        Gate {
            dir: dir.to_path_buf(),
            me: RwLock::default(),
            name: RwLock::new(platform::process::hostname().to_owned()),
            vault: RwLock::default(),
            boot_error: RwLock::default(),
            by: RwLock::default(),
            opened: tokio::sync::watch::Sender::new(None),
            keys: tokio::sync::Mutex::new(()),
            asks: Mutex::default(),
            failures: Mutex::default(),
            prompts: Mutex::default(),
            paired: Mutex::default(),
            peers: RwLock::default(),
        }
    }

    /// Boot: open the vault when a method opens without a human, else start sealed.
    pub async fn boot(dir: &Path, ks: KeyStore) -> anyhow::Result<Arc<Gate>> {
        Self::boot_with(dir, ks, &envelope::OsKeychain).await
    }

    /// [`Gate::boot`] with a given keychain (tests).
    pub async fn boot_with(
        dir: &Path,
        ks: KeyStore,
        kc: &dyn envelope::Keychain,
    ) -> anyhow::Result<Arc<Gate>> {
        std::fs::create_dir_all(dir)?;
        let gate = Arc::new(Gate::new(dir));
        match envelope::boot(dir, ks, kc)? {
            envelope::Boot::Unsealed { dek, by } => gate.open(dek, by).await.map_err(|e| anyhow::anyhow!("{e}"))?,
            envelope::Boot::Sealed { reason } => {
                tracing::warn!("secrets: {reason}");
                wr(&gate.boot_error, Some(reason));
            }
        }
        Ok(gate)
    }

    /// Use `p` for peer calls instead of the installed one (tests).
    pub fn set_peers(&self, p: Arc<dyn PeerCall>) {
        wr(&self.peers, Some(p));
    }

    fn peers(&self) -> Option<Arc<dyn PeerCall>> {
        rd(&self.peers).or_else(|| PEER.get().cloned())
    }

    /// This engine's mesh id and name (the mesh plugin tells it).
    pub fn set_identity(&self, id: &str, name: &str) {
        wr(&self.me, id.to_owned());
        if !name.is_empty() {
            wr(&self.name, name.to_owned());
        }
    }

    /// The open vault; `None` while sealed.
    pub fn vault(&self) -> Option<Arc<Vault>> {
        rd(&self.vault).filter(|v| !v.is_sealed())
    }

    /// The open vault, or the sealed error.
    pub fn require(&self) -> Result<Arc<Vault>, SealError> {
        self.vault().ok_or_else(|| SealError::Sealed(self.sealed_text()))
    }

    /// Whether reads are refused.
    pub fn is_sealed(&self) -> bool {
        self.vault().is_none()
    }

    /// "sky-cloud is sealed — unlock it from your phone or with the master password".
    pub fn sealed_text(&self) -> String {
        let name = rd(&self.name);
        let f = envelope::load(&self.dir).ok().flatten().unwrap_or_default();
        let device = f.methods.iter().any(|m| m.kind == Kind::Device);
        let password = f.find("password").is_some();
        let how = match (device, password) {
            (true, true) => "unlock it from your phone or another of your devices, or with the master password",
            (true, false) => "unlock it from your phone or another of your devices",
            (false, true) => "unlock it with the master password",
            (false, false) => "it has no unlock method left",
        };
        format!("{name} is sealed — {how}")
    }

    /// Resolves with the vault once it is open (at once when it is).
    pub async fn wait_open(&self) -> Arc<Vault> {
        let mut rx = self.opened.subscribe();
        loop {
            if let Some(v) = rx.borrow_and_update().clone() {
                return v;
            }
            if rx.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    async fn open(&self, dek: LockedKey, by: &'static str) -> Result<(), SealError> {
        if let Some(v) = rd(&self.vault) {
            if !v.unseal_with(&dek).await.map_err(failed)? {
                return Err(SealError::Failed("that key opens another vault".into()));
            }
        } else {
            envelope::harden_process();
            let v = Arc::new(Vault::open_with_dek(&self.dir, dek, by).await.map_err(|e| failed(format!("{e:#}")))?);
            wr(&self.vault, Some(v.clone()));
            self.opened.send_replace(Some(v));
        }
        wr(&self.by, Some(by));
        wr(&self.boot_error, None);
        lock(&self.asks).retain(|_, a| a.purpose != "unlock");
        tracing::info!("secrets: vault unsealed ({by})");
        Ok(())
    }

    fn keyfile(&self) -> Result<KeyFile, SealError> {
        Ok(envelope::load(&self.dir).map_err(failed)?.unwrap_or(KeyFile { version: 1, methods: Vec::new() }))
    }

    /// Status for `vault_status`.
    pub fn status(&self) -> VaultStatus {
        let f = self.keyfile().unwrap_or_default();
        let sealed = self.is_sealed();
        let by = rd(&self.by);
        let warning = if f.methods.iter().any(|m| m.kind == Kind::File) {
            Some("vault key stored unprotected on disk — add a master password".to_owned())
        } else if !sealed && f.methods.is_empty() {
            Some("vault key is in memory only — add a master password now, or the vault is lost at the next restart".to_owned())
        } else {
            None
        };
        let now = now_ms();
        let mut asking: Vec<String> = lock(&self.asks)
            .values()
            .filter(|a| a.purpose == "unlock" && now < a.at_ms + ASK_TTL_MS)
            .map(|a| a.owner.clone())
            .collect();
        asking.sort();
        asking.dedup();
        let me = rd(&self.me);
        VaultStatus {
            engine: (!me.is_empty()).then_some(me),
            name: Some(rd(&self.name)),
            state: if sealed { "sealed" } else { "unsealed" }.into(),
            unsealed_by: if sealed { None } else { by.map(str::to_owned) },
            password_set: f.find("password").is_some(),
            methods: f.methods.iter().map(MethodView::of).collect(),
            warning,
            asking: if sealed { asking } else { Vec::new() },
            error: if sealed {
                Some(rd(&self.boot_error).unwrap_or_else(|| self.sealed_text()))
            } else {
                None
            },
        }
    }

    /// `vault_seal`: lock reads now. Needs a password or device method (else nothing could
    /// unlock it but a restart).
    pub fn seal(&self) -> Result<(), SealError> {
        let f = self.keyfile()?;
        if !f.methods.iter().any(|m| matches!(m.kind, Kind::Password | Kind::Device)) {
            return Err(SealError::Refused("add a master password or a device unlock before sealing".into()));
        }
        if let Some(v) = rd(&self.vault) {
            v.seal(self.sealed_text());
        }
        tracing::info!("secrets: vault sealed");
        Ok(())
    }

    /// `vault_unseal` with the master password (Argon2id off the runtime; throttled).
    pub async fn unseal_password(&self, password: Zeroizing<String>) -> Result<(), SealError> {
        if !self.is_sealed() {
            return Ok(());
        }
        let now = now_ms();
        {
            let (n, last) = *lock(&self.failures);
            if n >= FREE_ATTEMPTS {
                let wait = (30_000u64 << (n - FREE_ATTEMPTS).min(5)).min(15 * 60 * 1000);
                if now < last + wait {
                    return Err(SealError::Throttled((last + wait - now).div_ceil(1000)));
                }
            }
        }
        let _g = self.keys.lock().await;
        let f = self.keyfile()?;
        if f.find("password").is_none() {
            return Err(SealError::Refused("no master password is set for this vault".into()));
        }
        let opened = tokio::task::spawn_blocking(move || envelope::open_password(&f, &password))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        match opened {
            Some(dek) => {
                *lock(&self.failures) = (0, 0);
                self.open(dek, "password").await
            }
            None => {
                let mut fl = lock(&self.failures);
                *fl = (fl.0 + 1, now_ms());
                tracing::warn!("secrets: wrong master password ({} in a row)", fl.0);
                Err(SealError::WrongPassword)
            }
        }
    }

    async fn rewrite(&self, edit: impl FnOnce(&mut KeyFile, &[u8; 32]) -> Result<(), SealError>) -> Result<(), SealError> {
        let v = self.require()?;
        let _g = self.keys.lock().await;
        let mut f = self.keyfile()?;
        edit(&mut f, v.dek())?;
        if f.methods.is_empty() {
            return Err(SealError::Refused("that would leave no way to unlock the vault".into()));
        }
        let drop_file = f.methods.iter().any(|m| matches!(m.kind, Kind::Password | Kind::Device))
            && f.methods.iter().any(|m| m.kind == Kind::File);
        if drop_file {
            f.methods.retain(|m| m.kind != Kind::File);
        }
        envelope::save(&self.dir, &f).map_err(failed)?;
        if drop_file || f.find("file").is_none() {
            crate::key::file_shred(&self.dir).map_err(failed)?;
        }
        Ok(())
    }

    /// `vault_add_password`.
    pub async fn add_password(&self, password: Zeroizing<String>) -> Result<(), SealError> {
        if self.keyfile()?.find("password").is_some() {
            return Err(SealError::Refused("a master password is already set; change it instead".into()));
        }
        let dek = Zeroizing::new(*self.require()?.dek());
        let w = tokio::task::spawn_blocking(move || envelope::password_method(&dek, &password))
            .await
            .map_err(failed)?
            .map_err(|e| SealError::Refused(e.to_string()))?;
        self.rewrite(|f, _| {
            f.put(w);
            Ok(())
        })
        .await
    }

    /// `vault_change_password`: the current one must open the vault.
    pub async fn change_password(&self, old: Zeroizing<String>, new: Zeroizing<String>) -> Result<(), SealError> {
        let f = self.keyfile()?;
        let dek = Zeroizing::new(*self.require()?.dek());
        let w = tokio::task::spawn_blocking(move || -> Result<Wrapped, SealError> {
            let opened = envelope::open_password(&f, &old).map_err(|e| SealError::Refused(e.to_string()))?;
            if !opened.is_some_and(|k| envelope::ct_eq(&k, &dek)) {
                return Err(SealError::WrongPassword);
            }
            envelope::password_method(&dek, &new).map_err(|e| SealError::Refused(e.to_string()))
        })
        .await
        .map_err(failed)??;
        self.rewrite(|f, _| {
            f.put(w);
            Ok(())
        })
        .await
    }

    /// `vault_remove_method`: never the last one.
    pub async fn remove_method(&self, id: &str) -> Result<(), SealError> {
        self.rewrite(|f, _| {
            if f.find(id).is_none() {
                return Err(SealError::Refused(format!("no unlock method {id}")));
            }
            if f.methods.len() == 1 {
                return Err(SealError::Refused("that is the last way to unlock the vault".into()));
            }
            f.methods.retain(|m| m.id != id);
            Ok(())
        })
        .await
    }

    /// Owner endpoint ids of this engine's device methods.
    pub fn device_owners(&self) -> Vec<String> {
        self.keyfile()
            .map(|f| f.methods.iter().filter(|m| m.kind == Kind::Device).map(|m| m.id.clone()).collect())
            .unwrap_or_default()
    }

    fn mint_ask(&self, owner: &str, purpose: &str) -> String {
        let nonce = data_encoding::BASE32_NOPAD.encode(&rand::random::<[u8; 16]>());
        let now = now_ms();
        let mut asks = lock(&self.asks);
        asks.retain(|_, a| now < a.at_ms + ASK_TTL_MS && !(a.owner == owner && a.purpose == purpose));
        asks.insert(nonce.clone(), Ask { owner: owner.to_owned(), purpose: purpose.to_owned(), at_ms: now });
        nonce
    }

    /// Node side: ask `owner` once. `Ok(true)` = done (the key came back at once), `Ok(false)` =
    /// a prompt is showing there; `Err` = not reachable or refused.
    pub async fn ask(&self, owner: &str, purpose: &str) -> Result<bool, SealError> {
        let peers = self.peers().ok_or_else(|| SealError::Failed("the mesh is not running".into()))?;
        let nonce = self.mint_ask(owner, purpose);
        let params = serde_json::json!([nonce, purpose, rd(&self.name)]);
        let reply = match peers.call(owner, "vault_peer_ask", params).await {
            Ok(Some(v)) => serde_json::from_value::<AskReply>(v).map_err(failed),
            Ok(None) => Err(SealError::Failed(format!("{} is not reachable", short(owner)))),
            Err(e) => Err(SealError::Refused(e)),
        };
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                lock(&self.asks).remove(&nonce);
                return Err(e);
            }
        };
        match reply.key {
            Some(key) => self
                .on_grant(owner, &nonce, purpose, &key, reply.name.as_deref().unwrap_or(""))
                .await
                .map(|()| true),
            None => Ok(false),
        }
    }

    /// Node side, `vault_peer_grant` (and an immediate `vault_peer_ask` answer): `caller` is the
    /// authenticated sender. Single use: the nonce must be outstanding, minted for `caller` and
    /// `purpose`, and is consumed here whatever happens next.
    pub async fn on_grant(&self, caller: &str, nonce: &str, purpose: &str, key: &str, owner_name: &str) -> Result<(), SealError> {
        let ask = {
            let mut asks = lock(&self.asks);
            match asks.get(nonce) {
                Some(a) if a.owner == caller && a.purpose == purpose && now_ms() < a.at_ms + ASK_TTL_MS => asks.remove(nonce),
                Some(_) => None,
                None => None,
            }
        };
        if ask.is_none() {
            tracing::warn!("secrets: unseal grant from {} refused: unknown, used or foreign nonce", short(caller));
            return Err(SealError::Refused("unknown or already used unlock request".into()));
        }
        let mut raw = Zeroizing::new(B64.decode(key).map_err(|_| SealError::Refused("bad key".into()))?);
        let kdev: Zeroizing<[u8; 32]> = Zeroizing::new(raw.as_slice().try_into().map_err(|_| SealError::Refused("bad key".into()))?);
        raw.zeroize();
        match purpose {
            "unlock" => {
                let _g = self.keys.lock().await;
                if !self.is_sealed() {
                    return Ok(());
                }
                let f = self.keyfile()?;
                let w = f
                    .methods
                    .iter()
                    .find(|m| m.kind == Kind::Device && m.id == caller)
                    .ok_or_else(|| SealError::Refused("not an unlock device of this vault".into()))?;
                let dek = envelope::unwrap(&kdev, w).ok_or_else(|| SealError::Refused("that key does not open this vault".into()))?;
                self.open(dek, "device").await
            }
            "enroll" => {
                let label = if owner_name.is_empty() { short(caller).to_owned() } else { owner_name.to_owned() };
                let id = caller.to_owned();
                self.rewrite(move |f, dek| {
                    f.put(Wrapped {
                        kind: Kind::Device,
                        id: id.clone(),
                        label,
                        kdf: None,
                        wrapped: envelope::wrap(&kdev, dek, Kind::Device, &id),
                        created_ms: now_ms(),
                    });
                    Ok(())
                })
                .await?;
                tracing::info!("secrets: {} can now unlock this vault", short(caller));
                Ok(())
            }
            other => Err(SealError::Refused(format!("unknown purpose {other}"))),
        }
    }

    /// Node side: while sealed, ask every device owner, retrying with back-off (5 s → 60 s).
    /// Runs until the vault opens by any method.
    pub async fn ask_until_open(self: Arc<Self>) {
        let mut wait = Duration::from_secs(5);
        while self.is_sealed() {
            let now = now_ms();
            let pending: Vec<String> = lock(&self.asks)
                .values()
                .filter(|a| a.purpose == "unlock" && now < a.at_ms + ASK_TTL_MS)
                .map(|a| a.owner.clone())
                .collect();
            let mut any = false;
            for owner in self.device_owners() {
                if pending.contains(&owner) {
                    any = true;
                    continue;
                }
                match self.ask(&owner, "unlock").await {
                    Ok(true) => return,
                    Ok(false) => {
                        any = true;
                        tracing::info!("secrets: asked {} to unlock the vault", short(&owner));
                    }
                    Err(e) => tracing::debug!("secrets: unlock ask to {}: {e}", short(&owner)),
                }
            }
            wait = if any { Duration::from_secs(15) } else { (wait * 2).min(Duration::from_secs(60)) };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.wait_open() => return,
            }
        }
    }

    /// `vault_add_device`: ask `owner` to become an unlock device. `Ok(true)` = added now.
    pub async fn add_device(&self, owner: &str) -> Result<bool, SealError> {
        self.require()?;
        self.ask(owner, "enroll").await
    }

    /// Both sides, right after a pairing with `peer` (mesh): remember it (an `enroll` from it is
    /// pre-approved for [`PAIRING_GRACE_MS`]), and on an engine without a keychain method ask it
    /// to become an unlock device.
    pub fn after_pairing(self: &Arc<Self>, peer: &str) {
        lock(&self.paired).insert(peer.to_owned(), now_ms());
        let f = self.keyfile().unwrap_or_default();
        if f.methods.iter().any(|m| m.kind == Kind::Keychain || (m.kind == Kind::Device && m.id == peer)) {
            return;
        }
        let (g, peer) = (self.clone(), peer.to_owned());
        tokio::spawn(async move {
            for wait in [3, 10, 30, 90] {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                match g.add_device(&peer).await {
                    Ok(_) => return,
                    Err(e) => tracing::info!("secrets: device unlock enrolment with {}: {e}", short(&peer)),
                }
            }
        });
    }

    // ----- owner side -----

    async fn nodes(&self, v: &Vault) -> Result<BTreeMap<String, Node>, SealError> {
        Ok(match v.local_secret(NODES).await.map_err(failed)? {
            Some(b) => serde_json::from_slice(&b).map_err(failed)?,
            None => BTreeMap::new(),
        })
    }

    async fn save_nodes(&self, v: &Vault, n: &BTreeMap<String, Node>) -> Result<(), SealError> {
        v.set_local_secret(NODES, &serde_json::to_vec(n).map_err(failed)?).await.map_err(failed)
    }

    async fn kdev(&self, v: &Vault, node: &str) -> Result<Zeroizing<[u8; 32]>, SealError> {
        let s = match v.local_secret(DEVICE_SECRET).await.map_err(failed)? {
            Some(s) => s,
            None => {
                let fresh = Zeroizing::new(rand::random::<[u8; 32]>().to_vec());
                v.set_local_secret(DEVICE_SECRET, &fresh).await.map_err(failed)?;
                fresh
            }
        };
        let mut h = Sha256::new();
        h.update(KDEV_DOMAIN);
        h.update(&*s);
        h.update(node.as_bytes());
        Ok(Zeroizing::new(h.finalize().into()))
    }

    /// Owner side, `vault_peer_ask` from authenticated `node`.
    pub async fn on_ask(&self, node: &str, nonce: &str, purpose: &str, node_name: &str) -> Result<AskReply, SealError> {
        let v = self.require()?;
        let nodes = self.nodes(&v).await?;
        let now = now_ms();
        let auto = match purpose {
            "unlock" => match nodes.get(node) {
                Some(n) => n.auto,
                None => return Err(SealError::Refused("this engine is not an unlock device for you".into())),
            },
            "enroll" => lock(&self.paired).get(node).is_some_and(|&t| now < t + PAIRING_GRACE_MS),
            other => return Err(SealError::Refused(format!("unknown purpose {other}"))),
        };
        if auto {
            if purpose == "enroll" {
                self.enrol(&v, node, node_name, false).await?;
            }
            tracing::info!("secrets: {purpose} for {} approved automatically", short(node));
            let key = self.kdev(&v, node).await?;
            return Ok(AskReply { key: Some(B64.encode(*key)), pending: false, name: Some(rd(&self.name)) });
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut p = lock(&self.prompts);
        p.retain(|_, x| !(x.view.node == node && x.view.purpose == purpose) && now < x.view.created_ms + ASK_TTL_MS);
        p.insert(
            id.clone(),
            Prompt {
                view: UnsealRequestView {
                    id,
                    node: node.to_owned(),
                    node_name: if node_name.is_empty() { short(node).to_owned() } else { node_name.to_owned() },
                    purpose: purpose.to_owned(),
                    created_ms: now,
                },
                nonce: nonce.to_owned(),
            },
        );
        Ok(AskReply { key: None, pending: true, name: Some(rd(&self.name)) })
    }

    async fn enrol(&self, v: &Vault, node: &str, name: &str, auto: bool) -> Result<(), SealError> {
        let mut nodes = self.nodes(v).await?;
        let e = nodes.entry(node.to_owned()).or_insert_with(|| Node { name: name.to_owned(), auto, enrolled_ms: now_ms() });
        e.auto |= auto;
        if !name.is_empty() {
            e.name = name.to_owned();
        }
        self.save_nodes(v, &nodes).await
    }

    /// Prompts waiting on this engine.
    pub fn requests(&self) -> Vec<UnsealRequestView> {
        let now = now_ms();
        let mut p = lock(&self.prompts);
        p.retain(|_, x| now < x.view.created_ms + ASK_TTL_MS);
        let mut v: Vec<_> = p.values().map(|x| x.view.clone()).collect();
        v.sort_by_key(|x| x.created_ms);
        v
    }

    /// `vault_answer_unseal`: `false` = already answered or expired. Approving sends the key to
    /// the node over the mesh (`vault_peer_grant`).
    pub async fn answer(&self, id: &str, approve: bool, always: bool) -> Result<bool, SealError> {
        let Some(p) = lock(&self.prompts).remove(id) else {
            return Ok(false);
        };
        if now_ms() >= p.view.created_ms + ASK_TTL_MS {
            return Ok(false);
        }
        if !approve {
            tracing::info!("secrets: {} for {} declined", p.view.purpose, short(&p.view.node));
            return Ok(true);
        }
        let v = self.require()?;
        let node = &p.view.node;
        match p.view.purpose.as_str() {
            "enroll" => self.enrol(&v, node, &p.view.node_name, always).await?,
            _ if always => self.set_auto(node, true).await?,
            _ => {}
        }
        let key = self.kdev(&v, node).await?;
        let peers = self.peers().ok_or_else(|| SealError::Failed("the mesh is not running".into()))?;
        let params = serde_json::json!([p.nonce, p.view.purpose, B64.encode(*key), rd(&self.name)]);
        match peers.call(node, "vault_peer_grant", params).await {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Err(SealError::Failed(format!("{} is not reachable right now", p.view.node_name))),
            Err(e) => Err(SealError::Failed(e)),
        }
    }

    /// Nodes this engine unlocks.
    pub async fn unlock_devices(&self) -> Result<Vec<UnlockDevice>, SealError> {
        let v = self.require()?;
        Ok(self
            .nodes(&v)
            .await?
            .into_iter()
            .map(|(node, n)| UnlockDevice { node, name: n.name, auto: n.auto, enrolled_ms: n.enrolled_ms })
            .collect())
    }

    /// `vault_set_auto_unlock`.
    pub async fn set_auto(&self, node: &str, auto: bool) -> Result<(), SealError> {
        let v = self.require()?;
        let mut nodes = self.nodes(&v).await?;
        let n = nodes.get_mut(node).ok_or_else(|| SealError::Refused("not a machine this device unlocks".into()))?;
        n.auto = auto;
        self.save_nodes(&v, &nodes).await
    }

    /// `vault_forget_node`: refuse its unlock requests from now on.
    pub async fn forget_node(&self, node: &str) -> Result<(), SealError> {
        let v = self.require()?;
        let mut nodes = self.nodes(&v).await?;
        nodes.remove(node);
        self.save_nodes(&v, &nodes).await
    }
}

/// The `tools::secrets` hook while the vault may be sealed: the real [`crate::external::Hook`]
/// once the vault opened, else a placeholder fails with the sealed text (and nothing to scrub).
pub struct GatedHook {
    gate: Arc<Gate>,
    inner: OnceLock<Arc<crate::external::Hook>>,
}

impl GatedHook {
    /// A hook with nothing behind it yet.
    pub fn new(gate: Arc<Gate>) -> Self {
        GatedHook { gate, inner: OnceLock::new() }
    }

    /// The vault opened: serve through `hook` from now on.
    pub fn set(&self, hook: Arc<crate::external::Hook>) {
        let _ = self.inner.set(hook);
    }

    fn open(&self) -> Result<&Arc<crate::external::Hook>, String> {
        match self.inner.get() {
            Some(h) if !self.gate.is_sealed() => Ok(h),
            _ => Err(self.gate.sealed_text()),
        }
    }
}

impl ::tools::secrets::Secrets for GatedHook {
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        if !::tools::secrets::mentions(args) {
            return Ok(0);
        }
        self.open()?.expand(args)
    }

    fn expand_with(
        &self,
        args: &mut serde_json::Value,
        fetched: &::tools::secrets::Fetched,
    ) -> Result<usize, String> {
        if fetched.is_empty() && !::tools::secrets::mentions(args) {
            return Ok(0);
        }
        self.open()?.expand_with(args, fetched)
    }

    fn external_access(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Vec<::tools::secrets::ExternalAccess> {
        self.inner.get().map_or_else(Vec::new, |h| h.external_access(tool, args))
    }

    fn resolve_external<'a>(
        &'a self,
        args: &'a serde_json::Value,
        cx: ::tools::secrets::ResolveCx,
    ) -> futures::future::BoxFuture<'a, Result<::tools::secrets::Fetched, String>> {
        match self.inner.get() {
            Some(h) => h.resolve_external(args, cx),
            None => Box::pin(async { Ok(::tools::secrets::Fetched::default()) }),
        }
    }

    fn redact<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        match self.inner.get() {
            Some(h) => h.redact(text),
            None => std::borrow::Cow::Borrowed(text),
        }
    }

    fn redact_value(&self, v: &mut serde_json::Value) {
        if let Some(h) = self.inner.get() {
            h.redact_value(v);
        }
    }

    fn redact_spans(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        self.inner.get().map_or_else(Vec::new, |h| h.redact_spans(text))
    }

    fn max_literal_len(&self) -> usize {
        self.inner.get().map_or(0, |h| h.max_literal_len())
    }

    fn sealed(&self) -> Option<String> {
        self.gate.is_sealed().then(|| self.gate.sealed_text())
    }
}

#[cfg(test)]
mod tests;
