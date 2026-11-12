use crate as sandbox;
use agent::rpc_support::{RpcServerExt, owner_root};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, send};
use runtime::SessionId;
use sandbox::rpc_support::sandbox;

#[rpc_core_macros::rpc_descriptors(owner = "sandbox", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait SandboxesApi {
    /// Every sandbox record (docker, ssh, vm) with state, resources, shares, metrics (79 §4.5).
    #[method(name = "sandboxes_list")]
    async fn sandboxes_list(&self) -> RpcResult<Vec<sandbox::SandboxInfo>>;
    /// Host readiness for a sandbox VM: runtime, virtualization, derived defaults, blockers (79 §5.1).
    #[method(name = "sandbox_preflight")]
    async fn sandbox_preflight(&self) -> RpcResult<sandbox::Preflight>;
    /// Install msb + libkrunfw; progress as `SandboxEvent::Progress { name: "runtime" }`.
    #[method(name = "sandbox_install_runtime")]
    async fn sandbox_install_runtime(&self) -> RpcResult<()>;
    /// Create a sandbox VM; returns at once in `creating`, bring-up streams `Progress` (79 §5.2).
    #[method(name = "sandbox_create")]
    async fn sandbox_create(&self, spec: sandbox::SandboxCreate)
    -> RpcResult<sandbox::SandboxInfo>;
    /// Resume a `failed` bring-up from its step.
    #[method(name = "sandbox_retry")]
    async fn sandbox_retry(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Boot a stopped VM (pending shares, ports and ceilings apply).
    #[method(name = "sandbox_start")]
    async fn sandbox_start(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Stop gracefully for `grace_secs` (20), then kill; `force` skips the busy-sessions warning.
    #[method(name = "sandbox_stop")]
    async fn sandbox_stop(
        &self,
        name: String,
        grace_secs: Option<u64>,
        force: Option<bool>,
    ) -> RpcResult<sandbox::SandboxInfo>;
    /// Pause (RAM kept).
    #[method(name = "sandbox_pause")]
    async fn sandbox_pause(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Resume a paused VM.
    #[method(name = "sandbox_resume")]
    async fn sandbox_resume(&self, name: String) -> RpcResult<sandbox::SandboxInfo>;
    /// Change CPUs / memory / disk / ceilings; live within ceilings (79 §5.5).
    #[method(name = "sandbox_resize")]
    async fn sandbox_resize(
        &self,
        name: String,
        change: sandbox::ResourceChange,
    ) -> RpcResult<sandbox::ResizeOutcome>;
    /// Replace the share list; applied by the next start (79 §5.6).
    #[method(name = "sandbox_set_shares")]
    async fn sandbox_set_shares(
        &self,
        name: String,
        shares: Vec<sandbox::Mount>,
    ) -> RpcResult<sandbox::Pending>;
    /// Take a disk (or `full`: memory too) snapshot.
    #[method(name = "sandbox_snapshot")]
    async fn sandbox_snapshot(
        &self,
        name: String,
        label: String,
        full: Option<bool>,
    ) -> RpcResult<sandbox::VmSnapshotInfo>;
    /// Restore a snapshot; the current disk is discarded.
    #[method(name = "sandbox_restore")]
    async fn sandbox_restore(
        &self,
        name: String,
        id: sandbox::SnapshotId,
    ) -> RpcResult<sandbox::SandboxInfo>;
    /// Remove peer, VM, vault row and record; snapshots kept unless `keep_snapshots: false`.
    #[method(name = "sandbox_destroy")]
    async fn sandbox_destroy(&self, name: String, keep_snapshots: Option<bool>) -> RpcResult<()>;
    /// noVNC endpoint and password for the pop-out (RPC only, never a tool result; 79 §6.2).
    #[method(name = "sandbox_desktop_access")]
    async fn sandbox_desktop_access(
        &self,
        name: String,
    ) -> RpcResult<sandbox::types::DesktopAccess>;
    /// Live CPU / memory / disk / network numbers.
    #[method(name = "sandbox_metrics")]
    async fn sandbox_metrics(&self, name: String) -> RpcResult<sandbox::VmMetrics>;
    /// Last `tail` (200) runtime log lines.
    #[method(name = "sandbox_logs")]
    async fn sandbox_logs(
        &self,
        name: String,
        tail: Option<usize>,
    ) -> RpcResult<Vec<sandbox::LogLine>>;
    /// The user takes (`on`) or hands back the VM's desktop; steers every running session on it (79 §6.5). Returns sessions steered.
    #[method(name = "sandbox_takeover")]
    async fn sandbox_takeover(&self, name: String, on: bool) -> RpcResult<u32>;
    /// Open an interactive shell in the VM as a PTY job of `session` (the caller's; an owner
    /// root is opened when absent). Returns `{ job, session }`; attach with `subscribe_pty`,
    /// type with `pty_input`, resize with `pty_resize`.
    #[method(name = "sandbox_terminal", with_extensions)]
    async fn sandbox_terminal(
        &self,
        name: String,
        session: Option<SessionId>,
    ) -> RpcResult<serde_json::Value>;
}

#[async_trait]
impl SandboxesApiServer for Server {

    // 79: sandbox VMs. Each method is one `ask` to the `SandboxActor` (crates/sandbox/src/actor.rs).

    async fn sandboxes_list(&self) -> RpcResult<Vec<sandbox::SandboxInfo>> {
        send(&sandbox()?, sandbox::List).await
    }

    async fn sandbox_preflight(&self) -> RpcResult<sandbox::Preflight> {
        send(&sandbox()?, sandbox::PreflightNow).await
    }

    async fn sandbox_install_runtime(&self) -> RpcResult<()> {
        send(&sandbox()?, sandbox::InstallRuntime).await
    }

    async fn sandbox_create(
        &self,
        spec: sandbox::SandboxCreate,
    ) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Create { spec, origin: None }).await
    }

    async fn sandbox_retry(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Retry { name }).await
    }

    async fn sandbox_start(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Start { name }).await
    }

    async fn sandbox_stop(
        &self,
        name: String,
        grace_secs: Option<u64>,
        force: Option<bool>,
    ) -> RpcResult<sandbox::SandboxInfo> {
        send(
            &sandbox()?,
            sandbox::Stop {
                name,
                grace_secs: grace_secs.unwrap_or(20),
                force: force.unwrap_or(false),
            },
        )
        .await
    }

    async fn sandbox_pause(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Pause { name }).await
    }

    async fn sandbox_resume(&self, name: String) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Resume { name }).await
    }

    async fn sandbox_resize(
        &self,
        name: String,
        change: sandbox::ResourceChange,
    ) -> RpcResult<sandbox::ResizeOutcome> {
        send(&sandbox()?, sandbox::Resize { name, change }).await
    }

    async fn sandbox_set_shares(
        &self,
        name: String,
        shares: Vec<sandbox::Mount>,
    ) -> RpcResult<sandbox::Pending> {
        send(&sandbox()?, sandbox::SetShares { name, shares }).await
    }

    async fn sandbox_snapshot(
        &self,
        name: String,
        label: String,
        full: Option<bool>,
    ) -> RpcResult<sandbox::VmSnapshotInfo> {
        send(
            &sandbox()?,
            sandbox::Snapshot {
                name,
                label,
                full: full.unwrap_or(false),
            },
        )
        .await
    }

    async fn sandbox_restore(
        &self,
        name: String,
        id: sandbox::SnapshotId,
    ) -> RpcResult<sandbox::SandboxInfo> {
        send(&sandbox()?, sandbox::Restore { name, id }).await
    }

    async fn sandbox_destroy(&self, name: String, keep_snapshots: Option<bool>) -> RpcResult<()> {
        send(
            &sandbox()?,
            sandbox::Destroy {
                name,
                keep_snapshots: keep_snapshots.unwrap_or(true),
            },
        )
        .await
    }

    async fn sandbox_desktop_access(
        &self,
        name: String,
    ) -> RpcResult<sandbox::types::DesktopAccess> {
        send(&sandbox()?, sandbox::DesktopAccess { name }).await
    }

    async fn sandbox_metrics(&self, name: String) -> RpcResult<sandbox::VmMetrics> {
        send(&sandbox()?, sandbox::Metrics { name }).await
    }

    async fn sandbox_logs(
        &self,
        name: String,
        tail: Option<usize>,
    ) -> RpcResult<Vec<sandbox::LogLine>> {
        send(
            &sandbox()?,
            sandbox::Logs {
                name,
                tail: tail.unwrap_or(200),
            },
        )
        .await
    }

    async fn sandbox_takeover(&self, name: String, on: bool) -> RpcResult<u32> {
        send(&sandbox()?, sandbox::Takeover { name, on }).await
    }

    async fn sandbox_terminal(
        &self,
        ext: &Extensions,
        name: String,
        session: Option<SessionId>,
    ) -> RpcResult<serde_json::Value> {
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        let auth = self.authorize(ext, session).await?;
        let job: String = send(
            &sandbox()?,
            sandbox::Terminal {
                name,
                session,
                root: auth.resources.root.clone(),
            },
        )
        .await?;
        Ok(serde_json::json!({ "job": job, "session": session }))
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(SandboxesApiServer::into_rpc(server), rpc_descriptors())
        .expect("sandbox RPC descriptors")
}
