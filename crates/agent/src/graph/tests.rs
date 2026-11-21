use super::*;

const H: u64 = 3_600_000;
const NOW: u64 = 1_800_000_000_000;

fn at(ms_: u64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(ms_ as i64).unwrap()
}

fn row(parent: Option<SessionId>, updated: u64) -> SessionSummary {
    SessionSummary {
        id: SessionId::new(),
        created_at: at(updated.saturating_sub(1000)),
        updated_at: at(updated),
        finished_at: None,
        title: "t".into(),
        tags: vec![],
        model: "m".into(),
        parent,
        name: parent.map(|_| "kid".into()),
        effort: None,
        agent: None,
        cwd: None,
        harness_model: None,
        context_size: None,
        desk: None,
        pinned: false,
        archived: false,
        project: None,
        notebook: None,
        retention: tools::Retention::Durable,
        running: false,
        owner: String::new(),
        epoch: 1,
        here: true,
        kind: SessionKind::Chat,
        memory: Default::default(),
        observed: None,
    }
}

fn observed_meta(state: ObservedState) -> ObservedMeta {
    serde_json::from_value(serde_json::json!({
        "source": "omp_task", "external": "x", "state": state, "cost_rule": "inside_parent",
        "can_cancel": false, "description": null, "tool_call": null, "stop": null, "detail": null
    }))
    .unwrap()
}

#[test]
fn node_kind_covers_every_row() {
    let root = row(None, NOW);
    assert_eq!(node_kind(&root), Some(GraphNodeKind::Root));
    let mut desk = row(None, NOW);
    desk.desk = Some("ops".into());
    assert_eq!(node_kind(&desk), Some(GraphNodeKind::Desk));
    let child = row(Some(root.id), NOW);
    assert_eq!(node_kind(&child), Some(GraphNodeKind::Child));
    let mut acp = row(Some(root.id), NOW);
    acp.agent = Some("claude".into());
    assert_eq!(node_kind(&acp), Some(GraphNodeKind::Child));
    let mut obs = row(Some(acp.id), NOW);
    obs.kind = SessionKind::Observed;
    assert_eq!(node_kind(&obs), Some(GraphNodeKind::Observed));
    let mut adv = row(Some(root.id), NOW);
    adv.kind = SessionKind::Advisor;
    assert_eq!(node_kind(&adv), None);
    let mut ask = row(None, NOW);
    ask.kind = SessionKind::Ask;
    assert_eq!(node_kind(&ask), None);
}

#[test]
fn outcome_wait_endpoint_pair_and_title() {
    assert_eq!(outcome_of("error"), Some(GraphOutcome::Failed));
    assert_eq!(outcome_of("cancelled"), Some(GraphOutcome::Cancelled));
    assert_eq!(outcome_of("timed_out"), Some(GraphOutcome::TimedOut));
    for r in ["interrupted", "suspended", "harness_continued"] {
        assert_eq!(outcome_of(r), None, "{r}");
    }
    assert_eq!(outcome_of("end_turn"), Some(GraphOutcome::Finished));
    assert_eq!(wait_of("question"), Some(GraphWait::User));
    assert_eq!(wait_of("job"), Some(GraphWait::Job));
    assert_eq!(wait_of("pty_wait"), Some(GraphWait::Job));
    assert_eq!(wait_of("child_result"), Some(GraphWait::Children));
    assert_eq!(wait_of("peer_reply"), None);

    let u = SessionId::new();
    assert_eq!(endpoint(&format!("session:{u}")), Some(u.to_string()));
    assert_eq!(endpoint("peer:mini"), Some("peer:mini".into()));
    assert_eq!(
        endpoint(&format!("peer:alice/session:{u}")),
        Some("peer:alice".into())
    );
    assert_eq!(endpoint("parent"), None);
    assert_eq!(endpoint("session:nope"), None);
    assert_eq!(endpoint("peer:"), None);

    assert_eq!(pair("b", "a"), ("a|b".into(), "a".into(), "b".into()));
    assert_eq!(pair("a", "b"), pair("b", "a"));

    let mut desk = row(None, NOW);
    desk.title = String::new();
    desk.desk = Some("ops".into());
    assert_eq!(title_of(&desk), "ops desk");
    let mut bare = row(None, NOW);
    bare.title = String::new();
    assert_eq!(title_of(&bare), bare.id.to_string()[..8]);
    assert_eq!(title_of(&row(Some(bare.id), NOW)), "kid");
}

