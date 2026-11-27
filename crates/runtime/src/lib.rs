//! The extension point. A plugin is a crate exporting one [`Plugin`]; the
//! binary holds a list of them and nothing else. Plugins talk through the
//! kameo message bus (events, type-routed) and the kameo registry (commands,
//! `ask` on a named actor). This crate must stay small (design doc §4).

use std::path::{Path, PathBuf};

use kameo::actor::ActorRef;
use kameo::mailbox;
use kameo::message::Message;
use kameo_actors::DeliveryStrategy;
use kameo_actors::message_bus::{MessageBus, Publish, Register};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use kameo;
pub use kameo_actors;

pub mod side_call;
pub use side_call::{CallKind, ModelRole, SideCall, SideCallable, SideReply};

type Extensions =
    std::collections::HashMap<std::any::TypeId, Box<dyn std::any::Any + Send + Sync>>;

/// Shared handles handed to every plugin at start.
#[derive(Clone)]
pub struct Runtime {
    /// The single type-routed event bus.
    pub bus: ActorRef<MessageBus>,
    /// Merged configuration; plugins extract their own section via [`Runtime::section`].
    pub config: figment::Figment,
    /// Directory owning all state: config, sqlite, logs, token, skills.
    pub data_dir: PathBuf,
    /// Cancelled when the process should exit (UI quit, ctrl-c, RPC shutdown).
    pub shutdown: CancellationToken,
    /// Plugin-neutral state shared by Runtime clones, keyed by component type.
    extensions: std::sync::Arc<parking_lot::Mutex<Extensions>>,
    /// Set to `true` by [`Runtime::mark_plugins_started`] once every plugin has
    /// finished starting, so [`Runtime::lookup_when`] stops polling for absent
    /// actors.
    plugins_started: std::sync::Arc<tokio::sync::watch::Sender<bool>>,
}

impl Runtime {
    /// Build a runtime with a `Guaranteed` bus (design doc §3). The bus takes
    /// the same unbounded mailbox as its subscribers: `publish` is awaited on
    /// every streaming chunk, and kameo's default bounded(64) would park the
    /// publisher behind a slow fan-out.
    pub fn new(config: figment::Figment, data_dir: PathBuf) -> Self {
        let bus = kameo::actor::Spawn::spawn_with_mailbox(
            MessageBus::new(DeliveryStrategy::Guaranteed),
            subscriber_mailbox(),
        );
        let (plugins_started, _) = tokio::sync::watch::channel(false);
        Self {
            bus,
            config,
            data_dir,
            shutdown: CancellationToken::new(),
            extensions: Default::default(),
            plugins_started: std::sync::Arc::new(plugins_started),
        }
    }

    /// Signal that every plugin's [`Plugin::start`] has finished and no further
    /// named actors will register. Once set, [`Runtime::lookup_when`] returns
    /// `None` after one final registry check instead of polling until
    /// [`Runtime::shutdown`].
    pub fn mark_plugins_started(&self) {
        self.plugins_started.send_if_modified(|started| {
            if *started {
                false
            } else {
                *started = true;
                true
            }
        });
    }

