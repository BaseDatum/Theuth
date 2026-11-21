//! 201 §4 (P2): the verbatim tail — the messages since the last trim,
//! carried as sent — and the trim rule. Pure: the loop hands in its carried
//! history, this module says where to cut.

use genai::chat::{ChatMessage, ChatRole, ContentPart};

/// Tail bytes, measured as the request size is (`compact::est`: text by
/// chars/4, a binary by its flat estimate), in bytes (× 4).
pub fn bytes(history: &[(i64, ChatMessage)]) -> u64 {
    history
        .iter()
        .map(|(_, m)| u64::from(crate::compact::est(m)) * 4)
        .sum()
}

/// A message that asks for tool results (they must stay with it).
fn calls_tools(m: &ChatMessage) -> bool {
    m.content
        .iter()
        .any(|p| matches!(p, ContentPart::ToolCall(_)))
}

/// Indices where a turn starts: a user message (not a tool result) right
/// after an assistant reply that called no tool. Index 0 is a start when it
/// is a user message. A cut there never separates a tool call from its
/// results. (A salvage note after a partial reply also matches; it is the
/// only mid-turn position that does, and it splits no tool call either.)
pub fn turn_starts(history: &[(i64, ChatMessage)]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, (_, m)) in history.iter().enumerate() {
        if m.role != ChatRole::User {
            continue;
        }
        let start = match i.checked_sub(1).map(|p| &history[p].1) {
            None => true,
            Some(prev) => prev.role == ChatRole::Assistant && !calls_tools(prev),
        };
        if start {
            out.push(i);
        }
    }
    out
}

/// What the trim decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trim {
    /// First history index kept (`> 0`).
    pub keep_from: usize,
    /// Bytes cut and kept.
    pub cut: u64,
    pub kept: u64,
}

/// §4.3: trim when the tail is over `high`, or over `low` with a cold cache:
/// cut the oldest whole turns until the tail is ≤ `low`, never the last
/// turn. `None`: no trim (under the marks, or nothing cuttable).
pub fn trim(history: &[(i64, ChatMessage)], low: u64, high: u64, cold: bool) -> Option<Trim> {
    let total = bytes(history);
    if !(total > high || (cold && total > low)) {
        return None;
    }
    let starts = turn_starts(history);
    let last = *starts.last()?;
    // Suffix sizes from each start; the first start whose suffix fits `low`,
    // else the last turn alone.
    let keep_from = starts
        .iter()
        .copied()
        .find(|&s| s > 0 && bytes(&history[s..]) <= low)
        .unwrap_or(last);
    if keep_from == 0 {
        return None;
    }
    let kept = bytes(&history[keep_from..]);
    Some(Trim {
        keep_from,
        cut: total - kept,
        kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::{ToolCall, ToolResponse};

    fn user(s: &str) -> ChatMessage {
        ChatMessage::user(s.to_owned())
    }
    fn reply(s: &str) -> ChatMessage {
        ChatMessage::assistant(s.to_owned())
    }
    fn call(id: &str) -> ChatMessage {
        ChatMessage::from(vec![ToolCall {
            call_id: id.into(),
            fn_name: "bash".into(),
            fn_arguments: serde_json::json!({"command": "ls"}),
            thought_signatures: None,
        }])
    }
    fn result(id: &str, s: &str) -> ChatMessage {
        ChatMessage::from(vec![ToolResponse::new(id, s)])
    }
    fn script() -> Vec<(i64, ChatMessage)> {
        let big = "x".repeat(4_000);
        let mut v = vec![
            user("t0"),
            reply(&big),
            user("t1"),
            call("c1"),
            result("c1", &big),
            reply("done"),
            user("t2"),
            reply(&big),
            user("t3"),
            reply(&big),
            user("t4"),
            reply(&big),
        ];
        v.drain(..)
            .enumerate()
            .map(|(i, m)| (i as i64, m))
            .collect()
    }

    #[test]
    fn a_trim_cuts_whole_turns_and_keeps_tool_calls_with_results() {
        let h = script();
        assert_eq!(turn_starts(&h), [0, 2, 6, 8, 10]);
        // Over high: cut to ≤ low on a turn start.
        let t = trim(&h, 9_000, 15_000, false).expect("over high");
        assert!(turn_starts(&h).contains(&t.keep_from));
        assert!(t.kept <= 9_000, "{t:?}");
        assert_eq!(t.cut + t.kept, bytes(&h));
        assert_eq!(t.keep_from, 8);
        // Never between a tool call and its result.
        for low in [1_000, 5_000, 9_000, 13_000] {
            let t = trim(&h, low, 15_000, false).unwrap();
            assert!(![4, 5].contains(&t.keep_from));
        }
    }

    #[test]
    fn the_last_turn_is_always_kept() {
        let h = script();
        let t = trim(&h, 100, 200, false).unwrap();
        assert_eq!(t.keep_from, 10, "the last turn stays even past the marks");
        // One turn only: nothing to cut.
        assert_eq!(trim(&h[10..], 100, 200, false), None);
    }

    #[test]
    fn a_cold_cache_trims_early_and_nothing_trims_otherwise() {
        let h = script();
        let total = bytes(&h);
        assert_eq!(
            trim(&h, total - 1, total + 1, false),
            None,
            "under high, warm"
        );
        let t = trim(&h, total - 1, total + 1, true).expect("cold over low");
        assert!(t.kept <= total - 1);
        assert_eq!(trim(&h, total, total + 1, true), None, "cold but under low");
    }
}
