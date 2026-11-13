//! Fullscreen iocraft app. Pure: reads [`UiEvent`]s from a channel, writes
//! [`Command`]s to another. All actor traffic lives in `lib.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use agent::CompactStyle;
use iocraft::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tools::{Decision, Risk};

use crate::clip;
use crate::components::{FocusClass, Lifecycle, Surface};
use crate::model::{
    Grouped, session_row,
};
use crate::model::{
    AcpRow, CardKind, Command, Entry, Level, Menu, MenuKey, Model, Prompt,
    TurnAction, UiEvent, describe,
    now_ms, result_line, window,
};
use crate::pathac;

/// Channels handed to the app through iocraft context (owned so the receiver
/// can be taken out on first render, per design doc §11.5).
pub struct Links {
    /// Taken by the first render into `use_future`.
    pub events: Option<UnboundedReceiver<UiEvent>>,
    /// Command sink.
    pub commands: UnboundedSender<Command>,
    /// `Info.default_model` (config as fallback): what the header shows before the first turn.
    pub default_model: String,
    /// `Info.default_effort` (config as fallback, 83 §3): the draft header's effort.
    pub default_effort: Option<String>,
    /// `tools.approvals` from config.
    pub approvals: bool,
    /// Boot-configured owners, taken by the first state initializer.
    pub components: Option<crate::components::Registry>,
    /// Context limits for the header.
    pub catalog: Arc<models::Catalog>,
    /// Session to `/resume` on first render.
    pub resume: Option<runtime::SessionId>,
    /// For `<data_dir>/attachments/` (17 §4).
    pub data_dir: PathBuf,
}

/// Rows a menu shows at most.
const MENU_ROWS: usize = 10;
/// Rows the board shows at most; the rest fold into `… n more (/agents)`.
const BOARD_ROWS: usize = 6;
// theuth dark palette (docs/design/25-desktop.md §2.4), truecolor. The terminal's own
// background stays; primary text is left uncoloured so it follows the terminal's fg.
// ponytail: assumes a dark terminal; query OSC 11 and swap to the light set if that hurts.
const fn rgb(hex: u32) -> Color {
    Color::Rgb {
        r: (hex >> 16) as u8,
        g: (hex >> 8) as u8,
        b: hex as u8,
    }
}
/// Highlighted rows, focused labels.
pub(crate) const BONE: Color = rgb(0xE8DCC8);
/// Secondary text.
pub(crate) const BONE_DIM: Color = rgb(0xC9B89A);
/// Tertiary text, hints, quiet borders.
pub(crate) const SAND: Color = rgb(0x9A8B72);
/// Accent: focus borders, headings, hints, the human's turn.
pub(crate) const OCHRE: Color = rgb(0xC47A2C);
/// Success / done.
pub(crate) const MALACHITE: Color = rgb(0x4A8A6E);
/// Info / agent state: tools, peers, asks.
pub(crate) const FAIENCE: Color = rgb(0x5B8F8A);
/// Error / stop.
pub(crate) const CARNELIAN: Color = rgb(0xC45A48);
/// Human turn cards.
const USER: Color = OCHRE;
/// Tool call cards.
pub(crate) const TOOL: Color = FAIENCE;
/// Peer (subagent / parent) message cards.
const PEER: Color = FAIENCE;
/// Markdown headings in assistant text.
const HEADING: Color = OCHRE;
/// Skill body cards.
const SKILL: Color = OCHRE;
/// System prompt cards.
const PROMPT: Color = SAND;
/// Finished background job cards.
const JOB: Color = MALACHITE;

/// Run the fullscreen UI until the user quits. Mouse capture stays off so
/// the terminal's own selection (and copy-on-select) keeps working. Ctrl+C
/// reaches the app (`ignore_ctrl_c`) so quitting always runs the terminal
/// teardown in `App` — iocraft's own exit skipped it.
pub async fn run(links: Links) -> std::io::Result<()> {
    element! {
        ContextProvider(value: Context::owned(links)) {
            App
        }
    }
    .fullscreen()
    .disable_mouse_capture()
    .ignore_ctrl_c()
    .await
}

