//! Whiteboard RPC methods (218 contract §4.1): create, blobs, turns and *Undo this turn*.
//! Listing, opening, syncing, renaming and trashing use the document RPCs.

use crate as document;
use document::rpc_support::{doc_send, documents, principal};
use document::wb::{WbBlob, WbBlobData, WbTurnInfo, WbUndoResult, WhiteboardCreated};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::Server;

/// Whiteboard RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "document", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait DocumentWhiteboardsApi {
    /// Create an empty whiteboard (no helper import); `title` defaults to `Whiteboard`.
    #[method(name = "whiteboard_create", with_extensions)]
    async fn whiteboard_create(&self, title: Option<String>) -> RpcResult<WhiteboardCreated>;
    /// Store one image blob of a whiteboard (`snap/<card>/<ver>`, `mark/<mark>`, `img/<ulid>`):
    /// `mime` ∈ `image/png`, `image/jpeg`, `image/webp`, `data` base64, ≤ 3 MiB decoded; overwrites.
    #[method(name = "wb_blob_put", with_extensions)]
    async fn wb_blob_put(
        &self,
        doc: document::DocId,
        key: String,
        mime: String,
        data: String,
    ) -> RpcResult<WbBlob>;
    /// One stored blob of a whiteboard, or `null` when absent.
    #[method(name = "wb_blob_get", with_extensions)]
    async fn wb_blob_get(&self, doc: document::DocId, key: String) -> RpcResult<Option<WbBlobData>>;
    /// The newest turn of `session` on this whiteboard (what it created or changed), or `null`.
    #[method(name = "whiteboard_turn", with_extensions)]
    async fn whiteboard_turn(
        &self,
        doc: document::DocId,
        session: String,
    ) -> RpcResult<Option<WbTurnInfo>>;
    /// Undo one turn of `session` on this whiteboard (default: the newest not yet undone),
    /// skipping and reporting every item the owner changed since.
    #[method(name = "whiteboard_undo_turn", with_extensions)]
    async fn whiteboard_undo_turn(
        &self,
        doc: document::DocId,
        session: String,
        turn: Option<u32>,
    ) -> RpcResult<WbUndoResult>;
}

#[async_trait]
impl DocumentWhiteboardsApiServer for Server {
    async fn whiteboard_create(
        &self,
        ext: &Extensions,
        title: Option<String>,
    ) -> RpcResult<WhiteboardCreated> {
        let (_, by) = principal(ext)?;
        let m = document::wb::CreateWhiteboard {
            title,
            by,
            home: document::Homing::Here,
        };
        doc_send(&documents()?, m).await
    }

    async fn wb_blob_put(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        key: String,
        mime: String,
        data: String,
    ) -> RpcResult<WbBlob> {
        let (_, by) = principal(ext)?;
        doc_send(
            &documents()?,
            document::wb::BlobPut {
                doc,
                key,
                mime,
                data,
                by,
            },
        )
        .await
    }

    async fn wb_blob_get(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        key: String,
    ) -> RpcResult<Option<WbBlobData>> {
        let (_, by) = principal(ext)?;
        doc_send(&documents()?, document::wb::BlobGet { doc, key, by }).await
    }

    async fn whiteboard_turn(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        session: String,
    ) -> RpcResult<Option<WbTurnInfo>> {
        let (_, by) = principal(ext)?;
        doc_send(&documents()?, document::wb::TurnOf { doc, session, by }).await
    }

    async fn whiteboard_undo_turn(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        session: String,
        turn: Option<u32>,
    ) -> RpcResult<WbUndoResult> {
        let (_, by) = principal(ext)?;
        doc_send(
            &documents()?,
            document::wb::UndoTurn {
                doc,
                session,
                turn,
                by,
            },
        )
        .await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(DocumentWhiteboardsApiServer::into_rpc(server), rpc_descriptors())
        .expect("document whiteboard RPC descriptors")
}
