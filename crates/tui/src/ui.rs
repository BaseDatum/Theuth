//! Fullscreen iocraft app. Pure: reads [`UiEvent`]s from a channel, writes
//! [`Command`]s to another. All actor traffic lives in `lib.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use agent::CompactStyle;
use iocraft::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tools::{Decision, Risk};

use crate::clip;
use crate::model::{
    AcpRow, CardKind, Command, Entry, Level, Menu, MenuKey, Model, Row, Switch, TurnAction,
    UiEvent, action_name, agent_row, describe, gate_name, k, result_line, short, slash_matches,
    window,
};

/// Channels handed to the app through iocraft context (owned so the receiver
/// can be taken out on first render, per design doc §11.5).
pub struct Links {
    /// Taken by the first render into `use_future`.
    pub events: Option<UnboundedReceiver<UiEvent>>,
    /// Command sink.
    pub commands: UnboundedSender<Command>,
    /// `agent.model` from config: what the header shows before the first turn.
    pub default_model: String,
    /// `tools.approvals` from config.
    pub approvals: bool,
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
/// Human turn cards.
const USER: Color = Color::AnsiValue(117);
/// Tool call cards.
const TOOL: Color = Color::Cyan;
/// Peer (subagent / parent) message cards.
const PEER: Color = Color::Magenta;
/// Skill body cards.
/// Markdown headings in assistant text.
const HEADING: Color = Color::Yellow;
const SKILL: Color = Color::Yellow;
/// System prompt cards.
const PROMPT: Color = Color::DarkGrey;
/// Finished background job cards.
const JOB: Color = Color::Green;

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
    let approvals = links.approvals;
    let catalog = links.catalog.clone();
    let mut model = hooks.use_state({
        let catalog = catalog.clone();
        move || {
            let mut m = Model::new(default_model, approvals);
            m.catalog = Some(catalog);
            m
        }
    });
    let mut input = hooks.use_state(String::new);
    let handle = hooks.use_ref_default::<TextInputHandle>();
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
                if pending.is_empty() && !model.read().animating() {
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
                // A withdrawn steer comes back into the (empty) editor.
                if let Some(text) = m.recall.take() {
                    input.set(text);
                }
                if std::mem::take(&mut m.refresh_mcp) {
                    let _ = commands.send(Command::ListMcp(m.session));
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
            let TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                if let TerminalEvent::Paste(text) = event
                    && m.input_focused()
                {
                    // A dropped image path attaches the file (17 §4.2).
                    let s = match clip::dropped_file(&text) {
                        Some(img) => m.attach(img),
                        None => m.paste(&text),
                    };
                    insert_at_cursor(&mut m, &s);
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
            if code == KeyCode::Char('o') && modifiers.contains(KeyModifiers::CONTROL) {
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
            if m.ask().is_some() {
                let text = input.read().to_string();
                if let Some(text) = m.ask_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            // Shift+Enter: the input inserts the newline itself.
            if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
                return;
            }
            // Alt+Up: pull the newest queued steer back into the editor.
            if code == KeyCode::Up && modifiers.contains(KeyModifiers::ALT) {
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
            if m.menu.is_some() {
                let text = input.read().to_string();
                if let MenuKey::Input(text) = m.menu_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            // Transcript scrolling. ↑/↓ are the editor's while it spans
            // lines; terminals without mouse capture send them for the wheel.
            let (w, h) = terminal_size;
            let view = view_rows(&m, h, &input.read());
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
                    m.submit(&line, &commands);
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
            View(border_style: BorderStyle::Double, border_color: Color::Yellow, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: format!("approval: {}  ({}){agent}", req.tool, risk_name(req.risk)), weight: Weight::Bold)
                #(describe(&req.tool, &req.args).into_iter().map(|l| element!(Text(content: l, color: Color::Cyan, wrap: TextWrap::Wrap))))
                Text(content: format!("[y] allow  [n/esc] deny{open}"), color: Color::Yellow)
            }
        }
        .into_any()
    });
    // Approval on top, then the model's questions; otherwise whichever menu is open.
    let (width, height) = terminal_size;
    let modal = modal
        .or_else(|| ask_box(&m))
        .or_else(|| menu(&m, &input.read(), width));
    let has_focus = m.input_focused();
    let (header, level) = m.header(&catalog);
    let header_color = match level {
        Level::Normal => Color::DarkGrey,
        Level::Warn => Color::Yellow,
        Level::Danger => Color::Red,
    };

    // Only the slice that can reach the screen is built; see `Model::window`.
    let view = view_rows(&m, height, &input.read());
    let win = m.window(view, usize::from(width));
    let entries = m.entries[win.from..win.to]
        .iter()
        .enumerate()
        .map(|(i, e)| entry(e, m.spinner(), if i == 0 { win.skip } else { 0 }, width));
    let transcript = if m.anchor.is_some() {
        // Anchored: the first `top` rows of the first entry sit above the fold.
        element! {
            View(overflow: Overflow::Hidden, width: 100pct, height: 100pct) {
                View(position: Position::Absolute, top: -(win.top as i32), width: 100pct, flex_direction: FlexDirection::Column) {
                    #(entries)
                }
            }
        }
    } else {
        // Pinned: bottom-aligned, so an over-tall slice is clipped at the top.
        element! {
            View(overflow: Overflow::Hidden, width: 100pct, height: 100pct, flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd) {
                #(entries)
            }
        }
    };
    element! {
        View(flex_direction: FlexDirection::Column, width, height) {
            View(flex_grow: 1.0, min_height: 0, flex_direction: FlexDirection::Column) {
                #(transcript)
            }
            // Activity and steers not yet in the context sit under the
            // transcript, in view whatever you scrolled to.
            #(m.waiting().then(|| element!(View(margin_top: 1) {
                Text(content: format!("{} thinking…", m.spinner()), color: Color::DarkGrey, italic: true)
            })))
            #(m.queued.iter().map(|q| element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: Color::DarkGrey, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: "◇ queued · ⌥↑ edit", color: Color::DarkGrey, weight: Weight::Bold)
                    Text(content: queued_text(&q.text), color: Color::DarkGrey, wrap: TextWrap::Wrap)
                }
            }))
            #(board(&m, width))
            #(modal)
            #(m.prompt.as_ref().map(|p| {
                element!(View(height: 1, padding_left: 1) {
                    Text(content: p.hint(), color: Color::Yellow)
                })
            }))
            View(height: 1, padding_left: 1) {
                Text(content: header, color: header_color)
            }
            // `flex_shrink: 0`: once the transcript fills the screen the box
            // would otherwise be squeezed back to `min_height` as it grows.
            View(border_style: BorderStyle::Round, border_color: Color::Blue, min_height: 3, flex_shrink: 0.0) {
                TextInput(
                    handle,
                    has_focus,
                    value: input.to_string(),
                    multiline: true,
                    auto_grow: true,
                    newline_modifiers: KeyModifiers::SHIFT,
                    on_change: move |v: String| {
                        model.write().sync_slash(&v);
                        input.set(v);
                    },
                )
            }
            View(height: 1, padding_left: 1, padding_right: 1, justify_content: JustifyContent::SpaceBetween) {
                Text(content: m.status(), color: Color::DarkGrey)
                Text(content: m.mcp_status(), color: Color::DarkGrey)
            }
        }
    }
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
                color: if here { Color::White } else { Color::Grey },
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
            View(border_style: BorderStyle::Double, border_color: Color::Magenta, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: title, weight: Weight::Bold)
                Text(content: q.question.clone(), color: Color::Cyan, wrap: TextWrap::Wrap)
                #(rows)
                Text(content: own, color: if a.cursor == n || a.typing { Color::White } else { Color::Grey }, weight: if a.cursor == n || a.typing { Weight::Bold } else { Weight::Normal })
                #((!keys.is_empty()).then(|| element!(Text(content: keys, color: Color::Magenta))))
            }
        }
        .into_any(),
    )
}

