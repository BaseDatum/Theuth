//! Hub duty (69 §2): what link follows.
//!
//! The **lease** — Slack's pattern: a [`mesh::lease::Follower`] fed by
//! [`mesh::MeshEvent::Lease`], the self id from `lookup_when::<mesh::Mesh>`
//! (link starts before mesh), and a reconcile ([`LinkActor::sync_mesh`])
//! that owns the mesh uplink. Gain → load `mesh/device_key`, start
//! [`MeshUplink`] at `/h/<mesh device id>`. Loss → [`MeshUplink::stop`]: the
//! supervisor does not redial and every mesh device conn hears `CLOSE 5`.
//! No mesh plugin (a lone engine) → `Follower::default().holding` is true
//! and it serves from the start.
//!
//! The **replicated tables** — the `Devices`/`Pending` mirrors are re-read
//! from `link.sqlite` every [`REFRESH_EVERY`] so rows other engines wrote (a
//! QR shown on engine B, a revoke on engine C) reach this engine; the hub
//! needs them to answer handshakes, every engine for `devices_list`.

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;

use crate::LinkActor;
use crate::devices::Row;
use crate::identity::{Identity, now_ms};
use crate::mesh_uplink::MeshUplink;
use crate::state::Ctx;
use crate::store::{PendingRow, Store};
use crate::types::LinkEvent;

/// How often the mirrors are re-read from the table.
// ponytail: a poll; the replica has no change feed. Replace with one when it grows one.
const REFRESH_EVERY: Duration = Duration::from_secs(3);
/// A failed mesh identity load (the vault not up, say) is retried after this.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// How long the first reconcile waits for the mesh plugin before treating this as a lone engine.
const MESH_WAIT: Duration = Duration::from_secs(10);

/// Internal: the mesh answered `Status` with this engine's id and the lease as it stands (a
/// restarting engine with stored peers says `holder: None` until one connects).
pub(crate) struct SetSelf(pub String, pub Option<mesh::LeaseState>);

/// Internal: the vault answered for `mesh/device_key` — `Ok(None)` = no mesh identity yet (no
/// device ever paired; the row arrives when one does, on any engine) — or failed.
struct MeshIdentityLoaded(Result<Option<Identity>, String>);

/// Internal: run the reconcile (start, an `Enable`, a retry).
pub(crate) struct SyncMesh;

/// Subscribe the actor to the lease, resolve the self id, start the table
/// refresh, and reconcile once (a lone engine serves from the start).
pub(crate) async fn start(
    rt: &Runtime,
    actor: &ActorRef<LinkActor>,
    ctx: Arc<Ctx>,
    store: Option<Store>,
) {
    rt.subscribe::<LinkActor, mesh::MeshEvent>(actor).await;
    // The mesh identity row may land after this engine became hub (a pairing shown on another
    // engine mints it; replication brings it): the vault's change event re-runs the reconcile.
    rt.subscribe::<LinkActor, secrets::SecretsEvent>(actor)
        .await;
    let me = actor.clone();
    let rt2 = rt.clone();
    tokio::spawn(async move {
        // The first reconcile waits for the mesh's self id (the lease it emits at start settles
        // against it) so a restarting engine does not register the mesh id for the 100 ms before
        // it learns another engine holds the lease — every device would bounce. No mesh within
        // MESH_WAIT (a slimmer plugin set) → a lone engine, serve.
        let mesh =
            tokio::time::timeout(MESH_WAIT, rt2.lookup_when::<mesh::Mesh>(mesh::MESH_ACTOR)).await;
        match mesh {
            Ok(Some(mesh)) => match mesh.ask(mesh::Status).await {
                Ok(st) => {
                    let _ = me.tell(SetSelf(st.self_id.0, Some(st.lease))).await;
                }
                Err(_) => {
                    let _ = me.tell(MeshAbsent).await;
                }
            },
            _ => {
                let _ = me.tell(MeshAbsent).await;
            }
        }
    });
    if let Some(store) = store {
        tokio::spawn(refresh(ctx, store));
    }
}

