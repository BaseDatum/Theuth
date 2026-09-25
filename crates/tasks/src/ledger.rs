//! The rules (67 §7.2–7.5) over [`Store`]: transitions, checkout and
//! epochs, the event log and thread, wakes (coalesce, dispatch, back-off),
//! the cross-task write cap, the orphan sweep, disposition enforcement.
//! No actor, no runtime: the actor in `lib.rs` delegates here and tests
//! drive it with a mock clock, router and thread.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::config::TasksConfig;
use crate::store::Store;
use crate::thread::Thread;
use crate::types::{
    Checkout, Owner, Task, TaskCreate, TaskEvent, TaskFilter, TaskId, TaskPatch, TaskState, Wake,
    WakeReason,
};
use crate::DeskRouter;

/// ms since the epoch, injectable for tests.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Blocker text of a task that its episode closed without a disposition (67 §7.3).
pub const NO_CONTINUATION: &str = "no continuation";

/// Placeholder `claimed_by` between claim and a successful delivery when the
/// agent has no desk yet; replaced by the desk id once delivery returns.
pub const DISPATCHING: &str = "dispatching";

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
    /// `updated_ms` of a task when its wake was claimed: the no-progress
    /// comparison point (67 §7.3). Local: a hub move restarts the count.
    delivered: HashMap<TaskId, u64>,
    /// Delivery failures per wake id: `(attempts, not before ms)`.
    retry: HashMap<String, (u32, u64)>,
    /// Engine → first sweep that saw it unreachable (ms).
    unreachable_since: HashMap<String, u64>,
}

type R<T> = Result<T, String>;

