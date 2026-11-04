//! `read_file`, `write_file`, `list_dir`, `grep` over `tokio::fs` (12 §10);
//! `edit` lives in [`crate::edit`]. `read_file` renders documents as
//! Markdown (20 §2.2).
//!
//! Relative paths are resolved against the session's seated working directory
//! ([`crate::seat`]), else the process cwd; `~` is not expanded. `write_file`
//! refuses a path outside the session's write root.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Output cap for `read_file`: the hard backstop after the line caps.
const MAX_BYTES: usize = 64 * 1024;
/// Lines shown when no `limit` is given.
const DEFAULT_LINES: usize = 2000;
/// Longest line echoed back; longer lines are cut with ` …[+K chars]`.
const MAX_LINE_CHARS: usize = 2000;

// The marker literals are split with `concat!` so this source file does not
// itself contain them: `write_file`/`edit` refuse content that does.
/// First line of every `read_file` result:
/// `[read_file <path>: N lines, B bytes; showing lines a–b]`.
pub const READ_HEADER_PREFIX: &str = concat!("[read_", "file ");
/// The header's range clause; together with [`READ_HEADER_PREFIX`] on one
/// line it identifies a pasted header.
pub const READ_HEADER_SHOWING: &str = concat!("; show", "ing");
/// End of the header of an `outline=true` result.
pub const READ_HEADER_OUTLINE: &str = concat!("; out", "line]");
/// First line of a `read_file` result cut at 64 KiB by the pre-header
/// format; still refused on write.
pub const TRUNCATED_HEAD_PREFIX: &str = concat!("[read_file: ", "showing bytes ");
/// End of the footer of a `read_file` result that stopped before EOF:
/// `[read_file: N more lines; use offset/limit, tail or ranges]`.
pub const TRUNCATED_TAIL: &str = concat!("; use offset/limit, ", "tail or ranges]");

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
        !content.lines().any(|l| {
            l.starts_with(READ_HEADER_PREFIX)
                && (l.contains(READ_HEADER_SHOWING) || l.ends_with(READ_HEADER_OUTLINE))
        }),
        "refusing to write: content looks like read_file output (header/line numbers); write the file's real text"
    );
    Ok(())
}

/// Arguments for [`ReadFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// File path, absolute or relative to the session's working directory (no `~`).
    pub path: PathBuf,
    /// 1-based first line to return.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Return the last N lines instead; not combinable with `offset`/`limit`.
    #[serde(default)]
    pub tail: Option<usize>,
    /// 1-based inclusive `[start, end]` line windows, shown in the order
    /// given, each under an `@@ lines a–b @@` line; at most `limit` lines in
    /// total (default 2000). Not combinable with `offset`/`tail`.
    #[serde(default)]
    pub ranges: Option<Vec<[usize; 2]>>,
    /// Prefix each line with its right-aligned line number and a tab
    /// (`cat -n`); default true. `false` returns the raw text.
    #[serde(default = "yes")]
    pub numbers: bool,
    /// Return the file's symbol outline (as `code_outline` renders it)
    /// instead of its text; needs the codeindex plugin. Not combinable with
    /// any other option.
    #[serde(default)]
    pub outline: bool,
}

fn yes() -> bool {
    true
}

/// Renders a source file's symbol outline for `read_file(outline=true)`;
/// `Ok(None)` when the file is outside every indexed root or its language
/// is not indexed. Registered once by the codeindex plugin via
/// [`set_outliner`] (`tools` must not depend on `codeindex`).
#[async_trait::async_trait]
pub trait Outliner: Send + Sync {
    /// The outline text for `path`, or `None` when the file is not indexed.
    async fn outline(&self, path: &Path) -> anyhow::Result<Option<String>>;
}

static OUTLINER: OnceLock<Arc<dyn Outliner>> = OnceLock::new();

/// Install the process-wide [`Outliner`]; `false` if one is already set.
pub fn set_outliner(o: Arc<dyn Outliner>) -> bool {
    OUTLINER.set(o).is_ok()
}

/// The installed [`Outliner`], if any.
pub fn outliner() -> Option<Arc<dyn Outliner>> {
    OUTLINER.get().cloned()
}

