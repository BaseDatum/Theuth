//! One `AgentService/Run`: a bidirectional Connect stream. The request body
//! stays open for the whole turn (run request, then a heartbeat every 5 s plus
//! replies to the server's KV/exec/interaction queries) while the response
//! body streams `AgentServerMessage`s. reqwest + hyper h2 give full duplex:
//! `send()` resolves on response headers and the connection task keeps
//! pulling the channel-backed body.
//!
//! Tool calls: the server executes tools through the exec channel, so an MCP
//! call arrives as `ExecServerMessage.mcp_args` mid-turn. Like omp's gateway
//! (`externalToolExecutor`), we answer at once with a success result saying the
//! call was handed off and the turn should end; the model then stops, the
//! server sends `turn_ended`, and the caller runs the tool and starts the next
//! step with the result in history.

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use genai::chat::{
    ChatRequest, ChatStreamEvent, ContentPart, MessageContent, StopReason, StreamChunk, StreamEnd,
    ToolCall, ToolChunk, Usage,
};
use prost::Message as _;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::connect::{self, Decoder, END_STREAM};
use crate::history::{self, Blobs};
use crate::proto::{
    self, agent_client_message::Message as Client, agent_server_message::Message as Server,
    exec_client_message::Message as ExecReply, exec_server_message::Message as Exec,
    interaction_query::Query, interaction_response::Result as Answer,
    interaction_update::Message as Update, kv_client_message::Message as KvReply,
    kv_server_message::Message as Kv,
};

/// What [`crate::chat_stream`] returns.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, String>> + Send>>;
type Out = mpsc::Sender<Result<ChatStreamEvent, String>>;

const HEARTBEAT: Duration = Duration::from_secs(5);
const HANDOFF: &str = "Tool call received and handed off to the external client for execution. \
Do not retry or call it again; end the turn. The result will be provided in the next request.";
const NOT_IMPLEMENTED: &str = "not implemented by this client";

/// Start a Run against `base` and return its event stream.
pub async fn start(
    base: &str,
    token: &str,
    model: &str,
    req: &ChatRequest,
    conversation: &str,
) -> Result<EventStream, String> {
    let (run, blobs) = history::run_request(model, req, conversation);
    let ctx = history::request_context(req);
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    let outbox = Outbox(tx);
    outbox.send(Client::RunRequest(run)).await?;
    let body = reqwest::Body::wrap_stream(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|b| (Ok::<_, std::convert::Infallible>(b), rx))
    }));
    let resp = crate::client()?
        .post(format!("{base}/agent.v1.AgentService/Run"))
        .headers(crate::headers(token, "application/connect+proto"))
        .header("connect-protocol-version", "1")
        .header("te", "trailers")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Cursor Run: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "Cursor Run HTTP {status}: {}",
            connect::error_text(text.as_bytes()).unwrap_or(text)
        ));
    }
    let (out, events) = mpsc::channel(64);
    tokio::spawn(drive(resp, outbox, out, blobs, ctx));
    Ok(Box::pin(futures::stream::unfold(
        events,
        |mut rx| async move { rx.recv().await.map(|e| (e, rx)) },
    )))
}

/// Request-body frame sink.
struct Outbox(mpsc::Sender<Bytes>);

impl Outbox {
    fn frame(m: Client) -> Bytes {
        connect::frame(
            0,
            &proto::AgentClientMessage { message: Some(m) }.encode_to_vec(),
        )
    }

    async fn send(&self, m: Client) -> Result<(), String> {
        self.0
            .send(Self::frame(m))
            .await
            .map_err(|_| "Cursor Run request body closed".to_owned())
    }
}

async fn drive(
    resp: reqwest::Response,
    outbox: Outbox,
    out: Out,
    mut blobs: Blobs,
    ctx: proto::RequestContext,
) {
    if out.send(Ok(ChatStreamEvent::Start)).await.is_err() {
        return;
    }
    let mut body = resp.bytes_stream();
    let mut tick = tokio::time::interval(HEARTBEAT);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    let mut dec = Decoder::default();
    let mut turn = Turn::default();
    let mut error = None;
    'run: loop {
        tokio::select! {
            chunk = body.next() => {
                match chunk {
                    None => break,
                    Some(Err(e)) => { error = Some(format!("Cursor Run transport: {e}")); break }
                    Some(Ok(chunk)) => dec.push(&chunk),
                }
                while let Some((flags, payload)) = dec.next() {
                    if flags & END_STREAM != 0 {
                        error = error.or_else(|| connect::error_text(&payload));
                        continue;
                    }
                    let msg = match proto::AgentServerMessage::decode(payload) {
                        Ok(m) => m,
                        Err(e) => { tracing::warn!("cursor: undecodable server frame: {e}"); continue }
                    };
                    if turn.handle(msg, &outbox, &out, &mut blobs, &ctx).await.is_err() {
                        break 'run;
                    }
                }
            }
            // Best effort: a full channel means hyper is already behind.
            _ = tick.tick() => { let _ = outbox.0.try_send(Outbox::frame(Client::ClientHeartbeat(proto::ClientHeartbeat {}))); }
        }
    }
    let last = match error {
        Some(e) => Err(e),
        None if !turn.ended => Err("Cursor stream ended before turn_ended".to_owned()),
        None => Ok(ChatStreamEvent::End(turn.finish())),
    };
    let _ = out.send(last).await;
}

