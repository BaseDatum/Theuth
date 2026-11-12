use super::*;
use crate::components::artifacts_apps::artifact_text;

impl Model {

    /// The input box takes keys: no approval prompt, no modal menu (the
    /// slash popup rides on the box) and the todo card not focused (F2). A
    /// shown `secret_request` types into the box (masked).
    pub fn input_focused(&self) -> bool {
        if self.approval().is_some()
            || (self.components.focused() && self.components.editor().is_none())
            || self.dictation.is_some()
            || self.suggestion.is_some()
            || self.talk_view
        {
            return false;
        }
        if self.components.editor().is_some_and(|(class, _)| class == crate::components::FocusClass::Secret) {
            return true;
        }
        if let Some(a) = self.ask() {
            return a.typing;
        }
        // 141 §10.1: a watch-only child takes no input (the status line says
        // why) — except the one slash command `/` opened the editor for.
        (self.observed.is_none() || self.watch_slash)
            && !self.todo_focus
            && !self.modal_menu()
            && self.inspect.is_none()
    }

    /// 141 §10.1: the read-only reason of a watch-only view, for the status
    /// line and as the one line typing gets; `None` for anything else.
    pub fn watch_hint(&self) -> Option<String> {
        let o = self.observed.as_ref()?;
        let stop = if o.can_cancel {
            "Esc stops it"
        } else {
            "cannot be stopped here"
        };
        Some(format!(
            "watch-only — {} runs this subagent · {stop}",
            harness_word(o.source)
        ))
    }

    /// A key the watch-only view swallows: the hint, once per run of keys.
    pub fn watch_swallow(&mut self) {
        if let Some(hint) = self.watch_hint()
            && !matches!(self.entries.last(), Some(Entry::Info(t)) if *t == hint)
        {
            self.entries.push(Entry::Info(hint));
        }
    }

    /// A menu that takes the keys is open: every menu but the slash popup.
    pub fn modal_menu(&self) -> bool {
        self.components.modal() || !matches!(self.menu, None | Some(Menu::Slash { .. }))
    }

    /// The `/search` menu is up (49 §6).
    pub fn search_open(&self) -> bool {
        matches!(self.menu, Some(Menu::Search | Menu::Attach))
    }

    /// The `/attach` menu is up (66 §10): Enter attaches instead of opening.
    pub fn attach_open(&self) -> bool {
        matches!(self.menu, Some(Menu::Attach))
    }
}

impl Model {

    /// `/search [query]` (49 §6): open the live menu, a fresh view with the
    /// remembered chips; a prefilled query runs at once. Any preview or
    /// inspection under way is left.
    pub fn open_search(&mut self, query: &str, tx: &UnboundedSender<Command>) {
        self.inspect = None;
        self.menu = Some(Menu::Search);
        for r in self.search.open(query) {
            send(tx, Command::Search(r));
        }
    }

    /// `/attach [query]` (66 §10): the search menu in pick mode — Enter on a
    /// row holds it as a [`Paste::Ref`] and puts its token in the input box.
    pub fn open_attach(&mut self, query: &str, tx: &UnboundedSender<Command>) {
        self.inspect = None;
        self.menu = Some(Menu::Attach);
        for r in self.search.open(query) {
            send(tx, Command::Search(r));
        }
    }

    /// Hold `r` for the next submit; returns its [`paste_token`]. Attaching
    /// the same kind+id twice is a no-op that returns the existing token.
    pub fn attach_ref(&mut self, r: agent::attach::AttachRef) -> String {
        if let Some((i, p)) = self
            .pastes
            .iter()
            .enumerate()
            .find(|(_, p)| matches!(p, Paste::Ref(x) if x.kind == r.kind && x.id == r.id))
        {
            return paste_token(i, p);
        }
        let p = Paste::Ref(r);
        let token = paste_token(self.pastes.len(), &p);
        self.pastes.push(p);
        token
    }

    /// The search editor changed (49 §6.1): the debounce starts.
    pub fn search_edit(&mut self, text: String) {
        if self.search_open() {
            self.search.edit(text, now_ms());
        }
    }

    /// When the event loop must next wake for search (unix ms); `None` idle.
    pub fn search_wake(&self) -> Option<u64> {
        self.search_open().then(|| self.search.wake()).flatten()
    }

    /// Send whatever search request is due at `now`, and anything a reply
    /// queued meanwhile.
    pub fn search_tick(&mut self, now: u64, tx: &UnboundedSender<Command>) {
        self.search_flush(tx);
        if !self.search_open() {
            return;
        }
        for r in self.search.tick(now) {
            send(tx, Command::Search(r));
        }
    }

