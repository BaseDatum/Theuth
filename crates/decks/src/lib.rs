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
        let store = match store::DeckStore::open(&rt.data_dir).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("decks store unavailable: {e:#}");
                return Ok(());
            }
        };

        let rt_bg = rt.clone();
        tokio::spawn(async move {
            let Some(doc_actor) = rt_bg
                .lookup_when::<document::DocumentActor>(document::DOCUMENT_ACTOR)
                .await
            else {
                tracing::warn!("decks: document actor unavailable; decks disabled");
                return;
            };
            let Some(registry) = rt_bg
                .lookup_when::<ToolRegistry>(::tools::REGISTRY_ACTOR)
                .await
            else {
                tracing::warn!("decks: tool registry unavailable; decks disabled");
                return;
            };
            let cwd = std::env::current_dir().unwrap_or_else(|_| rt_bg.data_dir.clone());
            let (sup, shared) =
                DeckSupervisor::new(store, rt_bg.clone(), doc_actor, registry.clone(), cwd, None);
            let actor = DeckSupervisor::spawn(sup);
            if let Err(e) = actor.register(DECKS_ACTOR) {
                tracing::warn!("decks: failed to register actor: {e:#}");
                return;
            }
            let watcher = DeckApprovalWatcher::spawn(DeckApprovalWatcher::new(shared));
            rt_bg
                .subscribe::<DeckApprovalWatcher, ApprovalRequested>(&watcher)
                .await;
            rt_bg
                .subscribe::<DeckApprovalWatcher, ApprovalResolved>(&watcher)
                .await;
            tools::register(&registry, actor, rt_bg).await;
            tracing::info!("decks plugin started");
        });
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
            tmp.path().to_path_buf(),
            Some(pool),
        );
        let sup = DeckSupervisor::spawn(sup_inst);
        let watcher = DeckApprovalWatcher::spawn(DeckApprovalWatcher::new(shared.clone()));
        rt.subscribe::<DeckApprovalWatcher, ApprovalRequested>(&watcher)
            .await;
        rt.subscribe::<DeckApprovalWatcher, ApprovalResolved>(&watcher)
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

        // Upgrade to rev2 (incompatible sha/rev) -> Cold fallback calls v2's `on_start(last_state)`
        // receiving durable_counter=2!
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
                approved_by: "owner".into(),
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
        assert_eq!(
            detail.pending_script.as_ref().unwrap().status,
            ScriptStatus::PendingReview
        );

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
                rev: 1,
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
}
