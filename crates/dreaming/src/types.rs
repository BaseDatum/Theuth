//! Dreaming's wire and on-disk types (design doc 136 §2.5–2.7, §6, §7.2):
//! cases, learned edits, nights, signals, status, settings, proposals,
//! metrics. Every type is serde + schemars + ts export; the JSON shapes of
//! `Case` and `EditRecord` are binding (contract §2.6).

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// A night's id: the local date `YYYY-MM-DD` the night began (136 §2.5).
pub type NightId = String;

/// Why a case exists (136 §6).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "kebab-case")]
pub enum CaseKind {
    /// The user corrected the assistant.
    #[default]
    Correction,
    /// The user asked again.
    ReAsk,
    /// The user disapproved.
    Disapproval,
    /// A rewind deleted the tail.
    Rewind,
    /// The turn was stopped.
    Stop,
    /// An approval or ask was denied.
    Denial,
    /// ≥ 3 errors on one tool in a row.
    ToolChain,
    /// A hint ignored and the id then opened by search.
    HintMiss,
    /// A desk task sent back.
    TaskReturned,
}

/// Train / validation split (136 §4 phase 3; stable hash).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// The proposer sees it.
    Train,
    /// Only the judge sees it.
    Val,
}

/// Where a case came from.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum CaseOrigin {
    /// Mined from the ledger.
    #[default]
    Real,
    /// Written by a model; never `val`.
    Synthetic,
}

/// A case's life (136 §6, §7.2 step 5).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum CaseState {
    /// Not yet addressed by an edit.
    #[default]
    Open,
    /// An accepted edit closed it (`closed_by`).
    Closed,
    /// A later case of the same intent reopened it.
    Reopened,
    /// > 180 days without a reopen; out of the replay pool.
    Settled,
}

/// One unit of hindsight (136 §6; JSON binding per contract §2.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Case {
    /// Ulid.
    pub id: String,
    /// The night that mined it.
    pub night: NightId,
    /// What kind of friction.
    pub kind: CaseKind,
    /// The generalised intent.
    pub intent: String,
    /// ≤ 1,200 chars ending just before the mistake; scrubbed.
    pub context: String,
    /// What the assistant did.
    pub happened: String,
    /// The user's words, verbatim; scrubbed.
    pub user_words: String,
    /// What right would have looked like.
    pub ideal: String,
    /// Free tags.
    pub tags: Vec<String>,
    /// `<skill name>` | `prompt:standing-preferences` | `knowledge:<id>`.
    pub skill_hint: String,
    /// Where it happened.
    pub source_session: SessionId,
    /// RFC 3339.
    pub at: String,
    /// Train or val.
    pub split: Split,
    /// Real or synthetic.
    pub origin: CaseOrigin,
    /// Life state.
    pub state: CaseState,
    /// `edit:<ulid>` when closed.
    pub closed_by: Option<String>,
    /// RFC 3339 when reopened.
    pub reopened_at: Option<String>,
}

/// A bounded edit op (136 §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "LearnedEditOp")
)]
#[serde(rename_all = "lowercase")]
pub enum EditOp {
    /// Append a bullet (after `anchor` when given).
    Add,
    /// Replace the bullet matching `anchor` exactly.
    Replace,
    /// Remove the bullet matching `anchor` exactly.
    Delete,
    /// Retire a dreaming-made skill (136 D4): `skills/dreaming-<skill>.md`
    /// moves to `skills/retired/<skill>.md` with a `retired_because:` line.
    /// Needs no anchor or gate; receipted and undone with the sidecars.
    Retire,
}

/// The gate's verdict on an edit (136 §7.2 step 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "GateVerdict"))]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Applied, live.
    Accepted,
    /// Into the rejected buffer with the judge's reasons.
    Rejected,
    /// Fewer than `min_val_cases`; staged in the journal, not applied.
    Unverified,
}

