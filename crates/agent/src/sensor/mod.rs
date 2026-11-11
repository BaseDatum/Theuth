//! Sensor runtime (design doc 40 §4): the Python `check` / `probe` a `when:`
//! schedule file names, run under the sensor profile in Monty ([`bridge`])
//! or through the CPython shim ([`cpython`]). The schedule crate's gate and
//! validation call [`SensorRunner`].

mod bridge;
mod cpython;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use monty_pool::Pool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tools::ToolRegistry;

/// Lines of a sensor file the header may occupy (40 §4.1).
const HEADER_LINES: usize = 16;
/// The shipped sensors and the shim, written by [`SensorRunner::install_builtins`].
const BUILTINS: &[(&str, &str)] = &[
    (
        "fs_exists.py",
        include_str!("../../../../assets/sensors/fs_exists.py"),
    ),
    (
        "pr_watch.py",
        include_str!("../../../../assets/sensors/pr_watch.py"),
    ),
    ("rss.py", include_str!("../../../../assets/sensors/rss.py")),
    (
        ".shim.py",
        include_str!("../../../../assets/sensors/.shim.py"),
    ),
];
/// The 40 §5 payload marker; `N` is the byte count of the original.
const TRUNCATED_MARKER: &str = "… [truncated, ";

/// Directory under `data_dir` holding the sensors (40 §4.5).
pub const DIR: &str = "sensors";

/// One `# arg:` header line (108 §9.1): what the form and the skill show for an argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SensorArg {
    /// `[a-z][a-z0-9_]*`; never `cwd`.
    pub name: String,
    /// `required` / `optional`.
    pub required: bool,
    /// The placeholder token; `None` when the line said `-`.
    pub example: Option<String>,
    /// Free text after ` — ` / ` -- `; may be empty.
    pub help: String,
}

/// What the header of a sensor file says (40 §4.1: the first 16 lines).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorMeta {
    /// File name, e.g. `pr_watch.py`.
    pub name: String,
    /// `<data_dir>/sensors/<name>`.
    pub path: PathBuf,
    /// `# theuth-sensor: <name> <version>` — built-ins only (40 §4.5).
    pub version: Option<String>,
    /// `# needs: cpython` — runs through the shim, not Monty (40 §1.3).
    pub needs_cpython: bool,
    /// `# hosts: a,b` — hosts `http_request` may POST to (40 §4.3).
    pub hosts: Vec<String>,
    /// The file defines `async def probe(args)` (40 §1.12).
    pub has_probe: bool,
    /// `# arg:` lines in header order (108 §9.1).
    pub args: Vec<SensorArg>,
    /// First non-empty line of the module docstring.
    pub description: Option<String>,
    /// The docstring line starting `when:`.
    pub usage: Option<String>,
}

/// One `check` result, validated (40 §1.4, §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorRun {
    /// The condition holds.
    pub fire: bool,
    /// ≤ 4 KiB; first line is the envelope (40 §1.5).
    pub payload: String,
    /// What the sensor wants back next time (≤ 16 KiB as JSON).
    pub state: serde_json::Value,
    /// Seconds until the next check the sensor asks for; the gate clamps it.
    pub next: Option<u64>,
    /// Sensor wall clock.
    pub elapsed: Duration,
    /// The payload was cut at 4 KiB.
    pub truncated: bool,
}

/// Why a check did not produce a [`SensorRun`]. `Display` is the reason text
/// the gate writes after `check broken: ` (40 §3.1) or the tool replies with
/// at creation (40 §1.3, §4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SensorError {
    /// No such file in `sensors/`.
    NotFound(String),
    /// `# needs: cpython` and no Python on `PATH`.
    NeedsCpython,
    /// The header could not be read (`# hosts:` malformed, file unreadable, not UTF-8).
    Header(String),
    /// The sensor did not return within the wall clock.
    Timeout(Duration),
    /// A §5 ceiling other than time: heap, tool calls, bytes fetched, a disallowed tool.
    Budget(String),
    /// The result was not `{fire, payload, state, next}` of the right types.
    Malformed(String),
    /// `state` serialised past 16 KiB; bytes.
    StateTooLarge(usize),
    /// The sensor raised, or the CPython child exited non-zero: `<exception>` / `exit 3: <tail>`.
    Failed(String),
    /// The runtime itself is not usable (no pool, no shim, not built).
    Unavailable(String),
}

impl fmt::Display for SensorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "when: no such sensor: {name}"),
            Self::NeedsCpython => f.write_str("when: needs cpython, none found"),
            Self::Header(why) => write!(f, "sensor header: {why}"),
            Self::Timeout(d) => write!(f, "timeout after {}s", d.as_secs()),
            Self::Budget(why) => write!(f, "over budget: {why}"),
            Self::Malformed(why) => write!(f, "malformed result: {why}"),
            Self::StateTooLarge(bytes) => {
                write!(f, "state too large: {} KiB", bytes.div_ceil(1024))
            }
            Self::Failed(why) | Self::Unavailable(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for SensorError {}

/// The sensor profile (40 §4.2, §5): tighter than `repl.rs`'s main runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorProfile {
    /// Heap; 64 MiB (main runtime: [`crate::repl::MAX_MEMORY`]).
    pub max_memory: usize,
    /// Monty suspensions per evaluation; 16 = the 8-tool-call cap (40 §5): an
    /// awaited call is two suspensions (`FunctionCall` + `ResolveFutures`).
    pub max_suspensions: usize,
    /// Cumulative sleep; 0 — a sensor waits through `next`, not `sleep`.
    pub max_total_sleep: Duration,
    /// Bytes fetched across `http_request` responses per evaluation; 1 MiB.
    pub max_fetch_bytes: usize,
    /// Payload cap; 4 KiB, truncated past it.
    pub max_payload: usize,
    /// State cap; 16 KiB, rejected past it.
    pub max_state: usize,
    /// Sensors running at once, harness-wide; 4.
    pub max_concurrent: usize,
    /// `web.allow_private_network`: a sensor's `http_request` may reach the
    /// local network (sensors are the owner's); `true` like the web default.
    pub allow_private_network: bool,
}

impl Default for SensorProfile {
    fn default() -> Self {
        Self {
            max_memory: 64 << 20,
            max_suspensions: 16,
            max_total_sleep: Duration::ZERO,
            max_fetch_bytes: 1 << 20,
            max_payload: 4 << 10,
            max_state: 16 << 10,
            max_concurrent: 4,
            allow_private_network: true,
        }
    }
}

impl SensorProfile {
    /// The default profile under `rt`'s `web.allow_private_network`.
    pub fn for_runtime(rt: &runtime::Runtime) -> Self {
        let web: web::WebConfig = rt.section("web").unwrap_or_default();
        Self {
            allow_private_network: web.allow_private_network,
            ..Self::default()
        }
    }
}

