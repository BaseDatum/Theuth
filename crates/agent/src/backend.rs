//! One seam for "which client streams this model" (12 §6, 05 §2.5).
//!
//! Cursor's AgentService is Connect-RPC/protobuf over HTTP/2 and not
//! OpenAI-shaped, so genai has no adapter for it; `crates/cursor` is that
//! adapter and this module is the only place in `agent` that reaches it.
//!
//! Named provider targets are final: their streams use a reused genai client
//! without auth/login resolvers; only built-in ids use the shared resolver.

use std::pin::Pin;

use cursor::chat_stream;
use futures::{Stream, StreamExt};
use genai::Headers;
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatFrameSink, ChatOptions, ChatRequest, ChatStreamEvent, FrameCtx, RawFrameRef, StopReason,
    StreamEnd, UsageCost, UsageCostSource,
};
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
/// OpenCode Go's base: the API family is in the path ([`go_endpoint`]); the
/// public model list is `{GO_BASE}v1/models`.
pub(crate) const GO_BASE: &str = "https://opencode.ai/inference/go/";

/// Appended when a user gateway answers Zen's `ModelProtocolUnsupported` (85 §0.8).
const BUILTIN_HINT: &str =
    "built-in provider — set the `opencode` / `opencode-go` / `openrouter` secret instead";

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
    /// Cursor's AgentService (05 §2.5); the bearer is `login`'s token (the `cursor`
    /// login, or a Cursor account instance's).
    Cursor {
        oauth: oauth::Store,
        login: oauth::Provider,
    },
}

/// 85 §2: a genai `ServiceTarget` theuth chooses instead of letting genai
/// infer it from the id — the kind decides the wire format and the URL path.
#[derive(Clone, Debug)]
pub(crate) struct ProtocolTarget {
    pub kind: AdapterKind,
    pub endpoint: Endpoint,
    pub auth: AuthData,
    /// An account instance logged in to this login kind: `auth` is filled per call
    /// from the instance's grant ([`oauth_target`]).
    pub login: Option<&'static oauth::Provider>,
}

/// The OpenAI API, where an OpenAI account's key goes.
const OPENAI_API: &str = "https://api.openai.com/v1/";
/// The Anthropic API (keys and Claude subscriptions alike).
const ANTHROPIC_API: &str = "https://api.anthropic.com/v1/";
/// The xAI API (keys and SuperGrok logins alike).
const XAI_API: &str = "https://api.x.ai/v1/";
/// A ChatGPT subscription's Responses backend (design doc 05 §2.3).
const CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/";

/// The target for a Zen model (`opencode/*` with a key: kind from the catalog
/// protocol, else the prefix guess), a Go model (`opencode-go/*` with a key,
/// [`go_target`]), an account instance ([`account_target`]) or a user gateway
/// ([`gateway_kind`]); `None` lets genai resolve the id itself.
pub(crate) fn target_for(resolved: &Resolved, auth: &Auth) -> Option<ProtocolTarget> {
    let provider = resolved.provider.as_deref()?;
    if provider == "opencode-go" {
        return auth.opencode_go_key().map(|k| go_target(resolved, k));
    }
    if let Some(kind) = auth.instance(provider) {
        return account_target(resolved, kind, auth.key(provider), auth.login(provider));
    }
    let zen = (provider == "opencode")
        .then(|| auth.opencode_key())
        .flatten();
    protocol_target(resolved, zen, auth.gateway(provider))
}

/// An account instance's target (`anthropic-work`): its key on the kind's API (a key
/// beats a login, as for the kind itself), else its login's shape with `login` set so
/// [`genai_request`] fills the auth per call, else the kind's API with no auth
/// ([`genai_request`] refuses it). Never `None` (genai would infer the kind's own
/// account) except for a Cursor account, which has its own backend.
fn account_target(
    resolved: &Resolved,
    kind: &'static oauth::Provider,
    key: Option<String>,
    logged_in: bool,
) -> Option<ProtocolTarget> {
    let family = &*kind.ns;
    if family == "cursor" {
        return None;
    }
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        let (adapter, endpoint) = match family {
            "anthropic" => (AdapterKind::Anthropic, ANTHROPIC_API),
            "xai" => (AdapterKind::Xai, XAI_API),
            _ => (openai_kind(resolved), OPENAI_API),
        };
        return Some(ProtocolTarget {
            kind: adapter,
            endpoint: Endpoint::from_static(endpoint),
            auth: AuthData::Key(key),
            login: None,
        });
    }
    let (adapter, endpoint) = match family {
        "anthropic" => (AdapterKind::Anthropic, ANTHROPIC_API),
        "xai" => (AdapterKind::Xai, XAI_API),
        _ => (AdapterKind::OpenAIResp, CODEX_ENDPOINT),
    };
    Some(ProtocolTarget {
        kind: adapter,
        endpoint: Endpoint::from_static(endpoint),
        auth: AuthData::None,
        login: logged_in.then_some(kind),
    })
}

/// The adapter an OpenAI key speaks for `resolved`: its catalog protocol, else genai's
/// own pick for the slug (Responses for `gpt-5`, `o3`, `*-codex` …, else chat completions).
fn openai_kind(resolved: &Resolved) -> AdapterKind {
    resolved
        .model
        .as_ref()
        .and_then(|m| m.protocol)
        .map_or_else(
            || match AdapterKind::from_model(slug(resolved)) {
                Ok(AdapterKind::OpenAIResp) => AdapterKind::OpenAIResp,
                _ => AdapterKind::OpenAI,
            },
            kind_of,
        )
}

