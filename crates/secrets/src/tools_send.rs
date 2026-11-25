//! 221 §12: Bitwarden Sends. `secret_send_create` shares one secret (a local vault secret, a
//! `bw/` field or an `aws/` secret) or one file (an absolute path, an artifact of the calling
//! session's tree, a Bitwarden attachment) through a Send; `secret_send_list` and
//! `secret_send_revoke` follow them up. The content is resolved and encrypted inside the harness:
//! it never reaches the model, the args or an event. The link is a stand-in (D23): stored as a
//! local vault secret (source `secret_send_create`, default name `send-<conn>-<8 hex>`) whose
//! placeholder the agent gets, or, with `send_link_to_agent`, handed over in the receipt.
//!
//! The Send itself is made by the connection's [`SendBackend`] (`secrets-bitwarden`); no SDK type
//! crosses this module.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{PreviewCx, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};
use zeroize::Zeroizing;

use crate::connections::Audit;
use crate::external::{ExtError, ExtRef};
use crate::tools_external::ExtCx;
use crate::{Request, SecretRequestField};

/// The vault `source` of a Send link or password secret (b221-core's refusal logic keys on it).
pub const SEND_SOURCE: &str = "secret_send_create";
/// Default text shown to the recipient.
pub const DEFAULT_NAME: &str = "Shared from theuth";
/// The longest a Send may live.
pub const MAX_DAYS: i64 = 31;

/// What a Send carries.
pub enum SendContent {
    /// Text, hidden-by-default for the recipient.
    Text(Zeroizing<String>),
    /// A file: the name the recipient sees, and the bytes.
    File {
        /// File name shown to and downloaded by the recipient.
        name: String,
        /// The plaintext; dropped as soon as it is encrypted.
        bytes: Zeroizing<Vec<u8>>,
    },
}

/// A Send's options.
pub struct SendOptions {
    /// Shown to the recipient.
    pub name: String,
    /// Private notes (the user's own view in Bitwarden).
    pub notes: Option<String>,
    /// When the link stops working.
    pub expiration: Option<DateTime<Utc>>,
    /// When Bitwarden deletes it (≤ 31 days).
    pub deletion: DateTime<Utc>,
    /// How many times it may be opened.
    pub max_access_count: Option<u32>,
    /// Access password.
    pub password: Option<Zeroizing<String>>,
    /// Hide the sender's email.
    pub hide_email: bool,
}

/// A created Send. `link` carries the decryption key in its fragment.
pub struct CreatedSend {
    /// Bitwarden's Send id.
    pub send_id: String,
    /// The public access id in the link.
    pub access_id: String,
    /// `<web vault>/#/send/<access_id>/<url-b64 key>`.
    pub link: Zeroizing<String>,
    /// RFC 3339.
    pub deletion_date: String,
    /// RFC 3339.
    pub expiration_date: Option<String>,
}

/// A Send's metadata after a sync. Never the link or the content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendInfo {
    /// Bitwarden's Send id.
    pub send_id: String,
    /// The Send's name.
    pub name: String,
    /// `text` | `file`.
    pub kind: String,
    /// File name of a file Send.
    pub file_name: Option<String>,
    /// Times opened.
    pub access_count: u32,
    /// Opening limit.
    pub max_access_count: Option<u32>,
    /// Has an access password.
    pub has_password: bool,
    /// RFC 3339.
    pub deletion_date: String,
    /// RFC 3339.
    pub expiration_date: Option<String>,
    /// Disabled by its owner.
    pub disabled: bool,
}

/// A connection that can make Sends (Bitwarden). Reached through
/// [`ExternalBackend::sender`].
#[async_trait::async_trait]
pub trait SendBackend: Send + Sync {
    /// Create a Send; a file Send whose upload fails is deleted again.
    async fn create_send(
        &self,
        content: SendContent,
        opts: SendOptions,
    ) -> Result<CreatedSend, ExtError>;
    /// Delete (revoke) a Send.
    async fn delete_send(&self, send_id: &str) -> Result<(), ExtError>;
    /// The account's Sends, synced.
    async fn list_sends(&self) -> Result<Vec<SendInfo>, ExtError>;
    /// An attachment's file name and recorded size (`which` = id or file name).
    async fn attachment_info(
        &self,
        r: &ExtRef,
        which: &str,
    ) -> Result<(String, Option<u64>), ExtError>;
    /// An attachment's bytes, refused before the download above `max_bytes`.
    async fn attachment_bytes(
        &self,
        r: &ExtRef,
        which: &str,
        max_bytes: u64,
    ) -> Result<(String, Zeroizing<Vec<u8>>), ExtError>;
}

/// Everything the Send tools need.
#[derive(Clone)]
pub struct SendCx {
    /// Connections, vault, settings.
    pub ext: ExtCx,
    /// `send_owner_only` (D17) as the gate reads it: on when any Bitwarden connection has it on.
    /// The registrar keeps it current ([`SendCx::set_owner_only`]).
    pub owner_only: Arc<AtomicBool>,
    /// `secret_send_create` acts on what its card showed (210 §7.3): the preview's plan per
    /// `(session, call_id)`.
    plans: Arc<Mutex<HashMap<(SessionId, String), Plan>>>,
    /// `secrets.request_timeout_secs`, for the password card.
    pub request_timeout: Duration,
    /// `artifact:<id>` lookups; `None` = artifact sources are refused.
    pub artifacts: Option<Arc<dyn ArtifactSource>>,
}

impl SendCx {
    /// A context; `owner_only` starts on (the default).
    pub fn new(
        ext: ExtCx,
        request_timeout: Duration,
        artifacts: Option<Arc<dyn ArtifactSource>>,
    ) -> Self {
        Self {
            ext,
            owner_only: Arc::new(AtomicBool::new(true)),
            plans: Arc::default(),
            request_timeout,
            artifacts,
        }
    }

    /// Update `send_owner_only` (any Bitwarden connection with it on → on).
    pub fn set_owner_only(&self, on: bool) {
        self.owner_only.store(on, Ordering::Relaxed);
    }

    fn pool(&self) -> &sqlx::SqlitePool {
        self.ext.vault.pool()
    }

    async fn audit(&self, line: String) {
        let _ = self.ext.actor.tell(Audit(line)).await;
    }

    /// Harness-wide external settings (`send_max_file_mb`, …).
    async fn ext_settings(&self) -> crate::settings::ExternalSettings {
        self.ext
            .actor
            .ask(crate::connections::GetExternalSettings)
            .await
            .unwrap_or_default()
    }

