//! `theuth/docs/1` ([`replica::ALPN_DOCS`]): a non-hub engine forwards a client's document session to
//! the hub (69 §5 decision 11, contract `69-contract-docs.md` §5). Framing as `theuth/sessions/1`:
//! `LengthDelimitedCodec` frames of one tag byte + postcard, frames up to [`MAX_FRAME`].
//!
//! **One stream per (client, document)** ([`Client`] × `DocId`), opened lazily by the first hub-only
//! call the [`DocumentActor`] receives off the hub (`OpenDoc`, `Subscribe`, `ApplyUpdate`,
//! `AwarenessUpdateMsg`, `Join`, `SetName`): the actor hands the call to the leg as a [`Cmd`]
//! ([`DocumentActor::forward`]); the leg task ([`run_leg`]) opens the stream, says who the client is
//! ([`Msg::Hello`]) and relays frames both ways. `Open`/`Join`/`SetName` are answered by one
//! [`Msg::Event`] carrying the reply JSON (never a `DocEvent`: no `type` key); `Subscribe` turns the
//! hub's `DocEvent`s into the local `mpsc` the RPC subscription already reads, so the desktop's
//! `provider.ts` sees no difference. [`Msg::Closed`] ends the leg: the reason reaches the subscriber
//! as `DocEvent::Closed` and every pending reply as an error — `hub_moved` when the lease left the
//! hub mid-stream; the client's next `doc_open(sv)` opens a new leg to whoever holds it then.
//!
//! Hub side ([`serve`]): every accepted stream is one remote client with a fresh connection id; its
//! frames become the same actor messages a local connection sends, so admission, roles, fan-out
//! skipping and the rate caps apply unchanged. A refusal is a `Closed{reason}` and the end of the
//! stream (the wire has no per-call ack; `Update` errors close, `Awareness` errors are dropped).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::ReplySender;
use mesh::EngineId;
use replica::{ALPN_DOCS, PeerId, Transport};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;

use crate::actor::{
    ApplyUpdate, AwarenessUpdateMsg, Create, CreateNote, DocumentActor, Edit, EditOutcome, Ensure,
    Homing, MAX_UPDATE_B64, MetaOf, OpenDoc, Subscribe,
};
use crate::cells::{CodeBlockText, CodeBlockTextReply, NoteKey};
use crate::helper::{Edit as BlockEdit, EditError};
use crate::hub::{DocError, HUB_MOVED, Handback, Handover, NOT_OWNER, Relayed};
use crate::meta::{Binding, DocEvent, DocId, DocMeta, DocOpen, DocSummary, Joined, Principal};
use crate::notebook::{MayRead, NotebookCreate};
use crate::share::{Join, JoinError, SetName};

/// Events a forwarded subscriber may leave unread before the leg drops it (as the actor's own queue).
const QUEUE: usize = 256;

/// Largest frame either side accepts (a `doc_open` state for an 8 MiB document, base64, plus headroom).
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Every message on a `theuth/docs/1` stream. Document ids are `DocId` strings; `bytes` payloads are
/// what the RPC methods carry (base64 v1 updates and awareness states as UTF-8, `DocEvent` as JSON).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Msg {
    /// Front door → hub, first frame: open `doc` for the client behind this stream; `sv` = the client's
    /// base64 state vector (`None` = everything). Answered by one [`Msg::Event`] carrying the `DocOpen`.
    Open {
        /// The document.
        doc: String,
        /// The client's state vector, base64 v1.
        sv: Option<String>,
    },
    /// Front door → hub: one client update (`doc_update`), base64 v1.
    Update {
        /// The document.
        doc: String,
        /// base64 v1 update.
        bytes: Vec<u8>,
    },
    /// Front door → hub: one awareness state (`doc_awareness`), base64 y-protocols.
    Awareness {
        /// The document.
        doc: String,
        /// base64 awareness update.
        bytes: Vec<u8>,
    },
    /// Front door → hub, first frame of the event leg: every `DocEvent` of `doc` for this client.
    Subscribe {
        /// The document.
        doc: String,
    },
    /// Hub → front door: one `DocEvent` (or the `DocOpen` reply), JSON.
    Event {
        /// The document.
        doc: String,
        /// The event as JSON.
        bytes: Vec<u8>,
    },
    /// Front door → hub: a guest's `doc_join` (59 §8.3) for the connection behind this stream.
    Join {
        /// The document.
        doc: String,
        /// The guest's X25519 static.
        guest_pub: [u8; 32],
        /// The Noise handshake hash of the guest's connection at the front door.
        handshake_hash: Vec<u8>,
        /// `HMAC-SHA256(sha256(secret), handshake_hash ‖ guest_pub)`.
        proof: Vec<u8>,
        /// Self-chosen display name.
        name: Option<String>,
    },
    /// Hub → front door: the stream ends; `reason` as `DocEvent::Closed` words it (`deleted`,
    /// `unloaded`, `slow`, `demoted`, `revoked`, `hub_moved`) or an error code (`not_member`, …).
    Closed {
        /// The document.
        doc: String,
        /// Why.
        reason: String,
    },
    /// Front door → hub, the very first frame (Y, beside the C2 set): who the client behind this
    /// stream is — `member` = a guest's static (base64url) for a mode-2 conn, `None` for the owner's
    /// own devices. Absent → owner. The hub mints one connection id for the stream.
    Hello {
        /// The member's static key, base64url, when the client is a share-link member.
        member: Option<String>,
    },
    /// Front door → hub: a member's `doc_set_name` (§8.5), on the member's own leg so the principal
    /// holds. Answered by one [`Msg::Event`] carrying `null`.
    SetName {
        /// The document.
        doc: String,
        /// The new name.
        name: String,
    },
    /// Front door → hub (E2): an agent's or owner-plugin's block edit (`Edit`) — the tools and the
    /// task-thread writer run on every engine, the helper transaction only on the hub. `edits` is the
    /// `Vec<BlockEdit>` as JSON (internally tagged: not postcard's). Answered by one [`Msg::Event`]
    /// carrying `Result<EditOutcome, EditError>` as JSON; an untyped failure is a [`Msg::Closed`].
    Edit {
        /// The document.
        doc: String,
        /// `Vec<BlockEdit>`, JSON.
        edits: Vec<u8>,
        /// Who the change stands for (`Agent{session}` from the tools, `Owner{device: None}` otherwise).
        by: Principal,
    },
    /// 184 §5.2, tag 10. Caller → would-be server: create a document (`req` = [`CreateReq`] JSON).
    /// No `doc`: the leg is keyed by the id the server derives the same way. Answered by one
    /// [`Msg::Event`] carrying `Result<DocSummary, String>` (a notebook's summary for
    /// `CreateReq::Notebook`). Refused on member streams.
    Create {
        /// [`CreateReq`], JSON.
        req: Vec<u8>,
    },
    /// 184 §4.3, tag 11. Claimant → source: hand `doc` back past `sv` (Yjs v1 state vector).
    /// Answered by `Result<HandbackReply, String>` JSON, or `Closed{not_home}`. Refused on member
    /// streams.
    Handback {
        /// The document.
        doc: String,
        /// The claimant's Yjs v1 state vector.
        sv: Vec<u8>,
    },
    /// 184 §6.2, tag 12. Reader → server: one code block and the note's markdown (`key` = `NoteKey`
    /// JSON; `seen` = the press's digest). Answered by `Result<CodeBlockTextReply, String>` JSON.
    /// Refused on member streams.
    Block {
        /// The document.
        doc: String,
        /// `NoteKey`, JSON.
        key: Vec<u8>,
        /// The digest the press carries.
        seen: Option<String>,
    },
    /// 184 §4.8, tag 13. Old server → new home: take `doc` (`meta` = `DocMeta` JSON, `home` = the
    /// receiver). Answered by `Result<i64, String>` JSON (the epoch once `Held`). Refused on member
    /// streams.
    Handover {
        /// The document.
        doc: String,
        /// `DocMeta`, JSON.
        meta: Vec<u8>,
    },
    /// 184 §7.3, tag 14. Front door → server: may the stream's principal (`Hello{member}`) read
    /// `doc`? Answered by `Result<(), String>` JSON. Allowed on member streams.
    MayRead {
        /// The document.
        doc: String,
    },
}

impl Msg {
    /// Frame tag byte (stable; new variants append).
    pub fn tag(&self) -> u8 {
        match self {
            Self::Open { .. } => 0,
            Self::Update { .. } => 1,
            Self::Awareness { .. } => 2,
            Self::Subscribe { .. } => 3,
            Self::Event { .. } => 4,
            Self::Join { .. } => 5,
            Self::Closed { .. } => 6,
            Self::Hello { .. } => 7,
            Self::SetName { .. } => 8,
            Self::Edit { .. } => 9,
            Self::Create { .. } => 10,
            Self::Handback { .. } => 11,
            Self::Block { .. } => 12,
            Self::Handover { .. } => 13,
            Self::MayRead { .. } => 14,
        }
    }

    /// One frame body: tag + postcard.
    pub fn encode(&self) -> anyhow::Result<Bytes> {
        let body = postcard::to_allocvec(self)?;
        let mut b = BytesMut::with_capacity(1 + body.len());
        b.extend_from_slice(&[self.tag()]);
        b.extend_from_slice(&body);
        Ok(b.freeze())
    }

    /// Inverse of [`Msg::encode`]; the tag must match the variant.
    pub fn decode(frame: &[u8]) -> anyhow::Result<Self> {
        let (&tag, body) = frame
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty frame"))?;
        let msg: Self = postcard::from_bytes(body)?;
        anyhow::ensure!(msg.tag() == tag, "frame tag {tag} does not match {msg:?}");
        Ok(msg)
    }
}

/// 184 §10.1: the body of [`Msg::Create`] (JSON), the create the would-be server runs as
/// `Relayed<..>` (§5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateReq {
    /// `Ensure` of a hub-only kind (knowledge, artifacts): answered with the bound document.
    Ensure {
        /// What it is bound to.
        binding: Binding,
        /// The derived file's current text.
        markdown: String,
    },
    /// `CreateNote` (`binding: Note`) or `Create`; `home` is the caller's `Homing` resolved
    /// (`Engine(<home>)` → `Some`, `Hub` → `None`), so `Here` keeps meaning the caller.
    Create {
        /// What it is bound to.
        binding: Binding,
        /// Display title.
        title: String,
        /// Initial body.
        markdown: String,
        /// Who created it.
        by: Principal,
        /// The new document's home.
        home: Option<mesh::EngineId>,
    },
    /// `NotebookCreate`.
    Notebook {
        /// Display title.
        title: String,
        /// The notebook's description (its body).
        description: String,
        /// Project, if any.
        project: Option<String>,
        /// Who created it.
        by: Principal,
    },
}

