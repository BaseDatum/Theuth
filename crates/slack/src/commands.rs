//! `/theuth <sub-command>` (design doc 21 §10.2): the 09 §9 table as
//! sub-commands, each one ask plus a Block Kit panel from [`crate::ui`].
//! The channel classifies the payload's `text`, decides ack-now vs
//! `response_url` with [`is_instant`], routes `cmd:*` clicks through
//! [`action`] and modal submits through [`view_submission`], and acts on
//! [`Outcome`] side effects.

use std::collections::HashMap;
use std::fmt::Display;

use agent::communication::{
    AnswerPeerRequest, CommunicationState, GetCommunication, GetPeerMessage, ListPeerMessages,
    ListPeers, PeerMessagePage, PeerRecord, PeerScope, SetCommunication, SetPeerScope,
};
use agent::{
    AGENT_ACTOR, Agents, BroadcastOutcome, BroadcastReceipt, BroadcastRequest, BroadcastTargets,
    Delivery,
};
use anyhow::Context as _;
use kameo::actor::ActorRef;
use runtime::SessionId;
use uuid::Uuid;

pub use crate::ui::Reply;
use crate::ui::{self, short};
use crate::{SLACK_ACTOR, SlackStatus};

/// A parsed `/theuth` sub-command.
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
    /// `communication [on|off [always]]` (owner, 50 §2.1).
    Communication(Vec<String>),
    /// `peers [<query> | scope owner|isolated [alias]]` (owner, 50 §2.3/§5.1).
    Peers(Vec<String>),
    /// `requests [<id> | answer <id> <text>]` (owner, 50 §5.3).
    Requests(Vec<String>),
    /// `a2a [reload]` (owner, 50 §6).
    A2a(Vec<String>),
    /// `broadcast [--next] running|roots|<id>[,<id>…] <text>` (owner, 62 §5.3).
    Broadcast(String),
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
        "communication",
        "[on|off [always]]",
        "agent-to-agent messaging: state, this session's switch, `always` the global one (owner)",
    ),
    (
        "peers",
        "[<query>|scope owner|isolated [alias]]",
        "who this thread's session may message; set its root scope (owner)",
    ),
    (
        "requests",
        "[<id>|answer <id> <text>]",
        "pending peer requests by exact id: list, inspect, answer (owner)",
    ),
    (
        "a2a",
        "[reload]",
        "network status; reload peers.yaml (owner)",
    ),
    (
        "broadcast",
        "[--next] running|roots|<id>[,<id>…] <text>",
        "one message to many sessions; --next lands after the turn (owner)",
    ),
    (
        "slack",
        "[status|allow <U>|rm <U>|channel add <C>|channel rm <C>]",
        "this channel (owner)",
    ),
];

const OWNER_MSG: &str = "This command is restricted to the owner.";
const NO_SESSION: &str =
    "no active thread here; run it in a DM with the bot or /theuth resume <id>";
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
        Some("communication") => Cmd::Communication(words()),
        Some("peers") => Cmd::Peers(words()),
        Some("requests") => Cmd::Requests(words()),
        Some("a2a") => Cmd::A2a(words()),
        Some("broadcast") => Cmd::Broadcast(arg.to_owned()),
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
        Cmd::Approvals(args) | Cmd::Communication(args) => args.is_empty(),
        _ => false,
    }
}

/// Owner-only: approvals, this channel's allowlist and everything that
/// widens who an agent may talk to (50 §10: a contact cannot grant scope,
/// enable global communication or reload network credentials).
fn owner_only(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Approvals(_)
            | Cmd::Slack(_)
            | Cmd::Communication(_)
            | Cmd::Peers(_)
            | Cmd::Requests(_)
            | Cmd::A2a(_)
            | Cmd::Broadcast(_)
    )
}

