//! 193 TEXT-REFS: the ref grammar (§1), per-session read snapshots (§2),
//! the clip store (§4 `clip`), the [`Syntax`] hook codeindex installs for
//! symbol refs and multi-line string interiors (D6), and the all-or-nothing
//! multi-file write (§5). The tools built on them live in [`crate::text`]
//! and in `edit`'s range mode.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;
use runtime::SessionId;
use similar::{Algorithm, DiffOp};

// ---------------------------------------------------------------- refs (§1)

/// What a ref points at inside a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// `Ln@id` / `Ln-Lm@id`: 1-based inclusive lines of snapshot `snap`.
    Lines {
        /// First line.
        first: usize,
        /// Last line (`== first` for `Ln`).
        last: usize,
        /// The 8-hex snapshot id the numbers came from.
        snap: String,
    },
    /// `name` or `Type.method`, resolved against the current text.
    Symbol(String),
    /// Before the first line.
    Start,
    /// After the last line.
    End,
}

/// A parsed ref.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    /// `path#anchor`.
    At {
        /// The path as written (resolved later like `read_file`'s).
        path: PathBuf,
        /// The part after the last `#`.
        anchor: Anchor,
    },
    /// `clip:name`.
    Clip(String),
}

const REF_FORMS: &str = "expected path#L<n>@<id>, path#L<n>-L<m>@<id>, path#<symbol>, path#start, path#end or clip:<name>";

/// Parse a full ref; `path` is split from the anchor at the last `#`.
pub fn parse_ref(s: &str) -> anyhow::Result<Ref> {
    if let Some(name) = s.strip_prefix("clip:") {
        check_clip_name(name)?;
        return Ok(Ref::Clip(name.to_owned()));
    }
    let Some((path, anchor)) = s.rsplit_once('#') else {
        anyhow::bail!("malformed ref `{s}`: no `#`; {REF_FORMS}");
    };
    anyhow::ensure!(
        !path.is_empty(),
        "malformed ref `{s}`: empty path; {REF_FORMS}"
    );
    let anchor = parse_anchor(anchor).map_err(|e| anyhow::anyhow!("ref `{s}`: {e}"))?;
    Ok(Ref::At {
        path: PathBuf::from(path),
        anchor,
    })
}

/// Parse an anchor without `path#` (`edit.range`, `indent.range`/`match`).
pub fn parse_anchor(s: &str) -> anyhow::Result<Anchor> {
    match s {
        "start" => return Ok(Anchor::Start),
        "end" => return Ok(Anchor::End),
        _ => {}
    }
    if is_line_anchor(s) {
        return parse_lines(s);
    }
    let ident = |p: &str| {
        let mut c = p.chars();
        c.next()
            .is_some_and(|f| f.is_ascii_alphabetic() || f == '_' || f == '$')
            && c.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    };
    anyhow::ensure!(
        !s.is_empty() && s.split('.').all(ident),
        "malformed anchor `{s}`: expected L<n>@<id>, L<n>-L<m>@<id>, a symbol (`name`, `Type.method`), start or end"
    );
    Ok(Anchor::Symbol(s.to_owned()))
}

/// `^L[0-9]+(-L[0-9]+)?(@[0-9a-f]{8})?$`: anything else is a symbol (so
/// `L2Cache` names a symbol).
fn is_line_anchor(s: &str) -> bool {
    let (lines, snap) = match s.split_once('@') {
        Some((l, id)) => (l, Some(id)),
        None => (s, None),
    };
    let num = |t: &str| {
        t.strip_prefix('L')
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
    };
    let lines_ok = match lines.split_once('-') {
        Some((a, b)) => num(a) && num(b),
        None => num(lines),
    };
    lines_ok
        && snap.is_none_or(|id| {
            id.len() == 8
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
}

fn parse_lines(s: &str) -> anyhow::Result<Anchor> {
    let bad = || {
        anyhow::anyhow!(
            "malformed line anchor `{s}`: expected L<n>@<id> or L<n>-L<m>@<id> (id: 8 hex chars from a read_file header or a tool receipt)"
        )
    };
    let Some((lines, snap)) = s.split_once('@') else {
        anyhow::bail!(
            "line anchor `{s}` needs the snapshot it came from: L<n>@<id>, the id from the read_file header"
        );
    };
    anyhow::ensure!(
        snap.len() == 8
            && snap
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        bad()
    );
    let num = |t: &str| -> anyhow::Result<usize> {
        let t = t.strip_prefix('L').ok_or_else(bad)?;
        anyhow::ensure!(
            !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()),
            bad()
        );
        t.parse().map_err(|_| bad())
    };
    let (first, last) = match lines.split_once('-') {
        Some((a, b)) => (num(a)?, num(b)?),
        None => {
            let n = num(lines)?;
            (n, n)
        }
    };
    anyhow::ensure!(first >= 1, "line anchor `{s}`: lines are 1-based");
    anyhow::ensure!(
        first <= last,
        "line anchor `{s}`: L{first} is after L{last}"
    );
    Ok(Anchor::Lines {
        first,
        last,
        snap: snap.to_owned(),
    })
}

fn check_clip_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-')),
        "clip name `{name}`: 1-64 of A-Z a-z 0-9 _ . -"
    );
    Ok(())
}

// ------------------------------------------------------------ documents

/// UTF-8 byte order mark.
const BOM: &str = "\u{feff}";

/// Files larger than this are not recorded and refuse line refs (§2).
pub const MAX_SNAPSHOT_BYTES: usize = 4 << 20;

