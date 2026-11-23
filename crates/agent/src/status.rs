//! Loop-local `set_status` tool: the agent's own one-line status and, optionally, its
//! session title (design 212 §8–§10).

use runtime::SessionId;
use schemars::JsonSchema;
use serde::Deserialize;
use tools::{Risk, ToolOutput, ToolSpec};

use crate::store::Store;

/// Tool name.
pub const NAME: &str = "set_status";
/// Longest status line, in chars, after whitespace is normalized; longer is cut at a word
/// boundary with `…`.
const STATUS_MAX: usize = 80;
/// Longest title, in chars, after whitespace is normalized; longer is cut at the last word
/// boundary within it, without `…`.
const TITLE_MAX: usize = 48;
/// The refusal in a session that does not keep a status (the dreaming one).
pub const REFUSED: &str = "set_status: not available in this session";
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

/// Validate and normalize the arguments of one call (direct or from python): runs of
/// whitespace become one space before the checks and the cuts.
pub fn parse(args: serde_json::Value) -> Result<StatusUpdate, String> {
    let a: StatusArgs =
        serde_json::from_value(args).map_err(|e| format!("set_status: bad arguments: {e}"))?;
    let status = normalize(&a.status);
    if status.is_empty() {
        return Err("set_status: status must not be empty".into());
    }
    let title = match a.title.as_deref().map(normalize) {
        None => None,
        Some(t) if t.is_empty() => None,
        Some(t) => {
            let words = t.split(' ').count();
            if !(2..=6).contains(&words) {
                return Err("set_status: title must be 3–5 words".into());
            }
            Some(clip_words(&t, TITLE_MAX))
        }
    };
    Ok(StatusUpdate {
        status: clip(&status, STATUS_MAX),
        title,
    })
}

/// What one call did with the title it asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TitleOutcome {
    /// No title was asked for.
    None,
    /// Written to the store.
    Saved(String),
    /// Refused: the user named the session.
    Kept,
    /// The store write failed.
    NotSaved,
}

/// One applied call: the status line and its title's fate. The loop publishes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The status line.
    pub status: String,
    /// The title's fate.
    pub title: TitleOutcome,
}

/// What a `set_status` call needs from its session (212 §8): the store a title goes to
/// and the session's state when the call runs.
#[derive(Clone)]
pub(crate) struct Cx {
    /// The session's store.
    pub store: Store,
    /// The session.
    pub session: SessionId,
    /// The user named the session: a title is refused.
    pub user_titled: bool,
    /// The session keeps no status (the dreaming one): the call is refused.
    pub off: bool,
}

/// Run one call: validate, write the title unless the user named the session, and answer
/// with the receipt. `Some(applied)` for the loop to apply after a successful call.
pub(crate) async fn call(cx: &Cx, args: serde_json::Value) -> (ToolOutput, Option<Applied>) {
    if cx.off {
        return (ToolOutput::err(REFUSED), None);
    }
    let u = match parse(args) {
        Ok(u) => u,
        Err(e) => return (ToolOutput::err(e), None),
    };
    let title = match u.title {
        None => TitleOutcome::None,
        Some(_) if cx.user_titled => TitleOutcome::Kept,
        Some(t) => match cx.store.set_title(cx.session, &t).await {
            Ok(()) => TitleOutcome::Saved(t),
            Err(e) => {
                tracing::warn!(session = %cx.session, "set_status: set_title: {e:#}");
                TitleOutcome::NotSaved
            }
        },
    };
    let applied = Applied {
        status: u.status,
        title,
    };
    (ToolOutput::ok(receipt(&applied.title)), Some(applied))
}

