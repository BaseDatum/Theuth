//! 183 Phase 3 end to end on loopback: two (or three) engines in one process, each with its
//! own iroh endpoint, `peers.yaml` and (where the test needs it) a live agent; mDNS is the
//! in-process fake. Nothing touches the real LAN.

use std::sync::Arc;
use std::time::Duration;

use roster::attention::Source as _;

use super::discovery::fake::Lan;
use super::*;
use crate::Ctx;
use crate::config::A2aConfig;
use crate::server::IrohGate;

struct Engine {
    ctx: Arc<Ctx>,
    net: Arc<mesh::net::Net>,
    _dir: tempfile::TempDir,
}

impl Engine {
    fn eid(&self) -> String {
        self.net.id.0.clone()
    }

    fn addrs(&self) -> Vec<std::net::SocketAddr> {
        self.net
            .a2a_reach()
            .0
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect()
    }
}

pub(crate) async fn node(name: &str, dir: &std::path::Path) -> Arc<mesh::net::Net> {
    let secret = iroh::SecretKey::generate();
    let store = mesh::store::Store::open(
        &dir.join("mesh.sqlite"),
        mesh::EngineId(secret.public().to_string()),
    )
    .await
    .unwrap();
    mesh::net::Net::bind(
        mesh::net::BindOpts {
            secret,
            relay: String::new(),
            discovery: false,
            bind_addr: Some("127.0.0.1:0".parse().unwrap()),
            storm_frames_per_sec: 50,
            first_seen_ms: 1_000,
            vault: None,
            mdns: None,
        },
        store,
        name.into(),
        mesh::HubPref::Eligible,
    )
    .await
    .unwrap()
}

/// One engine; `live` gives it an agent (then `peers.yaml` reloads, ceremonies pin). 191: its
/// own `contacts_ready`, shared by its agent; a live engine's first reload sets it, one with
/// no agent is taken as past its first pass. The mesh latch is done.
async fn engine(name: &str, lan: &Lan, live: bool, cfg: A2aConfig) -> Engine {
    let dir = tempfile::tempdir().unwrap();
    let net = node(name, dir.path()).await;
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(!live));
    let agents = if live {
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        Some(agent::spawn_engine_agents(rt, ready.clone()).await)
    } else {
        None
    };
    let ctx = crate::test_ctx(
        dir.path(),
        net.clone(),
        Arc::new(lan.clone()),
        cfg,
        agents,
        ready,
        Arc::new(|| true),
    )
    .await;
    if live {
        ctx.reload().await.unwrap();
    }
    Engine {
        ctx,
        net,
        _dir: dir,
    }
}

fn cfg(window_secs: u64) -> A2aConfig {
    A2aConfig {
        contact_window_secs: window_secs,
        ..A2aConfig::default()
    }
}

async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..200 {
        if let Some(t) = f() {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn ceremony_of(e: &Engine) -> Option<mesh::PairCeremony> {
    e.ctx
        .contacts
        .lock()
        .ceremonies
        .values()
        .next()
        .map(|r| r.view.clone())
}

fn failed(e: &Engine) -> u32 {
    e.ctx
        .contacts
        .lock()
        .window
        .as_ref()
        .map_or(0, |w| w.failed)
}

/// Put a LAN sighting of endpoint `eid` into `a`'s open window (what a ticketless request
/// needs).
fn sight(a: &Engine, eid: &str) {
    let mut st = a.ctx.contacts.lock();
    let w = st.window.as_mut().expect("an open window");
    w.nearby.insert(
        0,
        discovery::Sighting {
            service: "_a2a._tcp".into(),
            instance: "b".into(),
            addrs: vec!["127.0.0.1:5353".parse().unwrap()],
            txt: [("eid".to_owned(), eid[..16].to_owned())].into(),
            gone: false,
        },
    );
}

/// W-20: nothing is advertised or browsed outside a window; inside one the draft-zhao TXT
/// and the theuth hints go out under the first type, sightings of the others are listed, a
/// Cancel withdraws it at once and the window closes itself after `contact_window_secs`.
#[tokio::test]
async fn the_advert_and_the_browse_live_only_while_a_window_is_open() {
    let lan = Lan::default();
    let a = engine("a", &lan, false, cfg(2)).await;
    let b = engine("b", &lan, false, cfg(600)).await;
    assert!(lan.adverts().is_empty(), "no advert without a window");
    assert!(a.ctx.nearby().is_empty());
    assert_eq!(
        a.ctx.gate(&b.eid()),
        IrohGate::Refuse,
        "a stranger outside a window"
    );

    let w = a.ctx.open_window().await.unwrap();
    let t = ceremony::decode_ticket(&w.ticket).unwrap();
    assert_eq!((t.eid.as_str(), t.exp), (a.eid().as_str(), w.expires_ms));
    assert!(
        !t.addrs.is_empty(),
        "the ticket carries the direct addresses"
    );
    assert_eq!(
        a.ctx.open_window().await.unwrap(),
        w,
        "one window at a time"
    );
    let ad = &lan.adverts()[0];
    assert_eq!(ad.service, "_a2a._tcp");
    let txt: std::collections::BTreeMap<_, _> = ad.txt.iter().cloned().collect();
    assert_eq!(txt["card"], crate::config::CARD_SUFFIX);
    assert_eq!(txt["proto"], BINDING_IROH);
    assert_eq!(txt["v"], "1.0");
    assert_eq!(txt["pair"], "1");
    assert_eq!(txt["eid"], a.eid()[..16]);
    assert!(!txt.contains_key("tls"));
    assert_eq!(
        a.ctx.gate(&b.eid()),
        IrohGate::Pair,
        "a stranger inside a window"
    );

    b.ctx.open_window().await.unwrap();
    assert_eq!(lan.adverts().len(), 2);
    let seen = a.ctx.nearby();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].eid.as_deref(), Some(&b.eid()[..16]));
    assert_eq!(seen[0].service, "_a2a._tcp");
    assert!(
        seen[0]
            .card
            .as_deref()
            .unwrap()
            .ends_with(crate::config::CARD_SUFFIX)
    );
    assert!(
        !seen[0].verified,
        "a sighting is a hint until its card is fetched"
    );
    assert_eq!(b.ctx.nearby().len(), 1);

    b.ctx.close_window();
    assert_eq!(lan.adverts().len(), 1, "Cancel withdraws the advert");
    assert!(a.ctx.nearby().is_empty(), "the other side sees it go");
    assert!(b.ctx.nearby().is_empty());

    tokio::time::sleep(Duration::from_millis(2_300)).await;
    assert!(lan.adverts().is_empty(), "the window closed itself");
    assert!(!a.ctx.window_open());
    assert_eq!(a.ctx.gate(&b.eid()), IrohGate::Refuse);
}

/// W-23 by invite ticket: both sides show the same code, both Accepts pin each other (TOFU)
/// in `peers.yaml`, the inviter's window closes, and the gate then admits the contact.
#[tokio::test]
async fn a_ticket_ceremony_pins_both_sides_after_both_accept() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let vb = b
        .ctx
        .pair(ContactTarget::Ticket {
            ticket: w.ticket.clone(),
        })
        .await
        .unwrap();
    let va = until("the inviter's code", || ceremony_of(&a)).await;
    assert_eq!(va.code, vb.code, "both see one code");
    assert_eq!(vb.code.len(), 7);
    assert_eq!(vb.code.as_bytes()[3], b' ');
    assert_eq!(
        (va.peer.0.as_str(), vb.peer.0.as_str()),
        (b.eid().as_str(), a.eid().as_str())
    );
    assert_eq!(va.state, "waiting");

    // One Accept is not enough: nothing is pinned yet.
    a.ctx.answer(va.id, true).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(a.ctx.shared.read().staged.contacts.is_empty());
    b.ctx.answer(vb.id, true).unwrap();
    until("both pinned", || {
        (a.ctx.shared.read().staged.contacts.len() == 1
            && b.ctx.shared.read().staged.contacts.len() == 1)
            .then_some(())
    })
    .await;
    let (alias, facts) = {
        let live = a.ctx.shared.read().clone();
        let (k, v) = live.staged.contacts.iter().next().unwrap();
        (k.clone(), v.clone())
    };
    assert_eq!(facts.eid, b.eid());
    assert!(facts.paired_ms > 0);
    assert!(!a.ctx.window_open(), "success closes the inviter's window");
    assert!(matches!(a.ctx.gate(&b.eid()), IrohGate::Contact { alias: x, .. } if x == alias));
    assert!(matches!(b.ctx.gate(&a.eid()), IrohGate::Contact { .. }));
    // 191 (D2): the pin is a replicated record: the other side's endpoint is its anchor.
    let records = a.ctx.records().await.unwrap();
    assert!(
        records
            .endpoints
            .iter()
            .any(|e| e.eid.as_deref() == Some(b.eid().as_str()) && e.barred_ms.is_none()),
        "{records:?}"
    );
    assert_eq!(records.contacts[0].alias.as_deref(), Some(alias.as_str()));
}

/// W-23: a Reject ends it on both sides with nothing pinned; the ticket is single use; a
/// ceremony nobody answers expires; the third failure closes the window.
#[tokio::test]
async fn reject_reuse_and_expiry_are_refused_and_three_failures_close_the_window() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let ticket = ContactTarget::Ticket {
        ticket: w.ticket.clone(),
    };
    let vb = b.ctx.pair(ticket.clone()).await.unwrap();
    let va = until("the inviter's code", || ceremony_of(&a)).await;
    b.ctx.answer(vb.id, true).unwrap();
    a.ctx.answer(va.id, false).unwrap();
    until("both ended", || {
        (ceremony_of(&a).is_none() && ceremony_of(&b).is_none()).then_some(())
    })
    .await;
    assert!(a.ctx.shared.read().staged.contacts.is_empty());
    assert!(b.ctx.shared.read().staged.contacts.is_empty());
    assert_eq!(failed(&a), 1);

    let e = b.ctx.pair(ticket).await.unwrap_err();
    assert!(e.contains("used"), "single-use ticket: {e}");

    a.ctx.contacts.lock().ceremony_secs = Some(1);
    sight(&a, &b.eid());
    let dial = || {
        ceremony::start(
            b.ctx.clone(),
            a.eid(),
            reach::Hints {
                addrs: a.addrs(),
                relay: None,
            },
            None,
        )
    };
    for n in 2..=FAILED_MAX {
        // Nearby-style dials into the same open window (no ticket): nobody answers.
        let vb = dial().await.unwrap();
        assert_eq!(vb.state, "waiting");
        until("the inviter's ceremony", || ceremony_of(&a)).await;
        until("the ceremony to expire", || {
            ceremony_of(&a).is_none().then_some(())
        })
        .await;
        assert_eq!(failed(&a), n);
    }
    // Z3-R2: the third failure refuses that endpoint, it does not close the window.
    let e = dial().await.unwrap_err();
    assert!(e.contains("too many failed"), "{e}");
    assert!(a.ctx.window_open(), "one endpoint's failures leave it open");
    // The global ceiling does close it.
    for i in 0..FAILED_GLOBAL_MAX {
        a.ctx
            .ceremony_failed(&format!("stranger-{i}"), Role::Inviter);
    }
    assert!(
        !a.ctx.window_open(),
        "{FAILED_GLOBAL_MAX} failures close the window"
    );
    let expired = ceremony::decode_ticket(&w.ticket).map(|mut t| {
        t.exp = 1;
        ceremony::ticket_target(&t, &[], true)
    });
    assert!(matches!(expired, Ok(Err(e)) if e.contains("expired")));
}

