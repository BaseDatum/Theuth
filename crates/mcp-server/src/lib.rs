//! 158 W5 owns this file (stub from C158).
//!
//! theuth as an MCP server (design doc 158 §12): an rmcp streamable-HTTP listener on
//! `127.0.0.1:<mcp_server.port>` (default 7788), OFF by default, `Authorization: Bearer` with
//! the vault row `mcp.server.token`, Origin/Host rules against DNS rebinding, one catalogue
//! (`catalog.rs`, W5) served over HTTP here and over stdio by `t mcp`, and an append-only audit
//! `<data_dir>/mcp/server-audit.jsonl`. The catalogue reaches the engine through a `trait Rpc {
//! async fn call(method, params) }` that crates/theuth injects (in-process jsonrpsee `Methods`
//! for HTTP, `WsClient` for stdio): this crate is a dependency of `rpc`, never the reverse.
//!
//! Everything here is inert: no listener is bound, no token is generated, no file is written.

use runtime::Runtime;
use serde::{Deserialize, Serialize};

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

/// The `mcp_server:` section as configured; defaults when absent.
pub fn config(rt: &Runtime) -> McpServerConfig {
    rt.section("mcp_server").unwrap_or_default()
}

/// Bind the listener iff `mcp_server.enabled` (engine start, or after `set`). Stub: nothing binds.
pub async fn start(_rt: &Runtime) -> anyhow::Result<()> {
    Ok(())
}

/// Drop the listener and close sessions with `DELETE` semantics. Stub: nothing to drop.
pub async fn stop(_rt: &Runtime) -> anyhow::Result<()> {
    Ok(())
}

/// `mcp_server_get() -> McpServerStatus` (158 §15.2). Reads config only; never running before W5.
pub async fn status(rt: &Runtime) -> anyhow::Result<McpServerStatus> {
    let cfg = config(rt);
    Ok(McpServerStatus {
        enabled: cfg.enabled,
        port: cfg.port,
        running: false,
        url: format!("http://127.0.0.1:{}/mcp", cfg.port),
        token: None,
    })
}

/// `mcp_server_set(McpServerConfig) -> McpServerStatus`: persist with `write_config_value`,
/// bind or drop the listener. Stub: refused.
pub async fn set(_rt: &Runtime, _config: McpServerConfig) -> anyhow::Result<McpServerStatus> {
    Err(anyhow::anyhow!("158 W5: not built yet"))
}

/// `mcp_server_rotate_token()`: a fresh 64-hex token into the vault; open sessions end. Stub: refused.
pub async fn rotate_token(_rt: &Runtime) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("158 W5: not built yet"))
}

/// `mcp_server_audit(limit) -> Vec<AuditLine>`: the last `limit` lines, newest first. Stub: none.
pub async fn audit_tail(_rt: &Runtime, _limit: u32) -> anyhow::Result<Vec<AuditLine>> {
    Ok(Vec::new())
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
