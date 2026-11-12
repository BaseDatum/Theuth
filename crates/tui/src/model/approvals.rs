use super::*;

/// One `/double` subcommand (126 §10).
#[derive(Clone, Debug, PartialEq)]
pub enum DoubleCmd {
    /// `double_status`.
    Status,
    /// `enabled: false` in config.yaml via `double_settings_set`; takes effect at restart.
    Off,
    /// `double_pause(on)`.
    Pause(bool),
    /// `double_train(tier, venue)`.
    Train {
        /// `a` | `b`.
        tier: double::Tier,
        /// `auto` | `local` | `node:<name>`.
        venue: double::Venue,
    },
    /// `double_rollback(version)`.
    Rollback(String),
    /// `double_set_cap(class, rung, mutating_opt_in)`.
    Cap {
        /// The §3 key.
        class: String,
        /// 0..=3.
        rung: u8,
        /// `opt-in` was given.
        opt_in: bool,
    },
}

impl DoubleCmd {
    /// The usage line.
    pub const USAGE: &'static str = "usage: /double [status|off|pause [off]|train [a|b] [auto|local|node:<name>]|rollback <version>|cap <class> <rung> [opt-in]]";

    /// Parse the text after `/double`; `None` is a usage error. A class key may
    /// contain spaces (`approval:shell:mutating:git push:root`), so `cap` reads
    /// its trailing `<rung> [opt-in]` first and the rest is the key.
    pub fn parse(arg: &str) -> Option<Self> {
        let mut w = arg.split_whitespace();
        let head = w.next().unwrap_or("status");
        if head == "cap" {
            let mut rest: Vec<&str> = w.collect();
            let opt_in = rest.last() == Some(&"opt-in");
            if opt_in {
                rest.pop();
            }
            let rung: u8 = rest.pop()?.parse().ok().filter(|r| *r <= 3)?;
            if rest.is_empty() {
                return None;
            }
            return Some(Self::Cap {
                class: rest.join(" "),
                rung,
                opt_in,
            });
        }
        Some(match (head, w.next(), w.next(), w.next()) {
            ("status", None, ..) => Self::Status,
            ("off", None, ..) => Self::Off,
            ("pause", None, ..) => Self::Pause(true),
            ("pause", Some("off"), None, _) => Self::Pause(false),
            ("train", tier, venue, None) => Self::Train {
                tier: match tier.unwrap_or("a") {
                    "a" => double::Tier::A,
                    "b" => double::Tier::B,
                    _ => return None,
                },
                venue: match double::Venue::parse(venue.unwrap_or("auto"))? {
                    double::Venue::Off => return None,
                    v => v,
                },
            },
            ("rollback", Some(v), None, _) => Self::Rollback(v.to_owned()),
            _ => return None,
        })
    }
}

/// The chip text for one card's prediction (126 §10): `double: Allow 91 % · L1`, or at L2
/// the countdown `double: Allow 91 % · L2 · answers in 18 s (answer to take over)`.
/// `none` is the double saying it is unsure.
pub fn double_chip(p: &DoublePredicted, now_ms: u64) -> String {
    let mut s = format!("double: {} {:.0} % · L{}", p.choice, p.p * 100.0, p.rung);
    if let Some(left) = double::chip::grace_left_s(p, now_ms) {
        s.push_str(&format!(" · answers in {left} s (answer to take over)"));
    }
    s
}

/// A pending `ask_user` being answered one question at a time.
#[derive(Debug)]
pub struct Ask {
    /// The request.
    pub req: AskRequested,
    /// Index of the question being shown.
    pub current: usize,
    /// Row under the cursor: an option, or `options.len()` for "type your own".
    pub cursor: usize,
    /// Answers so far, one per finished question.
    pub answers: Vec<Answer>,
    /// The input box is taking a freeform answer.
    pub typing: bool,
}

impl Ask {
    pub(super) fn new(req: AskRequested) -> Self {
        let cursor = req
            .questions
            .first()
            .and_then(|q| q.recommended)
            .unwrap_or(0);
        Self {
            req,
            current: 0,
            cursor,
            answers: Vec::new(),
            typing: false,
        }
    }

    /// The question being shown.
    pub fn question(&self) -> &tools::Question {
        &self.req.questions[self.current]
    }
}

impl Model {

    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// The ask currently shown, if any.
    pub fn ask(&self) -> Option<&Ask> {
        self.asks.front()
    }

    /// The double's prediction for the shown approval (first) or ask, when it has one.
    fn shown_prediction(&self) -> Option<&DoublePredicted> {
        let id = match (self.approval(), self.ask()) {
            (Some(a), _) => a.id.0,
            (None, Some(a)) => a.req.id.0,
            (None, None) => return None,
        };
        self.double.get(&id.to_string())
    }

