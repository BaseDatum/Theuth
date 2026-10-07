//! Fullscreen iocraft app. Pure: reads [`UiEvent`]s from a channel, writes
//! [`Command`]s to another. All actor traffic lives in `lib.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use agent::{AdvisorSeverity, AdvisorState, CompactStyle};
use agent::todo::Status as TodoStatus;
use iocraft::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tools::{Decision, Risk};

use crate::clip;
use crate::dictation::{self, Dictating, Phase};
use crate::model::{
    APP_WHERE, AcpRow, CardKind, Command, Entry, Level, Menu, MenuKey, Model, Prompt, Row, Switch,
    TodoCard, TurnAction, UiEvent, action_name, agent_row, artifact_row, artifact_size, describe,
    gate_name, k, meta_line, now_ms, progress_line, result_line, short, slash_matches, window,
};
use crate::model::{
    ADVISORY_COLLAPSED, Grouped, ModelRow, PickTarget, advisory_severity, advisory_title,
    providers, session_row, severity_name, terminal_row,
};
use crate::pathac;
use crate::search::{self, Focus, Preview};
use crate::talk::{self, Look, Talking};

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
    /// `Info.advisor` (125 §6.5): the remembered advisor pick `/advisor on` re-uses.
    pub advisor: agent::AdvisorDefaults,
    /// `tools.approvals` from config.
    pub approvals: bool,
    /// `Info.app_enabled`: offer `/apps`.
    pub app_enabled: bool,
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
const BONE: Color = rgb(0xE8DCC8);
/// Secondary text.
const BONE_DIM: Color = rgb(0xC9B89A);
/// Tertiary text, hints, quiet borders.
const SAND: Color = rgb(0x9A8B72);
/// Accent: focus borders, headings, hints, the human's turn.
const OCHRE: Color = rgb(0xC47A2C);
/// Success / done.
const MALACHITE: Color = rgb(0x4A8A6E);
/// Info / agent state: tools, peers, asks.
const FAIENCE: Color = rgb(0x5B8F8A);
/// Error / stop.
const CARNELIAN: Color = rgb(0xC45A48);
/// Human turn cards.
const USER: Color = OCHRE;
/// Tool call cards.
const TOOL: Color = FAIENCE;
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
/// `/link pair` and `/nodes` invite QR cards.
const LINK: Color = FAIENCE;

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
    let advisor = links.advisor.clone();
    let approvals = links.approvals;
    let app_enabled = links.app_enabled;
    let catalog = links.catalog.clone();
    let mut model = hooks.use_state({
        let catalog = catalog.clone();
        move || {
            let mut m = Model::new(default_model, approvals);
            m.effort = default_effort.clone();
            m.default_effort = default_effort;
            m.advisor_default = advisor;
            m.app_enabled = app_enabled;
            m.catalog = Some(catalog);
            m.entries.push(Entry::Info(
                concat!("theuth ", env!("CARGO_PKG_VERSION")).into(),
            ));
            m
        }
    });
    let mut input = hooks.use_state(String::new);
    let mut handle = hooks.use_ref_default::<TextInputHandle>();
    // The `/search` query has its own editor and handle (49 §6.1): the chat
    // draft, its cursor and paste tokens are never touched by a search.
    let search_handle = hooks.use_ref_default::<TextInputHandle>();
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
            // Bracketed paste: a paste arrives as one `TerminalEvent::Paste`
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
            // Servers started connecting before this UI subscribed.
            let _ = commands.send(Command::McpStatus);
            // Skill names so `/<skill> [args]` resolves before `/skills` was ever opened.
            let _ = commands.send(Command::ListSkills);
            // The global communication default, before any session is shown (50 §10).
            let _ = commands.send(Command::GetCommunication(None));
            // The harness-wide vision fallback, so attaching an image to a
            // text-only model can say who will look at it (52 §3.2).
            let _ = commands.send(Command::GetVisionModel);
            // The streaming STT model: loaded now so Ctrl+D is instant (54 §0.5).
            let _ = commands.send(Command::DictationPrewarm);
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
                // A due search request (debounce, indexing poll) or terminal
                // poll (51 §7.3) wakes an idle loop.
                let now = now_ms();
                let due = model.read().search_wake().is_some_and(|t| t <= now)
                    || model.read().terminal_wake().is_some_and(|t| t <= now)
                    || model.read().path.wake().is_some_and(|t| t <= now);
                if pending.is_empty() && !model.read().animating() && !due {
                    continue;
                }
                let mut m = model.write();
                // Assistant text renders at the width current when it starts (30 §7).
                m.width = crossterm::terminal::size().map_or(0, |(w, _)| usize::from(w));
                for ev in pending.drain(..) {
                    m.apply(ev);
                }
                if m.animating() {
                    m.tick();
                }
                m.search_tick(now, &commands);
                m.terminal_tick(now, &commands);
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
                if std::mem::take(&mut m.refresh_mcp) {
                    let _ = commands.send(Command::ListMcp(m.session));
                }
                if std::mem::take(&mut m.refresh_link) {
                    let _ = commands.send(Command::LinkStatus);
                }
                if std::mem::take(&mut m.refresh_nodes) {
                    let _ = commands.send(Command::ListNodes);
                }
                if std::mem::take(&mut m.refresh_secrets) {
                    let _ = commands.send(Command::ListSecrets);
                }
                if std::mem::take(&mut m.refresh_artifacts)
                    && let Some(session) = m.session
                {
                    let _ = commands.send(Command::ListArtifacts {
                        session,
                        pick: false,
                        apps: false,
                    });
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
                if std::mem::take(&mut m.refresh_communication) {
                    let _ = commands.send(Command::GetCommunication(m.session));
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
                // Only the open `/peers` menu sets this; a stale flag after it closed is dropped.
                if std::mem::take(&mut m.refresh_peers)
                    && let Some(Menu::Peers { session, .. }) = &m.menu
                {
                    let _ = commands.send(Command::ListPeers(*session));
                    let _ = commands.send(Command::ListPeerRequests(*session));
                }
                // The ledger moved (51 §6.2): the owner summary, never a local sum.
                if std::mem::take(&mut m.refresh_cost)
                    && let Some(session) = m.session
                {
                    let _ = commands.send(Command::SessionCost(session));
                }
                // 125 §3: the link survives a restart; re-read it with every load.
                if std::mem::take(&mut m.refresh_advisor)
                    && let Some(session) = m.session
                {
                    let _ = commands.send(Command::AdvisorStatus(session));
                }
                // Root metadata moved while a grouped picker is open (51 §3): re-list it.
                if std::mem::take(&mut m.refresh_grouping) {
                    match (&m.menu, m.session) {
                        (Some(Menu::Sessions { .. }), _) => {
                            let _ = commands.send(Command::ListSessions);
                        }
                        (Some(Menu::Artifacts { apps, .. }), Some(session)) => {
                            let _ = commands.send(Command::ListArtifacts {
                                session,
                                pick: true,
                                apps: *apps,
                            });
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
            // 55 §3: the keyboard belongs to the program in the open terminal,
            // Ctrl-C included; only Ctrl-] comes back here.
            if m.terminal_interactive() {
                match event {
                    TerminalEvent::Key(KeyEvent {
                        code,
                        kind,
                        modifiers,
                        ..
                    }) if kind != KeyEventKind::Release => {
                        m.terminal_type(code, modifiers, &commands);
                    }
                    TerminalEvent::Paste(text) => m.terminal_paste(text, &commands),
                    _ => {}
                }
                return;
            }
            let TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                if let TerminalEvent::Paste(text) = event {
                    // Search paste is query text at the cursor: never an
                    // attachment, paste token or slash command (49 §6.1).
                    if m.search_open() {
                        if m.search.focus == Focus::Query && m.search.previews.is_empty() {
                            let flat = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
                            let mut v = m.search.query.clone();
                            let at = search_handle.read().cursor_offset().min(v.len());
                            let at = if v.is_char_boundary(at) { at } else { v.len() };
                            v.insert_str(at, &flat);
                            m.search_edit(v);
                        }
                        return;
                    }
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
            if kind == KeyEventKind::Release {
                return;
            }
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                m.quit = true;
                return;
            }
            // Talk (76 §0.12, §5.2): the pane owns the keys — Esc cancels the
            // agent's turn while it thinks or speaks, else leaves the pane (the
            // talk runs on); Ctrl+T stops it; the rest is swallowed; an
            // approval prompt on top keeps its own keys.
            let ctrl_t = code == KeyCode::Char('t') && modifiers.contains(KeyModifiers::CONTROL);
            if m.talk_view && m.approval().is_none() {
                match code {
                    KeyCode::Esc => {
                        if let Some(cmd) = m.talk_escape() {
                            let _ = commands.send(cmd);
                        }
                    }
                    _ if ctrl_t => {
                        if let Some(cmd) = m.toggle_talk(true) {
                            let _ = commands.send(cmd);
                        }
                    }
                    _ => {}
                }
                return;
            }
            if ctrl_t && m.dictation.is_none() && m.menu.is_none() && m.secret_request().is_none() {
                if let Some(cmd) = m.toggle_talk(true) {
                    let _ = commands.send(cmd);
                }
                return;
            }
            // Dictation (54 §6): the box is busy; Ctrl+D keeps, Esc discards,
            // every other key is swallowed.
            let ctrl_d = code == KeyCode::Char('d') && modifiers.contains(KeyModifiers::CONTROL);
            if m.dictation.is_some() {
                let cmd = match code {
                    _ if ctrl_d => m.toggle_dictation(String::new(), String::new()),
                    KeyCode::Esc => m.discard_dictation(),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    let _ = commands.send(cmd);
                }
                return;
            }
            // Idle: anchor the caret (a byte offset, on a char boundary) and start.
            if ctrl_d && m.input_focused() && m.menu.is_none() && m.secret_request().is_none() {
                let v = input.read();
                let at = handle.read().cursor_offset();
                let at = if v.is_char_boundary(at) { at } else { v.len() };
                if let Some(cmd) = m.toggle_dictation(v[..at].to_owned(), v[at..].to_owned()) {
                    let _ = commands.send(cmd);
                }
                return;
            }
            // Chat-only: a search menu on top must not toggle transcript cards.
            if code == KeyCode::Char('o')
                && modifiers.contains(KeyModifiers::CONTROL)
                && !m.search_open()
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
            // The masked box types into the editor; ⏎ stores, esc declines.
            if m.secret_request().is_some() {
                match code {
                    KeyCode::Enter => {
                        let line = std::mem::take(&mut *input.write());
                        let value = line.trim();
                        if !value.is_empty() {
                            m.answer_secret(Some(value.to_owned()), &commands);
                        }
                    }
                    KeyCode::Esc => {
                        input.set(String::new());
                        m.answer_secret(None, &commands);
                    }
                    _ => {}
                }
                return;
            }
            if m.ask().is_some() {
                let text = input.read().to_string();
                if let Some(text) = m.ask_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            // The todo card (47 §6.3): F2 opens and focuses / closes it; while
            // focused it takes the scroll keys, esc, tab and ⏎, so the draft is
            // neither sent nor cleared. Anything else hands the editor back:
            // a typed char is inserted here, since `TextInput` saw this key
            // unfocused. Modals above kept their answers; a modal menu keeps its keys.
            let (w, h) = terminal_size;
            if code == KeyCode::F(2) && !m.modal_menu() {
                m.toggle_todos(&commands);
                return;
            }
            if m.todo_focused() {
                if m.todo_key(code, usize::from(w), usize::from(h)) {
                    return;
                }
                if let KeyCode::Char(c) = code
                    && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    insert_at_cursor(&mut m, &c.to_string());
                    return;
                }
            }
            // Shift+Enter: the input inserts the newline itself.
            if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
                return;
            }
            // Alt+Up: pull the newest queued steer back into the editor (not through search).
            if code == KeyCode::Up && modifiers.contains(KeyModifiers::ALT) && !m.search_open() {
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
            let todo = m
                .todo_card(usize::from(w), usize::from(h))
                .map_or(0, |c| c.height());
            let view = view_rows(&m, h, &input.read(), todo);
            // `/search` (49 §6.1): its own focus ring; the query editor's own
            // keys reach the search `TextInput`, everything else lands here.
            if m.search_open() {
                let (inner, area, _, _) = search_layout(&m, w, view);
                m.search_key(code, inner, area, &commands);
                // `/attach` picked a row (66 §10): its token leads the draft.
                if let Some(token) = m.attach_token.take() {
                    let v = input.read().to_string();
                    let v = if v.is_empty() {
                        token
                    } else {
                        format!("{token} {v}")
                    };
                    m.sync_slash(&v);
                    input.set(v);
                }
                return;
            }
            // Path autocomplete (PATH-AC): Tab on a path token opens the list
            // under the editor; the slash popup's Tab wins while it has matches.
            if m.input_focused() {
                let text = input.read().to_string();
                let cursor = handle.read().cursor_offset().min(text.len());
                let slash_busy = matches!(m.menu, Some(Menu::Slash { .. }))
                    && !slash_matches(text.split(' ').next().unwrap_or_default(), m.app_enabled)
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
            if m.menu.is_some() {
                let text = input.read().to_string();
                if let MenuKey::Input(text) = m.menu_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            // Earlier-messages inspection (49 §7): scroll, jump to latest, back to results.
            if m.inspect.is_some() {
                let step = match code {
                    KeyCode::PageUp => Some(-(view as isize)),
                    KeyCode::PageDown => Some(view as isize),
                    KeyCode::Up => Some(-1),
                    KeyCode::Down => Some(1),
                    _ => None,
                };
                if let Some(step) = step {
                    m.scroll(step, view, usize::from(w));
                } else {
                    match code {
                        KeyCode::Enter | KeyCode::Char('j') => m.jump_latest(&commands),
                        KeyCode::Esc | KeyCode::Char('b') | KeyCode::Backspace => {
                            m.back_to_results(&commands)
                        }
                        _ => {}
                    }
                }
                return;
            }
            // `o` on an empty editor opens the newest artifact card's file with the OS (35 §6.1).
            if code == KeyCode::Char('o')
                && modifiers.is_empty()
                && m.prompt.is_none()
                && input.read().is_empty()
                && let Some(path) = m.artifact_path()
            {
                let _ = commands.send(Command::OpenPath(path.to_owned()));
                return;
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
        element! {
            View(border_style: BorderStyle::Double, border_color: OCHRE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: format!("approval: {}  ({}){agent}", req.tool, risk_name(req.risk)), weight: Weight::Bold)
                #(describe(&req.tool, &req.args).into_iter().map(|l| element!(Text(content: l, color: FAIENCE, wrap: TextWrap::Wrap))))
                Text(content: format!("[y] allow  [n/esc] deny{open}"), color: OCHRE)
            }
        }
        .into_any()
    });
    // Approval on top, then a secret request, then the model's questions;
    // otherwise whichever menu is open.
    let (width, height) = terminal_size;
    let modal = modal
        .or_else(|| secret_box(&m))
        .or_else(|| ask_box(&m))
        .or_else(|| {
            m.search_open().then(|| {
                let todo = m
                    .todo_card(usize::from(width), usize::from(height))
                    .map_or(0, |c| c.height());
                let budget = view_rows(&m, height, &input.read(), todo);
                search_box(&m, model, search_handle, width, budget)
            })
        })
        .or_else(|| terminal_box(&m, width, height))
        .or_else(|| menu(&m, &input.read(), width, height));
    // A secret's value (or a provider's key) shows as dots.
    let mask = (m.secret_request().is_some() || m.prompt.as_ref().is_some_and(Prompt::masked))
        .then_some('•');
    let has_focus = m.input_focused();
    let (header, level) = m.header(&catalog);
    let header_color = match level {
        Level::Normal => SAND,
        Level::Warn => OCHRE,
        Level::Danger => CARNELIAN,
    };

    // Only the slice that can reach the screen is built; see `Model::window`.
    let todo = m.todo_card(usize::from(width), usize::from(height));
    let view = view_rows(
        &m,
        height,
        &input.read(),
        todo.as_ref().map_or(0, TodoCard::height),
    );
    let win = m.window(view, usize::from(width));
    let entries = m.entries[win.from..win.to]
        .iter()
        .enumerate()
        .map(|(i, e)| entry(e, m.spinner(), if i == 0 { win.skip } else { 0 }, width));
    let transcript = if let (true, Some(t)) = (m.talk_view, &m.talk) {
        talk_pane(t, m.frame, width, view)
    } else if m.anchor.is_some() {
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
            #(todo.map(todo_card))
            #(modal)
            #(m.inspect_line().map(|hint| {
                element!(View(height: 1, padding_left: 1) {
                    Text(content: hint, color: OCHRE, wrap: TextWrap::NoWrap)
                })
            }))
            #(m.prompt.as_ref().map(|p| {
                element!(View(height: 1, padding_left: 1) {
                    Text(content: p.hint(), color: OCHRE)
                })
            }))
            View(height: 1, padding_left: 1) {
                #(match (&m.dictation, m.talk_view) {
                    (Some(d), _) => element!(MixedText(contents: dictation_header(d, m.frame), wrap: TextWrap::NoWrap)).into_any(),
                    (None, true) => element!(Text(content: "● talk  (Esc cancels / leaves · Ctrl+T stops)", color: SAND)).into_any(),
                    (None, false) => element!(Text(content: header, color: header_color)).into_any(),
                })
            }
            // `flex_shrink: 0`: once the transcript fills the screen the box
            // would otherwise be squeezed back to `min_height` as it grows.
            // While dictating the box turns cyan and shows the region (54 §6)
            // over a hidden, still-mounted editor: unmounting it would lose
            // its cursor state and the handle the stop restores through. The
            // talk pane (76) drops the border and the editor the same way —
            // never `display: None` on a bordered view: iocraft draws a border
            // of width 0 - 2 and panics.
            View(border_style: if m.talk_view { BorderStyle::None } else { BorderStyle::Round }, border_color: if m.dictation.is_some() { FAIENCE } else { OCHRE }, min_height: if m.talk_view { 0 } else { 3 }, flex_shrink: 0.0, flex_direction: FlexDirection::Column) {
                View(width: 100pct, display: if m.dictation.is_some() || m.talk_view { Display::None } else { Display::Flex }) {
                    TextInput(
                        handle,
                        has_focus,
                        value: input.to_string(),
                        multiline: true,
                        auto_grow: true,
                        newline_modifiers: KeyModifiers::SHIFT,
                        mask,
                        on_change: move |v: String| {
                            let mut m = model.write();
                            m.sync_slash(&v);
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
                #(m.dictation.as_ref().map(|d| element!(MixedText(contents: dictation_region(d), wrap: TextWrap::Wrap))))
            }
            #(path_box(&m, width))
            View(height: 1, padding_left: 1, padding_right: 1, justify_content: JustifyContent::SpaceBetween) {
                View() {
                    Text(content: m.status(), color: SAND)
                    #(m.advisor_segment().map(|(text, state)| element!(Text(content: text, color: advisor_color(state)))))
                }
                Text(content: m.mcp_status(), color: SAND)
            }
        }
    }
}