#[component]
fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut links = hooks.use_context_mut::<Links>();
    let default_model = links.default_model.clone();
    let default_effort = links.default_effort.clone();
    let approvals = links.approvals;
    let catalog = links.catalog.clone();
    let component_commands = links.commands.clone();
    let mut model = hooks.use_state({
        let catalog = catalog.clone();
        let components = links.components.take();
        move || {
            let mut m = Model::with_components(default_model, approvals, components.expect("first render owns the registry"));
            m.effort = default_effort.clone();
            m.default_effort = default_effort;
            m.catalog = Some(catalog);
            m.mount_components(&component_commands);
            m.entries.push(Entry::Info(
                concat!("theuth ", env!("CARGO_PKG_VERSION")).into(),
            ));
            m
        }
    });
    let mut input = hooks.use_state(String::new);
    let mut component_rows = hooks.use_ref_default::<usize>();
    let mut handle = hooks.use_ref_default::<TextInputHandle>();
    // Separate owner input never touches the composer's draft, cursor or paste tokens.
    let mut separate_handle = hooks.use_ref_default::<TextInputHandle>();
    let commands = links.commands.clone();
    let data_dir = links.data_dir.clone();

    // Runs once: the receiver is `Some` only on the first render.
    let events = links.events.take();
    let resume = links.resume.take();
    // Events and animation share one 60 Hz loop. iocraft renders the whole
    // tree on every `model.write()`, so applying events as they arrive made
    // a streaming turn (and every streaming subagent) render per token —
    // hundreds of frames a second, keys queued behind them. Now a frame
    // drains everything pending under one write; idle frames touch nothing
    // (`read` schedules no render).
    hooks.use_future({
        let commands = commands.clone();
        async move {
            let Some(mut events) = events else { return };
            // Kitty keyboard protocol: without it Shift+Enter is a plain `\r`.
            // Pushed after iocraft entered the alternate screen (separate stack).
            // Bracketed paste: a paste arrives as one `TerminalEvent::BracketedPaste`
            // instead of a key per character.
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PushKeyboardEnhancementFlags(
                    crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                ),
                crossterm::event::EnableBracketedPaste
            );
            if let Some(id) = resume {
                model.write().resume(id, &commands);
            }
            // Skill names so `/<skill> [args]` resolves before `/skills` was ever opened.
            let _ = commands.send(Command::ListSkills);
            let mut frames = tokio::time::interval(std::time::Duration::from_millis(16));
            frames.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut pending = Vec::new();
            loop {
                frames.tick().await;
                loop {
                    match events.try_recv() {
                        Ok(ev) => pending.push(ev),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
                    }
                }
                // Owner deadlines and core editor helpers wake the idle loop.
                let now = now_ms();
                let due = model.read().path.wake().is_some_and(|t| t <= now);
                let due = due || model.read().components.wake().is_some_and(|at| at <= now);
                if pending.is_empty() && !model.read().animating() && !due {
                    continue;
                }
                let mut m = model.write();
                // Assistant text renders at the width current when it starts (30 §7).
                let (width, height) = crossterm::terminal::size().unwrap_or((0, 0));
                m.width = usize::from(width);
                m.terminal_height = usize::from(height);
                for ev in pending.drain(..) {
                    m.apply(ev);
                }
                if m.animating() {
                    m.tick();
                }
                m.component_tick(now);
                let session = m.session;
                m.path.tick(now, session, &commands);
                // A withdrawn or unsent message comes back into the editor,
                // above anything typed since it left.
                if let Some(text) = m.recall.take() {
                    let typed = std::mem::take(&mut *input.write());
                    input.set(if typed.is_empty() {
                        text
                    } else {
                        format!("{text}\n{typed}")
                    });
                }
                // A dictation ended: the spliced value, then the caret in the
                // same tick (an external value change alone sends it to the end).
                if let Some((value, caret)) = m.pending_input.take() {
                    input.set(value);
                    handle.write().set_cursor_offset(caret);
                }
                if let Some(caret) = m.widget_cursor.take() {
                    separate_handle.write().set_cursor_offset(caret);
                }
                prepend_editor(&mut m, input);
                // "send it" (153 §7.4): the spliced value takes the send key's path, once.
                if let Some(text) = m.pending_send.take() {
                    m.submit(&text, &commands);
                    input.set(String::new());
                }
                // A context edit or rewind landed (48 §5): the settled transcript.
                if std::mem::take(&mut m.refresh_context)
                    && let Some(session) = m.session
                {
                    let _ = commands.send(Command::Load {
                        session,
                        settled_only: true,
                    });
                }
                // The engine behind the calls changed (63 §2.3): the shown
                // session and its subtree, loaded from the new one.
                if std::mem::take(&mut m.reselect)
                    && let Some(session) = m.session
                {
                    let _ = commands.send(Command::Load {
                        session,
                        settled_only: false,
                    });
                    let _ = commands.send(Command::ListAgents(session));
                }
                // Root metadata moved while a grouped picker is open (51 §3): re-list it.
                if std::mem::take(&mut m.refresh_grouping) {
                    match (&m.menu, m.session) {
                        (Some(Menu::Sessions { .. }), _) => {
                            let _ = commands.send(Command::ListSessions);
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    let terminal_size = hooks.use_terminal_size();
    hooks.use_terminal_events({
        let commands = commands.clone();
        move |event| {
            let mut m = model.write();
            m.width = usize::from(terminal_size.0);
            m.terminal_height = usize::from(terminal_size.1);
            // At the cursor; `TextInput` sees an insertion there and moves
            // its cursor past it. `State`/`Ref` are `Copy`: `move` leaves
            // `input` free for the key handling below.
            let mut insert_at_cursor = move |m: &mut Model, s: &str| {
                let mut v = input.read().to_string();
                let at = handle.read().cursor_offset().min(v.len());
                v.insert_str(at, s);
                m.sync_slash(&v);
                input.set(v);
            };
            if let TerminalEvent::Key(key) = &event
                && key.kind != KeyEventKind::Release
                && m.component_key(FocusClass::Interactive, key)
            { return; }
            let TerminalEvent::Key(key) = event
            else {
                if let TerminalEvent::BracketedPaste(text) = event {
                    if m.component_paste(&text, separate_handle.read().cursor_offset()) { return; }
                    if m.input_focused() {
                        // A dropped image path attaches the file (17 §4.2).
                        let s = match clip::dropped_file(&text) {
                            Some(img) => m.attach(img),
                            None => m.paste(&text),
                        };
                        insert_at_cursor(&mut m, &s);
                    }
                }
                return;
            };
            let KeyEvent { code, kind, modifiers, .. } = key;
            if kind == KeyEventKind::Release {
                return;
            }
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                m.quit = true;
                return;
            }
            if m.component_key_with_editor(FocusClass::Spoken, &key, &input.read(), handle.read().cursor_offset()) { return; }
            // Chat-only: a search menu on top must not toggle transcript cards.
            if code == KeyCode::Char('o')
                && modifiers.contains(KeyModifiers::CONTROL)
                && !m.components.separate_editor()
            {
                m.toggle_card();
                return;
            }
            if let Some(req) = m.approval() {
                let (session, child) = (req.session, Some(req.session) != m.session);
                match code {
                    KeyCode::Char('y') => m.decide(Decision::Allow, &commands),
                    KeyCode::Char('n') | KeyCode::Esc => m.decide(Decision::Deny, &commands),
                    // Read what led to a child's request; the modal stays up (19 §3.4).
                    KeyCode::Char('o') if child => m.resume(session, &commands),
                    _ => {}
                }
                return;
            }
            if let Some((FocusClass::Secret, editor)) = m.components.editor() {
                match code {
                    KeyCode::Enter => {
                        if editor.submit_empty || !input.read().trim().is_empty() {
                            let line = std::mem::take(&mut *input.write());
                            m.component_submit(line.trim());
                        }
                    }
                    KeyCode::Esc => {
                        input.set(String::new());
                        m.component_key(FocusClass::Secret, &key);
                    }
                    _ => {}
                }
                return;
            }
            if m.component_key(FocusClass::Secret, &key) { return; }
            if m.component_key(FocusClass::Suggestion, &key) { return; }
            if m.ask().is_some() {
                let text = input.read().to_string();
                if let Some(text) = m.ask_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            let captured = m.components.captures(FocusClass::Pinned);
            if m.component_key(FocusClass::Pinned, &key) { return; }
            // TextInput saw this key unfocused; a yielding capture returns typing to it.
            if captured && !m.components.captures(FocusClass::Pinned)
                && let KeyCode::Char(c) = code
                && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                insert_at_cursor(&mut m, &c.to_string());
                return;
            }
            let (w, h) = terminal_size;
            // Shift+Enter: the input inserts the newline itself.
            if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
                return;
            }
            // Alt+Up: pull the newest queued steer back into the editor (not through search).
            if code == KeyCode::Up && modifiers.contains(KeyModifiers::ALT) && !m.components.separate_editor() {
                m.withdraw(input.read().is_empty(), &commands);
                return;
            }
            // Ctrl+V: the OS clipboard image (bracketed paste carries text only).
            if code == KeyCode::Char('v')
                && modifiers.contains(KeyModifiers::CONTROL)
                && m.input_focused()
            {
                match clip::image(&data_dir) {
                    Ok(Some(img)) => {
                        let s = m.attach(img);
                        insert_at_cursor(&mut m, &s);
                    }
                    Ok(None) => {}
                    Err(e) => m.entries.push(Entry::Error(format!("paste: {e:#}"))),
                }
                return;
            }
            let view = view_rows(&m, h, &input.read(), *component_rows.read());
            m.width = usize::from(w);
            m.viewport = view;
            // `/search` (49 §6.1): its own focus ring; the query editor's own
            // keys reach the search `TextInput`, everything else lands here.
            if m.component_key(FocusClass::Search, &key) {
                prepend_editor(&mut m, input);
                return;
            }
            // Path autocomplete (PATH-AC): Tab on a path token opens the list
            // under the editor; the slash popup's Tab wins while it has matches.
            if m.input_focused() {
                let text = input.read().to_string();
                let cursor = handle.read().cursor_offset().min(text.len());
                let slash_busy = matches!(m.menu, Some(Menu::Slash { .. }))
                    && !m
                        .slash_rows(text.split(' ').next().unwrap_or_default())
                        .is_empty();
                let session = m.session;
                match m
                    .path
                    .key(code, &text, cursor, slash_busy, session, &commands)
                {
                    pathac::Key::Pass => {}
                    pathac::Key::Consumed => return,
                    pathac::Key::Input(text, caret) => {
                        m.sync_slash(&text);
                        m.pending_input = Some((text, caret));
                        return;
                    }
                }
            }
            if m.component_key(FocusClass::Modal, &key) { return; }
            if m.menu.is_some() {
                let text = input.read().to_string();
                if let MenuKey::Input(text) = m.menu_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            if m.component_key(FocusClass::Content, &key) { return; }
            // 141 §10.1: a watch-only child has no editor. `/` opens it for one
            // slash command (`/parent`, `/agents`, … — the way out); anything
            // else that would have been typed gets the read-only hint instead.
            if m.observed.is_some()
                && !m.input_focused()
                && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                match code {
                    KeyCode::Char('/') => {
                        m.watch_slash = true;
                        insert_at_cursor(&mut m, "/");
                        return;
                    }
                    KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Tab => {
                        m.watch_swallow();
                        return;
                    }
                    _ => {}
                }
            }
            // Owners open the newest available entry at the existing empty-editor shortcut.
            if code == KeyCode::Char('o')
                && modifiers.is_empty()
                && m.prompt.is_none()
                && input.read().is_empty()
            {
                if m.component_open_latest_entry() { return; }
            }
            // Transcript scrolling. ↑/↓ are the editor's while it spans
            // lines; terminals without mouse capture send them for the wheel.
            let step = match code {
                KeyCode::PageUp => Some(-(view as isize)),
                KeyCode::PageDown => Some(view as isize),
                KeyCode::Up if !input.read().contains('\n') => Some(-1),
                KeyCode::Down if !input.read().contains('\n') => Some(1),
                _ => None,
            };
            if let Some(step) = step {
                m.scroll(step, view, usize::from(w));
                return;
            }
            match code {
                KeyCode::Enter => {
                    let line = std::mem::take(&mut *input.write());
                    // Ctrl/Cmd+Enter (kitty-protocol terminals report both): interject instead of steer.
                    if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) {
                        m.submit_now(&line, &commands);
                    } else {
                        m.submit(&line, &commands);
                    }
                }
                KeyCode::Esc if m.prompt.is_some() => m.cancel_prompt(),
                // Cleared here, not via `recall`: the event loop only drains
                // that when events arrive or the model animates, and a stale
                // `recall` would later wipe whatever gets typed next.
                KeyCode::Esc if m.cancel_edit() => {
                    m.recall = None;
                    input.set(String::new());
                }
                KeyCode::Esc => m.cancel(&commands),
                _ => {}
            }
        }
    });

    if model.read().quit { model.write().component_notify(Lifecycle::Shutdown); }
    let m = model.read();
    if m.quit {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags,
            crossterm::event::DisableBracketedPaste
        );
        system.exit();
    }
    let modal = m.approval().map(|req| {
        let agent = m
            .peer_name(req.session)
            .map_or_else(String::new, |n| format!("  [agent: {n}]"));
        let open = if Some(req.session) == m.session { "" } else { "  [o] open" };
        let chip = m.component_text(Surface::PromptBadge);
        let chip = (!chip.is_empty()).then_some(chip);
        element! {
            View(border_style: BorderStyle::Double, border_color: OCHRE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: format!("approval: {}  ({}){agent}", req.tool, risk_name(req.risk)), weight: Weight::Bold)
                #(describe(&req.tool, &req.args).into_iter().map(|l| element!(Text(content: l, color: FAIENCE, wrap: TextWrap::Wrap))))
                #(chip.map(|c| element!(Text(content: c, color: SAND, italic: true))))
                Text(content: format!("[y] allow  [n/esc] deny{open}"), color: OCHRE)
            }
        }
        .into_any()
    });
    // Approval on top, then a secret request, then the model's questions;
    // otherwise whichever menu is open.
    let (width, height) = terminal_size;
    let pinned = component_slot(&m, Surface::Pinned, width, height, None);
    let composer = component_slot(&m, Surface::Composer, width, height, None);
    let composer_header = component_slot(&m, Surface::ComposerHeader, width, height, None);
    let composer_mode = m.components.composer_mode();
    let hint = component_slot(&m, Surface::Hint, width, height, None);
    let badge = component_slot(&m, Surface::PromptBadge, width, height, None);
    // The composer body/hint replace the existing editor presentation, not its viewport budget.
    let extra_rows = pinned.rows + badge.rows;
    *component_rows.write() = extra_rows;
    let modal = modal
        .or_else(|| component_slot(&m, Surface::Prompt(FocusClass::Secret), width, height, None).element())

        .or_else(|| ask_box(&m))

        .or_else(|| menu(&m, &input.read(), width, height))
        .or_else(|| {
            let budget = if m.components.separate_editor() {
                u16::try_from(view_rows(&m, height, &input.read(), extra_rows)).unwrap_or(u16::MAX)
            } else { height };
            component_slot(&m, Surface::Modal, width, budget, Some(separate_handle)).element()
        });
    // A secret's value (or a provider's key) shows as dots.
    let component_editor = m.components.editor();
    let mask = (m.prompt.as_ref().is_some_and(Prompt::masked)
        || component_editor.as_ref().is_some_and(|(_, editor)| editor.masked))
        .then_some('•');
    let has_focus = m.input_focused();
    let (header, level) = m.header(&catalog);
    let header_color = match level {
        Level::Normal => SAND,
        Level::Warn => OCHRE,
        Level::Danger => CARNELIAN,
    };

    // Only the slice that can reach the screen is built; see `Model::window`.
    let view = view_rows(
        &m,
        height,
        &input.read(),
        extra_rows,
    );
    let win = m.window(view, usize::from(width));
    let entries = m.entries[win.from..win.to]
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let decorations = e.key().map(|key| m.component_views(Surface::Entry { key, skipped: 0 },
                usize::from(width), m.entry_height(e, usize::from(width)))).unwrap_or_default();
            entry(e, m.spinner(), if i == 0 { win.skip } else { 0 }, width, decorations, m.entry_summary(e))
        });
    let transcript = if m.anchor.is_some() {
        // Anchored: the first `top` rows of the first entry sit above the fold.
        element! {
            View(overflow: Overflow::Hidden, width: 100pct, height: 100pct) {
                View(position: Position::Absolute, top: -(win.top as i32), width: 100pct, flex_direction: FlexDirection::Column) {
                    #(entries)
                }
            }
        }
        .into_any()
    } else {
        // Pinned: bottom-aligned, so an over-tall slice is clipped at the top.
        element! {
            View(overflow: Overflow::Hidden, width: 100pct, height: 100pct, flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd) {
                #(entries)
            }
        }
        .into_any()
    };
    let content = component_slot(&m, Surface::Content, width, u16::try_from(view).unwrap_or(u16::MAX), None);
    let transcript = if content.replace {
        content.element().unwrap_or(transcript)
    } else if let Some(extra) = content.element() {
        element!(View(flex_direction: FlexDirection::Column, height: 100pct) {
            #(transcript)
            #(extra)
        }).into_any()
    } else { transcript };
    element! {
        View(flex_direction: FlexDirection::Column, width, height) {
            View(flex_grow: 1.0_f32, min_height: 0, flex_direction: FlexDirection::Column) {
                #(transcript)
            }
            // Activity and steers not yet in the context sit under the
            // transcript, in view whatever you scrolled to.
            #(m.waiting().then(|| element!(View(margin_top: 1) {
                Text(content: format!("{} thinking…", m.spinner()), color: SAND, italic: true)
            })))
            #(m.queued.iter().map(|q| element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: SAND, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: "◇ queued · ⌥↑ edit", color: SAND, weight: Weight::Bold)
                    Text(content: queued_text(&q.text), color: SAND, wrap: TextWrap::Wrap)
                }
            }))
            #(board(&m, width))
            #(pinned.element())
            #(modal)
            #(badge.element())
            #(m.prompt.as_ref().map(|p| {
                element!(View(height: 1, padding_left: 1) {
                    Text(content: p.hint(), color: OCHRE)
                })
            }))
            #(component_editor.as_ref().filter(|(_, editor)| !editor.hint.is_empty()).map(|(_, editor)| element!(
                View(height: 1, padding_left: 1) {
                    Text(content: editor.hint.clone(), color: OCHRE)
                }
            )))
            View(height: 1, padding_left: 1) {
                #(composer_header.element().unwrap_or_else(|| element!(Text(content: header, color: header_color)).into_any()))
            }
            // `flex_shrink: 0`: once the transcript fills the screen the box
            // would otherwise be squeezed back to `min_height` as it grows.
            // Keep the editor mounted while an owner supplies a read-only or
            // hidden region: its cursor and restore handle must survive.
            // Never hide a bordered view itself; iocraft would draw width 0 - 2.
            View(border_style: if composer_mode == crate::components::ComposerMode::Hidden { BorderStyle::None } else { BorderStyle::Round }, border_color: if composer_mode == crate::components::ComposerMode::ReadOnly { FAIENCE } else { OCHRE }, min_height: if composer_mode == crate::components::ComposerMode::Hidden { 0 } else { 3 }, flex_shrink: 0.0, flex_direction: FlexDirection::Column) {
                View(width: 100pct, display: if composer_mode != crate::components::ComposerMode::Default { Display::None } else { Display::Flex }) {
                    TextInput(
                        handle,
                        has_focus,
                        value: input.to_string(),
                        multiline: true,
                        auto_grow: true,
                        newline_modifiers: KeyModifiers::SHIFT,
                        mask,
                        ignore_paste: true,
                        on_change: move |v: String| {
                            let mut m = model.write();
                            m.sync_slash(&v);
                            m.component_notify(Lifecycle::ComposerEdited(&v));
                            // `TextInput` reports the new text before its cursor state moves,
                            // so the caret is read off the edit itself: the end of the
                            // inserted run, or the point of deletion.
                            let caret = pathac::caret_after_edit(&input.read(), &v);
                            m.path.edit(&v, caret, now_ms());
                            drop(m);
                            input.set(v);
                        },
                    )
                }
                #(composer.element())
            }
            #(hint.element())
            #(path_box(&m, width))
            View(height: 1, padding_left: 1, padding_right: 1, justify_content: JustifyContent::SpaceBetween) {
                View() {
                    Text(content: m.status(), color: SAND)
                    #(component_slot(&m, Surface::Status(crate::components::StatusSlot::AfterCore), width, 1, None).elements)
                }
                Text(content: m.component_text(Surface::Status(crate::components::StatusSlot::Right)), color: SAND)
            }
        }
    }
}

