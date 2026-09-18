# 21 – Slack: a workspace-side frontend for the daemon

Status: as-built v1 · 2026-09-14 (§19) · extends `00-architecture.md` §4, §7, §13;
`09-signal.md` (sibling frontend; trust, threads, reactions and command
shapes are reused by reference); `19-subagent-board.md` §1–2 (subagents are
sessions; `list_agents`); `03-streaming-markdown.md` §2 (`markdown::Text`).
Where this doc disagrees with 09 about Slack, this doc wins. Slack API facts
were read from docs.slack.dev on 2026-09-14; slack-morphism facts from the
2.28.0 crate source the same day; jarvis references are to
`jarvis/crates/jarvis-slack/src/{channel,client,fmt}.rs` (`C:`, `K:`, `F:`).

Slack is a frontend, like the terminal and Signal: it sends `SendInput` /
`Steer` / `Cancel` / `Resolve` and renders `AgentEvent`s. It runs **only in
the daemon** (`wth --headless`). It is the first frontend that can show the
whole transcript live and natively: Slack's agent surface has a
server-rendered markdown stream with a tool-call timeline, a session status
with a stop button, thread titles, buttons, reactions and threads. This doc
uses all of it; the rule is *Slack's primitive first, our message second*.

## 0. What jarvis taught, in one table

jarvis-slack is 9.4k lines (`channel.rs` 7 201, `client.rs` 1 205, `fmt.rs`
1 011); the audit puts the essential part at ~1 200. The rest is machinery
around decisions this doc changes.

| jarvis did | why it hurt | wth does |
|---|---|---|
| Kept one stream alive across tool calls with no keepalive; on `msg_too_long` rolled to a new bubble up to 8× with a "never repost" invariant (C:2404-2516) | the invariant forced `streamed_total`/`uncounted_raw`/`presentation_prefix` accounting and a longest-overlap tail reconciliation (C:209-332, 5959-6005): ~1 000 lines | `markdown::Text::raw()` is the only truth; a bubble that fails is repaired with `chat.update(ts, markdown_text = raw)`; rotation is one routine with three triggers (§8.4); keepalive is a `task_update` refresh (§8.3) |
| Two markdown dialects: stream `markdown_text` + a regex md→mrkdwn converter for `postMessage` fallbacks (F:608-771) | 1 000 lines, unhandled cases (nested lists, quotes, `_italic_`) | `markdown_text` everywhere (`postMessage`, `update`, `*Stream` all take it, 12 000 chars); `fmt.rs` does not exist |
| `task_update` cards with only title + status, args and output withheld (C:486-500) | native look, zero observability | title = described args; `details` = first output line; full output uploaded and linked as a `source` (§8.3) |
| slack-morphism 2.22 without `.with_rate_control` (K:828); approval countdown `chat.update` storms starved `postMessage` (C:5536) | 429s became dropped cards and lost appends | rate control on; no countdowns; one pacer for `appendStream` (§4.2) |
| morphism's closed event enum failed on `app_context_changed` → unacked → Slack retried ×3 (C:4895) | log noise, duplicate work | upstream `Unknown(Value)` (§2.1); unknown events are acked and counted |
| Approvals/questions/loop-ack cards with short codes, `/jarvis` operator CLI (~1 700 lines) | product surface, not a frontend | one approval card, two buttons whose `value` is the `ApprovalId` (restart-safe, C:1856); `/wth` mirrors the Signal command table |
| Only `channel:thread_ts → conversation` persisted; live streams lost on restart → dangling "streaming…" bubbles (C:394) | | `threads.json` also lists open stream `ts`; boot stops them (§11) |
| No subagent visibility | | a thread per child + a board message in the parent (§9) |

Kept from jarvis because it was right: Socket Mode; the work off the ack
path; dedup on `event_id` in a bounded set (C:77-105);
`FootnotesMode::Invalidate` (F:38-41); a top-level DM message is a new
thread and a new session (C:1276-1284); the first delta opens the stream,
because an empty stream clears the "working" status (C:648); `mdstream`
block boundaries as the only place a message may be cut (F:6-11).

## 1. Goal, and what is not one

Goal: talk to the daemon from Slack (DM or `@wth` in a channel) and get the
same agent the terminal gets, rendered with Slack's agent UI: streamed
markdown, a timeline card per tool call with its output one click away,
reasoning in a collapsed quote, approval buttons, the native "Working…"
status and stop button, thread titles, one thread per subagent and a live
board of children. Every Slack API failure is visible (bus event, status
counters, log) and never silent.

Not goals, with the reason:

- **Events API over HTTP.** Needs a public URL; Socket Mode is the same
  payloads over a WebSocket we open, and a daemon can keep a socket up.
- **Running in TUI mode.** Slack delivers each envelope to *one* of an
  app's connections, round-robin; two processes on one app is a split
  brain. The terminal process never opens the socket; the actor answers
  `DaemonOnly` (09 §3 shape).
- **Marketplace distribution / OAuth install.** One workspace, one installed
  app, two tokens in the secrets vault (37). The 2025 non-Marketplace rate cut
  (`conversations.history` → 1/min) is for distributed apps and we never
  call `conversations.history`.
- **Voice/TTS.** Inbound files are saved and their path handed to the model
  (09 §7.4); images and PDFs ride along in `files` (17 §5, 20 §2.5).
- **Slack as a data source** (`app_context_changed` entities, channel
  history, canvases). wth has no Slack read tools; when one exists it gets
  its own doc and the context event becomes useful.
- **Plan mode** (`task_display_mode: plan`, `plan_update`). It maps onto
  wth's `todo` (07 §5) exactly, but the loop publishes no todo event; a
  `TodoChanged` `AgentEvent` is an `agent` change the TUI would also use
  (its status line shows nothing about todos today). Add the event first,
  then this is ~20 lines here. §17.
- **Feedback buttons** (`context_actions` / `feedback_buttons` in
  `stopStream` blocks). Nothing in wth consumes a 👍/👎 on a turn; when
  something does (knowledge harvest weighting?), one block.

## 2. Layout

```
crates/slack/
  Cargo.toml
  src/
    lib.rs        plugin, `slack` actor + messages, config, SlackEvent/SlackStatus, tool       (~250)
    socket.rs     morphism Socket Mode listener wiring, user-state → Arc<Channel>, dedup       (~150)
    api.rs        thin façade over morphism session: our methods, exact error codes, pacer     (~200)
    channel.rs    inbound routing, per-thread outbound workers, board, owner DM               (~600)
    bubble.rs     one streamed message per turn: Stream | Update mode over markdown::Text,
                  task cards, rotation, repair                                                  (~300)
    threads.rs    thread ↔ session map + open streams, `threads.json`                          (~100)
    commands.rs   `/wth …` parser + dispatcher (09 §9 table)                                    (~300)
    approvals.rs  card blocks, button routing                                                   (~100)
  manifest.yaml   the Slack app manifest; `wth --slack-manifest` prints it
~/.wth/slack/
  threads.json    { threads: { "C…:1699999.123456": { session, user, team, title } }, streams: [ { channel, ts } ] }
  inbox/          inbound files, `<ts>-<name>` (09 §7.4)
```

Budget: ~2 000 lines plus tests. `channel.rs` past 900 lines splits at the
seams already named (board, owner DM), not before.

### 2.1 Dependencies

