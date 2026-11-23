//! 136 §2.2 `LedgerSince`: the day ledger's read-only aggregates over this
//! store — `sessions`, `messages`, `inner_calls`, `tool_calls`,
//! `turn_steps`, `questions`, `model_calls` — for one window. The knowledge
//! half (`hints`) is the supervisor's: this returns the ids the window's
//! `<knowledge>` notes showed and the supervisor asks knowledge `Stats`.
//!
//! Private sessions live in their own `:memory:` store (51 §4.5) and are
//! never in this one; `exclude` drops a session and its descendants;
//! `owner_only` drops every tree whose root is a desk. Advisors (125) are
//! hidden children and never listed. Rows are compared on their RFC 3339
//! text, so the bounds are normalised to the store's own format first.

use std::collections::{BTreeMap, HashMap, HashSet};

use genai::chat::{ChatMessage, ChatRole, ContentPart};
use runtime::SessionId;

use super::Store;
use crate::dreaming_ledger::{
    Ledger, LedgerCount, LedgerQuestion, LedgerSession, LedgerSince, LedgerStop, LedgerToolError,
    LedgerUserMessage, MAX_ERROR_CHARS, MAX_SESSIONS, MAX_TEXT_BYTES, MAX_TOOL_ERRORS,
    MAX_USER_MESSAGES,
};

/// `sessions` columns the ledger needs, every row (the lineage walk needs
/// parents outside the window too).
type SessionRow = (
    String,
    Option<String>,
    Option<String>,
    bool,
    String,
    String,
    String,
    String,
    Option<String>,
);

/// Ancestor walks stop here (a cycle in persisted topology).
const LINEAGE_MAX: usize = 64;

/// The ids a `<knowledge>` note lists (`- `id` — …` lines, 10 §4.1).
pub fn hinted_ids(note: &str) -> impl Iterator<Item = &str> {
    note.lines().filter_map(|l| {
        let rest = l.strip_prefix("- `")?;
        let end = rest.find('`')?;
        Some(&rest[..end])
    })
}

/// RFC 3339 text in this store's own shape (`chrono::Utc::now().to_rfc3339()`),
/// so `>=`/`<` on columns compare like instants; unparsable input is kept.
fn bound(s: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc).to_rfc3339())
        .unwrap_or_else(|_| s.to_owned())
}

fn parse_id(s: &str) -> Option<SessionId> {
    s.parse().ok()
}

