# 110 — as built, MORPHISM-PIN (W6, 2026-10-06)

Branch `feat/110-morphism-pin` off main `5ae70e4a` (waves 1 and 2 landed). Commits: `6418efd8`
pin, `85ae9cf2` typed path, `4dd38759` typed plan builder, plus this note. Nothing pushed; main not
merged.

## The pin

| | before | after |
|---|---|---|
| source | `https://github.com/abdolence/slack-morphism-rust` (upstream) | `https://github.com/skymoore/slack-morphism-rust` (fork, branch `feat/block-kit-2026-blocks`) |
| rev | `e2a4b014b2bedc82c1ccfeae1706e0f2d6a184a3` (2.28.0) | `34de7199f9b715cd33f233722d639c7465976993` (2.29.0 `1945e53c` + 4) |
| features | `["hyper"]` | `["hyper"]` |

The four fork commits over upstream: `c08b6eb` `SlackBlock::Unknown(serde_json::Value)` untagged
last variant (unknown `type`, or a known `type` whose body does not match its model, round-trips
verbatim); `db497a8` typed `container` / `plan` / `data_table` / `data_visualization`; `0c04f86`
`blocks.validate`; `34de719` `canvases.*` + `conversations.canvases.create`.

**Cargo.lock.** `cargo update -p slack-morphism` rewrote 26 hunks: the slack-morphism
`version`/`source` lines plus 25 unrelated dependency re-resolutions (`windows-sys 0.61.2 →
0.60.2/0.59.0/0.48.0`, `windows-core 0.62 → 0.61`, `hashbrown 0.17 → 0.15`, `base64 0.22 → 0.21`,
`getrandom 0.4 → 0.3`, …) in other packages' dependency lists — cargo re-picking among versions
already in the lock, not new crates. Reverted `Cargo.lock` and applied only the two
slack-morphism lines by hand; `cargo metadata --locked` and every `--locked` gate below accept it.
slack-morphism's own dependency list is unchanged between 2.28.0 and 2.29.0. No call site needed a
fix: `cargo check -p slack` passed on the pin before any code change.

## What reverted to typed (`api.rs`)

- `post_blocks` / `update_blocks`: `SlackMessageContent::new().with_blocks(blocks(b)?)` through
  `SlackApiChatPostMessageRequest` / `SlackApiChatUpdateRequest` and `chat_post_message` /
  `chat_update`, as before wave 1. `post()` now takes the built request so `post_blocks` can keep
  wave 1's `unfurl_links: false` (`.with_unfurl_links(false)`); `post_markdown` builds the same
  request without it, as before.
- The three stream methods: `SlackApiChatStartStreamRequest` (`task_display_mode: Timeline`),
  `SlackApiChatAppendStreamRequest`, `SlackApiChatStopStreamRequest` (footer as a typed `context` /
  `mrkdwn` block in `SlackMessageContent`), via `chat_start_stream` / `chat_append_stream` /
  `chat_stop_stream` — morphism's rate configs are the ones the raw path copied
  (`SLACK_TIER2_METHOD_CONFIG`, `SLACK_TIER4_METHOD_CONFIG`). `impl TryFrom<Chunk> for
  SlackStreamChunk` is back; `Chunk::Blocks(Value)` → `SlackStreamChunk::Blocks { blocks:
  blocks(v)? }`.
- `blocks()` is total for any JSON array of objects (an unmodelled block lands in
  `SlackBlock::Unknown`); only a non-array input is `ApiError::Transport("blocks: …")`.
- Deleted: `PostBlocks`, `UpdateBlocks`, `StartStream`, `AppendStream`, `StopStream`, `Posted`,
  `Chunk::wire`, `chunks() -> Vec<Value>`. `api.rs` is 275 lines shorter, 257 of them tests.
- `map_err` untouched: `invalid_blocks` is still `ApiError::Slack("invalid_blocks")`, so
  `post_blocks_or_markdown`'s §6 fallback still fires only on a real Slack answer (`blocks()` cannot
  refuse a block any more).

