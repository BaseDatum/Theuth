//! `ask_user`: the model asks the user one or more multiple-choice questions
//! and blocks until a UI answers. Same shape as [`crate::approval`]: an
//! [`AskUser`] actor (registered as [`crate::ASK_ACTOR`]) publishes
//! [`AskRequested`], waits for an [`AnswerAsk`], publishes [`AskResolved`].
//!
//! Skipping one question keeps the rest of the answers; dismissing the whole
//! set is `answers: None`, which the RPC layer turns into a turn cancel.

use std::collections::HashMap;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

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
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
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
/// is now resolved. `answers: None` means dismissed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerAsk {
    /// The request.
    pub id: AskId,
    /// One per question, in order; `None` dismisses the set.
    pub answers: Option<Vec<Answer>>,
}

/// Tool → actor: publish [`AskRequested`] and wait. `ask` → the answers
/// (`None` = dismissed) and whether it timed out.
struct Request {
    session: SessionId,
    questions: Vec<Question>,
}

/// Internal: drop a pending entry whose waiter gave up.
struct Expire(AskId);

/// A waiting tool call: its session and where its answers go.
type Waiter = (
    SessionId,
    Vec<Question>,
    oneshot::Sender<Option<Vec<Answer>>>,
);

/// The ask actor.
#[derive(kameo::Actor)]
pub struct AskUser {
    rt: Runtime,
    timeout: Duration,
    pending: HashMap<AskId, Waiter>,
}

impl AskUser {
    /// Build with the runtime (for publishing) and the human timeout.
    pub fn new(rt: Runtime, timeout: Duration) -> Self {
        Self {
            rt,
            timeout,
            pending: HashMap::new(),
        }
    }
}

/// What the tool gets back from the actor.
#[derive(kameo::Reply)]
struct Outcome {
    answers: Option<Vec<Answer>>,
    timed_out: bool,
}

impl Message<Request> for AskUser {
    type Reply = DelegatedReply<Outcome>;

    async fn handle(&mut self, m: Request, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let id = AskId(uuid::Uuid::now_v7());
        let (atx, arx) = oneshot::channel();
        self.pending
            .insert(id, (m.session, m.questions.clone(), atx));
        let rt = self.rt.clone();
        let timeout = self.timeout;
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            rt.publish(AskRequested {
                id,
                session: m.session,
                questions: m.questions,
            })
            .await;
            let (answers, timed_out) = match tokio::time::timeout(timeout, arx).await {
                Ok(Ok(a)) => (a, false),
                // Sender dropped: actor died; treat as dismissed.
                Ok(Err(_)) => (None, false),
                Err(_) => {
                    let _ = me.tell(Expire(id)).await;
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

impl Message<AnswerAsk> for AskUser {
    type Reply = Option<SessionId>;

    async fn handle(&mut self, m: AnswerAsk, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (session, _questions, tx) = self.pending.remove(&m.id)?;
        tx.send(m.answers).ok().map(|()| session)
    }
}

impl Message<Expire> for AskUser {
    type Reply = ();

    async fn handle(&mut self, m: Expire, _: &mut Context<Self, Self::Reply>) {
        self.pending.remove(&m.0);
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
            .map(|(id, (session, questions, _))| AskRequested {
                id: *id,
                session: *session,
                questions: questions.clone(),
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
            })
            .await?;
        let Some(answers) = out.answers else {
            return Ok(ToolOutput::err(if out.timed_out {
                "ask_user: no answer from the user in time"
            } else {
                "ask_user: the user dismissed the questions"
            }));
        };
        Ok(render(&questions, &answers))
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
            },
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("in time"));
    }
}