/// 184 §4.3 step 5: a source's answer to [`Msg::Handback`], as JSON in a [`Msg::Event`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandbackReply {
    /// base64 v1: the source's state past the claimant's `sv`.
    pub update: String,
    /// The source's meta.
    pub meta: DocMeta,
    /// The source's `agents.json` (block id → agent session).
    pub agents: std::collections::BTreeMap<String, uuid::Uuid>,
    /// The source's fence epoch.
    pub epoch: i64,
}

/// The codec every `theuth/docs/1` stream uses.
pub fn framed(
    stream: Box<dyn replica::ByteStream>,
) -> Framed<Box<dyn replica::ByteStream>, LengthDelimitedCodec> {
    Framed::new(
        stream,
        LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME)
            .new_codec(),
    )
}

type Wire = Framed<Box<dyn replica::ByteStream>, LengthDelimitedCodec>;

// ---------------------------------------------------------------------------------------------
// Front door: legs
// ---------------------------------------------------------------------------------------------

/// Who a leg carries: the owner on one local RPC connection, or a share-link member by static key
/// (a guest's `Join` names no connection, so every member call keys by the static).
// ponytail: two conns of one guest static share a leg and one hub-side conn id, so the hub skips
// both on either's updates; per-conn member legs if a device ever opens the same doc twice.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Client {
    /// The owner's connection id.
    Conn(Option<u64>),
    /// A member's static, base64url.
    Member(String),
}

impl Client {
    /// The leg a call from `by` rides.
    pub fn of(by: &Principal) -> Self {
        match by.member() {
            Some(p) => Self::Member(p.to_owned()),
            None => Self::Conn(by.conn()),
        }
    }

    fn member(&self) -> Option<String> {
        match self {
            Self::Member(p) => Some(p.clone()),
            Self::Conn(_) => None,
        }
    }
}

/// One hub-only call handed to a leg; each carries the caller's reply.
pub enum Cmd {
    /// `OpenDoc`.
    Open {
        /// The client's state vector.
        sv: Option<String>,
        /// Reply.
        reply: ReplySender<Result<DocOpen, DocError>>,
    },
    /// `Subscribe`: the leg owns the sender, the RPC subscription reads the receiver.
    Subscribe {
        /// Reply.
        reply: ReplySender<Result<mpsc::Receiver<DocEvent>, DocError>>,
    },
    /// `ApplyUpdate`; answered once written (a refusal arrives as `Closed` on the subscription).
    Update {
        /// base64 v1 update.
        update: String,
        /// Reply.
        reply: ReplySender<Result<(), DocError>>,
    },
    /// `AwarenessUpdateMsg`; answered once written.
    Awareness {
        /// base64 awareness update.
        state: String,
        /// Reply.
        reply: ReplySender<Result<(), DocError>>,
    },
    /// `Join`.
    Join {
        /// The join.
        join: Join,
        /// Reply.
        reply: ReplySender<Result<Joined, JoinError>>,
    },
    /// `SetName`.
    SetName {
        /// The new name.
        name: String,
        /// Reply.
        reply: ReplySender<Result<(), DocError>>,
    },
    /// `Edit` (E2): the block edits and the principal, answered with the hub's typed outcome.
    Edit {
        /// The edits, in order.
        edits: Vec<BlockEdit>,
        /// Who the change stands for.
        by: Principal,
        /// Reply.
        reply: ReplySender<Result<EditOutcome, EditError>>,
    },
    /// 184 `Create` (tag 10), on the leg keyed by the new document's id.
    Create {
        /// The create.
        req: CreateReq,
        /// Reply: the server's `DocSummary` (or notebook summary) as JSON.
        reply: ReplySender<Result<serde_json::Value, String>>,
    },
    /// 184 `Handback` (tag 11).
    Handback {
        /// The claimant's Yjs v1 state vector.
        sv: Vec<u8>,
        /// Reply.
        reply: ReplySender<Result<HandbackReply, DocError>>,
    },
    /// 184 `Block` (tag 12).
    Block {
        /// The cell.
        key: NoteKey,
        /// The digest the press carries.
        seen: Option<String>,
        /// Reply.
        reply: ReplySender<Result<CodeBlockTextReply, String>>,
    },
    /// 184 `Handover` (tag 13).
    Handover {
        /// The meta, `home` = the leg's target.
        meta: Box<DocMeta>,
        /// Reply: the target's epoch once `Held`.
        reply: ReplySender<Result<i64, String>>,
    },
    /// 184 `MayRead` (tag 14), on the member's own leg.
    MayRead {
        /// Reply.
        reply: ReplySender<Result<(), String>>,
    },
}

/// A reply the hub still owes, in send order.
enum Pending {
    Open(ReplySender<Result<DocOpen, DocError>>),
    Join(ReplySender<Result<Joined, JoinError>>),
    Unit(ReplySender<Result<(), DocError>>),
    Edit(ReplySender<Result<EditOutcome, EditError>>),
    Create(ReplySender<Result<serde_json::Value, String>>),
    Handback(ReplySender<Result<HandbackReply, DocError>>),
    Block(ReplySender<Result<CodeBlockTextReply, String>>),
    Handover(ReplySender<Result<i64, String>>),
    MayRead(ReplySender<Result<(), String>>),
}

impl Pending {
    fn answer(self, bytes: &[u8]) {
        fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, DocError> {
            serde_json::from_slice(bytes)
                .map_err(|e| DocError::Other(format!("bad reply from the hub: {e}")))
        }
        /// A `Result<T, String>` reply JSON; an undecodable one is an error.
        fn parse_res<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
            serde_json::from_slice::<Result<T, String>>(bytes)
                .unwrap_or_else(|e| Err(format!("bad reply from the hub: {e}")))
        }
        match self {
            Self::Open(r) => r.send(parse(bytes)),
            Self::Join(r) => r.send(parse(bytes).map_err(|e| JoinError::Other(e.to_string()))),
            Self::Unit(r) => r.send(Ok(())),
            Self::Edit(r) => {
                r.send(serde_json::from_slice(bytes).unwrap_or_else(|e| {
                    Err(EditError::Other(format!("bad reply from the hub: {e}")))
                }))
            }
            Self::Create(r) => r.send(parse_res(bytes)),
            Self::Handback(r) => r.send(parse_res(bytes).map_err(|e| DocError::parse(&e))),
            Self::Block(r) => r.send(parse_res(bytes)),
            Self::Handover(r) => r.send(parse_res(bytes)),
            Self::MayRead(r) => r.send(parse_res(bytes)),
        }
    }

    fn fail(self, reason: &str) {
        let e = DocError::parse(reason);
        match self {
            Self::Open(r) => r.send(Err(e)),
            Self::Handback(r) => r.send(Err(e)),
            Self::Unit(r) => r.send(Err(e)),
            Self::Edit(r) => r.send(Err(EditError::Other(e.to_string()))),
            Self::Join(r) => r.send(Err(match e {
                DocError::NotHub { hub } => JoinError::NotHub(hub),
                DocError::Other(s) => JoinError::Other(s),
            })),
            Self::Create(r) => r.send(Err(e.to_string())),
            Self::Block(r) => r.send(Err(e.to_string())),
            Self::Handover(r) => r.send(Err(e.to_string())),
            Self::MayRead(r) => r.send(Err(e.to_string())),
        }
    }
}

impl Cmd {
    /// Refuse without a leg: `NotHub{hub}` (the caller retries; the RPC layer types it).
    fn fail(self, hub: &EngineId) {
        let reason = DocError::NotHub {
            hub: Some(hub.clone()),
        }
        .to_string();
        match self {
            Self::Open { reply, .. } => Pending::Open(reply).fail(&reason),
            Self::Join { reply, .. } => Pending::Join(reply).fail(&reason),
            Self::Edit { reply, .. } => Pending::Edit(reply).fail(&reason),
            Self::Update { reply, .. }
            | Self::Awareness { reply, .. }
            | Self::SetName { reply, .. } => Pending::Unit(reply).fail(&reason),
            Self::Subscribe { reply } => reply.send(Err(DocError::parse(&reason))),
            Self::Create { reply, .. } => Pending::Create(reply).fail(&reason),
            Self::Handback { reply, .. } => Pending::Handback(reply).fail(&reason),
            Self::Block { reply, .. } => Pending::Block(reply).fail(&reason),
            Self::Handover { reply, .. } => Pending::Handover(reply).fail(&reason),
            Self::MayRead { reply } => Pending::MayRead(reply).fail(&reason),
        }
    }
}

/// The legs an actor holds towards other engines, by client and document (with each leg's
/// target, 184 §4.5), and the transport they open on.
#[derive(Default)]
pub struct Legs {
    transport: Option<Arc<dyn Transport>>,
    pub(crate) legs: HashMap<(Client, DocId), (EngineId, mpsc::Sender<Cmd>)>,
}

impl Legs {
    /// 184 `reachable(e)`: `e` is in the docs transport's connected peer set.
    pub fn reachable(&self, e: &EngineId) -> bool {
        self.transport
            .as_ref()
            .is_some_and(|t| t.peers().iter().any(|p| p.0 == e.0))
    }
}

/// `tell`: the mesh transport legs open on (from [`serve`] once the mesh registered one; tests
/// hand in a loopback). Until then every forward answers `NotHub{hub}`.
pub struct SetTransport(pub Arc<dyn Transport>);

impl Message<SetTransport> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: SetTransport, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.legs.transport = Some(m.0);
        self.reroute();
    }
}

