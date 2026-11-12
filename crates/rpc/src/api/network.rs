use agent::rpc_forward as forward;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use link::rpc_support::{link, mint_as_holder};
use link::{CancelPair, Enable, LinkStatus, Pair, Pairing, Rename, RenameDevice, Revoke};
use mesh::rpc_support::mesh_actor;
use nodes::rpc_support::nodes;
use rpc_core::{Server, failed, send, UNAUTHORIZED};
use secrets::rpc_support::secrets;

#[rpc(server, client)]
pub trait NetworkApi {
    /// 184 §7.2: the mesh link id (`mesh/device_key`'s public id), minted only on the lease holder
    /// (`not the hub` elsewhere). Mesh-internal — accepted only over `theuth/rpc/1` (`Forwarded`).
    #[method(name = "link_mesh_id", with_extensions)]
    async fn link_mesh_id(&self) -> RpcResult<String>;
    /// Link state: relay, uplink, paired devices, pending pairing (31 §6.4).
    #[method(name = "link_status")]
    async fn link_status(&self) -> RpcResult<LinkStatus>;
    /// Turn the relay uplink on/off (persisted as `link.enabled`); `relay: Some` also rewrites `link.relay`.
    #[method(name = "link_enable")]
    async fn link_enable(&self, enabled: bool, relay: Option<String>) -> RpcResult<()>;
    /// Start (or restart) the single pending pairing: the QR string and its expiry.
    #[method(name = "link_pair")]
    async fn link_pair(&self) -> RpcResult<Pairing>;
    /// Drop the pending pairing.
    #[method(name = "link_cancel_pair")]
    async fn link_cancel_pair(&self) -> RpcResult<()>;
    /// Set the harness display name (persisted as `link.name`; empty → hostname).
    #[method(name = "link_rename")]
    async fn link_rename(&self, name: String) -> RpcResult<()>;
    /// Rename a paired device.
    #[method(name = "link_rename_device")]
    async fn link_rename_device(&self, id: String, name: String) -> RpcResult<()>;
    /// Revoke a paired device: close its conns, delete its row.
    #[method(name = "link_revoke_device")]
    async fn link_revoke_device(&self, id: String) -> RpcResult<()>;
    /// Every row of the replicated device registry, revoked included (69 §3).
    #[method(name = "devices_list")]
    async fn devices_list(&self) -> RpcResult<Vec<link::DeviceInfo>>;
    /// Revoke a device (69 §3): `revoked_ms` set, conns closed; replicates to every hub.
    #[method(name = "device_revoke")]
    async fn device_revoke(&self, id: String) -> RpcResult<()>;
    /// Rename a device in the replicated registry (69 §3).
    #[method(name = "device_rename")]
    async fn device_rename(&self, id: String, name: String) -> RpcResult<()>;
    /// Start a pairing against the MESH identity (69 decision 4): the QR
    /// string (4 parts, pending id last), its expiry and the pending id. Any
    /// engine may call it; the hub accepts the device. `name` is what the QR
    /// shows; `None` → the mesh's name.
    #[method(name = "device_pair_start")]
    async fn device_pair_start(&self, name: Option<String>) -> RpcResult<link::PairingPayload>;
    /// Paired nodes with liveness and child counts (32 §6.6).
    #[method(name = "list_nodes")]
    async fn list_nodes(&self) -> RpcResult<Vec<nodes::Node>>;
    /// Pair with a node from its pasted key (31 §4.4 string); `name` overrides
    /// the node's own display name locally (32 §6.6).
    #[method(name = "add_node")]
    async fn add_node(&self, key: String, name: Option<String>) -> RpcResult<nodes::Node>;
    /// Close the node's conns, delete its row and best-effort revoke this
    /// harness on the node (32 §6.6).
    #[method(name = "remove_node")]
    async fn remove_node(&self, id: String) -> RpcResult<()>;
    /// Set a node's local alias (32 §6.6).
    #[method(name = "rename_node")]
    async fn rename_node(&self, id: String, name: String) -> RpcResult<()>;
    /// `link_pair` on the node over the control conn: a QR string a phone
    /// scans to pair with the node directly (32 §6.6, §8.2).
    #[method(name = "invite_node")]
    async fn invite_node(&self, id: String) -> RpcResult<Pairing>;
    /// This engine, its peers and the hub lease (67 §4, §12).
    #[method(name = "mesh_status")]
    async fn mesh_status(&self) -> RpcResult<mesh::MeshStatus>;
    /// A pairing string another engine can `t mesh add` (67 §4.1).
    #[method(name = "mesh_pairing_string")]
    async fn mesh_pairing_string(&self) -> RpcResult<String>;
    /// Pair with the engine behind `pairing`; mutual and transitive (67 §4.1).
    #[method(name = "mesh_add")]
    async fn mesh_add(&self, pairing: String) -> RpcResult<mesh::Peer>;
    /// This engine's hub lease preference (67 §4.5); written to `mesh.hub`.
    #[method(name = "mesh_set_hub")]
    async fn mesh_set_hub(&self, pref: mesh::HubPref) -> RpcResult<()>;
    /// 183 §6.8: engines the Nearby browse sees, not paired; empty while no window is open.
    #[method(name = "mesh_nearby")]
    async fn mesh_nearby(&self) -> RpcResult<Vec<mesh::NearbyEngine>>;
    /// 183 §6.8: open (`true`, Settings › Network shown) or close the Nearby window. `true` is a
    /// 30 s lease: the open page re-sends it every 10 s and on reconnect.
    #[method(name = "mesh_nearby_watch")]
    async fn mesh_nearby_watch(&self, open: bool) -> RpcResult<()>;
    /// 183 §6.8: start a code-checked pairing with Nearby engine `id`.
    #[method(name = "mesh_pair_nearby")]
    async fn mesh_pair_nearby(&self, id: String) -> RpcResult<mesh::PairCeremony>;
    /// 183 §6.8: this side's Pair (`accept`) or Cancel for a ceremony.
    #[method(name = "mesh_pair_answer")]
    async fn mesh_pair_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()>;
    /// 183 §6.2 (W-14): agents on this engine and `id` may message each other (`on`), or
    /// not; written to `mesh.messaging_off`.
    #[method(name = "mesh_set_messaging")]
    async fn mesh_set_messaging(&self, id: String, on: bool) -> RpcResult<()>;
    /// 183 R1 (S1): keep (`keep`: pair it again) or drop a machine an unpaired machine
    /// introduced (`MeshStatus.quarantined`).
    #[method(name = "mesh_confirm_peer")]
    async fn mesh_confirm_peer(&self, id: String, keep: bool) -> RpcResult<()>;
    /// 183 §6.7: "Let nearby machines find this one while pairing"; written to `mesh.nearby`.
    #[method(name = "mesh_set_nearby")]
    async fn mesh_set_nearby(&self, on: bool) -> RpcResult<()>;
    /// 183 §6.7: rename a machine: `mesh.name` for this engine, `peers.name` otherwise.
    #[method(name = "mesh_rename")]
    async fn mesh_rename(&self, id: String, name: String) -> RpcResult<()>;
    /// 183 §6.9: the newest vault-key rotation; `null` before the first.
    #[method(name = "vault_rotation")]
    async fn vault_rotation(&self) -> RpcResult<Option<secrets::RotationState>>;
    /// 183 §6.9.12: rotate the vault key without an unpair, or retry a failed rotation.
    #[method(name = "vault_rotate")]
    async fn vault_rotate(&self) -> RpcResult<secrets::RotationState>;
}

