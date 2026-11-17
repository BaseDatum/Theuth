//! 200 CONTACT-FULL-ACCESS end to end on loopback: two engines, each its own iroh endpoint
//! and agent. A gives B full access: B lists A's visible sessions and messages one A never
//! shared, with no first-contact hold; A answers with `reply_to`; A turns it off: B is
//! refused, its listing too, and A's reply on that conversation is refused at home.

use agent::communication::network::access::{ListRequest, session_address};
use agent::{ListPeerMessages, PeerScope};

use super::*;

/// The newest record of `session` (on `agents`) with `text`, whatever its state.
async fn record(
    agents: &kameo::actor::ActorRef<agent::Agents>,
    session: runtime::SessionId,
    text: &'static str,
) -> agent::PeerRecord {
    eventually(text, || {
        let agents = agents.clone();
        async move {
            agents
                .ask(ListPeerMessages {
                    session,
                    cursor: None,
                    limit: None,
                    pending_only: false,
                })
                .await
                .unwrap()
                .messages
                .into_iter()
                .find(|r| r.text == text)
        }
    })
    .await
}

fn deliver(
    from: runtime::SessionId,
    to: String,
    text: &str,
    reply_to: Option<uuid::Uuid>,
) -> agent::supervisor::Deliver {
    agent::supervisor::Deliver {
        from,
        to,
        text: text.into(),
        await_reply: false,
        reply_to,
        request_id: None,
        admitted: tokio_util::sync::CancellationToken::new(),
    }
}

