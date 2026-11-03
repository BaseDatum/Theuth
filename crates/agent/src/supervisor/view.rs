//! 176 E1: roster and activity reads off the mailbox. A [`View`] is the
//! actor state those replies read, cloned synchronously in the handler; the
//! store reads and the `JobCounts` ask then run in a spawned task, so a
//! trivial ask never queues behind a session-list or transcript scan. The
//! reply is the state as of the handler plus the store as of the read.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use runtime::SessionId;
use tools::{Retention, Risk};

use super::{Agents, first_line};
use crate::communication::PeerAddress;
use crate::store::Store;
use crate::{AgentInfo, AgentState, ObservedState, SessionActivity, SessionSummary};

/// What roster and activity replies read from [`Agents`].
pub(super) struct View {
    store: Store,
    /// Open private roots in `Agents::private` order: `(root, store, closing)`.
    private: Vec<(SessionId, Arc<Store>, bool)>,
    /// Private member → its root (51 §4.5).
    private_member: HashMap<SessionId, SessionId>,
    /// `(parent, name)` of every live entry that has a parent.
    parents: HashMap<SessionId, (SessionId, String)>,
    /// Live entries whose actor is alive.
    alive: HashSet<SessionId>,
    /// Unanswered approval per session: `(tool, arrived ms)`.
    approvals: HashMap<SessionId, (String, u64)>,
    /// Pending reply waits in `Agents::waits` order: `(session, awaited endpoint, since ms)`.
    waits: Vec<(SessionId, String, u64)>,
    running: HashSet<SessionId>,
    last_stop: HashMap<SessionId, (String, DateTime<Utc>)>,
    last_input: HashMap<SessionId, DateTime<Utc>>,
    /// `child_turns[id].started_at`.
    turn_started: HashMap<SessionId, DateTime<Utc>>,
    caps: HashMap<SessionId, Risk>,
}

impl Agents {
    /// The [`View`] as of now; no I/O.
    pub(super) fn view(&self) -> View {
        View {
            store: self.store.clone(),
            private: self
                .private
                .iter()
                .map(|(root, p)| (*root, p.store.clone(), p.closing))
                .collect(),
            private_member: self.private_member.clone(),
            parents: self
                .sessions
                .iter()
                .filter_map(|(id, l)| Some((*id, l.parent.clone()?)))
                .collect(),
            alive: self
                .sessions
                .iter()
                .filter(|(_, l)| l.actor.is_alive())
                .map(|(id, _)| *id)
                .collect(),
            approvals: self
                .approvals
                .iter()
                .map(|(id, (_, tool, at))| (*id, (tool.clone(), *at)))
                .collect(),
            waits: self
                .waits
                .values()
                .map(|w| (w.session, w.to.clone(), w.since_ms))
                .collect(),
            running: self.running.clone(),
            last_stop: self.last_stop.clone(),
            last_input: self.last_input.clone(),
            turn_started: self
                .child_turns
                .iter()
                .map(|(id, ct)| (*id, ct.started_at))
                .collect(),
            caps: self.caps.clone(),
        }
    }
}

/// One `JobCounts` ask for `ids`; empty when no `Jobs` actor runs.
pub(super) async fn job_counts(ids: &[SessionId]) -> HashMap<SessionId, (u16, u16)> {
    match ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR) {
        Ok(Some(jobs)) => jobs
            .ask(tools::JobCounts(ids.to_vec()))
            .await
            .unwrap_or_default(),
        _ => HashMap::new(),
    }
}

impl View {
    /// The store `id`'s tree lives in (as [`Agents::store_for`]).
    fn store_for(&self, id: SessionId) -> &Store {
        self.private_member
            .get(&id)
            .and_then(|root| self.private.iter().find(|(r, ..)| r == root))
            .map_or(&self.store, |(_, s, _)| s.as_ref())
    }

    fn retention_of(&self, id: SessionId) -> Retention {
        if self.private_member.contains_key(&id) {
            Retention::Private
        } else {
            Retention::Durable
        }
    }

    /// Every stored session row, durable store first then each open private
    /// tree's, in each store's `list_sessions` order.
    pub(super) async fn all_rows(&self) -> Vec<SessionSummary> {
        let mut rows = self.store.list_sessions().await.unwrap_or_else(|e| {
            tracing::error!("list sessions: {e:#}");
            Vec::new()
        });
        for (_, store, _) in self.private.iter().filter(|(.., closing)| !closing) {
            rows.extend(store.list_sessions().await.unwrap_or_default());
        }
        rows
    }