Tests (`api::tests`): `blocks_chunk_reaches_the_stream_wire_verbatim` (a `data_table` + an
unmodelled block value-equal in the typed `appendStream` / `stopStream` bodies; typed
`SlackApiChatStopStreamResponse` parses an echo carrying them);
`typed_blocks_round_trip_container_plan_and_unknown` (the §6 container shape, a §5 plan with
`rich_text` details, a `callout` → `Container` / `Plan` / `Unknown`, `to_value == input`; typed
`chat.postMessage` / `chat.update` bodies carry them; `SlackApiChatPostMessageResponse` and
`SlackApiChatUpdateResponse` parse a bot-message echo with the blocks);
`surface_goldens_round_trip_through_typed_blocks` (every `crates/surface/fixtures/slack/*.blocks.json`
array through `blocks()` value-equal — see the `header.level` deviation).

## What stayed raw and why

Nothing in `api.rs`. `respond_blocks` / `respond` / `views_open` were never on the typed block
path (response_url is a plain reqwest POST; `views_open` types the view) and are unchanged.

## Typed builders

- `plan.rs::blocks`: `SlackPlanBlock::new(title, Vec<SlackTaskCardBlock>)`, `Status →
  SlackTaskCardStatus`, details via `SlackRichTextBlock::new(vec![SlackRichTextSection::from(text)
  .into()])` (3 lines for the old 8-line `rich_text`), serialised at the end with `json!`. Returns
  the same JSON array; the six plan tests are unchanged.
- `ui.rs` `collapsed` / `card` / `alert_error` stay `json!`: the typed `SlackContainerBlock` with
  `SlackBlockPlainTextOnly` titles and typed children would be longer than the JSON. The
  `collapsed` test now asserts the output types as `SlackBlock::Container` and round-trips, which
  is the field-name check the typed builder would have given.

## Not wired

`blocks_validate`, `canvases_*`, `conversations_canvases_create` compile in (the fork's
`api/mod.rs` has them unconditional; a throwaway probe resolved all three on
`SlackClientSession<SlackClientHyperHttpsConnector>` and was deleted). No live call; canvases need
`canvases:write` + reinstall.

## Gates

`PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-110-target`, sequential, logs
`/tmp/110/pin-<gate>.log`, on `4dd38759` (this note is the only later change).

```
cargo check -p slack --all-targets --locked          PASS  Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.45s
cargo test -p slack                                   PASS  test result: ok. 142 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.33s  (doc-tests: 0)
cargo clippy -p slack --all-targets -- -D warnings    PASS  Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.12s
cargo fmt -p slack -- --check                         PASS  exit 0, no diff
cargo check -p rpc --features ts                      PASS  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 00s
cargo check --workspace --locked                      PASS  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 11s  (pre-existing future-incompat note: block v0.1.6)
```

## Deviations

1. **`header.level` is dropped by the typed path** (fork gap). Slack's `header` block documents an
   optional integer `level` (docs.slack.dev/reference/block-kit/blocks/header-block);
   `surface::slack_blocks` emits it for `Heading`; the fork's `SlackHeaderBlock` has only
   `block_id` / `text`, so a `header` deserialises typed (not `Unknown`) and serialises without
   `level`. Found by the goldens round-trip (`heading-divider-image.blocks.json`), the only lossy
   case across the 15 goldens. Not worked around in theuth; the test strips `level` on both sides
   with a `ponytail:` note. Fork fix: `pub level: Option<u8>` on `SlackHeaderBlock`.
2. `data_table.column_settings` (also a Slack field) is likewise absent from `SlackDataTableBlock`
   and would be dropped; theuth never emits it, the old test's fixture did. Same fork fix shape.
3. `post()` signature changed to take the request (so `unfurl_links: false` survives the revert);
   not "as before wave 1" byte-for-byte.
4. Cargo.lock applied by hand (two lines) instead of accepting `cargo update`'s 26-hunk rewrite.
5. No live Slack call; the first live `plan` / `container` post still pending from INTEGRATE §14.
