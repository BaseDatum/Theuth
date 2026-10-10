//! theuth as an MCP server (design doc 158 §12): an rmcp streamable-HTTP listener on
//! `127.0.0.1:<mcp_server.port>` (default 7788), OFF by default, `Authorization: Bearer` with
//! the vault row `mcp.server.token`, Origin/Host rules against DNS rebinding (rmcp's own
//! `allowed_hosts` / `allowed_origins`), one catalogue ([`catalog`]) served over HTTP here and
//! over stdio by `t mcp`, and an append-only audit `<data_dir>/mcp/server-audit.jsonl`
//! ([`audit`]). The catalogue reaches the engine through [`Rpc`] — jsonrpsee's `ClientT`, so
//! `rpc::client::Local` (in-process) and `WsClient` (the stdio bridge) both fit without a line
//! of glue: this crate is a dependency of `rpc`, never the reverse. The engine injects its
//! client once with [`boot`]; `start`/`stop`/`set`/`rotate_token` then work on process state.

use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use jsonrpsee::core::ClientError;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::params::ArrayParams;
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub mod audit;
pub mod catalog;
pub mod http;

pub use audit::Audit;
pub use catalog::{Bridge, Catalog, McpCallResult};

/// Config section `mcp_server` (158 §15.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct McpServerConfig {
    /// Bind on engine start and keep serving; Settings › MCP flips it live.
    pub enabled: bool,
    /// Loopback port; RPC is 7777.
    pub port: u16,
}

/// 158 §12.2: beside RPC's 7777.
pub const DEFAULT_PORT: u16 = 7788;

/// Vault row holding the bearer token (158 §15.3).
pub const TOKEN_ROW: &str = "mcp.server.token";

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
        }
    }
}

/// `mcp_server_get` / `mcp_server_set` reply (158 §12.6): the config, whether the listener is
/// up, the URL a client pastes, and the token so Settings can copy the `mcpServers` block
/// (loopback-only; the UI never shows it unasked).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct McpServerStatus {
    /// As configured.
    pub enabled: bool,
    /// As configured.
    pub port: u16,
    /// A listener is bound right now.
    pub running: bool,
    /// `http://127.0.0.1:<port>/mcp`.
    pub url: String,
    /// Vault row `mcp.server.token`; `None` until one is generated.
    #[serde(default)]
    pub token: Option<String>,
    /// This engine's executable (`current_exe`), so the stdio block names the right `t` and not
    /// whatever PATH finds; `None` when the OS cannot tell.
    #[serde(default)]
    pub binary: Option<String>,
}

/// One line of `<data_dir>/mcp/server-audit.jsonl` (158 §12.5, W-j): args are hashed, never kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "McpAuditLine"))]
pub struct AuditLine {
    /// RFC 3339.
    pub at: String,
    /// `http` | `stdio`.
    pub transport: String,
    /// MCP `clientInfo.name`.
    pub client: String,
    /// Tool called.
    pub tool: String,
    /// SHA-256 of the canonical JSON args.
    pub args_sha256: String,
    /// The call returned a result (not an error).
    pub ok: bool,
    /// Wall time.
    pub ms: u64,
}

/// Why an engine call failed: the engine answered with an error (its message), or it could not
/// be reached. The catalogue shows both as tool errors; the stdio bridge keeps them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    /// A JSON-RPC error object: the engine's message.
    Call(String),
    /// Transport failure.
    Unreachable(String),
}

/// How the catalogue reaches the engine (158 §12.3): one positional JSON-RPC call.
#[async_trait::async_trait]
pub trait Rpc: Send + Sync + 'static {
    /// `method(params…)` on the engine; the result as JSON.
    async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, RpcError>;
}

/// Every jsonrpsee client is an [`Rpc`]: `rpc::client::Local` in the engine, `WsClient` in `t mcp`.
#[async_trait::async_trait]
impl<C: ClientT + Send + Sync + 'static> Rpc for C {
    async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, RpcError> {
        let mut array = ArrayParams::new();
        for p in params {
            array.insert(p).map_err(|e| RpcError::Call(e.to_string()))?;
        }
        self.request(method, array).await.map_err(|e| match e {
            ClientError::Call(e) => RpcError::Call(e.message().to_owned()),
            e => RpcError::Unreachable(e.to_string()),
        })
    }
}

/// The engine's client, set once by [`boot`].
static RPC: OnceLock<Arc<dyn Rpc>> = OnceLock::new();

/// Process state: the config as last applied (the file is read once at boot; `set` changes it
/// live) and the listener when one is bound.
struct State {
    cfg: McpServerConfig,
    running: Option<Running>,
}

