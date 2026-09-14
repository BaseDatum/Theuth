//! `/wth <sub-command>` (design doc 21 §10.2): the 09 §9 table as
//! sub-commands, each one ask plus a formatter, like `signal::commands`.
//! The channel classifies the payload's `text`, decides ack-now vs
//! `response_url` with [`is_instant`], and acts on [`Outcome`] side effects.

use std::fmt::Display;

use agent::{AGENT_ACTOR, Agents};
use anyhow::Context as _;
use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use runtime::SessionId;
use tools::McpAuth;

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
    /// Reply with this text.
    Text(String),
    /// `resume`: bind a new thread to `session`, rooted at a message with `text`.
    Resume {
        /// The resolved session.
        session: SessionId,
        /// Its current title.
        title: String,
        /// `resumed <id> · <title>`.
        text: String,
    },
    /// `cancel`: the turn was cancelled; the channel updates its thread.
    Cancelled {
        /// The cancelled session.
        session: SessionId,
        /// `cancelled <id>`.
        text: String,
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
}

/// Name, argument hint, one-line description (09 §9 table, 21 §10.2).
const COMMANDS: &[(&str, &str, &str)] = &[
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
/// `prompt` body cap so the fenced reply stays under Slack's 12 000-char message limit.
const PROMPT_MAX: usize = 11_900;

/// The help text: one line per sub-command inside a fence.
#[must_use]
pub fn help() -> String {
    table(
        COMMANDS
            .iter()
            .map(|(n, a, d)| vec![(*n).to_owned(), (*a).to_owned(), (*d).to_owned()]),
    )
}

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
        return Outcome::Text(OWNER_MSG.to_owned());
    }
    if session_scoped(&cmd) && cx.session.is_none() {
        return Outcome::Text(NO_SESSION.to_owned());
    }
    match run(cmd, &cx).await {
        Ok(out) => out,
        Err(e) => Outcome::Text(format!("error: {e:#}")),
    }
}