/// The open menu as a bordered box; `None` without one. Rows are clipped to
/// one less than the box's inner width: a `NoWrap` row wider than the
/// terminal makes the terminal wrap the frame, and one exactly as wide makes
/// taffy hand the box several extra rows (see `menu_box_is_tight_at_any_width`).
fn menu(m: &Model, input: &str, width: u16) -> Option<AnyElement<'static>> {
    // Char ranges to accent per row; only `/search` fills it.
    let mut marks: Vec<Vec<std::ops::Range<usize>>> = Vec::new();
    let (title, rows, cursor): (String, Vec<String>, usize) = match m.menu.as_ref()? {
        Menu::Slash { cursor } => {
            let word = input.split(' ').next().unwrap_or_default();
            let rows = slash_matches(word)
                .into_iter()
                .map(|(name, hint, desc)| format!("{:<30} {desc}", format!("{name} {hint}")))
                .collect::<Vec<_>>();
            (
                "↑↓ move  ⇥ complete  ⏎ run  esc close".to_owned(),
                rows,
                *cursor,
            )
        }
        Menu::Models { filter, cursor, .. } => (
            format!("model  filter: {filter}▏"),
            m.models_filtered()
                .iter()
                .map(|i| {
                    format!(
                        "{:<44} {:<32} {}{}",
                        i.id,
                        i.name,
                        k(i.context),
                        if i.image { " · vision" } else { "" }
                    )
                })
                .collect(),
            *cursor,
        ),
        Menu::Efforts { items, cursor } => (
            "effort  ⏎ set  esc close".to_owned(),
            items.clone(),
            *cursor,
        ),
        Menu::Sessions { items, cursor } => (
            "sessions  ⏎ switch  esc close  (the one you leave keeps running)".to_owned(),
            items
                .iter()
                .map(|s| {
                    // Row prefix (mark, id, stamp, model, gaps, box) is ~52 columns; the title gets the rest.
                    let room = usize::from(width).saturating_sub(52).max(16);
                    let title: String = s.title.chars().take(room).collect();
                    format!(
                        "{} {}  {}  {:<20} {}",
                        if Some(s.id) == m.session { "●" } else { " " },
                        short(s.id),
                        s.updated_at
                            .with_timezone(&chrono::Local)
                            .format("%m-%d %H:%M"),
                        s.model.rsplit('/').next().unwrap_or(&s.model),
                        title
                    )
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
        Menu::Search { query, hits, cursor } => {
            let rows: Vec<String> = hits
                .iter()
                .map(|h| {
                    let name = if h.title.is_empty() { short(h.session) } else { h.title.clone() };
                    let at = name.chars().count() + 3;
                    marks.push(
                        h.ranges
                            .iter()
                            .map(|r| at + r.start as usize..at + r.end as usize)
                            .collect(),
                    );
                    // Char-for-char so the ranges still line up.
                    let snippet: String = h
                        .snippet
                        .chars()
                        .map(|c| if c.is_whitespace() { ' ' } else { c })
                        .collect();
                    format!("{name} · {snippet}")
                })
                .collect();
            (
                format!(
                    "search: {query}  ({} hits)  ⏎ switch  esc close",
                    hits.len()
                ),
                if rows.is_empty() {
                    vec!["no matches".to_owned()]
                } else {
                    rows
                },
                *cursor,
            )
        }
        Menu::Agents { items, cursor } => (
            "agents  ⏎ open  esc close  (the one you leave keeps running)".to_owned(),
            items
                .iter()
                .map(|a| agent_row(a, m.spinner(), width))
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
    };
    let cursor = cursor.min(rows.len().saturating_sub(1));
    let shown = window(rows.len(), cursor, MENU_ROWS);
    // border 2 + padding 2 + cursor glyph 2, then one less.
    let fit = usize::from(width).saturating_sub(7);
    Some(
        element! {
            View(border_style: BorderStyle::Round, border_color: Color::DarkGrey, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: title, color: Color::DarkGrey)
                #(rows[shown.clone()].iter().enumerate().map(|(i, row)| {
                    let here = shown.start + i == cursor;
                    let color = if here { Color::White } else { Color::Grey };
                    let weight = if here { Weight::Bold } else { Weight::Normal };
                    let text: String = row.chars().take(fit).collect();
                    match marks.get(shown.start + i).filter(|r| !r.is_empty()) {
                        None => element!(Text(
                            content: format!("{} {text}", if here { "›" } else { " " }),
                            wrap: TextWrap::NoWrap,
                            color,
                            weight,
                        )).into_any(),
                        Some(ranges) => {
                            let run = |s: String, hit: bool| {
                                MixedTextContent::new(s)
                                    .color(if hit { HEADING } else { color })
                                    .weight(if hit { Weight::Bold } else { weight })
                            };
                            let chars: Vec<char> = text.chars().collect();
                            let mut contents = vec![run(format!("{} ", if here { "›" } else { " " }), false)];
                            let mut at = 0;
                            for r in ranges {
                                let (s, e) = (r.start.max(at), r.end.min(chars.len()));
                                if s >= e {
                                    continue;
                                }
                                if s > at {
                                    contents.push(run(chars[at..s].iter().collect(), false));
                                }
                                contents.push(run(chars[s..e].iter().collect(), true));
                                at = e;
                            }
                            if at < chars.len() {
                                contents.push(run(chars[at..].iter().collect(), false));
                            }
                            element!(MixedText(contents, wrap: TextWrap::NoWrap)).into_any()
                        }
                    }
                }))
            }
        }
        .into_any(),
    )
}