/// Runs sensors: resolves the file, picks Monty or the CPython shim (40 §4.1),
/// enforces the profile. One per process, owned by the schedule plugin;
/// shared as `Arc<SensorRunner>`.
pub struct SensorRunner {
    data_dir: PathBuf,
    /// The sensor-profile pool (40 §4.2); `None` = Monty sensors are `Unavailable`.
    pool: Option<Arc<Pool>>,
    /// The registry handle the plugin hands over. Unused by the bridge, which
    /// calls the tool structs directly (40 §4.3); kept so a later reader of the
    /// registry (a tool's spec for `/schedules test`) has it.
    tools: Option<ActorRef<ToolRegistry>>,
    profile: SensorProfile,
    /// `profile.max_concurrent` permits (40 §1.9: a deferred tick is deferred, not dropped).
    slots: Arc<Semaphore>,
}

impl SensorRunner {
    /// `data_dir` is the harness data dir; `sensors_dir()` is derived.
    pub fn new(
        data_dir: PathBuf,
        pool: Option<Arc<Pool>>,
        tools: Option<ActorRef<ToolRegistry>>,
        profile: SensorProfile,
    ) -> Self {
        let slots = Arc::new(Semaphore::new(profile.max_concurrent.max(1)));
        Self {
            data_dir,
            pool,
            tools,
            profile,
            slots,
        }
    }

    /// `<data_dir>/sensors`.
    pub fn sensors_dir(&self) -> PathBuf {
        self.data_dir.join(DIR)
    }

    /// The profile in force.
    pub fn profile(&self) -> &SensorProfile {
        &self.profile
    }

    /// The pool, when one was opened.
    pub fn pool(&self) -> Option<&Arc<Pool>> {
        self.pool.as_ref()
    }

    /// The registry handle, once known.
    pub fn tools(&self) -> Option<&ActorRef<ToolRegistry>> {
        self.tools.as_ref()
    }

    /// Concurrency permits (`max_concurrent`); the gate tries to acquire one
    /// before a check and defers the tick when none is free.
    pub fn slots(&self) -> &Arc<Semaphore> {
        &self.slots
    }

    /// Find `file` in `sensors_dir()` and read its header (40 §4.1).
    /// `Err(NotFound)` when absent; `Err(NeedsCpython)` when the header
    /// declares it and [`find_cpython`] finds none.
    pub fn resolve(&self, file: &str) -> Result<SensorMeta, SensorError> {
        let path = self.sensors_dir().join(file);
        if file.is_empty()
            || file.starts_with('.')
            || !file.ends_with(".py")
            || file.contains(['/', '\\'])
            || !path.is_file()
        {
            return Err(SensorError::NotFound(file.to_owned()));
        }
        let src = std::fs::read_to_string(&path)
            .map_err(|e| SensorError::Header(format!("{file}: {e}")))?;
        let (description, usage) = docstring(&src);
        let mut meta = SensorMeta {
            name: file.to_owned(),
            path,
            version: None,
            needs_cpython: false,
            hosts: Vec::new(),
            has_probe: src.lines().any(|l| l.starts_with("async def probe(")),
            args: Vec::new(),
            description,
            usage,
        };
        let bad = |what: &str| SensorError::Header(format!("{file}: {what}"));
        for line in src.lines().take(HEADER_LINES) {
            let Some((key, value)) = line.strip_prefix('#').and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "theuth-sensor" => match header_version(line) {
                    Some(v) => meta.version = Some(v.to_owned()),
                    None => {
                        return Err(bad(&format!(
                            "`# theuth-sensor:` wants `<name> <version>`, got `{value}`"
                        )));
                    }
                },
                "needs" => match value {
                    "cpython" => meta.needs_cpython = true,
                    other => {
                        return Err(bad(&format!("`# needs: {other}`: only `cpython` is known")));
                    }
                },
                "hosts" => {
                    meta.hosts = value
                        .split([',', ' '])
                        .filter(|h| !h.is_empty())
                        .map(str::to_ascii_lowercase)
                        .collect();
                    if meta.hosts.is_empty() || meta.hosts.iter().any(|h| h.contains(['/', ':'])) {
                        return Err(bad(&format!(
                            "`# hosts:` wants a comma list of host names, got `{value}`"
                        )));
                    }
                }
                "arg" => match parse_arg(value) {
                    Some(a) if !meta.args.iter().any(|x| x.name == a.name) => meta.args.push(a),
                    Some(a) => return Err(bad(&format!("`# arg: {}` repeated", a.name))),
                    None => {
                        return Err(bad(&format!(
                            "`# arg:` wants `<name> <required|optional> <example> — <help>`, got `{value}`"
                        )));
                    }
                },
                _ => {}
            }
        }
        if meta.needs_cpython && find_cpython().is_none() {
            return Err(SensorError::NeedsCpython);
        }
        Ok(meta)
    }

    /// Run `check(state, args)` under the profile within `timeout` (40 §1.4,
    /// §4). `args["cwd"]` is set from `cwd`. Takes one of [`Self::slots`],
    /// waiting ≤ 1 s; `Err(Budget("deferred"))` when none frees (40 §3.1).
    pub async fn check(
        &self,
        meta: &SensorMeta,
        args: &BTreeMap<String, String>,
        state: &Value,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<SensorRun, SensorError> {
        let started = Instant::now();
        let _slot = self.slot().await?;
        let raw = self
            .call(meta, "check", Some(state), args, cwd, timeout)
            .await?;
        let mut run = decode(raw, &self.profile)?;
        run.elapsed = started.elapsed();
        Ok(run)
    }

    /// Run the optional `probe(args) -> str | None` (40 §1.12): a reachability
    /// note (`Ok(Some(text))`), nothing to say (`Ok(None)`), or the same errors
    /// as [`Self::check`]. `Ok(None)` when `meta.has_probe` is false.
    pub async fn probe(
        &self,
        meta: &SensorMeta,
        args: &BTreeMap<String, String>,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<Option<String>, SensorError> {
        if !meta.has_probe {
            return Ok(None);
        }
        let _slot = self.slot().await?;
        match self.call(meta, "probe", None, args, cwd, timeout).await? {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s)),
            other => Err(SensorError::Malformed(format!(
                "probe: expected str | None, got {}",
                kind(&other)
            ))),
        }
    }

    async fn slot(&self) -> Result<tokio::sync::SemaphorePermit<'_>, SensorError> {
        tokio::time::timeout(Duration::from_secs(1), self.slots.acquire())
            .await
            .map_err(|_| SensorError::Budget("deferred".into()))?
            .map_err(|_| SensorError::Unavailable("sensor slots closed".into()))
    }

    /// The ladder (40 §4.1): the shim when the header says `cpython`, Monty otherwise.
    async fn call(
        &self,
        meta: &SensorMeta,
        fn_name: &str,
        state: Option<&Value>,
        args: &BTreeMap<String, String>,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<Value, SensorError> {
        let mut args = args.clone();
        if let Some(cwd) = cwd {
            args.insert("cwd".into(), cwd.display().to_string());
        }
        let call = Call {
            meta,
            fn_name,
            state,
            args: &args,
            cwd,
            timeout,
        };
        if meta.needs_cpython {
            // 40 §4.4: nothing secret reaches a child Python in v1.
            if args.values().any(|v| v.contains("{{secret:")) {
                return Err(SensorError::Failed(
                    "cpython sensors cannot use secrets in v1".into(),
                ));
            }
            let python = find_cpython().ok_or(SensorError::NeedsCpython)?;
            let shim = self.sensors_dir().join(".shim.py");
            if !shim.is_file() {
                return Err(SensorError::Unavailable(
                    "sensor runtime: .shim.py missing (install_builtins)".into(),
                ));
            }
            return cpython::run(&python, &shim, &call).await;
        }
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| SensorError::Unavailable("sensor runtime: no worker pool".into()))?;
        bridge::run(pool, &self.profile, &self.data_dir, &call).await
    }

    /// Write the built-in sensors and `.shim.py` into `sensors_dir()` per
    /// 40 §4.5's version rule; returns the names written. Creates the directory.
    ///
    /// A `.py` sensor is written when missing, or when the shipped version is
    /// newer *and* the file's sha256 is the one `sensors.json` recorded at the
    /// last install — an edited copy is left alone (logged). The dotfile shim
    /// is harness-owned and rewritten whenever its text differs (the skills rule).
    pub fn install_builtins(&self) -> std::io::Result<Vec<String>> {
        let dir = self.sensors_dir();
        std::fs::create_dir_all(&dir)?;
        let manifest_path = dir.join("sensors.json");
        let mut manifest: BTreeMap<String, Installed> = std::fs::read(&manifest_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut written = Vec::new();
        for (name, body) in BUILTINS {
            let path = dir.join(name);
            let shipped = Installed {
                version: header_version(body).unwrap_or("0").to_owned(),
                sha256: payload_hash(body),
            };
            let on_disk = std::fs::read_to_string(&path)
                .ok()
                .map(|s| payload_hash(&s));
            let write = match &on_disk {
                None => true,
                Some(sha) if *sha == shipped.sha256 => false,
                Some(_) if name.starts_with('.') => true,
                Some(sha) => match manifest.get(*name) {
                    Some(last) if last.sha256 == *sha && newer(&shipped.version, &last.version) => {
                        true
                    }
                    _ => {
                        tracing::info!(
                            "sensor: {name} (edited; shipped {} available — delete or rename the copy to take it)",
                            shipped.version
                        );
                        continue;
                    }
                },
            };
            if write {
                std::fs::write(&path, body)?;
                written.push((*name).to_owned());
            }
            manifest.insert((*name).to_owned(), shipped);
        }
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
        )?;
        Ok(written)
    }

    /// The binary's `# theuth-sensor:` version of a built-in; `None` for a user file.
    pub fn shipped_version(name: &str) -> Option<&'static str> {
        BUILTINS
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, body)| header_version(body))
    }

    /// A built-in whose on-disk text is neither the shipped copy nor the one
    /// `sensors.json` recorded at the last install — [`Self::install_builtins`]'
    /// "edited" rule (40 §4.5). `false` for a user file or an unreadable one.
    pub fn edited(&self, name: &str) -> bool {
        let Some((_, body)) = BUILTINS.iter().find(|(n, _)| *n == name) else {
            return false;
        };
        let dir = self.sensors_dir();
        let Ok(on_disk) = std::fs::read_to_string(dir.join(name)) else {
            return false;
        };
        let sha = payload_hash(&on_disk);
        if sha == payload_hash(body) {
            return false;
        }
        let manifest: BTreeMap<String, Installed> = std::fs::read(dir.join("sensors.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        manifest.get(name).is_none_or(|last| last.sha256 != sha)
    }
}

/// `sensors.json` row: what the last install wrote for a built-in (40 §4.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Installed {
    version: String,
    sha256: String,
}

