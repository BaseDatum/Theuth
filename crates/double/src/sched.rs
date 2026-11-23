//! The training cycle (126 §8.1): one state machine per run —
//! `idle → eligible → waiting → acquiring → running → evaluating → promoted |
//! rejected → idle` — with the preconditions of [`crate::precond`] re-checked
//! every 60 s while waiting and at every step boundary while running (a
//! violation pauses the run and releases the build slot; it resumes when the
//! preconditions hold again), a run cap of `training.run_cap_min` minutes of
//! `running` per night (cumulative per run; the run sleeps to the next night),
//! `rejected: stale` after three nights, and exactly one run at a time.
//! Transitions are `runs` rows and `DoubleTrainingState` events.
//!
//! Triggers (the 60 s tick): Tier A after every 25 new labels, debounced
//! (the count must stop moving for one tick), and nightly at the window's
//! start; Tier B when `training.triggers` fire and a Tier B [`Trainer`] exists
//! (W3's hook: [`Scheduler::tier_b`]), Tier C never. `double_train` runs the
//! same path by hand.
//!
//! The [`Scheduler`] actor (registered as [`ACTOR`]) also answers the W2 RPC
//! methods: `double_train`, `double_rollback`, `double_export`,
//! `double_delete_all`, `double_settings_set`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use kameo::actor::{ActorRef, Spawn as _};
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use runtime::Runtime;
use tokio::time::Duration;

use crate::config::{DoubleConfig, Training};
use crate::events::{DoubleTrainingState, DoubleVersionActivated};
use crate::precond::{self, Blocker, BuildSlot, Sensors, TRAIN_LABEL, Window};
use crate::store::{RunRow, Store, VersionRow};
use crate::types::{DoubleExport, DoubleRun, DoubleStatus, RunId, RunState, Tier, Venue};
use crate::venue::{self, Resolved};
use crate::versions;

/// Registry name of the scheduler actor.
pub const ACTOR: &str = "double-sched";
/// Tier A fires after this many new labels (§7.2).
pub const TIER_A_LABELS: u32 = 25;
/// Precondition / slot retry period.
pub const RETRY: Duration = Duration::from_secs(60);
/// A run that has not finished after this many nights is `rejected: stale`.
pub const MAX_NIGHTS: u32 = 3;

/// What one run produced; the trainer wrote the version directory already.
#[derive(Clone, Debug)]
pub struct Finished {
    /// The `versions` row to insert (`active` ignored; the gate decides).
    pub version: VersionRow,
    /// `Ok` → activate.
    pub verdict: Result<(), String>,
}

/// One tier's training procedure, cut into steps the scheduler can pause
/// between. Tier A is one step; Tier B's steps are resume budgets
/// (`crate::tier_b`).
#[async_trait::async_trait]
pub trait Trainer: Send {
    /// How many steps; the progress denominator.
    fn steps(&self) -> u32;
    /// Called once before step 0 with the `runs.id` and venue string so a
    /// trainer can publish its own `DoubleTrainingState` progress.
    fn bind(&mut self, _run_id: &str, _venue: &str) {}
    /// Run step `i` (0-based). `Ok(Some(_))` ends the run (any step);
    /// `Ok(None)` = not done yet — or, on the last step, nothing to train on.
    async fn step(&mut self, i: u32) -> anyhow::Result<Option<Finished>>;
}

/// `(tier, venue) → a trainer`, or `None` when the tier is not available.
pub type Factory = dyn Fn(Tier, &Resolved) -> Option<Box<dyn Trainer>> + Send + Sync;

/// Everything a run needs.
#[derive(Clone)]
pub struct Deps {
    /// For the bus.
    pub rt: Runtime,
    /// The store.
    pub store: Store,
    /// `<data_dir>/double/models`.
    pub models: PathBuf,
    /// Live training settings (replaced by `double_settings_set`).
    pub training: Arc<Mutex<Training>>,
    /// The sensors.
    pub sensors: Arc<dyn Sensors>,
    /// Build slot directory.
    pub slots: PathBuf,
}

impl Deps {
    /// Publish one transition (and persist the row).
    async fn transition(
        &self,
        run: &mut RunRow,
        state: RunState,
        progress: Option<f32>,
        message: impl Into<String>,
    ) -> anyhow::Result<()> {
        run.state = state;
        let message = message.into();
        if matches!(state, RunState::Promoted | RunState::Rejected) {
            run.ended_at = Some(Utc::now());
            run.result = Some(message.clone());
        }
        self.store.put_run(run).await?;
        self.rt
            .publish(DoubleTrainingState {
                run_id: run.id.clone(),
                tier: run.tier,
                venue: run.venue.clone(),
                state,
                progress,
                message,
            })
            .await;
        Ok(())
    }

    fn training(&self) -> Training {
        self.training.lock().clone()
    }
}

/// Drive one run from `eligible` to `promoted` / `rejected`. Returns the
/// terminal state; errors are recorded as `rejected: <error>`.
pub async fn run_one(d: &Deps, run: &mut RunRow, trainer: &mut dyn Trainer) -> RunState {
    match drive(d, run, trainer).await {
        Ok(state) => state,
        Err(e) => {
            let _ = d
                .transition(run, RunState::Rejected, None, format!("rejected: {e:#}"))
                .await;
            RunState::Rejected
        }
    }
}

