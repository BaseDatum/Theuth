//! Native collaborative live Decks plugin (design doc 229).
//!
//! Combines:
//! - Collaborative pure-Yjs Deck sections (`DocKind::Deck` in `crates/document`).
//! - Review-gated Monty Python scripts executing inside an isolated per-Deck `SessionId`
//!   via `agent::repl` with full live `ToolRegistry::Invoke` & configured MCP namespace.
//! - Scheduled one-shot collection, persistent cooperative streaming (`on_start` + `on_step`),
//!   interactive declared actions (`on_action`), durable last-good snapshots vs transient
//!   live frames, and bounded per-Deck coalesced subscriptions (`deck_subscribe`).

#![allow(missing_docs)]

pub mod example;
pub mod hub;
pub mod model;
pub mod owner;
pub mod rpc;
pub mod runtime;
pub mod store;
pub mod tools;

use ::runtime::Runtime;
use ::tools::{ApprovalRequested, ApprovalResolved, ToolRegistry};
use kameo::actor::Spawn;

pub use model::{
    DeckChanged, DeckDetail, DeckRuntimeEvent, DeckRuntimeStatus, DeckSnapshot, DeckSummary,
    DeclaredAction, InteractErrorCode, InteractResponse, LiveFrame, PendingApproval,
    RuntimeControlOp, RuntimeState, ScriptMode, ScriptRevision, ScriptStatus,
};
pub use owner::{DECKS_ACTOR, DeckApprovalWatcher, DeckSupervisor};
pub use rpc::rpc_module;
pub use tools::DECKS_SKILL;

/// Declare wire contracts emitted by the Decks plugin.
/// Note: high-frequency live frames NEVER cross the global `RuntimeWire` — only coarse `DeckChanged`
/// list notifications do; live frames travel over per-Deck bounded `deck_subscribe` channels.
pub async fn declare_wire(rt: &Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<DeckChanged>().await
}

/// First-class native Decks plugin.
pub struct DecksPlugin;

#[async_trait::async_trait]
impl ::runtime::Plugin for DecksPlugin {
    fn name(&self) -> &'static str {
        "decks"
    }

    async fn declare_wire(&mut self, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let enabled = rt
            .config
            .extract_inner::<bool>("decks.enabled")
            .unwrap_or(true);
        if !enabled {
            tracing::info!("decks disabled via decks.enabled = false");
            return Ok(());
        }

        let store = match store::DeckStore::open(&rt.data_dir).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("decks store unavailable: {e:#}");
                return Ok(());
            }
        };

        let Some(doc_actor) = rt
            .lookup_when::<document::DocumentActor>(document::DOCUMENT_ACTOR)
            .await
        else {
            tracing::warn!("decks: document actor unavailable; decks disabled");
            return Ok(());
        };
        let Some(registry) = rt
            .lookup_when::<ToolRegistry>(::tools::REGISTRY_ACTOR)
            .await
        else {
            tracing::warn!("decks: tool registry unavailable; decks disabled");
            return Ok(());
        };

        let (sup, shared) =
            DeckSupervisor::new(store, rt.clone(), doc_actor, registry.clone(), None);
        let actor = DeckSupervisor::spawn(sup);
        if let Err(e) = actor.register(DECKS_ACTOR) {
            tracing::warn!("decks: failed to register actor: {e:#}");
            return Ok(());
        }
        let watcher =
            DeckApprovalWatcher::spawn(DeckApprovalWatcher::with_supervisor(shared, actor.clone()));
        rt.subscribe::<DeckApprovalWatcher, ApprovalRequested>(&watcher)
            .await;
        rt.subscribe::<DeckApprovalWatcher, ApprovalResolved>(&watcher)
            .await;
        rt.subscribe::<DeckApprovalWatcher, document::DocumentChanged>(&watcher)
            .await;
        tools::register(&registry, actor.clone(), rt.clone()).await;

        // 229 H3: resume previously enabled streaming/scheduled Decks cleanly after boot.
        let _ = actor.tell(owner::RecoverEnabledDecksMsg).send().await;
        tracing::info!("decks plugin started");
        Ok(())
    }
}

#[cfg(all(test, feature = "inproc-monty"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use ::runtime::Runtime;
    use ::tools::{
        Approval, ApprovalRequested, ApprovalResolved, Decision, RegisterTool, Resolve, Risk, Tool,
        ToolCx, ToolOutput, ToolRegistry,
    };
    use document::{DocumentActor, DocumentsConfig, Principal};
    use kameo::actor::{ActorRef, Spawn};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    use crate::model::{
        DeckRuntimeEvent, DeclaredAction, InteractErrorCode, RuntimeControlOp, RuntimeState,
        ScriptMode, ScriptStatus,
    };
    use crate::owner::{
        ApproveScriptMsg, CreateDeckMsg, DeckApprovalWatcher, DeckSupervisor, GetDeck, InteractMsg,
        ProposeScriptMsg, RuntimeControlMsg, SubscribeDeckMsg, pending_approval_id,
    };
    use crate::store::DeckStore;

    static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct TestSecrets;
    impl ::tools::secrets::Secrets for TestSecrets {
        fn expand(&self, _args: &mut serde_json::Value) -> Result<usize, String> {
            Ok(0)
        }
        fn redact<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
            if text.contains("tok-deck-secret-99999") {
                std::borrow::Cow::Owned(
                    text.replace("tok-deck-secret-99999", "[REDACTED:DECK_TOKEN]"),
                )
            } else {
                std::borrow::Cow::Borrowed(text)
            }
        }
        fn redact_value(&self, v: &mut serde_json::Value) {
            match v {
                Value::String(s) => {
                    if let std::borrow::Cow::Owned(redacted) = self.redact(s) {
                        *s = redacted;
                    }
                }
                Value::Array(arr) => {
                    for item in arr {
                        self.redact_value(item);
                    }
                }
                Value::Object(map) => {
                    for val in map.values_mut() {
                        self.redact_value(val);
                    }
                }
                _ => {}
            }
        }
    }

    struct Harness {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
        rt: Runtime,
        sup: ActorRef<DeckSupervisor>,
        shared: crate::owner::SharedDeckState,
        registry: ActorRef<ToolRegistry>,
        approval: ActorRef<Approval>,
    }

    async fn evict_actor<A: kameo::Actor>(name: &str) {
        if let Ok(Some(actor)) = ActorRef::<A>::lookup(name) {
            actor.kill();
            actor.wait_for_shutdown().await;
        }
    }

    async fn setup_harness() -> Harness {
        let _lock = TEST_LOCK.lock().await;
        ::tools::secrets::install(Arc::new(TestSecrets));

        evict_actor::<ToolRegistry>(::tools::REGISTRY_ACTOR).await;
        evict_actor::<Approval>(::tools::APPROVAL_ACTOR).await;
        evict_actor::<::tools::AskUser>(::tools::ASK_ACTOR).await;
        evict_actor::<::tools::Jobs>(::tools::JOBS_ACTOR).await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let rt = Runtime::new(figment::Figment::new(), data_dir.clone());
        let registry = ::tools::registry::start(&rt).await.unwrap();
        let approval = ActorRef::<Approval>::lookup(::tools::APPROVAL_ACTOR)
            .unwrap()
            .expect("approval actor");

        let doc_store = document::store::Store::open(&data_dir).await.unwrap();
        let (ev_tx, _ev_rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = document::helper::Helper::new(ev_tx, rt.shutdown.clone());
        let doc_actor = DocumentActor::spawn(DocumentActor::new(
            doc_store,
            helper,
            DocumentsConfig::default(),
            rt.clone(),
        ));

        let deck_store = DeckStore::open(&data_dir).await.unwrap();
        let pool = agent::repl::test_pool(4).await.expect("inproc test pool");
        let (sup_inst, shared) = DeckSupervisor::new(
            deck_store,
            rt.clone(),
            doc_actor,
            registry.clone(),
            Some(pool),
        );
        let sup = DeckSupervisor::spawn(sup_inst);
        let watcher = DeckApprovalWatcher::spawn(DeckApprovalWatcher::with_supervisor(
            shared.clone(),
            sup.clone(),
        ));
        rt.subscribe::<DeckApprovalWatcher, ApprovalRequested>(&watcher)
            .await;
        rt.subscribe::<DeckApprovalWatcher, ApprovalResolved>(&watcher)
            .await;
        rt.subscribe::<DeckApprovalWatcher, document::DocumentChanged>(&watcher)
            .await;
        // Let bus registration settle.
        tokio::time::sleep(Duration::from_millis(20)).await;

        Harness {
            _lock,
            _tmp: tmp,
            rt,
            sup,
            shared,
            registry,
            approval,
        }
    }

    async fn new_deck(h: &Harness, title: &str) -> String {
        let d = h
            .sup
            .ask(CreateDeckMsg {
                title: Some(title.into()),
                description: None,
                example: false,
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        d.doc.doc_id
    }

    // ---- Test 1: Scheduled one-shot success, failure keeps last-good & marks stale ----
    #[tokio::test]
    async fn scheduled_oneshot_success_failure_keeps_last_good_and_marks_stale() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Scheduled Health").await;

        let script = r#"
runs = 0
async def on_start(last_state):
    global runs
    if isinstance(last_state, dict):
        runs = int(last_state.get("runs", 0))

async def collect():
    global runs
    runs += 1
    deck_checkpoint({"runs": runs})
    if runs == 2:
        raise RuntimeError("upstream timeout")
    deck_emit({"metric": runs * 100, "ok": True}, durable=True)
"#;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: Some(60),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "v1".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(rev.status, ScriptStatus::PendingReview);

        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Run 1 succeeds -> durable snapshot persisted, stale = false.
        let st1 = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::RunOnce,
            })
            .send()
            .await
            .unwrap();
        assert!(!st1.stale);
        assert_eq!(st1.consecutive_failures, 0);
        let snap1_id = st1
            .last_ok_snapshot_id
            .clone()
            .expect("last_ok_snapshot_id");

        let detail1 = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        assert_eq!(detail1.last_snapshot.as_ref().unwrap().data["metric"], 100);

        // Run 2 fails -> last-good snapshot MUST be preserved AND stale MUST be true.
        let run2 = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::RunOnce,
            })
            .send()
            .await;
        assert!(run2.is_err(), "expected failure on run 2");

        let detail2 = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        assert!(
            detail2.runtime.stale,
            "deck must be marked stale after failure"
        );
        assert_eq!(detail2.runtime.consecutive_failures, 1);
        assert!(
            detail2
                .runtime
                .last_error
                .as_deref()
                .unwrap_or("")
                .contains("upstream timeout")
        );
        assert_eq!(
            detail2.last_snapshot.as_ref().unwrap().snapshot_id,
            snap1_id,
            "last-good snapshot must be preserved on failure"
        );
        assert_eq!(detail2.last_snapshot.as_ref().unwrap().data["metric"], 100);
    }

    // ---- Test 2: Persistent globals across steps & actions + single-flight interleaving ----
    #[tokio::test]
    async fn persistent_globals_across_steps_and_actions_single_flight() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Persistent Globals").await;

        let script = r#"