    /// `conn`'s D20 settings.
    async fn conn_settings(&self, conn: &str) -> Result<crate::settings::ConnSettings, String> {
        self.ext
            .actor
            .ask(crate::connections::ConnSettingsOf(conn.to_owned()))
            .await
            .map_err(|e| e.to_string())
    }

    /// The Bitwarden connection `conn`, unlocked, as a [`SendBackend`].
    async fn sender(&self, conn: &str) -> Result<Arc<dyn SendBackend>, String> {
        let b = self.ext.conn(conn, "create a Send").await?;
        if b.backend() != "bw" {
            return Err(format!(
                "{conn} is not a Bitwarden connection; Sends are Bitwarden's"
            ));
        }
        b.sender()
            .ok_or_else(|| format!("bw/{conn} cannot make Sends in this build"))
    }
}

/// Register the three tools.
pub async fn register(reg: &ActorRef<ToolRegistry>, cx: SendCx) {
    let tools: [Arc<dyn tools::ErasedTool>; 3] = [
        Arc::new(SendCreateTool(cx.clone())),
        Arc::new(SendListTool(cx.clone())),
        Arc::new(SendRevokeTool(cx)),
    ];
    for t in tools {
        let _ = reg.ask(RegisterTool(t)).await;
    }
}

/// Where the access password comes from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum SendPassword {
    /// A local vault secret's current value.
    From {
        /// Vault name.
        from: String,
    },
    /// Ask the user on a masked card after the approval.
    Ask {
        /// `true`.
        ask: bool,
    },
}

/// `secret_send_create`: share one secret or one file through a Bitwarden Send. You never see the
/// content or the link.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct SendCreateArgs {
    /// Bitwarden connection the Send is created on (`personal`).
    pub connection: String,
    /// Text content: a local vault NAME (current version), `bw/CONN/ITEM#field` or
    /// `aws/CONN/NAME[#key]`. Exactly one of `source` / `file`. Never a placeholder, never a
    /// literal value.
    #[serde(default)]
    pub source: Option<String>,
    /// File content: an absolute path on this engine (`/Users/me/keys/prod.pem`), `artifact:<id>`
    /// (an artifact of this session's tree), or `bw/CONN/ITEM#attachment:<name or id>`.
    #[serde(default)]
    pub file: Option<String>,
    /// Send a text `source` as a file instead (for example a `.env`), with this file name.
    #[serde(default)]
    pub as_file: Option<String>,
    /// File name the recipient sees and downloads. Default: the source's own name.
    #[serde(default)]
    pub file_name: Option<String>,
    /// Shown to the recipient on the Send page. Default "Shared from theuth". Never the secret.
    #[serde(default)]
    pub name: Option<String>,
    /// When the link stops working: `30m`, `2h`, `3d`, or an ISO-8601 instant. Default `24h`.
    #[serde(default)]
    pub expires: Option<String>,
    /// When Bitwarden deletes it; ≥ `expires`, ≤ 31 days. Default = `expires`.
    #[serde(default)]
    pub delete: Option<String>,
    /// How many times it may be opened. Default 1.
    #[serde(default)]
    pub max_access_count: Option<u32>,
    /// Access password: `{"from": "VAULT_NAME"}` or `{"ask": true}`. Default none.
    #[serde(default)]
    pub password: Option<SendPassword>,
    /// Hide the sender's email from the recipient. Default true.
    #[serde(default)]
    pub hide_email: Option<bool>,
    /// Private notes on the Send (visible to the user in Bitwarden, not to the recipient).
    #[serde(default)]
    pub notes: Option<String>,
    /// Local vault name for the link. Default `send-<connection>-<8 hex>`.
    #[serde(default)]
    pub link_name: Option<String>,
}

/// Where the content comes from, resolved to everything but the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// A local vault secret.
    Local { name: String, version: u32 },
    /// An external reference (`bw/…`, `aws/…`), its version at preview time.
    External { text: String, version: String },
    /// An absolute path, its size and mtime.
    Path {
        path: String,
        bytes: u64,
        mtime_ms: u64,
    },
    /// An artifact.
    Artifact {
        id: String,
        path: String,
        name: String,
        bytes: u64,
    },
    /// A Bitwarden attachment.
    Attachment {
        text: String,
        which: String,
        name: String,
        bytes: Option<u64>,
    },
}

impl Source {
    fn label(&self) -> String {
        match self {
            Self::Local { name, version } => format!("local {name} v{version}"),
            Self::External { text, version } => format!("{text}, rev {version}"),
            Self::Path { path, bytes, .. } => format!("{} · from {path}", size(*bytes)),
            Self::Artifact {
                name, bytes, id, ..
            } => {
                format!("artifact {name} ({id}) · {}", size(*bytes))
            }
            Self::Attachment {
                text, name, bytes, ..
            } => format!(
                "{text} attachment {name}{}",
                bytes.map(|b| format!(" · {}", size(b))).unwrap_or_default()
            ),
        }
    }

    fn version(&self) -> String {
        match self {
            Self::Local { version, .. } => format!("v{version}"),
            Self::External { version, .. } => version.clone(),
            Self::Path { mtime_ms, .. } => format!("mtime {mtime_ms}"),
            Self::Artifact { id, .. } => id.clone(),
            Self::Attachment { which, .. } => which.clone(),
        }
    }

    fn is_file(&self) -> bool {
        matches!(
            self,
            Self::Path { .. } | Self::Artifact { .. } | Self::Attachment { .. }
        )
    }
}

/// What the card showed and the call does.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    conn: String,
    source: Source,
    source_text: String,
    /// File name the recipient sees (file Sends and `as_file`).
    file_name: Option<String>,
    name: String,
    expiration: DateTime<Utc>,
    deletion: DateTime<Utc>,
    max_access: u32,
    password: Option<SendPassword>,
    hide_email: bool,
    notes: Option<String>,
    link_name: String,
    link_to_agent: bool,
    max_bytes: u64,
}

fn size(b: u64) -> String {
    match b {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / (1 << 10) as f64),
        b => format!("{b} B"),
    }
}