async fn run(cmd: Cmd, cx: &CommandCx<'_>) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Text(s));
    match cmd {
        Cmd::Help => text(help()),
        Cmd::Unknown(word) => text(format!("unknown sub-command {word:?}\n{}", help())),
        Cmd::Status => text(status(cx).await?),
        Cmd::Sessions => {
            let rows = agent()?.ask(agent::ListSessions).await.map_err(err)?;
            text(table(
                rows.iter()
                    .filter(|s| s.parent.is_none())
                    .take(10)
                    .map(|s| vec![short(s.id), s.title.clone(), age(s.updated_at)]),
            ))
        }
        Cmd::Resume(id) => {
            if id.is_empty() {
                return text("usage: /wth resume <id>".into());
            }
            match resolve(&id).await? {
                Ok(s) => Ok(Outcome::Resume {
                    session: s.id,
                    text: format!("resumed {} · {}", short(s.id), s.title),
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
                text: format!("cancelled {}", short(session)),
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
            let models = agent()?.ask(agent::ListModels).await.map_err(err)?;
            let mut out = table(models.iter().take(20).map(|m| {
                vec![
                    m.id.clone(),
                    format!("{}{}", k(m.context), if m.image { " · vision" } else { "" }),
                ]
            }));
            if models.len() > 20 {
                out.push_str(&format!(
                    "\n{} more; /wth model <id> to set",
                    models.len() - 20
                ));
            }
            text(out)
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
            if rows.is_empty() {
                return text("no subagents".to_owned());
            }
            text(table(rows.iter().map(|a| {
                let since = if a.state == agent::AgentState::Running {
                    a.started_at
                } else {
                    a.finished_at
                };
                vec![
                    path(a, &rows),
                    a.state.word().to_owned(),
                    a.last.chars().take(40).collect(),
                    since.map_or_else(String::new, age),
                ]
            })))
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
            text(format!("```\n{body}\n```"))
        }
        Cmd::Skills(args) => skills(&args).await,
        Cmd::Mcp(args) => mcp_cmd(&args).await,
        Cmd::Login(None) => logins().await,
        Cmd::Login(Some(name)) => {
            let url = if is_provider(&name).await? {
                agent()?
                    .ask(agent::Login { provider: name })
                    .await
                    .map_err(err)?
            } else {
                mcp()?
                    .ask(tools::McpLogin { server: name })
                    .await
                    .map_err(err)?
            };
            text(format!("login: open {url}"))
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
async fn status(cx: &CommandCx<'_>) -> anyhow::Result<String> {
    let mut out = crate::render_status(&cx.status);
    let Some(session) = cx.session else {
        out.push_str("\nsession: none");
        return Ok(out);
    };
    // ponytail: LoadSession renders the whole history just for `running`
    // and `model`; ask the worker for its TurnStarted flag if this shows.
    let loaded = agent()?
        .ask(agent::LoadSession { session })
        .await
        .map_err(err)?;
    let turn = if loaded.running { "running" } else { "idle" };
    out.push_str(&format!(
        "\nsession: {} · {} · turn: {turn}",
        short(session),
        loaded.model
    ));
    Ok(out)
}

/// `skills [add <url> | rm <name> | sync]`.
async fn skills(args: &[String]) -> anyhow::Result<Outcome> {
    let (sub, rest) = split(args);
    let agent = agent()?;
    let out = match (sub, rest.as_str()) {
        ("", _) => {
            let rows = agent.ask(agent::ListSkills).await.map_err(err)?;
            table(rows.iter().map(|s| {
                vec![
                    s.name.clone(),
                    if s.always { "always" } else { "" }.to_owned(),
                    s.description.clone(),
                ]
            }))
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
    Ok(Outcome::Text(out))
}

/// `mcp [add <url|json> | sync]`.
async fn mcp_cmd(args: &[String]) -> anyhow::Result<Outcome> {
    // ponytail: the JSON came in as words and is rejoined with single
    // spaces; only string values with runs of spaces notice.
    let (sub, rest) = split(args);
    let mcp = mcp()?;
    let out = match (sub, rest.as_str()) {
        ("", _) => {
            let rows = mcp.ask(tools::ListServers::default()).await.map_err(err)?;
            table(rows.iter().map(|s| {
                vec![
                    s.name.clone(),
                    if s.connected { "connected" } else { "off" }.to_owned(),
                    format!("{} tools", s.tools.len()),
                    match s.auth {
                        McpAuth::None => "",
                        McpAuth::Required => "login required",
                        McpAuth::LoggedIn => "logged in",
                    }
                    .to_owned(),
                ]
            }))
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
    Ok(Outcome::Text(out))
}

/// `login`: providers, then MCP servers that use OAuth.
async fn logins() -> anyhow::Result<Outcome> {
    let mut rows: Vec<Vec<String>> = agent()?
        .ask(agent::ListLogins)
        .await
        .map_err(err)?
        .into_iter()
        .map(|l| {
            let status = match (l.logged_in, l.account) {
                (false, _) => "not logged in".to_owned(),
                (true, Some(a)) => format!("logged in ({a})"),
                (true, None) => "logged in".to_owned(),
            };
            vec![l.name, status]
        })
        .collect();
    rows.extend(
        mcp()?
            .ask(tools::ListServers::default())
            .await
            .map_err(err)?
            .into_iter()
            .filter(|s| s.auth != McpAuth::None)
            .map(|s| {
                let status = if s.auth == McpAuth::LoggedIn {
                    "logged in (mcp)"
                } else {
                    "login required (mcp)"
                };
                vec![s.name, status.to_owned()]
            }),
    );
    Ok(Outcome::Text(table(rows)))
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

/// `approvals on|off [always]`: `always` switches the persisted default.
async fn approvals_cmd(session: Option<SessionId>, args: &[String]) -> anyhow::Result<Outcome> {
    let mut words = args.iter().map(String::as_str);
    let enabled = match words.next() {
        Some("on") => true,
        Some("off") => false,
        _ => {
            return Ok(Outcome::Text(
                "usage: /wth approvals on|off [always]".into(),
            ));
        }
    };
    let session = match words.next() {
        Some("always") => None,
        None => Some(session.context(
            "no active thread here; run it in a DM with the bot, or use `/wth approvals off always`",
        )?),
        Some(w) => {
            return Ok(Outcome::Text(format!(
                "approvals: expected `always`, got {w:?}"
            )));
        }
    };
    let state = ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)?
        .context("tool registry is not running")?
        .ask(tools::SetApprovals { session, enabled })
        .await
        .map_err(err)?;
    Ok(Outcome::Text(format!(
        "approvals: {} (this session), {} (default)",
        on_off(state.session),
        on_off(state.global)
    )))
}

/// `slack [status | allow <U> | rm <U> | channel add <C> | channel rm <C>]`.
async fn slack_cmd(cx: &CommandCx<'_>, args: &[String]) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Text(s));
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["status"] => text(crate::render_status(&cx.status)),
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

/// Last 8 hex chars, as the TUI shows ids; `resume` accepts them.
fn short(id: SessionId) -> String {
    let s = id.to_string();
    s[s.len() - 8..].to_owned()
}

/// `12.3k`, `200k`, `1.0M`; `?` for 0.
fn k(n: u32) -> String {
    match n {
        0 => "?".to_owned(),
        1..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", f64::from(n) / 1000.0),
        _ => format!("{:.1}M", f64::from(n) / 1_000_000.0),
    }
}

/// `5s`, `12m`, `3h`, `2d` since `t`.
fn age(t: DateTime<Utc>) -> String {
    let s = (Utc::now() - t).num_seconds().max(0);
    match s {
        ..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// `parent/name` from the asked session down: a depth-1 child is its bare
/// name, a grandchild `parent/name`.
fn path(a: &agent::AgentInfo, rows: &[agent::AgentInfo]) -> String {
    match rows.iter().find(|p| p.id == a.parent) {
        Some(p) => format!("{}/{}", path(p, rows), a.name),
        None => a.name.clone(),
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
        }
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
    fn help_lists_every_command_once() {
        let h = help();
        for (name, _, _) in COMMANDS {
            assert_eq!(h.matches(&format!("\n{name} ")).count(), 1, "{name}");
        }
        assert!(h.starts_with("```\n") && h.ends_with("\n```"));
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
                Outcome::Text(OWNER_MSG.into()),
                "{cmd:?}"
            );
        }
        // The owner passes the gate; `slack status` needs no actor at all.
        let out = handle(Cmd::Slack(vec!["status".into()]), cx("U0WNER", None)).await;
        assert_ne!(out, Outcome::Text(OWNER_MSG.into()));
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
                Outcome::Text(NO_SESSION.into()),
                "{cmd:?}"
            );
        }
        // Help never needs a session.
        assert_eq!(
            handle(Cmd::Help, cx("U1", None)).await,
            Outcome::Text(help())
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
}
