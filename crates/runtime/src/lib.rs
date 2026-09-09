//! The extension point. A plugin is a crate exporting one [`Plugin`]; the
//! binary holds a list of them and nothing else. Plugins talk through the
//! kameo message bus (events, type-routed) and the kameo registry (commands,
//! `ask` on a named actor). This crate must stay small (design doc §4).

use std::path::PathBuf;

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
        let _ = self.bus.tell(Register(actor.clone().recipient::<T>())).await;
    }

    /// Extract a config section, e.g. `rt.section::<AgentConfig>("agent")`.
    pub fn section<'de, T: Deserialize<'de>>(&self, name: &str) -> anyhow::Result<T> {
        Ok(self.config.extract_inner(name)?)
    }
}

/// Mailbox every bus subscriber must use (design doc §3: Guaranteed +
/// unbounded = ordered, lossless, non-blocking).
pub fn subscriber_mailbox<A: kameo::Actor>() -> (mailbox::MailboxSender<A>, mailbox::MailboxReceiver<A>) {
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
