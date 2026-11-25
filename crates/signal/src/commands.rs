//! Slash commands for the phone (design doc 09 §9): the terminal's names and
//! arguments, parsed from a message and answered with plain text. Every arm
//! is one ask plus a formatter, like `tui::execute`.

use agent::{BroadcastRequest, BroadcastTargets, PeerScope};
use anyhow::Context as _;
use chat_common::{
    Selector, agent, communication_text, err, git_url, is_provider, jobs, mcp, mcp_entry, on_off,
    parse_broadcast, pending_id, receipt_text, short, synced, table, word,
};
use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use runtime::SessionId;
use tools::{Decision, McpAuth};

use crate::approvals::{self, Pending};
use crate::{SignalEvent, SignalStatus};

/// What the channel must do after a command; [`Reply::Text`] is just a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Send this text back.
    Text(String),
    /// `/new`: unbind the thread; the next message starts a session.
    NewThread,
    /// `/resume <id>`: bind the thread to this (validated) session.
    Resume(SessionId),
    /// `/signal allow <n>`: add to the allowlist (owner only).
    Allow(String),
    /// `/signal rm <n>`: remove from the allowlist (owner only).
    RemoveAllowed(String),
    /// `/signal unlink`: drop the registration (owner only).
    Unlink,
    /// `/signal link`: the channel replies with the provisioning URL.
    Link,
}

/// What a command runs against.
pub struct Cx<'a> {
    /// The thread's bound session.
    pub session: Option<SessionId>,
    /// Sender is `signal.owner`.
    pub is_owner: bool,
    /// Snapshot for `/status` and `/signal status`.
    pub status: SignalStatus,
    /// Approval codes for `/approve`, `/deny` and bare codes.
    pub approvals: &'a mut Pending,
}

/// Name, argument hint, one-line description (09 §9 table).
const COMMANDS: &[(&str, &str, &str)] = &[
    ("/help", "", "this list"),
    ("/status", "", "link state, session, model, turn"),
    ("/new", "", "start a new thread"),
    ("/sessions", "", "10 newest sessions"),
    (
        "/resume",
        "<id>",
        "bind this thread to a session (prefix ok)",
    ),
    (
        "/cancel",
        "[name]",
        "abort the turn, or a subagent's by name",
    ),
    ("/compact", "[summary|cut|snap|style …]", "fold the context"),
    (
        "/clear",
        "",
        "drop the conversation, keep the thread's settings",
    ),
    ("/undo", "", "drop the last turn (redo restores)"),
    ("/redo", "", "restore the last undone turn"),
    ("/model", "[id]", "show or set the model"),
    (
        "/effort",
        "[level|clear]",
        "show or set the reasoning effort",
    ),
    (
        "/agents",
        "",
        "this session's subagents: name · state · last · age",
    ),
    ("/jobs", "[kill <id>]", "list background jobs"),
    ("/prompt", "", "the assembled system prompt"),
    ("/skills", "[add <url>|rm <name>|sync]", "skills"),
    ("/mcp", "[add <url|json>|sync]", "MCP servers"),
    ("/login", "[name]", "list logins or start one"),
    ("/logout", "<name>", "log out"),
    (
        "/provider",
        "[add [name [url [key]]] | refresh <name> | rm <name>]",
        "built-in and custom providers",
    ),
    (
        "/approvals",
        "on|off [always]",
        "tool approval prompts (owner)",
    ),
    ("/approve", "<code>", "allow a pending tool call (owner)"),
    ("/deny", "<code>", "deny a pending tool call (owner)"),
    (
        "/signal",
        "[status|link|allow <n>|rm <n>|unlink]",
        "this channel (owner)",
    ),
    (
        "/communication",
        "[on|off [always]]",
        "peer messaging for this session or the default (owner)",
    ),
    (
        "/peers",
        "[<query> | scope owner|isolated [alias]]",
        "this session's peer directory or root scope (owner)",
    ),
    (
        "/requests",
        "[<id> | answer <id> <text>]",
        "pending peer requests: inspect or answer one (owner)",
    ),
    (
        "/broadcast",
        "[--next] running|roots|<id>[,<id>…] <text>",
        "one message to many sessions; --next lands after the turn (owner)",
    ),
    (
        "/a2a",
        "[reload]",
        "network status or config reload (owner)",
    ),
    (
        "/share",
        "[list|stop [<id>|all]]",
        "live public shares; stop one or all (owner)",
    ),
];

