use crate as link;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use link::LinkStatus;
use rpc_core::{send, failed, owner, ASK_FAILED, Server};
use link::rpc_support::link;
use mesh::rpc_support::mesh_actor;
use secrets::rpc_support::secrets;
use link::rpc_sharing::{LINK_OFF, doc_link_create_in_mesh, link_ids, link_origin};
use agent::rpc_document::doc_forward;
use document::rpc_support::{doc_send, documents};
use base64::Engine as _;

/// Hub RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "link", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait LinkHubApi {
    /// Remove a member (59 §8.4; owner only): `Closed{revoked}`, their connections dropped, the next
    /// handshake from that static finds no membership.
    #[method(name = "doc_member_revoke", with_extensions)]
    async fn doc_member_revoke(&self, doc: document::DocId, member: String) -> RpcResult<()>;
    /// A new share link (59 §8.2; owner only): `https://<relay>/d#1.<mesh device id | engine id>.<doc>.<secret>` (69),
    /// returned once — the harness keeps only `sha256(secret)`. Needs Link enabled (the relay host).
    #[method(name = "doc_link_create", with_extensions)]
    async fn doc_link_create(
        &self,
        doc: document::DocId,
        role: document::Role,
        name: Option<String>,
        expires_ms: Option<i64>,
    ) -> RpcResult<document::LinkCreated>;
    /// Leave an engine out of the mesh (67 §4.1); 183 D7: the remaining machines then rotate
    /// the vault key. `reset_devices` (D11, default false): also rotate `mesh/device_key`
    /// after the vault key, so every phone and iPad re-pairs.
    #[method(name = "mesh_remove")]
    async fn mesh_remove(&self, id: String, reset_devices: Option<bool>) -> RpcResult<()>;
    /// 183 §12.9: what unpairing `id` means, for the Unpair dialog.
    #[method(name = "mesh_unpair_preview")]
    async fn mesh_unpair_preview(&self, id: String) -> RpcResult<mesh::UnpairPreview>;
}

#[async_trait]
impl LinkHubApiServer for Server {

    async fn doc_member_revoke(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        member: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::MemberRevoke {
                doc: doc.clone(),
                pub_: member.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_member_revoke", (&doc, &member), r).await?;
        // 59 §8.4: their live conns go too — here as well as on the hub (the guest may hang off either
        // engine's uplink). Link off (no conns) is not an error.
        if let Ok(link) = link()
            && let Ok(key) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&member)
            && let Ok(key) = <[u8; 32]>::try_from(key)
        {
            let _ = link.ask(link::DropGuest { key }).await;
        }
        Ok(())
    }

    async fn doc_link_create(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        role: document::Role,
        name: Option<String>,
        expires_ms: Option<i64>,
    ) -> RpcResult<document::LinkCreated> {
        owner(ext)?;
        // 184 §7.2: in a mesh the document's server makes the link; a lone engine (paired phones
        // or not) runs today's code unchanged.
        if mesh::has_peers() == Some(true) {
            return doc_link_create_in_mesh(&self.rt, ext, doc, role, name, expires_ms).await;
        }
        // The URL needs the relay host and an id devices dial (59 §8.2): Link must be on. 69: the mesh
        // device id when this engine has one — the hub, whichever engine it is, answers `/h/<mesh id>`
        // — else the engine id (a single engine before any device paired). Old engine-id links keep
        // resolving: the engine uplink stays registered.
        let status: LinkStatus = send(&link()?, link::Status).await?;
        if !status.enabled {
            return Err(failed(LINK_OFF));
        }
        let (ids, via) = link_ids(&status, None, false)?;
        let origin = link_origin(&status.relay);
        let r = doc_send(
            &documents()?,
            document::LinkCreate {
                doc: doc.clone(),
                role,
                name: name.clone(),
                expires_ms,
            },
        )
        .await
        .map(|(link, secret)| document::LinkCreated {
            url: format!("{origin}/d#{ids}.{doc}.{secret}"),
            link,
            via,
        });
        doc_forward(ext, "doc_link_create", (&doc, role, &name, expires_ms), r).await
    }

    async fn mesh_remove(&self, id: String, reset_devices: Option<bool>) -> RpcResult<()> {
        let reset_devices = reset_devices == Some(true);
        send(
            &mesh_actor()?,
            mesh::Unpair {
                id: mesh::EngineId(id.clone()),
                reset_devices,
            },
        )
        .await?;
        if !reset_devices {
            return Ok(());
        }
        // D11: the unpair's rotation stores the new vault key with `device_reset_pending`; the
        // device key follows, as a new `mesh/device_key` version sealed under that key, so it
        // replicates to the remaining machines only. A crash before here is retried by the
        // vault's own tick (183 R1).
        let failed = |what: String| {
            ErrorObjectOwned::owned(
                ASK_FAILED,
                format!("mesh_remove: unpaired, but phone and iPad pairing was not reset: {what}"),
                None::<()>,
            )
        };
        let vault = send(&secrets()?, secrets::Handle).await?;
        match vault.rotation() {
            Some(r) if r.phase != secrets::RotationPhase::Failed && r.removed.contains(&id) => {}
            r => {
                return Err(failed(
                    r.and_then(|r| r.error)
                        .unwrap_or_else(|| "the vault key did not rotate".into()),
                ));
            }
        }
        let reset = vault
            .finish_device_reset()
            .await
            .map_err(|e| failed(format!("{e:#}")))?;
        if reset && let Ok(l) = link() {
            send(&l, link::ResetDevices).await?;
        }
        Ok(())
    }

    async fn mesh_unpair_preview(&self, id: String) -> RpcResult<mesh::UnpairPreview> {
        let mut p = send(&mesh_actor()?, mesh::PreviewUnpair(mesh::EngineId(id))).await?;
        // W-37: the paired phones and iPads; none when link is not running here.
        if let Ok(l) = link() {
            let devices = send(&l, link::DevicesList).await?;
            p.devices = u32::try_from(devices.iter().filter(|d| !d.revoked).count()).unwrap_or(0);
        }
        Ok(p)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(LinkHubApiServer::into_rpc(server), rpc_descriptors())
        .expect("link RPC descriptors")
}