/// W-23 commit-reveal: a dialer whose reveal does not open its commitment, or whose card is
/// not signed by the endpoint QUIC proved, is rejected before any code is shown.
#[tokio::test]
async fn a_broken_commitment_or_a_forged_card_is_rejected() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let dir = tempfile::tempdir().unwrap();
    let c = node("c", dir.path()).await;
    let c_card = |sign_as: Option<iroh::SecretKey>| {
        let facts = CardFacts {
            name: "mallory".into(),
            description: String::new(),
            machine: "c".into(),
            os_name: "Linux".into(),
            endpoint: c.id.0.clone(),
            public_url: None,
            skills: Vec::new(),
        };
        match sign_as {
            None => signed_card(&facts, &|m: &[u8]| c.sign(m)).unwrap(),
            Some(k) => signed_card(&facts, &|m: &[u8]| k.sign(m).to_bytes().to_vec()).unwrap(),
        }
    };
    let a_eid = a.eid();
    let addrs = a.addrs();
    let attempt = |reveal: String, card: AgentCard| {
        let c = c.clone();
        let (a_eid, addrs) = (a_eid.clone(), addrs.clone());
        async move {
            let mut s = c.a2a_stream(&a_eid, &addrs, None).await.unwrap();
            ceremony::write_frame(
                &mut s,
                PairFrame::Request {
                    name: "mallory".into(),
                    machine: "c".into(),
                    commit: ceremony::commit("the nonce I committed to"),
                    ticket: None,
                },
            )
            .await
            .unwrap();
            let line = reach::read_line(&mut s).await.unwrap();
            let challenge: PairLine = serde_json::from_slice(&line).unwrap();
            assert!(matches!(challenge.pair, PairFrame::Challenge { .. }));
            ceremony::write_frame(
                &mut s,
                PairFrame::Reveal {
                    nonce: reveal,
                    card: Box::new(card),
                },
            )
            .await
            .unwrap();
            let line = reach::read_line(&mut s).await.unwrap();
            match serde_json::from_slice::<PairLine>(&line).unwrap().pair {
                PairFrame::Reject { reason } => reason,
                other => panic!("expected a reject, got {other:?}"),
            }
        }
    };
    a.ctx.open_window().await.unwrap();
    sight(&a, &c.id.0);
    let why = attempt("another nonce".into(), c_card(None)).await;
    assert!(why.contains("commitment"), "{why}");
    let why = attempt(
        "the nonce I committed to".into(),
        c_card(Some(iroh::SecretKey::generate())),
    )
    .await;
    assert!(why.contains("does not verify"), "{why}");
    assert!(ceremony_of(&a).is_none(), "no code was ever shown");
    assert_eq!(failed(&a), 2);
}

/// §7.2, W-27, §7.7: once paired and granted an export, a contact reaches it over
/// `theuth/a2a/1` with the SDK client: its first task waits in `AUTH_REQUIRED`; Allow once
/// lets it through; Always lets the next straight through; Block refuses it at the gate.
#[tokio::test]
async fn a_contact_reaches_its_export_first_contact_waits_and_block_refuses_at_the_gate() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let vb = b
        .ctx
        .pair(ContactTarget::Ticket { ticket: w.ticket })
        .await
        .unwrap();
    let va = until("the inviter's code", || ceremony_of(&a)).await;
    a.ctx.answer(va.id, true).unwrap();
    b.ctx.answer(vb.id, true).unwrap();
    let alias = until("A pinned B", || {
        a.ctx.shared.read().staged.contacts.keys().next().cloned()
    })
    .await;
    // A publishes its default agent as `research`, `zeta` and `zulu` and grants them to B.
    let names = ["research", "zeta", "zulu"];
    for name in names {
        publish(
            &a,
            name,
            ExportTarget::Agent {
                agent: "theuth".into(),
            },
            "Answers research questions",
        )
        .await;
    }
    grant_ask(&a, &alias, &names).await;

    let client_for = |export: Option<&str>| {
        a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
            net: b.net.clone(),
            eid: a.eid(),
            members: Vec::new(),
            answered: Default::default(),
            hints: reach::Hints {
                addrs: a.addrs(),
                relay: None,
            },
            export: export.map(str::to_owned),
        }) as Box<dyn a2a_client::Transport>)
    };
    // `peer:<contact>` posts to `/rpc`: the first export granted (name order).
    let client = client_for(None);
    let send = |id: &str| {
        let mut m =
            a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("hi")]);
        m.message_id = id.into();
        a2a_types::SendMessageRequest {
            message: m,
            configuration: Some(a2a_types::SendMessageConfiguration {
                accepted_output_modes: Some(vec!["text/plain".into()]),
                task_push_notification_config: None,
                history_length: Some(0),
                return_immediately: Some(true),
            }),
            metadata: None,
            tenant: None,
        }
    };
    let task = |r: a2a_types::SendMessageResponse| match r {
        a2a_types::SendMessageResponse::Task(t) => t,
        other => panic!("expected a task, got {other:?}"),
    };

    let first = task(client.send_message(&send("m-1")).await.unwrap());
    assert_eq!(first.status.state, a2a_types::TaskState::AuthRequired);
    let why = first
        .status
        .message
        .as_ref()
        .and_then(|m| m.text())
        .unwrap();
    assert!(
        why.contains("has not allowed messages to research yet"),
        "{why}"
    );
    let held = FirstContactItems(Arc::downgrade(&a.ctx)).items().await;
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, format!("contact_first:{alias}:research"));
    assert_eq!(held[0].kind, roster::attention::AttentionKind::ContactFirst);

    // Allow once: this task goes on; the next one would ask again.
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap();
    assert!(
        FirstContactItems(Arc::downgrade(&a.ctx))
            .items()
            .await
            .is_empty()
    );
    let get = a2a_types::GetTaskRequest {
        id: first.id.clone(),
        history_length: Some(0),
        tenant: None,
    };
    let now = client.get_task(&get).await.unwrap();
    assert_ne!(now.status.state, a2a_types::TaskState::AuthRequired);

    // Always: the next task is not held.
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer: FirstContact::Allow,
        })
        .await
        .unwrap();
    let second = task(client.send_message(&send("m-2")).await.unwrap());
    assert_ne!(second.status.state, a2a_types::TaskState::AuthRequired);
    assert!(
        FirstContactItems(Arc::downgrade(&a.ctx))
            .items()
            .await
            .is_empty()
    );

    // 183 §7.7: `peer:<contact>/zeta` reaches that export, not the first one; its own
    // first contact holds it, and its task is followed on the same export.
    let zeta = client_for(Some("zeta"));
    let z = task(zeta.send_message(&send("m-z")).await.unwrap());
    assert_eq!(z.status.state, a2a_types::TaskState::AuthRequired);
    let why = z.status.message.as_ref().and_then(|m| m.text()).unwrap();
    assert!(
        why.contains("has not allowed messages to zeta yet"),
        "{why}"
    );
    let got = zeta
        .get_task(&a2a_types::GetTaskRequest {
            id: z.id.clone(),
            history_length: Some(0),
            tenant: None,
        })
        .await
        .unwrap();
    assert_eq!(got.id, z.id);
    assert!(
        client_for(Some("nope"))
            .send_message(&send("m-n"))
            .await
            .is_err(),
        "an export not granted is not found"
    );
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "zeta".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap();

    // Settings (no task waiting): the answer is only the export's standing default.
    let standing = || {
        a.ctx.shared.read().staged.contacts[&alias]
            .first_contact
            .get("research")
            .cloned()
    };
    let settings = |answer| {
        a.ctx.first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer,
        })
    };
    settings(FirstContact::Ask).await.unwrap();
    assert_eq!(
        standing(),
        Some(FirstContact::Ask),
        "an explicit Ask first is stored"
    );
    settings(FirstContact::Block).await.unwrap();
    assert_eq!(standing(), Some(FirstContact::Block));
    assert!(
        matches!(a.ctx.gate(&b.eid()), IrohGate::Contact { .. }),
        "a per-export block leaves the contact itself alone"
    );
    let refused = client.send_message(&send("m-3")).await.unwrap_err();
    assert!(
        refused.message.contains("not allowed to reach research"),
        "{}",
        refused.message
    );
    settings(FirstContact::Ask).await.unwrap();
    assert_eq!(standing(), Some(FirstContact::Ask));

    // Block on the Attention item (a task waits, here for `zulu`, whose inbox runs nothing
    // else): the contact is blocked and refused at the gate from now on.
    let zulu = client_for(Some("zulu"));
    let waiting = task(zulu.send_message(&send("m-4")).await.unwrap());
    assert_eq!(waiting.status.state, a2a_types::TaskState::AuthRequired);
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "zulu".into(),
            answer: FirstContact::Block,
        })
        .await
        .unwrap();
    assert_eq!(a.ctx.gate(&b.eid()), IrohGate::Refuse);
    assert!(
        client.send_message(&send("m-5")).await.is_err(),
        "a blocked contact's request never reaches the export"
    );
}

/// Records every [`crate::A2aChanged`] it hears.
#[derive(kameo::Actor)]
struct Changes(Arc<parking_lot::Mutex<usize>>);

impl kameo::message::Message<crate::A2aChanged> for Changes {
    type Reply = ();

    async fn handle(&mut self, _: crate::A2aChanged, _: &mut Context<Self, Self::Reply>) {
        *self.0.lock() += 1;
    }
}

/// 183 W-41, §12.10: archiving or unarchiving an exported chat fires `A2aChanged` on the
/// engine holding the export (its row's Paused changed); another chat fires nothing.
#[tokio::test]
async fn archiving_an_exported_chat_fires_a2a_changed() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let agents = a.ctx.agents().unwrap();
    let root = || {
        agents.ask(agent::NewRootSession {
            cwd: None,
            scope: agent::PeerScope::Owner,
        })
    };
    let (chat, other) = (root().await.unwrap(), root().await.unwrap());
    publish(&a, "ops-chat", ExportTarget::Chat { session: chat }, "").await;
    let heard = Arc::new(parking_lot::Mutex::new(0));
    let rec = kameo::actor::Spawn::spawn(Changes(heard.clone()));
    a.ctx.rt.subscribe::<Changes, crate::A2aChanged>(&rec).await;

    assert!(!a.ctx.chat_archived(other).await, "not exported here");
    assert!(a.ctx.chat_archived(chat).await, "archived");
    assert!(a.ctx.chat_archived(chat).await, "unarchived");
    until("two A2aChanged", || (*heard.lock() >= 2).then_some(())).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*heard.lock(), 2, "the other chat fired nothing");
}

