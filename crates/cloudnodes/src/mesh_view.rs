//! What the actor needs from the mesh, behind one trait so the create job and the list merge are
//! testable without an iroh endpoint: mint a remote ticket, read the peer set, unpair, and call
//! an RPC method on a peer (`theuth/rpc/1`, `agent::rpc_forward::to_peer`).

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::providers::{CloudError, Result};
use crate::types::{DesktopOpen, MeshPath};

/// One paired engine, as `mesh_status` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshPeer {
    /// Endpoint id.
    pub id: String,
    /// Display name (a cloud node's is its node name).
    pub name: String,
    /// A connection is up.
    pub reachable: bool,
    /// How it is reached.
    pub path: MeshPath,
}

/// The mesh as the service uses it.
#[async_trait]
pub trait MeshView: Send + Sync {
    /// A key-less single-use pairing ticket that lives `ttl_ms` (`mesh::RemoteTicket`).
    async fn remote_ticket(&self, ttl_ms: u64) -> Result<String>;
    /// Every paired engine but this one.
    async fn peers(&self) -> Result<Vec<MeshPeer>>;
    /// Unpair `id` (`mesh_remove(id)`, phone pairing untouched).
    async fn remove(&self, id: &str) -> Result<()>;
    /// `method(params)` on peer `id`, waiting at most `wait`. `Ok(None)`: not connected.
    async fn call(
        &self,
        id: &str,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Result<Option<Value>>;
    /// A loopback noVNC tunnel on this engine to peer `id`'s desktop (`engine_desktop`).
    async fn desktop(&self, id: &str) -> Result<DesktopOpen>;
}

/// The running engine's mesh actor and RPC forwarder.
pub struct LiveMesh;

fn actor() -> Result<kameo::actor::ActorRef<mesh::Mesh>> {
    kameo::actor::ActorRef::<mesh::Mesh>::lookup(mesh::MESH_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| CloudError("the mesh is not running on this engine".into()))
}

fn path_of(p: mesh::PeerPath, reachable: bool) -> MeshPath {
    match p {
        mesh::PeerPath::Lan => MeshPath::Direct,
        mesh::PeerPath::Relay => MeshPath::Relay,
        mesh::PeerPath::Offline if reachable => MeshPath::Direct,
        mesh::PeerPath::Offline => MeshPath::Offline,
    }
}

#[async_trait]
impl MeshView for LiveMesh {
    async fn remote_ticket(&self, ttl_ms: u64) -> Result<String> {
        actor()?
            .ask(mesh::RemoteTicket { ttl_ms })
            .await
            .map_err(|e| CloudError(format!("mesh ticket: {e}")))
    }

    async fn peers(&self) -> Result<Vec<MeshPeer>> {
        let status = actor()?
            .ask(mesh::Status)
            .await
            .map_err(|e| CloudError(format!("mesh status: {e}")))?;
        Ok(status
            .peers
            .into_iter()
            .map(|p| MeshPeer {
                path: path_of(p.path, p.reachable),
                id: p.id.0,
                name: p.name,
                reachable: p.reachable,
            })
            .collect())
    }

    async fn remove(&self, id: &str) -> Result<()> {
        actor()?
            .ask(mesh::Unpair {
                id: mesh::EngineId(id.to_owned()),
                reset_devices: false,
            })
            .await
            .map_err(|e| CloudError(format!("mesh remove: {e}")))
    }

    async fn call(
        &self,
        id: &str,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Result<Option<Value>> {
        let ext = jsonrpsee::Extensions::new();
        agent::rpc_forward::to_peer_within(
            &ext,
            &replica::PeerId(id.to_owned()),
            method,
            params,
            wait,
        )
        .await
        .map_err(|e| CloudError(format!("{method} on the node: {}", e.message())))
    }

    async fn desktop(&self, id: &str) -> Result<DesktopOpen> {
        let d = agent::rpc_engine::desktop_access(id)
            .await
            .map_err(CloudError)?;
        Ok(DesktopOpen {
            ws_url: d.ws_url,
            password: d.password,
            geometry: d.geometry,
            engine: d.engine,
        })
    }
}
