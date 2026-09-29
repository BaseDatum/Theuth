# 76 as built — KEEP-AFTER-TURN (the heard prefix after the turn finished)

Branch `talk/keep-after-turn`, based on `feature/talk` @ f68b4660. Closes
TALK-CORE §6 "Short replies are never truncated": a barge-in during playback
of a reply that had already finished streaming used to send `Cancel{keep}`
outside any stream, `keep` was ignored and the FULL reply stayed in history.

## 1. What was built

### `crates/agent`

- `store.rs` — `Store::trim_assistant(id, turn, keep) -> (revision, assistants, epoch, head, tail)`.
  One `BEGIN IMMEDIATE` transaction: locate the visible **user row** with
  `seq == turn`, take the rows up to the next turn start, pick the last
  `Assistant` row (typed vision notes skipped); `keep` non-empty → `UPDATE
  messages SET body = ChatMessage::assistant(keep), excluded = NULL`; empty →
  `DELETE` that row; `context_revision + 1 RETURNING`; then `active_in` in
  the same transaction for the effective frame (what `frame()` would return).
  Returns the assistant-row count of the turn. `Err("not a user row" | "no
  assistant row")`. Tool rows are untouched.
- `loop_.rs` — session-actor message `TrimTurn { turn: i64, keep: String }`
  (`ask → Result<(), String>`): the `idle()` check every turn-granular edit
  uses (`Err("turn in flight")`), `store.trim_assistant`, `install(epoch,
  head, history)` on the idle state (the `remove_context` path, not
  `SessionState::resume` — skills/todos survive), then
  `context_changed(revision, "trim")`. `assistants > 1` → `tracing::debug!`
  "tool calls ran after the spoken prefix; only the last assistant row is
  trimmed".
- `supervisor.rs` — `impl Message<TrimAssistant> for Agents`: the `Undo`
  shape (`search.invalidate()`, `ledger.bump(session)` — a context edit
  stales admitted candidates — `actor(session).native()?.ask(TrimTurn)`).
- `lib.rs` — public `TrimAssistant { session, turn: i64, keep: String }`
  beside `EditTurn`; `AgentEvent::ContextChanged.reason` gains `"trim"`.
  No new `AgentEvent`: both frontends already answer any `ContextChanged`
  with a newer revision by a settled reload of the transcript
  (`desktop/ui/src/routes/c.$session.tsx` `settle()`, `tui/src/model.rs`
  `refresh_context`), so the trimmed turn re-renders without a frontend change.

### `crates/talk`

- `policy.rs` — `Input::TurnStarted(i64)` now carries `turn_seq`;
  `Ledger.turn: Option<i64>` is set on every `TurnStarted` (also for the
  turn we submitted ourselves, whose ledger was reset at submit).
  `Action::Trim { turn, keep }`. `Policy::commit` (the five cuts): after
  `StopSpeech`, `ledger.turn.filter(|_| ledger.turn_done)` → `Trim { turn,
  keep }`, else `Cancel(Some(keep))` + `cancelled = true` (only the cancel
  path gets a `TurnFinished{interrupted}` to swallow). Everything after —
  `Interrupted{heard, dropped}`, `State(Listening)`, `interrupted = true` →
  `[spoken, N s, interrupted]` — is unchanged on both paths.
- `lib.rs` — `AgentEvent::TurnStarted { turn_seq, .. }` → `Input::TurnStarted(turn_seq)`;
  `Action::Trim` → `agents().ask(agent::TrimAssistant { session, turn, keep })`,
  `warn!` on `Err`.

### Docs

- `docs/design/76-talk.md` §7: heading "(one field, one message)" and one
  paragraph on the post-turn case and `TrimAssistant`.
- `docs/design/76-as-built-TALK-CORE.md` §6: the "Short replies" follow-up
  struck through with a pointer here (the brief names 76 §12, but the item
  never lived there — §12 has no short-replies entry; nothing to remove).

## 2. Decisions / deviations

- **Key on the user row, not on a store turn start.** `TurnStarted.turn_seq`
  is the turn's first persisted row. After a cancelled turn that left no
  assistant row (§0.9 late addition, Esc in the TUI) the next user row is not
  a turn *start* by `is_turn_start` (prev role must be Assistant), and a
  `cut_to`-style `"not a turn start"` refusal would have made the trim fail
  exactly in the talk flow. The store finds the user row by seq and bounds
  the turn at the next turn start after it. Covered by the agent test.
- **`ContextChanged{reason:"trim"}` instead of a new event** — see §1;
  frontends compare `revision`, not `reason`.
- **`install` not `resume`** for the frame swap (the `remove_context` path).
- **A removal mask on the rewritten row is dropped** (`excluded = NULL`):
  the mask indexed the old body's parts.
