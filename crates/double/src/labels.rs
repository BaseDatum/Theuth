//! Labels (126 §6): free supervision from what the owner did. Direct labels
//! come from the bus resolution events (`ApprovalResolved.decision`; an ask's
//! answer from the `ask_user` tool's rendered output) AND from the 109
//! `questions` row when the table exists ([`Journal`] — the running engine
//! has none until it is rebuilt, so the event path works alone, §6.3).
//! Hindsight negatives (rewind 0.5, interrupt 0.25, correction 0.5 × P) attach
//! to the decisions of the turn they land on ([`Turns`]); an override of a
//! double suggestion is weight 3. No bootstrapper (Revision 1 ruling 8):
//! nothing here reads `messages`.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use runtime::SessionId;
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use tools::{Answer, Answerer, Question, QuestionState};

use crate::ladder;
use crate::store::{LabelRow, Store};
use crate::types::LabelSource;

/// §6 weights.
pub const W_DIRECT: f32 = 1.0;
/// The owner took over or reversed an L2/L3 answer.
pub const W_OVERRIDE: f32 = 3.0;
/// A rewind of the turn.
pub const W_REWIND: f32 = 0.5;
/// The turn was cancelled within [`INTERRUPT_WINDOW_S`] of the decision.
pub const W_INTERRUPT: f32 = 0.25;
/// Nobody answered / the row was superseded.
pub const W_TIMEOUT: f32 = 0.25;
/// A correction counts only when the `double.correction` family says P(true) ≥ this.
pub const CORRECTION_MIN_P: f32 = 0.8;
/// Interrupt window, seconds.
pub const INTERRUPT_WINDOW_S: i64 = 30;
/// A hindsight signal reaches the previous turn's decisions for this many user turns.
pub const CORRECTION_TURNS: u8 = 2;

/// `0.5 × P(true)` when P(true) ≥ [`CORRECTION_MIN_P`].
pub fn correction_weight(p_true: f32) -> Option<f32> {
    (p_true >= CORRECTION_MIN_P).then_some(0.5 * p_true)
}

/// Write one label and re-score the example's prediction rows (§7.4).
pub async fn label(
    store: &Store,
    example_id: &str,
    source: LabelSource,
    choice: &str,
    weight: f32,
    at: DateTime<Utc>,
) -> anyhow::Result<()> {
    store
        .put_label(&LabelRow {
            example_id: example_id.to_owned(),
            source,
            choice: choice.to_owned(),
            weight,
            at,
        })
        .await?;
    rescore(store, example_id).await
}

/// Recompute `outcome` for every prediction row of `example_id` from its labels.
pub async fn rescore(store: &Store, example_id: &str) -> anyhow::Result<()> {
    let labels: Vec<(LabelSource, String, f32)> = store
        .labels(example_id)
        .await?
        .into_iter()
        .map(|l| (l.source, l.choice, l.weight))
        .collect();
    for mut p in store.predictions(example_id).await? {
        let outcome = ladder::outcome(&p.choice, &labels);
        if outcome != p.outcome {
            p.outcome = outcome;
            store.put_prediction(&p).await?;
        }
    }
    Ok(())
}

/// One settled `questions` row, as far as the labeller reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct JournalQuestion {
    /// Lifecycle state.
    pub state: QuestionState,
    /// `Decision` JSON for an approval, `Vec<Answer>` JSON for an ask.
    pub answer: Option<serde_json::Value>,
    /// When raised.
    pub asked_at: DateTime<Utc>,
    /// When it left `pending`.
    pub answered_at: Option<DateTime<Utc>>,
    /// Who answered.
    pub answered_by: Option<Answerer>,
}

/// Read-only view of `<data_dir>/sessions.sqlite`'s `questions` table (109).
/// `open` is `None` when the file is missing; [`Journal::question`] is `None`
/// when the table is not there yet (the pre-0033 engine).
#[derive(Clone)]
pub struct Journal {
    pool: SqlitePool,
}

impl Journal {
    /// Open the sessions database read-only.
    pub async fn open(data_dir: &Path) -> Option<Self> {
        let path = data_dir.join("sessions.sqlite");
        if !path.exists() {
            return None;
        }
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .busy_timeout(std::time::Duration::from_secs(2));
        match SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
        {
            Ok(pool) => Some(Self { pool }),
            Err(e) => {
                tracing::debug!("double: sessions.sqlite not readable: {e}");
                None
            }
        }
    }

    /// Whether the `questions` table exists.
    pub async fn has_questions(&self) -> bool {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'questions'",
        )
        .fetch_one(&self.pool)
        .await
        .is_ok_and(|n| n > 0)
    }

    /// The row for a card id; `None` when absent or the table does not exist.
    pub async fn question(&self, id: &str) -> Option<JournalQuestion> {
        let row = sqlx::query(
            "SELECT state, answer, asked_at, answered_at, answered_by FROM questions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        let state: String = row.try_get("state").ok()?;
        let answer: Option<String> = row.try_get("answer").ok()?;
        let asked_at: String = row.try_get("asked_at").ok()?;
        let answered_at: Option<String> = row.try_get("answered_at").ok()?;
        let by: Option<String> = row.try_get("answered_by").ok()?;
        Some(JournalQuestion {
            state: QuestionState::parse(&state)?,
            answer: answer.and_then(|a| serde_json::from_str(&a).ok()),
            asked_at: DateTime::parse_from_rfc3339(&asked_at)
                .ok()?
                .with_timezone(&Utc),
            answered_at: answered_at
                .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
                .map(|t| t.with_timezone(&Utc)),
            answered_by: by.and_then(|b| Answerer::parse(&b)),
        })
    }
}

