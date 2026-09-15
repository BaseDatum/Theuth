//! Agent plugin: one supervisor actor (`"agent"`) routing to one actor per
//! session. Consumes tools through the `"tools"` registry; persists every
//! message to sqlite; publishes [`AgentEvent`] for any UI.
//!
//! - commands on the `"agent"` actor ([`Agents`]): [`SendInput`], [`Steer`],
//!   [`Withdraw`], [`ListQueued`], [`Undo`], [`Redo`], [`Cancel`], [`Compact`],
//!   [`SetCompactStyle`], [`SetTitleModel`], [`GetTitleModel`], [`ListSessions`], [`LoadSession`], [`RenameSession`], [`ListAgents`],
//!   [`ListModels`], [`SetModel`], [`SetEffort`], [`ShowPrompt`], [`ListSkills`],
//!   [`LoadSkill`], [`UseSkill`], [`AddSkill`], [`RemoveSkill`], [`AddSkillRepo`],
//!   [`RemoveSkillRepo`], [`SyncSkills`], [`ListLogins`], [`Login`], [`Logout`],
//!   [`ListProviders`], [`AddProvider`], [`RemoveProvider`], [`NewSession`],
//!   [`ListAcp`], [`AddAcp`], [`RemoveAcp`], [`ListRegistry`], [`Ancestors`]
//! - the `"user"` actor ([`UserChannels`]) and the `message_user` tool (design doc 24) in [`user`].

use std::path::PathBuf;
use std::sync::Arc;

use runtime::{InputId, SessionId, TurnId};
use serde::{Deserialize, Serialize};

pub mod acp_child;
mod backend;
pub mod compact;
pub mod frame;
pub mod loop_;
pub mod peers;
pub mod providers;
pub mod repl;
pub mod search;
pub mod skills;
pub mod snap;
pub mod spill;
pub mod store;
pub mod supervisor;
pub mod title;
pub mod todo;
pub mod user;

pub use compact::CompactStyle;
pub use supervisor::Agents;
pub use user::{
    Deliver, MessageUserTool, Outbound, OutboundFile, RegisterUserSink, USER_ACTOR, UserChannels,
    UserSink,
};

/// Registry name of the [`Agents`] supervisor actor.
pub const AGENT_ACTOR: &str = "agent";

/// `ask` → [`SessionId`]. `session: None` creates a new session. Returns as
/// soon as the turn is queued; progress arrives as [`AgentEvent`]s.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendInput {
    /// Existing session, or `None` for a new one.
    pub session: Option<SessionId>,
    /// User text.
    pub text: String,
    /// Image files to attach (17 §2); absolute paths on this machine.
    #[serde(default)]
    pub files: Vec<PathBuf>,
}

/// `ask` → `()`. Injected as a user message after the current tool batch
/// completes (oh-my-pi steering); the lifecycle arrives as
/// [`AgentEvent::InputQueued`] → [`AgentEvent::InputInjected`] |
/// [`AgentEvent::InputWithdrawn`]. Idle session → starts a turn now; the id
/// still resolves through `InputInjected`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Steer {
    /// Target session.
    pub session: SessionId,
    /// Client-minted id, echoed by the lifecycle events.
    pub id: InputId,
    /// User text.
    pub text: String,
    /// Image files to attach (17 §2); absolute paths on this machine.
    #[serde(default)]
    pub files: Vec<PathBuf>,
}

/// `ask` → `Result<String, String>`: pull a queued steer back before it enters
/// the context; `Ok(text)` is the removed input, `Err("not queued")` when it
/// was already injected (or never queued).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Withdraw {
    /// Target session.
    pub session: SessionId,
    /// The id given to [`Steer`].
    pub id: InputId,
}

/// `ask` → `Vec<QueuedInput>`: steers waiting for the current tool batch to end.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ListQueued {
    /// Target session.
    pub session: SessionId,
}

