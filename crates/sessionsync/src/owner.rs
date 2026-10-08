//! Ownership transfer (68 §4): **take** a session here, **release** one a
//! taker asked for, **demote** when an applied row shows another engine
//! took one of ours, and the orphan bookkeeping that follows.
//!
//! Everything here runs off the actor's mailbox (a take waits on the network
//! for up to `2 × release_wait_secs`); the actor hands over clones of what
//! is needed in [`Take`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent::AgentEvent;
use agent::durability::{DurabilityConfig, ResumeCause, ResumeMode};
use agent::store::Store;
use agent::supervisor::transfer;
use anyhow::Context as _;
use futures::{SinkExt as _, StreamExt as _};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use replica::{ALPN_SESSIONS, PeerId, Transport};
use runtime::{Runtime, SessionId};

use crate::types::Msg;
use crate::{SessionSync, SessionsConfig, TakeReason};

/// How often a taker re-reads its cursor while waiting for the owner's tail.
const CURSOR_POLL: Duration = Duration::from_millis(100);

/// `tell` (from the apply path): an applied row raised the epoch of a
/// session this engine owned — `by` took it. 68 §4.3.
#[derive(Clone, Debug)]
pub struct Demote {
    /// The session.
    pub session: SessionId,
    /// The epoch we owned it in.
    pub old_epoch: u32,
    /// The new owner (origin of the row).
    pub by: PeerId,
}

/// A take's inputs, cloned off the actor (68 §4.2).
pub struct Take {
    /// Publishes `SessionOwnerChanged`.
    pub rt: Runtime,
    /// Timeouts.
    pub config: SessionsConfig,
    /// The durable store.
    pub store: Store,
    /// `None` before the mesh is up: no owner is reachable.
    pub transport: Option<Arc<dyn Transport>>,
    /// Peers with a connection right now.
    pub connected: Vec<PeerId>,
    /// The mesh's view of every peer (`last_seen_ms`); empty without a mesh.
    pub peers: Vec<mesh::Peer>,
}

/// 139 §6.2 step 9: what the taker did about the taken open turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOnTake {
    /// No turn was open on our copy.
    NoOpenTurn,
    /// `ResumeTurn{cause}` sent to the supervisor (resumes now).
    Resume(ResumeCause),
    /// `ResumePending` published: the Attention item; the user resumes.
    Pending,
    /// `durability.resume = off`: the next input resumes lazily.
    Nothing,
}

/// 139 §6.3 policy table (D5, Revision 1 Q10): a transfer follows
/// `durability.resume`; a takeover resumes only when the user asked for it
/// (`User` — the RPC passes `User` for a `Write` take from a real client
/// peer, W4); system takes (`Write`, `Orphan`) leave the item.
fn resume_policy(transferred: bool, reason: TakeReason, mode: ResumeMode) -> ResumeOnTake {
    if transferred {
        match mode {
            ResumeMode::Auto => ResumeOnTake::Resume(ResumeCause::Transfer),
            ResumeMode::Ask => ResumeOnTake::Pending,
            ResumeMode::Off => ResumeOnTake::Nothing,
        }
    } else {
        match reason {
            TakeReason::User => ResumeOnTake::Resume(ResumeCause::Takeover),
            TakeReason::Write | TakeReason::Orphan => ResumeOnTake::Pending,
        }
    }
}

impl Take {
    /// Take `id` here; returns the new `(owner, epoch)`. A session already
    /// ours (or unowned, lone engine) is a no-op. The taken open turn, if
    /// any, is resumed or left for the user per [`resume_policy`].
    pub async fn run(self, id: SessionId, reason: TakeReason) -> Result<(String, u32), String> {
        self.take(id, reason).await.map(|(owner, _)| owner)
    }

