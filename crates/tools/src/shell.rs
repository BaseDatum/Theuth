//! `shell` tool backed by an embedded `brush_core::Shell` per session. Every
//! call is a background job on the [`Jobs`] actor (14 §0): the reply is the
//! output if it exits within the grace window, else a stub naming the job.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use brush_core::{ProfileLoadBehavior, RcLoadBehavior, Shell};
use kameo::actor::ActorRef;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::jobs::{Jobs, Start, StartReply, lock};
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Interactive-only builtins we never register.
const SKIPPED_BUILTINS: &[&str] = &["bind", "history", "fc", "complete", "compgen", "compopt"];

/// Arguments for [`ShellTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShellArgs {
    /// Bash command line. State (cwd, variables, functions) persists across
    /// calls within a session.
    pub command: String,
}

/// Runs bash in-process; one persistent shell per session.
///
/// Shell state is last-finished-wins (14 §0.5): each job runs on a clone of
/// the session shell and, when it exits, that clone replaces the stored one.
/// Sequential `cd` / `export` persist; concurrent jobs are independent
/// snapshots and whichever exits later wins.
pub struct ShellTool {
    jobs: ActorRef<Jobs>,
    sessions: Arc<Mutex<HashMap<SessionId, Shell>>>,
}

impl ShellTool {
    /// Construct with the actor that runs the jobs.
    pub fn new(jobs: ActorRef<Jobs>) -> Self {
        Self {
            jobs,
            sessions: Arc::default(),
        }
    }

    /// The per-session shells, shared so `start` (22 §1.5) can default a PTY
    /// job's cwd to the session shell's.
    pub fn shells(&self) -> Arc<Mutex<HashMap<SessionId, Shell>>> {
        Arc::clone(&self.sessions)
    }

    pub(crate) async fn build_shell() -> anyhow::Result<Shell> {
        let builtins = brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode)
            .into_iter()
            .filter(|(name, _)| !SKIPPED_BUILTINS.contains(&name.as_str()));
        Ok(Shell::builder()
            .builtins(builtins)
            .profile(ProfileLoadBehavior::Skip)
            .rc(RcLoadBehavior::Skip)
            .interactive(false)
            .working_dir(std::env::current_dir()?)
            .build()
            .await?)
    }

    /// A clone of the session shell, built on first use.
    async fn session(&self, id: SessionId) -> anyhow::Result<Shell> {
        if let Some(shell) = lock(&self.sessions).get(&id) {
            return Ok(shell.clone());
        }
        let shell = Self::build_shell().await?;
        // Two concurrent first calls race here; both jobs put their shell
        // back and the later exit wins, like any concurrent pair.
        Ok(lock(&self.sessions).entry(id).or_insert(shell).clone())
    }
}

#[async_trait::async_trait]
impl Tool for ShellTool {
    const NAME: &'static str = "shell";
    const RISK: Risk = Risk::Destructive;
    type Args = ShellArgs;

    fn description(&self) -> String {
        "Run a bash command as a background job. Returns the output if it finishes within a few seconds, otherwise a job id and log path; the result is delivered later as <job_result>. cwd, variables and functions persist between calls.".into()
    }

    async fn call(&self, args: ShellArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let shell = self.session(cx.session).await?;
        let sessions = Arc::clone(&self.sessions);
        let session = cx.session;
        let started = Instant::now();
        let reply = self
            .jobs
            .ask(Start {
                session,
                command: args.command,
                call_id: cx.call_id,
                shell,
                put_back: Arc::new(move |shell| {
                    lock(&sessions).insert(session, shell);
                }),
            })
            .await
            .map_err(|e| anyhow::anyhow!("jobs actor: {e}"))?;
        Ok(match reply {
            StartReply::Done(out) => out,
            StartReply::Running { id, log, tail } => ToolOutput {
                content: format!(
                    "job {id} started {}s ago, still running.\nlog: {}\noutput so far:\n{}\nResult arrives as <job_result id=\"{id}\"> when it exits — keep working, or end the turn and it will start a new one. `wait` is for PTY jobs only; do not call it on {id}.",
                    started.elapsed().as_secs(),
                    log.display(),
                    tail.trim_end()
                ),
                is_error: false,
                structured: Some(serde_json::json!({ "job": id, "log": log })),
                files: Vec::new(),
            },
        })
    }
}
