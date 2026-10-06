# 110 — as built, SLACK-BLOCKS-INTEGRATE (W5, wave 2, 2026-10-06)

Branch `feat/110-blocks-integrate`, cut from `810bf83b` (wave 1 merged). Design 110 §8 (integration
half), §9, §14.5 follow-ups, §7.2 clause swap; the INTEGRATE note's NEEDS list. One worker, owning
`crates/slack` and `crates/surface`; `crates/slack/manifest.yaml` untouched (canvases deferred).

## §14.5 spike (orchestrator, 2026-10-06)

## §14.5 spike results — `blocks.validate` in MESSAGE context, run by the orchestrator 2026-10-06 13:58 with the workspace bot token (read-only validation; nothing was posted)

| payload | result |
|---|---|
| W4 golden `alert.blocks.json` (first two alert blocks) | ok — the "modals-only" doc note is NOT enforced by the validator for messages |
| W4 golden `card-collapsible.blocks.json` (three `container` blocks) | ok |
| W4 golden `table-small.blocks.json` (`data_table`) | ok |
| W4 golden `bars.blocks.json` (`data_visualization`) | ok |
| W4 golden `metric-grid.blocks.json` (`section` with `fields`) | ok |
| W4 golden `checklist.blocks.json` (`rich_text`) | ok |
| a `plan` block (title + 3 tasks: complete / in_progress with rich_text details / pending) | ok |
| W2's §6 peer container: `container` with ONE `markdown` child | **invalid_message** — `/blocks/0/child_blocks/0/type`: `unsupported type: markdown`; allowed container children are exactly `divider, image, section, contact_card, callout, canvas_table, actions, video, header, context, table, layout` |
| the same container with two `section` children (`mrkdwn` text) | ok |
| probe: a bare `callout` block | invalid — requires `child_blocks` and `background_color` (so `callout` is a container-like block, not an Alert) |

Consequences taken here: `ui::alert_error` stays an `alert` (accepted in messages); the W4 goldens
stand as recorded (no re-record); the peer container's child changes (below); `callout` is not used.

## Files

