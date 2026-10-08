//! One schedule file: front matter says *when*, the body is the prompt (34 §2.2, §3).
//! Parsing, `next`/`missed` arithmetic, directory listing, and the rendering
//! plus atomic write the `schedule` tool uses. No actor, no store.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// Smallest accepted `every:` interval.
const MIN_EVERY: Duration = Duration::from_secs(10);
/// Smallest accepted `every:` with `when:` (40 §2).
pub const MIN_EVERY_WHEN: Duration = Duration::from_secs(30);
/// `timeout:` default: the sensor's wall clock (40 §5).
pub const DEFAULT_SENSOR_TIMEOUT: Duration = Duration::from_secs(20);
/// `timeout:` ceiling (40 §5); an explicit value is also checked `< every / 2`.
pub const MAX_SENSOR_TIMEOUT: Duration = Duration::from_secs(60);
/// Nanos per whole currency unit (`budget: 1 USD` = 1_000_000_000 nanos; 136 §2.3).
pub const NANOS_PER_UNIT: u64 = 1_000_000_000;

/// `budget: <number> <CUR>` (136 §2.3): the run's money cap, sent to the
/// session as `agent::SetBudget` on every fire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// The cap, in nanos of `currency`.
    pub nanos: u64,
    /// ISO 4217 code, upper-case.
    pub currency: String,
}

impl Budget {
    /// Parse `1 USD`, `0.5 USD`, `1.25 EUR`: a non-negative decimal (≤ 9
    /// fractional digits), whitespace, a 3-letter code. `Err` is the
    /// user-facing reason (`budget: expected <number> <CUR>`).
    pub fn parse(s: &str) -> Result<Self, String> {
        const ERR: &str = "budget: expected <number> <CUR>, e.g. `1 USD`";
        let mut parts = s.split_whitespace();
        let (Some(amount), Some(cur), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(ERR.into());
        };
        if cur.len() != 3 || !cur.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(ERR.into());
        }
        let (whole, frac) = amount.split_once('.').unwrap_or((amount, ""));
        if whole.is_empty() && frac.is_empty()
            || !whole.chars().all(|c| c.is_ascii_digit())
            || !frac.chars().all(|c| c.is_ascii_digit())
            || frac.len() > 9
        {
            return Err(ERR.into());
        }
        let whole: u64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| ERR.to_string())?
        };
        let frac: u64 = if frac.is_empty() {
            0
        } else {
            format!("{frac:0<9}").parse().map_err(|_| ERR.to_string())?
        };
        let nanos = whole
            .checked_mul(NANOS_PER_UNIT)
            .and_then(|n| n.checked_add(frac))
            .ok_or_else(|| ERR.to_string())?;
        Ok(Self {
            nanos,
            currency: cur.to_ascii_uppercase(),
        })
    }
}

/// `when: <file>.py key=value …` (40 §1.2, §2): the sensor and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sensor {
    /// File name in `<data_dir>/sensors/`, ends in `.py`.
    pub file: String,
    /// `key=value` tokens after the file, in order.
    pub args: BTreeMap<String, String>,
}

impl Sensor {
    /// Parse the `when:` value (40 §2): first token a `.py` file name (no path
    /// separators, no `..`), then `key=value` tokens — keys `[a-z][a-z0-9_]*`,
    /// values may be `"quoted with spaces"`, no duplicate keys, `cwd` refused
    /// (the file's `cwd:` arrives as `args["cwd"]`).
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut toks = tokenize(s)?.into_iter();
        let file = toks.next().ok_or("when: empty")?;
        if !file.ends_with(".py")
            || file.len() == 3
            || file.contains(['/', '\\'])
            || file.contains("..")
        {
            return Err("when: first token must be a .py file name".into());
        }
        let mut args = BTreeMap::new();
        for t in toks {
            let (k, v) = t
                .split_once('=')
                .ok_or_else(|| format!("when: expected key=value, got {t:?}"))?;
            if !valid_key(k) {
                return Err(format!("when: key {k:?} must match [a-z][a-z0-9_]*"));
            }
            if k == "cwd" {
                return Err(
                    "when: cwd is not an accepted key (the file's cwd: arrives as args[\"cwd\"])"
                        .into(),
                );
            }
            if args.insert(k.to_string(), v.to_string()).is_some() {
                return Err(format!("when: duplicate key {k:?}"));
            }
        }
        Ok(Self { file, args })
    }

    /// The `when:` value as written: `file key=value …`; a value with spaces
    /// (or none) is `"quoted"` so [`Sensor::parse`] reads it back.
    pub fn render(&self) -> String {
        let mut s = self.file.clone();
        for (k, v) in &self.args {
            s.push(' ');
            s.push_str(k);
            s.push('=');
            if v.is_empty() || v.contains(char::is_whitespace) {
                s.push('"');
                s.push_str(v);
                s.push('"');
            } else {
                s.push_str(v);
            }
        }
        s
    }
}

/// `[a-z][a-z0-9_]*`.
fn valid_key(k: &str) -> bool {
    let mut b = k.bytes();
    b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

/// Whitespace-separated tokens; `"…"` groups spaces into one token and the
/// quotes are dropped (`k="a b"` → `k=a b`). `Err` on an unterminated quote.
fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut tok = String::new();
    let mut quoted = false;
    let mut in_tok = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_tok = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_tok {
                    out.push(std::mem::take(&mut tok));
                    in_tok = false;
                }
            }
            c => {
                tok.push(c);
                in_tok = true;
            }
        }
    }
    if quoted {
        return Err("when: unterminated quote".into());
    }
    if in_tok {
        out.push(tok);
    }
    Ok(out)
}

/// `fire:` — when a true check becomes a turn (40 §1.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fire {
    /// Fire when `fire` is true and the payload hash differs from the last fired one.
    #[default]
    OnChange,
    /// Fire on every true check, spaced by `cooldown`.
    WhileTrue,
    /// Disable after the first fired run.
    Once,
}

/// `mode:` — a fire while the watcher's run is in flight (40 §1.9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Row `skipped: busy`.
    #[default]
    Skip,
    /// One pending payload, newest wins, delivered when the run closes.
    Queue,
    /// Cancel the in-flight run and fire.
    Restart,
}

/// `memory:` — what happens to the schedule's session between fires (40 §1.11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Memory {
    /// The frame is kept; 07's compaction shortens it.
    #[default]
    Continue,
    /// 34's behaviour: `Compact { Clear }` before each fire.
    Clear,
}

/// `runtime:` — what happens to the session's python runtime at a fire that
/// clears the frame (131 D1). Watchers with `memory: continue` never clear,
/// so their runtime is kept whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// The saved heap and inventory are removed before each fire.
    #[default]
    Clear,
    /// Python globals persist between runs.
    Keep,
}

impl std::fmt::Display for Fire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Fire::OnChange => "on_change",
            Fire::WhileTrue => "while_true",
            Fire::Once => "once",
        })
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mode::Skip => "skip",
            Mode::Queue => "queue",
            Mode::Restart => "restart",
        })
    }
}

impl std::fmt::Display for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Memory::Continue => "continue",
            Memory::Clear => "clear",
        })
    }
}

impl std::fmt::Display for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Runtime::Clear => "clear",
            Runtime::Keep => "keep",
        })
    }
}

/// `on_change` / `while_true` / `once`, else `Err("fire: expected on_change|while_true|once")`.
fn parse_fire(s: &str) -> Result<Fire, String> {
    match s.trim() {
        "on_change" => Ok(Fire::OnChange),
        "while_true" => Ok(Fire::WhileTrue),
        "once" => Ok(Fire::Once),
        _ => Err("fire: expected on_change|while_true|once".into()),
    }
}

/// `skip` / `queue` / `restart`, else `Err("mode: expected skip|queue|restart")`.
fn parse_mode(s: &str) -> Result<Mode, String> {
    match s.trim() {
        "skip" => Ok(Mode::Skip),
        "queue" => Ok(Mode::Queue),
        "restart" => Ok(Mode::Restart),
        _ => Err("mode: expected skip|queue|restart".into()),
    }
}

/// `continue` / `clear`, else `Err("memory: expected continue|clear")`.
fn parse_memory(s: &str) -> Result<Memory, String> {
    match s.trim() {
        "continue" => Ok(Memory::Continue),
        "clear" => Ok(Memory::Clear),
        _ => Err("memory: expected continue|clear".into()),
    }
}

