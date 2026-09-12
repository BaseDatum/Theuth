//! One external harness process bound to one ACP session (design doc 06 §3.2,
//! §3.7, §8.1–§8.3). [`Harness::connect`] spawns the process, runs
//! `initialize` → auth → `session/load` | `session/new`, and hands back a
//! handle; `session/update` notifications and `session/request_permission`
//! requests arrive on the caller's channel as [`Update`]s in wire order.
//!
//! The SDK connection lives inside one `tokio::spawn`ed future; the child
//! process (its whole process group) dies when that future ends, which
//! happens on [`Harness::close`], when the last handle is dropped, or when
//! the child's stdout hits EOF.

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentRequest, AuthCapabilities, AuthMethod, AuthenticateRequest, CancelNotification,
    ClientCapabilities, ContentBlock, Error, ErrorCode, Implementation, InitializeRequest,
    InitializeResponse, LoadSessionRequest, NewSessionRequest, PermissionOption, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionId,
    SessionNotification, SessionUpdate, StopReason, ToolCallUpdate,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, JsonRpcRequest, LineDirection,
    Responder, is_incoming_transport_closed,
};
use futures::future::BoxFuture;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// [`probe`] gives up on `initialize` + `session/new` after this long.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the caller gets from the connection: `session/update` notifications
/// and `session/request_permission` requests, in wire order.
pub enum Update {
    /// A `session/update` notification's payload.
    Session(SessionUpdate),
    /// A `session/request_permission` request; answer it exactly once.
    Permission(Permission),
    /// stdout EOF / process exit: the exit error text (with the stderr tail on
    /// a non-zero status) or `process exited`.
    Closed(String),
}

impl fmt::Debug for Update {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Update::Session(u) => f.debug_tuple("Session").field(u).finish(),
            Update::Permission(p) => f.debug_tuple("Permission").field(&p.tool_call).finish(),
            Update::Closed(why) => f.debug_tuple("Closed").field(why).finish(),
        }
    }
}

/// A permission request; answer exactly once via [`Permission::respond`].
pub struct Permission {
    /// The tool call asking (title/kind/raw_input live under `.fields`).
    pub tool_call: ToolCallUpdate,
    /// Options offered by the harness (`allow_once`, `reject_once`, …).
    pub options: Vec<PermissionOption>,
    responder: Responder<RequestPermissionResponse>,
}

impl Permission {
    /// Send the outcome back to the harness.
    pub fn respond(self, outcome: RequestPermissionOutcome) {
        if let Err(e) = self
            .responder
            .respond(RequestPermissionResponse::new(outcome))
        {
            tracing::warn!("acp permission response: {e}");
        }
    }
}

/// How to connect.
pub struct Connect {
    /// Process launch configuration ([`crate::Entry::config`]).
    pub config: AcpAgentConfig,
    /// Working directory for `session/new` / `session/load` (the process itself
    /// inherits wth's cwd, 06 §8.4).
    pub cwd: PathBuf,
    /// Existing ACP session id to `session/load` when the agent advertises
    /// `loadSession`; else a new session.
    pub resume: Option<String>,
    /// Where the child's stderr lines are appended.
    pub stderr_log: PathBuf,
}

/// One in-flight `session/prompt`: resolves with the stop reason (serde name,
/// `end_turn` …) or `Err(text)` on transport failure. `'static`, so it can be
/// awaited from a spawned task while the [`Harness`] stays with its owner.
pub type Turn = BoxFuture<'static, Result<String, String>>;

/// A live connection to one harness process with one ACP session. Cheap to
/// clone; every clone shares the connection, and [`Harness::close`] on any of
/// them (or dropping the last one) ends it and kills the process group.
#[derive(Clone)]
pub struct Harness {
    cx: ConnectionTo<Agent>,
    session: SessionId,
    resumed: bool,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl fmt::Debug for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Harness")
            .field("session", &self.session)
            .field("resumed", &self.resumed)
            .finish_non_exhaustive()
    }
}

