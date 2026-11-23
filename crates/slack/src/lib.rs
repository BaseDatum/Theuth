//! Slack plugin: a workspace-side frontend for the daemon (design doc 21).
//!
//! [`socket`] keeps the Socket Mode connection, [`api`] wraps the Web API,
//! [`channel`] routes inbound messages to agent asks and agent events into
//! per-thread workers, [`bubble`] renders one streamed message per turn,
//! [`threads`] persists thread ↔ session bindings, [`commands`] answers
//! `/theuth`, [`user`] delivers `message_user` (24 §5).
//!
//! - commands on the `"slack"` actor: [`Status`], [`Allow`], [`RemoveAllowed`],
//!   [`AllowChannel`], [`RemoveChannel`]
//! - events: [`SlackEvent`]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

/// Messaging RPC methods and typed clients.
pub mod rpc_messaging;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_messaging::rpc_module(server),
    ]).expect("slack RPC composition")
}

pub mod api;
pub mod approvals;
pub mod asks;
pub mod board;
pub mod bubble;
pub mod channel;
pub mod commands;
mod forward;
pub mod plan;
mod prompt;
pub mod public_share;
pub mod setup;
pub mod socket;
pub mod threads;
pub mod ui;
pub mod user;

/// Registry name of the plugin's actor.
pub const SLACK_ACTOR: &str = "slack";

/// The Slack app manifest (21 §10.3); printed by `t --slack-manifest`.
pub const MANIFEST: &str = include_str!("../manifest.yaml");

/// `[slack]` config section (21 §12).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackConfig {
    /// The daemon opens the socket when tokens are present.
    pub enabled: bool,
    /// Owner user id (`U…`): owner-only commands, may click any approval,
    /// receives unbound approvals.
    pub owner: String,
    /// More user ids allowed to talk; each owns their threads.
    pub allowed: Vec<String>,
    /// Channels where `@theuth` is honoured; empty = DMs only.
    pub channels: Vec<String>,
    /// Which tool calls become timeline cards.
    pub tools: ToolCards,
    /// Post the quoted reasoning message.
    pub reasoning: bool,
    /// Where approvals for sessions not bound to a thread go.
    pub approvals: ApprovalsScope,
    /// The plan message (110 §5).
    pub plan: bool,
    /// What `MessageDelivered` renders (110 §6).
    pub deliveries: Deliveries,
    /// `json-render` fences in the reply as Block Kit (110 §8); `false`
    /// flattens them into code blocks as 30 §9 did.
    pub blocks: bool,
    /// Picture fences (`json-render` without a block form, `mermaid`,
    /// `openui`) as PNGs uploaded into the thread (164 D9–D10); `false`
    /// sends their text fallback only.
    pub pictures: bool,
    /// Deferred (110 §8 optional, §16): a canvas beside a board create.
    /// Parsed and unused — needs the `canvases:write` scope in
    /// `manifest.yaml` and a reinstall of the app.
    pub canvases: bool,
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            owner: String::new(),
            allowed: Vec::new(),
            channels: Vec::new(),
            tools: ToolCards::All,
            reasoning: true,
            approvals: ApprovalsScope::All,
            plan: true,
            deliveries: Deliveries::Hidden,
            blocks: true,
            pictures: true,
            canvases: false,
        }
    }
}

/// `slack.tools` (21 §8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCards {
    /// A card per tool call.
    All,
    /// Cards only for failed calls.
    Errors,
    /// No cards.
    None,
}

/// `slack.approvals` (21 §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalsScope {
    /// Unbound sessions' approvals go to the owner DM.
    All,
    /// Only sessions bound to a thread get cards.
    Threads,
}

/// `slack.deliveries` (110 §9): one variant so the key exists before its
/// second value (`collapsed`) does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Deliveries {
    /// `MessageDelivered` posts nothing; counted in `SlackStatus.hidden_deliveries`.
    #[default]
    Hidden,
}

/// Bus event: connection and rendering state (21 §13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum SlackEvent {
    /// Socket Mode is up.
    Connected {
        /// Live WebSocket connections.
        connections: u8,
    },
    /// The socket ended (auth failure, `disconnect` frame, transport); reconnecting.
    Disconnected {
        /// Why.
        reason: String,
    },
    /// A call was rate limited after morphism's retries.
    RateLimited {
        /// Slack method.
        method: String,
        /// `Retry-After`.
        retry_after_secs: u64,
    },
    /// A bubble left stream mode (21 §8.4).
    StreamFallback {
        /// Session whose turn it was.
        session: SessionId,
        /// The exact error code or reason.
        reason: String,
    },
    /// Tokens missing; nothing started.
    Unconfigured,
    /// Terminal mode: the channel only runs in `t --headless`.
    DaemonOnly,
}

