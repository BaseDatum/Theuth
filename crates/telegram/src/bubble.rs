//! The bubble: one prose block of a turn, live as a Telegram draft (private
//! chats) or an edited message (groups, repair), persisted as one rich
//! message at a block boundary (design doc 98 §8.1–§8.3, §8.5, §8.6).

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

use crate::api::{self, ApiError, MsgRef};
use crate::threads::OpenMsg;
use crate::worker::{WorkerCtx, WorkerNote, brand};

/// Finish a bubble at the block boundary where `raw()` crosses this (98 §8.5).
pub const BUBBLE_SOFT_MAX: usize = 30_000;
/// A silent draft is re-sent after this long so it does not expire (98 §8.2).
pub const KEEPALIVE: Duration = Duration::from_secs(25);
/// Plain `sendMessage` cap (98 §8.1).
pub const PLAIN_CHUNK: usize = 4096;

/// How the live text reaches the chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `sendRichMessageDraft` with this id (private chats).
    Draft {
        /// `draft_id`.
        id: i64,
    },
    /// A sent message edited at block boundaries; `None` until the first committed block.
    Edit {
        /// The message, once sent.
        msg: Option<MsgRef>,
    },
}

/// A reasoning run shown above the prose it preceded (98 §8.6).
#[derive(Debug)]
pub struct Reasoning {
    /// Committed reasoning text (empty when `cfg.reasoning` is off).
    pub text: String,
    /// `thought N s` once the run ended; `None` while it streams.
    pub heading: Option<String>,
    /// When the run began.
    pub started: Instant,
}

impl Reasoning {
    /// A run that starts now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            text: String::new(),
            heading: None,
            started: Instant::now(),
        }
    }

    /// Still streaming.
    #[must_use]
    pub fn live(&self) -> bool {
        self.heading.is_none()
    }

    /// Ends the run; `ms` from `ReasoningDone`, else the wall time.
    pub fn end(&mut self, ms: Option<u64>) {
        if self.heading.is_none() {
            let secs = ms.map_or_else(|| self.started.elapsed().as_secs(), |ms| ms / 1000);
            self.heading = Some(format!("thought {secs} s"));
        }
    }

    /// The expandable quote, or nothing when there is no text.
    #[must_use]
    pub fn quote(&self) -> String {
        if self.text.trim().is_empty() {
            return String::new();
        }
        format!(
            "**{}**\n<blockquote expandable>\n{}\n</blockquote>\n\n",
            self.heading.as_deref().unwrap_or("thinking"),
            self.text.trim_end()
        )
    }

    /// The draft-only indicator while the run streams.
    #[must_use]
    pub fn indicator(&self) -> String {
        if self.live() {
            format!(
                "<tg-thinking>thinking · {} s</tg-thinking>\n",
                self.started.elapsed().as_secs()
            )
        } else {
            String::new()
        }
    }
}

/// One prose block of the turn (98 §8.2).
#[derive(Debug)]
pub struct Bubble {
    /// Draft or edit.
    pub mode: Mode,
    /// This block's text; `raw()` is the truth.
    pub text: markdown::Text,
    /// Bytes of `raw()` the last draft/edit carried.
    pub sent: usize,
    /// Something changed since the last call.
    pub dirty: bool,
    /// The last API call.
    pub last_call: Instant,
    /// When the bubble opened.
    pub opened: Instant,
    /// Reasoning shown above the prose.
    pub reasoning: Option<Reasoning>,
    /// Edit mode fell back to plain text: only `raw[sent..]` goes out per flush.
    plain: bool,
}

impl Bubble {
    /// A fresh bubble: Draft mode in private chats, Edit mode in groups.
    #[must_use]
    pub fn open(ctx: &WorkerCtx, draft_id: i64) -> Self {
        let mode = if ctx.to.chat > 0 {
            Mode::Draft { id: draft_id }
        } else {
            Mode::Edit { msg: None }
        };
        let now = Instant::now();
        Self {
            mode,
            text: markdown::Text::new(),
            sent: 0,
            dirty: false,
            last_call: now,
            opened: now,
            reasoning: None,
            plain: false,
        }
    }

