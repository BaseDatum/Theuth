//! Compaction: styles, cut rules, summariser instruction, overflow classifier (design docs 07 §8, 11).

use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use genai::chat::{Binary, BinarySource, ChatMessage, ChatRole, ContentPart, MessageContent};
use serde::{Deserialize, Serialize};

/// Appended as one user message to the live frame; the summary replaces
/// everything above it in the next epoch (07 §8.3, 36 §6.2).
pub const COMPACT_INSTRUCTION: &str = "\
The conversation is about to be compacted: everything above this message will be replaced by
your summary, and only the most recent messages will be kept verbatim. Write the summary now.
Reply with text only — do not call tools. Use exactly these headings and short bullets:

## Goal
## Done  (facts established, files changed, commands run and whether they passed)
## Open  (in-progress work, unanswered questions, pending approvals)
## Next  (the immediate next step, and a short verbatim quote of the last thing you were
         doing so the continuation resumes mid-step rather than restarting)
## Notes (user preferences, constraints, names and paths to remember, runtime variables you
         rely on and what they hold, decisions taken and alternatives rejected)

If a previous summary is present at the top of the conversation it is authoritative for the
earlier history: carry forward what is still true, drop what is finished. Every request the
user made, in order, belongs under Goal or Done; do not invent tasks. Be specific — paths,
identifiers, exact error text — and omit chit-chat and tool-call boilerplate. A summary that
fits is worth more than one that is exhaustive: at most a few thousand words.";

/// Shortest summary accepted as healthy (36 §6.1); grok's smallest healthy prod summary is ~3,242 chars.
pub const SUMMARY_MIN_CHARS: usize = 500;

/// Substrings (lowercase) that mark a provider "context too long" error (07 §8.5, 36 §2.5).
const OVERFLOW_NEEDLES: [&str; 15] = [
    "context length",
    "context_length_exceeded",
    "maximum context",
    "prompt is too long",
    "input length and max_tokens exceed",
    "request too large",
    "exceeds the maximum number of tokens",
    "too many tokens",
    "too long for this model",
    "maximum allowed number of bytes",
    "exceeds the maximum allowed length",
    "payload too large",
    "content too large",
    "request entity too large",
    "413 ",
];

/// Substrings (lowercase) of a rate-limit error (36 §2.5): a 429 that says
/// "too large a burst" is Transient, not Overflow.
const RATE_LIMIT_NEEDLES: [&str; 2] = ["rate limit", "too many requests"];

/// Substrings (lowercase) of an error the same request would hit again (36 §2.1).
const DETERMINISTIC_NEEDLES: [&str; 7] = [
    "unauthorized",
    "forbidden",
    "invalid_request",
    "invalid api key",
    "authentication",
    "schema",
    "not found",
];

/// Flat token charge for one image part in [`est`] (17 §3.4): a ≤1568 px PNG lands near this.
pub const IMAGE_TOKENS: u32 = 1600;
/// Token charge per PDF page (20 §2.5): Anthropic bills text plus a rendered
/// image of each page, 1.5–3k tokens.
pub const PDF_PAGE_TOKENS: u32 = 2000;

/// Token estimate of one message: serialised chars / 4 (07 §7). The only
/// local estimate in the design; applied to appended slices, never the whole history.
/// An image part counts a flat `IMAGE_TOKENS` — providers bill images by pixels
/// (17 §3.4); a PDF part counts `PDF_PAGE_TOKENS` per page.
pub fn est(m: &ChatMessage) -> u32 {
    est_content(&m.content)
}

