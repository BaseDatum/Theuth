//! genai `ChatRequest` → Cursor `AgentRunRequest`. The server rebuilds the
//! model prompt from `root_prompt_messages_json` (Vercel-AI-SDK-shaped JSON
//! blobs) plus `RequestContext.rules`/`tools`; `turns` is display metadata.
//! Every blob is content-addressed (sha256) and served back over the KV
//! channel during the Run, so the [`Blobs`] map must outlive the request.

use std::collections::HashMap;

use genai::chat::{ChatRequest, ChatRole, ContentPart, Tool};
use prost::Message as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::proto::{self, conversation_action::Action};

/// Tool provider tag Cursor echoes back on every MCP call.
pub const PROVIDER: &str = "theuth";
const DEFAULT_SYSTEM: &str = "You are a helpful assistant.";

/// sha256 id → bytes.
pub type Blobs = HashMap<Vec<u8>, Vec<u8>>;

fn store(blobs: &mut Blobs, data: Vec<u8>) -> Vec<u8> {
    let id = Sha256::digest(&data).to_vec();
    blobs.insert(id.clone(), data);
    id
}

/// Cursor rejects ids outside `^[a-zA-Z0-9_-]+$` with an opaque
/// `resource_exhausted`; cap at 64 like omp.
pub fn tool_call_id(id: &str) -> String {
    let mut out: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(64);
    out
}

/// `serde_json::Value` → `google.protobuf.Value` wire bytes (MCP arg values,
/// tool input schemas).
pub fn encode_json(v: &Value) -> Vec<u8> {
    to_pb(v).encode_to_vec()
}

fn to_pb(v: &Value) -> prost_types::Value {
    use prost_types::value::Kind;
    let kind = match v {
        Value::Null => Kind::NullValue(0),
        Value::Bool(b) => Kind::BoolValue(*b),
        Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Kind::StringValue(s.clone()),
        Value::Array(a) => Kind::ListValue(prost_types::ListValue {
            values: a.iter().map(to_pb).collect(),
        }),
        Value::Object(o) => Kind::StructValue(prost_types::Struct {
            fields: o.iter().map(|(k, v)| (k.clone(), to_pb(v))).collect(),
        }),
    };
    prost_types::Value { kind: Some(kind) }
}

fn from_pb(v: prost_types::Value) -> Value {
    use prost_types::value::Kind;
    match v.kind {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(b),
        // protobuf Value has only doubles; keep integral args integers so
        // callers can deserialize `limit: 10` into an integer field.
        Some(Kind::NumberValue(n)) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => {
            json!(n as i64)
        }
        Some(Kind::NumberValue(n)) => json!(n),
        Some(Kind::StringValue(s)) => Value::String(s),
        Some(Kind::ListValue(l)) => Value::Array(l.values.into_iter().map(from_pb).collect()),
        Some(Kind::StructValue(s)) => {
            Value::Object(s.fields.into_iter().map(|(k, v)| (k, from_pb(v))).collect())
        }
    }
}

/// One MCP arg value off the wire: a `google.protobuf.Value`, whose string
/// form may itself be JSON text (the server double-encodes structured args);
/// raw UTF-8 as a last resort.
pub fn decode_arg(bytes: &[u8]) -> Value {
    let text = match prost_types::Value::decode(bytes).map(from_pb) {
        Ok(Value::String(s)) => s,
        Ok(v) => return v,
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    };
    match text.trim_start().chars().next() {
        Some('{' | '[' | '"') => serde_json::from_str(&text).unwrap_or(Value::String(text)),
        _ => Value::String(text),
    }
}

/// Whole `McpArgs.args` map as a JSON object.
pub fn decode_args(args: &HashMap<String, Vec<u8>>) -> Value {
    Value::Object(
        args.iter()
            .map(|(k, v)| (k.clone(), decode_arg(v)))
            .collect(),
    )
}

