//! The `Signals` actor (design doc 136 §9.3): keeps `<data_dir>/dreaming/signals.json`
//! — what the gate sensor reads — from bus events, atomic rename on every
//! write: `engine`, `tz_offset_minutes`, `last_root_turn_at` on `TurnStarted`
//! and `TurnFinished` of an attended root session (R-17: a turn in flight is
//! not idle), `sessions_since` over attended roots only (R-18: the dreaming
//! session and every routine never count), `new_concepts_since` on
//! `KnowledgeChanged` puts, `last_dream_at` from `state.json` (moved by
//! `DreamStarted`, counters reset by `Dreamt`), `desks_doing` from the
//! roster (`ListAgents`, `Working`; no desk event exists on the bus, so it
//! is re-read on every finished turn), `on_ac` / `battery_pct` from
//! `pmset -g batt` at most once a minute on macOS (`None` elsewhere). A
//! [`HEARTBEAT`] re-stamps `written_at` so the gate can tell a live file
//! from a frozen one (R-19). On `ContextChanged{reason: "rewind"}` the last
//! assistant text of that session — cached at its previous `TurnFinished`,
//! because the rewind has already hidden the rows when the event arrives —
//! is written scrubbed (≤ 2 KB) to `ledger/pending/<id>.json` for Recall.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Local, Utc};
use kameo::actor::Spawn as _;
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};

use crate::night::{read_state, write_atomic};
use crate::scrub::{clip, scrub};
use crate::types::SignalsSnapshot;

/// Registry name of the [`Signals`] actor.
pub const SIGNALS_ACTOR: &str = "dreaming-signals";
/// Rewind snapshot cap (136 §4 phase 1).
pub const REWIND_CHARS: usize = 2 * 1024;
/// Power probe spacing.
pub const POWER_EVERY: Duration = Duration::from_secs(60);
/// `written_at` re-stamp spacing (R-19); the gate refuses a file older than
/// [`crate::sensor_rules::STALE_SECS`].
pub const HEARTBEAT: Duration = Duration::from_secs(60);

/// `<data_dir>/dreaming/signals.json`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("signals.json")
}

/// `pmset -g batt` → `(on_ac, battery_pct)`: the first line names the
/// source, the battery line carries `NN%`.
pub fn parse_pmset(out: &str) -> (Option<bool>, Option<u8>) {
    let on_ac = if out.contains("'AC Power'") {
        Some(true)
    } else if out.contains("'Battery Power'") {
        Some(false)
    } else {
        None
    };
    let pct = out.lines().skip(1).find_map(|l| {
        l.split_whitespace()
            .find_map(|w| w.trim_end_matches(';').strip_suffix('%'))
            .and_then(|n| n.parse::<u8>().ok())
    });
    (on_ac, pct)
}

/// The power state now; `(None, None)` off macOS or when `pmset` fails.
pub fn probe_power() -> (Option<bool>, Option<u8>) {
    if !cfg!(target_os = "macos") {
        return (None, None);
    }
    match std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
    {
        Ok(o) if o.status.success() => parse_pmset(&String::from_utf8_lossy(&o.stdout)),
        _ => (None, None),
    }
}

/// The actor.
#[derive(kameo::Actor)]
pub struct Signals {
    data_dir: PathBuf,
    snap: SignalsSnapshot,
    /// Sessions that turned since the last dream (the `sessions_since` set).
    seen: HashSet<SessionId>,
    /// The agent's durable store, for "is this a root, is it unattended" and the rewind cache.
    store: Option<agent::store::Store>,
    /// Last assistant text per attended root (≤ 2 KB, scrubbed): the rewind snapshot source.
    last_assistant: HashMap<SessionId, String>,
    /// When `pmset` last ran.
    power_at: Option<Instant>,
    /// Test seam: skip the roster and `pmset`.
    probes: bool,
}

/// `ask` → [`SignalsSnapshot`]: what the file says now.
#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot;

