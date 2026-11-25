//! The hub gate (69 §5, contract `69-contract-docs.md` §4): the yrs server is a hub duty. One engine
//! of a mesh — the lease holder — opens documents, accepts updates and awareness, serves subscribers,
//! admits guests, administers members and links, imports the derived files it watches and exports
//! them. Every other engine holds the replicated mirror (`doc_read`, `Outline`, `Markdown` serve from
//! it anywhere) and answers the rest with [`DocError::NotHub`], which the RPC layer turns into the
//! typed `-32011` error and Y turns into forwarding.
//!
//! Lease following is Slack's pattern (`sync_socket`): a [`mesh::lease::Follower`] fed by
//! [`mesh::MeshEvent::Lease`] with the self id from `lookup_when::<mesh::Mesh>`, and a reconcile
//! holding `Option<CancellationToken>` for the derived-file watcher. `holder: None` is a loss once the
//! self id is known; `Follower::default()` (no mesh) is the hub, so a lone engine behaves as before.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use mesh::lease::Follower;
use mesh::{EngineId, LeaseState};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, StateVector, Transact};

use crate::actor::DocumentActor;
use crate::meta::{DocEvent, DocId, DocKind, DocMeta, DocSummary, Principal, ShareLink};
use crate::relay::{Client, Cmd, HandbackReply};

/// Why a document call was refused, when it is more than a message.
///
/// The actor's hub-only handlers answer `Err(DocError::NotHub { hub })`; handlers whose reply type
/// stayed `String` (the owner-plugin surface: `Ensure`, `Edit`, `Snapshot`, …) answer its
/// [`fmt::Display`] form `NotHub:<engine id>` (empty id when no engine holds the lease), which
/// [`DocError::parse`] reads back. The RPC layer maps either to `-32011` with data
/// `{"NotHub": {"hub": "<engine id>" | null}}` (serde external tagging, like `OwnerUnreachable`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum DocError {
    /// This engine does not hold the hub lease; `hub` is the holder as last seen (`None`: nobody).
    NotHub {
        /// The lease holder, when there is one.
        hub: Option<EngineId>,
    },
    /// Any other refusal, as the handler worded it.
    Other(String),
}

/// The `Display` prefix of [`DocError::NotHub`].
const NOT_HUB: &str = "NotHub:";

impl DocError {
    /// Read a [`fmt::Display`] form back: `NotHub:<id>` → `NotHub`, anything else → `Other`.
    pub fn parse(s: &str) -> Self {
        match s.strip_prefix(NOT_HUB) {
            Some(id) => Self::NotHub {
                hub: (!id.is_empty()).then(|| EngineId(id.to_owned())),
            },
            None => Self::Other(s.to_owned()),
        }
    }

    /// The `NotHub` payload, if this is one.
    pub fn not_hub(&self) -> Option<&Option<EngineId>> {
        match self {
            Self::NotHub { hub } => Some(hub),
            Self::Other(_) => None,
        }
    }
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHub { hub } => write!(
                f,
                "{NOT_HUB}{}",
                hub.as_ref().map(|h| h.0.as_str()).unwrap_or("")
            ),
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for DocError {}

impl From<String> for DocError {
    fn from(s: String) -> Self {
        Self::parse(&s)
    }
}

impl From<&str> for DocError {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<DocError> for String {
    fn from(e: DocError) -> Self {
        e.to_string()
    }
}

/// Where this engine stands: the follower, the holder as last seen, the watcher's token while
/// serving, and 184's `homes_on` (§8.1: off until the mesh's first `DocsGate` answer).
#[derive(Debug, Default)]
pub struct Hub {
    follower: Follower,
    holder: Option<EngineId>,
    watcher: Option<CancellationToken>,
    pub(crate) homes_on: bool,
}

impl Hub {
    /// Whether this engine serves documents now.
    pub fn holding(&self) -> bool {
        self.follower.holding
    }

    /// This engine's id, once the mesh told it.
    pub fn self_id(&self) -> Option<EngineId> {
        self.follower.self_id.clone().map(EngineId)
    }

    /// `e` is this engine.
    pub fn is_me(&self, e: &EngineId) -> bool {
        self.follower.self_id.as_deref() == Some(e.0.as_str())
    }

    /// `Ok` on the hub; the refusal every hub-only handler returns elsewhere.
    pub fn gate(&self) -> Result<(), DocError> {
        if self.follower.holding {
            Ok(())
        } else {
            Err(DocError::NotHub {
                hub: self.holder.clone(),
            })
        }
    }

    /// [`Self::gate`] for handlers whose error is a `String`.
    pub fn gate_str(&self) -> Result<(), String> {
        self.gate().map_err(String::from)
    }

    /// Where a hub-only call goes: `Ok(None)` serve here, `Ok(Some(hub))` forward to the holder over
    /// `theuth/docs/1`, `Err` refuse (nobody holds the lease — nothing to forward to).
    pub fn route(&self) -> Result<Option<EngineId>, DocError> {
        match self.gate() {
            Ok(()) => Ok(None),
            Err(DocError::NotHub { hub: Some(hub) }) => Ok(Some(hub)),
            Err(e) => Err(e),
        }
    }

    /// The lease holder as last seen.
    pub fn holder(&self) -> Option<&EngineId> {
        self.holder.as_ref()
    }

    /// The mesh answered `Status`: `Some(gained)` when the duties flip.
    pub fn set_self(&mut self, id: EngineId) -> Option<bool> {
        self.follower.set_self(id.0)
    }