/// `clear` / `keep`, else `Err("runtime: expected clear|keep")`.
fn parse_runtime(s: &str) -> Result<Runtime, String> {
    match s.trim() {
        "clear" => Ok(Runtime::Clear),
        "keep" => Ok(Runtime::Keep),
        _ => Err("runtime: expected clear|keep".into()),
    }
}

/// `<key>: <humantime>` → `Err("<key>: <reason>")`.
fn parse_dur(key: &str, s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s.trim()).map_err(|e| format!("{key}: {e}"))
}

/// When a schedule fires (34 §3).
#[derive(Clone, Debug)]
pub enum When {
    /// `cron:` — standard 5/6-field expression evaluated in local time.
    Cron(Box<croner::Cron>),
    /// `every:` — an interval measured from the last fire.
    Every(Duration),
    /// `at:` — one instant; the file is consumed by its fire.
    At(DateTime<Local>),
}

impl When {
    /// Next fire strictly after `now`: Cron → next occurrence; Every → `last + d`,
    /// or `now` when never fired; At → `t` while it is still ahead.
    pub fn next(
        &self,
        now: DateTime<Local>,
        last: Option<DateTime<Local>>,
    ) -> Option<DateTime<Local>> {
        match self {
            When::Cron(c) => c.find_next_occurrence(&now, false).ok(),
            When::Every(d) => Some(last.map_or(now, |l| l + *d)),
            When::At(t) => (*t > now).then_some(*t),
        }
    }

    /// Cron only: an occurrence lies in `(last, now]`. Every/At → false.
    pub fn missed(&self, last: DateTime<Local>, now: DateTime<Local>) -> bool {
        match self {
            When::Cron(c) => c.find_next_occurrence(&last, false).is_ok_and(|t| t <= now),
            When::Every(_) | When::At(_) => false,
        }
    }

    /// English line: `Cron::describe()`, `every 30m`, `at 2026-09-16T15:00`.
    pub fn describe(&self) -> String {
        match self {
            When::Cron(c) => c.describe(),
            When::Every(d) => format!("every {}", humantime::format_duration(*d)),
            When::At(t) => format!("at {}", t.format("%Y-%m-%dT%H:%M")),
        }
    }
}

impl Spec {
    /// [`When::describe`], plus ` when <file> key=value …` for a watcher (40 §2).
    pub fn describe(&self) -> String {
        match &self.sensor {
            Some(s) => format!("{} when {}", self.when.describe(), s.render()),
            None => self.when.describe(),
        }
    }
}

/// A parsed schedule file.
#[derive(Clone, Debug)]
pub struct Spec {
    /// File stem.
    pub name: String,
    /// When it fires.
    pub when: When,
    /// The clause as written: `cron 0 9 * * 1-5` / `every 30m` / `at 2026-09-16T15:00`.
    pub when_text: String,
    /// `quiet: true` — the run's reply is not delivered; only `message_user` reaches the user.
    pub quiet: bool,
    /// `missed: run` — a cron occurrence missed while down fires at boot.
    pub missed_run: bool,
    /// `model:` — model for the run's session.
    pub model: Option<String>,
    /// `enabled:` — default true; false is listed and never fires.
    pub enabled: bool,
    /// Body, trimmed; never empty.
    pub prompt: String,
    /// `until:` — shell command run before each fire; exit 0 ends the loop (33 §0.2, §3.1).
    pub until: Option<String>,
    /// `max_runs:` — fires since the last end row before the loop is capped (33 §3.2).
    pub max_runs: Option<u32>,
    /// `cwd:` — where `until` runs and the session is created; `~` expanded (33 §0.9).
    pub cwd: Option<PathBuf>,
    /// `approvals:` — default true; false maps to the per-session switch on every fire (33 §0.8).
    pub approvals: bool,
    /// `agent:` — a roster agent name; each fire creates a task owned by it
    /// instead of running in the schedule's own session (67 §7.4). Absent =
    /// today's behaviour.
    pub agent: Option<String>,
    /// `project:` — the project label of the task an `agent:` fire creates
    /// (67 §7.1); absent = inferred from `cwd`, else none.
    pub project: Option<String>,
    /// `when:` — the sensor whose check says *go* (40 §1.1); `None` for a plain
    /// 34 file. Named `sensor` because `when` is the clock clause above.
    pub sensor: Option<Sensor>,
    /// `fire:` — default `on_change` (40 §1.6). Meaningful only with `when`.
    pub fire: Fire,
    /// `for:` — consecutive true checks spanning ≥ this before the first fire (40 §1.8); default `0s`.
    pub for_: Duration,
    /// `cooldown:` — minimum spacing between fires (40 §1.8); default `0s`.
    pub cooldown: Duration,
    /// `mode:` — default `skip` (40 §1.9).
    pub mode: Mode,
    /// `memory:` — default `continue` with `when` (40 §1.11).
    pub memory: Memory,
    /// `runtime:` — default `clear`: the python runtime is reset at a fire
    /// that clears the frame (131 D1).
    pub runtime: Runtime,
    /// `expires:` — TTL from `created` (40 §1.10).
    pub expires: Option<Duration>,
    /// `timeout:` — the sensor's wall clock; default [`DEFAULT_SENSOR_TIMEOUT`] (40 §5).
    pub timeout: Duration,
    /// `created:` — written by `schedule` / `set_created` (40 §2); what `expires` counts from.
    pub created: Option<DateTime<Local>>,
    /// `run_timeout:` — time limit of one run; `None` = `schedule.run_timeout_secs` (88).
    pub run_timeout: Option<Duration>,
    /// `budget:` — the run's money cap (136 §2.3); `None` = no cap (`SetBudget{nanos: None}`).
    pub budget: Option<Budget>,
    /// `cap:` — the run's tool risk ceiling (136 D4): `read_only | mutating |
    /// destructive` → `tools::SetCap` on every fire; `None` = unchanged.
    pub cap: Option<::tools::Risk>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    cron: Option<String>,
    every: Option<String>,
    at: Option<String>,
    quiet: Option<bool>,
    missed: Option<String>,
    model: Option<String>,
    enabled: Option<bool>,
    until: Option<String>,
    max_runs: Option<u32>,
    cwd: Option<String>,
    approvals: Option<bool>,
    agent: Option<String>,
    project: Option<String>,
    when: Option<String>,
    fire: Option<String>,
    #[serde(rename = "for")]
    for_: Option<String>,
    cooldown: Option<String>,
    mode: Option<String>,
    memory: Option<String>,
    runtime: Option<String>,
    expires: Option<String>,
    timeout: Option<String>,
    created: Option<String>,
    run_timeout: Option<String>,
    budget: Option<String>,
    cap: Option<String>,
}