/// 183 §7.2: either side of a live `theuth/a2a/1` connection reaches the other. B dialed A
/// for the ceremony; A (no address for B: discovery and relay are off) reaches B's export
/// on that same connection, which B now serves too.
#[tokio::test]
async fn the_side_that_dialed_is_reached_on_its_own_connection() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let vb = b
        .ctx
        .pair(ContactTarget::Ticket { ticket: w.ticket })
        .await
        .unwrap();
    let va = until("the inviter's code", || ceremony_of(&a)).await;
    a.ctx.answer(va.id, true).unwrap();
    b.ctx.answer(vb.id, true).unwrap();
    let alias_of_a = until("B pinned A", || {
        b.ctx.shared.read().staged.contacts.keys().next().cloned()
    })
    .await;
    publish(
        &b,
        "research",
        ExportTarget::Agent {
            agent: "theuth".into(),
        },
        "",
    )
    .await;
    grant_ask(&b, &alias_of_a, &["research"]).await;
    let client = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
        net: a.net.clone(),
        eid: b.eid(),
        members: Vec::new(),
        answered: Default::default(),
        hints: reach::Hints::default(),
        export: None,
    }) as Box<dyn a2a_client::Transport>);
    let mut m = a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("hi")]);
    m.message_id = "m-1".into();
    let sent = tokio::time::timeout(
        Duration::from_secs(10),
        client.send_message(&a2a_types::SendMessageRequest {
            message: m,
            configuration: Some(a2a_types::SendMessageConfiguration {
                accepted_output_modes: Some(vec!["text/plain".into()]),
                task_push_notification_config: None,
                history_length: Some(0),
                return_immediately: Some(true),
            }),
            metadata: None,
            tenant: None,
        }),
    )
    .await
    .expect("B answers on the connection it dialed")
    .unwrap();
    match sent {
        a2a_types::SendMessageResponse::Task(t) => {
            assert_eq!(t.status.state, a2a_types::TaskState::AuthRequired)
        }
        other => panic!("expected a task, got {other:?}"),
    }
}

/// 183 §7.2: a contact's direct addresses from its live connection are written to
/// `peers.yaml` (only when they change), staged again by a reload, and handed to the dial as
/// hints: a LAN-only pair (no discovery, no relay) still finds each other after a restart.
#[tokio::test]
async fn a_contacts_last_direct_addresses_are_kept_and_offered_to_the_dial() {
    use mesh::net::ContactAlpn as _;
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let vb = b
        .ctx
        .pair(ContactTarget::Ticket { ticket: w.ticket })
        .await
        .unwrap();
    let va = until("the inviter's code", || ceremony_of(&a)).await;
    a.ctx.answer(va.id, true).unwrap();
    b.ctx.answer(vb.id, true).unwrap();
    let alias_of_b = until("A pinned B", || {
        a.ctx.shared.read().staged.contacts.keys().next().cloned()
    })
    .await;
    let gate = reach::Gate(Arc::downgrade(&a.ctx));
    assert!(gate.hints(&b.eid()).is_empty(), "nothing stored before");

    a.ctx.remember_addrs().await;
    let stored = a.ctx.shared.read().staged.contacts[&alias_of_b]
        .addrs
        .clone();
    assert!(
        !stored.is_empty() && stored.iter().all(|s| b.addrs().contains(s)),
        "B's own direct addresses, from the live connection: {stored:?} vs {:?}",
        b.addrs()
    );
    // 191 (D2): this machine's own `contact_addrs`, never replicated.
    let kept = a.ctx.records().await.unwrap().addrs[&b.eid()].clone();
    let want: Vec<String> = stored.iter().map(ToString::to_string).collect();
    assert_eq!(kept, want);
    assert_eq!(gate.hints(&b.eid()), stored, "the dial gets them");

    // Unchanged: no rewrite.
    let before = a.ctx.state.lock().addr_written[&alias_of_b];
    tokio::time::sleep(Duration::from_millis(20)).await;
    a.ctx.remember_addrs().await;
    assert_eq!(a.ctx.state.lock().addr_written[&alias_of_b], before);
}

/// Z3-R2: the one ceremony slot is taken at `admit_request` (a second handshake is refused
/// until the first fails or ends), a request without a ticket needs a LAN sighting of its
/// endpoint, and an endpoint's own failures refuse only that endpoint.
#[tokio::test]
async fn one_handshake_at_a_time_failures_count_per_endpoint_and_ticketless_needs_a_sighting() {
    let lan = Lan::default();
    let a = engine("a", &lan, false, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let nonce = ceremony::decode_ticket(&w.ticket).unwrap().nonce;
    let (x, y) = ("aaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbb");

    let e = a.ctx.admit_request(x, None).unwrap_err();
    assert!(e.contains("Nearby"), "no sighting, no ticket: {e}");
    sight(&a, x);
    sight(&a, y);
    a.ctx.admit_request(x, None).unwrap();
    let e = a.ctx.admit_request(y, None).unwrap_err();
    assert!(e.contains("another"), "the slot is taken: {e}");
    let e = a.ctx.admit_request(y, Some(&nonce)).unwrap_err();
    assert!(
        e.contains("another"),
        "the slot is taken, the ticket stays unspent: {e}"
    );
    a.ctx.ceremony_failed(x, Role::Inviter);
    a.ctx.admit_request(y, Some(&nonce)).unwrap();
    a.ctx.ceremony_failed(y, Role::Inviter);

    for _ in 1..FAILED_MAX {
        a.ctx.admit_request(x, None).unwrap();
        a.ctx.ceremony_failed(x, Role::Inviter);
    }
    let e = a.ctx.admit_request(x, None).unwrap_err();
    assert!(e.contains("too many failed"), "x is refused: {e}");
    a.ctx.admit_request(y, None).unwrap();
    assert!(
        a.ctx.window_open(),
        "a stranger's failures do not close it for everyone"
    );
}

/// Z3-R2 (SSRF): a card URL on a private, link-local or loopback address is refused unless a
/// LAN sighting carried it; a Nearby TXT card URL keeps its path but never its host; a
/// ticket's private addresses are not dialed.
#[tokio::test]
async fn private_card_urls_and_ticket_addresses_are_refused_without_a_sighting() {
    for url in [
        "http://10.1.2.3/",
        "http://127.0.0.1:9/",
        "http://169.254.169.254/latest/",
        "http://[::1]:9/",
        "http://[fd00::1]/",
    ] {
        let e = fetch_card(url, &[], false).await.unwrap_err();
        assert!(e.contains("private address"), "{url}: {e}");
    }
    // A sighting's address passes the check (the fetch itself then fails: nothing listens).
    let lan: Vec<std::net::IpAddr> = vec!["127.0.0.1".parse().unwrap()];
    let e = fetch_card("http://127.0.0.1:1/", &lan, false)
        .await
        .unwrap_err();
    assert!(e.starts_with("card fetch"), "{e}");

    let s = discovery::Sighting {
        service: "_a2a._tcp".into(),
        instance: "evil".into(),
        addrs: vec!["192.168.1.9:8000".parse().unwrap()],
        txt: [(
            "card".to_owned(),
            "http://169.254.169.254/.well-known/agent-card.json".to_owned(),
        )]
        .into(),
        gone: false,
    };
    assert_eq!(
        discovery::nearby(&s).card.as_deref(),
        Some("http://192.168.1.9:8000/.well-known/agent-card.json"),
        "the sighting's own address, the TXT's path"
    );

    let a = engine("a", &Lan::default(), false, cfg(600)).await;
    let w = a.ctx.open_window().await.unwrap();
    let t = ceremony::decode_ticket(&w.ticket).unwrap();
    assert!(!t.addrs.is_empty());
    assert!(
        ceremony::ticket_target(&t, &[], false)
            .unwrap()
            .addrs
            .is_empty()
    );
    assert!(
        !ceremony::ticket_target(&t, &[], true)
            .unwrap()
            .addrs
            .is_empty()
    );
}

/// Z3-R2: card names and machines are cut to 64 characters wherever they are kept or shown.
#[tokio::test]
async fn card_names_and_machines_are_cut_to_64_characters() {
    let dir = tempfile::tempdir().unwrap();
    let c = node("c", dir.path()).await;
    let long = "é".repeat(200);
    let facts = CardFacts {
        name: long.clone(),
        description: String::new(),
        machine: long.clone(),
        os_name: long,
        endpoint: c.id.0.clone(),
        public_url: None,
        skills: Vec::new(),
    };
    let card = signed_card(&facts, &|m: &[u8]| c.sign(m)).unwrap();
    assert_eq!(card_name(&card).chars().count(), CARD_TEXT_MAX);
    let (machine, os) = card_machine(&card);
    assert_eq!(machine.chars().count(), CARD_TEXT_MAX);
    assert_eq!(os.chars().count(), CARD_TEXT_MAX);
    assert_eq!(clip("short"), "short");
}

/// Z3-R2: a card that borrows an existing contact's name but has another key is shown as a
/// new key in the ceremony and pinned under a visibly different alias.
#[tokio::test]
async fn a_same_name_new_key_is_flagged_in_the_ceremony_and_the_alias() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    for (other, want) in [(&b, 1), (&c, 2)] {
        let w = a.ctx.open_window().await.unwrap();
        let vo = other
            .ctx
            .pair(ContactTarget::Ticket { ticket: w.ticket })
            .await
            .unwrap();
        let va = until("the inviter's code", || ceremony_of(&a)).await;
        if want == 2 {
            assert!(va.name.contains("new key"), "{}", va.name);
        } else {
            assert!(!va.name.contains("new key"), "{}", va.name);
        }
        a.ctx.answer(va.id, true).unwrap();
        other.ctx.answer(vo.id, true).unwrap();
        until("pinned", || {
            (a.ctx.shared.read().staged.contacts.len() == want && ceremony_of(&a).is_none())
                .then_some(())
        })
        .await;
    }
    let aliases: Vec<String> = a
        .ctx
        .shared
        .read()
        .staged
        .contacts
        .keys()
        .cloned()
        .collect();
    assert!(aliases.iter().any(|k| k.contains("new-key")), "{aliases:?}");
}

