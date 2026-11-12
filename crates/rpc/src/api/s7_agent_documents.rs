use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{owner, Server};
use agent::rpc_document::doc_forward;
use document::rpc_support::{doc_send, documents};

#[rpc(server, client)]
pub trait AgentDocumentsApi {
    /// A document's members and share links (59 §8; owner only).
    #[method(name = "doc_members", with_extensions)]
    async fn doc_members(&self, doc: document::DocId) -> RpcResult<document::Members>;
    /// Rename or re-role a member (59 §8.1; owner only). Demoting an editor closes their
    /// subscriptions with `Closed{demoted}`; their editor re-opens read-only.
    #[method(name = "doc_member_set", with_extensions)]
    async fn doc_member_set(
        &self,
        doc: document::DocId,
        member: String,
        role: Option<document::Role>,
        name: Option<String>,
    ) -> RpcResult<()>;
    /// Stop new joins through `link` (59 §8.4; owner only); its members stay until revoked.
    #[method(name = "doc_link_revoke", with_extensions)]
    async fn doc_link_revoke(&self, doc: document::DocId, link: String) -> RpcResult<()>;
    /// Import a snapshot's markdown as one diff transaction (59 §4.6: history is kept, not rewound).
    #[method(name = "doc_restore", with_extensions)]
    async fn doc_restore(&self, doc: document::DocId, snapshot: String) -> RpcResult<()>;
    /// Revert the last agent edit of `doc` as the owner (59 §5.5 page menu); one level, in memory.
    #[method(name = "doc_undo_agent", with_extensions)]
    async fn doc_undo_agent(&self, doc: document::DocId) -> RpcResult<()>;
    /// A new note (59 §5.3, §7.3): `kind: note`, derived `documents/<id>/note.md`, headed `# <title>` unless `markdown` has one.
    #[method(name = "doc_create", with_extensions)]
    async fn doc_create(
        &self,
        title: String,
        markdown: Option<String>,
    ) -> RpcResult<document::DocSummary>;
    /// Retitle a note: meta, index and its first block when that is a level-1 heading. Other kinds: `title_owned_by_binding`.
    #[method(name = "doc_rename", with_extensions)]
    async fn doc_rename(&self, doc: document::DocId, title: String) -> RpcResult<()>;
    /// Move a note to `documents/.trash/<id>` (swept after 30 days); subscribers get `Closed{deleted}`. Other kinds: `delete_via_owner`.
    #[method(name = "doc_delete", with_extensions)]
    async fn doc_delete(&self, doc: document::DocId) -> RpcResult<()>;
}

#[async_trait]
impl AgentDocumentsApiServer for Server {

    async fn doc_members(
        &self,
        ext: &Extensions,
        doc: document::DocId,
    ) -> RpcResult<document::Members> {
        owner(ext)?;
        let r = doc_send(&documents()?, document::MembersOf { doc: doc.clone() }).await;
        doc_forward(ext, "doc_members", (&doc,), r).await
    }

    async fn doc_member_set(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        member: String,
        role: Option<document::Role>,
        name: Option<String>,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::MemberSet {
                doc: doc.clone(),
                pub_: member.clone(),
                role,
                name: name.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_member_set", (&doc, &member, role, &name), r).await
    }

    async fn doc_link_revoke(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        link: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::LinkRevoke {
                doc: doc.clone(),
                link: link.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_link_revoke", (&doc, &link), r).await
    }

    async fn doc_restore(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        snapshot: String,
    ) -> RpcResult<()> {
        let r = doc_send(
            &documents()?,
            document::Restore {
                doc: doc.clone(),
                snapshot: snapshot.clone(),
            },
        )
        .await
        .map(|_| ());
        doc_forward(ext, "doc_restore", (&doc, &snapshot), r).await
    }

    async fn doc_undo_agent(&self, ext: &Extensions, doc: document::DocId) -> RpcResult<()> {
        let r = doc_send(
            &documents()?,
            document::UndoLastAgentEdit { doc: doc.clone() },
        )
        .await
        .map(|_| ());
        doc_forward(ext, "doc_undo_agent", (&doc,), r).await
    }

    async fn doc_create(
        &self,
        ext: &Extensions,
        title: String,
        markdown: Option<String>,
    ) -> RpcResult<document::DocSummary> {
        if title.trim().is_empty() {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "title must not be empty",
                None::<()>,
            ));
        }
        let r = doc_send(
            &documents()?,
            document::CreateNote {
                title: title.trim().to_owned(),
                markdown: markdown.clone().unwrap_or_default(),
                by: document::Principal::Owner { device: None },
                home: document::Homing::Here,
            },
        )
        .await;
        doc_forward(ext, "doc_create", (&title, &markdown), r).await
    }

    async fn doc_rename(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        title: String,
    ) -> RpcResult<()> {
        let r = doc_send(
            &documents()?,
            document::Rename {
                doc: doc.clone(),
                title: title.clone(),
            },
        )
        .await;
        doc_forward(ext, "doc_rename", (&doc, &title), r).await
    }

    async fn doc_delete(&self, ext: &Extensions, doc: document::DocId) -> RpcResult<()> {
        let r = doc_send(&documents()?, document::DeleteNote { doc: doc.clone() }).await;
        doc_forward(ext, "doc_delete", (&doc,), r).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    AgentDocumentsApiServer::into_rpc(server)
}
