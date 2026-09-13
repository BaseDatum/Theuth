//! Terminal UI plugin. Subscribes to `agent::AgentEvent` and
//! `tools::ApprovalRequested`; every command is an in-process call on
//! [`rpc::api::Server`], the same [`rpc::api::Api`] the socket exposes.
//! Cancels `rt.shutdown` on quit. See design doc §8.
//!
//! Shape: a bridge actor forwards bus events into an mpsc channel; the UI
//! (fullscreen iocraft, or a line printer when stdout is not a TTY or `-p`
//! was given) is pure over [`model::Model`]; one task turns
//! [`model::Command`]s into `rpc::Api` calls and feeds replies back as
//! [`model::UiEvent`]s.

use std::io::{IsTerminal, Write};
use std::sync::Arc;

use agent::{AgentConfig, AgentEvent, CompactStyle, LoginFinished};
use jsonrpsee::core::RpcResult;
use kameo::Actor;
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use rpc::api::{ApiServer, Server};
use runtime::{Runtime, SessionId};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tools::{ApprovalRequested, ApprovalResolved, Decision, JobEvent, McpAuth, McpProgress};

mod clip;
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
        rt.subscribe::<Bridge, JobEvent>(&bridge).await;
        let api = Arc::new(Server::new(rt.clone()));
        tokio::spawn(run_commands(api, commands_rx, events_tx));

        let default_model = rt.section::<AgentConfig>("agent").unwrap_or_default().model;
        let approvals = rt
            .section::<tools::ToolsConfig>("tools")
            .map_or(true, |c| c.approvals);
        let catalog = Arc::new(models::Catalog::load(&rt.data_dir));
        let shutdown = rt.shutdown.clone();
        let data_dir = rt.data_dir.clone();
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
                    data_dir,
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
    JobEvent => Job,
);

/// The one place commands become `rpc::Api` calls. Each command runs in its
/// own task so a slow call never delays the next one.
async fn run_commands(
    api: Arc<Server>,
    mut rx: UnboundedReceiver<Command>,
    tx: UnboundedSender<UiEvent>,
) {
    while let Some(cmd) = rx.recv().await {
        let tx = tx.clone();
        let api = api.clone();
        tokio::spawn(async move {
            if let Err(e) = execute(&api, cmd, &tx).await {
                let _ = tx.send(UiEvent::Error(format!("{e:#}")));
            }
        });
    }
}

