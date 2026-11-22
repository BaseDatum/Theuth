//! Publish → open → call, end to end through the real tool registry and
//! artifact store, with a canned parser verdict (`helper::install_canned`;
//! the real validator's contract is exercised in `packages/apps` and in
//! `js-runtime`/`helper.rs` tests). One test: the kameo registry names and the
//! canned responder are process-global.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app::{
    AppCallChanged, AppPlugin, Apps, CallArgs, CallStatus, CallStatusOf, CallTool, CloseApp,
    GetApp, PublishFromUi, Trigger,
};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use rpc_core::RpcPeer;
use parking_lot::Mutex;
use runtime::{Plugin, Runtime, SessionId};
use tools::{Invoke, Risk, Tool, ToolCall, ToolCx, ToolOutput, ToolRegistry};

const SECRET: &str = "hunter2";
const PLACEHOLDER: &str = "{{secret:APP_SMOKE}}";

/// One-secret vault: placeholder → value on the way in, value → placeholder out.
struct Vault;

impl tools::secrets::Secrets for Vault {
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        let mut n = 0;
        walk(args, &mut |s| {
            if s.contains(PLACEHOLDER) {
                *s = s.replace(PLACEHOLDER, SECRET);
                n += 1;
            }
        });
        Ok(n)
    }
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if text.contains(SECRET) {
            Cow::Owned(text.replace(SECRET, PLACEHOLDER))
        } else {
            Cow::Borrowed(text)
        }
    }
    fn redact_value(&self, v: &mut serde_json::Value) {
        walk(v, &mut |s| {
            if let Cow::Owned(r) = self.redact(s) {
                *s = r;
            }
        });
    }
}

fn walk(v: &mut serde_json::Value, f: &mut impl FnMut(&mut String)) {
    match v {
        serde_json::Value::String(s) => f(s),
        serde_json::Value::Array(a) => a.iter_mut().for_each(|v| walk(v, f)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|v| walk(v, f)),
        _ => {}
    }
}

/// Echo; with `{"wait": true}` it blocks until [`GATE`] is released, so a
/// test can close the view while the tool is genuinely still running.
struct Echo;

static GATE: std::sync::LazyLock<tokio::sync::Notify> =
    std::sync::LazyLock::new(tokio::sync::Notify::new);
static GATED_RUNS: AtomicUsize = AtomicUsize::new(0);

#[async_trait::async_trait]
impl Tool for Echo {
    const NAME: &'static str = "echo_tool";
    const RISK: Risk = Risk::ReadOnly;
    type Args = serde_json::Value;
    fn description(&self) -> String {
        "echo".into()
    }
    async fn call(&self, args: Self::Args, _: ToolCx) -> anyhow::Result<ToolOutput> {
        if args["wait"] == true {
            GATE.notified().await;
            GATED_RUNS.fetch_add(1, Ordering::SeqCst);
        }
        Ok(ToolOutput {
            structured: Some(serde_json::json!({ "echo": args })),
            ..ToolOutput::ok("echoed")
        })
    }
}

#[derive(kameo::Actor)]
struct Events(Arc<Mutex<Vec<AppCallChanged>>>);

impl Message<AppCallChanged> for Events {
    type Reply = ();
    async fn handle(&mut self, ev: AppCallChanged, _: &mut Context<Self, Self::Reply>) {
        self.0.lock().push(ev);
    }
}

/// Counts runs and keeps the last `target` exactly as the tool received it.
struct Wipe(Arc<AtomicUsize>, Arc<Mutex<String>>);

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct WipeArgs {
    target: String,
}

#[async_trait::async_trait]
impl Tool for Wipe {
    const NAME: &'static str = "wipe_tool";
    const RISK: Risk = Risk::Destructive;
    type Args = WipeArgs;
    fn description(&self) -> String {
        "wipe".into()
    }
    async fn call(&self, args: WipeArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        *self.1.lock() = args.target.clone();
        Ok(ToolOutput {
            structured: Some(serde_json::json!({ "wiped": args.target })),
            ..ToolOutput::ok("wiped")
        })
    }
}