impl Message<mesh::MeshEvent> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: mesh::MeshEvent, ctx: &mut Context<Self, ()>) {
        if let mesh::MeshEvent::Lease(l) = m {
            let change = self.follower.observe(&l);
            self.lease_changed(change, ctx.actor_ref().clone()).await;
        }
    }
}

impl Message<secrets::SecretsEvent> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: secrets::SecretsEvent, ctx: &mut Context<Self, ()>) {
        let wanted =
            matches!(m, secrets::SecretsEvent::Changed) && self.ctx.lock().mesh_identity.is_none();
        if wanted {
            self.sync_mesh(ctx.actor_ref().clone()).await;
        }
    }
}

impl Message<SetSelf> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: SetSelf, ctx: &mut Context<Self, ()>) {
        let seen = m.1.and_then(|l| self.follower.observe(&l));
        let change = self.follower.set_self(m.0).or(seen);
        self.mesh_ready = true;
        self.lease_changed(change, ctx.actor_ref().clone()).await;
    }
}

/// Internal: no mesh plugin answered within [`MESH_WAIT`] — a lone engine; serve.
struct MeshAbsent;

impl Message<MeshAbsent> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, _: MeshAbsent, ctx: &mut Context<Self, ()>) {
        self.mesh_ready = true;
        self.sync_mesh(ctx.actor_ref().clone()).await;
    }
}

impl Message<MeshIdentityLoaded> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: MeshIdentityLoaded, ctx: &mut Context<Self, ()>) {
        self.mesh_loading = false;
        let me = ctx.actor_ref().clone();
        match m.0 {
            Err(e) => {
                tracing::warn!(
                    "mesh identity unavailable ({e}); the mesh uplink retries in {RETRY_AFTER:?}"
                );
                let later = me.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RETRY_AFTER).await;
                    let _ = later.tell(SyncMesh).await;
                });
            }
            Ok(None) => tracing::debug!(
                "no mesh identity yet: the mesh uplink starts once a device pairing mints it"
            ),
            Ok(Some(_)) => self.sync_mesh(me).await,
        }
    }
}

impl Message<SyncMesh> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, _: SyncMesh, ctx: &mut Context<Self, ()>) {
        self.sync_mesh(ctx.actor_ref().clone()).await;
    }
}

impl LinkActor {
    /// One log line per transition, then the reconcile.
    async fn lease_changed(&mut self, change: Option<bool>, me: ActorRef<Self>) {
        match change {
            Some(true) => tracing::info!("hub lease gained: link serves paired devices"),
            Some(false) => tracing::info!("hub lease lost: link stops serving paired devices"),
            None => {}
        }
        self.sync_mesh(me).await;
    }

    /// Reconcile `follower.holding && cfg.enabled` with the running mesh
    /// uplink. Idempotent. The identity comes from the vault (cached in the
    /// state) — an existing row only, never minted here: `device_pair_start`
    /// mints. While it loads the reconcile returns and runs again when
    /// [`MeshIdentityLoaded`] arrives; no row → again on `SecretsEvent::Changed`.
    pub(crate) async fn sync_mesh(&mut self, me: ActorRef<Self>) {
        let (want, identity, relay) = {
            let st = self.ctx.lock();
            (
                self.mesh_ready && self.follower.holding && st.cfg.enabled,
                st.mesh_identity.clone(),
                st.cfg.relay.clone(),
            )
        };
        match (want, self.mesh.take()) {
            (true, Some(up)) => self.mesh = Some(up),
            (true, None) => match identity {
                Some(id) => {
                    tracing::info!(id = %id.id, %relay, "mesh uplink starting: this engine is the hub");
                    let token = self.ctx.rt.shutdown.child_token();
                    self.mesh = Some(MeshUplink::start(self.ctx.clone(), id, relay, token));
                }
                None if !self.mesh_loading => {
                    self.mesh_loading = true;
                    let ctx = self.ctx.clone();
                    tokio::spawn(async move {
                        let r = crate::existing_mesh_identity(&ctx).await;
                        let _ = me.tell(MeshIdentityLoaded(r)).await;
                    });
                }
                None => {}
            },
            (false, Some(up)) => {
                tracing::info!("mesh uplink stopping: not the hub (or link disabled); no redial");
                up.stop();
            }
            (false, None) => {}
        }
    }
}

