use super::*;


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
    /// Stable identity of the shown core prompt, independent of feature badges.
    pub(crate) fn prompt_id(&self) -> Option<Uuid> {
        self.approvals.front().map(|request| request.id.0)
            .or_else(|| self.asks.front().map(|ask| ask.req.id.0))
    }

    /// The approval currently shown, if any.
    pub fn approval(&self) -> Option<&ApprovalRequested> {
        self.approvals.front()
    }

    /// The ask currently shown, if any.
    pub fn ask(&self) -> Option<&Ask> {
        self.asks.front()
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
            self.component_notify(crate::components::Lifecycle::PromptChanged { resolved: None });
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
                    self.component_notify(crate::components::Lifecycle::PromptChanged { resolved: None });
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
            self.component_notify(crate::components::Lifecycle::PromptChanged { resolved: None });
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
