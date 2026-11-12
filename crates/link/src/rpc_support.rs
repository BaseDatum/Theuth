use crate as link;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use kameo::actor::ActorRef;
use rpc_core::{lookup, NOT_RUNNING};
use crate::{LinkActor, LINK_ACTOR};
use mesh::rpc_support::mesh_actor;
use rpc_core::send;

/// 184 §7.2 W-v (Z184-R2): only the synced lease holder mints `mesh/device_key`. `is_hub()` alone
/// is also true on both sides of a dual-hub window, and two mints would leave the loser's mesh
/// links dead. The id is read back after the mint, so a row that replicated in meanwhile wins.
pub async fn mint_as_holder(vault: &secrets::Vault) -> Result<String, String> {
    let holder = mesh::is_hub()
        && mesh::synced()
        && match mesh_actor() {
            Ok(m) => send(&m, mesh::Status)
                .await
                .is_ok_and(|s| s.lease.holder.as_ref() == Some(&s.self_id)),
            Err(_) => false,
        };
    link::mint_mesh_link_id(vault, holder).await?;
    link::mesh_link_id(vault)?.ok_or_else(|| "the mesh has no link id yet".to_owned())
}

/// The link actor; absent when the plugin is not in this process's list.
pub fn link() -> RpcResult<ActorRef<LinkActor>> {
    lookup::<LinkActor>(LINK_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "link is disabled", None::<()>),
        _ => e,
    })
}
