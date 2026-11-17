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
    // A's chat, never shared, in an Owner tree; an Isolated root (the default) stays hidden.
    let chat = session_of(a_agents.clone()).await;
    a_agents
        .ask(agent::SetPeerScope {
            root: chat,
            scope: PeerScope::Owner,
            alias: None,
        })
        .await
        .unwrap();
    let hidden = session_of(a_agents.clone()).await;
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
