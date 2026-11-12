use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use agent::Agents;
use agent::store::Store;
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::{ErrorObjectOwned, Id, Request, Response, ResponsePayload};
use replica::{ALPN_RPC, PeerId, Transport};
use runtime::{Runtime, SessionId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use agent::rpc_forward::*;

#[cfg(test)]
mod tests {
    use super::*;
    use replica::{ByteStream, PeerEvent};
    use std::collections::HashMap;
    use tokio::sync::{broadcast, mpsc};

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Two-node loopback switch, as `replica/tests/sync_loopback.rs`.
    struct Loop {
        me: PeerId,
        inboxes: Arc<parking_lot::Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    fn pair() -> (Arc<Loop>, Arc<Loop>) {
        crate::install_forward_hooks();
        let inboxes = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let mk = |name: &str| {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            inboxes.lock().insert(me.clone(), tx);
            Arc::new(Loop {
                me,
                inboxes: inboxes.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        };
        (mk("a"), mk("b"))
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.inboxes
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }

        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let to = self
                .inboxes
                .lock()
                .get(peer)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }

        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("closed"))
        }

        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    /// What the fake owner records per `send_input`: text, resolved files, extensions.
    type Seen = (String, Vec<PathBuf>, Option<Extensions>);

    /// The fake owner's context: where it reports, and the transport + data
    /// dir it resolves forwarded attachments with.
    struct Owner {
        seen: mpsc::UnboundedSender<Seen>,
        t: Arc<Loop>,
        data_dir: PathBuf,
    }

    /// A fake owner: one `send_input` method that resolves `files` like the
    /// real one and records what reached it.
    fn fake_owner(
        seen: mpsc::UnboundedSender<Seen>,
        t: Arc<Loop>,
        data_dir: PathBuf,
    ) -> jsonrpsee::Methods {
        let mut m = jsonrpsee::RpcModule::new(Owner { seen, t, data_dir });
        m.register_async_method("send_input", |params, o, ext| async move {
            let mut seq = params.sequence();
            let session: Option<String> = seq.next()?;
            let text: String = seq.next()?;
            let files: Vec<PathBuf> = seq.optional_next()?.unwrap_or_default();
            let files = resolve_files_via(&ext, o.t.as_ref(), &o.data_dir, files).await?;
            let _ = o.seen.send((text, files, Some(ext)));
            RpcResult::Ok(session.unwrap_or_default())
        })
        .unwrap();
        m.register_method("rename_session", |_, _, _| -> RpcResult<()> {
            Err(ErrorObjectOwned::owned(
                -32000,
                "no such session",
                None::<()>,
            ))
        })
        .unwrap();
        m.into()
    }

    #[tokio::test]
    async fn forwarded_send_input_reaches_the_owner_dispatch_as_internal_forwarded_peer() {
        let (a, b) = pair();
        let (seen_tx, mut seen) = mpsc::unbounded_channel();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(
            rt,
            b.clone(),
            fake_owner(seen_tx, b.clone(), std::env::temp_dir()),
        ));
        let params = serde_json::value::to_raw_value(&(Some("s-1"), "hello")).unwrap();
        let reply = forward(a.as_ref(), &b.me, "send_input", &params)
            .await
            .unwrap();
        assert_eq!(reply.get(), "\"s-1\"");
        let (text, files, ext) = seen.recv().await.unwrap();
        assert_eq!(text, "hello");
        assert!(files.is_empty());
        let ext = ext.unwrap();
        assert_eq!(
            ext.get::<Forwarded>().unwrap().0,
            a.me,
            "owner dispatch carries the one-hop marker naming the forwarder"
        );
        assert!(
            ext.get::<rpc_core::RpcPeer>().is_some(),
            "owner dispatch runs as the internal peer"
        );
    }

    /// A1: the forwarder sends `files` by name (`attachments/<sha16>.<ext>`);
    /// the owner fetches each from the forwarder over `theuth/artifacts/1`
    /// into its own `attachments/` before the actor sees the paths.
    #[tokio::test]
    async fn forwarded_send_input_carries_attachments_by_name_and_the_owner_resolves_them() {
        let (a, b) = pair();
        let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // The forwarder holds two attachments and serves the artifacts ALPN.
        let (png, pdf) = (b"\x89PNG one".to_vec(), b"%PDF two".to_vec());
        let p1 = agent::context::save_blob(&png, "png", a_dir.path()).unwrap();
        let p2 = agent::context::save_blob(&pdf, "pdf", a_dir.path()).unwrap();
        let rt_a = Runtime::new(figment::Figment::new(), a_dir.path().to_path_buf());
        let state = artifact::State::open(a_dir.path(), rt_a.clone(), vec![])
            .await
            .unwrap();
        tokio::spawn(artifact::mesh::serve_on(
            rt_a,
            a.clone(),
            Arc::new(artifact::Stores::new(Arc::new(state))),
        ));
        // The owner answers `theuth/rpc/1`.
        let (seen_tx, mut seen) = mpsc::unbounded_channel();
        let rt_b = Runtime::new(figment::Figment::new(), b_dir.path().to_path_buf());
        tokio::spawn(serve(
            rt_b,
            b.clone(),
            fake_owner(seen_tx, b.clone(), b_dir.path().to_path_buf()),
        ));

        let files = vec![p1.clone(), p2.clone(), PathBuf::from("/elsewhere/x.png")];
        let wire = serde_json::to_value(ByName(&files)).unwrap();
        let (n1, n2) = (
            p1.file_name().unwrap().to_str().unwrap(),
            p2.file_name().unwrap().to_str().unwrap(),
        );
        assert_eq!(
            wire,
            serde_json::json!([
                format!("attachments/{n1}"),
                format!("attachments/{n2}"),
                "/elsewhere/x.png"
            ]),
            "store paths travel as names, anything else verbatim"
        );
        let params = serde_json::value::to_raw_value(&(
            Some("s-1"),
            "look",
            ByName(&files[..2]),
            None::<()>,
        ))
        .unwrap();
        let reply = forward(a.as_ref(), &b.me, "send_input", &params)
            .await
            .unwrap();
        assert_eq!(reply.get(), "\"s-1\"");
        let (text, resolved, _) = seen.recv().await.unwrap();
        assert_eq!(text, "look");
        let dir = artifact::mesh::attachments_dir(b_dir.path());
        assert_eq!(
            resolved,
            vec![dir.join(n1), dir.join(n2)],
            "the owner's own attachments/ paths"
        );
        assert_eq!(std::fs::read(&resolved[0]).unwrap(), png);
        assert_eq!(std::fs::read(&resolved[1]).unwrap(), pdf);
    }

    #[tokio::test]
    async fn owner_errors_pass_through_verbatim() {
        let (a, b) = pair();
        let (seen_tx, _seen) = mpsc::unbounded_channel();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(
            rt,
            b.clone(),
            fake_owner(seen_tx, b.clone(), std::env::temp_dir()),
        ));
        let params = serde_json::value::to_raw_value(&("s-1", "t")).unwrap();
        let e = forward(a.as_ref(), &b.me, "rename_session", &params)
            .await
            .unwrap_err()
            .unwrap_err();
        assert_eq!(e.code(), -32000);
        assert_eq!(e.message(), "no such session");
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_owner_is_unreachable_after_the_timeout() {
        let (a, b) = pair();
        // b accepts but never answers.
        tokio::spawn(async move {
            let (_peer, _stream) = b.accept(ALPN_RPC).await.unwrap();
            std::future::pending::<()>().await;
        });
        let params = serde_json::value::to_raw_value(&("s-1",)).unwrap();
        let why = forward(a.as_ref(), &PeerId("b".into()), "cancel", &params)
            .await
            .unwrap_err()
            .unwrap();
        assert!(why.contains("no answer within 30s"), "{why}");
    }

    /// The shape U parses: `data = {"OwnerUnreachable": {"owner", "last_seen"}}`, code -32010.
    #[tokio::test]
    async fn owner_unreachable_error_is_the_external_enum_form() {
        let e = unreachable("engine-a".into(), "not connected").await;
        assert_eq!(e.code(), OWNER_UNREACHABLE);
        let data: serde_json::Value = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            serde_json::json!({ "OwnerUnreachable": { "owner": "engine-a", "last_seen": null } }),
            "no mesh in a test: never seen"
        );
        assert!(
            e.message().starts_with("owner engine-a unreachable"),
            "{}",
            e.message()
        );
    }

    /// 183 §4.1 (D1/D2): a row owned elsewhere forwards when its owner is
    /// connected and is `Offline` (→ -32010 at once) when not — user sessions
    /// and desks alike; nothing classifies as a take any more.
    #[tokio::test]
    async fn classify_forwards_to_a_connected_owner_and_is_offline_otherwise() {
        crate::install_forward_hooks();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite");
        let (user, desk, roster, mine) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        // Rows written by another engine ("them") ...
        let theirs = Store::open(&path, Some(mesh::EngineId("them".into())))
            .await
            .unwrap();
        theirs
            .create_session(user, "m", None, None, None)
            .await
            .unwrap();
        theirs
            .create_session(desk, "m", None, None, None)
            .await
            .unwrap();
        theirs.set_acp(desk, "omp", None, &[]).await.unwrap();
        // ... a roster desk: the row's `desk` mark (roster::open_desk) ...
        theirs
            .create_session(roster, "m", None, None, None)
            .await
            .unwrap();
        theirs.set_desk(roster, Some("ops")).await.unwrap();
        theirs.close().await;
        // ... seen from this engine ("me"), which also has a session of its own.
        let store = Store::open(&path, Some(mesh::EngineId("me".into())))
            .await
            .unwrap();
        store
            .create_session(mine, "m", None, None, None)
            .await
            .unwrap();
        let them = PeerId("them".into());
        let up = [them.clone()];
        assert_eq!(classify(&store, mine, &up).await.unwrap(), Route::Here);
        assert_eq!(
            classify(&store, SessionId::new(), &up).await.unwrap(),
            Route::Here,
            "unknown → local error path"
        );
        for s in [user, desk, roster] {
            assert_eq!(
                classify(&store, s, &up).await.unwrap(),
                Route::Forward(them.clone()),
                "connected owner: forward, never take"
            );
            assert_eq!(
                classify(&store, s, &[]).await.unwrap(),
                Route::Offline(them.clone()),
                "owner not connected: offline at once"
            );
        }
        // 183 §5.6: a chat "them" just created for us, its row not here yet,
        // is theirs; once a row is here (ours in this store), the row decides.
        let created = SessionId::new();
        sessionsync::owner::expect(created, them.clone());
        assert_eq!(
            classify(&store, created, &up).await.unwrap(),
            Route::Forward(them.clone())
        );
        assert_eq!(
            classify(&store, created, &[]).await.unwrap(),
            Route::Offline(them.clone())
        );
        sessionsync::owner::expect(mine, them.clone());
        assert_eq!(classify(&store, mine, &up).await.unwrap(), Route::Here);
        // Classifying never moved anything.
        assert_eq!(store.owner(user).await.unwrap().unwrap().0, "them");
        store.close().await;
    }

    /// 183 §4.8: the owner runs a forwarded call only for a row it owns; a row
    /// owned elsewhere is `-32013 NotOwner{owner}` (never relayed on).
    #[tokio::test]
    async fn a_forwarded_call_for_a_row_owned_elsewhere_is_not_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite");
        let (theirs_id, mine) = (SessionId::new(), SessionId::new());
        let theirs = Store::open(&path, Some(mesh::EngineId("them".into())))
            .await
            .unwrap();
        theirs
            .create_session(theirs_id, "m", None, None, None)
            .await
            .unwrap();
        theirs.close().await;
        let store = Store::open(&path, Some(mesh::EngineId("me".into())))
            .await
            .unwrap();
        store
            .create_session(mine, "m", None, None, None)
            .await
            .unwrap();
        admit_forwarded(&store, mine).await.unwrap();
        admit_forwarded(&store, SessionId::new()).await.unwrap();
        let e = admit_forwarded(&store, theirs_id).await.unwrap_err();
        assert_eq!(e.code(), NOT_OWNER);
        let data: FrontDoorError = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            FrontDoorError::NotOwner {
                owner: "them".into()
            }
        );
        store.close().await;
    }

    /// 183 §4.8.3: a call that arrived forwarded is never sent on a second hop.
    #[tokio::test]
    async fn a_forwarded_call_is_never_forwarded_again() {
        let mut ext = Extensions::new();
        ext.insert(Forwarded(PeerId("a".into())));
        let r: Option<String> = to_peer(&ext, &PeerId("c".into()), "send_input", ("s",))
            .await
            .unwrap();
        assert_eq!(r, None, "the caller keeps its local answer");
    }

    /// 69 P: the agent crate's hand-written `peer_deliver` request line is
    /// dispatched by [`serve`] like any forwarded RPC (positional params,
    /// internal peer, `Forwarded` marker), and jsonrpsee's response line is
    /// read back as the owner's receipt; an owner-side refusal is `Refused`.
    #[tokio::test]
    async fn peer_deliver_line_round_trips_through_the_owner_serve() {
        use agent::communication::mesh::{self as pm, Failure};
        use agent::{PeerEnvelope, PeerMessageState, PeerReceipt};

        let (a, b) = pair();
        let (seen_tx, mut seen) =
            mpsc::unbounded_channel::<(SessionId, PeerEnvelope, Option<Extensions>)>();
        let mut m = jsonrpsee::RpcModule::new(seen_tx);
        m.register_async_method("peer_deliver", |params, seen, ext| async move {
            let (session, env): (SessionId, PeerEnvelope) = params.parse()?;
            if env.text == "no" {
                return Err(ErrorObjectOwned::owned(
                    -32000,
                    "send_message: communication disabled: off",
                    None::<()>,
                ));
            }
            let receipt = PeerReceipt {
                id: env.id,
                to: format!("session:{session}"),
                state: PeerMessageState::Accepted,
                reply_to: env.reply_to,
                reply: None,
                task_id: None,
                error: None,
                answered_by: None,
            };
            let _ = seen.send((session, env, Some(ext)));
            RpcResult::Ok(receipt)
        })
        .unwrap();
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        tokio::spawn(serve(rt, b.clone(), m.into()));

        let session = SessionId::new();
        let env = PeerEnvelope {
            id: uuid::Uuid::now_v7(),
            from: SessionId::new(),
            text: "hello".into(),
            reply_to: Some(uuid::Uuid::now_v7()),
            request_id: Some("r1".into()),
            chain: 0,
        };
        let receipt = pm::forward(a.as_ref(), &b.me, session, &env).await.unwrap();
        assert_eq!(
            (receipt.id, receipt.state, receipt.reply_to),
            (env.id, PeerMessageState::Accepted, env.reply_to)
        );
        assert_eq!(receipt.to, format!("session:{session}"));
        let (s, e, ext) = seen.recv().await.unwrap();
        assert_eq!((s, e), (session, env.clone()));
        let ext = ext.unwrap();
        assert_eq!(ext.get::<Forwarded>().unwrap().0, a.me);
        assert!(ext.get::<rpc_core::RpcPeer>().is_some());

        let refused = pm::forward(
            a.as_ref(),
            &b.me,
            session,
            &PeerEnvelope {
                text: "no".into(),
                ..env
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            refused,
            Failure::Refused("send_message: communication disabled: off".into())
        );
    }
}
