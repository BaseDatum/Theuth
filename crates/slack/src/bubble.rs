//! One streamed message per turn (design doc 21 §8.1–8.4; cards 110 §4): a
//! [`Turn`] owns the current bubble (`chat.startStream` or, when a stream
//! cannot be opened, a `chat.postMessage` edited in place), one task card
//! per tool batch, the trailing reasoning card, rotation at block boundaries
//! and repair of dead streams. `markdown::Text::raw()` is the only truth
//! about the text; a bubble that fails is rewritten from it.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use artifact::preview::Appearance;
use artifact::snapshot::{FALLBACK_COLUMNS, SCALE, SLACK_WIDTH, SnapshotOutcome};
use runtime::{Runtime, SessionId};
use surface::{DRAWING, PictureFence, PictureLang, PicturePart};
use tokio::time::Instant;

use crate::api::{ApiError, Chunk, MARKDOWN_TEXT_MAX, SlackApi, Source, TaskStatus, UploadFile};
use crate::{SlackEvent, ToolCards};

/// Chars a stream message carries before it rotates at a block boundary:
/// text plus card `output` / `details`. Slack caps a stream at 12 000 chars
/// in total, not per call; card output counts too — a reply with 3 000 chars
/// of text died `msg_too_long` once its re-sent cards had grown (164,
/// 2026-11-03). A card's output never takes more than this on its own.
pub const BUBBLE_SOFT_MAX: usize = 11_000;
/// Slack closes streams around 5 minutes wall; rotate before that.
pub const BUBBLE_MAX_AGE: Duration = Duration::from_secs(270);
/// Re-send a live card if nothing was appended for this long.
pub const KEEPALIVE: Duration = Duration::from_secs(25);
/// Worker flush cadence.
pub const TICK: Duration = Duration::from_millis(750);
/// A live batch card is re-sent at most this often (110 §4.4, Telegram's
/// `EDIT_GAP`); a closed batch's last change goes at once.
pub const EDIT_FLOOR: Duration = Duration::from_millis(1500);
/// Task card title cap (chars).
pub const TITLE_MAX: usize = 256;
/// Task card details cap (chars).
pub const DETAILS_MAX: usize = 256;
/// Itemised lines per batch card before `… n more` (110 §4.3).
const ITEMS_MAX: usize = 60;
/// Chars per itemised line.
const LINE_MAX: usize = 200;
/// Chars of a thought's first line on its `💭` line.
const THOUGHT_MAX: usize = 120;
/// `sources` per card.
const SOURCES_MAX: usize = 3;
/// Consecutive `startStream` failures (other than the known fallback codes)
/// before the bubble gives up on streaming.
const OPEN_ATTEMPTS: u8 = 3;
/// Block Kit blocks one message may carry (110 §8; Slack drops blocks over
/// 50): a fence past it that fits a fresh message starts the next one, a
/// fence over it on its own is a picture.
const BLOCKS_MAX: usize = 50;
/// Picture fences settled per flush before the next tick takes over.
const SETTLE_MAX: usize = 32;

/// `startStream` errors that mean "this thread cannot stream": fall back to
/// update mode for the rest of the turn (21 §8.1).
const FALLBACK_CODES: [&str; 6] = [
    "channel_type_not_supported",
    "missing_recipient_user_id",
    "missing_recipient_team_id",
    "invalid_thread_ts",
    "missing_scope",
    "not_allowed_token_type",
];
/// `appendStream` errors that mean "this stream takes nothing more": stop
/// it as it stands and continue in a new one (21 §8.4).
const DEAD_CODES: [&str; 4] = [
    "message_not_in_streaming_state",
    "streaming_mode_mismatch",
    "msg_too_long",
    "stopped_by_user",
];
/// `stopStream` errors after which the message is final already (timed
/// out, gone, stopped by the user): nothing more goes into it.
const ENDED_CODES: [&str; 3] = [
    "message_not_in_streaming_state",
    "message_not_found",
    "stopped_by_user",
];

/// Where a turn renders.
#[derive(Clone, Debug)]
pub struct Target {
    /// Channel id.
    pub channel: String,
    /// Thread root ts.
    pub thread_ts: String,
    /// Recipient user (stream `recipient_user_id`).
    pub user: String,
    /// Recipient team (stream `recipient_team_id`).
    pub team: String,
}

/// The one-line turn summary under the bubble.
#[derive(Clone, Debug)]
pub struct Footer {
    /// Canonical model id.
    pub model: String,
    /// Turn token usage.
    pub usage: agent::Usage,
    /// Wall time of the turn.
    pub elapsed: Duration,
    /// Tool calls made.
    pub tools: u32,
    /// `(used, window)` when the session's window size is known — an ACP
    /// child's `usage_update` (75 §5); native sessions carry none here.
    pub context: Option<(u32, u32)>,
}

impl fmt::Display for Footer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} · {} in / {} out · {} · {} tools",
            self.model,
            fmt_k(self.usage.prompt),
            fmt_k(self.usage.completion),
            fmt_elapsed(self.elapsed),
            self.tools
        )?;
        if let Some((used, size)) = self.context {
            write!(f, " · ctx {}/{}", fmt_k(used), fmt_k(size))?;
        }
        Ok(())
    }
}

/// How the bubble is on the wire.
enum Mode {
    /// Nothing posted yet; the first flush opens it.
    Unsent,
    /// `chat.startStream` message.
    Stream(String),
    /// Plain message edited with `chat.update` (21 §8.2).
    Update(String),
}

/// One message of the turn's text (21 §8.1).
struct Bubble {
    mode: Mode,
    /// This bubble's share of the turn's text.
    text: markdown::Text,
    /// Bytes of `text.raw()` Slack has shown: everything appended (Stream)
    /// or `committed()` at the last update (Update).
    sent: usize,
    /// Stream mode: chars appended so far against Slack's per-stream cap
    /// ([`weight`]).
    used: usize,
    /// Stream mode: Block Kit blocks appended so far, against [`BLOCKS_MAX`].
    blocks_sent: usize,
    /// Update mode: rendered fences already posted as their own message.
    blocks_posted: usize,
    /// `slack.blocks`: `json-render` fences with a block form go as Block Kit.
    blocks: bool,
    /// Starts in `raw` of the picture fences settled as their D7 text.
    fallbacks: Vec<usize>,
    opened: Instant,
    last_append: Instant,
}

/// One piece of a bubble's text ([`Bubble::parts`]), in order.
enum Part<'a> {
    /// Prose, other fences and the D7 text of picture fences nobody drew.
    Text(String),
    /// A `json-render` fence as Block Kit (110 §8), with the fence.
    Blocks(Vec<serde_json::Value>, PictureFence<'a>),
    /// A closed picture fence waiting to be drawn (164 D9); always last.
    Picture(Range<usize>, PictureFence<'a>),
    /// A picture fence still streaming in, or a half-typed opener, at this
    /// offset (164 D8); always last.
    Open(usize),
}

/// A closed picture fence of a bubble, owned, waiting for its picture.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    /// Fence lines included, in the bubble's `raw`.
    range: Range<usize>,
    lang: PictureLang,
    body: String,
}

impl Pending {
    fn fence(&self) -> PictureFence<'_> {
        PictureFence {
            lang: self.lang,
            body: &self.body,
            closed: true,
            range: self.range.clone(),
        }
    }
}

impl Bubble {
    fn new(blocks: bool) -> Self {
        let now = Instant::now();
        Self {
            mode: Mode::Unsent,
            text: markdown::Text::new(),
            sent: 0,
            used: 0,
            blocks_sent: 0,
            blocks_posted: 0,
            blocks,
            fallbacks: Vec::new(),
            opened: now,
            last_append: now,
        }
    }

    /// `raw[range]` in order: text runs, Block Kit fences and — ending the
    /// list — the first picture fence not yet settled: `Picture` once it
    /// can be drawn, `Open` while it still streams in; everything after it
    /// is held (164 D8). `budget` is how many blocks this message may still
    /// take; a `json-render` fence over it, or without a block form, is a
    /// picture (D9). A fence settled as text, an unclosed one of a finished
    /// text and every non-picture fence stay in the text. Never a body.
    fn parts(&self, range: Range<usize>, budget: usize) -> Vec<Part<'_>> {
        let raw = self.text.raw();
        let done = self.text.is_done();
        // Scan from the line start so a partial line is never read as an opener.
        let from = raw[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let text = &raw[from..range.end];
        let half = (!done && range.end == raw.len())
            .then(|| half_opener(text))
            .flatten();
        let text = &text[..half.unwrap_or(text.len())];
        let mut out = Vec::new();
        let mut buf = String::new();
        let mut skip = range.start - from;
        let mut budget = budget;
        for part in surface::picture_parts(text) {
            let f = match part {
                PicturePart::Text(t) => {
                    let n = skip.min(t.len());
                    skip -= n;
                    buf.push_str(&t[n..]);
                    continue;
                }
                PicturePart::Picture(f) => f,
            };
            skip = 0;
            let r = f.range.clone();
            let r = from + r.start..from + r.end;
            if !f.closed {
                if done {
                    buf.push_str(&f.fallback(FALLBACK_COLUMNS));
                    continue;
                }
                push_text(&mut out, &mut buf);
                out.push(Part::Open(r.start));
                return out;
            }
            if self.fallbacks.contains(&r.start) {
                buf.push_str(&f.fallback(FALLBACK_COLUMNS));
                continue;
            }
            if let Some(b) = block_form(&f, self.blocks, budget) {
                budget -= b.len();
                push_text(&mut out, &mut buf);
                out.push(Part::Blocks(b, f));
                continue;
            }
            push_text(&mut out, &mut buf);
            // A closing line without its newline yet may still grow into content.
            out.push(if done || r.end < raw.len() {
                Part::Picture(r, f)
            } else {
                Part::Open(r.start)
            });
            return out;
        }
        push_text(&mut out, &mut buf);
        if let Some(h) = half {
            out.push(Part::Open(from + h));
        }
        out
    }

    /// Where the unsettled part of the bubble starts for a scan: update mode
    /// re-renders the whole message, stream mode continues after `sent`.
    fn scan(&self) -> (usize, usize) {
        match self.mode {
            Mode::Update(_) => (0, BLOCKS_MAX),
            Mode::Unsent | Mode::Stream(_) => {
                (self.sent, BLOCKS_MAX.saturating_sub(self.blocks_sent))
            }
        }
    }

    /// Offset of the first picture fence not yet settled, open or closed:
    /// nothing from here on goes out as text (164 D8).
    fn hold(&self) -> Option<usize> {
        let (from, budget) = self.scan();
        match self.parts(from..self.text.raw().len(), budget).pop()? {
            Part::Picture(r, _) => Some(r.start),
            Part::Open(start) => Some(start),
            Part::Text(_) | Part::Blocks(..) => None,
        }
    }

    /// The first picture fence ready to be drawn, when it is what holds the text.
    fn next_picture(&self) -> Option<Pending> {
        let (from, budget) = self.scan();
        match self.parts(from..self.text.raw().len(), budget).pop()? {
            Part::Picture(range, f) => Some(Pending {
                range,
                lang: f.lang,
                body: f.body.to_owned(),
            }),
            Part::Open(_) | Part::Text(_) | Part::Blocks(..) => None,
        }
    }

    /// `cut` moved to the start of the picture fence it falls inside, if any,
    /// so a rotation never splits a fence across bubbles.
    fn outside_fence(&self, cut: usize) -> usize {
        let raw = self.text.raw();
        surface::picture_parts(raw)
            .into_iter()
            .filter_map(|p| match p {
                PicturePart::Picture(f) => Some(f.range),
                PicturePart::Text(_) => None,
            })
            .find(|r| r.start < cut && cut < r.end)
            .map_or(cut, |r| r.start)
    }

    /// `raw[range]` as plain markdown: the repair text, with every fence as
    /// text (Block Kit as its D7 text, a pending picture as [`DRAWING`]).
    fn out(&self, range: Range<usize>) -> String {
        let done = self.text.is_done();
        let mut md = String::new();
        for p in self.parts(range, BLOCKS_MAX) {
            match p {
                Part::Text(s) => md.push_str(&s),
                Part::Blocks(_, f) => md.push_str(&f.fallback(FALLBACK_COLUMNS)),
                Part::Picture(_, f) if done => md.push_str(&f.fallback(FALLBACK_COLUMNS)),
                Part::Picture(..) | Part::Open(_) => md.push_str(DRAWING),
            }
        }
        md
    }

    /// Stream mode: `raw[range]` as stream chunks, `Chunk::Text` runs around
    /// one `Chunk::Blocks` per Block Kit fence (110 §8). `budget` is how
    /// many blocks this message may still take. A pending picture (never
    /// inside a range a caller streams) is [`DRAWING`].
    fn segments(&self, range: Range<usize>, budget: usize) -> Vec<Chunk> {
        let done = self.text.is_done();
        self.parts(range, budget)
            .into_iter()
            .map(|p| match p {
                Part::Text(s) => Chunk::Text(s),
                Part::Blocks(b, _) => Chunk::Blocks(serde_json::Value::Array(b)),
                Part::Picture(_, f) if done => Chunk::Text(f.fallback(FALLBACK_COLUMNS)),
                Part::Picture(..) | Part::Open(_) => Chunk::Text(DRAWING.to_owned()),
            })
            .collect()
    }

    /// Update mode (110 §8, 164 D8): the bubble's markdown up to the held
    /// fence (shown as [`DRAWING`]) with the Block Kit fences left out, and
    /// those block payloads in order.
    fn split(&self, range: Range<usize>) -> (String, Vec<serde_json::Value>) {
        let done = self.text.is_done();
        let mut md = String::new();
        let mut posts = Vec::new();
        for p in self.parts(range, BLOCKS_MAX) {
            match p {
                Part::Text(s) => md.push_str(&s),
                Part::Blocks(b, _) => posts.push(serde_json::Value::Array(b)),
                Part::Picture(_, f) if done => md.push_str(&f.fallback(FALLBACK_COLUMNS)),
                Part::Picture(..) | Part::Open(_) => md.push_str(DRAWING),
            }
        }
        (md, posts)
    }

    /// Stream mode: bytes of `raw` that may be appended now — up to the
    /// first picture fence not yet settled (164 D8).
    fn streamable(&self) -> usize {
        self.hold().unwrap_or(self.text.raw().len())
    }

    /// Stream mode: where an append may end now — [`Self::streamable`],
    /// held at the last block boundary once the message would pass
    /// [`BUBBLE_SOFT_MAX`], so a rotation cuts there and never has to take
    /// back text Slack shows (21 §8.4).
    fn stream_end(&self) -> usize {
        let end = self.streamable();
        let raw = self.text.raw();
        if self.sent >= end || self.used + raw[self.sent..end].chars().count() < BUBBLE_SOFT_MAX {
            return end;
        }
        end.min(self.text.committed().max(self.sent))
    }
}

/// `buf` as a `Part::Text` unless empty; `buf` cleared.
fn push_text(out: &mut Vec<Part<'_>>, buf: &mut String) {
    if !buf.is_empty() {
        out.push(Part::Text(std::mem::take(buf)));
    }
}

/// Offset of a trailing line that is so far a fence opener whose info string
/// may still become a picture language (`` ```mer ``, or bare backticks):
/// held back until its newline tells (164 D8).
fn half_opener(text: &str) -> Option<usize> {
    let start = text.rfind('\n').map_or(0, |i| i + 1);
    let line = &text[start..];
    let body = line.trim_start_matches(' ');
    if line.len() - body.len() > 3 {
        return None;
    }
    let marker = body.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let info = body.trim_start_matches(marker);
    if body.len() - info.len() < 3 {
        return info.is_empty().then_some(start);
    }
    let info = info.trim().to_ascii_lowercase();
    ["json-render", "jsonrender", "mermaid", "openui"]
        .iter()
        .any(|l| l.starts_with(&info))
        .then_some(start)
}

/// Reasoning text as shown wherever it is replaced in place (thought lines,
/// the update-mode quote): a picture fence never shows its body (164 D2,
/// Telegram's `Reasoning::shown`) — [`DRAWING`] while the run is live, its
/// D7 text once it ended.
fn reasoning_text(text: &str, live: bool) -> std::borrow::Cow<'_, str> {
    if live {
        surface::picture_preview(text)
    } else {
        surface::flatten(text, FALLBACK_COLUMNS).into()
    }
}

/// The one-line summary of reasoning (a thought line, a card's `details`):
/// the first line of [`reasoning_text`] while live, the first prose line once
/// ended (a D7 box's ```` ``` ```` opener says nothing in one line).
fn reasoning_line(text: &str, live: bool) -> Option<String> {
    if live {
        first_line(&surface::picture_preview(text))
    } else {
        first_line(&surface::strip_pictures(text))
    }
}

/// The live stream-mode reasoning card's `output`: every settled picture
/// fence (closed, closing line broken) as its D7 text, nothing from the first
/// unsettled one (open, closing line still unbroken, or a half-typed opener)
/// on. Each output is a prefix of every later one and of the ended run's
/// [`reasoning_text`], so the card only ever grows and no byte of a fence
/// body goes out.
fn reasoning_settled(text: &str) -> String {
    let text = &text[..half_opener(text).unwrap_or(text.len())];
    let mut out = String::with_capacity(text.len());
    for part in surface::picture_parts(text) {
        match part {
            PicturePart::Text(t) => out.push_str(t),
            PicturePart::Picture(f) if f.closed && text[f.range.end..].contains('\n') => {
                out.push_str(&f.fallback(FALLBACK_COLUMNS));
            }
            PicturePart::Picture(_) => break,
        }
    }
    out
}

/// The Block Kit form of a closed picture fence (110 §8, 164 D9): a
/// `json-render` fence `surface::slack_blocks` renders within `budget`
/// blocks while `blocks` is on. Everything else is a picture.
pub(crate) fn block_form(
    f: &PictureFence<'_>,
    blocks: bool,
    budget: usize,
) -> Option<Vec<serde_json::Value>> {
    if !blocks || f.lang != PictureLang::JsonRender || !f.closed {
        return None;
    }
    match surface::slack_blocks(f.body) {
        Ok(b) if b.len() <= budget => Some(b),
        Ok(b) => {
            tracing::debug!(
                blocks = b.len(),
                budget,
                "fence over the block cap; a picture"
            );
            None
        }
        Err(e) => {
            tracing::debug!(error = %e, "fence has no block form; a picture");
            None
        }
    }
}

/// The PNG of a closed picture fence as a Slack file (164 D4, D6, D9), or
/// `None` when nobody can draw it — `pictures` off, a `json-render` body
/// that does not parse, no renderer, failed, timed out — so its D7 text
/// goes instead. Title: the fence's title, else `diagram`; `alt_txt`: the
/// fence's alt text.
pub(crate) async fn picture_file(
    rt: &Runtime,
    pictures: bool,
    f: &PictureFence<'_>,
) -> Option<UploadFile> {
    if !pictures
        || !f.closed
        || (f.lang == PictureLang::JsonRender && surface::parse(f.body).is_err())
    {
        return None;
    }
    let outcome = artifact::snapshot::render(
        rt,
        f.lang.into(),
        f.body,
        SLACK_WIDTH,
        Appearance::Light,
        SCALE,
    )
    .await;
    let SnapshotOutcome::Done { png, .. } = outcome else {
        tracing::info!(lang = f.lang.as_str(), ?outcome, "fence sent as text");
        return None;
    };
    let title = f.title().unwrap_or_else(|| "diagram".to_owned());
    Some(UploadFile {
        name: if f.lang == PictureLang::Openui {
            "panel.png".to_owned()
        } else {
            "diagram.png".to_owned()
        },
        mime: "image/png".to_owned(),
        bytes: png,
        alt: Some(f.alt()),
        title: Some(title),
    })
}

/// One unit of a reply posted in one go (`message_user`, 164 D9), in order.
#[derive(Debug)]
pub(crate) enum Piece {
    /// Markdown: prose, other fences and the D7 text of pictures nobody drew.
    Text(String),
    /// A `json-render` fence as Block Kit (110 §8).
    Blocks(serde_json::Value),
    /// A picture fence drawn as a PNG.
    Picture(UploadFile),
}

