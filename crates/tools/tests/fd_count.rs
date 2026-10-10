//! Finished jobs leave no descriptors behind: the process's open fd count
//! after N pipe jobs and M PTY jobs (exited, timed out, cancelled) is within
//! a small constant of the count before them. Its own test binary, so the
//! parallel unit-test runner cannot move the count under it.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use kameo::actor::Spawn;
use runtime::{Runtime, SessionId};
use tools::jobs::{CancelJob, CancelOrigin, ListJobs};
use tools::pty::{StartArgs, StartTool};
use tools::shell::{ShellArgs, ShellTool};
use tools::{Jobs, SessionResources, Tool, ToolCx};

const PIPE_JOBS: usize = 30;
const PTY_JOBS: usize = 10;
/// Slack for descriptors that legitimately come and go (tokio blocking
/// threads, the kqueue, a reaper that has not yet dropped its last clone).
const SLACK: usize = 4;

fn open_fds() -> usize {
    let dir = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    // The iterator itself holds one descriptor while it runs.
    std::fs::read_dir(dir).unwrap().count() - 1
}

struct Fx {
    jobs: kameo::actor::ActorRef<Jobs>,
    session: SessionId,
    shell: ShellTool,
    start: StartTool,
    dir: std::path::PathBuf,
}

impl Fx {
    fn cx(&self) -> ToolCx {
        ToolCx {
            session: self.session,
            call_id: String::new(),
            quiet: false,
            resources: SessionResources::durable_arc(&self.dir),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }

    async fn pipe(&self, command: &str, timeout_secs: Option<u64>) {
        let args = ShellArgs {
            command: command.into(),
            cwd: None,
            timeout_secs,
            env: None,
        };
        Tool::call(&self.shell, args, self.cx()).await.unwrap();
    }

    async fn pty(&self, command: &str) {
        let args = StartArgs {
            command: command.into(),
            name: None,
            cwd: None,
            env: None,
            ready: None,
            idle_secs: None,
        };
        Tool::call(&self.start, args, self.cx()).await.unwrap();
    }

    async fn cancel(&self, id: &str) {
        self.jobs
            .ask(CancelJob {
                session: self.session,
                id: tools::JobId(id.into()),
                origin: CancelOrigin::Ui,
            })
            .await
            .unwrap();
    }

    async fn all_settled(&self, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            let rows = self.jobs.ask(ListJobs(self.session)).await.unwrap();
            if rows.iter().all(|r| r.state != "running") {
                return;
            }
            assert!(Instant::now() < deadline, "still running: {rows:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn finished_jobs_do_not_raise_the_fd_count() {
    let dir = std::env::temp_dir().join(format!("theuth-fd-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.clone());
    let jobs = Jobs::spawn_with_mailbox(
        Jobs::new(rt, Duration::from_millis(100), Duration::from_secs(60)),
        runtime::subscriber_mailbox(),
    );
    let fx = Fx {
        shell: ShellTool::new(jobs.clone(), false),
        start: StartTool::new(
            jobs.clone(),
            Arc::default(),
            Duration::from_secs(30),
            Arc::new(AtomicU64::new(0)),
            None,
        ),
        jobs,
        session: SessionId::new(),
        dir: dir.clone(),
    };

    // Warm up: the session shell, the first pty, tokio's blocking threads.
    fx.pipe("echo warm", None).await;
    fx.pty("echo warm").await;
    fx.all_settled(Duration::from_secs(10)).await;
    let before = open_fds();

    for i in 0..PIPE_JOBS - 2 {
        fx.pipe(&format!("echo pipe {i}"), None).await;
    }
    fx.pipe("sleep 30", Some(1)).await; // times out
    fx.pipe("sleep 30", None).await; // cancelled
    fx.cancel(&format!("j{}", PIPE_JOBS + 2)).await;
    for i in 0..PTY_JOBS - 1 {
        fx.pty(&format!("echo pty {i}")).await;
    }
    fx.pty("sleep 30").await; // cancelled
    fx.cancel(&format!("j{}", PIPE_JOBS + PTY_JOBS + 2)).await;
    fx.all_settled(Duration::from_secs(20)).await;
    let rows = fx.jobs.ask(ListJobs(fx.session)).await.unwrap();
    assert!(
        rows.iter().filter(|r| r.state == "killed").count() == 2
            && rows.iter().any(|r| r.state == "timeout"),
        "{rows:?}"
    );

    // Reader threads and task drops settle a moment after the rows do.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut after = open_fds();
    while after > before + SLACK && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        after = open_fds();
    }
    println!("fd count {before} -> {after} after {PIPE_JOBS} pipe + {PTY_JOBS} pty jobs");
    assert!(
        after <= before + SLACK,
        "finished jobs kept {} descriptors",
        after - before
    );
    let _ = std::fs::remove_dir_all(&dir);
}