    /// A lease event: `Some(gained)` when the duties flip.
    pub fn observe(&mut self, lease: &LeaseState) -> Option<bool> {
        self.holder = lease.holder.clone();
        self.follower.observe(lease)
    }
}

/// `tell`: the mesh told us our own id (`lookup_when::<mesh::Mesh>` → `Status`).
#[derive(Clone, Debug)]
pub struct SelfId(pub EngineId);

/// `tell`: reconcile duties with the lease (at start, and after every flip).
#[derive(Clone, Copy, Debug, Default)]
pub struct Reconcile;

impl DocumentActor {
    /// 67 §4.4 / 69: start the derived-file watcher when this engine holds the lease; on loss stop
    /// it. Either flip runs the reroute pass (184 §4.5): what this engine no longer serves is
    /// exported when due, snapshotted and unloaded (unhomed: `Closed{hub_moved}` as before), and the
    /// forwarding legs that point at the wrong engine drop. Idempotent.
    pub(crate) fn sync_hub(&mut self, me: ActorRef<Self>) {
        let want = self.hub.holding();
        match (want, self.hub.watcher.take()) {
            (true, Some(tok)) => self.hub.watcher = Some(tok),
            (true, None) => {
                let tok = self.rt.shutdown.child_token();
                match crate::watch::start(&self.data_dir, me, tok.clone()) {
                    Ok(()) => tracing::info!("documents: serving (hub lease held here)"),
                    Err(e) => tracing::warn!("documents file watcher unavailable: {e:#}"),
                }
                self.hub.watcher = Some(tok);
                self.reroute();
            }
            (false, Some(tok)) => {
                tok.cancel();
                let before = self.open.len();
                self.reroute();
                tracing::info!(
                    hub = ?self.hub.holder().map(|h| h.0.as_str()),
                    closed = before - self.open.len(),
                    "documents: another engine holds the hub lease; serving stopped"
                );
            }
            (false, None) => {}
        }
    }
}

/// `DocEvent::Closed { reason }` every subscriber gets on a lease loss.
pub const HUB_MOVED: &str = "hub_moved";

// ---- 184: notes on any machine (§14 frozen contracts) ---------------------------------------

/// 184 §4.3, §4.5: `DocEvent::Closed { reason }` when a document's server moved (a hand-back, a
/// reroute); the editor re-syncs on it (W-e).
pub const SERVER_MOVED: &str = "server_moved";
/// 184 §10.1: `Msg::Closed` for a `Handback` from an engine that is neither the home, the holder,
/// nor a `Handover` target; the source keeps serving.
pub const NOT_HOME: &str = "not_home";
/// 184 §10.1: `Msg::Closed` for tags 10-13 (`Create`, `Handback`, `Block`, `Handover`) on a member
/// stream; the stream ends.
pub const NOT_OWNER: &str = "not_owner";
/// 184 §4.3: a claim waits this long for each source's `HandbackReply`; parked calls as long.
pub const HANDBACK_TIMEOUT_MS: u64 = 5000;
/// 184 §4.3 step 4: a parked `Join` waits at most this, then answers `JoinError::Other("busy")`.
pub const JOIN_PARK_MS: u64 = 2000;
/// 184 §6.2 (W-u): a `Block` read with `seen` waits this long for the update that makes it match.
pub const BLOCK_WAIT_MS: u64 = 1000;
/// 184 §5.3: a forwarded create's home is remembered this long, or until its `meta.json` arrives.
pub const PENDING_HOME_TTL: Duration = Duration::from_secs(600);

/// 184 §4.2: where a call on one document goes ([`DocumentActor::route_doc`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Answer here.
    Serve,
    /// Wait for this engine's claim to end (§4.3 step 4), then re-send the message.
    Park,
    /// Hand to this engine over a `theuth/docs/1` leg (or refuse `NotHub{hub: Some(e)}` without one).
    Forward(EngineId),
}

/// 184 §4.3: this engine's record that it serves a homed document (§9.2, process state; dropped
/// when the document unloads).
pub(crate) enum Claim {
    /// Pulling the other copies before serving: `from` now, then `rest`; `epoch` = the highest
    /// fence seen so far; `parked` = the calls waiting, re-sent in arrival order at the end (W-t).
    Pending {
        from: EngineId,
        rest: Vec<EngineId>,
        epoch: i64,
        /// Behind a lock only so the actor stays `Sync`; the actor alone touches it.
        parked: parking_lot::Mutex<Vec<Resend>>,
    },
    /// Serving at this fence epoch.
    Held { epoch: i64 },
}

/// A parked call: re-sends its message to the actor (enqueued in order) and answers its caller.
pub(crate) type Resend = Box<dyn FnOnce(ActorRef<DocumentActor>) -> BoxFuture<'static, ()> + Send>;

/// How a parked or forwarded call's failure is worded in its reply's error type.
pub(crate) trait Refusal: fmt::Display + fmt::Debug + Send + 'static {
    /// A refusal carrying `s`.
    fn refusal(s: String) -> Self;
}

impl Refusal for String {
    fn refusal(s: String) -> Self {
        s
    }
}

impl Refusal for DocError {
    fn refusal(s: String) -> Self {
        Self::parse(&s)
    }
}

impl Refusal for crate::helper::EditError {
    fn refusal(s: String) -> Self {
        Self::Other(s)
    }
}

impl Refusal for crate::share::JoinError {
    fn refusal(s: String) -> Self {
        Self::Other(s)
    }
}

/// The handler's own error, or why the actor did not answer.
fn refused<X, E: Refusal>(e: SendError<X, E>) -> E {
    match e {
        SendError::HandlerError(e) => e,
        other => E::refusal(other.to_string()),
    }
}

/// Re-send `m` to `me` (enqueued now, so parked calls keep their order) and answer `tx` with its reply.
async fn resend<X, T, E>(me: ActorRef<DocumentActor>, m: X, tx: ReplySender<Result<T, E>>)
where
    X: Send + 'static,
    T: Send + 'static,
    E: Refusal,
    DocumentActor: Message<X, Reply = DelegatedReply<Result<T, E>>>,
{
    match me.ask(m).enqueue().await {
        Ok(pending) => {
            tokio::spawn(async move { tx.send(pending.await.map_err(refused)) });
        }
        Err(e) => tx.send(Err(E::refusal(e.to_string()))),
    }
}

impl DocumentActor {
    /// 184 §4.2: who serves `doc`'s live Doc, rules 1-6; may start a claim (§4.3). `relayed`: the
    /// call arrived over a docs leg ([`Relayed`]), so a `Forward` is refused except by rule 6.
    pub fn route_doc(&mut self, doc: &DocId, relayed: bool) -> Result<Route, DocError> {
        let relayed = relayed || self.relaying;
        let home = if self.hub.homes_on {
            self.home_of(doc)
        } else {
            None
        };
        match self.server_for(home.as_ref())? {
            None if home.is_some() => Ok(self.claim(doc, None)),
            None => Ok(Route::Serve),
            // Rule 6: only the holder relays once more, and only to the home.
            Some(e) if relayed && !(self.hub.holding() && home.as_ref() == Some(&e)) => {
                Err(DocError::NotHub { hub: Some(e) })
            }
            Some(e) => Ok(Route::Forward(e)),
        }
    }

    /// 184 §4.2: this engine serves `doc` now (homed: claim `Held` or `Pending`; unhomed or homes
    /// off: holding the lease). Never starts a claim.
    pub fn serves(&self, doc: &DocId) -> bool {
        if self.hub.homes_on && self.home_peek(doc).is_some() {
            self.claims.contains_key(doc)
        } else {
            self.hub.holding()
        }
    }

    /// 184 §4.2: the read-only route: `Ok(None)` this engine (a claim in flight counts),
    /// `Ok(Some(e))` engine `e`, `Err` nobody. Never starts a claim.
    pub fn server_of(&self, doc: &DocId) -> Result<Option<EngineId>, DocError> {
        let home = if self.hub.homes_on {
            self.home_peek(doc)
        } else {
            None
        };
        if home.is_some() && matches!(self.claims.get(doc), Some(Claim::Pending { .. })) {
            return Ok(None);
        }
        self.server_for(home.as_ref())
    }

    /// 184 §4.6: [`Self::route_doc`] for a handler with no leg: `Serve` or `Park`, and
    /// `Forward(e)` → `Err(NotHub{hub: Some(e)})` (the RPC layer's `doc_forward` goes there).
    pub fn gate_doc(&mut self, doc: &DocId, relayed: bool) -> Result<Route, DocError> {
        match self.route_doc(doc, relayed)? {
            Route::Forward(e) => Err(DocError::NotHub { hub: Some(e) }),
            r => Ok(r),
        }
    }

    /// [`Self::gate_doc`] for a delegated handler: `Some((m, tx))` to run here; parked or refused
    /// (`tx` answered) otherwise.
    pub(crate) fn gate_here<M, T, E>(
        &mut self,
        doc: &DocId,
        m: M,
        tx: ReplySender<Result<T, E>>,
    ) -> Option<(M, ReplySender<Result<T, E>>)>
    where
        M: Send + 'static,
        T: Send + 'static,
        E: Refusal + From<DocError>,
        DocumentActor: Message<M, Reply = DelegatedReply<Result<T, E>>>,
    {
        match self.gate_doc(doc, false) {
            Ok(Route::Serve) => Some((m, tx)),
            Ok(_) => {
                self.park(doc, m, tx, None);
                None
            }
            Err(e) => {
                tx.send(Err(e.into()));
                None
            }
        }
    }

