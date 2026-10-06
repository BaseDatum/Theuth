# 110 — As built: QUIET-THREAD (+ SLACK-VOICE)

Branch `feat/110-thread`, cut from `a53e378d`. Design 110 §5 (plan message), §6 (quiet thread),
§7 (inbound marker, prompt section), §9 (config), §10 (state), §13 items 2–4. W3 SLACK-VOICE is
folded in (`prompt.rs`).

## Changes outside the seam (integrator: reconcile first)

- `crates/slack/src/commands.rs:1348` — one line in the test fixture `status()`:
  `hidden_deliveries: 0,` so the crate's `--all-targets` build compiles after `SlackStatus` gained the
  field. Additive, test-only.
- **Not done, needs an `api.rs` owner**: the pinned `slack-morphism` (`e2a4b01`) `SlackBlock` enum has
  no `container` and no `plan` variant, and `api::blocks()` (`api.rs:480`) is
  `serde_json::from_value::<Vec<SlackBlock>>`. The live `Api::post_blocks` / `update_blocks` will
  therefore refuse the §5 plan block and the §6 peer container with `ApiError::Transport("blocks:
  unknown variant …")`; the fake API in tests accepts raw JSON, so the tests pass. `alert` exists in
  morphism and goes through. Fix belongs in `api.rs` (raw-JSON `chat.postMessage`/`chat.update` over the
  existing `reqwest` client, or a morphism bump); until then the §6 fallback below posts the peer result
  as markdown and the plan message does not appear (one failed call per `TodoChanged`, logged at debug).

## Files

| file | what |
|---|---|
| `crates/slack/src/channel.rs` | `Event::Peer` + `Bus: Message<PeerDelivered>` + `subscribe::<Bus, PeerDelivered>`; `Out::Peer`; router arm → the receiving session's worker. `State.hidden`, `Channel.plan`, `status().hidden_deliveries`. `Msg.dm` set at both construction sites; `handle_message` pushes `"[slack dm]\n{text}"` / `"[slack channel]\n{text}"` after routing, before the empty check. Worker: `plan: plan::Plan`, `compacted: Option<u32>`; `flush_plan(force)` (tick, `TodoChanged`, `TurnFinished`), `post_blocks_or_markdown`, `alert`, `peer`; `spawned` no longer posts the `↳ spawned` line (now sync); `child` → `alert` for a direct child's `Error`/cancelled turn; `apply`: `Error` → alert + ❌, `MessageDelivered` → counter + debug log, `Compacted` → `self.compacted`, `TodoChanged` → plan. `FakeApi` keeps block payloads (`blocks()`). |
| `crates/slack/src/plan.rs` (new) | `Plan { ts, version, snapshot, dirty, last_edit }` (§11 shape): `update` (higher version, non-empty), `take_due(now, force) -> Option<Edit { ts, blocks }>` (first post at once, edits ≥ `board::BOARD_MIN_GAP`), `posted`, `finish`. `blocks(&TodoSnapshot)`: one `plan` block, title = in-progress item else `Plan · n of m done`, `task_id` = snapshot index, `done→complete`, `blocked→error` + `details` rich_text, dropped omitted, > 50 → 49 + `… n more`. Pure; tests pass explicit `Instant`s. |
| `crates/slack/src/prompt.rs` (new) | `pub const PROMPT` = §7.2 text verbatim (checked by script against the design: equal). Test `prompt_section_starts_with_the_heading_and_names_every_native_block`. |
| `crates/slack/src/ui.rs` | `alert_error(text)` (first line ≤ `ALERT_MAX` 200, level `error`, `plain_text`), `collapsed(title, body)` (container, `is_collapsible`, `default_collapsed`, title/subtitle ≤ 150, one `markdown` child), `clip` now `pub(crate)`; `slack_panel` context line gains `hidden deliveries: n`. |
| `crates/slack/src/board.rs` | `fmt_age` → `pub(crate)` for the `↳ name · finished · <age>` title. No row-lookup helper: the worker already does `rows.iter().find(|r| r.session == …)`. |
| `crates/slack/src/lib.rs` | `SlackConfig { plan: true, deliveries: Deliveries::Hidden, blocks: true, canvases: false }`, `pub enum Deliveries { Hidden }`; `SlackStatus.hidden_deliveries: u64` (`#[serde(default)]`); `render_status` line `hidden deliveries: n`; `pub mod plan; mod prompt;`; `RegisterPrompt { plugin: "slack", text: prompt::PROMPT.to_owned() }` after `RegisterTool(SlackStatusTool)`. |
| `docs/design/110-as-built-QUIET-THREAD.md` | this note. |

