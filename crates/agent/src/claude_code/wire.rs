//! The request body an Anthropic subscription (OAuth) call must have to be
//! billed to the plan rather than refused as a third-party app (a bare 429
//! `rate_limit_error` "Error" with the plan nowhere near its limits):
//!
//! - the identity sentence as the **first system block, on its own** (joined
//!   into one string with theuth's prompt it does not count);
//! - every tool named `mcp__<name>`, as Claude Code names the tools it did
//!   not ship with, in the tool list and in replayed `tool_use` blocks;
//! - optional literal replacements in the system text.
//!
//! [`Wire::outgoing`] applies all three to a request just before it is sent
//! ([`crate::backend`]); [`Names::event`] gives the stream back theuth's own
//! tool names. Each rule is a field of `claude-code.json` (see the parent
//! module), so a changed requirement is a file edit before it is a release.
//! See docs/design/cc-version-notes.md.

use std::collections::HashMap;

use genai::chat::{
    CacheControl, ChatMessage, ChatRequest, ChatRole, ChatStreamEvent, ContentPart, ToolName,
};

/// The system block the plan requires first.
pub(crate) const DEFAULT_SYSTEM: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
/// The prefix every tool name goes out with.
pub(crate) const DEFAULT_TOOL_PREFIX: &str = "mcp__";

/// How a subscription request is shaped.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Wire {
    /// The first system block, alone; empty = none.
    pub(crate) system: String,
    /// Prepended to each tool name lacking it; empty = names as they are.
    pub(crate) tool_prefix: String,
    /// `(from, to)` literal replacements in every system text, in order.
    pub(crate) replace: Vec<(String, String)>,
}

impl Default for Wire {
    fn default() -> Self {
        Self {
            system: DEFAULT_SYSTEM.to_owned(),
            tool_prefix: DEFAULT_TOOL_PREFIX.to_owned(),
            replace: Vec::new(),
        }
    }
}

impl Wire {
    /// `req` as the plan accepts it, and the tool names to restore on the reply.
    pub(crate) fn outgoing(&self, mut req: ChatRequest) -> (ChatRequest, Names) {
        // genai sends `req.system` ahead of every system message; as a
        // message it can follow the identity block.
        let mut messages = Vec::with_capacity(req.messages.len() + 2);
        if !self.system.is_empty() {
            messages.push(ChatMessage::system(self.system.as_str()));
        }
        messages.extend(req.system.take().map(ChatMessage::system));
        messages.append(&mut req.messages);
        for m in messages.iter_mut().filter(|m| m.role == ChatRole::System) {
            for part in &mut m.content {
                if let ContentPart::Text(t) = part {
                    for (from, to) in &self.replace {
                        *t = t.replace(from.as_str(), to);
                    }
                }
            }
        }
        // genai joins the system blocks into one string unless one carries a
        // cache mark; the frame marks its system message, other callers may not.
        let systems = messages.iter().filter(|m| m.role == ChatRole::System);
        if systems.clone().count() > 1
            && systems.clone().all(|m| cache_mark(m).is_none())
            && let Some(last) = messages.iter_mut().rfind(|m| m.role == ChatRole::System)
        {
            last.options = Some(CacheControl::Ephemeral.into());
        }

        let mut names = Names {
            prefix: self.tool_prefix.clone(),
            own: HashMap::new(),
        };
        for tool in req.tools.iter_mut().flatten() {
            if let ToolName::Custom(name) = &mut tool.name {
                let wire = names.wire(name);
                names.own.insert(wire.clone(), std::mem::replace(name, wire));
            }
        }
        for m in &mut messages {
            for part in &mut m.content {
                match part {
                    ContentPart::ToolCall(c) => c.fn_name = names.wire(&c.fn_name),
                    ContentPart::ToolResponse(r) => {
                        if let Some(n) = &mut r.fn_name {
                            *n = names.wire(n);
                        }
                    }
                    _ => {}
                }
            }
        }
        req.messages = messages;
        (req, names)
    }
}

fn cache_mark(m: &ChatMessage) -> Option<&CacheControl> {
    m.options.as_ref()?.cache_control.as_ref()
}

/// Wire tool name → theuth's, for one request.
#[derive(Debug, Default)]
pub(crate) struct Names {
    prefix: String,
    own: HashMap<String, String>,
}

impl Names {
    fn wire(&self, name: &str) -> String {
        if self.prefix.is_empty() || name.starts_with(&self.prefix) {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        }
    }

    /// A name the model called: an offered tool's own name, else the prefix
    /// stripped (a runtime function called directly is bridged by its name).
    pub(crate) fn restore(&self, wire: &str) -> String {
        match self.own.get(wire) {
            Some(own) => own.clone(),
            None => wire
                .strip_prefix(self.prefix.as_str())
                .filter(|rest| !self.prefix.is_empty() && !rest.is_empty())
                .unwrap_or(wire)
                .to_owned(),
        }
    }

