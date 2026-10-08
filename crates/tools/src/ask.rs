//! `ask_user`: the model asks the user one or more multiple-choice questions
//! and blocks until a UI answers. Same shape as [`crate::approval`]: an
//! [`AskUser`] actor (registered as [`crate::ASK_ACTOR`]) publishes
//! [`AskRequested`], waits for an [`AnswerAsk`], publishes [`AskResolved`].
//!
//! Skipping one question keeps the rest of the answers; dismissing the whole
//! set is `answers: None`, which the RPC layer turns into a turn cancel.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::journal::{
    Answerer, Journal, QuestionKind, QuestionRow, QuestionState, WaitKind, WaitRow,
};
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Correlates a request with its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct AskId(pub uuid::Uuid);

/// One selectable answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Choice {
    /// Short label (a few words).
    pub label: String,
    /// Optional tradeoff or explanation shown under the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Option<String>,
}

/// One question. The user picks an option, types a freeform answer, or skips.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Question {
    /// Short stable key; the answer is reported under it.
    pub id: String,
    /// The question text.
    pub question: String,
    /// 2–5 distinct options; may be empty for a freeform-only question.
    #[serde(default)]
    pub options: Vec<Choice>,
    /// Index into `options` you recommend; shown to the user as such.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub recommended: Option<usize>,
}

/// The user's answer to one question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    /// Picked an option.
    Choice {
        /// Index into `Question::options`.
        index: usize,
    },
    /// Typed their own.
    Text {
        /// What they typed.
        text: String,
    },
    /// Left it unanswered.
    Skipped,
}

/// Event: a tool call is waiting on the user's answers. Every UI subscribes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct AskRequested {
    /// Use in [`AnswerAsk`].
    pub id: AskId,
    /// Owning session.
    pub session: SessionId,
    /// The questions, in order.
    pub questions: Vec<Question>,
}

/// Event: a request was answered, dismissed or timed out. UIs drop their
/// prompt on this.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct AskResolved {
    /// The request.
    pub id: AskId,
    /// The user closed the whole set without answering.
    pub dismissed: bool,
    /// Nobody answered in time.
    pub timed_out: bool,
}

/// `ask` → `Option<SessionId>`: the owning session if the id was pending and
/// is now resolved. `answers: None` means dismissed. A human answer; the
/// double answers with [`AnswerAskBy`] (126 §4.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerAsk {
    /// The request.
    pub id: AskId,
    /// One per question, in order; `None` dismisses the set.
    pub answers: Option<Vec<Answer>>,
}

/// [`AnswerAsk`] naming who answers (126 §4.3): the double's answer settles
/// the row with `answered_by = double`. The double never dismisses
/// (`answers: None` cancels the turn), so `answers` is required here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerAskBy {
    /// The request.
    pub id: AskId,
    /// One per question, in order.
    pub answers: Vec<Answer>,
    /// Who answers.
    pub by: Answerer,
}

/// Tool → actor: publish [`AskRequested`] and wait. `ask` → the answers
/// (`None` = dismissed) and whether it timed out. Public for the tasks
/// plugin's `escalate`, which asks the user the same way (67 §11).
pub struct Request {
    /// The session the card belongs to.
    pub session: SessionId,
    /// What to ask.
    pub questions: Vec<Question>,
    /// The asking tool call (109 §6.1: `questions.tool_call_id`); `None`
    /// when no tool call asks (`escalate`).
    pub call_id: Option<String>,
}

/// Resume → actor (109 §6.2): re-arm a `pending` `questions` row of kind
/// `ask` under its own id with `deadline - now` left, and publish the same
/// [`AskRequested`] again so a reconnecting UI renders the card it would
/// have rendered. `ask` → the [`Outcome`], as for [`Request`]; a passed
/// deadline is `timed_out` at once. Idempotent on the id.
pub struct Republish(pub QuestionRow);

/// Internal: drop a pending entry whose waiter gave up.
struct Expire(AskId);