/// The result text of a successful call (§8).
pub fn receipt(title: &TitleOutcome) -> String {
    match title {
        TitleOutcome::None => "status set".into(),
        TitleOutcome::Saved(t) => format!("status set · title \"{t}\""),
        TitleOutcome::Kept => "status set · title kept: the user named this session".into(),
        TitleOutcome::NotSaved => "status set · title not saved".into(),
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

/// Runs of whitespace as one space, trimmed.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `s` (normalized) cut to `max` chars at a word boundary, with `…` when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max - 1).collect();
    let cut = match head.rfind(' ') {
        Some(i) if i > 0 => &head[..i],
        _ => &head[..],
    };
    format!("{}…", cut.trim_end())
}

/// `s` (normalized) cut to at most `max` chars at the last word boundary within them, no
/// `…`; a single word longer than `max` is cut at `max`.
fn clip_words(s: &str, max: usize) -> String {
    let Some((end, _)) = s.char_indices().nth(max) else {
        return s.to_owned();
    };
    // The last word ends exactly at the limit.
    if s[end..].starts_with(' ') {
        return s[..end].to_owned();
    }
    match s[..end].rfind(' ') {
        Some(i) if i > 0 => s[..i].to_owned(),
        _ => s[..end].to_owned(),
    }
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
        // Whitespace runs are one space before the checks and the cuts.
        let u = parse(json!({"status": " reading\n\tthe   lock ", "title": "Fix\nthe \t lock"}))
            .unwrap();
        assert_eq!(u.status, "reading the lock");
        assert_eq!(u.title.as_deref(), Some("Fix the lock"));
        // A long title is cut at the last word boundary within 48 chars, no ellipsis.
        let long = "Reconcile overlapping quarterly infrastructure procurement budgets";
        let u = parse(json!({"status": "x", "title": long})).unwrap();
        assert_eq!(
            u.title.as_deref(),
            Some("Reconcile overlapping quarterly infrastructure")
        );
        // A word ending exactly at the limit stays whole.
        assert_eq!(
            clip_words(&"abcd ".repeat(10), 49),
            "abcd ".repeat(10).trim_end()
        );
        assert_eq!(clip_words("aaaa bbbb cccc", 9), "aaaa bbbb");
        assert_eq!(clip_words("aaaa bbbb cccc", 8), "aaaa");
        assert_eq!(clip_words("aaaaaaaaaa b", 4), "aaaa");
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
        assert_eq!(receipt(&TitleOutcome::None), "status set");
        assert_eq!(
            receipt(&TitleOutcome::Saved("Fix the lock".into())),
            "status set · title \"Fix the lock\""
        );
        assert_eq!(
            receipt(&TitleOutcome::Kept),
            "status set · title kept: the user named this session"
        );
        assert_eq!(
            receipt(&TitleOutcome::NotSaved),
            "status set · title not saved"
        );
    }

    /// `call`: refused in the dreaming session with nothing applied; a user-named session
    /// keeps its title; otherwise the title is stored before the receipt claims it.
    #[tokio::test]
    async fn call_refuses_keeps_and_saves() {
        let scratch = crate::test_dir("status-call");
        let store = Store::open(&scratch.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        let session = SessionId::new();
        store
            .create_session(session, "m", None, None, None)
            .await
            .unwrap();
        store.set_title(session, "Mine").await.unwrap();
        let cx = |user_titled, off| Cx {
            store: store.clone(),
            session,
            user_titled,
            off,
        };
        let args = json!({"status": "mapping it", "title": "Map the harness"});
        let title = || async { store.session(session).await.unwrap().unwrap().title };

        let (out, applied) = call(&cx(false, true), args.clone()).await;
        assert_eq!((out.is_error, out.content.as_str()), (true, REFUSED));
        assert_eq!(applied, None);
        assert_eq!(title().await, "Mine");

        let (out, applied) = call(&cx(true, false), args.clone()).await;
        assert_eq!(
            out.content,
            "status set · title kept: the user named this session"
        );
        assert_eq!(applied.unwrap().title, TitleOutcome::Kept);
        assert_eq!(title().await, "Mine");

        let (out, applied) = call(&cx(false, false), args).await;
        assert_eq!(out.content, "status set · title \"Map the harness\"");
        assert_eq!(
            applied,
            Some(Applied {
                status: "mapping it".into(),
                title: TitleOutcome::Saved("Map the harness".into()),
            })
        );
        assert_eq!(title().await, "Map the harness");
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
