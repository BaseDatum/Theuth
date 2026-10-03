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
    CallArgs, ExcType, MontyException, MontyObject, NameLookupResult, ObjectRef, OsPolicy,
    ResourceLimits, SandboxTimeZone, SleepMode,
    unstable::{self, MontyNode},
};
use regex::Regex;
use runtime::{SessionId, TurnId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::{Invoke, ListTools, Risk, ToolOutput, ToolRegistry, ToolSpec};

use crate::AgentEvent;
use crate::loop_::{CellPermit, CellRunning};

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
/// Longest single `time.sleep`/`asyncio.sleep`; the worker clamps longer delays
/// (`SleepMode::System`), the host waits. No runtime config section exists to
/// slot this into, so a constant like the limits above.
const MAX_SLEEP: Duration = Duration::from_secs(300);
/// Cumulative sleep per checkout (one turn, like `MAX_SUSPENSIONS`); the pool
/// rejects the sleep that would exceed it with an uncatchable `TimeoutError`,
/// so a sleeping loop cannot hang a session (`ResourceLimits::max_total_sleep`).
const MAX_TOTAL_SLEEP: Duration = Duration::from_secs(600);
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
parameters and description, `bindings()` lists your persisted globals. `/cwd` is the working \
directory (read-only); write files with `write_file()`.";

const RUNTIME_BLOCK: &str = "## Runtime
`python` runs a sandboxed Python 3.14 subset (Monty): no third-party packages, no
generators, no class inheritance; stdlib: json re math datetime time random copy collections
itertools functools dataclasses pathlib base64 asyncio(gather, sleep). State persists, except
names starting with `_`, which do not carry across cells. Imports and variables persist
across cells; a cell is type-checked before it runs and, if it fails, nothing ran and
nothing is lost. `asyncio.gather` takes positional awaitables only - no `return_exceptions=`
or other keyword arguments (monty limitation); to tolerate individual failures wrap each call
in an `async def` that try/excepts. A cell that raises before awaiting a tool future leaves
that call un-run. `shell()` returns the output when
the command finishes within a few seconds, otherwise `{\"job\": id, \"log\": path}`; the
result arrives later as <job_result>, the way a timer's message arrives as <reminder>. A PTY
job whose screen stops changing for a minute wakes you with <job_idle> and the screen: the
program is likely waiting on you. Waiting for something? Prefer `remind_me_in(seconds, message)`:
it returns immediately, the session is free, and your message comes back as a <reminder> input
when the timer fires. `asyncio.sleep` is for sub-minute pauses inside one cell.

The runtime is your working memory; the conversation is for decisions. Every tool call
is an `await` expression - bind its result to a name (`src = await read_file(path=\"x.rs\")`)
and work on it in Python: slice, filter, regex, count, diff. `read_file()` returns a header
line then `N<tab>text` lines; pass `numbers=False` when you want the file's raw text.
Print only what you need to decide the next step, never a whole file or a whole result;
the full value stays in the variable and survives compaction and restart. Use Python for
loops, conditions and parsing; use `shell()` to run programs, not to write shell programs.
Discover a tool's full parameters with `help(name)` - the index below is names and
signatures only. `bindings()` lists every persisted global with its type and length; a
cell result only footers the names that cell bound or changed. The tools defined directly
for you are awaitable here too, by the same name.

Runtime functions (NOT direct tools — call each as `r = await name(...)` inside `python`; all `async`):
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
    /// 50 §8: the turn's execution bound as the cell counts itself; `None`
    /// for a turn that holds no permit (opened by a human or the harness).
    pub(crate) permit: Option<CellPermit>,
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// Start the worker pool on `worker`, spawned as `<worker> subprocess`. In
/// production that is this binary (`t subprocess`, 08 §9), so nothing has
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
    out.push_str("def bindings() -> list[str]: ...\n");
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
        Value::Null => MontyObject::none(),
        Value::Bool(b) => MontyObject::bool(*b),
        Value::Number(n) => n.as_i64().map_or_else(
            || MontyObject::float(n.as_f64().unwrap_or(f64::NAN)),
            MontyObject::int,
        ),
        Value::String(s) => MontyObject::string(s.clone()),
        Value::Array(a) => MontyObject::list(a.iter().map(to_monty)),
        Value::Object(o) => MontyObject::dict(
            o.iter()
                .map(|(k, v)| (MontyObject::string(k.clone()), to_monty(v))),
        ),
    }
}

/// Sandbox value → JSON; dict keys are stringified, exotic values become their repr.
pub fn from_monty(o: ObjectRef<'_>) -> Value {
    if o.type_name() == "NoneType" {
        return Value::Null;
    }
    if let Some(b) = o.as_bool() {
        return Value::Bool(b);
    }
    if let Some(i) = o.as_int() {
        return Value::from(i);
    }
    if o.type_name() == "float" {
        return o
            .as_float()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number);
    }
    if let Some(s) = o.as_str() {
        return Value::String(s.to_owned());
    }
    if let Some(items) = o.items() {
        return Value::Array(items.into_iter().map(from_monty).collect());
    }
    if let Some(pairs) = o.pairs() {
        return Value::Object(
            pairs
                .into_iter()
                .map(|(k, v)| {
                    (
                        k.as_str().map_or_else(|| k.py_repr(), str::to_owned),
                        from_monty(v),
                    )
                })
                .collect(),
        );
    }
    // Leaves the stable accessors cannot read; `unstable` is the crate's own
    // escape hatch and may move at the next monty bump.
    match unstable::node(o) {
        MontyNode::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        MontyNode::Date(d) => Value::String(format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)),
        MontyNode::DateTime(d) => Value::String(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            d.year, d.month, d.day, d.hour, d.minute, d.second
        )),
        _ => Value::String(o.py_repr()),
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

/// Top-level import statements of a cell: bound name → the full line, so
/// a later cell that uses the name gets it re-imported in front of the
/// checked source (imports are idempotent; the heap does not change).
fn import_lines(code: &str) -> impl Iterator<Item = (String, String)> + '_ {
    code.lines()
        .filter(|l| l.starts_with("import ") || l.starts_with("from "))
        .flat_map(|l| candidates(l).into_iter().map(move |n| (n, l.to_owned())))
}

static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z_]\w*\b").expect("ident re"));

/// The stored import statements a cell needs: those whose bound name the
/// cell uses as an identifier and does not bind itself. One line each,
/// deduplicated (a `from` line binding two names appears once).
fn prelude(
    imports: &BTreeMap<String, String>,
    code: &str,
    cell_names: &BTreeSet<String>,
) -> String {
    let idents: BTreeSet<&str> = IDENT_RE.find_iter(code).map(|m| m.as_str()).collect();
    let lines: BTreeSet<&str> = imports
        .iter()
        .filter(|(n, _)| idents.contains(n.as_str()) && !cell_names.contains(*n))
        .map(|(_, l)| l.as_str())
        .collect();
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

static DIAG_RES: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(r"main\.py:(\d+):").expect("loc re"),
        Regex::new(r"(?m)^(\s*)(\d+) \|").expect("gutter re"),
        Regex::new(r"(?s)error\[([\w-]+)\]: ([^\n]*)\n.*?main\.py:(\d+):").expect("error re"),
    ]
});

