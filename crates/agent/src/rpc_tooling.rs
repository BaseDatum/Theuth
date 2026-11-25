use crate as agent;
use agent::rpc_forward as forward;
use agent::rpc_support::RpcServerExt;
use agent::{Agents, ShowPrompt};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{Server, ask};
use runtime::SessionId;
use tools::{
    ApprovalsState, BuiltinCategory, BuiltinsSwitchError, DeleteToolCategory, ListBuiltins,
    ListToolCategories, SetApprovals, SetBuiltinsDisabled, SetDefaultToolCategories, SetHidden,
    SetMcpCategory, SetToolCategory, ToolCategoryInfo, ToolRegistry, UpsertToolCategory,
};
use uuid::Uuid;

/// 193 §7: the registry tools `session_call_tool` runs for an ACP child's MCP shim; the shim
/// lists exactly these (with the registry's own specs from `list_tools`).
pub const SESSION_TOOLS: [&str; 6] = [
    "read_file",
    "edit",
    "move_text",
    "copy_text",
    "indent",
    "clip",
];

/// Tooling RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentToolingApi {
    /// 193 §7: run one registry tool ([`SESSION_TOOLS`] only) in `session` — its cwd, approval
    /// policy and secret handling, through the registry's `Invoke` — for the `t mcp --session`
    /// shim. A name outside the allowlist (invalid params) or an authorize refusal is an RPC
    /// `Err`; the tool's own failure is the returned output with `is_error`.
    #[method(name = "session_call_tool", with_extensions)]
    async fn session_call_tool(
        &self,
        session: SessionId,
        name: String,
        args: serde_json::Value,
    ) -> RpcResult<tools::ToolOutput>;
    /// Hide (or show again) registered tools for one session and its
    /// subagents, in memory: absent from the model's tool list, `unknown tool`
    /// when called. An `mcp:<server>` entry hides every tool of that MCP server,
    /// including tools it registers after a reconnect or restart; a `cat:<id>`
    /// entry (206) hides every builtin of that category the same way (a call
    /// then says the tool is turned off). A running turn picks the change up
    /// before its next model call (prompt index, python runtime functions,
    /// direct tools). The persistent form is `set_mcp_gate(.., Some(Off))` for
    /// MCP tools and `set_builtins_disabled` for builtins.
    #[method(name = "set_mcp_hidden", with_extensions)]
    async fn set_mcp_hidden(
        &self,
        session: SessionId,
        tools: Vec<String>,
        hidden: bool,
    ) -> RpcResult<()>;
    /// 206: the harness's builtin tools by category (display order, tools sorted by
    /// name; empty categories left out), each with `disabled` (harness-wide,
    /// `tools.builtins.disabled`) and, with a session, `hidden` (its or an ancestor's
    /// `set_mcp_hidden` set, by name or `cat:<id>`), answered by the session's owner.
    #[method(name = "list_builtins", with_extensions)]
    async fn list_builtins(&self, session: Option<SessionId>) -> RpcResult<Vec<BuiltinCategory>>;
    /// List high-level tool categories. With `session`, visibility and the list itself are
    /// answered by that session's owner, as for `list_builtins`.
    #[method(name = "list_tool_categories", with_extensions)]
    async fn list_tool_categories(
        &self,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<ToolCategoryInfo>>;
    /// Create or update a custom tool category in this engine's `config.yaml`.
    #[method(name = "upsert_tool_category", with_extensions)]
    async fn upsert_tool_category(
        &self,
        id: Option<String>,
        label: String,
        description: Option<String>,
    ) -> RpcResult<ToolCategoryInfo>;
    /// Delete a custom tool category and its assignments and new-chat default.
    #[method(name = "delete_tool_category", with_extensions)]
    async fn delete_tool_category(&self, id: String) -> RpcResult<()>;
    /// Assign an MCP server to a tool category, or clear the assignment with `None`.
    #[method(name = "set_mcp_category", with_extensions)]
    async fn set_mcp_category(&self, server: String, category: Option<String>) -> RpcResult<()>;
    /// Override a native tool's category, or restore its default with `None`.
    #[method(name = "set_tool_category", with_extensions)]
    async fn set_tool_category(&self, tool: String, category: Option<String>) -> RpcResult<()>;
    /// Set the category hides persisted into brand-new root interactive sessions.
    #[method(name = "set_default_tool_categories", with_extensions)]
    async fn set_default_tool_categories(&self, disabled: Vec<String>) -> RpcResult<()>;
    /// 206: switch builtin tools off (`disabled: true`) or on again for every session
    /// on an engine; `names` are registered builtin names and `cat:<id>` keys (when
    /// switching on, also any name in the set, so a stale entry can go). With
    /// `session`, the engine that owns that session changes its set (forwarded as
    /// `list_builtins`); without, this engine. Persists to `tools.builtins.disabled` in
    /// that engine's `config.yaml` before applying; running turns pick it up before
    /// their next model call. A name that is neither is invalid params; a file that
    /// cannot be written is an internal error saying why. Either way nothing changes.
    #[method(name = "set_builtins_disabled", with_extensions)]
    async fn set_builtins_disabled(
        &self,
        names: Vec<String>,
        disabled: bool,
        session: Option<SessionId>,
    ) -> RpcResult<()>;
    /// Turn approval prompts on/off for `session`, or globally (persisted) with `None`.
    /// 183 §4.5 (W-4): on a session `enabled` pins `sessions.approvals` to
    /// `'on'`/`'off'`; `clear: true` drops the pin (inherit `tools.approvals`).
    #[method(name = "set_approvals", with_extensions)]
    async fn set_approvals(
        &self,
        session: Option<SessionId>,
        enabled: bool,
        clear: Option<bool>,
    ) -> RpcResult<ApprovalsState>;
    /// The assembled system prompt; without `session` the peers section is omitted.
    #[method(name = "get_prompt", with_extensions)]
    async fn get_prompt(&self, session: Option<SessionId>) -> RpcResult<String>;
}