/// Needs `cx.session` (21 §10.2's session-scoped set).
fn session_scoped(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Cancel(None)
            | Cmd::Compact(None)
            | Cmd::Model(Some(_))
            | Cmd::Agents
            | Cmd::Prompt
            | Cmd::Peers(_)
            | Cmd::Requests(_)
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
                .filter(|s| s.parent.is_none() && s.desk.is_none())
                .collect();
            panel(ui::sessions(&rows, cx.page))
        }
        Cmd::Resume(id) => {
            if id.is_empty() {
                return text("usage: /theuth resume <id>".into());
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
            agent()?.ask(agent::Cancel { session, keep: None }).await.map_err(err)?;
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
                    head: None,
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
            // 82 §2: in a thread the switch is that thread's; only the thread-less
            // form has nothing but the default to set.
            let info = agent()?
                .ask(agent::SetModel {
                    session: cx.session,
                    model,
                    manual: cx.session.is_none(),
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
        Cmd::Logout(None) => text("usage: /theuth logout <name>".into()),
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
        Cmd::Communication(args) => communication_cmd(cx.session, &args).await,
        Cmd::Peers(args) => peers_cmd(cx.session.context(NO_SESSION)?, &args).await,
        Cmd::Requests(args) => requests_cmd(cx.session.context(NO_SESSION)?, &args).await,
        Cmd::A2a(args) => a2a_cmd(&args).await,
        Cmd::Broadcast(arg) => broadcast_cmd(&arg).await,
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
            settled_only: false,
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
        _ => "usage: /theuth skills [add <url> | rm <name> | sync]".to_owned(),
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
        _ => "usage: /theuth mcp [add <url|json> | sync]".to_owned(),
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
                "usage: /theuth approvals on|off [always]",
            )));
        }
    };
    let session = match words.next() {
        Some("always") => None,
        None => Some(session.context(
            "no active thread here; run it in a DM with the bot, or use `/theuth approvals off always`",
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
            "usage: /theuth slack [status | allow <U> | rm <U> | channel add <C> | channel rm <C>]"
                .into(),
        ),
    }
}

/// `communication [on|off [always]]` (50 §2.1): bare reads; `on|off`
/// moves this session's memory-only opt-out; `always` moves the persisted
/// global ceiling. The reply is the authoritative state, blocker included.
async fn communication_cmd(session: Option<SessionId>, args: &[String]) -> anyhow::Result<Outcome> {
    let mut words = args.iter().map(String::as_str);
    let enabled = match words.next() {
        None => {
            let state = agent()?
                .ask(GetCommunication { session })
                .await
                .map_err(err)?;
            return Ok(Outcome::Text(ui::code(&communication_text(&state))));
        }
        Some("on") => true,
        Some("off") => false,
        Some(_) => {
            return Ok(Outcome::Text(Reply::text(
                "usage: /theuth communication [on|off [always]]",
            )));
        }
    };
    let target = match words.next() {
        Some("always") => None,
        None => Some(session.context(
            "no active thread here; run it in a DM with the bot, or use `/theuth communication on|off always` for the global switch",
        )?),
        Some(w) => {
            return Ok(Outcome::Text(Reply::text(format!(
                "communication: expected `always`, got {w:?}"
            ))));
        }
    };
    let state = agent()?
        .ask(SetCommunication {
            session: target,
            enabled,
        })
        .await
        .map_err(err)?;
    let mut body = communication_text(&state);
    if enabled && target.is_some() && !state.global {
        body.insert_str(
            0,
            "still disabled globally; use /theuth communication on always to enable the harness\n",
        );
    }
    Ok(Outcome::Text(ui::code(&body)))
}

/// `broadcast [--next] running|roots|<id>[,<id>…] <text>` (62 §5.3): one
/// message to the selected sessions; the reply is the receipt, one line per
/// row.
async fn broadcast_cmd(arg: &str) -> anyhow::Result<Outcome> {
    let (delivery, selector, text) = match parse_broadcast(arg) {
        Ok(p) => p,
        Err(usage) => return Ok(Outcome::Text(Reply::text(usage))),
    };
    let targets = match selector {
        Selector::Running => BroadcastTargets::Running,
        Selector::Roots => BroadcastTargets::Roots,
        Selector::Ids(ids) => {
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match resolve(id).await? {
                    Ok(s) => out.push(s.id),
                    Err(msg) => return Ok(Outcome::Text(Reply::text(msg))),
                }
            }
            BroadcastTargets::Sessions { ids: out }
        }
    };
    // kameo flattens a `Result` reply: the supervisor's refusal is `HandlerError`.
    let receipt = agent()?
        .ask(agent::Broadcast(BroadcastRequest {
            targets,
            text,
            files: vec![],
            delivery,
            dry_run: false,
        }))
        .await;
    Ok(Outcome::Text(match receipt {
        Ok(r) => ui::code(&receipt_text(&r)),
        Err(kameo::error::SendError::HandlerError(e)) => {
            Reply::text(format!("broadcast refused: {e}"))
        }
        Err(e) => return Err(err(e)),
    }))
}

