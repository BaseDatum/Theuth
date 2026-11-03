# 110 – Slack under the cognitive-load lens

Status: design · 2026-10-06 · amends 21 §8.3, §8.5, §8.7, §9.2 and 30 §9; follows 70 TOOL-COLLAPSE
and 98 §8.4. Every `file:line` was read on `main` `ef1ad0a3` (2026-10-06). Slack API facts are the
brief's, read from docs.slack.dev on 2026-10-06 (URLs in §2); none has been exercised against a live
workspace yet — §14 does that before code. Where this doc disagrees with 21 about what a Slack thread
shows, this doc wins; 21's transport (streams, pacer, threads, trust) is unchanged.

The lens is 71's (`71-whiteboard-visuals.md:5`): *the thread shows what the person needs to understand
or decide; everything else is one click away or in theuth.*

## 0. The thread

2026-10-05, owner DM: "There were three agents working in the theuth project a little while ago, can
you tell me what happened with them? Did they finish?" The reply thread was ~45 rows. The user: "I
asked a simple question and I have this wall of stuff I have to decipher. This needs to be improved
to match how tool calls are handled and collapsed meaningfully in the desktop/mobile UI… not just how
tool calls render, also how agents communicate in slack… the goal is to reduce cognitive load, to
make life easier, not give the user a full agent trace to decipher for a simple question."

| what you saw | cause | desktop today |
|---|---|---|
| one `task_update` card per tool call (`python · code: <407 chars> ›`, `knowledge_get · id: … ›`, `shell · sqlite3 … ›`) | one `Task` per `call_id`, three chunks per call: `bubble.rs:372-396` (`tool_pending`), `:399-437` (`tool_started`), `:442-508` (`tool_finished`); 21 §8.3 | `foldRuns` folds consecutive calls into one collapsed card headed `5 tools · 2.1 s · 1 failed (edit)` (`ToolRunCard.tsx:27-49`, `:60-77`; 70 "Grouping rule") |
| nine `thinking ›` cards | a `reasoning-<n>` card per run, title `thinking`, never retitled (`bubble.rs:269-297`, `:316-335`); 21 §8.5 | reasoning between calls rides inside the folded run; a finished block reads `Thought 9s` (`MessageReasoning.tsx:11`, 91) |
| `code: <407 chars>` as the whole description of a python call | `describe()` collapses strings over 120 chars to `<n chars>` (`bubble.rs:1113-1122`) | same rule (`lib/describe.ts:31`) — the fix in §4.5 is for every surface |
| `⟵ job j5` / `⟵ job j6`: 60 lines of raw `git log` / `git worktree list` | `MessageDelivered { from, text }` posted whole as its own message (`channel.rs:2334-2341`); 21 §8.7 — a `<job_result>` the *model* needed (`supervisor.rs:2086-2092`) | a `peer` row; still noise, but folded under the transcript's own rules |
| json-render pictures as monospace boxes | `Bubble::out` runs `surface::flatten(…, FLATTEN_WIDTH = 60)` (`bubble.rs:37`, `:146-148`); 30 §9 — and the prompt tells the model "Default to showing" (`surface/src/prompt.rs:175-176`) without ever telling it the reader is in Slack | renders the fence |
| `↳ spawned …` line + a child thread + a board + a result | 21 §9.1–9.2 (`channel.rs:2164-2172`) | board card (71 §2.11) |

The desktop fixed its half on 2026-10-06 (70 TOOL-COLLAPSE, merged as `ef1ad0a3`); Telegram shipped
one activity card per batch from the start (98 §8.4, `telegram/src/cards.rs:1-2`). Slack is the
surface the user actually asked from and the one that never followed.

## 1. Goal, and what is not one

Goal: a Slack thread for a simple question reads as *question → one sentence of intent → one collapsed
activity card → the answer → footer*; tool activity, reasoning and harness plumbing are one click away
or not in the thread at all; the model knows it is writing for Slack and writes Slack-plain; and when
it does draw, the picture arrives as a native block, not a box of ASCII.

Not goals, with the reason:

- **Changing what the model does.** Same tools, same calls, same `<job_result>` injections (14, 47).
  Only how it is *shown* (§4–§6, §8) and what it is *told* (§7) change.
- **Slack as a data source** (21 §1). Unchanged.
- **TUI/desktop changes** beyond the shared `describe` rule for python (§4.5). The desktop is the
  template here, not a target.
- **Telegram/Signal code changes.** Both get the same `describe` rule and their own `[telegram …]` /
  `[signal …]` prompt marker as a parity follow-up *after* `fix/98c-telegram-wedge` lands; Telegram's
  card already folds (98 §8.4), Signal has no cards (09). Named in §16, not built here.
- **Hidden or on-demand child threads.** Sky's ruling 1 (2026-10-06 12:14): a live DM thread per
  child, plus the board and the result card, as today.

## 2. Primitives