/// `(code, message, line in the fed source)` of every `error[...]` in a
/// typing diagnostic.
fn diag_errors(diag: &str) -> Vec<(&str, &str, usize)> {
    DIAG_RES[2]
        .captures_iter(diag)
        .map(|c| {
            (
                c.get(1).expect("code").as_str(),
                c.get(2).expect("msg").as_str(),
                c[3].parse().unwrap_or(0),
            )
        })
        .collect()
}

/// Names a line rebinds that the heap holds: the identifiers in its target
/// region (`x = …`, `x: T = …`, `x += …`, `a, b = …`, `for x in …`,
/// `with … as x:`) that are in `names`.
fn rebound_names(line: &str, names: &BTreeMap<String, Slot>) -> Vec<String> {
    let t = line.trim_start();
    let region = if let Some(rest) = t.strip_prefix("for ") {
        rest.split(" in ").next().unwrap_or("")
    } else if t.starts_with("with ") {
        t.rsplit(" as ").next().unwrap_or("")
    } else {
        t.split('=').next().unwrap_or("")
    };
    IDENT_RE
        .find_iter(region)
        .filter(|m| names.contains_key(m.as_str()))
        .map(|m| m.as_str().to_owned())
        .collect()
}

/// The persisted names to redeclare (08 §4.6): the targets of every
/// `invalid-assignment` diagnostic that rebinds a name the heap holds — the
/// checker declared it with the type it had. Other diagnostics in the same
/// cell do not veto the retry: the old type usually drags follow-on errors
/// behind it (`m + 1` after `for m in [1, 2]` reports `unsupported-operator`
/// on `Match[str] | None`), and a retry that still fails returns the
/// original diagnostic. `None` when no diagnostic is such a rebinding.
fn redeclare_targets(
    errors: &[(&str, &str, usize)],
    src: &str,
    names: &BTreeMap<String, Slot>,
) -> Option<BTreeSet<String>> {
    let out: BTreeSet<String> = errors
        .iter()
        .filter(|(code, _, _)| *code == "invalid-assignment")
        .filter_map(|(_, _, line)| src.lines().nth(line.checked_sub(1)?))
        .flat_map(|l| rebound_names(l, names))
        .collect();
    (!out.is_empty()).then_some(out)
}

static UNRESOLVED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Name `(\w+)` used when not defined").expect("unresolved re"));

/// Every diagnostic is an `unresolved-reference` to a name the heap holds:
/// a module the prelude could not reconstruct, an `outN`, a name bound by
/// an unchecked feed. The checker is wrong, the cell can run.
fn all_unresolved_held(errors: &[(&str, &str, usize)], names: &BTreeMap<String, Slot>) -> bool {
    !errors.is_empty()
        && errors.iter().all(|(code, msg, _)| {
            *code == "unresolved-reference"
                && UNRESOLVED_RE
                    .captures(msg)
                    .is_some_and(|c| names.contains_key(&c[1]))
        })
}

/// Shift every `main.py:LINE:` and, in the snippets under a `main.py`
/// location, every ` N | ` gutter number down by `offset` (the prelude
/// lines the model did not write); gutter lines inside the prelude are
/// dropped. Snippets from other files (`stdlib/re.pyi`) are left alone.
fn rebase_lines(diag: &str, offset: usize) -> String {
    if offset == 0 {
        return diag.to_owned();
    }
    let [loc, gutter, _] = &*DIAG_RES;
    let mut out = String::with_capacity(diag.len());
    let mut in_main = false;
    for line in diag.lines() {
        if let Some(arrow) = line.find("--> ") {
            in_main = loc.is_match(&line[arrow..]);
            out.push_str(&loc.replace_all(line, |c: &regex::Captures| {
                format!(
                    "main.py:{}:",
                    c[1].parse::<usize>().unwrap_or(0).saturating_sub(offset)
                )
            }));
        } else if in_main && let Some(c) = gutter.captures(line) {
            let n: usize = c[2].parse().unwrap_or(0);
            if n <= offset {
                continue;
            }
            let width = c[1].len() + c[2].len();
            out.push_str(&format!("{:>width$} |", n - offset));
            out.push_str(&line[c.get(0).expect("m").end()..]);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !diag.ends_with('\n') {
        out.pop();
    }
    out
}

/// One tracked global.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Slot {
    kind: String,
    len: Option<usize>,
}

/// `.names.json` row; `import` is the statement that bound the name, when
/// it was an import (absent in files written before it was tracked).
#[derive(Serialize, Deserialize)]
struct Entry {
    name: String,
    kind: String,
    len: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import: Option<String>,
}

fn render_slots<'a>(it: impl Iterator<Item = (&'a String, &'a Slot)>) -> Option<String> {
    let parts = slot_items(it);
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn slot_items<'a>(it: impl Iterator<Item = (&'a String, &'a Slot)>) -> Vec<String> {
    it.map(|(n, s)| match s.len {
        Some(l) => format!("{n} ({}, {l})", s.kind),
        None => format!("{n} ({})", s.kind),
    })
    .collect()
}

/// Byte cap on a cell result's bindings footer (08 §3.3). The namespace lives
/// in the runtime (`bindings()`), not in every transcript row: one session
/// measured 402 cells × ~2.3k chars of footer ≈ 230k tokens, 66% of all its
/// tool-result text.
pub const FOOTER_CHARS: usize = 300;
const FOOTER_HINT: &str = "bindings (reuse, do not re-read; bindings() lists all): ";
const FOOTER_BARE: &str = "bindings: ";

/// The footer for the names this cell bound or changed, never longer than
/// [`FOOTER_CHARS`]: items that do not fit collapse into `… +n more`. `hint`
/// spells out the reuse rule; the caller sends it once per turn.
fn footer(items: &[String], hint: bool) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut out = String::from(if hint { FOOTER_HINT } else { FOOTER_BARE });
    let more_len = format!(" … +{} more", items.len()).len();
    let mut shown = 0;
    for item in items {
        let sep = if shown == 0 { "" } else { " · " };
        let reserve = if shown + 1 < items.len() { more_len } else { 0 };
        if out.len() + sep.len() + item.len() + reserve > FOOTER_CHARS {
            break;
        }
        out.push_str(sep);
        out.push_str(item);
        shown += 1;
    }
    if shown < items.len() {
        if shown > 0 {
            out.push(' ');
        }
        out.push_str(&format!("… +{} more", items.len() - shown));
    }
    Some(out)
}

/// The tracked inventory and its import statements from `.names.json`
/// (empty when absent or unreadable).
fn read_names(path: &Path) -> (BTreeMap<String, Slot>, BTreeMap<String, String>) {
    let rows = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Entry>>(&b).ok())
        .unwrap_or_default();
    let mut names = BTreeMap::new();
    let mut imports = BTreeMap::new();
    for e in rows {
        if let Some(line) = e.import {
            imports.insert(e.name.clone(), line);
        }
        names.insert(
            e.name,
            Slot {
                kind: e.kind,
                len: e.len,
            },
        );
    }
    (names, imports)
}

