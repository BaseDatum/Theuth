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
use agent::store::Store;
use anyhow::Context as _;
use futures::{SinkExt as _, StreamExt as _};
use kameo::actor::ActorRef;
use replica::{ALPN_SESSIONS, PeerId, Transport};
use runtime::{Runtime, SessionId};

use crate::types::Msg;
use crate::{SessionsConfig, TakeReason};

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

impl Take {
    /// Take `id` here; returns the new `(owner, epoch)`. A session already
    /// ours (or unowned, lone engine) is a no-op.
    pub async fn run(self, id: SessionId, reason: TakeReason) -> Result<(String, u32), String> {
        let me = self.store.engine().to_owned();
        let (epoch, owner) = self
            .store
            .fence(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{id}: no such session"))?;
        if owner.is_empty() || owner == me {
            return Ok((owner, epoch));
        }
        if me.is_empty() {
            return Err("this engine has no mesh identity".to_owned());
        }
        let peer = PeerId(owner.clone());
        if self.connected.contains(&peer) {
            match self.release_from(&peer, id).await {
                Ok(()) => {}
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
            other => return Err(format!("{id}: ownership moved to {other:?} during the take")),
        }
        unload(id).await;
        let next = epoch + 1;
        self.store.set_owner(id, &me, next).await.map_err(|e| format!("{e:#}"))?;
        self.store.set_fence(id, next, &me).await.map_err(|e| format!("{e:#}"))?;
        self.rt
            .publish(AgentEvent::SessionOwnerChanged {
                session: id,
                owner: me.clone(),
                epoch: next,
                here: true,
            })
            .await;
        Ok((me, next))
    }

    /// `Release` → `Released{last_seq}` over `theuth/sessions/1`, then wait
    /// until our cursor for the owner reached `last_seq`; each bounded by
    /// `release_wait_secs`.
    async fn release_from(&self, peer: &PeerId, id: SessionId) -> anyhow::Result<()> {
        let t = self.transport.as_ref().context("no transport")?;
        let wait = Duration::from_secs(self.config.release_wait_secs);
        let mut framed = crate::events::framed(t.open(peer, ALPN_SESSIONS).await?);
        framed.send(Msg::Release { session: id.to_string() }.encode()?).await?;
        let frame = tokio::time::timeout(wait, framed.next())
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
            anyhow::ensure!(Instant::now() < deadline, "tail not replicated: cursor {applied} < {last_seq}");
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
            return Err(format!("owner {owner} is reachable but has no sessions stream yet; retry"));
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

/// Owner side of a take (68 §4.2 step 1): stop the live actor, flush the
/// write connection, answer with `MAX(seq)` of the session's log rows. The
/// session stays readable here; the taker's row flips `owner` when it lands.
// ponytail: a turn that starts between the kill and the taker's row still
// writes and becomes an orphan on demote; a local "releasing" mark would close it.
pub async fn release(store: &Store, id: SessionId) -> anyhow::Result<u64> {
    unload(id).await;
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
    framed.send(Msg::Released { session, last_seq }.encode()?).await?;
    Ok(())
}

/// 68 §4.3: we were the owner in `old_epoch`; `by` now owns it. Stop our
/// actor, orphan our unshipped rows of that epoch (those above what `by`
/// had applied of our log), raise the fence, tell the UI, and leave a
/// local-only note in the transcript. Returns how many rows were orphaned.
pub async fn demote(rt: &Runtime, store: &Store, m: Demote) -> anyhow::Result<u64> {
    unload(m.session).await;
    let me = store.engine();
    let shipped = store
        .cursors()
        .await?
        .into_iter()
        .find(|c| c.origin == me && c.peer == m.by.0)
        .map_or(0, |c| c.seq);
    let n = store.log_orphan_after(m.session, m.old_epoch, shipped).await?;
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
                &format!("{n} turns from {from} were not delivered on {me}; `session_orphans` to view"),
            )
            .await?;
    }
    Ok(n)
}

/// Called after a row of `origin` was applied: if it raised the epoch of a
/// session we owned, the actor demotes (a `tell`, so the apply path never
/// waits on it).
pub(crate) async fn on_applied(store: &Store, origin: &PeerId, session: SessionId, before: Option<(u32, String)>, epoch: u32) {
    let Some((old, owner)) = before else { return };
    if epoch <= old || owner.is_empty() || owner != store.engine() {
        return;
    }
    let m = Demote { session, old_epoch: old, by: origin.clone() };
    match crate::actor() {
        Some(a) => {
            let _ = a.tell(m).await;
        }
        None => tracing::warn!("sessions: {session} taken by {origin} but no sessions actor to demote"),
    }
}

/// Stop the live actor of `id`, if the agent supervisor is running.
async fn unload(id: SessionId) {
    if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        let _ = agents.ask(agent::UnloadSession { session: id }).await;
    }
}

/// Hourly until shutdown: delete orphaned rows older than `keep_days`
/// (`sessions.orphan_keep_days`, 68 §4.3). The demoted owner's note stays.
pub async fn sweep_orphans(store: Store, keep_days: u64, shutdown: tokio_util::sync::CancellationToken) {
    loop {
        let before = now_ms().saturating_sub(keep_days * 86_400_000);
        match store.prune_orphans_before(before).await {
            Ok(n) if n > 0 => tracing::info!("sessions: pruned {n} orphaned log rows older than {keep_days} days"),
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
            Arc::new(Self { me, switch: switch.clone(), inbox: tokio::sync::Mutex::new(rx), events: broadcast::channel(8).0 })
        }
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.switch.0.lock().keys().filter(|p| **p != self.me).cloned().collect()
        }
        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let inboxes = self.switch.0.lock();
            let to = inboxes.get(peer).ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs))).map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }
        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("switch closed"))
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
        let dir = std::env::temp_dir().join(format!("theuth-owner-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite"), Some(mesh::EngineId(name.into()))).await.unwrap();
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
                let r = apply_log_row(&self.store, &origin, &row.into()).await.unwrap();
                if r != Applied::Duplicate {
                    from.store.set_cursor(&origin.0, self.store.engine(), seq).await.unwrap();
                }
                out.push(r);
            }
            out
        }

        fn take(&self, transport: Option<Arc<dyn Transport>>, connected: Vec<PeerId>, peers: Vec<mesh::Peer>) -> Take {
            Take {
                rt: self.rt.clone(),
                config: SessionsConfig { release_wait_secs: 2, ..SessionsConfig::default() },
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
        a.store.create_session(id, "m", None, None, None).await.unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        b.pull(&a).await;
        assert_eq!(b.store.fence(id).await.unwrap(), Some((1, "a".into())));
        // The tail B has not seen yet.
        a.store.append(id, &agent_msg("u2")).await.unwrap();
        let tail = a.store.log_seq();

        let switch = Arc::new(Switch::default());
        let (la, lb) = (Loop::new("a", &switch), Loop::new("b", &switch));
        // A's side: serve the stream, then ship (the shipper runs after the release here).
        let shipper = crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
        let la2 = la.clone();
        let served = tokio::spawn(async move {
            let (peer, stream) = la2.accept(ALPN_SESSIONS).await.unwrap();
            crate::SessionSync::serve(shipper, peer, stream).await.unwrap();
        });
        let (sb, sa, bid) = (b.store.clone(), a.store.clone(), b.id());
        let shipper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            for row in sa.log_after(0).await.unwrap() {
                let seq = row.seq;
                if apply_log_row(&sb, &PeerId("a".into()), &row.into()).await.unwrap() == Applied::Applied {
                    sa.set_cursor("a", &bid.0, seq).await.unwrap();
                }
            }
        });

        let got = b.take(Some(lb.clone()), vec![a.id()], vec![]).run(id, TakeReason::Write).await.unwrap();
        assert_eq!(got, ("b".into(), 2));
        served.await.unwrap();
        shipper.await.unwrap();
        assert_eq!(b.store.messages(id).await.unwrap().len(), 2, "the tail arrived before the take");
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        assert_eq!(b.store.fence(id).await.unwrap(), Some((2, "b".into())));
        b.store.append(id, &agent_msg("u3 on b")).await.unwrap();

        // B's take row reaches A: A demotes with nothing to orphan.
        assert!(a.pull(&b).await.iter().all(|r| *r == Applied::Applied));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        let n = demote(&a.rt, &a.store, Demote { session: id, old_epoch: 1, by: b.id() }).await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(a.store.messages(id).await.unwrap().len(), 3, "no note when nothing was lost");
        assert!(a.store.append(id, &agent_msg("late")).await.is_err(), "A is fenced out");
        assert_eq!(a.store.session_log_seq(id).await.unwrap(), tail);
        a.close().await;
        b.close().await;
    }

    /// A `Write` take of an unreachable owner waits `takeover_secs` from the
    /// mesh's last sight of it; `User` and `Orphan` take at once.
    #[tokio::test]
    async fn take_from_an_unreachable_owner_respects_takeover_secs() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store.create_session(id, "m", None, None, None).await.unwrap();
        b.pull(&a).await;
        let now = now_ms();

        let recent = vec![peer("a", false, now - 100_000)];
        let e = b.take(None, vec![], recent.clone()).run(id, TakeReason::Write).await.unwrap_err();
        assert!(e.contains("takeover after 300s"), "{e}");
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
        let e = b.take(None, vec![], vec![peer("a", true, now)]).run(id, TakeReason::Write).await.unwrap_err();
        assert!(e.contains("reachable"), "{e}");

        assert_eq!(b.take(None, vec![], recent).run(id, TakeReason::Orphan).await.unwrap(), ("b".into(), 2));
        // Already ours: a no-op at the current epoch.
        assert_eq!(b.take(None, vec![], vec![]).run(id, TakeReason::User).await.unwrap(), ("b".into(), 2));

        let c = engine("c").await;
        c.pull(&a).await;
        c.pull(&b).await;
        assert_eq!(c.store.fence(id).await.unwrap(), Some((2, "b".into())));
        let old = vec![peer("b", false, now - 301_000)];
        assert_eq!(c.take(None, vec![], old).run(id, TakeReason::Write).await.unwrap(), ("c".into(), 3));
        let e = c.take(None, vec![], vec![]).run(SessionId::new(), TakeReason::User).await.unwrap_err();
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
        a.store.create_session(id, "m", None, None, None).await.unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        b.pull(&a).await;
        // Partition: A keeps writing; B takes.
        a.store.append(id, &agent_msg("u2")).await.unwrap();
        a.store.append(id, &agent_msg("u3")).await.unwrap();
        assert_eq!(b.take(None, vec![], vec![peer("a", false, now_ms())]).run(id, TakeReason::User).await.unwrap(), ("b".into(), 2));
        b.store.append(id, &agent_msg("b1")).await.unwrap();

        // A comes back: B's rows land, A demotes.
        let applied = a.pull(&b).await;
        assert!(applied.iter().all(|r| *r == Applied::Applied), "{applied:?}");
        let n = demote(&a.rt, &a.store, Demote { session: id, old_epoch: 1, by: b.id() }).await.unwrap();
        assert_eq!(n, 2);
        let orphans = crate::orphans(&a.store, id).await.unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!((orphans[0].epoch, orphans[0].to_seq - orphans[0].from_seq), (1, 1));
        assert!(!orphans[0].at.is_empty());
        let last = a.store.messages(id).await.unwrap().pop().unwrap().1;
        let text = last.content.joined_texts().unwrap_or_default();
        assert!(text.starts_with("2 turns from ") && text.contains("on a;"), "{text}");
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        // A's stale rows never ship (orphans leave `log_after`); forced through, B drops them.
        assert!(a.store.log_after(0).await.unwrap().iter().all(|r| r.epoch != 1 || r.seq <= 2));
        for row in a.store.log_orphans(id).await.unwrap() {
            assert_eq!(apply_log_row(&b.store, &a.id(), &row.into()).await.unwrap(), Applied::StaleEpoch);
        }
        assert!(a.store.append(id, &agent_msg("late")).await.is_err());
        // Release on a non-owner is harmless: the session's last seq, nothing stopped.
        assert_eq!(release(&b.store, id).await.unwrap(), b.store.session_log_seq(id).await.unwrap() as u64);
        a.close().await;
        b.close().await;
    }

    fn agent_msg(t: &str) -> genai::chat::ChatMessage {
        genai::chat::ChatMessage::user(t)
    }
}
