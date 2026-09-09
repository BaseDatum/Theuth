//! In-process Connect server (h2c) proving the bidi Run round trip: the client
//! answers exec/KV queries on the still-open request body while the response
//! streams, and the resulting genai events carry text, reasoning, tool calls
//! with merged args, usage and stop reason.

use std::collections::HashMap;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::StreamExt;
use genai::chat::{ChatMessage, ChatRequest, ChatStreamEvent, ContentPart, StopReason, Tool};
use prost::Message as _;
use serde_json::json;
use tokio::net::TcpListener;

#[allow(clippy::all, dead_code)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/agent.v1.rs"));
}
use proto::{
    agent_client_message::Message as Client, agent_server_message::Message as Server,
    exec_client_message::Message as ExecReply, exec_server_message::Message as Exec,
    interaction_update::Message as Update, kv_client_message::Message as KvReply,
    kv_server_message::Message as Kv,
};

fn frame(flags: u8, payload: &[u8]) -> Bytes {
    let mut b = BytesMut::new();
    b.put_u8(flags);
    b.put_u32(payload.len() as u32);
    b.put_slice(payload);
    b.freeze()
}

fn server_frame(m: Server) -> Bytes {
    frame(
        0,
        &proto::AgentServerMessage { message: Some(m) }.encode_to_vec(),
    )
}

fn update(m: Update) -> Server {
    Server::InteractionUpdate(proto::InteractionUpdate { message: Some(m) })
}

fn mcp_call(id: &str, args: HashMap<String, Vec<u8>>) -> proto::ToolCall {
    proto::ToolCall {
        tool: Some(proto::tool_call::Tool::McpToolCall(proto::McpToolCall {
            args: Some(proto::McpArgs {
                name: "grep".into(),
                tool_name: "grep".into(),
                tool_call_id: id.into(),
                args,
                ..Default::default()
            }),
            result: None,
            description: None,
        })),
        tool_call_id: Some(id.into()),
    }
}

fn pb_string(s: &str) -> Vec<u8> {
    prost_types::Value {
        kind: Some(prost_types::value::Kind::StringValue(s.into())),
    }
    .encode_to_vec()
}

/// Next non-heartbeat client message off the request body.
async fn next_client(body: &mut h2::RecvStream, buf: &mut BytesMut) -> Client {
    loop {
        while buf.len() >= 5 {
            let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
            if buf.len() < 5 + len {
                break;
            }
            buf.advance(5);
            let msg = proto::AgentClientMessage::decode(buf.split_to(len).freeze()).unwrap();
            match msg.message.unwrap() {
                Client::ClientHeartbeat(_) => {}
                m => return m,
            }
        }
        let chunk = body.data().await.expect("request body stays open").unwrap();
        body.flow_control().release_capacity(chunk.len()).unwrap();
        buf.extend_from_slice(&chunk);
    }
}

/// The h2 connection only makes progress while `accept` is polled, so each
/// stream is handled on its own task.
async fn serve(listener: TcpListener) {
    let (sock, _) = listener.accept().await.unwrap();
    let mut conn = h2::server::handshake(sock).await.unwrap();
    while let Some(Ok((req, respond))) = conn.accept().await {
        tokio::spawn(handle(req, respond));
    }
}

