//! The `show_pty` and `show_note` tools and their [`PtyViewOpened`] / [`NoteViewOpened`] events
//! (design 211): siblings of `show_web` that open a job (a PTY terminal or a background job's
//! log) or a note in a tile beside the chat. `show_pty` can also start a new terminal through the
//! registered `start` tool, gate and all (211 §5). The engine resolves and checks what is named and
//! announces it; the desktop shell places the tile. The receipt's first line is the transcript's
//! only record — the UI parses it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use document::{DocKind, Principal};
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{
    Invoke, JobView, Jobs, ListJobs, ListTerminals, Risk, Tool, ToolCall, ToolCx, ToolOutput,
    ToolRegistry,
};

use crate::webview::clean_title;

/// The receipt line both tools add when no app would see the event (as `show_web`'s).
pub(crate) const NO_APP: &str = "note: no app is connected; the card in the chat opens it later";

/// Longest accepted `block` id, in bytes.
const MAX_BLOCK: usize = 64;

/// The login shell's job name when the call names none (211 §5).
const SHELL_NAME: &str = "shell";

/// Arguments of `show_pty` (211 §5).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowPtyArgs {
    /// An existing job to show: its id (`j3`) or the `name` given to `start`. A `start()`
    /// terminal opens as a terminal the user can type into; a `shell()` job as its live log.
    pub job: Option<String>,
    /// Start this command on a new terminal (as `start` does) and show it. With neither `job`
    /// nor `command`, the user's login shell is started.
    pub command: Option<String>,
    /// Working directory of a new terminal; default the session's.
    pub cwd: Option<PathBuf>,
    /// Label of a new terminal, as `start`'s `name` (default `shell` for the login shell).
    pub name: Option<String>,
    /// Tile and card title. Default: the job's name, else its command.
    pub title: Option<String>,
}

/// Event: a `show_pty` call opened a job tile (211 §1, §5).
/// Wire shape `{ "type": "PtyViewOpened", "session", "call_id", "job", "name", "title",
/// "running", "pty", "spawned" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct PtyViewOpened {
    /// Calling session (the job's owner).
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// The resolved job id (`j3`).
    pub job: String,
    /// `start`'s label, when it had one.
    pub name: Option<String>,
    /// Tile and card title.
    pub title: String,
    /// The job still runs; `false`: the tile shows its final screen or log.
    pub running: bool,
    /// `true`: an interactive terminal; `false`: a background job's read-only log.
    pub pty: bool,
    /// This call started the job.
    pub spawned: bool,
}

impl wire::WireEvent for PtyViewOpened {
    const TOPIC: &'static str = "artifact.pty-view-opened";
}

/// `show_pty`: resolve the job (or start a terminal through `start`), publish
/// [`PtyViewOpened`], return the receipt (211 §1, §5). `Mutating` for every call: the trait has
/// no per-call risk, and a spawn runs a process; the nested `start` keeps its own gate.
pub struct ShowPtyTool {
    /// For publishing the event.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowPtyTool {
    const NAME: &'static str = "show_pty";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowPtyArgs;

    fn description(&self) -> String {
        "Show a job in a tile beside the chat for the user to watch: a terminal from `start()` \
         they can type into (a dev server's log, a long build, a prompt only they can answer) or \
         a `shell()` background job's live log. `job` is the job id (`j3`) or the `name` given to \
         `start`. Without `job`, it starts a new terminal as `start` does and shows it: \
         `command` (with `cwd`, `name`), or with no arguments the user's own login shell; the new \
         job is a normal job for screen()/send()/cancel_job. `title` names the tile (default the \
         job's name, else its command). The same job again reuses its tile. `show_web` is for web \
         pages, `show` for files."
            .into()
    }