    /// [`Take::run`] reporting what step 9 did.
    pub(crate) async fn take(
        self,
        id: SessionId,
        reason: TakeReason,
    ) -> Result<((String, u32), ResumeOnTake), String> {
        let me = self.store.engine().to_owned();
        let (epoch, owner) = self
            .store
            .fence(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{id}: no such session"))?;
        if owner.is_empty() || owner == me {
            return Ok(((owner, epoch), ResumeOnTake::NoOpenTurn));
        }
        if me.is_empty() {
            return Err("this engine has no mesh identity".to_owned());
        }
        let peer = PeerId(owner.clone());
        let mut transferred = false;
        if self.connected.contains(&peer) {
            match self.release_from(&peer, id).await {
                Ok(()) => transferred = true,
                // The user chose to continue here now: A's tail becomes A's orphans (§4.3).
                Err(e) if reason == TakeReason::User => {
                    tracing::warn!("sessions: take {id} from {owner} without its tail: {e:#}");
                }
                Err(e) => return Err(format!("owner {owner} busy: {e:#}")),
            }
        } else if reason == TakeReason::Write {
            self.unseen_long_enough(&owner)?;
        }
        // Re-read: a concurrent take moved the fence meanwhile.
        match self.store.fence(id).await.map_err(|e| e.to_string())? {
            Some((e, o)) if e == epoch && o == owner => {}
            other => {
                return Err(format!(
                    "{id}: ownership moved to {other:?} during the take"
                ));
            }
        }
        // 139 D19: no taker-side `unload` — after D1 no actor exists here
        // for a session this engine does not own (I1).
        let next = epoch + 1;
        self.store
            .set_owner(id, &me, next)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.store
            .set_fence(id, next, &me)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.rt
            .publish(AgentEvent::SessionOwnerChanged {
                session: id,
                owner: me.clone(),
                epoch: next,
                here: true,
            })
            .await;
        let resumed = self.resume_on_take(id, &owner, transferred, reason).await;
        Ok(((me, next), resumed))
    }

    /// 139 §6.2 step 9 / 9′: the open turn on our copy, after `set_owner`,
    /// by the policy table. `ResumeTurn` is a `tell` so the take's reply is
    /// not delayed by the resume.
    async fn resume_on_take(
        &self,
        id: SessionId,
        from: &str,
        transferred: bool,
        reason: TakeReason,
    ) -> ResumeOnTake {
        let turn = match self.store.open_turn(id).await {
            Ok(Some((turn, _))) => turn,
            Ok(None) => return ResumeOnTake::NoOpenTurn,
            Err(e) => {
                tracing::warn!("sessions: take {id}: open_turn: {e:#}");
                return ResumeOnTake::NoOpenTurn;
            }
        };
        let action = resume_policy(transferred, reason, self.durability().resume);
        match &action {
            ResumeOnTake::Resume(cause) => {
                match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
                    Ok(Some(agents)) => {
                        let _ = agents
                            .tell(agent::supervisor::ResumeTurn {
                                session: id,
                                turn,
                                cause: Some(*cause),
                                from: Some(from.to_owned()),
                            })
                            .await;
                    }
                    _ => tracing::warn!(
                        "sessions: take {id}: no supervisor; turn {turn} resumes on the next start"
                    ),
                }
            }
            ResumeOnTake::Pending => {
                if !transferred {
                    // ponytail: the owner's next message replaces the note (owner wins).
                    let note = format!(
                        "taken from {from} at {} while a turn was running; press Resume to continue it here",
                        chrono::Local::now().format("%H:%M")
                    );
                    if let Err(e) = self.store.append_local_note(id, &note).await {
                        tracing::warn!("sessions: take {id}: note: {e:#}");
                    }
                }
                self.rt
                    .publish(AgentEvent::ResumePending {
                        session: id,
                        turn,
                        from: from.to_owned(),
                    })
                    .await;
            }
            ResumeOnTake::Nothing | ResumeOnTake::NoOpenTurn => {}
        }
        tracing::info!("sessions: take {id} from {from}: open turn {turn} → {action:?}");
        action
    }

    /// `durability:` as the agent reads it; defaults when absent.
    fn durability(&self) -> DurabilityConfig {
        self.rt
            .section::<DurabilityConfig>("durability")
            .unwrap_or_default()
    }

