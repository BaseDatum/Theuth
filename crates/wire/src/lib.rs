//! Plugin-owned wire contracts, registered before their producers start.
//!
//! [`RuntimeWire`] clones share one actor and a live-only, lossy 1024-frame
//! broadcast ring. The actor receives all registered types through the existing
//! Guaranteed bus and serializes them in bus-processing order. Upstream mailboxes
//! remain unbounded; this is not producer backpressure or durable replay.
//! No production consumer or event family is installed by this crate.
//!
//! # Registration
//!
//! Import `wire::RuntimeWire as _`, implement [`WireEvent`] in the event's owner,
//! and await `rt.register_wire_event::<E>()` before starting its producer (even
//! when that service is disabled). Concurrent/repeated declarations are
//! idempotent; topic collisions and source/event mixing fail registration.
//! Installation runs in the hub, so cancellation of the caller cannot leave a
//! partially installed recipient. Runtime's typed `extension::<T>()` accessor
//! owns the private shared handle; wire-specific state stays in this crate.
//!
//! Use `register_wire_event_with` only for owner-specific stateful projection;
//! typed and registered remote inputs share its state. `register_wire_source`
//! adapts transport carriers without publishing their inner data back to the
//! bus. Serialization/projection errors and undeclared ingress topics are
//! logged and dropped without killing fanout. Unknown remote data must instead
//! explicitly use [`WireIngress::Legacy`], which has no invented contract.
//!
//! `wire_schema()` returns a sorted, lazy catalog. Consumers supply the schema
//! generator, choosing its dialect and serialize/deserialize contract and sharing
//! definitions across topics. `wire_receiver_count()` counts all feed receivers,
//! not external RPC streams or rendering capabilities.
//! External RPC stream leases are counted separately for owner liveness hooks;
//! local feed receivers do not imply a renderer or attached frontend.

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock, atomic::{AtomicUsize, Ordering}},
};

use anyhow::{Result, bail};
use parking_lot::RwLock;
use runtime::{
    Runtime,
    kameo::{
        Actor,
        actor::{ActorRef, Spawn},
        message::{Context, Message},
    },
    kameo_actors::message_bus::{MessageBus, Register},
};
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

/// An owner-declared event family; Rust type names never identify wire topics.
pub trait WireEvent: Clone + Send + Serialize + JsonSchema + 'static {
    /// Stable owner-qualified family name.
    const TOPIC: &'static str;
    /// Inner JSON contract version, independent of the engine version.
    const VERSION: u32 = 1;

    /// Serialize once per accepted event, not once per receiver.
    fn to_json(&self) -> serde_json::Result<Value> {
        serde_json::to_value(self)
    }

    /// Extract scope and owner-defined semantics from the projected inner JSON.
    fn policy(json: &Value) -> EventPolicy {
        EventPolicy::legacy_fields(json)
    }
}

/// Downstream coalescing semantics; no event-tag matching lives in the core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delta {
    /// A control frame fences merges within its session.
    #[default]
    Control,
    /// Concatenate text for a matching downstream delta key.
    AppendText,
    /// Keep the latest cumulative progress value for a matching delta key.
    Replace,
}

/// Admission/coalescing metadata for the inner JSON, not a new privacy model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventPolicy {
    /// Top-level session string, without interpreting nested fields or UUIDs.
    pub session: Option<String>,
    /// Top-level parent string.
    pub parent: Option<String>,
    /// Top-level child string.
    pub child: Option<String>,
    /// Whether this frame enrolls a child in the existing subscription tree.
    pub announces_child: bool,
    /// Whether this family bypasses the ordinary wanted/session predicate.
    pub always_wanted: bool,
    /// How a downstream backlog may coalesce the frame.
    pub delta: Delta,
}

impl EventPolicy {
    /// Preserve legacy top-level string extraction; missing/null are absent.
    pub fn legacy_fields(json: &Value) -> Self {
        Self {
            session: json
                .get("session")
                .and_then(Value::as_str)
                .map(str::to_owned),
            parent: json
                .get("parent")
                .and_then(Value::as_str)
                .map(str::to_owned),
            child: json.get("child").and_then(Value::as_str).map(str::to_owned),
            ..Self::default()
        }
    }
}