/// A user input waiting for the current tool batch to end.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueuedInput {
    /// The id given to [`Steer`].
    pub id: InputId,
    /// User text.
    pub text: String,
}

/// `ask` → `Result<String, String>`: hide the last turn (a user message and
/// everything after it) from the frame and transcript; `Ok` is the first line
/// of its user text. `Err("turn in flight" | "nothing to undo")`. Nothing is
/// deleted until the next write; [`Redo`] restores.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Undo {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<String, String>`: restore the most recently undone turn;
/// `Ok` is the first line of its user text. `Err("turn in flight" | "nothing to redo")`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Redo {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `()`. Abort the in-flight turn of `session` and of every running
/// descendant (design doc 19 §2.3); each emits `TurnFinished { stop_reason: "cancelled" }`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Cancel {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<u32 /* epoch */, String>`: compact `session` now,
/// threshold ignored (design doc 07 §8.6). Refused while a turn is running.
/// `style: None` uses the configured default (design doc 11).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Compact {
    /// Session to compact.
    pub session: SessionId,
    /// One-shot style; `None` = configured default.
    #[serde(default)]
    pub style: Option<CompactStyle>,
}

/// `ask` → `Result<(), String>`: persist `agent.compact_style` and update live sessions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetCompactStyle {
    /// New default style.
    pub style: CompactStyle,
}

/// `ask` → [`CompactStyle`]: the live default style.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetCompactStyle;

/// `ask` → `Result<Option<ModelInfo>, String>`: persist `agent.title_model` and
/// use it for every session's next title call; `None` clears it (session model).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetTitleModel {
    /// `provider/model`, bare id, or `None`.
    pub model: Option<String>,
}

/// `ask` → `Option<ModelInfo>`: the live title model, if set.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetTitleModel;

/// `ask` → `Result<knowledge::HarvestResult, String>`: harvest a session's
/// finished frame now (design doc 10 §5.2). `Err("turn in flight")` while it runs.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Harvest {
    /// Session.
    pub session: SessionId,
}

/// `ask` → `Vec<SessionSummary>`, newest first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSessions;

/// `ask` → [`Loaded`] for rendering a resumed session. `before`/`limit` page
/// the rendered entries (design doc 25 §7): entries with `seq < before`, the
/// last `limit` of them; both `None` = everything.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LoadSession {
    /// Target session.
    pub session: SessionId,
    /// Only entries older than this seq (a previous page's `first_seq`).
    #[serde(default)]
    pub before: Option<i64>,
    /// At most this many entries, the newest of the range.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `ask` → `Result<(), String>`: set a session's title by hand and announce it
/// as [`AgentEvent::TitleChanged`]. Trimmed, cut at 200 chars; `Err("empty title")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenameSession {
    /// Target session.
    pub session: SessionId,
    /// New title.
    pub title: String,
}

/// `ask` → `Result<(), String>`: delete a session and every descendant — rows,
/// messages, epochs — and drop their actors; one [`AgentEvent::SessionDeleted`]
/// per removed session. `Err` while any of them has a turn in flight.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteSession {
    /// Root of the subtree to delete.
    pub session: SessionId,
}

/// `ask` → `Result<Vec<search::SearchHit>, String>`: fuzzy search (nucleo, design
/// doc 26) over every session's title and its user/assistant message text; the
/// best hit per session, best score first. Empty query → no hits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchSessions {
    /// Fuzzy pattern (fzf syntax: space-separated atoms, `'exact`, `^prefix`, `suffix$`, `!not`).
    pub query: String,
    /// Hits to return; `None` → [`search::DEFAULT_LIMIT`], clamped to [`search::MAX_LIMIT`].
    pub limit: Option<u32>,
}

