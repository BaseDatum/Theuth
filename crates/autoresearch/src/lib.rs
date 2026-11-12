//! Autoresearch (design doc 140): a measured experiment loop whose ratchet
//! the harness owns. A plugin crate (00 §1): its own sqlite and study
//! directories under `<data_dir>/autoresearch/`, one actor, seven tools, a
//! config section, events, RPC, a bundled skill. One schedule file per study
//! drives the clock (§8); the plugin does the science.
//!
//! Module map (owners per §21.1 / the 140-C brief; every stub returns
//! `Err(Error::NotBuilt("<owner>"))`):
//! - C: [`config`] (§12.2), [`types`] (§12.3–12.5), [`store`] (§12.6), [`error`], this file, [`rpc_impl`].
//! - W1: [`runner`], [`scanner`], [`gate`], [`custody`], [`slot`] (§5).
//! - W2: [`git`], [`finalize`], [`record`] (§6, §11, §7.3).
//! - W3: [`actor`], [`binding`], [`schedule_file`], [`stop`] (§4, §8, §12.3).
//! - W6: [`tools`], [`brief`], [`novelty`], [`review`] (§7, §9, §12.4), `skill.md` (§12.7).
//! - W5: [`wave`] (§10).
//!
//! 159 (seven variants; §18 of its design): C froze the contract in [`types`], [`store`],
//! [`scanner`], [`runner`] and the dispatch lines of [`actor`]; each wave owns one module —
//! W1 [`pareto`] + [`refs`], W2 [`cases`] (+ `crates/evalcore`), W3 [`judge`], W4
//! [`population`], W5 [`literature`] + [`hooks`], W6 [`world`], W7 [`mechanism`] + [`suite`] —
//! and one skill fragment under `skill/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;

pub mod actor;
pub mod binding;
pub mod brief;
pub mod cases;
pub mod config;
pub mod custody;
pub mod error;
pub mod finalize;
pub mod gate;
pub mod git;
pub mod hooks;
pub mod judge;
pub mod literature;
pub mod mechanism;
pub mod novelty;
pub mod pareto;
pub mod population;
pub mod record;
pub mod refs;
pub mod review;
pub mod rpc_impl;
pub mod runner;
pub mod scanner;
pub mod schedule_file;
pub mod setup;
pub mod slot;
pub mod stop;
pub mod store;
pub mod suite;
pub mod tools;
pub mod types;
pub mod wave;
pub mod world;

pub use config::AutoresearchConfig;
pub use error::{Error, Result};
pub use store::Store;
pub use types::*;

/// `Plugin::name`, the prompt block's key and the `Toolset.prompts` entry.
pub const PLUGIN: &str = "autoresearch";
/// Registry name of the [`Studies`] actor (140 §12.3).
pub const AUTORESEARCH_ACTOR: &str = "autoresearch";
/// `<data_dir>/autoresearch/` (140 §12.6).
pub const DIR: &str = "autoresearch";
/// Schedule file stem prefix: `autoresearch-<slug>` (140 §8.1).
pub const SCHEDULE_PREFIX: &str = "autoresearch-";
/// Why a study cannot run on Windows (2026-10-09): bench, check and holdout commands run under
/// `/bin/bash -lc` (runner.rs, custody.rs), which a stock Windows box does not have. `Create` and
/// the draft tool refuse with this text before anything is written.
pub const WINDOWS_UNSUPPORTED: &str = "autoresearch needs a POSIX shell (/bin/bash) for bench, check and holdout commands — on Windows run studies from WSL or a macOS/Linux engine";
/// The bundled `autoresearch` skill (binary-owned; `<data_dir>/skills/autoresearch.md`): the 140
/// core text plus one fragment per 159 wave (`skill/<kind>.md`, each owned by its wave; empty
/// until the wave lands, so the installed text is byte-identical to the core until then).
pub const SKILL: &str = concat!(
    include_str!("../skill.md"),
    include_str!("../skill/pareto.md"),     // 159:W1
    include_str!("../skill/prompts.md"),    // 159:W2
    include_str!("../skill/judged.md"),     // 159:W3
    include_str!("../skill/population.md"), // 159:W4
    include_str!("../skill/literature.md"), // 159:W5
    include_str!("../skill/world.md"),      // 159:W6
    include_str!("../skill/meta.md"),       // 159:W7
);
/// Where the skill is written, root-relative.
pub const SKILL_PATH: &str = "skills/autoresearch.md";