/// The last persisted inventory of `session`, rendered like
/// [`Repl::render_inventory`]: for the compaction head when no [`Repl`] is
/// open in the turn (`/compact` between turns). `None` when nothing is tracked.
pub fn saved_inventory(data_dir: &Path, session: SessionId) -> Option<String> {
    let (names, _) = read_names(
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
    /// Invocation order within the cell (the `n` of `"{call_id}.{n}"`).
    seq: u32,
    id: String,
    name: String,
    args: Value,
}

/// Output of an inner call the cell created but never awaited.
const NOT_AWAITED: &str = "ERROR: not run: the cell ended before this call was awaited";

/// Inner calls whose `ToolCallStarted` went out and whose future the cell has
/// not awaited yet. Whatever is left when the cell ends — return, raise, host
/// error, timeout — is finished as an error and never invoked (`drain`); a
/// cancelled cell (dropped `drive`) does the same from `Drop`, so no UI card
/// spins forever.
struct Pending<'a> {
    calls: HashMap<u32, PendingCall>,
    cx: &'a RunCx,
}

impl Pending<'_> {
    /// Remaining calls in invocation order, leaving none.
    fn take(&mut self) -> Vec<PendingCall> {
        let mut left: Vec<PendingCall> = self.calls.drain().map(|(_, c)| c).collect();
        left.sort_unstable_by_key(|c| c.seq);
        left
    }

    /// Finish every remaining call as not run; how many there were.
    async fn drain(&mut self) -> usize {
        let left = self.take();
        for c in &left {
            self.cx.rt.publish(not_run(self.cx, c)).await;
        }
        left.len()
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.calls.is_empty() {
            return;
        }
        let (left, cx) = (self.take(), self.cx.clone());
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                for c in &left {
                    cx.rt.publish(not_run(&cx, c)).await;
                }
            });
        }
    }
}

fn not_run(cx: &RunCx, c: &PendingCall) -> AgentEvent {
    AgentEvent::ToolCallFinished {
        session: cx.session,
        turn: cx.turn,
        call_id: c.id.clone(),
        output: NOT_AWAITED.into(),
        is_error: true,
        files: Vec::new(),
    }
}

