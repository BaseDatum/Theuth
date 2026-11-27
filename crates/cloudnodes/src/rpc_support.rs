//! Service lookup and error mapping for the RPC handlers.

use std::sync::Arc;

use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{ASK_FAILED, lookup, send};

use crate::actor::{CLOUD_ACTOR, CloudActor, GetCloud};
use crate::engine::Cloud;
use crate::providers::CloudError;

/// The service; `not running` before the plugin started.
pub async fn cloud() -> RpcResult<Arc<Cloud>> {
    send(&lookup::<CloudActor>(CLOUD_ACTOR)?, GetCloud).await
}

/// A [`CloudError`] as the JSON-RPC error the UI shows verbatim.
pub fn err(e: CloudError) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ASK_FAILED, e.0, None::<()>)
}