counter = 0
in_flight = 0
max_overlap = 0

async def on_start(last_state):
    global counter
    counter = 10

async def on_step():
    global counter, in_flight, max_overlap
    in_flight += 1
    if in_flight > max_overlap:
        max_overlap = in_flight
    counter += 1
    deck_emit({"counter": counter, "max_overlap": max_overlap}, durable=True)
    in_flight -= 1
    deck_sleep(60.0)

async def on_action(action, args, ctx):
    global counter, in_flight, max_overlap
    in_flight += 1
    if in_flight > max_overlap:
        max_overlap = in_flight
    delta = int(args.get("delta", 1))
    counter += delta
    res = {"counter": counter, "max_overlap": max_overlap}
    in_flight -= 1
    return res
"#;
        let actions = vec![DeclaredAction {
            name: "bump".into(),
            title: "Bump".into(),
            description: "Add delta to global counter".into(),
            schema: json!({
                "type": "object",
                "properties": { "delta": { "type": "integer", "minimum": 1, "maximum": 100 } },
                "required": ["delta"],
                "additionalProperties": false
            }),
            mutates_state: true,
            allow_while_paused: true,
        }];
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: actions,
                mode: ScriptMode::Hybrid,
                interval_secs: Some(60),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "globals".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Start runs on_start (counter=10) + first on_step (counter=11) with ZERO views subscribed.
        let st = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(st.state, RuntimeState::Streaming);
        assert!(st.checkout_alive);

        // Fire 3 concurrent actions + 1 RunOnce step: single-flight owner serializes all of them!
        let f1 = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "r1".into(),
                caller_id: "c1".into(),
                action: "bump".into(),
                args: json!({"delta": 5}),
                expected_rev: Some(1),
                timeout_ms: Some(10_000),
            })
            .send();
        let f2 = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "r2".into(),
                caller_id: "c2".into(),
                action: "bump".into(),
                args: json!({"delta": 7}),
                expected_rev: Some(1),
                timeout_ms: Some(10_000),
            })
            .send();
        let f3 = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::RunOnce,
            })
            .send();
        let (r1, r2, r3) = tokio::join!(f1, f2, f3);
        let r1 = r1.unwrap();
        let r2 = r2.unwrap();
        assert!(r1.ok && r2.ok);
        assert!(r3.is_ok());

        // Final counter MUST be 10 (on_start) + 1 (initial step) + 5 + 7 + 1 (RunOnce) = 24,
        // and max_overlap MUST be 1 (never concurrent execution!).
        let final_check = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "r_check".into(),
                caller_id: "c1".into(),
                action: "bump".into(),
                args: json!({"delta": 1}),
                expected_rev: Some(1),
                timeout_ms: Some(10_000),
            })
            .send()
            .await
            .unwrap();
        assert!(final_check.ok);
        let res = final_check.result.unwrap();
        assert_eq!(res["counter"], 25);
        assert_eq!(
            res["max_overlap"], 1,
            "Python must never execute concurrently"
        );
    }

    // ---- Test 3: Stream controls (approval gate, Pause retains checkout, Resume wakes, Stop checkpoints/fences) ----
    #[tokio::test]
    async fn stream_controls_approval_gate_pause_resume_stop_epoch() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Stream Controls").await;

        let script = r#"
ticks = 0
async def on_step():
    global ticks
    ticks += 1
    deck_emit({"ticks": ticks}, durable=False)
    deck_sleep(0.15)
"#;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: vec![],
                mode: ScriptMode::Streaming,
                interval_secs: Some(1),
                schedule_cron: None,
                proposed_by: "session:agent".into(),
                change_summary: "pending".into(),
            })
            .send()
            .await
            .unwrap();

        // Pending revision MUST NOT start or run_once.
        let blocked = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await;
        assert!(blocked.is_err(), "pending_review script must not start");

        // Owner approves -> Start succeeds and streams even with 0 subscribers.
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        let st_start = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(st_start.state, RuntimeState::Streaming);
        assert!(st_start.checkout_alive);
        let epoch1 = st_start.run_epoch;

        // Wait briefly for streaming steps with 0 views open.
        tokio::time::sleep(Duration::from_millis(350)).await;

        // Pause retains checkout & globals!
        let st_pause = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Pause,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(st_pause.state, RuntimeState::Paused);
        assert!(st_pause.checkout_alive, "Pause must retain live checkout");
        let steps_at_pause = st_pause.step_count;
        assert!(
            steps_at_pause >= 2,
            "stream should have advanced with 0 views open"
        );

        tokio::time::sleep(Duration::from_millis(250)).await;
        let detail_paused = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        assert_eq!(detail_paused.runtime.step_count, steps_at_pause);

        // Resume wakes the retained checkout without resetting epoch.
        let st_resume = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Resume,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(st_resume.state, RuntimeState::Streaming);
        assert_eq!(st_resume.run_epoch, epoch1);
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Stop checkpoints, releases checkout, and bumps run_epoch.
        let st_stop = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Stop,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(st_stop.state, RuntimeState::Stopped);
        assert!(!st_stop.checkout_alive, "Stop must release checkout");
        assert!(st_stop.step_count > steps_at_pause);
        assert!(
            st_stop.run_epoch > epoch1,
            "Stop must fence with a new run_epoch"
        );
    }

    // ---- Test 4: Declared actions, JSON Schema, private result vs shared emit, idempotency ----
    #[tokio::test]
    async fn declared_actions_schema_privacy_explicit_emit_and_idempotency() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Actions Privacy").await;

        let script = r#"
calls = 0
async def on_step():
    deck_sleep(60.0)

async def on_action(action, args, ctx):
    global calls
    calls += 1
    if action == "private_query":
        return {"secret_answer": f"for-{ctx['caller_id']}-{args['q']}", "calls": calls}
    if action == "broadcast_update":
        deck_emit({"shared_msg": args["msg"], "calls": calls}, durable=True)
        return {"emitted": True, "calls": calls}
    raise ValueError("unknown")
