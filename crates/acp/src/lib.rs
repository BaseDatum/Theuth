//! Agent Client Protocol, client half (design doc 06 §3, §8.5; 32 §6.5): the
//! `agents.yaml` loader, the public registry, one external harness — a
//! subprocess or a dialled node — bound to one ACP session ([`Harness`]), and
//! pure renderers that turn ACP tool content into text. A library, not an
//! actor: the `AcpChild` actor that publishes `AgentEvent`s lives in `agent`,
//! where the store and the bus are.

pub mod agents;
pub mod harness;
pub mod mesh;
pub mod meta;
pub mod peer;
pub mod registry;
pub mod render;

pub use agents::{
    AGENTS_FILE, DEFAULT_SKILLS_DIR, DIR, Entry, EntryInfo, describe, load, path, save, unknown,
};
pub use harness::{
    ConfigOption, ConfigValue, Connect, Dialer, Harness, LineDuplex, Permission, ProbeResult,
    SetOptionResult, Transport, Turn, Update, current_model, probe,
};
pub use mesh::{MeshDialer, line_duplex};
pub use meta::{META_KEY, SessionMeta};
pub use peer::{PEER_MESSAGE_METHOD, PeerMessage};
pub use render::{content_block_text, kind, render_content, render_diff, risk};
