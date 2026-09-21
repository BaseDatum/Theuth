//! Root discovery and the per-root index file name (60 §0.7, §0.10).

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};

/// `args.root` | git top-level of the process cwd (`gix::discover`) | cwd;
/// canonicalised; must be a directory. `ToolCx` carries no cwd, so this uses
/// `std::env::current_dir()` like `grep` (`tools/src/fs.rs:219-222`).
pub fn discover(arg: Option<&Path>) -> anyhow::Result<PathBuf> {
    let raw = match arg {
        Some(p) => p.to_path_buf(),
        None => {
            let cwd = std::env::current_dir().context("current_dir")?;
            gix::discover::upwards(&cwd)
                .ok()
                .and_then(|(path, _)| path.into_repository_and_work_tree_directories().1)
                .unwrap_or(cwd)
        }
    };
    let root = raw
        .canonicalize()
        .with_context(|| format!("root {} does not exist", raw.display()))?;
    if !root.is_dir() {
        bail!("root {} is not a directory", root.display());
    }
    Ok(root)
}

/// `<slug>-<hash8>.sqlite`: canonical path with `/` → `-` (leading `-`
/// dropped), first 8 hex of its SHA-256 (60 §0.7).
pub fn db_file_name(root: &Path) -> String {
    let s = root.to_string_lossy();
    let hash = Sha256::digest(s.as_bytes());
    let slug: String = s
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_whitespace() { '-' } else { c })
        .collect();
    format!("{}-{:02x}{:02x}{:02x}{:02x}.sqlite", slug.trim_start_matches('-'), hash[0], hash[1], hash[2], hash[3])
}

/// Join a user path to `root` and require it to canonicalise under it (60 §11).
pub fn under_root(root: &Path, p: &Path) -> anyhow::Result<PathBuf> {
    let full = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let canon = full
        .canonicalize()
        .with_context(|| format!("{} does not exist", full.display()))?;
    let rel = canon
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", canon.display(), root.display()))?;
    Ok(rel.to_path_buf())
}

/// Root-relative path with forward slashes, as stored in `files.path`.
pub fn rel_str(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_is_slug_plus_hash8() {
        let n = db_file_name(Path::new("/Users/x/Develop/theuth"));
        assert!(n.starts_with("Users-x-Develop-theuth-"), "{n}");
        assert!(n.ends_with(".sqlite"));
        let hash = &n["Users-x-Develop-theuth-".len()..n.len() - ".sqlite".len()];
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(n, db_file_name(Path::new("/Users/x/Develop/theuth2")));
    }
}