fn local_time(t: DateTime<Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// `30m` / `2h` / `3d` / an ISO-8601 instant, from `now`.
fn when(text: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let now_ms = now.timestamp_millis().max(0) as u64;
    let at = crate::expiry::parse(text, now_ms)?.at(now_ms);
    DateTime::from_timestamp_millis(at as i64).ok_or_else(|| format!("{text}: out of range"))
}

/// `send-<conn>-<8 hex>`.
fn default_link_name(conn: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("send-{conn}-{}", &id[..8])
}

/// The parts of a plan that need no I/O.
fn plan_dates(
    a: &SendCreateArgs,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let expiration = when(a.expires.as_deref().unwrap_or("24h"), now)?;
    let deletion = match &a.delete {
        Some(d) => when(d, now)?,
        None => expiration,
    };
    if deletion < expiration {
        return Err("delete must not come before expires".into());
    }
    if deletion > now + chrono::Duration::days(MAX_DAYS) {
        return Err("delete must be within 31 days".into());
    }
    Ok((expiration, deletion))
}

fn check_shape(a: &SendCreateArgs) -> Result<(), String> {
    match (&a.source, &a.file) {
        (Some(_), Some(_)) | (None, None) => {
            Err("give exactly one of source (text) or file".into())
        }
        (None, Some(_)) if a.as_file.is_some() => {
            Err("as_file turns a text source into a file; with file it is not needed".into())
        }
        _ if a.source.as_deref().is_some_and(|s| s.contains("{{")) => {
            Err("source is a reference (a vault NAME, bw/… or aws/…), not a placeholder".into())
        }
        _ => Ok(()),
    }
}

impl SendCx {
    /// Resolve `a` into a plan: the source exists, the connection is ready, the size fits.
    async fn plan(&self, a: &SendCreateArgs) -> Result<Plan, String> {
        check_shape(a)?;
        let now = Utc::now();
        let (expiration, deletion) = plan_dates(a, now)?;
        let ext = self.ext_settings().await;
        let settings = self.conn_settings(&a.connection).await?;
        let sender = self.sender(&a.connection).await?;
        let max_bytes = ext.send_max_file_mb.saturating_mul(1 << 20);
        let (source, source_text, own_name) = match (&a.source, &a.file) {
            (Some(s), _) => (self.text_source(s).await?, s.clone(), None),
            (_, Some(f)) => {
                let (src, name) = self.file_source(f, &sender, max_bytes, a).await?;
                (src, f.clone(), Some(name))
            }
            _ => unreachable!("check_shape"),
        };
        let file_name = match (&a.file_name, &a.as_file, own_name) {
            (Some(n), _, _) => Some(n.clone()),
            (None, Some(n), _) => Some(n.clone()),
            (None, None, n) => n,
        };
        if source.is_file() || a.as_file.is_some() {
            let name = file_name.as_deref().unwrap_or_default();
            if name.is_empty() || name.contains('/') || name.contains('\\') {
                return Err(format!(
                    "file name {name:?}: a plain file name, no directories"
                ));
            }
        }
        if let Some(SendPassword::From { from }) = &a.password
            && self.ext.vault.get(from, 0).is_none()
        {
            return Err(format!("password source {from} does not exist"));
        }
        if let Some(SendPassword::Ask { ask: false }) = &a.password {
            return Err("password: {\"ask\": true} or {\"from\": NAME}".into());
        }
        let link_name = a
            .link_name
            .clone()
            .unwrap_or_else(|| default_link_name(&a.connection));
        if !crate::vault::valid_name(&link_name) || self.ext.vault.get(&link_name, 0).is_some() {
            return Err(format!(
                "link_name {link_name:?} is taken or not a valid vault name"
            ));
        }
        Ok(Plan {
            conn: a.connection.clone(),
            source,
            source_text,
            file_name,
            name: a.name.clone().unwrap_or_else(|| DEFAULT_NAME.to_owned()),
            expiration,
            deletion,
            max_access: a.max_access_count.unwrap_or(1).max(1),
            password: a.password.clone(),
            hide_email: a.hide_email.unwrap_or(true),
            notes: a.notes.clone(),
            link_name,
            link_to_agent: settings.send_link_to_agent,
            max_bytes,
        })
    }

    async fn text_source(&self, s: &str) -> Result<Source, String> {
        if s.starts_with("bw/") || s.starts_with("aws/") {
            let (b, r) = self.ext.open(s, "read a Send's source").await?;
            let entry = self
                .ext
                .bounded("source", b.backend(), b.describe(&r))
                .await
                .map_err(|e| format!("source {s}: {e}"))?;
            let version = entry
                .versions
                .first()
                .map(|v| v.id.clone())
                .unwrap_or_default();
            return Ok(Source::External {
                text: s.to_owned(),
                version,
            });
        }
        let info = self
            .ext
            .vault
            .list()
            .into_iter()
            .find(|i| i.name == s)
            .ok_or_else(|| format!("source {s} does not exist"))?;
        Ok(Source::Local {
            name: s.to_owned(),
            version: info.current,
        })
    }

    async fn file_source(
        &self,
        f: &str,
        sender: &Arc<dyn SendBackend>,
        max_bytes: u64,
        a: &SendCreateArgs,
    ) -> Result<(Source, String), String> {
        let too_big = |name: &str, n: u64| {
            format!(
                "{name} is {}; the limit here is {} (Settings › Secrets › Send file size)",
                size(n),
                size(max_bytes)
            )
        };
        if let Some(id) = f.strip_prefix("artifact:") {
            let art = self
                .artifact(id)
                .await
                .ok_or_else(|| format!("artifact {id} not found in this session's tree"))?;
            if art.bytes > max_bytes {
                return Err(too_big(&art.name, art.bytes));
            }
            let name = art.name.clone();
            return Ok((
                Source::Artifact {
                    id: id.to_owned(),
                    path: art.path,
                    name: art.name,
                    bytes: art.bytes,
                },
                name,
            ));
        }
        if f.starts_with("bw/") {
            let (text, which) = f
                .split_once("#attachment:")
                .ok_or("a Bitwarden file is bw/CONN/ITEM#attachment:<name or id>")?;
            let (b, r) = self.ext.open(text, "read a Send's attachment").await?;
            let other = if b.conn() == a.connection {
                sender.clone()
            } else {
                b.sender().ok_or("that connection has no attachments")?
            };
            let (name, bytes) = other
                .attachment_info(&r, which)
                .await
                .map_err(|e| format!("{f}: {e}"))?;
            if let Some(n) = bytes.filter(|n| *n > max_bytes) {
                return Err(too_big(&name, n));
            }
            return Ok((
                Source::Attachment {
                    text: text.to_owned(),
                    which: which.to_owned(),
                    name: name.clone(),
                    bytes,
                },
                name,
            ));
        }
        let p = Path::new(f);
        if !p.is_absolute() {
            return Err(format!(
                "{f}: an absolute path, artifact:<id> or bw/…#attachment:<name>"
            ));
        }
        let meta = tokio::fs::metadata(p)
            .await
            .map_err(|e| format!("{f}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{f} is not a file"));
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if meta.len() > max_bytes {
            return Err(too_big(&name, meta.len()));
        }
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Ok((
            Source::Path {
                path: f.to_owned(),
                bytes: meta.len(),
                mtime_ms,
            },
            name,
        ))
    }

    /// The content, read now. `Err` when it changed since the card (210 §7.3).
    async fn content(&self, plan: &Plan) -> Result<SendContent, String> {
        let changed = || {
            format!(
                "{} changed since the approval card; ask again",
                plan.source_text
            )
        };
        let text = |t: Zeroizing<String>| match &plan.file_name {
            Some(name) if !plan.source.is_file() => SendContent::File {
                name: name.clone(),
                bytes: Zeroizing::new(t.as_bytes().to_vec()),
            },
            _ => SendContent::Text(t),
        };
        let file = |bytes: Zeroizing<Vec<u8>>| SendContent::File {
            name: plan.file_name.clone().unwrap_or_default(),
            bytes,
        };
        match &plan.source {
            Source::Local { name, version } => {
                let now = self.ext.vault.list().into_iter().find(|i| &i.name == name);
                if now.map(|i| i.current) != Some(*version) {
                    return Err(changed());
                }
                let v = self.ext.vault.get(name, 0).ok_or_else(changed)?;
                Ok(text(v))
            }
            Source::External { text: t, version } => {
                let (b, r) = self.ext.open(t, "read a Send's source").await?;
                let (v, ver) = self
                    .ext
                    .bounded("source", b.backend(), b.get(&r))
                    .await
                    .map_err(|e| format!("source {t}: {e}"))?;
                if &ver.id != version {
                    return Err(changed());
                }
                Ok(text(v))
            }
            Source::Path {
                path,
                bytes,
                mtime_ms,
            } => {
                let meta = tokio::fs::metadata(path)
                    .await
                    .map_err(|e| format!("{path}: {e}"))?;
                let m = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                if meta.len() != *bytes || m != *mtime_ms {
                    return Err(changed());
                }
                let b = tokio::fs::read(path)
                    .await
                    .map_err(|e| format!("{path}: {e}"))?;
                Ok(file(Zeroizing::new(b)))
            }
            Source::Artifact { path, bytes, .. } => {
                let b = tokio::fs::read(path)
                    .await
                    .map_err(|e| format!("artifact: {e}"))?;
                if b.len() as u64 != *bytes {
                    return Err(changed());
                }
                Ok(file(Zeroizing::new(b)))
            }
            Source::Attachment { text: t, which, .. } => {
                let (b, r) = self.ext.open(t, "read a Send's attachment").await?;
                let s = b.sender().ok_or("that connection has no attachments")?;
                let (_, bytes) = s
                    .attachment_bytes(&r, which, plan.max_bytes)
                    .await
                    .map_err(|e| format!("{t}: {e}"))?;
                Ok(file(bytes))
            }
        }
    }

    /// The access password, read or asked now. Returns it and the vault name it lives under.
    async fn password(
        &self,
        plan: &Plan,
        session: SessionId,
    ) -> Result<Option<(Zeroizing<String>, Option<String>)>, String> {
        match &plan.password {
            None => Ok(None),
            Some(SendPassword::From { from }) => {
                let v = self
                    .ext
                    .vault
                    .get(from, 0)
                    .ok_or_else(|| format!("password source {from} does not exist"))?;
                Ok(Some((v, None)))
            }
            Some(SendPassword::Ask { .. }) => {
                let name = format!("{}-password", plan.link_name);
                let req = Request {
                    session,
                    title: None,
                    fields: vec![SecretRequestField {
                        name: name.clone(),
                        label: format!("Send password for \"{}\"", plan.name),
                        masked: true,
                        current: None,
                        note: Some(format!("password of the Send {}", plan.link_name)),
                    }],
                    id: None,
                    expires: Some(crate::expiry::rfc3339(
                        plan.deletion.timestamp_millis() as u64
                    )),
                };
                let out = tokio::time::timeout(self.request_timeout, self.ext.actor.ask(req))
                    .await
                    .map_err(|_| "nobody typed the Send password in time".to_owned())?
                    .map_err(|e| format!("password card: {e}"))?;
                if out.stored.is_empty() {
                    return Err("the user did not set a Send password; nothing was created".into());
                }
                let v = self
                    .ext
                    .vault
                    .get(&name, 0)
                    .ok_or("the Send password was not stored")?;
                // Re-store under the Send source so `secret_delete` refuses it like the link.
                self.ext
                    .vault
                    .set_expiring(
                        &name,
                        &v,
                        Some(&format!("password of the Send {}", plan.link_name)),
                        SEND_SOURCE,
                        Some(plan.deletion.timestamp_millis().max(0) as u64),
                    )
                    .await
                    .map_err(|e| format!("the Send password was not stored: {e}"))?;
                Ok(Some((v, Some(name))))
            }
        }
    }
}

/// An artifact a Send may carry: its name, blob path and size.
pub struct ArtifactRef {
    /// `report.pdf`.
    pub name: String,
    /// The blob on this engine.
    pub path: String,
    /// Blob size.
    pub bytes: u64,
}

/// `artifact:<id>` sources (§12.3). `secrets` cannot depend on `artifact` (cycle through
/// `agent`), so the engine supplies the lookup: `ARTIFACT_ACTOR` `List { session, descendants:
/// true }` from the root of `session`'s tree, matched on `id`.
#[async_trait::async_trait]
pub trait ArtifactSource: Send + Sync {
    /// The artifact `id` of `session`'s tree, or `None`.
    async fn find(&self, session: SessionId, id: &str) -> Option<ArtifactRef>;
}

impl SendCx {
    async fn artifact(&self, id: &str) -> Option<ArtifactRef> {
        let session = CURRENT_SESSION.try_with(|s| *s).ok()?;
        self.artifacts.as_ref()?.find(session, id).await
    }
}

tokio::task_local! {
    /// The session a preview or call runs for (artifact lookups are scoped to its tree).
    static CURRENT_SESSION: SessionId;
}

/// The `secret_send_create` tool.
pub struct SendCreateTool(pub SendCx);

#[async_trait::async_trait]
impl Tool for SendCreateTool {
    const NAME: &'static str = "secret_send_create";
    const RISK: Risk = Risk::Destructive;
    const CONSUMES_SECRETS: bool = false;
    const APPROVAL_PREVIEW: bool = true;
    type Args = SendCreateArgs;

    fn description(&self) -> String {
        "Share one secret or one file through a Bitwarden Send: a link anyone who has it can open \
         until it expires or is used up. `source` = a local vault NAME, bw/CONN/ITEM#field or \
         aws/CONN/NAME (text), or `file` = an absolute path, artifact:<id> or \
         bw/CONN/ITEM#attachment:<name>. You never see the content or the link: you get \
         {{secret:send-…}}, which a messaging tool expands when it delivers it. Defaults: \
         expires 24h, 1 open, no password, sender email hidden. The user approves every Send."
            .into()
    }

    fn human_only(&self) -> bool {
        self.0.owner_only.load(Ordering::Relaxed)
    }

    async fn approval_preview(
        &self,
        a: SendCreateArgs,
        cx: PreviewCx,
    ) -> anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>> {
        let plan = CURRENT_SESSION
            .scope(cx.session, self.0.plan(&a))
            .await
            .map_err(anyhow::Error::msg)?;
        let mut m = serde_json::Map::new();
        let mut put = |k: &str, v: String| {
            m.insert(k.to_owned(), serde_json::Value::String(v));
        };
        if plan.source.is_file() || plan.file_name.is_some() {
            let name = plan.file_name.clone().unwrap_or_default();
            put("file", format!("{name} · {}", plan.source.label()));
            put("recipient sees", format!("the file name {name}"));
        } else {
            put("text", plan.source.label());
        }
        put("send on", format!("bw/{}", plan.conn));
        put("expires", local_time(plan.expiration));
        put("deleted", local_time(plan.deletion));
        put("max opens", plan.max_access.to_string());
        put(
            "password",
            match &plan.password {
                None => "none".into(),
                Some(SendPassword::From { from }) => format!("from {from}"),
                Some(SendPassword::Ask { .. }) => "asked on the next card".into(),
            },
        );
        put(
            "email",
            if plan.hide_email { "hidden" } else { "shown" }.into(),
        );
        put(
            "link",
            if plan.link_to_agent {
                "given to the agent (send_link_to_agent)".into()
            } else {
                format!("stored as {}", plan.link_name)
            },
        );
        self.0
            .plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((cx.session, cx.call_id), plan);
        Ok(Some(m))
    }

    async fn call(&self, a: SendCreateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cached = self
            .0
            .plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(cx.session, cx.call_id.clone()));
        let plan = match cached {
            Some(p) => p,
            None => match CURRENT_SESSION.scope(cx.session, self.0.plan(&a)).await {
                Ok(p) => p,
                Err(e) => return Ok(ToolOutput::err(e)),
            },
        };
        Ok(match self.0.create(&plan, cx.session, &cx.call_id).await {
            Ok((_, text)) => ToolOutput::ok(text),
            Err(e) => ToolOutput::err(e),
        })
    }
}

