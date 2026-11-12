use crate as document;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{failed, peer, UNAUTHORIZED, Server};
use document::rpc_support::{doc_send, documents, join_outcome, member_outcome, principal};
use base64::Engine as _;

/// Documents RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait DocumentDocumentsApi {
    /// Live documents, newest first (design doc 59 §6.1); `limit` defaults to 100.
    #[method(name = "doc_list")]
    async fn doc_list(
        &self,
        kind: Option<document::DocKind>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<document::DocSummary>>;
    /// Lossy markdown of the whole document (what its export writes) — Hire copies it into the charter body.
    #[method(name = "doc_markdown")]
    async fn doc_markdown(&self, doc: document::DocId) -> RpcResult<String>;
    /// Open `doc`: its metadata, the Yjs state missing from `sv` (base64 v1
    /// state vector; absent = everything) as ≤ 48 KiB base64 chunks, and the
    /// current awareness states.
    #[method(name = "doc_open", with_extensions)]
    async fn doc_open(
        &self,
        doc: document::DocId,
        sv: Option<String>,
    ) -> RpcResult<document::DocOpen>;
    /// One base64 v1 update from this connection; refused with `UpdateTooLarge` over 48 KiB and `DocTooLarge` past the document cap.
    #[method(name = "doc_update", with_extensions)]
    async fn doc_update(&self, doc: document::DocId, update: String) -> RpcResult<()>;
    /// One base64 awareness update (y-protocols) from this connection; relayed, never stored.
    #[method(name = "doc_awareness", with_extensions)]
    async fn doc_awareness(&self, doc: document::DocId, state: String) -> RpcResult<()>;
    /// A guest connection (31 §4.3 mode 2) proves a link secret for this connection and becomes a
    /// member (59 §8.3): `proof` = base64url `HMAC-SHA256(sha256(secret), handshake_hash ‖ guest_pub)`;
    /// the link is the one the proof verifies against (the URL carries no link id).
    /// Errors: `link_unknown`, `link_revoked`, `link_expired`, `bad_proof`, `members_full`, `sharing_off`.
    #[method(name = "doc_join", with_extensions)]
    async fn doc_join(
        &self,
        doc: document::DocId,
        proof: String,
        name: Option<String>,
    ) -> RpcResult<document::Joined>;
    /// A member renames themself (59 §8.5; editors only — `viewer_readonly` otherwise).
    #[method(name = "doc_set_name", with_extensions)]
    async fn doc_set_name(&self, doc: document::DocId, name: String) -> RpcResult<()>;
    /// Kept snapshots of `doc`, newest first (59 §4.6).
    #[method(name = "doc_snapshots")]
    async fn doc_snapshots(&self, doc: document::DocId) -> RpcResult<Vec<document::SnapshotInfo>>;
    /// Whether the documents helper (`theuth-documents-<version>` beside `t`, 59 §3.2) is there and how its
    /// process is doing (`process`: `running` | `crashed{stderr}` | `missing` | `starting`); a missing or
    /// crashed helper disables notes, knowledge editing and meeting notes. Answers whether documents run or not.
    #[method(name = "documents_status")]
    async fn documents_status(&self) -> RpcResult<document::HelperStatus>;
    /// How this engine handled peers' document snapshots since start (69 E2): `merged`, `unchanged`,
    /// `resnapshots`, `noop_applies_avoided`. Served on any engine; never persisted.
    #[method(name = "doc_stats")]
    async fn doc_stats(&self) -> RpcResult<document::DocStats>;
    /// Every `DocEvent` of `doc` not caused by this connection: other
    /// clients' updates and awareness, exports, snapshots, close.
    #[subscription(name = "doc_subscribe", unsubscribe = "doc_unsubscribe", item = document::DocEvent, with_extensions)]
    async fn doc_subscribe(&self, doc: document::DocId) -> SubscriptionResult;
}

#[async_trait]
impl DocumentDocumentsApiServer for Server {

    async fn doc_list(
        &self,
        kind: Option<document::DocKind>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<document::DocSummary>> {
        doc_send(&documents()?, document::ListDocs { kind, limit }).await
    }

    async fn doc_markdown(&self, doc: document::DocId) -> RpcResult<String> {
        doc_send(
            &documents()?,
            document::Markdown {
                doc,
                blocks: None,
                annotate: false,
            },
        )
        .await
    }

    async fn doc_open(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        sv: Option<String>,
    ) -> RpcResult<document::DocOpen> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::OpenDoc { doc, sv, by }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_update(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        update: String,
    ) -> RpcResult<()> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::ApplyUpdate { doc, update, by }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_awareness(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        state: String,
    ) -> RpcResult<()> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(
            &documents()?,
            document::AwarenessUpdateMsg { doc, state, by },
        )
        .await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_join(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        proof: String,
        name: Option<String>,
    ) -> RpcResult<document::Joined> {
        let peer = peer(ext)?;
        let Some(guest) = peer.guest() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "guests only",
                None::<()>,
            ));
        };
        let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(proof)
            .map_err(|_| failed("bad_proof"))?;
        let r = doc_send(
            &documents()?,
            document::Join {
                doc,
                guest_pub: guest.key,
                handshake_hash: guest.handshake_hash.to_vec(),
                proof,
                name,
            },
        )
        .await;
        join_outcome(guest, &r);
        r
    }

    async fn doc_set_name(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        name: String,
    ) -> RpcResult<()> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::SetName { doc, by, name }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn doc_snapshots(&self, doc: document::DocId) -> RpcResult<Vec<document::SnapshotInfo>> {
        doc_send(&documents()?, document::Snapshots { doc }).await
    }

    async fn documents_status(&self) -> RpcResult<document::HelperStatus> {
        Ok(document::helper_status())
    }

    async fn doc_stats(&self) -> RpcResult<document::DocStats> {
        doc_send(&documents()?, document::Stats).await
    }

    async fn doc_subscribe(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        doc: document::DocId,
    ) -> SubscriptionResult {
        // Admission and the actor's subscription before accept, so the client's first
        // `doc_open` after this cannot race an update it would then miss (59 §6.2).
        let admit = async {
            let (peer, by) = principal(ext)?;
            let r = doc_send(&documents()?, document::Subscribe { doc, by }).await;
            member_outcome(&peer, &r);
            r
        };
        let mut rx = match admit.await {
            Ok(rx) => rx,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        tokio::spawn(async move {
            loop {
                let ev = tokio::select! {
                    _ = sink.closed() => return,
                    ev = rx.recv() => match ev { Some(ev) => ev, None => return },
                };
                let Ok(raw) = serde_json::value::to_raw_value(&ev) else {
                    continue;
                };
                let closed = matches!(ev, document::DocEvent::Closed { .. });
                if sink.send(SubscriptionMessage::from(raw)).await.is_err() || closed {
                    return;
                }
            }
        });
        Ok(())
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    DocumentDocumentsApiServer::into_rpc(server)
}
