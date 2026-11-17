use super::*;
use crate::components::{Registry, Wrapper, WrapperOrigin, WrapperView};

/// A wire context revision (decimal string, 48 §3.1) as a number; `0` when
/// missing, so an unloaded transcript is older than any loaded one.
pub(super) fn revision(s: &str) -> u64 {
    s.parse().unwrap_or(0)
}

/// Transcript entries for one user input: a skill injection (04 §2.4) is a
/// collapsed `◇ name` card plus the user's words; an injected turn holding
/// several peer messages becomes one card each; anything else is `you`,
/// stamped with `turn` when the input opened one.
pub(super) fn user_entries(text: String, turn: Option<i64>, registry: &Registry) -> Vec<Entry> {
    // 152 D6: an endless parent's view leads a native child's opener (137 R1-D1); the brief follows it.
    if let Some((lines, body, rest)) = parent_view(&text) {
        let mut out = vec![card(
            CardKind::Memory,
            format!("parent's context · {lines} lines"),
            &body,
        )];
        out.extend(user_entries(rest.trim_start().to_owned(), turn, registry));
        return out;
    }
    // 74 §3: the model's `<time …/>` line is never a `you` row.
    let text = agent::clock::strip_owned(text);
    // 171F B4: parked cell results joined in front of the user's words.
    let (cells, said) = cell_results(&text);
    if !cells.is_empty() {
        let mut out: Vec<Entry> = cells.into_iter().map(Entry::Info).collect();
        if !said.is_empty() {
            out.extend(user_entries(said.to_owned(), turn, registry));
        }
        return out;
    }
    // 199: a contact file's notice row (automatic input) is its file line.
    if let Some(entry) = crate::components::files::notice(&text) {
        return vec![entry];
    }
    // 137G: a bare `<memory>` harness note is a muted info line (a tag with
    // attributes stays the user's message).
    if let Some(body) = agent::endless_ext::memory_note(&text) {
        return vec![Entry::Info(body.to_owned())];
    }
    if let Some((name, body, rest)) = agent::skills::injected(&text) {
        let mut out = vec![card(CardKind::Skill, name.to_owned(), body)];
        if !rest.is_empty() {
            out.push(Entry::User {
                text: rest.to_owned(),
                turn,
            });
        }
        return out;
    }
    match injected(&text, |row| registry.wrapper(row)) {
        None => vec![Entry::User { text, turn }],
        Some(cards) => cards
            .into_iter()
            .map(|(kind, title, body)| card(kind, title, &body))
            .collect(),
    }
}

/// 171F B4: the parked `<cell_result …>…</cell_result>` blocks the engine
/// joins in front of the user's words, as one `↳ cell <label or name> · exit
/// <n> · expect <…>` line each, and the rest (the user's own text). Only
/// leading blocks peel: one the user typed mid-message stays text.
pub(super) fn cell_results(text: &str) -> (Vec<String>, &str) {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(after) = rest.trim_start().strip_prefix("<cell_result ")
        && let Some((open, after)) = after.split_once('>')
        && let Some((_, tail)) = after.split_once("</cell_result>")
    {
        let who = attr(open, "label=\"")
            .or_else(|| attr(open, " name=\""))
            .or_else(|| attr(open, " cell=\""))
            .unwrap_or_default();
        let mut line = format!("↳ cell {}", unescape(who));
        if let Some(exit) = attr(open, " exit=\"") {
            let _ = write!(line, " · exit {exit}");
        }
        if let Some(expect) = attr(open, " expect=\"") {
            let _ = write!(line, " · expect {expect}");
        }
        lines.push(line);
        rest = tail;
    }
    if lines.is_empty() {
        (lines, text)
    } else {
        (lines, rest.trim_start())
    }
}

/// `<chat>\n{id}+{n}|{text}\n…</chat>{rest}` (endless/fold.rs `render`): the
/// line count, the lines as a fenced block and what follows `</chat>`. Only
/// the exact shape matches — a human typing `<chat>` keeps a `you` row.
pub(super) fn parent_view(text: &str) -> Option<(usize, String, &str)> {
    let t = text.trim_start();
    let inner = t.strip_prefix("<chat>\n")?;
    let (body, rest) = inner.split_once("</chat>")?;
    let shaped = |l: &str| {
        let Some((id, after)) = l.split_once('+') else {
            return false;
        };
        let Some((n, _)) = after.split_once('|') else {
            return false;
        };
        !id.is_empty() && !n.is_empty() && id.bytes().chain(n.bytes()).all(|b| b.is_ascii_digit())
    };
    let mut lines = body.lines().filter(|l| !l.is_empty());
    if !lines.all(shaped) {
        return None;
    }
    let count = body.lines().filter(|l| !l.is_empty()).count();
    Some((count, format!("```\n{}\n```", body.trim_end()), rest))
}

/// The fence hook every assistant entry renders through; one allocation for the process.
pub(super) static FENCE: LazyLock<markdown::FenceHook> = LazyLock::new(|| Arc::new(surface::fence));

