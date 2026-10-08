// 144-W3 owns this file.
//! Citation validation at `doc_insert` / `doc_replace` (design 144 §6.6): every `](theuth:` link of
//! inserted markdown is resolved on the hub; a `missing` / `unavailable` target is refused with the
//! nearby block ids, `changed` is accepted, and a `?s=` is added when absent. The skeleton finds
//! nothing, so the call site in `tools.rs` (`Docs::edit`, beside `check_excerpts`) is a no-op.

use surface::anchor::TheuthAnchor;

/// The `theuth://` links of `markdown` that the edit must check; empty until W3 lands.
pub fn check_theuth_links(_markdown: &str) -> Vec<TheuthAnchor> {
    Vec::new()
}
