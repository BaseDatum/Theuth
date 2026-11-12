//! 70 THINKING-TRACES: what a main round sends Anthropic for each effort,
//! captured at a fake `/v1/messages` (wiremock), and that the thinking block
//! a tool-use turn returns is replayed with its signature on the next round.
//! The request bodies are the evidence, exactly as the loop builds them
//! ([`Frame::request`] with [`crate::frame::default_effort`] filling `None`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage, ChatStreamEvent, ContentPart, MessageContent, ReasoningEffort, StreamEnd,
    ThoughtOrigin, ThoughtSignature, Tool, ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use parking_lot::Mutex;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;
use crate::frame::default_effort;

const THINKING: &str = "Let me think about the square.";
const SIGNATURE: &str = "sig-deadbeef";
/// `output_tokens` the fake reports: Anthropic counts thinking inside it.
const OUTPUT_TOKENS: u32 = 57;
/// The fake: records every request body, answers the first `reject` requests
/// with Anthropic's rejected-signature 400 (90 §0), then each with one signed
/// thinking block followed by one `tool_use` (a tool-use turn), then `message_stop`.
/// With `text`, a text block follows the thinking instead of the tool call
/// (91: a plain reply ends the thinking); with `silent`, no thinking block
/// streams at all. The thinking always streams as two deltas.
struct Fake {
    bodies: Arc<Mutex<Vec<Value>>>,
    reject: AtomicUsize,
    text: bool,
    silent: bool,
}

/// 90 §0: the body Anthropic answers a foreign or unverifiable signature with.
const REJECTED: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.3.content.0: Invalid `signature` in `thinking` block"}}"#;

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.bodies
            .lock()
            .push(serde_json::from_slice(&req.body).unwrap());
        if self
            .reject
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return ResponseTemplate::new(400).set_body_raw(REJECTED, "application/json");
        }
        let ev = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
        // Two deltas (91: the measurement starts at the first, not the last).
        let (thinking, second) = THINKING.split_at(THINKING.len() / 2);
        let reply = if self.text {
            [
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Four."}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 1}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": OUTPUT_TOKENS}}),
                ),
            ]
        } else {
            [
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "echo", "input": {}}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"x\":1}"}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 1}),
                ),
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": OUTPUT_TOKENS}}),
                ),
            ]
        };
        let thought = if self.silent {
            Vec::new()
        } else {
            vec![
                ev(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": thinking}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": second}}),
                ),
                ev(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": SIGNATURE}}),
                ),
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
            ]
        };
        let body = [ev(
            "message_start",
            json!({"type": "message_start", "message": {"id": "m", "role": "assistant", "content": [],
                "usage": {"input_tokens": 12, "output_tokens": 1}}}),
        )]
        .into_iter()
        .chain(thought)
        .chain(reply)
        .chain([ev("message_stop", json!({"type": "message_stop"}))])
        .collect::<Vec<_>>()
        .concat();
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }
}

struct Harness {
    bodies: Arc<Mutex<Vec<Value>>>,
    client: Client,
    catalog: models::Catalog,
    dir: crate::TestDir,
    _server: MockServer,
}

impl Harness {
    async fn new() -> Self {
        Self::build(0, false, false).await
    }

    /// A harness whose fake answers the first `reject` requests with [`REJECTED`].
    async fn rejecting(reject: usize) -> Self {
        Self::build(reject, false, false).await
    }

    /// A harness whose fake replies with text after the thinking (91).
    async fn texting() -> Self {
        Self::build(0, true, false).await
    }

    /// A harness whose fake replies with text and no thinking at all (91).
    async fn plain() -> Self {
        Self::build(0, true, true).await
    }

