//! 177: local model servers end to end against fakes (wiremock on loopback):
//! no `max_tokens: 0` (E1), thinking off on summary calls (E2), a reply spent
//! on thinking (E3), keyless gateways and runtime names (E4), server windows
//! (E5), the probe (E8) and the summary test call (E9).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use genai::chat::{ChatMessage, ChatStreamEvent};
use parking_lot::Mutex;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;
use crate::backend::Backend;
use crate::endless::compactor::{self, ModelCompactor, Step};
use crate::providers::Gateway;

/// One request the fake saw: its body and its `Authorization` header, if any.
type Seen = Arc<Mutex<Vec<(Value, Option<String>)>>>;

/// An omlx-like OpenAI chat server: pydantic's 422 on `max_tokens: 0`; with
/// `honour` and `chat_template_kwargs.enable_thinking == false` one text
/// line, else only `reasoning_content` until the cap (`finish_reason: length`).
struct Omlx {
    seen: Seen,
    honour: bool,
}

impl Respond for Omlx {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let auth = req
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        self.seen.lock().push((body.clone(), auth));
        if body.get("max_tokens").and_then(Value::as_u64) == Some(0) {
            return ResponseTemplate::new(422).set_body_json(json!({"error": {
                "message": "body -> max_tokens: Input should be greater than 0"}}));
        }
        let off = body.pointer("/chat_template_kwargs/enable_thinking") == Some(&json!(false));
        let cap = body
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(4096);
        let chunk = |delta: Value, finish: Value, usage: Value| {
            let mut c = json!({"id": "c1", "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
            if !usage.is_null() {
                c["usage"] = usage;
            }
            format!("data: {c}\n\n")
        };
        let sse = if self.honour && off {
            chunk(
                json!({"role": "assistant", "content": "The weekly sync is now Thursday 15:00; Dana has the invite."}),
                Value::Null,
                Value::Null,
            ) + &chunk(
                json!({}),
                json!("stop"),
                json!({"prompt_tokens": 300, "completion_tokens": 14}),
            )
        } else {
            chunk(
                json!({"role": "assistant", "reasoning_content": "Hmm, let me think about the sync."}),
                Value::Null,
                Value::Null,
            ) + &chunk(
                json!({}),
                json!("length"),
                json!({"prompt_tokens": 300, "completion_tokens": cap,
                    "completion_tokens_details": {"reasoning_tokens": cap}}),
            )
        };
        ResponseTemplate::new(200).set_body_raw(sse + "data: [DONE]\n\n", "text/event-stream")
    }
}

async fn omlx(honour: bool) -> (MockServer, Seen) {
    let server = MockServer::start().await;
    let seen = Seen::default();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(Omlx {
            seen: seen.clone(),
            honour,
        })
        .mount(&server)
        .await;
    (server, seen)
}

/// Auth with one keyless gateway `name` → `{server}/v1/`.
fn auth_with(dir: &std::path::Path, name: &str, server: &MockServer) -> crate::supervisor::Auth {
    let auth = crate::supervisor::Auth::stub(dir);
    auth.gateways.write().unwrap().insert(
        name.into(),
        Gateway {
            base_url: format!("{}/v1/", server.uri()),
            ..Default::default()
        },
    );
    auth
}

/// `provider/m` with a catalog entry of unknown limits (what a listing writes).
fn zero_model(provider: &str) -> models::Resolved {
    models::Resolved {
        id: format!("{provider}/m"),
        genai: format!("{provider}::m"),
        provider: Some(provider.into()),
        model: Some(models::Model {
            id: "m".into(),
            name: "m".into(),
            limit: models::Limit::default(),
            cost: None,
            reasoning: false,
            tool_call: true,
            image: false,
            reasoning_options: vec![],
            protocol: None,
            canonical: None,
        }),
    }
}

fn compactor_for(model: models::Resolved, auth: &crate::supervisor::Auth) -> ModelCompactor {
    let backend = Backend::for_model(&model, &genai::Client::builder().build().unwrap(), auth);
    let quiet = backend.local_openai(&model);
    let base = crate::backend::target_for(&model, auth).map(|t| t.endpoint.base_url().to_owned());
    ModelCompactor {
        session: runtime::SessionId::new(),
        local: compactor::local_endpoint(&model, base.as_deref()),
        model,
        backend,
        system: compactor::prompt("theuth"),
        effort: None,
        quiet,
        cfg: Default::default(),
    }
}

fn unit() -> Step {
    Step::Unit {
        kind: crate::endless::tree::Kind::Talk,
        text: "Done: the weekly sync is now Thursday 15:00 and Dana has the new invite.".into(),
        earlier: vec![],
        aim: None,
    }
}

/// E1 + E4: a turn-shaped and a title/side-shaped request on a 0/0 model of a
/// keyless local provider carry no `max_tokens` (the fake would 422 a 0); both
/// go through genai's OpenAI adapter to the provider's own endpoint with the
/// placeholder key (`Authorization: Bearer local`).
#[tokio::test]
async fn zero_limits_send_no_max_tokens_and_keyless_sends_placeholder_key() {
    let dir = crate::TestDir::new("local-e1");
    let (server, seen) = omlx(true).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let model = zero_model("mlx");
    let backend = Backend::for_model(&model, &genai::Client::builder().build().unwrap(), &auth);
    let tools = [genai::chat::Tool::new("echo")];
    for tools in [&tools[..], &[]] {
        let history = [ChatMessage::user("hi")];
        let frame = crate::frame::Frame {
            tools,
            system: "sys",
            head: None,
            history: &history,
            vision: false,
            strip_reasoning: false,
            max_tokens: None,
        };
        let (req, opts) = frame.request(
            crate::frame::Transport::OpenAiLike,
            runtime::SessionId::new(),
            None,
        );
        let mut s = backend
            .stream(&model, req, &opts, runtime::SessionId::new())
            .await
            .unwrap();
        let mut ended = false;
        while let Some(ev) = s.next().await {
            ended |= matches!(ev.unwrap(), ChatStreamEvent::End(_));
        }
        assert!(ended);
    }
    let seen = seen.lock();
    assert_eq!(seen.len(), 2);
    for (body, auth) in seen.iter() {
        assert!(body.get("max_tokens").is_none(), "{body}");
        assert_eq!(auth.as_deref(), Some("Bearer local"));
    }
}

/// 177 R1-1: the assumed window sizes memory only. A chat request on a local
/// model whose window is unknown is sent whatever its size (no local refusal,
/// no `max_tokens`); memory budgets size to 16,384; a known small window still
/// refuses locally.
#[tokio::test]
async fn assumed_window_sizes_memory_but_never_refuses() {
    let dir = crate::TestDir::new("local-r1");
    let (server, seen) = omlx(true).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let mut model = zero_model("mlx");
    model.model.as_mut().unwrap().limit.assumed = ASSUMED_CONTEXT;
    assert_eq!(crate::tokens::window(&model), crate::tokens::UNKNOWN_WINDOW);
    assert_eq!(crate::tokens::memory_window(&model), 16_384);
    let b = crate::tokens::budget(
        crate::tokens::Kind::Compactor,
        &model,
        crate::tokens::Limits {
            reply_tokens: u64::from(compactor::LOCAL_MAX_TOKENS),
            ..Default::default()
        },
    );
    assert_eq!(b.target_tokens, 16_384 / 4);
    // ≈ 30k tokens even at 4 B/token: over any 16k ceiling.
    let big = "theuth system prompt and tools. ".repeat(4_000);
    let backend = Backend::for_model(&model, &genai::Client::builder().build().unwrap(), &auth);
    let history = [ChatMessage::user(big.clone())];
    let frame = crate::frame::Frame {
        tools: &[],
        system: "sys",
        head: None,
        history: &history,
        vision: false,
        strip_reasoning: false,
        max_tokens: None,
    };
    let (req, opts) = frame.request(
        crate::frame::Transport::OpenAiLike,
        runtime::SessionId::new(),
        None,
    );
    let mut s = backend
        .stream(&model, req, &opts, runtime::SessionId::new())
        .await
        .expect("an unknown window never refuses");
    while s.next().await.is_some() {}
    assert_eq!(seen.lock().len(), 1);
    assert!(seen.lock()[0].0.get("max_tokens").is_none());

    // A known (server- or user-set) small window refuses before sending.
    let mut known = model.clone();
    known.model.as_mut().unwrap().limit.context = 8_192;
    let (req, opts) = frame.request(
        crate::frame::Transport::OpenAiLike,
        runtime::SessionId::new(),
        None,
    );
    let e = match backend
        .stream(&known, req, &opts, runtime::SessionId::new())
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("a known 8k window must refuse a 30k-token request"),
    };
    assert!(e.starts_with("prompt is too long:"), "{e}");
    assert_eq!(seen.lock().len(), 1, "nothing sent");
}

