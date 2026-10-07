//! The rules (67 §7.2–7.5) over [`Store`]: transitions, checkout and
//! epochs, the event log and thread, wakes (coalesce, dispatch, back-off),
//! the cross-task write cap, the orphan sweep, disposition enforcement.
//! No actor, no runtime: the actor in `lib.rs` delegates here and tests
//! drive it with a mock clock, router and thread.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::DeskRouter;
use crate::config::TasksConfig;
use crate::packet;
use crate::store::Store;
use crate::thread::Thread;
use crate::types::{
    Checkout, DoneBecause, Owner, Task, TaskCreate, TaskEvent, TaskFilter, TaskId, TaskPatch,
    TaskState, Wake, WakeReason,
};

/// ms since the epoch, injectable for tests.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Blocker text of a task that its episode closed without a disposition (67 §7.3).
pub const NO_CONTINUATION: &str = "no continuation";

/// A blocker the system (or an agent's disposition) wrote, which a user's
/// unblock clears: `no continuation`, `engine:*`, `person:*`. A `decision:*`
/// needs its answer and a task id (no colon) its task; those stay.
pub fn system_blocker(b: &str) -> bool {
    b == NO_CONTINUATION || (b.contains(':') && !b.starts_with("decision:"))
}

/// Placeholder `claimed_by` between claim and a successful delivery when the
/// agent has no desk yet; replaced by the desk id once delivery returns.
pub const DISPATCHING: &str = "dispatching";

/// `<kind> "<title>"` — how an attachment is named in events and thread lines (the id when the title is empty).
pub fn attachment_label(r: &agent::attach::AttachRef) -> String {
    let title = if r.title.is_empty() {
        r.id.as_str()
    } else {
        r.title.as_str()
    };
    format!("{} \"{}\"", r.kind, title)
}

/// Attachments deduplicated by kind + id, first occurrence kept, blank ids dropped.
pub fn dedupe_attachments(refs: Vec<agent::attach::AttachRef>) -> Vec<agent::attach::AttachRef> {
    let mut seen = HashSet::new();
    refs.into_iter()
        .filter(|r| !r.id.trim().is_empty() && seen.insert((r.kind, r.id.clone())))
        .collect()
}

/// 68 §4.4: takes an orphaned checkout's session here before the `Resume`
/// wake, so the resumed desk continues the replicated transcript instead of
/// starting over from the progress note.
#[async_trait::async_trait]
pub trait SessionTaker: Send + Sync {
    /// `Ok` once `session` is owned here; `Err` keeps the progress-note fallback.
    async fn take(&self, session: &str) -> Result<(), String>;
}

/// The real taker: `TakeSession { reason: Orphan }` on the sessions actor.
pub struct MeshTaker;