async fn full_access(e: &Engine, alias: &str, on: bool) -> Contact {
    e.ctx
        .update_contact(ContactUpdate {
            name: alias.to_owned(),
            patch: ContactPatch {
                full_access: Some(on),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn full_access_lists_and_messages_any_session_until_it_is_turned_off() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, b_alias) = paired(&a, &b).await;
    let (a_agents, b_agents) = (a.ctx.agents().unwrap(), b.ctx.agents().unwrap());
    let session_of = |agents: kameo::actor::ActorRef<agent::Agents>| async move {
        agents
            .ask(agent::CreateSession {
                options: agent::CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap()
            .session
    };
    // A's chat, never shared (a new chat is an Owner root); one set Isolated stays hidden.
    let chat = session_of(a_agents.clone()).await;
    let hidden = session_of(a_agents.clone()).await;
    a_agents
        .ask(agent::SetPeerScope {
            root: hidden,
            scope: PeerScope::Isolated,
            alias: None,
        })
        .await
        .unwrap();
    // A chat A shares with B the ordinary way (interpretation 8: it outlives full access).
    let shared = session_of(a_agents.clone()).await;
    a.ctx
        .put_export(Export {
            id: None,
            name: "proj".into(),
            target: ExportTarget::Chat { session: shared },
            description: "The project chat".into(),
            paused: None,
        })
        .await
        .unwrap();
    a.ctx
        .update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                grant: Some(vec!["proj".into()]),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    let origin = session_of(b_agents.clone()).await;
    let address = session_address(&b_alias, chat);

    // Not granted yet: B's card from A says so, and the send is refused before it leaves.
    until("B read A's card", || {
        b.ctx
            .offers
            .lock()
            .get(&a.eid())
            .and_then(|c| c.full_access)
            .map(|g| assert!(!g))
    })
    .await;
    let refused = eventually("B's send refused (card read)", || {
        let (b_agents, address) = (b_agents.clone(), address.clone());
        async move {
            b_agents
                .ask(deliver(origin, address, "too early", None))
                .await
                .err()
                .map(|e| e.to_string())
        }
    })
    .await;
    assert!(
        refused.contains("has not given you full access"),
        "{refused}"
    );

    // A gives B full access: its card for B says so; B lists the chat (not the hidden root).
    assert!(full_access(&a, &a_alias, true).await.full_access);
    until("B sees the grant", || {
        b.ctx
            .offers
            .lock()
            .get(&a.eid())
            .and_then(|c| c.full_access)
            .filter(|g| *g)
    })
    .await;
    let listed = eventually("B lists A's chat", || {
        let (b_agents, b_alias) = (b_agents.clone(), b_alias.clone());
        async move {
            let page = b_agents
                .ask(agent::ListPeers {
                    session: origin,
                    query: Some(b_alias),
                    cursor: None,
                    limit: None,
                    active_only: false,
                    kind: Some(agent::communication::DirectoryKind::Contact),
                })
                .await
                .ok()?;
            page.peers
                .iter()
                .any(|p| p.address.contains("/session:"))
                .then_some(page)
        }
    })
    .await;
    let row = listed
        .peers
        .iter()
        .find(|p| p.address == address)
        .expect("A's chat is listed");
    assert!(row.send, "{row:?}");
    assert_eq!(row.tags[..2], ["contact".to_owned(), "session".to_owned()]);
    assert!(
        !listed
            .peers
            .iter()
            .any(|p| p.address.ends_with(&hidden.to_string())),
        "an Isolated root is hidden"
    );
    assert!(listed.partial.is_empty(), "{:?}", listed.partial);

    // B messages the unshared chat: it arrives at once, no first-contact hold.
    b_agents
        .ask(deliver(origin, address.clone(), "hello", None))
        .await
        .unwrap();
    let x = record(&a_agents, chat, "hello").await;
    assert!(x.from.starts_with("peer:"), "{}", x.from);
    assert_eq!(x.receipt.to, format!("session:{chat}"));
    assert!(
        a_agents
            .ask(agent::communication::network::ContactsHeld)
            .await
            .unwrap()
            .is_empty(),
        "full access never holds for the owner"
    );

    // A answers with reply_to: B's session receives it.
    a_agents
        .ask(deliver(
            chat,
            format!("peer:{a_alias}"),
            "pong",
            Some(x.receipt.id),
        ))
        .await
        .unwrap();
    let back = record(&b_agents, origin, "pong").await;
    assert_eq!(back.receipt.to, format!("session:{origin}"));

    // Off: B's card says so, B is refused, its listing is refused, and A's reply on the
    // conversation is refused at home.
    assert!(!full_access(&a, &a_alias, false).await.full_access);
    until("B sees the grant gone", || {
        b.ctx
            .offers
            .lock()
            .get(&a.eid())
            .and_then(|c| c.full_access)
            .filter(|g| !*g)
    })
    .await;
    let refused = eventually("B's send refused after off", || {
        let (b_agents, address) = (b_agents.clone(), address.clone());
        async move {
            b_agents
                .ask(deliver(origin, address, "too late", None))
                .await
                .err()
                .map(|e| e.to_string())
        }
    })
    .await;
    assert!(
        refused.contains("has not given you full access"),
        "{refused}"
    );
    let listing = crate::contacts::sessions::list(
        &b.net,
        &b_alias,
        &[a.eid()],
        &|e: &str| e.to_owned(),
        &ListRequest {
            limit: 5,
            ..ListRequest::default()
        },
    )
    .await;
    assert_eq!(
        listing.unwrap_err(),
        format!("{b_alias} has not given you full access")
    );
    let refused = a_agents
        .ask(deliver(
            chat,
            format!("peer:{a_alias}"),
            "late",
            Some(x.receipt.id),
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("full access is off"), "{refused}");
    // The share still works.
    b_agents
        .ask(deliver(
            origin,
            format!("peer:{b_alias}/proj"),
            "still shared",
            None,
        ))
        .await
        .unwrap();
    let s = record(&a_agents, shared, "still shared").await;
    assert_eq!(s.receipt.to, format!("session:{shared}"));
}

/// One raw HTTP request from `from` to `to` on `theuth/a2a/1`, as a contact's client sends
/// it: the status, the content type and the body bytes.
async fn raw(
    from: &Engine,
    to: &Engine,
    method: hyper::Method,
    path: impl AsRef<str>,
    body: Option<serde_json::Value>,
) -> (u16, Option<String>, bytes::Bytes) {
    use http_body_util::BodyExt as _;
    let stream = from
        .net
        .a2a_stream(&to.eid(), &to.addrs(), None)
        .await
        .unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut req = hyper::Request::builder()
        .method(method)
        .uri(path.as_ref())
        .header(hyper::header::HOST, "theuth")
        .header("a2a-version", "1.0");
    if body.is_some() {
        req = req.header(hyper::header::CONTENT_TYPE, "application/json");
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let resp = sender
        .send_request(
            req.body(http_body_util::Full::new(bytes::Bytes::from(body)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let kind = resp
        .headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, kind, bytes)
}

/// A JSON-RPC `SendMessage` with a fixed request id and message id (only the path differs
/// between two calls), on `context` when given.
fn send_rpc(text: &str, context: Option<&str>) -> serde_json::Value {
    let mut message =
        a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text(text)]);
    message.message_id = format!("m-{text}");
    message.context_id = context.map(str::to_owned);
    let params = a2a_pb::protojson_conv::to_value(&a2a_types::SendMessageRequest {
        message,
        configuration: Some(a2a_types::SendMessageConfiguration {
            accepted_output_modes: Some(vec!["text/plain".into()]),
            task_push_notification_config: None,
            history_length: Some(0),
            return_immediately: Some(true),
        }),
        metadata: None,
        tenant: None,
    })
    .unwrap();
    serde_json::json!({"jsonrpc": "2.0", "id": "req-1", "method": "SendMessage", "params": params})
}

async fn chat(agents: &kameo::actor::ActorRef<agent::Agents>) -> runtime::SessionId {
    agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions::default(),
            peer: None,
        })
        .await
        .unwrap()
        .session
}

/// D7, revocation: turning full access off while a member's listing is in flight ends it
/// `401` with no body (never a title). D2: a context fixes its session: the contact reusing
/// one session's `contextId` for another session is refused as an unknown context (never
/// delivered into the first one's chat), and a context of its own lands in its own session.
#[tokio::test]
async fn off_ends_a_listing_in_flight_and_a_reused_context_stays_per_session() {
    use agent::communication::network::ReadDelayForTest;
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, _) = paired(&a, &b).await;
    let a_agents = a.ctx.agents().unwrap();
    let (x, y) = (chat(&a_agents).await, chat(&a_agents).await);
    full_access(&a, &a_alias, true).await;

    let send = |s: runtime::SessionId, text: &str, ctx: &str| {
        raw(
            &b,
            &a,
            hyper::Method::POST,
            format!("/agents/session:{s}/rpc"),
            Some(send_rpc(text, Some(ctx))),
        )
    };
    let json = |body: &[u8]| serde_json::from_slice::<serde_json::Value>(body).unwrap();
    let (status, _, body) = send(x, "one", "ctx-x").await;
    assert!(status == 200 && json(&body).get("result").is_some());
    // x's context id, sent to y: refused, never x's chat.
    let (status, _, body) = send(y, "two", "ctx-x").await;
    let v = json(&body);
    assert_eq!(
        (status, &v["error"]["code"], &v["error"]["message"]),
        (
            200,
            &serde_json::json!(a2a_types::error_code::INVALID_PARAMS),
            &serde_json::json!("unknown contextId")
        ),
        "{v}"
    );
    let (status, _, body) = send(y, "three", "ctx-y").await;
    assert!(status == 200 && json(&body).get("result").is_some());
    assert_eq!(
        record(&a_agents, x, "one").await.receipt.to,
        format!("session:{x}")
    );
    assert_eq!(
        record(&a_agents, y, "three").await.receipt.to,
        format!("session:{y}")
    );
    for s in [x, y] {
        let texts: Vec<String> = a_agents
            .ask(ListPeerMessages {
                session: s,
                cursor: None,
                limit: None,
                pending_only: false,
            })
            .await
            .unwrap()
            .messages
            .into_iter()
            .map(|r| r.text)
            .collect();
        assert!(!texts.iter().any(|t| t == "two"), "{s}: {texts:?}");
    }

    // The listing answers while on.
    let (status, _, body) = raw(&b, &a, hyper::Method::GET, "/theuth/sessions?limit=5", None).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&body).contains(&x.to_string()));
    // Held in flight (the member's session reads stall), then turned off.
    a_agents
        .ask(ReadDelayForTest(Duration::from_secs(10)))
        .await
        .unwrap();
    let off = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        a_agents
            .ask(ReadDelayForTest(Duration::ZERO))
            .await
            .unwrap();
        full_access(&a, &a_alias, false).await;
    };
    let started = std::time::Instant::now();
    let ((status, _, body), ()) = tokio::join!(
        raw(&b, &a, hyper::Method::GET, "/theuth/sessions?limit=5", None),
        off
    );
    assert_eq!(status, 401, "{}", String::from_utf8_lossy(&body));
    assert!(body.is_empty(), "no title after the change");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "ended by the epoch, not by the stalled read"
    );
    let (status, _, body) = raw(&b, &a, hyper::Method::GET, "/theuth/sessions?limit=5", None).await;
    assert_eq!((status, &body[..]), (403, &b"full access is off"[..]));
}