    /// `Release` → `Released{last_seq}` over `theuth/sessions/1`, then wait
    /// until our cursor for the owner reached `last_seq`. The reply is
    /// bounded by `release_wait_secs + durability.drain_secs` (139 Q4: the
    /// owner drains for `release_wait_secs − 1` and then flushes), the
    /// cursor by `release_wait_secs`.
    async fn release_from(&self, peer: &PeerId, id: SessionId) -> anyhow::Result<()> {
        let t = self.transport.as_ref().context("no transport")?;
        let wait = Duration::from_secs(self.config.release_wait_secs);
        let reply_wait = wait + Duration::from_secs(self.durability().drain_secs);
        let mut framed = crate::events::framed(t.open(peer, ALPN_SESSIONS).await?);
        framed
            .send(
                Msg::Release {
                    session: id.to_string(),
                }
                .encode()?,
            )
            .await?;
        let frame = tokio::time::timeout(reply_wait, framed.next())
            .await
            .context("release timed out")?
            .context("owner closed the stream")??;
        let last_seq = match Msg::decode(&frame)? {
            Msg::Released { last_seq, .. } => i64::try_from(last_seq)?,
            other => anyhow::bail!("unexpected reply {other:?}"),
        };
        let deadline = Instant::now() + wait;
        loop {
            let applied = self
                .store
                .cursors()
                .await?
                .into_iter()
                .find(|c| c.origin == peer.0 && c.peer.is_empty())
                .map_or(0, |c| c.seq);
            if applied >= last_seq {
                return Ok(());
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "tail not replicated: cursor {applied} < {last_seq}"
            );
            tokio::time::sleep(CURSOR_POLL).await;
        }
    }

    /// 68 §4.2 (c): an unreachable owner must have been unseen for
    /// `takeover_secs`. Unknown to the mesh = never seen = long enough.
    fn unseen_long_enough(&self, owner: &str) -> Result<(), String> {
        let Some(p) = self.peers.iter().find(|p| p.id.0 == owner) else {
            return Ok(());
        };
        if p.reachable {
            return Err(format!(
                "owner {owner} is reachable but has no sessions stream yet; retry"
            ));
        }
        let ago = now_ms().saturating_sub(p.last_seen_ms);
        let need = self.config.takeover_secs * 1000;
        if ago < need {
            return Err(format!(
                "owner {owner} was seen {}s ago; takeover after {}s",
                ago / 1000,
                self.config.takeover_secs
            ));
        }
        Ok(())
    }
}

/// Owner side of a take (68 §4.2 step 1; 139 §6.1 D3): drain the session —
/// its open turn is `turn_finished` or `suspended{release}` before the
/// reply — flush the write connection, answer with `MAX(seq)` of the
/// session's log rows. The session stays readable here; the taker's row
/// flips `owner` when it lands. Inputs are refused meanwhile (D16).
pub async fn release(store: &Store, id: SessionId) -> anyhow::Result<u64> {
    drain_session(store, id).await;
    store.flush().await?;
    Ok(u64::try_from(store.session_log_seq(id).await?).unwrap_or(0))
}

/// Answer one `Release` frame on an accepted stream.
pub(crate) async fn answer_release(
    store: &Store,
    session: String,
    framed: &mut crate::sync::Stream,
) -> anyhow::Result<()> {
    let last_seq = release(store, session.parse()?).await?;
    framed
        .send(Msg::Released { session, last_seq }.encode()?)
        .await?;
    Ok(())
}

