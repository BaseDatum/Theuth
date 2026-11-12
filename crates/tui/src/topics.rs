//! Single-source transport and the shrinking adapter for unmigrated panels.
//! Never republish decoded remote events on the runtime bus.
use std::sync::Arc;
use jsonrpsee::{core::client::Error, ws_client::WsClient};
use rpc::api::clients::CoreApiClient;
use runtime::Runtime;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use crate::model::UiEvent;

/// The original borrowed-access frame; remote and legacy data get no fake descriptor.
#[derive(Clone, Debug)]
pub enum TopicEvent {
    /// Shared in-process frame, never reserialized by the intake.
    Local(Arc<wire::WireFrame>),
    /// A server's explicit topic/version envelope.
    Remote(Arc<TopicNotification>),
    /// Old-engine bare JSON, without an invented topic or version.
    Legacy(Arc<Value>),
}

/// New-engine transport notification; its catalog need not match the local runtime.
#[derive(Debug, Deserialize)]
pub struct TopicNotification {
    /// Owner-qualified topic as sent by the server.
    pub topic: String,
    /// Inner contract version, independent of the engine version.
    pub version: u32,
    /// Original event body including forward-compatible fields.
    pub event: Value,
}

impl TopicEvent {
    /// None only for opaque legacy ingress.
    pub fn topic(&self) -> Option<&str> {
        match self {
            Self::Local(f) => f.descriptor.as_ref().map(|d| d.topic),
            Self::Remote(f) => Some(&f.topic),
            Self::Legacy(_) => None,
        }
    }
    /// None means unversioned legacy data, not version one.
    pub fn version(&self) -> Option<u32> {
        match self {
            Self::Local(f) => f.descriptor.as_ref().map(|d| d.version),
            Self::Remote(f) => Some(f.version),
            Self::Legacy(_) => None,
        }
    }
    /// Borrow the inner JSON without cloning.
    pub fn event(&self) -> &Value {
        match self {
            Self::Local(f) => &f.event,
            Self::Remote(f) => &f.event,
            Self::Legacy(v) => v,
        }
    }
    /// Connection-specific local control with the declared rpc.stream contract.
    pub fn control(event: Value) -> Self {
        Self::Remote(Arc::new(TopicNotification { topic: "rpc.stream".into(), version: 1, event }))
    }
    /// Only supported stream controls participate in snapshot/loss handling.
    pub fn is_control(&self) -> bool {
        (self.topic() == Some("rpc.stream") && self.version() == Some(1))
            || (self.topic().is_none() && matches!(self.event()["type"].as_str(), Some("PendingPrompts" | "Lagged")))
    }
}

/// Forward one trusted local stream, releasing its ring cursor on shutdown.
pub async fn local(rt: Runtime, mut stream: rpc::topics::LocalStream, tx: UnboundedSender<UiEvent>) {
    loop {
        tokio::select! {
            _ = rt.shutdown.cancelled() => return,
            _ = tx.closed() => return,
            item = stream.next() => {
                let event = match item {
                    Some(rpc::topics::LocalEvent::Frame(frame)) => TopicEvent::Local(frame),
                    Some(rpc::topics::LocalEvent::Control(value)) => TopicEvent::control(value),
                    None => return,
                };
                if tx.send(UiEvent::Wire(event)).is_err() { return; }
            }
        }
    }
}

/// Negotiate one attached stream; only -32601 selects legacy mode.
pub async fn remote(rt: &Runtime, client: &WsClient, tx: &UnboundedSender<UiEvent>) -> anyhow::Result<()> {
    // null keeps the existing broad session/tree scope; the registry filters locally.
    let (mut stream, legacy) = match client.subscribe_topics(None, None, None).await {
        Ok(stream) => (stream, false),
        Err(Error::Call(error)) if error.code() == -32601 =>
            (client.subscribe_events(None, None).await?, true),
        Err(error) => return Err(error.into()),
    };
    loop {
        tokio::select! {
            _ = rt.shutdown.cancelled() => return Ok(()),
            _ = tx.closed() => return Ok(()),
            value = stream.next() => {
                let Some(value) = value else { return Ok(()) };
                let value = value?;
                let event = if legacy {
                    TopicEvent::Legacy(Arc::new(value))
                } else {
                    match serde_json::from_value(value) {
                        Ok(frame) => TopicEvent::Remote(Arc::new(frame)),
                        Err(error) => {
                            tracing::warn!(%error, "invalid topic notification");
                            continue;
                        }
                    }
                };
                if tx.send(UiEvent::Wire(event)).is_err() { return Ok(()); }
            }
        }
    }
}

