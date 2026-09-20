//! Design doc 48: explicit removal from context. Wire types, the exclusion
//! masks persisted beside `messages`/`epochs` rows, and the pure projection,
//! protocol-closure and inventory helpers the store applies inside its
//! transactions. Nothing here touches sqlite; raw rows stay untouched and the
//! *effective* frame is a projection of row + mask.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use genai::chat::{Binary, ChatMessage, ChatRole, ContentPart, MessageContent};
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
}

/// What an inventory item is, for the `/shake <category>` filters.
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
    /// A user/assistant/system message with text.
    Message,
    /// An image occurrence (user attachment, tool screenshot, snap frame).
    Image,
    /// A non-image binary occurrence (native PDF).
    Document,
    /// A tool exchange row (tool results, or an assistant row that is only calls).
    Tool,
    /// The current epoch head.
    Summary,
}

/// One row of the active-context inventory (48 §3.2). No bodies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItem {
    /// What to send back to remove it.
    pub target: ContextTarget,
    /// Category.
    pub kind: ContextKind,
    /// Short safe display label (bounded preview or attachment name).
    pub label: String,
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
    /// Active items: head, its images, then rows and their binaries in seq order.
    pub items: Vec<ContextItem>,
    /// `compact::est` over the effective head + tail (not provider usage).
    pub estimated_tokens: u32,
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
        !self.whole && self.binaries.is_empty()
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
/// [`IMAGE_REMOVED`]. Other parts are byte-identical clones.
pub fn project(m: &ChatMessage, mask: &Mask) -> Option<ChatMessage> {
    if mask.whole {
        return None;
    }
    if mask.binaries.is_empty() {
        return Some(m.clone());
    }
    let mut parts = Vec::with_capacity(m.content.len());
    for (i, p) in m.content.iter().enumerate() {
        let part = u32::try_from(i).ok()?;
        match p {
            ContentPart::Binary(_) if mask.has(BinaryAt { part, image: None }) => {}
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
    /// Head row and its mask.
    pub head: Option<(ChatMessage, Mask)>,
    /// Raw tail rows with their masks, seq ascending.
    pub rows: Vec<(i64, ChatMessage, Mask)>,
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
            .and_then(|(h, mask)| project_head(h, mask));
        let tail = self
            .rows
            .iter()
            .filter_map(|(seq, m, mask)| project(m, mask).map(|m| (*seq, m)))
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

    /// Active-context inventory (48 §2.1): head, its binaries, then each live
    /// row and its binaries in seq order. `editable`/`reason` are the actor's.
    pub fn snapshot(&self, revision: &str, groups: &Groups) -> ContextSnapshot {
        let mut items = Vec::new();
        if let Some((h, mask)) = &self.head
            && !mask.whole
        {
            let owner = ContextOwner::Head { epoch: self.epoch };
            items.push(ContextItem {
                target: ContextTarget::Head { epoch: self.epoch },
                kind: ContextKind::Summary,
                label: format!("Summary (epoch {}): {}", self.epoch, preview(&head_body(h))),
                remove_with: Vec::new(),
            });
            items.extend(
                attachments(owner, h, mask)
                    .into_iter()
                    .filter(|a| !a.removed)
                    .map(|a| ContextItem {
                        target: a.target,
                        kind: ContextKind::Image,
                        label: format!("Summary image: {}", a.name),
                        remove_with: Vec::new(),
                    }),
            );
        }
        for (seq, m, mask) in &self.rows {
            if mask.whole {
                continue;
            }
            let owner = ContextOwner::Message { seq: *seq };
            let (kind, label) = describe(m);
            items.push(ContextItem {
                target: ContextTarget::Message { seq: *seq },
                kind,
                label,
                remove_with: groups.remove_with(*seq),
            });
            items.extend(
                attachments(owner, m, mask)
                    .into_iter()
                    .filter(|a| !a.removed)
                    .map(|a| ContextItem {
                        target: a.target,
                        kind: if is_image_mime(&a.mime) {
                            ContextKind::Image
                        } else {
                            ContextKind::Document
                        },
                        label: a.name,
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
            estimated_tokens: self.estimate(),
        }
    }

    /// Resolve `targets` against this frame (48 §3.2): deduplicate, refuse
    /// unknown/malformed coordinates, require every protocol companion of a
    /// whole-message target to be included, let whole removal subsume the
    /// row's binaries, and treat exact already-removed targets as no-ops.
    pub fn plan(&self, targets: &[ContextTarget], groups: &Groups) -> Result<Plan, String> {
        let targets: BTreeSet<ContextTarget> = targets.iter().copied().collect();
        let mut whole_rows: BTreeSet<i64> = BTreeSet::new();
        let mut whole_head = false;
        let mut bins: BTreeMap<ContextOwner, BTreeSet<BinaryAt>> = BTreeMap::new();
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
                    binary_at(m, part, image)?;
                    bins.entry(owner)
                        .or_default()
                        .insert(BinaryAt { part, image });
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
                        binaries: mask.binaries.clone(),
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
                    binaries: mask.binaries.clone(),
                });
                plan.heads += 1;
            }
        }
        for (owner, ats) in bins {
            let subsumed = match owner {
                ContextOwner::Message { seq } => whole_rows.contains(&seq),
                ContextOwner::Head { .. } => whole_head,
            };
            let (m, mask) = self.owner(owner)?;
            if subsumed || mask.whole {
                continue;
            }
            let mut new = mask.clone();
            for at in ats {
                if new.has(at) {
                    continue;
                }
                new.binaries.push(at);
                if binary_at(m, at.part, at.image)?.is_image() {
                    plan.images += 1;
                } else {
                    plan.documents += 1;
                }
            }
            if new.binaries.len() != mask.binaries.len() {
                new.binaries.sort();
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

/// The binary at `(part, image)` of `m`, or why the coordinates are malformed.
fn binary_at(m: &ChatMessage, part: u32, image: Option<u32>) -> Result<&Binary, String> {
    let p = m
        .content
        .parts()
        .get(part as usize)
        .ok_or_else(|| format!("part {part} does not exist"))?;
    match (p, image) {
        (ContentPart::Binary(b), None) => Ok(b),
        (ContentPart::ToolResponse(r), Some(k)) => r
            .images
            .get(k as usize)
            .ok_or_else(|| format!("part {part} has no image {k}")),
        (ContentPart::Binary(_), Some(_)) => {
            Err(format!("part {part} is a binary, not a tool response"))
        }
        (ContentPart::ToolResponse(_), None) => {
            Err(format!("part {part} is a tool response; name its image"))
        }
        _ => Err(format!("part {part} is not a binary")),
    }
}

/// Every binary occurrence of `m` with its removal state under `mask`.
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
    ContextEntry {
        target,
        active,
        removed: mask.whole,
        remove_with,
        attachments: attachments(owner, m, mask),
        vision: None,
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

/// Category and bounded label of a live row.
fn describe(m: &ChatMessage) -> (ContextKind, String) {
    let responses = m.content.tool_responses();
    if !responses.is_empty() {
        let names: Vec<&str> = responses
            .iter()
            .map(|r| r.fn_name.as_deref().unwrap_or(r.call_id.as_str()))
            .collect();
        return (
            ContextKind::Tool,
            preview(&format!("tool result: {}", names.join(", "))),
        );
    }
    let calls = m.content.tool_calls();
    let text = text_of(&m.content);
    if m.role == ChatRole::Assistant && text.trim().is_empty() && !calls.is_empty() {
        let names: Vec<&str> = calls.iter().map(|c| c.fn_name.as_str()).collect();
        return (
            ContextKind::Tool,
            preview(&format!("tool call: {}", names.join(", "))),
        );
    }
    let kind = if m.role == ChatRole::Tool {
        ContextKind::Tool
    } else {
        ContextKind::Message
    };
    let role = m.role.to_string().to_ascii_lowercase();
    (kind, preview(&format!("{role}: {text}")))
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
    let mut out: String = line.chars().take(LABEL_CHARS).collect();
    if line.chars().nth(LABEL_CHARS).is_some() {
        out.push('…');
    }
    out
}

// endregion: --- Active frame

/// Write `bytes` as `<data_dir>/attachments/<sha256[..16]>.<ext>`; an existing
/// file is not rewritten. The one attachment-store naming rule, shared by RPC
/// `put_attachment`, the ACP prompt path and history preview materialization.
pub fn save_blob(bytes: &[u8], ext: &str, data_dir: &Path) -> std::io::Result<PathBuf> {
    let hex = format!("{:x}", Sha256::digest(bytes));
    let dir = data_dir.join("attachments");
    let path = dir.join(format!("{}.{ext}", &hex[..16]));
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
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
            ContentPart::ThoughtSignature("sig".into()),
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
            head: None,
            rows: vec![
                row(0, user),
                row(1, asst),
                row(2, tool),
                row(3, ChatMessage::assistant(MessageContent::from_text("done"))),
            ],
        }
    }

    #[test]
    fn mask_round_trips_and_null_means_empty() {
        assert_eq!(Mask::decode(None).unwrap(), Mask::default());
        assert_eq!(Mask::default().encode().unwrap(), None);
        let m = Mask {
            whole: false,
            binaries: vec![at(1, None), at(2, Some(1))],
        };
        let s = m.encode().unwrap().unwrap();
        assert_eq!(Mask::decode(Some(&s)).unwrap(), m);
        assert!(Mask::decode(Some("{nope")).is_err());
    }

    #[test]
    fn project_drops_exact_occurrences_by_original_index() {
        let f = frame();
        // A and the PDF gone; B (same name as a tool image) stays.
        let mask = Mask {
            whole: false,
            binaries: vec![at(1, None), at(3, None)],
        };
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
        let mask = Mask {
            whole: false,
            binaries: vec![at(0, Some(0)), at(1, Some(0))],
        };
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
        assert!(
            project(
                &f.rows[1].1,
                &Mask {
                    whole: true,
                    binaries: vec![]
                }
            )
            .is_none()
        );
    }

    #[test]
    fn image_only_user_row_empties_out() {
        let m = ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("  ".into()),
            ContentPart::Binary(img("only.png")),
        ]));
        assert!(
            project(
                &m,
                &Mask {
                    whole: false,
                    binaries: vec![at(1, None)]
                }
            )
            .is_none()
        );
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
        let mask = Mask {
            whole: false,
            binaries: vec![at(2, None)],
        };
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
        assert!(
            project_head(
                &head,
                &Mask {
                    whole: true,
                    binaries: vec![]
                }
            )
            .is_none()
        );
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
            rows: [(
                2,
                Mask {
                    whole: true,
                    binaries: vec![],
                },
            )]
            .into(),
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
            head: Some((head, Mask::default())),
            rows: vec![
                row(4, ChatMessage::user("u")),
                row(5, ChatMessage::assistant(MessageContent::from_text("a"))),
            ],
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
                (msg(0), ContextKind::Message),
                (bin(0, 1, None), ContextKind::Image),
                (bin(0, 2, None), ContextKind::Image),
                (bin(0, 3, None), ContextKind::Document),
                (msg(1), ContextKind::Message),
                (msg(2), ContextKind::Tool),
                (bin(2, 0, Some(0)), ContextKind::Image),
                (bin(2, 0, Some(1)), ContextKind::Image),
                (bin(2, 1, Some(0)), ContextKind::Image),
                (msg(3), ContextKind::Message),
            ]
        );
        assert_eq!(snap.items[4].remove_with, [msg(2)]);
        assert_eq!(snap.items[5].remove_with, [msg(1)]);
        assert_eq!(snap.items[1].label, "a.png");
        assert!(
            snap.items[0].label.starts_with("user: look"),
            "{}",
            snap.items[0].label
        );
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
        let mask = Mask {
            whole: false,
            binaries: vec![at(0, Some(1))],
        };
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
            &Mask {
                whole: true,
                binaries: vec![],
            },
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

    #[test]
    fn save_blob_names_by_content_hash() {
        let dir = std::env::temp_dir().join(format!("theuth-blob-{}", uuid::Uuid::now_v7()));
        let p = save_blob(b"hello", "png", &dir).unwrap();
        let hex = format!("{:x}", Sha256::digest(b"hello"));
        assert_eq!(
            p,
            dir.join("attachments").join(format!("{}.png", &hex[..16]))
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        assert_eq!(save_blob(b"hello", "png", &dir).unwrap(), p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