/// The `/talk` pane (76 §0.12, §5.2) in a `width × height` content area: the
/// orb centred, the agent line (foreground) and the user line (muted) under
/// it, two rows each and tail-clamped, then the headphones caption. Colours
/// are the palette's: accent while speaking, muted while idle, the
/// terminal's own foreground while thinking.
fn talk_pane(t: &Talking, frame: usize, width: u16, height: usize) -> AnyElement<'static> {
    // Two subtitle blocks of two rows, one caption row, two gap rows.
    const RESERVE: usize = 7;
    let look = talk::look(t.state);
    let color = match look {
        Look::Idle => Some(SAND),
        Look::Thinking => None,
        Look::Speaking => Some(OCHRE),
    };
    let r = talk::radius(usize::from(width), height, RESERVE);
    let rows = talk::orb(look, t.orb_frame(frame), t.level, r);
    let agent = t.agent.as_ref().map_or("", |(_, s)| s.as_str()).to_owned();
    let user = t.user.clone().unwrap_or_default();
    let caption = if t.degraded { talk::DEGRADED } else { "" };
    element! {
        View(width: 100pct, height: 100pct, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center) {
            View(flex_direction: FlexDirection::Column) {
                #(rows.into_iter().map(|row| element!(Text(content: row, color, wrap: TextWrap::NoWrap))))
            }
            // Bottom-aligned in a fixed two-row box: a long line clips at its head, the tail stays.
            View(width: 100pct, height: 2, margin_top: 1, padding_left: 2, padding_right: 2, overflow: Overflow::Hidden, flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd) {
                Text(content: agent, wrap: TextWrap::Wrap, align: TextAlign::Center)
            }
            View(width: 100pct, height: 2, margin_top: 1, padding_left: 2, padding_right: 2, overflow: Overflow::Hidden, flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd) {
                Text(content: user, color: SAND, wrap: TextWrap::Wrap, align: TextAlign::Center)
            }
            View(height: 1) {
                Text(content: caption, color: SAND, wrap: TextWrap::NoWrap)
            }
        }
    }
    .into_any()
}

