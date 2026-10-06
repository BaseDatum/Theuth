//! One streamed message per turn (design doc 21 §8.1–8.4; cards 110 §4): a
//! [`Turn`] owns the current bubble (`chat.startStream` or, when a stream
//! cannot be opened, a `chat.postMessage` edited in place), one task card
//! per tool batch, the trailing reasoning card, rotation at block boundaries
//! and repair of dead streams. `markdown::Text::raw()` is the only truth
//! about the text; a bubble that fails is rewritten from it.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use runtime::{Runtime, SessionId};
use tokio::time::Instant;

use crate::api::{ApiError, Chunk, MARKDOWN_TEXT_MAX, SlackApi, Source, TaskStatus};
use crate::{SlackEvent, ToolCards};

/// Chars of text per bubble before rotating at a block boundary.
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
/// Column width `json-render` fences are rendered at for Slack (30 §9).
const FLATTEN_WIDTH: usize = 60;

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
/// `appendStream`/`stopStream` errors that mean "this message is no longer
/// a stream": repair it with `chat.update` and continue in a new one (21 §8.4).
const DEAD_CODES: [&str; 4] = [
    "message_not_in_streaming_state",
    "streaming_mode_mismatch",
    "msg_too_long",
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
    opened: Instant,
    last_append: Instant,
}

impl Bubble {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            mode: Mode::Unsent,
            text: markdown::Text::new(),
            sent: 0,
            opened: now,
            last_append: now,
        }
    }

    /// Chars of committed text (Slack counts chars, so the caps do too).
    fn committed_chars(&self) -> usize {
        self.text.raw()[..self.text.committed()].chars().count()
    }

    /// `raw[range]` as it goes on the wire: `json-render` fences rendered
    /// inside a plain code block (30 §9); everything else verbatim.
    fn out(&self, range: std::ops::Range<usize>) -> String {
        surface::flatten(&self.text.raw()[range], FLATTEN_WIDTH)
    }

    /// Stream mode: bytes of `raw` that may be appended now. A `json-render`
    /// fence still open in the pending tail is held back until it closes, so
    /// `out` sees it whole instead of its source having already streamed.
    fn streamable(&self) -> usize {
        let raw = self.text.raw();
        let committed = self.text.committed();
        let first = raw[committed..]
            .trim_start()
            .lines()
            .next()
            .unwrap_or_default();
        if first.trim_start_matches(['`', '~']).trim() == "json-render" {
            committed
        } else {
            raw.len()
        }
    }
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
        match first_line(&self.text) {
            Some(l) => format!("{head} — {}", truncate_chars(&l, THOUGHT_MAX)),
            None => head,
        }
    }
}

enum Item {
    Call(Call),
    Thought(Thought),
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
        let mut parts = vec![format!(
            "{} tool{}",
            calls.len(),
            if calls.len() == 1 { "" } else { "s" }
        )];
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
        let mut out: Vec<String> = self
            .items
            .iter()
            .take(ITEMS_MAX)
            .map(|i| {
                truncate_chars(
                    &match i {
                        Item::Call(c) => c.line(),
                        Item::Thought(t) => t.line(),
                    },
                    LINE_MAX,
                )
            })
            .collect();
        if self.items.len() > ITEMS_MAX {
            out.push(format!("… {} more", self.items.len() - ITEMS_MAX));
        }
        out
    }

    /// The stream-mode card (110 §4.2).
    fn chunk(&self) -> Chunk {
        Chunk::Task {
            id: self.id.clone(),
            title: self.title(),
            status: self.status(),
            details: self.details(),
            output: self.plain().is_none().then(|| self.lines().join("\n")),
            sources: self.sources.clone(),
        }
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
    /// A batch card by id, rendered when its floor allows (110 §4.4).
    Card(String),
    /// A reasoning card; `card` is its update-mode text.
    Chunk { chunk: Chunk, card: String },
}

/// Stream-mode trailing reasoning run: the `reasoning-<n>` card (21 §8.5,
/// 110 §4.1).
struct ReasoningRun {
    id: String,
    text: markdown::Text,
    /// Bytes of `text.raw()` sent as the card's `output`.
    sent: usize,
    started: Instant,
    /// `ReasoningDone.ms`.
    ms: Option<u64>,
}

