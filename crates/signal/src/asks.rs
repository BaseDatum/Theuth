//! `ask_user` on the phone (design doc 09 §10): one question at a time as
//! plain text, answered by replying. The channel owns a [`Pending`] map:
//! `number → Entry`; one ask is open per number, later ones queue behind it.

use std::collections::{HashMap, VecDeque};

use tools::{Answer, AskId, AskRequested, AskResolved, Question};

/// The open ask for one phone thread.
#[derive(Debug)]
pub struct Entry {
    /// The request.
    pub req: AskRequested,
    /// Thread (E.164) the questions go to.
    pub number: String,
    /// Index of the question currently on the phone.
    pub current: usize,
    /// Answers so far, one per question.
    pub answers: Vec<Answer>,
    /// We already sent the answers (or a dismiss); later texts are not answers.
    pub done: bool,
    /// Further asks for this number, started when this one resolves.
    pub queue: VecDeque<AskRequested>,
}

impl Entry {
    fn new(req: AskRequested, number: &str, queue: VecDeque<AskRequested>) -> Self {
        Self {
            req,
            number: number.to_owned(),
            current: 0,
            answers: Vec::new(),
            done: false,
            queue,
        }
    }

    /// The question awaiting an answer.
    #[must_use]
    pub fn question(&self) -> &Question {
        &self.req.questions[self.current]
    }

    /// Records `answer`; true when every question is answered.
    pub fn record(&mut self, answer: Answer) -> bool {
        self.answers.push(answer);
        self.current += 1;
        self.current >= self.req.questions.len()
    }
}

/// Open asks by number.
#[derive(Debug, Default)]
pub struct Pending {
    open: HashMap<String, Entry>,
}

impl Pending {
    /// Registers `req` for `number`. True when it is now open (send its first
    /// question); false when queued behind the one already open.
    pub fn insert(&mut self, req: AskRequested, number: &str) -> bool {
        match self.open.get_mut(number) {
            Some(e) => {
                e.queue.push_back(req);
                false
            }
            None => {
                self.open
                    .insert(number.to_owned(), Entry::new(req, number, VecDeque::new()));
                true
            }
        }
    }

    /// The ask `number` can still answer.
    pub fn answerable(&mut self, number: &str) -> Option<&mut Entry> {
        self.open.get_mut(number).filter(|e| !e.done)
    }

    /// Drops the entry for `id`; also the next queued ask for that number
    /// if any, now open (send its first question). `None` if unknown.
    pub fn resolve(&mut self, id: AskId) -> Option<(Entry, Option<AskRequested>)> {
        let number = self.open.iter().find(|(_, e)| e.req.id == id)?.0.clone();
        let mut entry = self.open.remove(&number)?;
        let next = entry.queue.pop_front();
        if let Some(req) = &next {
            let rest = std::mem::take(&mut entry.queue);
            self.open
                .insert(number.clone(), Entry::new(req.clone(), &number, rest));
        }
        Some((entry, next))
    }
}

/// Question `i` of `req` as sent to the phone.
#[must_use]
pub fn question_text(req: &AskRequested, i: usize) -> String {
    let q = &req.questions[i];
    let mut out = format!(
        "❓ Question {}/{}: {}\n",
        i + 1,
        req.questions.len(),
        q.question
    );
    for (n, c) in q.options.iter().enumerate() {
        out.push_str(&format!("{}. {}", n + 1, c.label));
        if let Some(d) = &c.description {
            out.push_str(" — ");
            out.push_str(d);
        }
        if q.recommended == Some(n) {
            out.push_str(" ★");
        }
        out.push('\n');
    }
    out.push_str(if q.options.is_empty() {
        "Reply with your answer, `skip`, or `dismiss` (ends the turn)"
    } else {
        "Reply with a number, your own answer, `skip`, or `dismiss` (ends the turn)"
    });
    out
}

/// What a text reply to a question means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Option index.
    Pick(usize),
    /// Freeform answer.
    Text(String),
    /// Leave this question unanswered.
    Skip,
    /// Close the whole set (ends the turn).
    Dismiss,
}