    async fn call(&self, args: ShowPtyArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let plan = match plan(&args) {
            Ok(p) => p,
            Err(msg) => return Ok(ToolOutput::err(format!("show_pty: {msg}"))),
        };
        let Some(jobs) = ActorRef::<Jobs>::lookup(tools::JOBS_ACTOR).ok().flatten() else {
            return Ok(ToolOutput::err("show_pty: jobs are not running on this engine"));
        };
        let (query, spawned) = match plan {
            Plan::Show(job) => (job, None),
            Plan::Spawn { command, name } => {
                let all = jobs.ask(ListJobs(cx.session)).await.unwrap_or_default();
                let login = command.is_none();
                let name = name.or_else(|| login.then(|| free_name(&all, SHELL_NAME)));
                let command = command.unwrap_or_else(login_shell);
                let cwd = match &args.cwd {
                    Some(c) => tools::seat::resolve(cx.session, c),
                    None => tools::seat::cwd(cx.session)?,
                };
                match start(&cx, &command, name.as_deref(), &cwd, login).await {
                    Ok(job) => (job, Some(Spawned { command, name, cwd })),
                    Err(msg) => return Ok(ToolOutput::err(format!("show_pty: {msg}"))),
                }
            }
        };
        let all = jobs.ask(ListJobs(cx.session)).await.unwrap_or_default();
        let Some(job) = resolve_job(&all, &query) else {
            return Ok(ToolOutput::err(format!(
                "show_pty: no job `{query}` in this session; give a job id (`j3`) or the name \
                 passed to start(), or leave `job` out to start a terminal"
            )));
        };
        if job.pty {
            let retained = jobs
                .ask(ListTerminals {
                    sessions: vec![cx.session],
                })
                .await
                .unwrap_or_default()
                .iter()
                .any(|t| t.id == job.id);
            if !retained {
                return Ok(ToolOutput::err(format!(
                    "show_pty: {}'s terminal is no longer retained (evicted after it finished); \
                     start it again",
                    job.id.0
                )));
            }
        }
        let title = args
            .title
            .map(|t| clean_title(&t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| default_job_title(job));
        let view = JobTile {
            job: job.id.0.clone(),
            title,
            running: job.state == "running",
            pty: job.pty,
        };
        self.rt
            .publish(PtyViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                job: view.job.clone(),
                name: job.name.clone(),
                title: view.title.clone(),
                running: view.running,
                pty: view.pty,
                spawned: spawned.is_some(),
            })
            .await;
        let content = job_receipt(
            &view,
            cx.session,
            spawned.as_ref(),
            crate::preview::feed_live(),
        );
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "session": cx.session,
                "job": view.job,
                "name": job.name,
                "title": view.title,
                "running": view.running,
                "pty": view.pty,
                "spawned": spawned.is_some(),
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// What a `show_pty` call does.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Show this existing job (id or name).
    Show(String),
    /// Start `command` (`None`: the login shell) as `name`, then show it.
    Spawn {
        command: Option<String>,
        name: Option<String>,
    },
}

/// 211 §5: `job` shows, `command` or nothing spawns, both is refused.
fn plan(args: &ShowPtyArgs) -> Result<Plan, String> {
    let given = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let (job, command, name) = (given(&args.job), given(&args.command), given(&args.name));
    match job {
        Some(_) if command.is_some() => Err("give job or command, not both".into()),
        Some(_) if name.is_some() || args.cwd.is_some() => {
            Err("`name` and `cwd` apply only when starting a terminal (no `job`)".into())
        }
        Some(job) => Ok(Plan::Show(job)),
        None => Ok(Plan::Spawn { command, name }),
    }
}

/// What a spawning call started, for the receipt's second line.
#[derive(Debug)]
struct Spawned {
    command: String,
    name: Option<String>,
    cwd: PathBuf,
}

/// The tile a call opens.
#[derive(Debug)]
struct JobTile {
    job: String,
    title: String,
    running: bool,
    pty: bool,
}

/// Start `command` through the registered `start` tool (its gate, `$SHELL -lc`, readiness and
/// idle rules) as call `<call_id>.start`; the job id, or `start`'s refusal. The login shell never
/// wakes the model with `<job_idle>`: it is the user's terminal.
async fn start(
    cx: &ToolCx,
    command: &str,
    name: Option<&str>,
    cwd: &Path,
    login: bool,
) -> Result<String, String> {
    let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(tools::REGISTRY_ACTOR) else {
        return Err("tool registry unavailable".into());
    };
    let mut args = serde_json::json!({ "command": command, "cwd": cwd });
    if let Some(n) = name {
        args["name"] = n.into();
    }
    if login {
        args["idle_secs"] = 0.into();
    }
    let out = reg
        .ask(Invoke(ToolCall {
            id: format!("{}.start", cx.call_id),
            name: "start".into(),
            args,
            session: cx.session,
            quiet: false,
        }))
        .await
        .map_err(|e| format!("tool registry: {e}"))?;
    if out.is_error {
        return Err(format!("start refused: {}", out.content));
    }
    out.structured
        .as_ref()
        .and_then(|s| s["job"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("start returned no job: {}", out.content))
}

/// The user's login shell as a `start` command: `exec $SHELL -l` (fallback `/bin/zsh`), so the
/// job is the shell itself rather than a wrapper around it.
fn login_shell() -> String {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    format!("exec {} -l", shell_word(&shell))
}

/// `s` as one word for `$SHELL` (which may be csh or fish): bare when plain, else single-quoted.
fn shell_word(s: &str) -> String {
    if s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        s.to_owned()
    } else {
        platform::shell::single_quote(s)
    }
}

/// `base`, else `base-2`, `base-3`, … — the first name no running job of the session holds
/// (`start` refuses a duplicate among running jobs).
fn free_name(jobs: &[JobView], base: &str) -> String {
    let taken = |n: &str| {
        jobs.iter()
            .any(|j| j.state == "running" && j.name.as_deref() == Some(n))
    };
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|i| format!("{base}-{i}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| base.to_owned())
}

/// `send`/`screen`'s rule (22 §4.8) over every job of the session: exact id, else the newest
/// running one named `query`, else the newest named `query`.
fn resolve_job<'a>(jobs: &'a [JobView], query: &str) -> Option<&'a JobView> {
    let named = |j: &&JobView| j.name.as_deref() == Some(query);
    jobs.iter()
        .find(|j| j.id.0 == query)
        .or_else(|| {
            jobs.iter()
                .rev()
                .filter(|j| j.state == "running")
                .find(named)
        })
        .or_else(|| jobs.iter().rev().find(named))
}