async fn drive(d: &Deps, run: &mut RunRow, trainer: &mut dyn Trainer) -> anyhow::Result<RunState> {
    d.transition(run, RunState::Eligible, None, "trigger met")
        .await?;
    trainer.bind(&run.id, &run.venue);
    wait_for_preconditions(d, run).await?;

    let needs_slot = run.tier != Tier::A;
    let steps = trainer.steps().max(1);
    let cap = Duration::from_secs(u64::from(d.training().run_cap_min) * 60);
    let mut slot: Option<BuildSlot> = None;
    let mut tonight: Option<NaiveDate> = None;
    let mut spent = Duration::ZERO;
    let mut nights = 0u32;
    let mut finished = None;

    for i in 0..steps {
        // Acquire (or re-acquire after a pause) the slot.
        if needs_slot && slot.is_none() {
            d.transition(run, RunState::Acquiring, None, "taking the build slot")
                .await?;
            loop {
                match BuildSlot::try_acquire(
                    &d.slots,
                    TRAIN_LABEL,
                    &format!("tier {} {}", run.tier.as_str(), run.id),
                )? {
                    Some(s) => {
                        slot = Some(s);
                        break;
                    }
                    None => {
                        d.transition(run, RunState::Waiting, None, "build slot held")
                            .await?;
                        tokio::time::sleep(RETRY).await;
                        wait_for_preconditions(d, run).await?;
                    }
                }
            }
        }

        // The night and its cap.
        let night = current_night(d);
        if tonight != Some(night) {
            tonight = Some(night);
            spent = Duration::ZERO;
            nights += 1;
            if nights > MAX_NIGHTS {
                return reject(d, run, "rejected: stale").await;
            }
        }

        let progress = i as f32 / steps as f32;
        d.transition(
            run,
            RunState::Running,
            Some(progress),
            format!("step {}/{steps}", i + 1),
        )
        .await?;
        let started = tokio::time::Instant::now();
        finished = trainer.step(i).await?;
        spent += started.elapsed();

        if finished.is_some() || i + 1 == steps {
            break;
        }
        // Step boundary: cap, then preconditions.
        if spent >= cap {
            slot = None; // release
            d.transition(
                run,
                RunState::Paused,
                Some((i + 1) as f32 / steps as f32),
                format!(
                    "run cap {} min reached; resumes next night",
                    cap.as_secs() / 60
                ),
            )
            .await?;
            while current_night(d) == night {
                tokio::time::sleep(RETRY).await;
            }
            wait_for_preconditions(d, run).await?;
            continue;
        }
        let blockers = precond::check(run.tier, &d.training(), d.sensors.as_ref()).await;
        if !blockers.is_empty() {
            slot = None;
            d.transition(
                run,
                RunState::Paused,
                Some((i + 1) as f32 / steps as f32),
                format!("paused: {}", describe(&blockers)),
            )
            .await?;
            tokio::time::sleep(RETRY).await;
            wait_for_preconditions(d, run).await?;
        }
    }
    drop(slot);

    d.transition(
        run,
        RunState::Evaluating,
        Some(1.0),
        "scoring the candidate",
    )
    .await?;
    let Some(Finished { version, verdict }) = finished else {
        return reject(d, run, "rejected: nothing new to train on").await;
    };
    let mut row = version;
    row.active = false;
    row.venue = run.venue.clone();
    d.store.put_run(run).await?;
    d.store.put_version(&row).await?;
    match verdict {
        Ok(()) => {
            let previous = versions::activate(&d.store, &d.models, &row.id).await?;
            d.rt.publish(DoubleVersionActivated {
                version: row.id.clone(),
                previous,
                metrics: row.metrics.clone(),
            })
            .await;
            d.transition(
                run,
                RunState::Promoted,
                Some(1.0),
                format!("promoted {}", row.id),
            )
            .await?;
            Ok(RunState::Promoted)
        }
        Err(why) => reject(d, run, why).await,
    }
}

async fn reject(d: &Deps, run: &mut RunRow, why: impl Into<String>) -> anyhow::Result<RunState> {
    d.transition(run, RunState::Rejected, None, why).await?;
    Ok(RunState::Rejected)
}

/// `waiting` until every precondition holds (re-checked every [`RETRY`]).
async fn wait_for_preconditions(d: &Deps, run: &mut RunRow) -> anyhow::Result<()> {
    loop {
        let blockers = precond::check(run.tier, &d.training(), d.sensors.as_ref()).await;
        if blockers.is_empty() {
            return Ok(());
        }
        if run.state != RunState::Waiting {
            d.transition(
                run,
                RunState::Waiting,
                None,
                format!("waiting: {}", describe(&blockers)),
            )
            .await?;
        }
        tokio::time::sleep(RETRY).await;
    }
}

