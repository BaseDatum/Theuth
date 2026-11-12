//! 193 TEXT-REFS tools: `move_text`, `copy_text`, `indent` and `clip` move
//! text the model names by ref ([`crate::textref`]) without retyping it.
//! Re-indent is a uniform relative shift (§3), never language-aware.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::textref::{self, Anchor, Clip, Doc, Planned, Ref, SyntaxInfo};
use crate::{Risk, Tool, ToolCx, ToolOutput};

// ------------------------------------------------------- indent rules (§3)

/// How the target file indents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Style {
    /// Render leading whitespace with tabs.
    pub tabs: bool,
    /// Columns per level.
    pub unit: usize,
    /// Columns a tab advances to (measuring and rendering).
    pub tab_width: usize,
}

/// `.editorconfig` for `path` (ec4rs), else detected from `lines` (§3).
pub(crate) fn style(path: &Path, lines: &[String]) -> Style {
    use ec4rs::property::{IndentSize, IndentStyle, TabWidth};
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let cfg = ec4rs::properties_of(&abs).unwrap_or_default();
    let style = cfg.get::<IndentStyle>().ok();
    let size = cfg.get::<IndentSize>().ok();
    let tab_width = match (cfg.get::<TabWidth>().ok(), size) {
        (Some(TabWidth::Value(n)), _) | (_, Some(IndentSize::Value(n))) if n > 0 => n,
        _ => 4,
    };
    let tabs = match style {
        Some(s) => s == IndentStyle::Tabs,
        None => detect_tabs(lines),
    };
    let unit = match size {
        Some(IndentSize::Value(n)) if n > 0 => n,
        Some(IndentSize::UseTabWidth) => tab_width,
        _ if tabs => tab_width,
        _ => detect_step(lines, tab_width),
    };
    // A silly `.editorconfig` size cannot overflow the width arithmetic.
    Style {
        tabs,
        unit: unit.min(MAX_WIDTH),
        tab_width: tab_width.min(MAX_WIDTH),
    }
}

/// More indented non-blank lines start with a tab than with a space.
fn detect_tabs(lines: &[String]) -> bool {
    let (mut tab, mut space) = (0, 0);
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        match l.as_bytes()[0] {
            b'\t' => tab += 1,
            b' ' => space += 1,
            _ => {}
        }
    }
    tab > space
}

/// The most common positive step between consecutive non-blank lines among
/// 2, 3, 4, 8; ties and nothing found: 4.
fn detect_step(lines: &[String], tab_width: usize) -> usize {
    let mut count = [0usize; 9];
    let mut prev: Option<usize> = None;
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        let w = width(l, tab_width);
        if let Some(p) = prev
            && let Some(d) = w.checked_sub(p)
            && matches!(d, 2 | 3 | 4 | 8)
        {
            count[d] += 1;
        }
        prev = Some(w);
    }
    let best = count.iter().copied().max().unwrap_or(0);
    let winners: Vec<usize> = (0..count.len())
        .filter(|&d| best > 0 && count[d] == best)
        .collect();
    match winners.as_slice() {
        [one] => *one,
        _ => 4,
    }
}

/// Columns of `line`'s leading spaces and tabs.
fn width(line: &str, tab_width: usize) -> usize {
    let mut w = 0;
    for c in line.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w += tab_width - w % tab_width,
            _ => break,
        }
    }
    w
}

/// `line` without its leading spaces and tabs.
fn body(line: &str) -> &str {
    line.trim_start_matches([' ', '\t'])
}

/// Leading whitespace of `w` columns in `style`.
fn render(w: usize, style: Style) -> String {
    if style.tabs {
        let mut s = "\t".repeat(w / style.tab_width);
        s.push_str(&" ".repeat(w % style.tab_width));
        s
    } else {
        " ".repeat(w)
    }
}

/// Smallest width among non-blank lines outside multi-line strings.
fn base(lines: &[String], strings: Option<&[bool]>, tab_width: usize) -> usize {
    lines
        .iter()
        .enumerate()
        .filter(|(i, l)| !l.trim().is_empty() && !strings.is_some_and(|s| s[*i]))
        .map(|(_, l)| width(l, tab_width))
        .min()
        .unwrap_or(0)
}

/// A block after [`shift`].
struct Shifted {
    lines: Vec<String>,
    /// Lines left alone because they sit inside a multi-line string.
    kept: usize,
}

