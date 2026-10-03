# 82 — as built: REGEN-STAMP

Status: built 2026-10-03 on `fix/regen-stamp` (base 4d6a6ed6). Amends 74 §3 (seam table) and §5 (`splitTime`).

## Bug

Regenerating a reply in the desktop app showed the harness clock line — `<time now="2026-10-03 00:05 +01:00 (Sat)"/>` — above the user's words in the user bubble until the turn finished.

## Root cause: (a) + (b), exposed by (c)

- (a) The stored user row *is* the model's frame (74 §3: the stamp is part of the message text, persisted and replayed byte-identically). So every stamped opener in the store carries `<time …/>\n` before the human's words. By design.
- (b) `RestartTurn { input: None }` (regenerate) built its `Input.text` from `text_of(&opener.content)` — the stored, stamped row — and `start_turn` publishes `Input.text` as `TurnStarted.text`. The normal send path clones `input.text` *before* `take_input` stamps it (74 §3 seam table), so only the replay path leaked the stamp into an event.
- (c) The desktop reducer stripped the stamp on history reload only (`stripTime` in `fromHistory`, 74 §5: "live events carry the unstamped text already"); the live `TurnStarted` path ran `classify` on the raw text, `<time` is no wrapper, so it became a plain `you` row with the stamp as its first line. The settled reload after `TurnDone` rebuilt the row from history, which is why it vanished when the turn finished.

## Fix

Engine (source of truth): no text a frontend receives carries the stamp; the model frame is untouched.

| where | change |
|---|---|
| `crates/agent/src/clock.rs:79-86` | `clock::strip_owned(String) -> String`: `strip` in place, no reallocation. |
| `crates/agent/src/loop_.rs:1589-1610` | `RestartTurn` replay: `Input.text = clock::strip_owned(text_of(&opener.content))`; `replay: Some(opener)` still hands the stored, stamped row to the frame. `TurnStarted.text` and the title seed are now the human's words, as on the send path. |
| `crates/agent/src/supervisor.rs:6769-6771` | `render`: `HistoryEntry::User.text` is `strip_owned` of the row — history rows reach every frontend stamp-free. |
| `crates/tui/src/model.rs:9446-9448` | `user_entries` strips first: a stamped text (live or replayed) is never a `◆ you` row. Before this, the TUI showed the stamp on *every* history reload of a stamped row, not just on regenerate — fixed by the same line. |

Desktop UI hardening (one function):

| where | change |
|---|---|
| `crates/desktop/ui/src/lib/time.ts:48-60` | `splitTime(text) → { text, at? }` replaces `stripTime`: the rest of the text, plus the stamp's instant (ms) parsed from `now="YYYY-MM-DD HH:MM ±HH:MM"`. |
| `crates/desktop/ui/src/store/reduce.ts:331-358` | `fromHistory` user rows: split; the stamp's `at` becomes the row's `at` when `context.at` is absent (rows before migration 0026); `context.at` wins when present (`fromHistory` overrides). Tool output: `splitTime(...).text`. |
| `crates/desktop/ui/src/store/reduce.ts:705-715` | `TurnStarted`: split before `classify`, before the own-send match (`ctx.sent.delete(text)`), before `userEntries`. |
| `crates/desktop/ui/src/store/reduce.ts:731-741` | `InputInjected`: the queued text is split the same way. |

Not touched: the optimistic row `markSent` adds (`store/sessions.ts:422`) is the user's own keystrokes — never engine-stamped — and the `sent` echo match needs that verbatim text. `MessageUser.tsx` renders the entry text as before; the split happens where the engine's text enters the store, so slash commands, search and `adopt` all see the clean text too.

## Behaviours and their tests

- Rust `crates/agent/src/loop_/regen_stamp_tests.rs` — `regenerate_replays_the_stamp_to_the_model_only`: one turn through the real `Session` actor at the fake provider (the first input of a session is always stamped); `RestartTurn { input: None }` on it; asserts `TurnStarted.text == "First question."`, `supervisor::render` of the frame shows `["First question."]`, the replayed store row is byte-identical to the stamped original, and the model request after the regenerate contains the stamped text.
- Rust `clock::tests::strip_and_parse_round_trip` — `strip_owned` equals `strip`, untouched text returned as is.
- Vitest `lib/time.test.ts` — `splitTime`: leading stamp with and without `since_last`, before a wrapper, with leading whitespace, stamp-only text, malformed instant (strips, no `at`), non-stamp text, `<timer>`, a stamp not at the head.
- Vitest `store/reduce.test.ts` — "a leading `<time …/>` line never renders as user text…": stamped `TurnStarted` → `you` row `hello`; stamped echo of an own send is still skipped; stamped `<message>` wrapper is still no `you` row; stamped queued steer injects as `later`; `fromHistory` uses the stamp's instant as `at` without `messages.at`, the row's `at` when present, no `at` for a plain row.

## Gates (verbatim counts)

Toolchain 1.98.1 (`$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin`).

- `cargo test -p agent` — `test result: ok. 357 passed; 0 failed; 0 ignored` (lib) and `0 passed` (doctests). The new test fails at base with `left: [(0, "<time now=\"…\"/>\nFirst question.")]`, `right: [(0, "First question.")]` (run with the one-line fix reverted).
- `cargo test -p tui` — `test result: ok. 121 passed; 0 failed` (lib), `1 passed` (integration), `0 passed` (doctests).
- `cargo clippy -p agent --all-targets -- -D warnings` — `Finished`, no warnings.
- `cargo clippy -p tui --all-targets --no-deps -- -D warnings` — `Finished`, no warnings. Without `--no-deps`, `-p tui` fails in a dependency at base: `crates/talk/src/policy.rs:580` `clippy::drain_collect` (`self.pending.drain(..).collect::<Vec<_>>()`), untouched by this branch and outside its seams.
- `cargo fmt --all -- --check` — 280 files of pre-existing drift; none of the diffs fall in this branch's hunks (`clock.rs`, `loop_.rs` 1587-1610 / 5612, `loop_/regen_stamp_tests.rs`, `supervisor.rs` 6769, `tui/src/model.rs` 9446). `regen_stamp_tests.rs` was run through `rustfmt` alone.
- `bun run check` (tsc) — clean, no output.
- `CI=1 bun run test < /dev/null` — `Test Files 125 passed (125)`, `Tests 699 passed (699)`.

## Deviations

- `crates/agent/src/supervisor.rs:6769-6771` is outside the listed seams (the brief names the loop, `clock.rs`, the UI and the TUI) but inside the brief's rule 1 ("events and `HistoryEntry`/history rows never contain the stamp"). Three lines in `render`, far from `SetModel`.
- `docs/design/74-time.md` §3 table and §5 updated to name the new seams; `stripTime` no longer exists.

## Open follow-ups

- `HistoryEntry::Tool.result` output still carries the batch stamp from the engine; the desktop strips it (`splitTime`), the TUI shows tool output through its own card and was not changed. Strip it in `render` too if a TUI user reports it.