    async fn build(reject: usize, text: bool, silent: bool) -> Self {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(Fake {
                bodies: bodies.clone(),
                reject: AtomicUsize::new(reject),
                text,
                silent,
            })
            .mount(&server)
            .await;
        let endpoint = format!("{}/v1/", server.uri());
        let client = Client::builder()
            .with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
                move |mut t: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
                    t.endpoint = Endpoint::from_owned(endpoint.clone());
                    t.auth = AuthData::Key("x".into());
                    Ok(t)
                },
            ))
            .build()
            .unwrap();
        let dir = crate::TestDir::new("thinking");
        Self {
            bodies,
            client,
            catalog: models::Catalog::load(&dir.0),
            dir,
            _server: server,
        }
    }

    fn resolve(&self, id: &str) -> Resolved {
        self.catalog.resolve(id, &|_| true).unwrap()
    }

    /// One main-round request for `history`, as `stream_once` builds it: the
    /// session's `effort` else the model's default. Returns the captured body
    /// and the stream's end.
    async fn round(
        &self,
        model: &Resolved,
        effort: Option<ReasoningEffort>,
        history: &[ChatMessage],
    ) -> (Value, StreamEnd) {
        let tool = Tool::new("echo").with_schema(json!({"type": "object"}));
        let frame = Frame {
            tools: std::slice::from_ref(&tool),
            system: "sys",
            head: None,
            history,
            vision: false,
            strip_reasoning: false,
            max_tokens: None,
        };
        let effort = effort.or_else(|| default_effort(model));
        let (req, opts) = frame.request(Transport::Anthropic, SessionId::new(), effort);
        let mut stream = self
            .client
            .exec_chat_stream(model.genai.as_str(), req, Some(&opts))
            .await
            .unwrap()
            .stream;
        let mut end = None;
        while let Some(ev) = stream.next().await {
            if let ChatStreamEvent::End(e) = ev.unwrap() {
                end = Some(e);
            }
        }
        let body = self.bodies.lock().pop().unwrap();
        (body, end.unwrap())
    }

    /// `Session::call` for one main round of `history` on `model`, through the
    /// loop's retry path, with the session's ledger rows
    /// `(call_id, request_id, kind, status)` by attempt.
    async fn turn_call(
        &self,
        model: Resolved,
        history: &[ChatMessage],
    ) -> (
        Result<StreamEnd, Abort>,
        Vec<(String, String, String, String)>,
    ) {
        let c = self.live_call(model, history).await;
        (c.end, c.rows)
    }

    /// [`Harness::turn_call`] as a live turn: every `AgentEvent` the call
    /// published, in order, and the thinking time it measured (91).
    async fn live_call(&self, model: Resolved, history: &[ChatMessage]) -> Called {
        let store = Store::open_memory().await.unwrap();
        let id = SessionId::new();
        store
            .create_session(id, &model.id, None, None, None)
            .await
            .unwrap();
        let rt = Runtime::new(figment::Figment::new(), self.dir.0.clone());
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let sink = kameo::actor::Spawn::spawn_with_mailbox(
            super::vision_fallback_tests::Sink(tx),
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<super::vision_fallback_tests::Sink, AgentEvent>(&sink)
            .await;
        let mut session = Session::new(
            id,
            0,
            (model, None),
            None,
            SessionState::resume(0, None, vec![]),
            Arc::new(AgentConfig::default()),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            store.clone(),
            self.client.clone(),
            Auth::stub(&self.dir.0),
            rt,
            Arc::new(models::Catalog::load(&self.dir.0)),
            None,
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&self.dir.0),
        );
        let mut turn = Turn::new(
            session.shared.clone(),
            Some(TurnId::new()),
            CancellationToken::new(),
            session.state.take().unwrap(),
        );
        let (current, transport) = turn.resolve().await.unwrap();
        let tool = Tool::new("echo").with_schema(json!({"type": "object"}));
        let frame = Frame {
            tools: std::slice::from_ref(&tool),
            system: "sys",
            head: None,
            history,
            vision: false,
            strip_reasoning: false,
            max_tokens: None,
        };
        let mut thinking = None;
        let end = turn
            .call(
                &current,
                frame,
                transport,
                turn.turn,
                id,
                CallKind::Round,
                &mut thinking,
            )
            .await;
        let rows = sqlx::query_as(
            "SELECT call_id, request_id, kind, status FROM model_calls WHERE owner_id = ? ORDER BY attempt",
        )
        .bind(id.to_string())
        .fetch_all(store.pool())
        .await
        .unwrap();
        // The bus delivers asynchronously; a marker published after the call
        // bounds what it emitted.
        turn.publish(AgentEvent::Error {
            session: id,
            turn: turn.turn,
            message: "marker".into(),
        })
        .await;
        let mut published = Vec::new();
        while let Some(ev) = events.recv().await {
            if matches!(&ev, AgentEvent::Error { message, .. } if message == "marker") {
                break;
            }
            published.push(ev);
        }
        Called {
            end,
            rows,
            thinking,
            events: published,
            turn,
            store,
            id,
        }
    }
}

