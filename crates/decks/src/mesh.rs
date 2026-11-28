//! Native Mesh Decks: single authoritative home engine + multi-source runtime ingestion.
//!
//! Architecture invariants:
//! - One authoritative **home engine** owns a Deck's runtime execution, snapshot/frame sequencing,
//!   trigger evaluation state, and routine dispatch. Runtime execution is **NOT** CRDT-elected.
//! - Non-home engines may read, render, and subscribe (keeping last-good Deck snapshots visible
//!   even while home is offline), but MUST refuse source ingest, trigger state writes, and routine
//!   dispatch rather than allowing dual-home execution.
//! - Any number of **source nodes** execute Monty locally against their own local `ToolRegistry`
//!   and local MCP tools (no distributed Python VM and no remote raw tool access) and push bounded,
//!   redacted [`DeckSourceEvent`] envelopes to the Deck's home engine.
//! - Source identity is bound to the authenticated engine/peer on the transport (`Forwarded(PeerId)`
//!   or local engine identity), never blindly trusted from untrusted RPC arguments.
//! - Delivery semantics: **no distributed exactly-once execution**. Home provides **persistent
//!   deduplicated ingest & dispatch** keyed by `(deck_id, source_id, source_epoch, seq)` and
//!   `(deck_id, trigger_key, source_id, source_epoch, seq)`. Duplicate retries return the persisted
//!   acknowledgment without duplicating frames, snapshots, or trigger dispatches.
//! - Historical events replayed from a source's bounded offline spool are explicitly tagged
//!   `replay: true`. Replayed history MUST NOT mark a source healthy (`Live`) and MUST NOT start,
//!   advance, or fire a fresh sustained-time threshold breach.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent::repl::MontyPool;
use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tools::ToolRegistry;

use crate::model::{MAX_METRIC_META_BYTES, MetricSample, MetricValue, ScriptRevision};
use crate::runtime::{DeckCheckout, MAX_SECTION_NAME_BYTES, TurnContext};
use crate::store::DeckStore;
use crate::triggers::is_safe_metric_name;

/// Maximum byte length of a [`DeckSourceStatus::source_id`].
pub const MAX_SOURCE_ID_BYTES: usize = 64;
/// Maximum byte length of a source display label.
pub const MAX_SOURCE_LABEL_BYTES: usize = 128;
/// Maximum byte length of an engine identifier.
pub const MAX_ENGINE_ID_BYTES: usize = 128;
/// Maximum serialized bytes for a source `frame` or `snapshot` payload.
pub const MAX_SOURCE_PAYLOAD_BYTES: usize = 256 * 1024;
/// Maximum serialized bytes for a source `heartbeat` payload.
pub const MAX_HEARTBEAT_PAYLOAD_BYTES: usize = 4 * 1024;
/// Maximum numeric metric keys carried on a single source event.
pub const MAX_SOURCE_METRICS: usize = 128;
/// Maximum byte length of a metric key name.
pub const MAX_SOURCE_METRIC_KEY_BYTES: usize = 64;
/// Default rolling 1-second ingest rate cap per `(deck_id, source_id)`.
pub const MAX_SOURCE_INGEST_PER_SEC: usize = 20;
/// Maximum dedup receipts retained per `(deck_id, source_id)` in SQLite.
pub const MAX_DEDUP_ENTRIES_PER_SOURCE: usize = 1024;
/// Default maximum queued events in a source's durable offline spool.
pub const DEFAULT_SPOOL_MAX_EVENTS: usize = 256;
/// Default maximum serialized bytes in a source's durable offline spool.
pub const DEFAULT_SPOOL_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Default source heartbeat cadence (seconds).
pub const DEFAULT_HEARTBEAT_SECS: u64 = 30;
/// Default threshold (seconds without fresh live event) before marking source `stale`.
pub const DEFAULT_STALE_AFTER_SECS: u64 = 90;
/// Default threshold (seconds without fresh live event) before marking source `offline`.
pub const DEFAULT_OFFLINE_AFTER_SECS: u64 = 300;
/// Bounds for source liveness thresholds.
pub const MIN_LIVENESS_SECS: u64 = 1;
pub const MAX_LIVENESS_SECS: u64 = 86_400;