    /// The double's chip for the shown approval or ask, when it has predicted it.
    pub fn double_chip(&self) -> Option<String> {
        self.shown_prediction().map(|p| double_chip(p, now_ms()))
    }

    /// The shown card is in an L2 grace: frames are needed for the countdown.
    pub(super) fn double_counting_down(&self) -> bool {
        self.shown_prediction()
            .is_some_and(|p| p.will_act_at.is_some())
    }
}

impl Model {

    /// A pending approval was answered (here or elsewhere): its row runs again.
    pub(super) fn approval_done(&mut self, session: SessionId) {
        if let Some(row) = self.agents.get_mut(&session)
            && matches!(row.info.state, AgentState::Approval { .. })
        {
            row.info.state = AgentState::Running;
            row.tool = None;
        }
    }
}

impl Model {

    /// Answer the shown approval and dismiss it locally; `ApprovalResolved`
    /// (or a `Resolve` failure) is then a no-op for this id.
    pub fn decide(&mut self, decision: Decision, tx: &UnboundedSender<Command>) {
        if let Some(req) = self.approvals.pop_front() {
            self.approval_done(req.session);
            send(
                tx,
                Command::Resolve {
                    id: req.id,
                    decision,
                },
            );
        }
    }

    /// A key while an ask is shown. Returns text for the input box when it
    /// must change (entering or leaving freeform typing, sending an answer).
    pub fn ask_key(
        &mut self,
        code: KeyCode,
        input: &str,
        tx: &UnboundedSender<Command>,
    ) -> Option<String> {
        let a = self.asks.front_mut()?;
        if a.typing {
            return match code {
                KeyCode::Enter => {
                    let text = input.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let text = text.to_owned();
                    self.ask_answer(Answer::Text { text }, tx);
                    Some(String::new())
                }
                KeyCode::Esc => {
                    a.typing = false;
                    Some(String::new())
                }
                _ => None,
            };
        }
        let n = a.question().options.len();
        match code {
            KeyCode::Up => a.cursor = a.cursor.saturating_sub(1),
            KeyCode::Down => a.cursor = (a.cursor + 1).min(n),
            KeyCode::Enter if a.cursor < n => {
                let index = a.cursor;
                self.ask_answer(Answer::Choice { index }, tx);
            }
            KeyCode::Enter => a.typing = true,
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let d = usize::from(c as u8 - b'0');
                if (1..=n).contains(&d) {
                    self.ask_answer(Answer::Choice { index: d - 1 }, tx);
                }
            }
            KeyCode::Char('s') | KeyCode::Tab => self.ask_answer(Answer::Skipped, tx),
            KeyCode::Esc => {
                if let Some(a) = self.asks.pop_front() {
                    send(
                        tx,
                        Command::AnswerAsk {
                            id: a.req.id,
                            answers: None,
                        },
                    );
                }
            }
            _ => {}
        }
        None
    }

    /// Record the shown question's answer; the last one sends the set.
    fn ask_answer(&mut self, answer: Answer, tx: &UnboundedSender<Command>) {
        let Some(a) = self.asks.front_mut() else {
            return;
        };
        a.answers.push(answer);
        a.typing = false;
        a.current += 1;
        if a.current < a.req.questions.len() {
            a.cursor = a.question().recommended.unwrap_or(0);
            return;
        }
        if let Some(a) = self.asks.pop_front() {
            send(
                tx,
                Command::AnswerAsk {
                    id: a.req.id,
                    answers: Some(a.answers),
                },
            );
        }
    }
}

impl Model {

    /// `/approvals [on|off] [always]`: bare shows the state; `always` switches
    /// the persisted default, otherwise the root session.
    pub(super) fn approvals_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let mut words = arg.split_whitespace();
        let enabled = match words.next() {
            None => {
                self.entries.push(Entry::Info(format!(
                    "approvals: {} (this session), {} (default)",
                    on_off(!self.approvals_off),
                    on_off(self.approvals_global)
                )));
                return;
            }
            Some("on") => true,
            Some("off") => false,
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/approvals: expected on|off, got {w:?}"
                )));
                return;
            }
        };
        let session = match words.next() {
            Some("always") => None,
            None => match self.session {
                Some(id) => Some(id),
                None => {
                    self.entries.push(Entry::Error(
                        "no session yet: send a message first, or use `/approvals off always`"
                            .into(),
                    ));
                    return;
                }
            },
            Some(w) => {
                self.entries.push(Entry::Error(format!(
                    "/approvals: expected `always`, got {w:?}"
                )));
                return;
            }
        };
        if session.is_some() {
            self.approvals_off = !enabled;
        }
        send(tx, Command::SetApprovals { session, enabled });
    }
}