impl wire::WireEvent for SlackEvent {
    const TOPIC: &'static str = "slack.events";
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<SlackEvent>().await
}

/// Per-method call counters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CallStats {
    /// `ok: true` answers.
    pub ok: u64,
    /// `ok: false` or transport failures.
    pub err: u64,
    /// 429s after retries.
    pub rate_limited: u64,
}

/// Snapshot served by [`Status`], `/theuth slack status`, `slack_status` and RPC (21 §13).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SlackStatus {
    /// Last state event; `None` before the supervisor has reported.
    pub state: Option<SlackEvent>,
    /// The bot's user id from `auth.test`.
    pub bot_user: Option<String>,
    /// The team id from `auth.test`.
    pub team: Option<String>,
    /// Owner user id.
    pub owner: String,
    /// Allowed user ids (excluding the owner).
    pub allowed: Vec<String>,
    /// Channels where `@theuth` is honoured.
    pub channels: Vec<String>,
    /// `channel:ts` → session bindings.
    pub threads: Vec<(String, SessionId)>,
    /// Streams currently open.
    pub open_streams: u32,
    /// Per-method counters.
    pub calls: BTreeMap<String, CallStats>,
    /// Envelopes dropped: unknown events and unparsable payloads.
    pub dropped_envelopes: u64,
    /// `MessageDelivered` events not rendered (110 §6).
    #[serde(default)]
    pub hidden_deliveries: u64,
    /// The latest `agents.sessions.setStatus` warning or error with what to
    /// fix (e.g. no Stop button: `missing_agent_session_stopped_event_subscription`);
    /// a clean `processing` answer clears it.
    #[serde(default)]
    pub session_status: Option<String>,
}

/// `ask` → [`SlackStatus`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Result<(), String>`: add a user id to `slack.allowed` (config file and live list).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Allow(pub String);

/// `ask` → `Result<(), String>`: remove a user id from `slack.allowed`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveAllowed(pub String);

/// `ask` → `Result<(), String>`: add a channel id to `slack.channels`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllowChannel(pub String);

/// `ask` → `Result<(), String>`: remove a channel id from `slack.channels`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveChannel(pub String);

/// The plugin. `channel: true` (headless) opens the socket; `false`
/// registers the actor and tool only (21 §3).
pub struct SlackPlugin {
    /// Run the Socket Mode channel.
    pub channel: bool,
}

/// Validates a Slack user id (`U…`/`W…`).
pub fn user_id(s: &str) -> Result<String, String> {
    let s = s.trim().trim_start_matches("<@").trim_end_matches('>');
    if s.len() > 1
        && (s.starts_with('U') || s.starts_with('W'))
        && s[1..].chars().all(|c| c.is_ascii_alphanumeric())
    {
        Ok(s.to_owned())
    } else {
        Err(format!("{s:?} is not a Slack user id (U…)"))
    }
}

/// Validates a Slack channel id (`C…`/`G…`).
pub fn channel_id(s: &str) -> Result<String, String> {
    let s = s.trim().trim_start_matches("<#").trim_end_matches('>');
    let s = s.split('|').next().unwrap_or(s);
    if s.len() > 1
        && (s.starts_with('C') || s.starts_with('G'))
        && s[1..].chars().all(|c| c.is_ascii_alphanumeric())
    {
        Ok(s.to_owned())
    } else {
        Err(format!("{s:?} is not a Slack channel id (C…)"))
    }
}

/// A list as a single-line YAML flow list of quoted strings.
fn yaml_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

/// The `"slack"` actor: control surface for the channel (daemon) or the
/// terminal-mode stand-in that only answers `Status` and edits config.
#[derive(kameo::Actor)]
pub struct Slack {
    handle: Option<channel::Handle>,
    cfg: SlackConfig,
    rt: runtime::Runtime,
    configured: bool,
}

impl Slack {
    fn config_path(&self) -> PathBuf {
        self.rt.data_dir.join("config.yaml")
    }

    fn persist(&self, key: &str, items: &[String]) -> Result<(), String> {
        let path = self.config_path();
        runtime::write_config_value(&path, "slack", key, &yaml_list(items))
            .map_err(|e| format!("write {}: {e}", path.display()))
    }