| primitive | fact | source |
|---|---|---|
| `chat.startStream` / `appendStream` / `stopStream` chunks | `markdown_text` (12 000 chars per stream, card output counting too — 164); `task_update { id, title ≤ 256, hide_title, icon, status: in_progress \| complete \| error, details ≤ 256, output, sources }` — `output` ≤ 12 000; an identical `id` re-sent replaces title and status in place but **appends** its `output` (164, 21 §8.5 amendment); `plan_update { title }`; **new** `blocks` chunk: ≤ 50 Block Kit blocks appended inline, blocks past 50 dropped with a warning | https://docs.slack.dev/reference/methods/chat.startStream |
| `task_display_mode` | `timeline` (cards interleaved with text) or `plan` (all task updates in one plan block); **locked at `startStream`** | same |
| `blocks` chunk editability | a `blocks` chunk cannot be edited afterwards; only `task_update` ids replace in place; anything that must change later is a task card or a separately `chat.update`d message | same |
| Block Kit, April–June 2026 | `alert` (text ≤ 200; level default \| info \| warning \| error \| success); `card` (title/subtitle ≤ 150, hero_image, icon, child blocks); `carousel`; `data_table` (sortable, paginated, rich cells); `data_visualization` (pie \| bar \| area \| line; ≤ 12 series; every `data_point.label` must match an axis category); `container` (title or `rich_text_title`, subtitle, `child_blocks` ≤ 10, `width`, `is_collapsible`, `default_collapsed`, `has_header_divider`); `plan` (title + `tasks` ≤ 50 task-card objects); `task_card` (task_id, title, status, details/output as rich_text, sources, hide_title); `blocks.validate` for offline-checkable payloads | https://slack.dev/slack-developer-changelog-recap-april-june-2026/ |
| manifest | `features.agent_view` replaces `assistant_view`; ours already uses it (`crates/slack/manifest.yaml:13`). Canvases need `canvases:write` (not in our scopes, `manifest.yaml:29-38`) and `canvases.create`; a canvas is a workspace object, not deleted with the thread | same |
| limits that still hold | `markdown_text` 12 000 per call (`api.rs:17`), a message ≈ 40 000; streams die at ~5 min wall, rotation at 4 min 30 s (`BUBBLE_MAX_AGE`, `bubble.rs:22`); `chat.update` on a live stream → `streaming_state_conflict` (21 §19); Tier 2 limits → the pacer: `APPEND_GAP` 650 ms, `UPDATE_GAP` 1 200 ms (`api.rs:19-22`); worker tick `TICK` 750 ms (`bubble.rs:26`); keepalive re-send of an `in_progress` card after `KEEPALIVE` 25 s (`bubble.rs:24`, `:876-889`) | 21 §4.2, §8.3, §8.4 |
| what exists in `api.rs` | `post_markdown` / `update_markdown` (`:196-208`), `post_blocks` / `update_blocks` taking a `serde_json::Value` array (`:210-222`), `upload_file` / `upload_text` (`:245-267`) | `crates/slack/src/api.rs` |

## 3. Rules

1. **Same fold rule as the desktop.** A batch is 70's run: consecutive tool calls plus the reasoning
   between them, split by assistant text or a deliverable; a lone call stays a plain card; a trailing
   reasoning run stays outside. One predicate, mirrored verbatim from `isDeliverable`
   (`ToolRunCard.tsx:19-22`). When the two disagree, the desktop is right and Slack changes.
2. **Slack's primitive first** (21 line 18). A thing that must change while live is a `task_update`
   id or a `chat.update`d message; a thing that is final is a `blocks` chunk; a thing that is long is a
   file. Never a hand-drawn imitation of a native block.
3. **The thread is for the person.** Model input (`<job_result>`, `<reminder>`, `<job_idle>`) is not
   posted; the model's next sentence carries what matters (ruling 4). Children's results are posted,
   collapsed. Harness bookkeeping (`Compacted`) is a footer item, not a message.
4. **The model writes for the medium.** Every inbound Slack text carries a marker the way voice notes
   do, and the prompt has a `## Slack` section beside `## Voice notes` and `## Live talk`.

## 4. Activity cards (`bubble.rs`)

### 4.1 Batches

A **batch** is design 70's run on the event stream: consecutive `ToolCallPending` / `ToolCallStarted`
/ `ToolCallFinished` for calls of the same turn, plus the `ReasoningDelta` runs between them. It ends
at the first `TextDelta` (assistant prose), `TurnFinished`, or a **deliverable** call. The predicate:

```rust
/// Mirrors `isDeliverable` in `crates/desktop/ui/src/components/chat/ToolRunCard.tsx:19-22`;
/// change both or neither.
fn is_deliverable(name: &str, args: &serde_json::Value) -> bool {
    matches!(name, "show" | "app" | "agent_draft" | "task_draft" | "routine_draft")
        || (name == "doc_create" && args.get("kind").and_then(|k| k.as_str()) == Some("board"))
}
```

(`isBoardCreate`, `BoardCard.tsx:18-20`, is the board half; a `doc_create` of a note is not a
deliverable.) A deliverable renders as today's single card (§8.4 gives it a block in wave 2) and splits
the batch: `[a, b] show [c, d]` → two batch cards around one deliverable card. A batch of one call with
no reasoning renders as today's single card — a header over one line adds a click and hides nothing
(70 "What the user sees", line 15). The arguments of a `ToolCallPending` are not known yet
(`lib.rs:1737-1746`), so the deliverable test runs at `ToolCallStarted`; a pending `doc_create` is
provisionally in the batch and is split out when its args arrive.

A **trailing reasoning run** — one that starts after the last call and is ended by `TextDelta` or
`TurnFinished`, not by another call — stays outside the batch as its own card, as today
(`reasoning-<n>`, `bubble.rs:269-297`). Its title is `thinking` while live and **`Thought N s`** once
finished (Sky's ruling 2026-10-04, as the desktop shows it: `MessageReasoning.tsx:11`), `N` from
`ReasoningDone.ms` (`lib.rs:1726-1734`; published before the chunk that ended it), which Slack ignores
today (91 as-built, line 7). Until the next event arrives the run is provisionally the batch's last
item; if that event is text or the end of the turn, the item is popped and the batch card re-sent
without it. Because a `task_update` id replaces its card, this costs one append and no visible flicker.

### 4.2 One card per batch

| field | while live | when done |
|---|---|---|
| `id` | the first call's `call_id` (stable for the batch's life, the way the desktop keys a run by its first call id, 70 "Files") | same |
| `title` | `` running `shell` · 3 of 5 `` — back-ticked tool name, `n of m` over the calls known so far, exactly `summarize`'s live text (`ToolRunCard.tsx:68`) | `5 tools · 12 s`, `5 tools · 12 s · 1 failed (python)`, `14 tools · 48 s · 2 failed (python, session_read)` — `summarize`'s done text (`:70-76`); duration = last `finished` − first `started`, `x.x s` under a minute, `n min` above |
| `status` | `in_progress` while any call is open | `error` if any call failed, else `complete` |
| `details` (≤ 256) | the last finished call's first result line (the desktop's `hint`, `:66`) | same |
| `output` | the itemised list, §4.3 | same |
| `sources` | ≤ 3 `http(s)` URLs harvested across the batch's outputs so far (`markdown::urls`, the loop at `bubble.rs:465-477`) | same |