/// A waiting tool call: its session, questions, journal row and where its
/// answers go.
struct Waiter {
    session: SessionId,
    questions: Vec<Question>,
    row: QuestionRow,
    tx: oneshot::Sender<Option<Vec<Answer>>>,
}

/// The ask actor.
#[derive(kameo::Actor)]
pub struct AskUser {
    rt: Runtime,
    timeout: Duration,
    pending: HashMap<AskId, Waiter>,
    /// 109 §6.1: the journal seam the `questions` rows go through.
    pub(crate) journal: Arc<dyn Journal>,
}

impl AskUser {
    /// Build with the runtime (for publishing) and the human timeout.
    pub fn new(rt: Runtime, timeout: Duration) -> Self {
        Self {
            rt,
            timeout,
            pending: HashMap::new(),
            journal: Arc::new(crate::NoopJournal),
        }
    }

    /// Journal a pending row's end (`answered` with the answers — `null`
    /// when dismissed — or `timed_out`) and clear the call's `waits` row.
    /// `by` names who answered (126 §4.3); `None` on a timeout.
    async fn settle(
        &self,
        mut row: QuestionRow,
        state: QuestionState,
        answer: Option<Value>,
        by: Option<Answerer>,
    ) {
        row.state = state;
        row.answered_at = Some(Utc::now());
        row.answer = answer;
        row.answered_by = by;
        if let Some(call) = &row.tool_call_id {
            self.journal.wait(None, row.session, call).await;
        }
        self.journal.question(row).await;
    }

    /// Deliver `answers` to the parked call and settle the row; `None` when
    /// `id` was not pending.
    async fn answer(
        &mut self,
        id: AskId,
        answers: Option<Vec<Answer>>,
        by: Answerer,
    ) -> Option<SessionId> {
        let w = self.pending.remove(&id)?;
        let answer = Some(serde_json::json!(answers));
        let delivered = w.tx.send(answers).ok().map(|()| w.session);
        self.settle(w.row, QuestionState::Answered, answer, Some(by))
            .await;
        delivered
    }

    /// Insert the waiter, journal the row (and the call's `waits(question)`
    /// row), publish the card and wait `timeout` for its answer off the
    /// mailbox. Shared by [`Request`] (fresh row) and [`Republish`] (the
    /// stored one).
    async fn arm(
        &mut self,
        ctx: &mut Context<Self, DelegatedReply<Outcome>>,
        row: QuestionRow,
        questions: Vec<Question>,
        timeout: Duration,
    ) -> DelegatedReply<Outcome> {
        let (reply, tx) = ctx.reply_sender();
        let id = AskId(uuid::Uuid::parse_str(&row.id).unwrap_or_else(|_| uuid::Uuid::now_v7()));
        let session = row.session;
        self.journal.question(row.clone()).await;
        if let Some(call) = &row.tool_call_id {
            self.journal
                .wait(
                    Some(WaitRow {
                        session,
                        tool_call_id: call.clone(),
                        kind: WaitKind::Question,
                        target: row.id.clone(),
                        deadline: Some(row.deadline),
                        payload: serde_json::json!({}),
                    }),
                    session,
                    call,
                )
                .await;
        }
        let (atx, arx) = oneshot::channel();
        self.pending.insert(
            id,
            Waiter {
                session,
                questions: questions.clone(),
                row,
                tx: atx,
            },
        );
        let rt = self.rt.clone();
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(AskRequested {
                id,
                session,
                questions,
            })
            .await;
            let (answers, timed_out) = match tokio::time::timeout(timeout, arx).await {
                Ok(Ok(a)) => (a, false),
                // Sender dropped: actor died; treat as dismissed.
                Ok(Err(_)) => (None, false),
                Err(_) => {
                    let _ = me.ask(Expire(id)).await;
                    (None, true)
                }
            };
            rt.publish(AskResolved {
                id,
                dismissed: answers.is_none() && !timed_out,
                timed_out,
            })
            .await;
            if let Some(tx) = tx {
                tx.send(Outcome { answers, timed_out });
            }
        });
        reply
    }
}