/// Reply to [`LoadSession`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Loaded {
    /// Rendered messages so far (an in-flight round is not yet stored).
    pub history: Vec<HistoryEntry>,
    /// A turn is in flight; its events keep streaming.
    pub running: bool,
    /// Canonical model id the session runs on.
    pub model: String,
    /// Reasoning effort in force; `None` = provider default.
    pub effort: Option<String>,
    /// Steers waiting for the current tool batch; empty when idle.
    pub queued: Vec<QueuedInput>,
    /// Turns hidden by [`Undo`] (the redo depth); 0 when none.
    pub undone: u32,
    /// External harness (`agents.yaml` key) when this is an ACP child (design doc 06).
    pub agent: Option<String>,
    /// Peer name when this is a subagent; `None` for a top-level session.
    pub name: Option<String>,
    /// Parent session when this is a subagent (where `/parent` goes).
    pub parent: Option<SessionId>,
    /// Skills whose body is in the stored frame (design doc 04 §2.4), sorted.
    pub skills: Vec<String>,
    /// Seq of the oldest entry in `history` (pass as `before` for the previous page); `None` when empty.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub first_seq: Option<i64>,
    /// Older entries exist beyond `history` (only with a `limit`).
    pub more: bool,
}

/// `ask` → `Vec<AgentInfo>`: the subtree under `session` (children,
/// grandchildren, …) in pre-order — parents before children, oldest first
/// within a level — each with its live state (design doc 19 §2.1). Frontends
/// sort (running first, …).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ListAgents {
    /// Root of the subtree (not included itself).
    pub session: SessionId,
}

/// `ask` → `Vec<AncestorInfo>`: `session` first, then its parent, …, the root
/// last (design doc 24 §2), walked over live sessions. Only the root carries a
/// title. An unknown session is returned alone.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Ancestors {
    /// Where to start.
    pub session: SessionId,
}

/// One link of an [`Ancestors`] chain.
#[derive(Clone, Debug, Serialize, Deserialize, kameo::Reply)]
pub struct AncestorInfo {
    /// Session id.
    pub id: SessionId,
    /// The session's title, non-empty; loaded for the root only.
    pub title: Option<String>,
    /// Peer name when this is a subagent.
    pub name: Option<String>,
}

/// One subagent in a session's subtree with its live state (design doc 19 §2.1).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AgentInfo {
    /// Session id.
    pub id: SessionId,
    /// Peer name (unique per parent).
    pub name: String,
    /// Owning session.
    pub parent: SessionId,
    /// 1 = direct child of the asked session.
    pub depth: u8,
    /// External harness (`agents.yaml` key); `None` = wth.
    pub agent: Option<String>,
    /// Model used.
    pub model: String,
    /// Live state.
    pub state: AgentState,
    /// Start of the current turn; `None` when not running.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last `TurnFinished` seen in this process; `None` before one.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// First line of the last assistant text, ≤ 80 chars; `""` when none.
    pub last: String,
}

/// Live state of one subagent (design doc 19 §2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentState {
    /// Never ran, or the last turn finished normally.
    Idle,
    /// A turn is in flight.
    Running,
    /// Blocked in `send_message{await_reply}` on `from` (`"parent"` or a child name).
    Awaiting {
        /// Whose answer it waits for.
        from: String,
    },
    /// An `ApprovalRequested` for this session is unanswered.
    Approval {
        /// Tool waiting on the human.
        tool: String,
    },
    /// The last turn ended with `stop_reason = "error"`.
    Error,
    /// The last turn ended `"cancelled"` (incl. the `max_runtime` timeout).
    Cancelled,
}

impl AgentState {
    /// The state as one lowercase word: `idle|running|awaiting|approval|error|cancelled`.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Awaiting { .. } => "awaiting",
            Self::Approval { .. } => "approval",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// `ask` → `Vec<ModelInfo>`: every catalog model whose provider is configured,
/// sorted by provider then id.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListModels;

/// `ask` → `Result<ModelInfo, String>`. Resolves `model` (design doc 02 §1);
/// switches `session` (if given) for its next model call, makes the id the
/// default for new sessions and writes it back to `<data_dir>/config.yaml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetModel {
    /// Session to switch; `None` changes only the default.
    pub session: Option<SessionId>,
    /// `provider/model`, or a bare model id resolved against configured providers.
    pub model: String,
}

