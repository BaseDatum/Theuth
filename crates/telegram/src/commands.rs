//! Native bot commands (design doc 98 §10.3): the one `(name, hint,
//! description)` table behind `setMyCommands` and `/help`, the three scopes,
//! and the dispatcher — one ask plus a formatter per name, the Signal arms
//! (`crates/signal/src/commands.rs`) with panels from [`ui`] where 98 §10.3
//! says so. The channel (ROUTE) classifies the message, fills [`CommandCx`]
//! and acts on the [`Outcome`].

use std::fmt::Display;

use agent::{
    AGENT_ACTOR, Agents, BroadcastOutcome, BroadcastReceipt, BroadcastRequest, BroadcastTargets,
    Delivery, PeerScope,
};
use anyhow::Context as _;
use kameo::actor::ActorRef;
use runtime::SessionId;

use crate::api::{BotCommandSpec, CommandScope, Keyboard};
use crate::port::Port;
use crate::ui::{self, age, k, short};
use crate::{TelegramStatus, render_status};

/// Name without slash, argument hint, one-line description (09 §9 table
/// minus `approve` / `deny` / `signal`, plus `start` and `telegram`).
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("start", "[code]", "pair with a code, or this list"),
    ("help", "", "this list"),
    ("status", "", "polling state, session, model, turn"),
    ("new", "", "start a new thread"),
    ("sessions", "", "10 newest sessions"),
    (
        "resume",
        "<id>",
        "bind this thread to a session (prefix ok)",
    ),
    (
        "cancel",
        "[name]",
        "abort the turn, or a subagent's by name",
    ),
    ("compact", "[summary|cut|snap|style …]", "fold the context"),
    (
        "clear",
        "",
        "drop the conversation, keep the thread's settings",
    ),
    ("undo", "", "drop the last turn (redo restores)"),
    ("redo", "", "restore the last undone turn"),
    ("model", "[id]", "show or set the model"),
    (
        "effort",
        "[level|clear]",
        "show or set the reasoning effort",
    ),
    (
        "agents",
        "",
        "this session's subagents: name · state · last · age",
    ),
    ("jobs", "[kill <id>]", "list background jobs"),
    ("prompt", "", "the assembled system prompt"),
    ("skills", "[add <url>|rm <name>|sync]", "skills"),
    ("mcp", "[add <url|json>|sync]", "MCP servers"),
    ("login", "[name]", "list logins or start one"),
    ("logout", "<name>", "log out"),
    (
        "provider",
        "[add [name [url [key]]] | refresh <name> | rm <name>]",
        "built-in and custom providers",
    ),
    (
        "approvals",
        "on|off [always]",
        "tool approval prompts (owner)",
    ),
    (
        "telegram",
        "[status|link|allow <id>|rm <id>|group add <id>|group rm <id>]",
        "this channel (owner)",
    ),
    (
        "communication",
        "[on|off [always]]",
        "peer messaging for this session or the default (owner)",
    ),
    (
        "peers",
        "[<query> | scope owner|isolated [alias]]",
        "this session's peer directory or root scope (owner)",
    ),
    (
        "requests",
        "[<id> | answer <id> <text>]",
        "pending peer requests: inspect or answer one (owner)",
    ),
    (
        "broadcast",
        "[--next] running|roots|<id>[,<id>…] <text>",
        "one message to many sessions; --next lands after the turn (owner)",
    ),
    ("a2a", "[reload]", "network status or config reload (owner)"),
];

const OWNER_ONLY: &[&str] = &[
    "approvals",
    "telegram",
    "communication",
    "peers",
    "requests",
    "a2a",
    "broadcast",
];

/// Commands in the group scope (98 §10.3), each `is_ephemeral`.
const GROUP: &[&str] = &[
    "help", "status", "new", "sessions", "resume", "cancel", "compact", "model", "agents",
];

/// The refusal a non-owner gets for an owner-only command (09 §9).
pub const OWNER_MSG: &str = "This command is restricted to the owner.";
/// The refusal a session-scoped command gets without a thread.
pub const NO_SESSION: &str = "no session yet: send a message first";

/// Owner-only names (09 §9 with `telegram` in place of `signal`).
#[must_use]
pub fn owner_only(name: &str) -> bool {
    OWNER_ONLY.contains(&name)
}

fn spec(name: &str, description: &str, ephemeral: bool) -> BotCommandSpec {
    BotCommandSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        ephemeral,
    }
}

/// The three `setMyCommands` scopes (98 §10.3): default = public set; the
/// owner's chat = everything; all groups = [`GROUP`] flagged ephemeral.
#[must_use]
pub fn scopes(owner: Option<i64>) -> Vec<(CommandScope, Vec<BotCommandSpec>)> {
    let public: Vec<BotCommandSpec> = COMMANDS
        .iter()
        .filter(|(n, _, _)| !owner_only(n))
        .map(|(n, _, d)| spec(n, d, false))
        .collect();
    let groups: Vec<BotCommandSpec> = COMMANDS
        .iter()
        .filter(|(n, _, _)| GROUP.contains(n))
        .map(|(n, _, d)| spec(n, d, true))
        .collect();
    let mut out = vec![
        (CommandScope::Default, public),
        (CommandScope::AllGroupChats, groups),
    ];
    if let Some(chat) = owner {
        out.push((
            CommandScope::Chat(chat),
            COMMANDS.iter().map(|(n, _, d)| spec(n, d, false)).collect(),
        ));
    }
    out
}

/// What a command runs against.
pub struct CommandCx<'a> {
    /// Invoker.
    pub user: u64,
    /// `telegram.owner`.
    pub owner: Option<u64>,
    /// Chat the command was typed in.
    pub chat: i64,
    /// Topic, if any.
    pub thread: Option<i64>,
    /// A group chat (replies ephemeral where possible).
    pub in_group: bool,
    /// The thread's bound session, else the invoker's latest in the chat
    /// (the channel resolves both before calling).
    pub session: Option<SessionId>,
    /// Snapshot for `/status` and `/telegram status`.
    pub status: TelegramStatus,
    /// Agent asks.
    pub port: &'a dyn Port,
    /// Panel page (`cmd:page:…`).
    pub page: usize,
}

impl CommandCx<'_> {
    fn is_owner(&self) -> bool {
        self.owner == Some(self.user)
    }
}

/// A rendered reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// Rich Markdown.
    pub markdown: String,
    /// Inline keyboard, if any.
    pub keyboard: Option<Keyboard>,
}

impl Reply {
    /// Text only.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            markdown: s.into(),
            keyboard: None,
        }
    }

    /// Text in a fence.
    pub fn code(s: &str) -> Self {
        Self::text(format!("```\n{s}\n```"))
    }
}