/// Shift `lines` so `base` lands at `target` (§3). `first` (1-based) and
/// `label` name the first line a negative shift would cut into.
fn shift(
    lines: &[String],
    strings: Option<&[bool]>,
    base: usize,
    target: i64,
    style: Style,
    label: &str,
    first: usize,
) -> anyhow::Result<Shifted> {
    let mut out = Vec::with_capacity(lines.len());
    let mut kept = 0;
    for (i, l) in lines.iter().enumerate() {
        if strings.is_some_and(|s| s[i]) {
            kept += usize::from(!l.is_empty());
            out.push(l.clone());
            continue;
        }
        if l.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let w = width(l, style.tab_width) as i64 - base as i64 + target;
        anyhow::ensure!(
            w >= 0,
            "the shift would cut into {label}:{}: `{}` has {} columns of indentation; shift by less",
            first + i,
            textref::cut_line(l.trim()),
            width(l, style.tab_width)
        );
        anyhow::ensure!(
            w <= MAX_WIDTH as i64,
            "the shift would indent {label}:{} to {w} columns; the limit is {MAX_WIDTH}",
            first + i,
        );
        out.push(format!("{}{}", render(w as usize, style), body(l)));
    }
    Ok(Shifted { lines: out, kept })
}

/// Widest indentation a shift may produce.
const MAX_WIDTH: usize = 1024;
/// Most levels `by` / `"+N"` may shift.
const MAX_LEVELS: i64 = 64;

fn check_levels(k: i64) -> anyhow::Result<i64> {
    anyhow::ensure!(
        (-MAX_LEVELS..=MAX_LEVELS).contains(&k),
        "a shift of {k} levels is refused; at most {MAX_LEVELS} either way"
    );
    Ok(k)
}

/// The line ends with an opener once a trailing comment is dropped.
// ponytail: comment detection ignores string literals (`"//"` inside a
// string truncates the line); the receipt shows the seam, so a miss costs
// one `indent` call.
fn opens(line: &str) -> bool {
    let mut l = line;
    if let Some(p) = l.find("//") {
        l = &l[..p];
    }
    if let Some((p, _)) = l
        .match_indices('#')
        .find(|(p, _)| *p == 0 || l[..*p].ends_with([' ', '\t']))
    {
        l = &l[..p];
    }
    l.trim_end().ends_with(['{', '(', '[', ':'])
}

/// The `match` depth for a block inserted before `lines[i]` (§3 rule 3).
fn line_depth(lines: &[String], i: usize, style: Style) -> usize {
    let prev = lines[..i].iter().rev().find(|l| !l.trim().is_empty());
    let next = lines[i..].iter().find(|l| !l.trim().is_empty());
    let w = |l: &String| width(l, style.tab_width);
    match (prev, next) {
        (Some(p), _) if opens(p) => w(p) + style.unit,
        (p, Some(n)) if p.is_none_or(|p| w(n) >= w(p)) => w(n),
        (Some(p), _) => w(p),
        _ => 0,
    }
}

/// `indent` on a paste: `"match"`, `"keep"`, `"+N"`, `"-N"` (units).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Match,
    Keep,
    By(i64),
}

fn mode(s: &str) -> anyhow::Result<Mode> {
    let units = |d: &str| d.bytes().all(|c| c.is_ascii_digit()) && !d.is_empty();
    match s {
        "match" => Ok(Mode::Match),
        "keep" => Ok(Mode::Keep),
        _ if s.starts_with(['+', '-']) && units(&s[1..]) => {
            let k: i64 = s
                .parse()
                .map_err(|_| anyhow::anyhow!("indent `{s}`: too many levels"))?;
            Ok(Mode::By(check_levels(k)?))
        }
        _ => anyhow::bail!("indent `{s}`: use \"match\", \"keep\", \"+N\" or \"-N\" (levels)"),
    }
}

/// `+4 spaces (1 level)`, `-1 tab (1 level)`, `+3 spaces`.
fn describe(delta: i64, style: Style) -> String {
    let sign = if delta < 0 { "-" } else { "+" };
    let n = delta.unsigned_abs() as usize;
    let s = |k: usize| if k == 1 { "" } else { "s" };
    let mut out = if style.tabs && n.is_multiple_of(style.tab_width) {
        let k = n / style.tab_width;
        format!("{sign}{k} tab{}", s(k))
    } else {
        format!("{sign}{n} space{}", s(n))
    };
    if style.unit > 0 && n.is_multiple_of(style.unit) && n > 0 {
        let k = n / style.unit;
        out.push_str(&format!(" ({k} level{})", s(k)));
    }
    out
}

/// The receipt note on strings for a re-indented block.
fn strings_note(strings: Option<&[bool]>, kept: usize) -> Option<String> {
    match strings {
        None => Some("no parser for this language: multi-line strings not protected".into()),
        Some(_) if kept == 1 => Some("1 line inside a multi-line string kept".into()),
        Some(_) if kept > 1 => Some(format!("{kept} lines inside a multi-line string kept")),
        Some(_) => None,
    }
}

