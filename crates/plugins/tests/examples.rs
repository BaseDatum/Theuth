//! Design 216 §15 (host steps): the two examples of `docs/design/216-examples/` (turns, ping)
//! run through the real host — `PluginsPlugin::start`, the vault's grants key, a human press,
//! the supervisor, the router and the placement board — on a temp data dir.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::actor::{ActorRef, Spawn as _};
use plugins::{
    Action, Approve, GetPlugin, HumanPress, ListPlugins, PluginRunState, PluginsActor, Surface,
    SurfaceLoad, Surfaces,
};
use runtime::Plugin as _;

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/216-examples")
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

fn on_path(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .is_ok()
}

async fn until<T>(what: &str, secs: u64, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn press() -> HumanPress {
    let (peer, guard) = rpc_core::RpcPeer::open();
    std::mem::forget(guard); // the "connection" stays open for the test
    let mut ext = jsonrpsee::Extensions::new();
    ext.insert(peer);
    HumanPress::from_rpc(&ext).expect("a local peer is a press")
}

async fn approve(actor: &ActorRef<PluginsActor>, name: &str) {
    let card = until(&format!("{name}'s card"), 15, async || {
        actor
            .ask(GetPlugin { name: name.into() })
            .await
            .ok()
            .and_then(|d| d.card)
    })
    .await;
    actor
        .ask(Approve {
            name: name.into(),
            manifest_sha256: card.manifest_sha256,
            code_sha256: card.code_sha256,
            press: press(),
        })
        .await
        .expect("approve");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turns_and_ping_examples_run_through_the_host() {
    if !on_path("python3") || !on_path("bash") {
        eprintln!("skipped: python3 and bash are needed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().to_path_buf();
    copy(&examples().join("turns"), &data.join("plugins/turns"));
    copy(&examples().join("ping"), &data.join("plugins/ping"));

    let rt = runtime::Runtime::new(Default::default(), data.clone());
    // The topics turns subscribes to, as the engine's declaration pass would have them.
    agent::declare_wire(&rt).await.unwrap();
    let vault = Arc::new(
        secrets::Vault::open(&data, secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let sa = secrets::SecretsActor::spawn(secrets::SecretsActor::new(
        vault,
        rt.clone(),
        Duration::from_secs(5),
    ));
    sa.register(secrets::SECRETS_ACTOR).unwrap();

    Box::new(plugins::PluginsPlugin).start(&rt).await.unwrap();
    let actor = until("the plugins actor", 5, async || {
        ActorRef::<PluginsActor>::lookup(plugins::PLUGINS_ACTOR)
            .ok()
            .flatten()
    })
    .await;

    // Nothing runs before a press: both wait with an install card.
    let list = until("both plugins listed", 15, async || {
        let l = actor.ask(ListPlugins).await.ok()?;
        (l.len() == 2 && l.iter().all(|p| p.needs_approval)).then_some(l)
    })
    .await;
    assert!(
        list.iter()
            .all(|p| p.state == PluginRunState::AwaitingApproval),
        "{list:?}"
    );

    approve(&actor, "turns").await;
    approve(&actor, "ping").await;
    until("both running", 20, async || {
        let l = actor.ask(ListPlugins).await.ok()?;
        l.iter()
            .all(|p| p.state == PluginRunState::Running && p.enabled && !p.needs_approval)
            .then_some(())
    })
    .await;
    assert!(data.join("plugins/grants.yaml").is_file());

    // One TurnFinished on the bus reaches turns through the wire feed and the router.
    rt.publish(agent::AgentEvent::TurnFinished {
        session: runtime::SessionId::new(),
        turn: runtime::TurnId::new(),
        stop_reason: "end_turn".into(),
        usage: agent::Usage {
            prompt: 100,
            completion: 20,
            cached: 0,
        },
        context: 120,
    })
    .await;
    let state = data.join("plugin-data/turns/state.json");
    // No meeting actor here: the feed holds frames for route's SEED_WAIT (10 s) before routing.
    until("turns: 1 in state.json", 20, async || {
        let s: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state).ok()?).ok()?;
        (s["turns"] == 1).then_some(())
    })
    .await;

    // The status dots arrived (ping's is ok or error depending on the network).
    until("status snapshots", 20, async || {
        let snaps = actor.ask(Surfaces).await.ok()?;
        let turns = snaps.iter().find(|s| s.plugin == "turns")?;
        let ping = snaps.iter().find(|s| s.plugin == "ping")?;
        (turns.badge.is_some() && ping.tooltip.as_deref() != Some("starting")).then_some(())
    })
    .await;

    // A press on the popover's button → `action` → the plugin resets and re-renders.
    let reply = actor
        .ask(Action {
            name: "turns".into(),
            placement: "turns".into(),
            action: "reset".into(),
            args: serde_json::json!({}),
            session: None,
            client: "desktop".into(),
            activation: Some("act-1".into()),
            form: None,
        })
        .await
        .expect("action");
    assert_eq!(reply, serde_json::json!({"ok": true}));
    until("the surface shows **0**", 15, async || {
        let c = actor
            .ask(Surface {
                name: "turns".into(),
                placement: "turns".into(),
                session: None,
                client: "desktop".into(),
            })
            .await
            .ok()?;
        (c.state == SurfaceLoad::Ready && c.source.as_deref()?.contains("**0**")).then_some(())
    })
    .await;

    // Engine shutdown: every process gets `shutdown` and is gone; `.pgids` is empty.
    rt.shutdown.cancel();
    until("processes gone", 10, async || {
        let pg = std::fs::read_to_string(data.join("plugins/.pgids")).unwrap_or_default();
        pg.trim().is_empty().then_some(())
    })
    .await;
}
