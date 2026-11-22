use crate as agent;
use agent::rpc_document::doc_forward;
use document::rpc_support::{doc_send, documents};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{Server, owner};

/// Documents RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
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
    /// `project` files it under that project (the sessions' label); omitted or `""` = ungrouped.
    #[method(name = "doc_create", with_extensions)]
    async fn doc_create(
        &self,
        title: String,
        markdown: Option<String>,
        project: Option<String>,
    ) -> RpcResult<document::DocSummary>;
    /// File a note or board under a project (the sessions' and notebooks' label, one level); `None` or `""`
    /// clears it. Written on this engine; the record replica carries it to the rest, like `notebooks.project`.
    /// Other kinds: `project_owned_by_binding`.
    #[method(name = "doc_set_project")]
    async fn doc_set_project(
        &self,
        doc: document::DocId,
        project: Option<String>,
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
        project: Option<String>,
    ) -> RpcResult<document::DocSummary> {
        if title.trim().is_empty() {
            return Err(invalid("title must not be empty"));
        }
        let label = project_label(project.as_deref())?;
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
        // The create may run elsewhere (184 §5.2) but answers here; the project is the hub's write.
        let created = doc_forward(ext, "doc_create", (&title, &markdown), r).await?;
        match label {
            None => Ok(created),
            Some(p) => set_project(created.id, Some(p)).await,
        }
    }

    async fn doc_set_project(
        &self,
        doc: document::DocId,
        project: Option<String>,
    ) -> RpcResult<document::DocSummary> {
        let label = project_label(project.as_deref())?;
        set_project(doc, label).await
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

fn invalid(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        jsonrpsee::types::error::INVALID_PARAMS_CODE,
        msg.into(),
        None::<()>,
    )
}

/// A document's project label as sessions normalize theirs; `None` and `""` (blank) are ungrouped.
fn project_label(project: Option<&str>) -> RpcResult<Option<String>> {
    match project {
        Some(p) if !p.trim().is_empty() => {
            agent::project::normalize_project(Some(p)).map_err(invalid)
        }
        _ => Ok(None),
    }
}

/// `docs.project` written on this engine; the replica carries it to the rest.
async fn set_project(
    doc: document::DocId,
    project: Option<String>,
) -> RpcResult<document::DocSummary> {
    doc_send(&documents()?, document::SetProject { doc, project }).await
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentDocumentsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::{Binding, DocId, DocKind, DocMeta};
    use kameo::actor::Spawn as _;

    fn meta(binding: Binding, title: &str) -> DocMeta {
        let id = DocId::new();
        DocMeta {
            derived: binding.derived(&id),
            id,
            kind: binding.kind(),
            title: title.into(),
            binding,
            fragment: "blocks".into(),
            members: vec![],
            links: vec![],
            agent: Default::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: 1,
            updated_ms: 1,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        }
    }

    async fn call(
        methods: &jsonrpsee::Methods,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let req =
            serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });
        let (resp, _) = methods.raw_json_request(&req.to_string(), 1).await.unwrap();
        serde_json::from_str(resp.get()).unwrap()
    }

    /// `doc_set_project` over the wire: the label is normalized, lands in `doc_list`'s summaries,
    /// `""` clears it, a bound kind refuses and a control character is invalid params.
    #[tokio::test]
    async fn doc_set_project_round_trips_through_doc_list() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let store = document::store::Store::open(dir.path()).await.unwrap();
        let note = meta(Binding::Note, "Plan");
        let meeting = meta(Binding::MeetingNote("m1".into()), "Weekly");
        for m in [&note, &meeting] {
            store.create(m).await.unwrap();
        }
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = document::helper::Helper::new(tx, rt.shutdown.clone());
        let actor = document::DocumentActor::spawn(document::DocumentActor::new(
            store,
            helper,
            document::DocumentsConfig::default(),
            rt.clone(),
        ));
        actor.register(document::DOCUMENT_ACTOR).unwrap();
        let methods: jsonrpsee::Methods = rpc_module(Server::new(rt)).module.into();
        let listed = || async {
            actor
                .ask(document::ListDocs {
                    kind: Some(DocKind::Note),
                    limit: None,
                })
                .await
                .unwrap()
        };

        let v = call(
            &methods,
            "doc_set_project",
            serde_json::json!([note.id, "  alpha "]),
        )
        .await;
        assert_eq!(v["result"]["project"], "alpha", "{v}");
        assert_eq!(v["result"]["id"], note.id.as_str());
        assert_eq!(listed().await[0].project.as_deref(), Some("alpha"));

        let v = call(
            &methods,
            "doc_set_project",
            serde_json::json!([note.id, ""]),
        )
        .await;
        assert!(v["result"]["project"].is_null(), "{v}");
        assert_eq!(listed().await[0].project, None);

        let v = call(
            &methods,
            "doc_set_project",
            serde_json::json!([meeting.id, "alpha"]),
        )
        .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("project_owned_by_binding"),
            "{v}"
        );

        let v = call(
            &methods,
            "doc_set_project",
            serde_json::json!([note.id, "a\u{7}b"]),
        )
        .await;
        assert_eq!(
            v["error"]["code"],
            jsonrpsee::types::error::INVALID_PARAMS_CODE,
            "{v}"
        );
        assert_eq!(listed().await[0].project, None);
    }
}
