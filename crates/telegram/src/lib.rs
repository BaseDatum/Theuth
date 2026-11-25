//! Telegram plugin: a phone-side frontend for the daemon over Bot API long
//! polling (design doc 98).
//!
//! [`api`] wraps `tgbot`, [`poll`] runs the lease-gated `getUpdates` loop and
//! classifies updates, [`channel`] routes inbound and fans events out to
//! per-thread [`worker`]s, [`threads`] persists thread ↔ session bindings,
//! [`commands`] / [`ui`] answer native bot commands, [`setup`] is
//! `t --telegram-setup`, [`user`] delivers `message_user` (24 §5).
//!
//! - commands on the `"telegram"` actor: [`Status`], [`Link`], [`Allow`],
//!   [`RemoveAllowed`], [`AllowGroup`], [`RemoveGroup`], [`SetOwner`]
//! - events: [`TelegramEvent`]
//! - tools: [`TelegramLinkTool`], [`TelegramStatusTool`]

// 98c: a guard alive across an `.await` pins a dispatch task (and its OS
// thread) forever; the lint catches it at compile time.
#![deny(clippy::await_holding_lock)]

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tools::{Risk, Tool, ToolCx, ToolOutput};

/// Messaging RPC methods and typed clients.
pub mod rpc_messaging;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_messaging::rpc_module(server),
    ]).expect("telegram RPC composition")
}

pub mod rpc_support;
pub mod api;
pub mod approvals;
pub mod asks;
pub mod board;
pub mod bubble;
pub mod cards;
pub mod channel;
pub mod commands;
pub mod poll;
pub mod port;
pub mod setup;
pub mod sharing;
pub mod threads;
pub mod ui;
pub mod user;
pub mod worker;

/// Registry name of the plugin's actor.
pub const TELEGRAM_ACTOR: &str = "telegram";
/// Vault key of the bot token (37 §3.6).
pub const SECRET_TOKEN: &str = "telegram_bot";
/// How long a pairing code lives (98 §10.4).
pub const PAIR_TTL: Duration = Duration::from_secs(600);
/// Pairing code length (98 §5).
pub const CODE_LEN: usize = 16;
/// The 09 §10 alphabet: no `0 O 1 I l`.
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// `[telegram]` config section (98 §12).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    /// The daemon polls when the token is present.
    pub enabled: bool,
    /// Owner user id: owner-only commands, may press any card, receives
    /// unbound cards. Set by setup / `telegram_link` / the deep link.
    pub owner: Option<u64>,
    /// More user ids allowed to talk; each owns their threads.
    pub allowed: Vec<u64>,
    /// Chat ids where mentions and commands are honoured; empty = private chats only.
    pub groups: Vec<i64>,
    /// Activity cards per tool batch.
    pub tools: ToolsMode,
    /// Reasoning text in the expandable quote.
    pub reasoning: bool,
    /// Diagrams and surfaces in replies go out as pictures (164 D10); `false` = text fallback.
    pub pictures: bool,
    /// Where approvals for sessions not bound to a thread go.
    pub approvals: ApprovalsMode,
    /// Tests and proofs only: overrides `https://api.telegram.org` so a live
    /// engine can be pointed at a fake Bot API server. `None` in production.
    pub api_host: Option<String>,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            owner: None,
            allowed: Vec::new(),
            groups: Vec::new(),
            tools: ToolsMode::All,
            reasoning: true,
            pictures: true,
            approvals: ApprovalsMode::All,
            api_host: None,
        }
    }
}

/// `telegram.tools` (98 §8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolsMode {
    /// A card per tool batch.
    All,
    /// Cards only for failed calls.
    Errors,
    /// No cards.
    None,
}

/// `telegram.approvals` (98 §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalsMode {
    /// Unbound sessions' approvals go to the owner's chat.
    All,
    /// Only sessions bound to a thread get cards.
    Threads,
}

