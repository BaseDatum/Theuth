//! Wire, RPC, and storage data models for Decks (design doc 229).

use std::collections::BTreeMap;

use document::deck::{DeckDocSnapshot, DeckSection, SectionKind, SectionRevision};
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::{ApprovalId, Risk};

/// Upper bounds for script proposals, scheduling, and interactive outputs (229 H4).
pub const MAX_SCRIPT_CODE_BYTES: usize = 256 * 1024;
pub const MAX_DECLARED_ACTIONS: usize = 32;
pub const MAX_ACTION_NAME_BYTES: usize = 64;
pub const MAX_CHANGE_SUMMARY_BYTES: usize = 1024;
pub const MIN_INTERVAL_SECS: u64 = 1;
pub const MAX_INTERVAL_SECS: u64 = 86_400 * 30;
pub const MAX_ACTION_RESULT_BYTES: usize = 256 * 1024;

/// Clamp a script's polling/step interval into the safe bounded range `[1s, 30d]`.
pub fn clamp_interval_secs(secs: u64) -> u64 {
    secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS)
}

/// Recursively sort JSON object keys so hashing/canonicalization is key-order independent.
pub fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&str, Value> = map
                .iter()
                .map(|(k, v)| (k.as_str(), canonicalize_json(v)))
                .collect();
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k.to_owned(), v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

/// Serialize `value` with recursively sorted object keys for deterministic digests.
pub fn canonical_json_string(value: &Value) -> String {
    serde_json::to_string(&canonicalize_json(value)).unwrap_or_else(|_| "null".to_owned())
}

/// Execution mode of a Deck script revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptMode {
    /// One-shot scheduled or on-demand collection (`on_step` or `collect`).
    Oneshot,
    /// Persistent streaming (`on_start` + cooperative `on_step` loop keeping checkout alive).
    Streaming,
    /// Both scheduled collection / streaming and interactive declared actions (`on_action`).
    Hybrid,
}

impl ScriptMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oneshot => "oneshot",
            Self::Streaming => "streaming",
            Self::Hybrid => "hybrid",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "oneshot" => Self::Oneshot,
            "streaming" => Self::Streaming,
            _ => Self::Hybrid,
        }
    }
}

/// Review gate status of a Deck script revision.
/// Visual CRDT edits can never transition a revision to `Approved`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptStatus {
    PendingReview,
    Approved,
    Rejected,
    Superseded,
}

impl ScriptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingReview => "pending_review",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "superseded" => Self::Superseded,
            _ => Self::PendingReview,
        }
    }
}

/// A declared interactive action accepted by a Deck script.
/// Only declared actions whose `args` validate against `schema` can enter the Monty checkout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeclaredAction {
    /// Unique action identifier within the Deck script (e.g. `"triage_incident"`).
    pub name: String,
    /// Short user-facing title.
    pub title: String,
    /// Description of what the action does.
    #[serde(default)]
    pub description: String,
    /// JSON Schema object validating the action's `args` payload.
    pub schema: Value,
    /// Whether this action mutates persistent Monty state or external systems.
    #[serde(default = "default_true")]
    pub mutates_state: bool,
    /// Whether this action may run while the stream is `Paused` (checkout retained).
    #[serde(default = "default_true")]
    pub allow_while_paused: bool,
}

fn default_true() -> bool {
    true
}

/// One immutable revision of a Deck's executable Monty script.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptRevision {
    pub deck_id: String,
    pub rev: u64,
    pub code: String,
    pub code_sha256: String,
    /// Canonical SHA-256 digest over `(code, declared_actions, mode, interval_secs, schedule_cron)`.
    /// Bound into `deck_script_approve(deck_id, rev, expected_digest)` to prevent mid-review swap.
    pub revision_sha256: String,
    pub declared_actions: Vec<DeclaredAction>,
    pub mode: ScriptMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_cron: Option<String>,
    pub status: ScriptStatus,
    pub proposed_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_reason: Option<String>,
    pub change_summary: String,
    pub created_at: String,
}

/// Durable last-good snapshot persisted separately from transient live frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckSnapshot {
    pub snapshot_id: String,
    pub deck_id: String,
    pub script_rev: u64,
    pub run_epoch: u64,
    pub seq: u64,
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub created_at: String,
}

/// Transient in-memory live frame emitted via `deck_emit(...)`.
/// Never persisted to whiteboard Y.Doc or Deck CRDT Y.Doc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LiveFrame {
    pub deck_id: String,
    pub script_rev: u64,
    pub run_epoch: u64,
    pub seq: u64,
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub emitted_at: String,
}

