//! Link plugin: pair phones/browsers to this harness through a stateless
//! relay and serve them the RPC API over Noise (design doc 31).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`LINK_ACTOR`] actor ([`LinkActor`]): [`Status`],
//!   [`Enable`], [`Pair`], [`CancelPair`], [`Rename`], [`RenameDevice`],
//!   [`Revoke`]
//! - events: [`LinkEvent`]
//! - [`rpc_core::RpcPeer`]: the authenticated connection behind an RPC call (mode-0
//!   conns here, local sockets in `rpc`), read from jsonrpsee extensions
//! - [`qr`] renders a pairing string for terminals
//!
//! Modules: [`identity`] (static key, prologue), [`store`] (`link.sqlite`:
//! the replicated `devices`/`pending` tables, 69), [`devices`] (their
//! in-memory mirrors), [`pairing`] (the §4.4 string), [`uplink`] (relay
//! socket, `NK`, demux), [`mesh_uplink`] (the mesh identity's second uplink,
//! hub duty, 69 §2), [`follow`] (the lease and the tables it follows),
//! [`conn`] (per-conn `IK`/`IKpsk2` handshake and records, dispatched by
//! mode — 32 §4.2), [`initiator`] (this harness as a device of another, 32
//! §4.1), [`state`] (what those share).
//!
//! What a conn dispatches into arrives by message: `rpc` `tell`s
//! [`SetDispatch`] once at start (32 §13).

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Hub RPC methods and typed clients.
pub mod rpc_hub;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_hub::rpc_module(server),
    ]).expect("link RPC composition")
}

pub mod rpc_sharing;
/// Provider-owned helpers used by RPC method owners.
pub mod rpc_support;
pub mod conn;
pub mod devices;
pub mod follow;
pub mod identity;
pub mod initiator;
pub mod mesh_uplink;
pub mod pairing;
pub mod state;
pub mod store;
pub mod types;
pub mod uplink;

pub use conn::RecordChannel;
pub use types::*;

use devices::Devices;
use identity::Identity;
use pairing::Pending;
use state::{Ctx, State};

/// The plugin.
pub struct LinkPlugin;

/// Declare wire contracts before starting producers, including disabled services.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<LinkEvent>().await
}

/// Render `text` as a unicode half-block QR code without quiet zone.
pub fn qr(text: &str) -> String {
    match qrcode::QrCode::new(text) {
        Ok(code) => code
            .render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(false)
            .build(),
        Err(e) => format!("(qr unavailable: {e})"),
    }
}

/// The [`LINK_ACTOR`] actor: config, identity, devices, the pending pairing,
/// the uplink task's lifetime and — as the hub — the mesh uplink's (69 §2).
#[derive(kameo::Actor)]
pub struct LinkActor {
    ctx: Arc<Ctx>,
    config_path: PathBuf,
    uplink: Option<CancellationToken>,
    pair_timer: Option<JoinHandle<()>>,
    /// Where this engine stands with the hub lease (`follow`).
    follower: mesh::lease::Follower,
    /// The mesh identity's uplink while this engine is the hub.
    mesh: Option<mesh_uplink::MeshUplink>,
    /// A `mesh/device_key` load is in flight.
    mesh_loading: bool,
    /// The mesh answered `Status` (or is absent): the reconcile may start the mesh uplink. Until
    /// then `follower.holding` is only its default and must not register anything.
    mesh_ready: bool,
}

/// Terminal-mode answer when the uplink is off.
const DISABLED: &str = "link is disabled";

/// YAML value for a string key: double-quoted (a JSON string is one).
fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).expect("a str serialises to JSON")
}

