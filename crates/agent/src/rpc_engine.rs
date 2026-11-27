//! The escape hatch on any engine of the mesh (224 §cloud nodes): a login
//! shell PTY, the engine log and the watchable desktop, here or on a peer by
//! its id, no SSH and no inbound ports.
//!
//! `engine_terminal` / `engine_logs` / `engine_desktop` run here when
//! `engine` is unset, empty or this engine, else forward over `theuth/rpc/1`
//! to that peer ([`forward::engine_peer`]). The PTY calls (`pty_input`,
//! `pty_resize`, `pty_screen`, `list_terminals`) take the same trailing
//! `engine`; the `subscribe_pty` stream for a peer's job rides its own
//! `theuth/pty/1` stream ([`ALPN_PTY`]): one [`PtyOpen`] line out, one
//! [`PtyAck`] line back, then one [`PtyChunk`] JSON line per item until
//! `exit` or either side closes ([`open`], [`serve`]). A peer's desktop is
//! reached through a loopback listener here whose every connection is piped
//! over a `theuth/desktop/1` stream ([`ALPN_DESKTOP`]) to the peer's
//! websockify ([`desktop_access`], [`serve_desktop`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::rpc_forward as forward;
use crate::rpc_support::{RpcServerExt, attach_pty, owner_root};
use base64::Engine as _;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{SubscriptionMessage, SubscriptionSink};
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{Server, ask, failed, lookup};
use replica::{ALPN_DESKTOP, ALPN_PTY, PeerId, Transport};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tools::{JOBS_ACTOR, JobId, Jobs, ListJobs, PtyAttached, PtyChunk, PtyOut};

/// `engine_logs` lines when `tail` is unset.
pub const LOG_TAIL_DEFAULT: u32 = 200;
/// Most `engine_logs` lines.
pub const LOG_TAIL_MAX: u32 = 2000;
/// The escape-hatch shell's job name; a running one is reused.
const TERMINAL: &str = "terminal";
/// Longest [`PtyOpen`] / [`PtyAck`] line.
const MAX_HEAD: u64 = 64 * 1024;

/// An interactive shell on an engine: attach with `subscribe_pty(session,
/// job, rows, cols, engine)`, type with `pty_input(…, engine)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineTerminal {
    /// The engine's owner root session holding the job.
    pub session: SessionId,
    /// The PTY job.
    pub job: JobId,
    /// Mesh id of the engine running it (pass it back as `engine`).
    pub engine: String,
}

/// The tail of an engine's own log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineLogs {
    /// Oldest first, secrets redacted, control characters escaped.
    pub lines: Vec<String>,
    /// The file read on that engine; `None` when it has no log yet.
    pub path: Option<String>,
}

/// A noVNC target: connect to `ws_url` (websockify) with VNC `password`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineDesktop {
    /// `ws://127.0.0.1:<port>/websockify` on the engine answering: the
    /// desktop's own websockify, or a mesh tunnel to a peer's.
    pub ws_url: String,
    /// The VNC password.
    pub password: String,
    /// `WIDTHxHEIGHT` of the desktop, when known.
    pub geometry: Option<String>,
    /// Mesh id of the engine whose desktop this is.
    pub engine: String,
}

/// Engine escape-hatch methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait EngineApi {
    /// Open (or reattach to) an interactive login shell (`$SHELL -l`, else
    /// `bash -l`) as a PTY job in the owner root session of `engine` (mesh
    /// id; unset, empty or this engine: here). Owner only. Attach with the
    /// usual PTY calls, passing the returned `engine`.
    #[method(name = "engine_terminal", with_extensions)]
    async fn engine_terminal(&self, engine: Option<String>) -> RpcResult<EngineTerminal>;
    /// The last `tail` lines (default 200, max 2000) of `engine`'s newest
    /// `logs/theuth.log.*`, redacted. Owner only.
    #[method(name = "engine_logs", with_extensions)]
    async fn engine_logs(&self, engine: Option<String>, tail: Option<u32>)
    -> RpcResult<EngineLogs>;
    /// The watchable desktop of `engine` (a VM cloud node's Xtigervnc behind
    /// websockify, `<data_dir>/desktop.json`): a peer's is tunnelled over the
    /// mesh to a loopback port here. Owner only. Errors when it has none.
    #[method(name = "engine_desktop", with_extensions)]
    async fn engine_desktop(&self, engine: Option<String>) -> RpcResult<EngineDesktop>;
}