/// Validate a typed source identifier (`1..=64` ASCII `[A-Za-z0-9._:-]`).
pub fn validate_source_id(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed != raw {
        return Err("source_id must be non-empty without surrounding whitespace".into());
    }
    if trimmed.len() > MAX_SOURCE_ID_BYTES {
        return Err(format!(
            "source_id `{trimmed}` exceeds {MAX_SOURCE_ID_BYTES} bytes"
        ));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
    {
        return Err(
            "source_id may only contain ASCII letters, digits, '.', '_', '-', or ':'".into(),
        );
    }
    Ok(trimmed.to_owned())
}

/// Validate a non-empty engine identifier.
pub fn validate_engine_id(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed != raw {
        return Err("engine_id must be non-empty without surrounding whitespace".into());
    }
    if trimmed.len() > MAX_ENGINE_ID_BYTES {
        return Err(format!("engine_id exceeds {MAX_ENGINE_ID_BYTES} bytes"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err("engine_id must not contain control characters".into());
    }
    Ok(trimmed.to_owned())
}

/// Clamp `(heartbeat_secs, stale_after_secs, offline_after_secs)` so `offline >= stale >= heartbeat`.
pub fn normalize_source_timeouts(
    heartbeat_secs: Option<u64>,
    stale_after_secs: Option<u64>,
    offline_after_secs: Option<u64>,
) -> (u64, u64, u64) {
    let hb = heartbeat_secs
        .unwrap_or(DEFAULT_HEARTBEAT_SECS)
        .clamp(MIN_LIVENESS_SECS, MAX_LIVENESS_SECS);
    let stale = stale_after_secs
        .unwrap_or_else(|| DEFAULT_STALE_AFTER_SECS.max(hb.saturating_mul(2)))
        .clamp(hb, MAX_LIVENESS_SECS);
    let offline = offline_after_secs
        .unwrap_or_else(|| DEFAULT_OFFLINE_AFTER_SECS.max(stale.saturating_mul(2)))
        .clamp(stale, MAX_LIVENESS_SECS);
    (hb, stale, offline)
}

/// Health state of a registered Deck source as tracked by the Deck's authoritative home engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceHealthState {
    /// Receiving fresh non-replay events/heartbeats within `stale_after_secs`.
    Live,
    /// No fresh live event/heartbeat within `stale_after_secs` (still under `offline_after_secs`).
    Stale,
    /// No fresh live event/heartbeat within `offline_after_secs` (or never seen live).
    #[default]
    Offline,
    /// Actively replaying historical spooled events (`replay: true`).
    /// Explicitly NOT healthy (`is_healthy() == false`) until a fresh live event arrives.
    Replaying,
}

impl SourceHealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Stale => "stale",
            Self::Offline => "offline",
            Self::Replaying => "replaying",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "live" => Self::Live,
            "stale" => Self::Stale,
            "replaying" => Self::Replaying,
            _ => Self::Offline,
        }
    }

    /// Only `Live` counts as a currently healthy source eligible for live trigger evidence.
    pub fn is_healthy(self) -> bool {
        self == Self::Live
    }
}

/// Kind of event emitted by a source node to its Deck's home engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceEventKind {
    /// Transient redacted live frame / metric payload.
    #[default]
    Frame,
    /// Durable redacted snapshot payload persisted on home.
    Snapshot,
    /// Liveness heartbeat (does not overwrite last-good frame/snapshot data).
    Heartbeat,
}

impl SourceEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Snapshot => "snapshot",
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// Persisted registration binding a logical `source_id` on a Deck to an authenticated `engine_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeckSourceRegistration {
    pub deck_id: String,
    pub source_id: String,
    /// Authenticated mesh engine ID allowed to emit events for this `source_id`.
    pub engine_id: String,
    pub label: String,
    pub heartbeat_secs: u64,
    pub stale_after_secs: u64,
    pub offline_after_secs: u64,
    pub enabled: bool,
    pub registered_at: String,
    pub updated_at: String,
}

/// Wire envelope emitted by a source node to the Deck's authoritative home engine.
///
/// Note: intentionally carries NO untrusted `engine_id` field for authentication — the home engine
/// binds `source_id` strictly against the transport-authenticated engine/peer identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckSourceEvent {
    pub deck_id: String,
    pub source_id: String,
    /// Monotonically increasing epoch per source start/restart (`>= 1`). Fences stale epochs.
    pub source_epoch: u64,
    /// Monotonically increasing sequence number within `(source_id, source_epoch)` (`>= 1`).
    pub seq: u64,
    /// RFC 3339 timestamp when the source observed this reading.
    pub observed_at: String,
    /// `true` when replayed from the source's offline spool. Replayed history MUST NOT mark a
    /// source healthy (`Live`) or create/advance a fresh sustained-time trigger breach.
    #[serde(default)]
    pub replay: bool,
    #[serde(default)]
    pub kind: SourceEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Redacted metric/frame payload.
    #[serde(default)]
    pub payload: Value,
    /// Optional numeric metrics extracted from the payload for fast trigger evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<BTreeMap<String, f64>>,
    /// Optional structured `deck_metric(name, value, meta=None)` samples (numbers or booleans)
    /// emitted by a source script turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_samples: Option<Vec<MetricSample>>,
}

