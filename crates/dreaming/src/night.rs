//! The `Night` actor (design doc 136 §9.3): owns `<data_dir>/dreaming/`
//! — `state.json`, `ledger/`, `cases.jsonl`, `edits.jsonl`, `backups/`,
//! `proposals.jsonl` and the journal document id. Wave C: the files' paths,
//! `state.json` read/write, a real `dreaming.status`, and stub handlers that
//! answer `not built: W3`. 136 W3 fills `BeginNight` / `Commit` / `Undo` /
//! `Export` / `RunNow` and the proposal and metrics asks.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::{Local, TimeZone, Utc};
use kameo::actor::Spawn as _;
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::config::DreamConfig;
use crate::sensor_rules::{local_minutes, parse_hhmm};
use crate::types::{
    Backlog, DreamSettings, DreamState, DreamStatus, Metrics, NightId, NightReceipt, NightSummary,
    Proposal, RunRequest, UndoReport,
};

/// Registry name of the [`Night`] actor.
pub const DREAMING_ACTOR: &str = "dreaming";
/// `<data_dir>/dreaming/`.
pub const DIR: &str = "dreaming";

/// `<data_dir>/dreaming/`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// `<data_dir>/dreaming/state.json`.
pub fn state_path(data_dir: &Path) -> PathBuf {
    dir(data_dir).join("state.json")
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

/// `dreaming.status` from `state.json` and the config (136 §2.7). Real in
/// wave C except `backlog` (zeros; 136 W3 counts fresh / untidied / open cases).
pub fn status(data_dir: &Path, cfg: &DreamConfig) -> DreamStatus {
    let s = read_state(data_dir);
    let today = Local::now().format("%Y-%m-%d").to_string();
    let spent_tonight = s
        .last_night
        .as_ref()
        .filter(|n| n.night == today)
        .map_or(0.0, |n| n.spend_usd);
    let proposals_pending = std::fs::read_to_string(dir(data_dir).join("proposals.jsonl"))
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count() as u64)
        .unwrap_or(0);
    DreamStatus {
        enabled: cfg.enabled,
        last_night: s.last_night,
        next_window: next_window(cfg, Utc::now()),
        budget_usd: cfg.budget_usd,
        budget_left_tonight_usd: (cfg.budget_usd - spent_tonight).max(0.0),
        backlog: Backlog::default(), // 136 W3
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

/// The actor.
#[derive(kameo::Actor)]
pub struct Night {
    /// The data dir.
    pub data_dir: PathBuf,
    /// The section as read at start (136 W3: `SetSettings` updates it).
    pub cfg: DreamConfig,
    /// For events and `data_dir`.
    pub rt: Runtime,
}

impl Night {
    /// Build one.
    pub fn new(rt: &Runtime, cfg: DreamConfig) -> Self {
        Self {
            data_dir: rt.data_dir.clone(),
            cfg,
            rt: rt.clone(),
        }
    }
}

/// `ask` → `Result<NightReceipt, String>` (136 §9.3): open tonight's night —
/// `ledger/`, `backups/<night>/`, the pre-night commit per id (136 W3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BeginNight {
    /// The night id (local date).
    pub night: NightId,
    /// `pass` | `backlog` | `nightly`.
    pub mode: String,
}

/// `ask` → `Result<(), String>` (136 §9.3): a phase finished; the knowledge
/// bundle is committed and the journal section appended (136 W3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Commit {
    /// The night id.
    pub night: NightId,
    /// `tidy` | `mine` | `rehearse` | `settle` | `suggest` | `journal`.
    pub phase: String,
    /// One line for the journal.
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

/// `ask` → `Result<PathBuf, String>` (136 §6): export cases in the 57 training
/// format (`laya` | `fixture`) under `<data_dir>/dreaming/export/` (136 W3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    /// `laya` | `fixture`.
    pub format: String,
}

/// `ask` → `Result<SessionId, String>` (136 §2.7 `dreaming.run_now`): record
/// the request in `state.json` and ask the schedule to `FireWith` the
/// routine (budget override; a 4 h run timeout in backlog mode) (136 W3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunNow(pub RunRequest);

