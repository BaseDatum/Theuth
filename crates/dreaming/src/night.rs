//! The `Night` actor (design doc 136 §9.3): owns `<data_dir>/dreaming/`
//! — `state.json`, `ledger/`, `cases.jsonl`, `edits.jsonl`, `backups/`,
//! `proposals.jsonl` and the journal. One record per night lives in
//! `ledger/<night>.night.json` ([`NightRecord`]): the summary the UI lists,
//! the dreaming session, and the undo receipt — every knowledge id dreaming
//! changed that night with the commit to restore to and its group (the
//! knowledge op), gathered from `KnowledgeChanged{source: dreaming}` while
//! the night is open. `BeginNight` opens it (`dreaming_ledger`),
//! `Commit{phase: "journal"}` closes it (`dreaming_journal`), `Undo` reverts
//! it, `RunNow` fires the routine with overrides, `SetSettings` persists the
//! section, `ProposalAct` acts on an inbox row, `MetricsOver` answers §13.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use chrono::{Local, TimeZone, Utc};
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::config::DreamConfig;
use crate::sensor_rules::{local_minutes, parse_hhmm};
use crate::types::{
    Backlog, DreamSettings, DreamState, DreamStatus, Metrics, NightCounts, NightId, NightReceipt,
    NightSummary, Proposal, ProposalKind, RunMode, RunRequest, UndoReport,
};
use crate::{DreamNightFinished, DreamNightStarted, cases, edits, journal, ledger, preferences};

/// Registry name of the [`Night`] actor.
pub const DREAMING_ACTOR: &str = "dreaming";
/// `<data_dir>/dreaming/`.
pub const DIR: &str = "dreaming";
/// `meta_note` cap (136 §2.5).
pub const META_NOTE_CHARS: usize = 1200;
/// Backlog-mode run timeout (136 §3.3 item 6).
pub const BACKLOG_TIMEOUT_SECS: u64 = 4 * 3600;

/// `<data_dir>/dreaming/`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// `<data_dir>/dreaming/state.json`.
pub fn state_path(data_dir: &Path) -> PathBuf {
    dir(data_dir).join("state.json")
}

/// `<data_dir>/dreaming/proposals.jsonl` (waiting rows).
pub fn proposals_path(data_dir: &Path) -> PathBuf {
    dir(data_dir).join("proposals.jsonl")
}

/// `<data_dir>/dreaming/proposals_done.jsonl` (acted rows, for the weekly cap and the metrics).
pub fn proposals_done_path(data_dir: &Path) -> PathBuf {
    dir(data_dir).join("proposals_done.jsonl")
}

