//! Cells (design doc 171 §3): the runnable fenced code blocks a reply or a note presents.
//!
//! A cell is a fence whose language word is runnable and whose body starts with header
//! lines (`#| key: value`; `//|` for JavaScript/TypeScript). [`parse`] reads one fence into a
//! [`Cell`], [`validate`] reports the §3.6 problems, [`lint`] the §3.7 chips, [`digest`] the
//! §3.4 identity hash, [`parse_defaults`] a note's ```` ```cells ```` block. Pure text in,
//! data out; the TypeScript mirror (`components/cells/grammar.ts`) reads the same fixtures
//! (`tests/cells.json`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The longest `label` the Run button shows whole (§3.2); a longer one is a warning and
/// shows [`short_label`]-truncated, the full text kept as its tooltip.
pub const LABEL_MAX: usize = 40;

/// `label` cut to [`LABEL_MAX`] characters with `…` as the last one when longer.
pub fn short_label(label: &str) -> String {
    if label.chars().count() <= LABEL_MAX {
        return label.to_string();
    }
    let mut s: String = label.chars().take(LABEL_MAX - 1).collect();
    s.push('…');
    s
}

/// Runnable fence languages (§3.1); every other language is never a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    /// `bash`.
    Bash,
    /// `zsh`.
    Zsh,
    /// `sh`.
    Sh,
    /// `pwsh` (alias `powershell`).
    Pwsh,
    /// `python` (alias `py`).
    Python,
    /// `javascript` (alias `js`); header prefix `//|`.
    Javascript,
    /// `typescript` (alias `ts`); header prefix `//|`.
    Typescript,
}

impl Lang {
    /// The comment prefix header lines start with.
    fn prefix(self) -> &'static str {
        match self {
            Lang::Javascript | Lang::Typescript => "//|",
            _ => "#|",
        }
    }

    /// The canonical language word (`bash`, `pwsh`, …): [`lang_of`] reads it back.
    pub fn word(self) -> &'static str {
        match self {
            Lang::Bash => "bash",
            Lang::Zsh => "zsh",
            Lang::Sh => "sh",
            Lang::Pwsh => "pwsh",
            Lang::Python => "python",
            Lang::Javascript => "javascript",
            Lang::Typescript => "typescript",
        }
    }
}

/// The runnable language of an info string: its first whitespace-separated word,
/// case-insensitive, aliases included. `None` = not a cell.
pub fn lang_of(info: &str) -> Option<Lang> {
    let word = info.split_whitespace().next()?.to_ascii_lowercase();
    Some(match word.as_str() {
        // BlockNote stores every shell code block (```` ```bash ````, `sh`, `zsh`) as `shellscript`.
        "bash" | "shellscript" => Lang::Bash,
        "zsh" => Lang::Zsh,
        "sh" => Lang::Sh,
        "pwsh" | "powershell" => Lang::Pwsh,
        "python" | "py" => Lang::Python,
        "javascript" | "js" => Lang::Javascript,
        "typescript" | "ts" => Lang::Typescript,
        _ => return None,
    })
}

/// Whether a fence with this info string could be a cell (it still needs a header).
pub fn is_runnable(info: &str) -> bool {
    lang_of(info).is_some()
}

/// One raw header line, in order. `value` is `None` for a bare key (`#| danger`).
///
/// A header line whose text is not `key` / `key: value` (`#| not a key!`) still belongs to
/// the header: it is recorded with `key` = its raw trimmed text, `value` = `None`, and the
/// same text lands in [`CellOptions::unknown`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HeaderLine {
    /// The key (`[a-z]+`, written in any case), or the raw line text when it does not parse
    /// as one.
    pub key: String,
    /// The value, trimmed, one pair of surrounding double quotes removed.
    pub value: Option<String>,
}

/// A parsed cell: the header as written and as typed, and the code that runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "CellSpec"))]
pub struct Cell {
    /// Language word of the fence.
    pub lang: Lang,
    /// Header lines, raw and in order.
    pub header: Vec<HeaderLine>,
    /// The header, typed (§3.2). A value that fails its type leaves the field at its
    /// default; [`validate`] reports it from `header`.
    pub options: CellOptions,
    /// Everything after the header, LF line ends, one trailing newline removed.
    pub code: String,
    /// Lines before the code (leading blanks, a shebang, header and blank lines between
    /// header lines); UI code line 1 is the line after them.
    pub header_lines: u32,
}

