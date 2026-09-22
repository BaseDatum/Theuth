//! `edit`: tolerant, line-anchored text replacement (12 §10). The engine
//! ([`apply`]) is pure over `&str`; the tool wraps it with line-ending / BOM
//! handling, sequential all-or-nothing multi-edit, and an atomic write. The
//! unified-diff rendering ([`unified`]) is shared with `write_file`.
//!
//! Matching ladder, stopping at the first tier with a match: exact bytes;
//! line-trimmed (trailing whitespace ignored, one uniform indentation offset
//! allowed and re-applied to `new`); whitespace-normalised (every whitespace
//! run is one space, `new` re-indented to the first matched line). Tiers two
//! and three match whole lines only. A tier that matches more than once and
//! `all == false` is an error; a looser tier is never tried in that case.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use crate::fs::refuse_read_output;
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Which tier of the matching ladder found `old`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Byte-exact substring.
    Exact,
    /// Whole lines, trailing whitespace ignored, uniform indentation offset.
    LineTrimmed,
    /// Whole lines, every whitespace run collapsed to one space.
    WhitespaceNormalised,
    /// `cat -n` prefixes (`N<tab>`, as `read_file` prints them) stripped
    /// from `old`, then the tiers above rerun.
    LineNumbersStripped,
}

impl Tier {
    /// Name used in messages and `structured.tier`.
    pub fn name(self) -> &'static str {
        match self {
            Tier::Exact => "exact",
            Tier::LineTrimmed => "line-trimmed",
            Tier::WhitespaceNormalised => "whitespace-normalised",
            Tier::LineNumbersStripped => "line-numbers-stripped",
        }
    }
}

/// Result of one successful [`apply`].
#[derive(Debug)]
pub struct Applied {
    /// Text after the replacement.
    pub text: String,
    /// Number of places replaced (1 unless `all`).
    pub count: usize,
    /// Tier that matched.
    pub tier: Tier,
}

/// Byte range of one match in the file and the text that replaces it.
struct Span {
    start: usize,
    end: usize,
    replacement: String,
}

/// Replace `old` with `new` in `text` (LF line endings). `path` only labels
/// the error messages, which are written for the model: a miss reports the
/// closest region with a diff, an ambiguous match lists the line numbers.
pub fn apply(text: &str, old: &str, new: &str, all: bool, path: &str) -> anyhow::Result<Applied> {
    if let Some(a) = ladder(text, old, new, all, path)? {
        return Ok(a);
    }
    // Loosest tier: `old` pasted from read_file with its `N<tab>` prefixes.
    // Only reached when the numbered text did not match, so a file whose
    // lines really look like that is never affected.
    if let Some(stripped) = strip_line_numbers(old) {
        let new_stripped = strip_line_numbers(new);
        let new = new_stripped.as_deref().unwrap_or(new);
        if let Some(mut a) = ladder(text, &stripped, new, all, path)? {
            a.tier = Tier::LineNumbersStripped;
            return Ok(a);
        }
    }
    let old_body = old.strip_suffix('\n').unwrap_or(old);
    Err(not_found(text, &Lines::new(text), old_body, path))
}

/// `s` with a `cat -n` prefix (`^\s*\d+\t`) removed from every line, or
/// `None` unless every non-blank line has one (and there is at least one).
fn strip_line_numbers(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut any = false;
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim().is_empty() {
            out.push_str(line);
            continue;
        }
        let t = line.trim_start();
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        let rest = t[digits..].strip_prefix('\t')?;
        if digits == 0 {
            return None;
        }
        any = true;
        out.push_str(rest);
    }
    any.then_some(out)
}

/// The three whitespace tiers; `None` when `old` is not in `text`.
fn ladder(
    text: &str,
    old: &str,
    new: &str,
    all: bool,
    path: &str,
) -> anyhow::Result<Option<Applied>> {
    let exact: Vec<Span> = text
        .match_indices(old)
        .map(|(i, _)| Span {
            start: i,
            end: i + old.len(),
            replacement: new.to_owned(),
        })
        .collect();
    if !exact.is_empty() {
        return splice(text, exact, all, Tier::Exact, path).map(Some);
    }
    let lines = Lines::new(text);
    // A trailing newline in `old` means "through the end of the last line",
    // which the byte range below includes; the lines themselves never carry it.
    let old_body = old.strip_suffix('\n').unwrap_or(old);
    let old_nl = old_body.len() < old.len();
    let old_lines: Vec<&str> = old_body.split('\n').collect();
    let n = old_lines.len();
    if n <= lines.len() && old_lines.iter().any(|l| !l.trim().is_empty()) {
        let trimmed = lines.windows(n, |window| {
            trimmed_delta(window, &old_lines).map(|d| reindent(new, d.strip(), d.add()))
        });
        if !trimmed.is_empty() {
            let spans = lines.spans(trimmed, n, old_nl);
            return splice(text, spans, all, Tier::LineTrimmed, path).map(Some);
        }
        let old_norm: Vec<String> = old_lines.iter().map(|l| normalise(l)).collect();
        let normalised = lines.windows(n, |window| {
            window
                .iter()
                .zip(&old_norm)
                .all(|(f, o)| normalise(f) == *o)
                .then(|| {
                    reindent(
                        new,
                        leading_ws(first_nonblank(&old_lines)),
                        leading_ws(first_nonblank(window)),
                    )
                })
        });
        if !normalised.is_empty() {
            let spans = lines.spans(normalised, n, old_nl);
            return splice(text, spans, all, Tier::WhitespaceNormalised, path).map(Some);
        }
    }
    Ok(None)
}