/// Rows the transcript pane gets: the terminal minus everything drawn
/// around it (header, status, editor box, prompt line, activity row, queued
/// chips, board). An estimate by the same rules the layout follows; a row or
/// two off only moves the first scroll step, never the pinned bottom.
fn view_rows(m: &Model, height: u16, input: &str) -> usize {
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
        .saturating_sub(2 + editor + usize::from(m.prompt.is_some()) + activity + queued + board)
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
            View(border_style: BorderStyle::Round, border_color: Color::DarkGrey, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: "agents", color: Color::DarkGrey)
                #(rows.iter().take(BOARD_ROWS).map(|r| {
                    let row = agent_row(r, m.spinner(), width);
                    let (glyph, rest) = split_chars(&row, 2);
                    let (name, rest) = split_chars(rest, 15);
                    element! {
                        View(flex_direction: FlexDirection::Row) {
                            Text(content: glyph.to_owned(), color: Color::Grey, wrap: TextWrap::NoWrap)
                            Text(content: name.to_owned(), color: PEER, wrap: TextWrap::NoWrap)
                            Text(content: rest.chars().take(fit.saturating_sub(17)).collect::<String>(), color: Color::Grey, wrap: TextWrap::NoWrap)
                        }
                    }
                }))
                #((more > 0).then(|| element!(Text(content: format!("… {more} more (/agents)"), color: Color::DarkGrey))))
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
fn entry(e: &Entry, spinner: &str, skip: usize, width: u16) -> AnyElement<'static> {
    match e {
        // Human turns: the same card shape as tools and peers, own colour and glyph.
        Entry::User { text: t, .. } => element! {
            View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: USER, padding_left: 1, padding_right: 1, margin_top: 1) {
                Text(content: "◆ you", color: USER, weight: Weight::Bold)
                #(plain(t, TextWrap::Wrap, |c| c))
            }
        }
        .into_any(),
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
                CardKind::Job => JOB,
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
            plain(&shown, TextWrap::Wrap, |c| c.color(Color::DarkGrey).italic())
        }
        Entry::Pending { name, .. } => element! {
            View(margin_top: 1) {
                Text(content: format!("{} preparing {name}…", spinner), color: Color::DarkGrey, italic: true)
            }
        }
        .into_any(),
        // Tool calls: a card in the tool colour; name, arguments, then result.
        Entry::Tool {
            name,
            args,
            result,
            expanded,
            job,
            ..
        } => {
            // Live job output: rows clipped, not wrapped, so the block never
            // grows past `JOB_LINES` (border 2 + padding 2, then one less).
            let fit = usize::from(width).saturating_sub(5);
            let job = job.as_ref().map(|j| {
                element! {
                    View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        Text(content: j.header(), color: Color::DarkGrey, italic: true, wrap: TextWrap::NoWrap)
                        #(j.rows().map(|l| element!(Text(content: l.chars().take(fit).collect::<String>(), color: Color::Grey, wrap: TextWrap::NoWrap))))
                    }
                }
            });
            let summary = match result {
                None => (format!("{spinner} running…"), Color::DarkGrey),
                Some((output, is_error)) => {
                    let mark = if *is_error { "✗" } else { "✓" };
                    let hint = match (expanded.is_some(), output.lines().count() > 1) {
                        (true, _) => "   ctrl+o collapses",
                        (false, true) => "   ctrl+o expands",
                        (false, false) => "",
                    };
                    (
                        format!("{mark} {}{hint}", result_line(output)),
                        if *is_error { Color::Red } else { Color::Green },
                    )
                }
            };
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: TOOL, padding_left: 1, padding_right: 1, margin_top: 1, margin_bottom: 1) {
                    Text(content: format!("⚙ {name}"), color: TOOL, weight: Weight::Bold)
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
        Entry::Info(t) => plain(t, TextWrap::Wrap, |c| c.color(Color::DarkGrey)),
        Entry::Error(t) => plain(t, TextWrap::Wrap, |c| c.color(Color::Red)),
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
                c = c.color(Color::DarkGrey);
            }
            if s.style.heading > 0 {
                c = c.color(HEADING);
                if s.style.heading <= 2 {
                    c = c.decoration(TextDecoration::Underline);
                }
            }
            if s.style.code {
                c = c.color(Color::Cyan);
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
        m.menu = Some(Menu::Models {
            items,
            filter: String::new(),
            cursor: 0,
        });
        for width in 60..=120u16 {
            let modal = menu(&m, "", width).unwrap();
            let frame = element! {
                View(flex_direction: FlexDirection::Column, width, height: 30u16) {
                    View(flex_grow: 1.0, min_height: 0)
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
            }));
        }
        for width in 60..=120u16 {
            let frame = element! {
                View(flex_direction: FlexDirection::Column, width, height: 30u16) {
                    View(flex_grow: 1.0, min_height: 0)
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
}