/// Typed cell options (§3.2, §15.1). Repeated non-repeatable keys: the last one wins.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CellOptions {
    /// Folder the cell runs in, as written (absolute, `~/…`, or relative to a note default).
    pub cwd: Option<String>,
    /// Stable id, slug `[a-z0-9-]{1,48}`.
    pub name: Option<String>,
    /// Run button text; over [`LABEL_MAX`] characters it shows truncated (a warning).
    pub label: Option<String>,
    /// Red outline; never in Run all.
    pub danger: bool,
    /// Advisory: a press before the named cell's latest run succeeded asks to confirm.
    pub after: Option<String>,
    /// Output expectations, one per `expect` line.
    pub expect: Vec<Expect>,
    /// `KEY=value` pairs, one per `env` line.
    pub env: Vec<(String, String)>,
    /// Which interpreter runs a python cell.
    pub python: Option<PythonChoice>,
    /// The agent wakes when the run ends.
    pub review: bool,
    /// The agent gets exit and expect only, never output.
    pub private: bool,
    /// `timeout` in seconds (`30s`, `10m`, `2h`).
    pub timeout_secs: Option<u64>,
    /// Reserved (wave 3): a sandbox VM or mesh engine.
    pub on: Option<String>,
    /// Unknown keys (and unparsable header lines), in order.
    pub unknown: Vec<String>,
}

/// One `expect` item: text that must appear in the cleaned output, or a `/regex/`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Expect {
    /// Literal text.
    Text(String),
    /// Regex source, the surrounding slashes stripped.
    Regex(String),
}

/// The `python:` option (§6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PythonChoice {
    /// The built-in Monty sandbox.
    Monty,
    /// Discover per scope (§6.1).
    Auto,
    /// `.venv` beside the cwd.
    Venv,
    /// `uv python find` in the nearest project.
    Uv,
    /// An explicit interpreter path (`/…` or `~/…`).
    Path(PathBuf),
}

/// Note-level defaults from the first ```` ```cells ```` block (§3.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NoteDefaults {
    /// Folder relative cell `cwd`s resolve against.
    pub cwd: Option<String>,
    /// The program that runs `bash`/`zsh`/`sh` cells in this note.
    pub shell: Option<String>,
    /// Default interpreter for python cells.
    pub python: Option<PythonChoice>,
    /// Unknown keys, in order.
    pub unknown: Vec<String>,
}

/// How bad a [`Problem`] is: errors make the cell invalid, warnings are chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The cell renders as invalid, no Run button; in chat the agent gets `<cell_error>`.
    Error,
    /// A chip on the cell.
    Warning,
}

/// One validation finding (§3.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Problem {
    /// Code from the §3.6 tables, e.g. `cwd_missing`.
    pub code: String,
    /// Error or warning.
    pub severity: Severity,
    /// Short human text.
    pub message: String,
    /// The header key this is about, when there is one.
    pub key: Option<String>,
}

/// What [`validate`] knows about the cell's surroundings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ValidateCtx {
    /// The cell is in a chat reply (`cwd` required) rather than a note.
    pub chat: bool,
    /// Every cell `name` in scope, this cell's own included once per occurrence.
    pub names: Vec<String>,
    /// `(name, after)` of the other named cells in scope, for cycle detection.
    pub after_edges: Vec<(String, String)>,
    /// Vault entry names; `None` = vault unknown, skip `secret_unknown`.
    pub secrets: Option<Vec<String>>,
    /// The note's defaults; `None` in chat or without a `cells` block.
    pub defaults: Option<NoteDefaults>,
    /// Monty's sufficiency check for this cell; `None` = not checked.
    pub monty_ok: Option<bool>,
    /// Why Monty can't run it (with `monty_ok: Some(false)`); `None` = no reason known.
    #[serde(default)]
    pub monty_why: Option<String>,
    /// Whether the cwd exists; `None` = unknown.
    pub cwd_exists: Option<bool>,
    /// The note has a second `cells` block.
    pub second_defaults: bool,
}

/// What a lint chip says (§3.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ChipKind {
    /// Zero-width / bidi / BOM characters anywhere in the cell.
    Invisible,
    /// `curl … | sh` and friends.
    PipeToShell,
    /// `base64 -d … | sh`, `exec(b64decode(…))`.
    DecodedBase64,
    /// Invokes a file the agent edited this session.
    EditedScript,
    /// `--yes`-style flags on a `danger` cell.
    SkipsPrompt,
    /// `sudo`.
    Sudo,
    /// `{{secret:NAME}}`.
    Secret,
}