/// Header row while dictating (54 §6): a blinking `●`, the level meter in
/// cyan, the elapsed clock and the keys in grey.
fn dictation_header(d: &Dictating, frame: usize) -> Vec<MixedTextContent> {
    let dot = if (frame / 30).is_multiple_of(2) {
        "● "
    } else {
        "  "
    };
    let state = match d.phase {
        Phase::Starting => "starting",
        Phase::Dictating => "dictating",
        Phase::Stopping => "stopping",
    };
    vec![
        MixedTextContent::new(format!("{dot}{state}  ")).color(FAIENCE),
        MixedTextContent::new(dictation::meter(&d.levels, dictation::METER_CELLS)).color(FAIENCE),
        MixedTextContent::new(format!("  {}", d.clock())),
        MixedTextContent::new(" (Ctrl+D stop · Esc discard)").color(SAND),
    ]
}

/// The composer while dictating (54 §5.1): `before`, `lead`, each utterance
/// (grey until the VAD closes it), `trail`, `after`. Empty utterances are
/// skipped, so the joins mirror `splice`.
fn dictation_region(d: &Dictating) -> Vec<MixedTextContent> {
    let mut out = vec![MixedTextContent::new(&d.before)];
    let mut first = true;
    for u in d.utts.iter().filter(|u| !u.text.is_empty()) {
        out.push(MixedTextContent::new(if first { d.lead() } else { " " }));
        first = false;
        let mut c = MixedTextContent::new(&u.text);
        if !u.is_final {
            c = c.color(SAND);
        }
        out.push(c);
    }
    if !first {
        out.push(MixedTextContent::new(d.trail()));
    }
    out.push(MixedTextContent::new(&d.after));
    if d.before.is_empty() && d.after.is_empty() && first {
        // An empty box still needs a row for the border to wrap.
        out.push(MixedTextContent::new(" "));
    }
    out
}

