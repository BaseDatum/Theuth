//! The `show_file` and `show_diff` tools, their [`FileViewOpened`] / [`DiffViewOpened`] events
//! and the content behind the `file_view_read` / `diff_view_read` RPCs (design 212). Events and
//! receipts carry a descriptor only (a path, a [`DiffSpec`]); the tile fetches the text through
//! the RPCs, which serve what this session showed in this engine lifetime or anything inside its
//! working directory or repository (§2). The receipt's first two lines are the transcript's only
//! record — the UI parses them.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::tileview::NO_APP;
use crate::webview::clean_title;

pub mod watch;

/// Largest file `show_file` opens and `file_view_read` returns (and `show_diff` compares), in bytes.
pub const MAX_FILE: u64 = 2 * 1024 * 1024;

/// Largest patch text `diff_view_read` returns, in bytes.
pub const MAX_DIFF: usize = 1024 * 1024;

/// Bytes searched for a NUL to call a file binary.
const SNIFF: usize = 8 * 1024;

/// Longest commit subject in a default `show_diff` title, in characters.
const MAX_SUBJECT: usize = 60;

/// Extension-less names the UI highlights by name (`lang` is the name itself).
const WELL_KNOWN: &[&str] = &[
    "Dockerfile",
    "Containerfile",
    "Makefile",
    "GNUmakefile",
    "makefile",
    "justfile",
    "Justfile",
    "Rakefile",
    "Gemfile",
    "Procfile",
    "Vagrantfile",
    "CMakeLists.txt",
];

/// The refusal of `file_view_read` / `subscribe_file_view` outside what §2 allows.
const NOT_SHOWN_FILE: &str = "not a shown file";
/// The refusal of `diff_view_read` outside what §2 allows.
const NOT_SHOWN_DIFF: &str = "not a shown diff";

// ---------------------------------------------------------------------------------------------
// Wire types

/// Arguments of `show_file` (212 §1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowFileArgs {
    /// The file: absolute, or relative to the session's working directory.
    pub path: PathBuf,
    /// First line to scroll to and mark (1-based).
    pub line: Option<u32>,
    /// Last marked line (1-based, needs `line`).
    pub end_line: Option<u32>,
    /// Tile and card title. Default: the file name.
    pub title: Option<String>,
}

/// Event: a `show_file` call opened a file tile (212 §1).
/// Wire shape `{ "type": "FileViewOpened", "session", "call_id", "id", "path", "title", "lang",
/// "line", "end_line", "size", "lines" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct FileViewOpened {
    /// Calling session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// Tile id: 16 hex of sha256(session + "\n" + path); the same file reuses its tile.
    pub id: String,
    /// Absolute (canonical) path; what `file_view_read` takes.
    pub path: String,
    /// Tile and card title.
    pub title: String,
    /// Lowercase extension, or a well-known file name (`Dockerfile`); empty when neither.
    pub lang: String,
    /// First marked line (1-based).
    pub line: Option<u32>,
    /// Last marked line.
    pub end_line: Option<u32>,
    /// Bytes at call time.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    /// Lines at call time.
    pub lines: u32,
}

impl wire::WireEvent for FileViewOpened {
    const TOPIC: &'static str = "artifact.file-view-opened";
}

/// Arguments of `show_diff` (212 §1): at most one of `commit`, `range`, `staged`,
/// `file_a`+`file_b`; none is the working tree against HEAD.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowDiffArgs {
    /// One commit against its first parent (any revision: `HEAD~2`, a sha, a tag).
    pub commit: Option<String>,
    /// `A..B` or `A...B` (an empty side is HEAD), as `git diff` takes it.
    pub range: Option<String>,
    /// `true`: the staged changes (`git diff --cached`).
    pub staged: Option<bool>,
    /// Compare two files (need not be in a repository): the old one.
    pub file_a: Option<PathBuf>,
    /// The new one.
    pub file_b: Option<PathBuf>,
    /// Limit a git diff to these pathspecs (relative to the working directory or the repository root).
    pub paths: Option<Vec<String>>,
    /// Tile and card title. Default: what is compared.
    pub title: Option<String>,
}

/// Which diff a [`DiffSpec`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum DiffMode {
    /// Working tree and index against HEAD, untracked files included.
    Worktree,
    /// The index against HEAD.
    Staged,
    /// `a` against its first parent (a root commit against the empty tree).
    Commit,
    /// `a..b` (or `a...b` with `three_dot`).
    Range,
    /// File `a` against file `b`.
    Files,
}

/// What a diff tile shows and re-requests through `diff_view_read` (212 §1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DiffSpec {
    /// Which diff.
    pub mode: DiffMode,
    /// Repository root (absolute) for the git modes; `null` for `files`.
    pub root: Option<String>,
    /// `commit`/`range`: a full sha; `files`: the old file's absolute path.
    pub a: Option<String>,
    /// `range`: a full sha; `files`: the new file's absolute path.
    pub b: Option<String>,
    /// `range`: `a...b` (from the merge base).
    #[serde(default)]
    pub three_dot: bool,
    /// Pathspecs relative to `root` (git modes).
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Event: a `show_diff` call opened a diff tile (212 §1).
/// Wire shape `{ "type": "DiffViewOpened", "session", "call_id", "id", "title", "spec", "files",
/// "added", "removed" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct DiffViewOpened {
    /// Calling session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// Tile id: 16 hex of sha256(session + "\n" + the spec's JSON); the same diff reuses its tile.
    pub id: String,
    /// Tile and card title.
    pub title: String,
    /// What the tile fetches with `diff_view_read`.
    pub spec: DiffSpec,
    /// Files changed at call time.
    pub files: u32,
    /// Lines added at call time.
    pub added: u32,
    /// Lines removed at call time.
    pub removed: u32,
}

impl wire::WireEvent for DiffViewOpened {
    const TOPIC: &'static str = "artifact.diff-view-opened";
}

/// Result of `file_view_read` (212 §2, §6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FileViewText {
    /// The content (at most 2 MiB, cut on a character boundary); empty when `unchanged`.
    pub text: String,
    /// The file's size in bytes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    /// Lines in `text`.
    pub lines: u32,
    /// Modification time, ms since the epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mtime_ms: u64,
    /// `text` stops at the cap.
    pub truncated: bool,
    /// The file is not UTF-8; invalid bytes became U+FFFD.
    pub lossy: bool,
    /// As [`FileViewOpened::lang`].
    pub lang: String,
    /// `if_mtime_ms` matched: nothing changed, `text` is empty.
    #[serde(default)]
    pub unchanged: bool,
}

/// One file of a [`DiffViewText`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DiffFileStat {
    /// Path (the new one for a rename), relative to the repository root; `files`: file_b.
    pub path: String,
    /// `A` added, `M` modified, `D` deleted, `R` renamed, `?` untracked.
    pub status: String,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// Result of `diff_view_read` (212 §2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DiffViewText {
    /// The unified patch (at most 1 MiB, then a `[diff truncated at N bytes]` line).
    pub text: String,
    /// Every file of the whole patch, in patch order.
    pub files: Vec<DiffFileStat>,
    /// Lines added over the whole patch.
    pub added: u32,
    /// Lines removed over the whole patch.
    pub removed: u32,
    /// `text` stops at the cap.
    pub truncated: bool,
}

