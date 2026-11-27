//! Design doc 155 §4.2 / §5: the supervisor side of the activity monitor —
//! a child's stall episode ([`ChildStalled`]) rendered as `<child_stalled>`
//! to the parent, `wait_peer` registration ([`WaitPeer`]), the settlement
//! check every state transition runs ([`Agents::check_peer_waits`]), and the
//! boot re-arm of `waits(peer_state)` rows ([`Agents::rebuild_peer_waits`]).
//! Wrappers `child_stalled` / `peer_state` live beside `job_idle` in
//! `supervisor.rs`.

use std::time::{Duration, Instant};

use chrono::Utc;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::SessionId;
use serde_json::json;
use tools::{WaitKind, WaitRow};

use super::{Agents, child_stalled, peer_state};
use crate::AGENT_ACTOR;
use crate::activity::{
    ChildStalled, PeerFacts, PeerStateWord, PeerWait, PeerWaitRegistered, StallPhase, WaitPeer,
    peer_state_of,
};
use crate::store::Scope;

/// `tell` (internal, 155 §5.3): a `wait_peer`'s deadline passed; settles
/// `timeout` when the wait is still registered.
struct PeerWaitDue {
    caller: SessionId,
    child: SessionId,
}

/// One §5.1 word with how long it has held (`since_secs`, 0 when unknown).
struct Word {
    word: PeerStateWord,
    since_secs: u64,
}

impl Agents {
    /// The §5.1 facts for `child`, or `None` when its session row is gone.
    /// `live` is a live actor; `dead` is folded into it (a child whose
    /// connection failed for good stops right after, `acp_child.rs`).
    async fn peer_facts(&self, child: SessionId) -> Option<PeerFacts> {
        let row = self.store.session(child).await.ok().flatten()?;
        let store = self.store_for(child);
        let ask = store
            .questions_pending(child)
            .await
            .is_ok_and(|q| !q.is_empty());
        let result_delivered = !self
            .store
            .child_waits(Scope::Here)
            .await
            .unwrap_or_default()
            .iter()
            .any(|w| w.target == child.to_string());
        // A native child reports its own turn end; an ACP child's entry says.
        let reports = match &row.agent {
            None => crate::activity::Reports {
                permission: true,
                turn_end: true,
                session_load: true,
            },
            #[cfg(not(target_os = "ios"))]
            Some(agent) => crate::peers::parse_agent(agent)
                .ok()
                .and_then(|(_, engine)| {
                    acp::load(&self.rt.data_dir)
                        .ok()?
                        .get(engine)
                        .map(acp::Entry::reports)
                })
                .unwrap_or_default(),
            #[cfg(target_os = "ios")]
            Some(_) => crate::activity::Reports::default(),
        };
        Some(PeerFacts {
            live: self
                .sessions
                .get(&child)
                .is_some_and(|l| l.actor.is_alive()),
            dead: false,
            running: self.running.contains(&child),
            approval: self.approvals.contains_key(&child),
            ask,
            requires_action: row
                .observed
                .as_ref()
                .is_some_and(|o| o.state == crate::ObservedState::RequiresAction),
            stalled_secs: self.activity.stalled_secs(child, Instant::now()),
            result_delivered,
            queued: self.ledger.activity(child).is_some(),
            reports,
        })
    }

    /// `peer_state_of` over [`Self::peer_facts`] with the word's age (§5.3).
    async fn peer_word(&self, child: SessionId) -> Word {
        let Some(f) = self.peer_facts(child).await else {
            return Word {
                word: PeerStateWord::NotRunning,
                since_secs: 0,
            };
        };
        let word = peer_state_of(&f);
        let now_ms = Utc::now().timestamp_millis().max(0) as u64;
        let since_secs = match word {
            PeerStateWord::Blocked => self
                .approvals
                .get(&child)
                .map_or(0, |(.., at)| now_ms.saturating_sub(*at) / 1000),
            PeerStateWord::Stalled => f.stalled_secs.unwrap_or(0),
            PeerStateWord::Idle | PeerStateWord::Done => self
                .last_stop
                .get(&child)
                .map_or(0, |(_, at)| (Utc::now() - *at).num_seconds().max(0) as u64),
            PeerStateWord::Running => self.child_turns.get(&child).map_or(0, |t| {
                (Utc::now() - t.started_at).num_seconds().max(0) as u64
            }),
            PeerStateWord::NotRunning | PeerStateWord::Timeout => 0,
        };
        Word { word, since_secs }
    }

