use super::*;

impl Model {

    pub(super) fn apply_agent(&mut self, ev: AgentEvent) {
        if self.awaiting_session && matches!(ev, AgentEvent::TurnStarted { .. }) {
            self.session = ev.session();
            self.awaiting_session = false;
        }
        // The send was refused before any turn (a rejected attachment, no
        // actor): its error names a session we do not know yet, so it would
        // fall through the ownership check below. Held for `SessionCreated`.
        if self.awaiting_session
            && self.session.is_none()
            && let AgentEvent::Error {
                session,
                turn: None,
                message,
            } = &ev
        {
            self.send_error = Some((*session, message.clone()));
        }
        // The plan is session chrome, not a transcript row (47 §6.1): it
        // never finishes a streaming tail below. Children's plans show when
        // opened; a hidden session refreshes from `Loaded.todos` then.
        let ev = match ev {
            AgentEvent::TodoChanged { session, snapshot } => {
                if self.session == Some(session) {
                    self.set_todos(snapshot);
                }
                return;
            }
            // 82 §3: global; the draft header (no session open) follows it
            // unless the draft picked its own model (148 R1).
            AgentEvent::DefaultModelChanged { model } => {
                if self.session.is_none() && self.draft.as_ref().is_none_or(|d| d.model.is_none()) {
                    self.model_id = model.clone();
                }
                self.default_model = model;
                return;
            }
            // 127 §3: global; an open `/model` picker re-renders from it.
            AgentEvent::FavoriteModelsChanged { models } => {
                self.set_favorites(models);
                return;
            }
            // 83 §3: global; the draft header (no session open) follows it
            // unless the draft picked its own effort (148 R1).
            AgentEvent::DefaultEffortChanged { effort } => {
                if self.session.is_none() && self.draft.as_ref().is_none_or(|d| d.effort.is_none())
                {
                    self.effort = effort.clone();
                }
                self.default_effort = effort;
                return;
            }
            ev => ev,
        };
        // Roster bookkeeping for the subtree, independent of what is shown.
        match &ev {
            // 125 §7.5: the advisor is session chrome (status segment, header
            // tag), never a transcript row — it must not finish a streaming
            // tail below. `set_advisor_status` prints the one change line.
            AgentEvent::AdvisorChanged { session, status } => {
                if self.session == Some(*session) {
                    self.set_advisor_status(status.clone());
                }
                return;
            }
            // 125 §7.4: the card goes above a streaming tail; a steer's row
            // may also open a turn, whose `TurnStarted` must not echo it.
            AgentEvent::AdvisoryAdded {
                session,
                seq,
                advisory,
            } => {
                if self.session == Some(*session) && self.advisory_seqs.insert(*seq) {
                    self.interject(Entry::Advisory {
                        advisory: advisory.clone(),
                        expanded: false,
                    });
                }
                return;
            }
            // 125 §7.5: the hidden advisor child never reaches the board (the
            // event carries no `kind`; the link's id is the guard).
            AgentEvent::SubagentSpawned { child, .. } if self.advisor.advisor == Some(*child) => {
                return;
            }
            AgentEvent::SubagentSpawned {
                parent,
                child,
                name,
                cap,
            } => {
                let depth = if self.session == Some(*parent) {
                    Some(1)
                } else {
                    self.agents.get(parent).map(|p| p.info.depth + 1)
                };
                if let Some(depth) = depth {
                    self.agents.insert(
                        *child,
                        AgentRow::new(AgentInfo {
                            id: *child,
                            name: name.clone(),
                            parent: *parent,
                            depth,
                            agent: None,
                            model: String::new(),
                            state: AgentState::Running,
                            started_at: Some(chrono::Utc::now()),
                            finished_at: None,
                            last: String::new(),
                            project: None,
                            retention: agent::Retention::Durable,
                            activity: Default::default(),
                            cap: *cap,
                            kind: Default::default(),
                            observed: None,
                        }),
                    );
                }
                if let Some(p) = &mut self.private
                    && p.member(*parent)
                {
                    p.members.insert(*child);
                }
                if self.session == Some(*parent) {
                    self.interject(Entry::Info(format!("↳ spawned {name}")));
                }
                return;
            }
            // 141 §10.1: a watch-only child is a `◎` board row, never a
            // `↳ spawned` line. On the shown child itself only a state change
            // says anything (one `watching · <state>` line); detail-only
            // updates must not finish a streaming tail below.
            AgentEvent::ObservedChildChanged {
                parent,
                child,
                name,
                model,
                observed,
            } => {
                if self.session == Some(*child) {
                    let changed = self
                        .observed
                        .as_ref()
                        .is_none_or(|o| o.state != observed.state);
                    self.observed = Some(observed.clone());
                    if changed {
                        self.interject(Entry::Info(format!(
                            "watching · {}",
                            observed_word(observed.state)
                        )));
                    }
                    return;
                }
                let depth = if self.session == Some(*parent) {
                    Some(1)
                } else {
                    self.agents.get(parent).map(|p| p.info.depth + 1)
                };
                let Some(depth) = depth else { return };
                let state = observed_state(observed);
                let row = self.agents.entry(*child).or_insert_with(|| {
                    AgentRow::new(AgentInfo {
                        id: *child,
                        name: name.clone(),
                        parent: *parent,
                        depth,
                        agent: None,
                        model: String::new(),
                        state: AgentState::Idle,
                        started_at: None,
                        finished_at: None,
                        last: String::new(),
                        project: None,
                        retention: agent::Retention::Durable,
                        activity: Default::default(),
                        cap: None,
                        kind: SessionKind::Observed,
                        observed: None,
                    })
                });
                row.info.name.clone_from(name);
                if let Some(m) = model {
                    row.info.model.clone_from(m);
                }
                if row.info.state != state {
                    let now = Some(chrono::Utc::now());
                    if state == AgentState::Running {
                        row.info.started_at = row.info.started_at.or(now);
                    } else {
                        row.info.started_at = None;
                        row.info.finished_at = now;
                    }
                    row.info.state = state;
                }
                // Terminal-bad and lost rows linger on the board like a
                // cancelled native child (`fresh`); a finished one leaves it.
                row.fresh = matches!(
                    observed.state,
                    ObservedState::Failed | ObservedState::Cancelled | ObservedState::Unknown
                );
                row.tool = None;
                row.info.observed = Some(observed.clone());
                return;
            }
            AgentEvent::MessageDelivered {
                session,
                from,
                text,
            } => {
                if self.session == Some(*session) {
                    let kind = if from.starts_with("job ") {
                        CardKind::Job
                    } else {
                        CardKind::Peer
                    };
                    // The typed `PeerDelivered` already carded this body (50 §10).
                    if kind == CardKind::Peer
                        && let Some(at) = self.peer_shown.iter().position(|t| t == text)
                    {
                        self.peer_shown.remove(at);
                        return;
                    }
                    self.interject(card(kind, from.clone(), text));
                }
                return;
            }
            // Ownership moved (68 §4.2): the picker's chip follows; the open
            // session gets one line. Nothing streams, so finish nothing below.
            AgentEvent::SessionOwnerChanged {
                session,
                owner,
                epoch,
                here,
                ..
            } => {
                if let Some(Menu::Sessions { items, .. }) = &mut self.menu {
                    for row in items.iter_mut() {
                        if let Grouped::Item((s, _)) = row
                            && s.id == *session
                        {
                            s.owner = owner.clone();
                            s.epoch = *epoch;
                            s.here = *here;
                        }
                    }
                }
                if self.session == Some(*session) {
                    self.entries.push(Entry::Info(if *here {
                        format!("session continues here (epoch {epoch})")
                    } else {
                        format!(
                            "session now owned by {} (epoch {epoch})",
                            engine_chip(owner)
                        )
                    }));
                }
                return;
            }
            // Queue bookkeeping mid-stream must not finish the streaming tail below.
            AgentEvent::InputQueued {
                session, id, text, ..
            } => {
                if self.session == Some(*session) && !self.queued.iter().any(|q| q.id == *id) {
                    self.queued.push(QueuedInput {
                        id: *id,
                        text: text.clone(),
                        channel: None,
                        files: Vec::new(),
                    });
                }
                return;
            }
            AgentEvent::InputWithdrawn { session, id } => {
                if self.session == Some(*session) {
                    self.queued.retain(|q| q.id != *id);
                }
                return;
            }
            // The sidebar of another frontend; nothing on screen changes and
            // a mid-stream arrival must not finish the streaming tail below.
            AgentEvent::TitleChanged { .. } | AgentEvent::SessionDeleted { .. } => {
                self.search_invalidate(SearchCategory::Sessions);
                return;
            }
            // Another client (or our own removal, whose reload is already on
            // its way) changed what the model sees. An edit or rewind reloads
            // the transcript once settled; the settled handback after a turn
            // only clears a stale mark, the live stream already showed it.
            AgentEvent::ContextChanged {
                session,
                revision: rev,
                reason,
            } => {
                if self.session == Some(*session) {
                    let newer = revision(rev) > revision(&self.context_revision);
                    self.refresh_context |= (newer && reason != "settled") || self.context_stale;
                }
                return;
            }
            // Root metadata moved (51 §3): an open grouped picker re-lists;
            // the shown tree's label follows. Session chrome, never a transcript row.
            AgentEvent::SessionMetadataChanged { session, project } => {
                if self.session == Some(*session) || self.session_parent == Some(*session) {
                    self.project = project.clone();
                }
                self.refresh_grouping |= matches!(
                    self.menu,
                    Some(Menu::Sessions { .. } | Menu::Artifacts { .. })
                );
                return;
            }
            // Non-transcript chrome must never finish a streaming assistant tail.
            AgentEvent::ModelUsageChanged { .. } => return,
            // The owned private tree is closing (a leave from here already
            // dropped `private`; this is the harness's own close): let go of
            // everything it held.
            AgentEvent::SessionClosing { session, .. } => {
                if self.private.as_ref().is_some_and(|p| p.root == *session) {
                    let shown = self
                        .session
                        .is_some_and(|s| self.private.as_ref().is_some_and(|p| p.member(s)));
                    self.private = None;
                    if shown {
                        self.reset();
                        self.entries
                            .push(Entry::Info("private session closed by the harness".into()));
                    }
                }
                return;
            }
            AgentEvent::TurnStarted { session, .. } if self.session == Some(*session) => {
                self.streaming = true;
            }
            AgentEvent::TurnFinished { session, .. } => {
                self.search_invalidate(SearchCategory::Sessions);
                if self.session == Some(*session) {
                    self.streaming = false;
                } else {
                    self.track(&ev);
                }
            }
            _ => self.track(&ev),
        }
        // A subagent's stream is not rendered here; open it to read it. An
        // inspected page (49 §7) is not the live tail: arriving events must
        // not stitch across the missing middle or move the match offscreen.
        if ev.session().is_none_or(|s| self.session != Some(s)) || self.inspect.is_some() {
            return;
        }
        if !matches!(ev, AgentEvent::ReasoningDelta { .. })
            && let Some(Entry::Reasoning { done, .. }) = self.entries.last_mut()
        {
            *done = true;
        }
        if !matches!(ev, AgentEvent::TextDelta { .. })
            && let Some(Entry::Assistant(t)) = self.entries.last_mut()
        {
            t.finish();
        }
        match ev {
            // Echo the turn's input unless the human just typed it here, it was
            // a steer whose card moved up on `InputInjected`, or the harness
            // injected it (`MessageDelivered` already shows those). The card
            // that opens the turn carries `turn_seq`: the `/edit` handle.
            AgentEvent::TurnStarted {
                turn,
                turn_seq,
                text,
                model,
                ..
            } => {
                self.model_id = model;
                self.turn_seq = Some(turn_seq);
                // 125 §7.4: a steer's row is already its card (by seq, or by
                // the `<advisory>` wrapper when the card has not landed yet).
                // 183 §11.6: a contact's row is the `PeerDelivered` card.
                if self.advisory_seqs.contains(&turn_seq)
                    || injected(&text).is_some()
                    || contact_row(&text).is_some()
                {
                    return;
                }
                // 171F B4: parked cell results ride in front of the words typed here.
                let (cells, said) = cell_results(&text);
                let shown = self.last_injected_turn.take() == Some(turn)
                    || self.sent.take().as_deref() == Some(said);
                if !shown {
                    self.entries.extend(user_entries(text, Some(turn_seq)));
                } else if let Some(at) = self
                    .entries
                    .iter()
                    .rposition(|e| matches!(e, Entry::User { turn: None, .. }))
                {
                    if let Entry::User { turn, .. } = &mut self.entries[at] {
                        *turn = Some(turn_seq);
                    }
                    self.entries
                        .splice(at..at, cells.into_iter().map(Entry::Info));
                }
            }
            // The steer enters the context now: its card moves from the queue
            // to the transcript, after everything the model said before it.
            AgentEvent::InputInjected { id, turn, .. } => {
                if let Some(at) = self.queued.iter().position(|q| q.id == id) {
                    self.entries
                        .extend(user_entries(self.queued.remove(at).text, None));
                    self.last_injected_turn = Some(turn);
                }
            }
            // `InputQueued` is handled above, before the streaming-tail bookkeeping.
            AgentEvent::InputQueued { .. } => {}
            AgentEvent::TextDelta { text, .. } => match self.entries.last_mut() {
                Some(Entry::Assistant(t)) if !t.is_done() => t.push(&text),
                _ => {
                    let mut t = markdown::Text::with(self.assistant_options());
                    t.push(&text);
                    self.entries.push(Entry::Assistant(Box::new(t)));
                }
            },
            AgentEvent::ReasoningDelta { text, .. } => match self.entries.last_mut() {
                Some(Entry::Reasoning {
                    text: s,
                    done: false,
                }) => s.push_str(&text),
                _ => self.entries.push(Entry::Reasoning {
                    text: text.to_string(),
                    done: false,
                }),
            },
            AgentEvent::ToolCallPending { call_id, name, .. } => {
                self.entries.push(Entry::Pending {
                    call_id,
                    name,
                    progress: None,
                });
            }
            AgentEvent::ToolCallProgress { call_id, args, .. } => {
                if let Some(Entry::Pending { progress, .. }) = self
                    .entries
                    .iter_mut()
                    .find(|e| matches!(e, Entry::Pending { call_id: c, .. } if *c == call_id))
                {
                    *progress = Some(Progress {
                        show: surface::partial_show(&args),
                        bytes: args.len(),
                    });
                }
            }
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } => {
                // A `.` in the id marks a call made from inside a `python` cell
                // (design doc 08 §4.1); it lands right after its parent card.
                let inner = call_id.contains('.');
                let tool = Entry::Tool {
                    call_id: call_id.clone(),
                    args: describe(&name, &args),
                    name: if inner { format!("↳ {name}") } else { name },
                    result: None,
                    expanded: None,
                    artifact: None,
                };
                match self
                    .entries
                    .iter_mut()
                    .find(|e| matches!(e, Entry::Pending { call_id: c, .. } if *c == call_id))
                {
                    Some(slot) => *slot = tool,
                    None => self.entries.push(tool),
                }
            }
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } => {
                match self.entries.iter_mut().rev().find(
                    |e| matches!(e, Entry::Tool { call_id: c, result: None, .. } if *c == call_id),
                ) {
                    Some(Entry::Tool { result, .. }) => {
                        *result = Some((output, is_error));
                    }
                    _ => self.entries.push(Entry::Info(format!(
                        "{} {}",
                        if is_error { "✗" } else { "✓" },
                        result_line(&output)
                    ))),
                }
            }
            AgentEvent::TurnFinished { usage, context, .. } => {
                // A call the model abandoned mid-stream never starts.
                self.entries.retain(|e| !matches!(e, Entry::Pending { .. }));
                self.usage = Some(usage);
                self.context = context;
                self.context_estimated = false;
                self.compacting = false;
                self.vision = None;
                self.resume_note = None;
            }
            // The gauge moves per round, with the cost (51 §6.4).
            AgentEvent::ContextUsed { context, size, .. } => {
                self.context = context;
                self.context_estimated = false;
                if let Some(s) = size {
                    self.context_size = Some(s);
                }
            }
            AgentEvent::ModelChanged { model, .. } => {
                self.entries.push(Entry::Info(format!("model → {model}")));
                self.model_id = model;
            }
            // The harness's own model (06 §8.2): announced only when it
            // switches away from one already shown, not at connect.
            AgentEvent::HarnessModelChanged { model, .. } => {
                if self.harness_model.as_ref().is_some_and(|m| *m != model) {
                    self.entries
                        .push(Entry::Info(format!("harness model → {model}")));
                }
                self.harness_model = Some(model);
            }
            // The harness's manifest (75 §3): the first report is silent, a
            // change in size afterwards is one line.
            AgentEvent::HarnessCommandsChanged { commands, .. } => {
                if !self.harness_commands.is_empty()
                    && self.harness_commands.len() != commands.len()
                {
                    self.entries.push(Entry::Info(format!(
                        "harness commands → {}",
                        commands.len()
                    )));
                }
                self.harness_commands = commands;
            }
            // Our own `SetEffort` is announced by its reply; anything else
            // (a `/model` switch dropping a level the new model lacks) here.
            AgentEvent::EffortChanged { effort, .. } => {
                if !self.awaiting_effort && self.effort != effort {
                    self.entries.push(Entry::Info(match &effort {
                        Some(e) => format!("effort → {e}"),
                        None => format!("effort reset: not offered by {}", self.model_id),
                    }));
                }
                self.effort = effort;
            }
            AgentEvent::Compacting { .. } => self.compacting = true,
            AgentEvent::Compacted {
                epoch,
                style,
                after,
                ..
            } => {
                self.compacting = false;
                self.context = after;
                self.context_estimated = true;
                if style == CompactStyle::Clear {
                    // A clean slate on screen too; `/resume` still shows everything.
                    // Compaction only runs between turns, so nothing is streaming.
                    self.entries.clear();
                    self.anchor = None;
                    self.entries
                        .push(Entry::Info(format!("context cleared (epoch {epoch})")));
                } else {
                    self.interject(Entry::Compacted {
                        epoch,
                        style,
                        summary: None,
                        expanded: false,
                    });
                }
            }
            AgentEvent::Error { message, .. } => {
                self.compacting = false;
                self.vision = None;
                self.entries.push(Entry::Error(message));
            }
            // 141 §10.1: handled in the roster match above (it returns before
            // the tail-finishing rules, which a detail-only update must skip).
            AgentEvent::ObservedChildChanged { .. } => {}
            // 91: the thinking block is already closed by the generic rule above
            // (any non-delta event); the TUI shows no thinking time.
            AgentEvent::ReasoningDone { .. }
            | AgentEvent::SubagentSpawned { .. }
            | AgentEvent::MessageDelivered { .. }
            | AgentEvent::InputWithdrawn { .. }
            | AgentEvent::TitleChanged { .. }
            | AgentEvent::SessionDeleted { .. }
            | AgentEvent::ContextChanged { .. }
            | AgentEvent::TodoChanged { .. }
            | AgentEvent::DefaultModelChanged { .. }
            | AgentEvent::DefaultEffortChanged { .. }
            | AgentEvent::FavoriteModelsChanged { .. } => {}
            AgentEvent::Harvesting { .. } => self.harvesting = true,
            AgentEvent::Harvested { added, updated, .. } => {
                self.harvesting = false;
                self.interject(Entry::Info(format!("knowledge +{added} ~{updated}")))
            }
            AgentEvent::SkillsChanged { skills, .. } => self.loaded_skills = skills,
            // The consultation's steps in the status line; the settled note
            // is one collapsed card naming the sources (52 §6). Its body text
            // arrives with the reloaded transcript, not on the wire.
            AgentEvent::VisionProgress {
                stage,
                model,
                sources,
                ..
            } => {
                let line = vision_line(stage, &model, sources.len());
                if matches!(stage, VisionStage::Questioning | VisionStage::Inspecting) {
                    self.vision = Some(line);
                } else {
                    self.vision = None;
                    let body = sources
                        .iter()
                        .map(|s| format!("{} · {}", s.label, s.reference()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.interject(card(CardKind::Vision, line, &body));
                }
            }
            AgentEvent::SessionMetadataChanged { .. }
            | AgentEvent::ModelUsageChanged { .. }
            | AgentEvent::SessionOwnerChanged { .. }
            | AgentEvent::SessionClosing { .. }
            | AgentEvent::PartialDropped { .. } => {}
            // 139 §9.3 (D5): a system takeover left the turn open here, not
            // resumed; the engine's local note (W2) carries the Resume hint.
            AgentEvent::ResumePending { from, .. } => {
                let chip = engine_chip(&from);
                self.interject(Entry::Divider(format!("taken from {chip}")));
                self.resume_note = Some(format!(
                    "taken from {chip} while a turn was running — not resumed"
                ));
            }
            // 125: handled before the streaming-tail bookkeeping above.
            // 145 §0.3: parked harness inputs are the talk's business; no transcript row.
            AgentEvent::AdvisorChanged { .. }
            | AgentEvent::AdvisoryAdded { .. }
            | AgentEvent::InputsParked { .. } => {}
            // 109 §10.3 / 139 §9.3: the divider and the status-line note, with
            // the previous owner after a transfer / takeover.
            AgentEvent::TurnResumed {
                kind,
                interrupted_at,
                cause,
                from,
                ..
            } => {
                let at = interrupted_at.with_timezone(&chrono::Local).format("%H:%M");
                let (label, note) = match (cause, from) {
                    (ResumeCause::Transfer | ResumeCause::Takeover, from) => {
                        let chip = engine_chip(from.as_deref().unwrap_or_default());
                        let what = if cause == ResumeCause::Takeover {
                            "takeover"
                        } else {
                            "transfer"
                        };
                        (
                            format!("resumed after {what} from {chip}"),
                            format!("taken from {chip} at {at} — resumed"),
                        )
                    }
                    (_, _) => match kind {
                        ResumeKind::Interrupted => (
                            "resumed after restart".to_owned(),
                            format!("interrupted by a restart at {at} — resumed"),
                        ),
                        ResumeKind::Suspended => (
                            "resumed after update".to_owned(),
                            format!("paused for an update at {at} — resumed"),
                        ),
                    },
                };
                self.interject(Entry::Divider(label));
                self.resume_note = Some(note);
            }
            AgentEvent::ResumeReport {
                replayed,
                interrupted,
                lost_jobs,
                ..
            } => {
                let mut note = format!("resumed · {} replayed", replayed.len());
                if !interrupted.is_empty() {
                    let names: Vec<&str> = interrupted.iter().map(|(_, n)| n.as_str()).collect();
                    note.push_str(&format!(
                        " · {} skipped: {} — effect unknown",
                        interrupted.len(),
                        names.join(", ")
                    ));
                }
                if !lost_jobs.is_empty() {
                    note.push_str(&format!(" · {} job(s) lost", lost_jobs.len()));
                }
                if !interrupted.is_empty() || !lost_jobs.is_empty() {
                    self.interject(Entry::Info(note.clone()));
                }
                self.resume_note = Some(note);
            }
            AgentEvent::TurnSuspended { reason, .. } => {
                self.interject(Entry::Divider(format!("suspended for {reason}")));
            }
        }
    }

