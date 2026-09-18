//! Admission and lifecycle bookkeeping for app tool calls (46 §7): pure
//! state, no I/O, so every rule has a counterexample test. The actor owns
//! one [`Scheduler`], takes decisions inside its mailbox and does the actual
//! dispatch in spawned tasks.
//!
//! `T` is the caller's reply handle (a oneshot sender in the actor, a plain
//! token in tests). Handles returned from a method are the caller's to
//! answer `Cancelled`/`Busy`; handles kept inside are answered on `finish`.
//!
//! Rules:
//! - ≤ [`RECORD_ACTIVE`] running and ≤ [`RECORD_PENDING`] queued reads per
//!   record across its views; ≤ [`GLOBAL_ACTIVE`] / [`GLOBAL_PENDING`] overall.
//! - A press never waits: no slot → `Busy`. Reads queue FIFO.
//! - One running + one latest desired value per (view, generation, query):
//!   a newer read for the same query replaces the queued one and cancels
//!   its handle; while one runs, the next waits behind it.
//! - A view is bound to its connection and record on first use; a higher
//!   generation adopts and discards queued/in-flight reads of older ones
//!   (their handles are cancelled; the permit stays until the tool ends).
//! - Closing a view (or its connection) drops queued reads and detaches
//!   in-flight reads; accepted presses keep their handle and finish.

use std::collections::{HashMap, VecDeque};

use crate::Trigger;

/// Running calls per record.
pub const RECORD_ACTIVE: usize = 8;
/// Queued reads per record.
pub const RECORD_PENDING: usize = 32;
/// Running calls per harness.
pub const GLOBAL_ACTIVE: usize = 32;
/// Queued reads per harness.
pub const GLOBAL_PENDING: usize = 128;

/// Bound views per connection.
pub const PEER_VIEWS: usize = 64;

/// A call the actor is asked to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Record the view shows.
    pub record: String,
    /// View.
    pub instance: String,
    /// Generation of the view when requested.
    pub generation: u64,
    /// Bound statement.
    pub statement_id: String,
    /// Invocation id.
    pub request_id: String,
    /// Tool arguments.
    pub args: serde_json::Value,
    /// Why.
    pub trigger: Trigger,
    /// Press action, if any.
    pub action_id: Option<String>,
}

struct View {
    peer: u64,
    record: String,
    generation: u64,
}

struct Queued<T> {
    run: Run,
    reply: T,
}

struct Inflight<T> {
    record: String,
    instance: String,
    generation: u64,
    statement_id: String,
    args: serde_json::Value,
    press: bool,
    reply: Option<T>,
}

/// What [`Scheduler::admit`] decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Admit<T> {
    /// Permits taken; dispatch now. The handle is held until [`Scheduler::finish`].
    /// Boxed: `Run` dwarfs the handle-only variants.
    Start(Box<Run>),
    /// Queued; the handle is held.
    Queued,
    /// Queued, replacing an older read of the same query; answer the old handle `Cancelled`.
    Superseded(T),
    /// Not admitted (press without a slot, or read queue full); answer `Busy`.
    Busy(T),
    /// The same request id with the same record, view, generation, statement,
    /// trigger kind and arguments is already running; answer `Running`.
    Running(T),
    /// The same request id is running with different input; answer `Unavailable`.
    Conflict(T),
}

/// What [`Scheduler::bind`] decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Bind<T> {
    /// Bound; `cancelled` are handles of older-generation reads this call obsoleted.
    Ok {
        /// Answer these `Cancelled`.
        cancelled: Vec<T>,
    },
    /// The view is already on a newer generation.
    Stale,
    /// Another connection or record owns this instance id.
    Refused(String),
}

/// A finished call.
pub struct Finished<T> {
    /// Handle to answer, unless the read was detached by close/adoption.
    pub reply: Option<T>,
    /// Record, for pumping its queue.
    pub record: String,
}

/// The bookkeeping.
pub struct Scheduler<T> {
    views: HashMap<String, View>,
    active_total: usize,
    active: HashMap<String, usize>,
    queued_total: usize,
    queue: HashMap<String, VecDeque<Queued<T>>>,
    inflight: HashMap<String, Inflight<T>>,
}

