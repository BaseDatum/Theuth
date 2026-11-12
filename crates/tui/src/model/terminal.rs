use super::*;

/// Poll period of the open terminal observer (51 §7.3: at most one ask per half second).
pub const TERMINAL_POLL_MS: u64 = 500;
/// Poll period while the human is typing into the live screen (55 §3).
pub const TERMINAL_INTERACTIVE_POLL_MS: u64 = 60;
/// Rows a history page asks for.
pub(super) const TERMINAL_PAGE: u16 = 40;

/// The `/terminals` observer over one PTY job (51 §7.4): what is asked
/// for, what was last painted, and the request bookkeeping that keeps one
/// ask in flight and drops replies for a selection or viewport left behind.
#[derive(Debug)]
pub struct TerminalView {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub job: JobId,
    /// `name · command` for the title.
    pub label: String,
    /// Rows wanted; `Live` follows the screen.
    pub viewport: PtyViewport,
    /// Bumped by [`Model`] whenever the selected terminal changes; a reply
    /// with another epoch is dropped even if session and job match.
    pub epoch: u64,
    /// Last page painted.
    pub page: Option<PtyPage>,
    /// Revision of `page` (or of the last `Unchanged`).
    pub revision: Option<u64>,
    /// An ask is out; nothing else is sent until it answers.
    pub inflight: bool,
    /// Something changed while an ask was out: re-ask as soon as it answers.
    pub dirty: bool,
    /// Unix ms of the next scheduled poll.
    pub next_poll: u64,
    /// Leading columns dropped (horizontal pan).
    pub col: usize,
    /// The owner said the terminal is gone.
    pub unavailable: bool,
    /// Daemon epoch of the first page; another one means the daemon restarted.
    pub daemon_epoch: Option<Option<String>>,
    /// A later page named another daemon epoch.
    pub restarted: bool,
    /// The last failed ask, shown in the status row.
    pub error: Option<String>,
    /// Keys go to the process (55 §3); Ctrl-] returns to viewing.
    pub interactive: bool,
}

impl TerminalView {
    /// A fresh observer on `t`, following the live screen.
    pub fn new(t: &TerminalSummary, epoch: u64) -> Self {
        let label = match &t.name {
            Some(n) => format!("{n} · {}", t.command),
            None => t.command.clone(),
        };
        Self {
            session: t.session,
            job: t.id.clone(),
            label,
            viewport: PtyViewport::Live,
            epoch,
            page: None,
            revision: None,
            inflight: false,
            dirty: false,
            next_poll: 0,
            col: 0,
            unavailable: false,
            daemon_epoch: None,
            restarted: false,
            error: None,
            interactive: false,
        }
    }

    /// Change the viewport: what was painted no longer matches, so the next
    /// ask carries no `if_revision`.
    fn set_viewport(&mut self, v: PtyViewport) {
        if v != self.viewport {
            self.viewport = v;
            self.revision = None;
            self.dirty = true;
            self.next_poll = 0;
        }
    }

    /// Scroll `rows` up (negative) or down (positive) through history; past
    /// the bottom the view is live again.
    pub(super) fn scroll(&mut self, rows: i64) {
        let retained = self.page.as_ref().map_or(0, |p| u64::from(p.retained));
        let (offset, height) = match self.viewport {
            PtyViewport::Live => (0i64, TERMINAL_PAGE),
            PtyViewport::History { offset, height } => (i64::from(offset), height),
        };
        // Up means an older page: a larger offset above the live screen.
        let offset = (offset - rows).clamp(0, i64::try_from(retained).unwrap_or(i64::MAX));
        let v = if offset == 0 {
            PtyViewport::Live
        } else {
            PtyViewport::History {
                offset: u32::try_from(offset).unwrap_or(u32::MAX),
                height,
            }
        };
        self.set_viewport(v);
    }

