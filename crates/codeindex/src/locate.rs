//! `Locate` (design 71 §2.5, M11): verify board anchors against the working
//! tree. Read-only: the range is read from disk (never the index's cached
//! `body`, R §D.7); the index is consulted only to find where a named
//! `Definition` sits now. Paths are root-bounded (60 §11) and `root` must be
//! a git top-level.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Codeindex, store};

/// `ask` → `Result<Vec<Located>, String>`: one reply per anchor, in order.
/// Batch form: one call per board open.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Locate {
    /// Absolute repository root every `path` resolves against; must be a
    /// git top-level.
    pub root: PathBuf,
    /// Anchors to verify; replies keep this order.
    pub anchors: Vec<LocateAnchor>,
}

/// One board anchor (71 §2.3): a root-relative path with an optional 1-based
/// inclusive line range, the definition name the range quotes, and the
/// SHA-256 (lowercase hex) of the quoted text.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LocateAnchor {
    /// Root-relative, forward slashes, no `..`.
    pub path: String,
    /// First line, 1-based; absent = whole file.
    pub start: Option<u32>,
    /// Last line, inclusive; absent = `start` alone.
    pub end: Option<u32>,
    /// Name of the `Definition` the range quotes; enables `moved`.
    pub symbol: Option<String>,
    /// `sha256(text)` of the quoted lines joined by `\n`, no trailing newline;
    /// absent = a bare link (the range only has to exist).
    pub text_sha: Option<String>,
}

/// How an anchor relates to the working tree now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum LocateStatus {
    /// Range exists and its text hashes to `text_sha` (or no `text_sha` was given).
    Same,
    /// Range exists, text differs, and `symbol` (if any) still starts there.
    Changed,
    /// `symbol` now starts on another line; `start`/`end`/`text` are the new range.
    Moved,
    /// File, range or `symbol` is gone.
    Missing,
}

/// Reply row of [`Locate`], positionally aligned with `Locate::anchors`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Located {
    /// Relation to the working tree.
    pub status: LocateStatus,
    /// First line of the current range, 1-based (the request's for `missing`).
    pub start: u32,
    /// Last line, inclusive.
    pub end: u32,
    /// Current text of `start..=end`; `None` for `missing`.
    pub text: Option<String>,
}

/// Lowercase hex SHA-256, the `text_sha` convention.
pub fn text_sha(text: &str) -> String {
    let d = Sha256::digest(text.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reject `..`, absolute and scheme-like paths before touching the disk, so a
/// path that does not exist can still be told apart from one that escapes.
fn lexical_rel(path: &str) -> anyhow::Result<&Path> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() || path.contains("://") || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        bail!("path {path:?} is not root-relative");
    }
    Ok(p)
}

/// Lines of `path` under `root`, or `None` when the file is gone. An existing
/// file that canonicalises outside `root` (symlink escape) is an error.
fn read_lines(root: &Path, rel: &Path) -> anyhow::Result<Option<Vec<String>>> {
    let full = root.join(rel);
    if !full.is_file() {
        return Ok(None);
    }
    let canon = full.canonicalize().with_context(|| format!("canonicalize {}", full.display()))?;
    if canon.strip_prefix(root).is_err() {
        bail!("{} is outside {}", rel.display(), root.display());
    }
    let bytes = std::fs::read(&canon).with_context(|| format!("read {}", canon.display()))?;
    Ok(Some(String::from_utf8_lossy(&bytes).lines().map(str::to_owned).collect()))
}

/// `start..=end` of `lines`, or `None` past EOF.
fn slice(lines: &[String], start: u32, end: u32) -> Option<String> {
    let (s, e) = (start as usize, end as usize);
    if s == 0 || e < s || e > lines.len() {
        return None;
    }
    Some(lines[s - 1..e].join("\n"))
}