/// Read a text file, or a document rendered as Markdown.
pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadFileArgs;

    fn description(&self) -> String {
        "Read a text file, or a PDF / Word / PowerPoint / Excel / ODF / RTF / EPUB document rendered as Markdown; paths are relative to the working directory, `~` is not expanded. Line 1 is always a header `[read_file <path>: N lines, B bytes; showing lines a–b]`, then each line as `N<tab>text` (`cat -n`) — strip the `N<tab>` prefix before reusing text in `edit`/`write_file`, or pass `numbers=false` for raw text. By default the first 2000 lines are shown and a `[read_file: N more lines …]` footer names the rest; select with `offset`/`limit` (1-based line window), `tail=N` (last N lines), `ranges=[[a,b],…]` (several windows, each under an `@@ lines a–b @@` line), or `outline=true` for the code_outline symbol view of an indexed source file. Lines over 2000 chars are cut with ` …[+K chars]` and output is hard-capped at 64 KiB; both are noted in the header. Never write a read result back as file content: the header, numbers and cut markers are not the file's text.".into()
    }

    async fn call(&self, a: ReadFileArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let shown = a.path.display().to_string();
        let path = crate::seat::resolve(cx.session, &a.path);
        crate::seat::refuse_cell_record(&cx.resources.root, &path)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| anyhow::anyhow!("{shown}: {e}"))?;
        let (note, text) = text_of(bytes, &shown).await?;
        let mut notes = Vec::new();
        if a.outline {
            exclusive(&a)?;
            let Some(o) = outliner() else {
                anyhow::bail!("outline needs the codeindex plugin (codeindex.enabled)");
            };
            match o.outline(&path).await? {
                Some(outline) => {
                    return Ok(ToolOutput::ok(format!(
                        "{READ_HEADER_PREFIX}{shown}: {} lines, {} bytes{READ_HEADER_OUTLINE}\n{outline}",
                        text.lines().count(),
                        text.len()
                    )));
                }
                None => notes.push("no outline for this language, showing text".into()),
            }
        }
        Ok(ToolOutput::ok(paginate(
            "read_file",
            &shown,
            note.as_deref(),
            &text,
            &a,
            notes,
        )?))
    }
}

/// `bytes` as text: a detected document (PDF, Office, …) rendered as
/// Markdown with a note saying so — a document that fails to convert is an
/// error the model can act on, never 64 KiB of object streams; undetected
/// bytes are lossy UTF-8.
pub async fn text_of(bytes: Vec<u8>, shown: &str) -> anyhow::Result<(Option<String>, String)> {
    Ok(match crate::doc::format(&bytes) {
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
    })
}

/// `read_file`'s page over `text`: the header
/// `[<tool> <shown>: N lines, B bytes; showing lines a–b]`, `note` if any,
/// the selected lines (`cat -n` unless `numbers=false`), and the
/// `[<tool>: N more lines; use offset/limit, tail or ranges]` footer when
/// the read stopped early. Capped at 64 KiB. `a.path`/`a.outline` are not
/// consulted. Shared with `artifact_read` (66 §6.2).
pub fn paginate(
    tool: &str,
    shown: &str,
    note: Option<&str>,
    text: &str,
    a: &ReadFileArgs,
    mut notes: Vec<String>,
) -> anyhow::Result<String> {
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    let mut sel = select(a, shown, total)?;
    sel.notes.append(&mut notes);
    let width = match sel.windows.iter().map(|w| w.1).max() {
        Some(last) if a.numbers => last.checked_ilog10().map_or(1, |d| d as usize + 1),
        _ => 0,
    };
    let mut body = String::new();
    let mut seps = Vec::new();
    let mut long = 0;
    for &(first, end) in &sel.windows {
        if sel.ranged {
            if !body.is_empty() {
                body.push('\n');
            }
            seps.push(body.len());
            body.push_str(&format!("@@ lines {first}–{end} @@"));
        }
        if end >= first {
            if !body.is_empty() {
                body.push('\n');
            }
            long += render(&mut body, &all[first - 1..end], first, width);
        }
    }
    let requested: usize = sel
        .windows
        .iter()
        .map(|&(f, e)| (e + 1).saturating_sub(f))
        .sum();
    let mut count = requested;
    let cut = body.len() > MAX_BYTES;
    if cut {
        let mut at = MAX_BYTES;
        while !body.is_char_boundary(at) {
            at -= 1;
        }
        body.truncate(at);
        if body.ends_with('\n') {
            body.pop();
        }
        // The line the cut lands in is shown partially and counts;
        // separator lines do not.
        count = body.matches('\n').count() + 1 - seps.iter().filter(|&&s| s < at).count();
    }
    let mut out = format!("[{tool} {shown}: {total} lines, {} bytes", text.len());
    let first = sel.windows.first().map_or(1, |w| w.0);
    let last = first + count - count.min(1);
    if sel.ranged {
        let n = sel.windows.len();
        let s = if n == 1 { "" } else { "s" };
        out.push_str(&format!(
            "{READ_HEADER_SHOWING} {n} range{s}, {count} lines"
        ));
    } else if count == 0 {
        out.push_str(&format!("{READ_HEADER_SHOWING} 0 lines"));
    } else if last < total {
        out.push_str(&format!(
            "{READ_HEADER_SHOWING} lines {first}–{last} of {total}"
        ));
    } else {
        out.push_str(&format!("{READ_HEADER_SHOWING} lines {first}–{last}"));
    }
    for note in &sel.notes {
        out.push_str("; ");
        out.push_str(note);
    }
    if long > 0 {
        out.push_str(&format!("; {long} long lines cut"));
    }
    if cut {
        out.push_str("; cut at 64 KiB — read the rest with offset/limit");
    }
    out.push(']');
    if let Some(note) = note {
        out.push('\n');
        out.push_str(note);
    }
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    if cut || sel.capped {
        let more = if sel.ranged {
            sel.more + requested - count
        } else {
            total - last
        };
        out.push_str(&format!("\n[{tool}: {more} more lines{TRUNCATED_TAIL}"));
    }
    Ok(out)
}