/// [`est`] over bare content (a streamed reply before it becomes a message).
/// A tool response counts its text plus `IMAGE_TOKENS` per attached image
/// (23 §5) rather than the serialised base64.
pub fn est_content(c: &MessageContent) -> u32 {
    let chars = |n: usize| u32::try_from(n / 4).unwrap_or(u32::MAX);
    c.iter().fold(0u32, |acc, part| {
        acc.saturating_add(match part {
            ContentPart::Text(t) => chars(t.len()),
            ContentPart::Binary(b) if b.is_pdf() => pdf_pages(b).saturating_mul(PDF_PAGE_TOKENS),
            ContentPart::Binary(_) => IMAGE_TOKENS,
            ContentPart::ToolResponse(r) => chars(r.content.len())
                .saturating_add((r.images.len() as u32).saturating_mul(IMAGE_TOKENS)),
            other => chars(serde_json::to_string(other).map_or(0, |s| s.len())),
        })
    })
}

/// Page count of a base64 PDF part: `/Type /Page` objects (not `/Pages`), minimum 1.
// ponytail: byte scan, not a parse; object streams hide page objects in
// some PDFs, so this undercounts them. lopdf if compaction ever cares.
fn pdf_pages(b: &Binary) -> u32 {
    let BinarySource::Base64(data) = &b.source else {
        return 1;
    };
    let Ok(bytes) = BASE64.decode(data.as_bytes()) else {
        return 1;
    };
    let n: usize = [&b"/Type/Page"[..], b"/Type /Page"]
        .into_iter()
        .map(|needle| {
            bytes
                .windows(needle.len() + 1)
                .filter(|w| w.starts_with(needle) && w[needle.len()] != b's')
                .count()
        })
        .sum();
    u32::try_from(n).unwrap_or(u32::MAX).max(1)
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

/// Why a model call failed, and what the loop does about it (36 §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The request exceeded the context window: compact and resend (07 §8.5).
    Overflow,
    /// 4xx other than 408/429, auth/config, schema rejection: the same request
    /// gets the same answer, so it is not retried.
    Deterministic,
    /// Everything else — 408, 429, 5xx, connection errors, dead streams: retry.
    Transient,
}

/// Classify a provider error message (36 §2.1). Rate-limit phrases and a
/// `429` code win first, then [`is_overflow`], then anything carrying a 4xx
/// status (a standalone three-digit `4dd` token, which covers genai's
/// `status: 4..`, `status code 4..` and `http 4..`) or an auth/schema word;
/// the rest is transient. Same heuristic as grok `session_compact.rs:145-217`.
pub fn classify(err: &str) -> Failure {
    let e = err.to_lowercase();
    let (mut rate, mut client) = (false, false);
    for code in e
        .split(|c: char| !c.is_ascii_digit())
        .filter(|t| t.len() == 3 && t.starts_with('4'))
    {
        match code {
            "429" => rate = true,
            "408" => {}
            _ => client = true,
        }
    }
    if rate || RATE_LIMIT_NEEDLES.iter().any(|n| e.contains(n)) {
        Failure::Transient
    } else if OVERFLOW_NEEDLES.iter().any(|n| e.contains(n)) {
        Failure::Overflow
    } else if client || DETERMINISTIC_NEEDLES.iter().any(|n| e.contains(n)) {
        Failure::Deterministic
    } else {
        Failure::Transient
    }
}

/// Summary body with a U+200B after `<` in every `<context_summary` /
/// `</context_summary`, so the model is not primed to open its next reply
/// with the tag it just saw (36 §6.1). One allocation; unchanged text otherwise.
pub fn neutralise(body: &str) -> String {
    const TAG: &str = "context_summary";
    let mut out = String::with_capacity(body.len() + 8);
    let mut rest = body;
    while let Some(i) = rest.find(TAG) {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        if head.ends_with('<') || head.ends_with("</") {
            out.push('\u{200B}');
        }
        out.push_str(TAG);
        rest = &tail[TAG.len()..];
    }
    out.push_str(rest);
    out
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
    /// `/clear` (11 §2.4): fold everything, keep nothing, no model call. Manual only —
    /// never a default or automatic style.
    Clear,
}