    /// §5.2: run `req` on `to` over `Cmd::Create` on the leg `(client, id)`; the reply decodes
    /// as `T`. `home`: remembered for `id` until its `meta.json` arrives, so the creator's next
    /// call (and the reroute pass, for this leg) goes to the home. A keyless create's leg (a fresh
    /// id nothing else uses) is dropped once answered.
    pub(crate) fn create_on<T: serde::de::DeserializeOwned>(
        &mut self,
        to: EngineId,
        id: DocId,
        client: Client,
        req: crate::relay::CreateReq,
        home: Option<EngineId>,
    ) -> impl Future<Output = Result<T, String>> + Send + 'static {
        use crate::relay::CreateReq;
        if let Some(h) = home {
            self.pending_homes
                .insert(id.clone(), (h, std::time::Instant::now()));
        }
        let one_shot = match &req {
            CreateReq::Create { binding, .. } => binding.keyless(),
            CreateReq::Notebook { .. } | CreateReq::Whiteboard { .. } => true,
            CreateReq::Ensure { .. } => false,
        };
        let (me, key) = (self.actor_ref(), (client.clone(), id.clone()));
        let ask = self.leg(to, id, client, move |reply| Cmd::Create { req, reply });
        async move {
            let r = ask.await;
            if one_shot {
                let _ = me.tell(DropLeg(key.0, key.1)).await;
            }
            serde_json::from_value(r?)
                .map_err(|e| format!("bad reply from the document's server: {e}"))
        }
    }

    /// 184 §5.2: where a document that does not exist yet is created, given its would-be `home`
    /// (`Homing` resolved by [`Self::resolve_home`]): `Ok(None)` here, `Ok(Some(e))` over
    /// `Cmd::Create` to `e`, `Err` refuse. Homes off: today's create gate (never forwarded, §8.2).
    pub fn route_home(&self, home: Option<&EngineId>) -> Result<Option<EngineId>, DocError> {
        if !self.hub.homes_on {
            return self.hub.gate().map(|()| None);
        }
        match self.server_for(home)? {
            // Rule 6 for a relayed create: only the holder relays once more, to the home.
            Some(e) if self.relaying && !(self.hub.holding() && home == Some(&e)) => {
                Err(DocError::NotHub { hub: Some(e) })
            }
            r => Ok(r),
        }
    }

    /// 184 §5.1: the `home` a new document of `kind` gets: `None` with homes off or for an
    /// unhomed kind; `Here` = this engine.
    pub(crate) fn resolve_home(
        &self,
        home: &crate::actor::Homing,
        kind: DocKind,
    ) -> Option<EngineId> {
        use crate::actor::Homing;
        if !self.hub.homes_on || !kind.homed() {
            return None;
        }
        match home {
            Homing::Here => self.hub.self_id(),
            Homing::Hub => None,
            Homing::Engine(e) => Some(e.clone()),
        }
    }

    /// Rules 2-5 for a document whose home (homes on) is `home`: `Ok(None)` here.
    fn server_for(&self, home: Option<&EngineId>) -> Result<Option<EngineId>, DocError> {
        match home {
            Some(h) if self.hub.is_me(h) => Ok(None),
            Some(h) if self.legs.reachable(h) => Ok(Some(h.clone())),
            // No home, or it is unreachable: the holder (here when holding: the fallback).
            _ => self.hub.route(),
        }
    }

    /// 184 §4.2 rule 1 without memoising: the open meta, the memo, `meta.json`, then a forwarded
    /// create's pending home (§5.3).
    pub(crate) fn home_peek(&self, doc: &DocId) -> Option<EngineId> {
        if let Some(o) = self.open.get(doc) {
            return o.meta.home.clone();
        }
        if let Some(h) = self.homes.get(doc) {
            return h.clone();
        }
        match self.store.read_meta(doc) {
            Ok(m) => m.home,
            Err(_) => self.pending_home(doc),
        }
    }

    /// [`Self::home_peek`], memoising a read `meta.json` (no call reads it twice).
    pub(crate) fn home_of(&mut self, doc: &DocId) -> Option<EngineId> {
        if let Some(o) = self.open.get(doc) {
            return o.meta.home.clone();
        }
        if let Some(h) = self.homes.get(doc) {
            return h.clone();
        }
        match self.store.read_meta(doc) {
            Ok(m) => {
                self.homes.insert(doc.clone(), m.home.clone());
                m.home
            }
            Err(_) => self.pending_home(doc),
        }
    }

    fn pending_home(&self, doc: &DocId) -> Option<EngineId> {
        self.pending_homes
            .get(doc)
            .filter(|(_, at)| at.elapsed() < PENDING_HOME_TTL)
            .map(|(e, _)| e.clone())
    }

    /// §4.3 steps 1-3: serve a homed document only after pulling what its last server and the
    /// holder hold (`first` goes first: a `Handover`'s old server). `Serve` when there is nobody
    /// to pull from (or the document cannot load: the handler answers that error).
    pub(crate) fn claim(&mut self, doc: &DocId, first: Option<EngineId>) -> Route {
        match self.claims.get(doc) {
            Some(Claim::Held { .. }) => return Route::Serve,
            Some(Claim::Pending { .. }) => return Route::Park,
            None => {}
        }
        let holder = self.hub.holder().cloned();
        let (served_by, epoch) = match self.load(doc) {
            Ok(o) => (o.meta.served_by.clone(), o.meta.serve_epoch),
            Err(_) => return Route::Serve,
        };
        // Z184-R2: reloading a note whose fence already names this engine, with nobody to pull
        // from, keeps that fence (no epoch bump, no meta.json rewrite on every load).
        let mine = served_by.as_ref().is_some_and(|s| self.hub.is_me(s));
        let mut sources: Vec<EngineId> = Vec::new();
        for s in [first, served_by, holder].into_iter().flatten() {
            if !self.hub.is_me(&s) && !sources.contains(&s) && self.legs.reachable(&s) {
                sources.push(s);
            }
        }
        if sources.is_empty() {
            self.hold(doc, if mine { epoch } else { epoch + 1 });
            return Route::Serve;
        }
        let from = sources.remove(0);
        tracing::info!(doc = %doc, from = %from, "documents: claiming; handing back from the last server first");
        self.claims.insert(
            doc.clone(),
            Claim::Pending {
                from: from.clone(),
                rest: sources,
                epoch,
                parked: Default::default(),
            },
        );
        self.send_handback(doc, from);
        Route::Park
    }

    /// §4.3 step 7: `Held{epoch}`; the fence names this engine (written by `Tick`), and a claim
    /// re-seals the meta's links (§7.4).
    fn hold(&mut self, doc: &DocId, epoch: i64) {
        let me = self.hub.self_id();
        let vault = self.store.vault();
        if let Some(o) = self.open.get_mut(doc) {
            let fence = (o.meta.serve_epoch, o.meta.served_by.clone());
            o.meta.serve_epoch = epoch;
            o.meta.served_by = me;
            let resealed = vault
                .as_ref()
                .is_some_and(|v| crate::share::reseal_links(v, &mut o.meta.links));
            o.meta_dirty |= resealed || fence != (o.meta.serve_epoch, o.meta.served_by.clone());
        }
        self.claims.insert(doc.clone(), Claim::Held { epoch });
    }

    /// §4.3 step 3: `Cmd::Handback{sv}` on the leg `(Conn(None), doc)` to `from`; the answer (or
    /// `HANDBACK_TIMEOUT_MS` of silence) returns as [`HandbackDone`].
    fn send_handback(&self, doc: &DocId, from: EngineId) {
        let sv = self
            .open
            .get(doc)
            .map(|o| o.doc.transact().state_vector().encode_v1())
            .unwrap_or_default();
        let ask = self.leg(
            from.clone(),
            doc.clone(),
            Client::Conn(None),
            move |reply| Cmd::Handback { sv, reply },
        );
        let (me, doc) = (self.actor_ref(), doc.clone());
        tokio::spawn(async move {
            let result = tokio::time::timeout(Duration::from_millis(HANDBACK_TIMEOUT_MS), ask)
                .await
                .unwrap_or_else(|_| Err(DocError::Other("no answer".into())));
            let _ = me.tell(HandbackDone { doc, from, result }).await;
        });
    }

    /// Ask `to` over a docs leg: `make` wraps the leg's reply sender in the `Cmd`.
    pub(crate) fn leg<T, E>(
        &self,
        to: EngineId,
        doc: DocId,
        client: Client,
        make: impl FnOnce(ReplySender<Result<T, E>>) -> Cmd + Send + 'static,
    ) -> impl Future<Output = Result<T, E>> + Send + 'static
    where
        T: Send + 'static,
        E: Refusal,
    {
        let me = self.actor_ref();
        async move {
            me.ask(Leg {
                to,
                doc,
                client,
                make: Box::new(make),
            })
            .await
            .map_err(refused)
        }
    }

    /// §4.3 step 4 (W-t): `m` waits for `doc`'s claim, then is re-sent (as [`Relayed`] when it
    /// arrived so) and `tx` hears its reply. `busy`: answer `tx` with that refusal if the claim is
    /// still pending after `JOIN_PARK_MS` (a `Join`).
    pub(crate) fn park<M, T, E>(
        &mut self,
        doc: &DocId,
        m: M,
        tx: ReplySender<Result<T, E>>,
        busy: Option<E>,
    ) where
        M: Send + 'static,
        T: Send + 'static,
        E: Refusal,
        DocumentActor: Message<M, Reply = DelegatedReply<Result<T, E>>>
            + Message<Relayed<M>, Reply = DelegatedReply<Result<T, E>>>,
    {
        let relayed = self.relaying;
        let slot = Arc::new(parking_lot::Mutex::new(Some(tx)));
        if let Some(e) = busy {
            let slot = slot.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(JOIN_PARK_MS)).await;
                let tx = slot.lock().take();
                if let Some(tx) = tx {
                    tx.send(Err(e));
                }
            });
        }
        let f: Resend = Box::new(move |me| {
            Box::pin(async move {
                let tx = slot.lock().take();
                let Some(tx) = tx else { return };
                if relayed {
                    resend::<Relayed<M>, T, E>(me, Relayed(m), tx).await;
                } else {
                    resend::<M, T, E>(me, m, tx).await;
                }
            })
        });
        match self.claims.get_mut(doc) {
            Some(Claim::Pending { parked, .. }) => parked.get_mut().push(f),
            _ => self.drain(vec![f]),
        }
    }

    /// Re-send parked calls in order.
    fn drain(&self, parked: Vec<Resend>) {
        if parked.is_empty() {
            return;
        }
        let me = self.actor_ref();
        tokio::spawn(async move {
            for f in parked {
                f(me.clone()).await;
            }
        });
    }

    /// Drop `doc`'s claim (it unloads); parked calls route afresh.
    pub(crate) fn drop_claim(&mut self, doc: &DocId) {
        if let Some(Claim::Pending { parked, .. }) = self.claims.remove(doc) {
            self.drain(parked.into_inner());
        }
    }

    /// §4.3 step 6: apply a source's reply as `File` (log, fan-out, export), then merge its meta
    /// (members, links, authors union; the seal that opens) and take its `agents.json` entries for
    /// every block this copy lacks.
    async fn take_handback(&mut self, doc: &DocId, r: HandbackReply) -> Result<(), String> {
        let update = base64::engine::general_purpose::STANDARD
            .decode(&r.update)
            .map_err(|e| format!("bad update: {e}"))?;
        // `[0, 0]` is the empty v1 update: the source held nothing new.
        if update.len() > 2 {
            self.apply(doc, update, Principal::File, false, Vec::new())
                .await?;
            self.schedule_export(doc);
        }
        let vault = self.store.vault();
        let o = self.load(doc)?;
        crate::mirror::merge_meta(&mut o.meta, &r.meta, &|l| {
            crate::share::seal_opens(vault.as_deref(), l)
        });
        for (block, session) in r.agents {
            o.agents.entry(block).or_insert(session);
        }
        o.authors_dirty = true;
        Ok(())
    }

    /// §4.3 step 7: `Held` above every fence seen, snapshot, re-send the parked calls, reroute.
    fn finish_claim(&mut self, doc: &DocId) {
        let Some(Claim::Pending { epoch, parked, .. }) = self.claims.remove(doc) else {
            return;
        };
        let local = self.open.get(doc).map_or(0, |o| o.meta.serve_epoch);
        self.hold(doc, epoch.max(local) + 1);
        if let Err(e) = self.snapshot(doc, None) {
            tracing::warn!(doc = %doc, error = %e, "documents: snapshot after a claim failed");
        }
        tracing::info!(doc = %doc, "documents: claim held; serving");
        self.drain(parked.into_inner());
        self.reroute();
    }

    /// 184 §4.5: unload what this engine no longer serves (export when due, snapshot;
    /// `server_moved` for a homed document, `hub_moved` for an unhomed one), drop the legs that
    /// point at the wrong engine, hand back from a partition's other server once it is reachable
    /// (§4.4), and on the holder re-home the documents of unpaired engines (§4.9).
    pub(crate) fn reroute(&mut self) {
        let pending = |c: Option<&Claim>| matches!(c, Some(Claim::Pending { .. }));
        let moved: Vec<DocId> = self
            .open
            .keys()
            .filter(|id| !pending(self.claims.get(*id)) && self.server_of(id) != Ok(None))
            .cloned()
            .collect();
        for id in &moved {
            if self
                .open
                .get(id)
                .is_some_and(|o| o.export_due.is_some() && !o.exporting)
            {
                self.export(id, None);
            }
        }
        for id in &moved {
            let reason = if self.home_peek(id).is_some() {
                SERVER_MOVED
            } else {
                HUB_MOVED
            };
            self.unload(id, reason);
        }
        let stale: Vec<(Client, DocId)> = self
            .legs
            .legs
            .iter()
            .filter(|((_, d), (to, _))| {
                !pending(self.claims.get(d))
                    && self.server_of(d).ok().flatten().as_ref() != Some(to)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            self.legs.legs.remove(&k);
        }
        // §4.4: the home hands back from the partition's other server once it can reach it.
        let duals: Vec<(DocId, EngineId)> = self
            .open
            .iter()
            .filter_map(|(id, o)| {
                let e = o.dual.clone()?;
                (o.meta.home.as_ref().is_some_and(|h| self.hub.is_me(h)) && self.legs.reachable(&e))
                    .then(|| (id.clone(), e))
            })
            .collect();
        for (id, e) in duals {
            self.rehand(&id, e);
        }
        if self.hub.homes_on && self.hub.holding() {
            self.ask_removed_peers();
        }
    }

    /// §4.4: the home pulls from `other` (the partition's second server) above its fence; `dual`
    /// clears and the editors hear `Server`.
    fn rehand(&mut self, doc: &DocId, other: EngineId) {
        let epoch = match self.claims.get(doc) {
            Some(Claim::Held { epoch }) => *epoch,
            _ => return,
        };
        self.set_dual(doc, None);
        self.claims.insert(
            doc.clone(),
            Claim::Pending {
                from: other.clone(),
                rest: Vec::new(),
                epoch,
                parked: Default::default(),
            },
        );
        self.send_handback(doc, other);
    }

    /// Set `doc`'s `dual` and tell its editors when it changed (`DocEvent::Server`).
    pub(crate) fn set_dual(&mut self, doc: &DocId, dual: Option<EngineId>) {
        let Some(me) = self.hub.self_id() else { return };
        let Some(o) = self.open.get_mut(doc) else {
            return;
        };
        if o.dual == dual {
            return;
        }
        o.dual = dual.clone();
        self.fan_out(doc, DocEvent::Server { server: me, dual }, None);
    }

    /// §4.4: this serving engine merged a fence (`serve_epoch`, `served_by`) that beats its claim
    /// and names another engine: a greater epoch, or an equal one with a greater engine id (the
    /// merge's own tie rule, §9.1; a tie used to leave the loser unaware of the second server).
    pub(crate) fn fence_beaten(&mut self, doc: &DocId) {
        let Some(Claim::Held { epoch }) = self.claims.get(doc) else {
            return;
        };
        let epoch = *epoch;
        let Some(o) = self.open.get(doc) else { return };
        let (theirs, other, home) = (
            o.meta.serve_epoch,
            o.meta.served_by.clone(),
            o.meta.home.clone(),
        );
        let me = self.hub.self_id();
        let Some(other) =
            other.filter(|e| !self.hub.is_me(e) && (theirs, Some(e)) > (epoch, me.as_ref()))
        else {
            return;
        };
        let at_home = home.as_ref().is_some_and(|h| self.hub.is_me(h));
        match (at_home, self.legs.reachable(&other)) {
            (true, true) => self.rehand(doc, other),
            (true, false) => {
                // Keep serving (W-q), flag it, re-claim above it once: the fallback never re-claims.
                self.set_dual(doc, Some(other));
                self.hold(doc, theirs + 1);
            }
            // A fallback whose home is back: the reroute pass unloads it.
            (false, _) if home.as_ref().is_some_and(|h| self.legs.reachable(h)) => self.reroute(),
            (false, _) => self.set_dual(doc, home),
        }
    }

    /// 184 §8.1: ask the mesh for the docs gate; the answer comes back as a
    /// [`mesh::DocsGateState`], the same path as the mesh's published flips.
    pub(crate) fn poll_gate(&self) {
        let Some(mesh) = ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR)
            .ok()
            .flatten()
        else {
            return;
        };
        let me = self.actor_ref();
        tokio::spawn(async move {
            if let Ok(s) = mesh.ask(mesh::DocsGate).await {
                let _ = me.tell(s).await;
            }
        });
    }

    /// 184 §4.9: ask the mesh which engines were unpaired; the answer comes back as [`Rehome`].
    fn ask_removed_peers(&self) {
        let Some(mesh) = ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR)
            .ok()
            .flatten()
        else {
            return;
        };
        let me = self.actor_ref();
        tokio::spawn(async move {
            if let Ok(removed) = mesh.ask(mesh::RemovedPeers).await
                && !removed.is_empty()
            {
                let _ = me.tell(Rehome(removed)).await;
            }
        });
    }

    /// §4.8: write `home = to`, `home_ms = now` into the live meta or `meta.json`; snapshot an open
    /// document so the mirror ships it.
    pub(crate) async fn write_home(
        &mut self,
        doc: &DocId,
        to: &EngineId,
    ) -> Result<DocMeta, String> {
        let now = crate::store::now_ms();
        let meta = match self.open.get_mut(doc) {
            Some(o) => {
                o.meta.home = Some(to.clone());
                o.meta.home_ms = now;
                o.meta.clone()
            }
            None => {
                let mut m = self.store.read_meta(doc).map_err(|e| format!("{e:#}"))?;
                m.home = Some(to.clone());
                m.home_ms = now;
                m
            }
        };
        self.store
            .write_meta(&meta)
            .await
            .map_err(|e| format!("meta.json: {e:#}"))?;
        if self.open.contains_key(doc)
            && let Err(e) = self.snapshot(doc, None)
        {
            tracing::warn!(doc = %doc, error = %e, "documents: snapshot after a move failed");
        }
        self.homes.insert(doc.clone(), meta.home.clone());
        Ok(meta)
    }

    /// §4.8: the checks `doc_move_home` runs, in order.
    fn move_checks(&self, meta: &DocMeta, to: &EngineId) -> Result<(), String> {
        if !self.hub.homes_on {
            return Err("notes cannot move until every machine runs this version".into());
        }
        match meta.kind {
            DocKind::Note => {}
            DocKind::Board => {
                return Err("boards stay on the machine whose folder they point at".into());
            }
            DocKind::MeetingNote => {
                return Err("meeting notes stay on the machine that recorded them".into());
            }
            DocKind::TaskThread => return Err("move the task instead".into()),
            DocKind::Whiteboard => {
                return Err("whiteboards stay on the machine that holds their images".into());
            }
            _ => return Err("this document is served by the hub".into()),
        }
        if !self.hub.is_me(to) && !self.legs.reachable(to) {
            return Err(format!("{to} is offline"));
        }
        if meta.home.as_ref() == Some(to) {
            return Err(format!("already lives on {to}"));
        }
        Ok(())
    }
}