/// A declared contract, whether or not its owner's service is active.
#[derive(Clone, Debug)]
pub struct EventDescriptor {
    /// Stable family name.
    pub topic: &'static str,
    /// Inner JSON contract version.
    pub version: u32,
    /// Generate lazily with the consumer's dialect, contract and definitions.
    pub schema: fn(&mut schemars::SchemaGenerator) -> Schema,
}

/// Shared feed item. A missing descriptor means legacy-only opaque JSON.
#[derive(Clone, Debug)]
pub struct WireFrame {
    /// Known local contract, or None for unknown old-peer payloads.
    pub descriptor: Option<Arc<EventDescriptor>>,
    /// Unwrapped inner event; legacy clients must receive only this value.
    pub event: Value,
    /// Metadata derived from the projected inner event.
    pub policy: EventPolicy,
}

/// A transport adapter's output, never republished onto the typed bus.
#[derive(Debug)]
pub enum WireIngress {
    /// Route through the declared topic's same stateful projector and policy.
    Registered {
        /// Already-declared local topic; missing topics are programming errors.
        topic: &'static str,
        /// Preserve unknown fields unless the owner explicitly projects them.
        event: Value,
    },
    /// Opaque old-peer data, visible to legacy consumers only.
    Legacy {
        /// Unchanged inner event.
        event: Value,
    },
}

/// Local and relayed input share one owner-owned projector state.
pub enum EventInput<'a, E> {
    /// Borrow the bus event, without an extra erasure clone/allocation.
    Typed(&'a E),
    /// Borrow known-topic raw remote JSON.
    Json(&'a Value),
}

/// Registration and synchronous feed access through the Runtime plugins receive.
pub trait RuntimeWire {
    /// Install a contract and await bus acknowledgement. Exact repeats are no-ops.
    fn register_wire_event<E: WireEvent>(&self) -> impl Future<Output = Result<()>> + Send {
        self.register_wire_event_with::<E, _>(|input| match input {
            EventInput::Typed(event) => event.to_json().map(Some),
            EventInput::Json(event) => Ok(Some(event.clone())),
        })
    }

    /// Install a stateful projector once. Repeated registration never resets it.
    fn register_wire_event_with<E, F>(&self, project: F) -> impl Future<Output = Result<()>> + Send
    where
        E: WireEvent,
        F: for<'a> FnMut(EventInput<'a, E>) -> serde_json::Result<Option<Value>> + Send + 'static;

    /// Install a transport carrier (not a schema event). Unknown declared topics
    /// are logged as programming errors and dropped, never demoted to legacy.
    fn register_wire_source<T, F>(&self, source: F) -> impl Future<Output = Result<()>> + Send
    where
        T: Clone + Send + 'static,
        F: FnMut(&T) -> serde_json::Result<Option<WireIngress>> + Send + 'static;

    /// Subscribe at the current cursor. No startup replay or hidden receiver.
    fn wire_events(&self) -> broadcast::Receiver<Arc<WireFrame>>;
    /// Shared broadcast seam for RPC transport assembly.
    fn wire_feed(&self) -> broadcast::Sender<Arc<WireFrame>>;
    /// Count external RPC subscriptions, excluding local feed consumers.
    fn wire_external_stream_count(&self) -> usize;
    /// Hold one external subscription alive until the returned lease drops.
    fn wire_external_stream(&self) -> ExternalStream;
    /// Count actual feed receivers, not renderers or external RPC streams.
    fn wire_receiver_count(&self) -> usize;
    /// Sorted contract descriptors; does not invoke schema generation.
    fn wire_schema(&self) -> Vec<EventDescriptor>;
}

/// One external RPC subscription. Dropping its task releases owner liveness.
pub struct ExternalStream(Arc<AtomicUsize>);

