//! Slash commands for the phone (design doc 09 §9): the terminal's names and
//! arguments, parsed from a message and answered with plain text. Every arm
//! is one ask plus a formatter, like `tui::execute`.

use std::fmt::Display;

use agent::{AGENT_ACTOR, Agents};
use anyhow::Context as _;
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
    ("/cancel", "", "abort the turn"),
    ("/compact", "", "fold the context"),
    ("/model", "[id]", "show or set the model"),
    ("/agents", "", "this session's subagents"),
    ("/prompt", "", "the assembled system prompt"),
    ("/skills", "[add <url>|rm <name>|sync]", "skills"),
    ("/mcp", "[add <url|json>|sync]", "MCP servers"),
    ("/login", "[name]", "list logins or start one"),
    ("/logout", "<name>", "log out"),
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
];

const OWNER_ONLY: &[&str] = &["/approvals", "/approve", "/deny", "/signal"];
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
                    .map(|s| vec![short(s.id), s.title.clone(), age(s.created_at)]),
            ))
        }
        "/resume" => {
            if arg.is_empty() {
                return text("usage: /resume <id>".into());
            }
            let rows = agent()?.ask(agent::ListSessions).await.map_err(err)?;
            let mut hits = rows.iter().filter(|s| {
                let id = s.id.to_string();
                id == arg || id.starts_with(arg) || id.ends_with(arg)
            });
            match (hits.next(), hits.next()) {
                (Some(s), None) => Ok(Reply::Resume(s.id)),
                (Some(_), Some(_)) => text(format!("{arg:?} matches several sessions")),
                (None, _) => text(format!("no session {arg:?}")),
            }
        }
        "/cancel" => {
            let session = cx.session.context(NO_SESSION)?;
            agent()?.ask(agent::Cancel { session }).await.map_err(err)?;
            text("cancelled".into())
        }
        "/compact" => {
            let session = cx.session.context(NO_SESSION)?;
            let epoch = agent()?
                .ask(agent::Compact { session })
                .await
                .map_err(err)?;
            text(format!("compacted → epoch {epoch}"))
        }
        "/model" if arg.is_empty() => {
            let models = agent()?.ask(agent::ListModels).await.map_err(err)?;
            let mut out = table(
                models
                    .iter()
                    .take(20)
                    .map(|m| vec![m.id.clone(), k(m.context)]),
            );
            if models.len() > 20 {
                out.push_str(&format!("\n{} more; /model <id> to set", models.len() - 20));
            }
            text(out)
        }
        "/model" => {
            let info = agent()?
                .ask(agent::SetModel {
                    session: cx.session,
                    model: arg.to_owned(),
                })
                .await
                .map_err(err)?;
            text(format!("model: {}", info.id))
        }
        "/agents" => {
            let parent = cx.session.context(NO_SESSION)?;
            let rows = agent()?
                .ask(agent::ListChildren { parent })
                .await
                .map_err(err)?;
            text(table(rows.iter().map(|s| {
                vec![
                    s.name.clone().unwrap_or_default(),
                    short(s.id),
                    s.model.clone(),
                    age(s.created_at),
                ]
            })))
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
    let model = agent
        .ask(agent::ListSessions)
        .await
        .map_err(err)?
        .into_iter()
        .find(|s| s.id == session)
        .map_or_else(|| "?".to_owned(), |s| s.model);
    // ponytail: LoadSession renders the whole history just for `running`;
    // have the channel pass its TurnStarted/TurnFinished flag if this shows.
    let loaded = agent
        .ask(agent::LoadSession { session })
        .await
        .map_err(err)?;
    let turn = if loaded.running { "running" } else { "idle" };
    out.push_str(&format!(
        "\nsession: {}\nmodel: {model}\nturn: {turn}",
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
        Some(SignalEvent::DaemonOnly) => "daemon only; run wth --headless".to_owned(),
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
            let rows = mcp.ask(tools::ListServers).await.map_err(err)?;
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
            .ask(tools::ListServers)
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

/// `name` is an OAuth provider (else it is taken as an MCP server).
async fn is_provider(name: &str) -> anyhow::Result<bool> {
    Ok(agent()?
        .ask(agent::ListLogins)
        .await
        .map_err(err)?
        .iter()
        .any(|l| l.name == name))
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
        .ask(tools::SetApprovals { session, enabled })
        .await
        .map_err(err)?;
    Ok(Reply::Text(format!(
        "approvals: {} (this session), {} (default)",
        on_off(state.session),
        on_off(state.global)
    )))
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

/// Last 8 hex chars, as the TUI shows ids; `/resume` accepts them.
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
        assert_eq!(classify(&p, "  /MODEL  "), Some(("/model", "")));
        assert_eq!(classify(&p, "/s"), Some(("/help", "")));
        assert_eq!(classify(&p, "/skills sync"), Some(("/skills", "sync")));
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

    #[test]
    fn table_aligns_columns() {
        let t = table(vec![
            vec!["a".into(), "one".into()],
            vec!["bbb".into(), "two".into()],
        ]);
        assert_eq!(t, "```\na    one\nbbb  two\n```");
        assert_eq!(table(Vec::<Vec<String>>::new()), "(none)");
    }
}