/// Pending tool approval surfaced when a Deck's Monty checkout invokes a gated tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PendingApproval {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    pub tool: String,
    pub risk: Risk,
    pub args: Value,
}

/// Lifecycle state of a Deck's single runtime owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Stopped,
    Starting,
    Streaming,
    Paused,
    ScheduledIdle,
    WaitingApproval,
    Error,
}

/// Observable runtime status for a Deck.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckRuntimeStatus {
    pub deck_id: String,
    pub session_id: SessionId,
    pub state: RuntimeState,
    pub run_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_rev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rev: Option<u64>,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<String>,
    pub consecutive_failures: u32,
    pub step_count: u64,
    pub checkout_alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval: Option<PendingApproval>,
}

/// Lifecycle control operations on a Deck runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeControlOp {
    /// Start persistent streaming or scheduled collection (approved revision only).
    Start,
    /// Pause stepping while retaining the live Monty checkout and globals.
    Pause,
    /// Resume a paused checkout immediately.
    Resume,
    /// Checkpoint state/.monty, release the checkout, and fence with a new `run_epoch`.
    Stop,
    /// Execute one bounded collection step immediately (approved revision only).
    RunOnce,
}

/// Structured error codes for [`InteractResponse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractErrorCode {
    NoApprovedScript,
    RevisionMismatch,
    UndeclaredAction,
    InvalidArgs,
    Busy,
    TimedOut,
    WaitingApproval,
    Paused,
    ExecutionError,
    Conflict,
}

/// Per-caller response to an interactive Deck action (`deck_interact`).
/// `result` is strictly private to the caller; shared broadcasts occur ONLY if Python called `deck_emit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InteractResponse {
    pub request_id: String,
    pub action: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<InteractErrorCode>,
    /// Always `true`: return results belong only to this caller and are never broadcast.
    pub private: bool,
    /// `true` when returned from the idempotency cache without re-executing Python.
    pub cached: bool,
    pub run_epoch: u64,
    pub script_rev: u64,
}

/// Summary row for listing Decks (left rail popover & `/decks`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckSummary {
    pub id: String,
    pub title: String,
    pub description: String,
    pub section_count: usize,
    pub session_id: SessionId,
    pub state: RuntimeState,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_rev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rev: Option<u64>,
    pub updated_ms: i64,
}

/// Full Deck detail payload for `/decks/$id` and `deck_get`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckDetail {
    pub doc: DeckDocSnapshot,
    pub runtime: DeckRuntimeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_script: Option<ScriptRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_script: Option<ScriptRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_snapshot: Option<DeckSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_frame: Option<LiveFrame>,
}

/// Per-Deck bounded subscription event (`deck_subscribe`).
/// Flows over per-Deck channels, NEVER the global `RuntimeWire`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum DeckRuntimeEvent {
    /// Initial state sent immediately upon subscribing.
    Sync {
        status: DeckRuntimeStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_snapshot: Option<DeckSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        live_frame: Option<LiveFrame>,
    },
    /// Transient live frame emitted by `deck_emit(data, durable=False)`.
    LiveFrame { frame: LiveFrame },
    /// Durable snapshot persisted by `deck_emit(data, durable=True)` or collection return.
    Snapshot { snapshot: DeckSnapshot },
    /// Runtime status change (streaming/paused/stopped/stale/error/epoch).
    Status { status: DeckRuntimeStatus },
    /// Tool approval pause surfaced to the Deck view.
    ApprovalPending { approval: PendingApproval },
    /// Tool approval resolved; Deck resumes or records denial.
    ApprovalCleared {
        approval_id: ApprovalId,
        approved: bool,
    },
    /// Subscription ended (Deck deleted or shutdown).
    Closed { reason: String },
}

/// Coarse global wire event for Deck list refresh (never carries live frames or snapshot payloads).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct DeckChanged {
    pub deck_id: String,
    pub title: String,
    pub updated_ms: i64,
    pub deleted: bool,
}

impl wire::WireEvent for DeckChanged {
    const TOPIC: &'static str = "decks.changed";
    const VERSION: u32 = 1;
}

/// Re-exports from `document::deck` for convenience in RPC/tool signatures.
pub use document::deck::SectionConflict;
pub type DeckSectionModel = DeckSection;
pub type SectionKindModel = SectionKind;
pub type SectionRevisionModel = SectionRevision;
