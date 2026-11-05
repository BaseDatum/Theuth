//! One seam for "which client streams this model" (12 §6, 05 §2.5).
//!
//! Cursor's AgentService is Connect-RPC/protobuf over HTTP/2 and not
//! OpenAI-shaped, so genai has no adapter for it; `crates/cursor` is that
//! adapter and this module is the only place in `agent` that reaches it.

use std::pin::Pin;

use cursor::chat_stream;
use futures::{Stream, StreamExt};
use genai::Headers;
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatRequest, ChatStreamEvent, StopReason, StreamEnd};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ModelSpec, ServiceTarget};
use models::{Protocol, Resolved};
use runtime::SessionId;

use crate::OpenRouterConfig;
use crate::frame::Transport;
use crate::loop_::ns;
use crate::providers::Gateway;
use crate::supervisor::Auth;

/// Zen's one base URL (85 §0.2); the protocol picks the path under it.
pub(crate) const ZEN_ENDPOINT: &str = "https://opencode.ai/zen/v1/";

/// Appended when a user gateway answers Zen's `ModelProtocolUnsupported` (85 §0.8).
const BUILTIN_HINT: &str = "built-in provider — set the `opencode` / `openrouter` secret instead";

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
    /// A genai adapter, a protocol target theuth builds itself (Zen, 85 §0.2),
    /// or a named OpenAI-compatible gateway (05 §3) — the last two looked up
    /// in `auth` per call so `/provider add` and a new key are live.
    Genai { client: Client, auth: Auth },
    /// Cursor's AgentService (05 §2.5); the bearer is the login's token.
    Cursor { oauth: oauth::Store },
}

/// 85 §2: a genai `ServiceTarget` theuth chooses instead of letting genai
/// infer it from the id — the kind decides the wire format and the URL path.
#[derive(Clone, Debug)]
pub(crate) struct ProtocolTarget {
    pub kind: AdapterKind,
    pub endpoint: Endpoint,
    pub auth: AuthData,
}

/// The target for a Zen model (`opencode/*` with a key: kind from the catalog
/// protocol, else the prefix guess) or a user gateway (kind OpenAI unless its
/// `<data_dir>/models.json` entry names a `protocol`); `None` lets genai
/// resolve the id itself.
pub(crate) fn target_for(resolved: &Resolved, auth: &Auth) -> Option<ProtocolTarget> {
    let provider = resolved.provider.as_deref()?;
    let zen = (provider == "opencode")
        .then(|| auth.opencode_key())
        .flatten();
    protocol_target(resolved, zen, auth.gateway(provider))
}

/// [`target_for`] with its key sources resolved (testable without a vault).
fn protocol_target(
    resolved: &Resolved,
    zen_key: Option<String>,
    gateway: Option<Gateway>,
) -> Option<ProtocolTarget> {
    // `Resolved.model` is a sibling for an unknown id (protocol stripped); the
    // guess must read the real slug, not the sibling's.
    let protocol = resolved.model.as_ref().and_then(|m| m.protocol);
    if let Some(key) = zen_key {
        return Some(ProtocolTarget {
            kind: kind_of(protocol.unwrap_or_else(|| Protocol::guess(slug(resolved)))),
            endpoint: Endpoint::from_static(ZEN_ENDPOINT),
            auth: AuthData::Key(key),
        });
    }
    let gw = gateway?;
    Some(ProtocolTarget {
        kind: protocol.map_or(AdapterKind::OpenAI, kind_of),
        endpoint: Endpoint::from_owned(gw.base_url),
        auth: AuthData::Key(gw.key),
    })
}

/// 85 §0.2: protocol → the genai adapter that speaks it.
fn kind_of(p: Protocol) -> AdapterKind {
    match p {
        Protocol::Anthropic => AdapterKind::Anthropic,
        Protocol::OpenaiResponses => AdapterKind::OpenAIResp,
        Protocol::Gemini => AdapterKind::Gemini,
        Protocol::OpenaiChat => AdapterKind::OpenAI,
    }
}

