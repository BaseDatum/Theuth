//! Compaction: styles, cut rules, summariser instruction, overflow classifier (design docs 07 §8, 11).

use std::fmt;
use std::str::FromStr;

use genai::chat::{ChatMessage, ChatRole, ContentPart};
use serde::{Deserialize, Serialize};

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

/// Flat token charge for one binary part in [`est`] (17 §3.4): a ≤1568 px PNG lands near this.
pub const IMAGE_TOKENS: u32 = 1600;

/// Token estimate of one message: serialised chars / 4 (07 §7). The only
/// local estimate in the design; applied to appended slices, never the whole history.
/// A binary part counts a flat `IMAGE_TOKENS` — providers bill images by pixels (17 §3.4).
pub fn est(m: &ChatMessage) -> u32 {
    let chars = |n: usize| u32::try_from(n / 4).unwrap_or(u32::MAX);
    m.content.iter().fold(0u32, |acc, part| {
        acc.saturating_add(match part {
            ContentPart::Text(t) => chars(t.len()),
            ContentPart::Binary(_) => IMAGE_TOKENS,
            other => chars(serde_json::to_string(other).map_or(0, |s| s.len())),
        })
    })
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

/// How a compaction builds the new epoch head (design doc 11).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum CompactStyle {
    /// Cache-hot LLM fold (07 §8.3). The default.
    #[default]
    Summary,
    /// Drop the folded span, keep the tail, no model call.
    Cut,
    /// Rasterize the folded span to images. Named in v0; not built.
    Snap,
}

impl fmt::Display for CompactStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Summary => "summary",
            Self::Cut => "cut",
            Self::Snap => "snap",
        })
    }
}

impl FromStr for CompactStyle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "summary" => Ok(Self::Summary),
            "cut" => Ok(Self::Cut),
            "snap" => Ok(Self::Snap),
            other => Err(format!("unknown compact style: {other}")),
        }
    }
}

/// Why compaction ran. Auto/overflow may rewrite `snap` to `cut`; manual snap errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompactReason {
    Auto,
    Manual,
    Overflow,
}

/// Pick the style that will actually run (design docs 11 §3.1, 13 §3):
/// `snap` on a text-only model errors when asked for by hand and falls back
/// to `cut` when automatic.
pub(crate) fn resolve(
    r#override: Option<CompactStyle>,
    cfg: CompactStyle,
    reason: CompactReason,
    vision: bool,
) -> Result<CompactStyle, String> {
    let requested = r#override.unwrap_or(cfg);
    if requested == CompactStyle::Snap && !vision {
        if reason == CompactReason::Manual {
            Err("snap needs a vision model (this one is text-only)".into())
        } else {
            Ok(CompactStyle::Cut)
        }
    } else {
        Ok(requested)
    }
}

/// Closing tag of a head; a snap head carries it as its own text part
/// after the frames (design doc 13 §2.4).
pub const HEAD_CLOSE: &str = "</context_summary>";

/// `<context_summary epoch="n" style="…">` wrapper (design doc 11 §2).
pub fn wrap_head(
    n: u32,
    style: CompactStyle,
    body: &str,
    parts: impl IntoIterator<Item = String>,
) -> String {
    let mut text = wrap_head_open(n, style, body, parts);
    text.push('\n');
    text.push_str(HEAD_CLOSE);
    text
}

/// [`wrap_head`] without the closing tag: the first text part of a snap
/// head, followed by the frames and [`HEAD_CLOSE`].
pub fn wrap_head_open(
    n: u32,
    style: CompactStyle,
    body: &str,
    parts: impl IntoIterator<Item = String>,
) -> String {
    let mut text = format!("<context_summary epoch=\"{n}\" style=\"{style}\">\n{body}");
    for part in parts {
        if !part.is_empty() {
            text.push('\n');
            text.push_str(&part);
        }
    }
    text
}

/// Style declared on a head; missing or unknown → [`CompactStyle::Summary`].
pub fn style_of_head(text: &str) -> CompactStyle {
    let Some(rest) = text.trim_start().strip_prefix("<context_summary") else {
        return CompactStyle::Summary;
    };
    let Some(tag_end) = rest.find('>') else {
        return CompactStyle::Summary;
    };
    let attrs = &rest[..tag_end];
    let Some(i) = attrs.find("style=\"") else {
        return CompactStyle::Summary;
    };
    let v = &attrs[i + 7..];
    let Some(j) = v.find('"') else {
        return CompactStyle::Summary;
    };
    v[..j].parse().unwrap_or(CompactStyle::Summary)
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
    fn est_charges_images_flat() {
        let img = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::from_binary_base64("image/png", "A".repeat(1024 * 1024), None),
        ]));
        assert!(est(&img) <= IMAGE_TOKENS + 64, "{}", est(&img));
        assert_eq!(est(&ChatMessage::user("x".repeat(400))), 100);
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

    #[test]
    fn resolve_table() {
        use CompactReason::*;
        use CompactStyle::*;
        assert_eq!(resolve(None, Summary, Manual, false).unwrap(), Summary);
        assert_eq!(resolve(None, Summary, Auto, true).unwrap(), Summary);
        assert_eq!(resolve(Some(Cut), Summary, Manual, true).unwrap(), Cut);
        // Text-only model: automatic snap degrades to cut, manual snap refuses.
        assert_eq!(resolve(None, Snap, Auto, false).unwrap(), Cut);
        assert_eq!(resolve(Some(Snap), Summary, Auto, false).unwrap(), Cut);
        assert_eq!(resolve(None, Snap, Overflow, false).unwrap(), Cut);
        assert_eq!(
            resolve(Some(Snap), Summary, Manual, false).unwrap_err(),
            "snap needs a vision model (this one is text-only)"
        );
        assert!(resolve(None, Snap, Manual, false).is_err());
        // Vision model: snap runs whatever the reason.
        assert_eq!(resolve(None, Snap, Manual, true).unwrap(), Snap);
        assert_eq!(resolve(Some(Snap), Summary, Auto, true).unwrap(), Snap);
        assert_eq!(resolve(None, Snap, Overflow, true).unwrap(), Snap);
    }

    #[test]
    fn wrap_head_has_style() {
        let h = wrap_head(
            3,
            CompactStyle::Cut,
            "folded 2 messages; no summary. Recent turns follow verbatim.",
            [String::from("Roster: a")],
        );
        assert!(h.contains("style=\"cut\""));
        assert!(h.starts_with("<context_summary epoch=\"3\" style=\"cut\">"));
        assert!(h.contains("Roster: a"));
        assert!(h.ends_with("</context_summary>"));
    }

    #[test]
    fn wrap_head_open_plus_close_equals_wrap_head() {
        let parts = || {
            [
                String::from("Roster: a"),
                String::new(),
                String::from("Runtime: x"),
            ]
        };
        let open = wrap_head_open(4, CompactStyle::Snap, "caption", parts());
        assert!(!open.contains(HEAD_CLOSE));
        assert_eq!(
            format!("{open}\n{HEAD_CLOSE}"),
            wrap_head(4, CompactStyle::Snap, "caption", parts())
        );
    }

    #[test]
    fn style_of_head_missing_attr_is_summary() {
        assert_eq!(
            style_of_head("<context_summary epoch=\"1\">\nold\n</context_summary>"),
            CompactStyle::Summary
        );
        assert_eq!(
            style_of_head(&wrap_head(2, CompactStyle::Cut, "x", Vec::<String>::new())),
            CompactStyle::Cut
        );
    }
}