    /// One slot per type; a private T lets only its owning crate reach the slot.
    /// Initialization runs unlocked and may race; the first inserted value wins.
    pub fn extension<T: Clone + Send + Sync + 'static>(&self, init: impl FnOnce() -> T) -> T {
        if let Some(value) = self.extensions.lock().get(&std::any::TypeId::of::<T>()) {
            return value.downcast_ref::<T>().unwrap().clone();
        }
        let value = init();
        let mut slots = self.extensions.lock();
        let value = slots.entry(std::any::TypeId::of::<T>()).or_insert_with(|| Box::new(value));
        value.downcast_ref::<T>().unwrap().clone()
    }

    /// Publish an event to every actor registered for `T`.
    pub async fn publish<T: Clone + Send + 'static>(&self, event: T) {
        // The bus is unbounded-mailbox only in the sense that publishers use
        // tell; a dead bus is a process-level failure, not a per-event one.
        let _ = self.bus.tell(Publish(event)).await;
    }

    /// Register `actor` to receive every published `T`.
    pub async fn subscribe<A, T>(&self, actor: &ActorRef<A>)
    where
        A: kameo::Actor + Message<T>,
        T: Send + 'static,
    {
        let _ = self
            .bus
            .tell(Register(actor.clone().recipient::<T>()))
            .await;
    }

    /// Extract a config section, e.g. `rt.section::<AgentConfig>("agent")`.
    pub fn section<'de, T: Deserialize<'de>>(&self, name: &str) -> anyhow::Result<T> {
        Ok(self.config.extract_inner(name)?)
    }

    /// `ActorRef::<A>::lookup(name)` until it resolves, `shutdown` fires, or
    /// [`Runtime::mark_plugins_started`] has been called and a final check still
    /// finds nothing (12 §2). Polls every 50 ms while plugins are still starting:
    /// kameo's registry has no watch API.
    pub async fn lookup_when<A: kameo::actor::Actor>(
        &self,
        name: &'static str,
    ) -> Option<ActorRef<A>> {
        let mut started = self.plugins_started.subscribe();
        loop {
            // Read `started` before `lookup` so any registration that happened-before
            // `mark_plugins_started` is guaranteed visible to the final check.
            let done = *started.borrow_and_update();
            if let Ok(Some(r)) = ActorRef::<A>::lookup(name) {
                return Some(r);
            }
            if done {
                return None;
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => return None,
                _ = started.changed() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
        }
    }
}

/// Set `section.key: value` in the YAML file at `path`, keeping everything
/// else (comments included) byte-for-byte.
///
/// Rules: `section:` is a column-0 header, optionally with a `# comment`.
/// Only a DIRECT child `key:` line is rewritten, i.e. one at the block's child
/// indent (the indent of the first non-blank, non-comment line in the block;
/// 2 when there is none); deeper `key:` lines never match. A ` # comment`
/// tail on the rewritten line is kept. A missing key is inserted right after
/// the header at the child indent; a missing section is appended; a missing
/// file is created. A single-line flow header (`section: {a: 1, b: 2}`) is
/// expanded to block style first, entries kept verbatim; `null`, `~` and `{}`
/// count as an empty block; any other inline value is `InvalidData`. The
/// result must still parse with figment's YAML provider whenever the old
/// text did, otherwise nothing is written.
pub fn write_config_value(
    path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> std::io::Result<()> {
    let old = read_or_empty(path)?;
    let mut lines: Vec<String> = old.lines().map(str::to_owned).collect();
    let Some(start) = header_in(&lines, 0..lines.len(), 0, section) else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend([format!("{section}:"), format!("  {key}: {value}")]);
        return commit(path, &old, &lines);
    };
    expand_header(&mut lines, start, section)?;
    let (end, child) = extent(&lines, start, 0);
    set_key(&mut lines, start, end, child, key, value);
    commit(path, &old, &lines)
}

/// Set `section.subsection.key: value` while preserving the rest of the YAML.
/// Same rules as [`write_config_value`] applied at both levels: `subsection:`
/// must be a direct child of the section, and only a direct child `key:` of
/// the subsection (at its own child indent; section child indent + 2 when
/// empty) is rewritten. The subsection ends at the first non-blank,
/// non-comment line indented no deeper than the section's children.
pub fn write_nested_config_value(
    path: &Path,
    section: &str,
    subsection: &str,
    key: &str,
    value: &str,
) -> std::io::Result<()> {
    let old = read_or_empty(path)?;
    let mut lines: Vec<String> = old.lines().map(str::to_owned).collect();
    let Some(section_start) = header_in(&lines, 0..lines.len(), 0, section) else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend([
            format!("{section}:"),
            format!("  {subsection}:"),
            format!("    {key}: {value}"),
        ]);
        return commit(path, &old, &lines);
    };
    expand_header(&mut lines, section_start, section)?;
    let (section_end, child) = extent(&lines, section_start, 0);
    let Some(subsection_start) =
        header_in(&lines, section_start + 1..section_end, child, subsection)
    else {
        let pad = " ".repeat(child);
        lines.splice(
            section_start + 1..section_start + 1,
            [
                format!("{pad}{subsection}:"),
                format!("{pad}  {key}: {value}"),
            ],
        );
        return commit(path, &old, &lines);
    };
    expand_header(&mut lines, subsection_start, subsection)?;
    let (subsection_end, sub_child) = extent(&lines, subsection_start, child);
    set_key(
        &mut lines,
        subsection_start,
        subsection_end,
        sub_child,
        key,
        value,
    );
    commit(path, &old, &lines)
}