/// D4: every session a full-access contact may not see answers byte for byte as an unknown
/// id, on the machine that received the request and on the owner side a forward reaches
/// (`a2a_forward`'s `serve`): another contact's inbox, an opted-out root, an Isolated root, an
/// advisor, a row owned by a machine no longer in the mesh, an inbox on an offline machine.
/// A listing's `after` set to any of their ids pages exactly like a random newer id. (A
/// private chat has no durable row, so it is the unknown id here; this rig has no tool
/// registry to open one: `supervisor::tests::a_private_chat_answers_like_an_unknown_id`.)
#[tokio::test]
async fn every_hidden_session_and_cursor_answers_like_an_unknown_one() {
    use agent::communication::mesh::ForwardFailure;
    use agent::communication::network::OwnedElsewhereForTest;
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, _) = paired(&a, &b).await;
    let a_agents = a.ctx.agents().unwrap();
    let store = a_agents.ask(agent::DurableStore).await.unwrap();
    let visible = chat(&a_agents).await;
    let inbox = chat(&a_agents).await;
    store
        .set_contact_inbox(inbox, "peer:x|export-1")
        .await
        .unwrap();
    let opted_out = chat(&a_agents).await;
    store
        .set_session_communication(opted_out, Some(false))
        .await
        .unwrap();
    let isolated = chat(&a_agents).await;
    a_agents
        .ask(agent::SetPeerScope {
            root: isolated,
            scope: PeerScope::Isolated,
            alias: None,
        })
        .await
        .unwrap();
    let advisor = runtime::SessionId::new();
    store
        .create_session_kind(
            advisor,
            "m",
            Some(visible),
            None,
            None,
            agent::SessionKind::Advisor,
        )
        .await
        .unwrap();
    let gone_owner = chat(&a_agents).await;
    a_agents
        .ask(OwnedElsewhereForTest(gone_owner, "f".repeat(64)))
        .await
        .unwrap();
    let offline_inbox = chat(&a_agents).await;
    store
        .set_contact_inbox(offline_inbox, "peer:y|export-2")
        .await
        .unwrap();
    a_agents
        .ask(OwnedElsewhereForTest(offline_inbox, "e".repeat(64)))
        .await
        .unwrap();
    full_access(&a, &a_alias, true).await;
    let hidden = [
        ("another contact's inbox", inbox),
        ("an opted-out root", opted_out),
        ("an Isolated root", isolated),
        ("an advisor", advisor),
        ("a row owned by a machine no longer in the mesh", gone_owner),
        ("an inbox on an offline machine", offline_inbox),
    ];

    let path = |s: runtime::SessionId| format!("/agents/session:{s}/rpc");
    let send = |s| {
        raw(
            &b,
            &a,
            hyper::Method::POST,
            path(s),
            Some(send_rpc("hi", None)),
        )
    };
    // Under the contact's rate bound (10 a second): every call here is one of B's.
    let pace = || tokio::time::sleep(Duration::from_millis(150));
    let unknown = runtime::SessionId::new();
    let want = send(unknown).await;
    let v: serde_json::Value = serde_json::from_slice(&want.2).unwrap();
    assert_eq!(
        (want.0, &v["error"]["code"], &v["error"]["message"]),
        (
            200,
            &serde_json::json!(-32030),
            &serde_json::json!("not owner")
        )
    );
    for (what, s) in hidden {
        pace().await;
        assert_eq!(send(s).await, want, "{what}");
    }
    // The owner side, as another machine of A's mesh carries it.
    for (what, s) in std::iter::once(("an unknown id", unknown)).chain(hidden) {
        pace().await;
        let r = a
            .ctx
            .forwarded(&b.eid(), &path(s), send_rpc("hi", None).to_string())
            .await;
        assert_eq!(r, Err(ForwardFailure::NotOwner), "{what}");
    }

    // `after` is a key, never looked up.
    let page = |after: runtime::SessionId| {
        raw(
            &b,
            &a,
            hyper::Method::GET,
            format!("/theuth/sessions?limit=50&after={after}"),
            None,
        )
    };
    pace().await;
    let want = page(runtime::SessionId::new()).await;
    assert_eq!(want.0, 200);
    assert!(String::from_utf8_lossy(&want.2).contains(&visible.to_string()));
    for (what, s) in hidden {
        pace().await;
        assert_eq!(page(s).await, want, "after {what}");
    }
    // A visible session answers otherwise (the table is not vacuous).
    pace().await;
    let mine = send(visible).await;
    pace().await;
    assert_ne!(mine, send(unknown).await);
}