/// Per line of `a..=b` (1-based): inside a multi-line string.
fn interior(info: Option<&SyntaxInfo>, a: usize, b: usize) -> Option<Vec<bool>> {
    let info = info?;
    Some(
        (a..=b)
            .map(|n| info.string_lines.binary_search(&n).is_ok())
            .collect(),
    )
}

/// Bytes `lines` take on disk with LF or CRLF breaks.
fn rendered_bytes(lines: &[String], crlf: bool) -> usize {
    let nl = if crlf { 2 } else { 1 };
    lines.iter().map(|l| l.len() + nl).sum()
}

// ------------------------------------------------------- shared plumbing

/// `a-b` or `a` for one line.
fn span(a: usize, b: usize) -> String {
    if a == b {
        a.to_string()
    } else {
        format!("{a}-{b}")
    }
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// One document's syntax (analysed at most once) and line diffs for the
/// duration of one call, plus receipt notes about them.
#[derive(Default)]
struct Parsed {
    /// `None`: not analysed yet; `Some(Err)`: the hook failed.
    info: Option<Result<Option<SyntaxInfo>, String>>,
    diffs: textref::Diffs,
    notes: Vec<String>,
}

/// Lines `(a, b)` (1-based, inclusive) an anchor names in `doc`.
async fn resolve(
    session: runtime::SessionId,
    doc: &Doc,
    anchor: &Anchor,
    p: &mut Parsed,
) -> anyhow::Result<(usize, usize)> {
    match anchor {
        Anchor::Lines { first, last, snap } => {
            textref::map_lines(session, doc, *first, *last, snap, &mut p.diffs).await
        }
        Anchor::Symbol(name) => {
            let info = syntax(doc, p)
                .await
                .map_err(|e| anyhow::anyhow!("{e}; use a line ref"))?;
            let (a, b) = textref::find_symbol(info, &doc.shown, name)?;
            let errors = info.is_some_and(|i| i.has_errors);
            let n = doc.lines.len();
            anyhow::ensure!(
                1 <= a && a <= b && b <= n,
                "the parser placed `{name}` at lines {a}-{b}, outside {} ({n} lines); use a line ref",
                doc.shown
            );
            let note = "file has syntax errors: symbol span may be approximate";
            if errors && !p.notes.iter().any(|x| x == note) {
                p.notes.push(note.into());
            }
            Ok((a, b))
        }
        Anchor::Start | Anchor::End => {
            anyhow::bail!("`start`/`end` only name a paste target (`at`); name lines or a symbol")
        }
    }
}

/// `doc`'s syntax, analysed once per call: `Err` when the hook failed.
async fn syntax<'a>(doc: &Doc, p: &'a mut Parsed) -> Result<Option<&'a SyntaxInfo>, String> {
    if p.info.is_none() {
        p.info = Some(textref::analyze(doc).await);
    }
    match p.info.as_ref() {
        Some(Ok(info)) => Ok(info.as_ref()),
        Some(Err(e)) => Err(e.clone()),
        None => Ok(None),
    }
}

/// One written file for the receipt and `structured.files`.
struct Written {
    shown: String,
    id: Option<String>,
    diff: String,
}

/// Write `docs` all-or-nothing (§5), record their snapshots for `session`.
/// Unchanged documents are not written but still reported.
async fn write_docs(session: runtime::SessionId, docs: &[&Doc]) -> anyhow::Result<Vec<Written>> {
    let rendered: Vec<Vec<u8>> = docs.iter().map(|d| d.render()).collect();
    let plan: Vec<Planned> = docs
        .iter()
        .zip(&rendered)
        .filter(|(d, after)| d.before.as_deref() != Some(after.as_slice()))
        .map(|(d, after)| Planned {
            path: d.path.clone(),
            shown: d.shown.clone(),
            before: d.before.clone(),
            after: after.clone(),
        })
        .collect();
    textref::commit(&plan).await?;
    let mut out = Vec::new();
    for (d, after) in docs.iter().zip(&rendered) {
        let orig = match &d.before {
            Some(b) => textref::normalize(std::str::from_utf8(b).unwrap_or_default())
                .2
                .into_owned(),
            None => String::new(),
        };
        out.push(Written {
            shown: d.shown.clone(),
            id: textref::record_doc(session, d, after),
            diff: crate::edit::unified(&orig, &d.text(), &d.shown).text,
        });
    }
    Ok(out)
}

/// ` · a.rs @1a2b3c4d` per file.
fn ids(files: &[Written]) -> String {
    files
        .iter()
        .map(|f| match &f.id {
            Some(id) => format!(" · {} @{id}", f.shown),
            None => format!(" · {} (over 4 MiB, no snapshot)", f.shown),
        })
        .collect()
}