/// `<data_dir>/autoresearch/<slug>/` (140 §12.6).
pub fn study_dir(root: &Path, slug: &str) -> PathBuf {
    root.join(slug)
}

/// `<data_dir>/autoresearch/drafts/<slug>/` (140 §4.1).
pub fn drafts_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("drafts").join(slug)
}

/// Write `skills/autoresearch.md` when the bundled text differs (binary-owned,
/// like dreaming's `loops.md`; `first_run` does not know this skill — R-120).
pub fn install_skill(data_dir: &Path) -> std::io::Result<bool> {
    let path = data_dir.join(SKILL_PATH);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(SKILL) {
        return Ok(false);
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&path, SKILL)?;
    Ok(true)
}

/// The `Studies` actor (140 §12.3): one per engine, registered as
/// [`AUTORESEARCH_ACTOR`]. Holds no state that is not a row (§8.5); the
/// handlers delegate to [`actor`].
#[derive(kameo::Actor)]
pub struct Studies {
    /// `<data_dir>`.
    pub data_dir: PathBuf,
    /// `<data_dir>/autoresearch` or `root:`.
    pub root: PathBuf,
    /// The section as read at start; `SetSettings` updates it.
    pub cfg: AutoresearchConfig,
    /// The `agent:` section's model defaults (R1-D5).
    pub agent: config::AgentDefaults,
    /// For events, `section`, `data_dir`.
    pub rt: Runtime,
    /// The ledger.
    pub store: Store,
    /// W1/W2/W6 seams (the real modules in production; test doubles in tests).
    pub backend: Arc<dyn actor::Backend>,
}

impl Studies {
    /// Publish a bus event (bridged to RPC clients).
    pub async fn publish(&self, ev: StudyEvent) {
        self.rt.publish(ev).await;
    }
}

macro_rules! delegate {
    ($($msg:ty => $reply:ty, $f:ident;)*) => {$(
        impl Message<$msg> for Studies {
            type Reply = std::result::Result<$reply, String>;

            async fn handle(&mut self, m: $msg, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
                actor::$f(self, m).await.map_err(|e| e.to_string())
            }
        }
    )*};
}

delegate! {
    Create => StudyInfo, create;
    StartDraft => StudyInfo, start_draft;
    Notes => NotesReport, notes;
    SteerMsg => SteerId, steer;
    IdeaOp => Idea, idea;
}

/// Begin / Run / Log measure for minutes; Control waits for a cancelled run and asks
/// the agent and schedule actors; Status / Detail / Experiment ask the agent actor for
/// the spend or read git. They all run off the mailbox on an [`actor::Env`] and reply
/// through `DelegatedReply` (plugins.md §3.2; R-115), so a `StopNow` cancel and every
/// read keep flowing while a benchmark runs (the asker still sees `Result<T, String>`).
macro_rules! delegate_off_mailbox {
    ($($msg:ty => $reply:ty, $f:ident;)*) => {$(
        impl Message<$msg> for Studies {
            type Reply = kameo::reply::DelegatedReply<std::result::Result<$reply, String>>;

            async fn handle(&mut self, m: $msg, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
                let (reply, tx) = ctx.reply_sender();
                let env = self.env();
                tokio::spawn(async move {
                    let r = actor::$f(&env, m).await.map_err(|e| e.to_string());
                    if let Some(tx) = tx {
                        tx.send(r);
                    }
                });
                reply
            }
        }
    )*};
}