    /// `ev` with theuth's tool names.
    pub(crate) fn event(&self, mut ev: ChatStreamEvent) -> ChatStreamEvent {
        match &mut ev {
            ChatStreamEvent::ToolCallChunk(c) => {
                c.tool_call.fn_name = self.restore(&c.tool_call.fn_name);
            }
            ChatStreamEvent::End(end) => {
                for part in end.captured_content.iter_mut().flatten() {
                    if let ContentPart::ToolCall(c) = part {
                        c.fn_name = self.restore(&c.fn_name);
                    }
                }
            }
            _ => {}
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::{StreamEnd, Tool, ToolCall, ToolChunk, ToolResponse};

    fn call(name: &str) -> ToolCall {
        ToolCall {
            call_id: "c1".into(),
            fn_name: name.into(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }
    }

    fn texts(m: &ChatMessage) -> String {
        m.content.joined_texts().unwrap_or_default()
    }

    /// The frame's request: one cached system message, tools, a replayed call.
    fn frame_request() -> ChatRequest {
        ChatRequest::new(vec![
            ChatMessage::system("You are theuth.").with_options(CacheControl::Ephemeral1h),
            ChatMessage::user("hi"),
            ChatMessage::assistant(vec![ContentPart::ToolCall(call("read_file"))]),
            ChatMessage::from(ToolResponse::new("c1", "ok")),
        ])
        .with_tools(vec![Tool::new("read_file"), Tool::new("mcp__srv__get")])
    }

    #[test]
    fn identity_is_its_own_first_block() {
        let (req, _) = Wire::default().outgoing(frame_request());
        assert_eq!(req.messages[0].role, ChatRole::System);
        assert_eq!(texts(&req.messages[0]), DEFAULT_SYSTEM);
        assert!(cache_mark(&req.messages[0]).is_none());
        assert_eq!(texts(&req.messages[1]), "You are theuth.");
        assert_eq!(cache_mark(&req.messages[1]), Some(&CacheControl::Ephemeral1h));
    }

    /// `req.system` and unmarked system messages: the identity still leads,
    /// and the last system block gets a mark so genai keeps them apart.
    #[test]
    fn unmarked_system_becomes_marked_blocks() {
        let req = ChatRequest::new(vec![ChatMessage::user("hi")]).with_system("Title this.");
        let (req, _) = Wire::default().outgoing(req);
        assert!(req.system.is_none());
        assert_eq!(texts(&req.messages[0]), DEFAULT_SYSTEM);
        assert_eq!(texts(&req.messages[1]), "Title this.");
        assert_eq!(cache_mark(&req.messages[1]), Some(&CacheControl::Ephemeral));
        // No system of its own: the identity alone, unmarked (no breakpoint spent).
        let (req, _) = Wire::default().outgoing(ChatRequest::new(vec![ChatMessage::user("hi")]));
        assert_eq!(texts(&req.messages[0]), DEFAULT_SYSTEM);
        assert!(cache_mark(&req.messages[0]).is_none());
    }

    #[test]
    fn tool_names_are_prefixed_out_and_restored_back() {
        let (req, names) = Wire::default().outgoing(frame_request());
        let tools: Vec<_> = req
            .tools
            .iter()
            .flatten()
            .map(|t| t.name.as_str().to_owned())
            .collect();
        assert_eq!(tools, ["mcp__read_file", "mcp__srv__get"]);
        assert_eq!(req.messages[3].content.tool_calls()[0].fn_name, "mcp__read_file");
        assert_eq!(names.restore("mcp__read_file"), "read_file");
        assert_eq!(names.restore("mcp__srv__get"), "mcp__srv__get");
        // A runtime function called directly (not offered) loses the prefix.
        assert_eq!(names.restore("mcp__discover_peers"), "discover_peers");
        assert_eq!(names.restore("read_file"), "read_file");

        let ev = names.event(ChatStreamEvent::ToolCallChunk(ToolChunk {
            tool_call: call("mcp__read_file"),
        }));
        let ChatStreamEvent::ToolCallChunk(c) = ev else { panic!() };
        assert_eq!(c.tool_call.fn_name, "read_file");
        let end = StreamEnd {
            captured_content: Some(vec![ContentPart::ToolCall(call("mcp__read_file"))].into()),
            ..StreamEnd::default()
        };
        let ChatStreamEvent::End(end) = names.event(ChatStreamEvent::End(end)) else {
            panic!()
        };
        assert_eq!(end.captured_content.unwrap().tool_calls()[0].fn_name, "read_file");
    }

    #[test]
    fn every_rule_can_be_turned_off_or_changed() {
        let wire = Wire {
            system: String::new(),
            tool_prefix: String::new(),
            replace: vec![("theuth".into(), "an assistant".into())],
        };
        let (req, names) = wire.outgoing(frame_request());
        assert_eq!(texts(&req.messages[0]), "You are an assistant.");
        assert_eq!(req.tools.unwrap()[0].name.as_str(), "read_file");
        assert_eq!(names.restore("mcp__x"), "mcp__x");
    }
}
