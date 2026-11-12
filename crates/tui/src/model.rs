//! Pure UI state: bus events in, commands out. Shared by the fullscreen
//! renderer and the non-TTY line printer; no I/O here.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use ::search::SearchCategory;
use acp::EntryInfo;
use acp::registry::RegistryAgent;
use agent::durability::{ResumeCause, ResumeKind};
use agent::todo::{Status as TodoStatus, TodoSnapshot, TodoSource};
use agent::{
    AdvisorDefaults, AdvisorSeverity, AdvisorState, AdvisorStatus, Advisory, AdvisoryKind,
    AgentEvent, AgentInfo, AgentState, AwsProfiles, CommunicationChanged, CommunicationState,
    CompactStyle, ContextAttachment, ContextEntry, ContextItem, ContextKind, ContextRemoval,
    ContextSnapshot, ContextTarget, HistoryEntry, Loaded, ModelInfo, ObservedMeta, ObservedSource,
    ObservedState, PeerDelivered, PeerInfo, PeerMessageChanged, PeerMessagePage, PeerOrigin,
    PeerPage, PeerReceipt, PeerRecord, PeerScope, ProviderInfo, QueuedInput, SessionKind,
    SessionSummary, SkillBody, SkillMeta, Usage, VisionNote, VisionStage, VisionStatus,
};
use agent::{
    BroadcastReceipt, BroadcastRequest, BroadcastTargets, CleanupStatus, CloseResult, CostBucket,
    CostScope, Coverage, CreateSessionOptions, MemoryMode, Money, Retention, SessionCost,
    SessionOpen,
};
use double::{DoubleAnswered, DoublePredicted};
use iocraft::{KeyCode, KeyModifiers};
use knowledge::{Change, ChangeStatus, HarvestResult, History, Meta, Revision};
use models::Catalog;
use runtime::{InputId, SessionId, TurnId};
use tokio::sync::mpsc::UnboundedSender;
use tools::git::SyncResult;
use tools::{
    Answer, ApprovalId, ApprovalRequested, ApprovalResolved, ApprovalsState, AskId, AskRequested,
    AskResolved, Decision, Gate, JobEvent, JobView, McpAuth, McpProgress, McpServer, McpToolInfo,
    PolicyAction, RepoInfo,
};
use tools::{JobId, Key, PtyPage, PtyScreenReply, PtyViewport, TerminalSummary};
use uuid::Uuid;

use crate::broadcast::{Broadcast, BroadcastStage};
use crate::clip::Image;
use crate::dictation::{self, Dictating, Phase, Suggestion};
use crate::pathac::PathState;
use crate::search;
use crate::talk::Talking;


