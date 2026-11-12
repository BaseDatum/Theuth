use super::*;

/// Body rows the open todo card shows at most (47 §6.3).
pub const TODO_ROWS: usize = 6;
/// Columns of a todo row's status column: `[in progress] `.
const TODO_LABEL: usize = 14;

/// What the fixed todo card draws (47 §6.3): the trigger row, and while
/// open the body rows in view with an overflow indicator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoCard {
    /// `> Todos · 2/5 done · 1 blocked   Now: …  [F2]`; `v` while open.
    pub header: String,
    /// Wrapped body rows in view (`Model::todo_scroll` applied); empty collapsed.
    /// The status colours an item's rows; `None` is a blocker's reason.
    pub rows: Vec<(Option<TodoStatus>, String)>,
    /// `↕ 3–8 / 14 · ↑↓ PgUp PgDn` when the body does not fit; `None` otherwise.
    pub overflow: Option<String>,
    /// The card has the keys.
    pub focused: bool,
}

impl TodoCard {
    /// Terminal rows the card takes: border, trigger, body, overflow row.
    pub fn height(&self) -> usize {
        3 + self.rows.len() + usize::from(self.overflow.is_some())
    }
}

/// The bracketed label of a todo row.
pub fn status_label(s: TodoStatus) -> &'static str {
    match s {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in progress",
        TodoStatus::Done => "done",
        TodoStatus::Dropped => "dropped",
        TodoStatus::Blocked => "blocked",
    }
}

/// Greedy word wrap at `width` chars (`0` = one line); a word longer than
/// the width is split. Never empty: an empty text is one empty row.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let n = word.chars().count();
        if width > 0 && len > 0 && len + 1 + n > width {
            lines.push(std::mem::take(&mut cur));
            len = 0;
        } else if len > 0 {
            cur.push(' ');
            len += 1;
        }
        for c in word.chars() {
            if width > 0 && len >= width {
                lines.push(std::mem::take(&mut cur));
                len = 0;
            }
            cur.push(c);
            len += 1;
        }
    }
    lines.push(cur);
    lines
}

impl Model {

    /// `Loaded.todos` (47 §5.4): a snapshot applies by version; none means the
    /// read failed — the plan on screen stays, labelled stale, never emptied.
    pub(super) fn loaded_todos(&mut self, todos: Option<TodoSnapshot>) {
        match todos {
            Some(s) => self.set_todos(s),
            None => self.todos_stale = true,
        }
    }

    /// Install a snapshot of the shown session: only a higher version
    /// replaces; an equal one is idempotent and, being authoritative, clears
    /// the stale mark; an older one (a slow `load_session` racing a
    /// `TodoChanged`) is dropped (47 §5.4). A newer *empty* plan is an
    /// explicit clear: the card hides whatever the disclosure choice was, and
    /// gives the editor back if it had the keys (47 §6.1).
    pub fn set_todos(&mut self, s: TodoSnapshot) {
        match &self.todos {
            Some(cur) if s.version < cur.version => return,
            Some(cur) if s.version == cur.version => {}
            _ => {
                if s.items.is_empty() {
                    if let Some(id) = self.session {
                        self.todo_open.insert(id, false);
                    }
                    self.todo_focus = false;
                }
                self.todos = Some(s);
            }
        }
        self.todos_stale = false;
    }
}

impl Model {

    /// The todo card has the keys: F2 focused it and nothing modal is up.
    /// Approvals, secret entry, `ask_user` and menus keep their own keys;
    /// their answers are never consumed here (47 §6.3).
    pub fn todo_focused(&self) -> bool {
        self.todo_focus
            && self.approval().is_none()
            && self.secret_request().is_none()
            && self.ask().is_none()
            && !self.modal_menu()
    }

    /// The shown session's disclosure choice; collapsed until F2 (47 §6.1).
    pub fn todo_expanded(&self) -> bool {
        self.session
            .is_some_and(|s| self.todo_open.get(&s).copied().unwrap_or(false))
    }