impl LinkActor {
    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        runtime::write_config_value(&self.config_path, "link", key, value)
            .map_err(|e| format!("write {}: {e}", self.config_path.display()))
    }

    /// Load or create the identity into the state; no-op when present.
    fn ensure_identity(&self) -> Result<(), String> {
        let mut st = self.ctx.lock();
        if st.identity.is_none() {
            let id = Identity::load_or_create(&self.ctx.rt.data_dir.join("link"))
                .map_err(|e| format!("link identity: {e:#}"))?;
            st.identity = Some(id);
        }
        Ok(())
    }

    fn start_uplink(&mut self) {
        self.stop_uplink();
        let token = CancellationToken::new();
        tokio::spawn(uplink::supervise(self.ctx.clone(), token.clone()));
        self.uplink = Some(token);
    }

    fn stop_uplink(&mut self) {
        if let Some(t) = self.uplink.take() {
            t.cancel();
        }
    }

    /// Drop the pending pairing; `true` when there was one.
    fn clear_pairing(&mut self) -> bool {
        if let Some(t) = self.pair_timer.take() {
            t.abort();
        }
        self.ctx.lock().pairing.take().is_some()
    }

    async fn publish_pairing_ended(&self) {
        self.ctx
            .rt
            .publish(LinkEvent::LinkPairing {
                key: None,
                expires_at_ms: None,
            })
            .await;
    }

    /// The mesh identity (69 decision 1) for a pairing: the vault's
    /// `mesh/device_key`, minted here when no engine of the mesh has one yet;
    /// cached in the state afterwards.
    async fn mesh_identity(&self) -> Result<Identity, String> {
        load_mesh_identity(&self.ctx).await
    }
}

/// [`LinkActor::mesh_identity`]: the row, minted if absent.
pub(crate) async fn load_mesh_identity(ctx: &Ctx) -> Result<Identity, String> {
    if let Some(id) = ctx.lock().mesh_identity.clone() {
        return Ok(id);
    }
    let key = secrets::mesh_device_key(&ctx.rt)
        .await
        .map_err(|e| format!("mesh device key: {e:#}"))?;
    Ok(cache_mesh_identity(ctx, Identity::from_private(*key)))
}

/// The mesh identity when the mesh has one — never minted (`follow`: a hub *serves* the mesh
/// identity; only a pairing creates it, so two engines that ran alone before pairing do not each
/// mint a key and collide, which would strand every device on the loser at the first failover).
pub(crate) async fn existing_mesh_identity(ctx: &Ctx) -> Result<Option<Identity>, String> {
    if let Some(id) = ctx.lock().mesh_identity.clone() {
        return Ok(Some(id));
    }
    let key = secrets::mesh_device_key_existing(&ctx.rt)
        .await
        .map_err(|e| format!("mesh device key: {e:#}"))?;
    Ok(key.map(|k| cache_mesh_identity(ctx, Identity::from_private(*k))))
}

fn cache_mesh_identity(ctx: &Ctx, id: Identity) -> Identity {
    let mut st = ctx.lock();
    let id = st.mesh_identity.get_or_insert(id).clone();
    st.mesh.get_or_insert(MeshLink {
        device_id: id.id.clone(),
        connected: false,
    });
    id
}

/// 184 §7.2 (W-v): the mesh link id, read only: the public id of `vault`'s `mesh/device_key` when
/// the mesh has one, `None` otherwise. Never mints; the private key is dropped on return and
/// nothing is cached (so `LinkStatus.mesh` stays unset on a Link-off engine).
pub fn mesh_link_id(vault: &secrets::Vault) -> Result<Option<String>, String> {
    let key = vault
        .mesh_device_key_if_set()
        .map_err(|e| format!("mesh device key: {e:#}"))?;
    Ok(key.map(|k| identity::b64(&identity::public_of(&k))))
}

/// 184 §7.2 (W-v): `link_mesh_id` on the lease holder, the only engine that may mint
/// `mesh/device_key` (a second minter's row would lose the LWW and kill its links): the mesh link
/// id, minted when the mesh has none yet. The holder's follow loop then loads the row and
/// registers the mesh uplink (`SecretsEvent::Changed`). `holder: false` refuses before the vault
/// is touched.
pub async fn mint_mesh_link_id(vault: &secrets::Vault, holder: bool) -> Result<String, String> {
    if !holder {
        return Err("not the hub".into());
    }
    let key = vault
        .mesh_device_key()
        .await
        .map_err(|e| format!("mesh device key: {e:#}"))?;
    Ok(identity::b64(&identity::public_of(&key)))
}

impl Message<Status> for LinkActor {
    type Reply = LinkStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.lock().status()
    }
}

impl Message<MeshLinkId> for LinkActor {
    type Reply = Result<String, String>;

