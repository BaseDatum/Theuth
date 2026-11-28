//! The [`DocumentActor`] (design 59 §3.1): one `yrs::Doc` per open document,
//! loaded lazily from the store, persisted as snapshot + log, unloaded when
//! idle; every accepted update fans out to the document's subscribers (§6.2)
//! and schedules the derived-file export (§4.3); imports (§4.4) go through
//! the helper and come back as ordinary updates stamped with their origin.
//!
//! Handlers do local I/O on the mailbox (sqlite, small files). Anything that
//! waits on the helper — export, import, restore — runs in a spawned task
//! and returns as a message.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use kameo::actor::{ActorRef, WeakActorRef};
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yrs::sync::{Awareness, AwarenessUpdate};
use yrs::types::xml::XmlOut;
use yrs::types::{Change, DeepObservable, Event};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, StateVector, Transact, TransactionMut, Update, Xml};

use crate::helper::{self, Edit as BlockEdit, EditError, Helper, HelperEvent, ImportMode};
use crate::hub::{DocError, Route};
use crate::meta::{
    AgentStyle, Binding, DocEvent, DocId, DocKind, DocMeta, DocOpen, DocSummary, DocumentChanged,
    Principal, SnapshotInfo, Who,
};
use crate::relay::{Client, Cmd, CreateReq};
use crate::store::{self, Store, now_ms, sha256, write_atomic};
use crate::{DocumentsConfig, Exported, FRAGMENT};

/// Snapshot when the log passes this (§4.1).
const LOG_MAX_BYTES: usize = 1024 * 1024;
/// … or this many records.
const LOG_MAX_COUNT: usize = 1000;
/// Export debounce hard cap (§4.3).
const EXPORT_CAP: Duration = Duration::from_secs(3);
/// `doc_open` state chunk: 36 KiB raw → 48 KiB base64 (§4.7).
const CHUNK: usize = 36 * 1024;
/// Events a slow subscriber may leave unread before it is dropped.
const SUB_QUEUE: usize = 256;
/// The agent's awareness state outlives its update by this (§6.4: the reveal runs ≤ 1.5 s).
const AGENT_PRESENCE: Duration = Duration::from_secs(2);
/// Awareness states relayed per participant per second (§6.2); the excess is dropped, cursors are best-effort.
const AWARENESS_RATE: u8 = 4;
/// `.trash/` is swept this often (§7.3) …
const TRASH_SWEEP: Duration = Duration::from_secs(3600);
/// … of directories deleted longer ago than this.
const TRASH_KEEP: Duration = Duration::from_secs(30 * 24 * 3600);
/// A replied import waits this long for its update before it is forgotten (none comes when nothing changed).
const IMPORT_GRACE: Duration = Duration::from_secs(10);
/// 184 §8.1: the docs gate is polled this often besides every mesh event.
const GATE_POLL: Duration = Duration::from_secs(30);

/// One loaded document.
pub(crate) struct Open {
    pub(crate) doc: yrs::Doc,
    pub(crate) meta: DocMeta,
    awareness: Awareness,
    log_count: usize,
    log_bytes: usize,
    /// Upper bound on the encoded state: snapshot + log bytes (§4.7 limit).
    pub(crate) bytes: usize,
    idle_since: Instant,
    pub(crate) export_due: Option<Instant>,
    export_first: Option<Instant>,
    pub(crate) exporting: bool,
    /// The owner hears [`Exported`] once this passes with the file settled (§4.3 as built: `export_quiet_secs`
    /// after the last update) — or at once when the document has no subscriber, unloads, or the harness stops.
    notify_due: Option<Instant>,
    /// The last agent edit's inverse (§5.5 "Undo last agent edit"); one level, in memory.
    last_agent_edit: Option<LastAgentEdit>,
    /// Bumped per agent presence; a delayed clear only fires for its own generation.
    agent_gen: u64,
    /// Per connection: start of the current one-second window and the states relayed in it (§6.2 cap).
    awareness_rate: HashMap<Option<u64>, RateSlot>,
    /// Block ids the last transaction wrote, filled by the deep observer on the fragment (§11 authorship).
    touched: Arc<Mutex<Vec<String>>>,
    /// 171 §9.6: block id → the agent session that last wrote it (`agents.json`; any other writer clears it).
    pub(crate) agents: std::collections::BTreeMap<String, uuid::Uuid>,
    /// `meta.authors` or `agents` changed since they were last written.
    pub(crate) authors_dirty: bool,
    /// 184: the fence changed since `meta.json` was last written (`Tick` writes it).
    pub(crate) meta_dirty: bool,
    /// 184 §4.4: the other engine also serving this document during a partition.
    pub(crate) dual: Option<mesh::EngineId>,
}

/// One participant's awareness budget: window start, states relayed since.
#[derive(Clone, Copy, Debug)]
struct RateSlot {
    since: Instant,
    sent: u8,
}

/// Whether one more awareness state fits into `slot`'s window at `now` (§6.2: ≤ [`AWARENESS_RATE`] per second).
fn admit_awareness(slot: &mut RateSlot, now: Instant) -> bool {
    if now.duration_since(slot.since) >= Duration::from_secs(1) {
        *slot = RateSlot {
            since: now,
            sent: 0,
        };
    }
    if slot.sent >= AWARENESS_RATE {
        return false;
    }
    slot.sent += 1;
    true
}

/// Block ids `e` wrote (§11 authorship): the nearest enclosing `blockContainer` of the changed
/// node, and every `blockContainer` a child insert added (a new block's own id).
fn touched_blocks(txn: &TransactionMut, e: &Event, out: &mut Vec<String>) {
    let push = |out: &mut Vec<String>, id: String| {
        if !out.contains(&id) {
            out.push(id);
        }
    };
    let target = match e {
        Event::XmlText(t) => Some(XmlOut::Text(t.target().clone())),
        Event::XmlFragment(f) => {
            for c in f.delta(txn) {
                if let Change::Added(items) = c {
                    for item in items {
                        if let Ok(node) = XmlOut::try_from(item.clone())
                            && let Some(id) = container_id(txn, &node)
                        {
                            push(out, id);
                        }
                    }
                }
            }
            Some(f.target().clone())
        }
        _ => None,
    };
    let mut node = target;
    while let Some(n) = node {
        if let Some(id) = container_id(txn, &n) {
            push(out, id);
            break;
        }
        node = match &n {
            XmlOut::Element(e) => e.parent(),
            XmlOut::Text(t) => t.parent(),
            XmlOut::Fragment(_) => None,
        };
    }
}

/// `id` of `node` when it is a BlockNote `blockContainer`.
fn container_id(txn: &TransactionMut, node: &XmlOut) -> Option<String> {
    let XmlOut::Element(el) = node else {
        return None;
    };
    if el.try_tag().is_none_or(|t| &**t != "blockContainer") {
        return None;
    }
    el.get_attribute(txn, "id").map(|v| v.to_string(txn))
}

/// What "Undo last agent edit" reverts.
struct LastAgentEdit {
    session: uuid::Uuid,
    undo: Vec<BlockEdit>,
}

/// An edit the helper is running; keyed by the helper request id. The update
/// and the reply arrive on two tasks, so the entry lives until both did.
struct PendingEdit {
    doc: DocId,
    by: Principal,
    reply: Option<ReplySender<Result<EditOutcome, EditError>>>,
    applied: bool,
}

pub(crate) struct Subscriber {
    pub(crate) conn: Option<u64>,
    /// The member's static when a member subscribed (§8.4: `Closed{demoted|revoked}` finds them).
    pub(crate) member: Option<String>,
    pub(crate) tx: mpsc::Sender<DocEvent>,
}

/// An import the helper is running; keyed by the helper request id. The update
/// (`ev: update`) and the reply arrive on two tasks: the entry lives until both
/// did, else the update could land after the reply and pass as a bare helper
/// transaction — stamped `Helper` and exported, when the import's `File` never
/// exports (that stray export is what made a `show` look like an edit, §7.2).
struct Import {
    doc: DocId,
    by: Principal,
    mode: ImportMode,
    sha: String,
    reply: ImportReply,
    /// The reply arrived, at this instant.
    done_at: Option<Instant>,
    /// The update arrived. An import that changed nothing never sends one: `Tick` sweeps those.
    applied: bool,
    /// 184 §5.3: a new document's first import — snapshot once both halves landed, so the mirror
    /// ships it within its debounce.
    first: bool,
}

pub(crate) enum ImportReply {
    None,
    Outcome(ReplySender<Result<ImportOutcome, String>>),
    Created(DocId, ReplySender<Result<DocId, String>>),
    CreatedNote(DocSummary, ReplySender<Result<DocSummary, String>>),
    /// 144: a notebook — the row's columns ride along with the document summary.
    CreatedNotebook(
        crate::notebook::Notebook,
        ReplySender<Result<crate::notebook::Notebook, String>>,
    ),
}

/// What an import did (§4.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, kameo::Reply)]
pub struct ImportOutcome {
    /// Body blocks replaced or added.
    pub changed: u32,
    /// Body blocks after the import.
    pub total: u32,
    /// The text was our own last export: nothing done.
    pub skipped: bool,
}

/// What an edit did (§5.2 tool results).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, kameo::Reply)]
pub struct EditOutcome {
    /// Ids inserted.
    pub inserted: Vec<String>,
    /// Ids removed.
    pub removed: Vec<String>,
}

/// The [`crate::DOCUMENT_ACTOR`] actor.
pub struct DocumentActor {
    pub(crate) store: Store,
    pub(crate) helper: Helper,
    pub(crate) cfg: DocumentsConfig,
    pub(crate) rt: Runtime,
    pub(crate) data_dir: PathBuf,
    pub(crate) open: HashMap<DocId, Open>,
    pub(crate) subs: HashMap<DocId, Vec<Subscriber>>,
    imports: HashMap<u64, Import>,
    edits: HashMap<u64, PendingEdit>,
    last_sweep: Option<Instant>,
    /// Set by [`Flush`]: every export that lands from here on is reported to its owner at once.
    shutting_down: bool,
    /// 69: the lease follower and the watcher token; every hub-only handler asks `hub.gate()`.
    pub(crate) hub: crate::hub::Hub,
    /// 69: off the hub, the `theuth/docs/1` legs carrying clients' sessions to the holder.
    pub(crate) legs: crate::relay::Legs,
    /// 184 §4.3: claims on homed documents this engine serves (or is about to).
    pub(crate) claims: HashMap<DocId, crate::hub::Claim>,
    /// 184 §4.2 rule 1: the `home_of` memo of closed documents.
    pub(crate) homes: HashMap<DocId, Option<mesh::EngineId>>,
    /// 184 §5.3: forwarded creates' homes until their `meta.json` arrives.
    pub(crate) pending_homes: HashMap<DocId, (mesh::EngineId, Instant)>,
    /// 184 §10.2: the message being handled arrived as [`crate::hub::Relayed`].
    pub(crate) relaying: bool,
    /// 184 §8.1: the last `DocsGate` poll from `Tick`.
    last_gate_poll: Option<Instant>,
    /// 184 §8.1: the mesh's latest docs-gate answer; an "off" applies only if it still holds
    /// after [`crate::hub::GATE_OFF_SETTLE`].
    pub(crate) gate_latest: bool,
    /// 184 §4.9 (Z184-R2): the unpaired engines the last re-home scan covered, sorted.
    pub(crate) rehomed_for: Vec<mesh::EngineId>,
    /// This actor (set at start): tasks it spawns talk back to it, not to the registered name.
    me: Option<WeakActorRef<Self>>,
    /// 218 §3.3: whiteboard turns, current whiteboards per session (in memory).
    pub(crate) wb: crate::wb::engine::WbState,
}

impl kameo::Actor for DocumentActor {
    type Args = Self;
    type Error = kameo::error::Infallible;

    async fn on_start(mut state: Self, me: ActorRef<Self>) -> Result<Self, Self::Error> {
        state.me = Some(me.downgrade());
        Ok(state)
    }
}

