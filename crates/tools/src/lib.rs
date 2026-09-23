//! Tools plugin: the registry every tool lives in, the single approval
//! contract (`Risk → PolicyAction`, 12 §5) every invocation passes through,
//! builtin tools, and MCP-sourced tools (drop-in `<data_dir>/mcp/*.json`,
//! hot-reloaded; `mcp/config.yaml` overrides a tool's [`Risk`] with a [`Gate`]).
//!
//! Public surface consumed by other plugins:
//! - commands on the `"tools"` actor ([`ToolRegistry`]): [`ListTools`], [`Invoke`], [`AppInvoke`], [`InspectTool`], [`DescribeTools`], [`ExternalApproval`], [`RegisterTool`], [`UnregisterTool`], [`RegisterActivity`], [`ActivityRecipient`], [`InstallCommunicationAccess`], [`RegisterLifecycle`], [`ExpectParticipants`], [`ParticipantsReady`], [`OpenTree`], [`CloseTreeAll`]
//! - commands on the `"approval"` actor ([`Approval`]): [`Resolve`]
//! - commands on the `"ask"` actor ([`AskUser`]): [`AnswerAsk`]
//! - commands on the `"mcp"` actor ([`Mcp`]): [`ListServers`], [`SetGate`], [`Restart`], [`AddServer`], [`RemoveServer`], [`AddRepo`], [`SyncMcp`], [`McpLogin`], [`McpLogout`]
//! - commands on the `"jobs"` actor ([`Jobs`]): [`ListJobs`], [`CancelJob`], [`ListTerminals`], [`PtyScreen`], [`PtyInput`], [`CloseSessions`]
//! - events: [`ApprovalRequested`], [`ApprovalResolved`], [`AskRequested`], [`AskResolved`], [`JobEvent`]
//! - activity protocol (47 §4, [`activity`]): the supervisor implements [`Admit`], [`Settle`] and
//!   [`Notice`] and registers an [`ActivitySink`]; producers hold an [`Activity`] guard for the real
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
pub mod edit;
pub mod fs;
pub mod git;
pub mod glob;
pub mod images;
pub mod jobs;
pub mod mcp;
pub mod pty;
pub mod registry;
pub mod resources;
mod schema;
pub mod secrets;
pub mod shell;

pub use activity::{
    Activity, ActivitySink, Admission, Admit, Kind, Notice, Operation, OperationId, Rejected,
    Settle, Tracked,
};
pub use approval::{Approval, ApprovalId, ApprovalRequested, ApprovalResolved, Decision, Resolve};
pub use ask::{
    Answer, AnswerAsk, AskId, AskRequested, AskResolved, AskUser, Choice, PendingAsks, Question,
};
pub use communication::{CommunicationAccess, Disabled};
pub use jobs::CloseSessions;
pub use jobs::{
    CancelJob, CancelOrigin, InstallActivity, JOBS_ACTOR, JobCounts, JobEvent, JobExit, JobId,
    JobView, Jobs, ListJobs,
};
pub use mcp::{
    AddRepo, AddServer, ListServers, Mcp, McpAuth, McpLogin, McpLogout, McpProgress, McpServer,
    McpToolInfo, Progress, RemoveServer, RepoInfo, Restart, SetGate, SyncMcp,
};
pub use pty::{
    Cell, Color, Key, ListTerminals, PtyAttach, PtyAttached, PtyChunk, PtyInput, PtyOut, PtyPage,
    PtyResize, PtyScreen, PtyScreenReply, PtyViewport, Style, StyledRun, TerminalSummary,
};
pub use registry::{
    ActivityRecipient, AppInvoke, AppInvoked, ApprovalsState, CheckFence, DescribeTools,
    ExternalApproval, FenceChecker, FenceError, GetApprovals, GetPtyIdle, HiddenTools,
    InspectTool, InstallCommunicationAccess, Invoke, ListPrompts, ListTools, RegisterActivity,
    RegisterFenceChecker, RegisterPrompt, RegisterTool, SetApprovals, SetCap, SetHidden,
    SetParent, SetPtyIdle, ToolInspection, ToolRegistry, UnregisterPrompt, UnregisterTool,
    UserActivation,
};
pub use registry::{
    CloseTreeAll, ExpectParticipants, OpenTree, ParticipantsReady, RegisterLifecycle,
};
pub use registry::{InstallResources, RemoveResources};
pub use resources::{CloseAck, CloseTree, TreeOpened};
pub use resources::{Retention, SessionResources};

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

impl Risk {
    /// The serde name: `read_only` | `mutating` | `destructive`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Mutating => "mutating",
            Self::Destructive => "destructive",
        }
    }
}

impl std::fmt::Display for Risk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Risk {
    type Err = String;

    /// The serde names; anything else names them in the error.
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "read_only" => Ok(Self::ReadOnly),
            "mutating" => Ok(Self::Mutating),
            "destructive" => Ok(Self::Destructive),
            _ => Err(format!("unknown risk `{s}`; use read_only | mutating | destructive")),
        }
    }
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

