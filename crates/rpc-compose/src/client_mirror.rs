use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClient;
use runtime::Runtime;
use crate::api::PendingPrompts;

/// `subscribe_events` on `client`, each JSON event decoded back into its bus
/// type (the same owner-declared serde types, matched by
/// their `type` tag) and published on the local bus. Returns when the
/// subscription ends — the socket closed.
pub async fn mirror(rt: &Runtime, client: &WsClient) -> anyhow::Result<()> {
    let mut events: Subscription<serde_json::Value> = client
        .subscribe(
            "subscribe_events",
            rpc_params![serde_json::Value::Null, serde_json::Value::Null],
            "unsubscribe_events",
        )
        .await?;
    while let Some(ev) = events.next().await {
        match ev {
            Ok(v) => publish(rt, &v).await,
            Err(e) => tracing::warn!(error = %e, "event stream"),
        }
    }
    Ok(())
}

/// Decode `v` as the first bus type it fits (tags are unique across the
/// stream) and publish it; the most frequent types are tried first. The
/// `PendingPrompts` snapshot has no bus type: its items are published one by
/// one, so in-process consumers (the TUI) see the pending cards as
/// `*Requested` events.
async fn publish(rt: &Runtime, v: &serde_json::Value) {
    if let Ok(p) = <PendingPrompts as serde::Deserialize>::deserialize(v) {
        for a in p.approvals {
            rt.publish(a).await;
        }
        for a in p.asks {
            rt.publish(a).await;
        }
        for s in p.secrets {
            rt.publish(s).await;
        }
        return;
    }
    // A tagged struct with no fields accepts *any* object: serde does not check
    // the tag on structs, so `A2aChanged {}` would swallow every event listed
    // after it (voice, talk, meeting, …). Match it by tag alone.
    if v["type"] == "A2aChanged" {
        rt.publish(a2a::A2aChanged {}).await;
        return;
    }
    macro_rules! republish {
        ($($t:ty),* $(,)?) => {$(
            if let Ok(ev) = <$t as serde::Deserialize>::deserialize(v) {
                rt.publish(ev).await;
                return;
            }
        )*};
    }
    republish!(
        agent::AgentEvent,
        tools::JobEvent,
        tools::ReminderEvent,
        tools::McpProgress,
        web::progress::ScrapeProgress,
        tools::ApprovalRequested,
        tools::ApprovalResolved,
        tools::AskRequested,
        tools::AskResolved,
        agent::LoginFinished,
        agent::CommunicationChanged,
        agent::PeerMessageChanged,
        agent::PeerDelivered,
        signal::SignalEvent,
        slack::SlackEvent,
        knowledge::KnowledgeChanged,
        document::DocumentChanged,
        browser::BrowserEvent,
        link::LinkEvent,
        nodes::NodeEvent,
        schedule::ScheduleEvent,
        artifact::ArtifactShown,
        artifact::WebViewOpened,
        secrets::SecretsEvent,
        update::UpdateReady,
        voice::VoiceEvent,
        talk::TalkEvent,
        meeting::MeetingEvent,
        meeting::BriefEvent,
        meeting::CalendarEvent,
        app::AppCallChanged,
        codeindex::CodeindexEvent,
        double::DoublePredicted,
        double::DoubleAnswered,
        roster::attention::AttentionChanged,
        dreaming::DreamNightStarted,
        dreaming::DreamNightFinished,
        dreaming::DreamUndone,
        autoresearch::StudyEvent,
    );
    tracing::debug!(
        tag = v["type"].as_str().unwrap_or("?"),
        "event with no bus type"
    );
}