/// The line windows a read shows, resolved against a file of `total` lines.
#[derive(Debug, Default)]
struct Selection {
    /// 1-based inclusive `(first, last)`; `last < first` is an empty window.
    windows: Vec<(usize, usize)>,
    /// `ranges` mode: every window is preceded by an `@@ lines a–b @@` line.
    ranged: bool,
    /// The default or `limit` line cap stopped the read before what was
    /// asked for: the footer is added.
    capped: bool,
    /// Requested lines the cap dropped (`ranges` only; a plain window
    /// derives it from `total`).
    more: usize,
    /// Header notes, e.g. `range 3 clamped to 1800–1842`.
    notes: Vec<String>,
}

/// Err when two selection options that exclude each other are both set.
fn exclusive(a: &ReadFileArgs) -> anyhow::Result<()> {
    let set = |n: &str| match n {
        "tail" => a.tail.is_some(),
        "offset" => a.offset.is_some(),
        "limit" => a.limit.is_some(),
        "ranges" => a.ranges.is_some(),
        "outline" => a.outline,
        _ => !a.numbers,
    };
    for (x, y) in [
        ("tail", "offset"),
        ("tail", "limit"),
        ("tail", "ranges"),
        ("ranges", "offset"),
        ("outline", "offset"),
        ("outline", "limit"),
        ("outline", "tail"),
        ("outline", "ranges"),
        ("outline", "numbers=false"),
    ] {
        anyhow::ensure!(
            !(set(x) && set(y)),
            "`{x}` cannot be combined with `{y}`; pass one of them"
        );
    }
    Ok(())
}

fn select(a: &ReadFileArgs, shown: &str, total: usize) -> anyhow::Result<Selection> {
    let mut sel = Selection::default();
    exclusive(a)?;
    if let Some(n) = a.tail {
        sel.windows.push((total.saturating_sub(n) + 1, total));
        return Ok(sel);
    }
    if let Some(ranges) = &a.ranges {
        anyhow::ensure!(
            !ranges.is_empty(),
            "`ranges` is empty; pass 1-based inclusive [start, end] pairs"
        );
        sel.ranged = true;
        let mut budget = a.limit.unwrap_or(DEFAULT_LINES);
        for (i, &[start, end]) in ranges.iter().enumerate() {
            let i = i + 1;
            anyhow::ensure!(start >= 1, "range {i} [{start}, {end}]: lines are 1-based");
            anyhow::ensure!(
                end >= start,
                "range {i} [{start}, {end}]: end is before start"
            );
            anyhow::ensure!(
                start <= total,
                "{shown}: range {i} [{start}, {end}] is past the end of the file ({total} lines)"
            );
            let mut end = end;
            if end > total {
                end = total;
                sel.notes
                    .push(format!("range {i} clamped to {start}–{end}"));
            }
            let want = end + 1 - start;
            if want > budget {
                sel.capped = true;
                sel.more += want - budget;
                if budget > 0 {
                    sel.windows.push((start, start + budget - 1));
                }
                budget = 0;
            } else {
                budget -= want;
                sel.windows.push((start, end));
            }
        }
        return Ok(sel);
    }
    let first = a.offset.unwrap_or(1).max(1);
    anyhow::ensure!(
        first <= total.max(1),
        "{shown}: offset {first} is past the end of the file ({total} lines)"
    );
    let end = match a.limit {
        Some(n) => (first - 1).saturating_add(n).min(total),
        None => {
            let end = (first - 1 + DEFAULT_LINES).min(total);
            sel.capped = end < total;
            end
        }
    };
    sel.windows.push((first, end));
    Ok(sel)
}

/// Append `lines` to `out`, numbered from `first` in `cat -n` shape
/// (right-aligned to `width`, one tab, the line verbatim) unless `width`
/// is 0. Lines over [`MAX_LINE_CHARS`] are cut with ` …[+K chars]`;
/// returns how many were.
fn render(out: &mut String, lines: &[&str], first: usize, width: usize) -> usize {
    use std::fmt::Write;
    out.reserve(lines.iter().map(|l| l.len() + width + 2).sum());
    let mut long = 0;
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if width > 0 {
            write!(out, "{:>width$}\t", first + i).unwrap();
        }
        match line.char_indices().nth(MAX_LINE_CHARS) {
            Some((at, _)) => {
                long += 1;
                write!(
                    out,
                    "{} …[+{} chars]",
                    &line[..at],
                    line[at..].chars().count()
                )
                .unwrap();
            }
            None => out.push_str(line),
        }
    }
    long
}

