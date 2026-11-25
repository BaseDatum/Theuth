//! Secrets plugin (design doc 37): named, versioned values the agent can use
//! but never read. `<data_dir>/secrets.vault` is one XChaCha20-Poly1305 blob
//! whose key sits in the OS keychain (or `secrets.key`, 0600, when there is
//! none). The model writes `{{secret:NAME}}` / `{{secret:NAME:-N}}` inside any
//! tool argument; `tools::secrets` swaps the value in after the approval gate
//! and scrubs it out of tool output again. New values come from the user's
//! keyboard (`secret_request`), a tool's output (`secret_capture`) or an older
//! version (`secret_restore`) — never from the model's arguments.
//!
//! - [`vault`]: the encrypted file, five versions per name, the scrubber.
//! - [`expiry`]: the `expires` grammar (`30m`, `2h`, `3d`, ISO-8601) and local-time text.
//! - [`requests`]: the replicated `secret_requests` rows (79 §6.6).
//! - [`tools`]: the five `secret_*` tools and the prompt block.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`SECRETS_ACTOR`] actor ([`SecretsActor`]): [`List`],
//!   [`Set`], [`Restore`], [`Delete`], [`Request`], [`Answer`], [`Waive`], [`PendingRequests`],
//!   [`ListRequests`], [`SetEngine`], [`Handle`]; 183: [`GetRotation`] (the mesh reaches the
//!   keyring through [`Handle`] and the [`Vault`] methods directly); 221: [`RegisterExternal`],
//!   [`UnregisterExternal`]
//! - [`external::ExternalBackend`]: the trait external backend crates implement
//! - events: [`SecretsEvent`]
//! - [`Vault`] as a library, for `t --secret-set` and `--slack-setup`

/// Credentials RPC methods and typed client.
pub mod rpc;
/// Provider-owned helpers used by RPC method owners.
pub mod rpc_support;
pub use rpc::{CredentialsApiClient, rpc_module};
pub mod connections;
pub mod expiry;
pub mod external;
mod key;
pub mod requests;
pub mod settings;
pub mod store;
pub mod tools;
pub mod tools_external;
pub mod tools_send;
pub mod vault;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use platform::time::now_ms;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

pub use connections::{BackendFactory, ConnStore, RegisterBackendFactory};
pub use external::{ExternalBackend, RegisterExternal, UnregisterExternal};
pub use vault::{
    KEEP, KeyInfo, KeyStore, MESH_PREFIX, OAUTH_PREFIX, PLUGINS_GRANTS_KEY, PLUGINS_PREFIX,
    RetireHold, RotationPeer, RotationPhase, RotationState, SHARE_PREFIX, SecretInfo, SecretsList,
    Vault, VersionInfo, WireKey,
};

/// How often the plugin looks for rows written by others (a peer's batch
/// applied by the replica, `t --secret-set`) and refreshes the in-memory copy.
const REFRESH_POLL: Duration = Duration::from_secs(1);

/// Registry name of the [`SecretsActor`].
pub const SECRETS_ACTOR: &str = "secrets";

/// `secrets:` config section (37 §9).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsConfig {
    /// Where the vault key lives: `auto` (keychain, else file), `keychain`, `file`.
    pub key_store: KeyStore,
    /// The agent-facing surface: the `secret_*` tools, the prompt block and
    /// placeholder expansion. Off, the vault still serves in-process readers.
    pub tools: bool,
    /// How long `secret_request` waits for the user.
    pub request_timeout_secs: u64,
    /// 221 D20: the harness-wide external-secret settings (`secrets.external`).
    pub external: settings::ExternalSettings,
    /// 221 §6: the external connections (`secrets.connections.<conn>`).
    pub connections: std::collections::BTreeMap<String, settings::ConnectionConfig>,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            key_store: KeyStore::Auto,
            tools: true,
            request_timeout_secs: 1800,
            external: settings::ExternalSettings::default(),
            connections: std::collections::BTreeMap::new(),
        }
    }
}

/// Correlates a [`SecretsEvent::Requested`] with its [`Answer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct RequestId(pub uuid::Uuid);

/// One field of a replicated [`SecretRequest`] (79 §6.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretRequestField {
    /// Vault name the answer is stored under.
    pub name: String,
    /// Label shown on the card.
    pub label: String,
    /// Masked input.
    pub masked: bool,
    /// `Some(v)` when the name exists: the answer adds `v + 1`.
    pub current: Option<u32>,
    /// Stored as the secret's note (shown in `secret_list`); `None` = the label. Absent on
    /// the wire when `None`, so a field without one looks as it did before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One `secret_requests` row (79 §6.6, contract §2.7): a session on any engine
/// asked for one or several secrets; any engine's app answers it. Replicated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretRequest {
    /// Row id.
    pub id: RequestId,
    /// Engine the session runs on.
    pub engine: String,
    /// The asking session.
    pub session: String,
    /// Card title (a form); `None` for a single `secret_request`.
    pub title: Option<String>,
    /// The `fields` JSON column.
    pub fields: Vec<SecretRequestField>,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
    /// Set by `answer_secret`.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub answered_ms: Option<u64>,
    /// `stored` | `declined` | `timeout` | [`PROCEED_WITHOUT`] once answered.
    pub outcome: Option<String>,
    /// Replication clock.
    pub hlc: Option<String>,
    /// The agent's expiry suggestion that pre-fills the card's control: `30m` / `2h` / `3d`
    /// (counted from when the user stores) or an RFC 3339 instant. `None` = Never.
    #[serde(default)]
    pub expires: Option<String>,
    /// 221 §7.3: the request stores into an external manager; names and status, never a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<connections::DestinationView>,
}

/// `ask` → [`SecretsList`]: names, notes, versions, key store. Never values.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct List;

/// `ask` → `Result<SecretInfo, String>`: a new version of `name`;
/// `note: None` keeps the old note. `Err` = invalid name or write failure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Set {
    /// `[A-Za-z0-9_.-]{1,64}`.
    pub name: String,
    /// The value. Transits once; never echoed.
    pub value: String,
    /// Shown in lists; `None` keeps the previous note.
    pub note: Option<String>,
    /// Who set it: `user`, `cli`, `provider add`, a tool name, …
    pub source: String,
}

/// `ask` → `Result<SecretInfo, String>`: copy version `-back` (1..=4) to the
/// top as a new version. `Err` = unknown name or `back` out of range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restore {
    /// The secret.
    pub name: String,
    /// Versions back from the current one.
    pub back: u8,
}

/// `ask` → `Result<(), String>`: the name and every version. User surfaces
/// (the model's delete is [`DeleteConfirmed`], 221 D24). `Err` = unknown name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delete {
    /// The secret.
    pub name: String,
}

/// `ask` → `Result<u64, String>` (how many versions went): [`Delete`] for `secret_delete` (221
/// D24), only while `confirm` is the current version, checked inside the delete's transaction.
/// `Err` = unknown name, or `<name> is at vN, not vM; …`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteConfirmed {
    /// The secret.
    pub name: String,
    /// The version the caller saw as current.
    pub confirm: u32,
}

/// `ask` → `Result<Outcome, String>`: ask the user to type one or several
/// secrets into masked fields. Leaves the mailbox; writes a replicated
/// `secret_requests` row, publishes [`SecretsEvent::Requested`] locally, waits
/// `request_timeout_secs` for an [`Answer`] here or on another engine (the
/// row's `answered_ms` or the vault rows arriving by replication), publishes
/// [`SecretsEvent::Resolved`]. `Err` = invalid name or write failure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    /// The asking session.
    pub session: SessionId,
    /// Card title (a form); `None` for a single `secret_request`.
    pub title: Option<String>,
    /// What to store, in card order (`label` = what it is for). `current` is filled here.
    pub fields: Vec<SecretRequestField>,
    /// 109 §6.2: the row id. `None` mints one. A `Some` naming a row that
    /// already exists re-arms the wait on that row — its own `fields`, the
    /// same [`SecretsEvent::Requested`], `created_ms + request_timeout_secs`
    /// as the deadline — instead of inserting a second one (a resumed
    /// `secret_request*` call); an answered row resolves at once.
    #[serde(default)]
    pub id: Option<RequestId>,
    /// The agent's expiry suggestion ([`SecretRequest::expires`]), already normalized by
    /// [`expiry::normalize`]; the user's choice on the card wins.
    #[serde(default)]
    pub expires: Option<String>,
}