/// Answer a [`Locate`]. Errors (whole batch): `root` not a directory or not a
/// git top-level; any `path` outside `root`.
pub async fn locate(ix: &Arc<Codeindex>, m: Locate) -> anyhow::Result<Vec<Located>> {
    let root = crate::root::discover(Some(&m.root))?;
    let top = gix::discover::upwards(&root)
        .ok()
        .and_then(|(p, _)| p.into_repository_and_work_tree_directories().1)
        .and_then(|p| p.canonicalize().ok());
    if top.as_deref() != Some(root.as_path()) {
        bail!("root {} is not a git top-level", root.display());
    }
    // Every path is checked before any file is read: one escape fails the batch.
    let rels: Vec<&Path> = m.anchors.iter().map(|a| lexical_rel(&a.path)).collect::<anyhow::Result<_>>()?;
    // The index is only needed to find a moved definition; opened once, lazily.
    let mut ri = None;
    let mut out = Vec::with_capacity(m.anchors.len());
    for (a, rel) in m.anchors.iter().zip(rels) {
        let req_start = a.start.unwrap_or(1);
        let req_end = a.end.unwrap_or(req_start);
        let missing = || Located { status: LocateStatus::Missing, start: a.start.unwrap_or(0), end: a.end.or(a.start).unwrap_or(0), text: None };
        let Some(lines) = read_lines(&root, rel)? else {
            out.push(missing());
            continue;
        };
        let (start, end) = match a.start {
            Some(_) => (req_start, req_end),
            None => (1, lines.len() as u32),
        };
        let text = slice(&lines, start, end);
        let same = match (&text, &a.text_sha) {
            (Some(t), Some(sha)) => text_sha(t).eq_ignore_ascii_case(sha),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if same {
            out.push(Located { status: LocateStatus::Same, start, end, text });
            continue;
        }
        let Some(symbol) = a.symbol.as_deref().filter(|s| !s.is_empty()) else {
            out.push(match text {
                Some(t) => Located { status: LocateStatus::Changed, start, end, text: Some(t) },
                None => missing(),
            });
            continue;
        };
        let (ri, stale) = match &ri {
            Some(x) => x,
            None => {
                let opened = ix.open_root(&root).await?;
                let stale = ix.reconcile(&opened, false).await?.stale;
                ri.insert((opened, stale))
            }
        };
        let stale = *stale;
        let rel_s = crate::root::rel_str(rel);
        // `definitions_named` is name-wide; several same-name definitions in
        // one file (impl methods) pick the nearest to `start`.
        let def = store::definitions_named(&ri.pool, symbol)
            .await?
            .into_iter()
            .filter(|d| d.path == rel_s)
            .min_by_key(|d| (d.line_start - i64::from(start)).unsigned_abs());
        out.push(match (def, text) {
            (Some(d), _) if d.line_start != i64::from(start) => {
                let new_start = d.line_start as u32;
                let new_end = (new_start + (end - start)).min(lines.len() as u32);
                Located { status: LocateStatus::Moved, start: new_start, end: new_end, text: slice(&lines, new_start, new_end) }
            }
            (Some(_), Some(t)) => Located { status: LocateStatus::Changed, start, end, text: Some(t) },
            // Symbol not indexed yet: the text differs, that much is true.
            (None, Some(t)) if stale => Located { status: LocateStatus::Changed, start, end, text: Some(t) },
            _ => missing(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_rel_rejects_escapes() {
        for bad in ["", "/etc/passwd", "../x.rs", "a/../../x.rs", "file:///x"] {
            assert!(lexical_rel(bad).is_err(), "{bad}");
        }
        assert!(lexical_rel("crates/a/src/lib.rs").is_ok());
    }

    #[test]
    fn sha_is_lowercase_hex_of_joined_lines() {
        assert_eq!(text_sha("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let lines: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(slice(&lines, 2, 3).as_deref(), Some("b\nc"));
        assert_eq!(slice(&lines, 3, 4), None);
        assert_eq!(slice(&lines, 0, 1), None);
    }
}
