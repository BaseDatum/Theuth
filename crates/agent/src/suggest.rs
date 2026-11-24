//! "What's next?" suggestions: after an interactive chat's turn ends normally, the
//! title model proposes up to [`MAX_ITEMS`] short next steps phrased as the user
//! would type them. A detached side call on the turn's last ask and final reply
//! ([`digest`]), published as [`crate::AgentEvent::Suggestions`]; never a
//! transcript row, never persisted. The reply format is loose on purpose:
//! [`parse`] takes lines, numbered lists, bullets or a JSON array, and `NONE`
//! (or nothing) means no suggestions.

use genai::chat::{ChatMessage, ChatRole};

use crate::loop_::text_of;

/// Most suggestions kept.
pub const MAX_ITEMS: usize = 3;
/// Longest suggestion kept, in chars (the prompt asks for under 60); longer ones
/// are cut at a word boundary, anything past [`DROP_CHARS`] is prose and dropped.
pub const ITEM_CHARS: usize = 80;
/// A "suggestion" this long is an explanation, not something to send.
const DROP_CHARS: usize = 200;
/// Last-ask cap in the digest.
const ASK_CHARS: usize = 600;
/// Final-reply cap in the digest: the reply's end, where next steps are offered.
const REPLY_CHARS: usize = 1500;
/// Reply token cap for the call: three short lines with room for a preamble.
pub const MAX_TOKENS: u32 = 300;

/// System prompt for the suggestions call.
pub const SYSTEM: &str = "\
You suggest what the user could send next in a chat with an AI assistant. You see the user's last message and the assistant's final reply.

Reply with 2 or 3 next steps, one per line, nothing else: no numbering, no quotes, no preamble.
- Write each as the user would type it to the assistant: an imperative request, specific to this conversation (\"Add a test for the empty case\", \"Show me the diff\").
- Under 60 characters each. No questions to the user, no restating what was already done.
- Prefer what the reply offered or left open; never suggest something unrelated.
If there is no sensible next step (small talk, a finished answer, a goodbye), reply with exactly NONE.";

/// What the suggester sees: the latest user-typed message of `history` and the
/// final assistant reply after it. `None` when either is missing (a turn opened
/// by a harness wrapper only, or a reply with no text) — no call is worth it.
pub fn digest(history: &[(i64, ChatMessage)]) -> Option<String> {
    let at = history.iter().rposition(|(_, m)| {
        m.role == ChatRole::User && {
            let t = crate::clock::strip(&text_of(&m.content)).trim().to_owned();
            !t.is_empty() && !t.starts_with('<')
        }
    })?;
    let ask = crate::clock::strip(&text_of(&history[at].1.content)).to_owned();
    let p = crate::attach::parse(&ask);
    let ask = if p.refs.is_empty() {
        ask
    } else {
        let titles: Vec<String> = p.refs.iter().map(|r| format!("\"{}\"", r.title)).collect();
        format!("(attached: {}) {}", titles.join(", "), p.body.trim())
    };
    let reply = history[at + 1..]
        .iter()
        .rev()
        .filter(|(_, m)| m.role == ChatRole::Assistant)
        .map(|(_, m)| m.content.texts().concat())
        .find(|t| !t.trim().is_empty())?;
    Some(format!(
        "User's last message:\n{}\n\nAssistant's final reply:\n{}\n",
        head(ask.trim(), ASK_CHARS),
        tail(reply.trim(), REPLY_CHARS),
    ))
}

/// The suggestions in a model reply, at most [`MAX_ITEMS`], deduplicated
/// (case-insensitively), each cut to [`ITEM_CHARS`]. Takes a JSON array of
/// strings (bare, fenced, or as an object's `suggestions`), else one per line
/// with list markers, quotes and markdown stripped; preamble lines (ending in
/// `:`) and `NONE` are skipped. Empty for an empty or `NONE` reply.
pub fn parse(reply: &str) -> Vec<String> {
    let raw: Vec<String> = json_items(reply).unwrap_or_else(|| {
        reply
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("```") && !l.ends_with(':'))
            .map(strip_marker)
            .map(str::to_owned)
            .collect()
    });
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let quotes: &[char] = &['"', '\'', '`', '*', '_', '“', '”', '‘', '’'];
        let words: Vec<&str> = item
            .trim()
            .trim_matches(quotes)
            .split_whitespace()
            .collect();
        let item = words.join(" ");
        let bare = item.trim_matches(|c: char| !c.is_alphanumeric());
        if bare.is_empty() || bare.eq_ignore_ascii_case("none") || item.chars().count() > DROP_CHARS
        {
            continue;
        }
        let item = fit(&item, ITEM_CHARS);
        if out.iter().any(|o| o.to_lowercase() == item.to_lowercase()) {
            continue;
        }
        out.push(item);
        if out.len() == MAX_ITEMS {
            break;
        }
    }
    out
}

