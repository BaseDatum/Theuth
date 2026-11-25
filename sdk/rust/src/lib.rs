// theuth plugin SDK for Rust 0.1.0 (design 216): std threads + serde_json. Vendored as src/theuth.rs.
//! Write a theuth plugin in a few lines: `mod theuth; use theuth::*;`, then
//! `plugin().action("inc", |_| { plugin().state.add("n", 1); }); plugin().run();`
//!
//! plugin.yaml is the contract: the host learns tools, subscriptions and placements from it, never by
//! running this code. `<bin> --theuth-manifest` prints what the code registered, to compare.
//!
//! Stdout is the protocol: `println!` corrupts it; use `plugin().log(..)` (stderr is the log file).
//! Handlers run one at a time on the `run()` thread; a reader thread routes responses, so `call()`/
//! `request()` from a handler never deadlocks. No SDK lock is held while a handler runs. A panic or an
//! `Err` in a handler answers -32603 (tools: an `isError` result); the process stays up.
//! Raw hatch: `send`, `request`, `on_request`, `on_notify`.
#![allow(dead_code, unused_imports)]

use std::any::Any;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{mpsc, Arc, LazyLock, Mutex, MutexGuard};
use std::{env, fmt, fs, process, thread};

pub use serde_json::{json, Map, Value};

pub const PROTOCOL: i64 = 1;

/// The plugin singleton. Register handlers on it, then call `plugin().run()`.
pub fn plugin() -> &'static Plugin {
    static P: LazyLock<Plugin> = LazyLock::new(Plugin::new);
    &P
}

/// A JSON-RPC error. Return it from a handler to answer with it; `call`/`request` return it.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError { pub code: i64, pub message: String, pub data: Option<Value> }

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self { RpcError { code, message: message.into(), data: None } }
    /// `data.retry_after_ms` of a rate-limit error.
    pub fn retry_after_ms(&self) -> Option<i64> { self.data.as_ref()?.get("retry_after_ms")?.as_i64() }
    fn from_json(e: &Value) -> Self {
        let message = e.get("message").and_then(Value::as_str).unwrap_or("").to_string();
        let data = e.get("data").filter(|d| !d.is_null()).cloned();
        RpcError { code: e.get("code").and_then(Value::as_i64).unwrap_or(-32603), message, data }
    }
    pub fn to_json(&self) -> Value {
        let mut e = json!({"code": self.code, "message": self.message});
        if let Some(d) = &self.data { e["data"] = d.clone() }
        e
    }
}
impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "{} ({})", self.message, self.code) }
}
impl std::error::Error for RpcError {}

/// A handler's return value before the per-kind mapping (see `action`, `query`, `command`, `tool`).
#[derive(Debug, Clone, PartialEq)]
pub enum Reply { Nil, Text(String), Json(Value) }

/// A handler failure: `Rpc` answers with that error; `Error` answers -32603 (tools: `isError` text).
#[derive(Debug, Clone, PartialEq)]
pub enum Fail { Rpc(RpcError), Error(String) }
impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self { Fail::Rpc(e) => e.fmt(f), Fail::Error(m) => f.write_str(m) }
    }
}

/// What a handler may return: `()`, `String`/`&str`, `Value`, numbers, `bool`, `Option<T>`, `Result<T, E>`.
pub trait IntoReply { fn into_reply(self) -> Result<Reply, Fail>; }
impl IntoReply for () { fn into_reply(self) -> Result<Reply, Fail> { Ok(Reply::Nil) } }
impl IntoReply for String { fn into_reply(self) -> Result<Reply, Fail> { Ok(Reply::Text(self)) } }
impl IntoReply for &str { fn into_reply(self) -> Result<Reply, Fail> { Ok(Reply::Text(self.into())) } }
impl IntoReply for Value {
    fn into_reply(self) -> Result<Reply, Fail> { Ok(if self.is_null() { Reply::Nil } else { Reply::Json(self) }) }
}
impl IntoReply for Map<String, Value> { fn into_reply(self) -> Result<Reply, Fail> { Value::Object(self).into_reply() } }
macro_rules! json_reply { ($($t:ty),*) => {$(
    impl IntoReply for $t { fn into_reply(self) -> Result<Reply, Fail> { Value::from(self).into_reply() } }
)*} }
json_reply!(bool, i32, i64, u32, u64, usize, f64);
impl<T: IntoReply> IntoReply for Option<T> {
    fn into_reply(self) -> Result<Reply, Fail> { self.map_or(Ok(Reply::Nil), T::into_reply) }
}
impl<T: IntoReply, E: IntoFail> IntoReply for Result<T, E> {
    fn into_reply(self) -> Result<Reply, Fail> { self.map_err(E::into_fail)?.into_reply() }
}

