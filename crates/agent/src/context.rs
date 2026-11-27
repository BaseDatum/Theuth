//! Design doc 48: explicit removal from context. Wire types, the exclusion
//! masks persisted beside `messages`/`epochs` rows, and the pure projection,
//! protocol-closure and inventory helpers the store applies inside its
//! transactions. Nothing here touches sqlite; raw rows stay untouched and the
//! *effective* frame is a projection of row + mask.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use genai::chat::{Binary, ChatMessage, ChatRole, ContentPart, MessageContent, ToolCall};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::compact;
use crate::loop_::text_of;
use crate::store::Epoch;

/// Which stored row a binary occurrence lives in.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextOwner {
    /// A `messages` row.
    Message {
        /// Store seq.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        seq: i64,
    },
    /// The head of an `epochs` row.
    Head {
        /// Epoch number.
        epoch: u32,
    },
}

/// A removable occurrence, addressed by stored coordinates (48 §3.1). `part`
/// and `image` index the **original** arrays, never the filtered ones.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextTarget {
    /// A whole message row.
    Message {
        /// Store seq.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        seq: i64,
    },
    /// The whole current head.
    Head {
        /// Epoch number.
        epoch: u32,
    },
    /// One binary part, or one image inside a tool response part.
    Binary {
        /// The row.
        owner: ContextOwner,
        /// Index into the row's original content parts.
        part: u32,
        /// `Some`: index into the `ToolResponse.images` of that part.
        image: Option<u32>,
    },
    /// One surface fence inside one text part of an assistant row (197 §3).
    Fence {
        /// The row; only `Message` owners have fences.
        owner: ContextOwner,
        /// Index into the row's original content parts (a text part).
        part: u32,
        /// 0-based ordinal of the surface fences of that original text part ([`surface_fences`]).
        index: u32,
    },
    /// The source argument of one `show` / `app` / `page_preview` call part (197 §3).
    Source {
        /// The row; only `Message` owners have sources.
        owner: ContextOwner,
        /// Index into the row's original content parts (a tool call part).
        part: u32,
    },
}

/// What an inventory item is, for the `/shake <category>` filters (197 §1).
/// Declaration order keeps the pre-197 wire positions (`message`, `image`,
/// `document`, `tool`, `summary`); the 197 kinds follow, so the schema
/// change is additive.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    /// Legacy: pre-197 text rows. Never emitted since 197 (user, assistant,
    /// note and peer rows replaced it); kept so the wire stays additive.
    Message,
    /// An image occurrence (user attachment, tool screenshot, tool image, summary frame).
    Image,
    /// A non-image binary occurrence (native PDF).
    Document,
    /// A tool exchange row (tool results, or an assistant row that is only calls).
    Tool,
    /// The current epoch head.
    Summary,
    /// A surface fence in a reply, or the source of a `show` / `app` / `page_preview` call.
    Surface,
    /// A harness-injected row (knowledge hints, todos, time stamps, notices).
    Note,
    /// A message another agent sent (the row has peer provenance).
    Peer,
    /// A row the user typed.
    User,
    /// An assistant row with text.
    Assistant,
}

/// One row of the active-context inventory (48 §3.2, 197 §2). No bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItem {
    /// What to send back to remove it.
    pub target: ContextTarget,
    /// Category.
    pub kind: ContextKind,
    /// Stable lowercase group key within the category (197 §1). `default`:
    /// a reply from an engine before 197 has none (wire receiver view).
    #[serde(default)]
    pub group: String,
    /// Short safe display label (bounded preview or attachment name), ≤ 80 chars.
    pub label: String,
    /// Secondary line (`PNG · 1568×1103`, `in reply`, the other tool names), ≤ 60 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail: Option<String>,
    /// `compact::est` of what removing this target takes out (197 §2 token rule).
    #[serde(default)]
    pub tokens: u32,
    /// RFC 3339 time of the row, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub at: Option<String>,
    /// Whole-message companions the protocol requires to go with it.
    pub remove_with: Vec<ContextTarget>,
}

/// The active context as the store sees it, guarded by `revision`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextSnapshot {
    /// Decimal `sessions.context_revision`.
    pub revision: String,
    /// The session actor accepts edits right now (filled by the actor).
    pub editable: bool,
    /// Why not, when `!editable`.
    pub reason: Option<String>,
    /// Current epoch; 0 before any compaction.
    pub epoch: u32,
    /// First seq of the active tail.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tail_from: i64,
    /// Active items: head, its images, then rows in seq order, each followed
    /// by its binaries, then its surface fences and sources.
    pub items: Vec<ContextItem>,
    /// `compact::est` over the effective head + tail (not provider usage).
    pub estimated_tokens: u32,
}

/// Where the tokens of the model's frame go (70 COMPACT-SIZE): the estimate
/// the engine drives compaction and the post-compaction gauge with, beside
/// the provider's last bill, so the two can be compared without sqlite.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextBreakdown {
    /// Archive frames and other binaries in the head.
    pub head_images_tokens: u32,
    /// Head text: caption/summary plus harness lines.
    pub head_text_tokens: u32,
    /// Messages replayed verbatim after the head.
    pub tail_messages: u32,
    /// `compact::est` over the tail (tool results as folded).
    pub tail_tokens: u32,
    /// System prompt plus tool definitions; 0 when answered from the store
    /// (no live actor).
    pub system_tokens: u32,
    /// Everything above, scaled by `calibration`: what the gauge shows as "≈".
    pub total_estimate: u32,
    /// Prompt + completion the provider last billed, if any this process.
    pub last_provider_context: Option<u32>,
    /// Provider tokens ÷ `est` for the last billed frame; `None` = uncalibrated.
    pub calibration: Option<f32>,
    /// Answered by the live session actor (system and calibration known).
    pub live: bool,
}

impl ContextBreakdown {
    /// Price `head` + `tail` with `compact::est`, `system_tokens` on top,
    /// the total scaled by `calibration`.
    pub fn build<'a>(
        system_tokens: u32,
        head: Option<&ChatMessage>,
        tail: impl Iterator<Item = &'a ChatMessage>,
        calibration: Option<f32>,
        last_provider_context: Option<u32>,
    ) -> Self {
        let head_images_tokens = head.map_or(0, |h| {
            h.content
                .iter()
                .filter(|p| matches!(p, ContentPart::Binary(_)))
                .fold(0u32, |acc, p| acc.saturating_add(compact::est_part(p)))
        });
        let head_text_tokens = head
            .map_or(0, compact::est)
            .saturating_sub(head_images_tokens);
        let (tail_messages, tail_tokens) = tail.fold((0u32, 0u32), |(n, t), m| {
            (n + 1, t.saturating_add(compact::est(m)))
        });
        let raw = system_tokens
            .saturating_add(head_images_tokens)
            .saturating_add(head_text_tokens)
            .saturating_add(tail_tokens);
        Self {
            head_images_tokens,
            head_text_tokens,
            tail_messages,
            tail_tokens,
            system_tokens,
            total_estimate: compact::calibrated(raw, calibration),
            last_provider_context,
            calibration,
            live: false,
        }
    }
}

/// One binary occurrence of a history row, for transcript controls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextAttachment {
    /// Its `ContextTarget::Binary` coordinates.
    pub target: ContextTarget,
    /// Display name (file name, else MIME type).
    pub name: String,
    /// Stored MIME type.
    pub mime: String,
    /// Preview file, when materialized by the history builder.
    pub path: Option<PathBuf>,
    /// Excluded from the effective frame.
    pub removed: bool,
}

/// One surface fence or source of a history row, for transcript controls (197 §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextPiece {
    /// Its `ContextTarget::Fence` or `ContextTarget::Source` coordinates.
    pub target: ContextTarget,
    /// The inventory label (diagram types, card name).
    pub label: String,
    /// Excluded from the effective frame.
    pub removed: bool,
}

/// Context metadata of one raw history row (48 §3.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextEntry {
    /// The row's whole-message target.
    pub target: ContextTarget,
    /// In the active frame (at or after `tail_from`, or the current head).
    pub active: bool,
    /// The whole row is excluded.
    pub removed: bool,
    /// Companions a whole-message removal requires (active rows only).
    pub remove_with: Vec<ContextTarget>,
    /// Binary occurrences with their own targets.
    pub attachments: Vec<ContextAttachment>,
    /// Typed consultation metadata when the row is a vision note (52 §6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<crate::vision::VisionNote>,
    /// Wall-clock time the row was appended (RFC 3339 UTC, 74 §5); absent
    /// for rows stored before the column existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub at: Option<String>,
    /// Surface fences of an assistant row, in text order across its parts (197 §5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<ContextPiece>>", optional))]
    pub fences: Vec<ContextPiece>,
    /// `show` / `app` / `page_preview` sources of an assistant row, in part order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<ContextPiece>>", optional))]
    pub sources: Vec<ContextPiece>,
}

/// Result of a committed (or no-op) removal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextRemoval {
    /// Revision after the edit (unchanged for a no-op).
    pub revision: String,
    /// Whole message rows newly excluded (companions included).
    pub messages: u32,
    /// Image occurrences newly excluded (not counting subsumed ones).
    pub images: u32,
    /// Non-image binaries newly excluded.
    pub documents: u32,
    /// 1 when the current head was newly excluded.
    pub heads: u32,
    /// Surface fences and sources newly excluded (not counting subsumed ones).
    #[serde(default)]
    pub surfaces: u32,
    /// Post-edit `compact::est` of the effective frame.
    pub estimated_tokens: u32,
}

/// Raw cut-visible transcript plus its context metadata, read consistently
/// at `revision`. Bodies are the stored rows; nothing is materialized.
#[derive(Clone, Debug)]
pub struct ContextHistory {
    /// Decimal `sessions.context_revision` the rest was read at.
    pub revision: String,
    /// Raw visible messages (turn identity).
    pub messages: Vec<(i64, ChatMessage)>,
    /// Raw visible epochs.
    pub epochs: Vec<Epoch>,
    /// Per message seq.
    pub entries: BTreeMap<i64, ContextEntry>,
    /// 91: ms the model thought before each assistant row that has it, by seq.
    pub thinking: BTreeMap<i64, u64>,
    /// Per epoch number.
    pub heads: BTreeMap<u32, ContextEntry>,
    /// Active inventory at the same revision (`editable` left `true`).
    pub snapshot: ContextSnapshot,
}

/// Neutral body a tool response keeps when the user removed its only content:
/// providers require the result envelope, not an empty array.
pub const IMAGE_REMOVED: &str = "Image removed by the user.";

/// Longest inventory label (chars) before `…`.
const LABEL_CHARS: usize = 80;

// region:    --- Masks

/// One excluded binary coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BinaryAt {
    /// Original content-part index.
    pub part: u32,
    /// `Some`: original `ToolResponse.images` index of that part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<u32>,
}

/// One excluded surface fence: original text part and its fence ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FenceAt {
    /// Original content-part index (a text part).
    pub part: u32,
    /// Ordinal among that part's surface fences ([`surface_fences`]).
    pub index: u32,
}

/// Exclusions of one row, stored as JSON in `messages.excluded` /
/// `epochs.excluded` (`NULL` = empty). Coordinates only, never content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mask {
    /// The whole row is out of the effective frame.
    #[serde(default)]
    pub whole: bool,
    /// Excluded binary occurrences, sorted.
    #[serde(default)]
    pub binaries: Vec<BinaryAt>,
    /// Excluded surface fences, sorted (197 §3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fences: Vec<FenceAt>,
    /// Call parts whose source argument is swapped for a marker, sorted (197 §3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<u32>,
}

impl Mask {
    /// From the stored column; `None`/empty → no exclusions. Malformed JSON is an error:
    /// silently replaying a row the user removed is the one thing this must never do.
    pub fn decode(raw: Option<&str>) -> anyhow::Result<Mask> {
        match raw {
            None => Ok(Mask::default()),
            Some(s) if s.trim().is_empty() => Ok(Mask::default()),
            Some(s) => {
                serde_json::from_str(s).map_err(|e| anyhow::anyhow!("bad exclusion mask: {e}"))
            }
        }
    }

    /// To the stored column; `None` when nothing is excluded.
    pub fn encode(&self) -> anyhow::Result<Option<String>> {
        if self.is_empty() {
            return Ok(None);
        }
        Ok(Some(serde_json::to_string(self)?))
    }

    /// Nothing excluded.
    pub fn is_empty(&self) -> bool {
        !self.whole && !self.partial()
    }

    /// Something inside the row is excluded (binaries, fences or sources).
    pub fn partial(&self) -> bool {
        !self.binaries.is_empty() || !self.fences.is_empty() || !self.sources.is_empty()
    }

    /// `at` is excluded.
    pub fn has(&self, at: BinaryAt) -> bool {
        self.binaries.contains(&at)
    }
}

// endregion: --- Masks

// region:    --- Projection

/// A part that makes a message worth sending; signatures and reasoning
/// alone are not (48 §4.1).
fn meaningful(p: &ContentPart) -> bool {
    match p {
        ContentPart::Text(t) => !t.trim().is_empty(),
        ContentPart::Binary(_)
        | ContentPart::ToolCall(_)
        | ContentPart::ToolResponse(_)
        | ContentPart::Custom(_) => true,
        ContentPart::ThoughtSignature(_) | ContentPart::ReasoningContent(_) => false,
    }
}

/// Effective copy of `m` under `mask`: `None` when the whole row is excluded
/// or its last meaningful part went with the excluded binaries. A tool
/// response whose only content was its images keeps its envelope with
/// [`IMAGE_REMOVED`]. An excluded surface fence becomes its one-line marker
/// inside its text part, an excluded source its marker inside that call's
/// arguments (197 §4). Other parts are byte-identical clones.
pub fn project(m: &ChatMessage, mask: &Mask) -> Option<ChatMessage> {
    if mask.whole {
        return None;
    }
    if !mask.partial() {
        return Some(m.clone());
    }
    let mut parts = Vec::with_capacity(m.content.len());
    for (i, p) in m.content.iter().enumerate() {
        let part = u32::try_from(i).ok()?;
        match p {
            ContentPart::Binary(_) if mask.has(BinaryAt { part, image: None }) => {}
            ContentPart::Text(t)
                if m.role == ChatRole::User
                    && part > 0
                    && parse_text_attachment(t).is_some()
                    && mask.has(BinaryAt { part, image: None }) => {}
            ContentPart::ToolResponse(r)
                if mask
                    .binaries
                    .iter()
                    .any(|b| b.part == part && b.image.is_some()) =>
            {
                let mut r = r.clone();
                let mut k = 0u32;
                r.images.retain(|_| {
                    let keep = !mask.has(BinaryAt {
                        part,
                        image: Some(k),
                    });
                    k += 1;
                    keep
                });
                if r.images.is_empty() && r.content.trim().is_empty() {
                    r.content = IMAGE_REMOVED.to_owned();
                }
                parts.push(ContentPart::ToolResponse(r));
            }
            ContentPart::Text(t) if mask.fences.iter().any(|f| f.part == part) => {
                parts.push(ContentPart::Text(cut_fences(t, |index| {
                    mask.fences.contains(&FenceAt { part, index })
                })));
            }
            ContentPart::ToolCall(c) if mask.sources.contains(&part) => {
                parts.push(ContentPart::ToolCall(cut_source(c)));
            }
            other => parts.push(other.clone()),
        }
    }
    if !parts.iter().any(meaningful) {
        return None;
    }
    Some(ChatMessage {
        role: m.role.clone(),
        content: MessageContent::from_parts(parts),
        options: m.options.clone(),
    })
}

