//! Terminal UI plugin. Subscribes to `agent::AgentEvent`,
//! `tools::ApprovalRequested`, `secrets::SecretsEvent` and the rest of the
//! bus; every command is an [`rpc::api::ApiClient`] call on an
//! [`rpc::client::Engine`]: in-process (this process is the engine) or over a
//! WebSocket to a running engine the TUI attached to (63 §2.3), whose events
//! [`rpc::client::mirror`] republishes on the local bus. Cancels
//! `rt.shutdown` on quit. See design doc §8.
//!
//! Shape: a bridge actor forwards bus events into an mpsc channel; the UI
//! (fullscreen iocraft, or a line printer when stdout is not a TTY or `-p`
//! was given) is pure over [`model::Model`]; one task turns
//! [`model::Command`]s into `rpc::Api` calls and feeds replies back as
//! [`model::UiEvent`]s.

use std::io::{IsTerminal, Write};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use agent::{AgentConfig, AgentEvent, CompactStyle, LoginFinished};
use futures::future::BoxFuture;
use jsonrpsee::core::client::Error as ClientError;
use jsonrpsee::ws_client::WsClient;
use kameo::Actor;
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use rpc::api::ApiClient;
use rpc::client::{Engine, Local};
use rpc::engine::{Host, Running};
use runtime::{Runtime, SessionId};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tools::{
    ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, Decision, JobEvent, McpAuth,
    McpProgress,
};

pub mod attach;
pub mod broadcast;
mod clip;
pub mod dictation;
pub mod model;
pub mod pathac;
pub mod search;
pub mod talk;
mod ui;

use model::{Command, DoubleCmd, LoginRow, Model, UiEvent};

/// The plugin.
pub struct TuiPlugin {
    /// Session to open on start (`t --resume <id>`); `None` starts fresh.
    pub resume: Option<SessionId>,
    /// `theuth -p <text>`: submit this, print the stream, exit at `TurnFinished`.
    pub prompt: Option<String>,
    /// Attached to a running engine (63 §0.3); `None` when this process is it.
    pub attached: Option<Attached>,
}

/// Starts the engine in this process (the plugin list `main` would have
/// started) when an attached TUI takes over (63 §2.3).
pub type StartEngine = Box<dyn FnOnce(Runtime) -> BoxFuture<'static, anyhow::Result<()>> + Send>;

/// The engine a TUI attached to.
pub struct Attached {
    /// Its socket.
    pub client: Arc<WsClient>,
    /// Its record, for the status line.
    pub running: Running,
    /// How to become the engine once it is gone.
    pub start_engine: StartEngine,
}

/// The calls' target, swapped on takeover; readers clone the `Arc` out.
type Api = Arc<RwLock<Arc<Engine>>>;

/// `attached · pid N · desktop` (63 §0.3).
fn attached_label(r: &Running) -> String {
    format!("attached · pid {} · {}", r.record.pid, r.record.host)
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
        rt.subscribe::<Bridge, AskRequested>(&bridge).await;
        rt.subscribe::<Bridge, AskResolved>(&bridge).await;
        rt.subscribe::<Bridge, double::DoublePredicted>(&bridge)
            .await;
        rt.subscribe::<Bridge, double::DoubleAnswered>(&bridge)
            .await;
        rt.subscribe::<Bridge, McpProgress>(&bridge).await;
        rt.subscribe::<Bridge, LoginFinished>(&bridge).await;
        rt.subscribe::<Bridge, JobEvent>(&bridge).await;
        rt.subscribe::<Bridge, link::LinkEvent>(&bridge).await;
        rt.subscribe::<Bridge, nodes::NodeEvent>(&bridge).await;
        rt.subscribe::<Bridge, artifact::ArtifactShown>(&bridge)
            .await;
        rt.subscribe::<Bridge, secrets::SecretsEvent>(&bridge).await;
        rt.subscribe::<Bridge, knowledge::KnowledgeChanged>(&bridge)
            .await;
        rt.subscribe::<Bridge, web::progress::ScrapeProgress>(&bridge)
            .await;
        rt.subscribe::<Bridge, update::UpdateReady>(&bridge).await;
        rt.subscribe::<Bridge, agent::CommunicationChanged>(&bridge)
            .await;
        rt.subscribe::<Bridge, agent::PeerMessageChanged>(&bridge)
            .await;
        rt.subscribe::<Bridge, agent::PeerDelivered>(&bridge).await;
        rt.subscribe::<Bridge, a2a::A2aChanged>(&bridge).await;
        rt.subscribe::<Bridge, voice::VoiceEvent>(&bridge).await;
        rt.subscribe::<Bridge, ::talk::TalkEvent>(&bridge).await;
        let api: Api = Arc::new(RwLock::new(Arc::new(match &self.attached {
            Some(a) => Engine::Remote(a.client.clone()),
            None => Engine::Local(Local::new(rt.clone())),
        })));
        if let Some(attached) = self.attached {
            let _ = events_tx.send(UiEvent::Engine(Some(attached_label(&attached.running))));
            tokio::spawn(supervise(
                rt.clone(),
                api.clone(),
                events_tx.clone(),
                attached,
            ));
        }
        // Startup facts the runtime does not hand out as config sections: whether
        // the app plugin runs (46 §9) and the engine's live default model and effort
        // (82 §3, 83 §3; config.yaml as read at launch is the fallback when `info` fails).
        let engine = api.read().unwrap().clone();
        let info = engine.info().await.ok();
        let app_enabled = info.as_ref().is_some_and(|i| i.app_enabled);
        tokio::spawn(run_commands(api, commands_rx, events_tx));

        let (default_model, default_effort, advisor, favorite_models) = info.map_or_else(
            || {
                let cfg = rt.section::<AgentConfig>("agent").unwrap_or_default();
                (
                    cfg.model,
                    cfg.effort,
                    Default::default(),
                    cfg.favorite_models,
                )
            },
            |i| {
                (
                    i.default_model,
                    i.default_effort,
                    i.advisor,
                    i.favorite_models,
                )
            },
        );
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
                    default_effort,
                    advisor,
                    favorite_models,
                    approvals,
                    app_enabled,
                    catalog,
                    resume,
                    data_dir,
                };
                // Own task so a panic in the render loop lands here (JoinError)
                // instead of skipping `shutdown.cancel()` and leaving a live
                // process with a dead screen.
                match tokio::spawn(ui::run(links)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::error!("tui render loop failed: {e}"),
                    Err(e) => tracing::error!("tui render loop panicked: {e}"),
                }
            } else {
                if prompt.is_none() {
                    tracing::warn!("stdout is not a terminal; using the line printer");
                }
                line_printer(
                    events_rx,
                    commands_tx,
                    resume,
                    approvals,
                    app_enabled,
                    prompt,
                )
                .await;
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
    AskRequested => Ask,
    AskResolved => AskResolved,
    double::DoublePredicted => DoublePredicted,
    double::DoubleAnswered => DoubleAnswered,
    McpProgress => McpProgress,
    LoginFinished => LoginFinished,
    JobEvent => Job,
    link::LinkEvent => LinkEvent,
    nodes::NodeEvent => NodeEvent,
    artifact::ArtifactShown => ArtifactShown,
    secrets::SecretsEvent => SecretsEvent,
    knowledge::KnowledgeChanged => KnowledgeChanged,
    web::progress::ScrapeProgress => ScrapeProgress,
    update::UpdateReady => UpdateReady,
    agent::CommunicationChanged => CommunicationChanged,
    agent::PeerMessageChanged => PeerMessageChanged,
    agent::PeerDelivered => PeerDelivered,
    a2a::A2aChanged => A2aChanged,
    voice::VoiceEvent => Voice,
    ::talk::TalkEvent => Talk,
);

