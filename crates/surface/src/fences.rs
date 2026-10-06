//! Fenced code blocks of a markdown text, found the way CommonMark does.
//!
//! An opening line of three or more `` ` `` or `~` (up to three spaces of indent) with an
//! info string, closed by a line of at least as many of the same character. Fences inside
//! another fenced block are content, not fences.

use std::ops::Range;

/// One fenced block.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fence<'a> {
    /// Trimmed info string.
    pub lang: &'a str,
    /// Everything between the fence lines, trailing newline included; to the end of the
    /// text when unclosed.
    pub body: &'a str,
    /// Start of the opening line to the end of the closing line (its newline excluded).
    pub range: Range<usize>,
    /// Whether a closing line was found.
    pub closed: bool,
}

/// Every fence in `text`, in order.
pub(crate) fn fences(text: &str) -> impl Iterator<Item = Fence<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < lines.len() {
            let (start, line) = lines[i];
            let Some((marker, len, lang)) = opening(line) else {
                i += 1;
                continue;
            };
            let body_start = start + line.len();
            let close = (i + 1..lines.len()).find(|&j| closing(lines[j].1, marker, len));
            return Some(match close {
                Some(j) => {
                    i = j + 1;
                    let (cstart, cline) = lines[j];
                    Fence {
                        lang,
                        body: &text[body_start..cstart],
                        range: start..cstart + cline.trim_end_matches(['\n', '\r']).len(),
                        closed: true,
                    }
                }
                None => {
                    i = lines.len();
                    Fence {
                        lang,
                        body: &text[body_start..],
                        range: start..text.len(),
                        closed: false,
                    }
                }
            });
        }
        None
    })
}

/// Every closed `json-render` fence of `text`, in order, as `(range, body)`:
/// `range` spans the fence lines, `body` is what [`crate::slack_blocks`] and
/// [`crate::flatten`] parse (design doc 110 §8: the caller splits its text here).
pub fn json_render_fences(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    fences(text)
        .filter(|f| f.closed && f.lang == "json-render")
        .map(|f| (f.range, f.body))
}

/// The line with up to three leading spaces removed; `None` if indented further.
fn unindented(line: &str) -> Option<&str> {
    let n = line.bytes().take_while(|b| *b == b' ').count();
    (n <= 3).then_some(&line[n..])
}

/// `(marker, run length, trimmed info string)` if `line` opens a fence.
fn opening(line: &str) -> Option<(u8, usize, &str)> {
    let line = unindented(line)?;
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = line.bytes().take_while(|b| *b == marker).count();
    let rest = &line[len..];
    if len < 3 || (marker == b'`' && rest.contains('`')) {
        return None;
    }
    Some((marker, len, rest.trim()))
}

/// Whether `line` closes a fence opened by `len` × `marker`.
fn closing(line: &str, marker: u8, len: usize) -> bool {
    unindented(line).is_some_and(|l| {
        let n = l.bytes().take_while(|b| *b == marker).count();
        n >= len && l[n..].trim().is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_fence_lines_and_bodies_exclude_them() {
        let text = "a\n```rs\nx\ny\n```\nb\n~~~\nz\n";
        let got: Vec<Fence<'_>> = fences(text).collect();
        assert_eq!(got.len(), 2);
        assert_eq!(&text[got[0].range.clone()], "```rs\nx\ny\n```");
        assert_eq!(
            (got[0].lang, got[0].body, got[0].closed),
            ("rs", "x\ny\n", true)
        );
        assert_eq!(&text[got[1].range.clone()], "~~~\nz\n");
        assert_eq!(
            (got[1].lang, got[1].body, got[1].closed),
            ("", "z\n", false)
        );
    }

    #[test]
    fn nested_fences_are_content() {
        let got: Vec<Fence<'_>> = fences("````md\n```json-render\n{}\n```\n````\n").collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].lang, "md");
        assert!(got[0].closed);
    }
}