/// Z3-R2: a broken HTTP peer entry never rolls back or blocks a contact edit; the error stays
/// in the status and the file keeps the entry for the owner to fix.
#[tokio::test]
async fn a_broken_http_peer_does_not_block_a_contact_edit() {
    let lan = Lan::default();
    let a = engine(
        "a",
        &lan,
        true,
        A2aConfig {
            enabled: true,
            ..cfg(600)
        },
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let eid = node("bob", dir.path()).await.id.0.clone();
    let path = a._dir.path().join(crate::config::PEERS_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A broken HTTP peer (a loopback card URL without allow_private; nothing is dialled).
    std::fs::write(
        &path,
        format!(
            "peers:\n  broken:\n    card_url: http://127.0.0.1:9/\n  bob:\n    endpoint: {eid}\n    name: bob\n"
        ),
    )
    .unwrap();
    assert!(a.ctx.reload().await.is_err(), "the full file is refused");

    // 191: a contact edit is a record write; the reconcile runs with the last good HTTP peers.
    a.ctx
        .update_contact(ContactUpdate {
            name: "bob".into(),
            patch: ContactPatch {
                blocked: Some(true),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    assert!(
        a.ctx.shared.read().staged.contacts["bob"].blocked,
        "the edit took"
    );
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("broken"),
        "the owner's entry stays"
    );
    assert!(
        a.ctx
            .state
            .lock()
            .error
            .as_deref()
            .is_some_and(|e| e.contains("broken")),
        "the HTTP peer's problem is still reported"
    );
}

/// Pair `a` (inviter) and `b` by ticket; A's alias for B and B's alias for A. 191 (P2): it
/// returns once each side based its record on the other's version 1 (the pushes and card
/// fetches of the pairing have landed, so no background pass races the caller's).
async fn paired(a: &Engine, b: &Engine) -> (String, String) {
    let w = a.ctx.open_window().await.unwrap();
    let vb = b
        .ctx
        .pair(ContactTarget::Ticket { ticket: w.ticket })
        .await
        .unwrap();
    let va = until("the inviter's code", || ceremony_of(a)).await;
    a.ctx.answer(va.id, true).unwrap();
    b.ctx.answer(vb.id, true).unwrap();
    // By the other's endpoint: an engine that already has a contact must not return early.
    let of = |e: &Engine, other: &Engine| {
        e.ctx
            .shared
            .read()
            .staged
            .contact_by_eid(&other.eid())
            .map(str::to_owned)
    };
    let a_alias = until("A pinned B", || of(a, b)).await;
    let b_alias = until("B pinned A", || of(b, a)).await;
    for (e, alias) in [(a, &a_alias), (b, &b_alias)] {
        let record = e.ctx.shared.read().staged.contacts[alias].id.clone();
        let mut based = false;
        for _ in 0..400 {
            if e.ctx
                .records()
                .await
                .is_ok_and(|r| r.local.iter().any(|l| l.record == record))
            {
                based = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(based, "{alias} based on the other side's roster");
        // The pass that basing started has finished.
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(e.ctx.reload_lock.lock().await);
    }
    (a_alias, b_alias)
}

/// 183 F1, F2: sharing an agent with a contact starts it on "Always allow" (an explicit "Ask
/// first" is kept on a re-grant), the contact's engine hears of the grant and lists the
/// export as its own `discover_peers` row (`peer:<contact>/<export>`, its description,
/// tags `contact`, its kind and state), and taking the grant back removes the row after the
/// notice. The contact's card is per reader: the public card lists nothing.
#[tokio::test]
async fn a_granted_export_is_a_discover_peers_row_on_the_contacts_side_until_revoked() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, b_alias) = paired(&a, &b).await;
    a.ctx
        .put_export(Export {
            id: None,
            name: "research".into(),
            target: ExportTarget::Agent {
                agent: "theuth".into(),
            },
            description: "Answers research questions".into(),
            paused: None,
        })
        .await
        .unwrap();
    let patch = |grant: &[&str], revoke: &[&str]| {
        let names = |l: &[&str]| Some(l.iter().map(|s| s.to_string()).collect());
        a.ctx.update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                grant: names(grant),
                revoke: names(revoke),
                ..ContactPatch::default()
            },
        })
    };
    let grant = |exports: &[&str]| patch(exports, &[]);
    let shared = grant(&["research"]).await.unwrap();
    assert_eq!(
        shared.first_contact.get("research"),
        Some(&FirstContact::Allow),
        "F2: sharing counts as allowing"
    );
    assert!(
        a.ctx.my_card().unwrap().skills.is_empty(),
        "the public card lists nothing"
    );

    // B's discover_peers, as one of its own chats sees it.
    let b_agents = b.ctx.agents().unwrap();
    let root = b_agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions::default(),
            peer: None,
        })
        .await
        .unwrap()
        .session;
    let rows = || async {
        b_agents
            .ask(agent::ListPeers {
                session: root,
                query: None,
                cursor: None,
                limit: None,
                active_only: true,
                kind: None,
            })
            .await
            .unwrap()
            .peers
    };
    let address = format!("peer:{b_alias}/research");
    let mut row = None;
    for _ in 0..100 {
        row = rows().await.into_iter().find(|p| p.address == address);
        if row.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let row = row.expect("the granted export is a row on the contact's side");
    assert_eq!(row.title.as_deref(), Some("Answers research questions"));
    assert_eq!(row.tags, ["contact", "agent", "allowed"]);
    assert!(row.send && row.reply);
    let all = rows().await;
    assert_eq!(
        all[..2]
            .iter()
            .map(|p| p.address.as_str())
            .collect::<Vec<_>>(),
        [format!("peer:{b_alias}"), address.clone()],
        "F6: the contact and its rows lead, and active_only keeps them"
    );
    assert_eq!(
        b.ctx.contact_list()[0].shared[0].name,
        "research",
        "Settings › Network › Contacts: Shared with you"
    );

    // F2: the owner switches research to "Ask first" (stored as such).
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: a_alias.clone(),
            export: "research".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap();
    assert_eq!(
        a.ctx.contact_list()[0].first_contact.get("research"),
        Some(&FirstContact::Ask)
    );
    until("B sees research ask first", || {
        b.ctx
            .offers_of(&a.eid())
            .first()
            .filter(|o| o.state == agent::communication::network::OfferState::AskFirst)
            .map(|_| ())
    })
    .await;

    // Taking it back: the notice reaches B and the row goes.
    patch(&[], &["research"]).await.unwrap();
    let mut gone = false;
    for _ in 0..100 {
        if !rows().await.iter().any(|p| p.address == address) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the revoked export's row is gone after the notice");
    assert!(
        rows()
            .await
            .iter()
            .any(|p| p.address == format!("peer:{b_alias}")),
        "the contact's own row stays"
    );

    // F2 (review 1): sharing it again keeps the owner's last explicit choice (Ask first);
    // an export never answered starts on Always allow (the first grant above).
    let again = grant(&["research"]).await.unwrap();
    assert_eq!(
        again.first_contact.get("research"),
        Some(&FirstContact::Ask),
        "regrant keeps the owner's Ask first"
    );

    // 183 review 1: the card is per reader — a third contact of A sees none of it.
    let c = engine("c", &lan, true, cfg(600)).await;
    paired(&a, &c).await;
    let card = a.ctx.contact_card(&c.eid()).await.unwrap();
    assert!(card.skills.is_empty(), "nothing is granted to C");
    let card = a.ctx.contact_card(&b.eid()).await.unwrap();
    assert_eq!(
        card.skills
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["research"],
        "B's card lists what B is granted"
    );
}

/// 183 F4: editing a contact (here: sharing a second export, which rotates its grant
/// epoch) keeps a first message waiting for an export it still has, and so does a new
/// description of that export (review 2); Always then delivers it like Allow once and it
/// stops waiting. 190 (D8): re-pointing the export (another agent) fails a waiting message
/// at once (`RebindHeld` with nothing kept).
#[tokio::test]
async fn a_grant_change_keeps_a_waiting_first_message_and_always_releases_it() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, _) = paired(&a, &b).await;
    for name in ["research", "zeta"] {
        a.ctx
            .put_export(Export {
                id: None,
                name: name.into(),
                target: ExportTarget::Agent {
                    agent: "theuth".into(),
                },
                description: String::new(),
                paused: None,
            })
            .await
            .unwrap();
    }
    let grant = |exports: &[&str]| {
        a.ctx.update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                grant: Some(exports.iter().map(|s| s.to_string()).collect()),
                ..ContactPatch::default()
            },
        })
    };
    grant(&["research"]).await.unwrap();
    // The owner sets research to Ask first; B's first message waits.
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: a_alias.clone(),
            export: "research".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap();
    let client = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
        net: b.net.clone(),
        eid: a.eid(),
        members: Vec::new(),
        answered: Default::default(),
        hints: reach::Hints {
            addrs: a.addrs(),
            relay: None,
        },
        export: Some("research".into()),
    }) as Box<dyn a2a_client::Transport>);
    let mut m = a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("hi")]);
    m.message_id = "m-1".into();
    let req = a2a_types::SendMessageRequest {
        message: m,
        configuration: Some(a2a_types::SendMessageConfiguration {
            accepted_output_modes: Some(vec!["text/plain".into()]),
            task_push_notification_config: None,
            history_length: Some(0),
            return_immediately: Some(true),
        }),
        metadata: None,
        tenant: None,
    };
    let first = match client.send_message(&req).await.unwrap() {
        a2a_types::SendMessageResponse::Task(t) => t,
        other => panic!("expected a task, got {other:?}"),
    };
    assert_eq!(first.status.state, a2a_types::TaskState::AuthRequired);
    let waiting = a.ctx.first_waiting().await;
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].preview, "hi");

    // Sharing zeta too rotates the contact's grant epoch: research still waits.
    grant(&["research", "zeta"]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.ctx.first_waiting().await.len(), 1, "still waiting");
    a.ctx
        .put_export(Export {
            id: None,
            name: "research".into(),
            target: ExportTarget::Agent {
                agent: "theuth".into(),
            },
            description: "Answers research questions".into(),
            paused: None,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        a.ctx.first_waiting().await.len(),
        1,
        "a description edit keeps it waiting"
    );

    // Always: delivered like Allow once, and nothing waits any more.
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: a_alias.clone(),
            export: "research".into(),
            answer: FirstContact::Allow,
        })
        .await
        .unwrap();
    assert!(a.ctx.first_waiting().await.is_empty());
    let now = client
        .get_task(&a2a_types::GetTaskRequest {
            id: first.id.clone(),
            history_length: Some(0),
            tenant: None,
        })
        .await
        .unwrap();
    assert_ne!(now.status.state, a2a_types::TaskState::AuthRequired);
    assert_ne!(
        now.status.state,
        a2a_types::TaskState::Failed,
        "it was let through, not revoked: {:?}",
        now.status.message.as_ref().and_then(|m| m.text())
    );

    // Re-pointed to another agent: the next waiting message fails at once, it never runs
    // against what the export publishes now.
    a.ctx
        .first_answer(ContactFirstAnswer {
            contact: a_alias.clone(),
            export: "research".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap();
    let mut m =
        a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("again")]);
    m.message_id = "m-2".into();
    let second = match client
        .send_message(&a2a_types::SendMessageRequest { message: m, ..req })
        .await
        .unwrap()
    {
        a2a_types::SendMessageResponse::Task(t) => t,
        other => panic!("expected a task, got {other:?}"),
    };
    assert_eq!(second.status.state, a2a_types::TaskState::AuthRequired);
    a.ctx
        .put_export(Export {
            id: None,
            name: "research".into(),
            target: ExportTarget::Agent {
                agent: "theuth@elsewhere".into(),
            },
            description: "Answers research questions".into(),
            paused: None,
        })
        .await
        .unwrap();
    assert!(a.ctx.first_waiting().await.is_empty(), "no longer waiting");
    let now = client
        .get_task(&a2a_types::GetTaskRequest {
            id: second.id.clone(),
            history_length: Some(0),
            tenant: None,
        })
        .await
        .unwrap();
    assert_eq!(now.status.state, a2a_types::TaskState::Failed);
    assert_eq!(
        now.status.message.as_ref().and_then(|m| m.text()),
        Some(agent::communication::network::HOLD_CHANGED)
    );
}

/// 183 review 1, 2: offers notices from one contact coalesce — five at once run the refresh
/// in flight and one follow-up — a failed refresh leaves the next notice free to run, and
/// the claim on `busy` is released however its task ends (a panic drops it unreleased).
#[tokio::test]
async fn offers_notices_coalesce_and_never_stick() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let eid = b.eid();
    // B is no contact of A: every fetch of its card fails (refused), and quickly.
    let state = || {
        let all = a.ctx.offers.lock();
        all.get(&eid)
            .map(|c| (c.busy, c.again, c.refreshes, c.error.is_some()))
    };
    for _ in 0..5 {
        a.ctx.offers_noticed(&eid);
    }
    assert_eq!(
        state(),
        Some((true, true, 0, false)),
        "one running, one more asked"
    );
    let done = until("the refreshes end", || state().filter(|s| !s.0)).await;
    assert_eq!(
        done,
        (false, false, 2, true),
        "the running one and one follow-up"
    );
    a.ctx.offers_noticed(&eid);
    let done = until("the next one ends", || state().filter(|s| !s.0)).await;
    assert_eq!(done.2, 3, "after an error a notice still runs");

    // A claim dropped without its loop releasing it (a panic) clears the flags.
    {
        let mut all = a.ctx.offers.lock();
        let c = all.get_mut(&eid).unwrap();
        (c.busy, c.again) = (true, true);
    }
    drop(super::offers::Busy {
        ctx: a.ctx.clone(),
        eid: eid.clone(),
        done: false,
    });
    assert_eq!(state().map(|s| (s.0, s.1)), Some((false, false)));
}