delegate_off_mailbox! {
    Begin => Brief, begin_env;
    Run => RunReport, run_env;
    Log => LogReport, log_env;
    Control => StudyInfo, control_env;
    Status => Vec<StudyInfo>, status_env;
    Detail => StudyDetail, detail_env;
    Experiments => Vec<ExperimentRow>, experiments_env;
    Experiment => ExperimentDetail, experiment_env;
}

impl Message<Settings> for Studies {
    type Reply = std::result::Result<AutoresearchConfig, String>;

    async fn handle(&mut self, _: Settings, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(self.cfg.clone())
    }
}

impl Message<DraftRoot> for Studies {
    type Reply = std::result::Result<PathBuf, String>;

    async fn handle(&mut self, _: DraftRoot, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ok(self.root.clone())
    }
}

impl Message<StudySlugs> for Studies {
    type Reply = std::result::Result<Vec<String>, String>;

    async fn handle(&mut self, _: StudySlugs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .list_studies()
            .await
            .map(|v| v.into_iter().map(|s| s.slug).collect())
            .map_err(|e| e.to_string())
    }
}

impl Message<SetSettings> for Studies {
    type Reply = std::result::Result<AutoresearchConfig, String>;

    /// Persist the whole section and apply it live (`autoresearch.settings.set`).
    /// `root` is fixed for the process (R-101): the store is open on it and the
    /// cancel / phase maps are keyed by it.
    async fn handle(
        &mut self,
        SetSettings(cfg): SetSettings,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if !(cfg.defaults.budget.usd.is_finite() && cfg.defaults.budget.usd > 0.0) {
            return Err("defaults.budget.usd must be a positive number".into());
        }
        if cfg.max_parallel == 0 {
            return Err("max_parallel must be at least 1".into());
        }
        if cfg.root(&self.data_dir) != self.root {
            return Err(format!(
                "root cannot change while the engine runs (the ledger is open at {}); it takes effect at restart",
                self.root.display()
            ));
        }
        config::persist(&self.data_dir.join("config.yaml"), &cfg)
            .map_err(|e| format!("config.yaml: {e}"))?;
        self.cfg = cfg;
        Ok(self.cfg.clone())
    }
}

/// Spawn and register the actor; open the store first.
pub async fn start(
    rt: &Runtime,
    cfg: AutoresearchConfig,
    agent: config::AgentDefaults,
) -> anyhow::Result<ActorRef<Studies>> {
    let root = cfg.root(&rt.data_dir);
    let store = Store::open(&root).await?;
    let studies = Studies {
        data_dir: rt.data_dir.clone(),
        root,
        cfg,
        agent,
        rt: rt.clone(),
        store,
        backend: Arc::new(actor::Real),
    };
    let env = studies.env();
    let actor = Studies::spawn_with_mailbox(studies, runtime::subscriber_mailbox());
    actor.register(AUTORESEARCH_ACTOR)?;
    // 140 §8.3 / R-5: the schedule applies the study toolset before every prompt.
    schedule::confine(SCHEDULE_PREFIX, binding::confinement(env.store.clone()));
    // §8.2 / §12.3 subscriptions: binding on `Fired`, dangling experiments on `TurnFinished`.
    rt.subscribe::<Studies, schedule::ScheduleEvent>(&actor)
        .await;
    rt.subscribe::<Studies, agent::AgentEvent>(&actor).await;
    // §8.5 restart recovery, then the attention source (§13.1).
    if let Err(e) = actor::recover(&env).await {
        tracing::warn!("autoresearch recovery: {e}");
    }
    let rt2 = rt.clone();
    let store = env.store.clone();
    tokio::spawn(async move {
        if let Some(inbox) = rt2
            .lookup_when::<roster::attention::Attention>(roster::attention::ATTENTION_ACTOR)
            .await
        {
            let _ = inbox
                .tell(roster::attention::RegisterSource(Arc::new(
                    actor::AttentionSource(store),
                )))
                .await;
        }
    });
    Ok(actor)
}

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<StudyEvent>().await
}

