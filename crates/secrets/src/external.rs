//! External secret backends (221 §4): AWS Secrets Manager, Bitwarden, and whatever fits the same
//! shape. A backend crate implements [`ExternalBackend`] for one connection and registers it on
//! the [`crate::SecretsActor`] with [`RegisterExternal`]; the `tools::secrets` hook then resolves
//! `{{secret:<backend>/<conn>/<ref>[#<key>]}}` through it, after the approval and before expansion.
//!
//! No SDK type crosses this module. Every `Display` string here is model-facing and never carries
//! a value; [`ExtError::Sdk`] text is scrubbed with `tools::secrets::redact` before it is shown.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use ::tools::secrets::{Direction, ExternalAccess, Fetched, ResolveCx};
use futures::future::BoxFuture;
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::SecretsActor;
use crate::vault::{Vault, placeholder_re};

/// The backend ids the placeholder grammar knows (221 §3.2). A reference naming any other backend
/// that no registered connection serves fails with `unknown secret backend x`.
pub const BACKENDS: &[&str] = &["aws", "bw"];

/// One connection to an external secret manager. Implementations are cheap to share
/// (`Arc<dyn ExternalBackend>`) and do their own locking; every method may be called concurrently.
#[async_trait::async_trait]
pub trait ExternalBackend: Send + Sync + 'static {
    /// `aws` | `bw`; the placeholder namespace and the always-ask key's backend.
    fn backend(&self) -> &'static str;
    /// The connection name (`[a-z0-9-]{1,32}`).
    fn conn(&self) -> &str;
    /// Cheap, no I/O: what the UIs show and what `resolve` checks first.
    fn status(&self) -> ConnStatus;
    /// Network check (AWS: `ListSecrets` max 1; Bitwarden: sync when unlocked, prelogin when not).
    async fn probe(&self) -> Result<ConnStatus, ExtError>;

    /// Metadata only. `query` = substring on name/folder/collection; `cursor` pages.
    async fn list(&self, q: ListQuery) -> Result<ListPage, ExtError>;
    /// One entry's metadata: versions, stages, fields (names and kinds, never values).
    async fn describe(&self, r: &ExtRef) -> Result<ExtEntry, ExtError>;
    /// The value at `r` (+ key/field), with the version it came from. Only `resolve_external`
    /// and `secret_external_import` call this.
    async fn get(&self, r: &ExtRef) -> Result<(Zeroizing<String>, ExtVersion), ExtError>;
    /// A new entry at `loc`; `Err(Exists)` if it exists. Returns the reference to use.
    async fn create(
        &self,
        loc: &Location,
        value: Value<'_>,
    ) -> Result<(ExtRef, ExtVersion), ExtError>;
    /// Replace the value at `r` (AWS: a new version; Bitwarden: edit the field in place).
    /// `expected` = the version the caller saw (AWS VersionId / Bitwarden revision date).
    /// A mismatch is `Conflict`, never a blind overwrite.
    async fn update(
        &self,
        r: &ExtRef,
        value: Value<'_>,
        expected: Option<&ExtVersion>,
    ) -> Result<ExtVersion, ExtError>;
    /// Delete `r` (D13). `Recoverable` = AWS recovery window / Bitwarden trash; `Permanent` only
    /// when the connection allows it (`allow_permanent_delete`), else `Err(Denied)`. `expected` as
    /// in `update`.
    async fn delete(
        &self,
        r: &ExtRef,
        mode: DeleteMode,
        expected: Option<&ExtVersion>,
    ) -> Result<DeleteOutcome, ExtError>;
    /// Undo a recoverable delete (AWS `RestoreSecret`, Bitwarden restore from trash). UI only.
    async fn restore(&self, r: &ExtRef) -> Result<ExtVersion, ExtError>;
    /// Lock (Bitwarden: drop keys; AWS: drop the cached SdkConfig credentials). Idempotent.
    async fn lock(&self);

    /// The connection as a Send maker (Bitwarden, 221 §12); `None` for backends without Sends.
    fn sender(&self) -> Option<Arc<dyn crate::tools_send::SendBackend>> {
        None
    }

    /// §6.3 `connection_login`: store this connection's credentials (sealed in `meta` through the
    /// [`ConnStore`] the factory gave it) and log in. Default: nothing to log in to.
    async fn login(&self, form: LoginForm) -> Result<LoginStep, ExtError> {
        let _ = form;
        Err(ExtError::Unsupported(format!(
            "{} connections have no login",
            self.backend()
        )))
    }
    /// §6.3 `connection_unlock` / the unlock card: turn the master password into keys.
    /// Default: nothing to unlock.
    async fn unlock(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        drop(password);
        Err(ExtError::Unsupported(format!(
            "{} connections have no unlock",
            self.backend()
        )))
    }
    /// Forget the stored credentials (the connection is being removed). Default: nothing stored.
    async fn logout(&self) {}
    /// The connection's D20 settings changed (also called once after the factory built it).
    fn apply_settings(&self, settings: &crate::settings::ConnSettings) {
        let _ = settings;
    }
}