/// A slot is measured once from exactly the owned elements that are rendered.
struct ComponentSlot {
    elements: Vec<AnyElement<'static>>,
    rows: usize,
    replace: bool,
}
impl ComponentSlot {
    fn element(self) -> Option<AnyElement<'static>> {
        if self.elements.is_empty() { return None; }
        Some(element!(View(flex_direction: FlexDirection::Column, height: self.rows as u32, flex_shrink: 0.0) {
            #(self.elements)
        }).into_any())
    }
}
fn prepend_editor(m: &mut Model, mut input: State<String>) {
    for prefix in std::mem::take(&mut m.editor_prefixes) {
        let value = input.read().to_string();
        let value = if value.is_empty() { prefix } else { format!("{prefix} {value}") };
        m.sync_slash(&value);
        input.set(value);
    }
}
fn component_slot(m: &Model, at: Surface<'_>, width: u16, height: u16, handle: Option<Ref<TextInputHandle>>) -> ComponentSlot {
    let mut slot = ComponentSlot { elements: Vec::new(), rows: 0, replace: false };
    for view in m.component_views(at, usize::from(width), usize::from(height)) {
        let (element, rows, replace) = match view {
            crate::components::View::Text(text) => {
                let rows = text.lines().count();
                (element!(Text(content: text, wrap: TextWrap::NoWrap)).into_any(), rows, false)
            }
            crate::components::View::Element { element, rows, replace } => (element, rows, replace),
            crate::components::View::Editor { editor, render, rows, replace } => {
                let handle = handle.expect("separate editor must render in its shell slot");
                let mut on_change = editor.on_change;
                let input = element!(TextInput(
                    handle, has_focus: editor.focused, value: editor.value, ignore_paste: true,
                    on_change: move |value| on_change(value),
                )).into_any();
                (render(input), rows, replace)
            }
        };
        if replace { slot.elements.clear(); slot.rows = 0; slot.replace = true; }
        slot.rows += rows;
        slot.elements.push(element!(View(height: rows as u32, flex_shrink: 0.0) { #(element) }).into_any());
    }
    slot
}




/// The shown `ask_user` question: options with the cursor, a "type your own"
/// row, and the keys. `None` without one.
fn ask_box(m: &Model) -> Option<AnyElement<'static>> {
    let a = m.ask()?;
    let q = a.question();
    let agent = m
        .peer_name(a.req.session)
        .map_or_else(String::new, |n| format!("  [agent: {n}]"));
    let title = format!(
        "question {}/{}{agent}",
        a.current + 1,
        a.req.questions.len()
    );
    let n = q.options.len();
    let rows = q.options.iter().enumerate().map(|(i, c)| {
        let here = a.cursor == i && !a.typing;
        let rec = if q.recommended == Some(i) { "  (recommended)" } else { "" };
        let desc = c
            .description
            .as_deref()
            .map_or_else(String::new, |d| format!(" — {d}"));
        element! {
            Text(
                content: format!("{} {}. {}{desc}{rec}", if here { "›" } else { " " }, i + 1, c.label),
                color: if here { BONE } else { BONE_DIM },
                weight: if here { Weight::Bold } else { Weight::Normal },
                wrap: TextWrap::Wrap,
            )
        }
    });
    let own = if a.typing {
        "› type your answer below   [⏎] send  [esc] back".to_owned()
    } else {
        format!(
            "{}    type your own…",
            if a.cursor == n { "›" } else { " " }
        )
    };
    let keys = if a.typing {
        String::new()
    } else {
        let pick = if n > 0 {
            "[1-9/⏎] pick  "
        } else {
            "[⏎] type  "
        };
        format!("[↑↓] move  {pick}[s] skip  [esc] dismiss (ends the turn)")
    };
    let chip = m.component_text(Surface::PromptBadge);
    let chip = (!chip.is_empty()).then_some(chip);
    Some(
        element! {
            View(border_style: BorderStyle::Double, border_color: FAIENCE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: title, weight: Weight::Bold)
                Text(content: q.question.clone(), color: FAIENCE, wrap: TextWrap::Wrap)
                #(rows)
                Text(content: own, color: if a.cursor == n || a.typing { BONE } else { BONE_DIM }, weight: if a.cursor == n || a.typing { Weight::Bold } else { Weight::Normal })
                #(chip.map(|c| element!(Text(content: c, color: SAND, italic: true))))
                #((!keys.is_empty()).then(|| element!(Text(content: keys, color: FAIENCE))))
            }
        }
        .into_any(),
    )
}

/// The open menu as a bordered box; `None` without one. Rows are clipped to
/// one less than the box's inner width: a `NoWrap` row wider than the
/// terminal makes the terminal wrap the frame, and one exactly as wide makes
/// taffy hand the box several extra rows (see `menu_box_is_tight_at_any_width`).
fn menu(m: &Model, input: &str, width: u16, height: u16) -> Option<AnyElement<'static>> {
    let (title, rows, cursor): (String, Vec<String>, usize) = match m.menu.as_ref()? {
        Menu::Slash { cursor } => {
            let word = input.split(' ').next().unwrap_or_default();
            let rows = m
                .slash_rows(word)
                .into_iter()
                .map(|r| format!("{:<30} {}", format!("{} {}", r.name, r.hint), r.desc))
                .collect::<Vec<_>>();
            (
                "↑↓ move  ⇥ complete  ⏎ run  esc close".to_owned(),
                rows,
                *cursor,
            )
        }
        Menu::Sessions { items, cursor } => (
            "sessions  ⏎ switch  esc close  (the one you leave keeps running)".to_owned(),
            items
                .iter()
                .map(|row| match row {
                    Grouped::Heading(h) => h.clone(),
                    Grouped::Item((s, depth)) => {
                        session_row(s, *depth, width, Some(s.id) == m.session)
                    }
                })
                .collect(),
            *cursor,
        ),
        Menu::Turns {
            items,
            cursor,
            action,
        } => (
            format!(
                "{}  ⏎ {}  esc close",
                match action {
                    TurnAction::Edit => "edit",
                    TurnAction::Regen => "regenerate",
                    TurnAction::Fork => "fork through",
                },
                match action {
                    TurnAction::Edit => "recall",
                    TurnAction::Regen => "redo reply",
                    TurnAction::Fork => "fork",
                }
            ),
            items
                .iter()
                .enumerate()
                .map(|(i, (_, _, line))| format!("#{}  ◆ {line}", i + 1))
                .collect(),
            *cursor,
        ),
        Menu::Agents { items, cursor } => (
            "agents  ⏎ open  esc close  (the one you leave keeps running)".to_owned(),
            items
                .iter()
                .map(|a| m.board_row(a, width))
                .collect(),
            *cursor,
        ),
        Menu::Acp { cursor, .. } => (
            "acp  p probe  ⏎ add from registry  d remove".to_owned(),
            m.acp_rows()
                .iter()
                .map(|r| match r {
                    AcpRow::Entry(e) => {
                        // ponytail: fixed 40-char command; pass the terminal width if it grates.
                        let cmd: String = format!("{} {}", e.command, e.args.join(" "))
                            .chars()
                            .take(40)
                            .collect();
                        format!(
                            "{:<16} {:<40} {}",
                            e.name,
                            cmd,
                            e.status.as_deref().unwrap_or_default()
                        )
                    }
                    AcpRow::Sep => String::new(),
                    AcpRow::Registry(a) => {
                        let desc: String = a.description.chars().take(50).collect();
                        format!(
                            "+ {:<16} {:<20} {}",
                            a.id,
                            format!("{} {}", a.name, a.version),
                            desc
                        )
                    }
                })
                .collect(),
            *cursor,
        ),
        Menu::Skills { items, cursor } => (
            "skills  ⏎ use  s show  esc close   ● in context".to_owned(),
            items
                .iter()
                .map(|s| {
                    // ponytail: fixed 48-char description; pass the terminal width if it grates.
                    let desc: String = s.description.chars().take(48).collect();
                    format!(
                        "{} {:<20} {:<48} {:<16} {}",
                        if m.loaded_skills.contains(&s.name) { "●" } else { " " },
                        s.name,
                        desc,
                        s.repo
                            .as_deref()
                            .map_or_else(String::new, |r| format!("[{r}]")),
                        if s.always { "always" } else { "" }
                    )
                })
                .collect(),
            *cursor,
        ),
        Menu::Logins { items, cursor } => (
            "login  ⏎ log in  esc close".to_owned(),
            items
                .iter()
                .map(|r| {
                    format!(
                        "{:<8} {:<20} {}",
                        if r.mcp { "mcp" } else { "provider" },
                        r.name,
                        r.status
                    )
                })
                .collect(),
            *cursor,
        ),
    };
    Some(menu_view(title, rows, cursor, width, height, false))
}