/// The `broadcast` selector word before id resolution.
#[derive(Debug, PartialEq, Eq)]
enum Selector {
    Running,
    Roots,
    /// Raw ids: full UUIDs or the prefixes `resume` accepts.
    Ids(Vec<String>),
}

/// `[--next] running|roots|<id>[,<id>…] <text>` → delivery, selector, text.
/// `Err` is the usage line.
fn parse_broadcast(arg: &str) -> Result<(Delivery, Selector, String), String> {
    const USAGE: &str = "usage: /theuth broadcast [--next] running|roots|<id>[,<id>…] <text>";
    let (delivery, rest) = match arg.trim().strip_prefix("--next ") {
        Some(r) => (Delivery::NextTurn, r.trim_start()),
        None => (Delivery::Steer, arg.trim()),
    };
    let (sel, text) = rest
        .split_once(char::is_whitespace)
        .map(|(s, t)| (s, t.trim()))
        .filter(|(_, t)| !t.is_empty())
        .ok_or_else(|| USAGE.to_owned())?;
    let selector = match sel {
        "running" => Selector::Running,
        "roots" => Selector::Roots,
        ids => Selector::Ids(
            ids.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
    };
    if matches!(&selector, Selector::Ids(v) if v.is_empty()) {
        return Err(USAGE.to_owned());
    }
    Ok((delivery, selector, text.to_owned()))
}

/// `broadcast <short-id> → N sessions`, then `title — outcome` per row.
fn receipt_text(r: &BroadcastReceipt) -> String {
    if r.rows.is_empty() {
        return "no matching sessions".to_owned();
    }
    let id = r.id.simple().to_string();
    let mut out = format!("broadcast {} → {} sessions", &id[..4], r.rows.len());
    for row in &r.rows {
        let title = if row.title.is_empty() {
            short(row.session)
        } else {
            row.title.clone()
        };
        let outcome = match &row.outcome {
            BroadcastOutcome::Started => "started".to_owned(),
            BroadcastOutcome::Queued { delivery, .. } => format!(
                "queued ({})",
                match delivery {
                    Delivery::Steer => "steer",
                    Delivery::NextTurn => "next turn",
                }
            ),
            BroadcastOutcome::Skipped { reason } => format!("skipped: {reason}"),
            BroadcastOutcome::Failed { error } => format!("failed: {error}"),
        };
        out.push_str(&format!("\n{title} — {outcome}"));
    }
    out
}

/// [`CommunicationState`] as lines: effective state, the global ceiling and
/// its source, network, this session's switch and scope, then the exact
/// blocker when off (50 §2.1 table) and the restart-lifetime note.
fn communication_text(s: &CommunicationState) -> String {
    let mut lines = vec![format!(
        "communication: {} · global {} (from {}) · network {} · listener {}",
        on_off(s.effective),
        on_off(s.global),
        s.source,
        if s.network_enabled {
            "enabled"
        } else {
            "disabled"
        },
        if s.listening { "up" } else { "down" },
    )];
    if let Some(session) = s.session {
        let alias = s
            .alias
            .as_deref()
            .map(|a| format!(" · alias {a}"))
            .unwrap_or_default();
        lines.push(format!(
            "session {}: {} · scope {}{alias}",
            short(session),
            if s.opted_out { "opted out" } else { "on" },
            s.scope.as_str(),
        ));
    }
    match (s.global, s.inherited_from) {
        (false, _) => lines.push(
            "blocked: global off is a ceiling; /theuth communication on always enables the harness"
                .into(),
        ),
        (true, Some(from)) if Some(from) != s.session => lines.push(format!(
            "blocked: ancestor {} opted out; /theuth communication on in that thread lifts it",
            short(from)
        )),
        (true, Some(_)) => {
            lines.push("blocked: this session opted out; /theuth communication on lifts it".into());
        }
        (true, None) => {}
    }
    if s.session.is_some() {
        lines.push("session switches are memory-only: they last until the daemon restarts".into());
    }
    lines.join("\n")
}

/// `peers [<query> | scope owner|isolated [alias]]` (50 §5.1, §2.3): the
/// directory `session` may address, or its root's scope and alias.
async fn peers_cmd(session: SessionId, args: &[String]) -> anyhow::Result<Outcome> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["scope"] => Ok(Outcome::Text(Reply::text(
            "usage: /theuth peers scope owner|isolated [alias]",
        ))),
        ["scope", scope, alias @ ..] => {
            let Ok(scope) = scope.parse::<PeerScope>() else {
                return Ok(Outcome::Text(Reply::text(
                    "usage: /theuth peers scope owner|isolated [alias]",
                )));
            };
            let alias = (!alias.is_empty()).then(|| alias.join(" "));
            let state = agent()?
                .ask(SetPeerScope {
                    root: session,
                    scope,
                    alias,
                })
                .await
                .map_err(err)?;
            Ok(Outcome::Text(ui::code(&communication_text(&state))))
        }
        _ => {
            let query = (!words.is_empty()).then(|| words.join(" "));
            let page = agent()?
                .ask(ListPeers {
                    session,
                    query,
                    cursor: None,
                    limit: None,
                    active_only: false,
                })
                .await
                .map_err(err)?;
            let rows = page.peers.iter().map(|p| {
                let caps: Vec<&str> = [(p.send, "send"), (p.reply, "reply"), (p.stream, "stream")]
                    .into_iter()
                    .filter_map(|(on, c)| on.then_some(c))
                    .collect();
                vec![
                    p.name.clone(),
                    p.address.clone(),
                    lower(&p.kind),
                    lower(&p.relationship),
                    lower(&p.state),
                    caps.join("+"),
                ]
            });
            let mut body = table(rows);
            if page.cursor.is_some() {
                body.push_str("\n(more: narrow with /theuth peers <query>)");
            }
            Ok(Outcome::Text(ui::result(&body)))
        }
    }
}

