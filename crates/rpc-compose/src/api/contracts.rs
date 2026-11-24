use std::collections::HashSet;
use agent::{CommunicationState, CompactStyle, AdvisorDefaults};
use kameo::actor::ActorRef;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use tools::{Approval, ApprovalRequested, ApprovalsState, AskRequested};
use rpc_core::PrivateMap;
use wire::EventPolicy;

/// Reply to `info`: what a client needs before its first session (25 §7);
/// the in-process TUI reads the same things off the [`runtime::Runtime`] directly.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Info {
    /// Harness version (the product version, `platform::version`).
    pub version: String,
    /// Host OS (`std::env::consts::OS`: `linux`, `macos`, `windows`, …); a
    /// parent shows it beside a node's name (32 §6.2).
    pub platform: String,
    /// `<data_dir>`: config, sqlite, logs, attachments.
    pub data_dir: String,
    /// `agent.model`: the model new sessions start on (live, 82 §3).
    pub default_model: String,
    /// `agent.effort`: the reasoning effort new sessions start on (live, 83 §3);
    /// `None` = the provider default. Absent from an older harness → `None`.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// `agent.favorite_models` (127 §1): starred model ids in starring order,
    /// shown first in every picker. Absent from an older harness → empty.
    #[serde(default)]
    pub favorite_models: Vec<String>,
    /// The global approval switch (`tools.approvals`).
    pub approvals: ApprovalsState,
    /// `agent.compact_style`.
    pub compact_style: CompactStyle,
    /// `agent.subagents.max_runtime_secs`.
    pub subagent_timeout_secs: u64,
    /// `schedule.run_timeout_secs` (88): seconds a routine run may take before it is
    /// cancelled and its history row says `timeout`; a routine's own `run_timeout:`
    /// overrides it. `0` from an older harness.
    #[serde(default)]
    pub schedule_run_timeout_secs: u64,
    /// `tasks.home_offline_ask_secs` (182 D9): how long a task's home machine may be
    /// offline, with tasks waiting on it, before the Attention item asks. `0` from an older harness.
    #[serde(default)]
    pub home_offline_ask_secs: u64,
    /// `agent.max_tool_rounds`: tool-call rounds a turn may run before the loop lands.
    pub max_tool_rounds: u32,
    /// `agent.stream_idle_secs`: seconds without a stream event before a model call is abandoned.
    pub stream_idle_secs: u64,
    /// `agent.max_salvages`: interrupted-reply recoveries per turn before it stops with an error.
    pub max_salvages: u32,
    /// `agent.status_every` (212 §15): tool calls between status reminders for new
    /// sessions; `0` = off. `0` from an older harness.
    #[serde(default)]
    pub status_every: u32,
    /// `agent.knowledge.hint_limit` (212 §15): concepts per memory hint for new
    /// sessions; `0` = no hints. `0` from an older harness.
    #[serde(default)]
    pub hint_limit: u32,
    /// `agent.auto_archive.enabled`: root chats with no event for
    /// `auto_archive_after_secs` are archived. `false` from an older harness.
    #[serde(default)]
    pub auto_archive_enabled: bool,
    /// `agent.auto_archive.after` in seconds (default 14 days). `0` from an older harness.
    #[serde(default)]
    pub auto_archive_after_secs: u64,
    /// `knowledge.peek_threshold` (212 §15): the live minimum P(true) for
    /// `peek_memory` to call a memory relevant. `0` from an older harness.
    #[serde(default)]
    pub peek_threshold: f32,
    /// `tools.pty_idle_secs`: quiet-screen seconds before a PTY job wakes the model with `<job_idle>` (61); `0` = never.
    pub pty_idle_secs: u64,
    /// Harness display name (`link.name`, else the hostname): what a linked
    /// device shows in its harness switcher (31 §6.4).
    pub name: String,
    /// The global communication switch (50 §10): `session: None`, the
    /// authoritative ceiling every per-session state inherits.
    pub communication: CommunicationState,
    /// The app plugin is running (`app.enabled`, 46 §9): `get_app`/`app_*`
    /// answer and the `app` tool exists. `false` leaves app artifacts
    /// readable as source only. Absent from an older harness → `false`.
    #[serde(default)]
    pub app_enabled: bool,
    /// 125 §6.5: the advisor's remembered pick and tuning keys (`advisor:` section).
    /// Absent from an older harness → defaults.
    #[serde(default)]
    pub advisor: AdvisorDefaults,
}