/// `$key` carries a secret placeholder: source is data, so publishing it is
/// not a credential use; only a call that passes it to a tool is.
const SOURCE: &str = "root = Stack([status, controls])\n$app = \"db\"\n$key = \"{{secret:APP_SMOKE}}\"\nhealth = Query(\"echo_tool\", {name: $app}, {ok: false}, 30)\nrisky = Query(\"wipe_tool\", {target: $app}, {}, 60)\nstatus = TextContent(\"\" + health.ok)\nsync = Mutation(\"wipe_tool\", {target: $app})\ncontrols = Button(\"Wipe\", Action([@Run(sync)]), null, \"destructive\")\n";

/// A responder answering every request with `body` under the request's id,
/// its `source` replaced by the request's own `source`. Echoing the source is
/// what lets the test see the bytes the `app` tool actually received.
fn canned(body: serde_json::Value) -> app::helper::Responder {
    Arc::new(move |line: &str| {
        let req: serde_json::Value = serde_json::from_str(line).unwrap();
        let mut out = body.clone();
        out["id"] = req["id"].clone();
        out["source"] = req["source"].clone();
        out.to_string()
    })
}

async fn identity(reg: &ActorRef<ToolRegistry>, session: SessionId, name: &str) -> String {
    reg.ask(tools::InspectTool {
        session,
        name: name.into(),
        args: None,
    })
    .await
    .unwrap()
    .identity
}

/// The one connection the main view `v1` is bound to; a fresh `internal()`
/// peer is another connection.
fn call(args: CallArgs) -> CallTool {
    static PEER: std::sync::LazyLock<RpcPeer> = std::sync::LazyLock::new(RpcPeer::internal);
    CallTool {
        peer: PEER.clone(),
        args,
    }
}

fn read(record: &str, instance: &str, statement: &str, request: &str) -> CallArgs {
    CallArgs {
        record_call_id: record.into(),
        instance_id: instance.into(),
        generation: 1,
        statement_id: statement.into(),
        request_id: request.into(),
        args: serde_json::json!({ "name": "db" }),
        trigger: Trigger::Load,
        action_id: None,
    }
}

fn press(record: &str, instance: &str, request: &str, target: &str) -> CallArgs {
    CallArgs {
        statement_id: "sync".into(),
        args: serde_json::json!({ "target": target }),
        trigger: Trigger::Press,
        action_id: Some("act-1".into()),
        ..read(record, instance, "sync", request)
    }
}

