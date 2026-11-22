//! Headless app smoke (46 §10 steps 3–4): real tool registry, artifact store
//! and app plugin on one data dir; publish through the registry's `Invoke`
//! of the `app` tool with the real (in-process) parser, then open and call the app
//! through the live actor — reads, a mutating press, a destructive press, a
//! secret-bearing press (no approval dialog, placeholder preserved in the
//! result), duplicate ids, status, an atomic edit and its conflict.
//!
//!   cargo run -p app --example smoke -- <data_dir> [record_call_id]
//!
//! With `record_call_id`, the run skips publishing and reopens that record: a
//! restart check. Each step prints one JSON line; a failed check exits 1.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app::{
    AppCallChanged, AppPlugin, Apps, CallArgs, CallStatus, CallStatusOf, CallTool, CloseApp,
    GetApp, Trigger,
};
use figment::providers::Format;
use futures::future::BoxFuture;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use rpc_core::RpcPeer;
use parking_lot::Mutex;
use runtime::{Plugin, Runtime, SessionId};
use tools::{Invoke, Risk, Tool, ToolCall, ToolCx, ToolOutput, ToolRegistry};

const SECRET_VALUE: &str = "smoke-secret-value-7f3a";
const PLACEHOLDER: &str = "{{secret:SMOKE_TOKEN}}";

/// Fixture vault: one secret, expanded on the way in, redacted on the way out.
struct FixtureSecrets;

