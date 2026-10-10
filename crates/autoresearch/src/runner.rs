//! W1 MEASURE — the bench runner (design doc 140 §5.1, §5.5, §8.5): owns the
//! benchmark process (never `shell()`), `/bin/bash -lc` in the worktree with
//! the `AUTORESEARCH_*` environment, its own process group, kill at
//! `bench.timeout`, output streamed through [`crate::scanner`] into
//! `runs/<n>/sample-<k>.log`; takes the bench slot and waits for the load
//! precondition; marks the sample `lost` when the engine dies.

use std::io::{Read as _, Seek as _};
use std::path::Path;
use std::time::{Duration, Instant};

use platform::{ProcessTree, Signal};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::scanner::{Scanned, Scanner};
use crate::slot;
use crate::types::{Sample, SampleRole, StudySpec};

/// Grace between `SIGTERM` and `SIGKILL` at a timeout or cancel (140 §5.1).
const KILL_GRACE: Duration = Duration::from_secs(2);
/// After the leader exits, how long a lingering pipe holder is drained before the group is killed.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// Tail shown to the model: lines and bytes (140 §5.1).
const TAIL_LINES: usize = 30;
const TAIL_BYTES: u64 = 4096;

/// What one sample needs besides the spec.
#[derive(Clone, Debug)]
pub struct SampleRequest<'a> {
    /// The study.
    pub slug: &'a str,
    /// Experiment number (0 for baseline/holdout outside an experiment).
    pub n: u32,
    /// Sample index.
    pub k: u32,
    /// The tree measured.
    pub tree_sha: &'a str,
    /// Role.
    pub role: SampleRole,
    /// cwd of the command.
    pub worktree: &'a Path,
    /// `evaluator/<segment>/` (custody copies; `AUTORESEARCH_EVALUATOR`).
    pub evaluator: &'a Path,
    /// `<root>/<slug>/runs/<n>/` for the log.
    pub runs_dir: &'a Path,
}

