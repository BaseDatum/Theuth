//! Placement surfaces (design 216 §9.2–§9.4): `status` / `ui` params, ui validation (`app::helper`,
//! `surface::parse`, no masked inputs), the per-plugin board of statuses and contents, the state
//! override of snapshots, and the token-bucket rate limits.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use app::helper::{Helper, ToolSnapshot};
use serde_json::{Map, Value};

use crate::error::PluginsError;
use crate::manifest::PlacementDecl;
use crate::types::{
    Badge, BindingKind, CheckIssue, Dot, PluginRunState, SurfaceBinding, SurfaceContent,
    SurfaceFormat, SurfaceLoad, SurfaceSnapshot,
};

/// Largest surface held (source + text bytes), design §9.2.
pub(crate) const SURFACE_MAX: usize = 256 * 1024;
/// Largest total of one plugin's surfaces, design §9.2.
pub(crate) const PLUGIN_MAX: usize = 4 * 1024 * 1024;

/// Longest string badge, characters (§6.3).
const BADGE_MAX: usize = 4;
/// Longest label, characters (§6.3).
const LABEL_MAX: usize = 16;
/// Longest tooltip, characters (§6.3).
const TOOLTIP_MAX: usize = 200;
/// The refusal of a masked `Input` (§9.4, review F10).
const MASKED: &str = "masked inputs are not allowed in plugin surfaces";

/// A placement's latest `status` (§6.3, §9.3).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Status {
    /// The dot.
    pub dot: Dot,
    /// A count or ≤ 4 characters.
    pub badge: Option<Badge>,
    /// ≤ 16 characters.
    pub label: Option<String>,
    /// ≤ 200 characters.
    pub tooltip: Option<String>,
}

/// `PluginsError::Invalid` located at `path`.
fn invalid(path: &str, message: &str) -> PluginsError {
    PluginsError::Invalid {
        message: if path.is_empty() {
            message.to_owned()
        } else {
            format!("{path}: {message}")
        },
    }
}

fn object<'a>(params: &'a Value, method: &str) -> Result<&'a Map<String, Value>, PluginsError> {
    params
        .as_object()
        .ok_or_else(|| invalid("", &format!("{method} params must be an object")))
}

fn required_str(obj: &Map<String, Value>, key: &str) -> Result<String, PluginsError> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(invalid(&format!("/{key}"), "must be a string")),
        None => Err(invalid(&format!("/{key}"), "is required")),
    }
}

fn optional_str(
    obj: &Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, PluginsError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.chars().count() <= max => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(invalid(
            &format!("/{key}"),
            &format!("must be at most {max} characters"),
        )),
        Some(_) => Err(invalid(&format!("/{key}"), "must be a string")),
    }
}

/// Parse `status` params: `{placement, dot, badge?, label?, tooltip?}` (§6.3).
pub(crate) fn parse_status(params: &Value) -> Result<(String, Status), PluginsError> {
    let obj = object(params, "status")?;
    let placement = required_str(obj, "placement")?;
    let dot = match obj.get("dot") {
        None => return Err(invalid("/dot", "is required")),
        Some(v) => serde_json::from_value::<Dot>(v.clone())
            .map_err(|_| invalid("/dot", "must be one of ok, busy, warn, error, off, none"))?,
    };
    let bad_badge = || {
        invalid(
            "/badge",
            &format!(
                "must be a non-negative integer or a string of at most {BADGE_MAX} characters"
            ),
        )
    };
    let badge = match obj.get("badge") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(Badge::Count(n.as_u64().ok_or_else(bad_badge)?)),
        Some(Value::String(s)) if s.chars().count() <= BADGE_MAX => Some(Badge::Text(s.clone())),
        Some(_) => return Err(bad_badge()),
    };
    let label = optional_str(obj, "label", LABEL_MAX)?;
    let tooltip = optional_str(obj, "tooltip", TOOLTIP_MAX)?;
    Ok((
        placement,
        Status {
            dot,
            badge,
            label,
            tooltip,
        },
    ))
}

/// Parsed `ui` params (§6.3).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UiParams {
    /// Placement id.
    pub placement: String,
    /// Content format.
    pub format: SurfaceFormat,
    /// The source (validated by [`validate`] before it is held).
    pub source: String,
    /// The required text fallback.
    pub text: String,
}