/// Polls `f` until it yields (40 s: a contact's task is followed by polling, 2 s doubling).
async fn eventually<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    for _ in 0..800 {
        if let Some(t) = f().await {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// 186 (D4, D5, D12), 190 (D2, D8, review 1): a chat shared with a contact. `cautious` is
/// refused, `full` is accepted and ignored, and a share is never refused for the contact's
/// version (its card unread or older). The contact's message is a peer row in the shared
/// chat (not `<contact_message>`), its receipt
/// reads consumed, the chat's `send_message(reply_to)` lands in the contact's originating
/// session as `<message from=peer:…>` and settles its awaiting send; the two go on answering
/// each other, several times each way, on one conversation. Unshared, an answer is refused
/// here and the contact's return route ends the conversation (`TASK_NOT_FOUND`). Shared again
/// and re-pointed to another chat, it stays shared in full.
#[tokio::test]
async fn a_chat_shared_in_full_talks_both_ways_like_a_mesh_peer() {
    use agent::communication::network::{FullWire, RETURN_ROUTE};
    use agent::{ListPeerMessages, PeerMessageState, ShareMode};

    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, b_alias) = paired(&a, &b).await;
    let (a_agents, b_agents) = (a.ctx.agents().unwrap(), b.ctx.agents().unwrap());
    // The runners report consumption and outcomes to the registered supervisor (one per
    // engine process): here A's, the side whose tasks complete on consumption. B's reports
    // reach A and find nothing of theirs; B's own steps here go by direct refs.
    a_agents
        .register(agent::AGENT_ACTOR)
        .expect("no other test registers a supervisor");
    let chat_of = |agents: kameo::actor::ActorRef<agent::Agents>| async move {
        agents
            .ask(agent::CreateSession {
                options: agent::CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap()
            .session
    };
    let chat = chat_of(a_agents.clone()).await;
    let origin = chat_of(b_agents.clone()).await;
    a.ctx
        .put_export(Export {
            id: None,
            name: "proj".into(),
            target: ExportTarget::Chat { session: chat },
            description: "The project chat".into(),
            paused: None,
        })
        .await
        .unwrap();
    let share = || {
        a.ctx.update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                grant: Some(vec!["proj".into()]),
                ..ContactPatch::default()
            },
        })
    };
    // Review 1: a contact whose card does not (yet) collaborate in full is shared with all
    // the same.
    until("A read B's card", || a.ctx.full_of(&b.eid()).then_some(())).await;
    if let Some(c) = a.ctx.offers.lock().get_mut(&b.eid()) {
        c.full = false;
    }
    let shared = share().await.unwrap();
    assert!(!shared.full_capable);
    assert_eq!(shared.exports, ["proj"]);
    a.ctx.refresh_offers(&b.eid()).await;
    until("B sees proj shared in full", || {
        b.ctx
            .offers_of(&a.eid())
            .iter()
            .find(|o| o.name == "proj" && o.mode == ShareMode::Full)
            .map(|_| ())
    })
    .await;

    // B's agent lists it (its send goes by the card's `full` tag).
    let address = format!("peer:{b_alias}/proj");
    let tags = eventually("B's discover_peers row", || {
        let (b_agents, address) = (b_agents.clone(), address.clone());
        async move {
            b_agents
                .ask(agent::ListPeers {
                    session: origin,
                    query: None,
                    cursor: None,
                    limit: None,
                    active_only: false,
                    kind: None,
                })
                .await
                .unwrap()
                .peers
                .into_iter()
                .find(|p| p.address == address)
                .map(|p| p.tags)
        }
    })
    .await;
    assert_eq!(tags, ["contact", "chat", "allowed"]);
    let deliver =
        |from, to: String, text: &str, reply_to, await_reply| agent::supervisor::Deliver {
            from,
            to,
            text: text.into(),
            await_reply,
            reply_to,
            request_id: None,
            admitted: tokio_util::sync::CancellationToken::new(),
        };
    // The newest record of `session` with `text`, once it is consumed.
    let consumed = |agents: kameo::actor::ActorRef<agent::Agents>, session, text: &'static str| {
        eventually(text, move || {
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
                    .find(|r| {
                        r.text == text
                            && r.receipt.to == format!("session:{session}")
                            && r.receipt.state != PeerMessageState::Accepted
                    })
            }
        })
    };

    // B writes, awaiting the answer.
    let sent = b_agents
        .ask(deliver(
            origin,
            format!("peer:{b_alias}/proj"),
            "ping 1",
            None,
            true,
        ))
        .await
        .unwrap();
    let waiting = sent.reply.expect("an awaited send");
    let x = consumed(a_agents.clone(), chat, "ping 1").await;
    assert_eq!(
        x.receipt.state,
        PeerMessageState::Consumed,
        "{:?}",
        x.receipt.error
    );
    assert_eq!(
        x.contact_mode,
        Some(ShareMode::Full),
        "a full contact's row"
    );
    assert!(x.from.starts_with("peer:"), "{}", x.from);
    let receipt = eventually("B's receipt reads consumed", || {
        let b_agents = b_agents.clone();
        async move {
            let r = b_agents
                .ask(agent::GetPeerMessage {
                    session: origin,
                    id: sent.receipt.id,
                })
                .await
                .unwrap();
            (r.receipt.state != PeerMessageState::Accepted).then_some(r)
        }
    })
    .await;
    assert_eq!(
        receipt.receipt.state,
        PeerMessageState::Consumed,
        "{:?}",
        receipt.receipt
    );
    assert!(receipt.receipt.reply.is_none(), "a receipt, not a reply");
    // The shared chat's history: a peer row, never `<contact_message>`.
    let rows = eventually("A's chat history", || {
        let a_agents = a_agents.clone();
        async move {
            let loaded = a_agents
                .ask(agent::LoadSession {
                    session: chat,
                    before: None,
                    limit: None,
                    settled_only: false,
                })
                .await
                .ok()?;
            let rows: Vec<String> = loaded
                .history
                .iter()
                .filter_map(|h| match h {
                    agent::HistoryEntry::User { text, peer, .. } => {
                        Some((text.clone(), peer.as_ref()?.contact_mode))
                    }
                    _ => None,
                })
                .filter(|(_, m)| *m == Some(ShareMode::Full))
                .map(|(t, _)| t)
                .collect();
            (!rows.is_empty()).then_some(rows)
        }
    })
    .await;
    assert!(rows[0].starts_with("<message from=\"peer:"), "{}", rows[0]);
    assert!(!rows[0].contains("<contact_message"), "{}", rows[0]);

    // A answers with reply_to: B's awaiting send settles with it.
    let answer = a_agents
        .ask(deliver(
            chat,
            format!("peer:{a_alias}"),
            "pong 1",
            Some(x.receipt.id),
            false,
        ))
        .await
        .unwrap();
    assert_eq!(answer.receipt.state, PeerMessageState::Accepted);
    let fin = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("the awaited send settles")
        .unwrap();
    assert_eq!(fin.state, PeerMessageState::Replied);
    assert_eq!(fin.reply.as_deref(), Some("pong 1"));

    // Any number of times: a second answer to the same message, then a ping-pong.
    a_agents
        .ask(deliver(
            chat,
            format!("peer:{a_alias}"),
            "pong 1b",
            Some(x.receipt.id),
            false,
        ))
        .await
        .unwrap();
    let mut last = consumed(b_agents.clone(), origin, "pong 1b").await;
    assert_eq!(last.contact_mode, Some(ShareMode::Full));
    for (ping, pong) in [("ping 2", "pong 2"), ("ping 3", "pong 3")] {
        b_agents
            .ask(deliver(
                origin,
                format!("peer:{b_alias}"),
                ping,
                Some(last.receipt.id),
                false,
            ))
            .await
            .unwrap();
        let got = consumed(a_agents.clone(), chat, ping).await;
        assert!(
            got.receipt.reply_to.is_some(),
            "{ping} names the message it answers"
        );
        a_agents
            .ask(deliver(
                chat,
                format!("peer:{a_alias}"),
                pong,
                Some(got.receipt.id),
                false,
            ))
            .await
            .unwrap();
        last = consumed(b_agents.clone(), origin, pong).await;
        assert_eq!(last.receipt.reply_to.map(|_| ()), Some(()), "{pong}");
    }

    // D12: unshared, the chat's answer is refused here and the conversation ends there.
    let context = {
        let client = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
            net: b.net.clone(),
            eid: a.eid(),
            members: Vec::new(),
            answered: Default::default(),
            hints: reach::Hints {
                addrs: a.addrs(),
                relay: None,
            },
            export: Some("proj".into()),
        }) as Box<dyn a2a_client::Transport>);
        client
            .get_task(&a2a_types::GetTaskRequest {
                id: x
                    .receipt
                    .task_id
                    .clone()
                    .expect("A's record names its task"),
                history_length: Some(0),
                tenant: None,
            })
            .await
            .unwrap()
            .context_id
    };
    a.ctx
        .update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                revoke: Some(vec!["proj".into()]),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    let refused = a_agents
        .ask(deliver(
            chat,
            format!("peer:{a_alias}"),
            "late",
            Some(x.receipt.id),
            false,
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("no longer shared"), "{refused}");
    until("B sees proj gone", || {
        (!b.ctx.offers_of(&a.eid()).iter().any(|o| o.name == "proj")).then_some(())
    })
    .await;
    let ret = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
        net: a.net.clone(),
        eid: b.eid(),
        members: Vec::new(),
        answered: Default::default(),
        hints: reach::Hints {
            addrs: b.addrs(),
            relay: None,
        },
        export: Some(RETURN_ROUTE.into()),
    }) as Box<dyn a2a_client::Transport>);
    let mut m = a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("late")]);
    m.message_id = "late-1".into();
    m.context_id = Some(context);
    m.metadata = Some(FullWire::default().metadata());
    let gone = ret
        .send_message(&a2a_types::SendMessageRequest {
            message: m,
            configuration: None,
            metadata: None,
            tenant: None,
        })
        .await
        .unwrap_err();
    assert_eq!(
        gone.code,
        a2a_types::error_code::TASK_NOT_FOUND,
        "{}",
        gone.message
    );

    // 190 (D8): re-pointed to another chat, the export stays shared, in full.
    share().await.unwrap();
    let other = chat_of(a_agents.clone()).await;
    a.ctx
        .put_export(Export {
            id: None,
            name: "proj".into(),
            target: ExportTarget::Chat { session: other },
            description: "The project chat".into(),
            paused: None,
        })
        .await
        .unwrap();
    let after = a
        .ctx
        .contact_list()
        .into_iter()
        .find(|c| c.name == a_alias)
        .unwrap();
    assert_eq!(after.exports, ["proj"], "re-pointed: still shared");
    assert!(
        crate::contacts::offers::granted(&a.ctx.shared.read().staged, &a_alias)
            .iter()
            .any(|o| o.name == "proj" && o.mode == ShareMode::Full),
        "re-pointed: full on the card"
    );
}