#[async_trait]
impl AgentToolingApiServer for Server {
    async fn session_call_tool(
        &self,
        ext: &Extensions,
        session: SessionId,
        name: String,
        args: serde_json::Value,
    ) -> RpcResult<tools::ToolOutput> {
        if !SESSION_TOOLS.contains(&name.as_str()) {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                format!("`{name}` is not offered to a session's MCP shim"),
                None::<()>,
            ));
        }
        self.authorize(ext, session).await?;
        ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            tools::Invoke(tools::ToolCall {
                id: format!("mcp-{}", Uuid::now_v7()),
                name,
                args,
                session,
                quiet: false,
            }),
        )
        .await
    }

    async fn set_mcp_hidden(
        &self,
        ext: &Extensions,
        session: SessionId,
        tools: Vec<String>,
        hidden: bool,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "set_mcp_hidden", (session, &tools, hidden)).await?
        {
            return Ok(r);
        }
        ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            SetHidden {
                session,
                tools,
                hidden,
            },
        )
        .await
    }

    async fn list_builtins(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<BuiltinCategory>> {
        self.authorize_opt_fwd(ext, session).await?;
        // The hides live in the owner's registry (`set_mcp_hidden` runs there), as
        // for `list_mcp`: owner offline → `-32010`, never this engine's state.
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "list_builtins", (session,)).await?
        {
            return Ok(r);
        }
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, ListBuiltins(session)).await
    }

    async fn list_tool_categories(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<Vec<ToolCategoryInfo>> {
        self.authorize_opt_fwd(ext, session).await?;
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "list_tool_categories", (session,)).await?
        {
            return Ok(r);
        }
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, ListToolCategories(session)).await
    }

    async fn upsert_tool_category(
        &self,
        ext: &Extensions,
        id: Option<String>,
        label: String,
        description: Option<String>,
    ) -> RpcResult<ToolCategoryInfo> {
        self.authorize_opt_fwd(ext, None).await?;
        ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            UpsertToolCategory {
                id,
                label,
                description,
            },
        )
        .await
    }

    async fn delete_tool_category(&self, ext: &Extensions, id: String) -> RpcResult<()> {
        self.authorize_opt_fwd(ext, None).await?;
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, DeleteToolCategory { id }).await
    }

    async fn set_mcp_category(
        &self,
        ext: &Extensions,
        server: String,
        category: Option<String>,
    ) -> RpcResult<()> {
        self.authorize_opt_fwd(ext, None).await?;
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetMcpCategory { server, category }).await
    }

    async fn set_tool_category(
        &self,
        ext: &Extensions,
        tool: String,
        category: Option<String>,
    ) -> RpcResult<()> {
        self.authorize_opt_fwd(ext, None).await?;
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetToolCategory { tool, category }).await
    }

    async fn set_default_tool_categories(
        &self,
        ext: &Extensions,
        disabled: Vec<String>,
    ) -> RpcResult<()> {
        self.authorize_opt_fwd(ext, None).await?;
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetDefaultToolCategories { disabled }).await
    }

    async fn set_builtins_disabled(
        &self,
        ext: &Extensions,
        names: Vec<String>,
        disabled: bool,
        session: Option<SessionId>,
    ) -> RpcResult<()> {
        self.authorize_opt_fwd(ext, session).await?;
        // The set the owner lists is the owner's: change it there, as `set_mcp_hidden`.
        if let Some(s) = session
            && let Some(r) =
                forward::route(ext, s, "set_builtins_disabled", (&names, disabled, session)).await?
        {
            return Ok(r);
        }
        let registry = rpc_core::lookup::<ToolRegistry>(tools::REGISTRY_ACTOR)?;
        match registry.ask(SetBuiltinsDisabled { names, disabled }).await {
            Ok(()) => Ok(()),
            Err(kameo::error::SendError::HandlerError(e)) => {
                let code = match e {
                    BuiltinsSwitchError::Unknown(_) => jsonrpsee::types::error::INVALID_PARAMS_CODE,
                    BuiltinsSwitchError::NotWritten(_) => {
                        jsonrpsee::types::error::INTERNAL_ERROR_CODE
                    }
                };
                Err(ErrorObjectOwned::owned(code, e.to_string(), None::<()>))
            }
            Err(e) => Err(ErrorObjectOwned::owned(
                rpc_core::ASK_FAILED,
                e.to_string(),
                None::<()>,
            )),
        }
    }

    async fn set_approvals(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        enabled: bool,
        clear: Option<bool>,
    ) -> RpcResult<ApprovalsState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 §4.5: run on the owner, which persists the pin on the row. `clear`
        // travels only when set, so a pre-183 owner still answers the plain call.
        if let Some(s) = session {
            let forwarded = match clear {
                Some(c) => forward::route(ext, s, "set_approvals", (session, enabled, c)).await?,
                None => forward::route(ext, s, "set_approvals", (session, enabled)).await?,
            };
            if let Some(r) = forwarded {
                return Ok(r);
            }
        }
        ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            SetApprovals {
                session,
                enabled,
                clear: clear.unwrap_or(false),
            },
        )
        .await
    }

    async fn get_prompt(&self, ext: &Extensions, session: Option<SessionId>) -> RpcResult<String> {
        self.authorize_opt(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ShowPrompt { session }).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentToolingApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