/// Bus event: connection and rendering state (98 §13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum TelegramEvent {
    /// `getMe` answered.
    Identity {
        /// `@username` without the `@`.
        bot: String,
        /// Topics are enabled for the bot.
        topics: bool,
    },
    /// First successful `getUpdates` after a (re)start.
    Connected {
        /// Bot username.
        bot: String,
    },
    /// A poll error (the loop backs off), or `hub lease held elsewhere`.
    Disconnected {
        /// Why.
        reason: String,
    },
    /// 409: another poller on this token.
    Conflict,
    /// A call was still rate limited after the client's retries.
    RateLimited {
        /// Bot API method.
        method: String,
        /// `retry_after`.
        retry_after_secs: u64,
    },
    /// A bubble left Draft mode, or rich → plain (98 §8.3).
    StreamFallback {
        /// Session whose turn it was.
        session: SessionId,
        /// The exact error or reason.
        reason: String,
    },
    /// The chat cannot be written to; its thread was unbound (98 §4.3).
    Unreachable {
        /// Chat id.
        chat: i64,
        /// Why.
        reason: String,
    },
    /// A pairing link was minted; show it as a QR (09 `LinkUrl` shape).
    PairingLink {
        /// `https://t.me/<bot>?start=<code>`.
        url: String,
        /// Unix milliseconds.
        expires_at_ms: u64,
    },
    /// `/start <code>` arrived: the owner changed.
    Paired {
        /// New owner.
        user: u64,
        /// Their name.
        name: String,
    },
    /// Token missing; nothing started.
    Unconfigured,
    /// Terminal mode: the channel only runs in `t --headless`.
    DaemonOnly,
    /// 98c: the actor is up but the vault has not answered yet; `Status`
    /// reports this instead of guessing `Unconfigured`. Never on the bus.
    Starting,
}

impl wire::WireEvent for TelegramEvent {
    const TOPIC: &'static str = "telegram.events";
    /// 216 §7.3 (a): the bot pairing link makes its holder the owner.
    const DROP: &'static [&'static str] = &["PairingLink"];
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<TelegramEvent>().await
}

/// `getMe` (98 §3.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BotIdentity {
    /// Bot user id.
    pub id: u64,
    /// Username without the `@`.
    pub username: String,
    /// `has_topics_enabled`.
    pub topics: bool,
    /// `allows_users_to_create_topics`.
    pub users_may_create_topics: bool,
}

/// Per-method call counters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CallStats {
    /// `ok: true` answers.
    pub ok: u64,
    /// `ok: false` or transport failures.
    pub err: u64,
    /// Still limited after retries.
    pub rate_limited: u64,
}

/// Snapshot served by [`Status`], `/telegram status`, `telegram_status` and RPC (98 §13).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TelegramStatus {
    /// Last state event; `None` before the supervisor has reported.
    pub state: Option<TelegramEvent>,
    /// `getMe`, once answered.
    pub bot: Option<BotIdentity>,
    /// This engine holds the lease and the poller is up.
    pub polling: bool,
    /// Owner user id.
    pub owner: Option<u64>,
    /// Allowed user ids (excluding the owner).
    pub allowed: Vec<u64>,
    /// Chats where mentions and commands are honoured.
    pub groups: Vec<i64>,
    /// `chat[:topic]` → session bindings.
    pub threads: Vec<(String, SessionId)>,
    /// Edit-mode messages in flight.
    pub open: u32,
    /// Per-method counters.
    pub calls: BTreeMap<String, CallStats>,
    /// Updates ignored (unknown kinds, edits, channel posts, …).
    pub dropped_updates: u64,
}

/// A minted pairing link (98 §10.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PairingLink {
    /// `https://t.me/<bot>?start=<code>`.
    pub url: String,
    /// Unix milliseconds.
    pub expires_at_ms: u64,
}

/// The live pairing code; single-use.
#[derive(Clone, Debug)]
pub struct Pairing {
    /// 16 chars of the 09 §10 alphabet.
    pub code: String,
    /// When it stops working.
    pub expires: Instant,
}

/// A fresh pairing code: [`CODE_LEN`] characters of [`ALPHABET`].
#[must_use]
pub fn mint_code() -> String {
    uuid::Uuid::new_v4()
        .into_bytes()
        .iter()
        .take(CODE_LEN)
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

/// `ask` → [`TelegramStatus`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Result<Option<PairingLink>, String>`: with `owner`, sets the
/// owner live and in config and answers `None`; without, mints a code and
/// answers the deep link.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Link {
    /// Known owner id, when the user already has it.
    pub owner: Option<u64>,
}