/// Parse `ui` params: `{placement, format, source, text}`, all required (§6.3).
pub(crate) fn parse_ui(params: &Value) -> Result<UiParams, PluginsError> {
    let obj = object(params, "ui")?;
    let placement = required_str(obj, "placement")?;
    let format = match obj.get("format") {
        None => return Err(invalid("/format", "is required")),
        Some(v) => serde_json::from_value::<SurfaceFormat>(v.clone())
            .map_err(|_| invalid("/format", "must be one of openui, json-render, markdown"))?,
    };
    let source = required_str(obj, "source")?;
    let text = required_str(obj, "text")?;
    Ok(UiParams {
        placement,
        format,
        source,
        text,
    })
}

fn issue(path: &str, message: impl Into<String>) -> CheckIssue {
    CheckIssue {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// Validate a surface before clients see it (§9.4): openui → `Helper::check` with one
/// [`ToolSnapshot`] per `Query`/`Mutation` name (contract §12.4), refusing diagnostics and masked
/// `Input`s; json-render → `surface::parse`; markdown as is. `helper: None` (no validator available)
/// accepts openui with the bindings of its `name = Query("x", …)` / `name = Mutation("x", …)` statements.
/// The source is validated as redacted (design §7.4, review R7), the form [`Board::set_content`]
/// holds and compares.
pub(crate) async fn validate(
    helper: Option<&Helper>,
    plugin: &str,
    format: SurfaceFormat,
    source: &str,
) -> Result<Vec<SurfaceBinding>, CheckIssue> {
    let redacted = tools::secrets::redact(source);
    let source = redacted.as_ref();
    match format {
        SurfaceFormat::Markdown => Ok(Vec::new()),
        SurfaceFormat::JsonRender => surface::parse(source).map(|_| Vec::new()).map_err(|errs| {
            errs.into_iter().next().map_or_else(
                || issue("", "invalid json-render spec"),
                |e| issue(&e.path, e.message),
            )
        }),
        SurfaceFormat::Openui => {
            if let Some(i) = masked_input(source) {
                return Err(i);
            }
            let Some(helper) = helper else {
                return Ok(statement_bindings(source));
            };
            let snapshots: Vec<ToolSnapshot> = called_names(source)
                .into_iter()
                .map(|(name, kind)| ToolSnapshot {
                    identity: format!("plugin:{plugin}:{name}"),
                    name,
                    schema: serde_json::json!({ "type": "object" }),
                    risk: match kind {
                        BindingKind::Query => tools::Risk::ReadOnly,
                        BindingKind::Mutation => tools::Risk::Mutating,
                    },
                })
                .collect();
            let r = helper
                .check(source, &snapshots)
                .await
                .map_err(|e| issue("", e.to_string()))?;
            if let Some(d) = r.diagnostics.into_iter().next() {
                return Err(issue(&d.at, d.message));
            }
            if let Some(i) = masked_input(&r.source) {
                return Err(i);
            }
            r.bindings
                .into_iter()
                .map(|b| {
                    let kind = match b.kind.as_str() {
                        "query" => BindingKind::Query,
                        "mutation" => BindingKind::Mutation,
                        other => {
                            return Err(issue(
                                &b.statement_id,
                                format!("unknown binding kind {other:?}"),
                            ));
                        }
                    };
                    Ok(SurfaceBinding {
                        statement_id: b.statement_id,
                        name: b.tool,
                        kind,
                    })
                })
                .collect()
        }
    }
}

/// An OpenUI `Input(…)` call with a `"password"` argument (or any other string argument spelling
/// `password`, as in `type: "password"`) → the refusal, located at `/source` (§9.4). An identifier
/// argument is resolved through the `name = …` statements it names, transitively, so
/// `t = "password"` then `Input("pw", t)` is refused too (review R10).
pub(crate) fn masked_input(source: &str) -> Option<CheckIssue> {
    let toks = lex(source);
    let statements = statements(source);
    calls(&toks, "Input")
        .any(|(open, close)| {
            let mut seen = BTreeSet::new();
            spells_password(&toks[open + 1..close], &statements, &mut seen)
        })
        .then(|| issue("/source", MASKED))
}

/// Whether `toks` hold a `"password"` string, directly or through a statement an identifier names.
fn spells_password<'a>(
    toks: &[Tok],
    statements: &'a BTreeMap<String, Vec<Tok>>,
    seen: &mut BTreeSet<&'a str>,
) -> bool {
    toks.iter().any(|t| match t {
        Tok::Str(s) => s.trim().eq_ignore_ascii_case("password"),
        Tok::Ident(id) => match statements.get_key_value(id.as_str()) {
            Some((name, body)) if seen.insert(name.as_str()) => {
                spells_password(body, statements, seen)
            }
            _ => false,
        },
        Tok::Punct(_) => false,
    })
}

/// `name = expr` statements, one per line (OpenUI Lang), as name → tokens of `expr`.
fn statements(source: &str) -> BTreeMap<String, Vec<Tok>> {
    source
        .lines()
        .filter_map(|line| {
            let (lhs, rhs) = line.split_once('=')?;
            let id = lhs.trim();
            let valid = !id.is_empty()
                && id.chars().all(ident_char)
                && !id.starts_with(|c: char| c.is_ascii_digit());
            (valid && !rhs.starts_with('=')).then(|| (id.to_owned(), lex(rhs)))
        })
        .collect()
}

/// One token of OpenUI source, enough to find calls and their string arguments.
#[derive(Debug, PartialEq)]
enum Tok {
    /// An identifier or a number.
    Ident(String),
    /// A decoded string literal.
    Str(String),
    /// Any other character.
    Punct(char),
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Tokenize: identifiers, `"…"`/`'…'` strings with backslash escapes, `//` comments skipped.
fn lex(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut it = src.chars().peekable();
    while let Some(c) = it.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '/' && it.peek() == Some(&'/') {
            for n in it.by_ref() {
                if n == '\n' {
                    break;
                }
            }
            continue;
        }
        if c == '"' || c == '\'' {
            let mut s = String::new();
            while let Some(n) = it.next() {
                match n {
                    '\\' => match it.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some(e) => s.push(e),
                        None => break,
                    },
                    n if n == c => break,
                    n => s.push(n),
                }
            }
            out.push(Tok::Str(s));
            continue;
        }
        if ident_char(c) {
            let mut s = String::from(c);
            while let Some(&n) = it.peek() {
                if !ident_char(n) {
                    break;
                }
                s.push(n);
                it.next();
            }
            out.push(Tok::Ident(s));
            continue;
        }
        out.push(Tok::Punct(c));
    }
    out
}