/// `state.json`, or the defaults when absent or unreadable (the file may be
/// deleted and rebuilt, 136 §9.6).
pub fn read_state(data_dir: &Path) -> DreamState {
    std::fs::read_to_string(state_path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Write `state.json` atomically (`.tmp` + rename; parents created).
pub fn write_state(data_dir: &Path, state: &DreamState) -> anyhow::Result<()> {
    write_atomic(&state_path(data_dir), &serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

/// `tmp` + rename; parents created.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn append_line<T: Serialize>(path: &Path, row: &T) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut line = serde_json::to_string(row)?;
    line.push('\n');
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())?;
    Ok(())
}

fn read_lines<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// RFC 3339 (UTC seconds) of the next local window start after `now`
/// (136 §2.7 `next_window`); `None` when `window[0]` is malformed.
pub fn next_window(cfg: &DreamConfig, now: chrono::DateTime<Utc>) -> Option<String> {
    let start = parse_hhmm(&cfg.window[0])?;
    let offset = Local
        .timestamp_opt(now.timestamp(), 0)
        .single()?
        .offset()
        .local_minus_utc()
        / 60;
    let local = local_minutes(now, offset);
    let ahead = (start + 1440 - local) % 1440;
    let ahead = if ahead == 0 { 1440 } else { ahead };
    let secs_into_minute = now.timestamp().rem_euclid(60);
    let next = now + chrono::Duration::minutes(i64::from(ahead))
        - chrono::Duration::seconds(secs_into_minute);
    Some(next.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// `dreaming.status` from `state.json` and the config (136 §2.7) with
/// `backlog` as given (the actor computes it from the live store; the file
/// path answers zeros).
pub fn status(data_dir: &Path, cfg: &DreamConfig) -> DreamStatus {
    status_with(data_dir, cfg, Backlog::default())
}

/// [`status`] with a computed backlog.
pub fn status_with(data_dir: &Path, cfg: &DreamConfig, backlog: Backlog) -> DreamStatus {
    let s = read_state(data_dir);
    let today = Local::now().format("%Y-%m-%d").to_string();
    let spent_tonight = s
        .last_night
        .as_ref()
        .filter(|n| n.night == today)
        .map_or(0.0, |n| n.spend_usd);
    let proposals_pending = std::fs::read_to_string(proposals_path(data_dir))
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count() as u64)
        .unwrap_or(0);
    DreamStatus {
        enabled: cfg.enabled,
        last_night: s.last_night,
        next_window: next_window(cfg, Utc::now()),
        budget_usd: cfg.budget_usd,
        budget_left_tonight_usd: (cfg.budget_usd - spent_tonight).max(0.0),
        backlog,
        proposals_pending,
        paused_until: s.paused_until,
    }
}

/// `dreaming.settings`: the Settings › Dreaming view of the config (136 §2.7).
pub fn settings(data_dir: &Path, cfg: &DreamConfig) -> DreamSettings {
    DreamSettings {
        enabled: cfg.enabled,
        window: cfg.window.clone(),
        budget_usd: cfg.budget_usd,
        models: crate::types::DreamModels {
            triage: cfg.models.triage.clone(),
            tidy: cfg.models.tidy.clone(),
            propose: cfg.models.propose.clone(),
            judge: cfg.models.judge.clone(),
        },
        preferences_auto: cfg.preferences.auto_from_explicit,
        digest: cfg.digest.clone(),
        paused_until: read_state(data_dir).paused_until,
    }
}

/// Persist the whole `dreaming:` section of `config.yaml` (the block is
/// replaced by `cfg` serialised; the rest of the file is byte-identical),
/// through a sibling temp file and a rename — the `double::sched::persist`
/// pattern; `runtime::write_config_value` handles one scalar per call.
pub fn persist(path: &Path, cfg: &DreamConfig) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let body = serde_yaml::to_string(cfg).map_err(std::io::Error::other)?;
    let mut block: Vec<String> = vec!["dreaming:".to_owned()];
    block.extend(body.lines().map(|l| format!("  {l}")));
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    match lines.iter().position(|l| l.trim_end() == "dreaming:") {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.starts_with(|c: char| !c.is_whitespace()))
                .map_or(lines.len(), |i| start + 1 + i);
            lines.splice(start..end, block);
        }
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
    let tmp = path.with_extension(format!("yaml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, lines.join("\n") + "\n")?;
    std::fs::rename(tmp, path)
}

/// Carry a settings change into the installed routine file: its `budget:`,
/// `when:` (window / idle / heavy-day args) and `run_timeout:` lines take
/// the new values; every other line — `enabled:` above all — stays.
pub fn update_routine_file(data_dir: &Path, cfg: &DreamConfig, engine: Option<&str>) -> std::io::Result<()> {
    let path = data_dir.join(schedule::DIR).join("dreaming.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let fresh = crate::routine_text(cfg, data_dir, engine);
    let want = |key: &str| {
        fresh
            .lines()
            .find(|l| l.starts_with(key))
            .map(str::to_owned)
    };
    let mut out = Vec::new();
    let mut in_front = false;
    let mut seen = 0;
    for line in text.lines() {
        if line == "---" {
            seen += 1;
            in_front = seen == 1;
            out.push(line.to_owned());
            continue;
        }
        if in_front {
            let key = line.split(':').next().unwrap_or("");
            if matches!(key, "budget" | "when" | "run_timeout")
                && let Some(w) = want(&format!("{key}:"))
            {
                out.push(w);
                continue;
            }
        }
        out.push(line.to_owned());
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    if joined != text {
        write_atomic(&path, joined.as_bytes())?;
    }
    Ok(())
}

/// One knowledge change of a night (the undo receipt).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Touched {
    /// The concept.
    pub id: String,
    /// The commit to restore to; "" when dreaming created the id that night.
    pub commit_before: String,
    /// The undo group: the knowledge op (`merge` | `supersede` | `archive` | `verify` | `retitle` | `put`).
    pub group: String,
    /// Already restored.
    #[serde(default)]
    pub undone: bool,
}

/// `ledger/<night>.night.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NightRecord {
    /// What the UI lists.
    pub summary: NightSummary,
    /// The dreaming session (spend owner; excluded from the ledger).
    #[serde(default)]
    pub session: Option<SessionId>,
    /// Knowledge ids changed, with their pre-night commit and group.
    #[serde(default)]
    pub knowledge: Vec<Touched>,
    /// Skills whose sidecar was backed up (`backups/<night>/<skill>.md`).
    #[serde(default)]
    pub backups: Vec<String>,
    /// Phases committed, in order.
    #[serde(default)]
    pub phases: Vec<String>,
    /// Proposal ids created this night.
    #[serde(default)]
    pub proposals: Vec<String>,
    /// Sidecars already restored by an undo.
    #[serde(default)]
    pub sidecars_undone: Vec<String>,
}

impl NightRecord {
    /// The contract receipt.
    pub fn receipt(&self) -> NightReceipt {
        NightReceipt {
            night: self.summary.night.clone(),
            knowledge: self
                .knowledge
                .iter()
                .map(|t| (t.id.clone(), t.commit_before.clone()))
                .collect(),
            backups: self.backups.clone(),
        }
    }
}

/// `ledger/<night>.night.json`.
pub fn record_path(data_dir: &Path, night: &str) -> PathBuf {
    ledger::dir(data_dir).join(format!("{night}.night.json"))
}

/// Read one night's record.
pub fn read_record(data_dir: &Path, night: &str) -> Option<NightRecord> {
    serde_json::from_str(&std::fs::read_to_string(record_path(data_dir, night)).ok()?).ok()
}

/// Write one night's record.
pub fn write_record(data_dir: &Path, r: &NightRecord) -> anyhow::Result<()> {
    write_atomic(
        &record_path(data_dir, &r.summary.night),
        &serde_json::to_vec_pretty(r)?,
    )?;
    Ok(())
}

/// Every night, newest first.
pub fn nights(data_dir: &Path) -> Vec<NightRecord> {
    let Ok(rd) = std::fs::read_dir(ledger::dir(data_dir)) else {
        return Vec::new();
    };
    let mut v: Vec<NightRecord> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".night.json"))
        })
        .filter_map(|p| serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok())
        .collect();
    v.sort_by(|a, b| b.summary.night.cmp(&a.summary.night));
    v
}

/// The sidecar backups of a night: skill names under `backups/<night>/`.
pub fn backed_up_skills(data_dir: &Path, night: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir(data_dir).join("backups").join(night)) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
        })
        .collect();
    v.sort();
    v
}

/// A proposal as the Suggest phase hands it in (136 §4 phase 6).
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProposalDraft {
    /// `routine` | `double_class` | `remember`.
    pub kind: ProposalKind,
    /// Line one: the thing to say back.
    pub title: String,
    /// Evidence and the cost of being wrong; a `routine` carries the draft
    /// schedule file in a ```markdown fence.
    pub body: String,
    /// Option labels in the user's words.
    #[serde(default)]
    pub options: Vec<String>,
}

impl Default for ProposalKind {
    fn default() -> Self {
        ProposalKind::Remember
    }
}