impl DocumentActor {
    /// Hand `cmd` to the leg of (`client`, `doc`) towards `to`, opening it when there is none, the
    /// last one ended, or it points at another engine. A leg that cannot take it answers
    /// `NotHub{to}`.
    pub(crate) fn forward(&mut self, to: EngineId, doc: &DocId, client: Client, cmd: Cmd) {
        let key = (client, doc.clone());
        let cmd = match self
            .legs
            .legs
            .get(&key)
            .filter(|(t, tx)| *t == to && !tx.is_closed())
        {
            Some((_, tx)) => match tx.try_send(cmd) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Full(c)) => {
                    tracing::warn!(doc = %doc, "document forward leg is behind; refused");
                    return c.fail(&to);
                }
                Err(mpsc::error::TrySendError::Closed(c)) => c,
            },
            None => cmd,
        };
        let Some(t) = self.legs.transport.clone() else {
            tracing::debug!(doc = %doc, to = %to, "document forward: no mesh transport yet");
            return cmd.fail(&to);
        };
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(run_leg(t, to.clone(), doc.clone(), key.0.clone(), rx));
        let _ = tx.try_send(cmd);
        self.legs.legs.insert(key, (to, tx));
    }
}

/// One leg: open the stream, `Hello`, then commands out / frames in until either side ends it.
async fn run_leg(
    t: Arc<dyn Transport>,
    hub: EngineId,
    doc: DocId,
    client: Client,
    mut cmds: mpsc::Receiver<Cmd>,
) {
    let peer = PeerId(hub.0.clone());
    let opened = if t.peers().contains(&peer) {
        t.open(&peer, ALPN_DOCS).await.map_err(|e| e.to_string())
    } else {
        Err("hub not connected".to_owned())
    };
    let mut wire = match opened {
        Ok(s) => framed(s),
        Err(e) => {
            tracing::debug!(doc = %doc, hub = %hub, error = %e, "document forward: no stream to the hub");
            return drain(cmds, &hub);
        }
    };
    if send(
        &mut wire,
        &Msg::Hello {
            member: client.member(),
        },
    )
    .await
    .is_err()
    {
        return drain(cmds, &hub);
    }
    let id = doc.0.clone();
    let mut pending: VecDeque<Pending> = VecDeque::new();
    let mut sub: Option<mpsc::Sender<DocEvent>> = None;
    let reason = loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break HUB_MOVED.to_owned() };
                let (msg, owed) = match cmd {
                    Cmd::Open { sv, reply } => (Msg::Open { doc: id.clone(), sv }, Some(Pending::Open(reply))),
                    Cmd::Join { join, reply } => (
                        Msg::Join {
                            doc: id.clone(),
                            guest_pub: join.guest_pub,
                            handshake_hash: join.handshake_hash,
                            proof: join.proof,
                            name: join.name,
                        },
                        Some(Pending::Join(reply)),
                    ),
                    Cmd::SetName { name, reply } => (Msg::SetName { doc: id.clone(), name }, Some(Pending::Unit(reply))),
                    Cmd::Edit { edits, by, reply } => match serde_json::to_vec(&edits) {
                        Ok(edits) => (Msg::Edit { doc: id.clone(), edits, by }, Some(Pending::Edit(reply))),
                        Err(e) => {
                            reply.send(Err(EditError::Other(format!("bad edits: {e}"))));
                            continue;
                        }
                    },
                    Cmd::Subscribe { reply } => {
                        let (tx, rx) = mpsc::channel(QUEUE);
                        sub = Some(tx);
                        reply.send(Ok(rx));
                        (Msg::Subscribe { doc: id.clone() }, None)
                    }
                    Cmd::Update { update, reply } => {
                        if update.len() > MAX_UPDATE_B64 {
                            reply.send(Err(format!(
                                "UpdateTooLarge: one update may be {MAX_UPDATE_B64} bytes at most, not {}",
                                update.len()
                            )
                            .into()));
                            continue;
                        }
                        reply.send(Ok(()));
                        (Msg::Update { doc: id.clone(), bytes: update.into_bytes() }, None)
                    }
                    Cmd::Awareness { state, reply } => {
                        reply.send(Ok(()));
                        (Msg::Awareness { doc: id.clone(), bytes: state.into_bytes() }, None)
                    }
                    // 184 §10.1: JSON of plain data cannot fail; an empty body would be refused by
                    // the server.
                    Cmd::Create { req, reply } => (
                        Msg::Create { req: serde_json::to_vec(&req).unwrap_or_default() },
                        Some(Pending::Create(reply)),
                    ),
                    Cmd::Handback { sv, reply } => (Msg::Handback { doc: id.clone(), sv }, Some(Pending::Handback(reply))),
                    Cmd::Block { key, seen, reply } => (
                        Msg::Block { doc: id.clone(), key: serde_json::to_vec(&key).unwrap_or_default(), seen },
                        Some(Pending::Block(reply)),
                    ),
                    Cmd::Handover { meta, reply } => (
                        Msg::Handover { doc: id.clone(), meta: serde_json::to_vec(&meta).unwrap_or_default() },
                        Some(Pending::Handover(reply)),
                    ),
                    Cmd::MayRead { reply } => (Msg::MayRead { doc: id.clone() }, Some(Pending::MayRead(reply))),
                };
                if let Some(p) = owed {
                    pending.push_back(p);
                }
                if send(&mut wire, &msg).await.is_err() {
                    break HUB_MOVED.to_owned();
                }
            }
            frame = wire.next() => {
                let msg = frame.and_then(|f| f.ok()).and_then(|f| Msg::decode(&f).ok());
                match msg {
                    Some(Msg::Event { bytes, .. }) => match serde_json::from_slice::<DocEvent>(&bytes) {
                        Ok(ev) => {
                            let closed = matches!(ev, DocEvent::Closed { .. });
                            if sub.as_ref().is_some_and(|s| s.try_send(ev).is_err()) {
                                sub = None;
                            }
                            if closed {
                                break String::new();
                            }
                        }
                        Err(_) => match pending.pop_front() {
                            Some(p) => p.answer(&bytes),
                            None => tracing::debug!(doc = %doc, "document forward: unexpected reply frame"),
                        },
                    },
                    Some(Msg::Closed { reason, .. }) => break reason,
                    Some(other) => {
                        tracing::debug!(doc = %doc, frame = ?other, "document forward: hub sent a client frame");
                        break HUB_MOVED.to_owned();
                    }
                    None => break HUB_MOVED.to_owned(),
                }
            }
        }
    };
    // `String::new()`: the hub already sent `DocEvent::Closed` through the subscription.
    if !reason.is_empty()
        && let Some(s) = sub
    {
        let _ = s.try_send(DocEvent::Closed {
            reason: reason.clone(),
        });
    }
    let reason = if reason.is_empty() {
        HUB_MOVED.to_owned()
    } else {
        reason
    };
    for p in pending {
        p.fail(&reason);
    }
    drain(cmds, &hub);
}

/// Answer every command still queued with `NotHub{hub}`.
fn drain(mut cmds: mpsc::Receiver<Cmd>, hub: &EngineId) {
    cmds.close();
    while let Ok(c) = cmds.try_recv() {
        c.fail(hub);
    }
}

