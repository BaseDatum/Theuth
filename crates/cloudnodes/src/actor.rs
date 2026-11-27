//! The `cloudnodes` plugin: registers the `cloudnodes.events` topic, opens the [`Cloud`]
//! service (records under `<data_dir>/cloudnodes`, credentials in the vault, the mesh) and
//! registers [`CloudActor`], which hands RPC handlers the shared service. Long jobs run as
//! detached tasks of the service, not in the actor's mailbox, so one slow provider call never
//! blocks another request.

use std::sync::Arc;

use async_trait::async_trait;
use kameo::actor::Spawn as _;
use kameo::message::{Context, Message};
use zeroize::Zeroizing;

use crate::creds::{Creds, SecretStore};
use crate::engine::{Cloud, LiveConnect, Pacing};
use crate::mesh_view::LiveMesh;
use crate::providers::{CloudError, ReqwestTransport, Result};
use crate::records::Records;
use crate::types::CloudEvent;

/// Registry name of [`CloudActor`].
pub const CLOUD_ACTOR: &str = "cloudnodes";

/// Where theuth-relay answers when neither `cloudnodes.relay` nor `link.relay` says otherwise.
pub const DEFAULT_RELAY: &str = "https://relay.theuth.io";

/// Holds the service.
#[derive(kameo::Actor)]
pub struct CloudActor {
    cloud: Arc<Cloud>,
}

/// `ask` → `Arc<Cloud>`.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetCloud;

impl Message<GetCloud> for CloudActor {
    type Reply = Arc<Cloud>;

    async fn handle(&mut self, _: GetCloud, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.cloud.clone()
    }
}

/// theuth-relay's https origin: `cloudnodes.relay`, else `link.relay` (`wss://…` → `https://…`),
/// else [`DEFAULT_RELAY`].
pub fn relay_base(cloud_relay: Option<&str>, link_relay: Option<&str>) -> String {
    let raw = cloud_relay.or(link_relay).unwrap_or(DEFAULT_RELAY).trim();
    let swapped = if let Some(rest) = raw.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = raw.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        raw.to_owned()
    };
    match url::Url::parse(&swapped) {
        Ok(u) if matches!(u.scheme(), "https" | "http") => u.origin().ascii_serialization(),
        _ => DEFAULT_RELAY.to_owned(),
    }
}

/// No vault on this engine: nothing is stored, every write says why.
struct NoVault;

#[async_trait]
impl SecretStore for NoVault {
    fn get_versioned(&self, _: &str) -> Option<(u32, Zeroizing<String>)> {
        None
    }
    async fn set_if_version(&self, _: &str, _: &str, _: Option<u32>) -> Result<bool> {
        Err(CloudError("the vault is unavailable on this engine".into()))
    }
    async fn delete(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

/// Register the `cloudnodes.events` topic.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<CloudEvent>().await
}

/// The plugin.
pub struct CloudPlugin;

#[async_trait]
impl runtime::Plugin for CloudPlugin {
    fn name(&self) -> &'static str {
        "cloudnodes"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let store: Arc<dyn SecretStore> = match secrets::handle(rt).await {
            Some(v) => v,
            None => {
                tracing::warn!("cloudnodes: no vault; provider credentials cannot be stored");
                Arc::new(NoVault)
            }
        };
        let get = |k: &str| rt.config.extract_inner::<String>(k).ok();
        let relay = relay_base(
            get("cloudnodes.relay").as_deref(),
            get("link.relay").as_deref(),
        );
        let http: Arc<dyn crate::providers::Transport> = Arc::new(ReqwestTransport::new()?);
        // Events reach the bus in the order the service emits them.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CloudEvent>();
        let bus = rt.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                bus.publish(ev).await;
            }
        });
        let cloud = Arc::new(Cloud::new(
            Records::new(rt.data_dir.join("cloudnodes")),
            Arc::new(LiveMesh),
            Creds {
                store,
                http: http.clone(),
                relay,
            },
            Arc::new(LiveConnect { http }),
            Arc::new(move |ev| {
                let _ = tx.send(ev);
            }),
            Pacing::default(),
            // iOS: no CLI logins to discover in the app's sandbox (224 §16.12).
            if cfg!(target_os = "ios") {
                None
            } else {
                dirs::home_dir()
            },
        ));
        // iOS has no mesh (no iroh): its nodes form their own mesh and the app pairs over the
        // relay. Set before `resume` so an interrupted phone-only create resumes as one.
        cloud.set_own_mesh(cfg!(target_os = "ios"));
        cloud.resume();
        let actor = CloudActor::spawn(CloudActor { cloud });
        actor.register(CLOUD_ACTOR)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_base_is_an_https_origin() {
        assert_eq!(relay_base(None, None), "https://relay.theuth.io");
        assert_eq!(
            relay_base(None, Some("wss://relay.theuth.io")),
            "https://relay.theuth.io"
        );
        assert_eq!(
            relay_base(
                Some("https://relay.example.com:8443/x/"),
                Some("wss://relay.theuth.io")
            ),
            "https://relay.example.com:8443"
        );
        assert_eq!(
            relay_base(None, Some("ws://127.0.0.1:9000/link")),
            "http://127.0.0.1:9000"
        );
        assert_eq!(relay_base(Some("nonsense"), None), DEFAULT_RELAY);
    }
}