    fn offline_status(&self, state: SlackEvent) -> SlackStatus {
        SlackStatus {
            state: Some(state),
            bot_user: None,
            team: None,
            owner: self.cfg.owner.clone(),
            allowed: self.cfg.allowed.clone(),
            channels: self.cfg.channels.clone(),
            threads: threads::Threads::load(&channel::threads_path(&self.rt.data_dir)).all(),
            open_streams: 0,
            calls: BTreeMap::new(),
            dropped_envelopes: 0,
            hidden_deliveries: 0,
            session_status: None,
        }
    }
}

impl Message<Status> for Slack {
    type Reply = SlackStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match &self.handle {
            Some(h) => h.status(),
            None if !self.configured => self.offline_status(SlackEvent::Unconfigured),
            None => self.offline_status(SlackEvent::DaemonOnly),
        }
    }
}

impl Message<Allow> for Slack {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Allow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = user_id(&m.0)?;
        if let Some(h) = &self.handle {
            h.allow(id.clone())?;
        }
        if !self.cfg.allowed.contains(&id) {
            self.cfg.allowed.push(id);
        }
        self.persist("allowed", &self.cfg.allowed)
    }
}

impl Message<RemoveAllowed> for Slack {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveAllowed,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let id = user_id(&m.0)?;
        if let Some(h) = &self.handle {
            h.remove_allowed(id.clone())?;
        }
        self.cfg.allowed.retain(|a| *a != id);
        self.persist("allowed", &self.cfg.allowed)
    }
}

impl Message<AllowChannel> for Slack {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: AllowChannel, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = channel_id(&m.0)?;
        if let Some(h) = &self.handle {
            h.allow_channel(id.clone())?;
        }
        if !self.cfg.channels.contains(&id) {
            self.cfg.channels.push(id);
        }
        self.persist("channels", &self.cfg.channels)
    }
}

impl Message<RemoveChannel> for Slack {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveChannel,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let id = channel_id(&m.0)?;
        if let Some(h) = &self.handle {
            h.remove_channel(id.clone())?;
        }
        self.cfg.channels.retain(|c| *c != id);
        self.persist("channels", &self.cfg.channels)
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for SlackPlugin {
    fn name(&self) -> &'static str {
        "slack"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: SlackConfig = rt.section("slack").unwrap_or_else(|e| {
            tracing::debug!("no [slack] config section, using defaults: {e}");
            SlackConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("slack disabled by config");
            return Ok(());
        }
        let run_channel = self.channel;
        let rt = rt.clone();
        tokio::spawn(async move {
            // Tokens live in the vault (37 §3.6); wait for it, then connect.
            let vault = secrets::handle(&rt).await;
            if vault.is_none() {
                tracing::warn!("slack: secrets vault unavailable; starting without API keys");
            }
            let get = |k| vault.as_ref().and_then(|v| v.get(k, 0));
            let tokens = match (get("slack_bot"), get("slack_app")) {
                (Some(bot), Some(app)) => Some((bot.as_str().to_owned(), app.as_str().to_owned())),
                _ => {
                    tracing::warn!(
                        "slack: secrets `slack_bot` (xoxb-…) and `slack_app` (xapp-…) must both be set (t --slack-setup); slack not started"
                    );
                    None
                }
            };
            let configured = tokens.is_some();
            let handle = match tokens {
                Some(tokens) if run_channel => {
                    match channel::spawn(cfg.clone(), tokens, rt.clone()) {
                        Ok(h) => Some(h),
                        Err(e) => {
                            tracing::error!("slack: channel failed to start: {e:#}");
                            return;
                        }
                    }
                }
                _ => None,
            };
            let sink = handle.clone();
            let actor = Slack::spawn(Slack {
                handle,
                cfg,
                rt: rt.clone(),
                configured,
            });
            if let Err(e) = actor.register(SLACK_ACTOR) {
                tracing::error!("slack: registering {SLACK_ACTOR}: {e}");
                return;
            }
            if !configured {
                rt.publish(SlackEvent::Unconfigured).await;
            }

            // First use of the registry (00 §4, 12 §2); then the user sink (24 §5).
            let Some(reg) = rt
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await
            else {
                return;
            };
            let _ = reg
                .ask(tools::RegisterTool(Arc::new(SlackStatusTool)))
                .await;
            // 110 §7.2: the `## Slack` prompt section, like `voice`'s.
            let _ = reg
                .ask(tools::RegisterPrompt {
                    plugin: "slack",
                    text: prompt::PROMPT.to_owned(),
                })
                .await;
            let Some(sink) = sink else {
                return;
            };
            if let Some(users) = rt
                .lookup_when::<agent::UserChannels>(agent::USER_ACTOR)
                .await
            {
                let _ = users.ask(agent::RegisterUserSink(Arc::new(sink))).await;
            }
        });
        Ok(())
    }
}

/// Arguments for [`SlackStatusTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SlackStatusArgs {}

/// The 21 §13 snapshot as text.
pub struct SlackStatusTool;

#[async_trait::async_trait]
impl Tool for SlackStatusTool {
    const NAME: &'static str = "slack_status";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SlackStatusArgs;

