use crate::tools::*;
use crate::{AgentScope, Knowledge, Scoper, Source, Visibility};
use runtime::SessionId;
use std::sync::Arc;
use tools::{Tool, ToolCx};

struct Routes {
    isolated: SessionId,
    missing: SessionId,
}
#[async_trait::async_trait]
impl Scoper for Routes {
    fn scope_for(&self, s: SessionId) -> Option<AgentScope> {
        (s == self.isolated).then(|| AgentScope {
            owner: "agent:ops".into(),
            line: Default::default(),
        })
    }
    async fn bundle_for(&self, s: SessionId) -> Result<Option<String>, String> {
        if s == self.missing {
            return Err("bundle is on another engine".into());
        }
        Ok((s == self.isolated).then(|| "ops".into()))
    }
}
fn context(session: SessionId, root: &std::path::Path) -> ToolCx {
    ToolCx {
        session,
        call_id: "isolation".into(),
        quiet: false,
        resources: Arc::new(tools::SessionResources {
            root: root.into(),
            retention: tools::Retention::Durable,
            generation: 1,
        }),
        cancel: tools::CancellationToken::new(),
        promote_after: None,
    }
}
fn note(id: &str, body: &str) -> crate::Put {
    crate::Put {
        id: id.into(),
        title: body.into(),
        type_: "fact".into(),
        body: body.into(),
        ..Default::default()
    }
}
#[tokio::test]
async fn native_memory_paths_never_cross_bundles_and_parked_rpc_inspection_resumes() {
    let dir = crate::TestDir::new("isolated-memory");
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.0.clone());
    let main = Knowledge::open(&dir.0, rt).await.unwrap();
    let (isolated, missing, plain) = (SessionId::new(), SessionId::new(), SessionId::new());
    main.set_scoper(Arc::new(Routes { isolated, missing }));
    std::fs::create_dir_all(dir.0.join("agents")).unwrap();
    let file = dir.0.join("agents/ops.md");
    std::fs::write(&file, "---\nisolated_memory: true\n---\nOps.").unwrap();
    main.put(note("facts/main", "mainonly sentinel"), Source::User)
        .await
        .unwrap();
    let cx = context(isolated, &dir.0);
    PutTool(main.clone())
        .call(note("facts/secret", "isolatedonly galaxy"), cx.clone())
        .await
        .unwrap();
    let own = main.for_session(isolated).await.unwrap();
    assert_eq!(own.root(), dir.0.join("agent-memory/ops/knowledge"));
    let meta = own.get("facts/secret").await.unwrap().meta;
    assert_eq!(
        (meta.owner.as_str(), meta.visibility),
        ("agent:ops", Visibility::Private)
    );
    for (session, query) in [(isolated, "mainonly"), (plain, "isolatedonly")] {
        let out = SearchTool(main.clone())
            .call(
                SearchArgs {
                    query: query.into(),
                    limit: None,
                },
                context(session, &dir.0),
            )
            .await
            .unwrap();
        assert_eq!(out.content, "no matches");
        let selected = main.for_session(session).await.unwrap();
        assert!(
            selected
                .hint_for(query, &dir.0, &Default::default(), 10, Some(session))
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        GetTool(main.clone())
            .call(
                IdArgs {
                    id: "facts/main".into()
                },
                cx.clone()
            )
            .await
            .is_err()
    );
    assert!(
        GetTool(main.clone())
            .call(
                IdArgs {
                    id: "facts/secret".into()
                },
                context(plain, &dir.0)
            )
            .await
            .is_err()
    );
    let listed = ListTool(main.clone())
        .call(
            ListArgs {
                prefix: None,
                type_: None,
                tag: None,
                limit: None,
            },
            cx.clone(),
        )
        .await
        .unwrap();
    let rows = listed.structured.unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "facts/secret");
    assert_eq!(
        main.list_for(None, None, None, 100, None).await.unwrap()[0].id,
        "facts/main"
    );
    let first = own.history(Some("facts/secret"), 10).await.unwrap().changes[0]
        .commit
        .clone();
    PutTool(main.clone())
        .call(note("facts/secret", "changed galaxy"), cx.clone())
        .await
        .unwrap();
    let past = GetTool(main.clone())
        .call(
            IdArgs {
                id: format!("facts/secret@{first}"),
            },
            cx.clone(),
        )
        .await
        .unwrap();
    assert!(past.content.contains("isolatedonly galaxy"));
    assert_eq!(
        own.revision(&first, None).await.unwrap().files[0].id,
        "facts/secret"
    );
    RestoreTool(main.clone())
        .call(
            RestoreArgs {
                id: "facts/secret".into(),
                commit: Some(first),
            },
            cx.clone(),
        )
        .await
        .unwrap();
    let history = HistoryTool(main.clone())
        .call(
            HistoryArgs {
                id: None,
                limit: None,
            },
            cx.clone(),
        )
        .await
        .unwrap();
    assert!(history.content.contains("facts/secret"));
    assert!(
        !main
            .history(None, 100)
            .await
            .unwrap()
            .changes
            .iter()
            .any(|c| c.message.contains("secret"))
    );
    let got = GetTool(main.clone())
        .call(
            IdArgs {
                id: "facts/secret".into(),
            },
            cx.clone(),
        )
        .await
        .unwrap();
    assert!(got.content.contains("isolatedonly galaxy"));
    let counters = StatsTool(main.clone())
        .call(
            StatsArgs {
                ids: vec!["facts/secret".into()],
            },
            cx.clone(),
        )
        .await
        .unwrap();
    assert_eq!(counters.structured.unwrap()[0]["opened"], 1);
    assert_eq!(
        main.stats(&["facts/secret".into()]).await.unwrap()[0].opened,
        0
    );
    let share = ShareTool(main.clone())
        .call(
            ShareArgs {
                id: "facts/secret".into(),
                visibility: Visibility::Shared,
            },
            cx,
        )
        .await
        .unwrap_err();
    assert_eq!(share.to_string(), "isolated memory cannot be shared");
    assert!(
        main.inspect(None)
            .await
            .unwrap()
            .get("facts/secret")
            .await
            .is_err()
    );
    assert!(
        main.inspect(Some("agent:ops"))
            .await
            .unwrap()
            .get("facts/secret")
            .await
            .is_ok()
    );
    assert_eq!(
        main.inspect(Some("agent:nope"))
            .await
            .err()
            .unwrap()
            .to_string(),
        "unknown bundle: agent:nope"
    );
    std::fs::write(&file, "Ops.").unwrap();
    assert!(
        main.inspect(Some("agent:ops"))
            .await
            .unwrap()
            .get("facts/secret")
            .await
            .is_ok(),
        "parked bundle remains owner-readable"
    );
    main.remove_bundle("ops").await.unwrap();
    assert!(!dir.0.join("agent-memory/ops").exists());
    let removed = std::fs::read_dir(dir.0.join("agent-memory/.removed"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(removed.join("knowledge/facts/secret.md").is_file());
    std::fs::write(file, "---\nisolated_memory: true\n---\nOps.").unwrap();
    assert!(
        main.for_session(isolated)
            .await
            .unwrap()
            .get("facts/secret")
            .await
            .is_err(),
        "a later hire is empty"
    );
    assert!(!dir.0.join("knowledge/facts/secret.md").exists());
}

#[tokio::test]
async fn owner_rpc_bundle_parameter_defaults_to_main_and_rejects_unknown_bundles() {
    use crate::rpc_concepts::KnowledgeConceptsApiServer;
    use kameo::actor::Spawn;
    let dir = crate::TestDir::new("isolated-rpc");
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.0.clone());
    let k = Knowledge::open(&dir.0, rt.clone()).await.unwrap();
    k.put(note("facts/main", "mainonly sentinel"), Source::User)
        .await
        .unwrap();
    std::fs::create_dir_all(dir.0.join("agents")).unwrap();
    std::fs::write(
        dir.0.join("agents/ops.md"),
        "---\nisolated_memory: true\n---\nOps.",
    )
    .unwrap();
    k.inspect(Some("agent:ops"))
        .await
        .unwrap()
        .put(note("facts/secret", "isolatedonly galaxy"), Source::Model)
        .await
        .unwrap();
    let actor = crate::KnowledgeActor::spawn(crate::KnowledgeActor { k });
    actor.register(crate::KNOWLEDGE_ACTOR).unwrap();
    let rpc = rpc_core::Server::new(rt);
    assert!(
        rpc.knowledge_search("isolatedonly".into(), None, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        rpc.knowledge_search("isolatedonly".into(), None, Some("agent:ops".into()))
            .await
            .unwrap()[0]
            .meta
            .id,
        "facts/secret"
    );
    assert!(
        rpc.knowledge_get("facts/secret".into(), None)
            .await
            .is_err()
    );
    assert_eq!(
        rpc.knowledge_get("facts/secret".into(), Some("agent:ops".into()))
            .await
            .unwrap()
            .meta
            .owner,
        "agent:ops"
    );
    assert_eq!(
        rpc.knowledge_list(None, None, None, None, None)
            .await
            .unwrap()[0]
            .id,
        "facts/main"
    );
    assert_eq!(
        rpc.knowledge_list(None, None, None, None, Some("agent:ops".into()))
            .await
            .unwrap()[0]
            .id,
        "facts/secret"
    );
    let e = rpc
        .knowledge_get("facts/x".into(), Some("agent:unknown".into()))
        .await
        .unwrap_err();
    assert_eq!(
        (e.code(), e.message()),
        (-32602, "unknown bundle: agent:unknown")
    );
    actor.stop_gracefully().await.unwrap();
    actor.wait_for_shutdown().await;
}

#[tokio::test]
async fn unresolved_identity_and_unopenable_bundle_fail_closed() {
    let dir = crate::TestDir::new("isolated-unavailable");
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.0.clone());
    let main = Knowledge::open(&dir.0, rt).await.unwrap();
    let (isolated, missing) = (SessionId::new(), SessionId::new());
    main.put(note("facts/main", "mainonly sentinel"), Source::User)
        .await
        .unwrap();
    assert!(
        main.for_session(isolated)
            .await
            .err()
            .unwrap()
            .to_string()
            .starts_with("isolated memory unavailable here:")
    );
    main.set_scoper(Arc::new(Routes { isolated, missing }));
    std::fs::create_dir_all(dir.0.join("agent-memory")).unwrap();
    std::fs::write(dir.0.join("agent-memory/ops"), "not a directory").unwrap();
    for session in [isolated, missing] {
        let e = SearchTool(main.clone())
            .call(
                SearchArgs {
                    query: "mainonly".into(),
                    limit: None,
                },
                context(session, &dir.0),
            )
            .await
            .unwrap_err();
        assert!(
            e.to_string()
                .starts_with("isolated memory unavailable here:"),
            "{e}"
        );
    }
}