/// 184 §10.2: `ask` → `M`'s reply: `M` arrived over a `theuth/docs/1` leg (the hub side of
/// [`crate::relay`] wraps every call it makes), so `route_doc(.., relayed: true)` never forwards it
/// again except by rule 6. Wraps `OpenDoc`, `Subscribe`, `ApplyUpdate`, `AwarenessUpdateMsg`,
/// `Join`, `SetName`, `Edit`, `Create`, `Ensure`, `NotebookCreate`, `Handback`, `CodeBlockText`,
/// `MayRead`.
#[derive(Clone, Debug)]
pub struct Relayed<M>(pub M);

impl<M: Send + 'static> Message<Relayed<M>> for DocumentActor
where
    DocumentActor: Message<M>,
{
    type Reply = <DocumentActor as Message<M>>::Reply;

    async fn handle(&mut self, m: Relayed<M>, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // The handler routes as relayed (`route_doc` reads the flag), and a parked call re-sends
        // as `Relayed` again.
        let was = std::mem::replace(&mut self.relaying, true);
        let r = <Self as Message<M>>::handle(self, m.0, ctx).await;
        self.relaying = was;
        r
    }
}

/// 184 §4.3 step 5, §10.2: `ask` → `Result<HandbackReply, DocError>`: `from` claims `doc`; answer
/// the state past `sv` (Yjs v1 state vector), the meta, `agents.json` and the fence, then unload
/// with `server_moved`. Not routed: answered for the home or the holder (a `Handover` target is
/// the home by then), `not_home` for anyone else.
#[derive(Clone, Debug)]
pub struct Handback {
    /// Document.
    pub doc: DocId,
    /// The claimant's Yjs v1 state vector.
    pub sv: Vec<u8>,
    /// The claimant.
    pub from: EngineId,
}