impl tools::secrets::Secrets for FixtureSecrets {
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String> {
        fn walk(v: &mut serde_json::Value, n: &mut usize) {
            match v {
                serde_json::Value::String(s) if s.contains(PLACEHOLDER) => {
                    *s = s.replace(PLACEHOLDER, SECRET_VALUE);
                    *n += 1;
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(|v| walk(v, n)),
                serde_json::Value::Object(o) => o.values_mut().for_each(|v| walk(v, n)),
                _ => {}
            }
        }
        let mut n = 0;
        walk(args, &mut n);
        Ok(n)
    }
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if text.contains(SECRET_VALUE) {
            Cow::Owned(text.replace(SECRET_VALUE, PLACEHOLDER))
        } else {
            Cow::Borrowed(text)
        }
    }
    fn redact_value(&self, v: &mut serde_json::Value) {
        match v {
            serde_json::Value::String(s) => {
                if let Cow::Owned(r) = self.redact(s) {
                    *s = r;
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(|v| self.redact_value(v)),
            serde_json::Value::Object(o) => o.values_mut().for_each(|v| self.redact_value(v)),
            _ => {}
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct EchoArgs {
    name: String,
}

struct Echo;

#[async_trait::async_trait]
impl Tool for Echo {
    const NAME: &'static str = "smoke_echo";
    const RISK: Risk = Risk::ReadOnly;
    type Args = EchoArgs;
    fn description(&self) -> String {
        "Echo the name (smoke fixture).".into()
    }
    async fn call(&self, args: EchoArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput {
            structured: Some(serde_json::to_value(&args)?),
            ..ToolOutput::ok(format!("echoed {}", args.name))
        })
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Target {
    target: String,
}

struct Counted(&'static str, Risk, Arc<AtomicUsize>);

impl tools::ErasedTool for Counted {
    fn spec(&self) -> tools::ToolSpec {
        tools::ToolSpec {
            name: self.0.to_owned(),
            description: format!("{} (smoke fixture)", self.0),
            schema: serde_json::to_value(schemars::schema_for!(Target)).unwrap_or_default(),
            risk: self.1,
            direct: false,
            returns: "dict".into(),
        }
    }
    fn call(
        &self,
        args: serde_json::Value,
        _: ToolCx,
    ) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        Box::pin(async move {
            let t: Target = serde_json::from_value(args)?;
            let n = self.2.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(ToolOutput {
                structured: Some(serde_json::json!({ "target": t.target, "runs": n })),
                ..ToolOutput::ok(format!("{} ran on {}", self.0, t.target))
            })
        })
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TokenArgs {
    token: String,
}

/// Sees the expanded secret and returns it: the registry's scrub must turn it
/// back into the placeholder before an app or client sees it.
struct UsesSecret;

#[async_trait::async_trait]
impl Tool for UsesSecret {
    const NAME: &'static str = "smoke_secret";
    const RISK: Risk = Risk::ReadOnly;
    type Args = TokenArgs;
    fn description(&self) -> String {
        "Reports the token it received (smoke fixture).".into()
    }
    async fn call(&self, args: TokenArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "expanded": args.token == SECRET_VALUE,
                "token": args.token,
            })),
            ..ToolOutput::ok(format!("token was {}", args.token))
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

const SOURCE: &str = r###"root = Stack([title, echoText, controls, result])
$name = "world"
title = TextContent("## Smoke")
echo = Query("smoke_echo", {name: $name}, {name: "loading"}, 30)
echoText = TextContent("Echo: " + echo.name)
mutate = Mutation("smoke_mutate", {target: $name})
wipe = Mutation("smoke_wipe", {target: $name})
secret = Mutation("smoke_secret", {token: "{{secret:SMOKE_TOKEN}}"})
peek = Query("smoke_secret", {token: "{{secret:SMOKE_TOKEN}}"}, {token: ""}, 60)
controls = Buttons([mBtn, wBtn, sBtn])
mBtn = Button("Mutate", Action([@Run(mutate), @Run(echo)]))
wBtn = Button("Wipe " + $name, Action([@Run(wipe)]), null, "destructive")
sBtn = Button("Use secret", Action([@Run(secret)]))
result = TextContent(mutate.status == "success" ? mutate.data.target : "")
"###;

struct Smoke {
    failures: usize,
}

/// App actor asks reply `Result<_, String>`; kameo lifts the `Err` into a
/// `SendError` that is not a `std::error::Error`.
fn err<M>(e: kameo::error::SendError<M, String>) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}

impl Smoke {
    fn step(&mut self, name: &str, ok: bool, detail: serde_json::Value) {
        if !ok {
            self.failures += 1;
        }
        println!(
            "{}",
            serde_json::json!({ "step": name, "ok": ok, "detail": detail })
        );
    }
}

fn read(
    record: &str,
    statement: &str,
    request: &str,
    args: serde_json::Value,
    generation: u64,
) -> CallArgs {
    CallArgs {
        record_call_id: record.into(),
        instance_id: "smoke-view".into(),
        generation,
        statement_id: statement.into(),
        request_id: request.into(),
        args,
        trigger: Trigger::Load,
        action_id: None,
    }
}

fn press(
    record: &str,
    statement: &str,
    request: &str,
    args: serde_json::Value,
    generation: u64,
) -> CallArgs {
    CallArgs {
        trigger: Trigger::Press,
        action_id: Some(format!("act-{request}")),
        ..read(record, statement, request, args, generation)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber_init();
    let mut argv = std::env::args().skip(1);
    let data_dir = match argv
        .next()
        .or_else(|| std::env::var("THEUTH_DATA_DIR").ok())
    {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::temp_dir().join(format!("theuth-app-smoke-{}", std::process::id())),
    };
    let reopen = argv.next();
    let reopened = reopen.is_some();
    std::fs::create_dir_all(&data_dir)?;
    let config_path = data_dir.join("config.yaml");
    let mut config = figment::Figment::new();
    if config_path.is_file() {
        config = config.merge(figment::providers::Yaml::file(&config_path));
    }
    let rt = Runtime::new(config, data_dir.clone());
    tools::secrets::install(Arc::new(FixtureSecrets));

    Box::new(tools::ToolsPlugin).start(&rt).await?;
    let reg = ActorRef::<ToolRegistry>::lookup(tools::REGISTRY_ACTOR)?
        .ok_or_else(|| anyhow::anyhow!("no tool registry"))?;
    let mutations = Arc::new(AtomicUsize::new(0));
    let wipes = Arc::new(AtomicUsize::new(0));
    for t in [
        Arc::new(Echo) as tools::DynTool,
        Arc::new(Counted("smoke_mutate", Risk::Mutating, mutations.clone())),
        Arc::new(Counted("smoke_wipe", Risk::Destructive, wipes.clone())),
        Arc::new(UsesSecret),
    ] {
        reg.ask(tools::RegisterTool(t)).await?;
    }
    Box::new(artifact::ArtifactPlugin).start(&rt).await?;
    Box::new(AppPlugin).start(&rt).await?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Events::spawn_with_mailbox(Events(events.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Events, AppCallChanged>(&sink).await;

    let mut s = Smoke { failures: 0 };
    let mut app_tool = false;
    for _ in 0..200 {
        if reg
            .ask(tools::ListTools(None))
            .await?
            .iter()
            .any(|t| t.name == "app")
        {
            app_tool = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    s.step(
        "app tool registered",
        app_tool,
        serde_json::json!({ "parser": "in-process (js-runtime)", "library_hash": app::hash() }),
    );
    let apps = ActorRef::<Apps>::lookup(app::APP_ACTOR)?
        .ok_or_else(|| anyhow::anyhow!("app actor not registered (app disabled?)"))?;
    let peer = RpcPeer::internal();
    let session = SessionId::new();
    let call = |args: CallArgs| CallTool {
        peer: peer.clone(),
        args,
    };

    let record = match reopen {
        Some(r) => r,
        None => {
            let out = reg
                .ask(Invoke(ToolCall {
                    id: format!("smoke-{}", uuid_like()),
                    name: "app".into(),
                    args: serde_json::json!({ "name": "smoke", "title": "Smoke", "source": SOURCE }),
                    session,
                    quiet: false,
                }))
                .await?;
            s.step(
                "publish via registry Invoke(app)",
                !out.is_error,
                serde_json::json!({ "content": out.content, "structured": out.structured }),
            );
            if out.is_error {
                std::process::exit(1);
            }
            out.structured
                .and_then(|v| v["record_call_id"].as_str().map(str::to_owned))
                .ok_or_else(|| anyhow::anyhow!("receipt without record_call_id"))?
        }
    };

    let doc = apps
        .ask(GetApp {
            record_call_id: record.clone(),
        })
        .await;
    let (session, generation) = match &doc {
        Ok(d) => {
            s.step(
                "get_app",
                d.library_hash == app::hash() && d.diagnostics.is_empty() && d.bindings.len() == 5,
                serde_json::json!({ "hash": d.library_hash, "bindings": d.bindings, "diagnostics": d.diagnostics, "version": d.artifact.version }),
            );
            (d.artifact.session, 1)
        }
        Err(e) => {
            s.step(
                "get_app",
                false,
                serde_json::json!({ "error": e.to_string() }),
            );
            std::process::exit(1);
        }
    };

    let r = apps
        .ask(call(read(
            &record,
            "echo",
            "r-load",
            serde_json::json!({ "name": "world" }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "automatic read runs (read_only, Allow)",
        r.status == CallStatus::Succeeded
            && r.structured.as_ref().is_some_and(|v| v["name"] == "world"),
        serde_json::json!(r),
    );

    let r = apps
        .ask(call(read(
            &record,
            "peek",
            "r-peek",
            serde_json::json!({ "token": PLACEHOLDER }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "automatic secret-bearing read needs approval (no dialog, nothing ran)",
        r.status == CallStatus::NeedsApproval,
        serde_json::json!(r),
    );

    let r = apps
        .ask(call(read(
            &record,
            "mutate",
            "r-auto-mut",
            serde_json::json!({ "target": "world" }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "mutation on a timer/load is unavailable",
        r.status == CallStatus::Unavailable && mutations.load(Ordering::SeqCst) == 0,
        serde_json::json!(r),
    );

    let r = apps
        .ask(call(press(
            &record,
            "mutate",
            "p-mutate",
            serde_json::json!({ "target": "world" }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "one press runs the mutating tool once, zero approval dialogs",
        r.status == CallStatus::Succeeded && mutations.load(Ordering::SeqCst) == 1,
        serde_json::json!(r),
    );
    let r = apps
        .ask(call(press(
            &record,
            "mutate",
            "p-mutate",
            serde_json::json!({ "target": "world" }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "same request id again returns the stored result without re-running",
        r.status == CallStatus::Succeeded && mutations.load(Ordering::SeqCst) == 1,
        serde_json::json!(r),
    );
    let r = apps
        .ask(call(press(
            &record,
            "mutate",
            "p-mutate",
            serde_json::json!({ "target": "else" }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "same request id with different input is refused",
        r.status == CallStatus::Unavailable && mutations.load(Ordering::SeqCst) == 1,
        serde_json::json!(r),
    );

    let r = apps
        .ask(call(press(
            &record,
            "wipe",
            "p-wipe",
            serde_json::json!({ "target": "world" }),
            generation,
        )))
        .await
        .map_err(err)?;
    s.step(
        "destructive press runs once on activation",
        r.status == CallStatus::Succeeded && wipes.load(Ordering::SeqCst) == 1,
        serde_json::json!(r),
    );

    let r = apps
        .ask(call(press(
            &record,
            "secret",
            "p-secret",
            serde_json::json!({ "token": PLACEHOLDER }),
            generation,
        )))
        .await
        .map_err(err)?;
    let expanded = r.structured.as_ref().is_some_and(|v| v["expanded"] == true);
    let leaked = serde_json::to_string(&r)?.contains(SECRET_VALUE);
    s.step(
        "secret press: tool saw the value, app result carries only the placeholder",
        r.status == CallStatus::Succeeded && expanded && !leaked,
        serde_json::json!(r),
    );

    let st = apps
        .ask(CallStatusOf {
            peer: peer.clone(),
            request_id: "p-wipe".into(),
        })
        .await
        .map_err(err)?;
    s.step(
        "status of a finished press is durable",
        st.status == CallStatus::Succeeded,
        serde_json::json!(st),
    );
    let st = apps
        .ask(CallStatusOf {
            peer: peer.clone(),
            request_id: "never-sent".into(),
        })
        .await
        .map_err(err)?;
    s.step(
        "status of an unknown request is unknown",
        st.status == CallStatus::Unknown,
        serde_json::json!(st),
    );

    let stale = apps
        .ask(call(read(
            &record,
            "echo",
            "r-stale",
            serde_json::json!({ "name": "x" }),
            generation.saturating_sub(1),
        )))
        .await
        .map_err(err)?;
    s.step(
        "older generation is cancelled",
        stale.status == CallStatus::Cancelled,
        serde_json::json!(stale),
    );

    if !reopened {
        let out = reg
            .ask(Invoke(ToolCall {
                id: format!("smoke-{}", uuid_like()),
                name: "app".into(),
                args: serde_json::json!({
                    "name": "smoke", "edit": true, "base_call_id": record,
                    "source": "title = TextContent(\"## Smoke v2\")\n"
                }),
                session,
                quiet: false,
            }))
            .await?;
        let v2 = out
            .structured
            .as_ref()
            .and_then(|v| v["record_call_id"].as_str().map(str::to_owned));
        s.step(
            "edit merges onto the base and commits v2",
            !out.is_error && v2.is_some(),
            serde_json::json!({ "content": out.content }),
        );
        let out = reg
            .ask(Invoke(ToolCall {
                id: format!("smoke-{}", uuid_like()),
                name: "app".into(),
                args: serde_json::json!({
                    "name": "smoke", "edit": true, "base_call_id": record,
                    "source": "title = TextContent(\"## lost update\")\n"
                }),
                session,
                quiet: false,
            }))
            .await?;
        s.step(
            "second edit on the old base is a conflict naming the latest",
            out.is_error && out.content.contains("conflict"),
            serde_json::json!({ "content": out.content }),
        );
        if let Some(v2) = v2 {
            let r = apps
                .ask(call(read(
                    &v2,
                    "echo",
                    "r-v2",
                    serde_json::json!({ "name": "v2" }),
                    generation + 1,
                )))
                .await
                .map_err(err)?;
            s.step(
                "the open view adopts v2 with the next generation",
                r.status == CallStatus::Succeeded,
                serde_json::json!(r),
            );
            let d = apps.ask(GetApp { record_call_id: v2 }).await;
            s.step(
                "v2 source carries the patched statement",
                d.as_ref()
                    .is_ok_and(|d| d.source.contains("Smoke v2") && d.artifact.version == 2),
                serde_json::json!(d.map(|d| d.source).unwrap_or_else(|e| e.to_string())),
            );
        }
    }

    apps.ask(CloseApp {
        peer: peer.clone(),
        instance_id: "smoke-view".into(),
    })
    .await
    .map_err(err)?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let evs = events.lock();
    s.step(
        "AppCallChanged published per finished call",
        evs.iter().any(|e| e.request_id == "p-wipe" && e.status == CallStatus::Succeeded && e.risk == Risk::Destructive)
            && evs.iter().all(|e| e.session == session),
        serde_json::json!(evs.iter().map(|e| serde_json::json!({ "request": e.request_id, "status": e.status, "tool": e.tool, "ms": e.ms })).collect::<Vec<_>>()),
    );
    drop(evs);

    println!(
        "{}",
        serde_json::json!({ "done": true, "failures": s.failures, "record_call_id": record, "data_dir": data_dir, "restart_check": format!("cargo run -p app --example smoke -- {} {record}", data_dir.display()) })
    );
    rt.shutdown.cancel();
    std::process::exit(if s.failures == 0 { 0 } else { 1 });
}

fn uuid_like() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    )
}

fn tracing_subscriber_init() {
    // Logs to stderr keep stdout as JSON lines; THEUTH_LOG=debug for the helper/tool path.
    let filter = std::env::var("THEUTH_LOG").unwrap_or_else(|_| "warn".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
