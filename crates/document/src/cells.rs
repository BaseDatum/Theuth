//! Seams for user cells in notes (design doc 171): the key a cell is addressed
//! by, the actor message that reads a code block's text, the `doc_read`
//! run-annotation hook (§9.6) and the replicated latest-output rows (§13.2).
//! 184 §6.2: the block is read on the document's server (a `Block` leg), with a local-replica
//! fallback; §6.4: `cell_latest` rows are whole-JSON `payload`s any engine upserts.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::Engine as _;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use mesh::EngineId;
use serde::{Deserialize, Serialize};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, StateVector, Transact, Update};

use crate::actor::{DocumentActor, Markdown, Subscribe};
use crate::hub::{BLOCK_WAIT_MS, DocError, HANDBACK_TIMEOUT_MS, Route};
use crate::meta::{CellLatestChanged, DocEvent, DocId, DocOpen, Principal};
use crate::relay::{Client, Cmd};
use crate::store::Store;

/// How a note cell is addressed (171 §15.1): by its `name:` header or by its
/// block id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum NoteKey {
    /// The cell's `name:` header.
    Name(String),
    /// The BlockNote block id.
    Block(String),
}

/// Reply to [`CodeBlockText`]: a code block read from the live Y.Doc.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, kameo::Reply)]
pub struct CodeBlockTextReply {
    /// Block id.
    pub block: String,
    /// Fence language.
    pub language: String,
    /// Fence body.
    pub text: String,
    /// Who last wrote the block (`DocEvent::Authored`, 171 §9.6).
    pub last_author: Principal,
    /// 184 §6.2: the whole note as the server's `Markdown{blocks: None, annotate: false}` (the
    /// run's defaults, cell names and `after:` target) for a press (`seen` set); `""` otherwise.
    #[serde(default)]
    pub markdown: String,
}

/// `ask` → `Result<CodeBlockTextReply, String>`: read one code block of a
/// note from the live Y.Doc (171 §15.2, internal — not RPC). 184 §6.2: on the document's server
/// (`route_doc`): here when this engine serves it, else over a `Block` leg; when the server cannot
/// answer, from the local replica with the author checked against the holder (W-i).
#[derive(Clone, Debug)]
pub struct CodeBlockText {
    /// Document.
    pub doc: DocId,
    /// The cell.
    pub key: NoteKey,
    /// 184 §6.2 (W-u): the digest the press carries; the server waits up to `BLOCK_WAIT_MS` for
    /// the update that makes the block match it, and then also answers the note's `markdown`.
    pub seen: Option<String>,
}

/// 184 §6.2 (W-i): the `last_author` member key of a block whose author cannot be told (the note
/// has members, its server is unreachable and so is the holder). `cells` turns it into
/// `MemberAck("?")`: the press asks the user to confirm instead of refusing.
pub const UNKNOWN_AUTHOR: &str = "?";

type BlockReply = Result<CodeBlockTextReply, String>;

impl Message<CodeBlockText> for DocumentActor {
    type Reply = DelegatedReply<BlockReply>;

    async fn handle(
        &mut self,
        m: CodeBlockText,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let me = ctx.actor_ref().clone();
        // A `Relayed(CodeBlockText)` from a leg routes as relayed (rule 6), and one this engine
        // does not serve is refused: the caller reads its own replica (W-i).
        let relayed = self.relaying;
        match self.route_doc(&m.doc, relayed) {
            Ok(Route::Serve) => self.serve_block(m, tx, me),
            // §4.3 step 4: wait for this engine's claim, then read the claimed Doc.
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            Ok(Route::Forward(server)) => {
                tokio::spawn(async move { tx.send(from_server(&me, server, m).await) });
            }
            Err(e) if relayed => tx.send(Err(e.to_string())),
            Err(_) => {
                tokio::spawn(async move { tx.send(fallback(&me, &m.doc, &m.key).await) });
            }
        }
        reply
    }
}

impl DocumentActor {
    /// The block as this engine's live Doc has it (no routing).
    fn read_block(&mut self, doc: &DocId, key: &NoteKey) -> BlockReply {
        let blocks = self.code_blocks(doc)?;
        crate::note_cells::find(blocks, key, doc)
    }