/// `requests [<id> | answer <id> <text>]` (50 §5.3): the requests a human
/// may resolve for `session` now, one record, or an answer by exact id (a
/// unique prefix of a pending id is accepted; never an arbitrary waiter).
async fn requests_cmd(session: SessionId, args: &[String]) -> anyhow::Result<Outcome> {
    let usage = || -> anyhow::Result<Outcome> {
        Ok(Outcome::Text(Reply::text(
            "usage: /theuth requests [<id> | answer <id> <text>]",
        )))
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => {
            let page = pending(session).await?;
            let mut body = table(page.messages.iter().map(|m| {
                let side = if page.awaiting_answer.contains(&m.receipt.id) {
                    "answer for this session"
                } else if page.awaiting_reply.contains(&m.receipt.id) {
                    "resolve this session's wait"
                } else {
                    m.receipt.state.as_str()
                };
                vec![
                    m.receipt.id.to_string(),
                    side.to_owned(),
                    format!("{} → {}", m.from, m.receipt.to),
                    ui::age(m.created_at),
                    crate::channel::clip(m.text.lines().next().unwrap_or(""), 60),
                ]
            }));
            if !page.messages.is_empty() {
                body.push_str(
                    "\n/theuth requests <id> shows one; /theuth requests answer <id> <text> resolves it",
                );
            }
            Ok(Outcome::Text(ui::result(&body)))
        }
        ["answer"] | ["answer", _] => usage(),
        ["answer", id, text @ ..] => {
            let reply_to = match request_id(session, id).await? {
                Ok(id) => id,
                Err(msg) => return Ok(Outcome::Text(Reply::text(msg))),
            };
            let receipt = agent()?
                .ask(AnswerPeerRequest {
                    session,
                    reply_to,
                    text: text.join(" "),
                })
                .await
                .map_err(err)?;
            Ok(Outcome::Text(ui::result(&format!(
                "answered {} → {} · {} · by {}",
                receipt.id,
                receipt.to,
                receipt.state.as_str(),
                receipt.answered_by.as_deref().unwrap_or("?")
            ))))
        }
        [id] => {
            let id = match request_id(session, id).await? {
                Ok(id) => id,
                Err(msg) => return Ok(Outcome::Text(Reply::text(msg))),
            };
            let record = agent()?
                .ask(GetPeerMessage { session, id })
                .await
                .map_err(err)?;
            Ok(Outcome::Text(ui::code(&record_text(&record))))
        }
        _ => usage(),
    }
}