/// `ask` → `Result<(), String>`: add a user id to `telegram.allowed` (config file and live list).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Allow(pub u64);

/// `ask` → `Result<(), String>`: remove a user id from `telegram.allowed`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RemoveAllowed(pub u64);

/// `ask` → `Result<(), String>`: add a chat id to `telegram.groups`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AllowGroup(pub i64);

/// `ask` → `Result<(), String>`: remove a chat id from `telegram.groups`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RemoveGroup(pub i64);

/// `ask` → `Result<(), String>`: set `telegram.owner` (config file and live); used by pairing.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetOwner(pub u64);

/// The plugin. `channel: true` (headless) polls; `false` registers the
/// actor and tools only (98 §3).
pub struct TelegramPlugin {
    /// Run the channel.
    pub channel: bool,
}

/// A list as a single-line YAML flow list.
fn yaml_list<T: std::fmt::Display>(items: &[T]) -> String {
    let s: Vec<String> = items.iter().map(ToString::to_string).collect();
    format!("[{}]", s.join(", "))
}

/// Reads the bot token on `SecretsChanged`: the vault in production, a stub in tests.
type TokenSource = Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// Starts the channel: [`channel::spawn`] in production, a local host in tests.
type Starter = Arc<
    dyn Fn(TelegramConfig, String, runtime::Runtime) -> anyhow::Result<channel::Handle>
        + Send
        + Sync,
>;

/// Internal `tell`: the vault was re-read after `SecretsChanged`.
struct TokenRead(Option<String>);

/// The `"telegram"` actor: control surface for the channel (daemon) or the
/// terminal-mode stand-in that answers `Status` and edits config. Hot-starts
/// the channel when the token appears later (98 §10.5).
#[derive(kameo::Actor)]
pub struct Telegram {
    handle: Option<channel::Handle>,
    cfg: TelegramConfig,
    rt: runtime::Runtime,
    /// `None` until the vault answered at boot (98c: `Starting`).
    configured: Option<bool>,
    /// This process runs the channel (`TelegramPlugin::channel`).
    run_channel: bool,
    /// Hash of the token in use; never the token (98 §10.5: no hot swap, log once).
    token_hash: Option<u64>,
    source: TokenSource,
    start: Starter,
}

/// `telegram_bot` from the vault, if both exist.
fn token_from(vault: Option<&Arc<secrets::Vault>>) -> Option<String> {
    vault
        .and_then(|v| v.get(SECRET_TOKEN, 0))
        .map(|t| t.as_str().to_owned())
}

