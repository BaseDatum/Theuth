//! The `Signals` actor (design doc 136 §9.3): keeps `<data_dir>/dreaming/signals.json`
//! — what the gate sensor reads — from bus events. Wave C (real): `engine`,
//! `tz_offset_minutes`, `last_root_turn_at` on `TurnFinished` of an attended
//! root session, `sessions_since`, `new_concepts_since` on `KnowledgeChanged`
//! puts, `last_dream_at` from `state.json`; atomic rename on every write.
//! 136 W3: `desks_doing` from roster task events, `on_ac` / `battery_pct`
//! from `pmset -g batt` (≤ once a minute, macOS only), and the rewind
//! snapshot (`ContextChanged{reason: "rewind"}` → `ledger/pending/<ulid>.json`,
//! ≤ 2 KB, scrubbed).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{Local, Utc};
use kameo::actor::Spawn as _;
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};

use crate::night::{read_state, write_atomic};
use crate::types::SignalsSnapshot;

/// Registry name of the [`Signals`] actor.
pub const SIGNALS_ACTOR: &str = "dreaming-signals";

/// `<data_dir>/dreaming/signals.json`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("signals.json")
}

/// The actor.
#[derive(kameo::Actor)]
pub struct Signals {
    data_dir: PathBuf,
    snap: SignalsSnapshot,
    /// Sessions that turned since the last dream (the `sessions_since` set).
    seen: HashSet<SessionId>,
    /// The agent's durable store, for "is this a root, is it unattended".
    store: Option<agent::store::Store>,
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
        }
    }

    /// Stamp the clock and write the file (atomic rename).
    fn flush(&mut self) {
        let now = Utc::now();
        self.snap.written_at = Some(now.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        self.snap.tz_offset_minutes = Local::now().offset().local_minus_utc() / 60;
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
}

impl Message<agent::AgentEvent> for Signals {
    type Reply = ();

    async fn handle(&mut self, e: agent::AgentEvent, _: &mut Context<Self, Self::Reply>) {
        // 136 W3: `ContextChanged { reason: "rewind", .. }` → snapshot the dropped tail.
        if let agent::AgentEvent::TurnFinished { session, .. } = e {
            if self.attended_root(session).await {
                self.snap.last_root_turn_at =
                    Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
            }
            if self.seen.insert(session) {
                self.snap.sessions_since = self.seen.len() as u32;
            }
            self.flush();
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
/// (136 W3's `Night` sends it from `Commit{phase: "journal"}`).
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn turn_finished_and_puts_update_the_file_and_dreamt_resets() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Signals::new(dir.path(), "engine-1", None);
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
            })
            .await
            .unwrap();
        // Dreaming's own writes and deletes do not count.
        actor
            .tell(knowledge::KnowledgeChanged {
                id: "facts/y".into(),
                op: knowledge::KnowledgeOp::Archive,
                source: knowledge::Source::Dreaming,
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
}
