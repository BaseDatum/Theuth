//! `read_file`, `write_file`, `list_dir`, `grep`, `edit` over `tokio::fs`
//! (12 §10).
//!
//! Paths are resolved against the process cwd; `~` is not expanded.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Output cap for `read_file`.
const MAX_BYTES: usize = 64 * 1024;

/// Arguments for [`ReadFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// 1-based first line to return.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Read a text file.
pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadFileArgs;

    fn description(&self) -> String {
        "Read a text file. Optional 1-based `offset` line and `limit` line count. Output is capped at 64 KiB. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: ReadFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let bytes = tokio::fs::read(&a.path)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut out: String = match (a.offset, a.limit) {
            (None, None) => text.into_owned(),
            (offset, limit) => text
                .lines()
                .skip(offset.unwrap_or(1).saturating_sub(1))
                .take(limit.unwrap_or(usize::MAX))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        if out.len() > MAX_BYTES {
            let mut cut = MAX_BYTES;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push_str("\n[truncated at 64 KiB; use offset/limit to read more]");
        }
        Ok(ToolOutput::ok(out))
    }
}

/// Arguments for [`WriteFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// Full new content; the file is replaced.
    pub content: String,
}

/// Write (create or replace) a text file, creating parent directories.
pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    const NAME: &'static str = "write_file";
    const RISK: Risk = Risk::Mutating;
    type Args = WriteFileArgs;

    fn description(&self) -> String {
        "Create or overwrite a file with the given content; parent directories are created. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: WriteFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        if let Some(parent) = a.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("{}: {e}", parent.display()))?;
        }
        tokio::fs::write(&a.path, &a.content)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
        Ok(ToolOutput::ok(format!(
            "wrote {} bytes to {}",
            a.content.len(),
            a.path.display()
        )))
    }
}

/// Arguments for [`ListDir`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListDirArgs {
    /// Directory path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
}

/// List a directory, one entry per line, directories suffixed with `/`.
pub struct ListDir;

#[async_trait::async_trait]
impl Tool for ListDir {
    const NAME: &'static str = "list_dir";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ListDirArgs;

    fn description(&self) -> String {
        "List a directory: one entry per line, sorted, directories end with `/`. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: ListDirArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::ok(list_dir(&a.path).await?))
    }
}

async fn list_dir(path: &Path) -> anyhow::Result<String> {
    let mut rd = tokio::fs::read_dir(path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let mut entries = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            name.push('/');
        }
        entries.push(name);
    }
    entries.sort();
    Ok(entries.join("\n"))
}

/// Arguments for [`Grep`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GrepArgs {
    /// Regex to search for (Rust regex syntax).
    pub pattern: String,
    /// File or directory to search; default: the process cwd.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Only search files matching this glob, e.g. `*.rs`.
    #[serde(default)]
    pub glob: Option<String>,
    /// Maximum number of matches to return; default 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Search file contents with a regex (12 §10).
pub struct Grep;

#[async_trait::async_trait]
impl Tool for Grep {
    const NAME: &'static str = "grep";
    const RISK: Risk = Risk::ReadOnly;
    type Args = GrepArgs;

    fn description(&self) -> String {
        "Search file contents with a regex. Respects .gitignore, skips hidden and binary files. Returns `path:line: text` per match, sorted by path, up to `limit` (default 200). `path` is a file or directory (default: cwd); `glob` filters files, e.g. `*.rs`.".into()
    }

    async fn call(&self, a: GrepArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let root = match a.path {
            Some(p) => p,
            None => std::env::current_dir()?,
        };
        let limit = a.limit.unwrap_or(GREP_LIMIT).max(1) as usize;
        let out =
            tokio::task::spawn_blocking(move || grep(&root, &a.pattern, a.glob.as_deref(), limit))
                .await??;
        Ok(ToolOutput::ok(out))
    }
}

/// Default match cap for `grep`.
const GREP_LIMIT: u32 = 200;
/// Longest match line echoed back; longer lines are cut with `…`.
const GREP_LINE_CHARS: usize = 400;

