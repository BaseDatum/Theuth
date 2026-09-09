//! Tools plugin: the registry every tool lives in, the single approval gate
//! every invocation passes through, builtin tools, and MCP-sourced tools
//! (drop-in `<data_dir>/mcp/*.json`, hot-reloaded).
//!
//! Public surface consumed by other plugins:
//! - commands on the `"tools"` actor ([`ToolRegistry`]): [`ListTools`], [`Invoke`], [`RegisterTool`], [`UnregisterTool`]
//! - commands on the `"approval"` actor ([`Approval`]): [`Resolve`]
//! - commands on the `"mcp"` actor ([`Mcp`]): [`ListServers`], [`SetGate`]
//! - events: [`ApprovalRequested`], [`ApprovalResolved`]

use std::sync::Arc;

use futures::future::BoxFuture;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod approval;
pub mod fs;
pub mod mcp;
pub mod registry;
pub mod shell;

pub use approval::{Approval, ApprovalId, ApprovalRequested, ApprovalResolved, Decision, Resolve};
pub use mcp::{ListServers, Mcp, McpServer, McpToolInfo, SetGate};
pub use registry::{
    ApprovalsState, Invoke, ListTools, RegisterTool, SetApprovals, SetParent, ToolRegistry,
    UnregisterTool,
};

/// Registry name of the [`ToolRegistry`] actor.
pub const REGISTRY_ACTOR: &str = "tools";
/// Registry name of the [`Approval`] actor.
pub const APPROVAL_ACTOR: &str = "approval";
/// Registry name of the [`Mcp`] actor.
pub const MCP_ACTOR: &str = "mcp";

/// How much damage a tool can do; the only input to the approval policy.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Observes only.
    ReadOnly,
    /// Changes state that can be undone or is scoped.
    Mutating,
    /// Irreversible or unbounded (arbitrary shell, delete, network writes).
    Destructive,
}

/// What the model sees for a tool.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ToolSpec {
    /// Unique name; MCP tools are `<server>__<tool>`.
    pub name: String,
    /// Shown to the model.
    pub description: String,
    /// JSON Schema for the arguments object.
    pub schema: serde_json::Value,
    /// Drives the approval policy.
    pub risk: Risk,
}

/// A model-requested invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider call id, echoed back in the tool response.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments object.
    pub args: serde_json::Value,
    /// Session the call belongs to (persistent shell, cwd, approvals).
    pub session: SessionId,
}

/// Result returned to the model. Errors are content, not `Err`: the model
/// must see them.
#[derive(Clone, Debug, Serialize, Deserialize, kameo::Reply)]
pub struct ToolOutput {
    /// Text handed back to the model.
    pub content: String,
    /// Marks the output as a failure for providers that distinguish.
    pub is_error: bool,
}

impl ToolOutput {
    /// Successful output.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    /// Failed output the model should read.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Per-invocation context.
#[derive(Clone, Debug)]
pub struct ToolCx {
    /// Owning session.
    pub session: SessionId,
    /// Provider call id.
    pub call_id: String,
}

/// One struct per tool; the schema is derived from `Args`.
#[async_trait::async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Unique tool name.
    const NAME: &'static str;
    /// Risk class for the approval policy.
    const RISK: Risk;
    /// Argument type; its `JsonSchema` is what the model sees.
    type Args: DeserializeOwned + JsonSchema + Send;

    /// Description shown to the model.
    fn description(&self) -> String;

    /// Run it.
    async fn call(&self, args: Self::Args, cx: ToolCx) -> anyhow::Result<ToolOutput>;
}

/// Object-safe form stored in the registry; MCP tools implement this directly.
pub trait ErasedTool: Send + Sync + 'static {
    /// Spec for the model.
    fn spec(&self) -> ToolSpec;
    /// Invoke with raw args.
    fn call(
        &self,
        args: serde_json::Value,
        cx: ToolCx,
    ) -> BoxFuture<'_, anyhow::Result<ToolOutput>>;
    /// Explicit approval action; `None` → registry uses `policy.action(spec.risk)`.
    /// Only MCP tools return `Some`.
    fn gate(&self) -> Option<PolicyAction> {
        None
    }
}

impl<T: Tool> ErasedTool for T {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: T::NAME.to_string(),
            description: self.description(),
            schema: serde_json::to_value(schemars::schema_for!(T::Args)).unwrap_or_default(),
            risk: T::RISK,
        }
    }

    fn call(
        &self,
        args: serde_json::Value,
        cx: ToolCx,
    ) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let args: T::Args = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("invalid arguments for {}: {e}", T::NAME))?;
            Tool::call(self, args, cx).await
        })
    }
}

/// Shared handle type stored in the registry.
pub type DynTool = Arc<dyn ErasedTool>;

/// What the gate does for a risk class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    /// Run without asking.
    Allow,
    /// Publish [`ApprovalRequested`] and wait for [`Resolve`].
    #[default]
    Ask,
    /// Refuse; the model sees an error output.
    Deny,
}

/// `tools` config section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Policy per risk class.
    pub policy: Policy,
    /// `false` never prompts: every `ask` (policy or MCP gate) runs as
    /// `allow`; `deny` still denies. Written back by [`SetApprovals`].
    pub approvals: bool,
    /// Seconds to wait for a human before denying.
    pub approval_timeout_secs: u64,
    /// Seconds a shell command may run before its process group is killed.
    pub shell_timeout_secs: u64,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            policy: Policy::default(),
            approvals: true,
            approval_timeout_secs: 300,
            shell_timeout_secs: 120,
        }
    }
}

/// Risk → action.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    /// For [`Risk::ReadOnly`].
    pub read_only: PolicyAction,
    /// For [`Risk::Mutating`].
    pub mutating: PolicyAction,
    /// For [`Risk::Destructive`].
    pub destructive: PolicyAction,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            read_only: PolicyAction::Allow,
            mutating: PolicyAction::Ask,
            destructive: PolicyAction::Ask,
        }
    }
}

impl Policy {
    /// Look up the action for a risk class.
    pub fn action(&self, risk: Risk) -> PolicyAction {
        match risk {
            Risk::ReadOnly => self.read_only,
            Risk::Mutating => self.mutating,
            Risk::Destructive => self.destructive,
        }
    }
}

/// The plugin. Starts [`ToolRegistry`] and [`Approval`], registers builtin
/// tools, then MCP servers from `<data_dir>/mcp` (natives first so they win
/// name clashes).
pub struct ToolsPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ToolsPlugin {
    fn name(&self) -> &'static str {
        "tools"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        let registry = registry::start(rt).await?;
        mcp::start(rt, registry).await
    }
}