async fn handle(req: http::Request<h2::RecvStream>, mut respond: h2::server::SendResponse<Bytes>) {
    assert_eq!(req.headers()["authorization"], "Bearer tok");
    assert_eq!(req.headers()["x-cursor-client-type"], "cli");
    let unary = req.uri().path().ends_with("GetUsableModels");
    let mut body = req.into_body();
    let mut buf = BytesMut::new();
    let ok = http::Response::builder().status(200);
    {
        if unary {
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                body.flow_control().release_capacity(chunk.len()).unwrap();
                buf.extend_from_slice(&chunk);
            }
            proto::GetUsableModelsRequest::decode(buf.freeze()).unwrap();
            let models = proto::GetUsableModelsResponse {
                models: vec![
                    proto::ModelDetails {
                        model_id: "gpt-5.4-high".into(),
                        display_name: "GPT-5.4 High".into(),
                        ..Default::default()
                    },
                    proto::ModelDetails {
                        model_id: "claude-4.6-opus-max".into(),
                        max_mode: Some(true),
                        ..Default::default()
                    },
                    proto::ModelDetails {
                        model_id: "  ".into(),
                        ..Default::default()
                    },
                ],
            };
            let mut send = respond
                .send_response(
                    ok.header("content-type", "application/proto")
                        .body(())
                        .unwrap(),
                    false,
                )
                .unwrap();
            send.send_data(models.encode_to_vec().into(), true).unwrap();
            return;
        }
        let mut send = respond
            .send_response(
                ok.header("content-type", "application/connect+proto")
                    .body(())
                    .unwrap(),
                false,
            )
            .unwrap();
        let Client::RunRequest(run) = next_client(&mut body, &mut buf).await else {
            panic!("first frame is the run")
        };
        assert_eq!(run.conversation_id.as_deref(), Some("conv-1"));
        let requested = run.requested_model.unwrap();
        if requested.model_id == "boom" {
            send.send_data(
                frame(
                    2,
                    br#"{"error":{"code":"resource_exhausted","message":"poisoned"}}"#,
                ),
                true,
            )
            .unwrap();
            return;
        }
        assert_eq!(requested.model_id, "gpt-5.4");
        assert_eq!(requested.parameters[0].value, "high");
        let state = run.conversation_state.unwrap();

        // Exec handshake: the client must answer while the response is open.
        send.send_data(
            server_frame(Server::ExecServerMessage(proto::ExecServerMessage {
                id: 1,
                message: Some(Exec::RequestContextArgs(Default::default())),
                ..Default::default()
            })),
            false,
        )
        .unwrap();
        let Client::ExecClientMessage(reply) = next_client(&mut body, &mut buf).await else {
            panic!("exec reply")
        };
        assert_eq!(reply.id, 1);
        let Some(ExecReply::RequestContextResult(ctx)) = reply.message else {
            panic!("request context")
        };
        let Some(proto::request_context_result::Result::Success(ok)) = ctx.result else {
            panic!("success")
        };
        let ctx = ok.request_context.unwrap();
        assert_eq!(ctx.rules[0].content, "Be terse.");
        assert_eq!(ctx.tools[0].tool_name, "grep");
        assert_eq!(ctx.tools[0].provider_identifier, "wth");

        // KV: fetch the system-prompt blob.
        send.send_data(
            server_frame(Server::KvServerMessage(proto::KvServerMessage {
                id: 7,
                message: Some(Kv::GetBlobArgs(proto::GetBlobArgs {
                    blob_id: state.root_prompt_messages_json[0].clone(),
                })),
                span_context: None,
            })),
            false,
        )
        .unwrap();
        let Client::KvClientMessage(kv) = next_client(&mut body, &mut buf).await else {
            panic!("kv reply")
        };
        assert_eq!(kv.id, 7);
        let Some(KvReply::GetBlobResult(blob)) = kv.message else {
            panic!("blob")
        };
        let blob: serde_json::Value = serde_json::from_slice(&blob.blob_data.unwrap()).unwrap();
        assert_eq!(blob, json!({"role": "system", "content": "Be terse."}));

        for m in [
            update(Update::TextDelta(proto::TextDeltaUpdate {
                text: "Hel".into(),
            })),
            update(Update::TextDelta(proto::TextDeltaUpdate {
                text: "lo".into(),
            })),
            update(Update::ThinkingDelta(proto::ThinkingDeltaUpdate {
                text: "hmm".into(),
            })),
            update(Update::ToolCallStarted(proto::ToolCallStartedUpdate {
                call_id: "c1".into(),
                tool_call: Some(mcp_call("tc1", HashMap::new())),
                model_call_id: String::new(),
            })),
            update(Update::PartialToolCall(proto::PartialToolCallUpdate {
                call_id: "c1".into(),
                args_text_delta: r#"{"q":"#.into(),
                ..Default::default()
            })),
            update(Update::PartialToolCall(proto::PartialToolCallUpdate {
                call_id: "c1".into(),
                args_text_delta: r#"{"q":"foo","tasks":[1,2]}"#.into(),
                ..Default::default()
            })),
            Server::ExecServerMessage(proto::ExecServerMessage {
                id: 2,
                message: Some(Exec::McpArgs(proto::McpArgs {
                    name: "grep".into(),
                    tool_name: "grep".into(),
                    tool_call_id: "tc1".into(),
                    args: HashMap::from([("q".to_owned(), pb_string("foo"))]),
                    ..Default::default()
                })),
                ..Default::default()
            }),
        ] {
            send.send_data(server_frame(m), false).unwrap();
        }
        let Client::ExecClientMessage(reply) = next_client(&mut body, &mut buf).await else {
            panic!("mcp reply")
        };
        assert_eq!(reply.id, 2);
        let Some(ExecReply::McpResult(r)) = reply.message else {
            panic!("mcp result")
        };
        let Some(proto::mcp_result::Result::Success(s)) = r.result else {
            panic!("handoff is a success")
        };
        let Some(proto::mcp_tool_result_content_item::Content::Text(t)) =
            s.content[0].content.clone()
        else {
            panic!()
        };
        assert!(t.text.contains("handed off"));

        // Completion downgrades `tasks` to a string; the streamed array must win.
        let completion = HashMap::from([
            ("q".to_owned(), pb_string("foo")),
            ("tasks".to_owned(), pb_string("[1,2]")),
            (
                "n".to_owned(),
                prost_types::Value {
                    kind: Some(prost_types::value::Kind::NumberValue(3.0)),
                }
                .encode_to_vec(),
            ),
        ]);
        for m in [
            update(Update::ToolCallCompleted(proto::ToolCallCompletedUpdate {
                call_id: "c1".into(),
                tool_call: Some(mcp_call("tc1", completion)),
                model_call_id: String::new(),
            })),
            update(Update::TokenDelta(proto::TokenDeltaUpdate { tokens: 7 })),
            Server::ConversationCheckpointUpdate(proto::ConversationStateStructure {
                token_details: Some(proto::ConversationTokenDetails {
                    used_tokens: 107,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            update(Update::TurnEnded(proto::TurnEndedUpdate {})),
        ] {
            send.send_data(server_frame(m), false).unwrap();
        }
        send.send_data(frame(2, b"{}"), true).unwrap();
    }
}

fn request() -> ChatRequest {
    ChatRequest {
        system: Some("Be terse.".into()),
        messages: vec![ChatMessage::user("find foo")],
        tools: Some(vec![
            Tool::new("grep".to_owned()).with_schema(json!({"type": "object"})),
        ]),
        ..Default::default()
    }
}

#[tokio::test]
async fn bidi_run_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(listener));

    let models = cursor::models_at(&base, "tok").await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(
        models[0],
        cursor::Model {
            id: "gpt-5.4-high".into(),
            name: "GPT-5.4 High".into(),
            max_mode: false
        }
    );
    assert_eq!(models[1].name, "claude-4.6-opus-max");
    assert!(models[1].max_mode);

    let stream = cursor::chat_stream_at(&base, "tok", "gpt-5.4-high", request(), "conv-1")
        .await
        .unwrap();
    let events: Vec<_> = stream.collect().await;
    let mut it = events.into_iter().map(|e| e.expect("no error item"));
    assert!(matches!(it.next(), Some(ChatStreamEvent::Start)));
    assert!(matches!(it.next(), Some(ChatStreamEvent::Chunk(c)) if c.content == "Hel"));
    assert!(matches!(it.next(), Some(ChatStreamEvent::Chunk(c)) if c.content == "lo"));
    assert!(matches!(it.next(), Some(ChatStreamEvent::ReasoningChunk(c)) if c.content == "hmm"));
    let Some(ChatStreamEvent::ToolCallChunk(tc)) = it.next() else {
        panic!("tool call chunk")
    };
    assert_eq!(tc.tool_call.call_id, "tc1");
    assert_eq!(tc.tool_call.fn_name, "grep");
    assert!(tc.tool_call.fn_arguments.is_null());
    let Some(ChatStreamEvent::End(end)) = it.next() else {
        panic!("end")
    };
    assert!(it.next().is_none());
    let content = end.captured_content.unwrap();
    let parts = content.parts();
    assert!(matches!(&parts[0], ContentPart::Text(t) if t == "Hello"));
    let ContentPart::ToolCall(call) = &parts[1] else {
        panic!("tool call part")
    };
    assert_eq!(
        call.fn_arguments,
        json!({"q": "foo", "tasks": [1, 2], "n": 3})
    );
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        end.captured_stop_reason,
        Some(StopReason::ToolCall(_))
    ));
    assert_eq!(end.captured_reasoning_content.as_deref(), Some("hmm"));
    let usage = end.captured_usage.unwrap();
    assert_eq!(
        (
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens
        ),
        (Some(100), Some(7), Some(107))
    );

    let stream = cursor::chat_stream_at(&base, "tok", "boom", request(), "conv-1")
        .await
        .unwrap();
    let events: Vec<_> = stream.collect().await;
    assert!(matches!(events[0], Ok(ChatStreamEvent::Start)));
    assert_eq!(
        events[1].as_ref().unwrap_err(),
        "Connect error resource_exhausted: poisoned"
    );
    assert_eq!(events.len(), 2);
}
