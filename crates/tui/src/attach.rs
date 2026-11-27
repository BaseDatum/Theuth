//! Shared transcript spelling of attachment markers and spoken turns.

use std::fmt::Write as _;

use agent::attach::parse;


/// A user turn for the transcript: leading markers as `⎘ <title> (<kind>)`
/// lines, then the body.
pub fn display(text: &str) -> String {
    let p = parse(text);
    if p.refs.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for r in &p.refs {
        let _ = writeln!(out, "⎘ {} ({})", r.title, r.kind);
    }
    out.push_str(p.body);
    out
}

/// 76 talk: `[spoken, N s] text` / `[spoken, N s, interrupted] text` is one
/// utterance from the live talk → `(seconds, interrupted, transcript)`.
pub fn spoken(text: &str) -> Option<(u32, bool, &str)> {
    let rest = text.strip_prefix("[spoken, ")?;
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let secs: u32 = rest[..digits].parse().ok()?;
    let rest = rest[digits..].strip_prefix(" s")?;
    let (rest, interrupted) = match rest.strip_prefix(", interrupted") {
        Some(r) => (r, true),
        None => (rest, false),
    };
    Some((secs, interrupted, rest.strip_prefix("] ")?))
}

/// The transcript tag for a spoken turn: `spoken 5 s` / `spoken 5 s · cut in`.
pub fn spoken_tag(secs: u32, interrupted: bool) -> String {
    if interrupted {
        format!("spoken {secs} s · cut in")
    } else {
        format!("spoken {secs} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::attach::marker;
    use agent::{SessionSummary, attach::{AttachRef, Kind}};
    use chrono::Utc;
    use crate::components::search_attach::attach::{date, from_session};

    #[test]
    fn sizes_and_display() {
        let r = AttachRef {
            kind: Kind::Artifact,
            id: "toolu_01".into(),
            title: "Q3 report".into(),
            meta: "pdf · v2".into(),
        };
        let text = format!("{}\ncheck it", marker(&r));
        assert_eq!(display(&text), "⎘ Q3 report (artifact)\ncheck it");
        assert_eq!(
            display("plain\n[attached note a \"b\"]"),
            "plain\n[attached note a \"b\"]"
        );
    }

    #[test]
    fn spoken_marker() {
        assert_eq!(spoken("[spoken, 5 s] hello"), Some((5, false, "hello")));
        assert_eq!(
            spoken("[spoken, 12 s, interrupted] no wait\nmore"),
            Some((12, true, "no wait\nmore"))
        );
        assert_eq!(spoken("[spoken, x s] hello"), None);
        assert_eq!(spoken("hello [spoken, 5 s]"), None);
        assert_eq!(spoken_tag(5, false), "spoken 5 s");
        assert_eq!(spoken_tag(5, true), "spoken 5 s · cut in");
    }

    #[test]
    fn session_fallbacks_follow_the_naming_table() {
        let now = Utc::now();
        let s = SessionSummary {
            id: runtime::SessionId::new(),
            created_at: now,
            updated_at: now,
            finished_at: None,
            title: String::new(),
            tags: vec![],
            model: "m".into(),
            parent: None,
            name: None,
            effort: None,
            agent: None,
            cwd: None,
            harness_model: None,
            context_size: None,
            desk: None,
            pinned: false,
            archived: true,
            project: None,
            notebook: None,
            retention: tools::Retention::Durable,
            running: false,
            owner: String::new(),
            epoch: 1,
            here: true,
            kind: Default::default(),
            memory: Default::default(),
            observed: None,
            mesh_id: None,
        };
        let r = from_session(&s);
        assert_eq!(r.kind, Kind::Session);
        assert!(r.title.starts_with("Untitled chat · "), "{}", r.title);
        assert_eq!(
            r.meta,
            format!("chat · no project · updated {} · archived", date(now))
        );
    }
}