/// Every call of `name` (not a member call `x.name(`): (index of `(`, index of its matching `)`,
/// or the end when unbalanced).
fn calls<'a>(toks: &'a [Tok], name: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    toks.windows(2).enumerate().filter_map(move |(i, w)| {
        let is_call = matches!(&w[0], Tok::Ident(s) if s == name) && w[1] == Tok::Punct('(');
        let member = i > 0 && toks[i - 1] == Tok::Punct('.');
        if !is_call || member {
            return None;
        }
        let open = i + 1;
        let mut depth = 0usize;
        for (j, t) in toks.iter().enumerate().skip(open) {
            match t {
                Tok::Punct('(') => depth += 1,
                Tok::Punct(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((open, j));
                    }
                }
                _ => {}
            }
        }
        Some((open, toks.len()))
    })
}

/// Distinct names of `Query("…")` / `Mutation("…")` calls, first kind wins except that a name also
/// used as a `Mutation` is offered as one (a mutating snapshot binds either way).
fn called_names(source: &str) -> Vec<(String, BindingKind)> {
    let toks = lex(source);
    let mut out: Vec<(String, BindingKind)> = Vec::new();
    for (word, kind) in [
        ("Mutation", BindingKind::Mutation),
        ("Query", BindingKind::Query),
    ] {
        for (open, _) in calls(&toks, word) {
            if let Some(Tok::Str(name)) = toks.get(open + 1)
                && !out.iter().any(|(n, _)| n == name)
            {
                out.push((name.clone(), kind));
            }
        }
    }
    out
}

