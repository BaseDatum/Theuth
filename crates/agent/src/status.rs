//! Loop-local `set_status` tool: the agent's own one-line status and, optionally, its
//! session title (design 212 §8–§10).

use schemars::JsonSchema;
use serde::Deserialize;
use tools::{Risk, ToolSpec};

/// Tool name.
pub const NAME: &str = "set_status";
/// Longest status line, in chars, after trim; longer is cut at a word boundary with `…`.
const STATUS_MAX: usize = 80;
/// Longest title, in chars, after trim; longer is cut at a word boundary with `…`.
const TITLE_MAX: usize = 48;
/// Tool calls without a status before the first nudge of a turn (§10).
pub const FIRST_NUDGE_AFTER: u32 = 2;
/// The tool-guidance sentence of the system prompt (§10).
pub const PROMPT: &str = "Keep the user informed: call set_status when you start a new phase of work; the harness reminds you every few tool calls.";
/// The nudge's first sentence; the whole text when the user named the session.
const NUDGE_STATUS: &str =
    "Status check: call set_status(status=\"…\") with what you are doing now, 3–12 words.";
/// The nudge's title sentence, left out when the user named the session.
const NUDGE_TITLE: &str = " Add title=\"…\" (3–5 words) if this session's overall task changed or its title no longer says it.";

/// Arguments of the `set_status` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatusArgs {
    /// What you are doing right now, 3–12 words (at most 80 characters).
    pub status: String,
    /// The session's overall task in 3–5 words (at most 48 characters); omit unless the
    /// task changed or the current title no longer says it.
    #[serde(default)]
    pub title: Option<String>,
}

/// A validated call: the status line and the title to apply, both trimmed and cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusUpdate {
    /// The status line.
    pub status: String,
    /// The new title, when one was given.
    pub title: Option<String>,
}

/// Validate and normalize the arguments of one call (direct or from python).
pub fn parse(args: serde_json::Value) -> Result<StatusUpdate, String> {
    let a: StatusArgs =
        serde_json::from_value(args).map_err(|e| format!("set_status: bad arguments: {e}"))?;
    let status = a.status.trim();
    if status.is_empty() {
        return Err("set_status: status must not be empty".into());
    }
    let title = match a.title.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(t) => {
            let words = t.split_whitespace().count();
            if !(2..=6).contains(&words) {
                return Err("set_status: title must be 3–5 words".into());
            }
            Some(clip(t, TITLE_MAX))
        }
    };
    Ok(StatusUpdate {
        status: clip(status, STATUS_MAX),
        title,
    })
}

/// The result text of a successful call (§8). `title` is what the call asked for;
/// `user_titled` = it was refused because the user named the session.
pub fn receipt(title: Option<&str>, user_titled: bool) -> String {
    match title {
        None => "status set".into(),
        Some(_) if user_titled => "status set · title kept: the user named this session".into(),
        Some(t) => format!("status set · title \"{t}\""),
    }
}

/// Whether the round prelude nudges now (§10): the first nudge of a turn after
/// [`FIRST_NUDGE_AFTER`] calls while `quiet_turn` (no `set_status` and no nudge yet
/// this turn), then every `every` calls. `every == 0` is off.
pub fn nudge_due(since_status: u32, quiet_turn: bool, every: u32) -> bool {
    if every == 0 {
        return false;
    }
    if quiet_turn && since_status >= FIRST_NUDGE_AFTER {
        return true;
    }
    since_status >= every
}

/// The nudge text (§10): the title sentence only when the user has not named the session.
pub fn nudge_text(user_titled: bool) -> String {
    if user_titled {
        NUDGE_STATUS.to_owned()
    } else {
        format!("{NUDGE_STATUS}{NUDGE_TITLE}")
    }
}

/// `s` (already trimmed) cut to `max` chars at a word boundary, with `…` when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max - 1).collect();
    let cut = match head.rfind(char::is_whitespace) {
        Some(i) if i > 0 => &head[..i],
        _ => &head[..],
    };
    format!("{}…", cut.trim_end())
}

/// Spec of the loop-local `set_status` tool.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: "Update the one-line status the user sees above the chat input while you \
            work (3–12 words, what you are doing now), and optionally retitle this session \
            (3–5 words, the overall task). Call it when you start a new phase of work and \
            whenever the harness asks for a status check."
            .into(),
        schema: serde_json::to_value(schemars::schema_for!(StatusArgs)).unwrap_or_default(),
        risk: Risk::ReadOnly,
        direct: true,
        returns: "str".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_trims_cuts_and_validates() {
        let u = parse(json!({"status": "  reading the build lock code  "})).unwrap();
        assert_eq!(u.status, "reading the build lock code");
        assert_eq!(u.title, None);
        assert_eq!(
            parse(json!({"status": "  "})).unwrap_err(),
            "set_status: status must not be empty"
        );
        let long = "word ".repeat(30);
        let u = parse(json!({"status": long})).unwrap();
        assert!(u.status.chars().count() <= STATUS_MAX, "{}", u.status);
        assert!(u.status.ends_with("word…"), "{}", u.status);
        let u = parse(json!({"status": "x", "title": " Fix the build lock "})).unwrap();
        assert_eq!(u.title.as_deref(), Some("Fix the build lock"));
        assert_eq!(
            parse(json!({"status": "x", "title": "Lock"})).unwrap_err(),
            "set_status: title must be 3–5 words"
        );
        assert_eq!(
            parse(json!({"status": "x", "title": "a b c d e f g"})).unwrap_err(),
            "set_status: title must be 3–5 words"
        );
        assert_eq!(
            parse(json!({"status": "x", "title": " "})).unwrap().title,
            None
        );
    }

    #[test]
    fn receipts_are_exact() {
        assert_eq!(receipt(None, false), "status set");
        assert_eq!(receipt(None, true), "status set");
        assert_eq!(
            receipt(Some("Fix the lock"), false),
            "status set · title \"Fix the lock\""
        );
        assert_eq!(
            receipt(Some("Fix the lock"), true),
            "status set · title kept: the user named this session"
        );
    }

    #[test]
    fn nudge_cadence() {
        // First nudge of a turn after 2 calls without any status or nudge.
        assert!(!nudge_due(1, true, 8));
        assert!(nudge_due(2, true, 8));
        // After a status or the first nudge, only every `status_every`.
        assert!(!nudge_due(2, false, 8));
        assert!(!nudge_due(7, false, 8));
        assert!(nudge_due(8, false, 8));
        // Off.
        assert!(!nudge_due(100, true, 0));
        assert_eq!(
            nudge_text(true),
            "Status check: call set_status(status=\"…\") with what you are doing now, 3–12 words."
        );
        assert_eq!(
            nudge_text(false),
            "Status check: call set_status(status=\"…\") with what you are doing now, 3–12 words. Add title=\"…\" (3–5 words) if this session's overall task changed or its title no longer says it."
        );
    }
}