impl Signals {
    /// Start from the file on disk (counters survive a restart) with the
    /// engine and the clock refreshed.
    pub fn new(data_dir: &Path, engine: &str, store: Option<agent::store::Store>) -> Self {
        let mut snap: SignalsSnapshot = std::fs::read_to_string(path(data_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        snap.engine = engine.to_owned();
        snap.last_dream_at = read_state(data_dir).last_dream_at;
        Self {
            data_dir: data_dir.to_owned(),
            snap,
            seen: HashSet::new(),
            store,
            last_assistant: HashMap::new(),
            power_at: None,
            probes: true,
        }
    }

    /// Stamp the clock, refresh power (≤ once a minute) and write the file.
    fn flush(&mut self) {
        let now = Utc::now();
        self.snap.written_at = Some(now.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        self.snap.tz_offset_minutes = Local::now().offset().local_minus_utc() / 60;
        if self.probes && self.power_at.is_none_or(|t| t.elapsed() >= POWER_EVERY) {
            let (ac, pct) = probe_power();
            self.snap.on_ac = ac;
            self.snap.battery_pct = pct;
            self.power_at = Some(Instant::now());
        }
        match serde_json::to_vec_pretty(&self.snap) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&path(&self.data_dir), &bytes) {
                    tracing::warn!("dreaming: signals.json not written: {e}");
                }
            }
            Err(e) => tracing::warn!("dreaming: signals.json not serialised: {e}"),
        }
    }

    /// An attended root: depth 0 and not a routine's session. Unknown → false.
    async fn attended_root(&self, s: SessionId) -> bool {
        let Some(store) = &self.store else {
            return true;
        };
        store.depth(s).await.unwrap_or(1) == 0 && !store.unattended(s).await.unwrap_or(true)
    }

    /// Desks with a turn running now, from the roster (178 F4: a desk only
    /// serving a wake — parked on a long child — does not hold the dream).
    async fn desks_doing(&self) -> Option<u32> {
        if !self.probes {
            return None;
        }
        let roster = roster::actor()?;
        tokio::time::timeout(Duration::from_secs(2), roster.ask(roster::DesksRunning))
            .await
            .ok()?
            .ok()
    }

    /// Remember the session's last assistant text for a later rewind.
    async fn cache_last_assistant(&mut self, s: SessionId) {
        let Some(store) = &self.store else {
            return;
        };
        let Ok(rows) = store.messages(s).await else {
            return;
        };
        if let Some(text) = rows
            .iter()
            .rev()
            .find(|(_, m)| m.role == genai::chat::ChatRole::Assistant)
            .and_then(|(_, m)| m.content.joined_texts())
        {
            self.last_assistant
                .insert(s, clip(&scrub(&text), REWIND_CHARS));
        }
    }

    /// `ContextChanged{reason: "rewind"}`: the dropped tail's last assistant
    /// text goes to `ledger/pending/<id>.json`.
    fn snapshot_rewind(&mut self, s: SessionId) {
        let Some(text) = self.last_assistant.remove(&s) else {
            return;
        };
        let rewind = agent::LedgerRewind {
            session: s,
            at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            dropped_assistant_text: text,
        };
        let p = crate::ledger::pending_dir(&self.data_dir)
            .join(format!("{}.json", crate::cases::new_id()));
        match serde_json::to_vec_pretty(&rewind) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&p, &bytes) {
                    tracing::warn!("dreaming: rewind snapshot not written: {e}");
                }
            }
            Err(e) => tracing::warn!("dreaming: rewind snapshot not serialised: {e}"),
        }
    }
}

impl Message<agent::AgentEvent> for Signals {
    type Reply = ();

    async fn handle(&mut self, e: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        match e {
            // R-17: a root turn in flight is activity — stamp at the start too.
            agent::AgentEvent::TurnStarted { session, .. } => {
                if self.attended_root(session).await {
                    self.snap.last_root_turn_at =
                        Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                    self.flush();
                }
            }
            agent::AgentEvent::TurnFinished { session, .. } => {
                if self.attended_root(session).await {
                    self.snap.last_root_turn_at =
                        Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                    self.cache_last_assistant(session).await;
                    // R-18: only the user's own sessions are "something new".
                    if self.seen.insert(session) {
                        self.snap.sessions_since = self.seen.len() as u32;
                    }
                }
                if let Some(n) = self.desks_doing().await {
                    self.snap.desks_doing = n;
                }
                self.flush();
            }
            agent::AgentEvent::ContextChanged {
                session, reason, ..
            } if reason == "rewind" => self.snapshot_rewind(session),
            _ => {}
        }
    }
}