/// Result of [`probe`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    /// `initialize` and `session/new` succeeded.
    pub ok: bool,
    /// `ok` | `login required: <cmd>` | `spawn failed: <why>` (the
    /// [`crate::EntryInfo::status`] strings).
    pub status: String,
}

/// The spawned connection before a session exists: the handle, the shutdown
/// trigger, and the connection task (its result is the process exit text).
/// Dropping it ends the connection.
struct Link {
    cx: ConnectionTo<Agent>,
    shutdown: oneshot::Sender<()>,
    done: Option<JoinHandle<Result<(), Error>>>,
}

/// Why the handshake could not open a session.
enum Fail {
    /// Authentication required and not doable from here: `cmd` is what to
    /// run in a terminal (terminal-type method) or the method's name.
    Login(String),
    /// Anything else, already rendered.
    Other(String),
}

impl Link {
    /// Spawn the process and wait for the connection handle.
    async fn spawn(
        config: AcpAgentConfig,
        stderr_log: &Path,
        tx: UnboundedSender<Update>,
    ) -> Result<Link, String> {
        let log = Mutex::new(open_log(stderr_log));
        let agent = AcpAgent::new(config).with_debug(move |line, dir| {
            if dir == LineDirection::Stderr
                && let Ok(mut guard) = log.lock()
                && let Some(f) = guard.as_mut()
            {
                let _ = writeln!(f, "{line}");
            }
        });

        let (cx_tx, cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (shutdown, shutdown_rx) = oneshot::channel::<()>();
        let notify_tx = tx.clone();
        let permission_tx = tx;
        let connection = Client
            .builder()
            .on_receive_notification(
                async move |n: SessionNotification, _cx| {
                    let _ = notify_tx.send(Update::Session(n.update));
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |r: RequestPermissionRequest, responder, _cx| {
                    let _ = permission_tx.send(Update::Permission(Permission {
                        tool_call: r.tool_call,
                        options: r.options,
                        responder,
                    }));
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            // Mandatory catch-all (06 §8.3): the SDK would otherwise queue any
            // unhandled session-scoped request (`fs/*`, `terminal/*`) forever.
            .on_receive_request(
                async |_r: AgentRequest, responder, _cx| {
                    let _ = responder.respond_with_error(Error::method_not_found());
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, |cx: ConnectionTo<Agent>| async move {
                let _ = cx_tx.send(cx.clone());
                // The connection (and the child) lives until `close`, the last
                // handle drop, or the child's stdout EOF.
                tokio::select! {
                    _ = shutdown_rx => {}
                    () = cx.incoming_closed() => {}
                }
                Ok(())
            });
        let done = tokio::spawn(connection);
        match cx_rx.await {
            Ok(cx) => Ok(Link {
                cx,
                shutdown,
                done: Some(done),
            }),
            Err(_) => Err(format!("spawn failed: {}", exit_text(done.await))),
        }
    }

    async fn request<R: JsonRpcRequest>(&self, req: R) -> Result<R::Response, Error> {
        self.cx.send_request(req).block_task().await
    }

    /// Text for a failed request: on transport EOF wait for the exit status,
    /// whose text carries the stderr tail.
    async fn explain(&mut self, e: Error) -> String {
        if is_incoming_transport_closed(&e)
            && let Some(done) = self.done.take()
        {
            return exit_text(done.await);
        }
        error_text(&e)
    }

    /// `initialize`, then `session/load` (when `resume` is set and the agent
    /// advertises `loadSession`; a failing load falls back to a new session)
    /// or `session/new`. `auth_required` → `authenticate` once with an
    /// agent-type method when `authenticate`, else [`Fail::Login`].
    async fn open(
        &mut self,
        config: &AcpAgentConfig,
        cwd: &Path,
        resume: Option<String>,
        authenticate: bool,
    ) -> Result<(SessionId, bool), Fail> {
        let init: InitializeResponse = match self.request(initialize_request()).await {
            Ok(i) => i,
            Err(e) => return Err(Fail::Other(self.explain(e).await)),
        };
        let mut load = resume.filter(|_| init.agent_capabilities.load_session);
        let mut authenticated = false;
        loop {
            let opened = match &load {
                Some(sid) => self
                    .request(LoadSessionRequest::new(SessionId::new(sid.as_str()), cwd))
                    .await
                    .map(|_| (SessionId::new(sid.as_str()), true)),
                None => self
                    .request(NewSessionRequest::new(cwd))
                    .await
                    .map(|r| (r.session_id, false)),
            };
            match opened {
                Ok(session) => return Ok(session),
                Err(e) if e.code == ErrorCode::AuthRequired && !authenticated => {
                    let agent_method = init.auth_methods.iter().find_map(|m| match m {
                        AuthMethod::Agent(a) => Some(a),
                        _ => None,
                    });
                    match agent_method {
                        Some(m) if authenticate => {
                            if let Err(e) =
                                self.request(AuthenticateRequest::new(m.id.clone())).await
                            {
                                let why = self.explain(e).await;
                                return Err(Fail::Other(format!(
                                    "authenticate ({}): {why}",
                                    m.name
                                )));
                            }
                            authenticated = true;
                        }
                        Some(m) => return Err(Fail::Login(m.name.clone())),
                        None => return Err(Fail::Login(login_command(config, &init.auth_methods))),
                    }
                }
                Err(e) if load.is_some() && !is_incoming_transport_closed(&e) => {
                    tracing::warn!("acp session/load failed, starting a new session: {e}");
                    load = None;
                }
                Err(e) => return Err(Fail::Other(self.explain(e).await)),
            }
        }
    }
}

impl Harness {
    /// Spawn the process, `initialize` (client capabilities: no `fs`, no
    /// `terminal`, `auth.terminal: true`; client info `wth`/version), authenticate
    /// per 06 §3.7 (agent-type method → `authenticate` once and retry; only a
    /// terminal-type method → `Err("not logged in — run `<cmd>` in a terminal, then retry")`),
    /// then `session/load` (when `resume` is set and `loadSession` is advertised)
    /// or `session/new {cwd}`. Updates flow to `tx`. The `Err` text is what the
    /// model and the user see; on process exit it includes the last stderr lines.
    pub async fn connect(c: Connect, tx: UnboundedSender<Update>) -> Result<Harness, String> {
        let Connect {
            config,
            cwd,
            resume,
            stderr_log,
        } = c;
        let mut link = Link::spawn(config.clone(), &stderr_log, tx.clone()).await?;
        let (session, resumed) = match link.open(&config, &cwd, resume, true).await {
            Ok(s) => s,
            Err(Fail::Login(cmd)) => {
                return Err(format!(
                    "not logged in — run `{cmd}` in a terminal, then retry"
                ));
            }
            Err(Fail::Other(why)) => return Err(why),
        };
        let Link { cx, shutdown, done } = link;
        if let Some(done) = done {
            tokio::spawn(async move {
                let _ = tx.send(Update::Closed(exit_text(done.await)));
            });
        }
        Ok(Harness {
            cx,
            session,
            resumed,
            shutdown: Arc::new(Mutex::new(Some(shutdown))),
        })
    }

    /// The harness's session id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session.0
    }

    /// Whether `session/load` was used (resume honoured).
    #[must_use]
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// One `session/prompt`; the returned future resolves with the stop reason
    /// string (`end_turn` …) or `Err(text)` on transport failure. Await it from
    /// a task, never from inside an SDK handler. Dropping it cancels the request.
    pub fn prompt(&self, text: String) -> Turn {
        let cx = self.cx.clone();
        let session = self.session.clone();
        Box::pin(async move {
            let r = cx
                .send_request(PromptRequest::new(session, vec![ContentBlock::from(text)]))
                .block_task()
                .await
                .map_err(|e| error_text(&e))?;
            Ok(stop_reason(r.stop_reason))
        })
    }

    /// `session/cancel` notification; the in-flight prompt then resolves `cancelled`.
    pub fn cancel(&self) -> Result<(), String> {
        self.cx
            .send_notification(CancelNotification::new(self.session.clone()))
            .map_err(|e| error_text(&e))
    }

    /// End the connection; kills the process group.
    pub fn close(self) {
        if let Ok(mut guard) = self.shutdown.lock() {
            guard.take();
        }
    }
}

/// Spawn, `initialize`, `session/new`, report, close. Never authenticates.
/// [`PROBE_TIMEOUT`] → `spawn failed: timeout`.
pub async fn probe(config: AcpAgentConfig, stderr_log: PathBuf) -> ProbeResult {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let attempt = async {
        let mut link = Link::spawn(config.clone(), &stderr_log, tx).await?;
        link.open(&config, &cwd, None, false)
            .await
            .map_err(|f| match f {
                Fail::Login(cmd) => format!("login required: {cmd}"),
                Fail::Other(why) => format!("spawn failed: {why}"),
            })
    };
    match tokio::time::timeout(PROBE_TIMEOUT, attempt).await {
        Ok(Ok(_)) => ProbeResult {
            ok: true,
            status: "ok".to_owned(),
        },
        Ok(Err(status)) => ProbeResult { ok: false, status },
        Err(_) => ProbeResult {
            ok: false,
            status: "spawn failed: timeout".to_owned(),
        },
    }
}

fn initialize_request() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            ClientCapabilities::new()
                .terminal(false)
                .auth(AuthCapabilities::new().terminal(true)),
        )
        .client_info(Implementation::new("wth", env!("CARGO_PKG_VERSION")))
}

/// The command line the user must run for a terminal-type auth method: our
/// configured invocation plus the method's `args`, prefixed by its `env`.
fn login_command(config: &AcpAgentConfig, methods: &[AuthMethod]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let terminal = methods.iter().find_map(|m| match m {
        AuthMethod::Terminal(t) => Some(t),
        _ => None,
    });
    if let Some(t) = terminal {
        parts.extend(t.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    parts.push(config.command().display().to_string());
    parts.extend(config.arguments().iter().cloned());
    if let Some(t) = terminal {
        parts.extend(t.args.iter().cloned());
    }
    parts.join(" ")
}

/// Serde name of a stop reason (`end_turn`, `cancelled`, …).
fn stop_reason(s: StopReason) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "end_turn".to_owned())
}

/// What the connection task's result says about the process.
fn exit_text(r: Result<Result<(), Error>, tokio::task::JoinError>) -> String {
    match r {
        Ok(Ok(())) => "process exited".to_owned(),
        Ok(Err(e)) => error_text(&e),
        Err(e) => format!("connection task failed: {e}"),
    }
}

/// `message: data` for the SDK's internal errors, whose `data` is either a
/// string or `{spawned_at, data}` (the spawn failure carries the OS error
/// there); the plain `Display` otherwise.
fn error_text(e: &Error) -> String {
    let detail = match &e.data {
        Some(serde_json::Value::String(s)) => Some(s.as_str()),
        Some(serde_json::Value::Object(o)) => o.get("data").and_then(|d| d.as_str()),
        _ => None,
    };
    match detail {
        Some(d) => format!("{}: {d}", e.message),
        None => e.to_string(),
    }
}

fn open_log(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match File::options().append(true).create(true).open(path) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!("acp stderr log {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{AuthMethodAgent, AuthMethodTerminal};

    #[test]
    fn stop_reasons_are_serde_names() {
        assert_eq!(stop_reason(StopReason::EndTurn), "end_turn");
        assert_eq!(
            stop_reason(StopReason::MaxTurnRequests),
            "max_turn_requests"
        );
        assert_eq!(stop_reason(StopReason::Cancelled), "cancelled");
    }

    #[test]
    fn login_command_appends_terminal_method() {
        let config = AcpAgentConfig::new("codex-acp").arg("--acp");
        let mut term = AuthMethodTerminal::new("login", "Log in");
        term.args = vec!["--login".into()];
        term.env = [("A".to_owned(), "1".to_owned())].into();
        let methods = vec![
            AuthMethod::Agent(AuthMethodAgent::new("oauth", "OAuth")),
            AuthMethod::Terminal(term),
        ];
        assert_eq!(
            login_command(&config, &methods),
            "A=1 codex-acp --acp --login"
        );
        assert_eq!(login_command(&config, &[]), "codex-acp --acp");
    }
}