/// `text` split for Slack the way a bubble is (164 D9): Block Kit fences
/// within the block cap while `blocks` is on, every other picture fence
/// drawn (all at once) while `pictures` is on, the rest text — never a body.
pub(crate) async fn pieces(rt: &Runtime, text: &str, blocks: bool, pictures: bool) -> Vec<Piece> {
    let parts = surface::picture_parts(text);
    let mut budget = BLOCKS_MAX;
    let forms: Vec<Option<Vec<serde_json::Value>>> = parts
        .iter()
        .map(|p| match p {
            PicturePart::Picture(f) => {
                let b = block_form(f, blocks, budget);
                budget -= b.as_ref().map_or(0, Vec::len);
                b
            }
            PicturePart::Text(_) => None,
        })
        .collect();
    let files = futures::future::join_all(parts.iter().zip(&forms).map(|(p, form)| async move {
        match p {
            PicturePart::Picture(f) if form.is_none() => picture_file(rt, pictures, f).await,
            PicturePart::Picture(_) | PicturePart::Text(_) => None,
        }
    }))
    .await;
    let mut out = Vec::new();
    let mut buf = String::new();
    let push = |out: &mut Vec<Piece>, buf: &mut String| {
        let t = buf.trim_start_matches(['\n', '\r']).trim_end();
        if !t.trim().is_empty() {
            out.push(Piece::Text(t.to_owned()));
        }
        buf.clear();
    };
    for ((part, form), file) in parts.into_iter().zip(forms).zip(files) {
        let f = match part {
            PicturePart::Text(t) => {
                buf.push_str(t);
                continue;
            }
            PicturePart::Picture(f) => f,
        };
        if let Some(b) = form {
            push(&mut out, &mut buf);
            out.push(Piece::Blocks(serde_json::Value::Array(b)));
        } else if let Some(file) = file {
            push(&mut out, &mut buf);
            out.push(Piece::Picture(file));
        } else {
            buf.push_str(&f.fallback(FALLBACK_COLUMNS));
        }
    }
    push(&mut out, &mut buf);
    out
}

/// Blocks inside the `Chunk::Blocks` of `chunks`, for the bubble's budget.
fn blocks_in(chunks: &[Chunk]) -> usize {
    chunks
        .iter()
        .map(|c| match c {
            Chunk::Blocks(b) => b.as_array().map_or(0, Vec::len),
            Chunk::Text(_) | Chunk::Task { .. } => 0,
        })
        .sum()
}

/// Mirrors `isDeliverable` in `crates/desktop/ui/src/components/chat/ToolRunCard.tsx:19-22`;
/// change both or neither. A deliverable is never in a batch and splits one
/// (110 §4.1).
fn is_deliverable(name: &str, args: &serde_json::Value) -> bool {
    matches!(
        name,
        "show" | "app" | "agent_draft" | "task_draft" | "routine_draft"
    ) || (name == "doc_create" && args.get("kind").and_then(|k| k.as_str()) == Some("board"))
}

/// One tool call of a batch (110 §11). Nested python inner calls
/// (`<parent>.<n>`) fold into their parent's `inner`, never an item of
/// their own.
struct Call {
    call_id: String,
    name: String,
    /// `describe` whitespace-joined to one line; `inner` wins once known.
    described: String,
    started: Option<Instant>,
    finished: Option<Instant>,
    error: bool,
    first_line: String,
    lines: usize,
    /// Inner tool names in call order (`×n` folded at render).
    inner: Vec<String>,
}

impl Call {
    fn pending(call_id: &str, name: &str) -> Self {
        Self {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            described: String::new(),
            started: None,
            finished: None,
            error: false,
            first_line: String::new(),
            lines: 0,
            inner: Vec::new(),
        }
    }

    fn open(&self) -> bool {
        self.finished.is_none()
    }

    /// What the call did, one line: the inner calls it actually made, else
    /// `describe` of its arguments.
    fn what(&self) -> String {
        if self.inner.is_empty() {
            self.described.clone()
        } else {
            fold_names(&self.inner)
        }
    }

    /// Its itemised line (110 §4.3).
    fn line(&self) -> String {
        let what = self.what();
        let what = if what.is_empty() {
            String::new()
        } else {
            format!(" · {what}")
        };
        match (self.started, self.finished) {
            (None, None) => format!("⏳ {}", self.name),
            (_, None) => format!("⠹ {}{what} · running", self.name),
            (started, Some(end)) => {
                let glyph = if self.error { "✗" } else { "✓" };
                let took = started.map_or_else(String::new, |s| {
                    format!(" · {}", fmt_elapsed(end.saturating_duration_since(s)))
                });
                let result = if self.first_line.is_empty() {
                    format!("<{} lines>", self.lines)
                } else {
                    self.first_line.clone()
                };
                format!("{glyph} {}{what}{took} · {result}", self.name)
            }
        }
    }
}

/// A reasoning run between two calls (110 §4.1): provisionally the batch's
/// last item while live; popped into a `reasoning-<n>` card when text or
/// the end of the turn follows instead of a call.
struct Thought {
    text: String,
    started: Instant,
    /// `ReasoningDone.ms`; measured here when none arrived.
    ms: Option<u64>,
    live: bool,
}

impl Thought {
    fn new(delta: &str) -> Self {
        Self {
            text: delta.to_owned(),
            started: Instant::now(),
            ms: None,
            live: true,
        }
    }

    /// The next non-reasoning event: the run is over.
    fn end(&mut self) {
        self.live = false;
        self.ms.get_or_insert_with(|| ms_of(self.started.elapsed()));
    }

    /// `💭 thought 9 s — <first line>`; `thinking` while live.
    fn line(&self) -> String {
        let head = if self.live {
            "💭 thinking".to_owned()
        } else {
            match self.ms.and_then(fmt_thought) {
                Some(t) => format!("💭 thought {t}"),
                None => "💭 thought".to_owned(),
            }
        };
        match reasoning_line(&self.text, self.live) {
            Some(l) => format!("{head} — {}", truncate_chars(&l, THOUGHT_MAX)),
            None => head,
        }
    }
}

enum Item {
    Call(Call),
    Thought(Thought),
}

impl Item {
    /// Its itemised line, capped (110 §4.3).
    fn line(&self) -> String {
        truncate_chars(
            &match self {
                Item::Call(c) => c.line(),
                Item::Thought(t) => t.line(),
            },
            LINE_MAX,
        )
    }

    /// Its line will not change again: the call finished, the thought ended.
    fn settled(&self) -> bool {
        match self {
            Item::Call(c) => !c.open(),
            Item::Thought(t) => !t.live,
        }
    }
}

/// A card's `output` as Slack holds it. Slack appends each `task_update`'s
/// `output` to the card's — re-sending the whole list nested every earlier
/// line inside the next (164, 2026-11-03) — so a card's output only ever
/// grows: a send carries what is new, and the card a rotation re-creates in
/// the next message goes on after what earlier messages showed.
#[derive(Default)]
struct Shown {
    /// The output on the wire so far, across messages.
    text: String,
    /// Some of it is in the current message.
    here: bool,
}

impl Shown {
    /// What of `full` goes out now: `None` when nothing is new, or when
    /// `full` no longer extends what is shown (Slack cannot take text back).
    fn next<'a>(&self, full: &'a str) -> Option<&'a str> {
        let Some(rest) = full.strip_prefix(self.text.as_str()) else {
            tracing::debug!("card output no longer extends what Slack shows; held");
            return None;
        };
        let rest = if self.here {
            rest
        } else {
            rest.trim_start_matches('\n')
        };
        (!rest.is_empty()).then_some(rest)
    }

    /// The new part of `full` ([`Self::next`]) went out.
    fn sent(&mut self, full: &str) {
        if self.next(full).is_some() {
            self.here = true;
            full.clone_into(&mut self.text);
        }
    }
}

/// `s` within [`BUBBLE_SOFT_MAX`] chars (a card never fills a message on
/// its own; Slack drops an `output` over 12 000) such that a growing output
/// stays a prefix of every later one: whole while shorter, else its first
/// `BUBBLE_SOFT_MAX - 1` chars and `…`.
fn cap_output(s: &str) -> std::borrow::Cow<'_, str> {
    match s.char_indices().nth(BUBBLE_SOFT_MAX - 1) {
        None => s.into(),
        Some((i, _)) => format!("{}…", &s[..i]).into(),
    }
}

/// Chars `chunks` add to a stream message against Slack's per-stream cap:
/// text and card `output` / `details` together ([`BUBBLE_SOFT_MAX`]).
fn weight(chunks: &[Chunk]) -> usize {
    chunks
        .iter()
        .map(|c| match c {
            Chunk::Text(s) => s.chars().count(),
            Chunk::Task {
                details, output, ..
            } => [details, output]
                .into_iter()
                .flatten()
                .map(|s| s.chars().count())
                .sum(),
            Chunk::Blocks(_) => 0,
        })
        .sum()
}

/// How a card goes into a message that closes (21 §8.4): nothing spins on
/// in a message nobody updates again.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Close {
    /// The turn goes on in the next message: a live card ends here as
    /// done so far, `continued below`, and goes on there.
    Rotate,
    /// The turn is over: a call that never finished is an error.
    Finish,
}

/// A live card's title in a message that a rotation closes.
fn continued(title: &str) -> String {
    truncate_chars(&format!("{title} · continued below"), TITLE_MAX)
}

/// One activity card: design 70's run (110 §4.1–4.3). One call with no
/// reasoning renders as a plain card; a deliverable is a closed batch of one.
struct Batch {
    /// The first call's id; the `task_update` id for the batch's life.
    id: String,
    /// Event order.
    items: Vec<Item>,
    /// No further item joins: ended by text, the turn, or a deliverable.
    closed: bool,
    dirty: bool,
    /// `None` = never sent in the current bubble.
    last_sent: Option<Instant>,
    /// Update mode: the batch message.
    card_ts: Option<String>,
    /// ≤ 3 `http(s)` URLs across the batch's outputs.
    sources: Vec<Source>,
    /// Stream mode: its `output` on the wire.
    shown: Shown,
}

impl Batch {
    /// `items` starts with a call.
    fn new(items: Vec<Item>, closed: bool) -> Self {
        let id = items
            .iter()
            .find_map(|i| match i {
                Item::Call(c) => Some(c.call_id.clone()),
                Item::Thought(_) => None,
            })
            .unwrap_or_default();
        Self {
            id,
            items,
            closed,
            dirty: true,
            last_sent: None,
            card_ts: None,
            sources: Vec::new(),
            shown: Shown::default(),
        }
    }

    fn calls(&self) -> impl Iterator<Item = &Call> {
        self.items.iter().filter_map(|i| match i {
            Item::Call(c) => Some(c),
            Item::Thought(_) => None,
        })
    }

    /// A call is still running or pending, or a thought is live.
    fn open(&self) -> bool {
        self.items.iter().any(|i| match i {
            Item::Call(c) => c.open(),
            Item::Thought(t) => t.live,
        })
    }

    /// One call, no reasoning: today's single card (110 §4.1).
    fn plain(&self) -> Option<&Call> {
        match &self.items[..] {
            [Item::Call(c)] => Some(c),
            _ => None,
        }
    }

    fn failed(&self) -> bool {
        self.calls().any(|c| c.error)
    }

    /// `summarize`'s text (`ToolRunCard.tsx:60-77`); a plain card is
    /// `name · describe`.
    fn title(&self) -> String {
        if let Some(c) = self.plain() {
            let what = c.what();
            return truncate_chars(
                &if what.is_empty() {
                    c.name.clone()
                } else {
                    format!("{} · {what}", c.name)
                },
                TITLE_MAX,
            );
        }
        let calls: Vec<&Call> = self.calls().collect();
        if let Some((i, c)) = calls.iter().enumerate().find(|(_, c)| c.open()) {
            return format!("running `{}` · {} of {}", c.name, i + 1, calls.len());
        }
        let count = format!(
            "{} tool{}",
            calls.len(),
            if calls.len() == 1 { "" } else { "s" }
        );
        // A live thought is no line yet (see `settled_output`): the title says so.
        if self.open() {
            return format!("thinking · {count}");
        }
        let mut parts = vec![count];
        if let (Some(Some(start)), Some(Some(end))) = (
            calls.first().map(|c| c.started),
            calls.last().map(|c| c.finished),
        ) {
            parts.push(fmt_run(end.saturating_duration_since(start)));
        }
        let failed: Vec<&str> = calls
            .iter()
            .filter(|c| c.error)
            .map(|c| c.name.as_str())
            .collect();
        if !failed.is_empty() {
            parts.push(format!("{} failed ({})", failed.len(), failed.join(", ")));
        }
        truncate_chars(&parts.join(" · "), TITLE_MAX)
    }

    fn status(&self) -> TaskStatus {
        if let Some(c) = self.plain() {
            return match (c.started, c.finished, c.error) {
                (None, None, _) => TaskStatus::Pending,
                (_, None, _) => TaskStatus::InProgress,
                (_, Some(_), true) => TaskStatus::Error,
                (_, Some(_), false) => TaskStatus::Complete,
            };
        }
        if self.open() {
            TaskStatus::InProgress
        } else if self.failed() {
            TaskStatus::Error
        } else {
            TaskStatus::Complete
        }
    }

    /// The last finished call's first result line.
    fn details(&self) -> Option<String> {
        let last = self.calls().filter(|c| c.finished.is_some()).last()?;
        (!last.first_line.is_empty()).then(|| last.first_line.clone())
    }