The per-call snippet upload for outputs over `TOOL_OUTPUT_CHARS = 2 500` (`bubble.rs:28`, `:478-496`)
is **dropped** (ruling 2): full outputs live in theuth; the itemised line carries the first line. The
`"full output · 12 KB"` fourth source goes with it. `upload_text` stays in `api.rs` for `message_user`.

### 4.3 The itemised list (`output`)

One line per item in event order; a line ≤ 200 chars; the list capped at 60 items, then `… n more`.

```
✓ shell · ps -axo pid,ppid,command · 0.4 s · 12 lines
✓ knowledge_get · id: facts/telegram-rust-bot-library-survey · 0.2 s · 1 line
💭 thought 9 s — Three sessions were bound to worktrees; check whether their branches merged
✗ python · knowledge_get, read_file ×2 · 1.8 s · KeyError: 'body'
⠹ read_file · path: crates/agent/src/loop_.rs · running
```

- `✓`/`✗ <name> · <describe one-line> · <elapsed> · <first result line>`; `<n lines>` when the first
  line is empty or the output is a list of files. `describe` is §4.5, whitespace-joined to one line as
  `tool_started` already does for the title (`bubble.rs:401-407`), cut to fit.
- `💭 thought N s — <first line ≤ 120 chars>` for a reasoning run between calls. The full text is not
  sent to Slack (the `reasoning-<n>` card's `output` today, `bubble.rs:327-334`, carries up to 12 000
  chars); the TUI/desktop have it. `slack.reasoning: false` drops the line (today it drops the card,
  `bubble.rs:262-264`).
- `⠹ <name> · <describe> · running` for an open call; `⏳ <name>` for a pending one.
- **Nested python inner calls** (`call_id` of the form `<parent>.<n>`, `agent/src/store.rs:501-506`)
  are folded into the parent's line, never separate items: the parent's `describe` already names them
  (§4.5) and a failed inner call puts its `✗` on the parent. Slack has no handling for inner ids today
  — they render as flat sibling cards (`Turn.tasks` keys on the raw id, `bubble.rs:207`).

### 4.4 Live refresh

The batch card is **dirty** when an item was added or changed. On the tick (`TICK`, `bubble.rs:26`)
a dirty card is re-sent with the same `id` if the last send is ≥ 1.5 s ago — Telegram's floor
(`EDIT_GAP`, `telegram/src/cards.rs:15`) — and always on batch close; the pacer's `APPEND_GAP`
(`api.rs:20`) still spaces the actual calls. A `running` item's elapsed time is not refreshed on its
own; the keepalive (`bubble.rs:876-889`) re-sends the batch card unchanged after 25 s idle, which is
the only "clock" the live header gets. A 20-call batch costs ~20 appends instead of today's ~60
(21 §4.2 priced 2–3 per call).

`slack.tools` (`lib.rs:57`, `ToolCards` `:79-88`): `all` as above; `errors` emits the batch card
only once a call in it has failed (then with every item, so the failure has its context); `none` emits
nothing. Unchanged meaning, one card instead of many.

### 4.5 `describe()` — the shared rule

The rule is for every surface; this doc states it once and the copies follow:

- `python` → the inner tool names in call order with `×n` counts: `knowledge_get, read_file ×2`.
  Harvested from the `code` argument with one regex over `await <name>(`; no inner call → the first
  `#` comment line; none → `<n lines>`. Never `code: <407 chars>`.
- `shell` → the command, as today (`bubble.rs:1101-1105`).
- everything else → `key: value` per argument, strings over 120 chars collapsed to `<n chars>`, as
  today (`bubble.rs:1113-1122`).

Copies to bring in line, as follow-ups outside this doc's seams: `tui/src/model.rs:1555`,
`telegram/src/cards.rs:362`, `desktop/ui/src/lib/describe.ts:31`.

### 4.6 Update mode, rotation, restart, footer

- **Update mode** (no stream, 21 §8.2; `stream_unsupported`, `bubble.rs:216`): one thread message per
  batch, posted with `post_markdown` on the first item and `update_markdown` in place as the card
  changes — the §4.2 header as a bold first line, the §4.3 list in a fence. Flush rules are Telegram's
  `Card::flush` (`telegram/src/cards.rs:289-333`): send once, edit when dirty and ≥ 1.5 s since the last
  edit, `last` forces the final edit; `UPDATE_GAP` (`api.rs:22`) spaces the calls. This replaces
  `flush_update_tasks`' one-message-per-call (`bubble.rs:931-971`).
- **Rotation** (21 §8.4, `rotate` at `bubble.rs:678-742`): the new bubble's first chunks re-emit
  **only the live batch card** (and a live trailing reasoning card), not every in-progress task
  (`:726-740` today). One chunk instead of n.
- **Restart**: an open batch card is inside an open stream; the boot cleanup `stop_stream`s every
  `threads.json.streams` entry (`channel.rs:582-592`, `threads.rs:214-226`) and the card freezes at its
  last `running …` header, exactly as today's in-progress cards do. §10 says why that is accepted in v1.
- **Footer** unchanged: `model · N in / M out · time · n tools` (`Footer`, `bubble.rs:87-103`); `tools`
  counts calls, not batches.

### 4.7 Before / after

Before (abridged; the real thread had ~45 rows):

```
you      There were three agents working in the theuth project a little while ago, …
theuth   thinking ›
         shell · ps -axo pid,ppid,command ›
         thinking ›
         python · code: <407 chars> ›
         knowledge_get · id: facts/… ›
         thinking ›
         shell · sqlite3 ~/.theuth/sessions.sqlite "select …" ›
         … (31 more cards)
         ⟵ job j5
         commit a1b2c3d (HEAD -> fix/98c-telegram-wedge) … (28 lines of git log)
         ⟵ job j6
         /Users/…/theuth-110-design  ef1ad0a3 [docs/110-slack-cognitive-load] … (32 lines)
         Three agents ran … (the answer)
         claude-sonnet-4-5 · 41.2k in / 2.1k out · 3 min · 14 tools
```

After:

```
you      There were three agents working in the theuth project a little while ago, …
theuth   Checking the three worktrees and their sessions.
         ▸ 14 tools · 48 s · 2 failed (python, session_read)
         All three finished; two merged, one is waiting on you.

         | agent            | branch                    | state                       |
         |------------------|---------------------------|-----------------------------|
         | ScoutDesktop     | fix/98c-telegram-wedge    | merged 14:02                |
         | RenderWorker     | feat/98-render            | merged 14:20                |
         | IntegrateWorker  | feat/98-integrate         | PR open, 1 failing test     |

         The failing test is `cards::flush_floor` — a 1.5 s timing assertion; IntegrateWorker left a
         note in its thread. Want me to re-run it?
         claude-sonnet-4-5 · 41.2k in / 2.1k out · 3 min · 14 tools
```

Expanding `▸` shows the 14 itemised lines of §4.3. The `⟵ job` messages are gone (§6); the model's
first sentence says what it is doing (§7); the `thinking` cards became two `💭` lines inside the card.

## 5. Plan message (new)

`AgentEvent::TodoChanged { session, snapshot }` exists (`agent/src/lib.rs:2008-2013`), published from
`loop_.rs:876-882` (reload after undo/redo), `:4372-4375` (a `todo` tool call committed a changed
list) and `:4950-4953` (compaction cleared it), plus `acp_child.rs` for ACP plan updates. The snapshot
(`agent/src/todo.rs:137-145`) is `{ version: u64 /* monotonic, 0 = never had a plan */, source:
TodoSource, items: Vec<Todo> }`; a `Todo` (`:25-33`) is `{ text, status, reason: Option<String> /*
Some iff Blocked */ }`; `Status` (`:49-60`) is `pending | in_progress | done | dropped | blocked`.
Slack ignores the event today with the comment "plan state for persistent UIs — a thread is not one"
(`channel.rs:2351-2352`, `:2374`). This doc disagrees: a thread *is* where the person waits.

While a turn has a non-empty todo list, the thread holds **one plan message**:

- Posted with `post_blocks` on the first `TodoChanged` of the turn whose `items` is non-empty:
  blocks = one `plan` block, `title` = the `in_progress` item's text, else `Plan · n of m done`;
  `tasks` = one task-card object per item, `task_id` = the item's index in the snapshot, `title` =
  `text`, `status` = `done → complete`, `in_progress → in_progress`, `blocked → error` with `reason` as
  `details`, `pending → pending`; `dropped` items omitted. Over 50 items: the first 49 and a last task
  `… n more`.
- `update_blocks` on later snapshots (apply only a higher `version`, `lib.rs:2005-2006`), coalesced by
  the tick ≥ 2 s apart like the board (`BOARD_MIN_GAP`, `board.rs:14`; the gate at
  `channel.rs:2024-2032`). `TurnFinished` forces the last edit. The final edit stays as history.
- It is a **separate message**, not a chunk: a `blocks` chunk inside a stream cannot be edited (§2),
  and `task_display_mode: plan` is locked at `startStream` and would mix the todo with the tool trace
  (§12).

`slack.plan: true | false`, default `true` (§9). A child thread gets its own plan message the same way
(nothing is special-cased by depth, 21 §9.1).

## 6. Quiet thread (`channel.rs`)

What arrives mid-turn, what the worker does today, what it does now:

| event | today | new |
|---|---|---|
| `MessageDelivered { from, text }` (`lib.rs:1857-1865`) — fired only by `Supervisor::inject` (`supervisor.rs:1114-1143`) for a finished job (`from = "job {id}"`, `:2091`), a quiet PTY job (`:2205`), a reminder (`"reminder {id}"`, `:2118`), and by the two `from: "you"` sites where the human answered an awaited peer message (`:4358`, `:4423`) | `⟵ **from**` + the whole body as one message (`channel.rs:2334-2341`) | **nothing posted** (ruling 4). No discriminator is needed: after 50 §10 this event carries only model input and the human's own echo (`communication.rs:641-645`; `schedule/src/lib.rs:1931-1932`), so the handler drops every one and bumps `SlackStatus.hidden_deliveries` (new counter beside `dropped_envelopes`, `lib.rs:170`). The `from` prefixes `job ` / `reminder ` stay in the log line. |
| `PeerDelivered { session, origin, text, seq, turn }` (`communication.rs:649-661`) — a child's or peer's result reaching this session | **not rendered**: Slack subscribes to `AgentEvent`, approvals, asks, artifacts, secrets and mesh only (`channel.rs:509-516`); 21 §8.7's "`⟵ grep-logs` card" stopped firing when peer bodies moved off `MessageDelivered` | subscribe; post via `post_blocks` one `container` block: `is_collapsible: true`, `default_collapsed: true`, `title` = `↳ grep-logs · finished · 2 min` (name and age from the board row whose `session` matches `origin.from = "session:<uuid>"`, `board.rs:70-85`; a peer with no row → `↳ peer:<name> · delivered`), `subtitle` = the body's first line, one `markdown` child block holding the body ≤ 12 000 chars, clipped with a count (`clip`, `channel.rs:2528`). Rotation-safe because it is its own message. |
| child `Error` / cancelled `TurnFinished` (depth 1; `channel.rs:2185-2195`) | `↳ **name** error: <line>` / `↳ **name** cancelled` as markdown | one `alert` block, level `error`, text ≤ 200: `grep-logs · error: <first line>` / `grep-logs · cancelled` |
| `Error { message }` (`channel.rs:2329-2333`) | `⚠️ <message>` | one `alert` block, level `error`, first line ≤ 200; the full message in the log. The ❌ reaction stays. |
| `Compacted { epoch, folded }` (`channel.rs:2342-2347`) | `⟲ compacted → epoch n (m messages folded)` as a message | no message; a `⟲ epoch n` item appended to the **next bubble's footer** (`Footer` gains `compacted: Option<u32>`, `bubble.rs:73-85`) |
| `SubagentSpawned` (router `Channel::spawned`, `channel.rs:1763-1794`; worker `spawned`, `:2164-2172`) | child root message + binding + `↳ spawned **name** → <link|thread>` line + board row | **as today** (ruling 1) minus the `↳ spawned` line: the board row (`:2169-2171`) and the child root's `child of <link|this thread>` (`:1769`) already carry it |
| board (21 §9.2; `render_board`, `channel.rs:2037-2058`) | one message, `chat.update`d ≥ 2 s apart | unchanged |
| approvals, `ask_user`, `secret_request` | 21 §10.1 | unchanged |

