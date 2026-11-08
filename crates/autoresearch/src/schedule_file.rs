//! W3 LOOP — the study's schedule file (design doc 140 §8.1):
//! `<data_dir>/schedules/autoresearch-<slug>.md` with `every: 0s`,
//! `max_runs`, `budget:`, `run_timeout:`, `approvals: false`, `quiet: true`,
//! `runtime: clear`, `cwd:`, `model:`, `enabled:` (with a `window:` also
//! `when:`, `every: 1m` and `memory: clear`). The plugin rewrites only
//! `enabled:`; it deletes the file through the schedule actor when the study stops.
//!
//! Deviation from §8.1's listing: `memory: clear` is only legal with `when:`
//! (`crates/schedule/src/file.rs` "memory: needs when:"); a clock-only file
//! clears the frame on every fire anyway (34 D4), so the `every: 0s` variant
//! omits the key.

use std::path::Path;
use std::time::Duration;

use kameo::actor::ActorRef;

use crate::error::{Error, Result};
use crate::types::{StartWhen, StudySpec};

/// Fires the schedule treats as a backstop beyond `budget.experiments` (§8.1: "+ 10 for interrupted fires").
const EXTRA_RUNS: u32 = 10;
/// `run_timeout` floor (§8.1 "at least 30m").
const MIN_RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Slack added to one experiment's worst case (§8.1 "+ 15m").
const SLACK: Duration = Duration::from_secs(15 * 60);

/// The schedule name of a study (`autoresearch-<slug>`).
pub fn name(slug: &str) -> String {
    format!("{}{slug}", crate::SCHEDULE_PREFIX)
}

/// A humantime string of the spec (`600s`), or `fallback` when unparsable.
pub fn duration(s: &str, fallback: Duration) -> Duration {
    humantime::parse_duration(s.trim()).unwrap_or(fallback)
}

/// `1.5 USD` — the schedule crate's `budget:` grammar (`<number> <CUR>`, ≤ 9 fraction digits).
pub fn budget_line(usd: f64) -> String {
    let nanos = (usd.max(0.0) * 1e9).round() as u64;
    let whole = nanos / 1_000_000_000;
    let frac = nanos % 1_000_000_000;
    if frac == 0 {
        format!("{whole} USD")
    } else {
        let f = format!("{frac:09}");
        format!("{whole}.{} USD", f.trim_end_matches('0'))
    }
}

/// The file text for `spec` (140 §8.1).
pub fn text(
    spec: &StudySpec,
    cfg: &crate::config::AutoresearchConfig,
    worktree: &Path,
    model: &str,
    start: &StartWhen,
) -> String {
    let mut s = String::from("---\n");
    let sensor = match start {
        StartWhen::Window { sensor } => Some(sensor.as_str()),
        StartWhen::Now => spec.window.as_deref(),
    }
    .map(str::trim)
    .filter(|w| !w.is_empty());
    // A sensor needs a cadence (40 §2: `when:` wants `every:` ≥ 30 s).
    s.push_str(if sensor.is_some() {
        "every: 1m\n"
    } else {
        "every: 0s\n"
    });
    s.push_str(&format!(
        "max_runs: {}\n",
        spec.budget.experiments.saturating_add(EXTRA_RUNS).max(1)
    ));
    s.push_str(&format!(
        "budget: {}\n",
        budget_line(spec.budget.per_experiment_usd)
    ));
    s.push_str(&format!(
        "run_timeout: {}m\n",
        run_timeout(spec, cfg).as_secs().div_ceil(60)
    ));
    s.push_str("approvals: false\n");
    s.push_str("quiet: true\n");
    if sensor.is_some() {
        s.push_str("memory: clear\n");
    }
    s.push_str("runtime: clear\n");
    s.push_str(&format!(
        "cwd: {}\n",
        yaml_str(&worktree.display().to_string())
    ));
    if !model.trim().is_empty() {
        s.push_str(&format!("model: {}\n", model.trim()));
    }
    s.push_str("enabled: true\n");
    // 182 D6: the study's schedule fires on the engine that started it, only.
    if let Some(e) = schedule::self_engine() {
        s.push_str(&format!("engine: {e}\n"));
    }
    if let Some(w) = sensor {
        s.push_str(&format!("when: {}\n", yaml_str(w)));
    }
    s.push_str("---\n\n");
    // 159:W7 — the fire prompt of the pinned mechanism, else the compiled text; the study
    // root is two levels above `<root>/<slug>/worktree`.
    let root = worktree.parent().and_then(Path::parent).unwrap_or(worktree);
    match crate::mechanism::fire_prompt(root, None) {
        Some(p) => s.push_str(&p.replace("{slug}", &spec.slug)),
        None => s.push_str(&format!(
            "Run the next experiment of autoresearch study `{}`. Call experiment_begin() first and follow the protocol \
             in the brief it returns. Do not ask questions; nobody is watching this session.\n",
            spec.slug
        )),
    }
    s
}