    /// `to` as one of `caller`'s children: `(child, name)` (§5.2).
    async fn resolve_child(
        &self,
        caller: SessionId,
        to: &str,
    ) -> Result<(SessionId, String), String> {
        let not_child = || format!("wait_peer: {to} is not one of your children");
        if let Some(id) = to.strip_prefix("session:") {
            let child: SessionId = id.parse().map_err(|_| not_child())?;
            let row = self
                .store
                .session(child)
                .await
                .ok()
                .flatten()
                .filter(|r| r.parent == Some(caller))
                .ok_or_else(not_child)?;
            return Ok((child, row.name.unwrap_or_else(|| child.to_string())));
        }
        match self.store.child_by_name(caller, to).await {
            Ok(Some(child)) => Ok((child, to.to_owned())),
            _ => Err(not_child()),
        }
    }

    /// 155 §5.2: the body of the `WaitPeer` handler; `me` arms the timeout
    /// (looked up when `None`).
    pub(crate) async fn register_peer_wait(
        &mut self,
        m: WaitPeer,
        me: Option<ActorRef<Agents>>,
    ) -> Result<PeerWaitRegistered, String> {
        let (child, name) = self.resolve_child(m.session, &m.to).await?;
        if m.until.is_empty() || m.until.iter().any(|w| !w.waitable()) {
            return Err(
                "wait_peer: `until` must be a non-empty subset of blocked|idle|done|stalled".into(),
            );
        }
        if m.timeout_secs == Some(0) {
            return Err("wait_peer: `timeout_secs` must be at least 1".into());
        }
        let facts = self
            .peer_facts(child)
            .await
            .ok_or_else(|| format!("wait_peer: {} is not one of your children", m.to))?;
        if !facts.reports.turn_end
            && m.until
                .iter()
                .any(|w| matches!(w, PeerStateWord::Idle | PeerStateWord::Done))
        {
            return Err(format!(
                "wait_peer: {name}'s harness does not report turn end; wait on blocked|stalled or use send_message{{await_reply}}"
            ));
        }
        let Word { word, since_secs } = self.peer_word(child).await;
        if word == PeerStateWord::NotRunning {
            return Err(format!(
                "wait_peer: {name} (session:{child}) is not running"
            ));
        }
        let holds = satisfies(&m.until, word);
        if holds {
            return Ok(PeerWaitRegistered {
                child,
                name,
                state: word,
                holds,
                since_secs,
            });
        }
        // One wait per (caller, child): the previous row goes with its entry.
        if let Some(old) = self.activity.waits.remove(&(m.session, child))
            && old.call_id != m.call_id
        {
            let _ = self.store.wait_clear(old.caller, &old.call_id).await;
        }
        let now = Instant::now();
        let deadline = m.timeout_secs.map(Duration::from_secs);
        let row = WaitRow {
            session: m.session,
            tool_call_id: m.call_id.clone(),
            kind: WaitKind::PeerState,
            target: child.to_string(),
            deadline: deadline
                .map(|d| Utc::now() + chrono::Duration::from_std(d).unwrap_or_default()),
            payload: json!({
                "until": m.until,
                "name": name,
                "since_ms": Utc::now().timestamp_millis().max(0),
            }),
        };
        if let Err(e) = self.store.wait_put(&row).await {
            return Err(format!("wait_peer: journal: {e:#}"));
        }
        let deadline = deadline.map(|d| now + d);
        self.activity.waits.insert(
            (m.session, child),
            PeerWait {
                caller: m.session,
                call_id: m.call_id,
                child,
                name: name.clone(),
                until: m.until,
                deadline,
                since: now,
            },
        );
        if let Some(at) = deadline {
            arm_peer_wait(me, m.session, child, at);
        }
        Ok(PeerWaitRegistered {
            child,
            name,
            state: word,
            holds: false,
            since_secs,
        })
    }

    /// Settle one wait: clear the row, drop the entry, wake the caller with
    /// `<peer_state>` (§5.3).
    async fn settle_peer_wait(
        &mut self,
        key: (SessionId, SessionId),
        word: PeerStateWord,
        since_secs: u64,
        body: String,
    ) {
        let Some(w) = self.activity.waits.remove(&key) else {
            return;
        };
        if let Err(e) = self.store.wait_clear(w.caller, &w.call_id).await {
            tracing::warn!(session = %w.caller, "wait_clear(peer_state): {e:#}");
        }
        let wrapped = peer_state(
            &w.name,
            w.child,
            word,
            since_secs,
            &body,
            chrono::Local::now(),
        );
        self.inject(
            w.caller,
            &format!("session:{}", w.child),
            body,
            wrapped,
            None,
        )
        .await;
    }