/// Redact secrets and validate structural/size caps on an incoming or outgoing [`DeckSourceEvent`].
pub fn sanitize_and_validate_source_event(
    event: &mut DeckSourceEvent,
) -> Result<(DateTime<Utc>, usize), String> {
    if event.deck_id.trim().is_empty() {
        return Err("deck_id must be non-empty".into());
    }
    validate_source_id(&event.source_id)?;
    if event.source_epoch == 0 {
        return Err("source_epoch must be >= 1".into());
    }
    if event.seq == 0 {
        return Err("seq must be >= 1".into());
    }
    let observed_at = DateTime::parse_from_rfc3339(&event.observed_at)
        .map_err(|e| format!("invalid observed_at RFC 3339 timestamp: {e}"))?
        .with_timezone(&Utc);

    // Redact payload & section BEFORE enforcing caps or storing/broadcasting.
    tools::secrets::redact_value(&mut event.payload);
    if let Some(sec) = event.section.as_mut() {
        let redacted = tools::secrets::redact(sec).into_owned();
        let trimmed = redacted.trim();
        if trimmed.is_empty() {
            event.section = None;
        } else if trimmed.len() > MAX_SECTION_NAME_BYTES {
            return Err(format!(
                "section name exceeds {MAX_SECTION_NAME_BYTES} bytes"
            ));
        } else if trimmed.chars().any(char::is_control) {
            return Err("section name must not contain control characters".into());
        } else {
            *sec = trimmed.to_owned();
        }
    }

    if let Some(metrics) = &event.metrics {
        if metrics.len() > MAX_SOURCE_METRICS {
            return Err(format!(
                "source metrics count ({}) exceeds cap ({MAX_SOURCE_METRICS})",
                metrics.len()
            ));
        }
        for (k, v) in metrics {
            if k.is_empty() || k.len() > MAX_SOURCE_METRIC_KEY_BYTES || k.chars().any(char::is_control)
            {
                return Err(format!("invalid metric key {k:?}"));
            }
            if !v.is_finite() {
                return Err(format!("metric `{k}` must be a finite number"));
            }
        }
    }

    if let Some(samples) = event.metric_samples.as_mut() {
        if samples.len() > MAX_SOURCE_METRICS {
            return Err(format!(
                "source metric_samples count ({}) exceeds cap ({MAX_SOURCE_METRICS})",
                samples.len()
            ));
        }
        for s in samples.iter_mut() {
            if s.name.trim() != s.name || !is_safe_metric_name(&s.name) {
                return Err(format!("invalid metric sample name {:?}", s.name));
            }
            if !s.value.is_finite() {
                return Err(format!("metric sample `{}` must be finite", s.name));
            }
            s.value = s.value.normalized();
            if let Some(meta) = s.meta.as_mut() {
                tools::secrets::redact_value(meta);
                let meta_bytes = serde_json::to_vec(meta)
                    .map(|b| b.len())
                    .unwrap_or(usize::MAX);
                if meta_bytes > MAX_METRIC_META_BYTES {
                    return Err(format!(
                        "metric sample `{}` meta ({meta_bytes} bytes) exceeds cap ({MAX_METRIC_META_BYTES})",
                        s.name
                    ));
                }
            }
        }
    }

    let payload_bytes = serde_json::to_vec(&event.payload)
        .map(|b| b.len())
        .unwrap_or(usize::MAX);
    let cap = match event.kind {
        SourceEventKind::Heartbeat => MAX_HEARTBEAT_PAYLOAD_BYTES,
        SourceEventKind::Frame | SourceEventKind::Snapshot => MAX_SOURCE_PAYLOAD_BYTES,
    };
    if payload_bytes > cap {
        return Err(format!(
            "source event payload ({payload_bytes} bytes) exceeds cap ({cap} bytes)"
        ));
    }

    Ok((observed_at, payload_bytes))
}

/// Extract finite top-level numeric metrics from a JSON object payload when explicit `metrics`
/// were not provided by the caller.
pub fn extract_numeric_metrics(payload: &Value) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    let Some(map) = payload.as_object() else {
        return out;
    };
    for (k, v) in map {
        if out.len() >= MAX_SOURCE_METRICS {
            break;
        }
        if k.is_empty() || k.len() > MAX_SOURCE_METRIC_KEY_BYTES || k.chars().any(char::is_control)
        {
            continue;
        }
        if let Some(n) = v.as_f64().filter(|n| n.is_finite()) {
            out.insert(k.clone(), n);
        }
    }
    out
}

/// Outcome classification for [`IngestSourceAck`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IngestStatus {
    Accepted,
    Duplicate,
    RejectedStaleEpoch,
    RejectedRateLimited,
    RejectedNotHome,
    RejectedUnauthorizedSource,
    RejectedInvalid,
}