/// One evaluation's request, shared by both ladder rungs.
#[derive(Clone, Copy)]
struct Call<'a> {
    meta: &'a SensorMeta,
    /// `check` or `probe`.
    fn_name: &'a str,
    /// `check` only.
    state: Option<&'a Value>,
    /// The `when:` args plus `cwd`.
    args: &'a BTreeMap<String, String>,
    cwd: Option<&'a Path>,
    timeout: Duration,
}

/// `# theuth-sensor: <name> <version>` → the version, from a line or a whole file.
fn header_version(text: &str) -> Option<&str> {
    text.lines().take(HEADER_LINES).find_map(|l| {
        let rest = l
            .strip_prefix('#')?
            .trim_start()
            .strip_prefix("theuth-sensor:")?;
        let mut it = rest.split_whitespace();
        match (it.next(), it.next(), it.next()) {
            (Some(_), Some(v), None) if v.parse::<u32>().is_ok() => Some(v),
            _ => None,
        }
    })
}

/// `<name> <required|optional> <example> [— <help>]` (108 §9.1), the text after
/// `# arg:`. `None` when the three tokens are not there, the name is not a
/// `when:` key, or the mode is neither word. ` -- ` is accepted for ` — `.
fn parse_arg(value: &str) -> Option<SensorArg> {
    let (head, help) = value
        .split_once(" — ")
        .or_else(|| value.split_once(" -- "))
        .map_or((value, ""), |(h, t)| (h, t.trim()));
    let mut it = head.split_whitespace();
    let (name, mode, example) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some()
        || name == "cwd"
        || !name.starts_with(|c: char| c.is_ascii_lowercase())
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let required = match mode {
        "required" => true,
        "optional" => false,
        _ => return None,
    };
    Some(SensorArg {
        name: name.to_owned(),
        required,
        example: (example != "-").then(|| example.to_owned()),
        help: help.to_owned(),
    })
}

/// The module docstring's first non-empty line and its `when:` line (108 §9.2).
fn docstring(src: &str) -> (Option<String>, Option<String>) {
    let mut lines = src.lines().map(str::trim);
    let first = lines.find_map(|l| l.strip_prefix("\"\"\""));
    let Some(first) = first else {
        return (None, None);
    };
    let mut description = None;
    let mut usage = None;
    let mut body = std::iter::once(first).chain(lines);
    for line in body.by_ref() {
        let (line, last) = match line.split_once("\"\"\"") {
            Some((l, _)) => (l.trim(), true),
            None => (line, false),
        };
        if description.is_none() && !line.is_empty() {
            description = Some(line.to_owned());
        }
        if usage.is_none() && line.starts_with("when:") {
            usage = Some(line.to_owned());
        }
        if last {
            break;
        }
    }
    (description, usage)
}

