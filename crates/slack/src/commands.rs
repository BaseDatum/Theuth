//! `/wth <sub-command>` (design doc 21 §10.2): the 09 §9 table as
//! sub-commands, each one ask plus a Block Kit panel from [`crate::ui`].
//! The channel classifies the payload's `text`, decides ack-now vs
//! `response_url` with [`is_instant`], routes `cmd:*` clicks through
//! [`action`] and modal submits through [`view_submission`], and acts on
//! [`Outcome`] side effects.

use std::collections::HashMap;
use std::fmt::Display;

use agent::{AGENT_ACTOR, Agents};
use anyhow::Context as _;
use kameo::actor::ActorRef;
use runtime::SessionId;

pub use crate::ui::Reply;
use crate::ui::{self, short};
use crate::{SLACK_ACTOR, SlackStatus};

/// A parsed `/wth` sub-command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// `help`.
    Help,
    /// `status`.
    Status,
    /// `sessions`.
    Sessions,
    /// `resume <id>` (full, unique prefix or suffix).
    Resume(String),
    /// `cancel [id]`: the thread's session or an explicit one.
    Cancel(Option<String>),
    /// `compact [id]`.
    Compact(Option<String>),
    /// `model [id]`: list or set.
    Model(Option<String>),
    /// `agents`.
    Agents,
    /// `prompt`.
    Prompt,
    /// `skills [add <url>|rm <name>|sync]`.
    Skills(Vec<String>),
    /// `mcp [add <url|json>|sync]`.
    Mcp(Vec<String>),
    /// `login [name]`.
    Login(Option<String>),
    /// `logout [name]`.
    Logout(Option<String>),
    /// `approvals on|off [always]` (owner).
    Approvals(Vec<String>),
    /// `slack [status|allow <U>|rm <U>|channel add <C>|channel rm <C>]` (owner).
    Slack(Vec<String>),
    /// Unknown or ambiguous first word.
    Unknown(String),
}

/// What the channel does after a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Send this reply.
    Text(Reply),
    /// `resume`: bind a new thread to `session`, rooted at a message with the reply text.
    Resume {
        /// The resolved session.
        session: SessionId,
        /// Its current title.
        title: String,
        /// `resumed <id> · <title>`.
        reply: Reply,
    },
    /// `cancel`: the turn was cancelled; the channel updates its thread.
    Cancelled {
        /// The cancelled session.
        session: SessionId,
        /// `cancelled <id>`.
        reply: Reply,
    },
}

/// What a command runs against.
pub struct CommandCx<'a> {
    /// Invoking user id.
    pub user: &'a str,
    /// `slack.owner`.
    pub owner: &'a str,
    /// Channel the command was typed in.
    pub channel: &'a str,
    /// The invoker's latest thread here (21 §10.2), or `None`.
    pub session: Option<SessionId>,
    /// Snapshot for `status` and `slack status`.
    pub status: SlackStatus,
    /// Page of a list panel: 0 from the slash command, `n` from a `More…` click.
    pub page: usize,
    /// Thread permalinks of bound sessions, for the `agents` panel's links.
    pub links: HashMap<SessionId, String>,
}

/// Name, argument hint, one-line description (09 §9 table, 21 §10.2).
pub(crate) const COMMANDS: &[(&str, &str, &str)] = &[
    ("help", "", "this list"),
    ("status", "", "connection, session, model, turn"),
    ("sessions", "", "10 newest sessions"),
    (
        "resume",
        "<id>",
        "open a new thread on a session (prefix ok)",
    ),
    ("cancel", "[id]", "abort the turn"),
    ("compact", "[id]", "fold the context"),
    ("model", "[id]", "show or set the model"),
    (
        "agents",
        "",
        "this session's subagents: name · state · last · age",
    ),
    ("prompt", "", "the assembled system prompt"),
    ("skills", "[add <url>|rm <name>|sync]", "skills"),
    ("mcp", "[add <url|json>|sync]", "MCP servers"),
    ("login", "[name]", "list logins or start one"),
    ("logout", "<name>", "log out"),
    (
        "approvals",
        "on|off [always]",
        "tool approval prompts (owner)",
    ),
    (
        "slack",
        "[status|allow <U>|rm <U>|channel add <C>|channel rm <C>]",
        "this channel (owner)",
    ),
];

