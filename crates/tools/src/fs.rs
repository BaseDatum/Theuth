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
/// First line of every `read_file` result:
/// `[read_file <path>: N lines, B bytes; showing lines a–b]`.
pub const READ_HEADER_PREFIX: &str = concat!("[read_", "file ");
/// The header's range clause; together with [`READ_HEADER_PREFIX`] on one
/// line it identifies a pasted header.
pub const READ_HEADER_SHOWING: &str = concat!("; show", "ing");
/// First line of a `read_file` result cut at 64 KiB by the pre-header
/// format; still refused on write.
pub const TRUNCATED_HEAD_PREFIX: &str = concat!("[read_file: ", "showing bytes ");
/// Last line of a cut `read_file` result.
pub const TRUNCATED_TAIL: &str =
    concat!("[truncated at 64 KiB; ", "use offset/limit to read more]");

/// Err when `content` looks like `read_file` output: a truncation marker
/// (writing it back would persist a file cut at 64 KiB — the 2026-09-21
/// loss) or a pasted header. Line-number prefixes alone are not refused:
/// TSV files legitimately have them.
pub(crate) fn refuse_read_output(content: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !content.contains(TRUNCATED_HEAD_PREFIX) && !content.contains(TRUNCATED_TAIL),
        "refusing to write: content contains a read_file truncation marker — the source read was cut at 64 KiB; read with offset/limit or use edit for targeted changes"
    );
    anyhow::ensure!(
        !content
            .lines()
            .any(|l| l.starts_with(READ_HEADER_PREFIX) && l.contains(READ_HEADER_SHOWING)),
        "refusing to write: content looks like read_file output (header/line numbers); write the file's real text"
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
    /// Prefix each line with its right-aligned line number and a tab
    /// (`cat -n`); default true. `false` returns the raw text.
    #[serde(default = "yes")]
    pub numbers: bool,
}

fn yes() -> bool {
    true
}

/// Read a text file, or a document rendered as Markdown.
pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadFileArgs;

    fn description(&self) -> String {
        "Read a text file, or a PDF / Word / PowerPoint / Excel / ODF / RTF / EPUB document rendered as Markdown. The first line is always a header `[read_file <path>: N lines, B bytes; showing lines a–b]`; each following line is `N<tab>text` (`cat -n`) unless `numbers=false` — strip the `N<tab>` prefix before reusing text in `edit`/`write_file`. Optional `offset`/`limit` select a 1-based line window. Output is capped at 64 KiB: a cut result says so in the header and ends with a `[truncated …]` footer; read the rest with offset/limit and never write a cut result back. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: ReadFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let shown = a.path.display().to_string();
        let bytes = tokio::fs::read(&a.path)
            .await
            .map_err(|e| anyhow::anyhow!("{shown}: {e}"))?;
        // A detected document that fails to convert is an error the model can
        // act on, never 64 KiB of object streams; undetected bytes stay text.
        let (note, text) = match crate::doc::format(&bytes) {
            Some(fmt) => {
                let md = crate::doc::to_markdown(bytes, fmt)
                    .await
                    .map_err(|e| anyhow::anyhow!("{shown}: {e}"))?;
                let note = format!(
                    "[{shown}: {fmt:?} rendered as Markdown; offset/limit count Markdown lines]"
                );
                (Some(note), md)
            }
            None => (None, String::from_utf8_lossy(&bytes).into_owned()),
        };
        let all: Vec<&str> = text.lines().collect();
        let total = all.len();
        let first = a.offset.unwrap_or(1).max(1);
        anyhow::ensure!(
            first <= total.max(1),
            "{shown}: offset {first} is past the end of the file ({total} lines)"
        );
        let end = match a.limit {
            Some(n) => (first - 1).saturating_add(n).min(total),
            None => total,
        };
        let mut body = render(&all[first - 1..end], first, a.numbers);
        let mut count = end + 1 - first;
        let cut = body.len() > MAX_BYTES;
        if cut {
            let mut at = MAX_BYTES;
            while !body.is_char_boundary(at) {
                at -= 1;
            }
            body.truncate(at);
            // The line the cut lands in is shown partially and counts.
            count = body.matches('\n').count() + 1;
        }
        let mut out = format!(
            "{READ_HEADER_PREFIX}{shown}: {total} lines, {} bytes",
            text.len()
        );
        match count {
            0 => out.push_str(&format!("{READ_HEADER_SHOWING} 0 lines")),
            n => out.push_str(&format!(
                "{READ_HEADER_SHOWING} lines {first}–{}",
                first + n - 1
            )),
        }
        if cut {
            out.push_str("; cut at 64 KiB — read the rest with offset/limit");
        }
        out.push(']');
        if let Some(note) = note {
            out.push('\n');
            out.push_str(&note);
        }
        if !body.is_empty() {
            out.push('\n');
            out.push_str(&body);
        }
        if cut {
            out.push('\n');
            out.push_str(TRUNCATED_TAIL);
        }
        Ok(ToolOutput::ok(out))
    }
}

