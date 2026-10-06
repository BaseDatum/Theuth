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
## Notes (user preferences, constraints, names and paths to remember, decisions taken and
         alternatives rejected; the Python runtime keeps every variable after this summary -
         for each name worth reusing say what it holds: type, size, where it came from -
         so it is reused, not recomputed)

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
const DETERMINISTIC_NEEDLES: [&str; 8] = [
    "unauthorized",
    "forbidden",
    "invalid_request",
    "invalid api key",
    "authentication",
    "schema",
    "not found",
    // genai's own pre-request failure (missing key env, target resolver): no
    // request was sent, and the same config yields the same answer.
    "resolver error",
];

/// Substrings (lowercase, backticks removed) of a provider error that means a
/// replayed reasoning block was refused (90 §1.5). Matched by
/// [`is_reasoning_rejected`] after the same normalisation.
const REASONING_REJECTED_NEEDLES: [&str; 6] = [
    // Anthropic: a signature that does not verify (the body reads
    // "Invalid `signature` in `thinking` block"; backticks are stripped first).
    "invalid signature in thinking block",
    // Anthropic: a thinking block edited or reordered between turns.
    "thinking blocks cannot be modified",
    // Anthropic: prefix-binding check (accounts created after 2026-08-31).
    "bound to a different conversation",
    // OpenAI Responses: `reasoning.encrypted_content` from another key/org/deployment.
    "invalid_encrypted_content",
    "encrypted content could not be",
    "could not be decrypted",
];

/// Gemini reports a bad `thoughtSignature` without one stable phrase: both
/// words must appear (90 §1.5).
const REASONING_REJECTED_PAIR: (&str, &str) = ("thought_signature", "invalid");

/// Flat token charge for one image part in [`est`] (17 §3.4): a ≤1568 px PNG lands near this.
pub const IMAGE_TOKENS: u32 = 1600;
/// Token charge per PDF page (20 §2.5): Anthropic bills text plus a rendered
/// image of each page, 1.5–3k tokens.
pub const PDF_PAGE_TOKENS: u32 = 2000;

/// Token estimate of one message: serialised chars / 4 (07 §7). The only
/// local estimate in the design; applied to appended slices, never the whole history.
/// An image part counts `w × h / 750` for a readable PNG, else a flat
/// `IMAGE_TOKENS` (17 §3.4); a PDF part counts `PDF_PAGE_TOKENS` per page.
pub fn est(m: &ChatMessage) -> u32 {
    est_content(&m.content)
}

/// [`est`] over bare content (a streamed reply before it becomes a message).
/// A tool response counts its text plus `IMAGE_TOKENS` per attached image
/// (23 §5) rather than the serialised base64.
pub fn est_content(c: &MessageContent) -> u32 {
    c.iter()
        .fold(0u32, |acc, part| acc.saturating_add(est_part(part)))
}

/// [`est`] of one content part.
pub fn est_part(part: &ContentPart) -> u32 {
    let chars = |n: usize| u32::try_from(n / 4).unwrap_or(u32::MAX);
    match part {
        ContentPart::Text(t) => chars(t.len()),
        ContentPart::Binary(b) if b.is_pdf() => pdf_pages(b).saturating_mul(PDF_PAGE_TOKENS),
        ContentPart::Binary(b) => image_tokens(b),
        ContentPart::ToolResponse(r) => chars(r.content.len())
            .saturating_add((r.images.len() as u32).saturating_mul(IMAGE_TOKENS)),
        other => chars(serde_json::to_string(other).map_or(0, |s| s.len())),
    }
}

/// `tokens` in `est` units scaled to the provider's units by the session's
/// calibration ratio (`None` = 1.0).
pub fn calibrated(tokens: u32, calibration: Option<f32>) -> u32 {
    match calibration {
        Some(r) => (tokens as f32 * r) as u32,
        None => tokens,
    }
}