/// Error types a handler may return in `Err`: `RpcError` keeps its code, the rest become -32603.
pub trait IntoFail { fn into_fail(self) -> Fail; }
impl IntoFail for RpcError { fn into_fail(self) -> Fail { Fail::Rpc(self) } }
impl IntoFail for Fail { fn into_fail(self) -> Fail { self } }
macro_rules! text_fail { ($($t:ty),*) => {$(
    impl IntoFail for $t { fn into_fail(self) -> Fail { Fail::Error(self.to_string()) } }
)*} }
text_fail!(String, &str, io::Error, serde_json::Error, fmt::Error,
    Box<dyn std::error::Error>, Box<dyn std::error::Error + Send + Sync>);

/// A host request (`action`, `query`, `command`, `tools/call`, `render`, ...). `name`: action / query /
/// command / tool name. `args`: `params.args` (`tools/call`: `params.arguments`). `raw`: the whole message.
#[derive(Debug, Clone, Default)]
pub struct Req {
    pub placement: Option<String>, pub name: String, pub args: Value, pub session: Option<String>,
    pub client: Option<String>, pub form: Value, pub activation: Option<String>, pub id: Value, pub raw: Value,
}

/// An `event` notification. `data` is the JSON the host sent, unfiltered.
#[derive(Debug, Clone, Default)]
pub struct Event {
    pub topic: String, pub version: Value, pub r#type: String, pub session: Option<String>,
    pub at_ms: Value, pub data: Value, pub raw: Value,
}

/// Full `status` form. Dot ok|busy|warn|error|off|none; `badge` int (0 hides) or ≤4 chars; label ≤16;
/// tooltip ≤200. `placement` defaults to the single granted one.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub dot: String, pub badge: Option<Value>, pub label: Option<String>, pub tooltip: Option<String>,
    pub placement: Option<String>,
}

/// Full `ui` form. Without `source`: format openui (or unset) -> an OpenUI card of `text` + `buttons`;
/// any other format -> `source = text`. `source` is sent unchanged.
#[derive(Debug, Clone, Default)]
pub struct Ui {
    pub text: String, pub buttons: Vec<Button>, pub source: Option<String>, pub format: Option<String>,
    pub placement: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button { pub label: String, pub action: String, pub style: String }
impl Button {
    /// A button with style "secondary".
    pub fn new(label: &str, action: &str) -> Self {
        Button { label: label.into(), action: action.into(), style: "secondary".into() }
    }
}
impl From<(&str, &str)> for Button { fn from((l, a): (&str, &str)) -> Self { Button::new(l, a) } }
impl From<(&str, &str, &str)> for Button {
    fn from((l, a, s): (&str, &str, &str)) -> Self { Button { style: s.into(), ..Button::new(l, a) } }
}

/// The OpenUI source `ui(text, buttons)` sends: deterministic, so equal input never remounts.
pub fn openui_card(text: &str, buttons: &[Button]) -> String {
    let root = if buttons.is_empty() { "root = Card([body])" } else { "root = Card([body, controls])" };
    let mut lines = vec![root.to_string(), format!("body = TextContent({})", js(text))];
    let mut calls = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        lines.push(format!("a{i} = Mutation({}, {{}})", js(&b.action)));
        calls.push(format!("Button({}, @Run(a{i}), {})", js(&b.label), js(&b.style)));
    }
    if !buttons.is_empty() { lines.push(format!("controls = Buttons([{}])", calls.join(", "))) }
    lines.join("\n")
}

fn js(s: &str) -> String { Value::from(s).to_string() } // serde_json never escapes non-ASCII or <>&
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }
fn s(v: &Value, k: &str) -> Option<String> { v.get(k)?.as_str().map(String::from) }
fn get(v: &Value, k: &str) -> Value { v.get(k).cloned().unwrap_or(Value::Null) }
fn obj(v: Option<&Value>) -> Value { v.filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!({})) }

/// Stdout (one flushed line per message, under a lock), or the `drive()` capture.
struct Wire { sink: Mutex<Option<Vec<Value>>> }
impl Wire {
    fn write(&self, msg: Value) {
        let mut sink = lock(&self.sink);
        if let Some(out) = sink.as_mut() { return out.push(msg) }
        let mut o = io::stdout().lock();
        let _ = writeln!(o, "{msg}").and_then(|_| o.flush());
    }
    fn send(&self, method: &str, params: Value) {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": obj(Some(&params))}))
    }
    fn log(&self, message: &str, level: &str, fields: Value) {
        let mut p = json!({"level": level, "message": message});
        if !fields.is_null() && fields != json!({}) { p["fields"] = fields }
        self.send("log", p)
    }
}