| file | what |
|---|---|
| `crates/slack/src/ui.rs` | `collapsed(title, body)`: body → `markdown::plain` → `markdown::chunks(…, 2900)` (newline, then whitespace, then hard cut) → up to 9 `section` children with `plain_text` text, then one `context` `… n more chars` when chunks were dropped (10-child cap); an empty body yields one `—` section. New `card(title, subtitle)`: one `card` block, `plain_text` title/subtitle clipped at 150. Test `collapsed_body_is_plain_sections_capped_at_ten_children`. |
| `crates/slack/src/api.rs` | `Chunk::Blocks(serde_json::Value)`; `Chunk::wire(self) -> Value` replaces `TryFrom<Chunk> for SlackStreamChunk` (`Text` → `{type: markdown_text}`, `Blocks` → `{type: blocks, blocks}`, `Task` → morphism's `SlackStreamChunk::TaskUpdate` serialised, so source URLs are still parsed). `StartStream` / `AppendStream` / `StopStream` request structs; the three methods go through `http_session_api.http_post::<_, Posted>` with morphism's own rate configs (`SLACK_TIER2_METHOD_CONFIG` start/stop, `SLACK_TIER4_METHOD_CONFIG` append) and the `APPEND_GAP` pacer. `stopStream`'s footer is the same `context`/`mrkdwn` block as JSON; `session_status` is still validated through `session_status()` and serialised as the morphism enum. Tests: `task_source_urls_are_validated_before_sending` (over `wire`), new `blocks_chunk_reaches_the_stream_wire_verbatim`. |
| `crates/surface/src/fences.rs`, `lib.rs` | `pub fn json_render_fences(text) -> impl Iterator<Item = (Range<usize>, &str)>`: closed `json-render` fences, `(range, body)`. The only new export; `flatten`/`slack_blocks` unchanged. |
| `crates/slack/src/bubble.rs` | `BLOCKS_MAX = 50`. `Bubble { blocks_sent, blocks_posted }`. `Bubble::segments(range, blocks, budget) -> Vec<Chunk>`: text runs (through `flatten`) around one `Chunk::Blocks` per fence that `slack_blocks` renders within the remaining budget; `Bubble::split(range, blocks) -> (String, Vec<Value>)` for update mode (text without the rendered fences + their payloads). `Turn.blocks` (new `Turn::new` parameter). Stream: `try_start_stream`, `flush_stream`, `rotate`, `finish` extend their chunk list with `segments(sent..end)`; `blocks_sent` grows on success. Update: `open`, `flush_update`, `rotate`, `finish` edit the split text and then `post_fence_blocks` posts payloads from index `blocks_posted` on, stopping at the first failure (retried next flush, in order). Repair of a dead stream uses the flattened text (`out`) as before. Tests: `json_render_fence_streams_flattened_when_blocks_are_off` (the old test, config off), `json_render_fence_streams_as_a_blocks_chunk_between_text`, `unsupported_fence_streams_flattened`, `update_mode_posts_fence_blocks_after_the_edit`, `fence_blocks_cap_at_fifty_per_bubble`. |
| `crates/slack/src/channel.rs` | `Channel.blocks` from `SlackConfig.blocks`, passed to `Turn::new`. `artifact()`: `Kind::App` → `ui::card(title or "App", APP_NOTE)` via `post_blocks`, no read, no upload; `show` unchanged (upload, title as comment). `Worker.board_creates: HashMap<call_id, title>` filled at `ToolCallStarted` when `name == "doc_create" && args.kind == "board"` (desktop's `isBoardCreate`), consumed at `ToolCallFinished`: not an error and output `created board <id> …` → `ui::card("Board · <title>", "Open in theuth: notes/<id>")`. `APP_NOTE` = `Interactive in theuth · not runnable in Slack`. Test fake `texts()` renders a blocks chunk as `{n blocks}`. Tests: `app_for_a_bound_session_posts_one_card_and_no_upload` (replaces `…uploads_its_source…`), `board_create_posts_a_card_with_the_notes_path_as_text` (note, failed board, successful board → exactly one card, no `theuth://`), peer tests updated to `section` children. |
| `crates/slack/src/prompt.rs` | The clause swap, byte-exact otherwise; test asserts the new clause and the absence of `monospace box`. |
| `crates/slack/src/lib.rs` | `SlackConfig.blocks` doc: read by the bubble; `canvases` doc: deferred, needs `canvases:write` + reinstall. No code change. |
| `docs/design/110-as-built-SLACK-BLOCKS-INTEGRATE.md` | this note. |

## Judgement calls

- **Container children are `plain_text` sections**, not `mrkdwn` (the spike validated `mrkdwn`): the
  brief asks for `markdown::plain` + `plain_text` so line breaks survive and no mrkdwn escaping is needed
  for arbitrary tool output. The subtitle is still the raw first line (clipped), as before.
- **`… n more chars` counts the dropped chunks' characters**, not the newlines `chunks` consumed at the
  cuts; the number is a hint, not a byte count.
- **Budget is per bubble in both modes.** Stream mode: `blocks_sent` is the sum of blocks in the
  chunks that `appendStream` accepted. Update mode: `split` recomputes from the start of the bubble
  with a fresh 50 so the result is a pure function of the text (the same fence always lands on the
  same side of the cap), and `blocks_posted` counts the payloads already posted. A rotation starts a
  new bubble and a new budget, matching "per message".
- **Update-mode text drops a rendered fence** instead of keeping its code block beside the separate
  blocks message; otherwise the same table would appear twice. A fence that flattens stays in the
  text, as before. The posts follow the edit; a failed post leaves `blocks_posted` where it was and
  the next flush retries from there, so order holds.
- **Repair text is the flattened whole** (`out`), even when blocks had been streamed: `chat.update`
  with `markdown_text` replaces the message, so the native blocks would vanish anyway; the code
  block is the honest fallback.
- **`Chunk::Task` still goes through morphism's typed chunk** for its URL parse and field names; only
  the envelope is ours. `SlackApiChat{Start,Append,Stop}StreamRequest` are no longer used in `api.rs`.
- **A card is title + subtitle only**: the reference requires `type` and one of
  `hero_image | title | actions | body`; no icon, no body, no actions (nothing to click in Slack).
- **Board detection happens in `channel.rs`, not `bubble.rs`**: the bubble already splits the
  deliverable's activity card (W1); the card is a thread message like the §6 peer container, posted
  from the same worker so it lands after the tool's own card in event order. The title is the tool's
  `args.title` (what the user asked for), `Board` when empty; the id is the first token after
  `created board `, matching the desktop's `createdBoard` regex.
- **`app` no longer reads the artifact file**: nothing is uploaded, so a missing `.openui` path cannot
  suppress the card (the test uses a non-existent path on purpose).
- `json_render_fences` lives in `surface` rather than re-finding fences in the bubble: the CommonMark
  rules (indent, marker run length, nesting) are already there and `flatten` uses the same iterator,
  so the text and block paths agree on what is a fence.

## Gates

`PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-110-target`, sequential under `nohup`
(`/tmp/110/w5-gates.sh`, logs `/tmp/110/w5-<gate>.log`), at the final commit:

```
cargo check -p slack -p surface --all-targets                        Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 50s          (exit 0)
cargo test -p slack                                                   test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (exit 0)
cargo test -p surface                                                 test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   (exit 0)
cargo clippy -p slack -p surface --all-targets -- -D warnings         Finished `dev` profile [unoptimized + debuginfo] target(s) in 59.91s          (exit 0)
cargo fmt -p slack -p surface -- --check                              no diff                                                                        (exit 0)
cargo check -p rpc --features ts                                      Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 08s          (exit 0)
cargo check --workspace                                               Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 23s          (exit 0)
```

## Deviations from the brief / design

- Container child text is `plain_text` as the brief says; design §6 said `markdown` and the spike
  validated `mrkdwn` — `plain_text` chosen for line breaks without escaping.
- Design §8 says "≤ 50 blocks per chunk"; the brief's "50 per message overall" is what is enforced
  (one budget per bubble, both modes). `slack_blocks`'s own `TooManyBlocks(>50)` still flattens a
  single oversized fence.
- Design §8 "the blocks are a separate `post_blocks` message after the bubble's edit" is implemented
  with the fence removed from the edited text (above), which the design does not spell out.
- `Turn::new` gained a `blocks: bool` parameter (7 arguments); no `Target` field, since it is a
  rendering switch, not an address.
- No `cards` field on `StopStream`/`threads.json` and no in-tick ordering change (both design §10 /
  INTEGRATE open items, out of this brief).

## Open

- **Canvases** (`slack.canvases`): parsed, unused; needs `canvases:write` in `manifest.yaml` and a
  reinstall; `manifest.yaml` untouched here.
- **`theuth://` handler** (design §16): the board card shows `Open in theuth: notes/<id>` as plain text
  until the desktop registers the scheme; then the card can carry a link (or a button).
- **Telegram/Signal parity** (design §16, INTEGRATE NEEDS): `describe`, inbound markers, prompt
  sections, quiet `MessageDelivered`.
- **PNG for Graph / Sequence / mermaid** (30 §9 v1): those fences still flatten to source.
- **Live render check** in a real DM — `alert`, `container` (with the new section children), `plan`,
  `data_table` inside a stream's `blocks` chunk, plus a 51-block chunk's behaviour — Sky after the
  engine rebuild; no Slack call was made in this wave.
- `slack_blocks` renders `Text`/`Badge` as `section`/`context` inside containers; whether a `blocks`
  chunk inside a *stream* accepts every block type the validator accepts for a message is part of the
  live check.