/// The plugin. Reads `autoresearch:`; when enabled opens the store, spawns
/// [`Studies`], writes the bundled skill, registers the tools and the prompt block.
pub struct AutoresearchPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for AutoresearchPlugin {
    fn name(&self) -> &'static str {
        PLUGIN
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: AutoresearchConfig = rt.section(PLUGIN).unwrap_or_else(|e| {
            tracing::debug!("no [autoresearch] config section, using defaults: {e}");
            AutoresearchConfig::default()
        });
        if !cfg.enabled {
            // 140 §12.2 / R-6: the schedule plugin still runs; study files must not fire unconfined.
            let peers = mesh::has_stored_peers(&rt.data_dir).await;
            match schedule_file::disable_all(&rt.data_dir, peers) {
                Ok(off) if off.is_empty() => {
                    tracing::info!("autoresearch disabled by config; no tools, no actor")
                }
                Ok(off) => tracing::info!(
                    "autoresearch disabled by config; no tools, no actor; study files paused: {}",
                    off.join(", ")
                ),
                Err(e) => tracing::warn!("autoresearch disabled; study files not paused: {e}"),
            }
            return Ok(());
        }
        match install_skill(&rt.data_dir) {
            Ok(true) => tracing::info!("autoresearch: {SKILL_PATH} written"),
            Ok(false) => {}
            Err(e) => tracing::warn!("autoresearch: {SKILL_PATH} not written: {e}"),
        }
        let paired = match mesh::paired_count() {
            Some(n) => n > 0,
            None => mesh::has_stored_peers(&rt.data_dir).await,
        };
        let agent = config::AgentDefaults::from_runtime(rt, paired);
        // R-117: without the actor the tools would only answer "not running"; register nothing.
        if let Err(e) = start(rt, cfg.clone(), agent.clone()).await {
            tracing::warn!(
                "autoresearch actor not started; tools and prompt block not registered: {e:#}"
            );
            return Ok(());
        }
        let rt_tools = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt_tools
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                tools::register(
                    &reg,
                    Arc::new(tools::Seams {
                        data_dir: rt_tools.data_dir.clone(),
                        cfg,
                        agent,
                    }),
                )
                .await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every actor message compiles against its handler; an empty ledger lists no studies.
    #[tokio::test]
    async fn actor_starts_and_settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let actor = start(&rt, AutoresearchConfig::default(), Default::default())
            .await
            .unwrap();
        assert!(actor.ask(Status::default()).await.unwrap().is_empty());
        let e = actor
            .ask(Begin {
                session: runtime::SessionId::new(),
                slot: None,
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("not an autoresearch session"), "{e}");
        let cfg = actor.ask(Settings).await.unwrap();
        assert_eq!(cfg, AutoresearchConfig::default());
        let mut cfg2 = cfg.clone();
        cfg2.keep_finished_days = 3;
        assert_eq!(actor.ask(SetSettings(cfg2.clone())).await.unwrap(), cfg2);
        assert!(dir.path().join("config.yaml").exists());
        assert_eq!(actor.ask(Settings).await.unwrap().keep_finished_days, 3);
        assert_eq!(rpc_impl::settings().await.unwrap().keep_finished_days, 3);
    }

    /// R-101: `root` cannot move under an open ledger.
    #[tokio::test]
    async fn settings_refuse_a_root_change_at_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let cfg = AutoresearchConfig::default();
        let root = cfg.root(&rt.data_dir);
        // Unregistered: the sibling test holds the registry name.
        let actor = Studies::spawn(Studies {
            data_dir: rt.data_dir.clone(),
            root,
            cfg: cfg.clone(),
            agent: Default::default(),
            rt,
            store: Store::open(&dir.path().join("autoresearch")).await.unwrap(),
            backend: Arc::new(actor::Real),
        });
        let mut cfg = actor.ask(Settings).await.unwrap();
        cfg.root = Some(dir.path().join("elsewhere").display().to_string());
        let e = actor.ask(SetSettings(cfg)).await.unwrap_err().to_string();
        assert!(e.contains("takes effect at restart"), "{e}");
        assert_eq!(actor.ask(Settings).await.unwrap().root, None);
    }
}