/// `ask` → `Result<(), String>`: set (or clear with `None`) the reasoning
/// effort. Validated against the session's current model via the catalog
/// (unknown/passthrough models accept any genai keyword or integer budget).
/// Like [`SetModel`]: switches `session` (from its next turn) and makes the
/// value the default (`agent.effort` in config.yaml).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEffort {
    /// Session to switch; `None` changes only the default.
    pub session: Option<SessionId>,
    /// Effort keyword or token budget; `None` clears.
    pub effort: Option<String>,
}

/// One selectable model.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ModelInfo {
    /// `provider/model`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// models.dev provider id.
    pub provider: String,
    /// Context window in tokens; 0 when unknown.
    pub context: u32,
    /// Supports reasoning.
    pub reasoning: bool,
    /// Accepts image input (catalog); false when unknown.
    pub image: bool,
    /// Effort keywords the model accepts (catalog); empty when unknown.
    pub efforts: Vec<String>,
    /// Smallest `budget_tokens` value when the model takes one.
    pub budget_min: Option<u32>,
    /// Largest `budget_tokens` value, when bounded.
    pub budget_max: Option<u32>,
}

/// Effort keyword or integer budget → genai. `"default"` → `None` (provider default).
pub fn effort_of(s: &str) -> Option<genai::chat::ReasoningEffort> {
    use genai::chat::ReasoningEffort as E;
    match s {
        "off" | "none" | "zero" => Some(E::Zero),
        "on" => Some(E::Medium),
        "default" => None,
        _ => s.parse().map(E::Budget).ok().or_else(|| E::from_keyword(s)),
    }
}

/// `ask` → `String`: the assembled system prompt (design doc 04 §1.1) as
/// `session` sees it; without a session the peers section is omitted.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ShowPrompt {
    /// Session whose peers block to include.
    pub session: Option<SessionId>,
}

/// `ask` → `Vec<SkillMeta>`, sorted by name.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSkills;

/// `ask` → `Result<SkillBody, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoadSkill {
    /// Skill name.
    pub name: String,
}

/// `ask` → `Result<SessionId, String>`: hand a skill to the model as user
/// input (design doc 04 §2.4) — the body [`LoadSkill`] returns, wrapped in
/// `<skill name="…">`, then `args` as the user's words. Same delivery as
/// [`SendInput`]: a new turn when idle, a steer when one is running.
/// `Err` names an unknown skill.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UseSkill {
    /// Existing session, or `None` for a new one.
    pub session: Option<SessionId>,
    /// Skill name.
    pub name: String,
    /// What the user wants done with it; may be empty.
    pub args: String,
}

/// `ask` → `Result<SkillMeta, String>`: write `<skills>/<name>.md`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddSkill {
    /// `[a-z0-9][a-z0-9._-]*`.
    pub name: String,
    /// Index line.
    pub description: String,
    /// Markdown body.
    pub body: String,
    /// Inline into every system prompt instead of indexing.
    pub always: bool,
}

/// `ask` → `Result<(), String>`: delete a local skill.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveSkill {
    /// Skill name.
    pub name: String,
}

/// `ask` → `Result<Vec<SkillMeta>, String>`: clone a skill repo; returns its skills.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddSkillRepo {
    /// Git URL.
    pub url: String,
    /// Directory name; defaults to the repo basename.
    pub name: Option<String>,
}

/// `ask` → `Result<(), String>`: delete a cloned skill repo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveSkillRepo {
    /// Directory name.
    pub name: String,
}

/// `ask` → `Vec<SyncResult>`: `git pull` every skill repo.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct SyncSkills;

/// `ask` → `Vec<LoginInfo>`: every provider a user can log in to (design doc 05 §2).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListLogins;