"#;
        let actions = vec![
            DeclaredAction {
                name: "private_query".into(),
                title: "Private Query".into(),
                description: "Returns private result only".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "q": { "type": "string", "minLength": 1 } },
                    "required": ["q"],
                    "additionalProperties": false
                }),
                mutates_state: true,
                allow_while_paused: true,
            },
            DeclaredAction {
                name: "broadcast_update".into(),
                title: "Broadcast".into(),
                description: "Explicitly emits shared update".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } },
                    "required": ["msg"],
                    "additionalProperties": false
                }),
                mutates_state: true,
                allow_while_paused: true,
            },
        ];
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: actions,
                mode: ScriptMode::Hybrid,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "actions".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Subscribe a viewer before running actions.
        let (_hub, _sub_id, mut rx) = h
            .sup
            .ask(SubscribeDeckMsg {
                deck_id: deck_id.clone(),
            })
            .send()
            .await
            .unwrap();
        // Consume initial Sync.
        assert!(matches!(
            rx.recv().await,
            Some(DeckRuntimeEvent::Sync { .. })
        ));

        // Undeclared action rejected.
        let undecl = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "u1".into(),
                caller_id: "alice".into(),
                action: "evil_eval".into(),
                args: json!({}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(!undecl.ok);
        assert_eq!(undecl.error_code, Some(InteractErrorCode::UndeclaredAction));

        // Invalid schema args rejected.
        let bad_args = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "b1".into(),
                caller_id: "alice".into(),
                action: "private_query".into(),
                args: json!({"q": ""}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(!bad_args.ok);
        assert_eq!(bad_args.error_code, Some(InteractErrorCode::InvalidArgs));

        // Revision mismatch rejected.
        let rev_err = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "rm1".into(),
                caller_id: "alice".into(),
                action: "private_query".into(),
                args: json!({"q": "hi"}),
                expected_rev: Some(99),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(
            rev_err.error_code,
            Some(InteractErrorCode::RevisionMismatch)
        );

        // Private query returns to caller ONLY — subscriber receives NO Snapshot/LiveFrame!
        let priv_resp = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "idem-1".into(),
                caller_id: "alice".into(),
                action: "private_query".into(),
                args: json!({"q": "salary"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(priv_resp.ok && priv_resp.private && !priv_resp.cached);
        assert_eq!(
            priv_resp.result.as_ref().unwrap()["secret_answer"],
            "for-alice-salary"
        );
        assert_eq!(priv_resp.result.as_ref().unwrap()["calls"], 1);

        // Drain any Status event and verify NO Snapshot/LiveFrame carried the private result!
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(80), rx.recv()).await {
            assert!(
                !matches!(
                    ev,
                    DeckRuntimeEvent::Snapshot { .. } | DeckRuntimeEvent::LiveFrame { .. }
                ),
                "private action return value must NEVER be broadcast: {ev:?}"
            );
        }

        // Idempotency: replaying same request_id returns cached response without incrementing `calls`.
        let replay = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "idem-1".into(),
                caller_id: "alice".into(),
                action: "private_query".into(),
                args: json!({"q": "salary"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(replay.ok && replay.cached);
        assert_eq!(replay.result.as_ref().unwrap()["calls"], 1);

        // Explicit `deck_emit` inside action DOES broadcast to subscribers!
        let bcast = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "bcast-1".into(),
                caller_id: "bob".into(),
                action: "broadcast_update".into(),
                args: json!({"msg": "hello team"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(bcast.ok);
        assert_eq!(bcast.result.as_ref().unwrap()["calls"], 2);

        let mut saw_snap = false;
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            if let DeckRuntimeEvent::Snapshot { snapshot } = ev
                && snapshot.data["shared_msg"] == "hello team"
            {
                saw_snap = true;
                break;
            }
        }
        assert!(
            saw_snap,
            "explicit deck_emit in action must broadcast snapshot"
        );
    }

    // ---- Test 5: Full session MCP tool exposure, approval pause/resume, secret redaction ----
    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct MockFetchArgs {
        topic: String,
    }
    struct MockMcpFetchTool;
    #[async_trait::async_trait]
    impl Tool for MockMcpFetchTool {
        const NAME: &'static str = "mcp_acme_fetch";
        const RISK: Risk = Risk::ReadOnly;
        const RETURNS: &'static str = "dict";
        type Args = MockFetchArgs;
        fn description(&self) -> String {
            "Mock MCP read tool".into()
        }
        async fn call(&self, a: MockFetchArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
            let data = json!({
                "topic": a.topic,
                "secret_leak": "tok-deck-secret-99999"
            });
            Ok(ToolOutput {
                structured: Some(data),
                ..ToolOutput::ok("fetched")
            })
        }
    }

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct MockDeployArgs {
        env: String,
    }
    struct MockMcpDeployTool(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl Tool for MockMcpDeployTool {
        const NAME: &'static str = "mcp_acme_deploy";
        const RISK: Risk = Risk::Mutating;
        const RETURNS: &'static str = "dict";
        type Args = MockDeployArgs;
        fn description(&self) -> String {
            "Mock gated MCP deploy tool".into()
        }
        async fn call(&self, a: MockDeployArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let data = json!({ "deployed": a.env });
            Ok(ToolOutput {
                structured: Some(data),
                ..ToolOutput::ok("deployed")
            })
        }
    }

    #[tokio::test]
    async fn full_session_mcp_tool_exposure_approval_pause_and_secret_redaction() {
        let h = setup_harness().await;

        let deploy_count = Arc::new(AtomicUsize::new(0));
        h.registry
            .ask(RegisterTool(Arc::new(MockMcpFetchTool)))
            .await
            .unwrap();
        h.registry
            .ask(RegisterTool(Arc::new(MockMcpDeployTool(
                deploy_count.clone(),
            ))))
            .await
            .unwrap();

        let deck_id = new_deck(&h, "MCP & Approvals").await;

        let script = r#"
async def on_step():
    avail = tools("mcp_acme")
    fetched = await mcp_acme_fetch(topic="ops")
    deck_emit({"tools_found": len(avail), "fetched": fetched}, durable=True)

async def on_action(action, args, ctx):
    if action == "deploy":
        res = await mcp_acme_deploy(env=args["env"])
        return {"deploy_result": res}
    raise ValueError("unknown")
"#;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: vec![DeclaredAction {
                    name: "deploy".into(),
                    title: "Deploy".into(),
                    description: "Calls gated MCP deploy tool".into(),
                    schema: json!({
                        "type": "object",
                        "properties": { "env": { "type": "string" } },
                        "required": ["env"],
                        "additionalProperties": false
                    }),
                    mutates_state: true,
                    allow_while_paused: true,
                }],
                mode: ScriptMode::Hybrid,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "mcp".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // 1. Read-only MCP tool executes via normal ToolRegistry::Invoke AND secret is redacted!
        let st = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::RunOnce,
            })
            .send()
            .await
            .unwrap();
        assert!(!st.stale);
        let detail = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let snap = detail.last_snapshot.unwrap();
        assert_eq!(snap.data["tools_found"], 2);
        let snap_text = snap.data.to_string();
        assert!(
            !snap_text.contains("tok-deck-secret-99999"),
            "secrets must be redacted from Deck snapshots: {snap_text}"
        );

        // 2. Gated Mutating MCP tool pauses for human approval, surfaces ApprovalPending on deck_subscribe,
        //    and resumes cleanly once resolved with Allow!
        let (_hub, _sub_id, mut rx) = h
            .sup
            .ask(SubscribeDeckMsg {
                deck_id: deck_id.clone(),
            })
            .send()
            .await
            .unwrap();
        let _ = rx.recv().await; // Sync

        let sup_clone = h.sup.clone();
        let deck_clone = deck_id.clone();
        let action_task = tokio::spawn(async move {
            sup_clone
                .ask(InteractMsg {
                    deck_id: deck_clone,
                    request_id: "deploy-1".into(),
                    caller_id: "owner".into(),
                    action: "deploy".into(),
                    args: json!({"env": "staging"}),
                    expected_rev: Some(1),
                    timeout_ms: Some(10_000),
                })
                .send()
                .await
                .unwrap()
        });

        // Wait for ApprovalPending event on the per-Deck subscription.
        let approval_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(DeckRuntimeEvent::ApprovalPending { approval }) => {
                        assert_eq!(approval.tool, "mcp_acme_deploy");
                        return approval.approval_id;
                    }
                    Some(_) => {}
                    None => panic!("deck subscription closed before ApprovalPending"),
                }
            }
        })
        .await
        .expect("expected ApprovalPending event");

        assert_eq!(pending_approval_id(&h.shared, &deck_id), Some(approval_id));
        assert_eq!(deploy_count.load(Ordering::SeqCst), 0);

        // Human approves the gated tool call -> Deck resumes and finishes action.
        let resolved = h
            .approval
            .ask(Resolve {
                id: approval_id,
                decision: Decision::Allow,
            })
            .send()
            .await
            .unwrap();
        assert!(resolved);

        let resp = tokio::time::timeout(Duration::from_secs(5), action_task)
            .await
            .expect("action_task timed out after approval")
            .unwrap();
        assert!(resp.ok, "action should succeed after approval: {resp:?}");
        assert_eq!(deploy_count.load(Ordering::SeqCst), 1);
        assert_eq!(resp.result.unwrap()["deploy_result"]["deployed"], "staging");
        let _ = h.rt;
    }

    // ---- Test 6: Checkpoint recovery (warm .monty restore vs cold on_start(last_state) fallback) ----
    #[tokio::test]
    async fn checkpoint_recovery_warm_restore_and_cold_fallback() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Checkpoint Recovery").await;

        let script_v1 = r#"
ephemeral_heap = "uninitialized"
durable_counter = 0

async def on_start(last_state):
    global ephemeral_heap, durable_counter
    ephemeral_heap = "created_in_v1_heap"
    if isinstance(last_state, dict):
        durable_counter = int(last_state.get("durable_counter", 0))

async def on_step():
    global durable_counter
    durable_counter += 1
    deck_checkpoint({"durable_counter": durable_counter})
    deck_emit({"ephemeral_heap": ephemeral_heap, "durable_counter": durable_counter}, durable=True)
    deck_sleep(60.0)
"#;
        let rev1 = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script_v1.into(),
                declared_actions: vec![],
                mode: ScriptMode::Hybrid,
                interval_secs: Some(60),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "v1".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev1.rev,
                expected_digest: rev1.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Start (runs on_start + step 1 -> durable_counter=1) then Stop (saves .monty dump).
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Stop,
            })
            .send()
            .await
            .unwrap();

        // Start again on SAME rev1 -> Warm .monty restore preserves `ephemeral_heap` without re-running on_start,
        // and step 2 increments durable_counter to 2!
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        let detail_warm = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let snap_warm = detail_warm.last_snapshot.unwrap();
        assert_eq!(snap_warm.data["ephemeral_heap"], "created_in_v1_heap");
        assert_eq!(snap_warm.data["durable_counter"], 2);

        // Upgrade to rev2 (incompatible sha/rev): Approve stops rev1 and leaves rev2 Stopped (B3 manual-only);
        // explicit owner Start then cold-starts v2's `on_start(last_state)` receiving durable_counter=2!
        let script_v2 = r#"
ephemeral_heap = "v2_fresh"
durable_counter = 0

async def on_start(last_state):
    global ephemeral_heap, durable_counter
    ephemeral_heap = "cold_started_v2"
    if isinstance(last_state, dict):
        durable_counter = int(last_state.get("durable_counter", 0))

async def on_step():
    global durable_counter
    durable_counter += 10
    deck_checkpoint({"durable_counter": durable_counter})
    deck_emit({"ephemeral_heap": ephemeral_heap, "durable_counter": durable_counter}, durable=True)
    deck_sleep(60.0)
