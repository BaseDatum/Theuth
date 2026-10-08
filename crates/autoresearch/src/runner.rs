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

/// Run one scored sample of `spec.bench` (140 §5.1): bench slot + load wait
/// (§5.5), stream and scan, parse `METRIC`/`CASE`/`ASI`, exit 0 with the
/// primary present = a sample, anything else = a crash sample (`value: None`,
/// tail in the log). `cancel` kills the process group.
pub async fn sample(
    spec: &StudySpec,
    cfg: &crate::config::Bench,
    req: SampleRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Sample> {
    let timeout = parse_duration(&spec.bench.timeout)?;
    let wait = parse_duration(&cfg.wait)?;
    let label = format!("bench:{}", req.slug);
    let _slot = if spec.bench.exclusive {
        Some(slot::acquire(cfg, &label, cancel).await?)
    } else {
        None
    };
    let waited = Instant::now();
    let held = slot::wait_for_load(cfg.max_load, slot::wait_budget(req.slug, req.n, wait), cancel).await?;
    slot::charge_wait(req.slug, req.n, waited.elapsed());
    let load = slot::load();
    let wrapper = (spec.bench.exclusive && cfg.lock != slot::BUILTIN).then(|| cfg.lock.as_str());

    std::fs::create_dir_all(req.runs_dir)?;
    let log = req.runs_dir.join(format!("sample-{}.log", req.k));
    let run = run(&spec.bench.command, wrapper, timeout, &req, &log, cancel).await?;
    let mut scanned = run.scanned;
    let primary = scanned.metrics.remove(&spec.metric.name);
    let value = if run.exit == Some(0) && scanned.bad_number.is_none() {
        primary
    } else {
        None
    };
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
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
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
    let run = run(command, None, timeout, &req, &log, cancel).await?;
    Ok((run.exit, tail(&log), run.wall_ms))
}

/// Last 30 lines / 4 KiB of `log` for the model (140 §5.1).
pub fn tail(log: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(log) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let mut text = String::new();
    if len > TAIL_BYTES {
        if file.seek(std::io::SeekFrom::Start(len - TAIL_BYTES)).is_err() {
            return String::new();
        }
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
) -> Result<Run> {
    let resolved = resolve(command, req.evaluator);
    let script = match wrapper {
        Some(w) => format!("{w} /bin/bash -lc {}", quote(&resolved)),
        None => resolved,
    };
    let mut cmd = tokio::process::Command::new("/bin/bash");
    cmd.arg("-lc")
        .arg(&script)
        .current_dir(req.worktree)
        .process_group(0)
        .stdin(std::process::Stdio::null())
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
        cmd.env("BUILD_SLOTS", "1").env("BUILD_LABEL", format!("bench:{}", req.slug));
    }
    let started = Instant::now();
    let mut child = cmd.spawn()?;
    let tree = child.id().and_then(|pid| ProcessTree::adopt(pid).ok());
    let kill = |tree: Option<ProcessTree>| {
        if let Some(t) = tree {
            let _ = t.signal(Signal::Term);
            tokio::spawn(async move {
                tokio::time::sleep(KILL_GRACE).await;
                t.kill();
            });
        }
    };

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
    loop {
        tokio::select! {
            chunk = rx.recv() => match chunk {
                Some(c) => {
                    file.write_all(&c).await?;
                    scanner.feed(&c);
                }
                None => break,
            },
            s = child.wait(), if status.is_none() => {
                status = Some(s?);
                drain.as_mut().reset(tokio::time::Instant::now() + DRAIN_GRACE);
            }
            () = &mut deadline, if !killed => {
                killed = true;
                let _ = file.write_all(format!("\n[autoresearch] timeout after {timeout:?}: process group killed\n").as_bytes()).await;
                kill(tree);
            }
            () = cancel.cancelled(), if !killed => {
                killed = true;
                let _ = file.write_all(b"\n[autoresearch] cancelled: process group killed\n").await;
                kill(tree);
            }
            () = &mut drain, if status.is_some() => {
                // The leader is gone but something still holds the pipe: stop waiting for it.
                if let Some(t) = tree { t.kill(); }
                break;
            }
        }
    }
    if status.is_none() {
        status = Some(child.wait().await?);
    }
    file.flush().await?;
    let exit = status.and_then(|s| if killed { None } else { s.code() });
    Ok(Run {
        exit,
        wall_ms: started.elapsed().as_millis() as u64,
        scanned: scanner.finish(),
    })
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

/// Every whitespace token that names a file in custody is replaced by that
/// copy's absolute path (140 §4.1 "Where the evaluator lives", §5.1).
fn resolve(command: &str, evaluator: &Path) -> String {
    command
        .split(' ')
        .map(|tok| match crate::custody::relative(tok) {
            Some(rel) if evaluator.join(rel).is_file() => evaluator.join(rel).to_string_lossy().into_owned(),
            _ => tok.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Single-quote `s` for `bash -c`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `<slug>/runs/<n>/<file>` from the absolute log path (the `samples.log_path` column).
fn root_relative(log: &Path) -> String {
    let parts: Vec<_> = log.components().rev().take(4).collect();
    parts
        .iter()
        .rev()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
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
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new()).await.unwrap();
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
        assert_eq!(out.log_path.as_deref(), Some("toy/runs/3/sample-1.log"));
        let log = std::fs::read_to_string(d.runs.join("sample-1.log")).unwrap();
        assert!(log.contains("noise"), "{log}");
        assert!(log.contains("METRIC time_ms=41.5"));
    }

    #[tokio::test]
    async fn crashes_are_samples_without_a_value() {
        let d = dirs();
        let s = spec("echo METRIC time_ms=5; echo boom >&2; exit 3", "10s");
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new()).await.unwrap();
        assert_eq!(out.value, None);
        assert_eq!(out.exit, Some(3));
        assert!(tail(&d.runs.join("sample-1.log")).contains("boom"));
        // Missing primary and a NaN primary are crashes too.
        let out = sample(&spec("echo METRIC other=5", "10s"), &bench_cfg(), req(&d, 2), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!((out.value, out.exit), (None, Some(0)));
        let out = sample(&spec("echo METRIC time_ms=nan", "10s"), &bench_cfg(), req(&d, 3), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.value, None);
    }

    #[tokio::test]
    async fn timeout_kills_the_whole_group() {
        let d = dirs();
        let s = spec("sleep 30 & echo PID=$!; sleep 30; echo METRIC time_ms=1", "300ms");
        let t = Instant::now();
        let out = sample(&s, &bench_cfg(), req(&d, 1), &CancellationToken::new()).await.unwrap();
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
        assert!(!platform::process::pid_alive(pid), "background child {pid} survived");
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
        assert!(std::fs::read_to_string(d.runs.join("sample-1.log")).unwrap().contains("cancelled"));
    }

    #[tokio::test]
    async fn custody_copy_wins_over_the_worktree_script() {
        let d = dirs();
        std::fs::write(d.worktree.join("bench.sh"), "echo METRIC time_ms=1; echo ASI who=worktree").unwrap();
        std::fs::write(d.evaluator.join("bench.sh"), "echo METRIC time_ms=2; echo ASI who=custody").unwrap();
        let out = sample(&spec("bash bench.sh", "10s"), &bench_cfg(), req(&d, 1), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.value, Some(2.0));
        assert_eq!(out.asi["who"], "custody");
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
        let (exit, ..) = command("true", Duration::from_secs(10), req(&d, 1), &CancellationToken::new())
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