/// A JSON array of strings in `reply` (possibly fenced or under `suggestions`).
fn json_items(reply: &str) -> Option<Vec<String>> {
    let start = reply.find(['[', '{'])?;
    let end = reply.rfind([']', '}'])?;
    let v: serde_json::Value = serde_json::from_str(reply.get(start..=end)?).ok()?;
    let arr = match &v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(o) => o.values().find_map(|v| v.as_array())?,
        _ => return None,
    };
    arr.iter().map(|v| v.as_str().map(str::to_owned)).collect()
}

/// `line` without a leading `-`, `*`, `•`, `1.`, `1)` or `(1)` marker.
fn strip_marker(line: &str) -> &str {
    let l = line.trim_start_matches(['-', '*', '•', '·', '–', ' ']);
    let digits = l.len() - l.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = &l[digits..];
    if digits > 0
        && let Some(r) = rest.strip_prefix(['.', ')', ':'])
    {
        return r.trim_start();
    }
    if let Some(r) = l.strip_prefix('(')
        && let Some((n, r)) = r.split_once(')')
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        return r.trim_start();
    }
    l
}

/// At most `max` chars of `s`, cut at a word boundary when it is longer (the
/// text is sent as typed, so no ellipsis).
fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    for w in s.split_whitespace() {
        if out.chars().count() + 1 + w.chars().count() > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        out = s.chars().take(max).collect();
    }
    out.trim_end_matches([',', ';', ':']).to_owned()
}

/// Char-boundary prefix with `…` when cut.
fn head(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let end = s.floor_char_boundary(max.saturating_sub(1));
    format!("{}…", &s[..end])
}

/// Char-boundary suffix with a leading `…` when cut.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let start = s.ceil_char_boundary(s.len() - max + 1);
    format!("…{}", &s[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_lines() {
        assert_eq!(
            parse("Add a test for the empty case\nShow me the diff\n"),
            ["Add a test for the empty case", "Show me the diff"]
        );
    }

    #[test]
    fn parse_numbered_and_bulleted_lists() {
        let want = ["Run the tests", "Commit the change", "Open a PR"];
        assert_eq!(
            parse("1. Run the tests\n2) Commit the change\n(3) Open a PR"),
            want
        );
        assert_eq!(
            parse("- Run the tests\n* Commit the change\n• Open a PR"),
            want
        );
        assert_eq!(
            parse(
                "Here are some next steps:\n\n1. **Run the tests**\n2. \"Commit the change\"\n3. `Open a PR`"
            ),
            want
        );
    }

    #[test]
    fn parse_json_bare_fenced_and_keyed() {
        let want = ["Run the tests", "Commit the change"];
        assert_eq!(parse(r#"["Run the tests", "Commit the change"]"#), want);
        assert_eq!(
            parse("```json\n[\"Run the tests\", \"Commit the change\"]\n```"),
            want
        );
        assert_eq!(
            parse(r#"{"suggestions": ["Run the tests", "Commit the change"]}"#),
            want
        );
        assert!(parse("[]").is_empty());
    }

    #[test]
    fn parse_empty_and_none() {
        assert!(parse("").is_empty());
        assert!(parse("  \n\n").is_empty());
        assert!(parse("NONE").is_empty());
        assert!(parse("none.").is_empty());
        assert!(parse("- None").is_empty());
    }

    #[test]
    fn parse_caps_dedupes_and_trims() {
        let got = parse("a one\nA one\nb two\nc three\nd four");
        assert_eq!(got, ["a one", "b two", "c three"]);
        let long = parse(&format!("{}\nShort one", "word ".repeat(20)));
        assert!(
            long[0].chars().count() <= ITEM_CHARS && !long[0].ends_with(' '),
            "{long:?}"
        );
        assert_eq!(long[1], "Short one");
        // Prose, not a suggestion.
        assert_eq!(
            parse(&format!("{}\nShort one", "x".repeat(300))),
            ["Short one"]
        );
    }

    #[test]
    fn digest_takes_last_typed_ask_and_final_reply() {
        let history = vec![
            (1, ChatMessage::user("first ask")),
            (2, ChatMessage::assistant("first reply")),
            (3, ChatMessage::user("fix the parser")),
            (4, ChatMessage::assistant("looking")),
            (5, ChatMessage::user("<todo>reminder</todo>")),
            (6, ChatMessage::assistant("Fixed it. Want a test too?")),
        ];
        let d = digest(&history).unwrap();
        assert!(d.contains("fix the parser"), "{d}");
        assert!(d.contains("Want a test too?"), "{d}");
        assert!(
            !d.contains("first") && !d.contains("<todo>") && !d.contains("looking"),
            "{d}"
        );
        // No reply after the ask, or no typed ask at all: nothing to suggest from.
        assert!(digest(&[(1, ChatMessage::user("hi"))]).is_none());
        assert!(
            digest(&[
                (1, ChatMessage::user("<schedule>x</schedule>")),
                (2, ChatMessage::assistant("ok"))
            ])
            .is_none()
        );
        // Long replies keep their end, where next steps are offered.
        let long = format!("{}END", "x".repeat(5000));
        let d = digest(&[
            (1, ChatMessage::user("go")),
            (2, ChatMessage::assistant(long)),
        ])
        .unwrap();
        assert!(
            d.len() < 2500 && d.trim_end().ends_with("END"),
            "{}",
            d.len()
        );
    }
}