/// What the channel does after a command.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Send the reply.
    Reply(Reply),
    /// Unbind this thread, then reply.
    NewThread(Reply),
    /// Bind this thread to `session`, then reply.
    Resume {
        /// Session.
        session: SessionId,
        /// Its title (for the topic name).
        title: String,
        /// Reply.
        reply: Reply,
    },
    /// The turn was cancelled; reply.
    Cancelled {
        /// Session.
        session: SessionId,
        /// Reply.
        reply: Reply,
    },
    /// `Close topic` on a finished child (98 §10.3): the channel closes the
    /// topic bound to `session`.
    CloseTopic(SessionId),
    /// Owner-only: the channel applies and replies.
    Allow(u64),
    /// Owner-only: the channel applies and replies.
    RemoveAllowed(u64),
    /// Owner-only: the channel applies and replies.
    AllowGroup(i64),
    /// Owner-only: the channel applies and replies.
    RemoveGroup(i64),
    /// The channel mints a code and replies with link + QR.
    Link,
    /// A `force_reply` prompt; the next reply to that message is `expect`.
    Prompt {
        /// The prompt.
        reply: Reply,
        /// What the answer is.
        expect: Expect,
    },
}

impl Outcome {
    /// The variant name, for logs.
    #[must_use]
    pub fn variant(&self) -> &'static str {
        match self {
            Self::Reply(_) => "Reply",
            Self::NewThread(_) => "NewThread",
            Self::Resume { .. } => "Resume",
            Self::Cancelled { .. } => "Cancelled",
            Self::CloseTopic(_) => "CloseTopic",
            Self::Allow(_) => "Allow",
            Self::RemoveAllowed(_) => "RemoveAllowed",
            Self::AllowGroup(_) => "AllowGroup",
            Self::RemoveGroup(_) => "RemoveGroup",
            Self::Link => "Link",
            Self::Prompt { .. } => "Prompt",
        }
    }
}

/// What a prompted reply is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// `/skills add …` URL.
    SkillsAdd,
    /// `/mcp add …` URL or JSON.
    McpAdd,
}

/// Runs `name` with `arg`. Owner-only names from a non-owner answer
/// [`OWNER_MSG`]; an unknown name answers [`help`]; ask failures become
/// `error: …` text.
pub async fn handle(cx: CommandCx<'_>, name: &str, arg: &str) -> Outcome {
    if owner_only(name) && !cx.is_owner() {
        return Outcome::Reply(Reply::text(OWNER_MSG));
    }
    match run(&cx, name, arg.trim()).await {
        Ok(o) => o,
        Err(e) if e.to_string() == NO_SESSION => Outcome::Reply(ui::no_session()),
        Err(e) => Outcome::Reply(Reply::text(format!("error: {e:#}"))),
    }
}

/// Finishes a [`Outcome::Prompt`] with the user's reply: the `add` runs.
pub async fn continue_prompt(cx: CommandCx<'_>, expect: Expect, text: &str) -> Outcome {
    let name = match expect {
        Expect::SkillsAdd => "skills",
        Expect::McpAdd => "mcp",
    };
    handle(cx, name, &format!("add {}", text.trim())).await
}

/// The help panel.
#[must_use]
pub fn help() -> Reply {
    ui::help()
}

/// The `Add…` prompt for `skills add` / `mcp add` without an argument.
#[must_use]
pub fn prompt(expect: Expect) -> Outcome {
    let what = match expect {
        Expect::SkillsAdd => "Reply with the URL",
        Expect::McpAdd => "Reply with the URL (or JSON for mcp)",
    };
    Outcome::Prompt {
        reply: Reply {
            markdown: what.to_owned(),
            keyboard: Some(Keyboard {
                rows: Vec::new(),
                force_reply: true,
            }),
        },
        expect,
    }
}

/// First word and the trimmed rest.
fn split(arg: &str) -> (&str, &str) {
    arg.split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()))
}