/// UI state.
#[derive(Debug, Default)]
pub struct Model {
    /// Current session (the one the human started, resumed or opened).
    pub session: Option<SessionId>,
    /// Peer name when the current session is a subagent (`Loaded.name`).
    pub session_name: Option<String>,
    /// Parent when the current session is a subagent (`Loaded.parent`): where `/parent` goes.
    pub session_parent: Option<SessionId>,
    /// The current session's subtree, seeded by `list_agents` on open and
    /// kept live by every event of a session in it (design doc 19 §3.3).
    pub agents: HashMap<SessionId, AgentRow>,
    /// `/agents` was typed; the next `UiEvent::Agents` opens the picker
    /// (seeding replies on open only fill the map).
    agents_picker_pending: bool,
    /// Multi-step `/provider add`; `None` when the input is a normal line.
    pub prompt: Option<Prompt>,
    /// A `Send { session: None }` is in flight; adopt the next `TurnStarted`.
    awaiting_session: bool,
    /// A routing failure of that send (`Error { turn: None }`) that arrived
    /// before `SessionCreated` named the session: shown once it does, if the
    /// ids agree; another session's error is dropped.
    send_error: Option<(SessionId, String)>,
    /// `/mcp` was typed; the next `UiEvent::Mcp` opens the menu. Refreshes
    /// after `SetGate` only update an open menu, never reopen a closed one.
    awaiting_mcp: bool,
    /// `/acp` was typed; the next `UiEvent::Acp` opens the menu (same rule as `awaiting_mcp`).
    awaiting_acp: bool,
    /// Transcript of the shown session.
    pub entries: Vec<Entry>,
    /// The current session has a turn in flight.
    pub streaming: bool,
    /// Animation frame; advances while something is in flight.
    pub frame: usize,
    /// Usage of the last finished turn of the shown session.
    pub usage: Option<Usage>,
    /// Model of the shown session (`TurnStarted.model`; before that the
    /// draft's pick, else `default_model`). For an ACP child this is the
    /// harness name (`omp`).
    pub model_id: String,
    /// The engine's default for new sessions (`Info.default_model`, then
    /// `DefaultModelChanged`, 82 §3): what the header shows after `/new`.
    /// Only the engine moves it (148 R1): a pick here never becomes the default.
    pub default_model: String,
    /// 148 R1: `/model` / `/effort` picked before the first send. The next
    /// session this view opens is created with these options (never the
    /// default); the header shows them meanwhile. Cleared once a session is adopted.
    pub draft: Option<CreateSessionOptions>,
    /// The engine's starred models (`Info.favorite_models`, then
    /// `FavoriteModelsChanged` / the `set_model_favorite` reply, 127 §3), in
    /// starring order; `/model` lists them first.
    pub favorite_models: Vec<String>,
    /// The LLM an ACP child's harness reports (06 §8.2); the header shows
    /// `model_id · harness_model`. `None` for native sessions.
    pub harness_model: Option<String>,
    /// The ACP harness's manifest of loaded skills (`skill:<name>`) and
    /// commands (75 §3); empty for native sessions.
    pub harness_commands: Vec<agent::HarnessCommand>,
    /// Context window the harness reported (75 §5); the header prefers it
    /// over the catalog limit. `None` for native sessions.
    pub context_size: Option<u32>,
    /// `Loaded.agent`: the shown session runs on an external harness (75 §8);
    /// `/model` and `/effort` then pick from the harness's own options.
    pub agent: Option<String>,
    /// 141 §10.1: the shown session is a watch-only harness subagent
    /// (`Loaded.observed`, kept live by `ObservedChildChanged`): no input,
    /// Esc stops it only when the harness accepts a per-child cancel.
    pub observed: Option<ObservedMeta>,
    /// 141 §10.1: `/` was typed in a watch-only view; the editor takes keys
    /// for that one slash command (the way to `/parent`, `/agents`, …) and
    /// lets go once the line no longer starts with `/`.
    pub watch_slash: bool,
    /// Reasoning effort of the shown session (`default_effort` before the first turn);
    /// `None` = provider default.
    pub effort: Option<String>,
    /// The engine's default for new sessions (`Info.default_effort`, then
    /// `DefaultEffortChanged`, 83 §3): what the header shows after `/new`.
    pub default_effort: Option<String>,
    /// A `SetEffort` is in flight; its `EffortChanged` echo is not announced twice.
    awaiting_effort: bool,
    /// Effort levels per model for the `/effort` picker; `None` in the line printer.
    pub catalog: Option<Arc<Catalog>>,
    /// Tokens the shown session's window held after its last turn.
    pub context: u32,
    /// `context` is the engine's estimate (after a compaction or a load), shown
    /// as `≈` until the provider's next usage replaces it (70 COMPACT-SIZE).
    pub context_estimated: bool,
    /// Open menu.
    pub menu: Option<Menu>,
    /// Path autocomplete under the editor (PATH-AC).
    pub path: PathState,
    /// Pending approvals for the current session or its subtree, oldest first; the front is shown.
    pub approvals: VecDeque<ApprovalRequested>,
    /// Pending `ask_user` calls for the current session or its subtree, oldest first; the front is shown.
    pub asks: VecDeque<Ask>,
    /// The double's prediction per pending card (`question_id` as text), from `DoublePredicted`;
    /// dropped when the card resolves.
    pub double: HashMap<String, DoublePredicted>,
    /// `tools.approvals` (global); false = never prompts.
    pub approvals_global: bool,
    /// The root session opted out of prompts (`/approvals off`).
    pub approvals_off: bool,
    /// `Info.app_enabled`: the app plugin runs, so `/apps` is offered. Off,
    /// app records stay readable under `/artifacts` (46 §9).
    pub app_enabled: bool,
    /// The harness-wide vision fallback (52 §3): a text-only shown model
    /// says at attach time whether this model will inspect the image.
    /// Asked once at startup; `/vision` replies keep it current.
    pub vision_model: Option<ModelInfo>,
    /// `/vision` was typed: the next reply prints its line (the startup
    /// fetch is silent).
    awaiting_vision: bool,
    /// Latest MCP reconcile snapshot; `None` before the first.
    pub mcp: Option<McpProgress>,
    /// A reconcile settled while the `/mcp` menu is open: the event loop
    /// re-lists so the rows show the servers that just (dis)connected.
    pub refresh_mcp: bool,
    /// Large pastes and attached images held out of the input box; the box
    /// shows one [`paste_token`] each, expanded back on submit.
    pub pastes: Vec<Paste>,
    /// Text of the last input submitted here, pastes expanded: the next
    /// `TurnStarted` echoing it is not shown again (its card is already up,
    /// with paste tokens).
    sent: Option<String>,
    /// Steers of the shown session waiting for the current tool batch to
    /// end, oldest first; drawn below the transcript until `InputInjected`.
    pub queued: Vec<QueuedInput>,
    /// Ids of the shown session's running background jobs (`JobEvent`).
    pub jobs: BTreeSet<String>,
    /// Turn the last `InputInjected` joined: its `TurnStarted` is not echoed
    /// (the card moved into `entries` on injection).
    last_injected_turn: Option<TurnId>,
    /// Text the editor takes on the next render: a withdrawn steer, or the
    /// opener `/edit` puts back.
    pub recall: Option<String>,
    /// Opener seq of the shown session's current or last turn (`TurnStarted.turn_seq`).
    pub turn_seq: Option<i64>,
    /// `/edit` in progress: `(entry index, turn seq)` of the message in the
    /// editor; the next plain line replaces that turn (29 §6.2).
    pub editing: Option<(usize, i64)>,
    /// Notice shown after the transcript reloads that an undo/redo triggered.
    notice: Option<String>,
    /// The shown session's summariser call is in flight (`Compacting`).
    pub compacting: bool,
    /// 109 §10.3: the status-line note after a restart (`resumed · 2
    /// replayed · 1 skipped`); cleared by the next `TurnFinished`.
    pub resume_note: Option<String>,
    /// 139 §9.3: the engine running the shown session's open turn when it is
    /// not this one (`Loaded.resume.owner` with `here = false`); the
    /// owner-offline line's `/take` then also resumes it.
    owner_turn: Option<String>,
    /// The shown session's harvest is in flight (`Harvesting` … `Harvested`).
    /// Detached from the turn (10 §5.2 as built): survives `TurnFinished`,
    /// shown only while nothing else is.
    pub harvesting: bool,
    /// The shown session's vision consultation step in flight
    /// (`VisionProgress`, 52 §6): the status-line text until it settles.
    pub vision: Option<String>,
    /// The user asked to leave.
    pub quit: bool,
    /// Skill index (`list_skills`), fetched at start and by `/skills`: names
    /// resolve `/<skill> [args]`.
    pub skills: Vec<SkillMeta>,
    /// `/skills` was typed: the next index reply opens the picker.
    skills_picker_pending: bool,
    /// Skills whose body is in the shown session's frame (`SkillsChanged`,
    /// `Loaded.skills`): the status-line badge.
    pub loaded_skills: Vec<String>,
    /// First visible transcript row while scrolled up: `(entry, rows into
    /// it)`. `None` follows the tail. Anchored to an entry rather than to a
    /// row count from the bottom so what you are reading stays put while a
    /// reply streams in below.
    pub anchor: Option<(usize, usize)>,
    /// Terminal columns, set by the fullscreen UI before events apply; `0`
    /// (tests, line printer) = unbounded. Assistant text renders at this
    /// width when constructed; resize does not re-render (design doc 30 §7).
    pub width: usize,
    /// Fixed feature registry; feature state never leaks into the core enum.
    pub(crate) components: crate::components::Registry,
    component_commands: Option<UnboundedSender<Command>>,
    /// Latest `list_nodes` (badge and `/nodes` menu); `None` until fetched.
    pub nodes: Option<Vec<nodes::Node>>,
    /// `/nodes` was typed (or a flow that reopens it ran); the next
    /// `UiEvent::Nodes` opens the menu.
    awaiting_nodes: bool,
    /// A node event arrived: the event loop re-asks `list_nodes`.
    pub refresh_nodes: bool,
    /// A loaded transcript holds `show` cards: the event loop asks
    /// `list_artifacts` to join their artifacts back (35 §0.3).
    pub refresh_artifacts: bool,
    /// The live `/nodes` invite card, redrawn each second by [`Model::tick`].
    invite: Option<PairingCard>,
    /// Latest `list_secrets` (the `/secrets` menu, the `replaces vN` hint); `None` until fetched.
    pub secrets: Option<secrets::SecretsList>,
    /// `/secrets` was typed; the next `UiEvent::Secrets` opens the menu.
    awaiting_secrets: bool,
    /// The vault changed while a list is held: the event loop re-asks `list_secrets`.
    pub refresh_secrets: bool,
    /// Pending `secret_request`s for the current session or its subtree, oldest first; the front is shown.
    pub secret_requests: VecDeque<SecretRequest>,
    /// Context revision of the shown transcript (`Loaded.context_revision`,
    /// 48 §3.1); empty until loaded. A `ContextChanged` with a newer one
    /// sets `refresh_context`; an older settled reload is dropped.
    pub context_revision: String,
    /// The shown session's context takes `/shake` (`Loaded.context_editable`).
    pub context_editable: bool,
    /// Why it does not, when it does not (`Loaded.context_reason`).
    pub context_reason: Option<String>,
    /// The transcript is behind a context edit or rewind and a settled
    /// reload was refused (the session was busy): retry at the next
    /// `ContextChanged { reason: "settled" }`.
    pub context_stale: bool,
    /// A context edit or rewind landed (here or in another client): the
    /// event loop re-asks `load_session(settled_only: true)`.
    pub refresh_context: bool,
    /// Status-line suffix while attached to another process's engine (63 §0.3).
    pub attached: Option<String>,
    /// The engine behind the calls changed: the event loop reloads the shown
    /// session and its subtree from the new one (63 §2.3).
    pub reselect: bool,
    /// Latest plan of the shown session (`Loaded.todos`, `TodoChanged`,
    /// `get_todos`; 47 §5.1). `None` until a load carried one.
    pub todos: Option<TodoSnapshot>,
    /// The plan on screen may be behind: the last load carried no snapshot.
    /// Labelled as such; never drawn as an empty plan (47 §5.4).
    pub todos_stale: bool,
    /// Disclosure choice per session, kept across switches within this
    /// client and across incoming snapshots (47 §6.1). Absent = collapsed.
    todo_open: HashMap<SessionId, bool>,
    /// The todo card has the keys (F2): ↑/↓/PgUp/PgDn scroll it, esc
    /// collapses, tab hands the editor back. Modals still win; see [`Model::todo_focused`].
    pub todo_focus: bool,
    /// First wrapped body row shown while expanded.
    pub todo_scroll: usize,
    /// Latest `get_communication` of the shown session (the global default
    /// without one): the header tag and the `/communication` lines (50 §10).
    /// `None` until fetched; never guessed.
    pub communication: Option<CommunicationState>,
    /// `/communication` (or a switch) was typed: the next state reply prints its lines.
    awaiting_communication: bool,
    /// The switches, a scope or the network changed: the event loop re-asks
    /// `get_communication` for the shown session.
    pub refresh_communication: bool,
    /// The `/peers` menu is open and its inputs changed: the event loop
    /// re-asks `list_peers` and `list_peer_messages`.
    pub refresh_peers: bool,
    /// Bodies `PeerDelivered` already carded: the legacy `MessageDelivered`
    /// for the same input (50 §10) is not shown twice.
    peer_shown: VecDeque<String>,
    /// `/search` state (49 §6.1): query, chips, request identity, groups,
    /// cursor, previews. Kept across session switches so chips survive and
    /// **Back to results** restores the last view; `menu == Menu::Search`
    /// says whether it is up.
    pub search: Box<search::State>,
    /// The **Earlier messages** inspection (49 §7): the transcript shows a
    /// page ending at an old match, isolated from the session's live events.
    pub inspect: Option<search::Inspect>,
    /// Search requests a reply produced where no sender was at hand (the
    /// refresh reply's re-ask); flushed by the next tick or key.
    search_outbox: Vec<search::Request>,
    /// `/cost` was typed; the next `UiEvent::Cost` opens the menu (same rule as `awaiting_mcp`).
    awaiting_cost: bool,
    /// Latest `session_cost` of the shown session (the header tag, `/cost`,
    /// the `/agents` child totals; 51 §8.3). `None` until fetched; never summed here.
    pub cost: Option<SessionCost>,
    /// The ledger of the shown session (or an ancestor) changed: the event
    /// loop re-asks `session_cost`.
    pub refresh_cost: bool,
    /// The shown session's advisor (125 §7.5): the status segment, the
    /// header tag, the picker's `●`. `state: Off` until a reply or event says otherwise.
    pub advisor: AdvisorStatus,
    /// `Info.advisor` (125 §6.5): the last remembered pick, served by `/advisor on`;
    /// follows every `AdvisorChanged` with a model.
    pub advisor_default: AdvisorDefaults,
    /// Where the next `UiEvent::Models` picker sends its pick.
    pick_target: PickTarget,
    /// What the next `UiEvent::AdvisorStatus` reply is for; `None` installs it silently.
    advisor_reply: Option<AdvisorReply>,
    /// Seqs of the Advisory rows `AdvisoryAdded` carded: a `TurnStarted`
    /// opened by one is not echoed as `◆ you`.
    advisory_seqs: BTreeSet<i64>,
    /// A transcript was loaded: the event loop re-asks `advisor_status`.
    pub refresh_advisor: bool,
    /// Effective project of the shown session's root (`Loaded.project`).
    pub project: Option<String>,
    /// A `/sessions` or `/artifacts` menu is open and session metadata
    /// changed: the event loop re-lists it.
    pub refresh_grouping: bool,
    /// The private root this view owns (51 §4): its id, lease secret and
    /// the tree members seen so far. Leaving the tree releases it.
    pub private: Option<Private>,
    /// The retention warning was shown once this run.
    private_warned: bool,
    /// Bumped whenever a terminal observer opens: replies carry the epoch
    /// they were asked under and older ones are dropped.
    terminal_epoch: u64,
    /// The dictation in progress (54 §6); the editor is read-only while `Some`.
    pub dictation: Option<Dictating>,
    /// `before`/`after` snapshot taken at Ctrl+D, waiting for `dictation_start`'s reply.
    dictation_pending: Option<(String, String)>,
    /// Streaming STT model state (`DictationModel` events); Ctrl+D needs `Ready`.
    pub dictation_model: voice::ModelState,
    /// `(value, caret)` the editor must take this frame: a dictation stopped or was discarded.
    pub pending_input: Option<(String, usize)>,
    /// The spliced value to submit this frame: a dictation ended on "send it" (153 §7.4).
    pub pending_send: Option<String>,
    /// The last dictation that landed in the editor, until the editor is sent or emptied
    /// (153 §6.1: its `dictation_correction` goes with the send).
    pub dictated: Option<voice::DictationId>,
    /// The dictionary suggestion shown under the composer (153 §6.4), until answered or 20 s.
    pub suggestion: Option<Suggestion>,
    /// The talk this view started (76 §0.12); `Some` whether or not the view is open.
    pub talk: Option<Talking>,
    /// The `/talk` pane covers the transcript; Esc closes it, the talk keeps running.
    pub talk_view: bool,
    /// `/talk` sent, waiting for `talk_start`'s reply.
    talk_pending: bool,
    /// A `/attach` pick this frame: the token the editor prepends to its text.
    pub attach_token: Option<String>,
    /// When Esc last cancelled (133 §5): three within [`ESC_TAPS_WINDOW`] stop the subtree too.
    esc_taps: Vec<std::time::Instant>,
}