/// 68 §4.3: we were the owner in `old_epoch`; `by` now owns it. Stop our
/// actor, orphan our unshipped rows of that epoch (those above what `by`
/// had applied of our log), raise the fence, tell the UI, and leave a
/// local-only note in the transcript. Returns how many rows were orphaned.
/// 139 D16: the taker's row landed, so the release hold on inputs ends.
pub async fn demote(rt: &Runtime, store: &Store, m: Demote) -> anyhow::Result<u64> {
    unload(m.session).await;
    transfer::demoted(m.session).await;
    let me = store.engine();
    let shipped = store
        .cursors()
        .await?
        .into_iter()
        .find(|c| c.origin == me && c.peer == m.by.0)
        .map_or(0, |c| c.seq);
    let n = store
        .log_orphan_after(m.session, m.old_epoch, shipped)
        .await?;
    let (owner, epoch) = store
        .owner(m.session)
        .await?
        .unwrap_or_else(|| (m.by.0.clone(), m.old_epoch + 1));
    store.set_fence(m.session, epoch, &owner).await?;
    rt.publish(AgentEvent::SessionOwnerChanged {
        session: m.session,
        owner,
        epoch,
        here: false,
    })
    .await;
    if n > 0 {
        let from = store
            .log_orphans(m.session)
            .await?
            .into_iter()
            .find(|r| r.epoch == i64::from(m.old_epoch))
            .and_then(|r| replica::Hlc::parse(&r.hlc))
            .and_then(|h| chrono::DateTime::from_timestamp_millis(i64::try_from(h.wall_ms).ok()?))
            .map_or_else(|| "an unknown time".to_owned(), |t| t.to_rfc3339());
        // ponytail: the new owner's next message lands on this seq and
        // replaces the note (owner wins); the orphans stay in `session_orphans`.
        store
            .append_local_note(
                m.session,
                &format!(
                    "{n} turns from {from} were not delivered on {me}; `session_orphans` to view"
                ),
            )
            .await?;
    }
    Ok(n)
}

/// Called after a row of `origin` was applied: if it raised the epoch of a
/// session we owned, the actor demotes (a `tell`, so the apply path never
/// waits on it).
pub(crate) async fn on_applied(
    store: &Store,
    origin: &PeerId,
    session: SessionId,
    before: Option<(u32, String)>,
    epoch: u32,
) {
    let Some((old, owner)) = before else { return };
    if epoch <= old || owner.is_empty() || owner != store.engine() {
        return;
    }
    let m = Demote {
        session,
        old_epoch: old,
        by: origin.clone(),
    };
    match crate::actor() {
        Some(a) => {
            let _ = a.tell(m).await;
        }
        None => {
            tracing::warn!("sessions: {session} taken by {origin} but no sessions actor to demote")
        }
    }
}

/// Stop the live actor of `id`, if the agent supervisor is running
/// (`UnloadSession`, 139 D19: the cancel lands before the stop).
async fn unload(id: SessionId) {
    if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        let _ = agents.ask(agent::UnloadSession { session: id }).await;
    }
}

/// 139 §6.1: the owner side of a take drains `id` — `suspended{release}`
/// first, in-flight calls until the window closes, jobs, then the actor —
/// through the supervisor; without one (a pairing run, these tests) the
/// store-only step 1 still leaves the resumable row (I3).
async fn drain_session(store: &Store, id: SessionId) {
    let deadline = Instant::now() + transfer::release_window(release_wait_secs().await);
    match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(agents)) => {
            if let Err(e) = agents
                .ask(agent::supervisor::DrainSession {
                    session: id,
                    reason: transfer::RELEASE,
                    deadline,
                })
                .await
            {
                tracing::warn!("sessions: release {id}: drain: {e}");
            }
        }
        _ => {
            transfer::suspend_open_turn(store, id, transfer::RELEASE).await;
        }
    }
}

/// `ask` → `sessions.release_wait_secs`: the owner's own window for a
/// release (139 Q4; `Msg::Release` carries none). `release` runs off the
/// actor's mailbox (an accepted stream), so asking it is safe there.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReleaseWaitSecs;

impl Message<ReleaseWaitSecs> for SessionSync {
    type Reply = u64;

    async fn handle(&mut self, _: ReleaseWaitSecs, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.config.release_wait_secs
    }
}

async fn release_wait_secs() -> u64 {
    match crate::actor() {
        Some(a) => a
            .ask(ReleaseWaitSecs)
            .await
            .unwrap_or(SessionsConfig::default().release_wait_secs),
        None => SessionsConfig::default().release_wait_secs,
    }
}

