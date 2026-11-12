//! Terminal UI: one topic intake, in-process or attached, feeding the same
//! synchronous model/component loop. Commands use the current RPC engine.
//! Only local jobs retain a typed subscriber, preserving kernel visibility.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use parking_lot::RwLock;
use std::time::Duration;

use agent::{AgentConfig, AgentEvent, CompactStyle};
use futures::future::BoxFuture;
use jsonrpsee::core::client::Error as ClientError;
use jsonrpsee::ws_client::WsClient;
use kameo::Actor;
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use rpc::api::clients::*;
use rpc::client::{Engine, Local};
use rpc::engine::{Host, Running};
use runtime::{Runtime, SessionId};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tools::{Decision, JobEvent, McpAuth};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod attach;
pub mod broadcast;
mod clip;
pub mod components;
pub mod topics;
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
    /// Method assembly used at startup and after attached-engine takeover.
    pub module: rpc_core::ModuleFactory,
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
        // Take the ring cursor before any asynchronous initialization.
        let local_stream = self.attached.is_none().then(|| rpc::topics::local_stream(rt));
        let local_jobs = Arc::new(AtomicBool::new(self.attached.is_none()));
        let bridge = JobsBridge::spawn_with_mailbox(
            JobsBridge { tx: events_tx.clone(), local: local_jobs.clone() },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<JobsBridge, JobEvent>(&bridge).await;
        if let Some(stream) = local_stream {
            tokio::spawn(topics::local(rt.clone(), stream, events_tx.clone()));
        }
        let api: Api = Arc::new(RwLock::new(Arc::new(match &self.attached {
            Some(a) => Engine::Remote(a.client.clone()),
            None => Engine::Local(Local::new(rt.clone(), self.module)),
        })));
        if let Some(attached) = self.attached {
            let _ = events_tx.send(UiEvent::Engine(Some(attached_label(&attached.running))));
            tokio::spawn(supervise(
                rt.clone(),
                api.clone(),
                events_tx.clone(),
                attached,
                self.module,
                local_jobs,
            ));
        }
        // Startup facts the runtime does not hand out as config sections: whether
        // the app plugin runs (46 §9) and the engine's live default model and effort
        // (82 §3, 83 §3; config.yaml as read at launch is the fallback when `info` fails).
        let engine = api.read().clone();
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

/// The only typed intake exception: the external wire projector hides kernels.
#[derive(Actor)]
struct JobsBridge {
    tx: UnboundedSender<UiEvent>,
    local: Arc<AtomicBool>,
}

impl Message<JobEvent> for JobsBridge {
    type Reply = ();
    async fn handle(&mut self, event: JobEvent, _: &mut Context<Self, ()>) {
        if self.local.load(Ordering::Acquire) {
            let _ = self.tx.send(UiEvent::Job(event));
        }
    }
}

/// Attached mode's supervisor (63 §2.3 Disconnect): when the socket closes,
/// take the lock and start the engine here, or — another client won — attach
/// to the winner; three attempts, then a plain error. Every switch tells the
/// UI (`Engine`), which re-selects its session on the new end.
async fn supervise(rt: Runtime, api: Api, tx: UnboundedSender<UiEvent>, attached: Attached, module: rpc_core::ModuleFactory, local_jobs: Arc<AtomicBool>) {
    // The record's listen until the implicit server binds (63 §6.4).
    let listen = rpc::configured_listen(&rt);
    let mut client = attached.client;
    let mut start_engine = Some(attached.start_engine);
    for attempt in 1..=3 {
        if let Err(e) = topics::remote(&rt, &client, &tx).await {
            tracing::warn!(error = %e, "engine event stream");
        }
        if rt.shutdown.is_cancelled() || tx.is_closed() { return; }
        let _ = tx.send(UiEvent::Info("node process gone — starting one…".into()));
        let running = match rpc::engine::acquire(&rt.data_dir, Host::Tui, listen.clone()) {
            Ok(lease) => {
                rpc::engine::hold(lease);
                let Some(start) = start_engine.take() else {
                    return;
                };
                let stream = rpc::topics::local_stream(&rt);
                local_jobs.store(true, Ordering::Release);
                match start(rt.clone()).await {
                    Ok(()) => {
                        *api.write() = Arc::new(Engine::Local(Local::new(rt.clone(), module)));
                        let _ = tx.send(UiEvent::Engine(None));
                        tokio::spawn(topics::local(rt.clone(), stream, tx.clone()));
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
        *api.write() = Arc::new(Engine::Remote(client.clone()));
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
                if let Err(e) = execute_current(&api, cmd, &tx).await {
                    let _ = tx.send(UiEvent::Error(format!("{e:#}")));
                }
            }
        }
    });
    while let Some(cmd) = rx.recv().await {
        if matches!(&cmd, Command::PtyInput { .. })
            || matches!(&cmd, Command::Component(effect) if effect.lane == components::Lane::OrderedInput)
        {
            let _ = keys_tx.send(cmd);
            continue;
        }
        let tx = tx.clone();
        let api = api.clone();
        tokio::spawn(async move {
            if let Err(e) = execute_current(&api, cmd, &tx).await {
                let _ = tx.send(UiEvent::Error(format!("{e:#}")));
            }
        });
    }
}

async fn execute_current(api: &Api, cmd: Command, tx: &UnboundedSender<UiEvent>) -> anyhow::Result<()> {
    let engine = api.read().clone();
    if let Command::Component(effect) = cmd {
        let reply = components::execute(effect, engine).await;
        let _ = tx.send(reply);
        Ok(())
    } else {
        execute(&engine, cmd, tx).await
    }
}

async fn execute(api: &Engine, cmd: Command, tx: &UnboundedSender<UiEvent>) -> anyhow::Result<()> {
    let reply = match cmd {
        Command::Component(_) => unreachable!("component effects use the generic executor"),
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
            draft,
        } => {
            let session = match draft {
                Some(d) => open_draft(api, d, tx).await.map(Some),
                None => Ok(session),
            };
            let r = match session {
                Ok(session) => msg(api
                    .send_input(session, text.clone(), files, None, None)
                    .await),
                Err(e) => Err(e),
            };
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
                    msg(api.cancel(session, None, None).await)?;
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
            msg(api.cancel(session, None, None).await)?;
            return Ok(());
        }
        Command::CancelTree(session) => {
            msg(api
                .cancel(session, Some(agent::CancelScope::Tree), None)
                .await)?;
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
        Command::TakeSession(id) => {
            let o = msg(api.session_take(id, "user".into()).await)?;
            UiEvent::Info(format!("taking session here (epoch {})", o.epoch))
        }
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
        Command::EndlessFold { session } => {
            let m = msg(api.endless_fold(session).await)?;
            UiEvent::Info(format!(
                "Backlog folded (excerpts, no model call). {}",
                agent::endless_ext::status(&m)
            ))
        }
        Command::EndlessPause { session, paused } => {
            let m = msg(api.endless_pause(session, paused).await)?;
            UiEvent::Info(format!(
                "Settling {}. {}",
                if paused { "paused" } else { "resumed" },
                agent::endless_ext::status(&m)
            ))
        }
        Command::EndlessModel { session, model } => {
            let m = msg(api.set_session_compactor_model(session, model).await)?;
            UiEvent::Info(format!(
                "Compactor for this chat: {} ({}).",
                m.model, m.model_source
            ))
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
            api.path_complete(session, input, cursor, None).await.ok(),
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
        Command::ListModels => UiEvent::Models(msg(api.list_models(None).await)?),
        // 148 R1: this session only; the default is the Settings UI's to set.
        Command::SetModel { session, model } => {
            UiEvent::ModelSet(msg(api.set_model(Some(session), model, None).await)?)
        }
        Command::SetEffort { session, effort } => {
            msg(api.set_effort(Some(session), effort.clone(), None).await)?;
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
            draft,
        } => {
            let session = match (session, draft) {
                (Some(s), _) => s,
                (None, Some(d)) => open_draft(api, d, tx).await?,
                (None, None) => {
                    let s = msg(api.new_session().await)?;
                    let _ = tx.send(UiEvent::SessionCreated(s));
                    s
                }
            };
            msg(api.set_mcp_hidden(session, tools, hidden).await)?;
            UiEvent::Mcp(msg(api.list_mcp(Some(session)).await)?)
        }
        Command::SetApprovals { session, enabled } => {
            UiEvent::Approvals(msg(api.set_approvals(session, enabled, None).await)?)
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
            draft,
        } => {
            let session = match draft {
                Some(d) => Some(open_draft(api, d, tx).await?),
                None => session,
            };
            UiEvent::SessionCreated(msg(api.use_skill(session, name, args).await)?)
        }
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
        } => UiEvent::ProviderAdded(msg(api
            .add_provider(name, base_url, (!key.is_empty()).then_some(key), None)
            .await)?),
        Command::RemoveProvider(name) => {
            msg(api.remove_provider(name.clone()).await)?;
            UiEvent::Info(format!("removed provider {name}"))
        }
        Command::RefreshProvider(name) => {
            UiEvent::Info(msg(api.refresh_provider_models(name).await)?.line())
        }
        Command::FindLocalServers => UiEvent::Info(
            msg(api.local_servers_probe().await)?
                .iter()
                .map(agent::LocalServer::line)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Command::SetModelLimits {
            provider,
            model,
            context,
            output,
        } => {
            // R2-8: no output given keeps the stored one (as the desktop does); 0 clears.
            let output = match output {
                Some(o) => (o > 0).then_some(o),
                None => msg(api.provider_models(provider.clone()).await)?
                    .into_iter()
                    .find(|m| m.id == model)
                    .map(|m| m.output)
                    .filter(|&o| o > 0),
            };
            let m = msg(api
                .set_model_limits(provider.clone(), model, context, output)
                .await)?;
            UiEvent::Info(format!("{provider}/{}", m.line()))
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
        Command::ExportArtifact { id, name, session } => {
            let dest = std::env::home_dir()
                .ok_or_else(|| anyhow::anyhow!("no home directory"))?
                .join("Downloads")
                .join(name);
            let path = msg(api.export_artifact(id, dest, session).await)?;
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
        Command::DictationStart { pressed, commands } => {
            // 153 §14.1: the TUI's surface and its slash commands (shaping stage 4), the press age for §8.7.
            let opts = voice::DictationOpts {
                surface: Some(voice::Surface::Tui),
                destination: Some(voice::Destination::AgentPrompt),
                app: None,
                commands,
                press_ago_ms: u32::try_from(pressed.elapsed().as_millis()).ok(),
            };
            match api.dictation_start(None, Some(opts)).await {
                Ok(id) => UiEvent::DictationStarting(id),
                Err(e) => UiEvent::DictationFailed(reason(e)),
            }
        }
        Command::DictationStop(id) => {
            msg(api.dictation_stop(id).await)?;
            return Ok(());
        }
        // Fire-and-forget: a failed correction is a debug line, not an error card.
        Command::DictationCorrection { id, text } => {
            if let Err(e) = api.dictation_correction(id, text).await {
                tracing::debug!("dictation_correction: {}", reason(e));
            }
            return Ok(());
        }
        Command::SuggestionAnswer { id, answer } => {
            msg(api.dictionary_suggestion_answer(id, answer).await)?;
            return Ok(());
        }
        Command::VoiceStatus => {
            let status = msg(api.voice_status().await)?;
            let dictation = msg(api.dictation_status().await)?;
            let devices = api.list_input_devices().await.unwrap_or_default();
            // Like the device list: a failed read drops its row, not the whole reply.
            let settings = api.voice_settings().await.ok();
            UiEvent::Info(model::voice_line(
                &status,
                &dictation,
                &devices,
                settings.as_ref(),
            ))
        }
        Command::SetInputDevices(names) => {
            msg(api.set_input_devices(names.clone()).await)?;
            UiEvent::Info(match names.first() {
                Some(n) => format!("mic → {n}"),
                None => "mic → system default".to_owned(),
            })
        }
        Command::SetShape(preset) => {
            msg(api.set_shape(voice::Surface::Tui, preset).await)?;
            UiEvent::Info(format!("voice shape (tui) → {preset:?}").to_lowercase())
        }
        Command::TalkStart { session, draft } => start_talk(api, session, draft, tx).await,
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

/// 148 R1: a draft with a pending model or effort opens its session through
/// `create_session` (the picks ride on the options, never the default); the
/// UI adopts it through `SessionCreated` before the command's own reply.
async fn open_draft(
    api: &Engine,
    draft: agent::CreateSessionOptions,
    tx: &UnboundedSender<UiEvent>,
) -> anyhow::Result<SessionId> {
    let s = msg(api.create_session(draft).await)?.session;
    let _ = tx.send(UiEvent::SessionCreated(s));
    Ok(s)
}

/// `/talk` (76 §5.1): with no session, create one first (on the draft's
/// picks, 148 R1) — the model adopts it through `SessionOpened` before the
/// talk id arrives — then `talk_start` with `sink: device`. `THEUTH_TALK_SIM`
/// set: the scripted [`talk::sim`] stands in for the engine (the brief's
/// development source).
async fn start_talk(
    api: &Engine,
    session: Option<SessionId>,
    draft: Option<agent::CreateSessionOptions>,
    tx: &UnboundedSender<UiEvent>,
) -> UiEvent {
    let session = match session {
        Some(s) => s,
        None => match api.create_session(draft.unwrap_or_default()).await {
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
            // A search hit is durable: a private tree is never indexed.
            let artifact = match api.get_artifact(call_id.clone(), None).await {
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
    model.components.set_interactive(false);
    model.app_enabled = app_enabled;
    model.mount_components(&commands);
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
        let wake = [model.search_wake(), model.components.wake()].into_iter().flatten().min();
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                let mut done = false;
                let mut search_reply = false;
                model.receive(ev, &mut |output| {
                    let (finished, search) = print_output(&mut out, output);
                    done |= print_mode && finished;
                    search_reply |= search;
                });
                if std::mem::take(&mut model.reselect)
                    && let Some(session) = model.session
                {
                    let _ = commands.send(Command::Load {
                        session,
                        settled_only: false,
                    });
                }
                if std::mem::take(&mut model.refresh_context)
                    && let Some(session) = model.session
                {
                    let _ = commands.send(Command::Load { session, settled_only: true });
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
            // Search and component deadlines share the existing loop, without a scheduler.
            _ = tokio::time::sleep(std::time::Duration::from_millis(
                wake.map_or(0, |t| t.saturating_sub(model::now_ms())).max(1),
            )), if wake.is_some() => {
                let now = model::now_ms();
                model.search_tick(now, &commands);
                let mut done = false;
                model.component_tick_with(now, &mut |output| {
                    done |= print_mode && print_output(&mut out, output).0;
                });
                if done { break; }
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
    model.component_notify(components::Lifecycle::Shutdown);
}

fn print_output(out: &mut impl Write, output: model::Output<'_>) -> (bool, bool) {
    match output {
        model::Output::Event(model, event) => {
            print_event(out, model, event);
            let finished = match event {
                UiEvent::Agent(AgentEvent::TurnFinished { session, .. }) => model.session == Some(*session),
                UiEvent::Error(_) => true,
                _ => false,
            };
            return (finished, matches!(event, UiEvent::Search(..)));
        }
        model::Output::Plain(components::Plain::Text(text)) => {
            let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
        }
        model::Output::Plain(components::Plain::Line(text) | components::Plain::TtyRequired(text)) => {
            let _ = writeln!(out, "{text}");
        }
    }
    (false, false)
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
        // 183 §5.3: one line.
        agent::HistoryEntry::Moved { moved, .. } => writeln!(out, "{}", model::moved_title(moved)),
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
        UiEvent::Wire(_) | UiEvent::Component(_) => unreachable!("intake is dispatched before printing"),
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
