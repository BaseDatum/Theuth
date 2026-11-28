//! Collaborative Deck document schema & actor handlers (design doc 229).
//!
//! A `DocKind::Deck` document is one pure `yrs::Doc` composed of five top-level `Y.Map`s
//! (`meta`, `sections`, `order`, `aliases`, `script_refs`), with no XML fragment and no
//! BlockNote helper mirror.
//!
//! Design invariants:
//! - Every section has a stable ID and its own nested `Y.Map` under `sections`.
//! - Local edits mutate only the target section's fields and append a compact revision entry —
//!   never a whole-document rewrite.
//! - Each section tracks an independent `rev: u64` counter for optimistic CAS (`expected_rev`)
//!   and per-section attribution (`created_by`, `updated_by`, `history`).
//! - `order` maps `section_id -> f64` rank (midpoint fractional indexing, tie-broken by ID) so
//!   concurrent section inserts and moves commute cleanly in Yjs without array rewrites.
//! - `script_refs` stores immutable display pointers only (`active_rev`, `pending_rev`, `mode`,
//!   `schedule`). Executable Python source, approvals, runtime state, snapshots, and live frames
//!   live exclusively in `crates/decks`; visual CRDT edits can never self-authorize code.

#![allow(missing_docs)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Array, ArrayPrelim, Doc, Map, MapPrelim, MapRef, Out, ReadTxn, StateVector, Transact,
    TransactionMut, Update,
};

use crate::FRAGMENT;
use crate::actor::{DocumentActor, Homing, Open};
use crate::hub::{DocError, Refusal};
use crate::meta::{AgentStyle, Binding, DocId, DocKind, DocMeta, DocumentChanged, Principal};
use crate::relay::{Client, CreateReq};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn any_int(v: i64) -> Any {
    Any::Number(yrs::any::Number::Int(v))
}

fn any_float(v: f64) -> Any {
    Any::Number(yrs::any::Number::Float(v))
}

/// Refusal returned when block-doc tools (`doc_read`, `doc_insert`, `doc_replace`, ...) target a Deck.
pub const DOC_REFUSAL: &str =
    "this is a Deck; use the deck_* tools (deck_read, deck_section_add, deck_section_patch, …)";

/// Root `Y.Map` names of a `DocKind::Deck` document.
pub const MAPS: [&str; 5] = ["meta", "sections", "order", "aliases", "script_refs"];

/// Schema version written to `meta.v`.
pub const SCHEMA: u32 = 1;

/// Maximum per-section revision entries retained in `sections[id].history`.
pub const MAX_SECTION_HISTORY: usize = 50;

/// Maximum section content bytes (256 KiB).
pub const MAX_SECTION_CONTENT_BYTES: usize = 256 * 1024;

/// Maximum OpenUI section source bytes (64 KiB).
pub const MAX_OPENUI_CONTENT_BYTES: usize = 64 * 1024;

/// Maximum content bytes stored inside a single history entry (16 KiB excerpt for large sections).
pub const MAX_HISTORY_CONTENT_BYTES: usize = 16 * 1024;

/// Maximum total history bytes retained per section (128 KiB).
pub const MAX_SECTION_HISTORY_BYTES: usize = 128 * 1024;

/// Maximum total history bytes retained across all sections of a Deck (1 MiB, well under 8 MiB `max_doc_bytes`).
pub const MAX_DECK_HISTORY_BYTES: usize = 1024 * 1024;

/// Maximum total sections (active + archived) in a single Deck.
pub const MAX_DECK_SECTIONS: usize = 128;

/// Maximum Deck title bytes.
pub const MAX_DECK_TITLE_BYTES: usize = 256;

/// Maximum Deck description bytes.
pub const MAX_DECK_DESCRIPTION_BYTES: usize = 4 * 1024;

/// Maximum section title bytes.
pub const MAX_SECTION_TITLE_BYTES: usize = 256;

/// Maximum section `data_key` bytes.
pub const MAX_SECTION_DATA_KEY_BYTES: usize = 128;

/// Maximum section history summary bytes.
pub const MAX_SECTION_SUMMARY_BYTES: usize = 512;

/// Maximum valid section revision counter (fits cleanly in Yjs signed 64-bit integer without overflow).
pub const MAX_SECTION_REV: u64 = i64::MAX as u64;

/// Maximum rev accepted from a raw client `doc_update` (`ApplyUpdate`) to prevent rev-exhaustion attacks.
const MAX_RAW_CLIENT_REV: i64 = 1_000_000_000_000;

/// Spacing between ranks after initialization or deterministic in-transaction re-spacing.
const RANK_STEP: f64 = 1000.0;

/// Minimum rank gap before triggering deterministic in-transaction re-spacing.
const MIN_RANK_GAP: f64 = 1e-6;

/// Maximum rank magnitude before triggering deterministic in-transaction re-spacing.
const MAX_RANK_ABS: f64 = 1e15;

/// Generate a random non-zero 32-bit Yjs client ID (matches `wb::model::ENGINE_CLIENT`).
pub(crate) fn random_client_id() -> u64 {
    u64::from(uuid::Uuid::new_v4().as_u128() as u32) | 1
}

/// Random per-process client ID used for engine-side Yjs writes (matches `wb::model::ENGINE_CLIENT`),
/// preventing `(client_id, clock)` collisions across engines in a mesh or across restarts.
pub(crate) static ENGINE_CLIENT: LazyLock<u64> = LazyLock::new(random_client_id);

/// Kind of visual section inside a Deck.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// Native `json-render` specification (bound to live snapshot/frame data).
    JsonRender,
    /// Collaborative markdown / text note section.
    Note,
    /// Strictly sandboxed HTML section (zero network/forms/same-origin, Deck intent bridge only).
    Html,
    /// Restricted interactive OpenUI Lang section (capped at 64 KiB, Deck synthetic catalog only).
    Openui,
}

impl SectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JsonRender => "json_render",
            Self::Note => "note",
            Self::Html => "html",
            Self::Openui => "openui",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "json_render" => Some(Self::JsonRender),
            "note" => Some(Self::Note),
            "html" => Some(Self::Html),
            "openui" => Some(Self::Openui),
            _ => None,
        }
    }

    pub fn max_content_bytes(self) -> usize {
        match self {
            Self::Openui => MAX_OPENUI_CONTENT_BYTES,
            Self::JsonRender | Self::Note | Self::Html => MAX_SECTION_CONTENT_BYTES,
        }
    }

    pub fn default_col_span(self) -> u32 {
        match self {
            Self::JsonRender | Self::Html | Self::Openui => 6,
            Self::Note => 12,
        }
    }
}

/// One historical revision recorded on a section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SectionRevision {
    /// Monotonic revision number (1-based).
    pub rev: u64,
    /// Timestamp (Unix ms).
    pub at: i64,
    /// Author (`"owner"`, `"session:<uuid>"`, `"engine"`).
    pub by: String,
    /// Short summary of what changed.
    pub summary: String,
    /// Section title at this revision.
    pub title: String,
    /// Section content at this revision.
    pub content: String,
    /// Archived flag at this revision.
    pub archived: bool,
}

/// A collaborative Deck section read from `yrs::Doc`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckSection {
    /// Stable section ID (UUIDv7-derived short token).
    pub id: String,
    /// Short monotonic alias (`"s1"`, `"s2"`, ...).
    pub alias: String,
    /// Section kind.
    pub kind: SectionKind,
    /// Display title.
    pub title: String,
    /// Section body (`json-render` JSON, markdown text, or HTML template).
    pub content: String,
    /// Grid column span (1..=12).
    pub col_span: u32,
    /// Optional data binding key into the Deck's live snapshot/frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_key: Option<String>,
    /// Whether this section is archived (hidden from active layout, still readable in history).
    pub archived: bool,
    /// Monotonic per-section revision counter for optimistic CAS.
    pub rev: u64,
    /// Fractional rank in `order`.
    pub rank: f64,
    /// Creation timestamp (Unix ms).
    pub created_at: i64,
    /// Creator attribution.
    pub created_by: String,
    /// Last modification timestamp (Unix ms).
    pub updated_at: i64,
    /// Last modifier attribution.
    pub updated_by: String,
}

/// Focus / presence marker stored in `meta.focus`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeckFocus {
    /// Focused section ID.
    pub section: String,
    /// Who set focus (`"owner"`, `"session:<uuid>"`).
    pub by: String,
    /// Timestamp (Unix ms).
    pub at: i64,
}

/// Read-only script pointer mirrored into `script_refs` for collaborative UI display.
/// Never contains executable source or grants authorization.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeckScriptRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_rev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

/// Complete snapshot of a Deck's collaborative Y.Doc state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeckDocSnapshot {
    pub doc_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub layout: String,
    pub columns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<DeckFocus>,
    pub script_ref: DeckScriptRef,
    /// Active (non-archived) sections in display order.
    pub sections: Vec<DeckSection>,
    /// Archived sections in display order.
    pub archived_sections: Vec<DeckSection>,
}

/// Structured optimistic-concurrency conflict error returned on stale `expected_rev`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SectionConflict {
    pub section_id: String,
    pub expected_rev: u64,
    pub actual_rev: u64,
    pub updated_by: String,
    pub updated_at: i64,
}

/// Errors produced by Deck Y.Doc operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeckDocError {
    NotFound(String),
    NotADeck(String),
    Conflict(SectionConflict),
    Invalid(String),
    Forbidden(String),
    Internal(String),
}

impl std::fmt::Display for DeckDocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::NotADeck(m) => write!(f, "not a deck: {m}"),
            Self::Conflict(c) => write!(
                f,
                "section_conflict: section {} is at rev {} (expected {}; last updated by {})",
                c.section_id, c.actual_rev, c.expected_rev, c.updated_by
            ),
            Self::Invalid(m) => write!(f, "invalid: {m}"),
            Self::Forbidden(m) => write!(f, "{m}"),
            Self::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<String> for DeckDocError {
    fn from(s: String) -> Self {
        Self::Internal(s)
    }
}

impl From<DocError> for DeckDocError {
    fn from(e: DocError) -> Self {
        match e {
            DocError::NotHub { .. } => Self::Forbidden(e.to_string()),
            DocError::Other(s) => Self::Internal(s),
        }
    }
}

impl Refusal for DeckDocError {
    fn refusal(s: String) -> Self {
        if s.starts_with("NotHub:")
            || s.starts_with("not_member")
            || s.starts_with("viewer_readonly")
            || s.starts_with("sharing_disabled")
        {
            Self::Forbidden(s)
        } else {
            Self::Internal(s)
        }
    }
}

/// Root Y.Maps of a Deck document.
pub struct Maps {
    pub meta: MapRef,
    pub sections: MapRef,
    pub order: MapRef,
    pub aliases: MapRef,
    pub script_refs: MapRef,
}

impl Maps {
    pub fn of(doc: &Doc) -> Self {
        Self {
            meta: doc.get_or_insert_map("meta"),
            sections: doc.get_or_insert_map("sections"),
            order: doc.get_or_insert_map("order"),
            aliases: doc.get_or_insert_map("aliases"),
            script_refs: doc.get_or_insert_map("script_refs"),
        }
    }
}

fn new_section_id() -> String {
    format!("sec_{}", uuid::Uuid::now_v7().simple())
}

fn any_str(out: Option<Out>) -> Option<String> {
    match out? {
        Out::Any(Any::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn any_i64(out: Option<Out>) -> Option<i64> {
    match out? {
        Out::Any(Any::Number(n)) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f >= (i64::MIN as f64) && *f <= (i64::MAX as f64))
                .map(|f| f as i64)
        }),
        _ => None,
    }
}

fn any_u64(out: Option<Out>) -> Option<u64> {
    any_i64(out).and_then(|n| u64::try_from(n).ok())
}

fn any_f64(out: Option<Out>) -> Option<f64> {
    let f = match out? {
        Out::Any(Any::Number(n)) => n.as_f64().or_else(|| n.as_i64().map(|i| i as f64)),
        _ => None,
    }?;
    f.is_finite().then_some(f)
}

fn any_bool(out: Option<Out>) -> Option<bool> {
    match out? {
        Out::Any(Any::Bool(b)) => Some(b),
        _ => None,
    }
}

