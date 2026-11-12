//! Cross-plugin wire methods, contracts and typed clients.

use std::collections::HashSet;
use std::sync::Arc;
use ::search::SearchCategory;
use wire::RuntimeWire as _;
use agent::rpc_sessions::AgentSessionsApiServer;
use agent::{Agents, PrivateTrees};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use knowledge::Get;
use link::LinkConfig;
use runtime::SessionId;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::time::Instant;
use tools::{GetApprovals, GetPtyIdle, ToolRegistry};
use crate::search::SearchResponse;
use rpc_core::{is_delta, Stream, Backlog, Next, ask, send, peer, ASK_FAILED, Server, PrivateMap};
use roster::rpc_support::{roster_actor, attention_actor, mask_alerts};
use schedule::rpc_support::schedule;
use wire::{WireFrame, EventPolicy};
use app::rpc_support::apps;
use agent::rpc_document::doc_forward;
use document::rpc_support::{doc_send, documents};
use knowledge::rpc_support::knowledge;
use contracts::{admitted, wanted};
use crate::{search, topics};
use agent::rpc_support::RpcServerExt;
use wire::WireEvent as _;
use rpc_core::failed;

mod contracts;
pub use contracts::{Info, PendingPrompts, StreamControl, Lagged, ConfigFlagKey, ConfigFlag};

/// All owner client traits for typed methods on any jsonrpsee client.
pub mod clients;