    /// `Serve` (§6.2): answer from the live Doc. With `seen`, a block that does not match it yet
    /// parks the reply until an applied update makes it match or `BLOCK_WAIT_MS` passes (W-u: the
    /// keystroke rides the editor's leg, this read another), then the reply carries the markdown.
    fn serve_block(&mut self, m: CodeBlockText, tx: ReplySender<BlockReply>, me: ActorRef<Self>) {
        let got = self.read_block(&m.doc, &m.key);
        let Some(seen) = m.seen else {
            return tx.send(got);
        };
        tokio::spawn(async move {
            let got = match got {
                Ok(b) if surface::cell::digest(&b.text) == seen => Ok(b),
                _ => wait_block(&me, &m.doc, &m.key, &seen).await,
            };
            let got = match got {
                Ok(mut b) => {
                    let md = Markdown {
                        doc: m.doc,
                        blocks: None,
                        annotate: false,
                    };
                    b.markdown = me.ask(md).await.unwrap_or_default();
                    Ok(b)
                }
                e => e,
            };
            tx.send(got);
        });
    }
}

fn ask_err<M, E: std::fmt::Display>(e: kameo::error::SendError<M, E>) -> String {
    match e {
        kameo::error::SendError::HandlerError(e) => e.to_string(),
        other => format!("documents: {other}"),
    }
}

/// W-u: re-read the block on every update applied to `doc` until its digest is `seen` or
/// `BLOCK_WAIT_MS` passed; the last read either way. Nothing waits inside the actor.
async fn wait_block(
    me: &ActorRef<DocumentActor>,
    doc: &DocId,
    key: &NoteKey,
    seen: &str,
) -> BlockReply {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(BLOCK_WAIT_MS);
    let by = Principal::Owner { device: None };
    let mut events = me
        .ask(Subscribe {
            doc: doc.clone(),
            by,
        })
        .await
        .ok();
    loop {
        // Read after subscribing, so an update landing in between is seen.
        let got = me
            .ask(ReadBlock {
                doc: doc.clone(),
                key: key.clone(),
            })
            .await
            .map_err(ask_err);
        if got
            .as_ref()
            .is_ok_and(|b| surface::cell::digest(&b.text) == seen)
        {
            return got;
        }
        let Some(rx) = events.as_mut() else {
            return got;
        };
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(DocEvent::Update { .. })) => break,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return got,
            }
        }
    }
}

/// `Forward(server)`: the server's answer over a `Block` leg; the local fallback when the leg
/// refuses, closes or stays silent for `HANDBACK_TIMEOUT_MS` (an old server never answers tag 12).
async fn from_server(
    me: &ActorRef<DocumentActor>,
    server: EngineId,
    m: CodeBlockText,
) -> BlockReply {
    let leg = me.ask(BlockLeg {
        server: server.clone(),
        doc: m.doc.clone(),
        key: m.key.clone(),
        seen: m.seen,
    });
    let wait = Duration::from_millis(HANDBACK_TIMEOUT_MS);
    match tokio::time::timeout(wait, async { leg.await }).await {
        Ok(Ok(b)) => return Ok(b),
        Ok(Err(e)) => {
            tracing::debug!(doc = %m.doc, %server, error = %ask_err(e), "cell block: server read failed; reading the replica")
        }
        Err(_) => {
            tracing::debug!(doc = %m.doc, %server, "cell block: server silent; reading the replica")
        }
    }
    fallback(me, &m.doc, &m.key).await
}