fn read_or_empty(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// Neither blank nor a comment.
fn is_content(l: &str) -> bool {
    let t = l.trim_start();
    !t.is_empty() && !t.starts_with('#')
}

/// Text after `name:` on `l` when `l` is a `name:` mapping key at `indent`.
fn header_value<'a>(l: &'a str, indent: usize, name: &str) -> Option<&'a str> {
    (indent_of(l) == indent)
        .then(|| l.trim_start().strip_prefix(name)?.strip_prefix(':'))
        .flatten()
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

fn header_in(
    lines: &[String],
    range: std::ops::Range<usize>,
    indent: usize,
    name: &str,
) -> Option<usize> {
    lines[range.clone()]
        .iter()
        .position(|l| header_value(l, indent, name).is_some())
        .map(|i| range.start + i)
}

/// End of the block headed at `start` (first content line indented no deeper
/// than `parent`) and the indent of its direct children (`parent + 2` when
/// the block is empty).
fn extent(lines: &[String], start: usize, parent: usize) -> (usize, usize) {
    let end = lines[start + 1..]
        .iter()
        .position(|l| is_content(l) && indent_of(l) <= parent)
        .map_or(lines.len(), |i| start + 1 + i);
    let child = lines[start + 1..end]
        .iter()
        .find(|l| is_content(l))
        .map_or(parent + 2, |l| indent_of(l));
    (end, child)
}

/// Rewrite the direct child `key:` line of the block `start..end`, or insert
/// one right after the header. A block value under the old line (a `- x`
/// list, possibly at the key's own indent, or a nested mapping) is dropped
/// with it; comments and blank lines after its last entry stay.
fn set_key(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    child: usize,
    key: &str,
    value: &str,
) {
    let prefix = format!("{key}:");
    let hit = lines[start + 1..end].iter().position(|l| {
        indent_of(l) == child
            && l.trim_start()
                .strip_prefix(&prefix)
                .is_some_and(|r| r.is_empty() || r.starts_with(char::is_whitespace))
    });
    match hit {
        Some(i) => {
            let at = start + 1 + i;
            let line = &lines[at];
            let rest = &line[child + prefix.len()..];
            let block = rest.trim_start().is_empty() || rest.trim_start().starts_with('#');
            let comment = rest.find(" #").map(|c| &rest[c..]).unwrap_or("");
            let new = format!("{}{prefix} {value}{comment}", &line[..child]);
            lines[at] = new;
            if block {
                let entries = lines[at + 1..end]
                    .iter()
                    .take_while(|l| {
                        let t = l.trim_start();
                        !is_content(l)
                            || indent_of(l) > child
                            || (indent_of(l) == child && (t == "-" || t.starts_with("- ")))
                    })
                    .enumerate()
                    .filter(|(_, l)| is_content(l))
                    .last()
                    .map_or(0, |(n, _)| n + 1);
                lines.drain(at + 1..at + 1 + entries);
            }
        }
        None => lines.insert(start + 1, format!("{}{prefix} {value}", " ".repeat(child))),
    }
}