/// The replay numbers behind a verdict.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GateResult {
    /// Mean judge score of the current text over the val cases.
    pub baseline: f64,
    /// Mean judge score of the candidate text.
    pub candidate: f64,
    /// The worst per-case `candidate − baseline`.
    pub min_delta: f64,
    /// Val cases replayed.
    pub val_cases: u32,
    /// The verdict.
    pub verdict: Verdict,
}

/// One learned edit and its outcome (136 §7.2; JSON binding per contract §2.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EditRecord {
    /// Ulid; the server assigns it when empty (136 D3).
    #[serde(default)]
    pub id: String,
    /// The night that proposed it; the server assigns it when empty.
    #[serde(default)]
    pub night: NightId,
    /// `skill:<name>` | `preferences`; the server assigns it.
    #[serde(default)]
    pub target: String,
    /// The op.
    pub op: EditOp,
    /// ≤ 140 chars; the bullet text.
    #[serde(default)]
    pub content: String,
    /// Exact text of the bullet to replace / delete / add after; `None` = append.
    #[serde(default)]
    pub anchor: Option<String>,
    /// The proposer's reason (never shown to the judge).
    #[serde(default)]
    pub rationale: String,
    /// Case ids it closes.
    #[serde(default)]
    pub cases: Vec<String>,
    /// The gate's numbers as the server computed them from `replays.jsonl`
    /// (136 D2); `None` before the record pass. The caller's value is advisory.
    #[serde(default)]
    pub gate: Option<GateResult>,
    /// RFC 3339 when applied.
    #[serde(default)]
    pub applied_at: Option<String>,
    /// RFC 3339 when undone.
    #[serde(default)]
    pub undone_at: Option<String>,
}

/// What applying one edit did (136 §7.2 step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// Applied.
    Applied,
    /// The anchor matched nothing.
    SkippedUnmatched,
    /// Refused (cap, length, a human region).
    Error,
}

/// `skill_learned_edit` result: one row per edit, plus the candidate text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ApplyReport {
    /// The target (`skill:<name>` | `preferences`).
    pub target: String,
    /// Per edit, in order: `(edit id, status, note)`.
    pub edits: Vec<(String, ApplyStatus, String)>,
    /// The sidecar text after the edits.
    pub text: String,
    /// `backups/<night>/<skill>.md` written before the edits.
    pub backup: Option<String>,
}

/// Per-phase counts of a night (136 §2.7).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NightCounts {
    /// Fresh entries triaged.
    pub triaged: u64,
    /// Titles shortened.
    pub retitled: u64,
    /// Entries folded.
    pub merged: u64,
    /// Entries superseded.
    pub superseded: u64,
    /// Entries archived.
    pub archived: u64,
    /// Entries verified (ok or stale).
    pub verified: u64,
    /// Cases mined.
    pub cases: u64,
    /// Learned edits accepted.
    pub edits_accepted: u64,
    /// Learned edits rejected.
    pub edits_rejected: u64,
    /// Attention rows proposed.
    pub proposals: u64,
}

/// One night as the UI lists it (136 §2.7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NightSummary {
    /// The night id.
    pub night: NightId,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339; `None` while running.
    pub finished_at: Option<String>,
    /// Counts.
    pub counts: NightCounts,
    /// Money spent.
    pub spend_usd: f64,
    /// `pass` | `backlog` | `nightly`.
    pub mode: String,
    /// Every group of the night was undone.
    pub undone: bool,
    /// How the night closed (136 D7): `None` = the model journaled it;
    /// `budget` = the turn ended on the budget guard; `incomplete` = the turn
    /// ended (error, timeout, early stop) before the journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub status: Option<String>,
    /// The judge ran on the proposer's model (no `agent.escalate_model`, 136 D12).
    #[serde(default)]
    pub judge_same_model: bool,
}

