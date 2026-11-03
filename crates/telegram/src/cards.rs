//! Activity cards: one rich message per tool batch, an entry per call,
//! edited as the batch progresses (design doc 98 §8.4, §8.6).

use std::time::Duration;

use tokio::time::Instant;

use crate::api::{self, MsgRef};
use crate::bubble::{Reasoning, fmt_elapsed, fmt_size, note_open, report, send_rich_or_plain};
use crate::worker::{WorkerCtx, brand};

/// Output shown inline; the rest behind the `out:` button.
pub const TOOL_OUTPUT_CHARS: usize = 2_500;
/// Card edits are at least this far apart (98 §4.2).
pub const EDIT_GAP: Duration = Duration::from_millis(1500);
/// A running call's time is refreshed this often.
pub const REFRESH: Duration = Duration::from_secs(30);
/// Summary line cap.
const SUMMARY_MAX: usize = 256;
/// Link lines per entry.
const URLS_MAX: usize = 3;

/// Where a call is.
#[derive(Debug)]
enum State {
    Pending,
    Running {
        desc: String,
        since: Instant,
    },
    Done {
        desc: String,
        ok: bool,
        elapsed: Duration,
        body: String,
        more: usize,
        size: usize,
        urls: Vec<String>,
    },
}

/// One card line.
#[derive(Debug)]
enum Item {
    Tool {
        call_id: String,
        name: String,
        state: State,
    },
    Thinking(Reasoning),
}

/// The card of one tool batch.
#[derive(Debug)]
pub struct Card {
    /// The message, once sent.
    pub msg: Option<MsgRef>,
    items: Vec<Item>,
    dirty: bool,
    /// The last send/edit.
    pub last_edit: Instant,
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

impl Card {
    /// An empty, unsent card.
    #[must_use]
    pub fn new() -> Self {
        Self {
            msg: None,
            items: Vec::new(),
            dirty: false,
            last_edit: Instant::now(),
        }
    }

    /// Calls in this batch.
    #[must_use]
    pub fn tools(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, Item::Tool { .. }))
            .count()
    }

    fn tool(&mut self, call_id: &str) -> Option<&mut State> {
        self.items.iter_mut().find_map(|i| match i {
            Item::Tool {
                call_id: c, state, ..
            } if c == call_id => Some(state),
            _ => None,
        })
    }

    /// `ToolCallPending`: `⏳ name`.
    pub fn pending(&mut self, call_id: &str, name: &str) {
        self.items.push(Item::Tool {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            state: State::Pending,
        });
        self.dirty = true;
    }

    /// `ToolCallStarted`: `⚙ name · describe(args) · running`.
    pub fn started(&mut self, call_id: &str, name: &str, args: &serde_json::Value) {
        let desc = describe(name, args);
        match self.tool(call_id) {
            Some(state) => {
                *state = State::Running {
                    desc,
                    since: Instant::now(),
                }
            }
            None => self.items.push(Item::Tool {
                call_id: call_id.to_owned(),
                name: name.to_owned(),
                state: State::Running {
                    desc,
                    since: Instant::now(),
                },
            }),
        }
        self.dirty = true;
    }

    /// `ToolCallFinished`: `✓`/`✗ name · describe · 1.2 s` + the output head.
    /// Returns how many chars were cut (0 = all shown).
    pub fn finished(&mut self, call_id: &str, name: &str, output: &str, is_error: bool) -> usize {
        let (desc, elapsed) = match self.tool(call_id) {
            Some(State::Running { desc, since }) => (std::mem::take(desc), since.elapsed()),
            _ => (String::new(), Duration::ZERO),
        };
        let trimmed = output.trim_end();
        let (body, more) = head_chars(trimmed, TOOL_OUTPUT_CHARS);
        let mut urls = Vec::new();
        for r in markdown::urls(trimmed) {
            let u = &trimmed[r];
            if !urls.iter().any(|x| x == u) {
                urls.push(u.to_owned());
                if urls.len() == URLS_MAX {
                    break;
                }
            }
        }
        let done = State::Done {
            desc,
            ok: !is_error,
            elapsed,
            body: body.to_owned(),
            more,
            size: trimmed.len(),
            urls,
        };
        match self.tool(call_id) {
            Some(state) => *state = done,
            None => self.items.push(Item::Tool {
                call_id: call_id.to_owned(),
                name: name.to_owned(),
                state: done,
            }),
        }
        self.dirty = true;
        more
    }

    /// A reasoning delta between tool calls (98 §8.6).
    pub fn think(&mut self, delta: &str, keep: bool) {
        let r = match self.items.last_mut() {
            Some(Item::Thinking(r)) if r.live() => r,
            _ => {
                self.items.push(Item::Thinking(Reasoning::start()));
                let Some(Item::Thinking(r)) = self.items.last_mut() else {
                    unreachable!()
                };
                r
            }
        };
        if keep {
            r.text.push_str(delta);
        }
        self.dirty = true;
    }

    /// Ends a live reasoning run.
    pub fn end_reasoning(&mut self, ms: Option<u64>) {
        if let Some(Item::Thinking(r)) = self.items.last_mut()
            && r.live()
        {
            r.end(ms);
            self.dirty = true;
        }
    }

    /// Adopts a reasoning run that preceded the batch with no prose.
    pub fn adopt(&mut self, r: Reasoning) {
        self.items.push(Item::Thinking(r));
        self.dirty = true;
    }

    fn running(&self) -> bool {
        self.items.iter().any(|i| {
            matches!(
                i,
                Item::Tool {
                    state: State::Running { .. } | State::Pending,
                    ..
                }
            )
        })
    }

    /// The whole card as Rich Markdown.
    #[must_use]
    pub fn render(&self, ctx: &WorkerCtx, footer: Option<&str>) -> String {
        let mut out = String::new();
        for item in &self.items {
            if !out.is_empty() {
                out.push('\n');
            }
            match item {
                Item::Thinking(r) => {
                    let text = if r.text.trim().is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}\n", r.shown().trim_end())
                    };
                    out.push_str(&format!(
                        "<details><summary>{}</summary>{text}</details>",
                        r.heading.as_deref().unwrap_or("thinking")
                    ));
                }
                Item::Tool {
                    call_id,
                    name,
                    state,
                } => match state {
                    State::Pending => out.push_str(&format!("⏳ {name}")),
                    State::Running { desc, since } => out.push_str(&format!(
                        "<details open><summary>⚙ {name}{} · running {} s</summary>\n\n```{}\n{desc}\n```\n</details>",
                        summary(desc),
                        since.elapsed().as_secs(),
                        if name == "shell" { "sh" } else { "text" },
                    )),
                    State::Done {
                        desc,
                        ok,
                        elapsed,
                        body,
                        more,
                        size,
                        urls,
                    } => {
                        out.push_str(&format!(
                            "<details><summary>{} {name}{} · {}</summary>\n\n```text\n{body}\n```\n",
                            if *ok { "✓" } else { "✗" },
                            summary(desc),
                            fmt_elapsed(*elapsed),
                        ));
                        if *more > 0 {
                            out.push_str(&format!(
                                "… {more} more chars\n<tg-button type=\"callback_data\" data=\"out:{call_id}\">full output · {}</tg-button>\n",
                                fmt_size(*size)
                            ));
                        }
                        if !urls.is_empty() {
                            out.push_str(&urls.join(" · "));
                            out.push('\n');
                        }
                        out.push_str("</details>");
                    }
                },
            }
        }
        if let Some(f) = footer {
            out.push_str("\n\n");
            out.push_str(f);
        }
        brand(ctx, &out)
    }

    /// Sends the card once, then edits it when something changed (≥ 1.5 s
    /// apart) or a call has run another 30 s; `last` forces the final edit.
    pub async fn flush(&mut self, ctx: &WorkerCtx, last: bool, footer: Option<&str>) {
        let refresh = self.running() && self.last_edit.elapsed() >= REFRESH;
        if !(self.dirty || refresh || last) {
            return;
        }
        if !last && self.msg.is_some() && self.last_edit.elapsed() < EDIT_GAP {
            return;
        }
        let md = self.render(ctx, footer);
        match self.msg {
            None => {
                if let Ok(Some(id)) = send_rich_or_plain(ctx, &md).await {
                    let m = MsgRef {
                        chat: ctx.to.chat,
                        id,
                    };
                    self.msg = Some(m);
                    if !last {
                        note_open(ctx, m, true);
                    }
                }
            }
            Some(m) => match ctx.api.edit_rich(&m, &md, None).await {
                Ok(()) => {}
                Err(e) if api::is_not_modified(&e) => {}
                Err(e) if api::is_gone_message(&e) => {
                    note_open(ctx, m, false);
                    self.msg = None;
                    if let Ok(Some(id)) = send_rich_or_plain(ctx, &md).await {
                        self.msg = Some(MsgRef {
                            chat: ctx.to.chat,
                            id,
                        });
                    }
                }
                Err(e) => report(ctx, &e, "card edit"),
            },
        }
        if last && let Some(m) = self.msg {
            note_open(ctx, m, false);
        }
        self.dirty = false;
        self.last_edit = Instant::now();
    }
}

