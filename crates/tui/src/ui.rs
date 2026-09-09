//! Fullscreen iocraft app. Pure: reads [`UiEvent`]s from a channel, writes
//! [`Command`]s to another. All actor traffic lives in `lib.rs`.

use std::sync::Arc;

use iocraft::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tools::{Decision, Risk};

use crate::model::{
    Command, Entry, Level, Menu, MenuKey, Model, Row, UiEvent, describe, gate_name, k, result_line,
    short, slash_matches, window,
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
}

/// Rows a menu shows at most.
const MENU_ROWS: usize = 10;
/// Human turn cards.
const USER: Color = Color::AnsiValue(117);
/// Tool call cards.
const TOOL: Color = Color::Cyan;
/// Peer (subagent / parent) message cards.
const PEER: Color = Color::Magenta;

/// Run the fullscreen UI until the user quits. Mouse capture stays off so
/// the terminal's own selection (and copy-on-select) keeps working.
pub async fn run(links: Links) -> std::io::Result<()> {
    element! {
        ContextProvider(value: Context::owned(links)) {
            App
        }
    }
    .fullscreen()
    .disable_mouse_capture()
    .await
}

#[component]
fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut links = hooks.use_context_mut::<Links>();
    let default_model = links.default_model.clone();
    let approvals = links.approvals;
    let mut model = hooks.use_state(move || Model::new(default_model, approvals));
    let mut input = hooks.use_state(String::new);
    let commands = links.commands.clone();
    let catalog = links.catalog.clone();

    // Runs once: the receiver is `Some` only on the first render.
    let events = links.events.take();
    let resume = links.resume.take();
    hooks.use_future({
        let commands = commands.clone();
        async move {
            let Some(mut events) = events else { return };
            // Kitty keyboard protocol: without it Shift+Enter is a plain `\r`.
            // Pushed after iocraft entered the alternate screen (separate stack).
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PushKeyboardEnhancementFlags(
                    crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                )
            );
            if let Some(id) = resume {
                model.write().resume(id, &commands);
            }
            while let Some(ev) = events.recv().await {
                model.write().apply(ev);
            }
        }
    });

    // Typewriter: `read` does not schedule a render, so idle frames are free.
    hooks.use_future(async move {
        let mut frames = tokio::time::interval(std::time::Duration::from_millis(16));
        loop {
            frames.tick().await;
            if model.read().revealing() {
                model.write().tick();
            }
        }
    });

    hooks.use_terminal_events({
        let commands = commands.clone();
        move |event| {
            let TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                return;
            };
            if kind == KeyEventKind::Release {
                return;
            }
            let mut m = model.write();
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                m.quit = true;
                return;
            }
            if code == KeyCode::Char('o') && modifiers.contains(KeyModifiers::CONTROL) {
                m.toggle_delivered();
                return;
            }
            if m.approval().is_some() {
                match code {
                    KeyCode::Char('y') => m.decide(Decision::Allow, &commands),
                    KeyCode::Char('n') | KeyCode::Esc => m.decide(Decision::Deny, &commands),
                    _ => {}
                }
                return;
            }
            // Shift+Enter: the input inserts the newline itself.
            if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
                return;
            }
            if m.menu.is_some() {
                let text = input.read().to_string();
                if let MenuKey::Input(text) = m.menu_key(code, &text, &commands) {
                    input.set(text);
                }
                return;
            }
            match code {
                KeyCode::Enter => {
                    let line = std::mem::take(&mut *input.write());
                    m.submit(&line, &commands);
                }
                KeyCode::Esc if m.focus.is_some() && input.read().is_empty() => {
                    m.focus("", &commands);
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
            crossterm::event::PopKeyboardEnhancementFlags
        );
        system.exit();
    }
    let modal = m.approval().map(|req| {
        let agent = m
            .peer_name(req.session)
            .map_or_else(String::new, |n| format!("  [agent: {n}]"));
        element! {
            View(border_style: BorderStyle::Double, border_color: Color::Yellow, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: format!("approval: {}  ({}){agent}", req.tool, risk_name(req.risk)), weight: Weight::Bold)
                #(describe(&req.tool, &req.args).into_iter().map(|l| element!(Text(content: l, color: Color::Cyan, wrap: TextWrap::Wrap))))
                Text(content: "[y] allow  [n/esc] deny", color: Color::Yellow)
            }
        }
        .into_any()
    });
    // Approval on top; otherwise whichever menu is open.
    let modal = modal.or_else(|| menu(&m, &input.read()));
    let has_focus = m.approval().is_none()
        && !matches!(
            m.menu,
            Some(Menu::Models { .. } | Menu::Sessions { .. } | Menu::Mcp { .. })
        );
    let (header, level) = m.header(&catalog);
    let header_color = match level {
        Level::Normal => Color::DarkGrey,
        Level::Warn => Color::Yellow,
        Level::Danger => Color::Red,
    };
    let (width, height) = hooks.use_terminal_size();

    element! {
        View(flex_direction: FlexDirection::Column, width, height) {
            View(flex_grow: 1.0, min_height: 0, flex_direction: FlexDirection::Column) {
                ScrollView(auto_scroll: true, scrollbar: Some(false)) {
                    View(flex_direction: FlexDirection::Column, width: 100pct) {
                        #(m.entries.iter().map(entry))
                    }
                }
            }
            #(modal)
            View(height: 1, padding_left: 1) {
                Text(content: header, color: header_color)
            }
            View(border_style: BorderStyle::Round, border_color: Color::Blue, min_height: 3) {
                TextInput(
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
            View(height: 1, padding_left: 1) {
                Text(content: m.status(), color: Color::DarkGrey)
            }
        }
    }
}

/// The open menu as a bordered box; `None` without one.
fn menu(m: &Model, input: &str) -> Option<AnyElement<'static>> {
    let (title, rows, cursor): (String, Vec<String>, usize) = match m.menu.as_ref()? {
        Menu::Slash { cursor } => {
            let word = input.split(' ').next().unwrap_or_default();
            let rows = slash_matches(word)
                .into_iter()
                .map(|(name, hint, desc)| format!("{:<16} {desc}", format!("{name} {hint}")))
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
                .map(|i| format!("{:<44} {:<32} {}", i.id, i.name, k(i.context)))
                .collect(),
            *cursor,
        ),
        Menu::Sessions { items, cursor } => (
            "sessions  ⏎ switch  esc close  (the one you leave keeps running)".to_owned(),
            items
                .iter()
                .map(|s| {
                    // ponytail: fixed 40-char title; pass the terminal width if it grates.
                    let title: String = s.title.chars().take(32).collect();
                    format!(
                        "{} {}  {}  {:<20} {}",
                        if Some(s.id) == m.session { "●" } else { " " },
                        short(s.id),
                        s.created_at
                            .with_timezone(&chrono::Local)
                            .format("%m-%d %H:%M"),
                        s.model.rsplit('/').next().unwrap_or(&s.model),
                        title
                    )
                })
                .collect(),
            *cursor,
        ),
        Menu::Mcp {
            cursor, expanded, ..
        } => (
            "mcp  ␣ cycle gate  ⌫ inherit  →/← expand".to_owned(),
            m.mcp_rows()
                .iter()
                .map(|r| match r {
                    Row::Server(s) => format!(
                        "{} {:<16} {:>2} tools   {:<6} {}",
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
                        gate_name(s.gate),
                        if s.connected {
                            "● connected"
                        } else {
                            "○ not connected"
                        },
                    ),
                    Row::Tool(_, t) => format!(
                        "    {:<24} {:<7}{}",
                        t.name,
                        t.gate.map_or_else(
                            || format!("({})", gate_name(t.effective)),
                            |g| gate_name(g).to_owned()
                        ),
                        if t.registered || t.effective == tools::PolicyAction::Deny {
                            ""
                        } else {
                            "  not registered"
                        },
                    ),
                })
                .collect(),
            *cursor,
        ),
    };
    let cursor = cursor.min(rows.len().saturating_sub(1));
    let shown = window(rows.len(), cursor, MENU_ROWS);
    Some(
        element! {
            View(border_style: BorderStyle::Round, border_color: Color::DarkGrey, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: title, color: Color::DarkGrey)
                #(rows[shown.clone()].iter().enumerate().map(|(i, row)| {
                    let here = shown.start + i == cursor;
                    element!(Text(
                        content: format!("{} {row}", if here { "›" } else { " " }),
                        wrap: TextWrap::NoWrap,
                        color: if here { Color::White } else { Color::Grey },
                        weight: if here { Weight::Bold } else { Weight::Normal },
                    ))
                }))
            }
        }
        .into_any(),
    )
}