/// Apply non-overlapping ascending `spans` (or error on ambiguity).
fn splice(
    text: &str,
    spans: Vec<Span>,
    all: bool,
    tier: Tier,
    path: &str,
) -> anyhow::Result<Applied> {
    if spans.len() > 1 && !all {
        let lines = Lines::new(text);
        let at: Vec<String> = spans
            .iter()
            .map(|s| (lines.line_of(s.start) + 1).to_string())
            .collect();
        anyhow::bail!(
            "old matches {} places in {path} (tier: {}) at lines {}; pass all=true or include more context",
            spans.len(),
            tier.name(),
            at.join(", ")
        );
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for s in &spans {
        out.push_str(&text[cursor..s.start]);
        out.push_str(&s.replacement);
        cursor = s.end;
    }
    out.push_str(&text[cursor..]);
    Ok(Applied {
        text: out,
        count: spans.len(),
        tier,
    })
}

/// The lines of an LF text with their byte offsets; a trailing newline does
/// not open a phantom empty last line.
struct Lines<'a> {
    text: &'a str,
    /// `(start offset, content without the newline)` per line.
    lines: Vec<(usize, &'a str)>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for l in text.split_inclusive('\n') {
            lines.push((start, l.strip_suffix('\n').unwrap_or(l)));
            start += l.len();
        }
        Self { text, lines }
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    /// Contents of lines `i..i + n`.
    fn window(&self, i: usize, n: usize) -> Vec<&'a str> {
        self.lines[i..i + n].iter().map(|(_, l)| *l).collect()
    }

    /// 0-based line index containing byte `offset`.
    fn line_of(&self, offset: usize) -> usize {
        self.lines
            .partition_point(|(start, _)| *start <= offset)
            .saturating_sub(1)
    }

    /// Non-overlapping windows of `n` lines for which `f` returns a
    /// replacement, as `(first line index, replacement)`.
    fn windows(&self, n: usize, f: impl Fn(&[&str]) -> Option<String>) -> Vec<(usize, String)> {
        let mut hits = Vec::new();
        let mut i = 0;
        while i + n <= self.lines.len() {
            match f(&self.window(i, n)) {
                Some(replacement) => {
                    hits.push((i, replacement));
                    i += n;
                }
                None => i += 1,
            }
        }
        hits
    }

    /// Byte spans for line windows; `through_newline` also consumes the
    /// newline ending the window (when `old` ended with one).
    fn spans(&self, hits: Vec<(usize, String)>, n: usize, through_newline: bool) -> Vec<Span> {
        hits.into_iter()
            .map(|(i, replacement)| {
                let (last_start, last) = self.lines[i + n - 1];
                let mut end = last_start + last.len();
                if through_newline && self.text[end..].starts_with('\n') {
                    end += 1;
                }
                Span {
                    start: self.lines[i].0,
                    end,
                    replacement,
                }
            })
            .collect()
    }
}

/// Uniform indentation offset between a file window and `old`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delta<'a> {
    /// Same indentation.
    Same,
    /// Every non-blank file line carries this extra prefix.
    FileExtra(&'a str),
    /// Every non-blank `old` line carries this extra prefix.
    OldExtra(&'a str),
}

impl<'a> Delta<'a> {
    /// Prefix to remove from `new`'s lines.
    fn strip(self) -> &'a str {
        match self {
            Delta::OldExtra(p) => p,
            _ => "",
        }
    }
    /// Prefix to add to `new`'s lines.
    fn add(self) -> &'a str {
        match self {
            Delta::FileExtra(p) => p,
            _ => "",
        }
    }
}