/// E4: a local provider named `omlx` shadows genai's omlx adapter — `omlx/m`
/// goes to the provider's URL; with no provider of that name genai resolves it.
#[test]
fn local_provider_named_omlx_shadows_the_adapter() {
    let dir = crate::TestDir::new("local-e4");
    let auth = crate::supervisor::Auth::stub(&dir.0);
    let model = zero_model("omlx");
    assert!(crate::backend::target_for(&model, &auth).is_none());
    auth.gateways.write().unwrap().insert(
        "omlx".into(),
        Gateway {
            base_url: "http://127.0.0.1:18080/v1/".into(),
            ..Default::default()
        },
    );
    let t = crate::backend::target_for(&model, &auth).unwrap();
    assert_eq!(t.endpoint.base_url(), "http://127.0.0.1:18080/v1/");
    assert_eq!(t.kind, genai::adapter::AdapterKind::OpenAI);
    assert!(matches!(
        &t.auth,
        genai::resolver::AuthData::Key(k) if k == crate::backend::KEYLESS_PLACEHOLDER
    ));
}

/// E2: a local OpenAI-protocol provider's summary call carries
/// `chat_template_kwargs.enable_thinking: false` and no effort keyword; a
/// cloud gateway's request does not.
#[tokio::test]
async fn local_summary_call_switches_thinking_off_by_chat_template() {
    let dir = crate::TestDir::new("local-e2");
    let (server, seen) = omlx(true).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let c = compactor_for(zero_model("mlx"), &auth);
    let built = compactor::build(
        &c,
        "<chat>\n</chat>",
        &unit(),
        512,
        3,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(built.tries > 0, "{built:?}");
    let seen = seen.lock();
    let body = &seen[0].0;
    assert_eq!(
        body["chat_template_kwargs"],
        json!({"enable_thinking": false})
    );
    assert!(body.get("reasoning_effort").is_none(), "{body}");
    assert_eq!(body["max_tokens"], compactor::LOCAL_MAX_TOKENS);

    // A cloud custom provider (not local by URL or preset): no kwargs, 600.
    auth.gateways.write().unwrap().insert(
        "rwx".into(),
        Gateway {
            base_url: "https://api.rwx.example/v1/".into(),
            key: "k".into(),
            runtime: None,
        },
    );
    let mut cloud = zero_model("rwx");
    cloud.model.as_mut().unwrap().cost = Some(models::Cost {
        input: 1.0,
        output: 1.0,
        cache_read: None,
        cache_write: None,
    });
    let c = compactor_for(cloud, &auth);
    assert!(!c.quiet && c.local.is_none());
    let (_, opts) = c.request(crate::frame::Transport::OpenAiLike, "ctx", &[]);
    assert!(opts.extra_body.is_none());
    assert_eq!(opts.max_tokens, Some(compactor::MAX_TOKENS));
}

/// E3: a model that thinks through the cap whatever it is told — one forced
/// retry at twice the cap, then the "whole reply on thinking" error; a model
/// that honours the kwargs answers on the first call.
#[tokio::test]
async fn reply_spent_on_thinking_retries_once_at_twice_the_cap() {
    let dir = crate::TestDir::new("local-e3");
    let (server, seen) = omlx(false).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let c = compactor_for(zero_model("mlx"), &auth);
    let f = compactor::build(
        &c,
        "<chat>\n</chat>",
        &unit(),
        512,
        5,
        Duration::from_secs(30),
    )
    .await
    .unwrap_err();
    assert!(
        f.error.starts_with(
            "mlx/m used its whole 4096-token reply on thinking (finish_reason=length)"
        ),
        "{}",
        f.error
    );
    assert!(f.error.ends_with(compactor::THINKING_HINT));
    assert_eq!(
        f.calls.len(),
        2,
        "one call and one forced retry, not five tries"
    );
    let caps: Vec<u64> = seen
        .lock()
        .iter()
        .map(|(b, _)| b["max_tokens"].as_u64().unwrap())
        .collect();
    assert_eq!(caps, [2048, 4096]);

    let (server, seen) = omlx(true).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let c = compactor_for(zero_model("mlx"), &auth);
    let b = compactor::build(
        &c,
        "<chat>\n</chat>",
        &unit(),
        512,
        5,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(b.tries == 1 && b.text.contains("Thursday"), "{b:?}");
    assert_eq!(seen.lock().len(), 1);
}

/// R2-4: the forced retry stays within the model's output limit; when it
/// cannot grow the cap, the thinking error comes at once (one call).
#[tokio::test]
async fn forced_retry_never_exceeds_the_output_limit() {
    let dir = crate::TestDir::new("local-r24");
    let (server, seen) = omlx(false).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let mut model = zero_model("mlx");
    model.model.as_mut().unwrap().limit.output = 2048;
    let c = compactor_for(model, &auth);
    let f = compactor::build(
        &c,
        "<chat>\n</chat>",
        &unit(),
        512,
        5,
        Duration::from_secs(30),
    )
    .await
    .unwrap_err();
    assert!(
        f.error
            .contains("used its whole 2048-token reply on thinking"),
        "{}",
        f.error
    );
    assert_eq!(seen.lock().len(), 1, "no retry that could not ask for more");
    // With room (output 3000), the retry asks for 3000, not 4096.
    let (server, seen) = omlx(false).await;
    let auth = auth_with(&dir.0, "mlx", &server);
    let mut model = zero_model("mlx");
    model.model.as_mut().unwrap().limit.output = 3000;
    let c = compactor_for(model, &auth);
    compactor::build(
        &c,
        "<chat>\n</chat>",
        &unit(),
        512,
        5,
        Duration::from_secs(30),
    )
    .await
    .unwrap_err();
    let caps: Vec<u64> = seen
        .lock()
        .iter()
        .map(|(b, _)| b["max_tokens"].as_u64().unwrap())
        .collect();
    assert_eq!(caps, [2048, 3000]);
}

/// R2-5: a LAN gateway (private host, no preset) in front of cloud models is
/// local for the tag and the optional key, but gets no thinking kwargs; a
/// preset or a loopback host does.
#[test]
fn lan_gateway_is_local_but_gets_no_thinking_kwargs() {
    let dir = crate::TestDir::new("local-r25");
    let auth = crate::supervisor::Auth::stub(&dir.0);
    let lan = Gateway {
        base_url: "http://10.0.0.5:4000/v1/".into(),
        key: "sk".into(),
        runtime: None,
    };
    assert!(lan.local() && !lan.on_this_machine());
    auth.gateways
        .write()
        .unwrap()
        .insert("lan".into(), lan.clone());
    let c = compactor_for(zero_model("lan"), &auth);
    assert!(!c.quiet);
    let (_, opts) = c.request(crate::frame::Transport::OpenAiLike, "ctx", &[]);
    assert!(opts.extra_body.is_none());
    for g in [
        Gateway {
            runtime: Some(LocalRuntime::Vllm),
            ..lan.clone()
        },
        Gateway {
            base_url: "http://localhost:8000/v1/".into(),
            ..lan
        },
    ] {
        assert!(g.on_this_machine(), "{g:?}");
        auth.gateways.write().unwrap().insert("lan".into(), g);
        assert!(compactor_for(zero_model("lan"), &auth).quiet);
    }
}

/// Answer `GET p` on `server` with `body`.
async fn serve(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p.to_owned()))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// E5: each server shape's window, read at listing time.
#[tokio::test]
async fn server_windows_by_runtime_shape() {
    let ids = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    // vLLM: `max_model_len` in /v1/models.
    let vllm = MockServer::start().await;
    serve(
        &vllm,
        "/v1/models",
        json!({"data": [{"id": "a", "max_model_len": 32768, "owned_by": "vllm"}]}),
    )
    .await;
    // LM Studio: /api/v0/models — a loaded model's window; an unloaded one's
    // `max_context_length` is the architecture maximum, not stored (R2-2).
    let lms = MockServer::start().await;
    serve(
        &lms,
        "/v1/models",
        json!({"data": [{"id": "b"}, {"id": "b2"}]}),
    )
    .await;
    serve(
        &lms,
        "/api/v0/models",
        json!({"data": [
            {"id": "b", "max_context_length": 131072, "loaded_context_length": 8192},
            {"id": "b2", "max_context_length": 131072}
        ]}),
    )
    .await;
    // llama.cpp: /props, one window for every id.
    let llama = MockServer::start().await;
    serve(&llama, "/v1/models", json!({"data": [{"id": "c"}]})).await;
    serve(
        &llama,
        "/props",
        json!({"default_generation_settings": {"n_ctx": 4096}}),
    )
    .await;
    // Ollama (R2-2): `num_ctx` from /api/show `parameters`, else /api/ps for a
    // loaded model, else unknown — never `model_info`'s architecture maximum.
    let ollama = MockServer::start().await;
    serve(
        &ollama,
        "/v1/models",
        json!({"data": [{"id": "tuned:1b"}, {"id": "loaded:3b"}, {"id": "cold:7b"}]}),
    )
    .await;
    serve(
        &ollama,
        "/api/tags",
        json!({"models": [{"name": "tuned:1b"}, {"name": "loaded:3b"}, {"name": "cold:7b"}]}),
    )
    .await;
    serve(
        &ollama,
        "/api/ps",
        json!({"models": [{"name": "loaded:3b", "context_length": 2048}]}),
    )
    .await;
    let info = json!({"general.architecture": "llama", "llama.context_length": 131072});
    for (model, show) in [
        (
            "tuned:1b",
            json!({"parameters": "num_ctx                        4096\nstop \"<|eot|>\"", "model_info": info}),
        ),
        ("loaded:3b", json!({"model_info": info})),
        ("cold:7b", json!({"model_info": info})),
    ] {
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .and(wiremock::matchers::body_partial_json(
                json!({"model": model}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(show))
            .mount(&ollama)
            .await;
    }
    let all = |server: &MockServer, s: &[&str]| {
        let base = format!("{}/v1/", server.uri());
        let ids = ids(s);
        async move { server_contexts(&base, "", true, &ids).await }
    };
    assert_eq!(all(&vllm, &["a"]).await.get("a"), Some(&32768));
    let got = all(&lms, &["b", "b2"]).await;
    assert_eq!((got.get("b"), got.get("b2")), (Some(&8192), None));
    assert_eq!(all(&llama, &["c"]).await.get("c"), Some(&4096));
    let got = all(&ollama, &["tuned:1b", "loaded:3b", "cold:7b"]).await;
    assert_eq!(
        (
            got.get("tuned:1b"),
            got.get("loaded:3b"),
            got.get("cold:7b")
        ),
        (Some(&4096), Some(&2048), None)
    );
    // A cloud provider is asked /v1/models only.
    let got = server_contexts(&format!("{}/v1/", llama.uri()), "k", false, &ids(&["c"])).await;
    assert!(got.is_empty(), "{got:?}");
    // R2-8: a slow endpoint keeps the windows already read.
    let slow = MockServer::start().await;
    serve(
        &slow,
        "/v1/models",
        json!({"data": [{"id": "s", "max_model_len": 9000}]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&slow)
        .await;
    assert_eq!(all(&slow, &["s"]).await.get("s"), Some(&9000));
}

/// R2-1: neither the probe nor the window reader follows a redirect to
/// another host.
#[tokio::test]
async fn redirects_are_not_followed() {
    let elsewhere = MockServer::start().await;
    let hit = Seen::default();
    let seen = hit.clone();
    Mock::given(method("GET"))
        .respond_with(move |r: &Request| {
            seen.lock().push((json!(r.url.path()), None));
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [{"id": "x", "max_model_len": 1}], "models": []}))
        })
        .mount(&elsewhere)
        .await;
    let redirecting = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/v1/models", elsewhere.uri())),
        )
        .mount(&redirecting)
        .await;
    let found = probe(&[redirecting.address().port()], &[]).await;
    assert!(found[0].reachable && found[0].models == 0, "{found:?}");
    let got = server_contexts(
        &format!("{}/v1/", redirecting.uri()),
        "",
        true,
        &["x".to_owned()],
    )
    .await;
    assert!(got.is_empty(), "{got:?}");
    assert!(hit.lock().is_empty(), "the other host was never asked");
}

/// E8: the probe on injected ports names each runtime by what answers, marks
/// an added one, and reports a closed port unreachable within the bound.
#[tokio::test]
async fn probe_identifies_runtimes_and_added_as() {
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    let ollama = MockServer::start().await;
    Mock::given(path("/api/tags"))
        .respond_with(ok(json!({"models": [{"name": "a"}, {"name": "b"}]})))
        .mount(&ollama)
        .await;
    let lms = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ok(json!({"data": [{"id": "x"}]})))
        .mount(&lms)
        .await;
    Mock::given(path("/api/v0/models"))
        .respond_with(ok(json!({"data": [{"id": "x"}]})))
        .mount(&lms)
        .await;
    let llama = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ok(json!({"data": [{"id": "g"}]})))
        .mount(&llama)
        .await;
    Mock::given(path("/props"))
        .respond_with(ok(json!({"default_generation_settings": {"n_ctx": 4096}})))
        .mount(&llama)
        .await;
    let vllm = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ok(json!({"data": [{"id": "q", "max_model_len": 8192}]})))
        .mount(&vllm)
        .await;
    let plain = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ok(json!({"data": [{"id": "p", "owned_by": "me"}]})))
        .mount(&plain)
        .await;
    let keyed = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"detail": "API key required"})),
        )
        .mount(&keyed)
        .await;
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let port = |s: &MockServer| s.address().port();
    let ports = [
        port(&ollama),
        port(&lms),
        port(&llama),
        port(&vllm),
        port(&plain),
        closed,
        port(&keyed),
    ];
    let added = vec![(
        "ollama".to_owned(),
        format!("http://localhost:{}/v1/", port(&ollama)),
    )];
    let started = std::time::Instant::now();
    let found = probe(&ports, &added).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let got: Vec<(LocalRuntime, bool, u32, Option<&str>)> = found
        .iter()
        .map(|s| (s.runtime, s.reachable, s.models, s.added_as.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            (LocalRuntime::Ollama, true, 2, Some("ollama")),
            (LocalRuntime::Lmstudio, true, 1, None),
            (LocalRuntime::Llamacpp, true, 1, None),
            (LocalRuntime::Vllm, true, 1, None),
            (LocalRuntime::Openai, true, 1, None),
            (LocalRuntime::Openai, false, 0, None),
            (LocalRuntime::Openai, true, 0, None),
        ]
    );
    assert_eq!(
        found[1].base_url,
        format!("http://127.0.0.1:{}/v1/", port(&lms))
    );
    assert_eq!(found[5].error.as_deref(), Some("connection refused"));
    assert_eq!(
        (
            found[6].reachable,
            found[6].models,
            found[6].error.as_deref()
        ),
        (true, 0, Some("answered 401: needs an API key")),
        "omlx with key verification on"
    );
}

