//! Body → content for `read_url` (design doc 16 §5, §8): one HTML parser
//! (`html_to_markdown_rs`) produces markdown; `text` is a plain projection of
//! that markdown, so both formats see the same noise removal.

use html_to_markdown_rs::{
    ConversionOptions, PreprocessingOptions, PreprocessingPreset, TierStrategy, convert,
};

/// Elements dropped with all their descendants before conversion.
const EXCLUDE: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "iframe", "canvas", "object", "embed",
    "form", "nav", "footer", "aside",
];

/// HTML → markdown with page chrome removed and images skipped.
#[must_use]
pub(super) fn html_to_markdown(html: &str) -> String {
    let options = ConversionOptions {
        extract_metadata: false,
        skip_images: true,
        exclude_selectors: EXCLUDE.iter().map(|s| (*s).to_owned()).collect(),
        preprocessing: PreprocessingOptions {
            preset: PreprocessingPreset::Aggressive,
            ..PreprocessingOptions::default()
        },
        // Tier-1 (byte scanner) does not honour `exclude_selectors`.
        tier_strategy: TierStrategy::Tier2,
        ..ConversionOptions::default()
    };
    match convert(html, options) {
        Ok(result) => result.content.unwrap_or_default(),
        Err(e) => {
            tracing::debug!(error = %e, "html conversion failed; returning nothing");
            String::new()
        }
    }
}

/// Plain-text projection of markdown: headings lose `#`, `[t](u)` → `t`,
/// `*`/`**` emphasis markers go, rules vanish, blank runs collapse.
///
/// ponytail: line-local heuristics, no markdown parser; upgrade to a real
/// parser if code blocks containing `[`/`*` turn out to matter.
#[must_use]
pub(super) fn markdown_to_text(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut blank = true; // suppress leading and repeated blank lines
    for raw in md.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            if !blank {
                out.push('\n');
                blank = true;
            }
            continue;
        }
        if is_rule(trimmed) {
            continue;
        }
        let body = trimmed.trim_start_matches('#');
        let line = if body.len() != trimmed.len() && body.starts_with(' ') {
            body.trim_start()
        } else {
            line
        };
        push_inline(&unwrap_links(line), &mut out);
        out.push('\n');
        blank = false;
    }
    out.truncate(out.trim_end().len());
    out
}

/// A thematic break: three or more of the same `-`, `*`, `_` or `=`, spaces allowed.
fn is_rule(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(first) = compact.chars().next() else {
        return false;
    };
    compact.len() >= 3
        && matches!(first, '-' | '*' | '_' | '=')
        && compact.chars().all(|c| c == first)
}

/// `[text](url)` → `text`; unbalanced brackets are left alone.
fn unwrap_links(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](") else {
            break;
        };
        let Some(close) = rest[open + mid..].find(')') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..open + mid]);
        rest = &rest[open + mid + close + 1..];
    }
    out.push_str(rest);
    out
}

/// Copy `line`, dropping `*` unless it is a bullet or a spaced-out operator
/// (space on both sides).
fn push_inline(line: &str, out: &mut String) {
    let bytes = line.as_bytes();
    for (i, ch) in line.char_indices() {
        if ch == '*' {
            let before = i == 0 || bytes[i - 1] == b' ';
            let after = bytes.get(i + 1).is_none_or(|b| *b == b' ');
            if !(before && after) {
                continue;
            }
        }
        out.push(ch);
    }
}

/// `<title>` text by a tiny scan (no parser); `None` when absent or empty.
#[must_use]
pub(super) fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = collapse_ws(&html[start..end]);
    (!title.is_empty()).then_some(title)
}

/// First `# heading` line of `md`, sans marker.
#[must_use]
pub(super) fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .find_map(|l| l.trim_start().strip_prefix("# "))
        .map(collapse_ws)
        .filter(|t| !t.is_empty())
}

/// Collapse whitespace runs to single spaces and trim.
#[must_use]
pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to at most `max_chars` scalars; on overflow the last kept char
/// becomes `…` so the result is exactly `max_chars` long. Returns the text and
/// whether it was cut.
#[must_use]
pub fn hard_truncate(s: &str, max_chars: usize) -> (String, bool) {
    if s.chars().count() <= max_chars {
        return (s.to_owned(), false);
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_projection_strips_markup() {
        let md = "# Title\n\n\nSome **bold** and *em* text, 2 * 3.\n\n* bullet\n- [Docs](https://x.y/docs) here\n\n---\n";
        assert_eq!(
            markdown_to_text(md),
            "Title\n\nSome bold and em text, 2 * 3.\n\n* bullet\n- Docs here"
        );
    }

    #[test]
    fn title_scan_and_truncate() {
        assert_eq!(
            html_title("<html><head><title lang=x>\n A  B </title>").as_deref(),
            Some("A B")
        );
        assert_eq!(html_title("<p>no title</p>"), None);
        assert_eq!(hard_truncate("abc", 2), ("a…".into(), true));
        assert_eq!(hard_truncate("abc", 3), ("abc".into(), false));
    }
}
