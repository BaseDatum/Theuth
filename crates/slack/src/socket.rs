//! Socket Mode listener wiring (design doc 21 §3.1): morphism's listener,
//! `Arc<Channel>` in the user state, dedup on `event_id` / `trigger_id`,
//! reconnect with backoff.

use std::sync::Arc;

use chat_common::backoff_delay;
use slack_morphism::errors::{SlackClientError, SlackClientSystemError};
use slack_morphism::prelude::*;
use tokio_util::sync::CancellationToken;

use crate::SlackEvent;
use crate::channel::{Channel, Inbound};

type Connector = SlackClientHyperHttpsConnector;

/// `Arc<Channel>` back out of morphism's user state.
async fn chan(states: &SlackClientEventsUserState) -> Option<Arc<Channel>> {
    states
        .read()
        .await
        .get_user_state::<Arc<Channel>>()
        .cloned()
}

async fn on_push(
    ev: SlackPushEventCallback,
    _client: Arc<SlackClient<Connector>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<()> {
    let Some(chan) = chan(&states).await else {
        return Ok(());
    };
    if chan.recent_seen(&ev.event_id.0) {
        tracing::debug!(event_id = %ev.event_id.0, "duplicate slack event dropped");
        return Ok(());
    }
    chan.inbound(Inbound::Push(Box::new(ev)));
    Ok(())
}

async fn on_interaction(
    ev: SlackInteractionEvent,
    _client: Arc<SlackClient<Connector>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<()> {
    let Some(chan) = chan(&states).await else {
        return Ok(());
    };
    let trigger = match &ev {
        SlackInteractionEvent::BlockActions(e) => Some(e.trigger_id.0.clone()),
        SlackInteractionEvent::MessageAction(e) => Some(e.trigger_id.0.clone()),
        SlackInteractionEvent::Shortcut(e) => Some(e.trigger_id.0.clone()),
        _ => None,
    };
    if let Some(t) = trigger
        && chan.recent_seen(&t)
    {
        tracing::debug!(trigger_id = %t, "duplicate slack interaction dropped");
        return Ok(());
    }
    chan.inbound(Inbound::Action(Box::new(ev)));
    Ok(())
}

async fn on_command(
    ev: SlackCommandEvent,
    _client: Arc<SlackClient<Connector>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<SlackCommandEventResponse> {
    let Some(chan) = chan(&states).await else {
        return Err("slack channel gone".into());
    };
    Ok(chan.command(ev).await)
}

/// `hello`: the socket is up. Generic because morphism does not re-export
/// `SlackSocketModeHelloEvent` (its `models::socket_mode` is shadowed by the
/// listener module of the same name); the type is inferred at registration
/// and `num_connections` read through serde.
async fn on_hello<E: serde::Serialize>(
    ev: E,
    _client: Arc<SlackClient<Connector>>,
    states: SlackClientEventsUserState,
) {
    let connections = serde_json::to_value(&ev)
        .ok()
        .and_then(|v| v.get("num_connections")?.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1);
    if let Some(chan) = chan(&states).await {
        chan.connected(connections).await;
    }
}

fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackClient<Connector>>,
    states: SlackClientEventsUserState,
) -> HttpStatusCode {
    tracing::warn!(error = %err, "slack listener error");
    // Sync handler: `try_read` is fine, the lock is only written once at setup.
    if let Ok(guard) = states.try_read()
        && let Some(chan) = guard.get_user_state::<Arc<Channel>>()
    {
        chan.protocol_error();
    }
    HttpStatusCode::BAD_REQUEST
}

/// One listener lifetime: connect, serve until `shutdown`, tear down.
async fn serve(
    chan: Arc<Channel>,
    app_token: &str,
    shutdown: &CancellationToken,
) -> ClientResult<()> {
    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new().map_err(
        |e| SlackClientError::SystemError(SlackClientSystemError::new().with_cause(Box::new(e))),
    )?));
    let env = Arc::new(
        SlackClientEventsListenerEnvironment::new(client)
            .with_error_handler(on_error)
            .with_user_state(chan),
    );
    let callbacks = SlackSocketModeListenerCallbacks::new()
        .with_hello_events(on_hello)
        .with_push_events(on_push)
        .with_interaction_events(on_interaction)
        .with_command_events(on_command);
    let config = SlackClientSocketModeConfig::new().with_max_connections_count(2);
    let listener = SlackClientSocketModeListener::new(&config, env, callbacks);
    listener
        .listen_for(&SlackApiToken::new(app_token.into()))
        .await?;
    tokio::select! {
        _ = listener.serve() => {},
        () = shutdown.cancelled() => {},
    }
    listener.shutdown().await;
    Ok(())
}

/// Runs the Socket Mode listener until `shutdown`, re-creating it with
/// [`backoff_delay`] when it cannot connect (21 §3.1).
pub async fn run(chan: Arc<Channel>, app_token: String, shutdown: CancellationToken) {
    let mut attempt: u32 = 0;
    while !shutdown.is_cancelled() {
        match serve(Arc::clone(&chan), &app_token, &shutdown).await {
            Ok(()) => return,
            Err(e) => {
                let delay = backoff_delay(attempt);
                attempt = attempt.saturating_add(1);
                tracing::warn!(error = %e, delay_s = delay.as_secs(), "slack socket mode failed; retrying");
                chan.publish(SlackEvent::Disconnected {
                    reason: e.to_string(),
                })
                .await;
                tokio::select! {
                    () = tokio::time::sleep(delay) => {},
                    () = shutdown.cancelled() => return,
                }
            }
        }
    }
}
