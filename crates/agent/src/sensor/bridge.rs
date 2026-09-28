//! The Monty path (40 §4.2–§4.3): one checkout per evaluation under the
//! sensor profile, `cwd` mounted read-only at `/cwd`, and a fixed five-entry
//! tool table answering the sandbox's calls.
//!
//! The table calls the tool structs directly — never the registry's `Invoke`
//! (40 §4.3): a sensor is read-only by construction, cannot wait on an
//! approval and must not raise one. Four of the five are `Risk::ReadOnly` on
//! the registry; `http_request` is `Mutating` there, and here the method/host
//! rule is its gate. Secret placeholders expand in `http_request` headers
//! only (37 §2), through the same `tools::secrets` hook the registry uses;
//! every result is scrubbed by the same redactor before it re-enters the
//! sandbox.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use kameo::actor::ActorRef;
use monty_pool::{
    MountSpec, MountSpecMode, Pool, PoolError, ReplConfig, ResumeValue, TurnEvent, on_print_sync,
};
use monty_types::{
    CallArgs, ExcType, MontyException, MontyObject, NameLookupResult, OsPolicy, ResourceLimits,
    SleepMode,
};
use runtime::SessionId;
use serde_json::Value;
use tools::{ErasedTool, Risk, SessionResources, ToolCx, ToolOutput, ToolSpec};

use super::{Call, SensorError, SensorMeta, SensorProfile};
use crate::repl::{from_monty, stubs, to_monty};

/// The allowlist (40 §4.3) with each tool's positional parameter order: the
/// order [`stubs`] renders (required in schema order, then optional by name),
/// so `http_request(url, "GET", None, headers)` lands as the stub says.
const TABLE: [(&str, &[&str]); 5] = [
    ("http_request", &["url", "method", "body", "headers"]),
    (
        "read_file",
        &[
            "path", "limit", "numbers", "offset", "outline", "ranges", "tail",
        ],
    ),
    ("glob", &["pattern", "hidden", "limit", "path"]),
    ("list_dir", &["path"]),
    ("knowledge_search", &["query", "limit"]),
];

/// The five specs, for the `.pyi` the checker sees (40 §4.3: "stubs = the
/// allowlist only") — so a sensor author gets the signatures the model sees.
static SPECS: LazyLock<Vec<ToolSpec>> = LazyLock::new(|| {
    let hand = |name: &str, schema: Value, returns: &str, description: &str| ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
        risk: Risk::ReadOnly,
        direct: false,
        returns: returns.to_owned(),
    };
    vec![
        hand(
            "http_request",
            serde_json::to_value(schemars::schema_for!(web::http::HttpRequestArgs))
                .unwrap_or_default(),
            "dict",
            "One HTTP request; GET anywhere, POST only to the sensor's `# hosts:`. Returns {url, status, headers, body, truncated, took_ms}.",
        ),
        tools::fs::ReadFile.spec(),
        tools::glob::Glob.spec(),
        tools::fs::ListDir.spec(),
        hand(
            "knowledge_search",
            serde_json::to_value(schemars::schema_for!(knowledge::tools::SearchArgs))
                .unwrap_or_default(),
            "list[dict]",
            "Search the knowledge base by keywords.",
        ),
    ]
});

/// The bridge's `http_request`, built once: 46 §5's bounds, its own client.
static HTTP: LazyLock<Result<web::http::HttpRequestTool, String>> = LazyLock::new(|| {
    web::fetch::Fetcher::new_with_timeouts(
        "theuth-sensor",
        web::http::MAX_BYTES,
        5,
        web::http::TIMEOUT,
        web::fetch::CONNECT_TIMEOUT,
    )
    .map(|f| web::http::HttpRequestTool {
        fetcher: Arc::new(f),
        deadline: web::http::TIMEOUT,
    })
    .map_err(|e| e.to_string())
});

/// One evaluation's bridge state.
struct Bridge<'a> {
    meta: &'a SensorMeta,
    profile: &'a SensorProfile,
    cwd: PathBuf,
    cx: ToolCx,
    /// Response body bytes fetched so far (40 §5: ≤ 1 MiB per evaluation).
    fetched: usize,
    /// Sandbox future id → (tool, merged JSON args).
    pending: HashMap<u32, (&'static str, Value)>,
}