/// Proposals created in the last 7 days (waiting + acted).
pub fn proposals_this_week(data_dir: &Path, now: chrono::DateTime<Utc>) -> usize {
    let cutoff = now - chrono::Duration::days(7);
    read_lines::<Proposal>(&proposals_path(data_dir))
        .into_iter()
        .chain(read_lines::<Proposal>(&proposals_done_path(data_dir)))
        .filter(|p| {
            chrono::DateTime::parse_from_rfc3339(&p.created_at)
                .is_ok_and(|t| t.with_timezone(&Utc) >= cutoff)
        })
        .count()
}

/// Append proposals (≤ `caps.rows_per_week` per 7 days, the rest dropped
/// with a journal note by the caller); the rows written.
pub fn add_proposals(
    data_dir: &Path,
    cfg: &DreamConfig,
    night: &str,
    drafts: Vec<ProposalDraft>,
) -> anyhow::Result<Vec<Proposal>> {
    let now = Utc::now();
    let mut room = (cfg.caps.rows_per_week as usize).saturating_sub(proposals_this_week(data_dir, now));
    let mut out = Vec::new();
    for d in drafts {
        if room == 0 {
            break;
        }
        room -= 1;
        let p = Proposal {
            id: cases::new_id(),
            kind: d.kind,
            title: crate::scrub::clip(&crate::scrub::scrub(&d.title), 200),
            body: crate::scrub::scrub(&d.body),
            options: d.options.iter().map(|o| crate::scrub::scrub(o)).collect(),
            created_at: cases::now_rfc3339(),
        };
        append_line(&proposals_path(data_dir), &p)?;
        out.push(p);
    }
    if let Some(mut r) = read_record(data_dir, night) {
        r.proposals.extend(out.iter().map(|p| p.id.clone()));
        r.summary.counts.proposals = r.proposals.len() as u64;
        write_record(data_dir, &r)?;
    }
    Ok(out)
}

/// Counts a night can read from its files: cases, edits, proposals.
pub fn file_counts(data_dir: &Path, night: &str, counts: &mut NightCounts) {
    counts.cases = cases::all(data_dir)
        .map(|v| v.iter().filter(|c| c.night == night).count() as u64)
        .unwrap_or(0);
    let e = edits::all(data_dir).unwrap_or_default();
    counts.edits_accepted = e
        .iter()
        .filter(|x| x.night == night && x.gate.as_ref().is_some_and(|g| g.verdict == crate::types::Verdict::Accepted))
        .count() as u64;
    counts.edits_rejected = e
        .iter()
        .filter(|x| x.night == night && x.gate.as_ref().is_some_and(|g| g.verdict == crate::types::Verdict::Rejected))
        .count() as u64;
    if let Some(r) = read_record(data_dir, night) {
        counts.proposals = r.proposals.len() as u64;
    }
}

/// The actor.
#[derive(kameo::Actor)]
pub struct Night {
    /// The data dir.
    pub data_dir: PathBuf,
    /// The section as read at start; `SetSettings` updates it.
    pub cfg: DreamConfig,
    /// For events and `data_dir`.
    pub rt: Runtime,
    /// The engine id (routine file args).
    pub engine: String,
    /// The open night, if any.
    pub active: Option<NightId>,
    /// The knowledge store, resolved on first use.
    knowledge: Option<Arc<knowledge::Knowledge>>,
}

impl Night {
    /// Build one.
    pub fn new(rt: &Runtime, cfg: DreamConfig, engine: &str) -> Self {
        Self {
            data_dir: rt.data_dir.clone(),
            cfg,
            rt: rt.clone(),
            engine: engine.to_owned(),
            active: None,
            knowledge: None,
        }
    }

    async fn knowledge(&mut self) -> Option<Arc<knowledge::Knowledge>> {
        if self.knowledge.is_none()
            && let Ok(Some(k)) =
                ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
            && let Ok(h) = k.ask(knowledge::Handle).await
        {
            self.knowledge = Some(h);
        }
        self.knowledge.clone()
    }

    /// The dreaming session's spend over the night (nanos), from the
    /// agent's ledger (the only seam that reads `model_calls`).
    async fn spend_nanos(&self, r: &NightRecord) -> u64 {
        let (Some(session), Ok(Some(agents))) = (
            r.session,
            ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR),
        ) else {
            return 0;
        };
        let mut ids = vec![session];
        if let Ok(store) = agents.ask(agent::DurableStore).await
            && let Ok(kids) = store.descendants(session).await
        {
            ids.extend(kids.into_iter().map(|(_, k)| k.id));
        }
        match agents
            .ask(agent::LedgerSince {
                since: r.summary.started_at.clone(),
                until: cases::now_rfc3339(),
                exclude: Vec::new(),
                owner_only: false,
            })
            .await
        {
            Ok(l) => l
                .sessions
                .iter()
                .filter(|s| ids.contains(&s.id))
                .map(|s| s.model_cost_nanos)
                .sum(),
            _ => 0,
        }
    }

    async fn finish(&mut self, mut r: NightRecord) -> anyhow::Result<NightRecord> {
        let night = r.summary.night.clone();
        file_counts(&self.data_dir, &night, &mut r.summary.counts);
        r.backups = backed_up_skills(&self.data_dir, &night);
        let spend = self.spend_nanos(&r).await;
        r.summary.spend_usd = spend as f64 / 1e9;
        let now = cases::now_rfc3339();
        r.summary.finished_at = Some(now.clone());
        write_record(&self.data_dir, &r)?;
        let mut state = read_state(&self.data_dir);
        state.last_night = Some(r.summary.clone());
        state.nights += 1;
        state.last_dream_at = Some(now.clone());
        state.run_request = None;
        write_state(&self.data_dir, &state)?;
        if let Err(e) = cases::settle_old(&self.data_dir, Utc::now()) {
            tracing::debug!("dreaming: settle: {e}");
        }
        if let Ok(Some(s)) =
            ActorRef::<crate::signals::Signals>::lookup(crate::signals::SIGNALS_ACTOR)
        {
            let _ = s.tell(crate::signals::Dreamt(now)).await;
        }
        let proposals: Vec<Proposal> = read_lines::<Proposal>(&proposals_path(&self.data_dir))
            .into_iter()
            .filter(|p| r.proposals.contains(&p.id))
            .collect();
        self.rt
            .publish(DreamNightFinished {
                night: night.clone(),
                counts: r.summary.counts,
                spend_nanos: spend,
                proposals,
            })
            .await;
        self.active = None;
        Ok(r)
    }
}