impl SendCx {
    async fn create(
        &self,
        plan: &Plan,
        session: SessionId,
        call_id: &str,
    ) -> Result<(SendRecord, String), String> {
        let cx_ = self;
        let content = cx_.content(plan).await?;
        let (file_bytes, kind) = match &content {
            SendContent::File { bytes, .. } => (Some(bytes.len() as u64), "file"),
            SendContent::Text(_) => (None, "text"),
        };
        let password = cx_.password(plan, session).await?;
        let sender = cx_.sender(&plan.conn).await?;
        let created = sender
            .create_send(
                content,
                SendOptions {
                    name: plan.name.clone(),
                    notes: plan.notes.clone(),
                    expiration: Some(plan.expiration),
                    deletion: plan.deletion,
                    max_access_count: Some(plan.max_access),
                    password: password.as_ref().map(|(p, _)| p.clone()),
                    hide_email: plan.hide_email,
                },
            )
            .await
            .map_err(|e| match &e {
                ExtError::Denied(m) if m.starts_with("file Sends need Premium") => {
                    if !plan.source.is_file() {
                        format!("{m} — retry without as_file to send it as text")
                    } else {
                        m.clone()
                    }
                }
                _ => format!("Send on bw/{}: {e}", plan.conn),
            })?;
        let deletion_ms = plan.deletion.timestamp_millis().max(0) as u64;
        let (link_secret, link_meta) = if plan.link_to_agent {
            let key = format!("ext/bw/{}/send/{}", plan.conn, created.send_id);
            cx_.ext
                .vault
                .set_local_secret(&key, created.link.as_bytes())
                .await
                .map_err(|e| format!("the Send exists but its link was not kept: {e:#}"))?;
            (None, Some(key))
        } else {
            cx_.ext
                .vault
                .set_expiring(
                    &plan.link_name,
                    &created.link,
                    Some(&format!(
                        "Bitwarden Send on bw/{}: {}",
                        plan.conn, plan.name
                    )),
                    SEND_SOURCE,
                    Some(deletion_ms),
                )
                .await
                .map_err(|e| format!("the Send exists but its link was not stored: {e}"))?;
            (Some(plan.link_name.clone()), None)
        };
        let password_secret = password.and_then(|(_, n)| n);
        let id = uuid::Uuid::new_v4().to_string();
        let now_ms = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO external_sends (id, conn, send_id, access_id, kind, name, file_name, \
             file_bytes, source_ref, source_version, link_secret, link_meta, password_secret, \
             created_ms, expiration_ms, deletion_ms, max_access, session, call_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&plan.conn)
        .bind(&created.send_id)
        .bind(&created.access_id)
        .bind(kind)
        .bind(&plan.name)
        .bind(&plan.file_name)
        .bind(file_bytes.map(|b| b as i64))
        .bind(&plan.source_text)
        .bind(plan.source.version())
        .bind(&link_secret)
        .bind(&link_meta)
        .bind(&password_secret)
        .bind(now_ms)
        .bind(plan.expiration.timestamp_millis())
        .bind(deletion_ms as i64)
        .bind(plan.max_access as i64)
        .bind(session.to_string())
        .bind(call_id)
        .execute(cx_.pool())
        .await
        .map_err(|e| format!("the Send exists but was not recorded: {e}"))?;
        cx_.audit(format!(
            "send.create conn=bw/{} kind={kind}{} source={} expires={} deleted={} max={} \
             password={} session={} call={}",
            plan.conn,
            plan.file_name
                .as_ref()
                .map(|n| format!(" file={n} bytes={}", file_bytes.unwrap_or_default()))
                .unwrap_or_default(),
            plan.source_text,
            created.expiration_date.as_deref().unwrap_or("-"),
            created.deletion_date,
            plan.max_access,
            plan.password.is_some(),
            session,
            call_id,
        ))
        .await;
        let what = match &plan.file_name {
            Some(n) => format!(
                "file Send on bw/{}: {n} ({}, from {})",
                plan.conn,
                size(file_bytes.unwrap_or_default()),
                plan.source_text
            ),
            None => format!("text Send on bw/{}", plan.conn),
        };
        let password_line = match &plan.password {
            None => String::new(),
            Some(SendPassword::From { from }) => format!(", password from {from}"),
            Some(SendPassword::Ask { .. }) => format!(
                ", password stored as {{{{secret:{}-password}}}} (send it by another channel than the link)",
                plan.link_name
            ),
        };
        let opens = if plan.max_access == 1 {
            "1 open".into()
        } else {
            format!("{} opens", plan.max_access)
        };
        let link = if plan.link_to_agent {
            format!("the link is {}", created.link.as_str())
        } else {
            format!(
                "the link is {{{{secret:{}}}}} (deleted with the Send on {}); the user can copy it from the card",
                plan.link_name,
                local_time(plan.deletion)
            )
        };
        let receipt = format!(
            "created {what}: expires {}, {opens}{password_line}, email {}; {link}",
            local_time(plan.expiration),
            if plan.hide_email { "hidden" } else { "shown" },
        );
        let record = SendRecord {
            id,
            conn: plan.conn.clone(),
            kind: kind.to_owned(),
            name: plan.name.clone(),
            file_name: plan.file_name.clone(),
            file_bytes: file_bytes.map(|b| b as i64),
            source_ref: plan.source_text.clone(),
            link_secret,
            created_ms: now_ms,
            expiration_ms: Some(plan.expiration.timestamp_millis()),
            deletion_ms: deletion_ms as i64,
            access_count: 0,
            max_access: Some(plan.max_access as i64),
            has_password: plan.password.is_some(),
            status: SendStatus::Active,
        };
        Ok((record, receipt))
    }
}