/// What [`Request`] resolves to: the new version per field when stored, else why not.
#[derive(Clone, Debug, kameo::Reply)]
pub struct Outcome {
    /// `(name, new version)` per field the user typed a value for; empty when declined or timed out.
    pub stored: Vec<(String, u32)>,
    /// Nobody answered in time.
    pub timed_out: bool,
    /// 207: the user left the request to the agent (outcome [`PROCEED_WITHOUT`]).
    pub waived: bool,
    /// When the stored values expire (absolute Unix ms), as the user set it on the card.
    pub expires_ms: Option<u64>,
    /// 221 §7.3: a request with a destination: `Ok(receipt)` (where it went, the reference to
    /// use) or `Err(why the write failed)`; `None` otherwise.
    pub external: Option<Result<String, String>>,
}

/// `ask` → `Result<Outcome, String>`: [`Request`] that stores into an external manager
/// (221 §7.3); `destination` is resolved on this engine and goes on the row and the card.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestTo {
    /// The request.
    pub request: Request,
    /// Where the answer goes.
    pub destination: Option<connections::DestinationView>,
}

impl Message<RequestTo> for SecretsActor {
    type Reply = DelegatedReply<Result<Outcome, String>>;

    async fn handle(&mut self, m: RequestTo, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.request(m.request, m.destination, ctx).await
    }
}

/// 207: outcome of a request the user left to the agent (beside `stored` | `declined` | `timeout`).
pub const PROCEED_WITHOUT: &str = "proceed_without";

/// `ask` → `Result<bool, String>`: answer a request — one waiting here, or a
/// replicated row from another engine (the vault rows and `answered_ms`
/// replicate back; its poller finishes the tool call). `value` is the single
/// field's value (`secret_request`), `values` per-field (`secret_request_form`);
/// neither = declined. Stored with source `user`. `false` when `id` is unknown
/// or already answered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answer {
    /// From [`SecretsEvent::Requested`] / the row.
    pub id: RequestId,
    /// The typed value for a single-field request.
    pub value: Option<String>,
    /// Field name → typed value for a form.
    pub values: HashMap<String, String>,
    /// When every stored value expires (absolute Unix ms, in the future); `None` = never.
    #[serde(default)]
    pub expires_ms: Option<u64>,
}

/// 207: `ask` → `Result<bool, String>` (true when it was unanswered): resolve as "none is coming".
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Waive {
    /// From [`SecretsEvent::Requested`] / the row.
    pub id: RequestId,
}

/// `tell`: this engine's mesh id, stamped on the rows this engine writes
/// (79 §6.6). Sent by the mesh plugin as soon as its endpoint is bound; the
/// hostname until then. Secrets cannot ask the mesh itself (mesh depends on secrets).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEngine(pub String);

/// `ask` → `Result<Vec<SecretRequest>, String>`: the replicated rows, newest
/// first (the attention inbox's `secret:<id>` items).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListRequests {
    /// Only rows without `answered_ms`.
    pub unanswered_only: bool,
}

/// `ask` → `Arc<Vault>`: in-process readers (genai auth, `web`, `slack`)
/// call `get` directly (37 §3.7). Never exposed over RPC.
#[derive(Clone, Copy, Debug, Default)]
pub struct Handle;

/// 183 §14.7: `ask` → `Option<RotationState>`: the newest rotation (`vault_rotation`).
#[derive(Clone, Copy, Debug, Default)]
pub struct GetRotation;

/// Bus events (37 §3.3).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum SecretsEvent {
    /// Names, notes, versions or the key store changed; UIs re-`list_secrets`.
    /// Wire `type` is `SecretsChanged`: `ScheduleEvent::Changed` already owns `Changed`.
    #[serde(rename = "SecretsChanged")]
    Changed,
    /// The model asked for one or several; frontends show masked fields.
    /// `name` / `note` / `current` are the first field (pre-79 clients);
    /// `fields` is every field, `title` the form's card title.
    #[serde(rename = "SecretRequested")]
    Requested {
        /// Answer with this id.
        id: RequestId,
        /// The asking session.
        session: SessionId,
        /// Name the (first) field is stored under.
        name: String,
        /// What it is for (the form's title when there is one).
        note: String,
        /// `Some(v)` when the name exists: the answer adds `v + 1`.
        current: Option<u32>,
        /// A form's card title.
        #[serde(default)]
        title: Option<String>,
        /// Every field, in card order; one entry for `secret_request`.
        #[serde(default)]
        fields: Vec<SecretRequestField>,
        /// The agent's expiry suggestion for the card's control ([`SecretRequest::expires`]);
        /// absent on the wire when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires: Option<String>,
        /// 221 §7.3: where the answer is stored when not (only) the local vault. Boxed: the
        /// view is large and most requests have none (`large_enum_variant`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination: Option<Box<connections::DestinationView>>,
    },
    /// The request was answered, declined or timed out; frontends drop the field.
    #[serde(rename = "SecretResolved")]
    Resolved {
        /// The request.
        id: RequestId,
        /// New version when stored.
        stored: Option<u32>,
        /// Nobody answered in time.
        timed_out: bool,
    },
    /// 183 §10.4 (D7): a rotation minted, swept, reached a peer, finished or failed.
    #[serde(rename = "SecretsRotation")]
    Rotation(RotationState),
    /// 221 §9.4: a call needs a locked connection; frontends show the unlock card and answer
    /// with `answer_unlock(id, password)`.
    #[serde(rename = "ExternalUnlockRequested")]
    UnlockRequested {
        /// Answer with this id.
        id: RequestId,
        /// The connection.
        conn: String,
        /// `aws` | `bw`.
        backend: String,
        /// What the card names: `me@… · vault.bitwarden.eu`.
        account: String,
        /// Why: `a tool call uses bw/personal/2f1c…#password`.
        reason: String,
    },
    /// The unlock card was answered, declined or timed out; frontends drop it.
    #[serde(rename = "ExternalUnlockResolved")]
    UnlockResolved {
        /// The card.
        id: RequestId,
        /// The connection is unlocked now.
        unlocked: bool,
    },
}

impl wire::WireEvent for SecretsEvent {
    const TOPIC: &'static str = "secrets.events";
    /// 216 §7.3 (a): the live masked-entry moments and wording a plugin could imitate.
    const DROP: &'static [&'static str] = &["SecretRequested", "ExternalUnlockRequested"];
    /// 216 §7.3, reviewed: `key` is a `KeyInfo` (key id hash, epoch, members, mint time; no
    /// material) and `has_key` a per-peer acknowledgement flag.
    const REVIEWED_PUBLIC: &'static [&'static str] =
        &["SecretsRotation.key", "SecretsRotation.peers.has_key"];
}

/// Internal `tell`: the waiter finished (answered, timed out) — drop its pending entry.
struct Forget(RequestId);

/// How often a waiting request re-reads its row (a peer's answer lands by replication).
const REQUEST_POLL: Duration = Duration::from_secs(1);

/// A waiting request: its session, the `Requested` event (replayed to late
/// subscribers) and the wake-up for a local [`Answer`].
type Waiter = (SessionId, SecretsEvent, oneshot::Sender<()>);

/// The [`SECRETS_ACTOR`] actor: the vault plus the requests waiting on a user.
#[derive(kameo::Actor)]
pub struct SecretsActor {
    vault: Arc<Vault>,
    rt: Runtime,
    timeout: Duration,
    /// This engine's mesh id once the mesh told us ([`SetEngine`]).
    engine: Option<String>,
    pending: HashMap<RequestId, Waiter>,
    /// 221 §4: the external connections, shared with the `tools::secrets` hook.
    externals: Arc<external::Externals>,
    /// 221 §6: configured connections, factories, `secrets.external`.
    connections: connections::Connections,
    /// 221 §7.3: the external write's receipt per answered request, until its tool takes it.
    receipts: HashMap<RequestId, Result<String, String>>,
}

