//! `dreaming_undo` (design doc 136 §3.3 item 5): `Restore` every knowledge id
//! of the night/group to its pre-night commit (the receipt's
//! `(id, commit_before)`), restore `backups/<night>/<skill>.md`, mark edits
//! `undone_at`, publish `DreamUndone`, one "Undone" journal line. 136 W3.

use std::path::Path;

use crate::types::{NightId, UndoReport};

/// Undo a night or one of its groups (136 W3).
pub async fn undo(
    data_dir: &Path,
    night: &NightId,
    group: Option<&str>,
) -> anyhow::Result<UndoReport> {
    // 136 W3
    let _ = (data_dir, night, group);
    anyhow::bail!("not built: W3 (dreaming_undo)")
}