    /// The itemised list (110 §4.3): ≤ 60 lines, then `… n more`.
    fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.items.iter().take(ITEMS_MAX).map(Item::line).collect();
        if self.items.len() > ITEMS_MAX {
            out.push(format!("… {} more", self.items.len() - ITEMS_MAX));
        }
        out
    }

    /// The stream card's `output` ([`Shown`]): the lines of the items
    /// settled so far, up to the first one still live, and `… n more` once
    /// the batch is over — it only ever grows; the live item is the title.
    fn settled_output(&self) -> String {
        let n = self.items.iter().take_while(|i| i.settled()).count();
        let mut lines: Vec<String> = self.items[..n]
            .iter()
            .take(ITEMS_MAX)
            .map(Item::line)
            .collect();
        if self.closed && n == self.items.len() && n > ITEMS_MAX {
            lines.push(format!("… {} more", n - ITEMS_MAX));
        }
        lines.join("\n")
    }

    /// The stream-mode card (110 §4.2) with only the output Slack lacks,
    /// frozen by `close` in a message that closes, and its whole output for
    /// [`Shown::sent`].
    fn chunk(&self, close: Option<Close>) -> (Chunk, String) {
        let full = if self.plain().is_some() {
            String::new()
        } else {
            cap_output(&self.settled_output()).into_owned()
        };
        let (mut title, mut status) = (self.title(), self.status());
        if self.open() {
            match close {
                Some(Close::Rotate) => {
                    title = continued(&title);
                    status = if self.failed() {
                        TaskStatus::Error
                    } else {
                        TaskStatus::Complete
                    };
                }
                Some(Close::Finish) => status = TaskStatus::Error,
                None => {}
            }
        }
        let chunk = Chunk::Task {
            id: self.id.clone(),
            title,
            status,
            details: self.details(),
            output: self.shown.next(&full).map(str::to_owned),
            sources: self.sources.clone(),
        };
        (chunk, full)
    }

    /// The update-mode message (110 §4.6): header bold, list in a fence.
    fn markdown(&self) -> String {
        let mut md = format!(
            "**{}**\n```\n{}\n```",
            self.title(),
            self.lines().join("\n")
        );
        if !self.sources.is_empty() {
            md.push('\n');
            md.push_str(
                &self
                    .sources
                    .iter()
                    .map(|s| format!("[{}]({})", s.text, s.url))
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        md
    }

    /// May be (re-)sent now (110 §4.4): first send at once, then the floor,
    /// except a closed batch's final change; `last` ignores the floor.
    fn due(&self, last: bool) -> bool {
        self.dirty
            && match self.last_sent {
                None => true,
                Some(at) => last || (self.closed && !self.open()) || at.elapsed() >= EDIT_FLOOR,
            }
    }

    fn add_sources(&mut self, output: &str) {
        for r in markdown::urls(output) {
            if self.sources.len() == SOURCES_MAX {
                break;
            }
            let url = &output[r];
            if self.sources.iter().any(|s| s.url == url) {
                continue;
            }
            self.sources.push(Source {
                url: url.to_owned(),
                text: host(url).to_owned(),
            });
        }
    }
}

/// `slack.tools` (110 §4.4): `all` every card; `errors` a batch only once a
/// call in it failed; `none` nothing.
fn emits(mode: ToolCards, b: &Batch) -> bool {
    match mode {
        ToolCards::All => true,
        ToolCards::Errors => b.failed(),
        ToolCards::None => false,
    }
}

/// Something waiting for the next flush, in event order.
enum Queued {
    /// A live card by id — a batch, rendered when its floor allows
    /// (110 §4.4), or the open reasoning run.
    Card(String),
    /// A finished reasoning card; `card` is its update-mode text.
    Chunk { chunk: Chunk, card: String },
}

/// A card that went out: its id and whole output after the send ([`Shown::sent`]).
type Sent = (String, String);

/// The card id a queue entry is about.
fn queued_id(q: &Queued) -> Option<&str> {
    match q {
        Queued::Card(id) => Some(id),
        Queued::Chunk { chunk, .. } => chunk_id(chunk),
    }
}

/// Stream-mode trailing reasoning run: the `reasoning-<n>` card (21 §8.5,
/// 110 §4.1).
struct ReasoningRun {
    id: String,
    text: markdown::Text,
    /// Bytes of `text.raw()` when the card was last queued.
    sent: usize,
    started: Instant,
    /// `ReasoningDone.ms`.
    ms: Option<u64>,
    /// Its `output` on the wire.
    shown: Shown,
    /// The card is in the current message.
    on_wire: bool,
}

impl ReasoningRun {
    /// The live card with only the output Slack lacks ([`reasoning_settled`],
    /// so nothing is taken back), frozen in a message a rotation closes, and
    /// its whole output for [`Shown::sent`].
    fn chunk(&self, close: Option<Close>) -> (Chunk, String) {
        let full = cap_output(&reasoning_settled(self.text.raw())).into_owned();
        let (title, status) = match close {
            None => ("thinking".to_owned(), TaskStatus::InProgress),
            Some(_) => (continued("thinking"), TaskStatus::Complete),
        };
        let chunk = Chunk::Task {
            id: self.id.clone(),
            title,
            status,
            details: None,
            output: self.shown.next(&full).map(str::to_owned),
            sources: Vec::new(),
        };
        (chunk, full)
    }
}

/// How a turn's picture fences go out (110 §8, 164 D9–D10).
#[derive(Clone, Copy, Debug)]
pub struct Fences {
    /// `slack.blocks`: `json-render` fences with a block form as Block Kit;
    /// off makes every one a picture.
    pub blocks: bool,
    /// `slack.pictures`: picture fences as PNGs in the thread; off sends
    /// their D7 text.
    pub pictures: bool,
}

/// One turn's streamed output in a thread (21 §8.1–8.4).
pub struct Turn {
    target: Target,
    tools_mode: ToolCards,
    reasoning: bool,
    fences: Fences,
    /// Pictures uploaded this turn; the footer still goes out after a reply
    /// that ends in one.
    pictures_posted: u32,
    api: Arc<dyn SlackApi>,
    rt: Runtime,
    session: SessionId,
    bubble: Option<Bubble>,
    queue: Vec<Queued>,
    /// Every batch of the turn, in order; the last one takes new calls
    /// while not `closed`.
    batches: Vec<Batch>,
    tools: u32,
    /// Reasoning cards so far; the next one is `reasoning-<runs + 1>`.
    reasoning_runs: u32,
    /// Open stream-mode trailing reasoning card.
    reasoning_run: Option<ReasoningRun>,
    /// Open update-mode quoted reasoning message.
    reasoning_segment: Option<Segment>,
    /// `Compacted { epoch }` seen this turn: `⟲ epoch n` on the footer (110 §6).
    compacted: Option<u32>,
    /// Streams cannot be opened in this thread: bubbles go straight to update mode.
    stream_unsupported: bool,
    open_failures: u8,
    fallback_published: bool,
}

impl Turn {
    /// A turn that has posted nothing yet.
    #[must_use]
    pub fn new(
        target: Target,
        tools_mode: ToolCards,
        reasoning: bool,
        fences: Fences,
        api: Arc<dyn SlackApi>,
        rt: Runtime,
        session: SessionId,
    ) -> Self {
        Self {
            target,
            tools_mode,
            reasoning,
            fences,
            pictures_posted: 0,
            api,
            rt,
            session,
            bubble: None,
            queue: Vec::new(),
            batches: Vec::new(),
            tools: 0,
            reasoning_runs: 0,
            reasoning_run: None,
            reasoning_segment: None,
            compacted: None,
            stream_unsupported: false,
            open_failures: 0,
            fallback_published: false,
        }
    }

    /// `TextDelta`: ends the open batch (110 §4.1); buffered, the next flush
    /// opens a bubble if none is open.
    pub async fn text(&mut self, delta: &str) {
        self.end_reasoning().await;
        self.close_batch();
        let blocks = self.fences.blocks;
        self.bubble
            .get_or_insert_with(|| Bubble::new(blocks))
            .text
            .push(delta);
    }

    /// `ReasoningDelta` (21 §8.5, 110 §4.1). Inside an open batch: a `💭`
    /// line of the card. Otherwise, stream mode: a `reasoning-<n>` card
    /// whose `output` grows each tick; update mode: a quoted message posted
    /// before the bubble. A run ends at the next non-reasoning event.
    pub async fn reasoning(&mut self, delta: &str) {
        if !self.reasoning {
            return;
        }
        if let Some(i) = self.open_batch() {
            let b = &mut self.batches[i];
            match b.items.last_mut() {
                Some(Item::Thought(t)) if t.live => t.text.push_str(delta),
                _ => b.items.push(Item::Thought(Thought::new(delta))),
            }
            self.touch(i);
            return;
        }
        if self.stream_unsupported {
            self.segment().push(delta).await;
            return;
        }
        if self.reasoning_run.is_none() {
            self.reasoning_runs += 1;
            let id = format!("reasoning-{}", self.reasoning_runs);
            self.reasoning_run = Some(ReasoningRun {
                id: id.clone(),
                text: markdown::Text::new(),
                sent: 0,
                started: Instant::now(),
                ms: None,
                shown: Shown::default(),
                on_wire: false,
            });
            self.queue_card(id);
        }
        if let Some(run) = &mut self.reasoning_run {
            run.text.push(delta);
        }
    }

    /// `ReasoningDone { ms }` (110 §4.1): the time behind `Thought N s` or
    /// `💭 thought N s` for the run that is ending. Published before the
    /// event that ends the run, so it lands on the live one.
    pub fn reasoning_done(&mut self, ms: u64) {
        if let Some(i) = self.open_batch()
            && let Some(Item::Thought(t)) = self.batches[i].items.last_mut()
            && t.live
        {
            t.ms = Some(ms);
            return;
        }
        if let Some(run) = &mut self.reasoning_run {
            run.ms = Some(ms);
        }
    }

    /// `Compacted { epoch }` (110 §6): no message; `⟲ epoch n` joins this
    /// turn's footer (`channel.rs` holds the epoch for the next turn when
    /// none is open).
    pub fn compacted(&mut self, epoch: u32) {
        self.compacted = Some(epoch);
    }

    fn segment(&mut self) -> &mut Segment {
        let t = &self.target;
        self.reasoning_segment.get_or_insert_with(|| {
            Segment::new(
                t.channel.clone(),
                t.thread_ts.clone(),
                Arc::clone(&self.api),
                Some("> "),
            )
        })
    }

    /// The batch still taking calls, if any.
    fn open_batch(&self) -> Option<usize> {
        let i = self.batches.len().checked_sub(1)?;
        (!self.batches[i].closed).then_some(i)
    }

    /// `(batch, item)` of a top-level call.
    fn locate(&self, call_id: &str) -> Option<(usize, usize)> {
        self.batches.iter().enumerate().find_map(|(i, b)| {
            b.items
                .iter()
                .position(|it| matches!(it, Item::Call(c) if c.call_id == call_id))
                .map(|j| (i, j))
        })
    }

    fn call_mut(&mut self, i: usize, j: usize) -> &mut Call {
        match &mut self.batches[i].items[j] {
            Item::Call(c) => c,
            Item::Thought(_) => unreachable!("located a call"),
        }
    }

    /// Batch `i` changed: re-render at the next flush its floor allows.
    fn touch(&mut self, i: usize) {
        let b = &mut self.batches[i];
        b.dirty = true;
        if emits(self.tools_mode, b) {
            let id = b.id.clone();
            self.queue_card(id);
        }
    }

    /// Card `id` goes out at the next flush that may send it.
    fn queue_card(&mut self, id: String) {
        let blocks = self.fences.blocks;
        self.bubble.get_or_insert_with(|| Bubble::new(blocks));
        if !self.queue.iter().any(|q| queued_id(q) == Some(&id)) {
            self.queue.push(Queued::Card(id));
        }
    }

    /// A non-reasoning event: the open reasoning run, if any, is over. A
    /// thought inside the open batch stays as its `💭` line.
    async fn end_reasoning(&mut self) {
        if let Some(mut s) = self.reasoning_segment.take() {
            s.finish().await;
        }
        if let Some(run) = self.reasoning_run.take() {
            let raw = reasoning_text(run.text.raw(), false);
            let ms = run.ms.unwrap_or_else(|| ms_of(run.started.elapsed()));
            // No `details`: the first line again would repeat the body.
            self.queue_chunk(
                Chunk::Task {
                    id: run.id.clone(),
                    title: thought_title(ms),
                    status: TaskStatus::Complete,
                    details: None,
                    output: run.shown.next(&cap_output(&raw)).map(str::to_owned),
                    sources: Vec::new(),
                },
                Segment::quote(&raw),
            );
        }
        if let Some(i) = self.open_batch()
            && let Some(Item::Thought(t)) = self.batches[i].items.last_mut()
            && t.live
        {
            t.end();
            self.touch(i);
        }
    }

    /// Text, the end of the turn or a deliverable ends the open batch
    /// (110 §4.1); a trailing thought leaves it as its own card.
    fn close_batch(&mut self) {
        if let Some(i) = self.open_batch() {
            self.close_batch_at(i);
        }
    }

    fn close_batch_at(&mut self, i: usize) {
        let b = &mut self.batches[i];
        b.closed = true;
        // A thought whose line Slack shows already stays: output only grows.
        if let Some(last @ Item::Thought(_)) = b.items.last()
            && !b.shown.text.ends_with(&last.line())
        {
            let Some(Item::Thought(t)) = b.items.pop() else {
                unreachable!()
            };
            self.touch(i);
            self.pop_thought(t);
        }
    }

    /// A thought that turned out trailing (110 §4.1): the `reasoning-<n>`
    /// card it would have been, complete.
    fn pop_thought(&mut self, mut t: Thought) {
        t.end();
        self.reasoning_runs += 1;
        let ms = t.ms.unwrap_or_default();
        let text = reasoning_text(&t.text, false);
        self.queue_chunk(
            Chunk::Task {
                id: format!("reasoning-{}", self.reasoning_runs),
                title: thought_title(ms),
                status: TaskStatus::Complete,
                details: None,
                output: Some(cap_output(&text).into_owned()),
                sources: Vec::new(),
            },
            Segment::quote(&text),
        );
    }

    /// Stream mode falling back to update mode with a reasoning card open:
    /// the card becomes the quoted message it would have been (21 §8.2).
    async fn demote_reasoning(&mut self) {
        let Some(run) = self.reasoning_run.take() else {
            return;
        };
        self.queue.retain(|q| queued_id(q) != Some(&run.id));
        self.segment().text = run.text;
        self.segment().tick().await;
    }

    /// Re-send the open reasoning card when its text grew since the last tick.
    fn tick_reasoning(&mut self) {
        let Some(run) = &mut self.reasoning_run else {
            return;
        };
        if run.text.raw().len() <= run.sent {
            return;
        }
        run.sent = run.text.raw().len();
        let id = run.id.clone();
        self.queue_card(id);
    }

    /// `ToolCallPending`. A deliverable by name alone (`show`, …) starts its
    /// own card; a `doc_create` is provisionally in the batch until its
    /// args arrive (110 §4.1).
    pub async fn tool_pending(&mut self, call_id: &str, name: &str) {
        self.end_reasoning().await;
        if agent::store::inner_call_id(call_id).is_some() {
            return;
        }
        self.add_call(
            Call::pending(call_id, name),
            is_deliverable(name, &serde_json::Value::Null),
        );
    }

    fn add_call(&mut self, call: Call, deliverable: bool) {
        let i = if deliverable {
            self.close_batch();
            self.batches.push(Batch::new(vec![Item::Call(call)], true));
            self.batches.len() - 1
        } else if let Some(i) = self.open_batch() {
            self.batches[i].items.push(Item::Call(call));
            i
        } else {
            self.batches.push(Batch::new(vec![Item::Call(call)], false));
            self.batches.len() - 1
        };
        self.touch(i);
    }

    /// `ToolCallStarted`.
    pub async fn tool_started(&mut self, call_id: &str, name: &str, args: &serde_json::Value) {
        self.end_reasoning().await;
        if let Some((parent, _)) = agent::store::inner_call_id(call_id) {
            if let Some((i, j)) = self.locate(parent) {
                self.call_mut(i, j).inner.push(name.to_owned());
                self.touch(i);
            }
            return;
        }
        self.tools += 1;
        let described = described(name, args);
        let deliverable = is_deliverable(name, args);
        let Some((i, j)) = self.locate(call_id) else {
            let mut c = Call::pending(call_id, name);
            c.started = Some(Instant::now());
            c.described = described;
            self.add_call(c, deliverable);
            return;
        };
        let c = self.call_mut(i, j);
        c.name = name.to_owned();
        c.described = described;
        c.started = Some(Instant::now());
        self.touch(i);
        if deliverable && self.batches[i].items.len() > 1 {
            self.split(i, j);
        }
    }

    /// A pending call whose args made it a deliverable (110 §4.1): out of
    /// its batch, which splits around it.
    fn split(&mut self, i: usize, j: usize) {
        let mut rest = self.batches[i].items.split_off(j + 1);
        let Some(d) = self.batches[i].items.pop() else {
            return;
        };
        let di = if self.batches[i].items.is_empty() {
            self.batches[i].items.push(d);
            self.batches[i].closed = true;
            i
        } else {
            self.close_batch_at(i);
            self.batches.insert(i + 1, Batch::new(vec![d], true));
            self.touch(i + 1);
            i + 1
        };
        // Leading reasoning is not part of a run (70): its own card.
        while let Some(Item::Thought(_)) = rest.first() {
            let Item::Thought(t) = rest.remove(0) else {
                unreachable!()
            };
            self.pop_thought(t);
        }
        if !rest.is_empty() {
            self.batches.insert(di + 1, Batch::new(rest, false));
            self.touch(di + 1);
        }
    }

    /// `ToolCallFinished`: the item's first result line, elapsed and URLs
    /// (110 §4.2–4.3); an inner call's failure marks its parent.
    pub async fn tool_finished(&mut self, call_id: &str, output: &str, is_error: bool) {
        self.end_reasoning().await;
        if let Some((parent, _)) = agent::store::inner_call_id(call_id) {
            if is_error && let Some((i, j)) = self.locate(parent) {
                self.call_mut(i, j).error = true;
                self.touch(i);
            }
            return;
        }
        let Some((i, j)) = self.locate(call_id) else {
            tracing::debug!(call_id, "tool finished without a card");
            return;
        };
        let c = self.call_mut(i, j);
        c.finished = Some(Instant::now());
        c.error |= is_error;
        c.first_line = first_line(output).unwrap_or_default();
        c.lines = output.trim_end().lines().count();
        self.batches[i].add_sources(output);
        self.touch(i);
    }

    /// Top-level tool calls started so far (for [`Footer::tools`]); inner
    /// calls and reasoning cards do not count.
    #[must_use]
    pub fn tool_count(&self) -> u32 {
        self.tools
    }

    /// Queue a finished reasoning card; one already queued for the same id
    /// is replaced in place, so one flush carries one card per id.
    fn queue_chunk(&mut self, chunk: Chunk, card: String) {
        let blocks = self.fences.blocks;
        self.bubble.get_or_insert_with(|| Bubble::new(blocks));
        let id = chunk_id(&chunk).map(str::to_owned);
        let q = Queued::Chunk { chunk, card };
        match self
            .queue
            .iter_mut()
            .find(|x| queued_id(x) == id.as_deref())
        {
            Some(slot) => *slot = q,
            None => self.queue.push(q),
        }
    }

    fn batch(&self, id: &str) -> Option<&Batch> {
        self.batches.iter().find(|b| b.id == id)
    }

    /// The open reasoning run when its id is `id`.
    fn run(&self, id: &str) -> Option<&ReasoningRun> {
        self.reasoning_run.as_ref().filter(|r| r.id == id)
    }

    /// Queued chunks that may go now, in queue order: reasoning cards, and
    /// batch cards that are due (`last` ignores the floor). The second
    /// value names the cards rendered, for [`Self::mark_sent`] once the
    /// call succeeds.
    fn collect(&self, last: bool) -> (Vec<Chunk>, Vec<Sent>) {
        let mut chunks = Vec::new();
        let mut sent = Vec::new();
        for q in &self.queue {
            let (chunk, full) = match q {
                Queued::Chunk { chunk, .. } => (chunk.clone(), String::new()),
                Queued::Card(id) => match (self.batch(id), self.run(id)) {
                    (Some(b), _) if emits(self.tools_mode, b) && b.due(last) => b.chunk(None),
                    (None, Some(r)) => r.chunk(None),
                    _ => continue,
                },
            };
            sent.push((queued_id(q).unwrap_or_default().to_owned(), full));
            chunks.push(chunk);
        }
        (chunks, sent)
    }

    /// The cards in `sent` went on the wire; cards not yet due stay queued.
    fn mark_sent(&mut self, sent: &[Sent]) {
        let now = Instant::now();
        for (id, full) in sent {
            if let Some(b) = self.batches.iter_mut().find(|b| b.id == *id) {
                b.dirty = false;
                b.last_sent = Some(now);
                b.shown.sent(full);
            } else if let Some(r) = self.reasoning_run.as_mut().filter(|r| r.id == *id) {
                r.on_wire = true;
                r.shown.sent(full);
            }
        }
        self.queue
            .retain(|q| !sent.iter().any(|(id, _)| queued_id(q) == Some(id)));
    }

    /// The keepalive payload: the live batch card, else the live reasoning
    /// card (110 §4.4).
    fn live_chunk(&self) -> Option<(Chunk, Sent)> {
        let (id, (chunk, full)) = self
            .batches
            .iter()
            .find(|b| b.open() && emits(self.tools_mode, b))
            .map(|b| (&b.id, b.chunk(None)))
            .or_else(|| self.reasoning_run.as_ref().map(|r| (&r.id, r.chunk(None))))?;
        Some((chunk, (id.clone(), full)))
    }

    /// The cards' last word in a stream message that closes: everything
    /// queued and every live card already in it, frozen by `close` (none
    /// spins on in a message nobody updates again). On a rotation a live
    /// card the message never showed waits for the next one.
    fn finals(&self, close: Close) -> Vec<(Chunk, Sent)> {
        let mut out = Vec::new();
        for q in &self.queue {
            let id = queued_id(q).unwrap_or_default();
            let (chunk, full) = match q {
                Queued::Chunk { chunk, .. } => (chunk.clone(), String::new()),
                Queued::Card(_) => match (self.batch(id), self.run(id)) {
                    (Some(b), _)
                        if emits(self.tools_mode, b)
                            && (b.last_sent.is_some() || !b.open() || close == Close::Finish) =>
                    {
                        b.chunk(Some(close))
                    }
                    (None, Some(r)) if r.on_wire => r.chunk(Some(close)),
                    _ => continue,
                },
            };
            out.push((chunk, (id.to_owned(), full)));
        }
        let queued = |id: &str| self.queue.iter().any(|q| queued_id(q) == Some(id));
        for b in &self.batches {
            if b.last_sent.is_some() && b.open() && emits(self.tools_mode, b) && !queued(&b.id) {
                let (chunk, full) = b.chunk(Some(close));
                out.push((chunk, (b.id.clone(), full)));
            }
        }
        if let Some(r) = &self.reasoning_run
            && r.on_wire
            && !queued(&r.id)
        {
            let (chunk, full) = r.chunk(Some(close));
            out.push((chunk, (r.id.clone(), full)));
        }
        out
    }

    /// The bubble closed (21 §8.4, 110 §4.6): the next message re-creates
    /// the live cards and those whose last change did not get in, each
    /// going on after the output Slack already shows.
    fn carry_cards(&mut self) {
        let mut carry = Vec::new();
        for b in &mut self.batches {
            b.last_sent = None;
            b.shown.here = false;
            if emits(self.tools_mode, b)
                && (b.open() || b.dirty)
                && !self.queue.iter().any(|q| queued_id(q) == Some(&b.id))
            {
                b.dirty = true;
                carry.push(Queued::Card(b.id.clone()));
            }
        }
        if let Some(r) = &mut self.reasoning_run {
            r.on_wire = false;
            r.shown.here = false;
            if !self.queue.iter().any(|q| queued_id(q) == Some(&r.id)) {
                carry.push(Queued::Card(r.id.clone()));
            }
        }
        self.queue.splice(0..0, carry);
    }

    /// Called every [`TICK`] by the worker: reasoning, rotation, flush,
    /// keepalive.
    pub async fn tick(&mut self) {
        if let Some(s) = &mut self.reasoning_segment {
            s.tick().await;
        }
        self.tick_reasoning();
        if self.bubble.is_none() {
            return;
        }
        if let Some(cut) = self.rotation_cut() {
            self.rotate(cut, cut, false).await;
        }
        self.flush().await;
    }

    /// `TurnFinished`: final flush and close. A turn that opened nothing
    /// makes no call. `true` when a `chat.stopStream` with
    /// `session_status: active` went through, so no separate status call is due.
    pub async fn finish(&mut self, footer: Footer) -> bool {
        self.end_reasoning().await;
        self.close_batch();
        let Some(b) = self.bubble.as_mut() else {
            return false;
        };
        b.text.finish();
        self.settle_pictures().await;
        // A cut at the very end moves nothing on: the final stop carries it all.
        let len = self.bubble.as_ref().map_or(0, |b| b.text.raw().len());
        if let Some(cut) = self.rotation_cut().filter(|&cut| cut < len) {
            self.rotate(cut, cut, false).await;
        }
        let mut footer = footer.to_string();
        if let Some(epoch) = self.compacted {
            footer.push_str(&format!(" · ⟲ epoch {epoch}"));
        }
        if matches!(self.bubble.as_ref().map(|b| &b.mode), Some(Mode::Unsent)) {
            if !self.has_work() {
                self.bubble = None;
                // The reply ended in a picture: its bubble closed before it.
                if self.pictures_posted > 0 {
                    let t = &self.target;
                    if let Err(e) = self
                        .api
                        .post_markdown(&t.channel, Some(&t.thread_ts), &format!("_{footer}_"))
                        .await
                    {
                        tracing::warn!(error = %e, "footer after a picture failed");
                    }
                }
                return false;
            }
            self.open().await;
        }
        let Some(mut b) = self.bubble.take() else {
            return false;
        };
        let len = b.text.raw().len();
        match &b.mode {
            Mode::Unsent => {
                tracing::warn!("turn finished before its bubble could be opened; text dropped");
                false
            }
            Mode::Stream(ts) => {
                let ts = ts.clone();
                let closed = self
                    .close_stream(
                        &ts,
                        &b,
                        Some(len),
                        Some(footer.clone()),
                        "active",
                        Close::Finish,
                    )
                    .await;
                self.queue.clear();
                if !closed.whole {
                    // What the stop could not take, and the footer, as a message of their own.
                    self.publish_fallback("stopStream failed".to_owned()).await;
                    let md = format!("{}\n\n_{footer}_", b.out(closed.shown..len));
                    let t = &self.target;
                    if let Err(e) = self
                        .api
                        .post_markdown(&t.channel, Some(&t.thread_ts), md.trim_start())
                        .await
                    {
                        tracing::warn!(error = %e, "reply tail after a failed stopStream failed");
                    }
                }
                closed.stopped
            }
            Mode::Update(ts) => {
                self.flush_cards(true).await;
                let (raw, posts) = b.split(0..len);
                let md = format!("{raw}\n\n_{footer}_");
                match self
                    .api
                    .update_markdown(&self.target.channel, ts, md.trim_start())
                    .await
                {
                    Ok(()) => post_fence_blocks(&*self.api, &self.target, &mut b, posts).await,
                    Err(e) => tracing::warn!(error = %e, ts, "final update failed"),
                }
                false
            }
        }
    }

    /// Stream `ts` currently open, for `threads.json.streams` bookkeeping.
    #[must_use]
    pub fn open_stream(&self) -> Option<&str> {
        match self.bubble.as_ref().map(|b| &b.mode) {
            Some(Mode::Stream(ts)) => Some(ts),
            _ => None,
        }
    }

    /// Something is waiting to go on the wire.
    fn has_work(&self) -> bool {
        if !self.queue.is_empty() {
            return true;
        }
        let Some(b) = &self.bubble else { return false };
        match b.mode {
            Mode::Unsent => !b.text.raw()[..b.streamable()].trim().is_empty(),
            Mode::Stream(_) => b.sent < b.stream_end(),
            Mode::Update(_) => b.sent < b.text.committed(),
        }
    }

    /// Where to cut the open bubble if a rotation trigger fired (21 §8.4:
    /// size at a block boundary; age for streams). Never past the held
    /// picture fence, never inside one, and in a stream never before what
    /// Slack shows (a stream takes nothing back).
    fn rotation_cut(&self) -> Option<usize> {
        let b = self.bubble.as_ref()?;
        let stream = matches!(b.mode, Mode::Stream(_));
        if matches!(b.mode, Mode::Unsent) {
            return None;
        }
        let raw = b.text.raw();
        // A stream goes on from `sent` with `used` spent; an update rewrites all.
        let (from, used) = if stream { (b.sent, b.used) } else { (0, 0) };
        let end = b.text.committed().max(from);
        let aged = stream && b.opened.elapsed() >= BUBBLE_MAX_AGE;
        if !aged && used + raw[from..end].chars().count() < BUBBLE_SOFT_MAX {
            return None;
        }
        let cut = match raw[from..end]
            .char_indices()
            .nth(MARKDOWN_TEXT_MAX - used.min(MARKDOWN_TEXT_MAX))
        {
            None => end,
            // The message holds something: the block goes on whole next.
            Some(_) if used > 0 => from,
            // ponytail: one block over the cap (a giant fence): cut at its
            // last newline under the cap; the rest renders raw next message.
            Some((i, _)) => raw[from..from + i]
                .rfind('\n')
                .map_or(from + i, |j| from + j + 1),
        };
        let cut = b.outside_fence(cut.min(b.streamable()));
        // ponytail: a block already streaming in past the boundary is split
        // there; `stream_end` holds text back so only age or cards do that.
        let cut = cut.max(from);
        // Nothing before the held fence in an empty message: a rotation would spin.
        (aged || cut > 0 || used > 0).then_some(cut)
    }

    /// Close the open bubble at `cut` (a block boundary; a dead stream ends
    /// where Slack stopped taking text) and start a fresh one seeded with
    /// `raw[resume..]` (`cut`, or the end of a picture fence just uploaded),
    /// the live cards going on in it (21 §8.4, 110 §4.6).
    async fn rotate(&mut self, cut: usize, resume: usize, dead: bool) {
        let Some(mut b) = self.bubble.take() else {
            return;
        };
        // Text before `cut` the closed message could not take.
        let mut tail = None;
        match &b.mode {
            Mode::Unsent => {}
            Mode::Stream(ts) => {
                let ts = ts.clone();
                if dead {
                    self.publish_fallback("stream died".to_owned()).await;
                }
                // stopStream defaults the session to `active`; the turn is
                // still running, so keep "Working…" and its Stop button.
                let closed = self
                    .close_stream(
                        &ts,
                        &b,
                        (!dead).then_some(cut),
                        None,
                        "processing",
                        Close::Rotate,
                    )
                    .await;
                tail = (closed.shown < cut).then_some(closed.shown);
            }
            Mode::Update(ts) => {
                let (md, posts) = b.split(0..cut);
                if b.sent != cut {
                    self.repair(ts, &md).await;
                }
                post_fence_blocks(&*self.api, &self.target, &mut b, posts).await;
            }
        }
        self.carry_cards();
        let raw = b.text.raw();
        let mut next = Bubble::new(self.fences.blocks);
        if let Some(from) = tail {
            next.text.push(&raw[from..cut]);
        }
        next.text.push(if resume > cut {
            raw[resume..].trim_start_matches(['\n', '\r'])
        } else {
            &raw[resume..]
        });
        if b.text.is_done() {
            next.text.finish();
        }
        self.bubble = Some(next);
    }

    /// End stream `ts` of `b` (21 §8.4) with one `stopStream`: the cards'
    /// [`Self::finals`], `raw[sent..cut]` (`None` for a stream that died),
    /// `footer` and session `status`. A stream Slack still holds open
    /// refuses `chat.update` (`streaming_state_conflict`), so a stop that
    /// fails, or would not fit, is retried with the cards' states only — no
    /// card spins on — then bare; their output and the text go on next.
    async fn close_stream(
        &mut self,
        ts: &str,
        b: &Bubble,
        cut: Option<usize>,
        footer: Option<String>,
        status: &str,
        close: Close,
    ) -> Closed {
        let finals = self.finals(close);
        let end = cut.filter(|&c| c > b.sent);
        let mut chunks: Vec<Chunk> = finals.iter().map(|(c, _)| c.clone()).collect();
        if let Some(end) = end {
            chunks.extend(b.segments(b.sent..end, BLOCKS_MAX - b.blocks_sent));
        }
        let api = Arc::clone(&self.api);
        let channel = self.target.channel.clone();
        let mut over = Closed {
            shown: b.sent,
            whole: false,
            stopped: false,
        };
        if b.used + weight(&chunks) <= MARKDOWN_TEXT_MAX {
            match api
                .stop_stream(&channel, ts, chunks, footer, Some(status))
                .await
            {
                Ok(()) => {
                    let sent: Vec<Sent> = finals.into_iter().map(|(_, s)| s).collect();
                    self.mark_sent(&sent);
                    return Closed {
                        shown: end.unwrap_or(b.sent),
                        whole: true,
                        stopped: true,
                    };
                }
                Err(e) if ended(&e) => {
                    tracing::warn!(error = %e, ts, "stream ended before its stop");
                    return over;
                }
                Err(e) => tracing::warn!(error = %e, ts, "stopStream failed; stopping bare"),
            }
        }
        let (states, mut kept): (Vec<Chunk>, Vec<Option<Sent>>) = finals
            .into_iter()
            .map(|(c, s)| match c {
                Chunk::Task {
                    output: Some(_), ..
                } => (without_output(c), None),
                c => (c, Some(s)),
            })
            .unzip();
        for chunks in [states, Vec::new()] {
            let bare = chunks.is_empty();
            match api
                .stop_stream(&channel, ts, chunks, None, Some(status))
                .await
            {
                Ok(()) => {
                    // A card whose output had nothing new said its last word.
                    if !bare {
                        let sent: Vec<Sent> =
                            std::mem::take(&mut kept).into_iter().flatten().collect();
                        self.mark_sent(&sent);
                    }
                    over.stopped = true;
                    return over;
                }
                Err(e) if ended(&e) => return over,
                Err(e) => tracing::warn!(error = %e, ts, bare, "stopStream failed"),
            }
        }
        over
    }

    /// `chat.update` an update-mode bubble with its text; one retry.
    async fn repair(&self, ts: &str, md: &str) {
        let md = if md.trim().is_empty() { "…" } else { md };
        for attempt in 0..2 {
            match self.api.update_markdown(&self.target.channel, ts, md).await {
                Ok(()) => return,
                Err(e) => tracing::warn!(error = %e, ts, attempt, "bubble repair failed"),
            }
        }
    }

    async fn publish_fallback(&mut self, reason: String) {
        if self.fallback_published {
            return;
        }
        self.fallback_published = true;
        tracing::warn!(session = %self.session, reason, "leaving stream mode");
        self.rt
            .publish(SlackEvent::StreamFallback {
                session: self.session,
                reason,
            })
            .await;
    }

    /// Text up to the held fence, then every picture fence ready to be
    /// drawn (164 D9): each one closes the bubble before it, goes up as a
    /// file and the text after it opens the next bubble — or, when nobody
    /// draws it, becomes its D7 text in place.
    async fn flush(&mut self) {
        self.flush_text().await;
        for _ in 0..SETTLE_MAX {
            if !self.settle_next().await {
                break;
            }
        }
    }

    /// [`Self::flush`] when a picture is waiting; otherwise nothing, so the
    /// final call of `finish` carries the pending text itself.
    async fn settle_pictures(&mut self) {
        if self
            .bubble
            .as_ref()
            .is_some_and(|b| b.next_picture().is_some())
        {
            self.flush().await;
        }
    }

    /// The first picture fence ready to be drawn, its text before it already
    /// out: drawn, the bubble closed before it, uploaded and the next bubble
    /// started past it — or settled as text in place; then the text flushed
    /// again. `false` when there is none, or its bubble could not be opened.
    async fn settle_next(&mut self) -> bool {
        let Some(b) = self.bubble.as_ref() else {
            return false;
        };
        let Some(p) = b.next_picture() else {
            return false;
        };
        let fresh = matches!(b.mode, Mode::Unsent);
        if fresh && p.range.start > 0 {
            return false;
        }
        // Over this message's block budget but within a fresh one's: the
        // next message takes it as Block Kit.
        if !fresh && block_form(&p.fence(), self.fences.blocks, BLOCKS_MAX).is_some() {
            self.rotate(p.range.start, p.range.start, false).await;
            self.flush_text().await;
            return true;
        }
        let file = picture_file(&self.rt, self.fences.pictures, &p.fence()).await;
        let mut p = p;
        if file.is_some() && !fresh {
            // Close the message first: the picture then follows every word
            // before it, even ones a failed stop moves into the next bubble.
            self.rotate(p.range.start, p.range.start, false).await;
            match self.bubble.as_ref().and_then(Bubble::next_picture) {
                Some(q) if q.range.start == 0 => p = q,
                // That text goes first; the next round draws it again.
                _ => {
                    self.flush_text().await;
                    return true;
                }
            }
        }
        let uploaded = match file {
            Some(file) => {
                let t = &self.target;
                match self
                    .api
                    .upload_file(&t.channel, Some(&t.thread_ts), &[file], None)
                    .await
                {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!(error = %e, "picture upload failed; sent as text");
                        false
                    }
                }
            }
            None => false,
        };
        if uploaded {
            self.pictures_posted += 1;
            self.rotate(p.range.start, p.range.end, false).await;
        } else if let Some(b) = self.bubble.as_mut() {
            b.fallbacks.push(p.range.start);
            b.sent = b.sent.min(p.range.start);
        }
        self.flush_text().await;
        true
    }

    /// Opens the bubble, or sends what is due in its mode.
    async fn flush_text(&mut self) {
        match self.bubble.as_ref().map(|b| &b.mode) {
            None => {}
            Some(Mode::Unsent) => {
                if self.has_work() {
                    self.open().await;
                }
            }
            Some(Mode::Stream(_)) => self.flush_stream().await,
            Some(Mode::Update(_)) => self.flush_update().await,
        }
    }

    /// First flush of a bubble: `startStream` with everything queued, or a
    /// plain message when streaming is unavailable here (21 §8.1–8.2).
    async fn open(&mut self) {
        if !self.stream_unsupported {
            match self.try_start_stream().await {
                Ok(()) | Err(None) => return,
                Err(Some(reason)) => {
                    self.stream_unsupported = true;
                    self.publish_fallback(reason).await;
                    self.demote_reasoning().await;
                    if !self.has_work() {
                        return;
                    }
                }
            }
        }
        let Some(b) = self.bubble.as_mut() else {
            return;
        };
        let committed = b.text.committed();
        let (md, posts) = b.split(0..committed);
        let body = if md.trim().is_empty() {
            "…"
        } else {
            md.trim_start_matches(['\n', '\r'])
        };
        let t = &self.target;
        match self
            .api
            .post_markdown(&t.channel, Some(&t.thread_ts), body)
            .await
        {
            Ok(ts) => {
                b.mode = Mode::Update(ts);
                b.sent = committed;
                b.opened = Instant::now();
                post_fence_blocks(&*self.api, t, b, posts).await;
                self.flush_cards(false).await;
            }
            Err(e) => tracing::warn!(error = %e, "postMessage for update-mode bubble failed"),
        }
    }

    /// `Ok`: streaming. `Err(None)`: transient failure, retry next tick.
    /// `Err(Some(reason))`: this thread cannot stream; fall back now.
    async fn try_start_stream(&mut self) -> Result<(), Option<String>> {
        let Some(b) = self.bubble.as_ref() else {
            return Err(None);
        };
        let end = b.stream_end();
        let (mut chunks, sent) = self.collect(false);
        if end > 0 {
            // A bubble after a picture starts at the prose, not the fence's blank line.
            let mut text = b.segments(0..end, BLOCKS_MAX);
            if let Some(Chunk::Text(s)) = text.first_mut() {
                let trimmed = s.trim_start_matches(['\n', '\r']).len();
                s.drain(..s.len() - trimmed);
            }
            chunks.extend(
                text.into_iter()
                    .filter(|c| !matches!(c, Chunk::Text(s) if s.is_empty())),
            );
        }
        let blocks_sent = blocks_in(&chunks);
        let used = weight(&chunks);
        let t = &self.target;
        match self
            .api
            .start_stream(&t.channel, &t.thread_ts, &t.user, &t.team, chunks)
            .await
        {
            Ok(ts) => {
                let b = self.bubble.as_mut().expect("bubble present");
                b.mode = Mode::Stream(ts);
                b.sent = end;
                b.used = used;
                b.blocks_sent = blocks_sent;
                b.opened = Instant::now();
                b.last_append = b.opened;
                self.open_failures = 0;
                self.mark_sent(&sent);
                Ok(())
            }
            Err(ApiError::Slack(code)) if FALLBACK_CODES.contains(&code.as_str()) => {
                Err(Some(code))
            }
            Err(e) => {
                self.open_failures += 1;
                tracing::warn!(error = %e, attempt = self.open_failures, "startStream failed");
                if self.open_failures < OPEN_ATTEMPTS {
                    Err(None)
                } else {
                    Err(Some(e.to_string()))
                }
            }
        }
    }

    /// Stream flush: due cards plus raw text since `sent`; with nothing to
    /// send, a keepalive refresh of the live card.
    async fn flush_stream(&mut self) {
        let Some(b) = self.bubble.as_ref() else {
            return;
        };
        let Mode::Stream(ts) = &b.mode else { return };
        let end = b.stream_end();
        let (mut chunks, mut sent) = self.collect(false);
        if b.sent < end {
            chunks.extend(b.segments(b.sent..end, BLOCKS_MAX - b.blocks_sent));
        }
        let blocks_sent = blocks_in(&chunks);
        if chunks.is_empty() {
            if b.last_append.elapsed() < KEEPALIVE {
                return;
            }
            let Some((live, id)) = self.live_chunk() else {
                return;
            };
            chunks.push(live);
            sent.push(id);
        }
        let used = weight(&chunks);
        if b.used + used > MARKDOWN_TEXT_MAX {
            // Slack would refuse it (`msg_too_long`): this goes on in the next message.
            let cut = b.sent;
            self.rotate(cut, cut, false).await;
            return;
        }
        match self
            .api
            .append_stream(&self.target.channel, ts, chunks)
            .await
        {
            Ok(()) => {
                let b = self.bubble.as_mut().expect("bubble present");
                b.sent = end;
                b.used += used;
                b.blocks_sent += blocks_sent;
                b.last_append = Instant::now();
                self.mark_sent(&sent);
            }
            Err(e) if is_dead(&e) => {
                tracing::warn!(error = %e, ts, "stream died; stopping it and going on in a new one");
                // Slack shows what it took up to `sent`; the rest moves on.
                let cut = b.sent;
                self.rotate(cut, cut, true).await;
            }
            Err(e) => tracing::warn!(error = %e, ts, "appendStream failed; will retry"),
        }
    }

    /// Update-mode flush (21 §8.2): committed text edited in place, up to
    /// the held fence (164 D8), the rendered fences it contains as their
    /// own messages (110 §8), then the cards.
    async fn flush_update(&mut self) {
        let Some(b) = self.bubble.as_mut() else {
            return;
        };
        let Mode::Update(ts) = &b.mode else { return };
        let committed = b.text.committed();
        if b.sent < committed {
            let (md, posts) = b.split(0..committed);
            match self
                .api
                .update_markdown(&self.target.channel, ts, &md)
                .await
            {
                Ok(()) => {
                    b.sent = committed;
                    post_fence_blocks(&*self.api, &self.target, b, posts).await;
                }
                Err(e) => tracing::warn!(error = %e, ts, "chat.update failed; will retry"),
            }
        }
        self.flush_cards(false).await;
    }

    /// Update-mode cards (110 §4.6): one thread message per batch, posted
    /// on its first due render and edited in place after; a reasoning card
    /// is its quoted message.
    async fn flush_cards(&mut self, last: bool) {
        let api = Arc::clone(&self.api);
        let t = self.target.clone();
        let mut keep = Vec::new();
        for q in std::mem::take(&mut self.queue) {
            match q {
                Queued::Chunk { card, .. } => {
                    if !card.is_empty()
                        && let Err(e) = api
                            .post_markdown(&t.channel, Some(&t.thread_ts), &card)
                            .await
                    {
                        tracing::warn!(error = %e, "reasoning message failed");
                    }
                }
                Queued::Card(id) => {
                    let Some(i) = self.batches.iter().position(|b| b.id == id) else {
                        continue;
                    };
                    if !emits(self.tools_mode, &self.batches[i]) {
                        continue;
                    }
                    if !self.batches[i].due(last) {
                        keep.push(Queued::Card(id));
                        continue;
                    }
                    let b = &mut self.batches[i];
                    let md = b.markdown();
                    let result = match &b.card_ts {
                        None => api
                            .post_markdown(&t.channel, Some(&t.thread_ts), &md)
                            .await
                            .map(|ts| b.card_ts = Some(ts)),
                        Some(ts) => api.update_markdown(&t.channel, ts, &md).await,
                    };
                    match result {
                        Ok(()) => {
                            b.dirty = false;
                            b.last_sent = Some(Instant::now());
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, batch = %id, "batch card failed");
                            keep.push(Queued::Card(id));
                        }
                    }
                }
            }
        }
        self.queue = keep;
    }
}