/// How [`ExternalBackend::delete`] deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    /// AWS recovery window (`days` 7..=30; `None` = the connection's `delete_recovery_days`) /
    /// Bitwarden trash (`days` ignored).
    Recoverable {
        /// AWS only: 7..=30.
        days: Option<u8>,
    },
    /// AWS `ForceDeleteWithoutRecovery` / Bitwarden permanent delete.
    Permanent,
}

/// What a delete did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DeleteOutcome {
    /// RFC 3339: until when [`ExternalBackend::restore`] can undo it; `None` = gone.
    pub recoverable_until: Option<String>,
}

/// §6.3 `connection_login`'s form. Values transit once and are never echoed.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum LoginForm {
    /// A personal API key (Account settings › Security › Keys).
    ApiKey {
        /// `user.<uuid>`.
        client_id: String,
        /// The secret half.
        client_secret: String,
    },
    /// Email + master password (+ 2FA), B5b.
    Password {
        /// The account.
        email: String,
        /// The master password.
        password: String,
        /// `[provider, code]` when the server asked for a second factor.
        two_factor: Option<(String, String)>,
        /// The new-device code when the server asked for one.
        new_device_otp: Option<String>,
    },
}

impl fmt::Debug for LoginForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApiKey { .. } => "LoginForm::ApiKey",
            Self::Password { .. } => "LoginForm::Password",
        })
    }
}

/// What a login step asks next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum LoginStep {
    /// Logged in.
    Done,
    /// The server wants a second factor from one of `providers`.
    NeedTwoFactor {
        /// `authenticator` | `email` | `yubikey`.
        providers: Vec<String>,
    },
    /// The server wants the new-device code it emailed.
    NeedNewDeviceOtp,
}

/// A parsed reference inside one connection: the part of `{{secret:<backend>/<conn>/<id>#<key>:-N}}`
/// after the backend.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExtRef {
    /// The connection name.
    pub conn: String,
    /// AWS: the secret name (may contain `/`). Bitwarden: the item UUID, lowercase.
    pub id: String,
    /// AWS: a top-level JSON key. Bitwarden: `password` | `username` | `totp` | `totp-seed` |
    /// `notes` | `uri` | `field:<name>`. `None` = the whole value / the item's default field.
    pub key: Option<String>,
    /// Versions back: 0 = current; AWS 1 = `AWSPREVIOUS`; Bitwarden `#password:-N` = history.
    pub back: u8,
}

/// A value to write. Never `Debug`: it is the secret.
#[derive(Clone)]
pub enum Value<'a> {
    /// One string.
    Plain(&'a str),
    /// AWS JSON key/value pairs; Bitwarden takes `Plain` only.
    Json(Vec<(&'a str, &'a str)>),
}

/// Where [`ExternalBackend::create`] puts a new entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "backend")]
pub enum Location {
    /// AWS Secrets Manager.
    Aws {
        /// The secret name (`app/ci/token`).
        name: String,
        /// `None` = the connection's default, else `aws/secretsmanager`.
        kms_key_id: Option<String>,
        /// The secret's description.
        description: Option<String>,
        /// Write `{"<json_key>": value}` instead of a plain string.
        json_key: Option<String>,
    },
    /// Bitwarden Password Manager.
    Bw {
        /// The organisation; `None` = the personal vault.
        org: Option<OrgId>,
        /// Organisation collections (org items only).
        collections: Vec<CollectionId>,
        /// The folder; `None` = no folder.
        folder: Option<FolderId>,
        /// Login or secure note.
        kind: BwKind,
        /// The item's name.
        item_name: String,
        /// Which field the value fills.
        field: BwField,
        /// Login username, from a non-masked card field.
        username: Option<String>,
        /// Login URI.
        uri: Option<String>,
    },
}

/// A Bitwarden organisation id (UUID text).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrgId(pub String);

/// A Bitwarden collection id (UUID text).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CollectionId(pub String);

/// A Bitwarden folder id (UUID text).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderId(pub String);

/// The Bitwarden item types theuth creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BwKind {
    /// A login item.
    Login,
    /// A secure note.
    SecureNote,
}

/// Which Bitwarden field a written value fills.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BwField {
    /// The login password.
    Password,
    /// The item's notes.
    Notes,
    /// The login TOTP seed.
    Totp,
    /// A custom field.
    Custom {
        /// The field's name.
        name: String,
        /// A hidden (masked) field.
        hidden: bool,
    },
}

/// One version of an external entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExtVersion {
    /// AWS: VersionId (UUID). Bitwarden: the cipher's revision_date (RFC 3339).
    pub id: String,
    /// AWS: VersionStages (AWSCURRENT, AWSPREVIOUS, custom). Bitwarden: empty.
    pub stages: Vec<String>,
    /// RFC 3339.
    pub changed: Option<String>,
}