/// Bindings without a validator: each line `id = Query("x"…)` / `id = Mutation("x"…)`.
fn statement_bindings(source: &str) -> Vec<SurfaceBinding> {
    source
        .lines()
        .filter_map(|line| {
            let (lhs, rhs) = line.split_once('=')?;
            let id = lhs.trim();
            if id.is_empty()
                || !id.chars().all(ident_char)
                || id.starts_with(|c: char| c.is_ascii_digit())
            {
                return None;
            }
            let rhs = rhs.trim_start();
            let (kind, rest) = if let Some(r) = rhs.strip_prefix("Query") {
                (BindingKind::Query, r)
            } else {
                (BindingKind::Mutation, rhs.strip_prefix("Mutation")?)
            };
            let rest = rest.trim_start().strip_prefix('(')?;
            match lex(rest).into_iter().next() {
                Some(Tok::Str(name)) => Some(SurfaceBinding {
                    statement_id: id.to_owned(),
                    name,
                    kind,
                }),
                _ => None,
            }
        })
        .collect()
}

/// A held surface.
struct Held {
    revision: u64,
    format: SurfaceFormat,
    source: String,
    text: String,
    bindings: Vec<SurfaceBinding>,
}

impl Held {
    fn size(&self) -> usize {
        self.source.len() + self.text.len()
    }
}

/// One placement's status and content.
#[derive(Default)]
struct Slot {
    status: Option<Status>,
    content: Option<Held>,
}

/// Per-plugin placement store (statuses + contents + revisions), not persisted (§9.2).
#[derive(Default)]
pub(crate) struct Board {
    plugins: HashMap<String, BTreeMap<String, Slot>>,
    /// Last revision per `(plugin, placement)`; survives `drop_plugin` so a restarted plugin never
    /// reuses a revision a client already saw.
    revisions: HashMap<(String, String), u64>,
}

impl Board {
    fn slot(&mut self, plugin: &str, placement: &str) -> &mut Slot {
        self.plugins
            .entry(plugin.to_owned())
            .or_default()
            .entry(placement.to_owned())
            .or_default()
    }

    fn get(&self, plugin: &str, placement: &str) -> Option<&Slot> {
        self.plugins.get(plugin)?.get(placement)
    }

    /// Store a status; true when it differs from the held one.
    pub fn set_status(&mut self, plugin: &str, placement: &str, s: Status) -> bool {
        let slot = self.slot(plugin, placement);
        if slot.status.as_ref() == Some(&s) {
            return false;
        }
        slot.status = Some(s);
        true
    }

    /// Hold validated content. `Ok(Some(rev))` new revision, `Ok(None)` byte-identical (format,
    /// source and text) to the held content (no new revision), `Err` over [`SURFACE_MAX`] or
    /// [`PLUGIN_MAX`] (the held content stays).
    pub fn set_content(
        &mut self,
        plugin: &str,
        placement: &str,
        ui: UiParams,
        bindings: Vec<SurfaceBinding>,
    ) -> Result<Option<u64>, String> {
        let ui = UiParams {
            source: tools::secrets::redact(&ui.source).into_owned(),
            text: tools::secrets::redact(&ui.text).into_owned(),
            ..ui
        };
        let held = self.get(plugin, placement).and_then(|s| s.content.as_ref());
        if held.is_some_and(|h| h.format == ui.format && h.source == ui.source && h.text == ui.text)
        {
            return Ok(None);
        }
        let size = ui.source.len() + ui.text.len();
        if size > SURFACE_MAX {
            return Err(format!(
                "surface {placement} is {size} bytes, over the {SURFACE_MAX} byte limit"
            ));
        }
        let others: usize = self
            .plugins
            .get(plugin)
            .map(|slots| {
                slots
                    .iter()
                    .filter(|(id, _)| id.as_str() != placement)
                    .filter_map(|(_, s)| s.content.as_ref().map(Held::size))
                    .sum()
            })
            .unwrap_or(0);
        if others + size > PLUGIN_MAX {
            return Err(format!(
                "surfaces of {plugin} would hold {} bytes, over the {PLUGIN_MAX} byte limit",
                others + size
            ));
        }
        let rev = self
            .revisions
            .entry((plugin.to_owned(), placement.to_owned()))
            .or_insert(0);
        *rev += 1;
        let revision = *rev;
        self.slot(plugin, placement).content = Some(Held {
            revision,
            format: ui.format,
            source: ui.source,
            text: ui.text,
            bindings,
        });
        Ok(Some(revision))
    }