/// One lint chip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Chip {
    /// Which chip.
    pub kind: ChipKind,
    /// Chip text.
    pub text: String,
    /// Run all stops before this cell.
    pub stops_run_all: bool,
    /// Count (invisible characters).
    pub count: Option<u32>,
    /// Secret name or script path.
    pub name: Option<String>,
}

// ---- parsing ----

/// `\r\n` and lone `\r` to `\n`.
fn normalize(body: &str) -> String {
    body.replace("\r\n", "\n").replace('\r', "\n")
}

/// `key` / `key: value` → (key, value); `None` when the text is not a header grammar line.
fn split_header(text: &str) -> Option<(String, Option<String>)> {
    let (key, rest) = match text.find(':') {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    };
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    // Keys are case-insensitive (`#| Label: x`); values keep their case.
    let key = key.to_ascii_lowercase();
    let value = rest.map(|v| {
        let v = v.trim();
        match v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            Some(inner) => inner.to_string(),
            None => v.to_string(),
        }
    });
    Some((key, value))
}

fn header_line(text: &str) -> HeaderLine {
    let text = text.trim();
    match split_header(text) {
        Some((key, value)) => HeaderLine { key, value },
        None => HeaderLine {
            key: text.to_string(),
            value: None,
        },
    }
}

/// A typed header line.
enum Opt {
    Cwd(String),
    Name(String),
    Label(String),
    Danger(bool),
    After(String),
    Expect(Expect),
    Env(String, String),
    Python(PythonChoice),
    Review(bool),
    Private(bool),
    Timeout(u64),
    On(String),
    Unknown,
}

fn non_empty(v: Option<&str>) -> Result<String, &'static str> {
    match v {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        _ => Err("bad_option"),
    }
}

fn flag(v: Option<&str>) -> Result<bool, &'static str> {
    match v {
        None | Some("true" | "yes" | "on") => Ok(true),
        Some("false" | "no" | "off") => Ok(false),
        Some(_) => Err("bad_option"),
    }
}

/// `30s` / `10m` / `2h` in seconds.
pub fn parse_duration(text: &str) -> Option<u64> {
    let unit = text.chars().last()?;
    let n: u64 = text[..text.len() - unit.len_utf8()].parse().ok()?;
    match unit {
        's' => Some(n),
        'm' => n.checked_mul(60),
        'h' => n.checked_mul(3600),
        _ => None,
    }
}

fn parse_python(v: &str) -> Option<PythonChoice> {
    Some(match v {
        "monty" => PythonChoice::Monty,
        "auto" => PythonChoice::Auto,
        ".venv" => PythonChoice::Venv,
        "uv" => PythonChoice::Uv,
        p if p.starts_with('/') || p.starts_with("~/") => PythonChoice::Path(PathBuf::from(p)),
        _ => return None,
    })
}

static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9-]{1,48}$").expect("slug re"));
static ENV_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("env key re"));

/// Type one header line. `Err(code)` = known key, value fails its type (`bad_option` or
/// `python_option`).
fn typed(line: &HeaderLine) -> Result<Opt, &'static str> {
    let v = line.value.as_deref();
    Ok(match line.key.as_str() {
        "cwd" => Opt::Cwd(non_empty(v)?),
        "name" => {
            let n = non_empty(v)?;
            if !SLUG.is_match(&n) {
                return Err("bad_option");
            }
            Opt::Name(n)
        }
        // Any length: a long label is a warning (`label_long`) and shows truncated.
        "label" => Opt::Label(non_empty(v)?),
        "danger" => Opt::Danger(flag(v)?),
        "review" => Opt::Review(flag(v)?),
        "private" => Opt::Private(flag(v)?),
        "after" => Opt::After(non_empty(v)?),
        "on" => Opt::On(non_empty(v)?),
        "expect" => {
            let e = non_empty(v)?;
            match e.strip_prefix('/').and_then(|e| e.strip_suffix('/')) {
                Some(re) if !re.is_empty() => {
                    Regex::new(re).map_err(|_| "bad_option")?;
                    Opt::Expect(Expect::Regex(re.to_string()))
                }
                Some(_) => return Err("bad_option"),
                None => Opt::Expect(Expect::Text(e)),
            }
        }
        "env" => {
            let e = non_empty(v)?;
            let (k, val) = e.split_once('=').ok_or("bad_option")?;
            if !ENV_KEY.is_match(k) {
                return Err("bad_option");
            }
            Opt::Env(k.to_string(), val.to_string())
        }
        "python" => Opt::Python(v.and_then(parse_python).ok_or("python_option")?),
        "timeout" => Opt::Timeout(v.and_then(parse_duration).ok_or("bad_option")?),
        _ => Opt::Unknown,
    })
}