#[test]
fn select_window_rules() {
    let window = 24 * H;
    let recent = row(None, NOW - H);
    let old_child = row(Some(recent.id), NOW - 48 * H);
    let old_root = row(None, NOW - 48 * H);
    let recent_child = row(Some(old_root.id), NOW - H);
    let live_root = row(None, NOW - 72 * H);
    let inc_root = row(None, NOW - 90 * H);
    let inc_child = row(Some(inc_root.id), NOW - 90 * H);
    let mut private = row(None, NOW);
    private.retention = tools::Retention::Private;
    let mut advisor = row(Some(recent.id), NOW);
    advisor.kind = SessionKind::Advisor;
    let gone = row(None, NOW - 100 * H);
    let all = vec![
        recent.clone(),
        old_child.clone(),
        old_root.clone(),
        recent_child.clone(),
        live_root.clone(),
        inc_root.clone(),
        inc_child.clone(),
        private.clone(),
        advisor.clone(),
        gone.clone(),
    ];
    let live: HashSet<SessionId> = [live_root.id].into();
    let (ids, older) = select_window(
        &all,
        &HashMap::new(),
        &live,
        &[inc_child.id],
        NOW,
        window,
        100,
    );
    let set: HashSet<SessionId> = ids.iter().copied().collect();
    assert!(set.contains(&recent.id));
    assert!(
        !set.contains(&old_child.id),
        "an old child of an active root stays out"
    );
    assert!(
        set.contains(&old_root.id) && set.contains(&recent_child.id),
        "the root comes with an entered node"
    );
    assert!(set.contains(&live_root.id));
    assert!(
        set.contains(&inc_child.id) && set.contains(&inc_root.id),
        "included + ancestors"
    );
    assert!(!set.contains(&private.id) && !set.contains(&advisor.id) && !set.contains(&gone.id));
    assert_eq!(older, None);
    let pos = |id: SessionId| ids.iter().position(|x| *x == id).unwrap();
    assert!(pos(old_root.id) < pos(recent_child.id), "parents first");

    // A clock entry beats updated_at.
    let last: HashMap<SessionId, u64> = [(gone.id, NOW - H)].into();
    let (ids, _) = select_window(&all, &last, &live, &[], NOW, window, 100);
    assert!(ids.contains(&gone.id));

    // Over the cap: idle trees fold oldest first; live and included trees never fold.
    let a = row(None, NOW - 3 * H);
    let b = row(None, NOW - 2 * H);
    let c = row(None, NOW - H);
    let all = vec![
        a.clone(),
        b.clone(),
        c.clone(),
        live_root.clone(),
        inc_root.clone(),
    ];
    let (ids, older) = select_window(&all, &HashMap::new(), &live, &[inc_root.id], NOW, window, 3);
    assert_eq!(
        older,
        Some(GraphOlder {
            roots: 2,
            nodes: 2,
            newest_ms: NOW - 2 * H
        })
    );
    let set: HashSet<SessionId> = ids.into_iter().collect();
    assert_eq!(set, [c.id, live_root.id, inc_root.id].into());
}

fn ex(id: &str, from: &str, to: &str, req: &str, state: &str, at_s: u32) -> ExchangeEdgeRow {
    ExchangeEdgeRow {
        id: id.into(),
        sender: from.into(),
        recipient: to.into(),
        sender_session: None,
        recipient_session: None,
        request_id: req.into(),
        state: state.into(),
        created_at: format!("2027-01-01T00:00:{at_s:02}+00:00"),
        updated_at: format!("2027-01-01T00:00:{at_s:02}+00:00"),
    }
}

#[test]
fn aggregate_counts_pairs() {
    let (p, c) = (SessionId::new(), SessionId::new());
    let (sp, sc) = (format!("session:{p}"), format!("session:{c}"));
    let rows = vec![
        ex("x4", &sc, &sp, "result:t1", "accepted", 40),
        ex("x3", &sp, &sc, "r3", "consumed", 30),
        ex("x2", &sp, "peer:alice/session:abc", "r2", "held", 20),
        ex("x1", &sp, "parent", "r1", "replied", 10),
    ];
    let (edges, peers) = aggregate(&rows, 10);
    assert_eq!(peers, ["peer:alice"]);
    assert_eq!(edges.len(), 2);
    let pc = edges
        .iter()
        .find(|e| e.id == pair(&p.to_string(), &c.to_string()).0)
        .unwrap();
    let (ab, ba) = if pc.a == p.to_string() {
        (pc.ab, pc.ba)
    } else {
        (pc.ba, pc.ab)
    };
    assert_eq!((ab, ba), (1, 1), "p→c once, c→p once");
    assert_eq!((pc.results, pc.open), (1, 1));
    assert_eq!(
        (pc.last_from.as_str(), pc.last_id.as_str()),
        (c.to_string().as_str(), "x4")
    );
    let pa = edges
        .iter()
        .find(|e| e.b == "peer:alice" || e.a == "peer:alice")
        .unwrap();
    assert_eq!(pa.open, 1);
    let (edges, peers) = aggregate(&rows, 1);
    assert_eq!(edges.len(), 1, "MAX_EDGES keeps the newest pairs");
    assert!(peers.is_empty());
}

