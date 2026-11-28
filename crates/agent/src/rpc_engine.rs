//! The escape hatch on any engine of the mesh (224 §cloud nodes): an
//! interactive login shell (in-memory, outside any session or job), the engine
//! log and the watchable desktop, here or on a peer by its id, no SSH and no
//! inbound ports.
//!
//! `engine_shell` / `shell_input` / `shell_resize` / `shell_close` /
//! `engine_logs` / `engine_desktop` run here when `engine` is unset, empty or
//! this engine, else forward over `theuth/rpc/1` to that peer
//! ([`forward::engine_peer`]). Both `subscribe_pty` (a session's PTY job) and
//! `subscribe_shell` (an in-memory engine shell) for a peer ride their own
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
use crate::rpc_support::attach_pty;
use base64::Engine as _;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage, SubscriptionSink};
use jsonrpsee::types::ErrorObjectOwned;
use replica::{ALPN_DESKTOP, ALPN_PTY, PeerId, Transport};
use rpc_core::{Server, ask, failed};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tools::shell_registry::{self, DEFAULT_SHELL_ID, login_shell, shell_cwd};
use tools::{JobExit, JobId, Key, PtyAttached, PtyChunk, PtyOut};

/// `engine_logs` lines when `tail` is unset.
pub const LOG_TAIL_DEFAULT: u32 = 200;
/// Most `engine_logs` lines.
pub const LOG_TAIL_MAX: u32 = 2000;
/// Longest [`PtyOpen`] / [`PtyAck`] line.
const MAX_HEAD: u64 = 64 * 1024;

/// Deprecated return type of `engine_terminal` (kept only for wire-schema
/// stability; `engine_terminal` now errors and directs callers to `engine_shell`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineTerminal {
    /// Deprecated: owner root session id.
    pub session: SessionId,
    /// Deprecated: PTY job id.
    pub job: JobId,
    /// Mesh id of the engine.
    pub engine: String,
}

/// An interactive login shell on an engine, outside any session or job: attach
/// with `subscribe_shell(shell, rows, cols, engine)`, type with `shell_input`,
/// resize with `shell_resize`, end with `shell_close`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineShell {
    /// Shell id in the engine's in-memory registry (default `"main"`).
    pub shell: String,
    /// Mesh id of the engine running it (pass it back as `engine`).
    pub engine: String,
}

/// One `subscribe_shell` item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum ShellChunk {
    /// Start over: reset the terminal to `rows`×`cols` and write `data` (base64).
    Reset { data: String, rows: u16, cols: u16 },
    /// Shell output (base64), in order.
    Data { data: String },
    /// The shell exited; the subscription closes.
    Exit { exit: JobExit },
    /// The mesh stream to the remote engine dropped before the shell exited.
    Disconnected { reason: String },
}

