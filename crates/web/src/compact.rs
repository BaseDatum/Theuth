//! Markdown compaction before the cap (design doc 122 D7): ten line-oriented
//! passes that remove what page chrome leaves in the markdown — invisible
//! characters, leaked utility classes, image runs, a11y phrases, repeated
//! blocks and lines, empty headings, blank runs — and optionally fold links
//! into a trailing `Links:` list. Every pass leaves fenced code verbatim.

use std::collections::HashSet;
use std::ops::Range;

use crate::engines::decode_entities;

/// What `compact` keeps or folds.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompactOptions {
    /// `[text](url)` → `text` plus a trailing `Links:` list (used by `format: text`).
    pub fold_links: bool,
    /// Keep image lines (scrape); `false` drops them after the collapse pass.
    pub keep_images: bool,
}

/// Compacted markdown and how much went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compacted {
    /// The compacted markdown.
    pub text: String,
    /// `input.chars().count() − text.chars().count()`.
    pub removed_chars: usize,
}

/// Zero-width and soft characters that never carry meaning in page text.
const INVISIBLE: &[char] = &['\u{200B}', '\u{200C}', '\u{200D}', '\u{00AD}', '\u{FEFF}'];

/// Tailwind-like class prefixes (pass 2).
const UTILITY_PREFIXES: &[&str] = &[
    "text-", "bg-", "px-", "py-", "pt-", "pb-", "pl-", "pr-", "mt-", "mb-", "ml-", "mr-", "mx-",
    "my-", "w-", "h-", "max-w-", "min-h-", "gap-", "rounded", "border-", "font-", "leading-",
    "tracking-", "shadow", "hover:", "focus:", "sm:", "md:", "lg:", "xl:", "dark:",
];

/// Whole-token utility classes (pass 2).
const UTILITY_EXACT: &[&str] = &[
    "flex", "grid", "hidden", "block", "inline", "relative", "absolute", "fixed", "sticky",
    "container", "truncate", "uppercase", "italic",
];

/// Screen-reader phrases (pass 4), lowercase; longer variants first.
const A11Y_CHROME: &[&str] = &[
    "(opens in a new window)",
    "opens in a new window",
    "opens in new tab",
    "skip to main content",
    "skip to content",
];

/// Blocks shorter than this are never deduplicated (pass 5).
const MIN_DEDUP_BLOCK_CHARS: usize = 20;
/// Words of fingerprint A that form fingerprint B (pass 5).
const PREFIX_WORDS: usize = 10;
/// Consecutive image-only lines that collapse to `[N images]` (pass 3).
const IMAGE_RUN: usize = 3;
/// Comma items on a line before repeated items are dropped (pass 7).
const COMMA_LIST_ITEMS: usize = 4;
/// Most entries in the trailing `Links:` list (pass 10).
const MAX_LINKS: usize = 60;

/// One input line; `fenced` lines (fence markers included) are never touched.
struct Line {
    text: String,
    fenced: bool,
}

/// Run the D7 passes over `markdown`, skipping fenced code verbatim.
///
/// `removed_chars` saturates at 0: `fold_links` can lengthen a link-dense
/// page by its `Links:` list.
#[must_use]
pub fn compact(markdown: &str, opts: &CompactOptions) -> Compacted {
    let input_chars = markdown.chars().count();
    let mut lines = split_fenced(markdown);
    strip_invisible(&mut lines);
    lines.retain(|l| l.fenced || !is_utility_line(&l.text));
    fold_image_runs(&mut lines, opts.keep_images);
    strip_a11y_chrome(&mut lines);
    dedup_blocks(&mut lines);
    dedup_lines_in_blocks(&mut lines);
    for l in lines.iter_mut().filter(|l| !l.fenced) {
        if let Some(deduped) = dedup_comma_list(&l.text) {
            l.text = deduped;
        }
    }
    drop_empty_headings(&mut lines);
    if opts.fold_links {
        fold_links(&mut lines);
    }
    let mut text = join_collapsed(&lines);
    if markdown.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    Compacted {
        removed_chars: input_chars.saturating_sub(text.chars().count()),
        text,
    }
}

/// Lowercase alphanumerics with whitespace collapsed to one space (fingerprint A).
fn fingerprint(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            space = true;
        }
    }
    out
}

// ---- fences -----------------------------------------------------------------

