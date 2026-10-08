//! Dreaming (design doc 136): a nightly, own-context routine that tidies the
//! knowledge base, mines the day's friction into cases, proposes bounded
//! skill edits behind a replay gate, and journals — with undo. A plugin
//! crate (00 §1): its own state under `<data_dir>/dreaming/`, two actors,
//! nine tools, a config section, events, RPC.
//!
//! - [`config`]: the `dreaming:` section (§9.2).
//! - [`types`]: cases, edits, nights, signals, status, settings (§2.5–2.7, §6).
//! - [`signals`]: the `Signals` actor → `signals.json` (§9.3; the gate's input).
//! - [`night`]: the `Night` actor → `state.json`, status, settings (§9.3).
//! - [`sensor_rules`]: the gate rule, mirrored by `sensor.py` (§4 phase 0).
//! - [`ledger`], [`cases`], [`edits`], [`replay`], [`preferences`], [`journal`], [`undo`]: the phases (W3).
//! - [`tools`]: the nine tools (§9.4). [`rpc`]: the `dreaming.*` methods (§9.5).
//!
//! Wave C (this file's shape) is the contract; behaviour lands in the modules
//! the `// 136 W3` markers name.

use std::path::Path;
use std::sync::Arc;

use runtime::Runtime;

pub mod cases;
pub mod config;
pub mod edits;
pub mod journal;
pub mod ledger;
pub mod night;
pub mod preferences;
pub mod replay;
pub mod rpc;
pub mod sensor_rules;
pub mod signals;
pub mod tools;
pub mod types;
pub mod undo;

pub use config::DreamConfig;
pub use night::{DIR, DREAMING_ACTOR, Night};
pub use signals::{SIGNALS_ACTOR, Signals};
pub use types::*;

/// The bundled `dreaming` skill (binary-owned; `<data_dir>/skills/dreaming.md`).
pub const SKILL: &str = include_str!("skill.md");
/// The bundled gate sensor (binary-owned; `<data_dir>/sensors/dreaming.py`).
pub const SENSOR: &str = include_str!("sensor.py");
/// The routine file template (`<data_dir>/schedules/dreaming.md`, written only when absent).
const ROUTINE_TEMPLATE: &str = include_str!("routine.md");

/// Event: a night began (136 §9.3).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DreamNightStarted {
    /// The night id.
    pub night: NightId,
}

/// Event: a night ended (136 §9.3); the attention digest and the HUD ring listen.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DreamNightFinished {
    /// The night id.
    pub night: NightId,
    /// Counts.
    pub counts: NightCounts,
    /// Money spent, nanos USD.
    pub spend_nanos: u64,
    /// Proposals raised.
    pub proposals: Vec<Proposal>,
}

/// Event: a night or a group was undone (136 §9.3).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct DreamUndone {
    /// The night id.
    pub night: NightId,
    /// The group, or `None` for the whole night.
    pub group: Option<String>,
}

/// The routine file text for `cfg` (design §9.7 with the config's values).
/// `cwd` is the data dir, so the sensor finds `dreaming/signals.json`.
pub fn routine_text(cfg: &DreamConfig, data_dir: &Path, engine: Option<&str>) -> String {
    let model_line = cfg
        .models
        .tidy
        .as_deref()
        .map_or(String::new(), |m| format!("model: {m}\n"));
    let engine_arg = engine.map_or(String::new(), |e| format!(" engine={e}"));
    ROUTINE_TEMPLATE
        .replace(
            "{{window}}",
            &format!("{}-{}", cfg.window[0], cfg.window[1]),
        )
        .replace("{{idle}}", &cfg.idle_minutes.to_string())
        .replace("{{heavy_concepts}}", &cfg.heavy_day.concepts.to_string())
        .replace("{{heavy_sessions}}", &cfg.heavy_day.sessions.to_string())
        .replace("{{engine_arg}}", &engine_arg)
        .replace("{{run_timeout}}", &cfg.run_timeout)
        .replace("{{budget}}", &format!("{} USD", cfg.budget_usd))
        .replace("{{cwd}}", &format!("\"{}\"", data_dir.display()))
        .replace("{{model_line}}", &model_line)
}

/// Write the bundled files (136 §2.4): `skills/dreaming.md` and
/// `sensors/dreaming.py` are binary-owned (rewritten when the text differs);
/// `schedules/dreaming.md` only when absent, so the user's `enabled:` toggle
/// survives restarts. Returns the root-relative paths written.
pub fn install_files(
    data_dir: &Path,
    cfg: &DreamConfig,
    engine: Option<&str>,
) -> std::io::Result<Vec<String>> {
    let mut written = Vec::new();
    for (rel, body) in [
        ("skills/dreaming.md", SKILL),
        ("sensors/dreaming.py", SENSOR),
    ] {
        let path = data_dir.join(rel);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body) {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&path, body)?;
            written.push(rel.to_owned());
        }
    }
    let routine = data_dir.join(schedule::DIR).join("dreaming.md");
    if !routine.exists() {
        std::fs::create_dir_all(data_dir.join(schedule::DIR))?;
        std::fs::write(&routine, routine_text(cfg, data_dir, engine))?;
        written.push(format!("{}/dreaming.md", schedule::DIR));
    }
    Ok(written)
}