**slack-morphism upstream Git** (`features = ["hyper"]`; Apache-2.0), pinned
to `e2a4b014b2bedc82c1ccfeae1706e0f2d6a184a3` on upstream `master` (its default
branch). PR #388 is merged there but not yet released to crates.io. It gives Socket Mode with
reconnect, ping (15 s, 5 misses), per-envelope ack, 2 connections and
`disconnect`-frame handling (`socket_mode/`), a rate controller that
honours 429 `Retry-After` and per-tier throttles
(`SlackApiRateControlConfig`), typed Block Kit including the `markdown`
block, `files.getUploadURLExternal`/`completeUploadExternal`,
`reactions.add`, `chat.postEphemeral`, `auth.test`, `conversations.open`,
and a generic `http_post(method, &Req, rate_ctl)` (client.rs:318) for
anything it lacks.

The gaps in release 2.28 that our PR #388 filled, now supplied by upstream:

| former gap | where | upstream support |
|---|---|---|
| `agent_session_stopped`, `agent_session_title_changed`, `app_context_changed` events; unknown events failed the whole envelope | events | typed variants plus `Unknown(serde_json::Value)` retaining unmodelled payloads |
| `markdown_text` on `SlackMessageContent` (`chat.postMessage`, `chat.update`) | chat.rs | one `Option<String>` field |
| `chat.startStream` / `appendStream` / `stopStream` | — | request/response structs + three methods, T2/T4/T2 rate configs |
| `agents.sessions.setStatus` / `rename` | assistant.rs has only `assistant.threads.*` | two methods, T3 |

The workspace dependency points directly to upstream with an exact Git `rev`;
there is no fork or `[patch.crates-io]` entry for Slack. Keep the Git pin until
a release contains PR #388. The merged API uses `SlackTaskId`,
`SlackTaskCardStatus` and `SlackTaskCardSource::Url(SlackUrlSourceElement)`;
source URLs are parsed before sending and invalid URLs return an error.
`chat.stopStream` carries footer blocks through its flattened `SlackMessageContent`.

Two known integration facts, so nobody rediscovers them: callbacks are
`fn` pointers taking `SlackClientEventsUserState` (callbacks.rs:28), so
`Arc<Channel>` lives in the user state (jarvis C:4879 did this; it is one
`insert`/`read` pair); with reqwest (workspace) and morphism's hyper-rustls
in one binary both `ring` and `aws-lc-rs` may link and rustls panics
choosing a provider (jarvis Cargo.toml:33) — the fix, if it bites, is
`rustls::crypto::aws_lc_rs::default_provider().install_default()` once in
`main`.

Rejected: a hand-rolled client over `reqwest` + `tokio-tungstenite`. v0 of
this doc chose it because of the three gaps above; that is reinventing a
maintained wheel to dodge a 60-line patch, and 00 §1.2/§1.3 say the
opposite. Rejected: `slack-rust`, `slacko`, `mecha-slack` (no Socket Mode,
low activity).

Other deps: `reqwest` (file downloads with the bot token — morphism's
connector is for `slack.com/api`), `mdstream` through `markdown`,
`serde_json`, `uuid`, `chrono`.

## 3. Plugin lifecycle

`main.rs` pushes `Box::new(slack::SlackPlugin { channel: cli.headless })`
after `signal`. `start`:

1. Read `slack:` config (§12); `enabled: false` → log, return. Read
   `slack_bot` / `slack_app` from the secrets vault (`secrets::Handle`, 37
   §3.6); either missing → `warn!` naming the keys, spawn
   the actor in `Unconfigured` state, return. The harness comes up
   regardless (plugins.md §3.2).
2. `channel == true`: `channel::spawn(cfg, tokens, rt)` → `Handle`; runs the
   supervisor under `rt.shutdown`. `channel == false`: no socket.
3. Spawn the `Slack` actor (`handle: Option<Handle>`), register as
   `"slack"`, register tool `slack_status` via `lookup_when::<ToolRegistry>`.

### 3.1 The supervisor (`socket::run`)

```
auth.test (bot token) → bot_user_id, team_id, team url          retry with backoff_delay until ok
conversations.open { users: owner } → owner DM channel id        (§10.1; retried lazily on first use if it fails)
stop every ts in threads.json.streams (best effort, §11)
listener = SlackClientSocketModeListener::new(config { max_connections_count: 2 }, env(with_rate_control, error_handler), callbacks)
  push events      → ack (morphism) → RecentKeys(event_id) ? drop : inbound_tx.send(Inbound::Push(ev))
  interaction      → ack           → inbound_tx.send(Inbound::Action(ev))
  slash commands   → answer per §10.2 inside the callback (Slack's 3 s), heavy work spawned
  hello            → publish Connected{connections}
  error_handler    → protocol error (an envelope we could not parse even with `Unknown`) → count, debug!;
                     transport error → publish Disconnected{reason}; morphism reconnects with its backoff
listener.listen_for(app_token) ; listener.serve() until rt.shutdown → listener.shutdown()
```

`RecentKeys` is a 1 024-entry FIFO set (jarvis C:45) keyed by `event_id`
(push) or `trigger_id` (interaction). Morphism acks before our callback
returns, so a crash between ack and `SendInput` loses that message; the
user sees no 👀 and resends. The alternative (ack after handling) turns any
> 3 s handler into a redelivery. Bounded loss is the right side.

Inbound is one ordered `mpsc` worker (`channel::inbound`), so a slow agent
ask never blocks the listener's read loop.

## 4. Web API (`api.rs`)

### 4.1 One façade

```rust
pub struct Api { client: Arc<SlackClient<SlackClientHyperConnector>>, bot: SlackApiToken,
                 pacer: Mutex<HashMap<&'static str, Instant>>, calls: Mutex<BTreeMap<&'static str, CallStats>> }
impl Api {
    pub async fn post_message(&self, channel, thread_ts, body: Body /* MarkdownText(String) | Blocks(Vec<SlackBlock>) */) -> Result<Ts>;
    pub async fn update(&self, channel, ts, body: Body) -> Result<()>;
    pub async fn start_stream(&self, channel, thread_ts, recipient: (&user, &team), first: Chunks) -> Result<Ts>;
    pub async fn append_stream(&self, channel, ts, chunks: Chunks) -> Result<()>;
    pub async fn stop_stream(&self, channel, ts, chunks: Chunks, blocks: Vec<SlackBlock>) -> Result<()>;
    pub async fn set_session_status(&self, channel, thread_ts, status: SessionStatus) -> Result<()>;
    pub async fn rename_session(&self, channel, thread_ts, title) -> Result<()>;
    pub async fn react(&self, channel, ts, emoji) -> Result<()>;
    pub async fn ephemeral(&self, channel, user, text) -> Result<()>;
    pub async fn upload_text(&self, channel, thread_ts, title, text) -> Result<Url /* file permalink */>;
    pub async fn download(&self, url_private_download) -> Result<Bytes>;   // reqwest with the bot token
}
pub enum ApiError { Slack(&'static str /* exact `error` code, interned */), RateLimited(Duration), Transport(String) }
```

Every call is a `tracing` span `slack.api{method}` with the error code on
failure and bumps `calls[method].{ok,err,rate_limited}` for `Status`.
Morphism's rate controller retries a 429 by `Retry-After`; when it gives up
we publish `SlackEvent::RateLimited { method, retry_after_secs }`.