## Judgement calls

- **Blocks refused → markdown.** `post_blocks_or_markdown` posts the alert/container and, on any
  `ApiError`, the same content as one markdown message (`⚠️ <line>` / `↳ name · finished · age\n\n<body>`,
  clipped to 12 000). Added because Slack's own reference lists `alert` as modal-only and `container`
  children without `markdown`, and because of the morphism gap above: a child's result or an error must
  not vanish from the thread while the wire shape settles. Drop it once §14 spikes confirm the blocks.
- **Plan lifecycle.** First non-empty snapshot posts at once (not on the next tick); later ones wait for
  the 750 ms tick and the 2 s gap; `TurnFinished` forces the edit, then `Plan::finish` clears `ts` so the
  next snapshot posts a new message (§10). `version` is kept across turns, so a stale snapshot after a
  rewind is still refused. An emptied list (compaction clear) leaves the last message as history and
  posts nothing. A post failure clears `dirty`; the next `TodoChanged` tries again — no retry loop.
- **Peer title age** uses the board row's `since` through `board::fmt_age` (`48s`, `2m05s`, `12m`,
  `1h05m`), not the design's illustrative `2 min`. A `from` without a matching row renders as
  `↳ <from> · delivered` (`peer:<uuid>` or `session:<uuid>`).
- **`Compacted`** is remembered on the worker (`compacted: Option<u32>`) and taken at `TurnFinished`
  where the `Footer` literal is built; the `Footer.compacted` field is W1's, so the hand-off is one
  `// 110 §6: the integrator wires Footer.compacted here` comment and a `let _compacted = …`. Nothing
  is posted.
- **`MessageDelivered`** drops every event (no `from` discriminator, per §6) and logs `from` + char
  count at debug.
- **`Error`** also logs the full message at `warn` (the alert carries the first line only).
- **Marker** uses a newline separator (`[slack dm]\nhi there`): "one marker line prepended". File and
  voice-note lines are untouched (they are pushed after the typed text, unmarked).
- **Test clock.** New worker-driven tests run `#[tokio::test(start_paused = true)]` so the plan
  coalescing cannot be perturbed by a loaded machine; pre-existing tests keep their real clock.

## Contract / seam changes

- `SlackStatus` += `hidden_deliveries: u64` (ts-exported; `cargo check -p rpc --features ts` passes —
  `just ts` regenerates the binding).
- `SlackConfig` += `plan`, `deliveries`, `blocks`, `canvases`; new `pub enum Deliveries`.
- New `pub mod plan` (`Plan`, `Edit`, `blocks`, `TASKS_MAX`); `prompt` is private.
- `ui::alert_error`, `ui::collapsed`, `ui::ALERT_MAX` public; `ui::clip`, `board::fmt_age` `pub(crate)`.
- `Worker::spawned` is no longer `async`.
- Removed renderings: `↳ spawned **name** → <link|thread>`, `⟵ **from**` deliveries,
  `⟲ compacted → epoch n (…)`, `↳ **name** error: …` / `↳ **name** cancelled` markdown, `⚠️ <message>`
  (kept only as the blocks fallback).

## Gates

Run with `PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-110-target`:

```
cargo check -p slack --all-targets                → Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 02s
cargo test -p slack                               → test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
                                                    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
cargo clippy -p slack --all-targets -- -D warnings → FAIL in a sibling crate: `could not compile `tasks` (lib) due to 4 previous errors`
                                                    (crates/tasks/src/packet.rs:406, tools.rs:480-481, tools.rs:741 — pre-existing, not slack)
cargo clippy -p slack --all-targets --no-deps -- -D warnings → Finished `dev` profile [unoptimized + debuginfo] target(s) in 48.45s  (slack itself is clean)
cargo fmt -p slack -- --check                     → exit 1: `Diff in …/bubble.rs:2004`, `Diff in …/commands.rs:394` — both pre-existing at
                                                    base a53e378d (verified with the tree stashed); bubble.rs is W1's, commands.rs is not in
                                                    this seam. My files (channel.rs, board.rs, plan.rs, prompt.rs, lib.rs, ui.rs) are clean.
cargo check -p rpc --features ts                  → Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 53s
```

Morphism probe (throwaway `examples/morph_probe.rs`, deleted): `serde_json::from_value::<Vec<SlackBlock>>`
→ `alert: typed ok`, `container: REJECTED: unknown variant`, `plan: REJECTED: unknown variant`.

Tests added: `channel::tests::{dm_text_is_prefixed_with_the_slack_dm_marker (renamed from
top_level_dm_from_owner_opens_an_owner_root_and_binds_the_thread), mention_text_is_prefixed_with_the_channel_marker_after_agent_routing,
attachment_lines_carry_no_slack_marker, empty_text_with_no_files_still_posts_nothing,
subagent_spawned_posts_the_child_root_and_no_spawned_line (replaces …_and_binds_it),
child_error_is_an_alert_block (replaces child_error_interjects_in_the_parent_thread),
error_is_an_alert_block_and_reacts, message_delivered_posts_nothing_and_counts,
compacted_posts_nothing_and_lands_in_the_next_footer,
peer_delivered_from_a_child_posts_a_collapsed_container_titled_from_the_board_row,
peer_delivered_body_is_clipped_with_a_count,
plan_message_posts_on_the_first_snapshot_and_turn_finished_forces_the_last_edit, plan_false_posts_nothing}`;
`plan::tests::{first_nonempty_snapshot_posts_a_plan_block, title_is_the_in_progress_item_else_progress,
blocked_maps_to_error_with_reason_dropped_omitted, lower_version_is_ignored,
edits_coalesce_at_two_seconds_and_turn_finished_forces_the_last, over_fifty_items_truncates_with_a_count}`;
`prompt::tests::prompt_section_starts_with_the_heading_and_names_every_native_block`;
`lib::tests::config_defaults_and_the_110_keys_parse`. Existing marker-sensitive assertions updated
(`reply_in_a_running_thread_steers`, `mentions_honour_the_channel_list_and_the_allowlist`,
`agent_names_fall_back_to_theuth_…`).

## Deviations from design 110

- §6 / §2: morphism cannot type `container` / `plan`; see "Changes outside the seam". Alert and
  container get a markdown fallback the design does not list.
- §6: peer title age is `fmt_age` format (`2m05s`), not `2 min`; no-row title is `↳ <from> · delivered`
  with the raw endpoint (`peer:<binding-uuid>`), there is no peer *name* on the event.
- §6: `Footer.compacted` not set — field is W1's (`bubble.rs` untouched); hand-off left as a comment.
- §5: `Plan` is always present on the worker (not `Option`); `ts` resets per turn, `version` persists.
  An empty snapshot is ignored rather than rendering an empty `plan`.
- §7.1: `voice_note_lines_carry_no_slack_marker` is exercised as `attachment_lines_carry_no_slack_marker`
  (no `voice` actor in tests; the attachment line is the same unmarked file-line path).
- §13.4: no plugin `start` test exists in `lib.rs`; the `RegisterPrompt` call is by inspection, beside
  `RegisterTool`, mirroring `voice`.
- §9: `blocks` and `canvases` parsed and defaulted, unused (wave 2 as designed).
