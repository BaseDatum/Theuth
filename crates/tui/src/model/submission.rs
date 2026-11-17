use super::*;

impl Model {

    /// Handle a submitted input line: slash command or message.
    pub fn submit(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        self.submit_with(line, false, tx);
    }

    /// Ctrl/Cmd+Enter: like `submit`, but a mid-turn message interjects —
    /// the turn is cancelled and the message starts the next one now. Slash
    /// commands, prompt answers and idle sessions behave as `submit`.
    pub fn submit_now(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        self.submit_with(line, true, tx);
    }

    fn submit_with(&mut self, line: &str, interject: bool, tx: &UnboundedSender<Command>) {
        self.component_commands = Some(tx.clone());
        self.component_reconcile_core_focus();
        let line = line.trim();
        // An empty note is an answer (keep / none); every other step waits for text.
        if line.is_empty()
            && !self.components.editor().is_some_and(|(_, editor)| editor.submit_empty)
        {
            return;
        }
        // Anything you send is what you want to see land.
        self.anchor = None;
        if self.component_submit(line) { return; }
        // `/edit` in progress: a plain line replaces the turn being edited
        // (slash commands still run and leave the edit pending).
        if let Some((idx, turn)) = self.editing.filter(|_| !line.starts_with('/')) {
            self.editing = None;
            let Some(session) = self.session else { return };
            let (text, _) = self.expand_pastes(line);
            self.entries.truncate(idx);
            self.entries.push(Entry::User {
                text: line.to_owned(),
                turn: None,
            });
            self.sent = Some(text.clone());
            send(
                tx,
                Command::EditTurn {
                    session,
                    turn,
                    text,
                },
            );
            return;
        }
        let (cmd, arg) = line
            .split_once(' ')
            .map_or((line, ""), |(c, a)| (c, a.trim()));
        if self.component_slash(cmd, arg) { return; }
        match cmd {
            "/quit" | "/exit" => {
                // Leaving the process leaves the owned private tree (51 §4.3).
                self.leave_private(tx);
                self.quit = true;
            }
            "/sessions" => send(tx, Command::ListSessions),
            "/take" => match self.session {
                Some(s) => send(tx, Command::TakeSession(s)),
                None => self.entries.push(Entry::Error("no session open".into())),
            },
            // 75 §10: on an ACP child, `/resume` alone restarts its harness.
            "/resume" if arg.is_empty() && self.agent.is_some() => {
                if let Some(s) = self.session {
                    send(tx, Command::ResumeChild(s));
                }
            }
            "/resume" => match arg.parse::<SessionId>() {
                Ok(id) => self.resume(id, tx),
                Err(_) => self
                    .entries
                    .push(Entry::Error(format!("not a session id: {arg:?}"))),
            },
            "/new" if arg == "private" => self.new_private(tx),
            "/new" => {
                self.leave_private(tx);
                self.reset();
                self.entries.push(Entry::Info("new session".into()));
            }
            "/approvals" => self.approvals_cmd(arg, tx),

            "/parent" => match self.session_parent {
                Some(parent) => self.resume(parent, tx),
                None => self
                    .entries
                    .push(Entry::Info("this is the top-level session".into())),
            },
            "/project" => match (self.session, arg) {
                (None, _) => self.entries.push(Entry::Info("no session".into())),
                (Some(_), "") => self
                    .entries
                    .push(Entry::Error("usage: /project <label|off>".into())),
                (Some(session), label) => send(
                    tx,
                    Command::SetSessionProject {
                        session,
                        project: (label != "off" && label != "clear").then(|| label.to_owned()),
                    },
                ),
            },
            "/compact" => self.compact_cmd(arg, tx),
            // `/endless`: the old name, accepted but not listed.
            "/autocontext" | "/endless" => self.endless_cmd(arg, tx),
            "/clear" => match self.session {
                Some(session) => send(
                    tx,
                    Command::Compact {
                        session,
                        style: Some(CompactStyle::Clear),
                    },
                ),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            // On a watch-only child this is Esc's rule (141 §10.1): only when cancellable.
            "/cancel" if self.observed.is_some() => self.cancel(tx),
            "/cancel" => {
                if let Some(session) = self.session {
                    send(tx, Command::Cancel(session));
                }
            }
            "/undo" | "/redo" => match self.session {
                Some(session) if cmd == "/undo" => send(tx, Command::Undo(session)),
                Some(session) => send(tx, Command::Redo(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/edit" => self.turn_cmd(TurnAction::Edit, cmd, arg, tx),
            "/regen" => self.turn_cmd(TurnAction::Regen, cmd, arg, tx),
            "/fork" => self.turn_cmd(TurnAction::Fork, cmd, arg, tx),
            // `/<skill> [args]` hands the skill to the model (04 §2.4); any other `/word` — a path, a typo —
            // is message text and takes the arm below.
            // A watch-only child takes neither (141 §10.1): the hint instead.
            _ if self.observed.is_some() => self.watch_swallow(),
            c if c.starts_with('/') && self.component_slash_fallback(c, arg) => {}
            _ => self.send_text(line, interject, tx),
        }
        self.component_reconcile_core_focus();
    }

    /// Send `line` as message text: a steer mid-turn (interjecting when
    /// asked), else the next turn's input. Also the raw path of a harness
    /// slash command picked from the popup (141 D15 c): no `use_skill`.
    pub(super) fn send_text(&mut self, line: &str, interject: bool, tx: &UnboundedSender<Command>) {
        self.component_notify(crate::components::Lifecycle::ComposerSubmitted(line));
        let (text, files) = self.expand_pastes(line);
        match self.session {
            // Mid-turn, or mid-`/compact` (a mini-turn without `TurnStarted`;
            // 07 §8.6): the card waits below the transcript until
            // `InputInjected` says where in the conversation it landed.
            Some(session) if self.streaming || self.compacting => {
                let id = InputId::new();
                self.queued.push(QueuedInput {
                    id,
                    text: text.clone(),
                    channel: None,
                    files: Vec::new(),
                });
                send(
                    tx,
                    Command::Steer {
                        session,
                        id,
                        text,
                        files,
                        interject,
                    },
                );
            }
            session => {
                self.entries.push(Entry::User {
                    text: line.to_owned(),
                    turn: None,
                });
                self.sent = Some(text.clone());
                self.awaiting_session = session.is_none();
                send(
                    tx,
                    Command::Send {
                        session,
                        text,
                        files,
                        draft: self.draft_options(),
                    },
                );
            }
        }
    }
}