#[async_trait::async_trait]
impl SessionTaker for MeshTaker {
    async fn take(&self, session: &str) -> Result<(), String> {
        let id = session.parse().map_err(|e| format!("{session}: {e}"))?;
        let actor =
            sessionsync::actor().ok_or_else(|| "sessions actor is not running".to_owned())?;
        actor
            .ask(sessionsync::TakeSession {
                id,
                reason: sessionsync::TakeReason::Orphan,
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// What the sweep knows about a checkout's session before calling it stale
/// (WAKE-LIVE-CHILD D4): whether it or any descendant works right now, and
/// when any of them was last active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Liveness {
    /// A turn, job, PTY or child is running now.
    pub active_now: bool,
    /// Newest `last_active` over the session and its descendants (epoch ms).
    pub last_active_ms: Option<u64>,
}

/// Answers [`Liveness`] for a session; `None` = unknown (no agent actor, a
/// session this engine never saw), which keeps the progress-note rule alone.
#[async_trait::async_trait]
pub trait LivenessProbe: Send + Sync {
    /// Liveness of `session` and its subtree.
    async fn liveness(&self, session: &str) -> Option<Liveness>;
}

/// One checkout's probe answer, pinned to the checkout it was asked for so
/// [`Ledger::sweep_apply`] can tell a checkout that moved on meanwhile.
#[derive(Clone, Debug)]
pub struct Probed {
    /// `Checkout::session` when probed.
    pub session: String,
    /// `Checkout::epoch` when probed.
    pub epoch: u32,
    /// The probe's answer.
    pub live: Option<Liveness>,
}

/// The asking half of the orphan sweep ([`Ledger::prober`]). It runs off the
/// tasks mailbox: the probe asks the agent actor, which may be asking tasks
/// back at that moment (`Identities::identity` → `ListTasks`).
#[derive(Clone)]
pub struct SweepProber {
    store: Store,
    probe: Arc<dyn LivenessProbe>,
    stale_ms: u64,
}

impl SweepProber {
    /// Probes every `doing` checkout on a reachable (or unknown) engine
    /// whose progress note is already stale at `now`; the rest need no answer.
    pub async fn probe(
        &self,
        now: u64,
        reachable: Option<&HashSet<String>>,
    ) -> R<HashMap<TaskId, Probed>> {
        let doing = self
            .store
            .list(&TaskFilter {
                state: Some(vec![TaskState::Doing]),
                ..Default::default()
            })
            .await
            .map_err(db)?;
        let mut out = HashMap::new();
        for t in doing {
            let Some(c) = t.checkout else { continue };
            if c.orphaned
                || reachable.is_some_and(|r| !r.contains(&c.engine))
                || now.saturating_sub(c.progress_at_ms) < self.stale_ms
            {
                continue;
            }
            let live = self.probe.liveness(&c.session).await;
            out.insert(
                t.id,
                Probed {
                    session: c.session,
                    epoch: c.epoch,
                    live,
                },
            );
        }
        Ok(out)
    }
}

/// One claimed wake the actor must deliver off the mailbox.
pub struct Delivery {
    /// The wake, `claimed_by` set.
    pub wake: Wake,
    /// Its task.
    pub task: Option<Task>,
}

/// The rules over the store.
pub struct Ledger {
    /// Rows.
    pub store: Store,
    /// Knobs.
    pub cfg: TasksConfig,
    /// The roster's desks, once registered.
    pub router: Option<Arc<dyn DeskRouter>>,
    /// Thread documents.
    pub thread: Arc<dyn Thread>,
    /// Now.
    pub clock: Clock,
    /// This engine, written into every checkout.
    pub engine: String,
    /// Takes a lost engine's session here before its `Resume` wake (68 §4.4).
    pub taker: Arc<dyn SessionTaker>,
    /// Whether a checkout's session still works (WAKE-LIVE-CHILD D4).
    pub probe: Arc<dyn LivenessProbe>,
    /// `(agent, task)` → `updated_ms` of the task when the agent's wake on it
    /// was claimed: the no-progress comparison point (67 §7.3). Local: a hub
    /// move restarts the count.
    delivered: HashMap<(String, TaskId), u64>,
    /// Delivery failures per wake id: `(attempts, not before ms)`.
    retry: HashMap<String, (u32, u64)>,
    /// Engine → first sweep that saw it unreachable (ms).
    unreachable_since: HashMap<String, u64>,
    /// Agent → the claimed wake id its waiting wakes were last reported held
    /// behind (WAKE-LIVE-CHILD D3): one log line per blocking wake.
    reported_busy: HashMap<String, String>,
}

type R<T> = Result<T, String>;

fn db(e: anyhow::Error) -> String {
    format!("tasks store: {e:#}")
}

/// The built-in default agent (`roster::types::AgentName::DEFAULT`; tasks must
/// not depend on roster): its desk and every descendant are uncapped (99 D1).
const UNCAPPED_AGENT: &str = "theuth";

/// A window in seconds as `{n} h` | `{n} min` | `{n} s` for error and inbox text (99 D5).
pub fn window_text(secs: u64) -> String {
    if secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// `actor` may act as `who`: the user (and the harness) may act as anyone.
fn acts_for(actor: &str, who: &Owner) -> bool {
    actor == "user" || actor == "system" || actor == who.to_string()
}

fn short(id: &TaskId) -> &str {
    id.0.get(..8).unwrap_or(&id.0)
}

fn agent_name(o: &Owner) -> Option<&str> {
    match o {
        Owner::Agent(a) => Some(a),
        _ => None,
    }
}

fn stamp(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn checkout_text(c: &Checkout) -> String {
    format!(
        "{}@{}#{}{}",
        c.session,
        c.engine,
        c.epoch,
        if c.orphaned { " (orphaned)" } else { "" }
    )
}

impl Ledger {
    /// A ledger over `store`.
    pub fn new(
        store: Store,
        cfg: TasksConfig,
        thread: Arc<dyn Thread>,
        clock: Clock,
        engine: String,
    ) -> Self {
        Self {
            store,
            cfg,
            router: None,
            thread,
            clock,
            engine,
            taker: Arc::new(MeshTaker),
            probe: Arc::new(crate::AgentProbe),
            delivered: HashMap::new(),
            retry: HashMap::new(),
            unreachable_since: HashMap::new(),
            reported_busy: HashMap::new(),
        }
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// The agent behind `session`, per the roster.
    pub fn agent_for(&self, session: &str) -> Option<String> {
        self.router.as_ref().and_then(|r| r.agent_for(session))
    }

    /// `agent`'s manager as an owner: the roster's answer, else the user.
    pub fn manager_of(&self, agent: &str) -> Owner {
        self.router
            .as_ref()
            .and_then(|r| r.manager_of(agent))
            .map_or(Owner::User, Owner::Agent)
    }

    // ---- events, thread, wakes -------------------------------------------

    async fn event(
        &self,
        t: &Task,
        actor: &str,
        session: Option<&str>,
        field: &str,
        before: Option<String>,
        after: Option<String>,
    ) {
        let e = TaskEvent {
            at_ms: self.now(),
            actor: actor.to_owned(),
            session: session.map(str::to_owned),
            field: field.to_owned(),
            before,
            after,
        };
        let line = match (&e.before, &e.after) {
            (Some(b), Some(a)) => format!("- {} **{actor}** {field}: {b} → {a}", stamp(e.at_ms)),
            (None, Some(a)) => format!("- {} **{actor}** {field}: {a}", stamp(e.at_ms)),
            (Some(b), None) => format!("- {} **{actor}** {field}: {b} → ∅", stamp(e.at_ms)),
            (None, None) => format!("- {} **{actor}** {field}", stamp(e.at_ms)),
        };
        self.record(t, &e, &line).await;
    }

    /// An event row plus its thread line.
    async fn record(&self, t: &Task, e: &TaskEvent, line: &str) {
        if let Err(err) = self.store.add_event(&t.id, e).await {
            tracing::error!(task = %t.id, "event not recorded: {err:#}");
        }
        if let Some(doc) = &t.thread {
            self.thread.append(doc, line).await;
        }
    }

    async fn say(&self, t: &Task, actor: &str, text: &str) {
        if let Some(doc) = &t.thread {
            self.thread
                .append(doc, &format!("**{actor}** ({}): {text}", stamp(self.now())))
                .await;
        }
    }

    /// Raise a wake for `who` about `t` (nothing for the user or the harness)
    /// and record it: a `wake` event and a `woke <agent> (<reason>)` thread line.
    /// A reason with no purpose in the task's state (STALE-WAKE-DESKS:
    /// a `comment` on a done task) raises nothing.
    pub async fn wake(&self, t: &Task, who: &Owner, reason: WakeReason, actor: &str) {
        let Some(agent) = agent_name(who) else { return };
        if !reason.applies_to(t.state) {
            tracing::debug!(agent, task = %t.id, reason = reason.as_str(), state = t.state.as_str(), "wake not raised: stale");
            return;
        }
        if let Err(e) = self
            .store
            .raise(agent, Some(&t.id), reason, self.now())
            .await
        {
            tracing::error!(agent, "wake not raised: {e:#}");
            return;
        }
        let e = TaskEvent {
            at_ms: self.now(),
            actor: actor.to_owned(),
            session: None,
            field: "wake".to_owned(),
            before: None,
            after: Some(agent.to_owned()),
        };
        let line = format!(
            "- {} **{actor}** woke {agent} ({})",
            stamp(e.at_ms),
            reason.as_str()
        );
        self.record(t, &e, &line).await;
    }

    /// Fill `blocks` (the inverse of `blockers`, computed).
    async fn with_blocks(&self, mut t: Task) -> R<Task> {
        t.blocks = self.blocks_of(&t.id).await?;
        Ok(t)
    }

    /// Tasks whose blockers name `id`.
    pub async fn blocks_of(&self, id: &TaskId) -> R<Vec<TaskId>> {
        Ok(self
            .store
            .blocked_on(&id.0)
            .await
            .map_err(db)?
            .into_iter()
            .map(|t| t.id)
            .collect())
    }

    async fn load(&self, id: &TaskId) -> R<Task> {
        self.store
            .get(id)
            .await
            .map_err(db)?
            .ok_or_else(|| "no such task".to_owned())
    }

    /// Write the row; wakes whose reason has no purpose in the saved state go
    /// with it (STALE-WAKE-DESKS: every state transition routes through here,
    /// so a `review` wake dies with `review → done`, whoever moved it).
    async fn save(&self, t: &Task) -> R<()> {
        self.store.put(t).await.map_err(db)?;
        match self.store.drop_stale_wakes(&t.id, t.state).await {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(task = %t.id, state = t.state.as_str(), n, "stale wakes dropped")
            }
            Err(e) => tracing::error!(task = %t.id, "stale wakes not dropped: {e:#}"),
        }
        Ok(())
    }

    /// The cross-task write cap (67 §7.3, 99 WRITE-CAP-LINE): a hired agent's
    /// session changing a task outside its reporting line spends one of
    /// `cross_task_write_cap` distinct tasks within the window. theuth's
    /// desk, the user's own sessions and everything either spawns are
    /// uncapped (D1).
    async fn charge(&self, session: Option<&str>, t: &Task) -> R<()> {
        let Some(session) = session else {
            return Ok(());
        };
        let Some(agent) = self.capped_agent(session) else {
            return Ok(());
        };
        let mine = t.checkout.as_ref().is_some_and(|c| c.session == session)
            || self.in_line(&agent, &t.owner);
        if mine {
            return Ok(());
        }
        let cap = self.cfg.cross_task_write_cap;
        let window_secs = self.cfg.cross_task_write_window_secs;
        let c = self
            .store
            .budget_write(session, &t.id, cap, self.now(), window_secs * 1000)
            .await
            .map_err(db)?;
        if c.allowed {
            return Ok(());
        }
        let when = if window_secs == 0 {
            "for this session's lifetime".to_owned()
        } else {
            format!("in the last {}", window_text(window_secs))
        };
        Err(format!(
            "tasks.cross_task_write_cap: this session has written to {} tasks outside {agent}'s \
             reporting line {when}; the cap is {cap}. Wait for older writes to age out, delegate \
             with task_create, or ask the owner's manager to make the change.",
            c.used
        ))
    }

    /// The hired agent behind `session` when the cap applies to it; `None`
    /// for the user's own sessions and theuth's line (D1).
    fn capped_agent(&self, session: &str) -> Option<String> {
        self.agent_for(session).filter(|a| a != UNCAPPED_AGENT)
    }

    /// `owner` is `agent` or reports to it, transitively (D2). Bounded:
    /// `reports_to` can loop.
    fn in_line(&self, agent: &str, owner: &Owner) -> bool {
        let Some(mut o) = agent_name(owner).map(str::to_owned) else {
            return false;
        };
        let mut seen = HashSet::new();
        for _ in 0..32 {
            if o == agent {
                return true;
            }
            if !seen.insert(o.clone()) {
                return false;
            }
            match self.manager_of(&o) {
                Owner::Agent(m) => o = m,
                _ => return false,
            }
        }
        false
    }

    /// Where `session` stands against the cap (D6); `None` = uncapped.
    pub async fn write_budget(&self, session: &str) -> R<Option<crate::WriteBudget>> {
        if self.capped_agent(session).is_none() {
            return Ok(None);
        }
        let window_secs = self.cfg.cross_task_write_window_secs;
        let used = self
            .store
            .budget_used(session, self.now(), window_secs * 1000)
            .await
            .map_err(db)?;
        Ok(Some(crate::WriteBudget {
            used,
            cap: self.cfg.cross_task_write_cap,
            window_secs,
        }))
    }

    /// 67 §7.5: a session that once held the checkout but no longer does was
    /// superseded — its cue to stop.
    async fn check_epoch(&self, session: Option<&str>, t: &Task) -> R<()> {
        let (Some(session), Some(c)) = (session, &t.checkout) else {
            return Ok(());
        };
        if c.session == session {
            return Ok(());
        }
        let held = self.store.events(&t.id).await.map_err(db)?.iter().any(|e| {
            e.field == "checkout" && e.after.as_deref().is_some_and(|a| a.starts_with(session))
        });
        if held {
            return Err(format!(
                "superseded: task {} is now checked out by {} on {} (epoch {}); stop, your work is stale",
                short(&t.id),
                c.session,
                c.engine,
                c.epoch
            ));
        }
        Ok(())
    }

    // ---- records -----------------------------------------------------------

    /// One task with `blocks`.
    pub async fn get(&self, id: &TaskId) -> R<Task> {
        let t = self.load(id).await?;
        self.with_blocks(t).await
    }

    /// Tasks by filter, project then oldest first.
    pub async fn list(&self, f: &TaskFilter) -> R<Vec<Task>> {
        self.store.list(f).await.map_err(db)
    }

    /// Events, oldest first.
    pub async fn events(&self, id: &TaskId) -> R<Vec<TaskEvent>> {
        self.store.events(id).await.map_err(db)
    }

    /// Create (67 §7.1): reviewer defaults to the owner's manager (the user
    /// when none), project to the parent's; a thread document. `todo` wakes
    /// the owner with `wake`; `backlog` wakes nobody (not dispatched).
    pub async fn create(
        &self,
        input: TaskCreate,
        created_by: &str,
        session: Option<&str>,
        wake: WakeReason,
    ) -> R<Task> {
        if !matches!(input.state, TaskState::Backlog | TaskState::Todo) {
            return Err(format!(
                "a task is created in backlog or todo, not {}",
                input.state.as_str()
            ));
        }
        let now = self.now();
        let parent = match &input.parent {
            Some(p) => Some(
                self.load(p)
                    .await
                    .map_err(|_| format!("parent {p}: no such task"))?,
            ),
            None => None,
        };
        let reviewer = input.reviewer.or_else(|| match &input.owner {
            Owner::Agent(a) => Some(self.manager_of(a)),
            _ => Some(Owner::User),
        });
        let id = TaskId::new();
        let mut t = Task {
            title: input.title,
            brief: input.brief,
            owner: input.owner,
            state: input.state,
            reviewer,
            blockers: vec![],
            blocks: vec![],
            parent: input.parent,
            project: input.project.or_else(|| parent.and_then(|p| p.project)),
            where_: input.where_,
            harness: input.harness,
            external: input.external,
            thread: None,
            checkout: None,
            schedule: None,
            created_by: created_by.to_owned(),
            created_ms: now,
            updated_ms: now,
            no_progress_count: 0,
            next_wake_after_ms: None,
            done_when: input
                .done_when
                .map(|d| d.trim().to_owned())
                .filter(|d| !d.is_empty()),
            attachments: dedupe_attachments(input.attachments),
            id,
        };
        t.thread = self
            .thread
            .create(
                &t.id.0,
                &format!("Task {}: {}", short(&t.id), t.title),
                &t.brief,
            )
            .await;
        self.save(&t).await?;
        self.event(
            &t,
            created_by,
            session,
            "created",
            None,
            Some(format!("owner {}", t.owner)),
        )
        .await;
        if t.state == TaskState::Todo {
            self.wake(&t, &t.owner, wake, created_by).await;
        }
        Ok(t)
    }

    /// Create the task of a schedule fire, or re-raise on the one that exists (67 §7.4).
    pub async fn create_for_schedule(
        &self,
        schedule: &str,
        fire_ms: u64,
        agent: &str,
        prompt: &str,
        project: Option<String>,
    ) -> R<Task> {
        if let Some(t) = self
            .store
            .by_schedule(schedule, fire_ms)
            .await
            .map_err(db)?
        {
            let w = self
                .store
                .raise(agent, Some(&t.id), WakeReason::Schedule, self.now())
                .await
                .map_err(db)?;
            self.say(
                &t,
                "system",
                &format!("schedule `{schedule}` fired again ({}×)", w.count),
            )
            .await;
            tracing::info!(schedule, agent, task = %t.id, count = w.count, "schedule fire: wake re-raised on the existing task");
            return Ok(t);
        }
        let input = TaskCreate {
            state: TaskState::Todo,
            title: format!("{schedule} @ {}", stamp(fire_ms)),
            brief: prompt.to_owned(),
            owner: Owner::Agent(agent.to_owned()),
            reviewer: None,
            parent: None,
            project,
            where_: None,
            harness: None,
            external: vec![],
            done_when: None,
            attachments: vec![],
        };
        let mut t = self
            .create(input, "system", None, WakeReason::Schedule)
            .await?;
        t.schedule = Some((schedule.to_owned(), fire_ms));
        self.save(&t).await?;
        tracing::info!(schedule, agent, task = %t.id, "schedule fire: task created, wake raised");
        Ok(t)
    }

    /// The schedule's `until` passed (67 §7.4): its open tasks are `done`,
    /// reviewer `system`.
    pub async fn close_for_schedule(&self, schedule: &str, why: &str) -> R<()> {
        let open: Vec<Task> = self
            .store
            .list(&TaskFilter::default())
            .await
            .map_err(db)?
            .into_iter()
            .filter(|t| {
                t.schedule.as_ref().is_some_and(|s| s.0 == schedule)
                    && !matches!(t.state, TaskState::Done | TaskState::Cancelled)
            })
            .collect();
        for mut t in open {
            let before = t.state;
            t.reviewer = Some(Owner::System);
            t.state = TaskState::Done;
            t.updated_ms = self.now();
            self.save(&t).await?;
            self.event(
                &t,
                "system",
                None,
                "state",
                Some(before.as_str().into()),
                Some("done".into()),
            )
            .await;
            self.say(&t, "system", why).await;
            self.on_closed(&t).await?;
        }
        Ok(())
    }

    // ---- blockers ------------------------------------------------------------

    /// Refuse a `blockers` list that would put `id` on a cycle: walk the
    /// blocker graph from each task-id entry; reaching `id` names the path.
    async fn check_cycle(&self, id: &TaskId, blockers: &[String]) -> R<()> {
        // ponytail: DFS with one `get` per hop; fine for boards of hundreds.
        let mut stack: Vec<(TaskId, Vec<String>)> = blockers
            .iter()
            .filter(|b| !b.contains(':'))
            .map(|b| (TaskId(b.clone()), vec![short(id).to_owned()]))
            .collect();
        let mut seen = HashSet::new();
        while let Some((cur, path)) = stack.pop() {
            let mut path = path;
            path.push(short(&cur).to_owned());
            if cur == *id {
                return Err(format!("blockers would form a cycle: {}", path.join(" → ")));
            }
            if !seen.insert(cur.clone()) {
                continue;
            }
            let Some(t) = self.store.get(&cur).await.map_err(db)? else {
                continue;
            };
            for b in t.blockers.iter().filter(|b| !b.contains(':')) {
                stack.push((TaskId(b.clone()), path.clone()));
            }
        }
        Ok(())
    }

    /// `t` left its blocked state: back to `doing` under a live checkout, else `todo`.
    async fn unblock(&self, t: &mut Task, actor: &str, reason: WakeReason) -> R<()> {
        let to = match &t.checkout {
            Some(c) if !c.orphaned => TaskState::Doing,
            _ => TaskState::Todo,
        };
        let before = t.state;
        t.state = to;
        t.updated_ms = self.now();
        self.save(t).await?;
        self.event(
            t,
            actor,
            None,
            "state",
            Some(before.as_str().into()),
            Some(to.as_str().into()),
        )
        .await;
        self.wake(t, &t.owner, reason, actor).await;
        Ok(())
    }

    /// `t` reached `done`/`cancelled` (70 DESK-PROTOCOL (c): results come
    /// back as wakes, never by polling): drop it from every blockers list — a
    /// task left with none returns to its previous state and its owner is
    /// woken `blockers_resolved`; when every child of its parent is now
    /// closed, the parent's owner is woken `children_completed`.
    async fn on_closed(&self, t: &Task) -> R<()> {
        for mut b in self.store.blocked_on(&t.id.0).await.map_err(db)? {
            let before = b.blockers.join(", ");
            b.blockers.retain(|x| *x != t.id.0);
            b.updated_ms = self.now();
            self.save(&b).await?;
            self.event(
                &b,
                "system",
                None,
                "blockers",
                Some(before),
                Some(b.blockers.join(", ")),
            )
            .await;
            if b.state == TaskState::Blocked && b.blockers.is_empty() {
                self.unblock(&mut b, "system", WakeReason::BlockersResolved)
                    .await?;
            }
        }
        if let Some(p) = &t.parent
            && let Ok(parent) = self.load(p).await
            && !matches!(parent.state, TaskState::Done | TaskState::Cancelled)
        {
            let open = self
                .store
                .children(p)
                .await
                .map_err(db)?
                .into_iter()
                .any(|c| {
                    c.id != t.id && !matches!(c.state, TaskState::Done | TaskState::Cancelled)
                });
            if !open {
                self.say(
                    &parent,
                    "system",
                    &format!(
                        "every child is closed (last: {} {})",
                        short(&t.id),
                        t.state.as_str()
                    ),
                )
                .await;
                self.wake(
                    &parent,
                    &parent.owner,
                    WakeReason::ChildrenCompleted,
                    "system",
                )
                .await;
            }
        }
        Ok(())
    }

    // ---- transitions -----------------------------------------------------------

    /// 67 §7.2. `blockers` is the list as it will be after this update.
    fn guard(t: &Task, to: TaskState, actor: &str, blockers: &[String]) -> R<()> {
        use TaskState::*;
        let from = t.state;
        if matches!(from, Done | Cancelled) {
            return Err(format!("task is {}", from.as_str()));
        }
        match (from, to) {
            (_, Cancelled) => Ok(()),
            (Backlog, Todo) | (Todo, Backlog) => Ok(()),
            (Backlog, _) => Err("a backlog task is started first (todo), then checked out".into()),
            (Todo, Doing) => Err("todo → doing is task_checkout".into()),
            (Review, Doing | Todo) => {
                let r = t.reviewer.clone().unwrap_or(Owner::User);
                if acts_for(actor, &r) {
                    Ok(())
                } else {
                    Err(format!("send back is the reviewer's ({r})"))
                }
            }
            (Blocked, Doing | Todo) => {
                if !blockers.is_empty() {
                    let hint = if blockers.iter().any(|b| b.starts_with("decision:")) {
                        "; a decision needs its answer"
                    } else {
                        ""
                    };
                    Err(format!("still blocked on {}{hint}", blockers.join(", ")))
                } else if acts_for(actor, &t.owner) {
                    Ok(())
                } else {
                    Err(format!(
                        "only the owner ({}) resumes a blocked task",
                        t.owner
                    ))
                }
            }
            (Doing, Review) => {
                if acts_for(actor, &t.owner) {
                    Ok(())
                } else {
                    Err(format!("review is the owner's ({}) move", t.owner))
                }
            }
            (Doing | Todo, Blocked) => {
                if blockers.is_empty() {
                    Err("blocked needs a blockers entry".into())
                } else {
                    Ok(())
                }
            }
            (Review, Done) => {
                let r = t.reviewer.clone().unwrap_or(Owner::User);
                // 70 DESK-PROTOCOL: "assigning yourself plus a please-review comment is
                // not a review path" — self-review only against explicit criteria.
                if r == t.owner && t.owner != Owner::User && t.done_when.is_none() {
                    return Err("done needs a reviewer other than the owner (self-review is only allowed against an explicit done_when)".into());
                }
                if acts_for(actor, &r) {
                    Ok(())
                } else {
                    Err(format!("done is the reviewer's ({r}) move"))
                }
            }
            _ => Err(format!(
                "{} → {} is not a transition",
                from.as_str(),
                to.as_str()
            )),
        }
    }

    /// Apply `patch` (67 §7.2 guards), log every changed field, wake whoever
    /// the change concerns.
    pub async fn update(
        &mut self,
        id: &TaskId,
        patch: TaskPatch,
        actor: &str,
        session: Option<&str>,
    ) -> R<Task> {
        let mut t = self.load(id).await?;
        self.charge(session, &t).await?;
        self.check_epoch(session, &t).await?;
        let before = t.clone();

        if let Some(b) = &patch.blockers
            && *b != t.blockers
        {
            self.check_cycle(id, b).await?;
        }
        let blockers = patch.blockers.clone().unwrap_or_else(|| t.blockers.clone());
        if let Some(to) = patch.state
            && to != t.state
        {
            Self::guard(&t, to, actor, &blockers)?;
        }
        // 70 DESK-PROTOCOL: `done` says why, judged against `done_when` first;
        // a self-reviewer may only claim the criteria. `user`/`system` default to intent.
        let done_because = match (patch.state, &patch.done_because) {
            (Some(TaskState::Done), Some(b)) if t.state != TaskState::Done => {
                if t.reviewer.as_ref().is_some_and(|r| *r == t.owner)
                    && t.owner != Owner::User
                    && *b != DoneBecause::CriteriaMet
                {
                    return Err(
                        "a self-reviewed task is done only because its done_when criteria are met"
                            .into(),
                    );
                }
                if *b == DoneBecause::CriteriaMet
                    && t.done_when.is_none()
                    && patch.done_when.is_none()
                {
                    return Err("criteria met needs a done_when on the task".into());
                }
                Some(b.clone())
            }
            (Some(TaskState::Done), None) if t.state != TaskState::Done => {
                if actor.starts_with("agent:") {
                    return Err("done needs done_because: criteria_met | intent_met | follow_ups_filed{tasks}".into());
                }
                Some(DoneBecause::IntentMet)
            }
            _ => None,
        };
        if let Some(DoneBecause::FollowUpsFiled { tasks }) = &done_because {
            if tasks.is_empty() {
                return Err("follow_ups_filed names the follow-up tasks".into());
            }
            for f in tasks {
                self.load(f)
                    .await
                    .map_err(|_| format!("follow-up {f}: no such task"))?;
            }
        }

        let mut changes: Vec<(&str, Option<String>, Option<String>)> = vec![];
        macro_rules! set {
            ($field:ident, $name:literal) => {
                if let Some(v) = patch.$field
                    && Some(&v) != t.$field.as_ref()
                {
                    changes.push((
                        $name,
                        t.$field.as_ref().map(|x| format!("{x}")),
                        Some(format!("{v}")),
                    ));
                    t.$field = Some(v);
                }
            };
        }
        if let Some(v) = patch.title
            && v != t.title
        {
            changes.push(("title", Some(t.title.clone()), Some(v.clone())));
            t.title = v;
        }
        if let Some(v) = patch.brief
            && v != t.brief
        {
            changes.push(("brief", None, Some("edited".into())));
            t.brief = v;
        }
        if let Some(v) = patch.owner
            && v != t.owner
        {
            changes.push(("owner", Some(t.owner.to_string()), Some(v.to_string())));
            t.owner = v;
        }
        set!(reviewer, "reviewer");
        set!(where_, "where");
        set!(project, "project");
        set!(harness, "harness");
        if let Some(v) = patch.blockers
            && v != t.blockers
        {
            changes.push(("blockers", Some(t.blockers.join(", ")), Some(v.join(", "))));
            t.blockers = v;
        }
        if let Some(v) = patch.external
            && v != t.external
        {
            changes.push(("external", Some(t.external.join(", ")), Some(v.join(", "))));
            t.external = v;
        }
        if let Some(v) = patch.attachments {
            let v = dedupe_attachments(v);
            if v != t.attachments {
                for r in t.attachments.iter().filter(|r| !v.contains(r)) {
                    changes.push(("detached", None, Some(attachment_label(r))));
                }
                for r in v.iter().filter(|r| !t.attachments.contains(r)) {
                    changes.push(("attached", None, Some(attachment_label(r))));
                }
                t.attachments = v;
            }
        }
        set!(done_when, "done_when");
        if let Some(to) = patch.state
            && to != t.state
        {
            changes.push((
                "state",
                Some(t.state.as_str().into()),
                Some(to.as_str().into()),
            ));
            t.state = to;
            if to == TaskState::Review || to == TaskState::Cancelled {
                t.checkout = None;
            }
            // 70 DESK-PROTOCOL: without explicit criteria a desk cannot review itself;
            // the review goes to its manager.
            if to == TaskState::Review
                && t.done_when.is_none()
                && let Owner::Agent(a) = &t.owner
                && t.reviewer.as_ref().is_none_or(|r| *r == t.owner)
            {
                let m = self.manager_of(a);
                changes.push((
                    "reviewer",
                    t.reviewer.as_ref().map(ToString::to_string),
                    Some(m.to_string()),
                ));
                t.reviewer = Some(m);
            }
        }

        // 67 §7.3 back-off: the acting agent's write after its wake is that wake's
        // completion — the owner's, or a reviewer's (STALE-WAKE-DESKS: a reviewer's
        // `review → done` used to leave its wake claimed for ever). Only the
        // owner's unmoved write counts as no progress.
        let mut completed = false;
        if let Some(agent) = actor.strip_prefix("agent:")
            && let Some(at) = self.delivered.remove(&(agent.to_owned(), id.clone()))
        {
            let owner = actor == before.owner.to_string();
            self.complete_wake(
                &mut t,
                agent,
                owner && at == before.updated_ms && changes.is_empty(),
            )
            .await;
            completed = true;
        }

        if changes.is_empty() {
            if completed {
                self.save(&t).await?;
            }
            return self.with_blocks(t).await;
        }
        if actor == "user" {
            // A human touched the task: the back-off is over, like a human comment.
            t.no_progress_count = 0;
            t.next_wake_after_ms = None;
        }
        t.updated_ms = self.now();
        self.save(&t).await?;
        for (field, b, a) in changes {
            self.event(&t, actor, session, field, b, a).await;
        }
        if let Some(b) = &done_because {
            self.say(&t, actor, &format!("done because: {b}")).await;
        }

        if t.owner != before.owner {
            self.wake(&t, &t.owner, WakeReason::Assigned, actor).await;
        }
        if t.state != before.state {
            match t.state {
                TaskState::Review => {
                    if let Some(r) = &t.reviewer {
                        self.wake(&t, r, WakeReason::Review, actor).await;
                    }
                }
                TaskState::Doing | TaskState::Todo
                    if matches!(before.state, TaskState::Review | TaskState::Blocked) =>
                {
                    self.wake(&t, &t.owner, WakeReason::Resumed, actor).await;
                }
                TaskState::Todo if before.state == TaskState::Backlog => {
                    self.wake(&t, &t.owner, WakeReason::Assigned, actor).await;
                }
                TaskState::Done | TaskState::Cancelled => self.on_closed(&t).await?,
                _ => {}
            }
        } else if t.state == TaskState::Blocked
            && t.blockers.is_empty()
            && !before.blockers.is_empty()
        {
            // Blockers cleared by hand (a decision answered): back to work.
            self.unblock(&mut t, actor, WakeReason::Answer).await?;
        }
        self.with_blocks(t).await
    }

    /// The states `actor` may move `t` to right now (67 §7.2 guards, current blockers).
    pub fn allowed_moves(t: &Task, actor: &str) -> Vec<TaskState> {
        use TaskState::*;
        [Backlog, Todo, Doing, Review, Blocked, Done, Cancelled]
            .into_iter()
            .filter(|&to| to != t.state && Self::guard(t, to, actor, &t.blockers).is_ok())
            .collect()
    }

    /// `task_move`: a state change on `actor`'s intent (backlog ↔ todo, → cancelled,
    /// review → done | todo, blocked → todo …). Refuses with the allowed list;
    /// `note` goes to the thread as a comment; `done_because` is the verdict a
    /// move to `done` records (70 DESK-PROTOCOL).
    pub async fn move_to(
        &mut self,
        id: &TaskId,
        to: TaskState,
        note: Option<&str>,
        done_because: Option<DoneBecause>,
        actor: &str,
        session: Option<&str>,
    ) -> R<Task> {
        let mut t = self.load(id).await?;
        // A user's blocked → todo|doing is an unblock: the system-owned blockers go
        // first (`no continuation`, `engine:*`, `person:*`); a `decision:*` or a task
        // blocker stays and the refusal names it. One place for the board, the
        // Attention card and the chat tool.
        let mut blockers = None;
        if actor == "user"
            && t.state == TaskState::Blocked
            && matches!(to, TaskState::Todo | TaskState::Doing)
        {
            let kept: Vec<String> = t
                .blockers
                .iter()
                .filter(|b| !system_blocker(b))
                .cloned()
                .collect();
            if kept.len() != t.blockers.len() {
                t.blockers = kept.clone();
                blockers = Some(kept);
            }
        }
        if to != t.state
            && let Err(why) = Self::guard(&t, to, actor, &t.blockers)
        {
            let allowed: Vec<_> = Self::allowed_moves(&t, actor)
                .into_iter()
                .map(TaskState::as_str)
                .collect();
            return Err(format!(
                "{} → {} is not allowed ({why}); from {} you may move to: {}",
                t.state.as_str(),
                to.as_str(),
                t.state.as_str(),
                if allowed.is_empty() {
                    "nothing".to_owned()
                } else {
                    allowed.join(", ")
                }
            ));
        }
        let t = self
            .update(
                id,
                TaskPatch {
                    state: Some(to),
                    blockers,
                    done_because,
                    ..Default::default()
                },
                actor,
                session,
            )
            .await?;
        if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
            self.comment(id, n, false, actor, session).await?;
        }
        Ok(t)
    }

    /// `task_reassign` (70 DESK-PROTOCOL 1, the fit check): the owner (or the
    /// user) hands a `todo`/`doing`/`blocked` task to `to`; a checkout is
    /// released and the task returns to `todo` (or stays `blocked`), the
    /// thread says `reassigned to <to>: <why>`, and `to` is woken `reassigned`.
    pub async fn reassign(
        &mut self,
        id: &TaskId,
        to: Owner,
        why: &str,
        actor: &str,
        session: Option<&str>,
    ) -> R<Task> {
        let mut t = self.load(id).await?;
        if !acts_for(actor, &t.owner) {
            return Err(format!(
                "only the owner ({}) or the user reassigns",
                t.owner
            ));
        }
        if !matches!(
            t.state,
            TaskState::Todo | TaskState::Doing | TaskState::Blocked
        ) {
            return Err(format!("a {} task is not reassigned", t.state.as_str()));
        }
        if to == t.owner {
            return Err(format!("{to} already owns it"));
        }
        let why = why.trim();
        if why.is_empty() {
            return Err("reassign says why (the fit)".into());
        }
        self.charge(session, &t).await?;
        self.check_epoch(session, &t).await?;
        let before = t.clone();
        t.owner = to.clone();
        t.checkout = None;
        if t.state == TaskState::Doing {
            t.state = TaskState::Todo;
        }
        if t.reviewer.as_ref() == Some(&to) {
            t.reviewer = Some(match &to {
                Owner::Agent(a) => self.manager_of(a),
                _ => Owner::User,
            });
        }
        t.updated_ms = self.now();
        if let Some(agent) = actor.strip_prefix("agent:")
            && self
                .delivered
                .remove(&(agent.to_owned(), id.clone()))
                .is_some()
        {
            self.complete_wake(&mut t, agent, false).await;
        }
        self.save(&t).await?;
        self.event(
            &t,
            actor,
            session,
            "owner",
            Some(before.owner.to_string()),
            Some(to.to_string()),
        )
        .await;
        if t.state != before.state {
            self.event(
                &t,
                actor,
                session,
                "state",
                Some(before.state.as_str().into()),
                Some(t.state.as_str().into()),
            )
            .await;
        }
        if t.reviewer != before.reviewer {
            self.event(
                &t,
                actor,
                session,
                "reviewer",
                before.reviewer.as_ref().map(ToString::to_string),
                t.reviewer.as_ref().map(ToString::to_string),
            )
            .await;
        }
        self.say(&t, actor, &format!("reassigned to {to}: {why}"))
            .await;
        if t.state != TaskState::Blocked {
            self.wake(&t, &to, WakeReason::Reassigned, actor).await;
        }
        self.with_blocks(t).await
    }

    /// `task_checkout`: `todo`/`blocked`/unheld `doing` → `doing` under
    /// `session` on this engine, `epoch + 1`. A held checkout is `conflict`.
    pub async fn checkout(&self, id: &TaskId, session: &str, actor: &str) -> R<Task> {
        let mut t = self.load(id).await?;
        if !acts_for(actor, &t.owner) {
            return Err(format!("not the owner ({})", t.owner));
        }
        match (t.state, &t.checkout) {
            (TaskState::Doing, Some(c)) if !c.orphaned => {
                return Err(format!(
                    "conflict: checked out by {} on {} (epoch {})",
                    c.session, c.engine, c.epoch
                ));
            }
            (TaskState::Todo | TaskState::Blocked | TaskState::Doing, _) => {}
            (s, _) => return Err(format!("task is {}", s.as_str())),
        }
        if t.state == TaskState::Blocked && !t.blockers.is_empty() {
            return Err(format!("still blocked on {}", t.blockers.join(", ")));
        }
        let now = self.now();
        let prev = t.checkout.take();
        let c = Checkout {
            session: session.to_owned(),
            engine: self.engine.clone(),
            epoch: prev.as_ref().map_or(0, |c| c.epoch) + 1,
            progress_at_ms: now,
            orphaned: false,
            taken: false,
        };
        let before = t.state;
        t.checkout = Some(c.clone());
        t.state = TaskState::Doing;
        t.updated_ms = now;
        self.save(&t).await?;
        self.event(
            &t,
            actor,
            Some(session),
            "checkout",
            prev.as_ref().map(checkout_text),
            Some(checkout_text(&c)),
        )
        .await;
        if before != TaskState::Doing {
            self.event(
                &t,
                actor,
                Some(session),
                "state",
                Some(before.as_str().into()),
                Some("doing".into()),
            )
            .await;
        }
        self.with_blocks(t).await
    }

    /// `task_comment` / `task_note`: a block in the thread. A comment wakes
    /// the owner (and the reviewer under review) unless they wrote it, plus
    /// every agent named in it; a human comment clears the back-off. A
    /// progress note bumps `checkout.progress_at` and wakes nobody.
    pub async fn comment(
        &self,
        id: &TaskId,
        text: &str,
        progress: bool,
        actor: &str,
        session: Option<&str>,
    ) -> R<()> {
        let mut t = self.load(id).await?;
        // 99 D3: notes are free — a desk woken by a mention must be able to answer.
        self.check_epoch(session, &t).await?;
        let now = self.now();
        let mut dirty = false;
        if progress {
            self.say(&t, actor, &format!("_progress note_\n\n{text}"))
                .await;
            if let Some(c) = &mut t.checkout
                && session.is_some_and(|s| s == c.session)
            {
                c.progress_at_ms = now;
                dirty = true;
            }
        } else {
            self.say(&t, actor, text).await;
            if actor == "user" && (t.no_progress_count > 0 || t.next_wake_after_ms.is_some()) {
                t.no_progress_count = 0;
                t.next_wake_after_ms = None;
                dirty = true;
            }
        }
        if dirty {
            self.save(&t).await?;
        }
        let first = text
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();
        let field = if progress { "note" } else { "comment" };
        if let Err(e) = self
            .store
            .add_event(
                &t.id,
                &TaskEvent {
                    at_ms: now,
                    actor: actor.to_owned(),
                    session: session.map(str::to_owned),
                    field: field.to_owned(),
                    before: None,
                    after: Some(first),
                },
            )
            .await
        {
            tracing::error!(task = %t.id, "event not recorded: {e:#}");
        }
        if progress {
            return Ok(());
        }
        let mut woken: HashSet<String> = HashSet::new();
        if actor != t.owner.to_string()
            && let Some(a) = agent_name(&t.owner)
        {
            woken.insert(a.to_owned());
            self.wake(&t, &t.owner, WakeReason::Comment, actor).await;
        }
        if t.state == TaskState::Review
            && let Some(r) = &t.reviewer
            && actor != r.to_string()
            && let Some(a) = agent_name(r)
            && woken.insert(a.to_owned())
        {
            self.wake(&t, r, WakeReason::Comment, actor).await;
        }
        // `@`-less mentions: a word that is an agent's name.
        if let Some(router) = &self.router {
            let me = actor.strip_prefix("agent:").unwrap_or("");
            let words: HashSet<String> = text
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .map(str::to_ascii_lowercase)
                .collect();
            for a in router.agents() {
                if a != me && words.contains(&a) && woken.insert(a.clone()) {
                    self.wake(&t, &Owner::Agent(a), WakeReason::Mention, actor)
                        .await;
                }
            }
        }
        Ok(())
    }

    /// `task_place`: record theuth's answer (67 §8) and its reason.
    pub async fn place(
        &mut self,
        id: &TaskId,
        where_: Option<String>,
        harness: Option<String>,
        reason: &str,
        actor: &str,
        session: Option<&str>,
    ) -> R<Task> {
        let patch = TaskPatch {
            where_,
            harness,
            ..Default::default()
        };
        let t = self.update(id, patch, actor, session).await?;
        self.say(
            &t,
            actor,
            &format!(
                "placed on `{}` / `{}`: {reason}",
                t.where_.as_deref().unwrap_or("?"),
                t.harness.as_deref().unwrap_or("?")
            ),
        )
        .await;
        Ok(t)
    }

    /// A chat message into a task's thread (67 §4.6): a comment by `actor`
    /// and a `mention` wake for the owner.
    pub async fn mention(&self, id: &TaskId, text: &str, actor: &str) -> R<()> {
        let t = self.load(id).await?;
        self.say(&t, actor, text).await;
        self.wake(&t, &t.owner, WakeReason::Mention, actor).await;
        Ok(())
    }

    /// `escalate` (67 §11, 101, 103): the question in the thread, `blocked: decision:<id>`,
    /// the **caller's** manager woken (`answer`). When the manager is the user
    /// the request needs a `summary` and a `board` with the decision skeleton
    /// (D1, R1, R2) and nothing is written without them; with a summary, board,
    /// pointers, recommendation or items (any audience) the request is stored as
    /// the `decision:<id>` packet and the thread block is its markdown (D3).
    /// 103 F1: when the task is already blocked on a decision somebody else
    /// raised (the newest such packet is the parent) every field the caller
    /// left empty — `question` (`""`), `summary`, `board`, `options` (with
    /// `recommended`), `pointers`, `items` — is inherited from it, D1 is judged
    /// on the merged packet, and the thread block's second line says
    /// `forwarded from <parent> by <actor>`. Returns the decision and the
    /// manager; `Owner::User` means the actor handler asks the user. The user
    /// has nobody to escalate to.
    pub async fn escalate(
        &mut self,
        id: &TaskId,
        mut ask: packet::Ask,
        actor: &str,
        session: Option<&str>,
    ) -> R<(String, Owner)> {
        let manager = match actor.strip_prefix("agent:") {
            Some(a) => self.manager_of(a),
            None => return Err("you are the top of the line; decide it".into()),
        };
        let t = self.load(id).await?;
        let mut parent = None;
        for b in t
            .blockers
            .iter()
            .rev()
            .filter(|b| b.starts_with("decision:"))
        {
            if let Some(p) = self.store.packet(b).await.map_err(db)?
                && p.by != actor
            {
                parent = Some(p);
                break;
            }
        }
        if let Some(par) = &parent {
            if ask.question.trim().is_empty() {
                ask.question = par.question.clone().unwrap_or_default();
            }
            if ask.summary.as_deref().is_none_or(|s| s.trim().is_empty()) {
                ask.summary = Some(par.summary.clone()).filter(|s| !s.trim().is_empty());
            }
            if ask.board.is_none() {
                ask.board = par.board.clone();
            }
            if ask.options.is_empty() {
                ask.options = par.options.clone();
                if ask.recommended.is_none() {
                    ask.recommended = par.recommended;
                }
            }
            if ask.pointers.is_empty() {
                ask.pointers = par.pointers.clone();
            }
            if ask.items.is_empty() {
                ask.items = par.items.clone();
            }
        }
        let decision = format!("decision:{}", uuid::Uuid::now_v7());
        let packet_wanted = parent.is_some()
            || ask.summary.is_some()
            || ask.board.is_some()
            || !ask.pointers.is_empty()
            || ask.recommendation.is_some()
            || !ask.items.is_empty();
        let p = packet::Packet {
            key: decision.clone(),
            task: id.clone(),
            kind: packet::PacketKind::Decision,
            by: actor.to_owned(),
            at_ms: self.now(),
            question: Some(ask.question),
            summary: ask.summary.unwrap_or_default(),
            board: ask.board,
            options: ask.options,
            recommended: ask.recommended,
            pointers: ask.pointers,
            checks: vec![],
            recommendation: ask.recommendation,
            items: ask.items,
            to: Some(manager.to_string()),
        };
        if manager == Owner::User {
            packet::require_for_user(&p)?;
            self.check_board(p.kind, p.board.as_deref().unwrap_or_default())
                .await?;
        }
        packet::validate(&p)?;
        if manager == Owner::User || packet_wanted {
            self.store.put_packet(&p).await.map_err(db)?;
        }
        let mut text = packet::thread_markdown(&p);
        if let Some(par) = &parent {
            let eol = text.find('\n').unwrap_or(text.len());
            text.insert_str(eol, &format!("\nforwarded from {} by {actor}", par.key));
        }
        text.push_str(&format!(
            "\n\nManager: answer with `decide {{ task, decision: \"{decision}\", answer }}`, or `escalate` the same task further."
        ));
        self.say(&t, actor, &text).await;
        let mut blockers = t.blockers.clone();
        blockers.push(decision.clone());
        let state = if matches!(t.state, TaskState::Doing | TaskState::Todo) {
            Some(TaskState::Blocked)
        } else {
            None
        };
        self.update(
            id,
            TaskPatch {
                blockers: Some(blockers),
                state,
                ..Default::default()
            },
            actor,
            session,
        )
        .await?;
        self.wake(&t, &manager, WakeReason::Answer, actor).await;
        Ok((decision, manager))
    }

    /// Who reviews `t` if it goes to review now: `reviewer` when given, else
    /// the task's; a desk naming itself without a `done_when` gets its
    /// manager (as [`Self::update`] resolves it); nobody named → the owner's
    /// manager (the user owns a task → the user).
    pub fn effective_reviewer(&self, t: &Task, reviewer: Option<&Owner>) -> Owner {
        let named = reviewer.or(t.reviewer.as_ref());
        match (&t.owner, named) {
            (Owner::Agent(a), None) => self.manager_of(a),
            (Owner::Agent(a), Some(r)) if *r == t.owner && t.done_when.is_none() => {
                self.manager_of(a)
            }
            (_, Some(r)) => r.clone(),
            (owner, None) => owner.clone(),
        }
    }

    /// A hand-over to review (101 D3): the `review:<task>` packet stored
    /// (the latest replaces) and its `**review requested**` block in the thread.
    pub async fn review_packet(
        &self,
        id: &TaskId,
        summary: String,
        board: Option<String>,
        pointers: Vec<packet::Pointer>,
        checks: Vec<packet::Check>,
        actor: &str,
    ) -> R<packet::Packet> {
        let t = self.load(id).await?;
        let p = packet::Packet {
            key: packet::review_key(id),
            task: id.clone(),
            kind: packet::PacketKind::Review,
            by: actor.to_owned(),
            at_ms: self.now(),
            question: None,
            summary,
            board,
            options: vec![],
            recommended: None,
            pointers,
            checks,
            recommendation: None,
            items: vec![],
            to: Some(self.effective_reviewer(&t, None).to_string()),
        };
        packet::validate(&p)?;
        self.store.put_packet(&p).await.map_err(db)?;
        self.say(&t, actor, &packet::thread_markdown(&p)).await;
        Ok(p)
    }

    /// 101 R2: `board` exists, is a board, and its first `##` sections are the
    /// skeleton of `kind` ([`packet::check_board`]). Run for the user audience
    /// before any write.
    pub async fn check_board(&self, kind: packet::PacketKind, board: &str) -> R<()> {
        let Some(d) = self.thread.headings(board).await? else {
            return Err(format!("board `{board}` does not exist"));
        };
        if d.kind != document::DocKind::Board {
            return Err(format!(
                "`{board}` is a {}, not a board (create it with doc_create(kind=\"board\"))",
                d.kind
            ));
        }
        packet::check_board(kind, &d.headings).map_err(|e| format!("board `{board}`: {e}"))
    }

    /// One packet by key.
    pub async fn packet(&self, key: &str) -> R<Option<packet::Packet>> {
        self.store.packet(key).await.map_err(db)
    }

    /// Every packet of `tasks`, newest first.
    pub async fn packets_for(&self, tasks: &[TaskId]) -> R<Vec<packet::Packet>> {
        self.store.packets_for(tasks).await.map_err(db)
    }

    /// `decide` (67 §11): the answer in the thread, the `decision:<id>` blocker
    /// dropped — the last blocker going re-wakes the owner (`answer`). 106 F-3:
    /// when the packet was raised by an agent other than the owner (a
    /// forwarding manager, 103 F1) that agent is woken `answer` too — the
    /// parent decision still blocks, so the owner wake alone never comes.
    pub async fn answer_decision(
        &mut self,
        decision: &str,
        answer: &str,
        actor: &str,
        session: Option<&str>,
    ) -> R<()> {
        let t = self
            .store
            .blocked_on(decision)
            .await
            .map_err(db)?
            .into_iter()
            .next()
            .ok_or_else(|| format!("no task is blocked on {decision}"))?;
        self.say(&t, actor, &format!("**decision** (`{decision}`): {answer}"))
            .await;
        let blockers = t
            .blockers
            .iter()
            .filter(|b| *b != decision)
            .cloned()
            .collect();
        self.update(
            &t.id,
            TaskPatch {
                blockers: Some(blockers),
                ..Default::default()
            },
            actor,
            session,
        )
        .await?;
        if let Ok(Some(p)) = self.store.packet(decision).await
            && let Ok(by @ Owner::Agent(_)) = p.by.parse::<Owner>()
            && by != t.owner
            && p.by != actor
        {
            self.wake(&t, &by, WakeReason::Answer, actor).await;
        }
        Ok(())
    }

    // ---- wakes: dispatch, completion, back-off -------------------------------------

    /// One dispatch pass (67 §7.3): per agent with nothing claimed, the
    /// oldest unclaimed wake whose task is not in back-off is claimed by the
    /// agent's desk and returned for delivery. A wake whose reason has no
    /// purpose in its task's current state (STALE-WAKE-DESKS: a `review` wake
    /// on a task already `done`) is dropped here with a thread line, not
    /// dispatched — this is also the startup sweep over rows left by earlier
    /// builds.
    pub async fn dispatch(&mut self) -> R<Vec<Delivery>> {
        let now = self.now();
        let wakes = self.store.wakes().await.map_err(db)?;
        let busy: HashSet<&str> = wakes
            .iter()
            .filter(|w| w.claimed_by.is_some())
            .map(|w| w.agent.as_str())
            .collect();
        // WAKE-LIVE-CHILD D3: say once, per blocking wake, that an agent's
        // newer wakes wait behind a claim its desk still holds.
        // ponytail: O(claimed × wakes) scan; the table holds a handful of rows.
        self.reported_busy
            .retain(|agent, _| busy.contains(agent.as_str()));
        for w in wakes.iter().filter(|w| w.claimed_by.is_some()) {
            let waiting = wakes
                .iter()
                .filter(|x| x.agent == w.agent && x.claimed_by.is_none())
                .count();
            if waiting == 0 || self.reported_busy.get(&w.agent) == Some(&w.id) {
                continue;
            }
            tracing::info!(
                agent = %w.agent,
                behind = %w.id,
                reason = w.reason.as_str(),
                task = ?w.task,
                claimed_by = ?w.claimed_by,
                waiting,
                "wakes held: desk still serving an earlier wake"
            );
            self.reported_busy.insert(w.agent.clone(), w.id.clone());
        }
        let mut picked: HashMap<String, Delivery> = HashMap::new();
        for w in &wakes {
            if w.claimed_by.is_some()
                || busy.contains(w.agent.as_str())
                || picked.contains_key(&w.agent)
            {
                continue;
            }
            if self
                .retry
                .get(&w.id)
                .is_some_and(|(_, not_before)| *not_before > now)
            {
                continue;
            }
            let task = match &w.task {
                Some(id) => self.store.get(id).await.map_err(db)?,
                None => None,
            };
            if let Some(id) = &w.task {
                let stale = match &task {
                    Some(t) => (!w.reason.applies_to(t.state))
                        .then(|| format!("task is {}", t.state.as_str())),
                    None => Some("task is gone".to_owned()),
                };
                if let Some(why) = stale {
                    tracing::info!(agent = %w.agent, task = %id, reason = w.reason.as_str(), "wake dropped: stale ({why})");
                    if let Some(t) = &task {
                        self.say(
                            t,
                            "system",
                            &format!(
                                "wake `{}` for {} dropped: stale ({why})",
                                w.reason.as_str(),
                                w.agent
                            ),
                        )
                        .await;
                    }
                    self.store.finish_wake(&w.id).await.map_err(db)?;
                    continue;
                }
            }
            if task
                .as_ref()
                .and_then(|t| t.next_wake_after_ms)
                .is_some_and(|after| after > now)
            {
                continue;
            }
            picked.insert(
                w.agent.clone(),
                Delivery {
                    wake: w.clone(),
                    task,
                },
            );
        }
        let mut out = Vec::with_capacity(picked.len());
        for (_, mut d) in picked {
            let desk = self
                .router
                .as_ref()
                .and_then(|r| r.desk_for(&d.wake.agent))
                .unwrap_or_else(|| DISPATCHING.to_owned());
            self.store
                .claim(&d.wake.id, Some(&desk), now)
                .await
                .map_err(db)?;
            d.wake.claimed_by = Some(desk);
            if let Some(t) = &d.task {
                self.delivered
                    .insert((d.wake.agent.clone(), t.id.clone()), t.updated_ms);
            }
            out.push(d);
        }
        Ok(out)
    }

    /// Delivery returned: fix the placeholder claim to the desk now open.
    pub async fn delivered(&mut self, wake: &Wake) -> R<()> {
        self.retry.remove(&wake.id);
        if wake.claimed_by.as_deref() == Some(DISPATCHING)
            && let Some(desk) = self.router.as_ref().and_then(|r| r.desk_for(&wake.agent))
        {
            self.store
                .claim(&wake.id, Some(&desk), self.now())
                .await
                .map_err(db)?;
        }
        Ok(())
    }

    /// Delivery failed: unclaim; retry after 5 s doubling to 5 min.
    pub async fn delivery_failed(&mut self, wake: &Wake) -> R<()> {
        self.release_wake(wake, 0).await
    }

    /// The desk turn that took `wake` did not yield (WAKE-LIVE-CHILD D2: a
    /// cancel, timeout or provider error): unclaim it so dispatch delivers it
    /// again, not before the delivery back-off (5 s doubling to 5 min) or
    /// `min_backoff_ms`, whichever is later.
    pub async fn release_wake(&mut self, wake: &Wake, min_backoff_ms: u64) -> R<()> {
        let now = self.now();
        let e = self.retry.entry(wake.id.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 = now
            + (5_000u64 << (e.0 - 1).min(6))
                .min(300_000)
                .max(min_backoff_ms);
        self.store.claim(&wake.id, None, now).await.map_err(db)
    }

    /// The wake on `t` completed (67 §7.3): no field change since delivery —
    /// including none in the completing write itself — counts against it; two
    /// in a row start the back-off.
    async fn complete_wake(&self, t: &mut Task, agent: &str, no_progress: bool) {
        if no_progress {
            t.no_progress_count += 1;
        } else {
            t.no_progress_count = 0;
            t.next_wake_after_ms = None;
        }
        if t.no_progress_count >= 2 {
            let n = t.no_progress_count - 2;
            let secs = (self.cfg.backoff_min_secs << n.min(20)).min(self.cfg.backoff_max_secs);
            t.next_wake_after_ms = Some(self.now() + secs * 1000);
        }
        if let Err(e) = self.store.finish_wakes(agent, &t.id).await {
            tracing::error!(task = %t.id, "wakes not finished: {e:#}");
        }
    }

    /// Wakes that are not claimed by a desk, for tests and the board.
    pub async fn wakes(&self) -> R<Vec<Wake>> {
        self.store.wakes().await.map_err(db)
    }

    /// The desk turn that took `wake` is over (the roster judged it handled or
    /// blocked it): the row goes, whether or not the agent wrote the task.
    pub async fn finish_wake(&mut self, wake: &Wake) -> R<()> {
        if let Some(t) = &wake.task {
            self.delivered.remove(&(wake.agent.clone(), t.clone()));
        }
        self.store.finish_wake(&wake.id).await.map_err(db)
    }

    /// A desk (re)opened on `session`: any wake still claimed by it belongs
    /// to a turn that no longer runs — released for dispatch.
    pub async fn release_claims(&self, session: &str) -> R<()> {
        self.store
            .release_claimed_by(session, self.now())
            .await
            .map_err(db)
    }

    // ---- orphan sweep, disposition enforcement -----------------------------------------

    /// 67 §7.5 every 30 s. `reachable`: the connected engines (self included)
    /// when the mesh is up; `None` = staleness only. A reachable checkout is
    /// stale when neither its session nor any descendant works right now
    /// ([`LivenessProbe`], WAKE-LIVE-CHILD D4) and both its last progress
    /// note and its last activity are older than `tasks.stale_secs` — a desk
    /// idle between its children's results is not lost. Orphans: the checkout
    /// is kept, marked, `epoch + 1`; the owner is woken with `resume`. Wakes
    /// claimed by a desk that is no longer the agent's are un-claimed.
    pub async fn sweep(&mut self, reachable: Option<&HashSet<String>>) -> R<Vec<TaskId>> {
        let probed = self.prober().probe(self.now(), reachable).await?;
        self.sweep_apply(reachable, &probed).await
    }

    /// The sweep's asking half, cheap to clone off the actor: the pieces
    /// [`SweepProber::probe`] needs and nothing the apply half mutates.
    pub fn prober(&self) -> SweepProber {
        SweepProber {
            store: self.store.clone(),
            probe: self.probe.clone(),
            stale_ms: self.cfg.stale_secs * 1000,
        }
    }

    /// The sweep's applying half: [`sweep`](Self::sweep) with the probe
    /// answers already in hand. A checkout that needs an answer but has none
    /// in `probed` — or whose session or epoch changed since it was probed —
    /// is left alone until the next sweep.
    pub async fn sweep_apply(
        &mut self,
        reachable: Option<&HashSet<String>>,
        probed: &HashMap<TaskId, Probed>,
    ) -> R<Vec<TaskId>> {
        let now = self.now();
        if let Some(r) = reachable {
            self.unreachable_since.retain(|e, _| !r.contains(e));
        }
        let doing = self
            .store
            .list(&TaskFilter {
                state: Some(vec![TaskState::Doing]),
                ..Default::default()
            })
            .await
            .map_err(db)?;
        let mut orphaned = vec![];
        for mut t in doing {
            let Some(c) = t.checkout.clone() else {
                continue;
            };
            if c.orphaned {
                continue;
            }
            let mut last_active_ms = None;
            let lost = match reachable {
                Some(r) if !r.contains(&c.engine) => {
                    let since = *self
                        .unreachable_since
                        .entry(c.engine.clone())
                        .or_insert(now);
                    now.saturating_sub(since) >= self.cfg.orphan_secs * 1000
                }
                _ => {
                    let stale_ms = self.cfg.stale_secs * 1000;
                    if now.saturating_sub(c.progress_at_ms) < stale_ms {
                        continue;
                    }
                    let Some(p) = probed
                        .get(&t.id)
                        .filter(|p| p.session == c.session && p.epoch == c.epoch)
                    else {
                        continue;
                    };
                    last_active_ms = p.live.and_then(|l| l.last_active_ms);
                    !p.live.is_some_and(|l| l.active_now)
                        && now.saturating_sub(c.progress_at_ms.max(last_active_ms.unwrap_or(0)))
                            >= stale_ms
                }
            };
            if !lost {
                continue;
            }
            let mut n = c.clone();
            n.orphaned = true;
            n.epoch += 1;
            // 68 §4.4: a session on a lost engine follows the work here; the
            // checkout then names this engine and the `Resume` wake lands on
            // it. A hung local session (staleness) is not taken.
            let taken = if c.engine != self.engine && !c.session.is_empty() {
                self.taker.take(&c.session).await.map_err(|e| {
                    tracing::info!(task = %t.id, "session {} not taken from {}: {e}", c.session, c.engine);
                })
            } else {
                Err(())
            };
            if taken.is_ok() {
                n.engine = self.engine.clone();
                n.taken = true;
            }
            t.checkout = Some(n.clone());
            t.updated_ms = now;
            self.save(&t).await?;
            self.event(
                &t,
                "system",
                None,
                "checkout",
                Some(checkout_text(&c)),
                Some(checkout_text(&n)),
            )
            .await;
            let since = match last_active_ms {
                Some(ms) => format!(
                    "; last activity {} min ago",
                    now.saturating_sub(ms) / 60_000
                ),
                None => String::new(),
            };
            let note = if taken.is_ok() {
                format!(
                    "session {} on {} is presumed lost{since}; taken here ({}), resume it",
                    c.session, c.engine, self.engine
                )
            } else {
                format!(
                    "session {} on {} is presumed lost{since}; resume from the last progress note",
                    c.session, c.engine
                )
            };
            self.say(&t, "system", &note).await;
            self.wake(&t, &t.owner, WakeReason::Resume, "system").await;
            orphaned.push(t.id);
        }
        // A claim is only good while the claiming session is the agent's desk
        // here: a moved desk, or none at all (a fresh hub before the agent's
        // desk opened — theuth's opens on its first delivered wake, which a
        // stale claim would otherwise hold back for ever), releases it.
        if let Some(router) = &self.router {
            for w in self.store.wakes().await.map_err(db)? {
                let Some(by) = &w.claimed_by else { continue };
                if by == DISPATCHING {
                    continue;
                }
                if router.desk_for(&w.agent).is_none_or(|d| d != *by) {
                    self.store.claim(&w.id, None, now).await.map_err(db)?;
                }
            }
        }
        Ok(orphaned)
    }

    /// `EpisodeClosing` (67 §7.3 last rule): every `doing` task checked out
    /// by one of `sessions` with no `continuing` disposition since its
    /// checkout is `blocked: no continuation` and the manager is woken.
    /// Wakes still claimed by the closing sessions are released, not deleted
    /// (67 §4.6: a wake the closed desk did not finish is delivered again —
    /// and validated then — by whoever holds the lease next).
    pub async fn episode_closing(&mut self, agent: &str, sessions: &[String]) -> R<Vec<TaskId>> {
        let now = self.now();
        for s in sessions {
            self.store.release_claimed_by(s, now).await.map_err(db)?;
        }
        self.delivered.retain(|(a, _), _| a != agent);
        let doing = self
            .store
            .list(&TaskFilter {
                state: Some(vec![TaskState::Doing]),
                ..Default::default()
            })
            .await
            .map_err(db)?;
        let mut blocked = vec![];
        for mut t in doing {
            let Some(c) = &t.checkout else { continue };
            if !sessions.contains(&c.session) {
                continue;
            }
            let continuing = self
                .store
                .last_event(&t.id, "disposition")
                .await
                .map_err(db)?
                .is_some_and(|e| {
                    e.after
                        .as_deref()
                        .is_some_and(|a| a.starts_with("continuing"))
                });
            if continuing {
                continue;
            }
            self.no_continuation(
                &mut t,
                agent,
                &format!("episode of {agent} closed without a disposition"),
            )
            .await?;
            blocked.push(t.id);
        }
        Ok(blocked)
    }

    /// The 67 §7.3 rule on one task (70 DESK-PROTOCOL nudges: the second
    /// unhandled turn): `blocked: no continuation`, `why` in the thread, the
    /// agent's manager woken `review`. Any open state; `done`/`cancelled` are left.
    pub async fn no_continuation(&mut self, t: &mut Task, agent: &str, why: &str) -> R<()> {
        if matches!(t.state, TaskState::Done | TaskState::Cancelled) {
            return Ok(());
        }
        let before = t.state;
        if !t.blockers.iter().any(|b| b == NO_CONTINUATION) {
            t.blockers.push(NO_CONTINUATION.to_owned());
        }
        t.state = TaskState::Blocked;
        t.checkout = None;
        t.updated_ms = self.now();
        self.save(t).await?;
        if before != TaskState::Blocked {
            self.event(
                t,
                "system",
                None,
                "state",
                Some(before.as_str().into()),
                Some("blocked".into()),
            )
            .await;
        }
        self.say(t, "system", why).await;
        self.wake(t, &self.manager_of(agent), WakeReason::Review, "system")
            .await;
        self.delivered.remove(&(agent.to_owned(), t.id.clone()));
        Ok(())
    }

    /// [`Self::no_continuation`] by id (the actor's `NoContinuation`).
    pub async fn no_continuation_by_id(&mut self, id: &TaskId, agent: &str, why: &str) -> R<Task> {
        let mut t = self.load(id).await?;
        self.no_continuation(&mut t, agent, why).await?;
        self.with_blocks(t).await
    }

    /// Record a disposition that changes no field (`continuing`, `delegated`).
    pub async fn note_disposition(
        &self,
        id: &TaskId,
        text: &str,
        actor: &str,
        session: Option<&str>,
    ) -> R<()> {
        let t = self.load(id).await?;
        self.event(
            &t,
            actor,
            session,
            "disposition",
            None,
            Some(text.to_owned()),
        )
        .await;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Thread blocks in memory: `doc id → blocks`.
    #[derive(Default)]
    pub struct MemThread(pub Mutex<HashMap<String, Vec<String>>>);

    #[async_trait::async_trait]
    impl Thread for MemThread {
        async fn create(&self, _task: &str, title: &str, body: &str) -> Option<String> {
            let id = format!("doc-{}", self.0.lock().unwrap().len() + 1);
            self.0
                .lock()
                .unwrap()
                .insert(id.clone(), vec![format!("# {title}"), body.to_owned()]);
            Some(id)
        }
        async fn append(&self, doc: &str, markdown: &str) {
            self.0
                .lock()
                .unwrap()
                .entry(doc.to_owned())
                .or_default()
                .push(markdown.to_owned());
        }
        async fn read(&self, doc: &str) -> Option<String> {
            self.0.lock().unwrap().get(doc).map(|b| b.join("\n\n"))
        }
        /// A doc whose first block is a `root:` frontmatter is a board; headings are its `#` lines.
        async fn headings(&self, doc: &str) -> Result<Option<document::DocHeadings>, String> {
            let map = self.0.lock().unwrap();
            let Some(blocks) = map.get(doc) else {
                return Ok(None);
            };
            let board = blocks.first().is_some_and(|b| b.starts_with("---\nroot:"));
            let headings = blocks
                .iter()
                .flat_map(|b| b.lines())
                .filter_map(|l| {
                    let text = l.trim_start_matches('#');
                    let level = l.len() - text.len();
                    (level > 0 && text.starts_with(' '))
                        .then(|| (level as u8, text.trim().to_owned()))
                })
                .collect();
            Ok(Some(document::DocHeadings {
                kind: if board {
                    document::DocKind::Board
                } else {
                    document::DocKind::Note
                },
                headings,
            }))
        }
    }

    /// A board document under `id` whose `##` sections are `sections`, in order.
    pub fn board(thread: &MemThread, id: &str, sections: &[&str]) {
        let mut blocks = vec!["---\nroot: /repo\n---".to_owned(), "# Board".to_owned()];
        blocks.extend(sections.iter().map(|s| format!("## {s}\n\nOne block.")));
        thread.0.lock().unwrap().insert(id.to_owned(), blocks);
    }

    /// A roster: `agent → manager`, `agent → desk`, `session → agent`, delivery log.
    #[derive(Default)]
    pub struct MemRouter {
        pub managers: HashMap<String, String>,
        pub desks: Mutex<HashMap<String, String>>,
        pub sessions: HashMap<String, String>,
        pub delivered: Mutex<Vec<(String, WakeReason)>>,
        pub fail: Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl DeskRouter for MemRouter {
        fn desk_for(&self, agent: &str) -> Option<String> {
            self.desks.lock().unwrap().get(agent).cloned()
        }
        async fn deliver(&self, agent: &str, wake: &Wake, _: Option<&Task>) -> anyhow::Result<()> {
            if *self.fail.lock().unwrap() {
                anyhow::bail!("desk down");
            }
            self.delivered
                .lock()
                .unwrap()
                .push((agent.to_owned(), wake.reason));
            Ok(())
        }
        fn manager_of(&self, agent: &str) -> Option<String> {
            self.managers.get(agent).cloned()
        }
        fn agent_for(&self, session: &str) -> Option<String> {
            self.sessions.get(session).cloned()
        }
        fn agents(&self) -> Vec<String> {
            let mut v: Vec<String> = self.managers.keys().cloned().collect();
            v.extend(self.managers.values().cloned());
            v.sort();
            v.dedup();
            v
        }
    }

    /// A sessions actor: records takes; `fail` = the owner refused.
    #[derive(Default)]
    pub struct MemTaker {
        pub taken: Mutex<Vec<String>>,
        pub fail: Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl SessionTaker for MemTaker {
        async fn take(&self, session: &str) -> Result<(), String> {
            if *self.fail.lock().unwrap() {
                return Err("owner busy".into());
            }
            self.taken.lock().unwrap().push(session.to_owned());
            Ok(())
        }
    }

    /// A liveness probe: `session → answer`; unknown sessions answer `None`.
    #[derive(Default)]
    pub struct MemProbe(pub tokio::sync::Mutex<HashMap<String, Liveness>>);

    #[async_trait::async_trait]
    impl LivenessProbe for MemProbe {
        async fn liveness(&self, session: &str) -> Option<Liveness> {
            self.0.lock().await.get(session).copied()
        }
    }

    pub struct Rig {
        pub ledger: Ledger,
        pub clock: Arc<AtomicU64>,
        pub thread: Arc<MemThread>,
        pub router: Arc<MemRouter>,
        pub taker: Arc<MemTaker>,
        pub probe: Arc<MemProbe>,
        _dir: tempfile::TempDir,
    }

    pub async fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite")).await.unwrap();
        let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
        let thread = Arc::new(MemThread::default());
        let c = clock.clone();
        let mut ledger = Ledger::new(
            store,
            TasksConfig::default(),
            thread.clone(),
            Arc::new(move || c.load(Ordering::SeqCst)),
            "mac".into(),
        );
        let mut router = MemRouter::default();
        router.managers.insert("ops".into(), "lead".into());
        router.sessions.insert("s-ops".into(), "ops".into());
        router.sessions.insert("s-lead".into(), "lead".into());
        router.sessions.insert("s-desk-ops".into(), "ops".into());
        let router = Arc::new(router);
        ledger.router = Some(router.clone());
        let taker = Arc::new(MemTaker::default());
        ledger.taker = taker.clone();
        let probe = Arc::new(MemProbe::default());
        ledger.probe = probe.clone();
        Rig {
            ledger,
            clock,
            thread,
            router,
            taker,
            probe,
            _dir: dir,
        }
    }

    impl Rig {
        pub fn tick(&self, ms: u64) {
            self.clock.fetch_add(ms, Ordering::SeqCst);
        }
        pub async fn task(&mut self, owner: &str) -> Task {
            self.ledger
                .create(
                    TaskCreate {
                        state: TaskState::Todo,
                        title: "fix deploy".into(),
                        brief: "the flaky one".into(),
                        owner: owner.parse().unwrap(),
                        reviewer: None,
                        parent: None,
                        project: None,
                        where_: None,
                        harness: None,
                        external: vec![],
                        done_when: None,
                        attachments: vec![],
                    },
                    "user",
                    None,
                    WakeReason::Assigned,
                )
                .await
                .unwrap()
        }
        pub async fn set_state(&mut self, id: &TaskId, s: TaskState, actor: &str) -> R<Task> {
            self.ledger
                .update(
                    id,
                    TaskPatch {
                        state: Some(s),
                        ..Default::default()
                    },
                    actor,
                    None,
                )
                .await
        }
        pub async fn blockers(&mut self, id: &TaskId, b: &[&str], actor: &str) -> R<Task> {
            let b = b.iter().map(|s| (*s).to_owned()).collect();
            self.ledger
                .update(
                    id,
                    TaskPatch {
                        blockers: Some(b),
                        ..Default::default()
                    },
                    actor,
                    None,
                )
                .await
        }
        pub async fn blocked(&mut self, id: &TaskId, on: &str, actor: &str) -> R<Task> {
            self.ledger
                .update(
                    id,
                    TaskPatch {
                        state: Some(TaskState::Blocked),
                        blockers: Some(vec![on.into()]),
                        ..Default::default()
                    },
                    actor,
                    None,
                )
                .await
        }
        pub async fn done(&mut self, id: &TaskId, because: DoneBecause, actor: &str) -> R<Task> {
            self.ledger
                .update(
                    id,
                    TaskPatch {
                        state: Some(TaskState::Done),
                        done_because: Some(because),
                        ..Default::default()
                    },
                    actor,
                    None,
                )
                .await
        }
    }

    #[tokio::test]
    async fn create_defaults_reviewer_thread_and_events() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        assert_eq!(t.reviewer, Some(Owner::Agent("lead".into())));
        assert_eq!(t.state, TaskState::Todo);
        let doc = t.thread.clone().expect("thread document");
        let blocks = r.thread.0.lock().unwrap().get(&doc).cloned().unwrap();
        assert!(blocks[0].starts_with("# Task ") && blocks[0].ends_with(": fix deploy"));
        assert!(blocks.iter().any(|b| b.contains("created")));
        let ev = r.ledger.events(&t.id).await.unwrap();
        assert_eq!(ev[0].field, "created");
        let wakes = r.ledger.wakes().await.unwrap();
        assert_eq!(
            (wakes[0].agent.as_str(), wakes[0].reason),
            ("ops", WakeReason::Assigned)
        );

        let u = r.task("user").await;
        assert_eq!(u.reviewer, Some(Owner::User));
        let mut nobody = rig().await;
        nobody.ledger.router = None;
        let t = nobody.task("agent:solo").await;
        assert_eq!(
            t.reviewer,
            Some(Owner::User),
            "no manager → the user reviews"
        );
    }

    #[tokio::test]
    async fn transition_table() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let id = t.id.clone();
        // todo → doing only by checkout
        assert!(
            r.set_state(&id, TaskState::Doing, "agent:ops")
                .await
                .unwrap_err()
                .contains("task_checkout")
        );
        assert!(
            r.set_state(&id, TaskState::Review, "agent:ops")
                .await
                .is_err()
        );
        assert!(r.set_state(&id, TaskState::Done, "user").await.is_err());
        // blocked needs a blocker
        assert!(
            r.set_state(&id, TaskState::Blocked, "agent:ops")
                .await
                .unwrap_err()
                .contains("blockers")
        );
        assert_eq!(
            r.blocked(&id, "person:sky", "agent:ops")
                .await
                .unwrap()
                .state,
            TaskState::Blocked
        );
        // blocked → doing only when clear; then it lands in todo (no checkout)
        assert!(
            r.set_state(&id, TaskState::Doing, "agent:ops")
                .await
                .unwrap_err()
                .contains("still blocked")
        );
        let t = r.blockers(&id, &[], "agent:ops").await.unwrap();
        assert_eq!(t.state, TaskState::Todo);
        // checkout → doing; owner only
        assert!(
            r.ledger
                .checkout(&id, "s-lead", "agent:lead")
                .await
                .unwrap_err()
                .contains("not the owner")
        );
        let t = r.ledger.checkout(&id, "s-ops", "agent:ops").await.unwrap();
        assert_eq!(t.state, TaskState::Doing);
        assert_eq!(t.checkout.as_ref().unwrap().epoch, 1);
        // doing → review is the owner's
        assert!(
            r.set_state(&id, TaskState::Review, "agent:lead")
                .await
                .is_err()
        );
        assert!(
            r.set_state(&id, TaskState::Done, "agent:lead")
                .await
                .is_err()
        );
        let t = r
            .set_state(&id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        assert_eq!(t.state, TaskState::Review);
        assert!(t.checkout.is_none());
        // review → doing is the reviewer's (send back), then review again, then done by reviewer
        assert!(
            r.set_state(&id, TaskState::Doing, "agent:ops")
                .await
                .is_err()
        );
        assert_eq!(
            r.set_state(&id, TaskState::Doing, "agent:lead")
                .await
                .unwrap()
                .state,
            TaskState::Doing
        );
        r.set_state(&id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        assert!(
            r.set_state(&id, TaskState::Done, "agent:ops")
                .await
                .unwrap_err()
                .contains("reviewer")
        );
        // an agent's `done` says why; the verdict lands in the thread
        assert!(
            r.set_state(&id, TaskState::Done, "agent:lead")
                .await
                .unwrap_err()
                .contains("done_because")
        );
        let t = r
            .done(&id, DoneBecause::IntentMet, "agent:lead")
            .await
            .unwrap();
        assert_eq!(t.state, TaskState::Done);
        // terminal
        assert!(
            r.set_state(&id, TaskState::Cancelled, "user")
                .await
                .unwrap_err()
                .contains("is done")
        );
        // * → cancelled
        let t2 = r.task("agent:ops").await;
        assert_eq!(
            r.set_state(&t2.id, TaskState::Cancelled, "agent:ops")
                .await
                .unwrap()
                .state,
            TaskState::Cancelled
        );
        assert!(r.set_state(&t2.id, TaskState::Doing, "user").await.is_err());
        // every state change is an event
        let states: Vec<_> = r
            .ledger
            .events(&id)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.field == "state")
            .map(|e| e.after.unwrap())
            .collect();
        assert_eq!(
            states,
            [
                "blocked", "todo", "doing", "review", "doing", "review", "done"
            ]
        );
    }

    #[tokio::test]
    async fn backlog_is_not_dispatched_and_moves_by_intent() {
        let mut r = rig().await;
        let mut input = TaskCreate {
            state: TaskState::Backlog,
            title: "later".into(),
            brief: String::new(),
            owner: Owner::Agent("ops".into()),
            reviewer: None,
            parent: None,
            project: None,
            where_: None,
            harness: None,
            external: vec![],
            done_when: None,
            attachments: vec![],
        };
        let t = r
            .ledger
            .create(input.clone(), "user", None, WakeReason::Assigned)
            .await
            .unwrap();
        assert_eq!(t.state, TaskState::Backlog);
        assert!(
            r.ledger.wakes().await.unwrap().is_empty(),
            "backlog wakes nobody"
        );
        // only backlog | todo at creation
        input.state = TaskState::Doing;
        assert!(
            r.ledger
                .create(input, "user", None, WakeReason::Assigned)
                .await
                .unwrap_err()
                .contains("backlog or todo")
        );
        // backlog → doing / review / blocked refused; checkout refused
        assert!(
            r.set_state(&t.id, TaskState::Doing, "agent:ops")
                .await
                .unwrap_err()
                .contains("started first")
        );
        assert!(
            r.set_state(&t.id, TaskState::Review, "agent:ops")
                .await
                .is_err()
        );
        assert!(
            r.ledger
                .checkout(&t.id, "s-ops", "agent:ops")
                .await
                .unwrap_err()
                .contains("is backlog")
        );
        // task_move refusal names the allowed list
        let e = r
            .ledger
            .move_to(&t.id, TaskState::Done, None, None, "user", None)
            .await
            .unwrap_err();
        assert!(
            e.contains("backlog → done is not allowed")
                && e.contains("todo")
                && e.contains("cancelled"),
            "{e}"
        );
        assert!(!e.contains("doing"), "{e}");
        // backlog → todo wakes the owner `assigned` (the note is a comment: its own wake); the note lands in the thread
        let t = r
            .ledger
            .move_to(&t.id, TaskState::Todo, Some("go ahead"), None, "user", None)
            .await
            .unwrap();
        assert_eq!(t.state, TaskState::Todo);
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Assigned),
            "{w:?}"
        );
        let doc = t.thread.clone().unwrap();
        assert!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .any(|b| b.ends_with("go ahead"))
        );
        // todo → backlog pulls it back; review → todo is the reviewer's send-back
        assert_eq!(
            r.ledger
                .move_to(&t.id, TaskState::Backlog, None, None, "user", None)
                .await
                .unwrap()
                .state,
            TaskState::Backlog
        );
        r.ledger
            .move_to(&t.id, TaskState::Todo, None, None, "user", None)
            .await
            .unwrap();
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.set_state(&t.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        assert!(
            r.ledger
                .move_to(&t.id, TaskState::Todo, None, None, "agent:ops", None)
                .await
                .unwrap_err()
                .contains("reviewer")
        );
        assert_eq!(
            r.ledger
                .move_to(&t.id, TaskState::Todo, None, None, "agent:lead", None)
                .await
                .unwrap()
                .state,
            TaskState::Todo
        );
        assert!(
            Ledger::allowed_moves(&r.ledger.get(&t.id).await.unwrap(), "user")
                .contains(&TaskState::Backlog)
        );
    }

    /// 70 DESK-PROTOCOL (d): "assigning yourself plus a please-review comment is
    /// not a review path". Without `done_when` a self-reviewed task goes to the
    /// manager; with it the owner may close it, but only as `criteria met`.
    #[tokio::test]
    async fn self_review_only_against_explicit_criteria() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    reviewer: Some(Owner::Agent("ops".into())),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let t = r
            .set_state(&t.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        assert_eq!(
            t.reviewer,
            Some(Owner::Agent("lead".into())),
            "no criteria: the manager reviews"
        );
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter()
                .any(|w| w.agent == "lead" && w.reason == WakeReason::Review),
            "{w:?}"
        );
        assert!(
            r.done(&t.id, DoneBecause::IntentMet, "agent:ops")
                .await
                .unwrap_err()
                .contains("reviewer")
        );

        let t2 = r.task("agent:ops").await;
        r.ledger
            .update(
                &t2.id,
                TaskPatch {
                    reviewer: Some(Owner::Agent("ops".into())),
                    done_when: Some("the cert is renewed and curl shows the new expiry".into()),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        r.ledger
            .checkout(&t2.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let t2 = r
            .set_state(&t2.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        assert_eq!(
            t2.reviewer,
            Some(Owner::Agent("ops".into())),
            "explicit criteria: self-review stays"
        );
        let e = r
            .done(&t2.id, DoneBecause::IntentMet, "agent:ops")
            .await
            .unwrap_err();
        assert!(e.contains("criteria"), "{e}");
        assert_eq!(
            r.done(&t2.id, DoneBecause::CriteriaMet, "agent:ops")
                .await
                .unwrap()
                .state,
            TaskState::Done
        );
        let doc = r.ledger.get(&t2.id).await.unwrap().thread.unwrap();
        let thread = r.thread.0.lock().unwrap()[&doc].join("\n");
        assert!(thread.contains("done because: criteria met"), "{thread}");

        let u = r.task("user").await;
        r.ledger.checkout(&u.id, "s-user", "user").await.unwrap();
        r.set_state(&u.id, TaskState::Review, "user").await.unwrap();
        assert_eq!(
            r.set_state(&u.id, TaskState::Done, "user")
                .await
                .unwrap()
                .state,
            TaskState::Done
        );
    }

    /// Sky's ruling: done because the criteria are met, the intent is met, or
    /// follow-ups carry the work on — each recorded in the thread; follow-ups
    /// must exist; `criteria met` needs a `done_when`.
    #[tokio::test]
    async fn done_because_three_reasons() {
        let mut r = rig().await;
        async fn review(r: &mut Rig, done_when: Option<&str>) -> Task {
            let t = r.task("agent:ops").await;
            if let Some(dw) = done_when {
                r.ledger
                    .update(
                        &t.id,
                        TaskPatch {
                            done_when: Some(dw.to_owned()),
                            ..Default::default()
                        },
                        "user",
                        None,
                    )
                    .await
                    .unwrap();
            }
            r.ledger
                .checkout(&t.id, "s-ops", "agent:ops")
                .await
                .unwrap();
            r.set_state(&t.id, TaskState::Review, "agent:ops")
                .await
                .unwrap()
        }
        let thread_of =
            |r: &Rig, t: &Task| r.thread.0.lock().unwrap()[t.thread.as_ref().unwrap()].join("\n");

        // (1) criteria met — only with a done_when
        let a = review(&mut r, None).await;
        assert!(
            r.done(&a.id, DoneBecause::CriteriaMet, "agent:lead")
                .await
                .unwrap_err()
                .contains("done_when")
        );
        let a = review(&mut r, Some("all three checks pass")).await;
        let a = r
            .done(&a.id, DoneBecause::CriteriaMet, "agent:lead")
            .await
            .unwrap();
        assert!(
            thread_of(&r, &a).contains("**agent:lead**")
                && thread_of(&r, &a).contains("done because: criteria met")
        );

        // (2) intent met — the user's approval defaults to it
        let b = review(&mut r, None).await;
        let b = r
            .ledger
            .move_to(
                &b.id,
                TaskState::Done,
                Some("good enough"),
                None,
                "user",
                None,
            )
            .await
            .unwrap();
        assert_eq!(b.state, TaskState::Done);
        assert!(
            thread_of(&r, &b).contains("done because: intent met"),
            "{}",
            thread_of(&r, &b)
        );

        // (3) follow-ups filed — they must exist and be named
        let c = review(&mut r, None).await;
        let e = r
            .done(
                &c.id,
                DoneBecause::FollowUpsFiled { tasks: vec![] },
                "agent:lead",
            )
            .await
            .unwrap_err();
        assert!(e.contains("names"), "{e}");
        let e = r
            .done(
                &c.id,
                DoneBecause::FollowUpsFiled {
                    tasks: vec![TaskId("nope".into())],
                },
                "agent:lead",
            )
            .await
            .unwrap_err();
        assert!(e.contains("no such task"), "{e}");
        let f = r.task("agent:ops").await;
        let c = r
            .done(
                &c.id,
                DoneBecause::FollowUpsFiled {
                    tasks: vec![f.id.clone()],
                },
                "agent:lead",
            )
            .await
            .unwrap();
        assert_eq!(c.state, TaskState::Done);
        assert!(thread_of(&r, &c).contains(&format!("done because: follow-ups filed ({})", f.id)));
    }

    /// 70 TASK-ATTACHMENTS: refs survive the sqlite round trip (migration 0005),
    /// are deduplicated by kind + id, and each change is an `attached`/`detached`
    /// event with a thread line naming the kind and title.
    #[tokio::test]
    async fn attachments_round_trip_and_events() {
        use agent::attach::{AttachRef, Kind};
        let note = AttachRef {
            kind: Kind::Note,
            id: "n1".into(),
            title: "Groceries".into(),
            meta: "updated Sep 24".into(),
        };
        let art = AttachRef {
            kind: Kind::Artifact,
            id: "toolu_1".into(),
            title: "Q3 report".into(),
            meta: String::new(),
        };
        let mut r = rig().await;
        let t = r
            .ledger
            .create(
                TaskCreate {
                    state: TaskState::Backlog,
                    title: "fold the report".into(),
                    brief: String::new(),
                    owner: Owner::User,
                    reviewer: None,
                    parent: None,
                    project: None,
                    where_: None,
                    harness: None,
                    external: vec![],
                    done_when: None,
                    attachments: vec![
                        note.clone(),
                        note.clone(),
                        AttachRef {
                            kind: Kind::Session,
                            id: " ".into(),
                            title: "x".into(),
                            meta: String::new(),
                        },
                    ],
                },
                "user",
                None,
                WakeReason::Assigned,
            )
            .await
            .unwrap();
        assert_eq!(
            t.attachments,
            vec![note.clone()],
            "duplicates and blank ids are dropped"
        );
        assert_eq!(
            r.ledger.load(&t.id).await.unwrap().attachments,
            vec![note.clone()],
            "read back from sqlite"
        );

        // replace whole: one detached, one attached; order as given
        let t = r
            .ledger
            .update(
                &t.id,
                TaskPatch {
                    attachments: Some(vec![art.clone()]),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        assert_eq!(t.attachments, vec![art.clone()]);
        let ev = r.ledger.events(&t.id).await.unwrap();
        let names: Vec<(&str, &str)> = ev
            .iter()
            .filter(|e| e.field == "attached" || e.field == "detached")
            .map(|e| (e.field.as_str(), e.after.as_deref().unwrap()))
            .collect();
        assert_eq!(
            names,
            [
                ("detached", "note \"Groceries\""),
                ("attached", "artifact \"Q3 report\"")
            ]
        );
        let thread = r.thread.0.lock().unwrap()[t.thread.as_ref().unwrap()].join("\n");
        assert!(
            thread.contains("**user** detached: note \"Groceries\""),
            "{thread}"
        );
        assert!(
            thread.contains("**user** attached: artifact \"Q3 report\""),
            "{thread}"
        );

        // same set again = no change, no event
        let before = ev.len();
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    attachments: Some(vec![art.clone()]),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        assert_eq!(r.ledger.events(&t.id).await.unwrap().len(), before);
    }

    /// 70 DESK-PROTOCOL (c): a closed child wakes the parent's owner
    /// `children_completed` once every child is closed; a resolved blocker
    /// wakes `blockers_resolved`.
    #[tokio::test]
    async fn children_completed_and_blockers_resolved_wakes() {
        let mut r = rig().await;
        let parent = r.task("agent:lead").await;
        async fn child(r: &mut Rig, parent: &TaskId, title: &str) -> Task {
            r.ledger
                .create(
                    TaskCreate {
                        state: TaskState::Todo,
                        title: title.to_owned(),
                        brief: String::new(),
                        owner: Owner::Agent("ops".into()),
                        reviewer: Some(Owner::User),
                        parent: Some(parent.clone()),
                        project: None,
                        where_: None,
                        harness: None,
                        external: vec![],
                        done_when: None,
                        attachments: vec![],
                    },
                    "agent:lead",
                    None,
                    WakeReason::Assigned,
                )
                .await
                .unwrap()
        }
        let c1 = child(&mut r, &parent.id, "c1").await;
        let c2 = child(&mut r, &parent.id, "c2").await;
        r.ledger
            .checkout(&parent.id, "s-lead", "agent:lead")
            .await
            .unwrap();
        r.blocked(&parent.id, &c1.id.0, "agent:lead").await.unwrap();
        r.set_state(&c1.id, TaskState::Cancelled, "user")
            .await
            .unwrap();
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter()
                .any(|w| w.agent == "lead" && w.reason == WakeReason::BlockersResolved),
            "{w:?}"
        );
        assert!(
            !w.iter().any(|w| w.reason == WakeReason::ChildrenCompleted),
            "c2 is still open: {w:?}"
        );
        r.set_state(&c2.id, TaskState::Cancelled, "user")
            .await
            .unwrap();
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter().any(|w| w.agent == "lead"
                && w.task == Some(parent.id.clone())
                && w.reason == WakeReason::ChildrenCompleted),
            "{w:?}"
        );
    }

    /// 70 DESK-PROTOCOL 1: the fit check hands a task over — owner changes,
    /// checkout released, back to `todo`, thread says why, new owner woken `reassigned`.
    #[tokio::test]
    async fn reassign_wakes_the_specialist() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        assert!(
            r.ledger
                .reassign(&t.id, Owner::Agent("karol".into()), "", "agent:ops", None)
                .await
                .unwrap_err()
                .contains("why")
        );
        assert!(
            r.ledger
                .reassign(
                    &t.id,
                    Owner::Agent("karol".into()),
                    "k8s",
                    "agent:lead",
                    None
                )
                .await
                .unwrap_err()
                .contains("owner")
        );
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let t = r
            .ledger
            .reassign(
                &t.id,
                Owner::Agent("karol".into()),
                "k8s cluster health is karol's capability",
                "agent:ops",
                Some("s-ops"),
            )
            .await
            .unwrap();
        assert_eq!(t.owner, Owner::Agent("karol".into()));
        assert_eq!(t.state, TaskState::Todo);
        assert!(t.checkout.is_none());
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter()
                .any(|w| w.agent == "karol" && w.reason == WakeReason::Reassigned),
            "{w:?}"
        );
        let thread = r.thread.0.lock().unwrap()[t.thread.as_ref().unwrap()].join("\n");
        assert!(
            thread.contains("reassigned to agent:karol: k8s cluster health is karol's capability"),
            "{thread}"
        );
        assert!(
            r.ledger
                .reassign(&t.id, Owner::Agent("karol".into()), "again", "user", None)
                .await
                .unwrap_err()
                .contains("already owns")
        );
    }

    #[tokio::test]
    async fn checkout_conflict_and_epoch() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let e = r
            .ledger
            .checkout(&t.id, "s-ops-2", "agent:ops")
            .await
            .unwrap_err();
        assert!(e.starts_with("conflict"), "{e}");
        assert!(
            r.ledger
                .checkout(&t.id, "s-ops-2", "user")
                .await
                .unwrap_err()
                .starts_with("conflict")
        );
        // sent back → checkout is gone → a new checkout bumps the epoch
        r.set_state(&t.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        r.set_state(&t.id, TaskState::Doing, "agent:lead")
            .await
            .unwrap();
        let t = r
            .ledger
            .checkout(&t.id, "s-ops-2", "agent:ops")
            .await
            .unwrap();
        assert_eq!(
            t.checkout.as_ref().unwrap().epoch,
            1,
            "epoch restarts once the checkout was cleared"
        );
        assert_eq!(t.checkout.as_ref().unwrap().engine, "mac");
    }

    #[tokio::test]
    async fn project_defaults_and_filter() {
        let mut r = rig().await;
        let parent = r
            .ledger
            .create(
                TaskCreate {
                    state: TaskState::Todo,
                    title: "parent".into(),
                    brief: String::new(),
                    owner: Owner::User,
                    reviewer: None,
                    parent: None,
                    project: Some("argo".into()),
                    where_: None,
                    harness: None,
                    external: vec![],
                    done_when: None,
                    attachments: vec![],
                },
                "user",
                None,
                WakeReason::Assigned,
            )
            .await
            .unwrap();
        let child = r
            .ledger
            .create(
                TaskCreate {
                    state: TaskState::Todo,
                    title: "child".into(),
                    brief: String::new(),
                    owner: Owner::Agent("ops".into()),
                    reviewer: None,
                    parent: Some(parent.id.clone()),
                    project: None,
                    where_: None,
                    harness: None,
                    external: vec![],
                    done_when: None,
                    attachments: vec![],
                },
                "user",
                None,
                WakeReason::Assigned,
            )
            .await
            .unwrap();
        assert_eq!(child.project.as_deref(), Some("argo"));
        let loose = r.task("agent:ops").await;
        assert_eq!(loose.project, None);
        let argo = r
            .ledger
            .list(&TaskFilter {
                project: Some("argo".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            argo.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["parent", "child"]
        );
        let all = r.ledger.list(&TaskFilter::default()).await.unwrap();
        assert_eq!(all.last().unwrap().id, loose.id, "no project sorts last");
        let ops = r
            .ledger
            .list(&TaskFilter {
                owner: Some("agent:ops".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(ops.len(), 2);
        // project is patchable and logged
        let t = r
            .ledger
            .update(
                &loose.id,
                TaskPatch {
                    project: Some("argo".into()),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        assert_eq!(t.project.as_deref(), Some("argo"));
        assert!(
            r.ledger
                .events(&loose.id)
                .await
                .unwrap()
                .iter()
                .any(|e| e.field == "project" && e.after.as_deref() == Some("argo"))
        );
    }

    #[tokio::test]
    async fn blocker_cycle_refused() {
        let mut r = rig().await;
        let a = r.task("agent:ops").await;
        let b = r.task("agent:ops").await;
        let c = r.task("agent:ops").await;
        r.blockers(&a.id, &[&b.id.0], "user").await.unwrap();
        r.blockers(&b.id, &[&c.id.0], "user").await.unwrap();
        let e = r.blockers(&c.id, &[&a.id.0], "user").await.unwrap_err();
        assert!(e.starts_with("blockers would form a cycle"), "{e}");
        assert!(e.contains(&a.id.0[..8]) && e.contains(&c.id.0[..8]), "{e}");
        assert!(
            r.blockers(&a.id, &[&a.id.0], "user")
                .await
                .unwrap_err()
                .contains("cycle")
        );
        // non-task entries and the parent link never cycle
        r.blockers(&c.id, &["decision:x", "person:sky"], "user")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn closing_a_blocker_unblocks_to_previous_state() {
        let mut r = rig().await;
        let dep = r.task("agent:lead").await;
        let waiting = r.task("agent:ops").await;
        let held = r.task("agent:ops").await;
        r.ledger
            .checkout(&held.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.blocked(&waiting.id, &dep.id.0, "agent:ops")
            .await
            .unwrap();
        r.blocked(&held.id, &dep.id.0, "agent:ops").await.unwrap();
        // inverse
        assert_eq!(
            r.ledger.blocks_of(&dep.id).await.unwrap(),
            vec![waiting.id.clone(), held.id.clone()]
        );
        assert_eq!(r.ledger.get(&dep.id).await.unwrap().blocks.len(), 2);
        assert!(r.ledger.get(&waiting.id).await.unwrap().blocks.is_empty());
        // dep is cancelled → both are free; one had a checkout
        r.set_state(&dep.id, TaskState::Cancelled, "user")
            .await
            .unwrap();
        let w = r.ledger.get(&waiting.id).await.unwrap();
        assert_eq!((w.state, w.blockers.is_empty()), (TaskState::Todo, true));
        let h = r.ledger.get(&held.id).await.unwrap();
        assert_eq!(h.state, TaskState::Doing);
        assert!(r.ledger.blocks_of(&dep.id).await.unwrap().is_empty());
        let resumes = r
            .ledger
            .wakes()
            .await
            .unwrap()
            .into_iter()
            .filter(|w| w.reason == WakeReason::BlockersResolved)
            .count();
        assert_eq!(resumes, 2, "one blockers_resolved wake per freed task");
        // a task with two blockers waits for both
        let two = r.task("agent:ops").await;
        let d1 = r.task("user").await;
        let d2 = r.task("user").await;
        r.ledger
            .update(
                &two.id,
                TaskPatch {
                    state: Some(TaskState::Blocked),
                    blockers: Some(vec![d1.id.0.clone(), d2.id.0.clone()]),
                    ..Default::default()
                },
                "agent:ops",
                None,
            )
            .await
            .unwrap();
        r.set_state(&d1.id, TaskState::Cancelled, "user")
            .await
            .unwrap();
        assert_eq!(
            r.ledger.get(&two.id).await.unwrap().state,
            TaskState::Blocked
        );
        r.set_state(&d2.id, TaskState::Cancelled, "user")
            .await
            .unwrap();
        assert_eq!(r.ledger.get(&two.id).await.unwrap().state, TaskState::Todo);
    }

    // ---- wakes (commit 2) ----------------------------------------------------------

    #[tokio::test]
    async fn wakes_coalesce_on_key() {
        let r = rig().await;
        let id = TaskId::new();
        let w1 = r
            .ledger
            .store
            .raise("ops", Some(&id), WakeReason::Comment, 1)
            .await
            .unwrap();
        let w2 = r
            .ledger
            .store
            .raise("ops", Some(&id), WakeReason::Comment, 2)
            .await
            .unwrap();
        assert_eq!((w1.id.as_str(), w1.count, w2.count), (w2.id.as_str(), 1, 2));
        r.ledger
            .store
            .raise("ops", Some(&id), WakeReason::Mention, 3)
            .await
            .unwrap();
        r.ledger
            .store
            .raise("ops", None, WakeReason::Manual, 4)
            .await
            .unwrap();
        let all = r.ledger.wakes().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].key, format!("ops|{id}|comment"));
        assert_eq!(all[2].key, "ops||manual");
    }

    #[tokio::test]
    async fn dispatch_claims_oldest_per_agent_and_retries() {
        let mut r = rig().await;
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("ops".into(), "d-ops".into());
        let t1 = r.task("agent:ops").await;
        r.tick(10);
        let t2 = r.task("agent:ops").await;
        r.tick(10);
        let t3 = r.task("agent:lead").await;
        let mut d = r.ledger.dispatch().await.unwrap();
        d.sort_by(|a, b| a.wake.agent.cmp(&b.wake.agent));
        assert_eq!(d.len(), 2);
        assert_eq!(
            (
                d[0].wake.agent.as_str(),
                d[0].wake.task.as_ref(),
                d[0].wake.claimed_by.as_deref()
            ),
            ("lead", Some(&t3.id), Some(DISPATCHING))
        );
        assert_eq!(
            (
                d[1].wake.agent.as_str(),
                d[1].wake.task.as_ref(),
                d[1].wake.claimed_by.as_deref()
            ),
            ("ops", Some(&t1.id), Some("d-ops"))
        );
        assert!(
            r.ledger.dispatch().await.unwrap().is_empty(),
            "one in flight per agent"
        );
        // lead's desk opened during delivery: the placeholder becomes the desk id
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("lead".into(), "d-lead".into());
        r.ledger.delivered(&d[0].wake).await.unwrap();
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!(
            w.iter()
                .find(|w| w.agent == "lead")
                .unwrap()
                .claimed_by
                .as_deref(),
            Some("d-lead")
        );
        // ops's desk disposes of t1 → its wake is finished → t2 is next
        r.ledger
            .checkout(&t1.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.set_state(&t1.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            d.iter()
                .map(|d| d.wake.task.clone().unwrap())
                .collect::<Vec<_>>(),
            vec![t2.id.clone()]
        );
        // a failed delivery is unclaimed and held back, then retried
        r.ledger.delivery_failed(&d[0].wake).await.unwrap();
        assert!(
            r.ledger.dispatch().await.unwrap().is_empty(),
            "held back 5 s"
        );
        r.tick(5_001);
        let again = r.ledger.dispatch().await.unwrap();
        assert_eq!(again[0].wake.task, Some(t2.id.clone()));
    }

    #[tokio::test]
    async fn no_progress_backoff_and_human_reset() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let id = t.id.clone();
        let cycle = async |r: &mut Rig| {
            r.ledger
                .store
                .raise("ops", Some(&id), WakeReason::Manual, 0)
                .await
                .unwrap();
            let d = r.ledger.dispatch().await.unwrap();
            assert_eq!(d.len(), 1, "wake dispatched");
            // the desk ends the wake with no field change (`continuing`)
            r.ledger
                .update(&id, TaskPatch::default(), "agent:ops", Some("s-desk-ops"))
                .await
                .unwrap()
        };
        let t = cycle(&mut r).await;
        assert_eq!((t.no_progress_count, t.next_wake_after_ms), (1, None));
        let now = r.clock.load(Ordering::SeqCst);
        let t = cycle(&mut r).await;
        assert_eq!(
            (t.no_progress_count, t.next_wake_after_ms),
            (2, Some(now + 120_000))
        );
        // in back-off: the wake waits
        r.ledger
            .store
            .raise("ops", Some(&id), WakeReason::Manual, 0)
            .await
            .unwrap();
        assert!(r.ledger.dispatch().await.unwrap().is_empty());
        r.tick(120_001);
        let t = cycle(&mut r).await;
        assert_eq!(
            t.next_wake_after_ms,
            Some(r.clock.load(Ordering::SeqCst) + 240_000)
        );
        r.tick(240_001);
        for _ in 0..5 {
            let after = r.clock.load(Ordering::SeqCst);
            let t = cycle(&mut r).await;
            r.tick(t.next_wake_after_ms.unwrap() - after + 1);
        }
        let t = r.ledger.get(&id).await.unwrap();
        assert_eq!(t.no_progress_count, 8);
        // capped at backoff_max
        r.ledger
            .store
            .raise("ops", Some(&id), WakeReason::Manual, 0)
            .await
            .unwrap();
        r.ledger.dispatch().await.unwrap();
        let before = r.clock.load(Ordering::SeqCst);
        let t = r
            .ledger
            .update(&id, TaskPatch::default(), "agent:ops", Some("s-desk-ops"))
            .await
            .unwrap();
        assert_eq!(t.next_wake_after_ms, Some(before + 1_800_000));
        // a human comment clears it; a real field change resets the count
        r.ledger
            .comment(&id, "still needed?", false, "user", None)
            .await
            .unwrap();
        let t = r.ledger.get(&id).await.unwrap();
        assert_eq!((t.no_progress_count, t.next_wake_after_ms), (0, None));
        assert!(
            !r.ledger.dispatch().await.unwrap().is_empty(),
            "the comment wake goes out at once"
        );
        r.ledger
            .update(
                &id,
                TaskPatch {
                    title: Some("renamed".into()),
                    ..Default::default()
                },
                "agent:ops",
                None,
            )
            .await
            .unwrap();
        r.ledger
            .store
            .raise("ops", Some(&id), WakeReason::Manual, 0)
            .await
            .unwrap();
        cycle(&mut r).await;
        let t = r.ledger.get(&id).await.unwrap();
        assert_eq!(
            t.no_progress_count, 1,
            "progress in between restarted the count"
        );
    }

    /// `update()` from `session` on `t` with a fresh title.
    async fn retitle(r: &mut Rig, t: &Task, actor: &str, session: &str) -> R<Task> {
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    title: Some(format!("retitled by {session}")),
                    ..Default::default()
                },
                actor,
                Some(session),
            )
            .await
    }

    async fn used(r: &Rig, session: &str) -> u32 {
        r.ledger.write_budget(session).await.unwrap().unwrap().used
    }

    /// A router with the `rig()` wiring plus `extra` (`(kind, from, to)`;
    /// kind `m` = manager, `s` = session), swapped into the ledger.
    fn reroute(r: &mut Rig, extra: &[(&str, &str, &str)]) {
        let mut router = MemRouter::default();
        router.managers.insert("ops".into(), "lead".into());
        router.sessions.insert("s-ops".into(), "ops".into());
        router.sessions.insert("s-lead".into(), "lead".into());
        router.sessions.insert("s-desk-ops".into(), "ops".into());
        for (kind, from, to) in extra {
            match *kind {
                "m" => router.managers.insert((*from).into(), (*to).into()),
                _ => router.sessions.insert((*from).into(), (*to).into()),
            };
        }
        r.ledger.router = Some(Arc::new(router));
    }

    #[tokio::test]
    async fn cross_task_write_cap() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 2;
        let a = r.task("agent:lead").await;
        let b = r.task("agent:lead").await;
        let c = r.task("agent:lead").await;
        let mine = r.task("agent:ops").await;
        retitle(&mut r, &a, "agent:ops", "s-ops").await.unwrap();
        retitle(&mut r, &b, "agent:ops", "s-ops").await.unwrap();
        let e = retitle(&mut r, &c, "agent:ops", "s-ops").await.unwrap_err();
        assert!(e.starts_with("tasks.cross_task_write_cap"), "{e}");
        assert!(e.contains("2 tasks") && e.contains("24 h"), "{e}");
        // D3: notes are free
        r.ledger
            .comment(&c.id, "three", false, "agent:ops", Some("s-ops"))
            .await
            .unwrap();
        // a repeat write to a charged task is free
        retitle(&mut r, &a, "agent:ops", "s-ops").await.unwrap();
        // own tasks and other sessions are unaffected
        retitle(&mut r, &mine, "agent:ops", "s-ops").await.unwrap();
        retitle(&mut r, &c, "agent:lead", "s-lead").await.unwrap();
        // a checkout holder owns the task for the cap even when not the owner agent
        let d = r.task("agent:lead").await;
        r.ledger
            .checkout(&d.id, "s-desk-ops", "agent:lead")
            .await
            .unwrap();
        retitle(&mut r, &d, "agent:ops", "s-desk-ops")
            .await
            .unwrap();
        assert_eq!(used(&r, "s-desk-ops").await, 0);
    }

    #[tokio::test]
    async fn theuth_and_user_sessions_are_uncapped() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 1;
        reroute(&mut r, &[("s", "s-theuth", "theuth")]);
        let tasks = [
            r.task("agent:lead").await,
            r.task("agent:lead").await,
            r.task("agent:lead").await,
            r.task("user").await,
        ];
        for (session, actor) in [("s-user", "user"), ("s-theuth", "agent:theuth")] {
            for t in &tasks {
                retitle(&mut r, t, actor, session).await.unwrap();
            }
            assert_eq!(r.ledger.write_budget(session).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn manager_writes_to_its_line_for_free() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 1;
        reroute(&mut r, &[("m", "sre", "ops"), ("s", "s-lead-2", "lead")]);
        let line = [
            r.task("agent:ops").await,
            r.task("agent:sre").await,
            r.task("agent:sre").await,
        ];
        for t in &line {
            retitle(&mut r, t, "agent:lead", "s-lead").await.unwrap();
        }
        assert_eq!(used(&r, "s-lead").await, 0);
        retitle(&mut r, &line[1], "agent:lead", "s-lead-2")
            .await
            .unwrap();
        assert_eq!(used(&r, "s-lead-2").await, 0);
        // a report writing upward is not in line
        let up = r.task("agent:lead").await;
        retitle(&mut r, &up, "agent:ops", "s-ops").await.unwrap();
        assert_eq!(used(&r, "s-ops").await, 1);
    }

    #[tokio::test]
    async fn user_owned_task_is_foreign_to_a_hired_agent() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 1;
        let a = r.task("user").await;
        let b = r.task("user").await;
        retitle(&mut r, &a, "agent:ops", "s-ops").await.unwrap();
        let e = retitle(&mut r, &b, "agent:ops", "s-ops").await.unwrap_err();
        assert!(e.starts_with("tasks.cross_task_write_cap"), "{e}");
    }

    #[tokio::test]
    async fn budget_window_decays() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 1;
        r.ledger.cfg.cross_task_write_window_secs = 3600;
        let a = r.task("agent:lead").await;
        let b = r.task("agent:lead").await;
        retitle(&mut r, &a, "agent:ops", "s-ops").await.unwrap();
        let e = retitle(&mut r, &b, "agent:ops", "s-ops").await.unwrap_err();
        assert!(e.contains("in the last 1 h"), "{e}");
        r.tick(3_600_001);
        retitle(&mut r, &b, "agent:ops", "s-ops").await.unwrap();
        assert_eq!(used(&r, "s-ops").await, 1);
    }

    #[tokio::test]
    async fn window_zero_is_lifetime() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 1;
        r.ledger.cfg.cross_task_write_window_secs = 0;
        let a = r.task("agent:lead").await;
        let b = r.task("agent:lead").await;
        retitle(&mut r, &a, "agent:ops", "s-ops").await.unwrap();
        r.tick(10 * 86_400_000);
        let e = retitle(&mut r, &b, "agent:ops", "s-ops").await.unwrap_err();
        assert!(e.contains("for this session's lifetime"), "{e}");
        assert_eq!(used(&r, "s-ops").await, 1);
    }

    #[tokio::test]
    async fn inbox_trailer_reports_budget() {
        let mut r = rig().await;
        let a = r.task("agent:lead").await;
        retitle(&mut r, &a, "agent:ops", "s-ops").await.unwrap();
        assert_eq!(
            r.ledger.write_budget("s-ops").await.unwrap(),
            Some(crate::WriteBudget {
                used: 1,
                cap: 20,
                window_secs: 86_400
            })
        );
        assert_eq!(window_text(86_400), "24 h");
        assert_eq!(window_text(5_400), "90 min");
        assert_eq!(window_text(45), "45 s");
    }

    #[tokio::test]
    async fn comment_wakes_owner_reviewer_and_mentions() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        // the owner's own comment wakes nobody; a mention of lead does
        r.ledger
            .comment(
                &t.id,
                "asking lead about this",
                false,
                "agent:ops",
                Some("s-ops"),
            )
            .await
            .unwrap();
        let reasons: Vec<_> = r
            .ledger
            .wakes()
            .await
            .unwrap()
            .into_iter()
            .map(|w| (w.agent, w.reason))
            .collect();
        assert!(reasons.contains(&("lead".into(), WakeReason::Mention)));
        assert!(!reasons.contains(&("ops".into(), WakeReason::Comment)));
        // the user's comment wakes the owner; under review also the reviewer
        r.ledger
            .comment(&t.id, "go", false, "user", None)
            .await
            .unwrap();
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.set_state(&t.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        r.ledger
            .comment(&t.id, "looks off", false, "user", None)
            .await
            .unwrap();
        let reasons: Vec<_> = r
            .ledger
            .wakes()
            .await
            .unwrap()
            .into_iter()
            .map(|w| (w.agent, w.reason))
            .collect();
        assert!(reasons.contains(&("ops".into(), WakeReason::Comment)));
        assert!(reasons.contains(&("lead".into(), WakeReason::Comment)));
        assert!(reasons.contains(&("lead".into(), WakeReason::Review)));
        let doc = t.thread.unwrap();
        let blocks = r.thread.0.lock().unwrap().get(&doc).cloned().unwrap();
        assert!(
            blocks
                .iter()
                .any(|b| b.starts_with("**user** (") && b.ends_with("): looks off"))
        );
        assert!(
            blocks
                .iter()
                .any(|b| b.contains("**user** woke ops (comment)")),
            "{blocks:?}"
        );
        assert!(
            r.ledger
                .events(&t.id)
                .await
                .unwrap()
                .iter()
                .any(|e| e.field == "wake" && e.after.as_deref() == Some("ops"))
        );
    }

    #[tokio::test]
    async fn raise_requeues_a_claimed_wake() {
        let mut r = rig().await;
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("ops".into(), "d-ops".into());
        let t = r.task("agent:ops").await;
        // the `assigned` wake goes out and is completed by the owner's write
        r.ledger.dispatch().await.unwrap();
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    title: Some("deploy".into()),
                    ..Default::default()
                },
                "agent:ops",
                None,
            )
            .await
            .unwrap();
        assert!(r.ledger.wakes().await.unwrap().is_empty());
        r.ledger
            .comment(&t.id, "first", false, "user", None)
            .await
            .unwrap();
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            d.iter().map(|d| d.wake.reason).collect::<Vec<_>>(),
            vec![WakeReason::Comment]
        );
        assert!(
            r.ledger.dispatch().await.unwrap().is_empty(),
            "claimed, one in flight"
        );
        // a second comment before the desk acted: the row is re-queued, not absorbed
        r.ledger
            .comment(&t.id, "second", false, "user", None)
            .await
            .unwrap();
        let w = r.ledger.wakes().await.unwrap();
        let w = w.iter().find(|w| w.reason == WakeReason::Comment).unwrap();
        assert_eq!((w.count, w.claimed_by.as_deref()), (2, None));
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            (d.len(), d[0].wake.count, d[0].wake.claimed_by.as_deref()),
            (1, 2, Some("d-ops"))
        );
    }

    /// STALE-WAKE-DESKS incident: the reviewer's `review → done` left its
    /// `review` wake claimed; a restart re-armed it and the task was reviewed
    /// twice. Now the terminal transition drops every wake on the task
    /// (whoever moved it), a comment on the done task wakes nobody, and the
    /// reviewer is no longer "busy" — its next wake goes out.
    #[tokio::test]
    async fn terminal_transition_drops_wakes_and_frees_the_reviewer() {
        let mut r = rig().await;
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("ops".into(), "d-ops".into());
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("lead".into(), "d-lead".into());
        let t = r.task("agent:ops").await;
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    reviewer: Some("agent:lead".parse().unwrap()),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        r.ledger.dispatch().await.unwrap();
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.set_state(&t.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            d.iter()
                .map(|d| (d.wake.agent.as_str(), d.wake.reason))
                .collect::<Vec<_>>(),
            vec![("lead", WakeReason::Review)]
        );
        // the reviewer closes it (not the owner): its wake is finished with the transition
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    state: Some(TaskState::Done),
                    done_because: Some(DoneBecause::IntentMet),
                    ..Default::default()
                },
                "agent:lead",
                Some("d-lead"),
            )
            .await
            .unwrap();
        assert!(
            r.ledger.wakes().await.unwrap().is_empty(),
            "no wake outlives a done task"
        );
        // a comment on the done task raises nothing
        r.ledger
            .comment(&t.id, "great work", false, "user", None)
            .await
            .unwrap();
        assert!(r.ledger.wakes().await.unwrap().is_empty());
        // the reviewer is free: a new task for it is dispatched at once
        r.task("agent:lead").await;
        assert_eq!(
            r.ledger.dispatch().await.unwrap().len(),
            1,
            "lead is not held busy by the finished review"
        );
    }

    /// A wake whose reason no longer fits its task (rows left by earlier
    /// builds, or a race) is dropped at dispatch with a thread line; a still
    /// applicable one on the same task goes out.
    #[tokio::test]
    async fn stale_wake_is_dropped_at_dispatch_not_delivered() {
        let mut r = rig().await;
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("lead".into(), "d-lead".into());
        let t = r.task("agent:ops").await;
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    state: Some(TaskState::Cancelled),
                    ..Default::default()
                },
                "user",
                None,
            )
            .await
            .unwrap();
        assert!(
            r.ledger.wakes().await.unwrap().is_empty(),
            "the assigned wake went with the transition"
        );
        // rows written by a pre-fix engine
        let stale = r
            .ledger
            .store
            .raise("lead", Some(&t.id), WakeReason::Review, 0)
            .await
            .unwrap();
        r.ledger
            .store
            .raise("ops", Some(&t.id), WakeReason::Comment, 1)
            .await
            .unwrap();
        let gone = r
            .ledger
            .store
            .raise("ops", Some(&TaskId::new()), WakeReason::Manual, 2)
            .await
            .unwrap();
        let todo = r.task("agent:lead").await;
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            d.iter().map(|d| d.wake.task.clone()).collect::<Vec<_>>(),
            vec![Some(todo.id.clone())],
            "only the live wake is delivered"
        );
        let left: Vec<String> = r
            .ledger
            .wakes()
            .await
            .unwrap()
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert!(
            !left.contains(&stale.id) && !left.contains(&gone.id) && left.len() == 1,
            "{left:?}"
        );
        let thread = r.thread.read(t.thread.as_deref().unwrap()).await.unwrap();
        assert!(
            thread.contains("wake `review` for lead dropped: stale (task is cancelled)"),
            "{thread}"
        );
        assert!(
            thread.contains("wake `comment` for ops dropped: stale (task is cancelled)"),
            "{thread}"
        );
    }

    /// The roster's turn-end verdict finishes the row even when the agent
    /// never wrote the task (a comment answered with a comment); a reopened
    /// desk releases what its dead turn had claimed.
    #[tokio::test]
    async fn finish_wake_and_release_claims() {
        let mut r = rig().await;
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("ops".into(), "d-ops".into());
        let t = r.task("agent:ops").await;
        let d = r.ledger.dispatch().await.unwrap();
        assert!(
            r.ledger.dispatch().await.unwrap().is_empty(),
            "claimed: ops is busy"
        );
        r.ledger.release_claims("d-ops").await.unwrap();
        let again = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            again[0].wake.id, d[0].wake.id,
            "released → dispatched again"
        );
        r.ledger.finish_wake(&again[0].wake).await.unwrap();
        assert!(r.ledger.wakes().await.unwrap().is_empty());
        r.ledger
            .comment(&t.id, "ping", false, "user", None)
            .await
            .unwrap();
        assert_eq!(
            r.ledger.dispatch().await.unwrap().len(),
            1,
            "ops is free again"
        );
    }

    /// WAKE-LIVE-CHILD D2: a wake handed back by a turn that did not yield is
    /// unclaimed and re-dispatched after its floor; without a floor the
    /// delivery back-off (5 s) applies.
    #[tokio::test]
    async fn release_wake_unclaims_with_a_floor() {
        let mut r = rig().await;
        r.task("agent:ops").await;
        let d = r.ledger.dispatch().await.unwrap();
        let w = &d[0].wake;
        r.ledger.release_wake(w, 300_000).await.unwrap();
        let row = r.ledger.wakes().await.unwrap().remove(0);
        assert_eq!((row.id.as_str(), row.claimed_by), (w.id.as_str(), None));
        r.tick(299_000);
        assert!(
            r.ledger.dispatch().await.unwrap().is_empty(),
            "under the floor"
        );
        r.tick(2_000);
        let again = r.ledger.dispatch().await.unwrap();
        assert_eq!(again[0].wake.id, w.id, "delivered again after the floor");
        r.ledger.delivered(&again[0].wake).await.unwrap();
        r.ledger.release_wake(&again[0].wake, 0).await.unwrap();
        r.tick(4_000);
        assert!(
            r.ledger.dispatch().await.unwrap().is_empty(),
            "5 s back-off"
        );
        r.tick(2_000);
        assert_eq!(r.ledger.dispatch().await.unwrap().len(), 1);
    }

    /// WAKE-LIVE-CHILD D3: the busy skip is reported once per blocking wake
    /// and forgotten when the agent is free.
    #[tokio::test]
    async fn busy_skip_is_reported_once_per_blocking_wake() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let a = r.ledger.dispatch().await.unwrap().remove(0).wake;
        assert!(r.ledger.reported_busy.is_empty(), "nothing waits yet");
        r.ledger
            .comment(&t.id, "ping", false, "user", None)
            .await
            .unwrap();
        for _ in 0..2 {
            assert!(r.ledger.dispatch().await.unwrap().is_empty());
            assert_eq!(r.ledger.reported_busy.get("ops"), Some(&a.id));
        }
        r.ledger.finish_wake(&a).await.unwrap();
        let b = r.ledger.dispatch().await.unwrap();
        assert_eq!((b.len(), b[0].wake.reason), (1, WakeReason::Comment));
        assert!(!r.ledger.reported_busy.contains_key("ops"));
    }

    #[tokio::test]
    async fn send_back_wakes_owner_resumed_and_clears_backoff() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.set_state(&t.id, TaskState::Review, "agent:ops")
            .await
            .unwrap();
        let mut backed = r.ledger.get(&t.id).await.unwrap();
        backed.no_progress_count = 3;
        backed.next_wake_after_ms = Some(r.clock.load(Ordering::SeqCst) + 600_000);
        r.ledger.save(&backed).await.unwrap();
        let t = r
            .ledger
            .move_to(&t.id, TaskState::Doing, None, None, "user", None)
            .await
            .unwrap();
        assert_eq!(
            (t.state, t.no_progress_count, t.next_wake_after_ms),
            (TaskState::Doing, 0, None)
        );
        let reasons: Vec<_> = r
            .ledger
            .wakes()
            .await
            .unwrap()
            .into_iter()
            .map(|w| (w.agent, w.reason))
            .collect();
        assert!(
            reasons.contains(&("ops".into(), WakeReason::Resumed)),
            "{reasons:?}"
        );
        let doc = t.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap().get(&doc).cloned().unwrap();
        assert!(
            blocks
                .iter()
                .any(|b| b.contains("**user** woke ops (resumed)")),
            "{blocks:?}"
        );
        assert!(
            r.ledger
                .dispatch()
                .await
                .unwrap()
                .iter()
                .any(|d| d.wake.agent == "ops"),
            "no back-off left"
        );
    }

    // ---- orphans, epochs, episodes (commit 3) ---------------------------------------

    #[tokio::test]
    async fn orphan_by_unreachable_engine_then_superseded() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let others: HashSet<String> = HashSet::from(["build-a".to_owned()]);
        assert!(
            r.ledger.sweep(Some(&others)).await.unwrap().is_empty(),
            "first sight starts the timer"
        );
        r.tick(119_000);
        assert!(r.ledger.sweep(Some(&others)).await.unwrap().is_empty());
        // it came back: the timer restarts
        let with_mac: HashSet<String> = HashSet::from(["mac".to_owned()]);
        r.ledger.sweep(Some(&with_mac)).await.unwrap();
        r.tick(2_000);
        assert!(r.ledger.sweep(Some(&others)).await.unwrap().is_empty());
        r.tick(120_000);
        assert_eq!(
            r.ledger.sweep(Some(&others)).await.unwrap(),
            vec![t.id.clone()]
        );
        let o = r.ledger.get(&t.id).await.unwrap();
        let c = o.checkout.as_ref().unwrap();
        assert!(c.orphaned && c.epoch == 2 && c.session == "s-ops");
        assert_eq!(o.state, TaskState::Doing);
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Resume)
        );
        assert!(
            r.ledger
                .events(&t.id)
                .await
                .unwrap()
                .iter()
                .any(|e| e.field == "checkout" && e.after.as_deref().unwrap().contains("orphaned"))
        );
        assert!(
            r.ledger.sweep(Some(&others)).await.unwrap().is_empty(),
            "orphaned once"
        );
        // the owner's new session takes it; the old session is superseded
        let n = r
            .ledger
            .checkout(&t.id, "s-ops-2", "agent:ops")
            .await
            .unwrap();
        assert_eq!(n.checkout.as_ref().unwrap().epoch, 3);
        let e = r
            .ledger
            .comment(&t.id, "done so far", true, "agent:ops", Some("s-ops"))
            .await
            .unwrap_err();
        assert!(e.starts_with("superseded"), "{e}");
        assert!(
            r.ledger
                .update(
                    &t.id,
                    TaskPatch {
                        state: Some(TaskState::Review),
                        ..Default::default()
                    },
                    "agent:ops",
                    Some("s-ops")
                )
                .await
                .unwrap_err()
                .starts_with("superseded")
        );
        r.ledger
            .comment(&t.id, "resumed", true, "agent:ops", Some("s-ops-2"))
            .await
            .unwrap();
        assert_eq!(
            r.ledger
                .get(&t.id)
                .await
                .unwrap()
                .checkout
                .unwrap()
                .progress_at_ms,
            r.clock.load(Ordering::SeqCst)
        );
    }

    /// 68 §4.4: the sweep takes a lost engine's session here before the
    /// `Resume` wake; the checkout then names this engine and is marked
    /// `taken`. A refused take keeps the progress-note fallback; a hung local
    /// session is never taken.
    #[tokio::test]
    async fn resume_takes_the_lost_engines_session() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let mut c = r.ledger.get(&t.id).await.unwrap().checkout.unwrap();
        c.engine = "build-a".into();
        let mut moved = r.ledger.get(&t.id).await.unwrap();
        moved.checkout = Some(c);
        r.ledger.save(&moved).await.unwrap();
        let others: HashSet<String> = HashSet::from(["mac".to_owned()]);
        r.ledger.sweep(Some(&others)).await.unwrap();
        r.tick(120_000);
        assert_eq!(
            r.ledger.sweep(Some(&others)).await.unwrap(),
            vec![t.id.clone()]
        );
        assert_eq!(*r.taker.taken.lock().unwrap(), vec!["s-ops".to_owned()]);
        let c = r.ledger.get(&t.id).await.unwrap().checkout.unwrap();
        assert!(c.orphaned && c.taken && c.epoch == 2, "{c:?}");
        assert_eq!(
            (c.engine.as_str(), c.session.as_str()),
            ("mac", "s-ops"),
            "the checkout followed the session here"
        );
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Resume)
        );
        assert!(
            r.thread
                .0
                .lock()
                .unwrap()
                .values()
                .flatten()
                .any(|b| b.contains("taken here (mac)"))
        );
        // the taken session keeps writing its progress notes
        r.ledger
            .comment(&t.id, "going on", true, "agent:ops", Some("s-ops"))
            .await
            .unwrap();

        // refused take: the note falls back, the checkout stays on the lost engine
        let t2 = r.task("agent:ops").await;
        r.ledger
            .checkout(&t2.id, "s-ops-2", "agent:ops")
            .await
            .unwrap();
        let mut moved = r.ledger.get(&t2.id).await.unwrap();
        moved.checkout.as_mut().unwrap().engine = "build-a".into();
        r.ledger.save(&moved).await.unwrap();
        *r.taker.fail.lock().unwrap() = true;
        // build-a has been unreachable since the first sweep: orphaned at once.
        assert_eq!(
            r.ledger.sweep(Some(&others)).await.unwrap(),
            vec![t2.id.clone()]
        );
        let c = r.ledger.get(&t2.id).await.unwrap().checkout.unwrap();
        assert!(c.orphaned && !c.taken && c.engine == "build-a", "{c:?}");
        assert!(
            r.thread
                .0
                .lock()
                .unwrap()
                .values()
                .flatten()
                .any(|b| b.contains("resume from the last progress note"))
        );

        // a hung session on this engine is orphaned by staleness, never taken
        *r.taker.fail.lock().unwrap() = false;
        let t3 = r.task("agent:ops").await;
        r.ledger
            .checkout(&t3.id, "s-ops-3", "agent:ops")
            .await
            .unwrap();
        r.tick(901_000);
        assert_eq!(r.ledger.sweep(None).await.unwrap(), vec![t3.id.clone()]);
        assert_eq!(r.taker.taken.lock().unwrap().len(), 1);
        assert!(!r.ledger.get(&t3.id).await.unwrap().checkout.unwrap().taken);
    }

    #[tokio::test]
    async fn orphan_by_staleness() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.tick(899_000);
        assert!(r.ledger.sweep(None).await.unwrap().is_empty());
        r.ledger
            .comment(&t.id, "still going", true, "agent:ops", Some("s-ops"))
            .await
            .unwrap();
        r.tick(899_000);
        assert!(
            r.ledger.sweep(None).await.unwrap().is_empty(),
            "the note refreshed progress_at"
        );
        r.tick(2_000);
        // reachable engine, hung session
        let mac: HashSet<String> = HashSet::from(["mac".to_owned()]);
        assert_eq!(
            r.ledger.sweep(Some(&mac)).await.unwrap(),
            vec![t.id.clone()]
        );
        // stale wake claims of a moved desk are released
        let w = r
            .ledger
            .store
            .raise("ops", Some(&t.id), WakeReason::Manual, 0)
            .await
            .unwrap();
        r.ledger
            .store
            .claim(&w.id, Some("old-desk"), 0)
            .await
            .unwrap();
        r.router
            .desks
            .lock()
            .unwrap()
            .insert("ops".into(), "new-desk".into());
        r.ledger.sweep(None).await.unwrap();
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .find(|x| x.id == w.id)
                .unwrap()
                .claimed_by
                .is_none()
        );
    }

    /// WAKE-LIVE-CHILD D4: a checkout whose session (or a child of it) is at
    /// work, or was active recently, is not stale however old its progress
    /// note; an idle one past `stale_secs` is orphaned with a `resume` wake;
    /// an unknown session keeps the progress-note rule.
    #[tokio::test]
    async fn checkout_with_live_work_is_not_stale() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.tick(3_600_000);
        let now = r.clock.load(Ordering::SeqCst);
        let set = |r: &Rig, l: Liveness| {
            let p = r.probe.clone();
            async move { p.0.lock().await.insert("s-ops".into(), l) }
        };
        set(
            &r,
            Liveness {
                active_now: true,
                last_active_ms: None,
            },
        )
        .await;
        assert!(
            r.ledger.sweep(None).await.unwrap().is_empty(),
            "a running child is not lost"
        );
        set(
            &r,
            Liveness {
                active_now: false,
                last_active_ms: Some(now - 120_000),
            },
        )
        .await;
        assert!(
            r.ledger.sweep(None).await.unwrap().is_empty(),
            "active 2 min ago is not lost"
        );
        set(
            &r,
            Liveness {
                active_now: false,
                last_active_ms: Some(now - 1_200_000),
            },
        )
        .await;
        assert_eq!(r.ledger.sweep(None).await.unwrap(), vec![t.id.clone()]);
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Resume)
        );
        let doc = t.thread.clone().unwrap();
        assert!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .any(|b| b.contains("presumed lost; last activity 20 min ago")),
        );
        // Unknown session (no agent actor): the progress note alone decides.
        let t2 = r.task("agent:ops").await;
        r.ledger
            .checkout(&t2.id, "s-other", "agent:ops")
            .await
            .unwrap();
        r.tick(899_000);
        assert!(r.ledger.sweep(None).await.unwrap().is_empty());
        r.tick(2_000);
        assert_eq!(r.ledger.sweep(None).await.unwrap(), vec![t2.id.clone()]);
    }

    #[tokio::test]
    async fn episode_closing_blocks_leftover_tasks() {
        let mut r = rig().await;
        let left = r.task("agent:ops").await;
        let cont = r.task("agent:ops").await;
        let other = r.task("agent:ops").await;
        r.ledger
            .checkout(&left.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.ledger
            .checkout(&cont.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        r.ledger
            .checkout(&other.id, "s-elsewhere", "agent:ops")
            .await
            .unwrap();
        r.ledger
            .note_disposition(
                &cont.id,
                "continuing:schedule nightly",
                "agent:ops",
                Some("s-ops"),
            )
            .await
            .unwrap();
        let w = r
            .ledger
            .store
            .raise("ops", Some(&left.id), WakeReason::Manual, 0)
            .await
            .unwrap();
        r.ledger
            .store
            .claim(&w.id, Some("s-desk-ops"), 0)
            .await
            .unwrap();
        let blocked = r
            .ledger
            .episode_closing("ops", &["s-desk-ops".into(), "s-ops".into()])
            .await
            .unwrap();
        assert_eq!(blocked, vec![left.id.clone()]);
        let l = r.ledger.get(&left.id).await.unwrap();
        assert_eq!(
            (l.state, l.blockers.as_slice()),
            (TaskState::Blocked, &[NO_CONTINUATION.to_owned()][..])
        );
        assert_eq!(
            r.ledger.get(&cont.id).await.unwrap().state,
            TaskState::Doing
        );
        assert_eq!(
            r.ledger.get(&other.id).await.unwrap().state,
            TaskState::Doing
        );
        let wakes = r.ledger.wakes().await.unwrap();
        let w = wakes
            .iter()
            .find(|x| x.id == w.id)
            .expect("the desk's unfinished wake survives the episode");
        assert_eq!(
            w.claimed_by, None,
            "released for the next episode (67 §4.6), validated at dispatch"
        );
        assert!(wakes.iter().any(|x| x.agent == "lead"
            && x.reason == WakeReason::Review
            && x.task == Some(left.id.clone())));
    }

    #[tokio::test]
    async fn user_unblock_clears_system_blockers_and_wakes_resumed() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let mut left = r.ledger.get(&t.id).await.unwrap();
        r.ledger
            .no_continuation(&mut left, "ops", "turn ended without a disposition")
            .await
            .unwrap();
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    blockers: Some(vec![NO_CONTINUATION.into(), "engine:repo:/x".into()]),
                    ..Default::default()
                },
                "system",
                None,
            )
            .await
            .unwrap();
        // an agent may not talk its way out
        assert!(
            r.ledger
                .move_to(&t.id, TaskState::Todo, None, None, "agent:ops", None)
                .await
                .unwrap_err()
                .contains("still blocked")
        );
        // the user's unblock clears both system blockers and lands in todo with a `resumed` wake
        let u = r
            .ledger
            .move_to(&t.id, TaskState::Todo, Some("go again"), None, "user", None)
            .await
            .unwrap();
        assert_eq!(
            (u.state, u.blockers.as_slice(), u.no_progress_count),
            (TaskState::Todo, &[][..], 0)
        );
        let w = r.ledger.wakes().await.unwrap();
        assert!(w.iter().any(|x| x.agent == "ops"
            && x.reason == WakeReason::Resumed
            && x.task == Some(t.id.clone())));
        assert!(
            r.ledger
                .events(&t.id)
                .await
                .unwrap()
                .iter()
                .any(|e| e.field == "blockers" && e.after.as_deref() == Some(""))
        );
    }

    #[tokio::test]
    async fn user_unblock_keeps_decision_and_task_blockers() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let other = r.task("agent:ops").await;
        let (decision, _) = r
            .ledger
            .escalate(&t.id, packet::Ask::new("which region?"), "agent:ops", None)
            .await
            .unwrap();
        let blockers: Vec<String> =
            vec![decision.clone(), NO_CONTINUATION.into(), other.id.0.clone()];
        r.ledger
            .update(
                &t.id,
                TaskPatch {
                    blockers: Some(blockers),
                    ..Default::default()
                },
                "system",
                None,
            )
            .await
            .unwrap();
        let err = r
            .ledger
            .move_to(&t.id, TaskState::Todo, None, None, "user", None)
            .await
            .unwrap_err();
        assert!(
            err.contains(&decision) && err.contains("a decision needs its answer"),
            "{err}"
        );
        assert!(
            err.contains(&other.id.0) && !err.contains(NO_CONTINUATION),
            "{err}"
        );
        // a refused unblock changes nothing
        let t2 = r.ledger.get(&t.id).await.unwrap();
        assert_eq!((t2.state, t2.blockers.len()), (TaskState::Blocked, 3));
        assert!(
            !r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .any(|x| x.reason == WakeReason::Resumed)
        );
    }

    // ---- schedules (commit 4) -----------------------------------------------------

    #[tokio::test]
    async fn schedule_fire_creates_coalesces_and_closes() {
        let mut r = rig().await;
        let t = r
            .ledger
            .create_for_schedule(
                "nightly",
                1_000,
                "ops",
                "<schedule>go</schedule>",
                Some("argo".into()),
            )
            .await
            .unwrap();
        assert_eq!(t.schedule, Some(("nightly".into(), 1_000)));
        assert_eq!(
            (t.owner.clone(), t.project.as_deref(), t.created_by.as_str()),
            (Owner::Agent("ops".into()), Some("argo"), "system")
        );
        assert_eq!(t.brief, "<schedule>go</schedule>");
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!(
            (w.len(), w[0].reason, w[0].count),
            (1, WakeReason::Schedule, 1)
        );
        let again = r
            .ledger
            .create_for_schedule("nightly", 1_000, "ops", "<schedule>go</schedule>", None)
            .await
            .unwrap();
        assert_eq!(again.id, t.id);
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!((w.len(), w[0].count), (1, 2));
        let doc = t.thread.clone().unwrap();
        assert!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .any(|b| b.contains("fired again (2×)"))
        );
        let next = r
            .ledger
            .create_for_schedule("nightly", 2_000, "ops", "p", None)
            .await
            .unwrap();
        assert_ne!(next.id, t.id);
        // WAKE-LIVE-CHILD T6: the first fire's wake reaches the desk; the
        // second waits behind it (reported once) and flows once it is finished.
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(d.len(), 1);
        let first = d.into_iter().next().unwrap();
        assert_eq!(
            (
                first.wake.agent.as_str(),
                first.wake.reason,
                first.task.map(|x| x.id)
            ),
            ("ops", WakeReason::Schedule, Some(t.id.clone()))
        );
        assert!(r.ledger.dispatch().await.unwrap().is_empty(), "ops is busy");
        assert_eq!(r.ledger.reported_busy.get("ops"), Some(&first.wake.id));
        r.ledger.finish_wake(&first.wake).await.unwrap();
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(
            (d.len(), d[0].task.as_ref().map(|x| x.id.clone())),
            (1, Some(next.id.clone()))
        );
        r.ledger.finish_wake(&d[0].wake).await.unwrap();
        // `until` passed: every open task of the schedule is done, reviewer system
        r.ledger
            .close_for_schedule("nightly", "done: until passed after 2 runs")
            .await
            .unwrap();
        for id in [&t.id, &next.id] {
            let d = r.ledger.get(id).await.unwrap();
            assert_eq!(
                (d.state, d.reviewer.clone()),
                (TaskState::Done, Some(Owner::System))
            );
            assert!(
                r.ledger
                    .events(id)
                    .await
                    .unwrap()
                    .iter()
                    .any(|e| e.field == "state" && e.after.as_deref() == Some("done"))
            );
        }
        assert_eq!(Owner::System.to_string(), "system");
        assert_eq!("system".parse::<Owner>(), Ok(Owner::System));
    }

    // ---- escalate (67 §11) --------------------------------------------------------

    #[tokio::test]
    async fn escalate_agent_manager_path() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .checkout(&t.id, "s-ops", "agent:ops")
            .await
            .unwrap();
        let (decision, manager) = r
            .ledger
            .escalate(
                &t.id,
                packet::Ask {
                    options: vec![packet::PacketOption {
                        label: "yes".into(),
                        description: None,
                    }],
                    ..packet::Ask::new("ship?")
                },
                "agent:ops",
                Some("s-ops"),
            )
            .await
            .unwrap();
        assert_eq!(manager, Owner::Agent("lead".into()));
        let b = r.ledger.get(&t.id).await.unwrap();
        assert_eq!(
            (b.state, b.blockers.clone()),
            (TaskState::Blocked, vec![decision.clone()])
        );
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .any(|w| w.agent == "lead" && w.reason == WakeReason::Answer)
        );
        let doc = t.thread.clone().unwrap();
        assert!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .any(|b| b.contains("Manager: answer with `decide"))
        );
        // the manager answers: blocker gone, back to doing (live checkout), owner woken `answer`
        r.ledger
            .answer_decision(&decision, "yes", "agent:lead", Some("s-lead"))
            .await
            .unwrap();
        let a = r.ledger.get(&t.id).await.unwrap();
        assert_eq!((a.state, a.blockers.is_empty()), (TaskState::Doing, true));
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Answer)
        );
        assert!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .any(|b| b.contains("**decision** (`decision:") && b.ends_with("yes"))
        );
        // nothing is blocked on it any more
        assert_eq!(
            r.ledger
                .answer_decision(&decision, "again", "user", None)
                .await,
            Err(format!("no task is blocked on {decision}"))
        );
    }

    #[tokio::test]
    async fn escalate_user_path() {
        let mut r = rig().await;
        let t = r.task("agent:lead").await;
        board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let ask = packet::Ask {
            summary: Some("Spend or save.".into()),
            board: Some("b1".into()),
            ..packet::Ask::new("budget?")
        };
        let (_, manager) = r
            .ledger
            .escalate(&t.id, ask, "agent:lead", None)
            .await
            .unwrap();
        assert_eq!(manager, Owner::User);
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter().all(|x| x.reason != WakeReason::Answer),
            "no agent wake for the user"
        );
        assert_eq!(r.ledger.get(&t.id).await.unwrap().state, TaskState::Blocked);
        // the user is the top of the line
        assert!(
            r.ledger
                .escalate(&t.id, packet::Ask::new("?"), "user", None)
                .await
                .is_err()
        );
    }

    // ---- packets (101 DECISION-SURFACE) ----------------------------------------

    fn code_pointer() -> packet::Pointer {
        packet::Pointer {
            kind: packet::PointerKind::Code,
            ref_: "src/x.rs#L12-L14".into(),
            title: "the retry".into(),
            note: Some("three attempts".into()),
            text: Some("a\nb\nc".into()),
            root: Some("/repo".into()),
        }
    }

    /// 101 D1 + R1/R2: an escalation to the user without a summary, without a
    /// board, with a board that does not exist, is a note, or lacks the skeleton
    /// writes nothing; the thread, blockers, wakes and packets stay as they were.
    #[tokio::test]
    async fn escalate_to_user_needs_a_summary_and_a_board() {
        let mut r = rig().await;
        let t = r.task("agent:lead").await;
        let doc = t.thread.clone().unwrap();
        let before = r.thread.0.lock().unwrap()[&doc].len();
        let e = r
            .ledger
            .escalate(&t.id, packet::Ask::new("budget?"), "agent:lead", None)
            .await
            .unwrap_err();
        assert!(e.contains("needs a `board`"), "{e}");
        assert!(e.contains("`## What you're deciding`"), "{e}");
        let with = |board: &str| packet::Ask {
            summary: Some("Spend or save.".into()),
            board: Some(board.into()),
            ..packet::Ask::new("budget?")
        };
        let e = r
            .ledger
            .escalate(&t.id, with("nope"), "agent:lead", None)
            .await
            .unwrap_err();
        assert_eq!(e, "board `nope` does not exist");
        r.thread.append("n1", "# Just a note").await;
        let e = r
            .ledger
            .escalate(&t.id, with("n1"), "agent:lead", None)
            .await
            .unwrap_err();
        assert_eq!(
            e,
            "`n1` is a note, not a board (create it with doc_create(kind=\"board\"))"
        );
        board(&r.thread, "b0", &["What you're deciding", "The picture"]);
        let e = r
            .ledger
            .escalate(&t.id, with("b0"), "agent:lead", None)
            .await
            .unwrap_err();
        assert!(
            e.starts_with(
                "board `b0`: section 2 of a decision board must be `## Why it matters to you`"
            ),
            "{e}"
        );
        board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let e = r
            .ledger
            .escalate(
                &t.id,
                packet::Ask {
                    summary: None,
                    ..with("b1")
                },
                "agent:lead",
                None,
            )
            .await
            .unwrap_err();
        assert!(e.contains("needs a `summary`"), "{e}");
        assert_eq!(
            r.thread.0.lock().unwrap()[&doc].len(),
            before,
            "no thread line"
        );
        let a = r.ledger.get(&t.id).await.unwrap();
        assert_eq!((a.state, a.blockers.is_empty()), (TaskState::Todo, true));
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .all(|w| w.reason != WakeReason::Answer)
        );
        assert!(
            r.ledger
                .packets_for(std::slice::from_ref(&t.id))
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 101 §3: the packet is stored under the decision key and mirrored into the thread.
    #[tokio::test]
    async fn escalate_with_a_packet_stores_and_mirrors_it() {
        let mut r = rig().await;
        let t = r.task("agent:lead").await;
        let ask = packet::Ask {
            summary: Some("## Why\n\nThe retry loop doubles cost.".into()),
            options: vec![
                packet::PacketOption {
                    label: "keep".into(),
                    description: None,
                },
                packet::PacketOption {
                    label: "cap at 3".into(),
                    description: Some("cheaper, rare misses".into()),
                },
            ],
            recommended: Some(1),
            pointers: vec![code_pointer()],
            ..packet::Ask::new("retry policy?")
        };
        let e = r
            .ledger
            .escalate(&t.id, ask.clone(), "agent:lead", None)
            .await
            .unwrap_err();
        assert!(e.contains("needs a `board`"), "{e}");
        board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let ask = packet::Ask {
            board: Some("b1".into()),
            ..ask
        };
        let (decision, _) = r
            .ledger
            .escalate(&t.id, ask, "agent:lead", None)
            .await
            .unwrap();
        let p = r.ledger.packet(&decision).await.unwrap().expect("stored");
        assert_eq!(
            (p.kind, p.task.clone()),
            (packet::PacketKind::Decision, t.id.clone())
        );
        assert_eq!(p.question.as_deref(), Some("retry policy?"));
        assert_eq!(p.recommended, Some(1));
        assert_eq!(p.pointers, vec![code_pointer()]);
        assert_eq!(p.by, "agent:lead");
        assert_eq!(p.board.as_deref(), Some("b1"));
        let doc = t.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap()[&doc].clone();
        let block = blocks
            .iter()
            .find(|b| b.contains("**decision requested**"))
            .expect("thread block");
        assert!(block.contains("The retry loop doubles cost."), "{block}");
        assert!(
            block.contains("- keep\n- cap at 3 (recommended) — cheaper, rare misses"),
            "{block}"
        );
        assert!(
            block.contains("Look at:\n- the retry (`src/x.rs#L12-L14`) — three attempts"),
            "{block}"
        );
        assert!(block.contains("Manager: answer with `decide"), "{block}");
        assert!(
            block.contains("\n\nBoard: theuth://notes/b1\n\n"),
            "{block}"
        );
        assert_eq!(
            r.ledger
                .packets_for(std::slice::from_ref(&t.id))
                .await
                .unwrap(),
            vec![p]
        );
        assert_eq!(r.ledger.packet("decision:nope").await.unwrap(), None);
        // the answer path is unchanged
        r.ledger
            .answer_decision(&decision, "cap at 3", "user", None)
            .await
            .unwrap();
        assert!(r.ledger.get(&t.id).await.unwrap().blockers.is_empty());
    }

    /// 101 D1: an agent audience needs no packet; one is stored when given.
    #[tokio::test]
    async fn escalate_to_an_agent_keeps_the_packet_optional() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let (d1, m) = r
            .ledger
            .escalate(&t.id, packet::Ask::new("ship?"), "agent:ops", None)
            .await
            .unwrap();
        assert_eq!(m, Owner::Agent("lead".into()));
        assert_eq!(r.ledger.packet(&d1).await.unwrap(), None);
        let ask = packet::Ask {
            summary: Some("because".into()),
            ..packet::Ask::new("ship?")
        };
        let (d2, _) = r
            .ledger
            .escalate(&t.id, ask, "agent:ops", None)
            .await
            .unwrap();
        assert!(r.ledger.packet(&d2).await.unwrap().is_some());
    }

    /// 101 §3 caps surface from `escalate`; nothing is written on refusal.
    #[tokio::test]
    async fn escalate_refuses_over_cap_packets_before_writing() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let ask = packet::Ask {
            summary: Some("x".repeat(packet::SUMMARY_MAX + 1)),
            ..packet::Ask::new("?")
        };
        let e = r
            .ledger
            .escalate(&t.id, ask, "agent:ops", None)
            .await
            .unwrap_err();
        assert!(e.contains("`summary`") && e.contains("16 KiB"), "{e}");
        assert!(r.ledger.get(&t.id).await.unwrap().blockers.is_empty());
    }

    fn desk_ask() -> packet::Ask {
        packet::Ask {
            summary: Some("Approve the autopay PR.".into()),
            board: Some("b1".into()),
            options: vec![
                packet::PacketOption {
                    label: "approve".into(),
                    description: Some("ships today".into()),
                },
                packet::PacketOption {
                    label: "hold".into(),
                    description: None,
                },
            ],
            recommended: Some(1),
            pointers: vec![code_pointer()],
            items: vec![packet::Item {
                id: "a-rec".into(),
                title: "A record".into(),
                description: None,
                options: vec![packet::PacketOption {
                    label: "add".into(),
                    description: None,
                }],
                recommended: Some(0),
            }],
            ..packet::Ask::new("approve PR 42?")
        }
    }

    fn backs(option: Option<u32>) -> packet::Recommendation {
        packet::Recommendation {
            text: "Low risk; CI is green.".into(),
            option,
        }
    }

    /// 103 F1: the desk escalates with a full packet to its manager (lead); lead
    /// forwards with an empty question and a recommendation. The new packet
    /// inherits question, summary, board, options, recommended, pointers and
    /// items; the inherited board satisfies D1 for the user; the thread block's
    /// second line names the parent; the parent decision is untouched.
    #[tokio::test]
    async fn escalate_forwards_the_parent_packet_with_a_recommendation() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let (d1, m) = r
            .ledger
            .escalate(&t.id, desk_ask(), "agent:ops", None)
            .await
            .unwrap();
        assert_eq!(m, Owner::Agent("lead".into()));
        r.tick(1000);
        let (d2, m) = r
            .ledger
            .escalate(
                &t.id,
                packet::Ask {
                    recommendation: Some(backs(Some(0))),
                    ..packet::Ask::new("")
                },
                "agent:lead",
                Some("s-lead"),
            )
            .await
            .unwrap();
        assert_eq!(m, Owner::User);
        assert_ne!(d1, d2);
        let parent = r.ledger.packet(&d1).await.unwrap().unwrap();
        let p = r.ledger.packet(&d2).await.unwrap().unwrap();
        assert_eq!(p.by, "agent:lead");
        assert_eq!(p.to.as_deref(), Some("user"), "106: addressed to the user");
        assert_eq!(parent.to.as_deref(), Some("agent:lead"));
        assert_eq!(p.question.as_deref(), Some("approve PR 42?"));
        assert_eq!(p.summary, "Approve the autopay PR.");
        assert_eq!(p.board.as_deref(), Some("b1"));
        assert_eq!(p.options, parent.options);
        assert_eq!(p.recommended, Some(1));
        assert_eq!(p.pointers, vec![code_pointer()]);
        assert_eq!(p.items, parent.items);
        assert_eq!(p.recommendation, Some(backs(Some(0))));
        assert_eq!(parent.recommendation, None, "the parent is untouched");
        let doc = t.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap()[&doc].clone();
        let block = blocks
            .iter()
            .find(|b| b.contains(&format!("**decision requested** (`{d2}`)")))
            .expect("thread block");
        assert!(
            block.contains(&format!(
                "**decision requested** (`{d2}`)\nforwarded from {d1} by agent:lead\n\nRecommendation: approve — Low risk; CI is green.\n\napprove PR 42?\n\nApprove the autopay PR.\n\nBoard: theuth://notes/b1\n\nOptions:\n- approve — ships today\n- hold (recommended)\n"
            )),
            "{block}"
        );
        assert!(
            block.contains("Items:\n1. A record: add (recommended)\n"),
            "{block}"
        );
        // the parent's block has no forwarded line
        assert!(
            !blocks
                .iter()
                .find(|b| b.contains(&format!("(`{d1}`)")))
                .unwrap()
                .contains("forwarded from")
        );
        let b = r.ledger.get(&t.id).await.unwrap();
        assert_eq!(b.blockers, vec![d1.clone(), d2.clone()]);
        // 106 F-3: the user answers the forward → lead (the forwarder, not the
        // owner) is woken `answer`; d1 still blocks, so no owner wake.
        for d in r.ledger.dispatch().await.unwrap() {
            r.ledger.finish_wake(&d.wake).await.unwrap();
        }
        assert!(r.ledger.wakes().await.unwrap().is_empty());
        r.ledger
            .answer_decision(&d2, "approve", "user", None)
            .await
            .unwrap();
        assert_eq!(
            r.ledger.get(&t.id).await.unwrap().blockers,
            vec![d1.clone()]
        );
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!(
            w.iter()
                .map(|w| (w.agent.as_str(), w.reason))
                .collect::<Vec<_>>(),
            vec![("lead", WakeReason::Answer)],
            "{w:?}"
        );
        for d in r.ledger.dispatch().await.unwrap() {
            r.ledger.finish_wake(&d.wake).await.unwrap();
        }
        // lead answers the parent with `decide`: the owner is woken as today, lead is not
        r.ledger
            .answer_decision(&d1, "approve", "agent:lead", Some("s-lead"))
            .await
            .unwrap();
        assert!(r.ledger.get(&t.id).await.unwrap().blockers.is_empty());
        let w = r.ledger.wakes().await.unwrap();
        assert!(
            w.iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Answer),
            "{w:?}"
        );
        assert!(w.iter().all(|w| w.agent != "lead"), "{w:?}");
    }

    /// 103 F1: a manager that passes its own fields keeps them; only the empty
    /// ones are inherited (own options drop the parent's `recommended`).
    #[tokio::test]
    async fn escalate_forward_keeps_the_callers_own_fields() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        board(&r.thread, "b1", &packet::DECISION_SECTIONS);
        let (d1, _) = r
            .ledger
            .escalate(&t.id, desk_ask(), "agent:ops", None)
            .await
            .unwrap();
        let (d2, _) = r
            .ledger
            .escalate(
                &t.id,
                packet::Ask {
                    summary: Some("Lead's abstract.".into()),
                    options: vec![packet::PacketOption {
                        label: "ship".into(),
                        description: None,
                    }],
                    recommendation: Some(backs(None)),
                    ..packet::Ask::new("ship it?")
                },
                "agent:lead",
                None,
            )
            .await
            .unwrap();
        let p = r.ledger.packet(&d2).await.unwrap().unwrap();
        assert_eq!(p.question.as_deref(), Some("ship it?"));
        assert_eq!(p.summary, "Lead's abstract.");
        assert_eq!(p.options.len(), 1);
        assert_eq!(p.recommended, None);
        assert_eq!(p.board.as_deref(), Some("b1"), "inherited");
        assert_eq!(p.pointers, vec![code_pointer()], "inherited");
        assert_eq!(p.items.len(), 1, "inherited");
        let doc = t.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap()[&doc].clone();
        let block = blocks
            .iter()
            .find(|b| b.contains(&format!("(`{d2}`)")))
            .unwrap();
        assert!(block.contains(&format!("\nforwarded from {d1} by agent:lead\n\nRecommendation: Low risk; CI is green.\n\nship it?")), "{block}");
    }

    /// 103 F1: no parent (no decision blocker, or only the caller's own) → plain
    /// behaviour: the empty question stays empty, D1 refuses without a board.
    #[tokio::test]
    async fn escalate_without_a_parent_inherits_nothing() {
        let mut r = rig().await;
        let t = r.task("agent:lead").await;
        let e = r
            .ledger
            .escalate(
                &t.id,
                packet::Ask {
                    recommendation: Some(backs(None)),
                    ..packet::Ask::new("")
                },
                "agent:lead",
                None,
            )
            .await
            .unwrap_err();
        assert!(e.contains("needs a `board`"), "{e}");
        assert!(r.ledger.get(&t.id).await.unwrap().blockers.is_empty());
        // ops' own earlier decision is not a parent for ops
        let o = r.task("agent:ops").await;
        let (d1, _) = r
            .ledger
            .escalate(&o.id, desk_ask(), "agent:ops", None)
            .await
            .unwrap();
        let (d2, _) = r
            .ledger
            .escalate(&o.id, packet::Ask::new(""), "agent:ops", None)
            .await
            .unwrap();
        let p = r.ledger.packet(&d2).await.unwrap();
        assert_eq!(p, None, "no packet without a parent or a summary");
        let doc = o.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap()[&doc].clone();
        let block = blocks
            .iter()
            .find(|b| b.contains(&format!("(`{d2}`)")))
            .unwrap();
        assert!(!block.contains("forwarded from"), "{block}");
        assert!(!block.contains(&d1), "{block}");
    }

    /// 103 F3: a bundle round-trips through the store and reads back from `packets_for`.
    #[tokio::test]
    async fn escalate_stores_items_round_trip() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let items: Vec<packet::Item> = (0..3)
            .map(|i| packet::Item {
                id: format!("rec-{i}"),
                title: format!("Record {i}"),
                description: (i == 0).then(|| "the apex".to_owned()),
                options: vec![
                    packet::PacketOption {
                        label: "add".into(),
                        description: None,
                    },
                    packet::PacketOption {
                        label: "skip".into(),
                        description: Some("later".into()),
                    },
                ],
                recommended: (i < 2).then_some(0),
            })
            .collect();
        let (d, _) = r
            .ledger
            .escalate(
                &t.id,
                packet::Ask {
                    items: items.clone(),
                    ..packet::Ask::new("DNS records?")
                },
                "agent:ops",
                None,
            )
            .await
            .unwrap();
        let p = r
            .ledger
            .packet(&d)
            .await
            .unwrap()
            .expect("stored for items alone");
        assert_eq!(p.items, items);
        assert_eq!(
            r.ledger
                .packets_for(std::slice::from_ref(&t.id))
                .await
                .unwrap()[0]
                .items,
            items
        );
        let doc = t.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap()[&doc].clone();
        let block = blocks.iter().find(|b| b.contains("Items:")).unwrap();
        assert!(
            block.contains("Items:\n1. Record 0 — the apex: add (recommended) | skip\n2. Record 1: add (recommended) | skip\n3. Record 2: add | skip\n"),
            "{block}"
        );
        // the answer is plain text through `answer_decision`
        r.ledger
            .answer_decision(
                &d,
                "1. Record 0: add\n2. Record 1: add\n3. Record 2: skip",
                "agent:lead",
                None,
            )
            .await
            .unwrap();
        assert!(r.ledger.get(&t.id).await.unwrap().blockers.is_empty());
    }

    /// 101 D3: `review_packet` replaces the row and writes `**review requested**`;
    /// `effective_reviewer` follows `update`'s resolution.
    #[tokio::test]
    async fn review_packet_replaces_and_mirrors() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        assert_eq!(
            r.ledger.effective_reviewer(&t, None),
            Owner::Agent("lead".into()),
            "nobody named → the manager"
        );
        assert_eq!(
            r.ledger.effective_reviewer(&t, Some(&Owner::User)),
            Owner::User
        );
        assert_eq!(
            r.ledger.effective_reviewer(&t, Some(&t.owner)),
            Owner::Agent("lead".into()),
            "self without done_when → the manager"
        );
        let lead = r.task("agent:lead").await;
        assert_eq!(r.ledger.effective_reviewer(&lead, None), Owner::User);
        let mine = r.task("user").await;
        assert_eq!(r.ledger.effective_reviewer(&mine, None), Owner::User);

        let p1 = r
            .ledger
            .review_packet(
                &t.id,
                "first".into(),
                None,
                vec![code_pointer()],
                vec![],
                "agent:ops",
            )
            .await
            .unwrap();
        assert_eq!(p1.key, format!("review:{}", t.id));
        r.tick(10);
        let checks = vec![packet::Check {
            text: "tests".into(),
            done: true,
            note: None,
        }];
        let p2 = r
            .ledger
            .review_packet(
                &t.id,
                "second".into(),
                Some("rb1".into()),
                vec![code_pointer()],
                checks,
                "agent:ops",
            )
            .await
            .unwrap();
        let all = r
            .ledger
            .packets_for(std::slice::from_ref(&t.id))
            .await
            .unwrap();
        assert_eq!(all, vec![p2.clone()], "one row, the latest");
        assert_eq!(p2.board.as_deref(), Some("rb1"));
        assert_eq!(r.ledger.packet(&p1.key).await.unwrap(), Some(p2));
        let doc = t.thread.clone().unwrap();
        let blocks = r.thread.0.lock().unwrap()[&doc].clone();
        assert_eq!(
            blocks
                .iter()
                .filter(|b| b.contains("**review requested**"))
                .count(),
            2
        );
        assert!(blocks.last().unwrap().contains("- [x] tests"));
        let e = r
            .ledger
            .review_packet(
                &t.id,
                "x".into(),
                None,
                vec![],
                (0..13)
                    .map(|_| packet::Check {
                        text: "c".into(),
                        done: false,
                        note: None,
                    })
                    .collect(),
                "agent:ops",
            )
            .await;
        assert!(e.is_ok(), "checks are uncapped");
    }

    #[tokio::test]
    async fn mention_comments_and_wakes_owner() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger.mention(&t.id, "any news?", "user").await.unwrap();
        let doc = t.thread.clone().unwrap();
        assert!(
            r.thread.0.lock().unwrap()[&doc]
                .iter()
                .any(|b| b.starts_with("**user**") && b.ends_with("any news?"))
        );
        assert!(
            r.ledger
                .wakes()
                .await
                .unwrap()
                .iter()
                .any(|w| w.agent == "ops" && w.reason == WakeReason::Mention)
        );
    }

    #[tokio::test]
    async fn mark_answered_dedups() {
        let r = rig().await;
        assert!(
            r.ledger
                .store
                .mark_answered("+15550100", 17, 1)
                .await
                .unwrap()
        );
        assert!(
            !r.ledger
                .store
                .mark_answered("+15550100", 17, 2)
                .await
                .unwrap()
        );
        assert!(
            r.ledger
                .store
                .mark_answered("+15550100", 18, 3)
                .await
                .unwrap()
        );
    }
}