/// Run one scored sample of `spec.bench` (140 §5.1): load wait, then the bench
/// slot, one re-check of the load under the slot (§5.5; R-31: the slot is not
/// held while waiting for the machine), stream and scan, parse
/// `METRIC`/`CASE`/`ASI`, exit 0 with the primary present = a sample, anything
/// else = a crash sample (`value: None`, tail in the log). `cancel` kills the
/// process group.
pub async fn sample(
    spec: &StudySpec,
    cfg: &crate::config::Bench,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Sample> {
    let timeout = parse_duration(&spec.bench.timeout)?;
    let wait = parse_duration(&cfg.wait)?;
    let label = format!("bench:{}", req.slug);
    let waited = Instant::now();
    let mut held = slot::wait_for_load(
        cfg.max_load,
        slot::wait_budget(req.slug, req.n, wait),
        cancel,
    )
    .await?;
    slot::charge_wait(req.slug, req.n, waited.elapsed());
    let _slot = if spec.bench.exclusive {
        let s = slot::acquire(cfg, &label, &spec.bench.command, cancel).await?;
        // Another study's sample may have run while we queued: look once more.
        let waited = Instant::now();
        held = slot::wait_for_load(
            cfg.max_load,
            slot::wait_budget(req.slug, req.n, wait),
            cancel,
        )
        .await?;
        slot::charge_wait(req.slug, req.n, waited.elapsed());
        Some(s)
    } else {
        None
    };
    let load = slot::load();
    let wrapper = (spec.bench.exclusive && cfg.lock != slot::BUILTIN).then_some(cfg.lock.as_str());

    std::fs::create_dir_all(req.runs_dir)?;
    let log = req.runs_dir.join(sample_log_name(req.k, req.role));
    let run = run(
        &spec.bench.command,
        wrapper,
        timeout,
        &req,
        &log,
        cancel,
        &[],
    )
    .await?;
    let mut scanned = run.scanned;
    let primary = scanned.metrics.remove(&spec.metric.name);
    // 159 W-m: `SPEND` is self-reported; a sample whose reported spend passes the
    // per-experiment cap is a crash, like a bad number.
    let cap_nanos = (spec.budget.per_experiment_usd * 1e9) as u64;
    let over_cap = spec.budget.per_experiment_usd > 0.0 && scanned.spend_nanos > cap_nanos;
    let value = if run.exit == Some(0) && scanned.bad_number.is_none() && !over_cap {
        primary
    } else {
        None
    };
    // R-41: why a finished run is not a sample, for the verdict line.
    let failure = value
        .is_none()
        .then(|| match (run.exit, &scanned.bad_number) {
            (Some(0), Some(b)) => format!("bad number {b}"),
            (Some(0), None) if over_cap => format!(
                "bench reported SPEND ${:.2} over per_experiment_usd ${:.2}",
                scanned.spend_nanos as f64 / 1e9,
                spec.budget.per_experiment_usd
            ),
            (Some(0), None) => format!("no METRIC {} line", spec.metric.name),
            (Some(e), _) => format!("exit {e}"),
            (None, _) => "killed".to_owned(),
        });
    Ok(Sample {
        n: req.n,
        k: req.k,
        tree_sha: req.tree_sha.to_owned(),
        role: Some(req.role),
        value,
        secondaries: scanned.metrics,
        cases: scanned.cases,
        asi: scanned.asi,
        exit: run.exit,
        wall_ms: run.wall_ms,
        load,
        noisy: !held,
        lost: false,
        log_path: Some(root_relative(&log)),
        // F4A's segment cut is by `at` string order at ms resolution (`store::now`); a
        // whole-second stamp here would put a sub-second bench's neighbours on the cut
        // (140-P2 live: `02:26:16Z` on every finished sample, the pending row had ms).
        at: crate::store::now(),
        slot_holders: slot::build_slot_holders(),
        failure,
        case_values: scanned.case_values,
        spend_nanos: scanned.spend_nanos,
    })
}

/// `sample-<k>.log` for the candidate; `sample-<k>-<role>.log` for an
/// incumbent / baseline / holdout / final sample sharing the run (R-23).
fn sample_log_name(k: u32, role: SampleRole) -> String {
    match role {
        SampleRole::Candidate => format!("sample-{k}.log"),
        other => format!("sample-{k}-{}.log", role_tag(other)),
    }
}

/// The role's ledger tag for the log name.
fn role_tag(role: SampleRole) -> &'static str {
    match role {
        SampleRole::Candidate => "candidate",
        SampleRole::Incumbent => "incumbent",
        SampleRole::Baseline => "baseline",
        SampleRole::Holdout => "holdout",
        SampleRole::Final => "final",
    }
}

/// Run `command` (checks, smoke, warm, holdout) with the same environment and
/// its own timeout; returns `(exit, tail, wall_ms)` (140 §5.1 Checks / Smoke tier).
pub async fn command(
    command: &str,
    timeout: Duration,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<(Option<i32>, String, u64)> {
    std::fs::create_dir_all(req.runs_dir)?;
    let log = req.runs_dir.join(format!("command-{}.log", req.k));
    let run = run(command, None, timeout, &req, &log, cancel, &[]).await?;
    Ok((run.exit, tail(&log), run.wall_ms))
}

/// 159 §5.10: what a plugin-side command (`Backend::plugin_command`: hooks, deploy, measure,
/// rollback) produced — the exit, the log tail and the output scanned with the v2 grammar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandRun {
    /// Process exit; `None` when killed.
    pub exit: Option<i32>,
    /// Last 30 lines / 4 KiB of the log.
    pub tail: String,
    /// Wall time.
    pub wall_ms: u64,
    /// `METRIC`, `CASE`, `SPEND`, `ASI` lines.
    pub scanned: Scanned,
}

