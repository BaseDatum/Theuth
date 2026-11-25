//! Whiteboards (218): a `DocKind::Whiteboard` document is one `yrs::Doc` of seven top-level
//! `Y.Map`s (contract §2) — no XML fragment, no helper mirror. This module is the engine's side:
//! the Rust twin of the schema ([`model`]), the packed-point codec ([`pack`]), the alias table
//! ([`aliases`]), the `<whiteboard>` outline ([`outline`]), placement ([`place`]), the `wb_*` tools
//! ([`tools`]), turn bookkeeping and *Undo this turn* ([`undo`]), and the actor messages
//! `crates/agent` and `crates/artifact` call (contract §3.5).

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{DocId, DocumentActor};

pub mod aliases;
pub mod engine;
pub mod model;
pub mod outline;
pub mod pack;
pub mod place;
pub mod tools;

pub use outline::{quote, unquote};

/// What every `doc_*` path answers for a whiteboard (§3.1).
pub const DOC_REFUSAL: &str = "this is a whiteboard; use the wb_* tools (wb_read, wb_card, wb_place, …)";

/// The outline of one whiteboard, as the agent reads it. Runs `ensure_aliases` first.
#[derive(Clone, Debug)]
pub struct WbOutline {
    /// Whiteboard.
    pub doc: DocId,
    /// A frame (alias or ULID): only that frame's rows and cards, each with its source summary.
    pub frame: Option<String>,
}

/// [`WbOutline`]'s answer.
#[derive(Clone, Debug)]
pub struct WbOutlineReply {
    /// Whiteboard.
    pub doc: DocId,
    /// `DocMeta.title`.
    pub title: String,
    /// The full `<whiteboard …>…</whiteboard>` block of §3.2 (or the frame detail when `frame` is set).
    pub text: String,
    /// Listed cards.
    pub cards: u32,
    /// Listed links.
    pub links: u32,
    /// Listed marks.
    pub marks: u32,
    /// `DocMeta.updated_ms`.
    pub updated_ms: i64,
    /// ULID → alias, every item that has one (to map the client's send context).
    pub aliases: std::collections::BTreeMap<String, String>,
}

impl Message<WbOutline> for DocumentActor {
    type Reply = DelegatedReply<Result<WbOutlineReply, String>>;

    async fn handle(&mut self, m: WbOutline, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_outline(&m.doc, m.frame.as_deref()).await);
        reply
    }
}

/// Title and card count for a marker's meta, and the kind check ("is this a whiteboard?").
#[derive(Clone, Debug)]
pub struct WbInfo {
    /// Document.
    pub doc: DocId,
}

/// [`WbInfo`]'s answer.
#[derive(Clone, Debug)]
pub struct WbInfoReply {
    /// Whiteboard.
    pub doc: DocId,
    /// `DocMeta.title`.
    pub title: String,
    /// Live and dimmed cards (archived ones are not counted).
    pub cards: u32,
    /// The document is in the trash.
    pub trashed: bool,
}

impl Message<WbInfo> for DocumentActor {
    type Reply = DelegatedReply<Result<WbInfoReply, String>>;

    async fn handle(&mut self, m: WbInfo, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_info(&m.doc).await);
        reply
    }
}

/// A turn boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnPhase {
    /// The injection site is about to hand the outline to a turn.
    Begin,
    /// The turn finished (best effort).
    End,
}

/// Turn boundary of `session` on `doc` (§3.3). Returns the turn number.
#[derive(Clone, Debug)]
pub struct WbTurn {
    /// Whiteboard.
    pub doc: DocId,
    /// The session whose turn it is.
    pub session: uuid::Uuid,
    /// Begin or end.
    pub phase: TurnPhase,
}

impl Message<WbTurn> for DocumentActor {
    type Reply = DelegatedReply<Result<u32, String>>;

    async fn handle(&mut self, m: WbTurn, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_turn(&m.doc, m.session, m.phase).await);
        reply
    }
}

/// Installed once by `crates/agent` at startup (the pattern of `install_insight_hooks`,
/// `crates/agent/src/rpc_document.rs#L57-L60`). `bind` is called by every `wb_*` write tool, by `wb_create`
/// and by `show_whiteboard`, at most once per (session, whiteboard) per process. Absent hooks = no-op.
pub mod hooks {
    use std::collections::HashSet;
    use std::sync::{LazyLock, Mutex, OnceLock};

    /// The agent's callbacks.
    pub struct Hooks {
        /// Bind `session` to the whiteboard (a `session_whiteboards` row, `by = 'tool'`).
        pub bind: std::sync::Arc<dyn Fn(uuid::Uuid, crate::DocId) + Send + Sync>,
    }

    static HOOKS: OnceLock<Hooks> = OnceLock::new();
    static SEEN: LazyLock<Mutex<HashSet<(uuid::Uuid, crate::DocId)>>> =
        LazyLock::new(Default::default);

    /// Install the hooks; a second install is ignored.
    pub fn install(h: Hooks) {
        let _ = HOOKS.set(h);
    }

    /// Bind `session` to `doc` through the installed hook, once per pair per process.
    pub fn bind(session: uuid::Uuid, doc: &crate::DocId) {
        let Some(h) = HOOKS.get() else { return };
        let fresh = SEEN
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((session, doc.clone()));
        if fresh {
            (h.bind)(session, doc.clone());
        }
    }
}