fn token_hash(token: &str) -> u64 {
    let mut h = DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// `RegisterUserSink` once the user actor is up (24 §5).
fn register_sink(rt: runtime::Runtime, sink: channel::Handle) {
    tokio::spawn(async move {
        if let Some(users) = rt
            .lookup_when::<agent::UserChannels>(agent::USER_ACTOR)
            .await
        {
            let _ = users.ask(agent::RegisterUserSink(Arc::new(sink))).await;
        }
    });
}

impl Telegram {
    pub(crate) fn new(
        handle: Option<channel::Handle>,
        cfg: TelegramConfig,
        rt: runtime::Runtime,
        token: Option<&str>,
        run_channel: bool,
    ) -> Self {
        let vault_rt = rt.clone();
        Self {
            handle,
            cfg,
            rt,
            configured: Some(token.is_some()),
            run_channel,
            token_hash: token.map(token_hash),
            source: Arc::new(move || {
                let rt = vault_rt.clone();
                Box::pin(async move { token_from(secrets::handle(&rt).await.as_ref()) })
            }),
            start: Arc::new(channel::spawn),
        }
    }

    /// The boot-time actor (98c): no handle yet and the token state unknown
    /// (`Starting`) until the vault answers with a `TokenRead`.
    pub(crate) fn starting(cfg: TelegramConfig, rt: runtime::Runtime, run_channel: bool) -> Self {
        Self {
            configured: None,
            ..Self::new(None, cfg, rt, None, run_channel)
        }
    }

    fn config_path(&self) -> PathBuf {
        self.rt.data_dir.join("config.yaml")
    }

    fn persist(&self, key: &str, value: &str) -> Result<(), String> {
        let path = self.config_path();
        runtime::write_config_value(&path, "telegram", key, value)
            .map_err(|e| format!("write {}: {e}", path.display()))
    }

    fn offline_status(&self, state: TelegramEvent) -> TelegramStatus {
        TelegramStatus {
            state: Some(state),
            bot: None,
            polling: false,
            owner: self.cfg.owner,
            allowed: self.cfg.allowed.clone(),
            groups: self.cfg.groups.clone(),
            threads: threads::Threads::load(&channel::threads_path(&self.rt.data_dir)).list(),
            open: 0,
            calls: BTreeMap::new(),
            dropped_updates: 0,
        }
    }

    fn set_owner(&mut self, id: u64) -> Result<(), String> {
        if let Some(h) = &self.handle {
            h.set_owner(id);
        }
        self.cfg.owner = Some(id);
        self.cfg.allowed.retain(|u| *u != id);
        self.persist("owner", &id.to_string())
    }

    /// 98 §10.5 / 98c: start the channel — at boot once the vault answered,
    /// or when the token appears later; a token replaced under a running
    /// channel is noted, not swapped. Nothing here awaits the network: the
    /// supervisor does getMe/setMyCommands/repair on its own task, so
    /// `Status` keeps answering.
    fn hot_start(&mut self, token: String) {
        let hash = token_hash(&token);
        if self.handle.is_some() {
            if self.token_hash != Some(hash) {
                self.token_hash = Some(hash);
                tracing::info!("telegram: bot token changed; restart the engine to use it");
            }
            return;
        }
        match (self.start)(self.cfg.clone(), token, self.rt.clone()) {
            Ok(h) => {
                register_sink(self.rt.clone(), h.clone());
                self.handle = Some(h);
                self.configured = Some(true);
                self.token_hash = Some(hash);
                tracing::info!("telegram: bot token present; channel started");
            }
            Err(e) => tracing::error!("telegram: channel failed to start: {e:#}"),
        }
    }
}

impl Message<secrets::SecretsEvent> for Telegram {
    type Reply = ();

    async fn handle(&mut self, ev: secrets::SecretsEvent, ctx: &mut Context<Self, ()>) {
        // Terminal mode never runs the channel (D4); the vault read stays off the mailbox.
        if !matches!(ev, secrets::SecretsEvent::Changed) || !self.run_channel {
            return;
        }
        let me = ctx.actor_ref().clone();
        let read = (self.source)();
        tokio::spawn(async move {
            let _ = me.tell(TokenRead(read.await)).await;
        });
    }
}

impl Message<TokenRead> for Telegram {
    type Reply = ();

    async fn handle(&mut self, m: TokenRead, _: &mut Context<Self, ()>) {
        let Some(token) = m.0 else {
            tracing::debug!("telegram: vault read; no `{SECRET_TOKEN}`");
            self.configured.get_or_insert(false);
            return;
        };
        if self.run_channel {
            self.hot_start(token);
        } else if self.configured != Some(true) {
            // Terminal mode (D4): the token exists, the channel runs in `t --headless`.
            self.configured = Some(true);
            self.token_hash = Some(token_hash(&token));
            self.rt.publish(TelegramEvent::DaemonOnly).await;
        }
    }
}

impl Message<Status> for Telegram {
    type Reply = TelegramStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match (&self.handle, self.configured) {
            (Some(h), _) => h.status(),
            (None, None) => self.offline_status(TelegramEvent::Starting),
            (None, Some(false)) => self.offline_status(TelegramEvent::Unconfigured),
            (None, Some(true)) => self.offline_status(TelegramEvent::DaemonOnly),
        }
    }
}

impl Message<Link> for Telegram {
    type Reply = Result<Option<PairingLink>, String>;

