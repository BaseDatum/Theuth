# 36 — Survival: retries, salvage, and what compaction leaves behind

Status: implemented v0 · 2026-09-16 · extends `07-context.md` §6, §8; `11-compaction-styles.md`;
`01-config-and-subagents.md` §2.5; `14-jobs.md`. Where this doc disagrees with those,
this doc wins; §9 lists the edits (applied). Code: `crates/agent/src/{loop_,compact,spill,supervisor}.rs`.
As built: `Turn::call` keeps `&self` (the frame borrows session state), so the retry
counter is an `AtomicU32`; `Abort::Partial` is also returned when a tool-call chunk had
arrived with no text, since genai delivers arguments only at `End`.

Sources: a code-verified survey of `../grok-build` (xAI Grok CLI:
`xai-grok-compaction`, `xai-grok-shell/src/session/*`, `sampler_turn.rs`,
`length_salvage.rs`) and `../jarvis` (`jarvis-agent/src/{run,compact,hygiene,budget}.rs`).
Everything jarvis does to the *middle* of the frame is already rejected in 07 §1/§14 and
stays rejected. What remains to borrow is almost entirely **what happens when a call
fails** and **what the model can still reach after a fold**. Every mechanism below is
either an appended tail message, an off-frame file, or a resend of an identical request:
none moves a byte of the cached prefix.

## 0. What breaks today

Verified against `crates/agent/src` at the date above.

| event | today | where |
|---|---|---|
| 429 / 5xx / connection reset / TLS hiccup on open | turn ends with `AgentEvent::Error("model: …")`, stop `error` | `loop_.rs:1456-1477`; `backend.rs:72-103` is a single `exec_chat_stream` |
| stream dies mid-reply (`None` before `End`, `Some(Err)`) | same; streamed prose is discarded | `loop_.rs:1062-1063` |
| stream hangs (no event for minutes) | the round blocks until the socket dies or the user cancels | no idle timer in `call` |
| `stop_reason == max_tokens` | `Round::Stop("max_tokens")`: the reply is persisted cut off, the turn ends | `loop_.rs:1524-1547` |
| process exits between the assistant `tool_use` row and its `tool_result` row | on resume the frame ends in a dangling `tool_use`; Anthropic rejects the next request with 400 | `store.rs:520-530`, `SessionState::resume` `loop_.rs:191-213` |
| summariser hangs or returns 40 chars | no timeout; anything non-empty is accepted as the epoch head | `loop_.rs:1989-2054` |
| the model echoes `<context_summary …>` inside its summary | wrapped verbatim; the next epoch's head contains a nested tag | `compact.rs` `wrap_head` |
| model not in the catalog (`window == 0`) | auto-compaction off for the whole session; only the once-per-turn overflow path remains | `loop_.rs:884-895` |
| after compaction | `used = 0` until the next provider report; the trigger under-projects for one call (13 §0.6) | `loop_.rs:1960` |
| a `<subagent_result>` | injected uncapped; a chatty child can fill the parent's window in one message | `supervisor.rs:589-592` |
| a 4-call batch of 32k-char results on a 32k-token window | fills the window (07 §6 stated ceiling) | one cap per result, no batch bound |
| `max_tool_rounds` reached | `AgentEvent::Error("stopped after 32 tool rounds")`, no reply | `loop_.rs:1184-1188` |
| the model retries the identical failing call | nothing; it burns rounds until the cap | — |
| after a fold | the folded turns exist only as sqlite rows the model cannot read | `messages` table |

## 1. Principle

Two rules, both consequences of 07 §0:

1. **Recovery appends or resends; it never edits.** A retry sends the same
   bytes. A salvage persists what streamed and appends a note. A repair appends
   a synthetic `tool_result`. Nothing before the previous request's last
   message changes.
2. **Recovery is bounded and counted.** Every loop has a constant cap and a
   `tracing::info!` line naming the cause; nothing retries forever, nothing
   retries silently.

Constants live in code, not config (07 §14 "one knob"), with two exceptions
that turned out to differ per provider: `agent.stream_idle_secs` and
`agent.max_salvages` (§2.3, §2.4; Settings › Agent loop). Bedrock stalls for
minutes mid tool-argument on long documents where Anthropic direct does not,
so the idle floor and the recovery budget are the user's call. Numbers come
from grok-build's production defaults (`sampler_turn.rs:16-79`,
`length_salvage.rs:5-14`, `agent/compaction.rs:29-35`) where they exist.