/// Tier two: `window` equals `old` line by line ignoring trailing whitespace,
/// with one indentation offset shared by every non-blank line (blank lines
/// pair with blank lines whatever their whitespace). Tabs and spaces are not
/// interchanged: the offset is a literal prefix.
fn trimmed_delta<'a>(window: &[&'a str], old: &[&'a str]) -> Option<Delta<'a>> {
    let mut delta = None;
    for (f, o) in window.iter().zip(old) {
        let (f, o) = (f.trim_end(), o.trim_end());
        match (f.is_empty(), o.is_empty()) {
            (true, true) => continue,
            (true, false) | (false, true) => return None,
            (false, false) => {}
        }
        let d = if let Some(p) = f.strip_suffix(o).filter(|p| p.trim().is_empty()) {
            if p.is_empty() {
                Delta::Same
            } else {
                Delta::FileExtra(p)
            }
        } else if let Some(p) = o.strip_suffix(f).filter(|p| p.trim().is_empty()) {
            Delta::OldExtra(p)
        } else {
            return None;
        };
        match delta {
            None => delta = Some(d),
            Some(prev) if prev == d => {}
            Some(_) => return None,
        }
    }
    delta
}

/// Tier three's line key: trimmed, interior whitespace runs collapsed.
fn normalise(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

fn first_nonblank<'a>(lines: &[&'a str]) -> &'a str {
    lines
        .iter()
        .copied()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
}

/// Re-indent `new`: on every non-blank line, drop the longest prefix shared
/// with `strip`, then prepend `add`. Blank lines and `new`'s own trailing
/// newline are kept as they are.
fn reindent(new: &str, strip: &str, add: &str) -> String {
    if strip.is_empty() && add.is_empty() {
        return new.to_owned();
    }
    let mut out = String::with_capacity(new.len() + add.len() * 8);
    for line in new.split_inclusive('\n') {
        if line.trim().is_empty() {
            out.push_str(line);
            continue;
        }
        let common = line
            .chars()
            .zip(strip.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        out.push_str(add);
        out.push_str(&line[common..]);
    }
    out
}

/// Longest the closest-candidate scan may run; a huge file reports the best
/// window seen so far instead of stalling the model.
const CANDIDATE_BUDGET: Duration = Duration::from_secs(2);
/// Lines of the `old` → candidate diff shown in a not-found error.
const CANDIDATE_DIFF_LINES: usize = 40;

/// Tier four: the not-found error, naming the window of `old`'s length that
/// is most similar (word-level ratio over whitespace-normalised text).
fn not_found(text: &str, lines: &Lines, old: &str, path: &str) -> anyhow::Error {
    if text.trim().is_empty() {
        return anyhow::anyhow!(
            "old not found in {path}: the file is empty. Use write_file to create its content."
        );
    }
    let old_lines: Vec<&str> = old.split('\n').collect();
    let n = old_lines.len().min(lines.len());
    let old_norm = old_lines
        .iter()
        .map(|l| normalise(l))
        .collect::<Vec<_>>()
        .join("\n");
    // ponytail: linear scan with a word-level diff per window, bounded by
    // CANDIDATE_BUDGET; index by line hash if multi-MB files turn out common.
    let deadline = Instant::now() + CANDIDATE_BUDGET;
    let mut best: Option<(f32, usize)> = None;
    for i in 0..=lines.len() - n {
        let cand = lines
            .window(i, n)
            .iter()
            .map(|l| normalise(l))
            .collect::<Vec<_>>()
            .join("\n");
        let ratio = TextDiff::configure()
            .deadline(deadline)
            .diff_words(old_norm.as_str(), cand.as_str())
            .ratio();
        if best.is_none_or(|(b, _)| ratio > b) {
            best = Some((ratio, i));
        }
        if Instant::now() > deadline {
            break;
        }
    }
    let Some((ratio, i)) = best.filter(|(r, _)| *r > 0.5) else {
        return anyhow::anyhow!(
            "old not found in {path}; no similar region (best similarity {:.2}). Re-read the file with read_file and retry with the exact text.",
            best.map_or(0.0, |(r, _)| r)
        );
    };
    let (first, last) = (i + 1, i + n);
    let candidate = lines.window(i, n).join("\n");
    let diff = TextDiff::from_lines(old, candidate.as_str())
        .unified_diff()
        .context_radius(3)
        .header("old", &format!("{path} lines {first}–{last}"))
        .missing_newline_hint(false)
        .to_string();
    let (diff, _, _) = cap_lines(&diff, CANDIDATE_DIFF_LINES);
    anyhow::anyhow!(
        "old not found in {path}. Closest match (similarity {ratio:.2}) is at lines {first}–{last}:\n{diff}Re-read those lines with read_file(path, offset={first}, limit={n}) and retry with the exact text."
    )
}

/// Keep the first `max` lines of `text`; returns `(kept, dropped, truncated)`.
fn cap_lines(text: &str, max: usize) -> (String, usize, bool) {
    let total = text.lines().count();
    if total <= max {
        return (text.to_owned(), 0, false);
    }
    let kept: String = text.split_inclusive('\n').take(max).collect();
    (
        format!("{kept}[diff truncated: {} more lines]\n", total - max),
        total - max,
        true,
    )
}

/// Lines of unified diff shown in a tool result before it is cut.
const DIFF_LINES: usize = 200;

/// A rendered unified diff with its line counts.
#[derive(Debug, Default)]
pub struct Diff {
    /// `--- a/path` / `+++ b/path` unified diff, capped at [`DIFF_LINES`]
    /// lines plus a `[diff truncated: N more lines]` trailer.
    pub text: String,
    /// Lines added.
    pub added: usize,
    /// Lines removed.
    pub removed: usize,
    /// Whether the cap cut the diff.
    pub truncated: bool,
}

/// Unified diff `orig` → `new` for `path`, three lines of context.
pub fn unified(orig: &str, new: &str, path: &str) -> Diff {
    let td = TextDiff::from_lines(orig, new);
    let (mut added, mut removed) = (0, 0);
    for c in td.iter_all_changes() {
        match c.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    let full = td
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    let (text, _, truncated) = cap_lines(&full, DIFF_LINES);
    Diff {
        text,
        added,
        removed,
        truncated,
    }
}

/// One replacement in [`EditArgs::edits`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EditOp {
    /// Text to replace.
    pub old: String,
    /// Replacement text.
    pub new: String,
    /// Replace every occurrence; default: the top-level `all`.
    #[serde(default)]
    pub all: Option<bool>,
}

/// Arguments for [`Edit`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EditArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// Text to replace (single-edit form). Mutually exclusive with `edits`.
    #[serde(default)]
    pub old: Option<String>,
    /// Replacement text (single-edit form).
    #[serde(default)]
    pub new: Option<String>,
    /// Replace every occurrence instead of requiring exactly one (single-edit
    /// form, or the default for `edits` entries that omit it).
    #[serde(default)]
    pub all: bool,
    /// Several replacements applied in order to the same file, all-or-nothing:
    /// each sees the previous one's result.
    #[serde(default)]
    pub edits: Option<Vec<EditOp>>,
}