/// The snapshot id of a file's bytes: the first 8 hex chars of sha256.
pub fn snapshot_id(bytes: &[u8]) -> String {
    platform::sha256::hex(bytes)[..8].to_owned()
}

/// A text file as lines: LF-normalised, BOM split off; rendered back with
/// its own line ending, BOM and final-newline state.
#[derive(Clone, Debug)]
pub(crate) struct Doc {
    /// Resolved path.
    pub path: PathBuf,
    /// The path as the model wrote it.
    pub shown: String,
    /// Bytes as read; `None`: the file does not exist.
    pub before: Option<Vec<u8>>,
    pub bom: bool,
    pub crlf: bool,
    pub final_nl: bool,
    /// Line contents without terminators.
    pub lines: Vec<String>,
}

impl Doc {
    /// `bytes` (`None`: missing file) as a document; non-UTF-8 is refused.
    pub fn from_bytes(path: PathBuf, shown: &str, bytes: Option<Vec<u8>>) -> anyhow::Result<Self> {
        let raw = match &bytes {
            Some(b) => std::str::from_utf8(b)
                .map_err(|_| anyhow::anyhow!("{shown}: not UTF-8; refs work on UTF-8 text only"))?,
            None => "",
        };
        let (bom, crlf, body) = normalize(raw);
        let (lines, final_nl) = split_lines(&body);
        Ok(Self {
            path,
            shown: shown.to_owned(),
            before: bytes,
            bom,
            crlf,
            final_nl,
            lines,
        })
    }

    /// Read `path`; a missing file is an error unless `create`.
    pub async fn load(path: PathBuf, shown: &str, create: bool) -> anyhow::Result<Self> {
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => Some(b),
            Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => anyhow::bail!("{shown}: {e}"),
        };
        Self::from_bytes(path, shown, bytes)
    }

    /// The snapshot id of the bytes as read (`None` for a missing file).
    pub fn id(&self) -> Option<String> {
        self.before.as_deref().map(snapshot_id)
    }

    /// LF text without BOM: what a snapshot stores.
    pub fn text(&self) -> String {
        join(&self.lines, self.final_nl)
    }

    /// The bytes to write.
    pub fn render(&self) -> Vec<u8> {
        let mut text = self.text();
        if self.crlf {
            text = text.replace('\n', "\r\n");
        }
        if self.bom {
            text.insert_str(0, BOM);
        }
        text.into_bytes()
    }

    /// Too large to snapshot (§2).
    pub fn too_large(&self) -> bool {
        self.before
            .as_ref()
            .is_some_and(|b| b.len() > MAX_SNAPSHOT_BYTES)
    }
}

/// `body` (LF) as lines; a trailing newline opens no empty last line.
pub(crate) fn split_lines(body: &str) -> (Vec<String>, bool) {
    if body.is_empty() {
        return (Vec::new(), false);
    }
    let final_nl = body.ends_with('\n');
    let body = body.strip_suffix('\n').unwrap_or(body);
    (body.split('\n').map(str::to_owned).collect(), final_nl)
}

/// Lines joined with LF, plus one when `final_nl` and there are lines.
pub(crate) fn join(lines: &[String], final_nl: bool) -> String {
    let mut out = lines.join("\n");
    if final_nl && !lines.is_empty() {
        out.push('\n');
    }
    out
}

/// `raw` without its BOM, CRLF → LF when most line breaks are CRLF (the
/// rule `edit` uses): `(bom, crlf, body)`. `body` is what [`Doc::text`]
/// gives back.
pub(crate) fn normalize(raw: &str) -> (bool, bool, std::borrow::Cow<'_, str>) {
    let (bom, body) = match raw.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let n = body.matches("\r\n").count();
    let crlf = n > 0 && n * 2 >= body.matches('\n').count();
    let body = if crlf {
        std::borrow::Cow::Owned(body.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(body)
    };
    (bom, crlf, body)
}

/// The store key for `path`: canonical where it exists.
pub(crate) fn key(path: &Path) -> PathBuf {
    crate::seat::real(path).unwrap_or_else(|| path.to_path_buf())
}

// ------------------------------------------------------- snapshots (§2)

/// Store caps; [`Snapshots::default`] is §2's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Caps {
    pub per_file: usize,
    pub per_session: usize,
    pub per_engine: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            per_file: 8,
            per_session: 32 << 20,
            per_engine: 256 << 20,
        }
    }
}

struct Snap {
    session: SessionId,
    path: PathBuf,
    id: String,
    text: Arc<str>,
    used: u64,
}

impl Snap {
    /// What a row costs against the byte caps: its text, its key and a
    /// fixed overhead, so many tiny files still count.
    fn cost(&self) -> usize {
        self.text.len() + self.path.as_os_str().len() + ROW_OVERHEAD
    }
}

/// Bytes a snapshot row is charged beyond its text and path.
const ROW_OVERHEAD: usize = 64;

/// What each read and write returned, per (session, canonical path, id).
// ponytail: one Vec scanned linearly for lookups and LRU eviction, under the
// lock; at 64 bytes of overhead per row the 256 MiB cap still allows ~4M
// tiny rows. Index by key (HashMap + LRU list) if it ever shows in a profile.
#[derive(Default)]
pub(crate) struct Snapshots {
    caps: Caps,
    all: Vec<Snap>,
    tick: u64,
}

impl Snapshots {
    #[cfg(test)]
    pub fn with_caps(caps: Caps) -> Self {
        Self {
            caps,
            ..Self::default()
        }
    }