/// Update mode (110 §8): the rendered fences of a bubble not yet posted,
/// each as its own `post_blocks` message after the bubble's edit; stops at
/// the first failure so the next flush retries from there, in order.
async fn post_fence_blocks(
    api: &dyn SlackApi,
    t: &Target,
    b: &mut Bubble,
    posts: Vec<serde_json::Value>,
) {
    for blocks in posts.into_iter().skip(b.blocks_posted) {
        match api
            .post_blocks(&t.channel, Some(&t.thread_ts), blocks)
            .await
        {
            Ok(_) => b.blocks_posted += 1,
            Err(e) => {
                tracing::warn!(error = %e, "fence blocks post failed; will retry");
                return;
            }
        }
    }
}

fn chunk_id(c: &Chunk) -> Option<&str> {
    match c {
        Chunk::Task { id, .. } => Some(id),
        Chunk::Text(_) | Chunk::Blocks(_) => None,
    }
}

fn is_dead(e: &ApiError) -> bool {
    match e {
        ApiError::RateLimited(_) => true,
        ApiError::Slack(code) => DEAD_CODES.contains(&code.as_str()),
        ApiError::Transport(_) => false,
    }
}

/// The stop is moot: Slack ended the message already ([`ENDED_CODES`]).
fn ended(e: &ApiError) -> bool {
    matches!(e, ApiError::Slack(code) if ENDED_CODES.contains(&code.as_str()))
}

/// A card update with its state only, for a stop that must get through.
fn without_output(c: Chunk) -> Chunk {
    match c {
        Chunk::Task {
            id,
            title,
            status,
            details,
            sources,
            ..
        } => Chunk::Task {
            id,
            title,
            status,
            details,
            output: None,
            sources,
        },
        c => c,
    }
}

/// How [`Turn::close_stream`] left the message.
struct Closed {
    /// The end of the bubble's text Slack shows.
    shown: usize,
    /// The first stop went through: text, cards and footer all in.
    whole: bool,
    /// Some stop went through with the session status.
    stopped: bool,
}

/// Update-mode text segment used for reasoning (21 §8.5): one message posted
/// on the first tick, `chat.update`d when a block completes.
struct Segment {
    channel: String,
    thread_ts: String,
    api: Arc<dyn SlackApi>,
    prefix: Option<&'static str>,
    text: markdown::Text,
    ts: Option<String>,
    /// `committed()` at the last update.
    shown: usize,
}

impl Segment {
    /// `prefix_each_line`: `Some("> ")` renders as a blockquote.
    fn new(
        target_channel: String,
        thread_ts: String,
        api: Arc<dyn SlackApi>,
        prefix_each_line: Option<&'static str>,
    ) -> Self {
        Self {
            channel: target_channel,
            thread_ts,
            api,
            prefix: prefix_each_line,
            text: markdown::Text::new(),
            ts: None,
            shown: 0,
        }
    }

    /// Buffer a delta.
    async fn push(&mut self, delta: &str) {
        self.text.push(delta);
    }

    /// Post the placeholder once; update when `committed()` grew.
    async fn tick(&mut self) {
        if self.ts.is_none() {
            if self.text.raw().is_empty() {
                return;
            }
            match self
                .api
                .post_markdown(&self.channel, Some(&self.thread_ts), "_thinking…_")
                .await
            {
                Ok(ts) => self.ts = Some(ts),
                Err(e) => {
                    tracing::warn!(error = %e, "reasoning post failed");
                    return;
                }
            }
        }
        let committed = self.text.committed();
        if committed <= self.shown {
            return;
        }
        let body = self.body(committed);
        let ts = self.ts.as_deref().unwrap_or_default();
        match self.api.update_markdown(&self.channel, ts, &body).await {
            Ok(()) => self.shown = committed,
            Err(e) => tracing::warn!(error = %e, ts, "reasoning update failed"),
        }
    }

    /// Final update with everything; also when nothing grew but the last
    /// update showed a picture fence as [`DRAWING`] (its D7 text is due).
    async fn finish(&mut self) {
        if matches!(
            surface::picture_preview(&self.text.raw()[..self.shown]),
            std::borrow::Cow::Owned(_)
        ) {
            self.shown = 0;
        }
        self.text.finish();
        self.tick().await;
    }