/// What [`Harness::live_call`] observed.
struct Called {
    end: Result<StreamEnd, Abort>,
    rows: Vec<(String, String, String, String)>,
    thinking: Option<u64>,
    events: Vec<AgentEvent>,
    turn: Turn,
    store: Store,
    id: SessionId,
}

/// 90 §1.5: a stored signature of `kind` over connection `connection`, as a
/// capture tags it. The loop labels an `anthropic/*` call `anthropic`
/// ([`crate::backend::connection_label`]), so that pair is the one Anthropic
/// replays; anything else is dropped before the request leaves (90 §1.2).
fn sig_from(kind: AdapterKind, connection: &str, s: &str) -> ContentPart {
    ContentPart::ThoughtSignature(ThoughtSignature::new(s).with_origin(ThoughtOrigin::new(
        &ModelIden::new(kind, "claude-x"),
        Some(connection),
    )))
}

/// A stored tool-use turn — user, assistant (signature + reasoning + tool
/// call), tool result — whose thinking block carries `signature`.
fn thinking_history(signature: ContentPart) -> Vec<ChatMessage> {
    let mut history = user();
    history.push(ChatMessage::assistant(MessageContent::from_parts(vec![
        signature,
        ContentPart::ReasoningContent("summary from before".into()),
        ContentPart::ToolCall(ToolCall {
            call_id: "toolu_1".into(),
            fn_name: "echo".into(),
            fn_arguments: json!({"x": 1}),
            thought_signatures: None,
        }),
    ])));
    history.push(ChatMessage::from(ToolResponse::new("toolu_1", "ok")));
    history
}

/// The block Anthropic itself issued to this connection: replayed, then
/// refused by the fake (a prefix-binding or verification failure).
fn foreign_thinking_history() -> Vec<ChatMessage> {
    thinking_history(sig_from(AdapterKind::Anthropic, "anthropic", "sig-refused"))
}

fn user() -> Vec<ChatMessage> {
    vec![ChatMessage::user("hi")]
}

#[test]
fn default_effort_is_medium_for_anthropic_thinking_models_only() {
    let cat = models::Catalog::load(std::path::Path::new("/nonexistent"));
    let r = |id: &str| cat.resolve(id, &|_| true).unwrap();
    for id in [
        "anthropic/claude-fable-5-1",
        "anthropic/claude-opus-4-8",
        "anthropic/claude-sonnet-4-5",
        "anthropic/claude-haiku-4-5",
        "amazon-bedrock/us.anthropic.claude-fable-5-1",
        "amazon-bedrock/anthropic.claude-opus-4-8",
        "amazon-bedrock/global.anthropic.claude-sonnet-4-5-20250929-v1:0",
        // 85 §0.4: Anthropic by canonical id or `claude-` slug under any provider.
        "opencode/claude-opus-5-5",
        "openrouter/anthropic/claude-opus-5.5",
    ] {
        assert!(
            matches!(default_effort(&r(id)), Some(ReasoningEffort::Medium)),
            "{id}"
        );
    }
    // Other providers keep their own default; a non-thinking or unknown model gets nothing.
    for id in [
        "openai/gpt-5.6-sol",
        "amazon-bedrock/amazon.nova-pro-v1:0",
        "amazon-bedrock/us.openai.gpt-6-astra",
        "google/gemini-2.5-pro",
        "opencode/gpt-5.6-sol",
        "openrouter/openai/gpt-5.6-sol",
    ] {
        assert!(default_effort(&r(id)).is_none(), "{id}");
    }
    let mut plain = r("anthropic/claude-fable-5-1");
    plain.model.as_mut().unwrap().reasoning = false;
    assert!(default_effort(&plain).is_none());
    plain.model = None;
    assert!(default_effort(&plain).is_none());
    // Non-Anthropic request options carry no effort at all when the session has none.
    let (_, opts) = Frame {
        tools: &[],
        system: "sys",
        head: None,
        history: &user(),
        vision: false,
        strip_reasoning: false,
        max_tokens: None,
    }
    .request(
        Transport::OpenAiLike,
        SessionId::new(),
        default_effort(&r("openai/gpt-5.6-sol")),
    );
    assert!(opts.reasoning_effort.is_none());
}