/// The owner's Send dialog (`create_send` RPC): the user is the actor, so there is no card. Same
/// resolution as the tool; `session` is the owner console's (artifact scope, the password card),
/// `call_id` names the request in the record. Returns the new record and the receipt text.
pub async fn create_send(
    cx: &SendCx,
    args: SendCreateArgs,
    session: SessionId,
    call_id: &str,
) -> Result<(SendRecord, String), String> {
    let plan = CURRENT_SESSION.scope(session, cx.plan(&args)).await?;
    cx.create(&plan, session, call_id).await
}

/// A Send's state, as the Secrets page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendStatus {
    /// Open-able.
    Active,
    /// Past its expiry.
    Expired,
    /// Opened `max_access` times.
    UsedUp,
    /// Revoked by theuth.
    Revoked,
    /// Gone on Bitwarden (deletion date, or deleted elsewhere).
    DeletedByBitwarden,
}

impl SendStatus {
    fn text(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::UsedUp => "used up",
            Self::Revoked => "revoked",
            Self::DeletedByBitwarden => "deleted by Bitwarden",
        }
    }
}

/// One `external_sends` row with its live counts (§12.4); never the link
/// (`crates/desktop/ui/src/components/library/secrets/types.ts` `SendRecord`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendRecord {
    /// Row id.
    pub id: String,
    /// Connection.
    pub conn: String,
    /// `text` | `file`.
    pub kind: String,
    /// Shown to the recipient.
    pub name: String,
    /// File name of a file Send.
    pub file_name: Option<String>,
    /// File size.
    pub file_bytes: Option<i64>,
    /// The source reference (vault name, `bw/…`, `aws/…`, path, `artifact:…`).
    pub source_ref: String,
    /// The vault name of the link (`None` with `send_link_to_agent`).
    pub link_secret: Option<String>,
    /// Unix ms.
    pub created_ms: i64,
    /// Unix ms.
    pub expiration_ms: Option<i64>,
    /// Unix ms.
    pub deletion_ms: i64,
    /// Times opened (after the last sync; 0 when unknown).
    pub access_count: u32,
    /// Opening limit.
    pub max_access: Option<i64>,
    /// Has an access password.
    pub has_password: bool,
    /// State.
    pub status: SendStatus,
}