/// One external entry's metadata. Never a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExtEntry {
    /// The placeholder body, `bw/personal/2f1c…` / `aws/prod/db/main`.
    pub r#ref: String,
    /// AWS name / Bitwarden item name.
    pub name: String,
    /// `string` | `json` | `binary` | `login` | `secure_note` | `card` | `identity` | `ssh_key`.
    pub kind: String,
    /// AWS JSON keys (names only, from a value fetch the user approved, else empty) /
    /// Bitwarden: `password`, `username`, `totp`, `notes`, `field:<name>`.
    pub keys: Vec<String>,
    /// AWS: region + KMS alias; Bitwarden: `org / collection / folder`.
    pub location: String,
    /// AWS: up to 5 by VersionIdsToStages; Bitwarden: one.
    pub versions: Vec<ExtVersion>,
    /// A deleted entry ([`ListQuery::deleted`]): RFC 3339 until when [`ExternalBackend::restore`]
    /// works (AWS purge date / Bitwarden trash). `None` for a live entry.
    #[serde(default)]
    pub recoverable_until: Option<String>,
}

/// [`ExternalBackend::list`]'s query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct ListQuery {
    /// Substring on name, folder, collection.
    pub query: Option<String>,
    /// From the previous page's [`ListPage::next`].
    pub cursor: Option<String>,
    /// List the deleted entries instead (AWS: pending deletion; Bitwarden: the trash), each with
    /// [`ExtEntry::recoverable_until`].
    pub deleted: bool,
}

/// One page of [`ExternalBackend::list`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ListPage {
    /// The entries, metadata only.
    pub entries: Vec<ExtEntry>,
    /// The cursor of the next page; `None` = last page.
    pub next: Option<String>,
}

/// A connection's state, as the UIs show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum ConnStatus {
    /// Usable now.
    Ready,
    /// Logged in, keys dropped: the unlock card brings it back.
    Locked,
    /// No credentials on this engine.
    LoggedOut,
    /// Credentials expired; `hint` says what the user runs (`aws sso login --profile …`).
    Expired {
        /// What to do.
        hint: String,
    },
    /// The last probe failed.
    Error(String),
    /// The server or build cannot serve this connection.
    Unsupported(String),
}

/// What a backend call fails with. `Display` is model-facing and never carries a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtError {
    /// No such connection on this engine.
    UnknownConn,
    /// Logged in but locked.
    Locked,
    /// No credentials.
    LoggedOut,
    /// AWS SSO; Bitwarden refresh token. The text is the hint.
    Expired(String),
    /// No such entry, key or field.
    NotFound,
    /// `create` over an existing entry.
    Exists,
    /// `update` / `delete` with a stale `expected`.
    Conflict {
        /// The version that is there now.
        current: ExtVersion,
    },
    /// IAM / org permission, or a connection setting that refuses it.
    Denied(String),
    /// Binary secret; card item; Vaultwarden endpoint.
    Unsupported(String),
    /// No answer within `secrets.external.timeout_secs`.
    Timeout,
    /// Transport failure.
    Network(String),
    /// The SDK's own error text (scrubbed before display).
    Sdk(String),
}

impl fmt::Display for ExtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownConn => f.write_str("unknown connection"),
            Self::Locked => f.write_str("locked"),
            Self::LoggedOut => f.write_str("logged out"),
            Self::Expired(hint) => write!(f, "credentials expired; {hint}"),
            Self::NotFound => f.write_str("not found"),
            Self::Exists => f.write_str("exists"),
            Self::Conflict { current } => write!(f, "changed since (now {})", current.id),
            Self::Denied(why) => write!(f, "denied: {why}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Timeout => f.write_str("did not answer in time"),
            Self::Network(e) => write!(f, "network error: {}", ::tools::secrets::redact(e)),
            Self::Sdk(e) => f.write_str(&::tools::secrets::redact(e)),
        }
    }
}

impl std::error::Error for ExtError {}

/// A connection name: `[a-z0-9-]{1,32}`.
pub fn valid_conn(conn: &str) -> bool {
    !conn.is_empty()
        && conn.len() <= 32
        && conn
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The registered connections, keyed by connection name (unique across backends, 221 §3.1).
/// Shared between the [`SecretsActor`] (which writes it) and the `tools::secrets` hook (which
/// reads it on every resolve). Also holds the open unlock cards (§9.4).
#[derive(Default)]
pub struct Externals {
    map: RwLock<BTreeMap<String, Arc<dyn ExternalBackend>>>,
    /// Per connection, the account the unlock card names (`me@… · vault.bitwarden.eu`).
    accounts: RwLock<BTreeMap<String, String>>,
    /// The bus the unlock card goes out on, and how long it waits; `None` = no card (a locked
    /// connection fails the call).
    unlock: Option<(runtime::Runtime, Duration)>,
    /// Unlock cards waiting for [`Externals::answer_unlock`], oldest first.
    pending: std::sync::Mutex<
        Vec<(
            UnlockRequestView,
            tokio::sync::oneshot::Sender<Option<Zeroizing<String>>>,
        )>,
    >,
    /// R17: per connection, one unlock card at a time; calls that waited behind it share its
    /// outcome.
    gates: std::sync::Mutex<BTreeMap<String, Arc<UnlockGate>>>,
}

/// One connection's unlock card serialisation (R17).
#[derive(Default)]
struct UnlockGate {
    /// Bumped when a card resolves.
    resolved: std::sync::atomic::AtomicU64,
    /// Held while a card is up; the last card's outcome.
    last: tokio::sync::Mutex<Option<Result<(), String>>>,
}

/// An open unlock card (§9.4): the `ExternalUnlockRequested` fields, for `list_unlock_requests`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UnlockRequestView {
    /// Answer with this id.
    pub id: crate::RequestId,
    /// The connection.
    pub conn: String,
    /// `aws` | `bw`.
    pub backend: String,
    /// What the card names: `me@… · vault.bitwarden.eu`.
    pub account: String,
    /// Why: `a tool call uses bw/personal/2f1c…#password`.
    pub reason: String,
}