struct Running {
    port: u16,
    cancel: CancellationToken,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(rt: &Runtime, f: impl FnOnce(&mut State) -> T) -> T {
    let mut g = STATE.lock();
    let state = g.get_or_insert_with(|| State {
        cfg: config(rt),
        running: None,
    });
    f(state)
}

/// The `mcp_server:` section as configured; defaults when absent.
pub fn config(rt: &Runtime) -> McpServerConfig {
    rt.section("mcp_server").unwrap_or_default()
}

/// The one engine boot hook: remember the in-process client, then [`start`] iff enabled. A bind
/// failure is a warning here (a second engine on the same data dir, 63 §6.4), an error in [`set`].
pub async fn boot(rt: &Runtime, rpc: Arc<dyn Rpc>) {
    let _ = RPC.set(rpc);
    if let Err(e) = start(rt).await {
        tracing::warn!(error = %e, "mcp server: not started");
    }
}

/// Bind the listener iff `mcp_server.enabled` (engine start, or after `set`). No-op when it is
/// already up on the configured port.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let cfg = with_state(rt, |s| s.cfg.clone());
    if !cfg.enabled {
        return Ok(());
    }
    if with_state(rt, |s| {
        s.running.as_ref().is_some_and(|r| r.port == cfg.port)
    }) {
        return Ok(());
    }
    let token = token(rt, false).await?;
    let catalog = catalog(rt, "http").await?;
    let cancel = CancellationToken::new();
    let port = http::serve(cfg.port, token, catalog, cancel.clone()).await?;
    // Replace (and so cancel) whatever was running on another port.
    with_state(rt, |s| s.running = Some(Running { port, cancel }));
    tracing::info!(port, "mcp server listening");
    Ok(())
}

/// The catalogue for `transport`, over the engine client [`boot`] installed and the vault.
async fn catalog(rt: &Runtime, transport: &'static str) -> anyhow::Result<Catalog> {
    let rpc = RPC
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("mcp server: no engine client installed (boot first)"))?;
    let vault = secrets::handle(rt)
        .await
        .ok_or_else(|| anyhow::anyhow!("secrets vault unavailable"))?;
    Ok(Catalog::new(
        rpc,
        transport,
        Audit::new(&rt.data_dir),
        vault,
    ))
}

/// `mcp_server_call(tool, args, client) -> McpCallResult` (158 §12.3): one catalogue call for the
/// `t mcp` stdio bridge, run here with the vault's redaction and an audit line `transport: stdio`.
/// `Err` = protocol error (unknown tool, bad arguments) or no engine client.
pub async fn call(
    rt: &Runtime,
    tool: String,
    args: Value,
    client: String,
) -> anyhow::Result<catalog::McpCallResult> {
    let args = match args {
        Value::Object(m) => m,
        Value::Null => Default::default(),
        _ => anyhow::bail!("args must be a JSON object"),
    };
    catalog(rt, "stdio")
        .await?
        .call(&tool, args, &client)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))
}

/// Drop the listener and close sessions with `DELETE` semantics (rmcp ends every session when
/// the cancellation token fires).
pub async fn stop(rt: &Runtime) -> anyhow::Result<()> {
    with_state(rt, |s| s.running = None);
    Ok(())
}

/// `mcp_server_get() -> McpServerStatus` (158 §15.2).
pub async fn status(rt: &Runtime) -> anyhow::Result<McpServerStatus> {
    let (cfg, running) = with_state(rt, |s| (s.cfg.clone(), s.running.is_some()));
    let token = match secrets::handle(rt).await {
        Some(v) => v.get(TOKEN_ROW, 0).map(|t| t.to_string()),
        None => None,
    };
    Ok(McpServerStatus {
        enabled: cfg.enabled,
        port: cfg.port,
        running,
        url: format!("http://127.0.0.1:{}/mcp", cfg.port),
        token,
        binary: std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
    })
}

/// `mcp_server_set(McpServerConfig) -> McpServerStatus`: persist with `write_config_value`,
/// bind or drop the listener.
pub async fn set(rt: &Runtime, config: McpServerConfig) -> anyhow::Result<McpServerStatus> {
    if config.port == 0 {
        anyhow::bail!("mcp_server.port must be 1..=65535");
    }
    let path = rt.data_dir.join("config.yaml");
    runtime::write_config_value(&path, "mcp_server", "enabled", &config.enabled.to_string())?;
    runtime::write_config_value(&path, "mcp_server", "port", &config.port.to_string())?;
    with_state(rt, |s| s.cfg = config.clone());
    if config.enabled {
        start(rt).await?;
    } else {
        stop(rt).await?;
    }
    status(rt).await
}

/// `mcp_server_rotate_token()`: a fresh 64-hex token into the vault; open sessions end (the
/// listener restarts with the new token).
pub async fn rotate_token(rt: &Runtime) -> anyhow::Result<()> {
    token(rt, true).await?;
    if with_state(rt, |s| s.running.take()).is_some() {
        start(rt).await?;
    }
    Ok(())
}

/// `mcp_server_audit(limit) -> Vec<AuditLine>`: the last `limit` lines, newest first.
pub async fn audit_tail(rt: &Runtime, limit: u32) -> anyhow::Result<Vec<AuditLine>> {
    Audit::new(&rt.data_dir).tail(limit as usize)
}

/// The vault row, created when missing (`fresh` = always minted): 64 hex chars like the RPC
/// token (`rpc/lib.rs:254-266`).
async fn token(rt: &Runtime, fresh: bool) -> anyhow::Result<String> {
    let vault = secrets::handle(rt)
        .await
        .ok_or_else(|| anyhow::anyhow!("secrets vault unavailable"))?;
    if !fresh && let Some(t) = vault.get(TOKEN_ROW, 0) {
        return Ok(t.to_string());
    }
    let t = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    vault
        .set(
            TOKEN_ROW,
            &t,
            Some("theuth MCP server bearer token"),
            "mcp-server",
        )
        .await
        .map_err(anyhow::Error::msg)?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 158 §15.3: an absent `mcp_server:` section is off on 7788.
    #[test]
    fn defaults_are_off_on_7788() {
        let cfg: McpServerConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, McpServerConfig::default());
        assert!(!cfg.enabled);
        assert_eq!(cfg.port, 7788);
    }
}