/// Hourly until shutdown: delete orphaned rows older than `keep_days`
/// (`sessions.orphan_keep_days`, 68 §4.3). The demoted owner's note stays.
pub async fn sweep_orphans(
    store: Store,
    keep_days: u64,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        let before = now_ms().saturating_sub(keep_days * 86_400_000);
        match store.prune_orphans_before(before).await {
            Ok(n) if n > 0 => {
                tracing::info!("sessions: pruned {n} orphaned log rows older than {keep_days} days")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("sessions: orphan sweep: {e:#}"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(3600)) => {}
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use replica::{ByteStream, PeerEvent};
    use tokio::sync::{broadcast, mpsc};

    use super::*;
    use crate::apply::{Applied, apply_log_row};
    use agent::durability::{ResumeKind, StepKind};

    // ---- the record replica's loopback transport, cut to what a take needs ----

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Every `Loop`'s accept queue; everyone is linked to everyone.
    #[derive(Default)]
    struct Switch(parking_lot::Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>);

    struct Loop {
        me: PeerId,
        switch: Arc<Switch>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    impl Loop {
        fn new(name: &str, switch: &Arc<Switch>) -> Arc<Self> {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            switch.0.lock().insert(me.clone(), tx);
            Arc::new(Self {
                me,
                switch: switch.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        }
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.switch
                .0
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }
        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let inboxes = self.switch.0.lock();
            let to = inboxes
                .get(peer)
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
                .ok_or_else(|| anyhow::anyhow!("switch closed"))
        }
        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    // ---- two engines, one session, hand-driven shipping ----

    struct Engine {
        store: Store,
        rt: Runtime,
        dir: PathBuf,
    }

    async fn engine(name: &str) -> Engine {
        let dir =
            std::env::temp_dir().join(format!("theuth-owner-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId(name.into())),
        )
        .await
        .unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        Engine { store, rt, dir }
    }

    impl Engine {
        fn id(&self) -> PeerId {
            PeerId(self.store.engine().to_owned())
        }

        /// Apply every row of `from` this engine has not applied yet (what
        /// L's shipper does), and record on `from` how far we got.
        async fn pull(&self, from: &Engine) -> Vec<Applied> {
            let origin = from.id();
            let mut out = Vec::new();
            for row in from.store.log_after(0).await.unwrap() {
                let seq = row.seq;
                let r = apply_log_row(&self.store, &origin, &row.into())
                    .await
                    .unwrap();
                if r != Applied::Duplicate {
                    from.store
                        .set_cursor(&origin.0, self.store.engine(), seq)
                        .await
                        .unwrap();
                }
                out.push(r);
            }
            out
        }

        fn take(
            &self,
            transport: Option<Arc<dyn Transport>>,
            connected: Vec<PeerId>,
            peers: Vec<mesh::Peer>,
        ) -> Take {
            Take {
                rt: self.rt.clone(),
                config: SessionsConfig {
                    release_wait_secs: 2,
                    ..SessionsConfig::default()
                },
                store: self.store.clone(),
                transport,
                connected,
                peers,
            }
        }

        async fn close(self) {
            self.store.close().await;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn peer(id: &str, reachable: bool, last_seen_ms: u64) -> mesh::Peer {
        mesh::Peer {
            id: mesh::EngineId(id.into()),
            name: id.into(),
            addrs: vec![],
            reachable,
            last_seen_ms,
            hub_pref: mesh::HubPref::Eligible,
        }
    }

    /// A reachable owner: `Release` → `Released{last_seq}`, the taker waits
    /// for the tail, both sides end at epoch 2 with owner B; nothing is
    /// orphaned on A because everything had shipped.
    #[tokio::test]
    async fn take_from_a_reachable_owner_moves_the_tail_and_the_epoch() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        b.pull(&a).await;
        assert_eq!(b.store.fence(id).await.unwrap(), Some((1, "a".into())));
        // The tail B has not seen yet: a message and an open turn (139 §6.2).
        a.store.append(id, &agent_msg("u2")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        let tail = a.store.log_seq();

        let switch = Arc::new(Switch::default());
        let (la, lb) = (Loop::new("a", &switch), Loop::new("b", &switch));
        // A's side: serve the stream, then ship (the shipper runs after the release here).
        let shipper = crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
        let la2 = la.clone();
        let served = tokio::spawn(async move {
            let (peer, stream) = la2.accept(ALPN_SESSIONS).await.unwrap();
            crate::SessionSync::serve(shipper, peer, stream)
                .await
                .unwrap();
        });
        let (sb, sa, bid) = (b.store.clone(), a.store.clone(), b.id());
        let shipper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            for row in sa.log_after(0).await.unwrap() {
                let seq = row.seq;
                if apply_log_row(&sb, &PeerId("a".into()), &row.into())
                    .await
                    .unwrap()
                    == Applied::Applied
                {
                    sa.set_cursor("a", &bid.0, seq).await.unwrap();
                }
            }
        });

        let (got, resumed) = b
            .take(Some(lb.clone()), vec![a.id()], vec![])
            .take(id, TakeReason::Write)
            .await
            .unwrap();
        assert_eq!(got, ("b".into(), 2));
        served.await.unwrap();
        shipper.await.unwrap();
        assert_eq!(
            b.store.messages(id).await.unwrap().len(),
            2,
            "the tail arrived before the take"
        );
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        assert_eq!(b.store.fence(id).await.unwrap(), Some((2, "b".into())));
        // 139 D3/D4: A's release drained the turn — `suspended{release}` is
        // on B at epoch 1 (before B's `set_owner`, or the fence would have
        // dropped it) and B decided to resume it as a transfer from A.
        let plan = b.store.resume_plan(id, turn).await.unwrap();
        assert_eq!(plan.kind, ResumeKind::Suspended);
        assert_eq!(
            plan.cause,
            ResumeCause::Transfer,
            "newest step is suspended{{release}}"
        );
        assert_eq!(resumed, ResumeOnTake::Resume(ResumeCause::Transfer));
        b.store.append(id, &agent_msg("u3 on b")).await.unwrap();

        // B's take row reaches A: A demotes with nothing to orphan.
        assert!(a.pull(&b).await.iter().all(|r| *r == Applied::Applied));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        let n = demote(
            &a.rt,
            &a.store,
            Demote {
                session: id,
                old_epoch: 1,
                by: b.id(),
            },
        )
        .await
        .unwrap();
        assert_eq!(n, 0);
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(
            a.store.messages(id).await.unwrap().len(),
            3,
            "no note when nothing was lost"
        );
        assert!(
            a.store.append(id, &agent_msg("late")).await.is_err(),
            "A is fenced out"
        );
        // 139 I3: the drain's row is the session's last row on A, and it shipped.
        assert!(a.store.session_log_seq(id).await.unwrap() > tail);
        a.close().await;
        b.close().await;
    }

    /// A `Write` take of an unreachable owner waits `takeover_secs` from the
    /// mesh's last sight of it; `User` and `Orphan` take at once.
    #[tokio::test]
    async fn take_from_an_unreachable_owner_respects_takeover_secs() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        b.pull(&a).await;
        let now = now_ms();

        let recent = vec![peer("a", false, now - 100_000)];
        let e = b
            .take(None, vec![], recent.clone())
            .run(id, TakeReason::Write)
            .await
            .unwrap_err();
        assert!(e.contains("takeover after 300s"), "{e}");
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
        let e = b
            .take(None, vec![], vec![peer("a", true, now)])
            .run(id, TakeReason::Write)
            .await
            .unwrap_err();
        assert!(e.contains("reachable"), "{e}");

        assert_eq!(
            b.take(None, vec![], recent)
                .run(id, TakeReason::Orphan)
                .await
                .unwrap(),
            ("b".into(), 2)
        );
        // Already ours: a no-op at the current epoch.
        assert_eq!(
            b.take(None, vec![], vec![])
                .run(id, TakeReason::User)
                .await
                .unwrap(),
            ("b".into(), 2)
        );

        let c = engine("c").await;
        c.pull(&a).await;
        c.pull(&b).await;
        assert_eq!(c.store.fence(id).await.unwrap(), Some((2, "b".into())));
        let old = vec![peer("b", false, now - 301_000)];
        assert_eq!(
            c.take(None, vec![], old)
                .run(id, TakeReason::Write)
                .await
                .unwrap(),
            ("c".into(), 3)
        );
        let e = c
            .take(None, vec![], vec![])
            .run(SessionId::new(), TakeReason::User)
            .await
            .unwrap_err();
        assert!(e.contains("no such session"), "{e}");
        a.close().await;
        b.close().await;
        c.close().await;
    }

    /// 68 §4.3: A wrote two turns nobody received, B took the session
    /// (`User`, at once). When B's row reaches A, A orphans exactly those
    /// two rows, leaves a local note, and A's stale rows are dropped by B.
    #[tokio::test]
    async fn stale_owner_demotes_with_its_unshipped_rows_orphaned() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        b.pull(&a).await;
        // Partition: A keeps writing; B takes.
        a.store.append(id, &agent_msg("u2")).await.unwrap();
        a.store.append(id, &agent_msg("u3")).await.unwrap();
        // 139 D5: the user's take resumes the open turn here as a takeover
        // (no `suspended` row shipped: `kind = Interrupted`).
        let (got, resumed) = b
            .take(None, vec![], vec![peer("a", false, now_ms())])
            .take(id, TakeReason::User)
            .await
            .unwrap();
        assert_eq!(got, ("b".into(), 2));
        assert_eq!(resumed, ResumeOnTake::Resume(ResumeCause::Takeover));
        assert_eq!(
            b.store.resume_plan(id, turn).await.unwrap().kind,
            ResumeKind::Interrupted
        );
        b.store.append(id, &agent_msg("b1")).await.unwrap();

        // A comes back: B's rows land, A demotes.
        let applied = a.pull(&b).await;
        assert!(
            applied.iter().all(|r| *r == Applied::Applied),
            "{applied:?}"
        );
        let n = demote(
            &a.rt,
            &a.store,
            Demote {
                session: id,
                old_epoch: 1,
                by: b.id(),
            },
        )
        .await
        .unwrap();
        assert_eq!(n, 2);
        let orphans = crate::orphans(&a.store, id).await.unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(
            (orphans[0].epoch, orphans[0].to_seq - orphans[0].from_seq),
            (1, 1)
        );
        assert!(!orphans[0].at.is_empty());
        let last = a.store.messages(id).await.unwrap().pop().unwrap().1;
        let text = last.content.joined_texts().unwrap_or_default();
        assert!(
            text.starts_with("2 turns from ") && text.contains("on a;"),
            "{text}"
        );
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        // A's stale rows never ship (orphans leave `log_after`); forced through, B drops them.
        assert!(
            a.store
                .log_after(0)
                .await
                .unwrap()
                .iter()
                .all(|r| r.epoch != 1 || r.seq <= 3)
        );
        for row in a.store.log_orphans(id).await.unwrap() {
            assert_eq!(
                apply_log_row(&b.store, &a.id(), &row.into()).await.unwrap(),
                Applied::StaleEpoch
            );
        }
        assert!(a.store.append(id, &agent_msg("late")).await.is_err());
        // Release on a non-owner is harmless: the session's last seq, nothing stopped.
        assert_eq!(
            release(&b.store, id).await.unwrap(),
            b.store.session_log_seq(id).await.unwrap() as u64
        );
        a.close().await;
        b.close().await;
    }

    fn agent_msg(t: &str) -> genai::chat::ChatMessage {
        genai::chat::ChatMessage::user(t)
    }

    /// Open a turn of `id` on `store` (its `input_accepted` step) and return it.
    async fn open_turn_on(store: &Store, id: SessionId) -> runtime::TurnId {
        let turn = runtime::TurnId::new();
        store
            .step(
                id,
                turn,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        turn
    }

    /// Collects the runtime's `AgentEvent`s.
    #[derive(kameo::Actor)]
    struct Sink {
        events: Arc<parking_lot::Mutex<Vec<AgentEvent>>>,
    }

    impl Message<AgentEvent> for Sink {
        type Reply = ();

        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            self.events.lock().push(e);
        }
    }

    async fn sink(rt: &Runtime) -> Arc<parking_lot::Mutex<Vec<AgentEvent>>> {
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let actor = kameo::actor::Spawn::spawn(Sink {
            events: events.clone(),
        });
        rt.subscribe::<Sink, AgentEvent>(&actor).await;
        events
    }

    /// 139 §6.3 (D5, Q10): the policy table as ruled.
    #[test]
    fn resume_policy_follows_the_table() {
        use ResumeOnTake::*;
        for reason in [TakeReason::User, TakeReason::Write, TakeReason::Orphan] {
            assert_eq!(
                resume_policy(true, reason, ResumeMode::Auto),
                Resume(ResumeCause::Transfer)
            );
            assert_eq!(resume_policy(true, reason, ResumeMode::Ask), Pending);
            assert_eq!(resume_policy(true, reason, ResumeMode::Off), Nothing);
        }
        for mode in [ResumeMode::Auto, ResumeMode::Ask, ResumeMode::Off] {
            assert_eq!(
                resume_policy(false, TakeReason::User, mode),
                Resume(ResumeCause::Takeover)
            );
            assert_eq!(resume_policy(false, TakeReason::Write, mode), Pending);
            assert_eq!(resume_policy(false, TakeReason::Orphan, mode), Pending);
        }
    }

    /// 139 D5: a system takeover (`Write` after `takeover_secs`, `Orphan`)
    /// of a session with an open turn does not resume it — `ResumePending`
    /// is published and the transcript gets the local note; a session with
    /// no open turn publishes nothing.
    #[tokio::test]
    async fn system_takeover_leaves_the_open_turn_pending() {
        let (a, b) = (engine("a").await, engine("b").await);
        let events = sink(&b.rt).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        b.pull(&a).await;
        let before = b.store.messages(id).await.unwrap().len();

        let old = vec![peer("a", false, now_ms() - 301_000)];
        let (got, resumed) = b
            .take(None, vec![], old.clone())
            .take(id, TakeReason::Write)
            .await
            .unwrap();
        assert_eq!((got, resumed), (("b".into(), 2), ResumeOnTake::Pending));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            events.lock().iter().any(|e| matches!(
                e,
                AgentEvent::ResumePending { session, turn: t, from } if *session == id && *t == turn && from == "a"
            )),
            "{:?}",
            events.lock()
        );
        let messages = b.store.messages(id).await.unwrap();
        assert_eq!(messages.len(), before + 1);
        let text = messages
            .last()
            .unwrap()
            .1
            .content
            .joined_texts()
            .unwrap_or_default();
        assert!(
            text.starts_with("taken from a at ") && text.contains("press Resume"),
            "{text}"
        );
        // The turn is still open on B: the user's Resume runs it.
        assert_eq!(
            b.store.open_turn(id).await.unwrap().map(|(t, _)| t),
            Some(turn)
        );

        // Orphan: the same, from a third engine; no open turn → nothing.
        let c = engine("c").await;
        let c_events = sink(&c.rt).await;
        c.pull(&a).await;
        c.pull(&b).await;
        let closed = SessionId::new();
        b.store
            .create_session(closed, "m", None, None, None)
            .await
            .unwrap();
        c.pull(&b).await;
        let (_, resumed) = c
            .take(None, vec![], vec![peer("b", false, now_ms())])
            .take(id, TakeReason::Orphan)
            .await
            .unwrap();
        assert_eq!(resumed, ResumeOnTake::Pending);
        let (_, resumed) = c
            .take(None, vec![], vec![peer("b", false, now_ms())])
            .take(closed, TakeReason::Orphan)
            .await
            .unwrap();
        assert_eq!(resumed, ResumeOnTake::NoOpenTurn);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let pending: Vec<SessionId> = c_events
            .lock()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ResumePending { session, .. } => Some(*session),
                _ => None,
            })
            .collect();
        assert_eq!(pending, [id]);
        a.close().await;
        b.close().await;
        c.close().await;
    }
}
