# 110 — as built, INTEGRATE (2026-10-06)

Integration of design-110 wave 1 on `int/110-wave1`: `feat/110-thread` (W2 QUIET-THREAD + W3
SLACK-VOICE), `feat/110-cards` (W1 ACTIVITY-CARDS) and `feat/110-blocks` (W4 SLACK-BLOCKS-RENDER),
all cut from `a53e378d`, merged without conflicts (`acf5db61`, `848e5d0b`), then `main` `5044521e`
(`e065ae55`). This note covers the commits after that merge: `48e33142` style, `1a1548d9` wiring +
raw blocks, `48f34539` tasks clippy, `16757cad` roster clippy, plus this note.

## Cross-seam fixes made at integration

1. **`Compacted` → footer** (`channel.rs` ↔ `bubble.rs`). W1 replaced the design's `Footer.compacted`
   field with `Turn::compacted(epoch)` (its note explains why: the `Footer` literal lives in
   `channel.rs`); W2 held the epoch on the worker behind a `// 110 §6` comment and a dead
   `let _compacted = self.compacted.take()`. Wired as W1 spelled out: the `Compacted { epoch, .. }`
   arm calls `t.compacted(epoch)` when a turn is open, else stores it on `Worker.compacted`; the
   `TurnStarted` arm hands a stored epoch to the fresh `Turn` right after `Turn::new`. The dead take
   and both hand-off comments are gone; `Worker.compacted` stays because compaction between turns
   is real (`Compacted.turn` is an `Option<TurnId>`, `loop_.rs:4940-4948`; a manual compaction
   carries `None`). No message is posted; the footer reads
   `model · N in / M out · time · n tools · ⟲ epoch n`.
