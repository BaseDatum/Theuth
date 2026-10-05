//! `ask_user` question cards (design doc 98 §10.2): one rich message per
//! question, posted in sequence as the previous one is answered (the
//! `crates/signal/src/asks.rs` pacing), with option buttons, Skip, Dismiss
//! and `force_reply` so a typed answer arrives as a reply to the card. Pure
//! text/keyboard builders plus the per-ask [`Card`] state; the channel posts,
//! edits and answers.

use tools::{Answer, AskId, AskRequested};

use crate::api::{Button, Keyboard, MsgRef, Target};

/// `callback_data` prefix: `ask:<id>:<q>:<k>` (option), `ask:<id>:<q>:s`
/// (skip), `ask:<id>:x` (dismiss the set).
pub const PREFIX: &str = "ask:";
/// Buttons per row.
const PER_ROW: usize = 8;
/// Option buttons at most (Slack's cap).
const MAX_OPTIONS: usize = 23;

/// One live ask: its request, who may answer, where the current card is.
#[derive(Clone, Debug)]
pub struct Card {
    /// The request.
    pub req: AskRequested,
    /// The thread's user, allowed to answer beside the owner; `None` for owner-chat cards.
    pub thread_user: Option<u64>,
    /// Index of the question whose card is live.
    pub current: usize,
    /// Answers so far.
    pub answers: Vec<Answer>,
    /// Where the cards go.
    pub to: Target,
    /// The live card, once posted.
    pub at: Option<MsgRef>,
    /// `session <last 8> · <title>` for owner-chat cards.
    pub context: Option<String>,
    /// Every question answered or the set dismissed here.
    pub done: bool,
}

/// What follows an accepted press or typed reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Post question `i`.
    Post(usize),
    /// Every question answered: hand these to the agent.
    Answer(Vec<Answer>),
    /// Dismiss the set (cancels the turn).
    Dismiss,
}

/// What a press or a typed reply asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An option button: index into `Question::options`.
    Pick(usize),
    /// A reply to the card.
    Text(String),
    /// Skip this question.
    Skip,
    /// Dismiss the whole set.
    Dismiss,
}

/// A parsed press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    /// The ask.
    pub id: AskId,
    /// The question the pressed card shows; `None` for Dismiss.
    pub question: Option<usize>,
    /// What was pressed.
    pub kind: Kind,
}

impl Card {
    /// A fresh card for `req`.
    #[must_use]
    pub fn new(
        req: AskRequested,
        thread_user: Option<u64>,
        to: Target,
        context: Option<String>,
    ) -> Self {
        Self {
            req,
            thread_user,
            current: 0,
            answers: Vec::new(),
            to,
            at: None,
            context,
            done: false,
        }
    }

    /// Applies `kind` for `question` (`None` = the set): the text replacing
    /// the live card and what follows; `Err` = the toast to show instead.
    pub fn step(&mut self, question: Option<usize>, kind: &Kind) -> Result<(String, Next), &'static str> {
        if self.done || question.is_some_and(|q| q != self.current) {
            return Err("already answered");
        }
        let i = self.current;
        let answer = match kind {
            Kind::Dismiss => {
                self.done = true;
                return Ok((closed(&self.req, i, true, false), Next::Dismiss));
            }
            Kind::Pick(k) if *k >= self.req.questions[i].options.len() => {
                return Err("unknown option");
            }
            Kind::Pick(k) => Answer::Choice { index: *k },
            Kind::Text(text) => Answer::Text { text: text.clone() },
            Kind::Skip => Answer::Skipped,
        };
        let text = answered(&self.req, i, &answer);
        self.answers.push(answer);
        self.current += 1;
        let next = if self.current < self.req.questions.len() {
            self.at = None;
            Next::Post(self.current)
        } else {
            self.done = true;
            Next::Answer(self.answers.clone())
        };
        Ok((text, next))
    }
}