/// Irreducible cross-plugin RPC operations.
#[rpc_core_macros::rpc_descriptors(owner = "rpc-compose", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait CompositionApi {
    /// Version, data dir, default model, approvals and compaction style in one
    /// call: the client's startup snapshot.
    #[method(name = "info")]
    async fn info(&self) -> RpcResult<Info>;
    /// Unified human search (design doc 49 §4.2) over sessions, artifacts,
    /// current knowledge and knowledge history: one group per selected category
    /// in fixed order, each `ready`/`indexing`/`unavailable`/`error` on its own.
    /// `categories` omitted → all four, `[]` → none; `limit` per category,
    /// default 10, max 50. Blank query → no groups, no cache work.
    /// Clients re-ask only `indexing` groups (`categories: [that]`) every 500 ms.
    #[method(name = "search")]
    async fn search(
        &self,
        query: String,
        categories: Option<Vec<SearchCategory>>,
        limit: Option<u32>,
    ) -> RpcResult<SearchResponse>;
    /// Retry `search`'s failed owners without matching: the selected owners
    /// (omitted → all four) drop failed/stale search build state so the next
    /// `search` rebuilds; knowledge also re-syncs current concepts with disk.
    /// Owners not running are skipped; any failing reset is the error.
    #[method(name = "search_refresh")]
    async fn search_refresh(&self, categories: Option<Vec<SearchCategory>>) -> RpcResult<()>;
    /// The document bound to `binding`. A knowledge binding whose concept
    /// exists but has no document yet gets one created from its file (59
    /// §7.1, on demand), an agent-charter binding from the roster file's body;
    /// `None` only when there is nothing to bind to.
    #[method(name = "doc_resolve", with_extensions)]
    async fn doc_resolve(&self, binding: document::Binding) -> RpcResult<Option<document::DocId>>;
    /// Stream every wire-visible event as `{"type": ..., ...}`, optionally
    /// only those carrying `session`. `descendants: true` (default `false`)
    /// also delivers events of every session in `session`'s subtree, plus
    /// `SubagentSpawned` whose `parent` is in it (19 §2.2). The first item is
    /// always a [`PendingPrompts`] snapshot; a client replaces its pending
    /// prompt set with it. A slow client sees `{"type":"Lagged","count":n}`
    /// where `n` non-delta events were dropped for it (129 §3.3), followed by
    /// a fresh snapshot; delta-class events are merged while they wait and
    /// are never lost that way. Deltas of sessions outside `set_event_focus`
    /// are not delivered.
    #[subscription(name = "subscribe_events", unsubscribe = "unsubscribe_events", item = serde_json::Value, with_extensions)]
    async fn subscribe_events(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult;
    /// Opt-in topic stream. Exact topic strings; null selects all, [] only controls.
    /// Every item is `{topic, version, event}`. `rpc.stream` snapshots and loss
    /// controls are always delivered; legacy `subscribe_events` stays unwrapped.
    #[subscription(name = "subscribe_topics", unsubscribe = "unsubscribe_topics", item = serde_json::Value, with_extensions)]
    async fn subscribe_topics(
        &self,
        topics: Option<Vec<String>>,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult;
    /// 129 §3.2: per-connection focus. Delta-class events (`TextDelta`, `ReasoningDelta`,
    /// `ToolCallProgress`, `VisionProgress`) reach this connection's `subscribe_events` streams
    /// only for `sessions`; every other event is unaffected. A connection that never calls this
    /// receives everything. Replaces the previous set; `[]` means no deltas at all. The set dies
    /// with the connection.
    #[method(name = "set_event_focus", with_extensions)]
    async fn set_event_focus(&self, sessions: Vec<SessionId>) -> RpcResult<()>;
    /// 188: one [`ConfigFlagKey`] (`decision.enabled`, `a2a.enabled`, `computer.enabled`,
    /// `double.enabled`, `web.allow_private_network`, `browser.allow_private_network`) as
    /// config.yaml holds it and as this engine started with it. Node-level, never
    /// forwarded; any other key is `INVALID_PARAMS` naming them.
    #[method(name = "config_flag_get")]
    fn config_flag_get(&self, key: ConfigFlagKey) -> RpcResult<ConfigFlag>;
    /// 188: write only that one key (`<section>.enabled` or
    /// `<section>.allow_private_network`) into config.yaml (every other line,
    /// comments included, untouched). Nothing changes live: the plugin keeps its boot
    /// state until theuth restarts, so the reply's `restart_required` says whether one is due.
    #[method(name = "config_flag_set")]
    fn config_flag_set(&self, key: ConfigFlagKey, on: bool) -> RpcResult<ConfigFlag>;
}

#[async_trait]
impl CompositionApiServer for Server {
    async fn info(&self) -> RpcResult<Info> {
        // 82 §3 / 83 §3: the supervisor's live defaults, not the config file read at startup.
        // 176 E3: one ask, so `info` queues behind at most one supervisor message.
        let agent::InfoDefaults {
            default_model,
            default_effort,
            favorite_models,
            compact_style,
            subagent_timeout_secs,
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            communication,
            advisor,
        } = ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetInfoDefaults).await?;
        let approvals =
            ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session: None }).await?;
        let pty_idle_secs = ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetPtyIdle).await?;
        let name = match self.rt.section::<LinkConfig>("link") {
            Ok(c) if !c.name.is_empty() => c.name,
            _ => platform::process::hostname().to_owned(),
        };
        // 88: the schedule actor's live value; the config file when it is not running.
        let schedule_run_timeout_secs = match schedule() {
            Ok(s) => send(&s, schedule::GetRunTimeout).await?,
            Err(_) => self
                .rt
                .section::<schedule::ScheduleConfig>("schedule")
                .map(|c| c.run_timeout_secs)
                .unwrap_or(1800),
        };
        Ok(Info {
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            data_dir: self.rt.data_dir.display().to_string(),
            default_model,
            default_effort,
            favorite_models,
            approvals,
            compact_style,
            subagent_timeout_secs,
            schedule_run_timeout_secs,
            home_offline_ask_secs: self
                .rt
                .section::<tasks::TasksConfig>("tasks")
                .map(|c| c.home_offline_ask_secs)
                .unwrap_or_else(|_| tasks::TasksConfig::default().home_offline_ask_secs),
            max_tool_rounds,
            stream_idle_secs,
            max_salvages,
            pty_idle_secs,
            name,
            communication,
            app_enabled: apps().is_ok(),
            // 125 §6.5: the supervisor's live advisor defaults (82 §3 rule).
            advisor,
        })
    }

    async fn search(
        &self,
        query: String,
        categories: Option<Vec<SearchCategory>>,
        limit: Option<u32>,
    ) -> RpcResult<SearchResponse> {
        tracing::debug!(%query, ?categories, ?limit, "rpc search");
        search::run(&self.rt, query, categories, limit)
            .await
            .map_err(|e| {
                ErrorObjectOwned::owned(jsonrpsee::types::error::INVALID_PARAMS_CODE, e, None::<()>)
            })
    }

    async fn search_refresh(&self, categories: Option<Vec<SearchCategory>>) -> RpcResult<()> {
        search::refresh(categories)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e, None::<()>))
    }

    async fn doc_resolve(
        &self,
        ext: &Extensions,
        binding: document::Binding,
    ) -> RpcResult<Option<document::DocId>> {
        let docs = documents()?;
        if let Some(id) = doc_send(
            &docs,
            document::Lookup {
                binding: binding.clone(),
            },
        )
        .await?
        {
            return Ok(Some(id));
        }
        let text = match &binding {
            // On demand (59 §7.1): a knowledge concept becomes a document from its current file.
            document::Binding::Knowledge(concept) => match send(
                &knowledge()?,
                Get {
                    id: concept.clone(),
                },
            )
            .await
            {
                Ok(c) => c.text,
                Err(e) if e.code() == ASK_FAILED => return Ok(None),
                Err(e) => return Err(e),
            },
            // An agent's charter becomes a document from the body of `agents/<name>.md`; theuth's is
            // `prompt.md`, read-only, never a document.
            document::Binding::AgentCharter(name) if name == roster::AgentName::DEFAULT => {
                return Ok(None);
            }
            document::Binding::AgentCharter(name) => {
                match send(&roster_actor()?, roster::GetAgent(name.clone())).await {
                    Ok(f) => f.body,
                    Err(e) if e.code() == ASK_FAILED => return Ok(None),
                    Err(e) => return Err(e),
                }
            }
            _ => return Ok(None),
        };
        let r = doc_send(
            &docs,
            document::Ensure {
                binding: binding.clone(),
                markdown: text,
            },
        )
        .await
        .map(Some);
        doc_forward(ext, "doc_resolve", (&binding,), r).await
    }


    async fn subscribe_events(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult {
        subscribe(self, pending, ext, session, descendants, EventSink::Legacy).await
    }

    async fn subscribe_topics(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        topics: Option<Vec<String>>,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> SubscriptionResult {
        subscribe(self, pending, ext, session, descendants, EventSink::Topics(topics)).await
    }

    async fn set_event_focus(&self, ext: &Extensions, sessions: Vec<SessionId>) -> RpcResult<()> {
        let peer = peer(ext)?;
        let set: Arc<HashSet<String>> =
            Arc::new(sessions.iter().map(ToString::to_string).collect());
        // 183 §5.1: a focused session streams from its owner while focused. The
        // relay skips one owned here and follows it if it moves away; a session
        // leaving the focus drops its guard (the stream closes).
        {
            let mut follows = self.follows.lock();
            let held = follows.entry(peer.id()).or_default();
            held.retain(|s, _| sessions.contains(s));
            for s in &sessions {
                if !held.contains_key(s)
                    && let Some(guard) = sessionsync::events::follow(*s)
                {
                    held.insert(*s, guard);
                }
            }
        }
        let fresh = self.focus.write().insert(peer.id(), set.clone()).is_none();
        if fresh {
            self.forget_on_close(peer.clone());
        }
        // 155 §7.5: looking at a session marks its finished alert seen here.
        if !set.is_empty()
            && let Ok(actor) = attention_actor()
            && let Ok(mut items) = actor.ask(roster::attention::ListAttention).await
        {
            let mut seen = self.seen.write();
            let fresh = !seen.contains_key(&peer.id());
            mask_alerts(&mut items, Some(&set), seen.entry(peer.id()).or_default());
            drop(seen);
            if fresh {
                self.forget_on_close(peer);
            }
        }
        Ok(())
    }
    fn config_flag_get(&self, key: ConfigFlagKey) -> RpcResult<ConfigFlag> {
        Ok(self.config_flag(key))
    }

    fn config_flag_set(&self, key: ConfigFlagKey, on: bool) -> RpcResult<ConfigFlag> {
        let path = self.rt.data_dir.join("config.yaml");
        let (section, field) = key.path();
        runtime::write_config_value(&path, section, field, if on { "true" } else { "false" })
            .map_err(|e| failed(format!("config.yaml: {e}")))?;
        Ok(self.config_flag(key))
    }
}

async fn subscribe(
        server: &Server,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: Option<SessionId>,
        descendants: Option<bool>,
        mode: EventSink,
    ) -> SubscriptionResult {
        // Subscribe before the snapshot and before accept so nothing published
        // in between is missed; a child in both the snapshot and the feed is
        // inserted twice, harmlessly.
        let mut rx = server.events.subscribe();
        let external = server.rt.wire_external_stream();
        // Admission first; a refusal is answered as the call's error (the
        // peer gate's `-32002`), not jsonrpsee's anonymous internal error.
        let admit = async {
            let peer = peer(ext)?.id();
            // Private trees opened elsewhere (another process's server, before
            // this map existed) become known here; the filter fails closed on
            // every member for everyone but its owner (51 §4.2).
            if let Ok(trees) = ask::<Agents, _>(agent::AGENT_ACTOR, PrivateTrees).await {
                server.private.write().extend(
                    trees
                        .into_iter()
                        .map(|(member, root, owner)| (member, (root, owner))),
                );
            }
            let tree: Option<HashSet<String>> = match (session, descendants) {
                (Some(root), Some(true)) => {
                    let mut set: HashSet<String> = server
                        .list_agents(ext, root)
                        .await?
                        .into_iter()
                        .map(|a| a.id.to_string())
                        .collect();
                    set.insert(root.to_string());
                    Some(set)
                }
                (Some(root), _) => {
                    // 183 §5.6: a chat just created elsewhere streams through the
                    // follow guard below (by id until its row lands).
                    server.authorize_fwd(ext, root).await?;
                    None
                }
                _ => None,
            };
            RpcResult::Ok((peer, tree))
        };
        let (peer, mut tree) = match admit.await {
            Ok(v) => v,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        // 68 §5: a session owned elsewhere streams its live events here while
        // this subscription lives (the guard drops with the task).
        let follow = session.and_then(sessionsync::events::follow);
        let session = session.map(|s| s.to_string());
        let private = server.private.clone();
        // The snapshot goes first, always (empty too): the client replaces its
        // pending set with it, so a card whose `*Resolved` was missed in a
        // relay gap is dropped and one whose `*Requested` was missed appears.
        let snapshot = mode.snapshot(peer, &private, session.as_deref(), tree.as_mut()).await;
        if let Ok(message) = mode.control(&snapshot)
            && sink.send(message).await.is_err()
        {
            return Ok(());
        }
        // 126 §4.4: the chips and countdowns of pending cards follow the snapshot.
        topics::republish().await;
        // 129 §3.1: the ring is read by one task and the sink written by
        // another, joined by `Stream`: a client that stops reading never makes
        // `rx.recv()` fall behind; what waits merges (deltas) or queues
        // (`BACKLOG`, counted into `Lagged` on overflow).
        let state = Arc::new(parking_lot::Mutex::new(Stream {
            backlog: Backlog::default(),
            tree,
            closed: false,
        }));
        let notify = Arc::new(tokio::sync::Notify::new());
        let focus = server.focus.clone();
        let read = {
            let (state, notify, session, private) = (
                state.clone(),
                notify.clone(),
                session.clone(),
                private.clone(),
            );
            let mode = mode.clone();
            async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            // Enrollment precedes topic selection, including excluded families.
                            let pass = wanted(&ev.policy, session.as_deref(), state.lock().tree.as_mut());
                            let allowed = admitted(&ev.policy, peer, &private);
                            if !pass || !allowed || !mode.selected(&ev) {
                                continue;
                            }
                            if is_delta(&ev) {
                                let focus = focus.read().get(&peer).cloned();
                                if let Some(set) = focus
                                    && !ev.policy.session.as_deref()
                                        .is_some_and(|s| set.contains(s))
                                {
                                    continue;
                                }
                            }
                            let now = Instant::now();
                            state.lock().backlog.push(ev, now);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            state.lock().backlog.lost += n
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            state.lock().closed = true;
                            notify.notify_one();
                            return;
                        }
                    }
                    notify.notify_one();
                }
            }
        };
        let write = async move {
            loop {
                let now = Instant::now();
                let next = state.lock().backlog.next(now);
                match next {
                    Next::Frame(ev) => {
                        if let Ok(message) = mode.frame(&ev)
                            && sink.send(message).await.is_err()
                        {
                            return;
                        }
                    }
                    Next::Lagged(n) => {
                        // Dropped events may include a `*Resolved`: the snapshot after
                        // `Lagged` puts the client's pending set right again.
                        let lagged = topics::lagged(n);
                        if let Ok(message) = mode.control(&lagged)
                            && sink.send(message).await.is_err()
                        {
                            return;
                        }
                        // Do not hold the tree lock across snapshot actor requests.
                        let snapshot = topics::pending(|v| {
                            let policy = EventPolicy::legacy_fields(v);
                            let pass = wanted(&policy, session.as_deref(), state.lock().tree.as_mut());
                            pass && admitted(&policy, peer, &private)
                        }, |topic| mode.topic(topic)).await;
                        if let Ok(message) = mode.control(&snapshot)
                            && sink.send(message).await.is_err()
                        {
                            return;
                        }
                    }
                    Next::Wait(at) => {
                        let closed = state.lock().closed;
                        if closed {
                            return;
                        }
                        tokio::select! {
                            _ = sink.closed() => return,
                            _ = notify.notified() => {}
                            _ = tokio::time::sleep_until(at.unwrap_or(now)), if at.is_some() => {}
                        }
                    }
                }
            }
        };
        tokio::spawn(async move {
            let _follow = follow;
            let _external = external;
            let reader = tokio::spawn(read);
            write.await;
            reader.abort();
        });
        Ok(())
    }