impl SecretsActor {
    /// Build over an open vault; `timeout` is the human wait for [`Request`] and the unlock card.
    pub fn new(vault: Arc<Vault>, rt: Runtime, timeout: Duration) -> Self {
        Self {
            vault,
            externals: Arc::new(external::Externals::new(rt.clone(), timeout)),
            rt,
            timeout,
            engine: None,
            pending: HashMap::new(),
            connections: connections::Connections::default(),
            receipts: HashMap::new(),
        }
    }

    /// 221 §6: the configured connections and `secrets.external` from `cfg`; `config_path` is
    /// where changes persist.
    pub fn with_config(
        mut self,
        cfg: &SecretsConfig,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        self.connections = connections::Connections::from_config(cfg, config_path);
        self
    }

    /// The external connections this actor registers into ([`RegisterExternal`]).
    pub fn externals(&self) -> Arc<external::Externals> {
        self.externals.clone()
    }
}

impl Message<List> for SecretsActor {
    type Reply = SecretsList;

    async fn handle(&mut self, _: List, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        SecretsList {
            key_store: self.vault.key_store().to_owned(),
            secrets: self.vault.list(),
        }
    }
}

impl Message<Set> for SecretsActor {
    type Reply = Result<SecretInfo, String>;

    async fn handle(&mut self, m: Set, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let info = self
            .vault
            .set(&m.name, &m.value, m.note.as_deref(), &m.source)
            .await?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(info)
    }
}

impl Message<Restore> for SecretsActor {
    type Reply = Result<SecretInfo, String>;

    async fn handle(&mut self, m: Restore, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let info = self.vault.restore(&m.name, m.back).await?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(info)
    }
}

impl Message<Delete> for SecretsActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Delete, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.vault.delete(&m.name).await?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(())
    }
}

impl Message<DeleteConfirmed> for SecretsActor {
    type Reply = Result<u64, String>;

    async fn handle(
        &mut self,
        m: DeleteConfirmed,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let n = self.vault.delete_confirmed(&m.name, m.confirm).await?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(n)
    }
}

impl SecretsActor {
    /// The engine id stamped on rows: the mesh id once known, else the hostname.
    fn engine(&self) -> String {
        self.engine
            .clone()
            .unwrap_or_else(|| platform::process::hostname().to_owned())
    }
}

impl Message<Request> for SecretsActor {
    type Reply = DelegatedReply<Result<Outcome, String>>;

    async fn handle(&mut self, m: Request, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.request(m, None, ctx).await
    }
}

impl SecretsActor {
    async fn request(
        &mut self,
        m: Request,
        destination: Option<connections::DestinationView>,
        ctx: &mut Context<Self, DelegatedReply<Result<Outcome, String>>>,
    ) -> DelegatedReply<Result<Outcome, String>> {
        let (reply, tx) = ctx.reply_sender();
        let fail = |tx: Option<kameo::reply::ReplySender<Result<Outcome, String>>>, e: String| {
            if let Some(tx) = tx {
                tx.send(Err(e));
            }
        };
        if m.fields.is_empty() {
            fail(tx, "fields: at least one".into());
            return reply;
        }
        if let Some(bad) = m.fields.iter().find(|f| !vault::valid_name(&f.name)) {
            fail(
                tx,
                format!("invalid secret name {:?}: [A-Za-z0-9_.-]{{1,64}}", bad.name),
            );
            return reply;
        }
        let known = self.vault.list();
        let mut fields: Vec<SecretRequestField> = m
            .fields
            .into_iter()
            .map(|mut f| {
                f.current = known.iter().find(|s| s.name == f.name).map(|s| s.current);
                f
            })
            .collect();
        let id = m.id.unwrap_or_else(|| RequestId(uuid::Uuid::now_v7()));
        let mut title = m.title;
        let mut expires = m.expires;
        let mut destination = destination;
        let mut timeout = self.timeout;
        match requests::get(self.vault.pool(), id).await {
            Ok(Some(row)) => {
                // 109 §6.2: re-arm the stored row; downtime counts (§6.3).
                fields = row.fields;
                title = row.title;
                expires = row.expires;
                destination = row.destination;
                let deadline = row.created_ms.saturating_add(timeout.as_millis() as u64);
                timeout = Duration::from_millis(deadline.saturating_sub(now_ms()));
            }
            Ok(None) => {
                let row = SecretRequest {
                    id,
                    engine: self.engine(),
                    session: m.session.to_string(),
                    title: title.clone(),
                    fields: fields.clone(),
                    created_ms: now_ms(),
                    answered_ms: None,
                    outcome: None,
                    hlc: None,
                    expires: expires.clone(),
                    destination: destination.clone(),
                };
                if let Err(e) = requests::insert(self.vault.pool(), &row).await {
                    fail(tx, format!("secret_requests: {e:#}"));
                    return reply;
                }
            }
            Err(e) => {
                fail(tx, format!("secret_requests: {e:#}"));
                return reply;
            }
        }
        let first = &fields[0];
        let requested = SecretsEvent::Requested {
            id,
            session: m.session,
            name: first.name.clone(),
            note: title.clone().unwrap_or_else(|| first.label.clone()),
            current: first.current,
            title,
            fields: fields.clone(),
            expires,
            destination: destination.map(Box::new),
        };
        let (atx, arx) = oneshot::channel();
        self.pending.insert(id, (m.session, requested.clone(), atx));
        let rt = self.rt.clone();
        let vault = self.vault.clone();
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(requested).await;
            let mut outcome = wait(&vault, id, &fields, arx, timeout).await;
            outcome.external = me.ask(TakeReceipt(id)).await.ok().flatten();
            let _ = me.tell(Forget(id)).await;
            rt.publish(SecretsEvent::Resolved {
                id,
                stored: outcome.stored.first().map(|(_, v)| *v),
                timed_out: outcome.timed_out,
            })
            .await;
            if let Some(tx) = tx {
                tx.send(Ok(outcome));
            }
        });
        reply
    }
}

/// Until the row is answered (here or by a peer, arriving by replication),
/// every field's vault row moved past its `current` (a peer stored them and
/// its `answered_ms` is still in flight), or `timeout` — checked every
/// [`REQUEST_POLL`] and at once when a local [`Answer`] fires `woken`.
async fn wait(
    vault: &Vault,
    id: RequestId,
    fields: &[SecretRequestField],
    woken: oneshot::Receiver<()>,
    timeout: Duration,
) -> Outcome {
    let pool = vault.pool();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut woken = Some(woken);
    let mut timed_out = false;
    let outcome = loop {
        match requests::get(pool, id).await {
            Ok(Some(r)) if r.answered_ms.is_some() => break r.outcome.unwrap_or_default(),
            Ok(_) => {}
            Err(e) => tracing::warn!("secrets: request {id:?}: {e:#}"),
        }
        let known = vault.list();
        let advanced = |f: &SecretRequestField| {
            known
                .iter()
                .find(|s| s.name == f.name)
                .is_some_and(|s| Some(s.current) > f.current)
        };
        if fields.iter().all(advanced) {
            let _ = requests::answer(pool, id, "stored", now_ms()).await;
            continue;
        }
        if tokio::time::Instant::now() >= deadline {
            match requests::answer(pool, id, "timeout", now_ms()).await {
                Ok(true) => {
                    timed_out = true;
                    break "timeout".to_owned();
                }
                // A peer answered meanwhile: the next read has its outcome.
                Ok(false) => continue,
                Err(e) => {
                    tracing::warn!("secrets: request {id:?}: {e:#}");
                    timed_out = true;
                    break "timeout".to_owned();
                }
            }
        }
        let nap =
            tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + REQUEST_POLL));
        match woken.as_mut() {
            Some(rx) => tokio::select! {
                () = nap => {}
                _ = rx => woken = None,
            },
            None => nap.await,
        }
    };
    let stored = if outcome == "stored" {
        // A peer's vault rows may have landed after the last 1 s refresh.
        if let Err(e) = vault.refresh().await {
            tracing::warn!("secrets: refresh: {e:#}");
        }
        let known = vault.list();
        fields
            .iter()
            .filter_map(|f| {
                known
                    .iter()
                    .find(|s| s.name == f.name)
                    .map(|s| (f.name.clone(), s.current))
            })
            .collect()
    } else {
        Vec::new()
    };
    let expires_ms = stored.first().and_then(|(n, _)| vault.expires_ms(n));
    Outcome {
        stored,
        timed_out,
        waived: outcome == PROCEED_WITHOUT,
        expires_ms,
        external: None,
    }
}