    /// A source changed: an open search re-asks, a closed one on restore.
    pub(super) fn search_invalidate(&mut self, category: SearchCategory) {
        let open = self.search_open();
        self.search.invalidate(category, open, now_ms());
    }

    /// A tagged search reply; stale generations are inert inside.
    pub(super) fn search_reply(&mut self, generation: u64, reply: search::Reply) {
        if !self.search_open() {
            return;
        }
        match self.search.reply(generation, reply, now_ms()) {
            search::Effect::None => {}
            search::Effect::Send(reqs) => {
                // Only the refresh reply sends from here; it needs the sender,
                // which `apply` lacks. Queue through the deadline instead.
                self.search_queue(reqs);
            }
            search::Effect::Artifact(load) => {
                let opts = self.assistant_options();
                let body = artifact_text(
                    &load.artifact,
                    load.bytes.as_deref().map_err(String::as_str),
                    &opts,
                );
                self.search.previews.push(search::Preview::Artifact {
                    artifact: Box::new(load.artifact),
                    body: Box::new(body),
                    matched: load.matched,
                    panel: false,
                    scroll: 0,
                });
            }
            search::Effect::Inspect {
                session,
                seq,
                loaded,
            } => self.inspect_page(session, seq, loaded),
        }
    }

    /// Requests produced while no sender was at hand: held until the next
    /// [`Model::search_tick`] / key.
    fn search_queue(&mut self, reqs: Vec<search::Request>) {
        self.search_outbox.extend(reqs);
    }

    /// Flush [`Model::search_queue`]'d requests.
    pub fn search_flush(&mut self, tx: &UnboundedSender<Command>) {
        for r in std::mem::take(&mut self.search_outbox) {
            send(tx, Command::Search(r));
        }
    }

    /// Keys while the `/search` menu is up (49 §6.1). `area` is the rows the
    /// result list gets and `width` the box's inner columns, for paging and
    /// preview clamping. Query-focus editing keys are the search editor's
    /// (its own `TextInput`); this sees the rest.
    pub fn search_key(
        &mut self,
        code: KeyCode,
        width: usize,
        area: usize,
        tx: &UnboundedSender<Command>,
    ) {
        use crate::search::{Act, FOOTER, Focus};
        let compact = area < 4;
        // A preview owns the keys while up; Esc pops it.
        if let Some(act) = self.search.preview_key(code, width, area) {
            match code {
                KeyCode::Char('o') => {
                    if let Some(a) = self.search.preview_artifact() {
                        send(tx, Command::OpenPath(a.path.clone()));
                    }
                }
                KeyCode::Char('e') => {
                    if let Some(a) = self.search.preview_artifact() {
                        send(
                            tx,
                            Command::ExportArtifact {
                                id: a.id.clone(),
                                name: a.name.clone(),
                                // A search hit is durable: a private tree is never indexed.
                                session: None,
                            },
                        );
                    }
                }
                _ => {}
            }
            self.search_act(act, tx);
            return;
        }
        let focus = self.search.focus;
        let act = match (code, focus) {
            (KeyCode::Esc, _) => {
                self.menu = None;
                self.search.close();
                return;
            }
            (KeyCode::Tab, Focus::Query) => {
                self.search.focus = Focus::Filters(0);
                Act::None
            }
            (KeyCode::Tab, Focus::Filters(_)) => {
                self.search.focus = Focus::Results;
                Act::None
            }
            (KeyCode::Tab, Focus::Results) => {
                self.search.focus = Focus::Footer(0);
                Act::None
            }
            (KeyCode::Tab, Focus::Footer(_)) => {
                self.search.focus = Focus::Query;
                Act::None
            }
            (KeyCode::BackTab, Focus::Query) => {
                self.search.focus = Focus::Footer(0);
                Act::None
            }
            (KeyCode::BackTab, Focus::Filters(_)) => {
                self.search.focus = Focus::Query;
                Act::None
            }
            (KeyCode::BackTab, Focus::Results) => {
                self.search.focus = Focus::Filters(0);
                Act::None
            }
            (KeyCode::BackTab, Focus::Footer(_)) => {
                self.search.focus = Focus::Results;
                Act::None
            }
            (KeyCode::Up, _) => {
                self.search.move_cursor(-1);
                Act::None
            }
            (KeyCode::Down, _) => {
                self.search.move_cursor(1);
                Act::None
            }
            (KeyCode::PageUp, _) => {
                self.search.move_cursor(-(area.max(1) as isize));
                Act::None
            }
            (KeyCode::PageDown, _) => {
                self.search.move_cursor(area.max(1) as isize);
                Act::None
            }
            (KeyCode::Left, Focus::Filters(i)) => {
                self.search.focus = Focus::Filters(i.saturating_sub(1));
                Act::None
            }
            (KeyCode::Right, Focus::Filters(i)) => {
                self.search.focus = Focus::Filters((i + 1).min(crate::search::N));
                Act::None
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Filters(i)) => {
                Act::Send(self.search.toggle(i))
            }
            (KeyCode::Left, Focus::Footer(i)) => {
                self.search.focus = Focus::Footer(i.saturating_sub(1));
                Act::None
            }
            (KeyCode::Right, Focus::Footer(i)) => {
                self.search.focus = Focus::Footer((i + 1).min(FOOTER.len() - 1));
                Act::None
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Footer(0)) => {
                Act::Send(self.search.refresh())
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Footer(_)) => {
                self.search.previews.push(search::Preview::Help);
                Act::None
            }
            (KeyCode::Enter, Focus::Query | Focus::Results) if self.attach_open() => {
                match self.search.pick() {
                    Some(r) => {
                        self.attach_token = Some(self.attach_ref(r));
                        self.menu = None;
                        self.search.close();
                        Act::None
                    }
                    // A `show more` row or nothing selectable: the ordinary key.
                    None => self.search.enter(),
                }
            }
            (KeyCode::Enter, Focus::Query | Focus::Results) => self.search.enter(),
            _ => Act::None,
        };
        self.search.ensure_visible(area, compact);
        self.search_act(act, tx);
    }