    /// 155 §5.3: recompute the §5.1 word for `child` and settle every
    /// registered wait it satisfies (or every wait on a `not_running` child).
    pub(crate) async fn check_peer_waits(&mut self, child: SessionId) {
        let keys: Vec<(SessionId, SessionId)> = self
            .activity
            .waits
            .keys()
            .filter(|(_, c)| *c == child)
            .copied()
            .collect();
        if keys.is_empty() {
            return;
        }
        let Word { word, since_secs } = self.peer_word(child).await;
        let gone = word == PeerStateWord::NotRunning;
        let row_gone = gone && self.store.session(child).await.ok().flatten().is_none();
        for key in keys {
            let Some(w) = self.activity.waits.get(&key) else {
                continue;
            };
            let body = if gone {
                let why = if row_gone {
                    "its session no longer exists"
                } else {
                    "it has no live actor (closed, lost or replaced)"
                };
                format!("{} is gone: {why}.", w.name)
            } else if satisfies(&w.until, word) {
                word_body(word, &w.name, since_secs)
            } else {
                continue;
            };
            self.settle_peer_wait(key, word, since_secs, body).await;
        }
    }

    /// `PeerWaitDue`: the deadline passed and the wait is still there.
    async fn peer_wait_due(&mut self, caller: SessionId, child: SessionId) {
        let key = (caller, child);
        let Some(w) = self.activity.waits.get(&key) else {
            return;
        };
        let now = Instant::now();
        let Some(deadline) = w.deadline.filter(|d| *d <= now) else {
            return;
        };
        let after = deadline.saturating_duration_since(w.since).as_secs();
        let Word { word, .. } = self.peer_word(child).await;
        let body = format!(
            "{} is still {} after {after} s.",
            self.activity.waits[&key].name,
            word.as_str()
        );
        self.settle_peer_wait(key, PeerStateWord::Timeout, after, body)
            .await;
    }

    /// 155 §5.4: boot — `Store::peer_state_waits(Scope::Here)` → re-arm,
    /// `timeout` or `not_running` each row. After `rearm_reminders_for`,
    /// before `resume_scan`. A child is a respawn candidate when it is
    /// mid-prompt or a `child_result` row still targets it (the sets
    /// `respawn_acp_children` reads); its first transition after the
    /// respawn runs `check_peer_waits` — no actor is live yet here.
    pub(crate) async fn rebuild_peer_waits(&mut self) {
        let rows = match self.store.peer_state_waits(Scope::Here).await {
            Ok(rows) => rows,
            Err(e) => return tracing::error!("155: peer_state_waits: {e:#}"),
        };
        if rows.is_empty() {
            return;
        }
        let child_rows = self
            .store
            .child_waits(Scope::Here)
            .await
            .unwrap_or_default();
        let now = Utc::now();
        let now_ms = now.timestamp_millis().max(0) as u64;
        let inow = Instant::now();
        for w in rows {
            let Ok(child) = w.target.parse::<SessionId>() else {
                let _ = self.store.wait_clear(w.session, &w.tool_call_id).await;
                continue;
            };
            let until: Vec<PeerStateWord> =
                serde_json::from_value(w.payload["until"].clone()).unwrap_or_default();
            let name = w.payload["name"]
                .as_str()
                .map_or_else(|| child.to_string(), str::to_owned);
            let since_ms = w.payload["since_ms"].as_u64().unwrap_or(now_ms);
            let since = inow - Duration::from_millis(now_ms.saturating_sub(since_ms));
            // `Some(None)`: a deadline already past.
            let left = w.deadline.map(|d| (d - now).to_std().ok());
            let key = (w.session, child);
            self.activity.waits.insert(
                key,
                PeerWait {
                    caller: w.session,
                    call_id: w.tool_call_id,
                    child,
                    name: name.clone(),
                    until,
                    deadline: left.flatten().map(|d| inow + d),
                    since,
                },
            );
            let row = self.store.session(child).await.ok().flatten();
            let candidate = row.is_some()
                && (self
                    .sessions
                    .get(&child)
                    .is_some_and(|l| l.actor.is_alive())
                    || child_rows.iter().any(|c| c.target == w.target)
                    || self.store.midprompt(child).await.unwrap_or(false));
            if !candidate {
                let why = if row.is_none() {
                    "its session no longer exists"
                } else {
                    "it was not running when the engine restarted"
                };
                self.settle_peer_wait(
                    key,
                    PeerStateWord::NotRunning,
                    0,
                    format!("{name} is gone: {why}."),
                )
                .await;
            } else if let Some(None) = left {
                let after = w.deadline.map_or(0, |d| {
                    (d.timestamp_millis().max(0) as u64).saturating_sub(since_ms) / 1000
                });
                self.settle_peer_wait(
                    key,
                    PeerStateWord::Timeout,
                    after,
                    format!(
                        "{name} was still running after {after} s (the engine restarted meanwhile)."
                    ),
                )
                .await;
            } else if let Some(Some(d)) = left {
                arm_peer_wait(None, w.session, child, inow + d);
            }
        }
    }
}

