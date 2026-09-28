# 74 — Time: a clock for the model, timestamps for the human

Status: built 2026-09-28. Amends 07 §2 rule 4 (§4 below); extends 14 (`<job_result>`, `<reminder>`), 50 §9 (`<message>`), 61 (`<job_idle>`), 48 §3.2 (`ContextEntry`). Code: `crates/agent/src/clock.rs`, seams in `loop_.rs`, `store.rs`, `supervisor.rs`; UI in `crates/desktop/ui/src/lib/time.ts`, `components/chat/Stamp.tsx`, `Transcript.tsx`.

## 1. Problem

The model had no clock. The baseline carries `Date: YYYY-MM-DD` (frame.rs) and nothing else; 07 rule 4 kept every finer clock out of the frame so the prefix stays cacheable. So the model could not tell whether the user answered in ten seconds or came back the next morning, how long a `python` cell or a job actually ran, or how stale a subagent's result was when it finally arrived. It reasons about "now" from the date and guesses the rest.

The human had the mirror problem: `messages` rows carried no time at all, so the transcript could not say when anything was said.

## 2. Evidence

- Cheng et al. 2025, *Your LLM Agents are Temporally Blind* (arXiv 2510.23853): agents that see no wall clock mis-plan waits and retries; injecting the current time *and the elapsed gap* since the last observation, at a bounded cadence, recovers most of the loss. The gap matters more than the absolute time.
- Claude Code issue #24182 and the community time-awareness hooks built around it: a hook appends a stamp to the user's message — a full stamp on the first emit of each hour, a short one after > 5 min of silence, silence otherwise. Rate-limited, tail-appended, never in the system prompt.
- 07 §1 (jarvis): a per-turn `core.time` minute clock in a `messages[1]` System row busted the prefix cache on every turn and replayed every old delta into history. The lesson was never "no clock"; it was "no clock in the prefix".

## 3. The stamp (engine)

One line, prepended to the text of an input the model is about to see, followed by a newline:

```
<time now="2026-09-28 14:03 -07:00 (Mon)" since_last="1h 12m"/>
```

- `now`: local wall clock with numeric offset and weekday. The offset, not a zone abbreviation: chrono has no abbreviation table without a tz database, `-07:00` is unambiguous where `CST` is not, and `clock::parse` reads it back on resume.
- `since_last`: elapsed since the previous stamp the model saw in this session, humanised `NNs` / `Nm` / `Nh Nm` / `Nd Nh`. Omitted on the first stamp of a session.
- Rate limit: `clock::TIME_STAMP_GAP` = 5 min. A stamp is written when there is none yet this frame or at least the gap has passed since the last one. Otherwise nothing.

Where (`clock::prepend_due`):

| input | seam |
|---|---|
| user turn, human or harness (`Input.text`, not a `replay`) | `Turn::take_input`, before `user_message` — so `TurnStarted.text` and the title counter see the unstamped text |
| tool batch | first `ToolResponse.content` of the batch, before `ChatMessage::from(responses)` — a long autonomous run still glances at the clock |

A text already opening with `<time ` is left alone. The stamp is part of the message text, so it is persisted and replayed byte-identically (07 rule 8): one append, no cache cost. `SessionState.last_stamp` holds the instant; `resume` recovers it from the newest stamped row in the frame (`loop_::last_stamp`), so a restart neither loses `since_last` nor double-stamps within the gap.

Readers that treat "starts with `<`" as harness text — `skills::injected` (and so `skills::in_frame` on resume), `title` asks, `hint_query` — call `clock::strip` first.

## 4. Wrapper clocks

Every harness wake-up says when it fired, in the same local `HH:MM`:

| wrapper | added | origin already there |
|---|---|---|
| `<job_result … secs="N" at="14:03">` | `at` | `secs` (job wall time) kept |
| `<job_idle … secs="N" at="14:03">` | `at` | `secs`, `quiet` kept |
| `<reminder … after="600" set_at="…" at="14:03">` | `at` | `after` (requested seconds) and `set_at` kept unchanged: the UI parses `after` |
| `<message from id [reply_to] [reply_expected] at="14:03" [after="1h 12m"]>` | `at`, `after` when the record is ≥ 1 min old (`PeerRecord.created_at`) | — |
| `<schedule name at="<rfc3339>" …>` | nothing | `at` already carries the fire time (34 §1.5) |

The builders take `now: DateTime<Local>` so tests pin the clock.

## 5. Timestamps for the human

- `messages.at TEXT NULL` (migration `0026_message_at.sql`), RFC 3339 UTC, written by `insert_in` on every append (the same instant as `sessions.updated_at`), copied by `fork`, NULL for older rows.
- It rides on `ContextEntry.at` (48 §3.2's per-row metadata, already on every `HistoryEntry`), so `Loaded.history[*].context.at` reaches the UI with no new event or entry variant.
- UI `Entry.at?: number` (ms): `Date.parse(context.at)` on load; `Date.now()` when a live card is created (own send, `TurnStarted`, `InputInjected`, `TextDelta`, `ToolCallStarted`). Absent → nothing shown.
- `Stamp`: `14:03` today, `Mon 14:03` this week, `28 Sep 14:03` older (`lib/time.ts formatAt`); full local date-time in `title`. Fine pointer: visible on message hover (`group-hover`); coarse pointer (`usePointerCoarse`): a tap on the message toggles it. It lives in the action row at the right on both user and assistant cards; the row is laid out whether or not the stamp is visible, so it never moves anything.
- Gap divider: `withGaps` in `Transcript.buildRows` puts a hairline row `1 h 12 min later · 15:15` before any row ≥ 1 h (`GAP_MS`) after the previous timed row. Derived per render, never stored.
- `stripTime`: `fromHistory` removes a leading `<time …/>` line from user text and tool output before classification and display. Live events carry the unstamped text already.

## 6. 07 amendment

Rule 4 stands for the prefix: the system prompt carries the date and nothing finer, and rebases only when the date rolls. What 07 rejected was a clock *before* stable content. A stamp inside a user or tool message at the tail is an ordinary append under rule 2 and rule 8: frozen once written, replayed identically, never rewritten. It costs the tokens of one line every ≥ 5 min and no cache.

## 7. Constants

| name | value | where |
|---|---|---|
| `TIME_STAMP_GAP` | 5 min | `clock.rs` |
| `after` on `<message>` | shown from 1 min | `loop_::peer_wrapper` |
| `GAP_MS` | 1 h | `ui/lib/time.ts` |

## 8. Deliberately not done

- No time in the system prompt; the date line is untouched.
- No stamp on every message, harness note (`<todo_continue>`, `<context_pressure>`), or `repair_dangling` placeholder — the gap rule decides, and notes follow an already-stamped input within seconds.
- No zone abbreviation (needs chrono-tz or libc FFI; the offset is unambiguous).
- No stored dividers, no per-turn duration summaries, no `at` on live events (the receipt instant is the row instant within milliseconds).
- `<schedule>` unchanged; `<reminder>`'s `after` keeps its seconds meaning.

## 9. Verification

- `cargo test -p agent`: `clock` (humanise; stamp emitted / suppressed around the gap; first stamp without `since_last`; already-stamped text untouched; strip/parse round trip), wrapper tests assert `at="14:03"` under a pinned clock and `after="1h 12m"` on a late peer record.
- `bun run test`: `lib/time.test.ts` (today / week / older, `gapLabel` threshold, `stripTime`, `withGaps`).
- Smoke: a fresh store migrates to 0026 and `context_history` returns `at` on new rows.
