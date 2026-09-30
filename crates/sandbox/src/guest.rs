//! Inside a sandbox VM (`sandbox.guest` set, written by the host at create,
//! 79 §2.10): this engine registers the guest prompt block and the
//! `sandbox_takeover` tool (so an agent can hand the desktop back explicitly),
//! nothing else — no VMs in VMs.

use std::sync::Arc;

use runtime::Runtime;

use crate::tools::{GuestTakeoverTool, guest_prompt};
use crate::types::GuestConfig;

/// Register the guest surface once the tool registry is up.
pub fn start(rt: &Runtime, guest: GuestConfig) {
    let rt = rt.clone();
    tokio::spawn(async move {
        if let Some(reg) = rt
            .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
            .await
        {
            let text = guest_prompt(&guest);
            let _ = reg
                .ask(tools::RegisterTool(Arc::new(GuestTakeoverTool {
                    rt: rt.clone(),
                    guest,
                })))
                .await;
            let _ = reg
                .ask(tools::RegisterPrompt {
                    plugin: "sandbox",
                    text,
                })
                .await;
        }
    });
}