    /// Record `text` (LF, no BOM) as snapshot `id` of `path` for `session`.
    /// Everything is built by the caller, outside the lock.
    pub fn record(&mut self, session: SessionId, path: PathBuf, id: String, text: Arc<str>) {
        if text.len() > MAX_SNAPSHOT_BYTES {
            return;
        }
        self.tick += 1;
        let tick = self.tick;
        if let Some(s) = self
            .all
            .iter_mut()
            .find(|s| s.session == session && s.id == id && s.path == path)
        {
            s.used = tick;
            return;
        }
        self.all.push(Snap {
            session,
            path,
            id,
            text,
            used: tick,
        });
        let caps = self.caps;
        let path = self.all[self.all.len() - 1].path.clone();
        self.evict(
            |s| s.session == session && s.path == path,
            |n, _| n > caps.per_file,
        );
        self.evict(|s| s.session == session, |_, b| b > caps.per_session);
        self.evict(|_| true, |_, b| b > caps.per_engine);
    }

    /// Drop the least recently used rows matching `scope` while `over(rows, bytes)`.
    fn evict(&mut self, scope: impl Fn(&Snap) -> bool, over: impl Fn(usize, usize) -> bool) {
        loop {
            let (n, bytes) = self
                .all
                .iter()
                .filter(|s| scope(s))
                .fold((0, 0), |(n, b), s| (n + 1, b + s.cost()));
            if !over(n, bytes) {
                return;
            }
            let Some(i) = (0..self.all.len())
                .filter(|&i| scope(&self.all[i]))
                .min_by_key(|&i| self.all[i].used)
            else {
                return;
            };
            self.all.swap_remove(i);
        }
    }

    /// Snapshot `id` of `path` for `session`, marked used.
    pub fn get(&mut self, session: SessionId, path: &Path, id: &str) -> Option<Arc<str>> {
        self.tick += 1;
        let tick = self.tick;
        let s = self
            .all
            .iter_mut()
            .find(|s| s.session == session && s.id == id && s.path == path)?;
        s.used = tick;
        Some(Arc::clone(&s.text))
    }
}

static SNAPSHOTS: LazyLock<Mutex<Snapshots>> = LazyLock::new(Mutex::default);

/// Record `text` for (`session`, `path`, `id`); the key (canonicalize
/// syscalls) and the `Arc` are built before the global lock is taken.
fn record(session: SessionId, path: &Path, id: &str, text: &str) {
    if text.len() > MAX_SNAPSHOT_BYTES {
        return;
    }
    let (k, text): (PathBuf, Arc<str>) = (key(path), text.into());
    SNAPSHOTS.lock().record(session, k, id.to_owned(), text);
}

/// Snapshot `id` of `path` for `session`; the key is built outside the lock.
fn lookup(session: SessionId, path: &Path, id: &str) -> Option<Arc<str>> {
    let k = key(path);
    SNAPSHOTS.lock().get(session, &k, id)
}

/// Record `doc`'s current lines as snapshot `id` for `session`; the id to
/// report, or `None` when the file is too large to record.
pub(crate) fn record_doc(session: SessionId, doc: &Doc, bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let id = snapshot_id(bytes);
    record(session, &doc.path, &id, &doc.text());
    Some(id)
}

/// Record raw file `bytes` read or written at `path`: the id, or `None` when
/// they are not UTF-8 or over 4 MiB.
pub(crate) fn record_bytes(session: SessionId, path: &Path, bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    let (_, _, body) = normalize(std::str::from_utf8(bytes).ok()?);
    let id = snapshot_id(bytes);
    record(session, path, &id, &body);
    Some(id)
}

/// D9 for `edit`/`write_file`: record `bytes` just written to `path` and
/// append `<shown> @<id>` to the answer (`structured.snapshot` too). Files
/// over 4 MiB or not UTF-8 get no line and a null snapshot.
pub(crate) fn stamp(
    out: &mut crate::ToolOutput,
    session: SessionId,
    path: &Path,
    shown: &str,
    bytes: &[u8],
) {
    let id = record_bytes(session, path, bytes);
    if let Some(id) = &id {
        if !out.content.ends_with('\n') {
            out.content.push('\n');
        }
        out.content.push_str(&format!("{shown} @{id}"));
    }
    if let Some(serde_json::Value::Object(m)) = out.structured.as_mut() {
        m.insert("snapshot".into(), id.into());
    }
}

/// Lines shown with a refusal (§2 step 3).
const CONTEXT_LINES: usize = 40;
/// Longest line shown in a refusal or a receipt seam.
const SHOWN_LINE_CHARS: usize = 200;

/// `line` cut at [`SHOWN_LINE_CHARS`] chars with ` …[+K chars]`, as
/// `read_file` cuts long lines.
pub(crate) fn cut_line(line: &str) -> std::borrow::Cow<'_, str> {
    match line.char_indices().nth(SHOWN_LINE_CHARS) {
        Some((at, _)) => format!("{} …[+{} chars]", &line[..at], line[at..].chars().count()).into(),
        None => line.into(),
    }
}

/// Line diffs of one call, per snapshot id: the current text is fixed for
/// the call, so several refs into one snapshot diff once.
#[derive(Default)]
pub(crate) struct Diffs(HashMap<String, Arc<Vec<DiffOp>>>);

/// Lines of an LF text, as [`split_lines`] counts them.
fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.matches('\n').count() + usize::from(!text.ends_with('\n'))
    }
}