/// One session's runtime for one turn (08 §4).
pub struct Repl {
    pool: Arc<Pool>,
    cfg: ReplConfig,
    cx: RunCx,
    /// `None` between a crash and the next cell (re-checked-out lazily).
    checkout: Option<Checkout>,
    mounts: Vec<MountSpec>,
    /// `<spill_dir>/<session>`: the `/spill` mount's host side; inner tool
    /// images are snapshotted here (52 §7.1).
    spill: PathBuf,
    dump_path: PathBuf,
    names_path: PathBuf,
    /// Last good dump: restored into a fresh checkout after a crash.
    last_dump: Option<Vec<u8>>,
    names: BTreeMap<String, Slot>,
    candidates: BTreeSet<String>,
    /// Bound name → the `import`/`from` line of a run cell that bound it;
    /// prepended to a later cell that uses the name so the checker knows it.
    imports: BTreeMap<String, String>,
    out_seq: u32,
    /// `<runtime>` line prepended to the next cell result.
    lost: Option<String>,
    /// Inner calls the last `drive` finished as not run (created, never awaited).
    unawaited: usize,
    /// The reuse hint went out in a cell footer this turn (one `Repl` = one turn).
    hinted: bool,
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
                max_total_sleep: Some(MAX_TOTAL_SLEEP),
                ..ResourceLimits::default()
            }),
            type_check: true,
            type_check_stubs: Some(stubs(&specs)),
            // The worker answers `datetime.now()`/`date.today()` itself (beta.2
            // `OsPolicy`); pin its zone to the host's so naive values read as
            // host local time, as the host-answered calls did before.
            // ponytail: fixed offset sampled per turn, no DST rule; use
            // `SandboxTimeZone::named` (needs the `tzdb` feature) if a turn
            // spanning a DST switch matters.
            os_policy: OsPolicy {
                timezone: SandboxTimeZone::Fixed {
                    offset_seconds: chrono::Local::now().offset().local_minus_utc(),
                    name: None,
                },
                sleep: SleepMode::System(MAX_SLEEP),
                ..OsPolicy::default()
            },
            ..ReplConfig::default()
        };
        let dir = data_dir.join("runtime");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("runtime dir {}: {e}", dir.display());
        }
        let dump_path = dir.join(format!("{session}.monty"));
        let names_path = dir.join(format!("{session}.names.json"));
        let (names, imports) = read_names(&names_path);
        let candidates = names.keys().cloned().collect();
        let mut mounts = Vec::with_capacity(2);
        let spill = cx.spill_dir.join(session.to_string());
        let _ = std::fs::create_dir_all(&spill);
        // beta.2 refuses mounts whose host directories nest (cwd = home with
        // the data dir under it); the spill is then reachable through `/cwd`.
        let nested = match (
            std::fs::canonicalize(&cx.cwd),
            std::fs::canonicalize(&spill),
        ) {
            (Ok(a), Ok(b)) => a.starts_with(&b) || b.starts_with(&a),
            _ => false,
        };
        let spill_mount = if nested {
            None
        } else {
            Some(("/spill", &spill))
        };
        for (virt, host) in std::iter::once(("/cwd", &cx.cwd)).chain(spill_mount) {
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
            spill,
            last_dump: std::fs::read(&dump_path).ok(),
            dump_path,
            names_path,
            names,
            candidates,
            imports,
            out_seq: 0,
            lost: None,
            unawaited: 0,
            hinted: false,
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
                self.imports.clear();
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
            return self.finish_output(
                format!("RuntimeError: runtime unavailable ({e})"),
                true,
                Vec::new(),
            );
        }
        let cell_names = candidates(code);
        self.candidates.extend(cell_names.iter().cloned());
        let before = self.names.clone();
        // Imports from earlier cells that this cell uses go in front of the
        // checked source (08 §4.6): the checker only knows what it checked.
        let prelude = prelude(&self.imports, code, &cell_names);
        let (mut src, mut offset) = if prelude.is_empty() {
            (code.to_owned(), 0)
        } else {
            (format!("{prelude}\n{code}"), prelude.lines().count())
        };

        let mut stdout = String::new();
        // 52 §7.1: every inner image this cell produced, in call/file order,
        // snapshotted as each inner result arrived. Owned by this call so a
        // dropped (cancelled) cell drops it and nothing reaches the next cell.
        let mut files = Vec::new();
        let mut on_print = on_print_sync(|_, t| stdout.push_str(t));
        let mut outcome = self
            .drive(&src, Vec::new(), false, call_id, &mut files, &mut on_print)
            .await;
        if offset > 0
            && let Err(PoolError::Typing(diag)) = &outcome
            && DIAG_RES[0]
                .captures_iter(diag)
                .any(|c| c[1].parse::<usize>().is_ok_and(|l| l <= offset))
        {
            // The prelude itself fails (a module no longer available): the
            // cell as written, its own diagnostics.
            src = code.to_owned();
            offset = 0;
            outcome = self
                .drive(&src, Vec::new(), false, call_id, &mut files, &mut on_print)
                .await;
        }
        if let Err(PoolError::Typing(diag)) = &outcome {
            let errors = diag_errors(diag);
            if let Some(targets) = redeclare_targets(&errors, &src, &self.names) {
                // The checker declared each name with the type it had; an
                // `Any` redeclaration lets the cell rebind it. `del` is not
                // an option: monty does not parse it and ty keeps the
                // declaration anyway. `_Any` stays out of the inventory and
                // of later cells' checks (`_`-prefixed).
                let mut retry = String::from("from typing import Any as _Any\n");
                for n in &targets {
                    retry.push_str(&format!("{n}: _Any\n"));
                }
                retry.push_str(&src);
                let outcome2 = self
                    .drive(
                        &retry,
                        Vec::new(),
                        false,
                        call_id,
                        &mut files,
                        &mut on_print,
                    )
                    .await;
                if !matches!(outcome2, Err(PoolError::Typing(_))) {
                    offset += 1 + targets.len();
                    outcome = outcome2;
                }
            } else if all_unresolved_held(&errors, &self.names) {
                tracing::info!(
                    target: "runtime",
                    session = %self.cx.session,
                    "type check skipped: every unresolved name is in the heap"
                );
                outcome = self
                    .drive(&src, Vec::new(), true, call_id, &mut files, &mut on_print)
                    .await;
            }
        }
        drop(on_print);

        if outcome.is_ok() {
            for n in &cell_names {
                self.imports.remove(n);
            }
            self.imports.extend(import_lines(code));
        }
        let (mut body, is_error) = match outcome {
            Ok(v) => (render_value(&stdout, &v), false),
            Err(PoolError::Runtime(exc)) if exc.exc_type() != ExcType::MemoryError => {
                let mut text = exc.to_string();
                if text.contains("gather() does not yet support keyword arguments") {
                    text.push_str(&format!(
                        "\nhint: monty's asyncio.gather takes positional awaitables only (no return_exceptions=); \
                         wrap each call in a small async helper that catches its exception instead. \
                         The {} tool calls created before the error were not run.",
                        self.unawaited
                    ));
                }
                (join_out(&stdout, &text), true)
            }
            Err(PoolError::Typing(diag)) => {
                return self.finish_output(
                    format!(
                        "type check failed (nothing ran; your variables are intact — resend the cell with the fix):\n{}",
                        rebase_lines(&diag, offset)
                    ),
                    true,
                    files,
                );
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
                    files,
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
                .drive(
                    &format!("{name} = ''"),
                    Vec::new(),
                    false,
                    "",
                    &mut Vec::new(),
                    &mut quiet,
                )
                .await
            {
                tracing::warn!("runtime: declaring {name}: {e}");
            }
            self.candidates.insert(name.clone());
            cell_names.insert(name.clone());
            inputs.push((name, MontyObject::string(stdout)));
        }
        if !self.candidates.is_empty() || !inputs.is_empty() {
            self.probe(inputs).await;
            self.imports.retain(|n, _| self.names.contains_key(n));
        }
        // Names this cell assigned, plus any tracked slot whose type or length
        // moved (a mutation through a call); the rest is one `bindings()` away.
        let changed = self
            .names
            .iter()
            .filter(|(n, s)| cell_names.contains(*n) || before.get(*n) != Some(s));
        if let Some(line) = footer(&slot_items(changed), !self.hinted) {
            self.hinted = true;
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&line);
        }
        self.dump().await;
        self.finish_output(body, is_error, files)
    }

    /// Prepend the pending `<runtime>` line, fill in `(no output)`, attach
    /// the cell's inner images, and scrub known secret values (37 §3.4:
    /// `open('/cwd/.env').read()` after a `write_file` that expanded a
    /// placeholder).
    fn finish_output(&mut self, body: String, is_error: bool, files: Vec<PathBuf>) -> ToolOutput {
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
        out.files = files;
        tools::secrets::redact_output(&mut out);
        out
    }

    /// Hidden feed: bind `inputs` as globals and read back `(name, type, len)`
    /// for every candidate; names that came back undefined are dropped.
    async fn probe(&mut self, inputs: Vec<(String, MontyObject)>) {
        let code = probe_code(&self.candidates);
        let mut quiet = on_print_sync(|_, _| {});
        match self
            .drive(&code, inputs, true, "", &mut Vec::new(), &mut quiet)
            .await
        {
            Ok(v) if v.type_name() == "list" => {
                let mut seen = BTreeMap::new();
                for row in v.as_ref().items().unwrap_or_default() {
                    if let Some(t) = row.items()
                        && let [name, kind, len] = t.as_slice()
                        && let (Some(name), Some(kind)) = (name.as_str(), kind.as_str())
                    {
                        seen.insert(
                            name.to_owned(),
                            Slot {
                                kind: kind.to_owned(),
                                len: len.as_int().and_then(|n| usize::try_from(n).ok()),
                            },
                        );
                    }
                }
                self.candidates.retain(|n| seen.contains_key(n));
                self.names = seen;
            }
            Ok(other) => tracing::warn!("runtime probe returned {}", other.py_repr()),
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
                import: self.imports.get(n).cloned(),
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
    /// Inner tool images are snapshotted the moment each result arrives and
    /// appended to `files` in call/file order (52 §7.1).
    async fn drive(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        skip_type_check: bool,
        call_id: &str,
        files: &mut Vec<PathBuf>,
        on_print: OnPrint<'_>,
    ) -> Result<MontyObject, PoolError> {
        let Self {
            checkout,
            cx,
            mounts,
            spill,
            unawaited,
            names,
            ..
        } = self;
        let (cx, spill): (&RunCx, &Path) = (cx, spill);
        let checkout = checkout.as_mut().ok_or(PoolError::Finished)?;
        let mut pending = Pending {
            calls: HashMap::new(),
            cx,
        };
        let mut n = 0u32;
        // The loop is a block so every exit — value, exception, host error —
        // falls through to the drain below.
        let out: Result<MontyObject, PoolError> = async {
            // 50 §8: the interpreter executes from the feed until it completes,
            // except while it awaits its tool calls (`ResolveFutures`), where the
            // guard is dropped and only those calls count. Holding one includes
            // the permit; a dropped `drive` (cancelled cell) drops the guard.
            let mut running = hold(cx.permit.as_ref()).await?;
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
                            "tools" | "help" | "bindings" => {
                                NameLookupResult::Value(MontyObject::function(name, None))
                            }
                            _ => match find_tool(&cx.registry, cx.session, &name).await {
                                Some(spec) => NameLookupResult::Value(MontyObject::function(
                                    name,
                                    Some(summary(&spec.description)),
                                )),
                                None => NameLookupResult::Undefined,
                            },
                        };
                        checkout.resume_name_lookup(answer, &mut *on_print).await?
                    }
                    TurnEvent::FunctionCall {
                        function_name,
                        args,
                        call_id: fid,
                        ..
                    } => {
                        let answer = match function_name.as_str() {
                            "tools" => {
                                let query = first_arg(&args, "query").and_then(|q| q.as_str());
                                let specs = cx
                                    .registry
                                    .ask(ListTools(Some(cx.session)))
                                    .await
                                    .unwrap_or_default();
                                ResumeValue::Return(MontyObject::list(
                                    specs
                                        .iter()
                                        .map(signature)
                                        .filter(|line| query.is_none_or(|q| line.contains(q)))
                                        .map(MontyObject::string),
                                ))
                            }
                            // The whole inventory as the footer would have listed it (08 §3.3).
                            "bindings" => ResumeValue::Return(MontyObject::list(
                                slot_items(names.iter())
                                    .into_iter()
                                    .map(MontyObject::string),
                            )),
                            "help" => match first_arg(&args, "name").and_then(|q| q.as_str()) {
                                Some(name) => {
                                    match find_tool(&cx.registry, cx.session, name).await {
                                        Some(spec) => {
                                            ResumeValue::Return(MontyObject::string(help(&spec)))
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
                                    let args = merge_args(&spec, &args);
                                    cx.rt
                                        .publish(AgentEvent::ToolCallStarted {
                                            session: cx.session,
                                            turn: cx.turn,
                                            call_id: id.clone(),
                                            name: spec.name.clone(),
                                            args: args.clone(),
                                        })
                                        .await;
                                    pending.calls.insert(
                                        fid,
                                        PendingCall {
                                            seq: n,
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
                    TurnEvent::ResolveFutures {
                        pending_call_ids, ..
                    } => {
                        // In invocation order (gather input order), whatever order
                        // the sandbox lists the futures in.
                        let mut calls: Vec<(u32, PendingCall)> = pending_call_ids
                            .iter()
                            .filter_map(|id| pending.calls.remove(id).map(|c| (*id, c)))
                            .collect();
                        calls.sort_unstable_by_key(|(_, c)| c.seq);
                        // Suspended computation, not execution: the calls count
                        // themselves through the registry's seam (a blocking peer
                        // send or an approval alone releases the permit; a sibling
                        // still running keeps it), and the interpreter resumes only
                        // once a permit is held again.
                        drop(running.take());
                        // Each result's images are copied as it lands: a sibling
                        // still running may overwrite the same path next.
                        let outs = join_all(calls.iter().map(|(_, c)| async move {
                            let mut out = cx
                                .registry
                                .ask(Invoke(tools::ToolCall {
                                    id: c.id.clone(),
                                    name: c.name.clone(),
                                    args: c.args.clone(),
                                    session: cx.session,
                                    quiet: false,
                                }))
                                .send()
                                .await
                                .unwrap_or_else(|e| ToolOutput::err(format!("tool registry: {e}")));
                            snapshot_files(&mut out, spill);
                            out
                        }))
                        .await;
                        running = hold(cx.permit.as_ref()).await?;
                        let mut results = Vec::with_capacity(calls.len());
                        for ((fid, call), mut out) in calls.into_iter().zip(outs) {
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
                            files.append(&mut out.files);
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
                    // `time.sleep`/`asyncio.sleep`: the worker clamps the delay to
                    // `MAX_SLEEP`, the pool charges `MAX_TOTAL_SLEEP`, and the host
                    // waits here — the wait is suspended computation like a tool
                    // call, not interpreter execution, so the permit is released.
                    // ponytail: the async form is answered settled after the wait,
                    // so `gather(sleep(n), tool())` waits n first; register the
                    // sleep as a `pending` future if overlap matters.
                    TurnEvent::OsCall {
                        system_sleep: Some(delay),
                        ..
                    } => {
                        drop(running.take());
                        tokio::time::sleep(delay).await;
                        running = hold(cx.permit.as_ref()).await?;
                        checkout
                            .resume(ResumeValue::Return(MontyObject::none()), &mut *on_print)
                            .await?
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
        .await;
        *unawaited = pending.drain().await;
        out
    }
}

/// 50 §8: count the interpreter as executing (permit held) until the guard
/// drops; nothing to count outside a bounded turn. `Err` when the turn ended
/// or was cancelled: the cell must not resume, and the worker — left
/// mid-turn on our side — is discarded like any abandoned protocol turn.
async fn hold(permit: Option<&CellPermit>) -> Result<Option<CellRunning<'_>>, PoolError> {
    match permit {
        None => Ok(None),
        Some(p) => p.run().await.map(Some).ok_or(PoolError::Protocol(
            "turn ended while the cell awaited its tool calls".into(),
        )),
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

fn first_arg<'a>(args: &'a CallArgs, name: &str) -> Option<ObjectRef<'a>> {
    args.arg(0).or_else(|| args.kwarg(name))
}

/// kwargs by name; positional args in signature order; unknown kwargs pass through.
fn merge_args(spec: &ToolSpec, args: &CallArgs) -> Value {
    let mut obj = serde_json::Map::new();
    for (name, v) in params(&spec.schema).iter().map(|p| p.name).zip(args.args()) {
        obj.insert(name.to_owned(), from_monty(v));
    }
    for (k, v) in args.kwargs() {
        let key = k.as_str().map_or_else(|| k.py_repr(), str::to_owned);
        obj.insert(key, from_monty(v));
    }
    Value::Object(obj)
}

/// 52 §7.1: replace each inner tool image path with a snapshot under `dir`,
/// taken now so a later inner call overwriting the same path (a second
/// screenshot) cannot change what this occurrence showed. Every occurrence
/// is kept — never deduplicated by path. A file that is not a bounded image
/// is dropped and named in the output, and the result becomes an error, so
/// neither the model nor the cell mistakes the omission for success; the
/// snapshots that did succeed still travel with it. Failure text carries the
/// file name and reason only — never bytes or the full path.
fn snapshot_files(out: &mut ToolOutput, dir: &Path) {
    let mut failed = String::new();
    out.files
        .retain_mut(|f| match tools::images::snapshot(f, dir) {
            Ok(p) => {
                *f = p;
                true
            }
            Err(e) => {
                failed.push_str(&format!("\nimage dropped: {e}"));
                false
            }
        });
    if !failed.is_empty() {
        out.content.push_str(&failed);
        out.is_error = true;
    }
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
            _ => MontyObject::string(out.content),
        },
    };
    ResumeValue::Return(value)
}

/// OS calls no mount covers: empty environment, else the call's own default
/// (`NotHandled`). Clocks are the worker's, in the host's zone (`os_policy`).
fn os_answer(function_name: &str, args: &CallArgs) -> ResumeValue {
    match function_name {
        "os.getenv" => {
            ResumeValue::Return(args.arg(1).map_or_else(MontyObject::none, |d| d.to_owned()))
        }
        "os.environ" => ResumeValue::Return(MontyObject::dict(Vec::new())),
        _ => ResumeValue::NotHandled,
    }
}

/// stdout + `→ repr` for a completed cell (uncapped stdout; capping happens
/// in [`render_capped`] once the `outN` name is known).
fn render_value(stdout: &str, v: &MontyObject) -> String {
    let repr = match v.type_name() {
        "NoneType" => None,
        _ => Some(cap(&v.py_repr(), |n| {
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

    /// The footer never outgrows [`FOOTER_CHARS`], whatever the namespace
    /// holds; the overflow is a count, and the hint is the caller's choice.
    #[test]
    fn footer_is_capped_regardless_of_namespace_size() {
        for n in [1usize, 5, 40, 600, 5000] {
            let names: BTreeMap<String, Slot> = (0..n)
                .map(|i| {
                    (
                        format!("w_result_{i}"),
                        Slot {
                            kind: "str".into(),
                            len: Some(30_000 + i),
                        },
                    )
                })
                .collect();
            let items = slot_items(names.iter());
            for hint in [true, false] {
                let line = footer(&items, hint).unwrap();
                assert!(
                    line.len() <= FOOTER_CHARS,
                    "{n}/{hint}: {} chars: {line}",
                    line.len()
                );
                assert_eq!(line.starts_with(FOOTER_HINT), hint, "{line}");
                assert!(line.contains("w_result_0 (str, 30000)"), "{line}");
                if n >= 600 {
                    assert!(line.contains(" more"), "{line}");
                } else if n == 1 {
                    assert!(!line.contains(" more"), "{line}");
                }
            }
        }
        assert_eq!(footer(&[], true), None);
        // one oversize item still yields a bounded line, not an empty prefix
        let huge = vec!["x".repeat(FOOTER_CHARS * 2)];
        let line = footer(&huge, false).unwrap();
        assert!(
            line.len() <= FOOTER_CHARS && line.ends_with("… +1 more"),
            "{line}"
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
    fn prelude_picks_used_unbound_imports() {
        let imports: BTreeMap<String, String> = import_lines(
            "import re\nimport collections as col\nfrom os import path, sep\nx = 1\nif x:\n    import json\n",
        )
        .collect();
        assert_eq!(
            imports.keys().map(String::as_str).collect::<Vec<_>>(),
            ["col", "path", "re", "sep"]
        );
        assert_eq!(imports["sep"], "from os import path, sep");
        let code = "col.Counter(re.findall('a', s))\nsep = '/'\npath";
        assert_eq!(
            prelude(&imports, code, &candidates(code)),
            "from os import path, sep\nimport collections as col\nimport re"
        );
        assert_eq!(
            prelude(&imports, "json.loads(x)", &candidates("json.loads(x)")),
            ""
        );
    }

    /// Captured from `import re` (prelude) + `x = 1\nre.compile(x)`: the
    /// model's lines shift, the prelude line vanishes, the stdlib snippet
    /// under `stdlib/re.pyi` keeps its numbers.
    #[test]
    fn rebase_lines_shifts_locations_and_gutters() {
        let diag = "error[no-matching-overload]: No overload of function `compile` matches arguments\n \
                    --> main.py:3:1\n  |\n1 | import re\n2 | x = 1\n3 | re.compile(x)\n  | ^^^^^^^^^^^^^\n\
                    info: First overload defined here\n   --> stdlib/re.pyi:261:1\n    |\n\
                    261 | / @overload\n262 | | def compile(pattern: AnyStr, flags: _FlagsType = 0) -> Pattern[AnyStr]: ...\n    |\n\
                    info: Possible overloads for function `compile`:\n";
        assert_eq!(
            rebase_lines(diag, 1),
            "error[no-matching-overload]: No overload of function `compile` matches arguments\n \
             --> main.py:2:1\n  |\n1 | x = 1\n2 | re.compile(x)\n  | ^^^^^^^^^^^^^\n\
             info: First overload defined here\n   --> stdlib/re.pyi:261:1\n    |\n\
             261 | / @overload\n262 | | def compile(pattern: AnyStr, flags: _FlagsType = 0) -> Pattern[AnyStr]: ...\n    |\n\
             info: Possible overloads for function `compile`:\n"
        );
        assert_eq!(rebase_lines(diag, 0), diag);
    }

    /// Captured diagnostics classify into the two retries — or neither.
    #[test]
    fn typing_retry_classification() {
        let names: BTreeMap<String, Slot> = ["m", "items", "out1"]
            .into_iter()
            .map(|n| {
                (
                    n.to_owned(),
                    Slot {
                        kind: "int".into(),
                        len: None,
                    },
                )
            })
            .collect();
        let src = "import re\nfor m in items:\n    pass\nm = 'x'";
        let assign = "error[invalid-assignment]: Object of type `int` is not assignable to `Match[str] | None`\n \
                      --> main.py:2:5\n  |\n2 | for m in items:\n  |     ^\n\
                      error[invalid-assignment]: Object of type `Literal[\"x\"]` is not assignable to `Match[str] | None`\n \
                      --> main.py:4:5\n  |\n4 | m = 'x'\n  |     ^^^\n";
        let errors = diag_errors(assign);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].0, "invalid-assignment");
        assert_eq!(
            redeclare_targets(&errors, src, &names),
            Some(BTreeSet::from(["m".to_owned()]))
        );
        assert!(!all_unresolved_held(&errors, &names));

        let unresolved = "error[unresolved-reference]: Name `out1` used when not defined\n \
                          --> main.py:1:5\n  |\n1 | len(out1)\n  |     ^^^^\n";
        let errors = diag_errors(unresolved);
        assert!(all_unresolved_held(&errors, &names));
        assert_eq!(redeclare_targets(&errors, "len(out1)", &names), None);
        let typo = unresolved.replace("out1", "nope");
        assert!(!all_unresolved_held(&diag_errors(&typo), &names));

        // follow-on errors ride along with the rebinding (retry is attempted,
        // a failure returns the original); an assignment to a name the heap
        // lacks is the model's own error
        let mixed = format!("{assign}{unresolved}");
        assert_eq!(
            redeclare_targets(&diag_errors(&mixed), src, &names),
            Some(BTreeSet::from(["m".to_owned()]))
        );
        assert!(!all_unresolved_held(&diag_errors(&mixed), &names));
        let fresh = "error[invalid-assignment]: Object of type `int` is not assignable to `str`\n \
                     --> main.py:1:1\n  |\n1 | z: str = 1\n  | ^\n";
        assert_eq!(
            redeclare_targets(&diag_errors(fresh), "z: str = 1", &names),
            None
        );
    }

    #[test]
    fn json_round_trip() {
        let v = serde_json::json!({"a": [1, 2.5, null, true, "s"], "b": {"c": {}}});
        assert_eq!(from_monty(to_monty(&v).as_ref()), v);
        let d = MontyObject::dict([(MontyObject::int(1), MontyObject::string("a"))]);
        assert_eq!(from_monty(d.as_ref()), serde_json::json!({"1": "a"}));
    }

    /// A pool over the `monty` binary on `PATH` (the test runner is not a
    /// worker); `None`, with a note, when absent so the test skips.
    async fn monty_pool() -> Option<Arc<Pool>> {
        let monty = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("monty"))
                .find(|p| p.is_file())
        });
        let Some(monty) = monty else {
            eprintln!("monty not on PATH; skipping");
            return None;
        };
        let pool = open_pool(&monty, 1).await;
        if pool.is_none() {
            eprintln!("monty pool failed to start; skipping");
        }
        pool
    }

    async fn run_cx(dir: &Path) -> RunCx {
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.to_path_buf());
        let registry = tools::registry::start(&rt).await.unwrap();
        RunCx {
            session: SessionId::new(),
            turn: TurnId::new(),
            rt,
            registry,
            spill_dir: dir.join("spill"),
            cwd: dir.to_path_buf(),
            permit: None,
        }
    }

    /// Drives a real checkout through `monty`; skipped when absent.
    #[test]
    fn capping_binds_out_name() {
        crate::with_test_registry(capping_scenario());
    }

    async fn capping_scenario() {
        let Some(pool) = monty_pool().await else {
            return;
        };
        let dir = tempdir();
        std::fs::write(dir.join("a.txt"), "hello mount").unwrap();
        let cx = run_cx(&dir).await;
        let session = cx.session;
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
                "line 2999\nbindings (reuse, do not re-read; bindings() lists all): n (int) · out1 (str, 30000)"
            ),
            "{}",
            out.content
        );
        // bound as a global, visible to the type checker, survives dump/restore
        let out = repl.run("c2", "len(out1)").await;
        assert_eq!(out.content, "→ 30000", "{}", out.content);
        let out = repl.run("c3", "await nope(1)").await;
        assert!(
            out.is_error
                && out.content.starts_with(
                    "type check failed (nothing ran; your variables are intact — resend the cell with the fix):\n"
                ),
            "{}",
            out.content
        );
        let out = repl.run("c4", "r = [1, 2, 3]\nr").await;
        // second footer of the turn: bare, and only the names this cell touched
        assert_eq!(out.content, "→ [1, 2, 3]\nbindings: r (list, 3)");
        let out = repl.run("c4a", "bindings()").await;
        assert_eq!(
            out.content, "→ ['n (int)', 'out1 (str, 30000)', 'r (list, 3)']",
            "{}",
            out.content
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
        // c7's `import asyncio` bound a name on the way to its error, so the
        // reopened Repl's one hint went out there; c8 is bare.
        assert!(
            out.content
                .starts_with("→ ('str', 'read_file:')\nbindings: ls (str, "),
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
        let mut repl = Repl::open(&pool, session, &dir, cx.clone()).await;
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
        // Same process-wide registry (`start` registers a global actor name),
        // fresh session and data dir.
        let dir = tempdir();
        inner_images_survive_overwrite_and_errors(
            &pool,
            &dir,
            RunCx {
                session: SessionId::new(),
                spill_dir: dir.join("spill"),
                cwd: dir.clone(),
                ..cx
            },
        )
        .await;
        pool.close().await;
    }

    /// A screenshot-like tool: writes a 1×1 grayscale image whose pixel is
    /// `v` to `<dir>/<name>.png` (`shot.png` by default, so successive calls
    /// overwrite the same path like a real screenshot tool). With `junk` it
    /// also lists a text file wearing `.png` and a path that does not exist.
    struct Shot(PathBuf);

    #[derive(Deserialize, JsonSchema)]
    struct ShotArgs {
        v: u8,
        name: Option<String>,
        junk: Option<bool>,
    }

    #[async_trait::async_trait]
    impl tools::Tool for Shot {
        const NAME: &'static str = "shot";
        const RISK: Risk = Risk::ReadOnly;
        type Args = ShotArgs;
        fn description(&self) -> String {
            "Take a screenshot.".into()
        }
        async fn call(&self, a: ShotArgs, _: tools::ToolCx) -> anyhow::Result<ToolOutput> {
            let mut bytes = Vec::new();
            let mut enc = png::Encoder::new(&mut bytes, 1, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header()?;
            w.write_image_data(&[a.v])?;
            w.finish()?;
            let path = self
                .0
                .join(a.name.as_deref().unwrap_or("shot"))
                .with_extension("png");
            std::fs::write(&path, bytes)?;
            let mut files = vec![path];
            if a.junk.unwrap_or(false) {
                let junk = self.0.join("junk.png");
                std::fs::write(&junk, b"not a png")?;
                files.push(junk);
                files.push(self.0.join("missing.png"));
            }
            Ok(ToolOutput {
                files,
                ..ToolOutput::ok("shot")
            })
        }
    }

    fn pixel(p: &Path) -> u8 {
        let mut r = png::Decoder::new(std::fs::File::open(p).unwrap())
            .read_info()
            .unwrap();
        let mut px = vec![0u8; r.output_buffer_size()];
        r.next_frame(&mut px).unwrap();
        px[0]
    }

    /// 52 §7.1: inner images reach the cell result as snapshots taken when
    /// each inner result arrived, in call order, even when the same path was
    /// overwritten and even when the cell later raises; nothing carries over.
    async fn inner_images_survive_overwrite_and_errors(pool: &Arc<Pool>, dir: &Path, cx: RunCx) {
        let shot = dir.join("shot.png");
        cx.registry
            .ask(tools::registry::RegisterTool(Arc::new(Shot(
                dir.to_path_buf(),
            ))))
            .await
            .unwrap();
        let spill = dir.join("spill").join(cx.session.to_string());
        let mut repl = Repl::open(pool, cx.session, dir, cx).await;

        // Sequential overwrite: both occurrences survive, in order, as copies.
        let out = repl
            .run("c1", "a = await shot(v=1)\nb = await shot(v=2)\n(a, b)")
            .await;
        assert!(
            out.content.starts_with("→ ('shot', 'shot')"),
            "{}",
            out.content
        );
        assert_eq!(out.files.len(), 2, "{:?}", out.files);
        assert!(
            out.files
                .iter()
                .all(|f| f != &shot && f.starts_with(&spill)),
            "{:?}",
            out.files
        );
        assert_eq!([pixel(&out.files[0]), pixel(&out.files[1])], [1, 2]);
        assert_eq!(pixel(&shot), 2);

        // Gathered to distinct paths: gather input order, not completion order.
        let out = repl
            .run("c2", "import asyncio\nr = await asyncio.gather(shot(v=3, name='x'), shot(v=4, name='y'), shot(v=5, name='z'))")
            .await;
        assert!(!out.is_error, "{}", out.content);
        let px: Vec<u8> = out.files.iter().map(|f| pixel(f)).collect();
        assert_eq!(px, [3, 4, 5]);

        // A cell that raises after its screenshots still returns them.
        let out = repl
            .run("c3", "await shot(v=6)\nawait shot(v=7)\n1/0")
            .await;
        assert!(
            out.is_error && out.content.ends_with("ZeroDivisionError: division by zero"),
            "{}",
            out.content
        );
        let px: Vec<u8> = out.files.iter().map(|f| pixel(f)).collect();
        assert_eq!(px, [6, 7]);

        // A file that is not an image fails that inner call visibly — the
        // cell sees the exception, its text names the file, not the path —
        // while the good snapshot from the same result and later ones stay.
        let out = repl
            .run(
                "c4",
                "try:\n    await shot(v=8, junk=True)\nexcept RuntimeError as e:\n    err = str(e)\nawait shot(v=9)\nerr",
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("junk.png") && out.content.contains("missing.png"),
            "{}",
            out.content
        );
        assert!(
            !out.content.contains(dir.to_str().unwrap()),
            "{}",
            out.content
        );
        let px: Vec<u8> = out.files.iter().map(|f| pixel(f)).collect();
        assert_eq!(px, [8, 9]);

        // The next cell starts empty: no inheritance through the probe or the runtime.
        let out = repl.run("c5", "1 + 1").await;
        assert_eq!(out.content, "→ 2");
        assert!(out.files.is_empty(), "{:?}", out.files);
        repl.close().await;
    }

    /// Every `AgentEvent` on the bus, in order.
    #[derive(kameo::Actor)]
    struct Sink(tokio::sync::mpsc::UnboundedSender<AgentEvent>);

    impl kameo::message::Message<AgentEvent> for Sink {
        type Reply = ();
        async fn handle(&mut self, e: AgentEvent, _: &mut kameo::message::Context<Self, ()>) {
            let _ = self.0.send(e);
        }
    }

    /// A cell that creates tool futures and raises before awaiting them
    /// (monty's `gather` rejects keyword arguments after building the
    /// futures) finishes each started call as an error without running it,
    /// and the traceback carries the gather hint; skipped without `monty`.
    #[test]
    fn unawaited_inner_calls_are_finished_not_run() {
        crate::with_test_registry(async {
            let Some(pool) = monty_pool().await else {
                return;
            };
            let dir = tempdir();
            let cx = run_cx(&dir).await;
            cx.registry
                .ask(tools::registry::RegisterTool(Arc::new(Shot(dir.clone()))))
                .await
                .unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let sink =
                kameo::actor::Spawn::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
            cx.rt.subscribe::<Sink, AgentEvent>(&sink).await;
            let mut repl = Repl::open(&pool, cx.session, &dir, cx.clone()).await;
            let out = repl
                .run(
                    "c1",
                    "import asyncio\nr = await asyncio.gather(shot(v=1), shot(v=2), return_exceptions=True)",
                )
                .await;
            assert!(
                out.is_error
                    && out.content.contains(
                        "NotImplementedError: gather() does not yet support keyword arguments\n\
                         hint: monty's asyncio.gather takes positional awaitables only (no return_exceptions=); \
                         wrap each call in a small async helper that catches its exception instead. \
                         The 2 tool calls created before the error were not run.\n"
                    ),
                "{}",
                out.content
            );
            assert!(!dir.join("shot.png").exists(), "tool ran");
            let mut seen = Vec::new();
            for _ in 0..4 {
                let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .expect("event")
                    .unwrap();
                seen.push(match ev {
                    AgentEvent::ToolCallStarted { call_id, .. } => (call_id, None),
                    AgentEvent::ToolCallFinished {
                        call_id,
                        output,
                        is_error,
                        files,
                        ..
                    } => {
                        assert!(is_error && files.is_empty());
                        (call_id, Some(output))
                    }
                    other => panic!("{other:?}"),
                });
            }
            let not_run = Some(NOT_AWAITED.to_owned());
            assert_eq!(
                seen,
                [
                    ("c1.1".to_owned(), None),
                    ("c1.2".to_owned(), None),
                    ("c1.1".to_owned(), not_run.clone()),
                    ("c1.2".to_owned(), not_run),
                ]
            );
            tokio::task::yield_now().await;
            assert!(rx.try_recv().is_err(), "extra event");
            repl.close().await;
        });
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("theuth-repl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A module imported in one cell is known to the checker in the next
    /// (the prelude), also after close/reopen; skipped without `monty`.
    #[test]
    fn imports_carry_across_cells() {
        crate::with_test_registry(async {
            let Some(pool) = monty_pool().await else {
                return;
            };
            let dir = tempdir();
            let cx = run_cx(&dir).await;
            let mut repl = Repl::open(&pool, cx.session, &dir, cx.clone()).await;
            let out = repl.run("c1", "import re\nre.compile('a').pattern").await;
            assert_eq!(
                out.content,
                "→ 'a'\nbindings (reuse, do not re-read; bindings() lists all): re (module)"
            );
            let out = repl.run("c2", "re.compile('b').pattern").await;
            assert_eq!(out.content, "→ 'b'", "{}", out.content);
            // a diagnostic in the model's own line numbers, not the prelude's
            let out = repl.run("c3", "x = 1\nre.compile(x)").await;
            assert!(
                out.is_error
                    && out.content.contains("main.py:2:1\n")
                    && out.content.contains("\n1 | x = 1\n")
                    && out.content.contains("\n262 | | def compile("),
                "{}",
                out.content
            );
            repl.close().await;
            let mut repl = Repl::open(&pool, cx.session, &dir, cx.clone()).await;
            let out = repl.run("c4", "re.compile('c').pattern").await;
            assert_eq!(out.content, "→ 'c'", "{}", out.content);
            repl.close().await;
        });
    }

    /// A persisted name can be rebound to another type (redeclare retry),
    /// and a name the heap holds but the checker never saw (bound by a cell
    /// that then raised) is usable (skip retry); skipped without `monty`.
    #[test]
    fn persisted_names_rebind_and_resolve() {
        crate::with_test_registry(async {
            let Some(pool) = monty_pool().await else {
                return;
            };
            let dir = tempdir();
            let cx = run_cx(&dir).await;
            let mut repl = Repl::open(&pool, cx.session, &dir, cx.clone()).await;
            let out = repl.run("c1", "import re\nm = re.match('a', 'a')").await;
            assert!(!out.is_error, "{}", out.content);
            // `for` targets are not candidates, but the probe sees `m`'s type
            // move (Match → int): a changed slot is footered, without the hint
            // the first cell already carried.
            let out = repl.run("c2", "for m in [1, 2]:\n    pass\nm + 1").await;
            assert_eq!(out.content, "→ 3\nbindings: m (int)", "{}", out.content);
            // the checker follows: later cells see the new type, no `_Any` leaks
            let out = repl.run("c3", "m = 'x'\nm * 2").await;
            assert_eq!(
                out.content, "→ 'xx'\nbindings: m (str, 1)",
                "{}",
                out.content
            );
            // a genuine error on a persisted name is still an error (the
            // checker's if it still types `m`, else the runtime's), never a run
            let out = repl.run("c4", "m + 1").await;
            assert!(
                out.is_error
                    && (out.content.contains("error[unsupported-operator]")
                        || out.content.contains("TypeError")),
                "{}",
                out.content
            );
            // bound, then the cell raised: in the heap, unknown to the checker
            let out = repl.run("c5", "k = 5\n1/0").await;
            assert!(out.is_error, "{}", out.content);
            let out = repl.run("c6", "k + 1").await;
            assert_eq!(out.content, "→ 6", "{}", out.content);
            repl.close().await;
        });
    }

    /// `time.sleep`/`asyncio.sleep` reach the host as `system_sleep` and are
    /// waited out (beta.2 raised `RuntimeError: 'system.sleep' is not
    /// supported` while `drive` answered `NotHandled`); skipped without `monty`.
    #[test]
    fn sleeps_are_waited_host_side() {
        crate::with_test_registry(async {
            let Some(pool) = monty_pool().await else {
                return;
            };
            let dir = tempdir();
            let cx = run_cx(&dir).await;
            let mut repl = Repl::open(&pool, cx.session, &dir, cx.clone()).await;
            let start = std::time::Instant::now();
            let out = repl
                .run(
                    "c1",
                    "import time, asyncio\nt = time.time()\ntime.sleep(0.2)\nawait asyncio.sleep(0.2)\ntime.time() - t >= 0.4",
                )
                .await;
            assert!(out.content.starts_with("→ True"), "{}", out.content);
            assert!(start.elapsed() >= Duration::from_millis(400));
            repl.close().await;
        });
    }
}
