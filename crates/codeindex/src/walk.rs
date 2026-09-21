//! Walk a root with `ignore` (60 §5.1): what `grep` can see is what gets
//! indexed. Metadata only — file contents are read at parse time, for the
//! changed set.

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::lang::Language;

/// Hard cap on files per root (60 §11).
pub const MAX_FILES: usize = 50_000;

/// One walked file.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Root-relative, forward slashes.
    pub path: String,
    /// Modified time as unix nanoseconds (sync key).
    pub mtime_ns: i64,
    /// Byte size (sync key).
    pub size: i64,
    /// By extension.
    pub language: Language,
}

/// The walk result.
#[derive(Debug, Default)]
pub struct Walk {
    /// Files in path order.
    pub entries: Vec<Entry>,
    /// The walk stopped at [`MAX_FILES`].
    pub truncated: bool,
}

/// Walk `root`: gitignore/`.ignore`/hidden rules plus `.codeindexignore`; skip
/// files over `max_bytes` and generated paths; stop at [`MAX_FILES`].
pub fn walk(root: &Path, max_bytes: u64) -> Walk {
    let skip_vendor = root.join("Cargo.toml").is_file() || root.join("go.mod").is_file();
    let mut b = ignore::WalkBuilder::new(root);
    b.add_custom_ignore_filename(".codeindexignore");
    b.sort_by_file_path(|a, b| a.cmp(b));
    b.filter_entry(move |e| {
        let name = e.file_name().to_string_lossy();
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
        if is_dir {
            // `depth() == 1` = a direct child of the root.
            !(name == "node_modules"
                || name == "target"
                || (skip_vendor && name == "vendor" && e.depth() == 1))
        } else {
            !generated_file(&name)
        }
    });
    let mut out = Walk::default();
    for entry in b.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > max_bytes {
            continue;
        }
        if out.entries.len() >= MAX_FILES {
            out.truncated = true;
            break;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let language = rel
            .extension()
            .and_then(|e| e.to_str())
            .map_or(Language::Other, Language::from_ext);
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        out.entries.push(Entry {
            path,
            mtime_ns,
            size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
            language,
        });
    }
    out
}

/// `*.min.js`, `*.pb.go`, `*_generated.*` (60 §5.1).
fn generated_file(name: &str) -> bool {
    name.ends_with(".min.js")
        || name.ends_with(".pb.go")
        || name
            .rsplit_once('.')
            .is_some_and(|(stem, _)| stem.ends_with("_generated"))
}

/// Path heuristic for `files.is_test` (60 §4): a `test`/`tests`/`__tests__`
/// directory, `*_test.go`, `test_*.py`/`*_test.py`, `*.test.*`/`*.spec.*`.
pub fn is_test_path(path: &str) -> bool {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    dir.split('/')
        .any(|d| matches!(d, "test" | "tests" | "__tests__" | "testdata"))
        || name.ends_with("_test.go")
        || name.starts_with("test_") && name.ends_with(".py")
        || name.ends_with("_test.py")
        || name.contains(".test.")
        || name.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristics() {
        assert!(generated_file("app.min.js"));
        assert!(generated_file("api.pb.go"));
        assert!(generated_file("schema_generated.rs"));
        assert!(!generated_file("generated.rs"));
        assert!(is_test_path("crates/x/tests/it.rs"));
        assert!(is_test_path("pkg/a_test.go"));
        assert!(is_test_path("src/test_store.py"));
        assert!(is_test_path("src/store.test.ts"));
        assert!(!is_test_path("src/testing.rs"));
        assert!(!is_test_path("src/contest.py"));
    }
}
