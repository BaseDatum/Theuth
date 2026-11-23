//! 210 §5.7 (D22): the engine's removal duties over the real vault and edges. The lease holder
//! starts a rekey on a tombstone (live from `mesh::share::subscribe_removals`, and reconciled
//! with `mesh::RemovedPeers` at start and on every lease gain), and advances every open
//! `share_rekeys` row once a minute. Any engine whose stash is not empty runs the heal rule.
//! None of this consults the sharing switch; nothing dials unless `share_edges` has a row.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use share_proto::{ErrorCode, Hello, PublicKey, SecretKey, SignedRekey, limits};

use crate::edge::{Control, EdgeDialer, EdgeError};
use crate::rekey::{EdgeRow, MeshKeys, RekeyAnswer, RekeyEdges, Rekeyer, Stash};
use crate::store::Store;

/// How often open rows are advanced.
const TICK: Duration = Duration::from_secs(60);

/// [`MeshKeys`] over the vault behind the mesh endpoint.
pub struct VaultMeshKeys(pub Arc<secrets::Vault>);

#[async_trait::async_trait]
impl MeshKeys for VaultMeshKeys {
    fn current(&self) -> anyhow::Result<Option<(u32, SecretKey)>> {
        Ok(self
            .0
            .mesh_share_key_if_set()?
            .map(|(v, b)| (v, SecretKey::from_bytes(&b))))
    }

    async fn replace(&self, fresh: &SecretKey, expected: u32) -> anyhow::Result<Option<u32>> {
        self.0
            .replace_mesh_share_key(&fresh.to_bytes(), expected)
            .await
    }

    fn vault_members(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.0.current_key().members)
    }

    fn synced(&self) -> bool {
        mesh::synced()
    }
}

/// [`RekeyEdges`] over fresh connections: dial the row's ticket, `GET /v1/info`, `hello` with a
/// delegation for this engine signed by the given key, then the call.
pub struct DialedEdges {
    /// The mesh endpoint's dialer.
    pub dialer: Arc<dyn EdgeDialer>,
    /// This engine's endpoint id.
    pub engine: PublicKey,
}

impl DialedEdges {
    /// `Ok(Err(to))`: the `hello` answered `mesh_rekeyed {to}`.
    async fn attach(&self, e: &EdgeRow, key: &SecretKey) -> anyhow::Result<Result<Control, PublicKey>> {
        let t: share_proto::ShareEdgeTicket = e.ticket.parse().context("share_edges ticket")?;
        let conn = self.dialer.dial(t.addr.clone(), share_proto::ALPNS).await?;
        let control = Control::new(conn);
        let info = control.info().await?;
        anyhow::ensure!(info.edge_id == e.edge, "ticket dials another edge");
        let delegation = share_proto::Delegation::new(
            info.domain.clone(),
            info.edge_id,
            key.public(),
            self.engine,
            platform::time::now_ms(),
            info.now_ms,
        )
        .sign(key)?;
        let hello = Hello {
            v: share_proto::PROTOCOL_MAJOR,
            delegation,
            invite: None,
            client: share_proto::version::client_agent(platform::version::get()),
            terms_rev: None,
        };
        match control.hello(&hello).await {
            Ok(_) => Ok(Ok(control)),
            Err(EdgeError::Http { body, .. }) if body.code == ErrorCode::MeshRekeyed && body.to.is_some() => {
                Ok(Err(body.to.expect("checked")))
            }
            Err(err) => Err(err.into()),
        }
    }
}

#[async_trait::async_trait]
impl RekeyEdges for DialedEdges {
    async fn revoke(&self, e: &EdgeRow, key: &SecretKey, engines: &[PublicKey]) -> anyhow::Result<()> {
        match self.attach(e, key).await? {
            Ok(c) => {
                c.revoke(engines.to_vec()).await?;
                Ok(())
            }
            // The mesh already moved on: the rekey's revoke list carries them.
            Err(_) => Ok(()),
        }
    }

    async fn rekey(&self, e: &EdgeRow, key: &SecretKey, signed: &SignedRekey) -> anyhow::Result<RekeyAnswer> {
        match self.attach(e, key).await? {
            Ok(c) => crate::rekey::rekey_answer(c.rekey(signed).await),
            Err(to) => Ok(RekeyAnswer::Rekeyed(to)),
        }
    }

    async fn mesh_id(&self, e: &EdgeRow, key: &SecretKey) -> anyhow::Result<Result<PublicKey, PublicKey>> {
        match self.attach(e, key).await? {
            Ok(c) => Ok(Ok(c.mesh().await?.mesh)),
            Err(to) => Ok(Err(to)),
        }
    }
}

/// Run the duties until the process ends. Needs the mesh endpoint and vault; waits for them.
pub async fn run(store: Store, data_dir: std::path::PathBuf, dialer: Arc<dyn EdgeDialer>) {
    let stash = Stash::in_data_dir(&data_dir);
    let mut removals = None;
    let mut was_holder = false;
    let mut tick = tokio::time::interval(TICK);
    loop {
        let removed_now = match removals.as_mut() {
            None => {
                removals = mesh::share::subscribe_removals();
                tick.tick().await;
                Vec::new()
            }
            Some(rx) => tokio::select! {
                _ = tick.tick() => Vec::new(),
                r = rx.recv() => match r {
                    Ok(id) => vec![id.0],
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Vec::new(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        removals = None;
                        Vec::new()
                    }
                },
            },
        };
        let (Some(net), Some(engine)) = (mesh::net(), mesh::self_id()) else { continue };
        let (Some(vault), Ok(engine)) = (net.vault(), engine.parse::<PublicKey>()) else { continue };
        let keys = VaultMeshKeys(vault);
        let edges = DialedEdges { dialer: dialer.clone(), engine };
        let rk = Rekeyer {
            store: &store,
            stash: &stash,
            keys: &keys,
            edges: &edges,
            settle_ms: limits::REKEY_SETTLE_SECS * 1000,
            heal_ms: limits::REKEY_HEAL_SECS * 1000,
        };
        let now = platform::time::now_ms();
        if let Err(e) = tick_once(&rk, removed_now, &mut was_holder, now).await {
            tracing::warn!("share: removal duties: {e:#}");
        }
    }
}

async fn tick_once(rk: &Rekeyer<'_>, removed_now: Vec<String>, was_holder: &mut bool, now: u64) -> anyhow::Result<()> {
    let holder = mesh::share::is_lease_holder().await;
    let gained = holder && !*was_holder;
    *was_holder = holder;
    if holder {
        if !removed_now.is_empty() {
            rk.on_removal(removed_now, now).await?;
        }
        if gained {
            // MESH1 doubt: tombstones that arrived while this engine was not running or not
            // the holder.
            if let Some(m) = mesh::actor()
                && let Ok(all) = m.ask(mesh::RemovedPeers).await
            {
                let all: Vec<String> = all.into_iter().map(|e| e.0).collect();
                let todo = crate::rekey::unhandled_removals(&all, &rk.store.rekeys().await?);
                if !todo.is_empty() {
                    rk.on_removal(todo, now).await?;
                }
            }
        }
        rk.run(now).await?;
    } else {
        rk.heal(now).await?;
    }
    rk.prune(now).await
}