/// Shared modal frame, selection window and clipping for component-owned rows.
pub(crate) fn menu_view(
    title: String,
    rows: Vec<String>,
    cursor: usize,
    width: u16,
    height: u16,
    tall: bool,
) -> AnyElement<'static> {
    let cursor = cursor.min(rows.len().saturating_sub(1));
    let max = if tall {
        usize::from(height).saturating_sub(10).max(MENU_ROWS)
    } else {
        MENU_ROWS
    };
    let shown = window(rows.len(), cursor, max);
    // border 2 + padding 2 + cursor glyph 2, then one less.
    let fit = usize::from(width).saturating_sub(7);
        element! {
            View(border_style: BorderStyle::Round, border_color: SAND, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: title, color: SAND)
                #(rows[shown.clone()].iter().enumerate().map(|(i, row)| {
                    let here = shown.start + i == cursor;
                    let color = if here { BONE } else { BONE_DIM };
                    let weight = if here { Weight::Bold } else { Weight::Normal };
                    let text: String = row.chars().take(fit).collect();
                    element!(Text(
                        content: format!("{} {text}", if here { "›" } else { " " }),
                        wrap: TextWrap::NoWrap,
                        color,
                        weight,
                    ))
                }))
            }
        }
        .into_any()
}


/// The path completion list under the editor (PATH-AC): a window of rows
/// around the highlighted one, `…` when the engine cut the listing.
fn path_box(m: &Model, width: u16) -> Option<AnyElement<'static>> {
    let popup = m.path.open.as_ref().filter(|p| !p.items.is_empty())?;
    let shown = window(popup.items.len(), popup.cursor, pathac::ROWS);
    let fit = usize::from(width).saturating_sub(7);
    let more = popup.truncated || shown.end < popup.items.len();
    Some(
        element! {
            View(border_style: BorderStyle::Round, border_color: SAND, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: "↑↓ move  ⇥ accept  esc close", color: SAND)
                #(popup.items[shown.clone()].iter().enumerate().map(|(i, c)| {
                    let here = shown.start + i == popup.cursor;
                    let text: String = c.display.chars().take(fit).collect();
                    element!(Text(
                        content: format!("{} {text}", if here { "›" } else { " " }),
                        wrap: TextWrap::NoWrap,
                        color: if here { BONE } else { BONE_DIM },
                        weight: if here { Weight::Bold } else { Weight::Normal },
                    ))
                }))
                #(more.then(|| element!(Text(content: "  …", color: SAND))))
            }
        }
        .into_any(),
    )
}


/// Rows the transcript pane gets: the terminal minus everything drawn
/// around it (header, status, editor box, prompt line, activity row, queued
/// chips, board, the todo card's `todo` rows). An estimate by the same
/// rules the layout follows; a row or two off only moves the first scroll
/// step, never the pinned bottom.
fn view_rows(m: &Model, height: u16, input: &str, todo: usize) -> usize {
    let editor = 2 + input.lines().count().max(1);
    let activity = if m.waiting() { 2 } else { 0 };
    let queued: usize = m
        .queued
        .iter()
        .map(|q| 4 + queued_text(&q.text).lines().count())
        .sum();
    let board = match m.board().len() {
        0 => 0,
        n => 3 + n.min(BOARD_ROWS) + usize::from(n > BOARD_ROWS),
    };
    usize::from(height)
        .saturating_sub(
            2 + editor
                + usize::from(m.prompt.is_some())
                + activity
                + queued
                + board
                + todo,
        )
        .max(1)
}


/// The subagent board (design doc 19 §3.3): the live rows of the current
/// session's subtree, between the transcript and the editor; `None` while
/// nothing is live. A display only: it takes no keys.
fn board(m: &Model, width: u16) -> Option<AnyElement<'static>> {
    let rows = m.board();
    if rows.is_empty() {
        return None;
    }
    let more = rows.len().saturating_sub(BOARD_ROWS);
    // border 2 + padding 2, then one less (see `menu`); glyph + name take 17.
    let fit = usize::from(width).saturating_sub(5);
    Some(
        element! {
            View(border_style: BorderStyle::Round, border_color: SAND, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: "agents", color: SAND)
                #(rows.iter().take(BOARD_ROWS).map(|r| {
                    let row = m.board_row(r, width);
                    let (glyph, rest) = split_chars(&row, 2);
                    let (name, rest) = split_chars(rest, 15);
                    element! {
                        View(flex_direction: FlexDirection::Row) {
                            Text(content: glyph.to_owned(), color: BONE_DIM, wrap: TextWrap::NoWrap)
                            Text(content: name.to_owned(), color: PEER, wrap: TextWrap::NoWrap)
                            Text(content: rest.chars().take(fit.saturating_sub(17)).collect::<String>(), color: BONE_DIM, wrap: TextWrap::NoWrap)
                        }
                    }
                }))
                #((more > 0).then(|| element!(Text(content: format!("… {more} more (/agents)"), color: SAND))))
            }
        }
        .into_any(),
    )
}


/// Split after `n` chars (not bytes): `("⠹ ", rest)`.
fn split_chars(s: &str, n: usize) -> (&str, &str) {
    s.split_at(s.char_indices().nth(n).map_or(s.len(), |(i, _)| i))
}

/// A queued input's chip text: a skill injection (04 §2.4) reads
/// `◇ <name>  <words>` rather than the whole body.
fn queued_text(text: &str) -> String {
    match agent::skills::injected(text) {
        Some((name, _, "")) => format!("◇ {name}"),
        Some((name, _, rest)) => format!("◇ {name}  {rest}"),
        None => text.to_owned(),
    }
}

