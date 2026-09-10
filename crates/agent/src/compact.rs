//! Compaction: cut rules, summariser instruction, overflow classifier (design doc 07 §8).

use genai::chat::{ChatMessage, ChatRole};

/// Appended as one user message to the live frame; the summary replaces
/// everything above it in the next epoch (07 §8.3).
pub const COMPACT_INSTRUCTION: &str = "\
The conversation is about to be compacted: everything above this message will be replaced by
your summary, and only the most recent messages will be kept verbatim. Write the summary now.
Reply with text only — do not call tools. Use exactly these headings and short bullets:

## Goal
## Done  (facts established, files changed, commands run and their outcomes)
## Open  (in-progress work, unanswered questions, pending approvals)
## Next  (the immediate next step)
## Notes (user preferences, constraints, names and paths to remember, runtime variables you rely on and what they hold)

If a previous summary is present at the top of the conversation, fold it in: keep what is still
true, drop what is finished. Be specific — paths, identifiers, exact error text — and omit
chit-chat and tool-call boilerplate.";

/// Substrings (lowercase) that mark a provider "context too long" error (07 §8.5).
const OVERFLOW_NEEDLES: [&str; 8] = [
    "context length",
    "context_length_exceeded",
    "maximum context",
    "prompt is too long",
    "input length and max_tokens exceed",
    "request too large",
    "exceeds the maximum number of tokens",
    "too many tokens",
];

/// Token estimate of one message: serialised chars / 4 (07 §7). The only
/// local estimate in the design; applied to appended slices, never the whole history.
pub fn est(m: &ChatMessage) -> u32 {
    let chars = serde_json::to_string(m).map_or(0, |s| s.len());
    u32::try_from(chars / 4).unwrap_or(u32::MAX)
}

/// Index of the first message kept verbatim (07 §8.2). `limit` is the token
/// count compaction triggers at (`compact_at × window`, or the window when
/// auto-compaction is off); the kept tail is at most `limit / 8`. Never an
/// index whose message is `Tool`; 0 means nothing to fold.
pub fn keep_from(history: &[ChatMessage], limit: u32) -> usize {
    let budget = limit / 8;
    let valid = |i: usize| history[i].role != ChatRole::Tool;
    let mut acc = 0u32;
    let mut keep = None;
    for i in (1..history.len()).rev() {
        acc = acc.saturating_add(est(&history[i]));
        if acc > budget {
            break;
        }
        if valid(i) {
            keep = Some(i);
        }
    }
    keep.unwrap_or_else(|| (1..history.len()).rev().find(|&i| valid(i)).unwrap_or(0))
}

/// Whether a provider error message means the request exceeded the context window (07 §8.5).
pub fn is_overflow(err: &str) -> bool {
    let e = err.to_lowercase();
    OVERFLOW_NEEDLES.iter().any(|n| e.contains(n))
}

#[cfg(test)]
mod tests {
    use genai::chat::{MessageContent, ToolCall, ToolResponse};

    use super::*;

    /// User, Assistant(tool_use), Tool, Assistant text … with `pad`-char payloads.
    fn history(rounds: usize, pad: usize) -> Vec<ChatMessage> {
        let text = "x".repeat(pad);
        let mut h = vec![ChatMessage::user(text.clone())];
        for i in 0..rounds {
            h.push(ChatMessage::from(vec![ToolCall {
                call_id: format!("c{i}"),
                fn_name: "shell".into(),
                fn_arguments: serde_json::json!({ "cmd": text }),
                thought_signatures: None,
            }]));
            h.push(ChatMessage::from(vec![ToolResponse::new(
                format!("c{i}"),
                text.clone(),
            )]));
            h.push(ChatMessage::assistant(MessageContent::from_text(
                text.clone(),
            )));
        }
        h
    }

    fn kept(h: &[ChatMessage], from: usize) -> u32 {
        h[from..].iter().map(est).sum()
    }

    #[test]
    fn cut_rules() {
        let h = history(6, 400);
        let total = kept(&h, 0);
        for window in [total / 4, total, total * 2, total * 8, total * 64] {
            let k = keep_from(&h, window);
            assert_ne!(h[k].role, ChatRole::Tool, "window {window} cut at Tool");
            let budget = window / 8;
            let fits = (1..h.len()).any(|i| h[i].role != ChatRole::Tool && kept(&h, i) <= budget);
            if fits {
                assert!(
                    k > 0 && kept(&h, k) <= budget,
                    "window {window}: kept over budget"
                );
            } else {
                let largest = (1..h.len())
                    .rev()
                    .find(|&i| h[i].role != ChatRole::Tool)
                    .unwrap();
                assert_eq!(k, largest, "window {window}: not the smallest tail");
            }
        }
    }

    #[test]
    fn zero_only_when_no_valid_cut() {
        // [User, Tool]: index 1 is Tool → nothing to fold.
        let h = vec![
            ChatMessage::user("q"),
            ChatMessage::from(vec![ToolResponse::new("c", "r")]),
        ];
        assert_eq!(keep_from(&h, 0), 0);
        assert_eq!(keep_from(&h, 1 << 20), 0);
        assert_eq!(keep_from(&[ChatMessage::user("q")], 1 << 20), 0);
        // One valid index → never 0, even with a zero window.
        let h = vec![
            ChatMessage::user("q"),
            ChatMessage::assistant(MessageContent::from_text("a")),
        ];
        assert_eq!(keep_from(&h, 0), 1);
    }

    #[test]
    fn overflow_needles() {
        assert!(is_overflow(
            "Error: Prompt is too long: 210000 tokens > 200000 maximum"
        ));
        assert!(is_overflow(
            "This model's maximum context length is 128000 tokens"
        ));
        assert!(!is_overflow("rate limit exceeded"));
    }
}
