//! The `Dreaming` document (design doc 136 §8): created once in the documents
//! store (id kept in `state.json`), a night section prepended per night, an
//! "Undone" line on undo. 136 W3.

use std::path::Path;

use crate::types::NightId;

/// Prepend `markdown` under `section` of tonight's entry (136 W3).
pub async fn append(
    data_dir: &Path,
    night: &NightId,
    section: &str,
    markdown: &str,
) -> anyhow::Result<()> {
    // 136 W3
    let _ = (data_dir, night, section, markdown);
    anyhow::bail!("not built: W3 (dreaming_journal)")
}