#[async_trait]
impl EngineApiServer for Server {
    async fn engine_terminal(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<EngineTerminal> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            let mut t: EngineTerminal =
                forward::call_peer(ext, &peer, "engine_terminal", (None::<String>,)).await?;
            t.engine = peer.0;
            return Ok(t);
        }
        let store = ask::<crate::Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
        let me = store.engine().to_owned();
        let session = owner_root().await?;
        let auth = self.authorize(ext, session).await?;
        let running = ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await?;
        if let Some(j) = running
            .iter()
            .find(|j| j.pty && j.state == "running" && j.name.as_deref() == Some(TERMINAL))
        {
            return Ok(EngineTerminal { session, job: j.id.clone(), engine: me });
        }
        let shell = login_shell(std::env::var_os("SHELL").map(PathBuf::from), Path::exists);
        let out = lookup::<Jobs>(JOBS_ACTOR)?
            .ask(tools::pty::StartPty {
                session,
                call_id: "engine_terminal".into(),
                args: tools::pty::StartArgs {
                    command: format!("{} -l", shell.display()),
                    name: Some(TERMINAL.into()),
                    cwd: None,
                    env: None,
                    ready: None,
                    idle_secs: Some(0),
                },
                cwd: shell_cwd(&self.rt.data_dir),
                ready_timeout: Duration::from_secs(30),
                idle_after: None,
                root: auth.resources.root.clone(),
                activity: None,
                pty_shell: None,
                program: Some((shell, vec!["-l".into()])),
                cell: None,
            })
            .await
            .map_err(|e| failed(format!("jobs actor: {e}")))?;
        if out.is_error {
            return Err(failed(out.content));
        }
        let job = out
            .structured
            .as_ref()
            .and_then(|s| s.get("job"))
            .and_then(|j| j.as_str())
            .ok_or_else(|| failed(format!("the terminal exited at once:\n{}", out.content)))?;
        Ok(EngineTerminal { session, job: JobId(job.to_owned()), engine: me })
    }

    async fn engine_logs(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        tail: Option<u32>,
    ) -> RpcResult<EngineLogs> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            return forward::call_peer(ext, &peer, "engine_logs", (None::<String>, tail)).await;
        }
        let n = tail.unwrap_or(LOG_TAIL_DEFAULT).min(LOG_TAIL_MAX) as usize;
        let dir = self.rt.data_dir.join("logs");
        tokio::task::spawn_blocking(move || read_logs(&dir, n, tools::secrets::redact))
            .await
            .map_err(|e| failed(e.to_string()))
    }

    async fn engine_desktop(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<EngineDesktop> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            return desktop_via(ext, &peer).await;
        }
        let store = ask::<crate::Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
        let file = read_desktop(&self.rt.data_dir).map_err(failed)?;
        Ok(EngineDesktop {
            ws_url: ws_url(file.port),
            password: file.password,
            geometry: file.geometry,
            engine: store.engine().to_owned(),
        })
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(EngineApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}

/// `$SHELL` when it names an existing file, else `/bin/bash`, else `/bin/sh`.
fn login_shell(shell: Option<PathBuf>, exists: impl Fn(&Path) -> bool) -> PathBuf {
    shell
        .filter(|s| s.is_absolute() && exists(s))
        .or_else(|| Some(PathBuf::from("/bin/bash")).filter(|b| exists(b)))
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// The data dir's parent (the `engine` user's home on a node), else `$HOME`.
fn shell_cwd(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.to_path_buf())
}

/// The last `n` lines of the newest `theuth.log.*` in `dir` (the daily
/// appender's `theuth.log.YYYY-MM-DD`; names sort by date), each through
/// `redact` (`tools::secrets::redact`) and with control characters escaped.
fn read_logs(dir: &Path, n: usize, redact: fn(&str) -> Cow<'_, str>) -> EngineLogs {
    let newest = std::fs::read_dir(dir).ok().and_then(|entries| {
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.starts_with("theuth.log."))
            .max()
    });
    let Some(name) = newest else {
        return EngineLogs::default();
    };
    let path = dir.join(name);
    EngineLogs {
        lines: tail(&path, n)
            .iter()
            .map(|l| escape_controls(&redact(l)))
            .collect(),
        path: Some(path.display().to_string()),
    }
}