/// [`command`] with `extra_env` on top of the study environment, scanned; the log is
/// `runs_dir/<what>-<k>.log`.
pub async fn command_run(
    what: &str,
    command: &str,
    timeout: Duration,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
    extra_env: &[(String, String)],
) -> Result<CommandRun> {
    std::fs::create_dir_all(req.runs_dir)?;
    let log = req.runs_dir.join(format!("{what}-{}.log", req.k));
    let run = run(command, None, timeout, &req, &log, cancel, extra_env).await?;
    Ok(CommandRun {
        exit: run.exit,
        tail: tail(&log),
        wall_ms: run.wall_ms,
        scanned: run.scanned,
    })
}

/// Last 30 lines / 4 KiB of `log` for the model (140 §5.1).
pub fn tail(log: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(log) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let mut text = String::new();
    if len > TAIL_BYTES
        && file
            .seek(std::io::SeekFrom::Start(len - TAIL_BYTES))
            .is_err()
    {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    text.push_str(&String::from_utf8_lossy(&bytes));
    let mut lines: Vec<&str> = text.lines().collect();
    if len > TAIL_BYTES && !lines.is_empty() {
        lines.remove(0); // a fragment cut mid-line
    }
    let skip = lines.len().saturating_sub(TAIL_LINES);
    lines[skip..].join("\n")
}

/// A humantime duration (`600s`, `20m`) as the spec and config write them.
pub fn parse_duration(s: &str) -> Result<Duration> {
    humantime::parse_duration(s.trim()).map_err(|e| Error::refused(format!("duration `{s}`: {e}")))
}

/// Outcome of one process run.
struct Run {
    exit: Option<i32>,
    wall_ms: u64,
    scanned: Scanned,
}

/// Spawn `command` as the plugin (140 §5.1): custody-resolved tokens, optional
/// lock wrapper, `/bin/bash -lc`, cwd = worktree, own process group, stdin null,
/// stdout+stderr streamed into `log` and the scanner; TERM then KILL at
/// `timeout` or on `cancel`.
async fn run(
    command: &str,
    wrapper: Option<&str>,
    timeout: Duration,
    req: &SampleRequest<'_>,
    log: &Path,
    cancel: &CancellationToken,
    extra_env: &[(String, String)],
) -> Result<Run> {
    let resolved = resolve(command, req.slug, req.evaluator);
    let script = match wrapper {
        Some(w) => format!("{w} /bin/bash -lc {}", quote(&resolved)),
        None => resolved,
    };
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.arg("-lc").arg(&script).current_dir(req.worktree);
    // Own process group on Unix (R-27); `process_group` is Unix-only in tokio, and on other
    // targets `ProcessTree::adopt` below is what holds the child's tree.
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("AUTORESEARCH", "1")
        .env("AUTORESEARCH_STUDY", req.slug)
        .env("AUTORESEARCH_RUN", req.n.to_string())
        .env("AUTORESEARCH_SAMPLE", req.k.to_string())
        .env("AUTORESEARCH_WORKTREE", req.worktree)
        .env("AUTORESEARCH_EVALUATOR", req.evaluator)
        .env("GIT_TERMINAL_PROMPT", "0");
    if wrapper.is_some() {
        cmd.env("BUILD_SLOTS", "1")
            .env("BUILD_LABEL", format!("bench:{}", req.slug));
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let mut child = cmd.spawn()?;
    // R-27: the group dies with this guard — an early `?`, a dropped future
    // (the tool call cancelled, the engine shutting down) must not leave a
    // benchmark running on the shared machine beside the next sample.
    let mut group = Group(child.id().and_then(|pid| ProcessTree::adopt(pid).ok()));

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, tx.clone()));
    }
    drop(tx);

    let mut file = tokio::fs::File::create(log).await?;
    let mut scanner = Scanner::new();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let drain = tokio::time::sleep(Duration::MAX);
    tokio::pin!(drain);
    let mut status: Option<std::process::ExitStatus> = None;
    let mut killed = false;
    let mut eof = false;
    loop {
        tokio::select! {
            chunk = rx.recv(), if !eof => match chunk {
                Some(c) => {
                    file.write_all(&c).await?;
                    scanner.feed(&c);
                }
                // R-22: pipe EOF (`exec >bench.log 2>&1`) is not the end of the
                // run — keep waiting for the leader under the same deadline.
                None => eof = true,
            },
            s = child.wait(), if status.is_none() => {
                status = Some(s?);
                if eof { break; }
                drain.as_mut().reset(tokio::time::Instant::now() + DRAIN_GRACE);
            }
            // R-29: a leader that already exited cannot time out; only pipe
            // holders remain, and the drain below deals with them.
            () = &mut deadline, if !killed && status.is_none() => {
                killed = true;
                let _ = file.write_all(format!("\n[autoresearch] timeout after {timeout:?}: process group killed\n").as_bytes()).await;
                group.terminate();
            }
            () = cancel.cancelled(), if !killed => {
                killed = true;
                let _ = file.write_all(b"\n[autoresearch] cancelled: process group killed\n").await;
                group.terminate();
            }
            () = &mut drain, if status.is_some() => {
                // The leader is gone but something still holds the pipe: stop waiting for it.
                break;
            }
        }
    }
    let status = match status {
        Some(s) => s,
        None => child.wait().await?,
    };
    // Whatever is left of the group (pipe holders, grandchildren) goes with the sample.
    group.disarm_and_kill();
    file.flush().await?;
    let exit = if killed { None } else { status.code() };
    Ok(Run {
        exit,
        wall_ms: started.elapsed().as_millis() as u64,
        scanned: scanner.finish(),
    })
}

