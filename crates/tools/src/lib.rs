//! Tools plugin: the registry every tool lives in, the single approval
//! contract (`Risk → PolicyAction`, 12 §5) every invocation passes through,
//! builtin tools, and MCP-sourced tools (drop-in `<data_dir>/mcp/*.json`,
//! hot-reloaded; `mcp/config.yaml` overrides a tool's [`Risk`] with a [`Gate`]).
//!
//! Public surface consumed by other plugins:
//! - commands on the `"tools"` actor ([`ToolRegistry`]): [`ListTools`], [`Invoke`], [`ExternalApproval`], [`RegisterTool`], [`UnregisterTool`], [`RegisterActivity`], [`ActivityRecipient`], [`InstallCommunicationAccess`]
//! - commands on the `"approval"` actor ([`Approval`]): [`Resolve`]
//! - commands on the `"ask"` actor ([`AskUser`]): [`AnswerAsk`]
//! - commands on the `"mcp"` actor ([`Mcp`]): [`ListServers`], [`SetGate`], [`Restart`], [`AddServer`], [`RemoveServer`], [`AddRepo`], [`SyncMcp`], [`McpLogin`], [`McpLogout`]
//! - commands on the `"jobs"` actor ([`Jobs`]): [`ListJobs`], [`CancelJob`]
//! - events: [`ApprovalRequested`], [`ApprovalResolved`], [`AskRequested`], [`AskResolved`], [`JobEvent`]
//! - activity protocol (47 §4, [`activity`]): the supervisor implements [`Admit`] and [`Settle`]
//!   and registers an [`ActivitySink`]; producers hold an [`Activity`] guard for the real
//!   lifetime of admitted work and read [`activity::current`] inside an invocation.

use std::sync::Arc;

use futures::future::BoxFuture;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod activity;
pub mod approval;
pub mod ask;
pub mod communication;
pub mod doc;
pub mod fs;
pub mod git;
pub mod jobs;
pub mod mcp;
pub mod pty;
pub mod registry;
pub mod secrets;
pub mod shell;

pub use activity::{
    Activity, ActivitySink, Admission, Admit, Kind, Operation, OperationId, Rejected, Settle,
    Tracked,
};
pub use approval::{Approval, ApprovalId, ApprovalRequested, ApprovalResolved, Decision, Resolve};
pub use ask::{
    Answer, AnswerAsk, AskId, AskRequested, AskResolved, AskUser, Choice, PendingAsks, Question,
};
pub use communication::{CommunicationAccess, Disabled};
pub use jobs::{
    CancelJob, InstallActivity, JOBS_ACTOR, JobEvent, JobExit, JobId, JobView, Jobs, ListJobs,
};
pub use mcp::{
    AddRepo, AddServer, ListServers, Mcp, McpAuth, McpLogin, McpLogout, McpProgress, McpServer,
    McpToolInfo, Progress, RemoveServer, RepoInfo, Restart, SetGate, SyncMcp,
};
pub use registry::{
    ActivityRecipient, ApprovalsState, ExternalApproval, GetApprovals, HiddenTools,
    InstallCommunicationAccess, Invoke, ListPrompts, ListTools, RegisterActivity, RegisterPrompt,
    RegisterTool, SetApprovals, SetHidden, SetParent, ToolRegistry, UnregisterPrompt,
    UnregisterTool,
};

/// Registry name of the [`ToolRegistry`] actor.
pub const REGISTRY_ACTOR: &str = "tools";
/// Registry name of the [`Approval`] actor.
pub const APPROVAL_ACTOR: &str = "approval";
/// Registry name of the [`AskUser`] actor.
pub const ASK_ACTOR: &str = "ask";
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
    /// Unique name. MCP tools register under their remote name, no prefix (01 §1.4).
    pub name: String,
    /// Shown to the model.
    pub description: String,
    /// JSON Schema for the arguments object.
    pub schema: serde_json::Value,
    /// Drives the approval policy.
    pub risk: Risk,
    /// Offered as a direct tool definition even when the runtime is on
    /// (08 §0.4): the floor is the tools that talk to the user or to other
    /// agents; everything that returns data is a runtime function only.
    pub direct: bool,
    /// Python annotation of what the runtime function returns (`ToolOutput::
    /// structured` when set, else the text): `str`, `list[dict]`, `Any`, ...
    pub returns: String,
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
    /// The caller consumes the output itself (`secret_capture`, 37 §3.2):
    /// publish no events, keep no log, and wait for completion instead of
    /// handing back a background job.
    #[serde(default)]
    pub quiet: bool,
}

/// Result returned to the model. Errors are content, not `Err`: the model
/// must see them.
#[derive(Clone, Debug, Serialize, Deserialize, kameo::Reply)]
pub struct ToolOutput {
    /// Text handed back to the model.
    pub content: String,
    /// Marks the output as a failure for providers that distinguish.
    pub is_error: bool,
    /// Machine-readable result (MCP `structuredContent`); `None` for text-only tools.
    pub structured: Option<serde_json::Value>,
    /// Files handed to the model as image parts of the tool result (23 §5):
    /// paths, not bytes, like `files` on inputs (17 §2). Empty for most tools.
    #[serde(default)]
    pub files: Vec<std::path::PathBuf>,
}