/// What `BeginNight` hands back: the night id and, per knowledge id touched
/// so far, the commit to restore to on undo (136 §3.3 item 5).
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NightReceipt {
    /// The night id.
    pub night: NightId,
    /// `(id, commit_before)`.
    pub knowledge: Vec<(String, String)>,
    /// Skills whose sidecar was backed up.
    pub backups: Vec<String>,
}

/// What a proposal asks (136 §4 phase 6).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// A recurring manual ask → `routine_draft()`.
    Routine,
    /// An always-approved confirmation → a double class / policy change.
    DoubleClass,
    /// A fact re-explained ≥ 2 times → "I'll remember: …".
    #[default]
    Remember,
}

/// One row for the attention inbox (136 §2.7).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Proposal {
    /// Ulid.
    pub id: String,
    /// What it asks.
    pub kind: ProposalKind,
    /// Line one: the thing to say back.
    pub title: String,
    /// The evidence and the cost of being wrong.
    pub body: String,
    /// The option labels, in the user's words.
    pub options: Vec<String>,
    /// RFC 3339.
    pub created_at: String,
}

/// `signals.json` (136 §2.5): what the sensor reads.
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct SignalsSnapshot {
    /// RFC 3339 of the last finished turn of an attended root session.
    pub last_root_turn_at: Option<String>,
    /// Desks with a task `doing`.
    pub desks_doing: u32,
    /// Concepts written since `last_dream_at`.
    pub new_concepts_since: u32,
    /// Sessions that turned since `last_dream_at`.
    pub sessions_since: u32,
    /// RFC 3339 of the last night's end.
    pub last_dream_at: Option<String>,
    /// Mains power; `None` = unknown.
    pub on_ac: Option<bool>,
    /// Battery percent; `None` = unknown / no battery.
    pub battery_pct: Option<u8>,
    /// The engine that writes this file.
    pub engine: String,
    /// RFC 3339 of this write.
    pub written_at: Option<String>,
    /// The engine's local UTC offset in minutes at the time of writing, so the
    /// sensor (UTC clock only) can place `now` inside the local window.
    pub tz_offset_minutes: i32,
}

/// How a manual run proceeds (136 §4.1).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// One pass of the phases.
    #[default]
    Pass,
    /// Repeat passes until nothing is fresh or untidied, or the budget is spent.
    Backlog,
}

/// `dreaming.run_now` (136 §2.4, §2.7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RunRequest {
    /// This run's money; the dialog's default is 5.
    pub budget_usd: f64,
    /// One pass or the backlog.
    #[serde(default)]
    pub mode: RunMode,
    /// This run's time limit; `None` = 4 h in backlog mode, the file's otherwise.
    #[serde(default)]
    pub run_timeout_secs: Option<u64>,
}

/// An edit the gate refused, remembered so it is not re-proposed from the same
/// evidence (136 §0.9; `state.json`, ≤ 200).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RejectedEdit {
    /// The edit's id.
    pub id: String,
    /// The target.
    pub target: String,
    /// The bullet text.
    pub content: String,
    /// The judge's reasons.
    pub reasons: Vec<String>,
    /// The night.
    pub night: NightId,
}

/// `state.json` (136 §2.5).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct DreamState {
    /// The last night that ran.
    pub last_night: Option<NightSummary>,
    /// Nights so far.
    pub nights: u64,
    /// RFC 3339 of the last night's end.
    pub last_dream_at: Option<String>,
    /// ≤ 1,200 chars: which kinds of edits helped.
    pub meta_note: String,
    /// ≤ 200.
    pub rejected: Vec<RejectedEdit>,
    /// A `run_now` waiting for its run.
    pub run_request: Option<RunRequest>,
    /// The engine that owns this state.
    pub engine: String,
    /// The `Dreaming` journal document id.
    pub journal: Option<String>,
    /// RFC 3339 until which the routine is paused.
    pub paused_until: Option<String>,
}