/// `ask` → `DreamStatus` (136 §2.7).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Status;

/// `ask` → `Vec<NightSummary>` newest first, ≤ `limit` (136 §2.7). Wave C:
/// the last night only, from `state.json`; 136 W3 reads `ledger/*.json`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Nights {
    /// Row cap.
    pub limit: u32,
}

/// `ask` → `DreamSettings` (136 §2.7).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Settings;

/// `ask` → `Result<DreamSettings, String>` (136 §2.7): persist the section
/// through the config path and apply it live (136 W3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetSettings(pub DreamSettings);

/// `ask` → `Vec<Proposal>` (136 §2.7). Wave C: `proposals.jsonl` read as is.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Proposals;

/// `ask` → `Result<(), String>` (136 §2.7): the user pressed `choice` (136 W3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProposalAct {
    /// Proposal id.
    pub id: String,
    /// The option label pressed.
    pub choice: String,
}

/// `ask` → `Result<Metrics, String>` (136 §13) (136 W3).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MetricsOver {
    /// The window, days.
    pub days: u32,
}

impl Message<BeginNight> for Night {
    type Reply = Result<NightReceipt, String>;

    async fn handle(&mut self, m: BeginNight, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3
        let _ = m;
        Err("not built: W3 (BeginNight)".into())
    }
}

impl Message<Commit> for Night {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Commit, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3
        let _ = m;
        Err("not built: W3 (Commit)".into())
    }
}

impl Message<Undo> for Night {
    type Reply = Result<UndoReport, String>;

    async fn handle(&mut self, m: Undo, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3
        let _ = m;
        Err("not built: W3 (Undo)".into())
    }
}

impl Message<Export> for Night {
    type Reply = Result<PathBuf, String>;

    async fn handle(&mut self, m: Export, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3
        let _ = m;
        Err("not built: W3 (Export)".into())
    }
}

impl Message<RunNow> for Night {
    type Reply = Result<SessionId, String>;

    async fn handle(&mut self, m: RunNow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3: `state.run_request = Some(m.0)`, then `schedule::FireWith { name: "dreaming", overrides }`.
        let _ = m;
        Err("not built: W3 (RunNow)".into())
    }
}

impl Message<Status> for Night {
    type Reply = DreamStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        status(&self.data_dir, &self.cfg)
    }
}

impl Message<Nights> for Night {
    type Reply = Vec<NightSummary>;

    async fn handle(&mut self, m: Nights, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3: every `ledger/<night>.json`, newest first.
        read_state(&self.data_dir)
            .last_night
            .into_iter()
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

    async fn handle(&mut self, m: SetSettings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3: write config.yaml's `dreaming:` section through the existing settings path.
        let _ = m;
        Err("not built: W3 (SetSettings)".into())
    }
}

impl Message<Proposals> for Night {
    type Reply = Vec<Proposal>;

    async fn handle(&mut self, _: Proposals, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        std::fs::read_to_string(dir(&self.data_dir).join("proposals.jsonl"))
            .map(|t| {
                t.lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Message<ProposalAct> for Night {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ProposalAct, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3
        let _ = m;
        Err("not built: W3 (ProposalAct)".into())
    }
}

impl Message<MetricsOver> for Night {
    type Reply = Result<Metrics, String>;

    async fn handle(&mut self, m: MetricsOver, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // 136 W3
        let _ = m;
        Err("not built: W3 (Metrics)".into())
    }
}

/// Spawn and register the actor; the `engine` is stamped into `state.json`
/// once so the sensor and `home_engine` have something to compare.
pub fn start(rt: &Runtime, cfg: DreamConfig, engine: &str) -> anyhow::Result<()> {
    let mut state = read_state(&rt.data_dir);
    if state.engine.is_empty() {
        state.engine = engine.to_owned();
        write_state(&rt.data_dir, &state).context("write dreaming/state.json")?;
    }
    let actor = Night::spawn(Night::new(rt, cfg));
    actor.register(DREAMING_ACTOR)?;
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
        std::fs::write(super::dir(dir.path()).join("proposals.jsonl"), "{}\n\n{}\n").unwrap();
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
}