/// Attached mode's supervisor (63 §2.3 Disconnect): when the socket closes,
/// take the lock and start the engine here, or — another client won — attach
/// to the winner; three attempts, then a plain error. Every switch tells the
/// UI (`Engine`), which re-selects its session on the new end.
async fn supervise(rt: Runtime, api: Api, tx: UnboundedSender<UiEvent>, attached: Attached) {
    // The record's listen until the implicit server binds (63 §6.4).
    let listen = rpc::configured_listen(&rt);
    let mut client = attached.client;
    let mut start_engine = Some(attached.start_engine);
    for attempt in 1..=3 {
        if let Err(e) = rpc::client::mirror(&rt, &client).await {
            tracing::warn!(error = %e, "engine event stream");
        }
        let _ = tx.send(UiEvent::Info("node process gone — starting one…".into()));
        let running = match rpc::engine::acquire(&rt.data_dir, Host::Tui, listen.clone()) {
            Ok(lease) => {
                rpc::engine::hold(lease);
                let Some(start) = start_engine.take() else {
                    return;
                };
                match start(rt.clone()).await {
                    Ok(()) => {
                        *api.write().unwrap() = Arc::new(Engine::Local(Local::new(rt.clone())));
                        let _ = tx.send(UiEvent::Engine(None));
                    }
                    Err(e) => {
                        rpc::engine::release();
                        let _ = tx.send(UiEvent::Error(format!(
                            "node process failed to start: {e:#}"
                        )));
                    }
                }
                return;
            }
            Err(rpc::engine::Error::Running(r)) => r,
            Err(e) => {
                let _ = tx.send(UiEvent::Error(e.to_string()));
                return;
            }
        };
        // The winner just took the lock; give it the desktop's 15 s to listen.
        let mut connected = None;
        for _ in 0..75 {
            match rpc::client::connect(&running).await {
                Ok(c) => {
                    connected = Some(c);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
        let Some(c) = connected else {
            let _ = tx.send(UiEvent::Error(format!(
                "node process gone; {running} but it never answered (attempt {attempt}/3)"
            )));
            return;
        };
        client = Arc::new(c);
        *api.write().unwrap() = Arc::new(Engine::Remote(client.clone()));
        let _ = tx.send(UiEvent::Engine(Some(attached_label(&running))));
    }
    let _ = tx.send(UiEvent::Error(
        "node process gone; could not attach to its successor after 3 attempts".into(),
    ));
}

/// The one place commands become `rpc::Api` calls. Each command runs in its
/// own task so a slow call never delays the next one — except keystrokes
/// (`PtyInput`, 55 §3), which take one serial lane so they reach the
/// process in the order they were typed.
async fn run_commands(api: Api, mut rx: UnboundedReceiver<Command>, tx: UnboundedSender<UiEvent>) {
    let (keys_tx, mut keys_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
    tokio::spawn({
        let (api, tx) = (api.clone(), tx.clone());
        async move {
            while let Some(cmd) = keys_rx.recv().await {
                let api = api.read().unwrap().clone();
                if let Err(e) = execute(&api, cmd, &tx).await {
                    let _ = tx.send(UiEvent::Error(format!("{e:#}")));
                }
            }
        }
    });
    while let Some(cmd) = rx.recv().await {
        if matches!(cmd, Command::PtyInput { .. }) {
            let _ = keys_tx.send(cmd);
            continue;
        }
        let tx = tx.clone();
        let api = api.read().unwrap().clone();
        tokio::spawn(async move {
            if let Err(e) = execute(&api, cmd, &tx).await {
                let _ = tx.send(UiEvent::Error(format!("{e:#}")));
            }
        });
    }
}

async fn execute(api: &Engine, cmd: Command, tx: &UnboundedSender<UiEvent>) -> anyhow::Result<()> {
    let reply = match cmd {
        Command::Resolve { id, decision } => {
            if !msg(api.resolve_approval(id, decision).await)? {
                anyhow::bail!("approval {} was no longer pending", id.0);
            }
            return Ok(());
        }
        Command::AnswerAsk { id, answers } => {
            if !msg(api.answer_ask(id, answers).await)? {
                anyhow::bail!("ask {} was no longer pending", id.0);
            }
            return Ok(());
        }
        Command::Send {
            session,
            text,
            files,
        } => {
            let r = msg(api
                .send_input(session, text.clone(), files, None, None)
                .await);
            if r.is_err() {
                let _ = tx.send(UiEvent::Unsent { id: None, text });
            }
            UiEvent::SessionCreated(r?)
        }
        Command::Steer {
            session,
            id,
            text,
            files,
            interject,
        } => {
            let r: anyhow::Result<()> = async {
                // Cancel first: its epoch bump would stale a steer admitted before it (todo auto-continue).
                if interject {
                    msg(api.cancel(session, None).await)?;
                }
                msg(api
                    .steer(session, text.clone(), Some(id), files, None)
                    .await)?;
                Ok(())
            }
            .await;
            if r.is_err() {
                let _ = tx.send(UiEvent::Unsent { id: Some(id), text });
            }
            return r;
        }
        Command::Withdraw { session, id } => UiEvent::Withdrawn {
            id,
            text: msg(api.withdraw(session, id).await)?,
        },
        // The transcript is reloaded here, not from the UI: commands run
        // concurrently and the notice must land before the new history.
        Command::Undo(session) => {
            let _ = tx.send(UiEvent::Undone(msg(api.undo(session).await)?));
            UiEvent::History(
                session,
                msg(api.load_session(session, None, None, None).await)?,
            )
        }
        Command::Redo(session) => {
            let _ = tx.send(UiEvent::Redone(msg(api.redo(session).await)?));
            UiEvent::History(
                session,
                msg(api.load_session(session, None, None, None).await)?,
            )
        }
        // Same shape as undo: the notice, then the settled transcript (48
        // §5). A refusal or a lost reply also reloads, so the screen never
        // keeps showing content whose removal is unknown.
        Command::RemoveContext {
            session,
            expected_revision,
            targets,
        } => {
            match api
                .remove_context(session, expected_revision, targets)
                .await
            {
                Ok(r) => {
                    let _ = tx.send(UiEvent::ContextRemoved(session, r));
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::Error(reason(e)));
                }
            }
            UiEvent::Refreshed(
                session,
                msg(api.load_session(session, None, None, Some(true)).await)?,
            )
        }
        Command::ContextItems { session, category } => UiEvent::ContextItems {
            session,
            category,
            snapshot: msg(api.context_items(session).await)?,
        },
        // The UI already truncated its transcript at the turn (29 §3); the
        // new turn streams back through ordinary events. On refusal, the
        // error lands first, then the transcript reloads to undo the cut.
        Command::Regenerate { session, turn } => match api.regenerate(session, turn).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = tx.send(UiEvent::Error(reason(e)));
                UiEvent::History(
                    session,
                    msg(api.load_session(session, None, None, None).await)?,
                )
            }
        },
        Command::EditTurn {
            session,
            turn,
            text,
        } => match api.edit_turn(session, turn, text, Vec::new()).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = tx.send(UiEvent::Error(reason(e)));
                UiEvent::History(
                    session,
                    msg(api.load_session(session, None, None, None).await)?,
                )
            }
        },
        Command::Fork { session, through } => {
            let id = msg(api.fork_session(session, through, None).await)?;
            UiEvent::Forked(id, msg(api.load_session(id, None, None, None).await)?)
        }
        Command::Cancel(session) => {
            msg(api.cancel(session, None).await)?;
            return Ok(());
        }
        Command::CancelTree(session) => {
            msg(api.cancel(session, Some(agent::CancelScope::Tree)).await)?;
            return Ok(());
        }
        Command::ListJobs(session) => UiEvent::Jobs(msg(api.list_jobs(session).await)?),
        Command::SetSessionProject { session, project } => {
            msg(api.set_session_project(session, project.clone()).await)?;
            UiEvent::Info(match project {
                Some(p) => format!("project: {p}"),
                None => "project cleared".to_owned(),
            })
        }
        Command::SessionCost(session) => {
            UiEvent::Cost(session, msg(api.session_cost(session).await)?)
        }
        Command::ListTerminals(session) => {
            UiEvent::Terminals(msg(api.list_terminals(session, Some(true)).await)?)
        }
        // Errors ride inside the reply so the observer knows its ask answered.
        Command::PtyScreen {
            session,
            job,
            viewport,
            if_revision,
            epoch,
        } => UiEvent::PtyScreen {
            session,
            job: job.clone(),
            viewport,
            epoch,
            reply: api
                .pty_screen(session, job, viewport, if_revision)
                .await
                .map_err(reason),
        },
        Command::PtyInput {
            session,
            job,
            text,
            keys,
        } => {
            msg(api.pty_input(session, job, text, Some(keys)).await)?;
            return Ok(());
        }
        Command::CreateSession(options) => {
            UiEvent::SessionOpened(msg(api.create_session(options).await)?)
        }
        Command::LeaveSession { session, secret } => UiEvent::SessionLeft(
            session,
            msg(api.leave_session(session, Some(secret)).await)?,
        ),
        Command::ListSchedules => UiEvent::Schedules(msg(api.list_schedules().await)?),
        Command::ListTasks(project) => {
            let filter = tasks::TaskFilter {
                project: project.clone(),
                ..Default::default()
            };
            UiEvent::Tasks(project, msg(api.tasks_list(filter).await)?)
        }
        Command::ListRoster => UiEvent::Roster(msg(api.agents_list().await)?),
        Command::TakeSession(id) => {
            let o = msg(api.session_take(id, "user".into()).await)?;
            UiEvent::Info(format!("taking session here (epoch {})", o.epoch))
        }
        Command::FireSchedule(name) => {
            let id = msg(api.fire_schedule(name.clone()).await)?;
            UiEvent::Info(format!("fired {name} → session {id}"))
        }
        Command::ScheduleHistory(name) => {
            let rows = msg(api.schedule_history(name.clone(), 20).await)?;
            // ponytail: a second round trip for the loop header; fold into
            // `schedule_history` if the RPC ever grows a combined reply.
            let info = msg(api.list_schedules().await)?
                .into_iter()
                .find(|s| s.name == name);
            UiEvent::ScheduleHistory(name, info, rows)
        }
        Command::EnableSchedule(name, enabled) => {
            msg(api.enable_schedule(name.clone(), enabled).await)?;
            UiEvent::Info(format!(
                "{name} {}",
                if enabled { "resumed" } else { "paused" }
            ))
        }
        Command::TestSchedule(name) => {
            let v = msg(api.test_schedule(name).await)?;
            UiEvent::Info(schedule::validate::render(&v))
        }
        // 140 §13.1: the TUI's direct forms render as text (no chart).
        Command::AutoresearchStatus(slug) => {
            let studies = msg(api.autoresearch_list().await)?;
            let mut text = match &slug {
                Some(s) => {
                    let Some(info) = studies.iter().find(|i| &i.slug == s) else {
                        return Err(anyhow::anyhow!("no autoresearch study `{s}`"));
                    };
                    autoresearch::tools::render_info(info)
                }
                None if studies.is_empty() => {
                    "no autoresearch studies (start one: /autoresearch <goal>)".to_owned()
                }
                None => studies
                    .iter()
                    .map(autoresearch::tools::render_info)
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            if let Some(s) = slug {
                let rows = msg(api.autoresearch_experiments(s, None, 10).await)?;
                if !rows.is_empty() {
                    text.push_str("\nLast experiments (newest first):");
                    for e in &rows {
                        text.push('\n');
                        text.push_str(&autoresearch::tools::render_row(e));
                    }
                }
            }
            UiEvent::Info(text)
        }
        Command::AutoresearchControl(slug, op) => {
            let info = msg(api.autoresearch_control(slug, op).await)?;
            UiEvent::Info(autoresearch::tools::render_info(&info))
        }
        Command::AutoresearchSteer(slug, text) => {
            let id = msg(api.autoresearch_steer(slug.clone(), text).await)?;
            UiEvent::Info(format!(
                "steer #{id} queued: the next experiment of `{slug}` reads it; the loop does not pause"
            ))
        }
        Command::ComputerStatus => UiEvent::Computer(msg(api.computer_permissions().await)?),
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
        Command::Memory { session, mode } => match mode {
            Some(mode) => {
                let m = msg(api.set_session_memory(session, mode).await)?;
                UiEvent::Info(agent::endless_ext::reply(mode, &m))
            }
            None => UiEvent::Info(agent::endless_ext::status(&msg(api
                .session_memory(session)
                .await)?)),
        },
        Command::GetCompactStyle => UiEvent::CompactStyle(msg(api.compact_style().await)?),
        Command::SetCompactStyle(style) => {
            msg(api.set_compact_style(style).await)?;
            UiEvent::Info(format!("compact style: {style}"))
        }
        Command::GetTitleModel => UiEvent::TitleModel(msg(api.title_model().await)?),
        Command::SetTitleModel(model) => {
            UiEvent::TitleModel(msg(api.set_title_model(model).await)?)
        }
        Command::GetVisionModel => UiEvent::VisionModel(msg(api.vision_model().await)?),
        Command::SetVisionModel(model) => {
            UiEvent::VisionModel(msg(api.set_vision_model(model).await)?)
        }
        Command::ListAwsProfiles => UiEvent::AwsProfiles {
            info: msg(api.list_aws_profiles().await)?,
            pick: true,
        },
        Command::SetAwsProfile(profile) => UiEvent::AwsProfiles {
            info: msg(api.set_aws_profile(profile).await)?,
            pick: false,
        },
        Command::ListSessions => UiEvent::Sessions(msg(api.list_sessions().await)?),
        // Every search reply carries the view generation that asked, errors
        // included, so a superseded request can never land (49 §6.1).
        Command::Search(search::Request { generation, kind }) => {
            UiEvent::Search(generation, search_request(api, kind).await)
        }
        // Failures show nothing: the list simply does not open (PATH-AC).
        Command::PathComplete {
            session,
            input,
            cursor,
            generation,
        } => UiEvent::PathComplete(
            generation,
            api.path_complete(session, input, cursor).await.ok(),
        ),
        Command::ListAgents(session) => UiEvent::Agents(msg(api.list_agents(session).await)?),
        Command::ListActivity => UiEvent::Activity(msg(api.list_activity().await)?),
        // The menu shows the refusal on its error line; not an `Error` entry.
        Command::Broadcast(req) => UiEvent::Broadcasted(api.broadcast(req).await.map_err(reason)),
        // A failed read must not leave the card looking current: the plan is
        // marked stale first, then the error line explains why (47 §5.4).
        Command::GetTodos(session) => match msg(api.get_todos(session).await) {
            Ok(s) => UiEvent::Todos(session, Some(s)),
            Err(e) => {
                let _ = tx.send(UiEvent::Todos(session, None));
                return Err(e);
            }
        },
        Command::Load {
            session,
            settled_only,
        } => {
            let loaded = msg(api
                .load_session(session, None, None, Some(settled_only))
                .await)?;
            if settled_only {
                UiEvent::Refreshed(session, loaded)
            } else {
                UiEvent::History(session, loaded)
            }
        }
        Command::ListModels => UiEvent::Models(msg(api.list_models().await)?),
        Command::SetModel { session, model } => {
            // A pick in this TUI's picker or `/model`: manual, so also the default (82 §2).
            UiEvent::ModelSet(msg(api.set_model(session, model, Some(true)).await)?)
        }
        Command::SetEffort { session, effort } => {
            // A pick in this TUI's picker or `/effort`: manual, so also the default (83 §2).
            msg(api.set_effort(session, effort.clone(), Some(true)).await)?;
            UiEvent::EffortSet(effort)
        }
        // 125 §7.1: `None` model = off; both replies carry the same status.
        Command::SetAdvisor {
            session,
            model,
            effort,
        } => UiEvent::AdvisorStatus(msg(api.set_advisor(session, model, effort).await)?),
        Command::AdvisorStatus(session) => {
            UiEvent::AdvisorStatus(msg(api.advisor_status(session).await)?)
        }
        // R1.2: `advisor consults → n`; the live status follows on `AdvisorChanged`.
        Command::SetAdvisorMaxConsults(n) => {
            let d = msg(api.set_advisor_max_consults(n).await)?;
            UiEvent::Info(format!("advisor consults → {}", d.max_consults))
        }
        Command::SetModelFavorite { model, favorite } => {
            UiEvent::FavoriteModels(msg(api.set_model_favorite(model, favorite).await)?)
        }
        Command::HarnessOptions(session) => {
            UiEvent::HarnessOptions(msg(api.harness_options(session).await)?)
        }
        Command::SetHarnessOption {
            session,
            config_id,
            value,
        } => {
            let r = msg(api
                .set_harness_option(session, config_id.clone(), value.clone())
                .await)?;
            UiEvent::HarnessOptionSet(config_id, value, r)
        }
        Command::ResumeChild(session) => {
            // 146 D8: a child that died mid-prompt continues its task.
            let text = match msg(api.resume_child(session).await)? {
                agent::supervisor::ChildResumed::Prompted => {
                    "harness resumed; resuming the interrupted task"
                }
                agent::supervisor::ChildResumed::Idle => {
                    "harness reconnected in its previous context; it was idle, nothing to continue — send a message"
                }
            };
            UiEvent::Info(text.into())
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
        Command::Double(cmd) => UiEvent::Info(match cmd {
            DoubleCmd::Status => double_status_line(&msg(api.double_status().await)?),
            DoubleCmd::Off => {
                let mut cfg = msg(api.double_settings_get().await)?;
                cfg.enabled = false;
                msg(api.double_settings_set(cfg).await)?;
                "double: disabled in config.yaml — takes effect at the next engine start".into()
            }
            DoubleCmd::Pause(on) => {
                let s = msg(api.double_pause(on).await)?;
                format!(
                    "double: learning {}; {}",
                    if s.paused { "paused" } else { "resumed" },
                    double_status_line(&s)
                )
            }
            DoubleCmd::Train { tier, venue } => format!(
                "double: training run {} started (tier {}, venue {})",
                msg(api.double_train(tier, venue.clone()).await)?.0,
                tier.as_str(),
                venue.as_config()
            ),
            DoubleCmd::Rollback(v) => double_status_line(&msg(api.double_rollback(v).await)?),
            DoubleCmd::Cap {
                class,
                rung,
                opt_in,
            } => {
                let c = msg(api.double_set_cap(class, rung, opt_in).await)?;
                format!(
                    "double: {} cap L{}{}{}",
                    c.key,
                    c.cap,
                    c.cap_reason
                        .as_deref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default(),
                    if c.mutating_opt_in {
                        " · mutating opt-in"
                    } else {
                        ""
                    }
                )
            }
        }),
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
        Command::KnowledgeHistory { id, limit } => {
            let fetch = u32::try_from(limit + 1).unwrap_or(u32::MAX);
            UiEvent::KnowledgeHistory {
                history: msg(api.knowledge_history(id.clone(), Some(fetch)).await)?,
                id,
                limit,
            }
        }
        Command::KnowledgeRevision { commit, id, card } => UiEvent::KnowledgeRevision {
            revision: msg(api.knowledge_revision(commit, id.clone()).await)?,
            id,
            card,
        },
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
        Command::LinkStatus => UiEvent::Link(msg(api.link_status().await)?),
        Command::LinkEnable(enabled) => {
            msg(api.link_enable(enabled, None).await)?;
            UiEvent::Link(msg(api.link_status().await)?)
        }
        Command::LinkRelay(url) => {
            let enabled = msg(api.link_status().await)?.enabled;
            msg(api.link_enable(enabled, Some(url)).await)?;
            UiEvent::Link(msg(api.link_status().await)?)
        }
        Command::LinkPair => UiEvent::Pairing(msg(api.link_pair().await)?),
        Command::LinkCancelPair => {
            msg(api.link_cancel_pair().await)?;
            UiEvent::Info("pairing cancelled".into())
        }
        Command::LinkRename(name) => {
            msg(api.link_rename(name).await)?;
            UiEvent::Link(msg(api.link_status().await)?)
        }
        Command::LinkRenameDevice(id, name) => {
            msg(api.link_rename_device(id, name).await)?;
            UiEvent::Link(msg(api.link_status().await)?)
        }
        Command::LinkRevoke(id) => {
            msg(api.link_revoke_device(id).await)?;
            UiEvent::Link(msg(api.link_status().await)?)
        }
        Command::MeshDevicesList => UiEvent::MeshDevices(msg(api.devices_list().await)?),
        Command::SandboxList => UiEvent::Sandboxes(msg(api.sandboxes_list().await)?),
        Command::SandboxDesktop(name) => {
            UiEvent::SandboxDesktop(name.clone(), msg(api.sandbox_desktop_access(name).await)?)
        }
        Command::MeshDevicesAdd(name) => {
            UiEvent::MeshDevicePairing(msg(api.device_pair_start(name).await)?)
        }
        Command::MeshDevicesRevoke(id) => {
            msg(api.device_revoke(id.clone()).await)?;
            let _ = tx.send(UiEvent::Info(format!("revoked device {id}")));
            UiEvent::MeshDevices(msg(api.devices_list().await)?)
        }
        Command::MeshDevicesRename(id, name) => {
            msg(api.device_rename(id.clone(), name.clone()).await)?;
            let _ = tx.send(UiEvent::Info(format!("renamed device {id} → {name}")));
            UiEvent::MeshDevices(msg(api.devices_list().await)?)
        }
        Command::ListNodes => UiEvent::Nodes(msg(api.list_nodes().await)?),
        Command::AddNode(key, name) => {
            let node = msg(api.add_node(key, name).await)?;
            let _ = tx.send(UiEvent::Info(format!("paired node {}", node.name)));
            UiEvent::Nodes(msg(api.list_nodes().await)?)
        }
        Command::RemoveNode(n) => {
            let node = resolve_node(api, &n).await?;
            msg(api.remove_node(node.id).await)?;
            let _ = tx.send(UiEvent::Info(format!("removed node {}", node.name)));
            UiEvent::Nodes(msg(api.list_nodes().await)?)
        }
        Command::RenameNode(n, name) => {
            let node = resolve_node(api, &n).await?;
            msg(api.rename_node(node.id, name.clone()).await)?;
            let _ = tx.send(UiEvent::Info(format!(
                "renamed node {} → {name}",
                node.name
            )));
            UiEvent::Nodes(msg(api.list_nodes().await)?)
        }
        Command::InviteNode(n) => {
            let node = resolve_node(api, &n).await?;
            UiEvent::NodeInvite(node.name, msg(api.invite_node(node.id).await)?)
        }
        Command::ListArtifacts {
            session,
            pick,
            apps,
        } => {
            let mut items = msg(api.list_artifacts(Some(session), Some(true)).await)?;
            if apps {
                items.retain(|a| a.kind == artifact::Kind::App);
            }
            // The picker groups by the owners' project (51 §3); cards need no join.
            let sessions = if pick && !items.is_empty() {
                msg(api.list_sessions().await)?
            } else {
                Vec::new()
            };
            UiEvent::Artifacts {
                items,
                pick,
                apps,
                sessions,
            }
        }
        Command::ExportArtifact { id, name } => {
            let dest = std::env::home_dir()
                .ok_or_else(|| anyhow::anyhow!("no home directory"))?
                .join("Downloads")
                .join(name);
            let path = msg(api.export_artifact(id, dest).await)?;
            UiEvent::Info(format!("exported → {}", path.display()))
        }
        Command::ListSecrets => UiEvent::Secrets(msg(api.list_secrets().await)?),
        Command::SetSecret { name, value, note } => {
            UiEvent::SecretSet(msg(api.set_secret(name, value, note).await)?)
        }
        Command::RestoreSecret { name, back } => {
            UiEvent::SecretSet(msg(api.restore_secret(name, back).await)?)
        }
        Command::DeleteSecret(name) => {
            msg(api.delete_secret(name.clone()).await)?;
            UiEvent::Info(format!("{name}: deleted"))
        }
        Command::AnswerSecret { id, value } => {
            msg(api.answer_secret(id, value, None).await)?;
            return Ok(());
        }
        // Detached, like `computer_open`: the TUI runs on the harness host, so the path is local.
        Command::OpenPath(path) => {
            platform::open::open_path(&path)
                .map_err(|e| anyhow::anyhow!("could not open {}: {e}", path.display()))?;
            return Ok(());
        }
        Command::GetCommunication(session) => {
            UiEvent::Communication(msg(api.get_communication(session).await)?)
        }
        // The shown session's view, not the switched one's: a global flip
        // is read back through the session it affects.
        Command::SetCommunication {
            session,
            enabled,
            view,
        } => {
            let state = msg(api.set_communication(session, enabled).await)?;
            if session == view {
                UiEvent::Communication(state)
            } else {
                UiEvent::Communication(msg(api.get_communication(view).await)?)
            }
        }
        Command::SetPeerScope { root, scope, alias } => {
            UiEvent::Communication(msg(api.set_peer_scope(root, scope, alias).await)?)
        }
        // Off is a reason, not an empty page: it lands in the menu.
        Command::ListPeers(session) => UiEvent::Peers(
            session,
            api.list_peers(session, None, None, None)
                .await
                .map_err(reason),
        ),
        Command::ListPeerRequests(session) => UiEvent::PeerRequests(
            session,
            msg(api.list_peer_messages(session, None, None, true).await)?,
        ),
        Command::AnswerPeerRequest {
            session,
            reply_to,
            text,
        } => UiEvent::PeerAnswered(msg(api.answer_peer_request(session, reply_to, text).await)?),
        Command::A2aStatus => UiEvent::A2a(msg(api.a2a_status().await)?),
        Command::ReloadA2a => UiEvent::A2a(msg(api.reload_a2a().await)?),
        // The reply carries the state even when `DictationModel` was published before this bridge subscribed.
        Command::DictationPrewarm => UiEvent::Voice(voice::VoiceEvent::DictationModel {
            state: msg(api.dictation_prewarm().await)?,
        }),
        Command::DictationStart => match api.dictation_start(None).await {
            Ok(id) => UiEvent::DictationStarting(id),
            Err(e) => UiEvent::DictationFailed(reason(e)),
        },
        Command::DictationStop(id) => {
            msg(api.dictation_stop(id).await)?;
            return Ok(());
        }
        Command::TalkStart(session) => start_talk(api, session, tx).await,
        Command::TalkStop(id) if talk_sim() => UiEvent::Talk(::talk::TalkEvent::TalkFinished {
            id,
            reason: ::talk::EndReason::Stopped,
        }),
        Command::TalkStop(id) => {
            msg(api.talk_stop(id).await)?;
            return Ok(());
        }
    };
    let _ = tx.send(reply);
    Ok(())
}

/// `/talk` (76 §5.1): with no session, create one first — the model adopts it
/// through `SessionOpened` before the talk id arrives — then `talk_start` with
/// `sink: device`. `THEUTH_TALK_SIM` set: the scripted [`talk::sim`] stands in
/// for the engine (the brief's development source).
async fn start_talk(
    api: &Engine,
    session: Option<SessionId>,
    tx: &UnboundedSender<UiEvent>,
) -> UiEvent {
    let session = match session {
        Some(s) => s,
        None => match api
            .create_session(agent::CreateSessionOptions::default())
            .await
        {
            Ok(open) => {
                let s = open.session;
                let _ = tx.send(UiEvent::SessionOpened(open));
                s
            }
            Err(e) => return UiEvent::TalkFailed(reason(e)),
        },
    };
    if talk_sim() {
        let id = ::talk::TalkId::new();
        tokio::spawn(run_talk_sim(id, session, tx.clone()));
        return UiEvent::TalkStarting(id);
    }
    match api.talk_start(session, ::talk::Sink::Device, None).await {
        Ok(id) => UiEvent::TalkStarting(id),
        Err(e) => UiEvent::TalkFailed(reason(e)),
    }
}

/// `THEUTH_TALK_SIM` set: `/talk` plays [`talk::sim`] instead of calling the engine.
fn talk_sim() -> bool {
    std::env::var_os("THEUTH_TALK_SIM").is_some()
}

/// Feed the scripted talk into the UI at its own pace; stops when the UI is gone.
async fn run_talk_sim(id: ::talk::TalkId, session: SessionId, tx: UnboundedSender<UiEvent>) {
    for (ms, ev) in talk::sim(id, session) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        if tx.send(ev).is_err() {
            return;
        }
    }
}

