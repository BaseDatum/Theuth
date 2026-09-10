//! Terminal UI plugin. Subscribes to `agent::AgentEvent` and
//! `tools::ApprovalRequested`; sends `SendInput` / `Steer` / `Cancel` /
//! `Resolve` by `ask`. Cancels `rt.shutdown` on quit. See design doc §8.
//!
//! Shape: a bridge actor forwards bus events into an mpsc channel; the UI
//! (fullscreen iocraft, or a line printer when stdout is not a TTY or `-p`
//! was given) is pure over [`model::Model`]; one task turns
//! [`model::Command`]s into actor asks and feeds replies back as
//! [`model::UiEvent`]s.

use std::io::{IsTerminal, Write};

use agent::{AGENT_ACTOR, AgentConfig, AgentEvent, Agents, LoginFinished};
use anyhow::Context as _;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tools::{
    APPROVAL_ACTOR, Approval, ApprovalRequested, ApprovalResolved, Decision, McpAuth, McpProgress,
};

pub mod model;
mod ui;

use model::{Command, LoginRow, Model, UiEvent};

/// The plugin.
pub struct TuiPlugin {
    /// Session to open on start (`wth --resume <id>`); `None` starts fresh.
    pub resume: Option<SessionId>,
    /// `wth -p <text>`: submit this, print the stream, exit at `TurnFinished`.
    pub prompt: Option<String>,
}

#[async_trait::async_trait]
impl runtime::Plugin for TuiPlugin {
    fn name(&self) -> &'static str {
        "tui"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let (events_tx, events_rx) = unbounded_channel();
        let (commands_tx, commands_rx) = unbounded_channel();
        let bridge = Bridge::spawn_with_mailbox(
            Bridge {
                tx: events_tx.clone(),
            },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Bridge, AgentEvent>(&bridge).await;
        rt.subscribe::<Bridge, ApprovalRequested>(&bridge).await;
        rt.subscribe::<Bridge, ApprovalResolved>(&bridge).await;
        rt.subscribe::<Bridge, McpProgress>(&bridge).await;
        rt.subscribe::<Bridge, LoginFinished>(&bridge).await;
        tokio::spawn(run_commands(commands_rx, events_tx));

        let default_model = rt.section::<AgentConfig>("agent").unwrap_or_default().model;
        let approvals = rt
            .section::<tools::ToolsConfig>("tools")
            .map_or(true, |c| c.approvals);
        let catalog = std::sync::Arc::new(models::Catalog::load(&rt.data_dir));
        let shutdown = rt.shutdown.clone();
        let resume = self.resume;
        let prompt = self.prompt;
        tokio::spawn(async move {
            if prompt.is_none() && std::io::stdout().is_terminal() {
                let links = ui::Links {
                    events: Some(events_rx),
                    commands: commands_tx,
                    default_model,
                    approvals,
                    catalog,
                    resume,
                };
                if let Err(e) = ui::run(links).await {
                    tracing::error!("tui render loop failed: {e}");
                }
            } else {
                if prompt.is_none() {
                    tracing::warn!("stdout is not a terminal; using the line printer");
                }
                line_printer(events_rx, commands_tx, resume, approvals, prompt).await;
            }
            shutdown.cancel();
        });
        Ok(())
    }
}

/// Bus subscriber: forwards every event into the UI channel.
#[derive(Actor)]
struct Bridge {
    tx: UnboundedSender<UiEvent>,
}

macro_rules! forward {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl Message<$ty> for Bridge {
            type Reply = ();
            async fn handle(&mut self, m: $ty, _: &mut Context<Self, ()>) {
                let _ = self.tx.send(UiEvent::$variant(m));
            }
        }
    )*};
}
forward!(
    AgentEvent => Agent,
    ApprovalRequested => Approval,
    ApprovalResolved => Resolved,
    McpProgress => McpProgress,
    LoginFinished => LoginFinished,
);

