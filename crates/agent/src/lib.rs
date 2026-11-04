//! Agent plugin: one supervisor actor (`"agent"`) routing to one actor per
//! session. Consumes tools through the `"tools"` registry; persists every
//! message to sqlite; publishes [`AgentEvent`] for any UI.
//!
//! - commands on the `"agent"` actor ([`Agents`]): [`SendInput`], [`Steer`],
//!   [`Withdraw`], [`ListQueued`], [`Undo`], [`Redo`], [`Cancel`], [`TrimAssistant`], [`Compact`],
//!   [`SetCompactStyle`], [`SetTitleModel`], [`GetTitleModel`], [`SetVisionModel`], [`GetVisionModel`], [`ListSessions`], [`LoadSession`], [`RenameSession`], [`ListAgents`],
//!   [`ListModels`], [`SetModel`], [`GetDefaultModel`], [`SetEffort`], [`GetDefaultEffort`], [`ShowPrompt`], [`ListSkills`],
//!   [`LoadSkill`], [`UseSkill`], [`AddSkill`], [`RemoveSkill`], [`AddSkillRepo`],
//!   [`RemoveSkillRepo`], [`SyncSkills`], [`ListLogins`], [`Login`], [`Logout`],
//!   [`ListProviders`], [`AddProvider`], [`RemoveProvider`], [`RefreshProviderModels`], [`NewSession`],
//!   [`ListAcp`], [`AddAcp`], [`RemoveAcp`], [`ListRegistry`], [`Ancestors`],
//!   [`ContextItems`], [`RemoveContext`], [`GetTodos`]; continuation/activity transitions in [`continuation`]
//! - communication policy and peer contracts (design doc 50) in [`communication`]:
//!   [`GetCommunication`], [`SetCommunication`], [`SetPeerScope`], [`ListPeers`],
//!   [`GetPeerMessage`], [`AnswerPeerRequest`]
//! - the `"user"` actor ([`UserChannels`]) and the `message_user` tool (design doc 24) in [`user`].
//! - vision fallback for a text-only main model (design doc 52) in [`vision`]:
//!   [`VisionNote`] on `HistoryEntry::User`, [`AgentEvent::VisionProgress`] live.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use runtime::{InputId, SessionId, TurnId};
use serde::{Deserialize, Serialize};

pub mod acp_child;
pub mod activity;
pub mod advisor;
pub mod ask;
pub mod attach;
mod aws;
mod backend;
pub mod clock;
pub mod communication;
pub mod compact;
pub mod context;
pub mod continuation;
pub mod dreaming_ledger;
pub mod durability;
pub mod endless;
pub mod endless_ext;
pub mod frame;
pub mod limits;
pub mod loop_;
pub mod notebook_context;
pub mod notebooks;
pub mod overview_audio;
pub mod overviews;
pub mod peers;
pub mod polish;
pub mod project;
pub mod providers;
pub mod repl;
pub mod search;
pub mod sensor;
pub mod session_read;
pub mod skills;
pub mod snap;
pub mod spill;
pub mod store;
pub mod supervisor;
pub mod title;
pub mod todo;
pub mod user;
pub mod vision;

/// Tests that start the process-global `tools` registry actor hold this for
/// their whole run: two test runtimes registering the same name race, and
/// the name is freed only when the first test's runtime drops the actor.
#[cfg(test)]
pub(crate) static REGISTRY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run a registry-starting test body: the lock is taken before the runtime
/// is built and released after it is dropped, so the actors the body
/// registered are dead — not merely about to be — when the next test's
/// `tools::registry::start` evicts them. A `#[tokio::test]` releases a guard
/// declared in its body *before* its runtime drops, which lost that race.
#[cfg(test)]
pub(crate) fn with_test_registry<F: Future>(body: F) -> F::Output {
    let _lock = REGISTRY_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // The knowledge actor a previous body registered died with its runtime
    // without its `on_stop` unregistration (`tools::registry::evict_dead`
    // covers only the registry's own names); the stale entry would refuse
    // the next `KnowledgePlugin::start`.
    if kameo::actor::ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
        .ok()
        .flatten()
        .is_some_and(|a| !a.is_alive())
    {
        kameo::registry::ACTOR_REGISTRY
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(knowledge::KNOWLEDGE_ACTOR);
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(body)
}

/// A test scratch directory under `$TMPDIR`, `theuth-<tag>-<uuid>`, removed
/// on drop — on a passing test and on a panic alike (141 R3 T15 / R4: leaked
/// `theuth-*` test dirs filled the shared machine's disk). Bind it before
/// anything that uses the path, so it drops last.
#[cfg(test)]
pub(crate) struct TestDir(pub(crate) std::path::PathBuf);

#[cfg(test)]
impl TestDir {
    pub(crate) fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("theuth-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 141 R3 T15: the guard deletes its dir on drop — also mid-panic, since
/// unwinding drops the test's locals.
#[cfg(test)]
#[test]
fn scratch_is_removed_on_drop() {
    let path = {
        let s = TestDir::new("scratch-check");
        std::fs::write(s.0.join("x"), b"x").unwrap();
        assert!(s.0.is_dir());
        s.0.clone()
    };
    assert!(!path.exists(), "{path:?}");
    let path = std::panic::catch_unwind(|| {
        let s = TestDir::new("scratch-panic");
        let p = s.0.clone();
        std::panic::panic_any(p);
    })
    .unwrap_err()
    .downcast::<std::path::PathBuf>()
    .unwrap();
    assert!(!path.exists(), "{path:?}");
}
pub mod cost;
pub mod costs;
pub mod lifecycle;
pub mod private;

pub use activity::{
    ActivityConfig, ChildStalled, PeerStateWord, PeerWaitRegistered, StallPhase, WaitPeer,
};
pub use aws::{AwsProfiles, ListAwsProfiles, SetAwsProfile};
pub use communication::{
    AnswerPeerRequest, ChannelChat, ChannelOrigin, ChannelVia, CommunicationChanged,
    CommunicationState, GetCommunication, GetPeerMessage, ListPeerMessages, ListPeers,
    PeerConsumed, PeerDeliver, PeerDelivered, PeerEnvelope, PeerInfo, PeerIntakeFailed,
    PeerMessageChanged, PeerMessagePage, PeerMessageState, PeerOrigin, PeerPage, PeerReceipt,
    PeerRecord, PeerScope, PeerTurnFinished, PeerUplink, SetCommunication, SetPeerScope,
};
pub use compact::CompactStyle;
pub use context::{
    ContextAttachment, ContextBreakdown, ContextEntry, ContextItem, ContextKind, ContextOwner,
    ContextRemoval, ContextSnapshot, ContextTarget,
};
pub use cost::{
    CallId, CallKind, CallStatus, ChildCost, CostBucket, CostObservation, CostScope, Coverage,
    MeterUpdate, Money, Provenance, SessionCost, SpendDay, SpendOwner, SpendOwnerKind, SpendReport,
    TokenTotals,
};
pub use costs::{
    CostBasis, CostCall, CostCallsCursor, CostCallsPage, CostCallsQuery, CostCells, CostCube,
    CostCubeChanged, CostCubeCursor, CostCubeMeta, CostModel, CostReason, CostRoot, CostRootEntry,
    CostService,
};
pub use dreaming_ledger::{
    Ledger, LedgerCount, LedgerHintUse, LedgerQuestion, LedgerRewind, LedgerSession, LedgerSince,
    LedgerStop, LedgerToolError, LedgerUserMessage,
};
pub use endless::{
    Backfill, Endless, EndlessConfig, EndlessProgress, EndlessStats, GuardTrips, MemoryMode,
    SessionMemory, SetMode, Stalled, UnitFilter, ViewLine, resolved_compactor_model,
};
pub use lifecycle::{
    Authorize, Authorized, CreateSession, LeaveSession, PrivateTrees, ResumePrivate, TreeOpened,
};
pub use lifecycle::{
    CleanupStatus, CloseAck, CloseResult, CloseTree, CreateSessionOptions, PrivateOwner,
    SessionOpen,
};
#[cfg(feature = "test-support")]
pub use supervisor::spawn_test_agents;
pub use supervisor::{
    ActivityOf, Agents, Broadcast, Identity, IdentitySource, InstallIdentitySource, ListActivity,
    RegisterNode, UnregisterNode,
};
pub use tools::Retention;
pub use user::{
    Deliver, MessageUserTool, Outbound, OutboundFile, RegisterUserSink, USER_ACTOR, UserChannels,
    UserSink,
};
pub use vision::{VisionNote, VisionObservation, VisionSource, VisionStatus};

/// Registry name of the [`Agents`] supervisor actor.
pub const AGENT_ACTOR: &str = "agent";

/// How a human input lands on a busy session (62 §0.3): `Steer` joins the
/// running turn after the current tool batch; `NextTurn` waits for the turn to
/// end. An idle session starts a turn now either way.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Inject after the current tool batch (today's behaviour).
    #[default]
    Steer,
    /// Hold until the running turn ends; opens the next one.
    NextTurn,
}

/// `ask` → `()` (145 §0.3–0.4). A live talk tells the agent whether its spoken
/// window is open; `None` when the talk ends. Only sessions that have received
/// `Some(..)` park anything or promote tool calls.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetTalk {
    /// The talk's session.
    pub session: SessionId,
    /// The live talk's window, or `None` when it ended.
    pub live: Option<TalkLive>,
}

/// What a live talk tells the agent (145 §2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TalkLive {
    /// Open: harness-made inputs may start a turn (after `TALK_COALESCE_MS`).
    /// Closed: they are parked.
    pub open: bool,
    /// `talk.promote_after_ms`; `None` never promotes.
    pub promote_after_ms: Option<u64>,
}

/// Items landing within this window join one turn.
pub const TALK_COALESCE_MS: u64 = 120;
/// A closed window releases what it parked after this long regardless.
pub const TALK_PARK_CAP_MS: u64 = 60_000;

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
    /// How it lands on a busy session (62 §0.3).
    #[serde(default)]
    pub delivery: Delivery,
    /// Client-minted idempotency key: a retry with the same id routes once
    /// and returns the same session (send never loses, never doubles).
    #[serde(default)]
    pub id: Option<InputId>,
    /// 172: the channel (Slack, Signal, Telegram) that delivered this input;
    /// stored with the user row. `None` for every other sender.
    #[serde(default)]
    pub channel: Option<ChannelOrigin>,
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
    /// How it lands on a busy session (62 §0.3).
    #[serde(default)]
    pub delivery: Delivery,
    /// 172: the channel that delivered this steer; see [`SendInput::channel`].
    #[serde(default)]
    pub channel: Option<ChannelOrigin>,
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
    /// 172: the channel that delivered this steer, if any (as on `InputQueued`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub channel: Option<ChannelOrigin>,
    /// 171F4: the files attached to the steer (as given to [`Steer`]), so a client that reloads while it waits still
    /// shows them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional))]
    pub files: Vec<PathBuf>,
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

/// `ask` → `Result<(), String>`: regenerate the reply to the turn opened at
/// `turn` — hide `seq >= turn`, then start a turn that replays the stored
/// opener verbatim (design doc 29 §4.3). `Err("turn in flight" | "not a turn start")`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Regenerate {
    /// Target session.
    pub session: SessionId,
    /// Seq of the user row that opens the turn (`HistoryEntry::User.turn`).
    pub turn: i64,
}

/// `ask` → `Result<(), String>`: replace the turn opened at `turn` with `text`
/// (+ `files`) — hide `seq >= turn`, then start a turn with the new input
/// (design doc 29 §4.3). `Err("turn in flight" | "not a turn start")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditTurn {
    /// Target session.
    pub session: SessionId,
    /// Seq of the user row that opens the turn (`HistoryEntry::User.turn`).
    pub turn: i64,
    /// New user text.
    pub text: String,
    /// Files to attach.
    pub files: Vec<PathBuf>,
}

/// `ask` → `Result<(), String>`: the reply of the turn opened at `turn` had
/// already finished when the user cut in (design doc 76 §7, the post-turn
/// case): replace the text of that turn's last assistant message with
/// `keep`, the plain text the user heard; empty `keep` deletes the message.
/// Tool messages stay. A `ContextChanged { reason: "trim" }` follows.
/// `Err("turn in flight" | "not a user row" | "no assistant row")`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrimAssistant {
    /// Target session.
    pub session: SessionId,
    /// Seq of the user row that opens the turn (`TurnStarted.turn_seq`).
    pub turn: i64,
    /// What the user heard of the reply.
    pub keep: String,
}

/// `ask` → `Result<SessionId, String>`: new session with a copy of the
/// conversation through the turn opened at `through` (all visible turns when
/// `None`); the source is only read (design doc 29 §4.3).
/// `Err("cannot fork an external harness session" | "nothing to fork" | "not a turn start")`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ForkSession {
    /// Source session.
    pub session: SessionId,
    /// Seq of the last turn opener to keep; `None` = everything visible.
    pub through: Option<i64>,
    /// Stop at this (0-based) assistant text of that turn instead of its end
    /// (design doc 29 §3); ignored without `through`.
    pub reply: Option<u32>,
}

