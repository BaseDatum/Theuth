//! Owner: E2 — `cell_latest` (design 171 §13.2): a note's replicated latest runs, readable by
//! the owner, members and guests (on `GUEST_METHODS`); membership checked per call like
//! `notebook_get` / `source_get`, answered from this engine's replica (any engine, cells plugin
//! or not).

use base64::Engine as _;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;

use crate::api::{Server, ask, peer};
use crate::cells::Latest;

/// `cell_latest`.
pub(crate) async fn latest(
    server: &Server,
    ext: &Extensions,
    doc: document::DocId,
) -> RpcResult<Latest> {
    let _ = server;
    let p = peer(ext)?;
    let device = Some(p.id());
    let by = match p.guest() {
        Some(g) => document::Principal::Member {
            pub_: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(g.key),
            device,
        },
        None => document::Principal::Owner { device },
    };
    let may = ask::<document::DocumentActor, _>(
        document::DOCUMENT_ACTOR,
        document::MayRead {
            doc: doc.clone(),
            by,
        },
    )
    .await;
    // 59 §8.3: success scopes a guest conn, a membership refusal counts toward its close.
    if let Some(g) = p.guest() {
        match &may {
            Ok(()) => g.mark_joined(),
            Err(e)
                if ["not_member", "viewer_readonly", "sharing_disabled"]
                    .iter()
                    .any(|r| e.message().starts_with(r)) =>
            {
                g.refuse();
            }
            Err(_) => {}
        }
    }
    may?;
    ask::<document::DocumentActor, _>(document::DOCUMENT_ACTOR, document::cells::CellLatestOf(doc))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{NOT_RUNNING, UNAUTHORIZED};

    #[tokio::test]
    async fn needs_an_authenticated_peer_and_the_documents_plugin() {
        let rt = runtime::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let server = Server::new(rt);
        let doc = document::DocId("d1".into());
        let e = latest(&server, &Extensions::new(), doc.clone())
            .await
            .unwrap_err();
        assert_eq!(e.code(), UNAUTHORIZED);
        // A guest conn reaches the membership check, which needs the document actor.
        let (guest, _guard) = link::RpcPeer::open_guest([7; 32], [9; 32]);
        let mut ext = Extensions::new();
        ext.insert(guest);
        let e = latest(&server, &ext, doc).await.unwrap_err();
        assert_eq!(e.code(), NOT_RUNNING);
    }
}
