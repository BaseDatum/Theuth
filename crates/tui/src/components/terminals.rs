//! Retained terminal observer and ordered interactive input.
use super::*;
use super::Component;
use iocraft::prelude::{View as BoxView, *};
use rpc_compose::api::clients::{AgentHubApiClient, JobsApiClient};
use runtime::SessionId;
use tools::{JobId, Key, PtyPage, PtyScreenReply, PtyViewport, TerminalSummary};
use crate::model::{short, now_ms};
const DIFF_STEP: usize = 8;

#[derive(Clone, Debug)]
pub(crate) enum Request {
    ListTerminals(SessionId),
    PtyScreen { session: SessionId, job: JobId, viewport: PtyViewport, if_revision: Option<u64>, epoch: u64 },
    PtyInput { session: SessionId, job: JobId, text: Option<String>, keys: Vec<Key> },
}
pub(crate) enum Msg {
    Terminals(Result<Vec<TerminalSummary>, String>),
    PtyScreen { session: SessionId, job: JobId, viewport: PtyViewport, epoch: u64, reply: Result<PtyScreenReply, String> },
    Input(Result<(), String>),
}
#[derive(Default)]
pub(crate) struct Terminals { pub(crate) menu: Option<Menu>, epoch: u64 }
#[derive(Debug)]
pub(crate) enum Menu { Terminals { items: Vec<TerminalSummary>, cursor: usize }, Terminal(Box<TerminalView>) }
/// Poll period of the open terminal observer (51 §7.3: at most one ask per half second).
pub const TERMINAL_POLL_MS: u64 = 500;
/// Poll period while the human is typing into the live screen (55 §3).
pub const TERMINAL_INTERACTIVE_POLL_MS: u64 = 60;
/// Rows a history page asks for.
pub(crate) const TERMINAL_PAGE: u16 = 40;