2. **`ReasoningDone` → `Thought N s`**. `channel.rs` dropped the event (it sat in the "nothing to
   say" arm). Now `ReasoningDone { ms, .. }` → `t.reasoning_done(ms)`, so a trailing thinking card
   is titled from the model's own time rather than W1's local measurement (which stays as the
   fallback when the event never arrives — e.g. a harness that does not publish it).
3. **Raw-JSON block path** (`api.rs`), below.
4. The channel-test `FakeApi` now records task-card titles as `[title]` inside the stream texts
   and the `stop_stream` footer after ` | `, so worker-driven tests can see both hand-offs land.
   Tests: `channel::tests::{compacted_posts_nothing_and_lands_in_the_footer (replaces
   …_in_the_next_footer; three turns: held epoch, mid-turn epoch, none carried over),
   reasoning_done_titles_the_thought_card_with_its_seconds}`, both `start_paused`, both fail with
   the arms stubbed out (checked). `peer_delivered_from_a_child_posts_a_collapsed_container_…` now
   also asserts no markdown fallback followed the successful container post.

## The raw-JSON block path (`api.rs`)

**Why.** The pinned slack-morphism (`~/.cargo/git/checkouts/slack-morphism-rust-…/e2a4b01`,
`src/models/blocks/kit.rs:20-61`) types `SlackBlock` as a closed `#[serde(tag = "type")]` enum:
section, header, divider, image, actions, context, input, file, video, markdown, rich_text, table,
task_card, alert, card, carousel, context_actions, share_shortcut, event. No `container`, `plan`,
`data_table`, `data_visualization`, no catch-all. `api::blocks()` is
`serde_json::from_value::<Vec<SlackBlock>>`, so the live `post_blocks` refused exactly the §5 plan
block and the §6 peer container with `Transport("blocks: unknown variant …")` before any HTTP call;
W2's `post_blocks_or_markdown` hid it behind a markdown fallback and the plan message never appeared.

**What.** `SlackApi::post_blocks` / `update_blocks` on `Api` no longer go through
`SlackMessageContent`. They serialise our own request structs and send them over morphism's generic
`SlackClientHttpSessionApi::http_post::<RQ, RS>(method, &req, rate_control)` (`src/client.rs:318-340`
in the checkout), which is the same connector path every typed method uses: the envelope `ok`/`error`
check (`hyper_tokio/connector.rs:143-159`), the rate controller, the `slack.api` span via `run`, the
per-method counters, and for `chat.update` the `UPDATE_GAP` pacer. `map_err` is unchanged, so
`invalid_blocks` still arrives as `ApiError::Slack("invalid_blocks")` and the §6 fallback now only
fires on a real Slack answer.

```jsonc
// chat.postMessage  (PostBlocks; rate ctl CHAT_POST_MESSAGE_SPECIAL_LIMIT_RATE_CTL)
{ "channel": "C1", "thread_ts": "1.1" /* omitted when None */, "blocks": <caller's Value, verbatim>, "unfurl_links": false }
// chat.update       (UpdateBlocks; rate ctl SLACK_TIER3_METHOD_CONFIG, after pace("chat.update", UPDATE_GAP))
{ "channel": "C1", "ts": "1.1", "blocks": <caller's Value, verbatim> }
// response, both   (Posted; every other field — incl. the echoed message.blocks — ignored)
{ "ts": "1.2" }
```

`pub(crate) fn blocks()` (typed) is kept: `channel::ack` answers a slash command with a typed
`SlackCommandEventResponse` and still needs it. `approvals.rs`, `asks.rs`, `ui.rs`, `plan.rs` only
produce JSON for `post_blocks`/`update_blocks`/`respond_blocks` and now reach the wire untouched.

Test `api::tests::raw_blocks_go_over_the_wire_verbatim_and_only_ts_is_read_back`: a `container`
block is refused by the typed path, serialises byte-for-byte inside `PostBlocks` (thread and
top-level) and `UpdateBlocks`, and the response
`{"ok":true,"channel":"C1","ts":"1.2","message":{"blocks":[{"type":"container",…}]}}` parses to
`ts = "1.2"`.

Not exercised live: no Slack call was made. The first live post of a `plan` / `container` block will
show whether Slack accepts them in a thread message (§14 spikes 3 and 4 were skipped).

## Formatting and clippy commits

- `48e33142 style(slack): rustfmt` — `cargo fmt -p slack` + `-p surface`: one hunk in
  `commands.rs:397` (the `Cancel` ask). The drift the workers reported in `channel.rs` /
  `bubble.rs` was already clean on the merged tree.
- `48f34539 chore(tasks): clippy lints (rust 1.98)` — `packet.rs:405` `collapsible_match` →
  guard form (the `_ => {}` arm keeps the fall-through, so behaviour is identical);
  `tools.rs:478-481` `doc_lazy_continuation` → the field doc rewrapped so no line starts with `+`
  (text unchanged); `tools.rs:741` `too_many_arguments` on `review_gate` → `#[allow]` with the
  one-line justification the workspace uses elsewhere (`agent/src/loop_.rs:2930`,
  `tools/src/registry.rs:1500`); `ledger.rs` tests ×4 `cloned_ref_to_slice_refs` →
  `std::slice::from_ref(&t.id)`. `cargo fmt -p tasks -- --check` still reports `place.rs:520/527`,
  pre-existing on main and outside this wave's gate; left alone.
- `16757cad chore(roster): clippy type_complexity (rust 1.98)` — not in the brief: `main`
  `5044521e` brought `roster/src/attention.rs:756` (a nine-tuple row type) into the dependency
  graph of the clippy gate. One `type ShowRow = (…)` alias; no code path changed.

## Gates on the merged tree

`PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-110-target`, sequential under `nohup`, logs in
`/tmp/110/<n>.log`. 1–3 and 5–7 ran at `48f34539`; 4 failed there on `roster` (text above), passed
at `16757cad` (the only diff between the two is the alias, which gate 4 compiles).

```
cargo check -p slack --all-targets                                   Finished `dev` profile [unoptimized + debuginfo] target(s) in 49.12s
cargo test -p slack                                                  test result: ok. 134 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
cargo test -p surface                                                test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
cargo clippy -p slack -p surface -p tasks --all-targets -- -D warnings   Finished `dev` profile [unoptimized + debuginfo] target(s) in 25.59s  (16757cad)
cargo fmt -p slack -p surface -- --check                             exit 0, no diff
cargo check -p rpc --features ts                                     Finished `dev` profile [unoptimized + debuginfo] target(s) in 55.67s
cargo check --workspace                                              Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 18s
```

Not run: `cargo test --workspace`; `just ts` (the `SlackStatus.hidden_deliveries` binding
regenerates on the next run; `cargo check -p rpc --features ts` covers the export).

## NEEDS for wave 2 (W5 SLACK-BLOCKS-INTEGRATE)

- **`Chunk::Blocks` in `appendStream` needs the same raw approach.** Morphism *does* have a
  `SlackStreamChunk::Blocks { blocks: Vec<SlackBlock> }` variant (`src/api/chat_stream.rs:138-140`),
  but it is typed over the same closed `SlackBlock` enum, so `data_table`, `data_visualization` and
  `container` fail `chunks()` (`api.rs:148`) exactly as `post_blocks` did. `start_stream` /
  `append_stream` / `stop_stream` must build their chunk list as `serde_json::Value` (`Text` →
  `{type: markdown_text}`, `Task` → the existing `SlackStreamChunk::TaskUpdate` via
  `serde_json::to_value`, `Blocks(v)` → `{type: blocks, blocks: v}`) and send `{ channel, ts,
  chunks, … }` through `http_session_api.http_post` with a `{ ts }` response — the three typed
  request structs (`SlackApiChatStartStreamRequest` etc.) are then unused in `api.rs`. Keep
  `task_source_urls_are_validated_before_sending` by validating `Source.url` before building the
  value.
- **§14.5 `blocks.validate` spike list**, before the `surface/fixtures/slack/*.blocks.json` goldens
  are trusted: (1) `alert` in a *message* — docs.slack.dev `alert-block` says "currently supported
  only in modals, not messages or Home tabs" (also SLACK-BLOCKS-RENDER note 5). That hits wave 1
  too: `ui::alert_error` (§6 `Error` / child error) is an `alert`; if a thread post answers
  `invalid_blocks`, every error falls back to `⚠️ <line>` markdown, which is the pre-110 rendering.
   Decide: `section` (mrkdwn) with a `⚠️` prefix, or `context`. (2) `container` with
  `is_collapsible` / `default_collapsed` and a `markdown` child in a thread (`ui::collapsed`),
  (3) `plan` block in a thread with `pending` as a task status and 50 tasks (`plan::blocks`),
  (4) `data_table` with `raw_number` cells and `caption`, (5) `data_visualization` bar + line with
  `axis_config.categories`, two per message, (6) `rich_text_preformatted` / `rich_text_list` /
  `rich_text_quote` with `link` elements inside a `container`, (7) `image` with `title`, (8) a
  51-block payload: error or "dropped with a warning". Record which the validator rejects; re-record
  the goldens from accepted shapes (`UPDATE_GOLDEN=1 cargo test -p surface slack::`).
- **`theuth://` handler** (design §16): still open. Board rows and result cards will carry
  `theuth://sessions/<id>` links once §8's card blocks land; the desktop registers no scheme and no
  `onOpenUrl` consumer (`desktop/src/lib.rs:74`), so the links open nothing. Needs its own note; W5
  should render them as plain text until then.
- **Telegram/Signal parity** (design §16): `describe` in `telegram/src/cards.rs:362` and
  `tui/src/model.rs:1555`; `[telegram …]` / `[signal …]` inbound markers with `## Telegram` /
  `## Signal` prompt sections (`slack/src/prompt.rs` is the template); Telegram's
  `MessageDelivered` handler (`telegram/src/worker.rs:550`) going quiet with a counter the way
  `SlackStatus.hidden_deliveries` does. After `fix/98c-telegram-wedge`.
- From the workers' notes, still open for W5: `slack.blocks` / `slack.canvases` are parsed and
  unused (§9); the `Chunk::Blocks` `out` split and update-mode `post_blocks` in `bubble.rs`; the
  deliverable `card` blocks in `channel.rs`; the §7.2 clause swap in `prompt.rs`; the §4.3
  "list of files" heuristic (not implemented, W1); the §4.6 in-tick ordering `ponytail:` in
  `bubble.rs` (card chunks before the text chunk of the same tick).

## Deviations from the brief

- `Posted` reads only `ts`; `channel` would be a never-read field (`dead_code`, red under
  `-D warnings`) since no caller uses it.
- `PostBlocks` has no `text` fallback field: the trait has no `text` parameter, so it would always
  serialise as absent. Add it with the parameter when a caller has notification text to give.
- `Worker.compacted` kept (not "dead hand-off state"): it is the between-turns hold W1's note asks
  for; the dead part was the `TurnFinished` take.
- `chore(roster)` commit outside the brief's crate list, reason above.