/// W-i: the block from the local replica. A note with members takes the block's author from the
/// holder's `DocOpen` (its state merged into a throwaway copy of the replica); when nobody
/// answers, the author is [`UNKNOWN_AUTHOR`] — the press then asks to confirm, never refuses.
async fn fallback(me: &ActorRef<DocumentActor>, doc: &DocId, key: &NoteKey) -> BlockReply {
    let here = me
        .ask(BlockHere {
            doc: doc.clone(),
            key: key.clone(),
        })
        .await
        .map_err(ask_err)?;
    let Some(check) = here.check else {
        return Ok(here.block);
    };
    // When nobody answers: a note this replica knows as shared has an unknown author (the
    // confirm); one with no members here keeps the replica's author (Z184-R2 asks the holder for
    // it anyway with homes on, in case this meta lags the snapshot).
    let unknown = if check.shared {
        CodeBlockTextReply {
            last_author: Principal::Member {
                pub_: UNKNOWN_AUTHOR.to_owned(),
                device: None,
            },
            ..here.block
        }
    } else {
        here.block
    };
    let Some(holder) = check.holder.clone() else {
        return Ok(unknown);
    };
    let open = me.ask(OpenAt {
        holder,
        doc: doc.clone(),
        sv: check.sv.clone(),
    });
    let wait = Duration::from_millis(HANDBACK_TIMEOUT_MS);
    match tokio::time::timeout(wait, async { open.await }).await {
        Ok(Ok(open)) => match merged(&check, &open) {
            Some(blocks) => crate::note_cells::find(blocks, key, doc),
            None => Ok(unknown),
        },
        _ => Ok(unknown),
    }
}

/// The code blocks of the replica merged with the holder's `DocOpen` state, authored by the
/// holder's `meta.authors` (and this engine's `agents.json`); `None` when the state does not decode.
fn merged(check: &Check, open: &DocOpen) -> Option<Vec<CodeBlockTextReply>> {
    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(Update::decode_v1(&check.state).ok()?)
            .ok()?;
        for chunk in &open.state {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk)
                .ok()?;
            txn.apply_update(Update::decode_v1(&bytes).ok()?).ok()?;
        }
    }
    Some(crate::note_cells::blocks_of(
        &doc,
        &check.fragment,
        &open.meta.authors,
        &check.agents,
    ))
}

/// `ask` (internal) → `BlockReply`: [`DocumentActor::read_block`], unrouted.
struct ReadBlock {
    doc: DocId,
    key: NoteKey,
}

impl Message<ReadBlock> for DocumentActor {
    type Reply = BlockReply;

    async fn handle(&mut self, m: ReadBlock, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.read_block(&m.doc, &m.key)
    }
}

/// `ask` (internal) → `BlockReply`: `Cmd::Block` on the owner's leg of `doc` to `server`.
struct BlockLeg {
    server: EngineId,
    doc: DocId,
    key: NoteKey,
    seen: Option<String>,
}

impl Message<BlockLeg> for DocumentActor {
    type Reply = DelegatedReply<BlockReply>;

    async fn handle(&mut self, m: BlockLeg, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            let cmd = Cmd::Block {
                key: m.key,
                seen: m.seen,
                reply: tx,
            };
            self.forward(m.server, &m.doc, Client::Conn(None), cmd);
        }
        reply
    }
}

/// `ask` (internal) → `Result<DocOpen, DocError>`: `Cmd::Open` with `sv` on the owner's leg of
/// `doc` to the lease `holder` (tag 0, served by every engine since 69).
struct OpenAt {
    holder: EngineId,
    doc: DocId,
    sv: String,
}

impl Message<OpenAt> for DocumentActor {
    type Reply = DelegatedReply<Result<DocOpen, DocError>>;

    async fn handle(&mut self, m: OpenAt, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            let cmd = Cmd::Open {
                sv: Some(m.sv),
                reply: tx,
            };
            self.forward(m.holder, &m.doc, Client::Conn(None), cmd);
        }
        reply
    }
}

/// What the replica needs to check a block's author with the holder (a note with members).
struct Check {
    /// The lease holder, when another engine holds it.
    holder: Option<EngineId>,
    /// The replica's state vector, base64 v1 (the `Open` asks only for what it lacks).
    sv: String,
    /// The replica's whole state, v1.
    state: Vec<u8>,
    fragment: String,
    agents: BTreeMap<String, uuid::Uuid>,
    /// This replica's meta lists members.
    shared: bool,
}

/// [`BlockHere`]'s answer: the replica's block, and `check` when the note has members.
struct Here {
    block: CodeBlockTextReply,
    check: Option<Check>,
}

/// `ask` (internal) → `Result<Here, String>`: the block from this engine's copy.
struct BlockHere {
    doc: DocId,
    key: NoteKey,
}

impl Message<BlockHere> for DocumentActor {
    type Reply = Result<Here, String>;