impl DocumentActor {
    /// Build; the caller spawns and registers it.
    pub fn new(store: Store, helper: Helper, cfg: DocumentsConfig, rt: Runtime) -> Self {
        Self {
            store,
            helper,
            cfg,
            data_dir: rt.data_dir.clone(),
            rt,
            open: HashMap::new(),
            subs: HashMap::new(),
            imports: HashMap::new(),
            edits: HashMap::new(),
            last_sweep: None,
            shutting_down: false,
            hub: Default::default(),
            legs: Default::default(),
            claims: HashMap::new(),
            homes: HashMap::new(),
            pending_homes: HashMap::new(),
            relaying: false,
            last_gate_poll: None,
            gate_latest: false,
            rehomed_for: Vec::new(),
            me: None,
            wb: Default::default(),
        }
    }

    /// Load `id` if it is not, and open its mirror in the helper (218: never for a whiteboard —
    /// it has no XML fragment and no helper mirror).
    pub(crate) fn load(&mut self, id: &DocId) -> Result<&mut Open, String> {
        if !self.open.contains_key(id) {
            let l = self.store.load(id).map_err(|e| format!("{e:#}"))?;
            let touched = Arc::new(Mutex::new(Vec::new()));
            if !l.meta.kind.is_pure_yjs() {
                let state = l
                    .doc
                    .transact()
                    .encode_state_as_update_v1(&StateVector::default());
                self.helper.open(id, &state, &l.meta.fragment).log("open");
                let sink = touched.clone();
                l.doc
                    .get_or_insert_xml_fragment(l.meta.fragment.as_str())
                    .observe_deep("authors", move |txn, events| {
                        let mut sink = sink.lock().unwrap_or_else(|p| p.into_inner());
                        for e in events.iter() {
                            touched_blocks(txn, e, &mut sink);
                        }
                    });
            }
            let agents = self.store.read_agents(id);
            self.open.insert(
                id.clone(),
                Open {
                    awareness: Awareness::new(l.doc.clone()),
                    doc: l.doc,
                    meta: l.meta,
                    log_count: l.log_count,
                    log_bytes: l.log_bytes,
                    bytes: l.snapshot_bytes + l.log_bytes,
                    idle_since: Instant::now(),
                    export_due: None,
                    export_first: None,
                    exporting: false,
                    notify_due: None,
                    last_agent_edit: None,
                    agent_gen: 0,
                    awareness_rate: HashMap::new(),
                    touched,
                    agents,
                    authors_dirty: false,
                    meta_dirty: false,
                    dual: None,
                },
            );
            self.enforce_open_cap();
        }
        Ok(self.open.get_mut(id).expect("inserted"))
    }