/// Validated replacement.
struct Op {
    old: String,
    new: String,
    all: bool,
}

impl EditArgs {
    /// Validate into the ordered list of replacements.
    fn ops(self) -> anyhow::Result<Vec<Op>> {
        let ops = match (self.old, self.new, self.edits) {
            (None, None, Some(edits)) => {
                anyhow::ensure!(
                    !edits.is_empty(),
                    "`edits` is empty; list at least one {{old, new}} or pass `old` and `new`"
                );
                edits
                    .into_iter()
                    .map(|e| Op {
                        old: e.old,
                        new: e.new,
                        all: e.all.unwrap_or(self.all),
                    })
                    .collect()
            }
            (Some(old), Some(new), None) => vec![Op {
                old,
                new,
                all: self.all,
            }],
            (_, _, Some(_)) => {
                anyhow::bail!("pass either `old`+`new` or `edits`, not both")
            }
            (old, _, None) => anyhow::bail!(
                "pass both `old` and `new` (missing `{}`), or `edits`",
                if old.is_none() { "old" } else { "new" }
            ),
        };
        let n = ops.len();
        for (i, op) in ops.iter().enumerate() {
            let check = || -> anyhow::Result<()> {
                anyhow::ensure!(!op.old.is_empty(), "`old` must not be empty");
                anyhow::ensure!(op.old != op.new, "no-op: old and new are identical");
                refuse_read_output(&op.new)
            };
            check().map_err(|e| label(e, i, n))?;
        }
        Ok(ops)
    }
}

/// Prefix an error with its op index in the multi-edit form.
fn label(e: anyhow::Error, i: usize, n: usize) -> anyhow::Error {
    if n == 1 {
        e
    } else {
        anyhow::anyhow!("edit {} of {n}: {e}", i + 1)
    }
}

/// Replace text in a file (12 §10).
pub struct Edit;