    /// The live draft id, if any.
    #[must_use]
    pub fn draft_id(&self) -> Option<i64> {
        match self.mode {
            Mode::Draft { id } => Some(id),
            Mode::Edit { .. } => None,
        }
    }

    /// Appends a prose delta.
    pub fn push(&mut self, delta: &str) {
        self.text.push(delta);
        self.dirty = true;
    }

    /// Appends a reasoning delta (text kept only with `cfg.reasoning`).
    pub fn think(&mut self, delta: &str, keep: bool) {
        let r = self.reasoning.get_or_insert_with(Reasoning::start);
        if keep {
            r.text.push_str(delta);
        }
        self.dirty = true;
    }

    /// Ends the reasoning run, if one is live.
    pub fn end_reasoning(&mut self, ms: Option<u64>) {
        if let Some(r) = &mut self.reasoning
            && r.live()
        {
            r.end(ms);
            self.dirty = true;
        }
    }

    /// Nothing to persist: no prose and no reasoning text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.raw().trim().is_empty()
            && self.reasoning.as_ref().is_none_or(|r| r.text.trim().is_empty())
    }

    /// The reasoning text and heading, when there is some.
    #[must_use]
    pub fn take_reasoning(&mut self) -> Option<Reasoning> {
        self.reasoning.take()
    }

    /// Reasoning (+ draft indicator) above `raw[..upto]`, branded.
    fn render(&self, ctx: &WorkerCtx, draft: bool, upto: usize) -> String {
        let mut out = String::new();
        if let Some(r) = &self.reasoning {
            if draft {
                out.push_str(&r.indicator());
            }
            out.push_str(&r.quote());
        }
        out.push_str(&self.text.raw()[..upto]);
        brand(ctx, &out)
    }

    /// The tick: re-send the draft when something changed (or as keepalive),
    /// or send/edit the committed prefix in Edit mode.
    pub async fn flush(&mut self, ctx: &WorkerCtx, turn_live: bool, card_live: bool) {
        loop {
            match self.mode {
                Mode::Draft { id } => {
                    let keepalive = turn_live
                        && !card_live
                        && self.sent > 0
                        && self.last_call.elapsed() >= KEEPALIVE;
                    if !self.dirty && !keepalive {
                        return;
                    }
                    let raw_len = self.text.raw().len();
                    let md = self.render(ctx, true, raw_len);
                    match ctx.api.draft(&ctx.to, id, &md, true).await {
                        Ok(()) => {
                            self.sent = raw_len;
                            self.dirty = false;
                            self.last_call = Instant::now();
                            return;
                        }
                        Err(ApiError::RateLimited(_)) => return,
                        Err(e) => {
                            fallback(ctx, &format!("draft: {e}"));
                            self.mode = Mode::Edit { msg: None };
                            self.sent = 0;
                        }
                    }
                }
                Mode::Edit { msg } => {
                    let committed = self.text.committed();
                    if committed <= self.sent {
                        return;
                    }
                    if self.plain {
                        send_plain(ctx, &self.text.raw()[self.sent..committed]).await;
                        self.sent = committed;
                        self.dirty = false;
                        self.last_call = Instant::now();
                        return;
                    }
                    let md = self.render(ctx, false, committed);
                    match msg {
                        None => match send_rich_or_plain(ctx, &md).await {
                            Ok(Some(id)) => {
                                let m = MsgRef {
                                    chat: ctx.to.chat,
                                    id,
                                };
                                self.mode = Mode::Edit { msg: Some(m) };
                                note_open(ctx, m, true);
                            }
                            Ok(None) => self.plain = true,
                            Err(e) => {
                                report(ctx, &e, "bubble send");
                                return;
                            }
                        },
                        Some(m) => {
                            if let Err(e) = ctx.api.edit_rich(&m, &md, None).await {
                                if api::is_gone_message(&e) {
                                    self.mode = Mode::Edit { msg: None };
                                    self.sent = 0;
                                    continue;
                                }
                                if !api::is_not_modified(&e) {
                                    report(ctx, &e, "bubble edit");
                                    return;
                                }
                            }
                        }
                    }
                    self.sent = committed;
                    self.dirty = false;
                    self.last_call = Instant::now();
                    return;
                }
            }
        }
    }

    /// Persists the block (+ footer) as one message; `false` when empty.
    pub async fn close(mut self, ctx: &WorkerCtx, footer: Option<&str>) -> bool {
        self.text.finish();
        if let Some(r) = &mut self.reasoning {
            r.end(None);
        }
        if self.is_empty() {
            if let Mode::Edit { msg: Some(m) } = self.mode {
                note_open(ctx, m, false);
            }
            return false;
        }
        let raw_len = self.text.raw().len();
        let mut md = self.render(ctx, false, raw_len);
        if let Some(f) = footer {
            md.push_str("\n\n");
            md.push_str(f);
        }
        match self.mode {
            Mode::Edit { msg: None } if self.plain => {
                let mut rest = self.text.raw()[self.sent..].to_owned();
                if let Some(f) = footer {
                    rest.push_str("\n\n");
                    rest.push_str(f);
                }
                send_plain(ctx, &rest).await;
            }
            Mode::Draft { .. } | Mode::Edit { msg: None } => {
                if let Err(e) = send_rich_or_plain(ctx, &md).await {
                    report(ctx, &e, "bubble close");
                }
            }
            Mode::Edit { msg: Some(m) } => {
                match ctx.api.edit_rich(&m, &md, None).await {
                    Ok(()) => {}
                    Err(e) if api::is_not_modified(&e) => {}
                    Err(e) if api::is_gone_message(&e) => {
                        if let Err(e) = send_rich_or_plain(ctx, &md).await {
                            report(ctx, &e, "bubble close");
                        }
                    }
                    Err(e) => report(ctx, &e, "bubble close"),
                }
                note_open(ctx, m, false);
            }
        }
        true
    }

    /// Over the soft cap: `(head, tail)` where `head` ends at a block
    /// boundary (or, for one giant block, at its last newline with the fence
    /// closed and re-opened in `tail`).
    #[must_use]
    pub fn split(&self) -> Option<(String, String)> {
        split_at_cap(self.text.raw(), self.text.committed())
    }

    /// Replaces the text with `head` so `close` persists exactly that.
    pub fn truncate_to(&mut self, head: &str) {
        self.text = markdown::Text::complete(head);
    }
}

