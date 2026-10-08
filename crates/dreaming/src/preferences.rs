//! Standing preferences (design doc 136 §7.1 rung 3): `<data_dir>/dreaming/preferences.md`,
//! ≤ `cap_lines` × 140 chars, surfaced as the plugin's prompt block
//! "## Standing preferences (learned)" when non-empty. Wave C: the file read
//! and the block text; 136 W3: `preferences_set` (add | replace | demote,
//! the cap, the demotion to a knowledge `decision`).

use std::path::{Path, PathBuf};

/// The block heading (136 §7.1).
pub const HEADING: &str = "## Standing preferences (learned)";

/// `<data_dir>/dreaming/preferences.md`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("preferences.md")
}

/// The prompt block: heading + the file's non-empty lines; `None` when the
/// file is absent or has no content line.
pub fn block(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path(data_dir)).ok()?;
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!("{HEADING}\n{}", lines.join("\n")))
}

/// `preferences_set` (136 W3): `op` ∈ add | replace | demote; returns the block text.
pub async fn set(
    data_dir: &Path,
    op: &str,
    line: &str,
    evidence: &[String],
    cap_lines: u32,
) -> anyhow::Result<String> {
    // 136 W3
    let _ = (data_dir, op, line, evidence, cap_lines);
    anyhow::bail!("not built: W3 (preferences_set)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_none_for_absent_or_blank_and_heading_plus_lines_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(block(dir.path()), None);
        std::fs::create_dir_all(crate::night::dir(dir.path())).unwrap();
        std::fs::write(path(dir.path()), "\n  \n").unwrap();
        assert_eq!(block(dir.path()), None);
        std::fs::write(path(dir.path()), "- never use ask_user; numbered text\n\n- commit as you go  \n").unwrap();
        assert_eq!(
            block(dir.path()).unwrap(),
            "## Standing preferences (learned)\n- never use ask_user; numbered text\n- commit as you go"
        );
    }
}
