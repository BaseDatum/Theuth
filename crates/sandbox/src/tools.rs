//! The `sandbox_*` tools (79 §4.6, contract §2.5) and the two prompt blocks.
//! Every tool is a thin `ask` on the actor; `sandbox_create` waits for `ready`
//! while the actor streams `Progress` into the caller's card. `CONSUMES_SECRETS`
//! is `false` on all: no argument takes a placeholder. Descriptions are pure
//! functions of config.
//! Argument fields mirror the contract (§2.5); the field docs that matter to the model are kept.
//! `Result<_, ToolOutput>` is the tool-body idiom here (the `Err` is what the model reads).
#![allow(missing_docs, clippy::result_large_err)]

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::Runtime;
use schemars::JsonSchema;
use serde::Deserialize;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::actor::{self, SandboxActor};
use crate::types::{
    GuestConfig, SandboxConfig, SandboxCreate, SandboxEvent, SandboxInfo, SandboxState,
};
use crate::vm::{Mount, ResourceChange, RuntimeState, SnapshotId};

/// The host block (design §4.6), one paragraph.
pub const HOST_PROMPT: &str = "A sandbox VM is an isolated Linux desktop machine (Debian + XFCE, its own kernel) running on this computer that is also an engine in the mesh. Create one with `sandbox_create()` when the user asks for a VM, a Linux box, an isolated desktop, or somewhere safe to install and run things; it takes a few minutes and the progress shows in the card. Work inside it with spawn_subagent{agent:\"<name>\"}; nothing on this computer is visible there except the directories shared explicitly (`sandbox_share()`; the user approves each). The user can watch or take over its desktop from the app; `sandbox_list()` shows every sandbox with its state and use. A secret_request name cannot contain `/`: use dots (`smoke.pw`, not `smoke/pw`).";

/// The guest block (contract §2.5), from `sandbox.guest`.
pub fn guest_prompt(g: &GuestConfig) -> String {
    let shares = if g.shares.is_empty() {
        "no shared directories".to_owned()
    } else {
        format!("shared directories {}", g.shares.join(", "))
    };
    format!(
        "This engine is the sandbox VM `{}` running on the engine `{}`: a Linux desktop on display :1 with {shares}; the computer_* tools drive that desktop. The user may be watching or driving the same desktop from the host. After an input `<desktop_takeover by=\"user\"/>` the user has the keyboard and mouse: stop calling computer_* until `<desktop_handback/>` arrives (or call sandbox_takeover{{on:false}} yourself to hand back explicitly). Secrets on this desktop: a vault placeholder may be typed (computer_type) only into a masked password field or a `read -s` prompt — never into a visible terminal, editor or text field, where the value would show on screen — and take no screenshot while a field shows a secret.",
        g.name, g.host
    )
}

/// What every host tool holds.
pub struct State {
    pub rt: Runtime,
    pub cfg: SandboxConfig,
}

impl State {
    fn actor(&self) -> Result<ActorRef<SandboxActor>, ToolOutput> {
        crate::actor().ok_or_else(|| ToolOutput::err("sandbox is not running"))
    }

    fn vm_note(&self) -> &'static str {
        if self.cfg.vm.enabled {
            ""
        } else {
            " Sandbox VMs are disabled on this engine (sandbox.vm.enabled: false); only docker/ssh engines are listed."
        }
    }
}

/// Register the host tools and the host prompt block.
pub async fn register(reg: &ActorRef<ToolRegistry>, rt: Runtime, cfg: SandboxConfig) {
    let vm = cfg.vm.enabled;
    let s = Arc::new(State { rt, cfg });
    let _ = reg.ask(RegisterTool(Arc::new(ListTool(s.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(CreateTool(s.clone())))).await;
    if vm {
        let _ = reg
            .ask(RegisterTool(Arc::new(InstallRuntimeTool(s.clone()))))
            .await;
        let _ = reg.ask(RegisterTool(Arc::new(StartTool(s.clone())))).await;
        let _ = reg.ask(RegisterTool(Arc::new(StopTool(s.clone())))).await;
        let _ = reg.ask(RegisterTool(Arc::new(PauseTool(s.clone())))).await;
        let _ = reg.ask(RegisterTool(Arc::new(ResumeTool(s.clone())))).await;
        let _ = reg.ask(RegisterTool(Arc::new(ResizeTool(s.clone())))).await;
        let _ = reg.ask(RegisterTool(Arc::new(ShareTool(s.clone())))).await;
        let _ = reg
            .ask(RegisterTool(Arc::new(UnshareTool(s.clone()))))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(SnapshotTool(s.clone()))))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(RestoreTool(s.clone()))))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(DestroyTool(s.clone()))))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(DesktopTool(s.clone()))))
            .await;
        let _ = reg
            .ask(RegisterTool(Arc::new(TakeoverTool(s.clone()))))
            .await;
        let _ = reg
            .ask(RegisterPrompt {
                plugin: "sandbox",
                text: HOST_PROMPT.to_owned(),
            })
            .await;
    }
}