/// `ask` → `Result<NightReceipt, String>` (136 §9.3): open tonight's night
/// (idempotent; a second window the same night reopens the record).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BeginNight {
    /// The night id (local date).
    pub night: NightId,
    /// `pass` | `backlog` | `nightly`.
    pub mode: String,
    /// The dreaming session (spend owner).
    #[serde(default)]
    pub session: Option<SessionId>,
}

/// `ask` → `Result<(), String>` (136 §9.3): a phase finished; `journal`
/// closes the night (counts, spend, `last_dream_at`, `Dreamt`, the event).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Commit {
    /// The night id.
    pub night: NightId,
    /// `tidy` | `mine` | `rehearse` | `settle` | `suggest` | `journal`.
    pub phase: String,
    /// One line for the record.
    pub summary: String,
}

/// `ask` → `Result<UndoReport, String>` (136 §3.3 item 5; also `dreaming.undo`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Undo {
    /// The night.
    pub night: NightId,
    /// A group of the night, or `None` for all of it.
    pub group: Option<String>,
}

/// `ask` → `Result<PathBuf, String>` (136 §6): export cases in the 57
/// training shapes (`laya` | `fixture`) under `<data_dir>/dreaming/export/`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    /// `laya` | `fixture`.
    pub format: String,
}

/// `ask` → `Result<SessionId, String>` (136 §2.7 `dreaming.run_now`): record
/// the request in `state.json` and ask the schedule to `FireWith` the
/// routine (budget override; a 4 h run timeout in backlog mode).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunNow(pub RunRequest);

/// `ask` → `DreamStatus` (136 §2.7).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Vec<NightSummary>` newest first, ≤ `limit` (136 §2.7).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Nights {
    /// Row cap.
    pub limit: u32,
}

/// `ask` → `DreamSettings` (136 §2.7).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Settings;

/// `ask` → `Result<DreamSettings, String>` (136 §2.7): persist the section
/// through `config.yaml`, the routine file and `state.json`, and apply it live.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetSettings(pub DreamSettings);

/// `ask` → `Vec<Proposal>` (136 §2.7): the waiting rows.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Proposals;

/// `ask` → `Result<(), String>` (136 §2.7): the user pressed `choice`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProposalAct {
    /// Proposal id.
    pub id: String,
    /// The option label pressed.
    pub choice: String,
}

/// `ask` → `Result<Metrics, String>` (136 §13).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MetricsOver {
    /// The window, days.
    pub days: u32,
}

/// `ask` → `Result<Vec<Proposal>, String>`: the Suggest phase's rows (via
/// `dreaming_journal{section: "proposals"}`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Propose {
    /// The night.
    pub night: NightId,
    /// The drafts.
    pub drafts: Vec<ProposalDraft>,
}

/// `ask` → `Result<String, String>`: replace the meta-note (≤ 1,200 chars);
/// answers the previous note (via `dreaming_journal{section: "meta-note"}`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetMetaNote(pub String);

impl Message<BeginNight> for Night {
    type Reply = Result<NightReceipt, String>;

    async fn handle(&mut self, m: BeginNight, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let state = read_state(&self.data_dir);
        if let Some(until) = state.paused_until.as_deref()
            && chrono::DateTime::parse_from_rfc3339(until)
                .is_ok_and(|t| t.with_timezone(&Utc) > Utc::now())
        {
            return Err(format!("paused until {until}"));
        }
        let now = cases::now_rfc3339();
        let mut r = match read_record(&self.data_dir, &m.night) {
            Some(mut r) => {
                if r.summary.finished_at.is_some() {
                    // A second window the same night (heavy day): reopen, keep the counts.
                    r.summary.finished_at = None;
                    r.summary.mode = m.mode.clone();
                }
                r
            }
            None => NightRecord {
                summary: NightSummary {
                    night: m.night.clone(),
                    started_at: now,
                    finished_at: None,
                    counts: NightCounts::default(),
                    spend_usd: 0.0,
                    mode: m.mode.clone(),
                    undone: false,
                },
                session: None,
                knowledge: Vec::new(),
                backups: Vec::new(),
                phases: Vec::new(),
                proposals: Vec::new(),
                sidecars_undone: Vec::new(),
            },
        };
        if m.session.is_some() {
            r.session = m.session;
        }
        let fresh = self.active.as_deref() != Some(m.night.as_str());
        write_record(&self.data_dir, &r).map_err(|e| e.to_string())?;
        let mut state = state;
        state.last_night = Some(r.summary.clone());
        write_state(&self.data_dir, &state).map_err(|e| e.to_string())?;
        self.active = Some(m.night.clone());
        if fresh {
            self.rt
                .publish(DreamNightStarted {
                    night: m.night.clone(),
                })
                .await;
        }
        Ok(r.receipt())
    }
}