/// `secret_send_list` args.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendListArgs {
    /// A Bitwarden connection; absent = every connection's Sends theuth made.
    #[serde(default)]
    pub connection: Option<String>,
}

/// One row of `external_sends`.
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    conn: String,
    send_id: String,
    kind: String,
    name: String,
    file_name: Option<String>,
    file_bytes: Option<i64>,
    source_ref: String,
    link_secret: Option<String>,
    link_meta: Option<String>,
    password_secret: Option<String>,
    created_ms: i64,
    expiration_ms: Option<i64>,
    deletion_ms: i64,
    max_access: Option<i64>,
    revoked_ms: Option<i64>,
}

const ROW_COLS: &str = "id, conn, send_id, kind, name, file_name, file_bytes, source_ref, \
                        link_secret, link_meta, password_secret, created_ms, expiration_ms, \
                        deletion_ms, max_access, revoked_ms";

fn ms_time(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms).map_or_else(|| ms.to_string(), local_time)
}

/// `known` = the connection's live Send list was read (so absence means deleted).
fn status_of(r: &Row, info: Option<&SendInfo>, known: bool, now: i64) -> SendStatus {
    if r.revoked_ms.is_some() {
        SendStatus::Revoked
    } else if r.deletion_ms <= now || (known && info.is_none()) {
        SendStatus::DeletedByBitwarden
    } else if info.is_some_and(|i| i.max_access_count.is_some_and(|m| i.access_count >= m)) {
        SendStatus::UsedUp
    } else if r.expiration_ms.is_some_and(|e| e <= now) {
        SendStatus::Expired
    } else {
        SendStatus::Active
    }
}

