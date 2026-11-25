//! `plugin.yaml` (design 216 §4): parse and §4.3 validation with JSON Pointer paths, the canonical
//! JSON manifest hash, the code hash, the grants a manifest asks for and the install-card lines (§10.1).
//! Public only for [`check_name`], [`RESERVED_NAMES`] and [`check_offline`] (contract §3.4).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use globset::GlobBuilder;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tools::Risk;

use crate::error::PluginsError;
use crate::types::{
    CallGrant, Capabilities, CheckIssue, CheckReport, CommandInfo, Fallback, GrantKey, GrantLine,
    Grants, OpenMode, PlacementInfo, PlacementKind, Scoped, Size, SurfaceFormat, ToolGrant,
};

/// The manifest's file name inside a plugin folder.
pub(crate) const FILE: &str = "plugin.yaml";

/// Built-in slash command names a plugin may not take (without the `/`): the TUI's `COMMANDS`
/// and its component slash specs (`crates/tui` `components::builtin().slash_specs()`), the
/// desktop's `slash.ts`, and `plugins`. B2's TUI test `tui_commands_are_reserved_plugin_names`
/// fails when a TUI command is missing here.
pub const RESERVED_NAMES: &[&str] = &[
    "a2a",
    "acp",
    "advisor",
    "agents",
    "always-ask",
    "approvals",
    "artifacts",
    "ask",
    "attach",
    "autoresearch",
    "aws",
    "broadcast",
    "builtins",
    "cancel",
    "clear",
    "communication",
    "compact",
    "computer",
    "cost",
    "dictate",
    "double",
    "edit",
    "effort",
    "endless",
    "escalate",
    "exit",
    "files",
    "fork",
    "fractal",
    "jobs",
    "knowledge",
    "link",
    "login",
    "logout",
    "mcp",
    "mesh",
    "model",
    "new",
    "nodes",
    "onboarding",
    "parent",
    "peers",
    "plugins",
    "project",
    "prompt",
    "provider",
    "quit",
    "redo",
    "regen",
    "resume",
    "roster",
    "sandbox",
    "schedules",
    "search",
    "secrets",
    "sessions",
    "shake",
    "share",
    "skill",
    "skills",
    "subagent",
    "take",
    "talk",
    "tasks",
    "terminals",
    "title",
    "todos",
    "tools",
    "undo",
    "vision",
    "voice",
];

/// Keys of `grants.yaml` beside the plugin entries: a plugin may not be named after them.
const GRANTS_KEYS: &[&str] = &["order", "hidden"];

/// `^[a-z][a-z0-9-]{0,31}$`, not a reserved name (a built-in slash command of the desktop or
/// the TUI, or `plugins`). Used by the loader and by `scaffold::new_plugin`.
pub fn check_name(name: &str) -> Result<(), PluginsError> {
    if !name_syntax(name) {
        return Err(PluginsError::Invalid {
            message: format!("plugin name `{name}` must match ^[a-z][a-z0-9-]{{0,31}}$"),
        });
    }
    if RESERVED_NAMES.binary_search(&name).is_ok() {
        return Err(PluginsError::Invalid {
            message: format!("plugin name `{name}` collides with /{name}"),
        });
    }
    if GRANTS_KEYS.contains(&name) {
        return Err(PluginsError::Invalid {
            message: format!("plugin name `{name}` is reserved (a key of grants.yaml)"),
        });
    }
    Ok(())
}

/// Validate a folder without an engine: parse + every §4.3 rule that needs no vault and no
/// catalog; those two are reported as warnings ("engine not running: secrets and topics not
/// checked"). Also computes both hashes and the card lines. The folder may be a source to install
/// from (`~/Downloads/turns-main`), so its name need not equal the plugin's (review R20).
pub fn check_offline(dir: &Path) -> CheckReport {
    let offline = issue("", "engine not running: secrets and topics not checked");
    match load_source(dir) {
        Ok(parsed) => {
            let m = &parsed.manifest;
            let req = requested(m, &hint_risk);
            let per_min = crate::PluginsConfig::default().input_per_min;
            let lines = card_lines(m, &req, None, dir, per_min, &|_| None);
            let mut warnings = parsed.warnings.clone();
            warnings.push(offline);
            CheckReport {
                ok: true,
                name: Some(m.name.clone()),
                errors: Vec::new(),
                warnings,
                manifest_sha256: Some(parsed.manifest_sha256.clone()),
                code_sha256: Some(code_hash(&code_files(dir))),
                lines,
            }
        }
        Err((errors, mut warnings)) => {
            warnings.push(offline);
            let parsed = std::fs::read_to_string(dir.join(FILE))
                .ok()
                .and_then(|t| parse(&t).ok());
            let name = match &parsed {
                Some(p) => Some(p.manifest.name.clone()),
                None => dir.file_name().and_then(|f| f.to_str()).map(str::to_owned),
            };
            CheckReport {
                ok: false,
                name,
                errors,
                warnings,
                manifest_sha256: parsed.map(|p| p.manifest_sha256),
                code_sha256: None,
                lines: Vec::new(),
            }
        }
    }
}

/// A parsed, validated `plugin.yaml`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Manifest {
    /// Plugin name (= folder name).
    pub name: String,
    /// Free text shown on the card.
    pub version: String,
    /// One line for Settings › Plugins.
    pub description: String,
    /// Wire protocol major (always 1).
    pub protocol: u32,
    /// argv; a string `run:` is split with shlex.
    pub run: Vec<String>,
    /// Working directory relative to the folder (default `.`).
    pub cwd: String,
    /// Environment, unexpanded.
    pub env: BTreeMap<String, String>,
    /// Restart policy (default on-failure).
    pub restart: RestartPolicy,
    /// Globs relative to the folder.
    pub watch: Vec<String>,
    /// What it reads.
    pub subscribe: Vec<Subscription>,
    /// What it may emit.
    pub emits: BTreeMap<String, EmitDecl>,
    /// Placements; reserved kinds and formats already skipped.
    pub placements: Vec<PlacementDecl>,
    /// Tools it offers the model.
    pub tools: Vec<ToolDecl>,
    /// Host-brokered powers.
    pub capabilities: Capabilities,
    /// The parsed manifest as JSON (env unexpanded) → `PluginDetail.manifest`.
    pub json: Value,
}

/// `restart:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestartPolicy {
    /// Restart after a non-zero exit (default).
    OnFailure,
    /// Restart after any exit.
    Always,
    /// Never restart.
    Never,
}

/// One `subscribe:` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Subscription {
    /// Wire topic (`agent.events`); exactly one of topic/event is Some.
    pub topic: Option<String>,
    /// Plugin event `turns.limit_reached` or `turns.*`.
    pub event: Option<String>,
    /// Empty = every type of the topic.
    pub types: Vec<String>,
    /// Empty = the topic's current version.
    pub version: Vec<u32>,
    /// Delta-class types wanted.
    pub deltas: bool,
}

/// One `emits:` entry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EmitDecl {
    /// Catalog description.
    pub description: String,
    /// Event version (default 1).
    pub version: u32,
    /// Global (default) or session-scoped.
    pub scope: Scoped,
    /// JSON Schema for subscribers (`{}` when absent).
    pub schema: Value,
}

/// One `placements:` entry (design §9.2).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlacementDecl {
    /// Unique within the plugin.
    pub id: String,
    /// Where it is drawn.
    pub kind: PlacementKind,
    /// Sidebar only (default popover).
    pub open: Option<OpenMode>,
    /// Lucide name or SVG path (empty for commands without one).
    pub icon: String,
    /// Tooltip / menu text / tile title.
    pub label: String,
    /// Draws a status dot.
    pub status: bool,
    /// Default size.
    pub size: Option<Size>,
    /// Absent for status and command.
    pub format: Option<SurfaceFormat>,
    /// What non-rendering surfaces show.
    pub fallback: Fallback,
    /// `kind == Command` only.
    pub command: Option<CommandDecl>,
}

/// The command keys of a `command` placement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandDecl {
    /// `name:` (default: the plugin name).
    pub name: String,
    /// One line for the slash menu.
    pub description: String,
    /// Argument hint.
    pub args: Option<String>,
}

/// One `tools:` entry (MCP's Tool shape).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToolDecl {
    /// Tool name.
    pub name: String,
    /// What the model reads.
    pub description: String,
    /// Object JSON Schema.
    pub input_schema: Value,
    /// Optional output schema.
    pub output_schema: Option<Value>,
    /// `annotations.readOnlyHint`.
    pub read_only_hint: Option<bool>,
    /// `annotations.destructiveHint`.
    pub destructive_hint: Option<bool>,
}