/// 191: what the record replica does between two machines of one mesh: every contact row
/// of `from`, applied on `to` column by column (the heads rows too with [`replicate_rows`]).
async fn replicate(from: &Engine, to: &Engine) {
    replicate_rows(from, to, false).await;
}

/// [`replicate`], `heads` with the `contact_roster_heads` rows (P2).
async fn replicate_rows(from: &Engine, to: &Engine, heads: bool) {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let r = from.ctx.records().await.unwrap();
    let t = |v: &Option<String>| v.clone().map_or(Cell::Null, Cell::Text);
    let i = |v: &Option<i64>| v.map_or(Cell::Null, Cell::Int);
    let mut w = Vec::new();
    for c in &r.contacts {
        w.push(RecordWrite::new(
            RecordTable::Contacts,
            c.id.clone(),
            vec![
                ("alias", t(&c.alias)),
                ("name", t(&c.name)),
                ("machine", t(&c.machine)),
                ("binding", t(&c.binding)),
                ("paired_ms", i(&c.paired_ms)),
                ("paired_on", t(&c.paired_on)),
                ("blocked", i(&c.blocked)),
                ("removed_ms", i(&c.removed_ms)),
                ("merged_into", t(&c.merged_into)),
            ],
        ));
    }
    for e in &r.endpoints {
        w.push(RecordWrite::new(
            RecordTable::Endpoints,
            e.id.clone(),
            vec![
                ("contact", t(&e.contact)),
                ("eid", t(&e.eid)),
                ("paired_ms", i(&e.paired_ms)),
                ("barred_ms", i(&e.barred_ms)),
                ("pinned_by", t(&e.pinned_by)),
            ],
        ));
    }
    for g in &r.grants {
        w.push(RecordWrite::new(
            RecordTable::Grants,
            g.id.clone(),
            vec![
                ("contact", t(&g.contact)),
                ("export", t(&g.export)),
                ("granted", i(&g.granted)),
                ("first_contact", t(&g.first_contact)),
            ],
        ));
    }
    for x in &r.exports {
        w.push(RecordWrite::new(
            RecordTable::Exports,
            x.id.clone(),
            vec![
                ("name", t(&x.name)),
                ("session", t(&x.session)),
                ("agent", t(&x.agent)),
                ("title", t(&x.title)),
                ("description", t(&x.description)),
                ("home", t(&x.home)),
                ("removed_ms", i(&x.removed_ms)),
            ],
        ));
    }
    // 191 (P2): our signed copies and the heads rows (a machine's own row is its own: never
    // written back over it); `contact_roster_local` never travels.
    for x in &r.rosters.rows {
        w.push(RecordWrite::new(
            RecordTable::Rosters,
            x.id.clone(),
            vec![
                ("contact", t(&x.contact)),
                ("chain", t(&x.chain)),
                ("version", i(&x.version)),
                ("signer", t(&x.signer)),
                ("hash", t(&x.hash)),
                ("roster", t(&x.roster)),
            ],
        ));
    }
    let dest = to.eid();
    for h in r
        .heads
        .iter()
        .filter(|h| heads && h.machine.as_deref() != Some(dest.as_str()))
    {
        w.push(RecordWrite::new(
            RecordTable::Heads,
            h.id.clone(),
            vec![
                ("record", t(&h.record)),
                ("machine", t(&h.machine)),
                ("chain", t(&h.chain)),
                ("version", i(&h.version)),
                ("hash", t(&h.hash)),
                ("history", t(&h.history)),
                ("members", t(&h.members)),
                ("conflict", i(&h.conflict)),
                ("at_ms", i(&h.at_ms)),
            ],
        ));
    }
    let agents = to.ctx.agents().unwrap();
    agents
        .ask(agent::communication::network::WriteContactRecords(w))
        .await
        .unwrap();
}

/// 191 (D2, the rig's "paired on n1, listed on n2"): a contact paired on one machine is a
/// record; another machine of the mesh, once the rows reach it, lists and admits the same
/// contact under the same record and binding id, with no ceremony of its own.
#[tokio::test]
async fn a_contact_paired_on_one_machine_is_listed_and_admitted_on_another() {
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let n2 = engine("n2", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n1, &c).await;
    assert!(n2.ctx.contact_list().is_empty());
    replicate(&n1, &n2).await;
    assert!(
        n2.ctx.tick().await,
        "the arrived rows change the content: a reconcile"
    );
    let (on1, on2) = (n1.ctx.contact_list(), n2.ctx.contact_list());
    assert_eq!(on2.len(), 1, "{on2:?}");
    assert_eq!(
        (&on2[0].name, &on2[0].endpoint, &on2[0].id, &on2[0].problem),
        (&alias, &c.eid(), &on1[0].id, &None)
    );
    let binding = |e: &Engine| match e.ctx.gate(&c.eid()) {
        IrohGate::Contact { binding, .. } => binding,
        other => panic!("not admitted: {other:?}"),
    };
    assert_eq!(
        binding(&n2),
        binding(&n1),
        "one binding id on every machine"
    );
    // The commit's own binding changes allow one more idempotent pass; then it settles.
    let _ = n2.ctx.tick().await;
    assert!(!n2.ctx.tick().await, "nothing changed since");
    assert_eq!(binding(&n2), binding(&n1));
}

/// 191 (D2, M1): a block another machine wrote arrives as one column of an existing row (no
/// `hlc` this machine keys on moves): the content hash catches it at the next tick, and the
/// gate refuses the contact.
#[tokio::test]
async fn a_block_written_elsewhere_applies_at_the_next_tick() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n1, &c).await;
    assert!(matches!(n1.ctx.gate(&c.eid()), IrohGate::Contact { .. }));
    let id = n1.ctx.contact_list()[0].id.clone();
    n1.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(vec![
            RecordWrite::new(RecordTable::Contacts, id, vec![("blocked", Cell::Int(1))]),
        ]))
        .await
        .unwrap();
    assert!(n1.ctx.tick().await, "the content changed");
    assert!(n1.ctx.shared.read().staged.contacts[&alias].blocked);
    assert_eq!(n1.ctx.gate(&c.eid()), IrohGate::Refuse);
}

/// 191 (D2, D7, M2): before the mesh latch nothing is staged and the gate refuses every
/// contact; the first reconcile after the latch revokes every local binding the synced
/// records do not hold (a contact removed elsewhere while this machine was down) before
/// `contacts_ready` is set and the contact is admitted.
#[tokio::test]
async fn contacts_are_refused_until_the_latch_and_the_first_reconcile() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    paired(&n1, &c).await;
    let dir = tempfile::tempdir().unwrap();
    let net = node("n2", dir.path()).await;
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let (ready, latch) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let agents = agent::spawn_engine_agents(rt, ready.clone()).await;
    let l = latch.clone();
    let ctx = crate::test_ctx(
        dir.path(),
        net.clone(),
        Arc::new(lan.clone()),
        cfg(600),
        Some(agents.clone()),
        ready.clone(),
        Arc::new(move || l.load(Ordering::SeqCst)),
    )
    .await;
    let n2 = Engine {
        ctx,
        net,
        _dir: dir,
    };
    // A binding this machine still holds for a contact the mesh removed while it was down.
    agents
        .ask(agent::communication::network::BindPeer {
            alias: "stale".into(),
            endpoint: agent::communication::network::contact_endpoint("stale-eid"),
        })
        .await
        .unwrap();
    replicate(&n1, &n2).await;
    n2.ctx.reload().await.unwrap();
    assert!(!ready.load(Ordering::SeqCst), "not before the latch");
    assert_eq!(
        n2.ctx.contact_list().len(),
        1,
        "listed from the local rows, never admitted"
    );
    assert!(
        n2.ctx.shared.read().bindings.is_empty(),
        "no route committed"
    );
    assert_eq!(n2.ctx.gate(&c.eid()), IrohGate::Refuse);

    latch.store(true, Ordering::SeqCst);
    assert!(n2.ctx.tick().await, "the first pass after the latch runs");
    assert!(ready.load(Ordering::SeqCst));
    assert!(matches!(n2.ctx.gate(&c.eid()), IrohGate::Contact { .. }));
    assert!(
        !agents
            .ask(agent::communication::network::RevokePeerBinding {
                alias: "stale".into()
            })
            .await
            .unwrap(),
        "already revoked by the first reconcile"
    );
}

/// Publish `target` as export `name` (an `a2a_export_put`).
async fn publish(e: &Engine, name: &str, target: ExportTarget, description: &str) {
    e.ctx
        .put_export(Export {
            id: None,
            name: name.into(),
            target,
            description: description.into(),
            paused: None,
        })
        .await
        .unwrap();
}

/// 191: grant `names` to contact `alias` as an earlier build's `peers.yaml` grant did (no
/// first-contact answer: ask first), as record rows.
async fn grant_ask(e: &Engine, alias: &str, names: &[&str]) {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let live = e.ctx.shared.read().clone();
    let id = live.staged.contacts[alias].id.clone();
    let w = names
        .iter()
        .map(|n| {
            let x = live.staged.export_ids[*n].clone();
            RecordWrite::new(
                RecordTable::Grants,
                format!("{id}|{x}"),
                vec![
                    ("contact", Cell::Text(id.clone())),
                    ("export", Cell::Text(x)),
                    ("granted", Cell::Int(1)),
                ],
            )
        })
        .collect();
    e.ctx.commit_records(w).await.unwrap();
}