/// Split into lines, marking fenced code (``` or ~~~, up to three leading
/// spaces, closer at least as long as the opener). An unclosed fence runs to
/// the end, as in CommonMark.
fn split_fenced(md: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut open: Option<(char, usize)> = None;
    for raw in md.lines() {
        let fenced = match open {
            Some((ch, n)) => {
                if fence_marker(raw).is_some_and(|(c, m, rest)| c == ch && m >= n && rest.trim().is_empty()) {
                    open = None;
                }
                true
            }
            None => match fence_marker(raw) {
                Some((c, n, rest)) if c != '`' || !rest.contains('`') => {
                    open = Some((c, n));
                    true
                }
                _ => false,
            },
        };
        out.push(Line {
            text: raw.to_owned(),
            fenced,
        });
    }
    out
}

/// `(fence char, run length, text after the run)` when `line` starts a fence run.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let s = line.trim_start_matches(' ');
    if line.len() - s.len() > 3 {
        return None;
    }
    let ch = s.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = s.chars().take_while(|c| *c == ch).count();
    (n >= 3).then(|| (ch, n, &s[n..]))
}

/// Index ranges of blocks: maximal runs of non-blank, non-fenced lines.
fn blocks(lines: &[Line]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, l) in lines.iter().enumerate() {
        let content = !l.fenced && !l.text.trim().is_empty();
        match (start, content) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                out.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..lines.len());
    }
    out
}

fn retain_unmarked(lines: &mut Vec<Line>, drop: &[bool]) {
    let mut i = 0;
    lines.retain(|_| {
        let keep = !drop[i];
        i += 1;
        keep
    });
}

// ---- pass 1: invisible characters and leaked entities ------------------------

fn strip_invisible(lines: &mut [Line]) {
    for l in lines.iter_mut().filter(|l| !l.fenced) {
        if l.text.contains(INVISIBLE) {
            l.text.retain(|c| !INVISIBLE.contains(&c));
        }
        if l.text.contains('&') {
            l.text = decode_entities(&l.text);
        }
    }
}

// ---- pass 2: utility-class lines -------------------------------------------

/// More than half of at least three whitespace tokens are utility classes.
fn is_utility_line(text: &str) -> bool {
    let mut total = 0;
    let mut hits = 0;
    for token in text.split_whitespace() {
        total += 1;
        if UTILITY_EXACT.contains(&token) || UTILITY_PREFIXES.iter().any(|p| token.starts_with(p)) {
            hits += 1;
        }
    }
    total >= 3 && hits * 2 > total
}

// ---- pass 3: image runs ------------------------------------------------------

/// The alt text when `line` is only an image (`![alt](src)`), optionally
/// link-wrapped (`[![alt](src)](href)`) or a list item.
fn image_alt(line: &str) -> Option<&str> {
    let mut s = line.trim();
    if let Some(item) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
        s = item.trim_start();
    }
    if s.starts_with('[') {
        let inner = s.strip_prefix('[')?.strip_suffix(')')?;
        let (img, href) = inner.rsplit_once("](")?;
        if href.contains(')') {
            return None;
        }
        s = img;
    }
    let body = s.strip_prefix("![")?;
    let mid = body.find("](")?;
    let alt = &body[..mid];
    let rest = &body[mid + 2..];
    (rest.ends_with(')') && !rest[..rest.len() - 1].contains(')') && !alt.contains(']')).then_some(alt)
}

fn fold_image_runs(lines: &mut Vec<Line>, keep_images: bool) {
    let mut out = Vec::with_capacity(lines.len());
    let mut run: Vec<(Line, String)> = Vec::new();
    let flush = |run: &mut Vec<(Line, String)>, out: &mut Vec<Line>| {
        if run.len() >= IMAGE_RUN {
            let alt = run.iter().map(|(_, a)| a.trim()).find(|a| !a.is_empty());
            let text = match alt {
                Some(alt) => format!("[{} images: {alt}]", run.len()),
                None => format!("[{} images]", run.len()),
            };
            out.push(Line {
                text,
                fenced: false,
            });
        } else if keep_images {
            out.extend(run.drain(..).map(|(l, _)| l));
        }
        run.clear();
    };
    for l in lines.drain(..) {
        match (l.fenced, image_alt(&l.text).map(str::to_owned)) {
            (false, Some(alt)) => run.push((l, alt)),
            _ => {
                flush(&mut run, &mut out);
                out.push(l);
            }
        }
    }
    flush(&mut run, &mut out);
    *lines = out;
}

// ---- pass 4: a11y chrome ------------------------------------------------------