    async fn handle(&mut self, _: MeshLinkId, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(id) = self.ctx.lock().mesh_identity.as_ref() {
            return Ok(id.id.clone());
        }
        let vault = secrets::handle(&self.ctx.rt)
            .await
            .ok_or_else(|| "secrets vault unavailable".to_owned())?;
        mesh_link_id(&vault)?.ok_or_else(|| "the mesh has no link id yet".to_owned())
    }
}

impl Message<SetDispatch> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: SetDispatch, _: &mut Context<Self, Self::Reply>) {
        self.ctx.set_dispatch(m);
    }
}

impl Message<Enable> for LinkActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Enable, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let relay_changed = m.relay.is_some();
        if let Some(relay) = &m.relay {
            if !(relay.starts_with("wss://") || relay.starts_with("ws://")) {
                return Err("relay must start with ws:// or wss://".into());
            }
            self.write("relay", &yaml_str(relay))?;
        }
        self.write("enabled", if m.enabled { "true" } else { "false" })?;
        {
            let mut st = self.ctx.lock();
            if let Some(relay) = m.relay {
                st.cfg.relay = relay;
            }
            st.cfg.enabled = m.enabled;
        }
        if m.enabled {
            self.ensure_identity()?;
            if self.uplink.is_none() || relay_changed {
                self.start_uplink();
            }
        } else {
            self.stop_uplink();
            if self.clear_pairing() {
                self.publish_pairing_ended().await;
            }
        }
        // The mesh uplink follows `enabled` and the relay too (69 §8: `link.relay` applies to both).
        if relay_changed && let Some(up) = self.mesh.take() {
            up.stop();
        }
        self.sync_mesh(ctx.actor_ref().clone()).await;
        Ok(())
    }
}

impl Message<Pair> for LinkActor {
    type Reply = Result<Pairing, String>;

    async fn handle(&mut self, _: Pair, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let pending = {
            let mut st = self.ctx.lock();
            let Some(identity) = st.identity.as_ref().filter(|_| st.cfg.enabled) else {
                return Err(DISABLED.into());
            };
            if st.devices.len() >= st.cfg.max_devices {
                return Err(format!(
                    "max_devices ({}) reached; revoke one first",
                    st.cfg.max_devices
                ));
            }
            let p = Pending::new(&st.cfg.relay, &identity.public, st.name());
            st.pairing = Some(p.clone());
            p
        };
        if let Some(t) = self.pair_timer.take() {
            t.abort();
        }
        let me = ctx.actor_ref().clone();
        let secret = pending.secret;
        let ttl = std::time::Duration::from_millis(pairing::TTL_MS);
        self.pair_timer = Some(tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            let _ = me.tell(Expire(secret)).await;
        }));
        self.ctx
            .rt
            .publish(LinkEvent::LinkPairing {
                key: Some(pending.key.clone()),
                expires_at_ms: Some(pending.expires_at_ms),
            })
            .await;
        Ok(pending.pairing())
    }
}

/// Internal: the pairing with this secret timed out.
struct Expire([u8; 32]);

impl Message<Expire> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        let ended = {
            let mut st = self.ctx.lock();
            let same = st.pairing.as_ref().is_some_and(|p| p.secret == m.0);
            same && st.pairing.take().is_some()
        };
        if ended {
            self.pair_timer = None;
            self.publish_pairing_ended().await;
        }
    }
}

impl Message<CancelPair> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, _: CancelPair, _: &mut Context<Self, Self::Reply>) {
        if self.clear_pairing() {
            self.publish_pairing_ended().await;
        }
    }
}

impl Message<Rename> for LinkActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Rename, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.write("name", &yaml_str(&m.name))?;
        self.ctx.lock().cfg.name = m.name;
        Ok(())
    }
}

impl Message<RenameDevice> for LinkActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: RenameDevice, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if !self.ctx.lock().devices.rename(&m.id, &m.name) {
            return Err(format!("unknown device {}", m.id));
        }
        self.ctx.rt.publish(LinkEvent::DevicesChanged).await;
        Ok(())
    }
}