/// One `subscribe_file_view` item (212 §6): the file settled after a change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FileViewChange {
    /// Modification time, ms since the epoch (0 when deleted).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mtime_ms: u64,
    /// Size in bytes (0 when deleted).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    /// The file is gone; a later item with `deleted: false` means it is back.
    pub deleted: bool,
}

// ---------------------------------------------------------------------------------------------
// What this engine published (212 §2 rule (a))

/// Per session: `file:<path>` and `diff:<spec JSON>` this engine's tools published.
static SHOWN: LazyLock<Mutex<HashMap<SessionId, HashSet<String>>>> =
    LazyLock::new(Default::default);

fn file_key(path: &str) -> String {
    format!("file:{path}")
}

fn diff_key(spec: &DiffSpec) -> String {
    format!("diff:{}", spec_json(spec))
}

fn remember(session: SessionId, key: String) {
    SHOWN.lock().entry(session).or_default().insert(key);
}

fn was_shown(session: SessionId, key: &str) -> bool {
    SHOWN.lock().get(&session).is_some_and(|s| s.contains(key))
}

/// Forget what `sessions` showed (the session was deleted or its tree closed).
pub fn forget(sessions: &[SessionId]) {
    let mut all = SHOWN.lock();
    for s in sessions {
        all.remove(s);
    }
}

// ---------------------------------------------------------------------------------------------
// show_file

/// `show_file`: resolve and check the file, publish [`FileViewOpened`], return the receipt (212 §1).
pub struct ShowFileTool {
    /// For publishing the event.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowFileTool {
    const NAME: &'static str = "show_file";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowFileArgs;

    fn description(&self) -> String {
        "Open a source file in a tile beside the chat, raw and syntax highlighted with line \
         numbers, scrolled to and marking `line`..`end_line` (1-based), for code the user should \
         look at; the tile follows the file as it changes. `path` is absolute or relative to the \
         working directory; `title` names the tile (default the file name); the same file again \
         reuses its tile. show is for rendered documents; show_web for web pages."
            .into()
    }

    async fn call(&self, args: ShowFileArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cwd = tools::seat::cwd(cx.session)?;
        let f = match open_file(&args, &cwd, &cx.resources.root) {
            Ok(f) => f,
            Err(msg) => return Ok(ToolOutput::err(format!("show_file: {msg}"))),
        };
        let title = args
            .title
            .as_deref()
            .map(clean_title)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                clean_title(
                    Path::new(&f.path)
                        .file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                        .as_ref(),
                )
            });
        let id = tile_id(cx.session, &f.path);
        remember(cx.session, file_key(&f.path));
        self.rt
            .publish(FileViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                id: id.clone(),
                path: f.path.clone(),
                title: title.clone(),
                lang: f.lang.clone(),
                line: f.line,
                end_line: f.end_line,
                size: f.size,
                lines: f.lines,
            })
            .await;
        let content = file_receipt(&title, &id, &f, crate::preview::feed_live());
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "session": cx.session,
                "id": id,
                "path": f.path,
                "title": title,
                "lang": f.lang,
                "line": f.line,
                "end_line": f.end_line,
                "size": f.size,
                "lines": f.lines,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// A file `show_file` accepted.
#[derive(Debug, PartialEq, Eq)]
struct OpenedFile {
    path: String,
    lang: String,
    size: u64,
    lines: u32,
    line: Option<u32>,
    end_line: Option<u32>,
}

/// Resolve `args.path` against `cwd` and check it (212 §1). `cell_root` is the session's
/// resource root for the cell-record refusal.
fn open_file(args: &ShowFileArgs, cwd: &Path, cell_root: &Path) -> Result<OpenedFile, String> {
    if args.path.as_os_str().is_empty() {
        return Err("`path` is required".into());
    }
    let (real, bytes) = load_text(&cwd.join(&args.path), cell_root)?;
    let lines = count_lines(&bytes);
    let path = real.to_string_lossy().into_owned();
    check_range(args.line, args.end_line, lines, &path)?;
    Ok(OpenedFile {
        lang: lang_of(&real),
        size: bytes.len() as u64,
        lines,
        line: args.line,
        end_line: args.end_line,
        path,
    })
}

/// A text file's canonical path and bytes: not a cell record, a regular file, at most
/// [`MAX_FILE`], not binary.
fn load_text(path: &Path, cell_root: &Path) -> Result<(PathBuf, Vec<u8>), String> {
    tools::seat::refuse_cell_record(cell_root, path).map_err(|e| e.to_string())?;
    let real = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", real.display()));
    }
    if meta.len() > MAX_FILE {
        return Err(format!(
            "{} is {} bytes; at most {MAX_FILE} bytes (2 MiB) open in a tile",
            real.display(),
            meta.len()
        ));
    }
    let bytes = std::fs::read(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if is_binary(&bytes) {
        return Err(format!(
            "{} is binary — use show for images and documents",
            real.display()
        ));
    }
    Ok((real, bytes))
}

/// A NUL in the first 8 KiB, or an image.
fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF)].contains(&0) || tools::images::sniff(bytes).is_some()
}

/// Lines of `bytes`: newlines, plus one for an unterminated last line.
fn count_lines(bytes: &[u8]) -> u32 {
    let nl = bytes.iter().filter(|b| **b == b'\n').count();
    let tail = usize::from(bytes.last().is_some_and(|b| *b != b'\n'));
    u32::try_from(nl + tail).unwrap_or(u32::MAX)
}

/// `line`/`end_line` are 1-based, `end_line` needs `line` and is not before it, both within the file.
fn check_range(line: Option<u32>, end: Option<u32>, lines: u32, path: &str) -> Result<(), String> {
    match (line, end) {
        (None, Some(_)) => return Err("`end_line` needs `line`".into()),
        (Some(0), _) | (_, Some(0)) => return Err("lines are 1-based".into()),
        (Some(l), Some(e)) if e < l => {
            return Err(format!("`end_line` {e} is before `line` {l}"));
        }
        _ => {}
    }
    match end.or(line) {
        Some(n) if n > lines => Err(format!(
            "{path} has {lines} lines; line {n} is past the end"
        )),
        _ => Ok(()),
    }
}