fn fill(header: &[HeaderLine]) -> CellOptions {
    let mut o = CellOptions::default();
    for line in header {
        match typed(line) {
            Ok(Opt::Cwd(v)) => o.cwd = Some(v),
            Ok(Opt::Name(v)) => o.name = Some(v),
            Ok(Opt::Label(v)) => o.label = Some(v),
            Ok(Opt::Danger(v)) => o.danger = v,
            Ok(Opt::After(v)) => o.after = Some(v),
            Ok(Opt::Expect(v)) => o.expect.push(v),
            Ok(Opt::Env(k, v)) => o.env.push((k, v)),
            Ok(Opt::Python(v)) => o.python = Some(v),
            Ok(Opt::Review(v)) => o.review = v,
            Ok(Opt::Private(v)) => o.private = v,
            Ok(Opt::Timeout(v)) => o.timeout_secs = Some(v),
            Ok(Opt::On(v)) => o.on = Some(v),
            Ok(Opt::Unknown) => o.unknown.push(line.key.clone()),
            Err(_) => {}
        }
    }
    o
}

/// Whether a header-region line is blank (only whitespace).
fn blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Parse one fence (`lang` = info string, `body` = text between the fence lines) into a
/// cell. `None` when the language is not runnable or the body has no header line.
///
/// The header may follow leading blank lines and one shebang line (`#!…`); blank lines
/// between header lines stay in the header, which ends at the first line that is neither a
/// header line nor blank (blank lines after the last header line belong to the code).
/// `header_lines` counts every line before the code.
pub fn parse(lang: &str, body: &str) -> Option<Cell> {
    let lang = lang_of(lang)?;
    let prefix = lang.prefix();
    let body = normalize(body);
    let lines: Vec<&str> = body.split('\n').collect();
    let mut start = 0;
    let mut shebang = false;
    while let Some(l) = lines.get(start) {
        if blank(l) {
            start += 1;
        } else if !shebang && l.starts_with("#!") {
            shebang = true;
            start += 1;
        } else {
            break;
        }
    }
    let mut header = Vec::new();
    let mut end = start;
    for (i, l) in lines.iter().enumerate().skip(start) {
        if let Some(rest) = l.strip_prefix(prefix) {
            header.push(header_line(rest));
            end = i + 1;
        } else if !blank(l) {
            break;
        }
    }
    if header.is_empty() {
        return None;
    }
    let mut code = lines[end..].join("\n");
    if code.ends_with('\n') {
        code.pop();
    }
    Some(Cell {
        lang,
        options: fill(&header),
        header_lines: end as u32,
        header,
        code,
    })
}

/// Read a note's ```` ```cells ```` block body: every `#| key: value` line; other lines are
/// ignored; unknown keys (and values that fail their type) go to `unknown`.
pub fn parse_defaults(body: &str) -> NoteDefaults {
    let mut d = NoteDefaults::default();
    for line in normalize(body).lines() {
        let Some(rest) = line.strip_prefix("#|") else {
            continue;
        };
        let line = header_line(rest);
        match (line.key.as_str(), line.value.as_deref()) {
            ("cwd", Some(v)) if !v.is_empty() => d.cwd = Some(v.to_string()),
            ("shell", Some(v)) if !v.is_empty() => d.shell = Some(v.to_string()),
            ("python", Some(v)) if parse_python(v).is_some() => d.python = parse_python(v),
            _ => d.unknown.push(line.key.clone()),
        }
    }
    d
}

/// SHA-256 (lower-case hex) over the fence body with LF line ends and one final newline
/// trimmed (§3.4).
pub fn digest(body: &str) -> String {
    let text = normalize(body);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    platform::sha256::hex(text.as_bytes())
}

// ---- validation ----

static SECRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{secret:([A-Za-z0-9_.\-]+)\}\}").expect("secret re"));
static CHANGING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\bpulumi\s+up\b|\bterraform\s+(?:apply|destroy)\b|\bkubectl\s+(?:apply|delete)\b|\bhelm\s+(?:upgrade|uninstall)\b|\baws\s[^\n|;&]*\b(?:delete|put|create)-[a-z]",
    )
    .expect("changing re")
});