/// A manifest that passed [`parse`].
#[derive(Clone, Debug)]
pub(crate) struct Parsed {
    /// The manifest.
    pub manifest: Manifest,
    /// Reserved and unknown keys.
    pub warnings: Vec<CheckIssue>,
    /// sha256 hex of the canonical JSON.
    pub manifest_sha256: String,
}

/// `(errors, warnings)` of a manifest that did not validate.
type Issues = (Vec<CheckIssue>, Vec<CheckIssue>);

/// Parse + every rule that needs no filesystem, vault or catalog. Err = (errors, warnings).
pub(crate) fn parse(text: &str) -> Result<Parsed, Issues> {
    let json: Value = serde_yaml::from_str(text).map_err(|e| {
        (
            vec![issue("", format!("plugin.yaml does not parse: {e}"))],
            Vec::new(),
        )
    })?;
    let Some(top) = json.as_object() else {
        return Err((
            vec![issue("", "plugin.yaml must be a mapping of keys")],
            Vec::new(),
        ));
    };
    let mut v = V::default();
    for key in top.keys() {
        let path = ptr("", key);
        match key.as_str() {
            k if TOP_KEYS.contains(&k) => {}
            "mcp" => v.warn(path, "`mcp` is reserved for a later version; ignored"),
            "isolation" => v.err(
                path,
                "`isolation` is not supported by this version: the plugin would run without the containment it asks for",
            ),
            _ => v.warn(path, format!("unknown key `{key}`; ignored")),
        }
    }
    control_chars(&mut v, "", &json);

    let name = match present(top, "name") {
        None => {
            v.err("/name", "required");
            String::new()
        }
        Some(Value::String(s)) => {
            if let Err(e) = check_name(s) {
                v.err("/name", e.to_string());
            }
            s.clone()
        }
        Some(_) => {
            v.err("/name", "expected a string");
            String::new()
        }
    };
    let version = match present(top, "version") {
        None => {
            v.err("/version", "required");
            String::new()
        }
        Some(val) => v.scalar("/version", val).unwrap_or_default(),
    };
    let description = present(top, "description")
        .and_then(|d| v.scalar("/description", d))
        .unwrap_or_default();
    match present(top, "protocol") {
        None => v.err("/protocol", "required"),
        Some(p) if p.as_u64() == Some(u64::from(crate::PROTOCOL)) => {}
        Some(_) => v.err(
            "/protocol",
            format!(
                "protocol must be {} (the protocol this host speaks)",
                crate::PROTOCOL
            ),
        ),
    }
    let run = match present(top, "run") {
        None => {
            v.err("/run", "required");
            Vec::new()
        }
        Some(val) => run_argv(&mut v, val),
    };
    let cwd = match present(top, "cwd") {
        None => ".".to_owned(),
        Some(val) => {
            let cwd = v.string("/cwd", val).unwrap_or_else(|| ".".to_owned());
            let p = Path::new(&cwd);
            if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
                v.err("/cwd", "cwd must be a relative path inside the folder");
            }
            cwd
        }
    };
    let env = present(top, "env")
        .map(|e| env_map(&mut v, e))
        .unwrap_or_default();
    let restart = match present(top, "restart") {
        None => RestartPolicy::OnFailure,
        Some(val) => v
            .one_of(
                "/restart",
                val,
                &[
                    ("on-failure", RestartPolicy::OnFailure),
                    ("always", RestartPolicy::Always),
                    ("never", RestartPolicy::Never),
                ],
            )
            .unwrap_or(RestartPolicy::OnFailure),
    };
    let watch = present(top, "watch")
        .map(|w| watch_globs(&mut v, w))
        .unwrap_or_default();
    let subscribe = present(top, "subscribe")
        .map(|s| subscriptions(&mut v, s))
        .unwrap_or_default();
    let emits = present(top, "emits")
        .map(|e| emit_decls(&mut v, e))
        .unwrap_or_default();
    let placements = present(top, "placements")
        .map(|p| placement_decls(&mut v, p, &name))
        .unwrap_or_default();
    let tools = present(top, "tools")
        .map(|t| tool_decls(&mut v, t))
        .unwrap_or_default();
    let capabilities = present(top, "capabilities")
        .map(|c| capability_list(&mut v, c))
        .unwrap_or_default();

    if !v.errors.is_empty() {
        return Err((v.errors, v.warnings));
    }
    let manifest_sha256 = manifest_hash(&json);
    Ok(Parsed {
        manifest: Manifest {
            name,
            version,
            description,
            protocol: crate::PROTOCOL,
            run,
            cwd,
            env,
            restart,
            watch,
            subscribe,
            emits,
            placements,
            tools,
            capabilities,
            json,
        },
        warnings: v.warnings,
        manifest_sha256,
    })
}

/// [`load_source`], then the installed-folder rule: `name` == folder name.
pub(crate) fn load(dir: &Path) -> Result<Parsed, Issues> {
    let parsed = load_source(dir)?;
    match dir.file_name().and_then(|f| f.to_str()) {
        Some(folder) if folder != parsed.manifest.name => Err((
            vec![issue(
                "/name",
                format!(
                    "name `{}` must equal the folder name `{folder}`",
                    parsed.manifest.name
                ),
            )],
            parsed.warnings,
        )),
        _ => Ok(parsed),
    }
}

/// Read `<dir>/plugin.yaml`, parse, then the rules that hold for any folder, installed or a
/// source to install from: `run[0]` resolves (absolute, relative to the folder/cwd, or on PATH).
pub(crate) fn load_source(dir: &Path) -> Result<Parsed, Issues> {
    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        (
            vec![issue("", format!("cannot read {}: {e}", path.display()))],
            Vec::new(),
        )
    })?;
    let parsed = parse(&text)?;
    let m = &parsed.manifest;
    if let Some(arg0) = m.run.first()
        && !resolves(dir, &m.cwd, arg0)
    {
        return Err((
            vec![issue(
                "/run/0",
                format!("`{arg0}` is not a file in the folder and not on PATH"),
            )],
            parsed.warnings,
        ));
    }
    Ok(parsed)
}

/// Directories never hashed: VCS data, dependency caches and build output (rebuilt, not authored).
const UNHASHED_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__"];

/// rel path → sha256 hex of every regular file in the folder, except [`UNHASHED_DIRS`], the root
/// `plugin.yaml` (it has its own hash and cards) and `.DS_Store` (review R4: a file dropped
/// beside the entry point, such as `json.py` or `sitecustomize.py`, changes what runs). `watch`
/// only triggers restarts; it does not scope this.
pub(crate) fn code_files(dir: &Path) -> BTreeMap<String, String> {
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut out = BTreeMap::new();
    walk(&root, &root, &mut out);
    out
}