/// The lowercase extension, or a well-known extension-less name; empty when neither.
pub fn lang_of(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if WELL_KNOWN.contains(&name) {
        return name.to_owned();
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// First 16 hex of sha256(session + "\n" + `what`).
fn tile_id(session: SessionId, what: &str) -> String {
    let digest = Sha256::digest(format!("{session}\n{what}").as_bytes());
    hex::encode(digest)[..16].to_owned()
}

/// The text the model reads; the first two lines are fixed (212 §1, §6).
fn file_receipt(title: &str, id: &str, f: &OpenedFile, live: bool) -> String {
    let mut s = format!("file view opened · {title} · tile file:{id}\npath: {}", f.path);
    match (f.line, f.end_line) {
        (Some(l), Some(e)) => {
            let _ = write!(s, "#L{l}-L{e}");
        }
        (Some(l), None) => {
            let _ = write!(s, "#L{l}");
        }
        _ => {}
    }
    let _ = write!(s, "\n{} lines, {} bytes.", f.lines, f.size);
    if !live {
        let _ = write!(s, "\n{NO_APP}");
    }
    s.push_str("\nThe tile follows the file as it changes on disk.");
    s
}

// ---------------------------------------------------------------------------------------------
// show_diff

/// `show_diff`: plan the spec (revisions resolved now), compute the diff once to validate and
/// count, publish [`DiffViewOpened`], return the receipt (212 §1).
pub struct ShowDiffTool {
    /// For publishing the event.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowDiffTool {
    const NAME: &'static str = "show_diff";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowDiffArgs;

    fn description(&self) -> String {
        "Open a colorized diff in a tile beside the chat for the user to review: by default the \
         working tree against HEAD (untracked files included); `staged: true` the staged changes; \
         `commit` one commit against its parent; `range` like `main..HEAD` or `main...HEAD`; or \
         `file_a` + `file_b` two files. `paths` limits a git diff to pathspecs; `title` names \
         the tile. The same diff again reuses its tile. show is for rendered documents; show_web \
         for web pages."
            .into()
    }

    async fn call(&self, args: ShowDiffArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cwd = tools::seat::cwd(cx.session)?;
        let root = cx.resources.root.clone();
        let planned = tokio::task::spawn_blocking(move || {
            let (spec, title) = plan_diff(&args, &cwd, &root)?;
            let diff = compute(&spec, &root)?;
            Ok::<_, String>((spec, title, diff))
        })
        .await?;
        let (spec, title, diff) = match planned {
            Ok(p) => p,
            Err(msg) => return Ok(ToolOutput::err(format!("show_diff: {msg}"))),
        };
        let id = tile_id(cx.session, &spec_json(&spec));
        remember(cx.session, diff_key(&spec));
        let (files, added, removed) = diff.totals();
        self.rt
            .publish(DiffViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                id: id.clone(),
                title: title.clone(),
                spec: spec.clone(),
                files,
                added,
                removed,
            })
            .await;
        let content = diff_receipt(&title, &id, &spec, (files, added, removed), crate::preview::feed_live());
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "session": cx.session,
                "id": id,
                "title": title,
                "spec": spec,
                "files": files,
                "added": added,
                "removed": removed,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// The spec's compact JSON (field order fixed by the struct): the receipt's `spec:` line and the id's input.
fn spec_json(spec: &DiffSpec) -> String {
    serde_json::to_string(spec).expect("DiffSpec serializes")
}

/// The text the model reads; the first two lines are fixed (212 §1).
fn diff_receipt(
    title: &str,
    id: &str,
    spec: &DiffSpec,
    (files, added, removed): (u32, u32, u32),
    live: bool,
) -> String {
    let mut s = format!(
        "diff view opened · {title} · tile diff:{id}\nspec: {}",
        spec_json(spec)
    );
    if files == 0 {
        s.push_str("\nno changes.");
    } else {
        let noun = if files == 1 { "file" } else { "files" };
        let _ = write!(s, "\n{files} {noun} changed, +{added} \u{2212}{removed}.");
    }
    if !live {
        let _ = write!(s, "\n{NO_APP}");
    }
    if matches!(spec.mode, DiffMode::Worktree | DiffMode::Staged) {
        s.push_str("\nIt shows the tree as it is now; the user presses Refresh to see later changes.");
    }
    s
}

/// The spec and default title a call names. Revisions resolve to full shas now; relative files
/// and pathspecs resolve against `cwd`.
fn plan_diff(args: &ShowDiffArgs, cwd: &Path, cell_root: &Path) -> Result<(DiffSpec, String), String> {
    let given = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let (commit, range) = (given(&args.commit), given(&args.range));
    let staged = args.staged == Some(true);
    let files = args.file_a.is_some() || args.file_b.is_some();
    let modes = [commit.is_some(), range.is_some(), staged, files];
    if modes.iter().filter(|m| **m).count() > 1 {
        return Err("give one of commit, range, staged or file_a+file_b".into());
    }
    let paths: Vec<String> = args
        .paths
        .iter()
        .flatten()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    let with_paths = |title: String| {
        if paths.is_empty() {
            title
        } else {
            format!("{title} · {}", paths.join(", "))
        }
    };
    if files {
        if !paths.is_empty() {
            return Err("`paths` applies to git diffs, not file_a+file_b".into());
        }
        let (Some(a), Some(b)) = (&args.file_a, &args.file_b) else {
            return Err("file_a and file_b go together".into());
        };
        let (a, _) = load_text(&cwd.join(a), cell_root)?;
        let (b, _) = load_text(&cwd.join(b), cell_root)?;
        let name = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let title = format!("{} \u{2194} {}", name(&a), name(&b));
        let spec = DiffSpec {
            mode: DiffMode::Files,
            root: None,
            a: Some(a.to_string_lossy().into_owned()),
            b: Some(b.to_string_lossy().into_owned()),
            three_dot: false,
            paths: Vec::new(),
        };
        return Ok((spec, title));
    }
    let root = repo_root(cwd)
        .ok_or_else(|| format!("{} is not in a git repository", cwd.display()))?;
    let mut spec = DiffSpec {
        mode: DiffMode::Worktree,
        root: Some(root.to_string_lossy().into_owned()),
        a: None,
        b: None,
        three_dot: false,
        paths: paths.iter().map(|p| pathspec(cwd, &root, p)).collect(),
    };
    let title = if let Some(rev) = commit {
        let sha = resolve_rev(&root, &rev)?;
        let line = git_text(&root, &["log", "-1", "--no-color", "--format=%h%x00%s", &sha, "--"])?;
        let (short, subject) = line.trim_end().split_once('\0').unwrap_or((&sha[..7], ""));
        spec.mode = DiffMode::Commit;
        spec.a = Some(sha.clone());
        format!("{short} {}", truncate_chars(subject, MAX_SUBJECT)).trim_end().to_owned()
    } else if let Some(r) = range {
        let (a, b, three) = if let Some((a, b)) = r.split_once("...") {
            (a, b, true)
        } else if let Some((a, b)) = r.split_once("..") {
            (a, b, false)
        } else {
            return Err(format!("range `{r}` is not A..B or A...B"));
        };
        let side = |s: &str| if s.is_empty() { "HEAD".to_owned() } else { s.to_owned() };
        spec.mode = DiffMode::Range;
        spec.a = Some(resolve_rev(&root, &side(a))?);
        spec.b = Some(resolve_rev(&root, &side(b))?);
        spec.three_dot = three;
        r
    } else if staged {
        spec.mode = DiffMode::Staged;
        "staged".into()
    } else {
        "working tree".into()
    };
    Ok((spec, with_paths(title)))
}

/// `s` cut to `max` characters, the last one `…` when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// A pathspec relative to `root`: a path that exists (from `cwd`) becomes root-relative; one that
/// does not (a deleted file, a glob) is taken from `cwd`'s place in the repository; `:` magic is kept.
fn pathspec(cwd: &Path, root: &Path, p: &str) -> String {
    if p.starts_with(':') {
        return p.to_owned();
    }
    let rel = |abs: &Path| {
        abs.strip_prefix(root).ok().map(|r| {
            let s = r.to_string_lossy().into_owned();
            if s.is_empty() { ".".to_owned() } else { s }
        })
    };
    if let Ok(real) = std::fs::canonicalize(cwd.join(p))
        && let Some(r) = rel(&real)
    {
        return r;
    }
    if Path::new(p).is_relative()
        && let Ok(base) = std::fs::canonicalize(cwd)
        && let Some(r) = rel(&base.join(p))
    {
        return if r == "." { p.to_owned() } else { r };
    }
    p.to_owned()
}

/// The repository root containing `dir` (canonical), when it is in one.
fn repo_root(dir: &Path) -> Option<PathBuf> {
    let top = git_text(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    std::fs::canonicalize(top.trim_end_matches('\n')).ok()
}

/// `rev` as a full commit sha, or `unknown revision`.
fn resolve_rev(root: &Path, rev: &str) -> Result<String, String> {
    let unknown = || format!("unknown revision `{rev}`");
    if rev.starts_with('-') {
        return Err(unknown());
    }
    git_text(root, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| is_sha(s))
        .ok_or_else(unknown)
}

/// A full sha-1 or sha-256 object name.
fn is_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A `git` command in `dir`: `GIT_*` scrubbed, no pager, hooks and prompts off, paths unquoted
/// (the spelling of `crates/autoresearch/src/git.rs`). `GIT_OPTIONAL_LOCKS=0`: a read never takes
/// the index lock to refresh it.
fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            c.env_remove(k);
        }
    }
    c.arg("--no-pager")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.quotepath=off",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "log.showSignature=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    c
}

/// Run `git <args>` in `dir`; non-zero exit is an error with git's last stderr line.
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = git_cmd(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "git {}: {}",
            args.first().copied().unwrap_or(""),
            err.trim().lines().last().unwrap_or("failed")
        ));
    }
    Ok(out.stdout)
}