/// One `/search` request against the API (49 §6–7). Failures are the reply's
/// `Err`, never a bare `UiEvent::Error`: the menu needs the identity.
async fn search_request(api: &Engine, kind: search::RequestKind) -> search::Reply {
    use search::{Reply, RequestKind};
    match kind {
        RequestKind::Results {
            query,
            categories,
            limit,
        } => Reply::Results {
            result: api
                .search(query, Some(categories.clone()), Some(limit))
                .await
                .map_err(reason),
            categories,
            limit,
        },
        RequestKind::Recent => Reply::Recent(api.list_sessions().await.map_err(reason)),
        RequestKind::Refresh(categories) => {
            Reply::Refreshed(api.search_refresh(Some(categories)).await.map_err(reason))
        }
        RequestKind::Artifact { call_id, query } => {
            let artifact = match api.get_artifact(call_id.clone()).await {
                Ok(a) => a,
                Err(e) => return Reply::Artifact(Err(reason(e))),
            };
            // Text-like kinds (and an archive's listing sidecar) are read here,
            // off the UI thread (49 §6.2).
            let bytes = match model::artifact_read_path(&artifact) {
                Some(p) => tokio::fs::read(p).await.map_err(|e| e.to_string()),
                None => Ok(Vec::new()),
            };
            let matched = match query {
                Some(q) => Some(api.artifact_search_match(call_id, q).await.map_err(reason)),
                None => None,
            };
            Reply::Artifact(Ok(search::ArtifactLoad {
                artifact,
                bytes,
                matched,
            }))
        }
        RequestKind::Concept(id) => Reply::Concept(api.knowledge_get(id).await.map_err(reason)),
        RequestKind::Historical { id, commit } => Reply::Historical(
            api.knowledge_read_revision(id, commit)
                .await
                .map_err(reason),
        ),
        RequestKind::Revision { commit, id } => Reply::Revision(
            api.knowledge_revision(commit, Some(id))
                .await
                .map_err(reason),
        ),
        RequestKind::Page { session, seq } => Reply::Page {
            session,
            seq,
            result: api
                .load_session(
                    session,
                    Some(seq.saturating_add(1)),
                    Some(search::PAGE),
                    Some(false),
                )
                .await
                .map_err(reason),
        },
    }
}