Schedule and routine fires never pass through `MessageDelivered`: `schedule::deliver`
(`schedule/src/lib.rs:1405-1421`) builds an `Outbound { origin: "schedule <name>" }` for
`UserChannels`, i.e. a `message_user`-class delivery the person is meant to read. Nothing to hide
there; it is already the human-facing form.

## 7. Writing for Slack

### 7.1 The inbound marker

Voice notes already tell the model where a message came from: `[voice note, N s] …` (`voice::note_text`,
used at `channel.rs:1459`), and live talk uses `[spoken, N s]` (`talk/src/policy.rs:583-585`). Slack
text has no marker, so the model writes for the terminal.

Grammar: exactly one marker line prepended to the typed text, `[slack dm]` for a direct message,
`[slack channel]` for an `app_mention` in a channel. The channel *name* is not available — `Msg`
carries the id only (`channel.rs:1897-1905`) and the manifest has no `channels:read` for
`conversations.info` — so `[slack #<name>]` waits for that scope (§16). DM-ness is known only at
dispatch (`channel_type == "im"`, `channel.rs:1167`; mentions at `:1189-1208`), so `Msg` gains `dm:
bool`, set at the two construction sites (`:1178-1186`, `:1198-1207`).

Where: in `handle_message` after `roster::routing::route` has consumed any `@agent` prefix
(`channel.rs:1246-1251`) and before `lines.push(text)` (`:1252-1254`): `lines.push(format!("[slack dm]
{text}"))`. Nothing else changes — the empty-input return (`:1257-1259`) still runs on the unmarked text,
`strip_mentions` (`:2509`) has already run, `/theuth` commands never pass through `handle_message`
(`Channel::command`, `:749-751`), and steer-vs-send is decided by the session's running state
(`:1287-1298`), not by the text. The marker rides into `steer`, `send_input` and `to_agent` alike.

A voice note carries **no Slack marker**: the desktop's `parseNote` (`lib/voice-note.ts:9-12`) and the
session title strip (`SessionItem.tsx:172-173`) anchor `[voice note, N s]` at the start of the line, and
the voice prompt already forbids everything the Slack section would (no markdown, no pictures;
`voice/src/lib.rs:144-153`). The brief's combined `[slack dm, voice note, N s]` would break both
consumers for one sentence of benefit; recorded as a deviation.

### 7.2 The prompt section

Prompt sections are plugin-owned constants registered on the tool registry: `voice` registers `##
Voice notes` (`voice/src/lib.rs:143-153`, `RegisterPrompt { plugin: "voice" }` at `:949-954`), `talk`
registers `## Live talk` (`talk/src/lib.rs:398-416`, `:1240-1245`), the agent registers its own and
`surface::prompt_block()` (`supervisor.rs:7546-7575`). `loop_::prompt_blocks` (`loop_.rs:2515-2531`)
joins whatever `ListPrompts` returns, filtered by toolset and attendance. So the Slack section lives in
**`crates/slack/src/prompt.rs`** as `pub const PROMPT: &str`, registered once in `SlackPlugin::start`
beside `RegisterTool(SlackStatusTool)` (`slack/src/lib.rs:408-416`) as `RegisterPrompt { plugin:
"slack", text: prompt::PROMPT.to_owned() }` — not in `crates/agent`, which owns no channel's voice.
Unattended sessions keep it (`prompt_blocks` strips only `surface`/`app`), which is right: a subagent
whose result lands in a Slack thread also writes for Slack.

The text, verbatim (wave 1):

```
## Slack
A user message that starts with `[slack dm]` or `[slack channel]` was typed in Slack, and your reply
renders there as a Slack message in the same thread. Lead with the answer. One screen — about twelve
lines — unless the user asks for more. Write the markdown Slack renders: bold, bullets, numbered lists,
tables of up to six columns, fenced code, links. Draw nothing Slack cannot show: no mermaid, no openui,
and no json-render except Alert, Callout, Table, Bars, Sparkline, Metric, MetricGrid, KeyValueList and
Checklist — and until Slack shows those natively they arrive as a monospace box, so prefer a markdown
table. Anything longer or richer than one screen goes through `show`; it arrives as a file the user
can preview. Your tool activity is one collapsed card the user can expand, so the sentence before a
batch of tool calls says what you are doing and why in one line, and you never list what each call
did. Subagents appear to the user as their own threads and a live board; do not narrate their
progress. `message_user` reaches other channels; to answer here, just reply. Keep this style while
messages carry the `[slack …]` marker; go back to your normal style when they stop.
```

Wave 2 (§8, W5) replaces the one clause "and until Slack shows those natively they arrive as a monospace
box, so prefer a markdown table" with "which Slack shows as native blocks". The section is one
paragraph after its heading, like `## Voice notes`; it is registered whenever the plugin starts with
tokens present, like `voice`'s. Telegram and Signal get the same shape with `[telegram …]` /
`[signal …]` markers as the §1 follow-up.

## 8. Pictures that render (wave 2: `surface` + `bubble.rs`)

`surface::slack_blocks(fence_body: &str) -> Result<Vec<serde_json::Value>, Error>` in a new
`crates/surface/src/slack.rs`, re-exported beside `flatten` (`surface/src/lib.rs:27`), over the parsed
`Spec` (`parse`, the same entry `flatten` uses, `flatten.rs:13-16`). `Component` is the 38-variant enum
in `spec.rs` (30 §10 as amended by 71 §6.4); the mapping:

| json-render | Block Kit |
|---|---|
| Alert, Callout | `alert`, level by variant (`info \| warning \| error \| success`, else `default`) |
| Table ≤ 15 rows | `data_table`; over 15 rows (or any cell over the cell cap) → `rich_text_preformatted` of the flattened table |
| Bars, Sparkline | `data_visualization` (`bar` / `line`; ≤ 12 series; every point label must be an axis category — the renderer builds the axis from the labels, so it holds by construction) |
| Metric, MetricGrid, KeyValueList | `section` with `fields` (≤ 10 fields; more → two sections) |
| Checklist, Steps, Timeline, Tags, Glossary | `rich_text` lists (`✓`/`○` glyphs for Checklist) and a `context` line for Tags |
| Card, Accordion | `container` (`is_collapsible` for Accordion, `default_collapsed: true`), children mapped recursively, ≤ 10 child blocks |
| Heading, Separator, Image, LinkList, Code, Quote | `header`, `divider`, `image` (http(s) only), `rich_text` links, `rich_text_preformatted`, `rich_text_quote` |
| Graph, Sequence, Heatmap, Kanban, Calendar, Gauge, Excerpt, Progress, Tree, Diff, Person, FileList, Stack, Grid, Tabs, Text, Badge; mermaid; openui | today's flattening (`surface::flatten`, `flatten.rs:10`); v1 later: PNG via the renderer, 30 §9's deferred path |

Integration (`bubble.rs`): the bubble already withholds text while a `json-render` fence is open
(`Bubble::streamable`, `bubble.rs:150-166`) and renders the closed fence through `Bubble::out`
(`:146-148`). In stream mode `out` now splits at the fence: text before it as `Chunk::Text`, the fence as
a `Chunk::Blocks(Vec<Value>)` (new variant, `api.rs` `Chunk`), text after as `Chunk::Text` — one
`appendStream` with up to three chunks. ≤ 50 blocks per chunk; a fence that maps to more, or whose
`slack_blocks` fails, falls back to flattening, byte-for-byte today's behaviour. In update mode the
blocks are a separate `post_blocks` message after the bubble's edit (a `markdown_text` message cannot
carry blocks mid-text). Rotation: a fence never spans a cut (21 §8.4 trigger 1 already cuts at block
boundaries). `slack.blocks: false` keeps flattening everywhere.

Validation: each `valid-*.json` fixture of the mapped components (`crates/surface/fixtures/json-render/valid-*.json`, 30 §10) is
run through `blocks.validate` once in a spike (§14.5), the accepted payload recorded as
`crates/surface/fixtures/slack/<name>.blocks.json` goldens, and `cargo test -p surface` compares offline from then on
(`UPDATE_GOLDEN=1` regenerates, the 30 §10 convention).

Deliverables (the §4.1 split cards):

- `show` stays a file upload with the title as the comment (`Worker::artifact`,
  `channel.rs:2086-2120`): Slack previews markdown, PDF and images.
- `app` → a `card` block (title, subtitle `interactive in theuth`), **no `.openui` source upload**
  (today `APP_NOTE` + the source file, `:2100-2102`).
- board create (`doc_create kind: board`) → a `card` block with the `theuth://notes/<id>` link the tool
  result carries (`document/src/tools.rs:869-874`) — the scheme exists in the result, not yet as a
  desktop handler (§16). **Optional**, behind the new `canvases:write` scope: a canvas built from
  `doc_export` markdown via `canvases.create`, linked from the card. It needs the user to reinstall the
  app with the new manifest, and the canvas outlives the thread; off unless `slack.canvases: true`.

Security: 30 §11 applies — `Image.src` and `LinkList.url` are already validated to `http(s)://`
(`30-surfaces.md:718-719`); `slack_blocks` emits nothing else as a URL and no `data:` images (Slack
fetches `image` blocks itself).

## 9. Config

`SlackConfig` (`crates/slack/src/lib.rs:43-76`), `[slack]` section:

```yaml
slack:
  tools: all            # all | errors | none — one activity card per batch (§4.4)       (exists)
  reasoning: true       # 💭 lines inside the batch card; trailing runs as a card (§4.3)  (exists)
  plan: true            # the plan message (§5)                                             new
  deliveries: hidden    # MessageDelivered: `hidden` is the only v1 value (§6);
                        # `collapsed` (a default_collapsed container) is the documented later value
  blocks: true          # wave 2: json-render → Block Kit (§8); false = flatten as today      new
  canvases: false       # wave 2, optional: board create also makes a canvas (§8)             new
```

Defaults in `impl Default for SlackConfig` (`lib.rs:64-76`). `deliveries` is an enum with one variant
so the key exists before its second value does. No pacing knobs (21 §12).

## 10. State and restart

`threads.json` (`threads.rs:70-77`) is `{ threads, streams: [{ channel, ts }] }`; boot drains
`streams` and `stop_stream`s each (`channel.rs:582-592`). Two new live objects:

- **Open batch card.** It is a `task_update` inside a stream; the boot cleanup already stops the
  stream and the card freezes at its last header. To finish the card honestly the cleanup would need
  its `id` — `streams[].cards: Vec<String>` and an `appendStream { task_update { id, status: error,
  title: "interrupted by restart" } }` before `stop_stream`. **Not in v1**: it is a rare path, today's
  in-progress cards freeze the same way, and the next turn is a new bubble. `ponytail:` known ceiling;
  add the `cards` field when a frozen `running` header is reported.
- **Plan message.** Never in a stream: `chat.update`d in place, final edit on `TurnFinished`. A crash
  mid-turn leaves the last edit as history — the same as a finished turn's — and the next
  `TodoChanged` posts a fresh message (the `Plan` is in memory, §11). Nothing to persist.

Everything else as 21 §11: workers, batches, boards and the dedup set are process-local and rebuilt
from events.

## 11. Types

