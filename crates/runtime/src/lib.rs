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
}

impl Runtime {
    /// Build a runtime with a `Guaranteed` bus (design doc §3).
    pub fn new(config: figment::Figment, data_dir: PathBuf) -> Self {
        let bus = kameo::actor::Spawn::spawn(MessageBus::new(DeliveryStrategy::Guaranteed));
        Self {
            bus,
            config,
            data_dir,
            shutdown: CancellationToken::new(),
        }
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

    /// `ActorRef::<A>::lookup(name)` until it resolves or `shutdown` fires (12 §2).
    /// Polls every 50 ms: kameo's registry has no watch API.
    pub async fn lookup_when<A: kameo::actor::Actor>(
        &self,
        name: &'static str,
    ) -> Option<ActorRef<A>> {
        loop {
            if let Ok(Some(r)) = ActorRef::<A>::lookup(name) {
                return Some(r);
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => return None,
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
        }
    }
}

/// Set `section.key: value` in the YAML file at `path`, keeping everything
/// else (comments included) byte-for-byte: the first `key:` line inside the
/// `section:` block is rewritten; a block without it gets the line inserted
/// first; no block → one is appended; a missing file is created.
pub fn write_config_value(
    path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let Some(start) = lines
        .iter()
        .position(|l| l.trim_end() == format!("{section}:"))
    else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend([format!("{section}:"), format!("  {key}: {value}")]);
        return std::fs::write(path, lines.join("\n") + "\n");
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with(|c: char| !c.is_whitespace()))
        .map_or(lines.len(), |i| start + 1 + i);
    let prefix = format!("{key}:");
    let indented =
        |l: &str| l.starts_with(char::is_whitespace) && l.trim_start().starts_with(&prefix);
    match lines[start + 1..end].iter().position(|l| indented(l)) {
        Some(i) => {
            let line = &mut lines[start + 1 + i];
            let indent = line.len() - line.trim_start().len();
            let rest = &line[indent + prefix.len()..];
            let comment = rest.find(" #").map(|c| &rest[c..]).unwrap_or("");
            *line = format!("{}{prefix} {value}{comment}", &line[..indent]);
        }
        None => lines.insert(start + 1, format!("  {key}: {value}")),
    }
    std::fs::write(path, lines.join("\n") + "\n")
}

/// Set `section.subsection.key: value` while preserving the rest of the YAML.
pub fn write_nested_config_value(
    path: &Path,
    section: &str,
    subsection: &str,
    key: &str,
    value: &str,
) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let Some(section_start) = lines
        .iter()
        .position(|l| l.trim_end() == format!("{section}:"))
    else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend([
            format!("{section}:"),
            format!("  {subsection}:"),
            format!("    {key}: {value}"),
        ]);
        return std::fs::write(path, lines.join("\n") + "\n");
    };
    let section_end = lines[section_start + 1..]
        .iter()
        .position(|l| l.starts_with(|c: char| !c.is_whitespace()))
        .map_or(lines.len(), |i| section_start + 1 + i);
    let subsection_line = format!("  {subsection}:");
    let Some(subsection_start) = lines[section_start + 1..section_end]
        .iter()
        .position(|l| l.trim_end() == subsection_line)
        .map(|i| section_start + 1 + i)
    else {
        lines.splice(
            section_start + 1..section_start + 1,
            [subsection_line, format!("    {key}: {value}")],
        );
        return std::fs::write(path, lines.join("\n") + "\n");
    };
    let subsection_end = lines[subsection_start + 1..section_end]
        .iter()
        .position(|l| {
            let trimmed = l.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with('#') && l.len() - trimmed.len() <= 2
        })
        .map_or(section_end, |i| subsection_start + 1 + i);
    let prefix = format!("{key}:");
    match lines[subsection_start + 1..subsection_end]
        .iter()
        .position(|l| l.trim_start().starts_with(&prefix))
    {
        Some(i) => {
            let line = &mut lines[subsection_start + 1 + i];
            let indent = line.len() - line.trim_start().len();
            let rest = &line[indent + prefix.len()..];
            let comment = rest.find(" #").map(|c| &rest[c..]).unwrap_or("");
            *line = format!("{}{prefix} {value}{comment}", &line[..indent]);
        }
        None => lines.insert(subsection_start + 1, format!("    {key}: {value}")),
    }
    std::fs::write(path, lines.join("\n") + "\n")
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

    #[test]
    fn write_back_edits_in_place_or_appends() {
        let dir = std::env::temp_dir().join(format!("wth-wb-{}", uuid::Uuid::new_v4()));
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