    /// Fold an event of a subtree session into its row (design doc 19 §3.3).
    /// A direct child's error or cancel also interjects one line here: they
    /// must not be silent, and the board has everything else.
    fn track(&mut self, ev: &AgentEvent) {
        let Some(row) = ev.session().and_then(|s| self.agents.get_mut(&s)) else {
            return;
        };
        let direct = self.session == Some(row.info.parent);
        let mut note = None;
        match ev {
            AgentEvent::TurnStarted { .. } => {
                row.info.state = AgentState::Running;
                row.info.started_at = Some(chrono::Utc::now());
                row.tool = None;
                row.fresh = false;
            }
            AgentEvent::ToolCallPending { name, .. } => {
                row.tool = Some(format!("preparing {name}"));
            }
            AgentEvent::ToolCallStarted { name, args, .. } => {
                if name == "send_message" && args["await_reply"] == true {
                    row.info.state = AgentState::Awaiting {
                        from: args["to"].as_str().unwrap_or("?").to_owned(),
                    };
                    row.question = args["message"]
                        .as_str()
                        .and_then(|m| m.lines().next())
                        .map(str::to_owned);
                }
                row.tool = Some(match first_arg(args) {
                    Some(arg) => format!("running {name} {arg}"),
                    None => format!("running {name}"),
                });
            }
            AgentEvent::ToolCallFinished { .. } => {
                row.tool = None;
                if matches!(row.info.state, AgentState::Awaiting { .. }) {
                    row.info.state = AgentState::Running;
                }
            }
            AgentEvent::TurnFinished { stop_reason, .. } => {
                row.info.state = match stop_reason.as_str() {
                    "error" => AgentState::Error,
                    "cancelled" => AgentState::Cancelled,
                    _ => AgentState::Idle,
                };
                row.fresh = row.info.state != AgentState::Idle;
                row.info.started_at = None;
                row.info.finished_at = Some(chrono::Utc::now());
                row.tool = None;
                if direct && stop_reason == "cancelled" {
                    note = Some(Entry::Info(format!("↳ {} cancelled", row.info.name)));
                }
            }
            AgentEvent::Error { message, .. } => {
                row.error = Some(message.clone());
                if direct {
                    note = Some(Entry::Error(format!(
                        "↳ {} error: {message}",
                        row.info.name
                    )));
                }
            }
            _ => {}
        }
        if let Some(e) = note {
            self.interject(e);
        }
    }
}