/// [`project`] for an epoch head: when frames were dropped from a snap head
/// its caption stops claiming them (48 §4.1). The rasterized frames that
/// remain are untouched. The epoch/`tail_from` boundary is the caller's:
/// `None` here never means "replay everything".
pub fn project_head(head: &ChatMessage, mask: &Mask) -> Option<ChatMessage> {
    let mut out = project(head, mask)?;
    let binaries = |c: &MessageContent| c.iter().filter(|p| p.is_binary()).count();
    let (before, after) = (binaries(&head.content), binaries(&out.content));
    let first = out.content.iter_mut().next().filter(|_| after < before);
    if let Some(ContentPart::Text(t)) = first {
        *t = partial_caption(t, after, before - after);
    }
    Some(out)
}

/// Rewrite `as N image(s)` in a snap caption to the kept count plus a
/// removal note; a caption without that phrase gets the note as its own line.
fn partial_caption(text: &str, kept: usize, dropped: usize) -> String {
    static FRAMES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"as (\d+) image\(s\)").expect("static regex"));
    match FRAMES.captures(text) {
        Some(c) => {
            let m = c.get(0).expect("whole match");
            let orig = &c[1];
            format!(
                "{}as {kept} image(s) ({dropped} of {orig} removed from context by the user){}",
                &text[..m.start()],
                &text[m.end()..]
            )
        }
        None => {
            let at = text.find('\n').map_or(text.len(), |i| i + 1);
            format!(
                "{}[{dropped} image(s) removed from context by the user]\n{}",
                &text[..at],
                &text[at..]
            )
        }
    }
}

// endregion: --- Projection

// region:    --- Surfaces

/// Fence info strings that are surfaces (197 §1): what the desktop renders as pictures.
const SURFACE_LANGS: [&str; 3] = ["json-render", "openui", "mermaid"];

/// One surface fence of a text part (197 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceFence<'a> {
    /// `json-render`, `openui` or `mermaid`.
    pub lang: &'a str,
    /// From the opening line's start to the closing line's end, its line
    /// break excluded; to the part's last line end when unclosed.
    pub range: std::ops::Range<usize>,
    /// The lines between the fence lines.
    pub body: &'a str,
}

/// The surface fences of one text part, in order: the one scanner the
/// inventory, the projection and the history entries share (197 §3; the
/// desktop applies the same rule to find the fence `index`).
///
/// A fence opens with a line that starts (no indent) with three or more
/// `` ` `` or `~` immediately followed by its info string (a backtick
/// opener's info holds no backtick). It closes at the next line made of the
/// same character, at least as many, then only whitespace; an unclosed fence
/// runs to the end of the part. Only info strings exactly `json-render`,
/// `openui` or `mermaid` (trailing whitespace aside) are surface fences and
/// counted. Every other fence (` ```rust `, a bare ` ``` `) is skipped whole:
/// a surface-looking line inside it is content. `\r\n` line ends are accepted.
pub fn surface_fences(text: &str) -> Vec<SurfaceFence<'_>> {
    let surface = |lang: &str| SURFACE_LANGS.contains(&lang);
    let mut out = Vec::new();
    // (fence char, run length, info, opening line start, body start)
    let mut open: Option<(u8, usize, &str, usize, usize)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let bare = line.trim_end_matches(['\n', '\r']);
        let lead = |c: u8| bare.bytes().take_while(|b| *b == c).count();
        match open {
            None => {
                let Some(c) = bare.bytes().next().filter(|c| matches!(c, b'`' | b'~')) else {
                    continue;
                };
                let run = lead(c);
                let info = bare[run..].trim_end();
                if run >= 3 && !(c == b'`' && info.contains('`')) {
                    open = Some((c, run, info, start, at));
                }
            }
            Some((c, run, lang, from, body)) => {
                let n = lead(c);
                if n >= run && bare[n..].trim().is_empty() {
                    if surface(lang) {
                        out.push(SurfaceFence {
                            lang,
                            range: from..start + bare.len(),
                            body: &text[body..start],
                        });
                    }
                    open = None;
                }
            }
        }
    }
    if let Some((_, _, lang, from, body)) = open
        && surface(lang)
    {
        out.push(SurfaceFence {
            lang,
            range: from..text.trim_end_matches(['\n', '\r']).len(),
            body: &text[body..],
        });
    }
    out
}

/// Up to three distinct json-render element `type`s in the order the
/// `elements` map lists them; empty when the body does not parse.
fn json_render_types(body: &str) -> Vec<String> {
    use serde::de::{IgnoredAny, MapAccess, Visitor};
    #[derive(Deserialize)]
    struct Element {
        #[serde(rename = "type")]
        ty: Option<String>,
    }
    /// The `elements` values in document order (serde_json's map is sorted).
    #[derive(Default)]
    struct Ordered(Vec<String>);
    impl<'de> Deserialize<'de> for Ordered {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Ordered;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an elements map")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                    let mut out: Vec<String> = Vec::new();
                    while let Some((_, e)) = map.next_entry::<IgnoredAny, Element>()? {
                        if let Some(t) = e.ty.filter(|t| out.len() < 3 && !out.contains(t)) {
                            out.push(t);
                        }
                    }
                    Ok(Ordered(out))
                }
            }
            d.deserialize_map(V)
        }
    }
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        elements: Ordered,
    }
    serde_json::from_str::<Doc>(body).map_or_else(|_| Vec::new(), |d| d.elements.0)
}

/// A mermaid body's diagram keyword (`sequenceDiagram`, `flowchart`): the
/// first word after front matter and `%%` lines.
fn mermaid_keyword(body: &str) -> Option<&str> {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut first = lines.next()?;
    if first == "---" {
        lines.by_ref().find(|l| *l == "---")?;
        first = lines.next()?;
    }
    std::iter::once(first)
        .chain(lines)
        .find(|l| !l.starts_with("%%"))?
        .split(|c: char| c.is_whitespace() || c == ';')
        .next()
}

/// An OpenUI Lang body's root component: `root = Stack([...])` → `Stack`.
fn openui_root(body: &str) -> Option<&str> {
    body.lines().find_map(|l| {
        let r = l
            .trim()
            .strip_prefix("root")?
            .trim_start()
            .strip_prefix('=')?;
        let r = r.trim_start();
        let id = &r[..r
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(r.len())];
        (!id.is_empty()).then_some(id)
    })
}

/// Inventory label of a surface fence (197 §1).
fn fence_label(f: &SurfaceFence<'_>) -> String {
    match f.lang {
        "json-render" => match json_render_types(f.body) {
            t if t.is_empty() => "diagram".into(),
            t => preview(&t.join(", ")),
        },
        "mermaid" => preview(mermaid_keyword(f.body).unwrap_or("mermaid")),
        _ => preview(&openui_root(f.body).map_or("openui".into(), |r| format!("openui: {r}"))),
    }
}

/// The one line the model sees in place of a removed fence (197 §4).
fn fence_marker(f: &SurfaceFence<'_>) -> String {
    match f.lang {
        "json-render" => match json_render_types(f.body) {
            t if t.is_empty() => "[diagram removed from context]".into(),
            t => format!("[diagram removed from context: {}]", t.join(", ")),
        },
        "mermaid" => "[mermaid diagram removed from context]".into(),
        _ => "[openui surface removed from context]".into(),
    }
}

/// `text` with each surface fence whose ordinal `removed` accepts replaced
/// by its marker; the text around it, newlines included, unchanged.
fn cut_fences(text: &str, removed: impl Fn(u32) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (k, f) in surface_fences(text).iter().enumerate() {
        if u32::try_from(k).is_ok_and(&removed) {
            out.push_str(&text[at..f.range.start]);
            out.push_str(&fence_marker(f));
            at = f.range.end;
        }
    }
    out.push_str(&text[at..]);
    out
}

/// A call whose source argument can be swapped for a marker (197 §3):
/// `(field, group, value)` for `show` `content` (group `page` for an
/// `.html`/`.htm` name, else `file`), `app` `source` and `page_preview`
/// `html`, when that field is a non-empty string.
fn source_of(c: &ToolCall) -> Option<(&'static str, &'static str, &str)> {
    let (field, group) = match c.fn_name.as_str() {
        "show" => {
            let name = arg(c, "name").unwrap_or("").to_ascii_lowercase();
            let page = name.ends_with(".html") || name.ends_with(".htm");
            ("content", if page { "page" } else { "file" })
        }
        "app" => ("source", "app"),
        "page_preview" => ("html", "page"),
        _ => return None,
    };
    Some((field, group, arg(c, field)?))
}

/// A non-empty string argument of `c`.
fn arg<'a>(c: &'a ToolCall, key: &str) -> Option<&'a str> {
    c.fn_arguments.get(key)?.as_str().filter(|s| !s.is_empty())
}

/// What a card is called: its `name` argument, else `title`, else the tool.
fn card_name(c: &ToolCall) -> &str {
    arg(c, "name")
        .or_else(|| arg(c, "title"))
        .unwrap_or(&c.fn_name)
}

/// `c` with its source argument replaced by `[<group> removed from context:
/// <name>]`; the call id and every other argument unchanged (197 §4).
fn cut_source(c: &ToolCall) -> ToolCall {
    let mut out = c.clone();
    if let Some((field, group, _)) = source_of(c) {
        out.fn_arguments[field] =
            serde_json::Value::String(format!("[{group} removed from context: {}]", card_name(c)));
    }
    out
}

/// One surface fence or source of a row, with its removal state.
struct Surface {
    target: ContextTarget,
    group: &'static str,
    label: String,
    detail: &'static str,
    tokens: u32,
    removed: bool,
}

/// Every surface fence (text order) then every source (part order) of an
/// assistant row under `mask`; nothing for other rows or a head.
fn surfaces(owner: ContextOwner, m: &ChatMessage, mask: &Mask) -> Vec<Surface> {
    let mut out = Vec::new();
    if m.role != ChatRole::Assistant || !matches!(owner, ContextOwner::Message { .. }) {
        return out;
    }
    // compact::est's chars/4.
    let est = |n: usize| u32::try_from(n / 4).unwrap_or(u32::MAX);
    let parts = || {
        m.content
            .iter()
            .enumerate()
            .map_while(|(i, p)| Some((u32::try_from(i).ok()?, p)))
    };
    for (part, p) in parts() {
        let ContentPart::Text(t) = p else { continue };
        for (k, f) in surface_fences(t).iter().enumerate() {
            let Ok(index) = u32::try_from(k) else { break };
            out.push(Surface {
                target: ContextTarget::Fence { owner, part, index },
                group: "diagram",
                label: fence_label(f),
                detail: "in reply",
                tokens: est(f.range.len()),
                removed: mask.whole || mask.fences.contains(&FenceAt { part, index }),
            });
        }
    }
    for (part, p) in parts() {
        let ContentPart::ToolCall(c) = p else {
            continue;
        };
        let Some((_, group, value)) = source_of(c) else {
            continue;
        };
        out.push(Surface {
            target: ContextTarget::Source { owner, part },
            group,
            label: preview(card_name(c)),
            detail: match c.fn_name.as_str() {
                "show" => "show",
                "app" => "app",
                _ => "page_preview",
            },
            tokens: est(value.len()),
            removed: mask.whole || mask.sources.contains(&part),
        });
    }
    out
}

// endregion: --- Surfaces

// region:    --- Tool protocol

/// Connected components of rows joined by tool-call/result identity (48 §4.2),
/// built once per snapshot. A call id with several calls or several results is
/// ambiguous; whole-message removal of that component refuses.
#[derive(Debug, Default)]
pub struct Groups {
    comp: HashMap<i64, usize>,
    members: Vec<Vec<i64>>,
    ambiguous: Vec<bool>,
}

impl Groups {
    /// Over the rows that are in the effective frame (whole-excluded rows are not passed).
    pub fn build<'a>(rows: impl IntoIterator<Item = (i64, &'a ChatMessage)>) -> Self {
        let rows: Vec<(i64, &ChatMessage)> = rows.into_iter().collect();
        let mut parent: Vec<usize> = (0..rows.len()).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let mut first: HashMap<&str, usize> = HashMap::new();
        let mut calls: HashMap<&str, u32> = HashMap::new();
        let mut results: HashMap<&str, u32> = HashMap::new();
        for (i, (_, m)) in rows.iter().enumerate() {
            for p in m.content.iter() {
                let id = match p {
                    ContentPart::ToolCall(c) => {
                        *calls.entry(c.call_id.as_str()).or_default() += 1;
                        c.call_id.as_str()
                    }
                    ContentPart::ToolResponse(r) => {
                        *results.entry(r.call_id.as_str()).or_default() += 1;
                        r.call_id.as_str()
                    }
                    _ => continue,
                };
                let j = *first.entry(id).or_insert(i);
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
        let bad: BTreeSet<&str> = calls
            .iter()
            .chain(results.iter())
            .filter(|(_, n)| **n > 1)
            .map(|(id, _)| *id)
            .collect();
        let mut index: HashMap<usize, usize> = HashMap::new();
        let mut out = Groups::default();
        for (i, (seq, m)) in rows.iter().enumerate() {
            let root = find(&mut parent, i);
            let k = *index.entry(root).or_insert_with(|| {
                out.members.push(Vec::new());
                out.ambiguous.push(false);
                out.members.len() - 1
            });
            out.members[k].push(*seq);
            out.comp.insert(*seq, k);
            if !bad.is_empty()
                && m.content.iter().any(|p| match p {
                    ContentPart::ToolCall(c) => bad.contains(c.call_id.as_str()),
                    ContentPart::ToolResponse(r) => bad.contains(r.call_id.as_str()),
                    _ => false,
                })
            {
                out.ambiguous[k] = true;
            }
        }
        out
    }

    /// Every seq of `seq`'s component, itself included; empty for a row not in the graph.
    pub fn component(&self, seq: i64) -> &[i64] {
        match self.comp.get(&seq) {
            Some(&k) => &self.members[k],
            None => &[],
        }
    }

    /// Companions of `seq` as whole-message targets (itself excluded).
    pub fn remove_with(&self, seq: i64) -> Vec<ContextTarget> {
        self.component(seq)
            .iter()
            .filter(|&&s| s != seq)
            .map(|&s| ContextTarget::Message { seq: s })
            .collect()
    }

    /// `seq`'s component has a repeated call id.
    pub fn ambiguous(&self, seq: i64) -> bool {
        self.comp.get(&seq).is_some_and(|&k| self.ambiguous[k])
    }
}

// endregion: --- Tool protocol

// region:    --- Active frame

/// The persisted active frame with its masks: the latest visible epoch's head
/// and the raw rows from `tail_from` on. Everything the store reads/writes for
/// one edit goes through this.
#[derive(Clone, Debug, Default)]
pub struct Active {
    /// Current epoch; 0 without a head.
    pub epoch: u32,
    /// First seq of the tail.
    pub tail_from: i64,
    /// Last message seq when the epoch was cut; rows at or before it were the
    /// kept tail. `-1` without an epoch, so no row folds.
    pub at_seq: i64,
    /// Head row and its mask.
    pub head: Option<(ChatMessage, Mask)>,
    /// Raw tail rows with their masks, seq ascending.
    pub rows: Vec<(i64, ChatMessage, Mask)>,
    /// Per-row facts beyond the body, by seq, for the inventory (197 §1–§2);
    /// a row without an entry is an untyped row of unknown time.
    pub meta: BTreeMap<i64, RowMeta>,
}

/// What the store knows about a row besides its body and mask.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowMeta {
    /// `messages.at` (RFC 3339 UTC).
    pub at: Option<String>,
    /// A typed note (vision, advisory, moved: the store's `Row::is_note`).
    pub note: bool,
    /// Peer provenance: the sender address (`PeerOrigin::from`).
    pub peer: Option<String>,
    /// The sender's display name, when the store knows one.
    pub sender: Option<String>,
}