/// Parse one file's text. `Err` is the user-facing reason (`cron: …`,
/// `exactly one of cron / every / at`, `every: below 10s`, `at: in the past`,
/// `missed: expected run|skip`, `empty prompt`; loops (33 §2): `every: 0s needs
/// until or max_runs`, `until: empty`, `max_runs: at least 1`, `cwd: not a
/// directory: …`, `at: a one-shot has no loop to stop`; watchers (40 §2):
/// `when: requires every`, `when: exclusive with cron`, `at: with when: a
/// one-shot has no cadence to poll on`, `every: below 30s with when:`,
/// `<key>: needs when:`, `when: needs a stop (until, max_runs or expires)`,
/// `missed: not allowed with when:`, `expires: below every`, `timeout: above
/// 60s`, `timeout: must be below every / 2`, `created: expected RFC 3339`;
/// `run_timeout: below 30s` (88)).
pub fn parse(name: &str, text: &str) -> Result<Spec, String> {
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let parsed = matter
        .parse::<FrontMatter>(text)
        .map_err(|e| format!("front matter: {e}"))?;
    let fm = parsed.data.unwrap_or_default();
    let prompt = parsed.content.trim();

    let until = fm.until.map(|u| u.trim().to_string());
    if until.as_deref() == Some("") {
        return Err("until: empty".into());
    }
    if fm.max_runs == Some(0) {
        return Err("max_runs: at least 1".into());
    }
    let has_stop = until.is_some() || fm.max_runs.is_some();
    let cwd = match fm.cwd.as_deref().map(str::trim) {
        None => None,
        Some(c) => {
            let path = match c.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => dirs::home_dir()
                    .ok_or("cwd: no home directory")?
                    .join(rest.trim_start_matches('/')),
                _ => PathBuf::from(c),
            };
            if !path.is_dir() {
                return Err(format!("cwd: not a directory: {}", path.display()));
            }
            Some(path)
        }
    };

    // 40 §2: `when` requires `every`, is exclusive with `cron` / `at`. The floor,
    // the stop, `missed` and the bounds follow once `every` is known. Whether the
    // file exists in `<data_dir>/sensors/` is the tool's / loader's check.
    let sensor = match fm.when.as_deref().map(str::trim) {
        None => None,
        Some(w) => {
            if fm.cron.is_some() {
                return Err("when: exclusive with cron".into());
            }
            if fm.at.is_some() {
                return Err("at: with when: a one-shot has no cadence to poll on".into());
            }
            if fm.every.is_none() {
                return Err("when: requires every".into());
            }
            Some(Sensor::parse(w)?)
        }
    };

    let (when, when_text) = match (&fm.cron, &fm.every, &fm.at) {
        (Some(c), None, None) => {
            let c = c.trim();
            let cron = croner::Cron::from_str(c).map_err(|e| format!("cron: {e}"))?;
            (When::Cron(Box::new(cron)), format!("cron {c}"))
        }
        (None, Some(e), None) => {
            let e = e.trim();
            let d = humantime::parse_duration(e).map_err(|err| format!("every: {err}"))?;
            // 40 §2: floor MIN_EVERY_WHEN with `when`; 34's 10s / 33's 0s rules otherwise.
            if let Some(s) = &sensor {
                if d < MIN_EVERY_WHEN {
                    return Err("every: below 30s with when:".into());
                }
                (When::Every(d), format!("every {e} when {}", s.file))
            } else {
                if d < MIN_EVERY && !has_stop {
                    return Err(if d.is_zero() {
                        "every: 0s needs until or max_runs".into()
                    } else {
                        "every: below 10s".into()
                    });
                }
                (When::Every(d), format!("every {e}"))
            }
        }
        (None, None, Some(a)) => {
            if has_stop {
                return Err("at: a one-shot has no loop to stop".into());
            }
            let a = a.trim();
            let t = parse_at(a)?;
            if t <= Local::now() {
                return Err("at: in the past".into());
            }
            (When::At(t), format!("at {a}"))
        }
        _ => return Err("exactly one of cron / every / at".into()),
    };

    let missed_run = match fm.missed.as_deref().map(str::trim) {
        None | Some("skip") => false,
        Some("run") => true,
        Some(_) => return Err("missed: expected run|skip".into()),
    };
    if prompt.is_empty() {
        return Err("empty prompt".into());
    }

    for (key, present) in [
        ("fire", fm.fire.is_some()),
        ("for", fm.for_.is_some()),
        ("cooldown", fm.cooldown.is_some()),
        ("mode", fm.mode.is_some()),
        ("memory", fm.memory.is_some()),
        ("timeout", fm.timeout.is_some()),
    ] {
        if present && sensor.is_none() {
            return Err(format!("{key}: needs when:"));
        }
    }
    // 40 §2: with `when`, the sensor decides — nothing to catch up; and a loop needs a stop.
    if sensor.is_some() {
        if fm.missed.is_some() {
            return Err("missed: not allowed with when:".into());
        }
        if !has_stop && fm.expires.is_none() {
            return Err("when: needs a stop (until, max_runs or expires)".into());
        }
    }
    let fire = fm
        .fire
        .as_deref()
        .map(parse_fire)
        .transpose()?
        .unwrap_or_default();
    let for_ = fm
        .for_
        .as_deref()
        .map(|s| parse_dur("for", s))
        .transpose()?
        .unwrap_or_default();
    let cooldown = fm
        .cooldown
        .as_deref()
        .map(|s| parse_dur("cooldown", s))
        .transpose()?
        .unwrap_or_default();
    let mode = fm
        .mode
        .as_deref()
        .map(parse_mode)
        .transpose()?
        .unwrap_or_default();
    let memory = fm
        .memory
        .as_deref()
        .map(parse_memory)
        .transpose()?
        .unwrap_or_default();
    let runtime = fm
        .runtime
        .as_deref()
        .map(parse_runtime)
        .transpose()?
        .unwrap_or_default();
    // 40 §2: `expires` ≥ `every`; `timeout` ≤ MAX_SENSOR_TIMEOUT and < every / 2.
    let every = match when {
        When::Every(d) => Some(d),
        _ => None,
    };
    let expires = fm
        .expires
        .as_deref()
        .map(|s| parse_dur("expires", s))
        .transpose()?;
    if let (Some(x), Some(e)) = (expires, every)
        && x < e
    {
        return Err("expires: below every".into());
    }
    // An explicit `timeout` must be < every / 2; the default is clamped to
    // every / 2 instead (20s would refuse every 40s and below, floor included).
    let timeout = match fm.timeout.as_deref() {
        Some(t) => {
            let t = parse_dur("timeout", t)?;
            if sensor.is_some() {
                if t > MAX_SENSOR_TIMEOUT {
                    return Err("timeout: above 60s".into());
                }
                if every.is_some_and(|e| t * 2 >= e) {
                    return Err("timeout: must be below every / 2".into());
                }
            }
            t
        }
        None => every.map_or(DEFAULT_SENSOR_TIMEOUT, |e| {
            DEFAULT_SENSOR_TIMEOUT.min(e / 2)
        }),
    };
    let created = fm
        .created
        .as_deref()
        .map(str::trim)
        .map(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|t| t.with_timezone(&Local))
                .map_err(|_| "created: expected RFC 3339".to_string())
        })
        .transpose()?;
    // 88: the time limit of one run; independent of the loop stops.
    let run_timeout = fm
        .run_timeout
        .as_deref()
        .map(|s| parse_dur("run_timeout", s))
        .transpose()?;
    if run_timeout.is_some_and(|t| t < Duration::from_secs(30)) {
        return Err("run_timeout: below 30s".into());
    }
    // 136 §2.3: the run's money cap.
    let budget = fm.budget.as_deref().map(Budget::parse).transpose()?;
    // 136 D4: the run's tool risk ceiling.
    let cap = fm
        .cap
        .as_deref()
        .map(|s| {
            s.trim()
                .parse::<::tools::Risk>()
                .map_err(|_| "cap: expected read_only|mutating|destructive".to_string())
        })
        .transpose()?;

    Ok(Spec {
        name: name.to_string(),
        when,
        when_text,
        quiet: fm.quiet.unwrap_or(false),
        missed_run,
        model: fm
            .model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty()),
        enabled: fm.enabled.unwrap_or(true),
        prompt: prompt.to_string(),
        until,
        max_runs: fm.max_runs,
        cwd,
        approvals: fm.approvals.unwrap_or(true),
        agent: fm
            .agent
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
        project: fm
            .project
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
        sensor,
        fire,
        for_,
        cooldown,
        mode,
        memory,
        runtime,
        expires,
        timeout,
        created,
        run_timeout,
        budget,
        cap,
    })
}

/// RFC 3339, else `%Y-%m-%dT%H:%M:%S`, else `%Y-%m-%dT%H:%M` read as local.
fn parse_at(s: &str) -> Result<DateTime<Local>, String> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Local));
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .map_err(|_| "at: expected RFC 3339 or YYYY-MM-DDTHH:MM[:SS]".to_string())?;
    naive
        .and_local_timezone(Local)
        .earliest()
        .ok_or_else(|| "at: not a valid local time".to_string())
}