const OWNER_MSG: &str = "This command is restricted to the owner.";
const NO_SESSION: &str = "no active thread here; run it in a DM with the bot or /wth resume <id>";
/// `prompt` body cap: at most four ≤ 3 000-char sections.
const PROMPT_MAX: usize = 11_900;

/// The sub-command in a slash payload's `text`: first word lowercased,
/// exact name else the unique name it prefixes; empty is `help`.
#[must_use]
pub fn classify(text: &str) -> Cmd {
    let text = text.trim();
    let (word, arg) = text
        .split_once(char::is_whitespace)
        .map_or((text, ""), |(w, a)| (w, a.trim()));
    let word = word.to_lowercase();
    if word.is_empty() {
        return Cmd::Help;
    }
    let opt = || (!arg.is_empty()).then(|| arg.to_owned());
    let words = || arg.split_whitespace().map(str::to_owned).collect();
    match complete(&word) {
        Some("help") => Cmd::Help,
        Some("status") => Cmd::Status,
        Some("sessions") => Cmd::Sessions,
        Some("resume") => Cmd::Resume(arg.to_owned()),
        Some("cancel") => Cmd::Cancel(opt()),
        Some("compact") => Cmd::Compact(opt()),
        Some("model") => Cmd::Model(opt()),
        Some("agents") => Cmd::Agents,
        Some("prompt") => Cmd::Prompt,
        Some("skills") => Cmd::Skills(words()),
        Some("mcp") => Cmd::Mcp(words()),
        Some("login") => Cmd::Login(opt()),
        Some("logout") => Cmd::Logout(opt()),
        Some("approvals") => Cmd::Approvals(words()),
        Some("slack") => Cmd::Slack(words()),
        _ => Cmd::Unknown(word),
    }
}

/// Exact name, else the unique command starting with `word`.
fn complete(word: &str) -> Option<&'static str> {
    let names = COMMANDS.iter().map(|c| c.0);
    if let Some(n) = names.clone().find(|n| *n == word) {
        return Some(n);
    }
    let mut hits = names.filter(|n| n.starts_with(word));
    match (hits.next(), hits.next()) {
        (Some(n), None) => Some(n),
        _ => None,
    }
}

/// The answer is pure formatting of at most one fast ask, so the channel
/// can return it in the 3 s ack instead of through `response_url`.
#[must_use]
pub fn is_instant(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Help
        | Cmd::Status
        | Cmd::Sessions
        | Cmd::Model(None)
        | Cmd::Agents
        | Cmd::Unknown(_) => true,
        Cmd::Approvals(args) => args.is_empty(),
        _ => false,
    }
}

fn owner_only(cmd: &Cmd) -> bool {
    matches!(cmd, Cmd::Approvals(_) | Cmd::Slack(_))
}

/// Needs `cx.session` (21 §10.2's session-scoped set).
fn session_scoped(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Cancel(None) | Cmd::Compact(None) | Cmd::Model(Some(_)) | Cmd::Agents | Cmd::Prompt
    )
}

/// Runs `cmd`. Owner-only commands from a non-owner answer [`OWNER_MSG`]
/// and session-scoped ones without a session [`NO_SESSION`], both before
/// any actor is asked; ask failures become `error: …` text.
pub async fn handle(cmd: Cmd, cx: CommandCx<'_>) -> Outcome {
    if owner_only(&cmd) && cx.user != cx.owner {
        return Outcome::Text(Reply::text(OWNER_MSG));
    }
    if session_scoped(&cmd) && cx.session.is_none() {
        return Outcome::Text(Reply::text(NO_SESSION));
    }
    match run(cmd, &cx).await {
        Ok(out) => out,
        Err(e) => Outcome::Text(Reply::text(format!("error: {e:#}"))),
    }
}