/// `first..=last` of snapshot `snap` mapped onto `doc` as it is now (§2):
/// 1-based inclusive lines of `doc`. The diff runs off the async threads.
pub(crate) async fn map_lines(
    session: SessionId,
    doc: &Doc,
    first: usize,
    last: usize,
    snap: &str,
    diffs: &mut Diffs,
) -> anyhow::Result<(usize, usize)> {
    let shown = &doc.shown;
    anyhow::ensure!(
        !doc.too_large(),
        "{shown} is over 4 MiB: it has no snapshots, so line refs into it are refused; use a symbol ref, or edit with old/new"
    );
    let Some(cur) = doc.id() else {
        anyhow::bail!("{shown} does not exist");
    };
    let Some(old) = lookup(session, &doc.path, snap) else {
        anyhow::bail!(
            "unknown snapshot @{snap} for {shown} (engine restarted or snapshot evicted): read the file again"
        );
    };
    let n = doc.lines.len();
    let range = |a: usize, b: usize| {
        if a == b {
            format!("line {a}")
        } else {
            format!("lines {a}-{b}")
        }
    };
    if cur == snap {
        anyhow::ensure!(
            last <= n,
            "{shown}: {} past the end of the file ({n} lines) @{cur}",
            range(first, last)
        );
        return Ok((first, last));
    }
    let old_n = count_lines(&old);
    anyhow::ensure!(
        last <= old_n,
        "{shown}: {} past the end of snapshot @{snap} ({old_n} lines)",
        range(first, last),
    );
    let ops = match diffs.0.get(snap) {
        Some(ops) => Arc::clone(ops),
        None => {
            let new_lines = doc.lines.clone();
            let ops = tokio::task::spawn_blocking(move || {
                let (old_lines, _) = split_lines(&old);
                let old: Vec<&str> = old_lines.iter().map(String::as_str).collect();
                let new: Vec<&str> = new_lines.iter().map(String::as_str).collect();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                similar::capture_diff_slices_deadline(Algorithm::Myers, &old, &new, Some(deadline))
            })
            .await?;
            let ops = Arc::new(ops);
            diffs.0.insert(snap.to_owned(), Arc::clone(&ops));
            ops
        }
    };
    // Each old line → (its new line if Equal, the new position of its op).
    let map = |i: usize| -> (Option<usize>, usize, usize) {
        for op in ops.iter() {
            let (o, ol, nw, nl) = match *op {
                DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                } => (old_index, len, new_index, len),
                DiffOp::Delete {
                    old_index,
                    old_len,
                    new_index,
                } => (old_index, old_len, new_index, 0),
                DiffOp::Insert { .. } => continue,
                DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => (old_index, old_len, new_index, new_len),
            };
            if (o..o + ol).contains(&i) {
                let exact = matches!(op, DiffOp::Equal { .. }).then(|| nw + (i - o));
                return (exact, nw, nw + nl);
            }
        }
        (None, n, n)
    };
    let mapped: Vec<_> = (first - 1..last).map(map).collect();
    let exact: Option<Vec<usize>> = mapped.iter().map(|m| m.0).collect();
    if let Some(ex) = exact
        && ex.windows(2).all(|w| w[1] == w[0] + 1)
    {
        return Ok((ex[0] + 1, ex[ex.len() - 1] + 1));
    }
    // Refused: the current text of the region the lines map to, under an id
    // the model can use straight away.
    record(session, &doc.path, &cur, &doc.text());
    let a = mapped.iter().map(|m| m.0.unwrap_or(m.1)).min().unwrap_or(0);
    let b = mapped
        .iter()
        .map(|m| m.0.map_or(m.2, |x| x + 1))
        .max()
        .unwrap_or(0)
        .max(a + 1)
        .min(n);
    Err(anyhow::anyhow!(
        "{} of {shown} changed since @{snap}; {shown} is now @{cur}:\n{}",
        range(first, last),
        numbered(&doc.lines, a + 1, b)
    ))
}

/// `lines[a-1..b]` as `N<tab>text`, at most [`CONTEXT_LINES`] of them, each
/// cut at [`SHOWN_LINE_CHARS`].
pub(crate) fn numbered(lines: &[String], a: usize, b: usize) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if a == 0 || a > b || a > lines.len() {
        out.push_str("(no lines there now)");
        return out;
    }
    let b = b.min(lines.len());
    let shown_end = b.min(a + CONTEXT_LINES - 1);
    let width = shown_end.to_string().len();
    for (i, l) in lines[a - 1..shown_end].iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let _ = write!(out, "{:>width$}\t{}", a + i, cut_line(l));
    }
    if shown_end < b {
        let _ = write!(out, "\n[{} more lines]", b - shown_end);
    }
    out
}

// ------------------------------------------------------- clips (§4 clip)

/// Most bytes one clip holds.
pub(crate) const CLIP_MAX: usize = 1 << 20;
/// Most clips per session.
pub(crate) const CLIP_COUNT: usize = 64;
/// Most clip bytes per session.
pub(crate) const CLIP_SESSION: usize = 16 << 20;
/// Most clip bytes in the engine; the least recently used clip of any
/// session goes first.
pub(crate) const CLIP_ENGINE: usize = 128 << 20;

/// A stored block.
#[derive(Clone, Debug)]
pub(crate) struct Clip {
    pub lines: Vec<String>,
    /// The last line ended with a newline in its file.
    pub final_nl: bool,
    /// Per line: inside a multi-line string (left alone by re-indent);
    /// `None` when the source had no parser.
    pub strings: Option<Vec<bool>>,
}