/// One transcript entry. `skip` drops that many leading rendered lines of a
/// reply (`Model::window`): the part that cannot reach the screen.
fn entry(e: &Entry, spinner: &str, skip: usize, _width: u16, decorations: Vec<crate::components::View>, summary: Option<String>) -> AnyElement<'static> {
    if let Some(index) = decorations.iter().rposition(|view| matches!(view, crate::components::View::Element { replace: true, .. })) {
        return element!(View(flex_direction: FlexDirection::Column) {
            #(decorations.into_iter().skip(index).filter_map(|view| match view { crate::components::View::Element { element, .. } => Some(element), _ => None }))
        }).into_any();
    }
    match e {
        // Human turns: the same card shape as tools and peers, own colour and glyph.
        // A live-talk utterance shows its `[spoken, N s]` marker as a muted tag line.
        Entry::User { text: t, .. } => {
            let (tag, body) = match crate::attach::spoken(t) {
                Some((secs, cut, body)) => (Some(crate::attach::spoken_tag(secs, cut)), body),
                None => (None, t.as_str()),
            };
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: USER, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: "◆ you", color: USER, weight: Weight::Bold)
                    #(tag.map(|t| element!(Text(content: t, color: SAND))))
                    #(plain(&crate::attach::display(body), TextWrap::Wrap, |c| c))
                }
            }
            .into_any()
        }
        // Cards (peer traffic, skill bodies, the prompt): body hidden until Ctrl+O.
        Entry::Card {
            kind,
            title,
            text,
            expanded,
            ..
        } => {
            let color = match kind {
                CardKind::Skill => SKILL,
                CardKind::Prompt => PROMPT,
                CardKind::Job | CardKind::Schedule => JOB,
                CardKind::Component { color, .. } => *color,
                CardKind::Removed => PROMPT,
                CardKind::Vision => JOB,
                // 152 D6: a restart note is harness bookkeeping, muted like the prompt.
                CardKind::Restart | CardKind::Memory | CardKind::Moved | CardKind::EngineRestart => PROMPT,
            };
            let v = text.view();
            // 183: an outside agent's words are plain text: no Markdown, no OSC 8 links, no control characters.
            let outside = kind.plain().then(|| crate::model::clean(text.raw()));
            let lines = outside.as_ref().map_or_else(|| v.lines().count(), |t| t.lines().count());
            let glyph = kind.glyph();
            let header = if *expanded {
                format!("{glyph} {title}   ctrl+o collapses")
            } else {
                format!("{glyph} {title}   {lines} lines · ctrl+o expands")
            };
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: color, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: header, color, weight: Weight::Bold)
                    #(expanded.then(|| element!(View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        #(match &outside {
                            Some(t) => vec![element!(Text(content: t.clone(), wrap: TextWrap::Wrap)).into_any()],
                            None => v.lines().map(line).collect(),
                        })
                    })))
                }
            }
            .into_any()
        }
        Entry::Assistant(t) => {
            let v = t.view();
            element! {
                View(flex_direction: FlexDirection::Column) {
                    #(v.lines().skip(skip).map(line))
                    #((!v.plain.is_empty() && v.has_blocks()).then(|| element!(Text(content: " "))))
                    #((!v.plain.is_empty()).then(|| plain(v.plain, TextWrap::Wrap, |c| c)))
                }
            }
            .into_any()
        }
        Entry::Reasoning { text, done } => {
            let shown = if *done {
                format!("… {}", text.lines().next().unwrap_or_default())
            } else {
                text.clone()
            };
            plain(&shown, TextWrap::Wrap, |c| c.color(SAND).italic())
        }
        Entry::Pending { name, .. } => {
            let row = summary.unwrap_or_else(|| format!("preparing {name}…"));
            element! {
                View(margin_top: 1) {
                    Text(content: format!("{spinner} {row}"), color: SAND, italic: true)
                }
            }
            .into_any()
        }
        // Tool calls: a card in the tool colour; name, arguments, then result.
        Entry::Tool {
            name,
            args,
            result,
            expanded,
            ..
        } => {
            let summary = match result {
                None => (
                    format!("{spinner} {}", summary.as_deref().unwrap_or("running…")),
                    SAND,
                ),
                Some((output, is_error)) => {
                    let mark = if *is_error { "✗" } else { "✓" };
                    let hint = match (expanded.is_some(), output.lines().count() > 1) {
                        (true, _) => "   ctrl+o collapses",
                        (false, true) => "   ctrl+o expands",
                        (false, false) => "",
                    };
                    (
                        format!("{mark} {}{hint}", result_line(output)),
                        if *is_error { CARNELIAN } else { MALACHITE },
                    )
                }
            };
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: TOOL, padding_left: 1, padding_right: 1, margin_top: 1, margin_bottom: 1) {
                    Text(content: format!("▸ {name}"), color: TOOL, weight: Weight::Bold)
                    #(args.iter().map(|l| element!(Text(content: l.clone(), wrap: TextWrap::Wrap))))
                    Text(content: summary.0, color: summary.1, wrap: TextWrap::Wrap)
                    #(expanded.as_ref().map(|t| element!(View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        #(t.view().lines().map(line))
                    })))
                    #(decorations.into_iter().filter_map(|view| match view { crate::components::View::Element { element, .. } => Some(element), _ => None }))
                }
            }
            .into_any()
        }
        // Compaction boundary: a divider in the prompt colour; the summary
        // (history only) hidden until Ctrl+O.
        Entry::Compacted {
            epoch,
            style,
            summary,
            expanded,
        } => {
            let hint = match (summary, *expanded) {
                (None, _) => String::new(),
                (Some(_), true) => "   ctrl+o collapses".to_owned(),
                (Some(t), false) => {
                    format!("   {} lines · ctrl+o expands", t.view().lines().count())
                }
            };
            let label = match style {
                CompactStyle::Clear => format!("context cleared (epoch {epoch})"),
                s => format!("context compacted (epoch {epoch} · {s})"),
            };
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: PROMPT, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: format!("── {label} ──{hint}"), color: PROMPT, weight: Weight::Bold)
                    #(summary.as_ref().filter(|_| *expanded).map(|t| element!(View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        #(t.view().lines().map(line))
                    })))
                }
            }
            .into_any()
        }
        Entry::Divider(label) => element! {
            View(border_style: BorderStyle::Round, border_color: PROMPT, padding_left: 1, padding_right: 1, margin_top: 1) {
                Text(content: format!("── {label} ──"), color: PROMPT, weight: Weight::Bold)
            }
        }
        .into_any(),
        Entry::Info(t) => plain(t, TextWrap::Wrap, |c| c.color(SAND)),
        Entry::Error(t) => plain(t, TextWrap::Wrap, |c| c.color(CARNELIAN)),
    }
}


/// Plain text as one `MixedText`; `paint` styles every run, and any bare
/// `http(s)://` URL is also underlined and an OSC 8 link (clickable in
/// terminals that support it).
fn plain(
    text: &str,
    wrap: TextWrap,
    paint: impl Fn(MixedTextContent) -> MixedTextContent,
) -> AnyElement<'static> {
    let urls = markdown::urls(text);
    let mut contents = Vec::with_capacity(urls.len() * 2 + 1);
    let mut at = 0;
    for r in urls {
        if r.start > at {
            contents.push(paint(MixedTextContent::new(&text[at..r.start])));
        }
        let url = &text[r.clone()];
        contents.push(
            paint(MixedTextContent::new(url))
                .decoration(TextDecoration::Underline)
                .hyperlink(url),
        );
        at = r.end;
    }
    if at < text.len() {
        contents.push(paint(MixedTextContent::new(&text[at..])));
    }
    element!(MixedText(contents, wrap)).into_any()
}

/// One rendered markdown line. `Style` is renderer-agnostic; this is the
/// iocraft mapping.
pub(crate) fn line(l: &markdown::Line) -> AnyElement<'static> {
    if l.is_empty() {
        // An empty `Text` measures zero rows.
        return element!(Text(content: " ")).into_any();
    }
    let contents: Vec<MixedTextContent> = l
        .iter()
        .map(|s| {
            let mut c = MixedTextContent::new(&s.text);
            if s.style.bold {
                c = c.weight(Weight::Bold);
            }
            if s.style.italic {
                c = c.italic();
            }
            if s.style.strike || s.style.dim {
                c = c.color(SAND);
            }
            if s.style.heading > 0 {
                c = c.color(HEADING);
                if s.style.heading <= 2 {
                    c = c.decoration(TextDecoration::Underline);
                }
            }
            if s.style.code {
                c = c.color(FAIENCE);
            }
            if let Some(url) = &s.style.link {
                c = c.decoration(TextDecoration::Underline).hyperlink(url);
            }
            c
        })
        .collect();
    element!(MixedText(contents: contents, wrap: TextWrap::Wrap)).into_any()
}