/// Numeric when both parse, else textual.
fn newer(a: &str, b: &str) -> bool {
    match (a.parse::<u32>(), b.parse::<u32>()) {
        (Ok(a), Ok(b)) => a > b,
        _ => a > b,
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "None",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Validate a `check` result (40 §1.4, §5): `fire` bool; `payload` str,
/// truncated at a char boundary past `max_payload`; `state` JSON ≤
/// `max_state`; `next` int ≥ 0 or None. `payload` and `state` may be absent
/// (`{"fire": false, "next": 300}` is a legal quiet result — 40 §8.2 step 3);
/// an absent `state` is `{}`.
fn decode(raw: Value, profile: &SensorProfile) -> Result<SensorRun, SensorError> {
    let Value::Object(mut o) = raw else {
        return Err(SensorError::Malformed(format!(
            "expected a dict, got {}",
            kind(&raw)
        )));
    };
    let fire = match o.remove("fire") {
        Some(Value::Bool(b)) => b,
        other => {
            return Err(SensorError::Malformed(format!(
                "fire: expected bool, got {}",
                other.as_ref().map_or("nothing", kind)
            )));
        }
    };
    let mut payload = match o.remove("payload") {
        Some(Value::String(s)) => s,
        None | Some(Value::Null) => String::new(),
        Some(other) => {
            return Err(SensorError::Malformed(format!(
                "payload: expected str, got {}",
                kind(&other)
            )));
        }
    };
    let state = match o.remove("state") {
        Some(v @ Value::Object(_)) => v,
        None | Some(Value::Null) => Value::Object(Default::default()),
        Some(other) => {
            return Err(SensorError::Malformed(format!(
                "state: expected dict, got {}",
                kind(&other)
            )));
        }
    };
    let state_len = state.to_string().len();
    if state_len > profile.max_state {
        return Err(SensorError::StateTooLarge(state_len));
    }
    let next = match o.remove("next") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => match n.as_u64() {
            Some(n) => Some(n),
            None => {
                return Err(SensorError::Malformed(format!(
                    "next: expected int >= 0 or None, got {n}"
                )));
            }
        },
        Some(other) => {
            return Err(SensorError::Malformed(format!(
                "next: expected int >= 0 or None, got {}",
                kind(&other)
            )));
        }
    };
    let truncated = payload.len() > profile.max_payload;
    if truncated {
        let total = payload.len();
        let mut cut = profile.max_payload;
        while !payload.is_char_boundary(cut) {
            cut -= 1;
        }
        payload.truncate(cut);
        payload.push_str(&format!("{TRUNCATED_MARKER}{total} bytes]"));
    }
    Ok(SensorRun {
        fire,
        payload,
        state,
        next,
        elapsed: Duration::ZERO,
        truncated,
    })
}

/// A CPython on `PATH` (40 §1.3): `python3`, then `python`, then Windows `py -3`.
/// Verified with `--version` once per process and cached. `None` = `# needs:
/// cpython` sensors are refused.
pub fn find_cpython() -> Option<PathBuf> {
    static FOUND: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
        let names: &[&str] = if cfg!(windows) {
            &["python3.exe", "python.exe", "py.exe"]
        } else {
            &["python3", "python"]
        };
        let path = std::env::var_os("PATH")?;
        names.iter().find_map(|name| {
            std::env::split_paths(&path)
                .map(|d| d.join(name))
                .find(|p| p.is_file() && python_works(p))
        })
    });
    FOUND.clone()
}

fn python_works(p: &Path) -> bool {
    let mut cmd = std::process::Command::new(p);
    if cfg!(windows) && p.file_stem().is_some_and(|s| s == "py") {
        cmd.arg("-3");
    }
    cmd.arg("--version");
    platform::process::hide_console(&mut cmd);
    cmd.output().is_ok_and(|o| o.status.success())
}