    /// Live state of `id` as the roster shows it (design doc 19 §2.1).
    /// `names` maps sibling ids to their peer names for `Awaiting { from }`.
    fn state_of(
        &self,
        id: SessionId,
        parent: Option<SessionId>,
        names: &HashMap<SessionId, &str>,
    ) -> AgentState {
        if let Some((tool, _)) = self.approvals.get(&id) {
            AgentState::Approval { tool: tool.clone() }
        } else if let Some((_, to, _)) = self.waits.iter().find(|(s, ..)| *s == id) {
            // The awaited peer by its convenience name when it has one.
            let from = match PeerAddress::local(to) {
                Some(p) if Some(p) == parent => "parent".to_owned(),
                Some(p) => names
                    .get(&p)
                    .map(|n| (*n).to_owned())
                    .or_else(|| self.parents.get(&p).map(|(_, n)| n.clone()))
                    .unwrap_or_else(|| to.clone()),
                None => to.clone(),
            };
            AgentState::Awaiting { from }
        } else if self.running.contains(&id) {
            AgentState::Running
        } else {
            match self.last_stop.get(&id).map(|(reason, _)| reason.as_str()) {
                Some("error") => AgentState::Error,
                Some("cancelled") => AgentState::Cancelled,
                _ => AgentState::Idle,
            }
        }
    }

    /// [`Self::state_of`] for a store row: a watch-only child's state comes
    /// from the harness (141 §5.2 table) — except an unanswered permission
    /// ask keyed to it, which is `Approval` like anyone's.
    fn state_of_row(
        &self,
        row: &SessionSummary,
        parent: Option<SessionId>,
        names: &HashMap<SessionId, &str>,
    ) -> AgentState {
        let live = self.state_of(row.id, parent, names);
        match &row.observed {
            Some(o) if !matches!(live, AgentState::Approval { .. }) => match o.state {
                ObservedState::Running => AgentState::Running,
                ObservedState::RequiresAction => AgentState::Approval {
                    tool: o.description.clone().unwrap_or_default(),
                },
                ObservedState::Failed => AgentState::Error,
                ObservedState::Cancelled => AgentState::Cancelled,
                ObservedState::Pending
                | ObservedState::Idle
                | ObservedState::Completed
                | ObservedState::Unknown => AgentState::Idle,
            },
            _ => live,
        }
    }

    /// The latest of `id`'s last input, last stop and turn start.
    fn last_active(&self, id: SessionId) -> Option<DateTime<Utc>> {
        [
            self.last_input.get(&id).copied(),
            self.last_stop.get(&id).map(|(_, at)| *at),
            self.turn_started.get(&id).copied(),
        ]
        .into_iter()
        .flatten()
        .max()
    }

    /// [`SessionActivity`] for `ids` (62 §0.5): one `JobCounts` ask for all
    /// of them, the rest from the view; the store is read only for a cold
    /// session's `updated_at`.
    pub(super) async fn activity_for(
        &self,
        ids: &[SessionId],
    ) -> HashMap<SessionId, SessionActivity> {
        let counts = job_counts(ids).await;
        self.assemble_activity(ids, &counts).await
    }

    /// [`Self::activity_for`] given the `JobCounts` reply.
    pub(super) async fn assemble_activity(
        &self,
        ids: &[SessionId],
        counts: &HashMap<SessionId, (u16, u16)>,
    ) -> HashMap<SessionId, SessionActivity> {
        let mut cold = HashMap::new();
        for &id in ids {
            if self.last_active(id).is_none()
                && let Ok(Some(s)) = self.store_for(id).session(id).await
            {
                cold.insert(id, s.updated_at);
            }
        }
        self.activity_with(ids, counts, &cold)
    }