fn map_str(any: &Any, key: &str) -> Option<String> {
    match any {
        Any::Map(m) => match m.get(key)? {
            Any::String(s) => Some(s.to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn map_i64(any: &Any, key: &str) -> Option<i64> {
    match any {
        Any::Map(m) => match m.get(key)? {
            Any::Number(n) => n.as_i64().or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite() && *f >= (i64::MIN as f64) && *f <= (i64::MAX as f64))
                    .map(|f| f as i64)
            }),
            _ => None,
        },
        _ => None,
    }
}

fn map_bool(any: &Any, key: &str) -> Option<bool> {
    match any {
        Any::Map(m) => match m.get(key)? {
            Any::Bool(b) => Some(*b),
            _ => None,
        },
        _ => None,
    }
}

fn next_section_rev(cur_rev: u64) -> u64 {
    cur_rev.saturating_add(1).min(MAX_SECTION_REV)
}

fn normalize_data_key(raw: Option<String>) -> Result<Option<String>, DeckDocError> {
    let Some(s) = raw else {
        return Ok(None);
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > MAX_SECTION_DATA_KEY_BYTES {
        return Err(DeckDocError::Invalid(format!(
            "data_key exceeds {MAX_SECTION_DATA_KEY_BYTES} bytes"
        )));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(DeckDocError::Invalid(
            "data_key must not contain control characters".into(),
        ));
    }
    Ok(Some(trimmed.to_owned()))
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    const SUFFIX: &str = "\n…[truncated]";
    let keep = max_bytes.saturating_sub(SUFFIX.len());
    let mut cut = keep.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(cut + SUFFIX.len());
    out.push_str(&s[..cut]);
    if max_bytes >= SUFFIX.len() {
        out.push_str(SUFFIX);
    }
    out
}

/// Ensure every live section has a unique monotonic alias (`s1`, `s2`, ...).
/// Deterministically resolves concurrent CRDT alias collisions by keeping the lexicographically
/// smallest section ID on the existing alias and reassigning colliding IDs to fresh numbers.
fn ensure_aliases(maps: &Maps, txn: &mut TransactionMut<'_>) -> usize {
    let live_ids: BTreeSet<String> = maps.sections.keys(txn).map(str::to_owned).collect();
    let mut max_n: u64 = 0;
    let mut by_alias: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut assigned: BTreeSet<String> = BTreeSet::new();

    for (k, v) in maps.aliases.iter(txn) {
        if let Out::Any(Any::String(alias)) = v {
            let s = alias.trim().to_owned();
            if let Some(num) = s.strip_prefix('s').and_then(|r| r.parse::<u64>().ok()) {
                max_n = max_n.max(num);
            }
            if live_ids.contains(k) && !s.is_empty() {
                by_alias.entry(s).or_default().push(k.to_owned());
                assigned.insert(k.to_owned());
            }
        }
    }

    let mut needs_alias: Vec<String> = live_ids
        .iter()
        .filter(|id| !assigned.contains(*id))
        .cloned()
        .collect();

    for (_alias, mut owners) in by_alias {
        if owners.len() > 1 {
            owners.sort();
            // Keep owners[0] on the existing alias; reassign the rest deterministically.
            needs_alias.extend(owners.into_iter().skip(1));
        }
    }

    needs_alias.sort();
    needs_alias.dedup();

    let mut changed = 0;
    for id in needs_alias {
        max_n = max_n.saturating_add(1);
        maps.aliases
            .insert(txn, id, Any::String(format!("s{max_n}").into()));
        changed += 1;
    }
    changed
}

/// Deterministically resolve a section ID or alias.
/// If concurrent CRDT writes produced duplicate aliases before `ensure_aliases` runs,
/// returns the lexicographically smallest live section ID (matching `ensure_aliases`).
fn resolve_section_id<T: ReadTxn>(maps: &Maps, txn: &T, id_or_alias: &str) -> Option<String> {
    if maps.sections.contains_key(txn, id_or_alias) {
        return Some(id_or_alias.to_owned());
    }
    let mut best: Option<String> = None;
    for (id, v) in maps.aliases.iter(txn) {
        if let Out::Any(Any::String(alias)) = v
            && alias.as_ref() == id_or_alias
            && maps.sections.contains_key(txn, id)
        {
            match &best {
                Some(cur) if cur.as_str() <= id => {}
                _ => best = Some(id.to_owned()),
            }
        }
    }
    best
}

fn read_section<T: ReadTxn>(
    maps: &Maps,
    txn: &T,
    id: &str,
    sec_map: &MapRef,
) -> Option<DeckSection> {
    let kind_str = any_str(sec_map.get(txn, "kind")).unwrap_or_else(|| "note".into());
    let kind = SectionKind::parse(&kind_str)?;
    let alias = any_str(maps.aliases.get(txn, id))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.to_owned());
    let rank = any_f64(maps.order.get(txn, id)).unwrap_or(0.0);
    let rev = any_u64(sec_map.get(txn, "rev"))
        .unwrap_or(1)
        .clamp(1, MAX_SECTION_REV);
    Some(DeckSection {
        id: id.to_owned(),
        alias,
        kind,
        title: any_str(sec_map.get(txn, "title")).unwrap_or_default(),
        content: any_str(sec_map.get(txn, "content")).unwrap_or_default(),
        col_span: any_u64(sec_map.get(txn, "col_span"))
            .unwrap_or(6)
            .clamp(1, 12) as u32,
        data_key: any_str(sec_map.get(txn, "data_key"))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        archived: any_bool(sec_map.get(txn, "archived")).unwrap_or(false),
        rev,
        rank,
        created_at: any_i64(sec_map.get(txn, "created_at")).unwrap_or(0),
        created_by: any_str(sec_map.get(txn, "created_by")).unwrap_or_else(|| "engine".into()),
        updated_at: any_i64(sec_map.get(txn, "updated_at")).unwrap_or(0),
        updated_by: any_str(sec_map.get(txn, "updated_by")).unwrap_or_else(|| "engine".into()),
    })
}

fn read_history<T: ReadTxn>(sec_map: &MapRef, txn: &T) -> Vec<SectionRevision> {
    let Some(Out::YArray(arr)) = sec_map.get(txn, "history") else {
        return Vec::new();
    };
    arr.iter(txn)
        .filter_map(|item| match item {
            Out::Any(any) => Some(SectionRevision {
                rev: map_i64(&any, "rev")
                    .and_then(|n| u64::try_from(n).ok())?
                    .min(MAX_SECTION_REV),
                at: map_i64(&any, "at").unwrap_or(0),
                by: map_str(&any, "by").unwrap_or_else(|| "engine".into()),
                summary: map_str(&any, "summary").unwrap_or_default(),
                title: map_str(&any, "title").unwrap_or_default(),
                content: map_str(&any, "content").unwrap_or_default(),
                archived: map_bool(&any, "archived").unwrap_or(false),
            }),
            _ => None,
        })
        .collect()
}

fn history_item_bytes(out: &Out) -> usize {
    match out {
        Out::Any(any) => {
            let mut n = 64usize;
            if let Any::Map(m) = any {
                for (k, v) in m.iter() {
                    n += k.len();
                    if let Any::String(s) = v {
                        n += s.len();
                    } else {
                        n += 8;
                    }
                }
            }
            n
        }
        _ => 64,
    }
}

fn push_history(sec_map: &MapRef, txn: &mut TransactionMut<'_>, entry: &SectionRevision) {
    let arr = match sec_map.get(txn, "history") {
        Some(Out::YArray(a)) => a,
        _ => sec_map.insert(txn, "history", ArrayPrelim::default()),
    };
    let mut m: HashMap<String, Any> = HashMap::new();
    let rev = entry.rev.min(MAX_SECTION_REV) as i64;
    let summary = truncate_utf8(&entry.summary, MAX_SECTION_SUMMARY_BYTES);
    let title = truncate_utf8(&entry.title, MAX_SECTION_TITLE_BYTES);
    let content = truncate_utf8(&entry.content, MAX_HISTORY_CONTENT_BYTES);
    m.insert("rev".into(), any_int(rev));
    m.insert("at".into(), any_int(entry.at));
    m.insert("by".into(), Any::String(entry.by.clone().into()));
    m.insert("summary".into(), Any::String(summary.into()));
    m.insert("title".into(), Any::String(title.into()));
    m.insert("content".into(), Any::String(content.into()));
    m.insert("archived".into(), Any::Bool(entry.archived));
    arr.push_back(txn, Any::Map(m.into()));

    trim_section_history(&arr, txn, MAX_SECTION_HISTORY, MAX_SECTION_HISTORY_BYTES, 1);
}

fn trim_section_history(
    arr: &yrs::ArrayRef,
    txn: &mut TransactionMut<'_>,
    max_entries: usize,
    max_bytes: usize,
    keep_min: usize,
) -> usize {
    let sizes: Vec<usize> = arr.iter(txn).map(|o| history_item_bytes(&o)).collect();
    let len = sizes.len();
    if len <= keep_min {
        return 0;
    }
    let mut total: usize = sizes.iter().sum();
    let mut drop = 0usize;
    while (len - drop) > keep_min && ((len - drop) > max_entries || total > max_bytes) {
        total = total.saturating_sub(sizes[drop]);
        drop += 1;
    }
    if drop > 0 {
        arr.remove_range(txn, 0, drop as u32);
    }
    drop
}

/// Enforce a Deck-wide history byte budget so per-section history can never exceed `max_doc_bytes`.
/// Trims archived sections first, then oldest entries from the largest active histories.
fn compact_deck_history(
    maps: &Maps,
    txn: &mut TransactionMut<'_>,
    budget_bytes: usize,
    keep_min_active: usize,
) -> usize {
    let mut sec_ids: Vec<String> = maps.sections.keys(txn).map(str::to_owned).collect();
    sec_ids.sort();

    let mut total_bytes = 0usize;
    let mut entries: Vec<(String, bool, Vec<usize>)> = Vec::new();
    for id in sec_ids {
        if let Some(Out::YMap(sec_map)) = maps.sections.get(txn, &id)
            && let Some(Out::YArray(arr)) = sec_map.get(txn, "history")
        {
            let archived = any_bool(sec_map.get(txn, "archived")).unwrap_or(false);
            let sizes: Vec<usize> = arr.iter(txn).map(|o| history_item_bytes(&o)).collect();
            let sum: usize = sizes.iter().sum();
            total_bytes += sum;
            if !sizes.is_empty() {
                entries.push((id, archived, sizes));
            }
        }
    }

    if total_bytes <= budget_bytes {
        return 0;
    }

    let mut removed = 0usize;

    // Pass 1: trim archived sections first (down to 0 under emergency budget, else 1).
    let keep_archived = if budget_bytes == 0 { 0 } else { 1 };
    for (id, archived, sizes) in &mut entries {
        if total_bytes <= budget_bytes {
            break;
        }
        if !*archived || sizes.len() <= keep_archived {
            continue;
        }
        let mut drop = 0usize;
        while sizes.len() - drop > keep_archived && total_bytes > budget_bytes {
            total_bytes = total_bytes.saturating_sub(sizes[drop]);
            drop += 1;
        }
        if drop > 0
            && let Some(Out::YMap(sec_map)) = maps.sections.get(txn, id)
            && let Some(Out::YArray(arr)) = sec_map.get(txn, "history")
        {
            arr.remove_range(txn, 0, drop as u32);
            sizes.drain(0..drop);
            removed += drop;
        }
    }

    // Pass 2: trim oldest entries from whichever section currently holds the most history bytes.
    while total_bytes > budget_bytes {
        let victim = entries
            .iter()
            .enumerate()
            .filter(|(_, (_, archived, sizes))| {
                let floor = if *archived { 0 } else { keep_min_active };
                sizes.len() > floor
            })
            .max_by(|(_, (id_a, _, sizes_a)), (_, (id_b, _, sizes_b))| {
                let sum_a: usize = sizes_a.iter().sum();
                let sum_b: usize = sizes_b.iter().sum();
                sum_a.cmp(&sum_b).then_with(|| id_b.cmp(id_a))
            })
            .map(|(idx, _)| idx);

        let Some(idx) = victim else {
            break;
        };
        let (id, _, sizes) = &mut entries[idx];
        let dropped_bytes = sizes.remove(0);
        total_bytes = total_bytes.saturating_sub(dropped_bytes);
        if let Some(Out::YMap(sec_map)) = maps.sections.get(txn, id)
            && let Some(Out::YArray(arr)) = sec_map.get(txn, "history")
        {
            arr.remove_range(txn, 0, 1);
            removed += 1;
        }
    }

    removed
}

/// Validate raw client `doc_update` (`ApplyUpdate`) against Deck invariants before mutating live state.
pub(crate) fn validate_raw_update(doc: &Doc, update: &[u8], by: &Principal) -> Result<(), String> {
    let u = Update::decode_v1(update).map_err(|e| format!("bad update: {e}"))?;
    let state = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let scratch = Doc::new();
    if let Ok(base) = Update::decode_v1(&state) {
        scratch
            .transact_mut()
            .apply_update(base)
            .map_err(|e| format!("decode deck state: {e}"))?;
    }
    let maps = Maps::of(&scratch);

    let (
        before_script_refs,
        before_bad_ranks,
        before_section_count,
        before_oversized_content,
        before_bad_revs,
        before_bad_kinds,
    ) = {
        let txn = scratch.transact();
        let sr: BTreeMap<String, String> = maps
            .script_refs
            .iter(&txn)
            .map(|(k, v)| (k.to_owned(), format!("{v:?}")))
            .collect();
        let bad_ranks: BTreeSet<String> = maps
            .order
            .iter(&txn)
            .filter_map(|(k, v)| any_f64(Some(v)).is_none().then(|| k.to_owned()))
            .collect();
        let sec_count = maps.sections.len(&txn) as usize;
        let mut oversized: BTreeSet<String> = BTreeSet::new();
        let mut bad_revs: BTreeMap<String, String> = BTreeMap::new();
        let mut bad_kinds: BTreeMap<String, String> = BTreeMap::new();
        for (sid, out) in maps.sections.iter(&txn) {
            if let Out::YMap(sm) = out {
                let kind_out = sm.get(&txn, "kind");
                let parsed_kind = match kind_out.clone() {
                    Some(ko) => match any_str(Some(ko.clone())).and_then(|k| SectionKind::parse(&k))
                    {
                        Some(k) => Some(k),
                        None => {
                            bad_kinds.insert(sid.to_owned(), format!("{ko:?}"));
                            None
                        }
                    },
                    None => Some(SectionKind::Note),
                };
                let max_bytes = parsed_kind
                    .map(SectionKind::max_content_bytes)
                    .unwrap_or(MAX_SECTION_CONTENT_BYTES);
                if any_str(sm.get(&txn, "content")).is_some_and(|c| c.len() > max_bytes) {
                    oversized.insert(sid.to_owned());
                }
                if let Some(rev_out) = sm.get(&txn, "rev") {
                    let valid = matches!(any_i64(Some(rev_out.clone())), Some(r) if (1..=MAX_RAW_CLIENT_REV).contains(&r));
                    if !valid {
                        bad_revs.insert(sid.to_owned(), format!("{rev_out:?}"));
                    }
                }
            }
        }
        (sr, bad_ranks, sec_count, oversized, bad_revs, bad_kinds)
    };

    scratch
        .transact_mut()
        .apply_update(u)
        .map_err(|e| format!("apply update: {e}"))?;

    let txn = scratch.transact();

    if !matches!(by, Principal::Helper) {
        let after_script_refs: BTreeMap<String, String> = maps
            .script_refs
            .iter(&txn)
            .map(|(k, v)| (k.to_owned(), format!("{v:?}")))
            .collect();
        if after_script_refs != before_script_refs {
            return Err("script_refs_readonly: script_refs is engine-managed".into());
        }
    }

    for (k, v) in maps.order.iter(&txn) {
        if any_f64(Some(v)).is_none() && !before_bad_ranks.contains(k) {
            return Err(format!(
                "invalid_deck_rank: non-finite rank for section {k}"
            ));
        }
    }

    let after_section_count = maps.sections.len(&txn) as usize;
    if after_section_count > MAX_DECK_SECTIONS && after_section_count > before_section_count {
        return Err(format!(
            "invalid_deck_sections: section count {after_section_count} exceeds limit {MAX_DECK_SECTIONS}"
        ));
    }

    for (sid, out) in maps.sections.iter(&txn) {
        if let Out::YMap(sm) = out {
            let parsed_kind = if let Some(kind_out) = sm.get(&txn, "kind") {
                match any_str(Some(kind_out.clone())).and_then(|k| SectionKind::parse(&k)) {
                    Some(k) => Some(k),
                    None => {
                        if before_bad_kinds.get(sid).map(String::as_str)
                            != Some(format!("{kind_out:?}").as_str())
                        {
                            return Err(format!(
                                "invalid_deck_kind: section {sid} has unsupported section kind"
                            ));
                        }
                        None
                    }
                }
            } else {
                Some(SectionKind::Note)
            };
            let max_bytes = parsed_kind
                .map(SectionKind::max_content_bytes)
                .unwrap_or(MAX_SECTION_CONTENT_BYTES);
            if let Some(rev_out) = sm.get(&txn, "rev") {
                let valid = matches!(any_i64(Some(rev_out.clone())), Some(r) if (1..=MAX_RAW_CLIENT_REV).contains(&r));
                if !valid
                    && before_bad_revs.get(sid).map(String::as_str)
                        != Some(format!("{rev_out:?}").as_str())
                {
                    return Err(format!(
                        "invalid_deck_rev: section {sid} has invalid or out-of-range rev"
                    ));
                }
            }
            if let Some(content) = any_str(sm.get(&txn, "content"))
                && content.len() > max_bytes
                && !before_oversized_content.contains(sid)
            {
                return Err(format!(
                    "invalid_deck_content: section {sid} content exceeds {max_bytes} bytes"
                ));
            }
        }
    }

    Ok(())
}

/// Read the complete collaborative Deck snapshot from any `yrs::Doc`.
pub fn read_snapshot(doc_id: &str, title: &str, doc: &Doc) -> DeckDocSnapshot {
    let maps = Maps::of(doc);
    let txn = doc.transact();
    let description = any_str(maps.meta.get(&txn, "description")).unwrap_or_default();
    let layout = any_str(maps.meta.get(&txn, "layout")).unwrap_or_else(|| "grid".into());
    let columns = any_u64(maps.meta.get(&txn, "columns"))
        .unwrap_or(12)
        .clamp(1, 12) as u32;
    let focus = match maps.meta.get(&txn, "focus") {
        Some(Out::Any(any)) => map_str(&any, "section").map(|section| DeckFocus {
            section,
            by: map_str(&any, "by").unwrap_or_else(|| "owner".into()),
            at: map_i64(&any, "at").unwrap_or(0),
        }),
        _ => None,
    };
    let script_ref = DeckScriptRef {
        active_rev: any_u64(maps.script_refs.get(&txn, "active_rev")),
        pending_rev: any_u64(maps.script_refs.get(&txn, "pending_rev")),
        mode: any_str(maps.script_refs.get(&txn, "mode")).filter(|s| !s.is_empty()),
        schedule: any_str(maps.script_refs.get(&txn, "schedule")).filter(|s| !s.is_empty()),
        updated_at: any_i64(maps.script_refs.get(&txn, "updated_at")).unwrap_or(0),
    };

    let mut active = Vec::new();
    let mut archived = Vec::new();
    for (id, out) in maps.sections.iter(&txn) {
        if let Out::YMap(sec_map) = out
            && let Some(sec) = read_section(&maps, &txn, id, &sec_map)
        {
            if sec.archived {
                archived.push(sec);
            } else {
                active.push(sec);
            }
        }
    }
    let cmp_sec =
        |a: &DeckSection, b: &DeckSection| a.rank.total_cmp(&b.rank).then_with(|| a.id.cmp(&b.id));
    active.sort_by(cmp_sec);
    archived.sort_by(cmp_sec);

    DeckDocSnapshot {
        doc_id: doc_id.to_owned(),
        title: title.to_owned(),
        description,
        layout,
        columns,
        focus,
        script_ref,
        sections: active,
        archived_sections: archived,
    }
}

/// Export readable derived markdown for `documents/<id>/deck.md` directly in Rust (never helper).
pub(crate) fn export_text(open: &Open) -> String {
    let snap = read_snapshot(open.meta.id.as_str(), &open.meta.title, &open.doc);
    export_snapshot_markdown(&snap)
}

pub(crate) fn export_snapshot_markdown(snap: &DeckDocSnapshot) -> String {
    let mut out = format!("# {}\n", snap.title);
    if !snap.description.is_empty() {
        out.push('\n');
        out.push_str(&snap.description);
        out.push('\n');
    }
    for sec in &snap.sections {
        out.push_str(&format!(
            "\n## {} ({}, {}, rev {}, span {})\n",
            sec.title,
            sec.alias,
            sec.kind.as_str(),
            sec.rev,
            sec.col_span
        ));
        if sec.kind == SectionKind::Note {
            out.push_str(&sec.content);
            out.push('\n');
        } else {
            out.push_str(&format!(
                "_data_key: {}_\n",
                sec.data_key.as_deref().unwrap_or("none")
            ));
        }
    }
    out
}

/// Try to compute a single-key fractional rank for inserting/moving a section to `index`
/// among `others` without touching any other section's rank.
/// Returns `None` when the gap is `<= MIN_RANK_GAP`, ranks collide/are out of order,
/// or float precision/magnitude limits are reached — signalling in-transaction re-spacing.
fn try_compute_rank(others: &[&DeckSection], index: Option<usize>) -> Option<f64> {
    if others.is_empty() {
        return Some(RANK_STEP);
    }
    let idx = index.unwrap_or(others.len()).min(others.len());
    if idx == 0 {
        let first = others[0].rank;
        if !first.is_finite() || first.abs() >= MAX_RANK_ABS {
            return None;
        }
        let cand = first - RANK_STEP;
        (cand.is_finite() && cand < first && (first - cand) > MIN_RANK_GAP).then_some(cand)
    } else if idx >= others.len() {
        let last = others[others.len() - 1].rank;
        if !last.is_finite() || last.abs() >= MAX_RANK_ABS {
            return None;
        }
        let cand = last + RANK_STEP;
        (cand.is_finite() && cand > last && (cand - last) > MIN_RANK_GAP).then_some(cand)
    } else {
        let lo = others[idx - 1].rank;
        let hi = others[idx].rank;
        if !lo.is_finite() || !hi.is_finite() || hi <= lo || (hi - lo) <= MIN_RANK_GAP {
            return None;
        }
        let mid = lo + (hi - lo) * 0.5;
        (mid.is_finite() && mid > lo && mid < hi).then_some(mid)
    }
}

/// Assign a rank to `target_id` at `index` among `active`, mutating `maps.order` inside `txn`.
/// Uses a single-key midpoint update when safe, and deterministically re-spaces all active
/// sections to `1000.0, 2000.0, ...` inside the same transaction when ranks collide or exhaust gap.
/// Returns `(assigned_rank, final_ordered_ids)`.
fn assign_rank_in_txn(
    maps: &Maps,
    txn: &mut TransactionMut<'_>,
    active: &[DeckSection],
    target_id: &str,
    index: Option<usize>,
) -> (f64, Vec<String>) {
    let others: Vec<&DeckSection> = active.iter().filter(|s| s.id != target_id).collect();
    let idx = index.unwrap_or(others.len()).min(others.len());

    // Also re-space if any active section in `maps.order` currently has a missing/non-finite rank.
    let all_finite_in_map = others
        .iter()
        .all(|s| any_f64(maps.order.get(txn, &s.id)).is_some());

    if all_finite_in_map && let Some(rank) = try_compute_rank(&others, Some(idx)) {
        maps.order
            .insert(txn, target_id.to_owned(), any_float(rank));
        let mut ids: Vec<String> = Vec::with_capacity(others.len() + 1);
        for s in &others[..idx] {
            ids.push(s.id.clone());
        }
        ids.push(target_id.to_owned());
        for s in &others[idx..] {
            ids.push(s.id.clone());
        }
        return (rank, ids);
    }

    // Deterministic in-transaction re-spacing: 1000.0, 2000.0, 3000.0, ...
    let mut ordered_ids: Vec<String> = Vec::with_capacity(others.len() + 1);
    for s in &others[..idx] {
        ordered_ids.push(s.id.clone());
    }
    ordered_ids.push(target_id.to_owned());
    for s in &others[idx..] {
        ordered_ids.push(s.id.clone());
    }

    let mut target_rank = RANK_STEP;
    for (pos, sid) in ordered_ids.iter().enumerate() {
        let r = ((pos + 1) as f64) * RANK_STEP;
        maps.order.insert(txn, sid.clone(), any_float(r));
        if sid == target_id {
            target_rank = r;
        }
    }
    (target_rank, ordered_ids)
}

// ---------------------------------------------------------------------------
// Actor messages
// ---------------------------------------------------------------------------

/// Create a new `DocKind::Deck` pure-Yjs document.
#[derive(Clone, Debug)]
pub struct CreateDeck {
    pub title: Option<String>,
    pub description: Option<String>,
    pub by: Principal,
}

/// Internal routed Deck create message preserving explicit [`Homing`] (used by [`CreateReq::Deck`]).
#[derive(Clone, Debug)]
pub(crate) struct CreateDeckHomed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub by: Principal,
    pub home: Homing,
}