async fn send(wire: &mut Wire, msg: &Msg) -> anyhow::Result<()> {
    wire.send(msg.encode()?).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Hub: serve
// ---------------------------------------------------------------------------------------------

/// The hub side for the plugin: wait for the mesh transport, hand it to the actor's legs, then
/// [`serve_on`] until shutdown.
pub async fn serve(rt: Runtime, actor: ActorRef<DocumentActor>) {
    let Some(t) = replica::transport(&rt).await else {
        return;
    };
    let _ = actor.tell(SetTransport(t.clone())).await;
    serve_on(t, actor, rt.shutdown.clone()).await;
}

/// Accept `theuth/docs/1` streams on `transport` and bind each to `actor` as one remote client.
pub async fn serve_on(
    transport: std::sync::Arc<dyn Transport>,
    actor: ActorRef<DocumentActor>,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => return,
            a = transport.accept(ALPN_DOCS) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                let actor = actor.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_stream(&actor, framed(stream), &peer).await {
                        tracing::debug!(%peer, error = %e, "document stream ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "document stream accept");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// 184 §5.2: a forwarded create, run here as `Relayed` (`home` as the caller resolved it); the
/// reply body. `Ensure` answers the bound document's summary.
async fn create(
    actor: &ActorRef<DocumentActor>,
    req: CreateReq,
) -> Result<serde_json::Value, String> {
    let homing = |h: Option<EngineId>| h.map_or(Homing::Hub, Homing::Engine);
    let value = match req {
        CreateReq::Ensure { binding, markdown } => {
            let doc = actor
                .ask(Relayed(Ensure { binding, markdown }))
                .await
                .map_err(reason_of)?;
            let by = Principal::Owner { device: None };
            let meta = actor.ask(MetaOf { doc, by }).await.map_err(reason_of)?;
            serde_json::to_value(DocSummary::from(&meta))
        }
        CreateReq::Create {
            binding: Binding::Note,
            title,
            markdown,
            by,
            home,
        } => {
            let m = CreateNote {
                title,
                markdown,
                by,
                home: homing(home),
            };
            serde_json::to_value(actor.ask(Relayed(m)).await.map_err(reason_of)?)
        }
        CreateReq::Create {
            binding,
            title,
            markdown,
            by,
            home,
        } => {
            let m = Create {
                binding,
                title,
                markdown,
                by,
                home: homing(home),
            };
            serde_json::to_value(actor.ask(Relayed(m)).await.map_err(reason_of)?)
        }
        CreateReq::Notebook {
            title,
            description,
            project,
            by,
        } => {
            let m = NotebookCreate {
                title,
                description,
                project,
                by,
            };
            serde_json::to_value(actor.ask(Relayed(m)).await.map_err(reason_of)?)
        }
    };
    value.map_err(|e| e.to_string())
}

/// The actor's answer as a `Closed` reason: the handler's error, or why the actor did not answer.
fn reason_of<M, E: std::fmt::Display>(e: kameo::error::SendError<M, E>) -> String {
    match e {
        kameo::error::SendError::HandlerError(e) => e.to_string(),
        other => other.to_string(),
    }
}

/// One accepted stream from `peer`: frames in → actor messages as the connection `conn`, each
/// wrapped in [`Relayed`] (184 §10.1); replies and the subscription's events out. Ends on the first
/// refusal (after `Closed`), a protocol error or EOF.
async fn serve_stream(
    actor: &ActorRef<DocumentActor>,
    mut wire: Wire,
    peer: &PeerId,
) -> anyhow::Result<()> {
    // A connection id of this process, like `RpcPeer` mints them: fan-out skips this stream's own updates.
    let conn = Some(rand::random::<u64>());
    let mut by = Principal::Owner { device: conn };
    let (out_tx, mut out_rx) = mpsc::channel::<DocEvent>(QUEUE);
    let mut doc_of = String::new();
    let parse = |s: &str| DocId::parse(s).ok_or_else(|| anyhow::anyhow!("bad document id {s:?}"));
    loop {
        tokio::select! {
            ev = out_rx.recv() => {
                let Some(ev) = ev else { return Ok(()) };
                let closed = matches!(ev, DocEvent::Closed { .. });
                if let DocEvent::Closed { reason } = ev {
                    send(&mut wire, &Msg::Closed { doc: doc_of.clone(), reason }).await?;
                } else {
                    send(&mut wire, &Msg::Event { doc: doc_of.clone(), bytes: serde_json::to_vec(&ev)? }).await?;
                }
                if closed {
                    return Ok(());
                }
            }
            frame = wire.next() => {
                let Some(frame) = frame else { return Ok(()) };
                let msg = Msg::decode(&frame?)?;
                let outcome: Result<Option<Vec<u8>>, String> = match msg {
                    Msg::Hello { member } => {
                        by = match member {
                            Some(pub_) => Principal::Member { pub_, device: conn },
                            None => Principal::Owner { device: conn },
                        };
                        Ok(None)
                    }
                    Msg::Open { doc, sv } => {
                        doc_of = doc.clone();
                        actor
                            .ask(Relayed(OpenDoc { doc: parse(&doc)?, sv, by: by.clone() }))
                            .await
                            .map(|o| serde_json::to_vec(&o).ok())
                            .map_err(reason_of)
                    }
                    Msg::Subscribe { doc } => {
                        doc_of = doc.clone();
                        match actor.ask(Relayed(Subscribe { doc: parse(&doc)?, by: by.clone() })).await {
                            Ok(mut rx) => {
                                let out = out_tx.clone();
                                tokio::spawn(async move {
                                    while let Some(ev) = rx.recv().await {
                                        if out.send(ev).await.is_err() {
                                            break;
                                        }
                                    }
                                });
                                Ok(None)
                            }
                            Err(e) => Err(reason_of(e)),
                        }
                    }
                    Msg::Update { doc, bytes } => {
                        doc_of = doc.clone();
                        let update = String::from_utf8(bytes).map_err(|_| "bad update".to_owned());
                        match update {
                            Ok(update) => actor
                                .ask(Relayed(ApplyUpdate { doc: parse(&doc)?, update, by: by.clone() }))
                                .await
                                .map(|()| None)
                                .map_err(reason_of),
                            Err(e) => Err(e),
                        }
                    }
                    Msg::Awareness { doc, bytes } => {
                        doc_of = doc.clone();
                        if let Ok(state) = String::from_utf8(bytes)
                            && let Err(e) = actor.ask(Relayed(AwarenessUpdateMsg { doc: parse(&doc)?, state, by: by.clone() })).await
                        {
                            tracing::debug!(doc = %doc, error = %reason_of(e), "forwarded awareness dropped");
                        }
                        Ok(None)
                    }
                    Msg::Join { doc, guest_pub, handshake_hash, proof, name } => {
                        doc_of = doc.clone();
                        actor
                            .ask(Relayed(Join { doc: parse(&doc)?, guest_pub, handshake_hash, proof, name }))
                            .await
                            .map(|j| serde_json::to_vec(&j).ok())
                            .map_err(reason_of)
                    }
                    Msg::SetName { doc, name } => {
                        doc_of = doc.clone();
                        actor
                            .ask(Relayed(SetName { doc: parse(&doc)?, by: by.clone(), name }))
                            .await
                            .map(|()| Some(b"null".to_vec()))
                            .map_err(reason_of)
                    }
                    Msg::Edit { doc, edits, by: who } => {
                        doc_of = doc.clone();
                        // The helper's typed refusals ride back as `Err` in the reply so the leg lives on;
                        // an untyped failure (`Other`, not serialisable) closes it like any handler error.
                        match serde_json::from_slice::<Vec<BlockEdit>>(&edits) {
                            Err(e) => Err(format!("bad edits: {e}")),
                            Ok(edits) => match actor.ask(Relayed(Edit { doc: parse(&doc)?, edits, by: who })).await {
                                Ok(o) => Ok(serde_json::to_vec(&Ok::<_, EditError>(o)).ok()),
                                Err(kameo::error::SendError::HandlerError(EditError::Other(s))) => Err(s),
                                Err(kameo::error::SendError::HandlerError(e)) => {
                                    Ok(serde_json::to_vec(&Err::<EditOutcome, _>(e)).ok())
                                }
                                Err(other) => Err(other.to_string()),
                            },
                        }
                    }
                    // 184 §10.1: owner-side calls; a member stream may not send them.
                    Msg::Create { .. } | Msg::Handback { .. } | Msg::Block { .. } | Msg::Handover { .. }
                        if by.member().is_some() =>
                    {
                        Err(NOT_OWNER.to_owned())
                    }
                    Msg::Create { req } => match serde_json::from_slice::<CreateReq>(&req) {
                        Err(e) => Err(format!("bad create: {e}")),
                        Ok(req) => Ok(Some(serde_json::to_vec(&create(actor, req).await)?)),
                    },
                    Msg::Handback { doc, sv } => {
                        doc_of = doc.clone();
                        let from = EngineId(peer.0.clone());
                        actor
                            .ask(Relayed(Handback { doc: parse(&doc)?, sv, from }))
                            .await
                            .map(|r| serde_json::to_vec(&Ok::<_, String>(r)).ok())
                            .map_err(reason_of)
                    }
                    Msg::Block { doc, key, seen } => {
                        doc_of = doc.clone();
                        let r = match serde_json::from_slice::<NoteKey>(&key) {
                            Err(e) => Err(format!("bad key: {e}")),
                            Ok(key) => actor
                                .ask(Relayed(CodeBlockText { doc: parse(&doc)?, key, seen }))
                                .await
                                .map_err(reason_of),
                        };
                        Ok(Some(serde_json::to_vec(&r)?))
                    }
                    Msg::Handover { doc, meta } => {
                        doc_of = doc.clone();
                        let r = match serde_json::from_slice::<DocMeta>(&meta) {
                            Err(e) => Err(format!("bad meta: {e}")),
                            Ok(meta) => actor
                                .ask(Handover { doc: parse(&doc)?, meta, from: EngineId(peer.0.clone()) })
                                .await
                                .map_err(reason_of),
                        };
                        Ok(Some(serde_json::to_vec(&r)?))
                    }
                    // §7.3: the stream's principal (`Hello{member}`) asks; a guest's leg may.
                    Msg::MayRead { doc } => {
                        doc_of = doc.clone();
                        let r = actor
                            .ask(Relayed(MayRead { doc: parse(&doc)?, by: by.clone() }))
                            .await
                            .map_err(reason_of);
                        Ok(Some(serde_json::to_vec(&r)?))
                    }
                    Msg::Event { .. } | Msg::Closed { .. } => anyhow::bail!("client sent a hub frame"),
                };
                match outcome {
                    Ok(Some(bytes)) => send(&mut wire, &Msg::Event { doc: doc_of.clone(), bytes }).await?,
                    Ok(None) => {}
                    Err(reason) => {
                        send(&mut wire, &Msg::Closed { doc: doc_of.clone(), reason }).await?;
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_with_matching_tag() {
        let msgs = vec![
            Msg::Open {
                doc: "d".into(),
                sv: Some("AA==".into()),
            },
            Msg::Update {
                doc: "d".into(),
                bytes: b"AQID".to_vec(),
            },
            Msg::Awareness {
                doc: "d".into(),
                bytes: vec![1, 2],
            },
            Msg::Subscribe { doc: "d".into() },
            Msg::Event {
                doc: "d".into(),
                bytes: br#"{"type":"Exported"}"#.to_vec(),
            },
            Msg::Join {
                doc: "d".into(),
                guest_pub: [7; 32],
                handshake_hash: vec![1; 32],
                proof: vec![2; 32],
                name: None,
            },
            Msg::Closed {
                doc: "d".into(),
                reason: crate::hub::HUB_MOVED.into(),
            },
            Msg::Hello {
                member: Some("pub".into()),
            },
            Msg::SetName {
                doc: "d".into(),
                name: "n".into(),
            },
            Msg::Edit {
                doc: "d".into(),
                edits: br#"[{"op":"delete","blocks":["b1"]}]"#.to_vec(),
                by: Principal::Agent {
                    session: uuid::Uuid::from_u128(7),
                },
            },
            // 184 §10.1: tags 10-14.
            Msg::Create {
                req: b"{}".to_vec(),
            },
            Msg::Handback {
                doc: "d".into(),
                sv: vec![0],
            },
            Msg::Block {
                doc: "d".into(),
                key: br#"{"kind":"name","value":"a"}"#.to_vec(),
                seen: Some("sha".into()),
            },
            Msg::Handover {
                doc: "d".into(),
                meta: b"{}".to_vec(),
            },
            Msg::MayRead { doc: "d".into() },
        ];
        for (i, m) in msgs.iter().enumerate() {
            let f = m.encode().unwrap();
            assert_eq!(f[0], i as u8, "{m:?}");
            assert_eq!(&Msg::decode(&f).unwrap(), m);
        }
        let mut wrong = msgs[0].encode().unwrap().to_vec();
        wrong[0] = 6;
        assert!(Msg::decode(&wrong).is_err());
        assert!(Msg::decode(&[]).is_err());
    }

    use std::collections::HashMap as Map;
    use std::path::Path;

    use base64::Engine as _;
    use kameo::actor::Spawn;
    use mesh::{LeaseState, MeshEvent};
    use replica::{ByteStream, PeerEvent};
    use tokio::sync::broadcast;
    use yrs::updates::decoder::Decode;
    use yrs::updates::encoder::Encode;
    use yrs::{Doc, GetString, ReadTxn, Text, Transact, Update};

    use crate::meta::{AgentStyle, Binding, DocKind, DocMeta, Who};
    use crate::store::Store;
    use crate::{DocumentsConfig, hub};

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Loopback switch, as `rpc::forward`'s tests; `cut` hides peers from this node (184: a home
    /// offline as seen from here).
    struct Loop {
        me: PeerId,
        inboxes: Arc<parking_lot::Mutex<Map<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
        cut: parking_lot::Mutex<Vec<PeerId>>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let mut n = nodes(&["a", "b"]);
        let b = n.pop().unwrap();
        (n.pop().unwrap(), b)
    }

    fn nodes(names: &[&str]) -> Vec<Arc<Loop>> {
        let inboxes = Arc::new(parking_lot::Mutex::new(Map::new()));
        names
            .iter()
            .map(|name| {
                let me = PeerId((*name).to_owned());
                let (tx, rx) = mpsc::unbounded_channel();
                inboxes.lock().insert(me.clone(), tx);
                Arc::new(Loop {
                    me,
                    inboxes: inboxes.clone(),
                    inbox: tokio::sync::Mutex::new(rx),
                    events: broadcast::channel(8).0,
                    cut: Default::default(),
                })
            })
            .collect()
    }

    impl Loop {
        fn set_cut(&self, peers: &[&str]) {
            *self.cut.lock() = peers.iter().map(|p| PeerId((*p).to_owned())).collect();
        }
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            let cut = self.cut.lock();
            self.inboxes
                .lock()
                .keys()
                .filter(|p| **p != self.me && !cut.contains(p))
                .cloned()
                .collect()
        }

        async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            assert_eq!(alpn, ALPN_DOCS);
            anyhow::ensure!(!self.cut.lock().contains(peer), "cut off");
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self
                .inboxes
                .lock()
                .get(peer)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    /// Tests that register `DOCUMENT_ACTOR` (the hub's `actor_ref`) run one at a time.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Register `a` as the process's document actor; a name left by an earlier test's hub (its
    /// runtime tore the actor down without unregistering) is replaced. Callers hold `SERIAL`.
    fn register_hub(a: &ActorRef<DocumentActor>) {
        kameo::registry::ACTOR_REGISTRY
            .lock()
            .unwrap()
            .remove(crate::DOCUMENT_ACTOR);
        a.register(crate::DOCUMENT_ACTOR).unwrap();
    }

    /// An actor over `data_dir` with no helper (nothing here imports or exports).
    async fn actor(data_dir: &Path) -> (ActorRef<DocumentActor>, Store) {
        actor_with(data_dir, &data_dir.join("no-helper")).await
    }

    /// An actor over `data_dir` whose helper is `bin`; its events reach the actor as in the plugin.
    async fn actor_with(data_dir: &Path, bin: &Path) -> (ActorRef<DocumentActor>, Store) {
        let rt = Runtime::new(figment::Figment::new(), data_dir.to_path_buf());
        let store = Store::open(data_dir).await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let helper = crate::helper::Helper::new(bin.to_path_buf(), tx, rt.shutdown.clone());
        let a = DocumentActor::spawn(DocumentActor::new(
            store.clone(),
            helper,
            DocumentsConfig::default(),
            rt,
        ));
        let me = a.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let _ = me.tell(ev).await;
            }
        });
        (a, store)
    }

    /// A `sh` helper: `edit` answers with one canned update (`update_b64`, a v1 update on the
    /// document's `t` text) as the transaction and `inserted: [b1]`; `import` changes nothing; every
    /// other op is `ok`.
    fn edit_helper(dir: &Path, update_b64: &str) -> std::path::PathBuf {
        let p = dir.join("edit-helper.sh");
        std::fs::write(
            &p,
            format!(
                r#"#!/bin/sh
if [ "$1" = --version ]; then echo "theuth-documents {}"; exit 0; fi
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  doc=$(printf '%s' "$line" | sed -n 's/.*"doc":"\([a-z0-9]*\)".*/\1/p')
  case "$line" in
    *'"op":"edit"'*)
      printf '{{"ev":"update","doc":"%s","update":"{update_b64}","touched":["b1"],"req":%s}}\n' "$doc" "$id"
      printf '{{"id":%s,"ok":true,"result":{{"inserted":["b1"],"removed":[],"touched":["b1"],"undo":[]}}}}\n' "$id" ;;
    *'"op":"import"'*) printf '{{"id":%s,"ok":true,"result":{{"changed":0,"total":0}}}}\n' "$id" ;;
    *) printf '{{"id":%s,"ok":true,"result":{{}}}}\n' "$id" ;;
  esac
done
"#,
                crate::helper::protocol_version()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn note(id: DocId) -> DocMeta {
        DocMeta {
            derived: Binding::Note.derived(&id),
            id,
            kind: Binding::Note.kind(),
            title: "t".into(),
            binding: Binding::Note,
            fragment: crate::FRAGMENT.into(),
            members: vec![],
            links: vec![],
            agent: AgentStyle::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: 1,
            updated_ms: 1,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        }
    }

    fn lease(holder: &str) -> MeshEvent {
        MeshEvent::Lease(LeaseState {
            holder: Some(EngineId(holder.into())),
            since_ms: 0,
            reason: "test".into(),
        })
    }

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    /// A base64 v1 update appending `s` to text `t` of `doc`.
    fn typed(doc: &Doc, s: &str) -> String {
        let t = doc.get_or_insert_text("t");
        let mut txn = doc.transact_mut();
        let len = t.len(&txn);
        t.insert(&mut txn, len, s);
        b64(&txn.encode_update_v1())
    }

    /// Text `t` of the state a `DocOpen` carries.
    fn text_of(opened: &DocOpen) -> String {
        let mut bytes = Vec::new();
        for c in &opened.state {
            bytes.extend(base64::engine::general_purpose::STANDARD.decode(c).unwrap());
        }
        let doc = Doc::new();
        doc.transact_mut()
            .apply_update(Update::decode_v1(&bytes).unwrap())
            .unwrap();
        let t = doc.get_or_insert_text("t");
        t.get_string(&doc.transact())
    }

    async fn next(rx: &mut mpsc::Receiver<DocEvent>) -> DocEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event within 5 s")
            .expect("open")
    }

    fn owner(device: u64) -> Principal {
        Principal::Owner {
            device: Some(device),
        }
    }

    #[tokio::test]
    async fn forward_open_update_awareness_round_trip_then_failover_without_duplication() {
        let _serial = SERIAL.lock().await;
        let (ta, tb) = pair();
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, store_a) = actor(da.path()).await;
        let (b, store_b) = actor(db.path()).await;
        // A is the hub (a lone follower holds; `Reconcile` takes the duties as the plugin does at
        // start); B follows a lease held by "a" and can reach it.
        // A's loss path fires pending exports through the registered name, as in the plugin.
        register_hub(&a);
        a.tell(hub::Reconcile).await.unwrap();
        b.tell(hub::SelfId(EngineId("b".into()))).await.unwrap();
        b.tell(lease("a")).await.unwrap();
        b.tell(SetTransport(tb.clone())).await.unwrap();
        let cancel = CancellationToken::new();
        tokio::spawn(serve_on(ta.clone(), a.clone(), cancel.clone()));
        let doc = DocId::new();
        store_a.create(&note(doc.clone())).await.unwrap();

        // A local subscriber on the hub, and two clients through B (devices 7 and 8).
        let mut sub_a = a
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(1),
            })
            .await
            .unwrap();
        let mut sub_7 = b
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(7),
            })
            .await
            .unwrap();
        let mut sub_8 = b
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(8),
            })
            .await
            .unwrap();
        let opened = b
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(7),
            })
            .await
            .unwrap();
        assert_eq!(
            (opened.meta.id.clone(), text_of(&opened)),
            (doc.clone(), String::new())
        );

        // 7 types through B: the hub applies it; the hub's subscriber and 8 hear it, 7 does not (own conn).
        let mine = Doc::new();
        b.ask(ApplyUpdate {
            doc: doc.clone(),
            update: typed(&mine, "hello "),
            by: owner(7),
        })
        .await
        .unwrap();
        assert!(matches!(
            next(&mut sub_a).await,
            DocEvent::Update { by: Who::Owner, .. }
        ));
        assert!(matches!(next(&mut sub_8).await, DocEvent::Update { .. }));
        b.ask(ApplyUpdate {
            doc: doc.clone(),
            update: typed(&mine, "world"),
            by: owner(7),
        })
        .await
        .unwrap();
        assert!(matches!(next(&mut sub_a).await, DocEvent::Update { .. }));
        assert!(matches!(next(&mut sub_8).await, DocEvent::Update { .. }));

        // Awareness rides the same leg and fans out like an update.
        let mut aw = yrs::sync::Awareness::new(Doc::new());
        aw.set_local_state_raw(r#"{"cursor":1}"#);
        let state = b64(&aw.update().unwrap().encode_v1());
        b.ask(AwarenessUpdateMsg {
            doc: doc.clone(),
            state: state.clone(),
            by: owner(7),
        })
        .await
        .unwrap();
        assert_eq!(
            next(&mut sub_a).await,
            DocEvent::Awareness {
                state: state.clone()
            }
        );
        assert_eq!(next(&mut sub_8).await, DocEvent::Awareness { state });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            sub_7.try_recv().is_err(),
            "7's own updates are not echoed to 7"
        );

        // A fresh open through B carries what the hub holds.
        let opened = b
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(9),
            })
            .await
            .unwrap();
        assert_eq!(text_of(&opened), "hello world");

        // Too large is refused at the front door, before any leg.
        let e = b
            .ask(ApplyUpdate {
                doc: doc.clone(),
                update: "x".repeat(MAX_UPDATE_B64 + 1),
                by: owner(7),
            })
            .await
            .unwrap_err();
        assert!(e.to_string().starts_with("UpdateTooLarge"), "{e}");

        // Failover: the lease leaves A. Its subscribers and every forwarded one hear `hub_moved`;
        // A snapshotted on loss, B gets the files (the mirror's job) and the lease, and a reopen on B
        // serves the same text once — same id, one Yjs lineage, nothing duplicated.
        a.tell(hub::SelfId(EngineId("a".into()))).await.unwrap();
        a.tell(lease("b")).await.unwrap();
        assert_eq!(next(&mut sub_a).await, hub::closed());
        assert_eq!(next(&mut sub_7).await, hub::closed());
        assert_eq!(next(&mut sub_8).await, hub::closed());
        store_b.create(&note(doc.clone())).await.unwrap();
        std::fs::copy(
            da.path().join(format!("documents/{doc}/snapshot.yjs")),
            db.path().join(format!("documents/{doc}/snapshot.yjs")),
        )
        .unwrap();
        b.tell(lease("b")).await.unwrap();
        let opened = b
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(7),
            })
            .await
            .unwrap();
        assert_eq!(text_of(&opened), "hello world");
        // A, now off the hub, forwards to B: the client's own state vector asks for nothing new.
        a.tell(SetTransport(ta.clone())).await.unwrap();
        tokio::spawn(serve_on(tb.clone(), b.clone(), cancel.clone()));
        let sv = b64(&{
            let d = Doc::new();
            let mut bytes = Vec::new();
            for c in &opened.state {
                bytes.extend(base64::engine::general_purpose::STANDARD.decode(c).unwrap());
            }
            d.transact_mut()
                .apply_update(Update::decode_v1(&bytes).unwrap())
                .unwrap();
            d.transact().state_vector().encode_v1()
        });
        let again = a
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: Some(sv),
                by: owner(1),
            })
            .await
            .unwrap();
        assert_eq!(
            text_of(&again),
            "",
            "nothing missing from a client that has it all"
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn forward_refuses_with_not_hub_when_the_hub_cannot_be_reached() {
        let (_ta, tb) = pair();
        let dir = tempfile::tempdir().unwrap();
        let (b, _) = actor(dir.path()).await;
        b.tell(hub::SelfId(EngineId("b".into()))).await.unwrap();
        b.tell(lease("a")).await.unwrap();
        let doc = DocId::new();
        // No transport yet.
        let e = b
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(1),
            })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "NotHub:a");
        // A transport that does not see "a" (a holder that is not connected).
        b.tell(SetTransport(tb.clone())).await.unwrap();
        b.tell(lease("zz")).await.unwrap();
        let e = b
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(1),
            })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "NotHub:zz");
        let e = b
            .ask(Join {
                doc: doc.clone(),
                guest_pub: [1; 32],
                handshake_hash: vec![2; 32],
                proof: vec![3; 32],
                name: None,
            })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "NotHub:zz");
        // E2: an agent edit off the hub is refused the same way — an error the tool shows, not a drop.
        let e = b
            .ask(Edit {
                doc,
                edits: vec![BlockEdit::Delete {
                    blocks: vec!["b1".into()],
                }],
                by: agent(),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&e, kameo::error::SendError::HandlerError(EditError::Other(s)) if s == "NotHub:zz"),
            "{e}"
        );
    }

    fn agent() -> Principal {
        Principal::Agent {
            session: uuid::Uuid::from_u128(42),
        }
    }

    /// E2 brief 4: the tools run on every engine; an `Edit` on the non-hub rides the leg, the hub's
    /// helper transaction lands in the hub's `Doc`, and the non-hub's mirror catches up once the hub
    /// snapshots (the replica's copy is done by hand, as in the other round trips).
    #[tokio::test]
    async fn forward_edit_lands_on_the_hub_and_the_mirror_catches_up() {
        let _serial = SERIAL.lock().await;
        let (ta, tb) = pair();
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // The canned transaction the fake helper answers every `edit` with.
        let typed_b64 = typed(&Doc::new(), "hello from the agent");
        let bin = edit_helper(da.path(), &typed_b64);
        let (a, store_a) = actor_with(da.path(), &bin).await;
        let (b, store_b) = actor(db.path()).await;
        register_hub(&a);
        a.tell(hub::Reconcile).await.unwrap();
        b.tell(hub::SelfId(EngineId("b".into()))).await.unwrap();
        b.tell(lease("a")).await.unwrap();
        b.tell(SetTransport(tb.clone())).await.unwrap();
        let cancel = CancellationToken::new();
        tokio::spawn(serve_on(ta.clone(), a.clone(), cancel.clone()));
        let doc = DocId::new();
        store_a.create(&note(doc.clone())).await.unwrap();

        // Through B: the hub's typed outcome comes back; the hub's Doc holds the text; B's does not yet.
        let edits = vec![BlockEdit::Insert {
            markdown: "hello from the agent".into(),
            after: None,
            before: None,
            at: Some("end".into()),
        }];
        let out = b
            .ask(Edit {
                doc: doc.clone(),
                edits,
                by: agent(),
            })
            .await
            .unwrap();
        assert_eq!(out.inserted, vec!["b1".to_owned()]);
        let opened = a
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(1),
            })
            .await
            .unwrap();
        assert_eq!(text_of(&opened), "hello from the agent");
        assert!(
            store_b.load(&doc).is_err(),
            "nothing on B before the mirror"
        );

        // The hub snapshots (quiet period / unload in the plugin); its mirror files reach B as the
        // replica would copy them; B takes them on disk and now reads the same text — from the hub's
        // very bytes, so B's own mirror file equals its row and nothing ships back.
        a.ask(crate::actor::Snapshot {
            doc: doc.clone(),
            label: "test".into(),
        })
        .await
        .unwrap();
        for kind in [
            crate::mirror::MirrorFile::Meta,
            crate::mirror::MirrorFile::Snapshot,
        ] {
            let name = crate::mirror::file_name(&doc, kind);
            std::fs::copy(
                store_a.mirror_dir().join(&name),
                store_b.mirror_dir().join(&name),
            )
            .unwrap();
            b.tell(crate::actor::MirrorChanged {
                path: store_b.mirror_dir().join(&name),
            })
            .await
            .unwrap();
        }
        let stats = b.ask(crate::actor::Stats).await.unwrap(); // a barrier: the tells above ran
        assert_eq!(stats.resnapshots, 0, "{stats:?}");
        let loaded = store_b.load(&doc).unwrap();
        assert_eq!(
            loaded
                .doc
                .get_or_insert_text("t")
                .get_string(&loaded.doc.transact()),
            "hello from the agent"
        );
        let snap =
            |d: &Path| std::fs::read(d.join(format!("documents/{doc}/snapshot.yjs"))).unwrap();
        assert_eq!(
            snap(db.path()),
            snap(da.path()),
            "B holds the hub's bytes, not a re-encoding"
        );
        cancel.cancel();
    }

    // ---- 184: notes on any machine -----------------------------------------------------------

    fn eid(s: &str) -> EngineId {
        EngineId(s.into())
    }

    /// A note homed on `home`, which last served it at fence epoch 1.
    fn homed(id: DocId, home: &str) -> DocMeta {
        DocMeta {
            home: Some(eid(home)),
            home_ms: 1,
            serve_epoch: 1,
            served_by: Some(eid(home)),
            ..note(id)
        }
    }

    /// A document of `binding` (its kind) with no home.
    fn bound(binding: Binding) -> DocMeta {
        let id = DocId::for_binding(&binding);
        DocMeta {
            derived: binding.derived(&id),
            kind: binding.kind(),
            binding,
            ..note(id)
        }
    }

    /// Engine `name` on `t` (a fake helper that imports), homes on, the lease held by `holder`,
    /// its `theuth/docs/1` streams served until `cancel`.
    async fn engine(
        name: &str,
        t: &Arc<Loop>,
        holder: &str,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> (ActorRef<DocumentActor>, Store) {
        let bin = edit_helper(dir, &typed(&Doc::new(), "x"));
        let (a, store) = actor_with(dir, &bin).await;
        a.tell(hub::SelfId(eid(name))).await.unwrap();
        a.tell(lease(holder)).await.unwrap();
        a.tell(SetTransport(t.clone())).await.unwrap();
        a.tell(hub::HomesOn(true)).await.unwrap();
        tokio::spawn(serve_on(t.clone(), a.clone(), cancel.clone()));
        (a, store)
    }

    async fn server(
        a: &ActorRef<DocumentActor>,
        doc: &DocId,
    ) -> Result<Option<EngineId>, DocError> {
        a.ask(hub::DocServer { doc: doc.clone() })
            .await
            .map_err(|e| DocError::parse(&e.to_string()))
    }

    /// §4.2 rules 2-6, read through `DocServer` (never claims) and relayed calls.
    #[tokio::test]
    async fn route_doc_rules_one_to_six() {
        let n = nodes(&["a", "b", "c"]);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cancel = CancellationToken::new();
        let (a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (b, store_b) = engine("b", &n[1], "a", db.path(), &cancel).await;
        n[0].set_cut(&["c"]);
        n[1].set_cut(&["c"]);
        let (unhomed, mine, on_a, on_c) = (DocId::new(), DocId::new(), DocId::new(), DocId::new());
        for s in [&store_a, &store_b] {
            s.create(&note(unhomed.clone())).await.unwrap();
            s.create(&homed(mine.clone(), "b")).await.unwrap();
            s.create(&homed(on_a.clone(), "a")).await.unwrap();
            s.create(&homed(on_c.clone(), "c")).await.unwrap();
        }
        // Rule 2: no home → the holder, as today. Rule 3: home = this engine. Rule 4: home
        // reachable. Rule 5: home unreachable → the holder; on the holder itself, the fallback.
        assert_eq!(server(&b, &unhomed).await, Ok(Some(eid("a"))));
        assert_eq!(server(&b, &mine).await, Ok(None));
        assert_eq!(server(&b, &on_a).await, Ok(Some(eid("a"))));
        assert_eq!(server(&b, &on_c).await, Ok(Some(eid("a"))));
        assert_eq!(server(&a, &on_c).await, Ok(None), "the holder falls back");
        assert_eq!(server(&a, &mine).await, Ok(Some(eid("b"))));
        // Rule 6: a relayed call is never forwarded again, except by the holder to the home.
        let open = |doc: &DocId| OpenDoc {
            doc: doc.clone(),
            sv: None,
            by: owner(1),
        };
        let e = b.ask(Relayed(open(&on_a))).await.unwrap_err();
        assert_eq!(e.to_string(), "NotHub:a");
        let e = b.ask(Relayed(open(&unhomed))).await.unwrap_err();
        assert_eq!(e.to_string(), "NotHub:a");
        let relayed = a.ask(Relayed(open(&mine))).await.unwrap();
        assert_eq!(
            relayed.server,
            Some(eid("b")),
            "the holder relayed to the home"
        );
        // Homes off: every document takes today's path; nobody holds the lease: refused.
        b.tell(hub::HomesOn(false)).await.unwrap();
        assert_eq!(server(&b, &mine).await, Ok(Some(eid("a"))));
        b.tell(hub::HomesOn(true)).await.unwrap();
        b.tell(MeshEvent::Lease(LeaseState {
            holder: None,
            since_ms: 0,
            reason: "test".into(),
        }))
        .await
        .unwrap();
        assert_eq!(server(&b, &on_c).await, Err(DocError::NotHub { hub: None }));
        assert_eq!(
            server(&b, &mine).await,
            Ok(None),
            "a home serves while cut off (W-q)"
        );
        cancel.cancel();
    }

    /// §4.3: the home claims before it serves: the fallback's edits come back in the hand-back,
    /// the fallback's editors hear `server_moved`, the parked open answers from the merged Doc,
    /// and the fence names the home above every epoch it saw. Then §4.8: Move to hands it over.
    #[tokio::test]
    async fn a_home_takes_the_fallbacks_edits_before_serving_and_move_hands_over() {
        let n = nodes(&["a", "b"]);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cancel = CancellationToken::new();
        let (a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (b, store_b) = engine("b", &n[1], "a", db.path(), &cancel).await;
        let doc = DocId::new();
        store_a.create(&homed(doc.clone(), "b")).await.unwrap();
        store_b.create(&homed(doc.clone(), "b")).await.unwrap();

        // B is offline as A sees it: A, the holder, serves the note and takes edits B lacks.
        n[0].set_cut(&["b"]);
        let mut sub_a = a
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(1),
            })
            .await
            .unwrap();
        a.ask(ApplyUpdate {
            doc: doc.clone(),
            update: typed(&Doc::new(), "typed on the fallback"),
            by: owner(1),
        })
        .await
        .unwrap();
        n[0].set_cut(&[]);

        // B returns and opens: parked until A handed back; nothing reached B by the mirror.
        let opened = b
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(2),
            })
            .await
            .unwrap();
        assert_eq!(text_of(&opened), "typed on the fallback");
        assert_eq!(opened.server, Some(eid("b")));
        assert_eq!(
            (opened.meta.serve_epoch, opened.meta.served_by.clone()),
            (3, Some(eid("b"))),
            "above A's fallback claim (2)"
        );
        assert_eq!(
            next(&mut sub_a).await,
            DocEvent::Closed {
                reason: hub::SERVER_MOVED.into()
            }
        );
        // One live Doc: A is a front door now.
        let via_a = a
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(3),
            })
            .await
            .unwrap();
        assert_eq!(
            (text_of(&via_a), via_a.server),
            ("typed on the fallback".to_owned(), Some(eid("b")))
        );
        // A handback from an engine that is neither home nor holder is refused.
        let e = b
            .ask(hub::Handback {
                doc: doc.clone(),
                sv: Vec::new(),
                from: eid("zz"),
            })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), hub::NOT_HOME);

        // §4.8: Move to A, on B (the server): A merges the home, pulls from B, and serves.
        let moved = b
            .ask(hub::MoveHome {
                doc: doc.clone(),
                to: eid("a"),
                checks: true,
            })
            .await
            .unwrap();
        assert_eq!(moved.id, doc);
        assert_eq!(store_a.read_meta(&doc).unwrap().home, Some(eid("a")));
        assert_eq!(server(&a, &doc).await, Ok(None));
        let via_b = b
            .ask(OpenDoc {
                doc: doc.clone(),
                sv: None,
                by: owner(4),
            })
            .await
            .unwrap();
        assert_eq!(
            (text_of(&via_b), via_b.server),
            ("typed on the fallback".to_owned(), Some(eid("a")))
        );
        cancel.cancel();
    }

    /// §4.3 steps 4 and 8: calls park while a claim runs; a `Join` answers the retryable `busy`
    /// after `JOIN_PARK_MS`; a source that never answers costs `HANDBACK_TIMEOUT_MS`, then the
    /// home serves what it has.
    #[tokio::test]
    async fn a_parked_join_answers_busy_and_a_silent_source_times_out() {
        let n = nodes(&["b", "z"]);
        let db = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let (b, store_b) = engine("b", &n[0], "z", db.path(), &cancel).await;
        let doc = DocId::new();
        store_b
            .create(&DocMeta {
                served_by: Some(eid("z")),
                ..homed(doc.clone(), "b")
            })
            .await
            .unwrap();
        let t0 = tokio::time::Instant::now();
        let join = b.ask(Join {
            doc: doc.clone(),
            guest_pub: [1; 32],
            handshake_hash: vec![2; 32],
            proof: vec![3; 32],
            name: None,
        });
        let open = b.ask(OpenDoc {
            doc: doc.clone(),
            sv: None,
            by: owner(1),
        });
        let (join, open) = tokio::join!(join, open);
        let e = join.unwrap_err();
        assert!(
            matches!(&e, kameo::error::SendError::HandlerError(j) if j.retryable()),
            "{e}"
        );
        assert!(open.is_ok(), "the parked open is served after the timeout");
        assert!(t0.elapsed() >= Duration::from_millis(hub::HANDBACK_TIMEOUT_MS));
        cancel.cancel();
    }

    /// 184 §4.3 step 4, §6.2: a block read on the home while its claim runs waits for the
    /// hand-back and reads the claimed Doc (the fallback's code block), not the home's stale copy.
    #[tokio::test]
    async fn a_block_read_during_a_claim_reads_the_claimed_doc() {
        use crate::cells::{CodeBlockText, NoteKey};
        use crate::note_cells::tests::{PREVIEW, edit, push_block};
        let n = nodes(&["a", "b"]);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cancel = CancellationToken::new();
        let (a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (b, store_b) = engine("b", &n[1], "a", db.path(), &cancel).await;
        let doc = DocId::new();
        store_a.create(&homed(doc.clone(), "b")).await.unwrap();
        store_b.create(&homed(doc.clone(), "b")).await.unwrap();
        // B is offline as A sees it: A, the fallback, takes the code block.
        n[0].set_cut(&["b"]);
        let block = edit(&Doc::new(), |txn, g| {
            push_block(txn, g, None, "b1", Some("bash"), PREVIEW)
        });
        a.ask(ApplyUpdate {
            doc: doc.clone(),
            update: block,
            by: owner(1),
        })
        .await
        .unwrap();
        n[0].set_cut(&[]);
        let got = b
            .ask(CodeBlockText {
                doc: doc.clone(),
                key: NoteKey::Block("b1".into()),
                seen: None,
            })
            .await
            .unwrap();
        assert_eq!(got.text, PREVIEW);
        cancel.cancel();
    }

    /// 184 §8.1, §15.3 checks 12 and 17: the mesh's `DocsGateState` flips homes without the 30 s
    /// poll. An "off" that a later "on" overrides within `GATE_OFF_SETTLE` (the holder's
    /// disconnect before the lease moves) changes nothing; one that holds makes the home stop
    /// serving its open note and the holder serve it.
    #[tokio::test]
    async fn a_published_gate_flip_reroutes_and_a_blip_does_not() {
        let n = nodes(&["a", "b"]);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cancel = CancellationToken::new();
        let (_a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (b, store_b) = engine("b", &n[1], "a", db.path(), &cancel).await;
        let doc = DocId::new();
        store_a.create(&homed(doc.clone(), "b")).await.unwrap();
        store_b.create(&homed(doc.clone(), "b")).await.unwrap();
        let mut sub = b
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(1),
            })
            .await
            .unwrap();
        assert_eq!(server(&b, &doc).await, Ok(None));
        let gate = |homes_on| mesh::DocsGateState {
            floor_ok: homes_on,
            homes_on,
        };
        b.tell(gate(false)).await.unwrap();
        b.tell(gate(true)).await.unwrap();
        tokio::time::sleep(hub::GATE_OFF_SETTLE + Duration::from_millis(300)).await;
        assert!(sub.try_recv().is_err(), "the blip unloaded nothing");
        assert_eq!(server(&b, &doc).await, Ok(None));
        let t = tokio::time::Instant::now();
        b.tell(gate(false)).await.unwrap();
        assert_eq!(
            next(&mut sub).await,
            DocEvent::Closed {
                reason: hub::SERVER_MOVED.into()
            }
        );
        assert!(t.elapsed() >= hub::GATE_OFF_SETTLE);
        assert_eq!(server(&b, &doc).await, Ok(Some(eid("a"))));
        cancel.cancel();
    }

    /// 184 §4.4, §15.3 check 12: a fallback serving at epoch 2 that merges the home's fence at the
    /// same epoch (a greater id wins the merge) learns of the second server: `Server{dual: home}`.
    #[tokio::test]
    async fn an_equal_epoch_fence_from_a_greater_id_flags_the_second_server() {
        use crate::mirror::{MirrorFile, file_name};
        let n = nodes(&["a", "b"]);
        let da = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let (a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let doc = DocId::new();
        store_a.create(&homed(doc.clone(), "b")).await.unwrap();
        n[0].set_cut(&["b"]);
        // A serves as the fallback: no source reachable, Held at epoch 2.
        let mut sub = a
            .ask(Subscribe {
                doc: doc.clone(),
                by: owner(1),
            })
            .await
            .unwrap();
        assert_eq!(server(&a, &doc).await, Ok(None));
        // B's meta arrives through the mirror: B claimed at epoch 2 too.
        let theirs = DocMeta {
            serve_epoch: 2,
            served_by: Some(eid("b")),
            ..homed(doc.clone(), "b")
        };
        let path = store_a.mirror_dir().join(file_name(&doc, MirrorFile::Meta));
        std::fs::write(&path, serde_json::to_vec(&theirs).unwrap()).unwrap();
        a.tell(crate::actor::MirrorChanged { path }).await.unwrap();
        let dual = loop {
            match next(&mut sub).await {
                DocEvent::Server { dual, .. } => break dual,
                _ => continue,
            }
        };
        assert_eq!(dual, Some(eid("b")));
        cancel.cancel();
    }

    /// 184 §4.7, §16: on a lone engine, an update that changes nothing — a client's catch-all
    /// echoing the delete set of a note with a deletion, or a re-sent update — writes no log
    /// record, keeps `updated_ms`, and reaches no subscriber.
    #[tokio::test]
    async fn an_update_that_changes_nothing_is_not_applied() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let id = DocId::new();
        store.create(&note(id.clone())).await.unwrap();
        let client = Doc::new();
        let edit = {
            let t = client.get_or_insert_text("t");
            let mut txn = client.transact_mut();
            t.insert(&mut txn, 0, "hello world");
            t.remove_range(&mut txn, 5, 6);
            b64(&txn.encode_update_v1())
        };
        let mut sub = a
            .ask(Subscribe {
                doc: id.clone(),
                by: owner(1),
            })
            .await
            .unwrap();
        let apply = |update: String| ApplyUpdate {
            doc: id.clone(),
            update,
            by: owner(2),
        };
        a.ask(apply(edit.clone())).await.unwrap();
        assert!(matches!(next(&mut sub).await, DocEvent::Update { .. }));
        let log = store.root().join(id.as_str()).join("updates.log");
        let len = std::fs::metadata(&log).unwrap().len();
        let meta_of = || MetaOf {
            doc: id.clone(),
            by: owner(2),
        };
        let updated = a.ask(meta_of()).await.unwrap().updated_ms;
        tokio::time::sleep(Duration::from_millis(5)).await;
        // What the client pushes past the server's vector when nothing is new: the delete set alone.
        let echo = {
            let txn = client.transact();
            b64(&txn.encode_state_as_update_v1(&txn.state_vector()))
        };
        for u in [echo, edit] {
            a.ask(apply(u)).await.unwrap();
        }
        assert_eq!(std::fs::metadata(&log).unwrap().len(), len, "no log record");
        assert_eq!(a.ask(meta_of()).await.unwrap().updated_ms, updated);
        assert!(sub.try_recv().is_err(), "no fan-out");
    }

    /// Z184-R2 (security must-fix): a guest revoked on the note's server is refused through
    /// another engine whose replica still lists them: a member's read there asks the server.
    #[tokio::test]
    async fn a_guest_revoked_on_the_server_is_refused_through_another_engine() {
        let n = nodes(&["a", "b"]);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cancel = CancellationToken::new();
        let (a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (b, store_b) = engine("b", &n[1], "a", db.path(), &cancel).await;
        let doc = DocId::new();
        let g = crate::meta::Member {
            pub_: "g".into(),
            name: "G".into(),
            color: String::new(),
            role: crate::meta::Role::Editor,
            added_ms: 1,
            via: None,
            last_seen_ms: 1,
        };
        let shared = DocMeta {
            members: vec![g],
            ..homed(doc.clone(), "b")
        };
        store_a.create(&shared).await.unwrap();
        store_b.create(&shared).await.unwrap();
        let guest = Principal::Member {
            pub_: "g".into(),
            device: Some(5),
        };
        let may = || crate::notebook::MayRead {
            doc: doc.clone(),
            by: guest.clone(),
        };
        a.ask(may()).await.unwrap();
        b.ask(crate::share::MemberRevoke {
            doc: doc.clone(),
            pub_: "g".into(),
        })
        .await
        .unwrap();
        assert_eq!(b.ask(may()).await.unwrap_err().to_string(), "not_member");
        // A's replica still lists G (no mirror here): A asks B, the server, and refuses.
        assert_eq!(store_a.read_meta(&doc).unwrap().members.len(), 1);
        assert_eq!(a.ask(may()).await.unwrap_err().to_string(), "not_member");
        cancel.cancel();
    }

    /// §5: every create runs on the engine that will serve it — here for homed kinds (no
    /// `NotHub` off the hub), on the holder over `Cmd::Create` for hub kinds and a thread homed
    /// there, idempotently for a keyed binding (W-f).
    #[tokio::test]
    async fn creates_off_the_hub_run_where_the_document_will_be_served() {
        let n = nodes(&["a", "b"]);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cancel = CancellationToken::new();
        // A must stay alive: it is the holder B forwards to.
        let (_a, store_a) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (b, store_b) = engine("b", &n[1], "a", db.path(), &cancel).await;
        let by = || Principal::Owner { device: None };
        let here = |s: &Store, id: &DocId| s.read_meta(id).map(|m| m.home).ok();

        // Homed kinds made on B (tool, dreaming, autoresearch: `CreateNote`; a board; a meeting
        // note; a thread whose task lives on B) live on B.
        let note = b
            .ask(crate::actor::CreateNote {
                title: "n".into(),
                markdown: String::new(),
                by: by(),
                home: Homing::Here,
            })
            .await
            .unwrap();
        let board = b
            .ask(crate::actor::CreateBoard {
                title: "b".into(),
                markdown: String::new(),
                root: db.path().to_path_buf(),
                by: by(),
            })
            .await
            .unwrap();
        let create = |binding: Binding, home: Homing| Create {
            binding,
            title: "t".into(),
            markdown: String::new(),
            by: by(),
            home,
        };
        let meeting = b
            .ask(create(Binding::MeetingNote("m1".into()), Homing::Here))
            .await
            .unwrap();
        let thread = b
            .ask(create(
                Binding::TaskThread("t1".into()),
                Homing::Engine(eid("b")),
            ))
            .await
            .unwrap();
        for s in [&note, &board, &meeting, &thread] {
            assert_eq!(here(&store_b, &s.id), Some(Some(eid("b"))), "{:?}", s.kind);
            assert_eq!(here(&store_a, &s.id), None, "not on the hub yet");
        }
        let m = store_b.read_meta(&note.id).unwrap();
        assert_eq!((m.serve_epoch, m.served_by), (1, Some(eid("b"))));

        // A thread whose task lives on A is created there; B routes to it before any mirror.
        let t2 = create(Binding::TaskThread("t2".into()), Homing::Engine(eid("a")));
        let remote = b.ask(t2.clone()).await.unwrap();
        assert_eq!(here(&store_a, &remote.id), Some(Some(eid("a"))));
        assert_eq!(here(&store_b, &remote.id), None);
        assert_eq!(server(&b, &remote.id).await, Ok(Some(eid("a"))));
        assert_eq!(
            b.ask(t2).await.unwrap().id,
            remote.id,
            "a retried forward is one document"
        );

        // Hub kinds (knowledge, notebooks, insight notes) are made by the holder.
        let k = b
            .ask(Ensure {
                binding: Binding::Knowledge("k".into()),
                markdown: "# K\n".into(),
            })
            .await
            .unwrap();
        assert_eq!(here(&store_a, &k), Some(None));
        let nb = b
            .ask(NotebookCreate {
                title: "nb".into(),
                description: String::new(),
                project: None,
                by: by(),
            })
            .await
            .unwrap();
        assert_eq!(here(&store_a, &nb.doc.id), Some(None));
        let insight = b
            .ask(crate::actor::CreateNote {
                title: "i".into(),
                markdown: String::new(),
                by: by(),
                home: Homing::Hub,
            })
            .await
            .unwrap();
        assert_eq!(here(&store_a, &insight.id), Some(None));

        // Homes off (a mixed mesh): today's refusal off the hub, and no home on the hub.
        b.tell(hub::HomesOn(false)).await.unwrap();
        let e = b
            .ask(crate::actor::CreateNote {
                title: "n".into(),
                markdown: String::new(),
                by: by(),
                home: Homing::Here,
            })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "NotHub:a");
        cancel.cancel();
    }

    /// §4.8: Move to is refused for every kind but a note, and for an offline or current home.
    #[tokio::test]
    async fn move_home_refuses_each_kind_with_its_copy() {
        let n = nodes(&["b"]);
        let db = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let (b, store) = engine("b", &n[0], "b", db.path(), &cancel).await;
        let board = DocMeta {
            kind: DocKind::Board,
            ..note(DocId::new())
        };
        let docs = [
            (
                board,
                "boards stay on the machine whose folder they point at",
            ),
            (
                bound(Binding::MeetingNote("m".into())),
                "meeting notes stay on the machine that recorded them",
            ),
            (
                bound(Binding::TaskThread("t".into())),
                "move the task instead",
            ),
            (
                bound(Binding::Knowledge("k".into())),
                "this document is served by the hub",
            ),
        ];
        let mv = |doc: &DocId, to: &str| hub::MoveHome {
            doc: doc.clone(),
            to: eid(to),
            checks: true,
        };
        let refusal = |r: Result<DocSummary, kameo::error::SendError<hub::MoveHome, String>>| {
            r.unwrap_err().to_string()
        };
        for (meta, copy) in docs {
            store.create(&meta).await.unwrap();
            assert_eq!(refusal(b.ask(mv(&meta.id, "b")).await), copy);
        }
        let mine = DocId::new();
        store.create(&homed(mine.clone(), "b")).await.unwrap();
        assert_eq!(refusal(b.ask(mv(&mine, "zz")).await), "zz is offline");
        assert_eq!(refusal(b.ask(mv(&mine, "b")).await), "already lives on b");
        b.tell(hub::HomesOn(false)).await.unwrap();
        assert_eq!(
            refusal(b.ask(mv(&mine, "b")).await),
            "notes cannot move until every machine runs this version"
        );
        cancel.cancel();
    }

    /// §4.9: the holder re-homes what an unpaired engine homed, once; a second pass writes nothing.
    #[tokio::test]
    async fn unpair_rehomes_to_the_holder_once() {
        let n = nodes(&["a"]);
        let da = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let (a, store) = engine("a", &n[0], "a", da.path(), &cancel).await;
        let (gone, kept) = (DocId::new(), DocId::new());
        store.create(&homed(gone.clone(), "c")).await.unwrap();
        store.create(&homed(kept.clone(), "d")).await.unwrap();
        a.ask(hub::Rehome(vec![eid("c")])).await.unwrap();
        let after = store.read_meta(&gone).unwrap();
        assert_eq!(after.home, Some(eid("a")));
        assert!(after.home_ms > 1);
        assert_eq!(store.read_meta(&kept).unwrap().home, Some(eid("d")));
        a.ask(hub::Rehome(vec![eid("c")])).await.unwrap();
        assert_eq!(
            store.read_meta(&gone).unwrap().home_ms,
            after.home_ms,
            "idempotent"
        );
        cancel.cancel();
    }

    /// §16: one engine with no mesh is today's: no home, no fence, nobody to forward to.
    #[tokio::test]
    async fn a_lone_engine_creates_and_serves_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let bin = edit_helper(dir.path(), &typed(&Doc::new(), "x"));
        let (a, store) = actor_with(dir.path(), &bin).await;
        let s = a
            .ask(crate::actor::CreateNote {
                title: "n".into(),
                markdown: String::new(),
                by: owner(1),
                home: Homing::Here,
            })
            .await
            .unwrap();
        let m = store.read_meta(&s.id).unwrap();
        assert_eq!(
            (m.home, m.home_ms, m.serve_epoch, m.served_by),
            (None, 0, 0, None)
        );
        let opened = a
            .ask(OpenDoc {
                doc: s.id.clone(),
                sv: None,
                by: owner(1),
            })
            .await
            .unwrap();
        assert_eq!((opened.server, opened.dual), (None, None));
        assert_eq!(server(&a, &s.id).await, Ok(None));
    }
}