impl Message<crate::InstallJournal> for AskUser {
    type Reply = ();

    async fn handle(&mut self, m: crate::InstallJournal, _: &mut Context<Self, Self::Reply>) {
        self.journal = m.0;
    }
}

/// What the tool gets back from the actor.
#[derive(kameo::Reply)]
pub struct Outcome {
    /// The answers, one per question; `None` = dismissed.
    pub answers: Option<Vec<Answer>>,
    /// No answer within the human timeout.
    pub timed_out: bool,
}

impl Message<Request> for AskUser {
    type Reply = DelegatedReply<Outcome>;

    async fn handle(&mut self, m: Request, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let id = AskId(uuid::Uuid::now_v7());
        let asked_at = Utc::now();
        let row = QuestionRow {
            id: id.0.to_string(),
            session: m.session,
            tool_call_id: m.call_id,
            kind: QuestionKind::Ask,
            payload: serde_json::json!(AskRequested {
                id,
                session: m.session,
                questions: m.questions.clone(),
            }),
            state: QuestionState::Pending,
            asked_at,
            deadline: asked_at + self.timeout,
            answered_at: None,
            answer: None,
            answered_by: None,
        };
        self.arm(ctx, row, m.questions, self.timeout).await
    }
}

impl Message<Republish> for AskUser {
    type Reply = DelegatedReply<Outcome>;

    async fn handle(
        &mut self,
        Republish(row): Republish,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let questions: Vec<Question> = row
            .payload
            .get("questions")
            .cloned()
            .and_then(|q| serde_json::from_value(q).ok())
            .unwrap_or_default();
        let left = (row.deadline - Utc::now()).to_std().unwrap_or_default();
        if left.is_zero() {
            // 109 §6.3: downtime counted against the wait; the tool's own timeout.
            let (reply, tx) = ctx.reply_sender();
            self.settle(row, QuestionState::TimedOut, None, None).await;
            if let Some(tx) = tx {
                tx.send(Outcome {
                    answers: None,
                    timed_out: true,
                });
            }
            return reply;
        }
        if let Ok(id) = uuid::Uuid::parse_str(&row.id)
            && self.pending.contains_key(&AskId(id))
        {
            // Already re-armed (§6.4: idempotent on the id): the live
            // waiter answers the earlier ask; this one is dismissed.
            let (reply, tx) = ctx.reply_sender();
            if let Some(tx) = tx {
                tx.send(Outcome {
                    answers: None,
                    timed_out: false,
                });
            }
            return reply;
        }
        self.arm(ctx, row, questions, left).await
    }
}

impl Message<AnswerAsk> for AskUser {
    type Reply = Option<SessionId>;

    async fn handle(&mut self, m: AnswerAsk, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.answer(m.id, m.answers, Answerer::Human).await
    }
}

impl Message<AnswerAskBy> for AskUser {
    type Reply = Option<SessionId>;

    async fn handle(&mut self, m: AnswerAskBy, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.answer(m.id, Some(m.answers), m.by).await
    }
}

impl Message<Expire> for AskUser {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        if let Some(w) = self.pending.remove(&m.0) {
            self.settle(w.row, QuestionState::TimedOut, None, None)
                .await;
        }
    }
}

/// `ask` → the still-pending asks. Lets a UI that subscribes (or reconnects
/// over the relay) *after* the [`AskRequested`] broadcast still be shown the
/// card it missed, instead of a card that never arrives (31 §7.3).
pub struct PendingAsks;

impl Message<PendingAsks> for AskUser {
    type Reply = Vec<AskRequested>;

    async fn handle(&mut self, _: PendingAsks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.pending
            .iter()
            .map(|(id, w)| AskRequested {
                id: *id,
                session: w.session,
                questions: w.questions.clone(),
            })
            .collect()
    }
}

/// Arguments of `ask_user`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AskArgs {
    /// One or more questions, shown together.
    pub questions: Vec<Question>,
}

/// The `ask_user` tool: hands the questions to the actor and formats the answers.
pub struct AskUserTool(pub ActorRef<AskUser>);