    /// [`Self::activity_for`] over rows already read: a cold row's
    /// `last_active` is its own `updated_at`, with no further read.
    pub(super) fn activity_of_rows<'a>(
        &self,
        rows: impl IntoIterator<Item = &'a SessionSummary>,
        counts: &HashMap<SessionId, (u16, u16)>,
    ) -> HashMap<SessionId, SessionActivity> {
        let (ids, cold): (Vec<_>, HashMap<_, _>) = rows
            .into_iter()
            .map(|r| (r.id, (r.id, r.updated_at)))
            .unzip();
        self.activity_with(&ids, counts, &cold)
    }

    /// The activity rows; `cold` is the `updated_at` fallback per id.
    fn activity_with(
        &self,
        ids: &[SessionId],
        counts: &HashMap<SessionId, (u16, u16)>,
        cold: &HashMap<SessionId, DateTime<Utc>>,
    ) -> HashMap<SessionId, SessionActivity> {
        // Live descendants per ancestor: walk each live session's parent chain.
        let mut children: HashMap<SessionId, u16> = HashMap::new();
        for id in &self.alive {
            let mut cur = *id;
            while let Some((p, _)) = self.parents.get(&cur) {
                *children.entry(*p).or_default() += 1;
                cur = *p;
            }
        }
        let names = HashMap::new();
        let mut out = HashMap::with_capacity(ids.len());
        for &id in ids {
            let parent = self.parents.get(&id).map(|(p, _)| *p);
            let last_active = self.last_active(id).or_else(|| cold.get(&id).copied());
            let (jobs, ptys) = counts.get(&id).copied().unwrap_or_default();
            let state = self.state_of(id, parent, &names);
            // 130 E2: how long the user (or a peer) has been waited on.
            let waiting_since_ms = match &state {
                AgentState::Approval { .. } => self.approvals.get(&id).map(|(_, at)| *at),
                AgentState::Awaiting { .. } => self
                    .waits
                    .iter()
                    .find(|(s, ..)| *s == id)
                    .map(|(.., since)| *since),
                _ => None,
            };
            out.insert(
                id,
                SessionActivity {
                    state,
                    last_active,
                    turn_started_at: self.turn_started.get(&id).copied(),
                    jobs,
                    ptys,
                    children: children.get(&id).copied().unwrap_or_default(),
                    asks: None,
                    live: self.alive.contains(&id),
                    waiting_since_ms,
                },
            );
        }
        out
    }

    /// One roster row from its store row plus live state.
    async fn info_row(
        &self,
        store: &Store,
        row: &SessionSummary,
        depth: u8,
        parent: SessionId,
        state: AgentState,
        activity: SessionActivity,
    ) -> AgentInfo {
        let id = row.id;
        let last = store.last_turn_text(id).await.unwrap_or_else(|e| {
            tracing::error!(session = %id, "last_turn_text: {e:#}");
            String::new()
        });
        AgentInfo {
            id,
            name: row.name.clone().unwrap_or_default(),
            parent,
            depth,
            agent: row.agent.clone(),
            model: row.model.clone(),
            state,
            started_at: self.turn_started.get(&id).copied(),
            finished_at: self.last_stop.get(&id).map(|(_, at)| *at),
            last: first_line(&last),
            project: row.project.clone(),
            retention: self.retention_of(id),
            activity,
            cap: self.caps.get(&id).copied(),
            kind: row.kind,
            observed: row.observed.clone(),
        }
    }

    /// [`AgentInfo`] for every descendant of `session`, store rows in
    /// pre-order plus live state (design doc 19 §2.1).
    pub(super) async fn agents(&self, session: SessionId) -> Vec<AgentInfo> {
        let store = self.store_for(session);
        let rows = match store.descendants(session).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(session = %session, "descendants: {e:#}");
                return Vec::new();
            }
        };
        let names: HashMap<SessionId, &str> = rows
            .iter()
            .filter_map(|(_, s)| Some((s.id, s.name.as_deref()?)))
            .collect();
        let ids: Vec<SessionId> = rows.iter().map(|(_, r)| r.id).collect();
        let counts = job_counts(&ids).await;
        let mut activity = self.activity_of_rows(rows.iter().map(|(_, r)| r), &counts);
        let mut out = Vec::with_capacity(rows.len());
        for (depth, row) in &rows {
            let parent = row.parent.unwrap_or(session);
            // Names of cold siblings are known here; keep the two states equal.
            let state = self.state_of_row(row, Some(parent), &names);
            let mut activity = activity.remove(&row.id).unwrap_or_default();
            activity.state = state.clone();
            out.push(
                self.info_row(store, row, *depth, parent, state, activity)
                    .await,
            );
        }
        out
    }

    /// `ListActivity`'s rows (62 §3.2): every root, plus every session with
    /// a live actor or a running job; `depth` from its own root.
    pub(super) async fn activity_rows(&self) -> Vec<AgentInfo> {
        let rows = self.all_rows().await;
        let ids: Vec<SessionId> = rows.iter().map(|r| r.id).collect();
        let counts = job_counts(&ids).await;
        let mut activity = self.activity_of_rows(&rows, &counts);
        // Pre-order like `agents()`: roots in roster order, then each one's
        // children oldest first. A cold child is skipped but its live
        // descendants are still walked (their depth counts it).
        let mut kids: HashMap<SessionId, Vec<&SessionSummary>> = HashMap::new();
        for r in &rows {
            if let Some(p) = r.parent {
                kids.entry(p).or_default().push(r);
            }
        }
        for v in kids.values_mut() {
            v.sort_by_key(|r| r.created_at);
        }
        let mut stack: Vec<(&SessionSummary, u8)> = rows
            .iter()
            .filter(|r| r.parent.is_none())
            .rev()
            .map(|r| (r, 0))
            .collect();
        let mut out = Vec::new();
        while let Some((row, depth)) = stack.pop() {
            if let Some(children) = kids.get(&row.id) {
                stack.extend(children.iter().rev().map(|c| (*c, depth.saturating_add(1))));
            }
            let Some(a) = activity.remove(&row.id) else {
                continue;
            };
            if depth > 0 && !a.live && a.jobs + a.ptys == 0 {
                continue;
            }
            let parent = row.parent.unwrap_or(row.id);
            let state = if row.observed.is_some() {
                self.state_of_row(row, row.parent, &HashMap::new())
            } else {
                a.state.clone()
            };
            out.push(
                self.info_row(self.store_for(row.id), row, depth, parent, state, a)
                    .await,
            );
        }
        out
    }
}