/// Keep only the message: the JSON-RPC code is not user-facing.
fn msg<T>(r: Result<T, ClientError>) -> anyhow::Result<T> {
    r.map_err(|e| anyhow::anyhow!("{}", reason(e)))
}

/// `/double status`: one line from `double_status` (126 §13).
fn double_status_line(s: &double::DoubleStatus) -> String {
    format!(
        "double: {}{} · version {} · venue {} · 30 d: n={} acc {:.0} % (baseline {:.0} %) ece {:.3} · asks avoided {} · {:.0} min saved{}",
        if s.enabled { "on" } else { "off" },
        if s.paused { " (learning paused)" } else { "" },
        s.active_version.as_deref().unwrap_or("baseline"),
        s.venue,
        s.kpis.n,
        s.kpis.accuracy * 100.0,
        s.baseline.accuracy * 100.0,
        s.kpis.ece,
        s.kpis.asks_avoided,
        s.kpis.minutes_saved,
        s.next_window
            .as_deref()
            .map(|w| format!(" · next window {w}"))
            .unwrap_or_default()
    )
}

/// The user-facing text of a failed call: the harness's message for a
/// refusal, the transport's for a lost socket. An `OwnerUnreachable` refusal
/// (68 §5) becomes the owner-offline sentence with `/take` as the action.
fn reason(e: ClientError) -> String {
    match e {
        ClientError::Call(e) => match model::owner_unreachable(e.data().map(|d| d.get())) {
            Some((owner, last_seen)) => model::owner_offline_line(&owner, &last_seen),
            None => e.message().to_owned(),
        },
        e => e.to_string(),
    }
}