/// Arguments for [`WriteFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// File path, absolute or relative to the session's working directory (no `~`).
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
        "Create or overwrite a file with the given content; parent directories are created and the write is atomic. Overwriting an existing text file returns a unified diff of the change; identical content is a no-op. Content carrying a read_file truncation marker is refused. Paths are relative to the working directory; `~` is not expanded.".into()
    }

    async fn call(&self, a: WriteFileArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        refuse_read_output(&a.content)?;
        let shown = a.path.display().to_string();
        let path = crate::seat::writable(cx.session, &a.path)?;
        crate::seat::refuse_cell_record(&cx.resources.root, &path)?;
        let existing = match tokio::fs::read(&path).await {
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
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| anyhow::anyhow!("{}: {e}", parent.display()))?;
                }
                (format!("created {shown} ({bytes} bytes)"), false)
            }
        };
        crate::edit::write_atomic(&path, a.content.as_bytes()).await?;
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
    /// Directory path, absolute or relative to the session's working directory (no `~`).
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
        "List a directory: one entry per line, sorted, directories end with `/`. Paths are relative to the working directory; `~` is not expanded.".into()
    }

    async fn call(&self, a: ListDirArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::ok(
            list_dir(&crate::seat::resolve(cx.session, &a.path)).await?,
        ))
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
    /// File or directory to search; default: the working directory.
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
        "Search file contents with a regex. Respects .gitignore, skips hidden and binary files. Returns `path:line: text` per match, sorted by path, up to `limit` (default 200). `path` is a file or directory (default: cwd); `glob` filters files, e.g. `*.rs`. When an ancestor .gitignore would hide `path` itself, the walk ignores ancestor rules and says so in a header line.".into()
    }

    async fn call(&self, a: GrepArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let root = match a.path {
            Some(p) => crate::seat::resolve(cx.session, &p),
            None => crate::seat::cwd(cx.session)?,
        };
        crate::seat::refuse_cell_record(&cx.resources.root, &root)?;
        let limit = a.limit.unwrap_or(GREP_LIMIT).max(1) as usize;
        let data = cx.resources.root.clone();
        let out = tokio::task::spawn_blocking(move || {
            grep(&root, &data, &a.pattern, a.glob.as_deref(), limit)
        })
        .await??;
        Ok(ToolOutput::ok(out))
    }
}

/// Default match cap for `grep`.
const GREP_LIMIT: u32 = 200;
/// Longest match line echoed back; longer lines are cut with `…`.
const GREP_LINE_CHARS: usize = 400;

