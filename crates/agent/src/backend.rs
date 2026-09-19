//! One seam for "which client streams this model" (12 §6, 05 §2.5).
//!
//! Cursor's AgentService is Connect-RPC/protobuf over HTTP/2 and not
//! OpenAI-shaped, so genai has no adapter for it; `crates/cursor` is that
//! adapter and this module is the only place in `agent` that reaches it.

use std::pin::Pin;

use cursor::chat_stream;
use futures::{Stream, StreamExt};
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatRequest, ChatStreamEvent, StopReason, StreamEnd};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ModelSpec, ServiceTarget};
use models::Resolved;
use runtime::SessionId;

use crate::frame::Transport;
use crate::loop_::ns;
use crate::providers::Gateway;
use crate::supervisor::Auth;

/// One model call's events, from genai or the Cursor backend.
pub(crate) type EventStream = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, String>> + Send>>;

/// 47 §3.3: the private stop classification beside the raw provider string.
/// A recognised normal completion (`end_turn`, `stop`, `STOP`, …; genai
/// normalises the provider forms, Cursor emits `end_turn` itself) with no tool
/// calls is a natural yield. A missing or unknown reason, truncation, filtering
/// or a stop sequence is not: those never authorise a same-turn continuation.
pub(crate) fn natural_stop(end: &StreamEnd) -> bool {
    matches!(end.captured_stop_reason, Some(StopReason::Completed(_)))
        && end.captured_tool_calls().is_none_or(|c| c.is_empty())
}

/// Which client streams a model. Chosen when the model is set
/// ([`Backend::for_model`]), never per call.
#[derive(Clone)]
pub(crate) enum Backend {
    /// A genai adapter, or a named OpenAI-compatible gateway (05 §3) looked
    /// up in `auth` per call so `/provider add` is live.
    Genai { client: Client, auth: Auth },
    /// Cursor's AgentService (05 §2.5); the bearer is the login's token.
    Cursor { oauth: oauth::Store },
}

impl Backend {
    /// The `cursor` namespace is the one id genai cannot serve; everything
    /// else, gateways included, goes through the genai client.
    pub(crate) fn for_model(resolved: &Resolved, client: &Client, auth: &Auth) -> Backend {
        if ns(&resolved.genai) == "cursor" {
            Backend::Cursor {
                oauth: auth.oauth.clone(),
            }
        } else {
            Backend::Genai {
                client: client.clone(),
                auth: auth.clone(),
            }
        }
    }

    /// Cache marks for `resolved`'s provider (07 §3): Cursor places none,
    /// gateways are OpenAI-compatible, genai adapters by resolved kind.
    pub(crate) async fn transport(&self, resolved: &Resolved) -> Result<Transport, String> {
        match self {
            Backend::Cursor { .. } => Ok(Transport::Plain),
            Backend::Genai { auth, .. } if gateway(auth, resolved).is_some() => {
                Ok(Transport::OpenAiLike)
            }
            Backend::Genai { client, .. } => {
                let target = client
                    .resolve_service_target(resolved.genai.as_str())
                    .await
                    .map_err(provider_error)?;
                Ok(Transport::of(target.model.adapter_kind))
            }
        }
    }

    /// Open the stream for one model call. `session` is Cursor's conversation id.
    pub(crate) async fn stream(
        &self,
        resolved: &Resolved,
        req: ChatRequest,
        opts: &ChatOptions,
        session: SessionId,
    ) -> Result<EventStream, String> {
        match self {
            Backend::Cursor { oauth: store } => {
                let t = store.access(&oauth::CURSOR).await?;
                chat_stream(&t.access_token, slug(resolved), req, &session.to_string()).await
            }
            Backend::Genai { client, auth } => {
                let spec: ModelSpec = match gateway(auth, resolved) {
                    Some(gw) => ServiceTarget {
                        endpoint: Endpoint::from_owned(gw.base_url),
                        auth: AuthData::Key(gw.key),
                        model: ModelIden::new(AdapterKind::OpenAI, slug(resolved).to_owned()),
                    }
                    .into(),
                    None => resolved.genai.as_str().into(),
                };
                client
                    .exec_chat_stream(spec, req, Some(opts))
                    .await
                    .map(|r| -> EventStream {
                        Box::pin(r.stream.map(|ev| ev.map_err(provider_error)))
                    })
                    .map_err(provider_error)
            }
        }
    }
}

/// genai currently wraps AWS credential failures as unsupported features.
/// Keep the provider detail, but explain the failed authentication and recovery.
fn provider_error(error: genai::Error) -> String {
    match error {
        genai::Error::AdapterNotSupported {
            adapter_kind: AdapterKind::BedrockSigv4,
            feature,
        } if feature.starts_with("AWS credential") => format!(
            "AWS authentication failed: {feature}\nCheck the selected AWS profile and credentials. \
             If it uses SSO, run `aws sso login --profile <profile>` (or `aws sso login` for the default profile), then retry."
        ),
        error => error.to_string(),
    }
}

/// The models a Cursor account may use (`ListModels`; 05 §2.5): Cursor is not
/// in models.dev, so the backend is asked.
pub(crate) async fn cursor_models(token: &str) -> Result<Vec<cursor::Model>, String> {
    cursor::models(token).await
}

/// The gateway `resolved`'s provider names, if any.
fn gateway(auth: &Auth, resolved: &Resolved) -> Option<Gateway> {
    resolved.provider.as_deref().and_then(|p| auth.gateway(p))
}

/// The model id without its `provider/` prefix.
fn slug(resolved: &Resolved) -> &str {
    resolved
        .id
        .split_once('/')
        .map_or(resolved.id.as_str(), |(_, m)| m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_credential_failure_is_actionable_and_not_retried() {
        let detail = "AWS credential resolution failed for profile 'work': token has expired";
        let message = provider_error(genai::Error::AdapterNotSupported {
            adapter_kind: AdapterKind::BedrockSigv4,
            feature: detail.into(),
        });
        assert!(message.contains(detail));
        assert!(message.contains("aws sso login"));
        assert_eq!(
            crate::compact::classify(&message),
            crate::compact::Failure::Deterministic
        );
    }
}
