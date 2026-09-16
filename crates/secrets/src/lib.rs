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
//!   [`Set`], [`Restore`], [`Delete`], [`Request`], [`Answer`], [`Handle`]
//! - events: [`SecretsEvent`]
//! - [`Vault`] as a library, for `wth --secret-set` and `--slack-setup`

mod key;
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

pub use vault::{KEEP, KeyStore, SecretInfo, SecretsList, Vault, VersionInfo};

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

/// A waiting `secret_request`: its session and where the value goes.
type Waiter = (SessionId, oneshot::Sender<Option<String>>);

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
            .set(&m.name, &m.value, m.note.as_deref(), &m.source)?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(info)
    }
}

impl Message<Restore> for SecretsActor {
    type Reply = Result<SecretInfo, String>;

    async fn handle(&mut self, m: Restore, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let info = self.vault.restore(&m.name, m.back)?;
        self.rt.publish(SecretsEvent::Changed).await;
        Ok(info)
    }
}

impl Message<Delete> for SecretsActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Delete, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.vault.delete(&m.name)?;
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
        self.pending.insert(id, (m.session, atx));
        let current = self
            .vault
            .list()
            .into_iter()
            .find(|s| s.name == m.name)
            .map(|s| s.current);
        let rt = self.rt.clone();
        let vault = self.vault.clone();
        let timeout = self.timeout;
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(SecretsEvent::Requested {
                id,
                session: m.session,
                name: m.name.clone(),
                note: m.note.clone(),
                current,
            })
            .await;
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
        let (session, tx) = self.pending.remove(&m.id)?;
        tx.send(m.value).ok().map(|()| session)
    }
}

impl Message<Expire> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        self.pending.remove(&m.0);
    }
}

impl Message<Handle> for SecretsActor {
    type Reply = Arc<Vault>;

    async fn handle(&mut self, _: Handle, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.vault.clone()
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
}

/// The plugin (37 §3.3): reads `secrets`, opens the vault (importing
/// `credentials.yaml`), spawns [`SecretsActor`] as [`SECRETS_ACTOR`], installs
/// the `tools::secrets` hook and registers the `secret_*` tools once the
/// registry is up.
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
        let vault = match Vault::open(&rt.data_dir, cfg.key_store) {
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