/// The job's name, else its command, one line; the id when both are blank.
fn default_job_title(j: &JobView) -> String {
    [j.name.as_deref(), Some(j.command.as_str())]
        .into_iter()
        .flatten()
        .map(clean_title)
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| j.id.0.clone())
}

/// The text the model reads; the first line is fixed, one form per tile kind (211 §1, §5).
fn job_receipt(v: &JobTile, session: SessionId, spawned: Option<&Spawned>, live: bool) -> String {
    let kind = if v.pty { "pty" } else { "job" };
    let mut s = format!(
        "{kind} view opened · {} · tile {kind}:{session}/{}",
        v.title, v.job
    );
    if let Some(sp) = spawned {
        let name = sp
            .name
            .as_deref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        let _ = write!(
            s,
            "\nstarted {}{name} in {}: {}\nIt is a normal job: screen()/send()/cancel_job work on it.",
            v.job,
            sp.cwd.display(),
            sp.command
        );
    }
    if !live {
        let _ = write!(s, "\n{NO_APP}");
    }
    if v.pty {
        s.push_str(
            "\nThe user can type into this terminal and the tile resizes it; you keep using screen()/send() as before.",
        );
        if !v.running {
            s.push_str("\nThe job has finished; the tile shows its final screen.");
        }
    } else {
        s.push_str(
            "\nThe tile shows this job's output live, read-only; its <job_result> still reaches you when it exits.",
        );
        if !v.running {
            s.push_str("\nThe job has finished; the tile shows its log.");
        }
    }
    s
}

/// Arguments of `show_note` (211 §1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowNoteArgs {
    /// A document id, `knowledge:<concept id>` (or a bare `a/b` concept id), or
    /// `meeting:<id>` (its note) — as the `doc_*` tools take it.
    pub doc: String,
    /// A block id from `doc_read` to scroll to.
    pub block: Option<String>,
    /// Tile and card title. Default: the document's title.
    pub title: Option<String>,
}