/// §6 `structured`.
fn structured(
    op: &str,
    lines: usize,
    bytes: usize,
    notes: &[String],
    files: &[Written],
) -> serde_json::Value {
    serde_json::json!({
        "op": op,
        "lines": lines,
        "bytes": bytes,
        "notes": notes,
        "files": files.iter().map(|f| serde_json::json!({
            "path": f.shown,
            "snapshot": f.id,
            "diff": f.diff,
        })).collect::<Vec<_>>(),
    })
}

/// `head`, then ` · note` for each note.
fn with_notes(mut head: String, notes: &[String]) -> String {
    for n in notes {
        head.push_str(" · ");
        head.push_str(n);
    }
    head
}

/// A resolved source block.
struct Block {
    /// `path:a-b` or `clip:name`.
    label: String,
    /// The file a shift refusal names (`path` or `clip:name`) and the line
    /// number of the block's first line in it.
    origin: (String, usize),
    lines: Vec<String>,
    /// The last line ended with a newline where it came from.
    final_nl: bool,
    strings: Option<Vec<bool>>,
}

/// Lines of `doc` named by `anchor`, with their string-interior flags.
async fn block_of(
    session: runtime::SessionId,
    doc: &Doc,
    anchor: &Anchor,
    p: &mut Parsed,
    want_strings: bool,
) -> anyhow::Result<(usize, usize, Block)> {
    let (a, b) = resolve(session, doc, anchor, p).await?;
    // A failed hook protects nothing (the receipt says so).
    let strings = if want_strings {
        interior(syntax(doc, p).await.ok().flatten(), a, b)
    } else {
        None
    };
    Ok((
        a,
        b,
        Block {
            label: format!("{}:{}", doc.shown, span(a, b)),
            origin: (doc.shown.clone(), a),
            lines: doc.lines[a - 1..b].to_vec(),
            final_nl: b < doc.lines.len() || doc.final_nl,
            strings,
        },
    ))
}

// ------------------------------------------------------ move/copy (§4)

/// Arguments for [`MoveText`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MoveTextArgs {
    /// Source: `path#L<n>-L<m>@<id>` (lines from the read_file header id or
    /// a receipt) or `path#Symbol` (`splice`, `Reader.load`; widened over
    /// doc comments, attributes and decorators).
    pub src: String,
    /// Insert above this line or symbol: `path#L<n>@<id>` or `path#Symbol`.
    #[serde(default)]
    pub before: Option<String>,
    /// Insert below this line or symbol.
    #[serde(default)]
    pub after: Option<String>,
    /// `path#start` or `path#end`; creates a missing file.
    #[serde(default)]
    pub at: Option<String>,
    /// `"match"` (default: fit the target's indentation), `"keep"`, or
    /// `"+N"`/`"-N"` levels from the source's own depth.
    #[serde(default = "match_mode")]
    pub indent: String,
}

/// Arguments for [`CopyText`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CopyTextArgs {
    /// As `move_text.src`, or `clip:<name>` for a stored clip.
    pub src: String,
    /// Insert above this line or symbol: `path#L<n>@<id>` or `path#Symbol`.
    #[serde(default)]
    pub before: Option<String>,
    /// Insert below this line or symbol.
    #[serde(default)]
    pub after: Option<String>,
    /// `path#start` or `path#end`; creates a missing file.
    #[serde(default)]
    pub at: Option<String>,
    /// `"match"` (default), `"keep"`, `"+N"`/`"-N"` levels.
    #[serde(default = "match_mode")]
    pub indent: String,
}

fn match_mode() -> String {
    "match".into()
}

/// Where a pasted block goes.
enum Target {
    Before(PathBuf, Anchor),
    After(PathBuf, Anchor),
    At(PathBuf, Anchor),
}

impl Target {
    fn parse(before: Option<&str>, after: Option<&str>, at: Option<&str>) -> anyhow::Result<Self> {
        let at_file = |s: &str| -> anyhow::Result<(PathBuf, Anchor)> {
            match textref::parse_ref(s)? {
                Ref::At { path, anchor } => Ok((path, anchor)),
                Ref::Clip(_) => anyhow::bail!("`clip:` is a source only; name a file target"),
            }
        };
        let line_or_symbol = |s: &str| -> anyhow::Result<(PathBuf, Anchor)> {
            let (p, a) = at_file(s)?;
            match &a {
                Anchor::Lines { first, last, .. } if first != last => anyhow::bail!(
                    "target `{s}` names lines {first}-{last}; a target is one line: L<n>@<id>"
                ),
                Anchor::Start | Anchor::End => {
                    anyhow::bail!("target `{s}`: start/end go in `at`, not `before`/`after`")
                }
                _ => Ok((p, a)),
            }
        };
        match (before, after, at) {
            (Some(b), None, None) => {
                let (p, a) = line_or_symbol(b)?;
                Ok(Self::Before(p, a))
            }
            (None, Some(x), None) => {
                let (p, a) = line_or_symbol(x)?;
                Ok(Self::After(p, a))
            }
            (None, None, Some(x)) => {
                let (p, a) = at_file(x)?;
                anyhow::ensure!(
                    matches!(a, Anchor::Start | Anchor::End),
                    "`at` takes path#start or path#end; use before/after for a line or symbol"
                );
                Ok(Self::At(p, a))
            }
            _ => anyhow::bail!("pass exactly one of `before`, `after`, `at`"),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Before(p, _) | Self::After(p, _) | Self::At(p, _) => p,
        }
    }
}