## 2. Model call: classify, retry, recover

### 2.1 Error classes

`Turn::call` (`loop_.rs:1038-1129`) returns `Err(Abort::Model(String))` for every
failure. Add a classifier in `compact.rs` beside `is_overflow`:

```rust
pub enum Failure { Overflow, Deterministic, Transient }
pub fn classify(err: &str) -> Failure
```

- `Overflow` — `is_overflow` (§2.5 needles). Handled by 07 §8.5 as today.
- `Deterministic` — HTTP 400/401/403/404/422 and any 4xx other than 408/429,
  auth/config errors, JSON schema rejections. Not retried: the same request
  gets the same answer. Detection is by status text in the genai error
  (`"status: 4"`) minus the two exceptions, same as grok `session_compact.rs:145-217`.
- `Transient` — everything else: 408, 429, 5xx, connection/TLS errors,
  `stream ended without End event`, idle timeout (§2.3), empty `End`.

### 2.2 Retry on open and on early death

Wrap the body of `call` in a loop:

```
attempt = 0
loop {
  match open + stream {
    Ok(end)                          => return Ok(end)
    Err(Cancelled)                   => return Err(Cancelled)
    Err(Model(e)) if classify(e) != Transient || attempt == RETRY_BACKOFF.len()
                                     => return Err(Model(e))
    Err(Model(e)) if streamed.text.is_empty() && streamed.calls.is_empty()
                                     => sleep(RETRY_BACKOFF[attempt] + jitter); attempt += 1
    Err(Model(e))                    => return Err(Abort::Partial(streamed, e))   // §2.4
  }
}
```

