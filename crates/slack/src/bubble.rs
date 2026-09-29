//! One streamed message per turn (design doc 21 §8.1–8.5): a [`Turn`] owns
//! the current bubble (`chat.startStream` or, when a stream cannot be opened,
//! a `chat.postMessage` edited in place), the tool task cards, the reasoning
//! card, rotation at block boundaries and repair of dead streams.
//! `markdown::Text::raw()` is the only truth about the text; a bubble that
//! fails is rewritten from it.

use std::collections::HashMap;
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
/// Re-send an `in_progress` task if nothing was appended for this long.
pub const KEEPALIVE: Duration = Duration::from_secs(25);
/// Worker flush cadence.
pub const TICK: Duration = Duration::from_millis(750);
/// Tool output longer than this (chars) is uploaded whole and linked.
pub const TOOL_OUTPUT_CHARS: usize = 2_500;
/// Task card title cap (chars).
pub const TITLE_MAX: usize = 256;
/// Task card details cap (chars).
pub const DETAILS_MAX: usize = 256;
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

/// One timeline card of the turn: a tool call, or a reasoning run.
struct Task {
    name: String,
    title: String,
    describe: String,
    status: TaskStatus,
    started: Instant,
    /// Update-mode card message (21 §8.2).
    card_ts: Option<String>,
    /// Last chunk sent for it; re-sent as keepalive and on rotation.
    chunk: Option<Chunk>,
}

/// A task chunk waiting for the next flush; `card` is its update-mode text.
struct Queued {
    call_id: String,
    chunk: Chunk,
    card: String,
}

