//! Engine startup progress (design doc 222): `<data_dir>/startup.json`, written
//! atomically (temp file + rename) on every phase/step change and by a 1 s
//! heartbeat thread while the engine is starting. The desktop shell tails it.
//!
//! One process-global reporter, set by [`init`] once the engine holds its data-dir
//! lock. Before that — and in tests, CLI subcommands and attached clients — every
//! call here is a no-op. After [`ready`] or [`failed`] the file is final: later
//! calls are no-ops too.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

/// The file name under the data dir.
pub const FILE: &str = "startup.json";
/// `done` keeps the last this many finished phases.
const DONE_CAP: usize = 200;
const HEARTBEAT: Duration = Duration::from_secs(1);

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// Start reporting for this boot: overwrites `<data_dir>/startup.json` with the
/// phase `lock` (begun at `since`) and starts the heartbeat thread. A second call
/// is ignored.
pub fn init(data_dir: &Path, since: Instant) {
    if REPORTER.get().is_some() {
        return;
    }
    let reporter = Reporter::new(data_dir, since, "lock", "Acquiring the engine lock");
    if REPORTER.set(reporter).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("startup-heartbeat".into())
        .spawn(|| {
            let Some(r) = REPORTER.get() else { return };
            loop {
                std::thread::sleep(HEARTBEAT);
                if !r.heartbeat() {
                    break;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "startup heartbeat thread");
    }
}

/// Whether [`init`] ran and startup is still in progress.
pub fn active() -> bool {
    REPORTER.get().is_some_and(Reporter::running)
}

/// Finish the current phase (into `done`) and start `id`.
pub fn phase(id: &str, label: &str, step: Option<(u64, u64)>) {
    if let Some(r) = REPORTER.get() {
        r.phase(id, label, step);
    }
}

/// Set the current phase's step `n` of `of`.
pub fn step(n: u64, of: u64) {
    if let Some(r) = REPORTER.get() {
        r.step(n, of);
    }
}

/// A nested phase (a migration inside `plugin:<name>`): the current phase resumes
/// when the returned guard drops.
pub fn scope(id: &str, label: &str) -> Scope<'static> {
    match REPORTER.get() {
        Some(r) => r.scope(id, label),
        None => Scope::none(),
    }
}

/// The RPC listener is bound: state `ready`; the heartbeat stops.
pub fn ready() {
    if let Some(r) = REPORTER.get() {
        r.ready();
    }
}

/// Startup returned `error` before ready: state `failed`; the heartbeat stops.
pub fn failed(error: &str) {
    if let Some(r) = REPORTER.get() {
        r.failed(error);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Running,
    Ready,
    Failed,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Ready => "ready",
            State::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct Active {
    id: String,
    label: String,
    step: Option<(u64, u64)>,
    since: Instant,
    /// `phase_ms`: epoch ms of `since`, fixed when the phase starts.
    since_ms: u64,
}

impl Active {
    fn now(id: &str, label: &str, step: Option<(u64, u64)>) -> Self {
        Active {
            id: id.to_owned(),
            label: label.to_owned(),
            step,
            since: Instant::now(),
            since_ms: now_ms(),
        }
    }
}

struct Done {
    phase: String,
    label: String,
    ms: u64,
}

struct Inner {
    path: PathBuf,
    tmp: PathBuf,
    pid: u32,
    started_ms: u64,
    seq: u64,
    state: State,
    active: Active,
    done: VecDeque<Done>,
    error: Option<String>,
}

/// The reporter behind the free functions; one per boot.
pub struct Reporter {
    inner: Mutex<Inner>,
}

/// Guard of [`scope`]: on drop, the nested phase moves to `done` and the phase it
/// interrupted resumes — unless another phase replaced it meanwhile.
#[must_use = "the nested phase ends when the guard drops"]
pub struct Scope<'a> {
    reporter: Option<&'a Reporter>,
    id: String,
    prev: Option<Active>,
}

impl Scope<'_> {
    fn none() -> Self {
        Scope {
            reporter: None,
            id: String::new(),
            prev: None,
        }
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        if let (Some(r), Some(prev)) = (self.reporter, self.prev.take()) {
            r.end_scope(&self.id, prev);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Reporter {
    fn new(data_dir: &Path, since: Instant, id: &str, label: &str) -> Self {
        let started_ms = now_ms().saturating_sub(since.elapsed().as_millis() as u64);
        let pid = std::process::id();
        let inner = Inner {
            path: data_dir.join(FILE),
            tmp: data_dir.join(format!(".{FILE}.{pid}.tmp")),
            pid,
            started_ms,
            seq: 0,
            state: State::Running,
            active: Active {
                id: id.to_owned(),
                label: label.to_owned(),
                step: None,
                since,
                since_ms: started_ms,
            },
            done: VecDeque::new(),
            error: None,
        };
        inner.write();
        Reporter {
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn running(&self) -> bool {
        self.lock().state == State::Running
    }

    fn phase(&self, id: &str, label: &str, step: Option<(u64, u64)>) {
        let mut g = self.lock();
        if g.state != State::Running {
            return;
        }
        g.finish_active();
        g.active = Active::now(id, label, step);
        g.seq += 1;
        g.write();
    }

    fn step(&self, n: u64, of: u64) {
        let mut g = self.lock();
        if g.state != State::Running || g.active.step == Some((n, of)) {
            return;
        }
        g.active.step = Some((n, of));
        g.seq += 1;
        g.write();
    }

    fn scope(&self, id: &str, label: &str) -> Scope<'_> {
        let mut g = self.lock();
        if g.state != State::Running {
            return Scope::none();
        }
        // The interrupted phase is not done: it resumes, with its own start, on drop.
        let prev = std::mem::replace(&mut g.active, Active::now(id, label, None));
        g.seq += 1;
        g.write();
        drop(g);
        Scope {
            reporter: Some(self),
            id: id.to_owned(),
            prev: Some(prev),
        }
    }

    fn end_scope(&self, id: &str, prev: Active) {
        let mut g = self.lock();
        if g.state != State::Running || g.active.id != id {
            return;
        }
        g.finish_active();
        g.active = prev;
        g.seq += 1;
        g.write();
    }

    /// Rewrite with a fresh `hb_ms`; `false` once startup is over.
    fn heartbeat(&self) -> bool {
        let g = self.lock();
        if g.state != State::Running {
            return false;
        }
        g.write();
        true
    }

    fn ready(&self) {
        let mut g = self.lock();
        if g.state != State::Running {
            return;
        }
        g.finish_active();
        g.active = Active::now("ready", "Ready", None);
        g.state = State::Ready;
        g.seq += 1;
        g.write();
    }

    fn failed(&self, error: &str) {
        let mut g = self.lock();
        if g.state != State::Running {
            return;
        }
        g.state = State::Failed;
        g.error = Some(error.to_owned());
        g.write();
    }
}

impl Inner {
    fn finish_active(&mut self) {
        if self.done.len() == DONE_CAP {
            self.done.pop_front();
        }
        self.done.push_back(Done {
            phase: self.active.id.clone(),
            label: self.active.label.clone(),
            ms: self.active.since.elapsed().as_millis() as u64,
        });
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "v": 1,
            "pid": self.pid,
            "started_ms": self.started_ms,
            "seq": self.seq,
            "hb_ms": now_ms(),
            "state": self.state.as_str(),
            "phase": self.active.id,
            "phase_ms": self.active.since_ms,
            "label": self.active.label,
            "step": self.active.step.map(|(n, of)| json!({"n": n, "of": of})),
            "done": self.done.iter().map(|d| json!({"phase": d.phase, "label": d.label, "ms": d.ms})).collect::<Vec<_>>(),
            "error": self.error,
        })
    }

    /// Temp file + rename in the same dir; a failure is logged, never fatal.
    fn write(&self) {
        let body = self.json().to_string();
        let res =
            std::fs::write(&self.tmp, body).and_then(|()| std::fs::rename(&self.tmp, &self.path));
        if let Err(e) = res {
            tracing::debug!(path = %self.path.display(), error = %e, "write startup.json");
        }
    }
}

/// `sqlx` migrations that report `migrate:<db>` with one step per pending migration.
#[cfg(feature = "sqlx")]
pub mod migrate {
    use std::collections::HashSet;

    use sqlx::migrate::{Migrate, MigrateError, Migrator};
    use sqlx::{SqliteConnection, SqlitePool};

    /// `db` names the store (`agent`, `mesh`, …): phase `migrate:<db>`.
    fn scope(db: &str) -> super::Scope<'static> {
        super::scope(
            &format!("migrate:{db}"),
            &format!("Migrating the {db} database"),
        )
    }

    /// Exactly `migrator.run(pool)`, reported as phase `migrate:<db>` with step
    /// n/of over the pending migrations. Applies them one `run_to` at a time, then
    /// a final `run` — the same checks in the same order, so the same errors.
    pub async fn run(db: &str, migrator: &Migrator, pool: &SqlitePool) -> Result<(), MigrateError> {
        if !super::active() {
            return migrator.run(pool).await;
        }
        // The probe's connection goes back before `run_to` (a 1-connection pool).
        let pending = match pool.acquire().await {
            Ok(mut conn) => pending(migrator, &mut conn).await,
            Err(_) => Vec::new(),
        };
        if pending.is_empty() {
            return migrator.run(pool).await;
        }
        let _scope = scope(db);
        let of = pending.len() as u64;
        for (i, version) in pending.into_iter().enumerate() {
            super::step(i as u64 + 1, of);
            migrator.run_to(version, pool).await?;
        }
        migrator.run(pool).await
    }

    /// [`run`] on one connection: exactly `migrator.run_direct(None, conn, false)`.
    pub async fn run_direct(
        db: &str,
        migrator: &Migrator,
        conn: &mut SqliteConnection,
    ) -> Result<(), MigrateError> {
        if !super::active() {
            return migrator.run_direct(None, conn, false).await;
        }
        let pending = pending(migrator, conn).await;
        if pending.is_empty() {
            return migrator.run_direct(None, conn, false).await;
        }
        let _scope = scope(db);
        let of = pending.len() as u64;
        for (i, version) in pending.into_iter().enumerate() {
            super::step(i as u64 + 1, of);
            migrator.run_direct(Some(version), conn, false).await?;
        }
        migrator.run_direct(None, conn, false).await
    }

    /// Versions `run` would apply, in order; empty when the probe fails (the
    /// caller then runs unreported and gets `run`'s own error).
    async fn pending(migrator: &Migrator, conn: &mut SqliteConnection) -> Vec<i64> {
        let table = &*migrator.table_name;
        if conn.ensure_migrations_table(table).await.is_err() {
            return Vec::new();
        }
        if !matches!(conn.dirty_version(table).await, Ok(None)) {
            return Vec::new();
        }
        let Ok(applied) = conn.list_applied_migrations(table).await else {
            return Vec::new();
        };
        let applied: HashSet<i64> = applied.into_iter().map(|m| m.version).collect();
        migrator
            .iter()
            .filter(|m| !m.migration_type.is_down_migration() && !applied.contains(&m.version))
            .map(|m| m.version)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "platform-startup-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(d: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(d.join(FILE)).unwrap()).unwrap()
    }

    #[test]
    fn writes_atomically_without_leftovers() {
        let d = dir();
        let r = Reporter::new(&d, Instant::now(), "lock", "Lock");
        let v = read(&d);
        assert_eq!(v["v"], 1);
        assert_eq!(v["pid"], std::process::id());
        assert_eq!(v["state"], "running");
        assert_eq!(v["phase"], "lock");
        assert_eq!(
            v["phase_ms"], v["started_ms"],
            "the first phase starts the boot"
        );
        assert!(v["step"].is_null());
        assert!(v["error"].is_null());
        std::thread::sleep(Duration::from_millis(5));
        r.phase("config", "Reading config", Some((1, 2)));
        let phase_ms = read(&d)["phase_ms"].as_u64().unwrap();
        assert!(phase_ms > v["phase_ms"].as_u64().unwrap());
        r.step(2, 2);
        assert!(r.heartbeat());
        assert_eq!(
            read(&d)["phase_ms"],
            phase_ms,
            "steps and heartbeats keep phase_ms"
        );
        let names: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![FILE.to_owned()]);
        let v = read(&d);
        assert_eq!(v["step"], json!({"n": 2, "of": 2}));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn seq_moves_only_on_phase_or_step() {
        let d = dir();
        let r = Reporter::new(&d, Instant::now(), "lock", "Lock");
        assert_eq!(read(&d)["seq"], 0);
        let hb = read(&d)["hb_ms"].as_u64().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(r.heartbeat());
        let v = read(&d);
        assert_eq!(v["seq"], 0);
        assert!(v["hb_ms"].as_u64().unwrap() > hb);
        r.phase("plugin:agent", "Starting agent", Some((1, 3)));
        assert_eq!(read(&d)["seq"], 1);
        r.step(2, 3);
        assert_eq!(read(&d)["seq"], 2);
        r.step(2, 3);
        assert_eq!(read(&d)["seq"], 2, "an unchanged step is no progress");
        r.failed("boom");
        let v = read(&d);
        assert_eq!(v["seq"], 2);
        assert_eq!(v["state"], "failed");
        assert_eq!(v["error"], "boom");
        assert!(!r.heartbeat(), "the heartbeat stops at failed");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn done_records_durations_and_ready_is_final() {
        let d = dir();
        let r = Reporter::new(&d, Instant::now(), "lock", "Lock");
        std::thread::sleep(Duration::from_millis(30));
        r.phase("config", "Reading config", None);
        r.ready();
        let v = read(&d);
        assert_eq!(v["state"], "ready");
        assert_eq!(v["phase"], "ready");
        let done = v["done"].as_array().unwrap();
        assert_eq!(done[0]["phase"], "lock");
        assert_eq!(done[0]["label"], "Lock");
        assert!(done[0]["ms"].as_u64().unwrap() >= 30);
        assert_eq!(done[1]["phase"], "config");
        r.phase("plugin:late", "Late", None);
        r.failed("late");
        let after = read(&d);
        assert_eq!(after["seq"], v["seq"]);
        assert_eq!(after["state"], "ready");
        assert!(!r.heartbeat());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn done_is_capped() {
        let d = dir();
        let r = Reporter::new(&d, Instant::now(), "lock", "Lock");
        for i in 0..DONE_CAP + 10 {
            r.phase(&format!("p{i}"), "x", None);
        }
        let v = read(&d);
        let done = v["done"].as_array().unwrap();
        assert_eq!(done.len(), DONE_CAP);
        assert_eq!(done.last().unwrap()["phase"], format!("p{}", DONE_CAP + 8));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn scope_resumes_the_interrupted_phase() {
        let d = dir();
        let r = Reporter::new(&d, Instant::now(), "lock", "Lock");
        r.phase("plugin:agent", "Starting agent", Some((4, 9)));
        let parent_ms = read(&d)["phase_ms"].clone();
        {
            let _s = r.scope("migrate:agent", "Migrating agent.db");
            r.step(1, 2);
            let v = read(&d);
            assert_eq!(v["phase"], "migrate:agent");
            assert_eq!(v["step"], json!({"n": 1, "of": 2}));
        }
        let v = read(&d);
        assert_eq!(v["phase"], "plugin:agent");
        assert_eq!(v["step"], json!({"n": 4, "of": 9}));
        assert_eq!(
            v["phase_ms"], parent_ms,
            "the resumed phase keeps its start"
        );
        let done: Vec<_> = v["done"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["phase"].clone())
            .collect();
        assert_eq!(done, vec![json!("lock"), json!("migrate:agent")]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn free_functions_are_noops_uninitialised() {
        // Tests never call `init`: the global stays unset.
        phase("config", "Reading config", None);
        step(1, 2);
        drop(scope("migrate:x", "Migrating x.db"));
        ready();
        failed("x");
        assert!(!active());
        assert!(REPORTER.get().is_none());
    }
}