    /// Fold a reply in. Applies only when it answers the current selection
    /// and viewport and is not older than what is shown; an `Unchanged` for
    /// another viewport re-asks. Returns whether the screen changed.
    pub fn reply(
        &mut self,
        session: SessionId,
        job: &JobId,
        viewport: PtyViewport,
        epoch: u64,
        reply: Result<PtyScreenReply, String>,
    ) -> bool {
        if session != self.session || *job != self.job || epoch != self.epoch {
            return false;
        }
        self.inflight = false;
        if viewport != self.viewport {
            // Answers the viewport before a scroll; the scroll already re-asked.
            self.dirty = true;
            self.next_poll = 0;
            return false;
        }
        match reply {
            Err(e) => {
                self.error = Some(e);
                false
            }
            Ok(PtyScreenReply::Unavailable) => {
                self.error = None;
                self.unavailable = true;
                true
            }
            Ok(PtyScreenReply::Unchanged {
                viewport: v,
                revision,
            }) => {
                self.error = None;
                if v != self.viewport {
                    self.revision = None;
                    self.dirty = true;
                    self.next_poll = 0;
                } else if self.revision.is_none_or(|r| r <= revision) {
                    self.revision = Some(revision);
                }
                false
            }
            Ok(PtyScreenReply::Updated(page)) => {
                self.error = None;
                if page.session != self.session
                    || page.id != self.job
                    || self.revision.is_some_and(|r| page.revision < r)
                {
                    return false;
                }
                match &self.daemon_epoch {
                    None => self.daemon_epoch = Some(page.epoch.clone()),
                    Some(e) if *e != page.epoch => self.restarted = true,
                    Some(_) => {}
                }
                self.revision = Some(page.revision);
                self.unavailable = false;
                if page.exit.is_some() {
                    self.interactive = false;
                }
                self.page = Some(page);
                true
            }
        }
    }

    /// The next ask when one is due at `now` and none is out: marks it in
    /// flight and schedules the following poll.
    pub fn poll(&mut self, now: u64) -> Option<Command> {
        if self.inflight || self.unavailable || (!self.dirty && now < self.next_poll) {
            return None;
        }
        self.inflight = true;
        self.dirty = false;
        let period = if self.interactive && self.viewport == PtyViewport::Live {
            TERMINAL_INTERACTIVE_POLL_MS
        } else {
            TERMINAL_POLL_MS
        };
        self.next_poll = now + period;
        Some(Command::PtyScreen {
            session: self.session,
            job: self.job.clone(),
            viewport: self.viewport,
            if_revision: self.revision,
            epoch: self.epoch,
        })
    }

    /// When the event loop must next wake for this view (unix ms); `None` while nothing is due.
    pub fn wake(&self) -> Option<u64> {
        (!self.inflight && !self.unavailable).then_some(if self.dirty { 0 } else { self.next_poll })
    }

    /// Title row: label, owner, state, viewport and revision.
    pub fn title(&self) -> String {
        let state = match &self.page {
            Some(p) => match &p.exit {
                Some(e) => format!("exit {e}"),
                None => "running".to_owned(),
            },
            None => "loading…".to_owned(),
        };
        let where_ = match self.viewport {
            PtyViewport::Live => "live".to_owned(),
            PtyViewport::History { offset, .. } => format!("history −{offset}"),
        };
        let rev = self
            .revision
            .map_or_else(String::new, |r| format!(" · rev {r}"));
        let keys = if self.interactive {
            "typing · Ctrl-] to stop"
        } else if self.page.as_ref().is_some_and(|p| p.exit.is_none()) {
            "↑↓ PgUp PgDn history  ←→ pan  r refresh  i type  esc close"
        } else {
            "↑↓ PgUp PgDn history  ←→ pan  r refresh  esc close"
        };
        format!(
            "terminal · {} · {} · {state} · {where_}{rev}   {keys}",
            short(self.session),
            self.job.0,
        )
    }

    /// Status row under the page, if anything needs saying.
    pub fn status(&self) -> Option<String> {
        if self.unavailable {
            return Some("No longer available.".to_owned());
        }
        let mut notes = Vec::new();
        if self.restarted {
            notes.push("Daemon restarted.".to_owned());
        }
        if let Some(p) = &self.page {
            if !p.redaction_complete {
                notes.push("redaction incomplete at retained boundary".to_owned());
            }
            if p.truncated {
                notes.push("top rows cut to fit the reply".to_owned());
            }
        }
        if let Some(e) = &self.error {
            notes.push(format!("last ask failed: {e}"));
        }
        (!notes.is_empty()).then(|| notes.join(" · "))
    }
}

/// One `/terminals` row: `● abcd1234  j3  name · command  running  rev 12`.
pub fn terminal_row(t: &TerminalSummary) -> String {
    let label = match &t.name {
        Some(n) => format!("{n} · {}", t.command),
        None => t.command.clone(),
    };
    let label: String = label.chars().take(48).collect();
    format!(
        "{} {}  {:<4} {:<48} {:<9} rev {}",
        if t.state == "running" { "●" } else { "○" },
        short(t.session),
        t.id.0,
        label,
        t.state,
        t.revision
    )
}

impl Model {

