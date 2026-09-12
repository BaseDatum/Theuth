//! Agent Client Protocol, client half (design doc 06 §3, §8.5): the
//! `agents.yaml` loader, the public registry, one external harness process
//! bound to one ACP session ([`Harness`]), and pure renderers that turn ACP
//! tool content into text. A library, not an actor: the `AcpChild` actor that
//! publishes `AgentEvent`s lives in `agent`, where the store and the bus are.

pub mod agents;
pub mod harness;
pub mod registry;
pub mod render;

pub use agents::{AGENTS_FILE, DIR, Entry, EntryInfo, describe, load, path, save, unknown};
pub use harness::{Connect, Harness, Permission, ProbeResult, Turn, Update, probe};
pub use render::{content_block_text, kind, render_content, render_diff, render_plan, risk};