    /// Fan an event to every subscriber of `doc` but the one on `skip`; drop the dead and the slow.
    pub(crate) fn fan_out(&mut self, doc: &DocId, ev: DocEvent, skip: Option<u64>) {
        let Some(subs) = self.subs.get_mut(doc) else {
            return;
        };
        subs.retain(|s| {
            if skip.is_some() && s.conn == skip {
                return !s.tx.is_closed();
            }
            match s.tx.try_send(ev.clone()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(doc = %doc, "document subscriber too slow; dropped");
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
        if subs.is_empty() {
            self.subs.remove(doc);
        }
    }

    /// Apply one update from `by` (§6.2): CRDT, log, fan-out, helper mirror, export.
    /// `touched` (a helper transaction's written ids) precedes the update as `Authored` when `by` is the agent.
    pub(crate) async fn apply(
        &mut self,
        doc: &DocId,
        update: Vec<u8>,
        by: Principal,
        from_helper: bool,
        touched: Vec<String>,
    ) -> Result<(), String> {
        let max = self.cfg.max_doc_bytes;
        let store = self.store.clone();
        let mut force_snapshot_after = false;
        {
            let o = self.load(doc)?;
            if o.bytes + update.len() > max {
                let log_count = o.log_count;
                let is_deck = o.meta.kind == DocKind::Deck;
                if log_count > 0 {
                    let _ = self.snapshot(doc, None);
                }
                let o = self.load(doc)?;
                if o.bytes + update.len() > max {
                    if is_deck {
                        // Allow Deck compaction/archiving updates that bring or move the compacted
                        // Y.Doc state within/toward `max_doc_bytes` so an oversized Deck is never bricked.
                        let pre_state = o
                            .doc
                            .transact()
                            .encode_state_as_update_v1(&StateVector::default());
                        let scratch = yrs::Doc::new();
                        let u_check =
                            Update::decode_v1(&update).map_err(|e| format!("bad update: {e}"))?;
                        if let Ok(pre_u) = Update::decode_v1(&pre_state) {
                            let _ = scratch.transact_mut().apply_update(pre_u);
                        }
                        scratch
                            .transact_mut()
                            .apply_update(u_check)
                            .map_err(|e| format!("apply update: {e}"))?;
                        let post_len = scratch
                            .transact()
                            .encode_state_as_update_v1(&StateVector::default())
                            .len();
                        if post_len <= max || post_len < pre_state.len() {
                            force_snapshot_after = true;
                        } else {
                            return Err(format!(
                                "DocTooLarge: document would exceed {max} bytes ({} + {}, compacted {})",
                                o.bytes,
                                update.len(),
                                post_len
                            ));
                        }
                    } else {
                        return Err(format!(
                            "DocTooLarge: document would exceed {max} bytes ({} + {})",
                            o.bytes,
                            update.len()
                        ));
                    }
                }
            }
        }
        let o = self.load(doc)?;
        let u = Update::decode_v1(&update).map_err(|e| format!("bad update: {e}"))?;
        // 184 §4.7, §16: a client update that changes neither the Doc's state vector nor its
        // delete set (a catch-all echoing the delete set, a re-sent update) is a no-op: no log
        // record, no `updated_ms`, no export, no fan-out. Helper updates always go through: an
        // import or edit that changed nothing still completes through them (ingest refresh). An
        // update whose first struct per client is at or past the Doc's clock (every keystroke) is
        // new and skips the check. ponytail: the delete-set comparison walks the Doc's blocks, only
        // for structless or re-sent client updates.
        let have = o.doc.transact().state_vector();
        let maybe_old = !from_helper
            && u.state_vector_lower()
                .iter()
                .all(|(c, clock)| *clock < have.get(c));
        let deleted_before = maybe_old.then(|| o.doc.transact().snapshot().delete_set);
        o.touched.lock().unwrap_or_else(|p| p.into_inner()).clear();
        o.doc
            .transact_mut()
            .apply_update(u)
            .map_err(|e| format!("apply update: {e}"))?;
        if let Some(before) = deleted_before {
            let txn = o.doc.transact();
            if txn.state_vector() == have && txn.snapshot().delete_set.diff(&before).is_empty() {
                return Ok(());
            }
        }
        // §11: a member's blocks carry their author until someone else writes them; 171 §9.6: so do the agent's,
        // whose helper transaction also names what it wrote (a first import's blocks arrive inside one new
        // `blockGroup`, which the observer cannot see into).
        let mut written = std::mem::take(&mut *o.touched.lock().unwrap_or_else(|p| p.into_inner()));
        if matches!(by, Principal::Agent { .. }) {
            for id in &touched {
                if !written.contains(id) {
                    written.push(id.clone());
                }
            }
        }
        for id in written {
            let changed = match &by {
                Principal::Member { pub_, .. } => {
                    o.agents.remove(&id).is_some()
                        | (o.meta.authors.insert(id, pub_.clone()).as_deref() != Some(pub_))
                }
                Principal::Agent { session } => {
                    o.meta.authors.remove(&id).is_some()
                        | (o.agents.insert(id, *session) != Some(*session))
                }
                _ => o.meta.authors.remove(&id).is_some() | o.agents.remove(&id).is_some(),
            };
            o.authors_dirty |= changed;
        }
        let now = now_ms();
        let n = store
            .append(doc, &by, &update)
            .map_err(|e| format!("updates.log: {e:#}"))?;
        o.log_count += 1;
        o.log_bytes += n;
        o.bytes += update.len();
        o.idle_since = Instant::now();
        o.meta.updated_ms = now;
        let whiteboard = o.meta.kind == DocKind::Whiteboard;
        let pure_yjs = o.meta.kind.is_pure_yjs();
        if !from_helper && !pure_yjs {
            self.helper.update(doc, &update).log("update");
        }
        // The file already holds what a File import brought in; only other authors change the derived file.
        if by != Principal::File {
            self.schedule_export(doc);
        }
        if let Principal::Agent { session } = &by
            && !touched.is_empty()
        {
            self.fan_out(
                doc,
                DocEvent::Authored {
                    blocks: touched,
                    by: Who::Agent {
                        session: session.to_string(),
                    },
                },
                None,
            );
        }
        let mut who = Who::from(&by);
        if let Who::Member { pub_, name } = &mut who
            && let Some(m) = self
                .open
                .get(doc)
                .and_then(|o| o.meta.members.iter().find(|m| m.pub_ == *pub_))
        {
            *name = m.name.clone();
        }
        let ev = DocEvent::Update {
            update: base64::engine::general_purpose::STANDARD.encode(&update),
            by: who,
        };
        // 69: the index row (`updated_ms`, `bytes`) is replicated; it moves at snapshot/export, not per keystroke.
        self.fan_out(doc, ev, by.conn());
        // 218 R8: a whiteboard past 2 MiB bakes its old answered ink (a separate write).
        if whiteboard && by != Principal::Helper {
            self.wb_bake_if_due(doc);
        }
        if force_snapshot_after
            && let Err(e) = self.snapshot(doc, None)
        {
            tracing::warn!(doc = %doc, error = %e, "post-compaction snapshot failed");
        }
        Ok(())
    }

    pub(crate) fn schedule_export(&mut self, doc: &DocId) {
        // 69, 184 §4.6: exports are the serving engine's duty; a replica's derived files come from
        // their owners (knowledge sync).
        if !self.serves(doc) {
            return;
        }
        let debounce = Duration::from_millis(self.cfg.export_debounce_ms);
        let quiet = Duration::from_secs(self.cfg.export_quiet_secs);
        if let Some(o) = self.open.get_mut(doc)
            && !o.meta.derived.is_empty()
        {
            let now = Instant::now();
            o.export_due = Some(now + debounce);
            o.export_first.get_or_insert(now);
            o.notify_due = Some(now + quiet);
        }
    }

    /// Whether `doc`'s owner is to hear [`Exported`] now: an update is waiting to be reported, the file holds
    /// it (no export pending or in flight), and the quiet period passed — or nobody is typing (no subscriber),
    /// or the harness is stopping.
    fn owner_due(&self, doc: &DocId, o: &Open, now: Instant) -> bool {
        o.notify_due.is_some_and(|due| {
            !o.exporting
                && o.export_due.is_none()
                && (now >= due || self.shutting_down || !self.subs.contains_key(doc))
        })
    }

    /// Tell `doc`'s owner its derived file is settled (§7): knowledge commits, the artifact store versions.
    async fn notify_owner(&mut self, doc: &DocId) {
        let Some(o) = self.open.get_mut(doc) else {
            return;
        };
        o.notify_due = None;
        let ev = Exported {
            doc: doc.clone(),
            binding: o.meta.binding.clone(),
            path: self.data_dir.join(&o.meta.derived),
        };
        self.rt.publish(ev).await;
    }

    /// The derived file's text when `doc` was never synced with it — no export or import ever
    /// landed (`meta.last_export` is `None`), none is in flight, and the file has a body. That is a
    /// binding whose first import failed (the helper was down): the blank page the installed app
    /// showed. `None` for a fresh note (its file does not exist until the first export).
    fn unsynced_file(&self, doc: &DocId) -> Option<String> {
        if self.imports.values().any(|i| i.doc == *doc) {
            return None;
        }
        // 218/229: pure-Yjs files (whiteboard/deck) are written from the document, never imported.
        if self
            .open
            .get(doc)
            .is_some_and(|o| o.meta.kind.is_pure_yjs())
        {
            return None;
        }
        let derived = match self.open.get(doc) {
            Some(o) => o
                .meta
                .last_export
                .is_none()
                .then(|| o.meta.derived.clone())?,
            None => {
                let m = self.store.read_meta(doc).ok()?;
                (m.last_export.is_none() && !m.kind.is_pure_yjs()).then_some(m.derived)?
            }
        };
        let text = std::fs::read_to_string(self.data_dir.join(derived)).ok()?;
        (!text.trim().is_empty()).then_some(text)
    }

    /// Start an import through the helper; the transaction returns as a [`HelperEvent::Update`].
    pub(crate) fn start_import(
        &mut self,
        doc: &DocId,
        markdown: String,
        by: Principal,
        mode: ImportMode,
        reply: ImportReply,
    ) {
        let sha = sha256(markdown.as_bytes());
        let o = match self.load(doc) {
            Ok(o) => o,
            Err(e) => {
                finish_reply(reply, Err(e));
                return;
            }
        };
        // 218/229: pure-Yjs docs have no blocks; nothing imports into them.
        if o.meta.kind == DocKind::Whiteboard {
            finish_reply(reply, Err(crate::wb::DOC_REFUSAL.into()));
            return;
        }
        if o.meta.kind == DocKind::Deck {
            finish_reply(reply, Err(crate::deck::DOC_REFUSAL.into()));
            return;
        }
        if mode == ImportMode::Diff && o.meta.last_export.as_deref() == Some(sha.as_str()) {
            finish_reply(
                reply,
                Ok(ImportOutcome {
                    skipped: true,
                    ..Default::default()
                }),
            );
            return;
        }
        // The import is about to make the doc agree with this text; an export
        // still pending from an earlier edit is rescheduled by that edit's successor.
        if by == Principal::File {
            o.export_due = None;
            o.export_first = None;
        }
        let frontmatter = matches!(o.meta.kind, DocKind::Knowledge | DocKind::Board);
        let pending = self.helper.import(doc, &markdown, mode, frontmatter);
        let req = pending.id();
        let first = matches!(reply, ImportReply::CreatedNote(..));
        self.imports.insert(
            req,
            Import {
                doc: doc.clone(),
                by,
                mode,
                sha,
                reply,
                done_at: None,
                applied: false,
                first,
            },
        );
        let me = self.actor_ref();
        tokio::spawn(async move {
            let result = pending.import_result().await;
            let _ = me.tell(ImportDone { req, result }).await;
        });
    }

    /// Start a §5.2 edit through the helper. The transaction returns as a
    /// [`HelperEvent::Update`] stamped `by`; the reply as [`EditDone`].
    fn start_edit(
        &mut self,
        doc: &DocId,
        edits: Vec<BlockEdit>,
        by: Principal,
        reply: ReplySender<Result<EditOutcome, EditError>>,
    ) {
        if let Err(e) = self.load(doc) {
            reply.send(Err(EditError::Other(e)));
            return;
        }
        // 218/229: pure-Yjs docs are edited with their dedicated tools only.
        if let Some(kind) = self.open.get(doc).map(|o| o.meta.kind) {
            if kind == DocKind::Whiteboard {
                reply.send(Err(EditError::Other(crate::wb::DOC_REFUSAL.into())));
                return;
            }
            if kind == DocKind::Deck {
                reply.send(Err(EditError::Other(crate::deck::DOC_REFUSAL.into())));
                return;
            }
        }
        // 144 §3.1: a source's text is derived; only the helper (ingest, the insight job) edits it.
        if let Err(e) = self.source_writable(doc, &by) {
            reply.send(Err(EditError::Other(e.to_string())));
            return;
        }
        // §6.4: the agent is present before its update lands; the touched ids follow with the update.
        if matches!(by, Principal::Agent { .. }) {
            self.set_agent_presence(doc, &[]);
        }
        let pending = self.helper.edit(doc, &edits);
        let req = pending.id();
        self.edits.insert(
            req,
            PendingEdit {
                doc: doc.clone(),
                by,
                reply: Some(reply),
                applied: false,
            },
        );
        let me = self.actor_ref();
        tokio::spawn(async move {
            let result = pending.edit_result().await;
            let _ = me.tell(EditDone { req, result }).await;
        });
    }

    /// Publish the synthetic agent client's awareness state (§6.4, §8.5). The authority's own
    /// awareness client never edits, so its state *is* the agent's; cleared [`AGENT_PRESENCE`] later.
    fn set_agent_presence(&mut self, doc: &DocId, blocks: &[String]) {
        let color = self.cfg.agent_color.clone();
        let Some(o) = self.open.get_mut(doc) else {
            return;
        };
        let user = serde_json::json!({ "name": "theuth", "color": color });
        let state = serde_json::json!({
            "user": user,
            "name": "theuth",
            "color": color,
            "agent": { "doc": doc.as_str(), "blocks": blocks, "at": now_ms() },
        });
        if let Err(e) = o.awareness.set_local_state(state) {
            tracing::warn!(doc = %doc, error = %e, "agent awareness state");
            return;
        }
        o.agent_gen += 1;
        let generation = o.agent_gen;
        self.fan_out_agent_awareness(doc);
        let me = self.actor_ref();
        let doc = doc.clone();
        tokio::spawn(async move {
            tokio::time::sleep(AGENT_PRESENCE).await;
            let _ = me
                .tell(ClearAgentPresence {
                    doc,
                    generation: Some(generation),
                })
                .await;
        });
    }

    fn fan_out_agent_awareness(&mut self, doc: &DocId) {
        let Some(o) = self.open.get(doc) else {
            return;
        };
        let client = o.awareness.client_id();
        match o.awareness.update_with_clients([client]) {
            Ok(u) => {
                let state = b64(&u.encode_v1());
                self.fan_out(doc, DocEvent::Awareness { state }, None);
            }
            Err(e) => tracing::warn!(doc = %doc, error = %e, "agent awareness update"),
        }
    }

    fn clear_agent_presence(&mut self, doc: &DocId) {
        let Some(o) = self.open.get_mut(doc) else {
            return;
        };
        if o.awareness.local_state_raw().is_none() {
            return;
        }
        o.awareness.clean_local_state();
        self.fan_out_agent_awareness(doc);
    }

    pub(crate) fn actor_ref(&self) -> ActorRef<Self> {
        self.me
            .as_ref()
            .and_then(WeakActorRef::upgrade)
            .or_else(|| {
                ActorRef::<Self>::lookup(crate::DOCUMENT_ACTOR)
                    .ok()
                    .flatten()
            })
            .expect("the document actor runs before it serves")
    }

    /// Fire the export of `doc`: helper markdown → tmp + rename → [`ExportDone`]; `done` hears the outcome.
    pub(crate) fn export(
        &mut self,
        doc: &DocId,
        done: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) {
        let Some(o) = self.open.get_mut(doc) else {
            return;
        };
        o.exporting = true;
        o.export_due = None;
        o.export_first = None;
        let path = self.data_dir.join(&o.meta.derived);
        let kind = o.meta.kind;
        // 218/229: pure-Yjs derived files are rendered directly in Rust; never the helper.
        let outline = match kind {
            DocKind::Whiteboard => Some(crate::wb::engine::export_text(o)),
            DocKind::Deck => Some(crate::deck::export_text(o)),
            _ => None,
        };
        let helper = self.helper.clone();
        let me = self.actor_ref();
        let doc = doc.clone();
        tokio::spawn(async move {
            let text = match outline {
                Some(text) => Ok(text),
                None => helper.markdown(&doc).await.map_err(|e| e.to_string()),
            };
            let result = match text {
                Ok(text) => write_atomic(&path, text.as_bytes())
                    .map(|()| (sha256(text.as_bytes()), title_of(kind, &text)))
                    .map_err(|e| format!("write {}: {e}", path.display())),
                Err(e) => Err(e),
            };
            if let Some(done) = done {
                let _ = done.send(result.as_ref().map(|_| ()).map_err(Clone::clone));
            }
            let _ = me.tell(ExportDone { doc, result }).await;
        });
    }

    pub(crate) fn snapshot(&mut self, doc: &DocId, label: Option<&str>) -> Result<(), String> {
        let keep = self.cfg.snapshots_keep;
        let store = self.store.clone();
        let Some(o) = self.open.get_mut(doc) else {
            return Err(format!("{doc} is not loaded"));
        };
        let n = store
            .snapshot(doc, &o.doc, label)
            .map_err(|e| format!("snapshot: {e:#}"))?;
        o.log_count = 0;
        o.log_bytes = 0;
        o.bytes = n;
        let (id, updated_ms, s) = (doc.clone(), o.meta.updated_ms, store.clone());
        tokio::spawn(async move {
            if let Err(e) = s.touch_at_snapshot(&id, updated_ms, n).await {
                tracing::warn!(doc = %id, error = %e, "documents index touch failed");
            }
        });
        if label.is_some() {
            let _ = store.prune_snapshots(doc, keep);
            self.fan_out(
                doc,
                DocEvent::Snapshot {
                    label: label.unwrap_or_default().to_owned(),
                },
                None,
            );
        }
        Ok(())
    }

    pub(crate) fn unload(&mut self, doc: &DocId, reason: &str) {
        if self.open.get(doc).is_some_and(|o| o.log_count > 0)
            && let Err(e) = self.snapshot(doc, None)
        {
            tracing::warn!(doc = %doc, error = %e, "snapshot at unload failed; log kept");
        }
        if let Some(o) = self.open.remove(doc) {
            if !o.meta.kind.is_pure_yjs() {
                self.helper.close(doc).log("close");
            }
            self.homes.insert(doc.clone(), o.meta.home.clone());
            // A quiet period still running at unload is over: the owner hears now, nothing is lost.
            if o.notify_due.is_some() && !o.exporting && o.export_due.is_none() {
                let rt = self.rt.clone();
                let ev = Exported {
                    doc: doc.clone(),
                    binding: o.meta.binding.clone(),
                    path: self.data_dir.join(&o.meta.derived),
                };
                tokio::spawn(async move { rt.publish(ev).await });
            }
            if o.authors_dirty || o.meta_dirty {
                let store = self.store.clone();
                tokio::spawn(async move {
                    if o.authors_dirty
                        && let Err(e) = store.write_agents(&o.meta.id, &o.agents)
                    {
                        tracing::warn!(doc = %o.meta.id, error = %e, "agents.json: not saved at unload");
                    }
                    if let Err(e) = store.write_meta(&o.meta).await {
                        tracing::warn!(doc = %o.meta.id, error = %e, "meta.json: authors not saved at unload");
                    }
                });
            }
        }
        self.drop_claim(doc);
        if let Some(subs) = self.subs.remove(doc) {
            for s in subs {
                let _ = s.tx.try_send(DocEvent::Closed {
                    reason: reason.to_owned(),
                });
            }
        }
    }

    fn enforce_open_cap(&mut self) {
        while self.open.len() > self.cfg.max_open_docs {
            let victim = self
                .open
                .iter()
                .filter(|(id, o)| !self.subs.contains_key(*id) && !o.exporting)
                .min_by_key(|(_, o)| o.idle_since)
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => self.unload(&id, "unloaded"),
                None => break,
            }
        }
    }

    /// Close, move the directory to `.trash/<id>`, mark deleted, tell subscribers (§7.1, §7.3).
    pub(crate) async fn delete_doc(&mut self, doc: &DocId) -> Result<(), String> {
        self.drop_claim(doc);
        self.homes.remove(doc);
        let changed = match self.open.remove(doc) {
            Some(o) => {
                if !o.meta.kind.is_pure_yjs() {
                    self.helper.close(doc).log("close");
                }
                Some(DocumentChanged::of(&o.meta, true))
            }
            None => match self.store.read_meta(doc) {
                Ok(meta) => Some(DocumentChanged::of(&meta, true)),
                Err(_) => self
                    .store
                    .summary(doc)
                    .await
                    .ok()
                    .flatten()
                    .map(|row| DocumentChanged {
                        doc: row.id,
                        kind: row.kind,
                        title: row.title,
                        updated_ms: row.updated_ms,
                        deleted: true,
                    }),
            },
        };
        if let Some(subs) = self.subs.remove(doc) {
            for s in subs {
                let _ = s.tx.try_send(DocEvent::Closed {
                    reason: "deleted".into(),
                });
            }
        }
        self.store.delete(doc).await.map_err(|e| format!("{e:#}"))?;
        if let Some(ev) = changed {
            self.rt.publish(ev).await;
        }
        Ok(())
    }
}

fn finish_reply(reply: ImportReply, r: Result<ImportOutcome, String>) {
    match reply {
        ImportReply::None => {
            if let Err(e) = r {
                tracing::warn!(error = %e, "document import failed");
            }
        }
        ImportReply::Outcome(tx) => tx.send(r),
        ImportReply::Created(id, tx) => tx.send(r.map(|_| id)),
        ImportReply::CreatedNote(summary, tx) => tx.send(r.map(|_| summary)),
        ImportReply::CreatedNotebook(nb, tx) => tx.send(r.map(|_| nb)),
    }
}

/// Title of a derived file (§4.3): knowledge from the header's `title`, else the first heading, else the first 80 chars.
pub fn title_of(kind: DocKind, text: &str) -> String {
    if kind == DocKind::Knowledge
        && let Some(rest) = text.strip_prefix("---\n")
        && let Some((header, _)) = rest.split_once("\n---")
    {
        for line in header.lines() {
            if let Some(v) = line.strip_prefix("title:") {
                let v = v.trim();
                let v = v
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(v);
                return v.replace("\\\"", "\"");
            }
        }
    }
    let body = text
        .strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n").map(|(_, b)| b))
        .unwrap_or(text);
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let t = t.trim_start_matches('#').trim();
        return t.chars().take(80).collect();
    }
    String::new()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("bad base64: {e}"))
}