/// 137G R2-8: what an endless compactor call sends Anthropic, on the wire.
/// Haiku 5.5 (thinks by default, accepts `disabled`): `thinking: disabled`.
/// Fable 5.1 (rejects `disabled`, thinks adaptively when nothing is sent):
/// adaptive thinking at the lowest effort. Both at the 600-token cap.
#[tokio::test]
async fn endless_compactor_turns_thinking_off_per_model() {
    use crate::endless::compactor::{MAX_TOKENS, ModelCompactor, prompt, thinking_off};
    let h = Harness::texting().await;
    let mut bodies = Vec::new();
    for id in ["anthropic/claude-haiku-5-5", "anthropic/claude-fable-5-1"] {
        let model = h.resolve(id);
        let c = ModelCompactor {
            session: SessionId::new(),
            backend: crate::backend::Backend::for_model(
                &model,
                &h.client,
                &crate::supervisor::Auth::stub(&h.dir.0),
            ),
            effort: thinking_off(&model),
            chosen: false,
            model: model.clone(),
            system: prompt("theuth"),
            local: None,
            quiet: false,
            cfg: Default::default(),
        };
        let step = [ChatMessage::user(
            "Compress this message into one line, in at most 512 bytes:\nhello",
        )];
        let (req, opts) = c.request(Transport::Anthropic, "<chat>\n</chat>", &step);
        let mut stream = h
            .client
            .exec_chat_stream(model.genai.as_str(), req, Some(&opts))
            .await
            .unwrap()
            .stream;
        while let Some(ev) = stream.next().await {
            ev.unwrap();
        }
        bodies.push(h.bodies.lock().pop().unwrap());
    }
    let [haiku, fable] = &bodies[..] else {
        unreachable!()
    };
    assert_eq!(haiku["thinking"], json!({"type": "disabled"}), "{haiku}");
    assert!(
        haiku
            .get("output_config")
            .is_none_or(|o| o.get("effort").is_none()),
        "{haiku}"
    );
    // R3-2: no reasoning capture, so no `display: "summarized"`.
    assert_eq!(fable["thinking"], json!({"type": "adaptive"}), "{fable}");
    assert_eq!(fable["output_config"]["effort"], "low", "{fable}");
    for b in [haiku, fable] {
        assert_eq!(b["max_tokens"], MAX_TOKENS, "{b}");
    }
}

