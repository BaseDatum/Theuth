//! Learned sidecar edits (design doc 136 §7.1–7.2): apply bounded
//! [`EditRecord`]s to `<data_dir>/skills/learned/<skill>.md` with exact-text
//! anchors, back the file up under `backups/<night>/<skill>.md`, append the
//! outcome to `edits.jsonl`, return an [`ApplyReport`]. Caps: 12 bullets per
//! skill, 140 chars each. 136 W3.

use std::path::Path;

use crate::types::{ApplyReport, EditRecord, NightId};

/// Apply `edits` to `skill`'s sidecar (136 W3).
pub fn apply(
    data_dir: &Path,
    skill: &str,
    edits: &[EditRecord],
    night: &NightId,
) -> anyhow::Result<ApplyReport> {
    // 136 W3
    let _ = (data_dir, skill, edits, night);
    anyhow::bail!("not built: W3 (skill_learned_edit)")
}
