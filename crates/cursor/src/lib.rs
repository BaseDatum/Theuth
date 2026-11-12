//! Cursor agent backend client (design doc 05 §Cursor): `agent.v1.AgentService`
//! at `api2.cursor.sh`, Connect-RPC protobuf over HTTP/2 only (the ALB answers
//! HTTP/1.1 with 464). Mirrors omp's `cursor.ts` in gateway mode: one `Run` per
//! assistant step, history replayed as content-addressed blobs, tools served as
//! MCP definitions and handed back to the caller as genai tool calls.
//!
//! - [`models`]: `GetUsableModels` unary.
//! - [`chat_stream`]: one assistant step as genai `ChatStreamEvent`s.
//!
//! Login and refresh live in `crates/oauth`; this crate takes a bearer token.

use std::sync::LazyLock;

use genai::chat::{ChatRequest, ReasoningEffort};
use prost::Message as _;
use reqwest::header::{HeaderMap, HeaderValue};

mod connect;
mod history;
mod run;
#[allow(clippy::all, missing_docs, dead_code)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/agent.v1.rs"));
}

pub use history::takes_reasoning;
pub use run::EventStream;

/// Production backend.
pub const BASE_URL: &str = "https://api2.cursor.sh";
const CLIENT_VERSION: &str = "cli-2026.07.23-e383d2b";

/// A model the account may use (`GetUsableModels`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// Slug for [`chat_stream`].
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether Cursor flags it as a Max-mode model.
    pub max_mode: bool,
}

/// Slugs the account may use.
pub async fn models(token: &str) -> Result<Vec<Model>, String> {
    models_at(BASE_URL, token).await
}

/// [`models`] against another base URL (tests, local bridges).
pub async fn models_at(base: &str, token: &str) -> Result<Vec<Model>, String> {
    let resp = client()?
        .post(format!("{base}/agent.v1.AgentService/GetUsableModels"))
        .headers(headers(token, "application/proto"))
        .body(proto::GetUsableModelsRequest::default().encode_to_vec())
        .send()
        .await
        .map_err(|e| format!("Cursor GetUsableModels: {e}"))?;
    let status = resp.status();
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("Cursor GetUsableModels: {e}"))?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body).into_owned();
        return Err(format!(
            "Cursor GetUsableModels HTTP {status}: {}",
            connect::error_text(&body).unwrap_or(text)
        ));
    }
    let decoded = proto::GetUsableModelsResponse::decode(connect::unary_payload(&body))
        .map_err(|e| format!("Cursor GetUsableModels: bad protobuf: {e}"))?;
    Ok(decoded
        .models
        .into_iter()
        .filter(|m| !m.model_id.trim().is_empty())
        .map(|m| {
            let name = [&m.display_name, &m.display_name_short, &m.model_id]
                .into_iter()
                .find(|s| !s.trim().is_empty())
                .cloned()
                .unwrap_or_default();
            Model {
                name,
                id: m.model_id,
                max_mode: m.max_mode.unwrap_or(false),
            }
        })
        .collect())
}

/// Run one assistant step on `model` for `req`; `conversation` is a stable
/// per-session UUID. `effort` is the caller's explicit reasoning pick (194):
/// on the OpenAI family it replaces the reasoning the slug's suffix implies;
/// `None` keeps the slug's. Yields `Start`, `Chunk`/`ReasoningChunk` deltas, one
/// `ToolCallChunk` per tool call as soon as it is named, then exactly one `End`
/// with the full content (text + tool calls with parsed args), stop reason and
/// usage when reported. Errors are `Err(String)` items; the stream then ends.
pub async fn chat_stream(
    token: &str,
    model: &str,
    effort: Option<&ReasoningEffort>,
    req: ChatRequest,
    conversation: &str,
) -> Result<EventStream, String> {
    run::start(BASE_URL, token, model, effort, &req, conversation).await
}

/// [`chat_stream`] against another base URL (tests, local bridges).
pub async fn chat_stream_at(
    base: &str,
    token: &str,
    model: &str,
    effort: Option<&ReasoningEffort>,
    req: ChatRequest,
    conversation: &str,
) -> Result<EventStream, String> {
    run::start(base, token, model, effort, &req, conversation).await
}

/// HTTP/2-only client (ALPN `h2`, prior knowledge on plain `http://`).
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .map_err(|e| format!("Cursor HTTP client: {e}"))
    });
    CLIENT.as_ref().map_err(Clone::clone)
}

/// The fixed header set both RPCs send (`te`/`connect-protocol-version` are
/// added by the streaming call).
fn headers(token: &str, content_type: &'static str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) {
        h.insert("authorization", v);
    }
    h.insert("x-ghost-mode", HeaderValue::from_static("true"));
    h.insert(
        "x-cursor-client-version",
        HeaderValue::from_static(CLIENT_VERSION),
    );
    h.insert("x-cursor-client-type", HeaderValue::from_static("cli"));
    if let Ok(v) = HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()) {
        h.insert("x-request-id", v);
    }
    h
}