/// Card for question `i`: heading, numbered options with descriptions and
/// the `_(recommended)_` mark, the reply hint, then the owner-chat context
/// line when given; keyboard = option rows + `Skip` `Dismiss`, `force_reply`.
#[must_use]
pub fn question(req: &AskRequested, i: usize, context: Option<&str>) -> (String, Keyboard) {
    let q = &req.questions[i];
    let id = req.id.0;
    let mut text = heading("❓", req, i);
    let mut buttons = Vec::with_capacity(q.options.len().min(MAX_OPTIONS));
    for (k, o) in q.options.iter().take(MAX_OPTIONS).enumerate() {
        text.push_str(&format!("\n{}. **{}**", k + 1, o.label));
        if let Some(d) = &o.description {
            text.push_str(&format!(" — {d}"));
        }
        if q.recommended == Some(k) {
            text.push_str(" _(recommended)_");
        }
        buttons.push(Button::Callback {
            text: o.label.clone(),
            data: format!("{PREFIX}{id}:{i}:{k}"),
        });
    }
    text.push_str(if q.options.is_empty() {
        "\n_(type your answer as a reply)_"
    } else {
        "\n_(or type your answer as a reply)_"
    });
    if let Some(c) = context {
        text.push('\n');
        text.push_str(c);
    }
    let mut rows: Vec<Vec<Button>> = buttons.chunks(PER_ROW).map(<[Button]>::to_vec).collect();
    rows.push(vec![
        Button::Callback {
            text: "Skip".into(),
            data: format!("{PREFIX}{id}:{i}:s"),
        },
        Button::Callback {
            text: "Dismiss".into(),
            data: format!("{PREFIX}{id}:x"),
        },
    ]);
    (
        text,
        Keyboard {
            rows,
            force_reply: true,
        },
    )
}

/// Question `i` after it was answered here.
#[must_use]
pub fn answered(req: &AskRequested, i: usize, answer: &Answer) -> String {
    let picked = match answer {
        Answer::Choice { index } => req.questions[i]
            .options
            .get(*index)
            .map_or_else(|| index.to_string(), |o| o.label.clone()),
        Answer::Text { text } => text.clone(),
        Answer::Skipped => "_skipped_".to_owned(),
    };
    format!("{}\n→ {picked}", heading("✅", req, i))
}

/// Question `i` once the set closed without an answer here: dismissed (turn
/// cancelled), timed out, or answered from another frontend.
#[must_use]
pub fn closed(req: &AskRequested, i: usize, dismissed: bool, timed_out: bool) -> String {
    if dismissed {
        "🚫 **Questions dismissed** (turn cancelled)".to_owned()
    } else if timed_out {
        format!(
            "⌛ **Question {}/{}** — timed out",
            i + 1,
            req.questions.len()
        )
    } else {
        format!("{}\n→ _answered elsewhere_", heading("✅", req, i))
    }
}

/// Parses `ask:<id>:<q>:<k>`, `ask:<id>:<q>:s`, `ask:<id>:x`; `None` otherwise.
#[must_use]
pub fn parse(data: &str) -> Option<Press> {
    let rest = data.strip_prefix(PREFIX)?;
    let mut parts = rest.split(':');
    let id = AskId(parts.next()?.parse().ok()?);
    let second = parts.next()?;
    if second == "x" {
        return Some(Press {
            id,
            question: None,
            kind: Kind::Dismiss,
        });
    }
    let question = second.parse().ok()?;
    let kind = match parts.next()? {
        "s" => Kind::Skip,
        k => Kind::Pick(k.parse().ok()?),
    };
    Some(Press {
        id,
        question: Some(question),
        kind,
    })
}

/// `<emoji> **Question i/n** — <question>`.
fn heading(emoji: &str, req: &AskRequested, i: usize) -> String {
    format!(
        "{emoji} **Question {}/{}** — {}",
        i + 1,
        req.questions.len(),
        req.questions[i].question
    )
}

#[cfg(test)]
mod tests {
    use runtime::SessionId;
    use tools::{Choice, Question};

    use super::*;