#[derive(Clone)]
enum EventSink {
    Legacy,
    Topics(Option<Vec<String>>),
}

#[derive(Serialize)]
struct TopicEnvelope<'a, T: Serialize> {
    topic: &'a str,
    version: u32,
    event: &'a T,
}

impl EventSink {
    fn topic(&self, topic: &str) -> bool {
        match self {
            Self::Legacy | Self::Topics(None) => true,
            Self::Topics(Some(topics)) => topics.iter().any(|selected| selected == topic),
        }
    }

    fn selected(&self, frame: &WireFrame) -> bool {
        matches!(self, Self::Legacy)
            || frame.descriptor.as_ref().is_some_and(|d| self.topic(d.topic))
    }

    fn message<T: Serialize>(&self, topic: &str, version: u32, event: &T)
        -> serde_json::Result<SubscriptionMessage>
    {
        let raw = match self {
            Self::Legacy => serde_json::value::to_raw_value(event)?,
            Self::Topics(_) => serde_json::value::to_raw_value(&TopicEnvelope { topic, version, event })?,
        };
        Ok(SubscriptionMessage::from(raw))
    }

    fn frame(&self, frame: &WireFrame) -> serde_json::Result<SubscriptionMessage> {
        let (topic, version) = frame.descriptor.as_ref().map_or(("", 0), |d| (d.topic, d.version));
        self.message(topic, version, &frame.event)
    }