/// The benchmark's process group: `TERM` then `KILL` after [`KILL_GRACE`] on
/// a timeout or cancel; killed outright when dropped unless the run completed.
struct Group(Option<ProcessTree>);

impl Group {
    fn terminate(&self) {
        if let Some(t) = self.0.clone() {
            let _ = t.signal(Signal::Term);
            tokio::spawn(async move {
                tokio::time::sleep(KILL_GRACE).await;
                t.kill();
            });
        }
    }

    /// The run is over: kill what remains now and stop guarding.
    fn disarm_and_kill(&mut self) {
        if let Some(t) = self.0.take() {
            t.kill();
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(t) = self.0.take() {
            t.kill();
        }
    }
}

/// Copy a pipe into the channel in 64 KiB chunks until EOF.
async fn pump<R: tokio::io::AsyncRead + Unpin>(mut r: R, tx: tokio::sync::mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Every whitespace token that names a file in custody (`bench.sh`, `./bench.sh`,
/// `<root>/drafts/<slug>/bench.sh`) is replaced by that copy's absolute path,
/// shell-quoted (140 §4.1 "Where the evaluator lives", §5.1; R-8, R-17).
fn resolve(command: &str, slug: &str, evaluator: &Path) -> String {
    let drafts = crate::custody::drafts_dir(evaluator, slug);
    command
        .split(' ')
        .map(|tok| match crate::custody::command_path(tok, &drafts) {
            Some(p) if evaluator.join(p.rel()).is_file() => {
                quote(&evaluator.join(p.rel()).to_string_lossy())
            }
            _ => tok.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Single-quote `s` for `bash -c`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `<slug>/runs/<n>/…` from the absolute log path (the `samples.log_path`
/// column): everything from the component before `runs`, however deep the
/// log sits (`runs/<n>/holdout/sample-0-holdout.log` included, R-37).
fn root_relative(log: &Path) -> String {
    let parts: Vec<String> = log
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let start = parts
        .iter()
        .rposition(|c| c == "runs")
        .and_then(|i| i.checked_sub(1))
        .unwrap_or(parts.len().saturating_sub(4));
    parts[start..].join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench_cfg() -> crate::config::Bench {
        crate::config::Bench {
            lock: slot::BUILTIN.into(),
            max_load: f64::INFINITY,
            wait: "1s".into(),
        }
    }

    fn spec(command: &str, timeout: &str) -> StudySpec {
        let mut s = StudySpec {
            slug: "toy".into(),
            ..StudySpec::default()
        };
        s.metric.name = "time_ms".into();
        s.bench.command = command.into();
        s.bench.timeout = timeout.into();
        s.bench.exclusive = false;
        s
    }

    struct Dirs {
        _tmp: tempfile::TempDir,
        worktree: std::path::PathBuf,
        evaluator: std::path::PathBuf,
        runs: std::path::PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().unwrap();
        let d = Dirs {
            worktree: tmp.path().join("toy/worktree"),
            evaluator: tmp.path().join("toy/evaluator/0"),
            runs: tmp.path().join("toy/runs/3"),
            _tmp: tmp,
        };
        std::fs::create_dir_all(&d.worktree).unwrap();
        std::fs::create_dir_all(&d.evaluator).unwrap();
        d
    }

    fn req<'a>(d: &'a Dirs, k: u32) -> SampleRequest<'a> {
        SampleRequest {
            slug: "toy",
            n: 3,
            k,
            tree_sha: "abc",
            role: SampleRole::Candidate,
            worktree: &d.worktree,
            evaluator: &d.evaluator,
            runs_dir: &d.runs,
        }
    }

    #[tokio::test]
    async fn a_good_sample_parses_metrics_and_sees_the_environment() {
        let d = dirs();
        let s = spec(
            "echo METRIC time_ms=41.5; echo METRIC peak_kb=1024; echo CASE n500 time_ms=3; echo ASI run=$AUTORESEARCH_RUN-$AUTORESEARCH_SAMPLE-$AUTORESEARCH; echo \"ASI cwd=$PWD\"; echo noise >&2",
            "10s",
        );
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.value, Some(41.5));
        assert_eq!(out.exit, Some(0));
        assert_eq!(out.secondaries["peak_kb"], 1024.0);
        assert_eq!(out.cases["n500"], 3.0);
        assert_eq!(out.asi["run"], "3-1-1");
        assert_eq!(
            std::fs::canonicalize(&out.asi["cwd"]).unwrap(),
            std::fs::canonicalize(&d.worktree).unwrap()
        );
        assert!(!out.noisy);
        assert!(
            out.at.len() == 24 && out.at.as_bytes()[19] == b'.' && out.at.ends_with('Z'),
            "{}",
            out.at
        );
        assert_eq!(out.log_path.as_deref(), Some("toy/runs/3/sample-1.log"));
        let log = std::fs::read_to_string(d.runs.join("sample-1.log")).unwrap();
        assert!(log.contains("noise"), "{log}");
        assert!(log.contains("METRIC time_ms=41.5"));
    }

    #[tokio::test]
    async fn crashes_are_samples_without_a_value() {
        let d = dirs();
        let s = spec("echo METRIC time_ms=5; echo boom >&2; exit 3", "10s");
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.value, None);
        assert_eq!(out.exit, Some(3));
        assert!(tail(&d.runs.join("sample-1.log")).contains("boom"));
        // Missing primary and a NaN primary are crashes too.
        let out = sample(
            &spec("echo METRIC other=5", "10s"),
            &bench_cfg(),
            req(&d, 2),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!((out.value, out.exit), (None, Some(0)));
        let out = sample(
            &spec("echo METRIC time_ms=nan", "10s"),
            &bench_cfg(),
            req(&d, 3),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.value, None);
    }

    /// 159 §5.3 / W-m: `SPEND` lines sum into the sample, `CASE` v2 names are kept, a reported
    /// spend past `per_experiment_usd` crashes the sample, and a bad `SPEND` is a bad number.
    #[tokio::test]
    async fn spend_sums_and_the_cap_crashes() {
        let d = dirs();
        let mut s = spec(
            "echo METRIC time_ms=5; echo SPEND 0.25 judge; echo SPEND 0.5; echo CASE c1 time_ms=1 pass=1",
            "10s",
        );
        s.budget.per_experiment_usd = 1.0;
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.value, Some(5.0));
        assert_eq!(out.spend_nanos, 750_000_000);
        assert_eq!(out.cases["c1"], 1.0);
        assert_eq!(out.case_values["c1"]["pass"], 1.0);
        s.budget.per_experiment_usd = 0.5;
        let out = sample(&s, &bench_cfg(), req(&d, 2), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!((out.value, out.exit), (None, Some(0)));
        assert!(
            out.failure
                .as_deref()
                .unwrap_or("")
                .contains("over per_experiment_usd"),
            "{:?}",
            out.failure
        );
        let out = sample(
            &spec("echo METRIC time_ms=5; echo SPEND lots", "10s"),
            &bench_cfg(),
            req(&d, 3),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.value, None);
        assert!(
            out.failure
                .as_deref()
                .unwrap_or("")
                .starts_with("bad number SPEND")
        );
    }

    #[tokio::test]
    async fn timeout_kills_the_whole_group() {
        let d = dirs();
        let s = spec(
            "sleep 30 & echo PID=$!; sleep 30; echo METRIC time_ms=1",
            "300ms",
        );
        let t = Instant::now();
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new())
            .await
            .unwrap();
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        assert_eq!(out.value, None);
        assert_eq!(out.exit, None);
        let log = std::fs::read_to_string(d.runs.join("sample-1.log")).unwrap();
        assert!(log.contains("timeout after 300ms"), "{log}");
        let pid: u32 = log
            .lines()
            .find_map(|l| l.strip_prefix("PID="))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        tokio::time::sleep(KILL_GRACE + Duration::from_millis(300)).await;
        assert!(
            !platform::process::pid_alive(pid),
            "background child {pid} survived"
        );
    }

    #[tokio::test]
    async fn cancel_kills_the_process() {
        let d = dirs();
        let s = spec("sleep 30; echo METRIC time_ms=1", "30s");
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let t = Instant::now();
        let out = sample(&s, &bench_cfg(), req(&d, 1), &cancel).await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(10));
        assert_eq!(out.exit, None);
        assert!(
            std::fs::read_to_string(d.runs.join("sample-1.log"))
                .unwrap()
                .contains("cancelled")
        );
    }

    #[tokio::test]
    async fn custody_copy_wins_over_the_worktree_script() {
        let d = dirs();
        std::fs::write(
            d.worktree.join("bench.sh"),
            "echo METRIC time_ms=1; echo ASI who=worktree",
        )
        .unwrap();
        std::fs::write(
            d.evaluator.join("bench.sh"),
            "echo METRIC time_ms=2; echo ASI who=custody",
        )
        .unwrap();
        let out = sample(
            &spec("bash bench.sh", "10s"),
            &bench_cfg(),
            req(&d, 1),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.value, Some(2.0));
        assert_eq!(out.asi["who"], "custody");
    }

    /// R-8/R-17: `./bench.sh` and the draft's absolute path resolve to the
    /// custody copy, quoted — a space in the data dir does not break the
    /// command (an absolute token cannot itself contain a space: it is split
    /// like bash would split it, and custody then refuses the study at Start).
    #[tokio::test]
    async fn dot_slash_and_draft_paths_resolve_into_custody_quoted() {
        for (sub, absolute) in [
            ("data dir/autoresearch", false),
            ("data/autoresearch", true),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join(sub);
            let worktree = root.join("toy/worktree");
            let evaluator = root.join("toy/evaluator/0");
            let runs = root.join("toy/runs/3");
            std::fs::create_dir_all(&worktree).unwrap();
            std::fs::create_dir_all(&evaluator).unwrap();
            std::fs::write(worktree.join("bench.sh"), "echo METRIC time_ms=1").unwrap();
            std::fs::write(evaluator.join("bench.sh"), "echo METRIC time_ms=2").unwrap();
            let req = SampleRequest {
                slug: "toy",
                n: 3,
                k: 1,
                tree_sha: "abc",
                role: SampleRole::Candidate,
                worktree: &worktree,
                evaluator: &evaluator,
                runs_dir: &runs,
            };
            let cmd = if absolute {
                format!("bash {}", root.join("drafts/toy/bench.sh").display())
            } else {
                "bash ./bench.sh".to_owned()
            };
            let out = sample(
                &spec(&cmd, "10s"),
                &bench_cfg(),
                req,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
            assert_eq!(out.value, Some(2.0), "{cmd}");
            let r = resolve("bash ./bench.sh --n 3", "toy", &evaluator);
            assert_eq!(
                r,
                format!("bash '{}' --n 3", evaluator.join("bench.sh").display())
            );
        }
    }

    /// R-22: a benchmark that redirects its output closes the pipes at once;
    /// the run must still end at the deadline, not hang on the leader.
    #[tokio::test]
    async fn redirected_output_still_times_out() {
        let d = dirs();
        let started = Instant::now();
        let out = sample(
            &spec(
                "exec >/dev/null 2>&1; sleep 30; echo METRIC time_ms=1",
                "1s",
            ),
            &bench_cfg(),
            req(&d, 1),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.exit, None);
        assert!(out.value.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    /// R-23/R-37: an incumbent sample sharing the run gets its own log; a
    /// holdout log under `runs/<n>/holdout/` keeps the slug in `log_path`.
    #[tokio::test]
    async fn role_logs_and_root_relative_paths() {
        let d = dirs();
        let mut r = req(&d, 1);
        r.role = SampleRole::Incumbent;
        let out = sample(
            &spec("echo METRIC time_ms=5", "10s"),
            &bench_cfg(),
            r,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            out.log_path.as_deref(),
            Some("toy/runs/3/sample-1-incumbent.log")
        );
        assert!(d.runs.join("sample-1-incumbent.log").is_file());
        assert_eq!(
            root_relative(Path::new(
                "/x/y/autoresearch/toy/runs/7/holdout/sample-0-holdout.log"
            )),
            "toy/runs/7/holdout/sample-0-holdout.log"
        );
        assert_eq!(root_relative(Path::new("/a/b/c/d/e.log")), "b/c/d/e.log");
    }

    #[tokio::test]
    async fn checks_command_returns_exit_and_tail() {
        let d = dirs();
        let (exit, tail, wall) = command(
            "for i in $(seq 1 50); do echo line $i; done; exit 1",
            Duration::from_secs(10),
            req(&d, 0),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(exit, Some(1));
        assert_eq!(tail.lines().count(), 30);
        assert!(tail.starts_with("line 21"));
        assert!(tail.ends_with("line 50"));
        assert!(wall < 10_000);
        let (exit, ..) = command(
            "true",
            Duration::from_secs(10),
            req(&d, 1),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(exit, Some(0));
    }

    #[test]
    fn tail_is_bounded_by_bytes_and_drops_the_cut_line() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("big.log");
        let line = "x".repeat(1000);
        let text: String = (0..10).map(|i| format!("{i}{line}\n")).collect();
        std::fs::write(&p, text).unwrap();
        let t = tail(&p);
        assert_eq!(t.lines().count(), 4);
        assert!(t.starts_with('6'), "{}", &t[..5]);
        assert!(t.len() <= TAIL_BYTES as usize);
        assert_eq!(tail(&tmp.path().join("missing.log")), "");
    }

    #[test]
    fn durations_and_quoting() {
        assert_eq!(parse_duration("600s").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration(" 20m ").unwrap(), Duration::from_secs(1200));
        assert!(parse_duration("soon").is_err());
        assert_eq!(quote("echo 'hi' there"), "'echo '\\''hi'\\'' there'");
    }
}
