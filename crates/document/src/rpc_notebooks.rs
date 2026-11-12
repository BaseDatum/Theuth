use crate as document;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::Server;
use document::rpc_support::{doc_send, documents, member_outcome, nb_result, principal};

/// Notebooks RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "document", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait DocumentNotebooksApi {
    /// What `notebook_delete` would do (144 §3.4): note count, exclusive sources, sources also in other notebooks.
    #[method(name = "notebook_delete_preview")]
    async fn notebook_delete_preview(
        &self,
        id: document::DocId,
    ) -> RpcResult<document::DeletePreview>;
    /// Live items of a notebook, oldest first. Any engine; membership-checked for guests.
    #[method(name = "notebook_items_list", with_extensions)]
    async fn notebook_items_list(
        &self,
        notebook: document::DocId,
    ) -> RpcResult<Vec<document::NotebookItem>>;
    /// One source's card. Any engine; membership-checked for guests.
    #[method(name = "source_get", with_extensions)]
    async fn source_get(&self, id: document::DocId) -> RpcResult<document::Source>;
    /// Sources of `notebook`, or every source (unfiled included). Any engine.
    #[method(name = "source_list")]
    async fn source_list(
        &self,
        notebook: Option<document::DocId>,
    ) -> RpcResult<Vec<document::Source>>;
    /// Hybrid block search over notes, sources, insights and notebook descriptions (144 §7.4). Any engine.
    #[method(name = "nb_search")]
    async fn nb_search(
        &self,
        args: document::index::NbSearchArgs,
    ) -> RpcResult<Vec<document::index::BlockHit>>;
}

#[async_trait]
impl DocumentNotebooksApiServer for Server {

    async fn notebook_delete_preview(
        &self,
        id: document::DocId,
    ) -> RpcResult<document::DeletePreview> {
        doc_send(&documents()?, document::NotebookDeletePreview { id }).await
    }

    async fn notebook_items_list(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
    ) -> RpcResult<Vec<document::NotebookItem>> {
        let (peer, by) = principal(ext)?;
        let r = doc_send(&documents()?, document::NotebookItemsList { notebook, by }).await;
        member_outcome(&peer, &r);
        r
    }

    async fn source_get(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::Source> {
        let (peer, by) = principal(ext)?;
        // 144 §11: readable by the owner, a member of the source, or a member of a notebook holding it.
        let docs = documents()?;
        let may = doc_send(
            &docs,
            document::MayRead {
                doc: id.clone(),
                by,
            },
        )
        .await;
        member_outcome(&peer, &may);
        may?;
        let r = nb_result(document::ingest::source_get(&docs, id).await);
        member_outcome(&peer, &r);
        r
    }

    async fn source_list(
        &self,
        notebook: Option<document::DocId>,
    ) -> RpcResult<Vec<document::Source>> {
        nb_result(document::ingest::source_list(&documents()?, notebook).await)
    }

    async fn nb_search(
        &self,
        args: document::index::NbSearchArgs,
    ) -> RpcResult<Vec<document::index::BlockHit>> {
        nb_result(document::index::nb_search(&documents()?, args).await)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(DocumentNotebooksApiServer::into_rpc(server), rpc_descriptors())
        .expect("document RPC descriptors")
}