    fn description(&self) -> String {
        "Slack connection state, owner, allowed users, channels, thread bindings and call counters."
            .into()
    }

    async fn call(&self, _: SlackStatusArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Ok(Some(actor)) = ActorRef::<Slack>::lookup(SLACK_ACTOR) else {
            return Ok(ToolOutput::err("slack actor is not running"));
        };
        let s = actor.ask(Status).await?;
        Ok(ToolOutput::ok(render_status(&s)))
    }
}

/// Plain-text rendering of a [`SlackStatus`].
pub fn render_status(s: &SlackStatus) -> String {
    let state = match &s.state {
        None => "unknown".to_owned(),
        Some(SlackEvent::Connected { connections }) => {
            format!("connected ({connections} connections)")
        }
        Some(SlackEvent::Disconnected { reason }) => format!("disconnected: {reason}"),
        Some(SlackEvent::RateLimited {
            method,
            retry_after_secs,
        }) => {
            format!("rate limited on {method}; retry in {retry_after_secs} s")
        }
        Some(SlackEvent::StreamFallback { session, reason }) => {
            format!("stream fallback for {session}: {reason}")
        }
        Some(SlackEvent::Unconfigured) => {
            "unconfigured (secrets slack_bot / slack_app missing; run t --slack-setup)".into()
        }
        Some(SlackEvent::DaemonOnly) => "daemon only (runs in t --headless)".into(),
    };
    let dash = |v: &[String]| {
        if v.is_empty() {
            "-".to_owned()
        } else {
            v.join(", ")
        }
    };
    let mut out = format!(
        "state: {state}\nbot: {}\nteam: {}\nowner: {}\nallowed: {}\nchannels: {}\nopen streams: {}\ndropped envelopes: {}\nhidden deliveries: {}\n",
        s.bot_user.as_deref().unwrap_or("-"),
        s.team.as_deref().unwrap_or("-"),
        if s.owner.is_empty() { "-" } else { &s.owner },
        dash(&s.allowed),
        dash(&s.channels),
        s.open_streams,
        s.dropped_envelopes,
        s.hidden_deliveries,
    );
    if let Some(note) = &s.session_status {
        out.push_str(&format!("session status: {note}\n"));
    }
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
    fn id_validation() {
        assert_eq!(user_id(" <@U0123ABC> ").unwrap(), "U0123ABC");
        assert!(user_id("C0123").is_err());
        assert!(user_id("U").is_err());
        assert_eq!(channel_id("<#C0123|general>").unwrap(), "C0123");
        assert_eq!(channel_id("G0123").unwrap(), "G0123");
        assert!(channel_id("U0123").is_err());
    }

    #[test]
    fn allowed_list_is_quoted_flow_yaml() {
        let l = yaml_list(&["U1".into(), "U2".into()]);
        assert_eq!(l, r#"["U1", "U2"]"#);
        let v: Vec<String> = serde_yaml::from_str(&l).unwrap();
        assert_eq!(v[1], "U2");
    }

    #[test]
    fn manifest_parses() {
        let m: serde_yaml::Value = serde_yaml::from_str(MANIFEST).unwrap();
        assert_eq!(m["features"]["slash_commands"][0]["command"], "/theuth");
        assert_eq!(m["settings"]["socket_mode_enabled"], true);
    }

    #[test]
    fn config_defaults_and_the_110_keys_parse() {
        let d = SlackConfig::default();
        assert!(d.plan && d.blocks && d.pictures && !d.canvases);
        assert_eq!(d.deliveries, Deliveries::Hidden);
        let c: SlackConfig = serde_yaml::from_str(
            "plan: false\ndeliveries: hidden\nblocks: false\npictures: false\ncanvases: true",
        )
        .unwrap();
        assert!(!c.plan && !c.blocks && !c.pictures && c.canvases);
        assert!(serde_yaml::from_str::<SlackConfig>("deliveries: collapsed").is_err());
    }
}