/// sha256 over the sorted (path, sha) list.
pub(crate) fn code_hash(files: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    for (path, sha) in files {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(sha.as_bytes());
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// readOnlyHint → ReadOnly, else destructiveHint → Destructive, else Mutating.
pub(crate) fn hint_risk(t: &ToolDecl) -> Risk {
    if t.read_only_hint == Some(true) {
        Risk::ReadOnly
    } else if t.destructive_hint == Some(true) {
        Risk::Destructive
    } else {
        Risk::Mutating
    }
}

/// What the manifest asks for. `tool_risk` decides each tool's risk (host: gate then hints).
pub(crate) fn requested(m: &Manifest, tool_risk: &dyn Fn(&ToolDecl) -> Risk) -> Grants {
    let mut subscribe: Vec<String> = Vec::new();
    let mut add = |n: String| {
        if !subscribe.contains(&n) {
            subscribe.push(n);
        }
    };
    for s in &m.subscribe {
        match (&s.topic, &s.event) {
            (Some(t), _) if !s.types.is_empty() => {
                s.types.iter().for_each(|ty| add(format!("{t}#{ty}")))
            }
            (Some(t), _) => add(t.clone()),
            (None, Some(e)) => add(e.clone()),
            (None, None) => {}
        }
    }
    Grants {
        subscribe,
        emits: m.emits.keys().cloned().collect(),
        placements: m.placements.iter().map(|p| p.id.clone()).collect(),
        tools: m
            .tools
            .iter()
            .map(|t| ToolGrant {
                name: t.name.clone(),
                risk: tool_risk(t),
            })
            .collect(),
        capabilities: m.capabilities.clone(),
        secrets: secret_names(&m.env),
        run: m.run.clone(),
        cwd: m.cwd.clone(),
        env: m.env.clone(),
    }
}

/// A secret, `input`, or `call` with max_risk above ReadOnly.
pub(crate) fn needs_code_hash(g: &Grants) -> bool {
    !g.secrets.is_empty()
        || g.capabilities.input
        || g.capabilities
            .call
            .as_ref()
            .is_some_and(|c| c.max_risk > Risk::ReadOnly)
}

/// Install-card lines (contract §5.1 GrantLine, design §10.1 wording). `prior` Some = widen card:
/// only lines new vs `prior` (new: true) plus the Run (and env) line when run/cwd/env changed.
/// `describe(name)` gives the catalog sentence for `topic#type` / `topic` / `plugin.event`;
/// `input_per_min` is `plugins.input_per_min`, the rate the Input line promises.
pub(crate) fn card_lines(
    m: &Manifest,
    req: &Grants,
    prior: Option<&Grants>,
    dir: &Path,
    input_per_min: u32,
    describe: &dyn Fn(&str) -> Option<String>,
) -> Vec<GrantLine> {
    let new = prior.is_some();
    let line = |key, text: String, risk| GrantLine {
        key,
        text,
        risk,
        new,
    };
    let mut out = Vec::new();

    let run_changed =
        prior.is_none_or(|p| p.run != req.run || p.cwd != req.cwd || p.env != req.env);
    if run_changed {
        let argv = shlex::try_join(req.run.iter().map(String::as_str))
            .unwrap_or_else(|_| req.run.join(" "));
        let from = if req.cwd == "." {
            dir.to_path_buf()
        } else {
            dir.join(&req.cwd)
        };
        out.push(line(
            GrantKey::Run,
            format!(
                "Runs `{argv}` from `{}` as you, with your files and network",
                tilde(&from)
            ),
            Risk::Destructive,
        ));
        // R17: the environment is part of what runs (`PATH`, `LD_PRELOAD`, `NODE_OPTIONS`);
        // shown with unexpanded values, so a secret appears as its placeholder.
        if !req.env.is_empty() {
            let env = req
                .env
                .iter()
                .map(|(k, v)| format!("{k}={}", shlex::try_quote(v).unwrap_or_else(|_| v.into())))
                .collect::<Vec<_>>()
                .join(" ");
            out.push(line(
                GrantKey::Run,
                format!("With the environment `{env}`"),
                Risk::Destructive,
            ));
        }
    }
    for s in req
        .secrets
        .iter()
        .filter(|s| prior.is_none_or(|p| !p.secrets.contains(s)))
    {
        out.push(line(
            GrantKey::Secret,
            format!("Gets the secret `{s}` in its environment"),
            Risk::Destructive,
        ));
    }
    for s in req
        .subscribe
        .iter()
        .filter(|s| prior.is_none_or(|p| !p.subscribe.contains(s)))
    {
        out.push(line(
            GrantKey::Subscribe,
            subscribe_text(s, describe),
            Risk::ReadOnly,
        ));
    }
    for e in req
        .emits
        .iter()
        .filter(|e| prior.is_none_or(|p| !p.emits.contains(e)))
    {
        out.push(line(
            GrantKey::Emit,
            format!("Publishes `{}.{e}`", m.name),
            Risk::ReadOnly,
        ));
    }
    for p in m.placements.iter().filter(|p| {
        req.placements.contains(&p.id) && prior.is_none_or(|g| !g.placements.contains(&p.id))
    }) {
        out.push(line(
            GrantKey::Placement,
            placement_text(p, &m.name),
            Risk::ReadOnly,
        ));
    }
    let tools: Vec<&ToolGrant> = req
        .tools
        .iter()
        .filter(|t| {
            prior.is_none_or(|p| !p.tools.iter().any(|g| g.name == t.name && g.risk >= t.risk))
        })
        .collect();
    if let Some(risk) = tools.iter().map(|t| t.risk).max() {
        let list = tools
            .iter()
            .map(|t| format!("{} ({})", t.name, risk_word(t.risk)))
            .collect::<Vec<_>>()
            .join(", ");
        let text = if tools.len() == 1 {
            format!("Offers the model 1 tool: {list}. The model reads what it returns.")
        } else {
            format!(
                "Offers the model {} tools: {list}. The model reads what they return.",
                tools.len()
            )
        };
        out.push(line(GrantKey::Tool, text, risk));
    }
    if let Some(call) = &req.capabilities.call {
        let widened = prior.is_none_or(|p| match &p.capabilities.call {
            None => true,
            Some(old) => {
                old.max_risk < call.max_risk || call.tools.iter().any(|t| !old.tools.contains(t))
            }
        });
        if widened {
            out.push(line(
                GrantKey::Call,
                format!(
                    "May call {} ({}), in its own background session",
                    call.tools.join(", "),
                    risk_word(call.max_risk)
                ),
                call.max_risk,
            ));
        }
    }
    if req.capabilities.input && prior.is_none_or(|p| !p.capabilities.input) {
        out.push(line(
            GrantKey::Input,
            format!("May send messages into your chats as you — at most {input_per_min} a minute — which the model then acts on with every tool your policy allows"),
            Risk::Destructive,
        ));
    }
    if req.capabilities.notify && prior.is_none_or(|p| !p.capabilities.notify) {
        out.push(line(
            GrantKey::Notify,
            "May add inbox items and notifications".to_owned(),
            Risk::ReadOnly,
        ));
    }
    out
}

/// A declared placement as clients list it.
pub(crate) fn placement_info(p: &PlacementDecl, plugin: &str, hidden: bool) -> PlacementInfo {
    PlacementInfo {
        id: p.id.clone(),
        kind: p.kind,
        label: p.label.clone(),
        icon: p.icon.clone(),
        status: p.status,
        open: p.open,
        size: p.size,
        format: p.format,
        fallback: p.fallback,
        hidden,
        command: p.command.as_ref().map(|c| CommandInfo {
            name: c.name.clone(),
            slash: slash(plugin, &c.name),
            description: c.description.clone(),
            args: c.args.clone(),
        }),
    }
}

// ---- validation ----

const TOP_KEYS: &[&str] = &[
    "name",
    "version",
    "description",
    "protocol",
    "run",
    "cwd",
    "env",
    "restart",
    "watch",
    "subscribe",
    "emits",
    "placements",
    "tools",
    "capabilities",
];
const RESERVED_KINDS: &[&str] = &["header", "composer", "card", "tray", "settings", "view"];
/// `agent.events` types that carry text people send theuth from channels.
const CHANNEL_TEXT_TYPES: &[&str] = &["InputQueued", "TurnStarted", "MessageDelivered"];

/// Collected issues.
#[derive(Default)]
struct V {
    errors: Vec<CheckIssue>,
    warnings: Vec<CheckIssue>,
}

impl V {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(CheckIssue {
            path: path.into(),
            message: message.into(),
        });
    }

    fn warn(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(CheckIssue {
            path: path.into(),
            message: message.into(),
        });
    }

    fn string(&mut self, path: &str, val: &Value) -> Option<String> {
        match val {
            Value::String(s) => Some(s.clone()),
            _ => {
                self.err(path, "expected a string");
                None
            }
        }
    }

    /// A string, number or bool, as text.
    fn scalar(&mut self, path: &str, val: &Value) -> Option<String> {
        let s = match val {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        };
        if s.is_none() {
            self.err(path, "expected a string");
        }
        s
    }

    fn boolean(&mut self, path: &str, val: &Value) -> Option<bool> {
        let b = val.as_bool();
        if b.is_none() {
            self.err(path, "expected true or false");
        }
        b
    }

    fn uint(&mut self, path: &str, val: &Value) -> Option<u32> {
        let n = val.as_u64().and_then(|n| u32::try_from(n).ok());
        if n.is_none() {
            self.err(path, "expected a non-negative integer");
        }
        n
    }

    fn one_of<T: Copy>(&mut self, path: &str, val: &Value, opts: &[(&str, T)]) -> Option<T> {
        let found = val
            .as_str()
            .and_then(|s| opts.iter().find(|(n, _)| *n == s))
            .map(|(_, t)| *t);
        if found.is_none() {
            let names = opts.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" | ");
            self.err(path, format!("expected one of {names}"));
        }
        found
    }

    fn string_list(&mut self, path: &str, val: &Value) -> Vec<String> {
        let Some(items) = val.as_array() else {
            self.err(path, "expected a list of strings");
            return Vec::new();
        };
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| self.string(&idx(path, i), item))
            .collect()
    }

    fn map<'a>(&mut self, path: &str, val: &'a Value) -> Option<&'a Map<String, Value>> {
        let m = val.as_object();
        if m.is_none() {
            self.err(path, "expected a mapping");
        }
        m
    }

    /// Warn for every key of `m` outside `known`.
    fn unknown_keys(&mut self, base: &str, m: &Map<String, Value>, known: &[&str]) {
        for k in m.keys().filter(|k| !known.contains(&k.as_str())) {
            self.warn(ptr(base, k), format!("unknown key `{k}`; ignored"));
        }
    }
}