    fn control(&self, control: &serde_json::Value) -> serde_json::Result<SubscriptionMessage> {
        self.message(StreamControl::TOPIC, StreamControl::VERSION, control)
    }

    async fn snapshot(
        &self,
        peer: u64,
        private: &PrivateMap,
        session: Option<&str>,
        mut tree: Option<&mut HashSet<String>>,
    ) -> serde_json::Value {
        topics::pending(|v| {
            let policy = EventPolicy::legacy_fields(v);
            wanted(&policy, session, tree.as_deref_mut()) && admitted(&policy, peer, private)
        }, |topic| self.topic(topic)).await
    }
}

pub(super) trait SettingsConfigExt {
    fn config_flag(&self, key: ConfigFlagKey) -> ConfigFlag;
}

impl SettingsConfigExt for Server {
    /// 188: `key` in config.yaml now (read fresh) and in the boot config (never re-read).
    fn config_flag(&self, key: ConfigFlagKey) -> ConfigFlag {
        use figment::providers::{Format, Yaml};
        let file = figment::Figment::from(Yaml::file_exact(self.rt.data_dir.join("config.yaml")));
        let (enabled, running) = (key.read(&file), key.read(&self.rt.config));
        ConfigFlag {
            key,
            enabled,
            running,
            restart_required: enabled != running,
        }
    }

}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
#[cfg(all(test, feature = "ts"))]
pub(crate) use tests::test_rpc_module;