impl Message<Commit> for Night {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Commit, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut r = read_record(&self.data_dir, &m.night)
            .ok_or_else(|| format!("no night {} (call dreaming_ledger first)", m.night))?;
        r.phases.push(format!("{}: {}", m.phase, crate::scrub::clip(&m.summary, 200)));
        if m.phase == "journal" {
            self.finish(r).await.map_err(|e| e.to_string())?;
        } else {
            write_record(&self.data_dir, &r).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Message<knowledge::KnowledgeChanged> for Night {
    type Reply = ();

    async fn handle(&mut self, e: knowledge::KnowledgeChanged, _: &mut Context<Self, Self::Reply>) {
        if e.source != knowledge::Source::Dreaming {
            return;
        }
        let Some(night) = self.active.clone() else {
            return;
        };
        let Some(mut r) = read_record(&self.data_dir, &night) else {
            return;
        };
        let group = match e.op {
            knowledge::KnowledgeOp::Merge => "merge",
            knowledge::KnowledgeOp::Supersede => "supersede",
            knowledge::KnowledgeOp::Archive => "archive",
            knowledge::KnowledgeOp::Verify => "verify",
            knowledge::KnowledgeOp::Retitle => "retitle",
            knowledge::KnowledgeOp::Put => "put",
            knowledge::KnowledgeOp::Delete => return,
        };
        match e.op {
            knowledge::KnowledgeOp::Merge => r.summary.counts.merged += 1,
            knowledge::KnowledgeOp::Supersede => r.summary.counts.superseded += 1,
            knowledge::KnowledgeOp::Archive => r.summary.counts.archived += 1,
            knowledge::KnowledgeOp::Verify => r.summary.counts.verified += 1,
            knowledge::KnowledgeOp::Retitle => r.summary.counts.retitled += 1,
            knowledge::KnowledgeOp::Put => r.summary.counts.triaged += 1,
            knowledge::KnowledgeOp::Delete => {}
        }
        if !r.knowledge.iter().any(|t| t.id == e.id) {
            let commit_before = match self.knowledge().await {
                Some(k) => match k.history(Some(&e.id), 2).await {
                    Ok(h) => h.changes.get(1).map(|c| c.commit.clone()).unwrap_or_default(),
                    Err(err) => {
                        tracing::debug!("dreaming: history of {}: {err}", e.id);
                        String::new()
                    }
                },
                None => String::new(),
            };
            r.knowledge.push(Touched {
                id: e.id.clone(),
                commit_before,
                group: group.to_owned(),
                undone: false,
            });
        }
        if let Err(err) = write_record(&self.data_dir, &r) {
            tracing::warn!("dreaming: night record not written: {err}");
        }
    }
}

impl Message<Undo> for Night {
    type Reply = Result<UndoReport, String>;

    async fn handle(&mut self, m: Undo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let k = self.knowledge().await;
        crate::undo::undo(
            &self.data_dir,
            k.as_deref(),
            Some(&self.rt),
            &m.night,
            m.group.as_deref(),
        )
        .await
        .map_err(|e| format!("{e:#}"))
    }
}

impl Message<Export> for Night {
    type Reply = Result<PathBuf, String>;

    async fn handle(&mut self, m: Export, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let cases = cases::all(&self.data_dir).map_err(|e| e.to_string())?;
        let rows: Vec<serde_json::Value> = match m.format.as_str() {
            // laya `typed-decisions`-style rows: state, question, gold, label.
            "laya" => cases
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id, "workflow": "dreaming", "state": c.context,
                        "question": c.intent, "gold": c.ideal, "label": c.kind,
                        "split": c.split, "skill_hint": c.skill_hint,
                    })
                })
                .collect(),
            // The bench fixture shape: one batch per case with the user's words as the published turn.
            "fixture" => cases
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id, "source": "dreaming", "kind": c.kind, "state": c.context,
                        "published": [{"title": c.user_words, "cached": c.happened}],
                        "expected": c.ideal, "origin": c.origin, "split": c.split,
                    })
                })
                .collect(),
            other => return Err(format!("unknown format {other:?}: laya | fixture")),
        };
        let doc = serde_json::json!({
            "_doc": "136 §6 cases export; design-57 training shape",
            "format": m.format, "exported_at": cases::now_rfc3339(), "rows": rows,
        });
        let path = dir(&self.data_dir)
            .join("export")
            .join(format!("cases.{}.json", m.format));
        write_atomic(&path, &serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Ok(path)
    }
}

impl Message<RunNow> for Night {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, RunNow(req): RunNow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !(req.budget_usd > 0.0) || !req.budget_usd.is_finite() {
            return Err("budget_usd must be a positive number".into());
        }
        let mut state = read_state(&self.data_dir);
        state.run_request = Some(req.clone());
        write_state(&self.data_dir, &state).map_err(|e| e.to_string())?;
        let sched = ActorRef::<schedule::Schedule>::lookup(schedule::SCHEDULE_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| "schedule is not running".to_owned())?;
        let backlog = req.mode == RunMode::Backlog;
        let overrides = schedule::FireOverrides {
            budget_nanos: Some((req.budget_usd * 1e9).round() as u64),
            run_timeout_secs: req
                .run_timeout_secs
                .or(backlog.then_some(BACKLOG_TIMEOUT_SECS)),
            payload: Some(format!(
                "mode: {}\nbudget_usd: {}",
                if backlog { "backlog" } else { "pass" },
                req.budget_usd
            )),
        };
        sched
            .ask(schedule::FireWith {
                name: "dreaming".into(),
                overrides,
            })
            .await
            .map_err(|e| e.to_string())
    }
}

impl Message<Status> for Night {
    type Reply = DreamStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let k = self.knowledge().await;
        let (_, backlog) = ledger::candidates(&self.data_dir, &self.cfg, k.as_deref()).await;
        status_with(&self.data_dir, &self.cfg, backlog)
    }
}

impl Message<Nights> for Night {
    type Reply = Vec<NightSummary>;

    async fn handle(&mut self, m: Nights, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        nights(&self.data_dir)
            .into_iter()
            .map(|r| r.summary)
            .take(m.limit as usize)
            .collect()
    }
}

impl Message<Settings> for Night {
    type Reply = DreamSettings;

    async fn handle(&mut self, _: Settings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        settings(&self.data_dir, &self.cfg)
    }
}

impl Message<SetSettings> for Night {
    type Reply = Result<DreamSettings, String>;

