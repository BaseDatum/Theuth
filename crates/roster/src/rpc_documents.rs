use crate as roster;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::Server;
use agent::rpc_document::doc_forward;
use document::rpc_support::{doc_send, documents};

/// Documents RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "roster", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait RosterDocumentsApi {
    /// The document bound to an agent-charter `binding`, created from `markdown` when there is none
    /// (the Hire wizard's draft, before `agents/<name>.md` exists). Other bindings: `owner_creates`.
    #[method(name = "doc_ensure", with_extensions)]
    async fn doc_ensure(
        &self,
        binding: document::Binding,
        markdown: String,
    ) -> RpcResult<document::DocId>;
}

#[async_trait]
impl RosterDocumentsApiServer for Server {

    async fn doc_ensure(
        &self,
        ext: &Extensions,
        binding: document::Binding,
        markdown: String,
    ) -> RpcResult<document::DocId> {
        if !matches!(&binding, document::Binding::AgentCharter(name) if name != roster::AgentName::DEFAULT)
        {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "owner_creates",
                None::<()>,
            ));
        }
        let r = doc_send(
            &documents()?,
            document::Ensure {
                binding: binding.clone(),
                markdown: markdown.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_ensure", (&binding, &markdown), r).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(RosterDocumentsApiServer::into_rpc(server), rpc_descriptors())
        .expect("roster RPC descriptors")
}