impl Store {
    /// The ledger of `[since, until)` minus `hints` (see the module doc),
    /// plus the distinct concept ids the window's `<knowledge>` notes showed.
    /// `redact` is the spill redactor every text passes through.
    pub async fn ledger_since(
        &self,
        m: &LedgerSince,
        redact: &(dyn Fn(&str) -> String + Sync),
    ) -> anyhow::Result<(Ledger, Vec<String>)> {
        let (since, until) = (bound(&m.since), bound(&m.until));
        let mut ledger = Ledger::empty(&m.since, &m.until);

        // --- sessions: lineage over every row, the window's rows listed.
        let rows: Vec<SessionRow> = sqlx::query_as(
            "SELECT id, parent, desk, unattended, kind, created_at, updated_at, title, finished_at \
             FROM sessions",
        )
        .fetch_all(&self.read)
        .await?;
        let parent: HashMap<&str, Option<&str>> = rows
            .iter()
            .map(|r| (r.0.as_str(), r.1.as_deref()))
            .collect();
        let desks: HashSet<&str> = rows
            .iter()
            .filter(|r| r.2.is_some())
            .map(|r| r.0.as_str())
            .collect();
        let isolated: HashSet<String> = sqlx::query_scalar::<_, String>(
            "SELECT id FROM sessions WHERE json_extract(knowledge_identity, '$[1]') = 1",
        )
        .fetch_all(&self.read)
        .await?
        .into_iter()
        .collect();
        let excluded: HashSet<String> = m.exclude.iter().map(ToString::to_string).collect();
        let root_of = |id: &str| -> (String, bool) {
            let mut cur = id;
            let mut dropped = excluded.contains(cur);
            for _ in 0..LINEAGE_MAX {
                match parent.get(cur).copied().flatten() {
                    Some(p) => {
                        cur = p;
                        dropped |= excluded.contains(cur);
                    }
                    None => break,
                }
            }
            (cur.to_owned(), dropped)
        };
        let mut listed: Vec<LedgerSession> = Vec::new();
        for (id, parent_id, desk, unattended, kind, created_at, updated_at, title, finished_at) in
            &rows
        {
            // 141 R1.2 Q7: watch-only harness subagents are not dreamt about
            // (their parent, the ACP child, still is). Lineage above keeps them.
            if kind == "advisor" || kind == "observed" {
                continue;
            }
            let in_window = created_at < &until
                && (created_at >= &since
                    || updated_at >= &since
                    || finished_at.as_deref().is_some_and(|f| f >= since.as_str()));
            if !in_window {
                continue;
            }
            let (root, dropped) = root_of(id);
            if dropped
                || isolated.contains(&root)
                || (m.owner_only && desks.contains(root.as_str()))
            {
                continue;
            }
            let Some(sid) = parse_id(id) else { continue };
            let kind = match (parent_id, desk, unattended) {
                (Some(_), _, _) => "child",
                (None, Some(_), _) => "desk",
                (None, None, true) => "routine",
                (None, None, false) => "root",
            };
            listed.push(LedgerSession {
                id: sid,
                title: redact(title),
                kind: kind.to_owned(),
                parent: parent_id.as_deref().and_then(parse_id),
                created_at: created_at.clone(),
                turns: 0,
                tool_calls: 0,
                errors: 0,
                model_cost_nanos: 0,
            });
        }
        listed.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.0.cmp(&b.id.0)));
        if listed.len() > MAX_SESSIONS {
            listed.truncate(MAX_SESSIONS);
            ledger.truncated = true;
        }
        let allowed: HashSet<String> = listed.iter().map(|s| s.id.to_string()).collect();
        let by_id: HashMap<String, usize> = listed
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.to_string(), i))
            .collect();

        // --- turns and stops.
        let steps: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT session_id, turn_id, at, payload FROM turn_steps \
             WHERE kind = 'turn_finished' AND at >= ?1 AND at < ?2 ORDER BY at",
        )
        .bind(&since)
        .bind(&until)
        .fetch_all(&self.read)
        .await?;
        for (session, turn_id, at, payload) in steps {
            let Some(i) = by_id.get(&session).copied() else {
                continue;
            };
            listed[i].turns += 1;
            let stop_reason = serde_json::from_str::<serde_json::Value>(&payload)
                .ok()
                .and_then(|v| v["stop_reason"].as_str().map(str::to_owned))
                .unwrap_or_default();
            if stop_reason != "end_turn" {
                ledger.stops.push(LedgerStop {
                    session: listed[i].id,
                    turn_id,
                    stop_reason,
                    at,
                });
            }
        }

        // --- tool calls: counts, errors, direct `load_skill`s.
        let mut skill_loads: BTreeMap<String, (u64, u64, HashSet<String>)> = BTreeMap::new();
        let mut load = |name: &str, args: &str, is_error: bool, session: &str| {
            if name != crate::skills::LOAD {
                return;
            }
            let skill = serde_json::from_str::<serde_json::Value>(args)
                .ok()
                .and_then(|v| v["name"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let e = skill_loads.entry(skill).or_default();
            e.0 += 1;
            e.1 += u64::from(is_error);
            e.2.insert(session.to_owned());
        };
        /// `(session_id, turn_id, name, args, attempt, is_error, at, output)`.
        type CallRow = (
            String,
            String,
            String,
            String,
            i64,
            bool,
            String,
            Option<String>,
        );
        let calls: Vec<CallRow> =
            sqlx::query_as(
                "SELECT session_id, turn_id, name, args, attempt, is_error, \
                        COALESCE(finished_at, started_at) AS at, output FROM tool_calls \
                 WHERE COALESCE(finished_at, started_at) >= ?1 AND COALESCE(finished_at, started_at) < ?2 \
                 ORDER BY at",
            )
            .bind(&since)
            .bind(&until)
            .fetch_all(&self.read)
            .await?;
        for (session, turn_id, name, args, attempt, is_error, at, output) in calls {
            let Some(i) = by_id.get(&session).copied() else {
                continue;
            };
            listed[i].tool_calls += 1;
            load(&name, &args, is_error, &session);
            if !is_error {
                continue;
            }
            listed[i].errors += 1;
            if ledger.tool_errors.len() >= MAX_TOOL_ERRORS {
                ledger.truncated = true;
                continue;
            }
            let first = output
                .as_deref()
                .and_then(|o| o.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or_default();
            let first: String = redact(first).chars().take(MAX_ERROR_CHARS).collect();
            ledger.tool_errors.push(LedgerToolError {
                session: listed[i].id,
                turn_id,
                name,
                attempt: u32::try_from(attempt).unwrap_or(1),
                at,
                output: first,
            });
        }

        // --- inner (python) calls by name; nested `load_skill`s.
        let inner: Vec<(String, String, String, bool)> = sqlx::query_as(
            "SELECT session_id, name, args, is_error FROM inner_calls WHERE at >= ?1 AND at < ?2",
        )
        .bind(&since)
        .bind(&until)
        .fetch_all(&self.read)
        .await?;
        let mut inner_counts: BTreeMap<String, (u64, u64, HashSet<String>)> = BTreeMap::new();
        for (session, name, args, is_error) in inner {
            if !allowed.contains(&session) {
                continue;
            }
            load(&name, &args, is_error, &session);
            let e = inner_counts.entry(name).or_default();
            e.0 += 1;
            e.1 += u64::from(is_error);
            e.2.insert(session);
        }
        let counts = |m: BTreeMap<String, (u64, u64, HashSet<String>)>| -> Vec<LedgerCount> {
            m.into_iter()
                .map(|(name, (calls, errors, sessions))| LedgerCount {
                    name,
                    calls,
                    errors,
                    sessions: sessions.len() as u64,
                })
                .collect()
        };
        ledger.inner_calls = counts(inner_counts);
        ledger.skill_loads = counts(skill_loads);

        // --- model cost per session: returned ?? estimated, any currency.
        let money: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT owner_id, returned_nanos, estimated_nanos FROM model_calls \
             WHERE created_at >= ?1 AND created_at < ?2",
        )
        .bind(&since)
        .bind(&until)
        .fetch_all(&self.read)
        .await?;
        for (owner, returned, estimated) in money {
            if let Some(i) = by_id.get(&owner).copied() {
                let nanos = returned.or(estimated).and_then(|n| u64::try_from(n).ok());
                listed[i].model_cost_nanos += nanos.unwrap_or(0);
            }
        }

        // --- questions.
        /// `(session_id, kind, state, answer, answered_by, asked_at)`.
        type QuestionRow = (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        );
        let questions: Vec<QuestionRow> = sqlx::query_as(
            "SELECT session_id, kind, state, answer, answered_by, asked_at FROM questions \
                 WHERE asked_at >= ?1 AND asked_at < ?2 ORDER BY asked_at",
        )
        .bind(&since)
        .bind(&until)
        .fetch_all(&self.read)
        .await?;
        for (session, kind, state, answer, answered_by, asked_at) in questions {
            let Some(i) = by_id.get(&session).copied() else {
                continue;
            };
            // A `Decision` is a JSON string (`"allow"`); an ask's answers stay JSON.
            let answer = answer.map(|a| {
                let plain = serde_json::from_str::<serde_json::Value>(&a)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or(a);
                redact(&plain).chars().take(MAX_ERROR_CHARS).collect()
            });
            ledger.questions.push(LedgerQuestion {
                session: listed[i].id,
                kind,
                state,
                answer,
                answered_by,
                asked_at,
            });
        }

        // --- user messages, `[app …]` presses, the ids `<knowledge>` notes showed.
        let messages: Vec<(String, i64, String, String)> = sqlx::query_as(
            "SELECT session_id, seq, at, body FROM messages \
             WHERE at >= ?1 AND at < ?2 ORDER BY at, seq",
        )
        .bind(&since)
        .bind(&until)
        .fetch_all(&self.read)
        .await?;
        let mut hinted: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (session, seq, at, body) in messages {
            let Some(i) = by_id.get(&session).copied() else {
                continue;
            };
            let Ok(msg) = serde_json::from_str::<ChatMessage>(&body) else {
                continue;
            };
            if msg.role != ChatRole::User {
                continue;
            }
            let text: String = msg
                .content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if let Some(note) = text.strip_prefix("<knowledge>") {
                for id in hinted_ids(note) {
                    if seen.insert(id.to_owned()) {
                        hinted.push(id.to_owned());
                    }
                }
                continue;
            }
            let row = |text: &str| LedgerUserMessage {
                session: listed[i].id,
                seq,
                at: at.clone(),
                text: platform::text::cut_bytes(&redact(text), MAX_TEXT_BYTES).to_owned(),
            };
            if text.starts_with("[app ") {
                ledger.app_presses.push(row(text));
            } else if !text.starts_with(['<', '[']) {
                if ledger.user_messages.len() >= MAX_USER_MESSAGES {
                    ledger.truncated = true;
                } else {
                    ledger.user_messages.push(row(text));
                }
            }
        }

        // `rewinds`: the corrected tail is deleted at the next write (29) and
        // nothing in this store keeps it; dreaming's `Signals` snapshots it at
        // event time. Left empty here.
        ledger.sessions = listed;
        Ok((ledger, hinted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::{CallKind, CallStatus, CostObservation, Money};
    use genai::chat::ToolCall;
    use runtime::TurnId;
    use tools::{Answerer, QuestionKind, QuestionRow, QuestionState, Risk, ToolOutput};

    fn redact(s: &str) -> String {
        s.replace("sk-SECRET", "[redacted]")
    }

    /// Two durable sessions (one excluded as the dreaming tree, its child
    /// too), a desk tree dropped by `owner_only`, an advisor never listed;
    /// every vector filled from fixture rows; the redactor applied; a private
    /// session lives in another store and is not here to begin with.
    #[tokio::test]
    async fn ledger_counts_filters_and_scrubs() {
        let scratch = crate::test_dir("ledger");
        let dir = scratch.path().to_path_buf();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let (user, dream, dream_child, desk, advisor) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        for (id, parent) in [
            (user, None),
            (dream, None),
            (dream_child, Some(dream)),
            (desk, None),
        ] {
            store
                .create_session(id, "m", parent, None, None)
                .await
                .unwrap();
        }
        store
            .create_session_kind(
                advisor,
                "m",
                Some(user),
                None,
                None,
                crate::SessionKind::Advisor,
            )
            .await
            .unwrap();
        store.set_desk(desk, Some("sre")).await.unwrap();
        store.set_unattended(dream).await.unwrap();
        store.set_title(user, "my sk-SECRET title").await.unwrap();

        // The user's session: plain text, a note, a press, a bracketed input, a `<knowledge>` note.
        for text in [
            "please fix the build with key sk-SECRET now",
            "<time at=\"x\"/>",
            "[app routine_draft accept]",
            "[image: shot.png] look",
            "<knowledge>\nConcepts that may be relevant:\n- `facts/a` — A [fact]: d\n- `procedures/b` — B [procedure]\n</knowledge>",
        ] {
            store.append(user, &ChatMessage::user(text)).await.unwrap();
        }
        store
            .append(user, &ChatMessage::assistant("done"))
            .await
            .unwrap();
        store
            .append(dream, &ChatMessage::user("dreaming's own words"))
            .await
            .unwrap();
        store
            .append(desk, &ChatMessage::user("desk's own words"))
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let turn = TurnId::new();
        // Journal: two finished turns (one cancelled), a failed `shell`, a
        // direct `load_skill`, two inner calls (one a nested `load_skill`).
        store.finish_turn(user, turn, "end_turn").await.unwrap();
        store
            .finish_turn(user, TurnId::new(), "cancelled")
            .await
            .unwrap();
        let call = |id: &str, name: &str, args: serde_json::Value| ToolCall {
            call_id: id.into(),
            fn_name: name.into(),
            fn_arguments: args,
            thought_signatures: None,
        };
        let planned = [
            (
                call("c1", "shell", serde_json::json!({"command": "ls"})),
                Risk::Mutating,
            ),
            (
                call(
                    "c2",
                    crate::skills::LOAD,
                    serde_json::json!({"name": "caveman"}),
                ),
                Risk::ReadOnly,
            ),
        ];
        store
            .append_tool_use(
                user,
                turn,
                &ChatMessage::from(planned.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>()),
                None,
                0,
                &planned,
            )
            .await
            .unwrap();
        store
            .call_finished(user, "c1", &ToolOutput::err("ERROR: boom sk-SECRET\nmore"))
            .await
            .unwrap();
        store
            .call_finished(user, "c2", &ToolOutput::ok("body"))
            .await
            .unwrap();
        for (id, name, args) in [
            (
                "c3.1",
                "knowledge_get",
                serde_json::json!({"id": "facts/a"}),
            ),
            (
                "c3.2",
                crate::skills::LOAD,
                serde_json::json!({"name": "ponytail"}),
            ),
        ] {
            store
                .inner_call_started(user, id, name, &args)
                .await
                .unwrap();
            store
                .inner_call_finished(user, id, "out", false, &[])
                .await
                .unwrap();
        }
        // Money: one settled call at $0.25.
        let call = store
            .reserve_call(
                user,
                "r",
                0,
                Some(turn),
                CallKind::Round,
                Some("p"),
                Some("m"),
            )
            .await
            .unwrap();
        store.mark_dispatched(&call).await.unwrap();
        store
            .settle_call(
                &call,
                CallStatus::Settled,
                CostObservation {
                    returned: Some(Money {
                        currency: "USD".into(),
                        nanos: 250_000_000,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // A denied approval.
        store
            .question_put(&QuestionRow {
                id: "q1".into(),
                session: user,
                tool_call_id: None,
                kind: QuestionKind::Approval,
                payload: serde_json::json!({}),
                state: QuestionState::Answered,
                asked_at: now,
                deadline: now,
                answered_at: Some(now),
                answer: Some(serde_json::json!("deny")),
                answered_by: Some(Answerer::Human),
            })
            .await
            .unwrap();

        let window = |exclude: Vec<SessionId>, owner_only: bool| LedgerSince {
            since: (now - chrono::Duration::hours(1)).to_rfc3339(),
            until: (now + chrono::Duration::hours(1)).to_rfc3339(),
            exclude,
            owner_only,
        };
        let (l, hinted) = store
            .ledger_since(&window(vec![dream], true), &redact)
            .await
            .unwrap();
        let ids: Vec<SessionId> = l.sessions.iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            [user],
            "dream tree excluded, desk dropped, advisor hidden"
        );
        let s = &l.sessions[0];
        assert_eq!(s.kind, "root");
        assert_eq!(s.title, "my [redacted] title");
        assert_eq!((s.turns, s.tool_calls, s.errors), (2, 2, 1));
        assert_eq!(s.model_cost_nanos, 250_000_000);
        assert_eq!(l.user_messages.len(), 1);
        assert_eq!(
            l.user_messages[0].text,
            "please fix the build with key [redacted] now"
        );
        assert_eq!(l.app_presses.len(), 1);
        assert_eq!(l.app_presses[0].text, "[app routine_draft accept]");
        assert_eq!(hinted, ["facts/a", "procedures/b"]);
        assert_eq!(l.stops.len(), 1);
        assert_eq!(l.stops[0].stop_reason, "cancelled");
        assert_eq!(l.tool_errors.len(), 1);
        assert_eq!(l.tool_errors[0].output, "ERROR: boom [redacted]");
        assert_eq!(l.tool_errors[0].name, "shell");
        assert_eq!(
            l.inner_calls,
            [
                LedgerCount {
                    name: "knowledge_get".into(),
                    calls: 1,
                    errors: 0,
                    sessions: 1
                },
                LedgerCount {
                    name: crate::skills::LOAD.into(),
                    calls: 1,
                    errors: 0,
                    sessions: 1
                },
            ]
        );
        assert_eq!(
            l.skill_loads,
            [
                LedgerCount {
                    name: "caveman".into(),
                    calls: 1,
                    errors: 0,
                    sessions: 1
                },
                LedgerCount {
                    name: "ponytail".into(),
                    calls: 1,
                    errors: 0,
                    sessions: 1
                },
            ]
        );
        assert_eq!(l.questions.len(), 1);
        assert_eq!(l.questions[0].answer.as_deref(), Some("deny"));
        assert_eq!(l.questions[0].answered_by.as_deref(), Some("human"));
        assert!(!l.truncated);

        // Without `owner_only` and `exclude`: the desk and the dreaming tree appear too.
        let (l, _) = store
            .ledger_since(&window(vec![], false), &redact)
            .await
            .unwrap();
        let kinds: HashMap<SessionId, &str> =
            l.sessions.iter().map(|s| (s.id, s.kind.as_str())).collect();
        assert_eq!(kinds[&dream], "routine");
        assert_eq!(kinds[&dream_child], "child");
        assert_eq!(kinds[&desk], "desk");
        assert_eq!(l.user_messages.len(), 3);

        // An empty window.
        let (l, hinted) = store
            .ledger_since(
                &LedgerSince {
                    since: "2000-01-01T00:00:00Z".into(),
                    until: "2000-01-02T00:00:00Z".into(),
                    exclude: vec![],
                    owner_only: false,
                },
                &redact,
            )
            .await
            .unwrap();
        assert!(l.sessions.is_empty() && hinted.is_empty());
    }

    #[test]
    fn hinted_ids_lists_the_ids() {
        let ids: Vec<&str> =
            hinted_ids("lead\n- `a/b` — T [fact]: d\n- not an id\n- `c/d` — U\n").collect();
        assert_eq!(ids, ["a/b", "c/d"]);
    }
}