    async fn handle(&mut self, SetSettings(s): SetSettings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !(s.budget_usd >= 0.0) || !s.budget_usd.is_finite() {
            return Err("budget_usd must be a non-negative number".into());
        }
        if parse_hhmm(&s.window[0]).is_none() || parse_hhmm(&s.window[1]).is_none() {
            return Err("window: two HH:MM times".into());
        }
        let mut cfg = self.cfg.clone();
        cfg.enabled = s.enabled;
        cfg.window = s.window.clone();
        cfg.budget_usd = s.budget_usd;
        cfg.models.triage = s.models.triage.clone().filter(|m| !m.is_empty());
        cfg.models.tidy = s.models.tidy.clone().filter(|m| !m.is_empty());
        cfg.models.propose = s.models.propose.clone().filter(|m| !m.is_empty());
        cfg.models.judge = s.models.judge.clone().filter(|m| !m.is_empty());
        cfg.preferences.auto_from_explicit = s.preferences_auto;
        cfg.digest = s.digest.clone();
        persist(&self.data_dir.join("config.yaml"), &cfg).map_err(|e| format!("config.yaml: {e}"))?;
        let home = cfg.home_engine.clone().filter(|h| !h.is_empty());
        if let Err(e) = update_routine_file(&self.data_dir, &cfg, home.as_deref()) {
            tracing::warn!("dreaming: routine file not updated: {e}");
        }
        let mut state = read_state(&self.data_dir);
        state.paused_until = s.paused_until.clone();
        write_state(&self.data_dir, &state).map_err(|e| e.to_string())?;
        self.cfg = cfg;
        Ok(settings(&self.data_dir, &self.cfg))
    }
}

impl Message<Proposals> for Night {
    type Reply = Vec<Proposal>;

    async fn handle(&mut self, _: Proposals, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        read_lines(&proposals_path(&self.data_dir))
    }
}

impl Message<Propose> for Night {
    type Reply = Result<Vec<Proposal>, String>;

    async fn handle(&mut self, m: Propose, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        add_proposals(&self.data_dir, &self.cfg, &m.night, m.drafts).map_err(|e| e.to_string())
    }
}

impl Message<SetMetaNote> for Night {
    type Reply = Result<String, String>;

    async fn handle(&mut self, SetMetaNote(note): SetMetaNote, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut state = read_state(&self.data_dir);
        let old = std::mem::take(&mut state.meta_note);
        state.meta_note = crate::scrub::clip(&crate::scrub::scrub(note.trim()), META_NOTE_CHARS);
        write_state(&self.data_dir, &state).map_err(|e| e.to_string())?;
        Ok(old)
    }
}

/// What pressing an option means (136 §4 phase 6).
fn choice_is_no(choice: &str) -> bool {
    let c = choice.to_lowercase();
    c.contains("not ") || c.starts_with("no") || c.contains("never") || c.contains("dismiss")
}

impl Message<ProposalAct> for Night {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ProposalAct, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let rows: Vec<Proposal> = read_lines(&proposals_path(&self.data_dir));
        let Some(p) = rows.iter().find(|p| p.id == m.id).cloned() else {
            return Err(format!("no proposal {}", m.id));
        };
        let rest: Vec<&Proposal> = rows.iter().filter(|r| r.id != m.id).collect();
        let text = rest
            .iter()
            .map(|r| serde_json::to_string(r).unwrap_or_default() + "\n")
            .collect::<String>();
        write_atomic(&proposals_path(&self.data_dir), text.as_bytes()).map_err(|e| e.to_string())?;
        append_line(&proposals_done_path(&self.data_dir), &p).map_err(|e| e.to_string())?;
        let night = ledger::tonight();
        let mut note = format!("- {:?} → **{}**", p.title, m.choice);
        if choice_is_no(&m.choice) {
            if m.choice.to_lowercase().contains("never") {
                let mut state = read_state(&self.data_dir);
                state.rejected.push(crate::types::RejectedEdit {
                    id: p.id.clone(),
                    target: format!("proposal:{}", serde_json::to_string(&p.kind).unwrap_or_default().trim_matches('"')),
                    content: p.title.clone(),
                    reasons: vec![format!("user: {}", m.choice)],
                    night: night.clone(),
                });
                state.rejected.truncate(edits::MAX_REJECTED);
                write_state(&self.data_dir, &state).map_err(|e| e.to_string())?;
                note.push_str(" (never again)");
            }
        } else {
            match p.kind {
                ProposalKind::Routine => {
                    let draft = fenced(&p.body).ok_or("the proposal carries no ```markdown schedule file")?;
                    let name = draft
                        .lines()
                        .find_map(|l| l.strip_prefix("name:"))
                        .map(|n| n.trim().to_owned())
                        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                        .ok_or("the draft has no valid `name:`")?;
                    let start = m.choice.to_lowercase().contains("start");
                    let mut lines: Vec<String> = draft
                        .lines()
                        .filter(|l| !l.starts_with("enabled:"))
                        .map(str::to_owned)
                        .collect();
                    if let Some(i) = lines.iter().position(|l| l == "---") {
                        lines.insert(i + 1, format!("enabled: {start}"));
                    }
                    let path = self.data_dir.join(schedule::DIR).join(format!("{name}.md"));
                    if path.exists() {
                        return Err(format!("a schedule named {name} already exists"));
                    }
                    write_atomic(&path, (lines.join("\n") + "\n").as_bytes()).map_err(|e| e.to_string())?;
                    note.push_str(&format!(" — `schedules/{name}.md` written, enabled: {start}"));
                }
                ProposalKind::Remember => {
                    let k = self.knowledge().await.ok_or("knowledge is not running")?;
                    let text = p
                        .title
                        .trim_start_matches("I'll remember:")
                        .trim()
                        .trim_matches('"')
                        .to_owned();
                    let slug: String = text
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect::<String>()
                        .split('-')
                        .filter(|s| !s.is_empty())
                        .take(8)
                        .collect::<Vec<_>>()
                        .join("-");
                    k.put(
                        knowledge::Put {
                            id: format!("decisions/{slug}"),
                            type_: "decision".into(),
                            title: crate::scrub::clip(&text, 120),
                            tags: vec!["dreaming".into()],
                            body: format!("{text}\n\n{}", p.body),
                            evidence: vec!["user".into()],
                            status: Some(knowledge::Status::Tidy),
                            ..Default::default()
                        },
                        knowledge::Source::Dreaming,
                    )
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                    note.push_str(&format!(" — remembered as `decisions/{slug}`"));
                }
                ProposalKind::DoubleClass => {
                    note.push_str(
                        " — to put this into effect use the double's 126 path: `double_policy` / Settings › Double (dreaming never changes approval policy itself)",
                    );
                }
            }
        }
        journal::append(&self.data_dir, &night, "Proposals acted", &note)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The first fenced block's body.
fn fenced(body: &str) -> Option<String> {
    let start = body.find("```")?;
    let after = &body[start + 3..];
    let nl = after.find('\n')?;
    let rest = &after[nl + 1..];
    let end = rest.find("```")?;
    Some(rest[..end].trim_end().to_owned())
}

impl Message<MetricsOver> for Night {
    type Reply = Result<Metrics, String>;