/// Every option label of an ask set in card order (the keyer hashes them;
/// empty = freeform).
pub fn ask_options(questions: &[Question]) -> Vec<String> {
    questions
        .iter()
        .flat_map(|q| q.options.iter().map(|c| c.label.clone()))
        .collect()
}

/// The one question double can answer: a set of exactly one question with
/// options. Multi-question sets and freeform asks are observed only.
pub fn ask_single(questions: &[Question]) -> Option<&Question> {
    match questions {
        [q] if !q.options.is_empty() => Some(q),
        _ => None,
    }
}

/// The chosen label and index of a single-question set from the settled
/// `questions.answer` (`[{"kind":"choice","index":n}]`); `None` for a text
/// answer, a skip or a multi-question set.
pub fn ask_choice(questions: &[Question], answer: &serde_json::Value) -> Option<(String, u32)> {
    let q = ask_single(questions)?;
    let answers: Vec<Answer> = serde_json::from_value(answer.clone()).ok()?;
    match answers.first()? {
        Answer::Choice { index } => q
            .options
            .get(*index)
            .map(|c| (c.label.clone(), *index as u32)),
        _ => None,
    }
}

/// The chosen label from the `ask_user` tool's rendered output (one line per
/// question, `<id>: <label>`), for a single-question set.
pub fn ask_choice_from_output(questions: &[Question], output: &str) -> Option<(String, u32)> {
    let q = ask_single(questions)?;
    let line = output.lines().next()?;
    let value = line.strip_prefix(&q.id)?.strip_prefix(": ")?;
    q.options
        .iter()
        .position(|c| c.label == value)
        .map(|i| (q.options[i].label.clone(), i as u32))
}

/// The `[Answer]` the double sends for `index` on a single-question set.
pub fn ask_answers(index: u32) -> Vec<Answer> {
    vec![Answer::Choice {
        index: index as usize,
    }]
}

/// Per-session memory of which decisions the current and the previous user
/// turn raised, for the §6.1 attachment rules. In-memory only.
// ponytail: lost on restart — a rewind right after a restart finds no turn;
// persist in `examples` (a `turn` column) if that ever matters.
#[derive(Debug, Default)]
pub struct Turns {
    sessions: HashMap<SessionId, SessionTurns>,
}

#[derive(Debug, Default)]
struct SessionTurns {
    /// `(question_id, asked_at)` of the turn in flight.
    current: Vec<(String, DateTime<Utc>)>,
    /// The last turn that raised decisions, and how many user turns ago it ended.
    previous: Vec<(String, DateTime<Utc>)>,
    age: u8,
}

impl Turns {
    /// A user message opened a new turn: the turn in flight becomes "previous".
    pub fn started(&mut self, session: SessionId) {
        let s = self.sessions.entry(session).or_default();
        if s.current.is_empty() {
            s.age = s.age.saturating_add(1);
            if s.age >= CORRECTION_TURNS {
                s.previous.clear();
            }
        } else {
            s.previous = std::mem::take(&mut s.current);
            s.age = 0;
        }
    }

    /// A decision card was raised in the session's turn in flight.
    pub fn card(&mut self, session: SessionId, question_id: &str, at: DateTime<Utc>) {
        self.sessions
            .entry(session)
            .or_default()
            .current
            .push((question_id.to_owned(), at));
    }

    /// A card was settled by a timeout: it never was the owner's decision.
    pub fn drop_card(&mut self, session: SessionId, question_id: &str) {
        if let Some(s) = self.sessions.get_mut(&session) {
            s.current.retain(|(q, _)| q != question_id);
            s.previous.retain(|(q, _)| q != question_id);
        }
    }

    /// `ContextChanged { reason: "rewind" }`: the decisions of the rewound turn
    /// (the one in flight, else the previous one), consumed.
    pub fn rewind(&mut self, session: SessionId) -> Vec<String> {
        let Some(s) = self.sessions.get_mut(&session) else {
            return Vec::new();
        };
        let turn = if s.current.is_empty() {
            std::mem::take(&mut s.previous)
        } else {
            std::mem::take(&mut s.current)
        };
        turn.into_iter().map(|(q, _)| q).collect()
    }