/// Distinct `{{secret:NAME}}` names in `text`, in order of first appearance.
fn secret_names(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in SECRET.captures_iter(text) {
        let name = &c[1];
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// The header as text (values only, joined), for secret scans.
fn header_text(cell: &Cell) -> String {
    let mut s = String::new();
    for l in &cell.header {
        s.push_str(&l.key);
        if let Some(v) = &l.value {
            s.push(' ');
            s.push_str(v);
        }
        s.push('\n');
    }
    s
}

fn is_absolute_cwd(cwd: &str) -> bool {
    cwd.starts_with('/') || cwd == "~" || cwd.starts_with("~/")
}

/// Walk `after` edges from `start`; true when the chain reaches `target`.
fn reaches(edges: &[(String, String)], start: &str, target: &str) -> bool {
    let mut cur = start;
    for _ in 0..=edges.len() {
        if cur == target {
            return true;
        }
        match edges.iter().find(|(n, _)| n == cur) {
            Some((_, next)) => cur = next,
            None => return false,
        }
    }
    false
}

/// The §3.6 problems of a cell: errors first (table order), then warnings.
pub fn validate(cell: &Cell, ctx: &ValidateCtx) -> Vec<Problem> {
    let mut out = Vec::new();
    let err = |code: &str, message: String, key: Option<&str>| Problem {
        code: code.to_string(),
        severity: Severity::Error,
        message,
        key: key.map(str::to_string),
    };
    let warn = |code: &str, message: String, key: Option<&str>| Problem {
        code: code.to_string(),
        severity: Severity::Warning,
        message,
        key: key.map(str::to_string),
    };
    let o = &cell.options;
    let default_cwd = ctx.defaults.as_ref().and_then(|d| d.cwd.as_deref());
    let has_cwd_line = cell.header.iter().any(|l| l.key == "cwd");

    match &o.cwd {
        None if !has_cwd_line && default_cwd.is_none() => {
            let msg = if ctx.chat {
                "a chat cell needs `cwd`".to_string()
            } else {
                "needs `cwd` or a note default".to_string()
            };
            out.push(err("cwd_missing", msg, Some("cwd")));
        }
        Some(c) if !is_absolute_cwd(c) && default_cwd.is_none() => out.push(err(
            "cwd_relative",
            format!("`cwd: {c}` is relative and there is no note default to resolve it against"),
            Some("cwd"),
        )),
        _ => {}
    }

    // Problems that never stop a run (a bad value of an advisory key is ignored, `after` is
    // advisory): warnings, listed first among the warnings below.
    let mut soft = Vec::new();
    for line in &cell.header {
        if line.key == "python" && cell.lang != Lang::Python {
            soft.push(warn(
                "python_option",
                "`python:` only applies to python cells; ignored".to_string(),
                Some("python"),
            ));
            continue;
        }
        if let Err(code) = typed(line) {
            let shown = line.value.as_deref().unwrap_or("");
            let key = line.key.as_str();
            let hint = match key {
                "name" => " (1–48 lowercase letters, digits or dashes)",
                "danger" | "review" | "private" => " (use true or false, or the bare key)",
                "expect" => " (text, or a /regex/ that compiles)",
                "env" => " (use KEY=value with a shell variable name)",
                "timeout" => " (a number with s, m or h, e.g. 30s or 10m)",
                _ => "",
            };
            let advisory = matches!(
                key,
                "name" | "label" | "review" | "private" | "after" | "expect" | "timeout" | "on"
            );
            let tail = if advisory { "; ignored" } else { "" };
            let msg = match code {
                "python_option" => {
                    format!("`python: {shown}` is not monty, auto, .venv, uv or a path")
                }
                _ if line.value.is_none() => format!("`{key}` needs a value{tail}"),
                _ => format!("`{key}: {shown}` is not a valid {key}{hint}{tail}"),
            };
            if advisory {
                soft.push(warn(code, msg, Some(key)));
            } else {
                out.push(err(code, msg, Some(key)));
            }
        }
    }

    if let Some(l) = &o.label {
        let n = l.chars().count();
        if n > LABEL_MAX {
            soft.push(warn(
                "label_long",
                format!("label is {n} characters; shown truncated to {LABEL_MAX}"),
                Some("label"),
            ));
        }
    }

    // `after` is advisory (#1): an unknown or looping name is a warning; the cell runs.
    if let Some(a) = &o.after {
        let own = o.name.as_deref();
        if own == Some(a.as_str()) {
            soft.push(warn(
                "after_cycle",
                format!("`after: {a}` names this cell; ignored"),
                Some("after"),
            ));
        } else if !ctx.names.iter().any(|n| n == a) {
            soft.push(warn("after_unknown", after_unknown(a, ctx), Some("after")));
        } else if own.is_some_and(|me| reaches(&ctx.after_edges, a, me)) {
            soft.push(warn(
                "after_cycle",
                format!("`after: {a}` loops back to this cell; ignored"),
                Some("after"),
            ));
        }
    }

    if let Some(n) = &o.name
        && ctx.names.iter().filter(|x| *x == n).count() >= 2
    {
        out.push(err(
            "name_duplicate",
            format!("two cells are named `{n}`"),
            Some("name"),
        ));
    }

    if let Some(vault) = &ctx.secrets {
        for name in secret_names(&header_text(cell)) {
            if !vault.contains(&name) {
                out.push(err(
                    "secret_unknown",
                    format!("no secret named `{name}`"),
                    Some("env"),
                ));
            }
        }
    }

    if SECRET.is_match(&cell.code) {
        out.push(err(
            "secret_in_code",
            "a `{{secret:…}}` placeholder in the code; put it in `#| env:` and use `$NAME`"
                .to_string(),
            None,
        ));
    }

    let monty = o
        .python
        .as_ref()
        .or(ctx.defaults.as_ref().and_then(|d| d.python.as_ref()));
    if monty == Some(&PythonChoice::Monty) && ctx.monty_ok == Some(false) {
        let msg = match &ctx.monty_why {
            Some(why) => format!("Monty can't run this cell: {why}"),
            None => "Monty can't run this cell".to_string(),
        };
        out.push(err("monty_insufficient", msg, Some("python")));
    }

    if cell.code.trim().is_empty() {
        out.push(err("empty", "no code after the header".to_string(), None));
    }

    out.extend(soft);
    for k in &o.unknown {
        out.push(warn(
            "unknown_option",
            format!("unknown option `{k}`; ignored"),
            Some(k),
        ));
    }
    if o.on.is_some() {
        out.push(warn(
            "on_not_yet",
            "`on:` is not yet supported".to_string(),
            Some("on"),
        ));
    }
    if ctx.cwd_exists == Some(false) {
        out.push(warn(
            "cwd_not_found",
            "arms when the folder exists".to_string(),
            Some("cwd"),
        ));
    }
    if ctx.second_defaults {
        out.push(warn(
            "second_defaults",
            "a second `cells` block is ignored".to_string(),
            None,
        ));
    }
    if o.label.is_none() && !o.danger && CHANGING.is_match(&cell.code) {
        out.push(warn(
            "changing_without_label",
            "a likely-changing command with neither `label` nor `danger`".to_string(),
            None,
        ));
    }
    if o.python == Some(PythonChoice::Monty) && o.timeout_secs.is_some_and(|t| t > 60) {
        out.push(warn(
            "monty_timeout",
            "Monty runs are capped at 60 s".to_string(),
            Some("timeout"),
        ));
    }
    out
}

// ---- lint ----

static PIPE_TO_SHELL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\b(?:curl|wget|iwr)\b[^\n|]*\|\s*(?:sudo\s+)?(?:sh|bash|zsh|python3?|iex)\b|\b(?:sh|bash|zsh|python3?)\s+(?:-c\s+)?["']?\$\((?:curl|wget|iwr)\b|\b(?:sh|bash|zsh|python3?)\s+<\((?:curl|wget|iwr)\b"#,
    )
    .expect("pipe re")
});
static BASE64_RUN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:base64\s+(?:-d|--decode)|b64decode)\b[^\n]*\|\s*(?:sudo\s+)?(?:sh|bash|zsh|eval|exec|iex)\b|\b(?:eval|exec|iex)\b[^\n]*(?:base64\s+(?:-d|--decode)|b64decode)\b",
    )
    .expect("base64 re")
});
static SKIPS_PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)(?:^|\s)(?:--yes|-y|-auto-approve|--auto-approve|--force|--skip-preview)(?:\s|$)",
    )
    .expect("skips re")
});
static SUDO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)(?:^|[\s;&|(`])sudo(?:\s|$)").expect("sudo re"));

fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

fn chip(kind: ChipKind, text: &str, stops_run_all: bool) -> Chip {
    Chip {
        kind,
        text: text.to_string(),
        stops_run_all,
        count: None,
        name: None,
    }
}

/// The §3.7 text chips over header and code (every kind but [`ChipKind::EditedScript`]).
pub fn lint(cell: &Cell) -> Vec<Chip> {
    let mut out = Vec::new();
    let header = header_text(cell);
    let invisible = header
        .chars()
        .chain(cell.code.chars())
        .filter(|c| is_invisible(*c))
        .count();
    if invisible > 0 {
        out.push(Chip {
            count: Some(invisible as u32),
            ..chip(
                ChipKind::Invisible,
                &format!("{invisible} invisible characters"),
                true,
            )
        });
    }
    if PIPE_TO_SHELL.is_match(&cell.code) {
        out.push(chip(
            ChipKind::PipeToShell,
            "pipes a download into a shell",
            true,
        ));
    }
    if BASE64_RUN.is_match(&cell.code) {
        out.push(chip(ChipKind::DecodedBase64, "runs decoded base64", true));
    }
    if cell.options.danger && SKIPS_PROMPT.is_match(&cell.code) {
        out.push(chip(
            ChipKind::SkipsPrompt,
            "skips the tool's own prompt",
            false,
        ));
    }
    if SUDO.is_match(&cell.code) {
        out.push(chip(ChipKind::Sudo, "asks for your password", false));
    }
    let mut text = header;
    text.push_str(&cell.code);
    for name in secret_names(&text) {
        out.push(Chip {
            name: Some(name.clone()),
            ..chip(ChipKind::Secret, &format!("uses secret {name}"), false)
        });
    }
    out
}

static SCRIPT_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)(?:^|[\s;&|(`])(?:(\.\.?/[^\s;&|)`]+)|(?:bash|sh|zsh|source|python3?|node|bun|pwsh|deno)\s+(?:-\S+\s+)*([^\s;&|)`-][^\s;&|)`]*)|(make)\b|(npm\s+run)\b)",
    )
    .expect("script re")
});

/// Whether `written` (as in the code, maybe relative to `cwd`) is one of `edited`.
///
/// ponytail: component-suffix match either way round; a real resolver would canonicalise.
fn script_edited(written: &str, cwd: Option<&str>, edited: &[PathBuf]) -> bool {
    let rel = Path::new(written.strip_prefix("./").unwrap_or(written));
    let mut candidates = vec![rel.to_path_buf()];
    if rel.is_relative()
        && let Some(cwd) = cwd
    {
        let cwd = cwd.strip_prefix("~/").unwrap_or(cwd);
        candidates.push(Path::new(cwd).join(rel));
    }
    edited
        .iter()
        .any(|e| candidates.iter().any(|c| e.ends_with(c) || c.ends_with(e)))
}

/// [`ChipKind::EditedScript`] chips: the code invokes a file (`./x.sh`, `bash x`, `python
/// x.py`, `make t` → `Makefile`, `npm run t` → `package.json`) that is in `edited`
/// (paths the session's agent wrote). `name` = the path as written.
pub fn lint_scripts(cell: &Cell, edited: &[PathBuf]) -> Vec<Chip> {
    let mut out: Vec<Chip> = Vec::new();
    for c in SCRIPT_CALL.captures_iter(&cell.code) {
        let written = if let Some(m) = c.get(1).or(c.get(2)) {
            m.as_str()
        } else if c.get(3).is_some() {
            "Makefile"
        } else {
            "package.json"
        };
        if out.iter().any(|ch| ch.name.as_deref() == Some(written)) {
            continue;
        }
        if script_edited(written, cell.options.cwd.as_deref(), edited) {
            out.push(Chip {
                name: Some(written.to_string()),
                ..chip(ChipKind::EditedScript, "calls a script theuth edited", true)
            });
        }
    }
    out
}

// ---- Monty ----

/// Modules Monty's stdlib provides (the list `crates/agent/src/repl.rs` teaches).
const MONTY_STDLIB: &[&str] = &[
    "json",
    "re",
    "math",
    "datetime",
    "time",
    "random",
    "copy",
    "collections",
    "itertools",
    "functools",
    "dataclasses",
    "pathlib",
    "base64",
    "hashlib",
    "decimal",
    "bisect",
    "csv",
    "tomllib",
    "os",
    "asyncio",
];

static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:from\s+([\w.]+)\s+import\b|import\s+([^\n#]+))").expect("import re")
});
static MONTY_BANNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:open|exec|eval)\(|\bsubprocess\b|\bsocket\b|\burllib\b|\brequests\b|\bos\.system\b|\bos\.popen\b")
        .expect("banned re")
});