/// How far a [`Cancel`] reaches (design doc 133 §2).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum CancelScope {
    /// The target session only; its running descendants keep working. The default.
    #[default]
    Root,
    /// The target and every running descendant (design doc 19 §2.3).
    Tree,
}

/// `ask` → `()`. Abort the in-flight turn of `session` — and, with
/// [`CancelScope::Tree`], of every running descendant (design doc 19 §2.3,
/// 133 §2); each emits `TurnFinished { stop_reason: "cancelled" }`.
///
/// `keep` (design doc 76 §7): plain text of the reply the user has already
/// received (a spoken prefix). When the target session's cancel lands inside a
/// model stream, the loop persists it as the round's assistant message and
/// that turn finishes with `stop_reason: "interrupted"`; nothing else is
/// added. `None` drops the streamed text as ever. Descendants never keep.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cancel {
    /// Target session.
    pub session: SessionId,
    /// What the user already received of the in-flight reply; `None` = drop it.
    pub keep: Option<String>,
    /// Root (default) or the whole running subtree (133 §2).
    #[serde(default)]
    pub scope: CancelScope,
}

/// Answer a pending `ask_user`: one answer per question, or
/// `None` to dismiss the set, which also cancels the owning session's turn.
/// `Ok(true)` if the ask was still pending. Every frontend answers through here.
/// 132 D6: answers for an ask no longer pending are not dropped — the
/// session gets them as a user message ([`LateAnswer`]); the result is still `false`.
pub async fn answer_ask(
    id: tools::AskId,
    answers: Option<Vec<tools::Answer>>,
) -> anyhow::Result<bool> {
    use kameo::actor::ActorRef;
    let ask = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)?
        .ok_or_else(|| anyhow::anyhow!("ask actor is not running"))?;
    let session = ask
        .ask(tools::AnswerAsk {
            id,
            answers: answers.clone(),
        })
        .await?;
    let agents = || {
        ActorRef::<Agents>::lookup(AGENT_ACTOR)?
            .ok_or_else(|| anyhow::anyhow!("agent actor is not running"))
    };
    match (session, answers) {
        (Some(session), None) => {
            agents()?
                .ask(Cancel {
                    session,
                    keep: None,
                    scope: CancelScope::Root,
                })
                .await?;
            Ok(true)
        }
        (Some(_), Some(_)) => Ok(true),
        (None, Some(answers)) if !answers.is_empty() => {
            // No agent actor (a test harness without one): nowhere to
            // deliver, still "not pending".
            match agents() {
                Ok(agents) => agents
                    .ask(LateAnswer { id, answers })
                    .await
                    .map_err(|e| anyhow::anyhow!("late answer: {e}"))?,
                Err(e) => tracing::warn!(ask = %id.0, "late answer dropped: {e}"),
            }
            Ok(false)
        }
        (None, _) => Ok(false),
    }
}

/// `ask` → `Result<(), String>`: 132 D6 — the answers to an ask that is no
/// longer pending, delivered to the asking session as a user message
/// (`Answer to your earlier question “<q>”: <a>`, one line per question),
/// the way a chat prompt lands. A question the journal does not know is `Ok(())`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LateAnswer {
    /// The ask.
    pub id: tools::AskId,
    /// One per question, in order.
    pub answers: Vec<tools::Answer>,
}

/// [`answer_ask`] for the owner's double (126 §4.3): the row is settled with
/// `answered_by = by`, and the set is never dismissed (a dismiss cancels the
/// turn). `Ok(true)` if the ask was still pending.
pub async fn answer_ask_by(
    id: tools::AskId,
    answers: Vec<tools::Answer>,
    by: tools::Answerer,
) -> anyhow::Result<bool> {
    use kameo::actor::ActorRef;
    let ask = ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)?
        .ok_or_else(|| anyhow::anyhow!("ask actor is not running"))?;
    Ok(ask
        .ask(tools::AnswerAskBy { id, answers, by })
        .await?
        .is_some())
}

/// `ask` → `Result<u32 /* epoch */, String>`: compact `session` now,
/// threshold ignored (design doc 07 §8.6). Refused while a turn is running.
/// `style: None` uses the configured default (design doc 11). `head: Some`
/// (meetings, 53 §0.7) is the summary body: no summariser and no pre-compaction
/// harvest call is made — the caller has done both — and the harness parts
/// (roster, todos, files, …) are assembled around it as usual.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Compact {
    /// Session to compact.
    pub session: SessionId,
    /// One-shot style; `None` = configured default.
    #[serde(default)]
    pub style: Option<CompactStyle>,
    /// A ready-made summary body instead of the summariser's.
    #[serde(default)]
    pub head: Option<String>,
}

/// `ask` → `Result<repl::ResetReport, String>`: forget `session`'s saved
/// python runtime (dump, inventory, lost-line sidecar; 131 D2). Refused
/// while a turn is running. The schedule runner asks this after the
/// `Compact { style: Clear }` of a fire whose spec says `runtime: clear`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ResetRuntime {
    /// Session whose runtime is reset.
    pub session: SessionId,
}

/// `ask` → `Result<(), String>`: persist `agent.compact_style` and update live sessions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetCompactStyle {
    /// New default style.
    pub style: CompactStyle,
}

/// A per-session money cap (136 §2.2): the routine's `budget:` key, or a
/// one-off `run_now` budget. The cap is per night (136 R-1 / D1): only
/// `model_calls` rows created at or after `since` count toward it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "SessionBudget")
)]
pub struct Budget {
    /// The cap, in nanos of `currency` (1 USD = 1_000_000_000).
    pub nanos: u64,
    /// ISO 4217 code, as `model_calls.returned_currency`.
    pub currency: String,
    /// RFC 3339 UTC instant the cap was set (stamped server-side by
    /// [`SetBudget`]); spend before it is not counted. Not on the wire.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub since: String,
}

/// `ask` → `Result<(), String>` (136 §2.2): set or clear a session's money
/// cap. The schedule plugin sends it on every fire (`None` when the file has
/// no `budget:`); `Reserved::open` refuses any model call of the session —
/// round, harvest, compaction or a detached side call — that would pass it;
/// a refused round call ends the turn with one `<budget>` note (136 W2), a
/// refused side call answers `budget exhausted: …`. The supervisor stamps
/// [`Budget::since`] = now and persists the cap on the session row, so a
/// durability-resumed turn runs under it (136 R-22).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetBudget {
    /// Target session.
    pub session: SessionId,
    /// The cap in nanos; `None` clears.
    pub nanos: Option<u64>,
    /// Currency of `nanos`.
    pub currency: String,
}

/// `ask` → `Result<(), String>`: one session's compaction override, in memory
/// only (`meetings/02-live.md` §0.2): `at_tokens` replaces `compact_at × window`
/// as the auto-compaction trigger, `hint` is appended as one paragraph to the
/// summariser instruction, `head` is a ready-made compaction head body. When
/// `head` is set, every compaction of the session that has no head of its own
/// (auto, and `Compact` without `head`) uses it as the given summary: Summary
/// style is forced — no summariser call, no snap frames, no pre-compaction
/// harvest — whatever `agent.compact_style` says (`/clear` still clears).
/// `None` clears each; nothing is persisted, so a restarted session compacts
/// like any other.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetCompaction {
    /// Target session.
    pub session: SessionId,
    /// Trigger in tokens; `None` = the configured `compact_at` fraction.
    #[serde(default)]
    pub at_tokens: Option<u32>,
    /// Extra summariser paragraph; `None` = the plain instruction.
    #[serde(default)]
    pub hint: Option<String>,
    /// Ready-made head body for every compaction without one of its own;
    /// `None` = the configured style runs.
    #[serde(default)]
    pub head: Option<String>,
}

/// A session's allowlisted toolset (`meetings/06-live-toolset.md`): only the
/// named registry tools (plus `todo` when listed) are in the direct array,
/// whatever their `DIRECT` flag; only the named plugins' prompt blocks are in
/// the system prompt; `runtime: false` drops `python` and its prompt block
/// even when the session has a pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toolset {
    /// Registry tool names (and `todo`) the model may call directly.
    pub tools: Vec<String>,
    /// `Plugin::name`s whose prompt blocks stay in the system prompt.
    pub prompts: Vec<String>,
    /// Whether `python` is offered at all.
    pub runtime: bool,
    /// Lines per `<knowledge>` note for this session (`meetings/06-live-toolset.md`
    /// §1.5, 81): `None` = the configured `knowledge.hint_limit`; `Some(0)` = no
    /// hints, no knowledge search at turn start.
    pub hint_limit: Option<u32>,
}

/// `ask` → `Result<(), String>`: one session's toolset override, in memory
/// only, live for a running turn from its next rebase. `None` = the ordinary
/// session: every tool, every block, python per `agent.runtime_only`.
#[derive(Clone, Debug)]
pub struct SetToolset {
    /// Target session.
    pub session: SessionId,
    /// The allowlist; `None` clears it.
    pub toolset: Option<Toolset>,
}

/// `ask` → `Result<(), String>`: pin one session's title (`meetings/06-live-toolset.md`
/// §1.7), in memory only: while pinned the titler never runs, however often
/// the session compacts or how much the inputs grow. Nothing is persisted, so
/// a restarted session is unpinned.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PinTitle {
    /// Target session.
    pub session: SessionId,
    /// `true` pins, `false` releases.
    pub pinned: bool,
}

/// `ask` → [`CompactStyle`]: the live default style.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetCompactStyle;

/// `ask` → `Result<(), String>`: persist the subagent runtime limit and use it for new turns.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetSubagentTimeout {
    /// Wall-clock limit in seconds.
    pub seconds: u64,
}

/// `ask` → `u64`: the live subagent runtime limit in seconds.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetSubagentTimeout;

/// `ask` → `Result<(), String>`: persist `agent.max_tool_rounds` and use it for new sessions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetMaxToolRounds {
    /// Tool-call rounds per turn; `0` is rejected.
    pub rounds: u32,
}

/// `ask` → `u32`: the live `agent.max_tool_rounds`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetMaxToolRounds;

/// `ask` → `Result<(), String>`: persist `agent.stream_idle_secs` and use it for new sessions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetStreamIdle {
    /// Seconds without a stream event before the call is abandoned; `0` is rejected.
    pub seconds: u64,
}

/// `ask` → `u64`: the live `agent.stream_idle_secs`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetStreamIdle;

/// `ask` → `Result<(), String>`: persist `agent.max_salvages` and use it for new sessions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetMaxSalvages {
    /// Interrupted-reply recoveries per turn; `0` turns salvage off.
    pub count: u32,
}

/// `ask` → `u32`: the live `agent.max_salvages`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetMaxSalvages;

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

/// `ask` → `Result<Option<ModelInfo>, String>`: persist `voice.polish_model` (153 §9.2)
/// and use it for the next polish side call; `None` clears it (title model, then session).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetPolishModel {
    /// `provider/model`, bare id, or `None`.
    pub model: Option<String>,
}

/// `ask` → `Option<ModelInfo>`: the live polish model, if set.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetPolishModel;

/// `ask` → `Result<PolishReply, String>`: polish a dictation transcript for a human reader
/// (153 §9) on `ModelRole::Polish`, charged to `session` when given.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Polish {
    /// The shaped transcript.
    pub text: String,
    /// Who reads it; `AgentPrompt` is refused (§9.1).
    pub destination: voice::Destination,
    /// The session to charge; `None` = the destination has none.
    pub session: Option<SessionId>,
}

/// `ask` → `Result<PolishReply, String>`: rewrite `selection` per `instruction` (153 §13.6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rewrite {
    /// The spoken instruction (the user message).
    pub instruction: String,
    /// The selected text (goes into the system prompt through `voice.rewrite_template`).
    pub selection: String,
    /// The app the selection lives in.
    pub app: voice::AppProfile,
    /// Earlier (instruction, result) pairs of the same selection.
    pub thread: Option<Vec<(String, String)>>,
}

/// Reply to [`Polish`] / [`Rewrite`] (153 §14.2). `text: None` = deliver the shaped text;
/// `refused` says why (no model, budget passed, `AgentPrompt`, provider error).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PolishReply {
    /// The polished text, when the call succeeded.
    pub text: Option<String>,
    /// The model that answered.
    pub model: Option<String>,
    /// Round trip in ms.
    pub ms: u32,
    /// Why nothing was polished.
    pub refused: Option<String>,
}

/// `ask` → `Result<Option<ModelInfo>, String>`: persist `agent.vision_model`
/// and use it for the next vision consultation (52 §3.1); `None` clears it
/// (no fallback). `Some` must name an exact catalog entry that accepts images
/// through its backend on a configured provider; the file is written before
/// the live value changes, so a failed write leaves the selection as it was.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetVisionModel {
    /// `provider/model`, a unique bare id, or `None`.
    pub model: Option<String>,
}

/// `ask` → `Option<ModelInfo>`: the live vision fallback model, if set.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetVisionModel;

/// `ask` → `Result<Option<ModelInfo>, String>`: persist `agent.escalate_model`
/// (57 §0.9, tier 2 of the decision oracle); `None` clears it (inherits the
/// title model, then the session's model). Same validation as the title model:
/// the id must resolve in the catalog; text capability is enough.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEscalateModel {
    /// `provider/model`, bare id, or `None`.
    pub model: Option<String>,
}