/// The first item of every `subscribe_events` stream: every approval,
/// `ask_user` and `secret_request` still waiting on a user, filtered like the
/// live events. A client subscribing (or reconnecting over the relay) after
/// the `*Requested` broadcast still gets the card, and one that missed the
/// `*Resolved` drops it: after this item the client's pending set is exactly
/// what the engine reports (31 §7.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct PendingPrompts {
    /// Pending approvals.
    pub approvals: Vec<ApprovalRequested>,
    /// Pending `ask_user` calls.
    pub asks: Vec<AskRequested>,
    /// Pending `secret_request` calls (`SecretRequested` variants only).
    pub secrets: Vec<secrets::SecretsEvent>,
}

impl PendingPrompts {
    /// The engine's pending prompts that `keep` admits (one item at a time, as
    /// its `{"type": …}` JSON). An actor that is not running contributes none.
    pub async fn collect(mut keep: impl FnMut(&serde_json::Value) -> bool) -> Self {
        let mut out = Self::default();
        fn admit<T: Serialize>(
            items: Vec<T>,
            keep: &mut impl FnMut(&serde_json::Value) -> bool,
        ) -> Vec<T> {
            items
                .into_iter()
                .filter(|i| serde_json::to_value(i).is_ok_and(|v| keep(&v)))
                .collect()
        }
        if let Ok(Some(a)) = ActorRef::<Approval>::lookup(tools::APPROVAL_ACTOR)
            && let Ok(v) = a.ask(tools::PendingApprovals).await
        {
            out.approvals = admit(v, &mut keep);
        }
        if let Ok(Some(a)) = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)
            && let Ok(v) = a.ask(tools::PendingAsks).await
        {
            out.asks = admit(v, &mut keep);
        }
        if let Ok(Some(a)) = ActorRef::<secrets::SecretsActor>::lookup(secrets::SECRETS_ACTOR)
            && let Ok(v) = a.ask(secrets::PendingRequests).await
        {
            out.secrets = admit(v, &mut keep);
        }
        // 183 W-19: cards relayed from the owners of followed sessions, so this
        // snapshot (and the one after `Lagged`) keeps them.
        let relayed = sessionsync::events::relayed_prompts();
        out.approvals.extend(admit(relayed.approvals, &mut keep));
        out.asks.extend(admit(relayed.asks, &mut keep));
        out
    }
}

/// Always-delivered per-subscription controls, never published on the bus.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum StreamControl {
    /// Initial pending state and refresh after loss.
    PendingPrompts(PendingPrompts),
    /// Number of discarded frames.
    Lagged(Lagged),
}

/// The legacy loss notification body.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct Lagged {
    /// Frames discarded by the ring or this subscription's backlog.
    pub count: u64,
}

impl wire::WireEvent for StreamControl {
    const TOPIC: &'static str = "rpc.stream";
}

