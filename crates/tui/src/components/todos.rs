//! Versioned pinned todo card, disclosure and scroll ownership.
use std::collections::HashMap;
use agent::todo::{Status as TodoStatus, TodoSnapshot, TodoSource};
use rpc_compose::api::clients::TurnsApiClient;
use super::*;

#[derive(Default)]
pub(crate) struct Todos {
    pub todos: Option<TodoSnapshot>,
    pub todos_stale: bool,
    open: HashMap<SessionId, bool>,
    pub focus: bool,
    pub scroll: usize,
    session: Option<SessionId>,
}
#[derive(Clone)]
pub(crate) enum Request { GetTodos(SessionId) }
pub(crate) enum Msg {
    Changed(SessionId, TodoSnapshot),
    Loaded(SessionId, Result<Option<TodoSnapshot>, String>),
}

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
    /// Wrapped body rows in view (`Todos::scroll` applied); empty collapsed.
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

impl Todos {

    /// `Loaded.todos` (47 §5.4): a snapshot applies by version; none means the
    /// read failed — the plan on screen stays, labelled stale, never emptied.
    fn loaded_todos(&mut self, todos: Option<TodoSnapshot>) {
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
    pub(crate) fn set_todos(&mut self, s: TodoSnapshot) {
        match &self.todos {
            Some(cur) if s.version < cur.version => return,
            Some(cur) if s.version == cur.version => {}
            _ => {
                if s.items.is_empty() {
                    if let Some(id) = self.session {
                        self.open.insert(id, false);
                    }
                    self.focus = false;
                }
                self.todos = Some(s);
            }
        }
        self.todos_stale = false;
    }
}

impl Todos {

    /// Core prompts and modals suspend capture without losing disclosure/focus.
    pub(crate) fn focused(&self, blocked: bool) -> bool { self.focus && !blocked }

    /// The shown session's disclosure choice; collapsed until F2 (47 §6.1).
    pub(crate) fn todo_expanded(&self) -> bool {
        self.session
            .is_some_and(|s| self.open.get(&s).copied().unwrap_or(false))
    }

    /// F2 and `/todos`: expand and focus a collapsed card, collapse an open
    /// one. The editor's draft is untouched. Opening re-reads `get_todos`
    /// so a plan written while this client was elsewhere shows (47 §6.3).
    fn toggle(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        let Some(session) = self.session else {
            cx.core(CoreAction::Notice("no session".into()));
            return;
        };
        let open = !self.todo_expanded();
        self.open.insert(session, open);
        self.focus = open;
        if open {
            cx.spawn_request(Lane::Concurrent, Request::GetTodos(session), |api, Request::GetTodos(session)| async move {
                Msg::Loaded(session, api.get_todos(session).await.map(Some).map_err(crate::reason))
            });
        }
    }

    /// A key while the card is focused ([`Todos::focused`]) at a
    /// `width`×`height` terminal: ↑/↓ and PgUp/PgDn scroll the body, esc
    /// collapses and hands the editor back (no turn is cancelled), tab
    /// hands it back with the card left open, ⏎ does nothing (the draft is
    /// never submitted from here). `true` when consumed; any other key
    /// hands the editor back and is *not* consumed, so typing lands in the
    /// draft instead of vanishing.
    pub(crate) fn todo_key(&mut self, code: KeyCode, width: usize, height: usize) -> bool {
        let total = self.todo_lines(width).len();
        let rows = total.min(self.todo_body_rows(height));
        let page = rows.max(1);
        // A resize may have left the scroll past the end; step from what is drawn.
        let at = self.scroll.min(total - rows);
        self.scroll = match code {
            KeyCode::Up => at.saturating_sub(1),
            KeyCode::Down => at + 1,
            KeyCode::PageUp => at.saturating_sub(page),
            KeyCode::PageDown => at + page,
            KeyCode::Esc => {
                if let Some(s) = self.session {
                    self.open.insert(s, false);
                }
                self.focus = false;
                at
            }
            KeyCode::Tab => {
                self.focus = false;
                at
            }
            KeyCode::Enter => at,
            _ => {
                self.focus = false;
                return false;
            }
        }
        .min(total - rows);
        true
    }

    /// The card is drawn: the plan has items, is stale or unavailable, or
    /// the user opened it. Only an authoritative empty plan hides it (47 §6.1).
    pub(crate) fn todo_shown(&self) -> bool {
        self.todo_expanded()
            || self.todos_stale
            || self.todos.as_ref().is_some_and(|t| !t.items.is_empty())
    }

    /// Body rows the open card may use: [`TODO_ROWS`], fewer on a short
    /// terminal so the trigger row, editor and some transcript stay; `0`
    /// leaves the trigger alone (47 §6.3).
    pub(crate) fn todo_body_rows(&self, height: usize) -> usize {
        TODO_ROWS.min(height.saturating_sub(14))
    }