/// Re-read both tables forever (until shutdown).
async fn refresh(ctx: Arc<Ctx>, store: Store) {
    let mut tick = tokio::time::interval(REFRESH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ctx.rt.shutdown.cancelled() => return,
            _ = tick.tick() => {}
        }
        match store.devices().await {
            Ok(rows) => {
                if reload_devices(&ctx, rows) {
                    ctx.rt.publish(LinkEvent::DevicesChanged).await;
                }
            }
            Err(e) => tracing::debug!("link.sqlite: devices: {e:#}"),
        }
        match store.pending().await {
            Ok(rows) => {
                reload_pending(&ctx, rows);
            }
            Err(e) => tracing::debug!("link.sqlite: pending: {e:#}"),
        }
    }
}

/// Mirror the table when it holds a row the mirror does not have in that
/// form (a pairing, a revoke, a rename from another engine); `false` when
/// nothing changed. The table may also *lag* the mirror by a queued local
/// write — that direction is left alone. A device revoked elsewhere loses
/// its conns here (`CLOSE 0`, as a local revoke).
pub(crate) fn reload_devices(ctx: &Ctx, rows: Vec<Row>) -> bool {
    let mut st = ctx.lock();
    if !rows.iter().any(|r| !st.devices.all().contains(r)) {
        return false;
    }
    let revoked: Vec<[u8; 32]> = rows
        .iter()
        .filter(|r| !r.active() && st.devices.by_key(&r.key).is_some())
        .map(|r| r.key)
        .collect();
    st.devices.reload(rows);
    if !revoked.is_empty() {
        for c in st
            .all_conns()
            .filter(|c| c.device.is_some_and(|k| revoked.contains(&k)))
        {
            c.cancel.cancel();
        }
    }
    true
}

/// Mirror the `pending` table when it holds a live row the mirror lacks (a
/// QR shown on another engine); `false` when nothing changed.
pub(crate) fn reload_pending(ctx: &Ctx, rows: Vec<PendingRow>) -> bool {
    let mut st = ctx.lock();
    let now = now_ms();
    if !rows
        .iter()
        .any(|r| r.expires_ms > now && st.pending.expires_ms(&r.id).is_none())
    {
        return false;
    }
    st.pending.reload(rows);
    true
}

#[cfg(test)]
mod tests {
    use kameo::actor::Spawn;

    use super::*;
    use crate::mesh_uplink::testing::{Event, FakeRelay, ctx, identity, until};
    use crate::state::ConnHandle;
    use crate::types::Status;
    use crate::uplink::{CLOSE_HUB_MOVED, OP_CLOSE};

    fn lease(holder: &str) -> mesh::MeshEvent {
        mesh::MeshEvent::Lease(mesh::LeaseState {
            holder: Some(mesh::EngineId(holder.into())),
            since_ms: 0,
            reason: "elected".into(),
        })
    }