/// A JSON object persisted to `<dir>/state.json`; dir = $THEUTH_PLUGIN_DATA, else hello `data_dir`, else
/// `./.plugin-data`. Lazy-loaded; `set`/`add` save atomically (tmp + rename). A corrupt file reads as `{}`.
pub struct State { inner: Mutex<(Option<Map<String, Value>>, Option<String>)>, wire: Arc<Wire> }
impl State {
    fn locked<T>(&self, f: impl FnOnce(&mut Map<String, Value>, &Path) -> T) -> T {
        let mut warn = None;
        let out = {
            let mut g = lock(&self.inner);
            let dir = env::var("THEUTH_PLUGIN_DATA").ok().filter(|d| !d.is_empty()).or_else(|| g.1.clone());
            let path = PathBuf::from(dir.unwrap_or_else(|| "./.plugin-data".into())).join("state.json");
            let data = g.0.get_or_insert_with(|| match fs::read_to_string(&path).map(|t| serde_json::from_str(&t)) {
                Ok(Ok(Value::Object(m))) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
                Ok(Ok(_)) => Map::new(),
                Ok(Err(e)) => { warn = Some(e.to_string()); Map::new() }
                Err(e) => { warn = Some(e.to_string()); Map::new() }
            });
            f(data, &path)
        };
        if let Some(w) = warn { self.wire.log(&format!("state.json unreadable, starting empty: {w}"), "warn", Value::Null) }
        out
    }
    fn report(&self, r: io::Result<()>) {
        if let Err(e) = r { self.wire.log(&format!("state.json save failed: {e}"), "error", Value::Null) }
    }
    pub fn get(&self, key: &str, default: impl Into<Value>) -> Value {
        let d = default.into();
        self.locked(|m, _| m.get(key).cloned().unwrap_or(d))
    }
    /// The integer at `key`; 0 when missing or not an integer.
    pub fn int(&self, key: &str) -> i64 { self.locked(|m, _| m.get(key).and_then(Value::as_i64).unwrap_or(0)) }
    /// Set and save (a failed save is an error log; call `save()` to see the error).
    pub fn set(&self, key: &str, value: impl Into<Value>) {
        let r = self.locked(|m, p| { m.insert(key.into(), value.into()); save_to(m, p) });
        self.report(r)
    }
    /// `int(key) + n`, saved; one read-modify-write under the state lock. Returns the new value.
    pub fn add(&self, key: &str, n: i64) -> i64 {
        let (v, r) = self.locked(|m, p| {
            let v = m.get(key).and_then(Value::as_i64).unwrap_or(0) + n;
            m.insert(key.into(), v.into());
            (v, save_to(m, p))
        });
        self.report(r);
        v
    }
    pub fn all(&self) -> Map<String, Value> { self.locked(|m, _| m.clone()) }
    pub fn save(&self) -> io::Result<()> { self.locked(|m, p| save_to(m, p)) }
}

fn save_to(m: &Map<String, Value>, p: &Path) -> io::Result<()> {
    if let Some(d) = p.parent() { fs::create_dir_all(d)? }
    let tmp = p.with_file_name("state.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(&serde_json::to_vec(m)?)?;
    f.sync_all()?; // on disk before the rename publishes it
    drop(f);
    fs::rename(&tmp, p)
}

type Handler = Arc<dyn Fn(Req) -> Result<Reply, Fail> + Send + Sync>;
type ValFn = Arc<dyn Fn(Value) + Send + Sync>;
type Hook = Arc<dyn Fn() + Send + Sync>;
/// Answers the plugin's requests in `drive()`: `(method, params) -> result`.
pub type Responder = Arc<dyn Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync>;

/// Request kinds: (method, params key holding the name). Handlers live in `Reg::kinds` in this order.
const KINDS: [(&str, &str); 4] = [("action", "action"), ("query", "query"), ("command", "name"), ("tools/call", "name")];
const TOOL: usize = 3;

#[derive(Default)]
struct Reg {
    kinds: [Vec<(String, Handler)>; 4],
    events: Vec<(String, Arc<dyn Fn(Event) + Send + Sync>)>,
    requests: Vec<(String, Arc<dyn Fn(Value) -> Result<Reply, Fail> + Send + Sync>)>,
    notifies: Vec<(String, ValFn)>,
    start: Vec<Hook>, shutdown: Vec<Hook>, granted: Vec<ValFn>, lagged: Vec<ValFn>, cancel: Vec<ValFn>,
    render: Vec<Arc<dyn Fn(Req) + Send + Sync>>,
}

#[derive(Default)]
struct Sess { hello: Value, grants: Value, data_dir: Option<String>, clients: Value, cancelled: Vec<Value> }

fn put<T>(v: &mut Vec<(String, T)>, k: &str, h: T) {
    match v.iter_mut().find(|(n, _)| n == k) { Some(e) => e.1 = h, None => v.push((k.into(), h)) }
}
fn find<T: Clone>(v: &[(String, T)], k: &str) -> Option<T> { v.iter().find(|(n, _)| n == k).map(|(_, h)| h.clone()) }

/// Run `f`, turning a panic into `Fail::Error(<panic message>)`.
fn guard<T>(f: impl FnOnce() -> Result<T, Fail>) -> Result<T, Fail> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p: Box<dyn Any + Send>| {
        let m = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
        Err(Fail::Error(m.unwrap_or_else(|| "panic".into())))
    })
}

fn plain(r: Reply) -> Value {
    match r { Reply::Nil => Value::Null, Reply::Text(t) => Value::String(t), Reply::Json(v) => v }
}