/// 85 §0.6: everything theuth adds to an OpenRouter request, in one place —
/// `provider` preferences and the trailing `cache_control` breakpoint for
/// Anthropic-backed models as body fields, header overrides as headers. genai
/// merges `extra_body` into the chat payload and `extra_headers` into the
/// request; the fork's dedicated OpenRouter options are a rename of these.
fn openrouter_options(
    resolved: &Resolved,
    cfg: &OpenRouterConfig,
    opts: ChatOptions,
) -> ChatOptions {
    if ns(&resolved.genai) != "open_router" {
        return opts;
    }
    let mut body = serde_json::Map::new();
    if let Some(p) = &cfg.provider {
        body.insert("provider".into(), p.clone());
    }
    if resolved
        .model
        .as_ref()
        .and_then(|m| m.canonical.as_deref())
        .is_some_and(|c| c.starts_with("anthropic/"))
    {
        body.insert(
            "cache_control".into(),
            serde_json::json!({"type": "ephemeral"}),
        );
    }
    let headers: Vec<(String, String)> = [
        ("HTTP-Referer", &cfg.referer),
        ("X-OpenRouter-Title", &cfg.title),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.as_ref().map(|v| (k.to_owned(), v.clone())))
    .collect();
    let opts = if body.is_empty() {
        opts
    } else {
        opts.with_extra_body(serde_json::Value::Object(body))
    };
    if headers.is_empty() {
        opts
    } else {
        opts.with_extra_headers(Headers::from(headers))
    }
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

    /// Cache marks for `resolved` (07 §3): Cursor places none; a protocol
    /// target or gateway by the kind theuth chose (Zen Claude → Anthropic
    /// breakpoints, Zen GPT → `prompt_cache_key`); genai adapters by resolved kind.
    pub(crate) async fn transport(&self, resolved: &Resolved) -> Result<Transport, String> {
        match self {
            Backend::Cursor { .. } => Ok(Transport::Plain),
            Backend::Genai { client, auth } => match target_for(resolved, auth) {
                Some(t) => Ok(Transport::of(t.kind)),
                None => {
                    let target = client
                        .resolve_service_target(resolved.genai.as_str())
                        .await
                        .map_err(provider_error)?;
                    Ok(Transport::of(target.model.adapter_kind))
                }
            },
        }
    }

    /// Open the stream for one model call. `session` is Cursor's conversation id.
    /// 137G G1: the one choke point — a request over the model's ceiling is
    /// refused here, locally ([`crate::tokens::preflight`]); every reported
    /// usage (and a provider's overflow count) teaches the model's bytes/token.
    pub(crate) async fn stream(
        &self,
        resolved: &Resolved,
        req: ChatRequest,
        opts: &ChatOptions,
        session: SessionId,
    ) -> Result<EventStream, String> {
        let reply = opts
            .max_tokens
            .or_else(|| resolved.model.as_ref().map(|m| m.limit.output))
            .map_or(0, u64::from);
        crate::tokens::preflight(resolved, &req, reply)?;
        let (bytes, _) = crate::tokens::request_size(&req);
        let id = resolved.id.clone();
        match self {
            Backend::Cursor { oauth: store } => {
                let t = store.access(&oauth::CURSOR).await?;
                chat_stream(&t.access_token, slug(resolved), req, &session.to_string()).await
            }
            Backend::Genai { client, auth } => {
                // 160 §5.4: `limit_auth` names the provider row the response headers feed.
                let (spec, label, limit_auth): (ModelSpec, String, &'static str) =
                    match target_for(resolved, auth) {
                        Some(t) => {
                            let label = connection_label(resolved, t.kind);
                            let limit_auth = if label == "opencode" {
                                "api_key"
                            } else {
                                "gateway"
                            };
                            (
                                ServiceTarget {
                                    endpoint: t.endpoint,
                                    auth: t.auth,
                                    model: ModelIden::new(t.kind, slug(resolved).to_owned()),
                                }
                                .into(),
                                label,
                                limit_auth,
                            )
                        }
                        None => {
                            let target = client
                                .resolve_service_target(resolved.genai.as_str())
                                .await
                                .map_err(provider_error)?;
                            let kind = target.model.adapter_kind;
                            (
                                resolved.genai.as_str().into(),
                                connection_label(resolved, kind),
                                crate::limits::auth_mode(kind, &target.auth, auth.login("xai")),
                            )
                        }
                    };
                let tap = crate::limits::HeaderTap::new(&label, limit_auth, slug(resolved));
                let opts = thought_connection(reply_cap(opts.clone(), resolved), &label)
                    .with_raw_frame_sink_arc(tap);
                let opts = openrouter_options(resolved, &auth.openrouter, opts);
                client
                    .exec_chat_stream(spec, req, Some(&opts))
                    .await
                    .map(|r| -> EventStream {
                        Box::pin(r.stream.map(move |ev| {
                            if let Ok(ChatStreamEvent::End(end)) = &ev
                                && let Some(p) =
                                    end.captured_usage.as_ref().and_then(|u| u.prompt_tokens)
                            {
                                crate::tokens::learn(&id, bytes, u64::try_from(p).unwrap_or(0));
                            }
                            ev.map_err(provider_error)
                        }))
                    })
                    .map_err(|e| {
                        let e = provider_error(e);
                        if let Some(n) = crate::compact::overflow_numbers(&e) {
                            crate::tokens::learn(&resolved.id, bytes, u64::from(n.prompt));
                        }
                        e
                    })
            }
        }
    }
}

/// The catalog's output limit is the request's `max_tokens` unless the frame
/// already set one (116: the reply budget, clamped to the remaining window);
/// genai's per-adapter guesses are wrong for new ids (Bedrock falls back to 4096).
fn reply_cap(opts: ChatOptions, resolved: &Resolved) -> ChatOptions {
    match &resolved.model {
        Some(m) if opts.max_tokens.is_none() => opts.with_max_tokens(m.limit.output),
        _ => opts,
    }
}

/// 90 §1.4: the name of the account/gateway a request goes to, so a thought
/// signature replays only to its issuer — the theuth provider id
/// (`anthropic`, `openai`, `opencode`, `openrouter`, `amazon-bedrock`, a user
/// gateway's name …), else the adapter kind's lower-case name for an id
/// without a provider.
pub(crate) fn connection_label(resolved: &Resolved, kind: AdapterKind) -> String {
    resolved
        .provider
        .clone()
        .unwrap_or_else(|| kind.as_lower_str().to_owned())
}

/// 90 §1.4: tag the call with its connection, so the signatures it captures
/// replay only to the same connection (genai `ThoughtSignature::readable_by`).
fn thought_connection(opts: ChatOptions, label: &str) -> ChatOptions {
    opts.with_thought_connection(label)
}

/// genai currently wraps AWS credential failures as unsupported features.
/// Keep the provider detail, but explain the failed authentication and recovery.
/// A Zen `ModelProtocolUnsupported` body means the model was reached through a
/// user gateway that speaks only chat completions (85 §0.8): point at the built-in.
fn provider_error(error: genai::Error) -> String {
    match error {
        genai::Error::AdapterNotSupported {
            adapter_kind: AdapterKind::BedrockSigv4,
            feature,
        } if feature.starts_with("AWS credential") => format!(
            "AWS authentication failed: {feature}\nCheck the selected AWS profile and credentials. \
             If it uses SSO, run `aws sso login --profile <profile>` (or `aws sso login` for the default profile), then retry."
        ),
        error => {
            let text = error.to_string();
            if text.contains("ModelProtocolUnsupported") {
                format!("{text}\n{BUILTIN_HINT}")
            } else {
                text
            }
        }
    }
}

/// The models a Cursor account may use (`ListModels`; 05 §2.5): Cursor is not
/// in models.dev, so the backend is asked.
pub(crate) async fn cursor_models(token: &str) -> Result<Vec<cursor::Model>, String> {
    cursor::models(token).await
}

/// Whether `resolved` can receive image parts through the backend that
/// would stream it (52 §3): the catalog says the model accepts images
/// **and** the backend is genai. Cursor's AgentService serialises text parts
/// only, so a Cursor id is text-only whatever its catalog sibling supports.
/// This is the one capability rule for `ModelInfo.image`, the loop's routing
/// branch, and vision-fallback validation.
pub(crate) fn supports_images(resolved: &Resolved) -> bool {
    ns(&resolved.genai) != "cursor" && resolved.model.as_ref().is_some_and(|m| m.image)
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
    use genai::resolver::ServiceTargetResolver;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn catalog() -> models::Catalog {
        models::Catalog::load(std::path::Path::new("/nonexistent"))
    }

    fn zen(resolved: &Resolved) -> ProtocolTarget {
        protocol_target(resolved, Some("zk".into()), None).unwrap()
    }

    // 85 §0.2: the kind follows the catalog protocol; an unknown Zen id is guessed from its slug.
    #[test]
    fn zen_targets_follow_catalog_protocol_then_prefix_guess() {
        let cat = catalog();
        let r = |id: &str| cat.resolve(id, &|_| true).unwrap();
        for (id, kind) in [
            ("opencode/claude-opus-5-5", AdapterKind::Anthropic),
            ("opencode/gpt-5.6-sol", AdapterKind::OpenAIResp),
            ("opencode/gemini-3.5-flash", AdapterKind::Gemini),
            ("opencode/deepseek-v4-pro", AdapterKind::OpenAI),
            ("opencode/qwen3.8-flash", AdapterKind::Anthropic),
            // Unknown ids: the sibling's protocol is stripped, the slug decides.
            ("opencode/claude-opus-9", AdapterKind::Anthropic),
            ("opencode/grok-9-fast", AdapterKind::OpenAIResp),
            ("opencode/gemini-9", AdapterKind::Gemini),
            ("opencode/kimi-k9", AdapterKind::OpenAI),
        ] {
            let t = zen(&r(id));
            assert_eq!(t.kind, kind, "{id}");
            assert_eq!(t.endpoint.base_url(), ZEN_ENDPOINT, "{id}");
            assert!(matches!(&t.auth, AuthData::Key(k) if k == "zk"), "{id}");
        }
        // No key, no gateway: genai resolves the id itself.
        assert!(protocol_target(&r("opencode/claude-opus-5-5"), None, None).is_none());
        assert!(protocol_target(&r("anthropic/claude-opus-5-5"), None, None).is_none());
        // A user gateway stays chat completions unless its entry names a protocol.
        let gw = || {
            Some(Gateway {
                base_url: "https://gw.example/v1/".into(),
                key: "gk".into(),
            })
        };
        let mut g = r("rwx/claude-opus-5-5");
        let t = protocol_target(&g, None, gw()).unwrap();
        assert_eq!(t.kind, AdapterKind::OpenAI);
        assert_eq!(t.endpoint.base_url(), "https://gw.example/v1/");
        assert!(matches!(&t.auth, AuthData::Key(k) if k == "gk"));
        g.model.as_mut().unwrap().protocol = Some(Protocol::Anthropic);
        assert_eq!(
            protocol_target(&g, None, gw()).unwrap().kind,
            AdapterKind::Anthropic
        );
        // Transport follows the chosen kind, not the provider.
        assert_eq!(
            Transport::of(zen(&r("opencode/claude-opus-5-5")).kind),
            Transport::Anthropic
        );
        assert_eq!(
            Transport::of(zen(&r("opencode/gpt-5.6-sol")).kind),
            Transport::OpenAiLike
        );
    }

    // 90 §1.4: the provider id names the connection; a bare id falls back to the kind.
    #[test]
    fn connection_label_is_provider_then_kind() {
        let cat = catalog();
        let r = |id: &str| cat.resolve(id, &|_| true).unwrap();
        let z = r("opencode/claude-opus-5-5");
        assert_eq!(connection_label(&z, zen(&z).kind), "opencode");
        assert_eq!(
            connection_label(&r("anthropic/claude-opus-5-5"), AdapterKind::Anthropic),
            "anthropic"
        );
        let bare = Resolved {
            id: "gpt-9".into(),
            genai: "gpt-9".into(),
            provider: None,
            model: None,
        };
        assert_eq!(
            connection_label(&bare, AdapterKind::OpenAIResp),
            "openai_resp"
        );
    }

    /// A fake Zen that records every request; the body is irrelevant.
    async fn fake() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("", "text/event-stream"))
            .mount(&server)
            .await;
        server
    }

    async fn send(client: &Client, spec: ModelSpec, opts: &ChatOptions) {
        let req = ChatRequest::new(vec![genai::chat::ChatMessage::user("hi")]);
        let r = client
            .exec_chat_stream(spec, req, Some(opts))
            .await
            .unwrap();
        let mut s = r.stream;
        while let Some(ev) = s.next().await {
            if ev.is_err() {
                break;
            }
        }
    }

    // 85 §0.2: the path and auth header on the wire, per Zen protocol.
    #[tokio::test]
    async fn zen_wire_path_and_auth_header_per_protocol() {
        let server = fake().await;
        let base = format!("{}/zen/v1/", server.uri());
        let client = Client::new().unwrap();
        let cat = catalog();
        let cases = [
            (
                "opencode/claude-opus-5-5",
                "/zen/v1/messages",
                "x-api-key",
                "zk",
            ),
            (
                "opencode/gpt-5.6-sol",
                "/zen/v1/responses",
                "authorization",
                "Bearer zk",
            ),
            (
                "opencode/gemini-3.5-flash",
                "/zen/v1/models/gemini-3.5-flash:streamGenerateContent",
                "x-goog-api-key",
                "zk",
            ),
            (
                "opencode/deepseek-v4-pro",
                "/zen/v1/chat/completions",
                "authorization",
                "Bearer zk",
            ),
        ];
        for (id, want_path, header, value) in cases {
            let resolved = cat.resolve(id, &|_| true).unwrap();
            let t = zen(&resolved);
            let spec: ModelSpec = ServiceTarget {
                endpoint: Endpoint::from_owned(base.clone()),
                auth: t.auth,
                model: ModelIden::new(t.kind, slug(&resolved).to_owned()),
            }
            .into();
            send(&client, spec, &ChatOptions::default()).await;
            let reqs = server.received_requests().await.unwrap();
            let req = reqs.last().unwrap();
            assert_eq!(req.url.path(), want_path, "{id}");
            assert_eq!(
                req.headers.get(header).map(|v| v.to_str().unwrap()),
                Some(value),
                "{id}"
            );
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            if t.kind != AdapterKind::Gemini {
                assert_eq!(body["model"], slug(&resolved), "{id}");
            }
        }
    }

    // 85 §0.6: provider preferences, the Anthropic trailing breakpoint and header
    // overrides reach an OpenRouter chat request; nothing is added elsewhere.
    #[tokio::test]
    async fn openrouter_passthrough_lands_in_body_and_headers() {
        let cat = catalog();
        let claude = cat
            .resolve("openrouter/anthropic/claude-opus-5.5", &|_| true)
            .unwrap();
        let cfg = OpenRouterConfig {
            provider: Some(serde_json::json!({"order": ["anthropic"], "allow_fallbacks": false})),
            referer: Some("https://example.test".into()),
            title: Some("t".into()),
        };
        let opts = openrouter_options(&claude, &cfg, ChatOptions::default());
        assert_eq!(
            opts.extra_body,
            Some(serde_json::json!({
                "provider": {"order": ["anthropic"], "allow_fallbacks": false},
                "cache_control": {"type": "ephemeral"}
            }))
        );
        // Empty config on a non-Anthropic model: untouched.
        let gpt = cat
            .resolve("openrouter/openai/gpt-5.6-sol", &|_| true)
            .unwrap();
        let plain = openrouter_options(&gpt, &OpenRouterConfig::default(), ChatOptions::default());
        assert!(plain.extra_body.is_none() && plain.extra_headers.is_none());
        // Other providers never see the section.
        let direct = cat.resolve("anthropic/claude-opus-5-5", &|_| true).unwrap();
        assert!(
            openrouter_options(&direct, &cfg, ChatOptions::default())
                .extra_body
                .is_none()
        );

        let server = fake().await;
        let base = format!("{}/api/v1/", server.uri());
        let client = Client::builder()
            .with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
                move |mut t: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
                    t.endpoint = Endpoint::from_owned(base.clone());
                    t.auth = AuthData::Key("ork".into());
                    Ok(t)
                },
            ))
            .build()
            .unwrap();
        send(&client, claude.genai.as_str().into(), &opts).await;
        let reqs = server.received_requests().await.unwrap();
        let req = reqs.last().unwrap();
        assert_eq!(req.url.path(), "/api/v1/chat/completions");
        let h = |k: &str| req.headers.get(k).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(h("authorization").as_deref(), Some("Bearer ork"));
        assert_eq!(h("http-referer").as_deref(), Some("https://example.test"));
        assert_eq!(h("x-openrouter-title").as_deref(), Some("t"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "anthropic/claude-opus-5.5");
        assert_eq!(
            body["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        assert_eq!(body["provider"]["allow_fallbacks"], false);
    }

    // 85 §0.8: a Zen protocol refusal through a gateway names the built-in.
    #[test]
    fn zen_protocol_unsupported_gets_builtin_hint() {
        let text = provider_error(genai::Error::Internal(
            r#"{"type":"error","error":{"type":"ModelProtocolUnsupported","message":"use /messages"}}"#.into(),
        ));
        assert!(text.contains("ModelProtocolUnsupported") && text.ends_with(BUILTIN_HINT));
        assert_eq!(
            provider_error(genai::Error::Internal("boom".into())),
            "Internal error: boom"
        );
    }

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

    // 160 §5.4 / review 160-LIM #4: a gateway's 429 reaches the limits hub
    // through `Backend::stream` (the fork's `on_response`), status `limited`.
    #[tokio::test]
    async fn gateway_429_through_stream_marks_the_row_limited() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("x-ratelimit-limit-requests", "100")
                    .insert_header("x-ratelimit-remaining-requests", "0")
                    .insert_header("x-ratelimit-reset-requests", "6m0s")
                    .insert_header("retry-after", "30")
                    .set_body_raw(r#"{"error":{"message":"slow down"}}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let dir = crate::TestDir::new("backend-429");
        let auth = crate::supervisor::Auth::stub(&dir.0);
        auth.gateways.write().unwrap().insert(
            "z160gw".into(),
            Gateway {
                base_url: format!("{}/v1/", server.uri()),
                key: "gk".into(),
            },
        );
        let backend = Backend::Genai {
            client: Client::new().unwrap(),
            auth,
        };
        let resolved = Resolved {
            id: "z160gw/m-1".into(),
            genai: "z160gw/m-1".into(),
            provider: Some("z160gw".into()),
            model: None,
        };
        let req = ChatRequest::new(vec![genai::chat::ChatMessage::user("hi")]);
        let opened = backend
            .stream(&resolved, req, &ChatOptions::default(), SessionId::new())
            .await;
        if let Ok(mut s) = opened {
            while let Some(ev) = s.next().await {
                if ev.is_err() {
                    break;
                }
            }
        }
        let snap = crate::limits::LimitsHub::global().snapshot();
        let row = snap
            .providers
            .iter()
            .find(|p| p.id == "z160gw:gateway")
            .expect("gateway row");
        assert_eq!(row.status, crate::limits::LimitStatus::Limited, "{row:?}");
        assert_eq!(row.rates[0].remaining, 0, "{row:?}");
    }

    #[test]
    fn cursor_never_supports_images() {
        let eyes = models::Model {
            id: "claude-sonnet-4-5".into(),
            name: "Sonnet".into(),
            limit: models::Limit {
                context: 200_000,
                output: 8_192,
            },
            cost: None,
            reasoning: false,
            tool_call: true,
            image: true,
            reasoning_options: vec![],
            protocol: None,
            canonical: None,
        };
        let mut r = Resolved {
            id: "cursor/claude-sonnet-4-5".into(),
            genai: "cursor::claude-sonnet-4-5".into(),
            provider: Some("cursor".into()),
            model: Some(eyes.clone()),
        };
        assert!(!supports_images(&r));
        r.genai = "anthropic::claude-sonnet-4-5".into();
        assert!(supports_images(&r));
        r.model.as_mut().unwrap().image = false;
        assert!(!supports_images(&r));
        r.model = None;
        assert!(!supports_images(&r));
    }

    // 116: a frame's reply budget is not overwritten; an unset one still gets the catalog limit.
    #[test]
    fn reply_cap_keeps_a_budget_the_frame_set() {
        let r = Resolved {
            id: "anthropic/claude-sonnet-4-5".into(),
            genai: "anthropic::claude-sonnet-4-5".into(),
            provider: Some("anthropic".into()),
            model: Some(models::Model {
                id: "claude-sonnet-4-5".into(),
                name: "Sonnet".into(),
                limit: models::Limit {
                    context: 200_000,
                    output: 8_192,
                },
                cost: None,
                reasoning: false,
                tool_call: true,
                image: true,
                reasoning_options: vec![],
                protocol: None,
                canonical: None,
            }),
        };
        let set = ChatOptions::default().with_max_tokens(4_000);
        assert_eq!(reply_cap(set, &r).max_tokens, Some(4_000));
        assert_eq!(
            reply_cap(ChatOptions::default(), &r).max_tokens,
            Some(8_192)
        );
        let unknown = Resolved { model: None, ..r };
        assert_eq!(reply_cap(ChatOptions::default(), &unknown).max_tokens, None);
    }
}