fn model_with(nodes: &[GraphNode]) -> Model {
    let mut m = Model::empty(NOW, 24 * H);
    for n in nodes {
        m.nodes.insert(n.id.clone(), n.clone());
    }
    m.published = m.nodes.clone();
    m
}

fn gnode(id: &str, parent: Option<&str>) -> GraphNode {
    let mut n = peer_node(id, NOW - H);
    n.kind = if parent.is_some() {
        GraphNodeKind::Child
    } else {
        GraphNodeKind::Root
    };
    n.parent = parent.map(str::to_owned);
    n.remote = false;
    n
}

#[test]
fn coalescer_spacing_equality_and_resync() {
    let (p, c) = (SessionId::new(), SessionId::new());
    let mut m = model_with(&[
        gnode(&p.to_string(), None),
        gnode(&c.to_string(), Some(&p.to_string())),
    ]);
    // The first mark schedules now; later marks do not reschedule.
    assert_eq!(m.mark(&p.to_string(), NOW), Some(NOW));
    assert_eq!(m.mark(&c.to_string(), NOW + 5), None);
    assert_eq!(m.mark("unknown", NOW + 6), None);
    let batch = m.begin_flush(NOW);
    assert_eq!(batch.sessions(&m).len(), 2);
    let running = SessionActivity {
        state: AgentState::Running,
        live: true,
        ..SessionActivity::default()
    };
    let act: HashMap<SessionId, SessionActivity> = [(p, running.clone()), (c, running)].into();
    let ev = m
        .finish_flush(batch, &act, &HashMap::new(), &HashMap::new(), NOW)
        .unwrap();
    assert_eq!((ev.seq, ev.nodes.len()), (1, 2), "one delta with both");
    assert_eq!(ev.nodes[0].id, p.to_string(), "parents first");
    // The next flush waits FLUSH_MS after the last.
    assert_eq!(m.mark(&p.to_string(), NOW + 10), Some(NOW + FLUSH_MS));
    let batch = m.begin_flush(NOW + FLUSH_MS);
    // Same facts: nothing visible changed, nothing published.
    assert_eq!(
        m.finish_flush(
            batch,
            &act,
            &HashMap::new(),
            &HashMap::new(),
            NOW + FLUSH_MS
        ),
        None
    );
    assert_eq!(m.seq, 1);

    // Delta-class: re-published only once the published clock is QUIET_BUMP_MS old.
    let t = NOW + 2 * FLUSH_MS;
    assert_eq!(
        m.bump(&p.to_string(), t),
        Some(t),
        "published clock is NOW - 1h"
    );
    let batch = m.begin_flush(t);
    let last: HashMap<SessionId, u64> = [(p, t)].into();
    let ev = m
        .finish_flush(batch, &act, &HashMap::new(), &last, t)
        .unwrap();
    assert_eq!(ev.nodes[0].last_event_ms, t);
    assert_eq!(
        m.bump(&p.to_string(), t + 1000),
        None,
        "within 30 s: not dirty"
    );
    assert!(m.flush_at.is_none());

    // Over MAX_UPSERTS: resync, all lists empty.
    let many: Vec<GraphNode> = (0..=MAX_UPSERTS)
        .map(|_| gnode(&SessionId::new().to_string(), None))
        .collect();
    let mut m = Model::empty(NOW, 24 * H);
    let t = NOW + 10_000;
    for n in &many {
        m.nodes.insert(n.id.clone(), n.clone());
        m.mark(&n.id, t);
    }
    let batch = m.begin_flush(t);
    let ev = m
        .finish_flush(batch, &HashMap::new(), &HashMap::new(), &HashMap::new(), t)
        .unwrap();
    assert!(ev.resync && ev.nodes.is_empty() && ev.edges.is_empty() && ev.pulses.is_empty());
}

