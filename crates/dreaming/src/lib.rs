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
//! - [`ledger`], [`tidy`], [`cases`], [`edits`], [`replay`], [`preferences`], [`journal`], [`undo`]: the phases.
//! - [`scrub`]: the secrets scrubber every write runs through (§10).
//! - [`tools`]: the nine tools (§9.4). [`rpc`]: the `dreaming.*` methods (§9.5).
//! - [`dry_run`]: `t dreaming dry-run` — Recall + rule-only Tidy, zero writes.

use std::path::Path;
use std::sync::Arc;

use runtime::Runtime;

pub mod cases;
pub mod config;
pub mod dry_run;
pub mod edits;
pub mod journal;
pub mod ledger;
pub mod night;
pub mod preferences;
pub mod replay;
pub mod rpc;
pub mod scrub;
pub mod sensor_rules;
pub mod signals;
pub mod tidy;
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

/// `budget_usd` as the routine's `budget:` value: rendered from nanos
/// (`{}.{:09} USD`, trailing zeros trimmed) so `schedule::file::Budget::parse`
/// always accepts it (136 D13 — `0.1234567891` or `1e20` from a number input
/// would otherwise break the file's parse).
pub fn budget_line(usd: f64) -> String {
    let n = (usd.max(0.0) * 1e9).round().min(u64::MAX as f64) as u64;
    let (whole, frac) = (n / 1_000_000_000, n % 1_000_000_000);
    if frac == 0 {
        format!("{whole} USD")
    } else {
        let frac = format!("{frac:09}");
        format!("{whole}.{} USD", frac.trim_end_matches('0'))
    }
}

/// The routine file text for `cfg` (design §9.7 with the config's values).
/// `cwd` is the data dir, so the sensor finds `dreaming/signals.json`.
/// `model:` is always written: the resolved tidy tier (136 D12).
pub fn routine_text(
    cfg: &DreamConfig,
    data_dir: &Path,
    engine: Option<&str>,
    agent: &config::AgentDefaults,
) -> String {
    let (models, _) = cfg.resolve_models(agent);
    let model_line = if models.tidy.is_empty() {
        String::new()
    } else {
        format!("model: {}\n", models.tidy)
    };
    let engine_arg = engine
        .filter(|e| !e.is_empty())
        .map_or(String::new(), |e| format!(" engine={e}"));
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
        .replace("{{budget}}", &budget_line(cfg.budget_usd))
        .replace("{{cwd}}", &format!("\"{}\"", data_dir.display()))
        .replace("{{model_line}}", &model_line)
}

/// The three bundled files, root-relative.
pub const BUNDLED: [&str; 3] = [
    "skills/dreaming.md",
    "sensors/dreaming.py",
    "schedules/dreaming.md",
];

/// Write the bundled files (136 §2.4): `skills/dreaming.md` and
/// `sensors/dreaming.py` are binary-owned (rewritten when the text differs);
/// `schedules/dreaming.md` is written when absent and otherwise patched in
/// place (`budget` / `when` / `run_timeout` / `model` / `cap` follow the
/// config, R-47; `enabled:` and every other line survive). Returns the
/// root-relative paths written.
pub fn install_files(
    data_dir: &Path,
    cfg: &DreamConfig,
    engine: Option<&str>,
    agent: &config::AgentDefaults,
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
    if routine.exists() {
        let before = std::fs::read_to_string(&routine)?;
        night::update_routine_file(data_dir, cfg, engine, agent)?;
        if std::fs::read_to_string(&routine)? != before {
            written.push(format!("{}/dreaming.md", schedule::DIR));
        }
    } else {
        std::fs::create_dir_all(data_dir.join(schedule::DIR))?;
        std::fs::write(&routine, routine_text(cfg, data_dir, engine, agent))?;
        written.push(format!("{}/dreaming.md", schedule::DIR));
    }
    Ok(written)
}