// ---- messages -------------------------------------------------------------

/// `ask` → `Result<DocOpen, String>`: load, and the state missing from `sv` (base64 state vector).
/// A member (§8.1) must be in `members` (`not_member` otherwise); the reply's `meta.links` is empty.
#[derive(Clone, Debug)]
pub struct OpenDoc {
    /// Document.
    pub doc: DocId,
    /// Client's state vector, base64 v1; `None` = everything.
    pub sv: Option<String>,
    /// Who opens.
    pub by: Principal,
}

impl Message<OpenDoc> for DocumentActor {
    type Reply = DelegatedReply<Result<DocOpen, DocError>>;

    async fn handle(&mut self, m: OpenDoc, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => tx.send(self.open_doc(m).await),
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            Ok(Route::Forward(to)) => self.forward(
                to,
                &m.doc,
                Client::of(&m.by),
                Cmd::Open {
                    sv: m.sv,
                    reply: tx,
                },
            ),
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl DocumentActor {
    async fn open_doc(&mut self, m: OpenDoc) -> Result<DocOpen, DocError> {
        let sv = match m.sv {
            Some(s) => StateVector::decode_v1(&unb64(&s)?).map_err(|e| format!("bad sv: {e}"))?,
            None => StateVector::default(),
        };
        self.admit(&m.doc, &m.by, false).await?;
        let o = self.load(&m.doc)?;
        o.idle_since = Instant::now();
        let mut seen = false;
        if let Some(pub_) = m.by.member()
            && let Some(member) = o.meta.members.iter_mut().find(|x| x.pub_ == pub_)
        {
            member.last_seen_ms = now_ms();
            seen = true;
        }
        let (state, server_sv) = {
            let txn = o.doc.transact();
            (
                txn.encode_state_as_update_v1(&sv),
                txn.state_vector().encode_v1(),
            )
        };
        let dual = o.dual.clone();
        let awareness = o
            .awareness
            .update()
            .map(|u| b64(&u.encode_v1()))
            .unwrap_or_default();
        let meta = DocMeta {
            links: Vec::new(),
            ..o.meta.clone()
        };
        if seen {
            self.save_members(&m.doc).await;
        }
        // Bound but never filled (the first import failed while the helper was down): import the
        // derived file now; the client gets the state as an ordinary update a moment after this reply.
        if let Some(markdown) = self.unsynced_file(&m.doc) {
            tracing::info!(doc = %m.doc, "document never imported; importing its file on open");
            self.start_import(
                &m.doc,
                markdown,
                Principal::File,
                ImportMode::Diff,
                ImportReply::None,
            );
        }
        Ok(DocOpen {
            meta,
            state: state.chunks(CHUNK).map(b64).collect(),
            awareness,
            // 184 §4.7: the client pushes what this state vector lacks; §10.3: who serves.
            server: self.hub.self_id(),
            dual,
            sv: b64(&server_sv),
        })
    }
}

/// `ask` → `Result<(), DocError>`: one v1 update (base64) from `by`; `UpdateTooLarge` past [`MAX_UPDATE_B64`],
/// `DocTooLarge` beyond the document cap; `NotHub` off the hub (69).
#[derive(Clone, Debug)]
pub struct ApplyUpdate {
    /// Document.
    pub doc: DocId,
    /// base64 v1 update.
    pub update: String,
    /// Author (its connection is skipped in fan-out).
    pub by: Principal,
}

/// One client update over the link, base64 (§4.7: 48 KiB; the client splits or refuses a bigger one).
/// Helper transactions (imports, agent edits) are not on the link and are not capped here.
pub const MAX_UPDATE_B64: usize = 48 * 1024;

impl Message<ApplyUpdate> for DocumentActor {
    type Reply = DelegatedReply<Result<(), DocError>>;

    async fn handle(
        &mut self,
        m: ApplyUpdate,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => tx.send(self.apply_update(m).await),
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            Ok(Route::Forward(hub)) => self.forward(
                hub,
                &m.doc,
                Client::of(&m.by),
                Cmd::Update {
                    update: m.update,
                    reply: tx,
                },
            ),
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl DocumentActor {
    async fn apply_update(&mut self, m: ApplyUpdate) -> Result<(), DocError> {
        if m.update.len() > MAX_UPDATE_B64 {
            return Err(format!(
                "UpdateTooLarge: one update may be {MAX_UPDATE_B64} bytes at most, not {}",
                m.update.len()
            )
            .into());
        }
        let update = unb64(&m.update)?;
        self.admit(&m.doc, &m.by, true).await?;
        self.source_writable(&m.doc, &m.by)?;
        {
            let o = self.load(&m.doc)?;
            if o.meta.kind == DocKind::Deck {
                crate::deck::validate_raw_update(&o.doc, &update, &m.by)?;
            }
        }
        Ok(self.apply(&m.doc, update, m.by, false, Vec::new()).await?)
    }
}

/// `ask` → `Result<mpsc::Receiver<DocEvent>, DocError>`: every event of `doc` not caused by `by`'s connection; hub only (69).
#[derive(Clone, Debug)]
pub struct Subscribe {
    /// Document.
    pub doc: DocId,
    /// The subscriber; its own connection's updates are not echoed, a member's role is checked (§8.1).
    pub by: Principal,
}

impl Message<Subscribe> for DocumentActor {
    type Reply = DelegatedReply<Result<mpsc::Receiver<DocEvent>, DocError>>;

    async fn handle(&mut self, m: Subscribe, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => tx.send(self.subscribe(m).await),
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            Ok(Route::Forward(hub)) => {
                self.forward(hub, &m.doc, Client::of(&m.by), Cmd::Subscribe { reply: tx })
            }
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl DocumentActor {
    async fn subscribe(&mut self, m: Subscribe) -> Result<mpsc::Receiver<DocEvent>, DocError> {
        self.admit(&m.doc, &m.by, false).await?;
        let o = self.load(&m.doc)?;
        o.idle_since = Instant::now();
        let (tx, rx) = mpsc::channel(SUB_QUEUE);
        self.subs.entry(m.doc).or_default().push(Subscriber {
            conn: m.by.conn(),
            member: m.by.member().map(str::to_owned),
            tx,
        });
        Ok(rx)
    }
}

/// `ask` → `Result<(), DocError>`: an awareness update (base64 y-protocols), relayed, never persisted; hub only (69).
#[derive(Clone, Debug)]
pub struct AwarenessUpdateMsg {
    /// Document.
    pub doc: DocId,
    /// base64 awareness update.
    pub state: String,
    /// Sender.
    pub by: Principal,
}

impl Message<AwarenessUpdateMsg> for DocumentActor {
    type Reply = DelegatedReply<Result<(), DocError>>;

    async fn handle(
        &mut self,
        m: AwarenessUpdateMsg,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => tx.send(self.awareness_update(m).await),
            // §4.3 step 4: a cursor is best-effort; dropped instead of parked.
            Ok(Route::Park) => tx.send(Ok(())),
            Ok(Route::Forward(hub)) => self.forward(
                hub,
                &m.doc,
                Client::of(&m.by),
                Cmd::Awareness {
                    state: m.state,
                    reply: tx,
                },
            ),
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl DocumentActor {
    async fn awareness_update(&mut self, m: AwarenessUpdateMsg) -> Result<(), DocError> {
        let bytes = unb64(&m.state)?;
        let u = AwarenessUpdate::decode_v1(&bytes).map_err(|e| format!("bad awareness: {e}"))?;
        self.admit(&m.doc, &m.by, false).await?;
        let o = self.load(&m.doc)?;
        let now = Instant::now();
        let slot = o.awareness_rate.entry(m.by.conn()).or_insert(RateSlot {
            since: now,
            sent: 0,
        });
        if !admit_awareness(slot, now) {
            tracing::trace!(doc = %m.doc, conn = ?m.by.conn(), "awareness over the rate cap; dropped");
            return Ok(());
        }
        o.awareness
            .apply_update(u)
            .map_err(|e| format!("awareness: {e}"))?;
        self.fan_out(&m.doc, DocEvent::Awareness { state: m.state }, m.by.conn());
        Ok(())
    }
}

/// `ask` → `Result<Option<DocId>, String>`: the live document bound to `binding`.
#[derive(Clone, Debug)]
pub struct Lookup {
    /// Binding.
    pub binding: Binding,
}

impl Message<Lookup> for DocumentActor {
    type Reply = Result<Option<DocId>, String>;

    async fn handle(&mut self, m: Lookup, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .lookup(&m.binding)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → `Result<DocId, String>`: the document bound to `binding`, created
/// from `markdown` (mode `replace`) when there is none yet (§7.1 on demand).
#[derive(Clone, Debug)]
pub struct Ensure {
    /// Binding.
    pub binding: Binding,
    /// The derived file's current text.
    pub markdown: String,
}

impl Message<Ensure> for DocumentActor {
    type Reply = DelegatedReply<Result<DocId, String>>;

    async fn handle(&mut self, m: Ensure, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // 184 §5.2: a hub-only kind is created by the holder; off it, `Cmd::Create` asks it.
        match self.route_home(None) {
            Ok(None) => {}
            Ok(Some(to)) => {
                let id = DocId::for_binding(&m.binding);
                let req = CreateReq::Ensure {
                    binding: m.binding,
                    markdown: m.markdown,
                };
                let ask = self.create_on::<DocSummary>(to, id, Client::Conn(None), req, None);
                tokio::spawn(async move { tx.send(ask.await.map(|s| s.id)) });
                return reply;
            }
            Err(e) => {
                tx.send(Err(e.into()));
                return reply;
            }
        }
        match self.store.lookup(&m.binding).await {
            Ok(Some(id)) => {
                // Bound but never filled (see `unsynced_file`): import now and answer when it has
                // landed, like a first open; `Diff` keeps an edit typed meanwhile.
                if self.unsynced_file(&id).is_some() && !m.markdown.trim().is_empty() {
                    tracing::info!(doc = %id, "document never imported; importing its file now");
                    self.start_import(
                        &id,
                        m.markdown,
                        Principal::File,
                        ImportMode::Diff,
                        ImportReply::Created(id.clone(), tx),
                    );
                    return reply;
                }
                tx.send(Ok(id));
                return reply;
            }
            Ok(None) => {}
            Err(e) => {
                tx.send(Err(format!("{e:#}")));
                return reply;
            }
        }
        let id = DocId::for_binding(&m.binding);
        let now = now_ms();
        let kind = m.binding.kind();
        let meta = DocMeta {
            derived: m.binding.derived(&id),
            id: id.clone(),
            kind,
            title: title_of(kind, &m.markdown),
            binding: m.binding,
            fragment: FRAGMENT.to_owned(),
            members: vec![],
            links: vec![],
            agent: AgentStyle::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: now,
            updated_ms: now,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        };
        if let Err(e) = self.store.create(&meta).await {
            tx.send(Err(format!("{e:#}")));
            return reply;
        }
        self.start_import(
            &id,
            m.markdown,
            Principal::File,
            ImportMode::Replace,
            ImportReply::Created(id.clone(), tx),
        );
        reply
    }
}

/// `ask` → `Result<ImportOutcome, String>`: derived markdown → one Y transaction (§4.4).
#[derive(Clone, Debug)]
pub struct ImportMarkdown {
    /// Document.
    pub doc: DocId,
    /// The whole file.
    pub markdown: String,
    /// Who the change stands for (`File` for every step-1 path).
    pub by: Principal,
    /// `Diff` unless the caller knows better.
    pub mode: ImportMode,
}

impl Message<ImportMarkdown> for DocumentActor {
    type Reply = DelegatedReply<Result<ImportOutcome, String>>;

    async fn handle(
        &mut self,
        m: ImportMarkdown,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // 184 §4.6: the serving engine imports (a homed journal on its home).
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        self.start_import(&m.doc, m.markdown, m.by, m.mode, ImportReply::Outcome(tx));
        reply
    }
}

/// `tell` (internal): the helper answered an import.
struct ImportDone {
    req: u64,
    result: Result<helper::ImportResult, helper::Error>,
}

impl Message<ImportDone> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: ImportDone, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(imp) = self.imports.get_mut(&m.req) else {
            return;
        };
        let reply = std::mem::replace(&mut imp.reply, ImportReply::None);
        let (doc, sha) = (imp.doc.clone(), imp.sha.clone());
        // 184 §5.3: a create whose update already landed snapshots now (else at the update).
        let first = imp.first && imp.applied && m.result.is_ok();
        // A failed import sends no update; a successful one that changed nothing may not either.
        if m.result.is_err() || imp.applied {
            self.imports.remove(&m.req);
        } else {
            imp.done_at = Some(Instant::now());
        }
        match m.result {
            Ok(r) => {
                if let Some(o) = self.open.get_mut(&doc) {
                    // The text and the doc agree now unless an edit is still waiting to be exported.
                    if o.export_due.is_none() && !o.exporting {
                        o.meta.last_export = Some(sha);
                    }
                    let meta = o.meta.clone();
                    if let Err(e) = self.store.write_meta(&meta).await {
                        tracing::warn!(doc = %doc, error = %e, "meta.json write failed");
                    }
                }
                if first && let Err(e) = self.snapshot(&doc, None) {
                    tracing::warn!(doc = %doc, error = %e, "documents: first snapshot failed");
                }
                finish_reply(
                    reply,
                    Ok(ImportOutcome {
                        changed: r.changed,
                        total: r.total,
                        skipped: false,
                    }),
                );
            }
            Err(e) => finish_reply(reply, Err(e.to_string())),
        }
    }
}

/// `tell` (internal): the export task finished.
struct ExportDone {
    doc: DocId,
    /// `(sha256 of the file, title)`.
    result: Result<(String, String), String>,
}

impl Message<ExportDone> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: ExportDone, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(o) = self.open.get_mut(&m.doc) else {
            return;
        };
        o.exporting = false;
        match m.result {
            Ok((sha, title)) => {
                o.meta.last_export = Some(sha);
                // A note or board keeps the title it was created with; the others follow their text.
                if !title.is_empty() && !o.meta.kind.free_standing() {
                    o.meta.title = title;
                }
                let meta = o.meta.clone();
                if let Err(e) = self.store.write_meta(&meta).await {
                    tracing::warn!(doc = %m.doc, error = %e, "meta.json write failed");
                }
                self.fan_out(&m.doc, DocEvent::Exported, None);
                self.rt.publish(DocumentChanged::of(&meta, false)).await;
                // The owner waits for the quiet period (Tick) unless nobody is typing or the harness is stopping.
                let due = self
                    .open
                    .get(&m.doc)
                    .is_some_and(|o| self.owner_due(&m.doc, o, Instant::now()));
                if due {
                    self.notify_owner(&m.doc).await;
                }
            }
            Err(e) => {
                tracing::warn!(doc = %m.doc, error = %e, "document export failed; retrying");
                let now = Instant::now();
                o.export_due = Some(now + Duration::from_secs(2));
                o.export_first = Some(now);
            }
        }
    }
}

/// `ask` → `Result<EditOutcome, EditError>`: §5.2 block edits as ONE transaction, stamped `by`
/// (`Agent{session}` from the tools; `Owner` for the undo). The agent's inverse is kept for [`UndoLastAgentEdit`].
#[derive(Clone, Debug)]
pub struct Edit {
    /// Document.
    pub doc: DocId,
    /// The edits, in order.
    pub edits: Vec<BlockEdit>,
    /// Who the change stands for.
    pub by: Principal,
}

impl Message<Edit> for DocumentActor {
    type Reply = DelegatedReply<Result<EditOutcome, EditError>>;

    async fn handle(&mut self, m: Edit, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // E2: the tools and owner plugins run on every engine; off the document's server the edit
        // rides a `theuth/docs/1` leg (`Cmd::Edit`) and the server's typed outcome comes back. A
        // server that cannot be reached answers `NotHub:<id>` — an error the caller sees and may
        // retry, never a drop.
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => self.start_edit(&m.doc, m.edits, m.by, tx),
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            Ok(Route::Forward(hub)) => self.forward(
                hub,
                &m.doc,
                Client::of(&m.by),
                Cmd::Edit {
                    edits: m.edits,
                    by: m.by,
                    reply: tx,
                },
            ),
            Err(e) => tx.send(Err(EditError::Other(e.to_string()))),
        }
        reply
    }
}

/// `tell` (internal): the helper answered an edit.
struct EditDone {
    req: u64,
    result: Result<helper::EditResult, helper::Error>,
}

impl Message<EditDone> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: EditDone, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(e) = self.edits.get_mut(&m.req) else {
            return;
        };
        let Some(tx) = e.reply.take() else { return };
        let doc = e.doc.clone();
        let by = e.by.clone();
        match m.result {
            Ok(r) => {
                // The update may still be on its way; the entry then goes with it.
                if e.applied {
                    self.edits.remove(&m.req);
                }
                if let Principal::Agent { session } = by
                    && let Some(o) = self.open.get_mut(&doc)
                {
                    o.last_agent_edit = Some(LastAgentEdit {
                        session,
                        undo: r.undo,
                    });
                }
                tx.send(Ok(EditOutcome {
                    inserted: r.inserted,
                    removed: r.removed,
                }));
            }
            Err(err) => {
                // Presence set at start goes with its own delayed clear; clearing here would
                // drop a concurrent edit's presence (seen: replace + failing insert in one call).
                self.edits.remove(&m.req);
                tx.send(Err(EditError::from_helper(&err)));
            }
        }
    }
}

/// `ask` → `Result<EditOutcome, String>`: revert the last agent edit of `doc` as the owner (§5.5);
/// one level, forgotten once used or when the document unloads.
#[derive(Clone, Debug)]
pub struct UndoLastAgentEdit {
    /// Document.
    pub doc: DocId,
}

impl Message<UndoLastAgentEdit> for DocumentActor {
    type Reply = DelegatedReply<Result<EditOutcome, String>>;

    async fn handle(
        &mut self,
        m: UndoLastAgentEdit,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        let last = self
            .open
            .get_mut(&m.doc)
            .and_then(|o| o.last_agent_edit.take());
        let Some(last) = last else {
            tx.send(Err("no agent edit to undo".into()));
            return reply;
        };
        tracing::debug!(doc = %m.doc, session = %last.session, "undoing the last agent edit");
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let r = me
                .ask(Edit {
                    doc: m.doc,
                    edits: last.undo,
                    by: Principal::Owner { device: None },
                })
                .await
                .map_err(|e| e.to_string());
            tx.send(r);
        });
        reply
    }
}

/// `tell` (internal): drop the agent's awareness state; `generation` guards against a newer edit's presence.
#[derive(Clone, Debug)]
struct ClearAgentPresence {
    doc: DocId,
    generation: Option<u64>,
}

impl Message<ClearAgentPresence> for DocumentActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: ClearAgentPresence,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(g) = m.generation
            && self.open.get(&m.doc).is_none_or(|o| o.agent_gen != g)
        {
            return;
        }
        self.clear_agent_presence(&m.doc);
    }
}