    async fn handle(&mut self, m: Link, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(id) = m.owner {
            self.set_owner(id)?;
            return Ok(None);
        }
        match (&self.handle, self.configured) {
            (Some(h), _) => h.link(None).await,
            (None, None) => Err("telegram is starting: the vault has not answered yet".into()),
            (None, Some(false)) => Err(format!(
                "telegram is not configured: set the {SECRET_TOKEN} secret (Settings › Channels, or t --telegram-setup)"
            )),
            (None, Some(true)) => {
                Err("pairing needs the daemon: run t --headless, or pass the owner id".into())
            }
        }
    }
}

impl Message<Allow> for Telegram {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Allow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.cfg.owner == Some(m.0) {
            return Err(format!("{} is the owner", m.0));
        }
        if let Some(h) = &self.handle {
            h.allow(m.0);
        }
        if !self.cfg.allowed.contains(&m.0) {
            self.cfg.allowed.push(m.0);
        }
        self.persist("allowed", &yaml_list(&self.cfg.allowed))
    }
}

impl Message<RemoveAllowed> for Telegram {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveAllowed,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if !self.cfg.allowed.contains(&m.0) {
            return Err(format!("{} is not in the allowed list", m.0));
        }
        if let Some(h) = &self.handle {
            h.remove_allowed(m.0);
        }
        self.cfg.allowed.retain(|u| *u != m.0);
        self.persist("allowed", &yaml_list(&self.cfg.allowed))
    }
}

impl Message<AllowGroup> for Telegram {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: AllowGroup, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(h) = &self.handle {
            h.allow_group(m.0);
        }
        if !self.cfg.groups.contains(&m.0) {
            self.cfg.groups.push(m.0);
        }
        self.persist("groups", &yaml_list(&self.cfg.groups))
    }
}

impl Message<RemoveGroup> for Telegram {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: RemoveGroup, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.cfg.groups.contains(&m.0) {
            return Err(format!("{} is not in the group list", m.0));
        }
        if let Some(h) = &self.handle {
            h.remove_group(m.0);
        }
        self.cfg.groups.retain(|c| *c != m.0);
        self.persist("groups", &yaml_list(&self.cfg.groups))
    }
}

impl Message<SetOwner> for Telegram {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetOwner, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.set_owner(m.0)
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for TelegramPlugin {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: TelegramConfig = rt.section("telegram").unwrap_or_else(|e| {
            tracing::debug!("no [telegram] config section, using defaults: {e}");
            TelegramConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("telegram disabled by config");
            return Ok(());
        }
        let run_channel = self.channel;
        let rt = rt.clone();
        // 98c: the actor exists and answers `Status` before any waiting —
        // the vault, getMe and the rest never sit between RPC and a snapshot.
        let actor = Telegram::spawn_with_mailbox(
            Telegram::starting(cfg, rt.clone(), run_channel),
            runtime::subscriber_mailbox(),
        );
        if let Err(e) = actor.register(TELEGRAM_ACTOR) {
            tracing::error!("telegram: registering {TELEGRAM_ACTOR}: {e}");
            return Ok(());
        }
        tokio::spawn(async move {
            // 98 §10.5: hot-start when the token is set later.
            rt.subscribe::<Telegram, secrets::SecretsEvent>(&actor)
                .await;
            // The token lives in the vault (37 §3.6); the actor connects when it arrives.
            let vault = secrets::handle(&rt).await;
            if vault.is_none() {
                tracing::warn!(
                    "telegram: secrets vault unavailable; starting without the bot token"
                );
            }
            let token = token_from(vault.as_ref());
            if token.is_none() {
                tracing::warn!(
                    "telegram: secret `{SECRET_TOKEN}` not set (Settings › Channels, or t --telegram-setup); the channel starts when it is"
                );
                rt.publish(TelegramEvent::Unconfigured).await;
            }
            let _ = actor.tell(TokenRead(token)).await;

            // First use of the registry (00 §4, 12 §2).
            let Some(reg) = rt
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await
            else {
                return;
            };
            let _ = reg
                .ask(tools::RegisterTool(Arc::new(TelegramLinkTool)))
                .await;
            let _ = reg
                .ask(tools::RegisterTool(Arc::new(TelegramStatusTool)))
                .await;
        });
        Ok(())
    }
}

fn telegram_actor() -> Option<ActorRef<Telegram>> {
    ActorRef::<Telegram>::lookup(TELEGRAM_ACTOR).ok().flatten()
}

/// Arguments for [`TelegramLinkTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TelegramLinkArgs {
    /// The owner's Telegram user id, when already known; otherwise a pairing
    /// link is minted.
    #[serde(default)]
    pub owner: Option<u64>,
}