impl Clip {
    pub(crate) fn bytes(&self) -> usize {
        self.lines.iter().map(|l| l.len() + 1).sum()
    }
}

/// One stored clip.
struct ClipEntry {
    clip: Arc<Clip>,
    /// Use tick for the engine-wide LRU.
    used: u64,
    /// Bytes, measured before the lock: nothing walks lines under it.
    size: usize,
}

/// Every session's clips.
#[derive(Default)]
pub(crate) struct Clips {
    engine_cap: usize,
    all: HashMap<SessionId, BTreeMap<String, ClipEntry>>,
    tick: u64,
}

impl Clips {
    pub(crate) fn with_engine_cap(engine_cap: usize) -> Self {
        Self {
            engine_cap,
            ..Self::default()
        }
    }

    /// Store `clip` (built and measured by the caller) under `name`.
    pub(crate) fn put(
        &mut self,
        session: SessionId,
        name: String,
        clip: Arc<Clip>,
        size: usize,
    ) -> anyhow::Result<()> {
        self.tick += 1;
        let tick = self.tick;
        let mine = self.all.entry(session).or_default();
        let others: usize = mine
            .iter()
            .filter(|(n, _)| **n != name)
            .map(|(_, e)| e.size)
            .sum();
        anyhow::ensure!(
            mine.contains_key(&name) || mine.len() < CLIP_COUNT,
            "this session already has {CLIP_COUNT} clips; reuse a name to replace one"
        );
        anyhow::ensure!(
            others + size <= CLIP_SESSION,
            "clips would hold {} bytes in this session; the limit is 16 MiB, reuse a name to replace one",
            others + size
        );
        mine.insert(
            name,
            ClipEntry {
                clip,
                used: tick,
                size,
            },
        );
        // ponytail: a linear scan over every clip per eviction; at most
        // 64 per session, fine until there are thousands of sessions.
        loop {
            let total: usize = self
                .all
                .values()
                .flat_map(|m| m.values())
                .map(|e| e.size)
                .sum();
            if total <= self.engine_cap {
                break;
            }
            let Some((s, n)) = self
                .all
                .iter()
                .flat_map(|(s, m)| m.iter().map(move |(n, e)| (e.used, *s, n.clone())))
                .filter(|(u, _, _)| *u != tick)
                .min_by_key(|(u, _, _)| *u)
                .map(|(_, s, n)| (s, n))
            else {
                break;
            };
            if let Some(m) = self.all.get_mut(&s) {
                m.remove(&n);
            }
        }
        Ok(())
    }

    /// Clip `name` of `session`, marked used; `Err` lists the names that exist.
    pub(crate) fn get(&mut self, session: SessionId, name: &str) -> Result<Arc<Clip>, Vec<String>> {
        self.tick += 1;
        let tick = self.tick;
        let mine = self.all.get_mut(&session);
        match mine {
            Some(m) => match m.get_mut(name) {
                Some(e) => {
                    e.used = tick;
                    Ok(Arc::clone(&e.clip))
                }
                None => Err(m.keys().cloned().collect()),
            },
            None => Err(Vec::new()),
        }
    }

    pub(crate) fn forget(&mut self, session: SessionId) {
        self.all.remove(&session);
    }
}

static CLIPS: LazyLock<Mutex<Clips>> =
    LazyLock::new(|| Mutex::new(Clips::with_engine_cap(CLIP_ENGINE)));

/// Store `clip` under `name` for `session`, replacing one of that name.
pub(crate) fn put_clip(session: SessionId, name: &str, clip: Clip) -> anyhow::Result<()> {
    let size = clip.bytes();
    anyhow::ensure!(
        size <= CLIP_MAX,
        "clip `{name}` would hold {size} bytes; one clip holds at most 1 MiB"
    );
    let (name, clip) = (name.to_owned(), Arc::new(clip));
    CLIPS.lock().put(session, name, clip, size)
}

/// Clip `name` of `session`; unknown names list the clips that exist.
pub(crate) fn get_clip(session: SessionId, name: &str) -> anyhow::Result<Arc<Clip>> {
    let got = CLIPS.lock().get(session, name);
    match got {
        Ok(c) => Ok(c),
        Err(names) if names.is_empty() => anyhow::bail!(
            "no clip `{name}`: this session has no clips; make one with clip(name, src)"
        ),
        Err(names) => anyhow::bail!("no clip `{name}`; clips: {}", names.join(", ")),
    }
}

/// Drop `session`'s clips (the session's tree closed).
pub fn forget_clips(session: SessionId) {
    CLIPS.lock().forget(session);
}

// ------------------------------------------------------ Syntax hook (D6)

/// One definition as the [`Syntax`] hook reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolSpan {
    /// Parent chain joined by `.`: `splice`, `Reader.load`.
    pub name: String,
    /// The tags kind (`function`, `method`, `class`, …).
    pub kind: String,
    /// 1-based first line, widened over attached doc comments, attributes
    /// and decorators.
    pub start: usize,
    /// 1-based last line, inclusive.
    pub end: usize,
}

/// What the hook knows about one text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyntaxInfo {
    /// Definitions in source order.
    pub symbols: Vec<SymbolSpan>,
    /// 1-based lines whose leading whitespace lies inside a multi-line
    /// string literal, ascending.
    pub string_lines: Vec<usize>,
    /// The parse had syntax errors: spans may be approximate (the receipt
    /// of a symbol ref says so).
    pub has_errors: bool,
}

