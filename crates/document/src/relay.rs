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

use crate::actor::{ApplyUpdate, AwarenessUpdateMsg, DocumentActor, Edit, EditOutcome, MAX_UPDATE_B64, OpenDoc, Subscribe};
use crate::helper::{Edit as BlockEdit, EditError};
use crate::hub::{DocError, HUB_MOVED};
use crate::meta::{DocEvent, DocId, DocOpen, Joined, Principal};
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
        let (&tag, body) = frame.split_first().ok_or_else(|| anyhow::anyhow!("empty frame"))?;
        let msg: Self = postcard::from_bytes(body)?;
        anyhow::ensure!(msg.tag() == tag, "frame tag {tag} does not match {msg:?}");
        Ok(msg)
    }
}

/// The codec every `theuth/docs/1` stream uses.
pub fn framed(stream: Box<dyn replica::ByteStream>) -> Framed<Box<dyn replica::ByteStream>, LengthDelimitedCodec> {
    Framed::new(stream, LengthDelimitedCodec::builder().max_frame_length(MAX_FRAME).new_codec())
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
}

/// A reply the hub still owes, in send order.
enum Pending {
    Open(ReplySender<Result<DocOpen, DocError>>),
    Join(ReplySender<Result<Joined, JoinError>>),
    Unit(ReplySender<Result<(), DocError>>),
    Edit(ReplySender<Result<EditOutcome, EditError>>),
}

impl Pending {
    fn answer(self, bytes: &[u8]) {
        fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, DocError> {
            serde_json::from_slice(bytes).map_err(|e| DocError::Other(format!("bad reply from the hub: {e}")))
        }
        match self {
            Self::Open(r) => r.send(parse(bytes)),
            Self::Join(r) => r.send(parse(bytes).map_err(|e| JoinError::Other(e.to_string()))),
            Self::Unit(r) => r.send(Ok(())),
            Self::Edit(r) => r.send(
                serde_json::from_slice(bytes).unwrap_or_else(|e| Err(EditError::Other(format!("bad reply from the hub: {e}")))),
            ),
        }
    }

    fn fail(self, reason: &str) {
        let e = DocError::parse(reason);
        match self {
            Self::Open(r) => r.send(Err(e)),
            Self::Unit(r) => r.send(Err(e)),
            Self::Edit(r) => r.send(Err(EditError::Other(e.to_string()))),
            Self::Join(r) => r.send(Err(match e {
                DocError::NotHub { hub } => JoinError::NotHub(hub),
                DocError::Other(s) => JoinError::Other(s),
            })),
        }
    }
}

impl Cmd {
    /// Refuse without a leg: `NotHub{hub}` (the caller retries; the RPC layer types it).
    fn fail(self, hub: &EngineId) {
        let reason = DocError::NotHub { hub: Some(hub.clone()) }.to_string();
        match self {
            Self::Open { reply, .. } => Pending::Open(reply).fail(&reason),
            Self::Join { reply, .. } => Pending::Join(reply).fail(&reason),
            Self::Edit { reply, .. } => Pending::Edit(reply).fail(&reason),
            Self::Update { reply, .. } | Self::Awareness { reply, .. } | Self::SetName { reply, .. } => {
                Pending::Unit(reply).fail(&reason)
            }
            Self::Subscribe { reply } => reply.send(Err(DocError::parse(&reason))),
        }
    }
}

/// The legs a non-hub actor holds, by client and document, and the transport they open on.
#[derive(Default)]
pub struct Legs {
    transport: Option<Arc<dyn Transport>>,
    legs: HashMap<(Client, DocId), mpsc::Sender<Cmd>>,
}

impl Legs {
    /// Drop every leg (a lease flip): each leg's subscriber hears `Closed{hub_moved}`.
    pub fn clear(&mut self) {
        self.legs.clear();
    }
}

/// `tell`: the mesh transport legs open on (from [`serve`] once the mesh registered one; tests
/// hand in a loopback). Until then every forward answers `NotHub{hub}`.
pub struct SetTransport(pub Arc<dyn Transport>);

impl Message<SetTransport> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: SetTransport, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.legs.transport = Some(m.0);
    }
}

