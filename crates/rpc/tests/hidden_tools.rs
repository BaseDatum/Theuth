//! Per-chat tool hides survive a restart: `set_mcp_hidden` on the owner writes
//! `sessions.hidden_tools` before it replies, and the chat's actor restores the
//! row into the registry when it starts again. Its own binary: it registers the
//! agent and tool registry actors, which the process-wide kameo registry allows
//! once.

use std::collections::HashSet;
use std::sync::Arc;

use agent::{CreateSessionOptions, DurableStore, PinTitle, SessionOpen, UnloadSession};
use kameo::actor::ActorRef;
use rpc::ApiServer;
use rpc::api::Server;
use runtime::{Runtime, SessionId};
use tools::{HiddenTools, ListTools, RestoreHidden, ToolRegistry};

/// The chat's actor stops, the registry forgets what it held (a fresh one after
/// an engine restart holds `stale`), and the next route starts the actor again.
async fn restart(
    agents: &ActorRef<agent::Agents>,
    reg: &ActorRef<ToolRegistry>,
    session: SessionId,
    stale: &[String],
) {
    assert!(agents.ask(UnloadSession { session }).await.unwrap());
    reg.ask(RestoreHidden {
        session,
        tools: stale.to_vec(),
    })
    .await
    .unwrap();
    agents
        .ask(PinTitle {
            session,
            pinned: false,
        })
        .await
        .unwrap();
}

fn set(names: &[&str]) -> HashSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[tokio::test]
async fn hides_survive_an_actor_restart_and_unhide_clears_them() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let reg = tools::registry::start(&rt).await.unwrap();
    let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
        .await
        .unwrap();
    let store = agents.ask(DurableStore).await.unwrap();
    reg.ask(tools::InstallJournal(Arc::new(
        agent::durability::journal::StoreJournal(Arc::new(store.clone())),
    )))
    .await
    .unwrap();

    let (peer, _guard) = link::RpcPeer::open();
    let mut api: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
    api.extensions_mut().insert(peer);
    let open: SessionOpen = api
        .call("create_session", (CreateSessionOptions::default(),))
        .await
        .unwrap();
    let s = open.session;
    let names: Vec<String> = reg
        .ask(ListTools(None))
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .take(3)
        .collect();
    let [a, b, c] = [&names[0], &names[1], &names[2]].map(String::as_str);
    let visible = |reg: ActorRef<ToolRegistry>| async move {
        reg.ask(ListTools(Some(s)))
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect::<HashSet<_>>()
    };

    // Hidden on the owner: written before the reply, sorted.
    let () = api.call("set_mcp_hidden", (s, [b, a], true)).await.unwrap();
    assert_eq!(store.session_hidden(s).await.unwrap(), [a, b]);

    // Restart: the registry comes back empty, the actor restores the row.
    restart(&agents, &reg, s, &[]).await;
    assert_eq!(reg.ask(HiddenTools(s)).await.unwrap(), set(&[a, b]));
    let shown = visible(reg.clone()).await;
    assert!(!shown.contains(a) && !shown.contains(b) && shown.contains(c));

    // A name no longer registered (a tool removed since) is inert: the rest
    // still applies and the listing is served.
    store
        .set_session_hidden(s, &["mcp__gone__tool".to_string(), a.to_string()])
        .await
        .unwrap();
    restart(&agents, &reg, s, &[]).await;
    assert_eq!(
        reg.ask(HiddenTools(s)).await.unwrap(),
        set(&["mcp__gone__tool", a])
    );
    let shown = visible(reg.clone()).await;
    assert!(!shown.contains(a) && shown.contains(b) && shown.contains(c));

    // Unhide clears the row; a registry still holding a stale set is cleared
    // when the actor starts.
    let () = api
        .call("set_mcp_hidden", (s, [a, "mcp__gone__tool"], false))
        .await
        .unwrap();
    assert!(store.session_hidden(s).await.unwrap().is_empty());
    restart(&agents, &reg, s, &[a.to_string()]).await;
    assert!(reg.ask(HiddenTools(s)).await.unwrap().is_empty());
    assert!(visible(reg.clone()).await.contains(a));
}