    /// `TurnFinished { stop_reason: "cancelled" }`: the turn's decisions asked
    /// within [`INTERRUPT_WINDOW_S`] of `now`; the turn becomes "previous".
    pub fn cancelled(&mut self, session: SessionId, now: DateTime<Utc>) -> Vec<String> {
        let Some(s) = self.sessions.get_mut(&session) else {
            return Vec::new();
        };
        let hit = s
            .current
            .iter()
            .filter(|(_, at)| (now - *at).num_seconds() <= INTERRUPT_WINDOW_S)
            .map(|(q, _)| q.clone())
            .collect();
        if !s.current.is_empty() {
            s.previous = std::mem::take(&mut s.current);
            s.age = 0;
        }
        hit
    }

    /// The previous turn's decisions — what a correction in the new user
    /// message is about (call after [`Turns::started`]).
    pub fn previous(&self, session: SessionId) -> Vec<String> {
        self.sessions
            .get(&session)
            .map(|s| s.previous.iter().map(|(q, _)| q.clone()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::Choice;

    fn q(id: &str, labels: &[&str]) -> Question {
        Question {
            id: id.into(),
            question: "?".into(),
            options: labels
                .iter()
                .map(|l| Choice {
                    label: (*l).into(),
                    description: None,
                })
                .collect(),
            recommended: None,
        }
    }

    #[test]
    fn ask_parsing() {
        let set = vec![q("mode", &["Add to backlog", "Start now", "Discard"])];
        assert_eq!(ask_options(&set).len(), 3);
        assert!(ask_single(&set).is_some());
        let a = serde_json::json!([{ "kind": "choice", "index": 1 }]);
        assert_eq!(ask_choice(&set, &a), Some(("Start now".into(), 1)));
        let t = serde_json::json!([{ "kind": "text", "text": "later" }]);
        assert_eq!(ask_choice(&set, &t), None);
        assert_eq!(
            ask_choice_from_output(&set, "mode: Discard\n{\"mode\":\"Discard\"}"),
            Some(("Discard".into(), 2))
        );
        assert_eq!(ask_choice_from_output(&set, "mode: (skipped)"), None);
        // Two questions: observed only.
        let two = vec![q("a", &["x", "y"]), q("b", &["z", "w"])];
        assert!(ask_single(&two).is_none());
        assert_eq!(ask_choice(&two, &a), None);
        // Freeform.
        assert!(ask_options(&[q("f", &[])]).is_empty());
        assert_eq!(ask_answers(2), vec![Answer::Choice { index: 2 }]);
    }

    #[test]
    fn correction_threshold() {
        assert_eq!(correction_weight(0.79), None);
        assert_eq!(correction_weight(0.9), Some(0.45));
    }

    #[test]
    fn turns_attach_hindsight() {
        let s = SessionId::new();
        let now = Utc::now();
        let mut t = Turns::default();
        t.started(s);
        t.card(s, "q1", now - chrono::Duration::seconds(60));
        t.card(s, "q2", now - chrono::Duration::seconds(5));
        // Interrupt: only the decision within 30 s.
        assert_eq!(t.cancelled(s, now), vec!["q2".to_owned()]);
        // The cancelled turn is now "previous": a correction in the next message hits it.
        t.started(s);
        assert_eq!(t.previous(s), vec!["q1".to_owned(), "q2".to_owned()]);
        // Two more user turns without decisions and it is out of reach.
        t.started(s);
        t.started(s);
        assert!(t.previous(s).is_empty());
        // Rewind takes the turn in flight, else the previous one.
        t.card(s, "q3", now);
        assert_eq!(t.rewind(s), vec!["q3".to_owned()]);
        assert!(t.rewind(s).is_empty());
        t.card(s, "q4", now);
        t.started(s);
        assert_eq!(t.rewind(s), vec!["q4".to_owned()]);
        // A timed-out card is dropped.
        t.card(s, "q5", now);
        t.drop_card(s, "q5");
        assert!(t.rewind(s).is_empty());
        // Unknown session: nothing.
        assert!(t.rewind(SessionId::new()).is_empty());
    }

    /// The journal path: a `questions` table in a sessions.sqlite, read read-only.
    #[tokio::test]
    async fn journal_reads_questions() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Journal::open(dir.path()).await.is_none());
        let path = dir.path().join("sessions.sqlite");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path).create_if_missing(true))
            .await
            .unwrap();
        let j = Journal::open(dir.path()).await.unwrap();
        assert!(!j.has_questions().await);
        assert_eq!(j.question("x").await, None);
        sqlx::query(
            "CREATE TABLE questions (id TEXT PRIMARY KEY, session_id TEXT, tool_call_id TEXT, kind TEXT, payload TEXT, state TEXT, asked_at TEXT, deadline TEXT, answered_at TEXT, answer TEXT, answered_by TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO questions VALUES ('q', 's', NULL, 'approval', '{}', 'answered', '2026-10-07T10:00:00+00:00', '2026-10-07T10:05:00+00:00', '2026-10-07T10:00:02+00:00', '\"allow\"', 'human')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(j.has_questions().await);
        let row = j.question("q").await.unwrap();
        assert_eq!(row.state, QuestionState::Answered);
        assert_eq!(row.answer, Some(serde_json::json!("allow")));
        assert_eq!(row.answered_by, Some(Answerer::Human));
        assert_eq!((row.answered_at.unwrap() - row.asked_at).num_seconds(), 2);
    }
}
