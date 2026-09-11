//! Frames: what one model call sends, with per-transport cache marks (design doc 07 §2–§4).
//!
//! A [`Frame`] is a pure function of `tools → system → head → history`; the
//! only per-transport difference is where the cache marks go (§3 table).

use std::path::Path;

use genai::adapter::AdapterKind;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, ReasoningEffort, Tool,
};
use runtime::SessionId;

use crate::skills;

/// Identity line the Claude Code login requires first in the system prompt (design doc 05 §2.3).
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Static todo/peer guidance (design doc 07 §4 part 8, §9): never changes, so it
/// never busts the system cache. The live roster is deliberately absent (§10);
/// plugin usage text arrives as prompt blocks (`ListPrompts`), not here.
const GUIDANCE: &str = "## Guidance\n\
For work with three or more steps, keep a `todo` list and mark items as you go.\n\
Spawn with spawn_subagent (put all shared background in `task`; the subagent sees nothing else); \
steer or ask with send_message. Results arrive as <subagent_result> messages; you do not need to poll.";

/// How cache marks are placed for a provider family (design doc 07 §3 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Explicit breakpoints: system block (1 h) + last message (5 min).
    Anthropic,
    /// Implicit prefix caching routed by `prompt_cache_key`, 24 h retention where supported.
    OpenAiLike,
    /// Nothing to place; a stable prefix is all these providers need.
    Plain,
}

impl Transport {
    /// The transport for a genai adapter. The `cursor` namespace never reaches
    /// genai; the caller uses [`Transport::Plain`] for it.
    pub fn of(kind: AdapterKind) -> Self {
        use AdapterKind::*;
        match kind {
            Anthropic | MiniMax => Self::Anthropic,
            OpenAI | OpenAIResp | Groq | Xai | DeepSeek | Together | Fireworks | OpenRouter
            | Nebius | Moonshot | Kimi | Mimo | Zai | Aliyun | Baidu | Aihubmix | AtlasCloud
            | Omlx | OpenCodeGo => Self::OpenAiLike,
            // Gemini, Vertex, Bedrock, Copilot, Ollama, Cohere, … and any kind genai adds later.
            _ => Self::Plain,
        }
    }
}

/// One model call's input, borrowed from session state (design doc 07 §2.1).
pub struct Frame<'a> {
    /// Direct tool definitions, sorted by name.
    pub tools: &'a [Tool],
    /// The baseline system prompt, frozen for the turn.
    pub system: &'a str,
    /// The current epoch's summary (user role); `None` in epoch 0.
    pub head: Option<&'a ChatMessage>,
    /// Every message since the epoch began, verbatim, in order.
    pub history: &'a [ChatMessage],
}

impl Frame<'_> {
    /// The request and options for `t`, cache marks placed (design doc 07 §3).
    pub fn request(
        &self,
        t: Transport,
        session: SessionId,
        effort: Option<ReasoningEffort>,
    ) -> (ChatRequest, ChatOptions) {
        let mut messages = Vec::with_capacity(
            usize::from(t == Transport::Anthropic)
                + usize::from(self.head.is_some())
                + self.history.len(),
        );
        if t == Transport::Anthropic {
            messages.push(ChatMessage::system(self.system).with_options(CacheControl::Ephemeral1h));
        }
        messages.extend(self.head.cloned());
        messages.extend_from_slice(self.history);
        if t == Transport::Anthropic
            && let Some(last) = messages.last_mut()
            && last.role != ChatRole::System
        {
            last.options = Some(CacheControl::Ephemeral.into());
        }

        let mut req = ChatRequest::new(messages).with_tools(self.tools.iter().cloned());
        if t != Transport::Anthropic {
            req = req.with_system(self.system);
        }
        // Reasoning content is always captured: Anthropic's thinking blocks
        // (with signatures) must round-trip in the assistant message of a
        // tool-use turn, or the next round is rejected.
        let mut opts = ChatOptions::default()
            .with_capture_tool_calls(true)
            .with_capture_usage(true)
            .with_capture_content(true)
            .with_capture_reasoning_content(true);
        if t == Transport::OpenAiLike {
            opts = opts
                .with_prompt_cache_key(session.to_string())
                .with_cache_control(CacheControl::Ephemeral24h);
        }
        if let Some(e) = effort {
            opts = opts.with_reasoning_effort(e);
        }
        (req, opts)
    }
}