    fn body(&self, upto: usize) -> String {
        let raw = &*reasoning_text(&self.text.raw()[..upto], !self.text.is_done());
        let prefixed = match self.prefix {
            Some(p) => Self::prefixed(raw, p),
            None => raw.trim().to_owned(),
        };
        let (head, more) = head_chars(&prefixed, MARKDOWN_TEXT_MAX - 32);
        if more == 0 {
            prefixed
        } else {
            format!("{head}\n… ({more} more chars)")
        }
    }

    /// `raw` as a `> ` blockquote (the update-mode reasoning card).
    fn quote(raw: &str) -> String {
        Self::prefixed(raw, "> ")
    }

    fn prefixed(raw: &str, p: &str) -> String {
        raw.trim()
            .lines()
            .map(|l| {
                if l.is_empty() {
                    p.trim_end().to_owned()
                } else {
                    format!("{p}{l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The first non-empty line of `s`, trimmed and capped at [`DETAILS_MAX`].
fn first_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| truncate_chars(l, DETAILS_MAX))
}

/// Argument description (110 §4.5, the rule every surface follows):
/// `shell` shows its command; `python` the inner tools its `code` awaits
/// (`knowledge_get, read_file ×2`), else its first `#` comment, else
/// `<n lines>`; anything else is `key: value` per argument, strings over
/// 120 chars collapsed to `<N chars>`, non-strings as compact JSON.
#[must_use]
pub fn describe(name: &str, args: &serde_json::Value) -> String {
    if name == "shell"
        && let Some(cmd) = args.get("command").and_then(|c| c.as_str())
    {
        return cmd.trim().to_owned();
    }
    if name == "python"
        && let Some(code) = args.get("code").and_then(|c| c.as_str())
    {
        let calls = awaited_calls(code);
        if !calls.is_empty() {
            return fold_names(&calls);
        }
        return code
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with('#'))
            .map_or_else(
                || format!("<{} lines>", code.lines().count()),
                |c| c.trim_start_matches('#').trim().to_owned(),
            );
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

/// `<name>` of every `await <name>(` in `code`, in order.
fn awaited_calls(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = code;
    while let Some(i) = rest.find("await ") {
        rest = &rest[i + "await ".len()..];
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end > 0 && rest[end..].trim_start().starts_with('(') {
            out.push(rest[..end].to_owned());
        }
    }
    out
}

/// Names with `×n` for repeats, first-appearance order: `knowledge_get, read_file ×2`.
fn fold_names(names: &[String]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for n in names {
        match counts.iter_mut().find(|(k, _)| *k == n.as_str()) {
            Some((_, c)) => *c += 1,
            None => counts.push((n, 1)),
        }
    }
    counts
        .iter()
        .map(|(k, c)| {
            if *c > 1 {
                format!("{k} ×{c}")
            } else {
                (*k).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `describe` as one line for a title or item: a command whitespace-joined,
/// `key: value` lines ` · `-separated.
fn described(name: &str, args: &serde_json::Value) -> String {
    let d = describe(name, args);
    if name == "shell" {
        return one_line(&d);
    }
    d.lines()
        .map(one_line)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Whitespace-joined to one line.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `max` chars of `s` and how many chars follow them.
fn head_chars(s: &str, max: usize) -> (&str, usize) {
    match s.char_indices().nth(max) {
        Some((cut, _)) => (&s[..cut], s[cut..].chars().count()),
        None => (s, 0),
    }
}

/// `s` cut to `max` chars, the last one an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    let (head, more) = head_chars(s, max);
    if more == 0 {
        return head.to_owned();
    }
    let (head, _) = head_chars(s, max - 1);
    format!("{head}…")
}

fn host(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|h| !h.is_empty())
        .unwrap_or(url)
}

fn fmt_k(n: u32) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", f64::from(n) / 1000.0)
    } else {
        format!("{:.1}M", f64::from(n) / 1_000_000.0)
    }
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 60.0 {
        format!("{} s", s as u64)
    } else {
        format!("{}m{:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// A batch header's duration, `summarize`'s way (`ToolRunCard.tsx:74`):
/// `x.x s` under a minute, `n min` from there.
fn fmt_run(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1} s")
    } else {
        format!("{} min", (s / 60.0).round())
    }
}

fn ms_of(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The time behind `Thought` (`lib/time.ts` `fmtThought`): `None` under a
/// second, `9 s` to the nearest second under a minute, `1m 12s` / `2m`
/// under an hour, `1h 2m` beyond.
fn fmt_thought(ms: u64) -> Option<String> {
    if ms < 1000 {
        return None;
    }
    let total = (ms + 500) / 1000;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    Some(if total < 60 {
        format!("{total} s")
    } else if h > 0 && m > 0 {
        format!("{h}h {m}m")
    } else if h > 0 {
        format!("{h}h")
    } else if s > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{m}m")
    })
}

/// `Thought N s` (110 §4.1, `MessageReasoning.tsx:11`); `Thought` under a second.
fn thought_title(ms: u64) -> String {
    match fmt_thought(ms) {
        Some(t) => format!("Thought {t}"),
        None => "Thought".to_owned(),
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};

    use artifact::preview::Client;
    use artifact::snapshot::{FenceSnapshotAnswer, FenceSnapshotRequested};
    use kameo::actor::Spawn;

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone, Debug)]
    enum Call {
        Start {
            ts: String,
            chunks: Vec<Chunk>,
        },
        Append {
            ts: String,
            chunks: Vec<Chunk>,
        },
        Stop {
            ts: String,
            chunks: Vec<Chunk>,
            footer: Option<String>,
            status: Option<String>,
        },
        Post {
            md: String,
        },
        Update {
            ts: String,
            md: String,
        },
        Upload,
        UploadFile {
            files: Vec<UploadFile>,
            comment: Option<String>,
        },
        PostBlocks {
            blocks: serde_json::Value,
        },
    }

    /// One thread message as Slack holds it.
    #[derive(Clone, Debug, Default)]
    struct Msg {
        ts: String,
        text: String,
        cards: Vec<Card>,
        /// Still streaming.
        open: bool,
        /// Chars counted against the stream cap.
        used: usize,
        /// An uploaded picture's title, instead of text.
        picture: Option<String>,
    }

    /// A fake Slack that keeps the thread the way Slack does: a stream
    /// takes text and card output up to `cap` chars in total (then
    /// `msg_too_long`, nothing applied), a card's `output` is appended,
    /// its title, status and details replaced, and `chat.update` on a
    /// message still streaming is `streaming_state_conflict`.
    #[derive(Default)]
    struct FakeApi {
        calls: Mutex<Vec<Call>>,
        fail_next: Mutex<HashMap<&'static str, ApiError>>,
        seq: AtomicU32,
        thread: Mutex<Vec<Msg>>,
        /// Stream cap; 0 is Slack's 12 000.
        cap: AtomicUsize,
        /// `msg_too_long` and `streaming_state_conflict` answers.
        refused: AtomicU32,
    }

    impl FakeApi {
        fn new() -> Arc<Self> {
            Arc::default()
        }

        fn fail(&self, method: &'static str, code: &str) {
            self.fail_next
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(method, ApiError::Slack(code.to_owned()));
        }

        fn check(&self, method: &'static str) -> Result<(), ApiError> {
            match self
                .fail_next
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(method)
            {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }

        fn record(&self, call: Call) {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }

        fn next_ts(&self) -> String {
            format!("ts{}", self.seq.fetch_add(1, Ordering::Relaxed))
        }

        fn calls(&self) -> Vec<Call> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn starts(&self) -> Vec<Vec<Chunk>> {
            self.calls()
                .into_iter()
                .filter_map(|c| match c {
                    Call::Start { chunks, .. } => Some(chunks),
                    _ => None,
                })
                .collect()
        }

        fn task_chunks(&self) -> Vec<Chunk> {
            self.calls()
                .into_iter()
                .flat_map(|c| match c {
                    Call::Start { chunks, .. }
                    | Call::Append { chunks, .. }
                    | Call::Stop { chunks, .. } => chunks,
                    _ => Vec::new(),
                })
                .filter(|c| matches!(c, Chunk::Task { .. }))
                .collect()
        }

        fn thread(&self) -> Vec<Msg> {
            self.thread
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn refuse(&self, code: &str) -> ApiError {
            self.refused.fetch_add(1, Ordering::Relaxed);
            ApiError::Slack(code.to_owned())
        }

        /// Slack's view of `chunks` on stream `ts`: refused, or applied
        /// (and the stream closed by a stop).
        fn stream(&self, ts: &str, chunks: &[Chunk], stop: bool) -> Result<(), ApiError> {
            let cap = match self.cap.load(Ordering::Relaxed) {
                0 => MARKDOWN_TEXT_MAX,
                n => n,
            };
            let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(m) = thread.iter_mut().find(|m| m.ts == ts) else {
                return Err(ApiError::Slack("message_not_found".into()));
            };
            if !m.open {
                return Err(ApiError::Slack("message_not_in_streaming_state".into()));
            }
            let weight: usize = chunks
                .iter()
                .map(|c| match c {
                    Chunk::Text(s) => s.chars().count(),
                    Chunk::Task { output, .. } => output.as_ref().map_or(0, |o| o.chars().count()),
                    Chunk::Blocks(_) => 0,
                })
                .sum();
            if m.used + weight > cap {
                drop(thread);
                return Err(self.refuse("msg_too_long"));
            }
            m.used += weight;
            for c in chunks {
                match c.clone() {
                    Chunk::Text(s) => m.text.push_str(&s),
                    Chunk::Blocks(_) => {}
                    Chunk::Task {
                        id,
                        title,
                        status,
                        details,
                        output,
                        sources,
                    } => {
                        let i = match m.cards.iter().position(|k| k.id == id) {
                            Some(i) => i,
                            None => {
                                m.cards.push(Card {
                                    id,
                                    title: String::new(),
                                    status: TaskStatus::Pending,
                                    details: None,
                                    output: None,
                                    sources: Vec::new(),
                                });
                                m.cards.len() - 1
                            }
                        };
                        let k = &mut m.cards[i];
                        k.title = title;
                        k.status = status;
                        if details.is_some() {
                            k.details = details;
                        }
                        if let Some(o) = output {
                            k.output.get_or_insert_default().push_str(&o);
                        }
                        if !sources.is_empty() {
                            k.sources = sources;
                        }
                    }
                }
            }
            m.open &= !stop;
            Ok(())
        }

        /// An injected `code` for `ts` that means Slack ended it.
        fn injected(&self, method: &'static str, ts: &str) -> Result<(), ApiError> {
            let r = self.check(method);
            if let Err(ApiError::Slack(code)) = &r
                && code == "message_not_in_streaming_state"
            {
                let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(m) = thread.iter_mut().find(|m| m.ts == ts) {
                    m.open = false;
                }
            }
            r
        }

        fn push(&self, m: Msg) {
            self.thread
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(m);
        }
    }

    #[async_trait]
    impl SlackApi for FakeApi {
        async fn start_stream(
            &self,
            _channel: &str,
            _thread_ts: &str,
            _user: &str,
            _team: &str,
            chunks: Vec<Chunk>,
        ) -> Result<String, ApiError> {
            self.check("start")?;
            let ts = self.next_ts();
            self.push(Msg {
                ts: ts.clone(),
                open: true,
                ..Msg::default()
            });
            if let Err(e) = self.stream(&ts, &chunks, false) {
                self.thread
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|m| m.ts != ts);
                return Err(e);
            }
            self.record(Call::Start {
                ts: ts.clone(),
                chunks,
            });
            Ok(ts)
        }
        async fn append_stream(
            &self,
            _channel: &str,
            ts: &str,
            chunks: Vec<Chunk>,
        ) -> Result<(), ApiError> {
            self.injected("append", ts)?;
            self.stream(ts, &chunks, false)?;
            self.record(Call::Append {
                ts: ts.to_owned(),
                chunks,
            });
            Ok(())
        }
        async fn stop_stream(
            &self,
            _channel: &str,
            ts: &str,
            chunks: Vec<Chunk>,
            footer: Option<String>,
            session_status: Option<&str>,
        ) -> Result<(), ApiError> {
            self.injected("stop", ts)?;
            self.stream(ts, &chunks, true)?;
            self.record(Call::Stop {
                ts: ts.to_owned(),
                chunks,
                footer,
                status: session_status.map(str::to_owned),
            });
            Ok(())
        }
        async fn post_markdown(
            &self,
            _channel: &str,
            _thread_ts: Option<&str>,
            markdown: &str,
        ) -> Result<String, ApiError> {
            self.check("post")?;
            self.record(Call::Post {
                md: markdown.to_owned(),
            });
            let ts = self.next_ts();
            self.push(Msg {
                ts: ts.clone(),
                text: markdown.to_owned(),
                ..Msg::default()
            });
            Ok(ts)
        }
        async fn update_markdown(
            &self,
            _channel: &str,
            ts: &str,
            markdown: &str,
        ) -> Result<(), ApiError> {
            self.check("update")?;
            {
                let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(m) = thread.iter_mut().find(|m| m.ts == ts) {
                    if m.open {
                        drop(thread);
                        return Err(self.refuse("streaming_state_conflict"));
                    }
                    // `markdown_text` replaces the message, cards and all.
                    markdown.clone_into(&mut m.text);
                    m.cards.clear();
                }
            }
            self.record(Call::Update {
                ts: ts.to_owned(),
                md: markdown.to_owned(),
            });
            Ok(())
        }
        async fn post_blocks(
            &self,
            _channel: &str,
            _thread_ts: Option<&str>,
            blocks: serde_json::Value,
        ) -> Result<String, ApiError> {
            self.check("post_blocks")?;
            self.record(Call::PostBlocks { blocks });
            let ts = self.next_ts();
            self.push(Msg {
                ts: ts.clone(),
                ..Msg::default()
            });
            Ok(ts)
        }
        async fn update_blocks(
            &self,
            _channel: &str,
            _ts: &str,
            _blocks: serde_json::Value,
        ) -> Result<(), ApiError> {
            unimplemented!()
        }
        async fn ephemeral(
            &self,
            _channel: &str,
            _user: &str,
            _text: &str,
        ) -> Result<(), ApiError> {
            unimplemented!()
        }
        async fn react(&self, _channel: &str, _ts: &str, _name: &str) -> Result<(), ApiError> {
            unimplemented!()
        }
        async fn set_session_status(
            &self,
            _channel: &str,
            _thread_ts: &str,
            _status: &str,
            _initiator: Option<&str>,
        ) -> Result<Vec<String>, ApiError> {
            unimplemented!()
        }
        async fn rename_session(
            &self,
            _channel: &str,
            _thread_ts: &str,
            _title: &str,
        ) -> Result<(), ApiError> {
            unimplemented!()
        }
        async fn upload_file(
            &self,
            _channel: &str,
            thread_ts: Option<&str>,
            files: &[crate::api::UploadFile],
            comment: Option<&str>,
        ) -> Result<String, ApiError> {
            self.check("upload_file")?;
            assert_eq!(thread_ts, Some("1.0"), "pictures go into the thread");
            self.push(Msg {
                picture: Some(files[0].title.clone().unwrap_or_default()),
                ..Msg::default()
            });
            self.record(Call::UploadFile {
                files: files.to_vec(),
                comment: comment.map(str::to_owned),
            });
            Ok("https://files.slack.com/f2".to_owned())
        }
        async fn upload_text(
            &self,
            _channel: &str,
            _thread_ts: &str,
            _title: &str,
            _text: &str,
        ) -> Result<String, ApiError> {
            self.check("upload")?;
            self.record(Call::Upload);
            Ok("https://files.slack.com/f1".to_owned())
        }
        async fn permalink(&self, _channel: &str, _ts: &str) -> Result<String, ApiError> {
            unimplemented!()
        }
        async fn download(&self, _url: &str) -> Result<Vec<u8>, ApiError> {
            unimplemented!()
        }
    }

    fn turn(api: &Arc<FakeApi>, mode: ToolCards) -> Turn {
        turn_with(api, mode, true, true)
    }

    fn turn_with(api: &Arc<FakeApi>, mode: ToolCards, reasoning: bool, blocks: bool) -> Turn {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        turn_in(rt, api, mode, reasoning, blocks, true)
    }

    fn turn_in(
        rt: Runtime,
        api: &Arc<FakeApi>,
        mode: ToolCards,
        reasoning: bool,
        blocks: bool,
        pictures: bool,
    ) -> Turn {
        let target = Target {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            user: "U1".into(),
            team: "T1".into(),
        };
        Turn::new(
            target,
            mode,
            reasoning,
            Fences { blocks, pictures },
            api.clone(),
            rt,
            SessionId::new(),
        )
    }

    /// A PNG the snapshot engine accepts: signature + IHDR with `w` × `h`.
    pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend(b"IHDR");
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    /// A client that answers every `FenceSnapshotRequested` with `png`.
    #[derive(kameo::Actor)]
    pub(crate) struct FakeRenderer {
        png: Vec<u8>,
    }

    impl kameo::message::Message<FenceSnapshotRequested> for FakeRenderer {
        type Reply = ();
        async fn handle(
            &mut self,
            m: FenceSnapshotRequested,
            _: &mut kameo::message::Context<Self, ()>,
        ) {
            assert_eq!(m.width, SLACK_WIDTH);
            assert_eq!(m.scale, SCALE);
            assert_eq!(m.appearance, Appearance::Light);
            artifact::snapshot::answer(
                m.id,
                FenceSnapshotAnswer::done(&self.png, 1440, 600, Client::Desktop),
            );
        }
    }

    /// A [`FakeRenderer`] drawing `png` on `rt`'s bus; installs the
    /// process-wide live feed once.
    pub(crate) async fn install_renderer(rt: &Runtime, png: Vec<u8>) {
        let (tx, rx) = tokio::sync::broadcast::channel::<Arc<serde_json::Value>>(4);
        artifact::preview::watch_feed(tx);
        std::mem::forget(rx);
        let actor = FakeRenderer::spawn(FakeRenderer { png });
        rt.subscribe::<FakeRenderer, FenceSnapshotRequested>(&actor)
            .await;
        std::mem::forget(actor);
    }

    /// A fresh runtime with a [`FakeRenderer`] drawing `png`.
    async fn runtime_with_renderer(png: Vec<u8>) -> Runtime {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        install_renderer(&rt, png).await;
        rt
    }

    /// A `Graph` fence: no Block Kit form, so a picture (164 D9).
    pub(crate) const GRAPH_SPEC: &str = r#"{"root":"g","elements":{"g":{"type":"Graph","props":{"title":"Data flow",
  "nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"}]}}}}"#;

    /// Every stream text chunk, markdown post and update, concatenated.
    fn all_text(api: &FakeApi) -> String {
        api.calls()
            .into_iter()
            .flat_map(|c| match c {
                Call::Start { chunks, .. }
                | Call::Append { chunks, .. }
                | Call::Stop { chunks, .. } => chunks
                    .into_iter()
                    .filter_map(|c| match c {
                        Chunk::Text(s) => Some(s),
                        Chunk::Task { .. } | Chunk::Blocks(_) => None,
                    })
                    .collect(),
                Call::Post { md } | Call::Update { md, .. } => vec![md],
                Call::Upload | Call::UploadFile { .. } | Call::PostBlocks { .. } => Vec::new(),
            })
            .collect()
    }

