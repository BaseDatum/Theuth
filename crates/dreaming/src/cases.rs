//! `cases.jsonl` (design doc 136 §6): append-only, one [`Case`] per line;
//! the stable split `sha256(seed + id) % 100 < 34 → val` (synthetic never
//! val); state transitions open → closed → reopened → settled. 136 W3.

use std::path::{Path, PathBuf};

use crate::types::{Case, CaseState, Split};

/// `<data_dir>/dreaming/cases.jsonl`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("cases.jsonl")
}

/// Append one case (136 W3).
pub fn append(data_dir: &Path, case: &Case) -> anyhow::Result<()> {
    // 136 W3
    let _ = (data_dir, case);
    anyhow::bail!("not built: W3 (case_put)")
}

/// Read, filtered (136 W3).
pub fn list(
    data_dir: &Path,
    state: Option<CaseState>,
    skill_hint: Option<&str>,
    split: Option<Split>,
    limit: usize,
) -> anyhow::Result<Vec<Case>> {
    // 136 W3
    let _ = (data_dir, state, skill_hint, split, limit);
    anyhow::bail!("not built: W3 (case_list)")
}