/// The shown `secret_request` (37 §5): name and note, what version the
/// answer becomes, the keys. The value itself is typed into the (masked)
/// editor below. `None` without one.
fn secret_box(m: &Model) -> Option<AnyElement<'static>> {
    let r = m.secret_request()?;
    let head = if r.note.is_empty() {
        r.name.clone()
    } else {
        format!("{} — {}", r.name, r.note)
    };
    let version = r.current.map_or_else(
        || "(new secret)".to_owned(),
        |v| {
            format!(
                "(adds v{}; v{v} stays as {{{{secret:{}:-1}}}})",
                v + 1,
                r.name
            )
        },
    );
    Some(
        element! {
            View(border_style: BorderStyle::Double, border_color: FAIENCE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: "secret requested", weight: Weight::Bold)
                Text(content: head, color: FAIENCE, wrap: TextWrap::Wrap)
                Text(content: version, color: BONE_DIM)
                Text(content: "type it below · [⏎] store  [esc] decline", color: FAIENCE)
            }
        }
        .into_any(),
    )
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
    Some(
        element! {
            View(border_style: BorderStyle::Double, border_color: FAIENCE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: title, weight: Weight::Bold)
                Text(content: q.question.clone(), color: FAIENCE, wrap: TextWrap::Wrap)
                #(rows)
                Text(content: own, color: if a.cursor == n || a.typing { BONE } else { BONE_DIM }, weight: if a.cursor == n || a.typing { Weight::Bold } else { Weight::Normal })
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
            let rows = slash_matches(word, m.app_enabled)
                .into_iter()
                .map(|(name, hint, desc)| format!("{:<30} {desc}", format!("{name} {hint}")))
                .collect::<Vec<_>>();
            (
                "↑↓ move  ⇥ complete  ⏎ run  esc close".to_owned(),
                rows,
                *cursor,
            )
        }
        Menu::Models {
            items,
            filter,
            cursor,
            provider,
            target,
        } => {
            // 125 §7.2: the same picker, titled for whose model it sets.
            let noun = match target {
                PickTarget::Session => "model",
                PickTarget::Advisor => "advisor",
            };
            let rows = m.models_rows();
            let title = if !filter.is_empty() {
                let none = rows.iter().all(|r| matches!(r, ModelRow::Current(_)));
                format!(
                    "{noun}  search: {filter}▏  ⏎ set  esc close{}",
                    if none { "  no match" } else { "" }
                )
            } else if let Some(p) = provider {
                let back = if providers(items).len() > 1 {
                    "←/⌫ back  "
                } else {
                    ""
                };
                let models = rows.iter().filter(|r| !matches!(r, ModelRow::Off { .. })).count();
                format!("{noun} › {p} ({models})  ⏎ set  {back}esc close")
            } else {
                format!("{noun}  ⏎ open provider  type to search all models  esc close")
            };
            let mark = |on: bool| if on { "●" } else { " " };
            let model_row = |on: bool, i: &agent::ModelInfo| {
                format!(
                    "{} {:<44} {:<32} {}{}",
                    mark(on),
                    i.id,
                    i.name,
                    k(i.context),
                    if i.image { " · vision" } else { "" }
                )
            };
            let picked = m.pick_current();
            let rows = rows
                .iter()
                .map(|r| match r {
                    ModelRow::Off { current } => {
                        format!("{} {:<44} no advisor", mark(*current), "Off")
                    }
                    ModelRow::Current(i) => model_row(true, i),
                    ModelRow::Model(i) => model_row(Some(i.id.as_str()) == picked, i),
                    ModelRow::Provider { id, count, current } => {
                        format!("{} {id:<24} {count:>4} models", mark(*current))
                    }
                })
                .collect();
            (title, rows, *cursor)
        }
        Menu::Efforts { items, cursor, .. } => (
            "effort  ⏎ set  esc close".to_owned(),
            items.clone(),
            *cursor,
        ),
        Menu::AwsProfiles { cursor, .. } => (
            "aws profile  ⏎ use for every session's Bedrock calls from the next request  esc close"
                .to_owned(),
            m.aws_rows(),
            *cursor,
        ),
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
        // Drawn by `search_box`, never here.
        Menu::Search | Menu::Attach => return None,
        Menu::Agents { items, cursor } => (
            "agents  ⏎ open  esc close  (the one you leave keeps running)".to_owned(),
            items
                .iter()
                .map(|a| format!("{}{}", agent_row(a, m.spinner(), width), m.child_cost_tag(a.info.id)))
                .collect(),
            *cursor,
        ),
        Menu::Mcp {
            cursor,
            expanded,
            pending,
            ..
        } => (
            match pending {
                Some(t) => format!(
                    "turn off {}{}?   press  s = this session only  ·  a = always (all sessions, saved to mcp/config.yaml)  ·  esc = cancel",
                    t.server,
                    t.tool.as_deref().map_or(String::new(), |n| format!(" › {n}")),
                ),
                None => {
                    "mcp  ⏎ turn on/off (asks: this session or always)  ␣ cycle risk  ⌫ reset risk  →/← tools  l login/logout  s sync  esc close"
                        .to_owned()
                }
            },
            m.mcp_rows()
                .iter()
                .map(|r| match r {
                    Row::Server(s) => format!(
                        "{} {:<16} {:>2} tools   {:<20} {:<18} {}{}",
                        if expanded.contains(&s.name) {
                            "▾"
                        } else {
                            "▸"
                        },
                        s.name,
                        if s.connected {
                            s.tools.len().to_string()
                        } else {
                            "–".to_owned()
                        },
                        r.switch().label(),
                        match s.gate {
                            None | Some(tools::Gate::Off) => "risk: annotations".to_owned(),
                            Some(g) => format!("risk: {}", gate_name(g)),
                        },
                        match (r.switch(), s.connected, s.auth) {
                            (Switch::OffAlways, ..) => "○ off",
                            (_, true, _) => "● connected",
                            (_, false, tools::McpAuth::Required) => "○ login required",
                            (_, false, _) => "○ not connected",
                        },
                        s.repo
                            .as_deref()
                            .map_or_else(String::new, |r| format!("   ⟳ {r}")),
                    ),
                    // `mutating (ask)`: effective risk, then what `tools.policy`
                    // does with it (12 §5). `*` marks the tool's own line;
                    // `as x__y` the name the model sees when it clashes.
                    Row::Tool(_, t) => format!(
                        "    {:<24} {:<20} {:<22}{}",
                        t.name,
                        r.switch().label(),
                        match t.gate {
                            Some(tools::Gate::Off) => String::new(),
                            Some(_) =>
                                format!("{}* ({})", risk_name(t.risk), action_name(t.action)),
                            None => format!("{} ({})", risk_name(t.risk), action_name(t.action)),
                        },
                        t.registered
                            .as_deref()
                            .filter(|n| *n != t.name)
                            .map_or_else(String::new, |n| format!("  as {n}")),
                    ),
                })
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
        Menu::Link { cursor } => (m.link_title(), m.link_rows(), *cursor),
        Menu::Nodes { cursor } => (m.nodes_title(), m.nodes_rows(), *cursor),
        Menu::Secrets { cursor, history } => {
            (m.secrets_title(), m.secrets_rows(), history.unwrap_or(*cursor))
        }
        Menu::Artifacts {
            items,
            cursor,
            apps,
        } => (
            if *apps {
                format!("apps · {APP_WHERE}  ⏎ open  s show source  e export to ~/Downloads  esc close")
            } else {
                "artifacts  ⏎ open  s show inline  e export to ~/Downloads  esc close".to_owned()
            },
            items
                .iter()
                .map(|r| match r {
                    Grouped::Heading(h) => h.clone(),
                    Grouped::Item(a) => format!("    {}", artifact_row(a)),
                })
                .collect(),
            *cursor,
        ),
        Menu::Shake(s) => (s.title(), s.rows(), s.cursor),
        Menu::Broadcast(b) => (b.title(), b.rows(m.spinner()), b.cursor),
        Menu::Peers { cursor, .. } => (
            m.peers_title(),
            m.peers_rows().iter().map(|r| r.line()).collect(),
            *cursor,
        ),
        Menu::Knowledge { items, cursor } => (
            "knowledge  ⏎/h history of concept  H history of all  esc close".to_owned(),
            if items.is_empty() {
                vec!["no concepts · H shows the history of all (deleted ones too)".to_owned()]
            } else {
                items.iter().map(meta_line).collect()
            },
            *cursor,
        ),
        Menu::KnowledgeHistory { .. } => m.knowledge_history_menu(),
        Menu::Cost { cost, cursor } => (
            format!(
                "cost · {} · rev {}   esc close   (owner summary; nothing is summed here)",
                short(cost.session),
                cost.revision
            ),
            m.cost_rows(),
            *cursor,
        ),
        Menu::Terminals { items, cursor } => (
            "terminals  ⏎ observe (read-only)  r refresh  esc close".to_owned(),
            items.iter().map(terminal_row).collect(),
            *cursor,
        ),
        // Drawn by `terminal_box`, never here.
        Menu::Terminal(_) => return None,
    };
    let cursor = cursor.min(rows.len().saturating_sub(1));
    // A diff or the cost breakdown gets the screen minus the frame around the box; lists stay short.
    let tall = matches!(
        m.menu,
        Some(
            Menu::KnowledgeHistory {
                revision: Some(_),
                ..
            } | Menu::Cost { .. }
        )
    );
    let max = if tall {
        usize::from(height).saturating_sub(10).max(MENU_ROWS)
    } else {
        MENU_ROWS
    };
    let shown = window(rows.len(), cursor, max);
    // border 2 + padding 2 + cursor glyph 2, then one less.
    let fit = usize::from(width).saturating_sub(7);
    Some(
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
        .into_any(),
    )
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

/// The `/terminals` observer (51 §7.4): the page's cells as styled text
/// runs, the cursor cell inverted, `col` leading columns panned off, then a
/// status row for what the page cannot show. Attributes the toolkit lacks
/// (background colour) drop; the redacted text is the same either way.
fn terminal_box(m: &Model, width: u16, height: u16) -> Option<AnyElement<'static>> {
    let Some(Menu::Terminal(v)) = &m.menu else {
        return None;
    };
    let dim = SAND;
    // Border 2 + padding 2; rows stay one cell short so taffy never wraps them.
    let inner = usize::from(width).saturating_sub(5).max(8);
    // Border, title and status around the page; the page gets the rest of the screen.
    let budget = usize::from(height).saturating_sub(10).max(5);
    let mut body: Vec<AnyElement<'static>> = Vec::new();
    match &v.page {
        None => body.push(
            element!(Text(content: if v.unavailable { "" } else { "loading…" }, color: dim))
                .into_any(),
        ),
        Some(p) => {
            for (r, row) in p.lines.iter().take(budget).enumerate() {
                let cursor = p
                    .cursor
                    .filter(|(cr, _)| usize::from(*cr) == r)
                    .map(|(_, c)| usize::from(c));
                body.push(terminal_line(row, v.col, inner, cursor));
            }
        }
    }
    let status = v.status();
    let title = search::fit(&v.title(), inner);
    Some(
        element! {
            View(border_style: BorderStyle::Round, border_color: OCHRE, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1, width) {
                Text(content: title, color: dim, wrap: TextWrap::NoWrap)
                #(body)
                #(status.map(|s| element!(Text(content: search::fit(&s, inner), color: OCHRE, wrap: TextWrap::NoWrap))))
            }
        }
        .into_any(),
    )
}

