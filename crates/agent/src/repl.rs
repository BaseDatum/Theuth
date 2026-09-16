//! Persistent Python runtime over monty-pool: bridge, inventory, persistence (design doc 08).
//!
//! One [`Repl`] per live turn owns a `Checkout`; every registered tool is an
//! `async` function inside the sandbox, routed through the registry's
//! `Invoke` (policy, approvals) exactly like a direct call. The heap is
//! dumped after every cell to `<data_dir>/runtime/<session>.monty` and
//! restored on the next open.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};

use futures::future::join_all;
use kameo::actor::ActorRef;
use monty_pool::{
    Checkout, MountSpec, MountSpecMode, OnPrint, Pool, PoolConfig, PoolError, ReplConfig,
    ResumeValue, TurnEvent, on_print_sync,
};
use monty_types::{
    ExcType, MontyDate, MontyDateTime, MontyException, MontyObject, NameLookupResult,
    ResourceLimits,
};
use regex::Regex;
use runtime::{SessionId, TurnId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::{Invoke, ListTools, Risk, ToolOutput, ToolRegistry, ToolSpec};

use crate::AgentEvent;

/// Tool name.
pub const NAME: &str = "python";
/// Cap on stdout and on the result repr in a cell result (design doc 08 §3.3).
pub const RUN_OUTPUT_CHARS: usize = 8_000;
/// Sandbox heap limit (08 §4.5).
pub const MAX_MEMORY: usize = 512 << 20;
/// Pool-side hard deadline per protocol turn (08 §4.5).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Host calls the pool services per checkout (08 §4.5).
const MAX_SUSPENSIONS: usize = 100_000;
/// Head / tail kept when output exceeds [`RUN_OUTPUT_CHARS`].
const HEAD_CHARS: usize = 6_000;
const TAIL_CHARS: usize = 2_000;
/// Characters of an inner tool output shown in [`AgentEvent::ToolCallFinished`].
const DISPLAY_CHARS: usize = 2_000;
/// Summary length in a signature line.
const SUMMARY_CHARS: usize = 120;

const DESCRIPTION: &str = "Run Python in your persistent runtime - your working memory. Globals, functions and \
imports persist across calls and across sessions. Tools are async functions named as listed under \
\"Runtime\" in the system prompt; bind every result to a name (`src = await read_file(path=\"x.rs\")`) and \
work on it in Python; run several at once with `await asyncio.gather(a(), b())`. The value of the last \
expression is returned; stdout is returned too, capped - print only what you need to decide the next \
step, never a whole file or result. `tools(query)` lists tools, `help(name)` shows one tool's full \
parameters and description. `/cwd` is the working directory (read-only); write files with `write_file`.";

const RUNTIME_BLOCK: &str = "## Runtime
`python` runs a sandboxed Python 3.14 subset (Monty): no third-party packages, no
generators, no class inheritance; stdlib: json re math datetime collections itertools
functools dataclasses pathlib base64 asyncio(gather). State persists, except names
starting with `_`, which do not carry across cells. A tool error inside a coroutine
passed to `gather` cannot be caught inside that coroutine (monty bug): catch around
the `gather` itself, or await tools one at a time. `shell()` returns the output when
the command finishes within a few seconds, otherwise `{\"job\": id, \"log\": path}`; the
result arrives later as <job_result>.

The runtime is your working memory; the conversation is for decisions. Every tool call
is an `await` expression - bind its result to a name (`src = await read_file(path=\"x.rs\")`)
and work on it in Python: slice, filter, regex, count, diff. Print only what you need to
decide the next step, never a whole file or a whole result; the full value stays in the
variable and survives compaction and restart. Use Python for loops, conditions and
parsing; use `shell()` to run programs, not to write shell programs. Discover a tool's
full parameters with `help(name)` - the index below is names and signatures only. The
tools defined directly for you are awaitable here too, by the same name.

Tools available only in the runtime (all `async`):
";

/// Arguments of the `python` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PythonArgs {
    /// Python source to run in the persistent runtime.
    pub code: String,
}

/// Spec of the loop-local `python` tool.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: DESCRIPTION.into(),
        schema: serde_json::to_value(schemars::schema_for!(PythonArgs)).unwrap_or_default(),
        risk: Risk::ReadOnly,
        direct: true,
        returns: "str".to_owned(),
    }
}