#[async_trait::async_trait]
impl Tool for AskUserTool {
    const NAME: &'static str = "ask_user";
    const RISK: Risk = Risk::ReadOnly;
    type Args = AskArgs;
    const RETURNS: &'static str = "dict[str, str | None]";
    const DIRECT: bool = true;

    fn description(&self) -> String {
        "Ask the user one or more multiple-choice questions and wait for the answers. Use only when \
         options have materially different tradeoffs the user must weigh; resolve everything else \
         from context. Each question: `id`, `question`, 2-5 short `options` ({label, description?}), \
         optional `recommended` index. The user may pick an option, type a freeform answer, or skip a \
         question; dismissing the whole set ends your turn. Returns one line per question and a \
         `{id: answer | null}` map."
            .into()
    }

    async fn call(&self, args: Self::Args, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        // 109 §6.2: a resumed call re-arms its stored question instead of
        // asking a second time; an answer that landed before the kill is
        // rendered without asking at all.
        if let Some(row) = crate::registry::take_republish(cx.session, &cx.call_id) {
            let questions: Vec<Question> = row
                .payload
                .get("questions")
                .cloned()
                .and_then(|q| serde_json::from_value(q).ok())
                .unwrap_or_default();
            return Ok(match row.state {
                QuestionState::Pending => {
                    let out = self.0.ask(Republish(row)).await?;
                    outcome(&questions, out)
                }
                QuestionState::Answered => {
                    let answers: Option<Vec<Answer>> =
                        row.answer.and_then(|a| serde_json::from_value(a).ok());
                    outcome(
                        &questions,
                        Outcome {
                            answers,
                            timed_out: false,
                        },
                    )
                }
                QuestionState::TimedOut => {
                    ToolOutput::err("ask_user: no answer from the user in time")
                }
                QuestionState::Superseded => {
                    ToolOutput::err("ask_user: the question was superseded")
                }
            });
        }
        if args.questions.is_empty() {
            return Ok(ToolOutput::err("ask_user: no questions"));
        }
        for q in &args.questions {
            if q.recommended.is_some_and(|r| r >= q.options.len()) {
                return Ok(ToolOutput::err(format!(
                    "ask_user: question {:?}: recommended {} is out of range",
                    q.id,
                    q.recommended.unwrap_or_default()
                )));
            }
        }
        let questions = args.questions;
        let out = self
            .0
            .ask(Request {
                session: cx.session,
                questions: questions.clone(),
                call_id: Some(cx.call_id.clone()),
            })
            .await?;
        Ok(outcome(&questions, out))
    }
}

/// The tool's result for an [`Outcome`]: the rendered answers, or the
/// timeout / dismissal error.
fn outcome(questions: &[Question], out: Outcome) -> ToolOutput {
    match out.answers {
        Some(answers) => render(questions, &answers),
        None if out.timed_out => ToolOutput::err("ask_user: no answer from the user in time"),
        None => ToolOutput::err("ask_user: the user dismissed the questions"),
    }
}

/// One line per question plus the `{id: answer | null}` map.
fn render(questions: &[Question], answers: &[Answer]) -> ToolOutput {
    let mut lines = Vec::with_capacity(questions.len());
    let mut map = serde_json::Map::with_capacity(questions.len());
    for (q, a) in questions.iter().zip(answers) {
        let value = match a {
            Answer::Choice { index } => q.options.get(*index).map(|c| c.label.clone()),
            Answer::Text { text } => Some(text.clone()),
            Answer::Skipped => None,
        };
        lines.push(match &value {
            Some(v) => format!("{}: {v}", q.id),
            None => format!("{}: (skipped)", q.id),
        });
        map.insert(
            q.id.clone(),
            value.map_or(serde_json::Value::Null, Into::into),
        );
    }
    if answers.len() < questions.len() {
        for q in &questions[answers.len()..] {
            lines.push(format!("{}: (skipped)", q.id));
            map.insert(q.id.clone(), serde_json::Value::Null);
        }
    }
    ToolOutput {
        content: lines.join("\n"),
        is_error: false,
        structured: Some(serde_json::Value::Object(map)),
        files: Vec::new(),
    }
}