/// The one place actors are looked up and asked. Each command runs in its
/// own task so a slow ask never delays the next one.
async fn run_commands(mut rx: UnboundedReceiver<Command>, tx: UnboundedSender<UiEvent>) {
    while let Some(cmd) = rx.recv().await {
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = execute(cmd, &tx).await {
                let _ = tx.send(UiEvent::Error(format!("{e:#}")));
            }
        });
    }
}

async fn execute(cmd: Command, tx: &UnboundedSender<UiEvent>) -> anyhow::Result<()> {
    let reply = match cmd {
        Command::Resolve { id, decision } => {
            let approval = ActorRef::<Approval>::lookup(APPROVAL_ACTOR)?
                .context("approval actor is not running")?;
            if !approval.ask(tools::Resolve { id, decision }).await? {
                anyhow::bail!("approval {} was no longer pending", id.0);
            }
            return Ok(());
        }
        Command::Send { session, text } => {
            UiEvent::SessionCreated(agent()?.ask(agent::SendInput { session, text }).await?)
        }
        Command::Steer { session, text } => {
            agent()?.ask(agent::Steer { session, text }).await?;
            return Ok(());
        }
        Command::Cancel(session) => {
            agent()?.ask(agent::Cancel { session }).await?;
            return Ok(());
        }
        Command::Compact(session) => {
            let epoch = agent()?
                .ask(agent::Compact { session })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            UiEvent::Info(format!("context compacted (epoch {epoch})"))
        }
        Command::ListSessions => UiEvent::Sessions(agent()?.ask(agent::ListSessions).await?),
        Command::ListChildren(parent) => {
            UiEvent::Children(agent()?.ask(agent::ListChildren { parent }).await?)
        }
        Command::Load(session) => {
            UiEvent::History(session, agent()?.ask(agent::LoadSession { session }).await?)
        }
        Command::ListModels => UiEvent::Models(agent()?.ask(agent::ListModels).await?),
        Command::SetModel { session, model } => UiEvent::ModelSet(
            agent()?
                .ask(agent::SetModel { session, model })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::ListMcp => UiEvent::Mcp(mcp()?.ask(tools::ListServers).await?),
        Command::McpStatus => UiEvent::McpProgress(mcp()?.ask(tools::Progress).await?),
        Command::SetGate {
            server,
            tool,
            action,
        } => {
            let mcp = mcp()?;
            mcp.ask(tools::SetGate {
                server,
                tool,
                action,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Re-list here, not from the UI: commands run concurrently.
            UiEvent::Mcp(mcp.ask(tools::ListServers).await?)
        }
        Command::SetApprovals { session, enabled } => {
            let registry = ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)?
                .context("tool registry is not running")?;
            UiEvent::Approvals(
                registry
                    .ask(tools::SetApprovals { session, enabled })
                    .await?,
            )
        }
        Command::ShowPrompt(session) => {
            UiEvent::Prompt(agent()?.ask(agent::ShowPrompt { session }).await?)
        }
        Command::ListSkills => UiEvent::Skills(agent()?.ask(agent::ListSkills).await?),
        Command::LoadSkill(name) => UiEvent::Skill(
            agent()?
                .ask(agent::LoadSkill { name })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::AddSkillRepo(url) => UiEvent::SkillsAdded(
            agent()?
                .ask(agent::AddSkillRepo { url, name: None })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        // A repo dir first (only removed when it holds `.git`), else a local skill.
        Command::RemoveSkill(name) => {
            let agent = agent()?;
            let repo = agent::RemoveSkillRepo { name: name.clone() };
            if agent.ask(repo).await.is_ok() {
                UiEvent::Info(format!("removed repo {name}"))
            } else {
                agent
                    .ask(agent::RemoveSkill { name: name.clone() })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                UiEvent::Info(format!("removed skill {name}"))
            }
        }
        Command::SyncSkills => UiEvent::Synced(agent()?.ask(agent::SyncSkills).await?),
        Command::KnowledgeList => UiEvent::Knowledge(
            agent()?
                .ask(agent::KnowledgeList {
                    limit: Some(50),
                    ..Default::default()
                })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::KnowledgeSearch(query) => UiEvent::Knowledge(
            agent()?
                .ask(agent::KnowledgeSearch { query, limit: None })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_iter()
                .map(|h| h.meta)
                .collect(),
        ),
        Command::KnowledgeHistory(id) => UiEvent::KnowledgeHistory(
            agent()?
                .ask(agent::KnowledgeHistory { id, limit: None })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::KnowledgeRestore { id, commit } => UiEvent::KnowledgeRestored(
            agent()?
                .ask(agent::KnowledgeRestore { id, commit })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::Harvest(session) => UiEvent::Harvested(
            agent()?
                .ask(agent::Harvest { session })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::AddMcp { name, entry } => UiEvent::McpAdded(
            mcp()?
                .ask(tools::AddServer { name, entry })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::AddMcpRepo(url) => UiEvent::RepoAdded(
            mcp()?
                .ask(tools::AddRepo { url, name: None })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        ),
        Command::SyncMcp => UiEvent::Synced(mcp()?.ask(tools::SyncMcp).await?),
        Command::ListLogins => {
            let mut rows: Vec<LoginRow> = agent()?
                .ask(agent::ListLogins)
                .await?
                .into_iter()
                .map(|l| LoginRow {
                    name: l.name,
                    mcp: false,
                    status: match (l.logged_in, l.account) {
                        (false, _) => "not logged in".to_owned(),
                        (true, Some(a)) => format!("logged in ({a})"),
                        (true, None) => "logged in".to_owned(),
                    },
                })
                .collect();
            rows.extend(
                mcp()?
                    .ask(tools::ListServers)
                    .await?
                    .into_iter()
                    .filter(|s| s.auth != McpAuth::None)
                    .map(|s| LoginRow {
                        name: s.name,
                        mcp: true,
                        status: match s.auth {
                            McpAuth::LoggedIn => "logged in".to_owned(),
                            _ => "login required".to_owned(),
                        },
                    }),
            );
            UiEvent::Logins(rows)
        }
        // A provider name first, else an MCP server.
        Command::Login(name) => {
            let url = if oauth::provider(&name).is_some() {
                agent()?
                    .ask(agent::Login { provider: name })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?
            } else {
                mcp()?
                    .ask(tools::McpLogin { server: name })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?
            };
            UiEvent::Info(format!("login: open {url}"))
        }
        Command::Logout(name) => {
            if oauth::provider(&name).is_some() {
                agent()?
                    .ask(agent::Logout {
                        provider: name.clone(),
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            } else {
                mcp()?
                    .ask(tools::McpLogout {
                        server: name.clone(),
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            UiEvent::Info(format!("logged out of {name}"))
        }
    };
    let _ = tx.send(reply);
    Ok(())
}

fn agent() -> anyhow::Result<ActorRef<Agents>> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)?.context("agent actor is not running")
}

fn mcp() -> anyhow::Result<ActorRef<tools::Mcp>> {
    ActorRef::<tools::Mcp>::lookup(tools::MCP_ACTOR)?.context("mcp actor is not running")
}

/// Non-TTY fallback and `-p` print mode: print what streams in, read lines
/// from stdin. `y`/`n` answer a pending approval; anything else goes through
/// [`Model::submit`]. With `prompt`, it is submitted first and the loop ends
/// when the root turn finishes (or a command fails); stdin EOF then denies
/// approvals instead of quitting, since a piped prompt leaves stdin closed.
async fn line_printer(
    mut events: UnboundedReceiver<UiEvent>,
    commands: UnboundedSender<Command>,
    resume: Option<SessionId>,
    approvals: bool,
    prompt: Option<String>,
) {
    let mut model = Model::new(String::new(), approvals);
    if let Some(id) = resume {
        model.resume(id, &commands);
    }
    let print_mode = prompt.is_some();
    if let Some(p) = &prompt {
        model.submit(p, &commands);
    }
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_open = true;
    let mut out = std::io::stdout();
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                print_event(&mut out, &model, &ev);
                let done = print_mode && match &ev {
                    UiEvent::Agent(AgentEvent::TurnFinished { session, .. }) => model.session == Some(*session),
                    UiEvent::Error(_) => true,
                    _ => false,
                };
                model.apply(ev);
                if done { break; }
                if !stdin_open && model.approval().is_some() {
                    let _ = writeln!(out, "denied: stdin closed");
                    model.decide(Decision::Deny, &commands);
                }
            }
            line = lines.next_line(), if stdin_open => {
                let Ok(Some(line)) = line else {
                    if print_mode { stdin_open = false; continue; }
                    break;
                };
                match (line.trim(), model.approval().is_some()) {
                    ("y", true) => model.decide(Decision::Allow, &commands),
                    ("n", true) => model.decide(Decision::Deny, &commands),
                    (line, _) => model.submit(line, &commands),
                }
                if model.quit { break; }
            }
        }
    }
}

fn print_event(out: &mut impl Write, model: &Model, ev: &UiEvent) {
    let _ = match ev {
        UiEvent::Agent(AgentEvent::SubagentSpawned { name, .. }) => {
            writeln!(out, "↳ spawned {name}")
        }
        UiEvent::Agent(AgentEvent::MessageDelivered { from, text, .. }) => {
            writeln!(
                out,
                "⟵ {from}: {}…",
                text.lines().next().unwrap_or_default()
            )
        }
        UiEvent::Agent(AgentEvent::TextDelta { text, .. }) => {
            out.write_all(text.as_bytes()).and_then(|()| out.flush())
        }
        UiEvent::Agent(AgentEvent::ToolCallStarted {
            call_id,
            name,
            args,
            ..
        }) => {
            writeln!(
                out,
                "\n⚙ {}{name}\n{}",
                if call_id.contains('.') { "↳ " } else { "" },
                model::describe(name, args).join("\n")
            )
        }
        UiEvent::Agent(AgentEvent::ToolCallFinished {
            output, is_error, ..
        }) => {
            writeln!(
                out,
                "{} {}",
                if *is_error { "✗" } else { "✓" },
                model::result_line(output)
            )
        }
        UiEvent::Agent(AgentEvent::TurnFinished {
            stop_reason, usage, ..
        }) => {
            let cache = (usage.cached * 100)
                .checked_div(usage.prompt)
                .map_or_else(String::new, |pct| format!(" cache {pct}%"));
            writeln!(
                out,
                "\n[{stop_reason}] {}↑ {}↓{cache}",
                usage.prompt, usage.completion
            )
        }
        UiEvent::Agent(AgentEvent::ModelChanged { model, .. }) => writeln!(out, "model → {model}"),
        UiEvent::Agent(AgentEvent::Compacted { epoch, .. }) => {
            writeln!(out, "context compacted (epoch {epoch})")
        }
        UiEvent::Agent(AgentEvent::Harvested { added, updated, .. }) => {
            writeln!(out, "knowledge +{added} ~{updated}")
        }
        UiEvent::Agent(AgentEvent::Error { message, .. }) | UiEvent::Error(message) => {
            writeln!(out, "error: {message}")
        }
        UiEvent::Agent(_) => Ok(()),
        UiEvent::ModelSet(info) => writeln!(out, "model → {}", info.id),
        UiEvent::Models(_) | UiEvent::Mcp(_) => writeln!(out, "menus need a TTY"),
        UiEvent::Logins(rows) => rows
            .iter()
            .try_for_each(|r| writeln!(out, "{}  {}", r.name, r.status)),
        UiEvent::LoginFinished(f) => match &f.error {
            None => writeln!(out, "login: {} ok", f.provider),
            Some(e) => writeln!(out, "login: {} failed: {e}", f.provider),
        },
        UiEvent::Prompt(text) | UiEvent::Info(text) => writeln!(out, "{text}"),
        UiEvent::Skills(list) | UiEvent::SkillsAdded(list) => list.iter().try_for_each(|s| {
            writeln!(
                out,
                "{}  {}{}{}",
                s.name,
                s.description,
                s.repo
                    .as_deref()
                    .map_or_else(String::new, |r| format!("  [{r}]")),
                if s.always { "  always" } else { "" }
            )
        }),
        UiEvent::Skill(s) => writeln!(out, "◇ {}\n{}\n{}", s.name, s.body, s.files.join("\n")),
        UiEvent::Synced(list) => list
            .iter()
            .try_for_each(|r| writeln!(out, "{}  {}", r.name, r.status)),
        UiEvent::McpAdded(s) => writeln!(out, "{}  {} tools", s.name, s.tools.len()),
        UiEvent::RepoAdded(r) => writeln!(out, "cloned {} → {}\n{}", r.name, r.path, r.readme),
        UiEvent::Knowledge(metas) => metas
            .iter()
            .try_for_each(|m| writeln!(out, "{}", model::meta_line(m))),
        UiEvent::KnowledgeHistory(h) => h
            .changes
            .iter()
            .try_for_each(|c| writeln!(out, "{}  {}  {}", c.commit, c.time, c.message))
            .and_then(|()| match &h.diff {
                Some(d) => writeln!(out, "{d}"),
                None => Ok(()),
            }),
        UiEvent::KnowledgeRestored(c) => writeln!(out, "{}  {}", c.commit, c.message),
        UiEvent::Harvested(r) => writeln!(out, "knowledge +{} ~{}", r.added, r.updated),
        UiEvent::Approvals(s) => writeln!(
            out,
            "approvals: {} (this session), {} (default)",
            if s.session { "on" } else { "off" },
            if s.global { "on" } else { "off" }
        ),
        UiEvent::McpProgress(p) if p.connecting.is_empty() => writeln!(
            out,
            "mcp: {} connected{}",
            p.connected.len(),
            match p.failed.as_slice() {
                [] => String::new(),
                f => format!(", failed: {}", f.join(", ")),
            }
        ),
        UiEvent::McpProgress(_) => Ok(()),
        UiEvent::Approval(req) => writeln!(
            out,
            "approve {} ({:?}){}? [y/n]\n{}",
            req.tool,
            req.risk,
            model
                .peer_name(req.session)
                .map_or_else(String::new, |n| format!(" [agent: {n}]")),
            model::describe(&req.tool, &req.args).join("\n")
        ),
        UiEvent::Resolved(_) => Ok(()),
        UiEvent::Sessions(list) => list
            .iter()
            .try_for_each(|s| writeln!(out, "{}  {}  {}", s.id, s.model, s.title)),
        UiEvent::Children(list) => list
            .iter()
            .try_for_each(|s| writeln!(out, "{}  {}", s.name.as_deref().unwrap_or_default(), s.id)),
        UiEvent::History(_, loaded) => loaded.history.iter().try_for_each(|h| match h {
            agent::HistoryEntry::User { text } => writeln!(out, "> {text}"),
            agent::HistoryEntry::Assistant { text } => writeln!(out, "{text}"),
            agent::HistoryEntry::Tool {
                name,
                args,
                output,
                is_error,
            } => writeln!(
                out,
                "⚙ {name}\n{}\n{} {}",
                model::describe(name, args).join("\n"),
                if *is_error { "✗" } else { "✓" },
                model::result_line(output)
            ),
            agent::HistoryEntry::Compacted { epoch, summary } => {
                writeln!(out, "── context compacted (epoch {epoch}) ──\n{summary}")
            }
        }),
        UiEvent::SessionCreated(id) => writeln!(out, "session {id}"),
    };
}