#[test]
fn peer_messages_make_edges_and_pulses() {
    let (p, c) = (SessionId::new(), SessionId::new());
    let (ps, cs) = (p.to_string(), c.to_string());
    let mut m = model_with(&[gnode(&ps, None), gnode(&cs, Some(&ps))]);
    let id = Uuid::now_v7();
    let ev = |state, result| PeerMessageChanged {
        session: p,
        id,
        state,
        from: format!("session:{c}"),
        to: format!("session:{p}"),
        result,
    };
    m.peer(&ev(PeerMessageState::Accepted, true), NOW);
    m.peer(&ev(PeerMessageState::Accepted, true), NOW); // the other participant's copy
    let b = m.begin_flush(NOW);
    let d = m
        .finish_flush(b, &HashMap::new(), &HashMap::new(), &HashMap::new(), NOW)
        .unwrap();
    assert_eq!(d.edges.len(), 1);
    let e = &d.edges[0];
    assert_eq!((e.results, e.open, e.ab + e.ba), (1, 1, 1));
    assert_eq!(e.last_from, cs);
    assert_eq!(
        d.pulses.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [GraphPulseKind::Result]
    );
    m.peer(&ev(PeerMessageState::Consumed, true), NOW + 300);
    m.peer(&ev(PeerMessageState::Replied, true), NOW + 300);
    let b = m.begin_flush(NOW + 300);
    let d = m
        .finish_flush(
            b,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            NOW + 300,
        )
        .unwrap();
    assert_eq!(d.edges[0].open, 0, "closed once per id");
    assert_eq!(
        d.pulses.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [GraphPulseKind::Delivered, GraphPulseKind::Replied]
    );

    // A plain message to a boundary peer: a peer node and a `sent` pulse.
    let id2 = Uuid::now_v7();
    m.peer(
        &PeerMessageChanged {
            session: p,
            id: id2,
            state: PeerMessageState::Accepted,
            from: format!("session:{p}"),
            to: "peer:alice".into(),
            result: false,
        },
        NOW + 1000,
    );
    let b = m.begin_flush(NOW + 1000);
    let d = m
        .finish_flush(
            b,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            NOW + 1000,
        )
        .unwrap();
    assert!(
        d.nodes
            .iter()
            .any(|n| n.id == "peer:alice" && n.kind == GraphNodeKind::Peer)
    );
    assert_eq!(d.pulses[0].kind, GraphPulseKind::Sent);

    // Deleting the child removes it and its edge.
    m.remove(&cs, NOW + 2000);
    let b = m.begin_flush(NOW + 2000);
    let d = m
        .finish_flush(
            b,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            NOW + 2000,
        )
        .unwrap();
    assert_eq!(d.removed, std::slice::from_ref(&cs));
    assert_eq!(d.edges_removed, [pair(&ps, &cs).0]);
}

#[test]
fn reply_cursor_children_lease_and_prune() {
    let p = SessionId::new();
    let mut m = model_with(&[gnode(&p.to_string(), None)]);
    let full = m.reply(None, NOW);
    assert!(!full.unchanged && full.nodes.len() == 1 && full.seq == 0 && full.epoch_ms == NOW);
    let same = m.reply(
        Some(GraphCursor {
            epoch_ms: NOW,
            seq: 0,
        }),
        NOW,
    );
    assert!(same.unchanged && same.nodes.is_empty());
    let stale = m.reply(
        Some(GraphCursor {
            epoch_ms: NOW - 1,
            seq: 0,
        }),
        NOW,
    );
    assert!(!stale.unchanged && stale.nodes.len() == 1);

    let c = SessionId::new();
    assert!(m.child(p, c, "builder", None, Some(&"m2".to_string()), None, NOW));
    assert!(!m.child(
        SessionId::new(),
        SessionId::new(),
        "x",
        None,
        None,
        None,
        NOW
    ));
    let o = SessionId::new();
    assert!(m.child(
        c,
        o,
        "explore",
        None,
        None,
        Some(&observed_meta(ObservedState::Completed)),
        NOW
    ));
    let b = m.begin_flush(NOW);
    let d = m
        .finish_flush(b, &HashMap::new(), &HashMap::new(), &HashMap::new(), NOW)
        .unwrap();
    assert_eq!(
        d.nodes.len(),
        2,
        "the child and the observed child (the parent is unchanged)"
    );
    let obs = d.nodes.iter().find(|n| n.id == o.to_string()).unwrap();
    assert_eq!(
        (obs.kind, obs.outcome),
        (GraphNodeKind::Observed, Some(GraphOutcome::Finished))
    );
    assert_eq!(obs.root, p.to_string());

    assert!(!lease_expired(
        NOW + LEASE_SECS * 1000,
        NOW + LEASE_SECS * 1000
    ));
    assert!(lease_expired(
        NOW + LEASE_SECS * 1000,
        NOW + LEASE_SECS * 1000 + 1
    ));

    // Prune: idle leaves out of the window go, bottom-up; included ones stay.
    let later = NOW + 30 * H;
    m.include(&[p]);
    m.prune(later);
    assert_eq!(m.nodes.len(), 1, "only the included root is left");
}