/// `ask` → [`EscalateModel`]: the escalation model and what it resolves to.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetEscalateModel;

/// The escalation slot as the UI shows it: what is set and what would answer.
#[derive(Clone, Debug, Serialize, Deserialize, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EscalateModel {
    /// `agent.escalate_model`, if set.
    pub model: Option<ModelInfo>,
    /// The model a tier-2 call uses today: `model`, else the title model;
    /// `None` = each session's own model.
    pub effective: Option<ModelInfo>,
}

// 57 §2 side-call types live in `runtime` (00 §10: `decision` sits below `agent`); re-exported
// so `agent::SideCall` etc. keep working.
pub use runtime::side_call::{ModelRole, SideCall, SideCallable, SideReply};

/// `ask` → `Result<Option<ModelInfo>, String>`: persist `agent.subagents.model`
/// and use it for every child spawned without an explicit `model`; the system
/// prompt names it. `None` clears it (children fall back to `agent.model`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetSubagentModel {
    /// `provider/model`, bare id, or `None`.
    pub model: Option<String>,
}

/// `ask` → `Option<ModelInfo>`: the live subagent default model, if set.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetSubagentModel;

/// `ask` → `Result<ModelInfo, String>` (148 R4): resolve `model` through the
/// catalog AND refuse a provider without credentials, exactly as
/// [`SetSubagentModel`] validates ("provider `x` has no credentials"). No
/// side effects: the roster validates `roster.theuth_model` and an agent
/// file's `model:` with it. An unknown id under a keyed provider still
/// resolves (pass-through), as everywhere else.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolveModel {
    /// `provider/model` or a bare model id.
    pub model: String,
}

/// `ask` → `Result<knowledge::HarvestResult, String>`: harvest a session's
/// finished frame (design doc 10 §5.2). `Err("turn in flight")` while a turn
/// runs, `Err("harvest in flight")` while the previous harvest still does; the
/// session is idle again as soon as the frame is snapshotted, the reply
/// follows the detached harvest.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Harvest {
    /// Session.
    pub session: SessionId,
}

/// `ask` → `Vec<SessionSummary>`, newest first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListSessions;

/// `ask` → the durable [`store::Store`] handle (68 §3): what the session
/// replication plugin writes `session_log`/`session_cursors` through and
/// applies remote changesets to. Never a private store.
#[derive(Clone, Copy, Debug, Default)]
pub struct DurableStore;

/// `ask` → whether a live actor was stopped. Stop the session's actor
/// without touching its rows (68 §4.2 release / §4.3 demote): the
/// transcript stays readable; the next input respawns an actor, whose
/// writes the store's owner guard then decides.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct UnloadSession {
    /// The session.
    pub session: SessionId,
}

/// `ask` → [`Loaded`] for rendering a resumed session. `before`/`limit` page
/// the rendered entries (design doc 25 §7): entries with `seq < before`, the
/// last `limit` of them; both `None` = everything. A page never splits a
/// group of entries sharing one seq (tool siblings, a divider): the whole
/// boundary group is included even past `limit`.
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
    /// Read only through the idle session actor (design doc 48 §5): a busy
    /// native session answers with `settled: false` and a `context_reason`
    /// instead of a snapshot that could race a live turn; clients reject it.
    #[serde(default)]
    pub settled_only: bool,
}

/// `ask` → `Result<SessionRead, String>`: the stored transcript of `session`
/// as the history projection, for the `session_read` tool (66 §6.1).
/// Authorised like `load_session` for `caller`'s tree: a session inside a
/// private root is readable only from a session of that same root;
/// archived sessions are readable.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ReadSession {
    /// Target session.
    pub session: SessionId,
    /// The session whose tool call is asking.
    pub caller: SessionId,
}

/// [`ReadSession`] reply: the row and the full projected history.
#[derive(Clone, Debug, Serialize, Deserialize, kameo::Reply)]
pub struct SessionRead {
    /// The session row, retention filled in.
    pub summary: SessionSummary,
    /// Every history entry, oldest first.
    pub history: Vec<HistoryEntry>,
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

/// `ask` → `Result<(), String>`: pin or unpin a session; pinned rows lead
/// [`ListSessions`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinSession {
    /// Target session.
    pub session: SessionId,
    /// `true` to pin, `false` to unpin.
    pub pinned: bool,
}

/// `ask` → `Result<(), String>`: mark a session as a roster agent's desk
/// (67 §6.2) — `agent` is the bare name, `None` clears. The roster's write when
/// it opens a desk; lists read it as [`SessionSummary::desk`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkDesk {
    /// Target session.
    pub session: SessionId,
    /// Bare agent name, or `None`.
    pub agent: Option<String>,
}

/// `ask` → `()`: the configured model of a desk session's agent (148 E2) — a
/// hired agent's file `model:`, or `roster.theuth_model` for theuth; `None`
/// when the agent has none. Kept in memory: a native child of the desk whose
/// spawn names no `model` starts on it before `agent.subagents.model` and
/// `agent.model`. The roster sends it whenever it opens a desk or the agent's
/// model changes; the entry goes with a put-away (`MarkDesk { agent: None }`)
/// or deleted desk.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetDeskModel {
    /// The desk session.
    pub session: SessionId,
    /// Configured model id, or `None` to follow the defaults.
    pub model: Option<String>,
}

/// `ask` → `Option<String>`: the model id stored on `session`'s row (148 R1
/// F4: the roster compares it with a desk's effective model before switching);
/// `None` for an unknown session.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SessionModel(pub SessionId);

/// `ask` → `Option<SessionId>`: the session marked as the bare-named agent's
/// desk on this engine (STALE-WAKE-DESKS: one desk per agent, reused across
/// episodes and restarts; `sessions_desk_owner` keeps it unique).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeskSession(pub String);

/// `ask` → `Result<(), String>`: archive or unarchive a root; archived trees
/// leave the sidebar but stay in [`ListSessions`] and search (51 §3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveSession {
    /// Target root session.
    pub session: SessionId,
    /// `true` to archive, `false` to unarchive.
    pub archived: bool,
}

/// `ask` → `Result<(), String>`: delete a session and every descendant — rows,
/// messages, epochs — and drop their actors; one [`AgentEvent::SessionDeleted`]
/// per removed session. `Err` while any of them has a turn in flight.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteSession {
    /// Root of the subtree to delete.
    pub session: SessionId,
}

/// `ask` → `Result<::search::SearchPage<search::SearchHit>, String>`: the
/// Sessions group of unified search (design doc 49 §4): fuzzy search (nucleo,
/// fzf syntax) over every session's title and its user/assistant message
/// text, the best hit per session, best score first. A cold or stale index
/// answers `indexing` and builds once off the mailbox; ask again. `Err` for
/// an over-long query; a blank query is an empty ready page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchSessionPage {
    /// Fuzzy pattern.
    pub query: String,
    /// Hits per page; `None` → the shared `DEFAULT_LIMIT` (10), clamped to `1..=50`.
    pub limit: Option<u32>,
}

/// `ask` → `()`. Drop the session search index, failed build included, so
/// the next [`SearchSessionPage`] rebuilds instead of repeating a cached
/// error (design doc 49 §4: explicit refresh). Nothing is built here.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SearchRefresh;

/// `ask` → `Result<ContextSnapshot, String>`: the active-context inventory of
/// `session` with its revision and editability (design doc 48 §3.2). An
/// external-harness session answers `editable: false`; an unknown id is `Err`
/// and creates nothing. No message bodies.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ContextItems {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<ContextBreakdown, String>` (70 COMPACT-SIZE): where the
/// session's frame tokens go — head images/text, tail, system — beside the
/// provider's last bill. From the live actor when idle, else from the store
/// (`system_tokens: 0`, no calibration). `Err("unknown session")`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SessionContextBreakdown {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<SessionMemory, String>` (137 §11.2): the session's memory
/// mode, view and compactor stats; `mode: epochs` with an empty view for an
/// epochs session.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SessionMemoryQuery {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<SessionMemory, String>` (137F `endless_retry`): reset the
/// backoff of every failed node, wake the pump, return the fresh state; an
/// epochs session's state comes back unchanged.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EndlessRetry {
    /// Target session.
    pub session: SessionId,
}

/// `ask` → `Result<Vec<ViewLine>, String>` (137 §9 `zoom`): the two lines
/// under `id+n`, or the whole unit when `n == 1`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EndlessZoom {
    /// Target session.
    pub session: SessionId,
    /// First message of the line.
    pub id: u64,
    /// Messages the line covers (a power of two).
    pub n: u64,
}

/// `ask` → `Result<Vec<ViewLine>, String>` (137 §9 `recall`): FTS over the
/// session's level-0 lines, raw units as fallback; `limit` default 8.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndlessRecall {
    /// Target session.
    pub session: SessionId,
    /// Words to search for.
    pub query: String,
    /// Lines to return; `None` = 8.
    pub limit: Option<u8>,
}

/// `ask` → `Result<SessionMemory, String>` (137 §8.6 `/endless on|off`):
/// idle sessions only (`Err("busy")`). `endless` starts the backfill; `epochs`
/// writes the view as the next epoch's head and stops the pump. `filter`
/// (R1-D6) is fixed at switch-on: `All` for chats and desks, `Meeting` for a
/// meeting session (transcript batches and topic/brief calls only).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetSessionMemory {
    /// Target session.
    pub session: SessionId,
    /// The mode to switch to.
    pub mode: MemoryMode,
    /// Which parts become units.
    #[serde(default)]
    pub filter: UnitFilter,
}

/// `ask` → `Result<Vec<ViewLine>, String>` (137 R1-D6): the session's view
/// re-folded to `bytes`, as lines — a meeting head's `## Earlier in this
/// meeting`. Works for a `UnitFilter::Meeting` session, whose turns never
/// run the fresh-turn frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RenderMemoryView {
    /// Target session.
    pub session: SessionId,
    /// Budget in bytes for the rendered lines.
    pub bytes: u32,
}

/// `ask` → `Result<SessionMemory, String>` (137 R1-D2b): the session's view
/// budget in bytes (16_000..=256_000); takes effect at the next turn boundary.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetSessionViewBytes {
    /// Target session.
    pub session: SessionId,
    /// View budget in bytes.
    pub bytes: u32,
}

/// `ask` → `Result<EndlessConfig, String>` (137 §11.3): the live `agent.endless` section (never `Err`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GetEndlessSettings;

/// `ask` → `Result<EndlessConfig, String>` (137 §11.3): validate, persist
/// every `agent.endless.*` key to config.yaml and apply it live — the
/// service's shared config (`view_bytes`/`slack` re-fold at the next render,
/// `settle_secs` per turn), `default_chat` for the next created session, and
/// [`EndlessConfigChanged`] on the bus for the roster (`default_desk`) and
/// the meeting plugin. The compactor model applies to sessions attached after
/// the change. Returns what was stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEndlessSettings {
    /// The whole section.
    pub config: EndlessConfig,
}

/// Bus event: `agent.endless` was changed through [`SetEndlessSettings`];
/// the roster and the meeting plugin refresh their copies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndlessConfigChanged(pub EndlessConfig);

/// `ask` → `Result<ContextRemoval, String>`: remove `targets` from the
/// session's effective context (design doc 48 §3.2) — masks beside the stored
/// rows, no model call, no transcript deletion. Refuses a stale
/// `expected_revision`, a busy session, an incomplete protocol group, and any
/// external-harness session. `Err("unknown session")` creates nothing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveContext {
    /// Target session.
    pub session: SessionId,
    /// Revision the client reviewed against ([`ContextSnapshot::revision`]).
    pub expected_revision: String,
    /// Concrete targets, companions included (server-computed closure must be present).
    pub targets: Vec<ContextTarget>,
}

/// `ask` → `Result<TodoSnapshot, String>`: the session's latest committed plan
/// (47 §5.1), read from the store so it never waits on a running turn.
/// `Err` for an unknown session or a failed read; never an empty stand-in.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GetTodos {
    /// Session.
    pub session: SessionId,
}

/// `ask` → `Result<Option<SessionCost>, String>`: the owner summary of
/// `session` from the accounting ledger (design doc 51 §6.5); `None` when the
/// session is not an accounting owner.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SessionCostQuery(pub SessionId);

/// `ask` → `Result<SpendReport, String>`: model-call spend of the durable
/// ledger over the last `days` local days (128 Rev 3; clamped to 1..=31).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SpendReportQuery(pub u32);