impl Message<Revoke> for LinkActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: Revoke, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        {
            let mut st = self.ctx.lock();
            let key = st
                .devices
                .by_id(&m.id)
                .map(|r| r.key)
                .ok_or_else(|| format!("unknown device {}", m.id))?;
            // Each conn task (either uplink) sends CLOSE 0 and removes itself.
            for c in st.all_conns().filter(|c| c.device == Some(key)) {
                c.cancel.cancel();
            }
            st.devices.remove(&m.id);
        }
        self.ctx
            .rt
            .publish(LinkEvent::LinkRevoked { id: m.id })
            .await;
        self.ctx.rt.publish(LinkEvent::DevicesChanged).await;
        Ok(())
    }
}
impl LinkActor {
    /// D11: revoke every active device (rows stay, conns close); the count revoked. Used by
    /// [`ResetDevices`] and by an engine that sees `mesh/device_key` change (183 R1).
    pub(crate) async fn revoke_all_devices(&mut self) -> u32 {
        let ids: Vec<String> = {
            let mut st = self.ctx.lock();
            let rows: Vec<(String, [u8; 32])> = st
                .devices
                .rows()
                .filter(|r| r.active())
                .map(|r| (r.id.clone(), r.key))
                .collect();
            for (id, key) in &rows {
                for c in st.all_conns().filter(|c| c.device == Some(*key)) {
                    c.cancel.cancel();
                }
                st.devices.remove(id);
            }
            rows.into_iter().map(|(id, _)| id).collect()
        };
        for id in &ids {
            self.ctx
                .rt
                .publish(LinkEvent::LinkRevoked { id: id.clone() })
                .await;
        }
        self.ctx.rt.publish(LinkEvent::DevicesChanged).await;
        u32::try_from(ids.len()).unwrap_or(u32::MAX)
    }
}

impl Message<ResetDevices> for LinkActor {
    type Reply = Result<u32, String>;

    async fn handle(
        &mut self,
        _: ResetDevices,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let n = self.revoke_all_devices().await;
        self.drop_mesh_identity(ctx.actor_ref().clone()).await;
        Ok(n)
    }
}

impl Message<DevicesList> for LinkActor {
    type Reply = Vec<DeviceInfo>;

    async fn handle(&mut self, _: DevicesList, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx
            .lock()
            .devices
            .all()
            .iter()
            .map(devices::Row::info)
            .collect()
    }
}

impl Message<DevicePairStart> for LinkActor {
    type Reply = Result<PairingPayload, String>;

    async fn handle(
        &mut self,
        m: DevicePairStart,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let identity = self.mesh_identity().await?;
        // A pairing may have just minted the mesh identity: a hub registers it now, not at the
        // next vault change.
        self.sync_mesh(ctx.actor_ref().clone()).await;
        // The name the QR shows: the caller's, else the mesh's, else ours.
        let name = match m
            .name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
        {
            Some(n) => n,
            None => match mesh::actor() {
                Some(a) => match a.ask(mesh::Status).await {
                    Ok(s) => s.mesh_name.unwrap_or(s.self_name),
                    _ => self.ctx.lock().name().to_owned(),
                },
                None => self.ctx.lock().name().to_owned(),
            },
        };
        let mut st = self.ctx.lock();
        if st.devices.len() >= st.cfg.max_devices {
            return Err(format!(
                "max_devices ({}) reached; revoke one first",
                st.cfg.max_devices
            ));
        }
        let (pending_id, psk, expires_ms) = st.pending.create(&name)?;
        Ok(PairingPayload {
            string: pairing::build(
                &st.cfg.relay,
                &identity.public,
                &psk,
                &name,
                Some(&pending_id),
            ),
            expires_ms,
            pending_id,
        })
    }
}

impl Message<DropGuest> for LinkActor {
    type Reply = ();

    async fn handle(&mut self, m: DropGuest, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let st = self.ctx.lock();
        // Each conn task (either uplink — a guest may arrive on the mesh identity, 69 §5) sends
        // CLOSE 0 and removes itself.
        for c in st.all_conns().filter(|c| c.device == Some(m.key)) {
            c.cancel.cancel();
        }
    }
}