/// What is waiting for tonight (136 §2.7).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Backlog {
    /// `status: fresh` entries.
    pub fresh: u64,
    /// Entries never tidied (fresh or over-long titles).
    pub untidied: u64,
    /// Open cases.
    pub open_cases: u64,
}

/// `dreaming.status` (136 §2.7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DreamStatus {
    /// `dreaming.enabled`.
    pub enabled: bool,
    /// The last night.
    pub last_night: Option<NightSummary>,
    /// RFC 3339 of the next window start.
    pub next_window: Option<String>,
    /// Configured nightly budget.
    pub budget_usd: f64,
    /// What is left tonight.
    pub budget_left_tonight_usd: f64,
    /// Waiting work.
    pub backlog: Backlog,
    /// Proposals waiting in the inbox.
    pub proposals_pending: u64,
    /// RFC 3339 while paused.
    pub paused_until: Option<String>,
}

/// `dreaming.undo` result (136 §3.3 item 5).
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UndoReport {
    /// The night.
    pub night: NightId,
    /// The group, or `None` for the whole night.
    pub group: Option<String>,
    /// Knowledge ids restored.
    pub knowledge_restored: Vec<String>,
    /// Skills whose sidecar was restored from backup.
    pub skills_restored: Vec<String>,
    /// Edit ids marked undone.
    pub edits_undone: Vec<String>,
    /// What could not be undone and why.
    pub errors: Vec<String>,
}

/// `dreaming.settings` / `set_settings` (136 §2.7): the Settings › Dreaming surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DreamSettings {
    /// `dreaming.enabled`.
    pub enabled: bool,
    /// Local `["HH:MM", "HH:MM"]`.
    pub window: [String; 2],
    /// Nightly budget.
    pub budget_usd: f64,
    /// The four tiers.
    pub models: DreamModels,
    /// `preferences.auto_from_explicit`.
    pub preferences_auto: bool,
    /// `"<Day> HH:MM"`.
    pub digest: String,
    /// RFC 3339 while paused.
    pub paused_until: Option<String>,
    /// The models that actually run each tier tonight (136 D12): the config's
    /// pick, else the server-side default. The UI shows these instead of
    /// computing defaults client-side. Ignored on `set_settings`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub resolved: Option<ResolvedModels>,
}

/// The four model tiers as the UI shows them (136 §2.7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DreamModels {
    /// `None` = decide_many only.
    pub triage: Option<String>,
    /// `None` = default.
    pub tidy: Option<String>,
    /// `None` = default.
    pub propose: Option<String>,
    /// `None` = default.
    pub judge: Option<String>,
}

/// What runs each tier (136 D12): `provider/model` strings, never `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "DreamResolvedModels")
)]
pub struct ResolvedModels {
    /// The routine's `model:` (config `tidy`, else `agent.model`).
    pub tidy: String,
    /// v1 = `tidy` (the proposer runs in the night's own turn).
    pub propose: String,
    /// Config `judge`, else `agent.escalate_model` when set and ≠ `propose`, else `propose`.
    pub judge: String,
    /// The replay target: config-free, `agent.model`.
    pub target: String,
}

/// `dreaming.metrics(days)` (136 §13).
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Metrics {
    /// The window.
    pub days: u32,
    /// Opens ÷ impressions.
    pub hint_open_rate: f64,
    /// Share of impressions on ids never opened.
    pub impressions_never_opened_share: f64,
    /// Cases of one intent reopened after a closing edit ÷ closed cases.
    pub repeat_correction_rate: f64,
    /// Live concepts (`hint: true`, status fresh | tidy).
    pub active_concepts: u64,
    /// Median title length, chars.
    pub median_title_chars: u64,
    /// Skill loads per 100 sessions.
    pub skill_loads_per_100_sessions: f64,
    /// Lines in the standing preferences block.
    pub preference_lines: u64,
    /// Money spent by dreaming in the window.
    pub spend_usd: f64,
    /// Nights or groups undone in the window.
    pub undos: u64,
}

