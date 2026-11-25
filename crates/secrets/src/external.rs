//! External secret backends (221 §4): AWS Secrets Manager, Bitwarden, and whatever fits the same
//! shape. A backend crate implements [`ExternalBackend`] for one connection and registers it on
//! the [`crate::SecretsActor`] with [`RegisterExternal`]; the `tools::secrets` hook then resolves
//! `{{secret:<backend>/<conn>/<ref>[#<key>]}}` through it, after the approval and before expansion.
//!
//! No SDK type crosses this module. Every `Display` string here is model-facing and never carries
//! a value; [`ExtError::Sdk`] text is scrubbed with `tools::secrets::redact` before it is shown.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::SecretsActor;

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
}

/// How [`ExternalBackend::delete`] deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteOutcome {
    /// RFC 3339: until when [`ExternalBackend::restore`] can undo it; `None` = gone.
    pub recoverable_until: Option<String>,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtVersion {
    /// AWS: VersionId (UUID). Bitwarden: the cipher's revision_date (RFC 3339).
    pub id: String,
    /// AWS: VersionStages (AWSCURRENT, AWSPREVIOUS, custom). Bitwarden: empty.
    pub stages: Vec<String>,
    /// RFC 3339.
    pub changed: Option<String>,
}

/// One external entry's metadata. Never a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// [`ExternalBackend::list`]'s query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    /// Substring on name, folder, collection.
    pub query: Option<String>,
    /// From the previous page's [`ListPage::next`].
    pub cursor: Option<String>,
}

/// One page of [`ExternalBackend::list`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPage {
    /// The entries, metadata only.
    pub entries: Vec<ExtEntry>,
    /// The cursor of the next page; `None` = last page.
    pub next: Option<String>,
}

/// A connection's state, as the UIs show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
/// reads it on every resolve).
#[derive(Default)]
pub struct Externals {
    map: RwLock<BTreeMap<String, Arc<dyn ExternalBackend>>>,
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
    /// The connection `conn` when it is a `backend` connection.
    pub fn get(&self, backend: &str, conn: &str) -> Option<Arc<dyn ExternalBackend>> {
        let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
        map.get(conn).filter(|b| b.backend() == backend).cloned()
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