/// One page row: cells from column `skip` on, at most `fit` columns, each
/// run's style mapped to the toolkit's; the cursor column inverted.
fn terminal_line(
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
                + usize::from(m.inspect.is_some())
                + activity
                + queued
                + board
                + todo,
        )
        .max(1)
}

/// The `/search` box geometry at `width` columns inside `budget` rows (the
/// transcript's rows: the box may take them all, never more): `(inner
/// columns, result rows, chip rows, compact)`. Query, chips, status and
/// footer are fixed; results get the rest, snippets dropped when fewer than
/// four rows remain (49 §6.2). Results keep one row even when the fixed
/// rows alone overflow the budget: the box then scrolls to the focus
/// (`search_box`) so every control stays reachable.
fn search_layout(
    m: &Model,
    width: u16,
    budget: usize,
) -> (usize, usize, Vec<search::ChipRow>, bool) {
    // Border 2 + padding 2; rows stay one cell short of the inner width so
    // taffy never wraps them (see `menu_box_is_tight_at_any_width`).
    let inner = usize::from(width).saturating_sub(5).max(8);
    let chips = search::chip_rows(&m.search, inner);
    let rows = budget.max(3) - 2;
    let area = if m.search.previews.is_empty() {
        rows.saturating_sub(3 + chips.len()).max(1)
    } else {
        rows - 1
    };
    (inner, area, chips, area < 4)
}