Error codes are matched exactly on the `error` field, never by substring
(jarvis C:311). The ones the design branches on:
`message_not_in_streaming_state`, `stopped_by_user`, `msg_too_long`,
`streaming_mode_mismatch`, `channel_type_not_supported`,
`missing_recipient_user_id`, `invalid_thread_ts`, `not_in_channel`,
`missing_scope`, `not_allowed_token_type`, `ratelimited`.

### 4.2 Pacing

Tiers (docs.slack.dev, 2026-09-14): `chat.appendStream` T4 (100+/min),
`chat.startStream`/`stopStream` T2 (20+/min), `chat.update` T3 (50+/min),
`chat.postMessage` special (~1/s/channel), `reactions.add` T3,
`agents.sessions.setStatus`/`rename` T3, `files.*External` T4. Limits are
per method per workspace per app, so two threads streaming share one
`appendStream` budget.

Morphism's controller throttles per configured tier and retries 429s; on
top of it `pacer` enforces a minimum gap per method across all workers:
`chat.appendStream` 650 ms (≤ 92/min in total), `chat.update` 1 200 ms,
others 0. A turn costs: 1 `startStream` + 1 `stopStream` per bubble, 2–3
`appendStream` per tool call (pending, in_progress, complete) plus one per
750 ms of text, 2 `reactions.add`, 2 `setStatus`, ≤ 1 `rename`, and 3
upload calls per long output. A 20-tool, 10-minute turn is ~150 calls; the
only T2 pressure is one `startStream` per bubble.

## 5. Trust

`slack.owner` (one `U…`) and `slack.allowed` (more). `slack.channels`: the
channels where `@wth` is honoured; empty = DMs only. A DM from a user not in
the list is dropped with one `warn!` and no reply (09 §5.1: a bot answering
strangers advertises itself). A mention in a listed channel from a
non-allowed user gets one `chat.postEphemeral` `not allowed`; a mention in
an unlisted channel is ignored. Owner-only: `/wth approvals`, `/wth slack
…`, cards in the owner DM (§10.1). Approval buttons in a thread: owner or
the thread's user. Ids, not names.

## 6. Threads and sessions (`threads.rs`)

One Slack thread ↔ one session, `Thread { session, user, team, title }`
keyed by `channel:thread_ts`, in `slack/threads.json`, written on every
change (09 §6 shape; `Threads::bind/unbind/by_session` + the open-stream
list §11).

- **Top-level DM message** (`message.im`, no `thread_ts`, not `bot_id`, no
  `subtype`) → `SendInput { session: None }`; the new session is bound to
  `channel:<that message's ts>`; every reply goes in its thread. This is
  Slack's own "each message in the agent tab is a session" model; `/new` is
  therefore implicit.
- **Reply in a bound thread** → `Steer` while a turn runs, else `SendInput`
  (09 §7 rule).
- **Channel**: `app_mention` only; `<@Ubot>` stripped; thread rule as above.
  Unmentioned channel traffic is never read (`channels:history` not
  requested).
- **`/wth resume <id>`** → binds a *new* thread (rooted at a message we post:
  `resumed <id> · <title>`) to an existing session (full id, unique prefix
  or suffix, 09 §6).
- **Subagents**: `SubagentSpawned { parent, child, name }` with a bound
  `parent` binds `child` to a new thread in the same channel (§9).
- **Restart** → the file is read (missing or corrupt → empty). Workers,
  bubbles, cards and dedup are process-local by design (§11).

## 7. Inbound (`channel::inbound`)