/// 132 D6: a late answer (the question is no longer pending) as the user
/// message the session gets instead of losing it — one line per question:
/// `Answer to your earlier question “<q>”: <a>` (the picked option's label,
/// the typed text, or `(skipped)`). Empty when there is nothing to say.
pub fn late_answer_text(questions: &[Question], answers: &[Answer]) -> String {
    questions
        .iter()
        .zip(answers)
        .map(|(q, a)| {
            let a = match a {
                Answer::Choice { index } => q
                    .options
                    .get(*index)
                    .map_or_else(|| format!("option {index}"), |c| c.label.clone()),
                Answer::Text { text } => text.clone(),
                Answer::Skipped => "(skipped)".to_owned(),
            };
            format!(
                "Answer to your earlier question \u{201c}{}\u{201d}: {a}",
                q.question.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_choice_text_skip_and_short_answers() {
        let q = |id: &str, n: usize| Question {
            id: id.into(),
            question: String::new(),
            options: (0..n)
                .map(|i| Choice {
                    label: format!("opt{i}"),
                    description: None,
                })
                .collect(),
            recommended: None,
        };
        let qs = [q("a", 2), q("b", 0), q("c", 2), q("d", 1)];
        let out = render(
            &qs,
            &[
                Answer::Choice { index: 1 },
                Answer::Text {
                    text: "mine".into(),
                },
                Answer::Skipped,
            ],
        );
        assert_eq!(out.content, "a: opt1\nb: mine\nc: (skipped)\nd: (skipped)");
        assert_eq!(
            out.structured.unwrap(),
            serde_json::json!({"a": "opt1", "b": "mine", "c": null, "d": null})
        );
    }

    /// 132 D6: the late-answer message names each question and its answer, one line each.
    #[test]
    fn late_answer_text_one_line_per_question() {
        let q = |text: &str, labels: &[&str]| Question {
            id: "q".into(),
            question: text.into(),
            options: labels
                .iter()
                .map(|l| Choice {
                    label: (*l).to_owned(),
                    description: None,
                })
                .collect(),
            recommended: None,
        };
        let qs = [
            q("Ship tonight? ", &["Yes", "No"]),
            q("Which region?", &[]),
            q("Keep the cache?", &["Keep"]),
        ];
        let text = late_answer_text(
            &qs,
            &[
                Answer::Choice { index: 1 },
                Answer::Text {
                    text: "eu-west".into(),
                },
                Answer::Skipped,
            ],
        );
        assert_eq!(
            text,
            "Answer to your earlier question \u{201c}Ship tonight?\u{201d}: No\n\
             Answer to your earlier question \u{201c}Which region?\u{201d}: eu-west\n\
             Answer to your earlier question \u{201c}Keep the cache?\u{201d}: (skipped)"
        );
        assert_eq!(late_answer_text(&qs, &[]), "");
    }

    /// Bus subscriber that answers the first ask it sees.
    #[derive(kameo::Actor)]
    struct Answerer(ActorRef<AskUser>, Option<Vec<Answer>>);

    impl Message<AskRequested> for Answerer {
        type Reply = ();
        async fn handle(&mut self, m: AskRequested, _: &mut Context<Self, ()>) {
            assert_eq!(m.questions.len(), 1);
            let _ = self
                .0
                .ask(AnswerAsk {
                    id: m.id,
                    answers: self.1.clone(),
                })
                .await;
        }
    }

    async fn round_trip(answers: Option<Vec<Answer>>, timeout: Duration) -> ToolOutput {
        use kameo::actor::Spawn;
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let actor = AskUser::spawn(AskUser::new(rt.clone(), timeout));
        let answerer = Answerer::spawn(Answerer(actor.clone(), answers));
        rt.subscribe::<Answerer, AskRequested>(&answerer).await;
        let args = AskArgs {
            questions: vec![Question {
                id: "q".into(),
                question: "which?".into(),
                options: vec![Choice {
                    label: "a".into(),
                    description: None,
                }],
                recommended: Some(0),
            }],
        };
        let cx = ToolCx {
            session: SessionId::new(),
            call_id: "1".into(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
        };
        Tool::call(&AskUserTool(actor), args, cx).await.unwrap()
    }

    #[tokio::test]
    async fn answered_dismissed_and_timed_out() {
        let long = Duration::from_secs(5);
        let out = round_trip(Some(vec![Answer::Choice { index: 0 }]), long).await;
        assert!(!out.is_error);
        assert_eq!(out.content, "q: a");

        let out = round_trip(None, long).await;
        assert!(out.is_error);
        assert!(out.content.contains("dismissed"));

        // Nobody answers: the actor gives up after the timeout.
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let actor = kameo::actor::Spawn::spawn(AskUser::new(rt, Duration::from_millis(20)));
        let out = Tool::call(
            &AskUserTool(actor),
            AskArgs {
                questions: vec![Question {
                    id: "q".into(),
                    question: String::new(),
                    options: Vec::new(),
                    recommended: None,
                }],
            },
            ToolCx {
                session: SessionId::new(),
                call_id: "1".into(),
                quiet: false,
                resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
            },
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("in time"));
    }

    fn one_question() -> Vec<Question> {
        vec![Question {
            id: "q".into(),
            question: "which?".into(),
            options: vec![Choice {
                label: "a".into(),
                description: None,
            }],
            recommended: Some(0),
        }]
    }

    async fn with_journal(
        timeout: Duration,
    ) -> (
        Runtime,
        ActorRef<AskUser>,
        Arc<crate::approval::tests::RecJournal>,
    ) {
        use kameo::actor::Spawn;
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let journal = Arc::new(crate::approval::tests::RecJournal::default());
        let actor = AskUser::spawn(AskUser::new(rt.clone(), timeout));
        actor
            .ask(crate::InstallJournal(journal.clone()))
            .await
            .unwrap();
        (rt, actor, journal)
    }

    fn cx(session: SessionId, call_id: &str) -> ToolCx {
        ToolCx {
            session,
            call_id: call_id.into(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
        }
    }

    /// 109 §6.1 / §6.2: the raise is a `pending` row (card as payload, the
    /// call's `waits(question)` row beside it); a *second* actor over the
    /// stored row — the restart — re-publishes the card under the **same
    /// id**, the answer to that id resolves the resumed call, and the row
    /// ends `answered` with the answers. Via the tool, so the
    /// `take_republish` path is the one exercised.
    #[tokio::test]
    async fn republish_same_id() {
        let (_rt, first, journal) = with_journal(Duration::from_secs(30)).await;
        let session = SessionId::new();
        let asked = tokio::spawn({
            let first = first.clone();
            async move {
                Tool::call(
                    &AskUserTool(first),
                    AskArgs {
                        questions: one_question(),
                    },
                    cx(session, "c1"),
                )
                .await
            }
        });
        let row = loop {
            if let Some(r) = journal.questions().first() {
                break r.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(
            (row.kind, row.state),
            (QuestionKind::Ask, QuestionState::Pending)
        );
        assert_eq!(row.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(row.payload["questions"][0]["id"], "q");
        assert_eq!(journal.waits().len(), 1);
        assert_eq!(
            journal.waits()[0].1.as_ref().unwrap().kind,
            WaitKind::Question
        );
        // The engine dies: the first actor and its waiter are gone.
        drop(asked);
        first.kill();

        let (rt2, second, journal2) = with_journal(Duration::from_secs(30)).await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        #[derive(kameo::Actor)]
        struct Seen(Arc<std::sync::Mutex<Vec<AskRequested>>>);
        impl Message<AskRequested> for Seen {
            type Reply = ();
            async fn handle(&mut self, m: AskRequested, _: &mut Context<Self, ()>) {
                crate::jobs::lock(&self.0).push(m);
            }
        }
        let sub = kameo::actor::Spawn::spawn(Seen(seen.clone()));
        rt2.subscribe::<Seen, AskRequested>(&sub).await;
        crate::registry::republish(row.clone());
        let resumed = tokio::spawn({
            let second = second.clone();
            async move {
                Tool::call(
                    &AskUserTool(second),
                    AskArgs {
                        questions: Vec::new(),
                    },
                    cx(session, "c1"),
                )
                .await
                .unwrap()
            }
        });
        let card = loop {
            if let Some(c) = crate::jobs::lock(&seen).first() {
                break c.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(card.id.0.to_string(), row.id, "same id after the restart");
        assert_eq!(card.questions, one_question());
        let pending = second.ask(PendingAsks).await.unwrap();
        assert_eq!(pending.len(), 1, "the attention item derives from this set");
        // Idempotent on the id (§6.4): a second Republish arms nothing.
        let dup = second.ask(Republish(row.clone())).await.unwrap();
        assert!(dup.answers.is_none() && !dup.timed_out);
        assert_eq!(second.ask(PendingAsks).await.unwrap().len(), 1);

        let answered = second
            .ask(AnswerAsk {
                id: card.id,
                answers: Some(vec![Answer::Choice { index: 0 }]),
            })
            .await
            .unwrap();
        assert_eq!(answered, Some(session));
        let out = resumed.await.unwrap();
        assert_eq!(out.content, "q: a");
        let rows = journal2.questions();
        assert_eq!(
            rows.first().map(|r| (r.id.as_str(), r.state)),
            Some((row.id.as_str(), QuestionState::Pending))
        );
        let last = rows.last().unwrap();
        assert_eq!(
            (last.id.as_str(), last.state),
            (row.id.as_str(), QuestionState::Answered)
        );
        assert_eq!(
            last.answer,
            Some(serde_json::json!([{"kind": "choice", "index": 0}]))
        );
        assert!(journal2.waits().last().unwrap().1.is_none(), "wait cleared");
    }

    /// §6.3: a stored row whose deadline passed while the engine was down is
    /// `timed_out` at resume and the tool answers its own timeout; an
    /// `answered` row is rendered without asking anyone.
    #[tokio::test]
    async fn republish_past_deadline_and_answered_row() {
        let (_rt, actor, journal) = with_journal(Duration::from_secs(30)).await;
        let session = SessionId::new();
        let asked_at = Utc::now() - chrono::Duration::minutes(40);
        let row = QuestionRow {
            id: uuid::Uuid::now_v7().to_string(),
            session,
            tool_call_id: Some("c2".into()),
            kind: QuestionKind::Ask,
            payload: serde_json::json!({ "id": "x", "session": session, "questions": one_question() }),
            state: QuestionState::Pending,
            asked_at,
            deadline: asked_at + chrono::Duration::minutes(30),
            answered_at: None,
            answer: None,
            answered_by: None,
        };
        crate::registry::republish(row.clone());
        let out = Tool::call(
            &AskUserTool(actor.clone()),
            AskArgs {
                questions: Vec::new(),
            },
            cx(session, "c2"),
        )
        .await
        .unwrap();
        assert!(out.is_error && out.content.contains("in time"), "{out:?}");
        let rows = journal.questions();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].id.as_str(), rows[0].state),
            (row.id.as_str(), QuestionState::TimedOut)
        );
        assert!(actor.ask(PendingAsks).await.unwrap().is_empty());

        let answered = QuestionRow {
            state: QuestionState::Answered,
            answer: Some(serde_json::json!([{"kind": "text", "text": "mine"}])),
            ..row
        };
        crate::registry::republish(answered);
        let out = Tool::call(
            &AskUserTool(actor.clone()),
            AskArgs {
                questions: Vec::new(),
            },
            cx(session, "c2"),
        )
        .await
        .unwrap();
        assert_eq!(out.content, "q: mine");
        assert_eq!(
            journal.questions().len(),
            1,
            "nothing asked, nothing journaled"
        );
    }
}