/// Event: a `show_note` call opened a note tile (211 §1).
/// Wire shape `{ "type": "NoteViewOpened", "session", "call_id", "doc", "kind", "title", "block" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct NoteViewOpened {
    /// Calling session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// The resolved 26-char document id.
    pub doc: String,
    /// `note` | `board` | `knowledge` | `meeting_note` (the `DocKind` serde name).
    pub kind: String,
    /// Tile and card title.
    pub title: String,
    /// Block to scroll to.
    pub block: Option<String>,
}

impl wire::WireEvent for NoteViewOpened {
    const TOPIC: &'static str = "artifact.note-view-opened";
}

/// Arguments of `show_whiteboard` (218 §3.4).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowWhiteboardArgs {
    /// The whiteboard's document id (the `id` of its `<whiteboard>` block, or what `wb_create` returned).
    pub whiteboard: String,
    /// Tile title. Default: the whiteboard's title.
    pub title: Option<String>,
}

/// Event: a `show_whiteboard` call opened a whiteboard (218 §4.3).
/// Wire shape `{ "type": "WhiteboardViewOpened", "session", "call_id", "doc", "title" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct WhiteboardViewOpened {
    /// Calling session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// The whiteboard's document id.
    pub doc: String,
    /// Tile title.
    pub title: String,
}

impl wire::WireEvent for WhiteboardViewOpened {
    const TOPIC: &'static str = "artifact.whiteboard-view-opened";
}

/// `show_note`: resolve the document as the `doc_*` tools do, check the agent may read it and that
/// it opens as a note, publish [`NoteViewOpened`], return the receipt (211 §1).
pub struct ShowNoteTool {
    /// For publishing the event; `data_dir` locates knowledge files for on-demand documents.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowNoteTool {
    const NAME: &'static str = "show_note";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowNoteArgs;

    fn description(&self) -> String {
        "Open a note, board, knowledge page or meeting note in a tile beside the chat, for the user \
         to read or edit alongside it; it updates live as you edit with doc_insert/doc_replace. \
         `doc` is what the doc_* tools take: a document id, `knowledge:<concept id>` or \
         `meeting:<id>`; `block` is a block id to scroll to; `title` names the tile (default the \
         document's title). The same document again reuses its tile. `show_web` is for web pages, \
         `show` for files."
            .into()
    }

    async fn call(&self, args: ShowNoteArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let block = match args.block.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(b) if valid_block(b) => Some(b.to_owned()),
            Some(_) => {
                return Ok(ToolOutput::err(
                    "show_note: `block` is one block id from doc_read's outline",
                ));
            }
        };
        let Some(handle) = document::Handle::lookup() else {
            return Ok(ToolOutput::err("show_note: documents are disabled on this engine"));
        };
        let docs = document::tools::Docs::new(handle.actor().clone(), self.rt.data_dir.clone());
        let id = match docs.resolve(args.doc.trim()).await {
            Ok(id) => id,
            Err(e) => return Ok(ToolOutput::err(format!("show_note: {e}"))),
        };
        let meta = match handle
            .actor()
            .ask(document::MetaOf {
                doc: id.clone(),
                by: Principal::Agent {
                    session: cx.session.0,
                },
            })
            .await
        {
            Ok(m) => m,
            Err(kameo::error::SendError::HandlerError(e)) => {
                return Ok(ToolOutput::err(format!("show_note: {e}")));
            }
            Err(e) => return Ok(ToolOutput::err(format!("show_note: {e}"))),
        };
        let kind = match note_kind(meta.kind) {
            Ok(k) => k,
            Err(msg) => return Ok(ToolOutput::err(format!("show_note: {msg}"))),
        };
        let title = args
            .title
            .map(|t| clean_title(&t))
            .filter(|t| !t.is_empty())
            .or_else(|| Some(clean_title(&meta.title)).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| "Untitled".to_owned());
        let doc = id.to_string();
        self.rt
            .publish(NoteViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                doc: doc.clone(),
                kind: kind.clone(),
                title: title.clone(),
                block: block.clone(),
            })
            .await;
        let content = note_receipt(&title, &doc, block.as_deref(), crate::preview::feed_live());
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "session": cx.session,
                "doc": doc,
                "kind": kind,
                "title": title,
                "block": block,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// `show_whiteboard`: resolve the document, check the agent may read it and that it is a
