//! The `schedule`, `schedule_done` and `schedule_status` tools and the
//! plugin's prompt block (design docs 34 §6, 33 §5), registered by
//! `SchedulePlugin::start` once the tool registry is up.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Local};
use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::ScheduleInfo;
use crate::file::{self, FileArgs, When};

/// The plugin's system-prompt block (34 §6, 33 §5.2). Static, so it never
/// busts the system cache.
pub const PROMPT: &str = "Schedules are markdown files in `~/.theuth/schedules/` (front matter `cron:` in local time, \
`every:`, or `at:`; body = the prompt). Create or replace one with `schedule`; read them with `read_file`, \
list with `list_dir`, cancel by deleting the file. An input wrapped in `<schedule name=… at=…>` is a timer \
firing, not a person: nobody will answer a question, so act on the prompt as written. Your reply is \
delivered to the user's chat channels when the run ends unless the wrapper says `quiet=\"true\"`, in which \
case only `message_user` reaches them — stay silent when there is nothing to say. Use `knowledge_put` for \
anything the next run should know; each run starts with a clear context. \
A schedule with `until` (a shell command) or `max_runs` is a loop: it runs again until the command exits 0, \
the cap is reached, or a run calls `schedule_done` — use `every: 0s` for \"keep at this until…\". The check's \
last output arrives in `<until>`. Draft a loop with `enabled: false` and show the user the file; start it by \
editing that line when they say so. `approvals: false` lets a run act without permission prompts: propose it \
only when nobody will be there to answer, and say so. When asked how a loop is going, call `schedule_status`. \
Read `skills/loops.md` before designing one that supervises subagents.";

/// The bundled `loops` skill (33 §5.3): the supervisor pattern, written to
/// `<data_dir>/skills/loops.md` by `first_run` like `visualize.md`.
pub const LOOPS_SKILL: &str = include_str!("../loops.md");

/// Register the three tools and the prompt block on `reg`; `dir` is the
/// schedules directory the `schedule` tool writes into.
pub async fn register(reg: &ActorRef<ToolRegistry>, dir: PathBuf) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "schedule",
            text: PROMPT.to_owned(),
        })
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(ScheduleTool { dir }))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ScheduleDoneTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ScheduleStatusTool))).await;
}

/// The schedule actor; `None` when the plugin is disabled or not yet started.
fn actor() -> Option<ActorRef<crate::Schedule>> {
    ActorRef::<crate::Schedule>::lookup(crate::SCHEDULE_ACTOR)
        .ok()
        .flatten()
}

/// `14h 3m` — whole minutes once past one, seconds below (34 §6's reply
/// style). Negative durations read as `0s`.
fn rel(d: chrono::Duration) -> String {
    let secs = u64::try_from(d.num_seconds()).unwrap_or(0);
    let secs = if secs >= 60 { secs - secs % 60 } else { secs };
    humantime::format_duration(std::time::Duration::from_secs(secs)).to_string()
}