/// One OAuth provider row.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LoginInfo {
    /// genai namespace (`openai`).
    pub name: String,
    /// Tokens on disk.
    pub logged_in: bool,
    /// Account the tokens belong to, when the provider tells us.
    pub account: Option<String>,
    /// Access token expiry, unix seconds.
    pub expires_at: Option<u64>,
}

/// `ask` → `Result<String, String>`: the URL to open; the browser is also
/// opened. Completion arrives as [`LoginFinished`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Login {
    /// genai namespace (`openai`).
    pub provider: String,
}

/// `ask` → `Result<(), String>`: delete the provider's tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Logout {
    /// genai namespace (`openai`).
    pub provider: String,
}

/// Event: the login started by [`Login`] ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct LoginFinished {
    /// genai namespace (`openai`).
    pub provider: String,
    /// `None` on success.
    pub error: Option<String>,
}

/// `ask` → `Vec<ProviderInfo>`: named OpenAI-compatible gateways.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListProviders;

/// One named OpenAI-compatible gateway.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderInfo {
    /// User-facing prefix (`rwx` → `rwx/model`).
    pub name: String,
    /// OpenAI-compatible base URL.
    pub base_url: String,
    /// Models last fetched from `GET {base}models`.
    pub models: u32,
}

/// `ask` → `Result<ProviderInfo, String>`: persist a gateway, fetch its models.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddProvider {
    /// `[a-z][a-z0-9_-]*`, not a built-in adapter.
    pub name: String,
    /// `https://host/v1`.
    pub base_url: String,
    /// Bearer key; stored in `credentials.yaml`.
    pub key: String,
}

/// `ask` → `Result<(), String>`: delete a gateway's url, key, and catalog models.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveProvider {
    /// Gateway name.
    pub name: String,
}

/// `ask` → `Result<SessionId, String>`: create an empty root session, for
/// ACP `session/new` (design doc 06 §4.1); `cwd` is recorded on the row.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewSession {
    /// The client's working directory.
    pub cwd: Option<PathBuf>,
}

/// `ask` → `Vec<acp::EntryInfo>`: configured external harnesses
/// (`<data_dir>/acp/agents.yaml`); `probe` runs `initialize` against each.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListAcp {
    /// Spawn each harness and report whether `initialize` succeeds.
    pub probe: bool,
}

/// `ask` → `Result<acp::EntryInfo, String>`: install a registry agent by id
/// and write its `agents.yaml` entry (design doc 06 §3.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddAcp {
    /// Registry id.
    pub id: String,
}

/// `ask` → `Result<(), String>`: delete an `agents.yaml` entry and its `bin/<name>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveAcp {
    /// Entry name.
    pub name: String,
}

/// `ask` → `Result<Vec<acp::registry::RegistryAgent>, String>`: the ACP
/// registry (cached a day; offline serves the cache).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListRegistry;

/// One indexed skill.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SkillMeta {
    /// Name (front matter, else file stem / directory name).
    pub name: String,
    /// Index line.
    pub description: String,
    /// Absolute path of the skill file.
    pub path: String,
    /// Inlined into the system prompt instead of indexed.
    pub always: bool,
    /// Clone directory name when the skill lives in a git repo under skills/.
    pub repo: Option<String>,
}

/// A loaded skill.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SkillBody {
    /// Name.
    pub name: String,
    /// Markdown without front matter.
    pub body: String,
    /// Other files of a `SKILL.md` skill, relative to its directory; empty otherwise.
    pub files: Vec<String>,
}

/// One row of the session list.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionSummary {
    /// Id.
    pub id: SessionId,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last activity: creation or the last appended message. The list order.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Model-written name of what the session is about now (07 §15); the first input, truncated, until then.
    pub title: String,
    /// Model used.
    pub model: String,
    /// Parent session when this is a subagent.
    pub parent: Option<SessionId>,
    /// Peer name; `None` for top-level sessions.
    pub name: Option<String>,
    /// Reasoning effort in force; `None` = provider default.
    pub effort: Option<String>,
    /// External harness (`agents.yaml` key) when this is an ACP child (design doc 06).
    pub agent: Option<String>,
    /// Working directory recorded by ACP (`session/new` or `spawn_subagent.cwd`).
    pub cwd: Option<String>,
}