impl fmt::Display for CompactStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Summary => "summary",
            Self::Cut => "cut",
            Self::Snap => "snap",
            Self::Clear => "clear",
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
            "clear" => Ok(Self::Clear),
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
/// to `cut` when automatic. `clear` is only ever manual; a hand-edited
/// default of `clear` compacts as `summary`.
pub(crate) fn resolve(
    r#override: Option<CompactStyle>,
    cfg: CompactStyle,
    reason: CompactReason,
    vision: bool,
) -> Result<CompactStyle, String> {
    let requested = r#override.unwrap_or(cfg);
    match requested {
        CompactStyle::Snap if !vision && reason == CompactReason::Manual => {
            Err("snap needs a vision model (this one is text-only)".into())
        }
        CompactStyle::Snap if !vision => Ok(CompactStyle::Cut),
        CompactStyle::Clear if reason != CompactReason::Manual => Ok(CompactStyle::Summary),
        s => Ok(s),
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
    fn est_charges_images_flat_and_pdfs_per_page() {
        let img = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::from_binary_base64("image/png", "A".repeat(1024 * 1024), None),
        ]));
        assert!(est(&img) <= IMAGE_TOKENS + 64, "{}", est(&img));
        assert_eq!(est(&ChatMessage::user("x".repeat(400))), 100);
        // Three page objects, one page tree node; both spellings.
        let pdf = b"%PDF-1.4\n1 0 obj<</Type /Pages/Kids[]>>endobj\n2 0 obj<</Type /Page>>endobj\n3 0 obj<</Type/Page/Parent 1 0 R>>endobj\n4 0 obj<</Type /Page /Parent 1 0 R>>endobj\n";
        let doc = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::from_binary_base64(
                "application/pdf",
                BASE64.encode(pdf),
                Some("a.pdf".into()),
            ),
        ]));
        assert_eq!(est(&doc), 3 * PDF_PAGE_TOKENS);
    }

    #[test]
    fn est_charges_tool_response_images_flat() {
        let content = "screenshot 800×600 attached";
        let m = ChatMessage::from(vec![ToolResponse::new("c1", content).with_images(vec![
            Binary::from_base64(
                "image/png",
                "A".repeat(200 * 1024),
                Some("shot-1.png".into()),
            ),
        ])]);
        assert_eq!(m.role, ChatRole::Tool);
        let n = est(&m);
        assert!(n <= IMAGE_TOKENS + (content.len() / 4) as u32 + 64, "{n}");
        assert!(n >= IMAGE_TOKENS, "{n}");
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
        assert!(is_overflow("413 Payload Too Large"));
        assert!(is_overflow("input is too long for this model"));
        assert!(!is_overflow("rate limit exceeded"));
        assert!(!is_overflow("too many requests"));
    }

    #[test]
    fn classify_table() {
        use Failure::*;
        for (err, want) in [
            ("429 too many requests", Transient),
            ("status: 400 invalid_request_error", Deterministic),
            ("prompt is too long", Overflow),
            ("413 Payload Too Large", Overflow),
            ("rate limit exceeded: request too large a burst", Transient),
            ("connection reset by peer", Transient),
            ("stream ended without End event", Transient),
            ("status: 401 unauthorized", Deterministic),
            ("status: 408 request timeout", Transient),
            ("status: 503 service unavailable", Transient),
            ("http 422 schema validation failed", Deterministic),
        ] {
            assert_eq!(classify(err), want, "{err}");
        }
    }

    #[test]
    fn neutralise_breaks_tags_only() {
        assert_eq!(
            neutralise("a <context_summary epoch=\"1\">b</context_summary> c"),
            "a <\u{200B}context_summary epoch=\"1\">b</\u{200B}context_summary> c"
        );
        assert_eq!(
            neutralise("plain context_summary text"),
            "plain context_summary text"
        );
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
        // Clear is `/clear` only: a hand-edited default never wipes automatically.
        assert_eq!(resolve(Some(Clear), Summary, Manual, false).unwrap(), Clear);
        assert_eq!(resolve(None, Clear, Auto, true).unwrap(), Summary);
        assert_eq!(resolve(None, Clear, Overflow, false).unwrap(), Summary);
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