"#;
        let rev2 = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script_v2.into(),
                declared_actions: vec![],
                mode: ScriptMode::Hybrid,
                interval_secs: Some(60),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "v2 upgrade".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev2.rev,
                expected_digest: rev2.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Approve-only must stop old rev1 and NOT auto-start rev2 (229 B3).
        tokio::time::sleep(Duration::from_millis(40)).await;
        let detail_approved = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        assert_eq!(detail_approved.runtime.state, RuntimeState::Stopped);

        // Explicit owner Start runs cold fallback on rev2.
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();

        let detail_v2 = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let snap_v2 = detail_v2.last_snapshot.unwrap();
        assert_eq!(snap_v2.script_rev, 2);
        assert_eq!(snap_v2.data["ephemeral_heap"], "cold_started_v2");
        assert_eq!(snap_v2.data["durable_counter"], 12);
    }

    // ---- Test 7: Built-in Workspace Pulse (Example) Deck ----
    #[tokio::test]
    async fn workspace_pulse_example_validates_surface_and_runs_after_approval() {
        let h = setup_harness().await;
        let detail = h
            .sup
            .ask(CreateDeckMsg {
                title: None,
                description: None,
                example: true,
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        assert_eq!(detail.doc.title, "Workspace Pulse (Example)");
        assert_eq!(detail.doc.sections.len(), 3);
        assert!(detail.active_script.is_none());
        let pending = detail.pending_script.as_ref().unwrap();
        assert_eq!(pending.status, ScriptStatus::PendingReview);

        // Validate the native json-render section against crates/surface parser/validator!
        let surface_sec = &detail.doc.sections[0];
        let parsed = surface::parse(&surface_sec.content);
        assert!(
            parsed.is_ok(),
            "example json-render section must pass surface::parse: {:?}",
            parsed.err()
        );

        // Approve & start example script, then run interactive actions.
        let deck_id = detail.doc.doc_id.clone();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: pending.rev,
                expected_digest: pending.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();
        let st = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(st.state, RuntimeState::Streaming);

        let triage = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "ex-1".into(),
                caller_id: "owner".into(),
                action: "triage_incident".into(),
                args: json!({"severity": "P1", "note": "mitigated"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(triage.ok);
        assert_eq!(triage.result.unwrap()["open_p1_remaining"], 2);

        let priv_tools = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "ex-2".into(),
                caller_id: "owner".into(),
                action: "inspect_tools".into(),
                args: json!({}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(priv_tools.ok && priv_tools.private);
    }

    // ---- Test 8 (B1): Owner-only & human-press RPC authorization ----
    #[tokio::test]
    async fn b1_rpc_auth_rejects_guest_internal_closed_forwarded_and_missing_peer() {
        use crate::rpc::{DecksApiServer, human_press, owner_only};
        use jsonrpsee::Extensions;
        use rpc_core::RpcPeer;

        // 1. Missing peer -> rejected by both owner_only and human_press.
        let empty = Extensions::new();
        assert!(owner_only(&empty).is_err());
        assert!(human_press(&empty).is_err());

        // 2. Share guest peer -> rejected by both owner_only and human_press.
        let (guest_peer, _guest_guard) = RpcPeer::open_guest([1u8; 32], [2u8; 32]);
        let mut guest_ext = Extensions::new();
        guest_ext.insert(guest_peer);
        assert!(owner_only(&guest_ext).is_err());
        assert!(human_press(&guest_ext).is_err());

        // 3. Engine-internal peer -> rejected by human_press.
        let mut internal_ext = Extensions::new();
        internal_ext.insert(RpcPeer::internal());
        assert!(human_press(&internal_ext).is_err());

        // 4. Closed owner peer -> rejected by both.
        let (closed_peer, closed_guard) = RpcPeer::open();
        drop(closed_guard);
        let mut closed_ext = Extensions::new();
        closed_ext.insert(closed_peer);
        assert!(owner_only(&closed_ext).is_err());
        assert!(human_press(&closed_ext).is_err());

        // 5. Forwarded owner peer -> rejected by human_press.
        let (owner_peer, _owner_guard) = RpcPeer::open();
        let mut forwarded_ext = Extensions::new();
        forwarded_ext.insert(owner_peer.clone());
        let from = serde_json::from_value(json!("other-engine")).unwrap();
        forwarded_ext.insert(agent::rpc_forward::Forwarded(from));
        assert!(human_press(&forwarded_ext).is_err());

        // 6. Live human owner peer -> accepted by both.
        let mut owner_ext = Extensions::new();
        owner_ext.insert(owner_peer);
        assert!(owner_only(&owner_ext).is_ok());
        assert!(human_press(&owner_ext).is_ok());

        // 7. End-to-end RPC server methods reject guest/internal/forwarded BEFORE hitting actor.
        let srv = rpc_core::Server::new(Runtime::new(Default::default(), std::env::temp_dir()));
        fn assert_auth_err<T: std::fmt::Debug>(res: Result<T, jsonrpsee::types::ErrorObjectOwned>) {
            let code = res.unwrap_err().code();
            assert!(
                matches!(code, -32001 | -32002),
                "expected auth rejection (-32001/-32002), got {code}"
            );
        }
        assert_auth_err(srv.deck_list(&guest_ext, None).await);
        assert_auth_err(srv.deck_snapshots(&guest_ext, "d1".into(), None).await);
        assert_auth_err(
            srv.deck_script_approve(&internal_ext, "d1".into(), 1, "abc".into())
                .await,
        );
        assert_auth_err(
            srv.deck_script_approve(&forwarded_ext, "d1".into(), 1, "abc".into())
                .await,
        );
        assert_auth_err(
            srv.deck_script_reject(&internal_ext, "d1".into(), 1, None)
                .await,
        );
        assert_auth_err(
            srv.deck_runtime_control(&forwarded_ext, "d1".into(), RuntimeControlOp::Start)
                .await,
        );
        assert_auth_err(
            srv.deck_interact(
                &guest_ext,
                "d1".into(),
                "act".into(),
                json!({}),
                "req-1".into(),
                None,
                None,
            )
            .await,
        );
    }

    // ---- Test 9 (B2): Direct SQLite forgery & .monty file bytes tampering rejected ----
    #[tokio::test]
    async fn b2_sqlite_forgery_and_monty_bytes_tampering_rejected() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Integrity").await;

        let script = r#"
origin = "init"
count = 0

async def on_start(last_state):
    global origin, count
    origin = "cold_started"
    if isinstance(last_state, dict):
        count = int(last_state.get("count", 0))

async def on_step():
    global origin, count
    count += 1
    deck_checkpoint({"count": count})
    deck_emit({"origin": origin, "count": count}, durable=True)
    origin = "warm_heap"
    deck_sleep(60.0)
"#;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: vec![DeclaredAction {
                    name: "ping".into(),
                    title: "Ping".into(),
                    description: "Safe read-only ping".into(),
                    schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                    mutates_state: false,
                    allow_while_paused: true,
                }],
                mode: ScriptMode::Hybrid,
                interval_secs: Some(60),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "v1".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Start then Stop so a genuine `.monty` dump is persisted with its SHA-256.
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Stop,
            })
            .send()
            .await
            .unwrap();

        let rec = h.shared.store.get_deck(&deck_id).await.unwrap().unwrap();
        let dump_path = h.shared.store.monty_dump_path(rec.session_id);
        assert!(
            dump_path.exists(),
            "expected .monty dump on disk after Stop"
        );

        // Part A: Tamper with `.monty` bytes on disk -> must reject warm restore, delete tampered file,
        // and fall back cleanly to cold `on_start(last_state)`!
        let mut tampered = std::fs::read(&dump_path).unwrap();
        tampered.extend_from_slice(b"FORGED-MONTY-BYTES");
        std::fs::write(&dump_path, tampered).unwrap();

        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();
        let after_tamper = h
            .sup
            .ask(GetDeck {
                deck_id: deck_id.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let snap = after_tamper.last_snapshot.unwrap();
        assert_eq!(
            snap.data["origin"], "cold_started",
            "tampered .monty bytes MUST trigger cold on_start fallback instead of warm restore"
        );
        assert_eq!(snap.data["count"], 2);

        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Stop,
            })
            .send()
            .await
            .unwrap();

        // Part B: Direct SQLite forgery of `code` -> rejected on load and execution!
        sqlx::query("UPDATE deck_script_revisions SET code = ?1 WHERE deck_id = ?2 AND rev = 1")
            .bind("async def on_step():\n    deck_emit({'pwned': True}, durable=True)\n")
            .bind(&deck_id)
            .execute(h.shared.store.pool_for_test())
            .await
            .unwrap();
        let forged_code_err = h.shared.store.get_active_script(&deck_id).await;
        assert!(
            forged_code_err.is_err(),
            "SQLite code tampering MUST fail integrity verification"
        );
        let run_forged = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::RunOnce,
            })
            .send()
            .await;
        assert!(
            run_forged.is_err(),
            "tampered active script must not execute"
        );

        // Part C: Restore `code` and tamper ONLY `declared_actions_json` -> canonical `revision_sha256` fails!
        sqlx::query(
            "UPDATE deck_script_revisions SET code = ?1, declared_actions_json = ?2 WHERE deck_id = ?3 AND rev = 1",
        )
        .bind(script)
        .bind("[]")
        .bind(&deck_id)
        .execute(h.shared.store.pool_for_test())
        .await
        .unwrap();
        let forged_meta_err = h.shared.store.get_active_script(&deck_id).await;
        assert!(
            forged_meta_err.is_err(),
            "SQLite declared_actions tampering MUST fail canonical revision_sha256 verification"
        );
    }

    // ---- Test 10 (B3): Approval digest binding, pending-only, manual-only, agent runtime gate ----
    #[tokio::test]
    async fn b3_approval_digest_binding_pending_only_manual_only_and_agent_runtime_gate() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Approval Gate").await;

        let rev1 = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: "async def on_step():\n    deck_emit({'v': 1}, durable=True)\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Streaming,
                interval_secs: Some(5),
                schedule_cron: None,
                proposed_by: "agent".into(),
                change_summary: "v1".into(),
            })
            .send()
            .await
            .unwrap();

        // Mid-approval swap: wrong expected_digest MUST fail.
        let wrong_digest = h
            .sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev1.rev,
                expected_digest: "0000000000000000000000000000000000000000000000000000000000000000"
                    .into(),
                approved_by: "owner".into(),
            })
            .send()
            .await;
        assert!(
            wrong_digest.is_err(),
            "wrong expected_digest must be rejected"
        );

        // Propose rev2 -> rev1 becomes Superseded; approving Superseded rev1 MUST fail.
        let rev2 = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: "async def on_step():\n    deck_emit({'v': 2}, durable=True)\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Streaming,
                interval_secs: Some(5),
                schedule_cron: None,
                proposed_by: "agent".into(),
                change_summary: "v2".into(),
            })
            .send()
            .await
            .unwrap();
        assert_ne!(rev1.revision_sha256, rev2.revision_sha256);

        let approve_superseded = h
            .sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev1.rev,
                expected_digest: rev1.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await;
        assert!(
            approve_superseded.is_err(),
            "superseded revision must not be approvable"
        );

        // Approve rev2 with exact digest -> succeeds, but MUST NOT enable schedule/stream ("Approve only").
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev2.rev,
                expected_digest: rev2.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();
        let rec = h.shared.store.get_deck(&deck_id).await.unwrap().unwrap();
        assert!(
            !rec.schedule_enabled,
            "approve-only must not enable schedule"
        );
        assert!(!rec.stream_enabled, "approve-only must not enable stream");

        // Rejecting an already-approved revision MUST fail.
        let reject_approved = h
            .sup
            .ask(crate::owner::RejectScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev2.rev,
                reason: Some("too late".into()),
            })
            .send()
            .await;
        assert!(
            reject_approved.is_err(),
            "cannot reject an already-approved revision"
        );

        // Propose rev3, reject it, then ensure approving Rejected rev3 MUST fail.
        let rev3 = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: "async def on_step():\n    deck_emit({'v': 3}, durable=True)\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: Some(60),
                schedule_cron: None,
                proposed_by: "agent".into(),
                change_summary: "v3".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(crate::owner::RejectScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev3.rev,
                reason: Some("nope".into()),
            })
            .send()
            .await
            .unwrap();
        let approve_rejected = h
            .sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev3.rev,
                expected_digest: rev3.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await;
        assert!(
            approve_rejected.is_err(),
            "rejected revision must not be approvable"
        );

        // AgentRuntimeOp MUST reject `start` and `resume`!
        assert!(serde_json::from_str::<crate::tools::AgentRuntimeOp>("\"start\"").is_err());
        assert!(serde_json::from_str::<crate::tools::AgentRuntimeOp>("\"resume\"").is_err());
        assert_eq!(
            serde_json::from_str::<crate::tools::AgentRuntimeOp>("\"pause\"").unwrap(),
            crate::tools::AgentRuntimeOp::Pause
        );
        assert_eq!(
            serde_json::from_str::<crate::tools::AgentRuntimeOp>("\"stop\"").unwrap(),
            crate::tools::AgentRuntimeOp::Stop
        );
        assert_eq!(
            serde_json::from_str::<crate::tools::AgentRuntimeOp>("\"run_once\"").unwrap(),
            crate::tools::AgentRuntimeOp::RunOnce
        );
    }

    // ---- Test 11 (B4): No supervisor deadlock during approval + re-entrant deck_* blocked ----
    #[tokio::test]
    async fn b4_supervisor_does_not_deadlock_during_tool_approval_and_blocks_reentrant_deck_tools()
    {
        let h = setup_harness().await;
        let deploy_count = Arc::new(AtomicUsize::new(0));
        h.registry
            .ask(RegisterTool(Arc::new(MockMcpFetchTool)))
            .await
            .unwrap();
        h.registry
            .ask(RegisterTool(Arc::new(MockMcpDeployTool(
                deploy_count.clone(),
            ))))
            .await
            .unwrap();
        // Register real deck_* tools into the registry so we verify they are filtered & blocked inside Deck scripts.
        crate::tools::register(&h.registry, h.sup.clone(), h.rt.clone()).await;

        let deck_a = new_deck(&h, "Blocked Deck A").await;
        let deck_b = new_deck(&h, "Healthy Deck B").await;

        let script_a = r#"
async def on_step():
    res = await mcp_acme_deploy(env="prod")
    deck_emit({"deployed": res}, durable=True)
"#;
        let rev_a = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_a.clone(),
                code: script_a.into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "gated".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_a.clone(),
                rev: rev_a.rev,
                expected_digest: rev_a.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        let (_hub, _sub_id, mut rx_a) = h
            .sup
            .ask(SubscribeDeckMsg {
                deck_id: deck_a.clone(),
            })
            .send()
            .await
            .unwrap();
        let _ = rx_a.recv().await; // Sync

        let sup_clone = h.sup.clone();
        let deck_a_clone = deck_a.clone();
        let run_task = tokio::spawn(async move {
            sup_clone
                .ask(RuntimeControlMsg {
                    deck_id: deck_a_clone,
                    op: RuntimeControlOp::RunOnce,
                })
                .send()
                .await
        });

        let approval_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx_a.recv().await {
                    Some(DeckRuntimeEvent::ApprovalPending { approval }) => {
                        return approval.approval_id;
                    }
                    Some(_) => {}
                    None => panic!("subscription closed"),
                }
            }
        })
        .await
        .expect("expected ApprovalPending on Deck A");

        // While Deck A is blocked on human approval, GetDeck(A), ListDecks, SubscribeDeck(A),
        // and GetDeck(B) MUST all complete immediately without deadlocking the supervisor!
        let (det_a, list, sub2, det_b) = tokio::time::timeout(Duration::from_secs(2), async {
            let det_a = h
                .sup
                .ask(GetDeck {
                    deck_id: deck_a.clone(),
                    by: Principal::Owner { device: None },
                })
                .send()
                .await
                .unwrap();
            let list = h
                .sup
                .ask(crate::owner::ListDecks { limit: None })
                .send()
                .await
                .unwrap();
            let sub2 = h
                .sup
                .ask(SubscribeDeckMsg {
                    deck_id: deck_a.clone(),
                })
                .send()
                .await
                .unwrap();
            let det_b = h
                .sup
                .ask(GetDeck {
                    deck_id: deck_b.clone(),
                    by: Principal::Owner { device: None },
                })
                .send()
                .await
                .unwrap();
            (det_a, list, sub2, det_b)
        })
        .await
        .expect("supervisor deadlocked while Deck A waited for approval");
        assert_eq!(det_a.runtime.state, RuntimeState::WaitingApproval);
        assert!(list.len() >= 2);
        let _ = sub2;
        assert_eq!(det_b.doc.doc_id, deck_b);

        // Deny approval so Deck A unwinds cleanly.
        h.approval
            .ask(Resolve {
                id: approval_id,
                decision: Decision::Deny,
            })
            .send()
            .await
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), run_task)
            .await
            .expect("run_task should complete after deny");

        // Part B: Deck B verifies `deck_*` & `show_deck` & `python` are excluded from `tools()`
        // while normal MCP tools remain callable, AND calling `deck_list()` is rejected cleanly (never deadlocks)!
        let script_b = r#"
