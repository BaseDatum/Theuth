//! Outbound text shaping (spec 06, "Outbound"): markdown → plain conversion and
//! chunking. The implementation lives in `markdown` (`plain` / `chunks`); this
//! module keeps the names the channel uses (docs/design/09-signal.md §2.2).

pub use markdown::{chunks as chunk_text, plain as markdown_to_plain};

/// Maximum characters per outbound Signal message.
pub const MAX_CHUNK_CHARS: usize = 2000;