impl Message<knowledge::KnowledgeChanged> for Signals {
    type Reply = ();

    async fn handle(&mut self, e: knowledge::KnowledgeChanged, _: &mut Context<Self, Self::Reply>) {
        // Harvest and model puts are the day's new entries; dreaming's own
        // writes and syncs are not.
        if e.op == knowledge::KnowledgeOp::Put
            && matches!(
                e.source,
                knowledge::Source::Harvest | knowledge::Source::Model
            )
        {
            self.snap.new_concepts_since += 1;
            self.flush();
        }
    }
}

impl Message<Snapshot> for Signals {
    type Reply = SignalsSnapshot;

    async fn handle(&mut self, _: Snapshot, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.snap.clone()
    }
}

/// `tell`: a night ended at `at` (RFC 3339) — the counters start over
/// (`Night` sends it when the night closes).
#[derive(Clone, Debug)]
pub struct Dreamt(pub String);

impl Message<Dreamt> for Signals {
    type Reply = ();

    async fn handle(&mut self, Dreamt(at): Dreamt, _: &mut Context<Self, Self::Reply>) {
        self.snap.last_dream_at = Some(at);
        self.snap.new_concepts_since = 0;
        self.snap.sessions_since = 0;
        self.seen.clear();
        self.flush();
    }
}

/// `tell`: a night began at `at` (136 D7) — `last_dream_at` moves so the
/// gate's "already dreamt this window" holds even for a night that is cut
/// off; the counters stay, so a heavy day keeps its second window.
#[derive(Clone, Debug)]
pub struct DreamStarted(pub String);

impl Message<DreamStarted> for Signals {
    type Reply = ();

    async fn handle(&mut self, DreamStarted(at): DreamStarted, _: &mut Context<Self, Self::Reply>) {
        self.snap.last_dream_at = Some(at);
        self.flush();
    }
}

/// `tell` (self, every [`HEARTBEAT`]): re-stamp `written_at` (R-19).
#[derive(Clone, Copy, Debug, Default)]
pub struct Heartbeat;

impl Message<Heartbeat> for Signals {
    type Reply = ();

    async fn handle(&mut self, _: Heartbeat, _: &mut Context<Self, Self::Reply>) {
        self.flush();
    }
}

/// `tell` (tests): remember a session's last assistant text as if its turn had finished.
#[derive(Clone, Debug)]
pub struct RememberAssistant(pub SessionId, pub String);

impl Message<RememberAssistant> for Signals {
    type Reply = ();

    async fn handle(&mut self, m: RememberAssistant, _: &mut Context<Self, Self::Reply>) {
        self.last_assistant
            .insert(m.0, clip(&scrub(&m.1), REWIND_CHARS));
    }
}