async fn execute(api: &Server, cmd: Command, tx: &UnboundedSender<UiEvent>) -> anyhow::Result<()> {
    let reply = match cmd {
        Command::Resolve { id, decision } => {
            if !msg(api.resolve_approval(id, decision).await)? {
                anyhow::bail!("approval {} was no longer pending", id.0);
            }
            return Ok(());
        }
        Command::Send {
            session,
            text,
            files,
        } => UiEvent::SessionCreated(msg(api.send_input(session, text, files).await)?),
        Command::Steer {
            session,
            id,
            text,
            files,
        } => {
            msg(api.steer(session, text, Some(id), files).await)?;
            return Ok(());
        }
        Command::Withdraw { session, id } => UiEvent::Withdrawn {
            id,
            text: msg(api.withdraw(session, id).await)?,
        },
        // The transcript is reloaded here, not from the UI: commands run
        // concurrently and the notice must land before the new history.
        Command::Undo(session) => {
            let _ = tx.send(UiEvent::Undone(msg(api.undo(session).await)?));
            UiEvent::History(session, msg(api.load_session(session).await)?)
        }
        Command::Redo(session) => {
            let _ = tx.send(UiEvent::Redone(msg(api.redo(session).await)?));
            UiEvent::History(session, msg(api.load_session(session).await)?)
        }
        Command::Cancel(session) => {
            msg(api.cancel(session).await)?;
            return Ok(());
        }
        Command::ListJobs(session) => UiEvent::Jobs(msg(api.list_jobs(session).await)?),
        Command::CancelJob { session, id } => {
            msg(api.cancel_job(session, tools::JobId(id.clone())).await)?;
            UiEvent::Info(format!("killed {id}"))
        }
        Command::Compact { session, style } => {
            let epoch = msg(api.compact(session, style).await)?;
            UiEvent::Info(match style {
                // The `Compacted` event wipes the transcript and announces it.
                Some(CompactStyle::Clear) => return Ok(()),
                Some(s) => format!("context compacted (epoch {epoch} · {s})"),
                None => format!("context compacted (epoch {epoch})"),
            })
        }
        Command::GetCompactStyle => UiEvent::CompactStyle(msg(api.compact_style().await)?),
        Command::SetCompactStyle(style) => {
            msg(api.set_compact_style(style).await)?;
            UiEvent::Info(format!("compact style: {style}"))
        }
        Command::GetTitleModel => UiEvent::TitleModel(msg(api.title_model().await)?),
        Command::SetTitleModel(model) => {
            UiEvent::TitleModel(msg(api.set_title_model(model).await)?)
        }
        Command::ListSessions => UiEvent::Sessions(msg(api.list_sessions().await)?),
        Command::ListAgents(session) => UiEvent::Agents(msg(api.list_agents(session).await)?),
        Command::Load(session) => UiEvent::History(session, msg(api.load_session(session).await)?),
        Command::ListModels => UiEvent::Models(msg(api.list_models().await)?),
        Command::SetModel { session, model } => {
            UiEvent::ModelSet(msg(api.set_model(session, model).await)?)
        }
        Command::SetEffort { session, effort } => {
            msg(api.set_effort(session, effort.clone()).await)?;
            UiEvent::EffortSet(effort)
        }
        Command::ListMcp(session) => UiEvent::Mcp(msg(api.list_mcp(session).await)?),
        Command::McpStatus => UiEvent::McpProgress(msg(api.mcp_status().await)?),
        Command::SetGate {
            server,
            tool,
            gate,
            session,
        } => {
            msg(api.set_mcp_gate(server, tool, gate).await)?;
            // Re-list here, not from the UI: commands run concurrently.
            UiEvent::Mcp(msg(api.list_mcp(session).await)?)
        }
        Command::SetHidden {
            session,
            tools,
            hidden,
        } => {
            let session = match session {
                Some(s) => s,
                None => {
                    let s = msg(api.new_session().await)?;
                    let _ = tx.send(UiEvent::SessionCreated(s));
                    s
                }
            };
            msg(api.set_mcp_hidden(session, tools, hidden).await)?;
            UiEvent::Mcp(msg(api.list_mcp(Some(session)).await)?)
        }
        Command::SetApprovals { session, enabled } => {
            UiEvent::Approvals(msg(api.set_approvals(session, enabled).await)?)
        }
        Command::ShowPrompt(session) => UiEvent::Prompt(msg(api.get_prompt(session).await)?),
        Command::ListSkills => UiEvent::Skills(msg(api.list_skills().await)?),
        Command::LoadSkill(name) => UiEvent::Skill(msg(api.load_skill(name).await)?),
        Command::UseSkill {
            session,
            name,
            args,
        } => UiEvent::SessionCreated(msg(api.use_skill(session, name, args).await)?),
        Command::AddSkillRepo(url) => {
            UiEvent::SkillsAdded(msg(api.add_skill_repo(url, None).await)?)
        }
        // A repo dir first (only removed when it holds `.git`), else a local skill.
        Command::RemoveSkill(name) => {
            if api.remove_skill_repo(name.clone()).await.is_ok() {
                UiEvent::Info(format!("removed repo {name}"))
            } else {
                msg(api.remove_skill(name.clone()).await)?;
                UiEvent::Info(format!("removed skill {name}"))
            }
        }
        Command::SyncSkills => UiEvent::Synced(msg(api.sync_skills().await)?),
        Command::KnowledgeList => {
            UiEvent::Knowledge(msg(api.knowledge_list(None, None, None, Some(50)).await)?)
        }
        Command::KnowledgeSearch(query) => UiEvent::Knowledge(
            msg(api.knowledge_search(query, None).await)?
                .into_iter()
                .map(|h| h.meta)
                .collect(),
        ),
        Command::KnowledgeHistory(id) => {
            UiEvent::KnowledgeHistory(msg(api.knowledge_history(id, None).await)?)
        }
        Command::KnowledgeRestore { id, commit } => {
            UiEvent::KnowledgeRestored(msg(api.knowledge_restore(id, commit).await)?)
        }
        Command::Harvest(session) => UiEvent::Harvested(msg(api.knowledge_harvest(session).await)?),
        Command::AddMcp { name, entry } => {
            UiEvent::McpAdded(msg(api.add_mcp_server(name, entry).await)?)
        }
        Command::AddMcpRepo(url) => UiEvent::RepoAdded(msg(api.add_mcp_repo(url, None).await)?),
        Command::SyncMcp => UiEvent::Synced(msg(api.sync_mcp().await)?),
        Command::ListAcp { probe } => UiEvent::Acp(msg(api.list_acp(probe).await)?),
        Command::AddAcp(id) => {
            let _ = tx.send(UiEvent::AcpAdded(msg(api.add_acp(id).await)?));
            // Re-list here, not from the UI: commands run concurrently.
            UiEvent::Acp(msg(api.list_acp(false).await)?)
        }
        Command::RemoveAcp(name) => {
            msg(api.remove_acp(name).await)?;
            UiEvent::Acp(msg(api.list_acp(false).await)?)
        }
        Command::ListAcpRegistry => UiEvent::AcpRegistry(msg(api.list_acp_registry().await)?),
        Command::ListLogins => {
            let mut rows: Vec<LoginRow> = msg(api.list_logins().await)?
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
                msg(api.list_mcp(None).await)?
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
                msg(api.login(name).await)?
            } else {
                msg(api.mcp_login(name).await)?
            };
            UiEvent::Info(format!("login: open {url}"))
        }
        Command::Logout(name) => {
            if oauth::provider(&name).is_some() {
                msg(api.logout(name.clone()).await)?;
            } else {
                msg(api.mcp_logout(name.clone()).await)?;
            }
            UiEvent::Info(format!("logged out of {name}"))
        }
        Command::ListProviders => UiEvent::Providers(msg(api.list_providers().await)?),
        Command::AddProvider {
            name,
            base_url,
            key,
        } => UiEvent::ProviderAdded(msg(api.add_provider(name, base_url, key).await)?),
        Command::RemoveProvider(name) => {
            msg(api.remove_provider(name.clone()).await)?;
            UiEvent::Info(format!("removed provider {name}"))
        }
    };
    let _ = tx.send(reply);
    Ok(())
}