/// Remove a row here when its responsibility moves to a component. agent.events
/// is decoded once and borrowed by components before the core reducer consumes it.
pub fn residual(event: &TopicEvent) -> Option<UiEvent> {
    let value = event.event();
    if event.topic().is_some() && event.version() != Some(1) { return None; }
    macro_rules! decode {
        ($ty:ty, $variant:ident) => { <$ty>::deserialize(value).ok().map(UiEvent::$variant) };
    }
    match event.topic() {
        Some("agent.events") => decode!(agent::AgentEvent, Agent),
        Some("tools.approval-requested") => decode!(tools::ApprovalRequested, Approval),
        Some("tools.approval-resolved") => decode!(tools::ApprovalResolved, Resolved),
        Some("tools.ask-requested") => decode!(tools::AskRequested, Ask),
        Some("tools.ask-resolved") => decode!(tools::AskResolved, AskResolved),
        Some("double.predicted") => decode!(double::DoublePredicted, DoublePredicted),
        Some("double.answered") => decode!(double::DoubleAnswered, DoubleAnswered),
        Some("tools.mcp-progress") => decode!(tools::McpProgress, McpProgress),
        Some("agent.login") => decode!(agent::LoginFinished, LoginFinished),
        Some("link.events") => decode!(link::LinkEvent, LinkEvent),
        Some("nodes.events") => decode!(nodes::NodeEvent, NodeEvent),
        Some("artifact.shown") => decode!(artifact::ArtifactShown, ArtifactShown),
        Some("secrets.events") => decode!(secrets::SecretsEvent, SecretsEvent),
        Some("knowledge.changed") => decode!(knowledge::KnowledgeChanged, KnowledgeChanged),
        Some("web.scrape-progress") => decode!(web::progress::ScrapeProgress, ScrapeProgress),
        Some("agent.communication") => decode!(agent::CommunicationChanged, CommunicationChanged),
        Some("agent.peer-message") => decode!(agent::PeerMessageChanged, PeerMessageChanged),
        Some("agent.peer-delivered") => decode!(agent::PeerDelivered, PeerDelivered),
        Some("a2a.changed") => decode!(a2a::A2aChanged, A2aChanged),
        Some("voice.events") => decode!(voice::VoiceEvent, Voice),
        Some("talk.events") => decode!(talk::TalkEvent, Talk),
        None => legacy(value, matches!(event, TopicEvent::Local(_))),
        _ => None,
    }
}

fn legacy(value: &Value, _local: bool) -> Option<UiEvent> {
    // Struct serde decoders do not validate their tag. Explicit tags avoid
    // swallowing unknown events (in particular the empty A2aChanged struct).
    macro_rules! tagged {
        ($($ty:ty => $tag:literal, $variant:ident),* $(,)?) => { match value["type"].as_str()? {
            $($tag => return <$ty>::deserialize(value).ok().map(UiEvent::$variant),)*
            _ => {}
        }};
    }
    tagged!(
        tools::ApprovalRequested => "ApprovalRequested", Approval,
        tools::ApprovalResolved => "ApprovalResolved", Resolved,
        tools::AskRequested => "AskRequested", Ask,
        tools::AskResolved => "AskResolved", AskResolved,
        double::DoublePredicted => "DoublePredicted", DoublePredicted,
        double::DoubleAnswered => "DoubleAnswered", DoubleAnswered,
        tools::McpProgress => "McpProgress", McpProgress,
        agent::LoginFinished => "LoginFinished", LoginFinished,
        artifact::ArtifactShown => "ArtifactShown", ArtifactShown,
        knowledge::KnowledgeChanged => "KnowledgeChanged", KnowledgeChanged,
        web::progress::ScrapeProgress => "ScrapeProgress", ScrapeProgress,
        agent::CommunicationChanged => "CommunicationChanged", CommunicationChanged,
        agent::PeerMessageChanged => "PeerMessageChanged", PeerMessageChanged,
        agent::PeerDelivered => "PeerDelivered", PeerDelivered,
        a2a::A2aChanged => "A2aChanged", A2aChanged,
    );
    macro_rules! owner {
        ($ty:ty, $variant:ident) => {
            if let Ok(event) = <$ty>::deserialize(value) { return Some(UiEvent::$variant(event)); }
        };
    }
    owner!(agent::AgentEvent, Agent);
    owner!(link::LinkEvent, LinkEvent);
    owner!(nodes::NodeEvent, NodeEvent);
    owner!(secrets::SecretsEvent, SecretsEvent);
    owner!(voice::VoiceEvent, Voice);
    owner!(talk::TalkEvent, Talk);
    None
}

#[cfg(test)]
mod tests;