/// Transcript entries for one history record; assistant text renders with
/// `opts` so replayed history looks like it did live. A row the user removed
/// from context (48 §2.2) is a collapsed `⊘` card over the original — the
/// archive stays readable (Ctrl+O), nothing re-enters the model's frame.
/// Attachment occurrences are listed by name, removed ones marked.
pub(crate) fn from_history(h: HistoryEntry, opts: &markdown::Options, registry: &mut Registry) -> Vec<Entry> {
    let h = match registry.history_entry(h, opts) { Ok(entries) => return entries, Err(row) => row };
    let removed = |c: &Option<ContextEntry>| c.as_ref().is_some_and(|c| c.removed);
    match h {
        // A consultation note (52 §6) is harness-authored, never `◆ you`:
        // one collapsed card, the question and answer on Ctrl+O.
        HistoryEntry::User {
            text,
            context,
            vision: Some(note),
            ..
        } => {
            if removed(&context) {
                return vec![removed_card("vision", &text)];
            }
            vec![card(CardKind::Vision, note_title(&note), &text)]
        }
        HistoryEntry::User {
            text,
            turn,
            context,
            peer,
            vision: None,
            ..
        } => {
            if removed(&context) {
                return vec![removed_card("you", &text)];
            }
            // Typed provenance (50 §9): the card is titled by the
            // authenticated sender, the wrapper is only unwrapped for the
            // body. A wrapper without `peer` stays the untrusted card
            // `user_entries` always drew: no id, no controls.
            // 199: a message's receiving placeholders follow its card as file lines.
            let files = crate::components::files::placeholders(&text);
            let projection = registry.wrapper(Wrapper { tag: "user", open: "", body: &text, sender: None,
                origin: peer.as_ref().map(|origin| WrapperOrigin { id: origin.id, from: &origin.from, reply_to: origin.reply_to, full: origin.contact_mode == Some(agent::ShareMode::Full) }), live: false });
            let mut out = match projection {
                Some(WrapperView::Card { kind, title, body }) => vec![card(kind, title, &body)],
                Some(WrapperView::Hidden) => Vec::new(),
                Some(WrapperView::Fallback) | None => user_entries(text, turn, registry),
            };
            out.extend(files);
            // One row each: `Entry::height` counts an `Info` as one line.
            out.extend(attachment_labels(&context).into_iter().map(Entry::Info));
            out
        }
        HistoryEntry::Assistant { text, context, .. } => {
            if removed(&context) {
                return vec![removed_card("assistant", &text)];
            }
            let mut t = markdown::Text::with(opts.clone());
            t.push(&text);
            t.finish();
            t.reveal_all();
            vec![Entry::Assistant(Box::new(t))]
        }
        // 70 THINKING-TRACES: the reload shows the same one-line collapsed block the stream left.
        HistoryEntry::Reasoning { text, .. } => vec![Entry::Reasoning { text, done: true }],
        HistoryEntry::Tool {
            call_id,
            name,
            args,
            result,
            files: _,
            context,
        } => {
            let mut args = describe(&name, &args);
            args.extend(attachment_labels(&context));
            // `.` marks a call from inside a `python` cell (08 §4.1), as live.
            let shown = if call_id.contains('.') {
                format!("↳ {name}")
            } else {
                name
            };
            if removed(&context) {
                if let Some((output, _)) = &result {
                    args.push(output.clone());
                }
                return vec![removed_card(&shown, &args.join("\n"))];
            }
            let mut out = Vec::new();
            let result = match result {
                Some((output, is_error)) => match split_engine_restart(&output) {
                    Some((note, rest)) => {
                        out.push(note);
                        Some((rest.to_owned(), is_error))
                    }
                    None => Some((output, is_error)),
                },
                None => None,
            };
            out.push(Entry::Tool {
                args,
                name: shown,
                call_id,
                result,
                expanded: None,
            });
            out
        }
        HistoryEntry::Compacted {
            epoch,
            summary,
            style,
            context,
        } => {
            if removed(&context) {
                return vec![removed_card(&format!("summary (epoch {epoch})"), &summary)];
            }
            vec![Entry::Compacted {
                epoch,
                style,
                summary: Some(Box::new(markdown::Text::complete(&summary))),
                expanded: false,
            }]
        }
        // Never model content: shown, never reasoned over (48 §3.3 has nothing to remove).
        // 199 D17: an ended share's file notice is its labelled file line, never the markup.
        HistoryEntry::Error { text, .. } => vec![crate::components::files::ended(&text).unwrap_or_else(|| Entry::Error(text))],
        // 109 §8.2: the dropped partial, greyed as an info line under its divider.
        HistoryEntry::Interrupted { text, .. } => vec![
            Entry::Divider("interrupted — resuming".to_owned()),
            Entry::Info(text),
        ],
        // 183 §5.3: one collapsed card; the model-facing text on Ctrl+O.
        HistoryEntry::Moved { moved, .. } => {
            vec![card(
                CardKind::Moved,
                moved_title(&moved),
                &moved.model_text(),
            )]
        }
        _ => Vec::new(),
    }
}

/// The collapsed marker of a row removed from context; `original` is the
/// archived text, shown on Ctrl+O only.
fn removed_card(what: &str, original: &str) -> Entry {
    card(
        CardKind::Removed,
        format!("{what} · removed from context"),
        original,
    )
}