async fn run(cx: &CommandCx<'_>, name: &str, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let reply = |r: Reply| Ok(Outcome::Reply(r));
    match name {
        "start" | "help" => reply(help()),
        "status" => reply(status(cx).await?),
        "new" => Ok(Outcome::NewThread(Reply::text(
            "new thread: the next message starts a session",
        ))),
        "sessions" => reply(ui::sessions(&roots(cx).await?, cx.page)),
        "resume" => {
            if arg.is_empty() {
                return text("usage: /resume <id>".into());
            }
            match resolve(cx, arg).await? {
                Ok(s) => Ok(Outcome::Resume {
                    reply: Reply::text(format!("resumed {} · {}", short(s.id), s.title)),
                    session: s.id,
                    title: s.title,
                }),
                Err(msg) => text(msg),
            }
        }
        "cancel" => cancel(cx, arg).await,
        "compact" => compact_cmd(cx, arg).await,
        "clear" => compact_cmd(cx, "clear").await,
        "undo" => {
            let session = session(cx)?;
            let hidden = agent()?.ask(agent::Undo { session }).await.map_err(err)?;
            text(format!("undone: {hidden} (/redo restores)"))
        }
        "redo" => {
            let session = session(cx)?;
            let restored = agent()?.ask(agent::Redo { session }).await.map_err(err)?;
            text(format!("restored: {restored}"))
        }
        "model" if arg.is_empty() => {
            let models = agent()?.ask(agent::ListModels).await.map_err(err)?;
            let current = match cx.session {
                Some(s) => cx
                    .port
                    .list_sessions()
                    .await
                    .map_err(err)?
                    .into_iter()
                    .find(|r| r.id == s)
                    .map(|r| r.model)
                    .unwrap_or_default(),
                None => String::new(),
            };
            reply(ui::model(&models, &current, cx.page))
        }
        "model" => {
            // A button carries at most the id's first 54 bytes (98 §10.3
            // caps callback_data at 64); a unique prefix completes it.
            let models = agent()?.ask(agent::ListModels).await.map_err(err)?;
            let mut hits = models.iter().filter(|m| m.id.starts_with(arg));
            let model = match (hits.next(), hits.next()) {
                (Some(m), None) => m.id.clone(),
                _ => arg.to_owned(),
            };
            // 148 R1: with a session the switch is that session's only; the
            // session-less form has nothing but the default to set.
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
        "effort" if arg.is_empty() => {
            let session = session(cx)?;
            let s = cx
                .port
                .list_sessions()
                .await
                .map_err(err)?
                .into_iter()
                .find(|s| s.id == session)
                .context("session not found")?;
            let current = s.effort.as_deref().unwrap_or("unset");
            let info = agent()?
                .ask(agent::ListModels)
                .await
                .map_err(err)?
                .into_iter()
                .find(|m| m.id == s.model);
            let accepts = match info {
                Some(m) if !m.efforts.is_empty() || m.budget_min.is_some() => {
                    let mut a = m.efforts.join("|");
                    if let Some(min) = m.budget_min {
                        let max = m.budget_max.map_or(String::new(), |x| x.to_string());
                        a.push_str(&format!("; or budget {min}..{max}"));
                    }
                    a
                }
                Some(_) => "no reasoning effort levels".to_owned(),
                None => "any genai keyword or token budget".to_owned(),
            };
            text(format!("effort: {current}; accepts: {accepts}"))
        }
        "effort" => {
            let effort = (arg != "clear").then(|| arg.to_owned());
            agent()?
                .ask(agent::SetEffort {
                    session: cx.session,
                    effort: effort.clone(),
                    manual: cx.session.is_none(),
                })
                .await
                .map_err(err)?;
            text(effort.map_or_else(|| "effort cleared".to_owned(), |e| format!("effort: {e}")))
        }
        "agents" => {
            let session = match session_or(cx, arg).await? {
                Ok(s) => s,
                Err(msg) => return text(msg),
            };
            let rows = cx.port.list_agents(session).await.map_err(err)?;
            reply(ui::agents(&rows, &cx.status.threads))
        }
        "jobs" => {
            let session = session(cx)?;
            match split(arg) {
                ("", _) => {
                    let rows = jobs()?.ask(tools::ListJobs(session)).await.map_err(err)?;
                    if rows.is_empty() {
                        return text("no jobs".into());
                    }
                    reply(Reply::code(&tools::jobs::table(&rows)))
                }
                ("kill", id) if !id.is_empty() => {
                    jobs()?
                        .ask(tools::CancelJob {
                            session,
                            id: tools::JobId(id.to_owned()),
                            origin: tools::CancelOrigin::Ui,
                        })
                        .await
                        .map_err(err)?;
                    text(format!("killed {id}"))
                }
                _ => text("usage: /jobs [kill <id>]".into()),
            }
        }
        "prompt" => {
            let prompt = agent()?
                .ask(agent::ShowPrompt {
                    session: cx.session,
                })
                .await
                .map_err(err)?;
            reply(Reply::code(&prompt))
        }
        "skills" => skills(cx, arg).await,
        "mcp" => mcp_cmd(cx, arg).await,
        "login" if arg.is_empty() => logins().await,
        "login" => {
            let name = arg.to_owned();
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
            reply(ui::login_url(arg, &url))
        }
        "logout" if arg.is_empty() => text("usage: /logout <name>".into()),
        "logout" => {
            let name = arg.to_owned();
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
        "provider" => provider_cmd(arg).await,
        "approvals" => approvals_cmd(cx.session, arg).await,
        "telegram" => telegram_cmd(cx, arg),
        "communication" => communication_cmd(cx.session, arg).await,
        "peers" => peers_cmd(cx.session, arg).await,
        "requests" => requests_cmd(cx.session, arg).await,
        "a2a" => a2a_cmd(arg).await,
        "broadcast" => broadcast_cmd(cx, arg).await,
        _ => reply(help()),
    }
}

/// `cx.session`, else [`NO_SESSION`].
fn session(cx: &CommandCx<'_>) -> anyhow::Result<SessionId> {
    cx.session.context(NO_SESSION)
}

/// `cx.session` when `id` is empty, else `id` resolved like `resume`.
/// `Err` is reply text.
async fn session_or(cx: &CommandCx<'_>, id: &str) -> anyhow::Result<Result<SessionId, String>> {
    if id.is_empty() {
        return Ok(Ok(session(cx)?));
    }
    Ok(resolve(cx, id).await?.map(|s| s.id))
}

/// Top-level chat sessions, newest first (desks and children dropped).
async fn roots(cx: &CommandCx<'_>) -> anyhow::Result<Vec<agent::SessionSummary>> {
    Ok(cx
        .port
        .list_sessions()
        .await
        .map_err(err)?
        .into_iter()
        .filter(|s| s.parent.is_none() && s.desk.is_none())
        .collect())
}

/// The one session whose id equals, starts or ends with `id`; `Err` is reply text.
pub(crate) async fn resolve(
    cx: &CommandCx<'_>,
    id: &str,
) -> anyhow::Result<Result<agent::SessionSummary, String>> {
    let rows = cx.port.list_sessions().await.map_err(err)?;
    Ok(session_by_id(rows, id))
}

fn session_by_id(
    rows: Vec<agent::SessionSummary>,
    id: &str,
) -> Result<agent::SessionSummary, String> {
    let mut hits = rows.into_iter().filter(|s| {
        let full = s.id.to_string();
        full == id || full.starts_with(id) || full.ends_with(id)
    });
    match (hits.next(), hits.next()) {
        (Some(s), None) => Ok(s),
        (Some(_), Some(_)) => Err(format!("{id:?} matches several sessions")),
        (None, _) => Err(format!("no session {id:?}")),
    }
}

/// `cancel [name | session-prefix]`: bare = the thread's turn; a session
/// prefix (what the panels send) = that session; else a subagent of the
/// thread's session by name or `parent/name`.
async fn cancel(cx: &CommandCx<'_>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    if arg.is_empty() {
        let session = session(cx)?;
        cx.port.cancel(session).await.map_err(err)?;
        return Ok(Outcome::Cancelled {
            session,
            reply: Reply::text("cancelled"),
        });
    }
    match resolve(cx, arg).await? {
        Ok(s) => {
            cx.port.cancel(s.id).await.map_err(err)?;
            return Ok(Outcome::Cancelled {
                session: s.id,
                reply: Reply::text(format!("cancelled {}", short(s.id))),
            });
        }
        Err(msg) if msg.contains("several") => return text(msg),
        Err(_) => {}
    }
    let session = session(cx)?;
    let rows = cx.port.list_agents(session).await.map_err(err)?;
    let hits: Vec<&agent::AgentInfo> = rows
        .iter()
        .filter(|a| a.name == arg || path(a, &rows) == arg)
        .collect();
    match hits.as_slice() {
        // 141 §10.3: a watch-only child is the harness's; only an RFD child
        // that advertised `capabilities.cancel` can be stopped on its own.
        [a] if a.observed.as_ref().is_some_and(|o| !o.can_cancel) => {
            let parent = rows
                .iter()
                .find(|p| p.id == a.parent)
                .map_or_else(|| short(a.parent), |p| p.name.clone());
            text(format!(
                "{arg} is a watch-only subagent of {parent}; stop the parent"
            ))
        }
        [a] => {
            cx.port.cancel(a.id).await.map_err(err)?;
            Ok(Outcome::Cancelled {
                session: a.id,
                reply: Reply::text(format!("cancelled {arg}")),
            })
        }
        [] => text(format!("no subagent named {arg}")),
        many => text(format!(
            "{arg} is ambiguous: {}",
            many.iter()
                .map(|a| path(a, &rows))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `/status`: the channel snapshot plus the bound session's model, turn and context.
async fn status(cx: &CommandCx<'_>) -> anyhow::Result<Reply> {
    let Some(session) = cx.session else {
        return Ok(ui::status(&cx.status, None));
    };
    // ponytail: LoadSession renders the whole history just for `running`;
    // have the channel pass its TurnStarted/TurnFinished flag if this shows.
    let loaded = cx.port.load_session(session).await.map_err(err)?;
    let turn = if loaded.running { "running" } else { "idle" };
    let context = loaded.context_estimate.map_or_else(|| "-".to_owned(), k);
    Ok(ui::status(
        &cx.status,
        Some([short(session), loaded.model, turn.to_owned(), context]),
    ))
}

/// `/telegram [status | link | allow <id> | rm <id> | group add <id> | group rm <id>]`.
fn telegram_cmd(cx: &CommandCx<'_>, arg: &str) -> anyhow::Result<Outcome> {
    const USAGE: &str =
        "usage: /telegram [status | link | allow <id> | rm <id> | group add <id> | group rm <id>]";
    let text = |s: &str| Ok(Outcome::Reply(Reply::text(s)));
    let (sub, rest) = split(arg);
    let (sub2, id) = split(rest);
    Ok(match (sub, sub2, id) {
        ("" | "status", _, _) => Outcome::Reply(Reply::code(&render_status(&cx.status))),
        ("link", _, _) => Outcome::Link,
        ("allow", n, "") => match n.parse() {
            Ok(id) => Outcome::Allow(id),
            Err(_) => return text("usage: /telegram allow <user id>"),
        },
        ("rm", n, "") => match n.parse() {
            Ok(id) => Outcome::RemoveAllowed(id),
            Err(_) => return text("usage: /telegram rm <user id>"),
        },
        ("group", "add", n) => match n.parse() {
            Ok(id) => Outcome::AllowGroup(id),
            Err(_) => return text("usage: /telegram group add <chat id>"),
        },
        ("group", "rm", n) => match n.parse() {
            Ok(id) => Outcome::RemoveGroup(id),
            Err(_) => return text("usage: /telegram group rm <chat id>"),
        },
        _ => return text(USAGE),
    })
}

/// `/skills [add <url> | rm <name> | sync]`.
async fn skills(cx: &CommandCx<'_>, arg: &str) -> anyhow::Result<Outcome> {
    let (sub, rest) = split(arg);
    if (sub, rest) == ("add", "") {
        return Ok(prompt(Expect::SkillsAdd));
    }
    let agent = agent()?;
    let out = match (sub, rest) {
        ("", _) => {
            let rows = agent.ask(agent::ListSkills).await.map_err(err)?;
            return Ok(Outcome::Reply(ui::skills(&rows, cx.page)));
        }
        ("sync", _) => synced(agent.ask(agent::SyncSkills).await.map_err(err)?),
        ("add", url) => {
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
        _ => "usage: /skills [add <url> | rm <name> | sync]".to_owned(),
    };
    Ok(Outcome::Reply(Reply::text(out)))
}

/// `/mcp [add <url|json> | sync]`.
async fn mcp_cmd(cx: &CommandCx<'_>, arg: &str) -> anyhow::Result<Outcome> {
    let (sub, rest) = split(arg);
    if (sub, rest) == ("add", "") {
        return Ok(prompt(Expect::McpAdd));
    }
    let mcp = mcp()?;
    let out = match (sub, rest) {
        ("", _) => {
            let rows = mcp.ask(tools::ListServers::default()).await.map_err(err)?;
            return Ok(Outcome::Reply(ui::mcp(&rows, cx.page)));
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
        ("add", src) => {
            let (name, entry) = mcp_entry(src).map_err(|e| anyhow::anyhow!("/mcp add: {e}"))?;
            let server = mcp
                .ask(tools::AddServer { name, entry })
                .await
                .map_err(err)?;
            format!("added {} ({} tools)", server.name, server.tools.len())
        }
        _ => "usage: /mcp [add <url|json> | sync]".to_owned(),
    };
    Ok(Outcome::Reply(Reply::text(out)))
}

/// `/compact [summary|cut|snap|style …|<session-prefix>]`.
async fn compact_cmd(cx: &CommandCx<'_>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let (sub, rest) = split(arg);
    let fold = async |session: SessionId, style: Option<agent::CompactStyle>| {
        let epoch = agent()?
            .ask(agent::Compact {
                session,
                style,
                head: None,
            })
            .await
            .map_err(err)?;
        anyhow::Ok(format!("compacted → epoch {epoch}"))
    };
    match (sub, rest) {
        ("", _) => text(fold(session(cx)?, None).await?),
        (token, _) if token.parse::<agent::CompactStyle>().is_ok() => {
            let style = token.parse().expect("guard");
            text(fold(session(cx)?, Some(style)).await?)
        }
        ("style", "") => {
            let current = agent()?.ask(agent::GetCompactStyle).await.map_err(err)?;
            text(format!(
                "compact style: {current} (default)\n  summary  cache-hot LLM fold (07 §8.3)\n  cut      drop the folded span, keep the tail, no model call\n  snap     rasterize the folded span to images (vision model)"
            ))
        }
        ("style", rest) if rest.parse::<agent::CompactStyle>().is_ok() => {
            let style = rest.parse().expect("guard");
            agent()?
                .ask(agent::SetCompactStyle { style })
                .await
                .map_err(err)?;
            text(format!("compact style: {style}"))
        }
        (id, "") => match resolve(cx, id).await? {
            Ok(s) => text(fold(s.id, None).await?),
            Err(msg) => text(msg),
        },
        _ => text("usage: /compact [summary|cut|snap|style …|<session>]".into()),
    }
}

async fn provider_cmd(arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let (sub, rest) = split(arg);
    match (sub, rest) {
        ("", _) => {
            let rows = agent()?.ask(agent::ListProviders).await.map_err(err)?;
            Ok(Outcome::Reply(Reply::text(ui::table(
                std::iter::once(vec![
                    "name".to_owned(),
                    "kind".to_owned(),
                    "url".to_owned(),
                    "models".to_owned(),
                ])
                .chain(rows.into_iter().map(|p| {
                    let kind = if p.builtin { "built-in" } else { "custom" };
                    vec![p.name, kind.to_owned(), p.base_url, p.models.to_string()]
                })),
            ))))
        }
        ("add", rest) => {
            let mut parts = rest.splitn(3, char::is_whitespace);
            let name = parts.next().unwrap_or_default();
            let url = parts.next().unwrap_or("").trim();
            let key = parts.next().unwrap_or("").trim();
            // 177 E10: a local URL needs no key (the engine refuses a cloud one without).
            if name.is_empty() || url.is_empty() {
                return text("usage: /provider add <name> <url> [key]".into());
            }
            let p = agent()?
                .ask(agent::AddProvider {
                    name: name.to_owned(),
                    base_url: url.to_owned(),
                    key: (!key.is_empty()).then(|| key.to_owned()),
                    runtime: None,
                })
                .await
                .map_err(err)?;
            text(format!(
                "provider {}  {}  {} models — /model {}/<id>",
                p.name, p.base_url, p.models, p.name
            ))
        }
        ("refresh", name) if !name.is_empty() => {
            let r = agent()?
                .ask(agent::RefreshProviderModels {
                    name: name.to_owned(),
                })
                .await
                .map_err(err)?;
            text(r.line())
        }
        ("rm", name) if !name.is_empty() => {
            agent()?
                .ask(agent::RemoveProvider {
                    name: name.to_owned(),
                })
                .await
                .map_err(err)?;
            text(format!("removed provider {name}"))
        }
        _ => text("usage: /provider [add [name [url [key]]] | refresh <name> | rm <name>]".into()),
    }
}

/// `/login`: providers, then MCP servers that use OAuth, as a panel.
async fn logins() -> anyhow::Result<Outcome> {
    let providers = agent()?.ask(agent::ListLogins).await.map_err(err)?;
    let servers = mcp()?
        .ask(tools::ListServers::default())
        .await
        .map_err(err)?;
    Ok(Outcome::Reply(ui::logins(&providers, &servers)))
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

/// `/approvals [on|off [always]]`: bare = the three-button panel; `always`
/// switches the persisted default.
async fn approvals_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let mut words = arg.split_whitespace();
    let enabled = match words.next() {
        None => return Ok(Outcome::Reply(ui::approvals())),
        Some("on") => true,
        Some("off") => false,
        _ => return text("usage: /approvals on|off [always]".into()),
    };
    let session = match words.next() {
        Some("always") => None,
        None => Some(
            session
                .context("no session yet: send a message first, or use `/approvals off always`")?,
        ),
        Some(w) => return text(format!("/approvals: expected `always`, got {w:?}")),
    };
    let state = ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)?
        .context("tool registry is not running")?
        .ask(tools::SetApprovals {
            session,
            enabled,
            clear: false,
        })
        .await
        .map_err(err)?;
    text(format!(
        "approvals: {} (this session), {} (default)",
        on_off(state.session),
        on_off(state.global)
    ))
}

/// `/communication [on|off [always]]`: read, or switch this session's opt-out
/// (`always`: the persisted global ceiling). The reply is the runtime's
/// effective state, never a guess (50 §2.1).
async fn communication_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let mut words = arg.split_whitespace();
    let state = match words.next() {
        None => agent()?
            .ask(agent::GetCommunication { session })
            .await
            .map_err(err)?,
        Some(w @ ("on" | "off")) => {
            let target = match words.next() {
                Some("always") => None,
                None => Some(session.context(
                    "no session yet: send a message first, or use `/communication on|off always`",
                )?),
                Some(w) => return text(format!("/communication: expected `always`, got {w:?}")),
            };
            agent()?
                .ask(agent::SetCommunication {
                    session: target,
                    enabled: w == "on",
                })
                .await
                .map_err(err)?
        }
        Some(_) => return text("usage: /communication [on|off [always]]".into()),
    };
    text(communication_text(&state))
}

/// `/broadcast [--next] running|roots|<id>[,<id>…] <text>` (62 §5.3): one
/// message to the selected sessions; the reply is the receipt, one line per
/// row.
async fn broadcast_cmd(cx: &CommandCx<'_>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let (delivery, selector, body) = match parse_broadcast(arg) {
        Ok(p) => p,
        Err(usage) => return text(usage),
    };
    let targets = match selector {
        Selector::Running => BroadcastTargets::Running,
        Selector::Roots => BroadcastTargets::Roots,
        Selector::Ids(ids) => {
            let rows = cx.port.list_sessions().await.map_err(err)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match session_by_id(rows.clone(), id) {
                    Ok(s) => out.push(s.id),
                    Err(msg) => return text(msg),
                }
            }
            BroadcastTargets::Sessions { ids: out }
        }
    };
    // kameo flattens a `Result` reply: the supervisor's refusal is `HandlerError`.
    let receipt = agent()?
        .ask(agent::Broadcast(BroadcastRequest {
            targets,
            text: body,
            files: vec![],
            delivery,
            dry_run: false,
        }))
        .await;
    text(match receipt {
        Ok(r) => receipt_text(&r),
        Err(kameo::error::SendError::HandlerError(e)) => format!("broadcast refused: {e}"),
        Err(e) => return Err(err(e)),
    })
}

/// The `/broadcast` selector word before id resolution.
#[derive(Debug, PartialEq, Eq)]
enum Selector {
    Running,
    Roots,
    /// Raw ids: full UUIDs or the prefixes `/resume` accepts.
    Ids(Vec<String>),
}

/// `[--next] running|roots|<id>[,<id>…] <text>` → delivery, selector, text.
/// `Err` is the usage line.
fn parse_broadcast(arg: &str) -> Result<(Delivery, Selector, String), String> {
    const USAGE: &str = "usage: /broadcast [--next] running|roots|<id>[,<id>…] <text>";
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

/// [`agent::CommunicationState`] as the phone reads it: the effective answer
/// first, then the exact blocker when it is off.
fn communication_text(s: &agent::CommunicationState) -> String {
    let mut out = format!("communication: {}", on_off(s.effective));
    if !s.effective {
        let why = if !s.global {
            format!("global default is off ({})", s.source)
        } else if s.opted_out {
            "this session opted out".to_owned()
        } else {
            s.inherited_from.map_or_else(
                || "blocked".to_owned(),
                |id| format!("inherited from {}", short(id)),
            )
        };
        out.push_str(&format!(" ({why})"));
    }
    out.push_str(&format!("\nglobal: {} ({})", on_off(s.global), s.source));
    if s.session.is_some() {
        out.push_str(&format!("\nscope: {}", s.scope.as_str()));
        if let Some(alias) = &s.alias {
            out.push_str(&format!(" · alias {alias}"));
        }
    }
    out.push_str(&format!(
        "\nnetwork: {}{}",
        if s.network_enabled {
            "enabled"
        } else {
            "disabled"
        },
        if s.listening { ", listening" } else { "" }
    ));
    out.push_str("\nsession opt-outs last until the daemon restarts; `always` persists");
    out
}

/// `/peers [<query> | scope owner|isolated [alias]]`: the bounded permitted
/// directory the bound session may address (50 §5.1), or the root's scope.
async fn peers_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let session = session.context(NO_SESSION)?;
    let (sub, rest) = split(arg);
    if sub == "scope" {
        let (name, alias) = split(rest);
        let Ok(scope) = name.parse::<PeerScope>() else {
            return text("usage: /peers scope owner|isolated [alias]".into());
        };
        let state = agent()?
            .ask(agent::SetPeerScope {
                root: session,
                scope,
                alias: (!alias.is_empty()).then(|| alias.to_owned()),
            })
            .await
            .map_err(err)?;
        return text(communication_text(&state));
    }
    let page = agent()?
        .ask(agent::ListPeers {
            session,
            query: (!arg.is_empty()).then(|| arg.to_owned()),
            cursor: None,
            limit: None,
            active_only: false,
        })
        .await
        .map_err(err)?;
    if page.peers.is_empty() {
        return text("(none)".into());
    }
    let header = ["name", "address", "kind", "relationship", "state", "caps"]
        .map(str::to_owned)
        .to_vec();
    let mut out = ui::table(std::iter::once(header).chain(page.peers.iter().map(|p| {
        let caps: Vec<&str> = [(p.send, "send"), (p.reply, "reply"), (p.stream, "stream")]
            .into_iter()
            .filter_map(|(on, w)| on.then_some(w))
            .collect();
        vec![
            p.name.clone(),
            p.address.clone(),
            word(&p.kind),
            word(&p.relationship),
            word(&p.state),
            caps.join(","),
        ]
    })));
    if page.cursor.is_some() {
        out.push_str("\n\nmore; /peers <query> narrows");
    }
    text(out)
}

/// `/requests [<id> | answer <id> <text>]`: the exact pending request ids the
/// owner may resolve for the bound session (either side, 50 §5.3), one
/// record, or an answer. `<id>` is a full or unique-prefix pending id, never
/// an arbitrary waiter.
async fn requests_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    let session = session.context(NO_SESSION)?;
    let pending = agent()?
        .ask(agent::ListPeerMessages {
            session,
            cursor: None,
            limit: None,
            pending_only: true,
        })
        .await
        .map_err(err)?;
    let (sub, rest) = split(arg);
    match (sub, rest) {
        ("", _) => {
            if pending.messages.is_empty() {
                return text("(none)".into());
            }
            let side = |id: &uuid::Uuid| {
                if pending.awaiting_answer.contains(id) {
                    "awaiting our answer"
                } else if pending.awaiting_reply.contains(id) {
                    "awaiting their reply"
                } else {
                    ""
                }
            };
            let header = ["id", "from", "state", "side", "text"]
                .map(str::to_owned)
                .to_vec();
            text(ui::table(std::iter::once(header).chain(
                pending.messages.iter().map(|m| {
                    vec![
                        m.receipt.id.to_string(),
                        m.from.clone(),
                        m.receipt.state.as_str().to_owned(),
                        side(&m.receipt.id).to_owned(),
                        m.text.chars().take(40).collect(),
                    ]
                }),
            )))
        }
        ("answer", rest) => {
            let Some((id, body)) = rest.split_once(char::is_whitespace) else {
                return text("usage: /requests answer <id> <text>".into());
            };
            let id = pending_id(&pending, id)?;
            let receipt = agent()?
                .ask(agent::AnswerPeerRequest {
                    session,
                    reply_to: id,
                    text: body.trim().to_owned(),
                })
                .await
                .map_err(err)?;
            text(format!(
                "answered {id}: {}{}",
                receipt.state.as_str(),
                receipt.error.map_or(String::new(), |e| format!(" ({e})"))
            ))
        }
        (id, _) => {
            let id = pending_id(&pending, id)?;
            let m = agent()?
                .ask(agent::GetPeerMessage { session, id })
                .await
                .map_err(err)?;
            text(format!(
                "{id}\nfrom: {}\nto: {}\nstate: {}{}\nreply_to: {}\nat: {}\n\n{}",
                m.from,
                m.receipt.to,
                m.receipt.state.as_str(),
                m.receipt.error.map_or(String::new(), |e| format!(" ({e})")),
                m.receipt.reply_to.map_or("-".to_owned(), |r| r.to_string()),
                age(m.created_at),
                m.text
            ))
        }
    }
}

/// The one pending id `s` names exactly or as a unique prefix.
fn pending_id(page: &agent::PeerMessagePage, s: &str) -> anyhow::Result<uuid::Uuid> {
    let mut hits = page
        .awaiting_answer
        .iter()
        .chain(&page.awaiting_reply)
        .filter(|id| id.to_string().starts_with(s));
    match (hits.next(), hits.next()) {
        (Some(id), None) => Ok(*id),
        (Some(_), Some(_)) => anyhow::bail!("{s:?} matches several pending requests"),
        (None, _) => anyhow::bail!("no pending request {s:?}"),
    }
}

/// `/a2a`: the network state as the agent policy reports it.
// NEEDS(integrator): `a2a` is not a dependency of this crate (Cargo.toml is
// not a COMMANDS seam), so the actor's full status (peers, exports, public
// URL) and `reload` are unreachable; Signal's `a2a_cmd` ports verbatim once
// `a2a = { workspace = true }` is added.
async fn a2a_cmd(arg: &str) -> anyhow::Result<Outcome> {
    let text = |s: String| Ok(Outcome::Reply(Reply::text(s)));
    if !arg.is_empty() {
        return text("a2a reload is not available from Telegram yet; use the terminal".into());
    }
    let comm = agent()?
        .ask(agent::GetCommunication { session: None })
        .await
        .map_err(err)?;
    text(format!(
        "a2a: {}{}",
        if comm.network_enabled {
            "enabled"
        } else {
            "disabled"
        },
        if comm.listening { ", listening" } else { "" }
    ))
}

/// A unit-variant enum's serde word (`Native` → `native`).
fn word<T: std::fmt::Debug>(v: &T) -> String {
    format!("{v:?}").to_lowercase()
}

fn agent() -> anyhow::Result<ActorRef<Agents>> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)?.context("agent actor is not running")
}

