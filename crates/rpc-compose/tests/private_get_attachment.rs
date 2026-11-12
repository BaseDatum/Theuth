//! 51 §4.2: a private tree's files are served by `get_attachment` to the
//! owning connection only, and never without naming the session. Its own
//! binary: it registers the agent and tool-registry actors, which the
//! process-wide kameo registry allows once.

use std::path::Path;

use agent::{Authorize, CreateSessionOptions, SessionOpen};
use base64::Engine as _;
use jsonrpsee::core::server::MethodsError;
use jsonrpsee::types::ErrorObjectOwned;

use runtime::{Runtime, SessionId};

/// `ask` failed: a handler error, here `authorize` refusing or the path
/// check (`rpc_core::ASK_FAILED`).
const ASK_FAILED: i32 = -32000;

/// One authenticated connection to the shared server state.
fn conn(methods: &jsonrpsee::Methods) -> (jsonrpsee::Methods, rpc_core::RpcPeer, rpc_core::RpcPeerGuard) {
    let (peer, guard) = rpc_core::RpcPeer::open();
    let mut methods = methods.clone();
    methods.extensions_mut().insert(peer.clone());
    (methods, peer, guard)
}

async fn get_attachment(
    methods: &jsonrpsee::Methods,
    path: &Path,
    session: Option<SessionId>,
) -> Result<String, ErrorObjectOwned> {
    methods
        .call::<_, String>("get_attachment", (path, session))
        .await
        .map_err(|e| match e {
            MethodsError::JsonRpc(e) => e,
            other => panic!("not a call error: {other}"),
        })
}

/// The artifact is a file written where the private store puts its blobs
/// (`<resources.root>/artifacts/`, `artifact::State::open_private`), not
/// produced by the `show` tool: the tool needs the artifact plugin and a
/// session turn, and the contract under test is the RPC method's path check
/// and admission, which see only the file.
#[tokio::test]
async fn private_get_attachment_serves_the_owner_only_and_only_with_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    // `CreateSession { private }` asks the registry whether every owner is
    // ready before any private byte exists (51 §8.2); none is expected here.
    tools::registry::start(&rt).await.unwrap();
    let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
        .await
        .unwrap();
    let base: jsonrpsee::Methods = rpc_module(rpc_compose::new_server(rt)).into();
    let (a, peer_a, _guard_a) = conn(&base);
    let (b, _peer_b, _guard_b) = conn(&base);

    let open: SessionOpen = a
        .call(
            "create_session",
            (CreateSessionOptions {
                retention: tools::Retention::Private,
                view: Some("v1".into()),
                ..Default::default()
            },),
        )
        .await
        .unwrap();
    assert_eq!(open.retention, tools::Retention::Private);
    let session = open.session;

    let root = agents
        .ask(Authorize {
            session,
            peer: Some(peer_a.id()),
        })
        .await
        .unwrap()
        .resources
        .root
        .clone();
    let blobs = root.join("artifacts");
    std::fs::create_dir_all(&blobs).unwrap();
    let path = blobs.join("a.md");
    let bytes = b"# private\n\nnot for anyone else\n";
    std::fs::write(&path, bytes).unwrap();

    // No session: the path is judged against `data_dir`, under which it is not.
    let e = get_attachment(&a, &path, None).await.unwrap_err();
    assert_eq!(e.code(), ASK_FAILED, "{e}");
    assert!(
        e.message().ends_with("not an attachment"),
        "{}",
        e.message()
    );

    // The owner, naming the session: the bytes.
    let b64 = get_attachment(&a, &path, Some(session)).await.unwrap();
    assert_eq!(b64, base64::engine::general_purpose::STANDARD.encode(bytes));

    // Another connection, same session: refused by `authorize`, not the path check.
    let e = get_attachment(&b, &path, Some(session)).await.unwrap_err();
    assert_eq!(e.code(), ASK_FAILED, "{e}");
    assert_eq!(e.message(), "unauthorized");

    // Release the tree so its scratch dir goes with the test.
    a.call::<_, agent::CloseResult>("leave_session", (session, open.lease_secret))
        .await
        .unwrap();
}

fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_core::merge([
        rpc_compose::rpc_module(server.clone()),
        a2a::rpc_module(server.clone()),
        agent::rpc_module(server.clone()),
        sessionsync::rpc_module(server.clone()),
        tasks::rpc_module(server.clone()),
        artifact::rpc_module(server.clone()),
        link::rpc_module(server.clone()),
        talk::rpc_module(server.clone()),
        search_providers::rpc_module(server.clone()),
        app::rpc_module(server.clone()),
        rpc::rpc_module(server),
    ])
}
