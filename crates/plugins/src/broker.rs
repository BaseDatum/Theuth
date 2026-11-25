//! Owner: B1 — plugin → host requests that reach other actors: `call` (always in the plugin's
//! own headless session, human-only asks), `input` (`ChannelVia::Plugin`), `ping` (design §6.4,
//! §10.3). Grant and rate checks happen in the actor before these run.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use runtime::SessionId;

use crate::host::PipeError;
use crate::types::CallGrant;

/// The plugin's headless session and the risk cap it was created with. Created on the first
/// brokered call and reused while the grant's `max_risk` stays the same; a changed cap (narrowed
/// or approved wider) opens a fresh session, since a session's cap is fixed at creation.
pub(crate) type SessionSlot = Arc<AsyncMutex<Option<(SessionId, tools::Risk)>>>;

fn agents_missing() -> PipeError {
    PipeError {
        code: -32000,
        message: "the agent supervisor is not running".into(),
        data: None,
    }
}

/// `plugin: <name>` in project `plugins`, no model turn ever; capped at the grant's `max_risk`
/// so the registry refuses anything above it. The held session is reused only while its cap
/// equals `cap` (review R13).
async fn headless(
    slot: &SessionSlot,
    plugin: &str,
    cap: tools::Risk,
) -> Result<SessionId, PipeError> {
    let mut held = slot.lock().await;
    if let Some((s, held_cap)) = *held
        && held_cap == cap
    {
        return Ok(s);
    }
    let agents = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(agents_missing)?;
    let open = agents
        .ask(agent::CreateSession {
            options: agent::CreateSessionOptions {
                project: Some("plugins".into()),
                cap: Some(cap),
                ..Default::default()
            },
            peer: None,
        })
        .await
        .map_err(|e| PipeError {
            code: -32000,
            message: format!("headless session: {e}"),
            data: None,
        })?;
    let _ = agents
        .ask(agent::RenameSession {
            session: open.session,
            title: format!("plugin: {plugin}"),
        })
        .await;
    *held = Some((open.session, cap));
    Ok(open.session)
}

/// Validate `call` params against the grant: `{tool, args}`, no `session` (v1: -32602).
pub(crate) fn check_call(
    params: &Value,
    grant: Option<&CallGrant>,
) -> Result<(String, Value), PipeError> {
    if params.get("session").is_some_and(|s| !s.is_null()) {
        return Err(PipeError::params(
            "/session",
            "call runs in the plugin's own session; `session` is not allowed",
        ));
    }
    let Some(tool) = params.get("tool").and_then(Value::as_str) else {
        return Err(PipeError::params("/tool", "`tool` is required"));
    };
    let Some(grant) = grant else {
        return Err(PipeError::not_granted("capability", "call"));
    };
    if !grant.tools.iter().any(|t| t == tool) {
        return Err(PipeError::not_granted("tool", tool));
    }
    let args = params.get("args").cloned().unwrap_or_else(|| json!({}));
    Ok((tool.to_owned(), args))
}

/// Run a granted `call` through the registry as a human-only invoke in the headless session.
pub(crate) async fn call(
    slot: SessionSlot,
    plugin: String,
    grant: CallGrant,
    tool: String,
    args: Value,
) -> Result<Value, PipeError> {
    let session = headless(&slot, &plugin, grant.max_risk).await?;
    let reg = kameo::actor::ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| PipeError {
            code: -32000,
            message: "the tool registry is not running".into(),
            data: None,
        })?;
    let call = tools::ToolCall {
        id: format!("plugin-{}", uuid::Uuid::now_v7()),
        name: tool,
        args,
        session,
        quiet: false,
    };
    let mut out = reg
        .ask(tools::InvokeHumanOnly {
            call,
            requester: format!("plugin {plugin}"),
        })
        .await
        .map_err(|e| PipeError {
            code: -32000,
            message: e.to_string(),
            data: None,
        })?;
    tools::secrets::redact_output(&mut out);
    Ok(json!({ "content": out.content, "is_error": out.is_error, "structured": out.structured }))
}

/// Run a granted, rate-admitted `input`: a `SendInput` stamped `via: plugin`.
pub(crate) async fn input(plugin: String, params: Value) -> Result<Value, PipeError> {
    let Some(text) = params
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
    else {
        return Err(PipeError::params("/text", "`text` is required"));
    };
    let session = match params.get("session") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            serde_json::from_value::<SessionId>(v.clone())
                .map_err(|_| PipeError::params("/session", "not a session id"))?,
        ),
    };
    let delivery = match params.get("delivery") {
        None | Some(Value::Null) => agent::Delivery::default(),
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|_| PipeError::params("/delivery", "steer or next_turn"))?,
    };
    let agents = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(agents_missing)?;
    let session = agents
        .ask(agent::SendInput {
            session,
            text: text.to_owned(),
            files: Vec::new(),
            delivery,
            id: None,
            channel: Some(agent::ChannelOrigin {
                via: agent::ChannelVia::Plugin,
                chat: agent::ChannelChat::Dm,
                from: Some(plugin),
                title: None,
            }),
        })
        .await
        .map_err(|e| PipeError {
            code: -32000,
            message: e.to_string(),
            data: None,
        })?;
    Ok(json!({ "session": session }))
}

/// `ping` → `{now_ms}`.
pub(crate) fn ping() -> Value {
    json!({ "now_ms": crate::actor::now_ms() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_checks_session_grant_and_tool() {
        let grant = CallGrant {
            tools: vec!["knowledge_search".into()],
            max_risk: tools::Risk::ReadOnly,
        };
        let e = check_call(
            &json!({"tool": "knowledge_search", "session": "x"}),
            Some(&grant),
        )
        .unwrap_err();
        assert_eq!(e.code, -32602);
        let e = check_call(&json!({"tool": "shell", "args": {}}), Some(&grant)).unwrap_err();
        assert_eq!((e.code, e.data), (-32003, Some(json!({"tool": "shell"}))));
        let e = check_call(&json!({"tool": "shell"}), None).unwrap_err();
        assert_eq!(e.code, -32003);
        let (tool, args) = check_call(
            &json!({"tool": "knowledge_search", "args": {"q": 1}}),
            Some(&grant),
        )
        .unwrap();
        assert_eq!((tool.as_str(), args), ("knowledge_search", json!({"q": 1})));
    }
}