fn describe(b: &[Blocker]) -> String {
    b.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn window(cfg: &Training) -> Window {
    Window::parse(&cfg.window)
        .unwrap_or_else(|| Window::parse(&Training::default().window).expect("default parses"))
}

fn current_night(d: &Deps) -> NaiveDate {
    let now = d.sensors.now_local();
    window(&d.training()).night_of(now.date(), now.time())
}

// --- triggers ------------------------------------------------------------

/// Labels (any source but `excluded`) landed after `since`.
pub async fn labels_since(store: &Store, since: Option<DateTime<Utc>>) -> anyhow::Result<u32> {
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM labels WHERE source != 'excluded' AND at > ?")
            .bind(since.map(|t| t.to_rfc3339()).unwrap_or_default())
            .fetch_one(store.pool())
            .await?;
    Ok(n as u32)
}

/// The end of the newest version's `data_range` for `tier`, when one exists.
pub async fn last_data_end(store: &Store, tier: Tier) -> anyhow::Result<Option<DateTime<Utc>>> {
    Ok(store
        .versions()
        .await?
        .into_iter()
        .filter(|v| v.tier == tier)
        .filter_map(|v| {
            v.data_range
                .rsplit_once("..")
                .and_then(|(_, b)| DateTime::parse_from_rfc3339(b).ok())
                .map(|t| t.with_timezone(&Utc))
        })
        .max())
}

/// The trigger state the 60 s tick carries between ticks.
#[derive(Debug, Default)]
pub struct Triggers {
    /// Last tick's Tier A label count (debounce: fire when it stops moving).
    pub last_a_count: Option<u32>,
    /// The night Tier A last ran nightly.
    pub nightly_a: Option<NaiveDate>,
}

impl Triggers {
    /// Which tier, if any, should run now.
    pub async fn due(&mut self, d: &Deps, has_tier_b: bool) -> anyhow::Result<Option<Tier>> {
        let cfg = d.training();
        let now = d.sensors.now_local();
        let w = window(&cfg);
        if cfg.tiers.a.enabled {
            let n = labels_since(&d.store, last_data_end(&d.store, Tier::A).await?).await?;
            let settled = self.last_a_count == Some(n);
            self.last_a_count = Some(n);
            if n >= TIER_A_LABELS && settled {
                return Ok(Some(Tier::A));
            }
            let night = w.night_of(now.date(), now.time());
            if n > 0 && w.contains(now.time()) && self.nightly_a != Some(night) {
                self.nightly_a = Some(night);
                return Ok(Some(Tier::A));
            }
        }
        if has_tier_b && cfg.tiers.b.enabled && w.contains(now.time()) {
            let end = last_data_end(&d.store, Tier::B).await?;
            let n = labels_since(&d.store, end).await?;
            let days = end.map_or(i64::MAX, |e| (Utc::now() - e).num_days());
            if n >= cfg.triggers.labels || days >= i64::from(cfg.triggers.days) {
                return Ok(Some(Tier::B));
            }
        }
        Ok(None)
    }
}

// --- the actor -----------------------------------------------------------

/// `ask` → `Result<RunId, String>`: `double_train(tier, venue)`.
#[derive(Clone, Debug)]
pub struct Train {
    /// Which tier.
    pub tier: Tier,
    /// Requested venue; `Auto` defers to the config.
    pub venue: Venue,
}

/// `ask` → `Result<DoubleStatus, String>`: `double_rollback(version)`.
#[derive(Clone, Debug)]
pub struct Rollback(pub String);

/// `ask` → `Result<DoubleExport, String>`: `double_export()`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Export;

/// `ask` → `Result<(), String>`: `double_delete_all()`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeleteAll;

/// `ask` → `Result<DoubleConfig, String>`: `double_settings_set(settings)`.
#[derive(Clone, Debug)]
pub struct SetSettings(pub DoubleConfig);

/// `ask` → `Result<DoubleStatus, String>`: what the scheduler knows (W1's actor fills the KPIs).
#[derive(Clone, Copy, Debug, Default)]
pub struct Status;

/// Published on the bus after `double_settings_set` persisted; W1's actor
/// subscribes for the live `capture` / `ladder` semantics.
#[derive(Clone, Debug)]
pub struct SettingsChanged(pub DoubleConfig);

/// The scheduler.
#[derive(kameo::Actor)]
pub struct Scheduler {
    deps: Deps,
    /// The whole config as last set (for `SettingsChanged` and the status).
    config: Arc<Mutex<DoubleConfig>>,
    factory: Arc<Factory>,
    /// The run in flight, if any.
    current: Arc<Mutex<Option<RunRow>>>,
    /// `<data_dir>/config.yaml`.
    config_path: PathBuf,
}

impl Scheduler {
    /// Whether a Tier B trainer is wired (W3).
    fn has_tier_b(&self) -> bool {
        (self.factory)(Tier::B, &Resolved::Local).is_some()
    }

    /// Start `tier` on `venue` now: one run at a time.
    async fn start_run(&self, tier: Tier, requested: Venue) -> Result<RunId, String> {
        if let Some(r) = self.current.lock().as_ref() {
            return Err(format!(
                "a run is in progress: {} ({})",
                r.id,
                r.state.as_str()
            ));
        }
        let cfg = self.deps.training();
        let resolved = venue::resolve(&requested, &cfg.venue, cfg.share_data).await;
        let resolved = match (tier, resolved) {
            (_, Resolved::Off) => return Err("training venue is off".into()),
            (_, Resolved::Unreachable(name)) => {
                return Err(format!("node {name:?} is not reachable"));
            }
            // Tier A is < 2 min of CPU: always local.
            (Tier::A, _) => Resolved::Local,
            (_, r) => r,
        };
        if tier == Tier::B
            && resolved == Resolved::Local
            && !crate::train_b::TierB::local_allowed(&self.config.lock().clone())
        {
            return Err(
                "local tier b is off: set double.training.tiers.b.local: true, or add a GPU node to the mesh"
                    .into(),
            );
        }
        let Some(mut trainer) = (self.factory)(tier, &resolved) else {
            return Err(format!(
                "tier {} is not available on venue {resolved}",
                tier.as_str()
            ));
        };
        let mut run = RunRow {
            id: uuid::Uuid::now_v7().to_string(),
            tier,
            venue: resolved.to_string(),
            state: RunState::Idle,
            started_at: Utc::now(),
            ended_at: None,
            result: None,
            log_path: None,
        };
        let id = RunId(run.id.clone());
        *self.current.lock() = Some(run.clone());
        let (deps, current) = (self.deps.clone(), self.current.clone());
        tokio::spawn(async move {
            run_one(&deps, &mut run, trainer.as_mut()).await;
            *current.lock() = None;
        });
        Ok(id)
    }

    async fn status(&self) -> DoubleStatus {
        let cfg = self.config.lock().clone();
        let active = self.deps.store.active_version().await.ok().flatten();
        let last = self
            .deps
            .store
            .runs(1)
            .await
            .ok()
            .and_then(|v| v.into_iter().next());
        let training = self.deps.training();
        let now = self.deps.sensors.now_local();
        DoubleStatus {
            enabled: cfg.enabled,
            paused: false,
            active_version: active.map(|v| v.id),
            last_run: last.map(|r| DoubleRun {
                id: RunId(r.id),
                tier: r.tier,
                venue: r.venue,
                state: r.state,
                started_at: r.started_at.to_rfc3339(),
                ended_at: r.ended_at.map(|t| t.to_rfc3339()),
                result: r.result,
            }),
            next_window: Some(next_window(&window(&training), now).to_string()),
            venue: venue::resolve(&Venue::Auto, &training.venue, training.share_data)
                .await
                .to_string(),
            ..DoubleStatus::default()
        }
    }
}

/// The next window start at or after `now`, local.
pub fn next_window(w: &Window, now: chrono::NaiveDateTime) -> chrono::NaiveDateTime {
    let today = now.date().and_time(w.start);
    if today > now {
        today
    } else {
        today + ChronoDuration::days(1)
    }
}

impl Message<Train> for Scheduler {
    type Reply = Result<RunId, String>;

    async fn handle(&mut self, m: Train, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.start_run(m.tier, m.venue).await
    }
}

impl Message<Rollback> for Scheduler {
    type Reply = Result<DoubleStatus, String>;

    async fn handle(&mut self, m: Rollback, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (previous, metrics) = versions::rollback(&self.deps.store, &self.deps.models, &m.0)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.deps
            .rt
            .publish(DoubleVersionActivated {
                version: m.0,
                previous,
                metrics,
            })
            .await;
        Ok(self.status().await)
    }
}

impl Message<Export> for Scheduler {
    type Reply = Result<DoubleExport, String>;

    async fn handle(&mut self, _: Export, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        export(&self.deps.store, &self.deps.rt.data_dir)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<DeleteAll> for Scheduler {
    type Reply = Result<(), String>;

    async fn handle(&mut self, _: DeleteAll, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(r) = self.current.lock().as_ref() {
            return Err(format!("a run is in progress: {}", r.id));
        }
        delete_all(&self.deps.store, &self.deps.rt.data_dir)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

impl Message<SetSettings> for Scheduler {
    type Reply = Result<DoubleConfig, String>;

    async fn handle(&mut self, m: SetSettings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        persist(&self.config_path, &m.0).map_err(|e| format!("config.yaml: {e}"))?;
        *self.deps.training.lock() = m.0.training.clone();
        *self.config.lock() = m.0.clone();
        self.deps.rt.publish(SettingsChanged(m.0.clone())).await;
        Ok(m.0)
    }
}

impl Message<Status> for Scheduler {
    type Reply = Result<DoubleStatus, String>;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(self.status().await)
    }
}

/// Spawn the scheduler: the actor, registered as [`ACTOR`], and the 60 s
/// trigger tick. `factory` supplies trainers; [`tier_a_factory`] is the W2
/// one (Tier A only; W3 wraps it to add Tier B).
pub fn start(
    rt: Runtime,
    store: Store,
    cfg: DoubleConfig,
    factory: Arc<Factory>,
) -> anyhow::Result<ActorRef<Scheduler>> {
    let models = versions::models_dir(&rt.data_dir);
    let deps = Deps {
        store,
        models,
        training: Arc::new(Mutex::new(cfg.training.clone())),
        sensors: Arc::new(precond::Live {
            data_dir: rt.data_dir.clone(),
            slots: PathBuf::from(precond::SLOT_DIR),
        }),
        slots: PathBuf::from(precond::SLOT_DIR),
        rt: rt.clone(),
    };
    let actor = Scheduler::spawn_with_mailbox(
        Scheduler {
            deps: deps.clone(),
            config: Arc::new(Mutex::new(cfg)),
            factory,
            current: Arc::new(Mutex::new(None)),
            config_path: rt.data_dir.join("config.yaml"),
        },
        runtime::subscriber_mailbox(),
    );
    actor.register(ACTOR)?;
    let tick = actor.clone();
    tokio::spawn(async move {
        let mut triggers = Triggers::default();
        loop {
            tokio::select! {
                () = rt.shutdown.cancelled() => return,
                () = tokio::time::sleep(RETRY) => {}
            }
            let has_b = tick.ask(HasTierB).await.unwrap_or(false);
            match triggers.due(&deps, has_b).await {
                Ok(Some(tier)) => {
                    if let Err(e) = tick
                        .ask(Train {
                            tier,
                            venue: Venue::Auto,
                        })
                        .await
                    {
                        tracing::debug!("double: trigger for tier {} skipped: {e}", tier.as_str());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("double: trigger check: {e:#}"),
            }
        }
    });
    Ok(actor)
}

/// `ask` → `bool` (internal: the tick asks whether W3 wired Tier B).
#[derive(Clone, Copy, Debug, Default)]
pub struct HasTierB;

impl Message<HasTierB> for Scheduler {
    type Reply = bool;

    async fn handle(&mut self, _: HasTierB, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.has_tier_b()
    }
}

// --- Tier A trainer ------------------------------------------------------

/// The one-step Tier A trainer over the store.
pub struct TierA {
    store: Store,
    models: PathBuf,
    base_hash: String,
}

#[async_trait::async_trait]
impl Trainer for TierA {
    fn steps(&self) -> u32 {
        1
    }

    async fn step(&mut self, _: u32) -> anyhow::Result<Option<Finished>> {
        let active = versions::load_active(&self.store, &self.models).await?;
        let now = Utc::now();
        let Some(c) =
            crate::train_a::run(&self.store, active.artifacts.as_ref(), &self.base_hash, now)
                .await?
        else {
            return Ok(None);
        };
        let manifest = versions::Manifest {
            version: c.version.id.clone(),
            tier: Tier::A,
            base_hash: c.version.base_hash.clone(),
            data_range: c.version.data_range.clone(),
            venue: "local".into(),
            metrics: c.version.metrics.clone(),
            trained_at: now.to_rfc3339(),
            duration_s: c.version.duration_s,
            seed: None,
        };
        let paths = versions::write(&self.models, &manifest, &c.artifacts)?;
        let mut version = c.version;
        version.head_path = Some(paths.heads.display().to_string());
        version.knn_path = Some(paths.knn.display().to_string());
        Ok(Some(Finished {
            version,
            verdict: c.verdict,
        }))
    }
}

/// The W2 factory: Tier A locally; `None` for B and C (W3 wraps this).
/// `base_hash` names the checkpoint the embeddings came from.
pub fn tier_a_factory(store: Store, models: PathBuf, base_hash: String) -> Arc<Factory> {
    Arc::new(move |tier, venue| match (tier, venue) {
        (Tier::A, Resolved::Local) => Some(Box::new(TierA {
            store: store.clone(),
            models: models.clone(),
            base_hash: base_hash.clone(),
        }) as Box<dyn Trainer>),
        _ => None,
    })
}

// --- export / delete / settings -----------------------------------------

/// `double_export()`: one JSON object per example — the scrubbed card, its
/// options, labels and embedding — under `<data_dir>/double/export/`.
pub async fn export(store: &Store, data_dir: &Path) -> anyhow::Result<DoubleExport> {
    use sqlx::Row as _;
    use std::io::Write as _;
    let dir = data_dir.join(crate::store::DIR).join("export");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "double-{}.jsonl",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let rows = sqlx::query(
        "SELECT id, class, question_id, requester, card_text, options, embedding, asked_at
         FROM examples ORDER BY asked_at, id",
    )
    .fetch_all(store.pool())
    .await?;
    let mut n = 0u32;
    for r in rows {
        let id: String = r.get("id");
        let labels: Vec<serde_json::Value> = store
            .labels(&id)
            .await?
            .into_iter()
            .map(|l| {
                serde_json::json!({
                    "source": l.source.as_str(), "choice": l.choice, "weight": l.weight,
                    "at": l.at.to_rfc3339(),
                })
            })
            .collect();
        let options: serde_json::Value =
            serde_json::from_str(r.get::<String, _>("options").as_str()).unwrap_or_default();
        let embedding = r
            .get::<Option<Vec<u8>>, _>("embedding")
            .map(|b| crate::store::decode_f16(&b));
        let row = serde_json::json!({
            "id": id,
            "class": r.get::<String, _>("class"),
            "question_id": r.get::<String, _>("question_id"),
            "requester": r.get::<String, _>("requester"),
            "card_text": r.get::<String, _>("card_text"),
            "options": options,
            "asked_at": r.get::<String, _>("asked_at"),
            "labels": labels,
            "embedding": embedding,
        });
        serde_json::to_writer(&mut f, &row)?;
        f.write_all(b"\n")?;
        n += 1;
    }
    f.flush()?;
    let bytes = std::fs::metadata(&path)?.len();
    Ok(DoubleExport {
        path: path.display().to_string(),
        examples: n,
        bytes,
    })
}

/// `double_delete_all()`: every row but the `baseline` version, and the
/// `models/`, `runs/`, `export/` directories. Config untouched.
pub async fn delete_all(store: &Store, data_dir: &Path) -> anyhow::Result<()> {
    let mut tx = store.pool().begin().await?;
    for sql in [
        "DELETE FROM examples",
        "DELETE FROM classes",
        "DELETE FROM runs",
        "DELETE FROM versions WHERE id != 'baseline'",
        "UPDATE versions SET active = 0",
    ] {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let root = data_dir.join(crate::store::DIR);
    for sub in ["models", "runs", "export"] {
        let p = root.join(sub);
        if p.exists() {
            std::fs::remove_dir_all(&p)?;
        }
    }
    Ok(())
}

/// Persist the whole `double:` section of `config.yaml`: the block is
/// replaced by `cfg` serialised (comments inside the block are lost; the
/// rest of the file is byte-identical), through a sibling temp file and a
/// rename. `runtime::write_config_value` handles one scalar per call and
/// would leave a block's children orphaned under a flow value, so the block
/// is rewritten whole.
pub fn persist(path: &Path, cfg: &DoubleConfig) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let body = serde_yaml::to_string(cfg).map_err(std::io::Error::other)?;
    let mut block: Vec<String> = vec!["double:".to_owned()];
    block.extend(body.lines().map(|l| format!("  {l}")));
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    match lines.iter().position(|l| l.trim_end() == "double:") {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.starts_with(|c: char| !c.is_whitespace()))
                .map_or(lines.len(), |i| start + 1 + i);
            lines.splice(start..end, block);
        }
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
    platform::files::write_atomic(path, (lines.join("\n") + "\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::precond::fake::{Fake, at};
    use crate::types::Tier;
    use parking_lot::Mutex as PMutex;
    use platform::power::Power;

    struct Harness {
        _dir: tempfile::TempDir,
        deps: Deps,
        fake: Arc<Fake>,
        events: Arc<PMutex<Vec<DoubleTrainingState>>>,
    }

    /// Collects `DoubleTrainingState` off the bus.
    #[derive(kameo::Actor)]
    struct Tap(Arc<PMutex<Vec<DoubleTrainingState>>>);

    impl Message<DoubleTrainingState> for Tap {
        type Reply = ();
        async fn handle(&mut self, m: DoubleTrainingState, _: &mut Context<Self, Self::Reply>) {
            self.0.lock().push(m);
        }
    }

    async fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let store = Store::open(&crate::store::path(dir.path())).await.unwrap();
        store
            .put_class(&crate::store::ClassRow {
                key: "c".into(),
                kind: crate::class::CardKind::Approval,
                rung: 0,
                cap: 2,
                mutating_opt_in: false,
                n: 0,
                stats: serde_json::json!({}),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let fake = Arc::new(Fake::green());
        let events = Arc::new(PMutex::new(Vec::new()));
        let tap = Tap::spawn_with_mailbox(Tap(events.clone()), runtime::subscriber_mailbox());
        rt.subscribe::<Tap, DoubleTrainingState>(&tap).await;
        let deps = Deps {
            rt,
            store,
            models: dir.path().join("double/models"),
            training: Arc::new(Mutex::new(Training::default())),
            sensors: fake.clone(),
            slots: dir.path().join("slots"),
        };
        Harness {
            _dir: dir,
            deps,
            fake,
            events,
        }
    }

    fn run_row(tier: Tier) -> RunRow {
        RunRow {
            id: "r1".into(),
            tier,
            venue: "local".into(),
            state: RunState::Idle,
            started_at: Utc::now(),
            ended_at: None,
            result: None,
            log_path: None,
        }
    }

    /// `steps` steps of `step_secs` (fake time) each; the last yields a version.
    struct Steps {
        steps: u32,
        step_secs: u64,
        verdict: Result<(), String>,
        ran: Arc<PMutex<Vec<u32>>>,
        models: PathBuf,
    }

    #[async_trait::async_trait]
    impl Trainer for Steps {
        fn steps(&self) -> u32 {
            self.steps
        }
        async fn step(&mut self, i: u32) -> anyhow::Result<Option<Finished>> {
            self.ran.lock().push(i);
            tokio::time::sleep(Duration::from_secs(self.step_secs)).await;
            if i + 1 < self.steps {
                return Ok(None);
            }
            let id = format!("v-{}", self.steps);
            versions::write(
                &self.models,
                &versions::Manifest {
                    version: id.clone(),
                    tier: Tier::B,
                    base_hash: "h".into(),
                    data_range: "a..b".into(),
                    venue: "local".into(),
                    metrics: serde_json::json!({}),
                    trained_at: "t".into(),
                    duration_s: 0,
                    seed: None,
                },
                &crate::train_a::Artifacts::empty(),
            )?;
            Ok(Some(Finished {
                version: VersionRow {
                    id,
                    tier: Tier::B,
                    base_hash: "h".into(),
                    adapter_path: None,
                    head_path: None,
                    knn_path: None,
                    metrics: serde_json::json!({"ok": true}),
                    data_range: "a..b".into(),
                    trained_at: Utc::now(),
                    venue: "local".into(),
                    duration_s: 0,
                    active: false,
                },
                verdict: self.verdict.clone(),
            }))
        }
    }

    fn states(h: &Harness) -> Vec<RunState> {
        h.events.lock().iter().map(|e| e.state).collect()
    }

    /// Until the `n`-th `state` event has been published (fake-time polling).
    async fn seen(ev: &PMutex<Vec<DoubleTrainingState>>, state: RunState, n: usize) {
        loop {
            if ev.lock().iter().filter(|e| e.state == state).count() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn settle() {
        // Let the bus deliver.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn promoted_path_and_rejected_path() {
        let h = harness().await;
        tokio::time::pause();
        let ran = Arc::new(PMutex::new(Vec::new()));
        let mut t = Steps {
            steps: 3,
            step_secs: 10,
            verdict: Ok(()),
            ran: ran.clone(),
            models: h.deps.models.clone(),
        };
        let mut run = run_row(Tier::B);
        assert_eq!(run_one(&h.deps, &mut run, &mut t).await, RunState::Promoted);
        settle().await;
        assert_eq!(*ran.lock(), vec![0, 1, 2]);
        assert_eq!(
            states(&h),
            vec![
                RunState::Eligible,
                RunState::Acquiring,
                RunState::Running,
                RunState::Running,
                RunState::Running,
                RunState::Evaluating,
                RunState::Promoted
            ]
        );
        assert_eq!(
            h.deps.store.active_version().await.unwrap().unwrap().id,
            "v-3"
        );
        assert_eq!(versions::pointer(&h.deps.models).as_deref(), Some("v-3"));
        assert!(!h.deps.slots.join("0").exists(), "slot released");
        let row = h.deps.store.runs(1).await.unwrap().remove(0);
        assert_eq!(row.state, RunState::Promoted);
        assert!(row.ended_at.is_some());

        h.events.lock().clear();
        let mut t = Steps {
            steps: 1,
            step_secs: 1,
            verdict: Err("rejected: golden".into()),
            ran: ran.clone(),
            models: h.deps.models.clone(),
        };
        let mut run = run_row(Tier::B);
        run.id = "r2".into();
        assert_eq!(run_one(&h.deps, &mut run, &mut t).await, RunState::Rejected);
        settle().await;
        assert_eq!(h.events.lock().last().unwrap().message, "rejected: golden");
        // Kept inactive with metrics; the active pointer is untouched.
        let versions = h.deps.store.versions().await.unwrap();
        assert!(versions.iter().any(|v| v.id == "v-1" && !v.active));
        assert_eq!(
            h.deps.store.active_version().await.unwrap().unwrap().id,
            "v-3"
        );
    }

    #[tokio::test]
    async fn waits_pauses_and_resumes_on_preconditions() {
        let h = harness().await;
        tokio::time::pause();
        let ran = Arc::new(PMutex::new(Vec::new()));
        *h.fake.power.lock() = Power::Battery;
        let mut t = Steps {
            steps: 2,
            step_secs: 5,
            verdict: Ok(()),
            ran: ran.clone(),
            models: h.deps.models.clone(),
        };
        let (fake, ev) = (h.fake.clone(), h.events.clone());
        let flip = tokio::spawn(async move {
            // Unplugged until the run is seen waiting; then a meeting starts
            // during step 0 and ends once the run has paused for it.
            seen(&ev, RunState::Waiting, 1).await;
            *fake.power.lock() = Power::Ac;
            seen(&ev, RunState::Running, 1).await;
            *fake.live.lock() = Some("meeting");
            seen(&ev, RunState::Paused, 1).await;
            *fake.live.lock() = None;
        });
        let mut run = run_row(Tier::B);
        assert_eq!(run_one(&h.deps, &mut run, &mut t).await, RunState::Promoted);
        flip.await.unwrap();
        settle().await;
        let s = states(&h);
        assert_eq!(s[0], RunState::Eligible);
        assert_eq!(s[1], RunState::Waiting, "{s:?}");
        assert!(s.contains(&RunState::Paused), "{s:?}");
        let paused_at = s.iter().position(|x| *x == RunState::Paused).unwrap();
        // Resumes through the acquire path (the slot was released at the pause).
        assert!(
            matches!(s[paused_at + 1], RunState::Waiting | RunState::Acquiring),
            "{s:?}"
        );
        assert!(s[paused_at + 1..].contains(&RunState::Running), "{s:?}");
        assert_eq!(*s.last().unwrap(), RunState::Promoted);
        let msgs: Vec<String> = h.events.lock().iter().map(|e| e.message.clone()).collect();
        assert!(msgs.iter().any(|m| m.contains("on battery")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("meeting live")), "{msgs:?}");
    }

    #[tokio::test]
    async fn run_cap_pauses_until_the_next_night_and_three_nights_is_stale() {
        let h = harness().await;
        tokio::time::pause();
        h.deps.training.lock().run_cap_min = 1;
        let ran = Arc::new(PMutex::new(Vec::new()));
        let mut t = Steps {
            steps: 4,
            step_secs: 61,
            verdict: Ok(()),
            ran: ran.clone(),
            models: h.deps.models.clone(),
        };
        // The fake wall clock moves a day forward every time the cap pauses the run.
        let (fake, ev) = (h.fake.clone(), h.events.clone());
        let clock = tokio::spawn(async move {
            let mut n = 1;
            loop {
                seen(&ev, RunState::Paused, n).await;
                let next = *fake.now.lock() + ChronoDuration::days(1);
                *fake.now.lock() = next;
                n += 1;
            }
        });
        let mut run = run_row(Tier::B);
        assert_eq!(run_one(&h.deps, &mut run, &mut t).await, RunState::Rejected);
        clock.abort();
        settle().await;
        // One step per night; the fourth night never comes.
        assert_eq!(*ran.lock(), vec![0, 1, 2]);
        let s = states(&h);
        assert_eq!(
            s.iter().filter(|x| **x == RunState::Paused).count(),
            3,
            "{s:?}"
        );
        assert_eq!(h.events.lock().last().unwrap().message, "rejected: stale");
        assert_eq!(*s.last().unwrap(), RunState::Rejected);
        *h.fake.now.lock() = at(3, 0);
    }

    #[tokio::test]
    async fn tier_a_takes_no_slot_but_waits_on_a_foreign_one() {
        let h = harness().await;
        tokio::time::pause();
        h.fake.foreign.lock().push("126-W1".into());
        let ran = Arc::new(PMutex::new(Vec::new()));
        let mut t = Steps {
            steps: 1,
            step_secs: 1,
            verdict: Ok(()),
            ran: ran.clone(),
            models: h.deps.models.clone(),
        };
        let (fake, ev) = (h.fake.clone(), h.events.clone());
        tokio::spawn(async move {
            seen(&ev, RunState::Waiting, 1).await;
            fake.foreign.lock().clear();
        });
        let mut run = run_row(Tier::A);
        assert_eq!(run_one(&h.deps, &mut run, &mut t).await, RunState::Promoted);
        settle().await;
        let s = states(&h);
        assert!(!s.contains(&RunState::Acquiring), "{s:?}");
        assert_eq!(s[1], RunState::Waiting);
        assert!(!h.deps.slots.exists());
    }

    #[tokio::test]
    async fn nothing_to_train_is_rejected_cleanly() {
        let h = harness().await;
        tokio::time::pause();
        struct Nothing;
        #[async_trait::async_trait]
        impl Trainer for Nothing {
            fn steps(&self) -> u32 {
                1
            }
            async fn step(&mut self, _: u32) -> anyhow::Result<Option<Finished>> {
                Ok(None)
            }
        }
        let mut run = run_row(Tier::A);
        assert_eq!(
            run_one(&h.deps, &mut run, &mut Nothing).await,
            RunState::Rejected
        );
        settle().await;
        assert!(
            h.events
                .lock()
                .last()
                .unwrap()
                .message
                .contains("nothing new")
        );
    }

    #[tokio::test]
    async fn triggers_fire_on_25_settled_labels_and_nightly() {
        let h = harness().await;
        *h.fake.now.lock() = at(12, 0);
        let mut tr = Triggers::default();
        assert_eq!(tr.due(&h.deps, false).await.unwrap(), None);
        // 25 labels land.
        let ex = crate::store::ExampleRow {
            id: "e".into(),
            class: "c".into(),
            question_id: "q".into(),
            session_id: None,
            requester: "root".into(),
            card_text: "t".into(),
            options: vec!["Allow".into()],
            embedding: None,
            asked_at: Utc::now(),
        };
        h.deps.store.put_example(&ex).await.unwrap();
        for i in 0..25 {
            let mut e = ex.clone();
            e.id = format!("e{i}");
            e.question_id = format!("q{i}");
            h.deps.store.put_example(&e).await.unwrap();
            h.deps
                .store
                .put_label(&crate::store::LabelRow {
                    example_id: e.id,
                    source: crate::types::LabelSource::Direct,
                    choice: "Allow".into(),
                    weight: 1.0,
                    at: Utc::now(),
                })
                .await
                .unwrap();
        }
        // First sight of 25: not settled yet; second tick with the same count fires.
        assert_eq!(tr.due(&h.deps, false).await.unwrap(), None);
        assert_eq!(tr.due(&h.deps, false).await.unwrap(), Some(Tier::A));
        // Nightly: inside the window, once per night, only with labels.
        let mut tr = Triggers {
            last_a_count: Some(25),
            ..Default::default()
        };
        *h.fake.now.lock() = at(2, 30);
        h.deps.training.lock().tiers.a.enabled = true;
        // Pretend a version consumed the labels so the count trigger is quiet.
        h.deps
            .store
            .put_version(&VersionRow {
                id: "a-x".into(),
                tier: Tier::A,
                base_hash: "h".into(),
                adapter_path: None,
                head_path: None,
                knn_path: None,
                metrics: serde_json::json!({}),
                data_range: format!(
                    "{}..{}",
                    Utc::now().to_rfc3339(),
                    (Utc::now() + ChronoDuration::hours(1)).to_rfc3339()
                ),
                trained_at: Utc::now(),
                venue: "local".into(),
                duration_s: 0,
                active: false,
            })
            .await
            .unwrap();
        assert_eq!(
            tr.due(&h.deps, false).await.unwrap(),
            None,
            "no labels after the version"
        );
        // Tier B is never due without a trainer.
        assert_eq!(tr.due(&h.deps, false).await.unwrap(), None);
    }

    #[test]
    fn persist_rewrites_only_the_double_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "agent:\n  model: x   # keep\n\ndouble:\n  enabled: false\n  training:\n    venue: auto\n\nslack:\n  enabled: false\n",
        )
        .unwrap();
        let mut cfg = DoubleConfig {
            enabled: true,
            ..Default::default()
        };
        cfg.training.venue = "node:gpu".into();
        persist(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("agent:\n  model: x   # keep\n\ndouble:\n"),
            "{text}"
        );
        assert!(text.ends_with("\nslack:\n  enabled: false\n"), "{text}");
        let doc: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let back: DoubleConfig = serde_yaml::from_value(doc["double"].clone()).unwrap();
        assert_eq!(back, cfg);
        // No block → appended; missing file → created.
        let p2 = dir.path().join("fresh.yaml");
        persist(&p2, &cfg).unwrap();
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&p2).unwrap()).unwrap();
        assert_eq!(
            doc["double"]["training"]["venue"],
            serde_yaml::Value::from("node:gpu")
        );
    }

    #[test]
    fn next_window_start() {
        let w = Window::parse("02:00-06:00").unwrap();
        assert_eq!(next_window(&w, at(1, 0)), at(2, 0));
        assert_eq!(
            next_window(&w, at(3, 0)),
            at(2, 0) + ChronoDuration::days(1)
        );
    }
}