/// `ask` → `Result<helper::OutlineResult, String>`: the §5.1 outline of `doc`.
#[derive(Clone, Debug)]
pub struct Outline {
    /// Document.
    pub doc: DocId,
    /// Nesting limit.
    pub depth: Option<u32>,
}

impl Message<Outline> for DocumentActor {
    type Reply = DelegatedReply<Result<helper::OutlineResult, String>>;

    async fn handle(&mut self, m: Outline, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.load(&m.doc) {
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
            // 218/229: no blocks, no helper.
            Ok(o) if o.meta.kind == DocKind::Whiteboard => {
                tx.send(Err(crate::wb::DOC_REFUSAL.into()));
                return reply;
            }
            Ok(o) if o.meta.kind == DocKind::Deck => {
                tx.send(Err(crate::deck::DOC_REFUSAL.into()));
                return reply;
            }
            Ok(_) => {}
        }
        let helper = self.helper.clone();
        let authors = self.author_labels(&m.doc);
        tokio::spawn(async move {
            tx.send(
                helper
                    .outline(&m.doc, m.depth)
                    .await
                    .map(|mut r| {
                        if !authors.is_empty() {
                            r.outline = label_outline(&r.outline, &authors);
                        }
                        r
                    })
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
}

/// Outline lines (`<indent><id> …`) of blocks in `authors` get their label appended (§11).
fn label_outline(outline: &str, authors: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(outline.len() + authors.len() * 48);
    for line in outline.lines() {
        out.push_str(line);
        if let Some(id) = line.trim_start().split(' ').next()
            && let Some(label) = authors.get(id)
        {
            out.push_str("  [by ");
            out.push_str(label);
            out.push(']');
        }
        out.push('\n');
    }
    out.pop();
    out
}

/// `ask` → `Result<Option<DocHeadings>, String>`: the kind of `doc` and its top-level headings in
/// order; `None` when no such document exists. What a task packet's mini-board is checked against
/// (101 R2) — an `Outline` at depth 1 reduced to its `[hN]` lines.
#[derive(Clone, Debug)]
pub struct Headings(pub DocId);

/// A document's kind and its top-level headings as `(level, text)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocHeadings {
    /// `kind` of the document.
    pub kind: DocKind,
    /// `(level, text)` of every top-level heading block, in order.
    pub headings: Vec<(u8, String)>,
}

impl Message<Headings> for DocumentActor {
    type Reply = DelegatedReply<Result<Option<DocHeadings>, String>>;

    async fn handle(&mut self, m: Headings, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if !self.open.contains_key(&m.0) && self.store.read_meta(&m.0).is_err() {
            tx.send(Ok(None));
            return reply;
        }
        let kind = match self.load(&m.0) {
            Ok(o) => o.meta.kind,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        // 218/229: pure-Yjs docs have no headings and no helper mirror.
        if kind.is_pure_yjs() {
            tx.send(Ok(Some(DocHeadings {
                kind,
                headings: Vec::new(),
            })));
            return reply;
        }
        let helper = self.helper.clone();
        tokio::spawn(async move {
            tx.send(
                helper
                    .outline(&m.0, Some(1))
                    .await
                    .map(|r| {
                        Some(DocHeadings {
                            kind,
                            headings: headings(&r.blocks),
                        })
                    })
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
}

/// The `(level, text)` of an outline tree's top-level heading blocks (`text` is `[hN] <text>`).
fn headings(blocks: &[serde_json::Value]) -> Vec<(u8, String)> {
    blocks
        .iter()
        .filter(|b| b["type"] == "heading")
        .filter_map(|b| {
            let (level, text) = b["text"].as_str()?.strip_prefix("[h")?.split_once(']')?;
            Some((level.parse().ok()?, text.trim().to_owned()))
        })
        .collect()
}

/// `ask` → `Result<DocMeta, String>`: the metadata of `doc` for `by`, members admitted like [`OpenDoc`].
#[derive(Clone, Debug)]
pub struct MetaOf {
    /// Document.
    pub doc: DocId,
    /// Who asks.
    pub by: Principal,
}

impl Message<MetaOf> for DocumentActor {
    type Reply = Result<DocMeta, String>;

    async fn handle(&mut self, m: MetaOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.admit(&m.doc, &m.by, false).await?;
        Ok(self.load(&m.doc)?.meta.clone())
    }
}

/// `ask` → `Result<String, String>`: lossy markdown of `doc` or of `blocks`, each block preceded by `<!-- id -->` when `annotate`.
#[derive(Clone, Debug)]
pub struct Markdown {
    /// Document.
    pub doc: DocId,
    /// Some blocks only (any depth).
    pub blocks: Option<Vec<String>>,
    /// Prefix every block with its id (§5.1).
    pub annotate: bool,
}

impl Message<Markdown> for DocumentActor {
    type Reply = DelegatedReply<Result<String, String>>;

    async fn handle(&mut self, m: Markdown, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.load(&m.doc) {
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
            // 218/229: refused cleanly, never a helper call.
            Ok(o) if o.meta.kind == DocKind::Whiteboard => {
                tx.send(Err(crate::wb::DOC_REFUSAL.into()));
                return reply;
            }
            Ok(o) if o.meta.kind == DocKind::Deck => {
                tx.send(Err(crate::deck::DOC_REFUSAL.into()));
                return reply;
            }
            Ok(_) => {}
        }
        let authors = if m.annotate {
            self.author_labels(&m.doc)
        } else {
            HashMap::new()
        };
        let helper = self.helper.clone();
        tokio::spawn(async move {
            tx.send(
                helper
                    .markdown_of(&m.doc, m.blocks.as_deref(), m.annotate, &authors)
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
}

/// 184 §5.1: where a new document lives. `Here` = this engine (the caller's), `Hub` = unhomed
/// (served by the lease holder), `Engine(e)` = `e` (a task thread whose task lives on `e`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Homing {
    /// The creating engine.
    #[default]
    Here,
    /// No home: hub-served, as before 184.
    Hub,
    /// That engine (a task thread whose task lives there).
    Engine(mesh::EngineId),
}

/// `ask` → `Result<DocSummary, String>`: a new `kind: note` document (§5.3) with derived file
/// `documents/<id>/note.md`, its body imported from `markdown` and stamped `by`. A body with no
/// level-1 heading gets `# <title>` as its first block so the title is visible in the editor (§7.3).
#[derive(Clone, Debug)]
pub struct CreateNote {
    /// Display title.
    pub title: String,
    /// Initial body; empty → the heading alone.
    pub markdown: String,
    /// Who created it.
    pub by: Principal,
    /// 184 §5.1: where it lives.
    pub home: Homing,
}

/// `ask` → `Result<DocSummary, String>`: a new document bound to `binding` (a meeting's note, §7.4),
/// headed `# <title>` like a note; fails when the binding is already bound. Notes use [`CreateNote`].
#[derive(Clone, Debug)]
pub struct Create {
    /// What it is bound to.
    pub binding: Binding,
    /// Display title.
    pub title: String,
    /// Initial body; empty → the heading alone.
    pub markdown: String,
    /// Who created it.
    pub by: Principal,
    /// 184 §5.1: where it lives.
    pub home: Homing,
}

/// `markdown`, headed by `# title` unless it already has a level-1 heading.
pub fn with_heading(title: &str, markdown: &str) -> String {
    if markdown.lines().any(|l| l.starts_with("# ")) {
        return markdown.to_owned();
    }
    let body = markdown.trim_start_matches('\n');
    if body.is_empty() {
        format!("# {title}\n")
    } else {
        format!("# {title}\n\n{body}")
    }
}

/// `ask` → `Result<DocSummary, String>`: a new `kind: board` document (71 §2.1) — a note whose first
/// block is the frontmatter `root: <root>`, the absolute repository root its anchors resolve against.
#[derive(Clone, Debug)]
pub struct CreateBoard {
    /// Display title.
    pub title: String,
    /// Initial body; empty → the heading alone.
    pub markdown: String,
    /// Absolute repository root; the caller has checked it is a directory.
    pub root: PathBuf,
    /// Who created it.
    pub by: Principal,
}

/// The YAML header for a board's frontmatter block: `root:` as a plain scalar, double-quoted (JSON
/// escaping, which the helper's YAML reader accepts) when the path carries a character YAML would
/// misread.
pub fn board_frontmatter(root: &Path) -> String {
    let root = root.to_string_lossy();
    let plain = !root.starts_with([
        ' ', '-', '?', '[', '{', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
    ]) && !root.contains(['"', '\\', '\n', '\r', '\t', '#'])
        && !root.contains(": ")
        && !root.ends_with([':', ' ']);
    let value = if plain {
        root.into_owned()
    } else {
        serde_json::to_string(&*root).unwrap_or_default()
    };
    format!("---\nroot: {value}\n---\n")
}

impl Message<CreateBoard> for DocumentActor {
    type Reply = DelegatedReply<Result<DocSummary, String>>;

    async fn handle(
        &mut self,
        m: CreateBoard,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // 184 §5.1: a board's `root:` is this engine's disk, so it is always `Homing::Here`.
        let home = self.resolve_home(&Homing::Here, DocKind::Board);
        if let Err(e) = self.route_home(home.as_ref()) {
            tx.send(Err(e.into()));
            return reply;
        }
        self.create_doc(
            Binding::Note,
            Some(&m.root),
            m.title,
            m.markdown,
            m.by,
            home,
            |s| ImportReply::CreatedNote(s, tx),
        )
        .await;
        reply
    }
}

impl Message<CreateNote> for DocumentActor {
    type Reply = DelegatedReply<Result<DocSummary, String>>;

    async fn handle(&mut self, m: CreateNote, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.create_routed(Binding::Note, m.title, m.markdown, m.by, m.home, tx)
            .await;
        reply
    }
}

impl Message<Create> for DocumentActor {
    type Reply = DelegatedReply<Result<DocSummary, String>>;

    async fn handle(&mut self, m: Create, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.create_routed(m.binding, m.title, m.markdown, m.by, m.home, tx)
            .await;
        reply
    }
}

impl DocumentActor {
    /// 184 §5.2: create on the would-be server — here, or over `Cmd::Create` (the server runs it
    /// as `Relayed`, with `home` resolved so `Here` keeps meaning this caller).
    async fn create_routed(
        &mut self,
        binding: Binding,
        title: String,
        markdown: String,
        by: Principal,
        home: Homing,
        tx: ReplySender<Result<DocSummary, String>>,
    ) {
        let home = self.resolve_home(&home, binding.kind());
        match self.route_home(home.as_ref()) {
            Ok(None) => {
                // W-f: a relayed keyed create answers the document an earlier attempt made.
                if self.relaying
                    && !binding.keyless()
                    && let Ok(Some(id)) = self.store.lookup(&binding).await
                    && let Ok(meta) = self.store.read_meta(&id)
                {
                    tx.send(Ok(DocSummary::from(&meta)));
                    return;
                }
                self.create_doc(binding, None, title, markdown, by, home, |s| {
                    ImportReply::CreatedNote(s, tx)
                })
                .await;
            }
            Ok(Some(to)) => {
                let id = DocId::for_binding(&binding);
                let client = Client::of(&by);
                let req = CreateReq::Create {
                    binding,
                    title,
                    markdown,
                    by,
                    home: home.clone(),
                };
                let ask = self.create_on::<DocSummary>(to, id, client, req, home);
                tokio::spawn(async move { tx.send(ask.await) });
            }
            Err(e) => tx.send(Err(e.into())),
        }
    }

    /// Meta + index row, `DocumentChanged`, then the headed body as one `replace` import. `board_root`
    /// makes the document a `DocKind::Board` headed by [`board_frontmatter`] (71 §2.1). `home`
    /// (184 §5.2): this engine serves it from the start, fence epoch 1.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_doc(
        &mut self,
        binding: Binding,
        board_root: Option<&Path>,
        title: String,
        markdown: String,
        by: Principal,
        home: Option<mesh::EngineId>,
        reply: impl FnOnce(DocSummary) -> ImportReply,
    ) -> DocId {
        let id = DocId::for_binding(&binding);
        let now = now_ms();
        let homed = home.is_some();
        let meta = DocMeta {
            derived: binding.derived(&id),
            id: id.clone(),
            kind: if board_root.is_some() {
                DocKind::Board
            } else {
                binding.kind()
            },
            title,
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
        let reply = reply(DocSummary::from(&meta));
        if let Err(e) = self.store.create(&meta).await {
            finish_reply(reply, Err(format!("{e:#}")));
            return id;
        }
        if homed {
            self.claims
                .insert(id.clone(), crate::hub::Claim::Held { epoch: 1 });
        }
        self.homes.insert(id.clone(), meta.home.clone());
        self.rt.publish(DocumentChanged::of(&meta, false)).await;
        let mut markdown = with_heading(&meta.title, &markdown);
        if let Some(root) = board_root {
            markdown.insert_str(0, &board_frontmatter(root));
        }
        self.start_import(&id, markdown, by, ImportMode::Replace, reply);
        id
    }
}

/// `ask` → `Result<(), String>`: retitle a note (§7.3): meta + index, and the first block when it is
/// a level-1 heading (one owner transaction). Other kinds: `title_owned_by_binding` — the knowledge
/// title lives in the frontmatter, the artifact's in the artifact.
#[derive(Clone, Debug)]
pub struct Rename {
    /// Document.
    pub doc: DocId,
    /// New title.
    pub title: String,
}

impl Message<Rename> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: Rename, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        let title = m.title.trim().to_owned();
        if title.is_empty() {
            tx.send(Err("title must not be empty".into()));
            return reply;
        }
        let meta = match self.load(&m.doc) {
            // 144 §3.2: a notebook's title is its name — renamed like a note.
            Ok(o) if !(o.meta.kind.free_standing() || o.meta.kind == DocKind::Notebook) => {
                tx.send(Err("title_owned_by_binding".into()));
                return reply;
            }
            Ok(o) => {
                o.meta.title = title.clone();
                o.meta.clone()
            }
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        if let Err(e) = self.store.write_meta(&meta).await {
            tx.send(Err(format!("{e:#}")));
            return reply;
        }
        self.fan_out(
            &m.doc,
            DocEvent::Meta {
                title: title.clone(),
            },
            None,
        );
        self.rt.publish(DocumentChanged::of(&meta, false)).await;
        // 218/229: pure-Yjs titles live in meta only; derived markdown follows at export.
        if meta.kind.is_pure_yjs() {
            self.schedule_export(&m.doc);
            tx.send(Ok(()));
            return reply;
        }
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move { tx.send(retitle_heading(me, m.doc, title).await) });
        reply
    }
}

/// The second half of a rename (§7.3): when the document's first block is a level-1 heading, replace
/// it with `# <title>` as the owner. Runs outside the actor (it asks it twice).
pub(crate) async fn retitle_heading(
    me: ActorRef<DocumentActor>,
    doc: DocId,
    title: String,
) -> Result<(), String> {
    let outline = me
        .ask(Outline {
            doc: doc.clone(),
            depth: Some(1),
        })
        .await
        .map_err(|e| e.to_string())?;
    // A board's first block is its frontmatter (`root:`); the heading follows it.
    let Some(first) = outline.blocks.iter().find(|b| b["type"] != "frontmatter") else {
        return Ok(());
    };
    let is_h1 = first["type"] == "heading"
        && first["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("[h1] "));
    let Some(id) = first["id"].as_str().filter(|_| is_h1) else {
        return Ok(());
    };
    me.ask(Edit {
        doc,
        edits: vec![BlockEdit::Replace {
            blocks: vec![id.to_owned()],
            markdown: format!("# {title}"),
        }],
        by: Principal::Owner { device: None },
    })
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// `ask` → `Result<DocSummary, String>`: file a note or board under a project (`None` = ungrouped) —
/// the `docs.project` column. Written on any engine that has the row: the replica merges it per
/// column (LWW) like `notebooks.project`, so no hub or home is needed. Other kinds:
/// `project_owned_by_binding`. The label arrives normalized.
#[derive(Clone, Debug)]
pub struct SetProject {
    /// Document.
    pub doc: DocId,
    /// Project label; `None` clears it.
    pub project: Option<String>,
}

impl Message<SetProject> for DocumentActor {
    type Reply = Result<DocSummary, String>;

    async fn handle(&mut self, m: SetProject, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let err = |e: anyhow::Error| format!("{e:#}");
        let Some(row) = self.store.summary(&m.doc).await.map_err(err)? else {
            return Err(format!("no document {}", m.doc));
        };
        if !row.kind.free_standing() {
            return Err("project_owned_by_binding".into());
        }
        if row.project == m.project {
            return Ok(row);
        }
        self.store
            .set_project(&m.doc, m.project.as_deref())
            .await
            .map_err(err)?;
        // The notes list refreshes on `DocumentChanged`; nothing in the document itself changed.
        let changed = DocumentChanged {
            doc: row.id.clone(),
            kind: row.kind,
            title: row.title.clone(),
            updated_ms: row.updated_ms,
            deleted: false,
        };
        self.rt.publish(changed).await;
        Ok(DocSummary {
            project: m.project,
            ..row
        })
    }
}

/// `ask` → `Result<(), String>`: delete a note (§7.3 UI). Other kinds answer `delete_via_owner`
/// (`knowledge_delete`, the artifact store); their owners use [`Delete`].
#[derive(Clone, Debug)]
pub struct DeleteNote {
    /// Document.
    pub doc: DocId,
}

impl Message<DeleteNote> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: DeleteNote, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        let r = async {
            let kind = match self.open.get(&m.doc) {
                Some(o) => o.meta.kind,
                None => {
                    self.store
                        .read_meta(&m.doc)
                        .map_err(|e| format!("{e:#}"))?
                        .kind
                }
            };
            if !kind.free_standing() {
                return Err("delete_via_owner".into());
            }
            self.delete_doc(&m.doc).await
        }
        .await;
        tx.send(r);
        reply
    }
}

/// `ask` → `Result<Vec<DocSummary>, String>`.
#[derive(Clone, Debug, Default)]
pub struct ListDocs {
    /// Filter.
    pub kind: Option<DocKind>,
    /// Default 100.
    pub limit: Option<u32>,
}

impl Message<ListDocs> for DocumentActor {
    type Reply = Result<Vec<DocSummary>, String>;

    async fn handle(&mut self, m: ListDocs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .list(m.kind, m.limit.unwrap_or(100))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → [`store::DocStats`]: the mirror merge counters (69 E2), served anywhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats;

impl Message<Stats> for DocumentActor {
    type Reply = store::DocStats;

    async fn handle(&mut self, _: Stats, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.stats()
    }
}

/// `ask` → `Result<(), String>`: a labelled snapshot (§4.6).
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Document.
    pub doc: DocId,
    /// Label (file stem suffix).
    pub label: String,
}

impl Message<Snapshot> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: Snapshot, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(
            self.load(&m.doc)
                .map(|_| ())
                .and_then(|()| self.snapshot(&m.doc, Some(&m.label))),
        );
        reply
    }
}

/// `ask` → `Result<(), String>`: move a document's derived file (§7.4: a meeting's `notes.md` becomes
/// `summary.md` once the summary is in it): meta + index, the old file removed, the new path written
/// before the reply so its readers find it at once.
#[derive(Clone, Debug)]
pub struct SetDerived {
    /// Document.
    pub doc: DocId,
    /// New derived path relative to the data dir.
    pub derived: String,
}

impl Message<SetDerived> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: SetDerived, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        let o = match self.load(&m.doc) {
            Ok(o) => o,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        let old = std::mem::replace(&mut o.meta.derived, m.derived);
        let meta = o.meta.clone();
        if let Err(e) = self.store.write_meta(&meta).await {
            tx.send(Err(format!("meta.json: {e:#}")));
            return reply;
        }
        if old != meta.derived && !old.is_empty() {
            let _ = std::fs::remove_file(self.data_dir.join(&old));
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        self.export(&m.doc, Some(done_tx));
        tokio::spawn(async move {
            tx.send(match done_rx.await {
                Ok(r) => r,
                Err(_) => Err("export task dropped".into()),
            });
        });
        reply
    }
}

/// `ask` → `Result<bool, String>`: hand a document to a new owner — `kind`, `binding` and `derived`
/// follow `binding` (the task threads minted as notes before TASK-NOTES-HIDDEN). `Ok(false)` when it
/// was already bound so; fails when `binding` is bound to another document.
#[derive(Clone, Debug)]
pub struct Bind {
    /// Document.
    pub doc: DocId,
    /// Its new owner.
    pub binding: Binding,
}

impl Message<Bind> for DocumentActor {
    type Reply = DelegatedReply<Result<bool, String>>;

    async fn handle(&mut self, m: Bind, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        let r = async {
            if !self
                .store
                .bind(&m.doc, &m.binding)
                .await
                .map_err(|e| format!("{e:#}"))?
            {
                return Ok(false);
            }
            let meta = self.store.read_meta(&m.doc).map_err(|e| format!("{e:#}"))?;
            if let Some(o) = self.open.get_mut(&m.doc) {
                o.meta.kind = meta.kind;
                o.meta.binding = meta.binding.clone();
                o.meta.derived = meta.derived.clone();
            }
            self.rt.publish(DocumentChanged::of(&meta, false)).await;
            Ok(true)
        }
        .await;
        tx.send(r);
        reply
    }
}

/// `ask` → `Result<Vec<SnapshotInfo>, String>`.
#[derive(Clone, Debug)]
pub struct Snapshots {
    /// Document.
    pub doc: DocId,
}

impl Message<Snapshots> for DocumentActor {
    type Reply = Result<Vec<SnapshotInfo>, String>;

    async fn handle(&mut self, m: Snapshots, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store.snapshots(&m.doc).map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → `Result<ImportOutcome, String>`: import a kept snapshot's markdown as one diff transaction (§4.6: not a rewind).
#[derive(Clone, Debug)]
pub struct Restore {
    /// Document.
    pub doc: DocId,
    /// `SnapshotInfo.name`.
    pub snapshot: String,
}

impl Message<Restore> for DocumentActor {
    type Reply = DelegatedReply<Result<ImportOutcome, String>>;

    async fn handle(&mut self, m: Restore, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        let kind = match self.open.get(&m.doc) {
            Some(o) => o.meta.kind,
            None => match self.store.read_meta(&m.doc) {
                Ok(meta) => meta.kind,
                Err(e) => {
                    tx.send(Err(format!("{e:#}")));
                    return reply;
                }
            },
        };
        if kind == DocKind::Whiteboard {
            tx.send(Err(
                "restoring a snapshot is not available for whiteboards; use undo on the canvas"
                    .into(),
            ));
            return reply;
        }
        if kind == DocKind::Deck {
            tx.send(Err(
                "restoring a whole-document snapshot is not available for Decks; inspect per-section history instead"
                    .into(),
            ));
            return reply;
        }
        let bytes = match self.store.read_snapshot(&m.doc, &m.snapshot) {
            Ok(b) => b,
            Err(e) => {
                tx.send(Err(format!("{e:#}")));
                return reply;
            }
        };
        let fragment = match self.load(&m.doc) {
            Ok(o) => o.meta.fragment.clone(),
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        // The snapshot's markdown comes from a throwaway mirror in the helper. The import is the
        // owner's (not `File`): a restore changes the document, so the derived file must follow it.
        let helper = self.helper.clone();
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let tmp = DocId::new();
            helper.open(&tmp, &bytes, &fragment).log("open snapshot");
            let md = helper.markdown(&tmp).await;
            helper.close(&tmp).log("close snapshot");
            let r = match md {
                Ok(markdown) => me
                    .ask(ImportMarkdown {
                        doc: m.doc,
                        markdown,
                        by: Principal::Owner { device: None },
                        mode: ImportMode::Diff,
                    })
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            tx.send(r);
        });
        reply
    }
}

/// `ask` → `Result<(), String>`: an owner (knowledge, artifacts) drops its document: close, move the
/// directory to `.trash/<id>`, mark deleted (§7.1). The UI deletes notes through [`DeleteNote`].
#[derive(Clone, Debug)]
pub struct Delete {
    /// Document.
    pub doc: DocId,
}

impl Message<Delete> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, m: Delete, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.delete_doc(&m.doc).await);
        reply
    }
}

/// `tell`: a derived file changed on disk (the watcher, §4.4).
#[derive(Clone, Debug)]
pub struct FileChanged {
    /// The binding the path maps to.
    pub binding: Binding,
    /// Absolute path.
    pub path: PathBuf,
}

impl Message<FileChanged> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: FileChanged, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = match self.store.lookup(&m.binding).await {
            Ok(Some(id)) => id,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "documents lookup failed");
                return;
            }
        };
        // 184 §4.6: the watcher runs on the holder; the import follows the document's server.
        if !self.serves(&id) {
            return;
        }
        let Ok(Some(bytes)) = store::read_opt(&m.path) else {
            return;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return;
        };
        // Cheap pre-check for unloaded documents: meta.json remembers our last export.
        if !self.open.contains_key(&id)
            && let Ok(meta) = self.store.read_meta(&id)
            && meta.last_export.as_deref() == Some(sha256(text.as_bytes()).as_str())
        {
            return;
        }
        self.start_import(
            &id,
            text,
            Principal::File,
            ImportMode::Diff,
            ImportReply::None,
        );
    }
}

/// `tell`: a file in `documents/mirror/` changed (the mirror watcher, 69): a peer's projection to
/// merge, or this process's own write to skip.
#[derive(Clone, Debug)]
pub struct MirrorChanged {
    /// Absolute path of the mirror file.
    pub path: PathBuf,
}

impl Message<MirrorChanged> for DocumentActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: MirrorChanged,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let Some((id, kind)) = crate::mirror::parse(&m.path) else {
            return;
        };
        // A loaded document this engine does not serve is a read cache (`doc_read`): drop it and
        // merge on disk. Closed documents merge on disk; a document a peer removed follows its
        // tombstone into `.trash`. 184 §4.6: a served one (a home's) merges live.
        if !self.serves(&id) && self.open.contains_key(&id) {
            self.unload(&id, "unloaded");
        }
        if !self.open.contains_key(&id) {
            let before = self.home_peek(&id);
            // 213 §4: a mirrored `meta.json` that changes the members or links is a sharing change.
            let sharing = |m: &DocMeta| (m.members.clone(), m.links.clone());
            let shared_before = match kind {
                crate::mirror::MirrorFile::Meta => {
                    self.store.read_meta(&id).ok().map(|m| sharing(&m))
                }
                crate::mirror::MirrorFile::Snapshot => None,
            };
            match self.store.mirror_apply(&m.path).await {
                Ok(store::Applied::Created(_)) | Ok(store::Applied::Merged(_)) => {
                    self.pending_homes.remove(&id);
                    self.homes.remove(&id);
                    if let Ok(meta) = self.store.read_meta(&id) {
                        self.rt.publish(DocumentChanged::of(&meta, false)).await;
                        let now = sharing(&meta);
                        let changed = match &shared_before {
                            Some(b) => *b != now,
                            None => !now.0.is_empty() || !now.1.is_empty(),
                        };
                        if changed {
                            self.rt
                                .publish(crate::meta::DocumentSharing { doc: id.clone() })
                                .await;
                        }
                    }
                    // §4.5: a moved home moves the legs.
                    if self.home_peek(&id) != before {
                        self.reroute();
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(doc = %id, error = %e, "documents: mirror file not merged")
                }
            }
            return;
        }
        let Ok(Some(bytes)) = store::read_opt(&m.path) else {
            // W-s: a served document whose files a peer's delete removed is deleted here too, not
            // re-snapshotted back into the mesh.
            let gone: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM docs WHERE id = ? AND deleted_ms IS NOT NULL")
                    .bind(id.as_str())
                    .fetch_optional(self.store.pool())
                    .await
                    .unwrap_or(None);
            if gone.is_some() {
                tracing::info!(doc = %id, "documents: deleted elsewhere while served here; deleting");
                if let Err(e) = self.delete_doc(&id).await {
                    tracing::warn!(doc = %id, error = %e, "documents: delete after a peer's delete failed");
                }
            }
            return;
        };
        if self
            .store
            .is_own_projection(&crate::mirror::file_name(&id, kind), &bytes)
        {
            return;
        }
        match kind {
            crate::mirror::MirrorFile::Snapshot => {
                let Some(o) = self.open.get(&id) else { return };
                let theirs = match store::decode_state(&bytes) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(doc = %id, error = %e, "documents: foreign snapshot undecodable");
                        return;
                    }
                };
                // Only news goes through `apply` (log, fan-out, export, then a re-snapshot that ships):
                // the diff below is never empty once the document holds a deletion, so its length says
                // nothing (69 E2).
                if !store::exceeds(&theirs, &o.doc) {
                    store::Counters::bump(&self.store.counters.unchanged);
                    store::Counters::bump(&self.store.counters.noop_applies_avoided);
                    return;
                }
                let diff = theirs
                    .transact()
                    .encode_state_as_update_v1(&o.doc.transact().state_vector());
                // Into the live document like any update (log, fan-out, helper mirror); the derived
                // file follows, which `Principal::File` alone would not schedule.
                if let Err(e) = self
                    .apply(&id, diff, Principal::File, false, Vec::new())
                    .await
                {
                    tracing::warn!(doc = %id, error = %e, "documents: foreign snapshot not merged");
                    return;
                }
                store::Counters::bump(&self.store.counters.merged);
                self.schedule_export(&id);
                tracing::info!(doc = %id, "document snapshot merged from the mesh (open)");
            }
            crate::mirror::MirrorFile::Meta => {
                let incoming: DocMeta = match self.store.parse_meta(&bytes) {
                    Ok((m, _)) => m,
                    Err(e) => {
                        tracing::warn!(doc = %id, error = %e, "documents: foreign meta.json undecodable");
                        return;
                    }
                };
                let Some(o) = self.open.get_mut(&id) else {
                    return;
                };
                // §7.4: the seal that opens here wins.
                let vault = self.store.vault();
                let opens =
                    |l: &crate::meta::ShareLink| crate::share::seal_opens(vault.as_deref(), l);
                let (home, fence, shared_before) = (
                    o.meta.home.clone(),
                    (o.meta.serve_epoch, o.meta.served_by.clone()),
                    (o.meta.members.clone(), o.meta.links.clone()),
                );
                if !crate::mirror::merge_meta(&mut o.meta, &incoming, &opens) {
                    return;
                }
                let home_moved = o.meta.home != home;
                let fence_moved = (o.meta.serve_epoch, o.meta.served_by.clone()) != fence;
                let sharing_moved =
                    (&o.meta.members, &o.meta.links) != (&shared_before.0, &shared_before.1);
                let (meta, title) = (o.meta.clone(), o.meta.title.clone());
                if let Err(e) = self.store.write_meta(&meta).await {
                    tracing::warn!(doc = %id, error = %e, "meta.json: merge not saved");
                }
                let (members, links) = (
                    meta.members.clone(),
                    meta.links
                        .iter()
                        .map(crate::meta::ShareLink::public)
                        .collect(),
                );
                self.fan_out(&id, DocEvent::Members { members, links }, None);
                self.fan_out(&id, DocEvent::Meta { title }, None);
                self.rt.publish(DocumentChanged::of(&meta, false)).await;
                if sharing_moved {
                    self.rt
                        .publish(crate::meta::DocumentSharing { doc: id.clone() })
                        .await;
                }
                // §4.4: a beaten fence names a second server; §4.5: a moved home reroutes.
                if fence_moved {
                    self.fence_beaten(&id);
                }
                if home_moved {
                    self.reroute();
                }
            }
        }
    }
}