/// `ask` → `Result<bool, String>`: [`Answer`] with the card's final `destination` (221 §7.3,
/// `answer_secret`'s fifth argument); `None` keeps the one the request named.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerTo {
    /// The answer.
    pub answer: Answer,
    /// The card's destination.
    pub destination: Option<connections::Destination>,
}

impl Message<AnswerTo> for SecretsActor {
    type Reply = Result<bool, String>;

    async fn handle(&mut self, m: AnswerTo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.answer(m.answer, m.destination).await
    }
}

impl Message<Answer> for SecretsActor {
    type Reply = Result<bool, String>;

    async fn handle(&mut self, m: Answer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.answer(m, None).await
    }
}

impl SecretsActor {
    async fn answer(
        &mut self,
        m: Answer,
        destination: Option<connections::Destination>,
    ) -> Result<bool, String> {
        let vault = self.vault.clone();
        let pool = vault.pool();
        let Some(row) = requests::get(pool, m.id).await.map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        if row.answered_ms.is_some() {
            return Ok(false);
        }
        let dest = destination.or_else(|| row.destination.clone().map(|v| v.destination));
        let backend = match &dest {
            None => None,
            Some(d) => {
                // D15: the engine that owns the asking session writes; values are never staged in
                // a replicated row.
                if row.engine != self.engine() {
                    return Err(format!(
                        "this request stores into {} on engine {}; answer it there",
                        d.connection, row.engine
                    ));
                }
                let b = self
                    .externals
                    .by_conn(&d.connection)
                    .ok_or_else(|| format!("unknown connection {} on this engine", d.connection))?;
                // Not consumed: the user unlocks (or logs in) and answers again.
                match b.status() {
                    external::ConnStatus::Locked => {
                        return Err(format!(
                            "{}/{} is locked; unlock it, then store",
                            b.backend(),
                            d.connection
                        ));
                    }
                    external::ConnStatus::LoggedOut => {
                        return Err(format!(
                            "{}/{} is logged out; log in from the Secrets page, then store",
                            b.backend(),
                            d.connection
                        ));
                    }
                    _ => {}
                }
                Some(b)
            }
        };
        if let Some(t) = m.expires_ms
            && t <= now_ms()
        {
            return Err(format!(
                "the expiry {} is in the past",
                expiry::local_time(t)
            ));
        }
        let mut values = m.values;
        if let Some(v) = m.value
            && let Some(first) = row.fields.first()
        {
            values.entry(first.name.clone()).or_insert(v);
        }
        // Names outside the row's fields are ignored: the card is the form.
        let outcome = if values.is_empty() {
            "declined"
        } else {
            let keep_local = dest.as_ref().is_none_or(|d| d.keep_local);
            if keep_local {
                for f in &row.fields {
                    if let Some(v) = values.get(&f.name) {
                        self.vault
                            .set_expiring(
                                &f.name,
                                v,
                                f.note
                                    .as_deref()
                                    .or((!f.label.is_empty()).then_some(f.label.as_str())),
                                "user",
                                m.expires_ms,
                            )
                            .await?;
                    }
                }
                self.rt.publish(SecretsEvent::Changed).await;
            }
            match (&dest, backend) {
                (Some(d), Some(b)) => {
                    let r = match self.store_external(b, d, &row.fields, &values).await {
                        Ok(t) => Ok(t),
                        // R11: a transient failure leaves the request open; the card stays
                        // and the user answers again.
                        Err(Failure { text, retry: true }) => {
                            let kept = if keep_local {
                                "; the local copy was kept"
                            } else {
                                ""
                            };
                            return Err(format!(
                                "{text}{kept}; the request is still open: try again"
                            ));
                        }
                        Err(Failure { text, .. }) => Err(text),
                    };
                    let r = match (r, keep_local) {
                        (Ok(t), true) => Ok(format!("{t}; a local copy is kept too")),
                        (Err(e), true) => Err(format!("{e}; the local copy was kept")),
                        (r, false) => r,
                    };
                    let failed = r.is_err();
                    self.receipts.insert(m.id, r);
                    if failed { "failed" } else { "stored" }
                }
                _ => "stored",
            }
        };
        let fresh = requests::answer(pool, m.id, outcome, now_ms())
            .await
            .map_err(|e| e.to_string())?;
        if let Some((_, _, tx)) = self.pending.remove(&m.id) {
            let _ = tx.send(());
        }
        match self.receipts.get(&m.id) {
            Some(Err(e)) => Err(e.clone()),
            _ => Ok(fresh),
        }
    }

    /// 221 §7.3: write an answer into `d` on `b` (create, or update `d.replace`). The values pass
    /// through once and are dropped; the receipt names where they went, never what they are.
    async fn store_external(
        &self,
        b: Arc<dyn ExternalBackend>,
        d: &connections::Destination,
        fields: &[SecretRequestField],
        values: &HashMap<String, String>,
    ) -> Result<String, Failure> {
        let mapped = |f: &SecretRequestField| {
            d.fields
                .as_ref()
                .and_then(|m| m.get(&f.name))
                .cloned()
                .unwrap_or_default()
        };
        let typed: Vec<&SecretRequestField> = fields
            .iter()
            .filter(|f| values.contains_key(&f.name))
            .collect();
        // The value field: the first not mapped to `username` (Bitwarden), else the first.
        let main = typed
            .iter()
            .find(|f| mapped(f) != "username")
            .or(typed.first())
            .ok_or_else(|| Failure::fatal("nothing to store"))?;
        let json = matches!(d.format, Some(connections::AwsFormat::Json));
        let pairs: Vec<(&str, &str)> = typed
            .iter()
            .map(|f| (f.name.as_str(), values[&f.name].as_str()))
            .collect();
        let value = if json && b.backend() == "aws" {
            external::Value::Json(pairs)
        } else {
            external::Value::Plain(values[&main.name].as_str())
        };
        let timeout = Duration::from_secs(self.connections.external.timeout_secs);
        if let Some(replace) = &d.replace {
            let (_, r) = external::parse_bare(replace)
                .ok_or_else(|| Failure::fatal(format!("{replace:?} is not a reference")))?;
            let v = settle(
                replace,
                timeout,
                tokio::time::timeout(timeout, b.update(&r, value, None)).await,
            )?;
            return Ok(format!(
                "stored in {replace} (version {}); use {{{{secret:{replace}}}}}",
                v.id
            ));
        }
        let scope = self
            .connections
            .conns
            .get(&d.connection)
            .map(|c| c.settings.write_scope)
            .unwrap_or_default();
        let mut loc =
            tools_external::location(b.backend(), d, &main.name, scope).map_err(Failure::fatal)?;
        if let external::Location::Bw { username, .. } = &mut loc {
            *username = typed
                .iter()
                .find(|f| mapped(f) == "username")
                .map(|f| values[&f.name].clone());
        }
        let what = format!("{}/{}/{}", b.backend(), d.connection, d.name);
        let (r, v) = settle(
            &what,
            timeout,
            tokio::time::timeout(timeout, b.create(&loc, value)).await,
        )?;
        let t = tools_external::ref_text(b.backend(), &r);
        Ok(format!(
            "stored in {t} (version {}); use {{{{secret:{t}}}}}",
            v.id
        ))
    }
}