/// How `match` measures the target (§3 rules 1-3).
#[derive(Clone, Copy)]
enum Depth {
    /// Rule 1: the symbol's widened first line, 0-based index.
    Symbol(usize),
    /// Rule 2: column 0.
    Edge,
    /// Rule 3: from the lines around the insertion point.
    Line,
}

/// Shared body of `move_text` and `copy_text`.
async fn paste(
    cut: bool,
    src: &str,
    target: Target,
    indent: &str,
    cx: &ToolCx,
) -> anyhow::Result<ToolOutput> {
    let session = cx.session;
    let mode = mode(indent)?;
    let want_strings = mode != Mode::Keep;

    // Target path first: same-file moves share one document.
    let tshown = target.path().display().to_string();
    let tpath = crate::seat::writable(session, target.path())?;
    crate::seat::refuse_cell_record(&cx.resources.root, &tpath)?;

    // Source.
    let (mut src_doc, src_parsed, src_span, block) = match textref::parse_ref(src)? {
        Ref::Clip(name) => {
            anyhow::ensure!(
                !cut,
                "move_text takes a file source; copy_text pastes clips"
            );
            let c = textref::get_clip(session, &name)?;
            let label = format!("clip:{name}");
            let block = Block {
                origin: (label.clone(), 1),
                label,
                lines: c.lines.clone(),
                final_nl: c.final_nl,
                strings: c.strings.clone(),
            };
            (None, Parsed::default(), None, block)
        }
        Ref::At { path, anchor } => {
            let shown = path.display().to_string();
            let resolved = if cut {
                crate::seat::writable(session, &path)?
            } else {
                crate::seat::resolve(session, &path)
            };
            crate::seat::refuse_cell_record(&cx.resources.root, &resolved)?;
            let doc = Doc::load(resolved, &shown, false).await?;
            let mut p = Parsed::default();
            let (a, b, block) = block_of(session, &doc, &anchor, &mut p, want_strings).await?;
            (Some(doc), p, Some((a, b)), block)
        }
    };
    let same = src_doc
        .as_ref()
        .is_some_and(|d| textref::key(&d.path) == textref::key(&tpath));

    // Target document and insertion point; a same-file paste edits the
    // source document itself.
    let mut notes = Vec::new();
    let (mut tdoc, mut tparsed) = match (same, src_doc.take()) {
        (true, Some(d)) => (d, src_parsed),
        (_, sd) => {
            src_doc = sd;
            notes.extend(src_parsed.notes);
            let create = matches!(target, Target::At(..));
            (Doc::load(tpath, &tshown, create).await?, Parsed::default())
        }
    };
    let (mut at, mut depth) = match &target {
        Target::At(_, Anchor::Start) => (0, Depth::Edge),
        Target::At(_, _) => (tdoc.lines.len(), Depth::Edge),
        Target::Before(_, a) | Target::After(_, a) => {
            let (x, y) = resolve(session, &tdoc, a, &mut tparsed).await?;
            let i = if matches!(target, Target::Before(..)) {
                x - 1
            } else {
                y
            };
            let d = match a {
                Anchor::Symbol(_) => Depth::Symbol(x - 1),
                _ => Depth::Line,
            };
            (i, d)
        }
    };
    notes.extend(tparsed.notes);

    // A move takes the block out first; same-file indices shift.
    let n = block.lines.len();
    if cut && let Some((a, b)) = src_span {
        if same {
            anyhow::ensure!(
                !(a - 1 < at && at < b),
                "the target lies inside the source ({}:{}); pick a target outside it",
                tdoc.shown,
                span(a, b)
            );
            tdoc.lines.drain(a - 1..b);
            if at >= b {
                at -= n;
            }
            if let Depth::Symbol(i) = &mut depth
                && *i >= b
            {
                *i -= n;
            }
        } else if let Some(d) = src_doc.as_mut() {
            d.lines.drain(a - 1..b);
        }
    }

    // Re-indent for the target, measured after the removal.
    let style = style(&tdoc.path, &tdoc.lines);
    let strings = block.strings.as_deref();
    let base = base(&block.lines, strings, style.tab_width);
    let target_w: Option<i64> = match mode {
        Mode::Keep => None,
        Mode::By(k) => Some(base as i64 + k * style.unit as i64),
        Mode::Match => Some(match depth {
            Depth::Symbol(i) => tdoc.lines.get(i).map_or(0, |l| width(l, style.tab_width)) as i64,
            Depth::Edge => 0,
            Depth::Line => line_depth(&tdoc.lines, at, style) as i64,
        }),
    };
    let (lines, delta) = match target_w {
        None => (block.lines.clone(), 0),
        Some(t) => {
            let (file, first) = &block.origin;
            let sh = shift(&block.lines, strings, base, t, style, file, *first)?;
            notes.extend(strings_note(strings, sh.kept));
            (sh.lines, t - base as i64)
        }
    };

    // Insert; the final-newline rules of §4.
    let bytes = rendered_bytes(&lines, tdoc.crlf);
    if at == tdoc.lines.len() {
        tdoc.final_nl = if tdoc.lines.is_empty() {
            block.final_nl
        } else {
            tdoc.final_nl || block.final_nl
        };
    }
    tdoc.lines.splice(at..at, lines);
    let (c, d) = (at + 1, at + n);
    let seams = seams(&tdoc, c, d);
    let tshown = tdoc.shown.clone();

    let files = match (cut, src_doc.as_ref()) {
        (true, Some(sd)) => write_docs(session, &[sd, &tdoc]).await?,
        _ => write_docs(session, &[&tdoc]).await?,
    };
    let verb = if cut { "moved" } else { "copied" };
    let ind = match mode {
        Mode::Keep => " (indent kept)".to_owned(),
        _ if delta != 0 => format!(" (indent {}{delta})", if delta > 0 { "+" } else { "" }),
        _ => String::new(),
    };
    let head = format!(
        "{verb} {n} line{} {} -> {tshown}:{}{ind}{}",
        s(n),
        block.label,
        span(c, d),
        ids(&files)
    );
    let mut out = ToolOutput::ok(format!("{}{seams}", with_notes(head, &notes)));
    out.structured = Some(structured(
        if cut { "move" } else { "copy" },
        n,
        bytes,
        &notes,
        &files,
    ));
    Ok(out)
}