/// Masks an edit writes; empty = no-op.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// New mask per changed message row.
    pub rows: BTreeMap<i64, Mask>,
    /// New head mask, when changed.
    pub head: Option<Mask>,
    /// Counts for [`ContextRemoval`].
    pub messages: u32,
    /// Image occurrences newly excluded.
    pub images: u32,
    /// Non-image binaries newly excluded.
    pub documents: u32,
    /// The head was newly excluded whole.
    pub heads: u32,
    /// Surface fences and sources newly excluded.
    pub surfaces: u32,
}

impl Plan {
    /// Nothing to write.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.head.is_none()
    }
}

impl Active {
    /// Rows in the effective frame with their seq.
    fn live(&self) -> impl Iterator<Item = (i64, &ChatMessage)> {
        self.rows
            .iter()
            .filter(|(_, _, mask)| !mask.whole)
            .map(|(s, m, _)| (*s, m))
    }

    /// Protocol groups over the live rows.
    pub fn groups(&self) -> Groups {
        Groups::build(self.live())
    }

    /// The effective head and tail: what `Store::frame` hands the actor.
    pub fn effective(&self) -> (Option<ChatMessage>, Vec<(i64, ChatMessage)>) {
        let head = self
            .head
            .as_ref()
            .and_then(|(h, mask)| project_head(h, mask))
            .map(|mut head| {
                crate::diff_output::strip_message(&mut head);
                head
            });
        let tail = self
            .rows
            .iter()
            .filter_map(|(seq, m, mask)| {
                let mut m = project(m, mask)?;
                crate::diff_output::strip_message(&mut m);
                // 70 COMPACT-SIZE: rows that were the kept tail when the epoch
                // was cut travel with oversize tool results folded, exactly as
                // the actor folded them in memory; rows appended since do not.
                if *seq <= self.at_seq {
                    compact::fold_tool_results(&mut m);
                }
                Some((*seq, m))
            })
            .collect();
        (head, tail)
    }

    /// `compact::est` over the effective frame.
    pub fn estimate(&self) -> u32 {
        let (head, tail) = self.effective();
        tail.iter()
            .map(|(_, m)| compact::est(m))
            .fold(head.as_ref().map_or(0, compact::est), u32::saturating_add)
    }

    fn row(&self, seq: i64) -> Result<(&ChatMessage, &Mask), String> {
        self.rows
            .iter()
            .find(|(s, _, _)| *s == seq)
            .map(|(_, m, mask)| (m, mask))
            .ok_or_else(|| format!("message {seq} is not in the active context"))
    }

    fn head_at(&self, epoch: u32) -> Result<(&ChatMessage, &Mask), String> {
        match &self.head {
            Some((h, mask)) if self.epoch == epoch => Ok((h, mask)),
            _ => Err(format!("epoch {epoch} is not the current head")),
        }
    }

    fn owner(&self, owner: ContextOwner) -> Result<(&ChatMessage, &Mask), String> {
        match owner {
            ContextOwner::Message { seq } => self.row(seq),
            ContextOwner::Head { epoch } => self.head_at(epoch),
        }
    }

    /// The row a fence or source target names; a head has neither (197 §3).
    fn surface_owner(&self, owner: ContextOwner) -> Result<(&ChatMessage, &Mask), String> {
        match owner {
            ContextOwner::Message { seq } => self.row(seq),
            ContextOwner::Head { epoch } => Err(format!(
                "epoch {epoch}: a summary head has no surface fences or sources"
            )),
        }
    }

    /// Active-context inventory (48 §2.1, 197 §1–§2): head, its binaries,
    /// then each live row followed by its binaries, fences and sources, in
    /// seq order. `editable`/`reason` are the actor's. Tokens price the
    /// effective frame, so a whole row counts what is left of it.
    pub fn snapshot(&self, revision: &str, groups: &Groups) -> ContextSnapshot {
        let (head, tail) = self.effective();
        let head_tokens = head.as_ref().map_or(0, compact::est);
        let tokens: HashMap<i64, u32> = tail.iter().map(|(s, m)| (*s, compact::est(m))).collect();
        let estimated_tokens = tail
            .iter()
            .fold(head_tokens, |acc, (s, _)| acc.saturating_add(tokens[s]));
        // Results rarely store their tool's name: the calls in the frame have it.
        let calls: HashMap<&str, &str> = self
            .rows
            .iter()
            .flat_map(|(_, m, _)| m.content.tool_calls())
            .map(|c| (c.call_id.as_str(), c.fn_name.as_str()))
            .collect();
        let mut items = Vec::new();
        if let Some((h, mask)) = &self.head
            && !mask.whole
        {
            items.push(ContextItem {
                target: ContextTarget::Head { epoch: self.epoch },
                kind: ContextKind::Summary,
                group: "summary".into(),
                label: preview(&format!("Summary (epoch {}): {}", self.epoch, head_body(h))),
                detail: None,
                tokens: head_tokens,
                at: None,
                remove_with: Vec::new(),
            });
            binary_items(
                &mut items,
                ContextOwner::Head { epoch: self.epoch },
                h,
                mask,
                None,
                &calls,
            );
        }
        let none = RowMeta::default();
        for (seq, m, mask) in &self.rows {
            if mask.whole {
                continue;
            }
            let owner = ContextOwner::Message { seq: *seq };
            let meta = self.meta.get(seq).unwrap_or(&none);
            let class = classify(m, meta, &calls);
            items.push(ContextItem {
                target: ContextTarget::Message { seq: *seq },
                kind: class.kind,
                group: class.group,
                label: class.label,
                detail: class.detail,
                tokens: tokens.get(seq).copied().unwrap_or(0),
                at: meta.at.clone(),
                remove_with: groups.remove_with(*seq),
            });
            binary_items(&mut items, owner, m, mask, meta.at.as_ref(), &calls);
            items.extend(
                surfaces(owner, m, mask)
                    .into_iter()
                    .filter(|s| !s.removed)
                    .map(|s| ContextItem {
                        target: s.target,
                        kind: ContextKind::Surface,
                        group: s.group.into(),
                        label: s.label,
                        detail: Some(s.detail.into()),
                        tokens: s.tokens,
                        at: meta.at.clone(),
                        remove_with: Vec::new(),
                    }),
            );
        }
        ContextSnapshot {
            revision: revision.to_owned(),
            editable: true,
            reason: None,
            epoch: self.epoch,
            tail_from: self.tail_from,
            items,
            estimated_tokens,
        }
    }

    /// Resolve `targets` against this frame (48 §3.2): deduplicate, refuse
    /// unknown/malformed coordinates, require every protocol companion of a
    /// whole-message target to be included, let whole removal subsume the
    /// row's binaries, fences and sources, and treat exact already-removed
    /// targets as no-ops.
    pub fn plan(&self, targets: &[ContextTarget], groups: &Groups) -> Result<Plan, String> {
        let targets: BTreeSet<ContextTarget> = targets.iter().copied().collect();
        let mut whole_rows: BTreeSet<i64> = BTreeSet::new();
        let mut whole_head = false;
        // Pieces asked for inside each owner, in the shape of a mask.
        let mut want: BTreeMap<ContextOwner, Mask> = BTreeMap::new();
        for t in targets {
            match t {
                ContextTarget::Message { seq } => {
                    self.row(seq)?;
                    whole_rows.insert(seq);
                }
                ContextTarget::Head { epoch } => {
                    self.head_at(epoch)?;
                    whole_head = true;
                }
                ContextTarget::Binary { owner, part, image } => {
                    let (m, _) = self.owner(owner)?;
                    attachment_at(m, part, image)?;
                    want.entry(owner)
                        .or_default()
                        .binaries
                        .push(BinaryAt { part, image });
                }
                ContextTarget::Fence { owner, part, index } => {
                    let (m, _) = self.surface_owner(owner)?;
                    fence_at(m, part, index)?;
                    want.entry(owner)
                        .or_default()
                        .fences
                        .push(FenceAt { part, index });
                }
                ContextTarget::Source { owner, part } => {
                    let (m, _) = self.surface_owner(owner)?;
                    source_at(m, part)?;
                    want.entry(owner).or_default().sources.push(part);
                }
            }
        }
        for &seq in &whole_rows {
            let (_, mask) = self.row(seq)?;
            if mask.whole {
                continue;
            }
            if groups.ambiguous(seq) {
                return Err(format!(
                    "message {seq}: repeated tool call id in its exchange; repair needed"
                ));
            }
            if let Some(c) = groups
                .component(seq)
                .iter()
                .find(|c| !whole_rows.contains(*c))
            {
                return Err(format!("removing message {seq} also requires message {c}"));
            }
        }
        let mut plan = Plan::default();
        for &seq in &whole_rows {
            let (_, mask) = self.row(seq)?;
            if !mask.whole {
                plan.rows.insert(
                    seq,
                    Mask {
                        whole: true,
                        ..mask.clone()
                    },
                );
                plan.messages += 1;
            }
        }
        if whole_head {
            let (_, mask) = self.head_at(self.epoch)?;
            if !mask.whole {
                plan.head = Some(Mask {
                    whole: true,
                    ..mask.clone()
                });
                plan.heads += 1;
            }
        }
        for (owner, want) in want {
            let subsumed = match owner {
                ContextOwner::Message { seq } => whole_rows.contains(&seq),
                ContextOwner::Head { .. } => whole_head,
            };
            let (m, mask) = self.owner(owner)?;
            if subsumed || mask.whole {
                continue;
            }
            let mut new = mask.clone();
            for at in want.binaries {
                if new.has(at) {
                    continue;
                }
                new.binaries.push(at);
                if attachment_at(m, at.part, at.image)?.is_image() {
                    plan.images += 1;
                } else {
                    plan.documents += 1;
                }
            }
            for at in want.fences {
                if !new.fences.contains(&at) {
                    new.fences.push(at);
                    plan.surfaces += 1;
                }
            }
            for part in want.sources {
                if !new.sources.contains(&part) {
                    new.sources.push(part);
                    plan.surfaces += 1;
                }
            }
            if new != *mask {
                new.binaries.sort();
                new.fences.sort();
                new.sources.sort();
                match owner {
                    ContextOwner::Message { seq } => {
                        plan.rows.insert(seq, new);
                    }
                    ContextOwner::Head { .. } => plan.head = Some(new),
                }
            }
        }
        Ok(plan)
    }

    /// This frame with `plan`'s masks in place of the old ones.
    pub fn apply(&self, plan: &Plan) -> Active {
        let mut out = self.clone();
        if let (Some(mask), Some((_, old))) = (&plan.head, out.head.as_mut()) {
            *old = mask.clone();
        }
        for (seq, _, mask) in &mut out.rows {
            if let Some(new) = plan.rows.get(seq) {
                *mask = new.clone();
            }
        }
        out
    }