/// `word` ends a wait on `until`: the word itself, or `done` for a wait on
/// `idle` (done is idle with the result delivered, §5.1).
fn satisfies(until: &[PeerStateWord], word: PeerStateWord) -> bool {
    until.contains(&word) || (word == PeerStateWord::Done && until.contains(&PeerStateWord::Idle))
}

/// The one-line reason of a `<peer_state>` for a waitable word (§5.3).
fn word_body(word: PeerStateWord, name: &str, since_secs: u64) -> String {
    match word {
        PeerStateWord::Done => {
            format!("{name} is done: its result was delivered to you and nothing is queued for it.")
        }
        PeerStateWord::Idle => {
            format!("{name} is idle: its turn ended and it can be prompted again.")
        }
        PeerStateWord::Blocked => format!(
            "{name} is blocked: an unanswered permission ask or question is keyed to it (for {since_secs} s)."
        ),
        PeerStateWord::Stalled => format!(
            "{name} is stalled: no session update for {since_secs} s and no tool call in progress."
        ),
        other => format!("{name} is {}.", other.as_str()),
    }
}

/// One `tokio` sleep per wait (§5.3): `PeerWaitDue` at `deadline`; `me` is
/// looked up when not given (boot). No actor (tests) → no timer.
fn arm_peer_wait(
    me: Option<ActorRef<Agents>>,
    caller: SessionId,
    child: SessionId,
    deadline: Instant,
) {
    let Some(me) = me.or_else(|| ActorRef::<Agents>::lookup(AGENT_ACTOR).ok().flatten()) else {
        return;
    };
    tokio::spawn(async move {
        tokio::time::sleep_until(deadline.into()).await;
        let _ = me.tell(PeerWaitDue { caller, child }).await;
    });
}

/// The guidance sentence of `<child_stalled>` per phase (§4.1).
fn stall_body(name: &str, phase: StallPhase, quiet_secs: u64) -> String {
    match phase {
        StallPhase::NoFirstUpdate => format!(
            "{name}'s prompt was delivered {quiet_secs} s ago and nothing came back; the harness may be loading or stuck at startup. Ask it what it is doing with send_message, wait on it with wait_peer, or check its session with session_read."
        ),
        StallPhase::Silent => format!(
            "{name} has sent no session update for {quiet_secs} s and no tool call is in progress. Ask it what it is doing with send_message, wait on it with wait_peer, or check its session with session_read; a child mid-build usually answers."
        ),
    }
}

impl Message<ChildStalled> for Agents {
    type Reply = ();

    /// 155 §4.2: `Some` records the episode and injects `<child_stalled>`
    /// into the parent; `None` clears it. Either way `check_peer_waits`.
    async fn handle(&mut self, m: ChildStalled, _: &mut Context<Self, Self::Reply>) {
        self.child_stalled(m).await;
    }
}

impl Agents {
    /// The body of the `ChildStalled` handler.
    pub(crate) async fn child_stalled(&mut self, m: ChildStalled) {
        match m.stall {
            Some(stall) => {
                self.activity
                    .stalls
                    .insert(m.child, (stall.phase, Instant::now()));
                // A root ACP session has no parent: recorded, nobody woken.
                if let Some((parent, name)) =
                    self.sessions.get(&m.child).and_then(|l| l.parent.clone())
                {
                    let body = stall_body(&name, stall.phase, stall.quiet_secs);
                    let wrapped = child_stalled(
                        &name,
                        m.child,
                        stall.phase,
                        stall.quiet_secs,
                        &stall.last,
                        &body,
                        chrono::Local::now(),
                    );
                    self.inject(parent, &format!("session:{}", m.child), body, wrapped, None)
                        .await;
                }
            }
            None => {
                self.activity.stalls.remove(&m.child);
            }
        }
        self.check_peer_waits(m.child).await;
    }
}

