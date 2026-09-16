//! The `schedule` tool and the plugin's prompt block (design doc 34 §6),
//! registered by `SchedulePlugin::start` once the tool registry is up.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Local;
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::file::{self, FileArgs, When};

/// The plugin's system-prompt block (34 §6). Static, so it never busts the
/// system cache.
pub const PROMPT: &str = "Schedules are markdown files in `~/.wth/schedules/` (front matter `cron:` in local time, \
`every:`, or `at:`; body = the prompt). Create or replace one with `schedule`; read them with `read_file`, \
list with `list_dir`, cancel by deleting the file. An input wrapped in `<schedule name=… at=…>` is a timer \
firing, not a person: nobody will answer a question, so act on the prompt as written. Your reply is \
delivered to the user's chat channels when the run ends unless the wrapper says `quiet=\"true\"`, in which \
case only `message_user` reaches them — stay silent when there is nothing to say. Use `knowledge_put` for \
anything the next run should know; each run starts with a clear context.";

/// Register the `schedule` tool and the prompt block on `reg`; `dir` is the
/// schedules directory the tool writes into.
pub async fn register(reg: &ActorRef<ToolRegistry>, dir: PathBuf) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "schedule",
            text: PROMPT.to_owned(),
        })
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(ScheduleTool { dir }))).await;
}

/// Arguments for [`ScheduleTool`] (34 §6).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleArgs {
    /// File stem: `[a-z0-9][a-z0-9-]*`, ≤ 40 chars. An existing schedule of this name is replaced.
    pub name: String,
    /// Standard cron, local time (`0 9 * * 1-5`). Exactly one of cron / every / at.
    pub cron: Option<String>,
    /// Interval (`30m`, `6h`).
    pub every: Option<String>,
    /// One instant, local (`2026-09-16T15:00`) or RFC 3339. Runs once; the schedule is then gone.
    pub at: Option<String>,
    /// What to do when it fires. Written for a run with nobody present.
    pub prompt: String,
    /// `true`: the run's reply is not sent to the user; only `message_user` reaches them.
    #[serde(default)]
    pub quiet: bool,
    /// Model for the run; default the harness default.
    pub model: Option<String>,
}

/// `schedule`: validate and atomically write `<dir>/<name>.md`, answer with
/// the next fire time.
pub struct ScheduleTool {
    /// The schedules directory (`<data_dir>/schedules`).
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Tool for ScheduleTool {
    const NAME: &'static str = "schedule";
    // Mutating on purpose (34 §6): the file makes the harness act later,
    // unattended, with every tool it has — the user sees each grant once.
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = ScheduleArgs;

    fn description(&self) -> String {
        "Create or replace a scheduled prompt: a file in the schedules directory that fires on a clock \
         (cron in local time, an interval, or once at an instant) and runs the prompt unattended in its \
         own session. Returns the next fire time. Read, list and delete schedules with the file tools."
            .into()
    }

    async fn call(&self, a: ScheduleArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if !file::valid_name(&a.name) {
            return Ok(ToolOutput::err(
                "name: [a-z0-9][a-z0-9-]*, at most 40 chars",
            ));
        }
        let text = file::render(&FileArgs {
            cron: a.cron.as_deref(),
            every: a.every.as_deref(),
            at: a.at.as_deref(),
            quiet: a.quiet,
            model: a.model.as_deref(),
            prompt: &a.prompt,
        });
        let spec = match file::parse(&a.name, &text) {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let path = file::write_atomic(&self.dir, &a.name, &text)?;

        let now = Local::now();
        let next = spec.when.next(now, None);
        let next_line = match (&spec.when, next) {
            (When::Every(_), _) => "next: now (first fire within a minute)".to_owned(),
            (_, Some(t)) => {
                let secs = u64::try_from((t - now).num_seconds()).unwrap_or(0);
                // Whole minutes once past one: `in 14h 3m`, not `in 14h 3m 12s`.
                let secs = if secs >= 60 { secs - secs % 60 } else { secs };
                let rel = humantime::format_duration(std::time::Duration::from_secs(secs));
                format!("next {} (in {rel})", t.format("%a %d %b %H:%M"))
            }
            (_, None) => "next: never".to_owned(),
        };
        let content = format!(
            "scheduled {}: {} · {} · {next_line}",
            spec.name,
            spec.when_text,
            spec.when.describe()
        );
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "name": spec.name,
                "path": path,
                "next": next.map(|t| t.to_rfc3339()),
            })),
            ..ToolOutput::ok(content)
        })
    }
}