/// Tokens a provider bills for one image part: `w × h / 750` (Anthropic's
/// formula; 13 §1) when the PNG header is readable, else the flat
/// `IMAGE_TOKENS`. A full 1568² snap frame is ~3.3k.
fn image_tokens(b: &Binary) -> u32 {
    match png_dims(b) {
        Some((w, h)) => u32::try_from(u64::from(w) * u64::from(h) / 750).unwrap_or(u32::MAX),
        None => IMAGE_TOKENS,
    }
}

/// Width and height from a base64 PNG's IHDR (bytes 16..24; 32 base64 chars = 24 bytes).
fn png_dims(b: &Binary) -> Option<(u32, u32)> {
    let BinarySource::Base64(data) = &b.source else {
        return None;
    };
    let head = BASE64.decode(data.get(..32)?.as_bytes()).ok()?;
    if head.len() < 24 || head[..8] != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    Some((be(16), be(20)))
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

/// Kept-tail budget when the estimator has never been calibrated against a
/// provider's `usage`: at most this many messages, whatever `est` says.
pub const TAIL_MESSAGES_UNCALIBRATED: usize = 60;

/// 07 §8.2 (81 COMPACT-FLOOR): the least headroom (`limit − fixed`, provider
/// tokens) a fold can work with. An `at_tokens` override below
/// `fixed + MIN_HEADROOM` is raised to it; a window-derived trigger below it
/// turns auto-compaction off for the session — at 60 % of 8k the head and
/// tail together get 4.8k, under which no fold keeps the current exchange.
pub const MIN_HEADROOM: u32 = 8_000;
/// Ceiling for a summary head (provider tokens): the instruction asks for "at
/// most a few thousand words" plus the harness lines.
pub const SUMMARY_TOKENS: u32 = 6_000;
/// Share of the headroom (percent) the kept tail may take after a fold;
/// with [`HEAD_SHARE`] 60 %, leaving 40 % for growth before the next trigger.
pub const TAIL_SHARE: u32 = 35;
/// Share of the headroom (percent) the head may take after a fold.
pub const HEAD_SHARE: u32 = 25;
/// Growth since the last fold (percent of the headroom) before the auto
/// trigger may fire again — a fold that freed nothing is not repeated next turn.
pub const REGROWTH_SHARE: u32 = 30;

/// `limit − fixed`: the tokens history and head may occupy under the trigger.
pub fn headroom(limit: u32, fixed: u32) -> u32 {
    limit.saturating_sub(fixed)
}

/// `pct` percent of `n`.
pub fn share(n: u32, pct: u32) -> u32 {
    (u64::from(n) * u64::from(pct) / 100) as u32
}

/// 07 §8.2: what a fold may keep, all in provider tokens. Invariant:
/// `head + tail ≤ 60 % of avail`, so the compacted frame is at most
/// `fixed + 0.6 × avail` and at least 40 % of the headroom is free for growth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldBudget {
    /// [`headroom`] of the trigger.
    pub avail: u32,
    /// Most tokens the kept tail may hold: `min(tail_tokens, 35 % avail,
    /// target_tokens − head)`.
    pub tail: u32,
    /// Most tokens the head may hold: `min(head_ceiling, 25 % avail)`; the
    /// ceiling is `image_tokens` for a snap fold, [`SUMMARY_TOKENS`] otherwise.
    pub head: u32,
}

/// The [`FoldBudget`] for a trigger `limit` over `fixed` tokens of system
/// prompt and tools, under the configured ceilings.
pub fn fold_budget(
    limit: u32,
    fixed: u32,
    head_ceiling: u32,
    cfg: &crate::CompactionConfig,
) -> FoldBudget {
    let avail = headroom(limit, fixed);
    let head = head_ceiling.min(share(avail, HEAD_SHARE));
    let tail = cfg
        .tail_tokens
        .min(share(avail, TAIL_SHARE))
        .min(cfg.target_tokens.saturating_sub(head));
    FoldBudget { avail, tail, head }
}

