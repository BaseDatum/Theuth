//! Owner: E2 — `cell_latest` (design 171 §13.2): a note's replicated latest runs, readable by
//! the owner, members and guests (on `GUEST_METHODS`); membership checked per call like
//! `notebook_get`. Skeleton: no membership check, the actor's neutral (empty) answer.

use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;

use crate::api::Server;
use crate::cells::{Latest, cells_err};

/// `cell_latest`.
pub(crate) async fn latest(
    server: &Server,
    ext: &Extensions,
    doc: document::DocId,
) -> RpcResult<Latest> {
    let _ = (server, ext);
    match ::cells::Cells::lookup().await {
        Some(c) => c.latest(doc).await.map_err(cells_err),
        None => Ok(Vec::new()),
    }
}