#[async_trait::async_trait]
impl Tool for Edit {
    const NAME: &'static str = "edit";
    const RISK: Risk = Risk::Mutating;
    type Args = EditArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Replace `old` with `new` in a text file, or apply several `edits` in order (each sees the previous result; any failure writes nothing). Matching is exact first, then tolerant of trailing whitespace and a uniform indentation offset, then of any whitespace differences on whole lines, and as a last resort `cat -n` line-number prefixes (`N<tab>`, as read_file prints them) are stripped from `old` (and from `new` when every line has them); a match at more than one place is an error unless `all` is true, and a miss reports the closest region as a diff with its line numbers. Returns a unified diff of what changed. A `new` carrying a read_file header or truncation marker is refused; paths are relative to the process cwd, `~` is not expanded.".into()
    }

    async fn call(&self, a: EditArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let path = a.path.clone();
        let ops = a.ops()?;
        edit_file(&path, &ops).await
    }
}

/// UTF-8 byte order mark.
const BOM: &str = "\u{feff}";

/// Apply `ops` in order to the file at `path`, all-or-nothing, written
/// atomically (tmp + rename, permissions kept). CRLF files are matched as LF
/// and written back as CRLF; a BOM and the presence or absence of a final
/// newline are preserved.
async fn edit_file(path: &Path, ops: &[Op]) -> anyhow::Result<ToolOutput> {
    let shown = path.display().to_string();
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| anyhow::anyhow!("{shown}: {e}"))?;
    let raw = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{shown}: not UTF-8"))?;
    let (bom, body) = match raw.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, raw.as_str()),
    };
    let crlf = {
        let crlf = body.matches("\r\n").count();
        crlf > 0 && crlf * 2 >= body.matches('\n').count()
    };
    let orig = if crlf {
        body.replace("\r\n", "\n")
    } else {
        body.to_owned()
    };

    let mut text = orig.clone();
    let (mut replacements, mut tier) = (0, Tier::Exact);
    for (i, op) in ops.iter().enumerate() {
        let (old, new) = (op.old.replace("\r\n", "\n"), op.new.replace("\r\n", "\n"));
        let applied =
            apply(&text, &old, &new, op.all, &shown).map_err(|e| label(e, i, ops.len()))?;
        text = applied.text;
        replacements += applied.count;
        tier = tier.max(applied.tier);
    }

    let diff = unified(&orig, &text, &shown);
    let mut out = if crlf {
        text.replace('\n', "\r\n")
    } else {
        text
    };
    if bom {
        out.insert_str(0, BOM);
    }
    if out == raw {
        let mut o = ToolOutput::ok(format!(
            "{shown}: no changes ({replacements} replacements left the text identical)"
        ));
        o.structured = Some(structured(&shown, replacements, tier, &diff));
        return Ok(o);
    }
    write_atomic(path, out.as_bytes()).await?;
    let plural = if replacements == 1 { "" } else { "s" };
    let tier_note = if tier == Tier::Exact {
        String::new()
    } else {
        format!(" [tier: {}]", tier.name())
    };
    let mut o = ToolOutput::ok(format!(
        "edited {shown}: {replacements} replacement{plural} (+{} −{} lines){tier_note}\n{}",
        diff.added, diff.removed, diff.text
    ));
    o.structured = Some(structured(&shown, replacements, tier, &diff));
    Ok(o)
}

fn structured(path: &str, replacements: usize, tier: Tier, diff: &Diff) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "replacements": replacements,
        "added": diff.added,
        "removed": diff.removed,
        "tier": tier.name(),
        "diff": diff.text,
        "truncated": diff.truncated,
    })
}