/// What `dry_run` prints (136 §3.3 item 7): the counts the journal would show, with zero writes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "DryRunReport"))]
pub struct Report {
    /// `status: fresh` entries.
    pub fresh: u64,
    /// Titles over 120 chars.
    pub retitle_candidates: u64,
    /// Same-stem groups (date / `-vN` suffix).
    pub duplicate_stems: u64,
    /// Records past their lifetime plus ≥ 50 shown / 0 opened non-warnings.
    pub archive_by_use_candidates: u64,
    /// The verification sample size.
    pub verification_sample: u64,
    /// Sessions in the ledger window.
    pub sessions: u64,
    /// Friction events the miner would see.
    pub friction_events: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract §2.6: the JSON shapes of a case and an edit are binding.
    #[test]
    fn case_and_edit_json_match_the_contract() {
        let case = serde_json::json!({
            "id":"01J0","night":"2026-10-08","kind":"re-ask","intent":"…","context":"c","happened":"h",
            "user_words":"w","ideal":"i","tags":["a"],"skill_hint":"prompt:standing-preferences",
            "source_session":"00000000-0000-0000-0000-000000000000","at":"2026-10-07T21:05:00Z",
            "split":"val","origin":"real","state":"open","closed_by":null,"reopened_at":null
        });
        let c: Case = serde_json::from_value(case.clone()).unwrap();
        assert_eq!(c.kind, CaseKind::ReAsk);
        assert_eq!(serde_json::to_value(&c).unwrap(), case);
        for k in [
            "correction",
            "disapproval",
            "rewind",
            "stop",
            "denial",
            "tool-chain",
            "hint-miss",
            "task-returned",
        ] {
            let v: CaseKind = serde_json::from_value(serde_json::json!(k)).unwrap();
            assert_eq!(serde_json::to_value(v).unwrap(), serde_json::json!(k));
        }
        let edit = serde_json::json!({
            "id":"01J1","night":"2026-10-08","target":"skill:harness","op":"add","content":"≤140 chars","anchor":null,
            "rationale":"r","cases":["01J0"],
            "gate":{"baseline":0.42,"candidate":0.71,"min_delta":-0.05,"val_cases":6,"verdict":"accepted"},
            "applied_at":null,"undone_at":null
        });
        let e: EditRecord = serde_json::from_value(edit.clone()).unwrap();
        assert_eq!(e.gate.unwrap().verdict, Verdict::Accepted);
        assert_eq!(serde_json::to_value(&e).unwrap(), edit);
    }

    /// R-3 (D3): the proposer prompt's shape — no `id`, `night`, `target`,
    /// `gate`, timestamps — deserialises; the server assigns the rest.
    #[test]
    fn the_proposer_prompts_edit_shape_parses() {
        let e: EditRecord = serde_json::from_str(
            r#"{"op":"add","content":"x","anchor":null,"rationale":"r","cases":[]}"#,
        )
        .unwrap();
        assert!(e.id.is_empty() && e.night.is_empty() && e.target.is_empty());
        assert!(e.gate.is_none() && e.applied_at.is_none());
        let r: EditRecord = serde_json::from_str(r#"{"op":"retire"}"#).unwrap();
        assert_eq!(r.op, EditOp::Retire);
    }

    #[test]
    fn state_and_signals_default_from_empty_json_and_run_request_defaults_mode() {
        let s: DreamState = serde_json::from_str("{}").unwrap();
        assert_eq!(s, DreamState::default());
        let g: SignalsSnapshot = serde_json::from_str(r#"{"engine":"e"}"#).unwrap();
        assert_eq!(g.engine, "e");
        assert!(g.on_ac.is_none());
        let r: RunRequest = serde_json::from_str(r#"{"budget_usd":5}"#).unwrap();
        assert_eq!(r.mode, RunMode::Pass);
        assert_eq!(serde_json::to_value(RunMode::Backlog).unwrap(), "backlog");
    }
}