    async fn handle(&mut self, m: BlockHere, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let block = self.read_block(&m.doc, &m.key)?;
        let holder = self.hub.route().ok().flatten();
        let homes_on = self.hub.homes_on;
        let Some(o) = self
            .open
            .get(&m.doc)
            .filter(|o| homes_on || !o.meta.members.is_empty())
        else {
            return Ok(Here { block, check: None });
        };
        let txn = o.doc.transact();
        let check = Check {
            holder,
            sv: base64::engine::general_purpose::STANDARD.encode(txn.state_vector().encode_v1()),
            state: txn.encode_state_as_update_v1(&StateVector::default()),
            fragment: o.meta.fragment.clone(),
            agents: o.agents.clone(),
            shared: !o.meta.members.is_empty(),
        };
        Ok(Here {
            block,
            check: Some(check),
        })
    }
}

/// 184 §6.4 (W-p): watch `handle`'s replica batches; a batch from a foreign origin touching
/// `cell_latest` publishes [`CellLatestChanged`] on `rt`'s bus for each document in it. Spawns
/// and returns; called once by the plugin after `replica::attach`.
pub fn watch_latest(handle: replica::ReplicaHandle, rt: runtime::Runtime) {
    let mut rx = handle.subscribe();
    let me = handle.origin().clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(batch) => {
                    for doc in latest_docs(&batch, &me) {
                        rt.publish(CellLatestChanged { doc }).await;
                    }
                }
                // ponytail: a lagged watcher misses those batches' events; the next run's row
                // or a reopen re-reads. Re-read every open note on lag if that shows.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// The documents whose `cell_latest` rows a foreign batch wrote (row id `<doc>:<cell>`).
fn latest_docs(batch: &replica::ChangeBatch, me: &replica::PeerId) -> HashSet<DocId> {
    if batch.origin == *me {
        return HashSet::new();
    }
    batch
        .changes
        .iter()
        .filter(|c| c.table == "cell_latest")
        .filter_map(|c| c.pk.split_once(':'))
        .map(|(doc, _)| DocId(doc.to_owned()))
        .collect()
}

/// What `doc_read` shows for a cell with a latest run (171 §9.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellAnnotation {
    /// The outline prefix, e.g. `[cell:bash name · ran 14:38 · exit 0 · 33 s · expect ✓]`.
    pub outline: String,
    /// Text of the ```` ```output ```` fence, when the run produced one.
    pub output: Option<String>,
    /// The run line, e.g. `ran 14:38 · exit 0 · 33 s · expect ✓`.
    pub run_line: String,
}

/// Process-global hook the cells plugin installs so `doc_read` can annotate
/// a code block with its latest run; `cells` depends on `document`, not the
/// reverse (171 §9.6, the `tools::secrets` pattern).
pub trait RunAnnotations: Send + Sync {
    /// The annotation for one code block, `None` when it is not a cell or
    /// never ran.
    fn note_cell(
        &self,
        doc: &DocId,
        block: &str,
        language: &str,
        text: &str,
    ) -> Option<CellAnnotation>;
}

static RUN_ANNOTATIONS: RwLock<Option<Arc<dyn RunAnnotations>>> = RwLock::new(None);

/// Install the hook; a later call replaces the earlier one.
pub fn set_run_annotations(a: Arc<dyn RunAnnotations>) {
    *RUN_ANNOTATIONS.write().unwrap_or_else(|e| e.into_inner()) = Some(a);
}

/// The installed hook, if any.
pub fn run_annotations() -> Option<Arc<dyn RunAnnotations>> {
    RUN_ANNOTATIONS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// One replicated latest-output row of a note cell (171 §13.2 `cell_latest`,
/// minus the HLC column).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CellLatest {
    /// `"<doc>:<name or block id>"`.
    pub id: String,
    /// Document.
    pub doc: DocId,
    /// Cell name or block id.
    pub cell: String,
    /// Run id.
    pub run: String,
    /// Final `RunState`, snake_case.
    pub state: String,
    /// Exit code.
    pub exit: Option<i32>,
    /// Wall time.
    pub secs: Option<f64>,
    /// `expect` verdict, rendered.
    pub expect: Option<String>,
    /// Python/Monty one-line summary.
    pub summary: Option<String>,
    /// Digest of the code that ran.
    pub code_sha: String,
    /// Unix ms.
    pub ended_ms: i64,
    /// Cleaned, redacted head+tail; `""` for a private cell.
    pub text: String,
    /// 184 §6.4: the engine that ran it; `""` for a row from a pre-184 engine (old columns only).
    #[serde(default)]
    pub engine: String,
}