/// Walk `root` in parallel (gitignore, hidden and binary files skipped, cell
/// records under the data dir `data` pruned) and collect `path:line: text`
/// matches for `pattern`, sorted by path then line, up to `limit`. Which
/// matches survive the cap depends on thread timing.
fn grep(
    root: &Path,
    data: &Path,
    pattern: &str,
    glob: Option<&str>,
    limit: usize,
) -> anyhow::Result<String> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};
    use ignore::WalkState::{Continue, Quit};

    let matcher = grep_regex::RegexMatcher::new(pattern)?;
    let overrides = match glob {
        Some(glob) => {
            let mut ov = ignore::overrides::OverrideBuilder::new(root);
            ov.add(glob)?;
            Some(ov.build()?)
        }
        None => None,
    };
    let is_dir = root.is_dir();
    let skip = std::sync::Arc::new(crate::seat::cell_record_filter(data, root));
    // One walk; returns (path, line, text) hits and the number of entries
    // yielded below the root.
    let walk = |parents: bool| {
        let mut walk = ignore::WalkBuilder::new(root);
        walk.parents(parents).threads(crate::glob::walk_threads());
        if let Some(ov) = &overrides {
            walk.overrides(ov.clone());
        }
        let skip = skip.clone();
        walk.filter_entry(move |e| !skip(e.path()));
        let hits: Mutex<Vec<(String, u64, String)>> = Mutex::new(Vec::new());
        let count = AtomicUsize::new(0);
        let visited = AtomicUsize::new(0);
        walk.build_parallel().run(|| {
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
            let (matcher, hits, count, visited) = (&matcher, &hits, &count, &visited);
            Box::new(move |entry| {
                let Ok(entry) = entry else { return Continue };
                if entry.depth() > 0 {
                    visited.fetch_add(1, Relaxed);
                }
                if count.load(Relaxed) >= limit {
                    return Quit;
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return Continue;
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
                    matcher,
                    path,
                    UTF8(|line, text| {
                        let n = count.fetch_add(1, Relaxed);
                        if n >= limit {
                            return Ok(false);
                        }
                        let text = text.trim_end();
                        let text = match text.char_indices().nth(GREP_LINE_CHARS) {
                            Some((i, _)) => format!("{}…", &text[..i]),
                            None => text.to_owned(),
                        };
                        hits.lock().unwrap().push((shown.clone(), line, text));
                        Ok(n + 1 < limit)
                    }),
                );
                if count.load(Relaxed) >= limit {
                    return Quit;
                }
                Continue
            })
        });
        (hits.into_inner().unwrap(), visited.into_inner())
    };
    let (mut hits, visited) = walk(true);
    let mut out = Vec::new();
    if is_dir && crate::glob::hidden_by_ancestor(root, visited) {
        let (retry, visited) = walk(false);
        if visited > 0 {
            hits = retry;
            out.push(format!(
                "[grep: an ancestor .gitignore hides {}; walked without ancestor ignore rules]",
                root.display()
            ));
        }
    }
    if hits.is_empty() {
        out.push("no matches".into());
        return Ok(out.join("\n"));
    }
    // The parallel walker yields in arbitrary order; the tool promises path
    // order (component-wise, as the serial `sort_by_file_path` gave).
    hits.sort_by(|a, b| Path::new(&a.0).cmp(Path::new(&b.0)).then(a.1.cmp(&b.1)));
    let capped = hits.len() >= limit;
    out.extend(
        hits.into_iter()
            .map(|(path, line, text)| format!("{path}:{line}: {text}")),
    );
    if capped {
        out.push(format!("[… limit {limit} reached]"));
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(content: &str) -> (crate::TestDir, PathBuf) {
        let dir = crate::TestDir::new("edit");
        let path = dir.0.join("f.txt");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn cx() -> ToolCx {
        ToolCx {
            session: crate::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
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
            tail: None,
            ranges: None,
            numbers: true,
            outline: false,
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
        let (_g, p) = scratch(&text);
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
            format!(
                "{}\n8\tl8\n9\tl9",
                header(&p, 12, text.len(), "lines 8–9 of 12")
            )
        );
        // Empty lines keep their number; a limit past EOF clamps.
        let (_g, p) = scratch("a\n\nc");
        let out = read(args(&p, Some(2), Some(50))).await;
        assert_eq!(out, format!("{}\n2\t\n3\tc", header(&p, 3, 4, "lines 2–3")));
    }

    /// 140 F4E: a seated session's relative paths resolve under its working
    /// directory and its writes stay inside its write root; reads outside pass.
    #[tokio::test]
    async fn seated_session_resolves_relative_paths_and_confines_writes() {
        // Drop guard (141 R4): the scratch dir goes when the test ends, pass or fail.
        let scratch = crate::TestDir::new("seat-fs");
        let dir = scratch.0.clone();
        let wt = dir.join("worktree");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("a.txt"), "in\n").unwrap();
        let elsewhere = dir.join("elsewhere.txt");
        std::fs::write(&elsewhere, "out\n").unwrap();
        let cx = cx();
        let s = cx.session;
        let outside_rel = PathBuf::from("../elsewhere.txt");

        // Unseated: relative paths follow the process cwd, so `a.txt` is not found.
        assert!(
            Tool::call(&ReadFile, args(Path::new("a.txt"), None, None), cx.clone())
                .await
                .is_err()
        );

        crate::seat::set_cwd(s, Some(wt.clone()));
        crate::seat::set_write_root(s, Some(wt.clone()));
        let out = Tool::call(&ReadFile, args(Path::new("a.txt"), None, None), cx.clone())
            .await
            .unwrap()
            .content;
        assert!(out.ends_with("\n1\tin"), "{out}");
        let out = Tool::call(&ListDir, ListDirArgs { path: ".".into() }, cx.clone())
            .await
            .unwrap()
            .content;
        assert_eq!(out, "a.txt");
        // A read outside the root is allowed.
        let out = Tool::call(&ReadFile, args(&outside_rel, None, None), cx.clone())
            .await
            .unwrap()
            .content;
        assert!(out.ends_with("\n1\tout"), "{out}");
        // A relative write lands in the worktree; one outside is refused by name.
        Tool::call(
            &WriteFile,
            WriteFileArgs {
                path: "sub/new.txt".into(),
                content: "new\n".into(),
            },
            cx.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.join("sub/new.txt")).unwrap(),
            "new\n"
        );
        let err = Tool::call(
            &WriteFile,
            WriteFileArgs {
                path: outside_rel.clone(),
                content: "x".into(),
            },
            cx.clone(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("outside this session's write root"), "{err}");
        assert!(err.contains(&wt.display().to_string()), "{err}");
        let err = Tool::call(
            &crate::edit::Edit,
            crate::edit::EditArgs {
                path: elsewhere.clone(),
                old: Some("out".into()),
                new: Some("x".into()),
                edits: None,
                all: false,
            },
            cx.clone(),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("outside this session's write root"), "{err}");
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "out\n");
        crate::seat::forget(s);
    }

    #[tokio::test]
    async fn read_numbers_false_gives_raw_text_with_header() {
        let (_g, p) = scratch("a\n\nc\n");
        let mut a = args(&p, None, None);
        a.numbers = false;
        assert_eq!(
            read(a).await,
            format!("{}\na\n\nc", header(&p, 3, 5, "lines 1–3"))
        );
    }

    #[tokio::test]
    async fn read_empty_file_and_offset_past_eof() {
        let (_g, p) = scratch("");
        assert_eq!(
            read(args(&p, None, None)).await,
            header(&p, 0, 0, "0 lines")
        );
        let (_g, p) = scratch("a\nb\n");
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
        let (_g, p) = scratch(&line.repeat(1000));
        let mut a = args(&p, None, None);
        a.numbers = false;
        let out = read(a).await;
        let head = out.lines().next().unwrap();
        let footer = |more: usize| format!("\n[read_file: {more} more lines{TRUNCATED_TAIL}");
        assert_eq!(
            head,
            header(
                &p,
                1000,
                70000,
                "lines 1–937 of 1000; cut at 64 KiB — read the rest with offset/limit"
            )
        );
        assert!(out.ends_with(&footer(63)), "{}", &out[out.len() - 80..]);
        let body = &out[head.len() + 1..out.len() - footer(63).len()];
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
                "lines 1–874 of 1000; cut at 64 KiB — read the rest with offset/limit"
            )
        );
        assert!(out.ends_with(&footer(126)), "{}", &out[out.len() - 80..]);
        let body = &out[head.len() + 1..out.len() - footer(126).len()];
        assert_eq!(body.len(), MAX_BYTES);
        assert!(body.starts_with(&format!("   1\t{}\n   2\t", "x".repeat(69))));
    }

    #[tokio::test]
    async fn read_defaults_to_2000_lines_with_a_footer() {
        let text: String = (1..=2500).map(|i| format!("{i}\n")).collect();
        let (_g, p) = scratch(&text);
        let out = read(args(&p, None, None)).await;
        let mut want = header(&p, 2500, text.len(), "lines 1–2000 of 2500");
        for i in 1..=2000 {
            want.push_str(&format!("\n{i:>4}\t{i}"));
        }
        want.push_str(&format!("\n[read_file: 500 more lines{TRUNCATED_TAIL}"));
        assert_eq!(out, want);
        // An explicit limit is the model's choice: no footer.
        let out = read(args(&p, Some(2400), Some(50))).await;
        assert!(out.starts_with(&header(&p, 2500, text.len(), "lines 2400–2449 of 2500")));
        assert!(out.ends_with("\n2449\t2449"), "{}", &out[out.len() - 40..]);
        // Offset without limit: the window starts there and may reach EOF.
        let out = read(args(&p, Some(2400), None)).await;
        assert!(out.starts_with(&header(&p, 2500, text.len(), "lines 2400–2500")));
        assert!(out.ends_with("\n2500\t2500"), "{}", &out[out.len() - 40..]);
    }

    #[tokio::test]
    async fn read_cuts_long_lines_at_a_char_boundary() {
        let long = format!("{}é{}", "a".repeat(1999), "b".repeat(10));
        let text = format!("short\n{long}\n{long}\n");
        let (_g, p) = scratch(&text);
        let out = read(args(&p, None, None)).await;
        assert_eq!(
            out,
            format!(
                "{}\n1\tshort\n2\t{cut}\n3\t{cut}",
                header(&p, 3, text.len(), "lines 1–3; 2 long lines cut"),
                cut = format!("{}é …[+10 chars]", "a".repeat(1999)),
            )
        );
        let (_g, p) = scratch(&"a".repeat(2000));
        assert!(!read(args(&p, None, None)).await.contains("…["));
    }

    #[tokio::test]
    async fn read_tail_shows_the_last_lines_with_real_numbers() {
        let text: String = (1..=120).map(|i| format!("l{i}\n")).collect();
        let (_g, p) = scratch(&text);
        let mut a = args(&p, None, None);
        a.tail = Some(3);
        assert_eq!(
            read(a).await,
            format!(
                "{}\n118\tl118\n119\tl119\n120\tl120",
                header(&p, 120, text.len(), "lines 118–120")
            )
        );
        // More than the file has: the whole file, no footer.
        let mut a = args(&p, None, None);
        a.tail = Some(500);
        let out = read(a).await;
        assert!(out.starts_with(&header(&p, 120, text.len(), "lines 1–120")));
        assert!(out.ends_with("\n120\tl120"));
        for (offset, limit, conflict) in [(Some(2), None, "offset"), (None, Some(2), "limit")] {
            let mut a = args(&p, offset, limit);
            a.tail = Some(3);
            let err = Tool::call(&ReadFile, a, cx())
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                format!("`tail` cannot be combined with `{conflict}`; pass one of them")
            );
        }
    }

    struct FakeOutliner;

    #[async_trait::async_trait]
    impl Outliner for FakeOutliner {
        async fn outline(&self, path: &Path) -> anyhow::Result<Option<String>> {
            Ok((path.extension().is_some_and(|e| e == "rs"))
                .then(|| format!("outline of {}", path.display())))
        }
    }

    // The hook is process-global, so every outline expectation lives in
    // this one test: the unregistered error first, then Some/None.
    #[tokio::test]
    async fn read_outline_uses_the_registered_hook() {
        let guard = crate::TestDir::new("outline");
        let dir = guard.0.clone();
        let rs = dir.join("a.rs");
        std::fs::write(&rs, "fn a() {}\nfn b() {}\n").unwrap();
        let outline = |p: &Path| {
            let mut a = args(p, None, None);
            a.outline = true;
            a
        };
        assert_eq!(
            read_err(outline(&rs)).await,
            "outline needs the codeindex plugin (codeindex.enabled)"
        );
        assert!(set_outliner(Arc::new(FakeOutliner)));
        assert!(!set_outliner(Arc::new(FakeOutliner)));
        let out = read(outline(&rs)).await;
        assert_eq!(
            out,
            format!(
                "{READ_HEADER_PREFIX}{}: 2 lines, 20 bytes{READ_HEADER_OUTLINE}\noutline of {}",
                rs.display(),
                rs.display()
            )
        );
        assert!(refuse_read_output(&out).is_err());
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "plain\n").unwrap();
        assert_eq!(
            read(outline(&txt)).await,
            format!(
                "{}\n1\tplain",
                header(
                    &txt,
                    1,
                    6,
                    "lines 1–1; no outline for this language, showing text"
                )
            )
        );
        for (field, conflict) in [("offset", "offset"), ("numbers", "numbers=false")] {
            let mut a = outline(&rs);
            match field {
                "offset" => a.offset = Some(1),
                _ => a.numbers = false,
            }
            assert_eq!(
                read_err(a).await,
                format!("`outline` cannot be combined with `{conflict}`; pass one of them")
            );
        }
    }

    async fn read_err(a: ReadFileArgs) -> String {
        Tool::call(&ReadFile, a, cx())
            .await
            .unwrap_err()
            .to_string()
    }

    #[tokio::test]
    async fn read_ranges_emit_separators_in_the_order_given() {
        let text: String = (1..=120).map(|i| format!("l{i}\n")).collect();
        let (_g, p) = scratch(&text);
        let mut a = args(&p, None, None);
        a.ranges = Some(vec![[100, 101], [5, 6], [100, 100]]);
        assert_eq!(
            read(a).await,
            format!(
                "{}\n@@ lines 100–101 @@\n100\tl100\n101\tl101\n@@ lines 5–6 @@\n  5\tl5\n  6\tl6\n@@ lines 100–100 @@\n100\tl100",
                header(&p, 120, text.len(), "3 ranges, 5 lines")
            )
        );
        // Past EOF clamps with a note; entirely past EOF is an error.
        let mut a = args(&p, None, None);
        a.ranges = Some(vec![[1, 1], [118, 500]]);
        assert_eq!(
            read(a).await,
            format!(
                "{}\n@@ lines 1–1 @@\n  1\tl1\n@@ lines 118–120 @@\n118\tl118\n119\tl119\n120\tl120",
                header(
                    &p,
                    120,
                    text.len(),
                    "2 ranges, 4 lines; range 2 clamped to 118–120"
                )
            )
        );
        let past = format!(
            "{}: range 2 [121, 130] is past the end of the file (120 lines)",
            p.display()
        );
        for (ranges, msg) in [
            (vec![[1, 1], [121, 130]], past.as_str()),
            (vec![[0, 3]], "range 1 [0, 3]: lines are 1-based"),
            (vec![[7, 3]], "range 1 [7, 3]: end is before start"),
            (
                vec![],
                "`ranges` is empty; pass 1-based inclusive [start, end] pairs",
            ),
        ] {
            let mut a = args(&p, None, None);
            a.ranges = Some(ranges);
            assert_eq!(read_err(a).await, msg);
        }
        for (offset, tail, conflict) in [
            (Some(1), None, ("ranges", "offset")),
            (None, Some(1), ("tail", "ranges")),
        ] {
            let mut a = args(&p, offset, None);
            a.tail = tail;
            a.ranges = Some(vec![[1, 2]]);
            assert_eq!(
                read_err(a).await,
                format!(
                    "`{}` cannot be combined with `{}`; pass one of them",
                    conflict.0, conflict.1
                )
            );
        }
    }

    #[tokio::test]
    async fn read_ranges_share_the_line_limit() {
        let text: String = (1..=120).map(|i| format!("l{i}\n")).collect();
        let (_g, p) = scratch(&text);
        let mut a = args(&p, None, Some(3));
        a.ranges = Some(vec![[10, 11], [50, 60], [70, 70]]);
        assert_eq!(
            read(a).await,
            format!(
                "{}\n@@ lines 10–11 @@\n10\tl10\n11\tl11\n@@ lines 50–50 @@\n50\tl50\n[read_file: 11 more lines{TRUNCATED_TAIL}",
                header(&p, 120, text.len(), "2 ranges, 3 lines")
            )
        );
        // The default window applies across ranges too.
        let text: String = (1..=2500).map(|i| format!("{i}\n")).collect();
        let (_g, p) = scratch(&text);
        let mut a = args(&p, None, None);
        a.ranges = Some(vec![[1, 1500], [2000, 2500]]);
        let out = read(a).await;
        assert!(out.starts_with(&header(&p, 2500, text.len(), "2 ranges, 2000 lines")));
        assert!(out.contains("\n@@ lines 2000–2499 @@\n"));
        assert!(out.ends_with(&format!(
            "\n2499\t2499\n[read_file: 1 more lines{TRUNCATED_TAIL}"
        )));
    }

    #[tokio::test]
    async fn read_window_under_cap_has_no_markers() {
        let line = format!("{}\n", "x".repeat(69));
        let (_g, p) = scratch(&line.repeat(1000));
        let mut a = args(&p, Some(2), Some(3));
        a.numbers = false;
        let out = read(a).await;
        assert_eq!(
            out,
            format!(
                "{}\n{}",
                header(&p, 1000, 70000, "lines 2–4 of 1000"),
                line.repeat(3).trim_end()
            )
        );
        let all = read(args(&p, Some(1), Some(1000))).await;
        assert!(all.contains("; cut at 64 KiB") && all.ends_with(TRUNCATED_TAIL));
    }

    #[tokio::test]
    async fn guard_refuses_pasted_header_but_not_tsv_numbers() {
        let (_g, p) = scratch("x");
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
        let (_g, p) = scratch("keep me");
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
        let guard = crate::TestDir::new("write");
        let dir = guard.0.clone();
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
        let (_g, p) = scratch("one\ntwo\n");
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
        let (_g, p) = scratch("#!/bin/sh\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        write(&p, "#!/bin/sh\necho\n").await.unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let guard = crate::TestDir::new("write-ro");
        let dir = guard.0.clone();
        let p = dir.join("f.txt");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let err = write(&p, "new").await.unwrap_err().to_string();
        assert!(err.contains(".tmp-"), "{err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "old");
        assert!(no_tmp_left(&p));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// Temp tree with a `.git` marker so `.gitignore` files are honoured.
    fn grep_tree(files: &[(&str, &str)]) -> (crate::TestDir, PathBuf) {
        let guard = crate::TestDir::new("grep");
        let root = guard.0.clone();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        for (f, content) in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        (guard, root)
    }

    #[test]
    fn grep_sorts_by_path_then_line_and_marks_the_cap() {
        let (_g, root) = grep_tree(&[
            ("b/z.rs", "hit\nmiss\nhit\n"),
            ("a.rs", "hit\n"),
            ("b/y.rs", "hit\n"),
        ]);
        let out = grep(&root, &root, "hit", None, 200).unwrap();
        assert_eq!(
            out,
            "a.rs:1: hit\nb/y.rs:1: hit\nb/z.rs:1: hit\nb/z.rs:3: hit"
        );
        let out = grep(&root, &root, "hit", Some("*.rs"), 2).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert_eq!(lines[2], "[… limit 2 reached]");
        assert!(lines[0] < lines[1], "{out}");
        assert_eq!(
            grep(&root, &root, "nothing_here", None, 200).unwrap(),
            "no matches"
        );
    }

    #[test]
    fn grep_bypasses_an_ancestor_gitignore_that_hides_the_root() {
        let (_g, outer) = grep_tree(&[(".gitignore", "*\n"), ("inner/a.rs", "needle\n")]);
        let inner = outer.join("inner");
        let out = grep(&inner, &outer, "needle", None, 200).unwrap();
        assert_eq!(
            out,
            format!(
                "[grep: an ancestor .gitignore hides {}; walked without ancestor ignore rules]\na.rs:1: needle",
                inner.display()
            )
        );
        // The root's own .gitignore is not an ancestor rule: plain no-match.
        assert_eq!(
            grep(&outer, &outer, "needle", None, 200).unwrap(),
            "no matches"
        );
    }

    /// 171F B24: a user cell's run record (under `<data>/cells/`, or a kernel
    /// job's log) is refused to the file tools, through `..` and symlinks;
    /// the rest of the data dir is not.
    #[tokio::test]
    async fn cell_records_are_refused_to_the_file_tools() {
        let guard = crate::TestDir::new("cell-records");
        let data = guard.0.clone();
        let run = data.join("cells/runs/r_1");
        let jobs = data.join("jobs/s1");
        for d in [&run, &jobs, &data.join("briefs")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(run.join("out.txt"), "SECRET\n").unwrap();
        std::fs::write(jobs.join("j1.cell.log"), "SECRET\n").unwrap();
        std::fs::write(jobs.join("j2.log"), "agent SECRET\n").unwrap();
        std::fs::write(data.join("briefs/x.md"), "brief SECRET\n").unwrap();
        std::os::unix::fs::symlink(run.join("out.txt"), data.join("briefs/link.txt")).unwrap();
        let mut cx = cx();
        cx.resources = crate::SessionResources::durable_arc(&data);
        let refused = |r: anyhow::Result<ToolOutput>| {
            r.err()
                .is_some_and(|e| e.to_string().contains(crate::seat::CELL_RECORD))
        };
        for p in [
            run.join("out.txt"),
            data.join("briefs/../cells/runs/r_1/out.txt"),
            jobs.join("j1.cell.log"),
            data.join("briefs/link.txt"),
        ] {
            let r = Tool::call(&ReadFile, args(&p, None, None), cx.clone()).await;
            assert!(refused(r), "{}", p.display());
        }
        let write = |path: PathBuf| WriteFileArgs {
            path,
            content: "x".into(),
        };
        let r = Tool::call(&WriteFile, write(run.join("new.txt")), cx.clone()).await;
        assert!(refused(r));
        let edit: crate::edit::EditArgs = serde_json::from_value(serde_json::json!({
            "path": run.join("out.txt"), "old": "SECRET", "new": "x"
        }))
        .unwrap();
        assert!(refused(
            Tool::call(&crate::edit::Edit, edit, cx.clone()).await
        ));
        let grep_in = |path: PathBuf| GrepArgs {
            pattern: "SECRET".into(),
            path: Some(path),
            glob: None,
            limit: None,
        };
        assert!(refused(
            Tool::call(&Grep, grep_in(run.clone()), cx.clone()).await
        ));
        let glob: crate::glob::GlobArgs = serde_json::from_value(serde_json::json!({
            "pattern": "*", "path": data.join("cells")
        }))
        .unwrap();
        assert!(refused(
            Tool::call(&crate::glob::Glob, glob, cx.clone()).await
        ));
        // A walk of the data dir skips the records and finds the rest.
        let out = Tool::call(&Grep, grep_in(data.clone()), cx.clone())
            .await
            .unwrap()
            .content;
        assert_eq!(
            out,
            "briefs/x.md:1: brief SECRET\njobs/s1/j2.log:1: agent SECRET"
        );
        // Neighbours read and write as before.
        for p in [data.join("briefs/x.md"), jobs.join("j2.log")] {
            let r = Tool::call(&ReadFile, args(&p, None, None), cx.clone()).await;
            assert!(r.is_ok(), "{}", p.display());
        }
        let r = Tool::call(&WriteFile, write(data.join("briefs/y.md")), cx.clone()).await;
        assert!(r.is_ok());
    }
}