/// The target a subscription login speaks (design doc 05 §2.3), its token from `p`'s grant
/// (refreshed when due): OpenAI → the ChatGPT Codex backend, Anthropic → the Messages API
/// with the Claude Code bearer and betas, xAI → the token as the key. Shared by the kinds'
/// own logins (`supervisor::client`'s resolver) and their account instances.
pub(crate) async fn oauth_target(
    family: &str,
    p: &oauth::Provider,
    store: &oauth::Store,
) -> Result<ProtocolTarget, String> {
    let t = store.access(p).await?;
    let (kind, endpoint, auth) = match family {
        "openai" => (
            AdapterKind::OpenAIResp,
            CODEX_ENDPOINT,
            AuthData::RequestOverride {
                url: format!("{CODEX_ENDPOINT}responses"),
                headers: Headers::from(vec![
                    ("Authorization", format!("Bearer {}", t.access_token)),
                    ("chatgpt-account-id", t.account_id.unwrap_or_default()),
                    ("OpenAI-Beta", "responses=v1".to_owned()),
                    ("originator", "codex_cli_rs".to_owned()),
                ]),
            },
        ),
        "anthropic" => {
            let cc = crate::claude_code::global().current();
            (
                AdapterKind::Anthropic,
                ANTHROPIC_API,
                AuthData::RequestOverride {
                    url: format!("{ANTHROPIC_API}messages?beta=true"),
                    headers: Headers::from(vec![
                        ("Authorization", format!("Bearer {}", t.access_token)),
                        ("anthropic-version", "2023-06-01".to_owned()),
                        ("User-Agent", cc.user_agent()),
                        ("anthropic-beta", cc.betas),
                        ("x-app", "cli".to_owned()),
                        (
                            "anthropic-dangerous-direct-browser-access",
                            "true".to_owned(),
                        ),
                    ]),
                },
            )
        }
        "xai" => (AdapterKind::Xai, XAI_API, AuthData::Key(t.access_token)),
        other => return Err(format!("{other}: no model API for its login")),
    };
    Ok(ProtocolTarget {
        kind,
        endpoint: Endpoint::from_static(endpoint),
        auth,
        login: None,
    })
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
            login: None,
        });
    }
    let gw = gateway?;
    let kind = gateway_kind(protocol, &gw);
    // 177 E4: a keyless local provider's calls still go through genai, whose
    // OpenAI adapter needs a key: a placeholder servers that check none ignore.
    let key = if gw.key.is_empty() {
        KEYLESS_PLACEHOLDER.to_owned()
    } else {
        gw.key
    };
    let endpoint = if kind == AdapterKind::Gemini {
        gemini_base(&gw.base_url)
    } else {
        gw.base_url
    };
    Some(ProtocolTarget {
        kind,
        endpoint: Endpoint::from_owned(endpoint),
        auth: AuthData::Key(key),
        login: None,
    })
}

/// A gateway's adapter: its model's `models.json` protocol, else its
/// `providers.yaml` row's, else Gemini on Google's Gemini API host (its
/// OpenAI-compatible surface rejects OpenAI's cache fields, and the native
/// one reports implicit-cache hits), else OpenAI chat completions.
fn gateway_kind(model: Option<Protocol>, gw: &Gateway) -> AdapterKind {
    model
        .map(kind_of)
        .or_else(|| gw.protocol.map(|p| p.kind()))
        .unwrap_or_else(|| {
            if gemini_api_host(&gw.base_url) {
                AdapterKind::Gemini
            } else {
                AdapterKind::OpenAI
            }
        })
}

/// Google's Gemini API host (`generativelanguage.googleapis.com`), whose
/// models are the built-in `google/*` ones.
pub(crate) const GEMINI_API_HOST: &str = "generativelanguage.googleapis.com";

fn url_host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// `url` is on [`GEMINI_API_HOST`].
pub(crate) fn gemini_api_host(url: &str) -> bool {
    url_host(url).is_some_and(|h| h == GEMINI_API_HOST)
}

/// `url` is on a Google API host (`*.googleapis.com`: the Gemini API, Vertex
/// `aiplatform`), whose OpenAI-compatible endpoints reject OpenAI's
/// `prompt_cache_key` / `prompt_cache_retention` with a 400.
pub(crate) fn google_api_host(url: &str) -> bool {
    url_host(url).is_some_and(|h| h == "googleapis.com" || h.ends_with(".googleapis.com"))
}

/// A Gemini-protocol base: genai appends `models/<id>:…`, so the
/// OpenAI-compatible suffix (`…/v1beta/openai/`) goes (`…/v1beta/`).
fn gemini_base(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let base = trimmed.strip_suffix("/openai").unwrap_or(trimmed);
    format!("{base}/")
}

/// A Go model's target: its catalog protocol's family, else chat completions —
/// Go serves every text model on every family (it translates), so an unknown
/// id needs no guess.
fn go_target(resolved: &Resolved, key: String) -> ProtocolTarget {
    let kind = resolved
        .model
        .as_ref()
        .and_then(|m| m.protocol)
        .map_or(AdapterKind::OpenAI, kind_of);
    ProtocolTarget {
        kind,
        endpoint: Endpoint::from_static(go_endpoint(kind)),
        auth: AuthData::Key(key),
        login: None,
    }
}

/// Go's base per API family (Console docs, harnesses § Base URLs); genai
/// appends `chat/completions`, `responses`, `messages` or `models/<id>:…`.
fn go_endpoint(kind: AdapterKind) -> &'static str {
    match kind {
        AdapterKind::Anthropic => "https://opencode.ai/inference/go/anthropic/v1/",
        AdapterKind::Gemini => "https://opencode.ai/inference/go/google/v1beta/",
        _ => "https://opencode.ai/inference/go/openai/v1/",
    }
}

/// 177 E4: the Bearer value a keyless local provider's model calls carry.
pub(crate) const KEYLESS_PLACEHOLDER: &str = "local";

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

/// 177 E2: the body field that switches a local server's chat template out
/// of thinking (vLLM, llama.cpp, omlx, SGLang read it), merged last. Never
/// paired with an effort keyword: genai's omlx adapter turns any
/// `reasoning_effort` into `enable_thinking: true`.
pub(crate) fn thinking_off(opts: ChatOptions) -> ChatOptions {
    opts.with_extra_body(serde_json::json!({
        "chat_template_kwargs": {"enable_thinking": false}
    }))
}

/// OpenCode Go's client rules: theuth's own user agent, the theuth session id
/// as `x-opencode-session` (a request without it is a 400 `MissingSessionID`),
/// and the key as a Bearer on every API family (genai's Anthropic and Gemini
/// adapters send only their own key header). Merged over headers already set.
fn go_headers(session: SessionId, key: &str, opts: ChatOptions) -> ChatOptions {
    let mut headers = opts.extra_headers.clone().unwrap_or_default();
    headers.merge(vec![
        ("Authorization", format!("Bearer {key}")),
        (
            "User-Agent",
            format!("theuth/{}", platform::version::get()),
        ),
        ("x-opencode-session", session.to_string()),
    ]);
    opts.with_extra_headers(headers)
}

/// Go reports what a call charged in `usage.cost` (`usageMetadata.cost` on
/// Gemini) on its final usage frame — 0 within the plan's limits; genai reads
/// it on the OpenAI families only. Forwards every frame to the limits tap and
/// keeps the last amount for [`GoCost::fill`].
struct GoCost {
    tap: std::sync::Arc<crate::limits::HeaderTap>,
    usd: parking_lot::Mutex<Option<f64>>,
}

