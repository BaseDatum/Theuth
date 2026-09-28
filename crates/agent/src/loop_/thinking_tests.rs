//! 70 THINKING-TRACES: what a main round sends Anthropic for each effort,
//! captured at a fake `/v1/messages` (wiremock), and that the thinking block
//! a tool-use turn returns is replayed with its signature on the next round.
//! The request bodies are the evidence, exactly as the loop builds them
//! ([`Frame::request`] with [`crate::frame::default_effort`] filling `None`).

use std::sync::Arc;

use genai::chat::{ChatMessage, ChatStreamEvent, ReasoningEffort, StreamEnd, Tool, ToolResponse};
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

/// The fake: records every request body, answers each with one signed thinking
/// block followed by one `tool_use` (a tool-use turn), then `message_stop`.
struct Fake(Arc<Mutex<Vec<Value>>>);

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        self.0
            .lock()
            .push(serde_json::from_slice(&req.body).unwrap());
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
    _server: MockServer,
}

impl Harness {
    async fn new() -> Self {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(Fake(bodies.clone()))
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