    /// F2 and `/todos`: expand and focus a collapsed card, collapse an open
    /// one. The editor's draft is untouched. Opening re-reads `get_todos`
    /// so a plan written while this client was elsewhere shows (47 §6.3).
    pub fn toggle_todos(&mut self, tx: &UnboundedSender<Command>) {
        let Some(session) = self.session else {
            self.entries.push(Entry::Info("no session".into()));
            return;
        };
        let open = !self.todo_expanded();
        self.todo_open.insert(session, open);
        self.todo_focus = open;
        if open {
            send(tx, Command::GetTodos(session));
        }
    }

    /// A key while the card is focused ([`Model::todo_focused`]) at a
    /// `width`×`height` terminal: ↑/↓ and PgUp/PgDn scroll the body, esc
    /// collapses and hands the editor back (no turn is cancelled), tab
    /// hands it back with the card left open, ⏎ does nothing (the draft is
    /// never submitted from here). `true` when consumed; any other key
    /// hands the editor back and is *not* consumed, so typing lands in the
    /// draft instead of vanishing.
    pub fn todo_key(&mut self, code: KeyCode, width: usize, height: usize) -> bool {
        let total = self.todo_lines(width).len();
        let rows = total.min(self.todo_body_rows(height));
        let page = rows.max(1);
        // A resize may have left the scroll past the end; step from what is drawn.
        let at = self.todo_scroll.min(total - rows);
        self.todo_scroll = match code {
            KeyCode::Up => at.saturating_sub(1),
            KeyCode::Down => at + 1,
            KeyCode::PageUp => at.saturating_sub(page),
            KeyCode::PageDown => at + page,
            KeyCode::Esc => {
                if let Some(s) = self.session {
                    self.todo_open.insert(s, false);
                }
                self.todo_focus = false;
                at
            }
            KeyCode::Tab => {
                self.todo_focus = false;
                at
            }
            KeyCode::Enter => at,
            _ => {
                self.todo_focus = false;
                return false;
            }
        }
        .min(total - rows);
        true
    }

    /// The card is drawn: the plan has items, is stale or unavailable, or
    /// the user opened it. Only an authoritative empty plan hides it (47 §6.1).
    pub fn todo_shown(&self) -> bool {
        self.todo_expanded()
            || self.todos_stale
            || self.todos.as_ref().is_some_and(|t| !t.items.is_empty())
    }

    /// Body rows the open card may use: [`TODO_ROWS`], fewer on a short
    /// terminal so the trigger row, editor and some transcript stay; `0`
    /// leaves the trigger alone (47 §6.3).
    pub fn todo_body_rows(&self, height: usize) -> usize {
        TODO_ROWS.min(height.saturating_sub(14))
    }

    /// The open body at `width` columns (`0` = unbounded): `[status] text`
    /// per item, wrapped under the text column, a blocker's reason on its
    /// own rows. Nothing is cut; scrolling reaches every row (47 §6.1).
    pub fn todo_lines(&self, width: usize) -> Vec<(Option<TodoStatus>, String)> {
        let Some(t) = &self.todos else {
            return Vec::new();
        };
        // Border 2 + padding 2 + one spare column (see `menu`), then the label column.
        let text_width = match width {
            0 => 0,
            w => w.saturating_sub(5 + TODO_LABEL).max(10),
        };
        let pad = " ".repeat(TODO_LABEL);
        let mut out = Vec::new();
        for item in &t.items {
            let label = format!(
                "{:<TODO_LABEL$}",
                format!("[{}]", status_label(item.status))
            );
            for (i, line) in wrap(&item.text, text_width).into_iter().enumerate() {
                let lead = if i == 0 { &label } else { &pad };
                out.push((Some(item.status), format!("{lead}{line}")));
            }
            if let Some(reason) = &item.reason {
                for line in wrap(reason, text_width) {
                    out.push((None, format!("{pad}{line}")));
                }
            }
        }
        out
    }