/// Rendered history for UIs (tool traffic collapsed to summaries).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// User text.
    User {
        /// Text.
        text: String,
    },
    /// Assistant text.
    Assistant {
        /// Text.
        text: String,
    },
    /// A tool call and its result; `result` is `None` while the call is
    /// still running (a session loaded mid-turn), and `ToolCallFinished`
    /// with the same `call_id` completes it.
    Tool {
        /// Provider call id (matches `ToolCallFinished`).
        call_id: String,
        /// Tool name.
        name: String,
        /// Raw arguments.
        args: serde_json::Value,
        /// `(output, is_error)` once finished; output possibly truncated.
        result: Option<(String, bool)>,
        /// Files the tool produced (23 §5), for UIs that render them inline.
        #[serde(default)]
        files: Vec<PathBuf>,
    },
    /// A compaction boundary: everything before it was folded into `summary`
    /// (design doc 07 §8.4).
    Compacted {
        /// Epoch number the boundary opened.
        epoch: u32,
        /// The summary text the model works from.
        summary: String,
        /// How the head was built; missing on old rows → summary.
        #[serde(default)]
        style: CompactStyle,
    },
}

/// Token usage for a turn.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Usage {
    /// Prompt tokens (cached + written + fresh).
    pub prompt: u32,
    /// Completion tokens.
    pub completion: u32,
    /// Prompt tokens read from the provider's prefix cache.
    pub cached: u32,
}