impl From<&PtyChunk> for ShellChunk {
    fn from(c: &PtyChunk) -> Self {
        match c {
            PtyChunk::Reset { data, rows, cols } => Self::Reset {
                data: data.clone(),
                rows: *rows,
                cols: *cols,
            },
            PtyChunk::Data { data } => Self::Data { data: data.clone() },
            PtyChunk::Exit { exit } => Self::Exit { exit: exit.clone() },
        }
    }
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
    /// Deprecated: `engine_terminal` no longer creates a session or job; returns
    /// an error directing callers to `engine_shell`. Owner only.
    #[method(name = "engine_terminal", with_extensions)]
    async fn engine_terminal(&self, engine: Option<String>) -> RpcResult<EngineTerminal>;
    /// Open (or reuse) an interactive login shell (`$SHELL -l`, else `bash -l`)
    /// in `engine`'s in-memory shell registry — no session, no job. `shell`
    /// defaults to `"main"`. Owner only.
    #[method(name = "engine_shell", with_extensions)]
    async fn engine_shell(
        &self,
        engine: Option<String>,
        shell: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> RpcResult<EngineShell>;
    /// List open in-memory shell ids on `engine` (default: this engine) — never
    /// creates a shell, session, or job. Owner only.
    #[method(name = "shells_list", with_extensions)]
    async fn shells_list(&self, engine: Option<String>) -> RpcResult<Vec<String>>;
    /// Type into an in-memory engine shell: `text` verbatim, then `keys`
    /// encoded for the shell's current terminal modes. Owner only.
    #[method(name = "shell_input", with_extensions)]
    async fn shell_input(
        &self,
        shell: String,
        text: Option<String>,
        keys: Option<Vec<Key>>,
        engine: Option<String>,
    ) -> RpcResult<()>;
    /// Resize an in-memory engine shell's grid (SIGWINCH). Rows 2–200,
    /// columns 10–400. Owner only.
    #[method(name = "shell_resize", with_extensions)]
    async fn shell_resize(
        &self,
        shell: String,
        rows: u16,
        cols: u16,
        engine: Option<String>,
    ) -> RpcResult<()>;
    /// End an in-memory engine shell (SIGHUP its process group and remove it
    /// from the registry). Idempotent. Owner only.
    #[method(name = "shell_close", with_extensions)]
    async fn shell_close(&self, shell: String, engine: Option<String>) -> RpcResult<()>;
    /// Attach a terminal emulator to an in-memory engine shell: first a `reset`
    /// replay, then every byte as `data`, `exit` when it ends, or `disconnected`
    /// if the mesh stream drops. `rows`/`cols` resize before the snapshot.
    /// Owner only.
    #[subscription(name = "subscribe_shell", unsubscribe = "unsubscribe_shell", item = ShellChunk, with_extensions)]
    async fn subscribe_shell(
        &self,
        shell: String,
        rows: Option<u16>,
        cols: Option<u16>,
        engine: Option<String>,
    ) -> SubscriptionResult;
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

/// Open or reuse a login shell in `registry` for this engine (never touches
/// sessions or Jobs).
fn open_local_shell(
    registry: &shell_registry::ShellRegistry,
    data_dir: &Path,
    shell: Option<&str>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> RpcResult<String> {
    let id = shell
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SHELL_ID);
    let program = login_shell(std::env::var_os("SHELL").map(PathBuf::from), Path::exists);
    let cwd = shell_cwd(data_dir);
    let argv = [String::from("-l")];
    registry
        .open_or_reuse(id, (&program, &argv), &cwd, cols, rows)
        .map_err(failed)
}

/// List open shells in `registry` locally (`peer` is `None`), or forward
/// `shells_list` over `theuth/rpc/1` to `peer` (same forwarding path as
/// `engine_shell`). Never creates a shell, session, or job.
pub(crate) async fn list_shells_in(
    registry: &shell_registry::ShellRegistry,
    transport: Option<Arc<dyn Transport>>,
    peer: Option<&PeerId>,
) -> RpcResult<Vec<String>> {
    if let Some(peer) = peer {
        return match forward::to_peer_via(
            &Extensions::new(),
            transport,
            peer,
            "shells_list",
            (None::<String>,),
            forward::TIMEOUT,
        )
        .await?
        {
            Some(ids) => Ok(ids),
            None => Err(forward::unreachable(peer.0.clone(), "not connected").await),
        };
    }
    Ok(registry.list())
}

/// Type `text` / `keys` into open shell `shell` in `registry` locally (`peer`
/// is `None`), or forward `shell_input` over `theuth/rpc/1` to `peer`. Never
/// creates a shell, session, or job.
pub(crate) async fn send_shell_input_in(
    registry: &shell_registry::ShellRegistry,
    transport: Option<Arc<dyn Transport>>,
    peer: Option<&PeerId>,
    shell: &str,
    text: Option<&str>,
    keys: Option<&[Key]>,
) -> RpcResult<()> {
    if let Some(peer) = peer {
        return match forward::to_peer_via(
            &Extensions::new(),
            transport,
            peer,
            "shell_input",
            (shell, &text, &keys),
            forward::TIMEOUT,
        )
        .await?
        {
            Some(()) => Ok(()),
            None => Err(forward::unreachable(peer.0.clone(), "not connected").await),
        };
    }
    registry
        .input(shell, text, keys.unwrap_or(&[]))
        .map_err(failed)
}

#[async_trait]
impl EngineApiServer for Server {
    async fn engine_terminal(
        &self,
        ext: &Extensions,
        _engine: Option<String>,
    ) -> RpcResult<EngineTerminal> {
        rpc_core::owner(ext)?;
        Err(failed(
            "engine_terminal has been replaced by engine_shell (standalone node terminals without sessions or jobs); update this caller",
        ))
    }

    async fn engine_shell(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        shell: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> RpcResult<EngineShell> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            let mut res: EngineShell = forward::call_peer(
                ext,
                &peer,
                "engine_shell",
                (None::<String>, shell, cols, rows),
            )
            .await?;
            res.engine = peer.0;
            return Ok(res);
        }
        let store = ask::<crate::Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
        let me = store.engine().to_owned();
        let id = open_local_shell(
            shell_registry::global(),
            &self.rt.data_dir,
            shell.as_deref(),
            cols,
            rows,
        )?;
        Ok(EngineShell { shell: id, engine: me })
    }

    async fn shells_list(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<Vec<String>> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            return forward::call_peer(ext, &peer, "shells_list", (None::<String>,)).await;
        }
        Ok(shell_registry::global().list())
    }