async fn pending(session: SessionId) -> anyhow::Result<PeerMessagePage> {
    agent()?
        .ask(ListPeerMessages {
            session,
            cursor: None,
            limit: None,
            pending_only: true,
        })
        .await
        .map_err(err)
}

/// `id` as a full UUID, else the one pending request id it prefixes; `Err` is reply text.
async fn request_id(session: SessionId, id: &str) -> anyhow::Result<Result<Uuid, String>> {
    if let Ok(id) = id.parse::<Uuid>() {
        return Ok(Ok(id));
    }
    let page = pending(session).await?;
    let mut hits = page
        .messages
        .iter()
        .map(|m| m.receipt.id)
        .filter(|u| u.to_string().starts_with(id));
    Ok(match (hits.next(), hits.next()) {
        (Some(u), None) => Ok(u),
        (Some(_), Some(_)) => Err(format!("{id:?} matches several pending requests")),
        (None, _) => Err(format!("no pending request {id:?}")),
    })
}

/// One [`PeerRecord`] as `key: value` lines, the body last.
fn record_text(r: &PeerRecord) -> String {
    let mut lines = vec![
        format!("id: {}", r.receipt.id),
        format!("from: {}", r.from),
        format!("to: {}", r.receipt.to),
        format!("state: {}", r.receipt.state.as_str()),
        format!("age: {}", ui::age(r.created_at)),
    ];
    let opt = |k: &str, v: Option<String>| v.map(|v| format!("{k}: {v}"));
    lines.extend(
        [
            opt("reply_to", r.receipt.reply_to.map(|u| u.to_string())),
            opt("request_scope", r.request_scope.map(|u| u.to_string())),
            opt("task_id", r.receipt.task_id.clone()),
            opt("answered_by", r.receipt.answered_by.clone()),
            opt("error", r.receipt.error.clone()),
            opt("reply", r.receipt.reply.clone()),
        ]
        .into_iter()
        .flatten(),
    );
    lines.push(String::new());
    lines.push(r.text.clone());
    lines.join("\n")
}

/// `a2a [reload]` (50 §6): network status, or validate and swap
/// `peers.yaml`. No actor with `a2a.enabled` off is "disabled"; no actor
/// with it on is an error, never a silent off.
async fn a2a_cmd(args: &[String]) -> anyhow::Result<Outcome> {
    let reload = match args {
        [] => false,
        [w] if w == "reload" => true,
        _ => return Ok(Outcome::Text(Reply::text("usage: /theuth a2a [reload]"))),
    };
    let actor = ActorRef::<a2a::A2aActor>::lookup(a2a::A2A_ACTOR)?;
    let Some(actor) = actor else {
        let state = agent()?
            .ask(GetCommunication { session: None })
            .await
            .map_err(err)?;
        anyhow::ensure!(!state.network_enabled, "a2a actor is not running");
        return Ok(Outcome::Text(ui::result(
            "a2a: disabled (a2a.enabled is off; enable it in config and restart)",
        )));
    };
    let status = if reload {
        actor.ask(a2a::Reload).await.map_err(err)?
    } else {
        actor.ask(a2a::GetStatus).await.map_err(err)?
    };
    let mut lines = vec![format!(
        "a2a: {} · listener {}{}{}",
        if status.enabled {
            "enabled"
        } else {
            "disabled"
        },
        if status.listening {
            "up"
        } else {
            "down (needs global communication on)"
        },
        status
            .address
            .as_deref()
            .map(|a| format!(" at {a}"))
            .unwrap_or_default(),
        status
            .public_url
            .as_deref()
            .map(|u| format!(" · public {u}"))
            .unwrap_or_default(),
    )];
    if let Some(e) = &status.error {
        lines.push(format!("error: {e}"));
    }
    if reload {
        lines.push("reloaded peers.yaml; listener settings need a restart".into());
    }
    lines.push(format!(
        "peers:\n{}",
        table(status.peers.iter().map(|p| vec![
            p.name.clone(),
            match (p.outbound, p.inbound) {
                (true, true) => "in+out",
                (true, false) => "out",
                (false, true) => "in",
                (false, false) => "-",
            }
            .to_owned(),
            p.sessions.iter().map(|s| short(*s)).collect::<Vec<_>>().join(","),
            p.exports.join(","),
            p.card.clone().unwrap_or_default(),
            p.error.clone().unwrap_or_default(),
        ]))
    ));
    lines.push(format!(
        "exports:\n{}",
        table(status.exports.iter().map(|x| vec![
            x.name.clone(),
            short(x.session),
            x.peer.clone().unwrap_or_else(|| "(no peer)".into()),
            x.url.clone(),
        ]))
    ));
    Ok(Outcome::Text(ui::result(&lines.join("\n"))))
}