/// Reply to [`CreateDeck`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckCreated {
    pub doc: DocId,
    pub title: String,
}

/// Operations executed on a `DocKind::Deck` document via [`DeckCall`].
fn deserialize_optional_nullable<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeckOp {
    Read,
    SectionAdd {
        kind: SectionKind,
        title: String,
        content: String,
        col_span: Option<u32>,
        data_key: Option<String>,
        index: Option<usize>,
    },
    SectionPatch {
        section_id: String,
        expected_rev: Option<u64>,
        title: Option<String>,
        content: Option<String>,
        col_span: Option<u32>,
        #[serde(default, deserialize_with = "deserialize_optional_nullable")]
        data_key: Option<Option<String>>,
        summary: Option<String>,
    },
    SectionMove {
        section_id: String,
        to_index: usize,
    },
    SectionArchive {
        section_id: String,
        archived: bool,
        expected_rev: Option<u64>,
    },
    SectionHistory {
        section_id: String,
    },
    Focus {
        section_id: Option<String>,
    },
    UpdateScriptRef {
        script_ref: DeckScriptRef,
    },
}

/// Execute a collaborative Deck document operation on [`DocumentActor`].
#[derive(Clone, Debug)]
pub struct DeckCall {
    pub doc: DocId,
    pub by: Principal,
    pub op: DeckOp,
}

/// Reply payload of [`DeckCall`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeckCallReply {
    Snapshot(DeckDocSnapshot),
    Section(DeckSection),
    Order(Vec<String>),
    History(Vec<SectionRevision>),
    Ok,
}

fn principal_label(by: &Principal) -> String {
    match by {
        Principal::Agent { session } => format!("session:{session}"),
        Principal::Owner { .. } | Principal::File => "owner".to_owned(),
        Principal::Member { pub_, .. } => format!("member:{pub_}"),
        Principal::Helper => "engine".to_owned(),
    }
}

impl DocumentActor {
    fn deck_open(&mut self, doc: &DocId) -> Result<&mut Open, DeckDocError> {
        let o = self.load(doc).map_err(DeckDocError::NotFound)?;
        if o.meta.kind != DocKind::Deck {
            return Err(DeckDocError::NotADeck(format!(
                "{doc} is a {}, not a deck",
                o.meta.kind
            )));
        }
        Ok(o)
    }

