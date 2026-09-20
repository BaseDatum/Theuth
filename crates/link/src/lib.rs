//! Link plugin: pair phones/browsers to this harness through a stateless
//! relay and serve them the RPC API over Noise (design doc 31).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`LINK_ACTOR`] actor ([`LinkActor`]): [`Status`],
//!   [`Enable`], [`Pair`], [`CancelPair`], [`Rename`], [`RenameDevice`],
//!   [`Revoke`]
//! - events: [`LinkEvent`]
//! - [`RpcPeer`]: the authenticated connection behind an RPC call (mode-0
//!   conns here, local sockets in `rpc`), read from jsonrpsee extensions
//! - [`qr`] renders a pairing string for terminals
//!
//! Modules: [`identity`] (static key, prologue), [`devices`] (`devices.yaml`),
//! [`pairing`] (the §4.4 string), [`uplink`] (relay socket, `NK`, demux),
//! [`conn`] (per-conn `IK`/`IKpsk2` handshake and records, dispatched by mode
//! — 32 §4.2), [`initiator`] (this harness as a device of another, 32 §4.1),
//! [`state`] (what those share).
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

pub mod conn;
pub mod devices;
pub mod identity;
pub mod initiator;
pub mod pairing;
pub mod state;
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

/// The [`LINK_ACTOR`] actor: config, identity, devices, the pending pairing
/// and the uplink task's lifetime.
#[derive(kameo::Actor)]
pub struct LinkActor {
    ctx: Arc<Ctx>,
    config_path: PathBuf,
    uplink: Option<CancellationToken>,
    pair_timer: Option<JoinHandle<()>>,
}

/// Terminal-mode answer when the uplink is off.
const DISABLED: &str = "link is disabled";

/// YAML value for a string key: double-quoted.
fn yaml_str(s: &str) -> String {
    format!("{s:?}")
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
}

impl Message<Status> for LinkActor {
    type Reply = LinkStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.ctx.lock().status()
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

    async fn handle(&mut self, m: Enable, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
        let mut st = self.ctx.lock();
        let row = st
            .devices
            .by_id_mut(&m.id)
            .ok_or_else(|| format!("unknown device {}", m.id))?;
        row.name = m.name;
        st.devices.save().map_err(|e| format!("devices.yaml: {e}"))
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
            // Each conn task sends CLOSE 0 and removes itself.
            for c in st.conns.values().filter(|c| c.device == Some(key)) {
                c.cancel.cancel();
            }
            st.devices
                .remove(&m.id)
                .map_err(|e| format!("devices.yaml: {e}"))?;
        }
        self.ctx
            .rt
            .publish(LinkEvent::LinkRevoked { id: m.id })
            .await;
        Ok(())
    }
}

/// This machine's hostname; `"theuth"` when `hostname` fails.
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "theuth".to_owned())
}

#[async_trait::async_trait]
impl runtime::Plugin for LinkPlugin {
    fn name(&self) -> &'static str {
        "link"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
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
        let state = State {
            cfg,
            hostname: hostname(),
            identity,
            devices: Devices::load(&dir),
            pairing: None,
            connected: false,
            conns: Default::default(),
        };
        let mut actor = LinkActor {
            ctx: Ctx::new(rt.clone(), state),
            config_path: rt.data_dir.join("config.yaml"),
            uplink: None,
            pair_timer: None,
        };
        if enabled {
            actor.start_uplink();
        } else {
            tracing::info!("link disabled by config");
        }
        let actor: ActorRef<LinkActor> = LinkActor::spawn(actor);
        actor.register(LINK_ACTOR)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_renders_pairing_string() {
        let key = super::pairing::build("wss://relay.example", &[1; 32], &[2; 32], "Mac");
        let q = super::qr(&key);
        assert!(q.lines().count() > 10, "{q}");
    }
}
