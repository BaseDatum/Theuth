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

use crate::file::{self, FileArgs, When};
use crate::{ScheduleInfo, validate};
use agent::sensor::SensorRunner;

/// The plugin's system-prompt block (34 §6, 33 §5.2). Static, so it never
/// busts the system cache.
pub const PROMPT: &str = "Schedules are markdown files in `~/.theuth/schedules/` (front matter `cron:` in local time, \
`every:`, or `at:`; body = the prompt). Create or replace one with `schedule()`; read them with `read_file()`, \
list with `list_dir()`, cancel by deleting the file. An input wrapped in `<schedule name=… at=…>` is a timer \
firing, not a person: nobody will answer a question, so act on the prompt as written. Your reply is \
delivered to the user's chat channels when the run ends unless the wrapper says `quiet=\"true\"`, in which \
case only `message_user` reaches them — stay silent when there is nothing to say. Use `knowledge_put()` for \
anything the next run should know; each run starts with a clear context and an empty python runtime \
(`runtime: keep` in the front matter keeps the python globals between runs). \
A schedule with `until` (a shell command) or `max_runs` is a loop: it runs again until the command exits 0, \
the cap is reached, or a run calls `schedule_done()` — use `every: 0s` for \"keep at this until…\". The check's \
last output arrives in `<until>`. Draft a loop with `enabled: false` and show the user the file; start it by \
editing that line when they say so. `approvals: false` lets a run act without permission prompts: propose it \
only when nobody will be there to answer, and say so. When asked how a loop is going, call `schedule_status()`. \
`run_timeout:` (e.g. `45m`) caps one run; the default is Settings › Agent loop › Routines. \
Read `skills/loops.md` before designing one that supervises subagents. \
To design a routine with the user, load the `routines` skill.";

/// The bundled `loops` skill (33 §5.3): the supervisor pattern, written to
/// `<data_dir>/skills/loops.md` by `first_run` like `visualize.md`.
pub const LOOPS_SKILL: &str = include_str!("../loops.md");

/// The bundled `routines` skill (108 §7): the mental model, the sentence
/// grammar and the `routine_draft` flow; installed beside `loops.md`.
pub const ROUTINES_SKILL: &str = include_str!("../routines.md");

/// Register the three tools and the prompt block on `reg`; `dir` is the
/// schedules directory the `schedule` tool writes into.
pub async fn register(reg: &ActorRef<ToolRegistry>, dir: PathBuf, sensors: Arc<SensorRunner>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "schedule",
            text: PROMPT.to_owned(),
        })
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(ScheduleTool { dir, sensors })))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(ScheduleDoneTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ScheduleStatusTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ListSensorsTool))).await;
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

/// `v` unless `watcher` and it spells the key's default (40 §2: written only
/// when non-default); a plain file keeps it so `<key>: needs when:` surfaces.
fn non_default<'a>(v: Option<&'a str>, default: &str, watcher: bool) -> Option<&'a str> {
    v.map(str::trim)
        .filter(|v| !(v.is_empty() || watcher && *v == default))
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
    /// Time limit for one run (`45m`, `2h`; at least `30s`). A run still going is cancelled and its history row says `timeout`. Default: the `schedule.run_timeout_secs` setting (30 min unless changed in Settings › Agent loop › Routines).
    pub run_timeout: Option<String>,
    /// Directory `until` runs in and the run's session starts in (`~` allowed). Must exist.
    pub cwd: Option<String>,
    /// false: the run never asks permission; say so when you propose it. Default true.
    pub approvals: Option<bool>,
    /// false: draft only; nothing fires until the enabled: line is flipped. Default true for a clock file,
    /// false for a watcher (`when`); an explicit true on a watcher is honoured when its dry run is clean (108 D12).
    pub enabled: Option<bool>,
    /// A roster agent: each fire creates a task owned by it instead of running in the schedule's own session (67 §7.4).
    pub agent: Option<String>,
    /// The project label of that task; only with `agent`.
    pub project: Option<String>,
    /// Watcher (40 §2): `<file>.py key=value …` — "requires `every`; exclusive with `cron` / `at`; first token a `.py` file in the sensors directory; then `key=value` tokens (`"quoted with spaces"` allowed), keys `[a-z][a-z0-9_]*`; `cwd` is not an accepted key". Needs a stop: until, max_runs or expires.
    pub when: Option<String>,
    /// 40 §2: "`on_change` (default) / `while_true` / `once`; requires `when`".
    pub fire: Option<String>,
    /// 40 §2: "duration, default `0s`; requires `when`" — consecutive true checks must span this long before the first fire.
    #[serde(rename = "for")]
    pub for_: Option<String>,
    /// 40 §2: "duration, default `0s`; requires `when`" — minimum spacing between fires.
    pub cooldown: Option<String>,
    /// 40 §2: "`skip` (default) / `queue` / `restart`; requires `when`" — what a fire does while the run is in flight.
    pub mode: Option<String>,
    /// 40 §2: "`continue` (default with `when`) / `clear`; requires `when`" — whether the session keeps its context between fires.
    pub memory: Option<String>,
    /// 131 D1: "`clear` (default) / `keep`" — reset the python globals before each run that clears the context, or keep them.
    pub runtime: Option<String>,
    /// 40 §2: "duration ≥ `every`; from the file's `created:` line" — TTL; on expiry the file is removed and the user told.
    pub expires: Option<String>,
    /// 40 §2: "sensor wall clock; default `20s`, ≤ `60s`, `< every / 2`; requires `when`".
    pub timeout: Option<String>,
}