- **Not `cut_to`/`commit_cut`**: those hide/delete from a turn opener to the
  end (the whole turn incl. the user row) and are undo-able; a trim edits one
  row in place, like `remove_context` writes one row's mask. No undo for a
  trim (the heard prefix is the truth; the dropped tail was never heard).
- `Action::Cancel` kept as is for the running-turn path and §0.9.

## 3. Verified

Toolchain per brief (`export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:…"`).

- `cargo test -p agent interrupt_tests`
  ```
  test loop_::interrupt_tests::cancel_with_keep_persists_the_heard_prefix_then_a_plain_cancel_drops ... ok
  test loop_::interrupt_tests::trim_rewrites_the_finished_reply_to_the_heard_prefix ... ok
  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 325 filtered out; finished in 0.68s
  ```
  The new scenario (`crates/agent/src/loop_/interrupt_tests.rs`, fake
  OpenAI endpoint that completes with `One. Two. Three.` on request): `TrimTurn`
  during the held stream → `Err("turn in flight")`; after the finished turn
  `TrimTurn{keep: "One. Two."}` → `Ok`, `ContextChanged{reason:"trim"}`
  published, the store row reads `One. Two.`; the next turn's request body
  (captured at the endpoint) contains `One. Two.` and not `Three.` — the
  in-memory history was replaced; `TrimTurn{keep: ""}` deletes the row (store
  rows `[User, User, Assistant, User]`, live `ContextBreakdown.tail_messages == 4`);
  a second trim → `Err("no assistant row")`. The trimmed turn's user row
  deliberately follows a cancelled turn with no reply, i.e. it is not a store
  turn start.
- `cargo test -p talk`
  ```
  test policy::tests::barge_in_trims_a_finished_turn_and_cancels_a_running_one ... ok
  test policy::tests::barge_in_commits_the_five_cuts_and_marks_the_next_input ... ok
  test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
  ```
  The five-cuts test now runs the finished-turn case (`speaking()` feeds
  `TurnFinished` before the first `SpeechStarted`, exactly the short-reply
  shape) and expects `Action::Trim { turn: 7, keep: "One." }`; the new test
  pins `Cancel(Some("One."))` while the paragraph is still streaming and
  `Trim` once it finished, plus `cancelled == false` / `interrupted == true`
  after a trim.
- `cargo clippy -p agent -p talk --all-targets` → no output, exit 0.
- `cargo fmt -p agent -p talk --check` → clean for the touched files
  (`loop_/vision_fallback_tests.rs:172` has a pre-existing diff; not touched).
- `cargo check --workspace --tests` fails on the **base** branch, not on
  this change: `crates/tui/src/talk.rs:234` `missing field `sink` in
  initializer of `VoiceEvent::SpeechFinished`` (`git diff f68b4660 -- crates/tui`
  is empty; `tui` is INTEGRATE's). `cargo check --workspace --tests
  --exclude tui --exclude theuth` → exit 0 (only the pre-existing
  `block v0.1.6` future-incompat note). `just ts` (bindings the `nodes`
  tests `include_str!`) → `8 passed`, 427 files in `bindings/` (gitignored);
  nothing new is `ts`-exported by this change.

## 4. Not verified

- No live device smoke (headless worker; INTEGRATE owns the integrated
  gate). The wiring `Action::Trim → agent::TrimAssistant` is exercised only
  by type-checking; the policy choice and the agent side are unit-tested.
- Frontend re-render after `ContextChanged{trim}` is by reading the existing
  reload paths, not by running a UI.

## 5. Outside the allow-list

- `docs/design/76-as-built-TALK-CORE.md` §6 (one bullet struck through).
- `docs/design/76-talk.md` §7 (asked for).

## 6. Follow-ups

- The tiny race where `TurnFinished` is in flight when the barge-in commits:
  `Cancel{keep}` goes out, finds no stream, `keep` is ignored (the old
  behaviour, for one event-delivery window). A `Cancel` that falls back to a
  trim inside the agent when nothing is in flight would close it (`agent`
  change: `CancelTurn` would need the turn seq).
- **`TurnFinished` precedes the handback.** The actor is idle for an edit
  only after `TurnDone` (the settled `ContextChanged`), a few µs–ms after
  `TurnFinished`; a `TrimAssistant` in that window is refused "turn in
  flight" and `talk` logs a `warn` (history keeps the full reply). A
  barge-in cannot commit that fast (hold 400 ms + a decide window after the
  first `SpeechStarted`, which follows synthesis), so no retry was added;
  the agent test waits for the settled event for exactly this reason.
- A `Trim` for a turn whose last assistant row is *before* tool results
  (narration → tools → nothing more) rewrites the narration, which is right;
  the `debug` line marks the multi-row case.