/// 07 §8.1: an `at_tokens` override raised to `fixed + MIN_HEADROOM`; the
/// returned flag says whether it was.
pub fn clamp_trigger(at_tokens: u32, fixed: u32) -> (u32, bool) {
    let floor = fixed.saturating_add(MIN_HEADROOM);
    (at_tokens.max(floor), at_tokens < floor)
}

/// 07 §8.1 hysteresis: the auto trigger fires at `projected ≥ limit` only
/// when the frame grew by [`REGROWTH_SHARE`] of `avail` since the last fold
/// (`last_after`, `None` before the first).
pub fn may_fire(projected: u32, limit: u32, last_after: Option<u32>, avail: u32) -> bool {
    projected >= limit
        && last_after
            .is_none_or(|after| projected.saturating_sub(after) >= share(avail, REGROWTH_SHARE))
}

/// 07 §8.1 floor clamp: `true` when no fold can get under the trigger — the
/// headroom is below [`MIN_HEADROOM`] (which includes `fixed + head ≥ limit`,
/// since `head ≤ avail / 4`).
pub fn below_floor(limit: u32, fixed: u32) -> bool {
    headroom(limit, fixed) < MIN_HEADROOM
}

/// Re-enable test for a disabled session: the trigger or the fixed size moved
/// by more than 10 % of the trigger it was disabled at (movement is measured
/// against the trigger, so a small `fixed` estimate wobbling is not a change).
pub fn inputs_moved((limit0, fixed0): (u32, u32), limit: u32, fixed: u32) -> bool {
    let step = share(limit0, 10);
    limit0.abs_diff(limit) > step || fixed0.abs_diff(fixed) > step
}

/// Index of the first message kept verbatim (07 §8.2). `budget` is the tail's
/// token budget in `est` units (the caller has already divided the configured
/// `tail_tokens` by the session's calibration ratio) measured with
/// [`est_kept`]; `max_msgs` caps the count. Never an index whose message is
/// `Tool`; 0 means nothing to fold.
pub fn keep_from(history: &[ChatMessage], budget: u32, max_msgs: usize) -> usize {
    let valid = |i: usize| history[i].role != ChatRole::Tool;
    let mut acc = 0u32;
    let mut keep = None;
    for i in (1..history.len()).rev() {
        acc = acc.saturating_add(est_kept(&history[i]));
        if acc > budget || history.len() - i > max_msgs {
            break;
        }
        if valid(i) {
            keep = Some(i);
        }
    }
    keep.unwrap_or_else(|| (1..history.len()).rev().find(|&i| valid(i)).unwrap_or(0))
}

/// A tool result kept verbatim in the tail longer than this many chars
/// (≈ 2k tokens) is folded to head + tail by [`fold_tool_results`]; the
/// store row and the archive image keep the whole text.
pub const FOLD_CHARS: usize = 8_000;
const FOLD_HEAD: usize = 5_000;
const FOLD_TAIL: usize = 2_000;

/// `s` as the tail keeps it: `None` when it fits, else head + marker + tail.
pub fn fold_text(s: &str) -> Option<String> {
    let total = s.chars().count();
    if total <= FOLD_CHARS {
        return None;
    }
    let at = |n: usize| s.char_indices().nth(n).map_or(s.len(), |(i, _)| i);
    Some(format!(
        "{}\n[… {} chars folded — read the archive]\n{}",
        &s[..at(FOLD_HEAD)],
        total - FOLD_HEAD - FOLD_TAIL,
        &s[at(total - FOLD_TAIL)..]
    ))
}

/// Fold every oversize tool result in `m` in place; how many were folded.
pub fn fold_tool_results(m: &mut ChatMessage) -> usize {
    let mut n = 0;
    for part in m.content.iter_mut() {
        if let ContentPart::ToolResponse(r) = part
            && let Some(folded) = fold_text(&r.content)
        {
            r.content = folded;
            n += 1;
        }
    }
    n
}