```
Inbound (already acked and deduped)
  Push app_mention | message.im (skip bot_id, subtype other than file_share, message_changed) →
    user allowed? no → drop (DM) / ephemeral (channel)
    thread_key = channel:(thread_ts ?? ts)
    text = strip_mentions(text) + one `[attachment: …]` line per file (§7.1); an
      `audio/*` file is transcribed instead → `[voice note, N s] …` (45 §4.5); empty → drop
    reactions.add 👀 on (channel, ts)                            "seen, working"
    session = threads[thread_key] (create per §6)
    turn in flight? Steer : SendInput (files → `files` param for images/PDFs)
    remember (session → user ts) for the terminal reaction (§8.6)
    ask failed → reply `⚠️ <error>` in thread, react ❌
  Push agent_session_stopped { channel, thread_ts } → Cancel(session)   (the native stop button)
  Push agent_session_title_changed { title } → threads[key].title = title  (a user renamed it; we stop overwriting, §8.6)
  Push Unknown → counted in Status.dropped, debug!
  Action block_actions → approvals::route (§10.1)
```

### 7.1 Files

`files[]` on the event: download `url_private_download` with the bot token
(`files:read`) into `slack/inbox/<ts>-<name>`, append
`[attachment: <path> (<mimetype>, <size>)]`, and pass images/PDFs in
`files` (09 §7.4, 17 §5). Failures append `[attachment: <name> — download
failed: <err>]` so the model knows.

## 8. Outbound (`channel::worker`, `bubble.rs`)

One ordered worker task per bound thread, spawned on first use, holding all
per-thread state; the bus subscriber actor routes each event by
`session → thread_key` into that worker's `mpsc`. One in-flight API call per
thread ⇒ appends never reorder; a slow thread never stalls another; the
pacer (§4.2) is the only cross-thread coupling. Events for unbound sessions
are dropped at the router.

```rust
struct Worker {
    key: ThreadKey, channel: String, thread_ts: String, user: String, team: String,
    turn: Option<Turn>,                       // TurnStarted..TurnFinished
    marks: Vec<String /*user message ts*/>,   // 👀 → ✅/🛑/❌ (§8.6)
    approvals: HashMap<ApprovalId, String /*card ts*/>,
    board: Option<Board>,                     // §9.2, parent threads only
    children: BTreeMap<SessionId, Row>,       // §9.2
    status_supported: bool, title_sent: Option<String>,
}
struct Turn {
    started: Instant, model: String,
    bubble: Option<Bubble>,                   // §8.1; None until the first event that needs one
    reasoning: Option<Segment>,               // §8.5
    tasks: HashMap<String /*call_id*/, Task { name, title, started: Instant, card_ts: Option<String> }>,
    usage: Usage,
}
```

Worker loop: `select! { ev = rx.recv() => apply(ev), _ = tick(750 ms) => flush() }`.

### 8.1 The bubble: one streamed message per turn

Slack's agent stream is one message with a **timeline** of task cards and a
markdown body. That is the turn: tool calls above, answer below. So the
unit is one `Bubble` per turn, not one per text segment:

Voice turns (45 §6, added 2026-09-17): when the input was a voice note, each
prose block — the text before a batch of tool calls, and the tail — is also
synthesized and uploaded in the thread as `reply.mp3`, in order, as soon as
the clip is ready. The bubble is unchanged; the clips sit beside it.

```rust
pub struct Bubble {
    ts: String,
    mode: Mode,                    // Stream | Update
    text: markdown::Text,          // this bubble's share of the turn's text; raw() is the truth
    sent: usize,                   // bytes of raw() Slack has confirmed (Stream) / shown (Update)
    tasks_here: HashSet<String>,   // call_ids whose card lives in this message
    opened: Instant, last_append: Instant,
}
pub enum Chunk { Text(String), Task { id, title, status: Pending|InProgress|Complete|Error, details: Option<String>, sources: Vec<(Url, String)> } }
```

**Open** on the first `TextDelta`, `ToolCallPending` or `ReasoningDelta`
of the turn (never on `TurnStarted` — an empty stream clears the "working"
status, jarvis C:648): `chat.startStream { channel, thread_ts,
recipient_user_id, recipient_team_id, task_display_mode: "timeline",
chunks: [first] }` → `Mode::Stream`; `ts` is appended to
`threads.json.streams` (§11). `channel_type_not_supported |
missing_recipient_* | invalid_thread_ts | missing_scope |
not_allowed_token_type` → `Mode::Update` (§8.2) and
`SlackEvent::StreamFallback { session, reason }`, once per process per
reason. Chunks mode, never top-level `markdown_text` on the stream: mode is
locked at start and chunks are the only way to carry tasks (jarvis C:302).

**Flush** on the tick while `raw().len() > sent` or task chunks are queued:
`chat.appendStream { chunks: [Text(raw[sent..]), Task…] }`; `ok` → `sent =
raw.len()`. Raw deltas go on the wire; Slack's stream renderer is built for
token streams and renders the accumulated markdown, so a half-open `**bol`
is Slack's to show and ours to complete. The final text is byte-identical
to the model's.

**Close** on `TurnFinished`: `text.finish()`, last flush, then
`chat.stopStream { chunks: [tail], blocks: [footer], session_status:
"active" }` where the footer is one `context` block: `claude-sonnet-4-5 ·
12.3k in / 1.8k out · 42 s · 3 tools`. `ts` leaves `threads.json.streams`.
A turn that opened nothing (cancelled before the first token) closes
nothing.

### 8.2 Update mode: the same bubble without a stream

When a stream cannot be opened, the bubble is an ordinary message,
`chat.postMessage { thread_ts, markdown_text }`, edited in place:

- **Text**: `chat.update { markdown_text: raw[..committed()] }` on each tick
  where `committed()` grew — `markdown::Text::committed()` is a new one-line
  accessor exposing `mapped` (03 §2). Only committed blocks are shown, so an
  edit never shows a half-table; the pending paragraph appears when it
  closes. Paragraph-granular reveal, zero flicker: right for a message
  that is being edited in place, and the "stream until a block is complete,
  then edit the message with the complete block" the TUI does.
- **Tasks**: one thread message per tool call, `⚙ **shell**  running…` +
  fenced described args on `ToolCallStarted`, `chat.update`d on
  `ToolCallFinished` to `✓ **shell**  1.2 s` + fenced output (≤ 2 500 chars,
  then `… n more chars`) and the snippet link (§8.3). `Task.card_ts` holds
  the message.
- **Close**: the last `chat.update` is the close; the footer is appended as
  a final line.

Update mode is also the **repair** path (§8.4), which is why it is a mode
of the same `Bubble` and not a separate code path.

### 8.3 Task cards (native tool observability)

| event | chunk |
|---|---|
| `ToolCallPending { call_id, name }` | `Task { id: call_id, title: "<name>", status: pending }` |
| `ToolCallStarted { args }` | `Task { title: "<name> · <describe(args)>" ≤ 256, status: in_progress }` |
| `ToolCallFinished { output, is_error }` | `Task { status: complete\|error, details: first line of output ≤ 256, sources }` |

`describe` is the TUI's (00 §4: `shell` → the command; anything else →
`key: value`, long strings collapsed to their size). `sources` carries up
to 3 `http(s)` URLs found in the output (`markdown::urls`) — for `read_url`
/ `search_web` that is Slack's citation UI for free — and, when the output
exceeds `TOOL_OUTPUT_CHARS = 2 500`, the permalink of a text snippet
uploaded whole (`files.getUploadURLExternal` → PUT →
`completeUploadExternal { channel_id, thread_ts, title: "<name> output" }`)
as `("<url>", "full output · 12 KB")`. The card shows what ran and the
first line of what came back; the whole output is one click away. That is
the "tool output observability" requirement inside Slack's own card.
`slack.tools: all | errors | none` (§12): `errors` sends cards only for
`is_error` results (as one `complete` chunk), `none` sends no task chunks.

**Keepalive.** Slack closes a stream after ~30 s without an append
(undocumented; python-slack-sdk#1859). A tool can run for minutes. Every
tick with an `in_progress` task and `last_append > 20 s` re-sends that
task's chunk unchanged — a status refresh, which is a legitimate
`task_update`, not a hack. A turn with no tool in flight and no text for
20 s is a stalled model; then the stream dies and §8.4 heals it.

### 8.4 Rotation and repair: one routine, three triggers

`rotate(reason)`: close the current bubble — `chat.stopStream` normally, or
`chat.update { ts, markdown_text: raw }` when the stream is already dead
(Slack staff recommend exactly this repair, python-slack-sdk#1859; the
message exists, the text is complete from byte 0, `chat.update` is
idempotent) — then the next event opens a fresh bubble in the same thread
whose first chunks re-emit every `in_progress` task, so the timeline
continues. Triggers:

1. **Size.** `markdown_text` is 12 000 chars per call and a message ~40 000.
   When `committed()` crosses `BUBBLE_SOFT_MAX = 11 000`, finish this
   bubble's text at that block boundary (`sent` clamped, the remainder
   re-pushed into the new bubble's `Text`) so no construct spans two
   messages. A single block over the cap (a giant fence) is cut at its
   last newline; the continuation renders raw in the next message.
   `ponytail:` known ceiling; re-open the fence if it grates.
2. **Age.** Slack closes streams around 5 minutes wall (measured in §16.3).
   At `BUBBLE_MAX_AGE = 4 min 30 s`, rotate at the next block boundary
   (or immediately if no text is pending).
3. **Death.** `appendStream`/`stopStream` answering
   `message_not_in_streaming_state | streaming_mode_mismatch |
   msg_too_long`, or `RateLimited` after morphism's retries → repair now,
   publish `SlackEvent::StreamFallback`. `stopped_by_user` → the user hit
   stop; `agent_session_stopped` is already on its way to `Cancel`, so
   just repair.

No roll counter, no tail reconciliation, no `uncounted_raw`: `raw()` is
complete and the repair is a full rewrite of one message.

### 8.5 Reasoning

In stream mode reasoning is a **timeline task card**, so it sits exactly
where it happened between the tool cards: on the first `ReasoningDelta` of
a reasoning run `Task { id: "reasoning-<n>", title: "thinking", status:
in_progress }`; on every tick while it runs the same id is re-sent with
`output` = the committed reasoning text so far (the spike showed `output`
accepts 40 000 chars and an identical id replaces the card); the first
non-reasoning event completes it (`status: complete`, `details` = first
line, `output` = the whole text ≤ 12 000). In update mode (no stream) it
is a separate quoted message (`_thinking…_` then a `> ` blockquote, edited
at block boundaries) — there every card is its own message anyway, so
post order is event order. `slack.reasoning: true | false`, default
`true`. Providers that expose no reasoning emit nothing.

As-built v1 posted the quoted message in stream mode too and it sorted
**below** the bubble whenever reasoning happened mid-turn: the bubble is
one message created at turn start and Slack orders a thread by post time.
Not an edit race; a consequence of one bubble per turn, fixed by putting
reasoning inside it.

### 8.6 Status: reactions, session status, title

Reactions on the *user's* message, the 09 §8.2 table verbatim: 👀 on
accept, ✅ on normal `TurnFinished`, 🛑 on `"cancelled"`, ❌ on `"error"` /
`Error`. Slack does not replace reactions, so the terminal one is added
beside 👀.

`agents.sessions.setStatus { channel_id, thread_ts, status: "processing" }`
on `TurnStarted`, `"active"` via `stopStream.session_status` or an explicit
call when nothing was streamed. Slack shows "Working…" and, because we
subscribe to `agent_session_stopped`, the native **stop button** that
reaches `Cancel` (§7). `processing` expires after 1 h; a turn that long
re-sends it from the tick. On `missing_scope`/`not_allowed_token_type` the
worker sets `status_supported = false` and logs once: the app was not
created as an agent (§10.3).

**Title.** After `TurnFinished`, `ListSessions` → this session's
`SessionSummary.title` (07 §15: model-written after the first turn); if it
differs from `title_sent` and the user has not renamed the thread
(`agent_session_title_changed` sets `Thread.title`, §7),
`agents.sessions.rename { channel_id, thread_ts, title ≤ 200 }`. One ask
and at most one T3 call per turn. Child threads use `name · task first
line`.

### 8.7 Other events

| event | thread message |
|---|---|
| `MessageDelivered { from, text }` | `⟵ **from**` + body as `markdown_text` (≤ 12 000, truncated with a count); a child's result or a `<job_result>` — posted as its own message, not into the bubble, so it survives the turn's rotation |
| `Error { message }` | `⚠️ <message>` |
| `Compacted { epoch, folded }` | `⟲ compacted → epoch n (m messages folded)` |
| `InputQueued` | 👀 on the steer message (done inbound); nothing else |
| `SubagentSpawned` | §9 |
| `ModelChanged`, `EffortChanged`, `SkillsChanged`, `Harvest*`, `Compacting`, `Input{Injected,Withdrawn}` | nothing |

- `message_user` (24 §5): a separate `chat.postMessage` (`markdown_text`)
  below the live bubble in the bound thread, or one
  `files.completeUploadExternal` with `initial_comment` when files ride
  along; unbound sessions land in the owner DM prefixed `_session <last 8>
  · <title>_`. `user.rs`; `SlackApi::upload_file`, of which `upload_text`
  (§8.3) is now a wrapper.

## 9. Subagents

19 §1: a subagent is a session. So a child gets what a session gets — a
thread with its own bubbles, task cards and approvals — and the parent
thread gets a board, the way the TUI gets one under its transcript.

### 9.1 A thread per child

`SubagentSpawned { parent, child, name }`, `parent` bound → post in the
parent's channel a **new top-level message** `↳ **grep-logs** · child of
<parent permalink|this thread>` and bind `child` to `channel:<its ts>`.
Permalinks come from `chat.getPermalink`, cached per thread (Enterprise
Grid workspaces serve a different host than `auth.test`'s `url`, so the
URL cannot be built; the built form is the fallback). The child's first
`TurnStarted.text` (the task) is posted into the child thread as `▸
**task**` + text (≤ 3 000); then the child's turns render there through
the same worker code — nothing is special-cased by depth. In the
**parent** thread: one line `↳ spawned **grep-logs** → <link>`; the result
arrives as the usual `⟵ grep-logs` card (§8.7); a child's `Error` or
cancelled `TurnFinished` interjects one line (19 §3.3 rule).

Typing in a child thread steers the child (`Steer`/`SendInput` to that
session; an `Awaiting` child gets its answer, 19 §2.3) — the thread map
makes this free. Depth 2 works the same. Bounded by `max_concurrent` (8)
per parent.

Rejected: child events inline in the parent thread, prefixed (streams
interleave; a table from one child splits another's paragraph); a child
thread *inside* the parent thread (no nested threads in Slack); children as
`task_update`s in the parent's timeline (a child's turn is minutes of
tools and text; a 256-char card cannot carry it — the board row links to
where it lives).

### 9.2 The board

While at least one descendant of a bound session is not idle, its thread
holds one **board message**, `chat.update`d in place:

```
**agents**
⠹ grep-logs      running   read_file crates/agent/src/loop_.rs   1m32s   <link|thread>
⏳ fix-tests      awaiting  parent: "should I also update the docs?"  48s   <link|thread>
⚠ scout          approval  shell `cargo test -p agent`                      <link|thread>
```

Rows are 19 §3.1's (glyph · name · state · detail · age · link), derived from
the child events the worker already routes (the child's worker forwards a
`RowChanged` to the parent's worker on `TurnStarted/Finished`,
`ToolCallStarted/Finished`, `Error`, `ApprovalRequested/Resolved`), capped
at 8 rows with `… n more`. Edits are coalesced by the tick (≥ 2 s apart).
When every child is idle the last edit reads `**agents** · all idle` and
the message stays as history. After a restart the board is rebuilt on the
next child event from `list_agents` (one ask, 19 §2.1). No new
`AgentEvent`; derive, don't duplicate (19 §1).

## 10. Interactivity

### 10.1 Approvals

An `ApprovalRequested` is posted as a Block Kit card — one `markdown` block
+ one `actions` block:

```
⚠️ **shell** wants to run                      [Allow] [Deny]
```sh
rm -rf target/
```
```

Body as `approvals::message` in 09 §10 (`shell` → command verbatim; others →
pretty JSON ≤ 600 chars). Buttons: `action_id` `approve` / `deny`, `value` =
the `ApprovalId` uuid — self-describing, so a card from before a restart
still works (jarvis C:1856).

Where it goes, `slack.approvals` (§12):

- session bound to a Slack thread → **that thread** (a child's approval
  lands in the child's thread; the parent board row says `approval`). Who
  may click: owner or the thread's user.
- session not bound (terminal, RPC, ACP, a job) and `approvals: all`
  (default) → the **owner's DM** as a top-level message with a context
  line `session <last 8> · <title>` (title from the `ListSessions` the
  worker already makes; `Unknown` when the ask fails). Only the owner may
  click. `approvals: threads` disables this and matches 09's behaviour.

`block_actions` (§7) → allowed clicker? → `ask Resolve { id, decision }`;
`false` (already resolved) → ephemeral `already resolved`; not allowed →
ephemeral `not allowed`. `ApprovalResolved` from anywhere (terminal, RPC,
timeout) → `chat.update` the card to `✓ allowed by <@U…>` / `✗ denied` /
`⌛ timed out`, buttons removed.

Rejected: short codes and `/wth approve` (buttons are the native gesture
and work on the phone client; 09's code path exists because Signal has no
buttons); 👍/👎 reactions (a second path for the same click); countdown
edits (jarvis C:5536 — they starved `postMessage`).

### 10.2 `/wth` slash command

A leading `/` in Slack is a Slack command and the client refuses unknown
ones, so text `/status` never reaches us. One manifest command `/wth`
carries the 09 §9 table as sub-commands with the same names, arguments and
meaning: `help status sessions resume cancel compact model agents prompt
skills mcp login logout approvals slack`. Not carried: `new` (a top-level
message is new), `approve/deny` (buttons), `signal`. Added: `slack status`
(the §13 snapshot).

Ack within 3 s: morphism's command callback returns the response; answers
that are pure formatting of one fast ask return blocks directly; the rest
return an empty ack and post through `response_url` (5 uses / 30 min) when
the ask finishes — `compact` and `resume` take seconds.

Slash payloads carry `channel_id` and `user_id` but no thread. A
session-scoped sub-command (`cancel`, `compact`, `agents`, `model <id>`,
`prompt`) resolves the session as **the invoking user's most recently
active thread in that channel**; none → the reply says so and offers the
session picker. Explicit `… <session-prefix>` overrides. Replies are
ephemeral, so a channel stays clean.

**Replies are Block Kit, not fenced text.** Slack has selects, buttons,
overflow menus and `replace_original`; a phone user should tap, not type
ids. v1 rendered every reply as a code block and this section replaces
that. `ui.rs` builds the blocks; `commands::handle` returns `Reply {
text: String /* notification fallback */, blocks: Option<Value> }`; button
and select clicks arrive as `block_actions` with `action_id` prefixed
`cmd:` and are routed by `channel::handle_inbound` next to the approval
clicks, answering through the click's `response_url` with
`replace_original: true` so the panel updates in place.

| command | blocks | actions (`action_id` → effect) |
|---|---|---|
| `/wth` (bare) / `help` | header `wth`, one `section` per group (session · harness · setup) with `mrkdwn` one-liners, an `actions` row of the common buttons: Sessions · Agents · Models · Status | `cmd:sessions`, `cmd:agents`, `cmd:models`, `cmd:status` re-render that panel in place |
| `status` | `section` with `fields` (connection · bot · team · threads · open streams · session · model · turn) + `context` of per-method call counts | `cmd:refresh` |
| `sessions` | header + one `section` per session (title bold, `last-8 · model · age` as context) with an `overflow` menu: Resume here · Cancel · Compact | `cmd:resume:<id>` posts the root message and binds (as §6), `cmd:cancel:<id>`, `cmd:compact:<id>` |
| `agents` | one `section` per row of `list_agents` (glyph, name, state, detail, age) with a Cancel button while running and an Open-thread link when bound | `cmd:cancel:<id>` |
| `model [id]` | `section` "current: …" + `static_select` of `list_models` grouped by provider (`option_groups`), initial option = current | `cmd:model` (select) → `set_model` on the resolved session, confirmation replaces the panel |
| `compact`, `cancel`, `prompt` | `section` result text (`prompt` in a fence; it is text) | – |
| `skills`, `mcp` | one `section` per entry with `context` (source · count) and an `actions` row: Sync · Add… | `cmd:skills:sync`, `cmd:mcp:sync`; Add opens a **modal** (`views.open` with the click's `trigger_id`) with one `plain_text_input` (URL or JSON), submitted as `view_submission` → the existing add ask |
| `login`, `logout` | one `section` per provider with its state and a Log in / Log out button; `login` replies with a `section` holding the URL as a link button | `cmd:login:<name>`, `cmd:logout:<name>` |
| `approvals` | `section` + `radio_buttons` (ask · off this session · off always) | `cmd:approvals` |
| `slack …` | `status` panel + `context` of allowed users / channels; Allow/Remove keep their text form (ids are pasted, not picked) | – |

Rules: the `text` fallback is one line (`sessions: 4`), never the whole
table; `section.text` ≤ 3 000 and ≤ 50 blocks per message (paginate at 20
entries with a `More…` button `cmd:<panel>:page:<n>`); every `action_id`
is unique within a message (Slack rejects duplicates, jarvis C:5700); a
click by a user who is neither owner nor allowed gets `not allowed` and no
effect; owner-only panels (`approvals`, `slack`) reject non-owner clicks
the same way. Modals need `trigger_id` within 3 s of the click, so the
`views.open` happens in the click handler before any ask.

### 10.3 Manifest and setup

`crates/slack/manifest.yaml` is in git and compiled in (`slack::MANIFEST`);
`wth --slack-manifest` prints it for the paste-into-api.slack.com path.
Contents: `settings.socket_mode_enabled: true`, `interactivity.is_enabled:
true`, `features.agent_view { agent_description, suggested_prompts }` (new
apps must use `agent_view`; prompts are static manifest data),
`features.slash_commands: [/wth]`, `bot_user { display_name: wth,
always_online: true }`, `event_subscriptions.bot_events: [message.im,
app_mention, agent_session_stopped, agent_session_title_changed]`,
`oauth_config.scopes.bot: [chat:write, im:history, im:write,
app_mentions:read, reactions:write, files:read, files:write, commands,
assistant:write]`. The app-level token needs `connections:write`.

**`wth --slack-setup`** drives as much of the rest as Slack's API allows,
one interactive terminal session, no daemon running:

1. Asks for an **app configuration token** (`xoxe.xoxp-…`, from
   api.slack.com/apps → "Your App Configuration Tokens", 12 h lifetime;
   the prompt prints that URL). `apps.manifest.create { manifest }` with it
   → `app_id`, `credentials`, `oauth_authorize_url`. An existing app can be
   updated instead: `--slack-setup --app <A…>` runs `apps.manifest.update`
   (how an installed jarvis-era app is brought to this manifest without
   recreating it).
2. Prints the install URL (`oauth_authorize_url`, or
   `https://api.slack.com/apps/<app_id>/install-on-team` for an update)
   and waits for the user to paste the **bot token** (`xoxb-`) the app page
   shows after install. Verified with `auth.test`; the bot user and team
   are printed.