impl Drop for ExternalStream {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

type Catalog = Arc<RwLock<Vec<EventDescriptor>>>;
type Subscribe =
    fn(ActorRef<MessageBus>, ActorRef<WireHub>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Project = Box<dyn for<'a> FnMut(ErasedInput<'a>) -> serde_json::Result<Option<Value>> + Send>;
type Source = Box<dyn FnMut(&dyn Any) -> serde_json::Result<Option<WireIngress>> + Send>;

enum ErasedInput<'a> {
    Typed(&'a dyn Any),
    Json(&'a Value),
}

struct EventEntry {
    descriptor: Arc<EventDescriptor>,
    project: Project,
    policy: fn(&Value) -> EventPolicy,
}

impl EventEntry {
    fn frame(&mut self, input: ErasedInput<'_>) -> serde_json::Result<Option<WireFrame>> {
        Ok((self.project)(input)?.map(|event| WireFrame {
            policy: (self.policy)(&event),
            descriptor: Some(self.descriptor.clone()),
            event,
        }))
    }
}

enum Entry {
    Event(EventEntry),
    Source(Source),
}

struct Install {
    type_id: TypeId,
    entry: Option<Entry>,
    bus: ActorRef<MessageBus>,
    subscribe: Subscribe,
}

struct State {
    hub: ActorRef<WireHub>,
    feed: broadcast::Sender<Arc<WireFrame>>,
    catalog: Catalog,
    external: Arc<AtomicUsize>,
}

fn state(rt: &Runtime) -> Arc<State> {
    // Extension initializers may race; only the winning slot may spawn the hub.
    rt.extension(|| Arc::new(OnceLock::<Arc<State>>::new()))
        .get_or_init(|| {
            let (feed, _) = broadcast::channel(1024);
            let catalog = Catalog::default();
            let hub = WireHub::spawn_with_mailbox(
                WireHub {
                    entries: HashMap::new(),
                    topics: HashMap::new(),
                    feed: feed.clone(),
                    catalog: catalog.clone(),
                },
                runtime::subscriber_mailbox(),
            );
            Arc::new(State { hub, feed, catalog, external: Arc::default() })
        })
        .clone()
}

fn subscribe<T: Send + 'static>(
    bus: ActorRef<MessageBus>,
    hub: ActorRef<WireHub>,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        bus.ask(Register(hub.recipient::<T>()))
            .await
            .map_err(|err| anyhow::anyhow!("wire bus registration failed: {err}"))
    })
}

impl RuntimeWire for Runtime {
    async fn register_wire_event_with<E, F>(&self, mut project: F) -> Result<()>
    where
        E: WireEvent,
        F: for<'a> FnMut(EventInput<'a, E>) -> serde_json::Result<Option<Value>> + Send + 'static,
    {
        let entry = Entry::Event(EventEntry {
            descriptor: Arc::new(EventDescriptor {
                topic: E::TOPIC,
                version: E::VERSION,
                schema: |generator| generator.subschema_for::<E>(),
            }),
            project: Box::new(move |input| {
                project(match input {
                    ErasedInput::Typed(event) => {
                        EventInput::Typed(event.downcast_ref::<E>().unwrap())
                    }
                    ErasedInput::Json(event) => EventInput::Json(event),
                })
            }),
            policy: E::policy,
        });
        state(self)
            .hub
            .ask(Install {
                type_id: TypeId::of::<E>(),
                entry: Some(entry),
                bus: self.bus.clone(),
                subscribe: subscribe::<E>,
            })
            .await
            .map_err(|err| anyhow::anyhow!("wire registration failed: {err}"))
    }

    async fn register_wire_source<T, F>(&self, mut source: F) -> Result<()>
    where
        T: Clone + Send + 'static,
        F: FnMut(&T) -> serde_json::Result<Option<WireIngress>> + Send + 'static,
    {
        state(self)
            .hub
            .ask(Install {
                type_id: TypeId::of::<T>(),
                entry: Some(Entry::Source(Box::new(move |event| {
                    source(event.downcast_ref::<T>().unwrap())
                }))),
                bus: self.bus.clone(),
                subscribe: subscribe::<T>,
            })
            .await
            .map_err(|err| anyhow::anyhow!("wire source registration failed: {err}"))
    }

    fn wire_events(&self) -> broadcast::Receiver<Arc<WireFrame>> {
        state(self).feed.subscribe()
    }

    fn wire_receiver_count(&self) -> usize {
        state(self).feed.receiver_count()
    }

    fn wire_feed(&self) -> broadcast::Sender<Arc<WireFrame>> {
        state(self).feed.clone()
    }

    fn wire_external_stream_count(&self) -> usize {
        state(self).external.load(Ordering::Relaxed)
    }

    fn wire_external_stream(&self) -> ExternalStream {
        let count = state(self).external.clone();
        count.fetch_add(1, Ordering::Relaxed);
        ExternalStream(count)
    }

    fn wire_schema(&self) -> Vec<EventDescriptor> {
        state(self).catalog.read().clone()
    }
}