/// `schedule`: validate and atomically write `<dir>/<name>.md`, answer with
/// the next fire time.
pub struct ScheduleTool {
    /// The schedules directory (`<data_dir>/schedules`).
    pub dir: PathBuf,
    /// The sensor runtime (40 §4): the creation-time dry run goes through it.
    pub sensors: Arc<SensorRunner>,
}

/// The file text `schedule` writes for `a` (108 §8.1 shares it with
/// `routine_draft`). 40 §2: watcher keys are written only when present and
/// non-default (a plain file keeps them so `<key>: needs when:` still
/// surfaces); `created: now` anchors `expires` on every write of a `when:` file.
pub fn render_args(a: &ScheduleArgs) -> String {
    let watcher = a.when.is_some();
    let created = watcher.then(|| Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false));
    file::render(&FileArgs {
        cron: a.cron.as_deref(),
        every: a.every.as_deref(),
        at: a.at.as_deref(),
        quiet: a.quiet,
        model: a.model.as_deref(),
        prompt: &a.prompt,
        until: a.until.as_deref(),
        max_runs: a.max_runs,
        run_timeout: a.run_timeout.as_deref(),
        // 136 §2.3 / D4: `budget:` and `cap:` are written by hand or by the bundled dreaming routine, not by the tool.
        budget: None,
        cap: None,
        cwd: a.cwd.as_deref(),
        approvals: a.approvals.unwrap_or(true),
        // 40 §1.12 / 108 D12: a watcher defaults to `enabled: false`; an
        // explicit `true` stands because an `Err` verdict refuses the write.
        enabled: a.enabled.unwrap_or(!watcher),
        missed_run: false,
        agent: a.agent.as_deref(),
        project: a.project.as_deref(),
        when: a.when.as_deref(),
        fire: non_default(a.fire.as_deref(), "on_change", watcher),
        for_: non_default(a.for_.as_deref(), "0s", watcher),
        cooldown: non_default(a.cooldown.as_deref(), "0s", watcher),
        mode: non_default(a.mode.as_deref(), "skip", watcher),
        memory: non_default(a.memory.as_deref(), "continue", watcher),
        // 131 D1: `clear` is the default for every kind, so it is dropped always.
        runtime: non_default(a.runtime.as_deref(), "clear", true),
        expires: a.expires.as_deref(),
        timeout: non_default(a.timeout.as_deref(), "20s", watcher),
        created: created.as_deref(),
    })
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
        let text = render_args(&a);
        let spec = match file::parse(&a.name, &text) {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        // 40 §1.12: resolve the sensor (and CPython), dry-run twice, probe; refuse
        // on `Err` — nothing is written. `subject=` from the payload titles the session.
        let verdict = match &spec.sensor {
            Some(_) => match validate::dry_run_with_subject(&spec, &self.sensors).await {
                Ok(v) => Some(v),
                Err(e) => return Ok(ToolOutput::err(e)),
            },
            None => None,
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
            spec.describe()
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
        if let Some((v, subject)) = &verdict {
            // 40 §1.12: bind the session now. The actor learns of the file from
            // its own reconcile; ask for one so the entry exists before the bind.
            match actor() {
                Some(actor) => {
                    let _ = actor.ask(crate::Reconcile).await;
                    // A handler `Err` is lifted into `SendError::HandlerError`, which displays as it.
                    match actor
                        .ask(crate::BindSession {
                            name: spec.name.clone(),
                            subject: subject.clone(),
                        })
                        .await
                    {
                        Ok(_) => content.push_str(" · session bound"),
                        Err(e) => tracing::warn!(name = %spec.name, "bind at creation: {e}"),
                    }
                }
                None => {
                    tracing::warn!(name = %spec.name, "bind at creation: schedule actor not running")
                }
            }
            content.push('\n');
            content.push_str(&validate::render(v));
        }
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "name": spec.name,
                "path": path,
                "next": next.filter(|_| spec.enabled).map(|t| t.to_rfc3339()),
                "until": spec.until,
                "max_runs": spec.max_runs,
                "enabled": spec.enabled,
                "verdict": verdict.as_ref().map(|(v, _)| v),
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
                // 40 §6: the watcher's last sensor check, on its own line too.
                if let Some(c) = &s.last_check {
                    out.push_str(&format!("\nlast check: {}", last_check(c)));
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
/// A watcher's `last:` is its last sensor check (40 §6): the run rows it
/// shares with `last_result` are gate rows, so `last_check` is authoritative.
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
    let last = match &s.last_check {
        Some(c) => format!("last check: {}", last_check(c)),
        None => format!("last: {}", s.last_result.as_deref().unwrap_or("—")),
    };
    let mut line = format!(
        "{} · {} · next {next} · runs {runs} · {state} · {last}",
        s.name, s.when,
    );
    if let Some(e) = &s.error {
        line.push_str(&format!(" · error: {e}"));
    }
    if !s.enabled {
        line.push_str(" · disabled");
    }
    line
}

/// `<at> · <outcome> · <elapsed> ms` (40 §6) from [`crate::LastCheck`].
fn last_check(c: &crate::LastCheck) -> String {
    format!(
        "{} · {} · {} ms",
        c.at.format("%H:%M:%S"),
        c.outcome,
        c.elapsed_ms
    )
}

/// Arguments for [`ListSensorsTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListSensorsArgs {}

/// `list_sensors` (108 §9.2): the installed sensors with their `# arg:`
/// headers, for the `routines` skill. Same rows as the `list_sensors` RPC.
pub struct ListSensorsTool;

#[async_trait::async_trait]
impl Tool for ListSensorsTool {
    const NAME: &'static str = "list_sensors";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list";
    type Args = ListSensorsArgs;

    fn description(&self) -> String {
        "The sensors a watcher routine can poll (`when: <file>.py key=value …`): one line per installed \
         sensor with its description, `when:` usage and arguments (name, required/optional, example, help). \
         Offer only sensors without `error`; a `needs cpython` sensor needs a Python on this machine."
            .into()
    }

    async fn call(&self, _: ListSensorsArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(actor) = actor() else {
            return Ok(ToolOutput::err("schedule is not running"));
        };
        let list = actor
            .ask(crate::ListSensors)
            .await
            .map_err(|e| anyhow::anyhow!("schedule actor: {e}"))?;
        if list.is_empty() {
            return Ok(ToolOutput::ok("no sensors installed"));
        }
        let lines: Vec<String> = list.iter().map(sensor_line).collect();
        Ok(ToolOutput {
            structured: Some(serde_json::to_value(&list)?),
            ..ToolOutput::ok(lines.join("\n"))
        })
    }
}

/// `pr_watch.py · PR babysitter … · when: pr_watch.py repo=… · args: repo (required, e.g. owner/name) —
/// the repository; … · needs cpython (none found) · edited · error: …`.
fn sensor_line(s: &crate::SensorInfo) -> String {
    let mut out = s.name.clone();
    if let Some(d) = &s.description {
        out.push_str(&format!(" · {d}"));
    }
    if let Some(u) = &s.usage {
        out.push_str(&format!(" · {u}"));
    }
    if !s.args.is_empty() {
        let args: Vec<String> = s
            .args
            .iter()
            .map(|a| {
                let mut t = format!(
                    "{} ({}",
                    a.name,
                    if a.required { "required" } else { "optional" }
                );
                if let Some(e) = &a.example {
                    t.push_str(&format!(", e.g. {e}"));
                }
                t.push(')');
                if !a.help.is_empty() {
                    t.push_str(&format!(" — {}", a.help));
                }
                t
            })
            .collect();
        out.push_str(&format!(" · args: {}", args.join("; ")));
    }
    if s.needs_cpython {
        out.push_str(if s.cpython_available {
            " · needs cpython"
        } else {
            " · needs cpython (none found)"
        });
    }
    if s.edited {
        out.push_str(" · edited");
    }
    if let Some(e) = &s.error {
        out.push_str(&format!(" · error: {e}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40 §1.12: a `when:` file whose sensor does not resolve is refused
    /// before anything reaches the schedules directory.
    #[tokio::test]
    async fn failing_dry_run_writes_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sensors = Arc::new(SensorRunner::new(
            tmp.path().to_path_buf(),
            None,
            None,
            Default::default(),
        ));
        let dir = tmp.path().join("schedules");
        let tool = ScheduleTool {
            dir: dir.clone(),
            sensors,
        };
        let cx = ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: Arc::new(tools::SessionResources {
                root: tmp.path().to_path_buf(),
                retention: tools::Retention::Durable,
                generation: 1,
            }),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        };
        let args: ScheduleArgs = serde_json::from_value(serde_json::json!({
            "name": "w", "prompt": "say hi", "every": "60s", "when": "missing.py", "max_runs": 3,
            "enabled": true
        }))
        .unwrap();
        let out = tool.call(args, cx).await.unwrap();
        assert!(out.is_error, "{}", out.content);
        assert_eq!(out.content, "when: no such sensor: missing.py");
        assert!(
            !dir.join("w.md").exists(),
            "D12: an explicit enabled: true does not bypass the refusal"
        );
    }

    /// 108 §9.5: `agent` / `project` reach the file; a watcher without
    /// `enabled` is written paused, with `enabled: true` it is written live
    /// (the dry run, not the render, is what refuses a broken one); a clock
    /// file keeps its default of enabled.
    #[test]
    fn render_args_enabled_rule_and_routing() {
        let args = |v: serde_json::Value| -> ScheduleArgs { serde_json::from_value(v).unwrap() };
        let watcher = args(serde_json::json!({
            "name": "w", "prompt": "say hi", "every": "60s", "when": "pr_watch.py repo=o/r pr=1 token=t",
            "max_runs": 3, "agent": "nikolai", "project": "web", "fire": "on_change", "mode": "queue"
        }));
        let text = render_args(&watcher);
        assert!(text.contains("enabled: false\n"), "{text}");
        assert!(
            text.contains("agent: nikolai\n") && text.contains("project: \"web\"\n"),
            "{text}"
        );
        assert!(
            !text.contains("fire:") && text.contains("mode: queue\n"),
            "defaults dropped: {text}"
        );
        assert!(text.contains("created: "), "{text}");
        let spec = file::parse("w", &text).unwrap();
        assert_eq!(
            (spec.agent.as_deref(), spec.project.as_deref(), spec.enabled),
            (Some("nikolai"), Some("web"), false)
        );

        let live = args(serde_json::json!({
            "name": "w", "prompt": "say hi", "every": "60s", "when": "pr_watch.py repo=o/r pr=1 token=t",
            "max_runs": 3, "enabled": true
        }));
        let text = render_args(&live);
        assert!(!text.contains("enabled:"), "{text}");
        assert!(file::parse("w", &text).unwrap().enabled);

        let clock =
            args(serde_json::json!({ "name": "c", "prompt": "brief", "cron": "0 9 * * 1-5" }));
        assert!(file::parse("c", &render_args(&clock)).unwrap().enabled);
        let paused = args(
            serde_json::json!({ "name": "c", "prompt": "brief", "cron": "0 9 * * 1-5", "enabled": false }),
        );
        assert!(!file::parse("c", &render_args(&paused)).unwrap().enabled);
    }

    /// 131 D1: `runtime` reaches the file only when it is `keep`.
    #[test]
    fn render_args_runtime_key_only_when_keep() {
        let args = |v: serde_json::Value| -> ScheduleArgs { serde_json::from_value(v).unwrap() };
        let keep = args(
            serde_json::json!({ "name": "c", "prompt": "brief", "every": "1h", "runtime": "keep" }),
        );
        let text = render_args(&keep);
        assert!(text.contains("\nruntime: keep\n"), "{text}");
        assert_eq!(
            file::parse("c", &text).unwrap().runtime,
            file::Runtime::Keep
        );
        for v in [
            serde_json::json!({ "name": "c", "prompt": "brief", "every": "1h" }),
            serde_json::json!({ "name": "c", "prompt": "brief", "every": "1h", "runtime": "clear" }),
        ] {
            let text = render_args(&args(v));
            assert!(!text.contains("runtime:"), "{text}");
            assert_eq!(
                file::parse("c", &text).unwrap().runtime,
                file::Runtime::Clear
            );
        }
    }

    #[test]
    fn sensor_line_reads_header() {
        let s = crate::SensorInfo {
            name: "pr_watch.py".into(),
            description: Some("PR babysitter.".into()),
            usage: Some("when: pr_watch.py repo=<o/r>".into()),
            args: vec![
                agent::sensor::SensorArg {
                    name: "repo".into(),
                    required: true,
                    example: Some("o/r".into()),
                    help: "the repository".into(),
                },
                agent::sensor::SensorArg {
                    name: "n".into(),
                    required: false,
                    example: None,
                    help: String::new(),
                },
            ],
            version: Some("2".into()),
            builtin: true,
            edited: true,
            shipped_version: Some("2".into()),
            needs_cpython: true,
            cpython_available: false,
            hosts: vec![],
            has_probe: true,
            error: None,
        };
        assert_eq!(
            sensor_line(&s),
            "pr_watch.py · PR babysitter. · when: pr_watch.py repo=<o/r> · args: repo (required, e.g. o/r) — the repository; n (optional) · needs cpython (none found) · edited"
        );
    }
}