/// Persisted acknowledgment returned by home `deck_source_ingest`.
///
/// Duplicate retries for an already-accepted `(deck_id, source_id, source_epoch, seq)` return the
/// persisted acknowledgment with `duplicate = true` without re-emitting frames/snapshots or
/// re-dispatching triggers (persistent deduplicated dispatch, not distributed exactly-once).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IngestSourceAck {
    pub accepted: bool,
    pub duplicate: bool,
    pub status: IngestStatus,
    pub deck_id: String,
    pub source_id: String,
    pub source_epoch: u64,
    pub seq: u64,
    pub home_engine_id: String,
    pub home_epoch: u64,
    pub source_health: SourceHealthState,
    pub replay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_seq: Option<u64>,
    #[serde(default)]
    pub trigger_dispatched: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_dedup_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Observable runtime status of one registered source on a Deck (rendered in Deck UI).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeckSourceStatus {
    pub deck_id: String,
    pub source_id: String,
    pub engine_id: String,
    pub label: String,
    pub health: SourceHealthState,
    pub replaying: bool,
    pub enabled: bool,
    pub last_epoch: u64,
    pub last_seq: u64,
    pub last_live_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_received_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_live_received_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_ms: Option<u64>,
    #[serde(default)]
    pub spool_depth: u64,
    #[serde(default)]
    pub dropped_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Observable mesh ownership and per-source health model for a Deck.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeckMeshStatus {
    pub deck_id: String,
    pub home_engine_id: String,
    pub local_engine_id: String,
    pub is_home: bool,
    pub home_online: bool,
    pub home_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_paused_reason: Option<String>,
    #[serde(default)]
    pub sources: Vec<DeckSourceStatus>,
}

/// Result of an explicit owner-initiated `deck_move_home` transfer.
///
/// Transfers NEVER auto-failover and ALWAYS cold-start the new home (clearing `.monty` heap dumps,
/// bumping `home_epoch` + `run_epoch`, and pausing recurring execution until local MCP/approvals
/// are verified and the owner explicitly starts the Deck).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeckMoveHomeResult {
    pub deck_id: String,
    pub previous_home_engine_id: String,
    pub home_engine_id: String,
    pub home_epoch: u64,
    pub run_epoch: u64,
    pub cold_started: bool,
    pub local_mcp_verified: bool,
    pub verified_tool_count: usize,
    pub requires_owner_start: bool,
}

// ---------------------------------------------------------------------------
// Trigger Evaluation Adapter (integration seam for `feat/decks-triggers`)
// ---------------------------------------------------------------------------

/// Normalized, redacted, deduplicated source metric sample handed to the trigger adapter on home.
#[derive(Clone, Debug)]
pub struct SourceTriggerSample {
    pub deck_id: String,
    pub source_id: String,
    pub source_epoch: u64,
    pub seq: u64,
    pub observed_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub replay: bool,
    pub kind: SourceEventKind,
    pub source_health: SourceHealthState,
    pub payload: Value,
    pub metrics: BTreeMap<String, f64>,
    pub metric_samples: Vec<MetricSample>,
}

/// Whether a [`SourceTriggerSample`] is eligible to serve as fresh live trigger evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerEvidenceDecision {
    /// Fresh non-replay frame/snapshot event from a healthy (`Live`) source on the authoritative home engine.
    EligibleLive,
    /// Replayed historical event — MUST NOT mark source healthy or create/advance sustained breaches.
    IgnoredReplay,
    /// Liveness heartbeat — refreshes liveness only and MUST NOT open, advance, or fire sustained thresholds.
    IgnoredHeartbeat,
    /// Source is currently unhealthy (`Stale`, `Offline`, or `Replaying`).
    IgnoredUnhealthySource(SourceHealthState),
    /// Non-home engine MUST NEVER write trigger state or dispatch routines.
    IgnoredNotHome,
}

/// Pure classifier enforcing mesh trigger evidence invariants.
///
/// Called by the home ingest path before evaluating any [`crate::model::TriggerSpec`]
/// sustained window or dispatching any routine.
pub fn classify_trigger_evidence(
    is_home: bool,
    sample: &SourceTriggerSample,
) -> TriggerEvidenceDecision {
    if !is_home {
        return TriggerEvidenceDecision::IgnoredNotHome;
    }
    if sample.replay {
        return TriggerEvidenceDecision::IgnoredReplay;
    }
    if sample.kind == SourceEventKind::Heartbeat {
        return TriggerEvidenceDecision::IgnoredHeartbeat;
    }
    if !sample.source_health.is_healthy() {
        return TriggerEvidenceDecision::IgnoredUnhealthySource(sample.source_health);
    }
    TriggerEvidenceDecision::EligibleLive
}

/// Persisted deduplicated trigger dispatch record.
///
/// Semantics: **persistent deduplicated dispatch, NOT distributed exactly-once execution**.
/// Each unique `(deck_id, trigger_key, source_id, source_epoch, seq)` dispatch key is persisted on
/// the home engine so duplicate ingest retries never re-dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TriggerDispatchRecord {
    pub deck_id: String,
    pub trigger_key: String,
    pub source_id: String,
    pub source_epoch: u64,
    pub seq: u64,
    pub dedup_key: String,
    pub fired_at: String,
}

/// Persisted per-trigger state stored on the Deck's home engine (`deck_trigger_fire_state`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerFireState {
    pub deck_id: String,
    pub trigger_key: String,
    pub source_id: String,
    pub source_epoch: u64,
    pub last_seq: u64,
    pub sustained_since_ms: Option<i64>,
    pub fired_count: u64,
    pub last_fired_at: Option<String>,
    pub last_dispatch_dedup_key: Option<String>,
}

/// Adapter seam invoked by the home ingest path after authentication, home check, dedup,
/// epoch fencing, and redaction succeed.
///
/// `feat/decks-triggers` plugs its `TriggerSpec` evaluator and `schedule` routine dispatcher here
/// without duplicating transport, auth, dedup, replay, or home-ownership logic.
#[async_trait::async_trait]
pub trait SourceTriggerAdapter: Send + Sync {
    async fn on_source_sample(
        &self,
        sample: &SourceTriggerSample,
        decision: TriggerEvidenceDecision,
    ) -> Result<Option<TriggerDispatchRecord>, String>;
}