impl<T> Default for Scheduler<T> {
    fn default() -> Self {
        Self {
            views: HashMap::new(),
            active_total: 0,
            active: HashMap::new(),
            queued_total: 0,
            queue: HashMap::new(),
            inflight: HashMap::new(),
        }
    }
}

impl<T> Scheduler<T> {
    /// Bind `instance` to `peer`/`record` (first use) or check the binding,
    /// and move the view to `generation` when higher. A different `record`
    /// is accepted only together with a higher generation (the view adopted
    /// a new committed version); the caller has already checked that both
    /// records are one app lineage. At most [`PEER_VIEWS`] views per peer.
    pub fn bind(&mut self, instance: &str, peer: u64, record: &str, generation: u64) -> Bind<T> {
        if !self.views.contains_key(instance)
            && self.views.values().filter(|v| v.peer == peer).count() >= PEER_VIEWS
        {
            return Bind::Refused(format!(
                "too many open views on this connection ({PEER_VIEWS}); close some"
            ));
        }
        let view = self
            .views
            .entry(instance.to_owned())
            .or_insert_with(|| View {
                peer,
                record: record.to_owned(),
                generation,
            });
        if view.peer != peer {
            return Bind::Refused("instance is bound to another connection".into());
        }
        if generation < view.generation {
            return Bind::Stale;
        }
        if view.record != record && generation == view.generation {
            return Bind::Refused(format!(
                "instance shows record {} at this generation; adopt a new record with a higher generation",
                view.record
            ));
        }
        let mut cancelled = Vec::new();
        if generation > view.generation {
            view.generation = generation;
            view.record = record.to_owned();
            cancelled = self.detach_reads(instance, |g| g < generation);
        }
        Bind::Ok { cancelled }
    }

    /// Whether any view is bound to `peer`.
    pub fn has_peer(&self, peer: u64) -> bool {
        self.views.values().any(|v| v.peer == peer)
    }

    /// Whether any view shows `record`.
    pub fn has_record(&self, record: &str) -> bool {
        self.views.values().any(|v| v.record == record)
    }

    /// The connection `instance` is bound to.
    pub fn peer_of(&self, instance: &str) -> Option<u64> {
        self.views.get(instance).map(|v| v.peer)
    }

    /// The record `instance` shows.
    pub fn record_of(&self, instance: &str) -> Option<String> {
        self.views.get(instance).map(|v| v.record.clone())
    }

    /// Whether `request_id` is running now.
    pub fn is_running(&self, request_id: &str) -> bool {
        self.inflight.contains_key(request_id)
    }

    /// Decide for one call. The view must be bound.
    pub fn admit(&mut self, run: Run, reply: T) -> Admit<T> {
        if let Some(f) = self.inflight.get(&run.request_id) {
            let same = f.record == run.record
                && f.instance == run.instance
                && f.generation == run.generation
                && f.statement_id == run.statement_id
                && f.press == run.trigger.is_press()
                && f.args == run.args;
            return if same {
                Admit::Running(reply)
            } else {
                Admit::Conflict(reply)
            };
        }
        // The same id waiting in a queue: a retry takes over its place (the
        // newest handle gets the result); different input is a conflict.
        // Never two entries per id, so `start` can never overwrite an
        // in-flight entry and leak its permit. Identity includes the view and
        // generation so the takeover keeps "one queued entry per query".
        if let Some(q) = self
            .queue
            .values_mut()
            .flat_map(|q| q.iter_mut())
            .find(|q| q.run.request_id == run.request_id)
        {
            let same = q.run.record == run.record
                && q.run.instance == run.instance
                && q.run.generation == run.generation
                && q.run.statement_id == run.statement_id
                && q.run.trigger.is_press() == run.trigger.is_press()
                && q.run.args == run.args;
            if !same {
                return Admit::Conflict(reply);
            }
            let old = std::mem::replace(q, Queued { run, reply });
            return Admit::Superseded(old.reply);
        }
        let press = run.trigger.is_press();
        let same_query_running = !press
            && self.inflight.values().any(|f| {
                !f.press
                    && f.instance == run.instance
                    && f.generation == run.generation
                    && f.statement_id == run.statement_id
            });
        if !same_query_running && self.slot_free(&run.record) {
            return Admit::Start(Box::new(self.start(run, reply)));
        }
        if press {
            return Admit::Busy(reply);
        }
        let queue = self.queue.entry(run.record.clone()).or_default();
        if let Some(q) = queue.iter_mut().find(|q| {
            q.run.instance == run.instance
                && q.run.generation == run.generation
                && q.run.statement_id == run.statement_id
        }) {
            let old = std::mem::replace(q, Queued { run, reply });
            return Admit::Superseded(old.reply);
        }
        if queue.len() >= RECORD_PENDING || self.queued_total >= GLOBAL_PENDING {
            return Admit::Busy(reply);
        }
        queue.push_back(Queued { run, reply });
        self.queued_total += 1;
        Admit::Queued
    }