#[async_trait]
impl NetworkApiServer for Server {

    async fn link_mesh_id(&self, ext: &Extensions) -> RpcResult<String> {
        if ext.get::<forward::Forwarded>().is_none() {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "link_mesh_id is mesh-internal",
                None::<()>,
            ));
        }
        // W-v: only the lease holder mints `mesh/device_key`; `not the hub` elsewhere.
        let vault = secrets::handle(&self.rt)
            .await
            .ok_or_else(|| failed("secrets vault unavailable"))?;
        mint_as_holder(&vault).await.map_err(failed)
    }

    async fn link_status(&self) -> RpcResult<LinkStatus> {
        send(&link()?, link::Status).await
    }

    async fn link_enable(&self, enabled: bool, relay: Option<String>) -> RpcResult<()> {
        send(&link()?, Enable { enabled, relay }).await
    }

    async fn link_pair(&self) -> RpcResult<Pairing> {
        send(&link()?, Pair).await
    }

    async fn link_cancel_pair(&self) -> RpcResult<()> {
        send(&link()?, CancelPair).await
    }

    async fn link_rename(&self, name: String) -> RpcResult<()> {
        send(&link()?, Rename { name }).await
    }

    async fn link_rename_device(&self, id: String, name: String) -> RpcResult<()> {
        send(&link()?, RenameDevice { id, name }).await
    }

    async fn link_revoke_device(&self, id: String) -> RpcResult<()> {
        send(&link()?, Revoke { id }).await
    }

    async fn devices_list(&self) -> RpcResult<Vec<link::DeviceInfo>> {
        send(&link()?, link::DevicesList).await
    }

    async fn device_revoke(&self, id: String) -> RpcResult<()> {
        send(&link()?, Revoke { id }).await
    }

    async fn device_rename(&self, id: String, name: String) -> RpcResult<()> {
        send(&link()?, RenameDevice { id, name }).await
    }

    async fn device_pair_start(&self, name: Option<String>) -> RpcResult<link::PairingPayload> {
        send(&link()?, link::DevicePairStart { name }).await
    }

    async fn list_nodes(&self) -> RpcResult<Vec<nodes::Node>> {
        send(&nodes()?, nodes::Status).await
    }

    async fn add_node(&self, key: String, name: Option<String>) -> RpcResult<nodes::Node> {
        send(&nodes()?, nodes::Add { key, name }).await
    }

    async fn remove_node(&self, id: String) -> RpcResult<()> {
        send(&nodes()?, nodes::Remove { id }).await
    }

    async fn rename_node(&self, id: String, name: String) -> RpcResult<()> {
        send(&nodes()?, nodes::Rename { id, name }).await
    }

    async fn invite_node(&self, id: String) -> RpcResult<Pairing> {
        send(&nodes()?, nodes::Invite { id }).await
    }

    // 67: every method is one `ask`; the actors answer `not built` until
    // their workers fill them (docs/design/67-contract.md).

    async fn mesh_status(&self) -> RpcResult<mesh::MeshStatus> {
        send(&mesh_actor()?, mesh::Status).await
    }

    async fn mesh_pairing_string(&self) -> RpcResult<String> {
        send(&mesh_actor()?, mesh::PairingString).await
    }

    async fn mesh_add(&self, pairing: String) -> RpcResult<mesh::Peer> {
        send(&mesh_actor()?, mesh::AddPeer(pairing)).await
    }

    async fn mesh_set_hub(&self, pref: mesh::HubPref) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::SetHubPref(pref)).await
    }

    // 183 Phase 2 (§10.1): one `ask` each; the mesh answers "not built yet" where
    // the Nearby and rotation workers have not landed.

    async fn mesh_nearby(&self) -> RpcResult<Vec<mesh::NearbyEngine>> {
        send(&mesh_actor()?, mesh::Nearby).await
    }

    async fn mesh_nearby_watch(&self, open: bool) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::NearbyWatch(open)).await
    }

    async fn mesh_pair_nearby(&self, id: String) -> RpcResult<mesh::PairCeremony> {
        send(&mesh_actor()?, mesh::PairNearby(mesh::EngineId(id))).await
    }

    async fn mesh_pair_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::PairAnswer { ceremony, accept }).await
    }

    async fn mesh_confirm_peer(&self, id: String, keep: bool) -> RpcResult<()> {
        send(
            &mesh_actor()?,
            mesh::ConfirmPeer {
                id: mesh::EngineId(id),
                keep,
            },
        )
        .await
    }

    async fn mesh_set_messaging(&self, id: String, on: bool) -> RpcResult<()> {
        send(
            &mesh_actor()?,
            mesh::SetMessaging {
                id: mesh::EngineId(id),
                on,
            },
        )
        .await
    }

    async fn mesh_set_nearby(&self, on: bool) -> RpcResult<()> {
        send(&mesh_actor()?, mesh::SetNearby(on)).await
    }

    async fn mesh_rename(&self, id: String, name: String) -> RpcResult<()> {
        send(
            &mesh_actor()?,
            mesh::Rename {
                id: mesh::EngineId(id),
                name,
            },
        )
        .await
    }

    async fn vault_rotation(&self) -> RpcResult<Option<secrets::RotationState>> {
        send(&secrets()?, secrets::GetRotation).await
    }

    async fn vault_rotate(&self) -> RpcResult<secrets::RotationState> {
        send(&mesh_actor()?, mesh::RotateVault).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    NetworkApiServer::into_rpc(server)
}