/// 191 (the 2026-11-13 revoke, as 333 saw it): sessions do not replicate, so a chat another
/// machine of the mesh shared (its export and grant rows here, its session not) is left out
/// of this machine's routes. `contact_list` still shows the share and `a2a_exports` lists the
/// export paused elsewhere (never tombstoned). Sharing this machine's own chat from the chat
/// menu right after `a2a_export_put` grants it (no "unknown export") and leaves the other
/// machine's grant alone, as a delta and as an older UI's whole list read from this view.
#[tokio::test]
async fn a_share_of_a_chat_on_another_machine_shows_here_and_survives_a_share_made_here() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&a, &b).await;
    let id = a.ctx.contact_list()[0].id.clone();
    // What the other machine wrote: its chat's export (the session never came here), its
    // home there, and the grant.
    let remote = uuid::Uuid::now_v7().to_string();
    for home in ["xeus".to_owned(), a.eid()] {
        a.ctx
            .commit_records(vec![
                RecordWrite::new(
                    RecordTable::Exports,
                    remote.clone(),
                    vec![
                        ("name", Cell::Text("xeus-chat".into())),
                        ("session", Cell::Text(runtime::SessionId::new().to_string())),
                        ("agent", Cell::Null),
                        ("title", Cell::Text("xeus-chat".into())),
                        ("description", Cell::Text("The chat on xeus".into())),
                        ("home", Cell::Text(home)),
                    ],
                ),
                RecordWrite::new(
                    RecordTable::Grants,
                    format!("{id}|{remote}"),
                    vec![
                        ("contact", Cell::Text(id.clone())),
                        ("export", Cell::Text(remote.clone())),
                        ("granted", Cell::Int(1)),
                        ("first_contact", Cell::Text("allow".into())),
                    ],
                ),
            ])
            .await
            .unwrap();
        assert!(
            !a.ctx.shared.read().staged.peers[&alias]
                .exports
                .contains(&"xeus-chat".to_owned()),
            "not routed here: its session is not here"
        );
        assert_eq!(a.ctx.contact_list()[0].exports, ["xeus-chat"], "but shown");
        // Made here (Settings shared a chat of another machine) or there: listed, paused.
        let listed = a.ctx.exports().await;
        let x = listed.iter().find(|x| x.name == "xeus-chat");
        assert_eq!(x.and_then(|x| x.paused.as_deref()), Some(PAUSED_ELSEWHERE));
    }
    let granted = || async {
        a.ctx
            .records()
            .await
            .unwrap()
            .grants
            .iter()
            .find(|g| g.export.as_deref() == Some(remote.as_str()))
            .and_then(|g| g.granted)
    };
    // The chat-menu share of this machine's own chat, as the UI sends it.
    let chat = a
        .ctx
        .agents()
        .unwrap()
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions::default(),
            peer: None,
        })
        .await
        .unwrap()
        .session;
    publish(&a, "own-chat", ExportTarget::Chat { session: chat }, "Mine").await;
    let update = |patch: ContactPatch| {
        a.ctx.update_contact(ContactUpdate {
            name: alias.clone(),
            patch,
        })
    };
    let shown = update(ContactPatch {
        grant: Some(vec!["own-chat".into()]),
        ..ContactPatch::default()
    })
    .await
    .unwrap();
    let mut both = shown.exports.clone();
    both.sort();
    assert_eq!(both, ["own-chat", "xeus-chat"]);
    assert_eq!(granted().await, Some(1));
    assert!(
        a.ctx.shared.read().staged.peers[&alias]
            .exports
            .contains(&"own-chat".to_owned()),
        "the own chat is routed"
    );
    // The whole list an earlier build sent is refused and writes nothing.
    let refused = update(ContactPatch {
        exports: Some(vec!["own-chat".into()]),
        ..ContactPatch::default()
    })
    .await
    .unwrap_err();
    assert_eq!(refused, EXPORTS_REPLACED);
    assert_eq!(granted().await, Some(1));
    // A revoke names its export alone.
    update(ContactPatch {
        grant: Some(vec!["own-chat".into()]),
        ..ContactPatch::default()
    })
    .await
    .unwrap();
    let after = update(ContactPatch {
        revoke: Some(vec!["own-chat".into()]),
        ..ContactPatch::default()
    })
    .await
    .unwrap();
    assert_eq!(after.exports, ["xeus-chat"]);
    assert_eq!(granted().await, Some(1));
    // Stop sharing: own-chat has no other holder and goes with it; xeus-chat stays.
    assert!(
        a.ctx.exports().await.iter().any(|x| x.name == "own-chat"),
        "a revoke keeps the export"
    );
    let after = update(ContactPatch {
        unshare: Some(vec!["own-chat".into()]),
        ..ContactPatch::default()
    })
    .await
    .unwrap();
    assert_eq!(after.exports, ["xeus-chat"]);
    let names: Vec<String> = a.ctx.exports().await.into_iter().map(|x| x.name).collect();
    assert!(
        !names.contains(&"own-chat".to_owned()) && names.contains(&"xeus-chat".to_owned()),
        "{names:?}"
    );
}

/// 191 (review 1, W-41): listing never reads a missing session as a deletion. An export
/// homed here whose chat this machine lacks is listed paused elsewhere and stays, before the
/// mesh latch (unfiltered display staging) and after it. Deleting a chat on the engine that
/// runs it (`SessionDeleted`, through the subscribed actor) tombstones every export of it,
/// whatever its home, so its grants stop counting.
#[tokio::test]
async fn only_a_deleted_chat_tombstones_its_exports_never_a_listing() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    use kameo::actor::Spawn as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    let lan = Lan::default();
    let dir = tempfile::tempdir().unwrap();
    let net = node("n", dir.path()).await;
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let (ready, latch) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let agents = agent::spawn_engine_agents(rt.clone(), ready.clone()).await;
    let l = latch.clone();
    let ctx = crate::test_ctx(
        dir.path(),
        net.clone(),
        Arc::new(lan.clone()),
        cfg(600),
        Some(agents.clone()),
        ready,
        Arc::new(move || l.load(Ordering::SeqCst)),
    )
    .await;
    let actor = crate::A2aActor::spawn(crate::A2aActor { ctx: ctx.clone() });
    rt.subscribe::<crate::A2aActor, agent::AgentEvent>(&actor)
        .await;
    let me = net.id.0.clone();
    let row = |id: &str, name: &str, session: &str, home: &str| {
        RecordWrite::new(
            RecordTable::Exports,
            id,
            vec![
                ("name", Cell::Text(name.into())),
                ("session", Cell::Text(session.into())),
                ("agent", Cell::Null),
                ("title", Cell::Text(name.into())),
                ("description", Cell::Text(String::new())),
                ("home", Cell::Text(home.into())),
            ],
        )
    };
    let removed = |id: &'static str| {
        let ctx = ctx.clone();
        async move {
            ctx.records()
                .await
                .unwrap()
                .exports
                .iter()
                .find(|x| x.id == id)
                .and_then(|x| x.removed_ms)
        }
    };
    // A chat of another machine, its export made here (Settings), its session not here.
    let absent = runtime::SessionId::new().to_string();
    ctx.commit_records(vec![row("x-away", "away", &absent, &me)])
        .await
        .unwrap();
    for latched in [false, true] {
        latch.store(latched, Ordering::SeqCst);
        ctx.reload().await.unwrap();
        let listed = ctx.exports().await;
        assert_eq!(
            listed
                .iter()
                .find(|x| x.name == "away")
                .and_then(|x| x.paused.as_deref()),
            Some(PAUSED_ELSEWHERE),
            "latched: {latched}"
        );
        assert_eq!(removed("x-away").await, None, "latched: {latched}");
    }
    // A chat of this machine, exported here and (for it) by another machine.
    let chat = agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions::default(),
            peer: None,
        })
        .await
        .unwrap()
        .session;
    ctx.commit_records(vec![
        row("x-mine", "mine", &chat.to_string(), &me),
        row("x-theirs", "theirs", &chat.to_string(), "xeus"),
    ])
    .await
    .unwrap();
    assert!(ctx.exports().await.iter().any(|x| x.name == "mine"));
    agents
        .ask(agent::DeleteSession { session: chat })
        .await
        .unwrap();
    // The event reaches the subscribed actor, which writes off its mailbox.
    let mut gone = false;
    for _ in 0..100 {
        if removed("x-mine").await.is_some() && removed("x-theirs").await.is_some() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        gone,
        "the deleted chat's exports are tombstoned, whatever their home"
    );
    let names: Vec<String> = ctx.exports().await.into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["away"], "the absent chat's export stays");
}

/// 191 (D2): a block that lands while a pass runs (after it staged its snapshot) is not in
/// the hash the pass keeps, so the next tick reconciles it and the gate refuses the contact.
#[tokio::test]
async fn a_block_landing_mid_pass_is_reconciled_at_the_next_tick() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    paired(&n1, &c).await;
    let id = n1.ctx.contact_list()[0].id.clone();
    let agents = n1.ctx.agents().unwrap();
    *n1.ctx.mid_pass.lock() = Some(Box::new(move || {
        Box::pin(async move {
            agents
                .ask(agent::communication::network::WriteContactRecords(vec![
                    RecordWrite::new(RecordTable::Contacts, id, vec![("blocked", Cell::Int(1))]),
                ]))
                .await
                .unwrap();
        })
    }));
    n1.ctx.reload().await.unwrap();
    assert!(
        matches!(n1.ctx.gate(&c.eid()), IrohGate::Contact { .. }),
        "the pass installed its snapshot, from before the block"
    );
    assert!(
        n1.ctx.tick().await,
        "the block is not in the hash the pass kept"
    );
    assert_eq!(n1.ctx.gate(&c.eid()), IrohGate::Refuse);
}

/// 191 (D2): a `peers.yaml` that no longer parses never stops the records: a block written
/// elsewhere applies at the next tick, the file's problem is in status, and a contact RPC
/// still applies and answers.
#[tokio::test]
async fn a_broken_peers_yaml_never_stops_a_block() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n1, &c).await;
    let path = n1._dir.path().join(crate::config::PEERS_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "peers: [not, a, map\n").unwrap();
    let id = n1.ctx.contact_list()[0].id.clone();
    n1.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(vec![
            RecordWrite::new(RecordTable::Contacts, id, vec![("blocked", Cell::Int(1))]),
        ]))
        .await
        .unwrap();
    assert!(n1.ctx.tick().await);
    assert_eq!(n1.ctx.gate(&c.eid()), IrohGate::Refuse, "the block applied");
    assert!(
        n1.ctx
            .state
            .lock()
            .error
            .as_deref()
            .is_some_and(|e| e.contains("peers.yaml")),
        "the file's problem is reported"
    );
    // One more idempotent pass may follow the commit's own binding changes; then it settles.
    let _ = n1.ctx.tick().await;
    assert!(
        !n1.ctx.tick().await,
        "an unchanged broken file does not run every tick"
    );
    n1.ctx
        .remove_contact(ContactRemove {
            name: alias,
            block: false,
        })
        .await
        .unwrap();
    assert!(n1.ctx.contact_list().is_empty(), "the RPC applied");
}

/// 191 (D4): what the record replica does with `contact_holds` between two machines: every
/// row of `from` on `to`, and the rows `from` deleted gone from `to` (the store's
/// `a_hold_row_and_its_deletion_ride_the_record_replica` proves the replica carries them;
/// this harness runs no replica between its engines). The harness's agents have no mesh id
/// (every store's engine is `""`), so a row arrives owned by `from`'s endpoint id, as a
/// mesh machine's would.
async fn replicate_holds(from: &Engine, to: &Engine) {
    use agent::communication::network::{ContactHoldRowsForTest, WriteContactRecords};
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let src = from
        .ctx
        .agents()
        .unwrap()
        .ask(ContactHoldRowsForTest)
        .await
        .unwrap();
    let dst = to
        .ctx
        .agents()
        .unwrap()
        .ask(ContactHoldRowsForTest)
        .await
        .unwrap();
    let t = |v: &Option<String>| v.clone().map_or(Cell::Null, Cell::Text);
    let i = |v: &Option<i64>| v.map_or(Cell::Null, Cell::Int);
    let owner = from.eid();
    let mut w: Vec<RecordWrite> = dst
        .iter()
        .filter(|d| d.owner.as_deref() == Some(owner.as_str()))
        .filter(|d| !src.iter().any(|s| s.id == d.id))
        .map(|d| RecordWrite::delete(RecordTable::Holds, d.id.clone()))
        .collect();
    for r in &src {
        w.push(RecordWrite::new(
            RecordTable::Holds,
            r.id.clone(),
            vec![
                ("contact", t(&r.contact)),
                ("machine", t(&r.machine)),
                ("export", t(&r.export)),
                ("session", t(&r.session)),
                ("owner", Cell::Text(owner.clone())),
                ("since_ms", i(&r.since_ms)),
                ("waiting", i(&r.waiting)),
                ("preview", t(&r.preview)),
                ("until_ms", i(&r.until_ms)),
            ],
        ));
    }
    to.ctx
        .agents()
        .unwrap()
        .ask(WriteContactRecords(w))
        .await
        .unwrap();
}

