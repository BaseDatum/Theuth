//! 191 P3 end to end on loopback: `a2a_forward`. n1 is a live engine with a contact C; n2 is
//! another machine of n1's mesh standing in for the owner (it answers `theuth/rpc/1` as told,
//! and records what it was asked). C's calls go to n1 as its own client sends them.

use std::collections::VecDeque;

use replica::Transport as _;

use super::*;

/// What the stand-in owner answers each `a2a_forward`, in turn (then the last one again).
#[derive(Clone)]
enum Answer {
    /// `{status, body}`.
    Router(u16, serde_json::Value),
    /// A JSON-RPC error of the forward call.
    Error(i32),
    /// Never answers (a frozen machine).
    Silent,
}

/// The stand-in owner: every `theuth/rpc/1` request is recorded; answered by `answers`.
fn owner(
    net: Arc<mesh::net::Net>,
    answers: Arc<parking_lot::Mutex<VecDeque<Answer>>>,
    seen: Arc<parking_lot::Mutex<Vec<serde_json::Value>>>,
) {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    tokio::spawn(async move {
        while let Ok((_, stream)) = net.accept(replica::ALPN_RPC).await {
            let (answers, seen) = (answers.clone(), seen.clone());
            tokio::spawn(async move {
                let (read, mut write) = tokio::io::split(stream);
                let mut line = String::new();
                tokio::io::BufReader::new(read)
                    .read_line(&mut line)
                    .await
                    .unwrap();
                seen.lock().push(serde_json::from_str(&line).unwrap());
                let answer = {
                    let mut a = answers.lock();
                    if a.len() > 1 {
                        a.pop_front()
                    } else {
                        a.front().cloned()
                    }
                };
                let reply = match answer {
                    Some(Answer::Router(status, body)) => serde_json::json!({"jsonrpc": "2.0",
                        "id": 1, "result": {"status": status, "body": body.to_string()}}),
                    Some(Answer::Error(code)) => serde_json::json!({"jsonrpc": "2.0", "id": 1,
                        "error": {"code": code, "message": "the owner's words"}}),
                    Some(Answer::Silent) | None => {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        return;
                    }
                };
                let mut out = reply.to_string();
                out.push('\n');
                write.write_all(out.as_bytes()).await.unwrap();
                write.flush().await.unwrap();
            });
        }
    });
}