impl Message<Handback> for DocumentActor {
    type Reply = Result<HandbackReply, DocError>;

    async fn handle(&mut self, m: Handback, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let home = self.home_of(&m.doc);
        if home.as_ref() != Some(&m.from) && self.hub.holder() != Some(&m.from) {
            tracing::info!(doc = %m.doc, from = %m.from, "documents: hand-back refused: neither the home nor the holder");
            return Err(DocError::Other(NOT_HOME.to_owned()));
        }
        let sv = if m.sv.is_empty() {
            StateVector::default()
        } else {
            StateVector::decode_v1(&m.sv).map_err(|e| DocError::Other(format!("bad sv: {e}")))?
        };
        let o = self.load(&m.doc)?;
        let update = o.doc.transact().encode_state_as_update_v1(&sv);
        let reply = HandbackReply {
            update: base64::engine::general_purpose::STANDARD.encode(update),
            meta: o.meta.clone(),
            agents: o.agents.clone(),
            epoch: o.meta.serve_epoch,
        };
        tracing::info!(doc = %m.doc, to = %m.from, "documents: handing back; serving stopped here");
        self.unload(&m.doc, SERVER_MOVED);
        Ok(reply)
    }
}

/// `tell` (internal): a source answered this engine's `Handback` (or did not, in time).
pub(crate) struct HandbackDone {
    doc: DocId,
    from: EngineId,
    result: Result<HandbackReply, DocError>,
}