impl fmt::Debug for Externals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_list()
            .entries(
                map.values()
                    .map(|b| format!("{}/{}", b.backend(), b.conn())),
            )
            .finish()
    }
}

impl Externals {
    /// A registry whose locked connections raise the unlock card on `rt` and wait `timeout`.
    pub fn new(rt: runtime::Runtime, timeout: Duration) -> Self {
        Self {
            unlock: Some((rt, timeout)),
            ..Self::default()
        }
    }

    /// The account the unlock card names for `conn`.
    pub fn set_account(&self, conn: &str, account: String) {
        let mut a = self
            .accounts
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        a.insert(conn.to_owned(), account);
    }

    /// §9.4: ask the user to unlock `b` on the unlock card and unlock it with the typed password.
    /// `Err` (model-facing): declined, timed out, no card possible, or the unlock failed. R17:
    /// one card per connection at a time; a call that waited behind a card takes its outcome.
    pub async fn request_unlock(
        &self,
        b: &Arc<dyn ExternalBackend>,
        reason: String,
    ) -> Result<(), String> {
        use std::sync::atomic::Ordering::SeqCst;
        let name = format!("{}/{}", b.backend(), b.conn());
        let Some((rt, timeout)) = &self.unlock else {
            return Err(format!("{name} is locked; unlock it in the Secrets page"));
        };
        let gate = self
            .gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(b.conn().to_owned())
            .or_default()
            .clone();
        let seen = gate.resolved.load(SeqCst);
        let mut last = gate.last.lock().await;
        if gate.resolved.load(SeqCst) != seen
            && let Some(out) = last.clone()
        {
            return out;
        }
        if matches!(b.status(), ConnStatus::Ready) {
            return Ok(());
        }
        let view = UnlockRequestView {
            id: crate::RequestId(uuid::Uuid::now_v7()),
            conn: b.conn().to_owned(),
            backend: b.backend().to_owned(),
            account: self
                .accounts
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(b.conn())
                .cloned()
                .unwrap_or_default(),
            reason,
        };
        let id = view.id;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((view.clone(), tx));
        rt.publish(crate::SecretsEvent::UnlockRequested {
            id,
            conn: view.conn,
            backend: view.backend,
            account: view.account,
            reason: view.reason,
        })
        .await;
        let answer = tokio::time::timeout(*timeout, rx).await;
        self.take_pending(id);
        let out = match answer {
            Ok(Ok(Some(password))) => match b.unlock(password).await {
                Ok(ConnStatus::Ready) => Ok(()),
                Ok(s) => Err(format!("{name} did not unlock ({s:?})")),
                Err(e) => Err(format!("{name}: unlock failed: {e}")),
            },
            Ok(_) => Err(format!("{name} is locked; the user declined to unlock it")),
            Err(_) => Err(format!(
                "{name} is locked; no unlock after {} s",
                timeout.as_secs()
            )),
        };
        *last = Some(out.clone());
        gate.resolved.fetch_add(1, SeqCst);
        drop(last);
        rt.publish(crate::SecretsEvent::UnlockResolved {
            id,
            unlocked: out.is_ok(),
        })
        .await;
        out
    }

    /// The unlock cards waiting for an answer, oldest first (`list_unlock_requests`).
    pub fn unlock_requests(&self) -> Vec<UnlockRequestView> {
        let p = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        p.iter().map(|(v, _)| v.clone()).collect()
    }

    /// Answer an unlock card: `Some(password)` unlocks, `None` declines. `false` when `id` is not
    /// waiting (answered, timed out, another engine's).
    pub fn answer_unlock(&self, id: crate::RequestId, password: Option<Zeroizing<String>>) -> bool {
        self.take_pending(id)
            .is_some_and(|tx| tx.send(password).is_ok())
    }

    /// Remove card `id` from the waiting list.
    fn take_pending(
        &self,
        id: crate::RequestId,
    ) -> Option<tokio::sync::oneshot::Sender<Option<Zeroizing<String>>>> {
        let mut p = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let i = p.iter().position(|(v, _)| v.id == id)?;
        Some(p.remove(i).1)
    }
}

impl Externals {
    /// The connection `conn` when it is a `backend` connection.
    pub fn get(&self, backend: &str, conn: &str) -> Option<Arc<dyn ExternalBackend>> {
        let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
        map.get(conn).filter(|b| b.backend() == backend).cloned()
    }

    /// The connection named `conn`, of whichever backend.
    pub fn by_conn(&self, conn: &str) -> Option<Arc<dyn ExternalBackend>> {
        let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
        map.get(conn).cloned()
    }