const OWNER_ONLY: &[&str] = &[
    "/approvals",
    "/approve",
    "/deny",
    "/signal",
    "/communication",
    "/peers",
    "/requests",
    "/a2a",
    "/broadcast",
    "/share",
];
const OWNER_MSG: &str = "This command is restricted to the owner.";
const NO_SESSION: &str = "no session yet: send a message first";

/// The help text: one line per command inside a fence.
pub fn help() -> String {
    table(
        COMMANDS
            .iter()
            .map(|(n, a, d)| vec![(*n).to_owned(), (*a).to_owned(), (*d).to_owned()]),
    )
}

/// `None` when `text` is not a slash command and not a bare pending approval
/// code. Owner-only commands from a non-owner answer [`OWNER_MSG`];
/// unknown or ambiguous commands answer [`help`]. Ask failures become
/// `error: …` text.
pub async fn handle(mut cx: Cx<'_>, text: &str) -> Option<Reply> {
    let (name, arg) = classify(cx.approvals, text)?;
    if !cx.is_owner && OWNER_ONLY.contains(&name) {
        return Some(Reply::Text(OWNER_MSG.to_owned()));
    }
    Some(match run(&mut cx, name, arg).await {
        Ok(reply) => reply,
        Err(e) => Reply::Text(format!("error: {e:#}")),
    })
}

/// Command name and argument for `text`: a bare pending code is
/// `/approve <code>`, an unknown or ambiguous slash word is `/help`, plain
/// text is `None`.
fn classify<'t>(pending: &Pending, text: &'t str) -> Option<(&'static str, &'t str)> {
    let text = text.trim();
    let Some(rest) = text.strip_prefix('/') else {
        return pending.is_code(text).then_some(("/approve", text));
    };
    let (word, arg) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(w, a)| (w, a.trim()));
    let word = format!("/{}", word.to_lowercase());
    Some((complete(&word).unwrap_or("/help"), arg))
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