    /// The open body at `width` columns (`0` = unbounded): `[status] text`
    /// per item, wrapped under the text column, a blocker's reason on its
    /// own rows. Nothing is cut; scrolling reaches every row (47 §6.1).
    pub(crate) fn todo_lines(&self, width: usize) -> Vec<(Option<TodoStatus>, String)> {
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
    pub(crate) fn todo_card(&self, width: usize, height: usize, blocked: bool) -> Option<TodoCard> {
        if !self.todo_shown() {
            return None;
        }
        let expanded = self.todo_expanded();
        let focused = self.focused(blocked);
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
        let from = self.scroll.min(total - rows);
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
/// The fixed todo card (47 §6.3): the trigger row, then while open the
/// body rows in view and the overflow indicator. Session chrome beside the
/// board, never a transcript entry; F2 gives it the keys.
fn render(c: TodoCard) -> AnyElement<'static> {
    use iocraft::prelude::*;
    use crate::ui::{OCHRE, SAND, BONE, BONE_DIM, FAIENCE};
    let border = if c.focused { OCHRE } else { SAND };
    let head = if c.focused { BONE } else { BONE_DIM };
    let paint = |s: Option<TodoStatus>| match s {
        Some(TodoStatus::InProgress) => FAIENCE,
        Some(TodoStatus::Pending) => BONE_DIM,
        Some(TodoStatus::Blocked) => OCHRE,
        Some(TodoStatus::Done | TodoStatus::Dropped) | None => SAND,
    };
    element! {
        View(border_style: BorderStyle::Round, border_color: border, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
            Text(content: c.header, color: head, wrap: TextWrap::NoWrap)
            #(c.rows.into_iter().map(|(status, text)| {
                element!(Text(content: text, color: paint(status), wrap: TextWrap::NoWrap))
            }))
            #(c.overflow.map(|s| element!(Text(content: s, color: SAND, wrap: TextWrap::NoWrap))))
        }
    }
    .into_any()
}

impl Component for Todos {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "todos", interests: &[Interest { topic: "agent.events", version: 1, variants: &["TodoChanged"] }],
            slash: &[("/todos", "", "show or hide the plan card (F2)")],
            bindings: &[Binding { class: FocusClass::Pinned, code: KeyCode::F(2), modifiers: KeyModifiers::NONE }] }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if event.event()["type"].as_str() != Some("TodoChanged") { return None; }
        match core.agent? {
            agent::AgentEvent::TodoChanged { session, snapshot } => Some(Msg::Changed(*session, snapshot.clone())),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        self.session = cx.core.session;
        match input {
            Input::Message(Msg::Changed(session, snapshot)) if Some(session) == self.session => self.set_todos(snapshot),
            Input::Message(Msg::Loaded(session, result)) => {
                let mut text = if cx.core.reasoning { "\x1b[0m\n".to_owned() } else { String::new() };
                if let Ok(Some(snapshot)) = &result {
                    use std::fmt::Write;
                    for item in &snapshot.items {
                        let _ = writeln!(text, "[{}] {}", status_label(item.status), item.text);
                        if let Some(reason) = &item.reason { let _ = writeln!(text, "    {reason}"); }
                    }
                }
                if !text.is_empty() { cx.plain(Plain::Text(text)); }
                match result {
                    Ok(snapshot) if Some(session) == self.session => self.loaded_todos(snapshot),
                    Err(error) => {
                        if Some(session) == self.session { self.loaded_todos(None); }
                        cx.error(error);
                    }
                    _ => {}
                }
            }
            Input::Lifecycle(Lifecycle::SessionRead(loaded)) => self.loaded_todos(loaded.todos.clone()),
            Input::Lifecycle(Lifecycle::SessionReset) => {
                self.todos = None;
                self.todos_stale = false;
                self.focus = false;
                self.scroll = 0;
                self.session = None;
            }
            Input::Slash { .. } => self.toggle(cx),
            Input::Key(key) if cx.core.prompt.is_none() && !cx.core.modal_open => {
                if key.code == KeyCode::F(2) { self.toggle(cx); cx.consume_key(); }
                else if self.focus && self.todo_key(key.code, cx.core.width, cx.core.terminal_height) { cx.consume_key(); }
            }
            _ => {}
        }
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Pinned && self.focus }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        let blocked = cx.core.prompt.is_some() || cx.core.modal_open;
        match at {
            Surface::Pinned => {
                let card = self.todo_card(cx.width, cx.height, blocked)?;
                Some(View::Element { rows: card.height(), element: render(card), replace: false })
            }
            Surface::Status(StatusSlot::Pinned) if self.focused(blocked) => Some(View::Text("  ·  todos: ↑↓ scroll · esc closes · tab editor".into())),
            _ => None,
        }
    }
}