fn mcp() -> anyhow::Result<ActorRef<tools::Mcp>> {
    ActorRef::<tools::Mcp>::lookup(tools::MCP_ACTOR)?.context("mcp actor is not running")
}

fn jobs() -> anyhow::Result<ActorRef<tools::Jobs>> {
    ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR)?.context("jobs actor is not running")
}

fn err<E: Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// `parent/name` from the asked session down: a depth-1 child is its bare
/// name, a grandchild `parent/name`. `/cancel` accepts either form.
pub(crate) fn path(a: &agent::AgentInfo, rows: &[agent::AgentInfo]) -> String {
    match rows.iter().find(|p| p.id == a.parent) {
        Some(p) => format!("{}/{}", path(p, rows), a.name),
        None => a.name.clone(),
    }
}

fn synced(rows: Vec<tools::git::SyncResult>) -> String {
    if rows.is_empty() {
        return "(none)".to_owned();
    }
    ui::table(
        std::iter::once(vec!["name".to_owned(), "status".to_owned()])
            .chain(rows.into_iter().map(|r| vec![r.name, r.status])),
    )
}

/// `/mcp add` argument that means "clone a repo": a `.git` URL or a known git host.
fn git_url(s: &str) -> bool {
    s.ends_with(".git")
        || ["github.com/", "gitlab.com/", "codeberg.org/"]
            .iter()
            .any(|h| s.contains(h))
}