    /// The whole duty through the actor: a lone engine serves from the start
    /// (engine and mesh uplinks both register); the lease going to another
    /// engine stops the mesh uplink — `CLOSE 5` to its device conn, no
    /// redial, `mesh.connected = false` — while the engine uplink is
    /// untouched; the lease coming back registers the mesh identity again.
    #[tokio::test]
    async fn follower_gain_and_loss_drive_the_mesh_uplink_only() {
        let engine = identity();
        let mesh_id = identity();
        let mut relay = FakeRelay::start(vec![engine.clone(), mesh_id.clone()]).await;
        let ctx = ctx(&relay.url, Some(engine.clone()), Some(mesh_id.clone()));
        let mut actor = LinkActor {
            ctx: ctx.clone(),
            config_path: std::env::temp_dir().join("theuth-link-follow-test.yaml"),
            uplink: None,
            pair_timer: None,
            follower: mesh::lease::Follower::default(),
            mesh: None,
            mesh_loading: false,
            mesh_ready: true,
        };
        actor.start_uplink();
        let actor = LinkActor::spawn(actor);
        actor.tell(SyncMesh).await.unwrap();

        let mut registered = vec![relay.next().await, relay.next().await];
        registered.sort_by_key(|e| format!("{e:?}"));
        let mut want = vec![
            Event::Registered(engine.id.clone()),
            Event::Registered(mesh_id.id.clone()),
        ];
        want.sort_by_key(|e| format!("{e:?}"));
        assert_eq!(
            registered, want,
            "a lone engine is its own hub: both identities serve"
        );
        until(|| {
            let st = ctx.lock();
            st.mesh.as_ref().is_some_and(|m| m.connected)
                && st.connected
                && st.mesh_conns.contains_key(&1)
        })
        .await;

        // Knowing our id changes nothing until a lease names someone.
        actor.tell(SetSelf("a".into(), None)).await.unwrap();
        // Lost: another engine holds the lease.
        actor.tell(lease("b")).await.unwrap();
        relay
            .expect(|e| matches!(e, Event::Frame(id, OP_CLOSE, 1, p) if *id == mesh_id.id && p.as_ref() == [CLOSE_HUB_MOVED]))
            .await;
        assert_eq!(relay.next().await, Event::Closed(mesh_id.id.clone()));
        let quiet = relay.drain(Duration::from_millis(2500)).await;
        assert!(
            !quiet
                .iter()
                .any(|e| matches!(e, Event::Registered(_) | Event::Closed(_))),
            "no redial, and the engine socket stays: {quiet:?}"
        );
        let st = actor.ask(Status).await.unwrap();
        assert!(st.connected, "engine uplink untouched");
        assert_eq!(st.mesh.as_ref().map(|m| m.connected), Some(false));
        assert!(ctx.lock().mesh_conns.is_empty());

        // Gained: the lease is ours again.
        actor.tell(lease("a")).await.unwrap();
        assert_eq!(relay.next().await, Event::Registered(mesh_id.id.clone()));
        until(|| ctx.lock().mesh.as_ref().is_some_and(|m| m.connected)).await;
        // A repeated lease for us is not a transition: nothing restarts.
        actor.tell(lease("a")).await.unwrap();
        let quiet = relay.drain(Duration::from_millis(300)).await;
        assert!(
            !quiet
                .iter()
                .any(|e| matches!(e, Event::Registered(_) | Event::Closed(_))),
            "{quiet:?}"
        );
    }

    /// A revoke replicated from another engine: the mirror reloads, the
    /// device's live conns are cancelled, and a table that merely lags a
    /// local write is left alone.
    #[tokio::test]
    async fn replicated_rows_reload_and_a_replicated_revoke_closes_conns() {
        let ctx = ctx("ws://x", None, None);
        let key = [7u8; 32];
        let row = Row {
            id: crate::devices::device_id(&key),
            key,
            name: "iPhone".into(),
            platform: "ios".into(),
            paired_at_ms: 1,
            last_seen_ms: 0,
            revoked_ms: None,
        };
        assert!(
            reload_devices(&ctx, vec![row.clone()]),
            "a row from another engine"
        );
        assert!(!reload_devices(&ctx, vec![row.clone()]), "same rows again");

        // A local pairing the table has not caught up with yet: not a reload.
        let local = Row {
            key: [8u8; 32],
            id: crate::devices::device_id(&[8u8; 32]),
            ..row.clone()
        };
        ctx.lock().devices.insert(local.clone());
        assert!(
            !reload_devices(&ctx, vec![row.clone()]),
            "the table lags; keep the mirror"
        );
        assert_eq!(ctx.lock().devices.len(), 2);

        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let cancel = tokio_util::sync::CancellationToken::new();
        ctx.lock().mesh_conns.insert(
            9,
            ConnHandle {
                device: Some(key),
                inbound: tx,
                cancel: cancel.clone(),
            },
        );
        let revoked = Row {
            revoked_ms: Some(5),
            ..row
        };
        assert!(reload_devices(&ctx, vec![revoked, local]));
        assert!(cancel.is_cancelled(), "the revoked device's conn is closed");
        assert!(ctx.lock().devices.by_key(&key).is_none());
        assert_eq!(ctx.lock().devices.all().len(), 2);
    }
}