/// `ask` → `Result<T, String>` → tool output; errors the model should see are `ToolOutput::err`.
async fn ask<M, T>(actor: &ActorRef<SandboxActor>, msg: M) -> Result<T, ToolOutput>
where
    SandboxActor: kameo::message::Message<M, Reply = Result<T, String>>,
    M: Send + 'static,
    T: Send + 'static,
{
    match actor.ask(msg).await {
        Ok(v) => Ok(v),
        Err(kameo::error::SendError::HandlerError(e)) => Err(ToolOutput::err(e)),
        Err(e) => Err(ToolOutput::err(format!("sandbox: {e}"))),
    }
}

fn mib(m: u32) -> String {
    if m.is_multiple_of(1024) {
        format!("{} GiB", m / 1024)
    } else {
        format!("{m} MiB")
    }
}

/// One line per fact about a sandbox (`sandbox_create`, `sandbox_start`, …).
pub fn info_lines(i: &SandboxInfo) -> String {
    let mut lines = vec![
        format!("name: {}", i.name),
        format!("backend: {}", i.backend),
        format!("state: {}", actor::state_str(&i.state)),
        format!(
            "engine: {}",
            i.engine.as_deref().unwrap_or("not paired yet")
        ),
    ];
    if let Some(vm) = &i.vm {
        let r = vm.resources;
        let mut res = format!(
            "resources: {} cpus (max {}), {} memory (max {}), {} disk",
            r.cpus,
            r.max_cpus,
            mib(r.memory_mib),
            mib(r.max_memory_mib),
            mib(r.root_disk_mib)
        );
        if let Some(m) = &vm.metrics {
            res.push_str(&format!(
                "; live: cpu {:.0}%, memory {} used, disk {} used / {} free",
                m.cpu_percent,
                platform::fmt::bytes(m.memory_bytes),
                platform::fmt::bytes(m.disk_used_bytes),
                platform::fmt::bytes(m.disk_free_bytes)
            ));
        }
        lines.push(res);
        lines.push(format!(
            "shares: {}",
            if vm.shares.is_empty() {
                "none".to_owned()
            } else {
                vm.shares
                    .iter()
                    .map(|m| {
                        format!(
                            "{} -> {}{}",
                            m.host.display(),
                            m.guest,
                            if m.readonly { " (ro)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        lines.push(format!(
            "desktop: {}",
            match &vm.desktop {
                Some(d) => format!("yes ({}) — the user opens it from the app", d.geometry),
                None => "no".into(),
            }
        ));
        let mut pending = Vec::new();
        if vm.pending.shares {
            pending.push("shares");
        }
        if vm.pending.ports {
            pending.push("ports");
        }
        if vm.pending.resources {
            pending.push("resources");
        }
        if !pending.is_empty() {
            lines.push(format!("pending until restart: {}", pending.join(", ")));
        }
        if !vm.snapshots.is_empty() {
            lines.push(format!(
                "snapshots: {}",
                vm.snapshots
                    .iter()
                    .map(|s| s.id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    lines.join("\n")
}

fn out_info(i: &SandboxInfo) -> ToolOutput {
    let mut o = ToolOutput::ok(info_lines(i));
    o.structured = serde_json::to_value(i).ok();
    o
}

fn one_line(i: &SandboxInfo, reachable: Option<bool>) -> String {
    let mut s = format!("{} {} {}", i.name, i.backend, actor::state_str(&i.state));
    match (&i.engine, reachable) {
        (None, _) => s.push_str(" engine=-"),
        (Some(e), Some(true)) => s.push_str(&format!(" engine={e} reachable")),
        (Some(e), Some(false)) => s.push_str(&format!(" engine={e} unreachable")),
        (Some(e), None) => s.push_str(&format!(" engine={e}")),
    }
    if let Some(vm) = &i.vm {
        let r = vm.resources;
        match &vm.metrics {
            Some(m) => s.push_str(&format!(
                " cpus={}({:.0}%) memory={}/{} (limit {}, guest MemTotal stays at boot size) disk={}/{}",
                r.cpus,
                m.cpu_percent,
                platform::fmt::bytes(m.memory_bytes),
                mib(r.memory_mib),
                platform::fmt::bytes(m.memory_limit_bytes),
                platform::fmt::bytes(m.disk_used_bytes),
                mib(r.root_disk_mib)
            )),
            None => s.push_str(&format!(
                " cpus={} memory={} disk={}",
                r.cpus,
                mib(r.memory_mib),
                mib(r.root_disk_mib)
            )),
        }
        s.push_str(&format!(
            " shares={}",
            if vm.shares.is_empty() {
                "-".to_owned()
            } else {
                vm.shares
                    .iter()
                    .map(|m| m.guest.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            }
        ));
        s.push_str(&format!(
            " desktop={}",
            if vm.desktop.is_some() { "yes" } else { "no" }
        ));
        if vm.pending.shares || vm.pending.ports || vm.pending.resources {
            s.push_str(" pending=restart");
        }
    }
    s
}

// ---- sandbox_create

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArgs {
    #[serde(flatten)]
    pub spec: SandboxCreate,
    /// Wait for `ready` (default true); false returns at once in `creating`.
    #[serde(default)]
    pub wait: Option<bool>,
}

pub struct CreateTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for CreateTool {
    const NAME: &'static str = "sandbox_create";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "dict";
    type Args = CreateArgs;

    fn description(&self) -> String {
        if !self.0.cfg.vm.enabled {
            return "Create a sandbox VM — disabled on this engine (sandbox.vm.enabled: false); tell the user to enable it in config.".into();
        }
        "Create a sandbox VM: an isolated Linux desktop machine on this computer that joins the mesh as an engine. Name it ([a-z0-9-]); size defaults come from the host; `shares` lists host directories to expose ({host, guest?, readonly?}); `desktop` true by default. Waits until the VM is ready (minutes; progress streams into this card) and returns its facts; afterwards spawn_subagent{agent:\"<name>\"} runs work inside it. Refuses with the reason when this host cannot run VMs.".into()
    }

    async fn call(&self, args: CreateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let name = args.spec.name.clone();
        let first = match ask(
            &actor,
            actor::Create {
                spec: args.spec,
                origin: Some((cx.session, cx.call_id.clone())),
            },
        )
        .await
        {
            Ok(i) => i,
            Err(e) => return Ok(e),
        };
        if !args.wait.unwrap_or(true) {
            return Ok(out_info(&first));
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.0.cfg.create_timeout_secs);
        let mut last = first;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let list = match actor.ask(actor::List).await {
                Ok(l) => l,
                Err(e) => return Ok(ToolOutput::err(format!("sandbox: {e}"))),
            };
            let Some(i) = list.into_iter().find(|i| i.name == name) else {
                return Ok(ToolOutput::err(format!(
                    "{name}: record disappeared during bring-up"
                )));
            };
            last = i;
            match &last.state {
                SandboxState::Creating { .. } => continue,
                SandboxState::Failed { step, error } => {
                    let mut o = ToolOutput::err(format!(
                        "{name} failed at {step}: {error}\nsandbox_retry is not a tool; ask the user to press Retry in Settings › Mesh, or destroy it with sandbox_destroy.\n{}",
                        info_lines(&last)
                    ));
                    o.structured = serde_json::to_value(&last).ok();
                    return Ok(o);
                }
                _ => return Ok(out_info(&last)),
            }
        }
        let mut o = ToolOutput::ok(format!(
            "{}\nstill creating after {} s — follow with sandbox_list",
            info_lines(&last),
            self.0.cfg.create_timeout_secs
        ));
        o.structured = serde_json::to_value(&last).ok();
        Ok(o)
    }
}

// ---- sandbox_install_runtime

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NoArgs {}

pub struct InstallRuntimeTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for InstallRuntimeTool {
    const NAME: &'static str = "sandbox_install_runtime";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = NoArgs;

    fn description(&self) -> String {
        "Set up the sandbox VM runtime (microsandbox: msb + libkrunfw, embedded in t, no download, no root) in the user's home. sandbox_create does this itself on first use; call it only when preflight reported the runtime half installed or a failed set-up. Refuses when the runtime is already set up or the host cannot run VMs.".into()
    }

    async fn call(&self, _: NoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let pf = match actor.ask(actor::PreflightNow).await {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::err(format!("sandbox: {e}"))),
        };
        if let RuntimeState::Installed { version, .. }
        | RuntimeState::Embedded {
            version,
            extracted: true,
            ..
        } = &pf.runtime
            && pf.blockers.iter().all(|b| !b.installable)
        {
            return Ok(ToolOutput::err(format!(
                "runtime already set up ({version})"
            )));
        }
        if pf.blockers.iter().any(|b| !b.installable) {
            return Ok(ToolOutput::err(actor::blockers_text(&pf.blockers)));
        }
        match ask(&actor, actor::InstallRuntime).await {
            Ok(()) => Ok(ToolOutput::ok("runtime set up; sandbox_create can run now")),
            Err(e) => Ok(e),
        }
    }
}

// ---- sandbox_list

pub struct ListTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for ListTool {
    const NAME: &'static str = "sandbox_list";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "list[dict]";
    type Args = NoArgs;

    fn description(&self) -> String {
        format!(
            "List the sandboxes this engine made (sandbox VMs, docker and ssh engines): one line each with state, engine reachability, cpus/memory/disk with live use, shares, desktop and what waits for a restart.{}",
            self.0.vm_note()
        )
    }

    async fn call(&self, _: NoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let list = match actor.ask(actor::List).await {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(format!("sandbox: {e}"))),
        };
        let peers = match mesh::actor() {
            Some(m) => m
                .ask(mesh::Status)
                .await
                .ok()
                .map(|s| s.peers)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        if list.is_empty() {
            return Ok(ToolOutput::ok("no sandboxes"));
        }
        let text = list
            .iter()
            .map(|i| {
                let reachable = i
                    .engine
                    .as_ref()
                    .and_then(|e| peers.iter().find(|p| &p.id.0 == e).map(|p| p.reachable));
                one_line(i, reachable)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut o = ToolOutput::ok(text);
        o.structured = serde_json::to_value(&list).ok();
        Ok(o)
    }
}

// ---- start / stop / pause / resume

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameArgs {
    /// The sandbox.
    pub name: String,
}

macro_rules! name_tool {
    ($ty:ident, $name:literal, $msg:ident, $desc:literal) => {
        pub struct $ty(Arc<State>);

        #[async_trait::async_trait]
        impl Tool for $ty {
            const NAME: &'static str = $name;
            const RISK: Risk = Risk::Mutating;
            const CONSUMES_SECRETS: bool = false;
            const RETURNS: &'static str = "dict";
            type Args = NameArgs;

            fn description(&self) -> String {
                $desc.into()
            }

            async fn call(&self, a: NameArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
                let actor = match self.0.actor() {
                    Ok(a) => a,
                    Err(e) => return Ok(e),
                };
                match ask(&actor, actor::$msg { name: a.name }).await {
                    Ok(i) => Ok(out_info(&i)),
                    Err(e) => Ok(e),
                }
            }
        }
    };
}

name_tool!(
    StartTool,
    "sandbox_start",
    Start,
    "Boot a stopped or crashed sandbox VM; pending shares, ports and ceilings apply. Returns its new state."
);
name_tool!(
    PauseTool,
    "sandbox_pause",
    Pause,
    "Pause a running sandbox VM (RAM kept; its engine becomes unreachable until resumed)."
);
name_tool!(
    ResumeTool,
    "sandbox_resume",
    Resume,
    "Resume a paused sandbox VM."
);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StopArgs {
    pub name: String,
    /// Seconds to wait for a clean shutdown before killing (default 20).
    #[serde(default)]
    pub grace_secs: Option<u64>,
    /// Stop even when sessions on the VM have a running turn.
    #[serde(default)]
    pub force: Option<bool>,
}

pub struct StopTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for StopTool {
    const NAME: &'static str = "sandbox_stop";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "dict";
    type Args = StopArgs;

    fn description(&self) -> String {
        "Stop a sandbox VM gracefully (disk kept; start boots it again). Refuses when sessions on it have a running turn unless `force`.".into()
    }

    async fn call(&self, a: StopArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        match ask(
            &actor,
            actor::Stop {
                name: a.name,
                grace_secs: a.grace_secs.unwrap_or(20),
                force: a.force.unwrap_or(false),
            },
        )
        .await
        {
            Ok(i) => Ok(out_info(&i)),
            Err(e) => Ok(e),
        }
    }
}

// ---- sandbox_resize

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResizeArgs {
    pub name: String,
    #[serde(default)]
    pub cpus: Option<u8>,
    #[serde(default)]
    pub memory_mib: Option<u32>,
    /// Grow-only.
    #[serde(default)]
    pub root_disk_mib: Option<u32>,
    /// Live-resize ceiling; raising it needs a restart.
    #[serde(default)]
    pub max_cpus: Option<u8>,
    #[serde(default)]
    pub max_memory_mib: Option<u32>,
}

pub struct ResizeTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for ResizeTool {
    const NAME: &'static str = "sandbox_resize";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "dict";
    type Args = ResizeArgs;

    fn description(&self) -> String {
        "Change a sandbox VM's CPUs, memory or root disk. CPUs and memory change live within the VM's ceilings (max_cpus / max_memory_mib); the disk only grows; raising a ceiling waits for a restart (restart_required). Returns what applied now and what waits.".into()
    }

    async fn call(&self, a: ResizeArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let change = ResourceChange {
            cpus: a.cpus,
            max_cpus: a.max_cpus,
            memory_mib: a.memory_mib,
            max_memory_mib: a.max_memory_mib,
            root_disk_mib: a.root_disk_mib,
        };
        match ask(
            &actor,
            actor::Resize {
                name: a.name,
                change,
            },
        )
        .await
        {
            Ok(o) => {
                let mut text = format!(
                    "applied now: {}\nwaits for restart: {}\nstatus: {:?}\nrestart_required: {}",
                    change_str(&o.applied),
                    change_str(&o.next_start),
                    o.status,
                    o.restart_required
                );
                if o.applied.memory_mib.is_some() {
                    text.push_str("\nnote: the memory figure is the hypervisor's allocation (metrics.memory_limit_bytes); the guest's MemTotal stays at the boot size by virtio-mem design — not a bug");
                }
                let mut out = ToolOutput::ok(text);
                out.structured = serde_json::to_value(&o).ok();
                Ok(out)
            }
            Err(e) => Ok(e),
        }
    }
}

fn change_str(c: &ResourceChange) -> String {
    let mut v = Vec::new();
    if let Some(x) = c.cpus {
        v.push(format!("cpus {x}"));
    }
    if let Some(x) = c.max_cpus {
        v.push(format!("max_cpus {x}"));
    }
    if let Some(x) = c.memory_mib {
        v.push(format!("memory {}", mib(x)));
    }
    if let Some(x) = c.max_memory_mib {
        v.push(format!("max_memory {}", mib(x)));
    }
    if let Some(x) = c.root_disk_mib {
        v.push(format!("disk {}", mib(x)));
    }
    if v.is_empty() {
        "nothing".into()
    } else {
        v.join(", ")
    }
}

// ---- sandbox_share / sandbox_unshare

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShareArgs {
    pub name: String,
    /// Absolute host directory.
    pub host: String,
    /// Guest mount point; default `/shared/<basename>`.
    #[serde(default)]
    pub guest: Option<String>,
    #[serde(default)]
    pub readonly: Option<bool>,
}

pub struct ShareTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for ShareTool {
    const NAME: &'static str = "sandbox_share";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = ShareArgs;

    fn description(&self) -> String {
        "Share a host directory into a sandbox VM (bind mount at /shared/<basename> unless `guest` is given; read-write unless `readonly`). This exposes the user's files to the VM. Applied at the next start: returns the share list and restart_required when the VM is running.".into()
    }

    async fn call(&self, a: ShareArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let mut shares = match current_shares(&actor, &a.name).await {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        shares.push(Mount {
            host: a.host.into(),
            guest: a.guest.unwrap_or_default(),
            readonly: a.readonly.unwrap_or(false),
        });
        set_shares(&actor, &a.name, shares).await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnshareArgs {
    pub name: String,
    /// The guest mount point to remove.
    pub guest: String,
}

pub struct UnshareTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for UnshareTool {
    const NAME: &'static str = "sandbox_unshare";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = UnshareArgs;

    fn description(&self) -> String {
        "Remove a shared directory from a sandbox VM by its guest path. Applied at the next start; returns the share list and restart_required when the VM is running.".into()
    }

    async fn call(&self, a: UnshareArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let mut shares = match current_shares(&actor, &a.name).await {
            Ok(s) => s,
            Err(e) => return Ok(e),
        };
        let before = shares.len();
        shares.retain(|m| m.guest != a.guest);
        if shares.len() == before {
            return Ok(ToolOutput::err(format!(
                "{}: no share at {}",
                a.name, a.guest
            )));
        }
        set_shares(&actor, &a.name, shares).await
    }
}

async fn current_shares(
    actor: &ActorRef<SandboxActor>,
    name: &str,
) -> Result<Vec<Mount>, ToolOutput> {
    let list = actor
        .ask(actor::List)
        .await
        .map_err(|e| ToolOutput::err(format!("sandbox: {e}")))?;
    let i = list
        .into_iter()
        .find(|i| i.name == name)
        .ok_or_else(|| ToolOutput::err(format!("no sandbox named {name}")))?;
    let vm = i.vm.ok_or_else(|| {
        ToolOutput::err(format!(
            "{name} is a {} engine, not a sandbox VM",
            i.backend
        ))
    })?;
    Ok(vm.shares)
}

async fn set_shares(
    actor: &ActorRef<SandboxActor>,
    name: &str,
    shares: Vec<Mount>,
) -> anyhow::Result<ToolOutput> {
    match ask(
        actor,
        actor::SetShares {
            name: name.to_owned(),
            shares,
        },
    )
    .await
    {
        Ok(p) => {
            let shares = current_shares(actor, name).await.unwrap_or_default();
            let list = if shares.is_empty() {
                "none".to_owned()
            } else {
                shares
                    .iter()
                    .map(|m| {
                        format!(
                            "{} -> {}{}",
                            m.host.display(),
                            m.guest,
                            if m.readonly { " (ro)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let mut o = ToolOutput::ok(format!(
                "shares:\n{list}\nrestart_required: {}",
                p.restart_required
            ));
            o.structured = Some(
                serde_json::json!({ "shares": shares, "restart_required": p.restart_required }),
            );
            Ok(o)
        }
        Err(e) => Ok(e),
    }
}

// ---- sandbox_snapshot / sandbox_restore

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SnapshotArgs {
    pub name: String,
    /// Label (letters, digits, - _ .); the id becomes `<name>:<label>`.
    pub label: String,
    /// Memory too (not available for systemd guests in v1).
    #[serde(default)]
    pub full: Option<bool>,
}

pub struct SnapshotTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for SnapshotTool {
    const NAME: &'static str = "sandbox_snapshot";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "dict";
    type Args = SnapshotArgs;

    fn description(&self) -> String {
        "Take a disk snapshot of a sandbox VM under a label; sandbox_restore brings the VM back to it later.".into()
    }

    async fn call(&self, a: SnapshotArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        match ask(
            &actor,
            actor::Snapshot {
                name: a.name,
                label: a.label,
                full: a.full.unwrap_or(false),
            },
        )
        .await
        {
            Ok(s) => {
                let mut o = ToolOutput::ok(format!("snapshot {} taken", s.id.0));
                o.structured = serde_json::to_value(&s).ok();
                Ok(o)
            }
            Err(e) => Ok(e),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    pub name: String,
    /// Snapshot id (`<vm>:<label>`, see sandbox_list).
    pub id: String,
}

pub struct RestoreTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for RestoreTool {
    const NAME: &'static str = "sandbox_restore";
    const RISK: Risk = Risk::Destructive;
    const CONSUMES_SECRETS: bool = false;
    const RETURNS: &'static str = "dict";
    type Args = RestoreArgs;

    fn description(&self) -> String {
        "Restore a sandbox VM to a snapshot. The current disk is discarded (an automatic snapshot is taken first); the VM boots from the snapshot with its shares and ports and its engine identity.".into()
    }

    async fn call(&self, a: RestoreArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        match ask(
            &actor,
            actor::Restore {
                name: a.name,
                id: SnapshotId(a.id),
            },
        )
        .await
        {
            Ok(i) => Ok(out_info(&i)),
            Err(e) => Ok(e),
        }
    }
}

// ---- sandbox_destroy

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DestroyArgs {
    pub name: String,
    /// Keep its snapshots (default true).
    #[serde(default)]
    pub keep_snapshots: Option<bool>,
}

pub struct DestroyTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for DestroyTool {
    const NAME: &'static str = "sandbox_destroy";
    const RISK: Risk = Risk::Destructive;
    const CONSUMES_SECRETS: bool = false;
    type Args = DestroyArgs;

    fn description(&self) -> String {
        "Destroy a sandbox VM: its engine leaves the mesh, the VM and its disk are deleted (snapshots kept unless keep_snapshots is false; shared host directories are never touched), the record is removed.".into()
    }

    async fn call(&self, a: DestroyArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        match ask(
            &actor,
            actor::Destroy {
                name: a.name.clone(),
                keep_snapshots: a.keep_snapshots.unwrap_or(true),
            },
        )
        .await
        {
            Ok(()) => Ok(ToolOutput::ok(format!(
                "{} destroyed; its engine was removed from the mesh",
                a.name
            ))),
            Err(e) => Ok(e),
        }
    }
}

// ---- sandbox_desktop

pub struct DesktopTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for DesktopTool {
    const NAME: &'static str = "sandbox_desktop";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    type Args = NameArgs;

    fn description(&self) -> String {
        "Open a sandbox VM's desktop for the user in the app (a pop-out window showing the live screen). Never returns the credential.".into()
    }

    async fn call(&self, a: NameArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let list = match actor.ask(actor::List).await {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(format!("sandbox: {e}"))),
        };
        let Some(i) = list.into_iter().find(|i| i.name == a.name) else {
            return Ok(ToolOutput::err(format!("no sandbox named {}", a.name)));
        };
        let Some(port) =
            i.vm.as_ref()
                .and_then(|v| v.desktop.as_ref())
                .and_then(|d| d.host_port)
        else {
            return Ok(ToolOutput::err(format!("{} has no desktop", a.name)));
        };
        self.0
            .rt
            .publish(SandboxEvent::OpenDesktop {
                name: a.name.clone(),
                ws_url: format!("ws://127.0.0.1:{port}/websockify"),
            })
            .await;
        Ok(ToolOutput::ok(format!(
            "asked the desktop app to open {}'s desktop; without an app attached the user opens Settings › Mesh › {} › Desktop",
            a.name, a.name
        )))
    }
}

// ---- sandbox_takeover (host and guest)

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TakeoverArgs {
    /// The sandbox VM.
    pub name: String,
    /// true = the user takes the desktop; false = hand it back to the agent.
    pub on: bool,
}

pub struct TakeoverTool(Arc<State>);

#[async_trait::async_trait]
impl Tool for TakeoverTool {
    const NAME: &'static str = "sandbox_takeover";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = TakeoverArgs;

    fn description(&self) -> String {
        "Tell every session running on a sandbox VM that the user has taken over its desktop (on: true — they stop computer_*) or handed it back (on: false). Normally pressed in the app; call it with on: false to hand back explicitly.".into()
    }

    async fn call(&self, a: TakeoverArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let actor = match self.0.actor() {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        match ask(
            &actor,
            actor::Takeover {
                name: a.name,
                on: a.on,
            },
        )
        .await
        {
            Ok(n) => Ok(ToolOutput::ok(format!(
                "{n} session(s) told: {}",
                if a.on {
                    "desktop taken over by the user"
                } else {
                    "desktop handed back"
                }
            ))),
            Err(e) => Ok(e),
        }
    }
}

/// `sandbox_takeover` inside a guest: steers this engine's own running sessions.
pub struct GuestTakeoverTool {
    pub rt: Runtime,
    pub guest: GuestConfig,
}

#[async_trait::async_trait]
impl Tool for GuestTakeoverTool {
    const NAME: &'static str = "sandbox_takeover";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = TakeoverArgs;

    fn description(&self) -> String {
        format!(
            "This engine is the sandbox VM `{}`. Tell every session running here that the user has taken over the desktop (on: true) or handed it back (on: false — call this to hand back explicitly after a `<desktop_takeover by=\"user\"/>`).",
            self.guest.name
        )
    }

    async fn call(&self, a: TakeoverArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        if a.name != self.guest.name {
            return Ok(ToolOutput::err(format!(
                "this engine is sandbox VM {}; it cannot steer {}",
                self.guest.name, a.name
            )));
        }
        let n = crate::takeover::steer(&self.rt, &a.name, "", a.on).await;
        Ok(ToolOutput::ok(format!(
            "{n} session(s) told: {}",
            if a.on {
                "desktop taken over by the user"
            } else {
                "desktop handed back"
            }
        )))
    }
}