/// Turn an inline header value into block form: `name: {a: 1, b: 2} # c`
/// becomes `name: # c` followed by one entry per line; `null`/`~`/`{}` become
/// a bare `name:`. Block headers are left alone; anything else is an error.
fn expand_header(lines: &mut Vec<String>, at: usize, name: &str) -> std::io::Result<()> {
    let line = &lines[at];
    let indent = indent_of(line);
    let rest = &line[indent + name.len() + 1..];
    let value = rest.trim_start();
    if value.is_empty() || value.starts_with('#') {
        return Ok(());
    }
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "config.yaml: `{name}:` holds an inline value this writer cannot edit: {value}"
            ),
        )
    };
    let (entries, tail) = if value.starts_with('{') {
        let (body, tail) = split_flow(value).ok_or_else(invalid)?;
        if !(tail.is_empty() || tail.trim_start().starts_with('#')) {
            return Err(invalid());
        }
        let entries: Vec<String> = flow_entries(body)
            .into_iter()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(|e| format!("{}  {e}", " ".repeat(indent)))
            .collect();
        if entries
            .iter()
            .any(|e| !e.contains(": ") && !e.ends_with(':'))
        {
            return Err(invalid());
        }
        (entries, tail)
    } else {
        let (scalar, tail) = value.split_at(value.find(char::is_whitespace).unwrap_or(value.len()));
        if !matches!(scalar, "null" | "~")
            || !(tail.is_empty() || tail.trim_start().starts_with('#'))
        {
            return Err(invalid());
        }
        (Vec::new(), tail)
    };
    lines[at] = format!("{}{name}:{tail}", " ".repeat(indent));
    lines.splice(at + 1..at + 1, entries);
    Ok(())
}

/// Split `{…}…` into the mapping body and what follows the matching brace.
/// `None` when the brace never closes on this line (multi-line or unbalanced)
/// or a quote is left open.
fn split_flow(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if c == '\\' => escaped = true,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '{' | '[' => depth += 1,
                '}' | ']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return (c == '}').then(|| (&s[1..i], &s[i + 1..]));
                    }
                }
                _ => {}
            },
        }
    }
    None
}

/// Top-level comma split of a flow mapping body (brackets and quotes
/// balanced by [`split_flow`] already).
fn flow_entries(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut escaped, mut from) = (0usize, None::<char>, false, 0);
    for (i, c) in body.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if c == '\\' => escaped = true,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(&body[from..i]);
                    from = i + 1;
                }
                _ => {}
            },
        }
    }
    out.push(&body[from..]);
    out
}

/// Write `lines` unless they stopped parsing as YAML while `old` still did.
fn commit(path: &Path, old: &str, lines: &[String]) -> std::io::Result<()> {
    use figment::{Provider as _, providers::Format as _};
    let parses = |s: &str| {
        figment::Figment::new()
            .merge(figment::providers::Yaml::string(s))
            .data()
            .is_ok()
    };
    let new = lines.join("\n") + "\n";
    if parses(old) && !parses(&new) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "config.yaml: refusing to write {} — the edit would break the YAML",
                path.display()
            ),
        ));
    }
    std::fs::write(path, new)
}

/// Mailbox every bus subscriber must use (design doc §3: Guaranteed +
/// unbounded = ordered, lossless, non-blocking).
pub fn subscriber_mailbox<A: kameo::Actor>()
-> (mailbox::MailboxSender<A>, mailbox::MailboxReceiver<A>) {
    mailbox::unbounded()
}

/// One per crate. `start` spawns actors, subscribes them, registers named
/// actors, and returns; it must not block on other plugins.
#[async_trait::async_trait]
pub trait Plugin: Send {
    /// Stable name used in logs and the kameo registry prefix.
    fn name(&self) -> &'static str;
    /// Declare inert contracts before any plugin starts; must not open services.
    async fn declare_wire(&mut self, _rt: &Runtime) -> anyhow::Result<()> { Ok(()) }
    /// Wire this plugin into the runtime.
    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()>;
}