    fn footer() -> Footer {
        Footer {
            model: "m".into(),
            usage: agent::Usage {
                prompt: 12_345,
                completion: 800,
                cached: 0,
            },
            elapsed: Duration::from_secs(42),
            tools: 3,
            context: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn first_delta_opens_one_stream_with_text() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text("hello").await;
        t.tick().await;
        t.tick().await;
        assert_eq!(api.starts(), vec![vec![Chunk::Text("hello".into())]]);
        assert_eq!(api.calls().len(), 1);
        assert_eq!(t.open_stream(), Some("ts0"));
    }

    /// A `Card` + `Checklist` fence: renders as one `container` (110 §8).
    const CARD_SPEC: &str = r#"{"root":"c","elements":{
  "c":{"type":"Card","props":{"title":"Rollout"},"children":["l"]},
  "l":{"type":"Checklist","props":{"items":[{"text":"schema migration","done":true}]}}}}"#;

    /// Every text chunk of every stream call, in order, concatenated.
    fn streamed_text(api: &FakeApi) -> String {
        api.calls()
            .into_iter()
            .flat_map(|c| match c {
                Call::Start { chunks, .. }
                | Call::Append { chunks, .. }
                | Call::Stop { chunks, .. } => chunks,
                _ => Vec::new(),
            })
            .filter_map(|c| match c {
                Chunk::Text(s) => Some(s),
                Chunk::Task { .. } | Chunk::Blocks(_) => None,
            })
            .collect()
    }

    /// 164 D9 with `slack.blocks: false`: every `json-render` fence is a
    /// picture; while open it is held back, not streamed raw.
    #[tokio::test(start_paused = true)]
    async fn json_render_fence_is_a_picture_when_blocks_are_off() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(7, 7)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, false, true);
        t.text("Status:\n\n```json-render\n").await;
        t.text(CARD_SPEC).await;
        t.tick().await;
        let [ref opened] = api.starts()[..] else {
            panic!()
        };
        assert!(
            matches!(&opened[..], [Chunk::Text(s)] if s.trim() == "Status:"),
            "{opened:?}"
        );
        t.text("\n```\n\nDone.").await;
        t.finish(footer()).await;
        let calls = api.calls();
        assert!(
            !calls.iter().any(|c| matches!(c, Call::PostBlocks { .. })),
            "{calls:?}"
        );
        let [
            Call::Start { .. },
            Call::Stop { .. },
            Call::UploadFile { files, .. },
            Call::Start { chunks, .. },
            Call::Stop { .. },
        ] = &calls[..]
        else {
            panic!("{calls:?}")
        };
        assert_eq!(files[0].title.as_deref(), Some("Rollout"));
        assert_eq!(files[0].bytes, png(7, 7));
        assert_eq!(chunks, &vec![Chunk::Text("Done.".into())]);
        let sent = streamed_text(&api);
        assert!(
            !sent.contains("\"type\"") && !sent.contains("```"),
            "{sent}"
        );
    }

    /// 110 §8: a fence that renders goes out as one `blocks` chunk between
    /// the text before and after it, in one stream call.
    #[tokio::test(start_paused = true)]
    async fn json_render_fence_streams_as_a_blocks_chunk_between_text() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text(&format!(
            "Status:\n\n```json-render\n{CARD_SPEC}\n```\n\nDone."
        ))
        .await;
        t.tick().await;
        let [ref chunks] = api.starts()[..] else {
            panic!("{:?}", api.calls())
        };
        let [
            Chunk::Text(before),
            Chunk::Blocks(blocks),
            Chunk::Text(after),
        ] = &chunks[..]
        else {
            panic!("{chunks:?}")
        };
        assert_eq!(before, "Status:\n\n");
        assert_eq!(blocks[0]["type"], "container");
        assert_eq!(blocks[0]["title"]["text"], "Rollout");
        assert_eq!(after, "\n\nDone.");
        t.finish(footer()).await;
        assert!(!streamed_text(&api).contains("```"), "fence source leaked");
    }

    /// 164 D9: a fence with no block form (Graph) is a picture; with no
    /// renderer it is its text fallback, never its JSON.
    #[tokio::test(start_paused = true)]
    async fn unsupported_fence_streams_as_its_fallback_without_a_renderer() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text(&format!("See:\n\n```json-render\n{GRAPH_SPEC}\n```\n"))
            .await;
        t.finish(footer()).await;
        let chunks: Vec<Chunk> = api
            .calls()
            .into_iter()
            .flat_map(|c| match c {
                Call::Start { chunks, .. } | Call::Stop { chunks, .. } => chunks,
                _ => Vec::new(),
            })
            .collect();
        assert!(
            chunks.iter().all(|c| matches!(c, Chunk::Text(_))),
            "{chunks:?}"
        );
        let sent = streamed_text(&api);
        let fallback = surface::picture_fallback(PictureLang::JsonRender, GRAPH_SPEC, 60);
        assert_eq!(sent, format!("See:\n\n{fallback}\n"));
        assert!(!sent.contains("\"nodes\""), "{sent}");
    }

    /// 110 §8 update mode: the bubble's text leaves the fence out and the
    /// blocks follow as their own message right after the edit.
    #[tokio::test(start_paused = true)]
    async fn update_mode_posts_fence_blocks_after_the_edit() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        api.fail("start", "channel_type_not_supported");
        t.text("Status:\n\n").await;
        t.tick().await;
        assert!(matches!(api.calls()[..], [Call::Post { .. }]));
        t.text(&format!("```json-render\n{CARD_SPEC}\n```\n\nDone.\n\n"))
            .await;
        t.tick().await;
        let calls = api.calls();
        let [_, Call::Update { md, .. }, Call::PostBlocks { blocks }] = &calls[..] else {
            panic!("{calls:?}")
        };
        assert!(md.starts_with("Status:"), "{md}");
        assert!(!md.contains("```") && !md.contains("Rollout"), "{md}");
        assert!(!md.contains("Done."), "pending paragraph shown: {md}");
        assert_eq!(blocks[0]["type"], "container");
        // The final edit does not post the same blocks again.
        t.finish(footer()).await;
        let calls = api.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|c| matches!(c, Call::PostBlocks { .. }))
                .count(),
            1,
            "{calls:?}"
        );
        let Some(Call::Update { md, .. }) = calls.last() else {
            panic!("{calls:?}")
        };
        assert!(!md.contains("Rollout"), "{md}");
    }

    /// 110 §8, 164 D9: once a bubble carries 50 blocks, a further fence that
    /// fits a fresh message closes this one and is Block Kit in the next —
    /// nothing drawn.
    #[tokio::test(start_paused = true)]
    async fn fence_over_the_fifty_block_cap_opens_the_next_message() {
        // A Stack of 30 Separators: 30 `divider` blocks.
        let ids: Vec<String> = (0..30).map(|i| format!("\"d{i}\"")).collect();
        let elements: Vec<String> = (0..30)
            .map(|i| format!("\"d{i}\":{{\"type\":\"Separator\"}}"))
            .collect();
        let spec = format!(
            "{{\"root\":\"s\",\"elements\":{{\"s\":{{\"type\":\"Stack\",\"children\":[{}]}},{}}}}}",
            ids.join(","),
            elements.join(",")
        );
        let fence = format!("```json-render\n{spec}\n```\n\n");
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(2, 2)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        t.text(&format!("A\n\n{fence}")).await;
        t.tick().await;
        t.text(&format!("B\n\n{fence}C\n\n")).await;
        t.tick().await;
        let calls = api.calls();
        let [
            Call::Start { chunks: first, .. },
            Call::Append { chunks: second, .. },
            Call::Stop { status, .. },
            Call::Start { chunks: third, .. },
        ] = &calls[..]
        else {
            panic!("{calls:?}")
        };
        assert!(
            matches!(&first[..], [Chunk::Text(_), Chunk::Blocks(b), Chunk::Text(_)] if b.as_array().unwrap().len() == 30),
            "{first:?}"
        );
        // 30 + 30 > 50: the message closes after B; the fence opens the next.
        assert_eq!(second, &vec![Chunk::Text("B\n\n".into())]);
        assert_eq!(status.as_deref(), Some("processing"));
        assert!(
            matches!(&third[..], [Chunk::Blocks(b), Chunk::Text(c)] if b.as_array().unwrap().len() == 30 && c.trim() == "C"),
            "{third:?}"
        );
        assert!(!all_text(&api).contains("Separator"), "{calls:?}");
    }

    /// 164 D9 (a), stream mode: prose, a Graph fence and prose go out as
    /// text, a PNG in the thread (the renderer's bytes, title, alt) and
    /// text, in that order; the JSON never.
    #[tokio::test(start_paused = true)]
    async fn graph_fence_streams_as_a_picture_between_its_text() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(1440, 600)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        t.text(&format!(
            "Status:\n\n```json-render\n{GRAPH_SPEC}\n```\n\nDone."
        ))
        .await;
        t.tick().await;
        t.finish(footer()).await;
        let calls = api.calls();
        let [
            Call::Start {
                ts: first,
                chunks: before,
                ..
            },
            Call::Stop {
                ts: stopped,
                chunks: none,
                status,
                ..
            },
            Call::UploadFile { files, comment },
            Call::Start { chunks: after, .. },
            Call::Stop { footer: foot, .. },
        ] = &calls[..]
        else {
            panic!("{calls:?}")
        };
        assert_eq!(before, &vec![Chunk::Text("Status:\n\n".into())]);
        let [file] = &files[..] else {
            panic!("{files:?}")
        };
        assert_eq!(file.bytes, png(1440, 600));
        assert_eq!(file.mime, "image/png");
        assert_eq!(file.name, "diagram.png");
        assert_eq!(file.title.as_deref(), Some("Data flow"));
        let alt = file.alt.as_deref().expect("alt_txt");
        assert!(!alt.is_empty() && !alt.contains('{'), "{alt}");
        assert_eq!(comment, &None);
        assert_eq!(
            (stopped, none.len(), status.as_deref()),
            (first, 0, Some("processing"))
        );
        assert_eq!(after, &vec![Chunk::Text("Done.".into())]);
        assert!(foot.is_some());
        let text = all_text(&api);
        assert!(!text.contains("nodes") && !text.contains("```"), "{text}");
    }

    /// 164 D9 (a), update mode: the live edit shows `_drawing…_` in place
    /// of the fence, the picture goes up, the bubble closes before it and
    /// the text after it is its own message; the JSON never.
    #[tokio::test(start_paused = true)]
    async fn graph_fence_in_update_mode_is_a_picture_between_its_text() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(8, 8)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        api.fail("start", "channel_type_not_supported");
        t.text(&format!(
            "Status:\n\n```json-render\n{GRAPH_SPEC}\n```\n\nDone."
        ))
        .await;
        t.tick().await;
        t.finish(footer()).await;
        let calls = api.calls();
        let [
            Call::Post { md: live },
            Call::Update {
                ts: first,
                md: closed,
            },
            Call::UploadFile { files, .. },
            Call::Post { .. },
            Call::Update {
                ts: second,
                md: last,
            },
        ] = &calls[..]
        else {
            panic!("{calls:?}")
        };
        assert_eq!(live, &format!("Status:\n\n{DRAWING}"));
        assert_eq!(files[0].bytes, png(8, 8));
        assert_eq!((first.as_str(), closed.as_str()), ("ts0", "Status:\n\n"));
        assert_ne!(first, second);
        assert!(last.starts_with("Done.\n\n_m ·"), "{last}");
        let text = all_text(&api);
        assert!(!text.contains("nodes") && !text.contains("```"), "{text}");
    }

    /// 164 D7 (b): with nobody drawing, the fence is its text fallback in
    /// place; no upload, no JSON.
    #[tokio::test(start_paused = true)]
    async fn graph_fence_without_a_renderer_is_its_fallback_text() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text(&format!(
            "Status:\n\n```json-render\n{GRAPH_SPEC}\n```\n\nDone."
        ))
        .await;
        t.tick().await;
        t.finish(footer()).await;
        let calls = api.calls();
        assert!(
            !calls.iter().any(|c| matches!(c, Call::UploadFile { .. })),
            "{calls:?}"
        );
        assert_eq!(api.starts().len(), 1, "{calls:?}");
        let text = all_text(&api);
        let fallback = surface::picture_fallback(PictureLang::JsonRender, GRAPH_SPEC, 60);
        assert_eq!(text, format!("Status:\n\n{fallback}\n\nDone."));
        assert!(!text.contains("nodes"), "{text}");
    }

    /// 164 D10 (c): `pictures: false` sends the fallback and asks no client.
    #[tokio::test(start_paused = true)]
    async fn pictures_off_sends_the_fallback_and_requests_nothing() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(8, 8)).await;
        let hits = Arc::new(AtomicU32::new(0));
        let counter = Counter::spawn(Counter(Arc::clone(&hits)));
        rt.subscribe::<Counter, FenceSnapshotRequested>(&counter)
            .await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, false);
        t.text(&format!(
            "Status:\n\n```json-render\n{GRAPH_SPEC}\n```\n\nDone."
        ))
        .await;
        t.tick().await;
        t.finish(footer()).await;
        let calls = api.calls();
        assert!(
            !calls.iter().any(|c| matches!(c, Call::UploadFile { .. })),
            "{calls:?}"
        );
        assert_eq!(hits.load(Ordering::Relaxed), 0, "a snapshot was requested");
        let text = all_text(&api);
        let fallback = surface::picture_fallback(PictureLang::JsonRender, GRAPH_SPEC, 60);
        assert_eq!(text, format!("Status:\n\n{fallback}\n\nDone."));
    }

    /// Counts `FenceSnapshotRequested` on a runtime's bus.
    #[derive(kameo::Actor)]
    struct Counter(Arc<AtomicU32>);

    impl kameo::message::Message<FenceSnapshotRequested> for Counter {
        type Reply = ();
        async fn handle(
            &mut self,
            _: FenceSnapshotRequested,
            _: &mut kameo::message::Context<Self, ()>,
        ) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 164 D8 (d): a reply streamed byte by byte never puts a byte of the
    /// fence — opener line included — into any stream call; the picture
    /// still lands between the prose.
    #[tokio::test(start_paused = true)]
    async fn fence_streamed_byte_by_byte_never_leaks() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(3, 3)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        let reply = "Here:\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nDone.";
        for (i, ch) in reply.char_indices() {
            t.text(&reply[i..i + ch.len_utf8()]).await;
            t.tick().await;
            let text = all_text(&api);
            assert!(
                !text.contains('`') && !text.contains("graph") && !text.contains("-->"),
                "after {:?}: {text}",
                &reply[..=i]
            );
        }
        t.finish(footer()).await;
        let calls = api.calls();
        let upload = calls
            .iter()
            .position(|c| matches!(c, Call::UploadFile { .. }))
            .unwrap_or_else(|| panic!("{calls:?}"));
        let before = all_text_of(&calls[..upload]);
        let after = all_text_of(&calls[upload..]);
        assert_eq!(before, "Here:\n\n");
        assert_eq!(after, "Done.");
    }

    /// 164 D9 (f): a Checklist fence still streams as Block Kit while a
    /// Graph fence in the same reply is a picture; with `blocks: false` the
    /// Checklist is a picture too.
    #[tokio::test(start_paused = true)]
    async fn block_capable_fence_stays_blocks_while_graph_is_a_picture() {
        let reply = format!(
            "Status:\n\n```json-render\n{CARD_SPEC}\n```\n\nThen:\n\n```json-render\n{GRAPH_SPEC}\n```\n\nDone."
        );
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(4, 4)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        t.text(&reply).await;
        t.tick().await;
        t.finish(footer()).await;
        let calls = api.calls();
        let [
            Call::Start { chunks: first, .. },
            Call::Stop { .. },
            Call::UploadFile { files, .. },
            Call::Start { chunks: second, .. },
            Call::Stop { .. },
        ] = &calls[..]
        else {
            panic!("{calls:?}")
        };
        let [Chunk::Text(a), Chunk::Blocks(b), Chunk::Text(c)] = &first[..] else {
            panic!("{first:?}")
        };
        assert_eq!((a.as_str(), c.as_str()), ("Status:\n\n", "\n\nThen:\n\n"));
        assert_eq!(b[0]["title"]["text"], "Rollout");
        assert_eq!(files[0].title.as_deref(), Some("Data flow"));
        assert_eq!(second, &vec![Chunk::Text("Done.".into())]);

        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(5, 5)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, false, true);
        t.text(&reply).await;
        t.tick().await;
        t.finish(footer()).await;
        let calls = api.calls();
        let uploads: Vec<&str> = calls
            .iter()
            .filter_map(|c| match c {
                Call::UploadFile { files, .. } => files[0].title.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(uploads, ["Rollout", "Data flow"], "{calls:?}");
        assert!(
            !calls.iter().any(|c| matches!(c, Call::PostBlocks { .. })),
            "{calls:?}"
        );
        let text = all_text(&api);
        assert!(
            !text.contains("```") && !text.contains("\"type\""),
            "{text}"
        );
        assert_eq!(text, "Status:\n\nThen:\n\nDone.");
    }

    /// A reply that ends in a picture still gets its footer, as its own line.
    #[tokio::test(start_paused = true)]
    async fn footer_follows_a_reply_that_ends_in_a_picture() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(6, 6)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        t.text("See:\n\n```mermaid\ngraph LR\n  A --> B\n```\n")
            .await;
        t.finish(footer()).await;
        let calls = api.calls();
        let [
            Call::Start { .. },
            Call::Stop { status, .. },
            Call::UploadFile { files, .. },
            Call::Post { md },
        ] = &calls[..]
        else {
            panic!("{calls:?}")
        };
        assert_eq!(files[0].title.as_deref(), Some("diagram"));
        assert_eq!(status.as_deref(), Some("processing"));
        assert_eq!(md, "_m · 12.3k in / 800 out · 42 s · 3 tools_");
    }

    /// The text chunks, posts and updates of `calls`, concatenated.
    fn all_text_of(calls: &[Call]) -> String {
        calls
            .iter()
            .flat_map(|c| match c {
                Call::Start { chunks, .. }
                | Call::Append { chunks, .. }
                | Call::Stop { chunks, .. } => chunks
                    .iter()
                    .filter_map(|c| match c {
                        Chunk::Text(s) => Some(s.clone()),
                        Chunk::Task { .. } | Chunk::Blocks(_) => None,
                    })
                    .collect(),
                Call::Post { md } | Call::Update { md, .. } => vec![md.clone()],
                Call::Upload | Call::UploadFile { .. } | Call::PostBlocks { .. } => Vec::new(),
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_after_idle() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "shell", &serde_json::json!({"command": "sleep 60"}))
            .await;
        t.tick().await;
        assert_eq!(api.calls().len(), 1);
        tokio::time::advance(Duration::from_secs(20)).await;
        t.tick().await;
        assert_eq!(api.calls().len(), 1, "no keepalive before {KEEPALIVE:?}");
        tokio::time::advance(Duration::from_secs(6)).await;
        t.tick().await;
        let calls = api.calls();
        assert_eq!(calls.len(), 2);
        let Call::Append { chunks, .. } = &calls[1] else {
            panic!("{calls:?}")
        };
        assert!(matches!(
            &chunks[0],
            Chunk::Task {
                status: TaskStatus::InProgress,
                ..
            }
        ));
        // Nothing else in progress → no further keepalive spam right away.
        t.tick().await;
        assert_eq!(api.calls().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn finish_stops_with_footer() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text("done").await;
        t.tick().await;
        t.text(" now").await;
        t.finish(footer()).await;
        let calls = api.calls();
        let Call::Stop {
            ts,
            chunks,
            footer: foot,
            status,
        } = calls.last().unwrap()
        else {
            panic!("{calls:?}")
        };
        assert_eq!(ts, "ts0");
        assert_eq!(chunks, &vec![Chunk::Text(" now".into())]);
        assert_eq!(
            foot.as_deref(),
            Some("m · 12.3k in / 800 out · 42 s · 3 tools")
        );
        assert_eq!(status.as_deref(), Some("active"));
        assert_eq!(t.open_stream(), None);

        let quiet = FakeApi::new();
        let mut t = turn(&quiet, ToolCards::All);
        t.tick().await;
        t.finish(footer()).await;
        assert!(quiet.calls().is_empty());
    }

    /// 21 §8.4: a stream that takes no more is stopped as it stands —
    /// never `chat.update`d, which Slack refuses while it streams — and the
    /// rest goes on in a new one with the live card.
    #[tokio::test(start_paused = true)]
    async fn dead_stream_is_stopped_and_goes_on_in_a_new_one() {
        for code in ["message_not_in_streaming_state", "msg_too_long"] {
            let api = FakeApi::new();
            let mut t = turn(&api, ToolCards::All);
            t.text("para one\n\n").await;
            t.tick().await;
            t.tool_started("c1", "shell", &serde_json::json!({"command": "ls"}))
                .await;
            t.text("more").await;
            api.fail("append", code);
            t.tick().await;
            assert_eq!(t.open_stream(), None, "{code}");
            t.tick().await;
            let calls = api.calls();
            assert!(
                !calls.iter().any(|c| matches!(c, Call::Update { .. })),
                "{code}: {calls:?}"
            );
            let thread = api.thread();
            let [old, new] = &thread[..] else {
                panic!("{code}: {thread:?}")
            };
            assert_eq!(
                (old.text.as_str(), old.open),
                ("para one\n\n", false),
                "{code}"
            );
            assert_eq!(new.text, "more", "{code}");
            assert_eq!(new.cards[0].id, "c1");
            assert_eq!(new.cards[0].status, TaskStatus::InProgress);
            assert_eq!(t.open_stream(), Some(new.ts.as_str()));
            assert_eq!(api.refused.load(Ordering::Relaxed), 0, "{code}");
        }
    }

    /// `s` fed as deltas of ≤ 120 bytes (ASCII), a tick after each.
    async fn feed(t: &mut Turn, s: &str, reasoning: bool) {
        for piece in s.as_bytes().chunks(120) {
            let piece = std::str::from_utf8(piece).unwrap();
            if reasoning {
                t.reasoning(piece).await;
            } else {
                t.text(piece).await;
            }
            t.tick().await;
            tokio::time::advance(EDIT_FLOOR / 2).await;
        }
    }

    /// `n` chars of fixed-width lines `tag-NNN …`: no line is another's
    /// prefix, so a line split across two messages never fakes a repeat.
    fn lines_of(tag: &str, n: usize) -> String {
        (0..)
            .map(|i| format!("{tag}-{i:03} lorem ipsum dolor sit amet\n"))
            .scan(0, |len, l| {
                *len += l.len();
                (*len <= n.max(l.len())).then_some(l)
            })
            .collect()
    }

    /// A whole `shell` call over two ticks, the card floor passing.
    async fn slow_call(t: &mut Turn, id: &str) {
        t.tool_started(id, "shell", &sh(id)).await;
        t.tick().await;
        tokio::time::advance(EDIT_FLOOR).await;
        t.tool_finished(id, &format!("{id} ok"), false).await;
        t.tick().await;
    }

    /// The reply Sky's thread of 2026-11-03 died on, `scale` chars of text
    /// and reasoning in all: leading reasoning, a batch with a long thought
    /// between two calls, a Graph fence, two calls, a mermaid fence,
    /// trailing reasoning, text; then the turn ends. Returns the text Slack
    /// should show, `[pic]` where a picture goes.
    async fn long_reply(t: &mut Turn, scale: usize) -> String {
        let mut want = String::new();
        let part = scale / 6;
        feed(t, &lines_of("lead", part), true).await;
        let intro = lines_of("intro", part) + "\n";
        feed(t, &intro, false).await;
        want.push_str(&intro);
        slow_call(t, "c1").await;
        feed(t, &lines_of("mid", part), true).await;
        t.reasoning_done(9_000);
        slow_call(t, "c2").await;
        let graph = format!("## 1 Graph\n\n```json-render\n{GRAPH_SPEC}\n```\n\n");
        feed(t, &graph, false).await;
        want.push_str("## 1 Graph\n\n[pic]");
        let after = lines_of("after", part) + "\n";
        feed(t, &after, false).await;
        want.push_str(&after);
        slow_call(t, "c3").await;
        slow_call(t, "c4").await;
        feed(
            t,
            "## 2 Mermaid\n\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n\n",
            false,
        )
        .await;
        want.push_str("## 2 Mermaid\n\n[pic]");
        feed(t, &lines_of("tail", part), true).await;
        let end = lines_of("end", part);
        feed(t, &end, false).await;
        want.push_str(&end);
        t.finish(footer()).await;
        want
    }

    /// The thread as Slack shows it is the reply: every message stopped,
    /// no card left spinning, no card line twice anywhere, a `Thought`
    /// card without a `details` copy of its body, both pictures once and
    /// in place, the text complete.
    fn assert_thread_is(api: &FakeApi, want: &str) {
        let mut got = String::new();
        let mut lines = std::collections::HashSet::new();
        for m in api.thread() {
            assert!(!m.open, "left streaming: {m:?}");
            if m.picture.is_some() {
                got.push_str("[pic]");
                continue;
            }
            got.push_str(&m.text);
            for c in &m.cards {
                assert!(
                    matches!(c.status, TaskStatus::Complete | TaskStatus::Error),
                    "spinning: {c:?}"
                );
                if c.id.starts_with("reasoning-") {
                    assert_eq!(c.details, None, "{c:?}");
                }
                for l in c.output.iter().flat_map(|o| o.lines()) {
                    assert!(
                        l.is_empty() || lines.insert(l.to_owned()),
                        "shown twice: {l}"
                    );
                }
            }
        }
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("💭 thought 9 s — mid-000")),
            "{lines:?}"
        );
        assert!(
            lines.contains("tail-000 lorem ipsum dolor sit amet"),
            "{lines:?}"
        );
        let squash = |s: &str| -> String {
            s.replace(&format!("_{}_", footer()), "")
                .split_whitespace()
                .collect()
        };
        assert_eq!(squash(&got), squash(want));
    }

    /// 164: a reply past Slack's 12 000 chars per stream — text and card
    /// output together — rolls over before Slack refuses anything.
    #[tokio::test(start_paused = true)]
    async fn long_reply_rolls_over_before_slack_refuses() {
        let api = FakeApi::new();
        let rt = runtime_with_renderer(png(8, 8)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        let want = long_reply(&mut t, 48_000).await;
        assert_eq!(api.refused.load(Ordering::Relaxed), 0, "{:?}", api.calls());
        // Two pictures make three streams; the size makes more.
        let sizes: Vec<(usize, usize, usize)> = api
            .thread()
            .iter()
            .map(|m| (m.text.len(), m.used, m.cards.len()))
            .collect();
        assert!(
            api.starts().len() >= 5,
            "{} streams: {sizes:?}",
            api.starts().len()
        );
        assert_thread_is(&api, &want);
    }

    /// 164: when Slack's cap is far below what the bubble budgets for,
    /// streams die `msg_too_long` again and again and the reply survives:
    /// each dead stream is stopped (Slack refuses `chat.update` while it
    /// streams), the next goes on where it ended, pictures stay in place.
    #[tokio::test(start_paused = true)]
    async fn streams_dying_msg_too_long_lose_and_repeat_nothing() {
        let api = FakeApi::new();
        api.cap.store(1_000, Ordering::Relaxed);
        let rt = runtime_with_renderer(png(8, 8)).await;
        let mut t = turn_in(rt, &api, ToolCards::All, true, true, true);
        let want = long_reply(&mut t, 6_000).await;
        assert!(api.refused.load(Ordering::Relaxed) > 0, "the cap never bit");
        assert!(
            !api.calls().iter().any(|c| matches!(c, Call::Update { .. })),
            "{:?}",
            api.calls()
        );
        assert_thread_is(&api, &want);
    }

    #[tokio::test(start_paused = true)]
    async fn long_text_rotates_at_block_boundaries_without_loss() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        let para = format!("{}\n\n", "x".repeat(498));
        let input = para.repeat(60);
        for _ in 0..60 {
            t.text(&para).await;
            t.tick().await;
        }
        t.finish(footer()).await;
        // Rebuild each bubble's final text: chunks in order, replaced by a
        // later chat.update on the same ts.
        let mut order: Vec<String> = Vec::new();
        let mut texts: HashMap<String, String> = HashMap::new();
        for c in api.calls() {
            match c {
                Call::Start { ts, chunks } => {
                    order.push(ts.clone());
                    texts.insert(ts, text_of(&chunks));
                }
                Call::Append { ts, chunks } | Call::Stop { ts, chunks, .. } => {
                    texts.get_mut(&ts).unwrap().push_str(&text_of(&chunks));
                }
                Call::Update { ts, md } => {
                    texts.insert(ts, md);
                }
                Call::Post { .. } => panic!("no fallback expected"),
                Call::Upload | Call::UploadFile { .. } | Call::PostBlocks { .. } => panic!(),
            }
        }
        assert!(order.len() >= 3, "{} bubbles", order.len());
        let joined: String = order.iter().map(|ts| texts[ts].as_str()).collect();
        assert_eq!(joined, input);
        for ts in &order {
            let n = texts[ts].chars().count();
            assert!(
                n <= BUBBLE_SOFT_MAX + para.len(),
                "bubble {ts} has {n} chars"
            );
        }
        // Every rotated bubble was stopped exactly once.
        let stops = api
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Stop { .. }))
            .count();
        assert_eq!(stops, order.len());
    }

    fn text_of(chunks: &[Chunk]) -> String {
        chunks
            .iter()
            .filter_map(|c| match c {
                Chunk::Text(s) => Some(s.as_str()),
                Chunk::Task { .. } | Chunk::Blocks(_) => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_channel_falls_back_to_update_mode() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        api.fail("start", "channel_type_not_supported");
        t.text("first\n\nsecond").await;
        t.tick().await;
        let calls = api.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let Call::Post { md } = &calls[0] else {
            panic!("{calls:?}")
        };
        assert!(md.starts_with("first"));
        assert!(!md.contains("second"), "pending block shown: {md}");
        assert_eq!(t.open_stream(), None);
        t.text(" more\n\nthird").await;
        t.tick().await;
        let calls = api.calls();
        let Call::Update { ts, md } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        assert_eq!(ts, "ts0");
        assert!(md.contains("second more"));
        assert!(!md.contains("third"));
        t.finish(footer()).await;
        let calls = api.calls();
        let Call::Update { md, .. } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        assert!(md.contains("third"));
        assert!(md.ends_with("_m · 12.3k in / 800 out · 42 s · 3 tools_"));
        assert!(!calls.iter().any(|c| matches!(c, Call::Stop { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_segment_quotes_committed_text() {
        let api = FakeApi::new();
        let mut s = Segment::new("C1".into(), "1.0".into(), api.clone(), Some("> "));
        s.tick().await;
        assert!(api.calls().is_empty(), "nothing to post yet");
        s.push("thinking about\nthings\n\nnext").await;
        s.tick().await;
        s.tick().await;
        let calls = api.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(matches!(&calls[0], Call::Post { md } if md == "_thinking…_"));
        let Call::Update { ts, md } = &calls[1] else {
            panic!()
        };
        assert_eq!(ts, "ts0");
        assert_eq!(md, "> thinking about\n> things");
        s.finish().await;
        let calls = api.calls();
        let Call::Update { md, .. } = calls.last().unwrap() else {
            panic!()
        };
        assert_eq!(md, "> thinking about\n> things\n>\n> next");
        assert_eq!(
            calls
                .iter()
                .filter(|c| matches!(c, Call::Post { .. }))
                .count(),
            1
        );
    }

    /// 164-Z R1: a model that repeats a picture fence while thinking (seen live on Telegram).
    const FENCED_THINKING: &str = "Let me reproduce it:\n\n```json-render\n{\"root\":\"g\",\"elements\":{\"g\":{\"type\":\"Graph\",\"props\":{\"nodes\":[{\"id\":\"a\",\"label\":\"Alpha\"},{\"id\":\"b\",\"label\":\"Beta\"}],\"edges\":[{\"from\":\"a\",\"to\":\"b\"}]}}}}\n```\n\nDone.";

    /// A fence line (or a half-typed one) or the body.
    fn leaks_fence(s: &str) -> bool {
        ["\"root\"", "\"elements\"", "```j", "```m", "```o", "{\"g\""]
            .iter()
            .any(|p| s.contains(p))
    }

    /// Streams `FENCED_THINKING` in 3-byte deltas, a tick after each.
    async fn stream_fenced_thinking(t: &mut Turn) {
        for chunk in FENCED_THINKING.as_bytes().chunks(3) {
            t.reasoning(std::str::from_utf8(chunk).unwrap()).await;
            t.tick().await;
        }
        t.reasoning_done(1_000);
        t.text("answer").await;
        t.tick().await;
    }

    /// Stream mode: the `reasoning-1` card's output, as Slack appends it, never shows a fence
    /// line or the body at any point, and ends as the D7 text exactly once — no send repeats
    /// what an earlier one showed; the `Thought` card has no `details` to repeat it either.
    #[tokio::test(start_paused = true)]
    async fn stream_reasoning_never_shows_a_picture_fence_body() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        stream_fenced_thinking(&mut t).await;
        let outs: Vec<String> = task_of(&api.task_chunks(), "reasoning-1")
            .into_iter()
            .filter_map(|(_, _, o)| o)
            .collect();
        assert!(outs.len() > 2, "{outs:?}");
        let mut shown = String::new();
        let mut seen = Vec::new();
        for o in &outs {
            shown.push_str(o);
            assert!(!leaks_fence(&shown), "{shown}");
            seen.push(shown.clone());
        }
        assert!(
            seen.iter().any(|s| s == "Let me reproduce it:\n\n"),
            "{seen:?}"
        );
        let card = last_card(&api, "reasoning-1");
        assert_eq!(card.status, TaskStatus::Complete);
        assert_eq!(card.details, None);
        let body = card.output.unwrap();
        assert_eq!(body, reasoning_text(FENCED_THINKING, false));
        assert!(
            body.contains("Alpha") && body.ends_with("```\n\nDone."),
            "{body}"
        );
        // A thought line inside a batch card: the same rule.
        let mut th = Thought::new(&FENCED_THINKING["Let me reproduce it:\n\n".len()..]);
        assert!(
            !leaks_fence(&th.line()) && th.line().contains(DRAWING),
            "{}",
            th.line()
        );
        th.end();
        assert!(!leaks_fence(&th.line()), "{}", th.line());
    }

    /// Update mode: every quoted reasoning message shows no fence line and no body; the last
    /// carries the D7 text.
    #[tokio::test(start_paused = true)]
    async fn update_reasoning_never_shows_a_picture_fence_body() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        api.fail("start", "channel_type_not_supported");
        stream_fenced_thinking(&mut t).await;
        let quoted: Vec<String> = api
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Update { ts, md } if ts == "ts0" => Some(md),
                _ => None,
            })
            .collect();
        assert!(quoted.len() > 1, "{:?}", api.calls());
        for md in &quoted {
            assert!(!leaks_fence(md), "{md}");
        }
        assert!(quoted.iter().any(|md| md.contains(DRAWING)), "{quoted:?}");
        let last = quoted.last().unwrap();
        assert!(
            last.contains("Alpha") && last.ends_with("> Done."),
            "{last}"
        );
    }

    fn task_of(chunks: &[Chunk], want: &str) -> Vec<(TaskStatus, Option<String>, Option<String>)> {
        chunks
            .iter()
            .filter_map(|c| match c {
                Chunk::Task {
                    id,
                    status,
                    details,
                    output,
                    ..
                } if id == want => Some((status.clone(), details.clone(), output.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_off_emits_nothing() {
        let api = FakeApi::new();
        let mut t = turn_with(&api, ToolCards::All, false, true);
        t.reasoning("hidden").await;
        t.tick().await;
        assert!(api.calls().is_empty());
        t.text("x").await;
        t.tick().await;
        assert!(api.task_chunks().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_in_update_mode_is_a_quoted_message_before_the_bubble() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        api.fail("start", "channel_type_not_supported");
        t.reasoning("why\n\nbecause").await;
        t.tick().await;
        let calls = api.calls();
        assert!(
            matches!(&calls[0], Call::Post { md } if md == "_thinking…_"),
            "{calls:?}"
        );
        assert!(
            matches!(&calls[1], Call::Update { ts, md } if ts == "ts0" && md == "> why"),
            "{calls:?}"
        );
        assert_eq!(calls.len(), 2, "no bubble yet: {calls:?}");
        t.text("reply").await;
        t.tick().await;
        let calls = api.calls();
        assert!(
            matches!(&calls[2], Call::Update { ts, md } if ts == "ts0" && md == "> why\n>\n> because"),
            "{calls:?}"
        );
        assert!(
            matches!(&calls[3], Call::Post { md } if md == "…"),
            "{calls:?}"
        );
        assert!(api.task_chunks().is_empty());
        // Later runs in the same thread go straight to a new quoted message.
        t.reasoning("more").await;
        t.tick().await;
        assert!(matches!(api.calls().last().unwrap(), Call::Post { md } if md == "_thinking…_"));
    }

    #[derive(Clone, Debug)]
    struct Card {
        id: String,
        title: String,
        status: TaskStatus,
        details: Option<String>,
        output: Option<String>,
        sources: Vec<Source>,
    }

    fn cards(chunks: &[Chunk]) -> Vec<Card> {
        chunks
            .iter()
            .filter_map(|c| match c {
                Chunk::Task {
                    id,
                    title,
                    status,
                    details,
                    output,
                    sources,
                } => Some(Card {
                    id: id.clone(),
                    title: title.clone(),
                    status: status.clone(),
                    details: details.clone(),
                    output: output.clone(),
                    sources: sources.clone(),
                }),
                Chunk::Text(_) | Chunk::Blocks(_) => None,
            })
            .collect()
    }

    /// Card `id` as Slack shows it in the last message holding it.
    fn last_card(api: &FakeApi, id: &str) -> Card {
        api.thread()
            .into_iter()
            .rev()
            .find_map(|m| m.cards.into_iter().find(|c| c.id == id))
            .unwrap_or_else(|| panic!("no card {id}: {:?}", api.calls()))
    }

    fn ids(api: &FakeApi) -> Vec<String> {
        cards(&api.task_chunks())
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    fn sh(cmd: &str) -> serde_json::Value {
        serde_json::json!({"command": cmd})
    }

    /// A whole `shell` call: started, then finished with `output`.
    async fn run(t: &mut Turn, id: &str, cmd: &str, output: &str, err: bool) {
        t.tool_started(id, "shell", &sh(cmd)).await;
        t.tool_finished(id, output, err).await;
    }

    /// 110 §4.1–4.3: two calls are one card keyed by the first call id, its
    /// output the itemised list in event order.
    #[tokio::test(start_paused = true)]
    async fn two_calls_fold_into_one_card_with_the_first_call_id() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_pending("c1", "shell").await;
        t.tool_pending("c2", "read_file").await;
        t.tool_started("c1", "shell", &sh("ls")).await;
        t.tool_started("c2", "read_file", &serde_json::json!({"path": "a.rs"}))
            .await;
        t.tool_finished("c1", "a.rs\nb.rs", false).await;
        t.tool_finished("c2", "fn main() {}", false).await;
        t.text("done").await;
        t.tick().await;
        let cards = cards(&api.task_chunks());
        assert_eq!(cards.len(), 1, "{cards:?}");
        let c = &cards[0];
        assert_eq!(c.id, "c1");
        assert_eq!(c.title, "2 tools · 0.0 s");
        assert_eq!(c.status, TaskStatus::Complete);
        assert_eq!(c.details.as_deref(), Some("fn main() {}"));
        assert_eq!(
            c.output.as_deref(),
            Some("✓ shell · ls · 0.0 s · a.rs\n✓ read_file · path: a.rs · 0.0 s · fn main() {}")
        );
        assert_eq!(t.tool_count(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn text_between_calls_splits_batches() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        run(&mut t, "c1", "a", "1", false).await;
        run(&mut t, "c2", "b", "2", false).await;
        t.text("mid\n\n").await;
        run(&mut t, "c3", "c", "3", false).await;
        run(&mut t, "c4", "d", "4", false).await;
        t.finish(footer()).await;
        let mut seen = ids(&api);
        seen.sort();
        seen.dedup();
        assert_eq!(seen, ["c1", "c3"]);
        assert_eq!(last_card(&api, "c1").output.unwrap().lines().count(), 2);
        let second = last_card(&api, "c3").output.unwrap();
        assert!(second.starts_with("✓ shell · c"), "{second}");
        assert!(!second.contains("· a ·"), "{second}");
    }

    /// 110 §4.1: `[a, b] show [c, d]` → two batch cards around one plain card.
    #[tokio::test(start_paused = true)]
    async fn deliverable_show_splits_a_batch_into_two_cards_around_it() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        for id in ["a", "b", "show", "c", "d"] {
            let name = if id == "show" { "show" } else { "shell" };
            t.tool_started(id, name, &sh(id)).await;
        }
        for id in ["a", "b", "show", "c", "d"] {
            t.tool_finished(id, "ok", false).await;
        }
        t.finish(footer()).await;
        let mut seen = ids(&api);
        seen.sort();
        seen.dedup();
        assert_eq!(seen, ["a", "c", "show"]);
        assert_eq!(last_card(&api, "a").output.unwrap().lines().count(), 2);
        let show = last_card(&api, "show");
        assert_eq!(show.output, None, "a deliverable is a plain card");
        assert_eq!(show.title, "show · command: show");
        assert_eq!(last_card(&api, "c").output.unwrap().lines().count(), 2);
    }

    /// 110 §4.1: a pending `doc_create` rides in the batch until its args
    /// name a board; a note stays in.
    #[tokio::test(start_paused = true)]
    async fn pending_doc_create_of_a_board_is_split_out_when_args_arrive() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_pending("a", "shell").await;
        t.tool_pending("d", "doc_create").await;
        t.tool_pending("c", "shell").await;
        t.tick().await;
        let early = last_card(&api, "a");
        assert_eq!(early.title, "running `shell` · 1 of 3");
        assert_eq!(early.output, None, "nothing settled yet");
        t.tool_started("a", "shell", &sh("a")).await;
        t.tool_started(
            "d",
            "doc_create",
            &serde_json::json!({"kind": "board", "title": "Plan"}),
        )
        .await;
        t.tool_started("c", "shell", &sh("c")).await;
        for id in ["a", "d", "c"] {
            t.tool_finished(id, "ok", false).await;
        }
        t.finish(footer()).await;
        assert_eq!(last_card(&api, "a").output, None);
        assert_eq!(
            last_card(&api, "d").title,
            "doc_create · kind: board · title: Plan"
        );
        assert_eq!(last_card(&api, "c").output, None);

        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_pending("a", "shell").await;
        t.tool_pending("n", "doc_create").await;
        t.tool_started("a", "shell", &sh("a")).await;
        t.tool_started("n", "doc_create", &serde_json::json!({"kind": "note"}))
            .await;
        t.tool_finished("a", "ok", false).await;
        t.tool_finished("n", "ok", false).await;
        t.finish(footer()).await;
        assert_eq!(ids(&api), ["a"]);
        assert_eq!(last_card(&api, "a").output.unwrap().lines().count(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn lone_call_renders_as_a_plain_card() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_pending("c1", "shell").await;
        t.tick().await;
        tokio::time::advance(EDIT_FLOOR).await;
        t.tool_started("c1", "shell", &sh("cargo test -p x")).await;
        t.tick().await;
        tokio::time::advance(EDIT_FLOOR).await;
        t.tool_finished("c1", "ok: 12 passed\nmore", false).await;
        t.tick().await;
        let cards = cards(&api.task_chunks());
        assert_eq!(cards.len(), 3, "{cards:?}");
        assert!(cards.iter().all(|c| c.id == "c1" && c.output.is_none()));
        assert_eq!(cards[0].title, "shell");
        assert_eq!(cards[0].status, TaskStatus::Pending);
        assert_eq!(cards[1].title, "shell · cargo test -p x");
        assert_eq!(cards[1].status, TaskStatus::InProgress);
        assert_eq!(cards[2].status, TaskStatus::Complete);
        assert_eq!(cards[2].details.as_deref(), Some("ok: 12 passed"));
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_between_calls_is_a_thought_line_inside_the_card() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        run(&mut t, "c1", "ps", "PID", false).await;
        t.reasoning("Three sessions were bound to worktrees\nmore")
            .await;
        t.reasoning_done(9_000);
        run(&mut t, "c2", "git log", "abc", false).await;
        t.text("x").await;
        t.tick().await;
        assert_eq!(ids(&api), ["c1"], "no reasoning card");
        assert_eq!(
            last_card(&api, "c1").output.as_deref(),
            Some(
                "✓ shell · ps · 0.0 s · PID\n💭 thought 9 s — Three sessions were bound to worktrees\n✓ shell · git log · 0.0 s · abc"
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn trailing_reasoning_is_popped_and_becomes_a_thought_card_titled_with_its_seconds() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        run(&mut t, "c1", "a", "1", false).await;
        run(&mut t, "c2", "b", "2", false).await;
        t.reasoning("Let me check").await;
        t.tick().await;
        let live = last_card(&api, "c1");
        assert_eq!(live.status, TaskStatus::InProgress);
        assert_eq!(live.title, "thinking · 2 tools");
        let settled = "✓ shell · a · 0.0 s · 1\n✓ shell · b · 0.0 s · 2";
        assert_eq!(
            live.output.as_deref(),
            Some(settled),
            "a live thought is no line yet"
        );
        t.reasoning_done(12_000);
        t.text("answer").await;
        t.tick().await;
        let calls = api.calls();
        let Call::Append { chunks, .. } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        let [batch, thought, Chunk::Text(text)] = &chunks[..] else {
            panic!("{chunks:?}")
        };
        assert_eq!(text, "answer");
        let [batch, thought] = &cards(&[batch.clone(), thought.clone()])[..] else {
            panic!()
        };
        assert_eq!(batch.id, "c1");
        assert_eq!(batch.status, TaskStatus::Complete);
        assert_eq!(batch.output, None, "nothing new for the card");
        assert_eq!(last_card(&api, "c1").output.as_deref(), Some(settled));
        assert_eq!(thought.id, "reasoning-1");
        assert_eq!(thought.title, "Thought 12 s");
        assert_eq!(thought.status, TaskStatus::Complete);
        assert_eq!(thought.details, None, "the body once, not again as details");
        assert_eq!(thought.output.as_deref(), Some("Let me check"));
    }

    #[tokio::test(start_paused = true)]
    async fn live_title_counts_running_n_of_m_with_backticked_name() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "shell", &sh("ls")).await;
        t.tool_started(
            "c2",
            "python",
            &serde_json::json!({"code": "await read_file('x')"}),
        )
        .await;
        t.tool_started("c3", "read_file", &serde_json::json!({"path": "y"}))
            .await;
        t.tool_finished("c1", "a\nb\nc", false).await;
        t.tick().await;
        let c = last_card(&api, "c1");
        assert_eq!(c.title, "running `python` · 2 of 3");
        assert_eq!(c.status, TaskStatus::InProgress);
        assert_eq!(c.details.as_deref(), Some("a"));
        assert_eq!(
            c.output.as_deref(),
            Some("✓ shell · ls · 0.0 s · a"),
            "running calls are the title, not lines yet"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn done_title_names_failed_tools_in_order() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "shell", &sh("ls")).await;
        t.tool_started("c2", "python", &serde_json::json!({"code": "x"}))
            .await;
        t.tool_started("c3", "session_read", &serde_json::json!({"id": "s"}))
            .await;
        t.tool_finished("c1", "ok", false).await;
        t.tool_finished("c2", "KeyError: 'body'", true).await;
        t.tool_finished("c3", "", true).await;
        t.text("x").await;
        t.tick().await;
        let c = last_card(&api, "c1");
        assert_eq!(c.title, "3 tools · 0.0 s · 2 failed (python, session_read)");
        assert_eq!(c.status, TaskStatus::Error);
        assert_eq!(
            c.output.as_deref(),
            Some(
                "✓ shell · ls · 0.0 s · ok\n✗ python · <1 lines> · 0.0 s · KeyError: 'body'\n✗ session_read · id: s · 0.0 s · <0 lines>"
            )
        );

        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        run(&mut t, "c1", "a", "ok", false).await;
        run(&mut t, "c2", "b", "boom", true).await;
        t.text("x").await;
        t.tick().await;
        assert_eq!(
            last_card(&api, "c1").title,
            "2 tools · 0.0 s · 1 failed (shell)"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn durations_format_seconds_then_minutes() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "shell", &sh("a")).await;
        t.tool_started("c2", "shell", &sh("b")).await;
        tokio::time::advance(Duration::from_millis(12_300)).await;
        t.tool_finished("c1", "1", false).await;
        t.tool_finished("c2", "2", false).await;
        t.text("x").await;
        t.tick().await;
        assert_eq!(last_card(&api, "c1").title, "2 tools · 12.3 s");

        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "shell", &sh("a")).await;
        t.tool_started("c2", "shell", &sh("b")).await;
        tokio::time::advance(Duration::from_secs(150)).await;
        t.tool_finished("c1", "1", false).await;
        t.tool_finished("c2", "2", false).await;
        t.text("x").await;
        t.tick().await;
        assert_eq!(last_card(&api, "c1").title, "2 tools · 3 min");
        assert_eq!(fmt_run(Duration::from_millis(59_950)), "60.0 s");
        assert_eq!(fmt_run(Duration::from_secs(60)), "1 min");
    }

    /// 110 §4.4: a live card waits 1.5 s between sends; a closed batch's
    /// last change goes at once.
    #[tokio::test(start_paused = true)]
    async fn card_is_not_resent_within_the_floor_but_always_on_close() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "shell", &sh("a")).await;
        t.tool_started("c2", "shell", &sh("b")).await;
        t.tick().await;
        assert_eq!(api.calls().len(), 1, "first send is immediate");
        t.tool_finished("c1", "1", false).await;
        t.tick().await;
        assert_eq!(api.calls().len(), 1, "within the floor");
        tokio::time::advance(EDIT_FLOOR).await;
        t.tick().await;
        assert_eq!(api.calls().len(), 2, "floor passed");
        assert_eq!(last_card(&api, "c1").title, "running `shell` · 2 of 2");
        t.tool_finished("c2", "2", false).await;
        t.tick().await;
        assert_eq!(api.calls().len(), 2, "batch still open: floor applies");
        t.text("done").await;
        t.tick().await;
        let calls = api.calls();
        assert_eq!(calls.len(), 3, "{calls:?}");
        let Call::Append { chunks, .. } = &calls[2] else {
            panic!("{calls:?}")
        };
        assert!(
            matches!(&chunks[..], [Chunk::Task { status: TaskStatus::Complete, .. }, Chunk::Text(s)] if s == "done"),
            "{chunks:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn details_is_the_last_finished_first_line_and_sources_cap_at_three() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        run(
            &mut t,
            "c1",
            "a",
            "see https://a.com/x and https://b.com/y",
            false,
        )
        .await;
        run(
            &mut t,
            "c2",
            "b",
            "\n\nsecond first\nhttps://c.com/z https://d.com/w https://a.com/x",
            false,
        )
        .await;
        t.text("x").await;
        t.tick().await;
        let c = last_card(&api, "c1");
        assert_eq!(c.details.as_deref(), Some("second first"));
        let urls: Vec<(&str, &str)> = c
            .sources
            .iter()
            .map(|s| (s.url.as_str(), s.text.as_str()))
            .collect();
        assert_eq!(
            urls,
            [
                ("https://a.com/x", "a.com"),
                ("https://b.com/y", "b.com"),
                ("https://c.com/z", "c.com")
            ]
        );
    }

    /// 110 §4.3: `<parent>.<n>` calls fold into the parent's line, with the
    /// inner names `×n` and a failed inner call's `✗` on the parent.
    #[tokio::test(start_paused = true)]
    async fn python_inner_calls_fold_into_the_parent_line() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        run(&mut t, "c1", "ls", "ok", false).await;
        t.tool_started("p1", "python", &serde_json::json!({"code": "x = 1"}))
            .await;
        t.tool_started("p1.1", "read_file", &serde_json::json!({"path": "a"}))
            .await;
        t.tool_started("p1.2", "read_file", &serde_json::json!({"path": "b"}))
            .await;
        t.tool_started("p1.3", "knowledge_get", &serde_json::json!({"id": "k"}))
            .await;
        t.tool_finished("p1.2", "ENOENT", true).await;
        t.tool_finished("p1.1", "a", false).await;
        t.tool_finished("p1.3", "k", false).await;
        t.tool_finished("p1", "done", false).await;
        t.text("x").await;
        t.tick().await;
        assert!(ids(&api).iter().all(|id| id == "c1"), "{:?}", ids(&api));
        let c = last_card(&api, "c1");
        assert_eq!(c.title, "2 tools · 0.0 s · 1 failed (python)");
        assert_eq!(
            c.output.as_deref().unwrap().lines().nth(1),
            Some("✗ python · read_file ×2, knowledge_get · 0.0 s · done")
        );
        assert_eq!(t.tool_count(), 2, "inner calls do not count");
    }

    #[tokio::test(start_paused = true)]
    async fn list_caps_at_sixty_items_with_a_count() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        for i in 0..65 {
            run(&mut t, &format!("c{i}"), "x", "ok", false).await;
        }
        t.text("x").await;
        t.tick().await;
        let c = last_card(&api, "c0");
        assert_eq!(c.title, "65 tools · 0.0 s");
        let lines: Vec<&str> = c.output.as_deref().unwrap().lines().collect();
        assert_eq!(lines.len(), 61);
        assert_eq!(lines[60], "… 5 more");
    }

    /// 110 §4.6: no stream → one thread message per batch, edited in place.
    #[tokio::test(start_paused = true)]
    async fn update_mode_posts_one_message_per_batch_and_edits_it() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        api.fail("start", "channel_type_not_supported");
        t.tool_started("c1", "shell", &sh("ls")).await;
        t.tick().await;
        let calls = api.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(matches!(&calls[0], Call::Post { md } if md == "…"));
        assert!(
            matches!(&calls[1], Call::Post { md } if md == "**shell · ls**\n```\n⠹ shell · ls · running\n```"),
            "{calls:?}"
        );
        t.tool_started("c2", "read_file", &serde_json::json!({"path": "a"}))
            .await;
        tokio::time::advance(EDIT_FLOOR).await;
        t.tick().await;
        let calls = api.calls();
        assert!(
            matches!(&calls[2], Call::Update { ts, md } if ts == "ts1" && md.starts_with("**running `shell` · 1 of 2**\n```\n⠹ shell")),
            "{calls:?}"
        );
        t.tool_finished("c1", "a", false).await;
        t.tool_finished("c2", "b\nhttps://x.org/p", false).await;
        t.text("done").await;
        t.finish(footer()).await;
        let calls = api.calls();
        let Call::Update { ts, md } = &calls[3] else {
            panic!("{calls:?}")
        };
        assert_eq!(ts, "ts1");
        assert_eq!(
            md,
            "**2 tools · 1.5 s**\n```\n✓ shell · ls · 1.5 s · a\n✓ read_file · path: a · 1.5 s · b\n```\n[x.org](https://x.org/p)"
        );
        assert!(
            matches!(&calls[4], Call::Update { ts, md } if ts == "ts0" && md.starts_with("done\n")),
            "{calls:?}"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|c| matches!(c, Call::Post { .. }))
                .count(),
            2,
            "bubble + one card message"
        );
    }

    /// 110 §4.6: a rotated bubble re-emits the live batch card, not the
    /// finished ones.
    #[tokio::test(start_paused = true)]
    async fn rotation_reemits_only_the_live_batch_card() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text("para\n\n").await;
        run(&mut t, "c1", "a", "1", false).await;
        run(&mut t, "c2", "b", "2", false).await;
        t.text("mid\n\n").await;
        t.tool_started("c3", "shell", &sh("sleep")).await;
        t.tick().await;
        assert_eq!(api.starts().len(), 1);
        tokio::time::advance(BUBBLE_MAX_AGE).await;
        t.tick().await;
        let starts = api.starts();
        assert_eq!(starts.len(), 2, "{:?}", api.calls());
        let reemitted = cards(&starts[1]);
        assert_eq!(reemitted.len(), 1, "{reemitted:?}");
        assert_eq!(reemitted[0].id, "c3");
        assert_eq!(reemitted[0].status, TaskStatus::InProgress);
        assert_eq!(t.open_stream(), Some("ts1"));
    }

    #[tokio::test(start_paused = true)]
    async fn tools_errors_emits_the_card_only_after_a_failure() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::Errors);
        run(&mut t, "c1", "ls", "fine", false).await;
        t.tick().await;
        assert!(api.calls().is_empty(), "{:?}", api.calls());
        run(&mut t, "c2", "rm", "boom", true).await;
        t.tick().await;
        let cards = cards(&api.task_chunks());
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert_eq!(cards[0].id, "c1");
        assert_eq!(cards[0].status, TaskStatus::Error);
        assert_eq!(
            cards[0].output.as_deref(),
            Some("✓ shell · ls · 0.0 s · fine\n✗ shell · rm · 0.0 s · boom"),
            "every item, so the failure has its context"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reasoning_off_drops_thought_lines() {
        let api = FakeApi::new();
        let mut t = turn_with(&api, ToolCards::All, false, true);
        run(&mut t, "c1", "a", "1", false).await;
        t.reasoning("hidden").await;
        run(&mut t, "c2", "b", "2", false).await;
        t.reasoning("also hidden").await;
        t.text("x").await;
        t.tick().await;
        assert_eq!(ids(&api), ["c1"]);
        assert!(!last_card(&api, "c1").output.unwrap().contains("💭"));
    }

    /// 110 §4.2 ruling 2: no snippet upload; the card keeps the URLs.
    #[tokio::test(start_paused = true)]
    async fn no_snippet_upload_for_long_outputs() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "read", &serde_json::json!({"path": "x"}))
            .await;
        let output = "see https://example.com/a\n".repeat(200);
        t.tool_finished("c1", &output, false).await;
        t.tick().await;
        assert!(!api.calls().iter().any(|c| matches!(c, Call::Upload)));
        let c = last_card(&api, "c1");
        assert_eq!(c.details.as_deref(), Some("see https://example.com/a"));
        assert_eq!(c.sources.len(), 1);
        assert_eq!(c.sources[0].url, "https://example.com/a");
        assert_eq!(c.sources[0].text, "example.com");
    }

    /// 110 §6: `Compacted` is a footer item.
    #[tokio::test(start_paused = true)]
    async fn compacted_epoch_lands_on_the_footer() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text("hi").await;
        t.compacted(3);
        t.finish(footer()).await;
        let calls = api.calls();
        let Call::Stop { footer: foot, .. } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        assert_eq!(
            foot.as_deref(),
            Some("m · 12.3k in / 800 out · 42 s · 3 tools · ⟲ epoch 3")
        );
    }

    /// 110 §4.1: reasoning before any call is its own card, `thinking`
    /// while live and `Thought N s` once text ends it.
    #[tokio::test(start_paused = true)]
    async fn leading_reasoning_is_a_growing_card_retitled_when_text_follows() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.reasoning("first idea\n").await;
        t.reasoning("second").await;
        t.tick().await;
        let starts = api.starts();
        assert_eq!(starts.len(), 1);
        assert_eq!(
            task_of(&starts[0], "reasoning-1"),
            vec![(
                TaskStatus::InProgress,
                None,
                Some("first idea\nsecond".into())
            )],
            "{:?}",
            starts[0]
        );
        t.reasoning(" thought").await;
        t.tick().await;
        t.reasoning_done(3_000);
        t.text("answer").await;
        t.tick().await;
        let calls = api.calls();
        let Call::Append { chunks, .. } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        // Slack appends `output`: the growth went out on the last tick, the
        // retitle carries nothing new.
        assert_eq!(
            chunks,
            &vec![
                Chunk::Task {
                    id: "reasoning-1".into(),
                    title: "Thought 3 s".into(),
                    status: TaskStatus::Complete,
                    details: None,
                    output: None,
                    sources: Vec::new(),
                },
                Chunk::Text("answer".into()),
            ]
        );
        assert_eq!(
            last_card(&api, "reasoning-1").output.as_deref(),
            Some("first idea\nsecond thought")
        );
        // Reasoning that a call ends is a card too; its time is measured
        // here when no `ReasoningDone` arrives.
        t.reasoning("again").await;
        tokio::time::advance(Duration::from_secs(2)).await;
        t.tool_started("c1", "shell", &sh("ls")).await;
        t.tick().await;
        let c = last_card(&api, "reasoning-2");
        assert_eq!(c.title, "Thought 2 s");
        assert_eq!(c.status, TaskStatus::Complete);
        assert_eq!(t.tool_count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tool_cards_none_emits_no_tasks() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::None);
        t.tool_pending("c1", "shell").await;
        t.tool_started("c1", "shell", &sh("ls")).await;
        t.tool_finished("c1", "boom", true).await;
        t.text("x").await;
        t.tick().await;
        assert!(api.task_chunks().is_empty());
        assert_eq!(t.tool_count(), 1);
    }

    #[test]
    fn describe_python_lists_inner_tools_with_counts() {
        let code = "a = await knowledge_get('x')\nfor f in fs:\n    b = await read_file(f)\nawait read_file('z')\nawait asyncio.sleep(1)\n";
        assert_eq!(
            describe("python", &serde_json::json!({"code": code})),
            "knowledge_get, read_file ×2"
        );
        assert_eq!(
            describe(
                "python",
                &serde_json::json!({"code": "# count the sessions\nprint(1)"})
            ),
            "count the sessions"
        );
        assert_eq!(
            describe("python", &serde_json::json!({"code": "x = 1\ny = 2\n"})),
            "<2 lines>"
        );
    }

    #[test]
    fn describe_shapes() {
        assert_eq!(
            describe("shell", &serde_json::json!({"command": " ls -la "})),
            "ls -la"
        );
        let long = "y".repeat(200);
        let d = describe(
            "edit",
            &serde_json::json!({"path": "a.rs", "content": long, "n": 3}),
        );
        assert_eq!(d, "path: a.rs\ncontent: <200 chars>\nn: 3");
        assert_eq!(describe("x", &serde_json::Value::Null), "");
        assert!(is_deliverable("show", &serde_json::Value::Null));
        assert!(is_deliverable("routine_draft", &serde_json::Value::Null));
        assert!(is_deliverable(
            "doc_create",
            &serde_json::json!({"kind": "board"})
        ));
        assert!(!is_deliverable(
            "doc_create",
            &serde_json::json!({"kind": "note"})
        ));
        assert!(!is_deliverable("shell", &serde_json::Value::Null));
        assert_eq!(truncate_chars("héllo", 3), "hé…");
        assert_eq!(fmt_k(999), "999");
        assert_eq!(fmt_k(16_500), "16.5k");
        assert_eq!(fmt_k(1_000_000), "1.0M");
        let mut f = footer();
        assert!(!f.to_string().contains("ctx"));
        f.context = Some((16_500, 1_000_000));
        assert!(
            f.to_string().ends_with(" · 3 tools · ctx 16.5k/1.0M"),
            "{f}"
        );
        assert_eq!(fmt_elapsed(Duration::from_millis(1200)), "1.2 s");
        assert_eq!(fmt_elapsed(Duration::from_secs(92)), "1m32s");
        assert_eq!(fmt_thought(999), None);
        assert_eq!(fmt_thought(9_400).as_deref(), Some("9 s"));
        assert_eq!(fmt_thought(72_000).as_deref(), Some("1m 12s"));
        assert_eq!(fmt_thought(120_000).as_deref(), Some("2m"));
        assert_eq!(fmt_thought(3_720_000).as_deref(), Some("1h 2m"));
        assert_eq!(thought_title(500), "Thought");
    }
}