    /// The held content, `state: Ready`.
    pub fn content(&self, plugin: &str, placement: &str) -> Option<SurfaceContent> {
        let h = self.get(plugin, placement)?.content.as_ref()?;
        Some(SurfaceContent {
            plugin: plugin.to_owned(),
            placement: placement.to_owned(),
            revision: h.revision,
            state: SurfaceLoad::Ready,
            format: Some(h.format),
            source: Some(h.source.clone()),
            text: Some(h.text.clone()),
            bindings: h.bindings.clone(),
            error: None,
        })
    }

    /// The held status.
    pub fn status(&self, plugin: &str, placement: &str) -> Option<&Status> {
        self.get(plugin, placement)?.status.as_ref()
    }

    /// Forget every status and content of `plugin` (it stopped, §9.2).
    pub fn drop_plugin(&mut self, plugin: &str) {
        self.plugins.remove(plugin);
    }
}

/// The snapshot clients draw (§9.3, contract §5.1): while `backoff` / `failed` / `exited` /
/// `awaiting_approval` (and `invalid` / `stopping` / `disabled`) the dot is overridden and the
/// tooltip is the state; no status yet → `off` "starting"; otherwise the plugin's status.
/// `revision` 0 = no content held (then `format` is `None`).
pub(crate) fn snapshot(
    plugin: &str,
    p: &PlacementDecl,
    state: PluginRunState,
    s: Option<&Status>,
    revision: u64,
) -> SurfaceSnapshot {
    let over = |dot: Dot, word: &str| (dot, Some(word.to_owned()));
    let (dot, tooltip) = match state {
        PluginRunState::Running | PluginRunState::Starting => match s {
            Some(s) => (s.dot, s.tooltip.clone()),
            None => over(Dot::Off, "starting"),
        },
        PluginRunState::Backoff => over(Dot::Off, "backoff"),
        PluginRunState::Failed => over(Dot::Error, "failed"),
        PluginRunState::Exited => over(Dot::Off, "exited"),
        PluginRunState::AwaitingApproval => over(Dot::Off, "awaiting approval"),
        PluginRunState::Invalid => over(Dot::Error, "invalid"),
        PluginRunState::Stopping => over(Dot::Off, "stopping"),
        PluginRunState::Disabled => over(Dot::Off, "disabled"),
    };
    SurfaceSnapshot {
        plugin: plugin.to_owned(),
        placement: p.id.clone(),
        dot,
        badge: s.and_then(|s| s.badge.clone()),
        label: s.and_then(|s| s.label.clone()),
        tooltip,
        revision,
        format: if revision > 0 { p.format } else { None },
    }
}

/// Token bucket: `per` tokens per `window`, at most `burst` held. `check` takes one token or
/// answers `Err(retry_after_ms)`.
pub(crate) struct RateLimit {
    /// Tokens per millisecond.
    rate: f64,
    burst: f64,
    tokens: f64,
    window: Duration,
    last: Option<Instant>,
}

impl RateLimit {
    /// A full bucket (`burst`, at least 1).
    pub fn new(per: u32, window: Duration, burst: u32) -> Self {
        let ms = window.as_secs_f64() * 1000.0;
        let rate = if ms > 0.0 { f64::from(per) / ms } else { 0.0 };
        let burst = f64::from(burst.max(1));
        Self {
            rate,
            burst,
            tokens: burst,
            window,
            last: None,
        }
    }