fn run_argv(v: &mut V, val: &Value) -> Vec<String> {
    let argv = match val {
        Value::String(s) => match shlex::split(s) {
            Some(a) => a,
            None => {
                v.err("/run", "run has unbalanced quotes");
                return Vec::new();
            }
        },
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| v.scalar(&idx("/run", i), item))
            .collect(),
        _ => {
            v.err("/run", "expected a list of arguments or a command string");
            return Vec::new();
        }
    };
    if argv.is_empty() {
        v.err("/run", "run must name a command");
    }
    argv
}

fn env_map(v: &mut V, val: &Value) -> BTreeMap<String, String> {
    let Some(m) = v.map("/env", val) else {
        return BTreeMap::new();
    };
    m.iter()
        .filter_map(|(k, x)| v.scalar(&ptr("/env", k), x).map(|s| (k.clone(), s)))
        .collect()
}

fn watch_globs(v: &mut V, val: &Value) -> Vec<String> {
    let globs = v.string_list("/watch", val);
    for (i, g) in globs.iter().enumerate() {
        if let Err(e) = GlobBuilder::new(g).literal_separator(true).build() {
            v.err(idx("/watch", i), format!("bad glob: {e}"));
        }
    }
    globs
}

fn subscriptions(v: &mut V, val: &Value) -> Vec<Subscription> {
    let Some(items) = val.as_array() else {
        v.err("/subscribe", "expected a list of subscriptions");
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let base = idx("/subscribe", i);
        let Some(s) = v.map(&base, item) else {
            continue;
        };
        for k in s.keys() {
            match k.as_str() {
                "topic" | "event" | "types" | "version" | "deltas" => {}
                "sessions" => v.warn(
                    ptr(&base, k),
                    "`sessions` is reserved for a later version; ignored",
                ),
                _ => v.warn(ptr(&base, k), format!("unknown key `{k}`; ignored")),
            }
        }
        let topic = present(s, "topic").and_then(|t| v.string(&ptr(&base, "topic"), t));
        let event = present(s, "event").and_then(|e| v.string(&ptr(&base, "event"), e));
        match (&topic, &event) {
            (Some(_), Some(_)) | (None, None) => {
                v.err(&base, "give exactly one of `topic` or `event`");
                continue;
            }
            (Some(t), None) if !topic_ok(t) => {
                v.err(ptr(&base, "topic"), format!("`{t}` is not a topic name"))
            }
            (None, Some(e)) if !event_ref_ok(e) => v.err(
                ptr(&base, "event"),
                format!("`{e}` is not a plugin event (`<plugin>.<event>` or `<plugin>.*`)"),
            ),
            _ => {}
        }
        let types = match present(s, "types") {
            None => Vec::new(),
            Some(_) if event.is_some() => {
                v.err(
                    ptr(&base, "types"),
                    "types apply to topic subscriptions only",
                );
                Vec::new()
            }
            Some(t) => {
                let path = ptr(&base, "types");
                let types = v.string_list(&path, t);
                for (j, ty) in types.iter().enumerate() {
                    if ty.is_empty() || !ty.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    {
                        v.err(idx(&path, j), format!("`{ty}` is not a type tag"));
                    }
                }
                types
            }
        };
        let version = match present(s, "version") {
            None => Vec::new(),
            Some(Value::Array(vs)) => {
                let path = ptr(&base, "version");
                vs.iter()
                    .enumerate()
                    .filter_map(|(j, x)| v.uint(&idx(&path, j), x))
                    .collect()
            }
            Some(x) => v.uint(&ptr(&base, "version"), x).into_iter().collect(),
        };
        let deltas = present(s, "deltas")
            .and_then(|d| v.boolean(&ptr(&base, "deltas"), d))
            .unwrap_or(false);
        out.push(Subscription {
            topic,
            event,
            types,
            version,
            deltas,
        });
    }
    out
}

fn emit_decls(v: &mut V, val: &Value) -> BTreeMap<String, EmitDecl> {
    let Some(m) = val.as_object() else {
        v.err("/emits", "expected a mapping of event names");
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (name, body) in m {
        let base = ptr("/emits", name);
        if !emit_name_ok(name) {
            v.err(
                &base,
                format!("event name `{name}` must match ^[a-z][a-z0-9_]{{0,47}}$"),
            );
        }
        let empty = Map::new();
        let e = match body {
            Value::Null => &empty,
            other => match v.map(&base, other) {
                Some(e) => e,
                None => continue,
            },
        };
        for k in e.keys() {
            match k.as_str() {
                "description" | "version" | "scope" | "schema" => {}
                "exposure" => v.err(
                    ptr(&base, k),
                    "`exposure` is not supported by this version: the event would be public, not private as asked",
                ),
                _ => v.warn(ptr(&base, k), format!("unknown key `{k}`; ignored")),
            }
        }
        let description = present(e, "description")
            .and_then(|d| v.scalar(&ptr(&base, "description"), d))
            .unwrap_or_default();
        let version = match present(e, "version") {
            None => 1,
            Some(x) => match v.uint(&ptr(&base, "version"), x) {
                Some(0) => {
                    v.err(ptr(&base, "version"), "version starts at 1");
                    1
                }
                Some(n) => n,
                None => 1,
            },
        };
        let scope = present(e, "scope")
            .and_then(|s| {
                v.one_of(
                    &ptr(&base, "scope"),
                    s,
                    &[("global", Scoped::Global), ("session", Scoped::Session)],
                )
            })
            .unwrap_or(Scoped::Global);
        let schema = match present(e, "schema") {
            None => Value::Object(Map::new()),
            Some(s @ Value::Object(_)) => s.clone(),
            Some(_) => {
                v.err(ptr(&base, "schema"), "schema must be a JSON object");
                Value::Object(Map::new())
            }
        };
        out.insert(
            name.clone(),
            EmitDecl {
                description,
                version,
                scope,
                schema,
            },
        );
    }
    out
}

fn placement_decls(v: &mut V, val: &Value, plugin: &str) -> Vec<PlacementDecl> {
    let Some(items) = val.as_array() else {
        v.err("/placements", "expected a list of placements");
        return Vec::new();
    };
    let mut out: Vec<PlacementDecl> = Vec::new();
    let mut at: Vec<usize> = Vec::new();
    let mut ids = BTreeSet::new();
    let mut commands = BTreeSet::new();
    for (i, item) in items.iter().enumerate() {
        let base = idx("/placements", i);
        let Some(p) = v.map(&base, item) else {
            continue;
        };
        let kind_path = ptr(&base, "kind");
        let kind = match present(p, "kind") {
            None => {
                v.err(kind_path, "required");
                continue;
            }
            Some(Value::String(k)) => k.as_str(),
            Some(_) => {
                v.err(kind_path, "expected a string");
                continue;
            }
        };
        if RESERVED_KINDS.contains(&kind) {
            v.warn(
                kind_path,
                format!(
                    "placement kind `{kind}` is reserved for a later version; placement skipped"
                ),
            );
            continue;
        }
        let kind = match kind {
            "sidebar" => PlacementKind::Sidebar,
            "status" => PlacementKind::Status,
            "tile" => PlacementKind::Tile,
            "command" => PlacementKind::Command,
            other => {
                v.err(
                    kind_path,
                    format!("unknown placement kind `{other}` (sidebar | status | tile | command)"),
                );
                continue;
            }
        };
        if present(p, "format").and_then(Value::as_str) == Some("html") {
            v.warn(
                ptr(&base, "format"),
                "format `html` is reserved for a later version; placement skipped",
            );
            continue;
        }
        if let Some(k) = ["file", "scope"].into_iter().find(|k| p.contains_key(*k)) {
            v.warn(
                ptr(&base, k),
                format!("placement key `{k}` is reserved for a later version; placement skipped"),
            );
            continue;
        }
        let (has_format, known): (bool, &[&str]) = match kind {
            PlacementKind::Sidebar => (
                true,
                &[
                    "id", "kind", "open", "icon", "label", "status", "size", "format", "fallback",
                ],
            ),
            PlacementKind::Tile => (
                true,
                &[
                    "id", "kind", "icon", "label", "status", "size", "format", "fallback",
                ],
            ),
            PlacementKind::Status => (false, &["id", "kind", "icon", "label", "status"]),
            PlacementKind::Command => (
                false,
                &["id", "kind", "icon", "label", "name", "description", "args"],
            ),
        };
        for k in p.keys().filter(|k| !known.contains(&k.as_str())) {
            match k.as_str() {
                "format" => v.err(
                    ptr(&base, k),
                    format!("{kind:?} placements have no format").to_lowercase(),
                ),
                "open" => v.err(ptr(&base, k), "open applies to sidebar placements only"),
                _ => v.warn(ptr(&base, k), format!("unknown key `{k}`; ignored")),
            }
        }

        let id_path = ptr(&base, "id");
        let id = match present(p, "id") {
            None => {
                v.err(&id_path, "required");
                String::new()
            }
            Some(x) => v.string(&id_path, x).unwrap_or_default(),
        };
        if !id.is_empty() {
            if !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                v.err(
                    &id_path,
                    format!("placement id `{id}` must match [a-z0-9-]+"),
                );
            } else if !ids.insert(id.clone()) {
                v.err(&id_path, format!("duplicate placement id `{id}`"));
            }
        }
        let open = (kind == PlacementKind::Sidebar).then(|| {
            present(p, "open")
                .and_then(|o| {
                    v.one_of(
                        &ptr(&base, "open"),
                        o,
                        &[("popover", OpenMode::Popover), ("tile", OpenMode::Tile)],
                    )
                })
                .unwrap_or(OpenMode::Popover)
        });
        let drawn = kind != PlacementKind::Command;
        let icon = match present(p, "icon") {
            None if drawn => {
                v.err(ptr(&base, "icon"), "required");
                String::new()
            }
            None => String::new(),
            Some(x) => v.string(&ptr(&base, "icon"), x).unwrap_or_default(),
        };
        let label = match present(p, "label") {
            None if drawn => {
                v.err(ptr(&base, "label"), "required");
                String::new()
            }
            None => String::new(),
            Some(x) => v.scalar(&ptr(&base, "label"), x).unwrap_or_default(),
        };
        // A `kind: status` placement has no surface: `status` updates are its only output, and
        // both clients draw it as status-bearing (review S1).
        let status = kind == PlacementKind::Status
            || (drawn
                && present(p, "status")
                    .and_then(|s| v.boolean(&ptr(&base, "status"), s))
                    .unwrap_or(false));
        let size = if has_format {
            present(p, "size").and_then(|s| size(v, &ptr(&base, "size"), s))
        } else {
            None
        };
        let format = if has_format {
            let needed = kind == PlacementKind::Tile || open == Some(OpenMode::Popover);
            match present(p, "format") {
                None if needed => {
                    v.err(
                        ptr(&base, "format"),
                        "required (openui | json-render | markdown)",
                    );
                    None
                }
                None => None,
                Some(f) => v.one_of(
                    &ptr(&base, "format"),
                    f,
                    &[
                        ("openui", SurfaceFormat::Openui),
                        ("json-render", SurfaceFormat::JsonRender),
                        ("markdown", SurfaceFormat::Markdown),
                    ],
                ),
            }
        } else {
            None
        };
        let fallback = if has_format {
            present(p, "fallback")
                .and_then(|f| {
                    v.one_of(
                        &ptr(&base, "fallback"),
                        f,
                        &[("text", Fallback::Text), ("hide", Fallback::Hide)],
                    )
                })
                .unwrap_or_default()
        } else {
            Fallback::Text
        };
        let command = (kind == PlacementKind::Command).then(|| {
            let path = ptr(&base, "name");
            let name = match present(p, "name") {
                None => plugin.to_owned(),
                Some(x) => v.string(&path, x).unwrap_or_default(),
            };
            if !name_syntax(&name) {
                v.err(
                    &path,
                    format!("command name `{name}` must match ^[a-z][a-z0-9-]{{0,31}}$"),
                );
            } else if !commands.insert(name.clone()) {
                v.err(&path, format!("duplicate command `{name}`"));
            }
            let description = present(p, "description")
                .and_then(|d| v.scalar(&ptr(&base, "description"), d))
                .unwrap_or_default();
            let args = present(p, "args").and_then(|a| v.scalar(&ptr(&base, "args"), a));
            CommandDecl {
                name,
                description,
                args,
            }
        });
        let label = match (&command, label.is_empty()) {
            (Some(c), true) => c.name.clone(),
            _ => label,
        };
        out.push(PlacementDecl {
            id,
            kind,
            open,
            icon,
            label,
            status,
            size,
            format,
            fallback,
            command,
        });
        at.push(i);
    }
    if !out.iter().any(|p| p.kind == PlacementKind::Tile) {
        for (p, i) in out.iter().zip(&at) {
            if p.open == Some(OpenMode::Tile) {
                v.err(
                    ptr(&idx("/placements", *i), "open"),
                    "open: tile needs a `tile` placement in this manifest",
                );
            }
        }
    }
    out
}

