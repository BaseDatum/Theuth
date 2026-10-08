//! The bundled `codeindex` skill (147 W5): shipped in-tree as
//! `crates/codeindex/codeindex.md`, installed to `<data_dir>/skills/codeindex.md`
//! when the plugin starts. Unlike the binary-owned skills `first_run` rewrites,
//! a copy the user edited is never overwritten: a sidecar records the SHA-256
//! of the text we last wrote, and a file that matches neither the sidecar nor
//! an older shipped text (one the current text only appends to) is the user's.

use std::path::Path;

use sha2::Digest;

/// The bundled skill text.
pub const CODEINDEX_SKILL: &str = include_str!("../codeindex.md");

/// `<data_dir>/skills/codeindex.md`.
const SKILL_PATH: &str = "skills/codeindex.md";
/// Sidecar holding the SHA-256 hex of the text last installed by this binary.
const STAMP_PATH: &str = "skills/.codeindex.md.sha256";

fn sha(text: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

/// Install [`CODEINDEX_SKILL`] under `data_dir`. Writes when the file is
/// absent, when it is byte-identical to what we last installed (the sidecar),
/// or when it is a prefix of the shipped text (an older shipped version, or
/// the pre-147 copy the shipped text was started from); leaves any other
/// text alone. Returns whether the skill was written.
pub fn install(data_dir: &Path) -> std::io::Result<bool> {
    let path = data_dir.join(SKILL_PATH);
    let stamp = data_dir.join(STAMP_PATH);
    let current = sha(CODEINDEX_SKILL);
    let ours = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(e),
        Ok(existing) if existing == CODEINDEX_SKILL => {
            if std::fs::read_to_string(&stamp).ok().as_deref() != Some(current.as_str()) {
                std::fs::write(&stamp, &current)?;
            }
            return Ok(false);
        }
        Ok(existing) => {
            let stamped = std::fs::read_to_string(&stamp).ok();
            stamped.as_deref() == Some(sha(&existing).as_str())
                || CODEINDEX_SKILL.starts_with(existing.trim_end())
        }
    };
    if !ours {
        tracing::info!(path = %path.display(), "codeindex skill edited by the user; bundled text not installed");
        return Ok(false);
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&path, CODEINDEX_SKILL)?;
    std::fs::write(&stamp, &current)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        let d = std::env::temp_dir().join(format!("theuth-skill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }

    #[test]
    fn skill_text_has_front_matter_and_the_map_section() {
        assert!(CODEINDEX_SKILL.starts_with("---\nname: codeindex\n"));
        assert!(CODEINDEX_SKILL.contains("\n## The map"));
        assert!(CODEINDEX_SKILL.contains("code_overview("));
    }

    #[test]
    fn skill_install_never_overwrites_user_copy() {
        let d = tempdir();
        let path = d.0.join(SKILL_PATH);
        // Absent → written and stamped; a second install is a no-op.
        assert!(install(&d.0).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CODEINDEX_SKILL);
        assert!(!install(&d.0).unwrap());
        // The user edits it → never overwritten, even with a stale stamp.
        let mine = format!("{CODEINDEX_SKILL}\n## Mine\nkeep this\n");
        std::fs::write(&path, &mine).unwrap();
        assert!(!install(&d.0).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
        // The pre-147 copy (a prefix of the shipped text) with no stamp → upgraded.
        let old = CODEINDEX_SKILL.split("\n## The map").next().unwrap();
        std::fs::write(&path, old).unwrap();
        std::fs::remove_file(d.0.join(STAMP_PATH)).unwrap();
        assert!(install(&d.0).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CODEINDEX_SKILL);
        // A foreign file with no stamp and no shared prefix → left alone.
        std::fs::write(&path, "---\nname: codeindex\n---\nmy own notes\n").unwrap();
        std::fs::remove_file(d.0.join(STAMP_PATH)).unwrap();
        assert!(!install(&d.0).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "---\nname: codeindex\n---\nmy own notes\n"
        );
    }
}