/// Arguments for [`ScheduleTool`] (34 §6, 33 §5.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleArgs {
    /// File stem: `[a-z0-9][a-z0-9-]*`, ≤ 40 chars. An existing schedule of this name is replaced.
    pub name: String,
    /// Standard cron, local time (`0 9 * * 1-5`). Exactly one of cron / every / at.
    pub cron: Option<String>,
    /// Interval (`30m`, `6h`). `0s` = again as soon as a run ends; allowed only with until or max_runs.
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
    /// Shell command run in cwd before each fire: exit 0 = the loop is done and the file is removed,
    /// 1 = run again (its output tail arrives in `<until>`), anything else = broken, loop ends.
    pub until: Option<String>,
    /// Stop after this many runs; the last history row says `max_runs N reached`.
    pub max_runs: Option<u32>,
    /// Directory `until` runs in and the run's session starts in (`~` allowed). Must exist.
    pub cwd: Option<String>,
    /// false: the run never asks permission; say so when you propose it. Default true.
    pub approvals: Option<bool>,
    /// false: draft only; nothing fires until the enabled: line is flipped. Default true.
    pub enabled: Option<bool>,
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
         own session. With `until` or `max_runs` it is a loop that runs again until done. Returns the \
         next fire time. Read, list and delete schedules with the file tools."
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
            until: a.until.as_deref(),
            max_runs: a.max_runs,
            cwd: a.cwd.as_deref(),
            approvals: a.approvals.unwrap_or(true),
            enabled: a.enabled.unwrap_or(true),
        });
        let spec = match file::parse(&a.name, &text) {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let path = file::write_atomic(&self.dir, &a.name, &text)?;

        let now = Local::now();
        let next = spec.when.next(now, None);
        let next_line = match (&spec.when, next) {
            _ if !spec.enabled => "next: — (disabled)".to_owned(),
            (When::Every(_), _) => "next: now (first fire within a minute)".to_owned(),
            (_, Some(t)) => format!("next {} (in {})", t.format("%a %d %b %H:%M"), rel(t - now)),
            (_, None) => "next: never".to_owned(),
        };
        let mut content = format!(
            "scheduled {}: {} · {} · {next_line}",
            spec.name,
            spec.when_text,
            spec.when.describe()
        );
        if let Some(u) = &spec.until {
            content.push_str(&format!(" · until: {u}"));
        }
        if let Some(n) = spec.max_runs {
            content.push_str(&format!(" · up to {n} runs"));
        }
        if !spec.approvals {
            content.push_str(" · approvals off");
        }
        if !spec.enabled {
            content.push_str(&format!(
                " · disabled: resume with /schedules resume {} or by editing enabled:",
                spec.name
            ));
        }
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "name": spec.name,
                "path": path,
                "next": next.filter(|_| spec.enabled).map(|t| t.to_rfc3339()),
                "until": spec.until,
                "max_runs": spec.max_runs,
                "enabled": spec.enabled,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// Arguments for [`ScheduleDoneTool`] (33 §5.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleDoneArgs {
    /// Why the loop is finished; recorded in its history and delivered to the user.
    pub why: String,
}

/// `schedule_done`: the run declares its loop finished (33 §0.2). Found
/// through the calling session via [`crate::Done`].
pub struct ScheduleDoneTool;

#[async_trait::async_trait]
impl Tool for ScheduleDoneTool {
    const NAME: &'static str = "schedule_done";
    // ReadOnly (33 §5.1): it only ever prevents action, and must work from an
    // unattended run whose approvals would time out to deny.
    const RISK: Risk = Risk::ReadOnly;
    type Args = ScheduleDoneArgs;

    fn description(&self) -> String {
        "Declare the calling loop done: ends it, removes its file, tells the user why. Only from inside \
         a schedule run."
            .into()
    }

    async fn call(&self, a: ScheduleDoneArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(actor) = actor() else {
            return Ok(ToolOutput::err("schedule is not running"));
        };
        let done = crate::Done {
            session: cx.session,
            why: a.why.clone(),
        };
        Ok(match actor.ask(done).await {
            Ok(msg) => ToolOutput::ok(format!("{msg}: {}", a.why)),
            Err(SendError::HandlerError(e)) => ToolOutput::err(e),
            Err(e) => ToolOutput::err(format!("schedule actor: {e}")),
        })
    }
}

/// Arguments for [`ScheduleStatusTool`] (33 §5.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleStatusArgs {
    /// One schedule: adds its stop, its last 10 runs and the newest report. Omit for the table.
    pub name: Option<String>,
}

/// `schedule_status`: what `/schedules` and `/schedules history` show, from
/// chat (33 §0.7).
pub struct ScheduleStatusTool;

#[async_trait::async_trait]
impl Tool for ScheduleStatusTool {
    const NAME: &'static str = "schedule_status";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = ScheduleStatusArgs;

    fn description(&self) -> String {
        "How the schedules and loops are going: one line per schedule (when, next fire, runs so far, \
         running or idle, last result); with a name, also its stop, the last 10 runs and the newest \
         run's report."
            .into()
    }