/// The host rule every locality decision uses; the desktop UI mirrors these
/// vectors (177 notes, decision 23).
#[test]
fn private_url_vectors() {
    for (url, want) in [
        ("HTTP://LOCALHOST:8080/V1/", true),
        ("http://u:p@127.0.0.1:8080/v1/", true),
        ("http://[::]:8000/v1/", true),
        ("http://0.0.0.0:8000/v1/", true),
        ("http://[::ffff:7f00:1]:8000/v1/", true),
        ("http://[::1]:8000/v1/", true),
        ("http://127.0.0.1:8000/v1/", true),
        ("http://10.1.2.3/v1/", true),
        ("http://172.16.0.1/v1/", true),
        ("http://172.31.255.255/v1/", true),
        ("http://172.32.0.1/v1/", false),
        ("http://192.168.1.5/v1/", true),
        ("http://100.64.0.1/v1/", true),
        ("http://100.127.255.255/v1/", true),
        ("http://100.128.0.1/v1/", false),
        ("http://169.254.1.1/v1/", true),
        ("http://[fd00::1]/v1/", true),
        ("http://[fe80::1]/v1/", true),
        ("http://[2001:db8::1]/v1/", false),
        ("http://8.8.8.8/v1/", false),
        ("http://gpu-box/v1/", true),
        ("http://studio.local:1234/v1/", true),
        ("http://studio.LAN/v1/", true),
        ("http://foo.localhost/v1/", true),
        ("http://foo.internal/v1/", true),
        ("http://nas.home.arpa/v1/", true),
        ("http://mac.tail1234.ts.net/v1/", true),
        ("https://api.openai.com/v1/", false),
        ("https://local.example.com/v1/", false),
        ("https://evil.local.example.com/v1/", false),
        ("not a url", false),
    ] {
        assert_eq!(private_url(url), want, "{url}");
    }
}

