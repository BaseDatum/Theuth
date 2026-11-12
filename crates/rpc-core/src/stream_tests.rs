//! Pure event constructors for the backlog behavior fixtures.
use runtime::SessionId;
use crate::stream::{Backlog, Next, BACKLOG, FLUSH_MS};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

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

        /// Every frame until `Wait`, with the `Lagged` counts in place.
        fn drain(b: &mut Backlog, now: Instant) -> Vec<serde_json::Value> {
            let mut out = Vec::new();
            loop {
                match b.next(now) {
                    Next::Frame(ev) => out.push(serde_json::Value::clone(&ev)),
                    Next::Lagged(n) => {
                        out.push(serde_json::json!({ "type": "Lagged", "count": n }))
                    }
                    Next::Wait(_) => return out,
                }
            }
        }

        /// (b) Ten `TextDelta`s of one turn become one event; the `ToolCallStarted`
        /// behind them follows it; `ToolCallProgress` keeps the latest args; a
        /// non-delta of the same session splits the merge so order stays exact,
        /// while another session's deltas merge across it.
        #[test]
        fn backlog_merges_deltas_in_session_order() {
            let now = Instant::now();
            let (s, r) = (SessionId::new(), SessionId::new());
            let mut b = Backlog::default();
            for i in 0..10 {
                b.push(Arc::new(delta("TextDelta", s, "t", &i.to_string())), now);
            }
            b.push(Arc::new(delta("ReasoningDelta", s, "t", "think")), now);
            let started =
                serde_json::json!({ "type": "ToolCallStarted", "session": s, "turn": "t" });
            b.push(Arc::new(started.clone()), now);
            b.push(Arc::new(progress(s, "c", "{\"a")), now);
            b.push(Arc::new(progress(s, "c", "{\"a\":1}")), now);
            b.push(Arc::new(progress(s, "d", "{}")), now);
            b.push(Arc::new(delta("TextDelta", r, "u", "p")), now);
            b.push(Arc::new(title(s, "x")), now);
            b.push(Arc::new(delta("TextDelta", r, "u", "q")), now);
            b.push(Arc::new(delta("TextDelta", s, "t", "z")), now);
            let out = drain(&mut b, now);
            assert_eq!(out[0], delta("TextDelta", s, "t", "0123456789"));
            assert_eq!(out[1], delta("ReasoningDelta", s, "t", "think"));
            assert_eq!(out[2], started);
            assert_eq!(out[3], progress(s, "c", "{\"a\":1}"));
            assert_eq!(out[4], progress(s, "d", "{}"));
            assert_eq!(out[5], delta("TextDelta", r, "u", "pq"));
            assert_eq!(out[6], title(s, "x"));
            // Only deltas left and the batch is not due: they wait for `FLUSH_MS`.
            assert_eq!(out.len(), 7, "{out:?}");
            assert!(
                matches!(b.next(now), Next::Wait(Some(at)) if at == now + Duration::from_millis(FLUSH_MS))
            );
            let later = now + Duration::from_millis(FLUSH_MS);
            assert_eq!(drain(&mut b, later), [delta("TextDelta", s, "t", "z")]);
            assert!(matches!(b.next(later), Next::Wait(None)));
        }

        /// (d) Past `BACKLOG` non-delta events the oldest go, counted into one
        /// `Lagged` ahead of what is left; deltas never count.
        #[test]
        fn backlog_overflow_drops_oldest_and_counts() {
            let now = Instant::now();
            let s = SessionId::new();
            let mut b = Backlog::default();
            b.push(Arc::new(delta("TextDelta", s, "t", "a")), now);
            for i in 0..BACKLOG + 5 {
                b.push(Arc::new(title(s, i)), now);
            }
            for _ in 0..100 {
                b.push(Arc::new(delta("TextDelta", s, "t", "b")), now);
            }
            // Past the flush deadline so the trailing delta batch goes too.
            let out = drain(&mut b, now + Duration::from_millis(FLUSH_MS));
            assert_eq!(out[0], serde_json::json!({ "type": "Lagged", "count": 5 }));
            assert_eq!(out[1], delta("TextDelta", s, "t", "a"));
            assert_eq!(out[2], title(s, 5));
            assert_eq!(out[1 + BACKLOG], title(s, BACKLOG + 4));
            assert_eq!(
                out[2 + BACKLOG],
                delta("TextDelta", s, "t", &"b".repeat(100))
            );
            assert_eq!(out.len(), 3 + BACKLOG);
        }