/// Everything a UI needs to render a session live. Every variant carries the
/// session and turn; UIs filter by session.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum AgentEvent {
    /// A user message was accepted and a model request is starting.
    TurnStarted {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// The user text (echo for UIs that did not send it).
        text: String,
        /// Canonical model id the turn runs on.
        model: String,
    },
    /// Streamed assistant text.
    TextDelta {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Fragment.
        text: Arc<str>,
    },
    /// Streamed reasoning text (providers that expose it).
    ReasoningDelta {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Fragment.
        text: Arc<str>,
    },
    /// The model has begun emitting a tool call (name known, arguments still
    /// streaming). Lets a UI show activity before `ToolCallStarted`.
    ToolCallPending {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id (matches `ToolCallStarted`).
        call_id: String,
        /// Tool name.
        name: String,
    },
    /// The model asked for a tool; the call is now routed to the registry.
    ToolCallStarted {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id.
        call_id: String,
        /// Tool name.
        name: String,
        /// Raw arguments; UIs format these per tool.
        args: serde_json::Value,
    },
    /// The tool returned.
    ToolCallFinished {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id.
        call_id: String,
        /// Output text, possibly truncated for display.
        output: String,
        /// Whether it failed.
        is_error: bool,
        /// Files the tool produced (screenshots, generated images); absolute paths on this machine.
        #[serde(default)]
        files: Vec<PathBuf>,
    },
    /// The turn ended (model finished, was cancelled, or errored).
    TurnFinished {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider stop reason, or `"cancelled"` / `"error"`.
        stop_reason: String,
        /// Token usage summed over the turn's model calls.
        usage: Usage,
        /// Prompt + completion tokens of the last model call: what the window holds now.
        context: u32,
    },
    /// Non-fatal or fatal error text for the UI.
    Error {
        /// Session.
        session: SessionId,
        /// Turn, if one was in flight.
        turn: Option<TurnId>,
        /// Message.
        message: String,
    },
    /// A [`Steer`] is waiting for the current tool batch to end.
    InputQueued {
        /// Session.
        session: SessionId,
        /// The steer's id.
        id: InputId,
        /// User text.
        text: String,
    },
    /// A queued input entered the context as a user message; published before
    /// `TurnStarted` when queued inputs start a turn.
    InputInjected {
        /// Session.
        session: SessionId,
        /// The steer's id.
        id: InputId,
        /// Turn it now belongs to.
        turn: TurnId,
    },
    /// A queued input was pulled back by [`Withdraw`]; it never entered the context.
    InputWithdrawn {
        /// Session.
        session: SessionId,
        /// The steer's id.
        id: InputId,
    },
    /// A subagent session was created for `parent`.
    SubagentSpawned {
        /// Owning session.
        parent: SessionId,
        /// New session.
        child: SessionId,
        /// Peer name, unique among the parent's children.
        name: String,
    },
    /// Text injected into `session` by the harness (subagent result, peer
    /// message or finished job), for transcripts.
    MessageDelivered {
        /// Receiving session.
        session: SessionId,
        /// Sender: a child's name, `"parent"`, or `"job j3"` for a `<job_result>`.
        from: String,
        /// Delivered text without its wrapper.
        text: String,
    },
    /// The session's model was switched (takes effect on its next model call).
    ModelChanged {
        /// Session.
        session: SessionId,
        /// Canonical model id.
        model: String,
    },
    /// The session's reasoning effort was set or cleared (takes effect on its next turn).
    EffortChanged {
        /// Session.
        session: SessionId,
        /// Effort keyword or budget; `None` = provider default.
        effort: Option<String>,
    },
    /// Compaction started: the summariser call is in flight (design doc 07 §8).
    Compacting {
        /// Session.
        session: SessionId,
        /// Turn, when triggered mid-turn; `None` for `/compact`.
        turn: Option<TurnId>,
    },
    /// Compaction finished: a new epoch opened.
    Compacted {
        /// Session.
        session: SessionId,
        /// Turn, when triggered mid-turn; `None` for `/compact`.
        turn: Option<TurnId>,
        /// New epoch number.
        epoch: u32,
        /// Projected tokens before the cut.
        before: u32,
        /// Messages folded into the summary.
        folded: u32,
        /// Style actually used (cut if the summariser fell back).
        style: CompactStyle,
    },
    /// The harvest continuation is in flight (design doc 10 §5.2); precedes `TurnFinished`.
    Harvesting {
        /// Session.
        session: SessionId,
    },
    /// A harvest finished for a session (design doc 10 §5.4).
    Harvested {
        /// Session.
        session: SessionId,
        /// Concepts created.
        added: u32,
        /// Concepts updated.
        updated: u32,
    },
    /// The set of skills whose body is in the frame changed (design doc 04
    /// §2.4): a user injection, a `load_skill` call, or a compaction that
    /// dropped them all.
    SkillsChanged {
        /// Session.
        session: SessionId,
        /// Skill names, sorted; empty right after a compaction.
        skills: Vec<String>,
    },
    /// The session's title was set: first words of the first input, the
    /// titler's rewrite (07 §15), or [`RenameSession`].
    TitleChanged {
        /// Session.
        session: SessionId,
        /// New title, non-empty.
        title: String,
    },
    /// The session's rows are gone ([`DeleteSession`]); lists drop it.
    SessionDeleted {
        /// Session.
        session: SessionId,
    },
}

impl AgentEvent {
    /// Session the event belongs to; `None` for global events.
    pub fn session(&self) -> Option<SessionId> {
        match self {
            Self::TurnStarted { session, .. }
            | Self::TextDelta { session, .. }
            | Self::ReasoningDelta { session, .. }
            | Self::ToolCallPending { session, .. }
            | Self::ToolCallStarted { session, .. }
            | Self::ToolCallFinished { session, .. }
            | Self::TurnFinished { session, .. }
            | Self::InputQueued { session, .. }
            | Self::InputInjected { session, .. }
            | Self::InputWithdrawn { session, .. }
            | Self::MessageDelivered { session, .. }
            | Self::ModelChanged { session, .. }
            | Self::EffortChanged { session, .. }
            | Self::Compacting { session, .. }
            | Self::Compacted { session, .. }
            | Self::Harvesting { session, .. }
            | Self::Harvested { session, .. }
            | Self::SkillsChanged { session, .. }
            | Self::TitleChanged { session, .. }
            | Self::SessionDeleted { session }
            | Self::Error { session, .. } => Some(*session),
            Self::SubagentSpawned { child, .. } => Some(*child),
        }
    }
}