/// Identifies a conversation; every event carries one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct SessionId(pub uuid::Uuid);

impl SessionId {
    /// Time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for SessionId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// One model round trip within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct TurnId(pub uuid::Uuid);

impl TurnId {
    /// Time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TurnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One user input, identified by its sender so it can be shown before any
/// reply arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct InputId(pub uuid::Uuid);

impl InputId {
    /// Time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for InputId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for InputId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for InputId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::{write_config_value, write_nested_config_value};
    use figment::providers::Format as _;
    use std::path::{Path, PathBuf};

    #[tokio::test]
    async fn extension_initializers_can_access_another_slot() {
        let rt = super::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let (tx, rx) = std::sync::mpsc::channel();
        let cloned = rt.clone();
        let thread = std::thread::spawn(move || {
            tx.send(cloned.extension(|| (cloned.extension(|| 41_u32), 1_u8))).unwrap();
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), (41, 1));
        thread.join().unwrap();
        assert_eq!(rt.extension(|| 99_u32), 41);
        assert_eq!(rt.extension::<(u32, u8)>(|| panic!("initialized twice")), (41, 1));
    }

    #[tokio::test]
    async fn lookup_when_returns_none_after_plugins_started() {
        let rt = super::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let cloned = rt.clone();
        let waiting = tokio::spawn(async move {
            cloned
                .lookup_when::<super::MessageBus>("runtime-test-absent-actor")
                .await
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        rt.mark_plugins_started();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("lookup_when should wake promptly after mark_plugins_started")
            .unwrap();
        assert!(got.is_none());

        // Calling lookup_when after mark_plugins_started also returns None immediately.
        let again = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rt.lookup_when::<super::MessageBus>("runtime-test-absent-actor"),
        )
        .await
        .expect("lookup_when should not block once plugins are started");
        assert!(again.is_none());
    }

    #[tokio::test]
    async fn lookup_when_finds_registered_actor_even_after_plugins_started() {
        let rt = super::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        rt.bus
            .register("runtime-test-present-actor")
            .expect("register test bus");
        rt.mark_plugins_started();
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rt.lookup_when::<super::MessageBus>("runtime-test-present-actor"),
        )
        .await
        .expect("lookup_when should resolve immediately");
        assert!(got.is_some());
    }