    async fn call(&self, a: ScheduleStatusArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(actor) = actor() else {
            return Ok(ToolOutput::err("schedule is not running"));
        };
        let list = actor
            .ask(crate::ListSchedules)
            .await
            .map_err(|e| anyhow::anyhow!("schedule actor: {e}"))?;
        let now = Local::now();
        let Some(name) = a.name else {
            if list.is_empty() {
                return Ok(ToolOutput::ok("no schedules"));
            }
            let lines: Vec<String> = list.iter().map(|s| status_line(s, now)).collect();
            return Ok(ToolOutput {
                structured: Some(serde_json::to_value(&list)?),
                ..ToolOutput::ok(lines.join("\n"))
            });
        };
        let s = list.into_iter().find(|s| s.name == name);
        let history = actor
            .ask(crate::History {
                name: name.clone(),
                limit: 10,
            })
            .await
            .map_err(|e| anyhow::anyhow!("schedule actor: {e}"))?;
        // A loop that ended removed its file (33 §3.3); its history is still
        // the answer to "how did it go".
        let mut out = match &s {
            Some(s) => {
                let mut out = status_line(s, now);
                if let Some(u) = &s.until {
                    out.push_str(&format!("\nuntil: {u}"));
                }
                if !s.approvals {
                    out.push_str("\napprovals off");
                }
                if let Some(c) = &s.cwd {
                    out.push_str(&format!("\ncwd: {}", c.display()));
                }
                out
            }
            None if history.is_empty() => return Ok(ToolOutput::err("no such schedule")),
            None => format!(
                "{name} · no file (ended or removed) · last: {}",
                history
                    .first()
                    .and_then(|r| r.result.as_deref())
                    .unwrap_or("—")
            ),
        };
        for r in &history {
            let took = r
                .finished_at
                .map_or_else(|| "running".to_owned(), |f| rel(f - r.fired_at));
            let first: String = r
                .report
                .as_deref()
                .and_then(|t| t.lines().find(|l| !l.trim().is_empty()))
                .map(|l| l.trim().chars().take(120).collect())
                .unwrap_or_default();
            out.push_str(&format!(
                "\nfired {} · took {took} · {}",
                r.fired_at.format("%d %b %H:%M"),
                r.result.as_deref().unwrap_or("—"),
            ));
            if !first.is_empty() {
                out.push_str(&format!(" · {first}"));
            }
        }
        if let Some(report) = history
            .iter()
            .find_map(|r| r.report.as_deref().filter(|t| !t.trim().is_empty()))
        {
            out.push_str(&format!("\nlast report:\n{}", report.trim()));
        }
        Ok(ToolOutput {
            structured: Some(serde_json::json!({ "schedule": s, "history": history })),
            ..ToolOutput::ok(out)
        })
    }
}

/// One `schedule_status` line (33 §5.1): `name · when · next … · runs 14/200 ·
/// running 3m · last: end_turn`, then ` · error: …` or ` · disabled` as applicable.
fn status_line(s: &ScheduleInfo, now: DateTime<Local>) -> String {
    let next = s
        .next
        .map_or_else(|| "—".to_owned(), |t| format!("in {}", rel(t - now)));
    let runs = match (s.until.is_some(), s.max_runs) {
        (_, Some(max)) => format!("{}/{max}", s.runs),
        (true, None) => s.runs.to_string(),
        (false, None) => "—".to_owned(),
    };
    let state = match (s.running, s.last) {
        (true, Some(l)) => format!("running {}", rel(now - l)),
        (true, None) => "running".to_owned(),
        (false, _) => "idle".to_owned(),
    };
    let mut line = format!(
        "{} · {} · next {next} · runs {runs} · {state} · last: {}",
        s.name,
        s.when,
        s.last_result.as_deref().unwrap_or("—")
    );
    if let Some(e) = &s.error {
        line.push_str(&format!(" · error: {e}"));
    }
    if !s.enabled {
        line.push_str(" · disabled");
    }
    line
}