fn size(v: &mut V, path: &str, val: &Value) -> Option<Size> {
    let m = v.map(path, val)?;
    let mut dim = |k: &str| match present(m, k) {
        None => {
            v.err(ptr(path, k), "required");
            None
        }
        Some(x) => match v.uint(&ptr(path, k), x) {
            Some(0) => {
                v.err(ptr(path, k), "must be above 0");
                None
            }
            n => n,
        },
    };
    let (width, height) = (dim("width"), dim("height"));
    Some(Size {
        width: width?,
        height: height?,
    })
}

fn tool_decls(v: &mut V, val: &Value) -> Vec<ToolDecl> {
    let Some(items) = val.as_array() else {
        v.err("/tools", "declare tools as a list");
        return Vec::new();
    };
    let mut names = BTreeSet::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let base = idx("/tools", i);
        let Some(t) = v.map(&base, item) else {
            continue;
        };
        v.unknown_keys(
            &base,
            t,
            &[
                "name",
                "description",
                "input_schema",
                "output_schema",
                "annotations",
            ],
        );
        let name_path = ptr(&base, "name");
        let name = match present(t, "name") {
            None => {
                v.err(&name_path, "required");
                String::new()
            }
            Some(x) => v.string(&name_path, x).unwrap_or_default(),
        };
        if !tool_name_ok(&name) {
            v.err(
                &name_path,
                format!("tool name `{name}` must match [a-zA-Z0-9_-]{{1,64}}"),
            );
        } else if !names.insert(name.clone()) {
            v.err(&name_path, format!("duplicate tool `{name}`"));
        }
        let description = present(t, "description")
            .and_then(|d| v.scalar(&ptr(&base, "description"), d))
            .unwrap_or_default();
        let schema_path = ptr(&base, "input_schema");
        let input_schema = match present(t, "input_schema") {
            Some(s @ Value::Object(o))
                if o.get("type").and_then(Value::as_str) == Some("object") =>
            {
                s.clone()
            }
            None => {
                v.err(&schema_path, "required");
                Value::Null
            }
            Some(_) => {
                v.err(
                    &schema_path,
                    "input_schema must be an object schema (`type: object`)",
                );
                Value::Null
            }
        };
        let output_schema = match present(t, "output_schema") {
            None => None,
            Some(s @ Value::Object(_)) => Some(s.clone()),
            Some(_) => {
                v.err(
                    ptr(&base, "output_schema"),
                    "output_schema must be a JSON object",
                );
                None
            }
        };
        let (mut read_only_hint, mut destructive_hint) = (None, None);
        if let Some(a) = present(t, "annotations") {
            let path = ptr(&base, "annotations");
            if let Some(a) = v.map(&path, a) {
                read_only_hint = present(a, "readOnlyHint")
                    .and_then(|h| v.boolean(&ptr(&path, "readOnlyHint"), h));
                destructive_hint = present(a, "destructiveHint")
                    .and_then(|h| v.boolean(&ptr(&path, "destructiveHint"), h));
            }
        }
        out.push(ToolDecl {
            name,
            description,
            input_schema,
            output_schema,
            read_only_hint,
            destructive_hint,
        });
    }
    out
}

fn capability_list(v: &mut V, val: &Value) -> Capabilities {
    let mut c = Capabilities::default();
    let Some(items) = val.as_array() else {
        v.err("/capabilities", "expected a list of capabilities");
        return c;
    };
    for (i, item) in items.iter().enumerate() {
        let path = idx("/capabilities", i);
        let (name, body) = match item {
            Value::String(s) => (s.as_str(), None),
            Value::Object(o) if o.len() == 1 => match o.iter().next() {
                Some((k, b)) => (k.as_str(), Some(b)),
                None => continue,
            },
            _ => {
                v.err(
                    &path,
                    "expected `notify`, `input` or `call: {tools, max_risk}`",
                );
                continue;
            }
        };
        match (name, body) {
            ("notify", None) => c.notify = true,
            ("input", None) => c.input = true,
            ("open", _) => v.warn(
                &path,
                "capability `open` is reserved for a later version; ignored",
            ),
            ("call", Some(b)) => c.call = call_grant(v, &ptr(&path, "call"), b),
            ("call", None) => v.err(&path, "call needs `{tools, max_risk}`"),
            ("notify" | "input", Some(_)) => v.err(&path, format!("`{name}` takes no settings")),
            (other, _) => v.err(
                &path,
                format!("unknown capability `{other}` (notify | input | call)"),
            ),
        }
    }
    c
}