#[tokio::test]
async fn publish_open_call_edit_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    // A wrongly gated publish fails in a second instead of hanging five minutes.
    let cfg = figment::Figment::new().merge(figment::providers::Serialized::default(
        "tools.approval_timeout_secs",
        1,
    ));
    let rt = Runtime::new(cfg, dir.path().to_path_buf());
    tools::secrets::install(Arc::new(Vault));
    let reg = tools::registry::start(&rt).await.unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let last_target = Arc::new(Mutex::new(String::new()));
    reg.ask(tools::RegisterTool(Arc::new(Echo))).await.unwrap();
    reg.ask(tools::RegisterTool(Arc::new(Wipe(
        runs.clone(),
        last_target.clone(),
    ))))
    .await
    .unwrap();
    Box::new(artifact::ArtifactPlugin).start(&rt).await.unwrap();
    let session = SessionId::new();

    let echo_id = identity(&reg, session, "echo_tool").await;
    let wipe_id = identity(&reg, session, "wipe_tool").await;
    let body = serde_json::json!({
        "source": "",
        "library_hash": app::hash(),
        "statements": [
            {"id": "root", "kind": "value"}, {"id": "$app", "kind": "state"}, {"id": "$key", "kind": "state"},
            {"id": "health", "kind": "query"}, {"id": "risky", "kind": "query"},
            {"id": "status", "kind": "value"}, {"id": "sync", "kind": "mutation"},
            {"id": "controls", "kind": "value"}
        ],
        "bindings": [
            {"statement_id": "health", "tool": "echo_tool", "kind": "query", "risk": "read_only", "identity": echo_id},
            {"statement_id": "risky", "tool": "wipe_tool", "kind": "query", "risk": "destructive", "identity": wipe_id},
            {"statement_id": "sync", "tool": "wipe_tool", "kind": "mutation", "risk": "destructive", "identity": wipe_id}
        ],
        "diagnostics": [],
        "state": {"$app": "db", "$key": PLACEHOLDER},
        "orphaned": []
    });
    assert!(app::helper::install_canned(canned(body)));
    Box::new(AppPlugin).start(&rt).await.unwrap();
    for _ in 0..200 {
        let specs = reg.ask(tools::ListTools(None)).await.unwrap();
        if specs.iter().any(|s| s.name == "app") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let apps = ActorRef::<Apps>::lookup(app::APP_ACTOR).unwrap().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Events::spawn_with_mailbox(Events(events.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Events, AppCallChanged>(&sink).await;

    // Publish through the registry like the model does.
    let out = reg
        .ask(Invoke(ToolCall {
            id: "call-1".into(),
            name: "app".into(),
            args: serde_json::json!({ "name": "panel", "source": SOURCE }),
            session,
            quiet: false,
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("record_call_id call-1"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("2 queries · 1 mutations"),
        "{}",
        out.content
    );
    let structured = out.structured.unwrap();
    assert_eq!(structured["record_call_id"], "call-1");
    assert_eq!(structured["artifact"]["kind"], "app");

    // Open: bindings, hash, no diagnostics while the tools are unchanged.
    let doc = apps
        .ask(GetApp {
            record_call_id: "call-1".into(),
        })
        .await
        .unwrap();
    assert_eq!(doc.library_hash, app::hash());
    assert_eq!(doc.bindings.len(), 3);
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    // Publishing source with a placeholder is not credential use: no approval
    // prompt (the 1s timeout above would have failed the publish) and the
    // placeholder is committed literally, never the value.
    assert_eq!(doc.source, SOURCE);
    assert!(doc.source.contains(PLACEHOLDER) && !doc.source.contains(SECRET));
    assert_eq!(doc.artifact.session, session);
    assert!(
        apps.ask(GetApp {
            record_call_id: "nope".into()
        })
        .await
        .is_err()
    );

    // The desktop publishes a reply fence as an app (30 §9): same validation and
    // commit, a synthetic `ui-` call id, listed as an artifact, no tool message.
    let from_ui = apps
        .ask(PublishFromUi {
            session,
            name: "fence".into(),
            source: SOURCE.into(),
        })
        .await
        .unwrap();
    assert!(
        from_ui.artifact.call_id.starts_with("ui-"),
        "{}",
        from_ui.artifact.call_id
    );
    assert_eq!(from_ui.artifact.name, "fence.openui");
    assert_eq!(from_ui.artifact.session, session);
    assert_eq!(from_ui.library_hash, app::hash());
    assert_eq!(from_ui.bindings.len(), 3);
    let listed: Vec<artifact::Artifact> =
        ActorRef::<artifact::ArtifactActor>::lookup(artifact::ARTIFACT_ACTOR)
            .unwrap()
            .unwrap()
            .ask(artifact::List {
                session: Some(session),
                descendants: false,
            })
            .await
            .unwrap();
    let mut ids: Vec<&str> = listed.iter().map(|a| a.call_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["call-1", from_ui.artifact.call_id.as_str()]);
    // Errors are the tool's text, as the RPC error message.
    let err = match apps
        .ask(PublishFromUi {
            session,
            name: "../etc".into(),
            source: SOURCE.into(),
        })
        .await
    {
        Err(kameo::error::SendError::HandlerError(e)) => e,
        other => panic!("expected a handler error, got {other:?}"),
    };
    assert!(err.contains("must be letters, digits"), "{err}");

    // Automatic read of an allowed tool runs; of an asking tool needs a press.
    let r = apps
        .ask(call(read("call-1", "v1", "health", "r1")))
        .await
        .unwrap();
    assert_eq!(r.status, CallStatus::Succeeded, "{}", r.content);
    assert_eq!(r.structured.unwrap()["echo"]["name"], "db");
    let mut risky = read("call-1", "v1", "risky", "r2");
    risky.args = serde_json::json!({ "target": "db" });
    let r = apps.ask(call(risky)).await.unwrap();
    assert_eq!(r.status, CallStatus::NeedsApproval);
    assert!(!r.is_error);
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    // A Mutation never runs from an automatic trigger; a press needs an action id.
    let mut auto = read("call-1", "v1", "sync", "r3");
    auto.args = serde_json::json!({ "target": "db" });
    let r = apps.ask(call(auto)).await.unwrap();
    assert_eq!(r.status, CallStatus::Unavailable);
    let mut no_action = press("call-1", "v1", "r4", "db");
    no_action.action_id = None;
    assert!(apps.ask(call(no_action)).await.is_err());
    let mut unbound = read("call-1", "v1", "status", "r5");
    unbound.args = serde_json::json!({});
    assert_eq!(
        apps.ask(call(unbound)).await.unwrap().status,
        CallStatus::Unavailable
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    // One press = one destructive run, no approval dialog; the same id again returns the
    // stored result; a different input under the same id is refused.
    let r = apps
        .ask(call(press("call-1", "v1", "p1", "db")))
        .await
        .unwrap();
    assert_eq!(r.status, CallStatus::Succeeded, "{}", r.content);
    assert_eq!(r.structured.as_ref().unwrap()["wiped"], "db");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    let again = apps
        .ask(call(press("call-1", "v1", "p1", "db")))
        .await
        .unwrap();
    assert_eq!(again.status, CallStatus::Succeeded);
    assert_eq!(again.structured.unwrap()["wiped"], "db");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "duplicate request id did not re-run"
    );
    let other = apps
        .ask(call(press("call-1", "v1", "p1", "other")))
        .await
        .unwrap();
    assert_eq!(other.status, CallStatus::Unavailable);
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    // A placeholder in a *call's* args is credential use: an automatic read of
    // an allowed tool now needs a press; the press expands it for the tool and
    // the result is redacted back to the placeholder.
    let mut keyed = read("call-1", "v1", "health", "r-key");
    keyed.args = serde_json::json!({ "name": PLACEHOLDER });
    assert_eq!(
        apps.ask(call(keyed)).await.unwrap().status,
        CallStatus::NeedsApproval
    );
    let r = apps
        .ask(call(press("call-1", "v1", "p2", PLACEHOLDER)))
        .await
        .unwrap();
    assert_eq!(r.status, CallStatus::Succeeded, "{}", r.content);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(*last_target.lock(), SECRET, "tool received the value");
    assert_eq!(
        r.structured.unwrap()["wiped"],
        PLACEHOLDER,
        "output redacted"
    );

    // Status is discoverable after the fact; unknown ids say so.
    let peer = RpcPeer::internal();
    let st = apps
        .ask(CallStatusOf {
            peer: peer.clone(),
            request_id: "p1".into(),
        })
        .await
        .unwrap();
    assert_eq!(st.status, CallStatus::Succeeded);
    let st = apps
        .ask(CallStatusOf {
            peer: peer.clone(),
            request_id: "never".into(),
        })
        .await
        .unwrap();
    assert_eq!(st.status, CallStatus::Unknown);

    // Instance binding: another connection cannot reuse the view id; a stale generation is
    // cancelled; closing frees it.
    let mut other_peer = call(read("call-1", "v1", "health", "r6"));
    other_peer.peer = RpcPeer::internal();
    assert!(apps.ask(other_peer).await.is_err());
    let bound_peer = RpcPeer::internal();
    let mut gen2 = read("call-1", "v2", "health", "r7");
    gen2.generation = 2;
    assert_eq!(
        apps.ask(CallTool {
            peer: bound_peer.clone(),
            args: gen2
        })
        .await
        .unwrap()
        .status,
        CallStatus::Succeeded
    );
    let stale = read("call-1", "v2", "health", "r8");
    assert_eq!(
        apps.ask(CallTool {
            peer: bound_peer.clone(),
            args: stale
        })
        .await
        .unwrap()
        .status,
        CallStatus::Cancelled
    );
    assert!(
        apps.ask(CloseApp {
            peer: RpcPeer::internal(),
            instance_id: "v2".into()
        })
        .await
        .is_err(),
        "only the owning connection closes a view"
    );
    apps.ask(CloseApp {
        peer: bound_peer,
        instance_id: "v2".into(),
    })
    .await
    .unwrap();

    // A read still running when its view closes: the tool finishes for real,
    // the client gets Cancelled, and the event says Cancelled too, never a
    // success nobody can see. A press's outcome would stay real (durable row).
    let slow_peer = RpcPeer::internal();
    let mut slow = read("call-1", "v3", "health", "r-slow");
    slow.args = serde_json::json!({ "name": "db", "wait": true });
    let pending = tokio::spawn({
        let apps = apps.clone();
        let peer = slow_peer.clone();
        async move { apps.ask(CallTool { peer, args: slow }).await.unwrap() }
    });
    for _ in 0..200 {
        let st = apps
            .ask(CallStatusOf {
                peer: peer.clone(),
                request_id: "r-slow".into(),
            })
            .await
            .unwrap();
        if st.status == CallStatus::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    apps.ask(CloseApp {
        peer: slow_peer,
        instance_id: "v3".into(),
    })
    .await
    .unwrap();
    let detached = pending.await.unwrap();
    assert_eq!(
        detached.status,
        CallStatus::Cancelled,
        "{}",
        detached.content
    );
    assert_eq!(
        GATED_RUNS.load(Ordering::SeqCst),
        0,
        "close never interrupts the tool"
    );
    GATE.notify_one();
    let ev = loop {
        if let Some(ev) = events
            .lock()
            .iter()
            .find(|e| e.request_id == "r-slow")
            .cloned()
        {
            break ev;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(
        GATED_RUNS.load(Ordering::SeqCst),
        1,
        "the tool ran to completion"
    );
    assert_eq!(ev.status, CallStatus::Cancelled, "{ev:?}");
    assert_eq!(ev.trigger, Trigger::Load);
    // A press that finished after the fact published its real outcome.
    let p1 = events
        .lock()
        .iter()
        .find(|e| e.request_id == "p1")
        .cloned()
        .unwrap();
    assert_eq!(p1.status, CallStatus::Succeeded);

    // Edit: a patch onto the base commits v2; the same base again is a conflict.
    let out = reg
        .ask(Invoke(ToolCall {
            id: "call-2".into(),
            name: "app".into(),
            args: serde_json::json!({ "name": "panel", "source": "status = TextContent(\"x\")", "edit": true, "base_call_id": "call-1" }),
            session,
            quiet: false,
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("v2"), "{}", out.content);
    let out = reg
        .ask(Invoke(ToolCall {
            id: "call-3".into(),
            name: "app".into(),
            args: serde_json::json!({ "name": "panel", "source": "status = TextContent(\"y\")", "edit": true, "base_call_id": "call-1" }),
            session,
            quiet: false,
        }))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(
        out.content.contains("conflict") && out.content.contains("call-2"),
        "{}",
        out.content
    );

    // Edits are scoped to the session and the name.
    let out = reg
        .ask(Invoke(ToolCall {
            id: "call-4".into(),
            name: "app".into(),
            args: serde_json::json!({ "name": "panel", "source": "x = 1", "edit": true, "base_call_id": "call-2" }),
            session: SessionId::new(),
            quiet: false,
        }))
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.contains("another session"),
        "{}",
        out.content
    );

    // The view adopts v2 with a higher generation; another app's record is refused.
    let mut adopt = read("call-2", "v1", "health", "r9");
    adopt.generation = 2;
    assert_eq!(
        apps.ask(call(adopt)).await.unwrap().status,
        CallStatus::Succeeded
    );
    let out = reg
        .ask(Invoke(ToolCall {
            id: "call-5".into(),
            name: "app".into(),
            args: serde_json::json!({ "name": "other", "source": SOURCE }),
            session,
            quiet: false,
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let mut foreign = read("call-5", "v1", "health", "r10");
    foreign.generation = 3;
    assert!(
        apps.ask(call(foreign)).await.is_err(),
        "different app lineage"
    );
}