3. Prints `https://api.slack.com/apps/<app_id>/general` and asks for an
   **app-level token** with `connections:write` (`xapp-`); Slack has no API
   that mints one. Verified with `apps.connections.open`.
4. Asks for the **owner**: an `@handle` or `U…` id; a handle is resolved
   with `users.lookupByEmail`/`users.list` only if `users:read` is granted
   (it is not in the manifest), so the prompt says how to copy the id from
   a Slack profile ("Copy member ID") and accepts `U…` only.
5. Writes `slack_bot` / `slack_app` into the secrets vault (`Vault::open`
   directly, source `slack setup`; 37 §3.6) and
   `slack.owner` / `slack.enabled: true` into `config.yaml`
   (`write_config_value`), prints `run wth --headless and DM the bot`.

Every step is idempotent and re-runnable; a step whose input is already
present (a token in the vault that passes `auth.test`) is
skipped with a line saying so. The Slack CLI (`slack` from slack.dev/cli)
was considered and rejected: it targets its own project layout and calls
the same `apps.manifest.*` methods underneath; a binary dependency for two
HTTP calls.

## 11. State, restart, and what is truth

Truth on disk: `threads.json` — thread → session bindings (+ user, team,
title) and the stream `ts` currently open. On boot every listed stream gets
a best-effort `chat.stopStream`; failures are logged and the list is
cleared (fixes jarvis's dangling "streaming…" bubbles, C:394). Everything
else is derived or ephemeral and rebuilt from events: workers spawn on
first event, boards from `list_agents`, a tool call in flight across a
restart shows its result as a fresh card (its `ToolCallFinished` has no
known task). The dedup set is lost; within Slack's retry window that is at
most three duplicates, which the agent treats as steers. Approval cards
survive because the button value is the id.

Per thread, in memory: the `Worker` (§8). Per process: `Threads`,
`RecentKeys`, the pacer, `auth.test` identity, the owner DM id, the call
counters.

## 12. Config

```yaml
slack:
  enabled: true            # daemon opens the socket when tokens are present
  owner: "U0123456789"     # owner-only commands; may click any approval; receives unbound approvals
  allowed: []              # more user ids; each owns their threads
  channels: []             # channels where @wth is honoured; empty = DMs only
  tools: all               # all | errors | none — task cards
  reasoning: true          # reasoning as a timeline card (stream) or quoted message (update)
  approvals: all           # all | threads — where approvals for non-Slack sessions go (§10.1)
```

Tokens are secrets `slack_bot` (`xoxb-…`) and `slack_app` (`xapp-…`) in the
encrypted vault (37; `wth --slack-setup` writes them, or `wth --secret-set
slack_bot` on a box without a UI) — keys never live in `config.yaml` (01
§1.3). Env overrides as everywhere
(`WTH_SLACK__OWNER`). No flush/pacing knobs: they are Slack's limits, not
preferences.

## 13. Types

```rust
// slack/src/lib.rs
pub const SLACK_ACTOR: &str = "slack";
pub struct SlackPlugin { pub channel: bool }                    // headless → true
pub struct Status;                                              // ask → SlackStatus
pub struct Allow(pub String); pub struct RemoveAllowed(pub String);   // ask → Result<(), String>; config + live
pub struct AllowChannel(pub String); pub struct RemoveChannel(pub String);

#[serde(tag = "type")]
pub enum SlackEvent {
    Connected     { connections: u8 },
    Disconnected  { reason: String },                      // ws end, `disconnect` frame, auth failure; morphism reconnects
    RateLimited   { method: String, retry_after_secs: u64 },
    StreamFallback{ session: SessionId, reason: String },  // a bubble left Stream mode
    Unconfigured,                                          // tokens missing; nothing started
    DaemonOnly,                                            // terminal mode
}
pub struct SlackStatus {
    state: Option<SlackEvent>, bot_user: Option<String>, team: Option<String>,
    owner: String, allowed: Vec<String>, channels: Vec<String>,
    threads: Vec<(String /*channel:ts*/, SessionId)>, open_streams: u32,
    calls: BTreeMap<String, CallStats { ok: u64, err: u64, rate_limited: u64 }>,
    dropped_envelopes: u64,
}

// slack/src/channel.rs
pub struct Handle { chan: Arc<Channel> }
struct Channel { api: Api, rt: Runtime, identity: OnceLock<Identity { bot_user, team, url, owner_dm }>, owner: String,
                 state: Mutex<State { allowed, channels, threads: Threads, workers: HashMap<ThreadKey, mpsc::UnboundedSender<Out>>,
                                      recent: RecentKeys, last_event, dropped }>,
                 port: Arc<dyn Port> }   // 09 §13's trait + cancel, list_sessions, list_agents
// bus subscriptions: AgentEvent, ApprovalRequested, ApprovalResolved
```

Tool `slack_status {}` (`ReadOnly`): the snapshot as text. No setup tool:
setup is the manifest + two tokens, once, in a browser. RPC
(`rpc/src/api.rs`): `slack_status() -> SlackStatus`, `slack_allow(user)`,
`slack_remove_allowed(user)`, `slack_allow_channel(id)`,
`slack_remove_channel(id)`; `SlackEvent` forwarded in `bridge.rs`, both
types in `export.rs`.

## 14. Rejected

- **Hand-rolled Socket Mode / Web API client** (v0 of this doc). §2.1.
- **One message per text segment instead of one bubble per turn** (v0).
  It sidestepped the stream lifetime but threw away the timeline — Slack's
  actual agent UI. Keepalive + rotation is ~60 lines and keeps it.
- **Separate tool messages as the primary representation** (v0). They are
  the update-mode form (§8.2) and the fallback; the native card is primary.
- **Block-gated appends in stream mode** (jarvis `StreamAssembler`,
  F:96-134). Slack renders accumulated markdown server-side; gating only
  delays the reveal. Gating is used where it buys correctness: update-mode
  edits and the rotation cut.
- **Tail reconciliation** (`final_text_tail`, C:5959). Removed by making
  `raw()` the single source and `chat.update` the repair.
- **A shared frontend layer** with Signal. Third copy of ~18 asks; 09 §14
  priced this; a fourth frontend is the trigger.
- **Slash commands per verb**, **short codes**, **reaction approvals**,
  **countdown edits** (§10).
- **Per-user sticky defaults** (jarvis `/effort` per user). `set_model` /
  `set_effort` on the session are the same thing, per thread.
- **Reasoning as a `task_update`** ("thinking…" card). 256 chars; a
  quoted message carries the text.

## 15. Checks

Unit tests in the module they check (`cargo test -p slack`); logic a
plausible bug would break, not wiring (plugins.md §5):

1. **Socket** (`socket::tests`): a `SlackPushEventCallback` with a
   duplicate `event_id` is dropped once, the 1 025th key evicts the first;
   `Unknown` events are counted, not forwarded; `agent_session_stopped`
   reaches `Cancel` for the bound session.
2. **Api** (`wiremock` behind morphism's connector): `ok:false` → exact
   code; two `append_stream` calls from two tasks are ≥ 650 ms apart; other
   methods are not paced; `upload_text` runs the three-step flow and
   returns the permalink.
3. **Bubble** (fake `Api` recording calls): first delta opens with
   `startStream` in chunks mode; tick cadence; `ToolCallPending/Started/
   Finished` become three task chunks with one id, title = described args,
   details = first output line, a long output uploads and appears as a
   source; an `in_progress` task with no append for 20 s re-sends its
   chunk; close appends the tail and stops with the footer; empty turn
   makes no call; `message_not_in_streaming_state` on append → `chat.update`
   with `raw` on the same `ts`, the next event opens a new bubble whose
   first chunks re-emit the in-progress tasks, and the concatenated text
   equals the deltas; a 30 000-char turn splits at a block boundary into
   bubbles with no construct spanning them; update mode never sends past
   `committed()` and renders tasks as separate cards; reasoning posts one
   quoted message edited at block boundaries and `reasoning: off` posts
   nothing.
4. **Routing** (`channel::tests`, stubbed `Port`): top-level DM →
   `SendInput(None)` + bind + 👀; thread reply while running → `Steer`;
   `app_mention` from a non-allowed user → ephemeral, no ask; stranger DM
   dropped silently; `SubagentSpawned` for a bound parent posts the child
   root and binds it; child `TurnStarted` posts the task; child events
   render in the child's thread and update the parent's board row;
   `ApprovalRequested` for a child lands in the child thread, for an
   unbound session in the owner DM (and nowhere with `approvals: threads`);
   a click by the thread user resolves, by a stranger does not;
   `ApprovalResolved` edits the card; `TurnFinished` renames the thread
   once per title and not after `agent_session_title_changed`.
5. **Commands** (`commands::tests`): unique prefixes complete; owner-only
   gating; session resolution picks the invoker's latest thread in the
   channel; `help` lists every sub-command once.
6. **Threads** round-trip incl. `streams` and `title`; corrupt file → empty.
7. **e2e, manual, real workspace** — recorded in the PR: create app from
   `wth --slack-manifest`, tokens in, `wth --headless`; DM "hi" → 👀 →
   Working… → streamed reply → footer → ✅ → thread title appears; a reply
   with a table renders as a table; "run `ls -la` then summarise" with
   `mutating: ask` → card with buttons → Allow → timeline card `shell · ls
   -la` in progress → complete with first line and a snippet link → text
   continues in the same bubble; a `sleep 90` tool keeps the stream alive;
   a 6-minute turn rotates once with the timeline continuing;
   `spawn_subagent` → child thread, board updates, result card in parent;
   typing in the child thread steers it; stop button → 🛑; a terminal
   session's approval shows up in the owner DM and Allow works; `/wth
   sessions`, `/wth resume` in a channel with `@wth`; kill the daemon
   mid-stream, restart → the bubble is stopped, thread still bound, next
   message continues; pause the provider > 30 s mid-text → `StreamFallback`
   and a correct final message.

## 16. Spikes first

Facts the doc assumes and the API docs do not settle. Each is a throwaway
`crates/slack/examples/*.rs` against a Developer Program sandbox, deleted
after (00 §11 pattern); results are folded into §8 before step 3 of §18.

1. `chat.startStream` in chunks mode in a DM of an `agent_view` app on a
   free sandbox: works? If it needs a paid plan, update mode (§8.2) is the
   default and stream mode is opportunistic — same code, different default.
2. Raw token appends: does the client render a half-open fence or table
   acceptably mid-stream, and does the final render match `chat.update`'s
   for the same text? If not, append `raw[..committed()]` plus a
   whitespace-bounded tail (one extra line in `flush`).
3. Measure the idle and wall limits; confirm a repeated identical
   `task_update` resets the idle clock without adding a card; confirm
   `chat.update { markdown_text }` on a dead stream `ts` succeeds and what
   happens to its timeline (kept, dropped, or error → then the repair keeps
   tasks as separate cards, §8.2).
4. `task_update` field limits for `details`, `output` and `sources[].text`
   (title is 256); whether `sources` render for a file permalink.
5. `agents.sessions.setStatus` and `rename` on a non-`agent_view` app: the
   exact error codes (§8.6 latch).
6. `files.completeUploadExternal` with `thread_ts`: appears in the thread,
   not the channel.
7. slack-morphism: upstream `Unknown(Value)` on `SlackEventCallbackBody`
   accepts unmodelled events; the dependency builds against the workspace
   rustls set without a provider panic.

## 17. Open

- **Plan mode.** Needs `AgentEvent::TodoChanged` from the loop (07 §5);
  then `task_display_mode: plan` + `plan_update` chunks mirror the todo
  list. An `agent` change, argued in its own amendment.
- **Feedback buttons.** One `context_actions` block in `stopStream.blocks`
  once something consumes the signal.
- **Slack read tools** (`app_context_changed`, history). A different doc.
- **Two processes, one app.** Slack round-robins envelopes across
  connections; a second daemon on the same tokens is a split brain. The
  guard is documentation.

## 18. Order

Each step leaves `wth` runnable; formatters and the suite once per step.

1. **Spikes** (§16) on a sandbox workspace; fold results into §8.
2. **Dependency + skeleton**: slack-morphism with the §2.1 APIs
   (originally a fork, now upstream Git); `Cargo.toml`, `lib.rs` (config, actor,
   `SlackEvent`, `SlackStatus`, `Unconfigured`/`DaemonOnly`), `api.rs`
   façade + pacer, `socket.rs` listener + dedup; `markdown::Text::committed()`.
   Check: §15 items 1–2; `wth --headless` with tokens logs `Connected`, a DM
   shows up as a debug line, `/wth help` answers.
3. **Threads + inbound + bubble text**: `threads.rs`, `channel::inbound`,
   workers, `bubble.rs` (Stream + Update, rotation, repair), reactions,
   session status, footer. Check: §15 items 3–4 (text half); e2e "hi" →
   streamed reply → ✅.
4. **Task cards, reasoning, titles, approvals (thread + owner DM), other
   events** (§8.3, §8.5–8.7, §10.1). Check: §15 item 3 (tasks, reasoning),
   item 4 (approvals, title); e2e `ls -la` flow, terminal approval in DM.
5. **Subagents**: child threads, task line, board, parent interjects (§9).
   Check: §15 item 4 (child half); e2e spawn flow.
6. **Commands** (§10.2), `slack_status` tool, RPC methods + `bridge.rs` +
   `export.rs`, `--slack-manifest`, boot-time stream cleanup (§11). Check:
   §15 items 5–6; `cargo check -p rpc --features ts`; `just ts` emits the types.
7. **Docs**: README `## Slack` section; 00 §10 line `slack/  plugin: Slack
   channel over Socket Mode (added in 21)`, 00 §13 row `slack → slack crate
   → plugin → 21`, 00 §1.3 patch list gains the morphism fork; this doc to
   as-built with the §15 item 7 record and what was not exercised.

## 19. Status (as-built, 2026-09-14)

Shipped: `crates/slack` (`lib`, `api`, `socket`, `channel`, `bubble`,
`threads`, `commands`, `ui`, `approvals`, `board`, `setup`; `manifest.yaml`),
slack-morphism (originally the four-commit fork; now upstream `master` pinned
at `e2a4b014`, containing PR #388), `markdown::Text::committed()`, `rpc`
methods/bridge/export, `wth --slack-manifest` / `--slack-setup`, README and
00 amendments. Steps 1–7 of §18 done; §16 spikes 1–7 answered (facts in
§8.3–8.5 and `local` notes: stream dies at ~5 min wall, 45 s idle survives,
`chat.update` on a live stream → `streaming_state_conflict`, on a dead one
repairs, `task_update.output` unbounded, `markdown_text` 12 000 enforced,
files v2 into a thread works, `getUploadURLExternal` is form-encoded,
permalinks need `chat.getPermalink` on Enterprise Grid).

Dependency cutover (2026-09-18): exact upstream Git pin, fork patch removed,
glue migrated to the merged typed task/source models and stop-stream content.
Verified: 89 Slack tests, offline connector/request/event smoke and `wth` build.
No live Slack API calls were made for this dependency change.

Exercised: `cargo clippy --workspace -D warnings`, `cargo test --workspace`
(slack: 73 tests — socket dedup, pacer spacing, bubble open/flush/keepalive/
rotation/repair/update-mode, reasoning card and quoted fallback, task cards
and snippet source, routing/trust/steer, approvals in thread and owner DM
with clicks, child threads and board, `/wth` panels and `cmd:*` clicks,
setup create/update/skip/re-prompt, threads round-trip), `cargo check -p rpc
--features ts`; the real workspace: `auth.test`, owner DM, boot-time
stream cleanup, two Socket Mode connections, `slack_status` over RPC; the
user's first pass (streamed reply in a thread, tables, tool timeline cards
with approval buttons, subagent thread + board + result card, `/jarvis`
text replies) reported good, with one finding — mid-turn reasoning sorted
below the bubble — fixed by §8.5's card.

Not exercised end-to-end yet (next user pass): reasoning as a timeline card
with a real provider; the Block Kit panels, overflow/select/radio clicks
and the two modals; `wth --slack-setup --app` against the live app; the
native stop button (needs the two `agent_session_*` events on the app,
§10.3); a > 5 min turn rotating with cards carried over; daemon restart
mid-stream (boot cleanup ran against an empty list only); `update` mode
(every stream in this workspace opened); rate-limit paths (no 429 seen).

Deviations from the draft, recorded: config `reasoning` is a bool, not
`on|off`; `Identity.url` dropped (permalinks fetched); approval cards are
tracked in one Channel-level map; child events fan out to every bound
ancestor so a top-level board shows grandchildren; `Segment` is private to
`bubble.rs`; `SlackApi` gained `respond`, `respond_blocks`, `views_open`;
`Port` gained `resolve`, `list_agents`; `cancel`/`compact` take an explicit
session prefix; `Api::new` installs the aws-lc-rs rustls provider (the §2.1
prediction came true: both `ring` and `aws-lc-rs` link).
