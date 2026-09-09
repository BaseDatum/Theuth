//! `ToolRegistry` actor, registered as [`crate::REGISTRY_ACTOR`].
//! Owns every [`DynTool`]; every invocation passes through the approval
//! policy here before the tool runs.

use runtime::Runtime;

use crate::{DynTool, ToolCall, ToolOutput, ToolSpec};

/// The registry actor. Look up with `ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)`.
pub struct ToolRegistry {
    // filled in by the tools implementation
}

/// `ask` → `Vec<ToolSpec>` for every registered tool, sorted by name.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListTools;

/// `ask` → [`ToolOutput`]. Never fails: unknown tool, denied approval, bad
/// args, tool panic and timeout all come back as `ToolOutput { is_error: true }`.
#[derive(Debug)]
pub struct Invoke(pub ToolCall);

/// `ask` → `()`. Replaces an existing tool with the same name.
pub struct RegisterTool(pub DynTool);

/// Spawn registry + approval actors, register builtins and MCP servers.
pub async fn start(rt: &Runtime) -> anyhow::Result<()> {
    let _ = rt;
    todo!("tools implementation")
}

#[allow(dead_code)]
fn _contract_uses(_: ToolSpec, _: ToolOutput) {}
