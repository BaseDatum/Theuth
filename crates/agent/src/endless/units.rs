//! 137 §4: units — one per part of one `messages` row, in row order then
//! part order. Pure: the store hands in `(seq, at, ChatMessage)`, this
//! module hands back [`Unit`]s with their index continuing from `next_i`.

use chrono::{DateTime, Utc};
use genai::chat::{ChatMessage, ChatRole, ContentPart};

use super::tree::{Kind, Unit};

/// A user `Text` that starts like this is a subagent's report (8.5): the
/// peer wrapper the loop uses for a child's result (`loop_::peer_wrapper`).
pub const WORK_MARKER: &str = "<message from=\"session:";

/// Units of one row (§4 table). `excluded` rows are skipped by the caller.
pub fn derive(
    seq: i64,
    at: Option<DateTime<Utc>>,
    m: &ChatMessage,
    cap_chars: u32,
    next_i: u64,
) -> Vec<Unit> {
    let mut out = Vec::new();
    for (part, p) in m.content.iter().enumerate() {
        let (kind, text) = match (p, &m.role) {
            (ContentPart::Text(t), ChatRole::User) => {
                let kind = if t.starts_with(WORK_MARKER) {
                    Kind::Work
                } else {
                    Kind::User
                };
                (kind, t.clone())
            }
            (ContentPart::Text(t), ChatRole::Assistant) => (Kind::Talk, t.clone()),
            (ContentPart::Text(t), _) => (Kind::Echo, t.clone()),
            (ContentPart::ToolCall(c), _) => {
                let args = serde_json::to_string(&c.fn_arguments).unwrap_or_default();
                (Kind::Tool, format!("{} {args}", c.fn_name))
            }
            (ContentPart::ToolResponse(r), _) => (Kind::Echo, cap(&r.content, cap_chars)),
            (ContentPart::Binary(_), _) => (
                if m.role == ChatRole::Assistant {
                    Kind::Talk
                } else {
                    Kind::User
                },
                "[image]".to_owned(),
            ),
            (ContentPart::ReasoningContent(_) | ContentPart::ThoughtSignature(_), _) => continue,
            (ContentPart::Custom(_), _) => continue,
        };
        let size = (kind.as_str().len() + 2 + text.len()) as u32;
        out.push(Unit {
            i: next_i + out.len() as u64,
            seq,
            part: part as u16,
            kind,
            text,
            size,
            at,
        });
    }
    out
}

/// §4 echo cap: head ⅔ + `[… N chars cut]` + tail ⅓, counted in chars.
pub fn cap(s: &str, cap_chars: u32) -> String {
    let cap = cap_chars as usize;
    let n = s.chars().count();
    if n <= cap {
        return s.to_owned();
    }
    let head = cap * 2 / 3;
    let tail = cap - head;
    let cut = n - head - tail;
    let head_end = s.char_indices().nth(head).map_or(s.len(), |(b, _)| b);
    let tail_start = s.char_indices().nth(n - tail).map_or(s.len(), |(b, _)| b);
    format!(
        "{}[… {cut} chars cut]{}",
        &s[..head_end],
        &s[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::{MessageContent, ToolCall, ToolResponse};

    fn parse(json: &str) -> ChatMessage {
        serde_json::from_str(json).unwrap()
    }

    /// §4: the real row shapes — a user text, an assistant text + tool call,
    /// a tool response, reasoning dropped, a work marker, an image.
    #[test]
    fn derives_units_from_stored_row_shapes() {
        let user = parse(r#"{"role":"User","content":[{"Text":"hello there"}]}"#);
        let u = derive(0, None, &user, 30_000, 0);
        assert_eq!(u.len(), 1);
        assert_eq!((u[0].kind, u[0].text.as_str(), u[0].size), (Kind::User, "hello there", 17));
        assert_eq!(u[0].tagged(), "user: hello there");

        let asst = ChatMessage::assistant(MessageContent::from_parts(vec![
            ContentPart::ReasoningContent("thinking…".into()),
            ContentPart::Text("on it".into()),
            ContentPart::ToolCall(ToolCall {
                call_id: "c1".into(),
                fn_name: "read".into(),
                fn_arguments: serde_json::json!({"path": "a.rs", "n": 1}),
                thought_signatures: None,
            }),
        ]));
        let json = serde_json::to_string(&asst).unwrap();
        let u = derive(1, None, &parse(&json), 30_000, 1);
        assert_eq!(u.len(), 2, "reasoning dropped");
        assert_eq!((u[0].i, u[0].kind, u[0].part), (1, Kind::Talk, 1));
        assert_eq!((u[1].i, u[1].kind, u[1].part), (2, Kind::Tool, 2));
        assert_eq!(u[1].text, r#"read {"n":1,"path":"a.rs"}"#);

        let tool = ChatMessage::from(vec![ToolResponse::new("c1", "x".repeat(100))]);
        let json = serde_json::to_string(&tool).unwrap();
        let u = derive(2, None, &parse(&json), 30, 3);
        assert_eq!(u[0].kind, Kind::Echo);
        assert_eq!(u[0].text, format!("{}[… 70 chars cut]{}", "x".repeat(20), "x".repeat(10)));

        let work = parse(
            r#"{"role":"User","content":[{"Text":"<message from=\"session:abc\" id=\"1\">done</message>"}]}"#,
        );
        assert_eq!(derive(3, None, &work, 30_000, 4)[0].kind, Kind::Work);
    }

    #[test]
    fn cap_counts_chars_not_bytes() {
        let s = "é".repeat(12);
        assert_eq!(cap(&s, 12), s);
        assert_eq!(cap(&s, 9), format!("{}[… 3 chars cut]{}", "é".repeat(6), "é".repeat(3)));
    }
}