/// [`est`] of `m` as the tail will carry it — oversize tool results folded.
pub fn est_kept(m: &ChatMessage) -> u32 {
    m.content.iter().fold(est(m), |acc, part| match part {
        ContentPart::ToolResponse(r) => {
            let total = r.content.chars().count();
            if total > FOLD_CHARS {
                acc.saturating_sub(((total - FOLD_HEAD - FOLD_TAIL) / 4) as u32)
            } else {
                acc
            }
        }
        _ => acc,
    })
}

/// Whether a provider error message means the request exceeded the context window (07 §8.5).
pub fn is_overflow(err: &str) -> bool {
    let e = err.to_lowercase();
    OVERFLOW_NEEDLES.iter().any(|n| e.contains(n))
}

/// 116: fewest tokens worth asking for; below this the request is sent anyway and the
/// overflow path (C, then compaction) takes over.
pub const REPLY_FLOOR: u32 = 4_096;
/// 116: flat pad over the scaled prompt estimate (tool-definition and framing drift).
pub const HEADROOM_PAD: u32 = 2_048;

/// 116: the `max_tokens` to send — the catalog output limit, clamped so the projected
/// prompt plus the reply fits the window. `window == 0` (unknown) → `output_limit`.
/// `projected` is in `est` units; `calibration` scales it to the provider's
/// (`None` reserves a 25 % margin for chars/4 against a real tokeniser, `Some`
/// 10 %). `output_limit == 0` (unknown) → the remaining window when it is
/// known, else 0 (the caller leaves `Frame::max_tokens` unset).
pub fn reply_budget(
    output_limit: u32,
    window: u32,
    projected: u32,
    calibration: Option<f32>,
    reasoning_floor: u32,
) -> u32 {
    if window == 0 {
        return output_limit;
    }
    let scaled = calibrated(projected, calibration);
    let margin = if calibration.is_none() {
        scaled / 4
    } else {
        scaled / 10
    };
    let reserve = scaled.saturating_add(margin).saturating_add(HEADROOM_PAD);
    let remaining = window.saturating_sub(reserve);
    let floor = REPLY_FLOOR.max(reasoning_floor);
    let fit = remaining.max(floor);
    if output_limit == 0 {
        fit
    } else {
        output_limit.min(fit)
    }
}

/// 116: the numbers a provider quotes in a context-overflow error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverflowNumbers {
    pub window: u32,
    pub prompt: u32,
    pub max_tokens: Option<u32>,
}

/// 116: the window, prompt and `max_tokens` a provider's overflow body
/// quotes, when it quotes them — vLLM/litellm ("maximum context length is W
/// … (P prompt tokens plus max_tokens M)"), OpenAI ("maximum context length
/// is W … (P in the messages, M in the completion)"), Anthropic ("context
/// limit: P + M > W" and "prompt is too long: P tokens > W maximum", the
/// latter with no `max_tokens`). `None` when no window is recognisable.
pub fn overflow_numbers(err: &str) -> Option<OverflowNumbers> {
    let e = err.to_lowercase();
    if let Some(rest) = after(&e, "context limit:") {
        let mut n = nums(rest);
        let (prompt, max_tokens, window) = (n.next()?, n.next()?, n.next()?);
        return Some(OverflowNumbers {
            window,
            prompt,
            max_tokens: Some(max_tokens),
        });
    }
    if let Some(rest) = after(&e, "maximum context length is") {
        let window = nums(rest).next()?;
        let (prompt, max_tokens) = match num_before(rest, " prompt tokens") {
            Some(p) => (p, after(rest, "max_tokens").and_then(|r| nums(r).next())),
            None => (
                num_before(rest, " in the messages")?,
                num_before(rest, " in the completion"),
            ),
        };
        return Some(OverflowNumbers {
            window,
            prompt,
            max_tokens,
        });
    }
    if let Some(rest) = after(&e, "too long:") {
        let prompt = nums(rest).next()?;
        let window = after(rest, ">").and_then(|r| nums(r).next())?;
        if rest.contains("maximum") {
            return Some(OverflowNumbers {
                window,
                prompt,
                max_tokens: None,
            });
        }
    }
    None
}