/// Why [`SecretsActor::store_external`] did not store: the text, and whether the request stays
/// open for another answer (network, timeout, locked, conflict).
struct Failure {
    text: String,
    retry: bool,
}

impl Failure {
    fn fatal(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            retry: false,
        }
    }
}

/// A bounded backend call's result as model-facing text naming `what`.
fn settle<T>(
    what: &str,
    timeout: Duration,
    r: Result<Result<T, external::ExtError>, tokio::time::error::Elapsed>,
) -> Result<T, Failure> {
    use external::ExtError as E;
    match r {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(Failure {
            retry: matches!(
                e,
                E::Network(_) | E::Timeout | E::Locked | E::Conflict { .. }
            ),
            text: format!("{what}: {e}"),
        }),
        Err(_) => Err(Failure {
            text: format!(
                "{what}: the manager did not answer in {} s",
                timeout.as_secs()
            ),
            retry: true,
        }),
    }
}

/// `ask` → `Option<Result<String, String>>`: take the external receipt of request `id` (221 §7.3).
struct TakeReceipt(RequestId);

impl Message<TakeReceipt> for SecretsActor {
    type Reply = Option<Result<String, String>>;

    async fn handle(&mut self, m: TakeReceipt, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.receipts.remove(&m.0)
    }
}

impl Message<Waive> for SecretsActor {
    type Reply = Result<bool, String>;

    async fn handle(&mut self, m: Waive, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let fresh = requests::answer(self.vault.pool(), m.id, PROCEED_WITHOUT, now_ms())
            .await
            .map_err(|e| e.to_string())?;
        if fresh && let Some((_, _, tx)) = self.pending.remove(&m.id) {
            let _ = tx.send(());
        }
        Ok(fresh)
    }
}

impl Message<Forget> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, m: Forget, _: &mut Context<Self, Self::Reply>) {
        self.pending.remove(&m.0);
    }
}

impl Message<SetEngine> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, m: SetEngine, _: &mut Context<Self, Self::Reply>) {
        self.vault.set_engine(&m.0);
        self.engine = Some(m.0);
    }
}

/// `ask` → `Result<Option<String>, String>`: D15 — the engine an answer to request `id` must be
/// forwarded to: `Some(engine)` when the row is unanswered, stores into an external manager (its
/// own destination, or `destination` = the answer names one) and was asked on another engine.
pub struct ForeignOwner {
    /// The request.
    pub id: RequestId,
    /// The answer carries a destination.
    pub destination: bool,
}

impl Message<ForeignOwner> for SecretsActor {
    type Reply = Result<Option<String>, String>;

    async fn handle(&mut self, m: ForeignOwner, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let row = requests::get(self.vault.pool(), m.id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(row
            .filter(|r| {
                r.answered_ms.is_none()
                    && (m.destination || r.destination.is_some())
                    && r.engine != self.engine()
            })
            .map(|r| r.engine))
    }
}

impl Message<ListRequests> for SecretsActor {
    type Reply = Result<Vec<SecretRequest>, String>;

    async fn handle(&mut self, m: ListRequests, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        requests::list(self.vault.pool(), m.unanswered_only)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The secrets actor; `None` before the plugin started or when the vault failed to open.
pub fn actor() -> Option<kameo::actor::ActorRef<SecretsActor>> {
    kameo::actor::ActorRef::<SecretsActor>::lookup(SECRETS_ACTOR)
        .ok()
        .flatten()
}

/// `ask` → the still-pending [`SecretsEvent::Requested`]s, for the
/// `subscribe_events` snapshot a client gets on (re)subscribe (31 §7.3).
pub struct PendingRequests;

impl Message<PendingRequests> for SecretsActor {
    type Reply = Vec<SecretsEvent>;

    async fn handle(
        &mut self,
        _: PendingRequests,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.pending.values().map(|(_, r, _)| r.clone()).collect()
    }
}

impl Message<Handle> for SecretsActor {
    type Reply = Arc<Vault>;

    async fn handle(&mut self, _: Handle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.vault.clone()
    }
}

impl Message<GetRotation> for SecretsActor {
    type Reply = Option<RotationState>;

    async fn handle(&mut self, _: GetRotation, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.vault.rotation()
    }
}

/// How long [`handle`] waits for the [`SECRETS_ACTOR`] to appear.
pub const HANDLE_WAIT: Duration = Duration::from_secs(10);

/// The vault for in-process readers (37 §3.7): waits for the actor up to
/// [`HANDLE_WAIT`] (plugins start in order, so it is registered or never will
/// be), `None` when the vault failed to open. Callers proceed without keys.
pub async fn handle(rt: &Runtime) -> Option<Arc<Vault>> {
    let actor = tokio::time::timeout(HANDLE_WAIT, rt.lookup_when::<SecretsActor>(SECRETS_ACTOR))
        .await
        .ok()
        .flatten()?;
    match actor.ask(Handle).await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("secrets vault handle: {e}");
            None
        }
    }
}

/// The mesh's Noise static private key (69 decision 1): the `mesh/device_key`
/// row, created on first use. `Err` when the vault is unavailable. The caller
/// (link) derives the keypair; secrets cannot name `link::Identity` without a
/// dependency cycle.
pub async fn mesh_device_key(rt: &Runtime) -> anyhow::Result<zeroize::Zeroizing<[u8; 32]>> {
    let vault = handle(rt)
        .await
        .ok_or_else(|| anyhow::anyhow!("secrets vault unavailable"))?;
    vault.mesh_device_key().await
}

/// [`Vault::mesh_device_key_if_set`] for the runtime: the row when it exists, never minted.
/// `Err` when the vault is unavailable.
pub async fn mesh_device_key_existing(
    rt: &Runtime,
) -> anyhow::Result<Option<zeroize::Zeroizing<[u8; 32]>>> {
    let vault = handle(rt)
        .await
        .ok_or_else(|| anyhow::anyhow!("secrets vault unavailable"))?;
    vault.mesh_device_key_if_set()
}

/// OAuth grants are vault rows `oauth/<ns>` (67 §5.4): the JSON is the
/// value, written under the [`Vault::set_if_version`] precondition.
#[async_trait::async_trait]
impl ::oauth::Grants for Vault {
    fn load(&self, ns: &str) -> Option<(u32, String)> {
        let (v, json) = Vault::get_versioned(self, &format!("{OAUTH_PREFIX}{ns}"))?;
        Some((v, json.to_string()))
    }

    async fn store(&self, ns: &str, json: &str, expected: Option<u32>) -> Result<bool, String> {
        Vault::set_if_version(
            self,
            &format!("{OAUTH_PREFIX}{ns}"),
            json,
            "oauth",
            expected,
        )
        .await
        .map(|written| written.is_some())
    }

    async fn remove(&self, ns: &str) -> Result<(), String> {
        match Vault::delete(self, &format!("{OAUTH_PREFIX}{ns}")).await {
            Err(e) if e.starts_with("unknown secret") => Ok(()),
            r => r,
        }
    }
}

/// Declare secrets wire metadata, never secret values, before opening the vault.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<SecretsEvent>().await?;
    Ok(())
}

