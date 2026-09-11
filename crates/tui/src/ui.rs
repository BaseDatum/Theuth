//! Fullscreen iocraft app. Pure: reads [`UiEvent`]s from a channel, writes
//! [`Command`]s to another. All actor traffic lives in `lib.rs`.

use std::sync::Arc;

use iocraft::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tools::{Decision, Risk};

use crate::model::{
    CardKind, Command, Entry, Level, Menu, MenuKey, Model, Row, UiEvent, action_name, describe,
    gate_name, k, result_line, short, slash_matches, window,
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

    // Runs once: the receiver is `Some` only on the first render.
    let events = links.events.take();
    let resume = links.resume.take();
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
            while let Some(ev) = events.recv().await {
                let mut m = model.write();
                m.apply(ev);
                // A withdrawn steer comes back into the (empty) editor.
                if let Some(text) = m.recall.take() {
                    input.set(text);
                }
            }
        }
    });

    // Animation: `read` does not schedule a render, so idle frames are free.
    hooks.use_future(async move {
        let mut frames = tokio::time::interval(std::time::Duration::from_millis(16));
        loop {
            frames.tick().await;
            if model.read().animating() {
                model.write().tick();
            }
        }
    });

    hooks.use_terminal_events({
        let commands = commands.clone();
        move |event| {
            let mut m = model.write();
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
                    // At the cursor; `TextInput` sees an insertion there and
                    // moves its cursor past it.
                    let mut v = input.read().to_string();
                    let at = handle.read().cursor_offset().min(v.len());
                    v.insert_str(at, &m.paste(&text));
                    m.sync_slash(&v);
                    input.set(v);
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
            // Alt+Up: pull the newest queued steer back into the editor.
            if code == KeyCode::Up && modifiers.contains(KeyModifiers::ALT) {
                m.withdraw(input.read().is_empty(), &commands);
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
                KeyCode::Esc if m.prompt.is_some() => m.cancel_prompt(),
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
            crossterm::event::PopKeyboardEnhancementFlags,
            crossterm::event::DisableBracketedPaste
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
    let (width, height) = hooks.use_terminal_size();
    let modal = modal.or_else(|| menu(&m, &input.read(), width));
    let has_focus = m.input_focused();
    let (header, level) = m.header(&catalog);
    let header_color = match level {
        Level::Normal => Color::DarkGrey,
        Level::Warn => Color::Yellow,
        Level::Danger => Color::Red,
    };

    // Only the tail that can reach the screen is built; see `Model::tail`.
    let from = m.tail(usize::from(height) * 8);
    element! {
        View(flex_direction: FlexDirection::Column, width, height) {
            View(flex_grow: 1.0, min_height: 0, flex_direction: FlexDirection::Column) {
                ScrollView(auto_scroll: true, scrollbar: Some(false)) {
                    View(flex_direction: FlexDirection::Column, width: 100pct) {
                        #(m.entries[from..].iter().map(|e| entry(e, m.spinner())))
                        #(m.waiting().then(|| element!(View(margin_top: 1) {
                            Text(content: format!("{} thinking…", m.spinner()), color: Color::DarkGrey, italic: true)
                        })))
                        // Steers not yet in the context: below the transcript,
                        // never among its entries, until `InputInjected`.
                        #(m.queued.iter().map(|q| element! {
                            View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: Color::DarkGrey, padding_left: 1, padding_right: 1, margin_top: 1) {
                                Text(content: "◇ queued · ⌥↑ edit", color: Color::DarkGrey, weight: Weight::Bold)
                                Text(content: q.text.clone(), color: Color::DarkGrey, wrap: TextWrap::Wrap)
                            }
                        }))
                    }
                }
            }
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

/// The open menu as a bordered box; `None` without one. Rows are clipped to
/// one less than the box's inner width: a `NoWrap` row wider than the
/// terminal makes the terminal wrap the frame, and one exactly as wide makes
/// taffy hand the box several extra rows (see `menu_box_is_tight_at_any_width`).
fn menu(m: &Model, input: &str, width: u16) -> Option<AnyElement<'static>> {
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
            "mcp  ␣ cycle risk  ⌫ annotations  →/← expand  l login/logout  s sync".to_owned(),
            m.mcp_rows()
                .iter()
                .map(|r| match r {
                    Row::Server(s) => format!(
                        "{} {:<16} {:>2} tools   {:<18} {}{}",
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
                        s.gate.map_or_else(
                            || "annotations".to_owned(),
                            |g| format!("risk: {}", gate_name(g))
                        ),
                        match (s.connected, s.auth) {
                            (true, _) => "● connected",
                            (false, tools::McpAuth::Required) => "○ login required",
                            (false, _) => "○ not connected",
                        },
                        s.repo
                            .as_deref()
                            .map_or_else(String::new, |r| format!("   ⟳ {r}")),
                    ),
                    // `mutating (ask)`: effective risk, then what `tools.policy`
                    // does with it (12 §5). `*` marks the tool's own line.
                    Row::Tool(_, t) => format!(
                        "    {:<24} {:<22}{}",
                        t.name,
                        match t.gate {
                            Some(tools::Gate::Off) => "off".to_owned(),
                            Some(_) =>
                                format!("{}* ({})", risk_name(t.risk), action_name(t.action)),
                            None => format!("{} ({})", risk_name(t.risk), action_name(t.action)),
                        },
                        match (t.gate, t.registered) {
                            (Some(tools::Gate::Off), _) => "  hidden",
                            (_, false) => "  not registered",
                            (_, true) => "",
                        },
                    ),
                })
                .collect(),
            *cursor,
        ),
        Menu::Skills { items, cursor } => (
            "skills  ⏎ show  esc close".to_owned(),
            items
                .iter()
                .map(|s| {
                    // ponytail: fixed 48-char description; pass the terminal width if it grates.
                    let desc: String = s.description.chars().take(48).collect();
                    format!(
                        "{:<20} {:<48} {:<16} {}",
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
                    element!(Text(
                        content: format!("{} {}", if here { "›" } else { " " }, row.chars().take(fit).collect::<String>()),
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

fn entry(e: &Entry, spinner: &str) -> AnyElement<'static> {
    match e {
        // Human turns: the same card shape as tools and peers, own colour and glyph.
        Entry::User(t) => element! {
            View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: USER, padding_left: 1, padding_right: 1, margin_top: 1) {
                Text(content: "◆ you", color: USER, weight: Weight::Bold)
                Text(content: t.clone(), wrap: TextWrap::Wrap)
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
            ..
        } => {
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
            element! {
                View(flex_direction: FlexDirection::Column, border_style: BorderStyle::Round, border_color: PROMPT, padding_left: 1, padding_right: 1, margin_top: 1) {
                    Text(content: format!("── context compacted (epoch {epoch} · {style}) ──{hint}"), color: PROMPT, weight: Weight::Bold)
                    #(summary.as_ref().filter(|_| *expanded).map(|t| element!(View(flex_direction: FlexDirection::Column, margin_top: 1) {
                        #(t.view().lines().map(line))
                    })))
                }
            }
            .into_any()
        }
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
}