/// Write `bytes` to `path` via `<path>.tmp-<random>` in the same directory
/// and rename over it, keeping the existing file's permissions. The tmp file
/// never outlives an error.
pub(crate) async fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", uuid::Uuid::new_v4().simple()));
    let tmp = PathBuf::from(tmp);
    let perm = match tokio::fs::metadata(path).await {
        Ok(m) => Some(m.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => anyhow::bail!("{}: {e}", path.display()),
    };
    let attempt = async {
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", tmp.display()))?;
        if let Some(perm) = perm {
            tokio::fs::set_permissions(&tmp, perm).await?;
        }
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    };
    let res = attempt.await;
    if res.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    res
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

    fn single(path: &Path, old: &str, new: &str, all: bool) -> EditArgs {
        EditArgs {
            path: path.to_owned(),
            old: Some(old.into()),
            new: Some(new.into()),
            all,
            edits: None,
        }
    }

    fn multi(path: &Path, ops: &[(&str, &str)]) -> EditArgs {
        EditArgs {
            path: path.to_owned(),
            old: None,
            new: None,
            all: false,
            edits: Some(
                ops.iter()
                    .map(|(o, n)| EditOp {
                        old: (*o).into(),
                        new: (*n).into(),
                        all: None,
                    })
                    .collect(),
            ),
        }
    }

    async fn run(a: EditArgs) -> anyhow::Result<ToolOutput> {
        Tool::call(&Edit, a, cx()).await
    }

    fn ok(text: &str, old: &str, new: &str, all: bool) -> Applied {
        apply(text, old, new, all, "f.rs").unwrap()
    }

    fn err(text: &str, old: &str, new: &str, all: bool) -> String {
        apply(text, old, new, all, "f.rs").unwrap_err().to_string()
    }

    // ---- engine tiers ----

    #[test]
    fn exact_tier_replaces_bytes() {
        let a = ok("a foo b", "foo", "bar", false);
        assert_eq!(
            (a.text.as_str(), a.count, a.tier),
            ("a bar b", 1, Tier::Exact)
        );
        let a = ok("foo foo", "foo", "x", true);
        assert_eq!((a.text.as_str(), a.count), ("x x", 2));
    }

    #[test]
    fn line_trimmed_ignores_trailing_ws_and_shifts_indent_with_spaces() {
        let file = "fn f() {\n        if x {   \n            y();\n        }\n}\n";
        // Model wrote the block one level (4 spaces) shallower than the file.
        let old = "    if x {\n        y();\n    }\n";
        let new = "    if x {\n        y();\n        z();\n    }\n";
        let a = ok(file, old, new, false);
        assert_eq!(a.tier, Tier::LineTrimmed);
        assert_eq!(
            a.text,
            "fn f() {\n        if x {\n            y();\n            z();\n        }\n}\n"
        );
        // And the other way round: model over-indented.
        let file = "if x {\n    y();\n}\n";
        let a = ok(
            file,
            "        if x {\n            y();\n        }",
            "        if x {\n            z();\n        }",
            false,
        );
        assert_eq!(a.tier, Tier::LineTrimmed);
        assert_eq!(a.text, "if x {\n    z();\n}\n");
    }

    #[test]
    fn line_trimmed_shifts_indent_with_tabs() {
        let file = "\t\tfoo();\n\t\tbar();\n";
        let a = ok(file, "\tfoo();\n\tbar();", "\tfoo();\n\tbaz();", false);
        assert_eq!(a.tier, Tier::LineTrimmed);
        assert_eq!(a.text, "\t\tfoo();\n\t\tbaz();\n");
        // Mixed tabs/spaces with a non-uniform delta is not tier two.
        let a = ok(
            "\tfoo();\n    bar();\n",
            "foo();\nbar();",
            "x();\ny();",
            false,
        );
        assert_eq!(a.tier, Tier::WhitespaceNormalised);
    }

    #[test]
    fn line_trimmed_keeps_blank_lines_and_relative_indent_in_new() {
        let file = "    a\n\n    b\n";
        let a = ok(file, "a\n\nb", "a\n\n  inner\nb", false);
        assert_eq!(a.text, "    a\n\n      inner\n    b\n");
    }

    #[test]
    fn whitespace_normalised_reindents_to_first_matched_line() {
        let file = "    let  x =   1;\n    let y = 2;\n";
        let old = "let x = 1;\nlet y = 2;";
        let new = "let x = 1;\nif x {\n    let y = 2;\n}";
        let a = ok(file, old, new, false);
        assert_eq!(a.tier, Tier::WhitespaceNormalised);
        assert_eq!(
            a.text,
            "    let x = 1;\n    if x {\n        let y = 2;\n    }\n"
        );
        // Tabs in the file, spaces in `new`: the file's indent wins.
        let a = ok("\tfoo  bar\n", "        foo bar", "        baz", false);
        assert_eq!(a.text, "\tbaz\n");
    }

    #[test]
    fn line_tiers_never_match_half_a_line() {
        // Trailing whitespace keeps the exact tier out; the line tiers must
        // refuse `prefix foo` for `foo`.
        let e = err("prefix foo\nbar\n", "foo  \nbar", "x", false);
        assert!(e.starts_with("old not found in f.rs"), "{e}");
    }

    #[test]
    fn trailing_newline_in_old_consumes_the_line() {
        let a = ok("a\n  b\nc\n", "b  \n", "", false);
        assert_eq!(a.tier, Tier::LineTrimmed);
        assert_eq!(a.text, "a\nc\n");
        let a = ok("a\n  b\nc\n", "b  ", "B", false);
        assert_eq!(a.text, "a\n  B\nc\n");
    }

    #[test]
    fn ambiguity_errors_name_tier_and_lines_and_do_not_fall_through() {
        let e = err("foo\nx\nfoo\n", "foo", "bar", false);
        assert_eq!(
            e,
            "old matches 2 places in f.rs (tier: exact) at lines 1, 3; pass all=true or include more context"
        );
        let e = err("  foo\nx\n  foo\n", "foo ", "bar", false);
        assert!(e.contains("(tier: line-trimmed) at lines 1, 3"), "{e}");
        let e = err("a  b\nx\na   b\n", "a b", "c", false);
        assert!(
            e.contains("(tier: whitespace-normalised) at lines 1, 3"),
            "{e}"
        );
        // all=true takes every match of the tier that matched.
        let a = ok("  foo\nx\n  foo\n", "foo ", "bar", true);
        assert_eq!((a.text.as_str(), a.count), ("  bar\nx\n  bar\n", 2));
    }

    #[test]
    fn line_numbers_pasted_from_read_file_are_stripped_last() {
        let file = "fn f() {\n    a();\n\n    b();\n}\n";
        // Numbered `old`, plain `new`: `new` is used as given.
        let a = ok(
            file,
            "2\t    a();\n3\t\n4\t    b();",
            "    a();\n    c();",
            false,
        );
        assert_eq!(a.tier, Tier::LineNumbersStripped);
        assert_eq!(a.text, "fn f() {\n    a();\n    c();\n}\n");
        // Both numbered (right-aligned widths too): both stripped.
        let a = ok(
            file,
            " 9\t    a();\n10\t\n11\t    b();",
            " 9\t    a();\n10\t    d();",
            false,
        );
        assert_eq!(a.tier, Tier::LineNumbersStripped);
        assert_eq!(a.text, "fn f() {\n    a();\n    d();\n}\n");
        // `new` with prefixes on only some lines is left alone.
        let a = ok(file, "4\t    b();", "4\t    b();\n    e();", false);
        assert_eq!(a.text, "fn f() {\n    a();\n\n4\t    b();\n    e();\n}\n");
        // A file whose lines really are `N<tab>…` matches exactly, unstripped.
        let tsv = "1\tfoo\n2\tbar\n";
        let a = ok(tsv, "1\tfoo\n2\tbar\n", "1\tFOO\n2\tbar\n", false);
        assert_eq!(a.tier, Tier::Exact);
        assert_eq!(a.text, "1\tFOO\n2\tbar\n");
        // Some lines numbered, some not: nothing is stripped, not found.
        let e = err(file, "2\t    a();\n    b();", "x", false);
        assert!(e.starts_with("old not found in f.rs"), "{e}");
        assert_eq!(strip_line_numbers("  \n\n"), None);
        assert_eq!(strip_line_numbers("12 x\n"), None);
    }

    #[test]
    fn not_found_reports_closest_window_with_lines_and_diff() {
        let file: String = (1..=30)
            .map(|i| format!("line number {i} here\n"))
            .chain(["fn target(a: u32) -> u32 {\n", "    a + 1\n", "}\n"].map(String::from))
            .collect();
        let e = err(
            &file,
            "fn target(a: u32) -> u64 {\n    a + 2\n}",
            "x",
            false,
        );
        assert!(
            e.starts_with("old not found in f.rs. Closest match (similarity 0."),
            "{e}"
        );
        assert!(e.contains("is at lines 31–33:\n"), "{e}");
        assert!(e.contains("\n-fn target(a: u32) -> u64 {\n"), "{e}");
        assert!(e.contains("\n+fn target(a: u32) -> u32 {\n"), "{e}");
        assert!(e.ends_with("Re-read those lines with read_file(path, offset=31, limit=3) and retry with the exact text."), "{e}");
        let e = err(
            "completely different\n",
            "nothing like it at all",
            "x",
            false,
        );
        assert!(e.contains("no similar region"), "{e}");
        let e = err("", "x", "y", false);
        assert!(e.contains("the file is empty"), "{e}");
    }

    // ---- tool: args, line endings, multi-edit, write ----

    #[tokio::test]
    async fn arg_validation() {
        let p = scratch("abc");
        let mut a = single(&p, "a", "a", false);
        assert_eq!(
            run(a).await.unwrap_err().to_string(),
            "no-op: old and new are identical"
        );
        a = single(&p, "", "a", false);
        assert!(
            run(a)
                .await
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
        a = single(&p, "a", "b", false);
        a.edits = Some(vec![]);
        assert!(run(a).await.unwrap_err().to_string().contains("not both"));
        a = multi(&p, &[]);
        assert!(
            run(a)
                .await
                .unwrap_err()
                .to_string()
                .contains("`edits` is empty")
        );
        a = single(&p, "a", "b", false);
        a.new = None;
        assert!(
            run(a)
                .await
                .unwrap_err()
                .to_string()
                .contains("missing `new`")
        );
        let e = run(multi(&p, &[("a", "b"), ("c", "c")]))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(e, "edit 2 of 2: no-op: old and new are identical");
        let e = run(single(
            &p,
            "a",
            &format!("x{}", crate::fs::TRUNCATED_TAIL),
            false,
        ))
        .await
        .unwrap_err()
        .to_string();
        assert!(e.starts_with("refusing to write:"), "{e}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "abc");
    }

    #[tokio::test]
    async fn result_carries_diff_and_structured() {
        let p = scratch("one\ntwo\nthree\n");
        let out = run(single(&p, "two", "2\n2b", false)).await.unwrap();
        let shown = p.display().to_string();
        assert!(
            out.content.starts_with(&format!(
                "edited {shown}: 1 replacement (+2 −1 lines)\n--- a/{shown}\n+++ b/{shown}\n@@ -1,3 +1,4 @@\n one\n-two\n+2\n+2b\n three\n"
            )),
            "{}",
            out.content
        );
        let s = out.structured.unwrap();
        assert_eq!(s["replacements"], 1);
        assert_eq!(s["added"], 2);
        assert_eq!(s["removed"], 1);
        assert_eq!(s["tier"], "exact");
        assert_eq!(s["truncated"], false);
        // Tier note only when not exact.
        let out = run(single(&p, "  three", "  3", false)).await.unwrap();
        assert!(
            out.content.contains("(+1 −1 lines) [tier: line-trimmed]\n"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn crlf_round_trips_byte_exact() {
        let p = scratch("a\r\nfoo\r\nb\r\n");
        run(single(&p, "foo\n", "bar\nbaz\n", false)).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nbar\r\nbaz\r\nb\r\n");
        // `old`/`new` given with CRLF match too.
        run(single(&p, "bar\r\nbaz", "q", false)).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nq\r\nb\r\n");
    }

    #[tokio::test]
    async fn no_trailing_newline_and_bom_are_preserved() {
        let p = scratch("a\nb");
        run(single(&p, "b", "c", false)).await.unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a\nc");
        let p = scratch("\u{feff}a\nb\n");
        let out = run(single(&p, "a", "x", false)).await.unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "\u{feff}x\nb\n");
        assert!(out.content.contains("\n-a\n+x\n"), "{}", out.content);
    }

    #[tokio::test]
    async fn multi_edit_is_sequential_and_all_or_nothing() {
        let p = scratch("a\nb\nc\n");
        let out = run(multi(&p, &[("a", "b"), ("b\nb", "B"), ("c", "d")]))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "B\nd\n");
        assert_eq!(out.structured.unwrap()["replacements"], 3);
        let p = scratch("a\nb\nc\n");
        let e = run(multi(&p, &[("a", "A"), ("b", "B"), ("zzz", "Z")]))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("edit 3 of 3: old not found in"), "{e}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a\nb\nc\n");
        assert!(
            !p.parent()
                .unwrap()
                .join(format!("{}.tmp", p.file_name().unwrap().to_str().unwrap()))
                .exists()
        );
    }

    #[tokio::test]
    async fn identical_result_skips_the_write() {
        let p = scratch("a\nb\n");
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        let out = run(multi(&p, &[("a", "x"), ("x", "a")])).await.unwrap();
        assert!(out.content.contains("no changes"), "{}", out.content);
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);
    }

    #[tokio::test]
    async fn diff_is_capped_at_200_lines() {
        // 300 lines all changed: 2 file headers + 1 hunk header + 600 lines.
        let orig: String = (0..300).map(|i| format!("l{i}\n")).collect();
        let new: String = (0..300).map(|i| format!("m{i}\n")).collect();
        let d = unified(&orig, &new, "f");
        assert!(d.truncated);
        assert_eq!((d.added, d.removed), (300, 300));
        assert_eq!(d.text.lines().count(), DIFF_LINES + 1);
        let tail = format!("[diff truncated: {} more lines]\n", 603 - DIFF_LINES);
        assert!(d.text.ends_with(&tail), "{}", &d.text[d.text.len() - 60..]);
        let p = scratch(&orig);
        let out = run(single(&p, "l", "m", true)).await.unwrap();
        let s = out.structured.unwrap();
        assert_eq!(s["truncated"], true);
        assert!(s["diff"].as_str().unwrap().ends_with(&tail));
        assert!(
            out.content.ends_with(&tail),
            "{}",
            &out.content[out.content.len() - 60..]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let p = scratch("#!/bin/sh\necho hi\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        run(single(&p, "hi", "bye", false)).await.unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "#!/bin/sh\necho bye\n"
        );
    }
}
