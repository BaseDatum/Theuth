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

/// One engine; `live` gives it an agent (then `peers.yaml` reloads, ceremonies pin).
async fn engine(name: &str, lan: &Lan, live: bool, cfg: A2aConfig) -> Engine {
    let dir = tempfile::tempdir().unwrap();
    let net = node(name, dir.path()).await;
    let agents = if live {
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        Some(agent::spawn_engine_agents(rt).await)
    } else {
        None
    };
    let ctx = crate::test_ctx(dir.path(), net.clone(), Arc::new(lan.clone()), cfg, agents).await;
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
    assert_eq!(
        facts.cap,
        tools::Risk::ReadOnly,
        "a new contact is read_only"
    );
    assert!(facts.paired_ms > 0);
    assert!(!a.ctx.window_open(), "success closes the inviter's window");
    assert!(matches!(a.ctx.gate(&b.eid()), IrohGate::Contact { alias: x, .. } if x == alias));
    assert!(matches!(b.ctx.gate(&a.eid()), IrohGate::Contact { .. }));
    let text = std::fs::read_to_string(a._dir.path().join(crate::config::PEERS_FILE)).unwrap();
    assert!(text.contains(&format!("endpoint: {}", b.eid())), "{text}");
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
    a.ctx
        .edit(|file| {
            for name in names {
                file.exports.insert(
                    name.into(),
                    ExportEntry {
                        session: None,
                        agent: Some("theuth".into()),
                        name: name.into(),
                        description: "Answers research questions".into(),
                        tools: crate::config::default_contact_tools(),
                    },
                );
            }
            file.peers.get_mut(&alias).unwrap().exports = names.map(String::from).to_vec();
            Ok(())
        })
        .await
        .unwrap();

    let client_for = |export: Option<&str>| {
        a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
            net: b.net.clone(),
            eid: a.eid(),
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
    a.ctx
        .edit(|file| {
            file.exports.insert(
                "ops-chat".into(),
                ExportEntry {
                    session: Some(chat),
                    agent: None,
                    name: "ops-chat".into(),
                    description: String::new(),
                    tools: crate::config::default_contact_tools(),
                },
            );
            Ok(())
        })
        .await
        .unwrap();
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
    b.ctx
        .edit(|file| {
            file.exports.insert(
                "research".into(),
                ExportEntry {
                    session: None,
                    agent: Some("theuth".into()),
                    name: "research".into(),
                    description: String::new(),
                    tools: crate::config::default_contact_tools(),
                },
            );
            file.peers.get_mut(&alias_of_a).unwrap().exports = vec!["research".into()];
            Ok(())
        })
        .await
        .unwrap();
    let client = a2a_client::A2AClient::new(Box::new(reach::IrohTransport {
        net: a.net.clone(),
        eid: b.eid(),
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
    let path = a._dir.path().join(crate::config::PEERS_FILE);
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains(&stored[0].to_string()), "{file}");
    assert_eq!(gate.hints(&b.eid()), stored, "the dial gets them");

    // Unchanged: no rewrite.
    let before = crate::contacts::store::stamp(&path);
    tokio::time::sleep(Duration::from_millis(20)).await;
    a.ctx.remember_addrs().await;
    assert_eq!(crate::contacts::store::stamp(&path), before);
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

    a.ctx
        .edit(|file| {
            file.peers.get_mut("bob").unwrap().blocked = true;
            Ok(())
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

/// Z3-R3: a put without `tools` (a re-grant, an older client) keeps the export's list; a new
/// export without one starts on the default.
#[tokio::test]
async fn a_put_without_tools_keeps_the_exports_allowlist() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let put = |name: &str, tools: Option<Vec<String>>| Export {
        name: name.into(),
        target: ExportTarget::Agent {
            agent: "theuth".into(),
        },
        description: "Research".into(),
        paused: None,
        tools,
    };
    let tools_of = |name: &'static str| {
        let ctx = a.ctx.clone();
        async move {
            ctx.exports()
                .await
                .into_iter()
                .find(|e| e.name == name)
                .and_then(|e| e.tools)
        }
    };
    a.ctx
        .put_export(put("research", Some(vec!["read_url".into()])))
        .await
        .unwrap();
    a.ctx.put_export(put("research", None)).await.unwrap();
    assert_eq!(
        tools_of("research").await,
        Some(vec!["read_url".to_owned()])
    );
    a.ctx
        .put_export(put("research", Some(Vec::new())))
        .await
        .unwrap();
    assert_eq!(
        tools_of("research").await,
        Some(Vec::new()),
        "an empty list is a list"
    );
    a.ctx.put_export(put("notes", None)).await.unwrap();
    assert_eq!(
        tools_of("notes").await,
        Some(crate::config::default_contact_tools())
    );
    // Z4: a put naming a tool a contact may never be granted is refused, each such tool
    // named, and the export is left as it was.
    let strs = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let e = a
        .ctx
        .put_export(put(
            "research",
            Some(strs(&[
                "read_url",
                "knowledge_put",
                "computer_screenshot",
                "remind_me_in",
                "shell",
            ])),
        ))
        .await
        .unwrap_err();
    for t in [
        "knowledge_put",
        "computer_screenshot",
        "remind_me_in",
        "shell",
    ] {
        assert!(e.contains(t), "{e}");
    }
    assert!(!e.contains("read_url"), "{e}");
    assert_eq!(tools_of("research").await, Some(Vec::new()));
    // The grantable set and a read-only MCP tool's key are accepted.
    let ok = strs(&["read_url", "knowledge_search", "mcp:files/search"]);
    a.ctx
        .put_export(put("research", Some(ok.clone())))
        .await
        .unwrap();
    assert_eq!(tools_of("research").await, Some(ok));
}

/// Pair `a` (inviter) and `b` by ticket; A's alias for B and B's alias for A.
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
            name: "research".into(),
            target: ExportTarget::Agent {
                agent: "theuth".into(),
            },
            description: "Answers research questions".into(),
            paused: None,
            tools: None,
        })
        .await
        .unwrap();
    let grant = |exports: &[&str]| {
        a.ctx.update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                exports: Some(exports.iter().map(|s| s.to_string()).collect()),
                ..ContactPatch::default()
            },
        })
    };
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
    grant(&[]).await.unwrap();
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
/// stops waiting. A changed cap fails a waiting message at once (review 1: `cap_changed` →
/// `RebindHeld` with nothing kept).
#[tokio::test]
async fn a_grant_change_keeps_a_waiting_first_message_and_always_releases_it() {
    let lan = Lan::default();
    let a = engine("a", &lan, true, cfg(600)).await;
    let b = engine("b", &lan, true, cfg(600)).await;
    let (a_alias, _) = paired(&a, &b).await;
    for name in ["research", "zeta"] {
        a.ctx
            .put_export(Export {
                name: name.into(),
                target: ExportTarget::Agent {
                    agent: "theuth".into(),
                },
                description: String::new(),
                paused: None,
                tools: None,
            })
            .await
            .unwrap();
    }
    let grant = |exports: &[&str]| {
        a.ctx.update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                exports: Some(exports.iter().map(|s| s.to_string()).collect()),
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
            name: "research".into(),
            target: ExportTarget::Agent {
                agent: "theuth".into(),
            },
            description: "Answers research questions".into(),
            paused: None,
            tools: None,
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

    // A changed cap: the next waiting message fails at once, it never runs under the new cap.
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
        .update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                cap: Some(tools::Risk::Mutating),
                ..ContactPatch::default()
            },
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
