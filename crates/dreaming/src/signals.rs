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
//!
//! 185 A3/A4: every heartbeat also writes where the hub is (`hub`, `peered`,
//! `hub_reachable`, `hub_name`, from the mesh) and recounts `sessions_since`
//! and `new_concepts_since` from the replicated session and knowledge
//! stores, so work done on any engine of the mesh counts; each counter is
//! the larger of that and this engine's own event count. Idle and desks stay
//! this engine's: they guard this machine's own load, not the mesh's.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, Utc};
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};

use crate::night::{read_state, write_atomic};
use crate::scrub::{clip, scrub};
use crate::types::SignalsSnapshot;

/// 185 A3/A5: the hub as this engine sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubView {
    /// This engine holds the lease (`mesh::is_hub`).
    pub hub: bool,
    /// This engine has paired peers.
    pub peered: bool,
    /// The lease holder's id and machine name; `None` without a holder.
    pub holder: Option<(String, String)>,
    /// The holder is this engine or connected to it.
    pub reachable: bool,
    /// Paired engines connected now: `(id, machine name)` (the pin's lookup).
    pub connected: Vec<(String, String)>,
}

/// [`HubView`] from `mesh::is_hub`, `mesh::has_peers` and the mesh status
/// (`None`: no mesh, or it did not answer).
pub fn hub_view(hub: bool, peered: bool, st: Option<&mesh::MeshStatus>) -> HubView {
    let connected: Vec<(String, String)> = st
        .map(|s| {
            s.peers
                .iter()
                .filter(|p| p.reachable)
                .map(|p| (p.id.0.clone(), p.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let holder = st.and_then(|s| {
        let h = s.lease.holder.as_ref()?;
        if *h == s.self_id {
            return Some(((h.0.clone(), s.self_name.clone()), true));
        }
        let name = s.peers.iter().find(|p| p.id == *h).map(|p| p.name.clone());
        let up = connected.iter().any(|(id, _)| *id == h.0);
        Some(((h.0.clone(), name.unwrap_or_default()), up))
    });
    HubView {
        hub,
        peered,
        reachable: holder.as_ref().is_some_and(|(_, up)| *up),
        holder: holder.map(|(h, _)| h),
        connected,
    }
}

/// [`hub_view`] now: asks the mesh actor for its status (2 s at most) —
/// only with paired peers; a single engine asks nothing (R1).
pub async fn hub_now() -> HubView {
    let peered = mesh::has_peers().unwrap_or(false);
    let st = match mesh::actor().filter(|_| peered) {
        Some(m) => tokio::time::timeout(Duration::from_secs(2), m.ask(mesh::Status))
            .await
            .ok()
            .and_then(Result::ok),
        None => None,
    };
    hub_view(mesh::is_hub(), peered, st.as_ref())
}

/// 185 A5: the engine a Dream now press runs on when it is not this one —
/// `(id, machine name)`: the `home_engine` pin when set and connected, else
/// the hub when this engine is peered, not the hub, and the hub is
/// connected. `None`: here (this engine dreams, or it is cut off).
pub fn dream_host(home_engine: Option<&str>, me: &str, v: &HubView) -> Option<(String, String)> {
    match home_engine.filter(|h| !h.is_empty()) {
        Some(pin) if pin == me => None,
        Some(pin) => v.connected.iter().find(|(id, _)| id == pin).cloned(),
        None => (v.peered && !v.hub && v.reachable)
            .then(|| v.holder.clone())
            .flatten(),
    }
}

/// 185 A4: harvest and model concepts first written after `since` (RFC
/// 3339; `None` = ever). The frontmatter `source` and `timestamp` travel
/// with the file, so a concept another engine wrote and the bundle sync
/// brought here counts like one written here.
/// ponytail: reads every concept's meta per heartbeat; the catalog is hundreds.
pub async fn concepts_since(k: &knowledge::Knowledge, since: Option<&str>) -> u32 {
    let since = since.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let Ok(metas) = k.list(None, None, None, u32::MAX).await else {
        return 0;
    };
    let n = metas
        .iter()
        .filter(|m| {
            matches!(
                m.source,
                Some(knowledge::Source::Harvest | knowledge::Source::Model)
            )
        })
        .filter(|m| {
            let at = m
                .timestamp
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
            match (at, since) {
                (Some(at), Some(since)) => at > since,
                (Some(_), None) => true,
                (None, _) => false,
            }
        })
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

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
    /// Test seam: skip the roster, `pmset`, the mesh and the store recounts.
    probes: bool,
    /// 185 A4: this engine's own harvest/model puts since the last dream.
    local_concepts: u32,
    /// 185 A4: the replicated stores' counts since [`Self::counted_from`].
    mesh_sessions: u32,
    mesh_concepts: u32,
    /// 185 A4: the last night's end (`Dreamt`): what the store counts run from.
    counted_from: Option<String>,
    /// The knowledge store, looked up on first use.
    knowledge: Option<Arc<knowledge::Knowledge>>,
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
            local_concepts: snap.new_concepts_since,
            counted_from: snap.last_dream_at.clone(),
            snap,
            seen: HashSet::new(),
            store,
            last_assistant: HashMap::new(),
            power_at: None,
            probes: true,
            mesh_sessions: 0,
            mesh_concepts: 0,
            knowledge: None,
        }
    }

    /// The counters the gate reads: the larger of this engine's events and
    /// the mesh-wide store counts (185 A4).
    fn counts(&mut self) {
        self.snap.sessions_since = (self.seen.len() as u32).max(self.mesh_sessions);
        self.snap.new_concepts_since = self.local_concepts.max(self.mesh_concepts);
    }

    /// 185 A4: recount attended root sessions and new concepts of every
    /// engine since the mesh's last night — the later of this engine's last
    /// `Dreamt` and (R1) the last run of another engine's dreaming session,
    /// from the replicated session rows: a node that never dreamt counts
    /// from the hub's night, not from "ever".
    async fn count_mesh(&mut self) {
        let mut since = self.counted_from.clone();
        if let Some(store) = &self.store {
            let t = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
            if let Ok(Some(at)) = store.routine_ran_elsewhere("dreaming").await
                && since.as_deref().and_then(t) < t(&at)
            {
                since = Some(at);
            }
            match store.attended_roots_since(since.as_deref()).await {
                Ok(n) => self.mesh_sessions = n,
                Err(e) => tracing::debug!("dreaming: sessions not counted: {e:#}"),
            }
        }
        if self.knowledge.is_none()
            && let Ok(Some(k)) =
                ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
            && let Ok(h) = k.ask(knowledge::Handle).await
        {
            self.knowledge = Some(h);
        }
        if let Some(k) = &self.knowledge {
            self.mesh_concepts = concepts_since(k, since.as_deref()).await;
        }
        self.counts();
    }

    /// 185 A3: where the hub is, into the snapshot.
    fn set_hub(&mut self, v: &HubView) {
        self.snap.hub = v.hub;
        self.snap.peered = v.peered;
        self.snap.hub_reachable = v.reachable;
        self.snap.hub_name = v
            .holder
            .as_ref()
            .map(|(_, n)| n.clone())
            .filter(|n| !n.is_empty());
    }

    /// The heartbeat's refresh: hub fields and mesh-wide counts (not in
    /// tests). R1: a single engine skips both — its own events are all the work.
    async fn refresh(&mut self) {
        if !self.probes {
            return;
        }
        let v = hub_now().await;
        self.set_hub(&v);
        if v.peered {
            self.count_mesh().await;
        } else {
            self.mesh_sessions = 0;
            self.mesh_concepts = 0;
            self.counts();
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
        !crate::isolation::excluded(&self.data_dir, s)
            && store.depth(s).await.unwrap_or(1) == 0
            && !store.unattended(s).await.unwrap_or(true)
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
                        self.counts();
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
            self.local_concepts += 1;
            self.counts();
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
        self.counted_from = Some(at.clone());
        self.snap.last_dream_at = Some(at);
        self.local_concepts = 0;
        self.mesh_sessions = 0;
        self.mesh_concepts = 0;
        self.seen.clear();
        self.counts();
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

/// `tell` (self, every [`HEARTBEAT`]): re-stamp `written_at` (R-19); 185:
/// refresh the hub fields and the mesh-wide counts first.
#[derive(Clone, Copy, Debug, Default)]
pub struct Heartbeat;

impl Message<Heartbeat> for Signals {
    type Reply = ();

    async fn handle(&mut self, _: Heartbeat, _: &mut Context<Self, Self::Reply>) {
        self.refresh().await;
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
    signals.refresh().await;
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
    async fn isolated_roots_do_not_count_or_cache_rewind_signals() {
        let dir = tempfile::tempdir().unwrap();
        let store = agent::store::Store::open(&dir.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        let root = SessionId::new();
        store
            .create_session(root, "m", None, None, None)
            .await
            .unwrap();
        store.set_memory_identity(root, "ops", true).await.unwrap();
        store
            .append(
                root,
                &genai::chat::ChatMessage::assistant("isolated secret"),
            )
            .await
            .unwrap();
        let mut s = Signals::new(dir.path(), "e", Some(store));
        s.probes = false;
        let actor = Signals::spawn(s);
        actor
            .tell(agent::AgentEvent::TurnFinished {
                session: root,
                turn: runtime::TurnId::new(),
                stop_reason: "end_turn".into(),
                usage: Default::default(),
                context: 0,
            })
            .await
            .unwrap();
        let snap = actor.ask(Snapshot).await.unwrap();
        assert_eq!(snap.sessions_since, 0);
        assert!(snap.last_root_turn_at.is_none());
        actor
            .tell(agent::AgentEvent::ContextChanged {
                session: root,
                revision: "1".into(),
                reason: "rewind".into(),
            })
            .await
            .unwrap();
        let _ = actor.ask(Snapshot).await.unwrap();
        assert!(crate::ledger::pending(dir.path()).is_empty());
    }

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

    /// 185 A4: the counters read the replicated stores — a root session row
    /// no turn event announced here (how another engine's arrives) and a
    /// concept another engine wrote (landed by the bundle sync, a `Sync`
    /// event here) count; a routine's session, a user concept and anything
    /// before the last night's end do not. R1: a node that never dreamt
    /// counts from the hub's last night (its dreaming session's row).
    #[tokio::test]
    async fn counts_include_work_done_on_another_engine() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let db = d.join("sessions.sqlite");
        let store = agent::store::Store::open(&db, Some(mesh::EngineId("eng-a".into())))
            .await
            .unwrap();
        let theirs = SessionId::new();
        let routine = SessionId::new();
        // Rows as replication leaves them: no turn event ever reached this engine.
        for s in [theirs, routine] {
            store
                .create_session(s, "m", None, None, None)
                .await
                .unwrap();
            store
                .append(s, &genai::chat::ChatMessage::user("hi"))
                .await
                .unwrap();
        }
        store.set_unattended(routine).await.unwrap();
        let rt = Runtime::new(figment::Figment::new(), d.to_owned());
        let k = knowledge::Knowledge::open(d, rt).await.unwrap();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let facts = k.root().join("facts");
        std::fs::create_dir_all(&facts).unwrap();
        for (id, source) in [("remote", "model"), ("typed", "user")] {
            std::fs::write(
                facts.join(format!("{id}.md")),
                format!(
                    "---\ntype: fact\ntitle: {id}\nsource: {source}\ntimestamp: {now}\n---\nbody\n"
                ),
            )
            .unwrap();
        }
        k.sync().await.unwrap();
        let mut s = Signals::new(d, "eng-a", Some(store));
        s.probes = false;
        s.knowledge = Some(k);
        s.count_mesh().await;
        assert_eq!((s.snap.sessions_since, s.snap.new_concepts_since), (1, 1));
        // The hub dreamt after all of it (its routine session row replicated
        // here): this node, which never dreamt, counts nothing new.
        let hub = agent::store::Store::open(&db, Some(mesh::EngineId("eng-h".into())))
            .await
            .unwrap();
        let night = SessionId::new();
        hub.create_session(night, "m", None, None, None)
            .await
            .unwrap();
        hub.set_title(night, "dreaming · dreaming").await.unwrap();
        hub.set_unattended(night).await.unwrap();
        hub.append(night, &genai::chat::ChatMessage::user("go"))
            .await
            .unwrap();
        hub.close().await;
        s.count_mesh().await;
        assert_eq!((s.snap.sessions_since, s.snap.new_concepts_since), (0, 0));
        // Nor after this engine's own night.
        s.counted_from = Some((Utc::now() + chrono::Duration::seconds(5)).to_rfc3339());
        s.count_mesh().await;
        assert_eq!((s.snap.sessions_since, s.snap.new_concepts_since), (0, 0));
    }

    /// 185 A5: Dream now runs on the hub while it is connected and is not
    /// this engine; here on the hub, on a side cut off, or alone; the pin
    /// wins when connected.
    #[test]
    fn dream_now_goes_to_the_connected_hub_only() {
        let hub = ("eng-h".to_owned(), "studio".to_owned());
        let v = |hub_here: bool, peered: bool, reachable: bool| HubView {
            hub: hub_here,
            peered,
            holder: Some(hub.clone()),
            reachable,
            connected: if reachable {
                vec![hub.clone()]
            } else {
                Vec::new()
            },
        };
        assert_eq!(
            dream_host(None, "me", &v(false, true, true)),
            Some(hub.clone())
        );
        assert_eq!(
            dream_host(None, "me", &v(true, true, true)),
            None,
            "the hub itself"
        );
        assert_eq!(
            dream_host(None, "me", &v(false, true, false)),
            None,
            "cut off: here"
        );
        assert_eq!(
            dream_host(None, "me", &v(false, false, false)),
            None,
            "alone"
        );
        assert_eq!(
            dream_host(Some("me"), "me", &v(false, true, true)),
            None,
            "pinned here"
        );
        assert_eq!(
            dream_host(Some("eng-h"), "me", &v(true, true, true)),
            Some(hub.clone()),
            "pinned there"
        );
        assert_eq!(
            dream_host(Some("eng-x"), "me", &v(false, true, true)),
            None,
            "pin not connected"
        );
    }
}