/// The `/search` menu (49 §6): the query editor, the wrapped chips, a status
/// row, the result window and the footer actions — or the innermost preview
/// over the same box. Sized to `budget` rows at most; nothing overflows the
/// terminal, rows are cut by display cells.
fn search_box(
    m: &Model,
    mut model: State<Model>,
    handle: Ref<TextInputHandle>,
    width: u16,
    budget: usize,
) -> AnyElement<'static> {
    let s = &m.search;
    let (inner, area, chips, compact) = search_layout(m, width, budget);
    let dim = SAND;
    let border = OCHRE;
    if let Some(p) = s.previews.last() {
        let title = search::fit(&p.title(), inner);
        let rows: Vec<AnyElement<'static>> = match p.rows(inner) {
            Some(rows) => rows
                .into_iter()
                .skip(p.scroll())
                .take(area)
                .map(|r| {
                    let color = match (p, r.chars().next()) {
                        (Preview::Changes { .. }, Some('+')) => MALACHITE,
                        (Preview::Changes { .. }, Some('-')) => CARNELIAN,
                        (Preview::Changes { .. }, Some('@')) => FAIENCE,
                        _ => BONE_DIM,
                    };
                    element!(Text(content: search::fit(&r, inner), wrap: TextWrap::NoWrap, color))
                        .into_any()
                })
                .collect(),
            None => match p {
                Preview::Artifact { body, scroll, .. } => body
                    .view()
                    .lines()
                    .skip(*scroll)
                    .take(area)
                    .map(line)
                    .collect(),
                _ => Vec::new(),
            },
        };
        let height = u16::try_from((2 + 1 + rows.len()).min(budget.max(3))).unwrap_or(u16::MAX);
        return element! {
            View(border_style: BorderStyle::Round, border_color: border, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1, width, height, overflow: Overflow::Hidden) {
                Text(content: title, color: dim, wrap: TextWrap::NoWrap)
                #(rows)
            }
        }
        .into_any();
    }
    // Every row in order, and the one the focus sits on: a box too short
    // for them all drops leading rows until that one is in view.
    let mut body: Vec<AnyElement<'static>> = Vec::with_capacity(3 + chips.len() + s.rows.len());
    let mut target = 0;
    let query_focus = s.focus == Focus::Query;
    body.push(
        element! {
            View(flex_direction: FlexDirection::Row, height: 1) {
                Text(content: "Query: ", color: if query_focus { BONE } else { dim }, weight: Weight::Bold)
                View(flex_grow: 1.0_f32, height: 1) {
                    TextInput(
                        handle,
                        has_focus: query_focus,
                        value: s.query.clone(),
                        on_change: move |v: String| model.write().search_edit(v),
                    )
                }
            }
        }
        .into_any(),
    );
    // Chips: selected ones carry a check and colour; the focused one inverts.
    for row in &chips {
        let mut contents = Vec::with_capacity(row.len() * 2);
        for (n, (i, text)) in row.iter().enumerate() {
            if n > 0 {
                contents.push(MixedTextContent::new(" "));
            }
            let on = text.contains('✓');
            let mut c = MixedTextContent::new(text).color(if on { FAIENCE } else { BONE_DIM });
            if s.focus == Focus::Filters(*i) {
                c = c.invert().weight(Weight::Bold);
                target = body.len();
            }
            contents.push(c);
        }
        body.push(element!(MixedText(contents, wrap: TextWrap::NoWrap)).into_any());
    }
    let status = search::fit(&s.status(), inner);
    body.push(
        element!(Text(content: status, color: if s.notice.is_some() { OCHRE } else { dim }, wrap: TextWrap::NoWrap))
            .into_any(),
    );
    // Result rows: the window starting at the clamped scroll.
    let first = search::first_row(&s.rows, s.scroll, s.cursor, area, compact);
    let text_fit = inner.saturating_sub(2);
    let mut used = 0;
    for (r, row) in s.rows.iter().enumerate().skip(first) {
        let h = row.height(compact);
        if used + h > area {
            break;
        }
        used += h;
        let here = r == s.cursor && row.selectable();
        if here && s.focus == Focus::Results {
            target = body.len();
        }
        let mark = if here { "› " } else { "  " };
        let color = if here { BONE } else { BONE_DIM };
        let weight = if here { Weight::Bold } else { Weight::Normal };
        match row {
            search::Row::Header(t) => body.push(
                element!(Text(content: search::fit(t, inner), color: dim, weight: Weight::Bold, wrap: TextWrap::NoWrap)).into_any(),
            ),
            search::Row::Note(t) => body.push(
                element!(Text(content: format!("  {}", search::fit(t, text_fit)), color: dim, italic: true, wrap: TextWrap::NoWrap)).into_any(),
            ),
            search::Row::More(g) => body.push(
                element!(Text(content: format!("{mark}{}", s.more_label(*g)), color: if here { BONE } else { FAIENCE }, weight, wrap: TextWrap::NoWrap)).into_any(),
            ),
            search::Row::Recent(i) => {
                let Some(sess) = s.recent.as_ref().and_then(|r| r.get(*i)) else { continue };
                let text = format!(
                    "{mark}{}  {}  {}",
                    short(sess.id),
                    sess.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
                    sess.title
                );
                body.push(element!(Text(content: search::fit(&text, inner), color, weight, wrap: TextWrap::NoWrap)).into_any());
            }
            search::Row::Hit(g, i) => {
                let Some(hit) = s.groups.get(*g).and_then(|g| g.hits.get(*i)) else { continue };
                let (title, prov) = (hit.title(), hit.provenance());
                let room = text_fit.saturating_sub(3);
                let prov_room = (room / 2).min(search::cells(&prov));
                let title = search::fit(&title, room.saturating_sub(prov_room + if prov_room > 0 { 2 } else { 0 }));
                let prov = search::fit(&prov, room.saturating_sub(search::cells(&title) + 2));
                body.push(
                    element!(MixedText(contents: vec![
                        MixedTextContent::new(format!("{mark}{title}")).color(color).weight(weight),
                        MixedTextContent::new(format!("  {prov}")).color(dim),
                    ], wrap: TextWrap::NoWrap))
                    .into_any(),
                );
                if !compact {
                    let (label, snippet, ranges) = hit.snippet();
                    let mut contents = vec![MixedTextContent::new(format!("    {label}")).color(dim)];
                    let max = text_fit.saturating_sub(2 + search::cells(label));
                    for (text, hot) in search::runs(snippet, ranges, max) {
                        let mut c = MixedTextContent::new(text).color(if hot { HEADING } else { BONE_DIM });
                        if hot {
                            c = c.weight(Weight::Bold);
                        }
                        contents.push(c);
                    }
                    body.push(element!(MixedText(contents, wrap: TextWrap::NoWrap)).into_any());
                }
            }
        }
    }
    // Footer: the actions the footer focus walks, the key legend before
    // them when the row has the room.
    let legend: &str = if m.attach_open() {
        "⇥ filters · ↑↓ results · ⏎ attach · esc close  "
    } else {
        "⇥ filters · ↑↓ results · ⏎ open · esc close  "
    };
    let actions: usize = search::FOOTER.iter().map(|a| search::cells(a) + 3).sum();
    let mut footer = Vec::with_capacity(1 + 2 * search::FOOTER.len());
    if search::cells(legend) + actions <= inner {
        footer.push(MixedTextContent::new(legend).color(dim));
    }
    for (i, action) in search::FOOTER.iter().enumerate() {
        let mut c = MixedTextContent::new(format!("[{action}]")).color(BONE_DIM);
        if s.focus == Focus::Footer(i) {
            c = c.invert().weight(Weight::Bold);
        }
        footer.push(c);
        footer.push(MixedTextContent::new(" "));
    }
    if matches!(s.focus, Focus::Footer(_)) {
        target = body.len();
    }
    body.push(element!(MixedText(contents: footer, wrap: TextWrap::NoWrap)).into_any());
    // Never taller than the transcript rows on offer: a short terminal
    // scrolls the box to the focus rather than the frame overflowing (49 §6.2).
    let shown = budget.max(3) - 2;
    let skip = (target + 1).saturating_sub(shown);
    let height = u16::try_from(2 + body.len().min(shown)).unwrap_or(u16::MAX);
    element! {
        View(border_style: BorderStyle::Round, border_color: border, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1, width, height, overflow: Overflow::Hidden) {
            #(body.into_iter().skip(skip))
        }
    }
    .into_any()
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
                    let row = agent_row(r, m.spinner(), width);
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