fn git_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    git(dir, args).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The empty tree's object name in this repository's hash.
fn empty_tree(root: &Path) -> Result<String, String> {
    Ok(git_text(root, &["hash-object", "-t", "tree", "/dev/null"])?
        .trim()
        .to_owned())
}

/// A whole diff with per-file numbers.
#[derive(Debug, Default)]
struct Diff {
    text: String,
    files: Vec<DiffFileStat>,
}

impl Diff {
    /// `(files, added, removed)`.
    fn totals(&self) -> (u32, u32, u32) {
        let files = u32::try_from(self.files.len()).unwrap_or(u32::MAX);
        let added = self.files.iter().map(|f| f.added).fold(0u32, u32::saturating_add);
        let removed = self.files.iter().map(|f| f.removed).fold(0u32, u32::saturating_add);
        (files, added, removed)
    }

    /// The RPC reply: the text cut at `cap` bytes (on a character boundary) with a note.
    fn into_view(self, cap: usize) -> DiffViewText {
        let (_, added, removed) = self.totals();
        let mut text = self.text;
        let truncated = text.len() > cap;
        if truncated {
            let n = floor_char_boundary(&text, cap);
            text.truncate(n);
            let _ = write!(text, "\n[diff truncated at {n} bytes]");
        }
        DiffViewText {
            text,
            files: self.files,
            added,
            removed,
            truncated,
        }
    }
}

fn floor_char_boundary(s: &str, at: usize) -> usize {
    let mut n = at.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// Shape checks before a spec reaches git: absolute root and files, full shas (never an option).
fn validate_spec(spec: &DiffSpec) -> Result<(), String> {
    let abs = |p: &Option<String>| p.as_deref().is_some_and(|p| Path::new(p).is_absolute());
    let sha = |p: &Option<String>| p.as_deref().is_some_and(is_sha);
    let ok = match spec.mode {
        DiffMode::Files => abs(&spec.a) && abs(&spec.b) && spec.paths.is_empty(),
        DiffMode::Worktree | DiffMode::Staged => abs(&spec.root),
        DiffMode::Commit => abs(&spec.root) && sha(&spec.a),
        DiffMode::Range => abs(&spec.root) && sha(&spec.a) && sha(&spec.b),
    };
    if ok { Ok(()) } else { Err("invalid diff spec".into()) }
}

/// Compute the diff a spec names, in full.
fn compute(spec: &DiffSpec, cell_root: &Path) -> Result<Diff, String> {
    validate_spec(spec)?;
    if spec.mode == DiffMode::Files {
        let (a, b) = (spec.a.as_deref().unwrap_or(""), spec.b.as_deref().unwrap_or(""));
        let (_, old) = load_text(Path::new(a), cell_root)?;
        let (_, new) = load_text(Path::new(b), cell_root)?;
        let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
        let text = similar::TextDiff::from_lines(old.as_ref(), new.as_ref())
            .unified_diff()
            .context_radius(3)
            .header(
                &format!("a/{}", a.trim_start_matches('/')),
                &format!("b/{}", b.trim_start_matches('/')),
            )
            .to_string();
        let mut files = file_stats(&text);
        for f in &mut files {
            f.path = b.to_owned();
        }
        return Ok(Diff { text, files });
    }
    let root = Path::new(spec.root.as_deref().unwrap_or(""));
    let mut args: Vec<String> = ["diff", "--no-color", "--no-ext-diff", "-M"]
        .map(String::from)
        .to_vec();
    let a = spec.a.clone().unwrap_or_default();
    match spec.mode {
        DiffMode::Worktree => {
            let head = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
            args.push(if head { "HEAD".into() } else { empty_tree(root)? });
        }
        DiffMode::Staged => args.push("--cached".into()),
        DiffMode::Commit => {
            let parent = git_text(root, &["rev-parse", "--verify", "--quiet", &format!("{a}^1")]);
            args.push(match parent {
                Ok(p) => p.trim().to_owned(),
                Err(_) => empty_tree(root)?,
            });
            args.push(a);
        }
        DiffMode::Range => {
            let dots = if spec.three_dot { "..." } else { ".." };
            args.push(format!("{a}{dots}{}", spec.b.as_deref().unwrap_or("")));
        }
        DiffMode::Files => unreachable!("handled above"),
    }
    args.push("--".into());
    args.extend(spec.paths.iter().cloned());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut text = git_text(root, &argv)?;
    let mut files = file_stats(&text);
    if spec.mode == DiffMode::Worktree {
        let mut ls = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
        ls.extend(spec.paths.iter().map(String::as_str));
        let listed = git(root, &ls)?;
        let mut untracked = String::new();
        for rel in listed.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            new_file_patch(root, &String::from_utf8_lossy(rel), &mut untracked);
        }
        files.extend(file_stats(&untracked).into_iter().map(|mut f| {
            f.status = "?".into();
            f
        }));
        text.push_str(&untracked);
    }
    Ok(Diff { text, files })
}