/// Lightweight sustained-threshold rule used by [`SustainedTriggerAdapter`] to exercise the
/// home trigger seam and verify replay/stale/dedup invariants before `feat/decks-triggers` lands.
#[derive(Clone, Debug, PartialEq)]
pub struct SustainedThresholdRule {
    pub trigger_key: String,
    pub metric: String,
    pub threshold: f64,
    /// Minimum continuous seconds on FRESH live readings before firing (`0` = immediate).
    pub sustained_secs: u64,
    /// Optional single-source scope; `None` matches any registered source.
    pub source_id: Option<String>,
}

/// Default small trigger adapter backed by `DeckStore::deck_trigger_fire_state`.
///
/// Enforces:
/// - Only `TriggerEvidenceDecision::EligibleLive` can open, advance, or fire a sustained window.
/// - `IgnoredReplay` / `IgnoredUnhealthySource` / `IgnoredNotHome` never advance or fire.
/// - Persistent deduplicated dispatch keyed by `(deck_id, trigger_key, source_id, source_epoch, seq)`.
pub struct SustainedTriggerAdapter {
    store: DeckStore,
    rules: parking_lot::RwLock<Vec<SustainedThresholdRule>>,
}

impl SustainedTriggerAdapter {
    pub fn new(store: DeckStore, rules: Vec<SustainedThresholdRule>) -> Self {
        Self {
            store,
            rules: parking_lot::RwLock::new(rules),
        }
    }

    pub fn set_rules(&self, rules: Vec<SustainedThresholdRule>) {
        *self.rules.write() = rules;
    }
}

#[async_trait::async_trait]
impl SourceTriggerAdapter for SustainedTriggerAdapter {
    async fn on_source_sample(
        &self,
        sample: &SourceTriggerSample,
        decision: TriggerEvidenceDecision,
    ) -> Result<Option<TriggerDispatchRecord>, String> {
        if decision != TriggerEvidenceDecision::EligibleLive {
            // Replay, unhealthy source, or non-home MUST NEVER advance or fire a sustained trigger.
            return Ok(None);
        }
        let rules = self.rules.read().clone();
        if rules.is_empty() {
            return Ok(None);
        }
        let mut first_dispatch = None;
        for rule in rules {
            if let Some(ref only_src) = rule.source_id
                && only_src != &sample.source_id
            {
                continue;
            }
            let Some(&value) = sample.metrics.get(&rule.metric) else {
                continue;
            };
            let breached = value >= rule.threshold;
            let dispatched = self
                .store
                .evaluate_sustained_trigger(
                    &sample.deck_id,
                    &rule.trigger_key,
                    &sample.source_id,
                    sample.source_epoch,
                    sample.seq,
                    sample.received_at,
                    breached,
                    rule.sustained_secs,
                )
                .await
                .map_err(|e| format!("{e:#}"))?;
            if first_dispatch.is_none() && dispatched.is_some() {
                first_dispatch = dispatched;
            }
        }
        Ok(first_dispatch)
    }
}

// ---------------------------------------------------------------------------
// Bounded Durable Offline Spool & Local Source Runtime
// ---------------------------------------------------------------------------

/// Bounded capacity limits for a source node's durable offline spool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpoolLimits {
    pub max_events: usize,
    pub max_bytes: usize,
}

impl Default for SourceSpoolLimits {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_SPOOL_MAX_EVENTS,
            max_bytes: DEFAULT_SPOOL_MAX_BYTES,
        }
    }
}

/// Current occupancy of a source's durable offline spool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpoolStats {
    pub depth: usize,
    pub bytes: usize,
}

/// Errors returned when spooling an offline event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpoolError {
    /// Spool reached its bounded `max_events` or `max_bytes` cap — apply backpressure!
    Backpressure {
        depth: usize,
        max_events: usize,
        bytes: usize,
        max_bytes: usize,
    },
    Storage(String),
}

impl std::fmt::Display for SpoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backpressure {
                depth,
                max_events,
                bytes,
                max_bytes,
            } => write!(
                f,
                "source spool backpressure: depth={depth}/{max_events}, bytes={bytes}/{max_bytes}"
            ),
            Self::Storage(s) => write!(f, "source spool storage error: {s}"),
        }
    }
}

impl std::error::Error for SpoolError {}

/// Transport error distinguishing retryable offline/backpressure conditions from permanent rejections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceSendError {
    /// Home engine is unreachable, offline, timed out, or transiently rate-limited -> spool & retry.
    Unavailable(String),
    /// Home permanently rejected the event (unauthorized source, invalid payload, wrong home).
    Rejected(String),
}

impl std::fmt::Display for SourceSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "home unavailable: {s}"),
            Self::Rejected(s) => write!(f, "home rejected event: {s}"),
        }
    }
}

/// Transport delivering redacted [`DeckSourceEvent`] envelopes from a source engine to the home engine.
#[async_trait::async_trait]
pub trait SourceTransport: Send + Sync {
    async fn send_event(
        &self,
        home_engine_id: &str,
        event: &DeckSourceEvent,
    ) -> Result<IngestSourceAck, SourceSendError>;
}