/// Spawn, register, subscribe, and write the first `signals.json`.
pub async fn start(rt: &Runtime, engine: &str) -> anyhow::Result<()> {
    let store = match kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(a)) => a.ask(agent::DurableStore).await.ok(),
        _ => None,
    };
    if store.is_none() {
        tracing::warn!(
            "dreaming: agent store unavailable; every finished turn counts as a root turn"
        );
    }
    let mut signals = Signals::new(&rt.data_dir, engine, store);
    signals.flush();
    let actor = Signals::spawn_with_mailbox(signals, runtime::subscriber_mailbox());
    actor.register(SIGNALS_ACTOR)?;
    rt.subscribe::<Signals, agent::AgentEvent>(&actor).await;
    rt.subscribe::<Signals, knowledge::KnowledgeChanged>(&actor)
        .await;
    // R-19: the heartbeat; the loop ends with the actor.
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            if actor.tell(Heartbeat).await.is_err() {
                break;
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(dir: &Path) -> Signals {
        let mut s = Signals::new(dir, "engine-1", None);
        s.probes = false;
        s
    }

    #[tokio::test]
    async fn turn_finished_and_puts_update_the_file_and_dreamt_resets() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = quiet(dir.path());
        s.flush();
        let read = || -> SignalsSnapshot {
            serde_json::from_str(&std::fs::read_to_string(path(dir.path())).unwrap()).unwrap()
        };
        let first = read();
        assert_eq!(first.engine, "engine-1");
        assert!(first.written_at.is_some() && first.last_root_turn_at.is_none());
        assert_eq!(
            first.tz_offset_minutes,
            Local::now().offset().local_minus_utc() / 60
        );

        let actor = Signals::spawn(s);
        let a = SessionId::new();
        let turn = |session| agent::AgentEvent::TurnFinished {
            session,
            turn: runtime::TurnId::new(),
            stop_reason: "end_turn".into(),
            usage: Default::default(),
            context: 0,
        };
        actor.tell(turn(a)).await.unwrap();
        actor.tell(turn(a)).await.unwrap();
        actor.tell(turn(SessionId::new())).await.unwrap();
        actor
            .tell(knowledge::KnowledgeChanged {
                id: "facts/x".into(),
                op: knowledge::KnowledgeOp::Put,
                source: knowledge::Source::Harvest,
                commit: None,
            })
            .await
            .unwrap();
        // Dreaming's own writes and deletes do not count.
        actor
            .tell(knowledge::KnowledgeChanged {
                id: "facts/y".into(),
                op: knowledge::KnowledgeOp::Archive,
                source: knowledge::Source::Dreaming,
                commit: None,
            })
            .await
            .unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_eq!((snap.sessions_since, snap.new_concepts_since), (2, 1));
        assert!(snap.last_root_turn_at.is_some());
        assert_eq!(read().sessions_since, 2);
        actor
            .tell(Dreamt("2026-10-08T03:00:00Z".into()))
            .await
            .unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_eq!((snap.sessions_since, snap.new_concepts_since), (0, 0));
        assert_eq!(snap.last_dream_at.as_deref(), Some("2026-10-08T03:00:00Z"));
        assert_eq!(
            read().last_dream_at.as_deref(),
            Some("2026-10-08T03:00:00Z")
        );
        // A restart keeps the counters.
        actor.tell(turn(SessionId::new())).await.unwrap();
        let _ = actor.ask(Snapshot).await.unwrap();
        let again = Signals::new(dir.path(), "engine-2", None);
        assert_eq!(
            (again.snap.sessions_since, again.snap.engine.as_str()),
            (1, "engine-2")
        );
    }

    /// R-17: a root turn that STARTED stamps `last_root_turn_at` (a long
    /// in-flight turn is not idle); R-18: a routine's (unattended) or a
    /// child's turn neither stamps nor counts toward `sessions_since` — the
    /// dreaming session's own turn end after `Dreamt` leaves it at 0.
    /// `DreamStarted` moves `last_dream_at` without resetting the counters.
    #[tokio::test]
    async fn only_attended_roots_stamp_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let store = agent::store::Store::open(&d.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let root = SessionId::new();
        let routine = SessionId::new();
        let child = SessionId::new();
        store
            .create_session(root, "m", None, None, None)
            .await
            .unwrap();
        store
            .create_session(routine, "m", None, None, None)
            .await
            .unwrap();
        store.set_unattended(routine).await.unwrap();
        store
            .create_session(child, "m", Some(root), None, None)
            .await
            .unwrap();
        let mut s = Signals::new(d, "e", Some(store));
        s.probes = false;
        let actor = Signals::spawn(s);
        let started = |session| agent::AgentEvent::TurnStarted {
            session,
            turn: runtime::TurnId::new(),
            text: String::new(),
            model: "m".into(),
            turn_seq: 1,
            channel: None,
        };
        let finished = |session| agent::AgentEvent::TurnFinished {
            session,
            turn: runtime::TurnId::new(),
            stop_reason: "end_turn".into(),
            usage: Default::default(),
            context: 0,
        };
        actor.tell(started(routine)).await.unwrap();
        actor.tell(started(child)).await.unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert!(snap.last_root_turn_at.is_none(), "R-18: not a root turn");
        actor.tell(started(root)).await.unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert!(snap.last_root_turn_at.is_some(), "R-17: in flight counts");
        assert_eq!(snap.sessions_since, 0, "a start is not a finished session");
        actor.tell(finished(routine)).await.unwrap();
        actor.tell(finished(child)).await.unwrap();
        actor.tell(finished(root)).await.unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_eq!(snap.sessions_since, 1, "the root only");
        actor
            .tell(DreamStarted("2026-10-08T01:00:00Z".into()))
            .await
            .unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_eq!(snap.last_dream_at.as_deref(), Some("2026-10-08T01:00:00Z"));
        assert_eq!(snap.sessions_since, 1, "DreamStarted keeps the counters");
        actor
            .tell(Dreamt("2026-10-08T03:00:00Z".into()))
            .await
            .unwrap();
        // The dreaming routine's own turn ends after the close.
        actor.tell(finished(routine)).await.unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_eq!(snap.sessions_since, 0, "R-18: nothing new");
        // Heartbeat re-stamps `written_at`.
        let before = snap.written_at.clone();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        actor.tell(Heartbeat).await.unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_ne!(snap.written_at, before);
    }

    #[tokio::test]
    async fn a_rewind_snapshots_the_cached_assistant_text_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let actor = Signals::spawn(quiet(dir.path()));
        let s = SessionId::new();
        let long = format!(
            "I would run it with sk-abcdefghijklmnop. {}",
            "x".repeat(3000)
        );
        actor.tell(RememberAssistant(s, long)).await.unwrap();
        // A rewind of a session with no cache is nothing.
        actor
            .tell(agent::AgentEvent::ContextChanged {
                session: SessionId::new(),
                revision: "1".into(),
                reason: "rewind".into(),
            })
            .await
            .unwrap();
        // A non-rewind change is nothing.
        actor
            .tell(agent::AgentEvent::ContextChanged {
                session: s,
                revision: "1".into(),
                reason: "trim".into(),
            })
            .await
            .unwrap();
        let _ = actor.ask(Snapshot).await.unwrap();
        assert!(crate::ledger::pending(dir.path()).is_empty());
        actor
            .tell(agent::AgentEvent::ContextChanged {
                session: s,
                revision: "2".into(),
                reason: "rewind".into(),
            })
            .await
            .unwrap();
        let _ = actor.ask(Snapshot).await.unwrap();
        let pending = crate::ledger::pending(dir.path());
        assert_eq!(pending.len(), 1);
        let r = &pending[0].1;
        assert_eq!(r.session, s);
        assert!(r.dropped_assistant_text.contains("[redacted]"));
        assert!(r.dropped_assistant_text.chars().count() <= REWIND_CHARS);
        // The cache is consumed: a second rewind of the same session writes nothing new.
        actor
            .tell(agent::AgentEvent::ContextChanged {
                session: s,
                revision: "3".into(),
                reason: "rewind".into(),
            })
            .await
            .unwrap();
        let _ = actor.ask(Snapshot).await.unwrap();
        assert_eq!(crate::ledger::pending(dir.path()).len(), 1);
    }

    #[test]
    fn pmset_output_parses_both_sources() {
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1234)\t100%; charged; 0:00 remaining present: true\n";
        assert_eq!(parse_pmset(ac), (Some(true), Some(100)));
        let batt = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1234)\t37%; discharging; 3:10 remaining present: true\n";
        assert_eq!(parse_pmset(batt), (Some(false), Some(37)));
        assert_eq!(
            parse_pmset("Now drawing from 'AC Power'\n"),
            (Some(true), None)
        );
        assert_eq!(parse_pmset("garbage"), (None, None));
    }
}