    /// Take one token at `now`, or `Err(ms until one is available)`.
    pub fn check(&mut self, now: Instant) -> Result<(), u64> {
        if let Some(last) = self.last {
            let elapsed = now.saturating_duration_since(last).as_secs_f64() * 1000.0;
            self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        }
        self.last = Some(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }
        if self.rate <= 0.0 {
            return Err(u64::try_from(self.window.as_millis())
                .unwrap_or(u64::MAX)
                .max(1));
        }
        let wait = ((1.0 - self.tokens) / self.rate).ceil();
        // Bounded by window / per, so the cast cannot overflow in practice.
        Err((wait as u64).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Fallback, PlacementKind};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn ui(placement: &str, format: SurfaceFormat, source: &str) -> UiParams {
        UiParams {
            placement: placement.into(),
            format,
            source: source.into(),
            text: "fallback".into(),
        }
    }

    fn placement(format: Option<SurfaceFormat>) -> PlacementDecl {
        PlacementDecl {
            id: "turns".into(),
            kind: PlacementKind::Sidebar,
            open: None,
            icon: "gauge".into(),
            label: "Turns".into(),
            status: true,
            size: None,
            format,
            fallback: Fallback::Text,
            command: None,
        }
    }

    const PANEL: &str = "root = Card([title, controls])\ntitle = TextContent(\"## Turns today\")\nreset = Mutation(\"reset\", {})\ncontrols = Buttons([Button(\"Reset counter\", @Run(reset), \"secondary\")])";

    #[tokio::test]
    async fn masked_input_refused() {
        let mut board = Board::default();
        let good = ui("turns", SurfaceFormat::Openui, PANEL);
        let b = validate(None, "turns", good.format, &good.source)
            .await
            .unwrap();
        assert_eq!(
            board.set_content("turns", "turns", good, b).unwrap(),
            Some(1)
        );

        let bad = "root = Form([pw])\npw = Input(\"pw\", \"password\")";
        let e = validate(None, "turns", SurfaceFormat::Openui, bad)
            .await
            .unwrap_err();
        assert!(!e.path.is_empty());
        assert_eq!(e.message, MASKED);
        // A refused `ui` is never held: the old content stays.
        let held = board.content("turns", "turns").unwrap();
        assert_eq!(held.revision, 1);
        assert_eq!(held.source.as_deref(), Some(PANEL));

        assert!(masked_input("x = Input(\"pw\", {type: 'Password'})").is_some());
        assert!(masked_input("x = Input(\"name\", \"Your name\")").is_none());
        // Inside a string or on another component, it is not an Input call.
        assert!(masked_input("t = TextContent(\"Input(\\\"pw\\\", \\\"password\\\")\")").is_none());
        assert!(masked_input("t = TextArea(\"pw\", \"password\")").is_none());
        // R10: a string reached through statements is resolved, transitively and cycle-safe.
        assert!(masked_input("t = \"password\"\npw = Input(\"pw\", t)").is_some());
        assert!(masked_input("a = b\nb = {type: \"password\"}\npw = Input(\"pw\", a)").is_some());
        assert!(masked_input("a = b\nb = a\npw = Input(\"pw\", a)").is_none());
        assert!(masked_input("t = \"Your name\"\npw = Input(\"pw\", t)").is_none());
    }

    /// R7: a vault value in `ui.source` is held and compared redacted.
    #[tokio::test]
    async fn source_is_redacted_before_validation_and_hold() {
        ::tools::secrets::install(std::sync::Arc::new(crate::route::tests::Fake));
        let p = crate::route::tests::placeholder();
        let raw = "# token hunter22";
        validate(None, "turns", SurfaceFormat::Markdown, raw)
            .await
            .unwrap();
        let mut board = Board::default();
        let u = ui("turns", SurfaceFormat::Markdown, raw);
        assert_eq!(
            board.set_content("turns", "turns", u, vec![]).unwrap(),
            Some(1)
        );
        let held = board.content("turns", "turns").unwrap();
        assert_eq!(held.source, Some(format!("# token {p}")));
        // The plugin sends the same secret again: byte-identical after redaction, no new revision.
        let again = ui("turns", SurfaceFormat::Markdown, raw);
        assert_eq!(
            board.set_content("turns", "turns", again, vec![]).unwrap(),
            None
        );
    }

    #[test]
    fn identical_source_keeps_revision() {
        let mut board = Board::default();
        assert_eq!(
            board
                .set_content("p", "a", ui("a", SurfaceFormat::Markdown, "# hi"), vec![])
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            board
                .set_content("p", "a", ui("a", SurfaceFormat::Markdown, "# hi"), vec![])
                .unwrap(),
            None
        );
        assert_eq!(board.content("p", "a").unwrap().revision, 1);
        assert_eq!(
            board
                .set_content("p", "a", ui("a", SurfaceFormat::Markdown, "# ho"), vec![])
                .unwrap(),
            Some(2)
        );
        // Revisions are per placement.
        assert_eq!(
            board
                .set_content("p", "b", ui("b", SurfaceFormat::Markdown, "# hi"), vec![])
                .unwrap(),
            Some(1)
        );
        let c = board.content("p", "a").unwrap();
        assert_eq!(c.state, SurfaceLoad::Ready);
        assert_eq!(c.error, None);
        assert_eq!(c.source.as_deref(), Some("# ho"));
        // A restarted plugin never reuses a revision.
        board.drop_plugin("p");
        assert!(board.content("p", "a").is_none());
        assert_eq!(
            board
                .set_content("p", "a", ui("a", SurfaceFormat::Markdown, "# hi"), vec![])
                .unwrap(),
            Some(3)
        );
    }

    #[tokio::test]
    async fn bindings_come_from_check() {
        let helper = Helper::new(tokio_util::sync::CancellationToken::new());
        let b = validate(Some(&helper), "turns", SurfaceFormat::Openui, PANEL)
            .await
            .unwrap();
        let want = vec![SurfaceBinding {
            statement_id: "reset".into(),
            name: "reset".into(),
            kind: BindingKind::Mutation,
        }];
        assert_eq!(b, want);
        // Without a validator the statements give the same table.
        assert_eq!(
            validate(None, "turns", SurfaceFormat::Openui, PANEL)
                .await
                .unwrap(),
            want
        );
    }

    #[tokio::test]
    async fn check_snapshots_and_diagnostics() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let log = seen.clone();
        let helper = Helper::canned(Arc::new(move |line: &str| {
            let req: Value = serde_json::from_str(line).unwrap();
            log.lock().unwrap().push(req["tools"].clone());
            json!({
                "id": req["id"],
                "source": req["source"],
                "library_hash": "h",
                "bindings": [],
                "diagnostics": [{"at": "q", "message": "no such thing", "code": "x"}],
            })
            .to_string()
        }));
        let src = "root = Stack([])\nq = Query(\"count\", {})\nm = Mutation(\"reset\", {})";
        let e = validate(Some(&helper), "turns", SurfaceFormat::Openui, src)
            .await
            .unwrap_err();
        assert_eq!(
            e,
            CheckIssue {
                path: "q".into(),
                message: "no such thing".into()
            }
        );
        let tools = seen.lock().unwrap()[0].clone();
        assert_eq!(
            tools,
            json!([
                {"name": "reset", "schema": {"type": "object"}, "risk": "mutating", "identity": "plugin:turns:reset"},
                {"name": "count", "schema": {"type": "object"}, "risk": "read_only", "identity": "plugin:turns:count"},
            ])
        );
        // json-render: the first spec error; markdown as is.
        assert!(
            validate(None, "turns", SurfaceFormat::JsonRender, "[]")
                .await
                .is_err()
        );
        assert_eq!(
            validate(
                None,
                "turns",
                SurfaceFormat::Markdown,
                "<input type=password>"
            )
            .await
            .unwrap(),
            vec![]
        );
    }