/// ` · <one-line describe>` for the summary, or nothing.
fn summary(desc: &str) -> String {
    let one = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return String::new();
    }
    let (head, more) = head_chars(&one, SUMMARY_MAX);
    if more == 0 {
        format!(" · {head}")
    } else {
        format!(" · {head}…")
    }
}

/// The first `max` chars of `s` and how many chars follow them.
#[must_use]
pub fn head_chars(s: &str, max: usize) -> (&str, usize) {
    match s.char_indices().nth(max) {
        Some((cut, _)) => (&s[..cut], s[cut..].chars().count()),
        None => (s, 0),
    }
}

/// TUI-style argument description (00 §4): `shell` shows its command;
/// anything else is `key: value` per argument, strings over 120 chars
/// collapsed to `<N chars>`, non-strings as compact JSON.
#[must_use]
pub fn describe(name: &str, args: &serde_json::Value) -> String {
    if name == "shell"
        && let Some(cmd) = args.get("command").and_then(|c| c.as_str())
    {
        return cmd.trim().to_owned();
    }
    let Some(obj) = args.as_object() else {
        return if args.is_null() {
            String::new()
        } else {
            args.to_string()
        };
    };
    obj.iter()
        .map(|(k, v)| match v {
            serde_json::Value::String(s) if s.chars().count() > 120 => {
                format!("{k}: <{} chars>", s.chars().count())
            }
            serde_json::Value::String(s) => format!("{k}: {s}"),
            other => format!("{k}: {other}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_rules() {
        assert_eq!(
            describe("shell", &serde_json::json!({"command": " ls -la "})),
            "ls -la"
        );
        let long = "x".repeat(130);
        let d = describe(
            "read_file",
            &serde_json::json!({"path": "a.rs", "body": long, "n": 3}),
        );
        let mut lines: Vec<&str> = d.lines().collect();
        lines.sort_unstable();
        assert_eq!(lines, ["body: <130 chars>", "n: 3", "path: a.rs"]);
        assert_eq!(describe("x", &serde_json::Value::Null), "");
    }

    #[test]
    fn head_chars_counts_rest() {
        assert_eq!(head_chars("héllo", 2), ("hé", 3));
        assert_eq!(head_chars("hi", 5), ("hi", 0));
    }
}