/// 188: the flags Settings had no write path for: four `<section>.enabled` and
/// the two `allow_private_network` keys behind Network's "Local network
/// access" (16 §5, amended 2026-11-11), 212 §15's switches, which apply to new
/// sessions without a restart, and 215's `onboarding.completed`. Any other string
/// fails to deserialize, an `INVALID_PARAMS` error listing these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum ConfigFlagKey {
    /// `decision.enabled` (57 §7.1): off, the decision actor escalates every ask.
    #[serde(rename = "decision.enabled")]
    Decision,
    /// `a2a.enabled` (183): the A2A HTTP client/server machinery.
    #[serde(rename = "a2a.enabled")]
    A2a,
    /// `computer.enabled` (27): the computer-use actor, tools and input thread.
    #[serde(rename = "computer.enabled")]
    Computer,
    /// `double.enabled` (126): the double actors, tools and prompt paragraph. Its own
    /// `double_settings_set` needs the running scheduler, so it cannot turn it on.
    #[serde(rename = "double.enabled")]
    Double,
    /// `web.allow_private_network`: the web tools reach the local network.
    #[serde(rename = "web.allow_private_network")]
    WebPrivateNetwork,
    /// `browser.allow_private_network`: the browser reaches the local network.
    #[serde(rename = "browser.allow_private_network")]
    BrowserPrivateNetwork,
    /// `agent.status_tool` (212 §15): new sessions get `set_status`, its prompt
    /// sentence and its reminders. Applies to new sessions, no restart.
    #[serde(rename = "agent.status_tool")]
    AgentStatusTool,
    /// `agent.auto_title` (212 §15): the title model names and renames new sessions.
    /// Applies to new sessions, no restart.
    #[serde(rename = "agent.auto_title")]
    AgentAutoTitle,
    /// `agent.suggestions`: "What's next?" suggestions after interactive chat turns.
    /// Applies to new sessions, no restart.
    #[serde(rename = "agent.suggestions")]
    AgentSuggestions,
    /// `knowledge.peek` (212 §15): new sessions list `peek_memory` and its hint and
    /// prompt sentences; off, its calls are refused at once. No restart.
    #[serde(rename = "knowledge.peek")]
    KnowledgePeek,
    /// `onboarding.completed` (215): the desktop's first-run wizard was finished or
    /// skipped. UI state only, nothing in the engine reads it; absent is `false`. No restart.
    #[serde(rename = "onboarding.completed")]
    OnboardingCompleted,
}

impl ConfigFlagKey {
    /// The config.yaml `(section, key)` this flag is.
    pub(super) fn path(self) -> (&'static str, &'static str) {
        match self {
            Self::Decision => ("decision", "enabled"),
            Self::A2a => ("a2a", "enabled"),
            Self::Computer => ("computer", "enabled"),
            Self::Double => ("double", "enabled"),
            Self::WebPrivateNetwork => ("web", "allow_private_network"),
            Self::BrowserPrivateNetwork => ("browser", "allow_private_network"),
            Self::AgentStatusTool => ("agent", "status_tool"),
            Self::AgentAutoTitle => ("agent", "auto_title"),
            Self::AgentSuggestions => ("agent", "suggestions"),
            Self::KnowledgePeek => ("knowledge", "peek"),
            Self::OnboardingCompleted => ("onboarding", "completed"),
        }
    }