/// Load the sensor, run `call.fn_name(...)` on a fresh checkout within
/// `call.timeout`, and return the decoded result value.
pub(super) async fn run(
    pool: &Arc<Pool>,
    profile: &SensorProfile,
    data_dir: &Path,
    call: &Call<'_>,
) -> Result<Value, SensorError> {
    let Call {
        meta,
        fn_name,
        state,
        args,
        cwd,
        timeout,
    } = *call;
    let src = std::fs::read_to_string(&meta.path)
        .map_err(|e| SensorError::Header(format!("{}: {e}", meta.name)))?;
    // `json.loads` of a repr'd literal: the checker learns nothing from
    // `inputs`, so the values travel inside the checked source.
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
    let call = match state {
        Some(s) => format!(
            "{fn_name}(_json.loads({}), _json.loads({}))",
            MontyObject::string(s.to_string()).py_repr(),
            MontyObject::string(args_json).py_repr()
        ),
        None => format!(
            "{fn_name}(_json.loads({}))",
            MontyObject::string(args_json).py_repr()
        ),
    };
    let code = format!("{src}\n\nimport json as _json\n_r = await {call}\n_r\n");
    let cwd = cwd.map_or_else(std::env::temp_dir, Path::to_path_buf);
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let cfg = ReplConfig {
        script_name: meta.name.clone(),
        limits: Some(ResourceLimits {
            max_memory: Some(profile.max_memory),
            max_suspensions: profile.max_suspensions,
            max_total_sleep: Some(profile.max_total_sleep),
            ..ResourceLimits::default()
        }),
        type_check: true,
        // The bridge raises `PermissionError` (40 §4.3); monty's trimmed
        // typeshed omits it, as it omits `open()` (see `repl::stubs`).
        type_check_stubs: Some(format!(
            "{}class PermissionError(Exception): ...\n",
            stubs(&SPECS)
        )),
        os_policy: OsPolicy {
            sleep: SleepMode::Zero,
            ..OsPolicy::default()
        },
        ..ReplConfig::default()
    };
    let mounts = match MountSpec::new("/cwd", &cwd, MountSpecMode::ReadOnly) {
        Ok(m) => vec![m],
        Err(e) => {
            tracing::warn!("sensor {}: mount /cwd → {}: {e}", meta.name, cwd.display());
            Vec::new()
        }
    };
    let mut bridge = Bridge {
        meta,
        profile,
        cwd,
        cx: ToolCx {
            session: SessionId::new(),
            call_id: format!("sensor:{}", meta.name),
            quiet: true,
            resources: SessionResources::durable_arc(data_dir),
        },
        fetched: 0,
        pending: HashMap::new(),
    };
    let mut checkout = pool.checkout(&cfg).await.map_err(|e| pool_error(meta, e))?;
    let mut stdout = String::new();
    let mut on_print = on_print_sync(|_, t| stdout.push_str(t));
    // The pool's `request_timeout` is per pool, not per checkout; the
    // watcher's clock is enforced here. A checkout dropped mid-turn kills its
    // worker (monty-pool's `Drop`), so `while True: pass` dies with it.
    let drive = async {
        let mut ev = checkout
            .feed(code, Vec::new(), mounts, false, &mut on_print)
            .await?;
        loop {
            ev = match ev {
                TurnEvent::Complete(v) => return Ok(v),
                TurnEvent::NameLookup { name, .. } => {
                    let answer = if TABLE.iter().any(|(n, _)| *n == name) {
                        NameLookupResult::Value(MontyObject::function(name, None))
                    } else {
                        NameLookupResult::Undefined
                    };
                    checkout.resume_name_lookup(answer, &mut on_print).await?
                }
                TurnEvent::FunctionCall {
                    function_name,
                    args,
                    call_id,
                    ..
                } => {
                    let answer = match TABLE.iter().find(|(n, _)| *n == function_name) {
                        Some((name, order)) => {
                            bridge.pending.insert(call_id, (name, merge(order, &args)));
                            ResumeValue::Future
                        }
                        None => ResumeValue::NotFound,
                    };
                    checkout.resume(answer, &mut on_print).await?
                }
                TurnEvent::ResolveFutures {
                    pending_call_ids, ..
                } => {
                    // Sequential, so the fetched-bytes budget is exact.
                    let mut results = Vec::with_capacity(pending_call_ids.len());
                    for id in pending_call_ids {
                        let r = match bridge.pending.remove(&id) {
                            Some((name, args)) => bridge.call(name, args).await,
                            None => ResumeValue::Error(MontyException::new(
                                ExcType::RuntimeError,
                                Some("unknown pending call".into()),
                            )),
                        };
                        results.push((id, r));
                    }
                    checkout.resume_futures(results, &mut on_print).await?
                }
                // `max_total_sleep` is 0 and `SleepMode::Zero`: a sensor waits
                // through `next`, not `sleep` (40 §4.2).
                TurnEvent::OsCall {
                    system_sleep: Some(_),
                    ..
                } => {
                    checkout
                        .resume(ResumeValue::Return(MontyObject::none()), &mut on_print)
                        .await?
                }
                TurnEvent::OsCall { .. } => match checkout.resume_from_mounts(&mut on_print).await?
                {
                    Some(ev) => ev,
                    None => {
                        checkout
                            .resume(ResumeValue::NotHandled, &mut on_print)
                            .await?
                    }
                },
            };
        }
    };
    let out = match tokio::time::timeout(timeout, drive).await {
        Ok(Ok(v)) => Ok(from_monty(v.as_ref())),
        Ok(Err(e)) => Err(pool_error(meta, e)),
        Err(_) => return Err(SensorError::Timeout(timeout)),
    };
    drop(on_print);
    if let Err(e) = checkout.finish().await {
        tracing::debug!("sensor {} checkout finish: {e}", meta.name);
    }
    if !stdout.is_empty() {
        tracing::info!(target: "sensor", "{} printed: {}", meta.name, stdout.trim_end());
    }
    out
}

