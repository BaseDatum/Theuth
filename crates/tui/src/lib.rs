//! Terminal UI: one topic intake, in-process or attached, feeding the same
//! synchronous model/component loop. Commands use the current RPC engine.
//! Local kernel-job visibility is owned by the jobs component's adapter.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use parking_lot::RwLock;
use std::time::Duration;

use agent::{AgentConfig, AgentEvent, CompactStyle};
use futures::future::BoxFuture;
use jsonrpsee::core::client::Error as ClientError;
use jsonrpsee::ws_client::WsClient;
use rpc_compose::api::clients::*;
use rpc::client::{Engine, Local};
use rpc::engine::{Host, Running};
use runtime::{Runtime, SessionId};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tools::{Decision, McpAuth};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod attach;
mod clip;
pub mod components;
pub mod topics;
pub mod model;
pub mod pathac;
pub mod talk;
mod ui;

use model::{Command, LoginRow, Model, UiEvent};

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
        let local_stream = self.attached.is_none().then(|| rpc_compose::topics::local_stream(rt));
        let local_jobs = Arc::new(AtomicBool::new(self.attached.is_none()));
        components::jobs::subscribe(rt, events_tx.clone(), local_jobs.clone()).await;
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
        let components = components::builtin(info.as_ref().is_some_and(|i| i.app_enabled));
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
                    components: Some(components),
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
                    components,
                    prompt,
                )
                .await;
            }
            shutdown.cancel();
        });
        Ok(())
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
                let stream = rpc_compose::topics::local_stream(&rt);
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
        if let Command::ComponentInput(reply) = cmd {
            let _ = tx.send(UiEvent::Component(reply));
            continue;
        }
        if matches!(&cmd, Command::Component(effect) if effect.lane == components::Lane::OrderedInput)
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
        Command::Component(_) | Command::ComponentInput(_) => unreachable!("component work uses the generic dispatcher"),
        Command::CreateDraft(options) => {
            return match open_draft(api, options, tx).await {
                Ok(_) => Ok(()),
                Err(error) => { let _ = tx.send(UiEvent::DraftFailed(error.to_string())); Ok(()) }
            };
        }
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
        Command::EditContext {
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
        Command::SetSessionProject { session, project } => {
            msg(api.set_session_project(session, project.clone()).await)?;
            UiEvent::Info(match project {
                Some(p) => format!("project: {p}"),
                None => "project cleared".to_owned(),
            })
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
        Command::SetApprovals { session, enabled } => {
            UiEvent::Approvals(msg(api.set_approvals(session, enabled, None).await)?)
        }
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


/// Keep only the message: the JSON-RPC code is not user-facing.
fn msg<T>(r: Result<T, ClientError>) -> anyhow::Result<T> {
    r.map_err(|e| anyhow::anyhow!("{}", reason(e)))
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
    mut components: components::Registry,
    prompt: Option<String>,
) {
    components.set_interactive(false);
    let mut model = Model::with_components(String::new(), approvals, components);
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
        let wake = model.components.wake();
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                let mut done = false;
                model.receive(ev, &mut |output| {
                    done |= print_output(&mut out, output) && print_mode;
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
                if done { break; }
                if !stdin_open && model.approval().is_some() {
                    let _ = writeln!(out, "denied: stdin closed");
                    model.decide(Decision::Deny, &commands);
                }
            }
            // Component deadlines share the existing loop, without a scheduler.
            _ = tokio::time::sleep(std::time::Duration::from_millis(
                wake.map_or(0, |t| t.saturating_sub(model::now_ms())).max(1),
            )), if wake.is_some() => {
                let now = model::now_ms();
                let mut done = false;
                model.component_tick_with(now, &mut |output| {
                    done |= print_output(&mut out, output) && print_mode;
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

fn print_output(out: &mut impl Write, output: model::Output<'_>) -> bool {
    match output {
        model::Output::Event(model, event) => {
            print_event(out, model, event);
            let finished = match event {
                UiEvent::Agent(AgentEvent::TurnFinished { session, .. }) => model.session == Some(*session),
                UiEvent::Error(_) => true,
                _ => false,
            };
            return finished;
        }
        model::Output::Plain(components::Plain::Finished) => return true,
        model::Output::Plain(components::Plain::Text(text)) => {
            let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
        }
        model::Output::Plain(components::Plain::Line(text) | components::Plain::TtyRequired(text)) => {
            let _ = writeln!(out, "{text}");
        }
    }
    false
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
        UiEvent::Wire(_) | UiEvent::Component(_) | UiEvent::DraftFailed(_) => unreachable!("intake is dispatched before printing"),
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
        UiEvent::Models(_) | UiEvent::PathComplete(..) => {
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
        UiEvent::Approvals(s) => writeln!(
            out,
            "approvals: {} (this session), {} (default)",
            if s.session { "on" } else { "off" },
            if s.global { "on" } else { "off" }
        ),
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
        | UiEvent::TalkStarting(_) => Ok(()),
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