fn strip_a11y_chrome(lines: &mut Vec<Line>) {
    lines.retain_mut(|l| {
        if l.fenced {
            return true;
        }
        let mut lower = l.text.to_ascii_lowercase();
        if !A11Y_CHROME.iter().any(|p| lower.contains(p)) {
            return true;
        }
        let mut text = l.text.clone();
        for phrase in A11Y_CHROME {
            while let Some(at) = lower.find(phrase) {
                text.replace_range(at..at + phrase.len(), "");
                lower.replace_range(at..at + phrase.len(), "");
            }
        }
        let text = remove_empty_links(&text);
        if text
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, '-' | '*' | '+' | '|'))
        {
            return false;
        }
        l.text = text;
        true
    });
}

/// `[](url)` links left behind by a removed label.
fn remove_empty_links(text: &str) -> String {
    let mut s = text.to_owned();
    while let Some(at) = s.find("[](") {
        match s[at + 3..].find(')') {
            Some(close) => s.replace_range(at..at + 3 + close + 1, ""),
            None => break,
        }
    }
    s
}

// ---- pass 5: block dedup -------------------------------------------------------

fn dedup_blocks(lines: &mut Vec<Line>) {
    let mut seen_full = HashSet::new();
    let mut seen_prefix = HashSet::new();
    let mut drop = vec![false; lines.len()];
    for range in blocks(lines) {
        let text = lines[range.clone()]
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if text.chars().count() < MIN_DEDUP_BLOCK_CHARS {
            continue;
        }
        let full = fingerprint(&text);
        if full.is_empty() {
            continue;
        }
        let prefix = full.split(' ').take(PREFIX_WORDS).collect::<Vec<_>>().join(" ");
        let dup_full = !seen_full.insert(full);
        let dup_prefix = !seen_prefix.insert(prefix);
        if dup_full || dup_prefix {
            drop[range].fill(true);
        }
    }
    retain_unmarked(lines, &drop);
}

// ---- pass 6: line dedup inside a block ---------------------------------------------

fn dedup_lines_in_blocks(lines: &mut Vec<Line>) {
    let mut drop = vec![false; lines.len()];
    for range in blocks(lines) {
        let mut seen = HashSet::new();
        for i in range {
            let fp = fingerprint(&lines[i].text);
            if !fp.is_empty() && !seen.insert(fp) {
                drop[i] = true;
            }
        }
    }
    retain_unmarked(lines, &drop);
}

// ---- pass 7: comma-list dedup -----------------------------------------------------

/// The line without repeated comma items; `None` when nothing changes.
fn dedup_comma_list(text: &str) -> Option<String> {
    let items: Vec<&str> = text.split(',').collect();
    if items.len() < COMMA_LIST_ITEMS {
        return None;
    }
    let mut seen = HashSet::new();
    let mut kept = Vec::with_capacity(items.len());
    for item in &items {
        let fp = fingerprint(item);
        if !fp.is_empty() && !seen.insert(fp) {
            continue;
        }
        kept.push(*item);
    }
    (kept.len() < items.len()).then(|| kept.join(","))
}

// ---- pass 8: empty headings ---------------------------------------------------------

/// ATX heading level of `text` (`#`–`######` followed by a space or the end).
fn heading_level(text: &str) -> Option<usize> {
    let s = text.trim_start();
    let n = s.bytes().take_while(|b| *b == b'#').count();
    ((1..=6).contains(&n) && (s.len() == n || s.as_bytes()[n] == b' ')).then_some(n)
}

/// Drop a heading whose next content is a heading of the same or a higher
/// level, or the end of the document; walking backwards also removes runs of
/// trailing headings.
fn drop_empty_headings(lines: &mut Vec<Line>) {
    let n = lines.len();
    let mut drop = vec![false; n];
    for i in (0..n).rev() {
        if lines[i].fenced {
            continue;
        }
        let Some(level) = heading_level(&lines[i].text) else {
            continue;
        };
        let next = (i + 1..n).find(|&j| !drop[j] && (lines[j].fenced || !lines[j].text.trim().is_empty()));
        let empty = match next {
            None => true,
            Some(j) => !lines[j].fenced && heading_level(&lines[j].text).is_some_and(|l| l <= level),
        };
        if empty {
            drop[i] = true;
        }
    }
    retain_unmarked(lines, &drop);
}

// ---- pass 9: blank runs ---------------------------------------------------------------

/// Join lines with at most one blank line between blocks, trailing whitespace
/// trimmed outside fences, no leading or trailing blank lines.
fn join_collapsed(lines: &[Line]) -> String {
    let mut out = String::new();
    let mut started = false;
    let mut pending_blank = false;
    for l in lines {
        let text = if l.fenced {
            l.text.as_str()
        } else {
            l.text.trim_end()
        };
        if text.is_empty() && !l.fenced {
            pending_blank = started;
            continue;
        }
        if started {
            out.push('\n');
        }
        if pending_blank {
            out.push('\n');
            pending_blank = false;
        }
        out.push_str(text);
        started = true;
    }
    out
}