/// The numbered line above and below an inserted block `c..=d` (1-based).
fn seams(doc: &Doc, c: usize, d: usize) -> String {
    let rows: Vec<(usize, &str)> = [c.checked_sub(1).filter(|&n| n >= 1), Some(d + 1)]
        .into_iter()
        .flatten()
        .filter(|&n| n <= doc.lines.len())
        .map(|n| (n, doc.lines[n - 1].as_str()))
        .collect();
    let w = rows
        .iter()
        .map(|(n, _)| n.to_string().len())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(n, l)| format!("\n  {}:{n:<w$}  {}", doc.shown, textref::cut_line(l)))
        .collect()
}

/// Move lines or a symbol to another place, same file or another, without
/// retyping them.
pub struct MoveText;

#[async_trait::async_trait]
impl Tool for MoveText {
    const NAME: &'static str = "move_text";
    const RISK: Risk = Risk::Mutating;
    type Args = MoveTextArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Move a block of text without retyping it: `src` names lines you have read (`path#L40-L52@<id>`, the id from the read_file header or an earlier receipt; the line ref follows the file's later changes through that snapshot id, and is refused only if those very lines changed) or a symbol (`path#Reader.load`, widened over its doc comments, attributes and decorators). Exactly one target: `before`/`after` (`path#L<n>@<id>` or `path#Symbol`) or `at` (`path#start`/`path#end`, creating a missing file). Same file or two files, all-or-nothing. `indent=\"match\"` (default) shifts the block to fit the target, `\"keep\"` pastes it as is, `\"+N\"`/`\"-N\"` shifts by levels (at most 64); where codeindex has a parser (rust, typescript, javascript, python, go), lines inside multi-line strings are not re-indented. Returns a one-line receipt with each file's new snapshot id and the lines either side of the paste; a refusal shows the named lines as they are now.".into()
    }

    async fn call(&self, a: MoveTextArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let t = Target::parse(a.before.as_deref(), a.after.as_deref(), a.at.as_deref())?;
        paste(true, &a.src, t, &a.indent, &cx).await
    }
}

/// Copy lines, a symbol or a clip to a place, without retyping them.
pub struct CopyText;