/// `(head, tail)` of `raw` once it crosses [`BUBBLE_SOFT_MAX`] (98 §8.5).
#[must_use]
pub fn split_at_cap(raw: &str, committed: usize) -> Option<(String, String)> {
    if raw.len() < BUBBLE_SOFT_MAX {
        return None;
    }
    if committed > 0 && raw.len() - committed < BUBBLE_SOFT_MAX {
        return Some((raw[..committed].to_owned(), raw[committed..].to_owned()));
    }
    // ponytail: byte cap, not chars; bytes ≥ chars so this is only conservative.
    let cap = raw.floor_char_boundary(BUBBLE_SOFT_MAX);
    let cut = raw[..cap].rfind('\n')?;
    let (head, tail) = (&raw[..cut], &raw[cut + 1..]);
    Some(match open_fence(head) {
        Some(lang) => (format!("{head}\n```"), format!("```{lang}\n{tail}")),
        None => (head.to_owned(), tail.to_owned()),
    })
}

/// The info string of a fence left open at the end of `s`.
fn open_fence(s: &str) -> Option<String> {
    let mut open = None;
    for line in s.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("```") {
            open = match open {
                Some(_) => None,
                None => Some(rest.trim().to_owned()),
            };
        }
    }
    open
}

/// `send_rich`; a refused rich message (400) goes out as plain chunks
/// instead (98 §8.1). `Ok(None)` = plain was used.
pub async fn send_rich_or_plain(ctx: &WorkerCtx, md: &str) -> Result<Option<i64>, ApiError> {
    match ctx.api.send_rich(&ctx.to, md, None, None, None).await {
        Ok(id) => Ok(Some(id)),
        Err(ApiError::Api { code: 400, .. }) => {
            fallback(ctx, "plain");
            send_plain(ctx, md).await;
            Ok(None)
        }
        Err(e) => {
            report(ctx, &e, "send_rich");
            Err(e)
        }
    }
}

/// `markdown::plain` + `chunks(4096)` → `send_text` per chunk.
pub async fn send_plain(ctx: &WorkerCtx, md: &str) {
    for chunk in markdown::chunks(&markdown::plain(md), PLAIN_CHUNK) {
        if let Err(e) = ctx.api.send_text(&ctx.to, &chunk, None).await {
            report(ctx, &e, "send_text");
            return;
        }
    }
}