/// A bare `1..=n` → `Pick`; `skip` → `Skip`; `dismiss`/`cancel` → `Dismiss`;
/// else `Text`. `Err` is the hint to send for an out-of-range number.
pub fn parse_reply(text: &str, q: &Question) -> Result<Reply, String> {
    let t = text.trim();
    let n = q.options.len();
    if n > 0
        && let Ok(k) = t.parse::<usize>()
    {
        return if (1..=n).contains(&k) {
            Ok(Reply::Pick(k - 1))
        } else {
            Err(format!("pick 1–{n}, your own answer, skip or dismiss"))
        };
    }
    Ok(match t.to_lowercase().as_str() {
        "skip" => Reply::Skip,
        "dismiss" | "cancel" => Reply::Dismiss,
        _ => Reply::Text(t.to_owned()),
    })
}

/// Reply text after `answer_ask` returned.
#[must_use]
pub fn outcome(res: Result<bool, String>, ok: &str) -> String {
    match res {
        Ok(true) => ok.to_owned(),
        Ok(false) => "questions were already answered".into(),
        Err(e) => format!("⚠️ {e}"),
    }
}

/// What to tell the phone when `entry` resolved with `res`, if anything.
#[must_use]
pub fn closing_note(entry: &Entry, res: &AskResolved) -> Option<&'static str> {
    if res.timed_out {
        Some("⌛ questions expired")
    } else if entry.done {
        None
    } else if res.dismissed {
        Some("questions dismissed elsewhere")
    } else {
        Some("questions answered elsewhere")
    }
}

#[cfg(test)]
mod tests {
    use runtime::SessionId;
    use tools::Choice;

    use super::*;

    fn choice(label: &str, description: Option<&str>) -> Choice {
        Choice {
            label: label.into(),
            description: description.map(Into::into),
        }
    }

    fn lang() -> Question {
        Question {
            id: "lang".into(),
            question: "Which language for the CLI?".into(),
            options: vec![
                choice("Rust", Some("fast, static binary")),
                choice("Go", Some("simple concurrency")),
                choice("Python", None),
            ],
            recommended: Some(0),
        }
    }

    fn name() -> Question {
        Question {
            id: "name".into(),
            question: "Project name?".into(),
            options: Vec::new(),
            recommended: None,
        }
    }

    fn req(questions: Vec<Question>) -> AskRequested {
        AskRequested {
            id: AskId(uuid::Uuid::new_v4()),
            session: SessionId::new(),
            questions,
        }
    }

    #[test]
    fn question_text_marks_recommended_and_numbers_options() {
        let r = req(vec![lang(), name()]);
        assert_eq!(
            question_text(&r, 0),
            "❓ Question 1/2: Which language for the CLI?\n\
             1. Rust — fast, static binary ★\n\
             2. Go — simple concurrency\n\
             3. Python\n\
             Reply with a number, your own answer, `skip`, or `dismiss` (ends the turn)"
        );
        assert_eq!(
            question_text(&r, 1),
            "❓ Question 2/2: Project name?\n\
             Reply with your answer, `skip`, or `dismiss` (ends the turn)"
        );
    }

    #[test]
    fn parse_reply_grammar() {
        let q = lang();
        assert_eq!(parse_reply(" 2 ", &q), Ok(Reply::Pick(1)));
        assert_eq!(
            parse_reply("4", &q),
            Err("pick 1–3, your own answer, skip or dismiss".into())
        );
        assert!(parse_reply("0", &q).is_err());
        assert_eq!(parse_reply("SKIP", &q), Ok(Reply::Skip));
        assert_eq!(parse_reply("dismiss", &q), Ok(Reply::Dismiss));
        assert_eq!(parse_reply("Cancel", &q), Ok(Reply::Dismiss));
        assert_eq!(
            parse_reply("  zig maybe ", &q),
            Ok(Reply::Text("zig maybe".into()))
        );
        // No options: digits are just text.
        assert_eq!(parse_reply("2", &name()), Ok(Reply::Text("2".into())));
    }

    #[test]
    fn second_ask_queues_until_first_resolves() {
        let mut p = Pending::default();
        let a = req(vec![name()]);
        let b = req(vec![lang()]);
        assert!(p.insert(a.clone(), "+1"));
        assert!(!p.insert(b.clone(), "+1"));
        assert_eq!(p.answerable("+1").unwrap().req.id, a.id);
        let (entry, next) = p.resolve(a.id).unwrap();
        assert_eq!(entry.req.id, a.id);
        assert_eq!(next.unwrap().id, b.id);
        assert_eq!(p.answerable("+1").unwrap().req.id, b.id);
        assert!(p.resolve(b.id).unwrap().1.is_none());
        assert!(p.answerable("+1").is_none());
    }
}