fn send_request(text: &str) -> serde_json::Value {
    let mut message =
        a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text(text)]);
    message.message_id = format!("m-{text}");
    a2a_pb::protojson_conv::to_value(&a2a_types::SendMessageRequest {
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
    .unwrap()
}

/// 191 (D3): the receiving machine serves what it owns and carries the rest to the owner, once
/// resolved again after a `NOT_OWNER`, a second `NOT_OWNER` back to the contact; a stream is
/// refused there and never carried; an owner not connected, not ready or not answering within
/// the forward's deadline is "the chat's machine is offline", before the contact's own
/// deadline; an owner's 401 is the transient refusal. The owner side checks the caller itself,
/// never forwards again, and answers while the a2a pass is held.
#[tokio::test]
async fn a_call_for_a_session_owned_elsewhere_is_carried_to_its_owner() {
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n1, &c).await;
    // n2: another machine of n1's mesh, connected.
    let dir = tempfile::tempdir().unwrap();
    let n2 = node("n2", dir.path()).await;
    let mut events = n1.net.subscribe_peers();
    n1.net.add_peer(&n2.pairing_string().await).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    let n2_id = n2.id.0.clone();
    let answers = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    owner(n2.clone(), answers.clone(), seen.clone());

    // n1 publishes chat X to C; X's row says n2 owns it.
    let agents = n1.ctx.agents().unwrap();
    let x = agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions::default(),
            peer: None,
        })
        .await
        .unwrap()
        .session;
    publish(&n1, "xchat", ExportTarget::Chat { session: x }, "X").await;
    n1.ctx
        .update_contact(ContactUpdate {
            name: alias.clone(),
            patch: ContactPatch {
                exports: Some(vec!["xchat".into()]),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    let owned_by = |owner: String| {
        let agents = agents.clone();
        async move {
            agents
                .ask(agent::communication::network::OwnedElsewhereForTest(
                    x, owner,
                ))
                .await
                .unwrap();
        }
    };
    owned_by(n2_id.clone()).await;

    let to_n1 = |export: Option<&str>| reach::IrohTransport {
        net: c.net.clone(),
        eid: n1.eid(),
        hints: reach::Hints {
            addrs: n1.addrs(),
            relay: None,
        },
        export: export.map(str::to_owned),
        members: Vec::new(),
        answered: Default::default(),
    };
    let v1 = std::collections::HashMap::from([("a2a-version".to_owned(), vec!["1.0".to_owned()])]);
    let result = serde_json::json!({"jsonrpc": "2.0", "id": "x", "result": {"from": "n2"}});

    // Carried: C's `POST /rpc` goes to n2 as the export n1 resolved, the body as C sent it.
    answers
        .lock()
        .push_back(Answer::Router(200, result.clone()));
    let got = to_n1(None)
        .call(&v1, "SendMessage", send_request("one"))
        .await
        .unwrap();
    assert_eq!(got, serde_json::json!({"from": "n2"}));
    {
        let s = seen.lock();
        assert_eq!(s.len(), 1);
        let p = &s[0]["params"];
        assert_eq!(s[0]["method"], agent::communication::mesh::A2A_FORWARD);
        assert_eq!(p[0], c.eid());
        assert_eq!(p[1], "/agents/xchat/rpc", "never the bare /rpc");
        let body: serde_json::Value = serde_json::from_str(p[2].as_str().unwrap()).unwrap();
        assert_eq!(body["method"], "SendMessage");
        assert_eq!(body["params"], send_request("one"));
    }

    // NOT_OWNER twice: resolved again once, then the contact hears `not owner` (its client
    // tries its next member); no third resolve.
    seen.lock().clear();
    *answers.lock() = VecDeque::from([Answer::Error(agent::communication::mesh::NOT_OWNER)]);
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("two"))
        .await
        .unwrap_err();
    assert_eq!(e.code, agent::communication::network::NOT_OWNER_CODE);
    assert_eq!(seen.lock().len(), 2, "two forwards, never a third");

    // The owner does not list the caller yet (401 there): the transient refusal.
    seen.lock().clear();
    *answers.lock() = VecDeque::from([Answer::Router(
        401,
        serde_json::Value::String(String::new()),
    )]);
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("three"))
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            a2a_types::error_code::UNSUPPORTED_OPERATION,
            super::super::forward::NOT_ACCEPTED_THERE
        )
    );

    // An owner's answer over 2 MiB fails, never cut.
    *answers.lock() = VecDeque::from([Answer::Router(
        200,
        serde_json::Value::String("a".repeat(agent::communication::mesh::FORWARD_BODY_MAX)),
    )]);
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("big"))
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            a2a_types::error_code::UNSUPPORTED_OPERATION,
            "the answer is larger than 2 MiB"
        )
    );

    // An owner that is not `contacts_ready` (OWNER_UNREACHABLE): offline.
    *answers.lock() =
        VecDeque::from([Answer::Error(agent::communication::mesh::OWNER_UNREACHABLE)]);
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("four"))
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        agent::communication::ContactRefusal::OwnerOffline.to_string()
    );

    // A stream is refused at n1 and never carried.
    seen.lock().clear();
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendStreamingMessage", send_request("five"))
        .await
        .unwrap_err();
    assert_eq!(e.code, a2a_types::error_code::UNSUPPORTED_OPERATION);
    assert!(
        e.message.contains("streaming is not carried"),
        "{}",
        e.message
    );
    assert!(seen.lock().is_empty(), "never carried");

    // A task id naming n2 goes there (GetTask), without the chat's resolve.
    *answers.lock() = VecDeque::from([Answer::Router(200, result.clone())]);
    let task = format!("{}-{}", &n2_id[..16], uuid::Uuid::now_v7());
    to_n1(Some("xchat"))
        .call(&v1, "GetTask", serde_json::json!({"id": task}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(seen.lock()[0]["params"][2].as_str().unwrap())
            .unwrap()["method"],
        "GetTask"
    );
    // Only `GetTask` and `CancelTask` read the id: a `SendMessage` carrying a crafted
    // `params.id` naming n2 resolves its own session (owned here now), never forwarded by it.
    owned_by(String::new()).await;
    seen.lock().clear();
    let mut crafted = send_request("crafted");
    crafted["id"] = serde_json::Value::String(task.clone());
    let _ = to_n1(Some("xchat")).call(&v1, "SendMessage", crafted).await;
    assert!(
        seen.lock().is_empty(),
        "a SendMessage is never routed by an id"
    );
    owned_by(n2_id.clone()).await;
    // A well-formed prefix naming no machine of this mesh answers as an offline machine does.
    let stray = format!("{}-{}", "f".repeat(16), uuid::Uuid::now_v7());
    let e = to_n1(Some("xchat"))
        .call(&v1, "GetTask", serde_json::json!({"id": stray}))
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        agent::communication::ContactRefusal::OwnerOffline.to_string()
    );
    assert!(seen.lock().is_empty(), "never carried");

    // An owner that never answers: offline at the forward's deadline, before C's own 30 s.
    *answers.lock() = VecDeque::from([Answer::Silent]);
    let at = std::time::Instant::now();
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("six"))
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        agent::communication::ContactRefusal::OwnerOffline.to_string()
    );
    let took = at.elapsed();
    assert!(
        took >= agent::communication::mesh::FORWARD_TIMEOUT && took < reach::CALL_TIMEOUT,
        "{took:?}"
    );

    // A row naming no machine of this mesh: plain `not owner` (C's client moves on), never
    // carried.
    seen.lock().clear();
    owned_by(iroh::SecretKey::generate().public().to_string()).await;
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("seven"))
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            agent::communication::network::NOT_OWNER_CODE,
            agent::communication::network::NOT_OWNER_TEXT
        )
    );
    // A machine of the mesh that is not connected (n2 stopped): offline at once.
    owned_by(n2_id.clone()).await;
    n2.shutdown().await;
    until("n1 sees n2 gone", || {
        (!replica::Transport::peers(n1.net.as_ref()).contains(&replica::PeerId(n2_id.clone())))
            .then_some(())
    })
    .await;
    let at = std::time::Instant::now();
    let e = to_n1(Some("xchat"))
        .call(&v1, "SendMessage", send_request("seven-b"))
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        agent::communication::ContactRefusal::OwnerOffline.to_string()
    );
    assert!(at.elapsed() < Duration::from_secs(2), "{:?}", at.elapsed());
    assert!(seen.lock().is_empty());

    // The owner side, on n1 (X owned by n2 again): a forward is never forwarded again.
    owned_by(n2_id.clone()).await;
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage",
        "params": send_request("eight")})
    .to_string();
    let path = "/agents/xchat/rpc";
    assert_eq!(
        n1.ctx.forwarded(&c.eid(), path, body.clone()).await,
        Err(agent::communication::mesh::ForwardFailure::NotOwner)
    );
    assert!(seen.lock().is_empty(), "no second hop");
    // A caller the owner does not list (a stranger inside an open window too): 401.
    n1.ctx.open_window().await.unwrap();
    let stranger = iroh::SecretKey::generate().public().to_string();
    assert_eq!(n1.ctx.gate(&stranger), IrohGate::Pair);
    assert_eq!(
        n1.ctx
            .forwarded(&stranger, path, body.clone())
            .await
            .map(|a| a.status),
        Ok(401)
    );
    n1.ctx.close_window();
    // Not carried: a stream, the card.
    assert!(matches!(
        n1.ctx
            .forwarded(
                &c.eid(),
                path,
                body.replace("SendMessage", "SendStreamingMessage")
            )
            .await,
        Err(agent::communication::mesh::ForwardFailure::Refused(_))
    ));
    assert!(matches!(
        n1.ctx
            .forwarded(&c.eid(), super::super::offers::CARD_PATH, body.clone())
            .await,
        Err(agent::communication::mesh::ForwardFailure::Refused(_))
    ));
    // Not ready: OWNER_UNREACHABLE.
    n1.ctx
        .ready
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        n1.ctx.forwarded(&c.eid(), path, body.clone()).await,
        Err(agent::communication::mesh::ForwardFailure::Offline(_))
    ));
    n1.ctx
        .ready
        .store(true, std::sync::atomic::Ordering::SeqCst);
    // Owned here: served in n1's own router, while the a2a pass is held.
    owned_by(String::new()).await;
    let pass = n1.ctx.reload_lock.lock().await;
    let served = tokio::time::timeout(
        Duration::from_secs(10),
        n1.ctx.forwarded(&c.eid(), path, body.clone()),
    )
    .await
    .expect("answered while the pass is held")
    .unwrap();
    drop(pass);
    assert_eq!(served.status, 200);
    let answer: serde_json::Value = serde_json::from_str(&served.body).unwrap();
    assert!(answer.get("result").is_some(), "{answer}");
    // The same messageId again (the contact's retry after the receiving machine's deadline,
    // while the first one was still admitted here): the task the first one made, once.
    let again = n1
        .ctx
        .forwarded(&c.eid(), path, body.clone())
        .await
        .unwrap();
    let again: serde_json::Value = serde_json::from_str(&again.body).unwrap();
    let task_of = |v: &serde_json::Value| v.pointer("/result/task/id").cloned();
    assert!(task_of(&answer).is_some(), "{answer}");
    assert_eq!(task_of(&again), task_of(&answer), "{again}");
    // A caller the owner blocked: 401 there (the transient refusal on the receiving machine).
    n1.ctx
        .update_contact(ContactUpdate {
            name: alias.clone(),
            patch: ContactPatch {
                blocked: Some(true),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    assert_eq!(
        n1.ctx
            .forwarded(&c.eid(), path, body.clone())
            .await
            .map(|a| a.status),
        Ok(401)
    );
}

/// 191 (D4): re-sharing an agent export whose home left the mesh moves its home here, and it
/// is no longer paused; a home that is a machine of the mesh stays where it is.
#[tokio::test]
async fn re_sharing_an_agent_export_whose_home_left_rehomes_it_here() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let dir = tempfile::tempdir().unwrap();
    let n2 = node("n2", dir.path()).await;
    let mut events = n1.net.subscribe_peers();
    n1.net.add_peer(&n2.pairing_string().await).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    let agent = ExportTarget::Agent {
        agent: "theuth".into(),
    };
    publish(&n1, "eagent", agent.clone(), "E").await;
    let row = || async {
        let r = n1.ctx.records().await.unwrap();
        let x = r
            .exports
            .into_iter()
            .find(|x| x.name.as_deref() == Some("eagent"))
            .unwrap();
        (x.id, x.home, r.me)
    };
    let (id, home, me) = row().await;
    assert_eq!(
        home.as_deref(),
        Some(me.as_str()),
        "a new export's home is here"
    );
    let set_home = |h: String| {
        n1.ctx.commit_records(vec![RecordWrite::new(
            RecordTable::Exports,
            id.clone(),
            vec![("home", Cell::Text(h))],
        )])
    };
    let paused = || async {
        n1.ctx
            .exports()
            .await
            .into_iter()
            .find(|e| e.name == "eagent")
            .unwrap()
            .paused
    };
    // A home that is a machine of the mesh stays.
    set_home(n2.id.0.clone()).await.unwrap();
    publish(&n1, "eagent", agent.clone(), "E").await;
    assert_eq!(row().await.1, Some(n2.id.0.clone()));
    assert_eq!(paused().await, None);
    // A home that left: paused; a re-share moves it here.
    set_home(iroh::SecretKey::generate().public().to_string())
        .await
        .unwrap();
    assert_eq!(
        paused().await.as_deref(),
        Some(crate::contacts::PAUSED_HOME_LEFT)
    );
    publish(&n1, "eagent", agent, "E").await;
    assert_eq!(row().await.1, Some(me));
    assert_eq!(paused().await, None);
}