#[tokio::test]
async fn anthropic_request_carries_thinking_per_effort() {
    let h = Harness::new().await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let sonnet45 = h.resolve("anthropic/claude-sonnet-4-5");

    // None → the default: adaptive thinking (summarised for display) at medium effort.
    let (b, _) = h.round(&fable, None, &user()).await;
    assert_eq!(
        b["thinking"],
        json!({"type": "adaptive", "display": "summarized"}),
        "{b}"
    );
    assert_eq!(b["output_config"]["effort"], "medium", "{b}");

    // off → nothing sent (Fable's API default is off).
    let (b, _) = h.round(&fable, Some(ReasoningEffort::Zero), &user()).await;
    assert!(b.get("thinking").is_none(), "{b}");
    assert!(b.get("output_config").is_none(), "{b}");

    // An explicit keyword wins over the default.
    let (b, _) = h.round(&fable, Some(ReasoningEffort::High), &user()).await;
    assert_eq!(b["thinking"]["type"], "adaptive", "{b}");
    assert_eq!(b["output_config"]["effort"], "high", "{b}");

    // A budget-only model gets the legacy shape: medium = 8k by default, or the budget asked for.
    let (b, _) = h.round(&sonnet45, None, &user()).await;
    assert_eq!(
        b["thinking"],
        json!({"type": "enabled", "budget_tokens": 8000}),
        "{b}"
    );
    assert!(b.get("output_config").is_none(), "{b}");
    let (b, _) = h
        .round(&sonnet45, Some(ReasoningEffort::Budget(4096)), &user())
        .await;
    assert_eq!(
        b["thinking"],
        json!({"type": "enabled", "budget_tokens": 4096}),
        "{b}"
    );
    let (b, _) = h
        .round(&sonnet45, Some(ReasoningEffort::Zero), &user())
        .await;
    assert!(b.get("thinking").is_none(), "{b}");
}

/// Claude Haiku 5+ takes adaptive thinking (genai fork: the legacy
/// `thinking.enabled` shape was a 400 on Haiku 5.5); Haiku 4.5 keeps the
/// budget shape.
#[tokio::test]
async fn haiku_5_request_uses_adaptive_thinking() {
    let h = Harness::new().await;
    let haiku55 = h.resolve("anthropic/claude-haiku-5-5");
    let haiku45 = h.resolve("anthropic/claude-haiku-4-5");

    let (b, _) = h.round(&haiku55, default_effort(&haiku55), &user()).await;
    assert_eq!(b["thinking"]["type"], "adaptive", "{b}");
    assert_eq!(b["output_config"]["effort"], "medium", "{b}");

    let (b, _) = h.round(&haiku55, Some(ReasoningEffort::Max), &user()).await;
    assert_eq!(b["thinking"]["type"], "adaptive", "{b}");
    assert_eq!(b["output_config"]["effort"], "max", "{b}");

    let (b, _) = h
        .round(&haiku55, Some(ReasoningEffort::Zero), &user())
        .await;
    assert_eq!(b["thinking"], json!({"type": "disabled"}), "{b}");

    let (b, _) = h.round(&haiku45, default_effort(&haiku45), &user()).await;
    assert_eq!(
        b["thinking"],
        json!({"type": "enabled", "budget_tokens": 8000}),
        "{b}"
    );
    assert!(b.get("output_config").is_none(), "{b}");
}