    /// Every registered connection, by name.
    pub fn all(&self) -> Vec<Arc<dyn ExternalBackend>> {
        let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
        map.values().cloned().collect()
    }

    /// Whether `backend` is a known id: one of [`BACKENDS`] or the id of a registered connection.
    pub fn knows_backend(&self, backend: &str) -> bool {
        BACKENDS.contains(&backend) || {
            let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
            map.values().any(|b| b.backend() == backend)
        }
    }

    /// Add or replace a connection. `Err` = an invalid name, or the name is another backend's.
    pub fn insert(&self, b: Arc<dyn ExternalBackend>) -> Result<(), String> {
        let conn = b.conn().to_owned();
        if !valid_conn(&conn) {
            return Err(format!(
                "invalid connection name {conn:?}: [a-z0-9-], 1 to 32 characters"
            ));
        }
        let mut map = self.map.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(old) = map.get(&conn)
            && old.backend() != b.backend()
        {
            return Err(format!(
                "connection name {conn} is already used by {}/{conn}",
                old.backend()
            ));
        }
        map.insert(conn, b);
        Ok(())
    }

    /// Drop a connection; the backend when it was there.
    pub fn remove(&self, backend: &str, conn: &str) -> Option<Arc<dyn ExternalBackend>> {
        let mut map = self.map.write().unwrap_or_else(PoisonError::into_inner);
        if map.get(conn).is_some_and(|b| b.backend() == backend) {
            map.remove(conn)
        } else {
            None
        }
    }
}

/// `ask` → `Result<(), String>`: serve this connection from now on (221 §4). Replaces an earlier
/// registration of the same `backend/conn`. `Err` = invalid name, or the name belongs to a
/// connection of another backend.
#[derive(Clone)]
pub struct RegisterExternal(pub Arc<dyn ExternalBackend>);

/// `ask` → `bool`: stop serving `backend/conn` (`true` when it was registered). References to it
/// then fail with `unknown connection`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnregisterExternal {
    /// `aws` | `bw`.
    pub backend: String,
    /// The connection name.
    pub conn: String,
}

impl Message<RegisterExternal> for SecretsActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RegisterExternal,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.externals.insert(m.0)
    }
}

impl Message<UnregisterExternal> for SecretsActor {
    type Reply = bool;

    async fn handle(
        &mut self,
        m: UnregisterExternal,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Some(b) = self.externals.remove(&m.backend, &m.conn) else {
            return false;
        };
        // Nobody can reach it any more: drop its keys and cached credentials.
        tokio::spawn(async move { b.lock().await });
        true
    }
}

/// Split a placeholder's external body (`CONN/REF` after `<backend>/`) into an [`ExtRef`].
/// `Err` = no `/REF`, or an invalid connection name.
pub fn parse_ref(backend: &str, rest: &str, key: Option<&str>, back: u8) -> Result<ExtRef, String> {
    let Some((conn, id)) = rest.split_once('/').filter(|(_, id)| !id.is_empty()) else {
        return Err(format!(
            "{backend}/{rest}: an external reference is {backend}/CONNECTION/NAME"
        ));
    };
    if !valid_conn(conn) {
        return Err(format!(
            "{backend}/{rest}: {conn:?} is not a connection name ([a-z0-9-], 1 to 32 characters)"
        ));
    }
    Ok(ExtRef {
        conn: conn.to_owned(),
        id: id.to_owned(),
        key: key.map(str::to_owned),
        back,
    })
}

/// An external reference as data (`aws/prod/app/db#password`, no braces): the `ref` / `source`
/// argument of the `secret_external_*` and `secret_send_*` tools. `None` when it is not one.
pub fn parse_bare(text: &str) -> Option<(&str, ExtRef)> {
    let (backend, rest) = text.split_once('/')?;
    let (rest, key) = match rest.split_once('#') {
        Some((r, k)) => (r, Some(k)),
        None => (rest, None),
    };
    parse_ref(backend, rest, key, 0).ok().map(|r| (backend, r))
}

/// One external placeholder in a call's args.
struct Found {
    /// The whole `{{secret:…}}` text: the [`Fetched`] key and the scrub placeholder.
    text: String,
    backend: String,
    r: Result<ExtRef, String>,
}