/// Wire model id + `RequestedModel.parameters` for a catalog slug. OpenAI
/// family effort siblings (`gpt-5.4-mini-low[-fast]`) must go as base id +
/// `reasoning` parameter or the Run fails `resource_exhausted`; `-none` is
/// the bare base. Bare `composer-2.5` pins the Standard lane. Everything else
/// (Claude, Grok, k3, …) passes through unchanged, as omp does.
pub fn wire_model(id: &str) -> (String, Vec<(&'static str, String)>) {
    if id == "composer-2.5" {
        return (id.to_owned(), vec![("fast", "false".to_owned())]);
    }
    let openai = id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4");
    if !openai {
        return (id.to_owned(), vec![]);
    }
    let (stem, lane) = match id.strip_suffix("-fast") {
        Some(s) => (s, "-fast"),
        None => (id, ""),
    };
    for effort in ["minimal", "low", "medium", "high", "xhigh", "max", "none"] {
        if let Some(base) = stem.strip_suffix(&format!("-{effort}")) {
            let params = if effort == "none" {
                vec![]
            } else {
                vec![("reasoning", effort.to_owned())]
            };
            return (format!("{base}{lane}"), params);
        }
    }
    (id.to_owned(), vec![])
}

/// System prompt entries: `req.system` plus any `System` role messages.
fn systems(req: &ChatRequest) -> Vec<&str> {
    req.system
        .as_deref()
        .into_iter()
        .chain(
            req.messages
                .iter()
                .filter(|m| m.role == ChatRole::System)
                .flat_map(|m| m.content.texts()),
        )
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// The `RequestContext` the server asks for over the exec channel: system
/// prompts as global `.mdc` rules, every genai tool as an MCP tool.
pub fn request_context(req: &ChatRequest) -> proto::RequestContext {
    let rules = systems(req)
        .into_iter()
        .enumerate()
        .map(|(i, content)| proto::CursorRule {
            full_path: format!("/t/system-prompt/{i}.mdc"),
            content: content.to_owned(),
            r#type: Some(proto::CursorRuleType {
                r#type: Some(proto::cursor_rule_type::Type::Global(
                    proto::CursorRuleTypeGlobal {},
                )),
            }),
            source: proto::CursorRuleSource::User as i32,
            ..Default::default()
        })
        .collect();
    let tools = req.tools.iter().flatten().map(mcp_tool).collect();
    proto::RequestContext {
        rules,
        tools,
        ..Default::default()
    }
}

fn mcp_tool(t: &Tool) -> proto::McpToolDefinition {
    let name = t.name.as_str().to_owned();
    let schema = match &t.schema {
        Some(s @ Value::Object(_)) => s.clone(),
        _ => json!({"type": "object", "properties": {}, "required": []}),
    };
    proto::McpToolDefinition {
        name: name.clone(),
        provider_identifier: PROVIDER.to_owned(),
        tool_name: name,
        description: t.description.clone().unwrap_or_default(),
        input_schema: encode_json(&schema),
        input_schema_json: None,
    }
}

/// History as omp's root-prompt JSON shapes, one blob per message, excluding
/// `messages[active..]`. Tool results pair with a prior `tool-call` by id;
/// orphans become assistant text so the server never sees a dangling result.
pub fn root_prompt_json(req: &ChatRequest, active: usize) -> Vec<Value> {
    let mut out: Vec<Value> = match systems(req).as_slice() {
        [] => vec![json!({"role": "system", "content": DEFAULT_SYSTEM})],
        s => s
            .iter()
            .map(|c| json!({"role": "system", "content": c}))
            .collect(),
    };
    let names = call_names(req, active);
    for m in &req.messages[..active] {
        match m.role {
            ChatRole::System => {}
            ChatRole::User => {
                let parts: Vec<Value> = m
                    .content
                    .texts()
                    .into_iter()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| json!({"type": "text", "text": t}))
                    .collect();
                if !parts.is_empty() {
                    out.push(json!({"role": "user", "content": parts}));
                }
            }
            ChatRole::Assistant => {
                let parts: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text(t) if !t.is_empty() => Some(json!({"type": "text", "text": t})),
                        ContentPart::ToolCall(c) => Some(json!({
                            "type": "tool-call",
                            "toolCallId": tool_call_id(&c.call_id),
                            "toolName": c.fn_name,
                            "args": if c.fn_arguments.is_object() { c.fn_arguments.clone() } else { json!({}) },
                        })),
                        _ => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    out.push(json!({"role": "assistant", "content": parts}));
                }
            }
            ChatRole::Tool => {
                for p in m.content.iter() {
                    let ContentPart::ToolResponse(r) = p else {
                        continue;
                    };
                    let Some(name) = names
                        .get(r.call_id.as_str())
                        .copied()
                        .or(r.fn_name.as_ref())
                    else {
                        out.push(json!({"role": "assistant", "content": [{"type": "text", "text": orphan(&r.content)}]}));
                        continue;
                    };
                    let id = tool_call_id(&r.call_id);
                    out.push(json!({
                        "role": "tool",
                        "id": id,
                        "content": [{"type": "tool-result", "toolName": name, "toolCallId": id, "result": r.content}],
                    }));
                }
            }
        }
    }
    out
}

