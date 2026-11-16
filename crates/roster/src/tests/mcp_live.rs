//! The real roster allowlist, live registry and persistent Python checkout.
use super::*;
use ::tools::{InstallMcpAccess, ListTools};

#[test]
fn mcp_live_desk_and_descendant() {
    with_engine_lock(async {
        let Some(worker) =
            std::env::var_os("THEUTH_MONTY_WORKER").or_else(|| std::env::var_os("THEUTH_WORKER"))
        else {
            eprintln!("no THEUTH_MONTY_WORKER/THEUTH_WORKER; skipping MCP Python lifecycle test");
            return;
        };
        let data = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), data.path().to_owned());
        let registry = ::tools::registry::start(&rt).await.unwrap();
        let r = roster_on(rt.clone(), data.path()).await;
        let ops = file::parse(
            "ops",
            "---\ntitle: Ops\nmcp: [other]\n---\nKeep prod healthy.\n",
        )
        .unwrap();
        let qa = file::parse("qa", "---\nmcp: [other]\n---\nTest it.\n").unwrap();
        r.write_file(&ops).await.unwrap();
        r.write_file(&qa).await.unwrap();
        let shared = r.shared.clone();
        let desk = runtime::SessionId::new();
        let child = runtime::SessionId::new();
        let other = runtime::SessionId::new();
        shared.set_desk("ops", desk);
        shared.inherit(desk, child);
        shared.set_desk("qa", other);
        registry
            .ask(InstallMcpAccess(Arc::new(Desks(shared.clone()))))
            .await
            .unwrap();
        let actor = Roster::spawn(r);
        actor.register(ROSTER_ACTOR).unwrap();
        ::tools::mcp::start(&rt, registry.clone(), ::tools::Policy::default())
            .await
            .unwrap();
        let pool = agent::repl::open_pool(std::path::Path::new(&worker), 2)
            .await
            .expect("Monty worker");
        for (session, name) in [(desk, "desk-added"), (child, "child-added")] {
            registry
                .ask(::tools::SetApprovals {
                    session: Some(session),
                    enabled: false,
                    clear: false,
                })
                .await
                .unwrap();
            let cx = agent::repl::RunCx::for_test(rt.clone(), registry.clone(), session);
            let mut repl = agent::repl::Repl::open(&pool, session, data.path(), cx).await;
            let code = format!(
                "added = await add_mcp_server(name={name:?}, command='python3', args=[{:?}])\nprint(added)",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/tests/mcp_live.py")
            );
            let out = repl.run("add", &code).await;
            assert!(!out.is_error, "desk add: {}", out.content);
            let ident = "live_echo";
            assert!(
                registry
                    .ask(ListTools(Some(session)))
                    .await
                    .unwrap()
                    .iter()
                    .any(|s| s.name == ident),
                "desk allowlist still hides the server it just added"
            );
            let out = repl
                .run(
                    "call",
                    &format!("reply = await {ident}(value='desk')\nprint(reply)"),
                )
                .await;
            assert!(
                !out.is_error && out.content.contains("live:desk:"),
                "desk's next cell must see the server it added: {}",
                out.content
            );
            let specs = registry.ask(ListTools(Some(session))).await.unwrap();
            let index = agent::repl::prompt_block(&specs, true);
            assert!(
                index.contains(&format!("async def {ident}(value: str) -> Any")),
                "desk index: {index}"
            );
            let persisted = std::fs::read_to_string(data.path().join("agents/ops.md")).unwrap();
            let f = file::parse("ops", &persisted).unwrap();
            assert!(
                f.mcp.as_ref().unwrap().contains(&name.to_owned()),
                "grant must persist in agent file: {persisted}"
            );
            assert_eq!(
                shared.file("ops").unwrap(),
                f,
                "Agents view must see the write immediately"
            );
            assert_eq!(shared.file("qa").unwrap(), qa, "another agent is untouched");
            assert!(
                !registry
                    .ask(ListTools(Some(other)))
                    .await
                    .unwrap()
                    .iter()
                    .any(|s| s.name.ends_with("live_echo")),
                "another agent must not gain the server"
            );
            let before = persisted;
            let out = repl
                .run("remove", &format!("await remove_mcp_server(name={name:?})"))
                .await;
            assert!(!out.is_error, "{}", out.content);
            assert_eq!(
                std::fs::read_to_string(data.path().join("agents/ops.md")).unwrap(),
                before,
                "remove edits no agent file"
            );
            repl.close().await;
        }
        actor.kill();
        unregister(&[ROSTER_ACTOR]);
    });
}
