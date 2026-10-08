//! W3 LOOP — the study's schedule file (design doc 140 §8.1):
//! `<data_dir>/schedules/autoresearch-<slug>.md` with `every: 0s`,
//! `max_runs`, `budget:`, `run_timeout:`, `approvals: false`, `quiet: true`,
//! `runtime: clear`, `cwd:`, `model:`, `enabled:` (and `when:` + `every: 1m`
//! + `memory: clear` with a `window:`). The plugin rewrites only `enabled:`
//! and deletes the file through the schedule actor when the study stops.
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
pub fn text(spec: &StudySpec, cfg: &crate::config::AutoresearchConfig, worktree: &Path, model: &str, start: &StartWhen) -> String {
    let mut s = String::from("---\n");
    let sensor = match start {
        StartWhen::Window { sensor } => Some(sensor.as_str()),
        StartWhen::Now => spec.window.as_deref(),
    }
    .map(str::trim)
    .filter(|w| !w.is_empty());
    // A sensor needs a cadence (40 §2: `when:` wants `every:` ≥ 30 s).
    s.push_str(if sensor.is_some() { "every: 1m\n" } else { "every: 0s\n" });
    s.push_str(&format!("max_runs: {}\n", spec.budget.experiments.saturating_add(EXTRA_RUNS).max(1)));
    s.push_str(&format!("budget: {}\n", budget_line(spec.budget.per_experiment_usd)));
    s.push_str(&format!("run_timeout: {}m\n", run_timeout(spec, cfg).as_secs().div_ceil(60)));
    s.push_str("approvals: false\n");
    s.push_str("quiet: true\n");
    if sensor.is_some() {
        s.push_str("memory: clear\n");
    }
    s.push_str("runtime: clear\n");
    s.push_str(&format!("cwd: {}\n", yaml_str(&worktree.display().to_string())));
    if !model.trim().is_empty() {
        s.push_str(&format!("model: {}\n", model.trim()));
    }
    s.push_str("enabled: true\n");
    if let Some(w) = sensor {
        s.push_str(&format!("when: {}\n", yaml_str(w)));
    }
    s.push_str("---\n\n");
    s.push_str(&format!(
        "Run the next experiment of autoresearch study `{}`. Call experiment_begin() first and follow the protocol \
         in the brief it returns. Do not ask questions; nobody is watching this session.\n",
        spec.slug
    ));
    s
}

/// A double-quoted YAML scalar.
fn yaml_str(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `run_timeout` = `(crash_retries + confirm_runs + 2) × bench.timeout + checks.timeout + bench.wait + 15m`, at least 30 m.
pub fn run_timeout(spec: &StudySpec, cfg: &crate::config::AutoresearchConfig) -> Duration {
    let bench = duration(&spec.bench.timeout, Duration::from_secs(600));
    let checks = spec
        .checks
        .as_ref()
        .map_or(Duration::ZERO, |c| duration(&c.timeout, Duration::from_secs(300)));
    let wait = duration(&cfg.bench.wait, Duration::from_secs(20 * 60));
    let samples = u32::from(cfg.defaults.crash_retries) + u32::from(cfg.gate.confirm_runs) + 2;
    (bench * samples + checks + wait + SLACK).max(MIN_RUN_TIMEOUT)
}

/// Write the file (enabled) under `data_dir`, after the schedule crate's own
/// parser accepted it — a broken file would otherwise sit silently in the dir.
pub fn write(data_dir: &Path, slug: &str, text: &str) -> Result<()> {
    let name = name(slug);
    schedule::file::parse(&name, text).map_err(|e| Error::refused(format!("schedule file {name}: {e}")))?;
    schedule::file::write_atomic(&data_dir.join(schedule::DIR), &name, text)?;
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
        .ask(schedule::SetEnabled { name: name(slug), enabled })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    Ok(())
}

/// Fire the study's schedule once (`RunOnce`; 33 D6 `/schedules run`). A
/// paused study is enabled for the fire and disabled again right after.
pub async fn fire(slug: &str, paused: bool) -> Result<runtime::SessionId> {
    let sched = schedule_actor()?;
    let n = name(slug);
    if paused {
        let _ = sched.ask(schedule::SetEnabled { name: n.clone(), enabled: true }).await;
    }
    let fired = sched
        .ask(schedule::Fire { name: n.clone() })
        .await
        .map_err(|e| Error::refused(e.to_string()));
    if paused {
        let _ = sched.ask(schedule::SetEnabled { name: n, enabled: false }).await;
    }
    fired
}

/// End the loop from inside its run through `schedule::Done` (33 §3.5); the
/// file is removed by the schedule actor. `Err` when `session` is not a
/// running schedule session (the run already settled) — then [`done`].
pub async fn done_run(session: runtime::SessionId, reason: &str) -> Result<()> {
    schedule_actor()?
        .ask(schedule::Done { session, why: reason.to_owned() })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    Ok(())
}

/// End the loop and delete the file through the schedule actor (140 §8.1).
/// Tries `Done` on the study's bound session when one is given, else
/// `Delete` (the same effect once the run has settled).
pub async fn done(slug: &str, reason: &str) -> Result<()> {
    let _ = reason;
    schedule_actor()?
        .ask(schedule::Delete { name: name(slug) })
        .await
        .map_err(|e| Error::refused(e.to_string()))?;
    Ok(())
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
        checked.checks = Some(crate::types::CommandSpec { command: "t".into(), timeout: "5m".into() });
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
        let t = text(&spec(), &cfg, dir.path(), "openai/gpt-5.6-sol", &StartWhen::Now);
        assert!(t.contains("every: 0s\n"), "{t}");
        assert!(t.contains("max_runs: 130\n"));
        assert!(t.contains("budget: 1.5 USD\n"));
        assert!(t.contains("run_timeout: 95m\n"));
        assert!(t.contains("approvals: false\n") && t.contains("quiet: true\n") && t.contains("runtime: clear\n"));
        assert!(t.contains("model: openai/gpt-5.6-sol\n") && t.contains("enabled: true\n"));
        assert!(!t.contains("memory:"), "memory: needs when: in the schedule parser");
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
            &StartWhen::Window { sensor: "sensors/idle.py".into() },
        );
        assert!(w.contains("every: 1m\n") && w.contains("memory: clear\n") && w.contains("when: \"sensors/idle.py\"\n"));
        assert!(schedule::file::parse("autoresearch-sort-speed", &w).is_ok(), "{w}");
    }

    #[test]
    fn write_refuses_a_broken_file_and_writes_a_good_one() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        assert!(write(&data, "x", "---\nevery: 0s\n---\n\nhi\n").is_err(), "0s needs max_runs");
        let cfg = AutoresearchConfig::default();
        let t = text(&spec(), &cfg, dir.path(), "", &StartWhen::Now);
        write(&data, "sort-speed", &t).unwrap();
        let on_disk = std::fs::read_to_string(data.join("schedules/autoresearch-sort-speed.md")).unwrap();
        assert_eq!(on_disk, t);
    }
}