/// Stream-mode reasoning run: the `reasoning-<n>` card (21 §8.5).
struct ReasoningRun {
    id: String,
    text: markdown::Text,
    /// Bytes of `text.raw()` sent as the card's `output`.
    sent: usize,
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
    tasks: HashMap<String, Task>,
    tools: u32,
    /// Reasoning runs started so far; the next one is `reasoning-<runs + 1>`.
    reasoning_runs: u32,
    /// Open stream-mode reasoning card.
    reasoning_run: Option<ReasoningRun>,
    /// Open update-mode quoted reasoning message.
    reasoning_segment: Option<Segment>,
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
            tasks: HashMap::new(),
            tools: 0,
            reasoning_runs: 0,
            reasoning_run: None,
            reasoning_segment: None,
            stream_unsupported: false,
            open_failures: 0,
            fallback_published: false,
        }
    }

    /// `TextDelta`: buffered; the next flush opens a bubble if none is open.
    pub async fn text(&mut self, delta: &str) {
        self.end_reasoning().await;
        self.bubble.get_or_insert_with(Bubble::new).text.push(delta);
    }

    /// `ReasoningDelta` (21 §8.5). Stream mode: a `reasoning-<n>` timeline
    /// card whose `output` grows each tick; update mode: a quoted message
    /// posted before the bubble. A run ends at the next non-reasoning event.
    pub async fn reasoning(&mut self, delta: &str) {
        if !self.reasoning {
            return;
        }
        if self.stream_unsupported {
            self.segment().push(delta).await;
            return;
        }
        if self.reasoning_run.is_none() {
            self.reasoning_runs += 1;
            let id = format!("reasoning-{}", self.reasoning_runs);
            self.tasks.insert(
                id.clone(),
                Task {
                    name: "thinking".to_owned(),
                    title: "thinking".to_owned(),
                    describe: String::new(),
                    status: TaskStatus::InProgress,
                    started: Instant::now(),
                    card_ts: None,
                    chunk: None,
                },
            );
            self.queue_task(
                &id,
                TaskStatus::InProgress,
                None,
                None,
                Vec::new(),
                "_thinking…_".to_owned(),
            );
            self.reasoning_run = Some(ReasoningRun {
                id,
                text: markdown::Text::new(),
                sent: 0,
            });
        }
        if let Some(run) = &mut self.reasoning_run {
            run.text.push(delta);
        }
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

    /// A non-reasoning event: complete the open reasoning run, if any.
    async fn end_reasoning(&mut self) {
        if let Some(mut s) = self.reasoning_segment.take() {
            s.finish().await;
        }
        let Some(run) = self.reasoning_run.take() else {
            return;
        };
        let raw = run.text.raw();
        if let Some(t) = self.tasks.get_mut(&run.id) {
            t.status = TaskStatus::Complete;
        }
        self.queue_task(
            &run.id,
            TaskStatus::Complete,
            first_line(raw),
            Some(truncate_chars(raw, MARKDOWN_TEXT_MAX)),
            Vec::new(),
            Segment::quote(raw),
        );
    }

    /// Stream mode falling back to update mode with a reasoning card open:
    /// the card becomes the quoted message it would have been (21 §8.2).
    async fn demote_reasoning(&mut self) {
        let Some(run) = self.reasoning_run.take() else {
            return;
        };
        self.tasks.remove(&run.id);
        self.queue.retain(|q| q.call_id != run.id);
        self.segment().text = run.text;
        self.segment().tick().await;
    }

    /// Re-send the open reasoning card when its text grew since the last tick.
    fn tick_reasoning(&mut self) {
        let Some(run) = &mut self.reasoning_run else {
            return;
        };
        let raw = run.text.raw();
        if raw.len() <= run.sent {
            return;
        }
        run.sent = raw.len();
        let id = run.id.clone();
        let output = truncate_chars(raw, MARKDOWN_TEXT_MAX);
        self.queue_task(
            &id,
            TaskStatus::InProgress,
            None,
            Some(output),
            Vec::new(),
            String::new(),
        );
    }

    /// `ToolCallPending`.
    pub async fn tool_pending(&mut self, call_id: &str, name: &str) {
        self.end_reasoning().await;
        self.tasks.insert(
            call_id.to_owned(),
            Task {
                name: name.to_owned(),
                title: name.to_owned(),
                describe: String::new(),
                status: TaskStatus::Pending,
                started: Instant::now(),
                card_ts: None,
                chunk: None,
            },
        );
        if self.tools_mode == ToolCards::All {
            self.queue_task(
                call_id,
                TaskStatus::Pending,
                None,
                None,
                Vec::new(),
                String::new(),
            );
        }
    }

    /// `ToolCallStarted`.
    pub async fn tool_started(&mut self, call_id: &str, name: &str, args: &serde_json::Value) {
        self.end_reasoning().await;
        self.tools += 1;
        let describe = describe(name, args);
        let title = truncate_chars(
            &format!(
                "{name} · {}",
                describe.split_whitespace().collect::<Vec<_>>().join(" ")
            ),
            TITLE_MAX,
        );
        let card = format!("⚙ **{name}**  running…\n```\n{describe}\n```");
        let task = self
            .tasks
            .entry(call_id.to_owned())
            .or_insert_with(|| Task {
                name: name.to_owned(),
                title: String::new(),
                describe: String::new(),
                status: TaskStatus::Pending,
                started: Instant::now(),
                card_ts: None,
                chunk: None,
            });
        task.title = title;
        task.describe = describe;
        task.started = Instant::now();
        task.status = TaskStatus::InProgress;
        if self.tools_mode == ToolCards::All {
            self.queue_task(
                call_id,
                TaskStatus::InProgress,
                None,
                None,
                Vec::new(),
                card,
            );
        }
    }

    /// `ToolCallFinished`: card details are the first output line; sources
    /// are up to 3 URLs from the output plus the uploaded full output when
    /// it is long (21 §8.3).
    pub async fn tool_finished(&mut self, call_id: &str, output: &str, is_error: bool) {
        self.end_reasoning().await;
        let status = if is_error {
            TaskStatus::Error
        } else {
            TaskStatus::Complete
        };
        let emit = match self.tools_mode {
            ToolCards::All => true,
            ToolCards::Errors => is_error,
            ToolCards::None => false,
        };
        let Some(task) = self.tasks.get_mut(call_id) else {
            tracing::debug!(call_id, "tool finished without a pending card");
            return;
        };
        task.status = status.clone();
        if !emit {
            return;
        }
        let name = task.name.clone();
        let elapsed = task.started.elapsed();
        let mut sources = Vec::new();
        for r in markdown::urls(output) {
            let url = &output[r];
            if sources.len() == 3 {
                break;
            }
            if sources.iter().any(|s: &Source| s.url == url) {
                continue;
            }
            sources.push(Source {
                url: url.to_owned(),
                text: host(url).to_owned(),
            });
        }
        let (shown, more) = head_chars(output.trim_end(), TOOL_OUTPUT_CHARS);
        let mut link = String::new();
        if more > 0 {
            let t = &self.target;
            match self
                .api
                .upload_text(&t.channel, &t.thread_ts, &format!("{name} output"), output)
                .await
            {
                Ok(url) => {
                    link = format!("\n[full output]({url})");
                    sources.push(Source {
                        url,
                        text: format!("full output · {}", fmt_size(output.len())),
                    });
                }
                Err(e) => tracing::warn!(error = %e, tool = %name, "tool output upload failed"),
            }
        }
        let details = first_line(output);
        let glyph = if is_error { "✗" } else { "✓" };
        let mut body = shown.to_owned();
        if more > 0 {
            body.push_str(&format!("\n… {more} more chars"));
        }
        let card = format!(
            "{glyph} **{name}**  {}\n```\n{body}\n```{link}",
            fmt_elapsed(elapsed)
        );
        self.queue_task(call_id, status, details, None, sources, card);
    }

    /// Tool calls started so far (for [`Footer::tools`]); reasoning cards
    /// do not count.
    #[must_use]
    pub fn tool_count(&self) -> u32 {
        self.tools
    }

    /// Queue a card chunk; a chunk already queued for the same id is
    /// replaced in place, so one flush carries one card per id.
    fn queue_task(
        &mut self,
        call_id: &str,
        status: TaskStatus,
        details: Option<String>,
        output: Option<String>,
        sources: Vec<Source>,
        card: String,
    ) {
        let Some(task) = self.tasks.get_mut(call_id) else {
            return;
        };
        let chunk = Chunk::Task {
            id: call_id.to_owned(),
            title: task.title.clone(),
            status,
            details,
            output,
            sources,
        };
        task.chunk = Some(chunk.clone());
        self.bubble.get_or_insert_with(Bubble::new);
        match self.queue.iter_mut().find(|q| q.call_id == call_id) {
            Some(q) => {
                q.chunk = chunk;
                q.card = card;
            }
            None => self.queue.push(Queued {
                call_id: call_id.to_owned(),
                chunk,
                card,
            }),
        }
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
        let footer = footer.to_string();
        let raw = b.out(0..b.text.raw().len());
        match &b.mode {
            Mode::Unsent => {
                tracing::warn!("turn finished before its bubble could be opened; text dropped");
            }
            Mode::Stream(ts) => {
                // Cards first: they completed before the text that follows them.
                let mut chunks: Vec<Chunk> = self.queue.drain(..).map(|q| q.chunk).collect();
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
                self.flush_update_tasks().await;
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
    /// the stream is dead) and start a fresh one seeded with the remainder
    /// and every in-progress task (21 §8.4).
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
        let reemit: Vec<Queued> = self
            .tasks
            .iter()
            .filter(|(id, t)| {
                t.status == TaskStatus::InProgress && !self.queue.iter().any(|q| &q.call_id == *id)
            })
            .filter_map(|(id, t)| {
                Some(Queued {
                    call_id: id.clone(),
                    chunk: t.chunk.clone()?,
                    card: String::new(),
                })
            })
            .collect();
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
                self.flush_update_tasks().await;
            }
            Err(e) => tracing::warn!(error = %e, "postMessage for update-mode bubble failed"),
        }
    }

    /// `Ok`: streaming. `Err(None)`: transient failure, retry next tick.
    /// `Err(Some(reason))`: this thread cannot stream; fall back now.
    async fn try_start_stream(&mut self) -> Result<(), Option<String>> {
        let Some(b) = self.bubble.as_mut() else {
            return Err(None);
        };
        let end = b.streamable();
        let mut chunks: Vec<Chunk> = self.queue.iter().map(|q| q.chunk.clone()).collect();
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
                b.mode = Mode::Stream(ts);
                b.sent = end;
                b.opened = Instant::now();
                b.last_append = b.opened;
                self.open_failures = 0;
                self.queue.clear();
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

    /// Stream flush: raw text since `sent` plus queued task chunks; with
    /// nothing to send, a keepalive refresh of an in-progress task.
    async fn flush_stream(&mut self) {
        let Some(b) = self.bubble.as_ref() else {
            return;
        };
        let Mode::Stream(ts) = &b.mode else { return };
        let end = b.streamable();
        let mut chunks: Vec<Chunk> = self.queue.iter().map(|q| q.chunk.clone()).collect();
        if b.sent < end {
            chunks.push(Chunk::Text(b.out(b.sent..end)));
        }
        if chunks.is_empty() {
            if b.last_append.elapsed() < KEEPALIVE {
                return;
            }
            let Some(live) = self
                .tasks
                .values()
                .find(|t| t.status == TaskStatus::InProgress)
                .and_then(|t| t.chunk.clone())
            else {
                return;
            };
            chunks.push(live);
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
                self.queue.clear();
            }
            Err(e) if is_dead(&e) => {
                tracing::warn!(error = %e, ts, "stream died; repairing");
                let len = b.text.raw().len();
                self.rotate(len, true).await;
            }
            Err(e) => tracing::warn!(error = %e, ts, "appendStream failed; will retry"),
        }
    }

    /// Update-mode flush (21 §8.2): committed text edited in place, one
    /// thread message per tool call.
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
        self.flush_update_tasks().await;
    }

    async fn flush_update_tasks(&mut self) {
        let t = &self.target;
        for q in std::mem::take(&mut self.queue) {
            let Chunk::Task { status, .. } = &q.chunk else {
                continue;
            };
            let Some(task) = self.tasks.get_mut(&q.call_id) else {
                continue;
            };
            let result = match (status, &task.card_ts) {
                (TaskStatus::Pending, _) => continue,
                (TaskStatus::InProgress, _) => self
                    .api
                    .post_markdown(&t.channel, Some(&t.thread_ts), &q.card)
                    .await
                    .map(|ts| {
                        task.card_ts = Some(ts);
                    }),
                (_, Some(card_ts)) => self.api.update_markdown(&t.channel, card_ts, &q.card).await,
                (_, None) => {
                    let combined = match q.card.find('\n') {
                        Some(i) if !task.describe.is_empty() => format!(
                            "{}\n```\n{}\n```{}",
                            &q.card[..i],
                            task.describe,
                            &q.card[i..]
                        ),
                        _ => q.card.clone(),
                    };
                    self.api
                        .post_markdown(&t.channel, Some(&t.thread_ts), &combined)
                        .await
                        .map(|_| ())
                }
            };
            if let Err(e) = result {
                tracing::warn!(error = %e, tool = %task.name, "task card failed");
            }
        }
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

fn fmt_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
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

#[cfg(test)]
mod tests {
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
        Upload {
            title: String,
        },
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
            title: &str,
            _text: &str,
        ) -> Result<String, ApiError> {
            self.check("upload")?;
            self.record(Call::Upload {
                title: title.to_owned(),
            });
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
    async fn tool_lifecycle_emits_three_task_chunks() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_pending("c1", "shell").await;
        t.tick().await;
        t.tool_started(
            "c1",
            "shell",
            &serde_json::json!({"command": "cargo test -p x"}),
        )
        .await;
        t.tick().await;
        t.tool_finished("c1", "ok: 12 passed\nmore", false).await;
        t.tick().await;
        let chunks = api.task_chunks();
        assert_eq!(chunks.len(), 3);
        let ids: Vec<_> = chunks
            .iter()
            .map(|c| match c {
                Chunk::Task { id, .. } => id.clone(),
                Chunk::Text(_) => unreachable!(),
            })
            .collect();
        assert_eq!(ids, vec!["c1", "c1", "c1"]);
        let Chunk::Task {
            title,
            status,
            details,
            ..
        } = &chunks[2]
        else {
            panic!()
        };
        assert!(title.contains("cargo test -p x"), "{title}");
        assert_eq!(*status, TaskStatus::Complete);
        assert_eq!(details.as_deref(), Some("ok: 12 passed"));
        let Chunk::Task { status, .. } = &chunks[1] else {
            panic!()
        };
        assert_eq!(*status, TaskStatus::InProgress);
    }

    #[tokio::test(start_paused = true)]
    async fn long_output_is_uploaded_and_linked() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::All);
        t.tool_started("c1", "read", &serde_json::json!({"path": "x"}))
            .await;
        let output = "see https://example.com/a\n".repeat(200);
        t.tool_finished("c1", &output, false).await;
        t.tick().await;
        assert!(
            api.calls()
                .iter()
                .any(|c| matches!(c, Call::Upload { title } if title == "read output"))
        );
        let chunks = api.task_chunks();
        let Chunk::Task { sources, .. } = chunks.last().unwrap() else {
            panic!()
        };
        assert!(sources.iter().any(|s| s.url == "https://files.slack.com/f1" && s.text.starts_with("full output")));
        assert!(
            sources
                .iter()
                .any(|s| s.url == "https://example.com/a" && s.text == "example.com")
        );
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
                Call::Upload { .. } => panic!(),
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
    async fn tool_cards_none_emits_no_tasks() {
        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::None);
        t.tool_pending("c1", "shell").await;
        t.tool_started("c1", "shell", &serde_json::json!({"command": "ls"}))
            .await;
        t.tool_finished("c1", "boom", true).await;
        t.text("x").await;
        t.tick().await;
        assert!(api.task_chunks().is_empty());
        assert_eq!(t.tool_count(), 1);

        let api = FakeApi::new();
        let mut t = turn(&api, ToolCards::Errors);
        t.tool_pending("c1", "shell").await;
        t.tool_started("c1", "shell", &serde_json::json!({"command": "ls"}))
            .await;
        t.tool_finished("c1", "fine", false).await;
        t.tool_pending("c2", "shell").await;
        t.tool_started("c2", "shell", &serde_json::json!({"command": "rm"}))
            .await;
        t.tool_finished("c2", "boom", true).await;
        t.tick().await;
        let chunks = api.task_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(
            matches!(&chunks[0], Chunk::Task { id, status: TaskStatus::Error, .. } if id == "c2")
        );
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
    async fn reasoning_is_a_growing_card_completed_by_text() {
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
                    title: "thinking".into(),
                    status: TaskStatus::Complete,
                    details: Some("first idea".into()),
                    output: Some("first idea\nsecond thought".into()),
                    sources: Vec::new(),
                },
                Chunk::Text("answer".into()),
            ]
        );
        // A run after a tool call is a new card.
        t.tool_started("c1", "shell", &serde_json::json!({"command": "ls"}))
            .await;
        t.reasoning("again").await;
        t.tick().await;
        let ids: Vec<_> = api
            .task_chunks()
            .into_iter()
            .filter_map(|c| match c {
                Chunk::Task { id, .. } => Some(id),
                Chunk::Text(_) => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "reasoning-1",
                "reasoning-1",
                "reasoning-1",
                "c1",
                "reasoning-2"
            ]
        );
        assert_eq!(t.tool_count(), 1);
        t.finish(footer()).await;
        let calls = api.calls();
        let Call::Stop { chunks, .. } = calls.last().unwrap() else {
            panic!("{calls:?}")
        };
        assert_eq!(
            task_of(chunks, "reasoning-2"),
            vec![(
                TaskStatus::Complete,
                Some("again".into()),
                Some("again".into())
            )]
        );
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
        assert_eq!(truncate_chars("héllo", 3), "hé…");
        assert_eq!(fmt_k(999), "999");
        assert_eq!(fmt_k(16_500), "16.5k");
        assert_eq!(fmt_k(1_000_000), "1.0M");
        let mut f = footer();
        assert!(!f.to_string().contains("ctx"));
        f.context = Some((16_500, 1_000_000));
        assert!(f.to_string().ends_with(" · 3 tools · ctx 16.5k/1.0M"), "{f}");
        assert_eq!(fmt_elapsed(Duration::from_millis(1200)), "1.2 s");
        assert_eq!(fmt_elapsed(Duration::from_secs(92)), "1m32s");
    }
}