/// The system prompt (design doc 07 §4), rendered once per turn: Claude Code
/// identity (when `claude_code`), built-in header, `<data_dir>/prompt.md`,
/// `always` skill bodies, skills index, `peer_line` verbatim, `plugin_blocks`
/// (registered `RegisterPrompt` texts, already joined), static guidance.
/// Empty parts skipped, one blank line between the rest.
pub fn baseline(
    data_dir: &Path,
    peer_line: &str,
    plugin_blocks: &str,
    claude_code: bool,
) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let header = format!(
        "You are a fast, direct assistant that does things for the user on their machine.\nWorking directory: {cwd}\nDate: {}",
        chrono::Local::now().format("%Y-%m-%d")
    );
    let standing = std::fs::read_to_string(data_dir.join("prompt.md")).unwrap_or_default();
    let skills = skills::index(&skills::dir(data_dir));
    [
        if claude_code {
            CLAUDE_CODE_IDENTITY
        } else {
            ""
        },
        header.as_str(),
        &standing,
        &skills::always_bodies(&skills),
        &skills::prompt_index(&skills),
        peer_line,
        plugin_blocks,
        GUIDANCE,
    ]
    .iter()
    .map(|s| s.trim())
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

/// Char-boundary-safe prefix with a marker when cut (UI copies of tool output;
/// model-facing output goes through [`crate::spill::cap`]).
pub fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…[truncated {} chars]", &s[..i], s.len() - i),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::{ToolCall, ToolResponse};
    use serde_json::json;

    const ALL: [Transport; 3] = [
        Transport::Anthropic,
        Transport::OpenAiLike,
        Transport::Plain,
    ];

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            fn_name: name.into(),
            fn_arguments: json!({"x": id}),
            thought_signatures: None,
        }
    }

    /// The §12.1 script: user, assistant+2 tool_use, results, steering,
    /// assistant text, next user, `todo` call + result, `<todo>` note.
    fn script() -> Vec<ChatMessage> {
        vec![
            ChatMessage::user("hi"),
            ChatMessage::from(vec![call("c1", "shell"), call("c2", "read_file")]),
            ChatMessage::from(vec![
                ToolResponse::new("c1", "out1"),
                ToolResponse::new("c2", "out2"),
            ]),
            ChatMessage::user("<message from=\"user\">also check y</message>"),
            ChatMessage::assistant("done with x"),
            ChatMessage::user("now do z"),
            ChatMessage::from(vec![call("c3", "todo")]),
            ChatMessage::from(vec![ToolResponse::new(
                "c3",
                "Todo (0/2 done):\n  ▶ a\n  ☐ b",
            )]),
            ChatMessage::user("<todo>Todo (0/2 done):\n  ▶ a\n  ☐ b</todo>"),
        ]
    }

    /// `(system-or-leading-system-message, tools JSON, messages JSON without options and closing `]`)`.
    fn shape(req: &ChatRequest, t: Transport) -> (String, String, String) {
        let mut msgs = req.messages.clone();
        let system = match t {
            Transport::Anthropic => {
                let s = msgs.remove(0);
                assert_eq!(s.role, ChatRole::System);
                assert_eq!(
                    s.options.as_ref().and_then(|o| o.cache_control.clone()),
                    Some(CacheControl::Ephemeral1h)
                );
                assert!(req.system.is_none());
                serde_json::to_string(&s.content).unwrap()
            }
            _ => req.system.clone().unwrap(),
        };
        if t == Transport::Anthropic && !msgs.is_empty() {
            let marks = msgs
                .iter()
                .filter(|m| {
                    m.options.as_ref().and_then(|o| o.cache_control.as_ref())
                        == Some(&CacheControl::Ephemeral)
                })
                .count();
            assert_eq!(marks, 1, "exactly one Ephemeral mark");
            assert!(
                msgs.last().unwrap().options.is_some(),
                "mark on the last message"
            );
        }
        for m in &mut msgs {
            m.options = None;
        }
        let mut body = serde_json::to_string(&msgs).unwrap();
        assert_eq!(body.pop(), Some(']'));
        (system, serde_json::to_string(&req.tools).unwrap(), body)
    }

    fn assert_prefix_chain(head: Option<&ChatMessage>, history: &[ChatMessage]) {
        let tools = [
            Tool::new("read_file")
                .with_description("read")
                .with_schema(json!({"type": "object"})),
            Tool::new("shell")
                .with_description("run")
                .with_schema(json!({"type": "object"})),
            Tool::new("todo")
                .with_description("plan")
                .with_schema(json!({"type": "object"})),
        ];
        let session = SessionId::new();
        for t in ALL {
            let mut prev: Option<(String, String, String)> = None;
            for n in 0..=history.len() {
                let frame = Frame {
                    tools: &tools,
                    system: "baseline",
                    head,
                    history: &history[..n],
                };
                let (req, opts) = frame.request(t, session, None);
                assert_eq!(opts.capture_tool_calls, Some(true));
                let cur = shape(&req, t);
                if let Some(p) = &prev {
                    assert_eq!(p.0, cur.0, "{t:?}: system changed at {n}");
                    assert_eq!(p.1, cur.1, "{t:?}: tools changed at {n}");
                    assert!(
                        cur.2.starts_with(&p.2),
                        "{t:?}: messages not a byte prefix at {n}"
                    );
                }
                prev = Some(cur);
            }
        }
    }

    #[test]
    fn prefix_oracle_across_rounds_and_epochs() {
        let history = script();
        assert_prefix_chain(None, &history);

        // Epoch 1: summary head + the tail kept verbatim, then more rounds appended.
        let head =
            ChatMessage::user("<context_summary epoch=\"1\">\n## Goal\nz\n</context_summary>");
        let mut tail = history[5..].to_vec();
        tail.extend(script());
        assert_prefix_chain(Some(&head), &tail);
    }

    #[test]
    fn transport_marks() {
        let history = script();
        let frame = Frame {
            tools: &[],
            system: "sys",
            head: None,
            history: &history,
        };
        let session = SessionId::new();

        let (req, opts) = frame.request(Transport::OpenAiLike, session, None);
        assert_eq!(req.system.as_deref(), Some("sys"));
        assert_eq!(req.messages.len(), history.len());
        assert!(req.messages.iter().all(|m| m.options.is_none()));
        assert_eq!(
            opts.prompt_cache_key.as_deref(),
            Some(session.to_string().as_str())
        );
        assert_eq!(opts.cache_control, Some(CacheControl::Ephemeral24h));

        let (req, opts) = frame.request(Transport::Plain, session, None);
        assert_eq!(req.system.as_deref(), Some("sys"));
        assert!(opts.cache_control.is_none() && opts.prompt_cache_key.is_none());

        let (req, opts) = frame.request(Transport::Anthropic, session, None);
        assert!(req.system.is_none());
        assert_eq!(req.messages.len(), history.len() + 1);
        assert!(opts.cache_control.is_none());
        // Empty history: the system block keeps its 1 h mark; nothing gets the 5 min one.
        let empty = Frame {
            tools: &[],
            system: "sys",
            head: None,
            history: &[],
        };
        let (req, _) = empty.request(Transport::Anthropic, session, None);
        assert_eq!(
            req.messages[0].options.as_ref().unwrap().cache_control,
            Some(CacheControl::Ephemeral1h)
        );
    }
}