impl GoCost {
    /// The reported charge as the call's provider-reported cost, unless genai already read one.
    fn fill(&self, end: &mut StreamEnd) {
        let Some(amount) = *self.usd.lock() else {
            return;
        };
        let usage = end.captured_usage.get_or_insert_default();
        if usage.cost.is_none() {
            usage.cost = Some(UsageCost {
                amount,
                currency: "USD".into(),
                source: UsageCostSource::ProviderReported,
            });
        }
    }
}

/// `usage.cost` / `usageMetadata.cost` of one frame's JSON, when it has one.
fn go_frame_cost(data: &str) -> Option<f64> {
    if !data.contains("\"cost\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let usage = v.get("usage").or_else(|| v.get("usageMetadata"))?;
    usage
        .get("cost")?
        .as_f64()
        .filter(|c| c.is_finite() && *c >= 0.0)
}

impl ChatFrameSink for GoCost {
    fn on_frame(&self, ctx: &FrameCtx, frame: RawFrameRef<'_>) {
        self.tap.on_frame(ctx, frame);
        if let Some(c) = go_frame_cost(frame.data) {
            *self.usd.lock() = Some(c);
        }
    }

    fn on_end(&self, ctx: &FrameCtx, end: &StreamEnd) {
        self.tap.on_end(ctx, end);
    }

    fn on_error(&self, ctx: &FrameCtx, err: &genai::Error) {
        self.tap.on_error(ctx, err);
    }

    fn on_response(
        &self,
        ctx: &FrameCtx,
        status: reqwest::StatusCode,
        headers: &reqwest::header::HeaderMap,
    ) {
        self.tap.on_response(ctx, status, headers);
    }
}

impl Backend {
    /// The `cursor` namespace (the `cursor` login, or a Cursor account instance's, whose
    /// ids the catalog routes there) is the one id genai cannot serve; everything else,
    /// gateways included, goes through the genai client.
    pub(crate) fn for_model(resolved: &Resolved, client: &Client, auth: &Auth) -> Backend {
        if ns(&resolved.genai) == "cursor" {
            let login = match resolved.provider.as_deref() {
                Some(p) if auth.instance(p).is_some() => oauth::instance(&oauth::CURSOR, p),
                _ => oauth::CURSOR,
            };
            Backend::Cursor {
                oauth: auth.oauth.clone(),
                login,
            }
        } else {
            Backend::Genai {
                client: client.clone(),
                auth: auth.clone(),
            }
        }
    }

    /// 177 E2: `resolved` is served by a local model server over the OpenAI
    /// chat protocol — a custom provider (gateway) of kind OpenAI with a
    /// runtime preset or a loopback host (R2-5, not a LAN gateway in front of
    /// cloud models), or genai's own omlx adapter — where `chat_template_kwargs`
    /// steers thinking.
    pub(crate) fn local_openai(&self, resolved: &Resolved) -> bool {
        let Backend::Genai { auth, .. } = self else {
            return false;
        };
        match target_for(resolved, auth) {
            Some(t) => {
                t.kind == AdapterKind::OpenAI
                    && resolved
                        .provider
                        .as_deref()
                        .and_then(|p| auth.gateway(p))
                        .is_some_and(|g| g.on_this_machine())
            }
            None => ns(&resolved.genai) == "omlx",
        }
    }

    /// Cache marks for `resolved` (07 §3): Cursor places none; a protocol
    /// target or gateway by the kind theuth chose (Zen Claude → Anthropic
    /// breakpoints, Zen GPT → `prompt_cache_key`); genai adapters by resolved
    /// kind. Both by endpoint too: a Google host takes no cache fields.
    pub(crate) async fn transport(&self, resolved: &Resolved) -> Result<Transport, String> {
        match self {
            Backend::Cursor { .. } => Ok(Transport::Plain),
            Backend::Genai { client, auth } => match target_for(resolved, auth) {
                Some(t) => Ok(Transport::of(t.kind, t.endpoint.base_url())),
                None => {
                    let target = client
                        .resolve_service_target(resolved.genai.as_str())
                        .await
                        .map_err(provider_error)?;
                    Ok(Transport::of(
                        target.model.adapter_kind,
                        target.endpoint.base_url(),
                    ))
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
        // R2-4: images/PDFs count flat, not per byte — the learner sees text only.
        let (bytes, flat) = crate::tokens::request_size(&req);
        let id = resolved.id.clone();
        match self {
            Backend::Cursor {
                oauth: store,
                login,
            } => {
                let t = store.access(login).await?;
                // 194: the request's explicit effort (no default is set for
                // Cursor, `frame::default_effort`) replaces the slug's reasoning.
                let effort = opts.reasoning_effort.as_ref();
                chat_stream(
                    &t.access_token,
                    slug(resolved),
                    effort,
                    req,
                    &session.to_string(),
                )
                .await
            }
            Backend::Genai { client, auth } => {
                let (client, spec, label, limit_auth, subscription) =
                    genai_request(client, resolved, auth).await?;
                // An Anthropic subscription call is shaped as Claude Code's
                // (`claude_code::wire`); the reply gets theuth's tool names back.
                let (req, names) = if subscription {
                    let (req, names) = crate::claude_code::global().current().wire.outgoing(req);
                    (req, Some(names))
                } else {
                    (req, None)
                };
                let tap = crate::limits::HeaderTap::new(&label, limit_auth, slug(resolved));
                // Go with a key: its headers, and its reported charge read off the frames.
                let go_key = (resolved.provider.as_deref() == Some("opencode-go"))
                    .then(|| auth.opencode_go_key())
                    .flatten();
                let (sink, go_cost): (std::sync::Arc<dyn ChatFrameSink>, _) = match go_key {
                    Some(_) => {
                        let c = std::sync::Arc::new(GoCost {
                            tap,
                            usd: parking_lot::Mutex::new(None),
                        });
                        (c.clone(), Some(c))
                    }
                    None => (tap, None),
                };
                let opts = thought_connection(reply_cap(opts.clone(), resolved), &label)
                    .with_raw_frame_sink_arc(sink);
                let opts = openrouter_options(resolved, &auth.openrouter, opts);
                let opts = match &go_key {
                    Some(k) => go_headers(session, k, opts),
                    None => opts,
                };
                client
                    .exec_chat_stream(spec, req, Some(&opts))
                    .await
                    .map(|r| -> EventStream {
                        Box::pin(r.stream.map(move |mut ev| {
                            if let (Ok(e), Some(n)) = (&mut ev, &names) {
                                *e = n.event(std::mem::replace(e, ChatStreamEvent::Start));
                            }
                            if let (Ok(ChatStreamEvent::End(end)), Some(c)) = (&mut ev, &go_cost) {
                                c.fill(end);
                            }
                            if let Ok(ChatStreamEvent::End(end)) = &ev
                                && let Some(p) =
                                    end.captured_usage.as_ref().and_then(|u| u.prompt_tokens)
                            {
                                crate::tokens::learn(
                                    &id,
                                    bytes,
                                    flat,
                                    u64::try_from(p).unwrap_or(0),
                                );
                            }
                            ev.map_err(provider_error)
                        }))
                    })
                    .map_err(|e| {
                        let e = provider_error(e);
                        if let Some(n) = crate::compact::overflow_numbers(&e) {
                            crate::tokens::learn(&resolved.id, bytes, flat, u64::from(n.prompt));
                        }
                        e
                    })
            }
        }
    }
}

/// The client and spec `stream` sends, with the existing limits attribution,
/// and whether the call goes out on an Anthropic subscription — the kind's own
/// (`supervisor::client`'s resolver swaps in its bearer) or an account instance's.
async fn genai_request<'a>(
    client: &'a Client,
    resolved: &Resolved,
    auth: &Auth,
) -> Result<(&'a Client, ModelSpec, String, &'static str, bool), String> {
    // 160 §5.4: `limit_auth` names the provider row the response headers feed.
    match target_for(resolved, auth) {
        Some(t) => {
            let account = resolved
                .provider
                .as_deref()
                .filter(|p| auth.instance(p).is_some());
            let logged_in = t.login.is_some();
            let t = match (t.login, account) {
                (Some(kind), Some(name)) => {
                    oauth_target(&kind.ns, &oauth::instance(kind, name), &auth.oauth).await?
                }
                (None, Some(name)) if matches!(t.auth, AuthData::None) => {
                    return Err(format!(
                        "{name} has no API key or login: add a key or log in under Settings › Model providers"
                    ));
                }
                _ => t,
            };
            let label = connection_label(resolved, t.kind);
            let limit_auth = if account.is_some() {
                crate::limits::auth_mode(t.kind, &t.auth, logged_in)
            } else if matches!(label.as_str(), "opencode" | "opencode-go") {
                "api_key"
            } else {
                "gateway"
            };
            let subscription = account.is_some()
                && t.kind == AdapterKind::Anthropic
                && matches!(t.auth, AuthData::RequestOverride { .. });
            Ok((
                crate::supervisor::protocol_client()?,
                ServiceTarget {
                    endpoint: t.endpoint,
                    auth: t.auth,
                    model: ModelIden::new(t.kind, target_model(resolved, t.kind).to_owned()),
                }
                .into(),
                label,
                limit_auth,
                subscription,
            ))
        }
        None => {
            let target = client
                .resolve_service_target(resolved.genai.as_str())
                .await
                .map_err(provider_error)?;
            let kind = target.model.adapter_kind;
            Ok((
                client,
                resolved.genai.as_str().into(),
                connection_label(resolved, kind),
                crate::limits::auth_mode(kind, &target.auth, auth.login("xai")),
                kind == AdapterKind::Anthropic && auth.login("anthropic"),
            ))
        }
    }
}

/// The catalog's output limit is the request's `max_tokens` unless the frame
/// already set one (116: the reply budget, clamped to the remaining window);
/// genai's per-adapter guesses are wrong for new ids (Bedrock falls back to 4096).
fn reply_cap(opts: ChatOptions, resolved: &Resolved) -> ChatOptions {
    match &resolved.model {
        // 177 E1: 0 = unknown; a server rejects `max_tokens: 0` (omlx: 422).
        Some(m) if opts.max_tokens.is_none() && m.limit.output > 0 => {
            opts.with_max_tokens(m.limit.output)
        }
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
/// A Claude Code too-old refusal raises the live client version
/// ([`crate::claude_code`]); the loop resends a turn's call, others say to resend.
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
            } else if let Some(v) = crate::claude_code::global().heal(&text) {
                format!("{text}\ntheuth updated its Claude Code client version to {v}; resend.")
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

/// The model name a protocol target sends: the slug, without the `models/`
/// prefix Google's OpenAI-compatible listing gives its ids
/// (`gemini/models/gemini-2.5-pro`) when genai's Gemini adapter, which
/// inserts `models/` itself, speaks it.
fn target_model(resolved: &Resolved, kind: AdapterKind) -> &str {
    let s = slug(resolved);
    match kind {
        AdapterKind::Gemini => s.strip_prefix("models/").unwrap_or(s),
        _ => s,
    }
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

    async fn oauth_auth() -> (crate::TestDir, Auth) {
        let scratch = crate::test_dir("backend-oauth-route");
        let vault = std::sync::Arc::new(
            secrets::Vault::open(scratch.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let mut auth = Auth::stub(scratch.path());
        auth.vault = Some(vault.clone());
        auth.oauth = auth.oauth.with_grants(Some(vault.clone()));
        for ns in ["openai", "anthropic", "xai"] {
            auth.oauth
                .save(
                    ns,
                    &oauth::Tokens {
                        access_token: format!("{ns}-oauth-token"),
                        refresh_token: None,
                        expires_at: 4_102_444_800,
                        account_id: Some("test-account".into()),
                    },
                )
                .await
                .unwrap();
            assert!(auth.login(ns), "{ns} login must be in force");
        }
        vault.set("opencode", "zk", None, "test").await.unwrap();
        vault.set("opencode-go", "gk", None, "test").await.unwrap();
        (scratch, auth)
    }

    async fn routed_target(resolved: &Resolved, auth: &Auth) -> ServiceTarget {
        let shared = crate::supervisor::client(auth.clone()).unwrap();
        let (client, spec, label, limit_auth, subscription) =
            genai_request(&shared, resolved, auth).await.unwrap();
        // Every login is in force here: only the built-in Anthropic id is
        // shaped as Claude Code's; a gateway or Zen target never is.
        assert_eq!(
            subscription,
            target_for(resolved, auth).is_none() && ns(&resolved.genai) == "anthropic",
            "{}",
            resolved.id
        );
        if target_for(resolved, auth).is_some() {
            assert_eq!(label, resolved.provider.as_deref().unwrap());
            assert_eq!(
                limit_auth,
                if matches!(label.as_str(), "opencode" | "opencode-go") {
                    "api_key"
                } else {
                    "gateway"
                }
            );
        }
        // Resolves only; no chat, listing, or token refresh can contact a host.
        client.resolve_service_target(spec).await.unwrap()
    }

    /// A `gateway` row at `base` (row `protocol`) serving `model` (its
    /// `models.json` `protocol`): the routed target and the transport.
    async fn gateway_route(
        base: &str,
        row: Option<crate::providers::GatewayProtocol>,
        model: &str,
        protocol: Option<Protocol>,
    ) -> (ServiceTarget, Transport) {
        let (scratch, auth) = oauth_auth().await;
        auth.gateways.write().unwrap().insert(
            "gateway".into(),
            Gateway {
                base_url: base.into(),
                key: "gateway-key".into(),
                runtime: None,
                protocol: row,
            },
        );
        std::fs::write(
            scratch.path().join("models.json"),
            serde_json::json!({"gateway": {"models": {model: {
                "id": model, "name": model, "protocol": protocol,
                "limit": {"context": 0, "output": 0}
            }}}})
            .to_string(),
        )
        .unwrap();
        let resolved = models::Catalog::load(scratch.path())
            .resolve(&format!("gateway/{model}"), &|_| true)
            .unwrap();
        let target = routed_target(&resolved, &auth).await;
        assert!(
            matches!(&target.auth, AuthData::Key(k) if k == "gateway-key"),
            "gateway key replaced: {:?}",
            target.auth
        );
        let backend = Backend::Genai {
            client: Client::new().unwrap(),
            auth,
        };
        let transport = backend.transport(&resolved).await.unwrap();
        (target, transport)
    }

    async fn assert_gateway_route(base: &str, protocol: Option<Protocol>, kind: AdapterKind) {
        let (target, _) = gateway_route(base, None, "x", protocol).await;
        assert_eq!(target.endpoint.base_url(), base, "gateway endpoint changed");
        assert_eq!(target.model.adapter_kind, kind, "gateway kind changed");
        assert_eq!(&*target.model.model_name, "x");
    }

    const GOOGLE_OPENAI: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";

    // A gateway on Google's Gemini API host speaks native Gemini by default:
    // `openai/` dropped from the base, the listing's `models/` prefix dropped
    // from the name (genai inserts it), its own key, no OpenAI cache fields.
    #[tokio::test]
    async fn google_host_gateway_routes_to_native_gemini() {
        let (t, transport) =
            gateway_route(GOOGLE_OPENAI, None, "models/gemini-2.5-flash", None).await;
        assert_eq!(t.model.adapter_kind, AdapterKind::Gemini);
        assert_eq!(
            t.endpoint.base_url(),
            "https://generativelanguage.googleapis.com/v1beta/"
        );
        assert_eq!(&*t.model.model_name, "gemini-2.5-flash");
        assert_eq!(transport, Transport::Plain);
    }

    // `protocol: openai` on a Google host keeps the OpenAI adapter and URL,
    // and the guard drops `prompt_cache_key` / retention (Plain transport).
    #[tokio::test]
    async fn google_host_gateway_explicit_openai_drops_cache_fields() {
        use crate::providers::GatewayProtocol;
        let (t, transport) = gateway_route(
            GOOGLE_OPENAI,
            Some(GatewayProtocol::Openai),
            "models/gemini-2.5-flash",
            None,
        )
        .await;
        assert_eq!(t.model.adapter_kind, AdapterKind::OpenAI);
        assert_eq!(t.endpoint.base_url(), GOOGLE_OPENAI);
        assert_eq!(&*t.model.model_name, "models/gemini-2.5-flash");
        assert_eq!(transport, Transport::Plain);
        // A model's own `models.json` protocol still wins over the row's.
        let (t, _) = gateway_route(
            GOOGLE_OPENAI,
            Some(GatewayProtocol::Openai),
            "x",
            Some(Protocol::Gemini),
        )
        .await;
        assert_eq!(t.model.adapter_kind, AdapterKind::Gemini);
        // Elsewhere an OpenAI gateway keeps its cache key.
        let (_, transport) = gateway_route("https://gateway.example/v1/", None, "x", None).await;
        assert_eq!(transport, Transport::OpenAiLike);
    }

    // `protocol: gemini` on any other host: native Gemini on the wire —
    // `…/v1beta/models/<id>:streamGenerateContent`, the row's key as
    // `x-goog-api-key`, through `Backend::stream`.
    #[tokio::test]
    async fn gemini_protocol_gateway_on_other_host_wire() {
        let server = fake().await;
        let dir = crate::test_dir("backend-gemini-gw");
        let auth = crate::supervisor::Auth::stub(dir.path());
        auth.gateways.write().unwrap().insert(
            "gproxy".into(),
            Gateway {
                base_url: format!("{}/v1beta/openai/", server.uri()),
                key: "gpk".into(),
                runtime: None,
                protocol: Some(crate::providers::GatewayProtocol::Gemini),
            },
        );
        let backend = Backend::Genai {
            client: Client::new().unwrap(),
            auth,
        };
        let resolved = Resolved {
            id: "gproxy/models/gemini-2.5-pro".into(),
            genai: "gproxy::models/gemini-2.5-pro".into(),
            provider: Some("gproxy".into()),
            model: None,
        };
        assert_eq!(
            backend.transport(&resolved).await.unwrap(),
            Transport::Plain
        );
        let req = ChatRequest::new(vec![genai::chat::ChatMessage::user("hi")]);
        let mut s = backend
            .stream(&resolved, req, &ChatOptions::default(), SessionId::new())
            .await
            .unwrap();
        while let Some(ev) = s.next().await {
            if ev.is_err() {
                break;
            }
        }
        let reqs = server.received_requests().await.unwrap();
        let req = reqs.last().unwrap();
        assert_eq!(
            req.url.path(),
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent"
        );
        assert_eq!(
            req.headers
                .get("x-goog-api-key")
                .map(|v| v.to_str().unwrap()),
            Some("gpk")
        );
    }

    #[test]
    fn gemini_base_drops_openai_segment_only() {
        for (raw, want) in [
            (
                GOOGLE_OPENAI,
                "https://generativelanguage.googleapis.com/v1beta/",
            ),
            (
                "https://h.example/v1beta/openai",
                "https://h.example/v1beta/",
            ),
            ("https://h.example/v1beta/", "https://h.example/v1beta/"),
            ("https://h.example/myopenai/", "https://h.example/myopenai/"),
        ] {
            assert_eq!(gemini_base(raw), want, "{raw}");
        }
    }

    #[tokio::test]
    async fn oauth_routing_gateway_chat_is_final() {
        assert_gateway_route("https://gateway.example/v1/", None, AdapterKind::OpenAI).await;
    }

    #[tokio::test]
    async fn oauth_routing_gateway_at_openai_endpoint_is_final() {
        assert_gateway_route("https://api.openai.com/v1/", None, AdapterKind::OpenAI).await;
    }

    #[tokio::test]
    async fn oauth_routing_gateway_responses_is_final() {
        assert_gateway_route(
            "https://gateway.example/v1/",
            Some(Protocol::OpenaiResponses),
            AdapterKind::OpenAIResp,
        )
        .await;
    }

    #[tokio::test]
    async fn oauth_routing_gateway_anthropic_is_final() {
        assert_gateway_route(
            "https://gateway.example/v1/",
            Some(Protocol::Anthropic),
            AdapterKind::Anthropic,
        )
        .await;
    }

    #[tokio::test]
    async fn oauth_routing_zen_responses_is_final() {
        let (_scratch, auth) = oauth_auth().await;
        let resolved = catalog()
            .resolve("opencode/gpt-5.6-sol", &|_| true)
            .unwrap();
        let target = routed_target(&resolved, &auth).await;
        assert_eq!(
            target.endpoint.base_url(),
            ZEN_ENDPOINT,
            "Zen endpoint changed"
        );
        assert_eq!(target.model.adapter_kind, AdapterKind::OpenAIResp);
        assert_eq!(&*target.model.model_name, "gpt-5.6-sol");
        assert!(
            matches!(&target.auth, AuthData::Key(k) if k == "zk"),
            "Zen key replaced: {:?}",
            target.auth
        );
    }

    // Go with the `opencode-go` vault key: Go's base and key, the catalog kind,
    // attributed to the `opencode-go:api_key` limits row; genai's adapter is not used.
    #[tokio::test]
    async fn oauth_routing_go_is_final() {
        let (_scratch, auth) = oauth_auth().await;
        let resolved = catalog()
            .resolve("opencode-go/claude-haiku-5-5", &|_| true)
            .unwrap();
        let target = routed_target(&resolved, &auth).await;
        assert_eq!(
            target.endpoint.base_url(),
            "https://opencode.ai/inference/go/anthropic/v1/"
        );
        assert_eq!(target.model.adapter_kind, AdapterKind::Anthropic);
        assert_eq!(&*target.model.model_name, "claude-haiku-5-5");
        assert!(matches!(&target.auth, AuthData::Key(k) if k == "gk"));
        // The legacy vault name (genai's namespace) still keys it.
        let vault = auth.vault.clone().unwrap();
        vault.delete("opencode-go").await.unwrap();
        vault
            .set("opencode_go", "legacy", None, "test")
            .await
            .unwrap();
        let t = target_for(&resolved, &auth).unwrap();
        assert!(matches!(&t.auth, AuthData::Key(k) if k == "legacy"));
    }

    #[tokio::test]
    async fn oauth_routing_builtin_login_rewrites_remain() {
        let (_scratch, auth) = oauth_auth().await;
        for (id, kind, endpoint, url) in [
            (
                "openai/gpt-5.6-sol",
                AdapterKind::OpenAIResp,
                "https://chatgpt.com/backend-api/codex/",
                "https://chatgpt.com/backend-api/codex/responses",
            ),
            (
                "anthropic/claude-opus-5-5",
                AdapterKind::Anthropic,
                "https://api.anthropic.com/v1/",
                "https://api.anthropic.com/v1/messages?beta=true",
            ),
        ] {
            let resolved = catalog().resolve(id, &|_| true).unwrap();
            assert!(target_for(&resolved, &auth).is_none());
            let target = routed_target(&resolved, &auth).await;
            assert_eq!(target.model.adapter_kind, kind, "{id}");
            assert_eq!(target.endpoint.base_url(), endpoint, "{id}");
            assert!(
                matches!(&target.auth, AuthData::RequestOverride { url: actual, .. } if actual == url),
                "{id}: missing login override: {:?}",
                target.auth
            );
        }
        let resolved = catalog().resolve("xai/grok-9-fast", &|_| true).unwrap();
        let target = routed_target(&resolved, &auth).await;
        assert_eq!(target.model.adapter_kind, AdapterKind::Xai);
        assert!(matches!(&target.auth, AuthData::Key(k) if k == "xai-oauth-token"));
    }

    /// [`oauth_auth`] (every kind's own login in force) plus account instance `name` of
    /// `kind` with `key` as its vault secret, else a login of its own; and a catalog
    /// that knows the instance.
    async fn account_auth(
        kind: &'static oauth::Provider,
        name: &str,
        key: Option<&str>,
    ) -> (crate::TestDir, Auth, models::Catalog) {
        let (scratch, auth) = oauth_auth().await;
        auth.instances.write().unwrap().insert(name.into(), kind);
        match key {
            Some(k) => {
                let vault = auth.vault.as_ref().unwrap();
                vault.set(name, k, None, "test").await.unwrap();
            }
            None => auth
                .oauth
                .save(
                    name,
                    &oauth::Tokens {
                        access_token: format!("{name}-oauth-token"),
                        refresh_token: None,
                        expires_at: 4_102_444_800,
                        account_id: Some("work-account".into()),
                    },
                )
                .await
                .unwrap(),
        }
        let catalog = models::Catalog::load_accounts(
            scratch.path(),
            &|_| false,
            &[(name.to_owned(), kind.ns.to_string())],
        );
        (scratch, auth, catalog)
    }

    /// The routed target of account model `id`, with its limits attribution and
    /// whether it is shaped as a Claude subscription call.
    async fn account_route(
        auth: &Auth,
        catalog: &models::Catalog,
        id: &str,
    ) -> (ServiceTarget, String, &'static str, bool) {
        let resolved = catalog.resolve(id, &|_| true).unwrap();
        assert!(
            target_for(&resolved, auth).is_some(),
            "{id} must never reach genai's inference"
        );
        let shared = crate::supervisor::client(auth.clone()).unwrap();
        let (client, spec, label, limit_auth, subscription) =
            genai_request(&shared, &resolved, auth).await.unwrap();
        let target = client.resolve_service_target(spec).await.unwrap();
        (target, label, limit_auth, subscription)
    }

    fn bearer(auth: &AuthData) -> Option<&str> {
        match auth {
            AuthData::RequestOverride { headers, .. } => headers
                .iter()
                .find(|(k, _)| *k == "Authorization")
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    #[tokio::test]
    async fn account_anthropic_key_routes_to_the_api_with_its_own_key() {
        let (_s, auth, catalog) =
            account_auth(&oauth::ANTHROPIC, "anthropic-work", Some("work-key")).await;
        let (t, label, limit_auth, subscription) =
            account_route(&auth, &catalog, "anthropic-work/claude-opus-5-5").await;
        assert_eq!(t.model.adapter_kind, AdapterKind::Anthropic);
        assert_eq!(t.endpoint.base_url(), ANTHROPIC_API);
        assert_eq!(t.model.model_name.to_string(), "claude-opus-5-5");
        assert!(matches!(&t.auth, AuthData::Key(k) if k == "work-key"));
        assert_eq!(
            (label.as_str(), limit_auth, subscription),
            ("anthropic-work", "api_key", false)
        );
    }

    #[tokio::test]
    async fn account_anthropic_login_is_a_subscription_call_on_its_own_grant() {
        let (_s, auth, catalog) = account_auth(&oauth::ANTHROPIC, "anthropic-work", None).await;
        let (t, label, limit_auth, subscription) =
            account_route(&auth, &catalog, "anthropic-work/claude-opus-5-5").await;
        assert_eq!(t.model.adapter_kind, AdapterKind::Anthropic);
        assert!(matches!(&t.auth, AuthData::RequestOverride { url, .. }
                if url == "https://api.anthropic.com/v1/messages?beta=true"));
        // Its own token, never the kind's (`anthropic-oauth-token`).
        assert_eq!(bearer(&t.auth), Some("Bearer anthropic-work-oauth-token"));
        assert_eq!(
            (label.as_str(), limit_auth, subscription),
            ("anthropic-work", "oauth", true)
        );
    }

    #[tokio::test]
    async fn account_openai_login_routes_to_codex_on_its_own_grant() {
        let (_s, auth, catalog) = account_auth(&oauth::OPENAI, "openai-work", None).await;
        let (t, _, limit_auth, subscription) =
            account_route(&auth, &catalog, "openai-work/gpt-5.6-sol").await;
        assert_eq!(t.model.adapter_kind, AdapterKind::OpenAIResp);
        assert_eq!(t.endpoint.base_url(), CODEX_ENDPOINT);
        assert!(matches!(&t.auth, AuthData::RequestOverride { url, headers }
                if url == "https://chatgpt.com/backend-api/codex/responses"
                    && headers.iter().any(|(k, v)| k == "chatgpt-account-id" && v == "work-account")));
        assert_eq!(bearer(&t.auth), Some("Bearer openai-work-oauth-token"));
        assert_eq!((limit_auth, subscription), ("oauth", false));
    }

    #[tokio::test]
    async fn account_xai_login_sends_its_token_as_the_key() {
        let (_s, auth, catalog) = account_auth(&oauth::XAI, "xai-two", None).await;
        let (t, _, limit_auth, _) = account_route(&auth, &catalog, "xai-two/grok-9-fast").await;
        assert_eq!(t.model.adapter_kind, AdapterKind::Xai);
        assert!(matches!(&t.auth, AuthData::Key(k) if k == "xai-two-oauth-token"));
        assert_eq!(limit_auth, "oauth");
    }

    #[tokio::test]
    async fn account_without_key_or_login_is_refused_not_sent_as_the_kind() {
        let (_s, auth, catalog) = account_auth(&oauth::ANTHROPIC, "anthropic-work", None).await;
        auth.oauth.remove("anthropic-work").await.unwrap();
        let resolved = catalog
            .resolve("anthropic-work/claude-opus-5-5", &|_| true)
            .unwrap();
        let shared = crate::supervisor::client(auth.clone()).unwrap();
        let Err(e) = genai_request(&shared, &resolved, &auth).await else {
            panic!("an account with no credentials must not be sent");
        };
        assert!(e.contains("no API key or login"), "{e}");
    }

    #[tokio::test]
    async fn account_cursor_streams_on_its_own_login() {
        let (_s, auth, catalog) = account_auth(&oauth::CURSOR, "cursor-two", None).await;
        let resolved = catalog.resolve("cursor-two/auto", &|_| true).unwrap();
        let Backend::Cursor { login, .. } =
            Backend::for_model(&resolved, &Client::new().unwrap(), &auth)
        else {
            panic!("a Cursor account streams through the Cursor backend");
        };
        assert_eq!(login.ns, "cursor-two");
        let resolved = catalog.resolve("cursor/auto", &|_| true).unwrap();
        let Backend::Cursor { login, .. } =
            Backend::for_model(&resolved, &Client::new().unwrap(), &auth)
        else {
            panic!("cursor");
        };
        assert_eq!(login.ns, "cursor");
    }

    fn zen(resolved: &Resolved) -> ProtocolTarget {
        protocol_target(resolved, Some("zk".into()), None).unwrap()
    }

    fn go(resolved: &Resolved) -> ProtocolTarget {
        go_target(resolved, "gk".into())
    }

    // Go: the catalog protocol's API family under `inference/go`, else chat
    // completions (Go translates; no slug guess), always the Go key.
    #[test]
    fn go_targets_follow_catalog_protocol_else_chat_completions() {
        let cat = catalog();
        let r = |id: &str| cat.resolve(id, &|_| true).unwrap();
        let openai = "https://opencode.ai/inference/go/openai/v1/";
        for (id, kind, base) in [
            (
                "opencode-go/claude-haiku-5-5",
                AdapterKind::Anthropic,
                "https://opencode.ai/inference/go/anthropic/v1/",
            ),
            ("opencode-go/gpt-6-luna", AdapterKind::OpenAIResp, openai),
            (
                "opencode-go/gemini-4-argon",
                AdapterKind::Gemini,
                "https://opencode.ai/inference/go/google/v1beta/",
            ),
            ("opencode-go/kimi-k3", AdapterKind::OpenAI, openai),
            // genai's own adapter sent this to chat completions; the catalog says messages.
            (
                "opencode-go/qwen4-max",
                AdapterKind::Anthropic,
                "https://opencode.ai/inference/go/anthropic/v1/",
            ),
            // Unknown ids (no catalog protocol): chat completions, whatever the slug.
            ("opencode-go/claude-opus-9", AdapterKind::OpenAI, openai),
            ("opencode-go/gpt-9", AdapterKind::OpenAI, openai),
        ] {
            let t = go(&r(id));
            assert_eq!((t.kind, t.endpoint.base_url()), (kind, base), "{id}");
            assert!(matches!(&t.auth, AuthData::Key(k) if k == "gk"), "{id}");
        }
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
                runtime: None,
                protocol: None,
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
            Transport::of(zen(&r("opencode/claude-opus-5-5")).kind, ZEN_ENDPOINT),
            Transport::Anthropic
        );
        assert_eq!(
            Transport::of(zen(&r("opencode/gpt-5.6-sol")).kind, ZEN_ENDPOINT),
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

    // Go on the wire: each API family's path under `inference/go`, the key as a
    // Bearer on every family (beside Anthropic's and Gemini's own key header),
    // theuth's user agent and the session id; options' own headers kept.
    #[tokio::test]
    async fn go_wire_path_auth_user_agent_and_session_per_family() {
        let server = fake().await;
        let client = Client::new().unwrap();
        let cat = catalog();
        let session = SessionId::new();
        let ua = format!("theuth/{}", platform::version::get());
        for (id, want_path, native) in [
            (
                "opencode-go/claude-haiku-5-5",
                "/inference/go/anthropic/v1/messages",
                Some("x-api-key"),
            ),
            (
                "opencode-go/gpt-6-luna",
                "/inference/go/openai/v1/responses",
                None,
            ),
            (
                "opencode-go/gemini-4-argon",
                "/inference/go/google/v1beta/models/gemini-4-argon:streamGenerateContent",
                Some("x-goog-api-key"),
            ),
            (
                "opencode-go/kimi-k3",
                "/inference/go/openai/v1/chat/completions",
                None,
            ),
        ] {
            let resolved = cat.resolve(id, &|_| true).unwrap();
            let t = go(&resolved);
            let base = t
                .endpoint
                .base_url()
                .replace("https://opencode.ai", &server.uri());
            let spec: ModelSpec = ServiceTarget {
                endpoint: Endpoint::from_owned(base),
                auth: t.auth,
                model: ModelIden::new(t.kind, slug(&resolved).to_owned()),
            }
            .into();
            let opts = ChatOptions::default()
                .with_extra_headers(Headers::from(vec![("x-a", "1".to_owned())]));
            send(&client, spec, &go_headers(session, "gk", opts)).await;
            let reqs = server.received_requests().await.unwrap();
            let req = reqs.last().unwrap();
            let h = |k: &str| req.headers.get(k).map(|v| v.to_str().unwrap().to_owned());
            assert_eq!(req.url.path(), want_path, "{id}");
            assert_eq!(h("authorization").as_deref(), Some("Bearer gk"), "{id}");
            if let Some(native) = native {
                assert_eq!(h(native).as_deref(), Some("gk"), "{id}");
            }
            assert_eq!(h("user-agent").as_deref(), Some(ua.as_str()), "{id}");
            assert_eq!(h("x-opencode-session"), Some(session.to_string()), "{id}");
            assert_eq!(h("x-a").as_deref(), Some("1"), "{id}");
        }
    }

    // Go's `usage.cost` on an Anthropic `message_delta` (genai reads cost there
    // only from Zen's `ping`) becomes the call's provider-reported cost: 0
    // within the plan, not the catalog's list price. A cost genai read wins.
    #[tokio::test]
    async fn go_anthropic_usage_cost_is_the_reported_cost() {
        let sse = [
            r#"{"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2,"cost":0,"opencode":{"billing_source":"go","allowance":0.0012,"list_cost":0.0012}}}"#,
            r#"{"type":"message_stop"}"#,
        ]
        .iter()
        .map(|d| {
            let ty = serde_json::from_str::<serde_json::Value>(d).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned();
            format!("event: {ty}\ndata: {d}\n\n")
        })
        .collect::<String>();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;
        let sink = std::sync::Arc::new(GoCost {
            tap: crate::limits::HeaderTap::new("opencode-go", "api_key", "claude-haiku-5-5"),
            usd: parking_lot::Mutex::new(None),
        });
        let spec: ModelSpec = ServiceTarget {
            endpoint: Endpoint::from_owned(format!("{}/anthropic/v1/", server.uri())),
            auth: AuthData::Key("gk".into()),
            model: ModelIden::new(AdapterKind::Anthropic, "claude-haiku-5-5".to_owned()),
        }
        .into();
        let opts = ChatOptions::default()
            .with_capture_usage(true)
            .with_raw_frame_sink_arc(sink.clone());
        let req = ChatRequest::new(vec![genai::chat::ChatMessage::user("hi")]);
        let mut s = Client::new()
            .unwrap()
            .exec_chat_stream(spec, req, Some(&opts))
            .await
            .unwrap()
            .stream;
        let mut end = None;
        while let Some(ev) = s.next().await {
            if let ChatStreamEvent::End(e) = ev.unwrap() {
                end = Some(e);
            }
        }
        let mut end = end.expect("an End event");
        assert!(end.captured_usage.as_ref().unwrap().cost.is_none());
        sink.fill(&mut end);
        let cost = end.captured_usage.as_ref().unwrap().cost.clone().unwrap();
        assert_eq!(
            (cost.amount, cost.source),
            (0.0, UsageCostSource::ProviderReported)
        );
        // Gemini's field; a frame without a cost leaves the amount alone.
        assert_eq!(
            go_frame_cost(r#"{"usageMetadata":{"totalTokenCount":3,"cost":0.25}}"#),
            Some(0.25)
        );
        assert_eq!(go_frame_cost(r#"{"usage":{"output_tokens":2}}"#), None);
        *sink.usd.lock() = Some(9.0);
        sink.fill(&mut end);
        assert_eq!(end.captured_usage.unwrap().cost.unwrap().amount, 0.0);
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
        let dir = crate::test_dir("backend-429");
        let auth = crate::supervisor::Auth::stub(dir.path());
        auth.gateways.write().unwrap().insert(
            "z160gw".into(),
            Gateway {
                base_url: format!("{}/v1/", server.uri()),
                key: "gk".into(),
                runtime: None,
                protocol: None,
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
                ..Default::default()
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
                    ..Default::default()
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
        let unknown = Resolved {
            model: None,
            ..r.clone()
        };
        assert_eq!(reply_cap(ChatOptions::default(), &unknown).max_tokens, None);
        // 177 E1: an output limit of 0 is unknown — no `max_tokens` at all.
        let mut zero = r;
        zero.model.as_mut().unwrap().limit.output = 0;
        assert_eq!(reply_cap(ChatOptions::default(), &zero).max_tokens, None);
    }
}