/// One entry of an external harness's command manifest
/// (`available_commands_update`, design doc 75 §3). omp announces loaded
/// skills as `skill:<name>` and extension commands as `/ponytail…`;
/// opencode strips the source, so skills and commands are indistinguishable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HarnessCommand {
    /// Name as announced (`skill:ponytail`, `/ponytail-review`, `init`).
    pub name: String,
    /// Harness-provided description.
    pub description: String,
    /// `input.hint`: what the command takes after its name, when it takes input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub hint: Option<String>,
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
    /// Model the external harness reports for its session (design doc 06
    /// §8.2); `None` for native sessions and a harness that reports none.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub harness_model: Option<String>,
    /// Commands the external harness announced (`available_commands_update`,
    /// design doc 75 §3): its loaded skills and slash commands. Whether the
    /// model used one is not knowable. Empty for native sessions.
    #[serde(default)]
    pub harness_commands: Vec<HarnessCommand>,
    /// Context window size the external harness reported (`usage_update.size`,
    /// 75 §5); `None` for native sessions and before the first report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub context_size: Option<u32>,
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
    /// Session revision the page was read at (design doc 48 §3.1); `""` when the read failed.
    #[serde(default)]
    pub context_revision: String,
    /// Context removal is admissible now: native session, idle, state present.
    #[serde(default)]
    pub context_editable: bool,
    /// Why not editable (`"turn in flight"`, external harness, …).
    #[serde(default)]
    pub context_reason: Option<String>,
    /// Estimated tokens the effective context holds; an estimate, not provider usage.
    #[serde(default)]
    pub context_estimate: Option<u32>,
    /// The snapshot was read while the session actor owned idle state
    /// ([`LoadSession::settled_only`]); `false` means a live turn could race it.
    #[serde(default)]
    pub settled: bool,
    /// The session's plan (47 §5.1), independent of paging; `None` when the
    /// read failed — unavailable, not empty.
    #[serde(default)]
    pub todos: Option<todo::TodoSnapshot>,
    /// Effective communication state of this session (50 §10); `None` when
    /// the session is unknown to the policy (no row).
    #[serde(default)]
    pub communication: Option<CommunicationState>,
    /// Effective project label (51 §3): the root's value, also on children.
    #[serde(default)]
    pub project: Option<String>,
    /// Storage class of the root (51 §4); durable unless created private.
    #[serde(default)]
    pub retention: tools::Retention,
    /// 109 §10.3: the open turn being resumed, if any; `None` once it closed.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub resume: Option<ResumeState>,
    /// Row kind (141 §8.1): `observed` = a watch-only harness subagent; the
    /// session view hides the composer without a second RPC. `chat` from an
    /// older harness.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub kind: SessionKind,
    /// Watch-only facts when `kind == observed` (141 §5.1); `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub observed: Option<ObservedMeta>,
}

/// 109 §10.3: [`Loaded.resume`] — the session has an open turn (interrupted
/// by a crash or suspended by a drain) that resume owns, or the last resumed
/// turn's report until the next turn starts; the banner's state across a
/// reload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResumeState {
    /// The open turn.
    pub turn: TurnId,
    /// Crash or drain.
    pub kind: durability::ResumeKind,
    /// Newest journal row's time.
    pub interrupted_at: chrono::DateTime<chrono::Utc>,
    /// true when the supervisor has already relaunched this turn
    /// (durability.resume = auto); false while it waits for `resume_turn`
    /// (ask mode). Old engines omit it: `false` keeps "resuming…".
    #[serde(default)]
    pub resumed: bool,
    /// The counts the resumed step settled with (`resume_reports`), once it
    /// did; `None` while the resume is still running or for old engines.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub report: Option<ResumeReport>,
    /// 139 §5.2 (D2): `sessions.owner` when non-empty — who runs the turn.
    /// A replica displays it and never says "resuming".
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub owner: Option<String>,
    /// 139 §5.2: the session is owned by the engine that answered
    /// (`owner = '' OR owner = me`). `resumed` is meaningful only when
    /// `here`. Old engines omit it: `true` renders as before.
    #[serde(default = "crate::default_true")]
    pub here: bool,
    /// 139 §6.3: why the turn was / will be resumed — the `resume_reports`
    /// row's, or the boot rule for an open turn. `None` from old engines.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cause: Option<durability::ResumeCause>,
}

fn default_true() -> bool {
    true
}

/// Wire default for `TurnResumed.cause` from a pre-139 engine (`EventRelay`).
fn default_cause() -> durability::ResumeCause {
    durability::ResumeCause::Crash
}

/// 109 §8.2: what a resumed turn's step settled with — the live
/// `ResumeReport` event's counts, persisted so a reload still shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResumeReport {
    /// `(call_id, name)` of calls run as a new attempt.
    pub replayed: Vec<(String, String)>,
    /// `(call_id, name)` of rows marked `interrupted`: effect unknown.
    pub interrupted: Vec<(String, String)>,
    /// Jobs that died with the engine (§5).
    pub lost_jobs: Vec<tools::JobId>,
    /// ACP children restored with `session/load` (§7.3).
    pub children_restored: u32,
    /// ACP children replaced by a fresh child with a handoff (§7.3).
    pub children_handed_off: u32,
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
    /// External harness (`agents.yaml` key); `None` = theuth.
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
    /// Effective project label (51 §3): the root's value.
    #[serde(default)]
    pub project: Option<String>,
    /// Storage class of the root (51 §4).
    #[serde(default)]
    pub retention: tools::Retention,
    /// Live facts (62 §3.2); `state` duplicates `activity.state`.
    #[serde(default)]
    pub activity: SessionActivity,
    /// Risk ceiling the parent set with `spawn_subagent(tools=…)` (12 §5);
    /// `None` = no cap. Known for children spawned in this process only.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cap: Option<tools::Risk>,
    /// Row kind (125 §6.2); an advisor is never listed, but the field is
    /// carried so clients can filter defensively. `chat` from an older harness.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub kind: SessionKind,
    /// Watch-only facts when `kind == observed` (141 §5.1): a subagent the
    /// harness runs inside itself; `agent` is `None` and nothing drives it.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub observed: Option<ObservedMeta>,
}

/// Live facts about one session, computed on `Agents` (62 §0.5, §3.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionActivity {
    /// As `agents()` computes it.
    pub state: AgentState,
    /// `max(last_input, last_stop, turn start)`; the store's `updated_at` when cold.
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    /// Start of the current turn; `None` when not running.
    pub turn_started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Running background jobs (non-PTY).
    pub jobs: u16,
    /// Running PTY jobs.
    pub ptys: u16,
    /// Descendants with a live actor.
    pub children: u16,
    /// Reserved: sessions blocked in `ask_user` (not in v1, always `None`).
    pub asks: Option<u16>,
    /// Has an actor now (vs stored/cold).
    pub live: bool,
    /// 130 E2: ms since epoch `state` entered `awaiting` / `approval`; `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
    pub waiting_since_ms: Option<u64>,
}

impl Default for SessionActivity {
    fn default() -> Self {
        Self {
            state: AgentState::Idle,
            last_active: None,
            turn_started_at: None,
            jobs: 0,
            ptys: 0,
            children: 0,
            asks: None,
            live: false,
            waiting_since_ms: None,
        }
    }
}

/// Which sessions a [`Broadcast`] reaches (62 §0.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BroadcastTargets {
    /// Roots whose state is `Running | Awaiting | Approval` or with `jobs + ptys > 0`.
    Running,
    /// Every root with a live actor.
    Roots,
    /// Explicit ids, roots or children; order kept, duplicates dropped.
    Sessions {
        /// The sessions.
        ids: Vec<SessionId>,
    },
}

/// One message to many sessions (62 §0.1); `ask` the supervisor with
/// [`Broadcast`] → `Result<BroadcastReceipt, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BroadcastRequest {
    /// Selector.
    pub targets: BroadcastTargets,
    /// User text; stored behind a `[broadcast <id> · <n> sessions]` marker line.
    pub text: String,
    /// Image files (17 §2); copied per recipient by path.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// How it lands on a busy recipient.
    #[serde(default)]
    pub delivery: Delivery,
    /// Resolve and report the rows without sending.
    #[serde(default)]
    pub dry_run: bool,
}

/// What happened to one broadcast recipient.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BroadcastOutcome {
    /// Was idle; a turn opened (`TurnStarted` follows).
    Started,
    /// Was busy; queued with the given delivery.
    Queued {
        /// The minted input id (`InputQueued`/`InputInjected` echo it).
        input_id: InputId,
        /// How it will land.
        delivery: Delivery,
    },
    /// Not sent: `"dry run"`, `"remote node"`, `"no actor and not a root"`,
    /// `"not visible from this device"`.
    Skipped {
        /// Why.
        reason: String,
    },
    /// Send attempted and failed, or `"unknown session"`.
    Failed {
        /// The error.
        error: String,
    },
}

/// One recipient of a broadcast.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BroadcastRow {
    /// Session id.
    pub session: SessionId,
    /// Title; empty when the caller may not see it.
    pub title: String,
    /// Owning session; `None` for a root.
    pub parent: Option<SessionId>,
    /// What was true at send (or what the picker showed).
    pub before: SessionActivity,
    /// Outcome.
    pub outcome: BroadcastOutcome,
}

/// Reply of [`Broadcast`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BroadcastReceipt {
    /// Broadcast id; the marker's short id is its first 4 hex chars.
    pub id: uuid::Uuid,
    /// When it was sent (or resolved, for a dry run).
    pub at: chrono::DateTime<chrono::Utc>,
    /// One row per resolved session, in selector order.
    pub rows: Vec<BroadcastRow>,
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

/// `ask` → `Result<ModelInfo, String>`. Resolves `model` (design doc 02 §1).
/// With `session`, switches that session for its next model call and nothing
/// else (148 R1: a pick inside a chat applies to that chat only, whatever
/// `manual` says). With no session and `manual`, the id becomes the default
/// for new sessions: `agent.model` in `<data_dir>/config.yaml` is rewritten
/// and [`AgentEvent::DefaultModelChanged`] follows (the Settings path). With
/// no session and `manual: false` it only resolves and reports the id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetModel {
    /// Session to switch (that session only); `None` targets the default (needs `manual`).
    pub session: Option<SessionId>,
    /// `provider/model`, or a bare model id resolved against configured providers.
    pub model: String,
    /// With no session: a person set the default in Settings.
    /// Defaults to `true` so older frontends keep their behaviour.
    #[serde(default = "yes")]
    pub manual: bool,
}

/// `ask` → `String`: the model new sessions start on (`agent.model` as last
/// set by a manual [`SetModel`]), the live value rather than the config file
/// read at startup (82 §3).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetDefaultModel;

/// `ask` → `Result<(), String>`: set (or clear with `None`) the reasoning
/// effort. Validated against the session's current model via the catalog
/// (unknown/passthrough models accept any genai keyword or integer budget).
/// Like [`SetModel`]: with `session`, switches that session from its next
/// turn and nothing else (148 R1); with no session and `manual` the value
/// becomes the default for new sessions: `agent.effort` in
/// `<data_dir>/config.yaml` is rewritten and
/// [`AgentEvent::DefaultEffortChanged`] follows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetEffort {
    /// Session to switch (that session only); `None` targets the default (needs `manual`).
    pub session: Option<SessionId>,
    /// Effort keyword or token budget; `None` clears.
    pub effort: Option<String>,
    /// With no session: a person set the default in Settings.
    /// Defaults to `true` so older frontends keep their behaviour.
    #[serde(default = "yes")]
    pub manual: bool,
}

/// `ask` → `Option<String>`: the effort new sessions start on (`agent.effort`
/// as last set by a manual [`SetEffort`]), the live value rather than the
/// config file read at startup (83 §3).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetDefaultEffort;

/// Severity of one advisor note (125 §4.8); ordered so a card's severity is
/// the max of its notes.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AdvisorSeverity {
    /// The answer to a consultation (125 R1); never emitted by `advise`,
    /// never steered, never posted. First: it ranks below `Nit`, so a
    /// worst-of pick never promotes a card to `Advice`.
    Advice,
    /// Batched into one card at the next step boundary.
    Nit,
    /// Steered into the running turn; preserved as a card after a terminal answer.
    Concern,
    /// Steered; opens a turn when the session is idle.
    Blocker,
}

/// One note the advisor emitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AdvisoryNote {
    /// Severity.
    pub severity: AdvisorSeverity,
    /// The note text, normalised by the emission guard.
    pub note: String,
}

/// What an Advisory row is (125 R1 D-R1.6): the watchdog's notes or the
/// answer to a `consult` call. Flattened into [`Advisory`] under the `kind`
/// tag; rows written before R1 have no `kind` key and read as `Review`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdvisoryKind {
    /// The watchdog's notes (every row written before R1).
    #[default]
    Review,
    /// The answer to a `consult` call; `notes` holds exactly one `Advice` note.
    Consultation {
        /// The question the agent asked.
        question: String,
    },
}

impl AdvisoryKind {
    /// Flattened deserialiser: a row without a `kind` key is a `Review`
    /// (serde ignores `default` on flattened fields, so the tolerance lives here).
    fn or_review<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        #[derive(Deserialize)]
        struct Raw {
            kind: Option<String>,
            question: Option<String>,
        }
        let raw = Raw::deserialize(d)?;
        match raw.kind.as_deref() {
            None | Some("review") => Ok(Self::Review),
            Some("consultation") => Ok(Self::Consultation {
                question: raw
                    .question
                    .ok_or_else(|| D::Error::missing_field("question"))?,
            }),
            Some(other) => Err(D::Error::unknown_variant(
                other,
                &["review", "consultation"],
            )),
        }
    }
}