// ---- pass 10: link folding ------------------------------------------------------------------

/// `[text](url)` → `text`, `![alt](src)` → `alt`; links collected into a
/// trailing `Links:` list, deduped by href, at most `MAX_LINKS`, `#` and
/// `javascript:` targets skipped.
fn fold_links(lines: &mut Vec<Line>) {
    let mut seen = HashSet::new();
    let mut links: Vec<String> = Vec::new();
    for l in lines.iter_mut().filter(|l| !l.fenced && l.text.contains("](")) {
        l.text = unwrap_links(&l.text, &mut |label, href| {
            let lower = href.to_ascii_lowercase();
            if href.is_empty()
                || lower.starts_with('#')
                || lower.starts_with("javascript:")
                || links.len() >= MAX_LINKS
                || !seen.insert(href.to_owned())
            {
                return;
            }
            let label = label.trim();
            let label = if label.is_empty() { href } else { label };
            links.push(format!("- {label}: {href}"));
        });
    }
    if links.is_empty() {
        return;
    }
    lines.push(Line {
        text: String::new(),
        fenced: false,
    });
    lines.push(Line {
        text: "Links:".to_owned(),
        fenced: false,
    });
    lines.extend(links.into_iter().map(|text| Line {
        text,
        fenced: false,
    }));
}

/// Replace every `[label](target)` / `![alt](src)` in `text` with its label,
/// innermost first; `on_link(label, href)` is called for links, not images.
fn unwrap_links(text: &str, on_link: &mut impl FnMut(&str, &str)) -> String {
    let mut s = text.to_owned();
    let mut from = 0;
    while let Some(rel) = s[from..].find("](") {
        let mid = from + rel;
        let Some(open) = matching_open(&s[..mid]) else {
            from = mid + 2;
            continue;
        };
        let Some(close_rel) = closing_paren(&s[mid + 2..]) else {
            from = mid + 2;
            continue;
        };
        let close = mid + 2 + close_rel;
        let is_image = open > 0 && s.as_bytes()[open - 1] == b'!';
        let label = s[open + 1..mid].to_owned();
        if !is_image {
            let target = s[mid + 2..close].trim();
            let href = target.split_whitespace().next().unwrap_or_default();
            on_link(&label, href.trim_matches(['<', '>']));
        }
        let start = if is_image { open - 1 } else { open };
        s.replace_range(start..=close, &label);
        from = start;
    }
    s
}

/// Byte index of the `[` that balances the `]` just past `prefix`.
fn matching_open(prefix: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in prefix.char_indices().rev() {
        match c {
            ']' => depth += 1,
            '[' if depth == 0 => return Some(i),
            '[' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Byte index in `rest` of the `)` that balances the `(` just before it.
fn closing_paren(rest: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_lowercase_alphanumerics_with_single_spaces() {
        assert_eq!(fingerprint("  Hello,   World-Wide! 42 "), "hello worldwide 42");
        assert_eq!(fingerprint("---"), "");
    }

    #[test]
    fn fence_state_tracks_openers_and_closers() {
        let lines = split_fenced("a\n```rust\nb\n````\nc\n```\nd\n~~~\ne");
        let fenced: Vec<bool> = lines.iter().map(|l| l.fenced).collect();
        // ```` closes ``` (longer closer ok); `~~~` opens an unclosed fence to the end.
        assert_eq!(fenced, [false, true, true, true, false, true, true, true, true]);
    }

    #[test]
    fn image_only_lines_are_recognised() {
        assert_eq!(image_alt("![Logo](a.png)"), Some("Logo"));
        assert_eq!(image_alt("- [![x](a.png)](https://e.test/)"), Some("x"));
        assert_eq!(image_alt("![a](b.png) caption"), None);
        assert_eq!(image_alt("text"), None);
    }

    #[test]
    fn links_unwrap_innermost_first_with_balanced_parens() {
        let mut seen = Vec::new();
        let out = unwrap_links(
            "see [![alt](i.png)](https://e.test/a_(b)) and [x](#top) ![i](j.png)",
            &mut |l, h| seen.push(format!("{l}={h}")),
        );
        assert_eq!(out, "see alt and x i");
        assert_eq!(seen, ["alt=https://e.test/a_(b)", "x=#top"]);
    }

    #[test]
    fn heading_levels() {
        assert_eq!(heading_level("## Two"), Some(2));
        assert_eq!(heading_level("#"), Some(1));
        assert_eq!(heading_level("#hashtag"), None);
        assert_eq!(heading_level("####### seven"), None);
    }
}