/// The records of `conn` (or every connection), newest first, with live counts, plus the
/// lines about Sends made elsewhere (`manage_all_sends`) and unreadable connections.
async fn gather(cx: &SendCx, conn: Option<&str>) -> Result<(Vec<SendRecord>, Vec<String>), String> {
    let rows: Vec<Row> = match conn {
        Some(c) => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {ROW_COLS} FROM external_sends WHERE conn = ? ORDER BY created_ms DESC"
            )))
            .bind(c)
            .fetch_all(cx.pool())
            .await
        }
        None => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {ROW_COLS} FROM external_sends ORDER BY created_ms DESC"
            )))
            .fetch_all(cx.pool())
            .await
        }
    }
    .map_err(|e| format!("external_sends: {e}"))?;
    let mut conns: Vec<String> = rows.iter().map(|r| r.conn.clone()).collect();
    conns.extend(conn.map(str::to_owned));
    conns.sort();
    conns.dedup();
    let mut live: HashMap<String, SendInfo> = HashMap::new();
    let mut known: Vec<String> = Vec::new();
    let mut lines = Vec::new();
    for c in &conns {
        let settings = cx.conn_settings(c).await.unwrap_or_default();
        let list = match cx.sender(c).await {
            Ok(s) => s.list_sends().await.map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        let list = match list {
            Ok(l) => l,
            Err(e) => {
                lines.push(format!("bw/{c}: Sends not readable now ({e})"));
                continue;
            }
        };
        known.push(c.clone());
        for i in list {
            let ours = rows.iter().any(|r| &r.conn == c && r.send_id == i.send_id);
            if settings.manage_all_sends && !ours {
                lines.push(format!(
                    "{} · bw/{c} · made elsewhere · {} · opened {}/{} · password {} · deleted {}",
                    i.send_id,
                    i.file_name
                        .as_deref()
                        .map_or_else(|| i.kind.clone(), |f| format!("file {f}")),
                    i.access_count,
                    i.max_access_count.map_or("∞".into(), |m| m.to_string()),
                    if i.has_password { "yes" } else { "no" },
                    i.deletion_date,
                ));
            }
            live.insert(i.send_id.clone(), i);
        }
    }
    let now = Utc::now().timestamp_millis();
    let records = rows
        .into_iter()
        .map(|r| {
            let info = live.get(&r.send_id);
            let status = status_of(&r, info, known.contains(&r.conn), now);
            SendRecord {
                access_count: info.map_or(0, |i| i.access_count),
                has_password: r.password_secret.is_some() || info.is_some_and(|i| i.has_password),
                status,
                id: r.id,
                conn: r.conn,
                kind: r.kind,
                name: r.name,
                file_name: r.file_name,
                file_bytes: r.file_bytes,
                source_ref: r.source_ref,
                link_secret: r.link_secret,
                created_ms: r.created_ms,
                expiration_ms: r.expiration_ms,
                deletion_ms: r.deletion_ms,
                max_access: r.max_access,
            }
        })
        .collect();
    Ok((records, lines))
}

/// The Secrets page's Sends tab (`list_sends` RPC): the Sends theuth made on `conn` (or every
/// connection), newest first, with live counts. Never a link.
pub async fn list_records(cx: &SendCx, conn: Option<&str>) -> Result<Vec<SendRecord>, String> {
    gather(cx, conn).await.map(|(r, _)| r)
}

/// The `secret_send_list` tool.
pub struct SendListTool(pub SendCx);

#[async_trait::async_trait]
impl Tool for SendListTool {
    const NAME: &'static str = "secret_send_list";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    type Args = SendListArgs;

    fn description(&self) -> String {
        "List the Bitwarden Sends theuth created (all Sends with manage_all_sends): link name, \
         kind, source, dates, opens, password yes/no, status. Never a link."
            .into()
    }

    async fn call(&self, a: SendListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let (records, mut lines) = match gather(&self.0, a.connection.as_deref()).await {
            Ok(x) => x,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        for r in &records {
            lines.push(format!(
                "{} · bw/{} · {} · {} · from {} · created {} · expires {} · deleted {} · opened {}/{} · password {} · {}",
                r.link_secret.clone().unwrap_or_else(|| r.id.clone()),
                r.conn,
                r.name,
                match &r.file_name {
                    Some(f) if r.kind == "file" =>
                        format!("file {f} · {}", size(r.file_bytes.unwrap_or_default() as u64)),
                    _ => r.kind.clone(),
                },
                r.source_ref,
                ms_time(r.created_ms),
                r.expiration_ms.map_or("-".into(), ms_time),
                ms_time(r.deletion_ms),
                r.access_count,
                r.max_access.map_or("∞".into(), |m| m.to_string()),
                if r.has_password { "yes" } else { "no" },
                r.status.text(),
            ));
        }
        if lines.is_empty() {
            return Ok(ToolOutput::ok("no Sends"));
        }
        Ok(ToolOutput::ok(lines.join("\n")))
    }
}

/// `secret_send_revoke` args.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendRevokeArgs {
    /// The link name (`send-personal-3fa9c2d1`) or the Bitwarden Send id.
    pub send: String,
}

/// The `secret_send_revoke` tool.
pub struct SendRevokeTool(pub SendCx);

#[async_trait::async_trait]
impl Tool for SendRevokeTool {
    const NAME: &'static str = "secret_send_revoke";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = SendRevokeArgs;

    fn description(&self) -> String {
        "Revoke a Bitwarden Send theuth created (by link name or Send id): Bitwarden deletes it, \
         the link stops working, and its link and password secrets leave the vault."
            .into()
    }

    async fn call(&self, a: SendRevokeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(match revoke(&self.0, &a.send, Some(&cx)).await {
            Ok(t) => ToolOutput::ok(t),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Revoke `send` (link name or Send id): delete it on Bitwarden, drop its secrets, stamp the row.
/// Also what the owner RPC and connection removal call. A Send theuth did not make is revoked
/// only with `manage_all_sends`.
pub async fn revoke(cx: &SendCx, send: &str, call: Option<&ToolCx>) -> Result<String, String> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ROW_COLS} FROM external_sends WHERE link_secret = ? OR send_id = ? \
         ORDER BY created_ms DESC LIMIT 1"
    )))
    .bind(send)
    .bind(send)
    .fetch_optional(cx.pool())
    .await
    .map_err(|e| format!("external_sends: {e}"))?;
    let Some(row) = row else {
        return revoke_foreign(cx, send).await;
    };
    if row.revoked_ms.is_some() {
        return Err(format!("{send} was already revoked"));
    }
    let sender = cx.sender(&row.conn).await?;
    let opened = sender
        .list_sends()
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|i| i.send_id == row.send_id));
    match sender.delete_send(&row.send_id).await {
        Ok(()) | Err(ExtError::NotFound) => {}
        Err(e) => return Err(format!("revoke {send}: {e}")),
    }
    for name in [&row.link_secret, &row.password_secret]
        .into_iter()
        .flatten()
    {
        let _ = cx.ext.vault.delete(name).await;
    }
    if let Some(key) = &row.link_meta {
        let _ = cx.ext.vault.delete_local_prefix(key).await;
    }
    sqlx::query("UPDATE external_sends SET revoked_ms = ? WHERE send_id = ?")
        .bind(Utc::now().timestamp_millis())
        .bind(&row.send_id)
        .execute(cx.pool())
        .await
        .map_err(|e| format!("external_sends: {e}"))?;
    cx.audit(format!(
        "send.revoke conn=bw/{} send={} kind={} source={}{}",
        row.conn,
        row.send_id,
        row.kind,
        row.source_ref,
        call.map(|c| format!(" session={} call={}", c.session, c.call_id))
            .unwrap_or_default()
    ))
    .await;
    let label = row.link_secret.clone().unwrap_or(row.send_id.clone());
    Ok(match opened {
        Some(i) => format!(
            "revoked {label} (opened {} of {} times)",
            i.access_count,
            i.max_access_count.map_or("∞".into(), |m| m.to_string())
        ),
        None => format!("revoked {label}"),
    })
}