/// One MCP server from `/mcp add <json>`: a server object with a `name` key
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

/// Test doubles shared with `ui`.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use agent::{AgentInfo, Loaded, SessionSummary, communication::PeerScope};
    use async_trait::async_trait;
    use chrono::Utc;
    use parking_lot::Mutex;
    use runtime::SessionId;
    use tools::{Answer, ApprovalId, AskId, Decision};

    use super::*;

    /// A [`Port`] over fixed session and agent lists; records cancels.
    #[derive(Default)]
    pub struct FakePort {
        pub sessions: Vec<SessionSummary>,
        pub agents: Vec<AgentInfo>,
        pub cancelled: Mutex<Vec<SessionId>>,
    }

    #[async_trait]
    impl Port for FakePort {
        async fn send_input(
            &self,
            _: Option<SessionId>,
            _: String,
            _: Vec<PathBuf>,
            _: PeerScope,
            _: agent::ChannelOrigin,
        ) -> Result<SessionId, String> {
            Err("fake".into())
        }
        async fn new_root(&self, _: PeerScope) -> Result<SessionId, String> {
            Err("fake".into())
        }
        async fn steer(
            &self,
            _: SessionId,
            _: String,
            _: Vec<PathBuf>,
            _: agent::ChannelOrigin,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn cancel(&self, session: SessionId) -> Result<(), String> {
            self.cancelled.lock().push(session);
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
            Ok(self.sessions.clone())
        }
        async fn load_session(&self, _: SessionId) -> Result<Loaded, String> {
            Err("fake: no history".into())
        }
        async fn resolve(&self, _: ApprovalId, _: Decision) -> Result<bool, String> {
            Ok(true)
        }
        async fn answer_ask(&self, _: AskId, _: Option<Vec<Answer>>) -> Result<bool, String> {
            Ok(true)
        }
        async fn list_agents(&self, _: SessionId) -> Result<Vec<AgentInfo>, String> {
            Ok(self.agents.clone())
        }
        async fn ancestors(&self, s: SessionId) -> Result<Vec<SessionId>, String> {
            Ok(vec![s])
        }
        async fn session_title(&self, s: SessionId) -> Result<Option<String>, String> {
            Ok(self
                .sessions
                .iter()
                .find(|r| r.id == s)
                .map(|r| r.title.clone()))
        }
    }

    /// A top-level session row.
    pub fn summary(id: SessionId, title: &str) -> SessionSummary {
        SessionSummary {
            id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            title: title.to_owned(),
            tags: vec![],
            model: "openai/gpt-5".into(),
            parent: None,
            name: None,
            effort: None,
            agent: None,
            cwd: None,
            harness_model: None,
            context_size: None,
            desk: None,
            pinned: false,
            archived: false,
            project: None,
            notebook: None,
            retention: tools::Retention::Durable,
            running: false,
            owner: String::new(),
            epoch: 1,
            here: true,
            kind: Default::default(),
            memory: Default::default(),
            observed: None,
        }
    }

    pub fn status() -> TelegramStatus {
        TelegramStatus {
            state: None,
            bot: None,
            polling: false,
            owner: Some(1),
            allowed: vec![],
            groups: vec![],
            threads: vec![],
            open: 0,
            calls: BTreeMap::new(),
            dropped_updates: 0,
        }
    }

    pub fn cx<'a>(port: &'a FakePort, user: u64, session: Option<SessionId>) -> CommandCx<'a> {
        CommandCx {
            user,
            owner: Some(1),
            chat: 7,
            thread: None,
            in_group: false,
            session,
            status: status(),
            port,
            page: 0,
        }
    }

    pub fn text(s: &str) -> Outcome {
        Outcome::Reply(Reply::text(s))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{FakePort, cx, summary, text};
    use super::*;

    #[test]
    fn scopes_split_public_owner_and_group() {
        let s = scopes(Some(7));
        assert_eq!(s.len(), 3);
        let (scope, public) = &s[0];
        assert_eq!(*scope, CommandScope::Default);
        assert!(public.iter().all(|c| !owner_only(&c.name) && !c.ephemeral));
        assert!(public.iter().any(|c| c.name == "start"));
        let (scope, groups) = &s[1];
        assert_eq!(*scope, CommandScope::AllGroupChats);
        assert_eq!(groups.len(), GROUP.len());
        assert!(groups.iter().all(|c| c.ephemeral));
        let (scope, all) = &s[2];
        assert_eq!(*scope, CommandScope::Chat(7));
        assert_eq!(all.len(), COMMANDS.len());
        assert!(all.iter().any(|c| c.name == "telegram"));
        assert!(all.iter().all(|c| !c.ephemeral));
        assert_eq!(scopes(None).len(), 2);
        // Every scope is derived from the one table.
        for (_, cmds) in &s {
            for c in cmds {
                let (_, _, d) = COMMANDS.iter().find(|(n, _, _)| *n == c.name).unwrap();
                assert_eq!(c.description, *d);
            }
        }
    }

    #[test]
    fn table_is_well_formed_for_the_api() {
        for (n, _, d) in COMMANDS {
            assert!((1..=32).contains(&n.len()), "{n}");
            assert!(
                n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{n}"
            );
            assert!((3..=256).contains(&d.len()), "{n}");
        }
        assert!(
            !COMMANDS
                .iter()
                .any(|(n, _, _)| *n == "approve" || *n == "deny" || *n == "signal")
        );
        assert!(owner_only("telegram"));
        assert!(!owner_only("help"));
        for n in OWNER_ONLY {
            assert!(COMMANDS.iter().any(|(c, _, _)| c == n), "{n}");
        }
    }

    #[tokio::test]
    async fn owner_only_commands_are_gated() {
        let port = FakePort::default();
        for (name, arg) in [
            ("approvals", "off"),
            ("telegram", "link"),
            ("communication", "on always"),
            ("peers", "scope owner"),
            ("requests", ""),
            ("a2a", "reload"),
            ("broadcast", "running hello"),
        ] {
            assert_eq!(
                handle(cx(&port, 2, None), name, arg).await,
                text(OWNER_MSG),
                "{name}"
            );
        }
        // Not owner-only: `new` passes without an actor.
        assert!(matches!(
            handle(cx(&port, 2, None), "new", "").await,
            Outcome::NewThread(_)
        ));
        // The owner passes; `telegram` needs no actor.
        assert_eq!(
            handle(cx(&port, 1, None), "telegram", "rm 55").await,
            Outcome::RemoveAllowed(55)
        );
        assert_eq!(
            handle(cx(&port, 1, None), "telegram", "group add -100123").await,
            Outcome::AllowGroup(-100_123)
        );
        assert_eq!(
            handle(cx(&port, 1, None), "telegram", "allow x").await,
            text("usage: /telegram allow <user id>")
        );
        assert_eq!(
            handle(cx(&port, 1, None), "telegram", "link").await,
            Outcome::Link
        );
        let Outcome::Reply(r) = handle(cx(&port, 1, None), "telegram", "").await else {
            panic!()
        };
        assert!(r.markdown.starts_with("```\nstate: "));
    }

    #[tokio::test]
    async fn resume_resolves_full_prefix_and_suffix_and_refuses_ambiguity() {
        // v7 ids minted together share their first 8 chars; pick two apart.
        let a = SessionId(uuid::Uuid::from_u128(
            0x0123_4567_89ab_cdef_0123_4567_89ab_cdef,
        ));
        let b = SessionId(uuid::Uuid::from_u128(
            0xfedc_ba98_7654_3210_fedc_ba98_7654_3210,
        ));
        let port = FakePort {
            sessions: vec![summary(a, "alpha"), summary(b, "beta")],
            ..Default::default()
        };
        let full = a.to_string();
        for id in [full.as_str(), &full[..8], &full[full.len() - 8..]] {
            let Outcome::Resume {
                session,
                title,
                reply,
            } = handle(cx(&port, 2, None), "resume", id).await
            else {
                panic!("{id}")
            };
            assert_eq!(session, a);
            assert_eq!(title, "alpha");
            assert_eq!(reply.markdown, format!("resumed {} · alpha", short(a)));
        }
        assert_eq!(
            handle(cx(&port, 2, None), "resume", "").await,
            text("usage: /resume <id>")
        );
        assert_eq!(
            handle(cx(&port, 2, None), "resume", "zz").await,
            text("no session \"zz\"")
        );
        let port2 = FakePort {
            sessions: vec![summary(a, "a"), summary(a, "a again")],
            ..Default::default()
        };
        assert_eq!(
            handle(cx(&port2, 2, None), "resume", &full[..8]).await,
            text(&format!("{:?} matches several sessions", &full[..8]))
        );
    }

    #[tokio::test]
    async fn session_scoped_without_session_says_so() {
        let port = FakePort::default();
        for (name, arg) in [("cancel", ""), ("undo", ""), ("agents", ""), ("jobs", "")] {
            assert_eq!(
                handle(cx(&port, 2, None), name, arg).await,
                Outcome::Reply(ui::no_session()),
                "{name}"
            );
        }
        let Outcome::Reply(r) = handle(cx(&port, 2, None), "help", "").await else {
            panic!()
        };
        assert_eq!(r, help());
        // Unknown names answer help too.
        let Outcome::Reply(r) = handle(cx(&port, 2, None), "zzz", "").await else {
            panic!()
        };
        assert_eq!(r, help());
    }

    #[tokio::test]
    async fn cancel_prefers_a_session_prefix_then_a_subagent_name() {
        let s = SessionId::new();
        let child = SessionId::new();
        let mut info = agent::AgentInfo {
            id: child,
            name: "grep-logs".into(),
            parent: s,
            depth: 1,
            agent: None,
            model: "m".into(),
            state: agent::AgentState::Running,
            started_at: None,
            finished_at: None,
            last: String::new(),
            project: None,
            retention: tools::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
            observed: None,
        };
        info.activity.state = agent::AgentState::Running;
        let port = FakePort {
            sessions: vec![summary(s, "root")],
            agents: vec![info],
            ..Default::default()
        };
        assert_eq!(
            handle(cx(&port, 2, Some(s)), "cancel", "").await,
            Outcome::Cancelled {
                session: s,
                reply: Reply::text("cancelled")
            }
        );
        assert_eq!(
            handle(cx(&port, 2, None), "cancel", &short(s)).await,
            Outcome::Cancelled {
                session: s,
                reply: Reply::text(format!("cancelled {}", short(s)))
            }
        );
        assert_eq!(
            handle(cx(&port, 2, Some(s)), "cancel", "grep-logs").await,
            Outcome::Cancelled {
                session: child,
                reply: Reply::text("cancelled grep-logs")
            }
        );
        assert_eq!(
            handle(cx(&port, 2, Some(s)), "cancel", "nope").await,
            text("no subagent named nope")
        );
        assert_eq!(port.cancelled.lock().as_slice(), &[s, s, child]);
    }

    /// 141 §10.3: `cancel <name>` on a watch-only child refuses with the
    /// parent to stop instead, unless the harness accepts a per-child cancel.
    #[tokio::test]
    async fn cancel_refuses_a_watch_only_subagent_unless_cancellable() {
        let s = SessionId::new();
        let child = SessionId::new();
        let observed = |can_cancel| agent::ObservedMeta {
            source: agent::ObservedSource::Rfd,
            external: "sess_child_1".into(),
            state: agent::ObservedState::Running,
            stop: None,
            can_cancel,
            description: None,
            tool_call: None,
            cost_rule: agent::CostRule::DisplayOnly,
            detail: serde_json::Value::Null,
        };
        let info = |can_cancel| agent::AgentInfo {
            id: child,
            name: "ReadA".into(),
            parent: s,
            depth: 1,
            agent: None,
            model: "m".into(),
            state: agent::AgentState::Running,
            started_at: None,
            finished_at: None,
            last: String::new(),
            project: None,
            retention: tools::Retention::Durable,
            activity: Default::default(),
            cap: None,
            kind: agent::SessionKind::Observed,
            observed: Some(observed(can_cancel)),
        };
        let port = FakePort {
            sessions: vec![summary(s, "root")],
            agents: vec![info(false)],
            ..Default::default()
        };
        assert_eq!(
            handle(cx(&port, 2, Some(s)), "cancel", "ReadA").await,
            text(&format!(
                "ReadA is a watch-only subagent of {}; stop the parent",
                short(s)
            ))
        );
        assert!(port.cancelled.lock().is_empty());
        let port = FakePort {
            sessions: vec![summary(s, "root")],
            agents: vec![info(true)],
            ..Default::default()
        };
        assert_eq!(
            handle(cx(&port, 2, Some(s)), "cancel", "ReadA").await,
            Outcome::Cancelled {
                session: child,
                reply: Reply::text("cancelled ReadA")
            }
        );
        assert_eq!(port.cancelled.lock().as_slice(), &[child]);
    }

    #[tokio::test]
    async fn model_with_an_id_asks_set_model() {
        // No actor runs here: the ask goes out and fails, which proves the
        // arm reached SetModel rather than the listing panel.
        let port = FakePort::default();
        assert_eq!(
            handle(cx(&port, 2, None), "model", "gpt").await,
            text("error: agent actor is not running")
        );
        assert_eq!(
            handle(cx(&port, 2, None), "model", "").await,
            text("error: agent actor is not running")
        );
    }

    #[tokio::test]
    async fn add_without_argument_prompts_and_the_reply_runs_the_add() {
        let port = FakePort::default();
        let Outcome::Prompt { reply, expect } = handle(cx(&port, 2, None), "skills", "add").await
        else {
            panic!()
        };
        assert_eq!(expect, Expect::SkillsAdd);
        assert!(reply.keyboard.as_ref().unwrap().force_reply);
        assert!(reply.keyboard.as_ref().unwrap().rows.is_empty());
        assert_eq!(
            handle(cx(&port, 2, None), "mcp", "add").await,
            prompt(Expect::McpAdd)
        );
        // The continuation reaches the add (and fails: no actor).
        assert_eq!(
            continue_prompt(cx(&port, 2, None), Expect::SkillsAdd, " https://x/y.git ").await,
            text("error: agent actor is not running")
        );
        // The mcp continuation needs the mcp actor first.
        assert_eq!(
            continue_prompt(cx(&port, 2, None), Expect::McpAdd, "{\"a\": 1}").await,
            text("error: mcp actor is not running")
        );
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
                parse_broadcast(bad).is_err_and(|u| u.starts_with("usage: /broadcast")),
                "{bad}"
            );
        }
    }

    #[test]
    fn mcp_entry_accepts_the_three_shapes() {
        let (n, e) = mcp_entry(r#"{"name":"x","command":"c"}"#).unwrap();
        assert_eq!(n, "x");
        assert_eq!(e["command"], "c");
        assert!(e.get("name").is_none());
        let (n, _) = mcp_entry(r#"{"y":{"url":"u"}}"#).unwrap();
        assert_eq!(n, "y");
        let (n, _) = mcp_entry(r#"{"mcpServers":{"z":{"command":"c"}}}"#).unwrap();
        assert_eq!(n, "z");
        assert!(mcp_entry("[]").is_err());
        assert!(mcp_entry(r#"{"command":"c"}"#).is_err());
    }

    /// 162 D8: `refresh` without a name is a usage error, never an engine call.
    #[tokio::test]
    async fn provider_refresh_needs_a_name() {
        let usage = Outcome::Reply(Reply::text(
            "usage: /provider [add [name [url [key]]] | refresh <name> | rm <name>]",
        ));
        for arg in ["refresh", "zzz", "rm"] {
            assert_eq!(provider_cmd(arg).await.unwrap(), usage, "{arg}");
        }
    }
}