/// One Advisory card: every note the advisor emitted for one step boundary
/// (nits batched), one steer (125 §6.2), or one consultation's answer (R1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Advisory {
    /// The advisor session (never shown to the model).
    pub advisor: SessionId,
    /// `AdvisorStep` count of the link when the notes were raised.
    pub step: u32,
    /// Opener seq of the advised turn under review; `None` when idle.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub turn: Option<i64>,
    /// Unix milliseconds when the card was raised.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: i64,
    /// At least one; the card's severity is the max.
    pub notes: Vec<AdvisoryNote>,
    /// Review or consultation; 0034 rows have no `kind` key -> `Review`.
    #[serde(default, flatten, deserialize_with = "AdvisoryKind::or_review")]
    pub kind: AdvisoryKind,
}

impl Advisory {
    /// 125 §9: the one line a channel posts in the bound thread — the
    /// card's highest-severity note, first line only — or `None` for a card
    /// of nits (110 §3 rule 3: model input is not posted).
    #[must_use]
    pub fn channel_line(&self) -> Option<String> {
        let top = self.notes.iter().max_by_key(|n| n.severity)?;
        let word = match top.severity {
            AdvisorSeverity::Advice | AdvisorSeverity::Nit => return None,
            AdvisorSeverity::Concern => "concern",
            AdvisorSeverity::Blocker => "blocker",
        };
        let line = top.note.lines().next().unwrap_or_default().trim();
        Some(format!("advisor · {word} — {line}"))
    }
}

/// Live state of one advisor link (125 §4.3).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AdvisorState {
    /// No advisor for this session.
    #[default]
    Off,
    /// Linked, waiting for the next step.
    Idle,
    /// The advisor session is running a review turn.
    Reviewing,
    /// Paused on a usage limit; the batch is retained.
    Paused,
    /// Halted after repeated failures or a permanent rejection.
    Halted,
}

/// Notes emitted so far, by severity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AdvisorNoteCounts {
    /// Nits.
    pub nit: u32,
    /// Concerns.
    pub concern: u32,
    /// Blockers.
    pub blocker: u32,
}

/// `consult` calls answered for one link (125 R1 D-R1.7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AdvisorConsultCounts {
    /// Consults in the advised session's current user turn (its own; subagents not folded in).
    pub turn: u32,
    /// Consults answered for this link since it was created: the root's and its subagents'.
    pub total: u32,
    /// The live `advisor.max_consults` (0 = tool absent).
    pub cap: u8,
}

/// Everything a UI shows about one session's advisor (125 §6.2): the pill,
/// the popover and `/advisor status`. `state: Off` with everything else
/// default when the session has none.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AdvisorStatus {
    /// The advised session.
    pub session: SessionId,
    /// The advisor session, when linked.
    pub advisor: Option<SessionId>,
    /// Live state.
    pub state: AdvisorState,
    /// `provider/model` of the advisor.
    pub model: Option<String>,
    /// Effort of the advisor.
    pub effort: Option<String>,
    /// Notes emitted so far.
    pub notes: AdvisorNoteCounts,
    /// Tokens of the advisor's context in use.
    pub context_used: u32,
    /// Its model's context window.
    pub context_limit: u32,
    /// Prompt tokens spent by the advisor.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tokens_in: u64,
    /// Completion tokens spent by the advisor.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tokens_out: u64,
    /// `session_cost(advisor).total`.
    pub cost: Option<cost::CostBucket>,
    /// `AdvisorStep`s seen by the link.
    pub step: u32,
    /// Deltas waiting while the advisor is busy (125 §4.6).
    pub backlog: u32,
    /// The most recent note.
    pub last_note: Option<AdvisoryNote>,
    /// Why paused/halted, first line.
    pub detail: Option<String>,
    /// `consult` counts (R1).
    #[serde(default)]
    pub consults: AdvisorConsultCounts,
    /// D-R1.5 pairing warning, set once at attach/re-pick when the advisor's
    /// catalog output price is below the advised session's model's; `None`
    /// when either price is unknown or the order is fine.
    #[serde(default)]
    pub warning: Option<String>,
}

/// One `session_advisors` row (125 §6.1): the durable link between an
/// advised session and its advisor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AdvisorLink {
    /// The advised session.
    pub session: SessionId,
    /// The advisor session.
    pub advisor: SessionId,
    /// `provider/model` of the advisor.
    pub model: String,
    /// Effort of the advisor.
    pub effort: Option<String>,
    /// Rendered-entry count already delivered to the advisor (125 §4.4).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cursor: i64,
    /// Persisted state.
    pub state: AdvisorState,
}

/// `advisor.sync_backlog` (125 §4.12): how many pending deltas the advised
/// loop waits for at turn end; `off` never waits.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SyncBacklog {
    /// Never wait.
    #[default]
    Off,
    /// Wait for one delta.
    #[serde(rename = "1")]
    One,
    /// Wait for up to three.
    #[serde(rename = "3")]
    Three,
    /// Wait for up to five.
    #[serde(rename = "5")]
    Five,
}

/// Live advisor defaults carried by `Info` beside `default_model` /
/// `default_effort` (125 §6.5): the remembered pick and the three tuning keys.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AdvisorDefaults {
    /// `advisor.model`: the last manual pick; pins the picker and serves `/advisor on`.
    pub model: Option<String>,
    /// `advisor.effort`: the same pick's effort.
    pub effort: Option<String>,
    /// `advisor.max_notes_per_update` (1–32, default 4).
    pub max_notes_per_update: u8,
    /// `advisor.immune_turns` (0–10, default 3).
    pub immune_turns: u8,
    /// `advisor.sync_backlog` (default `off`).
    pub sync_backlog: SyncBacklog,
    /// `advisor.max_consults` (0–32, default 6; 0 removes the tool).
    #[serde(default = "six")]
    pub max_consults: u8,
}

fn six() -> u8 {
    6
}

/// `ask` → `Result<AdvisorStatus, String>`: turn the advisor of `session` on
/// (`Some(model)`: create the hidden child or re-pick its model) or off
/// (`None`; the advisor row and its notes stay, 125 §4.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetAdvisor {
    /// The advised session.
    pub session: SessionId,
    /// `provider/model`; `None` = Off.
    pub model: Option<String>,
    /// Effort for the advisor; `None` = provider default.
    pub effort: Option<String>,
}

/// `ask` → `AdvisorStatus` of `session` (`state: Off` when it has none).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AdvisorStatusOf {
    /// The advised session.
    pub session: SessionId,
}

/// `ask` → `AdvisorDefaults`: persist the given `advisor.*` keys
/// (`write_config_value`) and return the live defaults (125 §6.5). Every
/// `None` leaves its key alone; `effort: Some(None)` clears it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SetAdvisorDefaults {
    /// `advisor.effort`.
    pub effort: Option<Option<String>>,
    /// `advisor.max_notes_per_update`.
    pub max_notes_per_update: Option<u8>,
    /// `advisor.immune_turns`.
    pub immune_turns: Option<u8>,
    /// `advisor.sync_backlog`.
    pub sync_backlog: Option<SyncBacklog>,
    /// `advisor.max_consults` (0–32).
    pub max_consults: Option<u8>,
}

/// `ask` → `AdvisorDefaults`: the live `advisor.*` defaults (the remembered
/// pick and the three tuning keys) `Info.advisor` carries (125 §6.5) — the
/// actor's copy, not the config file read at startup.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetAdvisorDefaults;

/// Loop message (`impl Message<_> for Session`, 125 §4.1 item 4): set the
/// session's "has an advisor" flag so its step hooks cost one atomic load.
#[derive(Clone, Copy, Debug)]
pub struct SetAdvisorOn(pub bool);

/// Loop message (125 §4.10): append an Advisory row passively — stored with
/// its metadata, shown to the model at the next batch boundary, never
/// opening a turn.
#[derive(Clone, Debug)]
pub struct AppendAdvisory {
    /// The typed card.
    pub advisory: Advisory,
    /// The model-facing text of the row.
    pub text: String,
    /// `false` only for consultations (R1): the row is also masked
    /// (`excluded.whole`), a card for the UIs and never model context.
    pub model_visible: bool,
}

/// `ask` → `Vec<String>`: the favorite model ids (`agent.favorite_models`,
/// 127 §1) in the order they were starred — the live list, not the config
/// file read at startup.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetFavoriteModels;

/// `ask` → `Result<InfoDefaults, String>`: every live default the RPC `info`
/// reads from the supervisor, in one ask (176 E3) instead of ten.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct GetInfoDefaults;

/// [`GetInfoDefaults`]'s reply: what the per-field `Get*` asks answer.
#[derive(Clone, Debug)]
pub struct InfoDefaults {
    /// [`GetDefaultModel`].
    pub default_model: String,
    /// [`GetDefaultEffort`].
    pub default_effort: Option<String>,
    /// [`GetFavoriteModels`].
    pub favorite_models: Vec<String>,
    /// [`GetCompactStyle`].
    pub compact_style: CompactStyle,
    /// [`GetSubagentTimeout`].
    pub subagent_timeout_secs: u64,
    /// [`GetMaxToolRounds`].
    pub max_tool_rounds: u32,
    /// [`GetStreamIdle`].
    pub stream_idle_secs: u64,
    /// [`GetMaxSalvages`].
    pub max_salvages: u32,
    /// [`GetCommunication`] with `session: None`.
    pub communication: CommunicationState,
    /// [`GetAdvisorDefaults`].
    pub advisor: AdvisorDefaults,
}

/// `ask` → `Result<Vec<String>, String>`: star or unstar a model (127 §1).
/// `model` is resolved through the catalog like [`SetModel`] (aliases
/// normalised, unknown ids rejected with the same error). A no-op when the
/// model is already in the requested state; otherwise `agent.favorite_models`
/// in `<data_dir>/config.yaml` is rewritten and
/// [`AgentEvent::FavoriteModelsChanged`] follows. The reply is the new list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetModelFavorite {
    /// `provider/model`, or a bare model id resolved against configured providers.
    pub model: String,
    /// `true` stars, `false` unstars.
    pub favorite: bool,
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

/// `ask` → `Vec<ProviderInfo>`: the built-in key providers (`opencode`, `openrouter`;
/// 85 §0.9) first, then the custom providers by name.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListProviders;

/// One provider row of the Connections view: a built-in or a custom provider.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderInfo {
    /// User-facing prefix (`rwx` → `rwx/model`).
    pub name: String,
    /// OpenAI-compatible base URL (the fixed upstream URL for a built-in).
    pub base_url: String,
    /// Models in the catalog under this prefix.
    pub models: u32,
    /// `opencode` / `openrouter`: keyed by a secret, not removable (85 §0.9).
    #[serde(default)]
    pub builtin: bool,
    /// A key exists (vault or env); custom providers always carry one.
    #[serde(default)]
    pub configured: bool,
    /// Refresh models is available (custom providers; opencode/openrouter with a key) (162 D6).
    #[serde(default)]
    pub refreshable: bool,
}

/// `ask` → `Result<ProviderInfo, String>`: persist a custom provider, fetch its models.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddProvider {
    /// `[a-z][a-z0-9_-]*`, not a built-in adapter.
    pub name: String,
    /// `https://host/v1`.
    pub base_url: String,
    /// Bearer key; stored in the secrets vault (37) under the provider's name.
    pub key: String,
}

/// `ask` → `Result<(), String>`: delete a custom provider's url, key, and catalog models.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveProvider {
    /// Custom provider name.
    pub name: String,
}

/// `ask` → `Result<RefreshedModels, String>`: list the provider's models again (162 D3/D4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshProviderModels {
    /// A custom provider, `opencode`, or `openrouter`.
    pub name: String,
}

/// Reply of [`RefreshProviderModels`]: the row after the refresh and the ids it added or removed.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RefreshedModels {
    /// The row after the refresh (`models` is the new catalog count).
    pub provider: ProviderInfo,
    /// Ids the listing added, sorted.
    pub added: Vec<String>,
    /// Ids no longer listed and removed (custom providers only), sorted.
    pub removed: Vec<String>,
}

impl RefreshedModels {
    /// The one-line notice frontends print (162 D7):
    /// `rwx: 14 models (+2, -1)`, or `rwx: 14 models, no change`.
    pub fn line(&self) -> String {
        let p = &self.provider;
        if self.added.is_empty() && self.removed.is_empty() {
            format!("{}: {} models, no change", p.name, p.models)
        } else {
            format!(
                "{}: {} models (+{}, -{})",
                p.name,
                p.models,
                self.added.len(),
                self.removed.len()
            )
        }
    }
}

/// `ask` → `Result<SessionId, String>`: create an empty root session, for
/// ACP `session/new` (design doc 06 §4.1); `cwd` is recorded on the row.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewSession {
    /// The client's working directory.
    pub cwd: Option<PathBuf>,
    /// Nobody reads this session directly (the scheduler's roots): its system
    /// prompt gets no visualization or interactive-screen instructions.
    #[serde(default)]
    pub unattended: bool,
}