    /// Execute one atomic local-diff Y.Doc write on a Deck document via a scratch doc,
    /// routing through the single `DocumentActor::apply` persistence & fan-out path.
    async fn deck_write<R>(
        &mut self,
        doc: &DocId,
        by: Principal,
        f: impl FnOnce(&Maps, &mut TransactionMut<'_>, &str, i64) -> Result<(R, bool), DeckDocError>,
    ) -> Result<R, DeckDocError> {
        self.deck_write_with_client(doc, by, *ENGINE_CLIENT, f)
            .await
    }

    async fn deck_write_with_client<R>(
        &mut self,
        doc: &DocId,
        by: Principal,
        client_id: u64,
        f: impl FnOnce(&Maps, &mut TransactionMut<'_>, &str, i64) -> Result<(R, bool), DeckDocError>,
    ) -> Result<R, DeckDocError> {
        let max_bytes = self.cfg.max_doc_bytes;
        let (state, sv, pre_bytes) = {
            let o = self.deck_open(doc)?;
            let t = o.doc.transact();
            (
                t.encode_state_as_update_v1(&StateVector::default()),
                t.state_vector(),
                o.bytes,
            )
        };
        let scratch = Doc::with_client_id(client_id);
        scratch
            .transact_mut()
            .apply_update(
                Update::decode_v1(&state).map_err(|e| DeckDocError::Internal(e.to_string()))?,
            )
            .map_err(|e| DeckDocError::Internal(e.to_string()))?;
        let maps = Maps::of(&scratch);
        let who = principal_label(&by);
        let now = now_ms();
        let (result, dirty) = {
            let mut txn = scratch.transact_mut();
            let added_before = ensure_aliases(&maps, &mut txn);
            let (r, mutated) = f(&maps, &mut txn, &who, now)?;
            let added_after = ensure_aliases(&maps, &mut txn);
            let mut compacted = if pre_bytes >= max_bytes {
                // Emergency compaction so an already-oversized Deck can always be compacted/archived.
                let c = compact_deck_history(&maps, &mut txn, 64 * 1024, 1);
                if c == 0 {
                    compact_deck_history(&maps, &mut txn, 0, 0)
                } else {
                    c
                }
            } else if pre_bytes >= max_bytes.saturating_mul(3) / 4 {
                compact_deck_history(&maps, &mut txn, MAX_DECK_HISTORY_BYTES / 2, 1)
            } else {
                compact_deck_history(&maps, &mut txn, MAX_DECK_HISTORY_BYTES, 1)
            };
            if pre_bytes >= max_bytes {
                // Ensure oversized legacy single-entry histories (> 64 KiB each) also shed weight.
                compacted += compact_deck_history(&maps, &mut txn, 64 * 1024, 0);
            }
            (
                r,
                mutated || added_before > 0 || added_after > 0 || compacted > 0,
            )
        };
        if dirty {
            let update = scratch.transact().encode_diff_v1(&sv);
            self.apply(doc, update, by, false, Vec::new())
                .await
                .map_err(DeckDocError::Internal)?;
        }
        Ok(result)
    }

    async fn deck_create_routed(
        &mut self,
        title: Option<String>,
        description: Option<String>,
        by: Principal,
        home: Homing,
        tx: ReplySender<Result<DeckCreated, String>>,
    ) {
        if by.member().is_some() {
            tx.send(Err("not_owner: members cannot create decks".into()));
            return;
        }
        let title = title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Untitled Deck".to_owned());
        if title.len() > MAX_DECK_TITLE_BYTES {
            tx.send(Err(format!(
                "deck title exceeds {MAX_DECK_TITLE_BYTES} bytes"
            )));
            return;
        }
        let description = description.unwrap_or_default();
        if description.len() > MAX_DECK_DESCRIPTION_BYTES {
            tx.send(Err(format!(
                "deck description exceeds {MAX_DECK_DESCRIPTION_BYTES} bytes"
            )));
            return;
        }
        let home = self.resolve_home(&home, DocKind::Deck);
        match self.route_home(home.as_ref()) {
            Ok(None) => tx.send(self.deck_create_here(title, description, by, home).await),
            Ok(Some(to)) => {
                let id = DocId::for_binding(&Binding::Deck);
                let client = Client::of(&by);
                let req = CreateReq::Deck {
                    title,
                    description,
                    by,
                    home: home.clone(),
                };
                let ask = self.create_on::<DeckCreated>(to, id, client, req, home);
                tokio::spawn(async move { tx.send(ask.await) });
            }
            Err(e) => tx.send(Err(e.into())),
        }
    }

    async fn deck_create_here(
        &mut self,
        title: String,
        description: String,
        by: Principal,
        home: Option<mesh::EngineId>,
    ) -> Result<DeckCreated, String> {
        let binding = Binding::Deck;
        let id = DocId::for_binding(&binding);
        let now = now_ms();
        let homed = home.is_some();
        let meta = DocMeta {
            derived: binding.derived(&id),
            id: id.clone(),
            kind: DocKind::Deck,
            title: title.clone(),
            binding,
            fragment: FRAGMENT.to_owned(),
            members: vec![],
            links: vec![],
            agent: AgentStyle {
                color: self.cfg.agent_color.clone(),
            },
            last_export: None,
            authors: Default::default(),
            created_ms: now,
            updated_ms: now,
            home,
            home_ms: if homed { now } else { 0 },
            serve_epoch: i64::from(homed),
            served_by: if homed { self.hub.self_id() } else { None },
            revoked: Default::default(),
        };
        self.store
            .create(&meta)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if homed {
            self.claims
                .insert(id.clone(), crate::hub::Claim::Held { epoch: 1 });
        }
        self.homes.insert(id.clone(), meta.home.clone());
        self.rt.publish(DocumentChanged::of(&meta, false)).await;
        self.deck_write(&id, by, |maps, txn, who, now| {
            maps.meta.insert(txn, "v", any_int(i64::from(SCHEMA)));
            maps.meta.insert(txn, "created", any_int(now));
            maps.meta.insert(txn, "created_by", Any::String(who.into()));
            maps.meta
                .insert(txn, "description", Any::String(description.into()));
            maps.meta.insert(txn, "layout", Any::String("grid".into()));
            maps.meta.insert(txn, "columns", any_int(12));
            Ok(((), true))
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(DeckCreated { doc: id, title })
    }

    async fn deck_call(&mut self, m: DeckCall) -> Result<DeckCallReply, DeckDocError> {
        let is_write = !matches!(m.op, DeckOp::Read | DeckOp::SectionHistory { .. });
        self.admit(&m.doc, &m.by, is_write).await.map_err(|e| {
            if matches!(
                e.as_str(),
                "not_member" | "viewer_readonly" | "sharing_disabled"
            ) {
                DeckDocError::Forbidden(e)
            } else {
                DeckDocError::NotFound(e)
            }
        })?;

        match m.op {
            DeckOp::Read => {
                let o = self.deck_open(&m.doc)?;
                let snap = read_snapshot(o.meta.id.as_str(), &o.meta.title, &o.doc);
                Ok(DeckCallReply::Snapshot(snap))
            }
            DeckOp::SectionHistory { section_id } => {
                let o = self.deck_open(&m.doc)?;
                let maps = Maps::of(&o.doc);
                let txn = o.doc.transact();
                let id = resolve_section_id(&maps, &txn, &section_id)
                    .ok_or_else(|| DeckDocError::NotFound(format!("section {section_id}")))?;
                let Some(Out::YMap(sec_map)) = maps.sections.get(&txn, &id) else {
                    return Err(DeckDocError::NotFound(format!("section {section_id}")));
                };
                Ok(DeckCallReply::History(read_history(&sec_map, &txn)))
            }
            DeckOp::SectionAdd {
                kind,
                title,
                content,
                col_span,
                data_key,
                index,
            } => {
                let max_bytes = kind.max_content_bytes();
                if content.len() > max_bytes {
                    return Err(DeckDocError::Invalid(format!(
                        "section content exceeds {max_bytes} bytes"
                    )));
                }
                let title = title.trim().to_owned();
                if title.is_empty() {
                    return Err(DeckDocError::Invalid(
                        "section title cannot be empty".into(),
                    ));
                }
                if title.len() > MAX_SECTION_TITLE_BYTES {
                    return Err(DeckDocError::Invalid(format!(
                        "section title exceeds {MAX_SECTION_TITLE_BYTES} bytes"
                    )));
                }
                let normalized_dk = normalize_data_key(data_key)?;
                let span = col_span.unwrap_or(kind.default_col_span()).clamp(1, 12);

                let sec = self
                    .deck_write(&m.doc, m.by, |maps, txn, who, now| {
                        if maps.sections.len(txn) as usize >= MAX_DECK_SECTIONS {
                            return Err(DeckDocError::Invalid(format!(
                                "deck has reached the maximum of {MAX_DECK_SECTIONS} sections"
                            )));
                        }
                        // Compute rank among active sections (re-spacing in-transaction if needed).
                        let mut active = Vec::new();
                        for (sid, out) in maps.sections.iter(txn) {
                            if let Out::YMap(sm) = out
                                && let Some(s) = read_section(maps, txn, sid, &sm)
                                && !s.archived
                            {
                                active.push(s);
                            }
                        }
                        active.sort_by(|a, b| {
                            a.rank.total_cmp(&b.rank).then_with(|| a.id.cmp(&b.id))
                        });
                        let id = new_section_id();
                        let (_rank, _ids) = assign_rank_in_txn(maps, txn, &active, &id, index);
                        let sec_map = maps.sections.insert(txn, id.clone(), MapPrelim::default());
                        sec_map.insert(txn, "id", Any::String(id.clone().into()));
                        sec_map.insert(txn, "kind", Any::String(kind.as_str().into()));
                        sec_map.insert(txn, "title", Any::String(title.clone().into()));
                        sec_map.insert(txn, "content", Any::String(content.clone().into()));
                        sec_map.insert(txn, "col_span", any_int(i64::from(span)));
                        if let Some(dk) = normalized_dk.as_deref() {
                            sec_map.insert(txn, "data_key", Any::String(dk.into()));
                        }
                        sec_map.insert(txn, "archived", Any::Bool(false));
                        sec_map.insert(txn, "rev", any_int(1));
                        sec_map.insert(txn, "created_at", any_int(now));
                        sec_map.insert(txn, "created_by", Any::String(who.into()));
                        sec_map.insert(txn, "updated_at", any_int(now));
                        sec_map.insert(txn, "updated_by", Any::String(who.into()));
                        push_history(
                            &sec_map,
                            txn,
                            &SectionRevision {
                                rev: 1,
                                at: now,
                                by: who.to_owned(),
                                summary: "created".into(),
                                title: title.clone(),
                                content: content.clone(),
                                archived: false,
                            },
                        );
                        ensure_aliases(maps, txn);
                        let sec = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                            DeckDocError::Internal("failed to read created section".into())
                        })?;
                        Ok((sec, true))
                    })
                    .await?;
                Ok(DeckCallReply::Section(sec))
            }
            DeckOp::SectionPatch {
                section_id,
                expected_rev,
                title,
                content,
                col_span,
                data_key,
                summary,
            } => {
                if let Some(c) = content.as_ref()
                    && c.len() > MAX_SECTION_CONTENT_BYTES
                {
                    return Err(DeckDocError::Invalid(format!(
                        "section content exceeds {MAX_SECTION_CONTENT_BYTES} bytes"
                    )));
                }
                if let Some(s) = summary.as_ref()
                    && s.len() > MAX_SECTION_SUMMARY_BYTES
                {
                    return Err(DeckDocError::Invalid(format!(
                        "section summary exceeds {MAX_SECTION_SUMMARY_BYTES} bytes"
                    )));
                }
                let normalized_dk = match data_key {
                    Some(dk_opt) => Some(normalize_data_key(dk_opt)?),
                    None => None,
                };
                let sec = self
                    .deck_write(&m.doc, m.by, |maps, txn, who, now| {
                        let id = resolve_section_id(maps, txn, &section_id).ok_or_else(|| {
                            DeckDocError::NotFound(format!("section {section_id}"))
                        })?;
                        let Some(Out::YMap(sec_map)) = maps.sections.get(txn, &id) else {
                            return Err(DeckDocError::NotFound(format!("section {section_id}")));
                        };
                        let cur = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                            DeckDocError::NotFound(format!("section {section_id}"))
                        })?;
                        if let Some(exp) = expected_rev
                            && cur.rev != exp
                        {
                            return Err(DeckDocError::Conflict(SectionConflict {
                                section_id: cur.id,
                                expected_rev: exp,
                                actual_rev: cur.rev,
                                updated_by: cur.updated_by,
                                updated_at: cur.updated_at,
                            }));
                        }
                        if cur.archived {
                            return Err(DeckDocError::Invalid(format!(
                                "section {id} is archived; unarchive it before patching"
                            )));
                        }
                        if let Some(c) = content.as_ref() {
                            let max_bytes = cur.kind.max_content_bytes();
                            if c.len() > max_bytes {
                                return Err(DeckDocError::Invalid(format!(
                                    "section content exceeds {max_bytes} bytes"
                                )));
                            }
                        }
                        let mut changed = Vec::new();
                        if let Some(t) = title {
                            let t = t.trim().to_owned();
                            if t.is_empty() {
                                return Err(DeckDocError::Invalid(
                                    "section title cannot be empty".into(),
                                ));
                            }
                            if t.len() > MAX_SECTION_TITLE_BYTES {
                                return Err(DeckDocError::Invalid(format!(
                                    "section title exceeds {MAX_SECTION_TITLE_BYTES} bytes"
                                )));
                            }
                            if t != cur.title {
                                sec_map.insert(txn, "title", Any::String(t.into()));
                                changed.push("title");
                            }
                        }
                        if let Some(c) = content
                            && c != cur.content
                        {
                            sec_map.insert(txn, "content", Any::String(c.into()));
                            changed.push("content");
                        }
                        if let Some(span) = col_span {
                            let span = span.clamp(1, 12);
                            if span != cur.col_span {
                                sec_map.insert(txn, "col_span", any_int(i64::from(span)));
                                changed.push("col_span");
                            }
                        }
                        if let Some(normalized) = normalized_dk
                            && normalized != cur.data_key
                        {
                            match normalized {
                                Some(dk) => {
                                    sec_map.insert(txn, "data_key", Any::String(dk.into()));
                                }
                                None => {
                                    sec_map.remove(txn, "data_key");
                                }
                            }
                            changed.push("data_key");
                        }
                        if changed.is_empty() {
                            return Ok((cur, false));
                        }
                        let next_rev = next_section_rev(cur.rev);
                        sec_map.insert(txn, "rev", any_int(next_rev as i64));
                        sec_map.insert(txn, "updated_at", any_int(now));
                        sec_map.insert(txn, "updated_by", Any::String(who.into()));
                        let updated = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                            DeckDocError::Internal("failed to read patched section".into())
                        })?;
                        push_history(
                            &sec_map,
                            txn,
                            &SectionRevision {
                                rev: next_rev,
                                at: now,
                                by: who.to_owned(),
                                summary: summary
                                    .unwrap_or_else(|| format!("updated {}", changed.join(", "))),
                                title: updated.title.clone(),
                                content: updated.content.clone(),
                                archived: updated.archived,
                            },
                        );
                        Ok((updated, true))
                    })
                    .await?;
                Ok(DeckCallReply::Section(sec))
            }
            DeckOp::SectionMove {
                section_id,
                to_index,
            } => {
                let order = self
                    .deck_write(&m.doc, m.by, |maps, txn, _who, _now| {
                        let id = resolve_section_id(maps, txn, &section_id).ok_or_else(|| {
                            DeckDocError::NotFound(format!("section {section_id}"))
                        })?;
                        let Some(Out::YMap(sec_map)) = maps.sections.get(txn, &id) else {
                            return Err(DeckDocError::NotFound(format!("section {section_id}")));
                        };
                        let cur = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                            DeckDocError::NotFound(format!("section {section_id}"))
                        })?;
                        if cur.archived {
                            return Err(DeckDocError::Invalid(format!(
                                "section {id} is archived; unarchive it before moving"
                            )));
                        }
                        let mut active = Vec::new();
                        for (sid, out) in maps.sections.iter(txn) {
                            if let Out::YMap(sm) = out
                                && let Some(s) = read_section(maps, txn, sid, &sm)
                                && !s.archived
                            {
                                active.push(s);
                            }
                        }
                        active.sort_by(|a, b| {
                            a.rank.total_cmp(&b.rank).then_with(|| a.id.cmp(&b.id))
                        });
                        let (_new_rank, ids) =
                            assign_rank_in_txn(maps, txn, &active, &id, Some(to_index));
                        Ok((ids, true))
                    })
                    .await?;
                Ok(DeckCallReply::Order(order))
            }
            DeckOp::SectionArchive {
                section_id,
                archived,
                expected_rev,
            } => {
                let sec = self
                    .deck_write(&m.doc, m.by, |maps, txn, who, now| {
                        let id = resolve_section_id(maps, txn, &section_id).ok_or_else(|| {
                            DeckDocError::NotFound(format!("section {section_id}"))
                        })?;
                        let Some(Out::YMap(sec_map)) = maps.sections.get(txn, &id) else {
                            return Err(DeckDocError::NotFound(format!("section {section_id}")));
                        };
                        let cur = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                            DeckDocError::NotFound(format!("section {section_id}"))
                        })?;
                        if let Some(exp) = expected_rev
                            && cur.rev != exp
                        {
                            return Err(DeckDocError::Conflict(SectionConflict {
                                section_id: cur.id,
                                expected_rev: exp,
                                actual_rev: cur.rev,
                                updated_by: cur.updated_by,
                                updated_at: cur.updated_at,
                            }));
                        }
                        if cur.archived == archived {
                            return Ok((cur, false));
                        }
                        let next_rev = next_section_rev(cur.rev);
                        sec_map.insert(txn, "archived", Any::Bool(archived));
                        sec_map.insert(txn, "rev", any_int(next_rev as i64));
                        sec_map.insert(txn, "updated_at", any_int(now));
                        sec_map.insert(txn, "updated_by", Any::String(who.into()));
                        if archived
                            && let Some(Out::Any(any)) = maps.meta.get(txn, "focus")
                            && map_str(&any, "section").as_deref() == Some(id.as_str())
                        {
                            maps.meta.remove(txn, "focus");
                        }
                        let updated = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                            DeckDocError::Internal("failed to read archived section".into())
                        })?;
                        push_history(
                            &sec_map,
                            txn,
                            &SectionRevision {
                                rev: next_rev,
                                at: now,
                                by: who.to_owned(),
                                summary: if archived { "archived" } else { "unarchived" }.into(),
                                title: updated.title.clone(),
                                content: updated.content.clone(),
                                archived,
                            },
                        );
                        Ok((updated, true))
                    })
                    .await?;
                Ok(DeckCallReply::Section(sec))
            }
            DeckOp::Focus { section_id } => {
                self.deck_write(&m.doc, m.by, |maps, txn, who, now| {
                    match section_id {
                        Some(ref target) => {
                            let id = resolve_section_id(maps, txn, target).ok_or_else(|| {
                                DeckDocError::NotFound(format!("section {target}"))
                            })?;
                            let Some(Out::YMap(sec_map)) = maps.sections.get(txn, &id) else {
                                return Err(DeckDocError::NotFound(format!("section {target}")));
                            };
                            let cur = read_section(maps, txn, &id, &sec_map).ok_or_else(|| {
                                DeckDocError::NotFound(format!("section {target}"))
                            })?;
                            if cur.archived {
                                return Err(DeckDocError::Invalid(format!(
                                    "section {id} is archived; cannot focus an archived section"
                                )));
                            }
                            let mut f: HashMap<String, Any> = HashMap::new();
                            f.insert("section".into(), Any::String(id.into()));
                            f.insert("by".into(), Any::String(who.into()));
                            f.insert("at".into(), any_int(now));
                            maps.meta.insert(txn, "focus", Any::Map(f.into()));
                        }
                        None => {
                            maps.meta.remove(txn, "focus");
                        }
                    }
                    Ok(((), true))
                })
                .await?;
                Ok(DeckCallReply::Ok)
            }
            DeckOp::UpdateScriptRef { script_ref } => {
                if !matches!(m.by, Principal::Helper) {
                    return Err(DeckDocError::Forbidden(
                        "script_refs is engine-managed; only Principal::Helper may update it"
                            .into(),
                    ));
                }
                // Only metadata pointers — never Python source or authorization flags.
                self.deck_write(&m.doc, Principal::Helper, |maps, txn, _who, now| {
                    match script_ref.active_rev {
                        Some(r) => {
                            maps.script_refs.insert(
                                txn,
                                "active_rev",
                                any_int(r.min(MAX_SECTION_REV) as i64),
                            );
                        }
                        None => {
                            maps.script_refs.remove(txn, "active_rev");
                        }
                    }
                    match script_ref.pending_rev {
                        Some(r) => {
                            maps.script_refs.insert(
                                txn,
                                "pending_rev",
                                any_int(r.min(MAX_SECTION_REV) as i64),
                            );
                        }
                        None => {
                            maps.script_refs.remove(txn, "pending_rev");
                        }
                    }
                    match script_ref.mode {
                        Some(m) => {
                            maps.script_refs.insert(txn, "mode", Any::String(m.into()));
                        }
                        None => {
                            maps.script_refs.remove(txn, "mode");
                        }
                    }
                    match script_ref.schedule {
                        Some(s) => {
                            maps.script_refs
                                .insert(txn, "schedule", Any::String(s.into()));
                        }
                        None => {
                            maps.script_refs.remove(txn, "schedule");
                        }
                    }
                    maps.script_refs.insert(
                        txn,
                        "updated_at",
                        any_int(if script_ref.updated_at > 0 {
                            script_ref.updated_at
                        } else {
                            now
                        }),
                    );
                    Ok(((), true))
                })
                .await?;
                Ok(DeckCallReply::Ok)
            }
        }
    }
}