/// The static half of Monty sufficiency (§6.2 a): `Err(reason)` when an import is outside
/// Monty's stdlib or the code opens files, runs processes, touches the network or evals.
/// The reason names its line (`line N: …`) and the way out.
pub fn monty_scan(code: &str) -> Result<(), String> {
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    for c in IMPORT.captures_iter(code) {
        let Some(g) = c.get(1).or_else(|| c.get(2)) else {
            continue;
        };
        let modules: Vec<&str> = match c.get(1) {
            Some(m) => vec![m.as_str()],
            None => g.as_str().split(',').collect(),
        };
        for m in modules {
            let root = m.trim().split([' ', '.']).next().unwrap_or("");
            if root.is_empty() {
                continue;
            }
            if !MONTY_STDLIB.contains(&root) {
                return Err(format!(
                    "line {}: no module named {root} — Monty runs the standard library only; \
                     use a local python",
                    line_of(g.start())
                ));
            }
        }
    }
    if let Some(m) = MONTY_BANNED.find(code) {
        return Err(format!(
            "line {}: uses `{}` — Monty has no files, processes or network; use a local python",
            line_of(m.start()),
            m.as_str()
        ));
    }
    Ok(())
}

/// The reason in a Monty type-check `diagnostics` text (§6.2 b) for a cell whose code starts
/// `line_offset` lines into the checked file: the first error that is not an
/// `unresolved-reference` (a name an earlier cell may have bound in the kernel) as
/// `line N: <message>`, `None` when nothing else remains (sufficient). Reads the full
/// (`error[rule]: msg` + ` --> file:L:C`) and the concise (`file:L:C: error[rule] msg`)
/// renderings; text with no recognisable diagnostic is its own reason (first line).
pub fn monty_reason(diagnostics: &str, line_offset: usize) -> Option<String> {
    let lines: Vec<&str> = diagnostics.lines().map(str::trim).collect();
    // `path:L:C` → L.
    let line_no = |loc: &str| -> Option<usize> {
        let mut it = loc.trim().rsplitn(3, ':');
        let _col = it.next()?;
        it.next()?.parse().ok()
    };
    let mut found = false;
    for (i, l) in lines.iter().enumerate() {
        let (loc, rest) = if let Some(rest) = l.strip_prefix("error[") {
            let loc = lines[i + 1..]
                .iter()
                .take_while(|n| !n.starts_with("error[") && !n.starts_with("warning["))
                .find_map(|n| n.strip_prefix("-->"));
            (loc, rest)
        } else if let Some((loc, rest)) = l.split_once(": error[") {
            (Some(loc), rest)
        } else {
            continue;
        };
        let Some((rule, msg)) = rest.split_once(']') else {
            continue;
        };
        found = true;
        if rule == "unresolved-reference" {
            continue;
        }
        let msg = msg.trim_start_matches(':').trim();
        return Some(match loc.and_then(line_no) {
            Some(n) if n > line_offset => format!("line {}: {msg}", n - line_offset),
            _ => msg.to_string(),
        });
    }
    if found {
        return None;
    }
    Some(
        lines
            .into_iter()
            .find(|l| !l.is_empty())
            .unwrap_or("Monty's type check failed")
            .to_string(),
    )
}

