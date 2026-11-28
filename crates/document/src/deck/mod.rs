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

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
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
use crate::meta::{AgentStyle, Binding, DocId, DocKind, DocMeta, DocumentChanged, Principal};

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

/// Deterministic client ID used for engine-side Yjs writes (distinct from whiteboard's client ID).
static ENGINE_CLIENT: LazyLock<u64> = LazyLock::new(|| {
    let h = blake_client_id(b"theuth-deck-engine-v1");
    h.max(1)
});

fn blake_client_id(seed: &[u8]) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(seed);
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(buf) & ((1u64 << 53) - 1)
}

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
}

impl SectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JsonRender => "json_render",
            Self::Note => "note",
            Self::Html => "html",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "json_render" => Some(Self::JsonRender),
            "note" => Some(Self::Note),
            "html" => Some(Self::Html),
            _ => None,
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
            Self::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<String> for DeckDocError {
    fn from(s: String) -> Self {
        Self::Internal(s)
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
        Out::Any(Any::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

fn any_u64(out: Option<Out>) -> Option<u64> {
    any_i64(out).and_then(|n| u64::try_from(n).ok())
}

fn any_f64(out: Option<Out>) -> Option<f64> {
    match out? {
        Out::Any(Any::Number(n)) => n.as_f64().or_else(|| n.as_i64().map(|i| i as f64)),
        _ => None,
    }
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
            Any::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
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

fn ensure_aliases(maps: &Maps, txn: &mut TransactionMut<'_>) -> usize {
    let mut max_n: u64 = 0;
    let mut existing: HashMap<String, String> = HashMap::new();
    for (k, v) in maps.aliases.iter(txn) {
        if let Out::Any(Any::String(alias)) = v {
            let s = alias.to_string();
            if let Some(num) = s.strip_prefix('s').and_then(|r| r.parse::<u64>().ok()) {
                max_n = max_n.max(num);
            }
            existing.insert(k.to_owned(), s);
        }
    }
    let mut missing: Vec<String> = maps
        .sections
        .keys(txn)
        .filter(|id| !existing.contains_key(*id))
        .map(str::to_owned)
        .collect();
    missing.sort();
    let mut added = 0;
    for id in missing {
        max_n += 1;
        maps.aliases
            .insert(txn, id, Any::String(format!("s{max_n}").into()));
        added += 1;
    }
    added
}

fn resolve_section_id<T: ReadTxn>(maps: &Maps, txn: &T, id_or_alias: &str) -> Option<String> {
    if maps.sections.contains_key(txn, id_or_alias) {
        return Some(id_or_alias.to_owned());
    }
    for (id, v) in maps.aliases.iter(txn) {
        if let Out::Any(Any::String(alias)) = v
            && alias.as_ref() == id_or_alias
            && maps.sections.contains_key(txn, id)
        {
            return Some(id.to_owned());
        }
    }
    None
}

fn read_section<T: ReadTxn>(
    maps: &Maps,
    txn: &T,
    id: &str,
    sec_map: &MapRef,
) -> Option<DeckSection> {
    let kind_str = any_str(sec_map.get(txn, "kind")).unwrap_or_else(|| "note".into());
    let kind = SectionKind::parse(&kind_str)?;
    let alias = any_str(maps.aliases.get(txn, id)).unwrap_or_else(|| id.to_owned());
    let rank = any_f64(maps.order.get(txn, id)).unwrap_or(0.0);
    Some(DeckSection {
        id: id.to_owned(),
        alias,
        kind,
        title: any_str(sec_map.get(txn, "title")).unwrap_or_default(),
        content: any_str(sec_map.get(txn, "content")).unwrap_or_default(),
        col_span: any_u64(sec_map.get(txn, "col_span"))
            .unwrap_or(6)
            .clamp(1, 12) as u32,
        data_key: any_str(sec_map.get(txn, "data_key")).filter(|s| !s.is_empty()),
        archived: any_bool(sec_map.get(txn, "archived")).unwrap_or(false),
        rev: any_u64(sec_map.get(txn, "rev")).unwrap_or(1),
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
                rev: map_i64(&any, "rev").and_then(|n| u64::try_from(n).ok())?,
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

fn push_history(sec_map: &MapRef, txn: &mut TransactionMut<'_>, entry: &SectionRevision) {
    let arr = match sec_map.get(txn, "history") {
        Some(Out::YArray(a)) => a,
        _ => sec_map.insert(txn, "history", ArrayPrelim::default()),
    };
    let mut m: HashMap<String, Any> = HashMap::new();
    m.insert("rev".into(), any_int(entry.rev as i64));
    m.insert("at".into(), any_int(entry.at));
    m.insert("by".into(), Any::String(entry.by.clone().into()));
    m.insert("summary".into(), Any::String(entry.summary.clone().into()));
    m.insert("title".into(), Any::String(entry.title.clone().into()));
    m.insert("content".into(), Any::String(entry.content.clone().into()));
    m.insert("archived".into(), Any::Bool(entry.archived));
    arr.push_back(txn, Any::Map(m.into()));
    let len = arr.len(txn) as usize;
    if len > MAX_SECTION_HISTORY {
        arr.remove_range(txn, 0, (len - MAX_SECTION_HISTORY) as u32);
    }
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

/// Compute a fractional rank for inserting/moving a section to `index` among `ordered` (excluding `skip_id`).
fn compute_rank(ordered: &[DeckSection], index: Option<usize>, skip_id: Option<&str>) -> f64 {
    let ranks: Vec<f64> = ordered
        .iter()
        .filter(|s| skip_id != Some(s.id.as_str()))
        .map(|s| s.rank)
        .collect();
    if ranks.is_empty() {
        return 1000.0;
    }
    let idx = index.unwrap_or(ranks.len()).min(ranks.len());
    if idx == 0 {
        ranks[0] - 1000.0
    } else if idx >= ranks.len() {
        ranks[ranks.len() - 1] + 1000.0
    } else {
        let lo = ranks[idx - 1];
        let hi = ranks[idx];
        if (hi - lo).abs() > 1e-6 {
            (lo + hi) * 0.5
        } else {
            // Rare collision fallback: step slightly; caller may re-normalize if needed.
            lo + 0.0001
        }
    }
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

/// Reply to [`CreateDeck`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeckCreated {
    pub doc: DocId,
    pub title: String,
}

/// Operations executed on a `DocKind::Deck` document via [`DeckCall`].
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
        let o = self.deck_open(doc)?;
        let (state, sv) = {
            let t = o.doc.transact();
            (
                t.encode_state_as_update_v1(&StateVector::default()),
                t.state_vector(),
            )
        };
        let scratch = Doc::with_client_id(*ENGINE_CLIENT);
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
            (r, mutated || added_before > 0 || added_after > 0)
        };
        if dirty {
            let update = scratch.transact().encode_diff_v1(&sv);
            self.apply(doc, update, by, false, Vec::new())
                .await
                .map_err(DeckDocError::Internal)?;
        }
        Ok(result)
    }

    async fn deck_create_here(
        &mut self,
        title: String,
        description: String,
    ) -> Result<DeckCreated, String> {
        let binding = Binding::Deck;
        let id = DocId::for_binding(&binding);
        let now = now_ms();
        let home = self.resolve_home(&Homing::Here, DocKind::Deck);
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
        self.deck_write(&id, Principal::Helper, |maps, txn, _who, now| {
            maps.meta.insert(txn, "v", any_int(i64::from(SCHEMA)));
            maps.meta.insert(txn, "created", any_int(now));
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
}

impl Message<CreateDeck> for DocumentActor {
    type Reply = DelegatedReply<Result<DeckCreated, String>>;

    async fn handle(&mut self, m: CreateDeck, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let title = m
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Untitled Deck".to_owned());
        let description = m.description.unwrap_or_default();
        tx.send(self.deck_create_here(title, description).await);
        reply
    }
}

impl Message<DeckCall> for DocumentActor {
    type Reply = Result<DeckCallReply, DeckDocError>;

    async fn handle(&mut self, m: DeckCall, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
                if content.len() > MAX_SECTION_CONTENT_BYTES {
                    return Err(DeckDocError::Invalid(format!(
                        "section content exceeds {MAX_SECTION_CONTENT_BYTES} bytes"
                    )));
                }
                let title = title.trim().to_owned();
                if title.is_empty() {
                    return Err(DeckDocError::Invalid(
                        "section title cannot be empty".into(),
                    ));
                }
                let span = col_span
                    .unwrap_or(match kind {
                        SectionKind::JsonRender => 6,
                        SectionKind::Html => 6,
                        SectionKind::Note => 12,
                    })
                    .clamp(1, 12);

                let sec = self
                    .deck_write(&m.doc, m.by, |maps, txn, who, now| {
                        // Compute rank among active sections.
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
                        let rank = compute_rank(&active, index, None);
                        let id = new_section_id();
                        let sec_map = maps.sections.insert(txn, id.clone(), MapPrelim::default());
                        sec_map.insert(txn, "id", Any::String(id.clone().into()));
                        sec_map.insert(txn, "kind", Any::String(kind.as_str().into()));
                        sec_map.insert(txn, "title", Any::String(title.clone().into()));
                        sec_map.insert(txn, "content", Any::String(content.clone().into()));
                        sec_map.insert(txn, "col_span", any_int(i64::from(span)));
                        if let Some(dk) = data_key.as_deref().filter(|s| !s.is_empty()) {
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
                        maps.order.insert(txn, id.clone(), any_float(rank));
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
                        let mut changed = Vec::new();
                        if let Some(t) = title {
                            let t = t.trim().to_owned();
                            if t.is_empty() {
                                return Err(DeckDocError::Invalid(
                                    "section title cannot be empty".into(),
                                ));
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
                        if let Some(dk_opt) = data_key {
                            let normalized = dk_opt.filter(|s| !s.trim().is_empty());
                            if normalized != cur.data_key {
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
                        }
                        if changed.is_empty() {
                            return Ok((cur, false));
                        }
                        let next_rev = cur.rev + 1;
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
                        let new_rank = compute_rank(&active, Some(to_index), Some(&id));
                        // Local edit: only updates the moved section's key in `order`.
                        maps.order.insert(txn, id.clone(), any_float(new_rank));
                        for s in &mut active {
                            if s.id == id {
                                s.rank = new_rank;
                            }
                        }
                        active.sort_by(|a, b| {
                            a.rank.total_cmp(&b.rank).then_with(|| a.id.cmp(&b.id))
                        });
                        let ids = active.into_iter().map(|s| s.id).collect();
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
                        let next_rev = cur.rev + 1;
                        sec_map.insert(txn, "archived", Any::Bool(archived));
                        sec_map.insert(txn, "rev", any_int(next_rev as i64));
                        sec_map.insert(txn, "updated_at", any_int(now));
                        sec_map.insert(txn, "updated_by", Any::String(who.into()));
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
                // Only metadata pointers — never Python source or authorization flags.
                self.deck_write(&m.doc, Principal::Helper, |maps, txn, _who, now| {
                    match script_ref.active_rev {
                        Some(r) => {
                            maps.script_refs
                                .insert(txn, "active_rev", any_int(r as i64));
                        }
                        None => {
                            maps.script_refs.remove(txn, "active_rev");
                        }
                    }
                    match script_ref.pending_rev {
                        Some(r) => {
                            maps.script_refs
                                .insert(txn, "pending_rev", any_int(r as i64));
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
    use kameo::actor::Spawn;
    use runtime::Runtime;

    async fn test_actor() -> (tempfile::TempDir, kameo::actor::ActorRef<DocumentActor>) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let cfg = crate::DocumentsConfig::default();
        let store = crate::store::Store::open(dir.path()).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = crate::helper::Helper::new(tx, rt.shutdown.clone());
        let actor = DocumentActor::spawn(DocumentActor::new(store, helper, cfg, rt));
        (dir, actor)
    }

    #[tokio::test]
    async fn concurrent_section_edits_and_cas_conflicts() {
        let (_dir, actor) = test_actor().await;
        let created = actor
            .ask(CreateDeck {
                title: Some("Ops Deck".into()),
                description: Some("Team operations".into()),
                by: Principal::Owner { device: None },
            })
            .await
            .unwrap();

        let owner = Principal::Owner { device: None };
        let agent = Principal::Agent {
            session: uuid::Uuid::new_v4(),
        };

        // Add two independent sections.
        let DeckCallReply::Section(sec_a) = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner.clone(),
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
                by: owner.clone(),
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

        // Same-section CAS conflict: stale expected_rev=1 must be rejected without mutating sec_a!
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

        // Verify content stayed "Updated by owner" and history has 2 revisions.
        let DeckCallReply::History(hist) = actor
            .ask(DeckCall {
                doc: created.doc.clone(),
                by: owner.clone(),
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
                by: owner.clone(),
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
}
