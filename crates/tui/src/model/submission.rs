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
        let line = line.trim();
        // An empty note is an answer (keep / none); every other step waits for text.
        if line.is_empty() && !matches!(self.prompt, Some(Prompt::SecretNote { .. })) {
            return;
        }
        // Anything you send is what you want to see land.
        self.anchor = None;
        if self.prompt.is_some() {
            self.prompt_submit(line, tx);
            return;
        }
        if self.component_submit(line) { return; }
        self.component_notify(crate::components::Lifecycle::ComposerSubmitted(line));
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
            "/search" => self.open_search(arg, tx),
            "/attach" => self.open_attach(arg, tx),
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
            // 75 §8: an ACP session picks from its harness's options.
            "/model" | "/effort" if self.agent.is_some() && arg.is_empty() => {
                if let Some(s) = self.session {
                    send(tx, Command::HarnessOptions(s));
                }
            }
            "/model" | "/effort" if self.agent.is_some() => {
                if let Some(s) = self.session {
                    send(
                        tx,
                        Command::SetHarnessOption {
                            session: s,
                            config_id: cmd[1..].to_owned(),
                            value: arg.to_owned(),
                        },
                    );
                }
            }
            "/model" if arg.is_empty() => send(tx, Command::ListModels),
            "/model" => self.pick_model(arg.to_owned(), tx),
            "/title" if arg.is_empty() => send(tx, Command::GetTitleModel),
            "/title" => send(
                tx,
                Command::SetTitleModel((arg != "off" && arg != "clear").then(|| arg.to_owned())),
            ),
            "/vision" => {
                self.awaiting_vision = true;
                send(
                    tx,
                    if arg.is_empty() {
                        Command::GetVisionModel
                    } else {
                        Command::SetVisionModel(
                            (arg != "off" && arg != "clear").then(|| arg.to_owned()),
                        )
                    },
                );
            }
            "/effort" if arg.is_empty() => {
                let items = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.closest(&self.model_id))
                    .map(|m| m.efforts())
                    .unwrap_or_default();
                self.open_efforts(items, PickTarget::Session);
            }
            "/effort" => self.set_effort((arg != "clear").then(|| arg.to_owned()), tx),
            "/aws" if arg.is_empty() => send(tx, Command::ListAwsProfiles),
            "/aws" => send(
                tx,
                Command::SetAwsProfile((arg != "clear").then(|| arg.to_owned())),
            ),
            "/approvals" => self.approvals_cmd(arg, tx),
            "/advisor" => self.advisor_cmd(arg, tx),
            "/communication" => self.communication_cmd(arg, tx),
            "/peers" => self.peers_cmd(arg, tx),
            "/a2a" if arg.is_empty() => send(tx, Command::A2aStatus),
            "/a2a" if arg == "reload" => send(tx, Command::ReloadA2a),
            "/a2a" => self
                .entries
                .push(Entry::Error("usage: /a2a [reload]".into())),
            "/double" => match DoubleCmd::parse(arg) {
                Some(cmd) => send(tx, Command::Double(cmd)),
                None => self.entries.push(Entry::Error(DoubleCmd::USAGE.into())),
            },
            "/mcp" | "/tools" if arg.is_empty() => {
                self.awaiting_mcp = true;
                send(tx, Command::ListMcp(self.session));
            }
            "/mcp" | "/tools" => self.mcp_cmd(arg, tx),
            "/acp" if arg.is_empty() => {
                self.awaiting_acp = true;
                send(tx, Command::ListAcp { probe: false });
                send(tx, Command::ListAcpRegistry);
            }
            "/acp" => match arg.split_once(' ').map(|(s, r)| (s, r.trim())) {
                Some(("add", id)) if !id.is_empty() => send(tx, Command::AddAcp(id.to_owned())),
                Some(("remove", name)) if !name.is_empty() => {
                    send(tx, Command::RemoveAcp(name.to_owned()));
                }
                _ => self.entries.push(Entry::Error(
                    "usage: /acp [add <registry-id> | remove <name>]".into(),
                )),
            },
            "/skills" => self.skills_cmd(arg, tx),
            "/prompt" => send(tx, Command::ShowPrompt(self.session)),
            "/knowledge" => self.knowledge_cmd(arg, tx),
            "/login" if arg.is_empty() => send(tx, Command::ListLogins),
            "/login" => send(tx, Command::Login(arg.to_owned())),
            "/logout" if arg.is_empty() => self
                .entries
                .push(Entry::Error("usage: /logout <name>".into())),
            "/logout" => send(tx, Command::Logout(arg.to_owned())),
            "/todos" => self.toggle_todos(tx),
            // The line was taken out of the editor to get here: the region lands in an empty box.
            "/dictate" => {
                if let Some(cmd) = self.toggle_dictation(String::new(), String::new()) {
                    send(tx, cmd);
                }
            }
            "/voice" => match voice_cmd(arg) {
                Ok(cmd) => send(tx, cmd),
                Err(e) => self.entries.push(Entry::Error(e.into())),
            },
            "/talk" => {
                if let Some(cmd) = self.toggle_talk(false) {
                    send(tx, cmd);
                }
            }
            "/broadcast" => self.broadcast_cmd(arg, tx),
            "/agents" => match self.session {
                Some(session) => {
                    self.agents_picker_pending = true;
                    send(tx, Command::ListAgents(session));
                }
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/parent" => match self.session_parent {
                Some(parent) => self.resume(parent, tx),
                None => self
                    .entries
                    .push(Entry::Info("this is the top-level session".into())),
            },
            "/jobs" => match (self.session, arg.split_once(' ')) {
                (None, _) => self.entries.push(Entry::Info("no session".into())),
                (Some(session), None) if arg.is_empty() => send(tx, Command::ListJobs(session)),
                (Some(session), Some(("kill", id))) if !id.trim().is_empty() => send(
                    tx,
                    Command::CancelJob {
                        session,
                        id: id.trim().to_owned(),
                    },
                ),
                _ => self
                    .entries
                    .push(Entry::Error("usage: /jobs [kill <id>]".into())),
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
            "/cost" => match self.session {
                Some(session) => {
                    self.awaiting_cost = true;
                    send(tx, Command::SessionCost(session));
                }
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/terminals" => match self.session {
                Some(session) => send(tx, Command::ListTerminals(session)),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/apps" if !self.app_enabled => self.entries.push(Entry::Info(
                "apps are off (app.enabled); /artifacts lists their sources".into(),
            )),
            "/artifacts" | "/apps" => match self.session {
                Some(session) => send(
                    tx,
                    Command::ListArtifacts {
                        session,
                        pick: true,
                        apps: cmd == "/apps",
                    },
                ),
                None => self.entries.push(Entry::Info("no session".into())),
            },
            "/schedules" => match arg.split_once(' ') {
                None if arg.is_empty() => send(tx, Command::ListSchedules),
                Some(("run", name)) if !name.trim().is_empty() => {
                    send(tx, Command::FireSchedule(name.trim().to_owned()))
                }
                Some(("history", name)) if !name.trim().is_empty() => {
                    send(tx, Command::ScheduleHistory(name.trim().to_owned()))
                }
                Some(("pause", name)) if !name.trim().is_empty() => {
                    send(tx, Command::EnableSchedule(name.trim().to_owned(), false))
                }
                Some(("resume", name)) if !name.trim().is_empty() => {
                    send(tx, Command::EnableSchedule(name.trim().to_owned(), true))
                }
                Some(("test", name)) if !name.trim().is_empty() => {
                    send(tx, Command::TestSchedule(name.trim().to_owned()))
                }
                _ => self.entries.push(Entry::Info(SCHEDULES_USAGE.into())),
            },
            // 140 §13.1: list/status/pause/resume/stop/steer are direct RPC; a goal (or a control in words) hands
            // the `autoresearch` skill to the model (the model calls autoresearch_draft/status/control).
            "/autoresearch" => match arg
                .split_once(' ')
                .map_or((arg, ""), |(a, b)| (a, b.trim()))
            {
                ("", _) | ("list", "") | ("status", "") => {
                    send(tx, Command::AutoresearchStatus(None))
                }
                ("status", slug) if !slug.is_empty() => {
                    send(tx, Command::AutoresearchStatus(Some(slug.to_owned())))
                }
                (op @ ("pause" | "resume" | "stop" | "stop_now"), slug) if !slug.is_empty() => {
                    let op = match op {
                        "pause" => autoresearch::ControlOp::Pause,
                        "resume" => autoresearch::ControlOp::Resume,
                        "stop" => autoresearch::ControlOp::StopAfter,
                        _ => autoresearch::ControlOp::StopNow,
                    };
                    send(tx, Command::AutoresearchControl(slug.to_owned(), op))
                }
                ("steer", rest) if rest.contains(' ') => {
                    let (slug, text) = rest.split_once(' ').unwrap_or((rest, ""));
                    send(
                        tx,
                        Command::AutoresearchSteer(slug.to_owned(), text.trim().to_owned()),
                    )
                }
                // One word that is not a verb: a slug (the engine says so if it is not).
                (slug, "")
                    if slug
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') =>
                {
                    send(tx, Command::AutoresearchStatus(Some(slug.to_owned())))
                }
                _ => self.use_skill("autoresearch".into(), arg.to_owned(), tx),
            },
            "/tasks" => send(tx, Command::ListTasks(self.project.clone())),
            "/roster" => send(tx, Command::ListRoster),
            "/compact" => self.compact_cmd(arg, tx),
            "/endless" => self.endless_cmd(arg, tx),
            "/shake" => self.shake_cmd(arg, tx),
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
            "/provider" => self.provider_cmd(arg, tx),
            "/secrets" => self.secrets_cmd(arg, tx),
            "/mesh" => self.mesh_cmd(arg, tx),
            "/link" => self.link_cmd(arg, tx),
            "/nodes" => self.nodes_cmd(arg, tx),
            // `/<skill> [args]` hands the skill to the model (04 §2.4); any other `/word` — a path, a typo —
            // is message text and takes the arm below.
            // A watch-only child takes neither (141 §10.1): the hint instead.
            _ if self.observed.is_some() => self.watch_swallow(),
            c if c.starts_with('/') && self.skill_word(&c[1..]) => {
                self.use_skill(c[1..].to_owned(), arg.to_owned(), tx);
            }
            _ => self.send_text(line, interject, tx),
        }
    }

    /// Send `line` as message text: a steer mid-turn (interjecting when
    /// asked), else the next turn's input. Also the raw path of a harness
    /// slash command picked from the popup (141 D15 c): no `use_skill`.
    pub(super) fn send_text(&mut self, line: &str, interject: bool, tx: &UnboundedSender<Command>) {
        // 153 §6.1: what went out, when a dictation contributed to it (fire-and-forget).
        if let Some(id) = self.dictated.take() {
            send(
                tx,
                Command::DictationCorrection {
                    id,
                    text: line.to_owned(),
                },
            );
        }
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