pub(crate) fn risk_name(r: Risk) -> &'static str {
    match r {
        Risk::ReadOnly => "read-only",
        Risk::Mutating => "mutating",
        Risk::Destructive => "destructive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::models::Picker;
    use crate::model::tests::{models_mut, models_reply, models_selection, models_key, harness_reply};
    fn models_menu(m: &Model, width: u16, height: u16) -> Option<AnyElement<'static>> {
        m.component_views(Surface::Modal, usize::from(width), usize::from(height))
            .into_iter().find_map(|view| match view {
                crate::components::View::Element { element, .. } => Some(element), _ => None,
            })
    }
    use agent::ModelInfo;
    use runtime::SessionId;
    use crate::components::search_attach::{SearchAttach, Msg as SearchMsg, state::{self as search, Focus}};

    #[test]
    fn entry_summary_replaces_the_spinner_text_without_adding_rows() {
        use crate::components::web_progress::WebProgress;
        for (raw, rows, fallback) in [
            (Entry::Pending { call_id: "crawl".into(), name: "crawl".into() }, 2, "preparing crawl…"),
            (Entry::Tool { call_id: "crawl".into(), name: "crawl".into(), args: vec!["https://example.com".into()], result: None, expanded: None }, 7, "running…"),
        ] {
            let mut model = Model::new(String::new(), false);
            model.entries.push(raw);
            let render = |model: &Model| {
                let raw = &model.entries[0];
                entry(raw, "·", 0, 80, Vec::new(), model.entry_summary(raw))
            };
            assert_eq!(render(&model).render(Some(80)).height(), rows);
            assert!(render(&model).to_string().contains(fallback));
            model.components.get_mut::<WebProgress>().unwrap().progress.insert("crawl".into(), "crawling… 2/50 · example.com".into());
            let mut frame = render(&model);
            assert_eq!(frame.render(Some(80)).height(), rows);
            assert_eq!(model.entry_height(&model.entries[0], 80), rows);
            let text = frame.to_string();
            assert!(text.contains("· crawling… 2/50 · example.com"), "{text}");
            assert_eq!(text.matches("crawling…").count(), 1);
            assert!(!text.contains(fallback), "{text}");
            assert_eq!(model.activity().as_deref(), Some("crawling… 2/50 · example.com"));
        }
    }

    #[test]
    fn missing_and_late_artifact_hydration_keeps_raw_fallback_heights_and_links() {
        use crate::components::{LocalReply, Surface, View as OwnerView};
        use crate::components::artifacts_apps::{ArtifactsApps, Msg};
        use agent::{HistoryEntry, Loaded};
        let session = SessionId::new();
        let loaded = || Loaded {
            history: vec![HistoryEntry::Tool {
                call_id: "hydrated".into(), name: "show".into(),
                args: serde_json::json!({"name":"note.md", "title":"Note", "content":"hello"}),
                result: Some(("shown note.md".into(), false)), files: Vec::new(), context: None,
            }],
            ..Loaded::default()
        };
        let mut model = Model::new(String::new(), false);
        model.session = Some(session);
        model.apply(UiEvent::History(session, loaded()));
        let old_generation = model.components.get::<ArtifactsApps>().unwrap().generation;
        model.apply(UiEvent::History(session, loaded()));
        let current_generation = model.components.get::<ArtifactsApps>().unwrap().generation;
        let artifact = artifact::Artifact {
            id: "late".into(), session, call_id: "hydrated".into(),
            name: "note.md".into(), title: "Note".into(), kind: artifact::Kind::Markdown,
            mime: "text/markdown".into(), bytes: 5, version: 1,
            path: PathBuf::from("/missing-theuth-wire-artifact.md"),
            source: None, preview: None, pages: None, origin: None, created_at: 0,
        };
        let hydrate = |model: &mut Model, generation| {
            model.apply(UiEvent::Component(LocalReply {
                to: model.components.token("artifacts-apps"), engine_epoch: 0,
                body: Box::new(Msg::Listed { session, generation, pick: false, apps: false,
                    result: Ok((vec![artifact.clone()], Vec::new())) }),
            }));
        };
        let surface = Surface::Entry { key: "hydrated", skipped: 0 };
        assert_eq!(model.entry_height(&model.entries[0], 80), 8);
        assert!(model.component_views(surface, 80, 24).is_empty());
        hydrate(&mut model, old_generation);
        assert_eq!(model.entry_height(&model.entries[0], 80), 8);
        let raw = entry(&model.entries[0], "·", 0, 80, model.component_views(surface, 80, 24), None).to_string();
        assert!(raw.contains("▸ show") && raw.contains("✓ shown note.md"), "{raw}");
        assert!(!raw.contains("file://"));
        hydrate(&mut model, current_generation);
        let decorations = model.component_views(surface, 80, 24);
        assert!(matches!(decorations.as_slice(), [OwnerView::Element { rows: 7, replace: true, .. }]));
        assert_eq!(model.entry_height(&model.entries[0], 80), 7);
        let canvas = entry(&model.entries[0], "·", 0, 80, decorations, None).render(Some(80));
        assert_eq!(canvas.height(), 7);
        let mut bytes = Vec::new();
        canvas.write_ansi(&mut bytes).unwrap();
        let rendered = String::from_utf8(bytes).unwrap();
        assert!(rendered.contains("▣ note.md"), "{rendered}");
        assert!(rendered.contains("\x1b]8;;file:///missing-theuth-wire-artifact.md\x1b\\"), "{rendered}");
        // The deliberately absent source was not read to join, measure or draw the card.
    }

    /// A search view with wide-character titles and snippets, one hit per
    /// category plus a `show more` row, as the menu would show it.
    fn search_model() -> Model {
        use ::search::{MatchRange, SearchPage, TextMatch};
        use rpc_compose::search::{SearchGroup, SearchResponse};
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = Model::default();
        m.components.register(SearchAttach::default()).unwrap();
        m.mount_components(&tx);
        m.submit("/search 日本語 deploy", &tx);
        let generation = m.components.get::<SearchAttach>().unwrap().state.generation;
        let session = agent::search::SearchHit {
            session: SessionId::new(),
            title: "日本語のデプロイ計画 · a long session title that keeps going".into(),
            kind: agent::search::HitKind::Message,
            seq: Some(3),
            role: Some(agent::search::HitRole::Assistant),
            snippet: "…日本語のデプロイ deploy 計画について長い説明が続きます…".into(),
            ranges: vec![MatchRange { start: 10, end: 16 }],
            score: 5,
            matches: 2,
            created_at: "2026-09-18T09:00:00Z".into(),
            updated_at: "2026-09-18T10:00:00Z".into(),
            context_removed: true,
            archived: false,
        };
        let mut page = SearchPage::ready(vec![session], 10);
        page.has_more = true;
        let knowledge = knowledge::search::ConceptSearchHit {
            meta: knowledge::Meta {
                id: "facts/日本語".into(),
                type_: "fact".into(),
                title: "日本語の概念".into(),
                description: String::new(),
                tags: vec!["tag".into()],
                scope: None,
                source: None,
                timestamp: None,
                owner: "user".into(),
                visibility: Default::default(),
                ..Default::default()
            },
            r#match: TextMatch {
                field: "body".into(),
                snippet: "本文 deploy 本文".into(),
                ranges: vec![MatchRange { start: 3, end: 9 }],
            },
        };
        let reply = search::Reply::Results {
                categories: ::search::SearchCategory::ALL.to_vec(),
                limit: 10,
                result: Ok(SearchResponse {
                    groups: vec![
                        SearchGroup::Sessions { page },
                        SearchGroup::Artifacts {
                            page: SearchPage::unavailable("artifact is disabled"),
                        },
                        SearchGroup::Knowledge {
                            page: SearchPage::ready(vec![knowledge], 10),
                        },
                        SearchGroup::KnowledgeHistory {
                            page: SearchPage::indexing(),
                        },
                    ],
                }),
        };
        m.apply(UiEvent::Component(crate::components::LocalReply {
            to: m.components.token("search-attach"), engine_epoch: 0,
            body: Box::new(SearchMsg::Reply { generation, reply }),
        }));
        m
    }

    #[derive(Default, Props)]
    struct ProbeProps {
        width: u16,
        budget: usize,
        focus: Focus,
    }

    /// `search_box` over [`search_model`] with `focus`, with the hooks a
    /// component owns.
    #[component]
    fn SearchProbe(mut hooks: Hooks, props: &mut ProbeProps) -> impl Into<AnyElement<'static>> {
        let focus = props.focus;
        let model = hooks.use_state(|| {
            let mut m = search_model();
            m.components.get_mut::<SearchAttach>().unwrap().state.focus = focus;
            m
        });
        let handle = hooks.use_ref_default::<TextInputHandle>();
        let m = model.read();
        component_slot(&m, Surface::Modal, props.width, props.budget as u16, Some(handle)).element().expect("search view")
    }

    /// Wide characters count cells, chips wrap whole, and the box never
    /// exceeds its row budget or the terminal width (49 §6.2). A box too
    /// short for every row (30 columns wrap the chips onto four rows, so
    /// six rows cannot hold the query and all chips at once) scrolls to
    /// the focus: the last chip and the footer actions are in view whenever
    /// they have the keys.
    #[test]
    fn search_box_fits_its_budget_at_any_width() {
        for (width, budget) in (30..=120u16).flat_map(|w| [(w, 6usize), (w, 12), (w, 30)]) {
            for (focus, shown) in [
                (Focus::Query, "Query:"),
                (Focus::Filters(6), "Meetings]"),
                (Focus::Footer(1), "[? Help]"),
            ] {
                let frame = element!(SearchProbe(width, budget, focus)).to_string();
                let lines: Vec<&str> = frame.lines().collect();
                let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
                let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
                assert!(
                    bottom - top < budget,
                    "width {width} budget {budget} {focus:?}: box spans {} rows",
                    bottom - top + 1
                );
                assert!(
                    lines.iter().all(|l| search::cells(l) <= usize::from(width)),
                    "width {width} budget {budget} {focus:?}: a row overflows\n{frame}"
                );
                assert!(
                    frame.contains(shown),
                    "width {width} budget {budget} {focus:?}: {shown} not in view\n{frame}"
                );
            }
        }
    }

    /// Long rows used to make the box taller than its rows (empty space under
    /// the list) at some widths; the box must be title + rows + border. The
    /// title wrapped below width 74 until 194 D6 fitted it: now every width
    /// is exactly title + ten rows.
    #[test]
    fn menu_box_is_tight_at_any_width() {
        let mut m = Model::new(String::new(), false);
        let items: Vec<ModelInfo> = (0..10)
            .map(|i| ModelInfo {
                id: format!("amazon-bedrock/us.anthropic.claude-sonnet-4-5-2025092{i}-v1:0"),
                name: "Claude Sonnet 4.5 (2025-09-29)".into(),
                provider: "amazon-bedrock".into(),
                context: 200_000,
                reasoning: false,
                image: false,
                efforts: Vec::new(),
                budget_min: None,
                budget_max: None,
                local: false,
            })
            .collect();
        // The provider's models level lists all ten rows (124 §3.2).
        let selection = models_selection(&m);
        models_mut(&mut m).menu = Some(Picker {
            items,
            filter: String::new(),
            cursor: 0,
            provider: Some("amazon-bedrock".into()),
            selection,
            chip: 0,
        });
        m.components.focus_fixture("models", crate::components::FocusClass::Modal);
        for width in 60..=120u16 {
            let modal = models_menu(&m, width, 30).unwrap();
            let frame = element! {
                View(flex_direction: FlexDirection::Column, width, height: 30u16) {
                    View(flex_grow: 1.0_f32, min_height: 0)
                    #(Some(modal))
                    View(height: 3)
                }
            }
            .to_string();
            let lines: Vec<&str> = frame.lines().collect();
            let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
            let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
            let inner = &lines[top + 1..bottom];
            let is_row = |l: &str| l.contains("amazon-bedrock/us.");
            assert_eq!(
                inner.iter().filter(|l| is_row(l)).count(),
                10,
                "width {width}"
            );
            assert!(
                inner.iter().rev().take(10).all(|l| is_row(l)),
                "width {width}: empty space under the list in {top}..{bottom}"
            );
            assert_eq!(bottom - top, 12, "width {width}: box spans {top}..{bottom}");
            assert!(
                lines
                    .iter()
                    .all(|l| l.chars().count() <= usize::from(width)),
                "width {width}: a row overflows"
            );
        }
    }

    /// 125 §7.4: the card is border + title + one row per shown note (+ the
    /// `… +N more` row collapsed), its badges in the rows, nothing wider than
    /// the terminal (`Entry::height` counts the same rows; `model.rs` tests it).
    #[test]
    fn advisory_card_is_tight_at_any_width() {
        use agent::{Advisory, AdvisoryNote, AdvisorSeverity};
        use crate::components::advisor::{Card as AdvisoryCard, render_card};
        let notes = |n: usize| -> Vec<AdvisoryNote> {
            (0..n)
                .map(|i| AdvisoryNote {
                    severity: match i % 3 {
                        0 => AdvisorSeverity::Nit,
                        1 => AdvisorSeverity::Concern,
                        _ => AdvisorSeverity::Blocker,
                    },
                    note: format!("note {i} stays short so it never wraps"),
                })
                .collect()
        };
        let card = |n: usize, expanded: bool| AdvisoryCard {
            advisory: Advisory {
                advisor: runtime::SessionId::new(),
                step: 1,
                turn: None,
                at_ms: 0,
                notes: notes(n),
                kind: Default::default(),
            },
            expanded,
        };
        for width in 60..=120u16 {
            for (e, rows, more) in [
                (card(5, false), 3, Some("… +2 more notes")),
                (card(5, true), 5, None),
                (card(2, false), 2, None),
            ] {
                let frame = element! {
                    View(flex_direction: FlexDirection::Column, width, height: 20u16) {
                        #(Some(render_card(&e.advisory, e.expanded)))
                    }
                }
                .to_string();
                let lines: Vec<&str> = frame.lines().collect();
                let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
                let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
                let body = 1 + rows + usize::from(more.is_some());
                assert_eq!(bottom - top, body + 1, "width {width}:\n{frame}");
                assert!(
                    lines
                        .iter()
                        .all(|l| l.chars().count() <= usize::from(width))
                );
                assert!(lines[top + 1].contains("◎ advisor ·"), "{frame}");
                assert!(lines[top + 2].contains("[nit]") && lines[top + 3].contains("[concern]"));
                if let Some(more) = more {
                    assert!(lines[top + body].contains(more), "{frame}");
                }
            }
        }
    }

    /// R1.2: the consultation card titles with the question, shows the
    /// answer's lines unbadged (three until Ctrl+O, `… +N more lines`), in
    /// SAND, and never exceeds the terminal width.
    #[test]
    fn consultation_card_is_tight_at_any_width() {
        use agent::{Advisory, AdvisoryNote, AdvisorSeverity, AdvisoryKind};
        use crate::components::advisor::{Card as AdvisoryCard, render_card, severity_color};
        let answer = (0..5)
            .map(|i| format!("answer line {i} stays short"))
            .collect::<Vec<_>>()
            .join("\n");
        let card = |expanded: bool| AdvisoryCard {
            advisory: Advisory {
                advisor: runtime::SessionId::new(),
                step: 1,
                turn: None,
                at_ms: 0,
                notes: vec![AdvisoryNote {
                    severity: AdvisorSeverity::Advice,
                    note: answer.clone(),
                }],
                kind: AdvisoryKind::Consultation {
                    question: "justfile\nor README?".into(),
                },
            },
            expanded,
        };
        assert_eq!(severity_color(AdvisorSeverity::Advice), SAND);
        for width in 60..=120u16 {
            for (e, rows, more) in [
                (card(false), 3, Some("… +2 more lines")),
                (card(true), 5, None),
            ] {
                let frame = element! {
                    View(flex_direction: FlexDirection::Column, width, height: 20u16) {
                        #(Some(render_card(&e.advisory, e.expanded)))
                    }
                }
                .to_string();
                let lines: Vec<&str> = frame.lines().collect();
                let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
                let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
                let body = 1 + rows + usize::from(more.is_some());
                assert_eq!(bottom - top, body + 1, "width {width}:\n{frame}");
                assert!(
                    lines
                        .iter()
                        .all(|l| l.chars().count() <= usize::from(width))
                );
                assert!(
                    lines[top + 1].contains("◎ consulted: justfile or README?"),
                    "{frame}"
                );
                assert!(lines[top + 2].contains("answer line 0") && !frame.contains("[advice]"));
                if let Some(more) = more {
                    assert!(lines[top + body].contains(more), "{frame}");
                }
            }
        }
    }

    /// 127 §3: the two-char marker column — `●` current, `★` favorite — on
    /// every level, and the `⇥ ★ favorite` hint in each title.
    #[test]
    fn model_rows_carry_current_and_favorite_markers() {
        let model = |id: &str, name: &str, provider: &str| ModelInfo {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            context: 200_000,
            reasoning: false,
            image: false,
            efforts: Vec::new(),
            budget_min: None,
            budget_max: None,
            local: false,
        };
        let items = vec![
            model("google/gemini-2.5-pro", "Gemini 2.5 Pro", "google"),
            model("openai/gpt-5.4", "GPT-5.4", "openai"),
            model("openai/gpt-5.4-mini", "GPT-5.4 mini", "openai"),
        ];
        let mut m = Model::new("openai/gpt-5.4".into(), false);
        models_mut(&mut m).favorite_models = vec!["openai/gpt-5.4".into(), "google/gemini-2.5-pro".into()];
        let render = |m: &Model| models_menu(m, 120, 30).unwrap().to_string();
        // Providers level: favorites with their markers, the current one `●★`; the current provider `● `.
        let selection = models_selection(&m);
        models_mut(&mut m).menu = Some(Picker {
            items: items.clone(),
            filter: String::new(),
            cursor: 0,
            provider: None,
            selection,
            chip: 0,
        });
        m.components.focus_fixture("models", crate::components::FocusClass::Modal);
        let frame = render(&m);
        assert!(frame.contains("⏎ open provider  ⇥ ★ favorite"), "{frame}");
        assert!(frame.contains("●★ openai/gpt-5.4 "), "{frame}");
        assert!(frame.contains(" ★ google/gemini-2.5-pro "), "{frame}");
        assert!(frame.contains("●  openai "), "{frame}");
        // Models level: `●★` on the current favorite, blanks on a plain model.
        let selection = models_selection(&m);
        models_mut(&mut m).menu = Some(Picker {
            items: items.clone(),
            filter: String::new(),
            cursor: 0,
            provider: Some("openai".into()),
            selection,
            chip: 0,
        });
        m.components.focus_fixture("models", crate::components::FocusClass::Modal);
        let frame = render(&m);
        assert!(frame.contains("⏎ set  ⇥ ★ favorite  ←/⌫ back"), "{frame}");
        assert!(frame.contains("●★ openai/gpt-5.4 "), "{frame}");
        assert!(frame.contains("   openai/gpt-5.4-mini "), "{frame}");
        // Search: the Current row then a favorite hit.
        let selection = models_selection(&m);
        models_mut(&mut m).menu = Some(Picker {
            items,
            filter: "gemini".into(),
            cursor: 1,
            provider: None,
            selection,
            chip: 0,
        });
        m.components.focus_fixture("models", crate::components::FocusClass::Modal);
        let frame = render(&m);
        assert!(frame.contains("⏎ set  ⇥ ★ favorite  esc close"), "{frame}");
        assert!(frame.contains("●★ openai/gpt-5.4 "), "{frame}");
        assert!(frame.contains(" ★ google/gemini-2.5-pro "), "{frame}");
    }

    /// 194 D6: the chip row renders on the line right under the picked model
    /// and nowhere else — Default first, `●` on the active level, `[…]` on
    /// the highlighted chip once the cursor is there, the title naming the
    /// chip keys — and at every width the title stays one line.
    #[test]
    fn effort_chips_render_under_the_picked_model() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let model = |id: &str, efforts: &[&str]| ModelInfo {
            id: id.into(),
            name: id.into(),
            provider: "openai".into(),
            context: 200_000,
            reasoning: true,
            image: false,
            efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
            budget_min: None,
            budget_max: None,
            local: false,
        };
        let mut m = Model::new("openai/gpt-5.4".into(), false);
        m.session = Some(SessionId::new());
        m.effort = Some("high".into());
        models_reply(&mut m, vec![
            model("openai/gpt-5.4", &["low", "high"]),
            model("openai/o3-pro", &["low", "medium", "high"]),
        ]);
        let frame = models_menu(&m, 120, 30).unwrap().to_string();
        let lines: Vec<&str> = frame.lines().collect();
        let row = lines
            .iter()
            .position(|l| l.contains("openai/gpt-5.4 "))
            .unwrap();
        assert!(
            lines[row + 1].contains("effort  Default   low   ●high "),
            "{frame}"
        );
        assert!(lines[row + 2].contains("openai/o3-pro "), "{frame}");
        assert_eq!(frame.matches("effort").count(), 1, "{frame}");
        assert!(!frame.contains("medium"), "{frame}");
        models_key(&mut m, KeyCode::Down, &tx);
        let frame = models_menu(&m, 120, 30).unwrap().to_string();
        assert!(
            frame.contains("model › effort  ←→ choose  ⏎ set  esc close"),
            "{frame}"
        );
        assert!(frame.contains("effort  Default   low  [●high]"), "{frame}");
        for width in 40..=120u16 {
            for at in [0, 1] {
                if let Some(Picker { cursor, .. }) = &mut models_mut(&mut m).menu {
                    *cursor = at;
                }
                let modal = models_menu(&m, width, 30).unwrap();
                let frame = element! {
                    View(flex_direction: FlexDirection::Column, width, height: 30u16) {
                        View(flex_grow: 1.0_f32, min_height: 0)
                        #(Some(modal))
                    }
                }
                .to_string();
                let lines: Vec<&str> = frame.lines().collect();
                let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
                let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
                assert_eq!(bottom - top, 5, "width {width} cursor {at}:\n{frame}");
                assert!(
                    lines
                        .iter()
                        .all(|l| l.chars().count() <= usize::from(width)),
                    "width {width} cursor {at}: a row overflows\n{frame}"
                );
            }
        }
    }

    /// 194 D6: at a narrow width the chip row is a window around the focused
    /// chip, `…` where chips are hidden, so the highlighted chip always shows.
    #[test]
    fn focused_chip_stays_visible_at_narrow_widths() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = Model::new("openai/gpt-5.4".into(), false);
        m.session = Some(SessionId::new());
        models_reply(&mut m, vec![ModelInfo {
            id: "openai/gpt-5.4".into(),
            name: "GPT-5.4".into(),
            provider: "openai".into(),
            context: 200_000,
            reasoning: true,
            image: false,
            efforts: ["low", "medium", "high", "xhigh", "max"]
                .map(str::to_owned)
                .to_vec(),
            budget_min: None,
            budget_max: None,
            local: false,
        }]);
        models_key(&mut m, KeyCode::Down, &tx);
        for _ in 0..5 {
            models_key(&mut m, KeyCode::Right, &tx);
        }
        let width = 40u16;
        let modal = models_menu(&m, width, 30).unwrap();
        let frame = element! {
            View(flex_direction: FlexDirection::Column, width, height: 30u16) {
                #(Some(modal))
            }
        }
        .to_string();
        let chips = frame.lines().find(|l| l.contains("   effort ")).unwrap();
        assert!(chips.contains("[max]"), "{frame}");
        assert!(chips.contains("… "), "{frame}");
        assert!(!chips.contains("Default"), "{frame}");
        assert!(chips.chars().count() <= usize::from(width), "{frame}");
    }

    /// D1 parity: a local model's row carries the `local` tag; a favorite is
    /// starred in the model picker but never in the harness picker (⇥ is off there).
    #[test]
    fn rows_tag_local_models_and_harness_rows_have_no_stars() {
        let model = |id: &str, local: bool| ModelInfo {
            id: id.into(),
            name: id.into(),
            provider: "ollama".into(),
            context: 32_000,
            reasoning: false,
            image: false,
            efforts: Vec::new(),
            budget_min: None,
            budget_max: None,
            local,
        };
        let mut m = Model::new("ollama/qwen3".into(), false);
        models_mut(&mut m).favorite_models = vec!["ollama/qwen3".into(), "a/x".into()];
        models_reply(&mut m, vec![
            model("ollama/qwen3", true),
            model("ollama/cloudy", false),
        ]);
        let frame = models_menu(&m, 120, 30).unwrap().to_string();
        let line = |id: &str| {
            frame
                .lines()
                .find(|l| l.contains(&format!("{id} ")))
                .unwrap_or_else(|| panic!("no {id} row:\n{frame}"))
                .to_owned()
        };
        assert!(line("ollama/qwen3").contains("· local"), "{frame}");
        assert!(line("ollama/qwen3").contains('★'), "{frame}");
        assert!(!line("ollama/cloudy").contains("local"), "{frame}");
        // The harness picker: `a/x` is a normal favorite, yet its row has no star.
        models_mut(&mut m).menu = None;
        m.session = Some(SessionId::new());
        m.agent = Some("omp".into());
        let options: Vec<acp::ConfigOption> = serde_json::from_value(serde_json::json!([
            {"id":"model","name":"Model","category":"model","current":"a/x",
             "options":[{"value":"a/x","name":"X","description":null}]}
        ]))
        .unwrap();
        harness_reply(&mut m, options, false);
        let frame = models_menu(&m, 120, 30).unwrap().to_string();
        let row = frame.lines().find(|l| l.contains("a/x ")).unwrap();
        assert!(row.contains('●') && !row.contains('★'), "{frame}");
    }

    /// Eight live children: six rows, one fold line, and no row wider than
    /// the terminal (the rows are `NoWrap` texts in a row view).
    #[test]
    fn board_caps_rows_and_stays_tight() {
        use agent::AgentEvent;
        use runtime::SessionId;
        let mut m = Model::default();
        let root = SessionId::new();
        m.session = Some(root);
        for i in 0..8 {
            m.apply(UiEvent::Agent(AgentEvent::SubagentSpawned {
                parent: root,
                child: SessionId::new(),
                name: format!("worker-{i}-with-a-long-name"),
                cap: None,
            }));
        }
        for width in 60..=120u16 {
            let frame = element! {
                View(flex_direction: FlexDirection::Column, width, height: 30u16) {
                    View(flex_grow: 1.0_f32, min_height: 0)
                    #(board(&m, width))
                    View(height: 3)
                }
            }
            .to_string();
            let lines: Vec<&str> = frame.lines().collect();
            let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
            let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
            // title + 6 rows + fold line + bottom border.
            assert_eq!(bottom - top, 9, "width {width}: box spans {top}..{bottom}");
            assert!(lines[bottom - 1].contains("… 2 more (/agents)"));
            assert!(
                lines
                    .iter()
                    .all(|l| l.chars().count() <= usize::from(width)),
                "width {width}: a row overflows"
            );
        }
        // Idle everywhere: no board.
        for id in m.agents.keys().copied().collect::<Vec<_>>() {
            m.apply(UiEvent::Agent(AgentEvent::TurnFinished {
                session: id,
                turn: runtime::TurnId::new(),
                stop_reason: "end_turn".into(),
                usage: agent::Usage::default(),
                context: 0,
            }));
        }
        assert!(board(&m, 80).is_none() && m.status().contains("agents: 8 idle"));
    }

    /// Every look at 80×24 and 120×40 renders without panicking, fits the
    /// pane, keeps the orb block horizontally centred (±1 cell) and shows the
    /// two subtitles and the caption under it.
    #[test]
    fn talk_pane_is_centred_at_both_sizes() {
        use ::talk::{State, TalkEvent, TalkId};
        use crate::components::talk::{talk_pane, state::{self as talk, Talking}};
        let mut t = Talking::new(TalkId::new(), SessionId::new());
        let id = t.id;
        t.apply(
            TalkEvent::Saying {
                id,
                speech: voice::SpeechId::new(),
                text: "Berlin is mild and cloudy today, around fourteen degrees.".into(),
            },
            0,
        );
        t.apply(
            TalkEvent::Heard {
                id,
                utterance: 0,
                text: "what about tomorrow".into(),
                is_final: false,
            },
            0,
        );
        t.apply(
            TalkEvent::Degraded {
                id,
                degradation: ::talk::TalkDegradation::MicNoAec,
            },
            0,
        );
        t.level = 0.8;
        for (width, height) in [(80u16, 24usize), (120, 40)] {
            for state in [
                State::Listening,
                State::Thinking,
                State::Speaking,
                State::Holding,
            ] {
                t.apply(TalkEvent::TalkState { id, state }, 0);
                let frame = element! {
                    View(width, height: height as u16) {
                        #(Some(talk_pane(&t, 45, width, height)))
                    }
                }
                .to_string();
                let lines: Vec<&str> = frame.lines().collect();
                assert!(
                    lines.len() <= height,
                    "{state:?} {width}x{height}: {} rows",
                    lines.len()
                );
                let orb: Vec<&str> = lines
                    .iter()
                    .copied()
                    .filter(|l| {
                        l.chars().any(|c| "•·▓▒░●".contains(c))
                            && !l.chars().any(char::is_alphabetic)
                    })
                    .collect();
                let r = talk::radius(usize::from(width), height, 7);
                assert!(
                    !orb.is_empty() && orb.len() <= 2 * r + 1,
                    "{state:?} {width}x{height}: {} orb rows\n{frame}",
                    orb.len()
                );
                let mid = f64::from(width) / 2.0;
                for l in &orb {
                    let first = l.chars().position(|c| c != ' ').unwrap();
                    let last = l.chars().count() - l.chars().rev().position(|c| c != ' ').unwrap();
                    let centre = (first + last) as f64 / 2.0;
                    assert!(
                        (centre - mid).abs() <= 1.0,
                        "{state:?} {width}x{height}: row centre {centre} vs {mid}\n{frame}"
                    );
                }
                assert!(frame.contains("fourteen degrees."), "{state:?}\n{frame}");
                assert!(frame.contains("what about tomorrow"), "{state:?}\n{frame}");
                assert!(frame.contains(talk::DEGRADED), "{state:?}\n{frame}");
            }
        }
    }

    /// A live-talk turn draws its marker as a tag line, not as raw text.
    #[test]
    fn spoken_user_card_shows_tag_not_marker() {
        let e = Entry::User {
            text: "[spoken, 5 s, interrupted] hello".into(),
            turn: None,
        };
        let frame = element! {
            View(flex_direction: FlexDirection::Column, width: 60u16) {
                #(entry(&e, "", 0, 60, Vec::new(), None))
            }
        }
        .to_string();
        assert!(frame.contains("spoken 5 s · cut in"), "{frame}");
        assert!(frame.contains("hello"), "{frame}");
        assert!(!frame.contains("[spoken"), "{frame}");
    }

    /// 183: an outside sender's card draws its words as written: no Markdown,
    /// no link, no control character; a mesh session's card is still Markdown.
    #[test]
    fn outside_peer_card_is_plain_text() {
        let frame = |title: &str| {
            let e = Entry::Card { entry_key: None, kind: crate::components::peers::kind(title),
            title: title.into(),
            text: Box::new(markdown::Text::complete(
                "**now** [click](http://x/go)\u{1b}[2J",
            )),
            expanded: true, };
            element! {
                View(flex_direction: FlexDirection::Column, width: 60u16) {
                    #(entry(&e, "", 0, 60, Vec::new(), None))
                }
            }
            .to_string()
        };
        let outside = frame("contact:studio:bob · 00000000");
        assert!(
            outside.contains("**now** [click](http://x/go)[2J"),
            "{outside}"
        );
        assert!(!outside.contains('\u{1b}'), "{outside:?}");
        let mesh = frame("session:abc · 00000000");
        assert!(!mesh.contains("**now**"), "{mesh}");
        assert!(mesh.contains("now"), "{mesh}");
    }

    /// 109 §10.3: the resume divider takes the `Compacted` shape — a
    /// bordered `── label ──` line, 3 rows (`Entry::height` agrees); 139 §9.3:
    /// the transfer / takeover labels name the previous owner in the same shape.
    #[test]
    fn resume_divider_takes_the_compacted_shape() {
        for label in [
            "resumed after restart",
            "resumed after transfer from abcdef01",
            "resumed after takeover from abcdef01",
            "taken from abcdef01",
        ] {
            let e = Entry::Divider(label.into());
            let frame = element! {
                View(flex_direction: FlexDirection::Column, width: 60u16) {
                    #(entry(&e, "", 0, 60, Vec::new(), None))
                }
            }
            .to_string();
            assert!(frame.contains(&format!("── {label} ──")), "{frame}");
            assert!(frame.contains('╭') && frame.contains('╰'), "{frame}");
            assert_eq!(
                frame.trim_end_matches('\n').lines().count(),
                4,
                "margin + 3 rows: {frame}"
            );
        }
    }
}