    /// The flag in `config` as its plugin reads it at start: a missing or
    /// unreadable section is the section type's default.
    pub(super) fn read(self, config: &figment::Figment) -> bool {
        let (s, _) = self.path();
        match self {
            Self::Decision => {
                let c: decision::DecisionConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::A2a => {
                let c: a2a::config::A2aConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::Computer => {
                let c: computer::ComputerConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::Double => {
                let c: double::DoubleConfig = config.extract_inner(s).unwrap_or_default();
                c.enabled
            }
            Self::WebPrivateNetwork => {
                let c: web::WebConfig = config.extract_inner(s).unwrap_or_default();
                c.allow_private_network
            }
            Self::BrowserPrivateNetwork => {
                let c: browser::BrowserConfig = config.extract_inner(s).unwrap_or_default();
                c.allow_private_network
            }
            Self::AgentStatusTool | Self::AgentAutoTitle | Self::AgentSuggestions => {
                let c: agent::AgentConfig = config.extract_inner(s).unwrap_or_default();
                match self {
                    Self::AgentStatusTool => c.status_tool,
                    Self::AgentSuggestions => c.suggestions,
                    _ => c.auto_title,
                }
            }
            Self::KnowledgePeek => {
                let c: knowledge::KnowledgeConfig = config.extract_inner(s).unwrap_or_default();
                c.peek
            }
            Self::OnboardingCompleted => {
                config.extract_inner::<bool>("onboarding.completed").unwrap_or(false)
            }
        }
    }

    /// 212 §15: the switch the running engine applies to new sessions at once (no
    /// restart); `None` for the plugin flags that wait for one.
    pub(super) fn live(self) -> Option<agent::LoopFlag> {
        match self {
            Self::AgentStatusTool => Some(agent::LoopFlag::StatusTool),
            Self::AgentAutoTitle => Some(agent::LoopFlag::AutoTitle),
            Self::AgentSuggestions => Some(agent::LoopFlag::Suggestions),
            Self::KnowledgePeek => Some(agent::LoopFlag::KnowledgePeek),
            _ => None,
        }
    }

    /// Does the saved value take effect without a restart? The 212 §15 switches
    /// ([`Self::live`]) and 215's `onboarding.completed`, which only the UI reads.
    pub(super) fn restartless(self) -> bool {
        self.live().is_some() || self == Self::OnboardingCompleted
    }
}

/// 188: a [`ConfigFlagKey`] as config.yaml holds it and as this engine runs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConfigFlag {
    /// Which flag.
    pub key: ConfigFlagKey,
    /// The value `<data_dir>/config.yaml` holds now (the section's default when absent).
    pub enabled: bool,
    /// The value this process started with (its boot config); plugins keep it until a
    /// restart. A 212 §15 switch (`agent.status_tool`, `agent.auto_title`, `agent.suggestions`, `knowledge.peek`)
    /// or `onboarding.completed` (215) runs what the file holds: no restart.
    pub running: bool,
    /// `enabled != running`: the saved value applies after theuth restarts. Always
    /// `false` for a 212 §15 switch and `onboarding.completed`.
    pub restart_required: bool,
}

/// Does `peer` get `ev` past private ownership (51 §4.2)? An event naming a
/// private member in `session`, `parent` or `child` reaches only that tree's
/// owner; a `SubagentSpawned` (or 141 `ObservedChildChanged`) under a private
/// parent enrols `child` first so its own later events are filtered too.
/// Events naming no known member pass: durable trees are not tracked.
pub(super) fn admitted(policy: &EventPolicy, peer: u64, private: &PrivateMap) -> bool {
    let id = |s: Option<&str>| s.and_then(|s| s.parse::<SessionId>().ok());
    if policy.announces_child
        && let (Some(parent), Some(child)) =
            (id(policy.parent.as_deref()), id(policy.child.as_deref()))
    {
        let known = private.read().get(&parent).copied();
        if let Some((root, owner)) = known {
            private.write().insert(child, (root, owner));
            return owner == peer;
        }
    }
    let map = private.read();
    [policy.session.as_deref(), policy.parent.as_deref(), policy.child.as_deref()]
        .into_iter()
        .filter_map(id)
        .all(|s| map.get(&s).is_none_or(|&(_, owner)| owner == peer))
}

/// Does a subscription on `session` forward `ev`? With `tree` (descendants
/// mode) an event passes when its `session` is in the tree, or it is a
/// `SubagentSpawned` / `ObservedChildChanged` whose `parent` is — then
/// `child` joins the tree. Without, everything passes unless both sides name
/// a different session. `CommunicationChanged` always passes: a global or
/// ancestor switch changes every descendant's effective state, the filter
/// cannot see ancestry, and the event carries only a session id and a
/// revision (50 §10).
pub(super) fn wanted(
    policy: &EventPolicy,
    session: Option<&str>,
    tree: Option<&mut HashSet<String>>,
) -> bool {
    if policy.always_wanted {
        return true;
    }
    let Some(tree) = tree else {
        return match (session, policy.session.as_deref()) {
            (Some(want), Some(have)) => want == have,
            _ => true,
        };
    };
    if policy.session.as_deref().is_some_and(|s| tree.contains(s)) {
        return true;
    }
    if policy.announces_child
        && policy.parent.as_deref().is_some_and(|p| tree.contains(p))
        && let Some(child) = policy.child.as_deref()
    {
        tree.insert(child.to_owned());
        return true;
    }
    false
}