/// Every distinct external placeholder in the string leaves of `args`, in order.
fn external_placeholders(args: &serde_json::Value) -> Vec<Found> {
    fn walk(v: &serde_json::Value, seen: &mut HashSet<String>, out: &mut Vec<Found>) {
        match v {
            serde_json::Value::String(s) if s.contains("{{secret:") => {
                for c in placeholder_re().captures_iter(s) {
                    let (Some(backend), Some(rest)) = (c.get(1), c.get(2)) else {
                        continue;
                    };
                    if !seen.insert(c[0].to_owned()) {
                        continue;
                    }
                    let back = c.get(5).map_or(0, |m| m.as_str().parse().unwrap_or(0));
                    out.push(Found {
                        text: c[0].to_owned(),
                        backend: backend.as_str().to_owned(),
                        r: parse_ref(
                            backend.as_str(),
                            rest.as_str(),
                            c.get(3).map(|m| m.as_str()),
                            back,
                        ),
                    });
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, seen, out)),
            serde_json::Value::Object(o) => o.values().for_each(|v| walk(v, seen, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(args, &mut HashSet::new(), &mut out);
    out
}

/// The `tools::secrets` hook (37 §3.4, 221 §3.3): the vault for local placeholders and the
/// registered connections for external ones.
pub struct Hook {
    vault: Arc<Vault>,
    externals: Arc<Externals>,
    /// `secrets.external.timeout_secs`: the bound on one backend call.
    timeout: Duration,
}

impl Hook {
    /// The hook over `vault` and `externals`; every backend call is bounded by `timeout`.
    pub fn new(vault: Arc<Vault>, externals: Arc<Externals>, timeout: Duration) -> Self {
        Self {
            vault,
            externals,
            timeout,
        }
    }

    /// The access of `conn` in `dir`, when `conn` is a registered connection.
    fn conn_access(&self, conn: &str, dir: Direction) -> Option<ExternalAccess> {
        let b = self.externals.by_conn(conn)?;
        Some(ExternalAccess {
            backend: b.backend(),
            conn: conn.to_owned(),
            dir,
        })
    }

    /// The access of a bare reference (`aws/prod/x`) in `dir`, when its connection is registered.
    fn ref_access(&self, text: &str, dir: Direction) -> Option<ExternalAccess> {
        let (backend, r) = parse_bare(text)?;
        let b = self.externals.get(backend, &r.conn)?;
        Some(ExternalAccess {
            backend: b.backend(),
            conn: r.conn,
            dir,
        })
    }

    /// [`::tools::secrets::Secrets::resolve_external`]: fetch every external placeholder in
    /// `args`, each once; the first failure fails the call.
    async fn resolve(&self, args: &serde_json::Value) -> Result<Fetched, String> {
        let found = external_placeholders(args);
        let mut fetched = Fetched::default();
        for f in found {
            if !self.externals.knows_backend(&f.backend) {
                return Err(format!("unknown secret backend {}", f.backend));
            }
            let r = f.r?;
            let Some(b) = self.externals.get(&f.backend, &r.conn) else {
                return Err(format!(
                    "unknown connection {}/{} on this engine; secret_external_list shows the connections",
                    f.backend, r.conn
                ));
            };
            let body = &f.text["{{secret:".len()..f.text.len() - "}}".len()];
            match b.status() {
                ConnStatus::Locked => {
                    let reason = format!("a tool call uses {body}");
                    self.externals.request_unlock(&b, reason).await?;
                }
                ConnStatus::LoggedOut => {
                    return Err(format!(
                        "{}/{} is logged out on this engine; the user logs in from the Secrets page",
                        f.backend, r.conn
                    ));
                }
                _ => {}
            }
            let value = match tokio::time::timeout(self.timeout, b.get(&r)).await {
                Ok(Ok((value, _))) => value,
                Ok(Err(e)) => return Err(format!("{body}: {e}")),
                Err(_) => {
                    return Err(format!(
                        "{body}: {} did not answer in {} s",
                        f.backend,
                        self.timeout.as_secs()
                    ));
                }
            };
            self.vault.remember_external(&f.text, &value);
            fetched.insert(f.text, value);
        }
        Ok(fetched)
    }
}

impl ::tools::secrets::Secrets for Hook {
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        self.vault.expand(args)
    }

    fn expand_with(
        &self,
        args: &mut serde_json::Value,
        fetched: &Fetched,
    ) -> Result<usize, String> {
        self.vault.expand_with(args, fetched)
    }

    /// By tool name for the tools that take references as data (221 §7, §12.3); otherwise every
    /// external placeholder is a read. A reference to a connection this engine does not have
    /// gates nothing: the call fails at resolve, or in the tool.
    fn external_access(&self, tool: &str, args: &serde_json::Value) -> Vec<ExternalAccess> {
        let arg = |k: &str| args.get(k).and_then(serde_json::Value::as_str);
        let by_ref = |k: &str, dir| arg(k).and_then(|t| self.ref_access(t, dir));
        let by_conn = |k: &str, dir| arg(k).and_then(|c| self.conn_access(c, dir));
        let out: Vec<Option<ExternalAccess>> = match tool {
            // R16: without `connection`, `secret_external_list` names the connections, their
            // status and account summaries (emails, regions, servers) from config, and contacts
            // no manager, so it gates nothing; with one, the connection's read ask applies.
            "secret_external_list" | "secret_send_list" => {
                vec![by_conn("connection", Direction::Read)]
            }
            "secret_external_describe" | "secret_external_import" => {
                vec![by_ref("ref", Direction::Read)]
            }
            "secret_external_create" => vec![by_conn("connection", Direction::Write)],
            "secret_external_update" => vec![by_ref("ref", Direction::Write)],
            "secret_external_delete" => vec![by_ref("ref", Direction::Delete)],
            "secret_request" | "secret_request_form" => vec![
                args.get("destination")
                    .and_then(|d| d.get("connection"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|c| self.conn_access(c, Direction::Write)),
            ],
            "secret_send_create" => vec![
                by_conn("connection", Direction::Send),
                by_ref("source", Direction::Read),
                by_ref("file", Direction::Read),
            ],
            _ => external_placeholders(args)
                .into_iter()
                .filter_map(|f| {
                    let r = f.r.ok()?;
                    let b = self.externals.get(&f.backend, &r.conn)?;
                    Some(Some(ExternalAccess {
                        backend: b.backend(),
                        conn: r.conn,
                        dir: Direction::Read,
                    }))
                })
                .collect(),
        };
        let mut out: Vec<ExternalAccess> = out.into_iter().flatten().collect();
        out.dedup();
        out
    }

    fn resolve_external<'a>(
        &'a self,
        args: &'a serde_json::Value,
        _cx: ResolveCx,
    ) -> BoxFuture<'a, Result<Fetched, String>> {
        Box::pin(self.resolve(args))
    }

    fn redact<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        self.vault.redact(text)
    }

    fn redact_value(&self, v: &mut serde_json::Value) {
        self.vault.redact_value(v);
    }

    fn redact_spans(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        self.vault.redact_spans(text)
    }

    fn max_literal_len(&self) -> usize {
        self.vault.max_literal_len()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ::tools::secrets::Secrets as _;
    use serde_json::json;

    use super::*;
    use crate::KeyStore;

    /// An in-memory backend `fake/<conn>`: `get` of id `x` answers [`VALUE`] (`#k` → `k=VALUE`),
    /// anything else `NotFound`; every other method is unsupported.
    pub(crate) struct Fake(pub(crate) &'static str);

    pub(crate) const VALUE: &str = "s3cret-from-fake";

    #[async_trait::async_trait]
    impl ExternalBackend for Fake {
        fn backend(&self) -> &'static str {
            "fake"
        }
        fn conn(&self) -> &str {
            self.0
        }
        fn status(&self) -> ConnStatus {
            ConnStatus::Ready
        }
        async fn probe(&self) -> Result<ConnStatus, ExtError> {
            Ok(ConnStatus::Ready)
        }
        async fn list(&self, _: ListQuery) -> Result<ListPage, ExtError> {
            Ok(ListPage::default())
        }
        async fn describe(&self, _: &ExtRef) -> Result<ExtEntry, ExtError> {
            Err(ExtError::Unsupported("describe".into()))
        }
        async fn get(&self, r: &ExtRef) -> Result<(Zeroizing<String>, ExtVersion), ExtError> {
            if r.id != "x" {
                return Err(ExtError::NotFound);
            }
            let v = match &r.key {
                Some(k) => format!("{k}={VALUE}"),
                None => VALUE.to_owned(),
            };
            Ok((
                Zeroizing::new(v),
                ExtVersion {
                    id: "1".into(),
                    stages: Vec::new(),
                    changed: None,
                },
            ))
        }
        async fn create(
            &self,
            _: &Location,
            _: Value<'_>,
        ) -> Result<(ExtRef, ExtVersion), ExtError> {
            Err(ExtError::Unsupported("create".into()))
        }
        async fn update(
            &self,
            _: &ExtRef,
            _: Value<'_>,
            _: Option<&ExtVersion>,
        ) -> Result<ExtVersion, ExtError> {
            Err(ExtError::Unsupported("update".into()))
        }
        async fn delete(
            &self,
            _: &ExtRef,
            _: DeleteMode,
            _: Option<&ExtVersion>,
        ) -> Result<DeleteOutcome, ExtError> {
            Err(ExtError::Unsupported("delete".into()))
        }
        async fn restore(&self, _: &ExtRef) -> Result<ExtVersion, ExtError> {
            Err(ExtError::Unsupported("restore".into()))
        }
        async fn lock(&self) {}
    }

    async fn hook(dir: &std::path::Path) -> (Hook, Arc<Vault>) {
        let vault = Arc::new(Vault::open(dir, KeyStore::File).await.unwrap());
        let ext = Arc::new(Externals::default());
        ext.insert(Arc::new(Fake("c"))).unwrap();
        (Hook::new(vault.clone(), ext, Duration::from_secs(5)), vault)
    }

    fn cx() -> ResolveCx {
        ResolveCx {
            session: runtime::SessionId::new(),
            call_id: "1".into(),
        }
    }

    /// 221 §13 B2 (a): an external placeholder resolves, expands, and its value is scrubbed
    /// back to the placeholder in output; a local one beside it still expands.
    #[tokio::test]
    async fn external_placeholder_expands_and_scrubs_back() {
        let dir = tempfile::tempdir().unwrap();
        let (hook, vault) = hook(dir.path()).await;
        vault
            .set("LOCAL", "local-value-long", None, "user")
            .await
            .unwrap();
        let mut args =
            json!({"cmd": "x {{secret:fake/c/x}} {{secret:fake/c/x#k}} {{secret:LOCAL}}"});
        let fetched = hook.resolve_external(&args, cx()).await.unwrap();
        assert_eq!(fetched.len(), 2);
        assert_eq!(hook.expand_with(&mut args, &fetched).unwrap(), 3);
        assert_eq!(args["cmd"], format!("x {VALUE} k={VALUE} local-value-long"));
        drop(fetched);
        assert_eq!(
            hook.redact(&format!("out {VALUE} and k={VALUE}")),
            "out {{secret:fake/c/x}} and {{secret:fake/c/x#k}}"
        );
        let mut b64 =
            json!({"o": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, VALUE)});
        hook.redact_value(&mut b64);
        assert_eq!(b64["o"], "{{secret:fake/c/x}}");
    }

    /// R22: `#field:<name>` (a name with spaces, and with `:-N`) reaches the backend as the key
    /// `field:<name>`; the Bitwarden backend maps it to that custom field.
    #[tokio::test]
    async fn field_reference_reaches_the_backend_whole() {
        let dir = tempfile::tempdir().unwrap();
        let (hook, _vault) = hook(dir.path()).await;
        let mut args = json!({"cmd": "{{secret:fake/c/x#field:API token}}"});
        let fetched = hook.resolve_external(&args, cx()).await.unwrap();
        hook.expand_with(&mut args, &fetched).unwrap();
        assert_eq!(args["cmd"], format!("field:API token={VALUE}"));
        let c = placeholder_re()
            .captures("{{secret:bw/conn/Item#field:API token:-1}}")
            .unwrap();
        assert_eq!(
            (&c[1], &c[2], &c[3], &c[5]),
            ("bw", "conn/Item", "field:API token", "1")
        );
    }

    /// 221 §13 B2 (b): an unknown connection, an unknown backend or a missing entry fails the
    /// whole call; without a resolve, an external placeholder never expands.
    #[tokio::test]
    async fn unknown_connection_or_backend_fails_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let (hook, vault) = hook(dir.path()).await;
        let fail = |s: &str| json!({"cmd": format!("ok {{{{secret:fake/c/x}}}} {s}")});
        let e = hook
            .resolve_external(&fail("{{secret:fake/nope/x}}"), cx())
            .await
            .unwrap_err();
        assert_eq!(
            e,
            "unknown connection fake/nope on this engine; secret_external_list shows the connections"
        );
        let e = hook
            .resolve_external(&fail("{{secret:zz/c/x}}"), cx())
            .await
            .unwrap_err();
        assert_eq!(e, "unknown secret backend zz");
        let e = hook
            .resolve_external(&fail("{{secret:oauth/github}}"), cx())
            .await
            .unwrap_err();
        assert_eq!(e, "unknown secret backend oauth");
        let e = hook
            .resolve_external(&fail("{{secret:fake/c/y}}"), cx())
            .await
            .unwrap_err();
        assert_eq!(e, "fake/c/y: not found");
        let mut args = json!({"cmd": "{{secret:fake/c/x}}"});
        let e = vault.expand(&mut args).unwrap_err();
        assert!(e.contains("was not resolved"), "{e}");
    }

    #[test]
    fn grammar_splits_conn_ref_key_and_back() {
        let caps = |s: &str| {
            let c = placeholder_re().captures(s).unwrap();
            let g = |i| c.get(i).map(|m| m.as_str().to_owned());
            (g(1), g(2), g(3), g(4), g(5))
        };
        assert_eq!(
            caps("{{secret:aws/prod/app/db/main#password:-1}}"),
            (
                Some("aws".into()),
                Some("prod/app/db/main".into()),
                Some("password".into()),
                None,
                Some("1".into())
            )
        );
        assert_eq!(
            caps("{{secret:bw/personal/2f1c#field:recovery code}}"),
            (
                Some("bw".into()),
                Some("personal/2f1c".into()),
                Some("field:recovery code".into()),
                None,
                None
            )
        );
        assert_eq!(
            caps("{{secret:GITHUB:-2}}"),
            (None, None, None, Some("GITHUB".into()), Some("2".into()))
        );
        assert!(parse_ref("aws", "prod", None, 0).is_err());
        assert!(parse_ref("aws", "Prod/x", None, 0).is_err());
        assert_eq!(
            parse_bare("aws/prod/a/b#k").map(|(b, r)| (b.to_owned(), r)),
            Some((
                "aws".to_owned(),
                ExtRef {
                    conn: "prod".into(),
                    id: "a/b".into(),
                    key: Some("k".into()),
                    back: 0
                }
            ))
        );
    }

    #[tokio::test]
    async fn access_is_parsed_from_placeholders_and_tool_args() {
        let dir = tempfile::tempdir().unwrap();
        let (hook, _vault) = hook(dir.path()).await;
        let acc = |tool: &str, args| {
            hook.external_access(tool, &args)
                .into_iter()
                .map(|a| a.always_ask_key())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            acc(
                "shell",
                json!({"c": "{{secret:fake/c/x}} {{secret:fake/c/x}} {{secret:fake/nope/x}}"})
            ),
            ["secret:fake/c:read"]
        );
        assert!(acc("shell", json!({"c": "{{secret:LOCAL}}"})).is_empty());
        assert_eq!(
            acc(
                "secret_external_delete",
                json!({"ref": "fake/c/x", "expected": "1"})
            ),
            ["secret:fake/c:delete"]
        );
        assert_eq!(
            acc("secret_external_create", json!({"connection": "c"})),
            ["secret:fake/c:write"]
        );
        assert!(acc("secret_external_list", json!({})).is_empty());
    }
}