    /// A call ended: release its permits, hand back its handle.
    pub fn finish(&mut self, request_id: &str) -> Option<Finished<T>> {
        let f = self.inflight.remove(request_id)?;
        self.active_total = self.active_total.saturating_sub(1);
        if let Some(n) = self.active.get_mut(&f.record) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.active.remove(&f.record);
            }
        }
        Some(Finished {
            reply: f.reply,
            record: f.record,
        })
    }

    /// Pump every record with a queue after a slot freed anywhere: under the
    /// global cap the record that finished is not necessarily the one that
    /// should start next. Round-robin, one start per record per round, so a
    /// record with a deep queue cannot take every freed slot ahead of others.
    pub fn pump_all(&mut self) -> (Vec<Run>, Vec<T>) {
        let mut started = Vec::new();
        let mut cancelled = Vec::new();
        loop {
            let before = started.len();
            let mut records: Vec<String> = self.queue.keys().cloned().collect();
            records.sort_unstable();
            for r in records {
                let (s, c) = self.pump_up_to(&r, 1);
                started.extend(s);
                cancelled.extend(c);
            }
            if started.len() == before {
                break;
            }
        }
        (started, cancelled)
    }

    /// Start queued reads of `record` while slots allow. Entries whose view
    /// is gone or generation is stale were already detached; this skips any
    /// stragglers and cancels their handles.
    pub fn pump(&mut self, record: &str) -> (Vec<Run>, Vec<T>) {
        self.pump_up_to(record, usize::MAX)
    }

    fn pump_up_to(&mut self, record: &str, limit: usize) -> (Vec<Run>, Vec<T>) {
        let mut started = Vec::new();
        let mut cancelled = Vec::new();
        while started.len() < limit && self.slot_free(record) {
            let Some(queue) = self.queue.get_mut(record) else {
                break;
            };
            // Skip a query whose twin is still running; it stays queued behind it.
            let idx = queue.iter().position(|q| {
                !self.inflight.values().any(|f| {
                    !f.press
                        && f.instance == q.run.instance
                        && f.generation == q.run.generation
                        && f.statement_id == q.run.statement_id
                })
            });
            let Some(idx) = idx else { break };
            let Some(q) = queue.remove(idx) else { break };
            self.queued_total = self.queued_total.saturating_sub(1);
            let current = self
                .views
                .get(&q.run.instance)
                .is_some_and(|v| v.generation == q.run.generation)
                && !self.inflight.contains_key(&q.run.request_id);
            if !current {
                cancelled.push(q.reply);
                continue;
            }
            started.push(self.start(q.run, q.reply));
        }
        if self.queue.get(record).is_some_and(VecDeque::is_empty) {
            self.queue.remove(record);
        }
        (started, cancelled)
    }

    /// Close one view: its queued reads and in-flight reads hand back their
    /// handles; presses keep theirs.
    pub fn close(&mut self, instance: &str) -> Vec<T> {
        if self.views.remove(instance).is_none() {
            return Vec::new();
        }
        self.detach_reads(instance, |_| true)
    }

    /// Close every view of `peer`.
    pub fn peer_closed(&mut self, peer: u64) -> Vec<T> {
        self.close_where(|v| v.peer == peer)
    }

    /// Close every view showing `record` (its session was deleted).
    pub fn close_record(&mut self, record: &str) -> Vec<T> {
        self.close_where(|v| v.record == record)
    }

    fn close_where(&mut self, gone: impl Fn(&View) -> bool) -> Vec<T> {
        let gone: Vec<String> = self
            .views
            .iter()
            .filter(|(_, v)| gone(v))
            .map(|(k, _)| k.clone())
            .collect();
        gone.into_iter().flat_map(|i| self.close(&i)).collect()
    }

    /// Running calls right now (tests, diagnostics).
    pub fn active(&self) -> usize {
        self.active_total
    }

    /// Queued reads right now (tests, diagnostics).
    pub fn queued(&self) -> usize {
        self.queued_total
    }

    fn slot_free(&self, record: &str) -> bool {
        self.active_total < GLOBAL_ACTIVE
            && self.active.get(record).copied().unwrap_or(0) < RECORD_ACTIVE
    }

    /// Take permits and record the call; returns the run to dispatch.
    fn start(&mut self, run: Run, reply: T) -> Run {
        self.active_total += 1;
        *self.active.entry(run.record.clone()).or_insert(0) += 1;
        self.inflight.insert(
            run.request_id.clone(),
            Inflight {
                record: run.record.clone(),
                instance: run.instance.clone(),
                generation: run.generation,
                statement_id: run.statement_id.clone(),
                args: run.args.clone(),
                press: run.trigger.is_press(),
                reply: Some(reply),
            },
        );
        run
    }

    /// Drop queued reads and detach in-flight reads of `instance` whose
    /// generation satisfies `obsolete`.
    fn detach_reads(&mut self, instance: &str, obsolete: impl Fn(u64) -> bool) -> Vec<T> {
        let mut out = Vec::new();
        for queue in self.queue.values_mut() {
            let mut i = 0;
            while i < queue.len() {
                if queue[i].run.instance == instance && obsolete(queue[i].run.generation) {
                    if let Some(q) = queue.remove(i) {
                        out.push(q.reply);
                        self.queued_total = self.queued_total.saturating_sub(1);
                    }
                } else {
                    i += 1;
                }
            }
        }
        self.queue.retain(|_, q| !q.is_empty());
        for f in self.inflight.values_mut() {
            if !f.press
                && f.instance == instance
                && obsolete(f.generation)
                && let Some(r) = f.reply.take()
            {
                out.push(r);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        instance: &str,
        generation: u64,
        statement: &str,
        request: &str,
        trigger: Trigger,
    ) -> Run {
        Run {
            record: "rec".into(),
            instance: instance.into(),
            generation,
            statement_id: statement.into(),
            request_id: request.into(),
            args: serde_json::json!({}),
            trigger,
            action_id: trigger.is_press().then(|| "act".to_owned()),
        }
    }

    fn read(request: &str) -> Run {
        run("v1", 1, "health", request, Trigger::Refresh)
    }

    fn bound() -> Scheduler<u32> {
        let mut s = Scheduler::default();
        assert_eq!(s.bind("v1", 7, "rec", 1), Bind::Ok { cancelled: vec![] });
        s
    }

    #[test]
    fn instance_binding_is_immutable_and_stale_generations_are_refused() {
        let mut s = bound();
        assert!(
            matches!(s.bind("v1", 8, "rec", 1), Bind::Refused(_)),
            "other connection"
        );
        assert!(
            matches!(s.bind("v1", 7, "other", 1), Bind::Refused(_)),
            "other record, same generation"
        );
        assert_eq!(s.bind("v1", 7, "rec", 2), Bind::Ok { cancelled: vec![] });
        assert_eq!(s.bind("v1", 7, "rec", 1), Bind::Stale);
        assert!(s.has_peer(7) && !s.has_peer(8));
    }

    #[test]
    fn adopting_a_new_version_moves_the_view_and_keeps_old_permits_under_the_old_record() {
        let mut s = bound();
        assert!(matches!(s.admit(read("r1"), 1), Admit::Start(_)));
        assert_eq!(s.admit(read("r2"), 2), Admit::Queued);
        let Bind::Ok { mut cancelled } = s.bind("v1", 7, "rec-v2", 2) else {
            panic!()
        };
        cancelled.sort_unstable();
        assert_eq!(cancelled, vec![1, 2]);
        assert_eq!(s.record_of("v1").as_deref(), Some("rec-v2"));
        assert!(!s.has_record("rec") && s.has_record("rec-v2"));
        assert_eq!(s.active(), 1, "old read still holds its permit");
        let mut run = run("v1", 2, "health", "r3", Trigger::Load);
        run.record = "rec-v2".into();
        assert!(matches!(s.admit(run, 3), Admit::Start(_)));
        assert_eq!(s.finish("r1").map(|f| f.record), Some("rec".into()));
        assert_eq!(s.finish("r3").map(|f| f.record), Some("rec-v2".into()));
        assert_eq!(s.active(), 0);
        // Going back to the old record at the same generation is a refusal, not a rebind.
        assert!(matches!(s.bind("v1", 7, "rec", 2), Bind::Refused(_)));
    }

    #[test]
    fn views_per_connection_are_capped() {
        let mut s: Scheduler<u32> = Scheduler::default();
        for i in 0..PEER_VIEWS {
            assert!(matches!(
                s.bind(&format!("v{i}"), 1, "rec", 1),
                Bind::Ok { .. }
            ));
        }
        assert!(matches!(s.bind("one-more", 1, "rec", 1), Bind::Refused(_)));
        assert!(
            matches!(s.bind("v0", 1, "rec", 1), Bind::Ok { .. }),
            "existing views still bind"
        );
        assert!(matches!(s.bind("other-peer", 2, "rec", 1), Bind::Ok { .. }));
        assert_eq!(s.close("v0"), Vec::<u32>::new());
        assert!(
            matches!(s.bind("one-more", 1, "rec", 1), Bind::Ok { .. }),
            "closing frees a slot"
        );
    }

    #[test]
    fn press_gets_busy_rather_than_a_queue_and_reads_queue_fifo() {
        let mut s = bound();
        for i in 0..RECORD_ACTIVE {
            let r = run("v1", 1, &format!("q{i}"), &format!("r{i}"), Trigger::Load);
            assert!(matches!(s.admit(r, i as u32), Admit::Start(_)));
        }
        assert_eq!(s.active(), RECORD_ACTIVE);
        let press = run("v1", 1, "sync", "p1", Trigger::Press);
        assert_eq!(s.admit(press, 100), Admit::Busy(100));
        assert_eq!(
            s.admit(run("v1", 1, "a", "ra", Trigger::Load), 101),
            Admit::Queued
        );
        assert_eq!(
            s.admit(run("v1", 1, "b", "rb", Trigger::Load), 102),
            Admit::Queued
        );
        assert_eq!(s.queued(), 2);
        // One finishes → the first queued read starts, in order.
        let done = s.finish("r0").unwrap();
        assert_eq!(done.reply, Some(0));
        let (started, cancelled) = s.pump("rec");
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].request_id, "ra");
        assert!(cancelled.is_empty());
        assert_eq!(s.queued(), 1);
        assert!(s.is_running("ra") && !s.is_running("r0"));
    }

    #[test]
    fn record_queue_overflows_to_busy() {
        let mut s = bound();
        for i in 0..RECORD_ACTIVE {
            assert!(matches!(
                s.admit(
                    run("v1", 1, &format!("q{i}"), &format!("r{i}"), Trigger::Load),
                    0
                ),
                Admit::Start(_)
            ));
        }
        for i in 0..RECORD_PENDING {
            assert_eq!(
                s.admit(
                    run("v1", 1, &format!("p{i}"), &format!("x{i}"), Trigger::Load),
                    1
                ),
                Admit::Queued
            );
        }
        assert_eq!(
            s.admit(run("v1", 1, "extra", "xx", Trigger::Load), 9),
            Admit::Busy(9)
        );
        // A newer value for an already queued query still replaces it when full.
        assert_eq!(
            s.admit(run("v1", 1, "p0", "x0b", Trigger::StateChange), 10),
            Admit::Superseded(1)
        );
    }

    #[test]
    fn global_active_cap_spans_records() {
        let mut s: Scheduler<u32> = Scheduler::default();
        for r in 0..(GLOBAL_ACTIVE / 4) {
            let record = format!("rec{r}");
            let instance = format!("v{r}");
            s.bind(&instance, 1, &record, 1);
            for i in 0..4 {
                let mut run = run(
                    &instance,
                    1,
                    &format!("q{i}"),
                    &format!("{record}-{i}"),
                    Trigger::Load,
                );
                run.record = record.clone();
                assert!(matches!(s.admit(run, 0), Admit::Start(_)));
            }
        }
        assert_eq!(s.active(), GLOBAL_ACTIVE);
        s.bind("vz", 1, "recz", 1);
        let mut press = run("vz", 1, "m", "pz", Trigger::Press);
        press.record = "recz".into();
        assert_eq!(s.admit(press, 5), Admit::Busy(5));
    }

    #[test]
    fn same_query_coalesces_to_one_running_plus_latest_desired() {
        let mut s = bound();
        assert!(matches!(s.admit(read("r1"), 1), Admit::Start(_)));
        // Same query while running: waits behind it even though slots are free.
        assert_eq!(s.admit(read("r2"), 2), Admit::Queued);
        // A newer value replaces the waiting one and cancels it.
        assert_eq!(s.admit(read("r3"), 3), Admit::Superseded(2));
        assert_eq!(s.queued(), 1);
        // A different query on the same view is independent.
        assert!(matches!(
            s.admit(run("v1", 1, "other", "r4", Trigger::Load), 4),
            Admit::Start(_)
        ));
        let done = s.finish("r1").unwrap();
        assert_eq!(done.reply, Some(1));
        let (started, _) = s.pump("rec");
        assert_eq!(
            started
                .iter()
                .map(|r| r.request_id.as_str())
                .collect::<Vec<_>>(),
            ["r3"]
        );
    }

    #[test]
    fn queued_duplicate_ids_never_produce_two_entries_or_leak_permits() {
        let mut s = bound();
        for i in 0..RECORD_ACTIVE {
            assert!(matches!(
                s.admit(
                    run("v1", 1, &format!("q{i}"), &format!("r{i}"), Trigger::Load),
                    0
                ),
                Admit::Start(_)
            ));
        }
        assert_eq!(
            s.admit(run("v1", 1, "late", "L", Trigger::Load), 1),
            Admit::Queued
        );
        // Retry of the queued read: the new handle takes its place.
        assert_eq!(
            s.admit(run("v1", 1, "late", "L", Trigger::Load), 2),
            Admit::Superseded(1)
        );
        // Same id, other input: conflict, nothing queued twice.
        let mut other = run("v1", 1, "late", "L", Trigger::Load);
        other.args = serde_json::json!({ "x": 1 });
        assert_eq!(s.admit(other, 3), Admit::Conflict(3));
        assert_eq!(s.queued(), 1);
        // A slot frees: the queued read starts once and finishes once.
        s.finish("r0");
        let (started, _) = s.pump_all();
        assert_eq!(started.len(), 1);
        assert_eq!(s.active(), RECORD_ACTIVE);
        assert_eq!(s.finish("L").unwrap().reply, Some(2));
        assert!(s.finish("L").is_none());
        assert_eq!(s.active(), RECORD_ACTIVE - 1);
    }

    #[test]
    fn a_slot_freed_under_the_global_cap_serves_other_records() {
        let mut s: Scheduler<u32> = Scheduler::default();
        for r in 0..(GLOBAL_ACTIVE / RECORD_ACTIVE) {
            let record = format!("rec{r}");
            s.bind(&format!("v{r}"), 1, &record, 1);
            for i in 0..RECORD_ACTIVE {
                let mut run = run(
                    &format!("v{r}"),
                    1,
                    &format!("q{i}"),
                    &format!("{record}-{i}"),
                    Trigger::Load,
                );
                run.record = record.clone();
                assert!(matches!(s.admit(run, 0), Admit::Start(_)));
            }
        }
        assert_eq!(s.active(), GLOBAL_ACTIVE);
        s.bind("vz", 1, "recz", 1);
        let mut waiting = run("vz", 1, "h", "z1", Trigger::Load);
        waiting.record = "recz".into();
        assert_eq!(s.admit(waiting, 9), Admit::Queued);
        // A call on an unrelated record finishes; pumping only that record would starve recz.
        s.finish("rec0-0");
        let (started, _) = s.pump_all();
        assert_eq!(
            started
                .iter()
                .map(|r| r.request_id.as_str())
                .collect::<Vec<_>>(),
            ["z1"]
        );
        assert_eq!(s.active(), GLOBAL_ACTIVE);
    }

    #[test]
    fn duplicate_request_id_while_running_reports_running_only_for_identical_input() {
        let mut s = bound();
        let press = run("v1", 1, "sync", "p1", Trigger::Press);
        assert!(matches!(s.admit(press.clone(), 1), Admit::Start(_)));
        assert_eq!(s.admit(press.clone(), 2), Admit::Running(2));
        let mut other_args = press.clone();
        other_args.args = serde_json::json!({ "target": "else" });
        assert_eq!(s.admit(other_args, 3), Admit::Conflict(3));
        let mut other_statement = press.clone();
        other_statement.statement_id = "wipe".into();
        assert_eq!(s.admit(other_statement, 4), Admit::Conflict(4));
        let mut as_read = press;
        as_read.trigger = Trigger::Load;
        as_read.action_id = None;
        assert_eq!(s.admit(as_read, 5), Admit::Conflict(5));
    }

    #[test]
    fn newer_generation_discards_older_reads_but_not_presses() {
        let mut s = bound();
        assert!(matches!(s.admit(read("r1"), 1), Admit::Start(_)));
        assert_eq!(s.admit(read("r2"), 2), Admit::Queued);
        let press = run("v1", 1, "sync", "p1", Trigger::Press);
        assert!(matches!(s.admit(press, 3), Admit::Start(_)));
        let Bind::Ok { mut cancelled } = s.bind("v1", 7, "rec", 2) else {
            panic!()
        };
        cancelled.sort_unstable();
        assert_eq!(cancelled, vec![1, 2], "queued and in-flight reads of gen 1");
        assert_eq!(s.active(), 2, "permits stay until the tools end");
        assert_eq!(s.queued(), 0);
        // Late output of the obsolete read has nobody to answer; the press keeps its handle.
        assert_eq!(s.finish("r1").unwrap().reply, None);
        assert_eq!(s.finish("p1").unwrap().reply, Some(3));
        assert_eq!(s.active(), 0);
        // Reads of the new generation flow normally.
        assert!(matches!(
            s.admit(run("v1", 2, "health", "r9", Trigger::Load), 9),
            Admit::Start(_)
        ));
    }

    #[test]
    fn close_and_peer_close_drop_reads_keep_presses_and_free_the_instance() {
        let mut s = bound();
        s.bind("v2", 7, "rec", 1);
        s.bind("v3", 8, "rec", 1);
        assert!(matches!(s.admit(read("r1"), 1), Admit::Start(_)));
        assert_eq!(s.admit(read("r2"), 2), Admit::Queued);
        assert!(matches!(
            s.admit(run("v1", 1, "sync", "p1", Trigger::Press), 3),
            Admit::Start(_)
        ));
        assert!(matches!(
            s.admit(run("v2", 1, "h", "r5", Trigger::Load), 5),
            Admit::Start(_)
        ));
        assert!(matches!(
            s.admit(run("v3", 1, "h", "r6", Trigger::Load), 6),
            Admit::Start(_)
        ));

        let mut dropped = s.peer_closed(7);
        dropped.sort_unstable();
        assert_eq!(dropped, vec![1, 2, 5]);
        assert!(!s.has_peer(7) && s.has_peer(8));
        assert_eq!(
            s.active(),
            4,
            "in-flight permits are held until the tools end"
        );
        assert_eq!(s.queued(), 0);
        assert_eq!(
            s.finish("p1").unwrap().reply,
            Some(3),
            "accepted write still answers"
        );
        assert_eq!(s.finish("r1").unwrap().reply, None);
        assert!(s.close("v1").is_empty(), "closing again is a no-op");
        // The instance id can be rebound by any connection now.
        assert_eq!(s.bind("v1", 9, "rec", 1), Bind::Ok { cancelled: vec![] });
        assert_eq!(s.close("v3"), vec![6]);
        assert_eq!(s.finish("r6").unwrap().reply, None);
        assert_eq!(s.finish("r5").unwrap().reply, None);
        assert_eq!(s.active(), 0);
        assert!(s.finish("r5").is_none());
    }

    #[test]
    fn freed_global_slots_are_shared_round_robin_across_waiting_records() {
        let mut s: Scheduler<u32> = Scheduler::default();
        for r in 0..(GLOBAL_ACTIVE / RECORD_ACTIVE) {
            let record = format!("rec{r}");
            s.bind(&format!("v{r}"), 1, &record, 1);
            for i in 0..RECORD_ACTIVE {
                let mut run = run(
                    &format!("v{r}"),
                    1,
                    &format!("q{i}"),
                    &format!("{record}-{i}"),
                    Trigger::Load,
                );
                run.record = record.clone();
                assert!(matches!(s.admit(run, 0), Admit::Start(_)));
            }
        }
        // Two idle records each queue three reads behind the global cap.
        for rec in ["a", "b"] {
            s.bind(rec, 1, rec, 1);
            for i in 0..3 {
                let mut w = run(
                    rec,
                    1,
                    &format!("h{i}"),
                    &format!("{rec}{i}"),
                    Trigger::Load,
                );
                w.record = rec.into();
                assert_eq!(s.admit(w, 9), Admit::Queued);
            }
        }
        assert_eq!(s.queued(), 6);
        // Two slots free on an unrelated record: one goes to each waiting record,
        // not both to whichever queue is visited first.
        s.finish("rec0-0");
        s.finish("rec0-1");
        let (started, cancelled) = s.pump_all();
        assert!(cancelled.is_empty());
        let mut by_record: Vec<&str> = started.iter().map(|r| r.record.as_str()).collect();
        by_record.sort_unstable();
        assert_eq!(by_record, ["a", "b"]);
        assert_eq!(s.active(), GLOBAL_ACTIVE);
        assert_eq!(s.queued(), 4);
        // Nothing free: pumping again dispatches nothing and leaks nothing.
        assert!(s.pump_all().0.is_empty());
        assert_eq!(s.active(), GLOBAL_ACTIVE);
    }

    #[test]
    fn duplicate_request_id_from_another_view_or_generation_is_a_conflict() {
        let mut s = bound();
        s.bind("v2", 7, "rec", 1);
        for i in 0..RECORD_ACTIVE {
            assert!(matches!(
                s.admit(
                    run("v1", 1, &format!("q{i}"), &format!("r{i}"), Trigger::Load),
                    0
                ),
                Admit::Start(_)
            ));
        }
        // Running under v1: the same id from v2 is not the same call.
        assert_eq!(
            s.admit(run("v2", 1, "q0", "r0", Trigger::Load), 1),
            Admit::Conflict(1)
        );
        assert_eq!(
            s.admit(run("v1", 1, "late", "L", Trigger::Load), 2),
            Admit::Queued
        );
        // Queued under v1 gen 1: other view or newer generation is a conflict, not a takeover.
        assert_eq!(
            s.admit(run("v2", 1, "late", "L", Trigger::Load), 3),
            Admit::Conflict(3)
        );
        let Bind::Ok { mut cancelled } = s.bind("v1", 7, "rec", 2) else {
            panic!()
        };
        cancelled.sort_unstable();
        assert_eq!(
            cancelled.last(),
            Some(&2),
            "queued L cancelled with the gen-1 reads"
        );
        assert_eq!(s.queued(), 0);
        assert_eq!(
            s.admit(run("v1", 2, "late", "L", Trigger::Load), 4),
            Admit::Queued
        );
        assert_eq!(
            s.admit(run("v1", 1, "late", "L", Trigger::Load), 5),
            Admit::Conflict(5)
        );
        assert_eq!(s.queued(), 1);
        s.finish("r0");
        let (started, cancelled) = s.pump_all();
        assert_eq!(
            started
                .iter()
                .map(|r| r.request_id.as_str())
                .collect::<Vec<_>>(),
            ["L"]
        );
        assert!(cancelled.is_empty());
        assert_eq!(s.finish("L").unwrap().reply, Some(4));
    }
}