/// `[a-z0-9][a-z0-9-]*`, 1..=40 chars.
pub fn valid_name(name: &str) -> bool {
    let n = name.len();
    (1..=40).contains(&n)
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Every `*.md` in `dir` (non-recursive; names starting with `.` ignored), sorted
/// by name: `(stem, raw text, parse result)`. Missing dir → empty; an unreadable
/// file → `Err` with the io error.
pub fn load_dir(dir: &Path) -> Vec<(String, String, Result<Spec, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let file = path.file_name()?.to_str()?;
            if file.starts_with('.') || path.extension()? != "md" {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            Some(match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let spec = parse(&name, &text);
                    (name, text, spec)
                }
                Err(e) => (name, String::new(), Err(e.to_string())),
            })
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Inputs for [`render`]: what the `schedule` tool received.
#[derive(Clone, Copy, Debug)]
pub struct FileArgs<'a> {
    /// `cron:` expression.
    pub cron: Option<&'a str>,
    /// `every:` interval.
    pub every: Option<&'a str>,
    /// `at:` instant.
    pub at: Option<&'a str>,
    /// `quiet:` — written only when true.
    pub quiet: bool,
    /// `model:` — written only when present.
    pub model: Option<&'a str>,
    /// `until:` — the stop command (33 §2); written only when present.
    pub until: Option<&'a str>,
    /// `max_runs:` — the cap (33 §2); written only when present.
    pub max_runs: Option<u32>,
    /// `cwd:` — where `until` runs and the session starts (33 §2); written only when present.
    pub cwd: Option<&'a str>,
    /// `approvals:` — written only when false (33 §0.8).
    pub approvals: bool,
    /// `enabled:` — written only when false: the draft (33 §0.6).
    pub enabled: bool,
    /// `missed: run` — written only when true.
    pub missed_run: bool,
    /// `agent:` — written only when present (67 §7.4).
    pub agent: Option<&'a str>,
    /// `project:` — written only when present (67 §7.1).
    pub project: Option<&'a str>,
    /// `when:` — the sensor line (40 §2); written only when present.
    pub when: Option<&'a str>,
    /// `fire:` — written only when present (non-default).
    pub fire: Option<&'a str>,
    /// `for:` — written only when present.
    pub for_: Option<&'a str>,
    /// `cooldown:` — written only when present.
    pub cooldown: Option<&'a str>,
    /// `mode:` — written only when present.
    pub mode: Option<&'a str>,
    /// `memory:` — written only when present.
    pub memory: Option<&'a str>,
    /// `runtime:` — written only when present.
    pub runtime: Option<&'a str>,
    /// `expires:` — written only when present.
    pub expires: Option<&'a str>,
    /// `timeout:` — written only when present.
    pub timeout: Option<&'a str>,
    /// `created:` — RFC 3339; written only when present (`set_created` adds it on load).
    pub created: Option<&'a str>,
    /// `run_timeout:` — written only when present (88).
    pub run_timeout: Option<&'a str>,
    /// `budget:` — written only when present (136 §2.3).
    pub budget: Option<&'a str>,
    /// `cap:` — written only when present (136 D4).
    pub cap: Option<&'a str>,
    /// Body.
    pub prompt: &'a str,
}

/// One schedule file as the desktop editor reads and writes it: every front
/// matter key as written (no defaults applied, `cwd` unexpanded) plus the body.
/// Read leniently so a file that fails [`parse`] can still be repaired.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScheduleFile {
    /// File stem.
    pub name: String,
    /// `cron:` expression.
    pub cron: Option<String>,
    /// `every:` interval.
    pub every: Option<String>,
    /// `at:` instant.
    pub at: Option<String>,
    /// `quiet:` (default false).
    pub quiet: bool,
    /// `missed: run` (default skip).
    pub missed_run: bool,
    /// `model:`.
    pub model: Option<String>,
    /// `enabled:` (default true).
    pub enabled: bool,
    /// `until:`.
    pub until: Option<String>,
    /// `max_runs:`.
    pub max_runs: Option<u32>,
    /// `cwd:` as written.
    pub cwd: Option<String>,
    /// `approvals:` (default true).
    pub approvals: bool,
    /// `agent:`.
    pub agent: Option<String>,
    /// `project:`.
    pub project: Option<String>,
    /// `when:` as written (40 §2): `<file>.py key=value …`.
    pub when: Option<String>,
    /// `fire:` as written.
    pub fire: Option<String>,
    /// `for:` as written.
    #[serde(rename = "for")]
    pub for_: Option<String>,
    /// `cooldown:` as written.
    pub cooldown: Option<String>,
    /// `mode:` as written.
    pub mode: Option<String>,
    /// `memory:` as written.
    pub memory: Option<String>,
    /// `runtime:` as written.
    pub runtime: Option<String>,
    /// `expires:` as written.
    pub expires: Option<String>,
    /// `timeout:` as written.
    pub timeout: Option<String>,
    /// `created:` as written (RFC 3339).
    pub created: Option<String>,
    /// `run_timeout:` as written (88).
    pub run_timeout: Option<String>,
    /// `budget:` as written (136 §2.3).
    #[serde(default)]
    pub budget: Option<String>,
    /// `cap:` as written (136 D4).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cap: Option<String>,
    /// Body, trimmed.
    pub prompt: String,
}

impl ScheduleFile {
    /// Front matter + body of `text`; `Err("front matter: …")` only when the YAML itself is broken.
    pub fn read(name: &str, text: &str) -> Result<Self, String> {
        let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
        let parsed = matter
            .parse::<FrontMatter>(text)
            .map_err(|e| format!("front matter: {e}"))?;
        let fm = parsed.data.unwrap_or_default();
        Ok(Self {
            name: name.to_string(),
            cron: fm.cron,
            every: fm.every,
            at: fm.at,
            quiet: fm.quiet.unwrap_or(false),
            missed_run: fm.missed.as_deref().map(str::trim) == Some("run"),
            model: fm.model,
            enabled: fm.enabled.unwrap_or(true),
            until: fm.until,
            max_runs: fm.max_runs,
            cwd: fm.cwd,
            approvals: fm.approvals.unwrap_or(true),
            agent: fm.agent,
            project: fm.project,
            when: fm.when,
            fire: fm.fire,
            for_: fm.for_,
            cooldown: fm.cooldown,
            mode: fm.mode,
            memory: fm.memory,
            runtime: fm.runtime,
            expires: fm.expires,
            timeout: fm.timeout,
            created: fm.created,
            run_timeout: fm.run_timeout,
            budget: fm.budget,
            cap: fm.cap,
            prompt: parsed.content.trim().to_string(),
        })
    }

    /// Borrow as [`render`] input; empty strings count as absent.
    pub fn args(&self) -> FileArgs<'_> {
        fn some(s: &Option<String>) -> Option<&str> {
            s.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        FileArgs {
            cron: some(&self.cron),
            every: some(&self.every),
            at: some(&self.at),
            quiet: self.quiet,
            model: some(&self.model),
            until: some(&self.until),
            max_runs: self.max_runs,
            cwd: some(&self.cwd),
            approvals: self.approvals,
            enabled: self.enabled,
            missed_run: self.missed_run,
            agent: some(&self.agent),
            project: some(&self.project),
            when: some(&self.when),
            fire: some(&self.fire),
            for_: some(&self.for_),
            cooldown: some(&self.cooldown),
            mode: some(&self.mode),
            memory: some(&self.memory),
            runtime: some(&self.runtime),
            expires: some(&self.expires),
            timeout: some(&self.timeout),
            created: some(&self.created),
            run_timeout: some(&self.run_timeout),
            budget: some(&self.budget),
            cap: some(&self.cap),
            prompt: &self.prompt,
        }
    }
}