    fn req(n_options: usize) -> AskRequested {
        let options = (0..n_options)
            .map(|k| Choice {
                label: format!("opt{k}"),
                description: (k == 0).then(|| "first".to_owned()),
            })
            .collect();
        AskRequested {
            id: AskId(uuid::Uuid::new_v4()),
            session: SessionId::new(),
            questions: vec![
                Question {
                    id: "db".into(),
                    question: "Which database?".into(),
                    options,
                    recommended: Some(0),
                },
                Question {
                    id: "free".into(),
                    question: "Anything else?".into(),
                    options: vec![],
                    recommended: None,
                },
            ],
        }
    }

    #[test]
    fn question_card_lists_options_and_chunks_buttons_by_eight() {
        let r = req(10);
        let (text, k) = question(&r, 0, Some("session 1234abcd · T"));
        assert!(text.starts_with("❓ **Question 1/2** — Which database?\n1. **opt0** — first _(recommended)_\n2. **opt1**"));
        assert!(text.ends_with("_(or type your answer as a reply)_\nsession 1234abcd · T"));
        assert!(k.force_reply);
        assert_eq!(k.rows.len(), 3);
        assert_eq!(k.rows[0].len(), 8);
        assert_eq!(k.rows[1].len(), 2);
        assert_eq!(
            k.rows[2],
            vec![
                Button::Callback {
                    text: "Skip".into(),
                    data: format!("ask:{}:0:s", r.id.0)
                },
                Button::Callback {
                    text: "Dismiss".into(),
                    data: format!("ask:{}:x", r.id.0)
                }
            ]
        );
        let Button::Callback { data, .. } = &k.rows[0][3] else {
            panic!()
        };
        assert!(data.len() <= 64);
        assert_eq!(
            parse(data),
            Some(Press {
                id: r.id,
                question: Some(0),
                kind: Kind::Pick(3)
            })
        );
        let (text, _) = question(&r, 1, None);
        assert!(text.ends_with("_(type your answer as a reply)_"));
    }

    #[test]
    fn presses_parse_and_garbage_does_not() {
        let id = AskId(uuid::Uuid::new_v4());
        assert_eq!(
            parse(&format!("ask:{}:x", id.0)),
            Some(Press {
                id,
                question: None,
                kind: Kind::Dismiss
            })
        );
        assert_eq!(
            parse(&format!("ask:{}:2:s", id.0)),
            Some(Press {
                id,
                question: Some(2),
                kind: Kind::Skip
            })
        );
        assert_eq!(parse(&format!("ask:{}:2", id.0)), None);
        assert_eq!(parse(&format!("ask:{}:2:q", id.0)), None);
        assert_eq!(parse("ap:x"), None);
    }

    #[test]
    fn card_steps_through_the_questions_and_rejects_stale_presses() {
        let r = req(2);
        let to = Target {
            chat: 1,
            thread: None,
        };
        let mut c = Card::new(r.clone(), Some(7), to, None);
        assert_eq!(c.step(Some(1), &Kind::Skip), Err("already answered"));
        assert_eq!(c.step(Some(0), &Kind::Pick(5)), Err("unknown option"));
        let (text, next) = c.step(Some(0), &Kind::Pick(1)).unwrap();
        assert_eq!(text, "✅ **Question 1/2** — Which database?\n→ opt1");
        assert_eq!(next, Next::Post(1));
        let (text, next) = c.step(Some(1), &Kind::Text("no".into())).unwrap();
        assert_eq!(text, "✅ **Question 2/2** — Anything else?\n→ no");
        assert_eq!(
            next,
            Next::Answer(vec![
                Answer::Choice { index: 1 },
                Answer::Text { text: "no".into() }
            ])
        );
        assert!(c.done);
        assert_eq!(c.step(Some(1), &Kind::Skip), Err("already answered"));

        let mut c = Card::new(r.clone(), None, to, None);
        let (text, next) = c.step(None, &Kind::Dismiss).unwrap();
        assert_eq!(text, "🚫 **Questions dismissed** (turn cancelled)");
        assert_eq!(next, Next::Dismiss);
        assert_eq!(closed(&r, 1, false, true), "⌛ **Question 2/2** — timed out");
        assert!(closed(&r, 0, false, false).ends_with("→ _answered elsewhere_"));
    }
}