impl Message<WaitPeer> for Agents {
    type Reply = Result<PeerWaitRegistered, String>;

    /// 155 §5.2: resolve `to` among the caller's children (pinned), validate
    /// `until` against the harness's `reports`, answer at once when the word
    /// already holds, else write the `peer_state` row and the in-memory wait.
    async fn handle(&mut self, m: WaitPeer, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let me = ctx.actor_ref().clone();
        self.register_peer_wait(m, Some(me)).await
    }
}

impl Message<PeerWaitDue> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: PeerWaitDue, _: &mut Context<Self, Self::Reply>) {
        self.peer_wait_due(m.caller, m.child).await;
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use std::sync::Arc;

    use kameo::actor::{ActorRef, Spawn};
    use runtime::TurnId;
    use tokio::sync::mpsc::UnboundedSender;

    use super::*;
    use crate::AgentEvent;
    use crate::acp_child::AcpChild;
    use crate::supervisor::tests::agents_on;
    use crate::supervisor::{Live, LiveActor};

    /// Every `AgentEvent` on the fixture's bus.
    #[derive(kameo::Actor)]
    struct Sink(UnboundedSender<AgentEvent>);

    impl Message<AgentEvent> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            let _ = self.0.send(e);
        }
    }

    /// A dialer that never connects: the child actor stays alive and idle.
    #[derive(Debug)]
    struct Offline;

    #[async_trait::async_trait]
    impl acp::Dialer for Offline {
        async fn dial(&self) -> Result<acp::LineDuplex, String> {
            Err("offline".to_owned())
        }
    }

    struct Rig {
        _scratch: crate::TestDir,
        a: Agents,
        parent: SessionId,
        rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        /// Deliveries drained but not yet asked for: `(from, text)`.
        pending: Vec<(String, String)>,
        _sink: ActorRef<Sink>,
    }

    impl Rig {
        async fn new() -> Self {
            let scratch = crate::test_dir("155-w1");
            let a = agents_on(scratch.path().to_path_buf(), Some("a"), true).await;
            let parent = SessionId::new();
            a.store
                .create_session(parent, "m", None, None, None)
                .await
                .unwrap();
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
            a.rt.subscribe::<Sink, AgentEvent>(&sink).await;
            Self {
                _scratch: scratch,
                a,
                parent,
                rx,
                pending: Vec::new(),
                _sink: sink,
            }
        }

        /// A child row named `name` under the parent with a live (idle) actor.
        async fn child(&mut self, name: &str) -> (SessionId, ActorRef<AcpChild>) {
            let child = SessionId::new();
            self.a
                .store
                .create_session(child, "m", Some(self.parent), Some(name), None)
                .await
                .unwrap();
            let actor = AcpChild::spawn(AcpChild::new(
                child,
                "omp".into(),
                acp::Transport::Dial(Arc::new(Offline)),
                self.a.rt.data_dir.clone(),
                None,
                None,
                None,
                None,
                None,
                Duration::from_secs(60),
                self.a.store.clone(),
                self.a.rt.clone(),
                Default::default(),
                Vec::new(),
            ));
            self.a.sessions.insert(
                child,
                Live {
                    actor: LiveActor::Acp(actor.clone()),
                    parent: Some((self.parent, name.to_owned())),
                },
            );
            (child, actor)
        }

        async fn wait(
            &mut self,
            to: &str,
            until: &[PeerStateWord],
            timeout_secs: Option<u64>,
        ) -> Result<PeerWaitRegistered, String> {
            self.a
                .register_peer_wait(
                    WaitPeer {
                        session: self.parent,
                        call_id: format!("call-{}", self.a.activity.waits.len() + 1),
                        to: to.to_owned(),
                        until: until.to_vec(),
                        timeout_secs,
                    },
                    None,
                )
                .await
        }

        async fn rows(&self) -> Vec<WaitRow> {
            self.a.store.peer_state_waits(Scope::Here).await.unwrap()
        }

        /// The `<peer_state>` bodies delivered to the parent from `child`, in order.
        async fn wakes(&mut self, child: SessionId) -> Vec<String> {
            // A bus `tell` lands after our own ask round-trips through the sink.
            let _ = self
                ._sink
                .ask(AgentEvent::TitleChanged {
                    session: self.parent,
                    title: String::new(),
                    tags: Vec::new(),
                })
                .await;
            while let Ok(e) = self.rx.try_recv() {
                if let AgentEvent::MessageDelivered {
                    session,
                    from,
                    text,
                    ..
                } = e
                    && session == self.parent
                {
                    self.pending.push((from, text));
                }
            }
            let from = format!("session:{child}");
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|(f, _)| *f == from);
            self.pending = rest;
            mine.into_iter().map(|(_, t)| t).collect()
        }
    }

    use PeerStateWord::*;

    /// §5.2: `holds: true` registers nothing and writes no row; the
    /// refusals — not a child, an empty or unwaitable `until`, a zero
    /// timeout, idle/done on a harness without `turn_end`.
    #[tokio::test]
    async fn holds_true_registers_nothing_and_bad_calls_are_refused() {
        let mut rig = Rig::new().await;
        let (child, _actor) = rig.child("w1").await;
        let r = rig.wait("w1", &[Done, Idle], None).await.unwrap();
        assert_eq!((r.child, r.state, r.holds), (child, Done, true));
        assert_eq!(r.name, "w1");
        assert!(rig.rows().await.is_empty());
        assert!(rig.a.activity.waits.is_empty());
        let r = rig
            .wait(&format!("session:{child}"), &[Idle], None)
            .await
            .unwrap();
        assert_eq!(
            (r.state, r.holds),
            (Done, true),
            "done is idle with the result delivered"
        );
        assert!(rig.rows().await.is_empty());

        assert_eq!(
            rig.wait("nobody", &[Done], None).await.unwrap_err(),
            "wait_peer: nobody is not one of your children"
        );
        let stranger = SessionId::new();
        rig.a
            .store
            .create_session(stranger, "m", None, Some("w1"), None)
            .await
            .unwrap();
        assert!(
            rig.wait(&format!("session:{stranger}"), &[Done], None)
                .await
                .unwrap_err()
                .ends_with("is not one of your children")
        );
        assert!(
            rig.wait("w1", &[], None)
                .await
                .unwrap_err()
                .contains("non-empty subset")
        );
        assert!(
            rig.wait("w1", &[Running], None)
                .await
                .unwrap_err()
                .contains("non-empty subset")
        );
        assert!(
            rig.wait("w1", &[Done], Some(0))
                .await
                .unwrap_err()
                .contains("timeout_secs")
        );

        // An ACP child of a harness without `turn_end` (unknown command).
        acp::save(
            &rig.a.rt.data_dir,
            &serde_yaml::from_str("mystery:\n  command: mystery-acp\n").unwrap(),
        )
        .unwrap();
        let (c2, _a2) = rig.child("w2").await;
        rig.a.store.set_acp(c2, "mystery", None, &[]).await.unwrap();
        let e = rig.wait("w2", &[Idle], None).await.unwrap_err();
        assert!(e.contains("does not report turn end"), "{e}");
        rig.a.running.insert(c2);
        let r = rig.wait("w2", &[Blocked, Stalled], None).await.unwrap();
        assert_eq!((r.state, r.holds), (Running, false));
    }

    /// §5.3: a registered wait writes the `peer_state` row and the caller is
    /// woken with `done` after the child's result is handed off.
    #[tokio::test]
    async fn done_wakes_the_caller_after_hand_off() {
        let mut rig = Rig::new().await;
        let (child, _actor) = rig.child("w1").await;
        rig.a.running.insert(child);
        let r = rig.wait("w1", &[Done], None).await.unwrap();
        assert_eq!((r.state, r.holds), (Running, false));
        let rows = rig.rows().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            (
                rows[0].session,
                rows[0].kind,
                rows[0].target.as_str(),
                rows[0].deadline
            ),
            (
                rig.parent,
                WaitKind::PeerState,
                child.to_string().as_str(),
                None
            )
        );
        assert_eq!(rows[0].payload["until"], json!(["done"]));
        assert_eq!(rows[0].payload["name"], "w1");
        assert!(rig.a.activity.waits.contains_key(&(rig.parent, child)));

        // Still running: a transition that changes nothing settles nothing.
        rig.a.check_peer_waits(child).await;
        assert_eq!(rig.rows().await.len(), 1);

        let turn = TurnId::new();
        rig.a.turn_text.insert((child, turn), "final".into());
        rig.a.turn_finished(child, turn, "end_turn").await;
        let wakes = rig.wakes(child).await;
        let done = wakes
            .iter()
            .find(|t| t.starts_with("w1 is done"))
            .unwrap_or_else(|| panic!("no done wake: {wakes:?}"));
        assert_eq!(
            done,
            "w1 is done: its result was delivered to you and nothing is queued for it."
        );
        assert!(rig.rows().await.is_empty());
        assert!(rig.a.activity.waits.is_empty());
    }

    /// §5.2 pinning and §5.3 `not_running`: a second child under the same
    /// name never satisfies the wait; the pinned actor stopping settles it
    /// as `not_running`.
    #[tokio::test]
    async fn pinned_wait_ends_not_running_when_the_actor_stops() {
        let mut rig = Rig::new().await;
        let (first, actor) = rig.child("w1").await;
        rig.a.running.insert(first);
        let r = rig.wait("w1", &[Done], None).await.unwrap();
        assert_eq!((r.child, r.holds), (first, false));

        // The replacement: same name, idle and done; nothing for it is registered.
        rig.a.sessions.remove(&first);
        let (second, _a2) = rig.child("w1").await;
        rig.a.check_peer_waits(second).await;
        assert_eq!(rig.rows().await.len(), 1, "the first wait is untouched");
        assert!(rig.wakes(second).await.is_empty());

        actor.kill();
        actor.wait_for_shutdown().await;
        rig.a.check_peer_waits(first).await;
        let wakes = rig.wakes(first).await;
        assert_eq!(wakes.len(), 1, "{wakes:?}");
        assert!(
            wakes[0].starts_with("w1 is gone: it has no live actor"),
            "{}",
            wakes[0]
        );
        assert!(rig.rows().await.is_empty());
        assert!(rig.a.activity.waits.is_empty());
    }

    /// H1: the boot pass losing a child (`<child_lost>`) settles a wait on it
    /// as `not_running` in the same step; no transition is needed.
    #[tokio::test]
    async fn lost_child_settles_its_wait_at_once() {
        let mut rig = Rig::new().await;
        let (child, actor) = rig.child("w1").await;
        rig.a.running.insert(child);
        rig.wait("w1", &[Done], None).await.unwrap();
        actor.kill();
        actor.wait_for_shutdown().await;

        let mut boot = crate::supervisor::acp_resume::Boot::for_tests();
        rig.a
            .lose(
                &mut boot,
                Some(rig.parent),
                child,
                "w1".into(),
                "its cwd is gone".into(),
            )
            .await;
        let wakes = rig.wakes(child).await;
        assert_eq!(wakes.len(), 2, "{wakes:?}");
        assert!(wakes[0].contains("was not respawned"), "{}", wakes[0]);
        assert!(wakes[1].starts_with("w1 is gone:"), "{}", wakes[1]);
        assert!(rig.rows().await.is_empty());
        assert!(rig.a.activity.waits.is_empty());
    }

    /// §5.3: the deadline settles `timeout` and clears the row; a due tell
    /// for a wait already gone does nothing.
    #[tokio::test]
    async fn timeout_settles_and_clears_the_row() {
        let mut rig = Rig::new().await;
        let (child, _actor) = rig.child("w1").await;
        rig.a.running.insert(child);
        rig.wait("w1", &[Done], Some(30)).await.unwrap();
        let rows = rig.rows().await;
        assert!(rows[0].deadline.is_some());
        let key = (rig.parent, child);
        // Not due yet.
        rig.a.peer_wait_due(rig.parent, child).await;
        assert_eq!(rig.rows().await.len(), 1);
        // Move the clock: registered 30 s ago, due now.
        let now = Instant::now();
        let w = rig.a.activity.waits.get_mut(&key).unwrap();
        w.deadline = Some(now);
        w.since = now - Duration::from_secs(30);
        rig.a.peer_wait_due(rig.parent, child).await;
        let wakes = rig.wakes(child).await;
        assert_eq!(wakes, ["w1 is still running after 30 s."]);
        assert!(rig.rows().await.is_empty());
        assert!(rig.a.activity.waits.is_empty());
        rig.a.peer_wait_due(rig.parent, child).await;
        assert!(rig.wakes(child).await.is_empty());
    }

    /// §5.4 boot: a row on a respawn candidate is re-armed; a past deadline
    /// settles `timeout`; a missing child, or one that is no candidate,
    /// settles `not_running`.
    #[tokio::test]
    async fn boot_rearms_and_settles_rows() {
        let mut rig = Rig::new().await;
        let parent = rig.parent;
        let now = Utc::now();
        let row = |child: SessionId, call: &str, deadline: Option<chrono::DateTime<Utc>>| WaitRow {
            session: parent,
            tool_call_id: call.to_owned(),
            kind: WaitKind::PeerState,
            target: child.to_string(),
            deadline,
            payload: json!({
                "until": ["done"],
                "name": call,
                "since_ms": (now - chrono::Duration::seconds(70)).timestamp_millis(),
            }),
        };
        // Candidates: a `child_result` row still targets them (no actor at boot).
        let (open, timed, gone, idle) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        for (c, name) in [(open, "open"), (timed, "timed"), (idle, "idle")] {
            rig.a
                .store
                .create_session(c, "m", Some(parent), Some(name), None)
                .await
                .unwrap();
        }
        for (c, call) in [(open, "spawn-open"), (timed, "spawn-timed")] {
            super::super::child_waits::child_spawned(&mut rig.a, parent, c, call, None).await;
        }
        for r in [
            row(open, "open", None),
            row(timed, "timed", Some(now - chrono::Duration::seconds(10))),
            row(gone, "gone", None),
            row(idle, "idle", Some(now + chrono::Duration::hours(1))),
        ] {
            rig.a.store.wait_put(&r).await.unwrap();
        }
        assert_eq!(rig.rows().await.len(), 4);

        rig.a.rebuild_peer_waits().await;

        let rows = rig.rows().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].tool_call_id, "open");
        let w = &rig.a.activity.waits[&(parent, open)];
        assert_eq!(
            (w.call_id.as_str(), w.name.as_str(), w.deadline),
            ("open", "open", None)
        );
        assert_eq!(w.until, [Done]);
        assert!(
            w.since.elapsed() >= Duration::from_secs(69),
            "since restored from the row"
        );
        assert_eq!(rig.a.activity.waits.len(), 1);

        assert_eq!(
            rig.wakes(timed).await,
            ["timed was still running after 60 s (the engine restarted meanwhile)."]
        );
        assert_eq!(
            rig.wakes(gone).await,
            ["gone is gone: its session no longer exists."]
        );
        assert_eq!(
            rig.wakes(idle).await,
            ["idle is gone: it was not running when the engine restarted."]
        );
        assert!(rig.wakes(open).await.is_empty());
    }

    /// §4.2: a stall records the episode, wakes the parent once with
    /// `<child_stalled>`, shows in `stalled_secs`, and settles a `stalled`
    /// wait; the all-clear removes it. A root ACP session wakes nobody.
    #[tokio::test]
    async fn child_stalled_wakes_the_parent_and_settles_waits() {
        let mut rig = Rig::new().await;
        let (child, _actor) = rig.child("w1").await;
        rig.a.running.insert(child);
        rig.wait("w1", &[Stalled], None).await.unwrap();
        rig.a
            .child_stalled(ChildStalled {
                child,
                stall: Some(crate::activity::Stall {
                    phase: StallPhase::Silent,
                    quiet_secs: 184,
                    last: "shell: cargo check".into(),
                }),
            })
            .await;
        assert_eq!(rig.a.activity.stalled_secs(child, Instant::now()), Some(0));
        let wakes = rig.wakes(child).await;
        assert_eq!(wakes.len(), 2, "{wakes:?}");
        assert!(
            wakes[0].starts_with(
                "w1 has sent no session update for 184 s and no tool call is in progress."
            ),
            "{}",
            wakes[0]
        );
        assert!(
            wakes[1].starts_with("w1 is stalled: no session update for 0 s"),
            "{}",
            wakes[1]
        );
        assert!(rig.rows().await.is_empty());

        rig.a
            .child_stalled(ChildStalled { child, stall: None })
            .await;
        assert_eq!(rig.a.activity.stalled_secs(child, Instant::now()), None);

        // A root ACP session: recorded, nobody woken.
        rig.a.sessions.get_mut(&child).unwrap().parent = None;
        rig.a
            .child_stalled(ChildStalled {
                child,
                stall: Some(crate::activity::Stall {
                    phase: StallPhase::NoFirstUpdate,
                    quiet_secs: 60,
                    last: String::new(),
                }),
            })
            .await;
        assert!(rig.a.activity.stalls.contains_key(&child));
        assert!(rig.wakes(child).await.is_empty());
    }
}