#[tokio::test]
async fn thinking_block_round_trips_with_signature_on_tool_use_turn() {
    let h = Harness::new().await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let (_, end) = h.round(&fable, None, &user()).await;

    // Thinking tokens are inside `output_tokens`, which is what the loop bills as output.
    assert_eq!(
        end.captured_usage.as_ref().unwrap().completion_tokens,
        Some(OUTPUT_TOKENS as i32)
    );
    assert_eq!(end.captured_reasoning_content.as_deref(), Some(THINKING));

    // The loop persists this message for the tool round (loop_.rs `into_assistant_message_for_tool_use`).
    let assistant = end.into_assistant_message_for_tool_use().unwrap();
    // …and a reload renders the same thinking ahead of the tool card (no assistant text here).
    let rows: Vec<(i64, ChatMessage)> = vec![(0, ChatMessage::user("hi")), (1, assistant.clone())];
    let rendered = crate::supervisor::render(&rows, &Default::default(), &Default::default());
    assert!(
        matches!(&rendered[1], (1, crate::HistoryEntry::Reasoning { text, .. }) if text == THINKING),
        "{rendered:?}"
    );
    assert!(
        matches!(&rendered[2], (1, crate::HistoryEntry::Tool { .. })),
        "{rendered:?}"
    );
    let mut history = user();
    history.push(assistant);
    history.push(ChatMessage::from(ToolResponse::new("toolu_1", "ok")));
    let (b, _) = h.round(&fable, None, &history).await;

    let msgs = b["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "{b}");
    let content = msgs[1]["content"].as_array().unwrap();
    assert_eq!(
        content[0],
        json!({"type": "thinking", "thinking": THINKING, "signature": SIGNATURE}),
        "{b}"
    );
    assert_eq!(content[1]["type"], "tool_use", "{b}");
    assert_eq!(content[1]["id"], "toolu_1", "{b}");
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result", "{b}");
}

/// 90 §1.5: a 400 for a replayed signature is resent once with every
/// reasoning part stripped; the tool round survives and the ledger shows two
/// attempts of one request.
#[tokio::test]
async fn rejected_thinking_signature_is_resent_stripped() {
    let h = Harness::rejecting(1).await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let (end, rows) = h.turn_call(fable, &foreign_thinking_history()).await;

    let end = match end {
        Ok(end) => end,
        Err(Abort::Model(e)) => panic!("resend failed: {e}"),
        Err(Abort::Partial { err, .. }) => panic!("partial: {err}"),
        Err(_) => panic!("cancelled"),
    };
    assert_eq!(end.captured_reasoning_content.as_deref(), Some(THINKING));
    let bodies = h.bodies.lock().clone();
    assert_eq!(bodies.len(), 2, "{bodies:?}");
    // First attempt replayed the block as stored.
    assert_eq!(
        bodies[0]["messages"][1]["content"][0]["type"], "thinking",
        "{}",
        bodies[0]
    );
    // The resend has no thinking block; tool_use and tool_result are intact.
    let msgs = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "{}", bodies[1]);
    let content = msgs[1]["content"].as_array().unwrap();
    assert!(
        content.iter().all(|c| c["type"] != "thinking"),
        "{}",
        bodies[1]
    );
    assert_eq!(content[0]["type"], "tool_use", "{}", bodies[1]);
    assert_eq!(content[0]["id"], "toolu_1", "{}", bodies[1]);
    assert_eq!(
        msgs[2]["content"][0]["type"], "tool_result",
        "{}",
        bodies[1]
    );

    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_ne!(rows[0].0, rows[1].0);
    assert_eq!(rows[0].1, rows[1].1);
    assert_eq!(
        (rows[0].3.as_str(), rows[1].3.as_str()),
        ("failed", "settled")
    );
}

/// 90 §1.2 end to end: a block another provider signed (an OpenAI Responses
/// `encrypted_content` captured under the `openai` connection) never reaches
/// Anthropic — one request, no `thinking` block, the tool round intact — so
/// the resend path is not even needed.
#[tokio::test]
async fn foreign_signature_is_not_replayed_at_all() {
    let h = Harness::new().await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let history = thinking_history(sig_from(
        AdapterKind::OpenAIResp,
        "openai",
        "gAAAA-openai-fernet",
    ));
    let (end, rows) = h.turn_call(fable, &history).await;

    match end {
        Ok(_) => {}
        Err(Abort::Model(e)) => panic!("call failed: {e}"),
        Err(Abort::Partial { err, .. }) => panic!("partial: {err}"),
        Err(_) => panic!("cancelled"),
    }
    let bodies = h.bodies.lock().clone();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    let msgs = bodies[0]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "{}", bodies[0]);
    let content = msgs[1]["content"].as_array().unwrap();
    assert!(
        content.iter().all(|c| c["type"] != "thinking"),
        "{}",
        bodies[0]
    );
    assert_eq!(content[0]["type"], "tool_use", "{}", bodies[0]);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].3, "settled");
}

/// 90 §1.5: a second rejection is the deterministic error it is today — two
/// requests, no third.
#[tokio::test]
async fn second_rejection_is_a_turn_error() {
    let h = Harness::rejecting(2).await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let (end, rows) = h.turn_call(fable, &foreign_thinking_history()).await;

    let err = match end {
        Err(Abort::Model(e)) => e,
        Ok(_) => panic!("a second rejection must not succeed"),
        Err(_) => panic!("expected Abort::Model"),
    };
    assert!(
        err.contains("Invalid `signature` in `thinking` block"),
        "{err}"
    );
    assert_eq!(h.bodies.lock().len(), 2);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].1, rows[1].1);
    assert_eq!(
        (rows[0].3.as_str(), rows[1].3.as_str()),
        ("failed", "failed")
    );
}