    async fn shell_input(
        &self,
        ext: &Extensions,
        shell: String,
        text: Option<String>,
        keys: Option<Vec<Key>>,
        engine: Option<String>,
    ) -> RpcResult<()> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            return forward::call_peer(ext, &peer, "shell_input", (&shell, &text, &keys)).await;
        }
        shell_registry::global()
            .input(&shell, text.as_deref(), keys.as_deref().unwrap_or(&[]))
            .map_err(failed)
    }

    async fn shell_resize(
        &self,
        ext: &Extensions,
        shell: String,
        rows: u16,
        cols: u16,
        engine: Option<String>,
    ) -> RpcResult<()> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            return forward::call_peer(ext, &peer, "shell_resize", (&shell, rows, cols)).await;
        }
        shell_registry::global()
            .resize(&shell, rows, cols)
            .map_err(failed)
    }

    async fn shell_close(
        &self,
        ext: &Extensions,
        shell: String,
        engine: Option<String>,
    ) -> RpcResult<()> {
        rpc_core::owner(ext)?;
        if let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? {
            return forward::call_peer(ext, &peer, "shell_close", (&shell,)).await;
        }
        shell_registry::global().close(&shell);
        Ok(())
    }

    async fn subscribe_shell(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        shell: String,
        rows: Option<u16>,
        cols: Option<u16>,
        engine: Option<String>,
    ) -> SubscriptionResult {
        if let Err(e) = rpc_core::owner(ext) {
            pending.reject(e).await;
            return Ok(());
        }
        let remote = async {
            let Some(peer) = forward::engine_peer(ext, engine.as_deref()).await? else {
                return Ok(None);
            };
            let req = PtyOpen::Shell {
                shell: shell.clone(),
                rows,
                cols,
            };
            open(&peer, req).await.map(Some)
        };
        let remote = match remote.await {
            Ok(r) => r,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        if let Some((read, write)) = remote {
            let sink = pending.accept().await?;
            tokio::spawn(relay_shell(read, write, ShellSub(sink)));
            return Ok(());
        }
        let at = match shell_registry::global()
            .attach(&shell, rows.zip(cols))
            .map_err(failed)
        {
            Ok(a) => a,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        let reattach = move || {
            let shell = shell.clone();
            async move {
                shell_registry::global()
                    .attach(&shell, None)
                    .map_err(failed)
            }
        };
        tokio::spawn(pump(at, reattach, ShellSub(sink)));
        Ok(())
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

/// Where [`relay_shell`] writes [`ShellChunk`] items.
pub(crate) trait ShellSink: Send {
    /// One item; `false` when the reader is gone.
    fn send_shell(&mut self, c: &ShellChunk) -> impl Future<Output = bool> + Send;
    /// Resolves once the reader is gone.
    fn closed(&self) -> impl Future<Output = ()> + Send;
}

/// Subscription sink adapter for `subscribe_shell`: converts [`PtyChunk`] from
/// [`pump`] into [`ShellChunk`] and sends [`ShellChunk`] directly from
/// [`relay_shell`].
struct ShellSub(SubscriptionSink);

impl ShellSink for ShellSub {
    async fn send_shell(&mut self, c: &ShellChunk) -> bool {
        let raw = serde_json::value::to_raw_value(c).expect("ShellChunk serializes");
        SubscriptionSink::send(&self.0, SubscriptionMessage::from(raw))
            .await
            .is_ok()
    }
    async fn closed(&self) {
        SubscriptionSink::closed(&self.0).await;
    }
}

impl PtySink for ShellSub {
    async fn send(&mut self, c: &PtyChunk) -> bool {
        self.send_shell(&ShellChunk::from(c)).await
    }
    async fn closed(&self) {
        SubscriptionSink::closed(&self.0).await;
    }
}

/// The accept side's writer: JSON lines; `gone` once the opener closed.
struct Lines<W> {
    write: W,
    gone: CancellationToken,
}

impl<W: AsyncWrite + Unpin + Send> PtySink for &mut Lines<W> {
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
///
/// `Job` carries `{session, job, rows?, cols?}` (identical to the legacy struct
/// so older peers understand it); `Shell` carries `{shell, rows?, cols?}` for a
/// standalone in-memory shell outside any session or job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PtyOpen {
    /// A standalone in-memory shell in the accepting engine's shell registry.
    Shell {
        /// Shell id (default `"main"`).
        shell: String,
        /// Resize to `rows`×`cols` first (both or neither).
        rows: Option<u16>,
        /// See `rows`.
        cols: Option<u16>,
    },
    /// A session's PTY job on the accepting engine.
    Job {
        /// Owning session on the accepting engine.
        session: SessionId,
        /// The PTY job.
        job: JobId,
        /// Resize to `rows`×`cols` first (both or neither).
        rows: Option<u16>,
        /// See `rows`.
        cols: Option<u16>,
    },
}

impl PtyOpen {
    fn without_size(&self) -> Self {
        match self {
            Self::Shell { shell, .. } => Self::Shell {
                shell: shell.clone(),
                rows: None,
                cols: None,
            },
            Self::Job { session, job, .. } => Self::Job {
                session: *session,
                job: job.clone(),
                rows: None,
                cols: None,
            },
        }
    }
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

/// Relay a remote shell's `theuth/pty/1` stream into `sink`, emitting
/// [`ShellChunk::Disconnected`] if the stream ends before `exit` while the
/// subscriber is still listening.
pub(crate) async fn relay_shell<R, W, S>(mut read: BufReader<R>, write: W, mut sink: S)
where
    R: AsyncRead + Unpin,
    S: ShellSink,
{
    let mut saw_exit = false;
    let mut sink_closed = false;
    let mut drop_reason: Option<String> = None;
    loop {
        let mut line = String::new();
        let got = tokio::select! {
            () = sink.closed() => {
                sink_closed = true;
                break;
            }
            r = read.read_line(&mut line) => r,
        };
        match got {
            Ok(0) => {
                drop_reason = Some("connection to node lost".into());
                break;
            }
            Err(e) => {
                drop_reason = Some(format!("connection to node lost: {e}"));
                break;
            }
            Ok(_) => {}
        }
        let Ok(chunk) = serde_json::from_str::<PtyChunk>(line.trim_end()) else {
            drop_reason = Some("malformed terminal stream from node".into());
            break;
        };
        let exit = matches!(chunk, PtyChunk::Exit { .. });
        if !sink.send_shell(&ShellChunk::from(&chunk)).await {
            sink_closed = true;
            break;
        }
        if exit {
            saw_exit = true;
            break;
        }
    }
    if !saw_exit && !sink_closed {
        let reason = drop_reason.unwrap_or_else(|| "connection to node lost".into());
        let _ = sink.send_shell(&ShellChunk::Disconnected { reason }).await;
    }
    drop(write);
}

/// The opener's halves of a stream after [`handshake`]: item lines to read,
/// and the write half that keeps the stream open.
pub(crate) type Opened<S> = (BufReader<tokio::io::ReadHalf<S>>, tokio::io::WriteHalf<S>);

/// `subscribe_pty` / `subscribe_shell` on mesh peer `peer`: attached there, or
/// that engine's refusal / [`forward::OWNER_UNREACHABLE`].
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
    A: Fn(PtyOpen) -> F,
    F: Future<Output = RpcResult<PtyAttached>>,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let line = tokio::time::timeout(forward::TIMEOUT, read_head(&mut read)).await??;
    let open: PtyOpen = serde_json::from_str(line.trim_end())?;
    let at = match attach(open.clone()).await {
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
    let watch_task = tokio::spawn(async move {
        let mut sink = [0u8; 256];
        while matches!(read.read(&mut sink).await, Ok(1..)) {}
        watch.cancel();
    });
    let reattach_req = open.without_size();
    let reattach = || attach(reattach_req.clone());
    let mut lines = Lines { write, gone: gone.clone() };
    pump(at, reattach, &mut lines).await;
    gone.cancel();
    let _ = lines.write.shutdown().await;
    watch_task.abort();
    Ok(())
}

/// Accept `theuth/pty/1` streams until shutdown from connected (paired) peers:
/// each attaches a local PTY job or standalone in-memory shell for the opening
/// peer (a paired engine of the owner, trusted as `theuth/rpc/1` forwards are)
/// and streams it.
pub async fn serve(rt: Runtime, transport: Arc<dyn Transport>) {
    loop {
        let accepted = tokio::select! {
            () = rt.shutdown.cancelled() => return,
            a = transport.accept(ALPN_PTY) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                if !transport.peers().contains(&peer) {
                    tracing::warn!(%peer, "pty stream from an unpaired peer refused");
                    continue;
                }
                tokio::spawn(async move {
                    let attach = |open: PtyOpen| async move {
                        match open {
                            PtyOpen::Job {
                                session,
                                job,
                                rows,
                                cols,
                            } => attach_pty(session, &job, rows.zip(cols)).await,
                            PtyOpen::Shell { shell, rows, cols } => shell_registry::global()
                                .attach(&shell, rows.zip(cols))
                                .map_err(failed),
                        }
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

    /// A sink collecting `ShellChunk` items for `relay_shell`.
    struct CollectShell(tokio::sync::mpsc::UnboundedSender<ShellChunk>);
    impl ShellSink for CollectShell {
        async fn send_shell(&mut self, c: &ShellChunk) -> bool {
            self.0.send(c.clone()).is_ok()
        }
        async fn closed(&self) {
            self.0.closed().await;
        }
    }

    fn open_req() -> PtyOpen {
        PtyOpen::Job {
            session: SessionId::new(),
            job: JobId("j1".into()),
            rows: Some(30),
            cols: Some(100),
        }
    }

    fn open_shell_req() -> PtyOpen {
        PtyOpen::Shell {
            shell: "main".into(),
            rows: Some(30),
            cols: Some(100),
        }
    }

    #[test]
    fn pty_open_wire_format_separates_job_and_shell() {
        let job_req = open_req();
        let job_json = serde_json::to_string(&job_req).unwrap();
        assert!(job_json.contains("\"session\""), "{job_json}");
        assert!(job_json.contains("\"job\""), "{job_json}");
        assert!(!job_json.contains("\"shell\""), "{job_json}");
        assert_eq!(serde_json::from_str::<PtyOpen>(&job_json).unwrap(), job_req);

        let shell_req = open_shell_req();
        let shell_json = serde_json::to_string(&shell_req).unwrap();
        assert!(shell_json.contains("\"shell\":\"main\""), "{shell_json}");
        assert!(!shell_json.contains("\"session\""), "{shell_json}");
        assert!(!shell_json.contains("\"job\""), "{shell_json}");
        assert_eq!(serde_json::from_str::<PtyOpen>(&shell_json).unwrap(), shell_req);
    }

    #[tokio::test]
    async fn engine_terminal_refuses_and_never_creates_a_session_or_job() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::new(Runtime::new(Default::default(), dir.path().to_path_buf()));
        // Owner call returns a clear migration error (no Agents / Jobs actors
        // are even started in this test, proving neither owner_root nor StartPty runs).
        let (owner_peer, _owner_guard) = rpc_core::RpcPeer::open();
        let mut owner_ext = Extensions::new();
        owner_ext.insert(owner_peer);
        let err = EngineApiServer::engine_terminal(&server, &owner_ext, None)
            .await
            .expect_err("engine_terminal must refuse");
        assert!(err.message().contains("engine_shell"), "{}", err.message());

        // Owner shells_list runs purely against the in-memory registry without
        // needing Agents or Jobs actors.
        let _ = EngineApiServer::shells_list(&server, &owner_ext, None)
            .await
            .expect("owner shells_list succeeds without sessions or jobs");

        // Missing peer (unauthenticated) is rejected.
        let unauth = Extensions::new();
        assert_eq!(
            EngineApiServer::engine_shell(&server, &unauth, None, None, None, None)
                .await
                .expect_err("unauthenticated engine_shell must be rejected")
                .code(),
            rpc_core::UNAUTHORIZED
        );

        // Guest peer is rejected on every new shell RPC.
        let (guest_peer, _guest_guard) = rpc_core::RpcPeer::open_guest([1; 32], [2; 32]);
        let mut non_owner = Extensions::new();
        non_owner.insert(guest_peer);
        let err = EngineApiServer::engine_shell(&server, &non_owner, None, None, None, None)
            .await
            .expect_err("non-owner engine_shell must be rejected");
        assert_eq!(err.code(), rpc_core::UNAUTHORIZED);
        let err = EngineApiServer::shells_list(&server, &non_owner, None)
            .await
            .expect_err("non-owner shells_list must be rejected");
        assert_eq!(err.code(), rpc_core::UNAUTHORIZED);
        let err = EngineApiServer::shell_input(&server, &non_owner, "main".into(), Some("hi".into()), None, None)
            .await
            .expect_err("non-owner shell_input must be rejected");
        assert_eq!(err.code(), rpc_core::UNAUTHORIZED);
        let err = EngineApiServer::shell_resize(&server, &non_owner, "main".into(), 24, 80, None)
            .await
            .expect_err("non-owner shell_resize must be rejected");
        assert_eq!(err.code(), rpc_core::UNAUTHORIZED);
        let err = EngineApiServer::shell_close(&server, &non_owner, "main".into(), None)
            .await
            .expect_err("non-owner shell_close must be rejected");
        assert_eq!(err.code(), rpc_core::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn open_local_shell_runs_without_jobs_or_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let reg = shell_registry::ShellRegistry::new();
        let id = open_local_shell(&reg, dir.path(), None, Some(80), Some(24)).unwrap();
        assert_eq!(id, DEFAULT_SHELL_ID);
        let reused = open_local_shell(&reg, dir.path(), Some("main"), None, None).unwrap();
        assert_eq!(reused, DEFAULT_SHELL_ID);
        reg.close(&id);
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
                answer(far, move |got| {
                    assert_eq!(got, want);
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
    async fn shell_relay_streams_shell_open_and_emits_disconnected_on_stream_drop() {
        let (near, far) = tokio::io::duplex(4096);
        let (at, tx) = attached(b"hello$ ", None);
        let slot = std::sync::Mutex::new(Some(at));
        let want = open_shell_req();
        let server = tokio::spawn({
            let want = want.clone();
            async move {
                let slot = slot;
                answer(far, move |got| {
                    assert_eq!(got, want);
                    let at = slot.lock().unwrap().take();
                    async move { at.ok_or_else(|| failed("attached once")) }
                })
                .await
            }
        });
        let (read, write) = handshake(near, &want).await.expect("attached");
        let (sink_tx, mut items) = tokio::sync::mpsc::unbounded_channel();
        let client = tokio::spawn(relay_shell(read, write, CollectShell(sink_tx)));
        let first = tokio::time::timeout(Duration::from_secs(5), items.recv())
            .await
            .expect("reset arrives")
            .unwrap();
        assert!(matches!(
            first,
            ShellChunk::Reset { ref data, rows: 24, cols: 80 } if *data == b64("hello$ ")
        ));
        // Drop the far side's broadcast sender without an Exit event to
        // simulate a sudden stream drop.
        drop(tx);
        let dropped = tokio::time::timeout(Duration::from_secs(5), items.recv())
            .await
            .expect("disconnected arrives on stream drop")
            .unwrap();
        assert!(
            matches!(dropped, ShellChunk::Disconnected { .. }),
            "expected Disconnected on stream drop, got {dropped:?}"
        );
        tokio::time::timeout(Duration::from_secs(5), client)
            .await
            .expect("client ends")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server ends")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn pty_relay_passes_the_refusal_verbatim() {
        let (near, far) = tokio::io::duplex(4096);
        let server = tokio::spawn(answer(far, |_| async {
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
        let server = tokio::spawn(answer(far, move |_| {
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