/// Join `lines`, numbered from `first` in `cat -n` shape (right-aligned to
/// the widest number shown, one tab, the line verbatim) when `numbers`.
fn render(lines: &[&str], first: usize, numbers: bool) -> String {
    use std::fmt::Write;
    if !numbers {
        return lines.join("\n");
    }
    let last = first + lines.len().saturating_sub(1);
    let width = last.checked_ilog10().map_or(1, |d| d as usize + 1);
    let mut out = String::with_capacity(lines.iter().map(|l| l.len() + width + 2).sum());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        write!(out, "{:>width$}\t{line}", first + i).unwrap();
    }
    out
}

/// Arguments for [`WriteFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// Full new content; the file is replaced.
    pub content: String,
}

/// Write (create or replace) a text file atomically, creating parent
/// directories; an overwrite reports a unified diff.
pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    const NAME: &'static str = "write_file";
    const RISK: Risk = Risk::Mutating;
    type Args = WriteFileArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Create or overwrite a file with the given content; parent directories are created and the write is atomic. Overwriting an existing text file returns a unified diff of the change; identical content is a no-op. Content carrying a read_file truncation marker is refused. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: WriteFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        refuse_read_output(&a.content)?;
        let shown = a.path.display().to_string();
        let existing = match tokio::fs::read(&a.path).await {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => anyhow::bail!("{shown}: {e}"),
        };
        let bytes = a.content.len();
        let mut diff = crate::edit::Diff::default();
        let (head, replaced) = match &existing {
            Some(old) if old.as_slice() == a.content.as_bytes() => {
                let mut out = ToolOutput::ok(format!("{shown} unchanged ({bytes} bytes)"));
                out.structured = Some(write_structured(&shown, bytes, false, &diff));
                return Ok(out);
            }
            Some(old) => match std::str::from_utf8(old) {
                Ok(text) => {
                    diff = crate::edit::unified(text, &a.content, &shown);
                    let head = format!(
                        "wrote {bytes} bytes to {shown} (replaced {} bytes, +{} −{} lines)\n{}",
                        old.len(),
                        diff.added,
                        diff.removed,
                        diff.text
                    );
                    (head, true)
                }
                // A binary original has no line diff worth showing.
                Err(_) => (
                    format!(
                        "wrote {bytes} bytes to {shown} (replaced {} non-UTF-8 bytes)",
                        old.len()
                    ),
                    true,
                ),
            },
            None => {
                if let Some(parent) = a.path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| anyhow::anyhow!("{}: {e}", parent.display()))?;
                }
                (format!("created {shown} ({bytes} bytes)"), false)
            }
        };
        crate::edit::write_atomic(&a.path, a.content.as_bytes()).await?;
        let mut out = ToolOutput::ok(head);
        out.structured = Some(write_structured(&shown, bytes, !replaced, &diff));
        Ok(out)
    }
}