fn orphan(text: &str) -> String {
    format!(
        "[Tool Result]\n{}",
        if text.is_empty() {
            "(empty result)"
        } else {
            text
        }
    )
}

/// call id → fn name for every assistant tool call before `active`.
fn call_names(req: &ChatRequest, active: usize) -> HashMap<&str, &String> {
    req.messages[..active]
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .flat_map(|m| m.content.tool_calls())
        .map(|c| (c.call_id.as_str(), &c.fn_name))
        .collect()
}

/// Deterministic message id so unchanged turns hash to the same blob.
fn turn_id(index: usize, text: &str) -> String {
    let h = Sha256::digest(format!("u:{index}:{text}"));
    uuid::Uuid::from_slice(&h[..16])
        .map(|u| u.to_string())
        .unwrap_or_default()
}

fn user_message(text: String, message_id: String) -> proto::UserMessage {
    proto::UserMessage {
        text,
        message_id,
        ..Default::default()
    }
}

fn step(m: proto::conversation_step::Message) -> Vec<u8> {
    proto::ConversationStep { message: Some(m) }.encode_to_vec()
}

/// `ConversationTurnStructure` blob ids: one turn per user message with the
/// assistant steps (text, MCP tool call + result) that followed it.
fn turns(req: &ChatRequest, active: usize, blobs: &mut Blobs) -> Vec<Vec<u8>> {
    let names = call_names(req, active);
    let results: HashMap<&str, &String> = req.messages[..active]
        .iter()
        .filter(|m| m.role == ChatRole::Tool)
        .flat_map(|m| m.content.iter())
        .filter_map(|p| match p {
            ContentPart::ToolResponse(r) => Some((r.call_id.as_str(), &r.content)),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < active {
        let m = &req.messages[i];
        i += 1;
        if m.role != ChatRole::User {
            continue;
        }
        let text = m.content.texts().join("\n").trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let user = store(
            blobs,
            user_message(text.clone(), turn_id(out.len(), &text)).encode_to_vec(),
        );
        let mut steps = Vec::new();
        while i < active && req.messages[i].role != ChatRole::User {
            for p in req.messages[i].content.iter() {
                let msg = match (&req.messages[i].role, p) {
                    (ChatRole::Assistant, ContentPart::Text(t)) if !t.is_empty() => {
                        proto::conversation_step::Message::AssistantMessage(
                            proto::AssistantMessage { text: t.clone() },
                        )
                    }
                    (ChatRole::Assistant, ContentPart::ToolCall(c)) => {
                        let id = tool_call_id(&c.call_id);
                        let args = c
                            .fn_arguments
                            .as_object()
                            .map(|o| o.iter().map(|(k, v)| (k.clone(), encode_json(v))).collect())
                            .unwrap_or_default();
                        let result =
                            results
                                .get(c.call_id.as_str())
                                .map(|text| proto::McpToolResult {
                                    result: Some(proto::mcp_tool_result::Result::Success(
                                        proto::McpSuccess {
                                            content: vec![proto::McpToolResultContentItem {
                                            content: Some(
                                                proto::mcp_tool_result_content_item::Content::Text(
                                                    proto::McpTextContent {
                                                        text: (*text).clone(),
                                                        output_location: None,
                                                    },
                                                ),
                                            ),
                                        }],
                                            is_error: false,
                                        },
                                    )),
                                });
                        proto::conversation_step::Message::ToolCall(proto::ToolCall {
                            tool: Some(proto::tool_call::Tool::McpToolCall(proto::McpToolCall {
                                args: Some(proto::McpArgs {
                                    name: c.fn_name.clone(),
                                    args,
                                    tool_call_id: id.clone(),
                                    provider_identifier: PROVIDER.to_owned(),
                                    tool_name: c.fn_name.clone(),
                                    ..Default::default()
                                }),
                                result,
                                description: None,
                            })),
                            tool_call_id: Some(id),
                        })
                    }
                    (ChatRole::Tool, ContentPart::ToolResponse(r))
                        if !names.contains_key(r.call_id.as_str()) =>
                    {
                        proto::conversation_step::Message::AssistantMessage(
                            proto::AssistantMessage {
                                text: orphan(&r.content),
                            },
                        )
                    }
                    _ => continue,
                };
                steps.push(store(blobs, step(msg)));
            }
            i += 1;
        }
        let turn = proto::ConversationTurnStructure {
            turn: Some(
                proto::conversation_turn_structure::Turn::AgentConversationTurn(
                    proto::AgentConversationTurnStructure {
                        user_message: user,
                        steps,
                        request_id: None,
                    },
                ),
            ),
        };
        out.push(store(blobs, turn.encode_to_vec()));
    }
    out
}

/// Index of the message that becomes `user_message_action`: the last one when
/// it is a user message, else `messages.len()` (resume after tool results).
fn active_index(req: &ChatRequest) -> usize {
    match req.messages.last() {
        Some(m) if m.role == ChatRole::User && !m.content.texts().join("").trim().is_empty() => {
            req.messages.len() - 1
        }
        _ => req.messages.len(),
    }
}

/// The complete `AgentRunRequest` plus the blobs it references.
pub fn run_request(
    model: &str,
    req: &ChatRequest,
    conversation: &str,
) -> (proto::AgentRunRequest, Blobs) {
    let mut blobs = Blobs::new();
    let active = active_index(req);
    let root_prompt_messages_json = root_prompt_json(req, active)
        .iter()
        .map(|v| store(&mut blobs, serde_json::to_vec(v).unwrap_or_default()))
        .collect();
    let turns = turns(req, active, &mut blobs);
    let action = match req.messages.get(active) {
        Some(m) => Action::UserMessageAction(proto::UserMessageAction {
            user_message: Some(user_message(
                m.content.texts().join("\n").trim().to_owned(),
                uuid::Uuid::new_v4().to_string(),
            )),
            request_context: None,
            send_to_interaction_listener: None,
        }),
        None => Action::ResumeAction(proto::ResumeAction {
            request_context: None,
        }),
    };
    let (wire, params) = wire_model(model);
    // ponytail: `max_mode` really comes from `GetUsableModels`; the slug
    // suffix is the only signal a bare model id carries. Thread `Model` through
    // if a `-max` slug ever reports `max_mode: false`.
    let max_mode = model.ends_with("-max");
    let run = proto::AgentRunRequest {
        conversation_state: Some(proto::ConversationStateStructure {
            root_prompt_messages_json,
            turns,
            ..Default::default()
        }),
        action: Some(proto::ConversationAction {
            action: Some(action),
        }),
        model_details: Some(proto::ModelDetails {
            model_id: wire.clone(),
            display_model_id: model.to_owned(),
            display_name: model.to_owned(),
            max_mode: max_mode.then_some(true),
            ..Default::default()
        }),
        requested_model: Some(proto::RequestedModel {
            model_id: wire,
            max_mode,
            parameters: params
                .into_iter()
                .map(|(id, value)| proto::RequestedModelModelParameterbytes {
                    id: id.to_owned(),
                    value,
                })
                .collect(),
            credentials: None,
        }),
        conversation_id: Some(conversation.to_owned()),
        ..Default::default()
    };
    (run, blobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::{ChatMessage, ToolCall, ToolResponse};

    #[test]
    fn wire_model_table() {
        let cases = [
            (
                "gpt-5.4-mini-low",
                "gpt-5.4-mini",
                vec![("reasoning", "low")],
            ),
            (
                "gpt-5.6-sol-xhigh-fast",
                "gpt-5.6-sol-fast",
                vec![("reasoning", "xhigh")],
            ),
            ("gpt-5.4-none", "gpt-5.4", vec![]),
            ("gpt-5.4", "gpt-5.4", vec![]),
            ("composer-2.5", "composer-2.5", vec![("fast", "false")]),
            ("composer-2.5-fast", "composer-2.5-fast", vec![]),
            ("claude-4.6-opus-high", "claude-4.6-opus-high", vec![]),
            ("cursor-grok-4.6-xhigh", "cursor-grok-4.6-xhigh", vec![]),
        ];
        for (input, id, params) in cases {
            let (got_id, got_params) = wire_model(input);
            assert_eq!(got_id, id, "{input}");
            let got: Vec<(&str, &str)> = got_params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            assert_eq!(got, params, "{input}");
        }
    }

    #[test]
    fn tool_call_id_charset() {
        assert_eq!(tool_call_id("call|abc.1"), "call_abc_1");
        assert_eq!(tool_call_id(&"x".repeat(80)).len(), 64);
    }

    #[test]
    fn json_value_round_trip() {
        let v = json!({"a": [1, "two", null, true], "b": {"c": 2.5}});
        assert_eq!(decode_arg(&encode_json(&v)), v);
        // Double-encoded structured arg and raw text fallbacks.
        assert_eq!(
            decode_arg(&encode_json(&json!("{\"k\":1}"))),
            json!({"k": 1})
        );
        assert_eq!(decode_arg(b"plain"), json!("plain"));
    }

    fn req() -> ChatRequest {
        ChatRequest {
            system: Some("Be terse.".into()),
            messages: vec![
                ChatMessage::user("hi"),
                ChatMessage::from(vec![ToolCall {
                    call_id: "call|1".into(),
                    fn_name: "read".into(),
                    fn_arguments: json!({"path": "a.rs"}),
                    thought_signatures: None,
                }]),
                ChatMessage::from(ToolResponse::new("call|1", "fn main(){}")),
                ChatMessage::from(ToolResponse::new("nope", "lost")),
                ChatMessage::assistant("done"),
                ChatMessage::user("next"),
            ],
            tools: None,
            ..Default::default()
        }
    }

    #[test]
    fn root_prompt_shapes_and_action_split() {
        let r = req();
        let active = active_index(&r);
        assert_eq!(active, 5);
        let json = root_prompt_json(&r, active);
        assert_eq!(json[0], json!({"role": "system", "content": "Be terse."}));
        assert_eq!(
            json[1],
            json!({"role": "user", "content": [{"type": "text", "text": "hi"}]})
        );
        assert_eq!(
            json[2],
            json!({"role": "assistant", "content": [{"type": "tool-call", "toolCallId": "call_1", "toolName": "read", "args": {"path": "a.rs"}}]})
        );
        assert_eq!(
            json[3],
            json!({"role": "tool", "id": "call_1", "content": [{"type": "tool-result", "toolName": "read", "toolCallId": "call_1", "result": "fn main(){}"}]})
        );
        assert_eq!(json[4]["role"], "assistant");
        assert_eq!(json[4]["content"][0]["text"], "[Tool Result]\nlost");
        assert_eq!(
            json[5],
            json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]})
        );
        assert_eq!(json.len(), 6);

        let (run, blobs) = run_request("gpt-5.4-high", &r, "conv");
        let state = run.conversation_state.unwrap();
        assert_eq!(state.root_prompt_messages_json.len(), 6);
        assert_eq!(state.turns.len(), 1);
        assert!(
            state
                .root_prompt_messages_json
                .iter()
                .chain(&state.turns)
                .all(|id| blobs.contains_key(id))
        );
        let Some(Action::UserMessageAction(a)) = run.action.unwrap().action else {
            panic!("expected user message action")
        };
        assert_eq!(a.user_message.unwrap().text, "next");
        let rm = run.requested_model.unwrap();
        assert_eq!(rm.model_id, "gpt-5.4");
        assert_eq!(rm.parameters[0].value, "high");

        // Trailing tool result → resume with the whole history.
        let mut r2 = r;
        r2.messages.pop();
        r2.messages.pop();
        let (run, _) = run_request("m", &r2, "conv");
        assert!(matches!(
            run.action.unwrap().action,
            Some(Action::ResumeAction(_))
        ));
        assert_eq!(
            run.conversation_state
                .unwrap()
                .root_prompt_messages_json
                .len(),
            5
        );
    }

    #[test]
    fn request_context_rules_and_tools() {
        let mut r = req();
        r.tools = Some(vec![Tool::new("grep".to_owned()).with_schema(
            json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        )]);
        let ctx = request_context(&r);
        assert_eq!(ctx.rules[0].full_path, "/t/system-prompt/0.mdc");
        assert_eq!(ctx.rules[0].content, "Be terse.");
        assert_eq!(ctx.tools[0].provider_identifier, PROVIDER);
        assert_eq!(
            decode_arg(&ctx.tools[0].input_schema)["properties"]["q"]["type"],
            "string"
        );
    }
}