    /// Carry out what a search key asked.
    fn search_act(&mut self, act: search::Act, tx: &UnboundedSender<Command>) {
        self.search_flush(tx);
        match act {
            search::Act::None => {}
            search::Act::Send(reqs) => {
                for r in reqs {
                    send(tx, Command::Search(r));
                }
            }
            // A title hit or a recent row: an ordinary session switch (49 §7).
            // The current session needs no reload; the menu just closes.
            search::Act::Resume(id) => {
                self.menu = None;
                self.search.close();
                if Some(id) != self.session || self.inspect.is_some() {
                    self.resume(id, tx);
                }
            }
        }
    }

    /// Show the page ending at `seq` as the **Earlier messages** inspection
    /// (49 §7): the session's rows up to the match, the match revealed at the
    /// top (a removed row as its archive card), live events kept out until
    /// **Jump to latest** loads the normal tail. Nothing here enters the
    /// model's context.
    fn inspect_page(&mut self, session: SessionId, seq: i64, loaded: Loaded) {
        let approvals = std::mem::take(&mut self.approvals);
        let asks = std::mem::take(&mut self.asks);
        self.reset();
        self.approvals = approvals;
        self.asks = asks;
        self.session = Some(session);
        let opts = self.assistant_options();
        let mut target = None;
        let mut entries = Vec::new();
        for h in loaded.history {
            let here = h
                .context()
                .is_some_and(|c| c.target == ContextTarget::Message { seq });
            if here {
                target = Some(entries.len());
                entries.push(Entry::Info(format!("▸ matched message · seq {seq}")));
            }
            entries.extend(from_history(h, &opts));
        }
        if target.is_none() {
            entries.insert(
                0,
                Entry::Info(format!(
                    "message {seq} is not on this page (removed or hidden); showing the messages around it"
                )),
            );
        }
        self.entries = entries;
        self.anchor = Some((target.unwrap_or(0), 0));
        self.model_id = loaded.model;
        self.harness_model = loaded.harness_model;
        self.harness_commands = loaded.harness_commands;
        self.context_size = loaded.context_size;
        self.agent = loaded.agent;
        self.effort = loaded.effort;
        self.session_name = loaded.name;
        self.session_parent = loaded.parent;
        self.streaming = loaded.running;
        self.inspect = Some(search::Inspect {
            session,
            seq,
            found: target.is_some(),
        });
    }

    /// **Jump to latest** from the inspection: the normal tail, live again.
    pub fn jump_latest(&mut self, tx: &UnboundedSender<Command>) {
        if let Some(i) = self.inspect.take() {
            self.resume(i.session, tx);
        }
    }

    /// **Back to results** from the inspection: the normal tail loads behind
    /// the reopened menu, which shows the view as left.
    pub fn back_to_results(&mut self, tx: &UnboundedSender<Command>) {
        if let Some(i) = self.inspect.take() {
            self.resume(i.session, tx);
        }
        self.menu = Some(Menu::Search);
        for r in self.search.restore() {
            send(tx, Command::Search(r));
        }
    }

    /// The inspection's hint row (49 §7).
    pub fn inspect_line(&self) -> Option<String> {
        let i = self.inspect.as_ref()?;
        Some(format!(
            "earlier messages · {} · message {}{}   ↑↓ scroll  ⏎/j jump to latest  esc/b back to results",
            short(i.session),
            i.seq,
            if i.found { "" } else { " (not on this page)" }
        ))
    }
}