/// A `cmd:*` click (21 §10.2 table) as the command it stands for, run
/// through [`handle`] so the same gates apply. `value` is a button's
/// `value` or a select's chosen option; `cmd:<panel>:page:<n>` re-renders
/// page `n`. An unknown id answers `unknown action`.
pub async fn action(action_id: &str, value: Option<&str>, mut cx: CommandCx<'_>) -> Outcome {
    let unknown = || Outcome::Text(Reply::text(format!("unknown action {action_id:?}")));
    let Some(rest) = action_id.strip_prefix("cmd:") else {
        return unknown();
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let words = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    let cmd = match parts.as_slice() {
        ["sessions"] => Cmd::Sessions,
        ["agents"] => Cmd::Agents,
        ["models"] => Cmd::Model(None),
        ["status" | "refresh"] => Cmd::Status,
        ["resume", id] => Cmd::Resume((*id).to_owned()),
        ["cancel", id] => Cmd::Cancel(Some((*id).to_owned())),
        ["compact", id] => Cmd::Compact(Some((*id).to_owned())),
        ["model"] => match value {
            Some(model) => Cmd::Model(Some(model.to_owned())),
            None => return unknown(),
        },
        ["skills", "sync"] => Cmd::Skills(words(&["sync"])),
        ["mcp", "sync"] => Cmd::Mcp(words(&["sync"])),
        ["login", name] => Cmd::Login(Some((*name).to_owned())),
        ["logout", name] => Cmd::Logout(Some((*name).to_owned())),
        ["approvals"] => match value {
            Some("ask") => Cmd::Approvals(words(&["on"])),
            Some("off_session") => Cmd::Approvals(words(&["off"])),
            Some("off_always") => Cmd::Approvals(words(&["off", "always"])),
            _ => return unknown(),
        },
        [panel, "page", n] => {
            let Ok(page) = n.parse() else {
                return unknown();
            };
            cx.page = page;
            match *panel {
                "sessions" => Cmd::Sessions,
                "skills" => Cmd::Skills(Vec::new()),
                "mcp" => Cmd::Mcp(Vec::new()),
                _ => return unknown(),
            }
        }
        _ => return unknown(),
    };
    handle(cmd, cx).await
}

/// A submitted Add… modal ([`ui::modal_add`]) as the command it stands
/// for: `cmd:skills:add` → `skills add <input>`, `cmd:mcp:add` → `mcp add <input>`.
#[must_use]
pub fn view_submission(callback_id: &str, input: &str) -> Cmd {
    let add = || vec!["add".to_owned(), input.trim().to_owned()];
    match callback_id {
        "cmd:skills:add" => Cmd::Skills(add()),
        "cmd:mcp:add" => Cmd::Mcp(add()),
        _ => Cmd::Unknown(callback_id.to_owned()),
    }
}

async fn run(cmd: Cmd, cx: &CommandCx<'_>) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Text(ui::result(&s)));
    let panel = |r: Reply| Ok(Outcome::Text(r));
    match cmd {
        Cmd::Help => panel(ui::help()),
        Cmd::Unknown(word) => {
            let mut r = ui::help();
            r.text = format!("unknown sub-command {word:?}");
            panel(r)
        }
        Cmd::Status => panel(status(cx).await?),
        Cmd::Sessions => {
            let rows: Vec<_> = agent()?
                .ask(agent::ListSessions)
                .await
                .map_err(err)?
                .into_iter()
                .filter(|s| s.parent.is_none())
                .collect();
            panel(ui::sessions(&rows, cx.page))
        }
        Cmd::Resume(id) => {
            if id.is_empty() {
                return text("usage: /wth resume <id>".into());
            }
            match resolve(&id).await? {
                Ok(s) => Ok(Outcome::Resume {
                    session: s.id,
                    reply: ui::result(&format!("resumed {} · {}", short(s.id), s.title)),
                    title: s.title,
                }),
                Err(msg) => text(msg),
            }
        }
        Cmd::Cancel(id) => {
            let session = match session_or(cx, id.as_deref()).await? {
                Ok(s) => s,
                Err(msg) => return text(msg),
            };
            agent()?.ask(agent::Cancel { session }).await.map_err(err)?;
            Ok(Outcome::Cancelled {
                session,
                reply: ui::result(&format!("cancelled {}", short(session))),
            })
        }
        Cmd::Compact(id) => {
            let session = match session_or(cx, id.as_deref()).await? {
                Ok(s) => s,
                Err(msg) => return text(msg),
            };
            let epoch = agent()?
                .ask(agent::Compact {
                    session,
                    style: None,
                })
                .await
                .map_err(err)?;
            text(format!("compacted → epoch {epoch}"))
        }
        Cmd::Model(None) => {
            let agent = agent()?;
            let models = agent.ask(agent::ListModels).await.map_err(err)?;
            let current = match cx.session {
                Some(session) => agent
                    .ask(agent::ListSessions)
                    .await
                    .map_err(err)?
                    .into_iter()
                    .find(|s| s.id == session)
                    .map(|s| s.model)
                    .unwrap_or_default(),
                None => String::new(),
            };
            panel(ui::models(&models, &current))
        }
        Cmd::Model(Some(model)) => {
            let info = agent()?
                .ask(agent::SetModel {
                    session: cx.session,
                    model,
                })
                .await
                .map_err(err)?;
            text(format!("model: {}", info.id))
        }
        Cmd::Agents => {
            let session = cx.session.context(NO_SESSION)?;
            let rows = agent()?
                .ask(agent::ListAgents { session })
                .await
                .map_err(err)?;
            panel(ui::agents(&rows, &cx.links))
        }
        Cmd::Prompt => {
            let prompt = agent()?
                .ask(agent::ShowPrompt {
                    session: cx.session,
                })
                .await
                .map_err(err)?;
            let mut body: String = prompt.chars().take(PROMPT_MAX).collect();
            if body.len() < prompt.len() {
                body.push_str("\n…");
            }
            panel(ui::code(&body))
        }
        Cmd::Skills(args) => skills(&args, cx.page).await,
        Cmd::Mcp(args) => mcp_cmd(&args, cx.page).await,
        Cmd::Login(None) => logins().await,
        Cmd::Login(Some(name)) => {
            let url = if is_provider(&name).await? {
                agent()?
                    .ask(agent::Login {
                        provider: name.clone(),
                    })
                    .await
                    .map_err(err)?
            } else {
                mcp()?
                    .ask(tools::McpLogin {
                        server: name.clone(),
                    })
                    .await
                    .map_err(err)?
            };
            panel(ui::login_url(&name, &url))
        }
        Cmd::Logout(None) => text("usage: /wth logout <name>".into()),
        Cmd::Logout(Some(name)) => {
            if is_provider(&name).await? {
                agent()?
                    .ask(agent::Logout {
                        provider: name.clone(),
                    })
                    .await
                    .map_err(err)?;
            } else {
                mcp()?
                    .ask(tools::McpLogout {
                        server: name.clone(),
                    })
                    .await
                    .map_err(err)?;
            }
            text(format!("logged out of {name}"))
        }
        Cmd::Approvals(args) => approvals_cmd(cx.session, &args).await,
        Cmd::Slack(args) => slack_cmd(cx, &args).await,
    }
}

/// `cx.session`, or `id` resolved like `resume`. `Err` is reply text.
async fn session_or(
    cx: &CommandCx<'_>,
    id: Option<&str>,
) -> anyhow::Result<Result<SessionId, String>> {
    match id {
        None => Ok(Ok(cx.session.context(NO_SESSION)?)),
        Some(id) => Ok(resolve(id).await?.map(|s| s.id)),
    }
}

/// The one session whose id equals, starts or ends with `id`; `Err` is reply text.
async fn resolve(id: &str) -> anyhow::Result<Result<agent::SessionSummary, String>> {
    let rows = agent()?.ask(agent::ListSessions).await.map_err(err)?;
    let mut hits = rows.into_iter().filter(|s| {
        let full = s.id.to_string();
        full == id || full.starts_with(id) || full.ends_with(id)
    });
    Ok(match (hits.next(), hits.next()) {
        (Some(s), None) => Ok(s),
        (Some(_), Some(_)) => Err(format!("{id:?} matches several sessions")),
        (None, _) => Err(format!("no session {id:?}")),
    })
}

/// `status`: the channel snapshot plus the bound session's model and turn.
async fn status(cx: &CommandCx<'_>) -> anyhow::Result<Reply> {
    let Some(session) = cx.session else {
        return Ok(ui::status(&cx.status, None));
    };
    // ponytail: LoadSession renders the whole history just for `running`
    // and `model`; ask the worker for its TurnStarted flag if this shows.
    let loaded = agent()?
        .ask(agent::LoadSession {
            session,
            before: None,
            limit: None,
        })
        .await
        .map_err(err)?;
    let turn = if loaded.running { "running" } else { "idle" };
    let line = format!("{} · {} · turn: {turn}", short(session), loaded.model);
    Ok(ui::status(&cx.status, Some(&line)))
}

/// `skills [add <url> | rm <name> | sync]`.
async fn skills(args: &[String], page: usize) -> anyhow::Result<Outcome> {
    let (sub, rest) = split(args);
    let agent = agent()?;
    let out = match (sub, rest.as_str()) {
        ("", _) => {
            let rows = agent.ask(agent::ListSkills).await.map_err(err)?;
            return Ok(Outcome::Text(ui::skills(&rows, page)));
        }
        ("sync", _) => synced(agent.ask(agent::SyncSkills).await.map_err(err)?),
        ("add", url) if !url.is_empty() => {
            let added = agent
                .ask(agent::AddSkillRepo {
                    url: url.to_owned(),
                    name: None,
                })
                .await
                .map_err(err)?;
            format!(
                "added {} skills: {}",
                added.len(),
                added
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        // A repo dir first (only removed when it holds `.git`), else a local skill.
        ("rm", name) if !name.is_empty() => {
            let repo = agent::RemoveSkillRepo {
                name: name.to_owned(),
            };
            if agent.ask(repo).await.is_ok() {
                format!("removed repo {name}")
            } else {
                agent
                    .ask(agent::RemoveSkill {
                        name: name.to_owned(),
                    })
                    .await
                    .map_err(err)?;
                format!("removed skill {name}")
            }
        }
        _ => "usage: /wth skills [add <url> | rm <name> | sync]".to_owned(),
    };
    Ok(Outcome::Text(ui::result(&out)))
}

/// `mcp [add <url|json> | sync]`.
async fn mcp_cmd(args: &[String], page: usize) -> anyhow::Result<Outcome> {
    // ponytail: the JSON came in as words and is rejoined with single
    // spaces; only string values with runs of spaces notice.
    let (sub, rest) = split(args);
    let mcp = mcp()?;
    let out = match (sub, rest.as_str()) {
        ("", _) => {
            let rows = mcp.ask(tools::ListServers::default()).await.map_err(err)?;
            return Ok(Outcome::Text(ui::mcp(&rows, page)));
        }
        ("sync", _) => synced(mcp.ask(tools::SyncMcp).await.map_err(err)?),
        ("add", src) if git_url(src) => {
            let repo = mcp
                .ask(tools::AddRepo {
                    url: src.to_owned(),
                    name: None,
                })
                .await
                .map_err(err)?;
            format!("cloned {} → {}", repo.name, repo.path)
        }
        ("add", src) if !src.is_empty() => {
            let (name, entry) = mcp_entry(src).map_err(|e| anyhow::anyhow!("mcp add: {e}"))?;
            let server = mcp
                .ask(tools::AddServer { name, entry })
                .await
                .map_err(err)?;
            format!("added {} ({} tools)", server.name, server.tools.len())
        }
        _ => "usage: /wth mcp [add <url|json> | sync]".to_owned(),
    };
    Ok(Outcome::Text(ui::result(&out)))
}

/// `login`: providers, then MCP servers that use OAuth.
async fn logins() -> anyhow::Result<Outcome> {
    let providers = agent()?.ask(agent::ListLogins).await.map_err(err)?;
    let servers = mcp()?
        .ask(tools::ListServers::default())
        .await
        .map_err(err)?;
    Ok(Outcome::Text(ui::logins(&providers, &servers)))
}

/// `name` is an OAuth provider (else it is taken as an MCP server).
async fn is_provider(name: &str) -> anyhow::Result<bool> {
    Ok(agent()?
        .ask(agent::ListLogins)
        .await
        .map_err(err)?
        .iter()
        .any(|l| l.name == name))
}

/// `approvals [on|off [always]]`: bare shows the radio panel; `always`
/// switches the persisted default.
async fn approvals_cmd(session: Option<SessionId>, args: &[String]) -> anyhow::Result<Outcome> {
    let mut words = args.iter().map(String::as_str);
    let enabled = match words.next() {
        None => return Ok(Outcome::Text(ui::approvals(None))),
        Some("on") => true,
        Some("off") => false,
        Some(_) => {
            return Ok(Outcome::Text(Reply::text(
                "usage: /wth approvals on|off [always]",
            )));
        }
    };
    let session = match words.next() {
        Some("always") => None,
        None => Some(session.context(
            "no active thread here; run it in a DM with the bot, or use `/wth approvals off always`",
        )?),
        Some(w) => {
            return Ok(Outcome::Text(Reply::text(format!(
                "approvals: expected `always`, got {w:?}"
            ))));
        }
    };
    let state = ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)?
        .context("tool registry is not running")?
        .ask(tools::SetApprovals { session, enabled })
        .await
        .map_err(err)?;
    let current = match (state.global, state.session) {
        (false, _) => "off_always",
        (true, false) => "off_session",
        (true, true) => "ask",
    };
    let mut r = ui::approvals(Some(current));
    r.text = format!(
        "approvals: {} (this session), {} (default)",
        on_off(state.session),
        on_off(state.global)
    );
    Ok(Outcome::Text(r))
}

/// `slack [status | allow <U> | rm <U> | channel add <C> | channel rm <C>]`.
async fn slack_cmd(cx: &CommandCx<'_>, args: &[String]) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Text(ui::result(&s)));
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["status"] => Ok(Outcome::Text(ui::slack_panel(&cx.status))),
        ["allow", u] => {
            ask_slack(crate::Allow((*u).to_owned())).await?;
            text(format!("allowed {u}"))
        }
        ["rm", u] => {
            ask_slack(crate::RemoveAllowed((*u).to_owned())).await?;
            text(format!("removed {u}"))
        }
        ["channel", "add", c] => {
            ask_slack(crate::AllowChannel((*c).to_owned())).await?;
            text(format!("channel added {c}"))
        }
        ["channel", "rm", c] => {
            ask_slack(crate::RemoveChannel((*c).to_owned())).await?;
            text(format!("channel removed {c}"))
        }
        _ => text(
            "usage: /wth slack [status | allow <U> | rm <U> | channel add <C> | channel rm <C>]"
                .into(),
        ),
    }
}

/// Asks the `"slack"` actor so `lib.rs` keeps config persistence in one place.
async fn ask_slack<M>(m: M) -> anyhow::Result<()>
where
    M: Send + 'static,
    crate::Slack: kameo::message::Message<M, Reply = Result<(), String>>,
{
    ActorRef::<crate::Slack>::lookup(SLACK_ACTOR)?
        .context("slack actor is not running")?
        .ask(m)
        .await
        .map_err(err)
}

fn agent() -> anyhow::Result<ActorRef<Agents>> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)?.context("agent actor is not running")
}