    /// What the fixed todo card draws this frame; `None` while hidden.
    pub fn todo_card(&self, width: usize, height: usize) -> Option<TodoCard> {
        if !self.todo_shown() {
            return None;
        }
        let expanded = self.todo_expanded();
        let focused = self.todo_focused();
        let (glyph, hint) = if expanded { ("v", "") } else { (">", "  [F2]") };
        let external = match &self.todos {
            Some(t) if t.source == TodoSource::Acp => " (external plan)",
            _ => "",
        };
        let mut header = format!("{glyph} Todos{external} · {}", self.todo_summary());
        if !expanded {
            if let Some(now) = self.todo_now() {
                // Fit the row: border, padding and a spare column (see `menu`).
                let room = match width {
                    0 => usize::MAX,
                    w => w
                        .saturating_sub(5 + header.chars().count() + hint.len() + 3)
                        .max(8),
                };
                let now: String = if now.chars().count() > room {
                    now.chars().take(room - 1).chain(['…']).collect()
                } else {
                    now
                };
                header.push_str("   ");
                header.push_str(&now);
            }
            header.push_str(hint);
            return Some(TodoCard {
                header,
                rows: Vec::new(),
                overflow: None,
                focused,
            });
        }
        let lines = self.todo_lines(width);
        let total = lines.len();
        let rows = total.min(self.todo_body_rows(height));
        let from = self.todo_scroll.min(total - rows);
        let overflow = (total > rows).then(|| {
            if rows == 0 {
                format!("{total} rows need a taller terminal")
            } else {
                format!("↕ {}–{} / {total} · ↑↓ PgUp PgDn", from + 1, from + rows)
            }
        });
        Some(TodoCard {
            header,
            rows: lines.into_iter().skip(from).take(rows).collect(),
            overflow,
            focused,
        })
    }

    /// `2/5 done · 1 blocked`; a settled list with drops reads `3 done, 1
    /// dropped`, never `100%`; `none` for an empty plan, `unavailable`
    /// without one; ` · stale` while the shown plan may be behind (47 §6.1).
    fn todo_summary(&self) -> String {
        let Some(t) = &self.todos else {
            return "unavailable".into();
        };
        let count = |s: TodoStatus| t.items.iter().filter(|i| i.status == s).count();
        let (done, dropped, blocked) = (
            count(TodoStatus::Done),
            count(TodoStatus::Dropped),
            count(TodoStatus::Blocked),
        );
        let open = t.items.iter().any(|i| i.status.is_actionable());
        let mut s = match (t.items.len(), open, blocked, dropped) {
            (0, ..) => "none".to_owned(),
            (_, false, 0, 1..) => format!("{done} done, {dropped} dropped"),
            (n, ..) => format!("{done}/{n} done"),
        };
        if dropped > 0 && open {
            s.push_str(&format!(" · {dropped} dropped"));
        }
        if blocked > 0 {
            s.push_str(&format!(" · {blocked} blocked"));
        }
        if self.todos_stale {
            s.push_str(" · stale");
        }
        s
    }

    /// The collapsed row's task: `Now: <in progress>`, else the first
    /// pending item, else `Waiting: N blocked` for a blocked-only list;
    /// nothing when settled (47 §6.1).
    fn todo_now(&self) -> Option<String> {
        let t = self.todos.as_ref()?;
        let pick = |s: TodoStatus| t.items.iter().find(|i| i.status == s);
        if let Some(i) = pick(TodoStatus::InProgress).or_else(|| pick(TodoStatus::Pending)) {
            return Some(format!("Now: {}", i.text));
        }
        match t
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Blocked)
            .count()
        {
            0 => None,
            n => Some(format!("Waiting: {n} blocked")),
        }
    }
}