/// whiteboard, bind the calling session to it, publish [`WhiteboardViewOpened`], return the
/// receipt (218 §3.4).
pub struct ShowWhiteboardTool {
    /// For publishing the event; `data_dir` locates knowledge files for on-demand documents.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowWhiteboardTool {
    const NAME: &'static str = "show_whiteboard";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowWhiteboardArgs;

    fn description(&self) -> String {
        "Open a whiteboard for the user: a tile beside the chat on desktop, the full-window \
         whiteboard on iPad. `whiteboard` is its document id (the `id` of its <whiteboard> block, \
         or what wb_create returned); `title` names the tile (default the whiteboard's title). \
         Opening it binds this chat to the whiteboard, so its outline and changes reach you. \
         Notes and boards open with `show_note`."
            .into()
    }

    async fn call(&self, args: ShowWhiteboardArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(handle) = document::Handle::lookup() else {
            return Ok(ToolOutput::err(
                "show_whiteboard: documents are disabled on this engine",
            ));
        };
        let docs = document::tools::Docs::new(handle.actor().clone(), self.rt.data_dir.clone());
        let id = match docs.resolve(args.whiteboard.trim()).await {
            Ok(id) => id,
            Err(e) => return Ok(ToolOutput::err(format!("show_whiteboard: {e}"))),
        };
        let meta = match handle
            .actor()
            .ask(document::MetaOf {
                doc: id.clone(),
                by: Principal::Agent {
                    session: cx.session.0,
                },
            })
            .await
        {
            Ok(m) => m,
            Err(e) => return Ok(ToolOutput::err(format!("show_whiteboard: {e}"))),
        };
        if let Some(msg) = not_a_whiteboard(&id.to_string(), meta.kind) {
            return Ok(ToolOutput::err(format!("show_whiteboard: {msg}")));
        }
        let title = args
            .title
            .map(|t| clean_title(&t))
            .filter(|t| !t.is_empty())
            .or_else(|| Some(clean_title(&meta.title)).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| "Whiteboard".to_owned());
        document::wb::hooks::bind(cx.session.0, &id);
        let doc = id.to_string();
        self.rt
            .publish(WhiteboardViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                doc: doc.clone(),
                title: title.clone(),
            })
            .await;
        let content = whiteboard_receipt(&title, &doc, crate::preview::feed_live());
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "session": cx.session,
                "doc": doc,
                "title": title,
                "href": format!("theuth://whiteboards/{doc}"),
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// `None` for a whiteboard; else the refusal, pointing note-tile kinds at `show_note`.
fn not_a_whiteboard(id: &str, kind: DocKind) -> Option<String> {
    if kind == DocKind::Whiteboard {
        return None;
    }
    let name = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    Some(match note_kind(kind) {
        Ok(_) => format!("{id} is a {name}; use show_note"),
        Err(_) => format!("{id} is a {name}, not a whiteboard"),
    })
}

/// The text the model reads; the first line is fixed (218 §3.4).
fn whiteboard_receipt(title: &str, doc: &str, live: bool) -> String {
    let mut s = format!(
        "opened whiteboard {}\ntheuth://whiteboards/{doc}",
        serde_json::to_string(title).unwrap_or_default()
    );
    if !live {
        let _ = write!(s, "\n{NO_APP}");
    }
    s
}

/// A block id: one token of id characters (no spaces, `#`, `/` or controls), ≤ [`MAX_BLOCK`].
fn valid_block(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= MAX_BLOCK
        && b
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The serde name of a kind that opens in a note tile, or the refusal with a hint.
fn note_kind(kind: DocKind) -> Result<String, String> {
    let name = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let hint = match kind {
        DocKind::Note | DocKind::Board | DocKind::Knowledge | DocKind::MeetingNote => {
            return Ok(name);
        }
        DocKind::Notebook | DocKind::Source => {
            "notebooks and sources open in their own pages in the app"
        }
        DocKind::Artifact => "hand the file over with `show` instead",
        DocKind::AgentCharter => "a charter opens in the Agents view",
        DocKind::TaskThread => "a task thread opens with its task",
        DocKind::Whiteboard => return Err("this is a whiteboard; use show_whiteboard".to_owned()),
    };
    Err(format!(
        "{name} documents do not open in a note tile; {hint}"
    ))
}

/// The text the model reads; the first line is fixed (211 §1).
fn note_receipt(title: &str, doc: &str, block: Option<&str>, live: bool) -> String {
    let mut s = format!("note view opened · {title} · tile note:{doc}\ntheuth://notes/{doc}");
    if let Some(b) = block {
        let _ = write!(s, "#{b}");
    }
    if !live {
        let _ = write!(s, "\n{NO_APP}");
    }
    s.push_str("\nIt updates live as you edit it with doc_insert/doc_replace.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::JobId;

    fn job(id: &str, name: Option<&str>, command: &str, state: &str, pty: bool) -> JobView {
        JobView {
            session: SessionId::new(),
            id: JobId(id.into()),
            command: command.into(),
            state: state.into(),
            secs: 1,
            log: "/tmp/x.log".into(),
            tail: String::new(),
            name: name.map(Into::into),
            pty,
            exit_code: None,
            started_ms: None,
            ended_ms: None,
            kind: None,
            call_id: None,
            log_bytes: None,
        }
    }

    fn session() -> SessionId {
        serde_json::from_value(serde_json::json!("00000000-0000-0000-0000-000000000001")).unwrap()
    }

    fn tile(pty: bool, running: bool) -> JobTile {
        JobTile {
            job: "j3".into(),
            title: "dev server".into(),
            running,
            pty,
        }
    }

    #[test]
    fn pty_receipt_first_line() {
        let live = job_receipt(&tile(true, true), session(), None, true);
        assert_eq!(
            live,
            "pty view opened · dev server · tile pty:00000000-0000-0000-0000-000000000001/j3\n\
             The user can type into this terminal and the tile resizes it; you keep using screen()/send() as before."
        );
        let done = job_receipt(&tile(true, false), session(), None, false);
        assert_eq!(
            done,
            "pty view opened · dev server · tile pty:00000000-0000-0000-0000-000000000001/j3\n\
             note: no app is connected; the card in the chat opens it later\n\
             The user can type into this terminal and the tile resizes it; you keep using screen()/send() as before.\n\
             The job has finished; the tile shows its final screen."
        );
    }

    #[test]
    fn job_receipt_first_line() {
        let r = job_receipt(&tile(false, true), session(), None, true);
        assert_eq!(
            r.lines().next().unwrap(),
            "job view opened · dev server · tile job:00000000-0000-0000-0000-000000000001/j3"
        );
        assert!(r.contains("read-only"), "{r}");
        let done = job_receipt(&tile(false, false), session(), None, true);
        assert!(done.ends_with("\nThe job has finished; the tile shows its log."), "{done}");
    }

    #[test]
    fn spawned_receipt_second_line() {
        let sp = Spawned {
            command: "exec /bin/zsh -l".into(),
            name: Some("shell".into()),
            cwd: "/work".into(),
        };
        let r = job_receipt(&tile(true, true), session(), Some(&sp), false);
        let lines: Vec<_> = r.lines().collect();
        assert_eq!(
            lines[0],
            "pty view opened · dev server · tile pty:00000000-0000-0000-0000-000000000001/j3"
        );
        assert_eq!(lines[1], "started j3 (shell) in /work: exec /bin/zsh -l");
        assert_eq!(
            lines[2],
            "It is a normal job: screen()/send()/cancel_job work on it."
        );
        assert_eq!(lines[3], NO_APP);
        let unnamed = Spawned { name: None, ..sp };
        let r = job_receipt(&tile(true, true), session(), Some(&unnamed), true);
        assert_eq!(r.lines().nth(1).unwrap(), "started j3 in /work: exec /bin/zsh -l");
    }

    #[test]
    fn note_receipt_first_line() {
        let doc = "01jabcdefghjkmnpqrstvwxyz0";
        assert_eq!(doc.len(), 26);
        assert_eq!(
            note_receipt("Plan", doc, None, true),
            format!(
                "note view opened · Plan · tile note:{doc}\ntheuth://notes/{doc}\n\
                 It updates live as you edit it with doc_insert/doc_replace."
            )
        );
        assert_eq!(
            note_receipt("Plan", doc, Some("b12"), false),
            format!(
                "note view opened · Plan · tile note:{doc}\ntheuth://notes/{doc}#b12\n\
                 {NO_APP}\nIt updates live as you edit it with doc_insert/doc_replace."
            )
        );
    }

    #[test]
    fn event_wire_shapes() {
        let session = SessionId::new();
        let v = serde_json::to_value(PtyViewOpened {
            session,
            call_id: "c".into(),
            job: "j3".into(),
            name: Some("dev".into()),
            title: "dev".into(),
            running: true,
            pty: false,
            spawned: false,
        })
        .unwrap();
        assert_eq!(v["type"], "PtyViewOpened");
        assert_eq!(v["session"], serde_json::json!(session));
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["call_id", "job", "name", "pty", "running", "session", "spawned", "title", "type"]
        );
        let v = serde_json::to_value(NoteViewOpened {
            session,
            call_id: "c".into(),
            doc: "01jabcdefghjkmnpqrstvwxyz0".into(),
            kind: "meeting_note".into(),
            title: "Standup".into(),
            block: None,
        })
        .unwrap();
        assert_eq!(v["type"], "NoteViewOpened");
        assert_eq!(v["block"], serde_json::Value::Null);
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["block", "call_id", "doc", "kind", "session", "title", "type"]
        );
    }

    #[test]
    fn name_resolves_to_id() {
        let jobs = [
            job("j1", Some("dev"), "npm run dev", "exited 0", true),
            job("j2", Some("dev"), "npm run dev", "running", true),
            job("j3", Some("dev"), "npm run dev", "killed", true),
            job("j4", None, "cargo build", "running", false),
        ];
        // A background `shell()` job resolves by id like a terminal.
        assert_eq!(resolve_job(&jobs, "j4").unwrap().id.0, "j4");
        // A running job with the name wins over a newer finished one.
        assert_eq!(resolve_job(&jobs, "dev").unwrap().id.0, "j2");
        // No running one: the newest with the name.
        assert_eq!(resolve_job(&jobs[..1], "dev").unwrap().id.0, "j1");
        assert_eq!(
            resolve_job(&[jobs[0].clone(), jobs[2].clone()], "dev")
                .unwrap()
                .id
                .0,
            "j3"
        );
        assert!(resolve_job(&jobs, "nope").is_none());
    }

    #[test]
    fn default_titles() {
        assert_eq!(
            default_job_title(&job("j1", Some("dev"), "npm run dev", "running", true)),
            "dev"
        );
        assert_eq!(
            default_job_title(&job("j1", None, "cargo  build\n--release", "running", false)),
            "cargo build --release"
        );
        assert_eq!(
            default_job_title(&job("j1", Some(" "), "", "running", true)),
            "j1"
        );
    }

    #[test]
    fn plans_and_refusals() {
        let args = |job: Option<&str>, command: Option<&str>| ShowPtyArgs {
            job: job.map(Into::into),
            command: command.map(Into::into),
            ..ShowPtyArgs::default()
        };
        assert_eq!(plan(&args(Some("j3"), None)), Ok(Plan::Show("j3".into())));
        assert_eq!(
            plan(&args(None, Some("npm run dev"))),
            Ok(Plan::Spawn {
                command: Some("npm run dev".into()),
                name: None
            })
        );
        // Nothing (or blanks): the login shell.
        assert_eq!(
            plan(&args(None, Some("  "))),
            Ok(Plan::Spawn {
                command: None,
                name: None
            })
        );
        assert_eq!(
            plan(&args(Some("j3"), Some("ls"))),
            Err("give job or command, not both".into())
        );
        let named = ShowPtyArgs {
            name: Some("x".into()),
            ..args(Some("j3"), None)
        };
        assert!(plan(&named).unwrap_err().contains("only when starting"));
    }

    #[test]
    fn login_shell_names_and_quoting() {
        let running = |name: &str| job("j1", Some(name), "zsh", "running", true);
        assert_eq!(free_name(&[], "shell"), "shell");
        assert_eq!(free_name(&[running("shell")], "shell"), "shell-2");
        assert_eq!(
            free_name(&[running("shell"), running("shell-2")], "shell"),
            "shell-3"
        );
        // A finished `shell` does not hold the name.
        let done = job("j1", Some("shell"), "zsh", "exited 0", true);
        assert_eq!(free_name(&[done], "shell"), "shell");
        assert!(login_shell().starts_with("exec ") && login_shell().ends_with(" -l"));
        assert_eq!(shell_word("/bin/zsh"), "/bin/zsh");
        assert_eq!(shell_word("/opt/my shell/fish"), "'/opt/my shell/fish'");
        assert_eq!(shell_word("/a'b"), r"'/a'\''b'");
    }

    #[test]
    fn note_kinds() {
        assert_eq!(note_kind(DocKind::Note).unwrap(), "note");
        assert_eq!(note_kind(DocKind::Board).unwrap(), "board");
        assert_eq!(note_kind(DocKind::Knowledge).unwrap(), "knowledge");
        assert_eq!(note_kind(DocKind::MeetingNote).unwrap(), "meeting_note");
        let nb = note_kind(DocKind::Notebook).unwrap_err();
        assert!(
            nb.starts_with("notebook documents do not open in a note tile; "),
            "{nb}"
        );
        assert!(note_kind(DocKind::Artifact).unwrap_err().contains("`show`"));
        for k in [DocKind::Source, DocKind::AgentCharter, DocKind::TaskThread] {
            assert!(note_kind(k).is_err(), "{k:?}");
        }
    }

    #[test]
    fn block_ids() {
        assert!(valid_block("b12"));
        assert!(valid_block("01J-ab_c"));
        for bad in ["", "a b", "a#b", "../x", &"a".repeat(MAX_BLOCK + 1)] {
            assert!(!valid_block(bad), "{bad}");
        }
    }

    /// 218 §3.4: the event goes out under its declared topic with the §4.3 shape, and the receipt's
    /// first line is fixed.
    #[tokio::test]
    async fn show_whiteboard_publishes_the_event() {
        use wire::RuntimeWire as _;
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        crate::declare_wire(&rt).await.unwrap();
        let mut rx = rt.wire_events();
        let doc = "01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b";
        rt.publish(WhiteboardViewOpened {
            session: session(),
            call_id: "toolu_1".into(),
            doc: doc.into(),
            title: "Attention inbox".into(),
        })
        .await;
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            frame.event,
            serde_json::json!({
                "type": "WhiteboardViewOpened",
                "session": "00000000-0000-0000-0000-000000000001",
                "call_id": "toolu_1",
                "doc": doc,
                "title": "Attention inbox",
            })
        );
        assert_eq!(
            frame.descriptor.as_ref().unwrap().topic,
            "artifact.whiteboard-view-opened"
        );
        assert_eq!(
            whiteboard_receipt("Attention \"inbox\"", doc, true),
            format!("opened whiteboard \"Attention \\\"inbox\\\"\"\ntheuth://whiteboards/{doc}")
        );
        assert!(whiteboard_receipt("A", doc, false).ends_with(NO_APP));
    }

    #[test]
    fn show_note_refuses_a_whiteboard() {
        assert_eq!(
            note_kind(DocKind::Whiteboard).unwrap_err(),
            "this is a whiteboard; use show_whiteboard"
        );
    }

    #[test]
    fn show_whiteboard_refuses_a_note() {
        assert_eq!(not_a_whiteboard("d1", DocKind::Whiteboard), None);
        assert_eq!(
            not_a_whiteboard("d1", DocKind::Note).as_deref(),
            Some("d1 is a note; use show_note")
        );
        assert_eq!(
            not_a_whiteboard("d1", DocKind::Notebook).as_deref(),
            Some("d1 is a notebook, not a whiteboard")
        );
    }
}