#[async_trait::async_trait]
impl Tool for CopyText {
    const NAME: &'static str = "copy_text";
    const RISK: Risk = Risk::Mutating;
    type Args = CopyTextArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Copy a block of text without retyping it: as move_text, but the source stays, and `src` may also be `clip:<name>` (a block stored earlier with clip). Targets: `before`/`after` (`path#L<n>@<id>` or `path#Symbol`) or `at` (`path#start`/`path#end`, creating a missing file). `indent`: \"match\" (default), \"keep\", \"+N\"/\"-N\" levels. Returns a receipt with the target's new snapshot id and the lines either side of the paste.".into()
    }

    async fn call(&self, a: CopyTextArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let t = Target::parse(a.before.as_deref(), a.after.as_deref(), a.at.as_deref())?;
        paste(false, &a.src, t, &a.indent, &cx).await
    }
}

// --------------------------------------------------------------- indent

/// Arguments for [`Indent`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IndentArgs {
    /// File path, absolute or relative to the session's working directory (no `~`).
    pub path: PathBuf,
    /// The block: `L<n>-L<m>@<id>` (lines from the read_file header id) or
    /// a symbol name (`Reader.load`).
    pub range: String,
    /// Levels to shift by (negative: outdent), in the file's own unit.
    #[serde(default)]
    pub by: Option<i64>,
    /// A line `L<n>@<id>` in the same file whose indentation the block's
    /// base takes (then `by` levels are added).
    #[serde(default, rename = "match")]
    pub r#match: Option<String>,
}

/// Re-indent a block in one call.
pub struct Indent;

#[async_trait::async_trait]
impl Tool for Indent {
    const NAME: &'static str = "indent";
    const RISK: Risk = Risk::Mutating;
    type Args = IndentArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Fix a block's indentation in one call: `range` is `L<n>-L<m>@<id>` (lines from the read_file header id) or a symbol name; `by` shifts it by N levels (negative outdents, at most 64), `match=\"L<n>@<id>\"` gives it that line's indentation (plus `by` levels). The whole block moves together, keeping its inner relative indentation; the unit comes from .editorconfig or is detected from the file; blank lines become empty; where codeindex has a parser (rust, typescript, javascript, python, go), lines inside multi-line strings are left untouched. A shift that would cut into text, or indent past 1024 columns, is refused. Returns a one-line receipt with the file's new snapshot id.".into()
    }

    async fn call(&self, a: IndentArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        anyhow::ensure!(
            a.by.is_some() || a.r#match.is_some(),
            "pass `by` (levels), `match` (a line to align with), or both"
        );
        let by = check_levels(a.by.unwrap_or(0))?;
        let session = cx.session;
        let shown = a.path.display().to_string();
        let path = crate::seat::writable(session, &a.path)?;
        crate::seat::refuse_cell_record(&cx.resources.root, &path)?;
        let range = textref::parse_anchor(&a.range)?;
        let mut doc = Doc::load(path, &shown, false).await?;
        let mut p = Parsed::default();
        let (x, y, block) = block_of(session, &doc, &range, &mut p, true).await?;
        let style = style(&doc.path, &doc.lines);
        let strings = block.strings.as_deref();
        let base = base(&block.lines, strings, style.tab_width);
        let anchor = match &a.r#match {
            Some(m) => {
                let anchor = textref::parse_anchor(m)?;
                anyhow::ensure!(
                    matches!(&anchor, Anchor::Lines { first, last, .. } if first == last),
                    "`match` is one line: L<n>@<id>"
                );
                let (n, _) = resolve(session, &doc, &anchor, &mut p).await?;
                width(&doc.lines[n - 1], style.tab_width)
            }
            None => base,
        };
        let target = anchor as i64 + by * style.unit as i64;
        let sh = shift(&block.lines, strings, base, target, style, &shown, x)?;
        let delta = target - base as i64;
        let bytes = rendered_bytes(&sh.lines, doc.crlf);
        doc.lines.splice(x - 1..y, sh.lines);
        let files = write_docs(session, &[&doc]).await?;
        let mut notes = p.notes;
        notes.extend(strings_note(strings, sh.kept));
        let head = format!(
            "indented {shown}:{} by {}{}",
            span(x, y),
            describe(delta, style),
            ids(&files)
        );
        let mut out = ToolOutput::ok(with_notes(head, &notes));
        out.structured = Some(structured("indent", y + 1 - x, bytes, &notes, &files));
        Ok(out)
    }
}

// ----------------------------------------------------- edit range (§4)