impl Message<HelperEvent> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: HelperEvent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match m {
            HelperEvent::Restarted => {
                let ids: Vec<DocId> = self.open.keys().cloned().collect();
                for id in ids {
                    let o = &self.open[&id];
                    if o.meta.kind.is_pure_yjs() {
                        continue;
                    }
                    let state = o
                        .doc
                        .transact()
                        .encode_state_as_update_v1(&StateVector::default());
                    self.helper
                        .open(&id, &state, &o.meta.fragment)
                        .log("reopen");
                }
            }
            HelperEvent::Update {
                doc,
                update,
                touched,
                req,
                ratio,
            } => {
                let mut done = false;
                let mut first = false;
                let (by, snapshot_first) = match (
                    self.imports.get_mut(&req),
                    self.edits.get_mut(&req),
                ) {
                    (Some(imp), _) => {
                        imp.applied = true;
                        done = imp.done_at.is_some();
                        first = done && imp.first;
                        (
                            imp.by.clone(),
                            imp.mode == ImportMode::Diff
                                && ratio.is_some_and(|(c, t)| t > 0 && c * 2 > t),
                        )
                    }
                    (None, Some(e)) => {
                        e.applied = true;
                        done = e.reply.is_none();
                        (e.by.clone(), false)
                    }
                    (None, None) => {
                        tracing::warn!(doc = %doc, req, "helper update matches no import or edit; stamped Helper");
                        (Principal::Helper, false)
                    }
                };
                if done {
                    self.imports.remove(&req);
                    self.edits.remove(&req);
                }
                if snapshot_first && let Err(e) = self.snapshot(&doc, Some("import-file")) {
                    tracing::warn!(doc = %doc, error = %e, "import-file snapshot failed");
                }
                if matches!(by, Principal::Agent { .. }) && !touched.is_empty() {
                    self.set_agent_presence(&doc, &touched);
                }
                if let Err(e) = self.apply(&doc, update, by, true, touched).await {
                    tracing::warn!(doc = %doc, error = %e, "helper update refused");
                } else if first && let Err(e) = self.snapshot(&doc, None) {
                    // 184 §5.3: the create's reply came first; its update now snapshots.
                    tracing::warn!(doc = %doc, error = %e, "documents: first snapshot failed");
                }
            }
        }
    }
}