#[async_trait::async_trait]
impl runtime::Plugin for LinkPlugin {
    fn name(&self) -> &'static str {
        "link"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: LinkConfig = rt.section("link").unwrap_or_else(|e| {
            tracing::debug!("no [link] config section, using defaults: {e}");
            LinkConfig::default()
        });
        let dir = rt.data_dir.join("link");
        let identity = if cfg.enabled {
            match Identity::load_or_create(&dir) {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!("link identity unavailable: {e:#}; link stays off");
                    None
                }
            }
        } else {
            None
        };
        let enabled = cfg.enabled && identity.is_some();
        // 69 decision 3: the registry is `link.sqlite`, replicated once the
        // replica plugin is up (it starts after this one); memory only when
        // the file cannot be opened. The vault seals pending PSKs.
        let store = match store::Store::open(&rt.data_dir.join(store::DB)).await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("link.sqlite unavailable ({e:#}); devices are memory only");
                None
            }
        };
        let devices = match &store {
            Some(s) => Devices::open(s.clone(), &dir).await,
            None => Devices::empty(),
        };
        let vault = secrets::handle(rt).await;
        if let Some(pool) = store.as_ref().map(|s| s.pool().clone()) {
            let rt2 = rt.clone();
            tokio::spawn(async move {
                if rt2
                    .lookup_when::<replica::Replica>(replica::REPLICA_ACTOR)
                    .await
                    .is_none()
                {
                    return;
                }
                match replica::attach(&pool, store::TABLES).await {
                    Ok(_) => tracing::info!("link: devices/pending replicated"),
                    Err(e) => tracing::info!("link: replication unavailable ({e}); local only"),
                }
            });
        }
        let state = State {
            cfg,
            hostname: platform::process::hostname().to_owned(),
            identity,
            devices,
            pairing: None,
            pending: devices::Pending::new(store.clone(), vault),
            mesh_identity: None,
            mesh: None,
            connected: false,
            conns: Default::default(),
            mesh_conns: Default::default(),
            guests: Default::default(),
        };
        let ctx = Ctx::new(rt.clone(), state);
        let mut actor = LinkActor {
            ctx: ctx.clone(),
            config_path: rt.data_dir.join("config.yaml"),
            uplink: None,
            pair_timer: None,
            follower: mesh::lease::Follower::default(),
            mesh: None,
            mesh_loading: false,
            mesh_ready: false,
        };
        if enabled {
            actor.start_uplink();
        } else {
            tracing::info!("link disabled by config");
        }
        let actor: ActorRef<LinkActor> = LinkActor::spawn(actor);
        actor.register(LINK_ACTOR)?;
        // 69 §2: the hub serves the mesh identity; the tables follow the replica.
        follow::start(rt, &actor, ctx, store).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_renders_pairing_string() {
        let key = super::pairing::build("wss://relay.example", &[1; 32], &[2; 32], "Mac", None);
        let q = super::qr(&key);
        assert!(q.lines().count() > 10, "{q}");
    }

    #[test]
    fn yaml_str_round_trips_quote_backslash_newline() {
        let s = "a \"q\" \\ b\nc";
        let v: String = serde_yaml::from_str(&super::yaml_str(s)).unwrap();
        assert_eq!(v, s);
    }
}

#[cfg(test)]
mod mesh_link_tests {
    use super::*;

    /// 184 §7.2 (W-v): the pub-only read never mints; only the holder mints, once; both answer
    /// the public id of the one `mesh/device_key` (the id the mesh uplink registers).
    #[tokio::test]
    async fn the_mesh_link_id_is_read_anywhere_and_minted_on_the_holder_only() {
        let dir = tempfile::tempdir().unwrap();
        let vault = secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        assert_eq!(mesh_link_id(&vault).unwrap(), None);
        assert!(
            vault.mesh_device_key_if_set().unwrap().is_none(),
            "never minted by a read"
        );
        assert_eq!(
            mint_mesh_link_id(&vault, false).await.unwrap_err(),
            "not the hub"
        );
        assert!(
            vault.mesh_device_key_if_set().unwrap().is_none(),
            "a follower never mints"
        );
        let minted = mint_mesh_link_id(&vault, true).await.unwrap();
        let key = vault.mesh_device_key_if_set().unwrap().unwrap();
        assert_eq!(minted, Identity::from_private(*key).id);
        assert_eq!(
            mesh_link_id(&vault).unwrap().as_deref(),
            Some(minted.as_str())
        );
        assert_eq!(
            mint_mesh_link_id(&vault, true).await.unwrap(),
            minted,
            "minted once"
        );
    }
}
