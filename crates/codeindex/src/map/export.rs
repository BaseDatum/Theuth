//! Export to the repository (147 §13.1, S3) and the `code_overview` text
//! form (§12.1): `codemap_export(root)` writes `<root>/.omm/` in
//! oh-my-mermaid's layout — `config.yaml`, one directory per element nested
//! by containment, `diagram.mmd` = `surface::graph::to_mermaid`, the present
//! prose files, `meta.yaml` with omm's fields plus a `theuth:` block.
//! Refuses when a foreign `.omm/` exists (`import::is_ours`).
//!
//! owner: W4 — design §12.1 (text form), §13.1

use super::{Export, MapElement, MapExport, MapIndex, not_built};
use crate::index::Codeindex;

/// `config.yaml` `generator:` value that marks an `.omm/` as ours.
pub const GENERATOR: &str = "theuth-147";
/// `config.yaml` `version:`.
pub const OMM_VERSION: &str = "0.1.0";
/// The exported directory under the root.
pub const OMM_DIR: &str = ".omm";

/// `codemap_export`: write `<root>/.omm/`; structure-only elements export
/// their structure diagram and no prose; imported perspectives are skipped.
pub async fn export(ix: &Codeindex, m: Export) -> anyhow::Result<MapExport> {
    let _ = (ix, m);
    Err(not_built("export::export", 4))
}

/// `code_overview`'s text form (§12.1): the perspective tree as indented
/// lines (omm's `tree`), then the element's mermaid when one is asked for,
/// cut at `budget` bytes. Returns the text and the lines dropped.
pub fn code_overview_text(
    index: &MapIndex,
    element: Option<&MapElement>,
    budget: usize,
) -> anyhow::Result<(String, u32)> {
    let _ = (index, element, budget);
    Err(not_built("export::code_overview_text", 4))
}