/// Walk `root` (gitignore, hidden and binary files skipped) and collect
/// `path:line: text` matches for `pattern`, sorted by path, up to `limit`.
fn grep(root: &Path, pattern: &str, glob: Option<&str>, limit: usize) -> anyhow::Result<String> {
    use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};

    let matcher = grep_regex::RegexMatcher::new(pattern)?;
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build();
    let mut walk = ignore::WalkBuilder::new(root);
    walk.sort_by_file_path(|a, b| a.cmp(b));
    if let Some(glob) = glob {
        let mut ov = ignore::overrides::OverrideBuilder::new(root);
        ov.add(glob)?;
        walk.overrides(ov.build()?);
    }
    let is_dir = root.is_dir();
    let mut out = Vec::new();
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let shown = if is_dir {
            path.strip_prefix(root).unwrap_or(path)
        } else {
            path
        };
        let shown = shown.display().to_string();
        // Read errors on one file (permissions, vanished) skip it, not the search.
        let _ = searcher.search_path(
            &matcher,
            path,
            UTF8(|line, text| {
                let text = text.trim_end();
                match text.char_indices().nth(GREP_LINE_CHARS) {
                    Some((i, _)) => out.push(format!("{shown}:{line}: {}…", &text[..i])),
                    None => out.push(format!("{shown}:{line}: {text}")),
                }
                Ok(out.len() < limit)
            }),
        );
        if out.len() >= limit {
            out.push(format!("[… limit {limit} reached]"));
            break;
        }
    }
    if out.is_empty() {
        return Ok("no matches".into());
    }
    Ok(out.join("\n"))
}

/// Arguments for [`Edit`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EditArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// Exact text to replace.
    pub old: String,
    /// Replacement text.
    pub new: String,
    /// Replace every occurrence instead of requiring exactly one.
    #[serde(default)]
    pub all: bool,
}

/// Replace text in a file (12 §10).
pub struct Edit;

#[async_trait::async_trait]
impl Tool for Edit {
    const NAME: &'static str = "edit";
    const RISK: Risk = Risk::Mutating;
    type Args = EditArgs;

    fn description(&self) -> String {
        "Replace `old` with `new` in a text file. `old` must match exactly once unless `all` is true; nothing is written otherwise. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: EditArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::ok(edit(&a.path, &a.old, &a.new, a.all).await?))
    }
}

/// Replace `old` in the file at `path`; written atomically (tmp + rename),
/// nothing touched on error.
async fn edit(path: &Path, old: &str, new: &str, all: bool) -> anyhow::Result<String> {
    anyhow::ensure!(!old.is_empty(), "`old` must not be empty");
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{}: not UTF-8", path.display()))?;
    let count = text.matches(old).count();
    anyhow::ensure!(count > 0, "`old` not found in {}", path.display());
    anyhow::ensure!(
        count == 1 || all,
        "`old` occurs {count} times in {}; pass all=true or make it unique",
        path.display()
    );
    let n = if all { count } else { 1 };
    let text = text.replacen(old, new, n);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let perm = tokio::fs::metadata(path).await?.permissions();
    tokio::fs::write(&tmp, &text)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", tmp.display()))?;
    tokio::fs::set_permissions(&tmp, perm).await?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(format!(
        "edited {}: {n} replacement{}",
        path.display(),
        if n == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("wth-edit-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn edit_ambiguous_old_errors_and_leaves_file_untouched() {
        let p = scratch("a foo b foo c");
        let err = edit(&p, "foo", "bar", false).await.unwrap_err().to_string();
        assert!(err.contains("occurs 2 times"), "{err}");
        assert_eq!(std::fs::read(&p).unwrap(), b"a foo b foo c");
        assert!(!p.with_extension("txt.tmp").exists());
    }

    #[tokio::test]
    async fn edit_all_replaces_every_occurrence() {
        let p = scratch("a foo b foo c");
        let msg = edit(&p, "foo", "bar", true).await.unwrap();
        assert!(msg.ends_with("2 replacements"), "{msg}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a bar b bar c");
    }

    #[tokio::test]
    async fn edit_single_occurrence_writes_expected_content() {
        let p = scratch("line one\nline two\n");
        let msg = edit(&p, "two", "2", false).await.unwrap();
        assert!(msg.ends_with("1 replacement"), "{msg}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "line one\nline 2\n");
        assert!(edit(&p, "missing", "x", false).await.is_err());
    }
}
