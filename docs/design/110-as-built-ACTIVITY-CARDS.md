# 110 — As built: ACTIVITY-CARDS (one Slack task card per tool batch)

Branch `feat/110-cards`, cut from `a53e378d`. Design 110 §4, §10, §11, §13 (bubble part);
templates `ToolRunCard.tsx` (`foldRuns`, `summarize`, `isDeliverable`), `telegram/src/cards.rs`
(`Card::flush`, `EDIT_GAP`).

## Files

| file | what |
|---|---|
| `crates/slack/src/bubble.rs` | The card model is now `Batch { id, items: Vec<Item>, closed, dirty, last_sent, card_ts, sources }` with `Item::Call { call_id, name, described, started, finished, error, first_line, lines, inner }` / `Item::Thought { text, started, ms, live }`; `Turn.batches: Vec<Batch>` replaces `tasks: HashMap<call_id, Task>` entirely (a lone call, a deliverable and a batch are all a `Batch`; one `Call` with no `Thought` renders as today's plain card, so the first call's card *becomes* the batch card under the same id). `is_deliverable` mirrors the desktop verbatim. `Batch::{title, status, details, lines, chunk, markdown, due}` render §4.2–4.3 and §4.6; `EDIT_FLOOR = 1.5 s`, `ITEMS_MAX = 60`, `LINE_MAX = 200`, `THOUGHT_MAX = 120`, `SOURCES_MAX = 3`. `Queued::{Card(id), Chunk{chunk, card}}` keeps event order; cards render at flush time when `due`. `describe()` gains the python rule (`awaited_calls` + `fold_names`); `described()` one-lines it (` · ` between `key: value` lines). `Turn::reasoning_done(ms)` and `Turn::compacted(epoch)` added. `TOOL_OUTPUT_CHARS`, `fmt_size`, the `upload_text` call and the `"full output · n KB"` source are gone. Rotation re-emits only live batches (`last_sent = None`) and the live reasoning run; keepalive re-sends the first live batch card. Update mode: `flush_cards` posts one message per batch and `update_markdown`s it; `flush_update_tasks` is gone. |
| `docs/design/110-as-built-ACTIVITY-CARDS.md` | this note |

Nothing else touched: `api.rs` needed no new `Chunk` field or `TaskStatus` helper.

## Judgement calls