/// The last `n` lines of `path`, reading backwards in chunks; missing → empty.
fn tail(path: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;
    if n == 0 {
        return Vec::new();
    }
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let mut pos = len;
    let mut buf: Vec<u8> = Vec::new();
    while pos > 0 && buf.iter().filter(|&&b| b == b'\n').count() <= n {
        let take = CHUNK.min(pos);
        pos -= take;
        let mut chunk = vec![0; take as usize];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut chunk).is_err() {
            break;
        }
        chunk.extend_from_slice(&buf);
        buf = chunk;
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| (*l).to_owned())
        .collect()
}

/// Control characters (tab kept) as Rust escapes: a log line cannot repaint
/// the viewer's terminal.
fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && c != '\t' {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// The mesh peer a PTY call's trailing `engine` names (owner only); `None`:
/// the job is here, under the usual session admission.
pub(crate) async fn pty_peer(ext: &Extensions, engine: Option<&str>) -> RpcResult<Option<PeerId>> {
    let peer = forward::engine_peer(ext, engine).await?;
    if peer.is_some() {
        rpc_core::owner(ext)?;
    }
    Ok(peer)
}

// ---- subscribe_pty over the mesh ----

/// Where [`pump`] writes a PTY's items.
pub(crate) trait PtySink: Send {
    /// One item; `false` when the reader is gone.
    fn send(&mut self, c: &PtyChunk) -> impl Future<Output = bool> + Send;
    /// Resolves once the reader is gone.
    fn closed(&self) -> impl Future<Output = ()> + Send;
}

impl PtySink for SubscriptionSink {
    async fn send(&mut self, c: &PtyChunk) -> bool {
        let raw = serde_json::value::to_raw_value(c).expect("PtyChunk serializes");
        SubscriptionSink::send(self, SubscriptionMessage::from(raw))
            .await
            .is_ok()
    }
    async fn closed(&self) {
        SubscriptionSink::closed(self).await;
    }
}

/// The accept side's writer: JSON lines; `gone` once the opener closed.
struct Lines<W> {
    write: W,
    gone: CancellationToken,
}

impl<W: AsyncWrite + Unpin + Send> PtySink for Lines<W> {
    async fn send(&mut self, c: &PtyChunk) -> bool {
        write_line(&mut self.write, c).await.is_ok()
    }
    fn closed(&self) -> impl Future<Output = ()> + Send {
        self.gone.clone().cancelled_owned()
    }
}

/// Feed an attached PTY to `sink`: a `reset` with the replay, then every
/// byte as `data`, `exit` once it ends. A reader that falls behind the ring
/// gets a fresh `reset` from `reattach`. Ends when the sink closes.
pub(crate) async fn pump<S, A, F>(mut at: PtyAttached, mut reattach: A, mut sink: S)
where
    S: PtySink,
    A: FnMut() -> F,
    F: Future<Output = RpcResult<PtyAttached>>,
{
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    loop {
        let reset = PtyChunk::Reset {
            data: b64(&at.replay),
            rows: at.rows,
            cols: at.cols,
        };
        if !sink.send(&reset).await {
            return;
        }
        if let Some(exit) = at.exit {
            sink.send(&PtyChunk::Exit { exit }).await;
            return;
        }
        loop {
            let out = tokio::select! {
                () = sink.closed() => return,
                r = at.rx.recv() => r,
            };
            let chunk = match out {
                Ok(PtyOut::Data(bytes)) => PtyChunk::Data { data: b64(&bytes) },
                Ok(PtyOut::Exit(exit)) => {
                    sink.send(&PtyChunk::Exit { exit }).await;
                    return;
                }
                // Behind by more than the ring: start over from a snapshot.
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if !sink.send(&chunk).await {
                return;
            }
        }
        at = match reattach().await {
            Ok(a) => a,
            Err(_) => return,
        };
    }
}

/// The first line on `theuth/pty/1`: what to attach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOpen {
    /// Owning session on the accepting engine.
    pub session: SessionId,
    /// The PTY job.
    pub job: JobId,
    /// Resize to `rows`×`cols` first (both or neither).
    pub rows: Option<u16>,
    /// See `rows`.
    pub cols: Option<u16>,
}

/// The accept side's answer to [`PtyOpen`]: `{"ok":{}}`, then the items, or
/// `{"refused":{code,message,data}}` and the stream ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyAck {
    /// Attached; [`PtyChunk`] lines follow.
    Ok {},
    /// The attach error, verbatim.
    Refused {
        /// JSON-RPC error code.
        code: i32,
        /// Its message.
        message: String,
        /// Its data.
        data: Option<serde_json::Value>,
    },
}