/// E9: the summary test call — ok on a model that honours the kwargs, the
/// thinking error with its hint on one that does not; charged to the test owner.
#[tokio::test]
async fn test_summary_call_ok_and_thinking_failure() {
    for honour in [true, false] {
        let dir = crate::TestDir::new("local-e9");
        let (server, _seen) = omlx(honour).await;
        let auth = auth_with(&dir.0, "omlx", &server);
        let store = crate::store::Store::open(&dir.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let endless = crate::endless::Endless::new(
            store,
            runtime::Runtime::new(figment::Figment::new(), dir.0.clone()),
            genai::Client::builder().build().unwrap(),
            auth,
            Arc::new(models::Catalog::load(&dir.0)),
            crate::endless::EndlessConfig::default(),
            None,
        );
        let t = endless.test_summary("omlx/m").await;
        assert_eq!(t.model, "omlx/m");
        assert!(t.thinking_off_sent, "{t:?}");
        if honour {
            assert!(t.ok, "{t:?}");
            assert!(t.line.as_deref().is_some_and(|l| l.contains("Thursday")));
            assert_eq!(t.finish_reason.as_deref(), Some("stop"));
            assert_eq!((t.tokens_in, t.tokens_out), (Some(300), Some(14)));
            assert!(t.error.is_none() && t.hint.is_none());
        } else {
            assert!(!t.ok);
            assert!(
                t.error
                    .as_deref()
                    .is_some_and(|e| e.contains("reply on thinking")),
                "{t:?}"
            );
            assert_eq!(t.hint.as_deref(), Some(compactor::THINKING_HINT));
            assert_eq!(t.finish_reason.as_deref(), Some("length"));
            assert!(t.reasoning_seen);
            assert_eq!(t.reasoning_tokens, Some(4096));
        }
    }
    // A server that is not running (a closed loopback port): an error and the hint.
    let dir = crate::TestDir::new("local-e9b");
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let auth = crate::supervisor::Auth::stub(&dir.0);
    auth.gateways.write().unwrap().insert(
        "omlx".into(),
        Gateway {
            base_url: format!("http://127.0.0.1:{closed}/v1/"),
            ..Default::default()
        },
    );
    let store = crate::store::Store::open(&dir.0.join("sessions.sqlite"), None)
        .await
        .unwrap();
    let endless = crate::endless::Endless::new(
        store,
        runtime::Runtime::new(figment::Figment::new(), dir.0.clone()),
        genai::Client::builder().build().unwrap(),
        auth,
        Arc::new(models::Catalog::load(&dir.0)),
        crate::endless::EndlessConfig::default(),
        None,
    );
    let t = endless.test_summary("omlx/m").await;
    assert!(!t.ok && t.error.is_some(), "{t:?}");
    assert_eq!(
        t.hint.as_deref(),
        Some("The server did not answer: is it running at the provider's URL?"),
        "{t:?}"
    );
}