/// `cell_latest` row as sqlx reads it (the old columns, then `payload`).
type LatestRow = (
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<f64>,
    Option<String>,
    Option<String>,
    String,
    i64,
    String,
    String,
);

impl Store {
    /// Latest-output rows of a document (171 §13.2), by id. 184 §6.4 (W-h): a row's `payload` when
    /// it has one, else its old columns with `engine = ""` (a pre-184 writer).
    pub async fn cell_latest(&self, doc: &DocId) -> anyhow::Result<Vec<CellLatest>> {
        let rows: Vec<LatestRow> = sqlx::query_as(
            "SELECT id, doc, cell, run, state, exit, secs, expect, summary, code_sha, ended_ms, text, payload \
             FROM cell_latest WHERE doc = ? ORDER BY id",
        )
        .bind(doc.as_str())
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    doc,
                    cell,
                    run,
                    state,
                    exit,
                    secs,
                    expect,
                    summary,
                    code_sha,
                    ended_ms,
                    text,
                    payload,
                )| {
                    serde_json::from_str(&payload).unwrap_or_else(|_| CellLatest {
                        id,
                        doc: DocId(doc),
                        cell,
                        run,
                        state,
                        exit: exit.and_then(|e| i32::try_from(e).ok()),
                        secs,
                        expect,
                        summary,
                        code_sha,
                        ended_ms,
                        text,
                        engine: String::new(),
                    })
                },
            )
            .collect())
    }

    /// Upsert one latest-output row (171 §13.2). 184 §6.4 (W-h): `payload` = the whole row as
    /// JSON, so per-column LWW keeps one run's row intact when two engines upsert it; the old
    /// columns stay for pre-184 readers.
    pub async fn put_cell_latest(&self, row: &CellLatest) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO cell_latest (id, doc, cell, run, state, exit, secs, expect, summary, code_sha, ended_ms, text, payload) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET doc = excluded.doc, cell = excluded.cell, run = excluded.run, \
             state = excluded.state, exit = excluded.exit, secs = excluded.secs, expect = excluded.expect, \
             summary = excluded.summary, code_sha = excluded.code_sha, ended_ms = excluded.ended_ms, text = excluded.text, \
             payload = excluded.payload",
        )
        .bind(&row.id)
        .bind(row.doc.as_str())
        .bind(&row.cell)
        .bind(&row.run)
        .bind(&row.state)
        .bind(row.exit.map(i64::from))
        .bind(row.secs)
        .bind(&row.expect)
        .bind(&row.summary)
        .bind(&row.code_sha)
        .bind(row.ended_ms)
        .bind(&row.text)
        .bind(serde_json::to_string(row)?)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

// 171:E2-ADD — `Store` is actor-private, so the cells plugin reaches the `cell_latest` rows
// through these two messages (171 §13.2; the bodies call D2's `Store` accessors).

/// `ask` → `Result<Vec<CellLatest>, String>`: the replicated latest-output rows of a document.
/// Any engine (replica); membership is the caller's check.
#[derive(Clone, Debug)]
pub struct CellLatestOf(pub DocId);

impl Message<CellLatestOf> for DocumentActor {
    type Reply = Result<Vec<CellLatest>, String>;

    async fn handle(&mut self, m: CellLatestOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.store
            .cell_latest(&m.0)
            .await
            .map_err(|e| e.to_string())
    }
}

/// `ask` → `Result<(), String>`: upsert one latest-output row. Any engine (184 §6.4): each upserts
/// after its own runs, and the replica's HLC picks the newest run of the cell.
#[derive(Clone, Debug)]
pub struct PutCellLatest(pub CellLatest);

impl Message<PutCellLatest> for DocumentActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: PutCellLatest,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.store
            .put_cell_latest(&m.0)
            .await
            .map_err(|e| e.to_string())
    }
}