fn call_grant(v: &mut V, path: &str, val: &Value) -> Option<CallGrant> {
    let m = v.map(path, val)?;
    v.unknown_keys(path, m, &["tools", "max_risk"]);
    let tools_path = ptr(path, "tools");
    let tools = match present(m, "tools") {
        None => {
            v.err(&tools_path, "required");
            Vec::new()
        }
        Some(t) => v.string_list(&tools_path, t),
    };
    for (i, t) in tools.iter().enumerate() {
        if !tool_name_ok(t) {
            v.err(idx(&tools_path, i), format!("`{t}` is not a tool name"));
        }
    }
    let max_risk = present(m, "max_risk")
        .and_then(|r| {
            v.one_of(
                &ptr(path, "max_risk"),
                r,
                &[
                    ("read_only", Risk::ReadOnly),
                    ("mutating", Risk::Mutating),
                    ("destructive", Risk::Destructive),
                ],
            )
        })
        .unwrap_or(Risk::ReadOnly);
    Some(CallGrant { tools, max_risk })
}

// ---- small helpers ----

fn issue(path: &str, message: impl Into<String>) -> CheckIssue {
    CheckIssue {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// `base` + `/key` with JSON Pointer escaping.
fn ptr(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn idx(base: &str, i: usize) -> String {
    format!("{base}/{i}")
}

/// The value of `key`, treating YAML `~`/empty as absent.
fn present<'a>(m: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    m.get(key).filter(|v| !v.is_null())
}

/// Refuse control characters in every key and string of the manifest (review R11): an ANSI
/// sequence or a carriage return in `version`, `run` or a tool name could repaint the card a
/// human approves on a terminal. A `description` (at any depth) may hold newlines and tabs.
fn control_chars(v: &mut V, path: &str, val: &Value) {
    let bad = |s: &str, prose: bool| {
        s.chars()
            .any(|c| c.is_control() && !(prose && (c == '\n' || c == '\t')))
    };
    match val {
        Value::String(s) => {
            let prose = path.rsplit('/').next() == Some("description");
            if bad(s, prose) {
                v.err(path, "control characters are not allowed");
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                control_chars(v, &idx(path, i), item);
            }
        }
        Value::Object(map) => {
            for (k, item) in map {
                let p = ptr(path, k);
                if bad(k, false) {
                    v.err(p.clone(), "control characters are not allowed in a key");
                }
                control_chars(v, &p, item);
            }
        }
        _ => {}
    }
}

/// `^[a-z][a-z0-9-]{0,31}$`.
fn name_syntax(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `^[a-z][a-z0-9_]{0,47}$`.
fn emit_name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// `[a-zA-Z0-9_-]{1,64}`.
fn tool_name_ok(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn topic_ok(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'_')
        })
}

/// `<plugin>.<event>` or `<plugin>.*`.
fn event_ref_ok(s: &str) -> bool {
    s.split_once('.')
        .is_some_and(|(p, e)| name_syntax(p) && (e == "*" || emit_name_ok(e)))
}

/// sha256 hex of the canonical JSON (keys sorted, no whitespace).
fn manifest_hash(v: &Value) -> String {
    let mut s = String::new();
    canonical(v, &mut s);
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(&String, &Value)> = m.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (k, x)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(x, out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Vault secret names referenced by double-brace `secret:NAME` placeholders, sorted, unique.
fn secret_names(env: &BTreeMap<String, String>) -> Vec<String> {
    const OPEN: &str = "{";
    const CLOSE: &str = "}";
    let open = [OPEN, OPEN, "secret:"].concat();
    let close = [CLOSE, CLOSE].concat();
    let mut out = BTreeSet::new();
    for value in env.values() {
        let mut rest = value.as_str();
        while let Some(at) = rest.find(&open) {
            let after = &rest[at + open.len()..];
            let Some(end) = after.find(&close) else { break };
            let name = after[..end].trim();
            if !name.is_empty() {
                out.insert(name.to_owned());
            }
            rest = &after[end + close.len()..];
        }
    }
    out.into_iter().collect()
}

fn resolves(dir: &Path, cwd: &str, arg0: &str) -> bool {
    let p = Path::new(arg0);
    if p.is_absolute() {
        return p.is_file();
    }
    dir.join(cwd).join(p).is_file()
        || (!arg0.contains('/') && platform::shell::which(arg0).is_some())
}

fn rel_key(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_into(out: &mut BTreeMap<String, String>, file: &Path, rel: &Path) {
    let Ok(bytes) = std::fs::read(file) else {
        return;
    };
    out.insert(rel_key(rel), hex::encode(Sha256::digest(&bytes)));
}

/// Every regular file under `cur` but the unhashed ones. A symlink to a file is hashed by its
/// content; any other symlink (a directory, a dangling link) by its target path, since it is not
/// entered.
fn walk(root: &Path, cur: &Path, out: &mut BTreeMap<String, String>) {
    let Ok(entries) = std::fs::read_dir(cur) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let name = e.file_name();
        let skipped = if ft.is_dir() {
            UNHASHED_DIRS.iter().any(|d| name == *d)
        } else {
            rel == Path::new(FILE) || name == ".DS_Store"
        };
        if skipped {
            continue;
        }
        if ft.is_dir() {
            walk(root, &path, out);
        } else if path.is_file() {
            hash_into(out, &path, rel);
        } else if ft.is_symlink()
            && let Ok(target) = std::fs::read_link(&path)
        {
            let digest = Sha256::digest(format!("symlink:{}", target.display()).as_bytes());
            out.insert(rel_key(rel), hex::encode(digest));
        }
    }
}

/// `/<plugin>` when the command is named after the plugin, else `/<plugin>:<name>`.
fn slash(plugin: &str, name: &str) -> String {
    if name == plugin {
        format!("/{plugin}")
    } else {
        format!("/{plugin}:{name}")
    }
}

fn risk_word(r: Risk) -> &'static str {
    match r {
        Risk::ReadOnly => "read-only",
        Risk::Mutating => "mutating",
        Risk::Destructive => "destructive",
    }
}

/// The path with the home directory shown as `~`.
fn tilde(p: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return Path::new("~").join(rest).display().to_string();
    }
    p.display().to_string()
}

fn subscribe_text(name: &str, describe: &dyn Fn(&str) -> Option<String>) -> String {
    let (head, channel) = match name.split_once('#') {
        Some((topic, ty)) => (
            format!("Reads {topic}: {ty}"),
            topic == "agent.events" && CHANNEL_TEXT_TYPES.contains(&ty),
        ),
        None => (format!("Reads {name}"), name == "agent.events"),
    };
    let mut text = match describe(name) {
        Some(d) if !d.is_empty() => format!("{head} — {d}"),
        _ => head,
    };
    if channel {
        text.push_str(", including messages people send theuth on Slack/Telegram/Signal");
    }
    text
}

fn placement_text(p: &PlacementDecl, plugin: &str) -> String {
    match p.kind {
        PlacementKind::Sidebar if p.open == Some(OpenMode::Tile) => {
            "Adds a sidebar icon that opens its tile".to_owned()
        }
        PlacementKind::Sidebar => "Adds a sidebar icon with a popover".to_owned(),
        PlacementKind::Status => "Adds a status-strip item".to_owned(),
        PlacementKind::Tile => "Adds a tile".to_owned(),
        PlacementKind::Command => {
            let name = p.command.as_ref().map_or(plugin, |c| c.name.as_str());
            format!("Adds the command {}", slash(plugin, name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Design §4.2, with the secret placeholder line as `@SECRET@`.
    const DESIGN: &str = r#"
name: turns                       # required; ^[a-z][a-z0-9-]{0,31}$; must equal the folder name
version: 0.2.0                    # required; free text shown on the card
description: Counts finished turns today; the sidebar dot turns amber past a daily limit.
protocol: 1                       # required; the wire protocol major (§6)
run: [python3, main.py]           # required; argv list, or a string split by shell words (no shell is run)
cwd: .                            # optional; relative to the folder
env:
  TURNS_LIMIT: "50"
  GITHUB_TOKEN: "@SECRET@"
restart: on-failure               # on-failure (default) | always | never
watch: [main.py, "lib/**/*.py"]   # optional; restart on change, and the code hash covers these (§10.2)

subscribe:                        # optional; nothing is delivered that is not listed here
  - topic: agent.events           # a wire topic …
    types: [TurnFinished]         # … optionally narrowed to `type` tags
    version: [1]                  # optional; the versions this plugin understands (default: the current one)
  - topic: tools.approval-requested
  - event: other-plugin.something # a plugin event (§7.6); `other-plugin.*` for all of one plugin's events
  - topic: agent.events
    types: [TextDelta]
    deltas: true                  # delta-class types are opt-in per subscription (high volume)

emits:                            # optional; the only events this plugin may `emit`
  limit_reached:
    description: The daily turn count reached TURNS_LIMIT.
    version: 1                    # default 1
    scope: global                 # global (default) | session (session events must carry `session`)
    schema:                       # optional JSON Schema, published in the catalog for subscribers;
      type: object                # v1 checks only that `data` is an object (§7.6)
      required: [day, turns]
      properties: {day: {type: string}, turns: {type: integer}}

placements:                       # optional; §9
  - id: turns                     # unique within the plugin; [a-z0-9-]+
    kind: sidebar                 # sidebar | status | tile | command  (others reserved, §20)
    open: popover                 # sidebar only: popover (default) | tile
    icon: gauge                   # a lucide icon name, or a path to an SVG in the folder
    label: Turns today            # tooltip / menu text / tile title
    status: true                  # draws a status dot (+ badge) fed by `status` messages
    size: {width: 320, height: 220}   # popover/tile default; tiles are resizable
    format: openui                # openui | json-render | markdown  (html reserved, §20)
    fallback: text                # what non-rendering surfaces show: text (default) | hide

tools:                            # optional; static, in MCP's Tool shape (§8)
  - name: turns_today
    description: How many turns finished today and how many tokens they used.
    input_schema: {type: object, properties: {}}
    annotations: {readOnlyHint: true}

capabilities:                     # optional; each is a line on the install card (§10)
  - notify                        # inbox items + OS notifications
  - input                         # send text into a session as the user (destructive, rate-limited)
  - call:                         # host-brokered tool calls, in the plugin's own headless session
      tools: [knowledge_search, http_request]
      max_risk: read_only
"#;

    fn placeholder(name: &str) -> String {
        let open = "{".repeat(2);
        format!("{open}secret:{name}}}}}")
    }

    fn base(extra: &str) -> String {
        format!("name: demo\nversion: '1'\nprotocol: 1\nrun: [sh, main.sh]\n{extra}")
    }

    fn errors(text: &str) -> Vec<CheckIssue> {
        match parse(text) {
            Ok(_) => panic!("expected errors for:\n{text}"),
            Err((e, _)) => e,
        }
    }

    fn has(issues: &[CheckIssue], path: &str) -> bool {
        issues.iter().any(|i| i.path == path)
    }

    #[test]
    fn design_example_parses() {
        let text = DESIGN.replace("@SECRET@", &placeholder("github.pat"));
        let p = parse(&text).unwrap_or_else(|(e, _)| panic!("{e:?}"));
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let m = &p.manifest;
        assert_eq!(m.name, "turns");
        assert_eq!(m.run, ["python3", "main.py"]);
        assert_eq!(m.subscribe.len(), 4);
        assert!(m.subscribe[3].deltas);
        assert_eq!(
            m.subscribe[2].event.as_deref(),
            Some("other-plugin.something")
        );
        assert_eq!(m.emits["limit_reached"].scope, Scoped::Global);
        assert_eq!(
            m.placements[0].size,
            Some(Size {
                width: 320,
                height: 220
            })
        );
        assert_eq!(m.placements[0].format, Some(SurfaceFormat::Openui));
        assert_eq!(m.watch, ["main.py", "lib/**/*.py"]);
        assert_eq!(p.manifest_sha256.len(), 64);
        // Comments and key order do not change the manifest hash.
        let reparsed =
            parse(&text.replace("# required; free text shown on the card", "")).map_err(|(e, _)| e);
        assert_eq!(
            reparsed.map(|r| r.manifest_sha256).ok(),
            Some(p.manifest_sha256.clone())
        );

        let req = requested(m, &hint_risk);
        assert_eq!(req.secrets, ["github.pat"]);
        assert_eq!(
            req.subscribe,
            [
                "agent.events#TurnFinished",
                "tools.approval-requested",
                "other-plugin.something",
                "agent.events#TextDelta"
            ]
        );
        assert_eq!(
            req.tools,
            [ToolGrant {
                name: "turns_today".into(),
                risk: Risk::ReadOnly
            }]
        );
        assert!(needs_code_hash(&req));

        let describe =
            |n: &str| (n == "agent.events#TurnFinished").then(|| "a turn finished".to_owned());
        let lines = card_lines(m, &req, None, Path::new("/x/plugins/turns"), 6, &describe);
        assert_eq!(lines[0].key, GrantKey::Run);
        assert_eq!(lines[0].risk, Risk::Destructive);
        assert_eq!(
            lines[0].text,
            "Runs `python3 main.py` from `/x/plugins/turns` as you, with your files and network"
        );
        let text_of = |k: GrantKey| {
            lines
                .iter()
                .filter(|l| l.key == k)
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            text_of(GrantKey::Secret),
            ["Gets the secret `github.pat` in its environment"]
        );
        assert_eq!(
            text_of(GrantKey::Subscribe)[0],
            "Reads agent.events: TurnFinished — a turn finished"
        );
        assert_eq!(text_of(GrantKey::Emit), ["Publishes `turns.limit_reached`"]);
        assert_eq!(
            text_of(GrantKey::Placement),
            ["Adds a sidebar icon with a popover"]
        );
        assert_eq!(
            text_of(GrantKey::Tool),
            ["Offers the model 1 tool: turns_today (read-only). The model reads what it returns."]
        );
        assert_eq!(
            text_of(GrantKey::Call),
            ["May call knowledge_search, http_request (read-only), in its own background session"]
        );
        assert_eq!(text_of(GrantKey::Input).len(), 1);
        assert!(text_of(GrantKey::Input)[0].contains("at most 6 a minute"));
        let ph = placeholder("github.pat");
        assert_eq!(
            text_of(GrantKey::Run)[1],
            format!("With the environment `GITHUB_TOKEN='{ph}' TURNS_LIMIT=50`"),
            "R17: env keys and unexpanded values are on the card"
        );
        let r32 = card_lines(m, &req, None, Path::new("/x"), 2, &describe);
        assert!(
            r32.iter()
                .any(|l| l.key == GrantKey::Input && l.text.contains("at most 2 a minute"))
        );
        assert_eq!(
            text_of(GrantKey::Notify),
            ["May add inbox items and notifications"]
        );
        assert!(lines.iter().all(|l| !l.new));

        // Widen: nothing new → no lines; a new emit and a changed env → Run, env and Emit lines.
        assert!(card_lines(m, &req, Some(&req), Path::new("/x"), 6, &describe).is_empty());
        let mut prior = req.clone();
        prior.emits.clear();
        prior.env.remove("TURNS_LIMIT");
        let widen = card_lines(m, &req, Some(&prior), Path::new("/x"), 6, &describe);
        assert_eq!(
            widen.iter().map(|l| l.key).collect::<Vec<_>>(),
            [GrantKey::Run, GrantKey::Run, GrantKey::Emit]
        );
        assert!(widen.iter().all(|l| l.new));
    }

    #[test]
    fn examples_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/216-examples");
        let mut seen = 0;
        for entry in std::fs::read_dir(&root).expect("examples dir") {
            let dir = entry.expect("entry").path();
            let Ok(text) = std::fs::read_to_string(dir.join(FILE)) else {
                continue;
            };
            let p = parse(&text).unwrap_or_else(|(e, _)| panic!("{}: {e:?}", dir.display()));
            assert_eq!(
                Some(p.manifest.name.as_str()),
                dir.file_name().and_then(|f| f.to_str())
            );
            assert!(p.warnings.is_empty(), "{}: {:?}", dir.display(), p.warnings);
            seen += 1;
        }
        assert!(seen >= 2, "expected the turns and ping examples");
    }

    #[test]
    fn errors_have_pointer_paths() {
        let e = errors(
            "name: demo\nversion: '1'\nprotocol: 2\nrun: [sh]\n\
             emits:\n  BadName: {}\n  a/b: {}\n\
             placements:\n  - {id: ok, kind: bogus}\n  - {id: Bad_Id, kind: status, icon: x, label: y, format: openui}\n\
             subscribe:\n  - {topic: agent.events, event: x.y}\n\
             tools:\n  - {name: 'bad name', input_schema: {type: string}}\n\
             capabilities: [teleport]\n",
        );
        for path in [
            "/protocol",
            "/emits/BadName",
            "/emits/a~1b",
            "/placements/0/kind",
            "/placements/1/id",
            "/placements/1/format",
            "/subscribe/0",
            "/tools/0/name",
            "/tools/0/input_schema",
            "/capabilities/0",
        ] {
            assert!(has(&e, path), "missing {path} in {e:?}");
        }
        let missing = errors("description: nothing else\n");
        for path in ["/name", "/version", "/protocol", "/run"] {
            assert!(has(&missing, path), "missing {path} in {missing:?}");
        }
        let sidebar = errors(&base(
            "placements:\n  - {id: s, kind: sidebar, open: tile, icon: x, label: y}\n",
        ));
        assert!(has(&sidebar, "/placements/0/open"), "{sidebar:?}");
        let pop = errors(&base(
            "placements:\n  - {id: s, kind: sidebar, icon: x, label: y}\n",
        ));
        assert!(has(&pop, "/placements/0/format"), "{pop:?}");
        let dup = errors(&base(
            "placements:\n  - {id: s, kind: status, icon: x, label: y}\n  - {id: s, kind: status, icon: x, label: y}\n",
        ));
        assert!(has(&dup, "/placements/1/id"), "{dup:?}");
    }

    #[test]
    fn isolation_and_emit_exposure_are_errors() {
        let e = errors(&base(
            "isolation: sandbox\nemits:\n  x:\n    exposure: private\n",
        ));
        assert!(has(&e, "/isolation"), "{e:?}");
        assert!(has(&e, "/emits/x/exposure"), "{e:?}");
    }

    #[test]
    fn reserved_kind_is_skipped_with_warning() {
        let p = parse(&base(
            "mcp: {server: x}\n\
             subscribe:\n  - {topic: agent.events, sessions: current}\n\
             placements:\n\
             \x20 - {id: h, kind: header, icon: x, label: y}\n\
             \x20 - {id: w, kind: tile, icon: x, label: y, format: html}\n\
             \x20 - {id: f, kind: tile, icon: x, label: y, format: openui, file: ui.html}\n\
             \x20 - {id: t, kind: tile, icon: x, label: y, format: markdown}\n\
             \x20 - {id: s, kind: sidebar, open: tile, icon: x, label: y}\n\
             capabilities: [open, notify]\n\
             unknown_top: 1\n",
        ))
        .unwrap_or_else(|(e, _)| panic!("{e:?}"));
        let ids: Vec<&str> = p
            .manifest
            .placements
            .iter()
            .map(|x| x.id.as_str())
            .collect();
        assert_eq!(ids, ["t", "s"]);
        for path in [
            "/mcp",
            "/subscribe/0/sessions",
            "/placements/0/kind",
            "/placements/1/format",
            "/placements/2/file",
            "/capabilities/0",
            "/unknown_top",
        ] {
            assert!(
                has(&p.warnings, path),
                "missing warning {path} in {:?}",
                p.warnings
            );
        }
        assert!(p.manifest.capabilities.notify);
        let req = requested(&p.manifest, &hint_risk);
        let lines = card_lines(&p.manifest, &req, None, Path::new("/x"), 6, &|_| None);
        let placements: Vec<&str> = lines
            .iter()
            .filter(|l| l.key == GrantKey::Placement)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(
            placements,
            ["Adds a tile", "Adds a sidebar icon that opens its tile"]
        );
    }

    /// S1: a `kind: status` placement always takes `status` updates; others opt in.
    #[test]
    fn status_placement_defaults_to_status() {
        let p = parse(&base(
            "placements:\n  - {id: s, kind: status, icon: x, label: y}\n  - {id: t, kind: status, icon: x, label: y, status: false}\n  - {id: u, kind: tile, icon: x, label: y, format: markdown}\n  - {id: v, kind: tile, icon: x, label: y, format: markdown, status: true}\n",
        ))
        .unwrap_or_else(|(e, _)| panic!("{e:?}"));
        let status: Vec<bool> = p.manifest.placements.iter().map(|p| p.status).collect();
        assert_eq!(status, [true, true, false, true]);
    }

    /// R11: control characters in manifest strings are refused (a description may span lines).
    #[test]
    fn control_characters_are_refused() {
        let e = errors(&base("").replace("version: '1'", "version: \"1\\e[2K\""));
        assert!(has(&e, "/version"), "{e:?}");
        let e = errors(&base(
            "tools:\n  - {name: \"x\\r\", description: d, inputSchema: {type: object}}\n",
        ));
        assert!(e.iter().any(|i| i.path.starts_with("/tools/0")), "{e:?}");
        let e = errors(&base("env: {\"A\\e\": \"1\"}\n"));
        assert!(has(&e, "/env/A\u{1b}"), "{e:?}");
        let ok = parse(&base("description: \"line one\\nline\\ttwo\"\n"));
        assert!(ok.is_ok(), "{:?}", ok.err());
    }

    #[test]
    fn tools_inline_is_an_error() {
        let e = errors(&base("tools: inline\n"));
        assert!(
            e.iter()
                .any(|i| i.path == "/tools" && i.message.contains("declare tools as a list")),
            "{e:?}"
        );
    }

    #[test]
    fn code_hash_covers_every_file_but_caches() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("demo");
        std::fs::create_dir_all(dir.join("lib")).expect("mkdir");
        std::fs::create_dir_all(dir.join("__pycache__")).expect("mkdir");
        let write = |rel: &str, body: &str| std::fs::write(dir.join(rel), body).expect("write");
        write(
            FILE,
            &base("watch: [\"lib/**/*.py\"]\nenv: {TOKEN: \"@S@\"}\n")
                .replace("@S@", &placeholder("tok")),
        );
        write("main.sh", "echo hi\n");
        write("lib/a.py", "x = 1\n");
        write("README.md", "hello\n");
        write("__pycache__/a.pyc", "cache\n");

        let p = load(&dir).unwrap_or_else(|(e, _)| panic!("{e:?}"));
        assert_eq!(p.manifest.watch, ["lib/**/*.py"]);
        let files = code_files(&dir);
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["README.md", "lib/a.py", "main.sh"]
        );
        let h0 = code_hash(&files);

        write("__pycache__/a.pyc", "other cache\n");
        assert_eq!(code_hash(&code_files(&dir)), h0, "caches are not hashed");
        // R4: a new file beside the entry point, outside `watch`, changes the hash.
        write("sitecustomize.py", "import os\n");
        let h1 = code_hash(&code_files(&dir));
        assert_ne!(h1, h0);
        write("main.sh", "echo bye\n");
        let h2 = code_hash(&code_files(&dir));
        assert_ne!(h2, h1);
        write("lib/a.py", "x = 2\n");
        assert_ne!(code_hash(&code_files(&dir)), h2);

        let report = check_offline(&dir);
        assert!(report.ok, "{:?}", report.errors);
        assert_eq!(report.name.as_deref(), Some("demo"));
        assert!(report.code_sha256.is_some() && report.manifest_sha256.is_some());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.message.contains("engine not running"))
        );
        assert_eq!(report.lines[0].key, GrantKey::Run);
        assert!(
            report
                .lines
                .iter()
                .any(|l| l.key == GrantKey::Secret && l.text.contains("`tok`"))
        );

        // R20: a source folder may be named anything; an installed folder must carry the name.
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).expect("mkdir");
        std::fs::copy(dir.join(FILE), other.join(FILE)).expect("copy");
        let source = check_offline(&other);
        assert!(source.ok, "{:?}", source.errors);
        assert_eq!(source.name.as_deref(), Some("demo"));
        let Err((errors, _)) = load(&other) else {
            panic!("installed under the wrong name must fail");
        };
        assert!(errors.iter().any(|e| e.path == "/name"), "{errors:?}");
    }

    #[test]
    fn name_rules() {
        let mut sorted = RESERVED_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, RESERVED_NAMES);
        for ok in ["turns", "a", "a-1", "ping2", &"a".repeat(32)] {
            assert_eq!(check_name(ok), Ok(()), "{ok}");
        }
        for bad in ["", "Turns", "1a", "-a", "a_b", "a.b", &"a".repeat(33)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        for reserved in [
            "model", "quit", "plugins", "edit", "tools", "order", "hidden",
        ] {
            assert!(check_name(reserved).is_err(), "{reserved}");
        }
        assert_eq!(
            check_name("model").map_err(|e| e.to_string()),
            Err("plugin name `model` collides with /model".to_owned())
        );
        let p = parse(&base("").replace("name: demo", "name: model"));
        assert!(p.is_err_and(|(e, _)| has(&e, "/name")));
    }
}
