//! Session titles (design doc 07 §15): a model-written name that tracks what
//! the session is about *now*, not only how it started.
//!
//! The first finished turn replaces the placeholder (the first input line after
//! any attachment markers, cut to 60 chars — `attach::seed_title`). Later refreshes are rate-limited by [`due`]: every [`EVERY`] turns or
//! after a compaction, and only when the user has typed enough since the last
//! title to have possibly changed subject. The titler sees a [`digest`] — the
//! current title and tags, the epoch's summary head, the user's asks oldest →
//! newest, and the latest reply — and is told to return the current title
//! verbatim when it still fits, so the list stays stable instead of flickering
//! per turn.
//!
//! The same call assigns [`tags`]: one to [`MAX_TAGS`] lowercase keywords on a
//! `tags:` line after the title, so `discover_peers` (50 §5.1 as-built) can
//! show what each of many sessions is about in a few words. A reply without
//! the line leaves the tags empty; a reply without a usable title changes nothing.

use genai::chat::{ChatMessage, ChatRole};

use crate::loop_::text_of;

/// Turns between refresh attempts.
pub const EVERY: u32 = 4;
/// Substantive user chars since the last title before a refresh is worth a call:
/// a run of "yes" / "continue" / "ok" never re-titles.
pub const MIN_CHARS: usize = 16;
/// Longest stored title.
pub const MAX_CHARS: usize = 60;
/// Most tags kept per session.
pub const MAX_TAGS: usize = 4;
/// Longest tag, in chars.
pub const TAG_CHARS: usize = 24;
/// Per-message cap in the digest.
const MSG_CHARS: usize = 240;
/// Head-summary cap in the digest.
const HEAD_CHARS: usize = 1200;
/// Latest-reply cap in the digest.
const REPLY_CHARS: usize = 400;
/// Digest keeps the first `KEEP_HEAD` and last `KEEP_TAIL` user messages.
const KEEP_HEAD: usize = 2;
const KEEP_TAIL: usize = 8;

/// System prompt for the title call.
pub const SYSTEM: &str = "\
You write titles and tags for a list of past sessions between a developer and a coding agent. \
Reply with exactly two lines: the title, then `tags: a, b, c`.