/// Production mesh transport sending `deck_source_ingest` to the home engine over `theuth/rpc/1`
/// (authenticated by the iroh Ed25519 peer identity via `agent::rpc_forward::forward_within`).
pub struct MeshRpcSourceTransport {
    transport: Arc<dyn replica::Transport>,
    timeout: Duration,
}

impl MeshRpcSourceTransport {
    pub fn new(transport: Arc<dyn replica::Transport>) -> Self {
        Self {
            transport,
            timeout: Duration::from_secs(15),
        }
    }
}

#[async_trait::async_trait]
impl SourceTransport for MeshRpcSourceTransport {
    async fn send_event(
        &self,
        home_engine_id: &str,
        event: &DeckSourceEvent,
    ) -> Result<IngestSourceAck, SourceSendError> {
        let peer = replica::PeerId(home_engine_id.to_owned());
        if !self.transport.peers().contains(&peer) {
            return Err(SourceSendError::Unavailable(format!(
                "home engine `{home_engine_id}` is not connected"
            )));
        }
        let params = serde_json::value::to_raw_value(&json!([event]))
            .map_err(|e| SourceSendError::Rejected(format!("serialize source event: {e}")))?;
        match agent::rpc_forward::forward_within(
            self.transport.as_ref(),
            &peer,
            "deck_source_ingest",
            &params,
            self.timeout,
        )
        .await
        {
            Ok(raw) => serde_json::from_str::<IngestSourceAck>(raw.get())
                .map_err(|e| SourceSendError::Unavailable(format!("malformed ingest ack: {e}"))),
            Err(Ok(why)) => Err(SourceSendError::Unavailable(why)),
            Err(Err(rpc_err)) => Err(SourceSendError::Rejected(rpc_err.message().to_owned())),
        }
    }
}

/// Outcome of delivering one event from [`DeckSourceRuntime`].
#[derive(Clone, Debug)]
pub enum SourceEmitOutcome {
    /// Delivered directly to home engine (fresh live event).
    Delivered(IngestSourceAck),
    /// Home unavailable or prior spooled events pending -> durably spooled for ordered replay.
    Spooled { stats: SpoolStats, reason: String },
    /// Spool full -> backpressure applied; event was not buffered.
    Backpressured { error: SpoolError },
}

/// Outcome of draining a source's durable offline spool upon reconnect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayFlushReport {
    pub replayed: usize,
    pub duplicates: usize,
    pub fenced_dropped: usize,
    pub remaining: usize,
    pub stopped_reason: Option<String>,
}

/// Local source runtime running Monty locally on a source engine and shipping redacted
/// [`DeckSourceEvent`] envelopes to the Deck's authoritative home engine with bounded offline
/// spooling and ordered reconnect replay.
///
/// Full session tools and MCP servers remain strictly LOCAL to this source engine (`registry` +
/// `session_id`); there is no distributed Python VM and no remote raw tool access.
pub struct DeckSourceRuntime {
    deck_id: String,
    source_id: String,
    home_engine_id: String,
    store: DeckStore,
    limits: SourceSpoolLimits,
    transport: Arc<dyn SourceTransport>,
    source_epoch: u64,
    seq: u64,
    dropped_count: u64,
    backpressured: bool,
    checkout: Option<DeckCheckout>,
    last_state: Value,
    last_snapshot: Value,
}