    /// `/terminals`: ⏎ opens the observer on the highlighted job, `r` re-lists.
    pub(super) fn terminals_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Terminals { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('r') => {
                if let Some(session) = self.session {
                    send(tx, Command::ListTerminals(session));
                }
            }
            KeyCode::Enter => {
                if let Some(t) = items.get(*cursor) {
                    self.terminal_epoch += 1;
                    let mut view = TerminalView::new(t, self.terminal_epoch);
                    if let Some(cmd) = view.poll(now_ms()) {
                        send(tx, cmd);
                    }
                    self.menu = Some(Menu::Terminal(Box::new(view)));
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// The observer's keys (51 §7.4): history scrolling, horizontal pan,
    /// `r` refresh, `i`/⏎ to type into the process (55 §3), esc back to the picker.
    pub(super) fn terminal_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Terminal(v)) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => v.scroll(-1),
            KeyCode::Down => v.scroll(1),
            KeyCode::PageUp => v.scroll(-i64::from(TERMINAL_PAGE)),
            KeyCode::PageDown => v.scroll(i64::from(TERMINAL_PAGE)),
            KeyCode::Left => v.col = v.col.saturating_sub(DIFF_STEP),
            KeyCode::Right => {
                let cols = v.page.as_ref().map_or(120, |p| usize::from(p.cols));
                v.col = (v.col + DIFF_STEP).min(cols.saturating_sub(DIFF_STEP));
            }
            KeyCode::Char('r') => {
                v.revision = None;
                v.dirty = true;
                v.next_poll = 0;
            }
            // 55 §3: hand the keyboard to the program while it runs.
            KeyCode::Char('i') | KeyCode::Enter
                if v.page.as_ref().is_some_and(|p| p.exit.is_none()) =>
            {
                v.interactive = true;
                v.set_viewport(PtyViewport::Live);
            }
            // Closes the overlay only; the owned tree is untouched (51 §4.3, §7.4).
            KeyCode::Esc => {
                self.menu = None;
                return MenuKey::Consumed;
            }
            _ => {}
        }
        if let Some(cmd) = v.poll(now_ms()) {
            send(tx, cmd);
        }
        MenuKey::Consumed
    }

    /// When the event loop must next wake for the observer (unix ms); `None` while none is open.
    pub fn terminal_wake(&self) -> Option<u64> {
        match &self.menu {
            Some(Menu::Terminal(v)) => v.wake(),
            _ => None,
        }
    }

    /// Send the observer's due poll, if any (51 §7.3: one in flight, one per half second).
    pub fn terminal_tick(&mut self, now: u64, tx: &UnboundedSender<Command>) {
        if let Some(Menu::Terminal(v)) = &mut self.menu
            && let Some(cmd) = v.poll(now)
        {
            send(tx, cmd);
        }
    }

    /// The open observer is forwarding keys to the process (55 §3).
    pub fn terminal_interactive(&self) -> bool {
        matches!(&self.menu, Some(Menu::Terminal(v)) if v.interactive)
    }

    /// A key while interactive: Ctrl-] detaches, everything else is typed
    /// into the process as structured input (`pty.rs` owns the byte table).
    pub fn terminal_type(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        tx: &UnboundedSender<Command>,
    ) {
        let Some(Menu::Terminal(v)) = &mut self.menu else {
            return;
        };
        if code == KeyCode::Char(']') && modifiers.contains(KeyModifiers::CONTROL) {
            v.interactive = false;
            return;
        }
        if let Some((text, keys)) = key_input(code, modifiers) {
            send(
                tx,
                Command::PtyInput {
                    session: v.session,
                    job: v.job.clone(),
                    text,
                    keys,
                },
            );
            v.dirty = true;
            if let Some(cmd) = v.poll(now_ms()) {
                send(tx, cmd);
            }
        }
    }

    /// Pasted text while interactive goes in one piece (bracketed when the
    /// program asked for it).
    pub fn terminal_paste(&mut self, text: String, tx: &UnboundedSender<Command>) {
        if let Some(Menu::Terminal(v)) = &mut self.menu {
            send(
                tx,
                Command::PtyInput {
                    session: v.session,
                    job: v.job.clone(),
                    text: None,
                    keys: vec![Key::Paste(text)],
                },
            );
            v.dirty = true;
        }
    }
}

/// One terminal key as `pty_input` wants it; `None` for keys with no
/// terminal meaning (bare modifiers, media keys).
fn key_input(code: KeyCode, modifiers: KeyModifiers) -> Option<(Option<String>, Vec<Key>)> {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    let key = match code {
        KeyCode::Char(c) if ctrl => Key::Ctrl(c),
        KeyCode::Char(c) if alt => Key::Alt(c),
        KeyCode::Char(c) => return Some((Some(c.to_string()), Vec::new())),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::ShiftTab,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    Some((None, vec![key]))
}