    #[tokio::test]
    async fn lookup_when_keeps_polling_until_shutdown_when_unmarked() {
        let rt = super::Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let cloned = rt.clone();
        let waiting = tokio::spawn(async move {
            cloned
                .lookup_when::<super::MessageBus>("runtime-test-unmarked-actor")
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(!waiting.is_finished());
        rt.shutdown.cancel();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("lookup_when should exit on shutdown")
            .unwrap();
        assert!(got.is_none());
    }

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("theuth-wb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.yaml")
    }

    /// Read `key` back the way the engine does: through figment's YAML provider.
    fn get<T: serde::de::DeserializeOwned>(path: &Path, key: &str) -> T {
        figment::Figment::new()
            .merge(figment::providers::Yaml::file(path))
            .extract_inner(key)
            .unwrap_or_else(|e| panic!("{key}: {e}"))
    }

    fn text(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    const LIVE: &str = "agent:
  favorite_models: [\"a/b\", \"c/d\"]
  vision_model: openai/gpt-6.1-sol
  max_tool_rounds: 384
  subagents:
    model: anthropic/claude-opus-5-5
    max_runtime_secs: 28800
  communication: true
  compact_style: snap
  title_model: anthropic/claude-haiku-5-5
  effort: \"max\"
  model: openai/gpt-5.6-sol
  # subagents:
  #   model: null              # default: agent.model
  #   max_depth: 1
";

    #[test]
    fn t1_direct_child_not_nested() {
        let path = tmp();
        std::fs::write(&path, LIVE).unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(
            text(&path),
            LIVE.replace("  model: openai/gpt-5.6-sol", "  model: x/new")
        );
        assert_eq!(get::<String>(&path, "agent.model"), "x/new");
        assert_eq!(
            get::<String>(&path, "agent.subagents.model"),
            "anthropic/claude-opus-5-5"
        );
    }

    #[test]
    fn t2_only_nested_key_present_inserts_direct() {
        let path = tmp();
        std::fs::write(
            &path,
            "agent:\n  subagents:\n    model: z/z\n  effort: max\n",
        )
        .unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(
            text(&path),
            "agent:\n  model: x/new\n  subagents:\n    model: z/z\n  effort: max\n"
        );
        assert_eq!(get::<String>(&path, "agent.subagents.model"), "z/z");
    }

    #[test]
    fn t3_comments_never_match_and_tail_kept() {
        let path = tmp();
        let before = "agent:\n  # model: a\n  #   model: null\n  model: old # inline\n";
        std::fs::write(&path, before).unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(
            text(&path),
            before.replace("model: old # inline", "model: x/new # inline")
        );
        assert_eq!(get::<String>(&path, "agent.model"), "x/new");

        std::fs::write(&path, "agent:\n  # model: a\n  effort: max\n").unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(
            text(&path),
            "agent:\n  model: x/new\n  # model: a\n  effort: max\n"
        );
    }

    #[test]
    fn t4_four_space_indent() {
        let path = tmp();
        let before =
            "agent:\n    subagents:\n        model: deep\n    model: old\nlog:\n    level: info\n";
        std::fs::write(&path, before).unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(
            text(&path),
            before.replace("    model: old", "    model: x/new")
        );
        assert_eq!(get::<String>(&path, "agent.subagents.model"), "deep");

        std::fs::write(&path, "agent:\n    effort: max\n").unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(text(&path), "agent:\n    model: x/new\n    effort: max\n");
    }

    #[test]
    fn t5_nested_direct_child_not_deeper() {
        let path = tmp();
        let before =
            "agent:\n  subagents:\n    pinned:\n      model: q/q\n    model: old\n  effort: max\n";
        std::fs::write(&path, before).unwrap();
        write_nested_config_value(&path, "agent", "subagents", "model", "n/n").unwrap();
        assert_eq!(
            text(&path),
            before.replace("    model: old", "    model: n/n")
        );
        assert_eq!(get::<String>(&path, "agent.subagents.pinned.model"), "q/q");
        assert_eq!(get::<String>(&path, "agent.subagents.model"), "n/n");
    }

    #[test]
    fn t6_nested_flow_subsection_becomes_block() {
        let path = tmp();
        std::fs::write(
            &path,
            "agent:\n  effort: max\n  subagents: {model: null, max_depth: 1} # c\n  compact_style: snap\n",
        )
        .unwrap();
        write_nested_config_value(&path, "agent", "subagents", "model", "X").unwrap();
        assert_eq!(
            text(&path),
            "agent:\n  effort: max\n  subagents: # c\n    model: X\n    max_depth: 1\n  compact_style: snap\n"
        );
        assert_eq!(get::<String>(&path, "agent.subagents.model"), "X");
        assert_eq!(get::<u32>(&path, "agent.subagents.max_depth"), 1);
    }

    #[test]
    fn t7_flow_section_becomes_block() {
        let path = tmp();
        std::fs::write(
            &path,
            "agent: {model: a, communication: true}\nlog:\n  level: info\n",
        )
        .unwrap();
        write_config_value(&path, "agent", "model", "b").unwrap();
        let after = text(&path);
        assert_eq!(
            after.matches("\nagent:").count() + usize::from(after.starts_with("agent:")),
            1
        );
        assert_eq!(
            after,
            "agent:\n  model: b\n  communication: true\nlog:\n  level: info\n"
        );
        assert_eq!(get::<String>(&path, "agent.model"), "b");
        assert!(get::<bool>(&path, "agent.communication"));
    }

    #[test]
    fn t8_unbalanced_flow_is_invalid_data() {
        let path = tmp();
        let before = "agent: {model: [a, b}\n";
        std::fs::write(&path, before).unwrap();
        let err = write_config_value(&path, "agent", "model", "b").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(text(&path), before);
    }

    #[test]
    fn t9_header_with_trailing_comment() {
        let path = tmp();
        let before = "agent:   # main\n  model: old\nlog:\n  level: info\n";
        std::fs::write(&path, before).unwrap();
        write_config_value(&path, "agent", "model", "x/new").unwrap();
        assert_eq!(text(&path), before.replace("model: old", "model: x/new"));
        assert_eq!(get::<String>(&path, "agent.model"), "x/new");
    }

    #[test]
    fn t10_safety_net_refuses_breaking_edit() {
        let path = tmp();
        let before = "agent:\n  model: old\n";
        std::fs::write(&path, before).unwrap();
        let err = write_config_value(&path, "agent", "model", "\"unterminated").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(text(&path), before);
    }

    /// 206 (B-9): a key holding a block value (a `- x` list at the key's indent or deeper,
    /// a nested mapping) is replaced whole; comments after it and siblings stay.
    #[test]
    fn t11_block_value_is_replaced_whole() {
        let path = tmp();
        std::fs::write(
            &path,
            "tools:\n  builtins:\n    disabled: # c\n    - a\n    # inner\n    - b\n    # after\n    other: 1\n  approvals: true\n",
        )
        .unwrap();
        write_nested_config_value(&path, "tools", "builtins", "disabled", "[\"x\"]").unwrap();
        assert_eq!(
            text(&path),
            "tools:\n  builtins:\n    disabled: [\"x\"] # c\n    # after\n    other: 1\n  approvals: true\n"
        );
        assert_eq!(get::<Vec<String>>(&path, "tools.builtins.disabled"), ["x"]);

        std::fs::write(
            &path,
            "agent:\n  model:\n      - a\n      - b\n  effort: max\n",
        )
        .unwrap();
        write_config_value(&path, "agent", "model", "m").unwrap();
        assert_eq!(text(&path), "agent:\n  model: m\n  effort: max\n");

        std::fs::write(&path, "agent:\n  model:\n  effort: max\n").unwrap();
        write_config_value(&path, "agent", "model", "m").unwrap();
        assert_eq!(text(&path), "agent:\n  model: m\n  effort: max\n");
    }

    #[test]
    fn write_back_edits_in_place_or_appends() {
        let dir = std::env::temp_dir().join(format!("theuth-wb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");

        let before = "# header\nrpc:\n  model: not-this\nagent:\n  # keep me\n  model: old # inline\n  # subagents:\n  #   model: null\nlog:\n  level: info\n";
        std::fs::write(&path, before).unwrap();
        write_config_value(&path, "agent", "model", "anthropic/claude-haiku-4-5").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before.replace(
                "model: old # inline",
                "model: anthropic/claude-haiku-4-5 # inline"
            )
        );

        std::fs::write(&path, "log:\n  level: info").unwrap();
        write_config_value(&path, "tools", "approvals", "false").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "log:\n  level: info\n\ntools:\n  approvals: false\n"
        );

        std::fs::write(&path, "agent:\n  max_tool_rounds: 4\n").unwrap();
        write_config_value(&path, "agent", "model", "x/y").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "agent:\n  model: x/y\n  max_tool_rounds: 4\n"
        );

        std::fs::write(
            &path,
            "agent:\n  model: x/y\n  subagents:\n    model: z/y\n    max_runtime_secs: 600 # keep\n  compact_style: cut\n",
        )
        .unwrap();
        write_nested_config_value(&path, "agent", "subagents", "max_runtime_secs", "900").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "agent:\n  model: x/y\n  subagents:\n    model: z/y\n    max_runtime_secs: 900 # keep\n  compact_style: cut\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