/// `tell` every 250 ms: exports due, owner notifications past their quiet period, snapshot policy, idle unload,
/// hourly trash sweep (§4.1, §4.3, §4.7, §7.3).
#[derive(Clone, Copy, Debug, Default)]
pub struct Tick;

impl Message<Tick> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, _: Tick, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let now = Instant::now();
        let mut exports = Vec::new();
        let mut snapshots = Vec::new();
        let mut unloads = Vec::new();
        let mut notifies = Vec::new();
        for (id, o) in &self.open {
            if !o.exporting
                && let Some(due) = o.export_due
                && (now >= due || o.export_first.is_some_and(|f| now >= f + EXPORT_CAP))
            {
                exports.push(id.clone());
            }
            if self.owner_due(id, o, now) {
                notifies.push(id.clone());
            }
            // §4.1 size/count policy, plus 69: a quiet period, so the replicated mirror lags by seconds.
            if o.log_bytes > LOG_MAX_BYTES
                || o.log_count > LOG_MAX_COUNT
                // 184 §4.6: the serving engine ships the mirror.
                || (self.serves(id)
                    && o.log_count > 0
                    && now.duration_since(o.idle_since)
                        >= Duration::from_millis(self.cfg.snapshot_quiet_ms))
            {
                snapshots.push(id.clone());
            }
            if !o.exporting
                && o.export_due.is_none()
                && !self.subs.contains_key(id)
                && now.duration_since(o.idle_since) > Duration::from_secs(self.cfg.idle_unload_secs)
            {
                unloads.push(id.clone());
            }
        }
        for id in exports {
            self.export(&id, None);
        }
        for id in notifies {
            self.notify_owner(&id).await;
        }
        for id in snapshots {
            if let Err(e) = self.snapshot(&id, None) {
                tracing::warn!(doc = %id, error = %e, "policy snapshot failed");
            }
        }
        for id in unloads {
            self.unload(&id, "unloaded");
        }
        // §11 authorship (and 184's fence) coalesced to one `meta.json` (+ `agents.json`) write per tick.
        let dirty: Vec<_> = self
            .open
            .values_mut()
            .filter_map(|o| {
                let agents = std::mem::take(&mut o.authors_dirty).then(|| o.agents.clone());
                let meta = std::mem::take(&mut o.meta_dirty);
                (agents.is_some() || meta).then(|| (o.meta.clone(), agents))
            })
            .collect();
        for (meta, agents) in dirty {
            if let Some(agents) = agents
                && let Err(e) = self.store.write_agents(&meta.id, &agents)
            {
                tracing::warn!(doc = %meta.id, error = %e, "agents.json: not saved");
            }
            if let Err(e) = self.store.write_meta(&meta).await {
                tracing::warn!(doc = %meta.id, error = %e, "meta.json: authors not saved");
            }
        }
        // 184 §8.1: the docs gate every 30 s besides every mesh event; §5.3: forwarded creates'
        // homes expire.
        if self
            .last_gate_poll
            .is_none_or(|t| now.duration_since(t) >= GATE_POLL)
        {
            self.last_gate_poll = Some(now);
            self.poll_gate();
            self.pending_homes
                .retain(|_, (_, at)| at.elapsed() < crate::hub::PENDING_HOME_TTL);
        }
        // Imports that replied but changed nothing send no update: forget them after a grace period.
        self.imports.retain(|_, i| {
            i.done_at
                .is_none_or(|t| now.duration_since(t) < IMPORT_GRACE)
        });
        // Subscribers whose receiver went away.
        for subs in self.subs.values_mut() {
            subs.retain(|s| !s.tx.is_closed());
        }
        self.subs.retain(|_, s| !s.is_empty());
        if self
            .last_sweep
            .is_none_or(|t| now.duration_since(t) >= TRASH_SWEEP)
        {
            self.last_sweep = Some(now);
            match self.store.sweep_trash(TRASH_KEEP).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "documents trash swept"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "documents trash sweep failed"),
            }
        }
    }
}