/// The plugin (37 §3.3, 67 §5.4): reads `secrets`, opens the vault (migrating
/// `secrets.vault`, importing `credentials.yaml`), spawns [`SecretsActor`] as
/// [`SECRETS_ACTOR`], attaches `vault` + `vault_current` to replication once
/// the replica is up, refreshes the in-memory copy whenever the file moves,
/// installs the `tools::secrets` hook and registers the `secret_*` tools
/// once the registry is up.
pub struct SecretsPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for SecretsPlugin {
    fn name(&self) -> &'static str {
        "secrets"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: SecretsConfig = if rt.config.contains("secrets") {
            rt.section("secrets")?
        } else {
            SecretsConfig::default()
        };
        let vault = match Vault::open(&rt.data_dir, cfg.key_store).await {
            Ok(v) => Arc::new(v),
            Err(e) => {
                tracing::error!("secrets vault unavailable: {e:#}");
                return Ok(());
            }
        };
        let sa = SecretsActor::new(
            vault.clone(),
            rt.clone(),
            Duration::from_secs(cfg.request_timeout_secs),
        )
        .with_config(&cfg, Some(rt.data_dir.join("config.yaml")));
        let externals = sa.externals();
        let actor = SecretsActor::spawn(sa);
        actor.register(SECRETS_ACTOR)?;
        tokio::spawn(replicate(rt.clone(), vault.clone()));
        tokio::spawn(sweeper(rt.clone(), vault.clone()));
        if !cfg.tools {
            // Nothing reads placeholders: the connections serve the owner's UI only.
            let _ = actor.tell(connections::SeedConnections).await;
            return Ok(());
        }
        ::tools::secrets::install(Arc::new(external::Hook::new(
            vault.clone(),
            externals,
            Duration::from_secs(cfg.external.timeout_secs),
        )));
        let rt = rt.clone();
        let timeout = Duration::from_secs(cfg.request_timeout_secs);
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                // 221 §6.3: the always-ask entries exist before any connection is registered.
                let _ = actor.ask(connections::SeedConnections).await;
                tools::register(&reg, vault, actor, timeout).await;
            }
        });
        Ok(())
    }
}