fn db(e: anyhow::Error) -> String {
    format!("tasks store: {e:#}")
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
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
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
    pub fn new(store: Store, cfg: TasksConfig, thread: Arc<dyn Thread>, clock: Clock, engine: String) -> Self {
        Self {
            store,
            cfg,
            router: None,
            thread,
            clock,
            engine,
            delivered: HashMap::new(),
            retry: HashMap::new(),
            unreachable_since: HashMap::new(),
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

    async fn event(&self, t: &Task, actor: &str, session: Option<&str>, field: &str, before: Option<String>, after: Option<String>) {
        let e = TaskEvent {
            at_ms: self.now(),
            actor: actor.to_owned(),
            session: session.map(str::to_owned),
            field: field.to_owned(),
            before,
            after,
        };
        if let Err(err) = self.store.add_event(&t.id, &e).await {
            tracing::error!(task = %t.id, "event not recorded: {err:#}");
        }
        if let Some(doc) = &t.thread {
            let line = match (&e.before, &e.after) {
                (Some(b), Some(a)) => format!("- {} **{actor}** {field}: {b} → {a}", stamp(e.at_ms)),
                (None, Some(a)) => format!("- {} **{actor}** {field}: {a}", stamp(e.at_ms)),
                (Some(b), None) => format!("- {} **{actor}** {field}: {b} → ∅", stamp(e.at_ms)),
                (None, None) => format!("- {} **{actor}** {field}", stamp(e.at_ms)),
            };
            self.thread.append(doc, &line).await;
        }
    }

    async fn say(&self, t: &Task, actor: &str, text: &str) {
        if let Some(doc) = &t.thread {
            self.thread
                .append(doc, &format!("**{actor}** ({}): {text}", stamp(self.now())))
                .await;
        }
    }

    /// Raise a wake for `who` (nothing for the user or the harness).
    pub async fn wake(&self, who: &Owner, task: Option<&TaskId>, reason: WakeReason) {
        if let Some(agent) = agent_name(who)
            && let Err(e) = self.store.raise(agent, task, reason, self.now()).await
        {
            tracing::error!(agent, "wake not raised: {e:#}");
        }
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
        self.store.get(id).await.map_err(db)?.ok_or_else(|| "no such task".to_owned())
    }

    async fn save(&self, t: &Task) -> R<()> {
        self.store.put(t).await.map_err(db)
    }

    /// The cross-task write cap (67 §7.3): a session writing to a task it
    /// does not own (neither its checkout nor its agent's) spends one of
    /// `cross_task_write_cap` distinct tasks.
    async fn charge(&self, session: Option<&str>, t: &Task) -> R<()> {
        let Some(session) = session else { return Ok(()) };
        let mine = t.checkout.as_ref().is_some_and(|c| c.session == session)
            || matches!(
                (self.agent_for(session), agent_name(&t.owner)),
                (Some(a), Some(o)) if a == o
            );
        if mine || self.store.budget_write(session, &t.id, self.cfg.cross_task_write_cap).await.map_err(db)? {
            return Ok(());
        }
        Err(format!(
            "tasks.cross_task_write_cap: this session has written to {} tasks it does not own; \
             delegate with task_create instead",
            self.cfg.cross_task_write_cap
        ))
    }

    /// 67 §7.5: a session that once held the checkout but no longer does was
    /// superseded — its cue to stop.
    async fn check_epoch(&self, session: Option<&str>, t: &Task) -> R<()> {
        let (Some(session), Some(c)) = (session, &t.checkout) else { return Ok(()) };
        if c.session == session {
            return Ok(());
        }
        let held = self
            .store
            .events(&t.id)
            .await
            .map_err(db)?
            .iter()
            .any(|e| e.field == "checkout" && e.after.as_deref().is_some_and(|a| a.starts_with(session)));
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
    /// when none), project to the parent's; a thread document; `wake` to the owner.
    pub async fn create(&self, input: TaskCreate, created_by: &str, session: Option<&str>, wake: WakeReason) -> R<Task> {
        let now = self.now();
        let parent = match &input.parent {
            Some(p) => Some(self.load(p).await.map_err(|_| format!("parent {p}: no such task"))?),
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
            state: TaskState::Todo,
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
            id,
        };
        t.thread = self
            .thread
            .create(&format!("Task {}: {}", short(&t.id), t.title), &t.brief)
            .await;
        self.save(&t).await?;
        self.event(&t, created_by, session, "created", None, Some(format!("owner {}", t.owner))).await;
        self.wake(&t.owner, Some(&t.id), wake).await;
        Ok(t)
    }

    /// Create the task of a schedule fire, or re-raise on the one that exists (67 §7.4).
    pub async fn create_for_schedule(&self, schedule: &str, fire_ms: u64, agent: &str, prompt: &str, project: Option<String>) -> R<Task> {
        if let Some(t) = self.store.by_schedule(schedule, fire_ms).await.map_err(db)? {
            let w = self
                .store
                .raise(agent, Some(&t.id), WakeReason::Schedule, self.now())
                .await
                .map_err(db)?;
            self.say(&t, "system", &format!("schedule `{schedule}` fired again ({}×)", w.count)).await;
            return Ok(t);
        }
        let input = TaskCreate {
            title: format!("{schedule} @ {}", stamp(fire_ms)),
            brief: prompt.to_owned(),
            owner: Owner::Agent(agent.to_owned()),
            reviewer: None,
            parent: None,
            project,
            where_: None,
            harness: None,
            external: vec![],
        };
        let mut t = self.create(input, "system", None, WakeReason::Schedule).await?;
        t.schedule = Some((schedule.to_owned(), fire_ms));
        self.save(&t).await?;
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
            self.event(&t, "system", None, "state", Some(before.as_str().into()), Some("done".into())).await;
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
            let Some(t) = self.store.get(&cur).await.map_err(db)? else { continue };
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
        self.event(t, actor, None, "state", Some(before.as_str().into()), Some(to.as_str().into())).await;
        self.wake(&t.owner, Some(&t.id), reason).await;
        Ok(())
    }

    /// `t` reached `done`/`cancelled`: drop it from every blockers list; a
    /// task left with none returns to its previous state and its owner is
    /// woken with `resume`.
    async fn on_closed(&self, t: &Task) -> R<()> {
        for mut b in self.store.blocked_on(&t.id.0).await.map_err(db)? {
            let before = b.blockers.join(", ");
            b.blockers.retain(|x| *x != t.id.0);
            b.updated_ms = self.now();
            self.save(&b).await?;
            self.event(&b, "system", None, "blockers", Some(before), Some(b.blockers.join(", "))).await;
            if b.state == TaskState::Blocked && b.blockers.is_empty() {
                self.unblock(&mut b, "system", WakeReason::Resume).await?;
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
            (Todo, Doing) => Err("todo → doing is task_checkout".into()),
            (Review, Doing) => {
                let r = t.reviewer.clone().unwrap_or(Owner::User);
                if acts_for(actor, &r) { Ok(()) } else { Err(format!("send back is the reviewer's ({r})")) }
            }
            (Blocked, Doing | Todo) => {
                if !blockers.is_empty() {
                    Err(format!("still blocked on {}", blockers.join(", ")))
                } else if acts_for(actor, &t.owner) {
                    Ok(())
                } else {
                    Err(format!("only the owner ({}) resumes a blocked task", t.owner))
                }
            }
            (Doing, Review) => {
                if acts_for(actor, &t.owner) { Ok(()) } else { Err(format!("review is the owner's ({}) move", t.owner)) }
            }
            (Doing | Todo, Blocked) => {
                if blockers.is_empty() { Err("blocked needs a blockers entry".into()) } else { Ok(()) }
            }
            (Review, Done) => {
                let r = t.reviewer.clone().unwrap_or(Owner::User);
                if r == t.owner && t.owner != Owner::User {
                    return Err("done needs a reviewer other than the owner".into());
                }
                if acts_for(actor, &r) { Ok(()) } else { Err(format!("done is the reviewer's ({r}) move")) }
            }
            _ => Err(format!("{} → {} is not a transition", from.as_str(), to.as_str())),
        }
    }

    /// Apply `patch` (67 §7.2 guards), log every changed field, wake whoever
    /// the change concerns.
    pub async fn update(&mut self, id: &TaskId, patch: TaskPatch, actor: &str, session: Option<&str>) -> R<Task> {
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

        let mut changes: Vec<(&str, Option<String>, Option<String>)> = vec![];
        macro_rules! set {
            ($field:ident, $name:literal) => {
                if let Some(v) = patch.$field
                    && Some(&v) != t.$field.as_ref()
                {
                    changes.push(($name, t.$field.as_ref().map(|x| format!("{x}")), Some(format!("{v}"))));
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
        if let Some(to) = patch.state
            && to != t.state
        {
            changes.push(("state", Some(t.state.as_str().into()), Some(to.as_str().into())));
            t.state = to;
            if to == TaskState::Review || to == TaskState::Cancelled {
                t.checkout = None;
            }
        }

        // 67 §7.3 back-off: the owner's write after a wake is that wake's completion.
        let mut completed = false;
        if let Some(agent) = agent_name(&before.owner).map(str::to_owned)
            && actor == before.owner.to_string()
            && let Some(at) = self.delivered.remove(id)
        {
            self.complete_wake(&mut t, &agent, at == before.updated_ms && changes.is_empty()).await;
            completed = true;
        }

        if changes.is_empty() {
            if completed {
                self.save(&t).await?;
            }
            return self.with_blocks(t).await;
        }
        t.updated_ms = self.now();
        self.save(&t).await?;
        for (field, b, a) in changes {
            self.event(&t, actor, session, field, b, a).await;
        }

        if t.owner != before.owner {
            self.wake(&t.owner, Some(&t.id), WakeReason::Assigned).await;
        }
        if t.state != before.state {
            match t.state {
                TaskState::Review => {
                    if let Some(r) = &t.reviewer {
                        self.wake(r, Some(&t.id), WakeReason::Review).await;
                    }
                }
                TaskState::Doing if before.state == TaskState::Review => {
                    self.wake(&t.owner, Some(&t.id), WakeReason::Comment).await;
                }
                TaskState::Done | TaskState::Cancelled => self.on_closed(&t).await?,
                _ => {}
            }
        } else if t.state == TaskState::Blocked && t.blockers.is_empty() && !before.blockers.is_empty() {
            // Blockers cleared by hand (a decision answered): back to work.
            self.unblock(&mut t, actor, WakeReason::Answer).await?;
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
                return Err(format!("conflict: checked out by {} on {} (epoch {})", c.session, c.engine, c.epoch));
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
        };
        let before = t.state;
        t.checkout = Some(c.clone());
        t.state = TaskState::Doing;
        t.updated_ms = now;
        self.save(&t).await?;
        self.event(&t, actor, Some(session), "checkout", prev.as_ref().map(checkout_text), Some(checkout_text(&c))).await;
        if before != TaskState::Doing {
            self.event(&t, actor, Some(session), "state", Some(before.as_str().into()), Some("doing".into())).await;
        }
        self.with_blocks(t).await
    }

    /// `task_comment` / `task_note`: a block in the thread. A comment wakes
    /// the owner (and the reviewer under review) unless they wrote it, plus
    /// every agent named in it; a human comment clears the back-off. A
    /// progress note bumps `checkout.progress_at` and wakes nobody.
    pub async fn comment(&self, id: &TaskId, text: &str, progress: bool, actor: &str, session: Option<&str>) -> R<()> {
        let mut t = self.load(id).await?;
        self.charge(session, &t).await?;
        self.check_epoch(session, &t).await?;
        let now = self.now();
        let mut dirty = false;
        if progress {
            self.say(&t, actor, &format!("_progress note_\n\n{text}")).await;
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
        let first = text.lines().next().unwrap_or("").chars().take(80).collect::<String>();
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
            self.wake(&t.owner, Some(&t.id), WakeReason::Comment).await;
        }
        if t.state == TaskState::Review
            && let Some(r) = &t.reviewer
            && actor != r.to_string()
            && let Some(a) = agent_name(r)
            && woken.insert(a.to_owned())
        {
            self.wake(r, Some(&t.id), WakeReason::Comment).await;
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
                    self.wake(&Owner::Agent(a), Some(&t.id), WakeReason::Mention).await;
                }
            }
        }
        Ok(())
    }

    /// `task_place`: record theuth's answer (67 §8) and its reason.
    pub async fn place(&mut self, id: &TaskId, where_: Option<String>, harness: Option<String>, reason: &str, actor: &str, session: Option<&str>) -> R<Task> {
        let patch = TaskPatch { where_, harness, ..Default::default() };
        let t = self.update(id, patch, actor, session).await?;
        self.say(&t, actor, &format!("placed on `{}` / `{}`: {reason}", t.where_.as_deref().unwrap_or("?"), t.harness.as_deref().unwrap_or("?"))).await;
        Ok(t)
    }

    /// A chat message into a task's thread (67 §4.6): a comment by `actor`
    /// and a `mention` wake for the owner.
    pub async fn mention(&self, id: &TaskId, text: &str, actor: &str) -> R<()> {
        let t = self.load(id).await?;
        self.say(&t, actor, text).await;
        self.wake(&t.owner, Some(id), WakeReason::Mention).await;
        Ok(())
    }

    /// `escalate` (67 §11): the question in the thread, `blocked: decision:<id>`,
    /// the **caller's** manager woken (`answer`). Returns the decision and the
    /// manager; `Owner::User` means the actor handler asks the user. The user
    /// has nobody to escalate to.
    pub async fn escalate(&mut self, id: &TaskId, question: &str, options: &[String], actor: &str, session: Option<&str>) -> R<(String, Owner)> {
        let manager = match actor.strip_prefix("agent:") {
            Some(a) => self.manager_of(a),
            None => return Err("you are the top of the line; decide it".into()),
        };
        let t = self.load(id).await?;
        let decision = format!("decision:{}", uuid::Uuid::now_v7());
        let mut text = format!("**decision requested** (`{decision}`)\n\n{question}");
        if !options.is_empty() {
            text.push_str("\n\nOptions:\n");
            for o in options {
                text.push_str(&format!("- {o}\n"));
            }
        }
        text.push_str(&format!(
            "\n\nManager: answer with `decide {{ task, decision: \"{decision}\", answer }}`, or `escalate` the same task further."
        ));
        self.say(&t, actor, &text).await;
        let mut blockers = t.blockers.clone();
        blockers.push(decision.clone());
        let state = if matches!(t.state, TaskState::Doing | TaskState::Todo) { Some(TaskState::Blocked) } else { None };
        self.update(id, TaskPatch { blockers: Some(blockers), state, ..Default::default() }, actor, session).await?;
        self.wake(&manager, Some(id), WakeReason::Answer).await;
        Ok((decision, manager))
    }

    /// `decide` (67 §11): the answer in the thread, the `decision:<id>` blocker
    /// dropped — the last blocker going re-wakes the owner (`answer`).
    pub async fn answer_decision(&mut self, decision: &str, answer: &str, actor: &str, session: Option<&str>) -> R<()> {
        let t = self
            .store
            .blocked_on(decision)
            .await
            .map_err(db)?
            .into_iter()
            .next()
            .ok_or_else(|| format!("no task is blocked on {decision}"))?;
        self.say(&t, actor, &format!("**decision** (`{decision}`): {answer}")).await;
        let blockers = t.blockers.iter().filter(|b| *b != decision).cloned().collect();
        self.update(&t.id, TaskPatch { blockers: Some(blockers), ..Default::default() }, actor, session).await?;
        Ok(())
    }

    // ---- wakes: dispatch, completion, back-off -------------------------------------

    /// One dispatch pass (67 §7.3): per agent with nothing claimed, the
    /// oldest unclaimed wake whose task is not in back-off is claimed by the
    /// agent's desk and returned for delivery.
    pub async fn dispatch(&mut self) -> R<Vec<Delivery>> {
        let now = self.now();
        let wakes = self.store.wakes().await.map_err(db)?;
        let busy: HashSet<&str> = wakes.iter().filter(|w| w.claimed_by.is_some()).map(|w| w.agent.as_str()).collect();
        let mut picked: HashMap<String, Delivery> = HashMap::new();
        for w in &wakes {
            if w.claimed_by.is_some() || busy.contains(w.agent.as_str()) || picked.contains_key(&w.agent) {
                continue;
            }
            if self.retry.get(&w.id).is_some_and(|(_, not_before)| *not_before > now) {
                continue;
            }
            let task = match &w.task {
                Some(id) => self.store.get(id).await.map_err(db)?,
                None => None,
            };
            if task.as_ref().and_then(|t| t.next_wake_after_ms).is_some_and(|after| after > now) {
                continue;
            }
            picked.insert(w.agent.clone(), Delivery { wake: w.clone(), task });
        }
        let mut out = Vec::with_capacity(picked.len());
        for (_, mut d) in picked {
            let desk = self
                .router
                .as_ref()
                .and_then(|r| r.desk_for(&d.wake.agent))
                .unwrap_or_else(|| DISPATCHING.to_owned());
            self.store.claim(&d.wake.id, Some(&desk), now).await.map_err(db)?;
            d.wake.claimed_by = Some(desk);
            if let Some(t) = &d.task {
                self.delivered.insert(t.id.clone(), t.updated_ms);
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
            self.store.claim(&wake.id, Some(&desk), self.now()).await.map_err(db)?;
        }
        Ok(())
    }

    /// Delivery failed: unclaim; retry after 5 s doubling to 5 min.
    pub async fn delivery_failed(&mut self, wake: &Wake) -> R<()> {
        let now = self.now();
        let e = self.retry.entry(wake.id.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 = now + (5_000u64 << (e.0 - 1).min(6)).min(300_000);
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

    /// Forget the delivery snapshot of `id` (a wake completed without a task write).
    pub fn forget_delivery(&mut self, id: &TaskId) {
        self.delivered.remove(id);
    }

    // ---- orphan sweep, disposition enforcement -----------------------------------------

    /// 67 §7.5 every 30 s. `reachable`: the connected engines (self included)
    /// when the mesh is up; `None` = staleness only. Orphans: the checkout is
    /// kept, marked, `epoch + 1`; the owner is woken with `resume`. Wakes
    /// claimed by a desk that is no longer the agent's are un-claimed.
    pub async fn sweep(&mut self, reachable: Option<&HashSet<String>>) -> R<Vec<TaskId>> {
        let now = self.now();
        if let Some(r) = reachable {
            self.unreachable_since.retain(|e, _| !r.contains(e));
        }
        let doing = self
            .store
            .list(&TaskFilter { state: Some(vec![TaskState::Doing]), ..Default::default() })
            .await
            .map_err(db)?;
        let mut orphaned = vec![];
        for mut t in doing {
            let Some(c) = t.checkout.clone() else { continue };
            if c.orphaned {
                continue;
            }
            let lost = match reachable {
                Some(r) if !r.contains(&c.engine) => {
                    let since = *self.unreachable_since.entry(c.engine.clone()).or_insert(now);
                    now.saturating_sub(since) >= self.cfg.orphan_secs * 1000
                }
                _ => now.saturating_sub(c.progress_at_ms) >= self.cfg.stale_secs * 1000,
            };
            if !lost {
                continue;
            }
            let mut n = c.clone();
            n.orphaned = true;
            n.epoch += 1;
            t.checkout = Some(n.clone());
            t.updated_ms = now;
            self.save(&t).await?;
            self.event(&t, "system", None, "checkout", Some(checkout_text(&c)), Some(checkout_text(&n))).await;
            self.say(&t, "system", &format!("session {} on {} is presumed lost; resume from the last progress note", c.session, c.engine)).await;
            self.wake(&t.owner, Some(&t.id), WakeReason::Resume).await;
            orphaned.push(t.id);
        }
        if let Some(router) = &self.router {
            for w in self.store.wakes().await.map_err(db)? {
                let Some(by) = &w.claimed_by else { continue };
                if by == DISPATCHING {
                    continue;
                }
                if router.desk_for(&w.agent).is_some_and(|d| d != *by) {
                    self.store.claim(&w.id, None, now).await.map_err(db)?;
                }
            }
        }
        Ok(orphaned)
    }

    /// `EpisodeClosing` (67 §7.3 last rule): every `doing` task checked out
    /// by one of `sessions` with no `continuing` disposition since its
    /// checkout is `blocked: no continuation` and the manager is woken.
    pub async fn episode_closing(&mut self, agent: &str, sessions: &[String]) -> R<Vec<TaskId>> {
        for s in sessions {
            self.store.finish_claimed_by(s).await.map_err(db)?;
        }
        let doing = self
            .store
            .list(&TaskFilter { state: Some(vec![TaskState::Doing]), ..Default::default() })
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
                .is_some_and(|e| e.after.as_deref().is_some_and(|a| a.starts_with("continuing")));
            if continuing {
                continue;
            }
            t.blockers.push(NO_CONTINUATION.to_owned());
            t.state = TaskState::Blocked;
            t.updated_ms = self.now();
            self.save(&t).await?;
            self.event(&t, "system", None, "state", Some("doing".into()), Some("blocked".into())).await;
            self.say(&t, "system", &format!("episode of {agent} closed without a disposition")).await;
            self.wake(&self.manager_of(agent), Some(&t.id), WakeReason::Review).await;
            self.delivered.remove(&t.id);
            blocked.push(t.id);
        }
        Ok(blocked)
    }

    /// Record a disposition that changes no field (`continuing`, `delegated`).
    pub async fn note_disposition(&self, id: &TaskId, text: &str, actor: &str, session: Option<&str>) -> R<()> {
        let t = self.load(id).await?;
        self.event(&t, actor, session, "disposition", None, Some(text.to_owned())).await;
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
        async fn create(&self, title: &str, body: &str) -> Option<String> {
            let id = format!("doc-{}", self.0.lock().unwrap().len() + 1);
            self.0.lock().unwrap().insert(id.clone(), vec![format!("# {title}"), body.to_owned()]);
            Some(id)
        }
        async fn append(&self, doc: &str, markdown: &str) {
            self.0.lock().unwrap().entry(doc.to_owned()).or_default().push(markdown.to_owned());
        }
        async fn read(&self, doc: &str) -> Option<String> {
            self.0.lock().unwrap().get(doc).map(|b| b.join("\n\n"))
        }
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
            self.delivered.lock().unwrap().push((agent.to_owned(), wake.reason));
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

    pub struct Rig {
        pub ledger: Ledger,
        pub clock: Arc<AtomicU64>,
        pub thread: Arc<MemThread>,
        pub router: Arc<MemRouter>,
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
        Rig { ledger, clock, thread, router, _dir: dir }
    }

    impl Rig {
        pub fn tick(&self, ms: u64) {
            self.clock.fetch_add(ms, Ordering::SeqCst);
        }
        pub async fn task(&mut self, owner: &str) -> Task {
            self.ledger
                .create(
                    TaskCreate {
                        title: "fix deploy".into(),
                        brief: "the flaky one".into(),
                        owner: owner.parse().unwrap(),
                        reviewer: None,
                        parent: None,
                        project: None,
                        where_: None,
                        harness: None,
                        external: vec![],
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
                .update(id, TaskPatch { state: Some(s), ..Default::default() }, actor, None)
                .await
        }
        pub async fn blockers(&mut self, id: &TaskId, b: &[&str], actor: &str) -> R<Task> {
            let b = b.iter().map(|s| (*s).to_owned()).collect();
            self.ledger
                .update(id, TaskPatch { blockers: Some(b), ..Default::default() }, actor, None)
                .await
        }
        pub async fn blocked(&mut self, id: &TaskId, on: &str, actor: &str) -> R<Task> {
            self.ledger
                .update(
                    id,
                    TaskPatch { state: Some(TaskState::Blocked), blockers: Some(vec![on.into()]), ..Default::default() },
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
        assert_eq!((wakes[0].agent.as_str(), wakes[0].reason), ("ops", WakeReason::Assigned));

        let u = r.task("user").await;
        assert_eq!(u.reviewer, Some(Owner::User));
        let mut nobody = rig().await;
        nobody.ledger.router = None;
        let t = nobody.task("agent:solo").await;
        assert_eq!(t.reviewer, Some(Owner::User), "no manager → the user reviews");
    }

    #[tokio::test]
    async fn transition_table() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        let id = t.id.clone();
        // todo → doing only by checkout
        assert!(r.set_state(&id, TaskState::Doing, "agent:ops").await.unwrap_err().contains("task_checkout"));
        assert!(r.set_state(&id, TaskState::Review, "agent:ops").await.is_err());
        assert!(r.set_state(&id, TaskState::Done, "user").await.is_err());
        // blocked needs a blocker
        assert!(r.set_state(&id, TaskState::Blocked, "agent:ops").await.unwrap_err().contains("blockers"));
        assert_eq!(r.blocked(&id, "person:sky", "agent:ops").await.unwrap().state, TaskState::Blocked);
        // blocked → doing only when clear; then it lands in todo (no checkout)
        assert!(r.set_state(&id, TaskState::Doing, "agent:ops").await.unwrap_err().contains("still blocked"));
        let t = r.blockers(&id, &[], "agent:ops").await.unwrap();
        assert_eq!(t.state, TaskState::Todo);
        // checkout → doing; owner only
        assert!(r.ledger.checkout(&id, "s-lead", "agent:lead").await.unwrap_err().contains("not the owner"));
        let t = r.ledger.checkout(&id, "s-ops", "agent:ops").await.unwrap();
        assert_eq!(t.state, TaskState::Doing);
        assert_eq!(t.checkout.as_ref().unwrap().epoch, 1);
        // doing → review is the owner's
        assert!(r.set_state(&id, TaskState::Review, "agent:lead").await.is_err());
        assert!(r.set_state(&id, TaskState::Done, "agent:lead").await.is_err());
        let t = r.set_state(&id, TaskState::Review, "agent:ops").await.unwrap();
        assert_eq!(t.state, TaskState::Review);
        assert!(t.checkout.is_none());
        // review → doing is the reviewer's (send back), then review again, then done by reviewer
        assert!(r.set_state(&id, TaskState::Doing, "agent:ops").await.is_err());
        assert_eq!(r.set_state(&id, TaskState::Doing, "agent:lead").await.unwrap().state, TaskState::Doing);
        r.set_state(&id, TaskState::Review, "agent:ops").await.unwrap();
        assert!(r.set_state(&id, TaskState::Done, "agent:ops").await.unwrap_err().contains("reviewer"));
        assert_eq!(r.set_state(&id, TaskState::Done, "agent:lead").await.unwrap().state, TaskState::Done);
        // terminal
        assert!(r.set_state(&id, TaskState::Cancelled, "user").await.unwrap_err().contains("is done"));
        // * → cancelled
        let t2 = r.task("agent:ops").await;
        assert_eq!(r.set_state(&t2.id, TaskState::Cancelled, "agent:ops").await.unwrap().state, TaskState::Cancelled);
        assert!(r.set_state(&t2.id, TaskState::Doing, "user").await.is_err());
        // every state change is an event
        let states: Vec<_> = r.ledger.events(&id).await.unwrap().into_iter().filter(|e| e.field == "state").map(|e| e.after.unwrap()).collect();
        assert_eq!(states, ["blocked", "todo", "doing", "review", "doing", "review", "done"]);
    }

    #[tokio::test]
    async fn reviewer_must_differ_from_owner_unless_user() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger
            .update(&t.id, TaskPatch { reviewer: Some(Owner::Agent("ops".into())), ..Default::default() }, "user", None)
            .await
            .unwrap();
        r.ledger.checkout(&t.id, "s-ops", "agent:ops").await.unwrap();
        r.set_state(&t.id, TaskState::Review, "agent:ops").await.unwrap();
        let e = r.set_state(&t.id, TaskState::Done, "agent:ops").await.unwrap_err();
        assert!(e.contains("other than the owner"), "{e}");
        assert!(r.set_state(&t.id, TaskState::Done, "user").await.unwrap_err().contains("other than the owner"));

        let u = r.task("user").await;
        r.ledger.checkout(&u.id, "s-user", "user").await.unwrap();
        r.set_state(&u.id, TaskState::Review, "user").await.unwrap();
        assert_eq!(r.set_state(&u.id, TaskState::Done, "user").await.unwrap().state, TaskState::Done);
    }

    #[tokio::test]
    async fn checkout_conflict_and_epoch() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger.checkout(&t.id, "s-ops", "agent:ops").await.unwrap();
        let e = r.ledger.checkout(&t.id, "s-ops-2", "agent:ops").await.unwrap_err();
        assert!(e.starts_with("conflict"), "{e}");
        assert!(r.ledger.checkout(&t.id, "s-ops-2", "user").await.unwrap_err().starts_with("conflict"));
        // sent back → checkout is gone → a new checkout bumps the epoch
        r.set_state(&t.id, TaskState::Review, "agent:ops").await.unwrap();
        r.set_state(&t.id, TaskState::Doing, "agent:lead").await.unwrap();
        let t = r.ledger.checkout(&t.id, "s-ops-2", "agent:ops").await.unwrap();
        assert_eq!(t.checkout.as_ref().unwrap().epoch, 1, "epoch restarts once the checkout was cleared");
        assert_eq!(t.checkout.as_ref().unwrap().engine, "mac");
    }

    #[tokio::test]
    async fn project_defaults_and_filter() {
        let mut r = rig().await;
        let parent = r
            .ledger
            .create(
                TaskCreate {
                    title: "parent".into(),
                    brief: String::new(),
                    owner: Owner::User,
                    reviewer: None,
                    parent: None,
                    project: Some("argo".into()),
                    where_: None,
                    harness: None,
                    external: vec![],
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
                    title: "child".into(),
                    brief: String::new(),
                    owner: Owner::Agent("ops".into()),
                    reviewer: None,
                    parent: Some(parent.id.clone()),
                    project: None,
                    where_: None,
                    harness: None,
                    external: vec![],
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
        let argo = r.ledger.list(&TaskFilter { project: Some("argo".into()), ..Default::default() }).await.unwrap();
        assert_eq!(argo.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["parent", "child"]);
        let all = r.ledger.list(&TaskFilter::default()).await.unwrap();
        assert_eq!(all.last().unwrap().id, loose.id, "no project sorts last");
        let ops = r.ledger.list(&TaskFilter { owner: Some("agent:ops".into()), ..Default::default() }).await.unwrap();
        assert_eq!(ops.len(), 2);
        // project is patchable and logged
        let t = r.ledger.update(&loose.id, TaskPatch { project: Some("argo".into()), ..Default::default() }, "user", None).await.unwrap();
        assert_eq!(t.project.as_deref(), Some("argo"));
        assert!(r.ledger.events(&loose.id).await.unwrap().iter().any(|e| e.field == "project" && e.after.as_deref() == Some("argo")));
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
        assert!(r.blockers(&a.id, &[&a.id.0], "user").await.unwrap_err().contains("cycle"));
        // non-task entries and the parent link never cycle
        r.blockers(&c.id, &["decision:x", "person:sky"], "user").await.unwrap();
    }

    #[tokio::test]
    async fn closing_a_blocker_unblocks_to_previous_state() {
        let mut r = rig().await;
        let dep = r.task("agent:lead").await;
        let waiting = r.task("agent:ops").await;
        let held = r.task("agent:ops").await;
        r.ledger.checkout(&held.id, "s-ops", "agent:ops").await.unwrap();
        r.blocked(&waiting.id, &dep.id.0, "agent:ops").await.unwrap();
        r.blocked(&held.id, &dep.id.0, "agent:ops").await.unwrap();
        // inverse
        assert_eq!(r.ledger.blocks_of(&dep.id).await.unwrap(), vec![waiting.id.clone(), held.id.clone()]);
        assert_eq!(r.ledger.get(&dep.id).await.unwrap().blocks.len(), 2);
        assert!(r.ledger.get(&waiting.id).await.unwrap().blocks.is_empty());
        // dep is cancelled → both are free; one had a checkout
        r.set_state(&dep.id, TaskState::Cancelled, "user").await.unwrap();
        let w = r.ledger.get(&waiting.id).await.unwrap();
        assert_eq!((w.state, w.blockers.is_empty()), (TaskState::Todo, true));
        let h = r.ledger.get(&held.id).await.unwrap();
        assert_eq!(h.state, TaskState::Doing);
        assert!(r.ledger.blocks_of(&dep.id).await.unwrap().is_empty());
        let resumes = r.ledger.wakes().await.unwrap().into_iter().filter(|w| w.reason == WakeReason::Resume).count();
        assert_eq!(resumes, 2, "one resume wake per freed task");
        // a task with two blockers waits for both
        let two = r.task("agent:ops").await;
        let d1 = r.task("user").await;
        let d2 = r.task("user").await;
        r.ledger.update(&two.id, TaskPatch { state: Some(TaskState::Blocked), blockers: Some(vec![d1.id.0.clone(), d2.id.0.clone()]), ..Default::default() }, "agent:ops", None).await.unwrap();
        r.set_state(&d1.id, TaskState::Cancelled, "user").await.unwrap();
        assert_eq!(r.ledger.get(&two.id).await.unwrap().state, TaskState::Blocked);
        r.set_state(&d2.id, TaskState::Cancelled, "user").await.unwrap();
        assert_eq!(r.ledger.get(&two.id).await.unwrap().state, TaskState::Todo);
    }

    // ---- wakes (commit 2) ----------------------------------------------------------

    #[tokio::test]
    async fn wakes_coalesce_on_key() {
        let r = rig().await;
        let id = TaskId::new();
        let w1 = r.ledger.store.raise("ops", Some(&id), WakeReason::Comment, 1).await.unwrap();
        let w2 = r.ledger.store.raise("ops", Some(&id), WakeReason::Comment, 2).await.unwrap();
        assert_eq!((w1.id.as_str(), w1.count, w2.count), (w2.id.as_str(), 1, 2));
        r.ledger.store.raise("ops", Some(&id), WakeReason::Mention, 3).await.unwrap();
        r.ledger.store.raise("ops", None, WakeReason::Manual, 4).await.unwrap();
        let all = r.ledger.wakes().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].key, format!("ops|{id}|comment"));
        assert_eq!(all[2].key, "ops||manual");
    }

    #[tokio::test]
    async fn dispatch_claims_oldest_per_agent_and_retries() {
        let mut r = rig().await;
        r.router.desks.lock().unwrap().insert("ops".into(), "d-ops".into());
        let t1 = r.task("agent:ops").await;
        r.tick(10);
        let t2 = r.task("agent:ops").await;
        r.tick(10);
        let t3 = r.task("agent:lead").await;
        let mut d = r.ledger.dispatch().await.unwrap();
        d.sort_by(|a, b| a.wake.agent.cmp(&b.wake.agent));
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].wake.agent.as_str(), d[0].wake.task.as_ref(), d[0].wake.claimed_by.as_deref()), ("lead", Some(&t3.id), Some(DISPATCHING)));
        assert_eq!((d[1].wake.agent.as_str(), d[1].wake.task.as_ref(), d[1].wake.claimed_by.as_deref()), ("ops", Some(&t1.id), Some("d-ops")));
        assert!(r.ledger.dispatch().await.unwrap().is_empty(), "one in flight per agent");
        // lead's desk opened during delivery: the placeholder becomes the desk id
        r.router.desks.lock().unwrap().insert("lead".into(), "d-lead".into());
        r.ledger.delivered(&d[0].wake).await.unwrap();
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!(w.iter().find(|w| w.agent == "lead").unwrap().claimed_by.as_deref(), Some("d-lead"));
        // ops's desk disposes of t1 → its wake is finished → t2 is next
        r.ledger.checkout(&t1.id, "s-ops", "agent:ops").await.unwrap();
        r.set_state(&t1.id, TaskState::Review, "agent:ops").await.unwrap();
        let d = r.ledger.dispatch().await.unwrap();
        assert_eq!(d.iter().map(|d| d.wake.task.clone().unwrap()).collect::<Vec<_>>(), vec![t2.id.clone()]);
        // a failed delivery is unclaimed and held back, then retried
        r.ledger.delivery_failed(&d[0].wake).await.unwrap();
        assert!(r.ledger.dispatch().await.unwrap().is_empty(), "held back 5 s");
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
            r.ledger.store.raise("ops", Some(&id), WakeReason::Manual, 0).await.unwrap();
            let d = r.ledger.dispatch().await.unwrap();
            assert_eq!(d.len(), 1, "wake dispatched");
            // the desk ends the wake with no field change (`continuing`)
            r.ledger.update(&id, TaskPatch::default(), "agent:ops", Some("s-desk-ops")).await.unwrap()
        };
        let t = cycle(&mut r).await;
        assert_eq!((t.no_progress_count, t.next_wake_after_ms), (1, None));
        let now = r.clock.load(Ordering::SeqCst);
        let t = cycle(&mut r).await;
        assert_eq!((t.no_progress_count, t.next_wake_after_ms), (2, Some(now + 120_000)));
        // in back-off: the wake waits
        r.ledger.store.raise("ops", Some(&id), WakeReason::Manual, 0).await.unwrap();
        assert!(r.ledger.dispatch().await.unwrap().is_empty());
        r.tick(120_001);
        let t = cycle(&mut r).await;
        assert_eq!(t.next_wake_after_ms, Some(r.clock.load(Ordering::SeqCst) + 240_000));
        r.tick(240_001);
        for _ in 0..5 {
            let after = r.clock.load(Ordering::SeqCst);
            let t = cycle(&mut r).await;
            r.tick(t.next_wake_after_ms.unwrap() - after + 1);
        }
        let t = r.ledger.get(&id).await.unwrap();
        assert_eq!(t.no_progress_count, 8);
        // capped at backoff_max
        r.ledger.store.raise("ops", Some(&id), WakeReason::Manual, 0).await.unwrap();
        r.ledger.dispatch().await.unwrap();
        let before = r.clock.load(Ordering::SeqCst);
        let t = r.ledger.update(&id, TaskPatch::default(), "agent:ops", Some("s-desk-ops")).await.unwrap();
        assert_eq!(t.next_wake_after_ms, Some(before + 1_800_000));
        // a human comment clears it; a real field change resets the count
        r.ledger.comment(&id, "still needed?", false, "user", None).await.unwrap();
        let t = r.ledger.get(&id).await.unwrap();
        assert_eq!((t.no_progress_count, t.next_wake_after_ms), (0, None));
        assert!(!r.ledger.dispatch().await.unwrap().is_empty(), "the comment wake goes out at once");
        r.ledger.update(&id, TaskPatch { title: Some("renamed".into()), ..Default::default() }, "agent:ops", None).await.unwrap();
        r.ledger.store.raise("ops", Some(&id), WakeReason::Manual, 0).await.unwrap();
        cycle(&mut r).await;
        let t = r.ledger.get(&id).await.unwrap();
        assert_eq!(t.no_progress_count, 1, "progress in between restarted the count");
    }

    #[tokio::test]
    async fn cross_task_write_cap() {
        let mut r = rig().await;
        r.ledger.cfg.cross_task_write_cap = 2;
        let a = r.task("agent:lead").await;
        let b = r.task("agent:lead").await;
        let c = r.task("agent:lead").await;
        let mine = r.task("agent:ops").await;
        r.ledger.comment(&a.id, "one", false, "agent:ops", Some("s-ops")).await.unwrap();
        r.ledger.comment(&b.id, "two", false, "agent:ops", Some("s-ops")).await.unwrap();
        r.ledger.comment(&a.id, "one again", false, "agent:ops", Some("s-ops")).await.unwrap();
        let e = r.ledger.comment(&c.id, "three", false, "agent:ops", Some("s-ops")).await.unwrap_err();
        assert!(e.starts_with("tasks.cross_task_write_cap"), "{e}");
        assert!(r.ledger.update(&c.id, TaskPatch { title: Some("x".into()), ..Default::default() }, "agent:ops", Some("s-ops")).await.is_err());
        // own tasks and other sessions are unaffected
        r.ledger.comment(&mine.id, "mine", false, "agent:ops", Some("s-ops")).await.unwrap();
        r.ledger.comment(&c.id, "three", false, "agent:lead", Some("s-lead")).await.unwrap();
        // a checkout holder owns the task for the cap even when not the owner agent
        r.ledger.checkout(&c.id, "s-user", "user").await.unwrap();
        r.ledger.comment(&c.id, "holder", false, "user", Some("s-user")).await.unwrap();
    }

    #[tokio::test]
    async fn comment_wakes_owner_reviewer_and_mentions() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        // the owner's own comment wakes nobody; a mention of lead does
        r.ledger.comment(&t.id, "asking lead about this", false, "agent:ops", Some("s-ops")).await.unwrap();
        let reasons: Vec<_> = r.ledger.wakes().await.unwrap().into_iter().map(|w| (w.agent, w.reason)).collect();
        assert!(reasons.contains(&("lead".into(), WakeReason::Mention)));
        assert!(!reasons.contains(&("ops".into(), WakeReason::Comment)));
        // the user's comment wakes the owner; under review also the reviewer
        r.ledger.comment(&t.id, "go", false, "user", None).await.unwrap();
        r.ledger.checkout(&t.id, "s-ops", "agent:ops").await.unwrap();
        r.set_state(&t.id, TaskState::Review, "agent:ops").await.unwrap();
        r.ledger.comment(&t.id, "looks off", false, "user", None).await.unwrap();
        let reasons: Vec<_> = r.ledger.wakes().await.unwrap().into_iter().map(|w| (w.agent, w.reason)).collect();
        assert!(reasons.contains(&("ops".into(), WakeReason::Comment)));
        assert!(reasons.contains(&("lead".into(), WakeReason::Comment)));
        assert!(reasons.contains(&("lead".into(), WakeReason::Review)));
        let doc = t.thread.unwrap();
        let blocks = r.thread.0.lock().unwrap().get(&doc).cloned().unwrap();
        assert!(blocks.iter().any(|b| b.starts_with("**user** (") && b.ends_with("): looks off")));
    }

    // ---- orphans, epochs, episodes (commit 3) ---------------------------------------

    #[tokio::test]
    async fn orphan_by_unreachable_engine_then_superseded() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger.checkout(&t.id, "s-ops", "agent:ops").await.unwrap();
        let others: HashSet<String> = HashSet::from(["build-a".to_owned()]);
        assert!(r.ledger.sweep(Some(&others)).await.unwrap().is_empty(), "first sight starts the timer");
        r.tick(119_000);
        assert!(r.ledger.sweep(Some(&others)).await.unwrap().is_empty());
        // it came back: the timer restarts
        let with_mac: HashSet<String> = HashSet::from(["mac".to_owned()]);
        r.ledger.sweep(Some(&with_mac)).await.unwrap();
        r.tick(2_000);
        assert!(r.ledger.sweep(Some(&others)).await.unwrap().is_empty());
        r.tick(120_000);
        assert_eq!(r.ledger.sweep(Some(&others)).await.unwrap(), vec![t.id.clone()]);
        let o = r.ledger.get(&t.id).await.unwrap();
        let c = o.checkout.as_ref().unwrap();
        assert!(c.orphaned && c.epoch == 2 && c.session == "s-ops");
        assert_eq!(o.state, TaskState::Doing);
        assert!(r.ledger.wakes().await.unwrap().iter().any(|w| w.agent == "ops" && w.reason == WakeReason::Resume));
        assert!(r.ledger.events(&t.id).await.unwrap().iter().any(|e| e.field == "checkout" && e.after.as_deref().unwrap().contains("orphaned")));
        assert!(r.ledger.sweep(Some(&others)).await.unwrap().is_empty(), "orphaned once");
        // the owner's new session takes it; the old session is superseded
        let n = r.ledger.checkout(&t.id, "s-ops-2", "agent:ops").await.unwrap();
        assert_eq!(n.checkout.as_ref().unwrap().epoch, 3);
        let e = r.ledger.comment(&t.id, "done so far", true, "agent:ops", Some("s-ops")).await.unwrap_err();
        assert!(e.starts_with("superseded"), "{e}");
        assert!(r.ledger.update(&t.id, TaskPatch { state: Some(TaskState::Review), ..Default::default() }, "agent:ops", Some("s-ops")).await.unwrap_err().starts_with("superseded"));
        r.ledger.comment(&t.id, "resumed", true, "agent:ops", Some("s-ops-2")).await.unwrap();
        assert_eq!(r.ledger.get(&t.id).await.unwrap().checkout.unwrap().progress_at_ms, r.clock.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn orphan_by_staleness() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger.checkout(&t.id, "s-ops", "agent:ops").await.unwrap();
        r.tick(899_000);
        assert!(r.ledger.sweep(None).await.unwrap().is_empty());
        r.ledger.comment(&t.id, "still going", true, "agent:ops", Some("s-ops")).await.unwrap();
        r.tick(899_000);
        assert!(r.ledger.sweep(None).await.unwrap().is_empty(), "the note refreshed progress_at");
        r.tick(2_000);
        // reachable engine, hung session
        let mac: HashSet<String> = HashSet::from(["mac".to_owned()]);
        assert_eq!(r.ledger.sweep(Some(&mac)).await.unwrap(), vec![t.id.clone()]);
        // stale wake claims of a moved desk are released
        let w = r.ledger.store.raise("ops", Some(&t.id), WakeReason::Manual, 0).await.unwrap();
        r.ledger.store.claim(&w.id, Some("old-desk"), 0).await.unwrap();
        r.router.desks.lock().unwrap().insert("ops".into(), "new-desk".into());
        r.ledger.sweep(None).await.unwrap();
        assert!(r.ledger.wakes().await.unwrap().iter().find(|x| x.id == w.id).unwrap().claimed_by.is_none());
    }

    #[tokio::test]
    async fn episode_closing_blocks_leftover_tasks() {
        let mut r = rig().await;
        let left = r.task("agent:ops").await;
        let cont = r.task("agent:ops").await;
        let other = r.task("agent:ops").await;
        r.ledger.checkout(&left.id, "s-ops", "agent:ops").await.unwrap();
        r.ledger.checkout(&cont.id, "s-ops", "agent:ops").await.unwrap();
        r.ledger.checkout(&other.id, "s-elsewhere", "agent:ops").await.unwrap();
        r.ledger.note_disposition(&cont.id, "continuing:schedule nightly", "agent:ops", Some("s-ops")).await.unwrap();
        let w = r.ledger.store.raise("ops", Some(&left.id), WakeReason::Manual, 0).await.unwrap();
        r.ledger.store.claim(&w.id, Some("s-desk-ops"), 0).await.unwrap();
        let blocked = r.ledger.episode_closing("ops", &["s-desk-ops".into(), "s-ops".into()]).await.unwrap();
        assert_eq!(blocked, vec![left.id.clone()]);
        let l = r.ledger.get(&left.id).await.unwrap();
        assert_eq!((l.state, l.blockers.as_slice()), (TaskState::Blocked, &[NO_CONTINUATION.to_owned()][..]));
        assert_eq!(r.ledger.get(&cont.id).await.unwrap().state, TaskState::Doing);
        assert_eq!(r.ledger.get(&other.id).await.unwrap().state, TaskState::Doing);
        let wakes = r.ledger.wakes().await.unwrap();
        assert!(wakes.iter().all(|x| x.id != w.id), "the desk's claimed wake is finished");
        assert!(wakes.iter().any(|x| x.agent == "lead" && x.reason == WakeReason::Review && x.task == Some(left.id.clone())));
    }

    // ---- schedules (commit 4) -----------------------------------------------------

    #[tokio::test]
    async fn schedule_fire_creates_coalesces_and_closes() {
        let r = rig().await;
        let t = r.ledger.create_for_schedule("nightly", 1_000, "ops", "<schedule>go</schedule>", Some("argo".into())).await.unwrap();
        assert_eq!(t.schedule, Some(("nightly".into(), 1_000)));
        assert_eq!((t.owner.clone(), t.project.as_deref(), t.created_by.as_str()), (Owner::Agent("ops".into()), Some("argo"), "system"));
        assert_eq!(t.brief, "<schedule>go</schedule>");
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!((w.len(), w[0].reason, w[0].count), (1, WakeReason::Schedule, 1));
        let again = r.ledger.create_for_schedule("nightly", 1_000, "ops", "<schedule>go</schedule>", None).await.unwrap();
        assert_eq!(again.id, t.id);
        let w = r.ledger.wakes().await.unwrap();
        assert_eq!((w.len(), w[0].count), (1, 2));
        let doc = t.thread.clone().unwrap();
        assert!(r.thread.0.lock().unwrap()[&doc].iter().any(|b| b.contains("fired again (2×)")));
        let next = r.ledger.create_for_schedule("nightly", 2_000, "ops", "p", None).await.unwrap();
        assert_ne!(next.id, t.id);
        // `until` passed: every open task of the schedule is done, reviewer system
        r.ledger.close_for_schedule("nightly", "done: until passed after 2 runs").await.unwrap();
        for id in [&t.id, &next.id] {
            let d = r.ledger.get(id).await.unwrap();
            assert_eq!((d.state, d.reviewer.clone()), (TaskState::Done, Some(Owner::System)));
            assert!(r.ledger.events(id).await.unwrap().iter().any(|e| e.field == "state" && e.after.as_deref() == Some("done")));
        }
        assert_eq!(Owner::System.to_string(), "system");
        assert_eq!("system".parse::<Owner>(), Ok(Owner::System));
    }

    // ---- escalate (67 §11) --------------------------------------------------------

    #[tokio::test]
    async fn escalate_agent_manager_path() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger.checkout(&t.id, "s-ops", "agent:ops").await.unwrap();
        let (decision, manager) = r.ledger.escalate(&t.id, "ship?", &["yes".into()], "agent:ops", Some("s-ops")).await.unwrap();
        assert_eq!(manager, Owner::Agent("lead".into()));
        let b = r.ledger.get(&t.id).await.unwrap();
        assert_eq!((b.state, b.blockers.clone()), (TaskState::Blocked, vec![decision.clone()]));
        assert!(r.ledger.wakes().await.unwrap().iter().any(|w| w.agent == "lead" && w.reason == WakeReason::Answer));
        let doc = t.thread.clone().unwrap();
        assert!(r.thread.0.lock().unwrap()[&doc].iter().any(|b| b.contains("Manager: answer with `decide")));
        // the manager answers: blocker gone, back to doing (live checkout), owner woken `answer`
        r.ledger.answer_decision(&decision, "yes", "agent:lead", Some("s-lead")).await.unwrap();
        let a = r.ledger.get(&t.id).await.unwrap();
        assert_eq!((a.state, a.blockers.is_empty()), (TaskState::Doing, true));
        assert!(r.ledger.wakes().await.unwrap().iter().any(|w| w.agent == "ops" && w.reason == WakeReason::Answer));
        assert!(r.thread.0.lock().unwrap()[&doc].iter().any(|b| b.contains("**decision** (`decision:") && b.ends_with("yes")));
        // nothing is blocked on it any more
        assert_eq!(r.ledger.answer_decision(&decision, "again", "user", None).await, Err(format!("no task is blocked on {decision}")));
    }

    #[tokio::test]
    async fn escalate_user_path() {
        let mut r = rig().await;
        let t = r.task("agent:lead").await;
        let (_, manager) = r.ledger.escalate(&t.id, "budget?", &[], "agent:lead", None).await.unwrap();
        assert_eq!(manager, Owner::User);
        let w = r.ledger.wakes().await.unwrap();
        assert!(w.iter().all(|x| x.reason != WakeReason::Answer), "no agent wake for the user");
        assert_eq!(r.ledger.get(&t.id).await.unwrap().state, TaskState::Blocked);
        // the user is the top of the line
        assert!(r.ledger.escalate(&t.id, "?", &[], "user", None).await.is_err());
    }

    #[tokio::test]
    async fn mention_comments_and_wakes_owner() {
        let mut r = rig().await;
        let t = r.task("agent:ops").await;
        r.ledger.mention(&t.id, "any news?", "user").await.unwrap();
        let doc = t.thread.clone().unwrap();
        assert!(r.thread.0.lock().unwrap()[&doc].iter().any(|b| b.starts_with("**user**") && b.ends_with("any news?")));
        assert!(r.ledger.wakes().await.unwrap().iter().any(|w| w.agent == "ops" && w.reason == WakeReason::Mention));
    }

    #[tokio::test]
    async fn mark_answered_dedups() {
        let r = rig().await;
        assert!(r.ledger.store.mark_answered("+15550100", 17, 1).await.unwrap());
        assert!(!r.ledger.store.mark_answered("+15550100", 17, 2).await.unwrap());
        assert!(r.ledger.store.mark_answered("+15550100", 18, 3).await.unwrap());
    }
}