impl Message<HandbackDone> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: HandbackDone, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !matches!(self.claims.get(&m.doc), Some(Claim::Pending { from, .. }) if *from == m.from)
        {
            return;
        }
        let mut seen = 0;
        let failed = match m.result {
            Ok(r) => {
                seen = r.epoch;
                self.take_handback(&m.doc, r).await.err()
            }
            Err(e) => Some(e.to_string()),
        };
        if let Some(reason) = failed {
            tracing::warn!(doc = %m.doc, "documents: hand-back from {} failed ({reason}); continuing without it", m.from);
        }
        let next = match self.claims.get_mut(&m.doc) {
            Some(Claim::Pending {
                from, rest, epoch, ..
            }) => {
                *epoch = (*epoch).max(seen);
                (!rest.is_empty()).then(|| {
                    *from = rest.remove(0);
                    from.clone()
                })
            }
            _ => return,
        };
        match next {
            Some(n) => self.send_handback(&m.doc, n),
            None => self.finish_claim(&m.doc),
        }
    }
}

/// 184 §4.8, §10.2: `ask` → `Result<i64, String>`: the old server hands `doc` to its new home
/// (this engine): merge `meta`, claim with `from` as first source, answer the epoch once `Held`.
#[derive(Clone, Debug)]
pub struct Handover {
    /// Document.
    pub doc: DocId,
    /// The old server's meta, `home` = this engine.
    pub meta: DocMeta,
    /// The old server.
    pub from: EngineId,
}

impl Message<Handover> for DocumentActor {
    type Reply = DelegatedReply<Result<i64, String>>;

    async fn handle(&mut self, m: Handover, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        if m.meta.id != m.doc || !m.meta.home.as_ref().is_some_and(|h| self.hub.is_me(h)) {
            tx.send(Err(NOT_HOME.to_owned()));
            return reply;
        }
        if let Err(e) = self.take_meta(&m.meta).await {
            tx.send(Err(e));
            return reply;
        }
        if !self.claims.contains_key(&m.doc) {
            self.claim(&m.doc, Some(m.from.clone()));
        }
        match self.claims.get(&m.doc) {
            Some(Claim::Held { epoch }) => tx.send(Ok(*epoch)),
            Some(Claim::Pending { .. }) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            None => tx.send(Err(format!("{} cannot be loaded here", m.doc))),
        }
        reply
    }
}

impl DocumentActor {
    /// Merge a peer's meta into this copy (open or on disk), or create the copy from it when the
    /// mirror has not brought it yet (a `Handover` racing replication).
    async fn take_meta(&mut self, meta: &DocMeta) -> Result<(), String> {
        let vault = self.store.vault();
        let opens = |l: &ShareLink| crate::share::seal_opens(vault.as_deref(), l);
        let merged = match self.open.get_mut(&meta.id) {
            Some(o) => crate::mirror::merge_meta(&mut o.meta, meta, &opens).then(|| o.meta.clone()),
            None => match self.store.read_meta(&meta.id) {
                Ok(mut local) => {
                    crate::mirror::merge_meta(&mut local, meta, &opens).then_some(local)
                }
                Err(_) => {
                    let fresh = DocMeta {
                        last_export: None,
                        ..meta.clone()
                    };
                    self.store
                        .create(&fresh)
                        .await
                        .map_err(|e| format!("{e:#}"))?;
                    None
                }
            },
        };
        if let Some(m) = merged {
            self.store
                .write_meta(&m)
                .await
                .map_err(|e| format!("meta.json: {e:#}"))?;
        }
        self.homes.remove(&meta.id);
        Ok(())
    }
}

/// 184 §4.8, §4.9, W-n: `ask` → `Result<DocSummary, String>`: give `doc` the home `to`. On its
/// server: write the home, hand over explicitly; elsewhere: write the home, reroute. `checks`:
/// `true` from `doc_move_home` (homes on, kind `Note`, `to` reachable, `to != home`); `false` from
/// a task move.
#[derive(Clone, Debug)]
pub struct MoveHome {
    /// Document.
    pub doc: DocId,
    /// The new home.
    pub to: EngineId,
    /// Run the user-facing checks of §4.8.
    pub checks: bool,
}

impl Message<MoveHome> for DocumentActor {
    type Reply = DelegatedReply<Result<DocSummary, String>>;

    async fn handle(&mut self, m: MoveHome, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let meta = match self.open.get(&m.doc) {
            Some(o) => Ok(o.meta.clone()),
            None => self.store.read_meta(&m.doc).map_err(|e| format!("{e:#}")),
        };
        let meta = match meta {
            Ok(meta) => meta,
            Err(e) => {
                tx.send(Err(e));
                return reply;
            }
        };
        if m.checks
            && let Err(e) = self.move_checks(&meta, &m.to)
        {
            tx.send(Err(e));
            return reply;
        }
        let serving = matches!(self.claims.get(&m.doc), Some(Claim::Held { .. }))
            && self.server_of(&m.doc) == Ok(None);
        let meta = if meta.home.as_ref() == Some(&m.to) {
            meta
        } else {
            match self.write_home(&m.doc, &m.to).await {
                Ok(meta) => {
                    tracing::info!(doc = %m.doc, to = %m.to, "documents: home moved");
                    meta
                }
                Err(e) => {
                    tx.send(Err(e));
                    return reply;
                }
            }
        };
        let summary = DocSummary::from(&meta);
        if self.hub.is_me(&m.to) {
            // The new home claims now when the document is open, else on its next call.
            if self.open.contains_key(&m.doc) && self.route_doc(&m.doc, false) == Ok(Route::Park) {
                let doc = m.doc.clone();
                self.park(&doc, MoveHome { checks: false, ..m }, tx, None);
            } else {
                tx.send(Ok(summary));
            }
            return reply;
        }
        if serving && self.legs.reachable(&m.to) {
            // §4.8 step 2: the new home merges the meta and claims with this engine first; this
            // engine answers its `Handback` and unloads.
            let to = m.to.clone();
            let ask = self.leg(m.to, m.doc, Client::Conn(None), move |reply| {
                Cmd::Handover {
                    meta: Box::new(meta),
                    reply,
                }
            });
            let me = ctx.actor_ref().clone();
            tokio::spawn(async move {
                let r = tokio::time::timeout(Duration::from_millis(HANDBACK_TIMEOUT_MS), ask).await;
                tx.send(match r {
                    Ok(Ok(_)) => Ok(summary),
                    _ => Err(format!("moved to {to}; it has not taken the note yet")),
                });
                let _ = me.tell(Reroute).await;
            });
            return reply;
        }
        self.reroute();
        tx.send(Ok(summary));
        reply
    }
}