/// The fixed todo card (47 §6.3): the trigger row, then while open the
/// body rows in view and the overflow indicator. Session chrome beside the
/// board, never a transcript entry; F2 gives it the keys.
fn todo_card(c: TodoCard) -> AnyElement<'static> {
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
fn entry(e: &Entry, spinner: &str, skip: usize, width: u16) -> AnyElement<'static> {
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
        } => {
            let color = match kind {
                CardKind::Peer => PEER,
                CardKind::Skill => SKILL,
                CardKind::Prompt => PROMPT,
                CardKind::Job | CardKind::Schedule => JOB,
                CardKind::Link | CardKind::Node => LINK,
                CardKind::Artifact => TOOL,
                CardKind::Removed => PROMPT,
                CardKind::Vision => JOB,
            };
            let v = text.view();
            let lines = v.lines().count();
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
                        #(v.lines().map(line))
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
        Entry::Pending { name, progress, .. } => {
            let row = match progress {
                Some(p) => progress_line(name, p),
                None => format!("preparing {name}…"),
            };
            element! {
                View(margin_top: 1) {
                    Text(content: format!("{spinner} {row}"), color: SAND, italic: true)
                }
            }
            .into_any()
        }
        // Artifact card (35 §6.1): `▣ name · vN · size`, the title when it
        // differs, the path as a `file://` link; a failed show keeps its ✗ line.
        // An app card (46 §8.2) says where it runs: the terminal has its source only.
        Entry::Tool {
            artifact: Some(a),
            result,
            expanded,
            ..
        } => {
            let mut header = format!("{} {}", CardKind::Artifact.glyph(), a.name);
            if a.version > 1 {
                header.push_str(&format!(" · v{}", a.version));
            }
            // `name · N files · size` for an archive (35 §3.2): the count rides in
            // the receipt (`shown a.zip (v1 · 42 files · 3 MB)`), so the header
            // still touches no file.
            if a.kind == artifact::Kind::Archive
                && let Some((output, false)) = result
                && let Some(files) = output.split(" · ").map(|p| p.trim_end_matches(')')).find(|p| p.ends_with(" files"))
            {
                header.push_str(&format!(" · {files}"));
            }
            header.push_str(&format!(" · {}", artifact_size(a)));
            if a.kind == artifact::Kind::App {
                header.push_str(&format!(" · {APP_WHERE}"));
            }
            let error = match result {
                Some((output, true)) => Some(format!("✗ {}", result_line(output))),
                _ => None,
            };
            let hint = if expanded.is_some() {
                "ctrl+o collapses"
            } else {
                "o open · ctrl+o"
            };
            let path = a.path.display().to_string();
            let link = vec![
                MixedTextContent::new(&path)
                    .color(SAND)
                    .decoration(TextDecoration::Underline)
                    .hyperlink(format!("file://{path}")),
                MixedTextContent::new(format!("   {hint}")).color(SAND),
            ];
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: TOOL, padding_left: 1, padding_right: 1, margin_top: 1, margin_bottom: 1) {
                    Text(content: header, color: TOOL, weight: Weight::Bold)
                    #((a.title != a.name).then(|| element!(Text(content: a.title.clone(), wrap: TextWrap::Wrap))))
                    #(error.map(|e| element!(Text(content: e, color: CARNELIAN, wrap: TextWrap::Wrap))))
                    MixedText(contents: link, wrap: TextWrap::Wrap)
                    #(expanded.as_ref().map(|t| element!(View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        #(t.view().lines().map(line))
                    })))
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
            job,
            scrape,
            ..
        } => {
            // Live job output: rows clipped, not wrapped, so the block never
            // grows past `JOB_LINES` (border 2 + padding 2, then one less).
            let fit = usize::from(width).saturating_sub(5);
            let job = job.as_ref().map(|j| {
                element! {
                    View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        Text(content: j.header(), color: SAND, italic: true, wrap: TextWrap::NoWrap)
                        #(j.rows().map(|l| element!(Text(content: l.chars().take(fit).collect::<String>(), color: BONE_DIM, wrap: TextWrap::NoWrap))))
                    }
                }
            });
            let summary = match result {
                None => (
                    format!("{spinner} {}", scrape.as_deref().unwrap_or("running…")),
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
                    #(job)
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
        // 125 §7.4: the Round card in its worst severity's colour; one row per
        // note, badge coloured, text plain; three rows until Ctrl+O.
        Entry::Advisory { advisory, expanded } => {
            let color = severity_color(advisory_severity(advisory));
            let n = advisory.notes.len();
            let shown = if *expanded { n } else { n.min(ADVISORY_COLLAPSED) };
            let title = advisory_title(advisory);
            let header = if n <= ADVISORY_COLLAPSED {
                title
            } else if *expanded {
                format!("{title}   ctrl+o collapses")
            } else {
                format!("{title}   ctrl+o expands")
            };
            let rows: Vec<AnyElement<'static>> = advisory
                .notes
                .iter()
                .take(shown)
                .map(|note| {
                    element!(MixedText(contents: vec![
                        MixedTextContent::new(format!("[{}]", severity_name(note.severity))).color(severity_color(note.severity)),
                        MixedTextContent::new(format!(" {}", note.note)),
                    ], wrap: TextWrap::Wrap))
                    .into_any()
                })
                .collect();
            let more = (n > shown).then(|| format!("… +{} more notes", n - shown));
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: color, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: header, color, weight: Weight::Bold)
                    #(rows)
                    #(more.map(|m| element!(Text(content: m, color: SAND))))
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

/// The advisor palette (125 §7.4): nit SAND (muted), concern OCHRE (warning),
/// blocker CARNELIAN (danger).
fn severity_color(s: AdvisorSeverity) -> Color {
    match s {
        AdvisorSeverity::Nit => SAND,
        AdvisorSeverity::Concern => OCHRE,
        AdvisorSeverity::Blocker => CARNELIAN,
    }
}

/// The status segment's colour by advisor state (125 §7.5): idle SAND,
/// reviewing FAIENCE, paused OCHRE, halted CARNELIAN.
fn advisor_color(s: AdvisorState) -> Color {
    match s {
        AdvisorState::Off | AdvisorState::Idle => SAND,
        AdvisorState::Reviewing => FAIENCE,
        AdvisorState::Paused => OCHRE,
        AdvisorState::Halted => CARNELIAN,
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
fn line(l: &markdown::Line) -> AnyElement<'static> {
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

fn risk_name(r: Risk) -> &'static str {
    match r {
        Risk::ReadOnly => "read-only",
        Risk::Mutating => "mutating",
        Risk::Destructive => "destructive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::ModelInfo;
    use runtime::SessionId;

    /// A search view with wide-character titles and snippets, one hit per
    /// category plus a `show more` row, as the menu would show it.
    fn search_model() -> Model {
        use ::search::{MatchRange, SearchPage, TextMatch};
        use rpc::search::{SearchGroup, SearchResponse};
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = Model::default();
        m.open_search("日本語 deploy", &tx);
        let generation = m.search.generation;
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
            },
            r#match: TextMatch {
                field: "body".into(),
                snippet: "本文 deploy 本文".into(),
                ranges: vec![MatchRange { start: 3, end: 9 }],
            },
        };
        m.apply(UiEvent::Search(
            generation,
            search::Reply::Results {
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
            },
        ));
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
            m.search.focus = focus;
            m
        });
        let handle = hooks.use_ref_default::<TextInputHandle>();
        let m = model.read();
        search_box(&m, model, handle, props.width, props.budget)
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
    /// the list) at some widths; the box must be title + rows + border.
    #[test]
    fn menu_box_is_tight_at_any_width() {
        let mut m = Model::default();
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
            })
            .collect();
        // The provider's models level lists all ten rows (124 §3.2).
        m.menu = Some(Menu::Models {
            items,
            filter: String::new(),
            cursor: 0,
            provider: Some("amazon-bedrock".into()),
            target: PickTarget::Session,
        });
        for width in 60..=120u16 {
            let modal = menu(&m, "", width, 30).unwrap();
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
        use agent::{Advisory, AdvisoryNote};
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
        let card = |n: usize, expanded: bool| Entry::Advisory {
            advisory: Advisory {
                advisor: runtime::SessionId::new(),
                step: 1,
                turn: None,
                at_ms: 0,
                notes: notes(n),
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
                        #(Some(entry(&e, "⠋", 0, width)))
                    }
                }
                .to_string();
                let lines: Vec<&str> = frame.lines().collect();
                let top = lines.iter().position(|l| l.starts_with('╭')).unwrap();
                let bottom = lines.iter().position(|l| l.starts_with('╰')).unwrap();
                let body = 1 + rows + usize::from(more.is_some());
                assert_eq!(bottom - top, body + 1, "width {width}:\n{frame}");
                assert!(lines.iter().all(|l| l.chars().count() <= usize::from(width)));
                assert!(lines[top + 1].contains("◎ advisor ·"), "{frame}");
                assert!(lines[top + 2].contains("[nit]") && lines[top + 3].contains("[concern]"));
                if let Some(more) = more {
                    assert!(lines[top + body].contains(more), "{frame}");
                }
            }
        }
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
                #(entry(&e, "", 0, 60))
            }
        }
        .to_string();
        assert!(frame.contains("spoken 5 s · cut in"), "{frame}");
        assert!(frame.contains("hello"), "{frame}");
        assert!(!frame.contains("[spoken"), "{frame}");
    }

    /// 109 §10.3: the resume divider takes the `Compacted` shape — a
    /// bordered `── label ──` line, 3 rows (`Entry::height` agrees).
    #[test]
    fn resume_divider_takes_the_compacted_shape() {
        let e = Entry::Divider("resumed after restart".into());
        let frame = element! {
            View(flex_direction: FlexDirection::Column, width: 60u16) {
                #(entry(&e, "", 0, 60))
            }
        }
        .to_string();
        assert!(frame.contains("── resumed after restart ──"), "{frame}");
        assert!(frame.contains('╭') && frame.contains('╰'), "{frame}");
        assert_eq!(
            frame.trim_end_matches('\n').lines().count(),
            4,
            "margin + 3 rows: {frame}"
        );
    }
}