```rust
// slack/src/bubble.rs
struct Batch {
    id: String,                 // first call id; the task_update id
    items: Vec<Item>,           // event order; ≤ 60 rendered, then `… n more`
    started: Instant,           // first ToolCallStarted
    finished: Option<Instant>,  // last ToolCallFinished when no call is open
    dirty: bool,
    last_sent: Option<Instant>, // ≥ 1.5 s between re-sends; None = never sent
    card_ts: Option<String>,    // update mode: the batch message
    sources: Vec<Source>,       // ≤ 3 http(s) across the batch
}
enum Item {
    Call {
        call_id: String, name: String, described: String,
        started: Option<Instant>, finished: Option<Instant>,
        error: bool, first_line: String, lines: u32,
        inner: Vec<String>,     // python inner tool names, in call order (×n folded at render)
    },
    Thought { ms: Option<u64>, first_line: String },   // between calls; trailing runs are not Items
}
// Turn gains: batch: Option<Batch>; `tasks: HashMap<call_id, Task>` keeps only deliverables,
// lone calls and `reasoning-<n>` cards.

// slack/src/plan.rs
pub struct Plan { ts: Option<String>, version: u64, snapshot: agent::todo::TodoSnapshot,
                  dirty: bool, last_edit: Option<Instant> }
pub fn blocks(snapshot: &TodoSnapshot) -> serde_json::Value;   // one `plan` block, §5

// slack/src/api.rs
pub enum Chunk { Text(String), Task { … }, Blocks(Vec<serde_json::Value>) }   // wave 2

// slack/src/lib.rs
pub enum Deliveries { Hidden }                       // `collapsed` later
// SlackConfig += plan: bool, deliveries: Deliveries, blocks: bool, canvases: bool
// SlackStatus += hidden_deliveries: u64

// surface/src/slack.rs (wave 2)
pub fn slack_blocks(fence_body: &str) -> Result<Vec<serde_json::Value>, Error>;
```

## 12. Rejected

- **`task_display_mode: plan` for tool calls.** Locked at `startStream` (§2), so a turn cannot choose
  per batch; it renders *every* task update in one plan block, mixing the todo with the trace; ≤ 50
  tasks. The plan block is for the plan (§5); the timeline is for the trace.
- **A `blocks` `container` per batch.** A `blocks` chunk cannot be edited (§2); a batch changes every
  few seconds. The `task_update` id is the only in-place primitive a stream has.
- **Child threads on demand / never.** Sky's ruling 1. The board row and the collapsed result already
  make the parent thread quiet; the child thread is where the person steers the child (21 §9.1).
- **Posting `<job_result>` / `<reminder>` collapsed.** Ruling 4: they are model input. Collapsed is
  still a row the person must decide to ignore; the model's next sentence is the human-facing form.
  `deliveries: collapsed` stays a documented later value for the day someone wants the audit trail.
- **Per-call snippet uploads.** Ruling 2: outputs live in theuth; three upload calls per long output
  (21 §4.2) bought a link nobody clicks from a phone.