/// 184 §10.2: `ask` → `Result<Option<EngineId>, DocError>` = [`DocumentActor::server_of`]: who
/// serves `doc` (`None` this engine); never starts a claim. `doc_link_create` and
/// `doc_move_home` forward to the answer (§7.2, §4.8).
#[derive(Clone, Debug)]
pub struct DocServer {
    /// Document.
    pub doc: DocId,
}

impl Message<DocServer> for DocumentActor {
    type Reply = Result<Option<EngineId>, DocError>;

    async fn handle(&mut self, m: DocServer, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.server_of(&m.doc)
    }
}

/// Builds a leg's `Cmd` around the reply sender it is given.
type MakeCmd<T, E> = Box<dyn FnOnce(ReplySender<Result<T, E>>) -> Cmd + Send>;

/// `ask` (internal): hand a `Cmd` built around this message's reply sender to the leg towards
/// `to` (the actor's own calls to a peer: `Handback`, `Handover`, `Create`, `MayRead`).
pub(crate) struct Leg<T, E> {
    to: EngineId,
    doc: DocId,
    client: Client,
    make: MakeCmd<T, E>,
}

impl<T: Send + 'static, E: Refusal> Message<Leg<T, E>> for DocumentActor {
    type Reply = DelegatedReply<Result<T, E>>;

    async fn handle(&mut self, m: Leg<T, E>, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.forward(m.to, &m.doc, m.client, (m.make)(tx));
        }
        reply
    }
}

/// `tell` (internal, and tests): the mesh's docs gate said `homes_on` (§8.1); a flip reroutes.
pub(crate) struct HomesOn(pub bool);

impl Message<HomesOn> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: HomesOn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.hub.homes_on != m.0 {
            self.hub.homes_on = m.0;
            tracing::info!(homes_on = m.0, "documents: note homes switched");
            self.reroute();
        }
    }
}

/// 184 §8.1: an "off" from the mesh applies only if it still holds this long. When the holder's
/// connection drops, the gate reads off for the few milliseconds before the lease moves (no live
/// holder frame); applied at once, that blip unloaded every homed note at its own home and made
/// it claim again.
pub(crate) const GATE_OFF_SETTLE: Duration = Duration::from_secs(1);

/// 184 §8.1: the mesh's docs gate, published when `homes_on` flips (so a flip reroutes at once
/// rather than on the next 30 s poll) or answered to [`DocumentActor::poll_gate`]. On applies
/// now; off after [`GATE_OFF_SETTLE`] if no later answer said on.
impl Message<mesh::DocsGateState> for DocumentActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: mesh::DocsGateState,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.gate_latest = m.homes_on;
        if m.homes_on || !self.hub.homes_on {
            return <Self as Message<HomesOn>>::handle(self, HomesOn(m.homes_on), ctx).await;
        }
        let me = self.actor_ref();
        tokio::spawn(async move {
            tokio::time::sleep(GATE_OFF_SETTLE).await;
            let _ = me.tell(GateSettled).await;
        });
    }
}

/// `tell` (internal): [`GATE_OFF_SETTLE`] passed since an "off": apply the latest answer.
pub(crate) struct GateSettled;

impl Message<GateSettled> for DocumentActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _: GateSettled,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let on = self.gate_latest;
        <Self as Message<HomesOn>>::handle(self, HomesOn(on), ctx).await
    }
}

/// `tell` (internal): a one-shot leg (a keyless forwarded create's) has answered.
pub(crate) struct DropLeg(Client, DocId);

impl Message<DropLeg> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: DropLeg, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.legs.legs.remove(&(m.0, m.1));
    }
}

/// `tell` (internal): run the reroute pass (§4.5).
pub(crate) struct Reroute;

impl Message<Reroute> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, _: Reroute, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.reroute();
    }
}

/// `tell` (internal, and tests): 184 §4.9 on the holder — every document homed on one of these
/// unpaired engines is re-homed here, permanently. Idempotent: a re-homed document no longer
/// matches.
pub(crate) struct Rehome(pub Vec<EngineId>);

impl Message<Rehome> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: Rehome, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(me) = self.hub.self_id() else { return };
        if m.0.is_empty() || !self.hub.homes_on || !self.hub.holding() {
            return;
        }
        // Z184-R2: one scan per new tombstone set, not per holder reroute.
        let mut removed = m.0;
        removed.sort();
        if self.rehomed_for == removed {
            return;
        }
        let docs = match self.store.list(None, u32::MAX).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("documents: re-home: {e:#}");
                return;
            }
        };
        let (mut n, mut failed) = (0, false);
        for d in docs {
            if self.home_of(&d.id).is_some_and(|h| removed.contains(&h)) {
                match self.write_home(&d.id, &me).await {
                    Ok(_) => n += 1,
                    Err(e) => {
                        failed = true;
                        tracing::warn!(doc = %d.id, error = %e, "documents: re-home failed");
                    }
                }
            }
        }
        // Z184-R3: a failed write leaves the set unscanned, so the next reroute tries again.
        if !failed {
            self.rehomed_for = removed;
        }
        if n > 0 {
            tracing::info!(
                rehomed = n,
                "documents: notes of unpaired engines re-homed here"
            );
            self.reroute();
        }
    }
}

impl Message<SelfId> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, m: SelfId, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.hub.set_self(m.0).is_some() {
            self.sync_hub(ctx.actor_ref().clone());
        }
    }
}

impl Message<mesh::MeshEvent> for DocumentActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: mesh::MeshEvent,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match m {
            mesh::MeshEvent::Lease(lease) if self.hub.observe(&lease).is_some() => {
                self.sync_hub(ctx.actor_ref().clone());
                if self.hub.holding() {
                    self.reseal().await;
                }
            }
            mesh::MeshEvent::Peer(_) => self.reroute(),
            _ => {}
        }
        self.poll_gate();
    }
}

impl Message<Reconcile> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, _: Reconcile, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.sync_hub(ctx.actor_ref().clone());
    }
}