impl PtyAck {
    fn refused(e: &ErrorObjectOwned) -> Self {
        Self::Refused {
            code: e.code(),
            message: e.message().to_owned(),
            data: e.data().and_then(|d| serde_json::from_str(d.get()).ok()),
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, v: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

async fn read_head<R: AsyncRead + Unpin>(r: &mut BufReader<R>) -> std::io::Result<String> {
    let mut line = String::new();
    (&mut *r).take(MAX_HEAD).read_line(&mut line).await?;
    Ok(line)
}

/// The opener's half over any byte stream: send `open`, wait for the ack
/// ([`forward::TIMEOUT`]). `Ok`: the item lines follow on the reader.
/// `Err(Ok(e))`: the peer refused with `e`. `Err(Err(why))`: no usable answer.
pub(crate) async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    open: &PtyOpen,
) -> Result<Opened<S>, Result<ErrorObjectOwned, String>> {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let ack = async {
        write_line(&mut write, open).await.map_err(|e| e.to_string())?;
        let line = read_head(&mut read).await.map_err(|e| e.to_string())?;
        if line.is_empty() {
            return Err("the engine closed the stream without answering".to_owned());
        }
        serde_json::from_str::<PtyAck>(line.trim_end()).map_err(|e| format!("malformed ack: {e}"))
    };
    let acked = tokio::time::timeout(forward::TIMEOUT, ack).await;
    match acked {
        Err(_) => Err(Err(format!("no answer within {}s", forward::TIMEOUT.as_secs()))),
        Ok(Err(why)) => Err(Err(why)),
        Ok(Ok(PtyAck::Ok {})) => Ok((read, write)),
        Ok(Ok(PtyAck::Refused { code, message, data })) => {
            Err(Ok(ErrorObjectOwned::owned(code, message, data)))
        }
    }
}

/// Relay the item lines after a successful [`handshake`] into `sink` until
/// `exit`, the peer ends the stream, or the sink closes (which drops the
/// stream, ending the peer's side).
pub(crate) async fn relay<R, W, S>(mut read: BufReader<R>, write: W, mut sink: S)
where
    R: AsyncRead + Unpin,
    S: PtySink,
{
    loop {
        let mut line = String::new();
        // `read_line` is not cancel-safe; it is only cancelled when the sink is gone.
        let got = tokio::select! {
            () = sink.closed() => break,
            r = read.read_line(&mut line) => r,
        };
        if !matches!(got, Ok(1..)) {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<PtyChunk>(line.trim_end()) else {
            break;
        };
        let exit = matches!(chunk, PtyChunk::Exit { .. });
        if !sink.send(&chunk).await || exit {
            break;
        }
    }
    drop(write);
}

/// The opener's halves of a stream after [`handshake`]: item lines to read,
/// and the write half that keeps the stream open.
pub(crate) type Opened<S> = (BufReader<tokio::io::ReadHalf<S>>, tokio::io::WriteHalf<S>);

/// `subscribe_pty` for a job on mesh peer `peer`: attached there, or that
/// engine's refusal / [`forward::OWNER_UNREACHABLE`].
pub(crate) async fn open(peer: &PeerId, open: PtyOpen) -> RpcResult<Opened<Box<dyn replica::ByteStream>>> {
    let Some(t) = forward::transport()
        .await
        .filter(|t| t.peers().contains(peer))
    else {
        return Err(forward::unreachable(peer.0.clone(), "not connected").await);
    };
    let stream = match t.open(peer, ALPN_PTY).await {
        Ok(s) => s,
        Err(e) => return Err(forward::unreachable(peer.0.clone(), &format!("{e:#}")).await),
    };
    match handshake(stream, &open).await {
        Ok(halves) => Ok(halves),
        Err(Ok(e)) => Err(e),
        Err(Err(why)) => Err(forward::unreachable(peer.0.clone(), &why).await),
    }
}

/// The accept side for one stream: read the [`PtyOpen`], attach with
/// `attach`, ack, then [`pump`] item lines until exit or the opener closes.
pub(crate) async fn answer<S, A, F>(stream: S, attach: A) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    A: Fn(SessionId, JobId, Option<(u16, u16)>) -> F,
    F: Future<Output = RpcResult<PtyAttached>>,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let line = tokio::time::timeout(forward::TIMEOUT, read_head(&mut read)).await??;
    let open: PtyOpen = serde_json::from_str(line.trim_end())?;
    let at = match attach(open.session, open.job.clone(), open.rows.zip(open.cols)).await {
        Ok(at) => at,
        Err(e) => {
            write_line(&mut write, &PtyAck::refused(&e)).await?;
            return Ok(());
        }
    };
    write_line(&mut write, &PtyAck::Ok {}).await?;
    // The opener sends nothing more: EOF (or any error) means it is gone.
    let gone = CancellationToken::new();
    let watch = gone.clone();
    tokio::spawn(async move {
        let mut sink = [0u8; 256];
        while matches!(read.read(&mut sink).await, Ok(1..)) {}
        watch.cancel();
    });
    let (session, job) = (open.session, open.job);
    let reattach = || attach(session, job.clone(), None);
    pump(at, reattach, Lines { write, gone: gone.clone() }).await;
    gone.cancel();
    Ok(())
}

/// Accept `theuth/pty/1` streams until shutdown: each attaches a local PTY
/// job for the opening peer (a paired engine of the owner, trusted as
/// `theuth/rpc/1` forwards are) and streams it.
pub async fn serve(rt: Runtime, transport: Arc<dyn Transport>) {
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_PTY) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                tokio::spawn(async move {
                    let attach = |session, job: JobId, size| async move {
                        attach_pty(session, &job, size).await
                    };
                    if let Err(e) = answer(stream, attach).await {
                        tracing::debug!(%peer, error = %e, "pty mesh stream ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "pty mesh accept");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

// ---- the desktop over the mesh ----

/// The file cloud-init writes beside the engine's data (`<data_dir>/desktop.json`).
pub const DESKTOP_FILE: &str = "desktop.json";
/// A tunnel listener with no new connection for this long closes.
const TUNNEL_IDLE: Duration = Duration::from_secs(10 * 60);

/// `<data_dir>/desktop.json`: `{"password", "geometry", "port"}`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct DesktopFile {
    password: String,
    #[serde(default)]
    geometry: Option<String>,
    /// websockify's port on 127.0.0.1.
    port: u16,
}

fn read_desktop(data_dir: &Path) -> Result<DesktopFile, String> {
    let path = data_dir.join(DESKTOP_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("this engine has no watchable desktop".into());
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let file: DesktopFile =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.port == 0 {
        return Err(format!("{}: no port", path.display()));
    }
    Ok(file)
}

/// The one address a `theuth/desktop/1` stream may reach: this engine's
/// websockify on loopback, at the port its own `desktop.json` names.
fn desktop_target(data_dir: &Path) -> Result<std::net::SocketAddr, String> {
    read_desktop(data_dir).map(|f| std::net::SocketAddr::from(([127, 0, 0, 1], f.port)))
}

fn ws_url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/websockify")
}

/// [`EngineDesktop`] of mesh peer `peer` for an in-process caller (cloud
/// nodes' `cloud_desktop`): its password and geometry, and `ws_url` a
/// loopback tunnel here to its websockify. `Err`: the peer's error message
/// (no desktop, not connected, …).
pub async fn desktop_access(peer: &str) -> Result<EngineDesktop, String> {
    desktop_via(&Extensions::new(), &PeerId(peer.to_owned()))
        .await
        .map_err(|e| e.message().to_owned())
}

async fn desktop_via(ext: &Extensions, peer: &PeerId) -> RpcResult<EngineDesktop> {
    let remote: EngineDesktop =
        forward::call_peer(ext, peer, "engine_desktop", (None::<String>,)).await?;
    let port = tunnel_port(peer).await.map_err(|e| failed(format!("desktop tunnel: {e}")))?;
    Ok(EngineDesktop {
        ws_url: ws_url(port),
        engine: peer.0.clone(),
        ..remote
    })
}

/// One live loopback listener per peer: its port and last use.
struct Tunnel {
    port: u16,
    touched: Arc<Mutex<Instant>>,
}

static TUNNELS: LazyLock<Mutex<HashMap<String, Tunnel>>> = LazyLock::new(Default::default);

/// The loopback port tunnelling to `peer`'s desktop: the live listener's, or
/// a fresh one on 127.0.0.1 that closes after [`TUNNEL_IDLE`] unused.
async fn tunnel_port(peer: &PeerId) -> std::io::Result<u16> {
    if let Some(t) = TUNNELS.lock().expect("tunnels").get(&peer.0) {
        *t.touched.lock().expect("touched") = Instant::now();
        return Ok(t.port);
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let touched = Arc::new(Mutex::new(Instant::now()));
    TUNNELS.lock().expect("tunnels").insert(
        peer.0.clone(),
        Tunnel { port, touched: touched.clone() },
    );
    let peer = peer.clone();
    tokio::spawn(async move {
        let to = peer.clone();
        let open = move || {
            let to = to.clone();
            async move {
                let t = forward::transport()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("the mesh is not running"))?;
                t.open(&to, ALPN_DESKTOP).await
            }
        };
        tunnel(listener, open, touched, TUNNEL_IDLE).await;
        let mut tunnels = TUNNELS.lock().expect("tunnels");
        if tunnels.get(&peer.0).is_some_and(|t| t.port == port) {
            tunnels.remove(&peer.0);
        }
    });
    Ok(port)
}

/// Accept loopback connections until none came (and nobody was handed the
/// port, `touched`) for `idle`; each is piped both ways through a stream
/// from `open`. Open connections outlive the listener.
async fn tunnel<O, F, S>(listener: tokio::net::TcpListener, open: O, touched: Arc<Mutex<Instant>>, idle: Duration)
where
    O: Fn() -> F,
    F: Future<Output = anyhow::Result<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    loop {
        let deadline = *touched.lock().expect("touched") + idle;
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((mut tcp, _)) = accepted else { continue };
                *touched.lock().expect("touched") = Instant::now();
                let stream = open();
                tokio::spawn(async move {
                    match stream.await {
                        Ok(mut s) => {
                            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut s).await;
                        }
                        Err(e) => tracing::debug!(error = %e, "desktop tunnel open"),
                    }
                });
            }
            () = tokio::time::sleep_until(deadline.into()) => {
                if touched.lock().expect("touched").elapsed() >= idle {
                    return;
                }
            }
        }
    }
}

/// The accept side for one stream: pipe it to this engine's websockify
/// ([`desktop_target`]; nothing else is reachable).
async fn answer_desktop<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, data_dir: &Path) -> anyhow::Result<()> {
    let target = desktop_target(data_dir).map_err(anyhow::Error::msg)?;
    let mut tcp = tokio::net::TcpStream::connect(target).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut tcp).await?;
    Ok(())
}