/// The `/terminals` observer over one PTY job (51 §7.4): what is asked
/// for, what was last painted, and the request bookkeeping that keeps one
/// ask in flight and drops replies for a selection or viewport left behind.
#[derive(Debug)]
pub(crate) struct TerminalView {
    /// Owning session.
    pub session: SessionId,
    /// Job.
    pub job: JobId,
    /// Rows wanted; `Live` follows the screen.
    pub viewport: PtyViewport,
    /// Bumped whenever the selected terminal changes; a reply
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
        Self {
            session: t.session,
            job: t.id.clone(),
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
    pub(crate) fn scroll(&mut self, rows: i64) {
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
    pub fn poll(&mut self, now: u64) -> Option<Request> {
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
        Some(Request::PtyScreen {
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

    /// Title row: owner, state, viewport and revision.
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

impl Terminals {
    fn send(request: Request, cx: &mut Cx<'_, '_, Msg>) {
        let lane = if matches!(request, Request::PtyInput { .. }) { Lane::OrderedInput } else { Lane::Concurrent };
        cx.spawn_request(lane, request, |api, request| async move {
            match request {
                Request::ListTerminals(session) => Msg::Terminals(api.list_terminals(session, Some(true)).await.map_err(crate::reason)),
                Request::PtyScreen { session, job, viewport, if_revision, epoch } => {
                    let reply = api.pty_screen(session, job.clone(), viewport, if_revision).await.map_err(crate::reason);
                    Msg::PtyScreen { session, job, viewport, epoch, reply }
                }
                Request::PtyInput { session, job, text, keys } => Msg::Input(api.pty_input(session, job, text, Some(keys)).await.map_err(crate::reason)),
            }
        });
    }
    fn poll(&mut self, now: u64, cx: &mut Cx<'_, '_, Msg>) {
        if let Some(Menu::Terminal(view)) = &mut self.menu && let Some(request) = view.poll(now) { Self::send(request, cx); }
    }
    pub(crate) fn interactive(&self) -> bool { matches!(&self.menu, Some(Menu::Terminal(view)) if view.interactive) }
    fn focus(&self, cx: &mut Cx<'_, '_, Msg>) {
        if self.menu.is_none() { cx.close(); }
        else { cx.focus(if self.interactive() { FocusClass::Interactive } else { FocusClass::Modal }); }
    }
    fn key(&mut self, key: &KeyEvent, cx: &mut Cx<'_, '_, Msg>) {
        match &mut self.menu {
            Some(Menu::Terminals { items, cursor }) => match key.code {
                KeyCode::Up => *cursor = cursor.saturating_sub(1),
                KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
                KeyCode::Esc => self.menu = None,
                KeyCode::Char('r') => if let Some(session) = cx.core.session { Self::send(Request::ListTerminals(session), cx); },
                KeyCode::Enter => if let Some(terminal) = items.get(*cursor) {
                    self.epoch += 1;
                    self.menu = Some(Menu::Terminal(Box::new(TerminalView::new(terminal, self.epoch))));
                    self.poll(now_ms(), cx);
                },
                _ => {}
            },
            Some(Menu::Terminal(view)) if view.interactive => {
                if key.code == KeyCode::Char(']') && key.modifiers.contains(KeyModifiers::CONTROL) { view.interactive = false; }
                else if let Some((text, keys)) = key_input(key.code, key.modifiers) {
                    Self::send(Request::PtyInput { session: view.session, job: view.job.clone(), text, keys }, cx);
                    view.dirty = true;
                    self.poll(now_ms(), cx);
                }
            }
            Some(Menu::Terminal(view)) => {
                match key.code {
                    KeyCode::Up => view.scroll(-1),
                    KeyCode::Down => view.scroll(1),
                    KeyCode::PageUp => view.scroll(-i64::from(TERMINAL_PAGE)),
                    KeyCode::PageDown => view.scroll(i64::from(TERMINAL_PAGE)),
                    KeyCode::Left => view.col = view.col.saturating_sub(DIFF_STEP),
                    KeyCode::Right => {
                        let cols = view.page.as_ref().map_or(120, |page| usize::from(page.cols));
                        view.col = (view.col + DIFF_STEP).min(cols.saturating_sub(DIFF_STEP));
                    }
                    KeyCode::Char('r') => { view.revision = None; view.dirty = true; view.next_poll = 0; }
                    KeyCode::Char('i') | KeyCode::Enter if view.page.as_ref().is_some_and(|page| page.exit.is_none()) => {
                        view.interactive = true; view.set_viewport(PtyViewport::Live);
                    }
                    KeyCode::Esc => { self.menu = None; self.focus(cx); return; }
                    _ => {}
                }
                self.poll(now_ms(), cx);
            }
            None => return,
        }
        self.focus(cx);
    }
}

impl Component for Terminals {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "terminals", interests: &[], slash: &[("/terminals", "", "PTY jobs of this session's tree (picker: ⏎ opens a read-only observer)")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Terminals(Ok(_)) | Msg::PtyScreen { .. })) {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Slash { .. } => match cx.core.session {
                Some(session) => Self::send(Request::ListTerminals(session), cx),
                None => cx.core(CoreAction::Notice("no session".into())),
            },
            Input::Message(Msg::Terminals(Ok(items))) => {
                if items.is_empty() {
                    let text = "no terminals in this session's tree".to_owned();
                    cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text));
                } else {
                    for item in &items { cx.plain(Plain::Line(terminal_row(item))); }
                    self.menu = Some(Menu::Terminals { items, cursor: 0 }); self.focus(cx);
                }
            }
            Input::Message(Msg::Terminals(Err(error)) | Msg::Input(Err(error))) => cx.error(error),
            Input::Message(Msg::PtyScreen { session, job, viewport, epoch, reply }) => {
                match &reply {
                    Ok(PtyScreenReply::Updated(page)) => for row in &page.lines {
                        let mut text: String = row.iter().flat_map(|run| run.cells.iter()).filter(|cell| !cell.continuation).map(|cell| cell.text.as_str()).collect();
                        text.truncate(text.trim_end().len());
                        cx.plain(Plain::Line(text));
                    },
                    Ok(PtyScreenReply::Unavailable) => cx.plain(Plain::Line("No longer available.".into())),
                    Err(error) => cx.plain(Plain::Line(format!("terminal: {error}"))),
                    _ => {}
                }
                if let Some(Menu::Terminal(view)) = &mut self.menu { view.reply(session, &job, viewport, epoch, reply); }
                self.focus(cx);
            }
            Input::Key(key) if self.menu.is_some() => { cx.consume_key(); self.key(key, cx); }
            Input::Paste(text) if self.interactive() => {
                if let Some(Menu::Terminal(view)) = &mut self.menu {
                    Self::send(Request::PtyInput { session: view.session, job: view.job.clone(), text: None, keys: vec![Key::Paste(text.into())] }, cx);
                    view.dirty = true; cx.consume_key();
                }
            }
            Input::Tick { now_ms, .. } => self.poll(now_ms, cx),
            Input::Lifecycle(Lifecycle::Mount) => self.focus(cx),
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::EngineChanged | Lifecycle::FocusLost) => { self.menu = None; self.epoch += 1; cx.close(); }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<super::View> {
        if !matches!(at, Surface::Modal) { return None; }
        let width = u16::try_from(cx.width).unwrap_or(u16::MAX);
        let height = u16::try_from(cx.height).unwrap_or(u16::MAX);
        let inner = cx.width.saturating_sub(5).max(8);
        let (title, body, rows) = match self.menu.as_ref()? {
            Menu::Terminals { items, cursor } => {
                let title = "terminals  ⏎ observe (read-only)  r refresh  esc close".to_owned();
                return Some(super::View::Element {
                    rows: items.len().min(10) + 3, replace: false,
                    element: crate::ui::menu_view(title, items.iter().map(terminal_row).collect(), *cursor, width, height, false),
                });
            }
            Menu::Terminal(view) => {
                let budget = usize::from(height).saturating_sub(10).max(5);
                let mut body = Vec::new();
                match &view.page {
                    None => body.push(element!(Text(content: if view.unavailable { "" } else { "loading…" }, color: crate::ui::SAND)).into_any()),
                    Some(page) => for (index, row) in page.lines.iter().take(budget).enumerate() {
                        let cursor = page.cursor.filter(|(r, _)| usize::from(*r) == index).map(|(_, c)| usize::from(c));
                        body.push(terminal_line(row, view.col, inner, cursor));
                    },
                }
                if let Some(status) = view.status() { body.push(element!(Text(content: crate::search::fit(&status, inner), color: crate::ui::OCHRE, wrap: TextWrap::NoWrap)).into_any()); }
                let rows = body.len() + 3;
                (crate::search::fit(&view.title(), inner), body, rows)
            }
        };
        Some(super::View::Element { rows, replace: false, element: element! {
            BoxView(border_style: BorderStyle::Round, border_color: crate::ui::OCHRE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1, width) {
                Text(content: title, color: crate::ui::SAND, wrap: TextWrap::NoWrap)
                #(body)
            }
        }.into_any() })
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Interactive && self.interactive() }
    fn wake(&self) -> Option<u64> { match &self.menu { Some(Menu::Terminal(view)) => view.wake(), _ => None } }
}

/// One page row: cells from column `skip` on, at most `fit` columns, each
/// run's style mapped to the toolkit's; the cursor column inverted.
pub(crate) fn terminal_line(
    row: &[tools::StyledRun],
    skip: usize,
    fit: usize,
    cursor: Option<usize>,
) -> AnyElement<'static> {
    let mut contents: Vec<MixedTextContent> = Vec::new();
    let mut col = 0usize;
    for run in row {
        let mut text = String::new();
        for cell in &run.cells {
            let here = col;
            col += 1;
            if here < skip || here >= skip + fit {
                continue;
            }
            // A wide glyph cut in half by the pan shows as a blank column.
            if cell.continuation && here == skip {
                text.push(' ');
                continue;
            }
            if cursor == Some(here) {
                if !text.is_empty() {
                    contents.push(styled(
                        &run.style,
                        MixedTextContent::new(std::mem::take(&mut text)),
                    ));
                }
                let glyph = if cell.text.is_empty() {
                    " "
                } else {
                    cell.text.as_str()
                };
                contents.push(styled(&run.style, MixedTextContent::new(glyph)).invert());
                continue;
            }
            text.push_str(&cell.text);
        }
        if !text.is_empty() {
            contents.push(styled(&run.style, MixedTextContent::new(text)));
        }
    }
    if contents.is_empty() {
        contents.push(MixedTextContent::new(" "));
    }
    element!(MixedText(contents, wrap: TextWrap::NoWrap)).into_any()
}

/// A PTY cell style on a text run: foreground colour, weight, italic,
/// underline, inverse. Background colour has no toolkit counterpart.
fn styled(s: &tools::Style, mut c: MixedTextContent) -> MixedTextContent {
    match s.fg {
        tools::Color::Default => {}
        tools::Color::Idx { idx } => c = c.color(Color::AnsiValue(idx)),
        tools::Color::Rgb { r, g, b } => c = c.color(Color::Rgb { r, g, b }),
    }
    if s.bold {
        c = c.weight(Weight::Bold);
    } else if s.dim {
        c = c.weight(Weight::Light);
    }
    if s.italic {
        c = c.italic();
    }
    if s.underline {
        c = c.decoration(TextDecoration::Underline);
    }
    if s.inverse {
        c = c.invert();
    }
    c
}