fn write_structured(
    path: &str,
    bytes: usize,
    created: bool,
    diff: &crate::edit::Diff,
) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "bytes": bytes,
        "created": created,
        "added": diff.added,
        "removed": diff.removed,
        "diff": diff.text,
        "truncated": diff.truncated,
    })
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
            numbers: true,
        }
    }

    fn header(p: &Path, total_lines: usize, bytes: usize, rest: &str) -> String {
        format!(
            "{READ_HEADER_PREFIX}{}: {total_lines} lines, {bytes} bytes{READ_HEADER_SHOWING} {rest}]",
            p.display()
        )
    }

    #[tokio::test]
    async fn read_numbers_lines_cat_n_style_under_an_exact_header() {
        let text: String = (1..=12).map(|i| format!("l{i}\n")).collect();
        let p = scratch(&text);
        let out = read(args(&p, None, None)).await;
        let mut want = header(&p, 12, text.len(), "lines 1–12");
        for i in 1..=12 {
            want.push_str(&format!("\n{i:>2}\tl{i}"));
        }
        assert_eq!(out, want);
        // Width follows the largest number shown, not the file's.
        let out = read(args(&p, Some(8), Some(2))).await;
        assert_eq!(
            out,
            format!("{}\n8\tl8\n9\tl9", header(&p, 12, text.len(), "lines 8–9"))
        );
        // Empty lines keep their number; a limit past EOF clamps.
        let p = scratch("a\n\nc");
        let out = read(args(&p, Some(2), Some(50))).await;
        assert_eq!(out, format!("{}\n2\t\n3\tc", header(&p, 3, 4, "lines 2–3")));
    }

    #[tokio::test]
    async fn read_numbers_false_gives_raw_text_with_header() {
        let p = scratch("a\n\nc\n");
        let mut a = args(&p, None, None);
        a.numbers = false;
        assert_eq!(
            read(a).await,
            format!("{}\na\n\nc", header(&p, 3, 5, "lines 1–3"))
        );
    }

    #[tokio::test]
    async fn read_empty_file_and_offset_past_eof() {
        let p = scratch("");
        assert_eq!(
            read(args(&p, None, None)).await,
            header(&p, 0, 0, "0 lines")
        );
        let p = scratch("a\nb\n");
        let err = Tool::call(&ReadFile, args(&p, Some(3), None), cx())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.ends_with("offset 3 is past the end of the file (2 lines)"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn read_over_cap_is_marked_at_both_ends_with_exact_counts() {
        // 1000 lines × 70 bytes = 70 000 bytes; raw, the cap falls mid-line
        // 937 (65536 / 70 = 936.2); numbered (+5 bytes a line) mid-line 874.
        let line = format!("{}\n", "x".repeat(69));
        let p = scratch(&line.repeat(1000));
        let mut a = args(&p, None, None);
        a.numbers = false;
        let out = read(a).await;
        let head = out.lines().next().unwrap();
        assert_eq!(
            head,
            header(
                &p,
                1000,
                70000,
                "lines 1–937; cut at 64 KiB — read the rest with offset/limit"
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

        let out = read(args(&p, None, None)).await;
        let head = out.lines().next().unwrap();
        assert_eq!(
            head,
            header(
                &p,
                1000,
                70000,
                "lines 1–874; cut at 64 KiB — read the rest with offset/limit"
            )
        );
        let body = &out[head.len() + 1..out.len() - TRUNCATED_TAIL.len() - 1];
        assert_eq!(body.len(), MAX_BYTES);
        assert!(body.starts_with(&format!("   1\t{}\n   2\t", "x".repeat(69))));
    }

    #[tokio::test]
    async fn read_window_under_cap_has_no_markers() {
        let line = format!("{}\n", "x".repeat(69));
        let p = scratch(&line.repeat(1000));
        let mut a = args(&p, Some(2), Some(3));
        a.numbers = false;
        let out = read(a).await;
        assert_eq!(
            out,
            format!(
                "{}\n{}",
                header(&p, 1000, 70000, "lines 2–4"),
                line.repeat(3).trim_end()
            )
        );
        let all = read(args(&p, Some(1), Some(1000))).await;
        assert!(all.contains("; cut at 64 KiB") && all.ends_with(TRUNCATED_TAIL));
    }

    #[tokio::test]
    async fn guard_refuses_pasted_header_but_not_tsv_numbers() {
        let p = scratch("x");
        let pasted = format!("{}\n1\tfoo\n2\tbar", header(&p, 2, 8, "lines 1–2"));
        let err = write(&p, &pasted).await.unwrap_err().to_string();
        assert_eq!(
            err,
            "refusing to write: content looks like read_file output (header/line numbers); write the file's real text"
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x");
        write(&p, "1\tfoo\n2\tbar\n").await.unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "1\tfoo\n2\tbar\n");
        // A header mid-file (e.g. quoted in prose) is still refused; the
        // prefix without the range clause is not.
        assert!(refuse_read_output(&format!("doc\n{pasted}")).is_err());
        assert!(refuse_read_output(&format!("{READ_HEADER_PREFIX}notes]\nbody")).is_ok());
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

    async fn write(path: &Path, content: &str) -> anyhow::Result<ToolOutput> {
        let a = WriteFileArgs {
            path: path.to_owned(),
            content: content.into(),
        };
        Tool::call(&WriteFile, a, cx()).await
    }

    fn no_tmp_left(path: &Path) -> bool {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        !std::fs::read_dir(path.parent().unwrap()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .starts_with(&format!("{name}.tmp-"))
        })
    }

    #[tokio::test]
    async fn write_creates_with_parents() {
        let dir = std::env::temp_dir().join(format!("theuth-write-{}", uuid::Uuid::new_v4()));
        let p = dir.join("a/b/new.txt");
        let out = write(&p, "hello\n").await.unwrap();
        assert_eq!(out.content, format!("created {} (6 bytes)", p.display()));
        let s = out.structured.unwrap();
        assert_eq!(
            (s["created"].as_bool(), s["bytes"].as_u64()),
            (Some(true), Some(6))
        );
        assert_eq!(s["diff"], "");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello\n");
        assert!(no_tmp_left(&p));
    }

    #[tokio::test]
    async fn write_overwrite_reports_diff_and_identical_is_a_no_op() {
        let p = scratch("one\ntwo\n");
        let out = write(&p, "one\n2\nthree\n").await.unwrap();
        let shown = p.display().to_string();
        assert_eq!(
            out.content,
            format!(
                "wrote 12 bytes to {shown} (replaced 8 bytes, +2 −1 lines)\n--- a/{shown}\n+++ b/{shown}\n@@ -1,2 +1,3 @@\n one\n-two\n+2\n+three\n"
            )
        );
        let s = out.structured.unwrap();
        assert_eq!(s["created"], false);
        assert_eq!(
            (s["added"].as_u64(), s["removed"].as_u64()),
            (Some(2), Some(1))
        );
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        let out = write(&p, "one\n2\nthree\n").await.unwrap();
        assert_eq!(out.content, format!("{shown} unchanged (12 bytes)"));
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);
        assert!(no_tmp_left(&p));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_preserves_permissions_and_leaves_no_tmp_on_error() {
        use std::os::unix::fs::PermissionsExt;
        let p = scratch("#!/bin/sh\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        write(&p, "#!/bin/sh\necho\n").await.unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let dir = std::env::temp_dir().join(format!("theuth-write-ro-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.txt");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let err = write(&p, "new").await.unwrap_err().to_string();
        assert!(err.contains(".tmp-"), "{err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "old");
        assert!(no_tmp_left(&p));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