impl DeckSourceRuntime {
    /// Create a new source runtime and allocate a monotonically higher persisted `source_epoch`
    /// so any previous incarnation of this source is fenced on home.
    pub async fn start(
        deck_id: String,
        source_id: String,
        home_engine_id: String,
        store: DeckStore,
        transport: Arc<dyn SourceTransport>,
        limits: SourceSpoolLimits,
    ) -> Result<Self, String> {
        validate_source_id(&source_id)?;
        validate_engine_id(&home_engine_id)?;
        let source_epoch = store
            .next_local_source_epoch(&deck_id, &source_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(Self {
            deck_id,
            source_id,
            home_engine_id,
            store,
            limits,
            transport,
            source_epoch,
            seq: 0,
            dropped_count: 0,
            backpressured: false,
            checkout: None,
            last_state: Value::Null,
            last_snapshot: Value::Null,
        })
    }

    pub fn deck_id(&self) -> &str {
        &self.deck_id
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn source_epoch(&self) -> u64 {
        self.source_epoch
    }

    pub fn last_seq(&self) -> u64 {
        self.seq
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped_count
    }

    pub fn is_backpressured(&self) -> bool {
        self.backpressured
    }

    pub fn set_home_engine(&mut self, home_engine_id: String) -> Result<(), String> {
        validate_engine_id(&home_engine_id)?;
        self.home_engine_id = home_engine_id;
        Ok(())
    }

    pub async fn spool_stats(&self) -> Result<SpoolStats, String> {
        self.store
            .spool_stats(&self.deck_id, &self.source_id)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Initialize a local Monty checkout on this source engine using this engine's local
    /// `ToolRegistry` and isolated working directory. Never shares a Python VM across engines.
    pub async fn attach_local_monty(
        &mut self,
        pool: Arc<MontyPool>,
        registry: ActorRef<ToolRegistry>,
        session_id: SessionId,
        work_dir: PathBuf,
        script: &ScriptRevision,
    ) -> Result<(), String> {
        let turn_cx = TurnContext {
            last_state: self.last_state.clone(),
            last_snapshot: self.last_snapshot.clone(),
        };
        let dump_path = self.store.monty_dump_path(session_id);
        let (checkout, _mode, effects) = DeckCheckout::open_or_restore(
            pool,
            registry,
            session_id,
            &work_dir,
            &self.store.spill_dir(),
            dump_path,
            script,
            None,
            false,
            &turn_cx,
            &mut |_| {},
        )
        .await?;
        if let Some(st) = effects.checkpoint_state {
            self.last_state = st;
        }
        self.checkout = Some(checkout);
        Ok(())
    }

    /// Execute one local Monty `on_step()` / `collect()` turn and emit each redacted `deck_emit`
    /// payload and `deck_metric` sample as [`DeckSourceEvent`]s to the Deck's home engine
    /// (spooling offline if needed).
    pub async fn run_local_step_and_emit(&mut self) -> Result<Vec<SourceEmitOutcome>, String> {
        let turn_cx = TurnContext {
            last_state: self.last_state.clone(),
            last_snapshot: self.last_snapshot.clone(),
        };
        let Some(checkout) = self.checkout.as_mut() else {
            return Err("local Monty checkout is not attached to this source runtime".into());
        };
        let outcome = checkout.run_step(&turn_cx, &mut |_| {}).await?;
        if let Some(st) = outcome.effects.checkpoint_state {
            self.last_state = st;
        }
        let mut turn_metrics = (!outcome.effects.metrics.is_empty()).then_some(outcome.effects.metrics);
        let mut results = Vec::new();
        let emit_count = outcome.effects.emits.len();
        for (idx, emit) in outcome.effects.emits.into_iter().enumerate() {
            let kind = if emit.durable {
                SourceEventKind::Snapshot
            } else {
                SourceEventKind::Frame
            };
            if emit.durable {
                self.last_snapshot = emit.data.clone();
            }
            let attach_metrics = if idx + 1 == emit_count {
                turn_metrics.take()
            } else {
                None
            };
            let res = self
                .emit_payload_with_metrics(
                    kind,
                    emit.data,
                    emit.section,
                    None,
                    attach_metrics,
                    Utc::now(),
                )
                .await?;
            results.push(res);
        }
        if results.is_empty() && !outcome.return_value.is_null() {
            self.last_snapshot = outcome.return_value.clone();
            let res = self
                .emit_payload_with_metrics(
                    SourceEventKind::Snapshot,
                    outcome.return_value,
                    None,
                    None,
                    turn_metrics.take(),
                    Utc::now(),
                )
                .await?;
            results.push(res);
        }
        if results.is_empty()
            && let Some(samples) = turn_metrics.take()
        {
            let res = self
                .emit_payload_with_metrics(
                    SourceEventKind::Frame,
                    Value::Null,
                    None,
                    None,
                    Some(samples),
                    Utc::now(),
                )
                .await?;
            results.push(res);
        }
        Ok(results)
    }

    /// Emit a redacted live event (or spool it durably if home is unavailable or spool is non-empty).
    pub async fn emit_payload(
        &mut self,
        kind: SourceEventKind,
        payload: Value,
        section: Option<String>,
        metrics: Option<BTreeMap<String, f64>>,
        observed_at: DateTime<Utc>,
    ) -> Result<SourceEmitOutcome, String> {
        self.emit_payload_with_metrics(kind, payload, section, metrics, None, observed_at)
            .await
    }

    /// Emit a redacted live event carrying optional structured `deck_metric` samples.
    pub async fn emit_payload_with_metrics(
        &mut self,
        kind: SourceEventKind,
        mut payload: Value,
        section: Option<String>,
        metrics: Option<BTreeMap<String, f64>>,
        metric_samples: Option<Vec<MetricSample>>,
        observed_at: DateTime<Utc>,
    ) -> Result<SourceEmitOutcome, String> {
        tools::secrets::redact_value(&mut payload);
        let resolved_metrics = metrics.or_else(|| {
            let mut extracted = extract_numeric_metrics(&payload);
            if let Some(samples) = &metric_samples {
                for s in samples {
                    if let MetricValue::Number(v) = s.value
                        && v.is_finite()
                        && extracted.len() < MAX_SOURCE_METRICS
                    {
                        extracted.insert(s.name.clone(), v);
                    }
                }
            }
            (!extracted.is_empty()).then_some(extracted)
        });
        let next_seq = self.seq.saturating_add(1);
        let mut event = DeckSourceEvent {
            deck_id: self.deck_id.clone(),
            source_id: self.source_id.clone(),
            source_epoch: self.source_epoch,
            seq: next_seq,
            observed_at: observed_at.to_rfc3339(),
            replay: false,
            kind,
            section,
            payload,
            metrics: resolved_metrics,
            metric_samples,
        };
        sanitize_and_validate_source_event(&mut event)?;

        // Preserve strict ordering: if older offline events are still spooled, enqueue behind them
        // instead of overtaking historical sequence numbers!
        let existing = self.spool_stats().await?;
        if existing.depth > 0 {
            return Ok(self
                .enqueue_to_spool(event, "spool_non_empty_preserving_order")
                .await);
        }

        match self.transport.send_event(&self.home_engine_id, &event).await {
            Ok(ack) if ack.status == IngestStatus::RejectedRateLimited => {
                Ok(self.enqueue_to_spool(event, "home_rate_limited").await)
            }
            Ok(ack) => {
                self.seq = next_seq;
                self.backpressured = false;
                Ok(SourceEmitOutcome::Delivered(ack))
            }
            Err(SourceSendError::Unavailable(why)) => Ok(self.enqueue_to_spool(event, &why).await),
            Err(SourceSendError::Rejected(why)) => Err(why),
        }
    }

    /// Emit a fresh live heartbeat (never spooled when offline — heartbeats are live liveness only).
    pub async fn emit_heartbeat(&mut self) -> Result<IngestSourceAck, SourceSendError> {
        let next_seq = self.seq.saturating_add(1);
        let event = DeckSourceEvent {
            deck_id: self.deck_id.clone(),
            source_id: self.source_id.clone(),
            source_epoch: self.source_epoch,
            seq: next_seq,
            observed_at: Utc::now().to_rfc3339(),
            replay: false,
            kind: SourceEventKind::Heartbeat,
            section: None,
            payload: Value::Null,
            metrics: None,
            metric_samples: None,
        };
        let ack = self
            .transport
            .send_event(&self.home_engine_id, &event)
            .await?;
        self.seq = next_seq;
        Ok(ack)
    }

    async fn enqueue_to_spool(&mut self, event: DeckSourceEvent, reason: &str) -> SourceEmitOutcome {
        match self.store.spool_enqueue(&event, self.limits).await {
            Ok(stats) => {
                self.seq = event.seq;
                self.backpressured = false;
                SourceEmitOutcome::Spooled {
                    stats,
                    reason: reason.to_owned(),
                }
            }
            Err(err @ SpoolError::Backpressure { .. }) => {
                self.dropped_count = self.dropped_count.saturating_add(1);
                self.backpressured = true;
                SourceEmitOutcome::Backpressured { error: err }
            }
            Err(err) => {
                self.dropped_count = self.dropped_count.saturating_add(1);
                SourceEmitOutcome::Backpressured { error: err }
            }
        }
    }

    /// Drain durably spooled offline events in strict `(source_epoch ASC, seq ASC)` order,
    /// explicitly tagging every replayed event with `replay: true`.
    ///
    /// Stops immediately on transport failure or home rate limiting so ordering is never violated.
    pub async fn flush_spool(&mut self, max_batch: usize) -> Result<ReplayFlushReport, String> {
        let mut report = ReplayFlushReport::default();
        let batch_cap = max_batch.max(1);
        let items = self
            .store
            .spool_peek_batch(&self.deck_id, &self.source_id, batch_cap)
            .await
            .map_err(|e| format!("{e:#}"))?;

        for mut event in items {
            // Replayed offline history MUST ALWAYS be tagged replay=true!
            event.replay = true;
            match self.transport.send_event(&self.home_engine_id, &event).await {
                Ok(ack) if ack.status == IngestStatus::RejectedRateLimited => {
                    report.stopped_reason = Some("home_rate_limited".into());
                    break;
                }
                Ok(ack) if ack.status == IngestStatus::RejectedNotHome => {
                    report.stopped_reason = Some("target_not_home".into());
                    break;
                }
                Ok(ack) if ack.status == IngestStatus::RejectedStaleEpoch => {
                    // Older epoch permanently fenced on home; drop from spool so newer epochs can proceed.
                    let _ = self
                        .store
                        .spool_remove(&event.deck_id, &event.source_id, event.source_epoch, event.seq)
                        .await;
                    report.fenced_dropped += 1;
                }
                Ok(ack) if ack.accepted || ack.duplicate => {
                    let _ = self
                        .store
                        .spool_remove(&event.deck_id, &event.source_id, event.source_epoch, event.seq)
                        .await;
                    if ack.duplicate {
                        report.duplicates += 1;
                    } else {
                        report.replayed += 1;
                    }
                }
                Ok(ack) => {
                    report.stopped_reason = ack
                        .error
                        .or_else(|| Some(format!("rejected:{:?}", ack.status)));
                    break;
                }
                Err(SourceSendError::Unavailable(why)) => {
                    report.stopped_reason = Some(why);
                    break;
                }
                Err(SourceSendError::Rejected(why)) => {
                    report.stopped_reason = Some(why);
                    break;
                }
            }
        }

        let remaining = self.spool_stats().await?.depth;
        report.remaining = remaining;
        if remaining < self.limits.max_events {
            self.backpressured = false;
        }
        Ok(report)
    }
}