mod advisor;
mod agents;
mod approvals;
mod artifacts;
mod boards;
mod broadcasts;
mod catalog;
mod concepts;
mod costs;
mod credentials;
mod components;
mod events;
mod harnesses;
mod history;
mod jobs;
mod lifecycle;
mod menus;
mod navigation;
mod network;
mod paste;
mod peers;
mod prompts;
mod sessions;
mod shake;
mod slash;
mod speech;
mod status;
mod submission;
mod terminal;
mod todos;
mod tooling;
mod transcript;
mod updates;

pub use advisor::{ADVISORY_COLLAPSED, ADVISOR_GLYPH, severity_name, advisory_severity, advisory_title, advisory_rows, advisory_row_count, advisory_lines, advisor_changed_line, advisor_state_name, advisor_status_lines};
pub(crate) use advisor::{AdvisorReply};
pub use agents::{AgentRow, sort_agents, agent_row, WATCH_GLYPH, source_tag, observed_word};
use agents::{rank, harness_word};
pub use approvals::{DoubleCmd, double_chip, Ask};
pub use artifacts::{Owner, artifact_rows, APP_WHERE, bytes_label, Progress, progress_line, scrape_line, artifact_size, artifact_row};
use artifacts::{SHOW, APP};
pub(crate) use artifacts::{artifact_read_path, artifact_text};
pub use catalog::{PickTarget, ModelRow, rank_models, providers, provider_models, title_model_line, vision_model_line, aws_profile_line};
#[cfg(test)]
use catalog::{search_terms};
pub use concepts::{meta_line, HISTORY_PAGE, revision_rows};
use concepts::{DIFF_STEP};
pub use costs::{money_units, cost_lines, cost_short};
pub use credentials::{LoginRow, SecretRequest};
use credentials::{bad_secret_name};
pub use events::{UiEvent, Command};
pub(crate) use components::Output;
pub(crate) use events::{send};
pub use harnesses::{AcpRow, harness_options_lines, harness_option_set_line};
use history::{revision, user_entries, cell_results, FENCE, from_history};
#[cfg(test)]
use history::{parent_view};
pub use jobs::{JOB_LINES, JobTail};
pub use menus::{window, Menu, Grouped, grouped_step, grouped_first, MenuKey};
pub(crate) use network::{PairingCard};
use network::{tick_card};
pub use network::{NodesArg, parse_nodes_arg, find_node, mesh_device_line};
pub use paste::{Paste, paste_token};
use peers::{on_off, short_uuid, peer_card, contact_row};
pub(crate) use peers::{clean, outside_card};
pub use peers::{communication_lines, a2a_lines, PeerRow};
pub use prompts::{Prompt};
pub use sessions::{Private, session_rows, TurnAction, ESC_TAPS_WINDOW, engine_chip, session_row, owner_offline_line, owner_turn_suffix, owner_unreachable, short, PRIVATE_WARNING};
use sessions::{project_heading, project_order};
pub use shake::{ShakeCategory, ShakeStage, Shake, ReviewRow, kind_name, expand, SHAKE_USAGE, removal_line};
pub use slash::{COMMANDS, slash_matches, SlashRow};
pub use speech::{voice_cmd, voice_line};
pub(crate) use status::{now_ms, when, ago};
pub use status::{Level, k};
pub(crate) use status::elapsed;
pub use terminal::{TERMINAL_POLL_MS, TERMINAL_INTERACTIVE_POLL_MS, TerminalView, terminal_row};
#[cfg(test)]
use terminal::{TERMINAL_PAGE};
pub use todos::{TODO_ROWS, TodoCard, status_label};
pub use tooling::{Row, Target, Switch, action_name, gate_name, git_url, mcp_entry};
pub use transcript::{Entry, CardKind, moved_title, injected, describe, output_markdown, result_line, Window, vision_line, note_title, attachment_label};
use transcript::{split_engine_restart, card, unescape, attr, attachment_labels, first_arg};

#[cfg(test)]
mod tests;
