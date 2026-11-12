//! Pure event constructors for the backlog behavior fixtures.
use runtime::SessionId;

/// A delta frame of the given session and turn.
pub fn delta(kind: &str, session: SessionId, turn: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"type": kind, "session": session, "turn": turn, "text": text})
}

/// Cumulative tool-call progress.
pub fn progress(session: SessionId, call: &str, args: &str) -> serde_json::Value {
    serde_json::json!({"type": "ToolCallProgress", "session": session, "turn": "t", "call_id": call, "args": args})
}

/// A non-delta event that acts as a per-session ordering barrier.
pub fn title(session: SessionId, title: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({"type": "TitleChanged", "session": session, "title": title.to_string()})
}