async fn run(cx: &mut Cx<'_>, name: &'static str, arg: &str) -> anyhow::Result<Reply> {
    let text = |s: String| Ok(Reply::Text(s));
    match name {
        "/help" => text(help()),
        "/status" => text(status(cx).await?),
        "/new" => Ok(Reply::NewThread),
        "/sessions" => {
            let rows = agent()?.ask(agent::ListSessions).await.map_err(err)?;
            text(table(
                rows.iter()
                    .filter(|s| s.parent.is_none())
                    .take(10)
                    .map(|s| vec![short(s.id), s.title.clone(), age(s.updated_at)]),
            ))
        }
        "/resume" => {
            if arg.is_empty() {
                return text("usage: /resume <id>".into());
            }
            let rows = agent()?.ask(agent::ListSessions).await.map_err(err)?;
            match session_by_id(&rows, arg) {
                Ok(s) => Ok(Reply::Resume(s.id)),
                Err(msg) => text(msg),
            }
        }
        "/cancel" if arg.is_empty() => {
            let session = cx.session.context(NO_SESSION)?;
            agent()?
                .ask(agent::Cancel {
                    session,
                    keep: None,
                    scope: agent::CancelScope::Root,
                    reason: agent::CancelReason::Stop,
                })
                .await
                .map_err(err)?;
            text("cancelled".into())
        }
        "/cancel" => {
            let session = cx.session.context(NO_SESSION)?;
            let rows = agent()?
                .ask(agent::ListAgents { session })
                .await
                .map_err(err)?;
            let hits: Vec<&agent::AgentInfo> = rows
                .iter()
                .filter(|a| a.name == arg || path(a, &rows) == arg)
                .collect();
            match hits.as_slice() {
                [a] => {
                    agent()?
                        .ask(agent::Cancel {
                            session: a.id,
                            keep: None,
                            scope: agent::CancelScope::Root,
                            reason: agent::CancelReason::Stop,
                        })
                        .await
                        .map_err(err)?;
                    text(format!("cancelled {arg}"))
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
        "/compact" => compact_cmd(cx, arg).await,
        "/clear" => compact_cmd(cx, "clear").await,
        "/undo" => {
            let session = cx.session.context(NO_SESSION)?;
            let hidden = agent()?.ask(agent::Undo { session }).await.map_err(err)?;
            text(format!("undone: {hidden} (/redo restores)"))
        }
        "/redo" => {
            let session = cx.session.context(NO_SESSION)?;
            let restored = agent()?.ask(agent::Redo { session }).await.map_err(err)?;
            text(format!("restored: {restored}"))
        }
        "/model" if arg.is_empty() => {
            let models = agent()?.ask(agent::ListModels).await.map_err(err)?;
            let mut out = table(models.iter().take(20).map(|m| {
                vec![
                    m.id.clone(),
                    format!("{}{}", k(m.context), if m.image { " · vision" } else { "" }),
                ]
            }));
            if models.len() > 20 {
                out.push_str(&format!("\n{} more; /model <id> to set", models.len() - 20));
            }
            text(out)
        }
        "/model" => {
            // 148 R1: with a session the switch is that session's only; the
            // session-less form has nothing but the default to set.
            let info = agent()?
                .ask(agent::SetModel {
                    session: cx.session,
                    model: arg.to_owned(),
                    manual: cx.session.is_none(),
                })
                .await
                .map_err(err)?;
            text(format!("model: {}", info.id))
        }
        "/effort" if arg.is_empty() => {
            let session = cx.session.context(NO_SESSION)?;
            let agent = agent()?;
            let s = agent
                .ask(agent::ListSessions)
                .await
                .map_err(err)?
                .into_iter()
                .find(|s| s.id == session)
                .context("session not found")?;
            let current = s.effort.as_deref().unwrap_or("unset");
            let info = agent
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
        "/effort" => {
            let effort = (arg != "clear").then(|| arg.to_owned());
            // 148 R1: with a session the change is that session's only; the
            // session-less form has nothing but the default to set.
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
        "/agents" => {
            let session = cx.session.context(NO_SESSION)?;
            let rows = agent()?
                .ask(agent::ListAgents { session })
                .await
                .map_err(err)?;
            if rows.is_empty() {
                return Ok(Reply::Text("no subagents".to_owned()));
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
        "/jobs" => {
            let session = cx.session.context(NO_SESSION)?;
            match arg.split_once(' ') {
                None if arg.is_empty() => {
                    let rows = jobs()?.ask(tools::ListJobs(session)).await.map_err(err)?;
                    text(if rows.is_empty() {
                        "no jobs".to_owned()
                    } else {
                        format!("```\n{}\n```", tools::jobs::table(&rows))
                    })
                }
                Some(("kill", id)) if !id.trim().is_empty() => {
                    let id = id.trim();
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
        "/prompt" => text(
            agent()?
                .ask(agent::ShowPrompt {
                    session: cx.session,
                })
                .await
                .map_err(err)?,
        ),
        "/skills" => skills(arg).await,
        "/mcp" => mcp_cmd(arg).await,
        "/login" if arg.is_empty() => logins().await,
        "/login" => {
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
            text(format!("login: open {url}"))
        }
        "/logout" if arg.is_empty() => text("usage: /logout <name>".into()),
        "/logout" => {
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
        "/provider" => provider_cmd(arg).await,
        "/approvals" => approvals_cmd(cx.session, arg).await,
        "/approve" | "/deny" => {
            if arg.is_empty() {
                return text(format!("usage: {name} <code>"));
            }
            let decision = if name == "/approve" {
                Decision::Allow
            } else {
                Decision::Deny
            };
            text(approvals::resolve_by_code(cx.approvals, arg, decision).await)
        }
        "/signal" => {
            let (sub, rest) = arg
                .split_once(char::is_whitespace)
                .map_or((arg, ""), |(s, r)| (s, r.trim()));
            match (sub, rest) {
                ("" | "status", _) => text(signal_status(&cx.status)),
                ("link", _) => Ok(Reply::Link),
                ("unlink", _) => Ok(Reply::Unlink),
                ("allow", n) if !n.is_empty() => Ok(Reply::Allow(n.to_owned())),
                ("rm", n) if !n.is_empty() => Ok(Reply::RemoveAllowed(n.to_owned())),
                _ => text("usage: /signal [status | link | allow <n> | rm <n> | unlink]".into()),
            }
        }
        "/communication" => communication_cmd(cx.session, arg).await,
        "/peers" => peers_cmd(cx.session, arg).await,
        "/requests" => requests_cmd(cx.session, arg).await,
        "/a2a" => a2a_cmd(arg).await,
        "/broadcast" => broadcast_cmd(arg).await,
        // 210 §7.4 item 9: never gated by an approval.
        "/share" => text(crate::sharing::command(arg).await?),
        _ => text(help()),
    }
}

/// `/status`: the channel snapshot plus the bound session's model and turn.
async fn status(cx: &Cx<'_>) -> anyhow::Result<String> {
    let mut out = signal_status(&cx.status);
    let Some(session) = cx.session else {
        out.push_str("\nsession: none");
        return Ok(out);
    };
    let agent = agent()?;
    let (model, effort) = agent
        .ask(agent::ListSessions)
        .await
        .map_err(err)?
        .into_iter()
        .find(|s| s.id == session)
        .map_or_else(|| ("?".to_owned(), None), |s| (s.model, s.effort));
    let effort = effort.map_or(String::new(), |e| format!(" · effort {e}"));
    // ponytail: LoadSession renders the whole history just for `running`;
    // have the channel pass its TurnStarted/TurnFinished flag if this shows.
    let loaded = agent
        .ask(agent::LoadSession {
            session,
            before: None,
            limit: None,
            settled_only: false,
        })
        .await
        .map_err(err)?;
    let turn = if loaded.running { "running" } else { "idle" };
    out.push_str(&format!(
        "\nsession: {}\nmodel: {model}{effort}\nturn: {turn}",
        short(session)
    ));
    Ok(out)
}

/// `/signal status`: link state, number, trust, thread count.
fn signal_status(s: &SignalStatus) -> String {
    let link = match &s.state {
        None if s.registered => "registered".to_owned(),
        None => "not linked".to_owned(),
        Some(SignalEvent::LinkUrl { .. }) => "linking; /signal link for the URL".to_owned(),
        Some(SignalEvent::Linked { number }) => format!("linked as {number}"),
        Some(SignalEvent::Connected) => "connected".to_owned(),
        Some(SignalEvent::Disconnected { retry_in_secs }) => {
            format!("disconnected; retry in {retry_in_secs}s")
        }
        Some(SignalEvent::Unlinked) => "unlinked".to_owned(),
        Some(SignalEvent::DaemonOnly) => "daemon only; run t --headless".to_owned(),
    };
    let allowed = if s.allowed.is_empty() {
        "(none)".to_owned()
    } else {
        s.allowed.join(", ")
    };
    format!(
        "link: {link}\nnumber: {}\nowner: {}\nallowed: {allowed}\nthreads: {}",
        s.number.as_deref().unwrap_or("-"),
        s.owner,
        s.threads.len()
    )
}

/// `/skills [add <url> | rm <name> | sync]`.
async fn skills(arg: &str) -> anyhow::Result<Reply> {
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    let agent = agent()?;
    let out = match (sub, rest) {
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
        _ => "usage: /skills [add <url> | rm <name> | sync]".to_owned(),
    };
    Ok(Reply::Text(out))
}

/// `/mcp [add <url|json> | sync]`.
async fn mcp_cmd(arg: &str) -> anyhow::Result<Reply> {
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    let mcp = mcp()?;
    let out = match (sub, rest) {
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
            let (name, entry) = mcp_entry(src).map_err(|e| anyhow::anyhow!("/mcp add: {e}"))?;
            let server = mcp
                .ask(tools::AddServer { name, entry })
                .await
                .map_err(err)?;
            format!("added {} ({} tools)", server.name, server.tools.len())
        }
        _ => "usage: /mcp [add <url|json> | sync]".to_owned(),
    };
    Ok(Reply::Text(out))
}

/// `/compact [summary|cut|snap|style …]`.
async fn compact_cmd(cx: &Cx<'_>, arg: &str) -> anyhow::Result<Reply> {
    let text = |s: String| Ok(Reply::Text(s));
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    match (sub, rest) {
        ("", _) => {
            let session = cx.session.context(NO_SESSION)?;
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
        (token, _) if token.parse::<agent::CompactStyle>().is_ok() => {
            let style = token.parse().expect("guard");
            let session = cx.session.context(NO_SESSION)?;
            let epoch = agent()?
                .ask(agent::Compact {
                    session,
                    style: Some(style),
                    head: None,
                })
                .await
                .map_err(err)?;
            text(format!("compacted → epoch {epoch}"))
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
        _ => text("usage: /compact [summary|cut|snap|style …]".into()),
    }
}

async fn provider_cmd(arg: &str) -> anyhow::Result<Reply> {
    let text = |s: String| Ok(Reply::Text(s));
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    match (sub, rest) {
        ("", _) => {
            let rows = agent()?.ask(agent::ListProviders).await.map_err(err)?;
            text(table(rows.into_iter().map(|p| {
                let kind = if p.builtin { "built-in" } else { "custom" };
                vec![
                    p.name,
                    kind.to_owned(),
                    p.base_url,
                    format!("{} models", p.models),
                ]
            })))
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

/// `/login`: providers, then MCP servers that use OAuth.
async fn logins() -> anyhow::Result<Reply> {
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
    Ok(Reply::Text(table(rows)))
}

/// `/approvals on|off [always]`: `always` switches the persisted default.
async fn approvals_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Reply> {
    let mut words = arg.split_whitespace();
    let enabled = match words.next() {
        Some("on") => true,
        Some("off") => false,
        _ => return Ok(Reply::Text("usage: /approvals on|off [always]".into())),
    };
    let session = match words.next() {
        Some("always") => None,
        None => Some(
            session
                .context("no session yet: send a message first, or use `/approvals off always`")?,
        ),
        Some(w) => {
            return Ok(Reply::Text(format!(
                "/approvals: expected `always`, got {w:?}"
            )));
        }
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
    Ok(Reply::Text(format!(
        "approvals: {} (this session), {} (default)",
        on_off(state.session),
        on_off(state.global)
    )))
}

/// `/communication [on|off [always]]`: read, or switch this session's opt-out
/// (`always`: the persisted global ceiling). The reply is the runtime's
/// effective state, never a guess (50 §2.1).
async fn communication_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Reply> {
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
                Some(w) => {
                    return Ok(Reply::Text(format!(
                        "/communication: expected `always`, got {w:?}"
                    )));
                }
            };
            agent()?
                .ask(agent::SetCommunication {
                    session: target,
                    enabled: w == "on",
                })
                .await
                .map_err(err)?
        }
        Some(_) => {
            return Ok(Reply::Text(
                "usage: /communication [on|off [always]]".into(),
            ));
        }
    };
    Ok(Reply::Text(communication_text(&state)))
}

/// `/broadcast [--next] running|roots|<id>[,<id>…] <text>` (62 §5.3): one
/// message to the selected sessions; the reply is the receipt, one line per
/// row.
async fn broadcast_cmd(arg: &str) -> anyhow::Result<Reply> {
    let (delivery, selector, text) = match parse_broadcast(arg) {
        Ok(p) => p,
        Err(usage) => return Ok(Reply::Text(usage)),
    };
    let targets = match selector {
        Selector::Running => BroadcastTargets::Running,
        Selector::Roots => BroadcastTargets::Roots,
        Selector::Ids(ids) => {
            let rows = agent()?.ask(agent::ListSessions).await.map_err(err)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match session_by_id(&rows, id) {
                    Ok(s) => out.push(s.id),
                    Err(msg) => return Ok(Reply::Text(msg)),
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
    Ok(Reply::Text(match receipt {
        Ok(r) => receipt_text(&r),
        Err(kameo::error::SendError::HandlerError(e)) => format!("broadcast refused: {e}"),
        Err(e) => return Err(err(e)),
    }))
}

/// `/peers [<query> | scope owner|isolated [alias]]`: the bounded permitted
/// directory the bound session may address (50 §5.1), or the root's scope.
async fn peers_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Reply> {
    let session = session.context(NO_SESSION)?;
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    if sub == "scope" {
        let (name, alias) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(w, a)| (w, a.trim()));
        let Ok(scope) = name.parse::<PeerScope>() else {
            return Ok(Reply::Text(
                "usage: /peers scope owner|isolated [alias]".into(),
            ));
        };
        let state = agent()?
            .ask(agent::SetPeerScope {
                root: session,
                scope,
                alias: (!alias.is_empty()).then(|| alias.to_owned()),
            })
            .await
            .map_err(err)?;
        return Ok(Reply::Text(communication_text(&state)));
    }
    let page = agent()?
        .ask(agent::ListPeers {
            session,
            query: (!arg.is_empty()).then(|| arg.to_owned()),
            cursor: None,
            limit: None,
            active_only: false,
            kind: None,
        })
        .await
        .map_err(err)?;
    let mut out = table(page.peers.iter().map(|p| {
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
    }));
    if page.cursor.is_some() {
        out.push_str("\nmore; /peers <query> narrows");
    }
    Ok(Reply::Text(out))
}

/// `/requests [<id> | answer <id> <text>]`: the exact pending request ids the
/// owner may resolve for the bound session (either side, 50 §5.3), one
/// record, or an answer. `<id>` is a full or unique-prefix pending id, never
/// an arbitrary waiter.
async fn requests_cmd(session: Option<SessionId>, arg: &str) -> anyhow::Result<Reply> {
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
    let (sub, rest) = arg
        .split_once(char::is_whitespace)
        .map_or((arg, ""), |(s, r)| (s, r.trim()));
    match (sub, rest) {
        ("", _) => {
            let side = |id: &uuid::Uuid| {
                if pending.awaiting_answer.contains(id) {
                    "awaiting our answer"
                } else if pending.awaiting_reply.contains(id) {
                    "awaiting their reply"
                } else {
                    ""
                }
            };
            Ok(Reply::Text(table(pending.messages.iter().map(|m| {
                vec![
                    m.receipt.id.to_string(),
                    m.from.clone(),
                    m.receipt.state.as_str().to_owned(),
                    side(&m.receipt.id).to_owned(),
                    m.text.chars().take(40).collect(),
                ]
            }))))
        }
        ("answer", rest) => {
            let Some((id, text)) = rest.split_once(char::is_whitespace) else {
                return Ok(Reply::Text("usage: /requests answer <id> <text>".into()));
            };
            let id = pending_id(&pending, id)?;
            let receipt = agent()?
                .ask(agent::AnswerPeerRequest {
                    session,
                    reply_to: id,
                    text: text.trim().to_owned(),
                })
                .await
                .map_err(err)?;
            Ok(Reply::Text(format!(
                "answered {id}: {}{}",
                receipt.state.as_str(),
                receipt.error.map_or(String::new(), |e| format!(" ({e})"))
            )))
        }
        (id, _) => {
            let id = pending_id(&pending, id)?;
            let m = agent()?
                .ask(agent::GetPeerMessage { session, id })
                .await
                .map_err(err)?;
            Ok(Reply::Text(format!(
                "{id}\nfrom: {}\nto: {}\nstate: {}{}\nreply_to: {}\nat: {}\n\n{}",
                m.from,
                m.receipt.to,
                m.receipt.state.as_str(),
                m.receipt.error.map_or(String::new(), |e| format!(" ({e})")),
                m.receipt.reply_to.map_or("-".to_owned(), |r| r.to_string()),
                age(m.created_at),
                m.text
            )))
        }
    }
}

/// `/a2a [reload]`: the network actor's status, or an atomic config reload.
/// No actor and `a2a.enabled: false` is "disabled"; no actor while enabled is
/// an error, never a silent off.
async fn a2a_cmd(arg: &str) -> anyhow::Result<Reply> {
    let actor = ActorRef::<a2a::A2aActor>::lookup(a2a::A2A_ACTOR)?;
    let status = match (actor, arg) {
        (Some(a), "") => a.ask(a2a::GetStatus).await.map_err(err)?,
        (Some(a), "reload") => a.ask(a2a::Reload).await.map_err(err)?,
        (None, "" | "reload") => {
            let comm = agent()?
                .ask(agent::GetCommunication { session: None })
                .await
                .map_err(err)?;
            anyhow::ensure!(
                !comm.network_enabled,
                "a2a is enabled but its actor is not running"
            );
            a2a::A2aStatus::disabled()
        }
        _ => return Ok(Reply::Text("usage: /a2a [reload]".into())),
    };
    Ok(Reply::Text(a2a_text(&status)))
}

fn a2a_text(s: &a2a::A2aStatus) -> String {
    let mut out = format!(
        "a2a: {}{}",
        if s.enabled { "enabled" } else { "disabled" },
        s.address
            .as_deref()
            .filter(|_| s.listening)
            .map_or(String::new(), |a| format!(", listening on {a}"))
    );
    if let Some(url) = &s.public_url {
        out.push_str(&format!("\npublic: {url}"));
    }
    if let Some(e) = &s.error {
        out.push_str(&format!("\nerror: {e}"));
    }
    if !s.peers.is_empty() {
        out.push_str("\npeers:\n");
        out.push_str(&table(s.peers.iter().map(|p| {
            vec![
                p.name.clone(),
                match (p.outbound, p.inbound) {
                    (true, true) => "in/out",
                    (true, false) => "out",
                    (false, true) => "in",
                    (false, false) => "-",
                }
                .to_owned(),
                p.card.clone().unwrap_or_default(),
                p.error.clone().unwrap_or_default(),
            ]
        })));
    }
    if !s.exports.is_empty() {
        out.push_str("\nexports:\n");
        out.push_str(&table(s.exports.iter().map(|e| {
            vec![
                e.name.clone(),
                short(e.session),
                e.peer.clone().unwrap_or_default(),
                e.url.clone(),
            ]
        })));
    }
    out
}

/// The one session whose id equals, starts or ends with `id`; `Err` is reply text.
fn session_by_id<'a>(
    rows: &'a [agent::SessionSummary],
    id: &str,
) -> Result<&'a agent::SessionSummary, String> {
    let mut hits = rows.iter().filter(|s| {
        let full = s.id.to_string();
        full == id || full.starts_with(id) || full.ends_with(id)
    });
    match (hits.next(), hits.next()) {
        (Some(s), None) => Ok(s),
        (Some(_), Some(_)) => Err(format!("{id:?} matches several sessions")),
        (None, _) => Err(format!("no session {id:?}")),
    }
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
    platform::fmt::age(u64::try_from((Utc::now() - t).num_seconds()).unwrap_or(0))
}

/// `parent/name` from the asked session down: a depth-1 child is its bare
/// name, a grandchild `parent/name`. `/cancel` accepts either form.
fn path(a: &agent::AgentInfo, rows: &[agent::AgentInfo]) -> String {
    match rows.iter().find(|p| p.id == a.parent) {
        Some(p) => format!("{}/{}", path(p, rows), a.name),
        None => a.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> SignalStatus {
        SignalStatus {
            state: Some(SignalEvent::Connected),
            registered: true,
            number: Some("+15550001111".into()),
            owner: "+15550002222".into(),
            allowed: vec![],
            threads: vec![],
        }
    }

    #[test]
    fn completes_unique_prefixes_only() {
        let p = Pending::default();
        assert_eq!(classify(&p, "/mo gpt"), Some(("/model", "gpt")));
        assert_eq!(classify(&p, "/eff high"), Some(("/effort", "high")));
        assert_eq!(classify(&p, "  /MODEL  "), Some(("/model", "")));
        assert_eq!(classify(&p, "/s"), Some(("/help", "")));
        assert_eq!(classify(&p, "/re"), Some(("/help", "")));
        assert_eq!(classify(&p, "/u"), Some(("/undo", "")));
        assert_eq!(classify(&p, "/skills sync"), Some(("/skills", "sync")));
        assert_eq!(classify(&p, "/j kill j3"), Some(("/jobs", "kill j3")));
        assert_eq!(classify(&p, "/zzz"), Some(("/help", "")));
        assert_eq!(classify(&p, "hello /model"), None);
        assert_eq!(classify(&p, "k7mq"), None);
    }

    #[test]
    fn bare_pending_code_is_approve() {
        let mut p = Pending::default();
        let req = tools::ApprovalRequested {
            id: tools::ApprovalId(uuid::Uuid::new_v4()),
            session: SessionId(uuid::Uuid::new_v4()),
            tool: "shell".into(),
            args: serde_json::json!({ "command": "ls" }),
            risk: tools::Risk::Mutating,
            human_only: false,
            always_ask: false,
        };
        let code = p.insert(&req, "+15550002222");
        assert_eq!(
            classify(&p, &format!(" {code} ")),
            Some(("/approve", code.as_str()))
        );
        assert_eq!(classify(&p, "zzzz"), None);
    }

    #[test]
    fn help_lists_every_command_once() {
        let h = help();
        for (name, _, _) in COMMANDS {
            assert_eq!(h.matches(&format!("{name} ")).count(), 1, "{name}");
        }
        assert!(h.starts_with("```\n") && h.ends_with("\n```"));
    }

    #[tokio::test]
    async fn owner_only_commands_are_gated() {
        let mut p = Pending::default();
        for text in [
            "/approve k7mq",
            "/deny k7mq",
            "/approvals off",
            "/signal unlink",
            "/communication on always",
            "/peers scope owner",
            "/requests",
            "/a2a reload",
            "/broadcast running hello",
            "/share stop",
            "/share",
            "/sha list",
        ] {
            let cx = Cx {
                session: None,
                is_owner: false,
                status: status(),
                approvals: &mut p,
            };
            assert_eq!(
                handle(cx, text).await,
                Some(Reply::Text(OWNER_MSG.into())),
                "{text}"
            );
        }
        let cx = Cx {
            session: None,
            is_owner: false,
            status: status(),
            approvals: &mut p,
        };
        assert_eq!(handle(cx, "/new").await, Some(Reply::NewThread));
        let cx = Cx {
            session: None,
            is_owner: true,
            status: status(),
            approvals: &mut p,
        };
        assert_eq!(
            handle(cx, "/signal rm +1 555").await,
            Some(Reply::RemoveAllowed("+1 555".into()))
        );
    }

    /// 210 §7.4 item 9: `/share` is in the table, owner-only, and reaches the share engine
    /// for the owner without an approval; no running engine says so.
    #[tokio::test]
    async fn share_is_owner_only_and_reaches_the_engine() {
        let mut p = Pending::default();
        assert!(OWNER_ONLY.contains(&"/share"));
        assert_eq!(
            classify(&p, "/share stop all"),
            Some(("/share", "stop all"))
        );
        assert_eq!(classify(&p, "/sh"), Some(("/share", "")));
        for text in ["/share stop", "/share stop all", "/share", "/share list"] {
            let cx = Cx {
                session: None,
                is_owner: true,
                status: status(),
                approvals: &mut p,
            };
            assert_eq!(
                handle(cx, text).await,
                Some(Reply::Text(crate::sharing::NOT_RUNNING.into())),
                "{text}"
            );
        }
    }

    /// 162 D8: `refresh` without a name is a usage error, never an engine call.
    #[tokio::test]
    async fn provider_refresh_needs_a_name() {
        let usage = Reply::Text(
            "usage: /provider [add [name [url [key]]] | refresh <name> | rm <name>]".into(),
        );
        for arg in ["refresh", "zzz", "rm"] {
            assert_eq!(provider_cmd(arg).await.unwrap(), usage, "{arg}");
        }
    }
}
