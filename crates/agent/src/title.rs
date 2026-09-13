//! Session titles (design doc 07 §15): a model-written name that tracks what
//! the session is about *now*, not only how it started.
//!
//! The first finished turn replaces the placeholder (the first input, cut to 60
//! chars). Later refreshes are rate-limited by [`due`]: every [`EVERY`] turns or
//! after a compaction, and only when the user has typed enough since the last
//! title to have possibly changed subject. The titler sees a [`digest`] — the
//! current title, the epoch's summary head, the user's asks oldest → newest, and
//! the latest reply — and is told to return the current title verbatim when it
//! still fits, so the list stays stable instead of flickering per turn.

use genai::chat::{ChatMessage, ChatRole};

use crate::loop_::text_of;

/// Turns between refresh attempts.
pub const EVERY: u32 = 4;
/// Substantive user chars since the last title before a refresh is worth a call:
/// a run of "yes" / "continue" / "ok" never re-titles.
pub const MIN_CHARS: usize = 16;
/// Longest stored title.
pub const MAX_CHARS: usize = 60;
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
You write titles for a list of past sessions between a developer and a coding agent. Reply with the title only.

Rules:
- 3 to 7 words, sentence case, under 60 characters. No quotes, no trailing period, no markdown.
- Name the concrete subject: the feature, bug, file, system, or question. Never the activity \
(\"discussing\", \"helping with\", \"working on\") and never the words \"session\", \"chat\", \"conversation\", \"user\", \"assistant\".
- The title says what the session is about now. When the work has moved to a new task, the new task is the title; \
keep the earlier subject only when it is still the theme (e.g. \"Gateway model limits, then session titles\").
- If the current title is still accurate, return it unchanged, character for character, so the list stays stable.
- Greetings or small talk only: say so plainly (e.g. \"Greeting\").";

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
pub fn digest(current: &str, head: Option<&ChatMessage>, history: &[(i64, ChatMessage)]) -> String {
    let mut out = String::new();
    out.push_str("Current title: ");
    out.push_str(if current.is_empty() {
        "(none)"
    } else {
        current
    });
    out.push('\n');
    if let Some(h) = head {
        out.push_str("\nSummary of earlier work in this session:\n");
        out.push_str(&cut(&text_of(&h.content), HEAD_CHARS));
        out.push('\n');
    }
    let asks: Vec<String> = history
        .iter()
        .filter(|(_, m)| m.role == ChatRole::User)
        .map(|(_, m)| text_of(&m.content))
        .filter(|t| !t.trim_start().starts_with('<') && !t.trim().is_empty())
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

/// The title in a model reply: first non-empty line, label/quotes/markdown
/// stripped, whitespace collapsed, at most [`MAX_CHARS`]. `None` when nothing
/// usable is left (the caller keeps the old title).
pub fn clean(reply: &str) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
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
    fn digest_elides_the_middle_and_skips_wrappers() {
        let mut h: Vec<(i64, ChatMessage)> = (0..15)
            .map(|i| (i, ChatMessage::user(format!("ask {i}"))))
            .collect();
        h.push((99, ChatMessage::user("<todo>\n- x\n</todo>")));
        h.push((100, ChatMessage::assistant("Done: shipped it.")));
        let d = digest("Old title", None, &h);
        assert!(d.contains("Current title: Old title"));
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
}