fn entry(e: &Entry) -> AnyElement<'static> {
    match e {
        // Human turns: the same card shape as tools and peers, own colour and glyph.
        Entry::User(t) => element! {
            View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: USER, padding_left: 1, padding_right: 1, margin_top: 1) {
                Text(content: "◆ you", color: USER, weight: Weight::Bold)
                Text(content: t.clone(), wrap: TextWrap::Wrap)
            }
        }
        .into_any(),
        // Peer traffic: a card in the peer colour, body hidden until Ctrl+O.
        Entry::Delivered {
            from,
            text,
            expanded,
        } => {
            let v = text.view();
            let lines = v.lines().count();
            let header = if *expanded {
                format!("⟵ {from}   ctrl+o collapses")
            } else {
                format!("⟵ {from}   {lines} lines · ctrl+o expands")
            };
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: PEER, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: header, color: PEER, weight: Weight::Bold)
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
                    #(v.lines().map(line))
                    #((!v.plain.is_empty() && v.has_blocks()).then(|| element!(Text(content: " "))))
                    #((!v.plain.is_empty()).then(|| element!(Text(content: v.plain.to_owned(), wrap: TextWrap::Wrap))))
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
            element!(Text(content: shown, color: Color::DarkGrey, italic: true, wrap: TextWrap::Wrap)).into_any()
        }
        // Tool calls: a card in the tool colour; name, arguments, then result.
        Entry::Tool {
            name,
            args,
            result,
            ..
        } => element! {
            View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: TOOL, padding_left: 1, padding_right: 1, margin_top: 1, margin_bottom: 1) {
                Text(content: format!("⚙ {name}"), color: TOOL, weight: Weight::Bold)
                #(args.iter().map(|l| element!(Text(content: l.clone(), wrap: TextWrap::Wrap))))
                #(result.as_ref().map(|(output, is_error)| element!(Text(
                    content: format!("{} {}", if *is_error { "✗" } else { "✓" }, result_line(output)),
                    color: if *is_error { Color::Red } else { Color::Green },
                    wrap: TextWrap::Wrap,
                ))))
            }
        }
        .into_any(),
        Entry::Info(t) => element!(Text(content: t.clone(), color: Color::DarkGrey)).into_any(),
        Entry::Error(t) => element!(Text(content: t.clone(), color: Color::Red)).into_any(),
    }
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
