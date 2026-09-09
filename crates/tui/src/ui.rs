//! Fullscreen iocraft app. Pure: reads [`UiEvent`]s from a channel, writes
//! [`Command`]s to another. All actor traffic lives in `lib.rs`.

use iocraft::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tools::{Decision, Risk};

use crate::model::{Command, Entry, Model, UiEvent};

/// Channels handed to the app through iocraft context (owned so the receiver
/// can be taken out on first render, per design doc §11.5).
pub struct Links {
    /// Taken by the first render into `use_future`.
    pub events: Option<UnboundedReceiver<UiEvent>>,
    /// Command sink.
    pub commands: UnboundedSender<Command>,
    /// Model name for the status line.
    pub model_name: String,
    /// Session to `/resume` on first render.
    pub resume: Option<runtime::SessionId>,
}

/// Run the fullscreen UI until the user quits.
pub async fn run(links: Links) -> std::io::Result<()> {
    element! {
        ContextProvider(value: Context::owned(links)) {
            App
        }
    }
    .fullscreen()
    .await
}

#[component]
fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut links = hooks.use_context_mut::<Links>();
    let mut model = hooks.use_state(Model::default);
    let mut input = hooks.use_state(String::new);
    let commands = links.commands.clone();
    let model_name = links.model_name.clone();

    // Runs once: the receiver is `Some` only on the first render.
    let events = links.events.take();
    let resume = links.resume.take();
    hooks.use_future({
        let commands = commands.clone();
        async move {
            let Some(mut events) = events else { return };
            if let Some(id) = resume {
                model.write().resume(id, &commands);
            }
            while let Some(ev) = events.recv().await {
                model.write().apply(ev);
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
            if m.approval().is_some() {
                match code {
                    KeyCode::Char('y') => m.decide(Decision::Allow, &commands),
                    KeyCode::Char('n') | KeyCode::Esc => m.decide(Decision::Deny, &commands),
                    _ => {}
                }
                return;
            }
            match code {
                KeyCode::Enter => {
                    let line = std::mem::take(&mut *input.write());
                    m.submit(&line, &commands);
                }
                KeyCode::Esc => m.cancel(&commands),
                _ => {}
            }
        }
    });

    let m = model.read();
    if m.quit {
        system.exit();
    }
    let modal = m.approval().map(|req| {
        element! {
            View(border_style: BorderStyle::Double, border_color: Color::Yellow, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
                Text(content: format!("approval: {}  ({})", req.tool, risk_name(req.risk)), weight: Weight::Bold)
                Text(content: req.summary.clone(), wrap: TextWrap::Wrap)
                Text(content: "[y] allow  [n/esc] deny", color: Color::Yellow)
            }
        }
    });
    let has_focus = modal.is_none();
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
                Text(content: m.status(&model_name), color: Color::DarkGrey)
            }
            View(border_style: BorderStyle::Round, border_color: Color::Blue, height: 3) {
                TextInput(
                    has_focus,
                    value: input.to_string(),
                    on_change: move |v| input.set(v),
                )
            }
        }
    }
}

fn entry(e: &Entry) -> AnyElement<'static> {
    match e {
        Entry::User(t) => {
            element!(Text(content: format!("> {t}"), weight: Weight::Bold, wrap: TextWrap::Wrap))
                .into_any()
        }
        Entry::Assistant(t) => element!(Text(content: t.clone(), wrap: TextWrap::Wrap)).into_any(),
        Entry::Reasoning { text, done } => {
            let shown = if *done {
                format!("… {}", text.lines().next().unwrap_or_default())
            } else {
                text.clone()
            };
            element!(Text(content: shown, color: Color::DarkGrey, italic: true, wrap: TextWrap::Wrap)).into_any()
        }
        Entry::Tool {
            name,
            summary,
            result,
            ..
        } => element! {
            View(flex_direction: FlexDirection::Column) {
                Text(content: format!("⚙ {name} {summary}"), color: Color::Cyan)
                #(result.as_ref().map(|(line, is_error)| element!(Text(
                    content: format!("{} {line}", if *is_error { "✗" } else { "✓" }),
                    color: if *is_error { Color::Red } else { Color::Green },
                ))))
            }
        }
        .into_any(),
        Entry::Info(t) => element!(Text(content: t.clone(), color: Color::DarkGrey)).into_any(),
        Entry::Error(t) => element!(Text(content: t.clone(), color: Color::Red)).into_any(),
    }
}

fn risk_name(r: Risk) -> &'static str {
    match r {
        Risk::ReadOnly => "read-only",
        Risk::Mutating => "mutating",
        Risk::Destructive => "destructive",
    }
}
