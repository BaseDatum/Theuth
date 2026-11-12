//! Link identifiers, origins and mesh-aware document sharing for RPC calls.

use crate as link;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use link::LinkStatus;
use runtime::Runtime;
use agent::rpc_forward as forward;
use rpc_core::{send, failed};
use link::rpc_support::{link, mint_as_holder};
use mesh::rpc_support::mesh_actor;
use document::rpc_support::{documents, doc_send};
use agent::rpc_document::{doc_forward, to_doc_server};

/// `doc_link_create` with Link off on a lone engine (a mesh makes a mesh link instead).
pub const LINK_OFF: &str = "link is off: turn on Settings › Link to share documents";

/// 184 §7.1, §7.2: a share link's ids, the fragment before `.<doc>.<secret>`, and how the guest
/// gets in. `mesh`: the mesh link id when known. Link off here (only in a mesh, Sky Q3) → the v1
/// mesh link `1.<mesh link id>`. Link on, `home_here` (this engine is the document's home in a
/// mesh with peers, W-j) → `2.<home link id>.<mesh link id>`. Otherwise today's v1: the mesh
/// device id Link serves, else this engine's link id.
pub fn link_ids(
    status: &LinkStatus,
    mesh: Option<&str>,
    home_here: bool,
) -> RpcResult<(String, document::LinkVia)> {
    use document::LinkVia::{Home, Mesh};
    if !status.enabled {
        return mesh
            .map(|m| (format!("1.{m}"), Mesh))
            .ok_or_else(|| failed(link::MESH_LINK_UNAVAILABLE));
    }
    if home_here && let (Some(m), Some(h)) = (mesh, status.harness_id.as_deref()) {
        return Ok((format!("2.{h}.{m}"), Home));
    }
    match (status.mesh.as_ref(), status.harness_id.as_deref()) {
        (Some(m), _) => Ok((format!("1.{}", m.device_id), Mesh)),
        (None, Some(h)) => Ok((format!("1.{h}"), Home)),
        (None, None) => Err(failed("link has no identity yet")),
    }
}

/// The guest page's origin for relay `relay` (`wss://h` → `https://h`).
pub fn link_origin(relay: &str) -> String {
    let relay = relay.trim_end_matches('/');
    relay
        .strip_prefix("wss://")
        .map(|h| format!("https://{h}"))
        .or_else(|| relay.strip_prefix("ws://").map(|h| format!("http://{h}")))
        .unwrap_or_else(|| relay.to_owned())
}

/// 184 §7.2 (W-v, Sky Q3): the mesh link id for a mesh link from a Link-off home: Link's pub-only
/// read; with no `mesh/device_key` yet, the lease holder mints it (this engine when it holds the
/// lease, else its `link_mesh_id`); [`link::MESH_LINK_UNAVAILABLE`] when no holder answers.
pub async fn mesh_link_id(rt: &Runtime) -> RpcResult<String> {
    if let Ok(id) = send(&link()?, link::MeshLinkId).await {
        return Ok(id);
    }
    if mesh::is_hub() {
        let vault = secrets::handle(rt)
            .await
            .ok_or_else(|| failed("secrets vault unavailable"))?;
        // Not yet synced or not the holder after all: Retry (the dialog's mesh_link_unavailable).
        return mint_as_holder(&vault)
            .await
            .map_err(|_| failed(link::MESH_LINK_UNAVAILABLE));
    }
    let unavailable = || failed(link::MESH_LINK_UNAVAILABLE);
    let holder = send(&mesh_actor()?, mesh::Status)
        .await
        .ok()
        .and_then(|s| s.lease.holder)
        .ok_or_else(unavailable)?;
    // A fresh internal query, not a second hop of the user's (possibly forwarded) call: the
    // holder answers it itself and never forwards it.
    match forward::to_peer::<String>(
        &Extensions::new(),
        &replica::PeerId(holder.0),
        "link_mesh_id",
        [(); 0],
    )
    .await
    {
        Ok(Some(id)) => Ok(id),
        _ => Err(unavailable()),
    }
}

/// 184 §7.2: `doc_link_create` in a mesh with peers. Off the document's server the call is
/// forwarded there; on it: Link on → v2 on the home, today's v1 elsewhere; Link off → a v1 mesh
/// link (Sky Q3), whose id this engine reads and never mints.
pub async fn doc_link_create_in_mesh(
    rt: &Runtime,
    ext: &Extensions,
    doc: document::DocId,
    role: document::Role,
    name: Option<String>,
    expires_ms: Option<i64>,
) -> RpcResult<document::LinkCreated> {
    let docs = documents()?;
    let params = (&doc, role, &name, expires_ms);
    if let Some(v) = to_doc_server(ext, &docs, &doc, "doc_link_create", params).await? {
        return Ok(v);
    }
    let status: LinkStatus = send(&link()?, link::Status).await?;
    let origin = link_origin(&status.relay);
    // Every id is known before the link is written: no link without a URL.
    let (ids, via) = match status.enabled {
        true => link_ids(&status, None, false)?,
        false => link_ids(&status, Some(&mesh_link_id(rt).await?), false)?,
    };
    let create = document::LinkCreate {
        doc: doc.clone(),
        role,
        name: name.clone(),
        expires_ms,
    };
    let r = match doc_send(&docs, create).await {
        Ok((share, secret)) => {
            // v2 only from the home itself (W-j), and only with Link on here.
            let home_here = status.enabled
                && doc_send(
                    &docs,
                    document::MetaOf {
                        doc: doc.clone(),
                        by: document::Principal::Owner { device: None },
                    },
                )
                .await
                .ok()
                .and_then(|m| m.home)
                .is_some_and(|h| mesh::self_id().is_some_and(|me| me == h.0));
            let mesh_id = match (home_here, status.mesh.as_ref()) {
                (false, _) => None,
                (true, Some(m)) => Some(m.device_id.clone()),
                (true, None) => send(&link()?, link::MeshLinkId).await.ok(),
            };
            let (ids, via) = match mesh_id {
                Some(m) => link_ids(&status, Some(&m), true)?,
                None => (ids, via),
            };
            Ok(document::LinkCreated {
                url: format!("{origin}/d#{ids}.{doc}.{secret}"),
                link: share,
                via,
            })
        }
        Err(e) => Err(e),
    };
    doc_forward(ext, "doc_link_create", params, r).await
}
