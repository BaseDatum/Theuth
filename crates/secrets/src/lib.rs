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
//! - [`tools`]: the four `secret_*` tools and the prompt block.
//!
//! Public surface consumed by other plugins:
//! - commands on the [`SECRETS_ACTOR`] actor ([`SecretsActor`]): [`List`],
//!   [`Set`], [`Restore`], [`Delete`], [`Request`], [`Answer`], [`PendingRequests`], [`Handle`]
//! - events: [`SecretsEvent`]
//! - [`Vault`] as a library, for `t --secret-set` and `--slack-setup`

mod key;
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
    KEEP, KeyStore, MESH_PREFIX, OAUTH_PREFIX, SecretInfo, SecretsList, Vault, VersionInfo,
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

/// `ask` → `Result<Outcome, String>`: ask the user to type `name` into a
/// masked field. Leaves the mailbox; publishes [`SecretsEvent::Requested`],
/// waits `request_timeout_secs` for an [`Answer`], publishes
/// [`SecretsEvent::Resolved`]. `Err` = invalid name or write failure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    /// The asking session.
    pub session: SessionId,
    /// Name to store under.
    pub name: String,
    /// What it is for, shown to the user.
    pub note: String,
}

/// What [`Request`] resolves to: the new version when stored, else why not.
#[derive(Clone, Copy, Debug, kameo::Reply)]
pub struct Outcome {
    /// New version number when the user typed a value.
    pub stored: Option<u32>,
    /// Nobody answered in time.
    pub timed_out: bool,
}

/// `ask` → `Option<SessionId>`: the requesting session if `id` was pending.
/// `value: None` declines; `Some` is stored with source `user`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answer {
    /// From [`SecretsEvent::Requested`].
    pub id: RequestId,
    /// The typed value, or `None` when the user declined.
    pub value: Option<String>,
}

/// `ask` → `Arc<Vault>`: in-process readers (genai auth, `web`, `slack`)
/// call `get` directly (37 §3.7). Never exposed over RPC.
#[derive(Clone, Copy, Debug, Default)]
pub struct Handle;

/// `ask` → `[u8; 32]`: the mesh data key (67 §5.4). The mesh puts it in the
/// pairing string it prints. Never exposed over RPC.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetMeshKey;

/// `ask` → `Result<(), String>`: take the mesh key a pairing string carried
/// (`t mesh add`); local rows are re-sealed under it.
#[derive(Clone, Copy, Debug)]
pub struct AdoptMeshKey(pub [u8; 32]);

/// Bus events (37 §3.3).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum SecretsEvent {
    /// Names, notes, versions or the key store changed; UIs re-`list_secrets`.
    /// Wire `type` is `SecretsChanged`: `ScheduleEvent::Changed` already owns `Changed`.
    #[serde(rename = "SecretsChanged")]
    Changed,
    /// The model asked for one; frontends show a masked field.
    #[serde(rename = "SecretRequested")]
    Requested {
        /// Answer with this id.
        id: RequestId,
        /// The asking session.
        session: SessionId,
        /// Name it will be stored under.
        name: String,
        /// What it is for.
        note: String,
        /// `Some(v)` when the name exists: the answer adds `v + 1`.
        current: Option<u32>,
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
}

/// Internal `tell`: drop a pending entry whose waiter gave up.
struct Expire(RequestId);

/// A waiting `secret_request`: its session, the `Requested` event (replayed to
/// late subscribers) and where the value goes.
type Waiter = (SessionId, SecretsEvent, oneshot::Sender<Option<String>>);

/// The [`SECRETS_ACTOR`] actor: the vault plus the requests waiting on a user.
#[derive(kameo::Actor)]
pub struct SecretsActor {
    vault: Arc<Vault>,
    rt: Runtime,
    timeout: Duration,
    pending: HashMap<RequestId, Waiter>,
}

impl SecretsActor {
    /// Build over an open vault; `timeout` is the human wait for [`Request`].
    pub fn new(vault: Arc<Vault>, rt: Runtime, timeout: Duration) -> Self {
        Self {
            vault,
            rt,
            timeout,
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

impl Message<Request> for SecretsActor {
    type Reply = DelegatedReply<Result<Outcome, String>>;

    async fn handle(&mut self, m: Request, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if !vault::valid_name(&m.name) {
            if let Some(tx) = tx {
                tx.send(Err(format!(
                    "invalid secret name {:?}: [A-Za-z0-9_.-]{{1,64}}",
                    m.name
                )));
            }
            return reply;
        }
        let id = RequestId(uuid::Uuid::now_v7());
        let (atx, arx) = oneshot::channel();
        let current = self
            .vault
            .list()
            .into_iter()
            .find(|s| s.name == m.name)
            .map(|s| s.current);
        let requested = SecretsEvent::Requested {
            id,
            session: m.session,
            name: m.name.clone(),
            note: m.note.clone(),
            current,
        };
        self.pending.insert(id, (m.session, requested.clone(), atx));
        let rt = self.rt.clone();
        let vault = self.vault.clone();
        let timeout = self.timeout;
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(requested).await;
            let (value, timed_out) = match tokio::time::timeout(timeout, arx).await {
                Ok(Ok(v)) => (v, false),
                // Sender dropped: actor died; treat as declined.
                Ok(Err(_)) => (None, false),
                Err(_) => {
                    let _ = me.tell(Expire(id)).await;
                    (None, true)
                }
            };
            let result = match value {
                Some(v) => vault
                    .set(
                        &m.name,
                        &v,
                        (!m.note.is_empty()).then_some(m.note.as_str()),
                        "user",
                    )
                    .await
                    .map(|info| Some(info.current)),
                None => Ok(None),
            };
            let stored = match &result {
                Ok(s) => *s,
                Err(_) => None,
            };
            if stored.is_some() {
                rt.publish(SecretsEvent::Changed).await;
            }
            rt.publish(SecretsEvent::Resolved {
                id,
                stored,
                timed_out,
            })
            .await;
            if let Some(tx) = tx {
                tx.send(result.map(|stored| Outcome { stored, timed_out }));
            }
        });
        reply
    }
}

impl Message<Answer> for SecretsActor {
    type Reply = Option<SessionId>;

    async fn handle(&mut self, m: Answer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (session, _requested, tx) = self.pending.remove(&m.id)?;
        tx.send(m.value).ok().map(|()| session)
    }
}

impl Message<Expire> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        self.pending.remove(&m.0);
    }
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

impl Message<GetMeshKey> for SecretsActor {
    type Reply = [u8; 32];

    async fn handle(&mut self, _: GetMeshKey, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        *self.vault.mesh_key()
    }
}

impl Message<AdoptMeshKey> for SecretsActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: AdoptMeshKey, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.vault
            .adopt_mesh_key(m.0)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(())
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
    let mut seen = vault.disk_stamp();
    let mut tick = tokio::time::interval(REFRESH_POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
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
