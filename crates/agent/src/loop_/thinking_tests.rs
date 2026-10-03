//! 70 THINKING-TRACES: what a main round sends Anthropic for each effort,
//! captured at a fake `/v1/messages` (wiremock), and that the thinking block
//! a tool-use turn returns is replayed with its signature on the next round.
//! The request bodies are the evidence, exactly as the loop builds them
//! ([`Frame::request`] with [`crate::frame::default_effort`] filling `None`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use genai::chat::{
    ChatMessage, ChatStreamEvent, ContentPart, MessageContent, ReasoningEffort, StreamEnd, Tool,
    ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ServiceTarget};
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
struct Fake {
    bodies: Arc<Mutex<Vec<Value>>>,
    reject: AtomicUsize,
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
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return ResponseTemplate::new(400).set_body_raw(REJECTED, "application/json");
        }
        let ev = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
        let body = [
            ev(
                "message_start",
                json!({"type": "message_start", "message": {"id": "m", "role": "assistant", "content": [],
                    "usage": {"input_tokens": 12, "output_tokens": 1}}}),
            ),
            ev(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
            ),
            ev(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": THINKING}}),
            ),
            ev(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": SIGNATURE}}),
            ),
            ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            ev(
                "content_block_start",
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "echo", "input": {}}}),
            ),
            ev(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"x\":1}"}}),
            ),
            ev("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            ev(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": OUTPUT_TOKENS}}),
            ),
            ev("message_stop", json!({"type": "message_stop"})),
        ]
        .concat();
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }
}

struct Harness {
    bodies: Arc<Mutex<Vec<Value>>>,
    client: Client,
    catalog: models::Catalog,
    dir: std::path::PathBuf,
    _server: MockServer,
}

impl Harness {
    async fn new() -> Self {
        Self::rejecting(0).await
    }

    /// A harness whose fake answers the first `reject` requests with [`REJECTED`].
    async fn rejecting(reject: usize) -> Self {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(Fake {
                bodies: bodies.clone(),
                reject: AtomicUsize::new(reject),
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
        let dir = std::env::temp_dir().join(format!("theuth-thinking-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            bodies,
            client,
            catalog: models::Catalog::load(&dir),
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
        let store = Store::open_memory().await.unwrap();
        let id = SessionId::new();
        store
            .create_session(id, &model.id, None, None, None)
            .await
            .unwrap();
        let rt = Runtime::new(figment::Figment::new(), self.dir.clone());
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
            Auth::stub(&self.dir),
            rt,
            Arc::new(models::Catalog::load(&self.dir)),
            None,
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&self.dir),
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
        };
        let end = turn
            .call(&current, frame, transport, None, id, CallKind::Round)
            .await;
        let rows = sqlx::query_as(
            "SELECT call_id, request_id, kind, status FROM model_calls WHERE owner_id = ? ORDER BY attempt",
        )
        .bind(id.to_string())
        .fetch_all(store.pool())
        .await
        .unwrap();
        (end, rows)
    }
}

/// 90 §1.5: the one construction of a thought signature in this module —
/// integration swaps the payload type here alone.
fn sig(s: &str) -> ContentPart {
    ContentPart::ThoughtSignature(s.into())
}

/// A stored tool-use turn whose thinking block another provider signed:
/// user, assistant (signature + reasoning + tool call), tool result.
fn foreign_thinking_history() -> Vec<ChatMessage> {
    let mut history = user();
    history.push(ChatMessage::assistant(MessageContent::from_parts(vec![
        sig("gAAAA-openai-fernet"),
        ContentPart::ReasoningContent("summary from elsewhere".into()),
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
    }
    .request(
        Transport::OpenAiLike,
        SessionId::new(),
        default_effort(&r("openai/gpt-5.6-sol")),
    );
    assert!(opts.reasoning_effort.is_none());
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
    let rendered = crate::supervisor::render(&rows, &Default::default());
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