- **One bubble per batch** (close the stream at each batch like Telegram's card, 98 §8.4). Slack's
  stream *has* a timeline; a card inside it is cheaper than a stop/start pair on the Tier 2 budget and
  keeps the prose in one message.
- **A `kind` field on `MessageDelivered`.** Not needed: after 50 §10 the event carries only model
  input and the human's own echo (§6); every one is hidden.
- **The prompt section in `crates/agent`.** Every channel owns its voice as a `RegisterPrompt`
  (`voice`, `talk`); the agent crate would otherwise learn about Slack.

## 13. Checks

Unit tests in the module they check; logic a plausible bug would break, not wiring (21 §15 shape).
Existing tests that pin the per-call cards (`tool_lifecycle_emits_three_task_chunks`,
`long_output_is_uploaded_and_linked`, `reasoning_is_a_growing_card_completed_by_text`,
`bubble.rs:1526`, `:1569`, `:1861`) are replaced, not kept beside the new ones.

1. **Batches** (`bubble::tests`, `FakeApi` as today, `bubble.rs:1220`):
   `two_calls_fold_into_one_card_with_the_first_call_id`; `text_between_calls_splits_batches`;
   `deliverable_show_splits_a_batch_into_two_cards_around_it`;
   `pending_doc_create_of_a_board_is_split_out_when_args_arrive`;
   `lone_call_renders_as_a_plain_card`; `reasoning_between_calls_is_a_thought_line_inside_the_card`;
   `trailing_reasoning_is_popped_and_becomes_a_thought_card_titled_with_its_seconds`;
   `live_title_counts_running_n_of_m_with_backticked_name`;
   `done_title_names_failed_tools_in_order`; `durations_format_seconds_then_minutes`;
   `card_is_not_resent_within_the_floor_but_always_on_close`;
   `details_is_the_last_finished_first_line_and_sources_cap_at_three`;
   `python_inner_calls_fold_into_the_parent_line`; `list_caps_at_sixty_items_with_a_count`;
   `update_mode_posts_one_message_per_batch_and_edits_it`;
   `rotation_reemits_only_the_live_batch_card`; `tools_errors_emits_the_card_only_after_a_failure`;
   `reasoning_off_drops_thought_lines`; `describe_python_lists_inner_tools_with_counts`
   (`await knowledge_get(`, `await read_file(` ×2 → `knowledge_get, read_file ×2`; comment fallback;
   `<n lines>` fallback); `no_snippet_upload_for_long_outputs`.
2. **Quiet thread** (`channel::tests`, stubbed `Port`): `message_delivered_posts_nothing_and_counts`
   (job, reminder and `you`); `peer_delivered_from_a_child_posts_a_collapsed_container_titled_from_the_board_row`;
   `peer_delivered_body_is_clipped_with_a_count`; `child_error_is_an_alert_block`;
   `error_is_an_alert_block_and_reacts`; `compacted_posts_nothing_and_lands_in_the_next_footer`;
   `subagent_spawned_posts_the_child_root_and_no_spawned_line` (adapt
   `subagent_spawned_posts_the_child_root_and_binds_it`, `channel.rs:3612`).
3. **Plan** (`plan::tests`): `first_nonempty_snapshot_posts_a_plan_block`;
   `title_is_the_in_progress_item_else_progress`; `blocked_maps_to_error_with_reason_dropped_omitted`;
   `lower_version_is_ignored`; `edits_coalesce_at_two_seconds_and_turn_finished_forces_the_last`;
   `plan_false_posts_nothing`; `over_fifty_items_truncates_with_a_count`.
4. **Marker and prompt** (`channel::tests`, `prompt::tests`): `dm_text_is_prefixed_with_the_slack_dm_marker`;
   `mention_text_is_prefixed_with_the_channel_marker_after_agent_routing`;
   `voice_note_lines_carry_no_slack_marker`; `empty_text_with_no_files_still_posts_nothing`;
   `prompt_section_starts_with_the_heading_and_names_every_native_block` (and the registration
   happens in `start` — checked by the existing plugin start test if one exists, else by hand in §14).
5. **Blocks** (`surface::tests`, wave 2): one golden per mapped component
   (`fixtures/slack/*.blocks.json`); `table_over_fifteen_rows_falls_back_to_preformatted`;
   `unmapped_component_is_an_error_so_the_caller_flattens`; `over_fifty_blocks_is_an_error`;
   `container_children_cap_at_ten`; `urls_are_http_only`. In `bubble::tests`:
   `closed_fence_streams_as_text_blocks_text_in_one_append`; `failed_mapping_flattens_as_before`;
   `update_mode_posts_blocks_as_a_separate_message`.
6. **Manual, real workspace** (recorded in the PR): the §0 question again → one narration sentence,
   one `▸ n tools` card that expands to itemised lines, no `⟵ job` rows, the answer, the footer; a
   `spawn_subagent` turn → child thread, board, collapsed `↳ name · finished` container in the parent;
   a `todo` turn → plan message updating in place and left as history; a reply with an Alert and a
   Table → native blocks (wave 2); `/theuth slack status` shows `hidden deliveries: n`.

## 14. Spikes first

Ten-minute live spikes the user runs against the real app (21 §16 pattern: a throwaway
`crates/slack/examples/*.rs`, deleted after); results fold into §4–§8 before wave 1 starts.

1. **`blocks` chunk inside a live stream**: `startStream` in timeline mode, append `markdown_text`,
   then a `blocks` chunk with one `alert` and one `data_table`, then more text — does the order hold,
   does the stream stay streamable, is a 51-block chunk really "dropped with a warning" and not an error?
2. **`task_update` replace cadence**: re-send one id every 1.5 s for two minutes with a growing
   `output` (60 lines) under Tier 2 — any `ratelimited`, any card duplication, does `details` update
   with `status: in_progress`?
3. **`plan` block in a DM thread** via `chat.postMessage` + `chat.update` every 2 s: renders, updates
   in place, `pending` accepted as a task status, 50-task cap behaviour.
4. **`container default_collapsed` in a thread** via `chat.postMessage` with a 12 000-char `markdown`
   child: collapsed on arrival on desktop and phone, expands, `has_header_divider` look.
5. **`blocks.validate`** against the §8 fixtures (wave 2 gate): which payloads it rejects, so the
   goldens are recorded from accepted ones.

## 15. Order

Wave 1 runs in parallel with strict seam ownership; each worker writes `docs/design/110-as-built-<W>.md`.

| worker | owns | does not touch |
|---|---|---|
| **W1 ACTIVITY-CARDS** | `crates/slack/src/bubble.rs` (+ its tests): §4 batches, card, itemised list, floor, `describe` for python, update mode, rotation, `Thought N s`, `Footer.compacted` field | `channel.rs`, `lib.rs`, `api.rs` (if a `Chunk` change is needed, message W2) |
| **W2 QUIET-THREAD** | `crates/slack/src/channel.rs` (§6 handlers, `PeerDelivered` subscription, `Msg.dm`, the §7.1 marker, `Footer.compacted` hand-off), `board.rs` (row lookup by session), new `plan.rs` (§5), `lib.rs` (§9 config, `hidden_deliveries`, `mod prompt;` + the `RegisterPrompt` call), `api.rs` if W1 asks | `bubble.rs`, `prompt.rs` body |
| **W3 SLACK-VOICE** | new `crates/slack/src/prompt.rs` (`pub const PROMPT`, §7.2 text verbatim, its tests) | everything else in `crates/slack`; nothing in `crates/agent` — the registration line is W2's by contract: `RegisterPrompt { plugin: "slack", text: prompt::PROMPT.to_owned() }` |
| **W4 SLACK-BLOCKS-RENDER** | new `crates/surface/src/slack.rs` + `fixtures/slack/*.blocks.json` goldens + the `pub use` in `surface/src/lib.rs` (§8 renderer only) | anything in `crates/slack` |

Wave 2, after wave 1 lands and spike §14.5 is recorded: **W5 SLACK-BLOCKS-INTEGRATE** — one worker —
`Chunk::Blocks` in `api.rs`, the `out` split in `bubble.rs`, the update-mode `post_blocks`, the
deliverable `card` blocks in `channel.rs`, `slack.blocks` / `slack.canvases`, the §7.2 clause swap in
`prompt.rs`, the manifest scope (optional canvases).

Gates, once per worker at the end (not mid-flight): for `slack` — `cargo check -p slack --all-targets`,
`cargo test -p slack`, `cargo clippy -p slack --all-targets -- -D warnings`, `cargo fmt -p slack --
--check`; W4 the same four with `-p surface`; W2 also `cargo check -p rpc --features ts` because
`SlackStatus` is exported (`lib.rs:149`) and `just ts` regenerates the binding. No worker runs the
workspace suite; the integrator does, once.

## 16. Open

- **`theuth://` as a desktop deep link.** The scheme exists only inside tool results
  (`document/src/tools.rs:870`); the desktop initialises `tauri-plugin-deep-link`
  (`desktop/src/lib.rs:74`) but registers no scheme and no `onOpenUrl` consumer, so a board row's or
  result card's `theuth://sessions/<id>` link would open nothing today. Needs its own note before §8's
  card links are clickable outside Slack.
- **Canvases** (§8, optional): scope, reinstall, lifetime outside the thread; worth it only if someone
  reads boards in Slack rather than in theuth.
- **Telegram/Signal parity**: the §4.5 `describe` rule in `telegram/src/cards.rs:362` and
  `tui/src/model.rs:1555`; `[telegram …]` / `[signal …]` markers and their `## Telegram` / `## Signal`
  sections; Telegram's `MessageDelivered` handler (`telegram/src/worker.rs:550`) going quiet the same
  way. After `fix/98c-telegram-wedge`.
- **PNG for Graph / Sequence / mermaid** (30 §9 v1): the `merman` raster path; Slack shows a picture
  where it now shows source.
- **`[slack #<name>]`**: `channels:read` + `conversations.info`, cached per channel id; a reinstall.