/// B30: the `after_unknown` text — at most 5 of the scope's names, closest to `a` first,
/// or "no cell named" when none is close.
fn after_unknown(a: &str, ctx: &ValidateCtx) -> String {
    let mut names: Vec<&str> = Vec::new();
    for n in &ctx.names {
        if !names.contains(&n.as_str()) {
            names.push(n);
        }
    }
    let want = a.to_lowercase();
    let rank = |n: &str| {
        let n = n.to_lowercase();
        let prefix = n.starts_with(&want) || want.starts_with(&n);
        (strsim::levenshtein(&n, &want), !prefix)
    };
    let close = |n: &str| {
        let (d, far) = rank(n);
        !far || d <= (want.chars().count() / 3).max(2)
    };
    if !names.iter().any(|n| close(n)) {
        let place = if ctx.chat { "chat" } else { "note" };
        return format!("`after: {a}`: no cell named `{a}` in this {place}");
    }
    names.sort_by_key(|n| rank(n)); // stable: equals keep the scope's order
    let mut list = names.iter().take(5).copied().collect::<Vec<_>>().join(", ");
    if names.len() > 5 {
        list.push_str(&format!(" … and {} more", names.len() - 5));
    }
    format!("`after: {a}` names no cell in scope; cells here: {list}")
}