/// `t dreaming dry-run --data-dir <copy>` (136 §3.3 item 7): Recall plus
/// the deterministic parts of Tidy with zero writes; the counts the journal
/// would show. Wave C: the empty report; 136 W3 fills it.
pub async fn dry_run(data_dir: &Path) -> anyhow::Result<Report> {
    // 136 W3
    let _ = data_dir;
    Ok(Report::default())
}

/// The plugin. Reads `dreaming:`; when enabled installs the bundled files,
/// spawns [`Signals`] and [`Night`], registers the tools and the prompt block.
pub struct DreamingPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for DreamingPlugin {
    fn name(&self) -> &'static str {
        "dreaming"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: DreamConfig = rt.section("dreaming").unwrap_or_else(|e| {
            tracing::debug!("no [dreaming] config section, using defaults: {e}");
            DreamConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("dreaming disabled by config");
            return Ok(());
        }
        // The engine id (68 §2): the mesh plugin starts after us, so read the identity file.
        let engine = match mesh::identity::Identity::load_or_create(&rt.data_dir.join(mesh::DIR)) {
            Ok(i) => i.id().to_string(),
            Err(e) => {
                tracing::warn!("dreaming: mesh identity unavailable; engine stays '': {e:#}");
                String::new()
            }
        };
        let home = cfg.home_engine.clone().filter(|h| !h.is_empty());
        match install_files(&rt.data_dir, &cfg, home.as_deref()) {
            Ok(w) if !w.is_empty() => tracing::info!(files = ?w, "dreaming: bundled files written"),
            Ok(_) => {}
            Err(e) => tracing::warn!("dreaming: bundled files not written: {e}"),
        }
        if let Err(e) = night::start(rt, cfg.clone(), &engine) {
            tracing::warn!("dreaming actor not started: {e:#}");
        }
        if let Err(e) = signals::start(rt, &engine).await {
            tracing::warn!("dreaming signals not started: {e:#}");
        }
        let rt_tools = rt.clone();
        tokio::spawn(async move {
            let knowledge = match rt_tools
                .lookup_when::<knowledge::KnowledgeActor>(knowledge::KNOWLEDGE_ACTOR)
                .await
            {
                Some(k) => k.ask(knowledge::Handle).await.ok(),
                None => None,
            };
            if let Some(reg) = rt_tools
                .lookup_when::<::tools::ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            {
                crate::tools::register(
                    &reg,
                    Arc::new(crate::tools::Seams {
                        data_dir: rt_tools.data_dir.clone(),
                        cfg,
                        knowledge,
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

    #[test]
    fn routine_parses_with_the_schedule_crate_and_carries_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let text = routine_text(&cfg, dir.path(), Some("eng1"));
        let spec =
            schedule::file::parse("dreaming", &text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(
            spec.budget,
            Some(schedule::file::Budget {
                nanos: 1_000_000_000,
                currency: "USD".into()
            })
        );
        assert_eq!(
            spec.run_timeout,
            Some(std::time::Duration::from_secs(45 * 60))
        );
        assert_eq!(spec.when_text, "every 30m when dreaming.py");
        let sensor = spec.sensor.unwrap();
        assert_eq!(sensor.args["window"], "01:00-06:00");
        assert_eq!(sensor.args["idle"], "20");
        assert_eq!(sensor.args["engine"], "eng1");
        assert!(spec.quiet && !spec.approvals && spec.enabled);
        assert_eq!(spec.memory, schedule::file::Memory::Clear);
        assert_eq!(spec.cwd.as_deref(), Some(dir.path()));
        assert!(spec.model.is_none());
        let with_model = routine_text(
            &DreamConfig {
                models: config::Models {
                    tidy: Some("anthropic/claude-haiku-4-5".into()),
                    ..Default::default()
                },
                budget_usd: 2.5,
                ..cfg
            },
            dir.path(),
            None,
        );
        let spec = schedule::file::parse("dreaming", &with_model).unwrap();
        assert_eq!(spec.model.as_deref(), Some("anthropic/claude-haiku-4-5"));
        assert_eq!(spec.budget.unwrap().nanos, 2_500_000_000);
        assert!(!spec.sensor.unwrap().args.contains_key("engine"));
    }

    #[test]
    fn install_writes_owned_files_always_and_the_routine_once() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let w = install_files(dir.path(), &cfg, None).unwrap();
        assert_eq!(
            w,
            [
                "skills/dreaming.md",
                "sensors/dreaming.py",
                "schedules/dreaming.md"
            ]
        );
        assert_eq!(
            install_files(dir.path(), &cfg, None).unwrap(),
            Vec::<String>::new()
        );
        // The user toggles the routine off and edits the skill: the toggle survives, the skill does not.
        let routine = dir.path().join("schedules/dreaming.md");
        let toggled = std::fs::read_to_string(&routine)
            .unwrap()
            .replace("---\nevery", "---\nenabled: false\nevery");
        std::fs::write(&routine, &toggled).unwrap();
        std::fs::write(dir.path().join("skills/dreaming.md"), "mine").unwrap();
        assert_eq!(
            install_files(dir.path(), &cfg, None).unwrap(),
            ["skills/dreaming.md"]
        );
        assert_eq!(std::fs::read_to_string(&routine).unwrap(), toggled);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("skills/dreaming.md")).unwrap(),
            SKILL
        );
        assert!(SENSOR.starts_with("# theuth-sensor: dreaming 1\n"));
    }

    #[tokio::test]
    async fn dry_run_is_the_empty_report_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dry_run(dir.path()).await.unwrap(), Report::default());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