#[derive(Actor)]
struct WireHub {
    entries: HashMap<TypeId, Entry>,
    topics: HashMap<&'static str, TypeId>,
    feed: broadcast::Sender<Arc<WireFrame>>,
    catalog: Catalog,
}

impl WireHub {
    async fn install(&mut self, install: &mut Install, actor: ActorRef<Self>) -> Result<()> {
        let entry = install.entry.take().unwrap();
        if let Some(existing) = self.entries.get(&install.type_id) {
            return match (existing, &entry) {
                (Entry::Event(old), Entry::Event(new))
                    if old.descriptor.topic == new.descriptor.topic
                        && old.descriptor.version == new.descriptor.version =>
                {
                    Ok(())
                }
                (Entry::Source(_), Entry::Source(_)) => Ok(()),
                _ => bail!("wire type already registered with a different contract or as a source"),
            };
        }
        if let Entry::Event(event) = &entry
            && self.topics.contains_key(event.descriptor.topic)
        {
            bail!("wire topic collision: {}", event.descriptor.topic);
        }
        // The actor serializes installations, including caller cancellation.
        // No registry/catalog lock is held while awaiting the bus acknowledgement.
        (install.subscribe)(install.bus.clone(), actor).await?;
        if let Entry::Event(event) = &entry {
            self.topics.insert(event.descriptor.topic, install.type_id);
            let mut catalog = self.catalog.write();
            catalog.push((*event.descriptor).clone());
            catalog.sort_unstable_by_key(|descriptor| descriptor.topic);
        }
        self.entries.insert(install.type_id, entry);
        Ok(())
    }

    fn ingress(&mut self, ingress: WireIngress) -> Result<Option<WireFrame>> {
        match ingress {
            WireIngress::Registered { topic, event } => {
                let Some(type_id) = self.topics.get(topic) else {
                    bail!("wire ingress references undeclared topic: {topic}");
                };
                let Entry::Event(entry) = self.entries.get_mut(type_id).unwrap() else {
                    unreachable!()
                };
                Ok(entry.frame(ErasedInput::Json(&event))?)
            }
            WireIngress::Legacy { event } => Ok(Some(WireFrame {
                policy: EventPolicy::legacy_fields(&event),
                descriptor: None,
                event,
            })),
        }
    }

    fn forward<M: Send + 'static>(&mut self, message: &M) -> Result<Option<WireFrame>> {
        match self.entries.get_mut(&TypeId::of::<M>()) {
            Some(Entry::Event(entry)) => Ok(entry.frame(ErasedInput::Typed(message))?),
            Some(Entry::Source(source)) => match source(message)? {
                Some(ingress) => self.ingress(ingress),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }
}

// All typed recipients target this same mailbox. Erasure borrows M; only the
// existing bus clones it. Install is private and cannot be a producer type.
impl<M: Send + 'static> Message<M> for WireHub {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        mut message: M,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(install) = (&mut message as &mut dyn Any).downcast_mut::<Install>() {
            return self.install(install, ctx.actor_ref().clone()).await;
        }
        match self.forward(&message) {
            Ok(Some(frame)) => {
                let _ = self.feed.send(Arc::new(frame));
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "dropping wire event: serialization/projection or ingress contract error")
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