/// `[agent]` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Model id, `provider/model` (`anthropic/claude-sonnet-4-5`) or bare
    /// (`claude-sonnet-4-5`, resolved against configured providers).
    pub model: String,
    /// Reasoning effort for new sessions (`low`, `high`, `off`, a token
    /// budget, …; what the model accepts); unset = provider default.
    pub effort: Option<String>,
    /// Max tool-call rounds per turn before the loop stops itself.
    pub max_tool_rounds: u32,
    /// Max characters of tool output sent back to the model.
    pub max_tool_output_chars: usize,
    /// Subagent limits and model.
    pub subagents: SubagentsConfig,
    /// Auto-compaction threshold as a fraction of the model's context window;
    /// `0` disables it (design doc 07 §8.1).
    pub compact_at: f32,
    /// How auto-compaction, overflow, and a bare `/compact` fold history:
    /// `summary` | `cut` | `snap`.
    pub compact_style: CompactStyle,
    /// Model for session titles (07 §15), ideally cheap and fast; unset = the session's model.
    pub title_model: Option<String>,
    /// How the loop uses the knowledge plugin (12 §9.2); the store itself is `knowledge.enabled`.
    pub knowledge: KnowledgeUse,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-sonnet-4-5".into(),
            effort: None,
            max_tool_rounds: 32,
            max_tool_output_chars: 32_000,
            subagents: SubagentsConfig::default(),
            compact_at: 0.8,
            compact_style: CompactStyle::Summary,
            title_model: None,
            knowledge: KnowledgeUse::default(),
        }
    }
}

/// `agent.knowledge`: how the loop uses the knowledge plugin (12 §9.2); the
/// store itself is `knowledge.enabled`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeUse {
    /// Lines per `<knowledge>` note.
    pub hint_limit: u32,
    /// Mid-turn hint cadence in rounds.
    pub hint_rounds: u32,
    /// Tool-using root turns per harvest; `0` disables harvest.
    pub harvest_every: u32,
    /// Documents accepted per harvest.
    pub harvest_max: u32,
}

impl Default for KnowledgeUse {
    fn default() -> Self {
        Self {
            hint_limit: 5,
            hint_rounds: knowledge::KNOWLEDGE_HINT_ROUNDS,
            harvest_every: 3,
            harvest_max: 8,
        }
    }
}

/// `[agent.subagents]` config section (design doc 01 §2.6).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentsConfig {
    /// Child model; defaults to `agent.model`.
    pub model: Option<String>,
    /// Child reasoning effort; defaults to `agent.effort`.
    pub effort: Option<String>,
    /// Sessions at depth ≥ this cannot spawn.
    pub max_depth: u8,
    /// Concurrent child turns; further spawns queue.
    pub max_concurrent: usize,
    /// Wall clock per child turn; on expiry the turn is cancelled.
    pub max_runtime_secs: u64,
    /// How long `send_message{await_reply}` waits.
    pub reply_timeout_secs: u64,
}

impl Default for SubagentsConfig {
    fn default() -> Self {
        Self {
            model: None,
            effort: None,
            max_depth: 1,
            max_concurrent: 8,
            max_runtime_secs: 600,
            reply_timeout_secs: 120,
        }
    }
}

/// The plugin.
pub struct AgentPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for AgentPlugin {
    fn name(&self) -> &'static str {
        "agent"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        supervisor::start(rt).await
    }
}