/// Pair the owner: set the id directly, or mint a deep link + QR (98 §10.4).
pub struct TelegramLinkTool;

#[async_trait::async_trait]
impl Tool for TelegramLinkTool {
    const NAME: &'static str = "telegram_link";
    const RISK: Risk = Risk::Mutating;
    type Args = TelegramLinkArgs;

    fn description(&self) -> String {
        "Pair the Telegram owner: with `owner` sets telegram.owner to that user id; without, mints a one-time https://t.me/<bot>?start=<code> link (10 min) to open from the owner's Telegram."
            .into()
    }

    async fn call(&self, a: TelegramLinkArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(actor) = telegram_actor() else {
            return Ok(ToolOutput::err("telegram actor is not running"));
        };
        match actor.ask(Link { owner: a.owner }).await {
            Ok(None) => Ok(ToolOutput::ok(format!(
                "telegram.owner set to {}; it applies live",
                a.owner.unwrap_or_default()
            ))),
            Ok(Some(link)) => Ok(ToolOutput::ok(format!(
                "open this from the owner's Telegram:\n{}\n\n{}\n(expires in 10 min; call again for a fresh one)",
                link.url,
                link::qr(&link.url)
            ))),
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
    }
}

/// Arguments for [`TelegramStatusTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TelegramStatusArgs {}

/// The 98 §13 snapshot as text.
pub struct TelegramStatusTool;

#[async_trait::async_trait]
impl Tool for TelegramStatusTool {
    const NAME: &'static str = "telegram_status";
    const RISK: Risk = Risk::ReadOnly;
    type Args = TelegramStatusArgs;

    fn description(&self) -> String {
        "Telegram polling state, bot identity, owner, allowed users, groups, thread bindings and call counters."
            .into()
    }