impl ToolOutput {
    /// Successful output.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            structured: None,
            files: Vec::new(),
        }
    }
    /// Failed output the model should read.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            structured: None,
            files: Vec::new(),
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
    /// [`ToolCall::quiet`], copied from the call.
    pub quiet: bool,
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
    /// Python annotation of the value the runtime hands back: the text unless
    /// `call` fills `ToolOutput::structured`, in which case name its shape.
    const RETURNS: &'static str = "str";
    /// Stay a direct tool definition when the runtime is on (08 §0.4). Only
    /// for tools whose result is not data the model would work on: talking to
    /// the user, spawning and messaging agents.
    const DIRECT: bool = false;

    /// Description shown to the model.
    fn description(&self) -> String;

    /// Run it.
    async fn call(&self, args: Self::Args, cx: ToolCx) -> anyhow::Result<ToolOutput>;
}

/// Object-safe form stored in the registry; MCP tools implement this directly.
/// The registry's only permission input is `spec().risk` (12 §5).
pub trait ErasedTool: Send + Sync + 'static {
    /// Spec for the model.
    fn spec(&self) -> ToolSpec;
    /// Invoke with raw args.
    fn call(
        &self,
        args: serde_json::Value,
        cx: ToolCx,
    ) -> BoxFuture<'_, anyhow::Result<ToolOutput>>;
}

impl<T: Tool> ErasedTool for T {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: T::NAME.to_string(),
            description: self.description(),
            schema: serde_json::to_value(schemars::schema_for!(T::Args)).unwrap_or_default(),
            risk: T::RISK,
            direct: T::DIRECT,
            returns: T::RETURNS.to_owned(),
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

/// What the gate does for a risk class. Ordered `Allow < Ask < Deny` so a
/// caller can only ever raise it (`action.max(Ask)`, 37 §0.4).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default, JsonSchema,
)]
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

/// One line of `mcp/config.yaml`: a [`Risk`] that overrides the remote tool's
/// annotations, or `off` (tool unregistered / server not connected). Not a
/// second policy: the risk still goes through `tools.policy` (12 §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// [`Risk::ReadOnly`].
    ReadOnly,
    /// [`Risk::Mutating`].
    Mutating,
    /// [`Risk::Destructive`].
    Destructive,
    /// Hidden from the model; on a server line, not connected.
    Off,
}

impl Gate {
    /// The risk this line assigns; `None` for [`Gate::Off`].
    pub fn risk(self) -> Option<Risk> {
        match self {
            Gate::ReadOnly => Some(Risk::ReadOnly),
            Gate::Mutating => Some(Risk::Mutating),
            Gate::Destructive => Some(Risk::Destructive),
            Gate::Off => None,
        }
    }
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
    /// Seconds `ask_user` waits for answers before giving up.
    pub ask_timeout_secs: u64,
    /// Seconds `shell` waits for a job to exit before replying with a stub
    /// instead of the output; `0` makes every command a stub.
    pub job_grace_secs: u64,
    /// Seconds a job may run before its process group is killed.
    pub job_timeout_secs: u64,
    /// `start`'s readiness deadline when `ready` gives no `timeout` (22 §6).
    pub pty_ready_timeout_secs: u64,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            policy: Policy::default(),
            approvals: true,
            approval_timeout_secs: 300,
            ask_timeout_secs: 1800,
            job_grace_secs: 3,
            job_timeout_secs: 1800,
            pty_ready_timeout_secs: 30,
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
        let _ = registry
            .ask(RegisterPrompt {
                plugin: "tools",
                text: prompt_block(&rt.data_dir),
            })
            .await;
        mcp::start(rt, registry, registry::config(rt)?.policy).await
    }
}

/// The plugin's system-prompt block (07 §4 part 7): where MCP servers and the
/// tool policy live and what an approval means. A function of the data dir
/// only, so it never busts the system cache.
fn prompt_block(data_dir: &std::path::Path) -> String {
    let d = data_dir.display();
    format!(
        "MCP servers are JSON files in {d}/mcp/ (any `mcp.json` shape, one or many servers per file; \
         `$VAR` and `$(cmd)` in command/args/env/url/headers expand through the user's shell). \
         Edit or drop a file and the tools appear or vanish within a second; add_mcp_server and \
         remove_mcp_server do the same from a tool call. {d}/mcp/config.yaml gates risk per server or tool \
         (`servers.<name>.risk` / `servers.<name>.tools.<tool>`: read_only | mutating | destructive | off); \
         a tool named `<server>__<tool>` is one that two servers both offer. \
         Each tool call is allowed, asked of the user, or denied by `tools.policy` in {d}/config.yaml \
         according to its risk; you cannot approve your own calls, and a `denied by policy` result is final. \
         Remote servers that answer 401 need the user to log in (`l` on the server in /mcp); you cannot."
    )
}