/// Attach the vault tables once the replica plugin is up (`Err` = local
/// only), then refresh the in-memory copy every [`REFRESH_POLL`] the file
/// moved, publishing [`SecretsEvent::Changed`] so lists redraw.
async fn replicate(rt: Runtime, vault: Arc<Vault>) {
    // The replica may never start (a slimmer plugin set): do not hold the
    // refresh loop on it.
    let (rt2, v2) = (rt.clone(), vault.clone());
    tokio::spawn(async move {
        if rt2
            .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
            .await
            .is_none()
        {
            return;
        }
        match replica::attach(v2.pool(), store::TABLES).await {
            // Rows written before the attach (the vault predates 67; the mesh key row can land
            // before the replica is up) never fired a trigger: announce them whole once, else a
            // later column write ships alone and trips NOT NULL on every peer (69 smoke).
            Ok(h) => match h.announce_unstamped().await {
                Ok(n) => {
                    tracing::info!("secrets: vault replicates ({n} pre-existing row(s) announced)")
                }
                Err(e) => {
                    tracing::warn!("secrets: vault replicates; existing rows not announced: {e:#}")
                }
            },
            Err(e) => tracing::warn!("secrets: vault is local only: {e:#}"),
        }
    });
    // 183 §10.4: every change of the rotation record goes on the bus.
    let (rt2, mut rotation) = (rt.clone(), vault.watch_rotation());
    tokio::spawn(async move {
        while rotation.changed().await.is_ok() {
            let state = rotation.borrow_and_update().clone();
            if let Some(s) = state {
                rt2.publish(SecretsEvent::Rotation(s)).await;
            }
        }
    });
    let mut seen = vault.disk_stamp();
    let mut tick = tokio::time::interval(REFRESH_POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n = 0u64;
    loop {
        tick.tick().await;
        // 183 §6.9.6: sweep and retire, on boot and then every MAINTAIN_EVERY ticks.
        n = n.wrapping_add(1);
        if n % MAINTAIN_EVERY == 1
            && let Err(e) = vault.maintain(now_ms()).await
        {
            tracing::warn!("secrets: vault-key sweep: {e:#}");
        }
        let now = vault.disk_stamp();
        if now == seen {
            continue;
        }
        seen = now;
        match vault.refresh().await {
            Ok(()) => rt.publish(SecretsEvent::Changed).await,
            Err(e) => tracing::warn!("secrets: refresh: {e:#}"),
        }
    }
}

/// [`REFRESH_POLL`] ticks between two [`Vault::maintain`] runs.
const MAINTAIN_EVERY: u64 = 5;

/// Longest the expiry sweeper sleeps without looking again (a wall-clock jump, a far expiry).
const SWEEP_MAX_NAP: Duration = Duration::from_secs(3600);
/// Shortest: a delete that keeps failing must not spin.
const SWEEP_MIN_NAP: Duration = Duration::from_millis(250);

/// Delete expired secrets on start, then on a timer armed for the soonest expiry and
/// re-armed whenever the vault changes (an own write, a peer's rows). Runs whether or not
/// the agent tools are on: an expiry the user set is kept regardless.
async fn sweeper(rt: Runtime, vault: Arc<Vault>) {
    let mut changes = vault.watch_changes();
    loop {
        changes.mark_unchanged();
        sweep(&rt, &vault, now_ms()).await;
        let nap = vault.next_expiry().map_or(SWEEP_MAX_NAP, |t| {
            Duration::from_millis(t.saturating_sub(now_ms())).clamp(SWEEP_MIN_NAP, SWEEP_MAX_NAP)
        });
        tokio::select! {
            () = tokio::time::sleep(nap) => {}
            r = changes.changed() => if r.is_err() { return },
            () = rt.shutdown.cancelled() => return,
        }
    }
}

/// Delete every secret whose current version expired at or before `now`, every version,
/// through [`Vault::delete_expired`] (the rows `/secrets rm` deletes, so the deletion
/// replicates), log one line per name (never a value) and publish one
/// [`SecretsEvent::Changed`] when anything went. Returns how many were deleted.
pub async fn sweep(rt: &Runtime, vault: &Vault, now: u64) -> usize {
    let mut n = 0;
    for name in vault.expired(now) {
        match vault.delete_expired(&name, now).await {
            Ok(true) => {
                tracing::info!("secrets: {name} expired; deleted with every version");
                n += 1;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("secrets: deleting expired {name}: {e}"),
        }
    }
    if n > 0 {
        rt.publish(SecretsEvent::Changed).await;
    }
    n
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kameo::actor::ActorRef;

    use super::*;

    fn field(name: &str, label: &str) -> SecretRequestField {
        SecretRequestField {
            name: name.into(),
            label: label.into(),
            masked: true,
            current: None,
            note: None,
        }
    }

    async fn actor(dir: &Path, timeout: Duration) -> (ActorRef<SecretsActor>, Arc<Vault>) {
        let rt = Runtime::new(figment::Figment::new(), dir.to_path_buf());
        let vault = Arc::new(Vault::open(dir, KeyStore::File).await.unwrap());
        let a = SecretsActor::spawn(SecretsActor::new(vault.clone(), rt, timeout));
        (a, vault)
    }

    /// The `Request` is delegated: ask it from a task and wait for its row to appear.
    async fn open_request(
        a: &ActorRef<SecretsActor>,
        title: Option<&str>,
        fields: Vec<SecretRequestField>,
    ) -> (
        tokio::task::JoinHandle<Result<Outcome, String>>,
        SecretRequest,
    ) {
        let a2 = a.clone();
        let title = title.map(Into::into);
        let ask = tokio::spawn(async move {
            a2.ask(Request {
                session: SessionId::new(),
                title,
                fields,
                id: None,
                expires: None,
            })
            .await
            .map_err(|e| e.to_string())
        });
        let row = loop {
            let rows = a
                .ask(ListRequests {
                    unanswered_only: true,
                })
                .await
                .unwrap();
            if let Some(r) = rows.into_iter().next() {
                break r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        (ask, row)
    }

    #[tokio::test]
    async fn single_request_round_trip_through_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let (ask, row) = open_request(&a, None, vec![field("TOKEN", "GitHub PAT")]).await;
        assert_eq!(row.title, None);
        assert_eq!(row.fields, [field("TOKEN", "GitHub PAT")]);
        assert_eq!(
            row.engine,
            platform::process::hostname(),
            "no mesh id told yet"
        );

        let pending = a.ask(PendingRequests).await.unwrap();
        match &pending[..] {
            [
                SecretsEvent::Requested {
                    id,
                    name,
                    note,
                    title,
                    fields,
                    ..
                },
            ] => {
                assert_eq!(*id, row.id);
                assert_eq!((name.as_str(), note.as_str()), ("TOKEN", "GitHub PAT"));
                assert_eq!(*title, None);
                assert_eq!(fields.len(), 1, "the single field is in `fields` too");
            }
            other => panic!("pending: {other:?}"),
        }

        let answer = Answer {
            id: row.id,
            value: Some("ghp_secret_value".into()),
            values: HashMap::new(),
            expires_ms: None,
        };
        assert!(a.ask(answer.clone()).await.unwrap());
        let out = ask.await.unwrap().unwrap();
        assert_eq!(out.stored, [("TOKEN".to_owned(), 1)]);
        assert!(!out.timed_out);
        assert_eq!(
            vault.get("TOKEN", 0).map(|v| v.to_string()),
            Some("ghp_secret_value".into())
        );
        let after = requests::get(vault.pool(), row.id).await.unwrap().unwrap();
        assert!(after.answered_ms.is_some());
        assert_eq!(after.outcome.as_deref(), Some("stored"));
        assert!(
            a.ask(ListRequests {
                unanswered_only: true,
            })
            .await
            .unwrap()
            .is_empty()
        );
        assert!(a.ask(PendingRequests).await.unwrap().is_empty());
        assert!(
            !a.ask(answer).await.unwrap(),
            "a second answer is not fresh"
        );
    }

    #[tokio::test]
    async fn form_stores_every_field_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        a.tell(SetEngine("engine-a".into())).await.unwrap();
        vault.set("gh.pass", "old", None, "user").await.unwrap();
        let (ask, row) = open_request(
            &a,
            Some("GitHub"),
            vec![
                field("gh.user", "Username"),
                field("gh.pass", "Password"),
                field("gh.totp", "One-time code"),
            ],
        )
        .await;
        assert_eq!(row.engine, "engine-a");
        assert_eq!(row.title.as_deref(), Some("GitHub"));
        assert_eq!(
            row.fields.iter().map(|f| f.current).collect::<Vec<_>>(),
            [None, Some(1), None],
            "`current` filled per field"
        );
        match &a.ask(PendingRequests).await.unwrap()[..] {
            [SecretsEvent::Requested { name, note, .. }] => {
                assert_eq!((name.as_str(), note.as_str()), ("gh.user", "GitHub"));
            }
            other => panic!("pending: {other:?}"),
        }
        let values = HashMap::from([
            ("gh.totp".to_owned(), "123456".to_owned()),
            ("gh.user".to_owned(), "alice".to_owned()),
            ("gh.pass".to_owned(), "hunter22".to_owned()),
            ("not.asked".to_owned(), "ignored".to_owned()),
        ]);
        assert!(
            a.ask(Answer {
                id: row.id,
                value: None,
                values,
                expires_ms: None,
            })
            .await
            .unwrap()
        );
        let out = ask.await.unwrap().unwrap();
        assert_eq!(
            out.stored,
            [
                ("gh.user".to_owned(), 1),
                ("gh.pass".to_owned(), 2),
                ("gh.totp".to_owned(), 1)
            ]
        );
        assert_eq!(
            vault.get("gh.pass", 0).map(|v| v.to_string()),
            Some("hunter22".into())
        );
        assert_eq!(
            vault.get("gh.pass", 1).map(|v| v.to_string()),
            Some("old".into())
        );
        assert!(vault.get("not.asked", 0).is_none());
        let pass = vault
            .list()
            .into_iter()
            .find(|s| s.name == "gh.pass")
            .unwrap();
        assert_eq!(pass.note, "Password", "the label is the note");
    }

    #[tokio::test]
    async fn decline_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let (ask, row) = open_request(&a, None, vec![field("TOKEN", "x")]).await;
        assert!(
            a.ask(Answer {
                id: row.id,
                value: None,
                values: HashMap::new(),
                expires_ms: None,
            })
            .await
            .unwrap()
        );
        let out = ask.await.unwrap().unwrap();
        assert!(out.stored.is_empty() && !out.timed_out);
        assert!(vault.get("TOKEN", 0).is_none());
        let after = requests::get(vault.pool(), row.id).await.unwrap().unwrap();
        assert_eq!(after.outcome.as_deref(), Some("declined"));
    }

    /// 207: `Waive` resolves a waiting request as `proceed_without` and wakes its waiter;
    /// an answered or unknown request is not fresh.
    #[tokio::test]
    async fn waive_proceeds_without() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let (ask, row) = open_request(&a, Some("f"), vec![field("A", "a"), field("B", "b")]).await;
        assert!(a.ask(Waive { id: row.id }).await.unwrap());
        let out = tokio::time::timeout(Duration::from_millis(500), ask)
            .await
            .expect("the waiter was not woken")
            .unwrap()
            .unwrap();
        assert!(out.waived && !out.timed_out && out.stored.is_empty());
        let after = requests::get(vault.pool(), row.id).await.unwrap().unwrap();
        assert!(after.answered_ms.is_some());
        assert_eq!(after.outcome.as_deref(), Some(PROCEED_WITHOUT));
        assert!(a.ask(PendingRequests).await.unwrap().is_empty());
        assert!(
            !a.ask(Waive { id: row.id }).await.unwrap(),
            "an answered request is not fresh"
        );
        assert!(
            !a.ask(Waive {
                id: RequestId(uuid::Uuid::from_u128(9)),
            })
            .await
            .unwrap(),
            "an unknown request is not fresh"
        );
    }

    /// A peer's `answer_secret` reaches this engine as rows only: the vault row
    /// and the request row's `answered_ms`. No `Answer` message here.
    #[tokio::test]
    async fn a_peer_answer_arriving_as_rows_finishes_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let (ask, row) = open_request(&a, None, vec![field("TOKEN", "x")]).await;
        vault.set("TOKEN", "from-peer", None, "user").await.unwrap();
        assert!(
            requests::answer(vault.pool(), row.id, "stored", 1)
                .await
                .unwrap()
        );
        let out = ask.await.unwrap().unwrap();
        assert_eq!(out.stored, [("TOKEN".to_owned(), 1)]);
        assert!(a.ask(PendingRequests).await.unwrap().is_empty());
    }

    /// The `answered_ms` may lag the vault rows: every field advancing past its `current` is enough.
    #[tokio::test]
    async fn vault_rows_alone_finish_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        vault.set("A", "a1", None, "user").await.unwrap();
        let (ask, row) = open_request(&a, Some("f"), vec![field("A", "a"), field("B", "b")]).await;
        vault.set("A", "a2", None, "user").await.unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            requests::get(vault.pool(), row.id)
                .await
                .unwrap()
                .unwrap()
                .answered_ms
                .is_none(),
            "one of two fields is not enough"
        );
        vault.set("B", "b1", None, "user").await.unwrap();
        let out = ask.await.unwrap().unwrap();
        assert_eq!(out.stored, [("A".to_owned(), 2), ("B".to_owned(), 1)]);
        let after = requests::get(vault.pool(), row.id).await.unwrap().unwrap();
        assert_eq!(
            after.outcome.as_deref(),
            Some("stored"),
            "the row is closed for every peer"
        );
    }

    #[tokio::test]
    async fn timeout_closes_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_millis(1500)).await;
        let (ask, row) = open_request(&a, None, vec![field("TOKEN", "x")]).await;
        let out = ask.await.unwrap().unwrap();
        assert!(out.timed_out && out.stored.is_empty());
        let after = requests::get(vault.pool(), row.id).await.unwrap().unwrap();
        assert_eq!(after.outcome.as_deref(), Some("timeout"));
        assert!(
            !a.ask(Answer {
                id: row.id,
                value: Some("late".into()),
                values: HashMap::new(),
                expires_ms: None,
            })
            .await
            .unwrap(),
            "a late answer is refused"
        );
        assert!(vault.get("TOKEN", 0).is_none());
    }

    /// A bus subscriber that forwards [`SecretsEvent`]s to a channel.
    #[derive(kameo::Actor)]
    struct Tap(tokio::sync::mpsc::UnboundedSender<SecretsEvent>);

    impl Message<SecretsEvent> for Tap {
        type Reply = ();

        async fn handle(&mut self, e: SecretsEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(e);
        }
    }

    async fn tap(rt: &Runtime) -> tokio::sync::mpsc::UnboundedReceiver<SecretsEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let t = Tap::spawn(Tap(tx));
        rt.subscribe::<Tap, SecretsEvent>(&t).await;
        rx
    }

    #[tokio::test]
    async fn an_answer_with_an_expiry_sets_it_on_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let mut pass = field("gh.pass", "Password");
        pass.note = Some("alice's GitHub password".into());
        let (ask, row) =
            open_request(&a, Some("GitHub"), vec![field("gh.user", "Username"), pass]).await;
        let at = now_ms() + 3_600_000;
        let values = HashMap::from([
            ("gh.user".to_owned(), "alice".to_owned()),
            ("gh.pass".to_owned(), "hunter22".to_owned()),
        ]);
        assert!(
            a.ask(Answer {
                id: row.id,
                value: None,
                values,
                expires_ms: Some(at),
            })
            .await
            .unwrap()
        );
        let out = ask.await.unwrap().unwrap();
        assert_eq!(
            out.stored,
            [("gh.user".to_owned(), 1), ("gh.pass".to_owned(), 1)]
        );
        assert_eq!(out.expires_ms, Some(at), "the receipt names the expiry");
        assert_eq!(vault.expires_ms("gh.user"), Some(at));
        assert_eq!(vault.expires_ms("gh.pass"), Some(at));
        let listed = vault.list();
        let pass = listed.iter().find(|s| s.name == "gh.pass").unwrap();
        assert_eq!(pass.expires.as_deref(), Some(expiry::rfc3339(at).as_str()));
        assert_eq!(
            pass.note, "alice's GitHub password",
            "a field's note is the secret's note"
        );
        let user = listed.iter().find(|s| s.name == "gh.user").unwrap();
        assert_eq!(user.note, "Username", "no note: the label");
    }

    #[tokio::test]
    async fn an_answer_with_a_past_expiry_is_refused_and_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (a, vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let (_ask, row) = open_request(&a, None, vec![field("TOKEN", "x")]).await;
        let err = a
            .ask(Answer {
                id: row.id,
                value: Some("value-for-token".into()),
                values: HashMap::new(),
                expires_ms: Some(now_ms() - 1),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("in the past"), "{err}");
        assert!(!err.contains("value-for-token"));
        assert!(vault.get("TOKEN", 0).is_none());
        let after = requests::get(vault.pool(), row.id).await.unwrap().unwrap();
        assert!(
            after.answered_ms.is_none(),
            "the card can still be answered"
        );
    }

    #[tokio::test]
    async fn the_agents_suggestion_rides_the_row_and_the_event() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _vault) = actor(dir.path(), Duration::from_secs(10)).await;
        let a2 = a.clone();
        tokio::spawn(async move {
            a2.ask(Request {
                session: SessionId::new(),
                title: None,
                fields: vec![field("TOKEN", "x")],
                id: None,
                expires: Some("2h".into()),
            })
            .await
        });
        let row = loop {
            let rows = a
                .ask(ListRequests {
                    unanswered_only: true,
                })
                .await
                .unwrap();
            if let Some(r) = rows.into_iter().next() {
                break r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(row.expires.as_deref(), Some("2h"));
        match &a.ask(PendingRequests).await.unwrap()[..] {
            [SecretsEvent::Requested { expires, .. }] => assert_eq!(expires.as_deref(), Some("2h")),
            other => panic!("pending: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_plain_replace_clears_the_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path(), KeyStore::File).await.unwrap();
        let at = now_ms() + 60_000;
        vault
            .set_expiring("TOKEN", "value-one-long", None, "user", Some(at))
            .await
            .unwrap();
        assert_eq!(vault.expires_ms("TOKEN"), Some(at));
        let same = vault
            .set_expiring("TOKEN", "value-one-long", None, "user", Some(at))
            .await
            .unwrap();
        assert_eq!(same.current, 1, "same value, same expiry: nothing written");
        let info = vault
            .set("TOKEN", "value-one-long", None, "user")
            .await
            .unwrap();
        assert_eq!(
            info.current, 2,
            "the same value without the expiry is a new version"
        );
        assert_eq!(info.expires, None);
        assert_eq!(vault.expires_ms("TOKEN"), None);
        assert_eq!(vault.next_expiry(), None);
    }

    #[tokio::test]
    async fn an_expired_placeholder_fails_before_the_sweeper_runs() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path(), KeyStore::File).await.unwrap();
        vault
            .set("TOKEN", "older-value-long", None, "user")
            .await
            .unwrap();
        let at = now_ms() - 1;
        vault
            .set_expiring("TOKEN", "newer-value-long", None, "user", Some(at))
            .await
            .unwrap();
        let want = format!("secret TOKEN expired at {}", expiry::local_time(at));
        for p in ["{{secret:TOKEN}}", "{{secret:TOKEN:-1}}"] {
            let mut args = serde_json::json!({ "cmd": format!("curl -H {p}") });
            assert_eq!(vault.expand(&mut args).unwrap_err(), want, "{p}");
        }
        let mut other = serde_json::json!({"cmd": "echo hi"});
        assert_eq!(
            vault.expand(&mut other).unwrap(),
            0,
            "no placeholder, no error"
        );
    }

    #[tokio::test]
    async fn sweep_deletes_every_version_and_announces_it() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let mut events = tap(&rt).await;
        let vault = Vault::open(dir.path(), KeyStore::File).await.unwrap();
        let now = now_ms();
        vault
            .set("GONE", "gone-value-one", None, "user")
            .await
            .unwrap();
        vault
            .set_expiring("GONE", "gone-value-two", None, "user", Some(now - 1))
            .await
            .unwrap();
        vault
            .set_expiring("LATER", "later-value-x", None, "user", Some(now + 60_000))
            .await
            .unwrap();
        vault
            .set("KEPT", "kept-value-xx", None, "user")
            .await
            .unwrap();
        assert_eq!(vault.next_expiry(), Some(now - 1));

        assert_eq!(sweep(&rt, &vault, now).await, 1);
        let names: Vec<_> = vault.list().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["KEPT", "LATER"]);
        assert!(vault.get("GONE", 1).is_none());
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM vault WHERE name = 'GONE'")
            .fetch_one(vault.pool())
            .await
            .unwrap();
        assert_eq!(rows, 0, "every version's row is gone");
        let ev = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ev, SecretsEvent::Changed), "{ev:?}");
        assert_eq!(sweep(&rt, &vault, now).await, 0, "nothing left to sweep");

        // A replace that lands between the scan and the delete keeps the secret.
        vault
            .set_expiring("RACE", "race-value-xx", None, "user", Some(now - 1))
            .await
            .unwrap();
        vault
            .set("RACE", "race-value-new", None, "user")
            .await
            .unwrap();
        assert!(!vault.delete_expired("RACE", now).await.unwrap());
        assert!(vault.get("RACE", 0).is_some());
    }

    #[tokio::test]
    async fn the_sweeper_task_arms_for_a_new_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let mut events = tap(&rt).await;
        let vault = Arc::new(Vault::open(dir.path(), KeyStore::File).await.unwrap());
        vault
            .set_expiring("BOOT", "boot-value-xx", None, "user", Some(now_ms() - 1))
            .await
            .unwrap();
        let task = tokio::spawn(sweeper(rt.clone(), vault.clone()));
        // On start: the already-expired one goes.
        let ev = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap();
        assert!(matches!(ev, Some(SecretsEvent::Changed)));
        assert!(vault.get("BOOT", 0).is_none());
        // Written after the sweeper went to sleep with nothing to wait for: the change re-arms it.
        vault
            .set_expiring("SOON", "soon-value-xx", None, "user", Some(now_ms() + 400))
            .await
            .unwrap();
        let ev = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap();
        assert!(matches!(ev, Some(SecretsEvent::Changed)));
        assert!(vault.get("SOON", 0).is_none());
        rt.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
}