impl Model {

    /// `/agents`: Enter opens the highlighted subagent — a session switch,
    /// the same path as `/sessions`; the one left keeps running.
    pub(super) fn agents_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Agents { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Enter => {
                let picked = items.get(*cursor).map(|a| a.info.id);
                self.menu = None;
                if let Some(id) = picked {
                    self.resume(id, tx);
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

/// One row of the `/agents` picker and the board: the supervisor's snapshot
/// plus what the TUI learned from this session's events since (design doc
/// 19 §3.3).
#[derive(Clone, Debug)]
pub struct AgentRow {
    /// The snapshot (`list_agents`) as kept live by `TurnStarted` / `TurnFinished` and friends.
    pub info: AgentInfo,
    /// `preparing <tool>` / `running <tool> <arg>` while a call is in flight; the
    /// tool and its args while an approval is pending. `None` = streaming text.
    pub tool: Option<String>,
    /// First line of the question while `Awaiting`.
    pub question: Option<String>,
    /// Last `Error` message; the detail of an `Error` row.
    pub error: Option<String>,
    /// The turn ended in error / cancelled while this TUI watched and the
    /// picker has not been opened since: the row stays on the board.
    pub fresh: bool,
}

impl AgentRow {
    /// A row with nothing learned yet.
    pub fn new(info: AgentInfo) -> Self {
        Self {
            info,
            tool: None,
            question: None,
            error: None,
            fresh: false,
        }
    }

    /// The agent is in a turn: running, asking, or waiting on an approval.
    pub fn live(&self) -> bool {
        matches!(
            self.info.state,
            AgentState::Running | AgentState::Awaiting { .. } | AgentState::Approval { .. }
        )
    }

    /// Shown on the board: live, or an unacknowledged error / cancel.
    pub fn on_board(&self) -> bool {
        self.live() || self.fresh
    }
}

/// Picker / board order (design doc 19 §3.1): running first in spawn order,
/// then awaiting / approval, then the rest newest-finished first. The id
/// (UUIDv7, time-ordered) breaks ties so a `HashMap`-fed board never jitters.
pub(super) fn rank(a: &AgentRow) -> (u8, i64, u128) {
    let ms = |t: Option<chrono::DateTime<chrono::Utc>>| t.map_or(0, |t| t.timestamp_millis());
    let id = a.info.id.0.as_u128();
    match a.info.state {
        AgentState::Running => (0, ms(a.info.started_at), id),
        AgentState::Awaiting { .. } | AgentState::Approval { .. } => (1, ms(a.info.started_at), id),
        _ => (2, -ms(a.info.finished_at), id),
    }
}

/// `/agents` picker order; see [`rank`].
pub fn sort_agents(mut list: Vec<AgentRow>) -> Vec<AgentRow> {
    list.sort_by_key(rank);
    list
}

/// One `/agents` picker or board row: `glyph name state detail age [jN pN ·age]`
/// (design doc 19 §3.1, 62 §5.1). `spinner` is the running glyph; depth ≥ 2
/// indents the name under its parent; `width` (terminal columns) bounds the
/// detail column. Age is elapsed while the agent is live, `ago` since it
/// finished otherwise; the trailing counts and `last_active` age come from
/// the snapshot's `activity` and appear only when known.
pub fn agent_row(a: &AgentRow, spinner: &str, width: u16) -> String {
    // 141 §10.1: a watch-only child is `◎ name <observed state> watch · <src> [lastIntent]`.
    let (glyph, word) = match &a.info.observed {
        Some(o) => (WATCH_GLYPH, observed_word(o.state)),
        None => (
            crate::components::broadcast::state::glyph(&a.info.state, spinner),
            a.info.state.word(),
        ),
    };
    let name = match a.info.depth {
        0 | 1 => a.info.name.clone(),
        d => format!("{}↳ {}", " ".repeat(2 * usize::from(d - 1)), a.info.name),
    };
    let now = chrono::Utc::now();
    let age = match (a.info.started_at, a.info.finished_at) {
        (Some(t), _) if a.live() => elapsed(now - t),
        (_, Some(t)) => ago(now - t),
        _ => String::new(),
    };
    let active = match crate::components::broadcast::state::age(a.info.activity.last_active) {
        s if s.is_empty() => s,
        s => format!(" ·{s}"),
    };
    let tail = format!(
        "  {age}{}{active}",
        crate::components::broadcast::state::counts(&a.info.activity)
    );
    // Glyph, name, state and their gaps take 27 columns, the picker frame 7; the detail gets
    // what is left after the measured tail (141 R4 T13b: an estimate let long ages overflow).
    let room = usize::from(width)
        .saturating_sub(34 + crate::search::cells(&tail))
        .max(16);
    let detail = match (&a.info.observed, &a.info.state) {
        (Some(o), _) => {
            let mut d = match o.detail["lastIntent"].as_str() {
                Some(intent) => format!("watch · {} · {intent}", source_tag(o.source)),
                None => format!("watch · {}", source_tag(o.source)),
            };
            // 141 R3 T13: the child's model when the column has room for it.
            let m = &a.info.model;
            if !m.is_empty() && d.chars().count() + 3 + m.chars().count() <= room {
                d = format!("{d} · {m}");
            }
            d
        }
        (None, AgentState::Running) => a.tool.clone().unwrap_or_else(|| "streaming".to_owned()),
        (None, AgentState::Awaiting { from }) => match &a.question {
            Some(q) => format!("{from}: \"{q}\""),
            None => from.clone(),
        },
        (None, AgentState::Approval { tool }) => a.tool.clone().unwrap_or_else(|| tool.clone()),
        (None, AgentState::Error) => a.error.clone().unwrap_or_else(|| a.info.last.clone()),
        (None, AgentState::Cancelled | AgentState::Idle) => a.info.last.clone(),
    };
    let detail = crate::search::fit(&detail, room);
    format!("{glyph} {name:<14} {word:<9} {detail:<room$}{tail}")
}

/// Row glyph of a watch-only child (141 §10.1).
pub const WATCH_GLYPH: &str = "◎";

/// The source tag of a watch-only child (141 §3 UI tags).
pub fn source_tag(source: ObservedSource) -> &'static str {
    match source {
        ObservedSource::Rfd => "ACP",
        ObservedSource::OpencodeExt | ObservedSource::OpencodeTask => "opencode",
        ObservedSource::OmpTask => "omp task",
    }
}

/// The harness that runs a watch-only child, for the read-only hint.
pub(super) fn harness_word(source: ObservedSource) -> &'static str {
    match source {
        ObservedSource::Rfd => "the harness",
        ObservedSource::OpencodeExt | ObservedSource::OpencodeTask => "opencode",
        ObservedSource::OmpTask => "omp",
    }
}

/// A watch-only state as the row/status word (141 §5.2 labels; Q1: idle is not done).
pub fn observed_word(state: ObservedState) -> &'static str {
    match state {
        ObservedState::Pending => "pending",
        ObservedState::Running => "running",
        ObservedState::RequiresAction => "waiting on you",
        ObservedState::Idle => "idle",
        ObservedState::Completed => "done",
        ObservedState::Failed => "failed",
        ObservedState::Cancelled => "stopped",
        ObservedState::Unknown => "lost contact",
    }
}

/// The board state a watch-only state maps to (141 §5.2 `state_of` column).
fn observed_state(o: &ObservedMeta) -> AgentState {
    match o.state {
        ObservedState::Running => AgentState::Running,
        ObservedState::RequiresAction => AgentState::Approval {
            tool: o.detail["currentTool"]
                .as_str()
                .unwrap_or("permission")
                .to_owned(),
        },
        ObservedState::Failed => AgentState::Error,
        ObservedState::Cancelled => AgentState::Cancelled,
        ObservedState::Pending
        | ObservedState::Idle
        | ObservedState::Completed
        | ObservedState::Unknown => AgentState::Idle,
    }
}