/// Logs a failed call; an unreachable chat is reported to the channel.
pub fn report(ctx: &WorkerCtx, e: &ApiError, what: &str) {
    tracing::warn!(key = %ctx.key, error = %e, "telegram {what} failed");
    if api::is_unreachable(e) {
        let _ = ctx.notes.send(WorkerNote::Unreachable {
            chat: ctx.to.chat,
            reason: e.to_string(),
        });
    }
}

/// `WorkerNote::OpenMsg` for an edit-mode message entering/leaving flight.
pub fn note_open(ctx: &WorkerCtx, m: MsgRef, open: bool) {
    let _ = ctx.notes.send(WorkerNote::OpenMsg {
        msg: OpenMsg {
            chat: m.chat,
            thread: ctx.to.thread,
            message_id: m.id,
        },
        open,
    });
}

/// `WorkerNote::Fallback` once per process per reason (98 §8.3).
pub fn fallback(ctx: &WorkerCtx, reason: &str) {
    static SEEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
    tracing::info!(key = %ctx.key, reason, "telegram stream fallback");
    if SEEN.lock().insert(reason.to_owned()) {
        let _ = ctx.notes.send(WorkerNote::Fallback {
            session: ctx.session,
            reason: reason.to_owned(),
        });
    }
}

/// `s` cut to `max` chars with an `… (n more chars)` tail when longer.
#[must_use]
pub fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((cut, _)) => format!("{}… ({} more chars)", &s[..cut], s[cut..].chars().count()),
        None => s.to_owned(),
    }
}

/// The turn summary line (21 §8.1 + 75 `ctx`): `model · 12.3k in / 1.8k out · 42 s · 3 tools · ctx 41k/200k`.
#[must_use]
pub fn footer_line(
    model: &str,
    usage: agent::Usage,
    elapsed: Duration,
    tools: u32,
    context: Option<(u32, u32)>,
) -> String {
    let mut s = format!(
        "{model} · {} in / {} out · {} · {tools} tools",
        fmt_k(usage.prompt),
        fmt_k(usage.completion),
        fmt_elapsed(elapsed)
    );
    if let Some((used, size)) = context {
        s.push_str(&format!(" · ctx {}/{}", fmt_k(used), fmt_k(size)));
    }
    s
}

/// The footer block around [`footer_line`]; flip here if `<footer>` is refused (98 §8.2).
#[must_use]
pub fn footer(line: &str) -> String {
    format!("<footer>{line}</footer>")
}

/// `950` · `12.3k` · `1.2M`.
#[must_use]
pub fn fmt_k(n: u32) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", f64::from(n) / 1000.0)
    } else {
        format!("{:.1}M", f64::from(n) / 1_000_000.0)
    }
}

/// `12 B` · `1.5 KB` · `2.0 MB`.
#[must_use]
pub fn fmt_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `0.3 s` · `42 s` · `1m32s`.
#[must_use]
pub fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 60.0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{}m{:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prefers_block_boundary() {
        let para = "x".repeat(1000) + "\n\n";
        let raw = para.repeat(31) + "tail";
        let committed = raw.len() - 4;
        let (head, tail) = split_at_cap(&raw, committed).unwrap();
        assert_eq!(head.len(), committed);
        assert_eq!(tail, "tail");
        assert!(split_at_cap("short", 0).is_none());
    }

    #[test]
    fn split_reopens_a_fence() {
        let mut raw = "intro\n\n```rust\n".to_owned();
        while raw.len() < BUBBLE_SOFT_MAX + 100 {
            raw.push_str("let x = 1;\n");
        }
        let (head, tail) = split_at_cap(&raw, 7).unwrap();
        assert!(head.ends_with("\n```"));
        assert!(tail.starts_with("```rust\n"));
        assert!(head.len() <= BUBBLE_SOFT_MAX + 4);
    }

    #[test]
    fn clip_counts_the_rest() {
        assert_eq!(clip("abcdef", 3), "abc… (3 more chars)");
        assert_eq!(clip("ab", 3), "ab");
    }
}
