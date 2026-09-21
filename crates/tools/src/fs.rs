//! `read_file`, `write_file`, `list_dir`, `grep` over `tokio::fs` (12 §10);
//! `edit` lives in [`crate::edit`]. `read_file` renders documents as
//! Markdown (20 §2.2).
//!
//! Paths are resolved against the process cwd; `~` is not expanded.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Output cap for `read_file`.
const MAX_BYTES: usize = 64 * 1024;

// The marker literals are split with `concat!` so this source file does not
// itself contain them: `write_file`/`edit` refuse content that does.
/// First line of a cut `read_file` result; followed by exact byte/line counts.
pub const TRUNCATED_HEAD_PREFIX: &str = concat!("[read_file: ", "showing bytes ");
/// Last line of a cut `read_file` result.
pub const TRUNCATED_TAIL: &str =
    concat!("[truncated at 64 KiB; ", "use offset/limit to read more]");

/// Err when `content` carries a `read_file` truncation marker: writing it
/// back would persist a file cut at 64 KiB (the 2026-09-21 loss).
pub(crate) fn refuse_truncated(content: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !content.contains(TRUNCATED_HEAD_PREFIX) && !content.contains(TRUNCATED_TAIL),
        "refusing to write: content contains a read_file truncation marker — the source read was cut at 64 KiB; read with offset/limit or use edit for targeted changes"
    );
    Ok(())
}

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

/// Read a text file, or a document rendered as Markdown.
pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadFileArgs;

    fn description(&self) -> String {
        "Read a text file, or a PDF / Word / PowerPoint / Excel / ODF / RTF / EPUB document rendered as Markdown. Optional `offset`/`limit` select a 1-based line window. Output is capped at 64 KiB: a cut result starts with a `[read_file: …]` header giving exact byte/line counts and ends with a `[truncated …]` footer; read the rest with offset/limit and never write a cut result back. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: ReadFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let bytes = tokio::fs::read(&a.path)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
        // A detected document that fails to convert is an error the model can
        // act on, never 64 KiB of object streams; undetected bytes stay text.
        let text: String = match crate::doc::format(&bytes) {
            Some(fmt) => {
                let md = crate::doc::to_markdown(bytes, fmt)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
                format!(
                    "[{}: {fmt:?} rendered as Markdown; offset/limit count Markdown lines]\n{md}",
                    a.path.display()
                )
            }
            None => String::from_utf8_lossy(&bytes).into_owned(),
        };
        let mut out: String = match (a.offset, a.limit) {
            (None, None) => text,
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
            let (bytes_total, lines_total) = (out.len(), out.lines().count());
            out.truncate(cut);
            let lines_shown = out.lines().count();
            out.insert_str(
                0,
                &format!(
                    "{TRUNCATED_HEAD_PREFIX}0–{cut} of {bytes_total} (lines 1–{lines_shown} of {lines_total}); truncated — use offset/limit, or edit for changes]\n"
                ),
            );
            out.push('\n');
            out.push_str(TRUNCATED_TAIL);
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
        "Create or overwrite a file with the given content; parent directories are created. Content carrying a read_file truncation marker is refused. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: WriteFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        refuse_truncated(&a.content)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("theuth-edit-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn cx() -> ToolCx {
        ToolCx {
            session: crate::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
        }
    }

    async fn read(args: ReadFileArgs) -> String {
        Tool::call(&ReadFile, args, cx()).await.unwrap().content
    }

    fn args(path: &Path, offset: Option<usize>, limit: Option<usize>) -> ReadFileArgs {
        ReadFileArgs {
            path: path.to_owned(),
            offset,
            limit,
        }
    }

    #[tokio::test]
    async fn read_over_cap_is_marked_at_both_ends_with_exact_counts() {
        // 1000 lines × 70 bytes = 70 000 bytes, 1 000 lines; the cap falls
        // mid-line 937 (65536 / 70 = 936.2), so 937 partial lines are shown.
        let line = format!("{}\n", "x".repeat(69));
        let p = scratch(&line.repeat(1000));
        let out = read(args(&p, None, None)).await;
        let head = out.lines().next().unwrap();
        assert_eq!(
            head,
            format!(
                "{TRUNCATED_HEAD_PREFIX}0–65536 of 70000 (lines 1–937 of 1000); truncated — use offset/limit, or edit for changes]"
            )
        );
        assert!(
            out.ends_with(&format!("\n{TRUNCATED_TAIL}")),
            "{}",
            &out[out.len() - 80..]
        );
        let body = &out[head.len() + 1..out.len() - TRUNCATED_TAIL.len() - 1];
        assert_eq!(body.len(), MAX_BYTES);
        assert_eq!(body, &line.repeat(1000)[..MAX_BYTES]);
    }

    #[tokio::test]
    async fn read_window_under_cap_has_no_markers() {
        let line = format!("{}\n", "x".repeat(69));
        let p = scratch(&line.repeat(1000));
        let out = read(args(&p, Some(2), Some(3))).await;
        assert_eq!(out, line.repeat(3).trim_end());
        let all = read(args(&p, Some(1), Some(1000))).await;
        assert!(all.starts_with(TRUNCATED_HEAD_PREFIX) && all.ends_with(TRUNCATED_TAIL));
    }

    #[tokio::test]
    async fn write_refuses_truncation_markers() {
        let p = scratch("keep me");
        for content in [
            format!("{TRUNCATED_HEAD_PREFIX}0–1 of 2]\nbody"),
            format!("body\n{TRUNCATED_TAIL}"),
        ] {
            let a = WriteFileArgs {
                path: p.clone(),
                content: content.clone(),
            };
            let err = Tool::call(&WriteFile, a, cx())
                .await
                .unwrap_err()
                .to_string();
            assert!(err.starts_with("refusing to write:"), "{err}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep me");
        }
        let a = WriteFileArgs {
            path: p.clone(),
            content: "plain".into(),
        };
        Tool::call(&WriteFile, a, cx()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "plain");
    }
}