/// `ask` → `Result<SessionId, String>`: a trusted creator (owner console,
/// scheduler, a channel that authenticated the owner) opens a root with an
/// explicit communication scope (50 §2.3). Deliberately not `Deserialize`:
/// it is never an RPC method or a public input flag; [`NewSession`] and a
/// root-creating [`SendInput`] stay `isolated`, and the owner promotes a
/// root with [`SetPeerScope`].
#[derive(Clone, Debug)]
pub struct NewRootSession {
    /// The client's working directory.
    pub cwd: Option<PathBuf>,
    /// Root scope declared by the trusted creator.
    pub scope: PeerScope,
}

/// `ask` → `Result<(), String>`: set or clear (`None`) the project label of
/// the root above `session` (design doc 51 §3). Normalized here: no control
/// characters, trimmed, 1–128 bytes. Publishes `SessionMetadataChanged`
/// for the root after the commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetSessionProject {
    /// Any session of the tree; the root is resolved.
    pub session: SessionId,
    /// New label, or `None` to clear.
    pub project: Option<String>,
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

/// What a `sessions` row is (125 §6.2): an ordinary chat, the hidden
/// `advisor` child that reviews its parent's transcript (ruling 1), a
/// watch-only `observed` subagent a harness runs inside itself (141 D2), or
/// an `ask` run across notebooks (144 §9). Stored as `sessions.kind`;
/// readers hide advisors unless asked.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// A conversation (default).
    #[default]
    Chat,
    /// The hidden advisor session of its parent (125 §4).
    Advisor,
    /// A watch-only child a harness runs inside itself (141 §5): `parent` =
    /// the ACP child (or an observed parent when nested), no `agent`,
    /// `acp_session` = the harness's own id. Never driven, never messaged.
    Observed,
    /// An "ask across notebooks" session (144 §9): a normal durable root
    /// without a notebook binding whose first turn is the planned,
    /// branched answer the engine wrote; the user keeps asking in it.
    /// Listed by `notebook_ask_list`, otherwise a chat.
    Ask,
}

cost::str_enum! {
    /// Which feed produced a watch-only child (141 §3 "Source"); stored as
    /// `sessions.observed_source`.
    ObservedSource {
        /// ACP RFD `subagent_update` + child-addressed `session/update`s (UI tag `ACP`).
        Rfd = "rfd",
        /// opencode 2.x `opencode/session/child_update` (UI tag `opencode`).
        OpencodeExt = "opencode_ext",
        /// opencode 1.x `task` tool-call lifecycle only (UI tag `opencode`).
        OpencodeTask = "opencode_task",
        /// omp `task` progress rows + on-disk transcript tail (UI tag `omp task`).
        OmpTask = "omp_task",
    }
}

cost::str_enum! {
    /// Lifecycle of a watch-only child as the harness reports it (141 §5.2);
    /// stored as `sessions.observed_state`. Any observation may move any
    /// state to any other; the restart sweep touches non-terminal rows only.
    ObservedState {
        /// Announced, not started.
        Pending = "pending",
        /// The harness is working on it.
        Running = "running",
        /// Waits on a human (permission/elicitation).
        RequiresAction = "requires_action",
        /// Turn ended; the harness may prompt it again (RFD).
        Idle = "idle",
        /// Finished successfully.
        Completed = "completed",
        /// Finished with an error.
        Failed = "failed",
        /// Stopped by a cancel.
        Cancelled = "cancelled",
        /// Lost contact / activity unobservable.
        Unknown = "unknown",
    }
}

impl ObservedState {
    /// `completed | failed | cancelled`: kept on a later `unknown` mark (141 §5.2).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// How a watch-only child's cost relates to its parent's (141 §7.6, D8); the
/// acp adapters choose it per frame, so the one type lives there.
pub use acp::CostRule;

/// Watch-only facts of an `observed` row (141 §5.1), carried on
/// [`AgentInfo`], [`SessionSummary`], [`Loaded`] and
/// [`AgentEvent::ObservedChildChanged`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ObservedMeta {
    /// Which feed produced the row.
    pub source: ObservedSource,
    /// The harness's own id for the child (`sessions.acp_session`).
    pub external: String,
    /// Lifecycle as the harness reports it.
    pub state: ObservedState,
    /// Stop reason or error text (`observed_stop`); `"lost contact"` from the engine.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stop: Option<String>,
    /// The harness accepts `session/cancel` for this child (RFD `capabilities.cancel`).
    pub can_cancel: bool,
    /// Roster card second line (`observed_description`).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub description: Option<String>,
    /// The spawning tool call id in the parent's transcript (`observed_tool_call`).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub tool_call: Option<String>,
    /// Cost rule applied (141 §7.6).
    pub cost_rule: CostRule,
    /// Source-specific live fields (`observed_detail`, 141 §6 mapping tables);
    /// `null` when none were stored.
    #[serde(default)]
    pub detail: serde_json::Value,
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
    /// Keywords the title model assigned beside the title (`[a-z0-9-]`, at
    /// most four); empty until the first model-written title.
    #[serde(default)]
    pub tags: Vec<String>,
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
    /// Model the external harness reports for its session (design doc 06 §8.2).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub harness_model: Option<String>,
    /// Context window size the external harness reported (75 §5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub context_size: Option<u32>,
    /// The roster agent whose desk this session is (67 §6.2); chat lists drop
    /// these rows, the Desks group shows them.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub desk: Option<String>,
    /// Kept at the top of the list.
    #[serde(default)]
    pub pinned: bool,
    /// Hidden from the sidebar (51 §3): the root's flag, shown on every row
    /// of its tree. Still listed and searchable.
    #[serde(default)]
    pub archived: bool,
    /// Effective project label (51 §3): stored on the root, shown on every
    /// row of its tree.
    #[serde(default)]
    pub project: Option<String>,
    /// 144 §8: the notebook this chat is bound to (root metadata like `project`); `None` = plain chat.
    #[serde(default)]
    pub notebook: Option<String>,
    /// Storage class of the root (51 §4); durable unless created private.
    #[serde(default)]
    pub retention: tools::Retention,
    /// A turn is in flight: the supervisor's live `TurnStarted`/`TurnFinished`
    /// view (what `AgentState::Running` means); `false` on a bare store row.
    #[serde(default)]
    pub running: bool,
    /// Engine that holds the live actor and writes the transcript (68 §2):
    /// its mesh `EngineId` as text; `""` on a lone engine that never joined a mesh.
    #[serde(default)]
    pub owner: String,
    /// Ownership epoch; bumps on every transfer (68 §4.2).
    #[serde(default = "one")]
    pub epoch: u32,
    /// The owner is this engine: writes are served locally, not forwarded.
    #[serde(default = "yes")]
    pub here: bool,
    /// Row kind (125 §6.2); `chat` from an older harness.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub kind: SessionKind,
    /// Memory mode (137 D1): `epochs` from an older harness.
    #[serde(default)]
    pub memory: MemoryMode,
    /// Watch-only facts when `kind == observed` (141 §5.1); `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub observed: Option<ObservedMeta>,
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

/// A project's icon and colour (51 §3 as-built): one row per styled project,
/// keyed by the label; both fields are allow-listed keys ([`project::ICONS`],
/// [`project::COLORS`]) that every client maps through its own theme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProjectStyle {
    /// The project label (`sessions.project`).
    pub name: String,
    /// Lucide icon name, `None` for the client's neutral default.
    pub icon: Option<String>,
    /// Palette key, `None` for the client's neutral default.
    pub color: Option<String>,
}

/// `ask` → `Vec<ProjectStyle>`: every styled project, by name.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ListProjectStyles;

/// `ask` → `Result<(), String>`: set a project's icon and colour (51 §3
/// as-built); both `None` drops the row. The label is normalized like
/// [`SetSessionProject`]; icon and colour must be allow-listed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetProjectStyle {
    /// Project label.
    pub name: String,
    /// Lucide icon name.
    pub icon: Option<String>,
    /// Palette key.
    pub color: Option<String>,
}

/// Rendered history for UIs (tool traffic collapsed to summaries). Every
/// variant carries `context`: the stored row's context-removal identity and
/// membership (design doc 48 §3.3) — the actual message (not its turn
/// opener) for user/assistant text, the backing response row for a tool
/// card, the epoch head for a divider. `None` for UI-only errors, an
/// unstored live card (a call still running), or when the read failed.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// User text.
    User {
        /// Text.
        text: String,
        /// Own seq when this row opens a turn (design doc 29 §4.3); `None`
        /// for steers and harness notes, which are not addressable.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        turn: Option<i64>,
        /// Context membership of the stored row; attachments are its binary parts.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
        /// Set when a peer exchange produced this row (design doc 50 §9):
        /// typed provenance for replay, the same shape [`PeerDelivered`]
        /// carries live. `None` for the owner's own input and harness notes.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        peer: Option<PeerOrigin>,
        /// Set when this row is a vision consultation note (design doc 52 §6):
        /// the typed record of which images were inspected and how it went.
        /// The body text is the note itself. `None` for every other user row.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        vision: Option<VisionNote>,
        /// 172: the channel that delivered this row (`messages.channel`);
        /// `None` for every other row, and for rows stored before 172.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        channel: Option<ChannelOrigin>,
    },
    /// Assistant text.
    Assistant {
        /// Text.
        text: String,
        /// Seq of the user row that opened the turn this reply belongs to
        /// (design doc 29 §4.3).
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        turn: Option<i64>,
        /// Context membership of the stored assistant row.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
    },
    /// The thinking the assistant did before the reply or tool calls that
    /// follow it (70 THINKING-TRACES): the stored row's `ReasoningContent`
    /// parts, so a reload shows the same collapsed block the live
    /// `ReasoningDelta` stream built.
    Reasoning {
        /// Text.
        text: String,
        /// Seq of the user row that opened the turn (design doc 29 §4.3).
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        turn: Option<i64>,
        /// Context membership of the assistant row that carries the thinking.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
        /// 91: ms the model thought; `None` for rows written before 91 and
        /// for harness (ACP) sessions.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional, type = "number | null"))]
        thinking_ms: Option<u64>,
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
        /// Context membership of the backing response row; attachments are
        /// this response's images only (siblings in the same row are on
        /// their own cards). `None` while the call is still running.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
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
        /// Context membership of the epoch head (`ContextTarget::Head`);
        /// attachments are snap images.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
    },
    /// A failure (provider, store, harness) the session surfaced as
    /// `AgentEvent::Error`, replayed in transcript order behind the message
    /// it followed. UI-only: never in the model frame, so nothing to remove.
    Error {
        /// Same text the live event carried.
        text: String,
        /// Always `None`: an error is not removable model context.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
    },
    /// 109 §8.2: the streamed partial of an open turn's model call, dropped
    /// when the resumed turn regenerates it. UI-only, never in the frame.
    Interrupted {
        /// Text streamed before the stop.
        text: String,
        /// When the partial was last checkpointed.
        at: chrono::DateTime<chrono::Utc>,
        /// Always `None`: a partial is not model context.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
    },
    /// 125 §5.4: an advisor's notes for one step boundary, stored as a
    /// user row with typed `messages.advisory` metadata (the vision
    /// pattern); never opens a turn. The row's text is the model-facing
    /// rendering, the card renders from `advisory`.
    Advisory {
        /// The typed card.
        advisory: Advisory,
        /// Context membership of the stored row.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        context: Option<ContextEntry>,
    },
}

impl HistoryEntry {
    /// The entry's context metadata, whichever variant.
    pub fn context(&self) -> Option<&ContextEntry> {
        match self {
            Self::User { context, .. }
            | Self::Assistant { context, .. }
            | Self::Reasoning { context, .. }
            | Self::Tool { context, .. }
            | Self::Compacted { context, .. }
            | Self::Error { context, .. }
            | Self::Interrupted { context, .. }
            | Self::Advisory { context, .. } => context.as_ref(),
        }
    }