/// `dreaming.enabled: false` (136 D11, design §8): the routine, the sensor
/// and the skill leave; the schedule's reconcile drops the removed entry.
/// Absent files are fine.
pub fn remove_files(data_dir: &Path) -> std::io::Result<()> {
    for rel in BUNDLED {
        match std::fs::remove_file(data_dir.join(rel)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `t dreaming dry-run --data-dir <copy>` (136 §3.3 item 7): Recall plus
/// the deterministic parts of Tidy with zero writes; the counts the journal
/// would show ([`dry_run::render`] prints them).
pub async fn dry_run(data_dir: &Path) -> anyhow::Result<Report> {
    dry_run::dry_run(data_dir).await
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
            // D11: an installed routine must not keep firing with the plugin off.
            match remove_files(&rt.data_dir) {
                Ok(()) => tracing::info!("dreaming disabled by config; bundled files removed"),
                Err(e) => {
                    tracing::warn!("dreaming disabled by config; bundled files not removed: {e}")
                }
            }
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
        let agent = config::AgentDefaults::from_runtime(rt);
        // R-33 (D14): `engine ≠ self` is enforced always — the home engine when
        // configured, else this engine.
        let home = cfg
            .home_engine
            .clone()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| engine.clone());
        match install_files(&rt.data_dir, &cfg, Some(&home), &agent) {
            Ok(w) if !w.is_empty() => tracing::info!(files = ?w, "dreaming: bundled files written"),
            Ok(_) => {}
            Err(e) => tracing::warn!("dreaming: bundled files not written: {e}"),
        }
        if let Err(e) = night::start(rt, cfg.clone(), &engine, agent.clone()) {
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
                        agent,
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

    fn agent() -> config::AgentDefaults {
        config::AgentDefaults {
            model: "openai/gpt-5.6-sol".into(),
            escalate_model: None,
        }
    }

    /// The routine parses, carries the budget, the cap (D4), the engine arg
    /// always (R-33) and the resolved tidy model always (R-14/D12).
    #[test]
    fn routine_parses_with_the_schedule_crate_and_carries_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let text = routine_text(&cfg, dir.path(), Some("eng1"), &agent());
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
        assert_eq!(
            spec.cap,
            Some(::tools::Risk::Mutating),
            "D4: capped at mutating"
        );
        assert_eq!(
            spec.model.as_deref(),
            Some("openai/gpt-5.6-sol"),
            "D12: tidy defaults to agent.model and is always written"
        );
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
            &agent(),
        );
        let spec = schedule::file::parse("dreaming", &with_model).unwrap();
        assert_eq!(spec.model.as_deref(), Some("anthropic/claude-haiku-4-5"));
        assert_eq!(spec.budget.unwrap().nanos, 2_500_000_000);
        assert!(!spec.sensor.unwrap().args.contains_key("engine"));
    }

    /// R-15 (D13): any finite budget renders to a line `Budget::parse` accepts.
    #[test]
    fn budget_line_always_parses() {
        assert_eq!(budget_line(1.0), "1 USD");
        assert_eq!(budget_line(2.5), "2.5 USD");
        assert_eq!(budget_line(0.1234567891), "0.123456789 USD");
        assert_eq!(budget_line(0.05), "0.05 USD");
        for usd in [0.1234567891, 1e12, 1e20, 0.0, 1000.0, 3.0000000004] {
            let dir = tempfile::tempdir().unwrap();
            let text = routine_text(
                &DreamConfig {
                    budget_usd: usd,
                    ..Default::default()
                },
                dir.path(),
                Some("e"),
                &agent(),
            );
            schedule::file::parse("dreaming", &text).unwrap_or_else(|e| panic!("{usd}: {e}"));
        }
    }

    /// Install writes the binary-owned files whenever they differ and the
    /// routine once; a later install patches the routine's config keys in
    /// place (R-47) and keeps the user's `enabled:` toggle.
    #[test]
    fn install_writes_owned_files_always_and_patches_the_routine_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DreamConfig::default();
        let w = install_files(dir.path(), &cfg, None, &agent()).unwrap();
        assert_eq!(
            w,
            [
                "skills/dreaming.md",
                "sensors/dreaming.py",
                "schedules/dreaming.md"
            ]
        );
        assert_eq!(
            install_files(dir.path(), &cfg, None, &agent()).unwrap(),
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
            install_files(dir.path(), &cfg, None, &agent()).unwrap(),
            ["skills/dreaming.md"]
        );
        assert_eq!(std::fs::read_to_string(&routine).unwrap(), toggled);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("skills/dreaming.md")).unwrap(),
            SKILL
        );
        assert!(SENSOR.starts_with("# theuth-sensor: dreaming 3\n"));
        // R-47: a hand-edited config.yaml (here: a new cfg) reaches the installed routine.
        let changed = DreamConfig {
            budget_usd: 4.0,
            ..cfg
        };
        assert_eq!(
            install_files(dir.path(), &changed, Some("eng9"), &agent()).unwrap(),
            ["schedules/dreaming.md"]
        );
        let after = std::fs::read_to_string(&routine).unwrap();
        assert!(after.contains("enabled: false\n"), "{after}");
        assert!(after.contains("budget: 4 USD\n"), "{after}");
        assert!(after.contains(" engine=eng9"), "{after}");
    }

    /// R-13 (D11): `enabled: false` removes the three files (absent ones are fine).
    #[test]
    fn disabling_removes_the_bundled_files() {
        let dir = tempfile::tempdir().unwrap();
        install_files(dir.path(), &DreamConfig::default(), None, &agent()).unwrap();
        for rel in BUNDLED {
            assert!(dir.path().join(rel).exists(), "{rel}");
        }
        remove_files(dir.path()).unwrap();
        for rel in BUNDLED {
            assert!(!dir.path().join(rel).exists(), "{rel}");
        }
        remove_files(dir.path()).unwrap();
    }

    #[test]
    fn skill_carries_the_judge_prompt_verbatim_and_stays_under_24kb() {
        assert!(
            SKILL.contains(crate::replay::JUDGE_PROMPT),
            "skill.md and replay::JUDGE_PROMPT differ"
        );
        assert!(SKILL.len() < 24 * 1024, "{} bytes", SKILL.len());
        for heading in [
            "## 0 · Gate",
            "## 1 · Recall",
            "## 2 · Tidy",
            "## 3 · Mine",
            "## 4 · Rehearse",
            "## 5 · Settle",
            "## 6 · Suggest",
            "## 7 · Journal",
            "### Proposer prompt",
            "### Judge prompt",
        ] {
            assert!(SKILL.contains(heading), "{heading}");
        }
        assert!(SKILL.contains("repeat passes while `dreaming_ledger()` reports backlog"));
    }

    /// A hash of every regular file under `dir` (sqlite side files excluded).
    fn fingerprint(dir: &Path) -> std::collections::BTreeMap<String, u64> {
        use std::hash::{Hash as _, Hasher as _};
        let mut out = std::collections::BTreeMap::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = p.to_string_lossy().to_string();
                if name.ends_with("-shm") || name.ends_with("-wal") {
                    continue;
                }
                let mut h = std::hash::DefaultHasher::new();
                std::fs::read(&p).unwrap().hash(&mut h);
                out.insert(name, h.finish());
            }
        }
        out
    }

    /// 30 concepts (fresh, long titles, a dated series, a never-opened noise
    /// fact, a warning, archived) and a 2-session store: the counts the
    /// contract names, and not one byte changes.
    #[tokio::test]
    async fn dry_run_counts_the_fixture_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let k = d.join(knowledge::DIR);
        let old = (chrono::Utc::now() - chrono::Duration::days(40))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let young = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let write = |rel: &str, type_: &str, title: &str, status: &str, ts: &str, extra: &str| {
            let p = k.join(format!("{rel}.md"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(
                p,
                format!("---\ntype: {type_}\ntitle: \"{title}\"\nstatus: {status}\ntimestamp: {ts}\n{extra}---\nbody\n"),
            )
            .unwrap();
        };
        let long = "a long title ".repeat(12); // 156 chars
        // 10 fresh short, 8 tidy long, 3 dated records (one series), 2 old records, 1 noise, 1 warning, 1 archived long, 4 plain = 30
        for i in 0..10 {
            write(
                &format!("facts/fresh-{i}"),
                "fact",
                "short fresh",
                "fresh",
                &young,
                "",
            );
        }
        for i in 0..8 {
            write(&format!("facts/long-{i}"), "fact", &long, "tidy", &old, "");
        }
        for day in ["2026-10-01", "2026-10-02", "2026-10-03"] {
            write(
                &format!("facts/dev-machine-state-{day}"),
                "record",
                "state",
                "tidy",
                &young,
                "",
            );
        }
        write("facts/old-record-a", "record", "landed", "tidy", &old, "");
        write("facts/old-record-b", "record", "landed", "tidy", &old, "");
        write("facts/noise", "fact", "mcp recon", "tidy", &old, "");
        write(
            "facts/warn",
            "warning",
            "never paste keys",
            "tidy",
            &old,
            "",
        );
        write("facts/dead", "fact", &long, "archived", &old, "");
        for i in 0..4 {
            write(
                &format!("procedures/plain-{i}"),
                "procedure",
                "how to",
                "tidy",
                &old,
                "verified: 2026-10-01\n",
            );
        }
        assert_eq!(dry_run::read_metas(d).len(), 30);
        // The uses table, through the knowledge store's own schema.
        {
            let rt = Runtime::new(figment::Figment::new(), d.to_owned());
            let store = knowledge::Knowledge::open(d, rt).await.unwrap();
            store
                .record_shown(
                    &["facts/noise".into(), "facts/warn".into()],
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
            for _ in 0..60 {
                store
                    .record_shown(&["facts/noise".into()], chrono::Utc::now())
                    .await
                    .unwrap();
                store
                    .record_shown(&["facts/warn".into()], chrono::Utc::now())
                    .await
                    .unwrap();
            }
            store
                .record_shown(&["facts/long-0".into()], chrono::Utc::now())
                .await
                .unwrap();
        }
        // Two sessions, one advisor (excluded), one tool error.
        {
            let store = agent::store::Store::open(&d.join("sessions.sqlite"), None)
                .await
                .unwrap();
            let a = runtime::SessionId::new();
            store
                .create_session(a, "m", None, None, None)
                .await
                .unwrap();
            store
                .create_session_kind(
                    runtime::SessionId::new(),
                    "m",
                    None,
                    None,
                    None,
                    agent::SessionKind::Advisor,
                )
                .await
                .unwrap();
            store
                .create_session(runtime::SessionId::new(), "m", Some(a), None, None)
                .await
                .unwrap();
        }
        // Let the stores' pools close (a WAL checkpoint on close is theirs, not the dry run's).
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let before = fingerprint(d);
        let r = dry_run(d).await.unwrap();
        assert_eq!(
            r,
            Report {
                fresh: 10,
                retitle_candidates: 8,
                duplicate_stems: 1,
                // two records past 14 days + the 61-shown / 0-opened fact; the warning is exempt
                archive_by_use_candidates: 3,
                verification_sample: 15,
                sessions: 2,
                friction_events: 0,
            },
            "{r:?}"
        );
        assert_eq!(fingerprint(d), before, "dry run changed a file");
        assert!(dry_run::render(&r).contains("retitle candidates      8"));
    }
}