fn tool_result(r: Reply) -> Value {
    let text = |t: String| json!({"content": [{"type": "text", "text": t}], "isError": false});
    match r {
        Reply::Text(t) => text(t),
        Reply::Nil => text("null".into()),
        Reply::Json(v) if v.get("content").is_some() || v.get("isError").is_some() => v,
        Reply::Json(v) => {
            let mut out = text(v.to_string());
            if v.is_object() { out["structuredContent"] = v }
            out
        }
    }
}

fn eof_error() -> RpcError { RpcError::new(-32006, "host closed stdin") }
fn no_responder(method: &str, _: &Value) -> Result<Value, RpcError> {
    Err(RpcError::new(-32601, format!("drive(): no responder for {method}")))
}

pub struct Plugin {
    /// Persisted state (see `State`).
    pub state: State,
    wire: Arc<Wire>,
    reg: Mutex<Reg>,
    sess: Mutex<Sess>,
    pending: Mutex<(HashMap<i64, mpsc::Sender<Result<Value, RpcError>>>, bool)>, // (waiters, stdin closed)
    next_id: AtomicI64,
    responder: Mutex<Option<Responder>>,
}

impl Default for Plugin { fn default() -> Self { Plugin::new() } }

impl Plugin {
    /// A fresh plugin; plugins use the `plugin()` singleton.
    pub fn new() -> Self {
        let wire = Arc::new(Wire { sink: Mutex::new(None) });
        let state = State { inner: Mutex::new((None, None)), wire: wire.clone() };
        Plugin { state, wire, reg: Mutex::default(), sess: Mutex::default(), pending: Mutex::default(),
                 next_id: AtomicI64::new(0), responder: Mutex::new(None) }
    }
    fn reg<T>(&self, f: impl FnOnce(&mut Reg) -> T) -> T { f(&mut lock(&self.reg)) }
    fn sess<T>(&self, f: impl FnOnce(&mut Sess) -> T) -> T { f(&mut lock(&self.sess)) }
    fn handler<R: IntoReply>(&self, kind: usize, name: &str, f: impl Fn(Req) -> R + Send + Sync + 'static) {
        self.reg(|r| put(&mut r.kinds[kind], name, Arc::new(move |q| f(q).into_reply())))
    }