    /// Mutable [`HistoryEntry::context`], for page-time attachment materialization.
    pub fn context_mut(&mut self) -> Option<&mut ContextEntry> {
        match self {
            Self::User { context, .. }
            | Self::Assistant { context, .. }
            | Self::Reasoning { context, .. }
            | Self::Tool { context, .. }
            | Self::Compacted { context, .. }
            | Self::Error { context, .. }
            | Self::Interrupted { context, .. }
            | Self::Advisory { context, .. } => context.as_mut(),
        }
    }
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
        /// Seq of the stored user row that opened the turn (design doc 29 §4.2).
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        turn_seq: i64,
        /// 172: the channel that delivered the opening user text, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        channel: Option<ChannelOrigin>,
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
    /// The thinking that `ReasoningDelta` streamed has ended (91): `ms` from
    /// the first reasoning chunk to the first non-reasoning event of the same
    /// model call. Published before the text/tool chunk that ended it.
    ReasoningDone {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Milliseconds spent thinking.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        ms: u64,
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
    /// Tool arguments still streaming: the cumulative partial JSON so far
    /// (genai delivers cumulative strings, 00 §11.4). Coalesced: at most one
    /// per 250 ms per call and only after ≥ 512 new bytes. Not persisted; UIs
    /// may jsonrepair it to name what is being written (design doc 35 §3.6).
    ToolCallProgress {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Provider call id (matches `ToolCallPending`).
        call_id: String,
        /// Cumulative argument JSON so far (may be truncated mid-token).
        args: Arc<str>,
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
    /// A [`Steer`] is waiting for the current tool batch to end (or, with
    /// `defer`, for the turn to end; 62 §0.3).
    InputQueued {
        /// Session.
        session: SessionId,
        /// The steer's id.
        id: InputId,
        /// User text.
        text: String,
        /// `true`: lands at the next turn, not after this tool batch.
        #[serde(default)]
        defer: bool,
        /// 172: the channel that delivered this steer, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        channel: Option<ChannelOrigin>,
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
    /// 145 §0.3: harness inputs parked behind a closed spoken window.
    InputsParked {
        /// Session.
        session: SessionId,
        /// Items parked right now.
        count: u32,
    },
    /// A subagent session was created for `parent`.
    SubagentSpawned {
        /// Owning session.
        parent: SessionId,
        /// New session.
        child: SessionId,
        /// Peer name, unique among the parent's children.
        name: String,
        /// `spawn_subagent(tools=…)` ceiling for the child and its descendants; `None` = no cap.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        cap: Option<tools::Risk>,
    },
    /// A watch-only child of `parent` was created or changed (141 §7.5): an
    /// upsert, also on every state/detail change (detail-only changes are
    /// throttled ≥ 250 ms per child). Not [`Self::SubagentSpawned`]: nothing
    /// here is drivable, so channels bind no thread/topic to it.
    ObservedChildChanged {
        /// The ACP child (or the observed parent when nested).
        parent: SessionId,
        /// The watch-only row.
        child: SessionId,
        /// Display name (RFD/opencode title, omp `progress[].id`).
        name: String,
        /// The child's own model when the source reports one.
        model: Option<String>,
        /// Source, external id, state, cost rule, detail.
        observed: ObservedMeta,
    },
    /// Text injected into `session` by the harness (subagent result, peer
    /// message, finished or quiet job), for transcripts.
    MessageDelivered {
        /// Receiving session.
        session: SessionId,
        /// Sender: a child's name, `"parent"`, or `"job j3"` for a
        /// `<job_result>` or a `<job_idle>` (61).
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
    /// The default model for new sessions changed (82 §3): a manual
    /// [`SetModel`] made `model` the new `agent.model`. Global, no session.
    DefaultModelChanged {
        /// Canonical model id.
        model: String,
    },
    /// The favorite model list changed (127 §1): a [`SetModelFavorite`]
    /// starred or unstarred a model. Global, no session; `models` is the
    /// whole new list in favorites order.
    FavoriteModelsChanged {
        /// Canonical model ids, in the order they were starred.
        models: Vec<String>,
    },
    /// An external harness (ACP child, design doc 06 §8.2) reported the LLM
    /// model its session runs on — at connect, or when it switched
    /// mid-session. `TurnStarted.model` stays the harness name.
    HarnessModelChanged {
        /// Session.
        session: SessionId,
        /// The harness's model id (`anthropic/claude-opus-4-8`).
        model: String,
    },
    /// An external harness announced its command manifest
    /// (`available_commands_update`, design doc 75 §3): loaded skills
    /// (`skill:<name>` on omp) and slash commands. Published only on change.
    HarnessCommandsChanged {
        /// Session.
        session: SessionId,
        /// The full manifest, replacing the previous one.
        commands: Vec<HarnessCommand>,
    },
    /// The session's reasoning effort was set or cleared (takes effect on its next turn).
    EffortChanged {
        /// Session.
        session: SessionId,
        /// Effort keyword or budget; `None` = provider default.
        effort: Option<String>,
    },
    /// The default effort for new sessions changed (83 §3): a manual
    /// [`SetEffort`] made `effort` the new `agent.effort`. Global, no session.
    DefaultEffortChanged {
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
        /// Estimated tokens of the whole next request (system, tools, head,
        /// kept tail), scaled by the session's calibration: what the gauge
        /// shows as `≈` until the provider's next usage (70 COMPACT-SIZE).
        after: u32,
        /// Messages folded into the summary.
        folded: u32,
        /// Style actually used (cut if the summariser fell back).
        style: CompactStyle,
    },
    /// A detached harvest of the finished frame started (design doc 10 §5.2 as
    /// built): it may follow `TurnFinished` and outlive the next `TurnStarted`;
    /// `Harvested` ends it either way.
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
        /// Tags in force after this change: the titler's set, or the
        /// unchanged stored set on a rename or placeholder.
        #[serde(default)]
        tags: Vec<String>,
    },
    /// The session's rows are gone ([`DeleteSession`]); lists drop it.
    SessionDeleted {
        /// Session.
        session: SessionId,
    },
    /// Ownership moved (68 §4.2): `owner` now holds the live actor and
    /// writes the transcript; `here` says whether that is this engine.
    SessionOwnerChanged {
        /// Session.
        session: SessionId,
        /// New owner (mesh `EngineId` as text).
        owner: String,
        /// New epoch.
        epoch: u32,
        /// `owner` is this engine.
        here: bool,
    },
    /// The session's effective context changed outside a normal turn
    /// (design doc 48 §3.2): a [`RemoveContext`] commit, a settled turn /
    /// compaction handback, or an undo/redo. Published after commit and
    /// installation in live state; carries no bodies. Clients reload the
    /// affected session's history at `revision`.
    ContextChanged {
        /// Session.
        session: SessionId,
        /// Committed session revision (decimal string).
        revision: String,
        /// `"context_edit"`, `"settled"`, `"rewind"` or `"trim"`.
        reason: String,
    },
    /// The session's canonical plan changed (47 §5.1): a native `todo`
    /// mutation committed with its tool result, an ACP plan update, a clear,
    /// or a rewind selecting another list. Full snapshots replace earlier
    /// state; apply only a higher `version`. No turn: a rewind changes the
    /// plan between turns.
    TodoChanged {
        /// Session.
        session: SessionId,
        /// The whole list with its projection version.
        snapshot: todo::TodoSnapshot,
    },
    /// A vision consultation step (design doc 52 §5.1): `Questioning` when
    /// the main model is asked to write the question (`model` = main id),
    /// `Inspecting` when the fallback call starts (`model` = fallback id),
    /// `Completed` / `Failed` once the note row committed, with its `seq`.
    /// `sources` are the images of this consultation, original coordinates.
    VisionProgress {
        /// Session.
        session: SessionId,
        /// Turn, when consulted mid-turn.
        turn: Option<TurnId>,
        /// Which step.
        stage: VisionStage,
        /// Canonical id of the model this step calls (or called).
        model: String,
        /// Images covered by this consultation.
        sources: Vec<VisionSource>,
        /// Seq of the committed note row; `Some` only for `Completed` / `Failed`.
        #[cfg_attr(feature = "ts", ts(type = "number | null"))]
        seq: Option<i64>,
    },
    /// The root's metadata changed (51 §3): `project` is the new effective
    /// value for the whole tree. Published after the commit.
    SessionMetadataChanged {
        /// Root session.
        session: SessionId,
        /// New project label; `None` = ungrouped.
        project: Option<String>,
    },
    /// The accounting ledger of `session` changed (design doc 51 §6.4);
    /// re-read `session_cost` when `revision` is newer than what is shown.
    ModelUsageChanged {
        /// Owner whose revision moved (the mutated owner or a live ancestor).
        session: SessionId,
        /// New opaque revision.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        revision: u64,
    },
    /// The window gauge moved mid-turn: one model round of `turn` settled and
    /// `context` is what `TurnFinished.context` will report if no round follows
    /// (prompt + completion of that round). Published on the same cadence as
    /// `ModelUsageChanged` so the context meter moves with the cost (51 §6.4).
    ContextUsed {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Prompt + completion tokens of the round that just settled.
        context: u32,
        /// Context window size when the source knows it: an external
        /// harness's `usage_update.size` (75 §5). Native rounds pass `None`
        /// and clients fall back to the model catalog.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        size: Option<u32>,
    },
    /// A private root tree is closing (51 §4.4): every member's data is
    /// being torn down under `generation`; owners fence late writes on it.
    /// `SessionDeleted` follows per member once cleanup ends.
    SessionClosing {
        /// Root session.
        session: SessionId,
        /// Lifecycle generation being torn down; writers holding an older
        /// one must drop their work.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        generation: u64,
    },
    /// 109 §8.2: an open turn is being resumed after a restart (crash) or a
    /// drain (`Suspended`); published before anything else happens.
    TurnResumed {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// Crash or drain.
        kind: durability::ResumeKind,
        /// Newest journal row's time.
        interrupted_at: chrono::DateTime<chrono::Utc>,
        /// When the resume began.
        resumed_at: chrono::DateTime<chrono::Utc>,
        /// An `assistant_partial` existed and was discarded.
        partial_dropped: bool,
        /// 139 §6.3: why (`crash | shutdown | update | transfer | takeover`).
        #[serde(default = "crate::default_cause")]
        cause: durability::ResumeCause,
        /// 139 §6.3: the previous owner for `transfer` / `takeover`.
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(optional))]
        from: Option<String>,
    },
    /// 139 §6.3 (D5): a system-initiated takeover found an open turn and did
    /// **not** resume it; the Attention actor raises `resume:<s>:<t>` with
    /// the takeover detail and the user presses Resume (`resume_turn`).
    /// Published by W2's `Take::run` step 9′; consumed by W1's attention.
    ResumePending {
        /// Session.
        session: SessionId,
        /// The open turn.
        turn: TurnId,
        /// The engine the session was taken from.
        from: String,
    },
    /// 109 §8.2: the resumed batch was reassembled; the banner's counts.
    ResumeReport {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// `(call_id, name)` of calls run as a new attempt.
        replayed: Vec<(String, String)>,
        /// `(call_id, name)` of rows marked `interrupted`: effect unknown.
        interrupted: Vec<(String, String)>,
        /// Jobs that died with the engine (§5).
        lost_jobs: Vec<tools::JobId>,
        /// ACP children restored with `session/load` (§7.3).
        children_restored: u32,
        /// ACP children replaced by a fresh child with a handoff (§7.3).
        children_handed_off: u32,
    },
    /// 109 §9: a drain could not finish the turn; a `suspended` step was written.
    TurnSuspended {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// `shutdown` | `update` | `parent_gone`.
        reason: String,
    },
    /// 109 §8.2: the streamed partial of the open model call was discarded;
    /// the re-issued call regenerates it.
    PartialDropped {
        /// Session.
        session: SessionId,
        /// Turn.
        turn: TurnId,
        /// The model call whose `assistant_partial` was dropped.
        call_id: String,
    },
    /// 125 §6.2: the advisor of `session` changed state, model, counters or cost.
    AdvisorChanged {
        /// The advised session.
        session: SessionId,
        /// Its advisor's full status.
        status: AdvisorStatus,
    },
    /// 125 §6.2: an Advisory row was appended to `session` (steer or aside);
    /// `seq` is its row.
    AdvisoryAdded {
        /// The advised session.
        session: SessionId,
        /// Seq of the stored user row carrying the advisory.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        seq: i64,
        /// The typed card.
        advisory: Advisory,
    },
}

impl AgentEvent {
    /// Session the event belongs to; `None` for global events.
    pub fn session(&self) -> Option<SessionId> {
        match self {
            Self::TurnStarted { session, .. }
            | Self::TextDelta { session, .. }
            | Self::ReasoningDelta { session, .. }
            | Self::ReasoningDone { session, .. }
            | Self::ToolCallPending { session, .. }
            | Self::ToolCallProgress { session, .. }
            | Self::ToolCallStarted { session, .. }
            | Self::ToolCallFinished { session, .. }
            | Self::TurnFinished { session, .. }
            | Self::InputQueued { session, .. }
            | Self::InputInjected { session, .. }
            | Self::InputWithdrawn { session, .. }
            | Self::InputsParked { session, .. }
            | Self::MessageDelivered { session, .. }
            | Self::ModelChanged { session, .. }
            | Self::HarnessCommandsChanged { session, .. }
            | Self::HarnessModelChanged { session, .. }
            | Self::EffortChanged { session, .. }
            | Self::Compacting { session, .. }
            | Self::Compacted { session, .. }
            | Self::Harvesting { session, .. }
            | Self::Harvested { session, .. }
            | Self::SkillsChanged { session, .. }
            | Self::TitleChanged { session, .. }
            | Self::SessionDeleted { session }
            | Self::SessionOwnerChanged { session, .. }
            | Self::ContextChanged { session, .. }
            | Self::TodoChanged { session, .. }
            | Self::VisionProgress { session, .. }
            | Self::SessionMetadataChanged { session, .. }
            | Self::Error { session, .. }
            | Self::ModelUsageChanged { session, .. }
            | Self::ContextUsed { session, .. }
            | Self::SessionClosing { session, .. }
            | Self::TurnResumed { session, .. }
            | Self::ResumePending { session, .. }
            | Self::ResumeReport { session, .. }
            | Self::TurnSuspended { session, .. }
            | Self::PartialDropped { session, .. }
            | Self::AdvisorChanged { session, .. }
            | Self::AdvisoryAdded { session, .. } => Some(*session),
            Self::SubagentSpawned { child, .. } | Self::ObservedChildChanged { child, .. } => {
                Some(*child)
            }
            Self::DefaultModelChanged { .. }
            | Self::DefaultEffortChanged { .. }
            | Self::FavoriteModelsChanged { .. } => None,
        }
    }
}

/// Step of a vision consultation, carried by [`AgentEvent::VisionProgress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum VisionStage {
    /// The main model is writing the question.
    Questioning,
    /// The fallback model is looking at the images.
    Inspecting,
    /// The note committed with at least one answered image.
    Completed,
    /// The note committed with every image failed (or no note could be built).
    Failed,
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
    /// Models starred in a picker (127 §1), `provider/model` ids in the order
    /// they were starred; every device attached to this engine shows them
    /// first. Written back as a one-line flow sequence by [`SetModelFavorite`].
    pub favorite_models: Vec<String>,
    /// Harness-wide Bedrock inference profile; unset uses the AWS default credential chain.
    pub aws_profile: Option<String>,
    /// Max tool-call rounds per turn before the loop stops itself.
    pub max_tool_rounds: u32,
    /// No stream event for this long is a dead socket, not a slow model (36 §2.3).
    pub stream_idle_secs: u64,
    /// Interrupted-reply recoveries per turn — a stalled stream or the output
    /// token cap — before the turn stops with an error (36 §2.4, §2.6).
    pub max_salvages: u32,
    /// Max characters of tool output sent back to the model.
    pub max_tool_output_chars: usize,
    /// With the runtime on, only `python` and the tools that talk to the user
    /// or to other agents are direct tool definitions; data-returning tools
    /// are runtime functions (08 §0.4). `false` makes every registered tool a
    /// direct definition as well - for models weak at code; MCP churn then
    /// rebases the prefix.
    pub runtime_only: bool,
    /// Harness-wide permission for model-to-model communication (design doc
    /// 50 §2.1): discovery, `send_message`, `spawn_subagent` and automatic
    /// child-result injection. Off is a ceiling a session cannot override;
    /// missing means off. `/communication on always` persists it.
    pub communication: bool,
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
    /// Model that answers the main model's questions about images it cannot
    /// see itself (52 §3); unset = no fallback, never the main model. Must be
    /// an exact catalog entry that accepts images through its backend.
    pub vision_model: Option<String>,
    /// Tier-2 model of the local decision oracle (57 §0.9): answers a typed
    /// question the oracle abstained on, in one cheap no-tools side call.
    /// Unset = `title_model`; both unset = the session's model.
    pub escalate_model: Option<String>,
    /// How the loop uses the knowledge plugin (12 §9.2); the store itself is `knowledge.enabled`.
    pub knowledge: KnowledgeUse,
    /// Human broadcast limits (62 §7).
    pub broadcast: BroadcastConfig,
    /// Post-compaction token budgets (tail, archive images, total).
    pub compaction: CompactionConfig,
    /// Request passthrough for the built-in OpenRouter provider (85 §0.6).
    pub openrouter: OpenRouterConfig,
    /// Endless memory mode defaults and compactor settings (137 §11.1).
    pub endless: EndlessConfig,
    /// 155 §3.5: the ACP child stall thresholds (D3).
    pub activity: ActivityConfig,
    /// The top-level `surface:` section (design doc 30 §12), filled by
    /// `supervisor::start` — not part of `agent:`.
    #[serde(skip)]
    pub surface: SurfaceConfig,
    /// The top-level `durability:` section (design doc 109 §10.1), filled by
    /// `supervisor::start` the way `surface` is — not part of `agent:`.
    #[serde(skip)]
    pub durability: durability::DurabilityConfig,
    /// The top-level `advisor:` section (125 §6.5), filled by
    /// `supervisor::start` the way `surface` is — not part of `agent:`.
    #[serde(skip)]
    pub advisor: AdvisorConfig,
    /// The top-level `acp:` section (176), filled by `supervisor::start`
    /// the way `surface` is — not part of `agent:`.
    #[serde(skip)]
    pub acp: AcpConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-sonnet-4-5".into(),
            effort: None,
            favorite_models: Vec::new(),
            aws_profile: None,
            max_tool_rounds: 32,
            stream_idle_secs: 120,
            max_salvages: 2,
            max_tool_output_chars: 32_000,
            runtime_only: true,
            communication: false,
            subagents: SubagentsConfig::default(),
            compact_at: 0.8,
            compact_style: CompactStyle::Summary,
            title_model: None,
            vision_model: None,
            escalate_model: None,
            knowledge: KnowledgeUse::default(),
            broadcast: BroadcastConfig::default(),
            compaction: CompactionConfig::default(),
            openrouter: OpenRouterConfig::default(),
            endless: EndlessConfig::default(),
            activity: ActivityConfig::default(),
            surface: SurfaceConfig::default(),
            durability: durability::DurabilityConfig::default(),
            advisor: AdvisorConfig::default(),
            acp: AcpConfig::default(),
        }
    }
}