/// `edit`'s range mode: each `(anchor, new)` names whole lines of `path`
/// (resolved against its snapshot or, for a symbol, the current text);
/// they must not overlap and apply together, bottom-up. `new` replaces the
/// lines with their breaks; `""` deletes them.
pub(crate) async fn edit_ranges(
    session: runtime::SessionId,
    path: &Path,
    ops: Vec<(Anchor, String)>,
) -> anyhow::Result<ToolOutput> {
    let shown = path.display().to_string();
    let mut doc = Doc::load(path.to_path_buf(), &shown, false).await?;
    let mut p = Parsed::default();
    let mut spans = Vec::with_capacity(ops.len());
    let count = ops.len();
    for (i, (anchor, new)) in ops.into_iter().enumerate() {
        let (a, b) = resolve(session, &doc, &anchor, &mut p).await.map_err(|e| {
            if count == 1 {
                e
            } else {
                anyhow::anyhow!("edit {} of {count}: {e}", i + 1)
            }
        })?;
        spans.push((a, b, new));
    }
    spans.sort_by_key(|s| s.0);
    for w in spans.windows(2) {
        anyhow::ensure!(
            w[1].0 > w[0].1,
            "ranges {shown}:{} and {shown}:{} overlap; merge them into one",
            span(w[0].0, w[0].1),
            span(w[1].0, w[1].1)
        );
    }
    let (mut removed, mut added, mut bytes) = (0, 0, 0);
    let mut single = String::new();
    for (a, b, new) in spans.iter().rev() {
        let (lines, nl) = textref::split_lines(new);
        if *b == doc.lines.len() && !new.is_empty() {
            doc.final_nl = doc.final_nl || nl;
        }
        removed += b + 1 - a;
        added += lines.len();
        bytes += rendered_bytes(&lines, doc.crlf);
        single = if lines.is_empty() {
            format!(
                "deleted {shown}:{} ({} line{})",
                span(*a, *b),
                b + 1 - a,
                s(b + 1 - a)
            )
        } else {
            format!(
                "replaced {shown}:{} ({} line{}) with {} line{}",
                span(*a, *b),
                b + 1 - a,
                s(b + 1 - a),
                lines.len(),
                s(lines.len())
            )
        };
        doc.lines.splice(a - 1..*b, lines);
    }
    let files = write_docs(session, &[&doc]).await?;
    let head = if spans.len() == 1 {
        single
    } else {
        format!(
            "edited {shown}: {} ranges, -{removed} +{added} lines",
            spans.len()
        )
    };
    let mut out = ToolOutput::ok(with_notes(format!("{head}{}", ids(&files)), &p.notes));
    out.structured = Some(structured("edit", added, bytes, &p.notes, &files));
    Ok(out)
}

// ----------------------------------------------------------------- clip

/// Arguments for [`ClipTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ClipArgs {
    /// Clip name: 1-64 of `A-Z a-z 0-9 _ . -`; an existing clip of that
    /// name is replaced.
    pub name: String,
    /// `path#L<n>-L<m>@<id>` or `path#Symbol`.
    pub src: String,
}

/// Store a block now, paste it later with `copy_text(src="clip:<name>")`.
pub struct ClipTool;

#[async_trait::async_trait]
impl Tool for ClipTool {
    const NAME: &'static str = "clip";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ClipArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Store a block of text under a name without retyping it, to paste later with copy_text(src=\"clip:<name>\"): `src` is `path#L<n>-L<m>@<id>` (lines from the read_file header id) or `path#Symbol`. The text is taken as it is now; a clip of the same name is replaced. Clips live in memory for this session (1 MiB each, 64 clips, 16 MiB in all) and are lost when the engine restarts. Cut-and-paste-later is clip, then edit(range=…, new=\"\") to delete.".into()
    }

    async fn call(&self, a: ClipArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let session = cx.session;
        let Ref::At { path, anchor } = textref::parse_ref(&a.src)? else {
            anyhow::bail!(
                "clip takes a file source (path#L<n>-L<m>@<id> or path#Symbol), not another clip"
            );
        };
        // Check the name before doing any work.
        textref::parse_ref(&format!("clip:{}", a.name))?;
        let shown = path.display().to_string();
        let resolved = crate::seat::resolve(session, &path);
        crate::seat::refuse_cell_record(&cx.resources.root, &resolved)?;
        let doc = Doc::load(resolved, &shown, false).await?;
        let mut p = Parsed::default();
        let (_, _, block) = block_of(session, &doc, &anchor, &mut p, true).await?;
        let n = block.lines.len();
        let bytes = rendered_bytes(&block.lines, doc.crlf);
        let label = block.label.clone();
        textref::put_clip(
            session,
            &a.name,
            Clip {
                lines: block.lines,
                final_nl: block.final_nl,
                strings: block.strings,
            },
        )?;
        let head = format!(
            "clipped {n} line{} {label} as clip:{} ({bytes} bytes)",
            s(n),
            a.name
        );
        let mut out = ToolOutput::ok(with_notes(head, &p.notes));
        out.structured = Some(structured("clip", n, bytes, &p.notes, &[]));
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
