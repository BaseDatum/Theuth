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
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yrs::sync::{Awareness, AwarenessUpdate};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, StateVector, Transact, Update};

use crate::helper::{self, Edit as BlockEdit, EditError, Helper, HelperEvent, ImportMode};
use crate::meta::{
    AgentStyle, Binding, DocEvent, DocId, DocKind, DocMeta, DocOpen, DocSummary, DocumentChanged,
    Principal, SnapshotInfo, Who,
};
use crate::store::{self, Store, now_ms, sha256, write_atomic};
use crate::{DocumentsConfig, Exported, FRAGMENT};

/// Snapshot when the log passes this (§4.1).
const LOG_MAX_BYTES: usize = 1024 * 1024;
/// … or this many records.
const LOG_MAX_COUNT: usize = 1000;
/// Unload after this long idle with no subscriber (§4.7).
const IDLE_UNLOAD: Duration = Duration::from_secs(600);
/// At most this many loaded documents (§4.7).
const MAX_OPEN: usize = 256;
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

/// One loaded document.
struct Open {
    doc: yrs::Doc,
    meta: DocMeta,
    awareness: Awareness,
    log_count: usize,
    log_bytes: usize,
    /// Upper bound on the encoded state: snapshot + log bytes (§4.7 limit).
    bytes: usize,
    idle_since: Instant,
    export_due: Option<Instant>,
    export_first: Option<Instant>,
    exporting: bool,
    /// The last agent edit's inverse (§5.5 "Undo last agent edit"); one level, in memory.
    last_agent_edit: Option<LastAgentEdit>,
    /// Bumped per agent presence; a delayed clear only fires for its own generation.
    agent_gen: u64,
    /// Per connection: start of the current one-second window and the states relayed in it (§6.2 cap).
    awareness_rate: HashMap<Option<u64>, RateSlot>,
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
        *slot = RateSlot { since: now, sent: 0 };
    }
    if slot.sent >= AWARENESS_RATE {
        return false;
    }
    slot.sent += 1;
    true
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

struct Subscriber {
    conn: Option<u64>,
    tx: mpsc::Sender<DocEvent>,
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
}