/// `acp:` (176 A1): external ACP harness children.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AcpConfig {
    /// Seconds without a turn, an approval, a job or any ACP traffic after
    /// which an external harness child's process is stopped; the next input
    /// resumes it with `session/load`. 0 = never.
    pub idle_stop_secs: u64,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            idle_stop_secs: 1800,
        }
    }
}

/// `agent.broadcast`: human broadcast limits (62 §7).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BroadcastConfig {
    /// Most sessions one broadcast may reach; more is refused before any send.
    pub max_targets: usize,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self { max_targets: 32 }
    }
}

/// `agent.openrouter`: what theuth adds to an OpenRouter request (85 §0.6).
/// All optional; genai sets the default `HTTP-Referer` / `X-OpenRouter-Title`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    /// OpenRouter `provider` preferences (`order`, `allow_fallbacks`,
    /// `data_collection`, `sort`, `only`, `ignore`, …), sent verbatim.
    pub provider: Option<serde_json::Value>,
    /// `HTTP-Referer` header override.
    pub referer: Option<String>,
    /// `X-OpenRouter-Title` header override.
    pub title: Option<String>,
}

/// `agent.compaction`: CEILINGS on what a fold keeps (70 COMPACT-SIZE, 80
/// COMPACT-FLOOR). The effective budgets are shares of the headroom over the
/// compaction trigger (`limit − fixed`, where `fixed` is the system prompt and
/// tool definitions): tail ≤ 35 %, head ≤ 25 %, each then capped by the value
/// here (`compact::fold_budget`). Measured with `compact::est` scaled by the
/// session's provider calibration.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// Ceiling on the history replayed verbatim after the head; the effective
    /// budget is `min(this, 35 % of the headroom)`.
    pub tail_tokens: u32,
    /// Ceiling on archive images in a `snap` head (oldest lines drop first);
    /// the effective cap is `min(this, 25 % of the headroom)`, and a fold whose
    /// single frame is over the cap runs as `summary`.
    pub image_tokens: u32,
    /// Absolute ceiling on head plus tail after a fold, whatever the headroom.
    pub target_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            tail_tokens: 40_000,
            image_tokens: 30_000,
            target_tokens: 120_000,
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

/// `[surface]` config section (design doc 30 §12): Mermaid / json-render blocks in replies.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SurfaceConfig {
    /// `false`: no prompt block, no rendering hook; fences stay code blocks everywhere.
    pub enabled: bool,
    /// `false`: no `<surface_error>` round; frontends still render what parses.
    pub correct: bool,
    /// Correction rounds per turn before the model is told to move on (30 §5 as-built).
    pub correct_rounds: u8,
}

impl Default for SurfaceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            correct: true,
            correct_rounds: 3,
        }
    }
}

/// The top-level `advisor:` config section (125 §6.5): the remembered pick
/// and the three tuning keys. No `enabled` exists (ruling 2): an advisor
/// exists only for the sessions a user turned it on for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvisorConfig {
    /// Last manual pick (`provider/model`); pins the picker, serves `/advisor on`.
    pub model: Option<String>,
    /// The same pick's effort.
    pub effort: Option<String>,
    /// Non-blocker notes accepted per update (1–32).
    pub max_notes_per_update: u8,
    /// Agent steps after a steer during which a concern is an aside (0–10; 0 disables).
    pub immune_turns: u8,
    /// How many pending deltas the advised loop waits for at turn end.
    pub sync_backlog: SyncBacklog,
    /// `consult` calls per user turn of the asking session (0–32; 0 removes the tool).
    pub max_consults: u8,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            model: None,
            effort: None,
            max_notes_per_update: 4,
            immune_turns: 3,
            sync_backlog: SyncBacklog::Off,
            max_consults: 6,
        }
    }
}

impl AdvisorConfig {
    /// The wire form `Info.advisor` carries.
    pub fn defaults(&self) -> AdvisorDefaults {
        AdvisorDefaults {
            model: self.model.clone(),
            effort: self.effort.clone(),
            max_notes_per_update: self.max_notes_per_update,
            immune_turns: self.immune_turns,
            sync_backlog: self.sync_backlog,
            max_consults: self.max_consults,
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
    /// Concurrent native child spawn turns per orchestrator tree (children
    /// this engine's loop executes); further spawns queue and the parent
    /// is told (01 §2.6).
    pub max_concurrent: usize,
    /// Concurrent external child spawn turns per orchestrator tree (ACP
    /// harness and mesh-node children) — a separate pool, so a long
    /// harness turn never starves a native spawn.
    pub max_concurrent_external: usize,
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
            max_concurrent: 16,
            max_concurrent_external: 32,
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
        // 144 §13.2 (E1): `notebook_context_set` / `notebook_context_estimate` ride the agent's store.
        notebook_context::register(rt);
        supervisor::start(rt).await
    }
}

/// 125 R1.9 (C): the wire shapes of the consultation types.
#[cfg(test)]
mod advisor_wire_tests {
    use super::*;

    fn card(kind: AdvisoryKind, severity: AdvisorSeverity) -> Advisory {
        Advisory {
            advisor: SessionId::new(),
            step: 1,
            turn: None,
            at_ms: 0,
            notes: vec![AdvisoryNote {
                severity,
                note: "answer\nmore".into(),
            }],
            kind,
        }
    }

    #[test]
    fn advisory_without_kind_is_a_review_and_consultation_round_trips() {
        let review = card(AdvisoryKind::Review, AdvisorSeverity::Nit);
        let mut json = serde_json::to_value(&review).unwrap();
        assert_eq!(json["kind"], "review");
        json.as_object_mut().unwrap().remove("kind");
        assert_eq!(serde_json::from_value::<Advisory>(json).unwrap(), review);

        let consultation = card(
            AdvisoryKind::Consultation {
                question: "which first?".into(),
            },
            AdvisorSeverity::Advice,
        );
        let json = serde_json::to_value(&consultation).unwrap();
        assert_eq!(json["kind"], "consultation");
        assert_eq!(json["question"], "which first?");
        assert_eq!(
            serde_json::from_value::<Advisory>(json).unwrap(),
            consultation
        );
        assert!(
            serde_json::from_str::<Advisory>(
                r#"{"advisor":"00000000-0000-0000-0000-000000000000","step":1,"turn":null,"at_ms":0,"notes":[],"kind":"consultation"}"#
            )
            .is_err(),
            "a consultation needs its question"
        );
    }

    #[test]
    fn advisor_status_and_defaults_parse_without_the_r1_fields() {
        let s: AdvisorStatus = serde_json::from_str(
            r#"{"session":"00000000-0000-0000-0000-000000000000","advisor":null,"state":"off","model":null,"effort":null,"notes":{"nit":0,"concern":0,"blocker":0},"context_used":0,"context_limit":0,"tokens_in":0,"tokens_out":0,"cost":null,"step":0,"backlog":0,"last_note":null,"detail":null}"#,
        )
        .unwrap();
        assert_eq!(s.consults, AdvisorConsultCounts::default());
        assert_eq!(s.warning, None);
        let d: AdvisorDefaults = serde_json::from_str(
            r#"{"model":null,"effort":null,"max_notes_per_update":4,"immune_turns":3,"sync_backlog":"off"}"#,
        )
        .unwrap();
        assert_eq!(d.max_consults, 6);
        assert_eq!(AdvisorConfig::default().defaults().max_consults, 6);
    }

    #[test]
    fn advice_ranks_lowest_and_is_never_posted() {
        assert!(AdvisorSeverity::Advice < AdvisorSeverity::Nit);
        assert!(AdvisorSeverity::Nit < AdvisorSeverity::Concern);
        assert!(AdvisorSeverity::Concern < AdvisorSeverity::Blocker);
        assert_eq!(
            serde_json::to_value(AdvisorSeverity::Advice).unwrap(),
            "advice"
        );
        assert_eq!(
            card(AdvisoryKind::Review, AdvisorSeverity::Advice).channel_line(),
            None
        );
        assert_eq!(
            card(AdvisoryKind::Review, AdvisorSeverity::Concern)
                .channel_line()
                .as_deref(),
            Some("advisor · concern — answer")
        );
    }
}