    /// The projected frame is a valid request shape (48 §4.2). Strict over the
    /// whole frame, not relative to the pre-edit one: an edit that completes a
    /// malformed group is fine, an edit that leaves one is refused, and a
    /// dangling trailing call is refused too (no `repair_dangling` reliance).
    /// Checks: no empty message, calls only in assistant rows and results only
    /// in tool rows, unique call ids on each side, every call batch answered by
    /// the tool rows that immediately follow it, no result without its call.
    pub fn validate(&self) -> Result<(), String> {
        let (head, tail) = self.effective();
        let mut calls: BTreeSet<&str> = BTreeSet::new();
        let mut results: BTreeSet<&str> = BTreeSet::new();
        let mut pending: BTreeSet<&str> = BTreeSet::new();
        for m in head.iter().chain(tail.iter().map(|(_, m)| m)) {
            if m.content.is_empty() {
                return Err(format!("removal would leave an empty {} message", m.role));
            }
            if m.role != ChatRole::Tool
                && let Some(id) = pending.first()
            {
                return Err(format!(
                    "removal would leave tool call {id} without a result"
                ));
            }
            for p in m.content.iter() {
                match p {
                    ContentPart::ToolCall(c) => {
                        if m.role != ChatRole::Assistant {
                            return Err(format!(
                                "removal would leave tool call {} outside an assistant message",
                                c.call_id
                            ));
                        }
                        if !calls.insert(c.call_id.as_str()) {
                            return Err(format!(
                                "removal would leave a repeated tool call {}",
                                c.call_id
                            ));
                        }
                        pending.insert(c.call_id.as_str());
                    }
                    ContentPart::ToolResponse(r) => {
                        if m.role != ChatRole::Tool {
                            return Err(format!(
                                "removal would leave tool result {} outside a tool message",
                                r.call_id
                            ));
                        }
                        if !results.insert(r.call_id.as_str()) {
                            return Err(format!(
                                "removal would leave a repeated tool result {}",
                                r.call_id
                            ));
                        }
                        if !pending.remove(r.call_id.as_str()) {
                            return Err(format!(
                                "removal would leave tool result {} without its call",
                                r.call_id
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        match pending.first() {
            Some(id) => Err(format!(
                "removal would leave tool call {id} without a result"
            )),
            None => Ok(()),
        }
    }
}

/// Leading line of every non-binary user attachment part (inlined text or path pointer).
pub const ATTACHMENT_PREFIX: &str = "the user attached: ";

/// Parsed non-binary user attachment part (inlined fenced body or bare path pointer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextAttachment<'a> {
    pub path: &'a str,
    pub inlined: bool,
}

impl TextAttachment<'_> {
    /// Display file name (last path segment), falling back to the raw path.
    pub(crate) fn file_name(&self) -> String {
        Path::new(self.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(self.path)
            .to_owned()
    }

    /// Best-effort MIME type for the attached file chip (overrides `mime_guess`
    /// where TypeScript `.ts` is mislabelled as `video/mp2t` and gives inlined
    /// extensionless files `text/plain`).
    pub(crate) fn mime(&self) -> String {
        let path = Path::new(self.path);
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_ascii_lowercase().as_str() {
                "svg" => return "image/svg+xml".into(),
                "md" | "markdown" | "mdx" => return "text/markdown".into(),
                "ts" | "mts" | "cts" | "tsx" if self.inlined => return "text/typescript".into(),
                "toml" => return "application/toml".into(),
                "yaml" | "yml" => return "application/yaml".into(),
                _ => {}
            }
        }
        match mime_guess::from_path(path).first() {
            Some(m) if m == mime_guess::mime::APPLICATION_OCTET_STREAM && self.inlined => {
                "text/plain".into()
            }
            Some(m) => m.essence_str().to_owned(),
            None if self.inlined => "text/plain".into(),
            None => "application/octet-stream".into(),
        }
    }
}

/// Parse a user message text part produced by `attachment_part`: either a bare
/// pointer line `the user attached: <path>` or a fenced inline body
/// `the user attached: <path>\n\n<fence><info>\n<body><fence>`.
pub(crate) fn parse_text_attachment(s: &str) -> Option<TextAttachment<'_>> {
    let rest = s.strip_prefix(ATTACHMENT_PREFIX)?;
    match rest.split_once('\n') {
        None => {
            if rest.is_empty() || rest.contains('\r') {
                return None;
            }
            Some(TextAttachment {
                path: rest,
                inlined: false,
            })
        }
        Some((path, after_first_nl)) => {
            if path.is_empty() || path.contains('\r') {
                return None;
            }
            let after_blank = after_first_nl.strip_prefix('\n')?;
            let fence_len = after_blank.bytes().take_while(|&b| b == b'`').count();
            if fence_len < 3 {
                return None;
            }
            let after_fence = &after_blank[fence_len..];
            let (info, body_and_close) = after_fence.split_once('\n')?;
            if !info.is_empty()
                && (info.len() > 16 || !info.bytes().all(|b| b.is_ascii_alphanumeric()))
            {
                return None;
            }
            let closing = &after_blank[..fence_len];
            let before_close = body_and_close.strip_suffix(closing)?;
            if !before_close.is_empty() && !before_close.ends_with('\n') {
                return None;
            }
            Some(TextAttachment {
                path,
                inlined: true,
            })
        }
    }
}

enum AttachmentRef<'a> {
    Binary(&'a Binary),
    Text,
}

impl AttachmentRef<'_> {
    fn is_image(&self) -> bool {
        match self {
            Self::Binary(b) => b.is_image(),
            Self::Text => false,
        }
    }
}

/// The binary or user text-attachment at `(part, image)` of `m`, or why the
/// coordinates are malformed.
fn attachment_at(
    m: &ChatMessage,
    part: u32,
    image: Option<u32>,
) -> Result<AttachmentRef<'_>, String> {
    let p = m
        .content
        .parts()
        .get(part as usize)
        .ok_or_else(|| format!("part {part} does not exist"))?;
    match (p, image) {
        (ContentPart::Binary(b), None) => Ok(AttachmentRef::Binary(b)),
        (ContentPart::ToolResponse(r), Some(k)) => r
            .images
            .get(k as usize)
            .map(AttachmentRef::Binary)
            .ok_or_else(|| format!("part {part} has no image {k}")),
        (ContentPart::Text(t), None)
            if m.role == ChatRole::User && part > 0 && parse_text_attachment(t).is_some() =>
        {
            Ok(AttachmentRef::Text)
        }
        (ContentPart::Binary(_), Some(_)) => {
            Err(format!("part {part} is a binary, not a tool response"))
        }
        (ContentPart::ToolResponse(_), None) => {
            Err(format!("part {part} is a tool response; name its image"))
        }
        _ => Err(format!("part {part} is not a binary")),
    }
}

/// Check that `(part, index)` names a surface fence of assistant text in `m`.
fn fence_at(m: &ChatMessage, part: u32, index: u32) -> Result<(), String> {
    let p = m
        .content
        .parts()
        .get(part as usize)
        .ok_or_else(|| format!("part {part} does not exist"))?;
    let ContentPart::Text(t) = p else {
        return Err(format!("part {part} is not a text part"));
    };
    if m.role != ChatRole::Assistant {
        return Err(format!("part {part} is not assistant text"));
    }
    if surface_fences(t).len() <= index as usize {
        return Err(format!("part {part} has no surface fence {index}"));
    }
    Ok(())
}

/// Check that `part` of `m` is a call with a removable source (197 §3).
fn source_at(m: &ChatMessage, part: u32) -> Result<(), String> {
    match m.content.parts().get(part as usize) {
        None => Err(format!("part {part} does not exist")),
        Some(ContentPart::ToolCall(c)) if source_of(c).is_some() => Ok(()),
        Some(ContentPart::ToolCall(_)) => Err(format!(
            "part {part} has no show, app or page_preview source"
        )),
        Some(_) => Err(format!("part {part} is not a tool call")),
    }
}

/// Every binary and user file-attachment occurrence of `m` with its removal state under `mask`.
pub fn attachments(owner: ContextOwner, m: &ChatMessage, mask: &Mask) -> Vec<ContextAttachment> {
    let mut out = Vec::new();
    let one = |b: &Binary, part: u32, image: Option<u32>| ContextAttachment {
        target: ContextTarget::Binary { owner, part, image },
        name: binary_name(b),
        mime: b.content_type.clone(),
        path: None,
        removed: mask.whole || mask.has(BinaryAt { part, image }),
    };
    for (i, p) in m.content.iter().enumerate() {
        let Ok(part) = u32::try_from(i) else { break };
        match p {
            ContentPart::Binary(b) => out.push(one(b, part, None)),
            ContentPart::ToolResponse(r) => {
                out.extend(
                    r.images
                        .iter()
                        .enumerate()
                        .map(|(k, b)| one(b, part, Some(k as u32))),
                );
            }
            ContentPart::Text(t) if m.role == ChatRole::User && part > 0 => {
                if let Some(att) = parse_text_attachment(t) {
                    out.push(ContextAttachment {
                        target: ContextTarget::Binary {
                            owner,
                            part,
                            image: None,
                        },
                        name: att.file_name(),
                        mime: att.mime(),
                        path: Some(PathBuf::from(att.path)),
                        removed: mask.whole || mask.has(BinaryAt { part, image: None }),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// Context metadata of one raw row for history rendering.
pub fn entry(
    owner: ContextOwner,
    m: &ChatMessage,
    mask: &Mask,
    active: bool,
    remove_with: Vec<ContextTarget>,
) -> ContextEntry {
    let target = match owner {
        ContextOwner::Message { seq } => ContextTarget::Message { seq },
        ContextOwner::Head { epoch } => ContextTarget::Head { epoch },
    };
    let (fences, sources) = surfaces(owner, m, mask)
        .into_iter()
        .map(|s| ContextPiece {
            target: s.target,
            label: s.label,
            removed: s.removed,
        })
        .partition(|p| matches!(p.target, ContextTarget::Fence { .. }));
    ContextEntry {
        target,
        active,
        removed: mask.whole,
        remove_with,
        attachments: attachments(owner, m, mask),
        vision: None,
        at: None,
        fences,
        sources,
    }
}

/// File name of a binary, else its MIME type. Never a directory component.
pub(crate) fn binary_name(b: &Binary) -> String {
    let name = b.name.as_deref().unwrap_or(&b.content_type);
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name)
        .to_owned()
}

fn is_image_mime(mime: &str) -> bool {
    mime.trim().to_ascii_lowercase().starts_with("image/")
}

/// Tools whose result images are screenshots (197 §1 `image` · `screenshot`).
const SCREENSHOT_TOOLS: [&str; 2] = ["computer_screenshot", "browser_screenshot"];

/// Longest inventory detail (chars) before `…`.
const DETAIL_CHARS: usize = 60;

/// Base64 chars of a binary decoded for its header (48 KiB of bytes).
// ponytail: a JPEG whose frame header sits past this (a huge EXIF block) reads format-only.
const HEADER_B64: usize = 64 * 1024;

/// The live, unmasked binary occurrences of `m` as inventory items (197 §1):
/// a head's are `image` · `summary`; direct binaries of a row are
/// `attachment`; tool-result images are `screenshot` for the screenshot
/// tools and `tool` otherwise; non-images are `document`.
fn binary_items(
    out: &mut Vec<ContextItem>,
    owner: ContextOwner,
    m: &ChatMessage,
    mask: &Mask,
    at: Option<&String>,
    calls: &HashMap<&str, &str>,
) {
    let head = matches!(owner, ContextOwner::Head { .. });
    let one = |out: &mut Vec<ContextItem>,
               part: u32,
               image: Option<u32>,
               b: &Binary,
               tokens: u32,
               tool: Option<&str>| {
        if mask.has(BinaryAt { part, image }) {
            return;
        }
        let (kind, group) = match (is_image_mime(&b.content_type), head, tool) {
            (false, _, _) => (ContextKind::Document, "document"),
            (true, true, _) => (ContextKind::Image, "summary"),
            (true, false, None) => (ContextKind::Image, "attachment"),
            (true, false, Some(t)) if SCREENSHOT_TOOLS.contains(&t) => {
                (ContextKind::Image, "screenshot")
            }
            (true, false, Some(_)) => (ContextKind::Image, "tool"),
        };
        let name = match (&b.name, tool) {
            (None, Some(t)) if !t.is_empty() => t.to_owned(),
            _ => binary_name(b),
        };
        out.push(ContextItem {
            target: ContextTarget::Binary { owner, part, image },
            kind,
            group: group.into(),
            label: preview(&if head {
                format!("Summary image: {name}")
            } else {
                name
            }),
            detail: Some(binary_detail(b)),
            tokens,
            at: at.cloned(),
            remove_with: Vec::new(),
        });
    };
    for (i, p) in m.content.iter().enumerate() {
        let Ok(part) = u32::try_from(i) else { break };
        match p {
            ContentPart::Binary(b) => one(out, part, None, b, compact::est_part(p), None),
            ContentPart::ToolResponse(r) => {
                let tool = r
                    .fn_name
                    .as_deref()
                    .or_else(|| calls.get(r.call_id.as_str()).copied())
                    .or(Some(""));
                for (k, b) in r.images.iter().enumerate() {
                    let Ok(k) = u32::try_from(k) else { break };
                    // `compact::est_part` charges a tool image flat.
                    one(out, part, Some(k), b, compact::IMAGE_TOKENS, tool);
                }
            }
            ContentPart::Text(t) if m.role == ChatRole::User && part > 0 => {
                if let Some(att) = parse_text_attachment(t)
                    && !mask.has(BinaryAt { part, image: None })
                {
                    let mime = att.mime();
                    let format = mime.rsplit('/').next().unwrap_or(&mime).to_ascii_uppercase();
                    out.push(ContextItem {
                        target: ContextTarget::Binary {
                            owner,
                            part,
                            image: None,
                        },
                        kind: ContextKind::Document,
                        group: "document".into(),
                        label: preview(&att.file_name()),
                        detail: Some(bound(&format, DETAIL_CHARS)),
                        tokens: compact::est_part(p),
                        at: at.cloned(),
                        remove_with: Vec::new(),
                    });
                }
            }
            _ => {}
        }
    }
}

/// `PNG · 1568×1103`: the MIME subtype uppercased, with the dimensions from
/// the stored header for PNG, JPEG, GIF and WebP; the format alone otherwise.
fn binary_detail(b: &Binary) -> String {
    let mime = b.content_type.split(';').next().unwrap_or("").trim();
    let format = mime.rsplit('/').next().unwrap_or(mime).to_ascii_uppercase();
    let dims = match &b.source {
        genai::chat::BinarySource::Base64(data) => {
            let n = data.len().min(HEADER_B64) / 4 * 4;
            BASE64
                .decode(&data.as_bytes()[..n])
                .ok()
                .and_then(|h| image_dims(&h))
        }
        _ => None,
    };
    bound(
        &match dims {
            Some((w, h)) => format!("{format} · {w}×{h}"),
            None => format,
        },
        DETAIL_CHARS,
    )
}

/// Width and height from the first bytes of a PNG, GIF, WebP or JPEG file.
fn image_dims(b: &[u8]) -> Option<(u32, u32)> {
    let at = |i: usize, n: usize| b.get(i..i + n);
    let be16 = |i| at(i, 2).map(|s| u32::from(u16::from_be_bytes([s[0], s[1]])));
    let le16 = |i| at(i, 2).map(|s| u32::from(u16::from_le_bytes([s[0], s[1]])));
    let le24 = |i| at(i, 3).map(|s| u32::from_le_bytes([s[0], s[1], s[2], 0]));
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        let be32 = |i| at(i, 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"RIFF") && at(8, 4) == Some(b"WEBP") {
        return match at(12, 4)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let v = le24(21)? | (u32::from(*b.get(24)?) << 24);
                Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        // Walk the marker segments to the first start-of-frame.
        let mut i = 2;
        while *b.get(i)? == 0xFF {
            let marker = *b.get(i + 1)?;
            match marker {
                0xFF => i += 1,
                0x01 | 0xD0..=0xD8 => i += 2,
                0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                    return Some((be16(i + 7)?, be16(i + 5)?));
                }
                _ => i += 2 + usize::try_from(be16(i + 2)?).ok()?,
            }
        }
    }
    None
}

/// Category, group, label and detail of a live row (197 §1).
struct Class {
    kind: ContextKind,
    group: String,
    label: String,
    detail: Option<String>,
}

/// Classify a live row. Tool exchanges first (group: the row's first tool
/// name, results named by their call when they do not store one); then, for
/// user rows, peer provenance, a typed note, and the transcript's harness
/// rule ([`injected`]); what is left is the user's.
fn classify(m: &ChatMessage, meta: &RowMeta, calls: &HashMap<&str, &str>) -> Class {
    let tool = |what: &str, names: Vec<&str>| {
        let mut distinct: Vec<&str> = Vec::new();
        for n in names {
            if !distinct.contains(&n) {
                distinct.push(n);
            }
        }
        Class {
            kind: ContextKind::Tool,
            group: distinct.first().copied().unwrap_or("tool").to_owned(),
            label: preview(&format!("{what}: {}", distinct.join(", "))),
            detail: (distinct.len() > 1).then(|| bound(&distinct[1..].join(", "), DETAIL_CHARS)),
        }
    };
    let responses = m.content.tool_responses();
    if !responses.is_empty() {
        let names = responses
            .iter()
            .map(|r| {
                r.fn_name
                    .as_deref()
                    .or_else(|| calls.get(r.call_id.as_str()).copied())
                    .unwrap_or(r.call_id.as_str())
            })
            .collect();
        return tool("tool result", names);
    }
    // `text_of` (binary tags included) for the rules; the words alone for labels.
    let text = text_of(&m.content);
    let words = match m.content.texts().join("\n") {
        w if w.trim().is_empty() => text.clone(),
        w => w,
    };
    let note = |group: &str| Class {
        kind: ContextKind::Note,
        group: group.to_owned(),
        label: match preview(wrapper_body(crate::clock::strip(&words))) {
            l if l.is_empty() => preview(&words),
            l => l,
        },
        detail: None,
    };
    match m.role {
        ChatRole::Assistant if text.trim().is_empty() && !m.content.tool_calls().is_empty() => {
            let names = m
                .content
                .tool_calls()
                .iter()
                .map(|c| c.fn_name.as_str())
                .collect();
            tool("tool call", names)
        }
        ChatRole::Assistant => Class {
            kind: ContextKind::Assistant,
            group: "assistant".into(),
            label: preview(&words),
            detail: None,
        },
        ChatRole::Tool => tool("tool result", Vec::new()),
        ChatRole::System => note("notice"),
        ChatRole::User => match &meta.peer {
            Some(address) => Class {
                kind: ContextKind::Peer,
                group: "peer".into(),
                label: preview(&format!(
                    "{}: {}",
                    meta.sender.as_deref().unwrap_or(address),
                    preview(wrapper_body(crate::clock::strip(&words)))
                )),
                detail: Some(bound(address, DETAIL_CHARS)),
            },
            None if meta.note => note("notice"),
            None => match injected(&text) {
                Some(group) => note(group),
                None => Class {
                    kind: ContextKind::User,
                    group: "user".into(),
                    label: preview(crate::clock::strip(&words)),
                    detail: None,
                },
            },
        },
    }
}

/// Harness wrappers that carry attributes (or none): the transcript cards
/// or hides them (desktop `lib/injected.ts` `classify`, TUI
/// `transcript::injected`).
const HARNESS_WRAPPERS: &[&str] = &[
    "subagent_result",
    "message",
    "job_result",
    "job_idle",
    "reminder",
    "schedule",
    "desk",
    "advisory",
    "child_lost",
    "child_stalled",
    "peer_state",
    "autoresearch",
    "cell_result",
    "cell_error",
    "budget",
    "resume_handoff",
    "context_summary",
    "runtime",
    "surface_error",
    "notebook",
    "notebook_changes",
    "contact_message",
];

/// Engine notes that are harness rows only as a bare tag (a human's
/// `<rounds source="user">` stays text).
const HARNESS_NOTES: &[&str] = &[
    "todo",
    "knowledge",
    "todo_continue",
    "rounds",
    "context_pressure",
    "recovery",
    "continue",
    "steer",
    "vision",
    "nudge",
    "journal",
    "memory",
];

/// Self-closing harness notes (183 §11).
const HARNESS_SELF_CLOSING: &[&str] = &["session_moved", "engine_restart", "contact_inbox"];

/// The `note` group of a harness-injected user row, `None` for the user's
/// own words. The rule is the transcript's (desktop `classify`, TUI
/// `transcript::injected`): after the model's `<time …/>` stamp the WHOLE
/// text is harness elements; leading text, a skill chip or one unknown tag
/// makes it the user's. A row that is only the stamp is `time`; otherwise
/// the first element picks `knowledge`, `todo` or `notice`.
fn injected(text: &str) -> Option<&'static str> {
    let stamped = text.trim_start().starts_with("<time ");
    let mut rest = crate::clock::strip(text).trim();
    if rest.is_empty() {
        return stamped.then_some("time");
    }
    let mut group = None;
    while !rest.is_empty() {
        // A desk's wake (roster 67 §7.3) is the last block and not tag-wrapped.
        if rest.starts_with("Wake: ") {
            return Some(group.unwrap_or("notice"));
        }
        let open = rest.strip_prefix('<')?;
        let end = open.find(|c: char| c == '>' || c == '/' || c.is_whitespace())?;
        let name = &open[..end];
        let bare = open[end..].starts_with('>');
        group.get_or_insert(match name {
            "knowledge" => "knowledge",
            "todo" => "todo",
            _ => "notice",
        });
        if HARNESS_SELF_CLOSING.contains(&name) {
            if name == "contact_inbox" {
                // Its notice line and the contact's first words ride on the card.
                return group;
            }
            rest = rest[rest.find("/>")? + 2..].trim_start();
            continue;
        }
        if !(HARNESS_WRAPPERS.contains(&name) || bare && HARNESS_NOTES.contains(&name)) {
            return None;
        }
        let close = format!("</{name}>");
        // A contact's untrusted words cannot end their card early.
        let at = if name == "contact_message" {
            rest.rfind(&close)?
        } else {
            rest.find(&close)?
        };
        rest = rest[at + close.len()..].trim_start();
    }
    group
}

/// The inside of a leading wrapper element (`<knowledge>\nX\n</knowledge>` →
/// `X`…); `text` itself when it does not open with a tag.
fn wrapper_body(text: &str) -> &str {
    let t = text.trim();
    if !t.starts_with('<') {
        return t;
    }
    let start = t.find('>').map_or(0, |i| i + 1);
    let end = t.rfind("</").filter(|e| *e >= start).unwrap_or(t.len());
    &t[start..end]
}

/// The caption/summary text of a head without its `<context_summary>` tags.
fn head_body(h: &ChatMessage) -> String {
    text_of(&h.content)
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty() && !l.starts_with("<context_summary") && *l != compact::HEAD_CLOSE
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// First non-empty line of `text`, at most [`LABEL_CHARS`] chars, `…` when cut.
pub fn preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    bound(line, LABEL_CHARS)
}

/// `text` cut to `max` chars, `…` when cut.
fn bound(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

// endregion: --- Active frame

/// Write `bytes` as `<data_dir>/attachments/<sha256[..16]>[.<ext>]` (no trailing
/// dot when `ext` is empty); an existing file is not rewritten. The one
/// attachment-store naming rule, shared by RPC `put_attachment`, the ACP prompt
/// path and history preview materialization.
pub fn save_blob(bytes: &[u8], ext: &str, data_dir: &Path) -> std::io::Result<PathBuf> {
    let hex = hex::encode(Sha256::digest(bytes));
    let dir = data_dir.join("attachments");
    let path = if ext.is_empty() {
        dir.join(&hex[..16])
    } else {
        dir.join(format!("{}.{ext}", &hex[..16]))
    };
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn diff_carrier_rebased_context_strips_before_kept_tail_folding() {
        for at_seq in [-1, 10] {
            let mut active = frame();
            active.at_seq = at_seq;
            let carried = crate::diff_output::carried("receipt");
            active.rows[2].1 = ChatMessage::from(vec![ToolResponse::new("c1", carried.clone())]);
            let (_, tail) = active.effective();
            assert_eq!(tail[2].1.content.tool_responses()[0].content, "receipt");
            assert_eq!(
                active.rows[2].1.content.tool_responses()[0].content,
                carried
            );
        }
    }

    use genai::chat::{ToolCall, ToolResponse};

    use super::*;

    fn img(name: &str) -> Binary {
        Binary::from_base64("image/png", "AAAA", Some(name.into()))
    }

    fn pdf(name: &str) -> Binary {
        Binary::from_base64("application/pdf", "JVBERi0=", Some(name.into()))
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            fn_name: "shell".into(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }
    }

    fn at(part: u32, image: Option<u32>) -> BinaryAt {
        BinaryAt { part, image }
    }

    fn bins(binaries: Vec<BinaryAt>) -> Mask {
        Mask {
            binaries,
            ..Mask::default()
        }
    }

    fn whole() -> Mask {
        Mask {
            whole: true,
            ..Mask::default()
        }
    }

    fn row(seq: i64, m: ChatMessage) -> (i64, ChatMessage, Mask) {
        (seq, m, Mask::default())
    }

    fn bin(seq: i64, part: u32, image: Option<u32>) -> ContextTarget {
        ContextTarget::Binary {
            owner: ContextOwner::Message { seq },
            part,
            image,
        }
    }

    fn msg(seq: i64) -> ContextTarget {
        ContextTarget::Message { seq }
    }

    /// user text + A/B, tool text + C/D, later reply (48 §7.1).
    fn frame() -> Active {
        let user = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("look".into()),
            ContentPart::Binary(img("a.png")),
            ContentPart::Binary(img("same.png")),
            ContentPart::Binary(pdf("spec.pdf")),
        ]));
        let asst = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::ThoughtSignature(genai::chat::ThoughtSignature::new("sig")),
            ContentPart::Text("checking".into()),
            ContentPart::ToolCall(call("c1")),
            ContentPart::ToolCall(call("c2")),
        ]));
        let tool = ChatMessage::from(vec![
            ToolResponse::new("c1", "out").with_images(vec![img("c.png"), img("same.png")]),
            ToolResponse::new("c2", "").with_images(vec![img("d.png")]),
        ]);
        Active {
            epoch: 0,
            tail_from: 0,
            at_seq: -1,
            head: None,
            rows: vec![
                row(0, user),
                row(1, asst),
                row(2, tool),
                row(3, ChatMessage::assistant(MessageContent::from_text("done"))),
            ],
            meta: BTreeMap::new(),
        }
    }

    #[test]
    fn mask_round_trips_and_null_means_empty() {
        assert_eq!(Mask::decode(None).unwrap(), Mask::default());
        assert_eq!(Mask::default().encode().unwrap(), None);
        let m = bins(vec![at(1, None), at(2, Some(1))]);
        let s = m.encode().unwrap().unwrap();
        assert_eq!(Mask::decode(Some(&s)).unwrap(), m);
        assert!(Mask::decode(Some("{nope")).is_err());
    }

    #[test]
    fn project_drops_exact_occurrences_by_original_index() {
        let f = frame();
        // A and the PDF gone; B (same name as a tool image) stays.
        let mask = bins(vec![at(1, None), at(3, None)]);
        let got = project(&f.rows[0].1, &mask).unwrap();
        let names: Vec<_> = got
            .content
            .binaries()
            .iter()
            .map(|b| b.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["same.png"]);
        assert_eq!(got.content.texts(), ["look"]);
        // Tool image C: text, call id and sibling image D untouched; c2's
        // image-only body keeps its envelope with the neutral marker.
        let mask = bins(vec![at(0, Some(0)), at(1, Some(0))]);
        let got = project(&f.rows[2].1, &mask).unwrap();
        let rs = got.content.tool_responses();
        assert_eq!(
            (rs[0].call_id.as_str(), rs[0].content.as_str()),
            ("c1", "out")
        );
        assert_eq!(rs[0].images[0].name.as_deref(), Some("same.png"));
        assert_eq!(
            (rs[1].call_id.as_str(), rs[1].content.as_str()),
            ("c2", IMAGE_REMOVED)
        );
        assert!(rs[1].images.is_empty());
        // Unmasked rows are clones; whole masks omit.
        assert_eq!(
            serde_json::to_string(&project(&f.rows[1].1, &Mask::default()).unwrap()).unwrap(),
            serde_json::to_string(&f.rows[1].1).unwrap()
        );
        assert!(project(&f.rows[1].1, &whole()).is_none());
    }

    #[test]
    fn image_only_user_row_empties_out() {
        let m = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("  ".into()),
            ContentPart::Binary(img("only.png")),
        ]));
        assert!(project(&m, &bins(vec![at(1, None)])).is_none());
    }

    #[test]
    fn partial_snap_head_recaptions_and_keeps_frames() {
        let caption = "Archive of 9 folded messages as 3 image(s), most recent last. Read them.";
        let head = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(compact::wrap_head_open(
                2,
                compact::CompactStyle::Snap,
                caption,
                Vec::<String>::new(),
            )),
            ContentPart::Binary(img("frame-0.png")),
            ContentPart::Binary(img("frame-1.png")),
            ContentPart::Binary(img("frame-2.png")),
            ContentPart::Text(compact::HEAD_CLOSE.into()),
        ]));
        let mask = bins(vec![at(2, None)]);
        let got = project_head(&head, &mask).unwrap();
        let text = text_of(&got.content);
        assert!(
            text.contains("as 2 image(s) (1 of 3 removed from context by the user)"),
            "{text}"
        );
        assert!(!text.contains("3 image(s),"));
        assert_eq!(compact::style_of_head(&text), compact::CompactStyle::Snap);
        let names: Vec<_> = got
            .content
            .binaries()
            .iter()
            .map(|b| b.name.clone().unwrap())
            .collect();
        assert_eq!(names, ["frame-0.png", "frame-2.png"]);
        // Untouched head: byte-identical; whole: gone.
        assert_eq!(
            serde_json::to_string(&project_head(&head, &Mask::default()).unwrap()).unwrap(),
            serde_json::to_string(&head).unwrap()
        );
        assert!(project_head(&head, &whole()).is_none());
    }

    #[test]
    fn groups_join_calls_and_results_and_flag_duplicates() {
        let f = frame();
        let g = f.groups();
        assert_eq!(g.component(1), [1, 2]);
        assert_eq!(g.remove_with(2), [msg(1)]);
        assert!(g.remove_with(0).is_empty());
        assert!(g.remove_with(3).is_empty());
        assert!(!g.ambiguous(1));
        // Batched responses across two tool rows form one component.
        let split = Active {
            rows: vec![
                row(0, ChatMessage::user("u")),
                row(1, ChatMessage::from(vec![call("c1"), call("c2")])),
                row(2, ChatMessage::from(vec![ToolResponse::new("c1", "x")])),
                row(3, ChatMessage::from(vec![ToolResponse::new("c2", "y")])),
            ],
            ..Active::default()
        };
        assert_eq!(split.groups().component(3), [1, 2, 3]);
        // A repeated call id is ambiguous, and whole removal refuses.
        let dup = Active {
            rows: vec![
                row(0, ChatMessage::from(vec![call("c1")])),
                row(1, ChatMessage::from(vec![ToolResponse::new("c1", "x")])),
                row(2, ChatMessage::from(vec![call("c1")])),
                row(3, ChatMessage::from(vec![ToolResponse::new("c1", "y")])),
            ],
            ..Active::default()
        };
        let g = dup.groups();
        assert!(g.ambiguous(0));
        let err = dup.plan(&[msg(0), msg(1), msg(2), msg(3)], &g).unwrap_err();
        assert!(err.contains("repair needed"), "{err}");
    }

    #[test]
    fn plan_requires_closure_dedups_and_subsumes() {
        let f = frame();
        let g = f.groups();
        // Incomplete protocol set refuses and names the companion.
        let err = f.plan(&[msg(2)], &g).unwrap_err();
        assert!(err.contains("requires message 1"), "{err}");
        // Whole group: two message rows, the tool image target is subsumed
        // (not counted), duplicates collapse.
        let p = f
            .plan(&[msg(1), msg(2), msg(1), bin(2, 0, Some(0))], &g)
            .unwrap();
        assert_eq!((p.messages, p.images, p.documents, p.heads), (2, 0, 0, 0));
        assert!(p.rows[&1].whole && p.rows[&2].whole);
        // Later reply survives; no orphan.
        let after = f.apply(&p);
        after.validate().unwrap();
        let (_, tail) = after.effective();
        assert_eq!(tail.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [0, 3]);
        // Exact image + document occurrences.
        let p = f
            .plan(&[bin(0, 1, None), bin(0, 3, None), bin(2, 0, Some(0))], &g)
            .unwrap();
        assert_eq!((p.messages, p.images, p.documents), (0, 2, 1));
        assert_eq!(p.rows[&0].binaries, [at(1, None), at(3, None)]);
        assert_eq!(p.rows[&2].binaries, [at(0, Some(0))]);
        // Already removed exact target is a no-op; a second edit still uses original indices.
        let after = f.apply(&p);
        let g2 = after.groups();
        assert!(after.plan(&[bin(0, 1, None)], &g2).unwrap().is_empty());
        let p2 = after.plan(&[bin(0, 2, None)], &g2).unwrap();
        assert_eq!(
            p2.rows[&0].binaries,
            [at(1, None), at(2, None), at(3, None)]
        );
        let (_, tail) = after.apply(&p2).effective();
        assert!(tail[0].1.content.binaries().is_empty());
        assert_eq!(tail[0].1.content.texts(), ["look"]);
    }

    #[test]
    fn validate_is_strict_over_the_post_edit_frame() {
        let tail = |rows: Vec<(i64, ChatMessage, Mask)>| Active {
            rows,
            ..Active::default()
        };
        let refuses = |f: &Active, needle: &str| {
            let err = f.validate().unwrap_err();
            assert!(err.contains(needle), "{err}");
        };
        // Well-formed: the fixture, and a batch answered across two tool rows.
        frame().validate().unwrap();
        let split = tail(vec![
            row(0, ChatMessage::user("u")),
            row(1, ChatMessage::from(vec![call("c1"), call("c2")])),
            row(2, ChatMessage::from(vec![ToolResponse::new("c1", "x")])),
            row(3, ChatMessage::from(vec![ToolResponse::new("c2", "y")])),
        ]);
        split.validate().unwrap();
        // A binary-only edit cannot repair a malformed frame, so it is refused
        // even though it did not create the fault: trailing dangling call...
        let user = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("look".into()),
            ContentPart::Binary(img("a.png")),
        ]));
        let dangling = tail(vec![
            row(0, user.clone()),
            row(1, ChatMessage::from(vec![call("c1")])),
        ]);
        let g = dangling.groups();
        let p = dangling.plan(&[bin(0, 1, None)], &g).unwrap();
        assert_eq!(p.images, 1);
        refuses(&dangling.apply(&p), "tool call c1 without a result");
        // ...repeated call id on either side...
        let dup_call = tail(vec![
            row(0, user.clone()),
            row(1, ChatMessage::from(vec![call("c1")])),
            row(2, ChatMessage::from(vec![ToolResponse::new("c1", "x")])),
            row(3, ChatMessage::from(vec![call("c1")])),
            row(4, ChatMessage::from(vec![ToolResponse::new("c1", "y")])),
        ]);
        refuses(&dup_call, "repeated tool call c1");
        let dup_result = tail(vec![
            row(0, user.clone()),
            row(1, ChatMessage::from(vec![call("c1")])),
            row(
                2,
                ChatMessage::from(vec![
                    ToolResponse::new("c1", "x"),
                    ToolResponse::new("c1", "y"),
                ]),
            ),
        ]);
        refuses(&dup_result, "repeated tool result c1");
        // ...result before its call, or a batch split by a non-tool row...
        let reversed = tail(vec![
            row(0, user.clone()),
            row(1, ChatMessage::from(vec![ToolResponse::new("c1", "x")])),
            row(2, ChatMessage::from(vec![call("c1")])),
            row(3, ChatMessage::from(vec![ToolResponse::new("c1", "y")])),
        ]);
        refuses(&reversed, "tool result c1 without its call");
        let interleaved = tail(vec![
            row(0, user.clone()),
            row(1, ChatMessage::from(vec![call("c1")])),
            row(2, ChatMessage::user("wait")),
            row(3, ChatMessage::from(vec![ToolResponse::new("c1", "x")])),
        ]);
        refuses(&interleaved, "tool call c1 without a result");
        // ...calls/results in the wrong role, and an empty message.
        let mut misplaced = ChatMessage::from(vec![ToolResponse::new("c1", "x")]);
        misplaced.role = ChatRole::Assistant;
        refuses(
            &tail(vec![row(0, user.clone()), row(1, misplaced)]),
            "tool result c1 outside a tool message",
        );
        let mut misplaced = ChatMessage::from(vec![call("c1")]);
        misplaced.role = ChatRole::User;
        refuses(
            &tail(vec![row(0, misplaced)]),
            "tool call c1 outside an assistant message",
        );
        let empty = ChatMessage::assistant(MessageContent::from_parts(Vec::<ContentPart>::new()));
        refuses(&tail(vec![row(0, empty)]), "empty Assistant message");
        // Whole-group removal: dropping only the tool row of the fixture is
        // refused by validation on its own (independent of `plan` closure)...
        let f = frame();
        let only_tool = Plan {
            rows: [(2, whole())].into(),
            ..Plan::default()
        };
        refuses(&f.apply(&only_tool), "tool call c1 without a result");
        // ...while removing the dangling assistant row repairs the frame.
        let g = dangling.groups();
        let p = dangling.plan(&[msg(1)], &g).unwrap();
        let after = dangling.apply(&p);
        after.validate().unwrap();
        assert_eq!(after.effective().1.len(), 1);
    }

    #[test]
    fn plan_refuses_malformed_and_foreign_targets() {
        let f = frame();
        let g = f.groups();
        for (t, needle) in [
            (msg(9), "not in the active context"),
            (ContextTarget::Head { epoch: 1 }, "not the current head"),
            (bin(0, 0, None), "not a binary"),
            (bin(0, 9, None), "does not exist"),
            (bin(0, 1, Some(0)), "not a tool response"),
            (bin(2, 0, None), "name its image"),
            (bin(2, 0, Some(5)), "no image 5"),
        ] {
            let err = f.plan(&[t], &g).unwrap_err();
            assert!(err.contains(needle), "{t:?}: {err}");
        }
    }

    #[test]
    fn head_removal_keeps_the_boundary() {
        let head = ChatMessage::user(compact::wrap_head(
            1,
            compact::CompactStyle::Summary,
            "facts",
            Vec::<String>::new(),
        ));
        let f = Active {
            epoch: 1,
            tail_from: 4,
            at_seq: 5,
            head: Some((head, Mask::default())),
            rows: vec![
                row(4, ChatMessage::user("u")),
                row(5, ChatMessage::assistant(MessageContent::from_text("a"))),
            ],
            meta: BTreeMap::new(),
        };
        let g = f.groups();
        let p = f.plan(&[ContextTarget::Head { epoch: 1 }], &g).unwrap();
        assert_eq!(p.heads, 1);
        let after = f.apply(&p);
        let (head, tail) = after.effective();
        assert!(head.is_none());
        assert_eq!((after.epoch, after.tail_from, tail.len()), (1, 4, 2));
        // Gone from the inventory; the tail is still there.
        let snap = after.snapshot("7", &after.groups());
        assert!(snap.items.iter().all(|i| i.kind != ContextKind::Summary));
        assert_eq!((snap.epoch, snap.tail_from, snap.items.len()), (1, 4, 2));
        assert!(
            after
                .plan(&[ContextTarget::Head { epoch: 1 }], &g)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn snapshot_lists_active_items_with_companions_and_kinds() {
        let f = frame();
        let snap = f.snapshot("3", &f.groups());
        let kinds: Vec<(ContextTarget, ContextKind)> =
            snap.items.iter().map(|i| (i.target, i.kind)).collect();
        assert_eq!(
            kinds,
            [
                (msg(0), ContextKind::User),
                (bin(0, 1, None), ContextKind::Image),
                (bin(0, 2, None), ContextKind::Image),
                (bin(0, 3, None), ContextKind::Document),
                (msg(1), ContextKind::Assistant),
                (msg(2), ContextKind::Tool),
                (bin(2, 0, Some(0)), ContextKind::Image),
                (bin(2, 0, Some(1)), ContextKind::Image),
                (bin(2, 1, Some(0)), ContextKind::Image),
                (msg(3), ContextKind::Assistant),
            ]
        );
        assert_eq!(snap.items[4].remove_with, [msg(2)]);
        assert_eq!(snap.items[5].remove_with, [msg(1)]);
        assert_eq!(snap.items[1].label, "a.png");
        assert_eq!(snap.items[0].label, "look");
        assert!(snap.items.iter().all(|i| i.kind != ContextKind::Message));
        assert_eq!(snap.revision, "3");
        assert!(
            snap.items
                .iter()
                .all(|i| i.label.chars().count() <= LABEL_CHARS + 1)
        );
        // Removed occurrences leave the inventory; the estimate follows.
        let p = f.plan(&[bin(0, 1, None)], &f.groups()).unwrap();
        let after = f.apply(&p);
        let snap2 = after.snapshot("4", &after.groups());
        assert_eq!(snap2.items.len(), snap.items.len() - 1);
        assert_eq!(
            snap.estimated_tokens - snap2.estimated_tokens,
            compact::IMAGE_TOKENS
        );
    }

    #[test]
    fn entries_report_every_occurrence_with_removed_flags() {
        let f = frame();
        let mask = bins(vec![at(0, Some(1))]);
        let e = entry(
            ContextOwner::Message { seq: 2 },
            &f.rows[2].1,
            &mask,
            true,
            vec![msg(1)],
        );
        assert_eq!(e.target, msg(2));
        assert!(e.active && !e.removed);
        let flags: Vec<(ContextTarget, bool)> = e
            .attachments
            .iter()
            .map(|a| (a.target, a.removed))
            .collect();
        assert_eq!(
            flags,
            [
                (bin(2, 0, Some(0)), false),
                (bin(2, 0, Some(1)), true),
                (bin(2, 1, Some(0)), false)
            ]
        );
        assert_eq!(e.attachments[1].name, "same.png");
        assert_eq!(e.attachments[1].mime, "image/png");
        assert!(e.attachments.iter().all(|a| a.path.is_none()));
        // Whole removal marks every occurrence removed.
        let e = entry(
            ContextOwner::Message { seq: 0 },
            &f.rows[0].1,
            &whole(),
            false,
            vec![],
        );
        assert!(e.removed && !e.active && e.attachments.iter().all(|a| a.removed));
    }

    #[test]
    fn preview_bounds_labels() {
        assert_eq!(preview("\n\n  hello \nworld"), "hello");
        let long = "x".repeat(200);
        let p = preview(&long);
        assert_eq!(p.chars().count(), LABEL_CHARS + 1);
        assert!(p.ends_with('…'));
        assert_eq!(preview(""), "");
    }

    // region:    --- 197 SHAKE-BY-CATEGORY

    fn tcall(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            fn_name: name.into(),
            fn_arguments: args,
            thought_signatures: None,
        }
    }

    fn image(mime: &str, bytes: &[u8], name: Option<&str>) -> Binary {
        Binary::from_base64(mime, BASE64.encode(bytes), name.map(Into::into))
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v.extend([8, 6, 0, 0, 0]);
        v
    }

    /// SOI, a JFIF APP0 segment, then the baseline frame header.
    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        v.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        v.extend([0xFF, 0xC0, 0x00, 0x11, 0x08]);
        v.extend(h.to_be_bytes());
        v.extend(w.to_be_bytes());
        v.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        v
    }

    fn gif(w: u16, h: u16) -> Vec<u8> {
        let mut v = b"GIF89a".to_vec();
        v.extend(w.to_le_bytes());
        v.extend(h.to_le_bytes());
        v.extend([0, 0, 0]);
        v
    }

    fn webp(chunk: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WEBP".to_vec();
        v.extend(chunk);
        v.extend((payload.len() as u32).to_le_bytes());
        v.extend(payload);
        v
    }

    const JSON_RENDER: &str = r#"{"root":"s","elements":{"m":{"type":"MetricGrid","props":{}},"c":{"type":"Checklist","props":{}},"m2":{"type":"MetricGrid","props":{}},"t":{"type":"Table","props":{}},"k":{"type":"KeyValueList","props":{}},"s":{"type":"Stack","props":{},"children":["m","c"]}}}"#;

    /// Two surface fences with prose between them, and a ```` ```rust ````
    /// fence holding a mermaid-looking line that is content.
    fn reply() -> String {
        format!(
            "The branch is in.\n```json-render\n{JSON_RENDER}\n```\nTwo things:\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n````rust\n```mermaid\nnot a fence\n```\n````\nDone."
        )
    }

    fn user_text(seq: i64, text: &str) -> (i64, ChatMessage, Mask) {
        row(seq, ChatMessage::user(text))
    }

    fn fence(seq: i64, part: u32, index: u32) -> ContextTarget {
        ContextTarget::Fence {
            owner: ContextOwner::Message { seq },
            part,
            index,
        }
    }

    fn source(seq: i64, part: u32) -> ContextTarget {
        ContextTarget::Source {
            owner: ContextOwner::Message { seq },
            part,
        }
    }

    /// One row per §1 kind/group, behind a snap head with one frame.
    fn categories() -> Active {
        let head = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("<context_summary epoch=\"1\">\nearlier work".into()),
            ContentPart::Binary(image("image/png", &png(800, 600), Some("frame-0.png"))),
            ContentPart::Text(compact::HEAD_CLOSE.into()),
        ]));
        let user = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(
                "<time now=\"2026-11-16 22:31 +00:00 (Mon)\"/>\nshake images please".into(),
            ),
            ContentPart::Binary(image("image/png", &png(1568, 1103), Some("shot.png"))),
            ContentPart::Binary(image("image/jpeg", &jpeg(640, 480), Some("/tmp/photo.jpg"))),
        ]));
        let cards = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::Text(reply()),
            ContentPart::ToolCall(tcall(
                "s1",
                "show",
                serde_json::json!({"name": "mock.html", "content": "<html><body>hi</body></html>"}),
            )),
            ContentPart::ToolCall(tcall(
                "a1",
                "app",
                serde_json::json!({"name": "counter", "source": "root = Stack([])", "title": "Counter"}),
            )),
            ContentPart::ToolCall(tcall(
                "p1",
                "page_preview",
                serde_json::json!({"html": "<p>draft</p>", "title": "Draft", "width": 390}),
            )),
            ContentPart::ToolCall(tcall(
                "s2",
                "show",
                serde_json::json!({"content": "# notes"}),
            )),
            ContentPart::ToolCall(tcall(
                "s3",
                "show",
                serde_json::json!({"path": "/x/y.md", "name": "y.md"}),
            )),
        ]));
        let results = ChatMessage::from(
            ["s1", "a1", "p1", "s2", "s3"]
                .map(|id| ToolResponse::new(id, "shown"))
                .to_vec(),
        );
        let shots = ChatMessage::from(vec![
            tcall("c1", "computer_screenshot", serde_json::json!({})),
            tcall(
                "c2",
                "read_file",
                serde_json::json!({"path": "/p/diagram.webp"}),
            ),
            tcall("c3", "read_file", serde_json::json!({"path": "/p/a.md"})),
        ]);
        let shot_results = ChatMessage::from(vec![
            ToolResponse::new("c1", "").with_images(vec![image("image/gif", &gif(32, 16), None)]),
            ToolResponse::new("c2", "").with_images(vec![image(
                "image/webp",
                &webp(b"VP8X", &[0, 0, 0, 0, 0x7f, 0x07, 0, 0x37, 0x04, 0]),
                Some("/p/diagram.webp"),
            )]),
            ToolResponse::new("c3", "# a"),
        ]);
        let pdf_only = ChatMessage::user(MessageContent::from_parts(vec![ContentPart::Binary(
            pdf("spec.pdf"),
        )]));
        let openui = ChatMessage::assistant(MessageContent::from_text(
            "Try it:\n```openui\nroot = Stack([a])\na = TextContent(\"x\")\n```\n",
        ));
        let mut meta = BTreeMap::new();
        meta.insert(
            0,
            RowMeta {
                at: Some("2026-11-16T22:31:00Z".into()),
                ..RowMeta::default()
            },
        );
        meta.insert(
            9,
            RowMeta {
                peer: Some("session:abc".into()),
                sender: Some("iso-engine".into()),
                ..RowMeta::default()
            },
        );
        meta.insert(
            10,
            RowMeta {
                note: true,
                ..RowMeta::default()
            },
        );
        Active {
            epoch: 1,
            tail_from: 0,
            at_seq: -1,
            head: Some((head, Mask::default())),
            rows: vec![
                row(0, user),
                row(1, cards),
                row(2, results),
                row(3, shots),
                row(4, shot_results),
                user_text(
                    5,
                    "<knowledge>\nConcepts that may be relevant:\n- `facts/a` — A\n</knowledge>",
                ),
                user_text(6, "<todo>\nTodo (0/2 done):\n  ▸ a\n</todo>"),
                user_text(7, "<time now=\"2026-11-16 22:40 +00:00 (Mon)\"/>"),
                user_text(
                    8,
                    "<job_result id=\"j1\" command=\"ls\" exit=\"0\" secs=\"1\">\nout\n</job_result>\n<child_stalled child=\"w\" session=\"s\" phase=\"p\" quiet_secs=\"9\" at=\"t\">\nquiet\n</child_stalled>",
                ),
                user_text(
                    9,
                    "<message from=\"session:abc\" id=\"i\">\nreport ready\n</message>",
                ),
                user_text(10, "the fallback model saw a cat"),
                row(11, pdf_only),
                user_text(12, "<rounds source=\"user\">mine</rounds>"),
                user_text(13, "<skill name=\"x\">body</skill> go"),
                row(14, openui),
            ],
            meta,
        }
    }

    #[test]
    fn classifier_gives_every_item_one_kind_and_group() {
        use ContextKind as K;
        let f = categories();
        f.validate().unwrap();
        let snap = f.snapshot("1", &f.groups());
        let head_bin = ContextTarget::Binary {
            owner: ContextOwner::Head { epoch: 1 },
            part: 1,
            image: None,
        };
        let got: Vec<(ContextTarget, ContextKind, &str)> = snap
            .items
            .iter()
            .map(|i| (i.target, i.kind, i.group.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (ContextTarget::Head { epoch: 1 }, K::Summary, "summary"),
                (head_bin, K::Image, "summary"),
                (msg(0), K::User, "user"),
                (bin(0, 1, None), K::Image, "attachment"),
                (bin(0, 2, None), K::Image, "attachment"),
                (msg(1), K::Assistant, "assistant"),
                (fence(1, 0, 0), K::Surface, "diagram"),
                (fence(1, 0, 1), K::Surface, "diagram"),
                (source(1, 1), K::Surface, "page"),
                (source(1, 2), K::Surface, "app"),
                (source(1, 3), K::Surface, "page"),
                (source(1, 4), K::Surface, "file"),
                (msg(2), K::Tool, "show"),
                (msg(3), K::Tool, "computer_screenshot"),
                (msg(4), K::Tool, "computer_screenshot"),
                (bin(4, 0, Some(0)), K::Image, "screenshot"),
                (bin(4, 1, Some(0)), K::Image, "tool"),
                (msg(5), K::Note, "knowledge"),
                (msg(6), K::Note, "todo"),
                (msg(7), K::Note, "time"),
                (msg(8), K::Note, "notice"),
                (msg(9), K::Peer, "peer"),
                (msg(10), K::Note, "notice"),
                (msg(11), K::User, "user"),
                (bin(11, 0, None), K::Document, "document"),
                (msg(12), K::User, "user"),
                (msg(13), K::User, "user"),
                (msg(14), K::Assistant, "assistant"),
                (fence(14, 0, 0), K::Surface, "diagram"),
            ]
        );
        // The legacy pre-197 kind is never emitted.
        assert!(snap.items.iter().all(|i| i.kind != K::Message));
        let item = |t: ContextTarget| snap.items.iter().find(|i| i.target == t).unwrap();
        let shown = |t| {
            let i = item(t);
            (i.label.as_str(), i.detail.as_deref())
        };
        assert_eq!(
            shown(head_bin),
            ("Summary image: frame-0.png", Some("PNG · 800×600"))
        );
        assert_eq!(shown(msg(0)), ("shake images please", None));
        assert_eq!(
            shown(bin(0, 1, None)),
            ("shot.png", Some("PNG · 1568×1103"))
        );
        assert_eq!(
            shown(bin(0, 2, None)),
            ("photo.jpg", Some("JPEG · 640×480"))
        );
        assert_eq!(
            shown(fence(1, 0, 0)),
            ("MetricGrid, Checklist, Table", Some("in reply"))
        );
        assert_eq!(shown(fence(1, 0, 1)), ("sequenceDiagram", Some("in reply")));
        assert_eq!(shown(fence(14, 0, 0)), ("openui: Stack", Some("in reply")));
        assert_eq!(shown(source(1, 1)), ("mock.html", Some("show")));
        assert_eq!(shown(source(1, 2)), ("counter", Some("app")));
        assert_eq!(shown(source(1, 3)), ("Draft", Some("page_preview")));
        assert_eq!(shown(source(1, 4)), ("show", Some("show")));
        assert_eq!(
            shown(msg(2)),
            (
                "tool result: show, app, page_preview",
                Some("app, page_preview")
            )
        );
        assert_eq!(
            shown(msg(3)),
            (
                "tool call: computer_screenshot, read_file",
                Some("read_file")
            )
        );
        assert_eq!(
            shown(bin(4, 0, Some(0))),
            ("computer_screenshot", Some("GIF · 32×16"))
        );
        assert_eq!(
            shown(bin(4, 1, Some(0))),
            ("diagram.webp", Some("WEBP · 1920×1080"))
        );
        assert_eq!(shown(msg(5)), ("Concepts that may be relevant:", None));
        assert_eq!(
            shown(msg(9)),
            ("iso-engine: report ready", Some("session:abc"))
        );
        assert_eq!(shown(bin(11, 0, None)), ("spec.pdf", Some("PDF")));
        // The row's time rides on the row and its pieces.
        assert_eq!(item(msg(0)).at.as_deref(), Some("2026-11-16T22:31:00Z"));
        assert_eq!(item(bin(0, 1, None)).at, item(msg(0)).at);
        assert_eq!(item(msg(1)).at, None);
        assert!(snap.items.iter().all(|i| {
            i.label.chars().count() <= LABEL_CHARS + 1
                && i.detail
                    .as_ref()
                    .is_none_or(|d| d.chars().count() <= DETAIL_CHARS + 1)
        }));
    }

    #[test]
    fn image_details_read_header_dimensions_or_fall_back_to_the_format() {
        let detail = |mime: &str, bytes: &[u8]| binary_detail(&image(mime, bytes, None));
        assert_eq!(detail("image/png", &png(1568, 1103)), "PNG · 1568×1103");
        assert_eq!(detail("image/jpeg", &jpeg(640, 480)), "JPEG · 640×480");
        assert_eq!(detail("image/gif", &gif(32, 16)), "GIF · 32×16");
        // VP8X canvas (w-1, h-1 as 24 bits), VP8L (14-bit fields), lossy VP8 frame.
        let vp8x = webp(b"VP8X", &[0, 0, 0, 0, 0x7f, 0x07, 0, 0x37, 0x04, 0]);
        let bits: u32 = 99 | (49 << 14);
        let mut vp8l = vec![0x2f];
        vp8l.extend(bits.to_le_bytes());
        let vp8l = webp(b"VP8L", &vp8l);
        let mut vp8 = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
        vp8.extend(300u16.to_le_bytes());
        vp8.extend(200u16.to_le_bytes());
        let vp8 = webp(b"VP8 ", &vp8);
        assert_eq!(detail("image/webp", &vp8x), "WEBP · 1920×1080");
        assert_eq!(detail("image/webp", &vp8l), "WEBP · 100×50");
        assert_eq!(detail("image/webp", &vp8), "WEBP · 300×200");
        // Format only: an unread format, a header that does not parse, a document.
        assert_eq!(detail("image/heic", b"\0\0\0\x18ftypheic"), "HEIC");
        assert_eq!(detail("image/png", b"not a png at all"), "PNG");
        assert_eq!(detail("image/jpeg", &[0xFF, 0xD8, 0xFF]), "JPEG");
        assert_eq!(detail("application/pdf; x=1", b"%PDF-1.7"), "PDF");
    }

    #[test]
    fn injected_rows_follow_the_transcript_rule() {
        for (text, want) in [
            ("<knowledge>\nx\n</knowledge>", Some("knowledge")),
            ("<time now=\"x\"/>\n<todo>\nTodo\n</todo>", Some("todo")),
            ("<time now=\"x\"/>", Some("time")),
            (
                "<reminder id=\"r3\" after=\"60\">\nping\n</reminder>",
                Some("notice"),
            ),
            ("<surface_error>\nfix it\n</surface_error>", Some("notice")),
            (
                "<cell_result run=\"1\" exit=\"0\" secs=\"1\">\nok\n</cell_result>",
                Some("notice"),
            ),
            (
                "<schedule name=\"daily\" at=\"t\">\ngo\n</schedule>",
                Some("notice"),
            ),
            ("<vision>\nobs\n</vision>", Some("notice")),
            ("<session_moved to=\"a\" from=\"b\"/>", Some("notice")),
            // The user's: leading words, attributes on a bare-only note, an
            // unclosed or unknown tag, text after the wrappers, a skill, no text.
            ("hello <todo>x</todo>", None),
            ("<todo source=\"me\">\nx\n</todo>", None),
            ("<knowledge>\nunclosed", None),
            ("<div>html</div>", None),
            ("<todo>\nx\n</todo>\nand my words", None),
            ("<skill name=\"x\">body</skill> go", None),
            ("", None),
            ("<time now=\"x\"/>\nreal words", None),
        ] {
            assert_eq!(injected(text), want, "{text:?}");
        }
    }

    #[test]
    fn surface_fence_scanner_rule() {
        let t = "```not``` a fence\n  ```mermaid\nindented: content\n``` mermaid\nspaced: other fence\n```\na\n~~~~mermaid\ngraph TD\n~~~\nstill inside\n~~~~~\nb\n```json-render \r\n{}\r\n```\r\nc\n```bash\n```openui\n```\n```openui\nroot = X()\n";
        let got = surface_fences(t);
        let langs: Vec<&str> = got.iter().map(|f| f.lang).collect();
        assert_eq!(langs, ["mermaid", "json-render", "openui"]);
        assert_eq!(
            &t[got[0].range.clone()],
            "~~~~mermaid\ngraph TD\n~~~\nstill inside\n~~~~~"
        );
        assert_eq!(got[0].body, "graph TD\n~~~\nstill inside\n");
        assert_eq!(&t[got[1].range.clone()], "```json-render \r\n{}\r\n```");
        assert_eq!(got[1].body, "{}\r\n");
        // Unclosed: to the end of the part, its final line break kept outside.
        assert_eq!(&t[got[2].range.clone()], "```openui\nroot = X()");
        assert_eq!(got[2].body, "root = X()\n");
        // Two in one part, and none in plain text or inside other fences.
        assert_eq!(surface_fences(&reply()).len(), 2);
        assert!(surface_fences("```rust\n```mermaid\n```\n").is_empty());
        assert!(surface_fences("no fences").is_empty());
    }

    #[test]
    fn fences_and_sources_project_to_markers_and_keep_the_exchange() {
        let f = categories();
        let g = f.groups();
        let p = f
            .plan(
                &[
                    fence(1, 0, 0),
                    fence(1, 0, 1),
                    source(1, 1),
                    source(1, 2),
                    source(1, 3),
                    source(1, 4),
                    fence(14, 0, 0),
                ],
                &g,
            )
            .unwrap();
        assert_eq!((p.messages, p.images, p.surfaces), (0, 0, 7));
        assert_eq!(
            p.rows[&1].fences,
            [FenceAt { part: 0, index: 0 }, FenceAt { part: 0, index: 1 }]
        );
        assert_eq!(p.rows[&1].sources, [1, 2, 3, 4]);
        let after = f.apply(&p);
        after.validate().unwrap();
        let (_, tail) = after.effective();
        let row = |seq: i64| &tail.iter().find(|(s, _)| *s == seq).unwrap().1;
        assert_eq!(
            row(1).content.texts(),
            [
                "The branch is in.\n[diagram removed from context: MetricGrid, Checklist, Table]\nTwo things:\n[mermaid diagram removed from context]\n````rust\n```mermaid\nnot a fence\n```\n````\nDone."
            ]
        );
        assert_eq!(
            row(14).content.texts(),
            ["Try it:\n[openui surface removed from context]\n"]
        );
        let calls = row(1).content.tool_calls();
        let field = |i: usize, k: &str| calls[i].fn_arguments[k].clone();
        assert_eq!(
            field(0, "content"),
            "[page removed from context: mock.html]"
        );
        assert_eq!(field(0, "name"), "mock.html");
        assert_eq!(field(1, "source"), "[app removed from context: counter]");
        assert_eq!(field(1, "title"), "Counter");
        assert_eq!(field(2, "html"), "[page removed from context: Draft]");
        assert_eq!(field(2, "width"), 390);
        assert_eq!(field(3, "content"), "[file removed from context: show]");
        // The path-shown card, every call id and the paired results are untouched.
        assert_eq!(
            calls[4].fn_arguments,
            serde_json::json!({"path": "/x/y.md", "name": "y.md"})
        );
        let ids: Vec<&str> = calls.iter().map(|c| c.call_id.as_str()).collect();
        assert_eq!(ids, ["s1", "a1", "p1", "s2", "s3"]);
        assert_eq!(
            serde_json::to_string(row(2)).unwrap(),
            serde_json::to_string(&f.rows[2].1).unwrap()
        );
        // The raw rows keep their originals.
        assert_eq!(after.rows[1].1.content.texts(), [reply().as_str()]);
        // A json-render body that does not parse gets the bare marker.
        assert_eq!(
            cut_fences("a\n```json-render\n{nope\n```\nb", |_| true),
            "a\n[diagram removed from context]\nb"
        );
        assert_eq!(
            fence_label(&surface_fences("```json-render\n{nope\n```")[0]),
            "diagram"
        );
    }

    #[test]
    fn plan_subsumes_refuses_and_noops_for_surfaces() {
        let f = categories();
        let g = f.groups();
        // A whole row takes its fences and sources: nothing counted twice.
        let p = f
            .plan(&[msg(1), msg(2), fence(1, 0, 0), source(1, 1)], &g)
            .unwrap();
        assert_eq!((p.messages, p.surfaces), (2, 0));
        assert!(p.rows[&1].whole && p.rows[&1].fences.is_empty());
        let head_fence = ContextTarget::Fence {
            owner: ContextOwner::Head { epoch: 1 },
            part: 0,
            index: 0,
        };
        let head_source = ContextTarget::Source {
            owner: ContextOwner::Head { epoch: 1 },
            part: 0,
        };
        for (t, needle) in [
            (head_fence, "summary head has no surface"),
            (head_source, "summary head has no surface"),
            (fence(99, 0, 0), "not in the active context"),
            (fence(1, 0, 2), "part 0 has no surface fence 2"),
            (fence(1, 9, 0), "part 9 does not exist"),
            (fence(1, 1, 0), "part 1 is not a text part"),
            (fence(0, 0, 0), "part 0 is not assistant text"),
            (source(1, 0), "part 0 is not a tool call"),
            (source(2, 0), "part 0 is not a tool call"),
            (
                source(1, 5),
                "part 5 has no show, app or page_preview source",
            ),
            (source(1, 6), "part 6 does not exist"),
        ] {
            let err = f.plan(&[t], &g).unwrap_err();
            assert!(err.contains(needle), "{t:?}: {err}");
        }
        // Already removed: a no-op; a later edit merges and keeps them sorted.
        let p = f.plan(&[source(1, 3), fence(1, 0, 1)], &g).unwrap();
        let after = f.apply(&p);
        let g2 = after.groups();
        assert!(
            after
                .plan(&[source(1, 3), fence(1, 0, 1)], &g2)
                .unwrap()
                .is_empty()
        );
        let p2 = after
            .plan(&[source(1, 1), fence(1, 0, 0), bin(0, 1, None)], &g2)
            .unwrap();
        assert_eq!((p2.surfaces, p2.images), (2, 1));
        assert_eq!(p2.rows[&1].sources, [1, 3]);
        assert_eq!(
            p2.rows[&1].fences,
            [FenceAt { part: 0, index: 0 }, FenceAt { part: 0, index: 1 }]
        );
        // Removed pieces leave the inventory.
        let snap = after.snapshot("2", &g2);
        assert!(
            !snap
                .items
                .iter()
                .any(|i| i.target == source(1, 3) || i.target == fence(1, 0, 1))
        );
        assert!(snap.items.iter().any(|i| i.target == fence(1, 0, 0)));
    }

    #[test]
    fn masks_with_surfaces_round_trip_and_old_masks_decode() {
        let old = r#"{"whole":false,"binaries":[{"part":1}]}"#;
        let m = Mask::decode(Some(old)).unwrap();
        assert_eq!(m, bins(vec![at(1, None)]));
        // An old-shape mask still encodes in the old shape.
        assert_eq!(m.encode().unwrap().as_deref(), Some(old));
        let m = Mask {
            fences: vec![FenceAt { part: 0, index: 1 }],
            sources: vec![2, 4],
            ..Mask::default()
        };
        let s = m.encode().unwrap().unwrap();
        assert!(
            s.contains(r#""fences":[{"part":0,"index":1}]"#) && s.contains(r#""sources":[2,4]"#),
            "{s}"
        );
        assert_eq!(Mask::decode(Some(&s)).unwrap(), m);
        assert!(!m.is_empty());
        let only_sources = Mask {
            sources: vec![1],
            ..Mask::default()
        };
        assert!(!only_sources.is_empty() && only_sources.partial());
        assert!(Mask::default().is_empty() && !whole().partial() && !whole().is_empty());
    }

    #[test]
    fn tokens_price_what_each_target_takes_out() {
        let f = categories();
        let g = f.groups();
        let snap = f.snapshot("1", &g);
        let item = |t: ContextTarget| snap.items.iter().find(|i| i.target == t).unwrap();
        // Pieces: a fence its text, a source its value, a tool image the flat charge.
        let text = reply();
        let fence0 = &surface_fences(&text)[0];
        assert_eq!(item(fence(1, 0, 0)).tokens, (fence0.range.len() / 4) as u32);
        assert_eq!(
            item(source(1, 1)).tokens,
            ("<html><body>hi</body></html>".len() / 4) as u32
        );
        assert_eq!(item(bin(4, 0, Some(0))).tokens, compact::IMAGE_TOKENS);
        assert_eq!(
            item(bin(0, 1, None)).tokens,
            compact::est_part(&f.rows[0].1.content.parts()[1])
        );
        // A whole row includes its pieces; the head is its projected head.
        let row1 = item(msg(1)).tokens;
        assert_eq!(row1, compact::est(&f.rows[1].1));
        let pieces: u32 = snap
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i.target,
                    ContextTarget::Fence {
                        owner: ContextOwner::Message { seq: 1 },
                        ..
                    } | ContextTarget::Source {
                        owner: ContextOwner::Message { seq: 1 },
                        ..
                    }
                )
            })
            .map(|i| i.tokens)
            .sum();
        assert!(row1 > pieces, "{row1} vs {pieces}");
        // The UI's sum rule: {row 1, its fence, its source, row 2} counts the
        // two rows only, and removing them takes exactly that out.
        let picked = [msg(1), fence(1, 0, 0), source(1, 1), msg(2)];
        let sum: u32 = picked
            .iter()
            .map(|t| item(*t))
            .filter(|i| {
                !matches!(i.target, ContextTarget::Fence { owner: ContextOwner::Message { seq }, .. } | ContextTarget::Source { owner: ContextOwner::Message { seq }, .. } if picked.contains(&msg(seq)))
            })
            .map(|i| i.tokens)
            .sum();
        assert_eq!(sum, row1 + item(msg(2)).tokens);
        let after = f.apply(&f.plan(&picked, &g).unwrap());
        assert_eq!(snap.estimated_tokens - after.estimate(), sum);
        assert_eq!(snap.estimated_tokens, f.estimate());
        // A partly shaken row counts what is left of it.
        let cut = f.apply(&f.plan(&[fence(1, 0, 0)], &g).unwrap());
        let left = cut.snapshot("2", &cut.groups());
        let row1_after = left
            .items
            .iter()
            .find(|i| i.target == msg(1))
            .unwrap()
            .tokens;
        assert_eq!(
            row1_after,
            compact::est(&project(&cut.rows[1].1, &cut.rows[1].2).unwrap())
        );
        assert!(row1_after < row1);
    }

    #[test]
    fn history_entries_carry_fences_and_sources_with_removed_flags() {
        let f = categories();
        let mask = Mask {
            fences: vec![FenceAt { part: 0, index: 0 }],
            sources: vec![1],
            ..Mask::default()
        };
        let e = entry(
            ContextOwner::Message { seq: 1 },
            &f.rows[1].1,
            &mask,
            true,
            vec![msg(2)],
        );
        let pieces = |ps: &[ContextPiece]| -> Vec<(ContextTarget, String, bool)> {
            ps.iter()
                .map(|p| (p.target, p.label.clone(), p.removed))
                .collect()
        };
        assert_eq!(
            pieces(&e.fences),
            [
                (
                    fence(1, 0, 0),
                    "MetricGrid, Checklist, Table".to_owned(),
                    true
                ),
                (fence(1, 0, 1), "sequenceDiagram".to_owned(), false),
            ]
        );
        assert_eq!(
            pieces(&e.sources),
            [
                (source(1, 1), "mock.html".to_owned(), true),
                (source(1, 2), "counter".to_owned(), false),
                (source(1, 3), "Draft".to_owned(), false),
                (source(1, 4), "show".to_owned(), false),
            ]
        );
        // A whole removal marks every piece removed; rows without pieces omit the arrays.
        let e = entry(
            ContextOwner::Message { seq: 1 },
            &f.rows[1].1,
            &whole(),
            false,
            vec![],
        );
        assert!(e.fences.iter().chain(&e.sources).all(|p| p.removed));
        let user = entry(
            ContextOwner::Message { seq: 0 },
            &f.rows[0].1,
            &Mask::default(),
            true,
            vec![],
        );
        let json = serde_json::to_value(&user).unwrap();
        assert!(
            json.get("fences").is_none() && json.get("sources").is_none(),
            "{json}"
        );
    }

    // endregion: --- 197 SHAKE-BY-CATEGORY

    #[test]
    fn save_blob_names_by_content_hash() {
        let scratch = crate::test_dir("blob");
        let dir = scratch.path().to_path_buf();
        let p = save_blob(b"hello", "png", &dir).unwrap();
        let hex = hex::encode(Sha256::digest(b"hello"));
        assert_eq!(
            p,
            dir.join("attachments").join(format!("{}.png", &hex[..16]))
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        assert_eq!(save_blob(b"hello", "png", &dir).unwrap(), p);

        // Extensionless names store without a trailing dot.
        let bare = save_blob(b"all:\n\techo ok\n", "", &dir).unwrap();
        let bare_hex = hex::encode(Sha256::digest(b"all:\n\techo ok\n"));
        assert_eq!(bare, dir.join("attachments").join(&bare_hex[..16]));
        assert!(!bare.to_string_lossy().ends_with('.'));
    }

    #[test]
    fn text_and_pointer_attachments_reload_as_removable_chips() {
        let msg = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("please inspect these".into()),
            ContentPart::Text(
                "the user attached: /work/notes.md\n\n```md\n# Hello\n```".into(),
            ),
            ContentPart::Text(
                "the user attached: /work/diagram.svg\n\n```svg\n<svg/>\n```".into(),
            ),
            ContentPart::Text(
                "the user attached: /work/page.html\n\n```html\n<h1>Hi</h1>\n```".into(),
            ),
            ContentPart::Text(
                "the user attached: /work/Makefile\n\n```\nall:\n\techo ok\n```".into(),
            ),
            ContentPart::Text("the user attached: /work/bundle.zip".into()),
        ]));
        let owner = ContextOwner::Message { seq: 7 };
        let atts = attachments(owner, &msg, &Mask::default());
        assert_eq!(atts.len(), 5);
        assert_eq!(atts[0].name, "notes.md");
        assert_eq!(atts[0].mime, "text/markdown");
        assert_eq!(atts[0].path, Some(PathBuf::from("/work/notes.md")));
        assert_eq!(
            atts[0].target,
            ContextTarget::Binary {
                owner,
                part: 1,
                image: None
            }
        );
        assert!(!atts[0].removed);

        assert_eq!(atts[1].name, "diagram.svg");
        assert_eq!(atts[1].mime, "image/svg+xml");
        assert_eq!(atts[1].path, Some(PathBuf::from("/work/diagram.svg")));

        assert_eq!(atts[2].name, "page.html");
        assert_eq!(atts[2].mime, "text/html");
        assert_eq!(atts[2].path, Some(PathBuf::from("/work/page.html")));

        assert_eq!(atts[3].name, "Makefile");
        assert_eq!(atts[3].mime, "text/plain");
        assert_eq!(atts[3].path, Some(PathBuf::from("/work/Makefile")));

        assert_eq!(atts[4].name, "bundle.zip");
        assert_eq!(atts[4].mime, "application/zip");
        assert_eq!(atts[4].path, Some(PathBuf::from("/work/bundle.zip")));

        // Removing the zip pointer (part 5) and markdown body (part 1) updates both
        // the chip's `removed` flag and the projected message sent to the model.
        let active = Active {
            epoch: 0,
            at_seq: -1,
            tail_from: 0,
            head: None,
            rows: vec![(7, msg.clone(), Mask::default())],
            meta: BTreeMap::new(),
        };
        let groups = active.groups();
        let plan = active
            .plan(&[atts[0].target, atts[4].target], &groups)
            .expect("removal plan");
        assert_eq!(plan.documents, 2);
        assert_eq!(plan.images, 0);
        let mask = &plan.rows[&7];
        let reloaded = attachments(owner, &msg, mask);
        assert!(reloaded[0].removed);
        assert!(!reloaded[1].removed);
        assert!(reloaded[4].removed);

        let projected = project(&msg, mask).expect("still has words and remaining parts");
        assert_eq!(projected.content.parts().len(), 4);
        assert!(projected.content.texts().iter().all(|t| !t.contains("notes.md") && !t.contains("bundle.zip")));
        assert!(projected.content.texts().iter().any(|t| t.contains("diagram.svg")));
    }
}