impl ReasoningRun {
    fn chunk(&self) -> Chunk {
        Chunk::Task {
            id: self.id.clone(),
            title: "thinking".to_owned(),
            status: TaskStatus::InProgress,
            details: None,
            output: Some(truncate_chars(self.text.raw(), MARKDOWN_TEXT_MAX)),
            sources: Vec::new(),
        }
    }
}

/// One turn's streamed output in a thread (21 §8.1–8.4).
pub struct Turn {
    target: Target,
    tools_mode: ToolCards,
    reasoning: bool,
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
        api: Arc<dyn SlackApi>,
        rt: Runtime,
        session: SessionId,
    ) -> Self {
        Self {
            target,
            tools_mode,
            reasoning,
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
        self.bubble.get_or_insert_with(Bubble::new).text.push(delta);
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
            let run = ReasoningRun {
                id: format!("reasoning-{}", self.reasoning_runs),
                text: markdown::Text::new(),
                sent: 0,
                started: Instant::now(),
                ms: None,
            };
            self.queue_chunk(run.chunk(), "_thinking…_".to_owned());
            self.reasoning_run = Some(run);
        }
        if let Some(run) = &mut self.reasoning_run {
            run.text.push(delta);
        }
    }

    /// `ReasoningDone { ms }` (110 §4.1): the time behind `Thought N s` or
    /// `💭 thought N s` for the run that is ending. Published before the
    /// event that ends the run, so it lands on the live one. Not wired
    /// today: `channel.rs` must call `t.reasoning_done(ms)` in its
    /// `ReasoningDone` arm.
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
    /// turn's footer. Not wired today: `channel.rs` must call
    /// `t.compacted(epoch)` (holding the epoch for the next turn when none
    /// is open).
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
        if !emits(self.tools_mode, b) {
            return;
        }
        let id = b.id.clone();
        self.bubble.get_or_insert_with(Bubble::new);
        if !self
            .queue
            .iter()
            .any(|q| matches!(q, Queued::Card(x) if *x == id))
        {
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
            let raw = run.text.raw();
            let ms = run.ms.unwrap_or_else(|| ms_of(run.started.elapsed()));
            self.queue_chunk(
                Chunk::Task {
                    id: run.id.clone(),
                    title: thought_title(ms),
                    status: TaskStatus::Complete,
                    details: first_line(raw),
                    output: Some(truncate_chars(raw, MARKDOWN_TEXT_MAX)),
                    sources: Vec::new(),
                },
                Segment::quote(raw),
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
        self.batches[i].closed = true;
        if let Some(Item::Thought(_)) = self.batches[i].items.last() {
            let Some(Item::Thought(t)) = self.batches[i].items.pop() else {
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
        self.queue_chunk(
            Chunk::Task {
                id: format!("reasoning-{}", self.reasoning_runs),
                title: thought_title(ms),
                status: TaskStatus::Complete,
                details: first_line(&t.text),
                output: Some(truncate_chars(&t.text, MARKDOWN_TEXT_MAX)),
                sources: Vec::new(),
            },
            Segment::quote(&t.text),
        );
    }

    /// Stream mode falling back to update mode with a reasoning card open:
    /// the card becomes the quoted message it would have been (21 §8.2).
    async fn demote_reasoning(&mut self) {
        let Some(run) = self.reasoning_run.take() else {
            return;
        };
        self.queue.retain(
            |q| !matches!(q, Queued::Chunk { chunk, .. } if chunk_id(chunk) == Some(&run.id)),
        );
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
        let chunk = run.chunk();
        self.queue_chunk(chunk, String::new());
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

    /// Queue a reasoning card; one already queued for the same id is
    /// replaced in place, so one flush carries one card per id.
    fn queue_chunk(&mut self, chunk: Chunk, card: String) {
        self.bubble.get_or_insert_with(Bubble::new);
        let id = chunk_id(&chunk).map(str::to_owned);
        let slot = self.queue.iter_mut().find_map(|q| match q {
            Queued::Chunk { chunk: c, card: k } if chunk_id(c).map(str::to_owned) == id => {
                Some((c, k))
            }
            _ => None,
        });
        match slot {
            Some((c, k)) => {
                *c = chunk;
                *k = card;
            }
            None => self.queue.push(Queued::Chunk { chunk, card }),
        }
    }

    fn batch(&self, id: &str) -> Option<&Batch> {
        self.batches.iter().find(|b| b.id == id)
    }

    /// Queued chunks that may go now, in queue order: reasoning cards, and
    /// batch cards that are due (`last` ignores the floor). The second
    /// value names the batches rendered, for [`Self::mark_sent`] once the
    /// call succeeds.
    fn collect(&self, last: bool) -> (Vec<Chunk>, Vec<String>) {
        let mut chunks = Vec::new();
        let mut sent = Vec::new();
        for q in &self.queue {
            match q {
                Queued::Chunk { chunk, .. } => chunks.push(chunk.clone()),
                Queued::Card(id) => {
                    if let Some(b) = self.batch(id)
                        && emits(self.tools_mode, b)
                        && b.due(last)
                    {
                        chunks.push(b.chunk());
                        sent.push(id.clone());
                    }
                }
            }
        }
        (chunks, sent)
    }

    /// The batches in `sent` went on the wire; cards not yet due stay queued.
    fn mark_sent(&mut self, sent: &[String]) {
        let now = Instant::now();
        for b in &mut self.batches {
            if sent.contains(&b.id) {
                b.dirty = false;
                b.last_sent = Some(now);
            }
        }
        self.queue
            .retain(|q| matches!(q, Queued::Card(id) if !sent.contains(id)));
    }

    /// The keepalive / rotation payload: the live batch card, else the live
    /// reasoning card (110 §4.4, §4.6).
    fn live_chunk(&self) -> Option<(Chunk, Option<String>)> {
        self.batches
            .iter()
            .find(|b| b.open() && emits(self.tools_mode, b))
            .map(|b| (b.chunk(), Some(b.id.clone())))
            .or_else(|| self.reasoning_run.as_ref().map(|r| (r.chunk(), None)))
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
            self.rotate(cut, false).await;
        }
        self.flush().await;
    }

    /// `TurnFinished`: final flush and close. A turn that opened nothing
    /// makes no call.
    pub async fn finish(&mut self, footer: Footer) {
        self.end_reasoning().await;
        self.close_batch();
        let Some(b) = self.bubble.as_mut() else {
            return;
        };
        b.text.finish();
        if let Some(cut) = self.rotation_cut() {
            self.rotate(cut, false).await;
        }
        if matches!(self.bubble.as_ref().map(|b| &b.mode), Some(Mode::Unsent)) {
            if !self.has_work() {
                self.bubble = None;
                return;
            }
            self.open().await;
        }
        let Some(b) = self.bubble.take() else { return };
        let mut footer = footer.to_string();
        if let Some(epoch) = self.compacted {
            footer.push_str(&format!(" · ⟲ epoch {epoch}"));
        }
        let raw = b.out(0..b.text.raw().len());
        match &b.mode {
            Mode::Unsent => {
                tracing::warn!("turn finished before its bubble could be opened; text dropped");
            }
            Mode::Stream(ts) => {
                // Cards first: they completed before the text that follows them.
                let (mut chunks, _) = self.collect(true);
                self.queue.clear();
                if b.sent < b.text.raw().len() {
                    chunks.push(Chunk::Text(b.out(b.sent..b.text.raw().len())));
                }
                let stopped = self
                    .api
                    .stop_stream(
                        &self.target.channel,
                        ts,
                        chunks,
                        Some(footer.clone()),
                        Some("active"),
                    )
                    .await;
                if let Err(e) = stopped {
                    tracing::warn!(error = %e, ts, "stopStream failed; repairing");
                    self.publish_fallback(e.to_string()).await;
                    self.repair(ts, &format!("{raw}\n\n_{footer}_")).await;
                }
            }
            Mode::Update(ts) => {
                self.flush_cards(true).await;
                let md = format!("{raw}\n\n_{footer}_");
                if let Err(e) = self
                    .api
                    .update_markdown(&self.target.channel, ts, md.trim_start())
                    .await
                {
                    tracing::warn!(error = %e, ts, "final update failed");
                }
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
            Mode::Unsent => b.streamable() > 0,
            Mode::Stream(_) => b.sent < b.streamable(),
            Mode::Update(_) => b.sent < b.text.committed(),
        }
    }

    /// Where to cut the open bubble if a rotation trigger fired (21 §8.4:
    /// size at a block boundary; age for streams).
    fn rotation_cut(&self) -> Option<usize> {
        let b = self.bubble.as_ref()?;
        let aged = matches!(b.mode, Mode::Stream(_)) && b.opened.elapsed() >= BUBBLE_MAX_AGE;
        if matches!(b.mode, Mode::Unsent) || !(aged || b.committed_chars() >= BUBBLE_SOFT_MAX) {
            return None;
        }
        let raw = b.text.raw();
        let committed = b.text.committed();
        if raw[..committed].chars().count() <= MARKDOWN_TEXT_MAX {
            return Some(committed);
        }
        // ponytail: one block over the per-call cap (a giant fence): cut at
        // its last newline under the cap; the rest renders raw next message.
        let cap = raw
            .char_indices()
            .nth(MARKDOWN_TEXT_MAX)
            .map_or(committed, |(i, _)| i);
        Some(raw[..cap].rfind('\n').map_or(cap, |i| i + 1))
    }

    /// Close the open bubble at `cut` (a block boundary, or `raw.len()` when
    /// the stream is dead) and start a fresh one seeded with the remainder,
    /// the live batch cards and the live reasoning card (21 §8.4, 110 §4.6).
    async fn rotate(&mut self, cut: usize, mut dead: bool) {
        let Some(b) = self.bubble.take() else { return };
        let raw = b.text.raw();
        let t = &self.target;
        match &b.mode {
            Mode::Unsent => {}
            Mode::Stream(ts) => {
                if !dead && b.sent < cut {
                    match self
                        .api
                        .append_stream(&t.channel, ts, vec![Chunk::Text(b.out(b.sent..cut))])
                        .await
                    {
                        Ok(()) => {}
                        Err(e) if is_dead(&e) => dead = true,
                        Err(e) => {
                            tracing::warn!(error = %e, ts, "final append before rotation failed")
                        }
                    }
                }
                if !dead {
                    match self
                        .api
                        .stop_stream(&t.channel, ts, Vec::new(), None, None)
                        .await
                    {
                        Ok(()) => {}
                        Err(e) if is_dead(&e) => dead = true,
                        Err(e) => tracing::warn!(error = %e, ts, "stopStream on rotation failed"),
                    }
                }
                if dead {
                    self.publish_fallback("stream died".to_owned()).await;
                }
                // Streaming shows raw deltas past the block boundary; trim them
                // so the text does not appear in two messages.
                if dead || b.sent > cut {
                    self.repair(ts, &b.out(0..cut)).await;
                }
            }
            Mode::Update(ts) => {
                if b.sent != cut {
                    self.repair(ts, &b.out(0..cut)).await;
                }
            }
        }
        let mut next = Bubble::new();
        next.text.push(&raw[cut..]);
        let mut reemit = Vec::new();
        for b in &mut self.batches {
            if b.open() && emits(self.tools_mode, b) {
                b.last_sent = None;
                b.dirty = true;
                if !self
                    .queue
                    .iter()
                    .any(|q| matches!(q, Queued::Card(id) if *id == b.id))
                {
                    reemit.push(Queued::Card(b.id.clone()));
                }
            }
        }
        if let Some(run) = &self.reasoning_run
            && !self.queue.iter().any(
                |q| matches!(q, Queued::Chunk { chunk, .. } if chunk_id(chunk) == Some(&run.id)),
            )
        {
            reemit.push(Queued::Chunk {
                chunk: run.chunk(),
                card: String::new(),
            });
        }
        self.queue.splice(0..0, reemit);
        self.bubble = Some(next);
    }

    /// `chat.update` a stopped or dead stream with its full text; one retry.
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

    async fn flush(&mut self) {
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
        let body = if committed == 0 {
            "…".to_owned()
        } else {
            b.out(0..committed)
        };
        let t = &self.target;
        match self
            .api
            .post_markdown(&t.channel, Some(&t.thread_ts), &body)
            .await
        {
            Ok(ts) => {
                b.mode = Mode::Update(ts);
                b.sent = committed;
                b.opened = Instant::now();
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
        let end = b.streamable();
        let (mut chunks, sent) = self.collect(false);
        if end > 0 {
            chunks.push(Chunk::Text(b.out(0..end)));
        }
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
        let end = b.streamable();
        let (mut chunks, mut sent) = self.collect(false);
        if b.sent < end {
            chunks.push(Chunk::Text(b.out(b.sent..end)));
        }
        if chunks.is_empty() {
            if b.last_append.elapsed() < KEEPALIVE {
                return;
            }
            let Some((live, id)) = self.live_chunk() else {
                return;
            };
            chunks.push(live);
            sent.extend(id);
        }
        match self
            .api
            .append_stream(&self.target.channel, ts, chunks)
            .await
        {
            Ok(()) => {
                let b = self.bubble.as_mut().expect("bubble present");
                b.sent = end;
                b.last_append = Instant::now();
                self.mark_sent(&sent);
            }
            Err(e) if is_dead(&e) => {
                tracing::warn!(error = %e, ts, "stream died; repairing");
                let len = b.text.raw().len();
                self.rotate(len, true).await;
            }
            Err(e) => tracing::warn!(error = %e, ts, "appendStream failed; will retry"),
        }
    }

    /// Update-mode flush (21 §8.2): committed text edited in place, then
    /// the cards.
    async fn flush_update(&mut self) {
        let Some(b) = self.bubble.as_mut() else {
            return;
        };
        let Mode::Update(ts) = &b.mode else { return };
        let committed = b.text.committed();
        if b.sent < committed {
            match self
                .api
                .update_markdown(&self.target.channel, ts, &b.out(0..committed))
                .await
            {
                Ok(()) => b.sent = committed,
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

fn chunk_id(c: &Chunk) -> Option<&str> {
    match c {
        Chunk::Task { id, .. } => Some(id),
        Chunk::Text(_) => None,
    }
}

fn is_dead(e: &ApiError) -> bool {
    match e {
        ApiError::RateLimited(_) => true,
        ApiError::Slack(code) => DEAD_CODES.contains(&code.as_str()),
        ApiError::Transport(_) => false,
    }
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

    /// Final update with everything.
    async fn finish(&mut self) {
        self.text.finish();
        self.tick().await;
    }

    fn body(&self, upto: usize) -> String {
        let raw = &self.text.raw()[..upto];
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
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Mutex, PoisonError};

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
    }

    #[derive(Default)]
    struct FakeApi {
        calls: Mutex<Vec<Call>>,
        fail_next: Mutex<HashMap<&'static str, ApiError>>,
        seq: AtomicU32,
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
            self.check("append")?;
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
            self.check("stop")?;
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
            Ok(self.next_ts())
        }
        async fn update_markdown(
            &self,
            _channel: &str,
            ts: &str,
            markdown: &str,
        ) -> Result<(), ApiError> {
            self.check("update")?;
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
            _blocks: serde_json::Value,
        ) -> Result<String, ApiError> {
            unimplemented!()
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
        ) -> Result<(), ApiError> {
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
            _thread_ts: Option<&str>,
            _files: &[crate::api::UploadFile],
            _comment: Option<&str>,
        ) -> Result<String, ApiError> {
            unimplemented!()
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
        turn_with(api, mode, true)
    }

    fn turn_with(api: &Arc<FakeApi>, mode: ToolCards, reasoning: bool) -> Turn {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let target = Target {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            user: "U1".into(),
            team: "T1".into(),
        };
        Turn::new(target, mode, reasoning, api.clone(), rt, SessionId::new())
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

    /// 30 §9: a `json-render` fence reaches Slack as a code block of its
    /// rendering, not its JSON; while open it is held back, not streamed raw.
    #[tokio::test(start_paused = true)]
    async fn json_render_fence_streams_flattened() {
        let spec = r#"{"root":"c","elements":{
  "c":{"type":"Card","props":{"title":"Rollout"},"children":["l"]},
  "l":{"type":"Checklist","props":{"items":[{"text":"schema migration","done":true}]}}}}"#;
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text("Status:\n\n```json-render\n").await;
        t.text(spec).await;
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
        let sent: String = api
            .calls()
            .into_iter()
            .flat_map(|c| match c {
                Call::Start { chunks, .. }
                | Call::Append { chunks, .. }
                | Call::Stop { chunks, .. } => chunks,
                _ => Vec::new(),
            })
            .filter_map(|c| match c {
                Chunk::Text(s) => Some(s),
                Chunk::Task { .. } => None,
            })
            .collect();
        assert!(
            sent.trim_start_matches("Status:")
                .trim_start()
                .starts_with("```\n╭"),
            "{sent}"
        );
        assert!(sent.contains("[x] schema migration"), "{sent}");
        assert!(!sent.contains("\"type\""), "{sent}");
        assert!(sent.ends_with("```\n\nDone."), "{sent}");
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

    #[tokio::test(start_paused = true)]
    async fn dead_stream_is_repaired_and_reopened() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.text("para one\n\n").await;
        t.tick().await;
        t.tool_started("c1", "shell", &serde_json::json!({"command": "ls"}))
            .await;
        t.text("more").await;
        api.fail("append", "message_not_in_streaming_state");
        t.tick().await;
        let calls = api.calls();
        let Call::Update { ts, md } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        assert_eq!(ts, "ts0");
        assert_eq!(md, "para one\n\nmore");
        assert_eq!(t.open_stream(), None);
        t.tick().await;
        let starts = api.starts();
        assert_eq!(starts.len(), 2);
        assert!(
            starts[1].iter().any(|c| matches!(c, Chunk::Task { id, status: TaskStatus::InProgress, .. } if id == "c1")),
            "{:?}",
            starts[1]
        );
        assert_eq!(t.open_stream(), Some("ts1"));
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
                Call::Upload => panic!(),
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
                Chunk::Task { .. } => None,
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
        let mut t = turn_with(&api, ToolCards::All, false);
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

    #[derive(Debug)]
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
                Chunk::Text(_) => None,
            })
            .collect()
    }

    /// The last card sent for `id`.
    fn last_card(api: &FakeApi, id: &str) -> Card {
        cards(&api.task_chunks())
            .into_iter()
            .rfind(|c| c.id == id)
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
        assert_eq!(
            early.output.as_deref(),
            Some("⏳ shell\n⏳ doc_create\n⏳ shell")
        );
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
        assert!(
            live.output
                .as_deref()
                .unwrap()
                .ends_with("\n💭 thinking — Let me check"),
            "{live:?}"
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
        assert!(
            !batch.output.as_deref().unwrap().contains("💭"),
            "{batch:?}"
        );
        assert_eq!(thought.id, "reasoning-1");
        assert_eq!(thought.title, "Thought 12 s");
        assert_eq!(thought.status, TaskStatus::Complete);
        assert_eq!(thought.details.as_deref(), Some("Let me check"));
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
            Some(
                "✓ shell · ls · 0.0 s · a\n⠹ python · read_file · running\n⠹ read_file · path: y · running"
            )
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
        let mut t = turn_with(&api, ToolCards::All, false);
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
        assert_eq!(
            chunks,
            &vec![
                Chunk::Task {
                    id: "reasoning-1".into(),
                    title: "Thought 3 s".into(),
                    status: TaskStatus::Complete,
                    details: Some("first idea".into()),
                    output: Some("first idea\nsecond thought".into()),
                    sources: Vec::new(),
                },
                Chunk::Text("answer".into()),
            ]
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