/// 91 THOUGHT-TIME: two reasoning chunks then a text chunk publish exactly
/// one `ReasoningDone`, before the `TextDelta`; the measured ms is handed
/// back and, persisted on the reply row, comes back on the reloaded
/// `Reasoning` entry.
#[tokio::test]
async fn thinking_time_is_measured_published_and_persisted() {
    let h = Harness::texting().await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let mut c = h.live_call(fable, &user()).await;
    let end = match c.end {
        Ok(end) => end,
        Err(_) => panic!("call failed"),
    };
    assert_eq!(end.captured_reasoning_content.as_deref(), Some(THINKING));
    let ms = c.thinking.expect("thinking measured");

    let kinds: Vec<&str> = c
        .events
        .iter()
        .map(|e| match e {
            AgentEvent::ReasoningDelta { .. } => "delta",
            AgentEvent::ReasoningDone { ms: m, .. } => {
                assert_eq!(*m, ms);
                "done"
            }
            AgentEvent::TextDelta { .. } => "text",
            _ => "other",
        })
        .filter(|k| *k != "other")
        .collect();
    assert_eq!(kinds, ["delta", "delta", "done", "text"], "{:?}", c.events);

    // The reply row carries the measurement; a reload renders it on the block.
    let reply = ChatMessage::assistant(end.captured_content.unwrap());
    assert!(c.turn.persist_timed(reply, Some(ms)).await);
    let history = c.store.context_history(c.id).await.unwrap();
    assert_eq!(history.thinking.get(&0), Some(&ms));
    let rendered =
        crate::supervisor::render(&history.messages, &history.entries, &history.thinking);
    assert!(
        matches!(
            &rendered[0],
            (0, crate::HistoryEntry::Reasoning { text, thinking_ms: Some(got), .. })
                if text == THINKING && *got == ms
        ),
        "{rendered:?}"
    );
    assert!(
        matches!(&rendered[1], (0, crate::HistoryEntry::Assistant { text, .. }) if text == "Four."),
        "{rendered:?}"
    );
}

/// 91: a tool-use reply ends the thinking too (`ToolCallChunk`), with the
/// `ReasoningDone` ahead of `ToolCallPending`.
#[tokio::test]
async fn thinking_time_ends_at_the_first_tool_call_chunk() {
    let h = Harness::new().await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let c = h.live_call(fable, &user()).await;
    assert!(c.end.is_ok());
    assert!(c.thinking.is_some());
    let kinds: Vec<&str> = c
        .events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ReasoningDone { .. } => Some("done"),
            AgentEvent::ToolCallPending { .. } => Some("tool"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["done", "tool"], "{:?}", c.events);
}

/// 91: a call that streams no reasoning measures nothing and publishes no
/// `ReasoningDone`; the row persists with `thinking_ms = NULL`.
#[tokio::test]
async fn no_reasoning_means_no_thinking_time() {
    let h = Harness::plain().await;
    let fable = h.resolve("anthropic/claude-fable-5-1");
    let mut c = h.live_call(fable, &user()).await;
    let end = match c.end {
        Ok(end) => end,
        Err(_) => panic!("call failed"),
    };
    assert!(c.thinking.is_none());
    assert!(
        !c.events.iter().any(|e| matches!(
            e,
            AgentEvent::ReasoningDelta { .. } | AgentEvent::ReasoningDone { .. }
        )),
        "{:?}",
        c.events
    );
    assert!(
        c.events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { .. }))
    );
    let reply = ChatMessage::assistant(end.captured_content.unwrap());
    assert!(c.turn.persist_timed(reply, c.thinking).await);
    let history = c.store.context_history(c.id).await.unwrap();
    assert!(history.thinking.is_empty());
    let rendered =
        crate::supervisor::render(&history.messages, &history.entries, &history.thinking);
    assert!(
        matches!(&rendered[..], [(0, crate::HistoryEntry::Assistant { text, .. })] if text == "Four."),
        "{rendered:?}"
    );
}