/// An untracked file as git's new-file patch (the index is never touched): symlinks show their
/// target; files over [`MAX_FILE`] or binary show as binary.
fn new_file_patch(root: &Path, rel: &str, out: &mut String) {
    let path = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return;
    };
    let (mode, bytes) = if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&path).unwrap_or_default();
        ("120000", Some(target.to_string_lossy().into_owned().into_bytes()))
    } else {
        #[cfg(unix)]
        let exec = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0;
        #[cfg(not(unix))]
        let exec = false;
        let mode = if exec { "100755" } else { "100644" };
        let bytes = (meta.len() <= MAX_FILE)
            .then(|| std::fs::read(&path).ok())
            .flatten();
        (mode, bytes)
    };
    let _ = write!(out, "diff --git a/{rel} b/{rel}\nnew file mode {mode}\n");
    let bytes = match bytes {
        Some(b) if b.is_empty() => return,
        Some(b) if !is_binary(&b) => b,
        _ => {
            let _ = writeln!(out, "Binary files /dev/null and b/{rel} differ");
            return;
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let n = count_lines(&bytes);
    let _ = write!(out, "--- /dev/null\n+++ b/{rel}\n");
    if n == 1 {
        out.push_str("@@ -0,0 +1 @@\n");
    } else {
        let _ = writeln!(out, "@@ -0,0 +1,{n} @@");
    }
    for line in text.split_inclusive('\n') {
        out.push('+');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}

/// Per-file numbers of a unified patch (git's `diff --git` sections, or bare `---`/`+++` ones).
/// Hunk bodies are walked by their header counts, so a removed line starting `--` is not a header.
fn file_stats(patch: &str) -> Vec<DiffFileStat> {
    enum St {
        Out,
        Header,
        Hunk { old: u32, new: u32 },
    }
    let mut out: Vec<DiffFileStat> = Vec::new();
    let mut st = St::Out;
    let strip = |p: &str, pre: &str| p.strip_prefix(pre).unwrap_or(p).to_owned();
    for line in patch.lines() {
        if let St::Hunk { old, new } = &mut st {
            let Some(cur) = out.last_mut() else { break };
            match line.as_bytes().first() {
                Some(b'+') => {
                    cur.added += 1;
                    *new = new.saturating_sub(1);
                }
                Some(b'-') => {
                    cur.removed += 1;
                    *old = old.saturating_sub(1);
                }
                Some(b'\\') => {}
                _ => {
                    *old = old.saturating_sub(1);
                    *new = new.saturating_sub(1);
                }
            }
            if *old == 0 && *new == 0 {
                st = St::Out;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            out.push(DiffFileStat {
                path: git_header_path(rest),
                status: "M".into(),
                ..DiffFileStat::default()
            });
            st = St::Header;
            continue;
        }
        if line.starts_with("--- ") && matches!(st, St::Out) {
            out.push(DiffFileStat {
                status: "M".into(),
                ..DiffFileStat::default()
            });
            st = St::Header;
        }
        let Some(cur) = out.last_mut() else { continue };
        if let Some(h) = line.strip_prefix("@@ ") {
            let (old, new) = hunk_counts(h);
            st = if old == 0 && new == 0 {
                St::Out
            } else {
                St::Hunk { old, new }
            };
            continue;
        }
        if !matches!(st, St::Header) {
            continue;
        }
        if line.starts_with("new file mode") {
            cur.status = "A".into();
        } else if line.starts_with("deleted file mode") {
            cur.status = "D".into();
        } else if let Some(p) = line.strip_prefix("rename to ") {
            cur.status = "R".into();
            cur.path = p.to_owned();
        } else if line.starts_with("rename from ") {
            cur.status = "R".into();
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if p != "/dev/null" && cur.status != "R" {
                cur.path = strip(p, "b/");
            }
        } else if let Some(p) = line.strip_prefix("--- ")
            && p != "/dev/null"
            && cur.path.is_empty()
        {
            cur.path = strip(p, "a/");
        }
    }
    out
}

/// `(old, new)` line counts of a hunk header after `@@ `.
fn hunk_counts(h: &str) -> (u32, u32) {
    let mut it = h.split_whitespace();
    // `-12,3` / `+5` (a missing count is 1).
    let count = |t: Option<&str>| {
        t.and_then(|t| t.get(1..))
            .and_then(|t| t.split_once(',').map_or(Some(1), |(_, n)| n.parse().ok()))
            .unwrap_or(0)
    };
    let old = count(it.next());
    let new = count(it.next());
    (old, new)
}

/// The path of `a/X b/Y` (git's header): the new side; equal halves split exactly.
fn git_header_path(rest: &str) -> String {
    let n = rest.len();
    if n >= 5 && (n - 5).is_multiple_of(2) {
        let half = (n - 5) / 2;
        if let Some(p) = rest.get(2..2 + half)
            && rest == format!("a/{p} b/{p}")
        {
            return p.to_owned();
        }
    }
    rest.rsplit_once(" b/")
        .map(|(_, p)| p.to_owned())
        .unwrap_or_else(|| rest.to_owned())
}

// ---------------------------------------------------------------------------------------------
// RPC backing (212 §2, §6)

/// The canonical seat cwd and the repository root containing it: where rule (b) allows reads.
/// Unseated sessions get none (never the engine's own working directory).
fn allowed_dirs(session: SessionId) -> Vec<PathBuf> {
    let Some(cwd) = tools::seat::seat_of(session).cwd else {
        return Vec::new();
    };
    [std::fs::canonicalize(&cwd).ok(), repo_root(&cwd)]
        .into_iter()
        .flatten()
        .collect()
}

fn inside(dirs: &[PathBuf], path: &Path) -> bool {
    std::fs::canonicalize(path).is_ok_and(|p| dirs.iter().any(|d| p.starts_with(d)))
}

/// May `session` read `path` (212 §2): never a cell record; a file this engine showed for the
/// session, or one inside its working directory or repository. The path to read, or
/// `not a shown file`.
pub fn authorize_file(session: SessionId, cell_root: &Path, path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if !p.is_absolute() || tools::seat::refuse_cell_record(cell_root, p).is_err() {
        return Err(NOT_SHOWN_FILE.into());
    }
    if was_shown(session, &file_key(path)) || inside(&allowed_dirs(session), p) {
        Ok(p.to_path_buf())
    } else {
        Err(NOT_SHOWN_FILE.into())
    }
}

/// May `session` read the diff `spec` names (212 §2): as [`authorize_file`] for its files (or
/// repository root); `not a shown diff` otherwise.
pub fn authorize_diff(session: SessionId, cell_root: &Path, spec: &DiffSpec) -> Result<(), String> {
    validate_spec(spec)?;
    let sides: Vec<&str> = match spec.mode {
        DiffMode::Files => [&spec.a, &spec.b].into_iter().flatten().map(String::as_str).collect(),
        _ => spec.root.as_deref().into_iter().collect(),
    };
    if sides
        .iter()
        .any(|p| tools::seat::refuse_cell_record(cell_root, Path::new(p)).is_err())
    {
        return Err(NOT_SHOWN_DIFF.into());
    }
    if was_shown(session, &diff_key(spec)) {
        return Ok(());
    }
    let dirs = allowed_dirs(session);
    if sides.iter().all(|p| inside(&dirs, Path::new(p))) {
        Ok(())
    } else {
        Err(NOT_SHOWN_DIFF.into())
    }
}

/// `file_view_read` on the owner: authorize, then read (blocking).
pub fn file_view_read(
    session: SessionId,
    cell_root: &Path,
    path: &str,
    if_mtime_ms: Option<u64>,
) -> Result<FileViewText, String> {
    let p = authorize_file(session, cell_root, path)?;
    read_file_view(&p, if_mtime_ms, MAX_FILE)
}

/// `diff_view_read` on the owner: authorize, then compute (blocking).
pub fn diff_view_read(session: SessionId, cell_root: &Path, spec: &DiffSpec) -> Result<DiffViewText, String> {
    authorize_diff(session, cell_root, spec)?;
    Ok(compute(spec, cell_root)?.into_view(MAX_DIFF))
}

/// Modification time of `meta` in ms since the epoch (0 when unknown).
pub(crate) fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Read up to `cap` bytes of `path` (cut back to a UTF-8 boundary), or `unchanged` when its
/// mtime is `if_mtime_ms`.
fn read_file_view(path: &Path, if_mtime_ms: Option<u64>, cap: u64) -> Result<FileViewText, String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let meta = std::fs::metadata(path).map_err(err)?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let (size, mtime, lang) = (meta.len(), mtime_ms(&meta), lang_of(path));
    if if_mtime_ms == Some(mtime) {
        return Ok(FileViewText {
            size,
            mtime_ms: mtime,
            lang,
            unchanged: true,
            ..FileViewText::default()
        });
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(cap + 1).read_to_end(&mut bytes))
        .map_err(err)?;
    let truncated = bytes.len() as u64 > cap;
    if truncated {
        let mut n = usize::try_from(cap).unwrap_or(usize::MAX);
        // `bytes[n]` is the first byte left out: back off while it continues a character.
        while n > 0 && bytes[n] & 0xC0 == 0x80 {
            n -= 1;
        }
        bytes.truncate(n);
    }
    let (text, lossy) = match String::from_utf8(bytes) {
        Ok(s) => (s, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    Ok(FileViewText {
        lines: count_lines(text.as_bytes()),
        text,
        size,
        mtime_ms: mtime,
        truncated,
        lossy,
        lang,
        unchanged: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionId {
        serde_json::from_value(serde_json::json!("00000000-0000-0000-0000-000000000001")).unwrap()
    }

    /// A temp repo with one commit (`a.txt` = "one\ntwo\n"); signing and hooks off.
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        sh(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        commit(&root, "first");
        (dir, root)
    }

    fn sh(root: &Path, args: &[&str]) -> String {
        let out = git_cmd(root)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn commit(root: &Path, msg: &str) -> String {
        sh(root, &["add", "-A"]);
        sh(root, &["commit", "-q", "-m", msg]);
        sh(root, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn opened(lines: u32) -> OpenedFile {
        OpenedFile {
            path: "/work/src/main.rs".into(),
            lang: "rs".into(),
            size: 120,
            lines,
            line: None,
            end_line: None,
        }
    }

    #[test]
    fn file_receipt_first_two_lines() {
        let id = "0123456789abcdef";
        assert_eq!(
            file_receipt("main.rs", id, &opened(9), true),
            "file view opened · main.rs · tile file:0123456789abcdef\n\
             path: /work/src/main.rs\n\
             9 lines, 120 bytes.\n\
             The tile follows the file as it changes on disk."
        );
        let one = OpenedFile { line: Some(4), ..opened(9) };
        let r = file_receipt("main.rs", id, &one, false);
        assert_eq!(r.lines().nth(1).unwrap(), "path: /work/src/main.rs#L4");
        assert_eq!(r.lines().nth(3).unwrap(), NO_APP);
        let range = OpenedFile { line: Some(4), end_line: Some(7), ..opened(9) };
        let r = file_receipt("main.rs", id, &range, true);
        assert_eq!(r.lines().nth(1).unwrap(), "path: /work/src/main.rs#L4-L7");
    }

    #[test]
    fn diff_receipt_first_two_lines() {
        let spec = DiffSpec {
            mode: DiffMode::Range,
            root: Some("/work".into()),
            a: Some("a".repeat(40)),
            b: Some("b".repeat(40)),
            three_dot: true,
            paths: vec!["src".into()],
        };
        let r = diff_receipt("main...HEAD · src", "0123456789abcdef", &spec, (3, 10, 2), true);
        let lines: Vec<_> = r.lines().collect();
        assert_eq!(lines[0], "diff view opened · main...HEAD · src · tile diff:0123456789abcdef");
        assert_eq!(
            lines[1],
            format!(
                "spec: {{\"mode\":\"range\",\"root\":\"/work\",\"a\":\"{}\",\"b\":\"{}\",\"three_dot\":true,\"paths\":[\"src\"]}}",
                "a".repeat(40),
                "b".repeat(40)
            )
        );
        assert_eq!(lines[2], "3 files changed, +10 \u{2212}2.");
        assert_eq!(lines.len(), 3, "{r}");
        let wt = DiffSpec { mode: DiffMode::Worktree, a: None, b: None, three_dot: false, paths: vec![], ..spec };
        let r = diff_receipt("working tree", "0123456789abcdef", &wt, (0, 0, 0), false);
        assert_eq!(
            r.lines().skip(2).collect::<Vec<_>>(),
            [
                "no changes.",
                NO_APP,
                "It shows the tree as it is now; the user presses Refresh to see later changes."
            ]
        );
    }

    #[test]
    fn event_wire_shapes() {
        let session = session();
        let v = serde_json::to_value(FileViewOpened {
            session,
            call_id: "c".into(),
            id: "0123456789abcdef".into(),
            path: "/w/a.rs".into(),
            title: "a.rs".into(),
            lang: "rs".into(),
            line: Some(3),
            end_line: None,
            size: 10,
            lines: 2,
        })
        .unwrap();
        assert_eq!(v["type"], "FileViewOpened");
        assert_eq!(v["end_line"], serde_json::Value::Null);
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["call_id", "end_line", "id", "lang", "line", "lines", "path", "session", "size", "title", "type"]
        );
        let v = serde_json::to_value(DiffViewOpened {
            session,
            call_id: "c".into(),
            id: "0123456789abcdef".into(),
            title: "staged".into(),
            spec: DiffSpec {
                mode: DiffMode::Staged,
                root: Some("/w".into()),
                a: None,
                b: None,
                three_dot: false,
                paths: vec![],
            },
            files: 1,
            added: 2,
            removed: 3,
        })
        .unwrap();
        assert_eq!(v["type"], "DiffViewOpened");
        assert_eq!(v["spec"]["mode"], "staged");
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["added", "call_id", "files", "id", "removed", "session", "spec", "title", "type"]
        );
        // Same file, same tile; another session, another tile.
        assert_eq!(tile_id(session, "/w/a.rs"), tile_id(session, "/w/a.rs"));
        assert_ne!(tile_id(session, "/w/a.rs"), tile_id(SessionId::new(), "/w/a.rs"));
        assert_eq!(tile_id(session, "/w/a.rs").len(), 16);
    }

    #[test]
    fn file_resolution_and_refusals() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(dir.path()).unwrap();
        let cells = tempfile::tempdir().unwrap();
        let cell_root = cells.path();
        std::fs::write(cwd.join("main.rs"), "fn main() {}\n\nlast").unwrap();
        let args = |p: &str, line: Option<u32>, end: Option<u32>| ShowFileArgs {
            path: p.into(),
            line,
            end_line: end,
            title: None,
        };
        let f = open_file(&args("main.rs", Some(2), Some(3)), &cwd, cell_root).unwrap();
        assert_eq!(f.path, cwd.join("main.rs").to_string_lossy());
        assert_eq!((f.lines, f.size, f.lang.as_str()), (3, 18, "rs"));
        let err = |a: ShowFileArgs| open_file(&a, &cwd, cell_root).unwrap_err();
        assert!(err(args("main.rs", Some(4), None)).ends_with("has 3 lines; line 4 is past the end"));
        assert_eq!(err(args("main.rs", None, Some(2))), "`end_line` needs `line`");
        assert_eq!(err(args("main.rs", Some(3), Some(2))), "`end_line` 2 is before `line` 3");
        assert_eq!(err(args("main.rs", Some(0), None)), "lines are 1-based");
        std::fs::write(cwd.join("blob.dat"), b"abc\0def").unwrap();
        assert!(err(args("blob.dat", None, None)).ends_with("is binary — use show for images and documents"));
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        std::fs::write(cwd.join("pic"), png).unwrap();
        assert!(err(args("pic", None, None)).contains("is binary"));
        std::fs::write(cwd.join("big.txt"), vec![b'a'; MAX_FILE as usize + 1]).unwrap();
        assert!(err(args("big.txt", None, None)).contains("at most 2097152 bytes"));
        assert!(err(args(".", None, None)).ends_with("is not a regular file"));
        assert!(err(args("nope.rs", None, None)).contains("nope.rs"));
        // A cell record under the session's root is refused.
        std::fs::create_dir_all(cell_root.join("cells")).unwrap();
        let record = cell_root.join("cells/run.json");
        std::fs::write(&record, "{}").unwrap();
        assert!(err(args(record.to_str().unwrap(), None, None)).contains(record.to_str().unwrap()));
    }

    #[test]
    fn langs() {
        assert_eq!(lang_of(Path::new("/a/Main.RS")), "rs");
        assert_eq!(lang_of(Path::new("/a/Dockerfile")), "Dockerfile");
        assert_eq!(lang_of(Path::new("/a/justfile")), "justfile");
        assert_eq!(lang_of(Path::new("/a/CMakeLists.txt")), "CMakeLists.txt");
        assert_eq!(lang_of(Path::new("/a/README")), "");
    }

    #[test]
    fn worktree_includes_untracked_and_keeps_the_index() {
        let (_d, root) = repo();
        std::fs::write(root.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/new.rs"), "x\ny").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();
        std::fs::write(root.join("ignored.log"), "junk\n").unwrap();
        let cells = tempfile::tempdir().unwrap();
        let (spec, title) = plan_diff(&ShowDiffArgs::default(), &root, cells.path()).unwrap();
        assert_eq!(title, "working tree");
        assert_eq!(spec.mode, DiffMode::Worktree);
        assert_eq!(spec.root.as_deref(), Some(root.to_str().unwrap()));
        let d = compute(&spec, cells.path()).unwrap();
        let by: HashMap<_, _> = d.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert_eq!(by.len(), 3, "{:?}", d.files);
        assert_eq!((by["a.txt"].status.as_str(), by["a.txt"].added, by["a.txt"].removed), ("M", 2, 1));
        assert_eq!((by["sub/new.rs"].status.as_str(), by["sub/new.rs"].added), ("?", 2));
        assert_eq!(by[".gitignore"].status, "?");
        assert!(d.text.contains("+++ b/sub/new.rs\n@@ -0,0 +1,2 @@\n+x\n+y\n\\ No newline at end of file\n"), "{}", d.text);
        assert!(!d.text.contains("b/ignored.log"), "{}", d.text);
        assert_eq!(d.totals(), (3, 5, 1));
        // The index is untouched: nothing staged.
        assert_eq!(sh(&root, &["diff", "--cached", "--name-only"]), "");
        // `paths` from a subdirectory cwd are root-relative.
        let sub = root.join("sub");
        let args = ShowDiffArgs { paths: Some(vec!["new.rs".into()]), ..ShowDiffArgs::default() };
        let (spec, title) = plan_diff(&args, &sub, cells.path()).unwrap();
        assert_eq!(spec.paths, ["sub/new.rs"]);
        assert_eq!(title, "working tree · new.rs");
        let d = compute(&spec, cells.path()).unwrap();
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path, "sub/new.rs");
    }

    #[test]
    fn staged_commit_and_range() {
        let (_d, root) = repo();
        let first = sh(&root, &["rev-parse", "HEAD"]).trim().to_owned();
        let cells = tempfile::tempdir().unwrap();
        let plan = |a: ShowDiffArgs| plan_diff(&a, &root, cells.path());
        // Root commit: against the empty tree.
        let (spec, title) = plan(ShowDiffArgs { commit: Some("HEAD".into()), ..Default::default() }).unwrap();
        assert_eq!(spec.a.as_deref(), Some(first.as_str()));
        assert_eq!(title, format!("{} first", &first[..7]));
        let d = compute(&spec, cells.path()).unwrap();
        assert_eq!(d.files, [DiffFileStat { path: "a.txt".into(), status: "A".into(), added: 2, removed: 0 }]);
        std::fs::write(root.join("a.txt"), "one\n-- dashes\n").unwrap();
        let second = commit(&root, &"long subject ".repeat(10));
        let (spec, title) = plan(ShowDiffArgs { commit: Some(second[..10].into()), ..Default::default() }).unwrap();
        assert_eq!(title.chars().count(), 8 + MAX_SUBJECT, "{title}");
        assert!(title.ends_with('…'));
        let d = compute(&spec, cells.path()).unwrap();
        // The removed `two` and the added `-- dashes` (a hunk line that looks like a header).
        assert_eq!(d.files, [DiffFileStat { path: "a.txt".into(), status: "M".into(), added: 1, removed: 1 }], "{}", d.text);
        // Range, two and three dots; resolved now.
        let (spec, title) = plan(ShowDiffArgs { range: Some(format!("{first}..")), ..Default::default() }).unwrap();
        assert_eq!(title, format!("{first}.."));
        assert_eq!((spec.a.as_deref(), spec.b.as_deref(), spec.three_dot), (Some(first.as_str()), Some(second.as_str()), false));
        let three = plan(ShowDiffArgs { range: Some("HEAD~1...HEAD".into()), ..Default::default() }).unwrap().0;
        assert!(three.three_dot);
        assert_eq!(compute(&three, cells.path()).unwrap().totals(), compute(&spec, cells.path()).unwrap().totals());
        // Staged: only what is in the index.
        std::fs::write(root.join("a.txt"), "staged\n").unwrap();
        sh(&root, &["add", "a.txt"]);
        std::fs::write(root.join("a.txt"), "staged\nunstaged\n").unwrap();
        let (spec, title) = plan(ShowDiffArgs { staged: Some(true), ..Default::default() }).unwrap();
        assert_eq!(title, "staged");
        let d = compute(&spec, cells.path()).unwrap();
        assert!(d.text.contains("+staged") && !d.text.contains("+unstaged"), "{}", d.text);
        // An empty diff still computes.
        let (spec, _) = plan(ShowDiffArgs { range: Some("HEAD..HEAD".into()), ..Default::default() }).unwrap();
        assert_eq!(compute(&spec, cells.path()).unwrap().totals(), (0, 0, 0));
    }

    #[test]
    fn refusals_and_unknown_revisions() {
        let (_d, root) = repo();
        let cells = tempfile::tempdir().unwrap();
        let plan = |a: ShowDiffArgs| plan_diff(&a, &root, cells.path());
        assert_eq!(
            plan(ShowDiffArgs { commit: Some("nope".into()), ..Default::default() }).unwrap_err(),
            "unknown revision `nope`"
        );
        assert_eq!(
            plan(ShowDiffArgs { commit: Some("--output=/tmp/x".into()), ..Default::default() }).unwrap_err(),
            "unknown revision `--output=/tmp/x`"
        );
        assert!(plan(ShowDiffArgs { range: Some("main..nope".into()), ..Default::default() }).unwrap_err().contains("unknown revision `nope`"));
        assert!(plan(ShowDiffArgs { range: Some("main".into()), ..Default::default() }).unwrap_err().contains("not A..B"));
        assert!(plan(ShowDiffArgs { commit: Some("HEAD".into()), staged: Some(true), ..Default::default() }).unwrap_err().starts_with("give one of"));
        assert_eq!(plan(ShowDiffArgs { file_a: Some("a.txt".into()), ..Default::default() }).unwrap_err(), "file_a and file_b go together");
        let outside = tempfile::tempdir().unwrap();
        let err = plan_diff(&ShowDiffArgs::default(), outside.path(), cells.path()).unwrap_err();
        assert!(err.ends_with("is not in a git repository"), "{err}");
        // A tampered spec never reaches git as an option.
        let bad = DiffSpec { mode: DiffMode::Commit, root: Some(root.to_string_lossy().into()), a: Some("--output=/tmp/x".into()), b: None, three_dot: false, paths: vec![] };
        assert_eq!(compute(&bad, cells.path()).unwrap_err(), "invalid diff spec");
    }

    #[test]
    fn two_files() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(cwd.join("old.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(cwd.join("new.txt"), "a\nB\nc\nd\n").unwrap();
        let cells = tempfile::tempdir().unwrap();
        let args = ShowDiffArgs { file_a: Some("old.txt".into()), file_b: Some("new.txt".into()), ..Default::default() };
        let (spec, title) = plan_diff(&args, &cwd, cells.path()).unwrap();
        assert_eq!(title, "old.txt \u{2194} new.txt");
        assert_eq!((spec.mode, spec.root.as_ref()), (DiffMode::Files, None));
        let d = compute(&spec, cells.path()).unwrap();
        let a = spec.a.unwrap();
        assert!(d.text.starts_with(&format!("--- a/{}\n+++ b/", a.trim_start_matches('/'))), "{}", d.text);
        assert_eq!(d.files, [DiffFileStat { path: spec.b.unwrap(), status: "M".into(), added: 2, removed: 1 }]);
        let args = ShowDiffArgs { paths: Some(vec!["x".into()]), ..args };
        assert!(plan_diff(&args, &cwd, cells.path()).unwrap_err().contains("`paths`"));
    }

    #[test]
    fn stats_walk_hunks_by_count() {
        let patch = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1,2 +1,1 @@\n--- not a header\n keep\ndiff --git a/old name b/new name\nsimilarity index 90%\nrename from old name\nrename to new name\ndiff --git a/gone b/gone\ndeleted file mode 100644\n--- a/gone\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";
        let s = file_stats(patch);
        let got: Vec<_> = s.iter().map(|f| (f.path.as_str(), f.status.as_str(), f.added, f.removed)).collect();
        assert_eq!(got, [("x", "M", 0, 1), ("new name", "R", 0, 0), ("gone", "D", 0, 1), ("img.png", "M", 0, 0)]);
    }

    #[test]
    fn read_authorization() {
        let (_d, root) = repo();
        let cells = tempfile::tempdir().unwrap();
        let s = SessionId::new();
        let file = root.join("a.txt").to_string_lossy().into_owned();
        let outside = tempfile::tempdir().unwrap();
        let far = std::fs::canonicalize(outside.path()).unwrap().join("x.rs");
        std::fs::write(&far, "x\n").unwrap();
        let far = far.to_string_lossy().into_owned();
        // Unseated, nothing shown: refused.
        assert_eq!(file_view_read(s, cells.path(), &file, None).unwrap_err(), NOT_SHOWN_FILE);
        // Inside the seat's repository (cwd a subdirectory): allowed.
        std::fs::create_dir(root.join("sub")).unwrap();
        tools::seat::set_cwd(s, Some(root.join("sub")));
        assert_eq!(file_view_read(s, cells.path(), &file, None).unwrap().text, "one\ntwo\n");
        assert_eq!(file_view_read(s, cells.path(), &far, None).unwrap_err(), NOT_SHOWN_FILE);
        assert_eq!(file_view_read(s, cells.path(), "a.txt", None).unwrap_err(), NOT_SHOWN_FILE);
        // Shown by this session: allowed anywhere.
        remember(s, file_key(&far));
        assert_eq!(file_view_read(s, cells.path(), &far, None).unwrap().text, "x\n");
        // Diffs: the seat's repository yes; two files outside only once shown.
        let spec = DiffSpec { mode: DiffMode::Worktree, root: Some(root.to_string_lossy().into()), a: None, b: None, three_dot: false, paths: vec![] };
        assert!(diff_view_read(s, cells.path(), &spec).is_ok());
        let files = DiffSpec { mode: DiffMode::Files, root: None, a: Some(far.clone()), b: Some(file.clone()), three_dot: false, paths: vec![] };
        assert_eq!(diff_view_read(s, cells.path(), &files).unwrap_err(), NOT_SHOWN_DIFF);
        remember(s, diff_key(&files));
        assert_eq!(diff_view_read(s, cells.path(), &files).unwrap().added, 2);
        // A cell record is refused even when shown.
        std::fs::create_dir_all(cells.path().join("cells")).unwrap();
        let record = std::fs::canonicalize(cells.path()).unwrap().join("cells/r.json");
        std::fs::write(&record, "{}").unwrap();
        let record = record.to_string_lossy().into_owned();
        remember(s, file_key(&record));
        assert_eq!(file_view_read(s, cells.path(), &record, None).unwrap_err(), NOT_SHOWN_FILE);
        forget(&[s]);
        assert_eq!(file_view_read(s, cells.path(), &far, None).unwrap_err(), NOT_SHOWN_FILE);
        tools::seat::forget(s);
    }

    #[test]
    fn truncation_and_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.txt");
        // "é" is two bytes: a cap of 3 falls inside the second one.
        std::fs::write(&p, "aéé\nz").unwrap();
        let v = read_file_view(&p, None, 4).unwrap();
        assert_eq!((v.text.as_str(), v.truncated, v.lossy, v.size), ("aé", true, false, 7));
        let v = read_file_view(&p, None, 3).unwrap();
        assert_eq!(v.text, "aé");
        let whole = read_file_view(&p, None, MAX_FILE).unwrap();
        assert_eq!((whole.text.as_str(), whole.lines, whole.truncated, whole.lang.as_str()), ("aéé\nz", 2, false, "txt"));
        let same = read_file_view(&p, Some(whole.mtime_ms), MAX_FILE).unwrap();
        assert!(same.unchanged && same.text.is_empty());
        std::fs::write(&p, [b'a', 0xff, b'\n']).unwrap();
        assert!(read_file_view(&p, None, MAX_FILE).unwrap().lossy);
        let d = Diff { text: "ab€cd".into(), files: vec![] };
        let v = d.into_view(4);
        assert_eq!(v.text, "ab\n[diff truncated at 2 bytes]");
        assert!(v.truncated);
    }
}
