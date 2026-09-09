//! Terminal UI plugin. Subscribes to `agent::AgentEvent` and
//! `tools::ApprovalRequested`; sends `SendInput` / `Steer` / `Cancel` /
//! `Resolve` by `ask`. Cancels `rt.shutdown` on quit. See design doc §8.
//!
//! Shape: a bridge actor forwards bus events into an mpsc channel; the UI
//! (fullscreen iocraft, or a line printer when stdout is not a TTY) is pure
//! over [`model::Model`]; one task turns [`model::Command`]s into actor asks
//! and feeds replies back as [`model::UiEvent`]s.

use std::io::{IsTerminal, Write};

use agent::{AGENT_ACTOR, AgentConfig, AgentEvent, Agents};
use anyhow::Context as _;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tools::{APPROVAL_ACTOR, Approval, ApprovalRequested, ApprovalResolved, Decision};

pub mod model;
mod ui;

use model::{Command, Model, UiEvent};

/// The plugin.
pub struct TuiPlugin {
    /// Session to open on start (`wth --resume <id>`); `None` starts fresh.
    pub resume: Option<SessionId>,
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
        tokio::spawn(run_commands(commands_rx, events_tx));

        let default_model = rt.section::<AgentConfig>("agent").unwrap_or_default().model;
        let catalog = std::sync::Arc::new(models::Catalog::load(&rt.data_dir));
        let shutdown = rt.shutdown.clone();
        let resume = self.resume;
        tokio::spawn(async move {
            if std::io::stdout().is_terminal() {
                let links = ui::Links {
                    events: Some(events_rx),
                    commands: commands_tx,
                    default_model,
                    catalog,
                    resume,
                };
                if let Err(e) = ui::run(links).await {
                    tracing::error!("tui render loop failed: {e}");
                }
            } else {
                tracing::warn!("stdout is not a terminal; using the line printer");
                line_printer(events_rx, commands_tx, resume).await;
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
forward!(AgentEvent => Agent, ApprovalRequested => Approval, ApprovalResolved => Resolved);

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

/// Non-TTY fallback: print what streams in, read lines from stdin. `y`/`n`
/// answer a pending approval; anything else goes through [`Model::submit`].
async fn line_printer(
    mut events: UnboundedReceiver<UiEvent>,
    commands: UnboundedSender<Command>,
    resume: Option<SessionId>,
) {
    let mut model = Model::default();
    if let Some(id) = resume {
        model.resume(id, &commands);
    }
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut out = std::io::stdout();
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                print_event(&mut out, &model, &ev);
                model.apply(ev);
            }
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
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
        UiEvent::Agent(AgentEvent::ToolCallStarted { name, summary, .. }) => {
            writeln!(out, "\n⚙ {name} {summary}")
        }
        UiEvent::Agent(AgentEvent::ToolCallFinished {
            output, is_error, ..
        }) => {
            writeln!(
                out,
                "{} {}",
                if *is_error { "✗" } else { "✓" },
                output.lines().next().unwrap_or_default()
            )
        }
        UiEvent::Agent(AgentEvent::TurnFinished {
            stop_reason, usage, ..
        }) => {
            writeln!(
                out,
                "\n[{stop_reason}] {}↑ {}↓",
                usage.prompt, usage.completion
            )
        }
        UiEvent::Agent(AgentEvent::ModelChanged { model, .. }) => writeln!(out, "model → {model}"),
        UiEvent::Agent(AgentEvent::Error { message, .. }) | UiEvent::Error(message) => {
            writeln!(out, "error: {message}")
        }
        UiEvent::Agent(_) => Ok(()),
        UiEvent::ModelSet(info) => writeln!(out, "model → {}", info.id),
        UiEvent::Models(_) | UiEvent::Mcp(_) => writeln!(out, "menus need a TTY"),
        UiEvent::Approval(req) => writeln!(
            out,
            "approve {} ({:?}){}: {}? [y/n]",
            req.tool,
            req.risk,
            model
                .peer_name(req.session)
                .map_or_else(String::new, |n| format!(" [agent: {n}]")),
            req.summary
        ),
        UiEvent::Resolved(_) => Ok(()),
        UiEvent::Sessions(list) => list
            .iter()
            .try_for_each(|s| writeln!(out, "{}  {}  {}", s.id, s.model, s.title)),
        UiEvent::Children(list) => list
            .iter()
            .try_for_each(|s| writeln!(out, "{}  {}", s.name.as_deref().unwrap_or_default(), s.id)),
        UiEvent::History(_, history) => history.iter().try_for_each(|h| match h {
            agent::HistoryEntry::User { text } => writeln!(out, "> {text}"),
            agent::HistoryEntry::Assistant { text } => writeln!(out, "{text}"),
            agent::HistoryEntry::Tool {
                name,
                summary,
                output,
                is_error,
            } => writeln!(
                out,
                "⚙ {name} {summary}\n{} {}",
                if *is_error { "✗" } else { "✓" },
                output.lines().next().unwrap_or_default()
            ),
        }),
        UiEvent::SessionCreated(id) => writeln!(out, "session {id}"),
    };
}