#[test]
fn observed_state_is_the_harness_view_unless_approval() {
    let o = observed_meta(ObservedState::Running);
    assert_eq!(observed_state(&o, &AgentState::Idle), AgentState::Running);
    let a = AgentState::Approval {
        tool: "bash".into(),
    };
    assert_eq!(observed_state(&o, &a), a);
    assert_eq!(
        observed_state(&observed_meta(ObservedState::Failed), &AgentState::Idle),
        AgentState::Error
    );
}

/// 208 §6 (actor): a seed through a live `Agents`, a delta for a spawned child, an `unchanged` renewal.
#[tokio::test]
async fn actor_seeds_publishes_one_delta_and_renews_unchanged() {
    use std::sync::{Arc, Mutex};
    #[derive(kameo::Actor)]
    struct Seen(Arc<Mutex<Vec<AgentGraphChanged>>>);
    impl Message<AgentGraphChanged> for Seen {
        type Reply = ();
        async fn handle(&mut self, ev: AgentGraphChanged, _: &mut Context<Self, ()>) {
            self.0.lock().unwrap().push(ev);
        }
    }
    let scratch = crate::TestDir::new("graph-actor");
    let rt = Runtime::new(figment::Figment::new(), scratch.0.clone());
    let a = crate::supervisor::bare_agents(rt.clone(), None, false).await;
    let store = a.store.clone();
    let agents = Agents::spawn(a);
    let root = SessionId::new();
    store
        .create_session(root, "m", None, None, None)
        .await
        .unwrap();
    let graph = Graph::spawn_with_mailbox(
        Graph::new(rt.clone(), store.clone(), agents),
        runtime::subscriber_mailbox(),
    );
    Graph::subscribe(&rt, &graph).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sub = Seen::spawn(Seen(seen.clone()));
    rt.subscribe::<Seen, AgentGraphChanged>(&sub).await;

    let g = graph
        .ask(Snapshot {
            window_ms: None,
            include: vec![],
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!((g.seq, g.unchanged), (0, false));
    assert!(
        g.nodes
            .iter()
            .any(|n| n.id == root.to_string() && n.kind == GraphNodeKind::Root)
    );

    let child = SessionId::new();
    rt.publish(AgentEvent::SubagentSpawned {
        parent: root,
        child,
        name: "builder".into(),
        cap: None,
        agent: None,
        model: Some("m".into()),
    })
    .await;
    rt.publish(AgentEvent::TurnStarted {
        session: child,
        turn: crate::TurnId::new(),
        text: "go".into(),
        model: "m".into(),
        turn_seq: 0,
        channel: None,
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one coalesced delta: {got:?}");
    let ids: HashSet<String> = got[0].nodes.iter().map(|n| n.id.clone()).collect();
    assert!(
        ids.contains(&child.to_string()),
        "the spawned child is in the delta"
    );
    assert_eq!(got[0].seq, 1);

    let renew = graph
        .ask(Snapshot {
            window_ms: None,
            include: vec![],
            cursor: Some(GraphCursor {
                epoch_ms: g.epoch_ms,
                seq: 1,
            }),
        })
        .await
        .unwrap();
    assert!(renew.unchanged && renew.nodes.is_empty());
}

/// The `rpc-compose` golden samples of `agent.graph` and `agent.peer-message` are what these types parse and emit
/// (the golden runner reaches them only after `agent.events`, red on main).
#[test]
fn wire_fixtures_round_trip() {
    fn check<E: serde::de::DeserializeOwned + Serialize>(name: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rpc-compose/tests/fixtures/wire")
            .join(format!("{name}.json"));
        let samples: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for s in samples {
            let ev: E = serde_json::from_value(s["input"].clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&ev).unwrap(),
                s["notification"]["params"]["result"],
                "{name}"
            );
        }
    }
    check::<AgentGraphChanged>("agent.graph");
    check::<PeerMessageChanged>("agent.peer-message");
}