enum ImportReply {
    None,
    Outcome(ReplySender<Result<ImportOutcome, String>>),
    Created(DocId, ReplySender<Result<DocId, String>>),
    CreatedNote(DocSummary, ReplySender<Result<DocSummary, String>>),
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
#[derive(kameo::Actor)]
pub struct DocumentActor {
    store: Store,
    helper: Helper,
    cfg: DocumentsConfig,
    rt: Runtime,
    data_dir: PathBuf,
    open: HashMap<DocId, Open>,
    subs: HashMap<DocId, Vec<Subscriber>>,
    imports: HashMap<u64, Import>,
    edits: HashMap<u64, PendingEdit>,
    last_sweep: Option<Instant>,
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
        }
    }

    /// Load `id` if it is not, and open its mirror in the helper.
    fn load(&mut self, id: &DocId) -> Result<&mut Open, String> {
        if !self.open.contains_key(id) {
            let l = self.store.load(id).map_err(|e| format!("{e:#}"))?;
            let state = l
                .doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            self.helper.open(id, &state, &l.meta.fragment).log("open");
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
                    last_agent_edit: None,
                    agent_gen: 0,
                    awareness_rate: HashMap::new(),
                },
            );
            self.enforce_open_cap();
        }
        Ok(self.open.get_mut(id).expect("inserted"))
    }

    /// Fan an event to every subscriber of `doc` but the one on `skip`; drop the dead and the slow.
    fn fan_out(&mut self, doc: &DocId, ev: DocEvent, skip: Option<u64>) {
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
    async fn apply(
        &mut self,
        doc: &DocId,
        update: Vec<u8>,
        by: Principal,
        from_helper: bool,
        touched: Vec<String>,
    ) -> Result<(), String> {
        let max = self.cfg.max_doc_bytes;
        let store = self.store.clone();
        let o = self.load(doc)?;
        if o.bytes + update.len() > max {
            return Err(format!(
                "DocTooLarge: document would exceed {max} bytes ({} + {})",
                o.bytes,
                update.len()
            ));
        }
        let u = Update::decode_v1(&update).map_err(|e| format!("bad update: {e}"))?;
        o.doc
            .transact_mut()
            .apply_update(u)
            .map_err(|e| format!("apply update: {e}"))?;
        let now = now_ms();
        let n = store
            .append(doc, &by, &update)
            .map_err(|e| format!("updates.log: {e:#}"))?;
        o.log_count += 1;
        o.log_bytes += n;
        o.bytes += update.len();
        o.idle_since = Instant::now();
        o.meta.updated_ms = now;
        let bytes = o.bytes;
        if !from_helper {
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
        let ev = DocEvent::Update {
            update: base64::engine::general_purpose::STANDARD.encode(&update),
            by: Who::from(&by),
        };
        self.fan_out(doc, ev, by.conn());
        if let Err(e) = self.store.touch(doc, now, bytes).await {
            tracing::warn!(doc = %doc, error = %e, "documents index touch failed");
        }
        Ok(())
    }

    fn schedule_export(&mut self, doc: &DocId) {
        let debounce = Duration::from_millis(self.cfg.export_debounce_ms);
        if let Some(o) = self.open.get_mut(doc)
            && !o.meta.derived.is_empty()
        {
            let now = Instant::now();
            o.export_due = Some(now + debounce);
            o.export_first.get_or_insert(now);
        }
    }

    /// Start an import through the helper; the transaction returns as a [`HelperEvent::Update`].
    fn start_import(
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
        let frontmatter = o.meta.kind == DocKind::Knowledge;
        let pending = self.helper.import(doc, &markdown, mode, frontmatter);
        let req = pending.id();
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

    fn actor_ref(&self) -> ActorRef<Self> {
        ActorRef::<Self>::lookup(crate::DOCUMENT_ACTOR)
            .ok()
            .flatten()
            .expect("the document actor is registered before it serves")
    }

    /// Fire the export of `doc`: helper markdown → tmp + rename → [`ExportDone`]; `done` hears the outcome.
    fn export(&mut self, doc: &DocId, done: Option<tokio::sync::oneshot::Sender<Result<(), String>>>) {
        let Some(o) = self.open.get_mut(doc) else {
            return;
        };
        o.exporting = true;
        o.export_due = None;
        o.export_first = None;
        let path = self.data_dir.join(&o.meta.derived);
        let kind = o.meta.kind;
        let helper = self.helper.clone();
        let me = self.actor_ref();
        let doc = doc.clone();
        tokio::spawn(async move {
            let result = match helper.markdown(&doc).await {
                Ok(text) => write_atomic(&path, text.as_bytes())
                    .map(|()| (sha256(text.as_bytes()), title_of(kind, &text)))
                    .map_err(|e| format!("write {}: {e}", path.display())),
                Err(e) => Err(e.to_string()),
            };
            if let Some(done) = done {
                let _ = done.send(result.as_ref().map(|_| ()).map_err(Clone::clone));
            }
            let _ = me.tell(ExportDone { doc, result }).await;
        });
    }

    fn snapshot(&mut self, doc: &DocId, label: Option<&str>) -> Result<(), String> {
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

    fn unload(&mut self, doc: &DocId, reason: &str) {
        if self.open.get(doc).is_some_and(|o| o.log_count > 0)
            && let Err(e) = self.snapshot(doc, None)
        {
            tracing::warn!(doc = %doc, error = %e, "snapshot at unload failed; log kept");
        }
        if self.open.remove(doc).is_some() {
            self.helper.close(doc).log("close");
        }
        if let Some(subs) = self.subs.remove(doc) {
            for s in subs {
                let _ = s.tx.try_send(DocEvent::Closed {
                    reason: reason.to_owned(),
                });
            }
        }
    }

    fn enforce_open_cap(&mut self) {
        while self.open.len() > MAX_OPEN {
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
    async fn delete_doc(&mut self, doc: &DocId) -> Result<(), String> {
        let meta = match self.open.remove(doc) {
            Some(o) => {
                self.helper.close(doc).log("close");
                Some(o.meta)
            }
            None => self.store.read_meta(doc).ok(),
        };
        if let Some(subs) = self.subs.remove(doc) {
            for s in subs {
                let _ = s.tx.try_send(DocEvent::Closed {
                    reason: "deleted".into(),
                });
            }
        }
        self.store
            .delete(doc)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Some(meta) = meta {
            self.rt.publish(DocumentChanged::of(&meta, true)).await;
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
#[derive(Clone, Debug)]
pub struct OpenDoc {
    /// Document.
    pub doc: DocId,
    /// Client's state vector, base64 v1; `None` = everything.
    pub sv: Option<String>,
}

impl Message<OpenDoc> for DocumentActor {
    type Reply = Result<DocOpen, String>;

    async fn handle(&mut self, m: OpenDoc, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let sv = match m.sv {
            Some(s) => StateVector::decode_v1(&unb64(&s)?).map_err(|e| format!("bad sv: {e}"))?,
            None => StateVector::default(),
        };
        let o = self.load(&m.doc)?;
        o.idle_since = Instant::now();
        let state = o.doc.transact().encode_state_as_update_v1(&sv);
        let awareness = o
            .awareness
            .update()
            .map(|u| b64(&u.encode_v1()))
            .unwrap_or_default();
        Ok(DocOpen {
            meta: o.meta.clone(),
            state: state.chunks(CHUNK).map(b64).collect(),
            awareness,
        })
    }
}

/// `ask` → `Result<(), String>`: one v1 update (base64) from `by`; `DocTooLarge` beyond the cap.
#[derive(Clone, Debug)]
pub struct ApplyUpdate {
    /// Document.
    pub doc: DocId,
    /// base64 v1 update.
    pub update: String,
    /// Author (its connection is skipped in fan-out).
    pub by: Principal,
}

impl Message<ApplyUpdate> for DocumentActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: ApplyUpdate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let update = unb64(&m.update)?;
        self.apply(&m.doc, update, m.by, false, Vec::new()).await
    }
}

/// `ask` → `Result<mpsc::Receiver<DocEvent>, String>`: every event of `doc` not caused by `conn`.
#[derive(Clone, Debug)]
pub struct Subscribe {
    /// Document.
    pub doc: DocId,
    /// The subscriber's connection id; its own updates are not echoed.
    pub conn: Option<u64>,
}

impl Message<Subscribe> for DocumentActor {
    type Reply = Result<mpsc::Receiver<DocEvent>, String>;

    async fn handle(&mut self, m: Subscribe, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let o = self.load(&m.doc)?;
        o.idle_since = Instant::now();
        let (tx, rx) = mpsc::channel(SUB_QUEUE);
        self.subs
            .entry(m.doc)
            .or_default()
            .push(Subscriber { conn: m.conn, tx });
        Ok(rx)
    }
}

/// `ask` → `Result<(), String>`: an awareness update (base64 y-protocols), relayed, never persisted.
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
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: AwarenessUpdateMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let bytes = unb64(&m.state)?;
        let u = AwarenessUpdate::decode_v1(&bytes).map_err(|e| format!("bad awareness: {e}"))?;
        let o = self.load(&m.doc)?;
        let now = Instant::now();
        let slot = o
            .awareness_rate
            .entry(m.by.conn())
            .or_insert(RateSlot { since: now, sent: 0 });
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
        match self.store.lookup(&m.binding).await {
            Ok(Some(id)) => {
                tx.send(Ok(id));
                return reply;
            }
            Ok(None) => {}
            Err(e) => {
                tx.send(Err(format!("{e:#}")));
                return reply;
            }
        }
        let id = DocId::new();
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
            created_ms: now,
            updated_ms: now,
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
                // A note keeps the title it was created with; the others follow their text.
                if !title.is_empty() && o.meta.kind != DocKind::Note {
                    o.meta.title = title;
                }
                let meta = o.meta.clone();
                if let Err(e) = self.store.write_meta(&meta).await {
                    tracing::warn!(doc = %m.doc, error = %e, "meta.json write failed");
                }
                self.fan_out(&m.doc, DocEvent::Exported, None);
                self.rt.publish(DocumentChanged::of(&meta, false)).await;
                self.rt
                    .publish(Exported {
                        doc: m.doc,
                        binding: meta.binding,
                        path: self.data_dir.join(&meta.derived),
                    })
                    .await;
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
        self.start_edit(&m.doc, m.edits, m.by, tx);
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
        let last = self.open.get_mut(&m.doc).and_then(|o| o.last_agent_edit.take());
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
        if let Err(e) = self.load(&m.doc) {
            tx.send(Err(e));
            return reply;
        }
        let helper = self.helper.clone();
        tokio::spawn(async move {
            tx.send(
                helper
                    .outline(&m.doc, m.depth)
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
        reply
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
        if let Err(e) = self.load(&m.doc) {
            tx.send(Err(e));
            return reply;
        }
        let helper = self.helper.clone();
        tokio::spawn(async move {
            tx.send(
                helper
                    .markdown_of(&m.doc, m.blocks.as_deref(), m.annotate)
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
        reply
    }
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

impl Message<CreateNote> for DocumentActor {
    type Reply = DelegatedReply<Result<DocSummary, String>>;

    async fn handle(&mut self, m: CreateNote, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.create_doc(Binding::Note, m.title, m.markdown, m.by, tx)
            .await;
        reply
    }
}

impl Message<Create> for DocumentActor {
    type Reply = DelegatedReply<Result<DocSummary, String>>;

    async fn handle(&mut self, m: Create, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.create_doc(m.binding, m.title, m.markdown, m.by, tx)
            .await;
        reply
    }
}

impl DocumentActor {
    /// Meta + index row, `DocumentChanged`, then the headed body as one `replace` import.
    async fn create_doc(
        &mut self,
        binding: Binding,
        title: String,
        markdown: String,
        by: Principal,
        tx: ReplySender<Result<DocSummary, String>>,
    ) {
        let id = DocId::new();
        let now = now_ms();
        let meta = DocMeta {
            derived: binding.derived(&id),
            id: id.clone(),
            kind: binding.kind(),
            title,
            binding,
            fragment: FRAGMENT.to_owned(),
            members: vec![],
            links: vec![],
            agent: AgentStyle {
                color: self.cfg.agent_color.clone(),
            },
            last_export: None,
            created_ms: now,
            updated_ms: now,
        };
        if let Err(e) = self.store.create(&meta).await {
            tx.send(Err(format!("{e:#}")));
            return;
        }
        self.rt.publish(DocumentChanged::of(&meta, false)).await;
        let markdown = with_heading(&meta.title, &markdown);
        self.start_import(
            &id,
            markdown,
            by,
            ImportMode::Replace,
            ImportReply::CreatedNote(DocSummary::from(&meta), tx),
        );
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
        let title = m.title.trim().to_owned();
        if title.is_empty() {
            tx.send(Err("title must not be empty".into()));
            return reply;
        }
        let meta = match self.load(&m.doc) {
            Ok(o) if o.meta.kind != DocKind::Note => {
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
        self.rt.publish(DocumentChanged::of(&meta, false)).await;
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let outline = match me.ask(Outline { doc: m.doc.clone(), depth: Some(1) }).await {
                Ok(o) => o,
                Err(e) => return tx.send(Err(e.to_string())),
            };
            let Some(first) = outline.blocks.first() else {
                return tx.send(Ok(()));
            };
            let is_h1 = first["type"] == "heading"
                && first["text"].as_str().is_some_and(|t| t.starts_with("[h1] "));
            let Some(id) = first["id"].as_str().filter(|_| is_h1) else {
                return tx.send(Ok(()));
            };
            let r = me
                .ask(Edit {
                    doc: m.doc,
                    edits: vec![BlockEdit::Replace {
                        blocks: vec![id.to_owned()],
                        markdown: format!("# {title}"),
                    }],
                    by: Principal::Owner { device: None },
                })
                .await;
            tx.send(r.map(|_| ()).map_err(|e| e.to_string()));
        });
        reply
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
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: DeleteNote, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let kind = match self.open.get(&m.doc) {
            Some(o) => o.meta.kind,
            None => self.store.read_meta(&m.doc).map_err(|e| format!("{e:#}"))?.kind,
        };
        if kind != DocKind::Note {
            return Err("delete_via_owner".into());
        }
        self.delete_doc(&m.doc).await
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

/// `ask` → `Result<(), String>`: a labelled snapshot (§4.6).
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Document.
    pub doc: DocId,
    /// Label (file stem suffix).
    pub label: String,
}

impl Message<Snapshot> for DocumentActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Snapshot, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.load(&m.doc)?;
        self.snapshot(&m.doc, Some(&m.label))
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
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Delete, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.delete_doc(&m.doc).await
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

impl Message<HelperEvent> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: HelperEvent, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        match m {
            HelperEvent::Restarted => {
                let ids: Vec<DocId> = self.open.keys().cloned().collect();
                for id in ids {
                    let o = &self.open[&id];
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
                let (by, snapshot_first) = match (self.imports.get_mut(&req), self.edits.get_mut(&req)) {
                    (Some(imp), _) => {
                        imp.applied = true;
                        done = imp.done_at.is_some();
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
                }
            }
        }
    }
}

/// `tell` every 250 ms: exports due, snapshot policy, idle unload, hourly trash sweep (§4.1, §4.3, §4.7, §7.3).
#[derive(Clone, Copy, Debug, Default)]
pub struct Tick;

impl Message<Tick> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, _: Tick, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let now = Instant::now();
        let mut exports = Vec::new();
        let mut snapshots = Vec::new();
        let mut unloads = Vec::new();
        for (id, o) in &self.open {
            if !o.exporting
                && let Some(due) = o.export_due
                && (now >= due || o.export_first.is_some_and(|f| now >= f + EXPORT_CAP))
            {
                exports.push(id.clone());
            }
            if o.log_bytes > LOG_MAX_BYTES || o.log_count > LOG_MAX_COUNT {
                snapshots.push(id.clone());
            }
            if !o.exporting
                && o.export_due.is_none()
                && !self.subs.contains_key(id)
                && now.duration_since(o.idle_since) > IDLE_UNLOAD
            {
                unloads.push(id.clone());
            }
        }
        for id in exports {
            self.export(&id, None);
        }
        for id in snapshots {
            if let Err(e) = self.snapshot(&id, None) {
                tracing::warn!(doc = %id, error = %e, "policy snapshot failed");
            }
        }
        for id in unloads {
            self.unload(&id, "unloaded");
        }
        // Imports that replied but changed nothing send no update: forget them after a grace period.
        self.imports
            .retain(|_, i| i.done_at.is_none_or(|t| now.duration_since(t) < IMPORT_GRACE));
        // Subscribers whose receiver went away.
        for subs in self.subs.values_mut() {
            subs.retain(|s| !s.tx.is_closed());
        }
        self.subs.retain(|_, s| !s.is_empty());
        if self.last_sweep.is_none_or(|t| now.duration_since(t) >= TRASH_SWEEP) {
            self.last_sweep = Some(now);
            match self.store.sweep_trash(TRASH_KEEP).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "documents trash swept"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "documents trash sweep failed"),
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
}