`RETRY_BACKOFF = [2, 10, 30]` seconds, jitter `0..1 s` — three resends per
call, ~42 s worst case, grok's constants. A per-turn cap `RETRY_MAX_PER_TURN
= 10` on `Turn` stops a flapping gateway from holding a turn for minutes
(`Transient` past the cap is returned as-is and ends the turn as today). The
sleep selects on `self.cancel` so cancel stays instant. Nothing was persisted,
so the resend is byte-identical to the failed request: a full cache hit if
the first attempt reached the provider.

The summariser, harvest and title calls go through the same `call`, so they
inherit the retry. The title call's own 45 s timeout (`loop_.rs:1324`) is
outside `call` and unchanged.

### 2.3 Idle timeout

Inside the stream loop, `tokio::time::timeout(idle, stream.next())` with
`idle = agent.stream_idle_secs` (default 120 s, snapshotted per session like
`max_tool_rounds`); every event — text, reasoning, tool-call chunk,
`Heartbeat` — resets it. Expiry is `Transient` with the message
`stream idle for {N}s`. Reasoning models emit `ReasoningChunk`s while they
think and Anthropic sends `Heartbeat`/`ping` on long gaps, so 120 s is a dead
socket, not a slow model. jarvis distinguishes a 600 s "tool arguments
streaming" idle (`run.rs:100-125`); a chunk of arguments is still a chunk, so
one timer covers it — a provider that goes silent that long mid-argument gets
a longer `stream_idle_secs`, not a second timer.

### 2.4 Salvage a partial reply

When the stream dies after content was streamed (`Abort::Partial`), a resend
would make the model re-generate what the user already watched, and the
partial text is often most of the answer. Instead, in `round`:

1. `persist(ChatMessage::assistant(streamed.text))` — only if the text is
   non-empty. Partial tool calls are dropped: genai delivers arguments whole
   at `End` (`loop_.rs:1085-1088`), so nothing usable exists; no `tool_use`
   is persisted, so no `tool_result` is owed.
2. `note("recovery", "Your reply was interrupted by a transport error after
   the text above. Continue from where it stopped; do not repeat it. If a
   tool call was in progress, make it again.")` — fixed bytes.
3. `Round::Tools` — call again; counts as a round.

Cap `agent.max_salvages` (default 2, shared with §2.6; `0` disables salvage);
past it the error ends the turn as today. The frame stays append-only:
assistant, then a user-role note, then the next assistant — the same shape as
a steering message.

### 2.5 Overflow needles

Extend `OVERFLOW_NEEDLES` (`compact.rs:29-38`) with grok's anchored slugs
(`failure.rs:9-60`), lowercased:

```
"too long for this model", "maximum allowed number of bytes",
"exceeds the maximum allowed length", "payload too large", "content too large",
"request entity too large", "413 "
```

and carry over grok's test table (`failure.rs:64-124`) as the regression
corpus for `is_overflow`, including its negatives (`rate limit exceeded`,
`too many requests` must stay `false` — a 429 that mentions "too large" is
`Transient`, checked first).

### 2.6 Length salvage

`stop == "max_tokens" | "length"` (raw provider reason, `loop_.rs:1524-1528`):
the model hit its output cap. Today the truncated reply is persisted and the
turn ends. Instead, when `calls.is_empty()`:

1. persist the content as today;
2. `note("continue", "Your previous reply hit the output token limit and was
   cut off. Continue from exactly where it stopped — do not repeat what is
   above. If a newer user message follows this note, answer that instead.")`;
3. `Round::Tools`.

Cap `agent.max_salvages` (default 2, grok's default budget) per turn. With tool
calls present and `max_tokens`, the arguments are truncated JSON; genai's
`captured_tool_calls` either fails to parse (→ treated as no calls, salvage
above) or yields calls the tool will reject with a parse error, which is the
normal tool-error path. Neither needs special handling.

## 3. Resume repair

`SessionState::resume` (`loop_.rs:191-213`) receives the frame from
`Store::frame`. If the last message is an assistant with tool calls (the
process died between `persist(assistant)` at `loop_.rs:1552` and
`persist(ChatMessage::from(responses))` at `:1658`), append — and
`Store::append` — one tool message answering every call with
`ERROR: not run: the harness exited before this tool completed`. Same shape
as the cancel path's `cancelled by user` responses (`loop_.rs:1606`). Logged
`resume: repaired N dangling tool calls`. Without it the session is dead:
every request 400s on the orphan `tool_use` and the user's only recourse is
`/undo`.

This is the only rewrite-free repair needed: every other persist point leaves
the frame valid (a user message, a complete tool batch, or an assistant text).

## 4. Tool output

### 4.1 Cap the subagent result

`Agents::child_finished` (`supervisor.rs:569-593`) wraps
`store.last_turn_text(child)` uncapped. Route `text` through `spill::cap(parent,
&child.to_string(), &text, cfg.max_tool_output_chars, &spill_dir)` before
wrapping; the child's own session id is the spill file stem, so the parent's
`read_file` retrieves it. The waiter branch (`spawn_subagent{wait}`) gets the
capped text too: it becomes a tool result and is subject to the same bound.

*Amendment (50 §7–8; implemented 2026-09-18, verification pending).* The cap
stays and applies at the same point, but the result no longer travels as an
uncorrelated `UserInput`: `child_finished` caps, redacts, then **accepts one
durable exchange** keyed `(child, TurnId, parent)` in the agent store before
anything is queued, and the recipient's consumption transaction links that
row to the appended history row before the model call. The spill file stem
is still the child's session id. One bounded intake contract covers child
results, peer messages and A2A task input alike (32 KiB per peer text,
bounded pending inputs per recipient and harness, busy rather than a silent
drop); a result that cannot be delivered because communication is off is
not injected and the parent is not woken — the child's transcript and the
exchange record remain inspectable.

### 4.2 Batch budget

After each result is capped (`loop_.rs:1623-1629`), bound the batch:

```
budget = limit / 4          // limit = compact_at × window, 07 §8.2; skip when window == 0
while Σ est(responses) > budget:
    i = argmax est
    responses[i].content = spill::cap(session, call_id, &out.content, current_chars(i) / 2, dir)
```

`cap` on the same content and `call_id` rewrites the same spill file with the
same bytes (idempotent), so the marker stays truthful. Floor 1 000 chars per
result so a marker plus a hint always survives. This closes the 07 §6 ceiling
(four 32k-char results on a 32k-token window) with one rule and no per-tool
cap table (jarvis `hygiene.rs:396-414`, grok `xai-grok-tools/lib.rs:9-13` have
one; the batch bound makes it unnecessary). Applied before `persist`, so it is
still "bounded once, at append time".

### 4.3 Repeated identical failure

Keep `last_failure: Option<(String, serde_json::Value)>` on `Turn`. When a
batch contains an error result whose `(name, args)` equals it, append
`note("steer", "The call <name> failed twice with the same arguments. Do not
repeat it; change the arguments or the approach.")` after the tool message
and clear it. jarvis `run.rs:445-448`. Cheap, and it converts a doomed loop
into one wasted round.

## 5. Tool-round budget: land, don't crash

At `rounds == max_tool_rounds − 2`, `note("rounds", "2 tool rounds remain in
this turn. Finish the current step and reply with results and next steps.")`.

At `rounds == max_tool_rounds`, instead of `Error("stopped after …")`:

1. `note("rounds", "No tool rounds remain in this turn. Reply with text now:
   what was done, what was not, and the next step.")`;
2. one more `call`. If it returns text → normal stop with the model's own
   reason. If it returns tool calls → persist the assistant message, persist a
   tool message with `ERROR: not run: tool rounds exhausted` for each call
   (keeps the pair invariant), stop `max_tool_rounds` with the text it did
   emit.

Tools stay in the request — removing them would rewrite the prefix for the
last call. jarvis `budget.rs` + `run.rs:4851` (soft-land) without its
phase machinery. The turn still ends at 33 calls; it ends with an answer.

## 6. Compaction

### 6.1 Summariser hardening

In `Turn::continuation` (`loop_.rs:2018-2054`):

- **Wall clock.** `tokio::time::timeout(SUMMARISE_TIMEOUT, self.call(…))`,
  `SUMMARISE_TIMEOUT = 300 s` (grok `agent/compaction.rs:32`). Expiry is
  `Err("summariser timed out")`, which `compact` treats like overflow: fall
  back to `cut` (`loop_.rs:1876-1883`). A runaway reasoning model no longer
  pins the session.
- **Degenerate reply.** A summary under `SUMMARY_MIN_CHARS = 500` takes the
  existing `text_only` retry branch (`loop_.rs:2041-2048`) with the prefix
  `Text only, complete summary. `; a second short reply → `Err("degenerate
  summary")` → `cut`. grok rejects `< 500` (`config.rs:16-21`, "smallest
  healthy prod summary ~3,242 chars"). Harvest's `continuation` is exempt (a
  short harvest is a valid "nothing new").
- **Tag neutralisation.** Before `wrap_head`, replace `<context_summary` and
  `</context_summary` in the body with the same text carrying a U+200B after
  `<` (grok `summary.rs:19-113`). The head parser (`style_of_head`) only reads
  the opening tag it wrote; the point is to stop the model from being primed
  to open its next reply with the tag it just saw.

### 6.2 The instruction

`COMPACT_INSTRUCTION` (`compact.rs:13-26`) keeps its five headings; the
prose gains four clauses that grok's and jarvis's templates converged on
(`full_replace_summary_prompt.txt`, `prompts/compact.md`), each cheap in the
summary and expensive when missing:

```
The conversation is about to be compacted: everything above this message will be replaced by
your summary, and only the most recent messages will be kept verbatim. Write the summary now.
Reply with text only — do not call tools. Use exactly these headings and short bullets:

## Goal
## Done  (facts established, files changed, commands run and whether they passed)
## Open  (in-progress work, unanswered questions, pending approvals)
## Next  (the immediate next step, and a short verbatim quote of the last thing you were
         doing so the continuation resumes mid-step rather than restarting)
## Notes (user preferences, constraints, names and paths to remember, runtime variables you
         rely on and what they hold, decisions taken and alternatives rejected)

If a previous summary is present at the top of the conversation it is authoritative for the
earlier history: carry forward what is still true, drop what is finished. Every request the
user made, in order, belongs under Goal or Done; do not invent tasks. Be specific — paths,
identifiers, exact error text — and omit chit-chat and tool-call boilerplate. A summary that
fits is worth more than one that is exhaustive: at most a few thousand words.
```

Fixed bytes, appended once per compaction to a dead-end branch: no cache
effect. Evaluate with §6.4 before and after.

### 6.3 Harness lines in the head

`compact` renders roster, todos, runtime inventory and knowledge ids
(`loop_.rs:1889-1915`). Add two more lines of harness truth the summariser
cannot reconstruct (grok `compaction_context.rs:66-123`):

- `Jobs: j3 running (pytest -x, 4m12s), j5 running (npm run dev)` — from
  `ListJobs(session)` filtered to `exit.is_none()`; omitted when none. A model
  that forgets a running job re-launches it.
- `Files written: a/b.rs, c/d.md` — `SessionState.files_written:
  BTreeSet<String>`, fed from `write_file` call arguments (`path`) in the tool
  batch, cleared with the epoch, capped at 40 entries.
  <!-- ponytail: only write_file is tracked; edits through `python` or a shell
  redirect are not. Add tool-declared side effects if a transcript shows the model
  re-writing a file it already wrote. -->
- `Earlier turns verbatim: <abs path>` — §6.4.

### 6.4 The folded span is a file

At compaction, before `history.drain(..keep_from)`, write the folded messages
to `<spill_dir>/<session>/epoch_<n>.txt` and the resulting head body to
`<spill_dir>/<session>/epoch_<n>.summary.md`. Format of the transcript: one
block per message, `--- user | assistant | tool(<name>) [seq N] ---` then the
text; tool responses as stored (already capped, so the file is bounded by the
window). Images and PDFs are named, not embedded. The head line in §6.3 names
the file; `read_file` with `offset`/`limit`, or `grep` through `shell`, is the
retrieval — the same convention as tool-output spill (07 §6). The 7-day spill
sweep applies.

This gives two things at zero model cost: the model can recover a detail the
summary dropped without a `/undo` (grok segments mode,
`compaction_mode.rs:13-84`), and every compaction leaves its exact input and
output on disk so the instruction in §6.2 can be iterated against real folds
(grok `persist_compaction_request_artifact`, `compaction.rs:2325-2385`).

### 6.5 Seed `used` after the fold

`loop_.rs:1960` sets `used = 0`; `after` two lines above is already
`est(head) + Σ est(tail)`. Set `used = after`. Off by chars/4 error for one
call, then the provider's number replaces it — instead of off by the whole
frame.

### 6.6 Window from overflow

When `window() == 0` and an overflow arrives, the projection that overflowed
is a hard upper bound on the window. Keep `window_hint: u32` on `SessionState`
(not persisted): set it to `projected` in the overflow branch
(`loop_.rs:1458`), and let `window()` return `catalog || window_hint`. The
next call's `compact_at × window_hint` trigger fires before the provider
does. One overflow per session instead of one per turn. grok
`sampler_turn.rs:1240-1275` does the same from the error's
`model_metadata.context_window`; genai does not expose that, so the
projection stands in.

## 7. Numbers

| constant | value | source |
|---|---|---|
| `RETRY_BACKOFF` | `[2, 10, 30]` s + 0–1 s jitter | grok `sampler_turn.rs:16-69` |
| `RETRY_MAX_PER_TURN` | 10 | grok "10 per prompt" |
| `agent.stream_idle_secs` | 120 s | jarvis `run.rs:100-125` (cold idle) |
| `agent.max_salvages` | 2 (transport + length together) | grok `length_salvage.rs:5-8` |
| `SUMMARISE_TIMEOUT` | 300 s | grok `agent/compaction.rs:32` |
| `SUMMARY_MIN_CHARS` | 500 | grok `code_compaction/config.rs:21` |
| batch budget | `limit / 4`, floor 1 000 chars/result | this doc |
| rounds warning | at `max_tool_rounds − 2` | this doc |
| `files_written` cap | 40 | this doc |
| epoch transcript | bounded by the folded span (already capped) | — |

New config keys: `agent.stream_idle_secs`, `agent.max_salvages` (both also
`Info` fields and `set_stream_idle` / `set_max_salvages` RPCs; new sessions only).

## 8. Rejected

- **Per-tool-class caps** (jarvis shell/web/MCP ≈ 25.6k vs files 80k; grok
  bash 20k / generic 40k). The batch budget (§4.2) bounds the round; a second
  table of caps is a second knob for the same protection.
- **Separate idle timers for "thinking" vs "tool arguments streaming"**
  (jarvis 120/600 s). Both emit chunks; one timer, one knob.
- **Retry counting per 10-minute episode window** (grok). Per-call and
  per-turn caps are enough; a turn is the episode.
- **Provider-ratio reseed of `used` after compaction** (grok
  `mutations.rs:491-507`, `total = est × provider/est_at_report`). `after` is
  already computed; the ratio buys one call of precision and costs a whole-
  frame estimate per report.
- **Two-pass prefire compaction** (grok, default off upstream). Latency of the
  summariser has not been measured as a problem; a cache-hot continuation is
  already the cheap variant.
- **Compaction suppression state machine** (grok `SUPPRESS_*`). theuth's
  compaction cannot fail short of `keep_from == 0`: every summariser failure
  falls back to `cut`. Nothing to suppress.
- **Stale-while-revalidate baseline** (jarvis `ReplacementBlocked`: keep the
  old system prompt when an input source is unavailable). `frame::baseline`
  has no fallible input today that renders as absence; revisit if the rebase
  log ever shows an A→B→A flap.
- **Dropping the kept tail entirely at compaction** (grok: "frees almost
  nothing and re-cues re-reading files"). theuth's `limit/8` tail was tuned in
  07 §14; a `compact_at`-relative A/B is a future experiment, not a change.
- **Memory flush as a second pre-compaction call** (grok `flush.rs`, jarvis
  `memory_flush_prompt`). Harvest before the cut (10 §7) is that call.
- **Post-compaction `<system-reminder>` as a separate message** (grok
  `reminder.rs`). The head already carries harness lines; §6.3 adds to them.
- **Removing tools from the final soft-land call** (jarvis). Rewrites the
  prefix for one call; the note plus synthetic results (§5) keep the pair
  invariant without it.

## 8a. Communication recovery (50 §7–8; implemented 2026-09-18, verification pending)

Bounded, visible recovery extends to peer traffic without a second recovery
system:

- **Restart.** No accepted peer input or A2A task auto-runs. At start the
  supervisor marks unconsumed accepted exchanges `failed:
  interrupted_by_restart` and running task scopes `FAILED` with that reason
  (`Store::fail_interrupted`); committed outcomes stay queryable; an already
  committed but unlinked result notification reconciles by its dedup key
  without a model call. A fresh request continues the work. This is durable
  accounting, not an at-least-once execution promise.
- **Disable.** Turning communication off is not an error path: accepted,
  unconsumed input becomes `suppressed` and does not revive on re-enable;
  pending waits expire with `communication_disabled`, distinct from a
  timeout; a turn is cancelled only when an A2A task opened it. In-flight
  network sends abort best effort and an unresolved remote outcome is
  `unknown`, never "undelivered".
- **Cancellation.** Per request scope, decided by one atomic terminal
  transition against completion; an external tool that will not stop
  surfaces `cancellation pending` and its remaining work rather than a false
  `CANCELED`. Dropping an SSE stream or closing a frontend is not a cancel.
- **Bounds instead of retries.** Pending inputs 32/recipient, 256/harness;
  pending waits 16/session, 128/harness; 32 automatic peer-triggered
  continuations per initiating request; every native peer-opened turn takes
  a child-work permit (released while it only waits) under the
  `max_runtime_secs` wall deadline, waits included (01 §2.6 amendment).
  Exhaustion refuses visibly. Outbound A2A never blindly
  replays `SendMessage`: without a task handle a transport failure after
  bytes may have left is `unknown` and needs an explicit decision;
  read/status reconnects reuse the existing bounded backoff and never mint a
  new task silently.

None of this has a recorded run yet; 50 §12.2 lists the scenarios that turn
it into a verification entry in §10.

## 9. Edits to earlier docs

- `07-context.md` §6: add the batch budget (§4.2) and the subagent-result cap
  (§4.1); §8.4: `used = after` (§6.5); §8.5: `Failure` classes and the
  transient retry (§2); §8.7: new note kinds `recovery`, `continue`, `steer`,
  `rounds`; §12: new checks below.
- `11-compaction-styles.md` §1: summariser timeout, degenerate fallback to
  `cut` (§6.1); the epoch transcript file (§6.4).
- `01-config-and-subagents.md` §2.5: the child result is capped (§4.1).
- `13-snap.md` §0.6: superseded by §6.5.
- `crates/theuth/harness.md`: `spill/<session>/epoch_<n>.txt` listed beside the
  tool-output spill files.

## 10. Verification

Each item is a runnable check. Results as of 2026-09-16: `just check`, `just clippy`
(`-D warnings`) and `just test` green across the workspace after the build; items marked
**Pass** ran; the rest need a shim gateway or a live model and are **Not yet run**.

1. **Transient retry.** Point a session at a gateway that returns 503 twice
   then succeeds (a 20-line local HTTP shim); one turn, log shows two
   `retrying model call` lines with 2 s and 10 s waits, the reply arrives, the
   cache-ratio on the third attempt ≥ 0.9 of the first (identical bytes).
2. **Classifier.** Unit table: `429` → Transient; `400 invalid_request` →
   Deterministic; `prompt is too long` → Overflow; `413 Payload Too Large` →
   Overflow; `rate limit exceeded … too large a burst` → Transient. grok's
   table for the negatives. **Pass** — `compact::tests::classify_table` (11 rows
   incl. 503, 408, `http 422 schema`), `overflow_needles` extended with the 413 /
   "too long for this model" positives and the `too many requests` negative.
3. **Idle.** Shim that accepts and never writes; call returns `Transient`
   after `stream_idle_secs` ± 1, retry follows.
4. **Salvage.** Shim streams 200 tokens then closes the socket; history gains
   an assistant text + `<recovery>` note + a second assistant; `/undo` removes
   all three as one turn.
5. **Length.** `max_tokens: 64` on a "write 500 words" prompt; two `<continue>`
   notes, three assistant messages, stop reason of the last is `end_turn`.
6. **Resume repair.** Insert an assistant `tool_use` row as the last message
   of a stored session; `--resume` logs `repaired 1 dangling tool calls`; the
   next request succeeds on Anthropic.
7. **Subagent cap.** Child replies with 100k chars; the parent's injected
   message is ≤ `max_tool_output_chars` with a marker naming
   `spill/<parent>/<child>.txt`.
8. **Batch budget.** Four `read_file` calls of 30k chars each on a
   `compact_at 0.8`, 32k-window catalog entry; Σ est of the tool message ≤
   6 400 tokens; four spill files.
9. **Soft land.** `max_tool_rounds: 4` on a task needing more; a `<rounds>`
   warning at round 2, a final text reply, stop reason `max_tool_rounds`, no
   `AgentEvent::Error`.
10. **Summariser fallback.** `SUMMARISE_TIMEOUT` set to 1 s in a test build;
    `/compact` produces a `cut` head and logs the timeout. A stub model
    answering "ok" produces a `cut` head after one `Text only` retry.
11. **Epoch file.** After `/compact`, `spill/<session>/epoch_1.txt` holds
    every folded message in order; the head's last harness line names it; the
    model answers "what was the exact error in the first turn" by reading it
    (headless, real model). **Unit part pass** — `spill::tests::epoch_writes_transcript_and_summary`
    (both role headers present, summary file holds the body); the head line and
    the model read-back are not yet run.
12. **`used` seed.** Log the first trigger projection after a compaction; it
    is within 15 % of the provider's `prompt_tokens` on the next call.
13. **Prefix oracle.** `frame::tests::prefix_oracle_across_rounds_and_epochs`
    extended with a salvage round and a soft-land round: every request is a
    strict extension of the previous one within the epoch. **Not yet extended**;
    the existing oracle passes.

## 11. Implementation order (done)

Each step leaves `theuth` runnable.

1. `compact::classify` + needle corpus (§2.1, §2.5) — tests only.
2. Retry loop and idle timer in `Turn::call` (§2.2, §2.3).
3. `Abort::Partial` + salvage, length salvage, shared cap (§2.4, §2.6).
4. Resume repair (§3).
5. Subagent cap, batch budget, repeated-failure note (§4).
6. Soft land (§5).
7. Summariser timeout / degenerate / neutralise; `used = after`; window hint
   (§6.1, §6.5, §6.6).
8. Epoch transcript file + head lines (§6.4, §6.3).
9. Instruction text (§6.2), compared on the epoch files from step 8.
10. Doc edits (§9), harness.md, verification results into §10.