/// Parses source text for symbol refs and re-indent protection. Installed
/// once by the codeindex plugin ([`set_syntax`]); `tools` must not depend on
/// `codeindex` (the [`crate::fs::Outliner`] precedent).
pub trait Syntax: Send + Sync {
    /// `text` (LF) parsed as `path`'s language; `None` when there is no
    /// parser for it.
    fn analyze(&self, path: &Path, text: &str) -> Option<SyntaxInfo>;
}

static SYNTAX: OnceLock<Arc<dyn Syntax>> = OnceLock::new();

/// Install the process-wide [`Syntax`] hook; `false` if one is already set.
pub fn set_syntax(s: Arc<dyn Syntax>) -> bool {
    SYNTAX.set(s).is_ok()
}

/// The installed [`Syntax`] hook, if any.
pub fn syntax() -> Option<Arc<dyn Syntax>> {
    SYNTAX.get().cloned()
}

/// Run the installed hook on `doc` off the async threads: `Ok(None)` when no
/// hook is installed or the language has no parser, `Err` when the hook
/// panicked (its own refusal, not "needs a parser").
pub(crate) async fn analyze(doc: &Doc) -> Result<Option<SyntaxInfo>, String> {
    let Some(hook) = syntax() else {
        return Ok(None);
    };
    let (path, text) = (doc.path.clone(), doc.text());
    tokio::task::spawn_blocking(move || hook.analyze(&path, &text))
        .await
        .map_err(|e| format!("the syntax parser failed on {}: {e}", doc.shown))
}

/// Resolve symbol `name` in `info` (from `shown`): an exact qualified match,
/// else the one symbol whose qualified name ends with `.name`. Ambiguity
/// lists the candidates. 1-based inclusive lines.
pub fn find_symbol(
    info: Option<&SyntaxInfo>,
    shown: &str,
    name: &str,
) -> anyhow::Result<(usize, usize)> {
    let Some(info) = info else {
        anyhow::bail!(
            "symbol refs need a parser for {shown}'s language (codeindex: rust, typescript, javascript, python, go); use a line ref"
        );
    };
    let suffix = format!(".{name}");
    let exact: Vec<&SymbolSpan> = info.symbols.iter().filter(|s| s.name == name).collect();
    let hits = if exact.is_empty() {
        info.symbols
            .iter()
            .filter(|s| s.name.ends_with(&suffix))
            .collect()
    } else {
        exact
    };
    match hits.as_slice() {
        [one] => Ok((one.start, one.end)),
        [] => anyhow::bail!("no symbol `{name}` in {shown}; code_outline lists its symbols"),
        many => {
            let list: Vec<String> = many
                .iter()
                .map(|s| format!("{} ({}, lines {}-{})", s.name, s.kind, s.start, s.end))
                .collect();
            // Same qualified name twice (two impls of one method, Go methods
            // without a receiver): qualifying cannot help.
            let advice = if many.iter().all(|s| s.name == many[0].name) {
                "use a line ref"
            } else {
                "qualify it (Type.method) or use a line ref"
            };
            anyhow::bail!(
                "`{name}` is ambiguous in {shown}: {}; {advice}",
                list.join("; ")
            )
        }
    }
}

// ----------------------------------------------------------- writes (§5)

/// One file of an all-or-nothing write.
pub(crate) struct Planned {
    pub path: PathBuf,
    pub shown: String,
    /// What the call resolved against; `None`: the file did not exist.
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
}