/// Persist a selected local knowledge edit before reporting success to its caller.
pub struct ExportNow(pub DocId);
impl Message<ExportNow> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;
    async fn handle(&mut self, m: ExportNow, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            if let Err(e) = self.load(&m.0) {
                tx.send(Err(e));
                return reply;
            }
            let (done, rx) = tokio::sync::oneshot::channel();
            self.export(&m.0, Some(done));
            tokio::spawn(async move {
                tx.send(rx.await.unwrap_or_else(|e| Err(e.to_string())));
            });
        }
        reply
    }
}

/// `ask` → `()` at harness shutdown: every pending derived-file write is fired and every settled edit is
/// reported to its owner now, whatever its quiet period says (§4.3 as built: nothing waits out the process).
#[derive(Clone, Copy, Debug, Default)]
pub struct Flush;

impl Message<Flush> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, _: Flush, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.shutting_down = true;
        let now = Instant::now();
        let waiting: Vec<DocId> = self
            .open
            .iter()
            .filter(|(_, o)| o.notify_due.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in waiting {
            let Some(o) = self.open.get(&id) else {
                continue;
            };
            if o.export_due.is_some() && !o.exporting {
                // Fired now; it reports through `ExportDone` (`shutting_down`), like one already in flight.
                self.export(&id, None);
            } else if self.owner_due(&id, o, now) {
                self.notify_owner(&id).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_of_reads_the_header_or_the_first_line() {
        assert_eq!(
            title_of(
                DocKind::Knowledge,
                "---\ntype: fact\ntitle: \"A: b\"\n---\n# H\n"
            ),
            "A: b"
        );
        assert_eq!(
            title_of(DocKind::Knowledge, "---\ntype: fact\n---\n# H\n\nx"),
            "H"
        );
        assert_eq!(
            title_of(DocKind::Note, "\n\nfirst line here\nsecond"),
            "first line here"
        );
        assert_eq!(title_of(DocKind::Note, ""), "");
    }

    #[test]
    fn with_heading_adds_the_title_once() {
        assert_eq!(with_heading("T", ""), "# T\n");
        assert_eq!(with_heading("T", "\n\nbody\n"), "# T\n\nbody\n");
        assert_eq!(with_heading("T", "# Own\n\nbody\n"), "# Own\n\nbody\n");
        assert_eq!(with_heading("T", "## sub\n"), "# T\n\n## sub\n");
    }

    #[test]
    fn awareness_cap_admits_four_per_second_then_resets() {
        let t0 = Instant::now();
        let mut slot = RateSlot { since: t0, sent: 0 };
        for _ in 0..4 {
            assert!(admit_awareness(&mut slot, t0 + Duration::from_millis(100)));
        }
        assert!(!admit_awareness(&mut slot, t0 + Duration::from_millis(900)));
        assert!(admit_awareness(&mut slot, t0 + Duration::from_millis(1000)));
        assert_eq!(slot.sent, 1);
    }

    #[test]
    fn headings_reads_the_outline_tree() {
        let blocks = serde_json::json!([
            {"id": "fm", "type": "frontmatter", "text": "root=/r", "children": []},
            {"id": "t", "type": "heading", "text": "[h1] Title", "children": []},
            {"id": "p", "type": "paragraph", "text": "prose", "children": []},
            {"id": "a", "type": "heading", "text": "[h2] What you're deciding", "children": []},
            {"id": "b", "type": "heading", "text": "[h3] ", "children": []},
            {"id": "c", "type": "codeBlock", "text": "[code:rust] fn", "children": []}
        ]);
        assert_eq!(
            headings(blocks.as_array().unwrap()),
            vec![
                (1, "Title".to_owned()),
                (2, "What you're deciding".to_owned()),
                (3, String::new())
            ]
        );
    }
}