/// A unit-variant enum's serde word: `Debug` lowercased (`A2a` → `a2a`).
fn lower<T: std::fmt::Debug>(v: &T) -> String {
    format!("{v:?}").to_lowercase()
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
        assert_eq!(classify("comm on"), Cmd::Communication(vec!["on".into()]));
        assert_eq!(classify("co"), Cmd::Unknown("co".into()));
        assert_eq!(classify("pe"), Cmd::Peers(vec![]));
        assert_eq!(classify("req"), Cmd::Requests(vec![]));
        assert_eq!(classify("a2a reload"), Cmd::A2a(vec!["reload".into()]));
        assert_eq!(
            classify("br --next roots  wrap up"),
            Cmd::Broadcast("--next roots  wrap up".into())
        );
        assert_eq!(classify(""), Cmd::Help);
    }

    #[test]
    fn help_panel_lists_every_command_once() {
        let h = serde_json::to_string(&ui::help().blocks).unwrap();
        for (name, _, _) in COMMANDS {
            assert_eq!(h.matches(&format!("`/theuth {name}")).count(), 1, "{name}");
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
            Cmd::Communication(vec![]),
            Cmd::Communication(vec!["on".into(), "always".into()]),
            Cmd::Peers(vec!["scope".into(), "owner".into()]),
            Cmd::Requests(vec!["answer".into(), "x".into(), "y".into()]),
            Cmd::A2a(vec!["reload".into()]),
            Cmd::Broadcast("running hello".into()),
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
        // Owner-only and session-scoped: the owner still needs a thread.
        for cmd in [Cmd::Peers(vec![]), Cmd::Requests(vec![])] {
            assert_eq!(
                handle(cmd.clone(), cx("U0WNER", None)).await,
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
    fn communication_text_names_the_exact_blocker() {
        let me = SessionId::new();
        let parent = SessionId::new();
        let state = |global, inherited_from| CommunicationState {
            global,
            source: "config".into(),
            session: Some(me),
            opted_out: inherited_from == Some(me),
            inherited_from,
            effective: global && inherited_from.is_none(),
            scope: PeerScope::Isolated,
            alias: None,
            network_enabled: false,
            listening: false,
            revision: 1,
        };
        // Global off is the ceiling even when nothing else blocks.
        let t = communication_text(&state(false, None));
        assert!(t.contains("blocked: global off"), "{t}");
        assert!(t.contains("on always"), "{t}");
        // An ancestor's opt-out is named as the ancestor, not this session.
        let t = communication_text(&state(true, Some(parent)));
        assert!(t.contains(&format!("ancestor {}", short(parent))), "{t}");
        // Own opt-out.
        let t = communication_text(&state(true, Some(me)));
        assert!(t.contains("this session opted out"), "{t}");
        assert!(t.contains("memory-only"), "{t}");
        // On: no blocker line.
        let t = communication_text(&state(true, None));
        assert!(!t.contains("blocked"), "{t}");
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

    #[test]
    fn broadcast_parses_selector_delivery_and_text() {
        let ok = |s| parse_broadcast(s).unwrap();
        assert_eq!(
            ok("running stop touching Cargo.lock"),
            (
                Delivery::Steer,
                Selector::Running,
                "stop touching Cargo.lock".into()
            )
        );
        assert_eq!(
            ok("--next roots  wrap up"),
            (Delivery::NextTurn, Selector::Roots, "wrap up".into())
        );
        assert_eq!(
            ok("abcd1234,ef56 hi there"),
            (
                Delivery::Steer,
                Selector::Ids(vec!["abcd1234".into(), "ef56".into()]),
                "hi there".into()
            )
        );
        for bad in ["", "running", "--next roots", ", hi", "--next"] {
            assert!(
                parse_broadcast(bad).is_err_and(|u| u.starts_with("usage: /theuth broadcast")),
                "{bad}"
            );
        }
        assert!(!is_instant(&Cmd::Broadcast("running x".into())));
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