/// `manage_all_sends`: a Send theuth did not make, by Send id, on whichever connection has it.
async fn revoke_foreign(cx: &SendCx, send_id: &str) -> Result<String, String> {
    for b in cx.ext.externals.all() {
        if b.backend() != "bw" {
            continue;
        }
        let conn = b.conn().to_owned();
        if !cx
            .conn_settings(&conn)
            .await
            .is_ok_and(|s| s.manage_all_sends)
        {
            continue;
        }
        let Some(s) = b.sender() else { continue };
        if s.list_sends()
            .await
            .is_ok_and(|l| l.iter().any(|i| i.send_id == send_id))
        {
            s.delete_send(send_id)
                .await
                .map_err(|e| format!("revoke {send_id}: {e}"))?;
            cx.audit(format!(
                "send.revoke conn=bw/{conn} send={send_id} foreign=true"
            ))
            .await;
            return Ok(format!(
                "revoked {send_id} on bw/{conn} (not made by theuth)"
            ));
        }
    }
    Err(format!(
        "no Send {send_id} that theuth made (and manage_all_sends is off for the others)"
    ))
}

/// Revoke every active Send of `conn` (connection removal). Failures are returned, not raised.
pub async fn revoke_all(cx: &SendCx, conn: &str) -> Vec<String> {
    let ids: Vec<(String,)> = sqlx::query_as(
        "SELECT send_id FROM external_sends WHERE conn = ? AND revoked_ms IS NULL AND deletion_ms > ?",
    )
    .bind(conn)
    .bind(Utc::now().timestamp_millis())
    .fetch_all(cx.pool())
    .await
    .unwrap_or_default();
    let mut failed = Vec::new();
    for (id,) in ids {
        if let Err(e) = revoke(cx, &id, None).await {
            failed.push(e);
        }
    }
    failed
}

/// The link of a Send theuth made, for the owner's Copy link (`copy_send_link`): only rows of
/// `external_sends`. `None` when unknown, revoked or gone.
pub async fn send_link(cx: &SendCx, send: &str) -> Result<Option<Zeroizing<String>>, String> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ROW_COLS} FROM external_sends WHERE (link_secret = ? OR send_id = ?) \
         AND revoked_ms IS NULL ORDER BY created_ms DESC LIMIT 1"
    )))
    .bind(send)
    .bind(send)
    .fetch_optional(cx.pool())
    .await
    .map_err(|e| format!("external_sends: {e}"))?;
    let Some(row) = row else { return Ok(None) };
    if let Some(name) = &row.link_secret {
        return Ok(cx.ext.vault.get(name, 0));
    }
    if let Some(key) = &row.link_meta {
        return Ok(cx
            .ext
            .vault
            .local_secret(key)
            .await
            .map_err(|e| format!("{e:#}"))?
            .and_then(|b| String::from_utf8(b.to_vec()).ok())
            .map(Zeroizing::new));
    }
    Ok(None)
}

/// Whether `name` is a Send link or password secret (vault source [`SEND_SOURCE`]).
pub fn is_send_secret(source: &str) -> bool {
    source == SEND_SOURCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: serde_json::Value) -> SendCreateArgs {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn shape_needs_exactly_one_content() {
        assert!(check_shape(&args(serde_json::json!({"connection": "p", "source": "X"}))).is_ok());
        assert!(check_shape(&args(serde_json::json!({"connection": "p", "file": "/x"}))).is_ok());
        assert!(check_shape(&args(serde_json::json!({"connection": "p"}))).is_err());
        assert!(
            check_shape(&args(
                serde_json::json!({"connection": "p", "source": "X", "file": "/x"})
            ))
            .is_err()
        );
        assert!(
            check_shape(&args(
                serde_json::json!({"connection": "p", "source": "{{secret:X}}"})
            ))
            .is_err()
        );
        assert!(
            check_shape(&args(
                serde_json::json!({"connection": "p", "file": "/x", "as_file": "a"})
            ))
            .is_err()
        );
    }

    #[test]
    fn dates_default_and_bounds() {
        let now = Utc::now();
        let (e, d) = plan_dates(
            &args(serde_json::json!({"connection": "p", "source": "X"})),
            now,
        )
        .unwrap();
        assert_eq!(e, d);
        assert!((e - now - chrono::Duration::hours(24)).num_seconds().abs() <= 1);
        let (e, d) = plan_dates(
            &args(serde_json::json!({"connection": "p", "source": "X", "expires": "2h", "delete": "3d"})),
            now,
        )
        .unwrap();
        assert!(d > e);
        assert!(
            plan_dates(
                &args(serde_json::json!({"connection": "p", "source": "X", "delete": "40d"})),
                now
            )
            .unwrap_err()
            .contains("31 days")
        );
        assert!(
            plan_dates(
                &args(serde_json::json!({"connection": "p", "source": "X", "expires": "3d", "delete": "1d"})),
                now
            )
            .is_err()
        );
    }

    #[test]
    fn password_shapes_and_link_names() {
        let a =
            args(serde_json::json!({"connection": "p", "source": "X", "password": {"from": "PW"}}));
        assert!(matches!(&a.password, Some(SendPassword::From { from }) if from == "PW"));
        let a =
            args(serde_json::json!({"connection": "p", "source": "X", "password": {"ask": true}}));
        assert!(matches!(a.password, Some(SendPassword::Ask { ask: true })));
        let n = default_link_name("personal");
        assert!(n.starts_with("send-personal-") && n.len() == "send-personal-".len() + 8);
        assert!(crate::vault::valid_name(&n));
        assert!(is_send_secret(SEND_SOURCE));
    }

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(3277), "3.2 KB");
        assert_eq!(size(3 << 20), "3.0 MB");
    }
}