/// The `Closed` event of a loss, for the actor's unload path and tests.
pub fn closed() -> DocEvent {
    DocEvent::Closed {
        reason: HUB_MOVED.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_hub_round_trips_its_display_and_json() {
        let e = DocError::NotHub {
            hub: Some(EngineId("abc".into())),
        };
        assert_eq!(e.to_string(), "NotHub:abc");
        assert_eq!(DocError::parse(&e.to_string()), e);
        assert_eq!(
            DocError::from(String::from("NotHub:")),
            DocError::NotHub { hub: None }
        );
        assert_eq!(DocError::from("nope"), DocError::Other("nope".into()));
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"NotHub":{"hub":"abc"}}"#
        );
        assert_eq!(
            serde_json::to_string(&DocError::NotHub { hub: None }).unwrap(),
            r#"{"NotHub":{"hub":null}}"#
        );
    }

    #[test]
    fn gate_follows_the_lease_and_a_lone_engine_is_the_hub() {
        let mut hub = Hub::default();
        assert!(hub.gate().is_ok(), "no mesh: hub");
        let lease = |holder: Option<&str>| LeaseState {
            holder: holder.map(|h| EngineId(h.into())),
            since_ms: 0,
            reason: "test".into(),
        };
        // The lease arrives before the self id: remembered, nothing flips yet.
        assert_eq!(hub.observe(&lease(Some("other"))), None);
        assert!(hub.gate().is_ok());
        assert_eq!(hub.set_self(EngineId("me".into())), Some(false));
        assert_eq!(
            hub.gate(),
            Err(DocError::NotHub {
                hub: Some(EngineId("other".into()))
            })
        );
        assert_eq!(hub.gate_str(), Err("NotHub:other".to_owned()));
        assert_eq!(hub.observe(&lease(Some("me"))), Some(true));
        assert!(hub.gate().is_ok());
        // Nobody holds it: a loss, with no hub to name.
        assert_eq!(hub.observe(&lease(None)), Some(false));
        assert_eq!(hub.gate(), Err(DocError::NotHub { hub: None }));
        assert_eq!(hub.observe(&lease(None)), None);
    }

    /// 183 R1: a link sealed under a retained key is re-sealed when this engine becomes the hub.
    #[tokio::test]
    async fn becoming_the_hub_reseals_share_links() {
        use crate::meta::{AgentStyle, Binding, DocId, DocKind, DocMeta, Role, ShareLink};
        use kameo::actor::Spawn as _;
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let vault = std::sync::Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let store = crate::store::Store::open(dir.path())
            .await
            .unwrap()
            .with_vault(vault.clone());
        let id = DocId::new();
        let binding = Binding::MeetingNote("m1".into());
        let hash = crate::share::secret_hash(&[7u8; 32]);
        store
            .create(&DocMeta {
                id: id.clone(),
                kind: DocKind::MeetingNote,
                title: "t".into(),
                derived: binding.derived(&id),
                binding,
                fragment: "blocks".into(),
                members: vec![],
                links: vec![ShareLink {
                    id: "lnk_test".into(),
                    role: Role::Editor,
                    secret_hash_sealed: crate::share::seal_hash(&vault, "lnk_test", &hash),
                    name: None,
                    created_ms: 0,
                    expires_ms: None,
                    revoked_ms: None,
                }],
                agent: AgentStyle::default(),
                last_export: None,
                authors: Default::default(),
                created_ms: 0,
                updated_ms: 0,
                home: None,
                home_ms: 0,
                serve_epoch: 0,
                served_by: None,
                revoked: Default::default(),
            })
            .await
            .unwrap();
        vault.rotate("a", vec![], vec![], 0, false).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper =
            crate::helper::Helper::new(tx, rt.shutdown.clone());
        let actor = DocumentActor::spawn(DocumentActor::new(
            store.clone(),
            helper,
            crate::DocumentsConfig::default(),
            rt,
        ));
        let me = EngineId("me".into());
        actor.tell(SelfId(me.clone())).await.unwrap();
        // Another engine holds the lease first (a follower here), then this one takes it.
        actor
            .tell(mesh::MeshEvent::Lease(LeaseState {
                holder: Some(EngineId("other".into())),
                since_ms: 0,
                reason: "test".into(),
            }))
            .await
            .unwrap();
        actor
            .tell(mesh::MeshEvent::Lease(LeaseState {
                holder: Some(me),
                since_ms: 0,
                reason: "test".into(),
            }))
            .await
            .unwrap();
        actor.ask(Reconcile).await.unwrap();
        let sealed = store.read_meta(&id).unwrap().links[0]
            .secret_hash_sealed
            .clone();
        let bytes = crate::share::unb64url(&sealed).unwrap();
        assert!(
            vault.reseal_with_mesh_key(b"lnk_test", &bytes).is_none(),
            "already under the current key"
        );
    }

    /// 184 §7.4: off the hub, a rotation re-seals the links of the documents this engine homes
    /// (or serves) and leaves the rest to the holder; the pass reports it ran.
    #[tokio::test]
    async fn off_the_hub_reseal_covers_what_this_engine_homes() {
        use crate::meta::{AgentStyle, Binding, DocId, DocKind, DocMeta, Role, ShareLink};
        use kameo::actor::Spawn as _;
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let vault = std::sync::Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let store = crate::store::Store::open(dir.path())
            .await
            .unwrap()
            .with_vault(vault.clone());
        let hash = crate::share::secret_hash(&[7u8; 32]);
        let doc = |home: Option<&str>| {
            let id = DocId::new();
            DocMeta {
                derived: Binding::Note.derived(&id),
                id,
                kind: DocKind::Note,
                title: "t".into(),
                binding: Binding::Note,
                fragment: "blocks".into(),
                members: vec![],
                links: vec![ShareLink {
                    id: "lnk_test".into(),
                    role: Role::Editor,
                    secret_hash_sealed: crate::share::seal_hash(&vault, "lnk_test", &hash),
                    name: None,
                    created_ms: 0,
                    expires_ms: None,
                    revoked_ms: None,
                }],
                agent: AgentStyle::default(),
                last_export: None,
                authors: Default::default(),
                created_ms: 0,
                updated_ms: 0,
                home: home.map(|h| EngineId(h.into())),
                home_ms: 1,
                serve_epoch: 0,
                served_by: None,
                revoked: Default::default(),
            }
        };
        let (mine, theirs) = (doc(Some("me")), doc(None));
        store.create(&mine).await.unwrap();
        store.create(&theirs).await.unwrap();
        vault.rotate("a", vec![], vec![], 0, false).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper =
            crate::helper::Helper::new(tx, rt.shutdown.clone());
        let actor = DocumentActor::spawn(DocumentActor::new(
            store.clone(),
            helper,
            crate::DocumentsConfig::default(),
            rt,
        ));
        actor.tell(SelfId(EngineId("me".into()))).await.unwrap();
        actor
            .tell(mesh::MeshEvent::Lease(LeaseState {
                holder: Some(EngineId("other".into())),
                since_ms: 0,
                reason: "test".into(),
            }))
            .await
            .unwrap();
        assert!(actor.ask(crate::share::Reseal).await.unwrap());
        let stale = |m: &DocMeta| {
            let sealed = store.read_meta(&m.id).unwrap().links[0]
                .secret_hash_sealed
                .clone();
            let bytes = crate::share::unb64url(&sealed).unwrap();
            vault.reseal_with_mesh_key(b"lnk_test", &bytes).is_some()
        };
        assert!(!stale(&mine), "re-sealed by its home");
        assert!(stale(&theirs), "the holder's to re-seal");
    }
}