/// YAML double-quoted scalar: `\` and `"` escaped, so `:`, `#`, `$(…)` and a
/// leading `~` survive.
fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Render the file text: front matter with only the keys present, a blank
/// line, the prompt, trailing newline.
pub fn render(a: &FileArgs<'_>) -> String {
    let mut s = String::from("---\n");
    if let Some(c) = a.cron {
        s.push_str(&format!("cron: \"{}\"\n", c.trim()));
    }
    if let Some(e) = a.every {
        s.push_str(&format!("every: {}\n", e.trim()));
    }
    if let Some(t) = a.at {
        s.push_str(&format!("at: {}\n", t.trim()));
    }
    // 40 §2: written only when present (the tool passes non-defaults only).
    if let Some(v) = a.when {
        s.push_str(&format!("when: {}\n", yaml_str(v.trim())));
    }
    if let Some(v) = a.fire {
        s.push_str(&format!("fire: {}\n", v.trim()));
    }
    if let Some(v) = a.for_ {
        s.push_str(&format!("for: {}\n", v.trim()));
    }
    if let Some(v) = a.cooldown {
        s.push_str(&format!("cooldown: {}\n", v.trim()));
    }
    if let Some(v) = a.mode {
        s.push_str(&format!("mode: {}\n", v.trim()));
    }
    if let Some(v) = a.memory {
        s.push_str(&format!("memory: {}\n", v.trim()));
    }
    if let Some(v) = a.runtime {
        s.push_str(&format!("runtime: {}\n", v.trim()));
    }
    if let Some(v) = a.expires {
        s.push_str(&format!("expires: {}\n", v.trim()));
    }
    if let Some(v) = a.timeout {
        s.push_str(&format!("timeout: {}\n", v.trim()));
    }
    if let Some(v) = a.created {
        s.push_str(&format!("created: {}\n", yaml_str(v.trim())));
    }
    if a.quiet {
        s.push_str("quiet: true\n");
    }
    if let Some(m) = a.model {
        s.push_str(&format!("model: {}\n", m.trim()));
    }
    if let Some(u) = a.until {
        s.push_str(&format!("until: {}\n", yaml_str(u.trim())));
    }
    if let Some(n) = a.max_runs {
        s.push_str(&format!("max_runs: {n}\n"));
    }
    if let Some(v) = a.run_timeout {
        s.push_str(&format!("run_timeout: {}\n", v.trim()));
    }
    if let Some(v) = a.budget {
        s.push_str(&format!("budget: {}\n", v.trim()));
    }
    if let Some(v) = a.cap {
        s.push_str(&format!("cap: {}\n", v.trim()));
    }
    if let Some(c) = a.cwd {
        s.push_str(&format!("cwd: {}\n", yaml_str(c.trim())));
    }
    if !a.approvals {
        s.push_str("approvals: false\n");
    }
    if !a.enabled {
        s.push_str("enabled: false\n");
    }
    if a.missed_run {
        s.push_str("missed: run\n");
    }
    if let Some(v) = a.agent {
        s.push_str(&format!("agent: {}\n", v.trim()));
    }
    if let Some(v) = a.project {
        s.push_str(&format!("project: {}\n", yaml_str(v.trim())));
    }
    s.push_str("---\n\n");
    s.push_str(a.prompt.trim());
    s.push('\n');
    s
}

/// Rewrite the `enabled:` line inside the front matter (33 §3.5): replace it
/// when present, else insert `enabled: <v>` right after the opening `---`.
/// Text without front matter gets one. Everything else is byte-for-byte.
pub fn set_enabled(text: &str, enabled: bool) -> String {
    set_line(text, "enabled:", &format!("enabled: {enabled}\n"))
}

/// Rewrite the `created:` line (40 §2) the way [`set_enabled`] does: `at` as
/// RFC 3339 with the local offset, whole seconds. The actor calls it on first
/// load of a hand-edited `when:` file that lacks the line; `expires` counts from it.
pub fn set_created(text: &str, at: DateTime<Local>) -> String {
    let stamp = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    set_line(
        text,
        "created:",
        &format!("created: {}\n", yaml_str(&stamp)),
    )
}

/// Replace the front-matter line starting with `key`, else insert `line` right
/// after the opening `---`. Text without front matter gets one.
fn set_line(text: &str, key: &str, line: &str) -> String {
    if !text.starts_with("---\n") {
        return format!("---\n{line}---\n{text}");
    }
    let mut lines = text.split_inclusive('\n');
    let open = lines.next().unwrap_or_default();
    let mut out = String::with_capacity(text.len() + line.len());
    out.push_str(open);
    let mut replaced = false;
    for l in &mut lines {
        if l.trim_end() == "---" {
            out.push_str(l);
            break;
        }
        if l.starts_with(key) {
            out.push_str(line);
            replaced = true;
        } else {
            out.push_str(l);
        }
    }
    if !replaced {
        out.insert_str(open.len(), line);
    }
    out.extend(lines);
    out
}

