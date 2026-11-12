use super::*;

#[rpc(server, client)]
pub trait McpApi {
    /// MCP servers, their tools, and their `mcp/config.yaml` risk lines; with
    /// a session, which tools it hid (`set_mcp_hidden`), answered by its owner.
    #[method(name = "list_mcp", with_extensions)]
    async fn list_mcp(&self, session: Option<SessionId>) -> RpcResult<Vec<McpServer>>;
    /// MCP connection progress snapshot (`McpProgress`), for a UI attaching after startup.
    #[method(name = "mcp_status")]
    async fn mcp_status(&self) -> RpcResult<McpProgress>;
    /// Set (or clear with `None`) a server or tool risk line
    /// (`read_only | mutating | destructive | off`); persists to `mcp/config.yaml`.
    #[method(name = "set_mcp_gate")]
    async fn set_mcp_gate(
        &self,
        server: String,
        tool: Option<String>,
        gate: Option<Gate>,
    ) -> RpcResult<()>;
    /// 187: where a server runs — `run_everywhere`, `only_home`, `move_here`,
    /// `off_here`, `on_here` (this engine); persists to `mcp/homes.yaml`, which replicates.
    #[method(name = "set_mcp_home")]
    async fn set_mcp_home(&self, server: String, action: McpHomeAction) -> RpcResult<()>;
    /// Write `mcp/<name>.json` and connect; returns the server with its tools.
    #[method(name = "add_mcp_server")]
    async fn add_mcp_server(&self, name: String, entry: serde_json::Value) -> RpcResult<McpServer>;
    /// Remove (or disable) an MCP server entry.
    #[method(name = "remove_mcp_server")]
    async fn remove_mcp_server(&self, name: String) -> RpcResult<()>;
    /// Clone a repo into `mcp/repos/<name>`.
    #[method(name = "add_mcp_repo")]
    async fn add_mcp_repo(&self, url: String, name: Option<String>) -> RpcResult<RepoInfo>;
    /// `git pull` every MCP repo, restarting servers whose repo changed.
    #[method(name = "sync_mcp")]
    async fn sync_mcp(&self) -> RpcResult<Vec<SyncResult>>;
    /// Start an MCP server login; returns the URL to open. The server reconnects on success (`McpProgress`).
    #[method(name = "mcp_login")]
    async fn mcp_login(&self, server: String) -> RpcResult<String>;
    /// Forget an MCP server login and restart it.
    #[method(name = "mcp_logout")]
    async fn mcp_logout(&self, server: String) -> RpcResult<()>;
    /// 158 §12.6 (W5): the MCP server's config, whether it is listening, URL and token.
    #[method(name = "mcp_server_get")]
    async fn mcp_server_get(&self) -> RpcResult<mcp_server::McpServerStatus>;
    /// 158 §12.2 (W5): persist `mcp_server` and bind or drop the listener live.
    #[method(name = "mcp_server_set")]
    async fn mcp_server_set(
        &self,
        config: mcp_server::McpServerConfig,
    ) -> RpcResult<mcp_server::McpServerStatus>;
    /// 158 §12.2 (W5): a fresh token into the vault; every open MCP session ends.
    #[method(name = "mcp_server_rotate_token")]
    async fn mcp_server_rotate_token(&self) -> RpcResult<()>;
    /// 158 §12.5 (W5): the last `limit` audit lines, newest first.
    #[method(name = "mcp_server_audit")]
    async fn mcp_server_audit(&self, limit: u32) -> RpcResult<Vec<mcp_server::AuditLine>>;
    /// 158 §12.3 (W5): one catalogue tool call for the `t mcp` stdio bridge, run in the engine
    /// with the vault's redaction and an audit line `transport: stdio`; `client` is the MCP
    /// `clientInfo.name`. Unknown tool / bad arguments → invalid params.
    #[method(name = "mcp_server_call")]
    async fn mcp_server_call(
        &self,
        tool: String,
        args: serde_json::Value,
        client: String,
    ) -> RpcResult<mcp_server::McpCallResult>;
}

#[async_trait]
impl McpApiServer for Server {

    async fn list_mcp(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<McpServer>> {
        self.authorize_opt_fwd(ext, session).await?;
        // The hides live in the owner's registry (`set_mcp_hidden` runs there):
        // read them there too, or a chat owned elsewhere reads back this
        // engine's (empty) set. Owner offline → `-32010`, never local state.
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "list_mcp", (session,)).await?
        {
            return Ok(r);
        }
        ask::<Mcp, _>(tools::MCP_ACTOR, ListServers { session }).await
    }

    async fn mcp_status(&self) -> RpcResult<McpProgress> {
        ask::<Mcp, _>(tools::MCP_ACTOR, Progress).await
    }

    async fn set_mcp_gate(
        &self,
        server: String,
        tool: Option<String>,
        gate: Option<Gate>,
    ) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, SetGate { server, tool, gate }).await
    }

    async fn set_mcp_home(&self, server: String, action: McpHomeAction) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, SetMcpHome { server, action }).await
    }

    async fn add_mcp_server(&self, name: String, entry: serde_json::Value) -> RpcResult<McpServer> {
        ask::<Mcp, _>(tools::MCP_ACTOR, AddServer { name, entry }).await
    }

    async fn remove_mcp_server(&self, name: String) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, RemoveServer { name }).await
    }

    async fn add_mcp_repo(&self, url: String, name: Option<String>) -> RpcResult<RepoInfo> {
        ask::<Mcp, _>(tools::MCP_ACTOR, AddRepo { url, name }).await
    }

    async fn sync_mcp(&self) -> RpcResult<Vec<SyncResult>> {
        ask::<Mcp, _>(tools::MCP_ACTOR, SyncMcp).await
    }

    async fn mcp_login(&self, server: String) -> RpcResult<String> {
        ask::<Mcp, _>(tools::MCP_ACTOR, McpLogin { server }).await
    }

    async fn mcp_logout(&self, server: String) -> RpcResult<()> {
        ask::<Mcp, _>(tools::MCP_ACTOR, McpLogout { server }).await
    }

    async fn mcp_server_get(&self) -> RpcResult<mcp_server::McpServerStatus> {
        mcp_server::status(&self.rt).await.map_err(stub_err)
    }

    async fn mcp_server_set(
        &self,
        config: mcp_server::McpServerConfig,
    ) -> RpcResult<mcp_server::McpServerStatus> {
        mcp_server::set(&self.rt, config).await.map_err(stub_err)
    }

    async fn mcp_server_rotate_token(&self) -> RpcResult<()> {
        mcp_server::rotate_token(&self.rt).await.map_err(stub_err)
    }

    async fn mcp_server_audit(&self, limit: u32) -> RpcResult<Vec<mcp_server::AuditLine>> {
        mcp_server::audit_tail(&self.rt, limit)
            .await
            .map_err(stub_err)
    }

    async fn mcp_server_call(
        &self,
        tool: String,
        args: serde_json::Value,
        client: String,
    ) -> RpcResult<mcp_server::McpCallResult> {
        mcp_server::call(&self.rt, tool, args, client)
            .await
            .map_err(|e| {
                ErrorObjectOwned::owned(
                    jsonrpsee::types::error::INVALID_PARAMS_CODE,
                    format!("{e:#}"),
                    None::<()>,
                )
            })
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    McpApiServer::into_rpc(server)
}