/// Accept `theuth/desktop/1` streams until shutdown from connected (paired)
/// peers and pipe each to this engine's websockify.
pub async fn serve_desktop(rt: Runtime, transport: Arc<dyn Transport>) {
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_DESKTOP) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                if !transport.peers().contains(&peer) {
                    tracing::warn!(%peer, "desktop stream from an unpaired peer refused");
                    continue;
                }
                let data_dir = rt.data_dir.clone();
                tokio::spawn(async move {
                    if let Err(e) = answer_desktop(stream, &data_dir).await {
                        tracing::debug!(%peer, error = %e, "desktop mesh stream ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "desktop mesh accept");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::JobExit;

    #[test]
    fn engine_param_routes_to_a_peer_only_when_it_names_another_engine() {
        let t = forward::engine_target;
        assert_eq!(t(None, false, "me"), None);
        assert_eq!(t(Some(""), false, "me"), None);
        assert_eq!(t(Some("me"), false, "me"), None);
        // A forwarded request never takes a second hop.
        assert_eq!(t(Some("node"), true, "me"), None);
        assert_eq!(t(Some("node"), false, "me"), Some(PeerId("node".into())));
    }

    #[test]
    fn login_shell_prefers_an_existing_absolute_shell() {
        let all = |_: &Path| true;
        let none = |_: &Path| false;
        let zsh = Some(PathBuf::from("/bin/zsh"));
        assert_eq!(login_shell(zsh.clone(), all), PathBuf::from("/bin/zsh"));
        assert_eq!(login_shell(Some("zsh".into()), all), PathBuf::from("/bin/bash"));
        assert_eq!(login_shell(None, all), PathBuf::from("/bin/bash"));
        assert_eq!(login_shell(zsh, none), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn logs_tail_the_newest_day_and_escape_controls() {
        let dir = tempfile::tempdir().unwrap();
        fn keep(t: &str) -> Cow<'_, str> {
            Cow::Borrowed(t)
        }
        assert_eq!(read_logs(&dir.path().join("missing"), 10, keep), EngineLogs::default());
        assert_eq!(read_logs(dir.path(), 10, keep), EngineLogs::default());
        std::fs::write(dir.path().join("theuth.log.2026-11-26"), "old\n").unwrap();
        let body: String = (0..300).map(|i| format!("line {i}\n")).collect();
        let newest = dir.path().join("theuth.log.2026-11-27");
        std::fs::write(&newest, format!("{body}bell \x07 tab\tend\n")).unwrap();
        std::fs::write(dir.path().join("other.log.2026-11-28"), "x\n").unwrap();
        let got = read_logs(dir.path(), 3, keep);
        assert_eq!(got.path.as_deref(), Some(newest.display().to_string().as_str()));
        assert_eq!(got.lines, ["line 298", "line 299", "bell \\u{7} tab\tend"]);
        assert_eq!(read_logs(dir.path(), 2000, keep).lines.len(), 301);
        assert!(read_logs(dir.path(), 0, keep).lines.is_empty());
    }

    #[test]
    fn log_lines_are_redacted_before_escaping() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("theuth.log.2026-11-27"),
            "INFO token=dop_v1_secret ok\nWARN plain\n",
        )
        .unwrap();
        fn redact(t: &str) -> Cow<'_, str> {
            t.replace("dop_v1_secret", "«secret»").into()
        }
        assert_eq!(
            read_logs(dir.path(), 10, redact).lines,
            ["INFO token=«secret» ok", "WARN plain"]
        );
    }

    fn attached(replay: &[u8], exit: Option<JobExit>) -> (PtyAttached, broadcast::Sender<PtyOut>) {
        let (tx, rx) = broadcast::channel(8);
        (PtyAttached { replay: replay.to_vec(), rows: 24, cols: 80, rx, exit }, tx)
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    /// A sink collecting items for the relay end.
    struct Collect(tokio::sync::mpsc::UnboundedSender<PtyChunk>);
    impl PtySink for Collect {
        async fn send(&mut self, c: &PtyChunk) -> bool {
            self.0.send(c.clone()).is_ok()
        }
        async fn closed(&self) {
            self.0.closed().await;
        }
    }

    fn open_req() -> PtyOpen {
        PtyOpen { session: SessionId::new(), job: JobId("j1".into()), rows: Some(30), cols: Some(100) }
    }

    #[tokio::test]
    async fn pty_relay_streams_reset_data_exit_over_a_byte_stream() {
        let (near, far) = tokio::io::duplex(4096);
        let (at, tx) = attached(b"prompt$ ", None);
        let slot = std::sync::Mutex::new(Some(at));
        let want = open_req();
        let server = tokio::spawn({
            let want = want.clone();
            async move {
                let slot = slot;
                answer(far, move |session, job, size| {
                    assert_eq!((session, &job, size), (want.session, &want.job, Some((30, 100))));
                    let at = slot.lock().unwrap().take();
                    async move { at.ok_or_else(|| failed("attached once")) }
                })
                .await
            }
        });
        let (read, write) = handshake(near, &want).await.expect("attached");
        let (sink_tx, mut items) = tokio::sync::mpsc::unbounded_channel();
        let client = tokio::spawn(relay(read, write, Collect(sink_tx)));
        let first = items.recv().await.unwrap();
        assert!(matches!(first, PtyChunk::Reset { ref data, rows: 24, cols: 80 } if *data == b64("prompt$ ")));
        tx.send(PtyOut::Data(Arc::from(&b"ls\r\n"[..]))).unwrap();
        tx.send(PtyOut::Exit(JobExit::Exited(0))).unwrap();
        assert!(matches!(items.recv().await.unwrap(), PtyChunk::Data { ref data } if *data == b64("ls\r\n")));
        assert!(matches!(items.recv().await.unwrap(), PtyChunk::Exit { exit: JobExit::Exited(0) }));
        client.await.unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn pty_relay_passes_the_refusal_verbatim() {
        let (near, far) = tokio::io::duplex(4096);
        let server = tokio::spawn(answer(far, |_, _, _| async {
            Err::<PtyAttached, _>(forward::runs_elsewhere("other".into()))
        }));
        let Err(Ok(e)) = handshake(near, &open_req()).await else {
            panic!("expected a refusal");
        };
        assert_eq!(e.code(), forward::RUNS_ELSEWHERE);
        assert!(e.data().is_some_and(|d| d.get().contains("other")));
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn pty_relay_ends_the_far_side_when_the_reader_leaves() {
        let (near, far) = tokio::io::duplex(4096);
        let (at, _tx) = attached(b"", None);
        let slot = std::sync::Mutex::new(Some(at));
        let server = tokio::spawn(answer(far, move |_, _, _| {
            let at = slot.lock().unwrap().take();
            async move { at.ok_or_else(|| failed("attached once")) }
        }));
        let (read, write) = handshake(near, &open_req()).await.expect("attached");
        let (sink_tx, mut items) = tokio::sync::mpsc::unbounded_channel();
        let client = tokio::spawn(relay(read, write, Collect(sink_tx)));
        assert!(matches!(items.recv().await.unwrap(), PtyChunk::Reset { .. }));
        drop(items);
        client.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the far side ends once the opener is gone")
            .unwrap()
            .unwrap();
    }

    #[test]
    fn desktop_json_parses_and_missing_means_no_desktop() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_desktop(dir.path()).unwrap_err(), "this engine has no watchable desktop");
        let file = dir.path().join(DESKTOP_FILE);
        std::fs::write(&file, r#"{"password":"abcd1234","geometry":"1440x900","port":6080}"#).unwrap();
        let d = read_desktop(dir.path()).unwrap();
        assert_eq!(
            d,
            DesktopFile { password: "abcd1234".into(), geometry: Some("1440x900".into()), port: 6080 }
        );
        assert_eq!(ws_url(d.port), "ws://127.0.0.1:6080/websockify");
        std::fs::write(&file, r#"{"password":"p","port":5901}"#).unwrap();
        assert_eq!(read_desktop(dir.path()).unwrap().geometry, None);
        for bad in [r#"{"password":"p","port":0}"#, r#"{"password":"p"}"#, "not json", r#"{"password":"p","port":70000}"#] {
            std::fs::write(&file, bad).unwrap();
            assert!(read_desktop(dir.path()).is_err(), "{bad}");
        }
    }

    #[test]
    fn desktop_stream_reaches_only_loopback_at_the_files_port() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DESKTOP_FILE), r#"{"password":"p","port":6080}"#).unwrap();
        assert_eq!(desktop_target(dir.path()).unwrap(), "127.0.0.1:6080".parse().unwrap());
    }

    /// A TCP echo server on loopback; its port.
    async fn echo() -> u16 {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = l.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = c.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn desktop_tunnel_pipes_bytes_both_ways_and_closes_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let port = echo().await;
        std::fs::write(
            dir.path().join(DESKTOP_FILE),
            format!(r#"{{"password":"p","port":{port}}}"#),
        )
        .unwrap();
        let data_dir = dir.path().to_path_buf();
        // The mesh stream pair is a duplex; the far half is the peer's accept side.
        let open = move || {
            let data_dir = data_dir.clone();
            async move {
                let (near, far) = tokio::io::duplex(4096);
                tokio::spawn(async move { answer_desktop(far, &data_dir).await });
                anyhow::Ok(near)
            }
        };
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let local = listener.local_addr().unwrap();
        let touched = Arc::new(Mutex::new(Instant::now()));
        let idle = Duration::from_millis(300);
        let tunnel = tokio::spawn(tunnel(listener, open, touched, idle));
        let mut c = tokio::net::TcpStream::connect(local).await.unwrap();
        c.write_all(b"RFB 003.008\n").await.unwrap();
        let mut got = [0u8; 12];
        c.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"RFB 003.008\n");
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), tunnel)
            .await
            .expect("the idle listener closes")
            .unwrap();
        assert!(tokio::net::TcpStream::connect(local).await.is_err());
    }
}