/// 191 (rig case 17, D3, D4): a hold answered on another machine. n2 holds C's first message
/// and mirrors it; once the row reaches n1, n1's next holds tick fires `A2aChanged` and
/// lists it (strip and Attention) with no other event. Allow once on n1 writes nothing and is
/// refused; Always on n1 is a grant write that, replicated, makes n2's reconcile release the
/// hold: C's task runs, n2's row goes, and n1's list clears at its next tick. n1 never holds
/// or delivers anything.
#[tokio::test]
async fn a_hold_answered_on_another_machine_is_released_by_its_owner() {
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let n2 = engine("n2", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n2, &c).await;
    publish(
        &n2,
        "research",
        ExportTarget::Agent {
            agent: "theuth".into(),
        },
        "",
    )
    .await;
    n2.ctx
        .update_contact(ContactUpdate {
            name: alias.clone(),
            patch: ContactPatch {
                grant: Some(vec!["research".into()]),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    n2.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap();
    replicate(&n2, &n1).await;
    assert!(n1.ctx.tick().await);
    let heard = Arc::new(parking_lot::Mutex::new(0usize));
    let rec = kameo::actor::Spawn::spawn(Changes(heard.clone()));
    n1.ctx
        .rt
        .subscribe::<Changes, crate::A2aChanged>(&rec)
        .await;
    let _ = n1.ctx.holds_tick().await;
    let before = *heard.lock();

    let client = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
        net: c.net.clone(),
        eid: n2.eid(),
        members: Vec::new(),
        answered: Default::default(),
        hints: reach::Hints {
            addrs: n2.addrs(),
            relay: None,
        },
        export: Some("research".into()),
    }) as Box<dyn a2a_client::Transport>);
    let mut m = a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("hi")]);
    m.message_id = "m-1".into();
    let req = a2a_types::SendMessageRequest {
        message: m,
        configuration: Some(a2a_types::SendMessageConfiguration {
            accepted_output_modes: Some(vec!["text/plain".into()]),
            task_push_notification_config: None,
            history_length: Some(0),
            return_immediately: Some(true),
        }),
        metadata: None,
        tenant: None,
    };
    let first = match client.send_message(&req).await.unwrap() {
        a2a_types::SendMessageResponse::Task(t) => t,
        other => panic!("expected a task, got {other:?}"),
    };
    assert_eq!(first.status.state, a2a_types::TaskState::AuthRequired);
    assert!(
        n1.ctx.first_waiting().await.is_empty(),
        "not replicated yet"
    );
    // P3 review S1: the owner's own row is its memory: never listed back as another's.
    assert!(
        n2.ctx
            .agents()
            .unwrap()
            .ask(agent::communication::network::ContactHolds)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!n2.ctx.first_waiting().await[0].elsewhere);

    // The row reaches n1: its next tick refreshes every surface, with no other event.
    replicate_holds(&n2, &n1).await;
    assert!(n1.ctx.holds_tick().await, "the holds hash changed");
    until("A2aChanged on n1", || {
        (*heard.lock() > before).then_some(())
    })
    .await;
    let on1 = n1.ctx.first_waiting().await;
    assert_eq!(on1.len(), 1, "{on1:?}");
    assert_eq!(
        (&on1[0].contact, on1[0].export.as_str(), on1[0].waiting),
        (&alias, "research", 1)
    );
    assert_eq!(on1[0].preview, "hi");
    assert_eq!(
        on1[0],
        FirstWaiting {
            elsewhere: true,
            ..n2.ctx.first_waiting().await[0].clone()
        },
        "the same hold, held elsewhere"
    );
    let items = FirstContactItems(Arc::downgrade(&n1.ctx)).items().await;
    assert!(
        items
            .iter()
            .any(|i| i.id == format!("contact_first:{alias}:research")),
        "{items:?}"
    );

    // Allow once is the owner's alone: refused on n1, nothing written.
    let once = n1
        .ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer: FirstContact::Ask,
        })
        .await
        .unwrap_err();
    assert!(once.contains("Allow once there"), "{once}");

    // Always on n1: a grant write. Its rows reach n2 as the replica would carry them.
    n1.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer: FirstContact::Allow,
        })
        .await
        .unwrap();
    assert!(
        !n2.ctx.first_waiting().await.is_empty(),
        "n2 still waits: n1 released nothing"
    );
    let t = |v: &Option<String>| v.clone().map_or(Cell::Null, Cell::Text);
    let grants: Vec<RecordWrite> = n1
        .ctx
        .records()
        .await
        .unwrap()
        .grants
        .iter()
        .map(|g| {
            RecordWrite::new(
                RecordTable::Grants,
                g.id.clone(),
                vec![
                    ("contact", t(&g.contact)),
                    ("export", t(&g.export)),
                    ("granted", g.granted.map_or(Cell::Null, Cell::Int)),
                    ("first_contact", t(&g.first_contact)),
                ],
            )
        })
        .collect();
    n2.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(grants))
        .await
        .unwrap();
    assert!(n2.ctx.tick().await, "the grant changed: a reconcile");
    assert!(n2.ctx.first_waiting().await.is_empty(), "released by n2");
    let now = client
        .get_task(&a2a_types::GetTaskRequest {
            id: first.id.clone(),
            history_length: Some(0),
            tenant: None,
        })
        .await
        .unwrap();
    assert_ne!(now.status.state, a2a_types::TaskState::AuthRequired);
    assert_ne!(
        now.status.state,
        a2a_types::TaskState::Failed,
        "{:?}",
        now.status.message.as_ref().and_then(|m| m.text())
    );
    let rows = n2
        .ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::ContactHoldRowsForTest)
        .await
        .unwrap();
    assert!(rows.is_empty(), "n2's row went: {rows:?}");

    // The deletion reaches n1: the strip and the item go there too.
    replicate_holds(&n2, &n1).await;
    assert!(n1.ctx.holds_tick().await);
    assert!(n1.ctx.first_waiting().await.is_empty());
    assert!(
        n1.ctx
            .agents()
            .unwrap()
            .ask(agent::communication::network::ContactsHeld)
            .await
            .unwrap()
            .is_empty(),
        "n1 never held anything"
    );
}

/// 191 (P3 review M2, S4): a hold fails on a block, wherever and however it is written: a
/// stored answer this machine does not know (read as block), a per-export block, and a block
/// of the whole contact on another machine of the mesh, replicated. The owner (n2) fails each
/// at its next reconcile, never starting it; the other machine releases nothing itself.
#[tokio::test]
async fn a_hold_fails_on_a_block_wherever_it_is_written() {
    use agent::communication::network::{ContactsHeld, WriteContactRecords};
    use agent::store::contacts::{Cell, RecordTable, RecordWrite};
    let lan = Lan::default();
    let n2 = engine("n2", &lan, true, cfg(600)).await;
    let c = engine("c", &lan, true, cfg(600)).await;
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let (alias, _) = paired(&n2, &c).await;
    publish(
        &n2,
        "research",
        ExportTarget::Agent {
            agent: "theuth".into(),
        },
        "",
    )
    .await;
    n2.ctx
        .update_contact(ContactUpdate {
            name: alias.clone(),
            patch: ContactPatch {
                grant: Some(vec!["research".into()]),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    let ask = async || {
        n2.ctx
            .write_answer(&alias, "research", FirstContact::Ask)
            .await
            .unwrap();
        let _ = n2.ctx.tick().await;
    };
    ask().await;
    let client = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
        net: c.net.clone(),
        eid: n2.eid(),
        members: Vec::new(),
        answered: Default::default(),
        hints: reach::Hints {
            addrs: n2.addrs(),
            relay: None,
        },
        export: Some("research".into()),
    }) as Box<dyn a2a_client::Transport>);
    let send = async |id: &str| {
        let mut m =
            a2a_types::Message::new(a2a_types::Role::User, vec![a2a_types::Part::text("hi")]);
        m.message_id = id.into();
        let req = a2a_types::SendMessageRequest {
            message: m,
            configuration: Some(a2a_types::SendMessageConfiguration {
                accepted_output_modes: Some(vec!["text/plain".into()]),
                task_push_notification_config: None,
                history_length: Some(0),
                return_immediately: Some(true),
            }),
            metadata: None,
            tenant: None,
        };
        let t = match client.send_message(&req).await.unwrap() {
            a2a_types::SendMessageResponse::Task(t) => t,
            other => panic!("expected a task, got {other:?}"),
        };
        assert_eq!(t.status.state, a2a_types::TaskState::AuthRequired, "{id}");
        t.id
    };
    let failed = async |id: &str| {
        let now = client
            .get_task(&a2a_types::GetTaskRequest {
                id: id.into(),
                history_length: Some(0),
                tenant: None,
            })
            .await
            .unwrap();
        assert_eq!(now.status.state, a2a_types::TaskState::Failed, "{id}");
    };
    let held = async || n2.ctx.agents().unwrap().ask(ContactsHeld).await.unwrap();

    // An answer this machine does not know reads as block.
    let t1 = send("m-1").await;
    let xid = n2.ctx.shared.read().staged.export_ids["research"].clone();
    let grant = n2
        .ctx
        .records()
        .await
        .unwrap()
        .grants
        .into_iter()
        .find(|g| g.export.as_deref() == Some(xid.as_str()))
        .unwrap();
    n2.ctx
        .agents()
        .unwrap()
        .ask(WriteContactRecords(vec![RecordWrite::new(
            RecordTable::Grants,
            grant.id,
            vec![("first_contact", Cell::Text("maybe".into()))],
        )]))
        .await
        .unwrap();
    assert!(n2.ctx.tick().await, "the grant changed: a reconcile");
    assert!(held().await.is_empty(), "failed");
    // A blocked export answers the contact nothing: its task reads again once it is asked.
    ask().await;
    failed(&t1).await;

    // A block of that export alone.
    let t2 = send("m-2").await;
    n2.ctx
        .write_answer(&alias, "research", FirstContact::Block)
        .await
        .unwrap();
    let _ = n2.ctx.tick().await;
    assert!(held().await.is_empty(), "failed");
    ask().await;
    failed(&t2).await;

    // A block of the whole contact on n1, which holds nothing: written there, replicated,
    // and failed here.
    let t3 = send("m-3").await;
    replicate(&n2, &n1).await;
    let _ = n1.ctx.tick().await;
    replicate_holds(&n2, &n1).await;
    let on1 = n1.ctx.first_waiting().await;
    assert!(
        on1.len() == 1 && on1[0].elsewhere && on1[0].contact == alias,
        "{on1:?}"
    );
    n1.ctx
        .first_answer(ContactFirstAnswer {
            contact: alias.clone(),
            export: "research".into(),
            answer: FirstContact::Block,
        })
        .await
        .unwrap();
    assert_eq!(held().await.len(), 1, "n1 released nothing");
    replicate(&n1, &n2).await;
    let _ = n2.ctx.tick().await;
    assert!(held().await.is_empty(), "failed by its owner");
    assert!(
        client
            .get_task(&a2a_types::GetTaskRequest {
                id: t3,
                history_length: Some(0),
                tenant: None,
            })
            .await
            .is_err(),
        "a blocked contact reads nothing"
    );
}

mod forward;
mod owners;
mod rosters;