/// Keep only the message: the JSON-RPC code is not user-facing.
fn msg<T>(r: RpcResult<T>) -> anyhow::Result<T> {
    r.map_err(|e| anyhow::anyhow!("{}", e.message()))
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
        UiEvent::Agent(AgentEvent::Compacted { epoch, style, .. }) => {
            writeln!(out, "context compacted (epoch {epoch} · {style})")
        }
        UiEvent::Agent(AgentEvent::Harvested { added, updated, .. }) => {
            writeln!(out, "knowledge +{added} ~{updated}")
        }
        UiEvent::Agent(AgentEvent::Error { message, .. }) | UiEvent::Error(message) => {
            writeln!(out, "error: {message}")
        }
        UiEvent::Agent(_) => Ok(()),
        UiEvent::ModelSet(info) => writeln!(out, "model → {}", info.id),
        UiEvent::EffortSet(Some(e)) => writeln!(out, "effort: {e}"),
        UiEvent::EffortSet(None) => writeln!(out, "effort cleared"),
        UiEvent::Models(_) | UiEvent::Mcp(_) => writeln!(out, "menus need a TTY"),
        UiEvent::Logins(rows) => rows
            .iter()
            .try_for_each(|r| writeln!(out, "{}  {}", r.name, r.status)),
        UiEvent::LoginFinished(f) => match &f.error {
            None => writeln!(out, "login: {} ok", f.provider),
            Some(e) => writeln!(out, "login: {} failed: {e}", f.provider),
        },
        UiEvent::Prompt(text) | UiEvent::Info(text) => writeln!(out, "{text}"),
        UiEvent::Jobs(jobs) => writeln!(out, "{}", tools::jobs::table(jobs)),
        UiEvent::Job(_) => Ok(()),
        UiEvent::Withdrawn { text, .. } => writeln!(out, "withdrawn: {text}"),
        UiEvent::Undone(text) => writeln!(out, "undone: {text} — /redo restores"),
        UiEvent::Redone(text) => writeln!(out, "restored: {text}"),
        UiEvent::CompactStyle(s) => writeln!(
            out,
            "compact style: {s} (default)\n  summary  cache-hot LLM fold (07 §8.3)\n  cut      drop the folded span, keep the tail, no model call\n  snap     rasterize the folded span to images (vision model)"
        ),
        UiEvent::TitleModel(m) => writeln!(out, "{}", model::title_model_line(m.as_ref())),
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
        UiEvent::Acp(list) => list.iter().try_for_each(|e| {
            writeln!(
                out,
                "{}  {} {}  {}",
                e.name,
                e.command,
                e.args.join(" "),
                e.status.as_deref().unwrap_or_default()
            )
        }),
        UiEvent::AcpRegistry(list) => list.iter().try_for_each(|a| {
            writeln!(
                out,
                "+ {}  {} {}  {}",
                a.id, a.name, a.version, a.description
            )
        }),
        UiEvent::AcpAdded(e) => writeln!(
            out,
            "acp: added `{}` ({} {})",
            e.name,
            e.command,
            e.args.join(" ")
        ),
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
        UiEvent::Agents(list) => list.iter().try_for_each(|a| {
            writeln!(
                out,
                "{}  {}  {}  {}",
                a.name,
                a.state.word(),
                a.last,
                model::short(a.id)
            )
        }),
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
            agent::HistoryEntry::Compacted {
                epoch,
                summary,
                style,
            } => {
                writeln!(
                    out,
                    "── context compacted (epoch {epoch} · {style}) ──\n{summary}"
                )
            }
        }),
        UiEvent::SessionCreated(id) => writeln!(out, "session {id}"),
        UiEvent::Providers(list) => list
            .iter()
            .try_for_each(|p| writeln!(out, "{}  {}  {} models", p.name, p.base_url, p.models)),
        UiEvent::ProviderAdded(p) => writeln!(
            out,
            "provider {}  {}  {} models — /model {}/<id>",
            p.name, p.base_url, p.models, p.name
        ),
    };
}