/// A pool error as the gate's reason (40 §3.1). A typing failure naming an
/// undefined name is the allowlist refusing a tool at load (40 §4.3).
fn pool_error(meta: &SensorMeta, e: PoolError) -> SensorError {
    match e {
        PoolError::Typing(diag) => {
            // A tool name is snake_case; a capitalised name is a builtin the
            // checker does not know, reported as the diagnostic it is.
            let name = diag
                .split("Name `")
                .nth(1)
                .and_then(|s| s.split('`').next())
                .filter(|n| {
                    diag.contains("used when not defined")
                        && n.chars()
                            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                });
            match name {
                Some(n) => SensorError::Budget(format!("sensor uses {n}, not allowed")),
                None => SensorError::Failed(format!("{}: type check failed:\n{diag}", meta.name)),
            }
        }
        PoolError::Runtime(exc) if exc.exc_type() == ExcType::MemoryError => {
            SensorError::Budget(format!("heap: {exc}"))
        }
        PoolError::Runtime(exc) => {
            SensorError::Failed(tools::secrets::redact(&exc.to_string()).into_owned())
        }
        other => SensorError::Unavailable(format!("sensor runtime: {other}")),
    }
}

/// kwargs by name; positional args in `order`.
fn merge(order: &[&str], args: &CallArgs) -> Value {
    let mut out = serde_json::Map::new();
    for (i, a) in args.args().enumerate() {
        if let Some(k) = order.get(i) {
            out.insert((*k).to_owned(), from_monty(a));
        }
    }
    for (k, v) in args.kwargs() {
        if let Some(k) = k.as_str() {
            out.insert(k.to_owned(), from_monty(v));
        }
    }
    Value::Object(out)
}

fn deny(msg: impl Into<String>) -> ResumeValue {
    ResumeValue::Error(MontyException::new(
        ExcType::PermissionError,
        Some(msg.into()),
    ))
}

/// 40 §4.3's method rule: `GET` anywhere, `POST` only to a declared host.
pub(super) fn host_rule(method: &str, url: &str, hosts: &[String]) -> Result<(), String> {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => Ok(()),
        "POST" => {
            let host = url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                .ok_or_else(|| format!("http_request: no host in `{url}`"))?;
            if hosts.contains(&host) {
                Ok(())
            } else {
                Err(format!(
                    "http_request: POST to {host} is not allowed; declare it in `# hosts:`"
                ))
            }
        }
        other => Err(format!(
            "http_request: method {other} is not allowed in a sensor (GET, or POST to `# hosts:`)"
        )),
    }
}

