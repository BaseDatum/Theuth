# 76 as built — AGENT-KEEP (`Cancel` keeps a heard prefix)

Branch `talk/agent-keep` off `feature/talk` @ c98be2b6. Spec: 76 §0.7, §7.

## What was built

### `crates/agent/src/lib.rs`

- `Cancel { session, keep: Option<String> }`. `Copy` dropped (String
  field); `Clone, Debug, Serialize, Deserialize` kept. Not a ts-rs wire type
  (the RPC `cancel(session)` is; unchanged). `answer_ask` dismiss passes
  `keep: None`.

### `crates/agent/src/loop_.rs`

- `CancelTurn { keep: Option<String> }` (was a unit struct).
- `Shared.keep: Arc<parking_lot::Mutex<Option<String>>>` — the payload slot
  beside the token. `Session`'s `CancelTurn` handler writes `m.keep` into it
  **only when a turn is in flight**, then cancels the token. `Session::enter`
  clears it, so a `keep` that landed outside a stream (tool batch, retry
  sleep) never reaches the next turn.
- `Abort::Interrupted { text: String }` beside `Partial`.
- `Turn::aborted()`: takes the slot → `Interrupted { text }` or `Cancelled`.
  Used by the two `stream_once` arms (stream open, every stream event).
  The retry-sleep arm clears the slot and returns `Cancelled` (see
  Deviations).
- `call`: `Cancelled | Interrupted` settle the model-call row as
  `CallStatus::Cancelled`.
- `round`: `Interrupted { text }` → `persist(ChatMessage::assistant(text))`
  when `text.trim()` is non-empty, no `recovery` note, `salvaged` untouched,
  `Round::Stop("interrupted")`. `TurnFinished.stop_reason == "interrupted"`.
- Turn end: `interrupted` joins `cancelled | timed_out` for the REPL-discard
  and harvest-skip decisions (what the cancelled path does).
- `aux_call` / vision question: `Interrupted` maps to `Err("cancelled")`
  like `Cancelled` (a keep during an auxiliary call is not stream text of
  the reply; the aux path already reports `cancelled`).

### `crates/agent/src/supervisor.rs`

- `LiveActor::tell_cancel(keep)`. The `Cancel` handler passes `m.keep`
  (taken) to the target session only; descendants and every other caller
  (peer-scope cancel, child timeout, close, shutdown) pass `None`.

### `crates/agent/src/acp_child.rs`

- `CancelTurn.keep` ignored (comment): the external harness owns its
  stream and transcript.

### `crates/agent/src/communication/network.rs`

- A2A scope settlement treats `"interrupted"` as `"cancelled" | "timed_out"`
  (`sc.interrupted = true`) rather than as a result.

### Tests — `crates/agent/src/loop_/interrupt_tests.rs`

One tokio test, `cancel_with_keep_persists_the_heard_prefix_then_a_plain_cancel_drops`,
through the real `Session` actor against a raw OpenAI-compatible TCP endpoint
that streams two sentences and a `shell` tool-call naming chunk, then holds
the stream open (never a `StreamEnd`). Three turns on one session:

1. `CancelTurn { keep: Some("The build is green. Two tests were skipped.") }`
   after `ToolCallPending` → `TurnFinished { stop_reason: "interrupted" }`,
   rows `[User, Assistant(keep)]` exactly, no `<recovery>` row, no
   `ToolCallStarted`.
2. `CancelTurn { keep: None }` → `"cancelled"`, no new assistant row (also
   proves the slot did not leak from turn 1).
3. `keep: Some("")` → `"interrupted"`, nothing persisted.

`vision_fallback_tests::Sink` made `pub(super)` for reuse.
`compact_queue_tests` updated to `CancelTurn { keep: None }`.

### Docs

- `docs/design/00-architecture.md` §5: `Cancel { session, keep }` sentences.
- `docs/design/29-rewind.md`: unchanged — it never states "cancel drops
  text" as an invariant (only UI Cancel buttons and the `Cancel` cascade).

## Outside the allow-list

One-line `keep: None` at `Cancel` constructions (brief item 5):
`crates/rpc/src/api.rs`, `crates/schedule/src/lib.rs` (2),
`crates/signal/src/commands.rs` (2), `crates/slack/src/channel.rs`,
`crates/slack/src/commands.rs`. No tui/meeting/tasks/roster sites exist —
they cancel through the RPC `cancel`.

## Deviations

- **Slot lives on `Shared`, not `SessionState`.** The brief says "stores it
  on the session state"; `SessionState` is moved into the turn task for the
  turn's duration (`Session::state.take()`), so the actor cannot write it
  mid-turn. `Shared` is the struct already shared between actor and turn
  (`steer`, `permit`, `toolset` follow the same pattern).
- **Retry-sleep arm returns `Cancelled`, clearing the slot.** The brief lists
  `loop_.rs:2837` among the arms that return `Interrupted`; 76 §7 names
  only the stream arms (`:2867, :2876`). In a retry sleep nothing of this
  call streamed, so persisting `keep` there would duplicate text already
  persisted by an earlier round. Design wins; the slot is still taken so a
  later plain cancel is plain.
- **Stream-open arm returns `Interrupted`** as §7 says, although nothing of
  that call has streamed either — the caller (`talk`) owns what `keep`
  means. See follow-ups.

## Verified

Toolchain `stable-aarch64-apple-darwin`, worktree
`~/Develop/theuth-talk-agent-keep`, 2026-09-28.

- `cargo test -p agent interrupt_tests` (first run, before the row helper
  ignored the `<time/>` line):
  `left: [(User, "<time now=\"2026-09-28 17:13 +01:00 (Mon)\"/>\nReport."), (Assistant, "The build is green. Two tests were skipped.")]`
  — behaviour right, assertion adjusted to compare the last line.
- `cargo test -p agent` →
  `test result: ok. 326 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 40.26s`
  (existing cancel tests in `compact_queue_tests`, `vision_fallback_tests`
  included).
- `cargo clippy -p agent --all-targets` →
  `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 3m 39s`, no warnings.
- `cargo fmt -p agent -- --check` → clean after the rustfmt commit.
- `just ts-desktop` → `export_bindings` ran, 417 files in `bindings/`, none
  named `Cancel*`; `Cancel` is not a wire type, so no binding to commit
  (`bindings/` is untracked).
- `CARGO_INCREMENTAL=0 cargo check --workspace --tests` →
  `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 7m 25s`;
  only the pre-existing future-incompat note for `block v0.1.6`. (A first
  run failed on `nodes` for a missing `bindings/methods.json` — `just ts`
  had not run yet in this fresh worktree — then on `No space left on
  device`; `target/debug/incremental` of this worktree was removed.)

## Not verified

- Live smoke through a real provider (no `talk` sender exists yet; the
  actor-level test is the end-to-end path minus the wire).
- Frontends on `stop_reason: "interrupted"`: tui/desktop/slack/signal map
  unknown reasons to their idle/done state; none renders it as a stop.

## Follow-ups

- Frontends (`tui/src/model.rs`, `desktop/ui/src/store/reduce.ts`,
  `slack/src/board.rs`, `signal/src/channel.rs`, `roster/src/desk.rs
  yielded`) treat `"interrupted"` as an ordinary finish; decide whether a
  talk interruption should render like a cancel.
- Stream-open arm: consider returning `Cancelled` there too if `talk` ever
  sends a `keep` spanning earlier rounds (duplicate assistant row risk; §7
  as written keeps it `Interrupted`).
- 76 §12: `ToolCx` cancellation token — a keep during a tool batch is
  ignored by design.