/// Write every file or none (§5): each must still hold what the call
/// resolved against; a failed write puts the earlier files back.
pub(crate) async fn commit(files: &[Planned]) -> anyhow::Result<()> {
    for f in files {
        let now = match tokio::fs::read(&f.path).await {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => anyhow::bail!("{}: {e}; nothing written", f.shown),
        };
        anyhow::ensure!(
            now == f.before,
            "{} changed on disk while this call ran; nothing written: read it again and retry",
            f.shown
        );
    }
    for (i, f) in files.iter().enumerate() {
        let res = async {
            if f.before.is_none()
                && let Some(parent) = f.path.parent().filter(|p| !p.as_os_str().is_empty())
            {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}: {e}", parent.display()))?;
            }
            crate::edit::write_atomic(&f.path, &f.after).await
        }
        .await;
        let Err(e) = res else { continue };
        let mut restored = Vec::new();
        let mut failed = Vec::new();
        for done in &files[..i] {
            let back = match &done.before {
                Some(b) => crate::edit::write_atomic(&done.path, b).await,
                None => tokio::fs::remove_file(&done.path)
                    .await
                    .map_err(anyhow::Error::from),
            };
            match back {
                Ok(()) => restored.push(done.shown.as_str()),
                Err(e) => failed.push(format!("{} ({e})", done.shown)),
            }
        }
        let mut msg = format!("{}: {e}; nothing written", f.shown);
        if !restored.is_empty() {
            msg = format!(
                "{}: {e}; {} written first and restored, nothing changed",
                f.shown,
                restored.join(", ")
            );
        }
        if !failed.is_empty() {
            msg.push_str(&format!(
                "; RESTORE FAILED for {}: they hold the new content",
                failed.join(", ")
            ));
        }
        anyhow::bail!(msg);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(first: usize, last: usize, snap: &str) -> Anchor {
        Anchor::Lines {
            first,
            last,
            snap: snap.into(),
        }
    }

    #[test]
    fn refs_parse_every_form_and_split_at_the_last_hash() {
        let at = |p: &str, anchor| Ref::At {
            path: p.into(),
            anchor,
        };
        assert_eq!(
            parse_ref("src/a.rs#L5@1a2b3c4d").unwrap(),
            at("src/a.rs", lines(5, 5, "1a2b3c4d"))
        );
        assert_eq!(
            parse_ref("a.rs#L5-L9@00ff00ff").unwrap(),
            at("a.rs", lines(5, 9, "00ff00ff"))
        );
        assert_eq!(
            parse_ref("a#b.py#Reader.load").unwrap(),
            at("a#b.py", Anchor::Symbol("Reader.load".into()))
        );
        assert_eq!(
            parse_ref("x.rs#splice").unwrap(),
            at("x.rs", Anchor::Symbol("splice".into()))
        );
        assert_eq!(parse_ref("x.rs#start").unwrap(), at("x.rs", Anchor::Start));
        assert_eq!(parse_ref("x.rs#end").unwrap(), at("x.rs", Anchor::End));
        assert_eq!(
            parse_ref("clip:a-b.c_9").unwrap(),
            Ref::Clip("a-b.c_9".into())
        );
        assert_eq!(
            parse_anchor("L3@abcdef01").unwrap(),
            lines(3, 3, "abcdef01")
        );
    }

    #[test]
    fn malformed_refs_are_refused_with_the_forms() {
        for bad in [
            "a.rs",
            "#L1@1a2b3c4d",
            "a.rs#",
            "a.rs#L5",
            "a.rs#L0@1a2b3c4d",
            "a.rs#L9-L5@1a2b3c4d",
            "a.rs#L5@1A2B3C4D",
            "a.rs#L5@1a2b3c",
            "a.rs#L5-9@1a2b3c4d",
            "a.rs#Reader..load",
            "a.rs#9lives",
            "a.rs#foo bar",
            "clip:",
            "clip:has space",
        ] {
            assert!(parse_ref(bad).is_err(), "{bad} parsed");
        }
        let e = parse_ref("a.rs#L5").unwrap_err().to_string();
        assert!(e.contains("needs the snapshot"), "{e}");
        assert!(parse_ref(&format!("clip:{}", "x".repeat(65))).is_err());
    }

    /// `Snapshots::record` with the caller-side conversions.
    fn rec(st: &mut Snapshots, s: SessionId, p: &str, id: &str, text: &str) {
        st.record(s, PathBuf::from(p), id.to_owned(), text.into());
    }

    #[test]
    fn snapshots_keep_the_last_8_per_file_and_honour_byte_caps_lru() {
        let (s, t) = (SessionId::new(), SessionId::new());
        let p = Path::new("/f");
        let mut st = Snapshots::with_caps(Caps {
            per_file: 8,
            per_session: 10_000,
            per_engine: 10_000,
        });
        for i in 0..9 {
            rec(&mut st, s, "/f", &format!("{i:08x}"), "x");
        }
        assert!(st.get(s, p, "00000000").is_none(), "oldest of 9 evicted");
        assert!(st.get(s, p, "00000001").is_some());
        assert!(st.get(t, p, "00000001").is_none(), "per session");
        // A row costs text + path + 64: /a 126, /b 96, /c 96.
        let mut st = Snapshots::with_caps(Caps {
            per_file: 8,
            per_session: 300,
            per_engine: 350,
        });
        rec(&mut st, s, "/a", "aaaaaaaa", &"a".repeat(60));
        rec(&mut st, s, "/b", "bbbbbbbb", &"b".repeat(30));
        assert!(st.get(s, Path::new("/a"), "aaaaaaaa").is_some(), "touch /a");
        rec(&mut st, s, "/c", "cccccccc", &"c".repeat(30));
        assert!(
            st.get(s, Path::new("/b"), "bbbbbbbb").is_none(),
            "LRU /b evicted"
        );
        assert!(st.get(s, Path::new("/a"), "aaaaaaaa").is_some());
        // Engine cap across sessions: /c is now the least recently used.
        rec(&mut st, t, "/d", "dddddddd", &"d".repeat(80));
        assert!(
            st.get(s, Path::new("/c"), "cccccccc").is_none(),
            "engine LRU"
        );
        assert!(st.get(s, Path::new("/a"), "aaaaaaaa").is_some());
        assert!(st.get(t, Path::new("/d"), "dddddddd").is_some());
        // Over 4 MiB is never recorded.
        rec(
            &mut st,
            s,
            "/f",
            "eeeeeeee",
            &"e".repeat(MAX_SNAPSHOT_BYTES + 1),
        );
        assert!(st.get(s, p, "eeeeeeee").is_none());
        // Empty texts still cost their key and overhead (67 bytes each here).
        let mut st = Snapshots::with_caps(Caps {
            per_file: 8,
            per_session: 150,
            per_engine: 10_000,
        });
        for f in ["/x1", "/x2", "/x3"] {
            rec(&mut st, s, f, "00000000", "");
        }
        assert!(
            st.get(s, Path::new("/x1"), "00000000").is_none(),
            "tiny rows count"
        );
        assert!(st.get(s, Path::new("/x3"), "00000000").is_some());
    }

    #[test]
    fn clips_have_an_engine_lru_cap_and_go_with_their_session() {
        let (s, t) = (SessionId::new(), SessionId::new());
        let clip = |n: usize| {
            let c = Clip {
                lines: vec!["y".repeat(n - 1)],
                final_nl: true,
                strings: None,
            };
            let size = c.bytes();
            (Arc::new(c), size)
        };
        let mut all = Clips::with_engine_cap(250);
        let (c, n) = clip(100);
        all.put(s, "a".into(), c, n).unwrap();
        let (c, n) = clip(100);
        all.put(t, "b".into(), c, n).unwrap();
        assert!(all.get(s, "a").is_ok(), "touch a");
        let (c, n) = clip(100);
        all.put(t, "c".into(), c, n).unwrap();
        assert_eq!(
            all.get(t, "b").unwrap_err(),
            vec!["c".to_owned()],
            "b evicted"
        );
        assert!(all.get(s, "a").is_ok());
        all.forget(s);
        assert!(
            all.get(s, "a").unwrap_err().is_empty(),
            "gone with its session"
        );
        assert!(all.get(t, "c").is_ok(), "other sessions keep theirs");
    }

    #[test]
    fn symbols_resolve_exact_then_by_suffix_and_list_ambiguity() {
        let sym = |name: &str, start, end| SymbolSpan {
            name: name.into(),
            kind: "function".into(),
            start,
            end,
        };
        let info = SyntaxInfo {
            symbols: vec![
                sym("load", 1, 3),
                sym("Reader.load", 5, 9),
                sym("Writer.load", 11, 12),
                sym("Reader.close", 14, 15),
            ],
            ..SyntaxInfo::default()
        };
        assert_eq!(find_symbol(Some(&info), "a", "load").unwrap(), (1, 3));
        assert_eq!(
            find_symbol(Some(&info), "a", "Reader.load").unwrap(),
            (5, 9)
        );
        assert_eq!(find_symbol(Some(&info), "a", "close").unwrap(), (14, 15));
        let info2 = SyntaxInfo {
            symbols: info.symbols[1..].to_vec(),
            ..SyntaxInfo::default()
        };
        let e = find_symbol(Some(&info2), "a", "load")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("Reader.load (function, lines 5-9)")
                && e.contains("Writer.load")
                && e.contains("qualify it"),
            "{e}"
        );
        // Two `A.fmt` (Display and Debug impls): qualifying cannot help.
        let twins = SyntaxInfo {
            symbols: vec![sym("A.fmt", 1, 3), sym("A.fmt", 5, 7)],
            ..SyntaxInfo::default()
        };
        let e = find_symbol(Some(&twins), "a", "fmt")
            .unwrap_err()
            .to_string();
        assert!(
            e.ends_with("; use a line ref") && !e.contains("qualify"),
            "{e}"
        );
        assert!(find_symbol(Some(&info), "a", "nope").is_err());
        let e = find_symbol(None, "a.txt", "x").unwrap_err().to_string();
        assert!(e.contains("need a parser"), "{e}");
    }

    #[test]
    fn line_anchors_are_exact_and_the_rest_are_symbols() {
        assert_eq!(
            parse_anchor("L2Cache").unwrap(),
            Anchor::Symbol("L2Cache".into())
        );
        assert_eq!(
            parse_anchor("L2Cache.get").unwrap(),
            Anchor::Symbol("L2Cache.get".into())
        );
        assert!(
            parse_anchor("L2")
                .unwrap_err()
                .to_string()
                .contains("needs the snapshot")
        );
        assert!(parse_anchor("L2-L3").is_err());
    }

    /// §5: a file that changed after the call resolved is refused before
    /// anything is written; a failed second write removes a file the call
    /// created first.
    #[cfg(unix)]
    #[tokio::test]
    async fn commit_refuses_stale_files_and_rolls_back_created_ones() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = crate::test_dir("commit");
        let a = scratch.path().join("a.txt");
        std::fs::write(&a, "old\n").unwrap();
        let stale = Planned {
            path: a.clone(),
            shown: "a.txt".into(),
            before: Some(b"older\n".to_vec()),
            after: b"new\n".to_vec(),
        };
        let e = commit(&[stale]).await.unwrap_err().to_string();
        assert!(
            e.contains("changed on disk while this call ran; nothing written"),
            "{e}"
        );
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "old\n");

        // Root ignores 0o555; the rollback cannot be forced then.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let ro = scratch.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::write(ro.join("b.txt"), "b\n").unwrap();
        let created = scratch.path().join("new/c.txt");
        let plan = [
            Planned {
                path: created.clone(),
                shown: "new/c.txt".into(),
                before: None,
                after: b"c\n".to_vec(),
            },
            Planned {
                path: ro.join("b.txt"),
                shown: "ro/b.txt".into(),
                before: Some(b"b\n".to_vec()),
                after: b"B\n".to_vec(),
            },
        ];
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let res = commit(&plan).await;
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = res.unwrap_err().to_string();
        assert!(e.contains("new/c.txt written first and restored"), "{e}");
        assert!(!created.exists(), "the created file is removed again");
        assert_eq!(std::fs::read_to_string(ro.join("b.txt")).unwrap(), "b\n");
    }

    #[test]
    fn docs_round_trip_crlf_bom_and_missing_final_newline() {
        for raw in ["a\r\nb\r\n", "\u{feff}a\nb", "", "\n", "a\nb\n"] {
            let d = Doc::from_bytes("/x".into(), "x", Some(raw.as_bytes().to_vec())).unwrap();
            assert_eq!(d.render(), raw.as_bytes(), "{raw:?}");
        }
        assert!(Doc::from_bytes("/x".into(), "x", Some(vec![0xff, 0xfe, 0])).is_err());
    }
}