/// The node `/nodes <verb> <n>` names (exact name, else a unique id prefix).
async fn resolve_node(api: &Engine, n: &str) -> anyhow::Result<nodes::Node> {
    let list = msg(api.list_nodes().await)?;
    model::find_node(&list, n)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no node named {n}"))
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
    app_enabled: bool,
    prompt: Option<String>,
) {
    let mut model = Model::new(String::new(), approvals);
    model.app_enabled = app_enabled;
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
                let search_reply = matches!(ev, UiEvent::Search(..));
                let done = print_mode && match &ev {
                    UiEvent::Agent(AgentEvent::TurnFinished { session, .. }) => model.session == Some(*session),
                    UiEvent::Error(_) => true,
                    _ => false,
                };
                model.apply(ev);
                if std::mem::take(&mut model.reselect)
                    && let Some(session) = model.session
                {
                    let _ = commands.send(Command::Load {
                        session,
                        settled_only: false,
                    });
                }
                model.search_tick(model::now_ms(), &commands);
                // `-p "/search …"` ends once every selected group has settled
                // (an indexing owner is re-asked first, like the menu).
                if done || (print_mode && search_reply && model.search_wake().is_none()) { break; }
                if !stdin_open && model.approval().is_some() {
                    let _ = writeln!(out, "denied: stdin closed");
                    model.decide(Decision::Deny, &commands);
                }
            }
            // A due search request (the `/search` debounce or indexing poll)
            // fires here too: this loop has no render ticks.
            _ = tokio::time::sleep(std::time::Duration::from_millis(
                model.search_wake().map_or(0, |t| t.saturating_sub(model::now_ms())).max(1),
            )), if model.search_wake().is_some() => {
                model.search_tick(model::now_ms(), &commands);
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

/// A loaded transcript as lines. A row removed from context (48 §2.2)
/// prints as one `⊘` marker over its first line — the archive is not
/// replayed — and attachment occurrences print by name.
fn print_history(out: &mut impl Write, loaded: &agent::Loaded) -> std::io::Result<()> {
    let labels = |c: &Option<agent::ContextEntry>| -> String {
        c.as_ref().map_or_else(String::new, |c| {
            c.attachments
                .iter()
                .map(|a| format!("\n{}", model::attachment_label(a)))
                .collect()
        })
    };
    let removed = |c: &Option<agent::ContextEntry>| c.as_ref().is_some_and(|c| c.removed);
    loaded.history.iter().try_for_each(|h| match h {
        // A consultation note (52 §6) is harness-authored: one header line,
        // never echoed as the human's `>` prompt or the assistant's reply.
        agent::HistoryEntry::User {
            vision: Some(note),
            context,
            ..
        } => {
            let title = model::note_title(note);
            if removed(context) {
                writeln!(out, "⊘ removed from context: ◉ {title}")
            } else {
                writeln!(out, "◉ {title}")
            }
        }
        agent::HistoryEntry::User { text, context, .. } if removed(context) => writeln!(
            out,
            "⊘ removed from context: > {}",
            text.lines().next().unwrap_or_default()
        ),
        agent::HistoryEntry::Assistant { text, context, .. } if removed(context) => writeln!(
            out,
            "⊘ removed from context: {}",
            text.lines().next().unwrap_or_default()
        ),
        agent::HistoryEntry::Tool { name, context, .. } if removed(context) => {
            writeln!(out, "⊘ removed from context: ▸ {name}")
        }
        agent::HistoryEntry::Compacted { epoch, context, .. } if removed(context) => {
            writeln!(out, "⊘ removed from context: summary (epoch {epoch})")
        }
        agent::HistoryEntry::User { text, context, .. } => {
            writeln!(out, "> {}{}", attach::display(text), labels(context))
        }
        agent::HistoryEntry::Assistant { text, .. } => writeln!(out, "{text}"),
        agent::HistoryEntry::Reasoning { text, .. } => writeln!(out, "\x1b[2m┆ {text}\x1b[0m"),
        agent::HistoryEntry::Tool {
            name,
            args,
            result,
            context,
            ..
        } => writeln!(
            out,
            "▸ {name}\n{}{}\n{}",
            model::describe(name, args).join("\n"),
            labels(context),
            match result {
                Some((output, is_error)) => format!(
                    "{} {}",
                    if *is_error { "✗" } else { "✓" },
                    model::result_line(output)
                ),
                None => "… running".to_owned(),
            }
        ),
        agent::HistoryEntry::Compacted {
            epoch,
            summary,
            style,
            ..
        } => {
            writeln!(
                out,
                "── context compacted (epoch {epoch} · {style}) ──\n{summary}"
            )
        }
        agent::HistoryEntry::Error { text, .. } => writeln!(out, "error: {text}"),
        agent::HistoryEntry::Interrupted { text, .. } => {
            writeln!(out, "── interrupted — resuming ──\n{text}")
        }
        // 125 §7.4: one line per note, the card's title first.
        agent::HistoryEntry::Advisory { advisory, .. } => {
            writeln!(out, "{}", model::advisory_title(advisory))?;
            model::advisory_lines(advisory)
                .iter()
                .try_for_each(|l| writeln!(out, "{l}"))
        }
    })
}

fn print_event(out: &mut impl Write, model: &Model, ev: &UiEvent) {
    // Thinking streams dim under a `┆` header (70 THINKING-TRACES); anything else closes the block.
    let thinking = matches!(
        model.entries.last(),
        Some(model::Entry::Reasoning { done: false, .. })
    );
    if thinking && !matches!(ev, UiEvent::Agent(AgentEvent::ReasoningDelta { .. })) {
        let _ = writeln!(out, "\x1b[0m");
    }
    let _ = match ev {
        UiEvent::Agent(AgentEvent::ReasoningDelta { text, .. }) => {
            let head = if thinking { "" } else { "\x1b[2m┆ " };
            write!(out, "{head}{text}").and_then(|()| out.flush())
        }
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
                "\n▸ {}{name}\n{}",
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
        UiEvent::Agent(AgentEvent::AdvisoryAdded { advisory, .. }) => {
            writeln!(out, "{}", model::advisory_title(advisory)).and_then(|()| {
                model::advisory_lines(advisory)
                    .iter()
                    .try_for_each(|l| writeln!(out, "{l}"))
            })
        }
        UiEvent::Agent(AgentEvent::AdvisorChanged { status, .. }) => {
            writeln!(out, "{}", model::advisor_changed_line(status))
        }
        UiEvent::Agent(AgentEvent::DefaultModelChanged { model }) => {
            writeln!(out, "default model → {model}")
        }
        UiEvent::Agent(AgentEvent::FavoriteModelsChanged { models }) => {
            writeln!(out, "favorites → {} models", models.len())
        }
        UiEvent::Agent(AgentEvent::DefaultEffortChanged { effort }) => {
            writeln!(
                out,
                "default effort → {}",
                effort.as_deref().unwrap_or("unset")
            )
        }
        UiEvent::Agent(AgentEvent::HarnessModelChanged { model, .. }) => {
            writeln!(out, "harness model → {model}")
        }
        UiEvent::Agent(AgentEvent::HarnessCommandsChanged { commands, .. }) => {
            writeln!(out, "harness commands → {}", commands.len())
        }
        UiEvent::Agent(AgentEvent::Compacted { epoch, style, .. }) => {
            writeln!(out, "context compacted (epoch {epoch} · {style})")
        }
        // 109 §10.3: the divider, then the counts once the batch is reassembled.
        UiEvent::Agent(AgentEvent::TurnResumed { kind, .. }) => {
            writeln!(out, "── resumed after restart ({kind:?}) ──")
        }
        UiEvent::Agent(AgentEvent::ResumeReport {
            replayed,
            interrupted,
            lost_jobs,
            ..
        }) => writeln!(
            out,
            "resumed · {} replayed · {} skipped · {} job(s) lost",
            replayed.len(),
            interrupted.len(),
            lost_jobs.len()
        ),
        UiEvent::Agent(AgentEvent::Harvested { added, updated, .. }) => {
            writeln!(out, "knowledge +{added} ~{updated}")
        }
        UiEvent::Agent(AgentEvent::Error { message, .. }) | UiEvent::Error(message) => {
            writeln!(out, "error: {message}")
        }
        UiEvent::Agent(AgentEvent::ContextChanged { reason, .. }) => {
            writeln!(out, "context changed ({reason})")
        }
        // A consultation step (52 §6): one status line; auxiliary question
        // text never streams, so nothing here can pass for the reply.
        UiEvent::Agent(AgentEvent::VisionProgress {
            stage,
            model,
            sources,
            ..
        }) => writeln!(
            out,
            "◉ {}",
            model::vision_line(*stage, model, sources.len())
        ),
        UiEvent::Agent(_) => Ok(()),
        // An app is a pointer to its source here: it runs on desktop/web (46 §8.2).
        UiEvent::ArtifactShown(ev) if ev.artifact.kind == artifact::Kind::App => writeln!(
            out,
            "[app: {} · {} → source {}]",
            ev.artifact.name,
            model::APP_WHERE,
            ev.artifact.path.display()
        ),
        UiEvent::ArtifactShown(ev) => writeln!(
            out,
            "[artifact: {} → {}]",
            ev.artifact.name,
            ev.artifact.path.display()
        ),
        UiEvent::ScrapeProgress(_) => Ok(()),
        UiEvent::Artifacts { items, apps, .. } if items.is_empty() => {
            writeln!(out, "{}", if *apps { "no apps" } else { "no artifacts" })
        }
        UiEvent::Artifacts { items, .. } => items
            .iter()
            .try_for_each(|a| writeln!(out, "{}", model::artifact_row(a))),
        UiEvent::ModelSet(info) => writeln!(out, "model → {}", info.id),
        UiEvent::EffortSet(Some(e)) => writeln!(out, "effort: {e}"),
        UiEvent::EffortSet(None) => writeln!(out, "effort cleared"),
        UiEvent::AdvisorStatus(s) => model::advisor_status_lines(s)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        UiEvent::FavoriteModels(models) => writeln!(out, "favorites → {} models", models.len()),
        UiEvent::HarnessOptions(options) => model::harness_options_lines(options)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        UiEvent::HarnessOptionSet(key, value, r) => {
            writeln!(out, "{}", model::harness_option_set_line(key, value, r))
        }
        UiEvent::Models(_) | UiEvent::Mcp(_) | UiEvent::PathComplete(..) => {
            writeln!(out, "menus need a TTY")
        }
        UiEvent::Logins(rows) => rows
            .iter()
            .try_for_each(|r| writeln!(out, "{}  {}", r.name, r.status)),
        UiEvent::LoginFinished(f) => match &f.error {
            None => writeln!(out, "login: {} ok", f.provider),
            Some(e) => writeln!(out, "login: {} failed: {e}", f.provider),
        },
        UiEvent::Prompt(text) | UiEvent::Info(text) => writeln!(out, "{text}"),
        UiEvent::Jobs(jobs) => writeln!(out, "{}", tools::jobs::table(jobs)),
        UiEvent::Schedules(rows) => writeln!(out, "{}", model::schedules_table(rows)),
        UiEvent::Tasks(project, rows) => {
            writeln!(out, "{}", model::tasks_table(project.as_deref(), rows))
        }
        UiEvent::Roster(rows) => writeln!(out, "{}", model::agents_table(rows)),
        UiEvent::ScheduleHistory(name, info, rows) => {
            writeln!(
                out,
                "{}",
                model::schedule_history_table(name, info.as_ref(), rows)
            )
        }
        UiEvent::Computer(status) => writeln!(out, "{}", model::computer_line(status)),
        UiEvent::Job(_) => Ok(()),
        UiEvent::Withdrawn { text, .. } => writeln!(out, "withdrawn: {text}"),
        // The `Error` line that follows names the failure; there is no editor to refill.
        UiEvent::Unsent { .. } => Ok(()),
        UiEvent::Undone(text) => writeln!(out, "undone: {text} — /redo restores"),
        UiEvent::Redone(text) => writeln!(out, "restored: {text}"),
        UiEvent::Forked(id, _) => writeln!(out, "forked: {id}"),
        UiEvent::CompactStyle(s) => writeln!(
            out,
            "compact style: {s} (default)\n  summary  cache-hot LLM fold (07 §8.3)\n  cut      drop the folded span, keep the tail, no model call\n  snap     rasterize the folded span to images (vision model)"
        ),
        UiEvent::TitleModel(m) => writeln!(out, "{}", model::title_model_line(m.as_ref())),
        UiEvent::VisionModel(m) => writeln!(out, "{}", model::vision_model_line(m.as_ref())),
        UiEvent::AwsProfiles { info, .. } => writeln!(out, "{}", model::aws_profile_line(info)),
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
        UiEvent::KnowledgeHistory { history: h, .. } => h
            .changes
            .iter()
            .try_for_each(|c| writeln!(out, "{}  {}  {}", c.commit, c.time, c.message))
            .and_then(|()| match &h.diff {
                Some(d) => writeln!(out, "{d}"),
                None => Ok(()),
            }),
        UiEvent::KnowledgeRevision { revision: r, .. } => writeln!(
            out,
            "{}  {}  {}  parent {}\n{}",
            r.change.commit,
            r.change.time,
            r.change.message,
            r.parent.as_deref().unwrap_or("none"),
            model::revision_rows(r).join("\n")
        ),
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
        UiEvent::DoublePredicted(p) => {
            writeln!(out, "{}", model::double_chip(p, model::now_ms()))
        }
        UiEvent::DoubleAnswered(a) => writeln!(out, "{}", double::chip::answered(a)),
        UiEvent::Ask(req) => req.questions.iter().try_for_each(|q| {
            let opts = q.options.iter().enumerate().map(|(i, c)| {
                let rec = if q.recommended == Some(i) {
                    "  (recommended)"
                } else {
                    ""
                };
                format!("\n  {}. {}{rec}", i + 1, c.label)
            });
            writeln!(
                out,
                "ask_user {}: {}{}",
                q.id,
                q.question,
                opts.collect::<String>()
            )
        }),
        UiEvent::AskResolved(_) => Ok(()),
        UiEvent::Sessions(list) => list.iter().try_for_each(|s| {
            let chip = if s.here {
                String::new()
            } else {
                model::engine_chip(&s.owner)
            };
            writeln!(out, "{}  {}  {chip:<8}  {}", s.id, s.model, s.title)
        }),
        // Real output groups with state, coverage and exact identities (49
        // §6.2); an interactive chip set is not on offer here.
        UiEvent::Search(_, reply) => search::print_lines(reply)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        UiEvent::KnowledgeChanged(_) => Ok(()),
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
        UiEvent::Activity(list) => list.iter().try_for_each(|a| {
            writeln!(
                out,
                "{}{}  {}  j{} p{}  {}  {}",
                "  ".repeat(usize::from(a.depth)),
                a.name,
                a.activity.state.word(),
                a.activity.jobs,
                a.activity.ptys,
                broadcast::age(a.activity.last_active),
                model::short(a.id)
            )
        }),
        UiEvent::Broadcasted(Err(e)) => writeln!(out, "broadcast: {e}"),
        UiEvent::Broadcasted(Ok(r)) => broadcast::result_lines(r)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        UiEvent::History(_, loaded) => print_history(out, loaded),
        // A busy session's settled reload is refused (48 §5); the model
        // keeps what it shows and says so.
        UiEvent::Refreshed(_, loaded) if !loaded.settled => writeln!(
            out,
            "{} — transcript refreshes when it settles",
            loaded
                .context_reason
                .as_deref()
                .unwrap_or("session is busy")
        ),
        UiEvent::Refreshed(_, loaded) => print_history(out, loaded),
        // Interactive selection needs a TTY; the inventory itself still prints.
        UiEvent::ContextItems { snapshot, .. } => {
            writeln!(out, "menus need a TTY").and_then(|()| {
                snapshot.items.iter().try_for_each(|i| {
                    writeln!(
                        out,
                        "{:<8} {}{}",
                        model::kind_name(i.kind),
                        i.label,
                        match i.remove_with.len() {
                            0 => String::new(),
                            n => format!(" · +{n} message(s)"),
                        }
                    )
                })
            })
        }
        UiEvent::ContextRemoved(_, r) => writeln!(out, "{}", model::removal_line(r)),
        UiEvent::SessionCreated(id) => writeln!(out, "session {id}"),
        UiEvent::Providers(list) => list
            .iter()
            .try_for_each(|p| writeln!(out, "{}  {}  {} models", p.name, p.base_url, p.models)),
        UiEvent::ProviderAdded(p) => writeln!(
            out,
            "provider {}  {}  {} models — /model {}/<id>",
            p.name, p.base_url, p.models, p.name
        ),
        UiEvent::Link(s) => writeln!(out, "{}", model::link_line(s)),
        UiEvent::Pairing(p) => writeln!(out, "{}\n{}", link::qr(&p.key), p.key),
        UiEvent::LinkEvent(link::LinkEvent::LinkPaired { device }) => {
            writeln!(out, "paired: {}", device.name)
        }
        UiEvent::LinkEvent(_) => Ok(()),
        UiEvent::UpdateReady(_) => Ok(()),
        UiEvent::Nodes(list) => list.iter().try_for_each(|n| {
            writeln!(
                out,
                "{} {}  {}  theuth {}  {}",
                if n.online { "●" } else { "○" },
                n.name,
                n.platform,
                n.version,
                if n.revoked {
                    "revoked"
                } else if n.online {
                    "online"
                } else {
                    "offline"
                }
            )
        }),
        UiEvent::NodeInvite(name, p) => {
            writeln!(out, "node invite · {name}\n{}\n{}", link::qr(&p.key), p.key)
        }
        UiEvent::Sandboxes(list) if list.is_empty() => writeln!(out, "sandboxes: none"),
        UiEvent::Sandboxes(list) => list
            .iter()
            .try_for_each(|i| writeln!(out, "{}", model::sandbox_line(i))),
        UiEvent::SandboxDesktop(name, d) => writeln!(out, "{name} desktop: {}", d.http_url),
        UiEvent::MeshDevices(list) if list.is_empty() => writeln!(out, "mesh devices: none paired"),
        UiEvent::MeshDevices(list) => {
            let now = model::now_ms();
            list.iter()
                .try_for_each(|d| writeln!(out, "{}", model::mesh_device_line(d, now)))
        }
        UiEvent::MeshDevicePairing(p) => writeln!(
            out,
            "{}\n{}\npending {} — expires in 5 min",
            link::qr(&p.string),
            p.string,
            p.pending_id
        ),
        UiEvent::NodeEvent(nodes::NodeEvent::NodeAdded { node }) => {
            writeln!(out, "paired node {}", node.name)
        }
        UiEvent::NodeEvent(_) => Ok(()),
        UiEvent::Secrets(list) => writeln!(out, "key: {}", list.key_store).and_then(|()| {
            list.secrets.iter().try_for_each(|s| {
                writeln!(
                    out,
                    "{} · v{} ({} kept) · {} · {}",
                    s.name,
                    s.current,
                    s.versions.len(),
                    s.note,
                    s.updated
                )
            })
        }),
        UiEvent::SecretSet(info) => writeln!(out, "{}: v{} set", info.name, info.current),
        // No editor here: the request waits out `request_timeout_secs` unless another client answers.
        UiEvent::SecretsEvent(secrets::SecretsEvent::Requested { name, note, .. }) => {
            writeln!(
                out,
                "secret requested: {name} — {note} (needs a TTY or another client)"
            )
        }
        UiEvent::SecretsEvent(_) => Ok(()),
        // `/todos` in print mode: plain rows, no card (47 §6.3); a failed
        // read prints its error line instead.
        UiEvent::Todos(_, snapshot) => snapshot.iter().flat_map(|s| &s.items).try_for_each(|t| {
            writeln!(out, "[{}] {}", model::status_label(t.status), t.text).and_then(|()| {
                t.reason
                    .as_deref()
                    .map_or(Ok(()), |r| writeln!(out, "    {r}"))
            })
        }),
        UiEvent::Communication(s) => model::communication_lines(s)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        UiEvent::Peers(_, Err(e)) => writeln!(out, "no peers · {e}"),
        UiEvent::Peers(_, Ok(p)) if p.peers.is_empty() => {
            writeln!(out, "no peers this session may address")
        }
        UiEvent::Peers(_, Ok(page)) => page
            .peers
            .iter()
            .try_for_each(|p| writeln!(out, "{}", model::PeerRow::Peer(p).line())),
        // Full ids: no menu here, the id is what a later `/peers` answer needs.
        UiEvent::PeerRequests(_, page) => {
            let mut any = false;
            page.messages
                .iter()
                .try_for_each(|r| {
                    let id = r.receipt.id;
                    let side = if page.awaiting_answer.contains(&id) {
                        format!("from {} · awaiting your answer", r.from)
                    } else if page.awaiting_reply.contains(&id) {
                        format!("to {} · awaiting their reply", r.receipt.to)
                    } else {
                        return Ok(());
                    };
                    any = true;
                    writeln!(
                        out,
                        "{id}  {side}  {}",
                        r.text.lines().next().unwrap_or_default()
                    )
                })
                .and_then(|()| {
                    if any {
                        Ok(())
                    } else {
                        writeln!(
                            out,
                            "no requests await a human (a restart clears live waits)"
                        )
                    }
                })
        }
        UiEvent::PeerAnswered(r) => writeln!(
            out,
            "peers: {} answered → {}{}{}",
            r.reply_to.unwrap_or(r.id),
            r.state.as_str(),
            r.answered_by
                .as_deref()
                .map_or_else(String::new, |by| format!(" · by {by}")),
            r.error
                .as_deref()
                .map_or_else(String::new, |e| format!(" · {e}")),
        ),
        UiEvent::A2a(s) => model::a2a_lines(s)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        // Typed provenance (50 §10); the legacy `MessageDelivered` no longer
        // fires for peer bodies, so this is the one echo.
        UiEvent::PeerDelivered(ev) => writeln!(
            out,
            "⟵ {} · {}{}: {}…",
            ev.origin.from,
            ev.origin.id,
            ev.origin
                .reply_to
                .map_or_else(String::new, |r| format!(" · reply to {r}")),
            ev.text.lines().next().unwrap_or_default()
        ),
        // Invalidations: the model re-reads; nothing to say.
        UiEvent::CommunicationChanged(_)
        | UiEvent::PeerMessageChanged(_)
        | UiEvent::A2aChanged(_) => Ok(()),
        // The owner's display strings (51 §6.2), never a local sum.
        UiEvent::Cost(_, c) => model::cost_lines(&c.total)
            .iter()
            .try_for_each(|l| writeln!(out, "{l}")),
        UiEvent::Terminals(list) if list.is_empty() => {
            writeln!(out, "no terminals in this session's tree")
        }
        UiEvent::Terminals(list) => list
            .iter()
            .try_for_each(|t| writeln!(out, "{}", model::terminal_row(t))),
        // The observer needs a TTY; a page that lands here prints its text rows.
        UiEvent::PtyScreen {
            reply: Ok(tools::PtyScreenReply::Updated(p)),
            ..
        } => p.lines.iter().try_for_each(|row| {
            let text: String = row
                .iter()
                .flat_map(|r| r.cells.iter())
                .filter(|c| !c.continuation)
                .map(|c| c.text.as_str())
                .collect();
            writeln!(out, "{}", text.trim_end())
        }),
        UiEvent::PtyScreen {
            reply: Ok(tools::PtyScreenReply::Unavailable),
            ..
        } => {
            writeln!(out, "No longer available.")
        }
        UiEvent::PtyScreen {
            reply: Ok(tools::PtyScreenReply::Unchanged { .. }),
            ..
        } => Ok(()),
        UiEvent::PtyScreen { reply: Err(e), .. } => writeln!(out, "terminal: {e}"),
        UiEvent::SessionOpened(o) => writeln!(
            out,
            "session {}{}",
            o.session,
            if o.retention == agent::Retention::Private {
                " (private)"
            } else {
                ""
            }
        ),
        UiEvent::SessionLeft(_, r) => writeln!(
            out,
            "private session closed{}",
            match r.cleanup {
                agent::CleanupStatus::Complete => "",
                agent::CleanupStatus::Pending => " (cleanup pending)",
                agent::CleanupStatus::Incomplete => " (cleanup incomplete)",
            }
        ),
        // No editor in print mode: nothing to dictate into.
        UiEvent::Voice(_)
        | UiEvent::Talk(_)
        | UiEvent::DictationStarting(_)
        | UiEvent::TalkStarting(_) => Ok(()),
        UiEvent::DictationFailed(m) => writeln!(out, "dictation: {m}"),
        UiEvent::TalkFailed(m) => writeln!(out, "talk: {m}"),
        // Stderr: `-p` output stays the reply alone.
        UiEvent::Engine(Some(label)) => {
            eprintln!("{label}");
            Ok(())
        }
        UiEvent::Engine(None) => {
            eprintln!("node process started here (pid {})", std::process::id());
            Ok(())
        }
    };
}