fn mcp() -> anyhow::Result<ActorRef<tools::Mcp>> {
    ActorRef::<tools::Mcp>::lookup(tools::MCP_ACTOR)?.context("mcp actor is not running")
}

fn err<E: Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// First word and the rest rejoined: `("add", "https://…")`.
fn split(args: &[String]) -> (&str, String) {
    match args.split_first() {
        Some((sub, rest)) => (sub.as_str(), rest.join(" ")),
        None => ("", String::new()),
    }
}

fn synced(rows: Vec<tools::git::SyncResult>) -> String {
    table(rows.into_iter().map(|r| vec![r.name, r.status]))
}

/// Rows as columns padded to equal width inside a ``` fence; `(none)` when empty.
fn table(rows: impl IntoIterator<Item = Vec<String>>) -> String {
    let rows: Vec<Vec<String>> = rows.into_iter().collect();
    if rows.is_empty() {
        return "(none)".to_owned();
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                if c > 0 {
                    line.push_str("  ");
                }
                line.push_str(cell);
                if c + 1 < r.len() {
                    line.extend(std::iter::repeat_n(' ', widths[c] - cell.chars().count()));
                }
            }
            line.trim_end().to_owned()
        })
        .collect();
    format!("```\n{}\n```", lines.join("\n"))
}

