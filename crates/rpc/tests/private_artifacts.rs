//! 51 §4.2: a private tree's artifact records, archive listings and blob
//! exports are served by `get_artifact`, `get_artifact_listing` and
//! `export_artifact` to the owning connection only, and only when it names
//! the session — the rule `get_attachment` applies to the bytes. Durable
//! artifacts are unchanged. Its own binary: it registers the agent, tool
//! registry and artifact actors, which the process-wide kameo registry
//! allows once.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::{Authorize, CreateSessionOptions, SessionOpen};
use jsonrpsee::core::server::MethodsError;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::{ActorRef, Spawn};
use rpc::ApiServer;
use rpc::api::Server;
use runtime::{Runtime, SessionId};
use serde::de::DeserializeOwned;

/// `ask` failed: a handler error, here `authorize` refusing or the lookup
/// missing the record (`rpc::api::ASK_FAILED`).
const ASK_FAILED: i32 = -32000;

/// An empty zip: the end-of-central-directory record alone.
const EMPTY_ZIP: [u8; 22] = [
    0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// One authenticated connection to the shared server state.
fn conn(methods: &jsonrpsee::Methods) -> (jsonrpsee::Methods, link::RpcPeer, link::RpcPeerGuard) {
    let (peer, guard) = link::RpcPeer::open();
    let mut methods = methods.clone();
    methods.extensions_mut().insert(peer.clone());
    (methods, peer, guard)
}

async fn call<T: DeserializeOwned + Clone>(
    methods: &jsonrpsee::Methods,
    method: &str,
    params: impl jsonrpsee::core::traits::ToRpcParams + Send,
) -> Result<T, ErrorObjectOwned> {
    methods
        .call::<_, T>(method, params)
        .await
        .map_err(|e| match e {
            MethodsError::JsonRpc(e) => e,
            other => panic!("{method}: not a call error: {other}"),
        })
}

/// The refusals the client sees: `-32000` and, from `authorize`, `unauthorized`.
fn refused<T: std::fmt::Debug>(r: Result<T, ErrorObjectOwned>, message: Option<&str>) {
    let e = r.expect_err("served");
    assert_eq!(e.code(), ASK_FAILED, "{e}");
    if let Some(m) = message {
        assert_eq!(e.message(), m);
    }
}

async fn show(
    artifacts: &ActorRef<artifact::ArtifactActor>,
    session: SessionId,
    call_id: &str,
    args: artifact::ShowArgs,
) -> artifact::Artifact {
    artifacts
        .ask(artifact::Show {
            session,
            call_id: call_id.into(),
            args,
        })
        .await
        .unwrap()
}

/// The artifacts come from the actor's real `Show` path. The actor is this
/// test's own over its own `Stores`, with the private tree mapped directly
/// (`Stores::open_tree`, what `TreeOpened` does), so no record can land in
/// the durable store by a race with the lifecycle registration.
#[tokio::test]
async fn private_artifacts_are_served_to_the_owner_with_the_session_only() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    tools::registry::start(&rt).await.unwrap();
    let agents = agent::spawn_test_agents(rt.clone(), |_: &str| true)
        .await
        .unwrap();
    let durable = artifact::store::State::open(dir.path(), rt.clone(), Vec::new())
        .await
        .unwrap();
    let stores = Arc::new(artifact::Stores::new(Arc::new(durable)));
    let artifacts = artifact::ArtifactActor::spawn(artifact::ArtifactActor::new(stores.clone()));
    artifacts.register(artifact::ARTIFACT_ACTOR).unwrap();

    let base: jsonrpsee::Methods = Server::new(rt).into_rpc().into();
    let (a, peer_a, _guard_a) = conn(&base);
    let (b, _peer_b, _guard_b) = conn(&base);

    let open: SessionOpen = call(
        &a,
        "create_session",
        (CreateSessionOptions {
            retention: tools::Retention::Private,
            view: Some("v1".into()),
            ..Default::default()
        },),
    )
    .await
    .unwrap();
    let private = open.session;
    let resources = agents
        .ask(Authorize {
            session: private,
            peer: Some(peer_a.id()),
        })
        .await
        .unwrap()
        .resources;
    stores
        .open_tree(private, &[private], &resources)
        .await
        .unwrap();

    let zip = dir.path().join("bundle.zip");
    std::fs::write(&zip, EMPTY_ZIP).unwrap();
    let note = show(
        &artifacts,
        private,
        "p-note",
        artifact::ShowArgs {
            content: Some("# private\n".into()),
            name: Some("a.md".into()),
            check: Some(false),
            ..Default::default()
        },
    )
    .await;
    assert!(note.path.starts_with(&resources.root), "{note:?}");
    let bundle = show(
        &artifacts,
        private,
        "p-zip",
        artifact::ShowArgs {
            path: Some(zip.clone()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(bundle.kind, artifact::Kind::Archive);
    let dest = |n: &str| -> PathBuf { dir.path().join("out").join(n) };
    let exported = |p: &Path| std::fs::read(p).unwrap();

    // No session: a private record is not found, listed or exported.
    refused(
        call::<artifact::Artifact>(&a, "get_artifact", ("p-note", None::<SessionId>)).await,
        None,
    );
    refused(
        call::<artifact::archive::ArchiveListing>(
            &a,
            "get_artifact_listing",
            ("p-zip", None::<SessionId>),
        )
        .await,
        None,
    );
    refused(
        call::<PathBuf>(
            &a,
            "export_artifact",
            (&note.id, dest("none.md"), None::<SessionId>),
        )
        .await,
        None,
    );
    assert!(!dest("none.md").exists());

    // The owner naming the session: the record, the listing, the blob.
    let got: artifact::Artifact = call(&a, "get_artifact", ("p-note", Some(private)))
        .await
        .unwrap();
    assert_eq!(got.id, note.id);
    let listing: artifact::archive::ArchiveListing =
        call(&a, "get_artifact_listing", ("p-zip", Some(private)))
            .await
            .unwrap();
    assert_eq!(listing.total_entries, 0);
    let written: PathBuf = call(
        &a,
        "export_artifact",
        (&note.id, dest("owner.md"), Some(private)),
    )
    .await
    .unwrap();
    assert_eq!(exported(&written), b"# private\n");

    // Another connection naming the session: refused by `authorize`.
    refused(
        call::<artifact::Artifact>(&b, "get_artifact", ("p-note", Some(private))).await,
        Some("unauthorized"),
    );
    refused(
        call::<artifact::archive::ArchiveListing>(
            &b,
            "get_artifact_listing",
            ("p-zip", Some(private)),
        )
        .await,
        Some("unauthorized"),
    );
    refused(
        call::<PathBuf>(
            &b,
            "export_artifact",
            (&note.id, dest("b.md"), Some(private)),
        )
        .await,
        Some("unauthorized"),
    );
    assert!(!dest("b.md").exists());

    // A durable artifact: any connection, with or without its session, as before.
    let plain: SessionOpen = call(&a, "create_session", (CreateSessionOptions::default(),))
        .await
        .unwrap();
    let shared = show(
        &artifacts,
        plain.session,
        "d-note",
        artifact::ShowArgs {
            content: Some("# shared\n".into()),
            name: Some("d.md".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(!shared.path.starts_with(&resources.root));
    for (who, m) in [("a", &a), ("b", &b)] {
        for session in [None, Some(plain.session)] {
            let got: artifact::Artifact =
                call(m, "get_artifact", ("d-note", session)).await.unwrap();
            assert_eq!(got.id, shared.id, "{who} {session:?}");
            let out = dest(&format!("{who}-{}.md", session.is_some()));
            let written: PathBuf = call(m, "export_artifact", (&shared.id, &out, session))
                .await
                .unwrap();
            assert_eq!(exported(&written), b"# shared\n");
        }
    }
    // Naming the private session does not hide durable records from its owner.
    let got: artifact::Artifact = call(&a, "get_artifact", ("d-note", Some(private)))
        .await
        .unwrap();
    assert_eq!(got.id, shared.id);

    // Release the tree so its scratch dir goes with the test.
    call::<agent::CloseResult>(&a, "leave_session", (private, open.lease_secret))
        .await
        .unwrap();
}