/// The text after the first `anchor`.
fn after<'a>(s: &'a str, anchor: &str) -> Option<&'a str> {
    s.find(anchor).map(|i| &s[i + anchor.len()..])
}

/// Every unsigned integer in `s`, in order (a number too big for `u32` is skipped).
fn nums(s: &str) -> impl Iterator<Item = u32> + '_ {
    s.split(|c: char| !c.is_ascii_digit())
        .filter_map(|t| t.parse().ok())
}

/// The integer immediately before the first `anchor` (`"120000 in the messages"`).
fn num_before(s: &str, anchor: &str) -> Option<u32> {
    let head = &s[..s.find(anchor)?];
    let start = head.rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    head[start..].parse().ok()
}

/// Whether a provider error message means a replayed reasoning block was
/// refused (90 §1.5): the request is resent once without reasoning parts.
pub fn is_reasoning_rejected(err: &str) -> bool {
    let e: String = err.to_lowercase().replace('`', "");
    REASONING_REJECTED_NEEDLES.iter().any(|n| e.contains(n))
        || (e.contains(REASONING_REJECTED_PAIR.0) && e.contains(REASONING_REJECTED_PAIR.1))
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
        for budget in [total / 32, total / 8, total / 4, total, total * 8] {
            let k = keep_from(&h, budget, usize::MAX);
            assert_ne!(h[k].role, ChatRole::Tool, "budget {budget} cut at Tool");
            let fits = (1..h.len()).any(|i| h[i].role != ChatRole::Tool && kept(&h, i) <= budget);
            if fits {
                assert!(
                    k > 0 && kept(&h, k) <= budget,
                    "budget {budget}: kept over budget"
                );
            } else {
                let largest = (1..h.len())
                    .rev()
                    .find(|&i| h[i].role != ChatRole::Tool)
                    .unwrap();
                assert_eq!(k, largest, "budget {budget}: not the smallest tail");
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
        assert_eq!(keep_from(&h, 0, usize::MAX), 0);
        assert_eq!(keep_from(&h, 1 << 20, usize::MAX), 0);
        assert_eq!(keep_from(&[ChatMessage::user("q")], 1 << 20, usize::MAX), 0);
        // One valid index → never 0, even with a zero window.
        let h = vec![
            ChatMessage::user("q"),
            ChatMessage::assistant(MessageContent::from_text("a")),
        ];
        assert_eq!(keep_from(&h, 0, usize::MAX), 1);
    }

    /// The message cap holds regardless of the token budget, and never lands on a Tool row.
    #[test]
    fn keep_from_honours_the_message_cap() {
        let h = history(20, 10);
        let k = keep_from(&h, u32::MAX, 7);
        assert!(h.len() - k <= 7, "kept {}", h.len() - k);
        assert_ne!(h[k].role, ChatRole::Tool);
        assert_eq!(keep_from(&h, u32::MAX, usize::MAX), 1);
    }

    /// A budget under the history's size folds more than the single message
    /// the 2026-09-28 session folded per turn.
    #[test]
    fn keep_from_folds_more_than_one_when_over_budget() {
        let h = history(6, 400);
        let total = kept(&h, 0);
        let k = keep_from(&h, total / 2, usize::MAX);
        assert!(k > 1, "folded {k}");
        assert!(kept(&h, k) <= total / 2);
    }

    #[test]
    fn fold_budget_is_a_share_of_the_headroom() {
        let cfg = crate::CompactionConfig::default();
        let b = fold_budget(24_000, 7_000, cfg.image_tokens, &cfg);
        assert_eq!(
            b,
            FoldBudget {
                avail: 17_000,
                tail: 5_950,
                head: 4_250
            }
        );
        assert!(b.head + b.tail <= share(b.avail, HEAD_SHARE + TAIL_SHARE));
        // Wide headroom: the configured ceilings hold.
        let b = fold_budget(800_000, 10_000, cfg.image_tokens, &cfg);
        assert_eq!(
            b,
            FoldBudget {
                avail: 790_000,
                tail: 40_000,
                head: 30_000
            }
        );
        // `target_tokens` stays the absolute ceiling on head + tail.
        let small = crate::CompactionConfig {
            target_tokens: 50_000,
            ..cfg
        };
        let b = fold_budget(800_000, 0, 30_000, &small);
        assert_eq!((b.head, b.tail), (30_000, 20_000));
        assert_eq!(
            fold_budget(5_000, 7_000, 30_000, &cfg),
            FoldBudget {
                avail: 0,
                tail: 0,
                head: 0
            }
        );
    }

    #[test]
    fn trigger_clamp_floor_and_hysteresis() {
        assert_eq!(clamp_trigger(24_000, 7_000), (24_000, false));
        assert_eq!(clamp_trigger(1, 7_000), (7_000 + MIN_HEADROOM, true));
        // A 9k-window model at compact_at 0.8 with nothing fixed: no fold can help.
        assert!(below_floor(7_200, 0));
        assert!(!below_floor(7_000 + MIN_HEADROOM, 7_000));
        assert!(may_fire(24_000, 24_000, None, 17_000));
        assert!(!may_fire(23_999, 24_000, None, 17_000));
        // Grew 1k since the last fold: under 30 % of 17k.
        assert!(!may_fire(24_000, 24_000, Some(23_000), 17_000));
        assert!(may_fire(29_000, 24_000, Some(23_000), 17_000));
        assert!(!inputs_moved((24_000, 7_000), 24_000, 7_500));
        assert!(inputs_moved((24_000, 7_000), 30_000, 7_000));
        assert!(!inputs_moved((24_000, 7_000), 24_000, 5_000));
        assert!(inputs_moved((24_000, 7_000), 24_000, 4_000));
        assert!(!inputs_moved((7_200, 30), 7_200, 0));
    }

    /// 81 COMPACT-FLOOR regression, the 2026-09-28 meeting shape: override
    /// 24k, ~7k fixed, two ~350-token messages per turn, default budgets,
    /// snap. Over 300 turns no two consecutive turns compact, every fold
    /// leaves 40 % of the headroom free, snap always degrades to summary
    /// (one frame is over the head cap) and the count is bounded.
    #[test]
    fn meeting_shape_never_loops() {
        const FIXED: u32 = 7_000;
        const TURNS: u32 = 300;
        let cfg = crate::CompactionConfig::default();
        let frame = crate::snap::shape("anthropic").tokens_per_frame;
        let msg = "m".repeat(1_400);
        let (limit, clamped) = clamp_trigger(24_000, FIXED);
        assert!(!clamped);
        assert!(!below_floor(limit, FIXED));
        let b = fold_budget(limit, FIXED, cfg.image_tokens, &cfg);
        assert_eq!(b.avail, 17_000);
        let per_turn = 2 * est(&ChatMessage::user(msg.clone()));
        // A fold leaves ≤ fixed + 60 % avail; the next trigger needs ≥ limit
        // again, so at least 40 % of avail of growth between folds.
        let free = b.avail - share(b.avail, HEAD_SHARE + TAIL_SHARE);
        let bound = TURNS / free.div_ceil(per_turn);
        let size =
            |hist: &[ChatMessage], head: u32| FIXED + head + hist.iter().map(est).sum::<u32>();
        let mut hist: Vec<ChatMessage> = Vec::new();
        let mut head = 0u32;
        let mut last_after = None;
        let mut compactions: Vec<u32> = Vec::new();
        for turn in 0..TURNS {
            hist.push(ChatMessage::user(msg.clone()));
            hist.push(ChatMessage::assistant(MessageContent::from_text(
                msg.clone(),
            )));
            let projected = size(&hist, head);
            if !may_fire(projected, limit, last_after, b.avail) {
                continue;
            }
            assert_ne!(
                compactions.last(),
                turn.checked_sub(1).as_ref(),
                "turn {turn}: two consecutive compactions"
            );
            assert!(frame > b.head, "frame {frame} fits head cap {}", b.head);
            let keep = keep_from(&hist, b.tail, usize::MAX);
            assert!(keep > 1, "turn {turn}: folded {keep}");
            hist.drain(..keep);
            // Worst case: the summary fills its cap.
            head = b.head;
            let after = size(&hist, head);
            assert!(
                after <= FIXED + share(b.avail, HEAD_SHARE + TAIL_SHARE),
                "turn {turn}: after {after}"
            );
            compactions.push(turn);
            last_after = Some(after);
        }
        assert!(compactions.len() > 1, "{compactions:?}");
        assert!(
            compactions.len() as u32 <= bound + 1,
            "{} compactions, bound {bound}",
            compactions.len()
        );
    }

    #[test]
    fn fold_keeps_head_and_tail_and_est_kept_matches() {
        assert_eq!(fold_text(&"x".repeat(FOLD_CHARS)), None);
        let big = format!(
            "{}{}{}",
            "h".repeat(FOLD_HEAD),
            "m".repeat(10_000),
            "t".repeat(FOLD_TAIL)
        );
        let f = fold_text(&big).unwrap();
        assert!(f.starts_with(&"h".repeat(FOLD_HEAD)) && f.ends_with(&"t".repeat(FOLD_TAIL)));
        assert!(
            f.contains("[… 10000 chars folded — read the archive]"),
            "{f}"
        );
        assert!(!f.contains('m'));
        let mut m = ChatMessage::from(vec![
            ToolResponse::new("c1", big.clone()),
            ToolResponse::new("c2", "small"),
        ]);
        let kept = est_kept(&m);
        assert_eq!(fold_tool_results(&mut m), 1);
        assert_eq!(fold_tool_results(&mut m), 0, "idempotent");
        assert!(est(&m) < est(&ChatMessage::from(vec![ToolResponse::new("c1", big)])));
        assert!(
            (est(&m) as i64 - kept as i64).abs() <= 16,
            "{} vs {kept}",
            est(&m)
        );
    }

    #[test]
    fn png_frames_are_priced_by_pixels() {
        let png = crate::snap::rasterize(&["hello".to_owned()], &crate::snap::shape("anthropic"));
        let m = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::from_binary_base64("image/png", BASE64.encode(&png[0]), None),
        ]));
        // 1568 wide × 16 tall (one row) → 33 tokens, not the flat 1600.
        assert_eq!(est(&m), 1568 * 16 / 750);
        assert_eq!(
            png_dims(&Binary::from_base64("image/png", "AAAA", None)),
            None
        );
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
            (
                "Resolver error for model 'Omlx::x'.\nCause: ApiKeyEnvNotFound { env_name: \"OMLX_API_KEY\" }",
                Deterministic,
            ),
        ] {
            assert_eq!(classify(err), want, "{err}");
        }
    }

    /// 90 §1.5: the provider bodies that mean "replayed reasoning refused",
    /// and the neighbours that must not.
    #[test]
    fn reasoning_rejected_table() {
        for (err, want) in [
            (
                r#"Request failed with status code '400'. Response body:
{"type":"error","error":{"type":"invalid_request_error","message":"messages.7.content.0: Invalid `signature` in `thinking` block"}}"#,
                true,
            ),
            (
                "400 messages.3.content.0: Invalid signature in thinking block",
                true,
            ),
            ("400 thinking blocks cannot be modified once sent", true),
            (
                "400 the thinking block is bound to a different conversation",
                true,
            ),
            (
                r#"{"error":{"message":"The encrypted content gAAA...S6Q= could not be verified. Reason: Encrypted content could not be decrypted or parsed.","type":"invalid_request_error","param":null,"code":"invalid_encrypted_content"}}"#,
                true,
            ),
            (
                "400 INVALID_ARGUMENT: thought_signature is invalid for this model",
                true,
            ),
            // Co-occurrence of the words alone is not a rejection.
            ("400 thinking block requires a signature field", false),
            ("400 thought_signature missing on function call part", false),
            ("prompt is too long", false),
            ("429 too many requests", false),
            (
                "http 422 schema validation failed: messages.0.content",
                false,
            ),
            ("connection reset by peer", false),
        ] {
            assert_eq!(is_reasoning_rejected(err), want, "{err}");
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

    // 116: the incident numbers and the edges of the clamp.
    #[test]
    fn reply_budget_clamps_to_the_remaining_window() {
        // Unknown window: the catalog figure stands.
        assert_eq!(reply_budget(943_717, 0, 105_663, None, 0), 943_717);
        // The incident: output 943 717 in a 1 048 576 window with a 105 663 prompt.
        let incident = reply_budget(943_717, 1_048_576, 105_663, None, 0);
        assert!(incident < 943_717, "{incident}");
        assert!(incident + 105_663 * 5 / 4 + HEADROOM_PAD <= 1_048_576, "{incident}");
        // A calibrated estimate reserves a thinner margin.
        assert!(reply_budget(943_717, 1_048_576, 105_663, Some(1.0), 0) > incident);
        // Projected near the window: the floor, and the request still goes out.
        assert_eq!(reply_budget(64_000, 200_000, 199_000, None, 0), REPLY_FLOOR);
        // Unknown output limit with a known window: whatever remains.
        assert_eq!(
            reply_budget(0, 200_000, 100_000, Some(1.0), 0),
            200_000 - 100_000 - 10_000 - HEADROOM_PAD
        );
        // Unknown both: 0, the caller leaves max_tokens unset.
        assert_eq!(reply_budget(0, 0, 100_000, None, 0), 0);
        // A reasoning floor above REPLY_FLOOR wins.
        assert_eq!(reply_budget(64_000, 200_000, 199_000, None, 16_384), 16_384);
        // Never over the catalog limit when one is known.
        assert_eq!(reply_budget(8_192, 200_000, 1_000, None, 16_384), 8_192);
    }

    const OVERFLOW_BODIES: [(&str, Option<OverflowNumbers>); 5] = [
        (
            "This server's maximum context length is 1048576 tokens: this request needs a 1049380-token context (105663 prompt tokens plus max_tokens 943717), which exceeds the context window this server was started for; shorten the prompt or reply.",
            Some(OverflowNumbers {
                window: 1_048_576,
                prompt: 105_663,
                max_tokens: Some(943_717),
            }),
        ),
        (
            "This model's maximum context length is 128000 tokens. However, you requested 130000 tokens (120000 in the messages, 10000 in the completion). Please reduce the length of the messages or completion.",
            Some(OverflowNumbers {
                window: 128_000,
                prompt: 120_000,
                max_tokens: Some(10_000),
            }),
        ),
        (
            "input length and max_tokens exceed context limit: 190000 + 64000 > 200000",
            Some(OverflowNumbers {
                window: 200_000,
                prompt: 190_000,
                max_tokens: Some(64_000),
            }),
        ),
        (
            "prompt is too long: 210000 tokens > 200000 maximum",
            Some(OverflowNumbers {
                window: 200_000,
                prompt: 210_000,
                max_tokens: None,
            }),
        ),
        ("request too large for this model (status 413)", None),
    ];

    // 116: the four provider bodies, bare and inside genai's web-error wrapper.
    #[test]
    fn overflow_numbers_reads_each_provider_body() {
        for (body, want) in OVERFLOW_BODIES {
            assert_eq!(overflow_numbers(body), want, "{body}");
            let wrapped = format!(
                "Web stream error for model 'zai-org/GLM-5.3-Flash': status 400. Body: {{\"error\":{{\"message\":\"litellm.BadRequestError: OpenAIException - {body}\",\"type\":\"invalid_request_error\",\"code\":400}}}}"
            );
            assert!(is_overflow(&wrapped) || want.is_none());
            assert_eq!(overflow_numbers(&wrapped), want, "{wrapped}");
        }
    }
}