/// A double-quoted YAML scalar.
fn yaml_str(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `run_timeout` = `(crash_retries + confirm_runs + 2) × bench.timeout + checks.timeout + bench.wait + 15m`, at least 30 m.
pub fn run_timeout(spec: &StudySpec, cfg: &crate::config::AutoresearchConfig) -> Duration {
    let bench = duration(&spec.bench.timeout, Duration::from_secs(600));
    let checks = spec.checks.as_ref().map_or(Duration::ZERO, |c| {
        duration(&c.timeout, Duration::from_secs(300))
    });
    let wait = duration(&cfg.bench.wait, Duration::from_secs(20 * 60));
    let samples = u32::from(cfg.defaults.crash_retries) + u32::from(cfg.gate.confirm_runs) + 2;
    (bench * samples + checks + wait + SLACK).max(MIN_RUN_TIMEOUT)
}

/// Write the file (enabled) under `data_dir`, after the schedule crate's own
/// parser accepted it — a broken file would otherwise sit silently in the dir —
/// then ask the schedule actor to re-read the directory: the dir watcher is a
/// convenience, not something a study's start may depend on (no FSEvents under
/// some launch environments, 140-P live). No actor (tests) = the watcher's job.
pub async fn write(data_dir: &Path, slug: &str, text: &str) -> Result<()> {
    let name = name(slug);
    schedule::file::parse(&name, text)
        .map_err(|e| Error::refused(format!("schedule file {name}: {e}")))?;
    schedule::file::write_atomic(&data_dir.join(schedule::DIR), &name, text)?;
    if let Ok(actor) = schedule_actor() {
        actor
            .ask(schedule::Reconcile)
            .await
            .map_err(|e| Error::refused(format!("schedule reload: {e}")))?;
    }
    Ok(())
}

fn schedule_actor() -> Result<ActorRef<schedule::Schedule>> {
    ActorRef::<schedule::Schedule>::lookup(schedule::SCHEDULE_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| Error::refused("schedule actor not running"))
}

/// Flip `enabled:` through the schedule actor's `SetEnabled` (pause/resume, 140 §4.2).
pub async fn set_enabled(slug: &str, enabled: bool) -> Result<()> {
    schedule_actor()?
        .ask(schedule::SetEnabled {
            name: name(slug),
            enabled,
        })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    Ok(())
}

/// Fire the study's schedule once (`RunOnce`; 33 D6 `/schedules run`). A
/// paused study is enabled for the fire and disabled again right after;
/// `SetEnabled(true)` re-arms `every: 0s` to *now*, so a tick interleaved
/// before `Fire` may fire the loop itself — then `Fire` answers `busy` and
/// the run that started is the one we report.
pub async fn fire(slug: &str, paused: bool) -> Result<runtime::SessionId> {
    let sched = schedule_actor()?;
    let n = name(slug);
    if paused {
        let _ = sched
            .ask(schedule::SetEnabled {
                name: n.clone(),
                enabled: true,
            })
            .await;
    }
    let mut fired = sched
        .ask(schedule::Fire { name: n.clone() })
        .await
        .map_err(|e| Error::refused(e.to_string()));
    if paused {
        if matches!(&fired, Err(e) if e.to_string().ends_with("busy")) {
            fired = sched
                .ask(schedule::ListSchedules)
                .await
                .map_err(|e| Error::refused(e.to_string()))
                .and_then(|list| {
                    list.into_iter()
                        .find(|s| s.name == n && s.running)
                        .and_then(|s| s.session)
                        .ok_or_else(|| Error::refused("busy"))
                });
        }
        let _ = sched
            .ask(schedule::SetEnabled {
                name: n,
                enabled: false,
            })
            .await;
    }
    fired
}

/// End the loop from inside its run: the study file bound to `session` is
/// deleted (never `schedule::Done` — its "done (model): …" line ignores
/// `quiet`, and the study's own result card is the loud ending, 33 D4).
/// `Err` when `session` is not a running study schedule — then [`done`].
pub async fn done_run(session: runtime::SessionId, reason: &str) -> Result<()> {
    let sched = schedule_actor()?;
    let name = sched
        .ask(schedule::ListSchedules)
        .await
        .map_err(|e| Error::refused(e.to_string()))?
        .into_iter()
        .find(|s| s.session == Some(session) && s.name.starts_with(crate::SCHEDULE_PREFIX))
        .map(|s| s.name)
        .ok_or_else(|| Error::refused("not a running study schedule"))?;
    tracing::debug!(%name, "study schedule ended: {reason}");
    sched
        .ask(schedule::Delete { name })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    Ok(())
}

/// End the loop and delete the file through the schedule actor (140 §8.1).
pub async fn done(slug: &str, reason: &str) -> Result<()> {
    tracing::debug!(slug, "study schedule ended: {reason}");
    schedule_actor()?
        .ask(schedule::Delete { name: name(slug) })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    Ok(())
}

/// `autoresearch.enabled: false` (140 §12.2 "existing study files stay
/// paused"): flip `enabled: false` on every `schedules/autoresearch-*.md` so
/// the schedule plugin, which still runs, fires no study session nobody
/// confines or binds. Returns the names switched off. 182 S11: with paired
/// mesh `peers`, an unstamped (legacy) file is not this engine's to flip —
/// the switch-off would replicate to the lease holder that runs it.
pub fn disable_all(data_dir: &Path, peers: bool) -> std::io::Result<Vec<String>> {
    let dir = data_dir.join(schedule::DIR);
    let mut off = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(off);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("md")
            || !stem.starts_with(crate::SCHEDULE_PREFIX)
        {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        // 182 D6: another engine's study file is that engine's; flipping it
        // here would replicate the switch-off to the engine that runs it.
        let engine = schedule::file::ScheduleFile::read(stem, &text)
            .ok()
            .and_then(|f| f.engine);
        if engine
            .as_ref()
            .map_or(peers, |e| Some(e) != schedule::self_engine().as_ref())
        {
            continue;
        }
        let disabled = schedule::file::set_enabled(&text, false);
        if disabled != text {
            schedule::file::write_atomic(&dir, stem, &disabled)?;
            off.push(stem.to_owned());
        }
    }
    Ok(off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoresearchConfig;

    fn spec() -> StudySpec {
        StudySpec {
            slug: "sort-speed".into(),
            budget: crate::types::BudgetSpec {
                experiments: 120,
                per_experiment_usd: 1.5,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn run_timeout_formula_and_floor() {
        let cfg = AutoresearchConfig::default();
        // (2 + 2 + 2) × 600s + 0 + 20m + 15m = 60m + 35m = 95m.
        assert_eq!(run_timeout(&spec(), &cfg), Duration::from_secs(95 * 60));
        let mut quick = spec();
        quick.bench.timeout = "1s".into();
        let mut cfg2 = cfg.clone();
        cfg2.bench.wait = "1s".into();
        assert_eq!(run_timeout(&quick, &cfg2), MIN_RUN_TIMEOUT);
        let mut checked = spec();
        checked.checks = Some(crate::types::CommandSpec {
            command: "t".into(),
            timeout: "5m".into(),
        });
        assert_eq!(run_timeout(&checked, &cfg), Duration::from_secs(100 * 60));
    }

    #[test]
    fn budget_line_grammar() {
        assert_eq!(budget_line(1.5), "1.5 USD");
        assert_eq!(budget_line(20.0), "20 USD");
        assert_eq!(budget_line(0.25), "0.25 USD");
    }

    #[test]
    fn text_parses_with_the_schedule_crate() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AutoresearchConfig::default();
        let t = text(
            &spec(),
            &cfg,
            dir.path(),
            "openai/gpt-5.6-sol",
            &StartWhen::Now,
        );
        assert!(t.contains("every: 0s\n"), "{t}");
        assert!(t.contains("max_runs: 130\n"));
        assert!(t.contains("budget: 1.5 USD\n"));
        assert!(t.contains("run_timeout: 95m\n"));
        assert!(
            t.contains("approvals: false\n")
                && t.contains("quiet: true\n")
                && t.contains("runtime: clear\n")
        );
        assert!(t.contains("model: openai/gpt-5.6-sol\n") && t.contains("enabled: true\n"));
        assert!(
            !t.contains("memory:"),
            "memory: needs when: in the schedule parser"
        );
        let parsed = schedule::file::parse("autoresearch-sort-speed", &t).unwrap();
        assert_eq!(parsed.max_runs, Some(130));
        assert!(!parsed.approvals && parsed.quiet && parsed.enabled);
        assert_eq!(parsed.budget.unwrap().nanos, 1_500_000_000);
        assert_eq!(parsed.run_timeout, Some(Duration::from_secs(95 * 60)));
        assert!(parsed.prompt.contains("experiment_begin()"));

        let w = text(
            &spec(),
            &cfg,
            dir.path(),
            "m",
            &StartWhen::Window {
                sensor: "idle.py".into(),
            },
        );
        assert!(
            w.contains("every: 1m\n")
                && w.contains("memory: clear\n")
                && w.contains("when: \"idle.py\"\n")
        );
        assert!(
            schedule::file::parse("autoresearch-sort-speed", &w).is_ok(),
            "{w}"
        );
    }

    #[tokio::test]
    async fn write_refuses_a_broken_file_and_writes_a_good_one() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        assert!(
            write(&data, "x", "---\nevery: 0s\n---\n\nhi\n")
                .await
                .is_err(),
            "0s needs max_runs"
        );
        let cfg = AutoresearchConfig::default();
        let t = text(&spec(), &cfg, dir.path(), "", &StartWhen::Now);
        write(&data, "sort-speed", &t).await.unwrap();
        let on_disk =
            std::fs::read_to_string(data.join("schedules/autoresearch-sort-speed.md")).unwrap();
        assert_eq!(on_disk, t);
    }

    /// R-6: with the plugin disabled every study file is paused in place;
    /// other schedules and already-paused studies are untouched.
    #[tokio::test]
    async fn disable_all_pauses_study_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let cfg = AutoresearchConfig::default();
        let t = text(&spec(), &cfg, dir.path(), "", &StartWhen::Now);
        write(&data, "sort-speed", &t).await.unwrap();
        let mut other = spec();
        other.slug = "paused".into();
        let p = schedule::file::set_enabled(
            &text(&other, &cfg, dir.path(), "", &StartWhen::Now),
            false,
        );
        write(&data, "paused", &p).await.unwrap();
        let sched = data.join(schedule::DIR);
        std::fs::write(sched.join("dreaming.md"), "---\nevery: 1h\n---\nx\n").unwrap();

        assert!(
            disable_all(&data, true).unwrap().is_empty(),
            "182 S11: an engine with peers leaves unstamped study files alone"
        );
        let off = disable_all(&data, false).unwrap();
        assert_eq!(off, vec!["autoresearch-sort-speed".to_owned()]);
        let s = std::fs::read_to_string(sched.join("autoresearch-sort-speed.md")).unwrap();
        assert!(
            !schedule::file::parse("autoresearch-sort-speed", &s)
                .unwrap()
                .enabled
        );
        assert_eq!(
            std::fs::read_to_string(sched.join("dreaming.md")).unwrap(),
            "---\nevery: 1h\n---\nx\n"
        );
        assert!(disable_all(&data, false).unwrap().is_empty(), "idempotent");
        assert!(
            disable_all(&dir.path().join("none"), false)
                .unwrap()
                .is_empty()
        );
    }
}