/// Write `<dir>/<name>.md` atomically (dotfile temp in `dir`, then rename) so the
/// watcher never reads half of it. Creates `dir`. Returns the final path.
pub fn write_atomic(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.md.tmp"));
    let path = dir.join(format!("{name}.md"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn md(front: &str, body: &str) -> String {
        format!("---\n{front}\n---\n{body}\n")
    }

    #[test]
    fn exactly_one_of_cron_every_at() {
        assert_eq!(
            parse("x", &md("quiet: true", "p")).unwrap_err(),
            "exactly one of cron / every / at"
        );
        assert_eq!(
            parse("x", &md("cron: \"0 9 * * *\"\nevery: 30m", "p")).unwrap_err(),
            "exactly one of cron / every / at"
        );
        assert!(parse("x", "no front matter at all").is_err());
        assert!(parse("x", &md("cron: \"0 9 * * *\"", "p")).is_ok());
    }

    #[test]
    fn every_bounds() {
        assert_eq!(
            parse("x", &md("every: 5s", "p")).unwrap_err(),
            "every: below 10s"
        );
        let s = parse("x", &md("every: 1h 30m", "p")).unwrap();
        assert!(matches!(s.when, When::Every(d) if d == Duration::from_secs(5400)));
        assert_eq!(s.when_text, "every 1h 30m");
    }

    #[test]
    fn at_forms() {
        assert_eq!(
            parse("x", &md("at: 2020-01-01T10:00", "p")).unwrap_err(),
            "at: in the past"
        );
        assert!(parse("x", &md("at: 2099-01-01T10:00:00+02:00", "p")).is_ok());
        assert!(parse("x", &md("at: 2099-01-01T10:00:00", "p")).is_ok());
        let s = parse("x", &md("at: 2099-01-01T10:00", "p")).unwrap();
        assert!(matches!(s.when, When::At(t) if t.hour() == 10));
        assert_eq!(s.when_text, "at 2099-01-01T10:00");
        assert!(parse("x", &md("at: tomorrow", "p")).is_err());
    }

    #[test]
    fn misc_keys() {
        assert_eq!(
            parse("x", &md("cron: \"0 9 * * *\"\nmissed: maybe", "p")).unwrap_err(),
            "missed: expected run|skip"
        );
        assert_eq!(
            parse("x", &md("cron: \"0 9 * * *\"", "  \n")).unwrap_err(),
            "empty prompt"
        );
        assert!(
            parse("x", &md("cron: \"0 99 * * *\"", "p"))
                .unwrap_err()
                .starts_with("cron: ")
        );
        let s = parse(
            "n",
            &md("cron: \"0 9 * * *\"\nmissed: run\nenabled: false", "  p  "),
        )
        .unwrap();
        assert!(s.missed_run && !s.enabled && !s.quiet);
        assert_eq!((s.name.as_str(), s.prompt.as_str()), ("n", "p"));
    }

    #[test]
    fn cron_next_holds_wall_clock_across_dst() {
        let c = When::Cron(Box::new(croner::Cron::from_str("0 9 * * *").unwrap()));
        // Two days before the 2026-11-01 fall-back; walk four fires across it.
        let mut now = Local.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap();
        for _ in 0..4 {
            let next = c.next(now, None).unwrap();
            assert!(next > now);
            assert_eq!((next.hour(), next.minute()), (9, 0));
            now = next;
        }
        assert_eq!(
            now.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()
        );
    }

    #[test]
    fn every_next_and_missed() {
        let now = Local.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap();
        let e = When::Every(Duration::from_secs(1800));
        assert_eq!(e.next(now, None), Some(now));
        let last = now - chrono::Duration::minutes(10);
        assert_eq!(
            e.next(now, Some(last)),
            Some(last + chrono::Duration::minutes(30))
        );
        assert!(!e.missed(last, now));

        let c = When::Cron(Box::new(croner::Cron::from_str("0 9 * * *").unwrap()));
        let last = Local.with_ymd_and_hms(2026, 9, 15, 9, 0, 0).unwrap();
        assert!(c.missed(last, now)); // 16th 09:00 lies between
        assert!(!c.missed(Local.with_ymd_and_hms(2026, 9, 16, 9, 0, 0).unwrap(), now));

        let at = When::At(now + chrono::Duration::hours(1));
        assert_eq!(at.next(now, None), Some(now + chrono::Duration::hours(1)));
        assert_eq!(at.next(now + chrono::Duration::hours(2), None), None);
    }

    const BASE: FileArgs<'static> = FileArgs {
        cron: None,
        every: None,
        at: None,
        quiet: false,
        model: None,
        until: None,
        max_runs: None,
        cwd: None,
        approvals: true,
        enabled: true,
        missed_run: false,
        agent: None,
        project: None,
        when: None,
        fire: None,
        for_: None,
        cooldown: None,
        mode: None,
        memory: None,
        runtime: None,
        expires: None,
        timeout: None,
        created: None,
        run_timeout: None,
        budget: None,
        cap: None,
        prompt: "p",
    };

    #[test]
    fn schedule_file_round_trip() {
        let f = ScheduleFile {
            name: "x".into(),
            cron: Some("0 9 * * 1-5".into()),
            quiet: true,
            missed_run: true,
            model: Some("m".into()),
            enabled: false,
            cwd: Some("~/src".into()),
            approvals: false,
            agent: Some("alice".into()),
            project: Some("web: v2".into()),
            run_timeout: Some("45m".into()),
            prompt: "do it".into(),
            ..ScheduleFile::default()
        };
        let text = render(&f.args());
        let back = ScheduleFile::read("x", &text).unwrap();
        assert_eq!(back.cron.as_deref(), Some("0 9 * * 1-5"));
        assert!(back.quiet && back.missed_run && !back.enabled && !back.approvals);
        assert_eq!(back.cwd.as_deref(), Some("~/src"));
        assert_eq!(back.agent.as_deref(), Some("alice"));
        assert_eq!(back.project.as_deref(), Some("web: v2"));
        assert_eq!(back.run_timeout.as_deref(), Some("45m"));
        assert_eq!(render(&back.args()), text);
        assert_eq!(back.prompt, "do it");
        // Broken semantics (no clause) still read: the editor can repair it.
        let broken = ScheduleFile::read("y", "---\nquiet: true\n---\n").unwrap();
        assert!(broken.cron.is_none() && broken.quiet && broken.prompt.is_empty());
    }

    #[test]
    fn cap_key() {
        // 136 D4: `cap: read_only|mutating|destructive` → the run's tool
        // ceiling; absent → None (unchanged); anything else is refused.
        let s = parse("x", &md("every: 1h\ncap: mutating", "p")).unwrap();
        assert_eq!(s.cap, Some(::tools::Risk::Mutating));
        assert_eq!(parse("x", &md("every: 1h", "p")).unwrap().cap, None);
        assert_eq!(
            parse("x", &md("every: 1h\ncap: all", "p")).unwrap_err(),
            "cap: expected read_only|mutating|destructive"
        );
        // The editor's read → render round trip keeps the key.
        let f = ScheduleFile::read("x", &md("every: 1h\ncap: mutating", "p")).unwrap();
        assert_eq!(f.cap.as_deref(), Some("mutating"));
        let text = render(&f.args());
        assert!(text.contains("cap: mutating\n"), "{text}");
        assert_eq!(parse("x", &text).unwrap().cap, Some(::tools::Risk::Mutating));
    }

    #[test]
    fn budget_key() {
        // 136 §2.3: `<number> <CUR>` → nanos + upper-case code; absent → None.
        let s = parse("x", &md("every: 1h\nbudget: 1 USD", "p")).unwrap();
        assert_eq!(
            s.budget,
            Some(Budget {
                nanos: NANOS_PER_UNIT,
                currency: "USD".into()
            })
        );
        assert_eq!(parse("x", &md("every: 1h", "p")).unwrap().budget, None);
        assert_eq!(Budget::parse("0.5 usd").unwrap().nanos, 500_000_000);
        assert_eq!(Budget::parse("1.25 EUR").unwrap().nanos, 1_250_000_000);
        assert_eq!(Budget::parse(".5 USD").unwrap().nanos, 500_000_000);
        assert_eq!(Budget::parse("5. USD").unwrap().nanos, 5_000_000_000);
        assert_eq!(Budget::parse("0.000000001 USD").unwrap().nanos, 1);
        for bad in [
            "1",
            "USD 1",
            "-1 USD",
            "1 US",
            "1 USD extra",
            "1,5 USD",
            "0.0000000001 USD",
            "",
        ] {
            assert!(Budget::parse(bad).is_err(), "{bad:?}");
        }
        assert!(
            parse("x", &md("every: 1h\nbudget: lots", "p"))
                .unwrap_err()
                .starts_with("budget:")
        );
        // The editor model keeps it and the renderer writes it.
        let f = ScheduleFile::read("x", &md("every: 1h\nbudget: 1 USD", "p")).unwrap();
        assert_eq!(f.budget.as_deref(), Some("1 USD"));
        assert!(render(&f.args()).contains("budget: 1 USD\n"));
    }

    #[test]
    fn run_timeout_key() {
        // 88: a humantime per-run cap, ≥ 30s, allowed with every kind and with `when:`.
        let s = parse("x", &md("every: 1h\nrun_timeout: 45m", "p")).unwrap();
        assert_eq!(s.run_timeout, Some(Duration::from_secs(2700)));
        assert_eq!(parse("x", &md("every: 1h", "p")).unwrap().run_timeout, None);
        assert_eq!(
            parse("x", &md("every: 1h\nrun_timeout: 10s", "p")).unwrap_err(),
            "run_timeout: below 30s"
        );
        assert!(
            parse("x", &md("every: 1h\nrun_timeout: soon", "p"))
                .unwrap_err()
                .starts_with("run_timeout:")
        );
        let at = parse("x", &md("at: 2099-01-01T10:00\nrun_timeout: 2h", "p")).unwrap();
        assert_eq!(at.run_timeout, Some(Duration::from_secs(7200)));
        let w = parse(
            "x",
            &md("every: 60s\nwhen: a.py\nmax_runs: 1\nrun_timeout: 90s", "p"),
        )
        .unwrap();
        assert_eq!(w.run_timeout, Some(Duration::from_secs(90)));
    }

    #[test]
    fn runtime_key() {
        // 131 D1: `clear` (default) | `keep`; written only when non-default.
        let s = parse("x", &md("every: 1h\nruntime: keep", "p")).unwrap();
        assert_eq!(s.runtime, Runtime::Keep);
        assert_eq!(
            parse("x", &md("every: 1h\nruntime: clear", "p"))
                .unwrap()
                .runtime,
            Runtime::Clear
        );
        assert_eq!(
            parse("x", &md("every: 1h", "p")).unwrap().runtime,
            Runtime::Clear
        );
        assert_eq!(
            parse("x", &md("every: 1h\nruntime: sometimes", "p")).unwrap_err(),
            "runtime: expected clear|keep"
        );
        let f = ScheduleFile {
            every: Some("1h".into()),
            runtime: Some("keep".into()),
            prompt: "p".into(),
            ..ScheduleFile::default()
        };
        let text = render(&f.args());
        assert!(text.contains("\nruntime: keep\n"), "{text}");
        let back = ScheduleFile::read("x", &text).unwrap();
        assert_eq!(back.runtime.as_deref(), Some("keep"));
        assert_eq!(render(&back.args()), text);
        assert!(!render(&BASE).contains("runtime:"));
    }

    #[test]
    fn watcher_keys_round_trip_and_structural_rules() {
        // `parse` owns the full rule table; this pins only the contract: keys parse,
        // defaults apply, the file round-trips, the structural refusals hold.
        let text = render(&FileArgs {
            every: Some("60s"),
            when: Some("pr_watch.py repo=o/r pr=482 token=gh_token"),
            mode: Some("queue"),
            cooldown: Some("5m"),
            expires: Some("14d"),
            max_runs: Some(40),
            created: Some("2026-09-28T10:00:00+01:00"),
            ..BASE
        });
        let s = parse("argo", &text).unwrap();
        let sensor = s.sensor.as_ref().unwrap();
        assert_eq!(sensor.file, "pr_watch.py");
        assert_eq!(sensor.args["pr"], "482");
        assert_eq!(
            sensor.render(),
            "pr_watch.py pr=482 repo=o/r token=gh_token"
        );
        assert_eq!(
            (s.fire, s.mode, s.memory),
            (Fire::OnChange, Mode::Queue, Memory::Continue)
        );
        assert_eq!(s.cooldown, Duration::from_secs(300));
        assert!(s.for_.is_zero());
        assert_eq!(s.expires, Some(Duration::from_secs(14 * 86400)));
        assert_eq!(s.timeout, DEFAULT_SENSOR_TIMEOUT);
        assert!(s.created.is_some());
        let back = ScheduleFile::read("argo", &text).unwrap();
        assert_eq!(
            back.when.as_deref(),
            Some("pr_watch.py repo=o/r pr=482 token=gh_token")
        );
        assert_eq!(back.mode.as_deref(), Some("queue"));
        assert_eq!(render(&back.args()), text);

        let plain = parse("x", &md("every: 1h", "p")).unwrap();
        assert!(plain.sensor.is_none() && plain.fire == Fire::OnChange);
        for (fm, err) in [
            ("when: a.py", "when: requires every"),
            (
                "cron: \"0 9 * * *\"\nwhen: a.py",
                "when: exclusive with cron",
            ),
            (
                "every: 60s\nwhen: a",
                "when: first token must be a .py file name",
            ),
            (
                "every: 60s\nwhen: a.py x",
                "when: expected key=value, got \"x\"",
            ),
            ("every: 60s\nmode: queue", "mode: needs when:"),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\nfire: maybe",
                "fire: expected on_change|while_true|once",
            ),
        ] {
            assert_eq!(parse("x", &md(fm, "p")).unwrap_err(), err, "{fm}");
        }
    }

    /// 40 §2's babysitter, verbatim except `cwd` (a temp dir so `is_dir` holds).
    fn babysitter(cwd: &str) -> String {
        md(
            &format!(
                "every: 60s\nwhen: pr_watch.py repo=owner/sre-argo pr=482 token=gh_token\n\
                 fire: on_change\nfor: 0s\ncooldown: 5m\nmode: queue\nmemory: continue\n\
                 max_runs: 40\nexpires: 14d\ncwd: {cwd}\nquiet: true\napprovals: false\nenabled: false"
            ),
            "You babysit PR #482.",
        )
    }

    #[test]
    fn when_tokenizer() {
        assert_eq!(tokenize("a.py").unwrap(), ["a.py"]);
        assert_eq!(
            tokenize("  a.py   k=v\tx=y ").unwrap(),
            ["a.py", "k=v", "x=y"]
        );
        assert_eq!(
            tokenize(r#"a.py msg="two words" k="" q="a=b c""#).unwrap(),
            ["a.py", "msg=two words", "k=", "q=a=b c"]
        );
        assert_eq!(
            tokenize(r#"a.py k="open"#).unwrap_err(),
            "when: unterminated quote"
        );
        assert!(tokenize("").unwrap().is_empty());
    }

    #[test]
    fn when_sensor_rules() {
        let s = Sensor::parse(r#"pr_watch.py repo=o/r title="a b" empty="""#).unwrap();
        assert_eq!(s.file, "pr_watch.py");
        assert_eq!(s.args["title"], "a b");
        assert_eq!(s.args["empty"], "");
        // render quotes what needs it and parses back equal.
        assert_eq!(s.render(), r#"pr_watch.py empty="" repo=o/r title="a b""#);
        assert_eq!(Sensor::parse(&s.render()).unwrap(), s);
        assert!(Sensor::parse("under_score1.py k_1=v").is_ok());
        for (line, err) in [
            ("", "when: empty"),
            ("../x.py", "when: first token must be a .py file name"),
            ("a/b.py", "when: first token must be a .py file name"),
            ("a\\b.py", "when: first token must be a .py file name"),
            (".py", "when: first token must be a .py file name"),
            ("a.py K=v", "when: key \"K\" must match [a-z][a-z0-9_]*"),
            ("a.py 1k=v", "when: key \"1k\" must match [a-z][a-z0-9_]*"),
            ("a.py k-1=v", "when: key \"k-1\" must match [a-z][a-z0-9_]*"),
            ("a.py =v", "when: key \"\" must match [a-z][a-z0-9_]*"),
            ("a.py k=1 k=2", "when: duplicate key \"k\""),
            (
                "a.py cwd=/tmp",
                "when: cwd is not an accepted key (the file's cwd: arrives as args[\"cwd\"])",
            ),
        ] {
            assert_eq!(Sensor::parse(line).unwrap_err(), err, "{line:?}");
        }
    }

    #[test]
    fn when_rules_table() {
        // Positive: the minimum watcher.
        let s = parse("x", &md("every: 60s\nwhen: a.py\nmax_runs: 1", "p")).unwrap();
        assert_eq!(s.when_text, "every 60s when a.py");
        assert_eq!(s.timeout, DEFAULT_SENSOR_TIMEOUT);
        // every floor: 30s passes with when (default timeout clamps to every/2), 29s fails; 10s still fine without.
        let floor = parse("x", &md("every: 30s\nwhen: a.py\nmax_runs: 1", "p")).unwrap();
        assert_eq!(floor.timeout, Duration::from_secs(15));
        assert!(parse("x", &md("every: 10s", "p")).is_ok());
        // Each stop alone satisfies the rule.
        for stop in ["until: \"true\"", "max_runs: 3", "expires: 1h"] {
            assert!(
                parse("x", &md(&format!("every: 60s\nwhen: a.py\n{stop}"), "p")).is_ok(),
                "{stop}"
            );
        }
        // expires == every is fine; timeout just below every/2 is fine; 60s ceiling ok with a long every.
        assert!(parse("x", &md("every: 60s\nwhen: a.py\nexpires: 60s", "p")).is_ok());
        assert!(
            parse(
                "x",
                &md("every: 60s\nwhen: a.py\nmax_runs: 1\ntimeout: 29s", "p")
            )
            .is_ok()
        );
        assert!(
            parse(
                "x",
                &md("every: 5m\nwhen: a.py\nmax_runs: 1\ntimeout: 60s", "p")
            )
            .is_ok()
        );
        // expires without when is allowed on a plain file (the table does not tie it to when).
        assert!(parse("x", &md("every: 1h\nexpires: 2h", "p")).is_ok());
        assert!(parse("x", &md("cron: \"0 9 * * *\"\nexpires: 1h", "p")).is_ok());

        for (fm, err) in [
            (
                "every: 29s\nwhen: a.py\nmax_runs: 1",
                "every: below 30s with when:",
            ),
            (
                "every: 0s\nwhen: a.py\nmax_runs: 1",
                "every: below 30s with when:",
            ),
            (
                "at: 2099-01-01T10:00\nwhen: a.py",
                "at: with when: a one-shot has no cadence to poll on",
            ),
            (
                "every: 60s\nwhen: a.py",
                "when: needs a stop (until, max_runs or expires)",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\nmissed: run",
                "missed: not allowed with when:",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\nmissed: skip",
                "missed: not allowed with when:",
            ),
            (
                "every: 60s\nwhen: a.py\nexpires: 59s",
                "expires: below every",
            ),
            ("every: 1h\nexpires: 30m", "expires: below every"),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\ntimeout: 30s",
                "timeout: must be below every / 2",
            ),
            (
                "every: 5m\nwhen: a.py\nmax_runs: 1\ntimeout: 61s",
                "timeout: above 60s",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\ntimeout: soon",
                "timeout: expected number at 0",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\nfor: x",
                "for: expected number at 0",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\nmode: sometimes",
                "mode: expected skip|queue|restart",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\nmemory: forget",
                "memory: expected continue|clear",
            ),
            (
                "every: 60s\nwhen: a.py\nmax_runs: 1\ncreated: yesterday",
                "created: expected RFC 3339",
            ),
            ("every: 60s\nfire: once", "fire: needs when:"),
            ("every: 60s\nfor: 1m", "for: needs when:"),
            ("every: 60s\ncooldown: 1m", "cooldown: needs when:"),
            ("every: 60s\nmemory: clear", "memory: needs when:"),
            ("every: 60s\ntimeout: 5s", "timeout: needs when:"),
        ] {
            assert_eq!(parse("x", &md(fm, "p")).unwrap_err(), err, "{fm}");
        }
    }

    #[test]
    fn describe_and_when_text_for_watchers() {
        let s = parse(
            "x",
            &md(
                "every: 60s\nwhen: pr_watch.py pr=482 repo=o/r\nmax_runs: 1",
                "p",
            ),
        )
        .unwrap();
        assert_eq!(s.when_text, "every 60s when pr_watch.py");
        assert_eq!(s.describe(), "every 1m when pr_watch.py pr=482 repo=o/r");
        let plain = parse("x", &md("every: 30m", "p")).unwrap();
        assert_eq!(
            (plain.when_text.as_str(), plain.describe().as_str()),
            ("every 30m", "every 30m")
        );
    }

    #[test]
    fn babysitter_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let a = parse("argo", &babysitter(cwd)).unwrap();
        let sensor = a.sensor.as_ref().unwrap();
        assert_eq!(sensor.file, "pr_watch.py");
        assert_eq!(sensor.args["repo"], "owner/sre-argo");
        assert_eq!(
            (a.fire, a.mode, a.memory),
            (Fire::OnChange, Mode::Queue, Memory::Continue)
        );
        assert_eq!(
            (a.for_, a.cooldown),
            (Duration::ZERO, Duration::from_secs(300))
        );
        assert_eq!(
            (a.max_runs, a.expires),
            (Some(40), Some(Duration::from_secs(14 * 86400)))
        );
        assert!(a.quiet && !a.approvals && !a.enabled && a.created.is_none());

        // Spec → file (as the tool writes it: defaults dropped) → Spec, equal.
        let when = sensor.render();
        let (cooldown, expires) = (
            humantime::format_duration(a.cooldown).to_string(),
            humantime::format_duration(a.expires.unwrap()).to_string(),
        );
        let text = render(&FileArgs {
            every: Some("60s"),
            when: Some(&when),
            cooldown: Some(&cooldown),
            mode: Some("queue"),
            max_runs: a.max_runs,
            expires: Some(&expires),
            cwd: Some(cwd),
            quiet: true,
            approvals: false,
            enabled: false,
            prompt: &a.prompt,
            ..BASE
        });
        let b = parse("argo", &text).unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        // Key order follows §2's example: clock, when, watcher keys, then the rest.
        let keys: Vec<_> = text
            .lines()
            .skip(1)
            .take_while(|l| *l != "---")
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "every",
                "when",
                "cooldown",
                "mode",
                "expires",
                "quiet",
                "max_runs",
                "cwd",
                "approvals",
                "enabled"
            ]
        );
    }

    #[test]
    fn set_created_inserts_or_replaces() {
        let at = Local.with_ymd_and_hms(2026, 9, 28, 10, 15, 3).unwrap();
        let stamp = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let text = md("every: 60s\nwhen: a.py\nmax_runs: 1", "p");
        let with = set_created(&text, at);
        assert_eq!(
            with,
            md(
                &format!("created: \"{stamp}\"\nevery: 60s\nwhen: a.py\nmax_runs: 1"),
                "p"
            )
        );
        assert_eq!(parse("x", &with).unwrap().created, Some(at));

        let later = at + chrono::Duration::hours(1);
        let replaced = set_created(&with, later);
        assert_eq!(parse("x", &replaced).unwrap().created, Some(later));
        assert_eq!(replaced.matches("created:").count(), 1);
        assert_eq!(set_created(&replaced, at), with);
        // Same write path as set_enabled: byte-for-byte elsewhere, front matter added when absent.
        assert_eq!(
            set_created("p\n", at),
            format!("---\ncreated: \"{stamp}\"\n---\np\n")
        );
    }

    #[test]
    fn render_parse_round_trip() {
        let text = render(&FileArgs {
            cron: Some("0 9 * * 1-5"),
            quiet: true,
            model: Some("claude-haiku-4-5"),
            prompt: "Brief me.\n\nTwice.",
            ..BASE
        });
        let s = parse("brief", &text).unwrap();
        assert_eq!(s.when_text, "cron 0 9 * * 1-5");
        assert!(s.quiet && s.enabled && !s.missed_run && s.approvals);
        assert_eq!(s.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(s.prompt, "Brief me.\n\nTwice.");
        assert!(s.until.is_none() && s.max_runs.is_none() && s.cwd.is_none());

        let text = render(&FileArgs {
            every: Some("30m"),
            ..BASE
        });
        assert!(!text.contains("quiet") && !text.contains("model"));
        assert!(!text.contains("approvals") && !text.contains("enabled"));
        let s = parse("q", &text).unwrap();
        assert!(!s.quiet && s.model.is_none());
        assert_eq!(s.when_text, "every 30m");

        let text = render(&FileArgs {
            at: Some("2099-01-01T10:00"),
            ..BASE
        });
        assert_eq!(parse("a", &text).unwrap().when_text, "at 2099-01-01T10:00");
    }

    #[test]
    fn loop_keys_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let text = render(&FileArgs {
            every: Some("0s"),
            until: Some("test \"$(gh issue view 42 -q .state)\" = CLOSED # done: closed"),
            max_runs: Some(200),
            cwd: Some(cwd),
            approvals: false,
            enabled: false,
            ..BASE
        });
        let s = parse("port", &text).unwrap();
        assert!(matches!(s.when, When::Every(d) if d.is_zero()));
        assert_eq!(
            s.until.as_deref(),
            Some("test \"$(gh issue view 42 -q .state)\" = CLOSED # done: closed")
        );
        assert_eq!(s.max_runs, Some(200));
        assert_eq!(s.cwd.as_deref(), Some(dir.path()));
        assert!(!s.approvals && !s.enabled);
    }

    #[test]
    fn loop_rules() {
        assert_eq!(
            parse("x", &md("every: 0s", "p")).unwrap_err(),
            "every: 0s needs until or max_runs"
        );
        let s = parse("x", &md("every: 0s\nmax_runs: 1", "p")).unwrap();
        assert!(matches!(s.when, When::Every(d) if d.is_zero()));
        assert!(parse("x", &md("every: 5s\nuntil: cargo test", "p")).is_ok());
        assert_eq!(
            parse("x", &md("at: 2099-01-01T10:00\nuntil: ./done.sh", "p")).unwrap_err(),
            "at: a one-shot has no loop to stop"
        );
        assert!(
            parse("x", &md("every: 1h\ncwd: /definitely/not/here", "p"))
                .unwrap_err()
                .starts_with("cwd: not a directory")
        );
        assert_eq!(
            parse("x", &md("every: 1h\nmax_runs: 0", "p")).unwrap_err(),
            "max_runs: at least 1"
        );
        assert_eq!(
            parse("x", &md("every: 1h\nuntil: \"  \"", "p")).unwrap_err(),
            "until: empty"
        );
        if let Some(home) = dirs::home_dir() {
            let s = parse("x", &md("every: 1h\ncwd: \"~\"", "p")).unwrap();
            assert_eq!(s.cwd.as_deref(), Some(home.as_path()));
        }
    }

    #[test]
    fn set_enabled_rewrites_or_inserts() {
        let text = md("every: 1h\nenabled: true\nquiet: true", "p");
        let off = set_enabled(&text, false);
        assert_eq!(off, md("every: 1h\nenabled: false\nquiet: true", "p"));
        assert!(!parse("x", &off).unwrap().enabled);
        assert_eq!(set_enabled(&off, true), text);

        let text = md("every: 1h", "p");
        let off = set_enabled(&text, false);
        assert_eq!(off, md("enabled: false\nevery: 1h", "p"));
        assert!(!parse("x", &off).unwrap().enabled);

        assert_eq!(set_enabled("p\n", false), "---\nenabled: false\n---\np\n");
    }

    #[test]
    fn valid_name_boundaries() {
        assert!(valid_name("a"));
        assert!(valid_name("morning-brief"));
        assert!(valid_name("0abc"));
        assert!(valid_name(&"a".repeat(40)));
        assert!(!valid_name(&"a".repeat(41)));
        assert!(!valid_name(""));
        assert!(!valid_name("-a"));
        assert!(!valid_name("Brief"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("a_b"));
    }

    #[test]
    fn load_dir_filters_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let good = md("cron: \"0 9 * * *\"", "p");
        std::fs::write(dir.path().join("b.md"), &good).unwrap();
        std::fs::write(dir.path().join("a.md"), md("every: 1h", "p")).unwrap();
        std::fs::write(dir.path().join("broken.md"), md("every: 1s", "p")).unwrap();
        std::fs::write(dir.path().join(".hidden.md"), &good).unwrap();
        std::fs::write(dir.path().join("notes.txt"), &good).unwrap();

        let got = load_dir(dir.path());
        let names: Vec<_> = got.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "b", "broken"]);
        assert_eq!(got.iter().filter(|(_, _, r)| r.is_err()).count(), 1);
        assert_eq!(got[2].2.as_ref().unwrap_err(), "every: below 10s");
        assert_eq!(got[1].1, good);

        assert!(load_dir(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn write_atomic_lands_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_atomic(&dir.path().join("schedules"), "x", "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