/// `mcp add` argument that means "clone a repo": a `.git` URL or a known git host.
fn git_url(s: &str) -> bool {
    s.ends_with(".git")
        || ["github.com/", "gitlab.com/", "codeberg.org/"]
            .iter()
            .any(|h| s.contains(h))
}

/// One MCP server from `mcp add <json>`: a server object with a `name` key
/// (stripped), `{ "<name>": {...} }`, or `{ "mcpServers": { "<name>": {...} } }`.
fn mcp_entry(text: &str) -> Result<(String, serde_json::Value), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let serde_json::Value::Object(mut obj) = v else {
        return Err("expected a JSON object".into());
    };
    if let Some(serde_json::Value::Object(servers)) = obj.remove("mcpServers") {
        obj = servers;
    }
    if obj.contains_key("command") || obj.contains_key("url") {
        return match obj.remove("name") {
            Some(serde_json::Value::String(name)) => Ok((name, serde_json::Value::Object(obj))),
            _ => Err("server object needs a \"name\" field".into()),
        };
    }
    match (obj.len(), obj.into_iter().next()) {
        (1, Some((name, entry))) if entry.is_object() => Ok((name, entry)),
        _ => Err("expected exactly one server: {\"<name>\": {\"command\": ...}}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> SlackStatus {
        SlackStatus {
            state: None,
            bot_user: None,
            team: None,
            owner: "U0WNER".into(),
            allowed: vec![],
            channels: vec![],
            threads: vec![],
            open_streams: 0,
            calls: Default::default(),
            dropped_envelopes: 0,
        }
    }

    fn cx<'a>(user: &'a str, session: Option<SessionId>) -> CommandCx<'a> {
        CommandCx {
            user,
            owner: "U0WNER",
            channel: "C1",
            session,
            status: status(),
            page: 0,
            links: HashMap::new(),
        }
    }

    fn text(s: &str) -> Outcome {
        Outcome::Text(Reply::text(s))
    }

    #[test]
    fn completes_unique_prefixes_only() {
        assert_eq!(classify("stat"), Cmd::Status);
        assert_eq!(classify("  STATUS "), Cmd::Status);
        assert_eq!(classify("s"), Cmd::Unknown("s".into()));
        assert_eq!(classify("mo gpt-5"), Cmd::Model(Some("gpt-5".into())));
        assert_eq!(classify("model"), Cmd::Model(None));
        assert_eq!(classify("resume abc123"), Cmd::Resume("abc123".into()));
        assert_eq!(classify("ca"), Cmd::Cancel(None));
        assert_eq!(classify("compact 1234"), Cmd::Compact(Some("1234".into())));
        assert_eq!(
            classify("skills add https://x/y.git"),
            Cmd::Skills(vec!["add".into(), "https://x/y.git".into()])
        );
        assert_eq!(classify("logi"), Cmd::Login(None));
        assert_eq!(classify("logo"), Cmd::Logout(None));
        assert_eq!(classify("log"), Cmd::Unknown("log".into()));
        assert_eq!(classify("zzz x"), Cmd::Unknown("zzz".into()));
        assert_eq!(classify(""), Cmd::Help);
    }

    #[test]
    fn help_panel_lists_every_command_once() {
        let h = serde_json::to_string(&ui::help().blocks).unwrap();
        for (name, _, _) in COMMANDS {
            assert_eq!(h.matches(&format!("`/wth {name}")).count(), 1, "{name}");
        }
    }

    #[tokio::test]
    async fn owner_only_commands_are_gated() {
        // No actor runs here: a gate that asked first would answer `error: …`.
        for cmd in [
            Cmd::Approvals(vec!["off".into()]),
            Cmd::Approvals(vec![]),
            Cmd::Slack(vec!["status".into()]),
            Cmd::Slack(vec!["allow".into(), "U2".into()]),
        ] {
            assert_eq!(
                handle(cmd.clone(), cx("U5TRANGER", None)).await,
                text(OWNER_MSG),
                "{cmd:?}"
            );
        }
        // The owner passes the gate; `slack status` needs no actor at all.
        let out = handle(Cmd::Slack(vec!["status".into()]), cx("U0WNER", None)).await;
        assert_ne!(out, text(OWNER_MSG));
    }

    #[tokio::test]
    async fn session_scoped_without_session_says_so() {
        for cmd in [
            Cmd::Cancel(None),
            Cmd::Compact(None),
            Cmd::Model(Some("gpt".into())),
            Cmd::Agents,
            Cmd::Prompt,
        ] {
            assert_eq!(
                handle(cmd.clone(), cx("U1", None)).await,
                text(NO_SESSION),
                "{cmd:?}"
            );
        }
        // Help never needs a session.
        assert_eq!(
            handle(Cmd::Help, cx("U1", None)).await,
            Outcome::Text(ui::help())
        );
    }

    #[test]
    fn instant_commands_are_the_formatting_only_ones() {
        assert!(is_instant(&Cmd::Help));
        assert!(is_instant(&Cmd::Model(None)));
        assert!(is_instant(&Cmd::Approvals(vec![])));
        assert!(!is_instant(&Cmd::Model(Some("x".into()))));
        assert!(!is_instant(&Cmd::Compact(None)));
        assert!(!is_instant(&Cmd::Resume("a".into())));
    }

    #[tokio::test]
    async fn actions_map_to_commands_with_the_same_gates() {
        // Session-scoped select without a thread: gate answers before any ask.
        assert_eq!(
            action("cmd:model", Some("openai/gpt-5"), cx("U1", None)).await,
            text(NO_SESSION)
        );
        // With a thread the SetModel ask goes out (and fails: no actor here).
        let Outcome::Text(r) =
            action("cmd:model", Some("x"), cx("U1", Some(SessionId::new()))).await
        else {
            panic!()
        };
        assert!(r.text.starts_with("error: agent actor"), "{}", r.text);
        // Owner-only radio from a stranger.
        assert_eq!(
            action("cmd:approvals", Some("off_always"), cx("U5TRANGER", None)).await,
            text(OWNER_MSG)
        );
        // Ids the panels never emit.
        assert_eq!(
            action("cmd:nope", None, cx("U1", None)).await,
            text("unknown action \"cmd:nope\"")
        );
        assert_eq!(
            action("approve", None, cx("U1", None)).await,
            text("unknown action \"approve\"")
        );
        // Help-panel buttons render panels without a session.
        assert_eq!(
            action("cmd:status", None, cx("U1", None)).await,
            Outcome::Text(ui::status(&status(), None))
        );
    }

    #[test]
    fn modal_submits_become_add_commands() {
        assert_eq!(
            view_submission("cmd:skills:add", " https://x/y.git "),
            Cmd::Skills(vec!["add".into(), "https://x/y.git".into()])
        );
        assert_eq!(
            view_submission("cmd:mcp:add", "{\"a\": {\"command\": \"x y\"}}"),
            Cmd::Mcp(vec!["add".into(), "{\"a\": {\"command\": \"x y\"}}".into()])
        );
        assert_eq!(view_submission("other", "x"), Cmd::Unknown("other".into()));
    }
}