    // ---- registration (re-registering a name replaces it; hooks accumulate and run in order) ----
    /// Event handler. `name`: `agent.events#TurnFinished`, `agent.events`, `turns.limit_reached`, or `*`.
    pub fn on(&self, name: &str, f: impl Fn(Event) + Send + Sync + 'static) { self.reg(|r| put(&mut r.events, name, Arc::new(f))) }
    /// `action` request. `()`/`None` -> `{"ok":true}`; any other value is the result.
    pub fn action<R: IntoReply>(&self, name: &str, f: impl Fn(Req) -> R + Send + Sync + 'static) { self.handler(0, name, f) }
    /// OpenUI `Query(name, ...)`. The return value is the query's value (`()` -> null).
    pub fn query<R: IntoReply>(&self, name: &str, f: impl Fn(Req) -> R + Send + Sync + 'static) { self.handler(1, name, f) }
    /// `/plugin:name` command. A string -> `{"text": s}`, an object as-is, `()` -> `{}`.
    pub fn command<R: IntoReply>(&self, name: &str, f: impl Fn(Req) -> R + Send + Sync + 'static) { self.handler(2, name, f) }
    /// `tools/call` of a tool declared in plugin.yaml. A string -> text content; an object with `content` or
    /// `isError` as-is; any other value -> its JSON as text (+ `structuredContent` for objects).
    pub fn tool<R: IntoReply>(&self, name: &str, f: impl Fn(Req) -> R + Send + Sync + 'static) { self.handler(TOOL, name, f) }
    /// Raw hatch: answer a host request method the helpers do not wrap. `f(params)` -> `result`.
    pub fn on_request<R: IntoReply>(&self, method: &str, f: impl Fn(Value) -> R + Send + Sync + 'static) {
        self.reg(|r| put(&mut r.requests, method, Arc::new(move |p| f(p).into_reply())))
    }
    /// Raw hatch: handle a host notification method. Called with the params.
    pub fn on_notify(&self, method: &str, f: impl Fn(Value) + Send + Sync + 'static) { self.reg(|r| put(&mut r.notifies, method, Arc::new(f))) }
    /// After `hello`, before the loop. Paint here. Also re-run on `render` unless on_render is set.
    pub fn on_start(&self, f: impl Fn() + Send + Sync + 'static) { self.reg(|r| r.start.push(Arc::new(f))) }
    /// On `shutdown` or stdin EOF, before exit 0.
    pub fn on_shutdown(&self, f: impl Fn() + Send + Sync + 'static) { self.reg(|r| r.shutdown.push(Arc::new(f))) }
    /// Grants changed; `grants()` is already replaced. Called with the new grants.
    pub fn on_granted(&self, f: impl Fn(Value) + Send + Sync + 'static) { self.reg(|r| r.granted.push(Arc::new(f))) }
    /// Events were dropped (`{dropped, since_ms}`). Without hooks: a warn log.
    pub fn on_lagged(&self, f: impl Fn(Value) + Send + Sync + 'static) { self.reg(|r| r.lagged.push(Arc::new(f))) }
    /// `notifications/cancelled` (`{requestId, reason}`). The id is already in `cancelled()`.
    pub fn on_cancel(&self, f: impl Fn(Value) + Send + Sync + 'static) { self.reg(|r| r.cancel.push(Arc::new(f))) }
    /// `render` request (`placement`, `session`, `client`). Send a `ui`; the SDK replies `{}`.
    pub fn on_render(&self, f: impl Fn(Req) + Send + Sync + 'static) { self.reg(|r| r.render.push(Arc::new(f))) }

    // ---- session ----
    /// The `hello` params.
    pub fn hello(&self) -> Value { self.sess(|s| s.hello.clone()) }
    /// Current grants (`hello.grants`, then each `granted`).
    pub fn grants(&self) -> Value { self.sess(|s| s.grants.clone()) }
    pub fn data_dir(&self) -> Option<String> { self.sess(|s| s.data_dir.clone()) }
    pub fn clients(&self) -> Value { self.sess(|s| s.clients.clone()) }
    /// Ids of host requests the host cancelled. The SDK does not stop the handler; a late reply is discarded.
    pub fn cancelled(&self) -> Vec<Value> { self.sess(|s| s.cancelled.clone()) }
    pub fn is_cancelled(&self, id: &Value) -> bool { self.sess(|s| s.cancelled.contains(id)) }

    // ---- plugin -> host notifications ----
    /// Raw notification (`Value::Null` params -> `{}`).
    pub fn send(&self, method: &str, params: Value) { self.wire.send(method, params) }
    /// `placement`, or the single granted one. Panics (nothing sent) when the grant holds 0 or several.
    fn placement(&self, placement: Option<&str>) -> String {
        if let Some(p) = placement { return p.into() }
        let ids = self.sess(|s| get(&s.grants, "placements"));
        match ids.as_array().map(Vec::as_slice) {
            Some([Value::String(id)]) => id.clone(),
            _ => panic!("pass a placement: the grant holds placements {ids}"),
        }
    }
    /// Dot, badge and label in the default placement. More fields: `status_with`.
    pub fn status(&self, dot: &str, badge: impl Into<Value>, label: &str) {
        self.status_with(Status { dot: dot.into(), badge: Some(badge.into()), label: Some(label.into()), ..Status::default() })
    }
    pub fn status_with(&self, st: Status) {
        let mut p = json!({"placement": self.placement(st.placement.as_deref()), "dot": st.dot});
        for (k, v) in [("badge", st.badge), ("label", st.label.map(Value::from)), ("tooltip", st.tooltip.map(Value::from))] {
            if let Some(v) = v { p[k] = v }
        }
        self.send("status", p)
    }
    /// An OpenUI card of `text` + buttons `(label, action)` in the default placement. More: `ui_with`.
    pub fn ui(&self, text: &str, buttons: &[(&str, &str)]) {
        self.ui_with(Ui { text: text.into(), buttons: buttons.iter().map(|&b| b.into()).collect(), ..Ui::default() })
    }
    /// Replace a placement's content. `text` is the TUI/channel fallback and always sent.
    pub fn ui_with(&self, u: Ui) {
        let (format, source) = match (u.source, u.format) {
            (Some(src), f) => (f.unwrap_or_else(|| "openui".into()), src),
            (None, Some(f)) if f != "openui" => (f, u.text.clone()),
            (None, _) => ("openui".into(), openui_card(&u.text, &u.buttons)),
        };
        let placement = self.placement(u.placement.as_deref());
        self.send("ui", json!({"placement": placement, "format": format, "text": u.text, "source": source}))
    }
    /// Publish a declared event (`data` must be an object). The host drops an undeclared name (-32003 in the
    /// plugin log); the SDK cannot see that, `emit` is a notification.
    pub fn emit(&self, event: &str, data: Value, session: Option<&str>) {
        assert!(data.is_object(), "emit data must be a JSON object");
        let mut p = json!({"event": event, "data": data});
        if let Some(s) = session { p["session"] = s.into() }
        self.send("emit", p)
    }
    /// Inbox item, level "info" (needs capability `notify`; dropped by the host otherwise).
    pub fn notify(&self, title: &str, body: &str) { self.notify_with(title, body, "info", None) }
    pub fn notify_with(&self, title: &str, body: &str, level: &str, open: Option<Value>) {
        let mut p = json!({"title": title, "body": body, "level": level});
        if let Some(o) = open { p["open"] = o }
        self.send("notify", p)
    }
    /// Clients re-run the placement's OpenUI Queries, keeping `$state`.
    pub fn invalidate(&self, placement: Option<&str>) { self.send("invalidate", json!({"placement": self.placement(placement)})) }
    /// An info line in the host's plugin log.
    pub fn log(&self, message: &str) { self.wire.log(message, "info", Value::Null) }
    /// A log line with level (debug|info|warn|error) and `fields` (an object; null or `{}` omitted).
    pub fn log_with(&self, message: &str, level: &str, fields: Value) { self.wire.log(message, level, fields) }

    // ---- plugin -> host requests: block until the response (from handlers under run(), or in drive()) ----
    /// Raw request (`Value::Null` params -> `{}`). Returns `result`, or the error object as `RpcError`.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let params = obj(Some(&params));
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let responder = lock(&self.responder).clone();
        if let Some(r) = responder {
            self.wire.write(msg);
            return r(method, &params);
        }
        let (tx, rx) = mpsc::channel();
        {
            let mut p = lock(&self.pending);
            if p.1 { return Err(eof_error()) }
            p.0.insert(id, tx);
        }
        self.wire.write(msg);
        rx.recv().unwrap_or_else(|_| Err(eof_error()))
    }
    /// Host-brokered tool call. Returns the ToolOutput `{content, is_error, structured?}` as-is.
    pub fn call(&self, tool: &str, args: Value) -> Result<Value, RpcError> {
        self.request("call", json!({"tool": tool, "args": obj(Some(&args))}))
    }
    /// Send text into a session as the user (capability `input`). Returns `{session}`.
    pub fn input(&self, text: &str, session: Option<&str>, delivery: Option<&str>) -> Result<Value, RpcError> {
        let mut p = json!({"text": text, "session": session});
        if let Some(d) = delivery { p["delivery"] = d.into() }
        self.request("input", p)
    }
    pub fn catalog(&self, topic: Option<&str>) -> Result<Value, RpcError> {
        self.request("catalog", topic.map_or_else(|| json!({}), |t| json!({"topic": t})))
    }
    /// Returns `{now_ms}`.
    pub fn ping(&self) -> Result<Value, RpcError> { self.request("ping", json!({})) }

    // ---- dispatch ----
    /// Run a hook or notification handler; a panic becomes an error log.
    fn hook(&self, what: &str, f: impl FnOnce()) {
        if let Err(e) = guard(|| { f(); Ok(()) }) { self.log_with(&format!("{what} failed: {e}"), "error", Value::Null) }
    }
    fn hooks<T: Clone>(&self, what: &str, list: impl FnOnce(&Reg) -> Vec<T>, call: impl Fn(&T)) {
        for h in self.reg(|r| list(r)) { self.hook(what, || call(&h)) }
    }

    fn on_hello(&self, p: &Value) {
        if p.get("protocol").and_then(Value::as_i64) != Some(PROTOCOL) {
            let m = format!("unsupported protocol {}; this SDK speaks {PROTOCOL}", get(p, "protocol"));
            if lock(&self.wire.sink).is_some() { panic!("{m}") } // drive(): fail the test, not the test process
            self.log_with(&m, "error", Value::Null);
            process::exit(1);
        }
        let data_dir = s(p, "data_dir");
        self.sess(|s| {
            (s.hello, s.grants, s.clients) = (p.clone(), obj(p.get("grants")), obj(p.get("clients")));
            s.data_dir = data_dir.clone();
        });
        if env::var("THEUTH_PLUGIN_DATA").map_or(true, |v| v.is_empty()) && data_dir.is_some() {
            *lock(&self.state.inner) = (None, data_dir);
        }
        self.hooks("on_start", |r| r.start.clone(), |h| h());
    }

    fn cancel_id(&self, msg: &Value) {
        let id = msg.pointer("/params/requestId").cloned().unwrap_or(Value::Null);
        self.sess(|s| if !s.cancelled.contains(&id) { s.cancelled.push(id) });
    }

    /// Handle one host message on the handler thread. Returns false on shutdown.
    fn dispatch(&self, msg: &Value) -> bool {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let p = obj(msg.get("params"));
        if let Some(id) = msg.get("id") {
            let body = match self.answer(method, &p, id, msg) {
                Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                Err(Fail::Rpc(e)) => json!({"jsonrpc": "2.0", "id": id, "error": e.to_json()}),
                Err(Fail::Error(m)) => {
                    self.log_with(&format!("{method} failed: {m}"), "error", Value::Null);
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": m}})
                }
            };
            self.wire.write(body);
            return true;
        }
        match method {
            "shutdown" => return false,
            "event" => {
                let ev = Event {
                    topic: s(&p, "topic").unwrap_or_default(), version: get(&p, "version"),
                    r#type: s(&p, "type").unwrap_or_default(), session: s(&p, "session"),
                    at_ms: get(&p, "at_ms"), data: get(&p, "data"), raw: msg.clone(),
                };
                let keys = [format!("{}#{}", ev.topic, ev.r#type), ev.topic.clone(), ev.r#type.clone(), "*".into()];
                if let Some(h) = self.reg(|r| keys.iter().find_map(|k| find(&r.events, k))) {
                    self.hook(method, || h(ev));
                }
            }
            "granted" => {
                let g = obj(p.get("grants"));
                self.sess(|s| s.grants = g.clone());
                self.hooks(method, |r| r.granted.clone(), |h| h(g.clone()));
            }
            "lagged" => {
                if self.reg(|r| r.lagged.is_empty()) {
                    self.log_with(&format!("lagged: {} events dropped", get(&p, "dropped")), "warn", p.clone());
                }
                self.hooks(method, |r| r.lagged.clone(), |h| h(p.clone()));
            }
            "notifications/cancelled" => self.hooks(method, |r| r.cancel.clone(), |h| h(p.clone())),
            m => {
                if let Some(h) = self.reg(|r| find(&r.notifies, m)) { self.hook(m, || h(p)) }
            }
        }
        true
    }

    fn answer(&self, method: &str, p: &Value, id: &Value, msg: &Value) -> Result<Value, Fail> {
        let session = match p.get("session") {
            Some(v) => v.as_str().map(String::from),
            None => p.pointer("/_meta/theuth/session").and_then(Value::as_str).map(String::from),
        };
        let mut req = Req {
            placement: s(p, "placement"), name: String::new(), args: get(p, "args"), session, client: s(p, "client"),
            form: get(p, "form"), activation: s(p, "activation"), id: id.clone(), raw: msg.clone(),
        };
        let Some(kind) = KINDS.iter().position(|(m, _)| *m == method) else { return self.answer_other(method, p, req) };
        let name = s(p, KINDS[kind].1).unwrap_or_default();
        req.name = name.clone();
        if kind == TOOL { req.args = obj(p.get("arguments")) }
        let h = self.reg(|r| find(&r.kinds[kind], &name));
        let h = h.ok_or_else(|| Fail::Rpc(RpcError::new(-32601, format!("{method} not handled: {name}"))))?;
        let out = guard(|| h(req));
        match (kind, out) {
            (TOOL, Err(Fail::Error(m))) => {
                self.log_with(&format!("tool {name} failed: {m}"), "error", Value::Null);
                Ok(json!({"content": [{"type": "text", "text": m}], "isError": true}))
            }
            (_, Err(e)) => Err(e),
            (TOOL, Ok(r)) => Ok(tool_result(r)),
            (0, Ok(Reply::Nil)) => Ok(json!({"ok": true})),
            (2, Ok(Reply::Nil)) => Ok(json!({})),
            (2, Ok(Reply::Text(t))) => Ok(json!({"text": t})),
            (_, Ok(r)) => Ok(plain(r)),
        }
    }

    fn answer_other(&self, method: &str, p: &Value, req: Req) -> Result<Value, Fail> {
        if let Some(h) = self.reg(|r| find(&r.requests, method)) { return guard(|| h(p.clone())).map(plain) }
        let (render, start) = self.reg(|r| (r.render.clone(), r.start.clone()));
        if method != "render" || (render.is_empty() && start.is_empty()) {
            return Err(Fail::Rpc(RpcError::new(-32601, format!("method not found: {method}"))));
        }
        for h in &render { guard(|| { h(req.clone()); Ok(()) })? }
        if render.is_empty() {
            for h in &start { guard(|| { h(); Ok(()) })? }
        }
        Ok(json!({}))
    }

    /// What the code registered, as the `--theuth-manifest` YAML fragment.
    pub fn manifest(&self) -> String {
        fn names<T>(v: &[(String, T)]) -> String { v.iter().map(|(n, _)| js(n)).collect::<Vec<_>>().join(", ") }
        self.reg(|r| {
            let k = &r.kinds;
            format!("# registered in code (plugin.yaml is the contract)\ntools: [{}]\nactions: [{}]\nqueries: [{}]\n\
                     commands: [{}]\non: [{}]\n", names(&k[TOOL]), names(&k[0]), names(&k[1]), names(&k[2]), names(&r.events))
        })
    }

    // ---- run loop ----
    /// Serve the host until `shutdown` or stdin EOF, then run on_shutdown hooks and exit 0.
    /// With `--theuth-manifest` in argv: print `manifest()` and exit 0 without reading stdin.
    pub fn run(&'static self) -> ! {
        if env::args().skip(1).any(|a| a == "--theuth-manifest") {
            print!("{}", self.manifest());
            let _ = io::stdout().flush();
            process::exit(0);
        }
        let (tx, rx) = mpsc::channel();
        thread::Builder::new().name("theuth-reader".into()).spawn(move || self.read(tx)).expect("spawn reader");
        let mut started = false;
        for msg in rx {
            if started {
                if !self.dispatch(&msg) { break }
            } else if msg.get("method").and_then(Value::as_str) == Some("hello") {
                self.on_hello(&obj(msg.get("params")));
                started = true;
            } else if let Some(id) = msg.get("id") {
                self.wire.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "hello first"}}));
            }
        }
        self.hooks("on_shutdown", |r| r.shutdown.clone(), |h| h());
        let _ = io::stdout().flush();
        process::exit(0) // the reader thread may sit in a blocking read
    }

    /// Reader thread: route responses to pending requests by id, queue everything else.
    fn read(&self, tx: mpsc::Sender<Value>) {
        let (mut input, mut buf) = (io::stdin().lock(), Vec::new());
        while matches!(input.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
            let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
            buf.clear();
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if !msg.is_object() { continue }
            if msg.get("method").is_none() && msg.get("id").is_some() {
                let waiter = msg["id"].as_i64().and_then(|id| lock(&self.pending).0.remove(&id));
                if let Some(w) = waiter {
                    let _ = w.send(msg.get("error").map_or_else(|| Ok(get(&msg, "result")), |e| Err(RpcError::from_json(e))));
                }
                continue;
            }
            if msg.get("method").and_then(Value::as_str) == Some("notifications/cancelled") { self.cancel_id(&msg) }
            if tx.send(msg).is_err() { break }
        }
        let mut p = lock(&self.pending);
        p.1 = true;
        for (_, w) in p.0.drain() { let _ = w.send(Err(eof_error())); }
    }

    // ---- testing ----
    /// Run host messages (objects, or JSON text as `Value::String`) through the dispatcher on this thread,
    /// without stdin. Returns what the plugin sent. `responder` answers the plugin's requests (`None`: every
    /// request fails with -32601). hello -> on_start; shutdown -> on_shutdown.
    pub fn drive<I: IntoIterator<Item = Value>>(&self, lines: I, responder: Option<Responder>) -> Vec<Value> {
        *lock(&self.wire.sink) = Some(Vec::new());
        *lock(&self.responder) = Some(responder.unwrap_or_else(|| Arc::new(no_responder)));
        for line in lines {
            let msg = match line { Value::String(t) => serde_json::from_str(&t).unwrap_or(Value::Null), v => v };
            match msg.get("method").and_then(Value::as_str) {
                _ if !msg.is_object() => continue,
                Some("hello") => self.on_hello(&obj(msg.get("params"))),
                Some("notifications/cancelled") => {
                    self.cancel_id(&msg);
                    self.dispatch(&msg);
                }
                _ if !self.dispatch(&msg) => {
                    self.hooks("on_shutdown", |r| r.shutdown.clone(), |h| h());
                    break;
                }
                _ => {}
            }
        }
        *lock(&self.responder) = None;
        lock(&self.wire.sink).take().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_round_trip() {
        let dir = env::temp_dir().join(format!("theuth-rs-sdk-test-{}", process::id()));
        env::remove_var("THEUTH_PLUGIN_DATA");
        let p = plugin();
        p.action("inc", |_| { plugin().state.add("n", 1); });
        p.action("ping", |_| plugin().ping());
        p.action("boom", |_| -> String { panic!("boom") });
        p.tool("presses", |_| plugin().state.int("n"));
        p.on_start(|| plugin().ui("hi <&> ü", &[("Press", "inc")]));
        let responder: Responder = Arc::new(|m: &str, _: &Value| if m == "ping" { Ok(json!({"now_ms": 7})) } else { Err(RpcError::new(-1, "no")) });
        let req = |id: i64, method: &str, params: Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let out = p.drive([
            json!({"jsonrpc": "2.0", "method": "hello", "params": {"protocol": 1, "data_dir": dir.to_str(), "grants": {"placements": ["presses"]}}}),
            req(1, "action", json!({"action": "inc"})),
            req(2, "tools/call", json!({"name": "presses"})),
            req(3, "action", json!({"action": "ping"})),
            Value::from(r#"{"jsonrpc":"2.0","id":4,"method":"query","params":{"query":"x"}}"#),
            req(5, "action", json!({"action": "boom"})),
            json!({"jsonrpc": "2.0", "method": "shutdown", "params": {}}),
        ], Some(responder));
        let _ = fs::remove_dir_all(&dir);
        let src = "root = Card([body, controls])\nbody = TextContent(\"hi <&> ü\")\na0 = Mutation(\"inc\", {})\n\
                   controls = Buttons([Button(\"Press\", @Run(a0), \"secondary\")])";
        assert_eq!(out[0]["params"]["source"], src);
        assert!(out[0].to_string().contains("<&> ü"));
        assert_eq!(out[1], json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}));
        assert_eq!(out[2]["result"], json!({"content": [{"type": "text", "text": "1"}], "isError": false}));
        assert_eq!(out[3], req(1, "ping", json!({})));
        assert_eq!(out[4]["result"], json!({"now_ms": 7}));
        assert_eq!(out[5]["error"]["code"], -32601);
        assert_eq!((&out[6]["params"]["level"], &out[7]["error"]), (&json!("error"), &json!({"code": -32603, "message": "boom"})));
        assert_eq!(p.manifest().lines().nth(1), Some(r#"tools: ["presses"]"#));
    }
}