/// `sha256(payload)` as lowercase hex (40 §1.6): what `fire: on_change` compares.
pub fn payload_hash(payload: &str) -> String {
    let digest = Sha256::digest(payload.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The envelope (40 §1.5): `key=value` tokens on the payload's first line, in
/// order of first appearance (a repeated key keeps the last value). Tokens
/// without `=` are ignored; an empty payload gives an empty map.
pub fn parse_envelope(payload: &str) -> BTreeMap<String, String> {
    payload
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|t| t.split_once('='))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_and_hash() {
        let p = "subject=pr/482 stage=waiting_ci events=ci_failed,new_commits head=9f3c1a2\nbody line k=v";
        let env = parse_envelope(p);
        assert_eq!(env["subject"], "pr/482");
        assert_eq!(env["events"], "ci_failed,new_commits");
        assert_eq!(env.len(), 4, "second line is body, not envelope");
        assert!(parse_envelope("").is_empty());
        assert!(parse_envelope("free text only").is_empty());
        assert_eq!(parse_envelope("a=1 =x b= a=2")["a"], "2");
        assert_eq!(parse_envelope("a=1 =x b=").len(), 2);

        assert_eq!(
            payload_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(payload_hash("a"), payload_hash("b"));
    }

    /// A scratch data dir (its guard) with a fresh runner; `pool` when a
    /// Monty test needs one.
    fn runner(name: &str, pool: Option<Arc<Pool>>) -> (crate::TestDir, SensorRunner) {
        let scratch = crate::TestDir::new(&format!("sensor-{name}"));
        let dir = scratch.0.clone();
        let r = SensorRunner::new(dir, pool, None, SensorProfile::default());
        r.install_builtins().unwrap();
        (scratch, r)
    }

    fn put(r: &SensorRunner, name: &str, src: &str) -> SensorMeta {
        std::fs::write(r.sensors_dir().join(name), src).unwrap();
        r.resolve(name).unwrap()
    }

    /// A pool over a worker binary: `$THEUTH_WORKER`, else `monty` or `t` on
    /// `PATH` (`t subprocess` is the production worker, 08 §9); `None`, with a
    /// note, when absent so the test skips.
    async fn worker_pool() -> Option<Arc<Pool>> {
        let worker = std::env::var_os("THEUTH_WORKER")
            .map(PathBuf::from)
            .or_else(|| {
                let path = std::env::var_os("PATH")?;
                ["monty", "t"].iter().find_map(|n| {
                    std::env::split_paths(&path)
                        .map(|d| d.join(n))
                        .find(|p| p.is_file())
                })
            });
        let Some(worker) = worker else {
            eprintln!("no monty/t worker on PATH; skipping");
            return None;
        };
        let pool = crate::repl::open_pool(&worker, 4).await;
        if pool.is_none() {
            eprintln!("worker pool failed to start; skipping");
        }
        pool
    }

    fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn error_wording() {
        assert_eq!(
            SensorError::Timeout(Duration::from_secs(20)).to_string(),
            "timeout after 20s"
        );
        assert_eq!(
            SensorError::StateTooLarge(17 * 1024 + 1).to_string(),
            "state too large: 18 KiB"
        );
        assert_eq!(
            SensorError::NeedsCpython.to_string(),
            "when: needs cpython, none found"
        );
    }

    #[test]
    fn header_parse_and_resolve_errors() {
        let (_scratch, r) = runner("header", None);
        assert_eq!(r.slots().available_permits(), 4);
        for bad in ["nope.py", "", ".shim.py", "x.txt", "../x.py", "sub/x.py"] {
            assert_eq!(
                r.resolve(bad).unwrap_err(),
                SensorError::NotFound(bad.into()),
                "{bad}"
            );
        }
        let m = put(
            &r,
            "full.py",
            "#!/usr/bin/env python3\n# theuth-sensor: full 3\n# hosts: api.github.com, GitHub.com example.org\n# unknown: ignored\n\"\"\"doc\"\"\"\nasync def probe(args: dict):\n    return None\nasync def check(s, a):\n    return {\"fire\": False}\n",
        );
        assert_eq!(m.version.as_deref(), Some("3"));
        assert_eq!(m.hosts, ["api.github.com", "github.com", "example.org"]);
        assert!(m.has_probe);
        assert!(!m.needs_cpython);
        let m = put(
            &r,
            "bare.py",
            "async def check(s, a):\n    return {\"fire\": False}\n",
        );
        assert_eq!((m.version, m.hosts.len(), m.has_probe), (None, 0, false));
        // Past the 16th line the header is body.
        let late = format!(
            "{}# needs: cpython\nasync def check(s, a): ...\n",
            "\n".repeat(16)
        );
        assert!(!put(&r, "late.py", &late).needs_cpython);
        for (name, src, what) in [
            ("v.py", "# theuth-sensor: v\n", "theuth-sensor"),
            ("v2.py", "# theuth-sensor: v one\n", "theuth-sensor"),
            ("n.py", "# needs: ruby\n", "needs"),
            ("h.py", "# hosts:\n", "hosts"),
            ("h2.py", "# hosts: https://x.org\n", "hosts"),
            ("a1.py", "# arg: repo\n", "arg"),
            ("a2.py", "# arg: repo needed o/r — x\n", "arg"),
            ("a3.py", "# arg: cwd required . — x\n", "arg"),
            ("a4.py", "# arg: Repo required o/r\n", "arg"),
            ("a5.py", "# arg: repo required o/r extra\n", "arg"),
            (
                "a6.py",
                "# arg: a required -\n# arg: a optional -\n",
                "repeated",
            ),
        ] {
            std::fs::write(r.sensors_dir().join(name), src).unwrap();
            match r.resolve(name).unwrap_err() {
                SensorError::Header(msg) => assert!(msg.contains(what), "{name}: {msg}"),
                other => panic!("{name}: {other:?}"),
            }
        }
        let built = r.resolve("fs_exists.py").unwrap();
        assert_eq!(built.version.as_deref(), Some("3"));
        assert_eq!(
            built.args,
            [SensorArg {
                name: "path".into(),
                required: true,
                example: Some("drop/*.txt".into()),
                help: "glob under the routine's directory; fires while it matches".into(),
            }]
        );
        assert_eq!(
            built.description.as_deref(),
            Some("Fires while a path exists under the file's cwd (design 40 §9 step 2).")
        );
        assert_eq!(
            built.usage.as_deref(),
            Some("when: fs_exists.py path=<glob>")
        );
    }

    /// 108 §9.1: `# arg:` lines in header order, required/optional, `-` =
    /// no example, ` -- ` as the em dash, empty help; `# needs:` / `# hosts:`
    /// beside them still parse; the docstring gives description and usage.
    #[test]
    fn arg_header_lines() {
        let (_scratch, r) = runner("args", None);
        let m = put(
            &r,
            "x.py",
            "# theuth-sensor: x 1\n# arg: repo required owner/name — the repository\n# unknown: ignored\n# arg: n optional - -- a count\n# hosts: api.github.com\n# arg: bare required 1\n\"\"\"\n\nFirst line.\n\nwhen: x.py repo=<o/r> [n=1]\nmore\n\"\"\"\nasync def check(s, a):\n    return {\"fire\": False}\n",
        );
        let arg = |name: &str, required: bool, example: Option<&str>, help: &str| SensorArg {
            name: name.into(),
            required,
            example: example.map(Into::into),
            help: help.into(),
        };
        assert_eq!(
            m.args,
            [
                arg("repo", true, Some("owner/name"), "the repository"),
                arg("n", false, None, "a count"),
                arg("bare", true, Some("1"), ""),
            ]
        );
        assert_eq!(m.version.as_deref(), Some("1"));
        assert_eq!(m.hosts, ["api.github.com"]);
        assert_eq!(m.description.as_deref(), Some("First line."));
        assert_eq!(m.usage.as_deref(), Some("when: x.py repo=<o/r> [n=1]"));
        // No docstring, no args: both absent.
        let m = put(
            &r,
            "y.py",
            "async def check(s, a):\n    return {\"fire\": False}\n",
        );
        assert!(m.args.is_empty() && m.description.is_none() && m.usage.is_none());
        // The shipped built-ins describe themselves.
        let pr = r.resolve("pr_watch.py").unwrap();
        assert_eq!(
            pr.args
                .iter()
                .map(|a| (a.name.as_str(), a.required))
                .collect::<Vec<_>>(),
            [
                ("repo", true),
                ("pr", true),
                ("token", true),
                ("stale_hours", false)
            ]
        );
        assert_eq!(pr.args[2].example.as_deref(), Some("secret"));
        assert!(
            pr.usage
                .as_deref()
                .is_some_and(|u| u.starts_with("when: pr_watch.py repo="))
        );
        let rss = r.resolve("rss.py").unwrap();
        assert_eq!(rss.args.len(), 3);
        assert_eq!(rss.args[1].example, None);
        assert_eq!(SensorRunner::shipped_version("rss.py"), Some("1"));
        assert_eq!(SensorRunner::shipped_version("x.py"), None);
        assert!(!r.edited("pr_watch.py") && !r.edited("x.py"));
        std::fs::write(
            r.sensors_dir().join("pr_watch.py"),
            "# theuth-sensor: pr_watch 2\n# mine\n",
        )
        .unwrap();
        assert!(r.edited("pr_watch.py"));
    }

    #[test]
    fn decode_validates_and_truncates() {
        let p = SensorProfile::default();
        let ok = decode(
            serde_json::json!({"fire": true, "payload": "a=1\nx", "state": {"k": 1}, "next": 30}),
            &p,
        )
        .unwrap();
        assert_eq!(
            (ok.fire, ok.payload.as_str(), ok.next, ok.truncated),
            (true, "a=1\nx", Some(30), false)
        );
        // Quiet short form (40 §8.2 step 3): payload and state optional.
        let quiet = decode(serde_json::json!({"fire": false, "next": 300}), &p).unwrap();
        assert_eq!(
            (
                quiet.payload.as_str(),
                quiet.state.to_string().as_str(),
                quiet.next
            ),
            ("", "{}", Some(300))
        );
        // Truncation at a char boundary with the §5 marker.
        let long = "€".repeat(2000); // 6000 bytes of 3-byte chars; 4096 falls mid-char
        let t = decode(serde_json::json!({"fire": true, "payload": long}), &p).unwrap();
        assert!(t.truncated);
        assert!(
            t.payload.ends_with("… [truncated, 6000 bytes]"),
            "{}",
            t.payload
        );
        let body = t.payload.trim_end_matches("… [truncated, 6000 bytes]");
        assert_eq!(body.len(), 4095, "backed off to the € boundary");
        assert!(body.chars().all(|c| c == '€'));
        // Oversized state.
        let big = serde_json::json!({"fire": false, "state": {"blob": "x".repeat(17 * 1024)}});
        assert!(matches!(decode(big, &p), Err(SensorError::StateTooLarge(n)) if n > 17 * 1024));
        for (raw, what) in [
            (serde_json::json!([1]), "expected a dict"),
            (
                serde_json::json!({"payload": "x"}),
                "fire: expected bool, got nothing",
            ),
            (
                serde_json::json!({"fire": "yes"}),
                "fire: expected bool, got str",
            ),
            (
                serde_json::json!({"fire": true, "payload": 3}),
                "payload: expected str",
            ),
            (
                serde_json::json!({"fire": true, "state": []}),
                "state: expected dict",
            ),
            (
                serde_json::json!({"fire": true, "next": -1}),
                "next: expected int >= 0",
            ),
            (
                serde_json::json!({"fire": true, "next": "soon"}),
                "next: expected int >= 0",
            ),
        ] {
            match decode(raw, &p) {
                Err(SensorError::Malformed(m)) => assert!(m.contains(what), "{m}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    }

    #[test]
    fn host_rule_for_post() {
        let hosts = vec!["api.github.com".to_owned()];
        assert!(bridge::host_rule("get", "https://anything.example/x", &hosts).is_ok());
        assert!(bridge::host_rule("POST", "https://api.github.com/graphql", &hosts).is_ok());
        assert!(bridge::host_rule("POST", "https://API.GitHub.com:443/graphql", &hosts).is_ok());
        let e = bridge::host_rule("POST", "https://evil.example/x", &hosts).unwrap_err();
        assert!(e.contains("POST to evil.example is not allowed"), "{e}");
        // Userinfo / backslash tricks resolve through the URL parser, not a split.
        assert!(
            bridge::host_rule("POST", "https://evil.example\\@api.github.com/", &hosts).is_err()
        );
        assert!(bridge::host_rule("POST", "https://user@evil.example/", &hosts).is_err());
        assert!(bridge::host_rule("POST", "not a url", &hosts).is_err());
        for m in ["PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
            let e = bridge::host_rule(m, "https://api.github.com/x", &hosts).unwrap_err();
            assert!(e.contains("not allowed"), "{m}: {e}");
        }
    }

    #[test]
    fn install_builtins_fresh_update_user_edited() {
        let (_scratch, r) = runner("install", None);
        let dir = r.sensors_dir();
        let _ = std::fs::remove_dir_all(&dir);
        // Fresh: everything written, manifest recorded.
        let mut fresh = r.install_builtins().unwrap();
        fresh.sort();
        assert_eq!(fresh, [".shim.py", "fs_exists.py", "pr_watch.py", "rss.py"]);
        let manifest: BTreeMap<String, Installed> =
            serde_json::from_slice(&std::fs::read(dir.join("sensors.json")).unwrap()).unwrap();
        assert_eq!(manifest["fs_exists.py"].version, "3");
        assert_eq!(manifest["fs_exists.py"].sha256, payload_hash(BUILTINS[0].1));
        // Unchanged: nothing written.
        assert_eq!(r.install_builtins().unwrap(), Vec::<String>::new());
        // Simulate an older install: on-disk file is the recorded (older) copy.
        let old =
            "# theuth-sensor: fs_exists 0\nasync def check(s, a):\n    return {\"fire\": False}\n";
        std::fs::write(dir.join("fs_exists.py"), old).unwrap();
        let mut m = manifest.clone();
        m.insert(
            "fs_exists.py".into(),
            Installed {
                version: "0".into(),
                sha256: payload_hash(old),
            },
        );
        std::fs::write(dir.join("sensors.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(
            r.install_builtins().unwrap(),
            ["fs_exists.py"],
            "newer + unedited → updated"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("fs_exists.py")).unwrap(),
            BUILTINS[0].1
        );
        // User-edited: recorded sha differs from the file → never overwritten.
        let edited = "# theuth-sensor: fs_exists 0\n# my tweak\nasync def check(s, a):\n    return {\"fire\": True}\n";
        std::fs::write(dir.join("fs_exists.py"), edited).unwrap();
        std::fs::write(dir.join("sensors.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(r.install_builtins().unwrap(), Vec::<String>::new());
        assert_eq!(
            std::fs::read_to_string(dir.join("fs_exists.py")).unwrap(),
            edited
        );
        // The shim is harness-owned: any difference is rewritten.
        std::fs::write(dir.join(".shim.py"), "print('x')\n").unwrap();
        assert_eq!(r.install_builtins().unwrap(), [".shim.py"]);
    }

    #[tokio::test]
    async fn semaphore_defers_when_full() {
        let (_scratch, r) = runner("slots", None);
        let meta = put(
            &r,
            "x.py",
            "async def check(s, a):\n    return {\"fire\": False}\n",
        );
        let permits: Vec<_> = futures::future::join_all((0..4).map(|_| r.slots().acquire())).await;
        assert_eq!(r.slots().available_permits(), 0);
        let t = Instant::now();
        let e = r
            .check(
                &meta,
                &args(&[]),
                &Value::Null,
                None,
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert_eq!(e, SensorError::Budget("deferred".into()));
        assert_eq!(e.to_string(), "over budget: deferred");
        assert!(t.elapsed() >= Duration::from_secs(1) && t.elapsed() < Duration::from_secs(3));
        drop(permits);
        // A free slot: the ladder reaches the pool, which this runner lacks.
        let e = r
            .check(
                &meta,
                &args(&[]),
                &Value::Null,
                None,
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(matches!(e, SensorError::Unavailable(m) if m.contains("no worker pool")));
    }

    /// 40 §9 step 2's runnable check: the built-in through a real pool.
    #[tokio::test]
    async fn fs_exists_through_monty() {
        let Some(pool) = worker_pool().await else {
            return;
        };
        let (_scratch, r) = runner("fs-exists", Some(pool));
        let meta = r.resolve("fs_exists.py").unwrap();
        let cwd = r.sensors_dir().parent().unwrap().join("cwd");
        std::fs::create_dir_all(cwd.join("sub")).unwrap();
        std::fs::write(cwd.join("hello.txt"), "hi").unwrap();
        std::fs::write(cwd.join("sub/deep.txt"), "hi").unwrap();
        let a = args(&[("path", "*.txt")]);
        let run = r
            .check(&meta, &a, &Value::Null, Some(&cwd), Duration::from_secs(20))
            .await
            .unwrap();
        println!(
            "fs_exists check #1: fire={} next={:?} truncated={} elapsed={:?}\n{}",
            run.fire, run.next, run.truncated, run.elapsed, run.payload
        );
        assert!(run.fire);
        assert_eq!(
            run.payload,
            "subject=fs/*.txt events=exists\nhello.txt\nsub/deep.txt"
        );
        assert_eq!(parse_envelope(&run.payload)["subject"], "fs/*.txt");
        assert_eq!(run.state, serde_json::json!({}));
        assert_eq!(run.next, None);
        // An absolute pattern (40 §9: `path=/tmp/…/*.txt`) is searched from its
        // directory when that lies inside cwd; outside cwd the runtime refuses.
        let abs = args(&[("path", &format!("{}/sub/*.txt", cwd.display()))]);
        let run = r
            .check(
                &meta,
                &abs,
                &Value::Null,
                Some(&cwd),
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        assert!(run.fire, "{}", run.payload);
        assert!(
            run.payload.ends_with("events=exists\ndeep.txt"),
            "{}",
            run.payload
        );
        let outside = args(&[("path", "/etc/*.conf")]);
        let e = r
            .check(
                &meta,
                &outside,
                &Value::Null,
                Some(&cwd),
                Duration::from_secs(20),
            )
            .await
            .unwrap_err();
        assert!(e.to_string().contains("outside cwd"), "{e}");
        std::fs::remove_file(cwd.join("hello.txt")).unwrap();
        std::fs::remove_file(cwd.join("sub/deep.txt")).unwrap();
        let run = r
            .check(&meta, &a, &run.state, Some(&cwd), Duration::from_secs(20))
            .await
            .unwrap();
        println!(
            "fs_exists check #2: fire={} payload={:?}",
            run.fire, run.payload
        );
        assert!(!run.fire);
        assert_eq!(run.payload, "subject=fs/*.txt events=exists");
        // No probe defined → nothing to say, no checkout.
        assert_eq!(
            r.probe(&meta, &a, Some(&cwd), Duration::from_secs(5))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn monty_budgets_and_allowlist() {
        let Some(pool) = worker_pool().await else {
            return;
        };
        let (_scratch, r) = runner("monty", Some(pool));
        let cwd = r.sensors_dir().parent().unwrap().join("cwd");
        std::fs::create_dir_all(cwd.join("inner")).unwrap();
        std::fs::write(cwd.join("note.txt"), "line one\n").unwrap();
        let a = args(&[]);
        let t = Duration::from_secs(20);

        // A runaway loop dies at the deadline, within timeout + 1 s.
        let meta = put(
            &r,
            "spin.py",
            "async def check(s, a):\n    while True:\n        pass\n",
        );
        let started = Instant::now();
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(e, SensorError::Timeout(Duration::from_secs(2)));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );

        // write_file is not bound: refused at load by the type check (40 §4.3).
        let meta = put(
            &r,
            "wf.py",
            "async def check(s, a):\n    await write_file(path=\"x\", content=\"y\")\n    return {\"fire\": False}\n",
        );
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), t)
            .await
            .unwrap_err();
        assert_eq!(
            e,
            SensorError::Budget("sensor uses write_file, not allowed".into()),
            "{e}"
        );

        // POST to an undeclared host is a PermissionError inside the sandbox.
        let meta = put(
            &r,
            "post.py",
            "# hosts: api.github.com\nasync def check(s, a):\n    await http_request(\"https://example.com/x\", \"POST\", {\"a\": 1}, None)\n    return {\"fire\": False}\n",
        );
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), t)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, SensorError::Failed(m) if m.contains("PermissionError") && m.contains("POST to example.com is not allowed")),
            "{e}"
        );
        // PUT is refused regardless of hosts; a sensor may catch it.
        let meta = put(
            &r,
            "put.py",
            "# hosts: example.com\nasync def check(s, a):\n    try:\n        await http_request(\"https://example.com/x\", \"PUT\", None, None)\n    except PermissionError as e:\n        return {\"fire\": True, \"payload\": str(e)}\n    return {\"fire\": False}\n",
        );
        let run = r
            .check(&meta, &a, &Value::Null, Some(&cwd), t)
            .await
            .unwrap();
        assert!(
            run.fire && run.payload.contains("method PUT is not allowed"),
            "{}",
            run.payload
        );

        // Files: rooted at cwd, /cwd spelling accepted, escapes refused; state, next, args["cwd"] round-trip.
        let meta = put(
            &r,
            "files.py",
            concat!(
                "async def check(s, a):\n",
                "    text = await read_file(\"note.txt\", numbers=False)\n",
                "    same = await read_file(\"/cwd/note.txt\", numbers=False)\n",
                "    listing = await list_dir(\".\")\n",
                "    try:\n",
                "        await read_file(\"../escape\")\n",
                "        esc = \"allowed\"\n",
                "    except PermissionError:\n",
                "        esc = \"refused\"\n",
                "    n = int(s.get(\"n\", 0)) + 1\n",
                "    return {\"fire\": True, \"payload\": text.split(\"\\n\")[1].strip() + \"|\" + str(text == same) + \"|\" + listing.replace(\"\\n\", \",\") + \"|\" + esc + \"|\" + a[\"k\"] + \"|\" + str(a[\"cwd\"] != \"\"), \"state\": {\"n\": n}, \"next\": n * 10}\n",
            ),
        );
        let run = r
            .check(
                &meta,
                &args(&[("k", "v")]),
                &serde_json::json!({"n": 4}),
                Some(&cwd),
                t,
            )
            .await
            .unwrap();
        assert_eq!(run.payload, "line one|True|inner/,note.txt|refused|v|True");
        assert_eq!(
            (run.state, run.next),
            (serde_json::json!({"n": 5}), Some(50))
        );

        // A raising sensor is `Failed` with the exception text; an oversize state is rejected.
        let meta = put(
            &r,
            "raise.py",
            "async def check(s, a):\n    raise ValueError(\"bad args: pr\")\n",
        );
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), t)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, SensorError::Failed(m) if m.contains("ValueError") && m.contains("bad args: pr")),
            "{e}"
        );
        let meta = put(
            &r,
            "big.py",
            "async def check(s, a):\n    return {\"fire\": False, \"state\": {\"blob\": \"x\" * 20000}}\n",
        );
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), t)
            .await
            .unwrap_err();
        assert!(
            matches!(e, SensorError::StateTooLarge(n) if n > 20000),
            "{e}"
        );
        let meta = put(
            &r,
            "long.py",
            "async def check(s, a):\n    return {\"fire\": True, \"payload\": \"k=v\\n\" + \"y\" * 5000}\n",
        );
        let run = r
            .check(&meta, &a, &Value::Null, Some(&cwd), t)
            .await
            .unwrap();
        assert!(
            run.truncated && run.payload.ends_with("… [truncated, 5004 bytes]"),
            "{}",
            run.payload
        );

        // probe: str | None.
        let meta = put(
            &r,
            "probe.py",
            "async def probe(args: dict):\n    return \"reachable: \" + args[\"k\"]\nasync def check(s, a):\n    return {\"fire\": False}\n",
        );
        assert!(meta.has_probe);
        assert_eq!(
            r.probe(&meta, &args(&[("k", "v")]), Some(&cwd), t)
                .await
                .unwrap()
                .as_deref(),
            Some("reachable: v")
        );

        // 40 §5: eight tool calls fit, the ninth dies (each call is two suspensions).
        for (n, ok) in [(8, true), (9, false)] {
            let meta = put(
                &r,
                &format!("calls{n}.py"),
                &format!(
                    "async def check(s, a):\n    for i in range({n}):\n        await list_dir(\".\")\n    return {{\"fire\": False}}\n"
                ),
            );
            let res = r.check(&meta, &a, &Value::Null, Some(&cwd), t).await;
            assert_eq!(res.is_ok(), ok, "{n} calls: {res:?}");
        }
    }

    /// 16 §5 (2026-11-11): a sensor's `http_request` follows `web.allow_private_network`
    /// (its profile): on, a GET to a loopback server answers; off, today's refusal.
    #[tokio::test]
    async fn sensor_http_follows_the_private_network_flag() {
        use wiremock::matchers::{method, path};
        let Some(pool) = worker_pool().await else {
            return;
        };
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("GET"))
            .and(path("/lan"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("on the lan"))
            .mount(&server)
            .await;
        let src = format!(
            "async def check(s, a):\n    try:\n        r = await http_request(\"{}/lan\", \"GET\", None, None)\n        return {{\"fire\": True, \"payload\": str(r[\"body\"])}}\n    except RuntimeError as e:\n        return {{\"fire\": False, \"payload\": str(e)}}\n",
            server.uri()
        );
        for (on, fire, want) in [
            (true, true, "on the lan".to_owned()),
            (
                false,
                false,
                format!("IP address `{}` is not allowed", server.address().ip()),
            ),
        ] {
            let scratch = crate::TestDir::new(&format!("sensor-lan-{on}"));
            let profile = SensorProfile {
                allow_private_network: on,
                ..SensorProfile::default()
            };
            let r = SensorRunner::new(scratch.0.clone(), Some(Arc::clone(&pool)), None, profile);
            r.install_builtins().unwrap();
            let meta = put(&r, "lan.py", &src);
            let run = r
                .check(
                    &meta,
                    &args(&[]),
                    &Value::Null,
                    None,
                    Duration::from_secs(20),
                )
                .await
                .unwrap();
            assert_eq!(run.fire, fire, "{on}: {}", run.payload);
            assert!(run.payload.contains(&want), "{on}: {}", run.payload);
        }
    }

    /// The shipped `pr_watch.py` must load under the sensor profile: a
    /// type-check failure here is a `Budget`/`Failed("type check failed")`, a
    /// clean load reaches its own argument validation (`ValueError`).
    #[tokio::test]
    async fn pr_watch_type_checks() {
        let Some(pool) = worker_pool().await else {
            return;
        };
        let (_scratch, r) = runner("prwatch", Some(pool));
        let meta = r.resolve("pr_watch.py").unwrap();
        assert_eq!(meta.hosts, ["api.github.com"]);
        let e = r
            .check(
                &meta,
                &args(&[]),
                &Value::Null,
                None,
                Duration::from_secs(20),
            )
            .await
            .unwrap_err();
        println!("pr_watch load: {e}");
        assert!(
            matches!(&e, SensorError::Failed(m) if m.contains("ValueError")),
            "pr_watch.py rejected: {e}"
        );
    }

    #[tokio::test]
    async fn cpython_ladder_runs_the_shim() {
        let Some(py) = find_cpython() else {
            eprintln!("no python on PATH; skipping");
            return;
        };
        println!("cpython: {}", py.display());
        let (_scratch, r) = runner("cpython", None);
        let cwd = r.sensors_dir().parent().unwrap().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("a.md"), "x").unwrap();
        let meta = put(
            &r,
            "sub.py",
            concat!(
                "# needs: cpython\n",
                "# hosts: example.com\n",
                "import os, subprocess\n",
                "async def check(state, args):\n",
                "    out = subprocess.run([\"echo\", \"hi\"], capture_output=True, text=True).stdout.strip()\n",
                "    found = await glob(\"*.md\")\n",
                "    try:\n",
                "        await http_request(\"https://evil.example/\", \"POST\", {\"a\": 1}, None)\n",
                "        post = \"allowed\"\n",
                "    except PermissionError:\n",
                "        post = \"refused\"\n",
                "    print(\"noise on stdout\")\n",
                "    return {\"fire\": True, \"payload\": out + \"|\" + \",\".join(found[\"files\"]) + \"|\" + post + \"|\" + args[\"k\"] + \"|\" + os.environ[\"THEUTH_SENSOR_K\"] + \"|\" + str(state.get(\"n\")), \"state\": {\"n\": int(state.get(\"n\", 0)) + 1}, \"next\": 60}\n",
                "async def probe(args):\n",
                "    return None\n",
            ),
        );
        assert!(meta.needs_cpython && meta.has_probe);
        let a = args(&[("k", "v")]);
        let run = r
            .check(
                &meta,
                &a,
                &serde_json::json!({"n": 1}),
                Some(&cwd),
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        assert_eq!(run.payload, "hi|a.md|refused|v|v|1");
        assert_eq!(
            (run.state, run.next),
            (serde_json::json!({"n": 2}), Some(60))
        );
        assert_eq!(
            r.probe(&meta, &a, Some(&cwd), Duration::from_secs(20))
                .await
                .unwrap(),
            None
        );
        // Secrets never reach a child Python (40 §4.4 v1).
        let e = r
            .check(
                &meta,
                &args(&[("token", "{{secret:gh}}")]),
                &Value::Null,
                Some(&cwd),
                Duration::from_secs(20),
            )
            .await
            .unwrap_err();
        assert_eq!(
            e,
            SensorError::Failed("cpython sensors cannot use secrets in v1".into())
        );
        // A failing child: exit code and the stderr tail.
        let meta = put(
            &r,
            "boom.py",
            "# needs: cpython\nimport sys\nasync def check(state, args):\n    print(\"partial\")\n    sys.exit(3)\n",
        );
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), Duration::from_secs(20))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, SensorError::Failed(m) if m.starts_with("exit 3: ")),
            "{e}"
        );
        // A runaway child (and its grandchild) is killed at the deadline.
        let meta = put(
            &r,
            "hang.py",
            "# needs: cpython\nimport subprocess, time\nasync def check(state, args):\n    subprocess.Popen([\"sleep\", \"600\"])\n    time.sleep(600)\n",
        );
        let started = Instant::now();
        let e = r
            .check(&meta, &a, &Value::Null, Some(&cwd), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(e, SensorError::Timeout(Duration::from_secs(2)));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }
}