- **`Footer.compacted` is a `Turn` method, not a `Footer` field.** `channel.rs:2306` builds `Footer { … }` as a literal without `..`; a new field cannot compile without editing `channel.rs`, which the brief forbids and the `cargo check` gate pins. `Turn::compacted(epoch)` stores the epoch and `finish` appends ` · ⟲ epoch n` to the footer string. W2's one-liner: in the `Compacted { epoch, .. }` arm, `if let Some(t) = &mut self.turn { t.compacted(epoch) } else { self.compacted = Some(epoch) }`, and after `Turn::new` at `TurnStarted`: `if let Some(e) = self.compacted.take() { t.compacted(e) }`.
- **`ReasoningDone` hand-off**: `channel.rs` currently drops `ReasoningDone` (`:2370`). W2's one-liner in that arm: `if let Some(t) = &mut self.turn { t.reasoning_done(ms) }`. Until it lands, a thought's time is measured locally (first delta → the event that ends it) so titles still read `Thought N s`; `ReasoningDone.ms` wins when it arrives.
- **Thought titles follow the desktop's `fmtThought`** rounding: `Thought` alone under a second, `Thought 9 s`, `Thought 1m 12s`, `Thought 2m`, `Thought 1h 2m`; the `💭` line uses the same string (`💭 thought 9 s — …`, `💭 thinking — …` while live).
- **Status `in_progress` also while a thought is live** inside an open batch (design says "while any call is open"): the model is still working; the spinner should spin.
- **`<n lines>` fallback only when the first line is empty**; the "or the output is a list of files" heuristic in §4.3 is not implemented (no rule given; the first line is shown as-is).
- **Inner python calls** (`<parent>.<n>`, `agent::store::inner_call_id`): never items; `ToolCallStarted` appends the name to the parent's `inner` (which then wins over the `code` harvest in the line and the plain title), a failed inner `ToolCallFinished` sets the parent's `error` (sticky through the parent's own finish). They do not count in `tool_count()` (footer `n tools`), so the footer agrees with the card header.
- **A thought that a `ToolCallFinished` ends** (`live = false`, no call after it yet) is popped at batch close like a live one — the pop rule is "last item is a Thought", so `[a, 💭] show` and `[a, b, 💭] text` both leave the thought out of the batch (desktop: a run ends with a tool).
- **Deliverable split with pending followers**: `Pending` events for a round arrive before any `Started`, so a `doc_create` whose args make it a board already has calls after it in the batch; `split` closes the batch before it, inserts the deliverable's own closed batch, and moves the rest into a new open batch (leading thoughts in the rest become their own cards, as 70 excludes leading reasoning from a run).
- **The floor applies to the lone card too** (it is a batch of one): pending → started → finished inside 1.5 s shows the pending card until the floor passes or the batch closes.
- **Update-mode card shape** is uniform: `**<title>**` + fenced itemised lines (a plain card: its one line) + `[host](url)` sources; a pending-only batch posts too (one message per batch regardless), where today's code skipped `Pending`.
- `ponytail:` within one tick, card chunks still go before the text chunk (as today, `finish`'s "cards first" comment): `text → tool` inside 750 ms puts the next batch's card above the sentence. Interleaving text at queue positions needs offset markers that rotation must shift; left for a reported case.

## Contract / seam changes

- `pub` signatures used by `channel.rs` today are unchanged: `Turn::{new, text, reasoning, tool_pending, tool_started, tool_finished, tick, finish, open_stream, tool_count}`, `Footer`, `Target`, `TICK`, `describe`.
- Added `pub fn Turn::reasoning_done(&mut self, ms: u64)` and `pub fn Turn::compacted(&mut self, epoch: u32)` (W2 wires both, one line each, see above); `pub const EDIT_FLOOR`.
- Removed `pub const TOOL_OUTPUT_CHARS` (no user outside `bubble.rs`).

## Gates

Run with `PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-110-target`:

```
cargo check -p slack --all-targets                    → Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.65s (pass)
cargo test -p slack                                   → test result: ok. 114 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s (pass)
cargo clippy -p slack --all-targets -- -D warnings    → error: could not compile `tasks` (lib) due to 4 previous errors — FAIL in sibling crate `crates/tasks` (pre-existing on base a53e378d: `packet.rs:406` collapsible_match, `tools.rs:480-481` doc_lazy_continuation, `tools.rs:741` too_many_arguments); not mine, not fixed.
                                                        `cargo clippy -p slack --all-targets --no-deps -- -D warnings` → Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.80s (slack itself: pass)
cargo fmt -p slack -- --check                         → Diff in crates/slack/src/channel.rs:184, :2351 and commands.rs:394 — FAIL on files I do not own, pre-existing on base a53e378d (verified with `git stash`); `bubble.rs` is rustfmt-clean (`rustfmt --edition 2024 crates/slack/src/bubble.rs`).
```

Tests added (design §13 item 1): `bubble::tests::{two_calls_fold_into_one_card_with_the_first_call_id, text_between_calls_splits_batches, deliverable_show_splits_a_batch_into_two_cards_around_it, pending_doc_create_of_a_board_is_split_out_when_args_arrive, lone_call_renders_as_a_plain_card, reasoning_between_calls_is_a_thought_line_inside_the_card, trailing_reasoning_is_popped_and_becomes_a_thought_card_titled_with_its_seconds, live_title_counts_running_n_of_m_with_backticked_name, done_title_names_failed_tools_in_order, durations_format_seconds_then_minutes, card_is_not_resent_within_the_floor_but_always_on_close, details_is_the_last_finished_first_line_and_sources_cap_at_three, python_inner_calls_fold_into_the_parent_line, list_caps_at_sixty_items_with_a_count, update_mode_posts_one_message_per_batch_and_edits_it, rotation_reemits_only_the_live_batch_card, tools_errors_emits_the_card_only_after_a_failure, reasoning_off_drops_thought_lines, describe_python_lists_inner_tools_with_counts, no_snippet_upload_for_long_outputs, compacted_epoch_lands_on_the_footer, leading_reasoning_is_a_growing_card_retitled_when_text_follows}`. Replaced (design §13: "replaced, not kept"): `tool_lifecycle_emits_three_task_chunks` → `lone_call_renders_as_a_plain_card`; `long_output_is_uploaded_and_linked` → `no_snippet_upload_for_long_outputs`; `reasoning_is_a_growing_card_completed_by_text` → `leading_reasoning_is_a_growing_card_retitled_when_text_follows`; the `errors` half of `tool_cards_none_emits_no_tasks` → `tools_errors_emits_the_card_only_after_a_failure`. `describe_shapes` extended with `is_deliverable`, `fmt_thought`, `thought_title`. All other existing tests (rotation, repair, keepalive, update-mode fallback, `Segment`) kept and passing.

## Deviations from design 110

- §6 / §11 `Footer.compacted: Option<u32>` → `Turn::compacted(epoch)` (reason above; the rendered footer is as designed).
- §4.1 `Thought N s` from `ReasoningDone.ms` → measured locally when the event is not forwarded; `ms` wins when forwarded.
- §4.2 `status: in_progress while any call is open` → also while a thought is live in the open batch.
- §4.3 `<n lines>` "or the output is a list of files" → only when the first line is empty.
- §4.6 footer `tools` "counts calls" → top-level calls only (inner python calls excluded).
- §4.6 update mode: a pending-only batch posts its message at once (one message per batch); the design did not say when the first post happens beyond "on the first item".