impl Bridge<'_> {
    /// Run one allowlisted call and turn its output into the sandbox's value
    /// or exception (the shape `repl::tool_result` gives a cell).
    async fn call(&mut self, name: &'static str, mut args: Value) -> ResumeValue {
        let out = match name {
            "http_request" => match self.http(&mut args).await {
                Ok(out) => out,
                Err(e) => return deny(e),
            },
            "knowledge_search" => self.knowledge(args).await,
            _ => match self.rooted(name, &mut args) {
                Ok(()) => {
                    let tool: &dyn ErasedTool = match name {
                        "read_file" => &tools::fs::ReadFile,
                        "glob" => &tools::glob::Glob,
                        _ => &tools::fs::ListDir,
                    };
                    tool.call(args, self.cx.clone())
                        .await
                        .unwrap_or_else(|e| ToolOutput::err(e.to_string()))
                }
                Err(e) => return deny(e),
            },
        };
        result(name, out)
    }

    /// The method/host rule, header-only secret expansion (37 §2), the
    /// fetched-bytes budget (40 §5).
    async fn http(&mut self, args: &mut Value) -> Result<ToolOutput, String> {
        let method = args["method"].as_str().unwrap_or("").to_owned();
        let url = args["url"].as_str().unwrap_or("").to_owned();
        host_rule(&method, &url, &self.meta.hosts)?;
        if tools::secrets::mentions(&args["url"]) || tools::secrets::mentions(&args["body"]) {
            return Err(
                "http_request: secret placeholders are allowed in headers only (37 §2)".into(),
            );
        }
        if let Some(h) = args.get_mut("headers").filter(|h| !h.is_null()) {
            tools::secrets::expand(h).map_err(|e| format!("http_request: {e}"))?;
        }
        let tool = HTTP.as_ref().map_err(|e| format!("http client: {e}"))?;
        let mut out = ErasedTool::call(tool, args.take(), self.cx.clone())
            .await
            .unwrap_or_else(|e| ToolOutput::err(e.to_string()));
        let body = out
            .structured
            .as_ref()
            .and_then(|s| s.get("body"))
            .map_or(0, |b| match b {
                Value::String(s) => s.len(),
                Value::Null => 0,
                other => other.to_string().len(),
            });
        self.fetched += body;
        if self.fetched > self.profile.max_fetch_bytes {
            out = ToolOutput::err(format!(
                "over budget: {} bytes fetched this evaluation (limit {})",
                self.fetched, self.profile.max_fetch_bytes
            ));
        }
        Ok(out)
    }

    /// `knowledge_search` through the knowledge actor's store handle; the
    /// bundle is read, nothing written.
    async fn knowledge(&self, args: Value) -> ToolOutput {
        let Ok(Some(actor)) =
            ActorRef::<knowledge::KnowledgeActor>::lookup(knowledge::KNOWLEDGE_ACTOR)
        else {
            return ToolOutput::err("knowledge_search: knowledge is not running");
        };
        let Ok(k) = actor.ask(knowledge::Handle).await else {
            return ToolOutput::err("knowledge_search: knowledge is not running");
        };
        ErasedTool::call(&knowledge::tools::SearchTool(k), args, self.cx.clone())
            .await
            .unwrap_or_else(|e| ToolOutput::err(e.to_string()))
    }

    /// Root `path` at `cwd` (40 §4.3): relative paths join it, `/cwd/...` is
    /// the mount's spelling of the same, anything resolving outside is refused.
    fn rooted(&self, name: &str, args: &mut Value) -> Result<(), String> {
        let given = match args.get("path") {
            None | Some(Value::Null) => {
                args["path"] = Value::String(self.cwd.display().to_string());
                return Ok(());
            }
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(format!("{name}: path must be a string")),
        };
        let rel = Path::new(&given)
            .strip_prefix("/cwd")
            .map_or(Path::new(&given), |p| p);
        let joined = self.cwd.join(rel);
        let real = std::fs::canonicalize(&joined).map_err(|e| format!("{name}: {given}: {e}"))?;
        if !real.starts_with(&self.cwd) {
            return Err(format!("{name}: {given} is outside cwd"));
        }
        args["path"] = Value::String(real.display().to_string());
        Ok(())
    }
}

/// Tool output → sandbox return value or exception, redacted (37 §3.4).
fn result(name: &str, mut out: ToolOutput) -> ResumeValue {
    tools::secrets::redact_output(&mut out);
    if out.is_error {
        let msg = if out.content.starts_with(name) {
            out.content
        } else {
            format!("{name}: {}", out.content)
        };
        return ResumeValue::Error(MontyException::new(ExcType::RuntimeError, Some(msg)));
    }
    let value = match out.structured {
        Some(v) => to_monty(&v),
        None => match serde_json::from_str::<Value>(&out.content) {
            Ok(v @ (Value::Object(_) | Value::Array(_))) => to_monty(&v),
            _ => MontyObject::string(out.content),
        },
    };
    ResumeValue::Return(value)
}