Title rules:
- 3 to 7 words, sentence case, under 60 characters. No quotes, no trailing period, no markdown.
- Name the concrete subject: the feature, bug, file, system, or question. Never the activity \
(\"discussing\", \"helping with\", \"working on\") and never the words \"session\", \"chat\", \"conversation\", \"user\", \"assistant\".
- The title says what the session is about now. When the work has moved to a new task, the new task is the title; \
keep the earlier subject only when it is still the theme (e.g. \"Gateway model limits, then session titles\").
- If the current title is still accurate, return it unchanged, character for character, so the list stays stable.
- Greetings or small talk only: say so plainly (e.g. \"Greeting\").

Tag rules:
- 1 to 4 tags, comma-separated, lowercase, letters/digits/hyphens only, one or two words each (e.g. `rust`, `desktop-ui`, `meeting`, `research`, `merge`).
- Name the project, language, area and kind of work, so someone scanning forty sessions can tell which are related to theirs.
- Keep the current tags that still apply; change them only when the subject moved.";

/// Whether a title call is worth making after a finished turn.
/// `first`: no model-written title yet (placeholder only) — any user text earns one.
pub fn due(first: bool, turns_since: u32, chars_since: usize, epoch_changed: bool) -> bool {
    if first {
        return chars_since > 0;
    }
    chars_since >= MIN_CHARS && (turns_since >= EVERY || epoch_changed)
}

/// Chars of `inputs` that are user-typed (harness wrappers start with `<`).
pub fn user_chars<'a>(texts: impl Iterator<Item = &'a str>) -> usize {
    texts
        .map(str::trim)
        .filter(|t| !t.starts_with('<'))
        .map(str::len)
        .sum()
}

/// What the titler sees. `history` is the current epoch; `head` its summary.
pub fn digest(
    current: &str,
    tags: &[String],
    head: Option<&ChatMessage>,
    history: &[(i64, ChatMessage)],
) -> String {
    let mut out = String::new();
    out.push_str("Current title: ");
    out.push_str(if current.is_empty() {
        "(none)"
    } else {
        current
    });
    out.push_str("\nCurrent tags: ");
    if tags.is_empty() {
        out.push_str("(none)");
    } else {
        out.push_str(&tags.join(", "));
    }
    out.push('\n');
    if let Some(h) = head {
        out.push_str("\nSummary of earlier work in this session:\n");
        out.push_str(&cut(&text_of(&h.content), HEAD_CHARS));
        out.push('\n');
    }
    // 66 §8: a leading marker line is a reference, never the subject's
    // spelling; the titler sees the attachment by its title.
    let asks: Vec<String> = history
        .iter()
        .filter(|(_, m)| m.role == ChatRole::User)
        .map(|(_, m)| text_of(&m.content))
        .filter(|t| !t.trim_start().starts_with('<') && !t.trim().is_empty())
        .map(|t| {
            let p = crate::attach::parse(&t);
            if p.refs.is_empty() {
                return t;
            }
            let titles: Vec<String> = p.refs.iter().map(|r| format!("\"{}\"", r.title)).collect();
            format!("(attached: {}) {}", titles.join(", "), p.body.trim())
        })
        .collect();
    out.push_str("\nWhat the developer asked, oldest first:\n");
    let n = asks.len();
    for (i, ask) in asks.iter().enumerate() {
        if n > KEEP_HEAD + KEEP_TAIL && i >= KEEP_HEAD && i < n - KEEP_TAIL {
            if i == KEEP_HEAD {
                out.push_str(&format!("(… {} more …)\n", n - KEEP_HEAD - KEEP_TAIL));
            }
            continue;
        }
        out.push_str("- ");
        out.push_str(&cut(ask.trim(), MSG_CHARS).replace('\n', " "));
        out.push('\n');
    }
    let reply = history
        .iter()
        .rev()
        .filter(|(_, m)| m.role == ChatRole::Assistant)
        .map(|(_, m)| m.content.texts().concat())
        .find(|t| !t.trim().is_empty());
    if let Some(r) = reply {
        out.push_str("\nLatest reply from the agent:\n");
        out.push_str(&cut(r.trim(), REPLY_CHARS));
        out.push('\n');
    }
    out
}

/// A `tags:` line (either case), the rest after the label.
fn tags_line(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(['*', '_', '`', '#', ' ']);
    rest.strip_prefix("tags:")
        .or_else(|| rest.strip_prefix("Tags:"))
        .or_else(|| rest.strip_prefix("TAGS:"))
}

/// The title in a model reply: first non-empty line that is not the `tags:`
/// line, label/quotes/markdown stripped, whitespace collapsed, at most
/// [`MAX_CHARS`]. `None` when nothing usable is left (the caller keeps the
/// old title).
pub fn clean(reply: &str) -> Option<String> {
    let line = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && tags_line(l).is_none())?;
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .unwrap_or(line);
    let strip: &[char] = &['"', '\'', '`', '*', '_', '#', ' ', '.', '“', '”', '‘', '’'];
    let words: Vec<&str> = line.trim_matches(strip).split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let mut out = String::new();
    for w in words {
        if !out.is_empty() && out.len() + 1 + w.len() > MAX_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.len() > MAX_CHARS {
        out = cut(&out, MAX_CHARS);
    }
    Some(out)
}

/// The tags in a model reply: the first `tags:` line split on commas, each
/// lowercased, runs of anything outside `[a-z0-9]` folded to one `-` and
/// trimmed, cut to [`TAG_CHARS`], deduplicated, at most [`MAX_TAGS`]. Empty
/// when the line is missing or holds nothing usable.
pub fn tags(reply: &str) -> Vec<String> {
    let Some(line) = reply.lines().find_map(tags_line) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for raw in line.split(',') {
        let mut tag = String::new();
        for c in raw.trim().trim_matches(['`', '"', '\'', '[', ']', '#']).chars() {
            match c.to_ascii_lowercase() {
                c if c.is_ascii_alphanumeric() => tag.push(c),
                _ if !tag.is_empty() && !tag.ends_with('-') => tag.push('-'),
                _ => {}
            }
        }
        let tag: String = tag.trim_end_matches('-').chars().take(TAG_CHARS).collect();
        let tag = tag.trim_end_matches('-').to_owned();
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
            if out.len() == MAX_TAGS {
                break;
            }
        }
    }
    out
}

/// Char-boundary prefix with `…` when cut.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max.saturating_sub(1);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_first_title_then_rate_limited() {
        assert!(due(true, 1, 2, false), "any text earns the first title");
        assert!(!due(true, 1, 0, false), "no text, no call");
        assert!(!due(false, 1, 500, false), "too soon");
        assert!(!due(false, EVERY, 3, false), "\"ok\" x4 never re-titles");
        assert!(due(false, EVERY, MIN_CHARS, false));
        assert!(due(false, 1, MIN_CHARS, true), "compaction moved the arc");
    }

    #[test]
    fn clean_strips_labels_quotes_and_caps() {
        assert_eq!(
            clean("Title: \"Gateway model limits.\"\n").as_deref(),
            Some("Gateway model limits")
        );
        assert_eq!(
            clean("**Session titles**").as_deref(),
            Some("Session titles")
        );
        assert_eq!(clean("\n\n  \n"), None);
        let long = clean(&"word ".repeat(30)).unwrap();
        assert!(long.len() <= MAX_CHARS && !long.ends_with(' '), "{long:?}");
        assert_eq!(clean("Ünïcödé títle").as_deref(), Some("Ünïcödé títle"));
    }

    #[test]
    fn tags_parse_normalise_dedupe_and_cap() {
        let reply = "Merge design 62\ntags: Theuth, Rust, desktop UI, theuth, C++, meeting, extra\n";
        assert_eq!(clean(reply).as_deref(), Some("Merge design 62"));
        assert_eq!(tags(reply), ["theuth", "rust", "desktop-ui", "c"]);
        assert_eq!(tags("**Tags:** `merge`, \"research\""), ["merge", "research"]);
        assert_eq!(tags("Just a title"), Vec::<String>::new());
        assert_eq!(tags("tags: , --, ***"), Vec::<String>::new());
        let long = tags(&format!("tags: {}", "a".repeat(40)));
        assert_eq!(long[0].len(), TAG_CHARS);
        // A reply that is only a tags line has no usable title.
        assert_eq!(clean("tags: rust"), None);
        assert_eq!(clean("\ntags: rust\nReal title"), Some("Real title".into()));
    }

    #[test]
    fn digest_elides_the_middle_and_skips_wrappers() {
        let mut h: Vec<(i64, ChatMessage)> = (0..15)
            .map(|i| (i, ChatMessage::user(format!("ask {i}"))))
            .collect();
        h.push((99, ChatMessage::user("<todo>\n- x\n</todo>")));
        h.push((100, ChatMessage::assistant("Done: shipped it.")));
        let d = digest("Old title", &["rust".to_owned()], None, &h);
        assert!(d.contains("Current title: Old title\nCurrent tags: rust\n"), "{d}");
        assert!(
            d.contains("- ask 0\n- ask 1\n(… 5 more …)\n- ask 7\n"),
            "{d}"
        );
        assert!(d.contains("- ask 14\n"));
        assert!(!d.contains("<todo>"));
        assert!(d.contains("Latest reply from the agent:\nDone: shipped it."));
        assert_eq!(
            user_chars(["  hi ", "<message from=\"x\">y</message>"].into_iter()),
            2
        );
    }

    /// 66 §8: the titler is shown an attachment by its title, never the marker line.
    #[test]
    fn digest_names_attachments_by_title() {
        let h = vec![(
            0,
            ChatMessage::user("[attached note abc \"Groceries\" · 1 KB]\n[doc knowledge:facts/x \"Relay timeouts\"]\nwhat's missing?"),
        )];
        let d = digest("", &[], None, &h);
        assert!(d.contains("- (attached: \"Groceries\", \"Relay timeouts\") what's missing?\n"), "{d}");
        assert!(!d.contains("[attached") && !d.contains("abc"), "{d}");
    }
}