    #[test]
    fn oversize_surface_refused() {
        let mut board = Board::default();
        let big = "x".repeat(SURFACE_MAX);
        assert!(
            board
                .set_content("p", "a", ui("a", SurfaceFormat::Markdown, &big), vec![])
                .is_err()
        );
        assert!(board.content("p", "a").is_none());
        // 4 MB per plugin: 16 surfaces just under 256 KB fit, the 17th does not.
        let fits = "x".repeat(SURFACE_MAX - 8);
        for i in 0..16 {
            let id = format!("s{i}");
            assert!(
                board
                    .set_content("p", &id, ui(&id, SurfaceFormat::Markdown, &fits), vec![])
                    .is_ok()
            );
        }
        let e = board
            .set_content(
                "p",
                "s16",
                ui("s16", SurfaceFormat::Markdown, &fits),
                vec![],
            )
            .unwrap_err();
        assert!(e.contains("byte limit"), "{e}");
        // Replacing a held surface counts its new size only.
        assert_eq!(
            board
                .set_content(
                    "p",
                    "s0",
                    ui("s0", SurfaceFormat::Markdown, "small"),
                    vec![]
                )
                .unwrap(),
            Some(2)
        );
        // Another plugin has its own budget.
        assert!(
            board
                .set_content("q", "a", ui("a", SurfaceFormat::Markdown, &fits), vec![])
                .is_ok()
        );
    }

