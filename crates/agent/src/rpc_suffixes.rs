//! Prompt suffix RPC methods: the mesh-wide saved list and each chat's "every message" suffix
//! (`store::suffixes`), plus the [`SuffixesChanged`] topic that tells open UIs to reload.

use crate as agent;
use crate::rpc_forward as forward;
use crate::rpc_support::RpcServerExt;
use crate::store::suffixes::{PromptSuffix, SUFFIX_TABLES};
use crate::{Agents, DurableStore};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use platform::time::now_ms_i64 as now_ms;
use rpc_core::{Server, ask, stub_err};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

/// Event: the saved suffixes or a chat's "every message" suffix changed, here or on another
/// machine of the mesh. No body: reload with `list_suffixes` / `session_suffix`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct SuffixesChanged {}

impl wire::WireEvent for SuffixesChanged {
    const TOPIC: &'static str = "agent.suffixes";

    fn policy(json: &serde_json::Value) -> wire::EventPolicy {
        wire::EventPolicy {
            always_wanted: true,
            ..wire::EventPolicy::legacy_fields(json)
        }
    }
}

/// Publish [`SuffixesChanged`] for every peer batch the replica applies to a suffix table
/// (local writes publish from their RPC). Ends with the feed.
pub(crate) fn watch(rt: Runtime, handle: replica::ReplicaHandle) {
    let mut feed = handle.subscribe();
    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match feed.recv().await {
                Ok(batch) => {
                    if &batch.origin != handle.origin()
                        && batch
                            .changes
                            .iter()
                            .any(|c| SUFFIX_TABLES.contains(&c.table.as_str()))
                    {
                        rt.publish(SuffixesChanged {}).await;
                    }
                }
                Err(RecvError::Lagged(_)) => rt.publish(SuffixesChanged {}).await,
                Err(RecvError::Closed) => return,
            }
        }
    });
}

async fn store() -> RpcResult<crate::store::Store> {
    ask::<Agents, _>(agent::AGENT_ACTOR, DurableStore).await
}

/// Prompt suffix RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait SuffixesApi {
    /// Every saved prompt suffix, oldest first; shared by every machine of the mesh.
    #[method(name = "list_suffixes")]
    async fn list_suffixes(&self) -> RpcResult<Vec<PromptSuffix>>;
    /// Create or update a suffix by `id`. `false` when that id was deleted (it never comes back).
    #[method(name = "put_suffix")]
    async fn put_suffix(&self, suffix: PromptSuffix) -> RpcResult<bool>;
    /// Delete a suffix everywhere (and turn it off in every chat). `false` when it was not live.
    #[method(name = "delete_suffix")]
    async fn delete_suffix(&self, id: String) -> RpcResult<bool>;
    /// The suffix `session` appends to every message, if any.
    #[method(name = "session_suffix", with_extensions)]
    async fn session_suffix(&self, session: SessionId) -> RpcResult<Option<String>>;
    /// Set (`suffix`) or turn off (`null`) the suffix `session` appends to every message;
    /// stored by the engine that hosts the chat.
    #[method(name = "set_session_suffix", with_extensions)]
    async fn set_session_suffix(&self, session: SessionId, suffix: Option<String>)
    -> RpcResult<()>;
}

#[async_trait]
impl SuffixesApiServer for Server {
    async fn list_suffixes(&self) -> RpcResult<Vec<PromptSuffix>> {
        store().await?.suffixes().await.map_err(stub_err)
    }

    async fn put_suffix(&self, suffix: PromptSuffix) -> RpcResult<bool> {
        let put = store()
            .await?
            .put_suffix(&suffix, now_ms())
            .await
            .map_err(stub_err)?;
        if put {
            self.rt.publish(SuffixesChanged {}).await;
        }
        Ok(put)
    }

    async fn delete_suffix(&self, id: String) -> RpcResult<bool> {
        let deleted = store()
            .await?
            .delete_suffix(&id, now_ms())
            .await
            .map_err(stub_err)?;
        self.rt.publish(SuffixesChanged {}).await;
        Ok(deleted)
    }

    async fn session_suffix(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Option<String>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "session_suffix", (session,)).await? {
            return Ok(r);
        }
        store()
            .await?
            .session_suffix(session)
            .await
            .map_err(stub_err)
    }

    async fn set_session_suffix(
        &self,
        ext: &Extensions,
        session: SessionId,
        suffix: Option<String>,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "set_session_suffix", (session, &suffix)).await?
        {
            return Ok(r);
        }
        store()
            .await?
            .set_session_suffix(session, suffix.as_deref())
            .await
            .map_err(stub_err)?;
        self.rt.publish(SuffixesChanged {}).await;
        Ok(())
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(SuffixesApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