/// Where a registered tool comes from, as an app may *show* it (46 §3.2).
/// Display only: no credential value is in it, and it is not what the tool's
/// identity hashes — that is [`ErasedTool::origin`], which the registry
/// reports as `ToolInspection::identity`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSource {
    /// Built into this harness or one of its plugins.
    Native {
        /// Harness package version that compiled the tool: a rebuilt native
        /// with the same name and schema is a new binding.
        version: String,
    },
    /// A server entry under `<data_dir>/mcp`.
    Mcp {
        /// Server name (file stem or wrapper key).
        server: String,
        /// The entry, scrubbed ([`McpEntry`]).
        entry: McpEntry,
    },
}

impl ToolSource {
    /// This build's native origin.
    pub fn native() -> Self {
        Self::Native {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// An MCP server entry as an app may inspect it: what runs / where it is,
/// never credentials. Scrubbed at construction and lossy by design (two
/// entries can display alike and still be different backends; identity
/// tells them apart): vault values → placeholders (37); `env` carries
/// variable *names* only; HTTP headers are omitted entirely; the URL keeps
/// scheme/host/port/path (no userinfo, query or fragment); argv values that
/// follow a credential-looking flag (`--token x`, `--api-key=x`, `-p x`)
/// are masked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpEntry {
    /// A child process.
    Stdio {
        /// Unexpanded `command` as written in the entry.
        command: String,
        /// Unexpanded `args`, credential values masked.
        args: Vec<String>,
        /// Absolute working directory; a clone under `mcp/repos/` for a
        /// model-authored integration (46 §5).
        cwd: Option<std::path::PathBuf>,
        /// Environment variable names the entry sets, sorted.
        env: Vec<String>,
        /// Name of the clone under `mcp/repos/` that holds `cwd`, if any:
        /// where an app's source snapshot of a model-authored integration
        /// comes from.
        repo: Option<String>,
    },
    /// A remote server.
    Http {
        /// Endpoint origin and path; userinfo, query and fragment removed.
        url: String,
    },
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
    /// Where this session's owners put files (51 §4.5); durable unless the
    /// supervisor installed a private root.
    pub resources: Arc<SessionResources>,
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
    /// Whether a `{{secret:NAME}}` placeholder in the args is credential
    /// use *by this call*: the gate raises it to `Ask` and the registry
    /// expands it to the value just before `call` (37 §0.4). `false` for a
    /// tool whose arguments are data, not commands to run: `app` commits
    /// source text as an inert artifact, where the placeholder stays literal
    /// until the terminal tool call the artifact later makes consumes it
    /// (46 §6); a search query naming a placeholder must match the literal
    /// text, never expand into the secret and hand it to the matcher (49
    /// §4.4). Expanding earlier would persist the secret and prompt for a
    /// call that never touches it. Output redaction applies either way.
    const CONSUMES_SECRETS: bool = true;

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
    /// Origin for display. Natives and plugin tools keep the default.
    fn source(&self) -> ToolSource {
        ToolSource::native()
    }
    /// Opaque fingerprint of where the tool actually runs, hashed with the
    /// name and schema into the identity an app binds to (46 §3.2). Never
    /// exposed, so it may cover the *raw* configuration: MCP hashes the whole
    /// server entry (URL with query, argv, env, headers), so two same-named
    /// tools on different endpoints never share an identity and a rotated
    /// credential conservatively rebinds. Natives: this build's version, so
    /// a rebuilt native with the same name and schema is a new binding.
    fn origin(&self) -> String {
        format!("native:{}", env!("CARGO_PKG_VERSION"))
    }
    /// [`Tool::CONSUMES_SECRETS`]: `false` only for tools that take an
    /// inert artifact as arguments. MCP tools always consume.
    fn consumes_secrets(&self) -> bool {
        true
    }
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

    fn consumes_secrets(&self) -> bool {
        T::CONSUMES_SECRETS
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
    /// Seconds a PTY job's screen must stay unchanged before the model is
    /// woken with `<job_idle>` (61 §1.6); `start.idle_secs` overrides per job.
    pub pty_idle_secs: u64,
    /// Seed each session shell's `PATH` from the user's login shell (so
    /// tools installed by rc files are found); `THEUTH_NO_LOGIN_PATH=1` also
    /// turns it off.
    pub login_path: bool,
    /// Program for `start`'s PTY jobs instead of the platform default
    /// (`$SHELL`; `pwsh.exe` else `powershell.exe` on Windows — 58 §0.3).
    pub pty_shell: Option<std::path::PathBuf>,
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
            pty_idle_secs: 60,
            login_path: true,
            pty_shell: None,
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
         A subagent spawned with `tools: read_only` (or `mutating`) only sees tools at or below that risk; \
         a call above it is `denied: subagent is capped`. \
         Remote servers that answer 401 need the user to log in (`l` on the server in /mcp); you cannot."
    )
}