/// Everything a cell needs from the turn.
#[derive(Clone)]
pub struct RunCx {
    /// Owning session.
    pub session: SessionId,
    /// Turn the cells run in.
    pub turn: TurnId,
    /// Event bus.
    pub rt: runtime::Runtime,
    /// Tool registry (`ListTools`, `Invoke`).
    pub registry: ActorRef<ToolRegistry>,
    /// `<data_dir>/spill`; `<spill_dir>/<session>` is mounted at `/spill`.
    pub spill_dir: PathBuf,
    /// Mounted read-only at `/cwd`.
    pub cwd: PathBuf,
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// Start the worker pool on `worker`, spawned as `<worker> subprocess`. In
/// production that is this binary (`wth subprocess`, 08 §9), so nothing has
/// to be installed. `None` (with one `warn!`) when the pool fails to start:
/// the runtime is off for the process.
pub async fn open_pool(worker: &Path, max_processes: usize) -> Option<Arc<Pool>> {
    let mut pool = PoolConfig::subprocess(worker);
    pool.min_processes = 1;
    pool.max_processes = max_processes.max(1);
    pool.request_timeout = Some(REQUEST_TIMEOUT);
    pool.max_checkouts_per_worker = Some(100);
    match Pool::new(pool).await {
        Ok(p) => {
            tracing::info!("python runtime on: {} subprocess", worker.display());
            Some(Arc::new(p))
        }
        Err(e) => {
            tracing::warn!("worker pool failed to start ({e}); python runtime disabled");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Signatures, stubs, help, prompt block
// ---------------------------------------------------------------------------

/// Registered name → Python identifier (`a-b.c` → `a_b_c`).
pub fn sanitize_ident(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// One parameter of a tool schema.
struct Param<'a> {
    name: &'a str,
    schema: &'a Value,
    required: bool,
}

/// Parameters in signature order: required in schema order, then the rest sorted by name.
fn params(schema: &Value) -> Vec<Param<'_>> {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut req: Vec<Param<'_>> = Vec::with_capacity(props.len());
    let mut opt: Vec<Param<'_>> = Vec::new();
    for (name, schema) in props {
        let required = required.contains(&name.as_str());
        let p = Param {
            name,
            schema,
            required,
        };
        if required { req.push(p) } else { opt.push(p) }
    }
    opt.sort_by(|a, b| a.name.cmp(b.name));
    req.extend(opt);
    req
}

/// JSON Schema → Python type (codemcp `py_type`).
fn py_type(schema: &Value) -> String {
    if let Some(vals) = schema.get("enum").and_then(Value::as_array) {
        let lits: Vec<String> = vals.iter().map(py_literal).collect();
        return format!("Literal[{}]", lits.join(", "));
    }
    if let Some(alts) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut seen = Vec::new();
        for t in alts.iter().map(py_type) {
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        return seen.join(" | ");
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        // `type: [T, "null"]` (schemars' nullable form)
        Some(Value::Array(ts)) => {
            let mut parts: Vec<String> = ts
                .iter()
                .filter_map(Value::as_str)
                .map(|t| py_type(&serde_json::json!({ "type": t })))
                .collect();
            parts.dedup();
            return parts.join(" | ");
        }
        _ => return "Any".into(),
    };
    match ty {
        "string" => "str".into(),
        "integer" => "int".into(),
        "number" => "float".into(),
        "boolean" => "bool".into(),
        "null" => "None".into(),
        "array" => format!(
            "list[{}]",
            schema.get("items").map_or_else(|| "Any".into(), py_type)
        ),
        "object" => "dict".into(),
        _ => "Any".into(),
    }
}

fn py_literal(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// First sentence of the description, cut at [`SUMMARY_CHARS`].
fn summary(description: &str) -> String {
    let d = description.trim();
    let end = d
        .find(". ")
        .map(|i| i + 1)
        .into_iter()
        .chain(d.find('\n'))
        .min()
        .unwrap_or(d.len());
    let s = d[..end].trim_end();
    match s.char_indices().nth(SUMMARY_CHARS) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

fn render_params(spec: &ToolSpec, extra: bool) -> String {
    let mut parts: Vec<String> = params(&spec.schema)
        .iter()
        .map(|p| {
            let t = py_type(p.schema);
            if p.required {
                format!("{}: {t}", p.name)
            } else if t == "None" || t.ends_with("| None") {
                format!("{}: {t} = None", p.name)
            } else {
                format!("{}: {t} | None = None", p.name)
            }
        })
        .collect();
    if extra {
        parts.push("**extra: Any".into());
    }
    parts.join(", ")
}

/// `async def name(a: str, b: int | None = None) -> str  # summary` (08 §3.2).
pub fn signature(spec: &ToolSpec) -> String {
    let summary = summary(&spec.description);
    let mut s = format!(
        "async def {}({}) -> {}",
        sanitize_ident(&spec.name),
        render_params(spec, false),
        spec.returns
    );
    if !summary.is_empty() {
        s.push_str("  # ");
        s.push_str(&summary);
    }
    s
}

/// The `.pyi` for `ReplConfig.type_check_stubs` (08 §4.6).
pub fn stubs(specs: &[ToolSpec]) -> String {
    let mut out = String::from("from typing import Any, Literal\n");
    for spec in specs {
        out.push_str(&format!(
            "async def {}({}) -> {}: ...\n",
            sanitize_ident(&spec.name),
            render_params(spec, true),
            spec.returns
        ));
    }
    out.push_str("def tools(query: str | None = None) -> list[str]: ...\n");
    out.push_str("def help(name: str) -> str: ...\n");
    // monty implements `open()` but its trimmed typeshed omits it.
    out.push_str("def open(file: str, mode: str = 'r', encoding: str | None = None) -> Any: ...\n");
    out
}

/// The system prompt `## Runtime` block (08 §3.2). Lists exactly the tools that
/// are *not* in the direct array, so every tool is defined once: with
/// `runtime_only` the non-`direct` ones; without it (every tool direct) none.
/// Direct tools stay awaitable in a cell either way - the model knows them
/// from their definitions.
pub fn prompt_block(specs: &[ToolSpec], runtime_only: bool) -> String {
    let mut out = String::from(RUNTIME_BLOCK);
    for spec in specs.iter().filter(|s| runtime_only && !s.direct) {
        out.push_str(&signature(spec));
        out.push('\n');
    }
    out
}

/// `help(name)` text (08 §4.2).
fn help(spec: &ToolSpec) -> String {
    let mut out = format!(
        "async def {}({}) -> {}\n\n{}\n",
        sanitize_ident(&spec.name),
        render_params(spec, false),
        spec.returns,
        spec.description.trim()
    );
    let ps = params(&spec.schema);
    if ps.is_empty() {
        return out;
    }
    let rows: Vec<(&str, String, &str, &str)> = ps
        .iter()
        .map(|p| {
            let mut ty = py_type(p.schema);
            if let Some(inner) = p.schema.get("properties").and_then(Value::as_object) {
                let fields: Vec<String> = inner
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", py_type(v)))
                    .collect();
                ty = format!("dict {{{}}}", fields.join(", "));
            }
            let desc = p
                .schema
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            (
                p.name,
                ty,
                if p.required { "required" } else { "optional" },
                desc,
            )
        })
        .collect();
    let w_name = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let w_ty = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
    out.push('\n');
    for (name, ty, req, desc) in rows {
        out.push_str(&format!(
            "  {name:<w_name$}  {ty:<w_ty$}  {req:<8}   {desc}\n"
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Values across the boundary
// ---------------------------------------------------------------------------

/// JSON → sandbox value.
pub fn to_monty(v: &Value) -> MontyObject {
    match v {
        Value::Null => MontyObject::None,
        Value::Bool(b) => MontyObject::Bool(*b),
        Value::Number(n) => n.as_i64().map_or_else(
            || MontyObject::Float(n.as_f64().unwrap_or(f64::NAN)),
            MontyObject::Int,
        ),
        Value::String(s) => MontyObject::String(s.clone()),
        Value::Array(a) => MontyObject::List(a.iter().map(to_monty).collect()),
        Value::Object(o) => MontyObject::dict(
            o.iter()
                .map(|(k, v)| (MontyObject::String(k.clone()), to_monty(v)))
                .collect::<Vec<_>>(),
        ),
    }
}

/// Sandbox value → JSON; dict keys are stringified, exotic values become their repr.
pub fn from_monty(o: &MontyObject) -> Value {
    match o {
        MontyObject::None => Value::Null,
        MontyObject::Bool(b) => Value::Bool(*b),
        MontyObject::Int(i) => Value::from(*i),
        MontyObject::Float(f) => {
            serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number)
        }
        MontyObject::String(s) => Value::String(s.clone()),
        MontyObject::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        MontyObject::List(items)
        | MontyObject::Tuple(items)
        | MontyObject::Set(items)
        | MontyObject::FrozenSet(items)
        | MontyObject::NamedTuple { values: items, .. } => {
            Value::Array(items.iter().map(from_monty).collect())
        }
        MontyObject::Dict(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        MontyObject::String(s) => s.clone(),
                        other => other.py_repr(),
                    };
                    (key, from_monty(v))
                })
                .collect(),
        ),
        MontyObject::Date(d) => Value::String(format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)),
        MontyObject::DateTime(d) => Value::String(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            d.year, d.month, d.day, d.hour, d.minute, d.second
        )),
        other => Value::String(other.py_repr()),
    }
}

// ---------------------------------------------------------------------------
// Inventory (08 §6)
// ---------------------------------------------------------------------------

static BIND_RES: LazyLock<[Regex; 5]> = LazyLock::new(|| {
    [
        Regex::new(r"(?m)^([A-Za-z_]\w*)\s*(?::[^=\n]+)?=[^=]").expect("assign re"),
        Regex::new(r"(?m)^(?:async\s+)?def\s+(\w+)").expect("def re"),
        Regex::new(r"(?m)^class\s+(\w+)").expect("class re"),
        Regex::new(r"(?m)^import\s+(\w+)(?:\S*\s+as\s+(\w+))?").expect("import re"),
        Regex::new(r"(?m)^from\s+\S+\s+import\s+(.+)").expect("from re"),
    ]
});

/// Top-level names a cell may bind (regex over column-0 lines).
// ponytail: regex, not a parser; tuple targets and augmented assignments are
// caught by the model printing. Swap in a parser if a transcript shows a gap.
pub fn candidates(code: &str) -> BTreeSet<String> {
    let [assign, def, class, import, from] = &*BIND_RES;
    let mut out = BTreeSet::new();
    for re in [assign, def, class] {
        out.extend(re.captures_iter(code).map(|c| c[1].to_owned()));
    }
    for c in import.captures_iter(code) {
        out.insert(
            c.get(2)
                .unwrap_or_else(|| c.get(1).expect("group 1"))
                .as_str()
                .to_owned(),
        );
    }
    for c in from.captures_iter(code) {
        for item in c[1].split(',') {
            let item = item.trim().trim_matches(['(', ')']).trim();
            let name = item.rsplit(" as ").next().unwrap_or(item).trim();
            if !name.is_empty() && name != "*" {
                out.insert(name.to_owned());
            }
        }
    }
    out.retain(|n| !n.starts_with('_') && n != "tools" && n != "help");
    out
}

/// One tracked global.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Slot {
    kind: String,
    len: Option<usize>,
}

/// `.names.json` row.
#[derive(Serialize, Deserialize)]
struct Entry {
    name: String,
    kind: String,
    len: Option<usize>,
}

fn render_slots<'a>(it: impl Iterator<Item = (&'a String, &'a Slot)>) -> Option<String> {
    let parts: Vec<String> = it
        .map(|(n, s)| match s.len {
            Some(l) => format!("{n} ({}, {l})", s.kind),
            None => format!("{n} ({})", s.kind),
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The tracked inventory from `.names.json` (empty when absent or unreadable).
fn read_names(path: &Path) -> BTreeMap<String, Slot> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Entry>>(&b).ok())
        .map(|rows| {
            rows.into_iter()
                .map(|e| {
                    (
                        e.name,
                        Slot {
                            kind: e.kind,
                            len: e.len,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The last persisted inventory of `session`, rendered like
/// [`Repl::render_inventory`]: for the compaction head when no [`Repl`] is
/// open in the turn (`/compact` between turns). `None` when nothing is tracked.
pub fn saved_inventory(data_dir: &Path, session: SessionId) -> Option<String> {
    let names = read_names(
        &data_dir
            .join("runtime")
            .join(format!("{session}.names.json")),
    );
    render_slots(names.iter())
}

fn probe_code(names: &BTreeSet<String>) -> String {
    let mut code = String::from("__inv = []\n");
    for n in names {
        code.push_str(&format!(
            "try: __inv.append(({n:?}, type({n}).__name__, len({n}) if isinstance({n}, (str, bytes, list, tuple, dict, set)) else None))\nexcept NameError: pass\n"
        ));
    }
    code.push_str("__inv");
    code
}

// ---------------------------------------------------------------------------
// Output capping (08 §3.3)
// ---------------------------------------------------------------------------

/// Head + tail with `marker(omitted_chars)` between when `s` exceeds
/// [`RUN_OUTPUT_CHARS`].
fn cap(s: &str, marker: impl FnOnce(usize) -> String) -> String {
    let total = s.chars().count();
    if total <= RUN_OUTPUT_CHARS {
        return s.to_owned();
    }
    let head_end = s.char_indices().nth(HEAD_CHARS).map_or(s.len(), |(i, _)| i);
    let tail_start = s
        .char_indices()
        .nth(total - TAIL_CHARS)
        .map_or(s.len(), |(i, _)| i);
    format!(
        "{}\n{}\n{}",
        &s[..head_end],
        marker(total - HEAD_CHARS - TAIL_CHARS),
        &s[tail_start..]
    )
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// A pending inner tool call: sandbox future id → what to invoke.
struct PendingCall {
    id: String,
    name: String,
    args: Value,
}

/// One session's runtime for one turn (08 §4).
pub struct Repl {
    pool: Arc<Pool>,
    cfg: ReplConfig,
    cx: RunCx,
    /// `None` between a crash and the next cell (re-checked-out lazily).
    checkout: Option<Checkout>,
    mounts: Vec<MountSpec>,
    dump_path: PathBuf,
    names_path: PathBuf,
    /// Last good dump: restored into a fresh checkout after a crash.
    last_dump: Option<Vec<u8>>,
    names: BTreeMap<String, Slot>,
    candidates: BTreeSet<String>,
    out_seq: u32,
    /// `<runtime>` line prepended to the next cell result.
    lost: Option<String>,
}

impl Repl {
    /// Check out a worker and restore the session's dump if there is one.
    /// Never fails: a failed restore (or checkout) leaves a fresh/empty
    /// runtime and the first cell result says so.
    pub async fn open(pool: &Arc<Pool>, session: SessionId, data_dir: &Path, cx: RunCx) -> Repl {
        let specs = cx
            .registry
            .ask(ListTools(Some(session)))
            .await
            .unwrap_or_default();
        let cfg = ReplConfig {
            limits: Some(ResourceLimits {
                max_memory: Some(MAX_MEMORY),
                max_suspensions: MAX_SUSPENSIONS,
                ..ResourceLimits::default()
            }),
            type_check: true,
            type_check_stubs: Some(stubs(&specs)),
            ..ReplConfig::default()
        };
        let dir = data_dir.join("runtime");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("runtime dir {}: {e}", dir.display());
        }
        let dump_path = dir.join(format!("{session}.monty"));
        let names_path = dir.join(format!("{session}.names.json"));
        let names = read_names(&names_path);
        let candidates = names.keys().cloned().collect();
        let mut mounts = Vec::with_capacity(2);
        let spill = cx.spill_dir.join(session.to_string());
        let _ = std::fs::create_dir_all(&spill);
        for (virt, host) in [("/cwd", &cx.cwd), ("/spill", &spill)] {
            match MountSpec::new(virt, host, MountSpecMode::ReadOnly) {
                Ok(m) => mounts.push(m),
                Err(e) => tracing::warn!("mount {virt} → {}: {e}", host.display()),
            }
        }
        let mut repl = Repl {
            pool: Arc::clone(pool),
            cfg,
            cx,
            checkout: None,
            mounts,
            last_dump: std::fs::read(&dump_path).ok(),
            dump_path,
            names_path,
            names,
            candidates,
            out_seq: 0,
            lost: None,
        };
        if let Err(e) = repl.ensure_checkout().await {
            tracing::warn!("python runtime checkout failed: {e}");
        }
        repl
    }

    /// Take a worker if we have none; restore the last dump into it. A dump
    /// that will not restore is renamed `.stale` and reported via `lost`.
    async fn ensure_checkout(&mut self) -> Result<(), PoolError> {
        if self.checkout.is_some() {
            return Ok(());
        }
        let mut c = self.pool.checkout(&self.cfg).await?;
        if let Some(bytes) = &self.last_dump {
            let mut quiet = on_print_sync(|_, _| {});
            if let Err(e) = c.restore(bytes.clone(), Vec::new(), &mut quiet).await {
                tracing::warn!(
                    "runtime restore failed for {}: {e}",
                    self.dump_path.display()
                );
                let _ = std::fs::rename(&self.dump_path, self.dump_path.with_extension("stale"));
                let held = render_slots(self.names.iter())
                    .map_or_else(String::new, |s| format!(" It held: {s}."));
                self.lost = Some(format!(
                    "<runtime> saved state could not be restored ({e}); the runtime is empty.{held}"
                ));
                self.last_dump = None;
                self.names.clear();
                self.candidates.clear();
                drop(c);
                c = self.pool.checkout(&self.cfg).await?;
            }
        }
        self.checkout = Some(c);
        Ok(())
    }

    /// Stubs follow the live tool set (a server that connected since the
    /// last cell is callable now, not next turn). The checker's config is
    /// fixed per checkout, so a change means a fresh checkout restored from
    /// the last dump — milliseconds, and the heap is byte-identical.
    async fn refresh_stubs(&mut self) {
        let specs = self
            .cx
            .registry
            .ask(ListTools(Some(self.cx.session)))
            .await
            .unwrap_or_default();
        let fresh = stubs(&specs);
        if self.cfg.type_check_stubs.as_deref() == Some(fresh.as_str()) {
            return;
        }
        self.cfg.type_check_stubs = Some(fresh);
        if let Some(c) = self.checkout.take() {
            tracing::info!(session = %self.cx.session, "runtime: tool set changed, refreshing stubs");
            if let Err(e) = c.finish().await {
                tracing::debug!("runtime checkout finish: {e}");
            }
        }
    }

    /// Full inventory line for the compaction head; `None` when nothing is tracked.
    pub fn render_inventory(&self) -> Option<String> {
        render_slots(self.names.iter())
    }

    /// Run one cell (08 §4.1, §3.3, §6, §5). `call_id` is the `python` tool
    /// call's id; inner calls are `"{call_id}.{n}"`.
    pub async fn run(&mut self, call_id: &str, code: &str) -> ToolOutput {
        self.refresh_stubs().await;
        if let Err(e) = self.ensure_checkout().await {
            return self.finish_output(format!("RuntimeError: runtime unavailable ({e})"), true);
        }
        let cell_names = candidates(code);
        self.candidates.extend(cell_names.iter().cloned());

        let mut stdout = String::new();
        let mut on_print = on_print_sync(|_, t| stdout.push_str(t));
        let outcome = self
            .drive(code, Vec::new(), false, call_id, &mut on_print)
            .await;
        drop(on_print);

        let (mut body, is_error) = match outcome {
            Ok(v) => (render_value(&stdout, &v), false),
            Err(PoolError::Runtime(exc)) if exc.exc_type() != ExcType::MemoryError => {
                let text = exc.to_string();
                // pydantic/monty: a host-future error inside a gathered coroutine
                // bypasses its try/except; two of them stall the scheduler. The
                // internal message is useless to the model; name the workaround.
                let text = if text.contains("asyncio scheduler stalled") {
                    format!(
                        "{text}\n(monty bug: a tool raised inside a coroutine passed to asyncio.gather; \
                         exceptions cannot be caught there. Catch around the gather itself, or await the tools one at a time.)"
                    )
                } else {
                    text
                };
                (join_out(&stdout, &text), true)
            }
            Err(PoolError::Typing(diag)) => {
                return self.finish_output(format!("type check failed:\n{diag}"), true);
            }
            Err(e) => {
                // Worker gone (crash, memory limit, timeout, protocol): the heap
                // is untrusted; restore the previous dump into a fresh worker.
                self.checkout = None;
                let restored = match self.ensure_checkout().await {
                    Ok(()) if self.lost.is_none() => "state restored to after the previous cell",
                    _ => "state could not be restored",
                };
                return self.finish_output(
                    format!("RuntimeError: runtime crashed ({e}); {restored} — this cell's effects are lost"),
                    true,
                );
            }
        };

        // Bind oversize stdout as `outN` and probe the candidate names. The
        // checker only learns names from type-checked source (never from
        // `inputs`, never `_`-prefixed), so a checked placeholder declares the
        // name and the probe's input then overwrites its value.
        let mut cell_names = cell_names;
        let mut inputs = Vec::new();
        if stdout.chars().count() > RUN_OUTPUT_CHARS {
            self.out_seq += 1;
            let name = format!("out{}", self.out_seq);
            body = render_capped(&stdout, &name, &body);
            let mut quiet = on_print_sync(|_, _| {});
            if let Err(e) = self
                .drive(&format!("{name} = ''"), Vec::new(), false, "", &mut quiet)
                .await
            {
                tracing::warn!("runtime: declaring {name}: {e}");
            }
            self.candidates.insert(name.clone());
            cell_names.insert(name.clone());
            inputs.push((name, MontyObject::String(stdout)));
        }
        if !self.candidates.is_empty() || !inputs.is_empty() {
            self.probe(inputs).await;
        }
        if let Some(bound) =
            render_slots(self.names.iter().filter(|(n, _)| cell_names.contains(*n)))
        {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("bindings (reuse, do not re-read): ");
            body.push_str(&bound);
            // The rest of the live inventory by name, so nothing is lost track of.
            let rest: Vec<&str> = self
                .names
                .keys()
                .filter(|n| !cell_names.contains(*n))
                .map(String::as_str)
                .collect();
            if !rest.is_empty() {
                body.push_str(" · also ");
                body.push_str(&rest.join(", "));
            }
        }
        self.dump().await;
        self.finish_output(body, is_error)
    }

    /// Prepend the pending `<runtime>` line, fill in `(no output)`, and scrub
    /// known secret values (37 §3.4: `open('/cwd/.env').read()` after a
    /// `write_file` that expanded a placeholder).
    fn finish_output(&mut self, body: String, is_error: bool) -> ToolOutput {
        let body = if body.is_empty() {
            "(no output)".to_owned()
        } else {
            body
        };
        let content = match self.lost.take() {
            Some(line) => format!("{line}\n{body}"),
            None => body,
        };
        let mut out = if is_error {
            ToolOutput::err(content)
        } else {
            ToolOutput::ok(content)
        };
        tools::secrets::redact_output(&mut out);
        out
    }

    /// Hidden feed: bind `inputs` as globals and read back `(name, type, len)`
    /// for every candidate; names that came back undefined are dropped.
    async fn probe(&mut self, inputs: Vec<(String, MontyObject)>) {
        let code = probe_code(&self.candidates);
        let mut quiet = on_print_sync(|_, _| {});
        match self.drive(&code, inputs, true, "", &mut quiet).await {
            Ok(MontyObject::List(rows)) => {
                let mut seen = BTreeMap::new();
                for row in &rows {
                    if let MontyObject::Tuple(t) = row
                        && let [MontyObject::String(name), MontyObject::String(kind), len] =
                            t.as_slice()
                    {
                        let len = match len {
                            MontyObject::Int(n) => usize::try_from(*n).ok(),
                            _ => None,
                        };
                        seen.insert(
                            name.clone(),
                            Slot {
                                kind: kind.clone(),
                                len,
                            },
                        );
                    }
                }
                self.candidates.retain(|n| seen.contains_key(n));
                self.names = seen;
            }
            Ok(other) => tracing::warn!("runtime probe returned {other:?}"),
            Err(e) => tracing::warn!("runtime probe failed: {e}"),
        }
    }

    /// Dump the heap and the inventory (atomic rename).
    async fn dump(&mut self) {
        let Some(c) = self.checkout.as_mut() else {
            return;
        };
        match c.dump().await {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&self.dump_path, &bytes) {
                    tracing::warn!("runtime dump {}: {e}", self.dump_path.display());
                }
                self.last_dump = Some(bytes);
            }
            Err(e) => tracing::warn!("runtime dump failed: {e}"),
        }
        let rows: Vec<Entry> = self
            .names
            .iter()
            .map(|(n, s)| Entry {
                name: n.clone(),
                kind: s.kind.clone(),
                len: s.len,
            })
            .collect();
        if let Ok(json) = serde_json::to_vec(&rows)
            && let Err(e) = write_atomic(&self.names_path, &json)
        {
            tracing::warn!("runtime names {}: {e}", self.names_path.display());
        }
    }

    /// Dump and hand the worker back to the pool (turn end).
    pub async fn close(mut self) {
        self.dump().await;
        if let Some(c) = self.checkout.take()
            && let Err(e) = c.finish().await
        {
            tracing::debug!("runtime checkout finish: {e}");
        }
    }

    /// Feed one snippet and answer every suspension until it completes.
    async fn drive(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        skip_type_check: bool,
        call_id: &str,
        on_print: OnPrint<'_>,
    ) -> Result<MontyObject, PoolError> {
        let Self {
            checkout,
            cx,
            mounts,
            ..
        } = self;
        let checkout = checkout.as_mut().ok_or(PoolError::Finished)?;
        let mut pending: HashMap<u32, PendingCall> = HashMap::new();
        let mut n = 0u32;
        let mut ev = checkout
            .feed(
                code,
                inputs,
                mounts.clone(),
                skip_type_check,
                &mut *on_print,
            )
            .await?;
        loop {
            ev = match ev {
                TurnEvent::Complete(v) => return Ok(v),
                TurnEvent::NameLookup { name, .. } => {
                    let answer = match name.as_str() {
                        "tools" | "help" => NameLookupResult::Value(MontyObject::Function {
                            name,
                            docstring: None,
                        }),
                        _ => match find_tool(&cx.registry, cx.session, &name).await {
                            Some(spec) => NameLookupResult::Value(MontyObject::Function {
                                name,
                                docstring: Some(summary(&spec.description)),
                            }),
                            None => NameLookupResult::Undefined,
                        },
                    };
                    checkout.resume_name_lookup(answer, &mut *on_print).await?
                }
                TurnEvent::FunctionCall {
                    function_name,
                    args,
                    kwargs,
                    call_id: fid,
                    ..
                } => {
                    let answer = match function_name.as_str() {
                        "tools" => {
                            let query = first_arg(&args, &kwargs, "query");
                            let query = query.as_ref().and_then(|q| match q {
                                MontyObject::String(s) => Some(s.as_str()),
                                _ => None,
                            });
                            let specs = cx
                                .registry
                                .ask(ListTools(Some(cx.session)))
                                .await
                                .unwrap_or_default();
                            ResumeValue::Return(MontyObject::List(
                                specs
                                    .iter()
                                    .map(signature)
                                    .filter(|line| query.is_none_or(|q| line.contains(q)))
                                    .map(MontyObject::String)
                                    .collect(),
                            ))
                        }
                        "help" => match first_arg(&args, &kwargs, "name") {
                            Some(MontyObject::String(name)) => {
                                match find_tool(&cx.registry, cx.session, &name).await {
                                    Some(spec) => {
                                        ResumeValue::Return(MontyObject::String(help(&spec)))
                                    }
                                    None => ResumeValue::Error(MontyException::new(
                                        ExcType::ValueError,
                                        Some(format!("help: unknown tool {name:?}")),
                                    )),
                                }
                            }
                            _ => ResumeValue::Error(MontyException::new(
                                ExcType::TypeError,
                                Some("help(name: str) takes one string argument".into()),
                            )),
                        },
                        _ => match find_tool(&cx.registry, cx.session, &function_name).await {
                            Some(spec) => {
                                n += 1;
                                let id = format!("{call_id}.{n}");
                                let args = merge_args(&spec, args, kwargs);
                                cx.rt
                                    .publish(AgentEvent::ToolCallStarted {
                                        session: cx.session,
                                        turn: cx.turn,
                                        call_id: id.clone(),
                                        name: spec.name.clone(),
                                        args: args.clone(),
                                    })
                                    .await;
                                pending.insert(
                                    fid,
                                    PendingCall {
                                        id,
                                        name: spec.name,
                                        args,
                                    },
                                );
                                ResumeValue::Future
                            }
                            None => ResumeValue::NotFound,
                        },
                    };
                    checkout.resume(answer, &mut *on_print).await?
                }
                TurnEvent::ResolveFutures { pending_call_ids } => {
                    let calls: Vec<(u32, PendingCall)> = pending_call_ids
                        .iter()
                        .filter_map(|id| pending.remove(id).map(|c| (*id, c)))
                        .collect();
                    let outs = join_all(calls.iter().map(|(_, c)| {
                        cx.registry
                            .ask(Invoke(tools::ToolCall {
                                id: c.id.clone(),
                                name: c.name.clone(),
                                args: c.args.clone(),
                                session: cx.session,
                                quiet: false,
                            }))
                            .send()
                    }))
                    .await;
                    let mut results = Vec::with_capacity(calls.len());
                    for ((fid, call), out) in calls.into_iter().zip(outs) {
                        let out =
                            out.unwrap_or_else(|e| ToolOutput::err(format!("tool registry: {e}")));
                        cx.rt
                            .publish(AgentEvent::ToolCallFinished {
                                session: cx.session,
                                turn: cx.turn,
                                call_id: call.id,
                                output: crate::frame::truncate(&out.content, DISPLAY_CHARS),
                                is_error: out.is_error,
                                files: out.files.clone(),
                            })
                            .await;
                        results.push((fid, tool_result(&call.name, out)));
                    }
                    // Ids we never registered (should not happen): fail them
                    // rather than strand the sandbox.
                    for id in pending_call_ids {
                        if !results.iter().any(|(fid, _)| *fid == id) {
                            results.push((
                                id,
                                ResumeValue::Error(MontyException::new(
                                    ExcType::RuntimeError,
                                    Some("unknown pending call".into()),
                                )),
                            ));
                        }
                    }
                    checkout.resume_futures(results, &mut *on_print).await?
                }
                TurnEvent::OsCall {
                    function_name,
                    args,
                    ..
                } => match checkout.resume_from_mounts(&mut *on_print).await? {
                    Some(ev) => ev,
                    None => {
                        checkout
                            .resume(os_answer(&function_name, &args), &mut *on_print)
                            .await?
                    }
                },
            };
        }
    }
}

/// Live name resolution: the tool `session` sees whose sanitised name is
/// `ident` (first by sorted name on a clash).
async fn find_tool(
    registry: &ActorRef<ToolRegistry>,
    session: SessionId,
    ident: &str,
) -> Option<ToolSpec> {
    registry
        .ask(ListTools(Some(session)))
        .await
        .ok()?
        .into_iter()
        .find(|s| sanitize_ident(&s.name) == ident)
}

fn first_arg(
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
    name: &str,
) -> Option<MontyObject> {
    args.first().cloned().or_else(|| {
        kwargs
            .iter()
            .find(|(k, _)| matches!(k, MontyObject::String(s) if s == name))
            .map(|(_, v)| v.clone())
    })
}

/// kwargs by name; positional args in signature order; unknown kwargs pass through.
fn merge_args(
    spec: &ToolSpec,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
) -> Value {
    let mut obj = serde_json::Map::new();
    for (name, v) in params(&spec.schema).iter().map(|p| p.name).zip(&args) {
        obj.insert(name.to_owned(), from_monty(v));
    }
    for (k, v) in &kwargs {
        let key = match k {
            MontyObject::String(s) => s.clone(),
            other => other.py_repr(),
        };
        obj.insert(key, from_monty(v));
    }
    Value::Object(obj)
}

/// Tool output → sandbox return value or exception (08 §3.3, §4.2).
fn tool_result(name: &str, out: ToolOutput) -> ResumeValue {
    if out.is_error {
        let exc = if out.content.ends_with("denied by user")
            || out.content.ends_with("denied by policy")
        {
            ExcType::PermissionError
        } else if out.content.ends_with("approval timed out") {
            ExcType::TimeoutError
        } else {
            ExcType::RuntimeError
        };
        let msg = if out.content.starts_with(name) {
            out.content
        } else {
            format!("{name}: {}", out.content)
        };
        return ResumeValue::Error(MontyException::new(exc, Some(msg)));
    }
    let value = match out.structured {
        Some(v) => to_monty(&v),
        None => match serde_json::from_str::<Value>(&out.content) {
            Ok(v @ (Value::Object(_) | Value::Array(_))) => to_monty(&v),
            _ => MontyObject::String(out.content),
        },
    };
    ResumeValue::Return(value)
}

/// OS calls no mount covers: clock from the host, empty environment, else the
/// call's own default (`NotHandled`).
fn os_answer(function_name: &str, args: &[MontyObject]) -> ResumeValue {
    match function_name {
        "datetime.now" => {
            let now = chrono::Utc::now();
            let (offset, tz_name) = match args.first() {
                Some(MontyObject::TimeZone(tz)) => (Some(tz.offset_seconds), tz.name.clone()),
                _ => (None, None),
            };
            let local = match offset {
                Some(o) => now + chrono::Duration::seconds(i64::from(o)),
                None => chrono::Local::now().naive_local().and_utc(),
            };
            use chrono::{Datelike, Timelike};
            ResumeValue::Return(MontyObject::DateTime(MontyDateTime {
                year: local.year(),
                month: local.month() as u8,
                day: local.day() as u8,
                hour: local.hour() as u8,
                minute: local.minute() as u8,
                second: local.second() as u8,
                microsecond: local.timestamp_subsec_micros(),
                offset_seconds: offset,
                timezone_name: tz_name,
            }))
        }
        "date.today" => {
            use chrono::Datelike;
            let d = chrono::Local::now().date_naive();
            ResumeValue::Return(MontyObject::Date(MontyDate {
                year: d.year(),
                month: d.month() as u8,
                day: d.day() as u8,
            }))
        }
        "os.getenv" => ResumeValue::Return(args.get(1).cloned().unwrap_or(MontyObject::None)),
        "os.environ" => ResumeValue::Return(MontyObject::dict(Vec::new())),
        _ => ResumeValue::NotHandled,
    }
}

/// stdout + `→ repr` for a completed cell (uncapped stdout; capping happens
/// in [`render_capped`] once the `outN` name is known).
fn render_value(stdout: &str, v: &MontyObject) -> String {
    let repr = match v {
        MontyObject::None => None,
        other => Some(cap(&other.py_repr(), |n| {
            format!("[… {n} chars omitted; the value is in the heap — slice or search it]")
        })),
    };
    join_out(stdout, &repr.map_or(String::new(), |r| format!("→ {r}")))
}

fn join_out(stdout: &str, rest: &str) -> String {
    let out = stdout.trim_end_matches('\n');
    match (out.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_owned(),
        (false, true) => out.to_owned(),
        (false, false) => format!("{out}\n{rest}"),
    }
}

/// Re-render a body whose stdout exceeded the cap, now that it is bound as `name`.
fn render_capped(stdout: &str, name: &str, body: &str) -> String {
    let trimmed = stdout.trim_end_matches('\n');
    let rest = body
        .strip_prefix(trimmed)
        .map_or("", |r| r.trim_start_matches('\n'));
    let capped = cap(trimmed, |n| {
        format!("[… {n} chars omitted; full output bound to {name} — slice or search it]")
    });
    join_out(&capped, rest)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ToolSpec {
        ToolSpec {
            name: "github__search-repos".into(),
            description: "Search for GitHub repositories. Uses the GitHub search syntax.".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "perPage": {"type": "integer", "description": "Results per page"},
                    "page": {"type": "integer"},
                    "sort": {"type": "string", "enum": ["stars", "forks"]},
                    "labels": {"type": "array", "items": {"type": "string"}},
                    "filters": {"type": "object", "properties": {"state": {"type": "string"}}},
                    "id": {"anyOf": [{"type": "string"}, {"type": "integer"}]}
                },
                "required": ["query", "perPage"]
            }),
            risk: Risk::Mutating,
            direct: false,
            returns: "Any".to_owned(),
        }
    }

    #[test]
    fn signature_matches_documented_form() {
        assert_eq!(
            signature(&fixture()),
            "async def github__search_repos(query: str, perPage: int, filters: dict | None = None, \
             id: str | int | None = None, labels: list[str] | None = None, page: int | None = None, \
             sort: Literal[\"stars\", \"forks\"] | None = None) -> Any  # Search for GitHub repositories."
        );
        let s = stubs(&[fixture()]);
        assert!(s.contains("page: int | None = None, sort: Literal[\"stars\", \"forks\"] | None = None, **extra: Any) -> Any: ..."));
        assert!(s.contains("\ndef help(name: str) -> str: ...\n"));
        let h = help(&fixture());
        assert!(h.contains("  filters  dict {state: str}"), "{h}");
        assert!(
            h.contains("  query    str  ") && h.contains("  required   Search query\n"),
            "{h}"
        );
    }

    /// A tool is defined for the model exactly once: direct tools are absent
    /// from the runtime index; with `runtime_only: false` the index is empty.
    #[test]
    fn runtime_index_excludes_direct_tools() {
        let runtime_only = fixture();
        let direct = ToolSpec {
            name: "ask_user".into(),
            direct: true,
            ..fixture()
        };
        let block = prompt_block(&[runtime_only, direct.clone()], true);
        assert!(block.contains("async def github__search_repos("));
        assert!(!block.contains("async def ask_user("), "{block}");
        assert!(!prompt_block(&[fixture(), direct], false).contains("async def "));
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_ident("a-b.c"), "a_b_c");
        assert_eq!(sanitize_ident("9lives"), "_9lives");
    }

    #[test]
    fn inventory_candidates() {
        let code = "import json\nimport collections as col\nfrom re import compile as rx, search\n\
                    x = 1\ny: int = 2\nz == 3\nw += 1\nif x:\n    inner = 5\n\
                    async def f():\n    pass\nclass K:\n    pass\n_hidden = 1\ntools = 3\n";
        let got: Vec<String> = candidates(code).into_iter().collect();
        assert_eq!(got, ["K", "col", "f", "json", "rx", "search", "x", "y"]);
    }

    #[test]
    fn json_round_trip() {
        let v = serde_json::json!({"a": [1, 2.5, null, true, "s"], "b": {"c": {}}});
        assert_eq!(from_monty(&to_monty(&v)), v);
        let d = MontyObject::dict(vec![(MontyObject::Int(1), MontyObject::String("a".into()))]);
        assert_eq!(from_monty(&d), serde_json::json!({"1": "a"}));
    }

    /// Drives a real checkout through a `monty` binary on `PATH` (the test
    /// runner is not a worker); skipped when absent.
    #[tokio::test]
    async fn capping_binds_out_name() {
        let monty = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("monty"))
                .find(|p| p.is_file())
        });
        let Some(monty) = monty else {
            eprintln!("monty not on PATH; skipping");
            return;
        };
        let Some(pool) = open_pool(&monty, 1).await else {
            eprintln!("monty pool failed to start; skipping");
            return;
        };
        let dir = tempdir();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let registry = tools::registry::start(&rt).await.unwrap();
        std::fs::write(dir.join("a.txt"), "hello mount").unwrap();
        let session = SessionId::new();
        let cx = RunCx {
            session,
            turn: TurnId::new(),
            rt,
            registry,
            spill_dir: dir.join("spill"),
            cwd: dir.clone(),
        };
        let mut repl = Repl::open(&pool, session, &dir, cx.clone()).await;
        let out = repl
            .run(
                "c1",
                "for i in range(3000):\n    print(f'line {i:04}')\nn = 3",
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("full output bound to out1 — slice or search it]"),
            "{}",
            out.content
        );
        assert!(out.content.starts_with("line 0000\n"));
        assert!(
            out.content.ends_with(
                "line 2999\nbindings (reuse, do not re-read): n (int) · out1 (str, 30000)"
            ),
            "{}",
            out.content
        );
        // bound as a global, visible to the type checker, survives dump/restore
        let out = repl.run("c2", "len(out1)").await;
        assert_eq!(out.content, "→ 30000", "{}", out.content);
        let out = repl.run("c3", "await nope(1)").await;
        assert!(
            out.is_error && out.content.starts_with("type check failed:"),
            "{}",
            out.content
        );
        let out = repl.run("c4", "r = [1, 2, 3]\nr").await;
        assert_eq!(
            out.content,
            "→ [1, 2, 3]\nbindings (reuse, do not re-read): r (list, 3) · also n, out1"
        );
        assert_eq!(
            repl.render_inventory().as_deref(),
            Some("n (int) · out1 (str, 30000) · r (list, 3)")
        );
        // worker killed mid-session → fresh worker, previous dump restored
        let pid = repl.checkout.as_ref().unwrap().pid().unwrap();
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let out = repl.run("c4b", "r.append(4)\nr").await;
        assert!(
            out.is_error
                && out.content.starts_with("RuntimeError: runtime crashed (")
                && out.content.ends_with(
                    "state restored to after the previous cell — this cell's effects are lost"
                ),
            "{}",
            out.content
        );
        let out = repl.run("c4c", "r").await;
        assert_eq!(out.content, "→ [1, 2, 3]");
        repl.close().await;

        let mut repl = Repl::open(&pool, session, &dir, cx.clone()).await;
        let out = repl.run("c5", "r + [n]").await;
        assert_eq!(out.content, "→ [1, 2, 3, 3]");
        let out = repl.run("c6", "1/0").await;
        assert!(
            out.is_error && out.content.ends_with("ZeroDivisionError: division by zero"),
            "{}",
            out.content
        );
        // inner tool calls (positional + keyword, gathered) and the read-only mount
        let out = repl
            .run("c7", "import asyncio\nls, txt = await asyncio.gather(list_dir('/'), read_file(path='/nonexistent'))")
            .await;
        assert!(
            out.is_error && out.content.contains("RuntimeError: read_file:"),
            "{}",
            out.content
        );
        let out = repl.run("c8", "ls = await list_dir(path='/')\nmsg = ''\ntry:\n    await read_file('/nonexistent')\nexcept RuntimeError as e:\n    msg = str(e)\n(type(ls).__name__, msg[:10])").await;
        assert!(
            out.content.starts_with(
                "→ ('str', 'read_file:')\nbindings (reuse, do not re-read): ls (str, "
            ),
            "{}",
            out.content
        );
        assert!(out.content.contains(") · msg (str, "), "{}", out.content);
        let out = repl.run("c9", "open('/cwd/a.txt').read()").await;
        assert_eq!(out.content, "→ 'hello mount'");
        let out = repl
            .run("c10", "print(tools('read_file')[0][:19]); help('list_dir')")
            .await;
        assert!(
            out.content.starts_with(
                "async def read_file\n→ 'async def list_dir(path: str) -> str\\n\\nList a directory"
            ),
            "{}",
            out.content
        );
        repl.close().await;

        // a corrupt dump → fresh runtime with the loss line naming what was held
        std::fs::write(
            dir.join("runtime").join(format!("{session}.monty")),
            b"garbage",
        )
        .unwrap();
        let mut repl = Repl::open(&pool, session, &dir, cx).await;
        let out = repl.run("c11", "1 + 1").await;
        assert!(
            out.content
                .starts_with("<runtime> saved state could not be restored"),
            "{}",
            out.content
        );
        assert!(
            out.content
                .contains("n (int) · out1 (str, 30000) · r (list, 3)")
                && out.content.ends_with("→ 2"),
            "{}",
            out.content
        );
        repl.close().await;
        pool.close().await;
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("wth-repl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
