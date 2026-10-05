//! Plain-text fallback and chunking for channels without rich text (design
//! doc 98 §1 seam table): [`plain`] and [`chunks`] are copies of
//! `crates/signal/src/text.rs` `markdown_to_plain` / `chunk_text` (frozen
//! protocol code there; edit this copy only).

/// Converts markdown-lite to plain text.
///
/// - Code fences (``` / ~~~) and their contents are kept verbatim.
/// - ATX headers lose their `#` prefix.
/// - `**bold**`, `__bold__`, `*em*`, `_em_` markers are stripped; list
///   bullets (`* item`, `- item`) are preserved.
/// - Links `[text](url)` become `text (url)`.
/// - Inline backticks are stripped (contents kept).
#[must_use]
pub fn plain(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    for (i, line) in md.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&plain_line(line));
    }
    if md.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn plain_line(line: &str) -> String {
    let line = strip_header(line);
    let line = replace_links(&line);
    let line = line.replace("**", "").replace("__", "");
    let line = strip_pair_emphasis(&line, '*');
    let line = strip_pair_emphasis(&line, '_');
    line.replace('`', "")
}

/// Removes a leading ATX header marker (`#{1,6} `), keeping indentation.
fn strip_header(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes)
        && let Some(text) = rest[hashes..].strip_prefix(' ')
    {
        return format!("{indent}{text}");
    }
    line.to_string()
}

/// Rewrites `[text](url)` (and `![alt](url)`) as `text (url)`.
fn replace_links(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' || (chars[i] == '!' && chars.get(i + 1) == Some(&'[')) {
            let open = if chars[i] == '!' { i + 1 } else { i };
            if let Some((text, url, end)) = parse_link(&chars, open) {
                out.push_str(&text);
                out.push_str(" (");
                out.push_str(&url);
                out.push(')');
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Parses `[text](url)` starting at the `[` index; returns
/// `(text, url, index-past-close-paren)`.
fn parse_link(chars: &[char], open: usize) -> Option<(String, String, usize)> {
    let close = (open + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let paren = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    let text: String = chars[open + 1..close].iter().collect();
    let url: String = chars[close + 2..paren].iter().collect();
    Some((text, url, paren + 1))
}

/// Strips paired emphasis markers (`*` / `_`) at word boundaries. A marker
/// opens when preceded by start/whitespace/open-bracket and followed by a
/// non-space; it closes when preceded by a non-space and followed by
/// end/whitespace/punctuation. List bullets (`* ` at line start) and
/// intra-word underscores (`snake_case`) are untouched.
fn strip_pair_emphasis(line: &str, marker: char) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut remove = vec![false; chars.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c != marker {
            continue;
        }
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };
        let next = chars.get(i + 1).copied();
        let can_close = !stack.is_empty()
            && prev.is_some_and(|p| !p.is_whitespace() && p != marker)
            && next.is_none_or(|n| !n.is_alphanumeric());
        if can_close {
            if let Some(open) = stack.pop() {
                remove[open] = true;
                remove[i] = true;
            }
            continue;
        }
        let can_open = prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '[' | '{'))
            && next.is_some_and(|n| !n.is_whitespace());
        if can_open {
            stack.push(i);
        }
    }
    chars
        .iter()
        .enumerate()
        .filter(|&(i, _)| !remove[i])
        .map(|(_, &c)| c)
        .collect()
}

/// Splits `text` into chunks of at most `max` characters, preferring to
/// break at a newline, then at whitespace, then hard-cutting. Returns an
/// empty vec for empty input.
#[must_use]
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    debug_assert!(max > 0);
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= max {
            chunks.push(chars[start..].iter().collect());
            break;
        }
        let window = &chars[start..start + max + 1];
        // A separator at index <= max lets the chunk fit; prefer the last
        // newline in the window, then the last whitespace.
        let cut = window
            .iter()
            .rposition(|&c| c == '\n')
            .or_else(|| window.iter().rposition(|c| c.is_whitespace()));
        match cut {
            Some(pos) if pos > 0 => {
                chunks.push(chars[start..start + pos].iter().collect());
                start += pos + 1;
            }
            _ => {
                chunks.push(chars[start..start + max].iter().collect());
                start += max;
            }
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 2000;

    #[test]
    fn strips_bold_italic_headers_and_links() {
        let md = "# Title\n\nThis is **bold** and *emphasis* and __also bold__.\nSee [docs](https://example.com).";
        assert_eq!(
            plain(md),
            "Title\n\nThis is bold and emphasis and also bold.\nSee docs (https://example.com)."
        );
    }

    #[test]
    fn keeps_lists_and_code_fences() {
        let md = "Steps:\n- first\n* second\n```rust\nlet x = 1; // **not stripped**\n```\ndone";
        assert_eq!(
            plain(md),
            "Steps:\n- first\n* second\n```rust\nlet x = 1; // **not stripped**\n```\ndone"
        );
    }

    #[test]
    fn keeps_snake_case_identifiers() {
        assert_eq!(plain("use foo_bar_baz here"), "use foo_bar_baz here");
    }

    #[test]
    fn strips_inline_backticks() {
        assert_eq!(plain("run `cargo test` now"), "run cargo test now");
    }

    #[test]
    fn chunk_exact_boundary_is_single_chunk() {
        let text = "a".repeat(2000);
        let out = chunks(&text, MAX);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chars().count(), 2000);
    }

    #[test]
    fn chunk_one_over_boundary_splits() {
        let text = "a".repeat(2001);
        let out = chunks(&text, MAX);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].chars().count(), 2000);
        assert_eq!(out[1].chars().count(), 1);
    }

    #[test]
    fn chunk_prefers_newline_then_space() {
        let text = format!("{}\n{}", "a".repeat(1500), "b".repeat(1000));
        let out = chunks(&text, MAX);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "a".repeat(1500));
        assert_eq!(out[1], "b".repeat(1000));

        let text = format!("{} {}", "a".repeat(1500), "b".repeat(1000));
        let out = chunks(&text, MAX);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "a".repeat(1500));
        assert_eq!(out[1], "b".repeat(1000));
    }

    #[test]
    fn chunk_counts_chars_not_bytes() {
        // 4-byte scalar values: 600 of them exceed 2000 bytes but are
        // only 600 chars — must stay a single chunk.
        let text = "🦀".repeat(600);
        assert_eq!(chunks(&text, MAX).len(), 1);
        let text = "🦀".repeat(2500);
        let out = chunks(&text, MAX);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.chars().count() <= 2000));
    }

    #[test]
    fn chunk_empty_is_empty() {
        assert!(chunks("", MAX).is_empty());
    }
}