    async fn handle(&mut self, m: MetricsOver, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let days = m.days.max(1);
        let now = Utc::now();
        let since = now - chrono::Duration::days(i64::from(days));
        let since_s = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut out = Metrics {
            days,
            ..Default::default()
        };
        if let Some(k) = self.knowledge().await
            && let Ok(metas) = k.list(None, None, None, ledger::LIST_LIMIT).await
        {
            let live: Vec<&knowledge::Meta> = metas.iter().filter(|x| x.status.is_live()).collect();
            out.active_concepts = live.len() as u64;
            let mut lens: Vec<usize> = live.iter().map(|x| x.title.chars().count()).collect();
            lens.sort_unstable();
            out.median_title_chars = lens.get(lens.len() / 2).copied().unwrap_or(0) as u64;
            let ids: Vec<String> = metas.iter().map(|x| x.id.clone()).collect();
            if let Ok(uses) = k.stats(&ids).await {
                let shown: u64 = uses.iter().map(|u| u.shown).sum();
                let opened: u64 = uses.iter().map(|u| u.opened).sum();
                let never: u64 = uses.iter().filter(|u| u.opened == 0).map(|u| u.shown).sum();
                if shown > 0 {
                    out.hint_open_rate = opened as f64 / shown as f64;
                    out.impressions_never_opened_share = never as f64 / shown as f64;
                }
            }
        }
        if let Ok(cases) = cases::all(&self.data_dir) {
            let in_window = |c: &crate::types::Case| {
                chrono::DateTime::parse_from_rfc3339(&c.at).is_ok_and(|t| t.with_timezone(&Utc) >= since)
            };
            let closed = cases.iter().filter(|c| in_window(c) && c.closed_by.is_some()).count();
            let reopened = cases.iter().filter(|c| in_window(c) && c.reopened_at.is_some()).count();
            if closed > 0 {
                out.repeat_correction_rate = reopened as f64 / closed as f64;
            }
        }
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(l) = agents
                .ask(agent::LedgerSince {
                    since: since_s.clone(),
                    until: now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    exclude: Vec::new(),
                    owner_only: true,
                })
                .await
            && !l.sessions.is_empty()
        {
            let loads: u64 = l.skill_loads.iter().map(|c| c.calls).sum();
            out.skill_loads_per_100_sessions = loads as f64 * 100.0 / l.sessions.len() as f64;
        }
        out.preference_lines = preferences::count(&self.data_dir);
        let cutoff = since.format("%Y-%m-%d").to_string();
        for r in nights(&self.data_dir) {
            if r.summary.night < cutoff {
                continue;
            }
            out.spend_usd += r.summary.spend_usd;
            if r.summary.undone || r.knowledge.iter().any(|t| t.undone) {
                out.undos += 1;
            }
        }
        Ok(out)
    }
}

/// Spawn, register and subscribe the actor; the `engine` is stamped into
/// `state.json` once so the sensor and `home_engine` have something to compare.
pub fn start(rt: &Runtime, cfg: DreamConfig, engine: &str) -> anyhow::Result<()> {
    let mut state = read_state(&rt.data_dir);
    if state.engine.is_empty() {
        state.engine = engine.to_owned();
        write_state(&rt.data_dir, &state).context("write dreaming/state.json")?;
    }
    let actor = Night::spawn_with_mailbox(Night::new(rt, cfg, engine), runtime::subscriber_mailbox());
    actor.register(DREAMING_ACTOR)?;
    let rt2 = rt.clone();
    tokio::spawn(async move {
        rt2.subscribe::<Night, knowledge::KnowledgeChanged>(&actor)
            .await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_status_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let st = status(dir.path(), &cfg);
        assert!(st.enabled && st.last_night.is_none() && st.proposals_pending == 0);
        assert_eq!(st.budget_usd, 1.0);
        assert_eq!(st.budget_left_tonight_usd, 1.0);
        assert!(
            st.next_window
                .as_deref()
                .is_some_and(|w| w.ends_with(":00Z"))
        );

        let today = Local::now().format("%Y-%m-%d").to_string();
        let night = NightSummary {
            night: today,
            started_at: "2026-10-08T01:00:00Z".into(),
            finished_at: Some("2026-10-08T01:40:00Z".into()),
            counts: Default::default(),
            spend_usd: 0.4,
            mode: "nightly".into(),
            undone: false,
        };
        let state = DreamState {
            last_night: Some(night.clone()),
            nights: 1,
            paused_until: Some("2026-10-15T00:00:00Z".into()),
            engine: "e".into(),
            ..Default::default()
        };
        write_state(dir.path(), &state).unwrap();
        assert_eq!(read_state(dir.path()), state);
        let st = status(dir.path(), &cfg);
        assert_eq!(st.last_night, Some(night));
        assert!((st.budget_left_tonight_usd - 0.6).abs() < 1e-9);
        assert_eq!(st.paused_until.as_deref(), Some("2026-10-15T00:00:00Z"));
        // A corrupt file is the defaults, not an error.
        std::fs::write(state_path(dir.path()), "{not json").unwrap();
        assert_eq!(read_state(dir.path()), DreamState::default());
        // Proposals pending = non-empty lines.
        std::fs::write(proposals_path(dir.path()), "{}\n\n{}\n").unwrap();
        assert_eq!(status(dir.path(), &cfg).proposals_pending, 2);
    }

    #[test]
    fn next_window_is_strictly_ahead_and_within_a_day() {
        let cfg = DreamConfig::default();
        let now = Utc::now();
        let next = next_window(&cfg, now).unwrap();
        let t = chrono::DateTime::parse_from_rfc3339(&next)
            .unwrap()
            .with_timezone(&Utc);
        let ahead = t.signed_duration_since(now);
        assert!(
            ahead > chrono::Duration::zero() && ahead <= chrono::Duration::hours(24),
            "{next}"
        );
        assert_eq!(
            local_minutes(
                t,
                Local
                    .timestamp_opt(t.timestamp(), 0)
                    .single()
                    .unwrap()
                    .offset()
                    .local_minus_utc()
                    / 60
            ),
            60
        );
        let bad = DreamConfig {
            window: ["x".into(), "06:00".into()],
            ..cfg
        };
        assert_eq!(next_window(&bad, now), None);
    }

    #[test]
    fn persist_replaces_only_the_dreaming_block_and_the_routine_file_keeps_its_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.yaml");
        std::fs::write(&cfg_path, "agent:\n  model: x\ndreaming:\n  budget_usd: 1\n  window: [\"01:00\", \"06:00\"]\nschedule:\n  enabled: true\n").unwrap();
        let cfg = DreamConfig {
            budget_usd: 2.5,
            window: ["23:00".into(), "05:00".into()],
            ..DreamConfig::default()
        };
        persist(&cfg_path, &cfg).unwrap();
        let text = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(text.starts_with("agent:\n  model: x\ndreaming:\n"), "{text}");
        assert!(text.ends_with("schedule:\n  enabled: true\n"), "{text}");
        assert!(text.contains("  budget_usd: 2.5\n"), "{text}");
        let back: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let section: DreamConfig = serde_yaml::from_value(back["dreaming"].clone()).unwrap();
        assert_eq!(section, cfg);
        // The routine file: budget / when / run_timeout follow; `enabled: false` survives.
        crate::install_files(dir.path(), &DreamConfig::default(), None).unwrap();
        let routine = dir.path().join("schedules/dreaming.md");
        let toggled = std::fs::read_to_string(&routine)
            .unwrap()
            .replace("---\nevery", "---\nenabled: false\nevery");
        std::fs::write(&routine, &toggled).unwrap();
        update_routine_file(dir.path(), &cfg, None).unwrap();
        let after = std::fs::read_to_string(&routine).unwrap();
        assert!(after.contains("enabled: false\n"), "{after}");
        assert!(after.contains("budget: 2.5 USD\n"), "{after}");
        assert!(after.contains("when: dreaming.py window=23:00-05:00 idle=20"), "{after}");
        let spec = schedule::file::parse("dreaming", &after).unwrap();
        assert_eq!(spec.budget.unwrap().nanos, 2_500_000_000);
        assert!(!spec.enabled);
    }

    #[test]
    fn night_records_list_newest_first_and_the_receipt_is_the_contract_shape() {
        let dir = tempfile::tempdir().unwrap();
        for (n, id) in [("2026-10-07", "a"), ("2026-10-08", "b")] {
            write_record(
                dir.path(),
                &NightRecord {
                    summary: NightSummary {
                        night: n.into(),
                        started_at: String::new(),
                        finished_at: None,
                        counts: NightCounts::default(),
                        spend_usd: 0.0,
                        mode: "nightly".into(),
                        undone: false,
                    },
                    session: None,
                    knowledge: vec![Touched {
                        id: format!("facts/{id}"),
                        commit_before: "abc1234".into(),
                        group: "archive".into(),
                        undone: false,
                    }],
                    backups: vec!["harness".into()],
                    phases: vec![],
                    proposals: vec![],
                    sidecars_undone: vec![],
                },
            )
            .unwrap();
        }
        let v = nights(dir.path());
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].summary.night, "2026-10-08");
        assert_eq!(
            v[0].receipt(),
            NightReceipt {
                night: "2026-10-08".into(),
                knowledge: vec![("facts/b".into(), "abc1234".into())],
                backups: vec!["harness".into()],
            }
        );
        assert_eq!(fenced("text\n```markdown\n---\nname: x\n---\nbody\n```\ntail").as_deref(), Some("---\nname: x\n---\nbody"));
        assert!(choice_is_no("Not this") && choice_is_no("never save this") && !choice_is_no("Create and start"));
    }

    #[test]
    fn proposals_respect_the_weekly_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let draft = |t: &str| ProposalDraft {
            kind: ProposalKind::Remember,
            title: t.into(),
            body: "because".into(),
            options: vec!["confirm".into(), "not that".into()],
        };
        let rows = add_proposals(dir.path(), &cfg, "2026-10-08", vec![draft("a"), draft("b")]).unwrap();
        assert_eq!(rows.len(), 2);
        let rows = add_proposals(dir.path(), &cfg, "2026-10-08", vec![draft("c"), draft("d")]).unwrap();
        assert_eq!(rows.len(), 1, "3 per 7 days");
        assert_eq!(proposals_this_week(dir.path(), Utc::now()), 3);
        assert_eq!(read_lines::<Proposal>(&proposals_path(dir.path())).len(), 3);
    }
}
