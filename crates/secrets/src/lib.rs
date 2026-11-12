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
//! - [`requests`]: the replicated `secret_requests` rows (79 §6.6).
//! - [`tools`]: the five `secret_*` tools and the prompt block.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`SECRETS_ACTOR`] actor ([`SecretsActor`]): [`List`],
//!   [`Set`], [`Restore`], [`Delete`], [`Request`], [`Answer`], [`PendingRequests`],
//!   [`ListRequests`], [`SetEngine`], [`Handle`]; 183: [`GetRotation`] (the mesh reaches the
//!   keyring through [`Handle`] and the [`Vault`] methods directly)
//! - events: [`SecretsEvent`]
//! - [`Vault`] as a library, for `t --secret-set` and `--slack-setup`

mod key;
pub mod requests;
pub mod store;
pub mod tools;
pub mod vault;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

pub use vault::{
    KEEP, KeyInfo, KeyStore, MESH_PREFIX, OAUTH_PREFIX, RetireHold, RotationPeer, RotationPhase,
    RotationState, SecretInfo, SecretsList, Vault, VersionInfo, WireKey,
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
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            key_store: KeyStore::Auto,
            tools: true,
            request_timeout_secs: 1800,
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
    /// `stored` | `declined` | `timeout` once answered.
    pub outcome: Option<String>,
    /// Replication clock.
    pub hlc: Option<String>,
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
/// only; the model has no delete. `Err` = unknown name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delete {
    /// The secret.
    pub name: String,
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
}

/// What [`Request`] resolves to: the new version per field when stored, else why not.
#[derive(Clone, Debug, kameo::Reply)]
pub struct Outcome {
    /// `(name, new version)` per field the user typed a value for; empty when declined or timed out.
    pub stored: Vec<(String, u32)>,
    /// Nobody answered in time.
    pub timed_out: bool,
}

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
}

impl wire::WireEvent for SecretsEvent {
    const TOPIC: &'static str = "secrets.events";
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
}

impl SecretsActor {
    /// Build over an open vault; `timeout` is the human wait for [`Request`].
    pub fn new(vault: Arc<Vault>, rt: Runtime, timeout: Duration) -> Self {
        Self {
            vault,
            rt,
            timeout,
            engine: None,
            pending: HashMap::new(),
        }
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
        let mut timeout = self.timeout;
        match requests::get(self.vault.pool(), id).await {
            Ok(Some(row)) => {
                // 109 §6.2: re-arm the stored row; downtime counts (§6.3).
                fields = row.fields;
                title = row.title;
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
        };
        let (atx, arx) = oneshot::channel();
        self.pending.insert(id, (m.session, requested.clone(), atx));
        let rt = self.rt.clone();
        let vault = self.vault.clone();
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(requested).await;
            let outcome = wait(&vault, id, &fields, arx, timeout).await;
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
    Outcome { stored, timed_out }
}

impl Message<Answer> for SecretsActor {
    type Reply = Result<bool, String>;

    async fn handle(&mut self, m: Answer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let pool = self.vault.pool();
        let Some(row) = requests::get(pool, m.id).await.map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        if row.answered_ms.is_some() {
            return Ok(false);
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
            for f in &row.fields {
                if let Some(v) = values.get(&f.name) {
                    self.vault
                        .set(
                            &f.name,
                            v,
                            (!f.label.is_empty()).then_some(&f.label),
                            "user",
                        )
                        .await?;
                }
            }
            self.rt.publish(SecretsEvent::Changed).await;
            "stored"
        };
        let fresh = requests::answer(pool, m.id, outcome, now_ms())
            .await
            .map_err(|e| e.to_string())?;
        if let Some((_, _, tx)) = self.pending.remove(&m.id) {
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

impl Message<ListRequests> for SecretsActor {
    type Reply = Result<Vec<SecretRequest>, String>;

    async fn handle(&mut self, m: ListRequests, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        requests::list(self.vault.pool(), m.unanswered_only)
            .await
            .map_err(|e| e.to_string())
    }
}

/// ms since the Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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

impl ::tools::secrets::Secrets for Vault {
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        Vault::expand(self, args)
    }

    fn redact<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        Vault::redact(self, text)
    }

    fn redact_value(&self, v: &mut serde_json::Value) {
        Vault::redact_value(self, v);
    }

    fn redact_spans(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        Vault::redact_spans(self, text)
    }

    fn max_literal_len(&self) -> usize {
        Vault::max_literal_len(self)
    }
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
        let actor = SecretsActor::spawn(SecretsActor::new(
            vault.clone(),
            rt.clone(),
            Duration::from_secs(cfg.request_timeout_secs),
        ));
        actor.register(SECRETS_ACTOR)?;
        tokio::spawn(replicate(rt.clone(), vault.clone()));
        if !cfg.tools {
            return Ok(());
        }
        ::tools::secrets::install(vault.clone());
        let rt = rt.clone();
        let timeout = Duration::from_secs(cfg.request_timeout_secs);
        tokio::spawn(async move {
            if let Some(reg) = rt
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
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
            })
            .await
            .unwrap(),
            "a late answer is refused"
        );
        assert!(vault.get("TOKEN", 0).is_none());
    }
}