    async fn call(&self, _: TelegramStatusArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(actor) = telegram_actor() else {
            return Ok(ToolOutput::err("telegram actor is not running"));
        };
        let s = actor.ask(Status).await?;
        Ok(ToolOutput::ok(render_status(&s)))
    }
}

/// Plain-text rendering of a [`TelegramStatus`].
#[must_use]
pub fn render_status(s: &TelegramStatus) -> String {
    let state = match &s.state {
        None => "unknown".to_owned(),
        Some(TelegramEvent::Identity { bot, topics }) => {
            format!(
                "identity @{bot} (topics {})",
                if *topics { "on" } else { "off" }
            )
        }
        Some(TelegramEvent::Connected { bot }) => format!("connected as @{bot}"),
        Some(TelegramEvent::Disconnected { reason }) => format!("disconnected: {reason}"),
        Some(TelegramEvent::Conflict) => "conflict: another poller uses this token".into(),
        Some(TelegramEvent::RateLimited {
            method,
            retry_after_secs,
        }) => format!("rate limited on {method}; retry in {retry_after_secs} s"),
        Some(TelegramEvent::StreamFallback { session, reason }) => {
            format!("stream fallback for {session}: {reason}")
        }
        Some(TelegramEvent::Unreachable { chat, reason }) => {
            format!("chat {chat} unreachable: {reason}")
        }
        Some(TelegramEvent::PairingLink { url, .. }) => format!("pairing link minted: {url}"),
        Some(TelegramEvent::Paired { user, name }) => format!("paired with {name} ({user})"),
        Some(TelegramEvent::Unconfigured) => {
            format!("unconfigured (secret {SECRET_TOKEN} missing; run t --telegram-setup)")
        }
        Some(TelegramEvent::DaemonOnly) => "daemon only (runs in t --headless)".into(),
        Some(TelegramEvent::Starting) => "starting (waiting for the vault)".into(),
    };
    let dash = |v: Vec<String>| {
        if v.is_empty() {
            "-".to_owned()
        } else {
            v.join(", ")
        }
    };
    let bot = match &s.bot {
        Some(b) => format!(
            "@{} ({}, topics {}{})",
            b.username,
            b.id,
            if b.topics { "on" } else { "off" },
            if b.users_may_create_topics {
                ", users may create topics"
            } else {
                ""
            }
        ),
        None => "-".to_owned(),
    };
    let mut out = format!(
        "state: {state}\nbot: {bot}\npolling: {}\nowner: {}\nallowed: {}\ngroups: {}\nopen messages: {}\ndropped updates: {}\n",
        if s.polling { "yes" } else { "no" },
        s.owner.map_or_else(|| "-".to_owned(), |o| o.to_string()),
        dash(s.allowed.iter().map(ToString::to_string).collect()),
        dash(s.groups.iter().map(ToString::to_string).collect()),
        s.open,
        s.dropped_updates,
    );
    if s.threads.is_empty() {
        out.push_str("threads: -\n");
    } else {
        out.push_str("threads:\n");
        for (key, session) in &s.threads {
            out.push_str(&format!("  {key} → {session}\n"));
        }
    }
    if s.calls.is_empty() {
        out.push_str("calls: -");
    } else {
        out.push_str("calls:");
        for (method, c) in &s.calls {
            out.push_str(&format!(
                "\n  {method}: {} ok, {} err, {} rate limited",
                c.ok, c.err, c.rate_limited
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_code_alphabet_and_length() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let c = mint_code();
            assert_eq!(c.len(), CODE_LEN);
            assert!(c.bytes().all(|b| ALPHABET.contains(&b)), "{c}");
            assert!(seen.insert(c));
        }
    }

    /// An unconfigured actor with the token source answering `None`; the
    /// channel, when started, talks to a local mock instead of Telegram.
    fn unconfigured(rt: &runtime::Runtime, run_channel: bool, host: String) -> Telegram {
        let mut t = Telegram::new(
            None,
            TelegramConfig::default(),
            rt.clone(),
            None,
            run_channel,
        );
        t.start = Arc::new(move |cfg, token, rt| channel::spawn_at(cfg, token, rt, Some(&host)));
        t
    }

    async fn state(actor: &ActorRef<Telegram>) -> Option<TelegramEvent> {
        for _ in 0..50 {
            let s = actor.ask(Status).await.unwrap();
            if s.state != Some(TelegramEvent::Unconfigured) {
                return s.state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Some(TelegramEvent::Unconfigured)
    }

    #[tokio::test]
    async fn secrets_changed_hot_starts_the_channel() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let server = wiremock::MockServer::start().await;

        // Terminal mode (D4): nothing happens even with a token in the vault.
        let mut term = unconfigured(&rt, false, server.uri());
        term.source = Arc::new(|| Box::pin(async { Some("1:terminal".to_owned()) }));
        let term = Telegram::spawn_with_mailbox(term, runtime::subscriber_mailbox());
        term.tell(secrets::SecretsEvent::Changed).await.unwrap();
        assert_eq!(state(&term).await, Some(TelegramEvent::Unconfigured));

        // Daemon: no token yet → still unconfigured; token appears → channel up.
        let vault = Arc::new(parking_lot::Mutex::new(None::<String>));
        let mut t = unconfigured(&rt, true, server.uri());
        let v = Arc::clone(&vault);
        t.source = Arc::new(move || {
            let tok = v.lock().clone();
            Box::pin(async move { tok })
        });
        let actor = Telegram::spawn_with_mailbox(t, runtime::subscriber_mailbox());
        actor.tell(secrets::SecretsEvent::Changed).await.unwrap();
        assert_eq!(state(&actor).await, Some(TelegramEvent::Unconfigured));
        assert!(matches!(
            actor.ask(Link { owner: None }).await,
            Err(e) if e.to_string().starts_with("telegram is not configured")
        ));

        *vault.lock() = Some("1:first".to_owned());
        actor.tell(secrets::SecretsEvent::Changed).await.unwrap();
        assert_ne!(state(&actor).await, Some(TelegramEvent::Unconfigured));
        rt.shutdown.cancel();
    }

    /// 98c H5: `Plugin::start` registers the actor before waiting for the
    /// vault. No secrets actor runs in this process, so `secrets::handle`
    /// would wait `HANDLE_WAIT` (10 s); `Status` still answers at once, as
    /// `Starting`, and `Link` says so instead of "not configured".
    #[tokio::test]
    async fn plugin_start_registers_the_actor_before_the_vault_answers() {
        use runtime::Plugin as _;
        let _registry = channel::tests::REGISTRY.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let started = Instant::now();
        Box::new(TelegramPlugin { channel: true })
            .start(&rt)
            .await
            .unwrap();
        let actor = telegram_actor().expect("registered synchronously in start");
        let s = tokio::time::timeout(Duration::from_secs(1), actor.ask(Status))
            .await
            .expect("Status answered within 1 s")
            .unwrap();
        assert_eq!(s.state, Some(TelegramEvent::Starting));
        assert!(
            started.elapsed() < secrets::HANDLE_WAIT,
            "{:?}",
            started.elapsed()
        );
        assert!(matches!(
            actor.ask(Link { owner: None }).await,
            Err(e) if e.to_string().starts_with("telegram is starting")
        ));
        rt.shutdown.cancel();
        channel::tests::kill_registered(actor).await;
    }

    #[test]
    fn config_defaults_and_lowercase_modes() {
        let c: TelegramConfig =
            serde_yaml::from_str("tools: errors\napprovals: threads\nowner: 5\n").unwrap();
        assert_eq!(c.tools, ToolsMode::Errors);
        assert_eq!(c.approvals, ApprovalsMode::Threads);
        assert_eq!(c.owner, Some(5));
        assert!(c.enabled);
        assert!(c.reasoning);
        assert!(c.pictures, "164 D10: pictures default on");
        let c: TelegramConfig = serde_yaml::from_str("pictures: false\n").unwrap();
        assert!(!c.pictures);
        assert_eq!(yaml_list(&[1u64, 2]), "[1, 2]");
        let v: Vec<i64> = serde_yaml::from_str(&yaml_list(&[-100i64, 7])).unwrap();
        assert_eq!(v, vec![-100, 7]);
    }

    #[test]
    fn render_status_smoke() {
        let mut calls = BTreeMap::new();
        calls.insert(
            "getMe".to_owned(),
            CallStats {
                ok: 1,
                err: 0,
                rate_limited: 0,
            },
        );
        let s = TelegramStatus {
            state: Some(TelegramEvent::Connected {
                bot: "theuth_bot".into(),
            }),
            bot: Some(BotIdentity {
                id: 42,
                username: "theuth_bot".into(),
                topics: true,
                users_may_create_topics: false,
            }),
            polling: true,
            owner: Some(5),
            allowed: vec![6, 7],
            groups: vec![],
            threads: vec![("7:3".into(), SessionId(uuid::Uuid::nil()))],
            open: 2,
            calls,
            dropped_updates: 1,
        };
        let text = render_status(&s);
        assert!(text.starts_with("state: connected as @theuth_bot\nbot: @theuth_bot (42, topics on)\npolling: yes\nowner: 5\nallowed: 6, 7\ngroups: -\nopen messages: 2\ndropped updates: 1\n"), "{text}");
        assert!(text.contains("  7:3 → "));
        assert!(text.ends_with("calls:\n  getMe: 1 ok, 0 err, 0 rate limited"));
        let empty = TelegramStatus {
            state: None,
            bot: None,
            polling: false,
            owner: None,
            allowed: vec![],
            groups: vec![],
            threads: vec![],
            open: 0,
            calls: BTreeMap::new(),
            dropped_updates: 0,
        };
        let text = render_status(&empty);
        assert!(text.contains("state: unknown\nbot: -\npolling: no\nowner: -\n"));
        assert!(text.ends_with("threads: -\ncalls: -"));
    }
}