impl Message<CreateDeck> for DocumentActor {
    type Reply = DelegatedReply<Result<DeckCreated, String>>;

    async fn handle(&mut self, m: CreateDeck, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.deck_create_routed(m.title, m.description, m.by, Homing::Here, tx)
            .await;
        reply
    }
}

impl Message<CreateDeckHomed> for DocumentActor {
    type Reply = DelegatedReply<Result<DeckCreated, String>>;

    async fn handle(
        &mut self,
        m: CreateDeckHomed,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.deck_create_routed(m.title, m.description, m.by, m.home, tx)
            .await;
        reply
    }
}

impl Message<DeckCall> for DocumentActor {
    type Reply = DelegatedReply<Result<DeckCallReply, DeckDocError>>;

    async fn handle(&mut self, m: DeckCall, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.deck_call(m).await);
        reply
    }
}

/// Pure JSON representation for tests/debugging (does not include executable code).
pub fn snapshot_summary_json(snap: &DeckDocSnapshot) -> Value {
    json!({
        "doc_id": snap.doc_id,
        "title": snap.title,
        "sections": snap.sections.len(),
        "archived_sections": snap.archived_sections.len(),
        "focus": snap.focus,
        "script_ref": snap.script_ref,
    })
}

/// Helper to read section metadata maps cleanly for external tests.
pub fn section_revs(snap: &DeckDocSnapshot) -> BTreeMap<String, u64> {
    snap.sections
        .iter()
        .map(|s| (s.id.clone(), s.rev))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use base64::Engine as _;
    use kameo::actor::Spawn;
    use runtime::Runtime;

    use crate::actor::{ApplyUpdate, DeleteNote, Restore, Snapshot, Subscribe};
    use crate::helper::HelperEvent;
    use crate::hub::SelfId;
    use crate::meta::{DocEvent, Member, Role};
    use crate::share::{MemberRevoke, MemberSet};

    #[derive(kameo::Actor)]
    struct ChangeSink(tokio::sync::mpsc::UnboundedSender<DocumentChanged>);

    impl Message<DocumentChanged> for ChangeSink {
        type Reply = ();

        async fn handle(
            &mut self,
            msg: DocumentChanged,
            _: &mut Context<Self, Self::Reply>,
        ) -> Self::Reply {
            let _ = self.0.send(msg);
        }
    }

    struct TestEnv {
        dir: tempfile::TempDir,
        rt: Runtime,
        store: crate::store::Store,
        actor: kameo::actor::ActorRef<DocumentActor>,
        helper_calls: Arc<AtomicUsize>,
    }

    async fn test_env_with_cfg(dir: tempfile::TempDir, cfg: crate::DocumentsConfig) -> TestEnv {
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let store = crate::store::Store::open(dir.path()).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper_calls = Arc::new(AtomicUsize::new(0));
        let seen = helper_calls.clone();
        let helper = crate::helper::Helper::fake(tx, move |_line| {
            seen.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        });
        let actor =
            DocumentActor::spawn(DocumentActor::new(store.clone(), helper, cfg, rt.clone()));
        TestEnv {
            dir,
            rt,
            store,
            actor,
            helper_calls,
        }
    }

    async fn test_env() -> TestEnv {
        test_env_with_cfg(
            tempfile::tempdir().unwrap(),
            crate::DocumentsConfig::default(),
        )
        .await
    }

    async fn test_actor() -> (tempfile::TempDir, kameo::actor::ActorRef<DocumentActor>) {
        let env = test_env().await;
        (env.dir, env.actor)
    }

    fn owner() -> Principal {
        Principal::Owner { device: None }
    }

    fn b64_update(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn init_peer_deck(client_id: u64, title: &str) -> Doc {
        let doc = Doc::with_client_id(client_id);
        let maps = Maps::of(&doc);
        let mut txn = doc.transact_mut();
        maps.meta.insert(&mut txn, "v", any_int(i64::from(SCHEMA)));
        maps.meta
            .insert(&mut txn, "created", any_int(1_700_000_000_000));
        maps.meta
            .insert(&mut txn, "created_by", Any::String("owner".into()));
        maps.meta
            .insert(&mut txn, "description", Any::String(title.into()));
        maps.meta
            .insert(&mut txn, "layout", Any::String("grid".into()));
        maps.meta.insert(&mut txn, "columns", any_int(12));
        drop(txn);
        doc
    }

    fn peer_add_section(
        doc: &Doc,
        id: &str,
        kind: SectionKind,
        title: &str,
        content: &str,
        who: &str,
        index: Option<usize>,
    ) {
        let maps = Maps::of(doc);
        let mut txn = doc.transact_mut();
        let mut active = Vec::new();
        for (sid, out) in maps.sections.iter(&txn) {
            if let Out::YMap(sm) = out
                && let Some(s) = read_section(&maps, &txn, sid, &sm)
                && !s.archived
            {
                active.push(s);
            }
        }
        active.sort_by(|a, b| a.rank.total_cmp(&b.rank).then_with(|| a.id.cmp(&b.id)));
        let _ = assign_rank_in_txn(&maps, &mut txn, &active, id, index);
        let sec_map = maps
            .sections
            .insert(&mut txn, id.to_owned(), MapPrelim::default());
        sec_map.insert(&mut txn, "id", Any::String(id.into()));
        sec_map.insert(&mut txn, "kind", Any::String(kind.as_str().into()));
        sec_map.insert(&mut txn, "title", Any::String(title.into()));
        sec_map.insert(&mut txn, "content", Any::String(content.into()));
        sec_map.insert(&mut txn, "col_span", any_int(6));
        sec_map.insert(&mut txn, "archived", Any::Bool(false));
        sec_map.insert(&mut txn, "rev", any_int(1));
        sec_map.insert(&mut txn, "created_at", any_int(1_700_000_000_001));
        sec_map.insert(&mut txn, "created_by", Any::String(who.into()));
        sec_map.insert(&mut txn, "updated_at", any_int(1_700_000_000_001));
        sec_map.insert(&mut txn, "updated_by", Any::String(who.into()));
        push_history(
            &sec_map,
            &mut txn,
            &SectionRevision {
                rev: 1,
                at: 1_700_000_000_001,
                by: who.to_owned(),
                summary: "created".into(),
                title: title.to_owned(),
                content: content.to_owned(),
                archived: false,
            },
        );
        ensure_aliases(&maps, &mut txn);
    }

    fn peer_patch_section(doc: &Doc, id: &str, new_title: &str, new_content: &str, who: &str) {
        let maps = Maps::of(doc);
        let mut txn = doc.transact_mut();
        let Out::YMap(sec_map) = maps.sections.get(&txn, id).unwrap() else {
            panic!("missing section {id}");
        };
        let cur = read_section(&maps, &txn, id, &sec_map).unwrap();
        let next_rev = next_section_rev(cur.rev);
        sec_map.insert(&mut txn, "title", Any::String(new_title.into()));
        sec_map.insert(&mut txn, "content", Any::String(new_content.into()));
        sec_map.insert(&mut txn, "rev", any_int(next_rev as i64));
        sec_map.insert(&mut txn, "updated_at", any_int(1_700_000_000_100));
        sec_map.insert(&mut txn, "updated_by", Any::String(who.into()));
        push_history(
            &sec_map,
            &mut txn,
            &SectionRevision {
                rev: next_rev,
                at: 1_700_000_000_100,
                by: who.to_owned(),
                summary: "peer patch".into(),
                title: new_title.to_owned(),
                content: new_content.to_owned(),
                archived: false,
            },
        );
    }

    fn sync_docs(a: &Doc, b: &Doc) {
        let sv_a = a.transact().state_vector();
        let sv_b = b.transact().state_vector();
        let diff_a_for_b = a.transact().encode_state_as_update_v1(&sv_b);
        let diff_b_for_a = b.transact().encode_state_as_update_v1(&sv_a);
        b.transact_mut()
            .apply_update(Update::decode_v1(&diff_a_for_b).unwrap())
            .unwrap();
        a.transact_mut()
            .apply_update(Update::decode_v1(&diff_b_for_a).unwrap())
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_section_edits_and_cas_conflicts() {
        let (_dir, actor) = test_actor().await;
        let created = actor
            .ask(CreateDeck {
                title: Some("Ops Deck".into()),
                description: Some("Team operations".into()),
                by: owner(),
            })
            .await
            .unwrap();

        let agent = Principal::Agent {
            session: uuid::Uuid::new_v4(),
        };

        // Add two independent sections.
        let DeckCallReply::Section(sec_a) = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "Owner Notes".into(),
                    content: "Initial owner note".into(),
                    col_span: Some(6),
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };

        let DeckCallReply::Section(sec_b) = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: agent.clone(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::JsonRender,
                    title: "Live KPIs".into(),
                    content: r#"{"root":"c","elements":{"c":{"type":"Card","props":{"title":"KPIs"},"children":[]}}}"#.into(),
                    col_span: Some(6),
                    data_key: Some("kpis".into()),
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };

        assert_eq!(sec_a.rev, 1);
        assert_eq!(sec_b.rev, 1);
        assert_eq!(sec_a.alias, "s1");
        assert_eq!(sec_b.alias, "s2");

        // Concurrent edits to DIFFERENT sections both succeed cleanly with independent revs.
        let (res_a, res_b) = tokio::join!(
            actor.ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionPatch {
                    section_id: sec_a.id.clone(),
                    expected_rev: Some(1),
                    title: None,
                    content: Some("Updated by owner".into()),
                    col_span: None,
                    data_key: None,
                    summary: Some("owner edit".into()),
                },
            }),
            actor.ask(DeckCall {
                doc: created.doc.clone(),
                by: agent.clone(),
                op: DeckOp::SectionPatch {
                    section_id: sec_b.id.clone(),
                    expected_rev: Some(1),
                    title: Some("Live KPIs v2".into()),
                    content: None,
                    col_span: None,
                    data_key: None,
                    summary: Some("agent edit".into()),
                },
            })
        );
        let DeckCallReply::Section(patched_a) = res_a.unwrap() else {
            panic!("expected patched section A");
        };
        let DeckCallReply::Section(patched_b) = res_b.unwrap() else {
            panic!("expected patched section B");
        };
        assert_eq!(patched_a.rev, 2);
        assert_eq!(patched_a.updated_by, "owner");
        assert_eq!(patched_b.rev, 2);
        assert!(patched_b.updated_by.starts_with("session:"));

        // Same-section CAS conflict on SectionPatch.
        let err = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: agent.clone(),
                op: DeckOp::SectionPatch {
                    section_id: sec_a.id.clone(),
                    expected_rev: Some(1),
                    title: None,
                    content: Some("Stale clobber attempt".into()),
                    col_span: None,
                    data_key: None,
                    summary: None,
                },
            })
            .await;
        match err {
            Err(kameo::error::SendError::HandlerError(DeckDocError::Conflict(c))) => {
                assert_eq!(c.section_id, sec_a.id);
                assert_eq!(c.expected_rev, 1);
                assert_eq!(c.actual_rev, 2);
                assert_eq!(c.updated_by, "owner");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        // Same-section CAS conflict on SectionArchive.
        let err = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: agent.clone(),
                op: DeckOp::SectionArchive {
                    section_id: sec_a.id.clone(),
                    archived: true,
                    expected_rev: Some(1),
                },
            })
            .await;
        assert!(matches!(
            err,
            Err(kameo::error::SendError::HandlerError(DeckDocError::Conflict(c)))
                if c.actual_rev == 2 && c.expected_rev == 1
        ));

        // Verify content stayed "Updated by owner" and history has 2 revisions.
        let DeckCallReply::History(hist) = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionHistory {
                    section_id: sec_a.id.clone(),
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected history");
        };
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].rev, 1);
        assert_eq!(hist[1].rev, 2);
        assert_eq!(hist[1].content, "Updated by owner");

        // Move section B before section A without modifying section maps.
        let DeckCallReply::Order(order) = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionMove {
                    section_id: sec_b.id.clone(),
                    to_index: 0,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected order");
        };
        assert_eq!(order, vec![sec_b.id.clone(), sec_a.id.clone()]);
    }

    #[tokio::test]
    async fn multi_peer_distinct_clients_and_restarts_converge_without_corruption() {
        // B1: ENGINE_CLIENT is non-zero and random_client_id produces distinct client IDs across
        // peers and process restarts.
        assert_ne!(*ENGINE_CLIENT, 0);
        let c1 = random_client_id();
        let c2 = random_client_id();
        let c3 = random_client_id();
        assert_ne!(c1, 0);
        assert_ne!(c2, 0);
        assert_ne!(c3, 0);
        assert_ne!(c1, c2);
        assert_ne!(c2, c3);
        assert_ne!(c1, c3);

        // Create initial shared state on peer A (client c1), replicate to peer B (client c2).
        let peer_a = init_peer_deck(c1, "Shared Deck");
        peer_add_section(
            &peer_a,
            "sec_00_base",
            SectionKind::Note,
            "Base",
            "v1",
            "owner",
            None,
        );
        let initial_state = peer_a
            .transact()
            .encode_state_as_update_v1(&StateVector::default());

        let peer_b = Doc::with_client_id(c2);
        peer_b
            .transact_mut()
            .apply_update(Update::decode_v1(&initial_state).unwrap())
            .unwrap();

        // Offline/concurrent writes on BOTH peers with distinct client IDs (no (client_id, clock) collision).
        peer_add_section(
            &peer_a,
            "sec_01_alpha",
            SectionKind::Note,
            "Peer A Note",
            "written on peer A",
            "owner",
            None,
        );
        peer_patch_section(
            &peer_a,
            "sec_00_base",
            "Base (A)",
            "edited on peer A",
            "owner",
        );

        peer_add_section(
            &peer_b,
            "sec_02_beta",
            SectionKind::JsonRender,
            "Peer B Chart",
            r#"{"root":"b"}"#,
            "member:peer_b",
            None,
        );

        // Bidirectional Yjs merge.
        sync_docs(&peer_a, &peer_b);

        // Simulate engine restart on peer A with a brand new client ID c3 loading persisted bytes.
        let persisted = peer_a
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let peer_a_restarted = Doc::with_client_id(c3);
        peer_a_restarted
            .transact_mut()
            .apply_update(Update::decode_v1(&persisted).unwrap())
            .unwrap();

        // Post-restart edits on peer_a_restarted concurrently with peer_b edits on a different section.
        peer_patch_section(
            &peer_a_restarted,
            "sec_01_alpha",
            "Peer A Note v2",
            "edited after restart",
            "owner",
        );
        peer_patch_section(
            &peer_b,
            "sec_02_beta",
            "Peer B Chart v2",
            r#"{"root":"b2"}"#,
            "member:peer_b",
        );
        sync_docs(&peer_a_restarted, &peer_b);

        // Resolve any concurrent alias collisions on both sides and re-sync.
        {
            let maps = Maps::of(&peer_a_restarted);
            let mut txn = peer_a_restarted.transact_mut();
            ensure_aliases(&maps, &mut txn);
        }
        sync_docs(&peer_a_restarted, &peer_b);

        let snap_a = read_snapshot("d1", "Shared Deck", &peer_a_restarted);
        let snap_b = read_snapshot("d1", "Shared Deck", &peer_b);
        assert_eq!(snap_a, snap_b, "peers must converge to identical snapshots");
        assert_eq!(snap_a.sections.len(), 3);
        let titles: BTreeMap<_, _> = snap_a
            .sections
            .iter()
            .map(|s| (s.id.as_str(), (s.title.as_str(), s.content.as_str(), s.rev)))
            .collect();
        assert_eq!(
            titles.get("sec_00_base"),
            Some(&("Base (A)", "edited on peer A", 2))
        );
        assert_eq!(
            titles.get("sec_01_alpha"),
            Some(&("Peer A Note v2", "edited after restart", 2))
        );
        assert_eq!(
            titles.get("sec_02_beta"),
            Some(&("Peer B Chart v2", r#"{"root":"b2"}"#, 2))
        );

        // Aliases must be unique after convergence.
        let unique_aliases: BTreeSet<&str> =
            snap_a.sections.iter().map(|s| s.alias.as_str()).collect();
        assert_eq!(unique_aliases.len(), 3);
    }

    #[tokio::test]
    async fn rbac_viewer_editor_revoked_create_by_and_non_serving_engine() {
        let env = test_env().await;
        let session = uuid::Uuid::new_v4();
        let agent = Principal::Agent { session };
        let guest_pub = "guest_pub_key_1";
        let guest = Principal::Member {
            pub_: guest_pub.into(),
            device: None,
        };

        // H6: Members cannot create Decks.
        let err = env
            .actor
            .ask(CreateDeck {
                title: Some("Forbidden".into()),
                description: None,
                by: guest.clone(),
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not_owner"),
            "member CreateDeck must fail with not_owner, got {err}"
        );

        // H6: CreateDeck records created_by from `by`.
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("RBAC Deck".into()),
                description: Some("Auth test".into()),
                by: agent.clone(),
            })
            .await
            .unwrap();
        {
            let doc_disk = env.store.load(&created.doc).unwrap().doc;
            let maps = Maps::of(&doc_disk);
            let txn = doc_disk.transact();
            assert_eq!(
                any_str(maps.meta.get(&txn, "created_by")),
                Some(format!("session:{session}"))
            );
        }

        // Seed one section as owner.
        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "S1".into(),
                    content: "hello".into(),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };

        // Non-member is refused on Read and writes.
        let err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: guest.clone(),
                op: DeckOp::Read,
            })
            .await;
        assert!(matches!(
            err,
            Err(kameo::error::SendError::HandlerError(DeckDocError::Forbidden(ref msg)))
                if msg == "not_member"
        ));

        // Grant Viewer membership via meta + reload on a fresh actor (or direct meta write before open).
        let mut meta = env.store.read_meta(&created.doc).unwrap();
        meta.members.push(Member {
            pub_: guest_pub.into(),
            name: "Guest".into(),
            color: "#3b82f6".into(),
            role: Role::Viewer,
            added_ms: now_ms(),
            via: Some("lnk_test".into()),
            last_seen_ms: 0,
        });
        env.store.write_meta(&meta).await.unwrap();
        // Restart actor over same dir so loaded meta reflects the Viewer member.
        env.actor.stop_gracefully().await.unwrap();
        env.actor.wait_for_shutdown().await;
        let env = test_env_with_cfg(env.dir, crate::DocumentsConfig::default()).await;

        // Viewer can Read and SectionHistory.
        assert!(matches!(
            env.actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: guest.clone(),
                    op: DeckOp::Read,
                })
                .await
                .unwrap(),
            DeckCallReply::Snapshot(_)
        ));
        assert!(matches!(
            env.actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: guest.clone(),
                    op: DeckOp::SectionHistory {
                        section_id: sec.id.clone(),
                    },
                })
                .await
                .unwrap(),
            DeckCallReply::History(_)
        ));

        // Viewer is refused on ALL write ops.
        for op in [
            DeckOp::SectionAdd {
                kind: SectionKind::Note,
                title: "Nope".into(),
                content: "x".into(),
                col_span: None,
                data_key: None,
                index: None,
            },
            DeckOp::SectionPatch {
                section_id: sec.id.clone(),
                expected_rev: Some(1),
                title: Some("Nope".into()),
                content: None,
                col_span: None,
                data_key: None,
                summary: None,
            },
            DeckOp::SectionMove {
                section_id: sec.id.clone(),
                to_index: 0,
            },
            DeckOp::SectionArchive {
                section_id: sec.id.clone(),
                archived: true,
                expected_rev: Some(1),
            },
            DeckOp::Focus {
                section_id: Some(sec.id.clone()),
            },
        ] {
            let err = env
                .actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: guest.clone(),
                    op,
                })
                .await;
            assert!(
                matches!(
                    err,
                    Err(kameo::error::SendError::HandlerError(DeckDocError::Forbidden(ref msg)))
                        if msg == "viewer_readonly"
                ),
                "viewer write should be Forbidden(viewer_readonly), got {err:?}"
            );
        }

        // Promote to Editor -> writes succeed, but UpdateScriptRef is STILL Helper-only!
        env.actor
            .ask(MemberSet {
                doc: created.doc.clone(),
                pub_: guest_pub.into(),
                role: Some(Role::Editor),
                name: None,
            })
            .await
            .unwrap();
        let DeckCallReply::Section(patched) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: guest.clone(),
                op: DeckOp::SectionPatch {
                    section_id: sec.id.clone(),
                    expected_rev: Some(1),
                    title: Some("Edited by member".into()),
                    content: None,
                    col_span: None,
                    data_key: None,
                    summary: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected patched section");
        };
        assert_eq!(patched.updated_by, format!("member:{guest_pub}"));

        for unauthorized in [guest.clone(), owner(), agent.clone()] {
            let err = env
                .actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: unauthorized,
                    op: DeckOp::UpdateScriptRef {
                        script_ref: DeckScriptRef {
                            active_rev: Some(1),
                            ..Default::default()
                        },
                    },
                })
                .await;
            assert!(matches!(
                err,
                Err(kameo::error::SendError::HandlerError(
                    DeckDocError::Forbidden(_)
                ))
            ));
        }
        // Helper succeeds on UpdateScriptRef.
        env.actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: Principal::Helper,
                op: DeckOp::UpdateScriptRef {
                    script_ref: DeckScriptRef {
                        active_rev: Some(3),
                        mode: Some("manual".into()),
                        ..Default::default()
                    },
                },
            })
            .await
            .unwrap();

        // Revoke member -> even Read is refused.
        env.actor
            .ask(MemberRevoke {
                doc: created.doc.clone(),
                pub_: guest_pub.into(),
            })
            .await
            .unwrap();
        let err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: guest.clone(),
                op: DeckOp::Read,
            })
            .await;
        assert!(matches!(
            err,
            Err(kameo::error::SendError::HandlerError(DeckDocError::Forbidden(ref msg)))
                if msg == "not_member"
        ));

        // Non-serving engine (another engine holds the hub lease): BOTH CreateDeck and DeckCall
        // must be refused with NotHub!
        env.actor
            .tell(SelfId(mesh::EngineId("me".into())))
            .await
            .unwrap();
        env.actor
            .tell(mesh::MeshEvent::Lease(mesh::LeaseState {
                holder: Some(mesh::EngineId("other-hub".into())),
                since_ms: 0,
                reason: "test".into(),
            }))
            .await
            .unwrap();

        let create_err = env
            .actor
            .ask(CreateDeck {
                title: Some("While Not Hub".into()),
                description: None,
                by: owner(),
            })
            .await
            .unwrap_err();
        assert!(
            create_err.to_string().contains("NotHub:other-hub"),
            "non-serving CreateDeck must return NotHub, got {create_err}"
        );

        let call_err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Read,
            })
            .await;
        match call_err {
            Err(kameo::error::SendError::HandlerError(DeckDocError::Forbidden(msg))) => {
                assert_eq!(
                    DocError::parse(&msg),
                    DocError::NotHub {
                        hub: Some(mesh::EngineId("other-hub".into()))
                    },
                    "non-serving DeckCall must return NotHub, got {msg}"
                );
            }
            other => panic!("expected NotHub error on non-serving DeckCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rank_exhaustion_collisions_and_non_finite_ranks_respace_cleanly() {
        let env = test_env().await;
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("Rank Stress".into()),
                description: None,
                by: owner(),
            })
            .await
            .unwrap();

        let DeckCallReply::Section(first) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "First".into(),
                    content: "0".into(),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        let DeckCallReply::Section(last) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "Last".into(),
                    content: "end".into(),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };

        let mut expected_order = vec![first.id.clone(), last.id.clone()];
        // Repeatedly insert at index 1 — 50 iterations drives 1000 / 2^50 (~8.8e-13) well past MIN_RANK_GAP (1e-6).
        for i in 0..50 {
            let DeckCallReply::Section(s) = env
                .actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: owner(),
                    op: DeckOp::SectionAdd {
                        kind: SectionKind::Note,
                        title: format!("Mid {i}"),
                        content: format!("{i}"),
                        col_span: None,
                        data_key: None,
                        index: Some(1),
                    },
                })
                .await
                .unwrap()
            else {
                panic!("expected section");
            };
            expected_order.insert(1, s.id);
        }

        let DeckCallReply::Snapshot(snap) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Read,
            })
            .await
            .unwrap()
        else {
            panic!("expected snapshot");
        };
        let actual_order: Vec<String> = snap.sections.iter().map(|s| s.id.clone()).collect();
        assert_eq!(actual_order, expected_order);
        for w in snap.sections.windows(2) {
            assert!(w[0].rank.is_finite() && w[1].rank.is_finite());
            assert!(
                w[0].rank < w[1].rank,
                "ranks must stay strictly increasing: {} vs {}",
                w[0].rank,
                w[1].rank
            );
        }

        // Now test colliding equal ranks AND non-finite (NaN / Inf) ranks injected into disk snapshot.
        let disk_doc = env.store.load(&created.doc).unwrap().doc;
        {
            let maps = Maps::of(&disk_doc);
            let mut txn = disk_doc.transact_mut();
            // Colliding equal ranks on two sections + NaN/Inf on two others.
            maps.order
                .insert(&mut txn, expected_order[0].clone(), any_float(1000.0));
            maps.order
                .insert(&mut txn, expected_order[1].clone(), any_float(1000.0));
            maps.order
                .insert(&mut txn, expected_order[2].clone(), any_float(f64::NAN));
            maps.order.insert(
                &mut txn,
                expected_order[3].clone(),
                any_float(f64::INFINITY),
            );
        }
        // Snapshot read never yields NaN/Inf.
        let corrupted_snap = read_snapshot(created.doc.as_str(), "Rank Stress", &disk_doc);
        assert!(corrupted_snap.sections.iter().all(|s| s.rank.is_finite()));

        // Persist corrupted doc, restart actor, and run SectionMove -> triggers clean re-spacing!
        env.store.snapshot(&created.doc, &disk_doc, None).unwrap();
        env.actor.stop_gracefully().await.unwrap();
        env.actor.wait_for_shutdown().await;
        let env = test_env_with_cfg(env.dir, crate::DocumentsConfig::default()).await;

        let DeckCallReply::Order(reordered) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionMove {
                    section_id: expected_order[1].clone(),
                    to_index: 0,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected order");
        };
        assert_eq!(reordered[0], expected_order[1]);

        let DeckCallReply::Snapshot(fixed) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Read,
            })
            .await
            .unwrap()
        else {
            panic!("expected snapshot");
        };
        for (i, sec) in fixed.sections.iter().enumerate() {
            assert!(sec.rank.is_finite());
            assert_eq!(sec.rank, ((i + 1) as f64) * RANK_STEP);
        }
    }

    #[tokio::test]
    async fn history_byte_budget_prevents_8mib_bloat_and_allows_compacting_oversized_deck() {
        // Part A: Rapidly patching a 200 KiB section 55 times stays well below max_doc_bytes (8 MiB).
        let env = test_env().await;
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("Heavy Deck".into()),
                description: None,
                by: owner(),
            })
            .await
            .unwrap();
        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "Big Section".into(),
                    content: "a".repeat(200 * 1024),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        let mut rev = sec.rev;
        for i in 0..55u32 {
            let body = format!("{i:04}:{}", "x".repeat(200 * 1024));
            let DeckCallReply::Section(updated) = env
                .actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: owner(),
                    op: DeckOp::SectionPatch {
                        section_id: sec.id.clone(),
                        expected_rev: Some(rev),
                        title: None,
                        content: Some(body),
                        col_span: None,
                        data_key: None,
                        summary: Some(format!("rev {i}")),
                    },
                })
                .await
                .unwrap()
            else {
                panic!("expected patched section");
            };
            rev = updated.rev;
        }
        let DeckCallReply::History(hist) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionHistory {
                    section_id: sec.id.clone(),
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected history");
        };
        assert!(!hist.is_empty() && hist.len() <= MAX_SECTION_HISTORY);
        for h in &hist {
            assert!(
                h.content.len() <= MAX_HISTORY_CONTENT_BYTES,
                "history excerpt {} exceeds {}",
                h.content.len(),
                MAX_HISTORY_CONTENT_BYTES
            );
        }
        let total_hist_bytes: usize = hist.iter().map(|h| h.content.len()).sum();
        assert!(
            total_hist_bytes <= MAX_SECTION_HISTORY_BYTES,
            "per-section history bytes {total_hist_bytes} exceeds {MAX_SECTION_HISTORY_BYTES}"
        );
        // Persisted snapshot must be far below 8 MiB.
        env.actor
            .ask(Snapshot {
                doc: created.doc.clone(),
                label: "after_patches".into(),
            })
            .await
            .unwrap();
        let persisted_doc = env.store.load(&created.doc).unwrap().doc;
        let state_bytes = persisted_doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
            .len();
        assert!(
            state_bytes < 1024 * 1024,
            "expected compact deck state < 1 MiB, got {state_bytes} bytes"
        );

        // Part B: Legacy oversized > 8 MiB Deck on disk can still be archived AND patched & compacted!
        for op_kind in ["archive", "patch"] {
            let env2 = test_env().await;
            let legacy = env2
                .actor
                .ask(CreateDeck {
                    title: Some(format!("Oversized {op_kind}")),
                    description: None,
                    by: owner(),
                })
                .await
                .unwrap();
            let DeckCallReply::Section(big_sec) = env2
                .actor
                .ask(DeckCall {
                    doc: legacy.doc.clone(),
                    by: owner(),
                    op: DeckOp::SectionAdd {
                        kind: SectionKind::Note,
                        title: "Bloated".into(),
                        content: "seed".into(),
                        col_span: None,
                        data_key: None,
                        index: None,
                    },
                })
                .await
                .unwrap()
            else {
                panic!("expected section");
            };
            env2.actor.stop_gracefully().await.unwrap();
            env2.actor.wait_for_shutdown().await;

            // Inject 40 x 220 KiB (~8.8 MiB) raw history entries directly into the stored Y.Doc.
            let disk_doc = env2.store.load(&legacy.doc).unwrap().doc;
            {
                let maps = Maps::of(&disk_doc);
                let mut txn = disk_doc.transact_mut();
                let Out::YMap(sec_map) = maps.sections.get(&txn, &big_sec.id).unwrap() else {
                    panic!("missing section");
                };
                let Out::YArray(arr) = sec_map.get(&txn, "history").unwrap() else {
                    panic!("missing history");
                };
                let big_payload = "Z".repeat(220 * 1024);
                for k in 2..=41i64 {
                    let mut m: HashMap<String, Any> = HashMap::new();
                    m.insert("rev".into(), any_int(k));
                    m.insert("at".into(), any_int(1_700_000_000_000 + k));
                    m.insert("by".into(), Any::String("owner".into()));
                    m.insert("summary".into(), Any::String("legacy".into()));
                    m.insert("title".into(), Any::String("Bloated".into()));
                    m.insert("content".into(), Any::String(big_payload.clone().into()));
                    m.insert("archived".into(), Any::Bool(false));
                    arr.push_back(&mut txn, Any::Map(m.into()));
                }
            }
            let pre_len = env2.store.snapshot(&legacy.doc, &disk_doc, None).unwrap();
            assert!(
                pre_len > 8 * 1024 * 1024,
                "legacy fixture must exceed 8 MiB, got {pre_len}"
            );

            // Reopen fresh actor over the >8 MiB Deck and verify archive / patch succeeds and compacts!
            let env2 = test_env_with_cfg(env2.dir, crate::DocumentsConfig::default()).await;
            let op = if op_kind == "archive" {
                DeckOp::SectionArchive {
                    section_id: big_sec.id.clone(),
                    archived: true,
                    expected_rev: Some(1),
                }
            } else {
                DeckOp::SectionPatch {
                    section_id: big_sec.id.clone(),
                    expected_rev: Some(1),
                    title: Some("Recovered".into()),
                    content: Some("compact now".into()),
                    col_span: None,
                    data_key: None,
                    summary: Some("recover".into()),
                }
            };
            env2.actor
                .ask(DeckCall {
                    doc: legacy.doc.clone(),
                    by: owner(),
                    op,
                })
                .await
                .expect("oversized legacy deck must still allow archive/patch compaction");

            let post_doc = env2.store.load(&legacy.doc).unwrap().doc;
            let post_len = post_doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default())
                .len();
            assert!(
                post_len < 1024 * 1024,
                "post-{op_kind} compacted deck must be < 1 MiB, got {post_len}"
            );
        }
    }

    #[test]
    fn alias_collisions_resolve_and_deduplicate_deterministically() {
        let c1 = random_client_id();
        let c2 = random_client_id();
        let peer_a = init_peer_deck(c1, "Alias Collision");
        let base = peer_a
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let peer_b = Doc::with_client_id(c2);
        peer_b
            .transact_mut()
            .apply_update(Update::decode_v1(&base).unwrap())
            .unwrap();

        // Both peers concurrently add their first section -> both assign alias "s1".
        peer_add_section(&peer_a, "sec_b", SectionKind::Note, "B", "b", "owner", None);
        peer_add_section(&peer_b, "sec_a", SectionKind::Note, "A", "a", "owner", None);
        sync_docs(&peer_a, &peer_b);

        // Before ensure_aliases runs, both sections map to "s1" in maps.aliases.
        // resolve_section_id must deterministically pick the lexicographically smallest ID ("sec_a") on BOTH peers.
        for peer in [&peer_a, &peer_b] {
            let maps = Maps::of(peer);
            let txn = peer.transact();
            assert_eq!(
                resolve_section_id(&maps, &txn, "s1").as_deref(),
                Some("sec_a")
            );
        }

        // Running ensure_aliases on either peer keeps "sec_a" on "s1" and reassigns "sec_b" to "s2".
        {
            let maps = Maps::of(&peer_a);
            let mut txn = peer_a.transact_mut();
            let reassigned = ensure_aliases(&maps, &mut txn);
            assert_eq!(reassigned, 1);
            assert_eq!(
                any_str(maps.aliases.get(&txn, "sec_a")).as_deref(),
                Some("s1")
            );
            assert_eq!(
                any_str(maps.aliases.get(&txn, "sec_b")).as_deref(),
                Some("s2")
            );
            assert_eq!(
                resolve_section_id(&maps, &txn, "s1").as_deref(),
                Some("sec_a")
            );
            assert_eq!(
                resolve_section_id(&maps, &txn, "s2").as_deref(),
                Some("sec_b")
            );
        }
    }

    #[tokio::test]
    async fn archived_section_operations_and_data_key_consistency() {
        let env = test_env().await;
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("Archive & DataKey".into()),
                description: None,
                by: owner(),
            })
            .await
            .unwrap();

        // Control chars in data_key rejected on SectionAdd.
        let err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::JsonRender,
                    title: "Bad Key".into(),
                    content: "{}".into(),
                    col_span: None,
                    data_key: Some("bad\nkey".into()),
                    index: None,
                },
            })
            .await;
        assert!(matches!(
            err,
            Err(kameo::error::SendError::HandlerError(
                DeckDocError::Invalid(_)
            ))
        ));

        // Whitespace-only data_key normalizes to None; padded data_key trims cleanly.
        let DeckCallReply::Section(sec_blank_dk) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::JsonRender,
                    title: "Blank DK".into(),
                    content: "{}".into(),
                    col_span: None,
                    data_key: Some("   ".into()),
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        assert_eq!(sec_blank_dk.data_key, None);

        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::JsonRender,
                    title: "Metrics".into(),
                    content: "{}".into(),
                    col_span: None,
                    data_key: Some("  kpis  ".into()),
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        assert_eq!(sec.data_key.as_deref(), Some("kpis"));

        // SectionPatch trims data_key and clears whitespace-only data_key.
        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionPatch {
                    section_id: sec.id.clone(),
                    expected_rev: Some(sec.rev),
                    title: None,
                    content: None,
                    col_span: None,
                    data_key: Some(Some("  metrics_v2  ".into())),
                    summary: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        assert_eq!(sec.data_key.as_deref(), Some("metrics_v2"));

        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionPatch {
                    section_id: sec.id.clone(),
                    expected_rev: Some(sec.rev),
                    title: None,
                    content: None,
                    col_span: None,
                    data_key: Some(Some("   ".into())),
                    summary: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        assert_eq!(sec.data_key, None);

        // Focus the section, then archive it -> focus must clear.
        env.actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Focus {
                    section_id: Some(sec.id.clone()),
                },
            })
            .await
            .unwrap();
        let DeckCallReply::Section(archived_sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionArchive {
                    section_id: sec.id.clone(),
                    archived: true,
                    expected_rev: Some(sec.rev),
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected archived section");
        };
        assert!(archived_sec.archived);

        let DeckCallReply::Snapshot(snap) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Read,
            })
            .await
            .unwrap()
        else {
            panic!("expected snapshot");
        };
        assert_eq!(
            snap.focus, None,
            "archiving focused section must clear focus"
        );
        assert_eq!(snap.archived_sections.len(), 1);

        // Archived sections reject Patch, Move, and Focus.
        for op in [
            DeckOp::SectionPatch {
                section_id: sec.id.clone(),
                expected_rev: Some(archived_sec.rev),
                title: Some("Should Fail".into()),
                content: None,
                col_span: None,
                data_key: None,
                summary: None,
            },
            DeckOp::SectionMove {
                section_id: sec.id.clone(),
                to_index: 0,
            },
            DeckOp::Focus {
                section_id: Some(sec.id.clone()),
            },
        ] {
            let err = env
                .actor
                .ask(DeckCall {
                    doc: created.doc.clone(),
                    by: owner(),
                    op,
                })
                .await;
            assert!(
                matches!(
                    err,
                    Err(kameo::error::SendError::HandlerError(
                        DeckDocError::Invalid(_)
                    ))
                ),
                "expected Invalid for archived section op, got {err:?}"
            );
        }

        // SectionHistory remains readable while archived.
        let DeckCallReply::History(hist) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionHistory {
                    section_id: sec.id.clone(),
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected history");
        };
        assert!(hist.last().unwrap().archived);

        // Unarchive restores Patch, Move, and Focus.
        let DeckCallReply::Section(restored) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionArchive {
                    section_id: sec.id.clone(),
                    archived: false,
                    expected_rev: Some(archived_sec.rev),
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected restored section");
        };
        assert!(!restored.archived);
        env.actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Focus {
                    section_id: Some(sec.id.clone()),
                },
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn raw_doc_update_invariants_and_saturating_rev() {
        let env = test_env().await;
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("Hardened Deck".into()),
                description: None,
                by: owner(),
            })
            .await
            .unwrap();
        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "S1".into(),
                    content: "safe".into(),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };

        let make_client_update_bytes =
            |mutate: &dyn Fn(&Maps, &mut TransactionMut<'_>)| -> Vec<u8> {
                let base = env
                    .store
                    .load(&created.doc)
                    .unwrap()
                    .doc
                    .transact()
                    .encode_state_as_update_v1(&StateVector::default());
                let d = Doc::with_client_id(random_client_id());
                d.transact_mut()
                    .apply_update(Update::decode_v1(&base).unwrap())
                    .unwrap();
                let sv = d.transact().state_vector();
                let maps = Maps::of(&d);
                {
                    let mut txn = d.transact_mut();
                    mutate(&maps, &mut txn);
                }
                d.transact().encode_diff_v1(&sv)
            };
        let make_client_update = |mutate: &dyn Fn(&Maps, &mut TransactionMut<'_>)| -> String {
            b64_update(&make_client_update_bytes(mutate))
        };

        // 1. Non-Helper raw doc_update cannot mutate script_refs.
        let bad_script_ref = make_client_update(&|maps, txn| {
            maps.script_refs.insert(txn, "active_rev", any_int(999));
        });
        let err = env
            .actor
            .ask(ApplyUpdate {
                doc: created.doc.clone(),
                update: bad_script_ref,
                by: owner(),
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("script_refs_readonly"),
            "expected script_refs_readonly, got {err}"
        );

        // 2. Raw doc_update cannot inject NaN or Infinity rank.
        let bad_rank = make_client_update(&|maps, txn| {
            maps.order.insert(txn, sec.id.clone(), any_float(f64::NAN));
        });
        let err = env
            .actor
            .ask(ApplyUpdate {
                doc: created.doc.clone(),
                update: bad_rank,
                by: owner(),
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid_deck_rank"),
            "expected invalid_deck_rank, got {err}"
        );

        // 3. Raw doc_update cannot set rev = i64::MAX (rev exhaustion attack).
        let bad_rev = make_client_update(&|maps, txn| {
            if let Some(Out::YMap(sm)) = maps.sections.get(txn, &sec.id) {
                sm.insert(txn, "rev", any_int(i64::MAX));
            }
        });
        let err = env
            .actor
            .ask(ApplyUpdate {
                doc: created.doc.clone(),
                update: bad_rev,
                by: owner(),
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid_deck_rev"),
            "expected invalid_deck_rev, got {err}"
        );

        // 4. Raw doc_update exceeding MAX_SECTION_CONTENT_BYTES is rejected (both by MAX_UPDATE_B64
        // on the wire and by validate_raw_update directly).
        let bad_content_bytes = make_client_update_bytes(&|maps, txn| {
            if let Some(Out::YMap(sm)) = maps.sections.get(txn, &sec.id) {
                sm.insert(
                    txn,
                    "content",
                    Any::String("X".repeat(MAX_SECTION_CONTENT_BYTES + 1).into()),
                );
            }
        });
        let loaded_doc = env.store.load(&created.doc).unwrap().doc;
        let err = validate_raw_update(&loaded_doc, &bad_content_bytes, &owner()).unwrap_err();
        assert!(
            err.contains("invalid_deck_content"),
            "expected invalid_deck_content, got {err}"
        );

        // 5. Legitimate raw update (editing note content within bounds) succeeds.
        let good_edit = make_client_update(&|maps, txn| {
            if let Some(Out::YMap(sm)) = maps.sections.get(txn, &sec.id) {
                sm.insert(txn, "content", Any::String("raw valid edit".into()));
            }
        });
        env.actor
            .ask(ApplyUpdate {
                doc: created.doc.clone(),
                update: good_edit,
                by: owner(),
            })
            .await
            .unwrap();

        // 6. Pre-existing rev = i64::MAX on disk saturates cleanly without overflow on SectionPatch.
        let disk_doc = env.store.load(&created.doc).unwrap().doc;
        {
            let maps = Maps::of(&disk_doc);
            let mut txn = disk_doc.transact_mut();
            let Out::YMap(sm) = maps.sections.get(&txn, &sec.id).unwrap() else {
                panic!("missing section");
            };
            sm.insert(&mut txn, "rev", any_int(i64::MAX));
        }
        env.store.snapshot(&created.doc, &disk_doc, None).unwrap();
        env.actor.stop_gracefully().await.unwrap();
        env.actor.wait_for_shutdown().await;
        let env = test_env_with_cfg(env.dir, crate::DocumentsConfig::default()).await;

        let DeckCallReply::Section(patched) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionPatch {
                    section_id: sec.id.clone(),
                    expected_rev: Some(MAX_SECTION_REV),
                    title: Some("Saturated OK".into()),
                    content: None,
                    col_span: None,
                    data_key: None,
                    summary: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected patched section");
        };
        assert_eq!(patched.rev, MAX_SECTION_REV);
    }

    #[tokio::test]
    async fn unloaded_restore_helper_restart_export_and_deletion_signal() {
        let env = test_env().await;
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("Lifecycle Deck".into()),
                description: Some("Daily status".into()),
                by: owner(),
            })
            .await
            .unwrap();
        env.actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "Summary".into(),
                    content: "All systems green.".into(),
                    col_span: Some(12),
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap();
        env.actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::JsonRender,
                    title: "Latency".into(),
                    content: "{}".into(),
                    col_span: Some(6),
                    data_key: Some("latency_ms".into()),
                    index: None,
                },
            })
            .await
            .unwrap();

        // Take a snapshot, verify HelperEvent::Restarted ignores open Deck, then restart actor so Deck is UNLOADED.
        env.actor
            .ask(Snapshot {
                doc: created.doc.clone(),
                label: "snap1".into(),
            })
            .await
            .unwrap();
        env.actor.tell(HelperEvent::Restarted).await.unwrap();
        // Sync barrier via Read.
        let DeckCallReply::Snapshot(snap) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Read,
            })
            .await
            .unwrap()
        else {
            panic!("expected snapshot");
        };
        assert_eq!(
            env.helper_calls.load(Ordering::SeqCst),
            0,
            "Deck must never invoke the BlockNote helper (including on HelperEvent::Restarted)"
        );

        // Verify export_snapshot_markdown renders title, description, note content, and non-note data_key.
        let md = export_snapshot_markdown(&snap);
        assert!(md.contains("# Lifecycle Deck"));
        assert!(md.contains("Daily status"));
        assert!(md.contains("All systems green."));
        assert!(md.contains("_data_key: latency_ms_"));

        let snaps = env.store.snapshots(&created.doc).unwrap();
        let snap_name = snaps[0].name.clone();
        env.actor.stop_gracefully().await.unwrap();
        env.actor.wait_for_shutdown().await;

        // Fresh actor: Deck is NOT loaded in self.open.
        let env = test_env_with_cfg(env.dir, crate::DocumentsConfig::default()).await;
        let restore_err = env
            .actor
            .ask(Restore {
                doc: created.doc.clone(),
                snapshot: snap_name,
            })
            .await
            .unwrap_err();
        assert!(
            restore_err.to_string().contains("not available for Decks"),
            "unloaded Restore on Deck must be refused immediately, got {restore_err}"
        );
        assert_eq!(
            env.helper_calls.load(Ordering::SeqCst),
            0,
            "unloaded Restore on Deck must not touch BlockNote helper"
        );

        // Subscribe to DocumentChanged on runtime bus AND open a DocEvent subscription, then DeleteNote.
        let (bus_tx, mut bus_rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = ChangeSink::spawn(ChangeSink(bus_tx));
        env.rt.subscribe::<ChangeSink, DocumentChanged>(&sink).await;
        let mut sub_rx = env
            .actor
            .ask(Subscribe {
                doc: created.doc.clone(),
                by: owner(),
            })
            .await
            .unwrap();
        env.actor
            .ask(DeleteNote {
                doc: created.doc.clone(),
            })
            .await
            .unwrap();

        // Subscriber receives Closed { reason: "deleted" }.
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), sub_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(closed, DocEvent::Closed { ref reason } if reason == "deleted"),
            "expected DocEvent::Closed(deleted), got {closed:?}"
        );

        // Runtime bus receives DocumentChanged { kind: DocKind::Deck, deleted: true }.
        let mut saw_deleted = false;
        while let Ok(Some(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), bus_rx.recv()).await
        {
            if ev.doc == created.doc && ev.kind == DocKind::Deck && ev.deleted {
                saw_deleted = true;
                break;
            }
        }
        assert!(
            saw_deleted,
            "DeleteNote on Deck must publish DocumentChanged {{ kind: Deck, deleted: true }}"
        );

        // Subsequent DeckCall returns NotFound.
        let err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::Read,
            })
            .await;
        assert!(matches!(
            err,
            Err(kameo::error::SendError::HandlerError(
                DeckDocError::NotFound(_)
            ))
        ));
        assert_eq!(env.helper_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn openui_section_kind_default_span_cap_and_raw_validation() {
        assert_eq!(SectionKind::parse("openui"), Some(SectionKind::Openui));
        assert_eq!(SectionKind::Openui.as_str(), "openui");
        assert_eq!(SectionKind::Openui.default_col_span(), 6);
        assert_eq!(
            SectionKind::Openui.max_content_bytes(),
            MAX_OPENUI_CONTENT_BYTES
        );
        assert_eq!(
            serde_json::to_value(SectionKind::Openui).unwrap(),
            json!("openui")
        );
        assert_eq!(
            serde_json::from_value::<SectionKind>(json!("openui")).unwrap(),
            SectionKind::Openui
        );

        let dir = tempfile::tempdir().unwrap();
        let env = test_env_with_cfg(dir, crate::DocumentsConfig::default()).await;
        let created = env
            .actor
            .ask(CreateDeck {
                title: Some("OpenUI Deck".into()),
                description: None,
                by: owner(),
            })
            .await
            .unwrap();

        // 1. Adding an openui section defaults col_span to 6 and accepts <= 64 KiB.
        let DeckCallReply::Section(sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Openui,
                    title: "Controls".into(),
                    content: "root = Card([TextContent(\"hi\")])".into(),
                    col_span: None,
                    data_key: Some("metrics".into()),
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected section");
        };
        assert_eq!(sec.kind, SectionKind::Openui);
        assert_eq!(sec.col_span, 6);
        assert_eq!(sec.data_key.as_deref(), Some("metrics"));

        // 2. Adding an openui section > 64 KiB is rejected, while a Note > 64 KiB (<= 256 KiB) succeeds.
        let over_64k = "x".repeat(MAX_OPENUI_CONTENT_BYTES + 1);
        let add_err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Openui,
                    title: "Too Big".into(),
                    content: over_64k.clone(),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await;
        assert!(matches!(
            add_err,
            Err(kameo::error::SendError::HandlerError(DeckDocError::Invalid(ref m)))
            if m.contains(&MAX_OPENUI_CONTENT_BYTES.to_string())
        ));

        let DeckCallReply::Section(note_sec) = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionAdd {
                    kind: SectionKind::Note,
                    title: "Large Note".into(),
                    content: over_64k.clone(),
                    col_span: None,
                    data_key: None,
                    index: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("expected note section");
        };
        assert_eq!(note_sec.col_span, 12);

        // 3. Patching the openui section over 64 KiB is rejected.
        let patch_err = env
            .actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner(),
                op: DeckOp::SectionPatch {
                    section_id: sec.id.clone(),
                    expected_rev: Some(sec.rev),
                    title: None,
                    content: Some(over_64k.clone()),
                    col_span: None,
                    data_key: None,
                    summary: None,
                },
            })
            .await;
        assert!(matches!(
            patch_err,
            Err(kameo::error::SendError::HandlerError(DeckDocError::Invalid(ref m)))
            if m.contains(&MAX_OPENUI_CONTENT_BYTES.to_string())
        ));

        // 4. Raw Yjs update validation:
        // Build a local doc mirroring the current Deck state to craft raw updates.
        let state_doc = Doc::new();
        {
            let maps = Maps::of(&state_doc);
            let mut txn = state_doc.transact_mut();
            let sm = maps.sections.insert(&mut txn, "s_note", MapPrelim::default());
            sm.insert(&mut txn, "kind", Any::String("note".into()));
            sm.insert(&mut txn, "content", Any::String(over_64k.clone().into()));
            sm.insert(&mut txn, "rev", any_int(1));
        }

        // 4a. Valid openui raw update (<= 64 KiB) passes.
        let peer_ok = Doc::with_client_id(9001);
        {
            let base = state_doc.transact().encode_state_as_update_v1(&StateVector::default());
            peer_ok.transact_mut().apply_update(Update::decode_v1(&base).unwrap()).unwrap();
            let sv = peer_ok.transact().state_vector();
            {
                let maps = Maps::of(&peer_ok);
                let mut txn = peer_ok.transact_mut();
                let sm = maps.sections.insert(&mut txn, "s_openui_ok", MapPrelim::default());
                sm.insert(&mut txn, "kind", Any::String("openui".into()));
                sm.insert(&mut txn, "content", Any::String("x".repeat(MAX_OPENUI_CONTENT_BYTES).into()));
                sm.insert(&mut txn, "rev", any_int(1));
            }
            let diff = peer_ok.transact().encode_diff_v1(&sv);
            assert!(validate_raw_update(&state_doc, &diff, &owner()).is_ok());
        }

        // 4b. Oversized openui raw update (> 64 KiB) is rejected.
        let peer_big = Doc::with_client_id(9002);
        {
            let base = state_doc.transact().encode_state_as_update_v1(&StateVector::default());
            peer_big.transact_mut().apply_update(Update::decode_v1(&base).unwrap()).unwrap();
            let sv = peer_big.transact().state_vector();
            {
                let maps = Maps::of(&peer_big);
                let mut txn = peer_big.transact_mut();
                let sm = maps.sections.insert(&mut txn, "s_openui_big", MapPrelim::default());
                sm.insert(&mut txn, "kind", Any::String("openui".into()));
                sm.insert(&mut txn, "content", Any::String(over_64k.clone().into()));
                sm.insert(&mut txn, "rev", any_int(1));
            }
            let diff = peer_big.transact().encode_diff_v1(&sv);
            let err = validate_raw_update(&state_doc, &diff, &owner()).unwrap_err();
            assert!(err.contains("invalid_deck_content"), "got {err}");
        }

        // 4c. Switching existing >64 KiB note section to kind="openui" is rejected.
        let peer_switch = Doc::with_client_id(9003);
        {
            let base = state_doc.transact().encode_state_as_update_v1(&StateVector::default());
            peer_switch.transact_mut().apply_update(Update::decode_v1(&base).unwrap()).unwrap();
            let sv = peer_switch.transact().state_vector();
            {
                let maps = Maps::of(&peer_switch);
                let mut txn = peer_switch.transact_mut();
                if let Some(Out::YMap(sm)) = maps.sections.get(&txn, "s_note") {
                    sm.insert(&mut txn, "kind", Any::String("openui".into()));
                }
            }
            let diff = peer_switch.transact().encode_diff_v1(&sv);
            let err = validate_raw_update(&state_doc, &diff, &owner()).unwrap_err();
            assert!(err.contains("invalid_deck_content"), "got {err}");
        }

        // 4d. Unsupported section kind (e.g. "mermaid") is rejected.
        let peer_bad_kind = Doc::with_client_id(9004);
        {
            let base = state_doc.transact().encode_state_as_update_v1(&StateVector::default());
            peer_bad_kind.transact_mut().apply_update(Update::decode_v1(&base).unwrap()).unwrap();
            let sv = peer_bad_kind.transact().state_vector();
            {
                let maps = Maps::of(&peer_bad_kind);
                let mut txn = peer_bad_kind.transact_mut();
                let sm = maps.sections.insert(&mut txn, "s_bad", MapPrelim::default());
                sm.insert(&mut txn, "kind", Any::String("mermaid".into()));
                sm.insert(&mut txn, "content", Any::String("graph TD; A-->B".into()));
                sm.insert(&mut txn, "rev", any_int(1));
            }
            let diff = peer_bad_kind.transact().encode_diff_v1(&sv);
            let err = validate_raw_update(&state_doc, &diff, &owner()).unwrap_err();
            assert!(err.contains("invalid_deck_kind"), "got {err}");
        }
    }
}