async def on_step():
    all_tools = tools()
    has_deck_tool = any(t.startswith("deck_") or t == "show_deck" or t == "python" for t in all_tools)
    fetched = await mcp_acme_fetch(topic="healthy")
    deck_emit({
        "has_deck_tool": has_deck_tool,
        "fetched_topic": fetched["topic"],
    }, durable=True)
"#;
        let rev_b = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_b.clone(),
                code: script_b.into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "reentrant check".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_b.clone(),
                rev: rev_b.rev,
                expected_digest: rev_b.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();
        let st_b = tokio::time::timeout(
            Duration::from_secs(5),
            h.sup
                .ask(RuntimeControlMsg {
                    deck_id: deck_b.clone(),
                    op: RuntimeControlOp::RunOnce,
                })
                .send(),
        )
        .await
        .expect("Deck B should run cleanly")
        .unwrap();
        assert!(!st_b.stale);
        let det_b2 = h
            .sup
            .ask(GetDeck {
                deck_id: deck_b.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let snap_b = det_b2.last_snapshot.unwrap();
        assert_eq!(snap_b.data["has_deck_tool"], false);
        assert_eq!(snap_b.data["fetched_topic"], "healthy");

        // Attempting to invoke `deck_list()` inside a Deck script MUST fail cleanly without deadlocking!
        let rev_b2 = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_b.clone(),
                code: "async def on_step():\n    await deck_list()\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "attempt reentrant deck_list".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_b.clone(),
                rev: rev_b2.rev,
                expected_digest: rev_b2.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();
        let reentrant_res = tokio::time::timeout(
            Duration::from_secs(5),
            h.sup
                .ask(RuntimeControlMsg {
                    deck_id: deck_b.clone(),
                    op: RuntimeControlOp::RunOnce,
                })
                .send(),
        )
        .await
        .expect("reentrant deck_list() call must never deadlock");
        assert!(
            reentrant_res.is_err(),
            "reentrant deck_list() must be rejected"
        );
    }

    // ---- Test 12 (B7): Idempotency privacy (caller-scoped + conflict on mismatch) ----
    #[tokio::test]
    async fn b7_idempotency_is_caller_scoped_and_rejects_mismatched_reuse() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Idempotency Privacy").await;

        let script = r#"
calls = 0
async def on_action(action, args, ctx):
    global calls
    calls += 1
    return {"who": ctx["caller_id"], "action": action, "q": args.get("q", ""), "calls": calls}
"#;
        let actions = vec![
            DeclaredAction {
                name: "query_a".into(),
                title: "Query A".into(),
                description: "Private query A".into(),
                schema: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
                mutates_state: false,
                allow_while_paused: true,
            },
            DeclaredAction {
                name: "query_b".into(),
                title: "Query B".into(),
                description: "Private query B".into(),
                schema: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
                mutates_state: false,
                allow_while_paused: true,
            },
        ];
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: actions,
                mode: ScriptMode::Hybrid,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "idem".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // Caller Alice calls with request_id "req-shared".
        let r_alice = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "req-shared".into(),
                caller_id: "owner:alice".into(),
                action: "query_a".into(),
                args: json!({"q": "secret-alice"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(r_alice.ok && !r_alice.cached);
        assert_eq!(r_alice.result.as_ref().unwrap()["who"], "owner:alice");
        assert_eq!(r_alice.result.as_ref().unwrap()["calls"], 1);

        // Caller Bob uses the SAME request_id "req-shared" -> MUST NOT receive Alice's cached result!
        let r_bob = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "req-shared".into(),
                caller_id: "owner:bob".into(),
                action: "query_a".into(),
                args: json!({"q": "secret-alice"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(r_bob.ok && !r_bob.cached);
        assert_eq!(r_bob.result.as_ref().unwrap()["who"], "owner:bob");
        assert_eq!(r_bob.result.as_ref().unwrap()["calls"], 2);

        // Alice reuses "req-shared" with DIFFERENT args -> Conflict!
        let r_conflict_args = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "req-shared".into(),
                caller_id: "owner:alice".into(),
                action: "query_a".into(),
                args: json!({"q": "other"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(!r_conflict_args.ok);
        assert_eq!(
            r_conflict_args.error_code,
            Some(InteractErrorCode::Conflict)
        );

        // Alice reuses "req-shared" with DIFFERENT action -> Conflict!
        let r_conflict_action = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "req-shared".into(),
                caller_id: "owner:alice".into(),
                action: "query_b".into(),
                args: json!({"q": "secret-alice"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(!r_conflict_action.ok);
        assert_eq!(
            r_conflict_action.error_code,
            Some(InteractErrorCode::Conflict)
        );

        // Alice retries "req-shared" with IDENTICAL (action, args, rev) -> cached hit without incrementing calls.
        let r_replay = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "req-shared".into(),
                caller_id: "owner:alice".into(),
                action: "query_a".into(),
                args: json!({"q": "secret-alice"}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(r_replay.ok && r_replay.cached);
        assert_eq!(r_replay.result.as_ref().unwrap()["calls"], 1);
    }

    // ---- Test 13 (B8): Rollover dump-too-large fails cleanly (no empty VM) & cold-recovers ----
    #[tokio::test]
    async fn b8_rollover_dump_too_large_triggers_clean_cold_recovery() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Rollover").await;
        let script = r#"
val = 0
async def on_start(last_state):
    global val
    if isinstance(last_state, dict):
        val = int(last_state.get("val", 0))
    else:
        val = 10

async def on_step():
    global val
    val += 1
    deck_checkpoint({"val": val})
    deck_emit({"val": val}, durable=True)
"#;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: vec![],
                mode: ScriptMode::Streaming,
                interval_secs: Some(5),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "rollover".into(),
            })
            .send()
            .await
            .unwrap();
        let approved = h
            .sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        let rec = h.shared.store.get_deck(&deck_id).await.unwrap().unwrap();
        let cwd = h.shared.store.deck_work_dir(&deck_id).unwrap();
        let dump_path = h.shared.store.monty_dump_path(rec.session_id);
        let pool = agent::repl::test_pool(2).await.unwrap();
        let cx = crate::runtime::TurnContext {
            last_state: json!(null),
            last_snapshot: json!(null),
        };
        let mut sink = |_: &crate::runtime::EmittedPayload| {};
        let (mut checkout, mode, _) = crate::runtime::DeckCheckout::open_or_restore(
            pool.clone(),
            h.registry.clone(),
            rec.session_id,
            &cwd,
            &h.shared.store.spill_dir(),
            dump_path.clone(),
            &approved,
            None,
            true,
            &cx,
            &mut sink,
        )
        .await
        .unwrap();
        assert_eq!(mode, crate::runtime::StartMode::ColdStarted);

        let step1 = checkout.run_step(&cx, &mut sink).await.unwrap();
        let durable_state = step1.effects.checkpoint_state.clone().unwrap();
        assert_eq!(durable_state["val"], 11);

        // Force rollover with an intentionally tiny dump cap -> MUST return Err("rollover_dump_too_large")
        // instead of silently swapping in an empty VM!
        checkout.set_dump_hard_bytes_for_test(16);
        checkout.force_rollover_for_test();
        let rollover_res = checkout.maybe_refresh_or_rollover().await;
        assert!(
            matches!(rollover_res, Err(ref e) if e.contains("rollover_dump_too_large")),
            "expected rollover_dump_too_large error, got {rollover_res:?}"
        );
        drop(checkout);

        // Cold recovery with `allow_warm_restore = false` restores durable state via `on_start(last_state)`.
        let cx2 = crate::runtime::TurnContext {
            last_state: durable_state,
            last_snapshot: json!(null),
        };
        let (mut recovered, mode2, _) = crate::runtime::DeckCheckout::open_or_restore(
            pool,
            h.registry.clone(),
            rec.session_id,
            &cwd,
            &h.shared.store.spill_dir(),
            dump_path,
            &approved,
            None,
            false,
            &cx2,
            &mut sink,
        )
        .await
        .unwrap();
        assert_eq!(mode2, crate::runtime::StartMode::ColdStarted);
        let step2 = recovered.run_step(&cx2, &mut sink).await.unwrap();
        assert_eq!(step2.effects.emits[0].data["val"], 12);
    }

    // ---- Test 14 (H1): Isolated per-Deck cwd/write_root & pinned approvals even when global approvals=false ----
    #[tokio::test]
    async fn h1_deck_session_has_isolated_cwd_write_root_and_pinned_approvals() {
        let _lock = TEST_LOCK.lock().await;
        ::tools::secrets::install(Arc::new(TestSecrets));
        evict_actor::<ToolRegistry>(::tools::REGISTRY_ACTOR).await;
        evict_actor::<Approval>(::tools::APPROVAL_ACTOR).await;
        evict_actor::<::tools::AskUser>(::tools::ASK_ACTOR).await;
        evict_actor::<::tools::Jobs>(::tools::JOBS_ACTOR).await;

        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        // Explicitly disable global approvals in config — Deck MUST still pin per-session approvals=true!
        let cfg = figment::Figment::new().merge(("tools.approvals", false));
        let rt = Runtime::new(cfg, data_dir.clone());
        let registry = ::tools::registry::start(&rt).await.unwrap();
        let approval = ActorRef::<Approval>::lookup(::tools::APPROVAL_ACTOR)
            .unwrap()
            .expect("approval actor");
        let deploy_count = Arc::new(AtomicUsize::new(0));
        registry
            .ask(RegisterTool(Arc::new(MockMcpDeployTool(
                deploy_count.clone(),
            ))))
            .await
            .unwrap();

        let doc_store = document::store::Store::open(&data_dir).await.unwrap();
        let (ev_tx, _ev_rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = document::helper::Helper::new(ev_tx, rt.shutdown.clone());
        let doc_actor = DocumentActor::spawn(DocumentActor::new(
            doc_store,
            helper,
            DocumentsConfig::default(),
            rt.clone(),
        ));
        let deck_store = DeckStore::open(&data_dir).await.unwrap();
        let pool = agent::repl::test_pool(2).await.unwrap();
        let (sup_inst, shared) = DeckSupervisor::new(
            deck_store,
            rt.clone(),
            doc_actor,
            registry.clone(),
            Some(pool),
        );
        let sup = DeckSupervisor::spawn(sup_inst);
        let watcher = DeckApprovalWatcher::spawn(DeckApprovalWatcher::with_supervisor(
            shared.clone(),
            sup.clone(),
        ));
        rt.subscribe::<DeckApprovalWatcher, ApprovalRequested>(&watcher)
            .await;
        rt.subscribe::<DeckApprovalWatcher, ApprovalResolved>(&watcher)
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let created = sup
            .ask(CreateDeckMsg {
                title: Some("Isolated Deck".into()),
                description: None,
                example: false,
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let deck_id = created.doc.doc_id;
        let rec = shared.store.get_deck(&deck_id).await.unwrap().unwrap();
        let expected_work = shared.store.deck_work_dir(&deck_id).unwrap();

        let rev = sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: "async def on_step():\n    await mcp_acme_deploy(env='prod')\n    deck_emit({'ok': True}, durable=True)\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "gated deploy".into(),
            })
            .send()
            .await
            .unwrap();
        sup.ask(ApproveScriptMsg {
            deck_id: deck_id.clone(),
            rev: rev.rev,
            expected_digest: rev.revision_sha256.clone(),
            approved_by: "owner".into(),
        })
        .send()
        .await
        .unwrap();

        let (_hub, _sub_id, mut rx) = sup
            .ask(SubscribeDeckMsg {
                deck_id: deck_id.clone(),
            })
            .send()
            .await
            .unwrap();
        let _ = rx.recv().await; // Sync

        let sup_c = sup.clone();
        let did_c = deck_id.clone();
        let run_task = tokio::spawn(async move {
            sup_c
                .ask(RuntimeControlMsg {
                    deck_id: did_c,
                    op: RuntimeControlOp::RunOnce,
                })
                .send()
                .await
        });

        // Must pause for approval even though global tools.approvals=false!
        let approval_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Some(DeckRuntimeEvent::ApprovalPending { approval }) => {
                        return approval.approval_id;
                    }
                    Some(_) => {}
                    None => panic!("closed before approval"),
                }
            }
        })
        .await
        .expect("Deck must enforce pinned per-session approvals even when global approvals=false");

        // Verify Deck's Seat has isolated per-deck cwd & write_root outside data_dir/decks.
        let seat = ::tools::seat::seat_of(rec.session_id);
        assert_eq!(seat.cwd.as_deref(), Some(expected_work.as_path()));
        assert_eq!(seat.write_root.as_deref(), Some(expected_work.as_path()));
        assert!(
            !expected_work.starts_with(data_dir.join("decks")),
            "deck work_dir must be outside protected data_dir/decks"
        );
        assert_eq!(deploy_count.load(Ordering::SeqCst), 0);

        approval
            .ask(Resolve {
                id: approval_id,
                decision: Decision::Allow,
            })
            .send()
            .await
            .unwrap();
        let run_res = tokio::time::timeout(Duration::from_secs(5), run_task)
            .await
            .unwrap()
            .unwrap();
        assert!(run_res.is_ok());
        assert_eq!(deploy_count.load(Ordering::SeqCst), 1);
    }

    // ---- Test 15 (H2): Deletion teardown, no phantom decks ----
    #[tokio::test]
    async fn h2_deck_deletion_teardown_and_no_phantom_decks() {
        let h = setup_harness().await;

        // Phantom deck checks: nonexistent deck_id must error and create ZERO deck_meta rows.
        let bogus = "deck-does-not-exist-999";
        assert!(
            h.sup
                .ask(SubscribeDeckMsg {
                    deck_id: bogus.into(),
                })
                .send()
                .await
                .is_err()
        );
        assert!(
            h.sup
                .ask(RuntimeControlMsg {
                    deck_id: bogus.into(),
                    op: RuntimeControlOp::Start,
                })
                .send()
                .await
                .is_err()
        );
        assert!(
            h.sup
                .ask(InteractMsg {
                    deck_id: bogus.into(),
                    request_id: "p1".into(),
                    caller_id: "owner".into(),
                    action: "x".into(),
                    args: json!({}),
                    expected_rev: None,
                    timeout_ms: None,
                })
                .send()
                .await
                .is_err()
        );
        assert!(
            h.sup
                .ask(ProposeScriptMsg {
                    deck_id: bogus.into(),
                    code: "async def on_step():\n    pass\n".into(),
                    declared_actions: vec![],
                    mode: ScriptMode::Oneshot,
                    interval_secs: None,
                    schedule_cron: None,
                    proposed_by: "owner".into(),
                    change_summary: "phantom".into(),
                })
                .send()
                .await
                .is_err()
        );
        assert!(
            h.shared.store.get_deck(bogus).await.unwrap().is_none(),
            "nonexistent deck must never create phantom deck_meta rows"
        );

        // Create, approve, start a streaming Deck, subscribe a viewer, then delete via DocumentChanged!
        let deck_id = new_deck(&h, "Disposable").await;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: "async def on_step():\n    deck_emit({'alive': True}, durable=True)\n    deck_sleep(0.2)\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Streaming,
                interval_secs: Some(1),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "live".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();

        let rec = h.shared.store.get_deck(&deck_id).await.unwrap().unwrap();
        let work_dir = h.shared.store.deck_work_dir(&deck_id).unwrap();
        assert!(work_dir.exists());

        let (_hub, _sub_id, mut rx) = h
            .sup
            .ask(SubscribeDeckMsg {
                deck_id: deck_id.clone(),
            })
            .send()
            .await
            .unwrap();
        let _ = rx.recv().await; // Sync

        // Emit DocumentChanged { deleted: true } as DocumentActor does on delete.
        h.rt.publish(document::DocumentChanged {
            doc: document::DocId(deck_id.clone()),
            kind: document::DocKind::Deck,
            title: "Disposable".into(),
            updated_ms: chrono::Utc::now().timestamp_millis(),
            deleted: true,
        })
        .await;

        // Subscriber must observe terminal Closed { reason: "deck_deleted" }.
        let closed_reason = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match rx.recv().await {
                    Some(DeckRuntimeEvent::Closed { reason }) => return reason,
                    Some(_) => {}
                    None => return "channel_dropped".into(),
                }
            }
        })
        .await
        .expect("subscriber should receive Closed on deck deletion");
        assert_eq!(closed_reason, "deck_deleted");

        // SQLite state, work_dir, and session seat must all be cleaned up.
        assert!(h.shared.store.get_deck(&deck_id).await.unwrap().is_none());
        assert!(
            !work_dir.exists(),
            "deck work dir should be removed on delete"
        );
        assert!(
            ::tools::seat::seat_of(rec.session_id).cwd.is_none(),
            "deck seat should be forgotten on delete"
        );
        assert!(
            h.sup
                .ask(RuntimeControlMsg {
                    deck_id: deck_id.clone(),
                    op: RuntimeControlOp::Start,
                })
                .send()
                .await
                .is_err(),
            "deleted deck must not restart"
        );
    }

    // ---- Test 16 (H3): Boot recovery resumes previously enabled Decks only ----
    #[tokio::test]
    async fn h3_boot_recovery_resumes_enabled_decks_only() {
        let h = setup_harness().await;
        let deck_enabled = new_deck(&h, "Boot Resume Enabled").await;
        let deck_stopped = new_deck(&h, "Boot Resume Stopped").await;

        let script = r#"
ticks = 0
async def on_start(last_state):
    global ticks
    if isinstance(last_state, dict):
        ticks = int(last_state.get("ticks", 0))

async def on_step():
    global ticks
    ticks += 1
    deck_checkpoint({"ticks": ticks})
    deck_emit({"ticks": ticks}, durable=True)
    deck_sleep(0.2)
"#;
        for did in [&deck_enabled, &deck_stopped] {
            let rev = h
                .sup
                .ask(ProposeScriptMsg {
                    deck_id: did.clone(),
                    code: script.into(),
                    declared_actions: vec![],
                    mode: ScriptMode::Streaming,
                    interval_secs: Some(1),
                    schedule_cron: None,
                    proposed_by: "owner".into(),
                    change_summary: "boot".into(),
                })
                .send()
                .await
                .unwrap();
            h.sup
                .ask(ApproveScriptMsg {
                    deck_id: did.clone(),
                    rev: rev.rev,
                    expected_digest: rev.revision_sha256.clone(),
                    approved_by: "owner".into(),
                })
                .send()
                .await
                .unwrap();
        }

        // Start only `deck_enabled`; leave `deck_stopped` Stopped.
        h.sup
            .ask(RuntimeControlMsg {
                deck_id: deck_enabled.clone(),
                op: RuntimeControlOp::Start,
            })
            .send()
            .await
            .unwrap();

        // Simulate engine restart: open a fresh DeckSupervisor against the same DeckStore & doc actor.
        let store2 = DeckStore::open(&h.rt.data_dir).await.unwrap();
        let pool2 = agent::repl::test_pool(2).await.unwrap();
        let (sup2_inst, _shared2) = DeckSupervisor::new(
            store2,
            h.rt.clone(),
            h.shared.document.clone(),
            h.registry.clone(),
            Some(pool2),
        );
        let sup2 = DeckSupervisor::spawn(sup2_inst);
        let recovered = sup2
            .ask(crate::owner::RecoverEnabledDecksMsg)
            .send()
            .await
            .unwrap();
        assert_eq!(
            recovered, 1,
            "only the 1 enabled deck should resume on boot"
        );

        tokio::time::sleep(Duration::from_millis(120)).await;
        let det_en = sup2
            .ask(GetDeck {
                deck_id: deck_enabled.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let det_st = sup2
            .ask(GetDeck {
                deck_id: deck_stopped.clone(),
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        assert_eq!(det_en.runtime.state, RuntimeState::Streaming);
        assert_eq!(det_st.runtime.state, RuntimeState::Stopped);
        let _ = sup2
            .ask(RuntimeControlMsg {
                deck_id: deck_enabled,
                op: RuntimeControlOp::Stop,
            })
            .send()
            .await;
    }

    // ---- Test 17 (H4): Caps (code/actions/summary/emits/stdout/action-return) & checked time ----
    #[tokio::test]
    async fn h4_caps_and_checked_time_prevent_abuse_and_panics() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Caps").await;

        // 1. Oversized code rejected.
        let huge_code = "a = 1\n".repeat(50_000);
        assert!(
            h.sup
                .ask(ProposeScriptMsg {
                    deck_id: deck_id.clone(),
                    code: huge_code,
                    declared_actions: vec![],
                    mode: ScriptMode::Oneshot,
                    interval_secs: None,
                    schedule_cron: None,
                    proposed_by: "owner".into(),
                    change_summary: "too big".into(),
                })
                .send()
                .await
                .is_err()
        );

        // 2. Too many declared actions (>32) rejected.
        let too_many_actions: Vec<DeclaredAction> = (0..33)
            .map(|i| DeclaredAction {
                name: format!("act_{i}"),
                title: format!("Act {i}"),
                description: "d".into(),
                schema: json!({"type": "object"}),
                mutates_state: false,
                allow_while_paused: true,
            })
            .collect();
        assert!(
            h.sup
                .ask(ProposeScriptMsg {
                    deck_id: deck_id.clone(),
                    code: "async def on_step():\n    pass\n".into(),
                    declared_actions: too_many_actions,
                    mode: ScriptMode::Oneshot,
                    interval_secs: None,
                    schedule_cron: None,
                    proposed_by: "owner".into(),
                    change_summary: "many actions".into(),
                })
                .send()
                .await
                .is_err()
        );

        // 3. Duplicate action names rejected.
        let dup_actions = vec![
            DeclaredAction {
                name: "dup".into(),
                title: "D1".into(),
                description: "d".into(),
                schema: json!({"type": "object"}),
                mutates_state: false,
                allow_while_paused: true,
            },
            DeclaredAction {
                name: "dup".into(),
                title: "D2".into(),
                description: "d".into(),
                schema: json!({"type": "object"}),
                mutates_state: false,
                allow_while_paused: true,
            },
        ];
        assert!(
            h.sup
                .ask(ProposeScriptMsg {
                    deck_id: deck_id.clone(),
                    code: "async def on_step():\n    pass\n".into(),
                    declared_actions: dup_actions,
                    mode: ScriptMode::Oneshot,
                    interval_secs: None,
                    schedule_cron: None,
                    proposed_by: "owner".into(),
                    change_summary: "dup".into(),
                })
                .send()
                .await
                .is_err()
        );

        // 4. Oversized summary (>1 KiB) rejected.
        assert!(
            h.sup
                .ask(ProposeScriptMsg {
                    deck_id: deck_id.clone(),
                    code: "async def on_step():\n    pass\n".into(),
                    declared_actions: vec![],
                    mode: ScriptMode::Oneshot,
                    interval_secs: None,
                    schedule_cron: None,
                    proposed_by: "owner".into(),
                    change_summary: "x".repeat(2048),
                })
                .send()
                .await
                .is_err()
        );

        // 5. Extreme interval_secs (u64::MAX) clamps safely and never panics Instant::checked_add;
        //    also test per-turn emit cap (>16), multibyte UTF-8 stdout truncation, and >256 KiB action return cap!
        let script = r#"
async def on_step():
    print("€" * 12000)
    for i in range(17):
        deck_emit({"i": i}, durable=False)

async def on_action(action, args, ctx):
    if action == "utf8_ok":
        print("€" * 12000)
        return {"ok": True}
    if action == "huge_return":
        return {"blob": "x" * 300000}
    return {}
"#;
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: script.into(),
                declared_actions: vec![
                    DeclaredAction {
                        name: "utf8_ok".into(),
                        title: "UTF8".into(),
                        description: "prints multibyte utf8".into(),
                        schema: json!({"type": "object"}),
                        mutates_state: false,
                        allow_while_paused: true,
                    },
                    DeclaredAction {
                        name: "huge_return".into(),
                        title: "Huge".into(),
                        description: "returns >256KiB".into(),
                        schema: json!({"type": "object"}),
                        mutates_state: false,
                        allow_while_paused: true,
                    },
                ],
                mode: ScriptMode::Oneshot,
                interval_secs: Some(u64::MAX),
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "caps".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(rev.interval_secs, Some(crate::model::MAX_INTERVAL_SECS));

        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();

        // 17 emits in one turn MUST fail cleanly with per-turn emit cap error.
        let step_err = h
            .sup
            .ask(RuntimeControlMsg {
                deck_id: deck_id.clone(),
                op: RuntimeControlOp::RunOnce,
            })
            .send()
            .await;
        assert!(
            step_err.is_err(),
            "17 deck_emit calls in one turn must fail"
        );

        // Multibyte UTF-8 stdout > 16 KiB must truncate safely without panicking!
        let utf8_resp = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "utf8-1".into(),
                caller_id: "owner".into(),
                action: "utf8_ok".into(),
                args: json!({}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(
            utf8_resp.ok,
            "multibyte UTF-8 stdout must not panic: {utf8_resp:?}"
        );

        // Action returning > 256 KiB payload must fail cleanly with size cap error!
        let huge_resp = h
            .sup
            .ask(InteractMsg {
                deck_id: deck_id.clone(),
                request_id: "huge-1".into(),
                caller_id: "owner".into(),
                action: "huge_return".into(),
                args: json!({}),
                expected_rev: Some(1),
                timeout_ms: None,
            })
            .send()
            .await
            .unwrap();
        assert!(!huge_resp.ok, "oversized action return must be rejected");
        assert!(
            huge_resp.error.as_deref().unwrap_or("").contains("cap"),
            "unexpected error: {:?}",
            huge_resp.error
        );

        // DeckHub::admit_emit must never panic with checked_sub.
        let hub = crate::hub::DeckHub::new();
        assert!(hub.admit_emit("d-test"));
    }

    // ---- Test 18 (M1, M2, L1, L3): Approval queue restore, DeckChanged events, snapshot order, decks.enabled=false ----
    #[tokio::test]
    async fn m1_m2_l1_l3_approval_queue_events_snapshot_order_and_disabled_flag() {
        let h = setup_harness().await;
        let deck_id = new_deck(&h, "Polish").await;
        let rec = h.shared.store.get_deck(&deck_id).await.unwrap().unwrap();

        // M1: Two concurrent approvals on one Deck while Paused -> resolving both restores Paused (NOT Streaming).
        h.shared.live_cache.lock().insert(
            deck_id.clone(),
            crate::owner::LiveDeckCache {
                status: crate::model::DeckRuntimeStatus {
                    deck_id: deck_id.clone(),
                    session_id: rec.session_id,
                    state: RuntimeState::Paused,
                    run_epoch: 1,
                    active_rev: Some(1),
                    pending_rev: None,
                    stale: false,
                    last_ok_snapshot_id: None,
                    last_ok_at: None,
                    last_error: None,
                    last_error_at: None,
                    consecutive_failures: 0,
                    step_count: 1,
                    checkout_alive: true,
                    pending_approval: None,
                },
                last_snapshot: None,
                live_frame: None,
            },
        );
        let id1 = ::tools::ApprovalId(uuid::Uuid::new_v4());
        let id2 = ::tools::ApprovalId(uuid::Uuid::new_v4());
        h.shared.on_approval_requested(&ApprovalRequested {
            id: id1,
            session: rec.session_id,
            tool: "tool_one".into(),
            args: json!({}),
            risk: Risk::Mutating,
            human_only: false,
            always_ask: false,
            always_ask_why: None,
        });
        h.shared.on_approval_requested(&ApprovalRequested {
            id: id2,
            session: rec.session_id,
            tool: "tool_two".into(),
            args: json!({}),
            risk: Risk::Mutating,
            human_only: false,
            always_ask: false,
            always_ask_why: None,
        });
        assert_eq!(pending_approval_id(&h.shared, &deck_id), Some(id1));
        assert_eq!(
            h.shared
                .live_cache
                .lock()
                .get(&deck_id)
                .unwrap()
                .status
                .state,
            RuntimeState::WaitingApproval
        );

        // Resolving id1 advances queue to id2 while staying WaitingApproval.
        h.shared.on_approval_resolved(&ApprovalResolved {
            id: id1,
            decision: Decision::Allow,
            timed_out: false,
            dismissed: false,
        });
        assert_eq!(pending_approval_id(&h.shared, &deck_id), Some(id2));
        assert_eq!(
            h.shared
                .live_cache
                .lock()
                .get(&deck_id)
                .unwrap()
                .status
                .state,
            RuntimeState::WaitingApproval
        );

        // Resolving id2 empties queue and restores exact pre-approval state (`Paused`)!
        h.shared.on_approval_resolved(&ApprovalResolved {
            id: id2,
            decision: Decision::Allow,
            timed_out: false,
            dismissed: false,
        });
        assert_eq!(pending_approval_id(&h.shared, &deck_id), None);
        assert_eq!(
            h.shared
                .live_cache
                .lock()
                .get(&deck_id)
                .unwrap()
                .status
                .state,
            RuntimeState::Paused
        );

        // L1: Snapshots with identical `created_at` sort by `(run_epoch DESC, seq DESC)`.
        let ts = "2026-11-28T00:00:00Z".to_string();
        h.shared
            .store
            .save_snapshot(&crate::model::DeckSnapshot {
                snapshot_id: "s-e1-2".into(),
                deck_id: deck_id.clone(),
                script_rev: 1,
                run_epoch: 1,
                seq: 2,
                data: json!({"n": 1}),
                section: None,
                created_at: ts.clone(),
            })
            .await
            .unwrap();
        h.shared
            .store
            .save_snapshot(&crate::model::DeckSnapshot {
                snapshot_id: "s-e2-1".into(),
                deck_id: deck_id.clone(),
                script_rev: 1,
                run_epoch: 2,
                seq: 1,
                data: json!({"n": 2}),
                section: None,
                created_at: ts,
            })
            .await
            .unwrap();
        let snaps = h.shared.store.list_snapshots(&deck_id, 10).await.unwrap();
        assert_eq!(snaps[0].snapshot_id, "s-e2-1");
        assert_eq!(snaps[1].snapshot_id, "s-e1-2");

        // M2: DeckChanged bus event fires on SectionAddMsg & ProposeScriptMsg.
        let changed_count = Arc::new(AtomicUsize::new(0));
        #[derive(kameo::Actor)]
        struct CounterActor(Arc<AtomicUsize>);
        impl kameo::message::Message<crate::model::DeckChanged> for CounterActor {
            type Reply = ();
            async fn handle(
                &mut self,
                _msg: crate::model::DeckChanged,
                _ctx: &mut kameo::message::Context<Self, Self::Reply>,
            ) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let counter_actor = CounterActor::spawn(CounterActor(changed_count.clone()));
        h.rt.subscribe::<CounterActor, crate::model::DeckChanged>(&counter_actor)
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        h.sup
            .ask(crate::owner::SectionAddMsg {
                deck_id: deck_id.clone(),
                kind: document::deck::SectionKind::Note,
                title: "Note".into(),
                content: "hello".into(),
                col_span: Some(12),
                data_key: None,
                index: None,
                by: Principal::Owner { device: None },
            })
            .send()
            .await
            .unwrap();
        let rev = h
            .sup
            .ask(ProposeScriptMsg {
                deck_id: deck_id.clone(),
                code: "async def on_step():\n    pass\n".into(),
                declared_actions: vec![],
                mode: ScriptMode::Oneshot,
                interval_secs: None,
                schedule_cron: None,
                proposed_by: "owner".into(),
                change_summary: "m2".into(),
            })
            .send()
            .await
            .unwrap();
        h.sup
            .ask(ApproveScriptMsg {
                deck_id: deck_id.clone(),
                rev: rev.rev,
                expected_digest: rev.revision_sha256.clone(),
                approved_by: "owner".into(),
            })
            .send()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            changed_count.load(Ordering::SeqCst) >= 3,
            "expected DeckChanged events on section add, script propose, and script approve"
        );

        // L3: DecksPlugin with `decks.enabled = false` returns Ok without registering DECKS_ACTOR.
        evict_actor::<DeckSupervisor>(crate::DECKS_ACTOR).await;
        let tmp_off = tempfile::tempdir().unwrap();
        let rt_off = Runtime::new(
            figment::Figment::new().merge(("decks.enabled", false)),
            tmp_off.path().to_path_buf(),
        );
        use ::runtime::Plugin as _;
        Box::new(crate::DecksPlugin).start(&rt_off).await.unwrap();
        assert!(
            ActorRef::<DeckSupervisor>::lookup(crate::DECKS_ACTOR)
                .unwrap()
                .is_none(),
            "decks.enabled = false must not register DECKS_ACTOR"
        );
    }
}