impl DocumentActor {
    /// Off the hub: hand `cmd` to the leg of (`client`, `doc`) towards `hub`, opening it when there
    /// is none or the last one ended. A leg that cannot take it answers `NotHub{hub}`.
    pub(crate) fn forward(&mut self, hub: EngineId, doc: &DocId, client: Client, cmd: Cmd) {
        let key = (client, doc.clone());
        let cmd = match self.legs.legs.get(&key).filter(|tx| !tx.is_closed()) {
            Some(tx) => match tx.try_send(cmd) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Full(c)) => {
                    tracing::warn!(doc = %doc, "document forward leg is behind; refused");
                    return c.fail(&hub);
                }
                Err(mpsc::error::TrySendError::Closed(c)) => c,
            },
            None => cmd,
        };
        let Some(t) = self.legs.transport.clone() else {
            tracing::debug!(doc = %doc, hub = %hub, "document forward: no mesh transport yet");
            return cmd.fail(&hub);
        };
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(run_leg(t, hub, doc.clone(), key.0.clone(), rx));
        let _ = tx.try_send(cmd);
        self.legs.legs.insert(key, tx);
    }
}

/// One leg: open the stream, `Hello`, then commands out / frames in until either side ends it.
async fn run_leg(t: Arc<dyn Transport>, hub: EngineId, doc: DocId, client: Client, mut cmds: mpsc::Receiver<Cmd>) {
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
    if send(&mut wire, &Msg::Hello { member: client.member() }).await.is_err() {
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
    if !reason.is_empty() && let Some(s) = sub {
        let _ = s.try_send(DocEvent::Closed { reason: reason.clone() });
    }
    let reason = if reason.is_empty() { HUB_MOVED.to_owned() } else { reason };
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
    let Some(t) = replica::transport(&rt).await else { return };
    let _ = actor.tell(SetTransport(t.clone())).await;
    serve_on(t, actor, rt.shutdown.clone()).await;
}

/// Accept `theuth/docs/1` streams on `transport` and bind each to `actor` as one remote client.
pub async fn serve_on(transport: std::sync::Arc<dyn Transport>, actor: ActorRef<DocumentActor>, cancel: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => return,
            a = transport.accept(ALPN_DOCS) => a,
        };
        match accepted {
            Ok((peer, stream)) => {
                let actor = actor.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_stream(&actor, framed(stream)).await {
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

/// The actor's answer as a `Closed` reason: the handler's error, or why the actor did not answer.
fn reason_of<M, E: std::fmt::Display>(e: kameo::error::SendError<M, E>) -> String {
    match e {
        kameo::error::SendError::HandlerError(e) => e.to_string(),
        other => other.to_string(),
    }
}

/// One accepted stream: frames in → actor messages as the connection `conn`; replies and the
/// subscription's events out. Ends on the first refusal (after `Closed`), a protocol error or EOF.
async fn serve_stream(actor: &ActorRef<DocumentActor>, mut wire: Wire) -> anyhow::Result<()> {
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
                            .ask(OpenDoc { doc: parse(&doc)?, sv, by: by.clone() })
                            .await
                            .map(|o| serde_json::to_vec(&o).ok())
                            .map_err(reason_of)
                    }
                    Msg::Subscribe { doc } => {
                        doc_of = doc.clone();
                        match actor.ask(Subscribe { doc: parse(&doc)?, by: by.clone() }).await {
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
                                .ask(ApplyUpdate { doc: parse(&doc)?, update, by: by.clone() })
                                .await
                                .map(|()| None)
                                .map_err(reason_of),
                            Err(e) => Err(e),
                        }
                    }
                    Msg::Awareness { doc, bytes } => {
                        doc_of = doc.clone();
                        if let Ok(state) = String::from_utf8(bytes)
                            && let Err(e) = actor.ask(AwarenessUpdateMsg { doc: parse(&doc)?, state, by: by.clone() }).await
                        {
                            tracing::debug!(doc = %doc, error = %reason_of(e), "forwarded awareness dropped");
                        }
                        Ok(None)
                    }
                    Msg::Join { doc, guest_pub, handshake_hash, proof, name } => {
                        doc_of = doc.clone();
                        actor
                            .ask(Join { doc: parse(&doc)?, guest_pub, handshake_hash, proof, name })
                            .await
                            .map(|j| serde_json::to_vec(&j).ok())
                            .map_err(reason_of)
                    }
                    Msg::SetName { doc, name } => {
                        doc_of = doc.clone();
                        actor
                            .ask(SetName { doc: parse(&doc)?, by: by.clone(), name })
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
                            Ok(edits) => match actor.ask(Edit { doc: parse(&doc)?, edits, by: who }).await {
                                Ok(o) => Ok(serde_json::to_vec(&Ok::<_, EditError>(o)).ok()),
                                Err(kameo::error::SendError::HandlerError(EditError::Other(s))) => Err(s),
                                Err(kameo::error::SendError::HandlerError(e)) => {
                                    Ok(serde_json::to_vec(&Err::<EditOutcome, _>(e)).ok())
                                }
                                Err(other) => Err(other.to_string()),
                            },
                        }
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
            Msg::Open { doc: "d".into(), sv: Some("AA==".into()) },
            Msg::Update { doc: "d".into(), bytes: b"AQID".to_vec() },
            Msg::Awareness { doc: "d".into(), bytes: vec![1, 2] },
            Msg::Subscribe { doc: "d".into() },
            Msg::Event { doc: "d".into(), bytes: br#"{"type":"Exported"}"#.to_vec() },
            Msg::Join {
                doc: "d".into(),
                guest_pub: [7; 32],
                handshake_hash: vec![1; 32],
                proof: vec![2; 32],
                name: None,
            },
            Msg::Closed { doc: "d".into(), reason: crate::hub::HUB_MOVED.into() },
            Msg::Hello { member: Some("pub".into()) },
            Msg::SetName { doc: "d".into(), name: "n".into() },
            Msg::Edit {
                doc: "d".into(),
                edits: br#"[{"op":"delete","blocks":["b1"]}]"#.to_vec(),
                by: Principal::Agent { session: uuid::Uuid::from_u128(7) },
            },
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

    use crate::meta::{AgentStyle, Binding, DocMeta, Who};
    use crate::store::Store;
    use crate::{DocumentsConfig, hub};

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Two-node loopback switch, as `rpc::forward`'s tests.
    struct Loop {
        me: PeerId,
        inboxes: Arc<parking_lot::Mutex<Map<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        let inboxes = Arc::new(parking_lot::Mutex::new(Map::new()));
        let mk = |name: &str| {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            inboxes.lock().insert(me.clone(), tx);
            Arc::new(Loop {
                me,
                inboxes: inboxes.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        };
        (mk("a"), mk("b"))
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes.lock().keys().filter(|p| **p != self.me).cloned().collect()
        }

        async fn open(&self, peer: &PeerId, alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            assert_eq!(alpn, ALPN_DOCS);
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self.inboxes.lock().get(peer).cloned().ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs))).map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("closed"))
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
        kameo::registry::ACTOR_REGISTRY.lock().unwrap().remove(crate::DOCUMENT_ACTOR);
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
        let a = DocumentActor::spawn(DocumentActor::new(store.clone(), helper, DocumentsConfig::default(), rt));
        let me = a.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let _ = me.tell(ev).await;
            }
        });
        (a, store)
    }

    /// A `sh` helper: `edit` answers with one canned update (`update_b64`, a v1 update on the
    /// document's `t` text) as the transaction and `inserted: [b1]`; every other op is `ok`.
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
        }
    }

    fn lease(holder: &str) -> MeshEvent {
        MeshEvent::Lease(LeaseState { holder: Some(EngineId(holder.into())), since_ms: 0, reason: "test".into() })
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
        doc.transact_mut().apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        let t = doc.get_or_insert_text("t");
        t.get_string(&doc.transact())
    }

    async fn next(rx: &mut mpsc::Receiver<DocEvent>) -> DocEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event within 5 s").expect("open")
    }

    fn owner(device: u64) -> Principal {
        Principal::Owner { device: Some(device) }
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
        let mut sub_a = a.ask(Subscribe { doc: doc.clone(), by: owner(1) }).await.unwrap();
        let mut sub_7 = b.ask(Subscribe { doc: doc.clone(), by: owner(7) }).await.unwrap();
        let mut sub_8 = b.ask(Subscribe { doc: doc.clone(), by: owner(8) }).await.unwrap();
        let opened = b.ask(OpenDoc { doc: doc.clone(), sv: None, by: owner(7) }).await.unwrap();
        assert_eq!((opened.meta.id.clone(), text_of(&opened)), (doc.clone(), String::new()));

        // 7 types through B: the hub applies it; the hub's subscriber and 8 hear it, 7 does not (own conn).
        let mine = Doc::new();
        b.ask(ApplyUpdate { doc: doc.clone(), update: typed(&mine, "hello "), by: owner(7) }).await.unwrap();
        assert!(matches!(next(&mut sub_a).await, DocEvent::Update { by: Who::Owner, .. }));
        assert!(matches!(next(&mut sub_8).await, DocEvent::Update { .. }));
        b.ask(ApplyUpdate { doc: doc.clone(), update: typed(&mine, "world"), by: owner(7) }).await.unwrap();
        assert!(matches!(next(&mut sub_a).await, DocEvent::Update { .. }));
        assert!(matches!(next(&mut sub_8).await, DocEvent::Update { .. }));

        // Awareness rides the same leg and fans out like an update.
        let mut aw = yrs::sync::Awareness::new(Doc::new());
        aw.set_local_state_raw(r#"{"cursor":1}"#);
        let state = b64(&aw.update().unwrap().encode_v1());
        b.ask(AwarenessUpdateMsg { doc: doc.clone(), state: state.clone(), by: owner(7) }).await.unwrap();
        assert_eq!(next(&mut sub_a).await, DocEvent::Awareness { state: state.clone() });
        assert_eq!(next(&mut sub_8).await, DocEvent::Awareness { state });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(sub_7.try_recv().is_err(), "7's own updates are not echoed to 7");

        // A fresh open through B carries what the hub holds.
        let opened = b.ask(OpenDoc { doc: doc.clone(), sv: None, by: owner(9) }).await.unwrap();
        assert_eq!(text_of(&opened), "hello world");

        // Too large is refused at the front door, before any leg.
        let e = b.ask(ApplyUpdate { doc: doc.clone(), update: "x".repeat(MAX_UPDATE_B64 + 1), by: owner(7) }).await.unwrap_err();
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
        let opened = b.ask(OpenDoc { doc: doc.clone(), sv: None, by: owner(7) }).await.unwrap();
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
            d.transact_mut().apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
            d.transact().state_vector().encode_v1()
        });
        let again = a.ask(OpenDoc { doc: doc.clone(), sv: Some(sv), by: owner(1) }).await.unwrap();
        assert_eq!(text_of(&again), "", "nothing missing from a client that has it all");
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
        let e = b.ask(OpenDoc { doc: doc.clone(), sv: None, by: owner(1) }).await.unwrap_err();
        assert_eq!(e.to_string(), "NotHub:a");
        // A transport that does not see "a" (a holder that is not connected).
        b.tell(SetTransport(tb.clone())).await.unwrap();
        b.tell(lease("zz")).await.unwrap();
        let e = b.ask(Subscribe { doc: doc.clone(), by: owner(1) }).await.unwrap_err();
        assert_eq!(e.to_string(), "NotHub:zz");
        let e = b
            .ask(Join { doc: doc.clone(), guest_pub: [1; 32], handshake_hash: vec![2; 32], proof: vec![3; 32], name: None })
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "NotHub:zz");
        // E2: an agent edit off the hub is refused the same way — an error the tool shows, not a drop.
        let e = b
            .ask(Edit { doc, edits: vec![BlockEdit::Delete { blocks: vec!["b1".into()] }], by: agent() })
            .await
            .unwrap_err();
        assert!(matches!(&e, kameo::error::SendError::HandlerError(EditError::Other(s)) if s == "NotHub:zz"), "{e}");
    }

    fn agent() -> Principal {
        Principal::Agent { session: uuid::Uuid::from_u128(42) }
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
        let edits = vec![BlockEdit::Insert { markdown: "hello from the agent".into(), after: None, before: None, at: Some("end".into()) }];
        let out = b.ask(Edit { doc: doc.clone(), edits, by: agent() }).await.unwrap();
        assert_eq!(out.inserted, vec!["b1".to_owned()]);
        let opened = a.ask(OpenDoc { doc: doc.clone(), sv: None, by: owner(1) }).await.unwrap();
        assert_eq!(text_of(&opened), "hello from the agent");
        assert!(store_b.load(&doc).is_err(), "nothing on B before the mirror");

        // The hub snapshots (quiet period / unload in the plugin); its mirror files reach B as the
        // replica would copy them; B takes them on disk and now reads the same text — from the hub's
        // very bytes, so B's own mirror file equals its row and nothing ships back.
        a.ask(crate::actor::Snapshot { doc: doc.clone(), label: "test".into() }).await.unwrap();
        for kind in [crate::mirror::MirrorFile::Meta, crate::mirror::MirrorFile::Snapshot] {
            let name = crate::mirror::file_name(&doc, kind);
            std::fs::copy(store_a.mirror_dir().join(&name), store_b.mirror_dir().join(&name)).unwrap();
            b.tell(crate::actor::MirrorChanged { path: store_b.mirror_dir().join(&name) }).await.unwrap();
        }
        let stats = b.ask(crate::actor::Stats).await.unwrap(); // a barrier: the tells above ran
        assert_eq!(stats.resnapshots, 0, "{stats:?}");
        let loaded = store_b.load(&doc).unwrap();
        assert_eq!(loaded.doc.get_or_insert_text("t").get_string(&loaded.doc.transact()), "hello from the agent");
        let snap = |d: &Path| std::fs::read(d.join(format!("documents/{doc}/snapshot.yjs"))).unwrap();
        assert_eq!(snap(db.path()), snap(da.path()), "B holds the hub's bytes, not a re-encoding");
        cancel.cancel();
    }
}