/// The assistant step being accumulated from `InteractionUpdate`s.
#[derive(Default)]
struct Turn {
    parts: Vec<ContentPart>,
    reasoning: String,
    /// envelope `call_id` → (index into `parts`, cumulative args JSON text).
    open: HashMap<String, (usize, String)>,
    /// tool call id → index into `parts`; exec and streamed frames both name it.
    calls: HashMap<String, usize>,
    completion_tokens: i32,
    context_tokens: i32,
    ended: bool,
}

fn mcp_args(tc: Option<&proto::ToolCall>) -> Option<&proto::McpArgs> {
    match tc?.tool.as_ref()? {
        proto::tool_call::Tool::McpToolCall(m) => m.args.as_ref(),
        _ => None,
    }
}

/// Completion-frame args win per key, except a string must not downgrade a
/// structured value the streamed JSON already carried (omp #2615).
fn merge(streamed: Value, completion: Value) -> Value {
    let Value::Object(mut out) = streamed else {
        return completion;
    };
    if let Value::Object(c) = completion {
        for (k, v) in c {
            if v.is_string() && out.get(&k).is_some_and(|s| s.is_object() || s.is_array()) {
                continue;
            }
            out.insert(k, v);
        }
    }
    Value::Object(out)
}

impl Turn {
    async fn handle(
        &mut self,
        msg: proto::AgentServerMessage,
        outbox: &Outbox,
        out: &Out,
        blobs: &mut Blobs,
        ctx: &proto::RequestContext,
    ) -> Result<(), String> {
        match msg.message {
            Some(Server::InteractionUpdate(u)) => self.update(u, out).await,
            Some(Server::ExecServerMessage(exec)) => self.exec(exec, outbox, out, ctx).await,
            Some(Server::KvServerMessage(kv)) => {
                let reply = match kv.message {
                    Some(Kv::GetBlobArgs(a)) => KvReply::GetBlobResult(proto::GetBlobResult {
                        blob_data: blobs.get(&a.blob_id).cloned(),
                    }),
                    Some(Kv::SetBlobArgs(a)) => {
                        blobs.insert(a.blob_id, a.blob_data);
                        KvReply::SetBlobResult(proto::SetBlobResult { error: None })
                    }
                    None => return Ok(()),
                };
                outbox
                    .send(Client::KvClientMessage(proto::KvClientMessage {
                        id: kv.id,
                        message: Some(reply),
                    }))
                    .await
            }
            Some(Server::InteractionQuery(q)) => match answer(q.query) {
                Some(result) => {
                    outbox
                        .send(Client::InteractionResponse(proto::InteractionResponse {
                            id: q.id,
                            result: Some(result),
                        }))
                        .await
                }
                None => Ok(()),
            },
            Some(Server::ConversationCheckpointUpdate(s)) => {
                if let Some(t) = s.token_details {
                    self.context_tokens = t.used_tokens as i32;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn emit(out: &Out, ev: ChatStreamEvent) -> Result<(), String> {
        out.send(Ok(ev))
            .await
            .map_err(|_| "consumer dropped".to_owned())
    }

    /// Register a tool call once, whichever of the streamed start or the exec
    /// frame names it first; emit its `ToolCallChunk` on first sight.
    async fn call(
        &mut self,
        id: String,
        name: String,
        args: Value,
        out: &Out,
    ) -> Result<usize, String> {
        if let Some(&i) = self.calls.get(&id) {
            if let ContentPart::ToolCall(tc) = &mut self.parts[i] {
                tc.fn_arguments = merge(std::mem::take(&mut tc.fn_arguments), args);
            }
            return Ok(i);
        }
        let tool_call = ToolCall {
            call_id: id.clone(),
            fn_name: name,
            fn_arguments: if args.as_object().is_some_and(|o| o.is_empty()) {
                Value::Null
            } else {
                args
            },
            thought_signatures: None,
        };
        let i = self.parts.len();
        self.parts.push(ContentPart::ToolCall(tool_call.clone()));
        self.calls.insert(id, i);
        Self::emit(out, ChatStreamEvent::ToolCallChunk(ToolChunk { tool_call })).await?;
        Ok(i)
    }

    async fn update(&mut self, u: proto::InteractionUpdate, out: &Out) -> Result<(), String> {
        match u.message {
            Some(Update::TextDelta(t)) => {
                match self.parts.last_mut() {
                    Some(ContentPart::Text(s)) => s.push_str(&t.text),
                    _ => self.parts.push(ContentPart::Text(t.text.clone())),
                }
                Self::emit(out, ChatStreamEvent::Chunk(StreamChunk { content: t.text })).await
            }
            Some(Update::ThinkingDelta(t)) => {
                self.reasoning.push_str(&t.text);
                Self::emit(
                    out,
                    ChatStreamEvent::ReasoningChunk(StreamChunk { content: t.text }),
                )
                .await
            }
            Some(Update::ToolCallStarted(s)) => {
                let Some(a) = mcp_args(s.tool_call.as_ref()) else {
                    return Ok(());
                };
                let id = if a.tool_call_id.is_empty() {
                    s.call_id.clone()
                } else {
                    a.tool_call_id.clone()
                };
                let name = if a.tool_name.is_empty() {
                    a.name.clone()
                } else {
                    a.tool_name.clone()
                };
                let i = self
                    .call(id, name, history::decode_args(&a.args), out)
                    .await?;
                self.open.insert(s.call_id, (i, String::new()));
                Ok(())
            }
            Some(Update::PartialToolCall(p)) => {
                // `args_text_delta` is the aggregated JSON so far; keep the
                // longest consistent snapshot.
                if let Some((_, buf)) = self.open.get_mut(&p.call_id) {
                    if p.args_text_delta.starts_with(buf.as_str()) {
                        *buf = p.args_text_delta;
                    } else {
                        buf.push_str(&p.args_text_delta);
                    }
                }
                Ok(())
            }
            Some(Update::ToolCallCompleted(c)) => {
                if let Some((i, buf)) = self.open.remove(&c.call_id) {
                    let streamed = serde_json::from_str(&buf).unwrap_or(Value::Null);
                    let completion =
                        mcp_args(c.tool_call.as_ref()).map(|a| history::decode_args(&a.args));
                    if let ContentPart::ToolCall(tc) = &mut self.parts[i] {
                        let base = merge(std::mem::take(&mut tc.fn_arguments), streamed);
                        tc.fn_arguments = match completion {
                            Some(c) => merge(base, c),
                            None => base,
                        };
                    }
                }
                Ok(())
            }
            Some(Update::TokenDelta(t)) => {
                self.completion_tokens += t.tokens;
                Ok(())
            }
            Some(Update::TurnEnded(_)) => {
                self.ended = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn exec(
        &mut self,
        exec: proto::ExecServerMessage,
        outbox: &Outbox,
        out: &Out,
        ctx: &proto::RequestContext,
    ) -> Result<(), String> {
        let reply = |m: ExecReply| {
            Client::ExecClientMessage(proto::ExecClientMessage {
                id: exec.id,
                exec_id: exec.exec_id.clone(),
                message: Some(m),
                ..Default::default()
            })
        };
        let result = match &exec.message {
            Some(Exec::RequestContextArgs(_)) => {
                ExecReply::RequestContextResult(proto::RequestContextResult {
                    result: Some(proto::request_context_result::Result::Success(
                        proto::RequestContextSuccess {
                            request_context: Some(ctx.clone()),
                            served_from_disk_cache: None,
                        },
                    )),
                })
            }
            Some(Exec::McpArgs(a)) => {
                let result = if a.smart_mode_approval_only {
                    // A permission probe, not an invocation: nothing here can ask
                    // the user, so refuse rather than pre-authorize (as omp does).
                    proto::mcp_result::Result::Rejected(proto::McpRejected {
                        reason: format!(
                            "Tool \"{}\" is not approved to run without asking.",
                            a.tool_name
                        ),
                        is_readonly: false,
                    })
                } else {
                    let name = if a.tool_name.is_empty() {
                        a.name.clone()
                    } else {
                        a.tool_name.clone()
                    };
                    self.call(
                        a.tool_call_id.clone(),
                        name,
                        history::decode_args(&a.args),
                        out,
                    )
                    .await?;
                    proto::mcp_result::Result::Success(proto::McpSuccess {
                        content: vec![proto::McpToolResultContentItem {
                            content: Some(proto::mcp_tool_result_content_item::Content::Text(
                                proto::McpTextContent {
                                    text: HANDOFF.to_owned(),
                                    output_location: None,
                                },
                            )),
                        }],
                        is_error: false,
                    })
                };
                ExecReply::McpResult(proto::McpResult {
                    result: Some(result),
                })
            }
            // Native tools (shell, read, write, …) are not served by this
            // client; fail the frame in band so the model routes around it.
            _ => {
                let control = |m| {
                    Client::ExecClientControlMessage(proto::ExecClientControlMessage {
                        message: Some(m),
                    })
                };
                outbox
                    .send(control(proto::exec_client_control_message::Message::Throw(
                        proto::ExecClientThrow {
                            id: exec.id,
                            error: format!("Native tool execution is {NOT_IMPLEMENTED}"),
                            stack_trace: None,
                            error_code: Some("not_implemented".to_owned()),
                        },
                    )))
                    .await?;
                return outbox
                    .send(control(
                        proto::exec_client_control_message::Message::StreamClose(
                            proto::ExecClientStreamClose { id: exec.id },
                        ),
                    ))
                    .await;
            }
        };
        outbox.send(reply(result)).await
    }

    fn finish(self) -> StreamEnd {
        let has_calls = !self.calls.is_empty();
        let completion = (self.completion_tokens > 0).then_some(self.completion_tokens);
        let total = (self.context_tokens > 0).then_some(self.context_tokens);
        let captured_usage = (completion.is_some() || total.is_some()).then(|| Usage {
            prompt_tokens: total.zip(completion).map(|(t, c)| (t - c).max(0)),
            completion_tokens: completion,
            total_tokens: total,
            ..Default::default()
        });
        StreamEnd {
            captured_usage,
            captured_stop_reason: Some(if has_calls {
                StopReason::ToolCall("tool_calls".to_owned())
            } else {
                StopReason::Completed("end_turn".to_owned())
            }),
            captured_content: Some(MessageContent::from_parts(self.parts)),
            captured_reasoning_content: (!self.reasoning.is_empty()).then_some(self.reasoning),
            captured_response_id: None,
        }
    }
}

/// Hosted search/fetch permission gates are approved; interactive prompts we
/// cannot surface are rejected so the server is not left waiting. VM setup has
/// a success-only result and is left unanswered rather than faked.
fn answer(q: Option<Query>) -> Option<Answer> {
    Some(match q? {
        Query::WebSearchRequestQuery(_) => {
            Answer::WebSearchRequestResponse(proto::WebSearchRequestResponse {
                result: Some(proto::web_search_request_response::Result::Approved(
                    Default::default(),
                )),
            })
        }
        Query::ExaSearchRequestQuery(_) => {
            Answer::ExaSearchRequestResponse(proto::ExaSearchRequestResponse {
                result: Some(proto::exa_search_request_response::Result::Approved(
                    Default::default(),
                )),
            })
        }
        Query::ExaFetchRequestQuery(_) => {
            Answer::ExaFetchRequestResponse(proto::ExaFetchRequestResponse {
                result: Some(proto::exa_fetch_request_response::Result::Approved(
                    Default::default(),
                )),
            })
        }
        Query::WebFetchRequestQuery(_) => {
            Answer::WebFetchRequestResponse(proto::WebFetchRequestResponse {
                result: Some(proto::web_fetch_request_response::Result::Approved(
                    Default::default(),
                )),
            })
        }
        Query::AskQuestionInteractionQuery(_) => {
            Answer::AskQuestionInteractionResponse(proto::AskQuestionInteractionResponse {
                result: Some(proto::AskQuestionResult {
                    result: Some(proto::ask_question_result::Result::Rejected(
                        proto::AskQuestionRejected {
                            reason: format!("Interactive questions are {NOT_IMPLEMENTED}"),
                        },
                    )),
                }),
            })
        }
        Query::SwitchModeRequestQuery(_) => {
            Answer::SwitchModeRequestResponse(proto::SwitchModeRequestResponse {
                result: Some(proto::switch_mode_request_response::Result::Rejected(
                    proto::SwitchModeRequestResponseRejected {
                        reason: format!("Mode switches are {NOT_IMPLEMENTED}"),
                    },
                )),
            })
        }
        Query::CreatePlanRequestQuery(_) => {
            Answer::CreatePlanRequestResponse(proto::CreatePlanRequestResponse {
                result: Some(proto::CreatePlanResult {
                    result: Some(proto::create_plan_result::Result::Error(
                        proto::CreatePlanError {
                            error: format!("Plan files are {NOT_IMPLEMENTED}"),
                        },
                    )),
                    ..Default::default()
                }),
            })
        }
        Query::SetupVmEnvironmentArgs(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_keeps_structured_over_downgraded_string() {
        let streamed = json!({"tasks": [1, 2], "name": "a", "n": 1});
        let completion = json!({"tasks": "[1,2]", "name": "b", "extra": true});
        assert_eq!(
            merge(streamed, completion),
            json!({"tasks": [1, 2], "name": "b", "n": 1, "extra": true})
        );
        assert_eq!(merge(Value::Null, json!({"a": 1})), json!({"a": 1}));
    }
}