    #[test]
    fn status_validation() {
        let (p, s) = parse_status(&json!({"placement": "turns", "dot": "warn", "badge": 51, "label": "51/50", "tooltip": "Turns today: 51"})).unwrap();
        assert_eq!(p, "turns");
        assert_eq!(
            s,
            Status {
                dot: Dot::Warn,
                badge: Some(Badge::Count(51)),
                label: Some("51/50".into()),
                tooltip: Some("Turns today: 51".into())
            }
        );
        let (_, s) =
            parse_status(&json!({"placement": "t", "dot": "none", "badge": "new!"})).unwrap();
        assert_eq!(s.badge, Some(Badge::Text("new!".into())));
        for (bad, path) in [
            (json!({"placement": "t", "dot": "green"}), "/dot"),
            (json!({"placement": "t"}), "/dot"),
            (json!({"dot": "ok"}), "/placement"),
            (
                json!({"placement": "t", "dot": "ok", "badge": "12345"}),
                "/badge",
            ),
            (
                json!({"placement": "t", "dot": "ok", "badge": -1}),
                "/badge",
            ),
            (
                json!({"placement": "t", "dot": "ok", "label": "x".repeat(17)}),
                "/label",
            ),
            (
                json!({"placement": "t", "dot": "ok", "tooltip": "x".repeat(201)}),
                "/tooltip",
            ),
        ] {
            match parse_status(&bad) {
                Err(PluginsError::Invalid { message }) => {
                    assert!(message.starts_with(path), "{message}")
                }
                other => panic!("{bad}: {other:?}"),
            }
        }
        let mut board = Board::default();
        assert!(board.set_status("p", "t", s.clone()));
        assert!(!board.set_status("p", "t", s.clone()));
        assert_eq!(board.status("p", "t"), Some(&s));

        let u = parse_ui(
            &json!({"placement": "t", "format": "json-render", "source": "{}", "text": "x"}),
        )
        .unwrap();
        assert_eq!(u.format, SurfaceFormat::JsonRender);
        assert!(
            parse_ui(&json!({"placement": "t", "format": "html", "source": "", "text": ""}))
                .is_err()
        );
        assert!(parse_ui(&json!({"placement": "t", "format": "markdown", "source": ""})).is_err());
    }

    #[test]
    fn snapshot_override() {
        let p = placement(Some(SurfaceFormat::Openui));
        let s = Status {
            dot: Dot::Ok,
            badge: Some(Badge::Count(3)),
            label: Some("3".into()),
            tooltip: Some("fine".into()),
        };
        let snap = snapshot("turns", &p, PluginRunState::Running, None, 0);
        assert_eq!(
            (snap.dot, snap.tooltip.as_deref(), snap.format),
            (Dot::Off, Some("starting"), None)
        );
        let snap = snapshot("turns", &p, PluginRunState::Running, Some(&s), 2);
        assert_eq!(
            snap,
            SurfaceSnapshot {
                plugin: "turns".into(),
                placement: "turns".into(),
                dot: Dot::Ok,
                badge: Some(Badge::Count(3)),
                label: Some("3".into()),
                tooltip: Some("fine".into()),
                revision: 2,
                format: Some(SurfaceFormat::Openui),
            }
        );
        for (state, dot, word) in [
            (PluginRunState::Backoff, Dot::Off, "backoff"),
            (PluginRunState::Failed, Dot::Error, "failed"),
            (PluginRunState::Exited, Dot::Off, "exited"),
            (
                PluginRunState::AwaitingApproval,
                Dot::Off,
                "awaiting approval",
            ),
        ] {
            let snap = snapshot("turns", &p, state, Some(&s), 2);
            assert_eq!(
                (snap.dot, snap.tooltip.as_deref()),
                (dot, Some(word)),
                "{state:?}"
            );
        }
    }

    #[test]
    fn rate_limit_bucket() {
        let t0 = Instant::now();
        let mut rl = RateLimit::new(4, Duration::from_secs(1), 2);
        assert!(rl.check(t0).is_ok());
        assert!(rl.check(t0).is_ok());
        assert!(matches!(rl.check(t0), Err(249..=251)));
        assert!(matches!(
            rl.check(t0 + Duration::from_millis(100)),
            Err(149..=151)
        ));
        assert!(rl.check(t0 + Duration::from_millis(260)).is_ok());
        // Refill never exceeds the burst.
        let later = t0 + Duration::from_secs(60);
        assert!(rl.check(later).is_ok());
        assert!(rl.check(later).is_ok());
        assert!(rl.check(later).is_err());
    }
}