/// `whiteboard_create`'s answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WhiteboardCreated {
    /// The new whiteboard.
    pub doc: DocId,
    /// Its title.
    pub title: String,
}

/// `wb_blob_put`'s answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WbBlob {
    /// The stored key.
    pub key: String,
    /// Decoded size.
    pub bytes: u32,
}

/// `wb_blob_get`'s answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WbBlobData {
    /// The key.
    pub key: String,
    /// `image/png` | `image/jpeg` | `image/webp`.
    pub mime: String,
    /// Base64.
    pub data: String,
}

/// One turn of a session on a whiteboard (for the reply bubble).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WbTurnInfo {
    /// 1-based per (whiteboard, session).
    pub turn: u32,
    /// Card ULIDs the turn created or changed.
    pub cards: Vec<String>,
    /// Link ULIDs the turn created or changed.
    pub links: Vec<String>,
    /// Frame ULIDs the turn created or changed.
    pub frames: Vec<String>,
    /// `wb_*` writes in the turn.
    pub writes: u32,
    /// *Undo this turn* ran.
    pub undone: bool,
    /// The turn finished.
    pub ended: bool,
}

/// `whiteboard_undo_turn`'s answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WbUndoResult {
    /// The turn undone.
    pub turn: u32,
    /// Writes reverted.
    pub undone: u32,
    /// Items left alone because the owner changed them since.
    pub skipped: Vec<WbSkipped>,
}

/// An item *Undo this turn* left alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WbSkipped {
    /// ULID.
    pub id: String,
    /// Its alias, when it has one.
    pub alias: Option<String>,
    /// Why, e.g. "moved by you since".
    pub why: String,
}

/// `ask` → [`WhiteboardCreated`]: a new, empty whiteboard (§4.1). Never imports through the helper;
/// a non-serving engine forwards it (`CreateReq::Whiteboard`).
#[derive(Clone, Debug)]
pub struct CreateWhiteboard {
    /// Title; `Whiteboard` when absent or blank.
    pub title: Option<String>,
    /// Who creates it.
    pub by: crate::Principal,
    /// Where it lives (184 §5.1); `Here` from the RPC.
    pub home: crate::Homing,
}

impl Message<CreateWhiteboard> for DocumentActor {
    type Reply = DelegatedReply<Result<WhiteboardCreated, String>>;

    async fn handle(
        &mut self,
        m: CreateWhiteboard,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        self.wb_create_routed(m.title, m.by, m.home, tx).await;
        reply
    }
}

/// `ask` → [`WbBlob`]: store one blob of a whiteboard (§4.1 `wb_blob_put`).
#[derive(Clone, Debug)]
pub struct BlobPut {
    /// Whiteboard.
    pub doc: DocId,
    /// `snap/<card>/<ver>` | `mark/<mark>` | `img/<ulid>`.
    pub key: String,
    /// `image/png` | `image/jpeg` | `image/webp`.
    pub mime: String,
    /// Base64.
    pub data: String,
    /// Caller.
    pub by: crate::Principal,
}

impl Message<BlobPut> for DocumentActor {
    type Reply = DelegatedReply<Result<WbBlob, String>>;

    async fn handle(&mut self, m: BlobPut, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_blob_put(m).await);
        reply
    }
}

/// `ask` → `Option<WbBlobData>`: one blob of a whiteboard (§4.1 `wb_blob_get`).
#[derive(Clone, Debug)]
pub struct BlobGet {
    /// Whiteboard.
    pub doc: DocId,
    /// The key.
    pub key: String,
    /// Caller.
    pub by: crate::Principal,
}

impl Message<BlobGet> for DocumentActor {
    type Reply = DelegatedReply<Result<Option<WbBlobData>, String>>;

    async fn handle(&mut self, m: BlobGet, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_blob_get(m).await);
        reply
    }
}

/// `ask` → `Option<WbTurnInfo>`: the newest turn of `session` on `doc` (§4.1 `whiteboard_turn`).
#[derive(Clone, Debug)]
pub struct TurnOf {
    /// Whiteboard.
    pub doc: DocId,
    /// Session id.
    pub session: String,
    /// Caller.
    pub by: crate::Principal,
}

impl Message<TurnOf> for DocumentActor {
    type Reply = DelegatedReply<Result<Option<WbTurnInfo>, String>>;

    async fn handle(&mut self, m: TurnOf, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_turn_of(m).await);
        reply
    }
}

/// `ask` → [`WbUndoResult`]: *Undo this turn* (§3.3, §4.1 `whiteboard_undo_turn`).
#[derive(Clone, Debug)]
pub struct UndoTurn {
    /// Whiteboard.
    pub doc: DocId,
    /// Session id.
    pub session: String,
    /// Turn number; the newest not yet undone when absent.
    pub turn: Option<u32>,
    /// Caller.
    pub by: crate::Principal,
}

impl Message<UndoTurn> for DocumentActor {
    type Reply = DelegatedReply<Result<WbUndoResult, String>>;

    async fn handle(&mut self, m: UndoTurn, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        let Some((m, tx)) = self.gate_here(&doc, m, tx) else {
            return reply;
        };
        tx.send(self.wb_undo_turn(m).await);
        reply
    }
}
