# 172 CHANNEL-ORIGIN — ENG as built (D1–D3)

Status: built 2026-11-03 on `feat/172-channel-origin-eng` (main `7d5011a4` + design `b11ec784`). Scope: D1–D3 of `172-channel-origin.md`. D4–D6 (chip, quote cards, icons) are the UI worker's.

## What was built, and where

### Types (D2)
- `ChannelOrigin`, `ChannelVia`, `ChannelChat` live next to `PeerOrigin`: `crates/agent/src/communication.rs:652`, `:671`, `:684`. They are re-exported from `agent` (`crates/agent/src/lib.rs:160`) and registered in the bindings export (`crates/rpc/src/export.rs:89-91`).
- serde and ts-rs follow `PeerOrigin`. Both enums are `rename_all = "lowercase"`. `from` and `title` are `#[serde(default, skip_serializing_if = "Option::is_none")]` plus `ts(optional)`.
- The generated TypeScript matches D2 type for type (printed below). Each of the three files also carries the doc comments as JSDoc.

### Inputs (D3)
- `SendInput.channel` and `Steer.channel` are `Option<ChannelOrigin>` with `#[serde(default)]`: `crates/agent/src/lib.rs:275`, `:299`.
- Every constructor in the workspace was updated. Every caller except the three channels passes `None`: desktop/RPC `send_input`/`steer` (`crates/rpc/src/api.rs`), TUI, talk, meeting, roster, schedule, autoresearch, sandbox takeover, overviews, broadcast, late answers, skills and tests.

### Supervisor routing
- `Agents::human_input`, `input` and `route` take `channel: Option<ChannelOrigin>` (`crates/agent/src/supervisor.rs:1310`, `:1349`, `:1432`).
- The origin also rides `Held::Routed` (`:511`). An input held for a queued child is replayed with its origin.
- `route` hands it to the session actor on `UserInput.channel` (`crates/agent/src/loop_.rs:142`).

### Session actor (native loop)
- `Input.channel` (`crates/agent/src/loop_.rs:162`) is filled from `UserInput` (`:1404`).
- **Stored:** `Turn::take_input` takes the origin (`:4804`) and passes it to `Store::append_input`. The row and its `messages.channel` are written in the same transaction as `input_accepted`. A steer queued mid-turn and later injected goes through the same `take_input`, so it keeps its origin.
- **Live:**
  - `TurnStarted.channel` is the opener's origin (`:1247`, `:1339`).
  - `InputQueued.channel` is the queued input's origin (`:1425`).
  - `InputInjected` names the queued id and has no field, as specified.
- **Regenerate:** `RestartTurn` with `input: None` replays the stored opener and re-reads that row's origin from `Store::channels` (`:1964`), so the new row keeps the chip. An edited opener (`input: Some`) is new desktop text and gets `None`.

### ACP harness children
- `acp_child::Queued.channel` (`crates/agent/src/acp_child.rs:176`) is filled from `UserInput`.
- The user row is written with `Store::append_user` (`:799`), which writes the row and its `channel` in one transaction.
- `TurnStarted` (`:920`) and `InputQueued` (`:1800`) echo the origin. A continuation turn and a synthetic respawn note get `None`.

### Store
- Migration `crates/agent/migrations/0049_channel.sql` adds `messages.channel TEXT NULL` (JSON `ChannelOrigin`), following the `vision` and `advisory` columns.
- New functions:
  - `channel_in`: an UPDATE inside the caller's insert transaction (`crates/agent/src/store.rs:3546`);
  - `Store::append_user` (`:1695`);
  - `Store::channels(id) -> BTreeMap<seq, ChannelOrigin>` (`:1711`). Malformed JSON is an error, as with `advisories`.
- `append_input` gained the `channel` parameter (`crates/agent/src/store/durability.rs:869`).
- `fork` copies the column (`store.rs:2428`).
- The row write is part of the same transaction, so the `sqlite3session` changeset that replicates it carries the column.

### History
- `HistoryEntry::User.channel` comes after `vision` (`crates/agent/src/lib.rs:2596`).
- `render_with` leaves it `None`. `mark_channels` (`crates/agent/src/supervisor.rs:8982`) then sets it from `Store::channels`, the same way `peer` is filled from consumed exchanges. It runs in `LoadSession` (`:7121`) and in `ReadSession` (`:7026`, for MCP/app/notebook readers).
- The model-facing `session_read` rendering ignores the field, so no prompt changed.
- A row stored before 172 has a NULL column and reads back `None`.

### Events
- `AgentEvent::TurnStarted.channel` and `AgentEvent::InputQueued.channel` are optional and absent when `None`: `crates/agent/src/lib.rs:2772`, `:2896`.

## Every channel input path and the origin it sets

Each channel builds one `ChannelOrigin` per inbound message, right before the routing `match`. The same value goes to every branch: steer, existing or bound session, new owner root, new isolated root, and a child reached through a marker reply. Text, voice notes, files/images, forwarded or linked blocks and quotes are all joined into that one input, so they all carry it. The model input is byte-identical to before (D1).

| Channel | Code | via | chat | from | title |
|---|---|---|---|---|---|
| Slack | `crates/slack/src/channel.rs:1462` (`handle_message`) | `slack` | `dm` when `m.dm` (an `im` event), else `group` (an app mention in a listed channel) | Slack user id when the sender is not the owner, else absent | always absent |
| Signal | `crates/signal/src/channel.rs:1274` (`handle_envelope`) | `signal` | `dm` | sender's number when not the owner, else absent | absent |
| Telegram | `crates/telegram/src/channel.rs:1470` (`route_message`, also used by albums) | `telegram` | `dm` for `ChatKind::Private`; `group` for group, supergroup and forum topic | `@username`, else the full name (the rule `remember_chat` uses) when the sender is not the paired owner | `ChatRef.title` in a group, absent in a private chat |

The port traits changed: Slack `Port::send_input`/`steer` (`channel.rs:96`), Signal `Port` (`channel.rs:291`) and Telegram `Port` (`port.rs:69`) each take `channel: ChannelOrigin`. The live implementations wrap it in `Some`.

## Decisions (with reasons)
1. **Storage is a column, not text and not `ContextEntry`.** A column keeps provenance out of the model's input (D1). Keeping it out of `ContextEntry` means `ContextEntry.ts` is unchanged, so the bindings diff is exactly the new fields (gate 5). The column follows the `vision`/`advisory` precedent.
2. **History is filled after rendering (`mark_channels`), not by a new `render_with` parameter.** This mirrors how `peer` is filled and leaves `render_with`'s four callers untouched. The advisor's `rows_since` does not need the chip.
3. **`from` follows what each channel actually has.** The design and the brief assume Signal and Telegram send non-owner text as `name: text`, and give Slack `"ops: hi"` as an example. In the code none of the three prefixes anything; Slack's `"ops: hi"` test is the *owner* addressing a roster agent named `ops`. So `from` is the identity the channel already holds and uses for that user, with no new API call: the Slack user id (the allowlist stores ids), the Signal number (the allowlist stores numbers), and Telegram's `@username` or name (what `remember_chat`/`chat_label` already show).
4. **The Slack `title` is always absent.** The `message`/`app_mention` events carry only the channel id. The name needs `conversations.info`, which the brief rules out, and no channel-name cache exists. A `ponytail:` comment at the site marks the upgrade path.
5. **Regenerate keeps the origin.** Regenerate replays the stored Slack/Telegram text byte for byte, so the chip and the leading-marker stripping in D4 stay consistent.
6. **ACP harness children carry it too.** A bound Slack/Telegram thread can address an ACP child, and its rows are user rows like any other ("child sessions" in the brief).
7. **Test stubs record origins in a separate `origins` list.** The existing `Call::SendInput`/`Call::Steer` variants and every expected text stay byte-identical. The new assertions read `stub.origins()`.
8. **The enum doc comments stay.** ts-rs emits them as JSDoc above `ChannelVia`/`ChannelChat`. The type statements match D2 exactly, and dropping the docs would trip the crate's `missing_docs` lint.

## Tests (new)
- `crates/agent/src/store.rs`, `store::tests::channel_origin_round_trips_and_old_rows_read_none`:
  - a literal pre-172 row (`INSERT INTO messages (session_id, seq, body)`) reads back `channel: None`;
  - a row appended with an origin reads back with it, through `render_with` + `mark_channels`;
  - a fork keeps it;
  - the wire shape: `{"via":"slack","chat":"dm"}`, `from`/`title`/`channel` absent when unset, all lowercase;
  - old `HistoryEntry` and `SendInput` JSON without the field parse to `None`.
- `crates/agent/src/loop_/channel_tests.rs`, `loop_::channel_tests::channel_origin_rides_beside_the_text`, driven through the real `Session` actor:
  - send: `TurnStarted.channel` is set, `Store::channels` holds it at `turn_seq`, and no request to the model contains it;
  - regenerate keeps it;
  - a desktop input stores none;
  - an identified input queued behind a running compaction gets `InputQueued.channel`, then opens the next turn with `TurnStarted.channel` and a stored origin.
- Slack (`crates/slack/src/channel.rs`), each asserting the exact `ChannelOrigin`:
  - owner DM text (`dm_text_is_prefixed_with_the_slack_dm_marker`);
  - steer (`reply_in_a_running_thread_steers`);
  - listed channel mention from an allowed user: `group`, `from` = id (`mention_text_is_prefixed_with_the_channel_marker_after_agent_routing`);
  - audio file / voice-note path (`attachment_lines_carry_no_slack_marker`);
  - every forwarded/linked-message test, through `input_of`.
- Signal (`crates/signal/src/channel.rs`):
  - owner DM text (`hub_answers_by_default`);
  - steer (`eyes_then_check_on_same_message_and_steer_in_flight`);
  - allowed contact: `from` = number (`bus_sends_keep_inbound_aci`);
  - new `voice_note_carries_the_signal_origin`.
- Telegram (`crates/telegram/src/channel.rs`):
  - private DM text (`general_message_in_a_topics_chat_…`);
  - steer (`message_in_a_bound_topic_steers_while_a_turn_runs`);
  - allowed user's DM: `from` = `@u…` (`flat_private_chat_binds_the_chat_key`);
  - supergroup with title `ops`: `group`, `from`, `title` (`plain_group_needs_a_mention_and_strips_it`);
  - new `voice_note_carries_the_telegram_origin`.
- The voice-note tests run without the voice plugin, so the clip becomes an untranscribed attachment line. It travels the same input, which is the property under test.

## Gate results
Env: `export PATH="$HOME/.theuth/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH" CARGO_BUILD_JOBS=4`; `NO_COLOR` unset for test runs.

1. **fmt.**
   - Command: `cargo fmt -p agent -p slack -p signal -p telegram -p rpc -p talk -p sandbox -p schedule -p roster -p meeting -p autoresearch -p tui -p dreaming -p double -- --check`.
   - Result: rc 0. Only files touched by this change were ever reformatted.
2. **clippy.**
   - Command: `BUILD_LABEL=172e build-lock mbx clippy -p agent -p slack -p signal -p telegram -p rpc --all-targets --no-deps -- -D warnings`.
   - Result: rc 101 on the pre-existing `deprecated` `fetch_update` in `crates/agent/src/loop_/thinking_tests.rs:52` (untouched).
   - Rerun with `-A clippy::chunks_exact_to_as_chunks -A deprecated`, as the brief allows: rc 0, zero warnings in the five crates.
   - Pre-existing warnings printed from dependencies: `crates/link/src/types.rs:323` (`deprecated`) and `crates/mcp-server/src/catalog.rs` (rmcp `ServerInfo`).
   - Log: `/tmp/172/eng-clippy2.log`.
3. **tests.**
   - Build: `BUILD_LABEL=172e build-lock mbx test -p agent -p slack -p signal -p telegram -p rpc --lib --no-run`, rc 0.
   - Run, bare: `mbx test -p agent -p slack -p signal -p telegram -p rpc --lib --no-fail-fast`.
   - Results: agent 660 passed / 4 failed, rpc 62/0, signal 70/0, slack 164/0, telegram 141/0. Log: `/tmp/172/eng-test2.log`.
   - The 4 agent failures (`sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks}`, `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused`) all come from `/opt/homebrew/bin/t`, a stale Python worker on `PATH`: "monty worker crashed: unsupported protocol version 6 (server supports protocol version 5)".
   - Rerun of those tests with `/opt/homebrew/bin` dropped from `PATH`: 13/13 ok (`/tmp/172/eng-test3.log`). They are environmental and do not involve this change.
4. **workspace check.**
   - Command: `BUILD_SLOTS=1 BUILD_LABEL=172e build-lock mbx check --workspace --exclude desktop --all-targets --message-format short`.
   - Result: rc 0 (`/tmp/172/eng-check4.log`).
   - Warnings from `crates/` are all pre-existing and in untouched files: `agent/src/loop_/thinking_tests.rs:52`, `codeindex/src/map/export.rs:627`, `link/src/types.rs:323`, `mcp-server/src/catalog.rs`, `mcp-server/tests/catalog.rs`, `theuth/src/mcp.rs`.
   - The first attempt failed only because `bindings/methods.json` was missing before the export (`crates/nodes/src/client.rs:123`).
5. **bindings.**
   - Command: `BUILD_SLOTS=1 BUILD_LABEL=172e build-lock mbx test -p rpc --features ts --lib --quiet export_bindings`, 8 passed.
   - Diff: `/tmp/172/eng-bindings.diff`.
   - Against main's export (`/tmp/172/bindings-main`), `HistoryEntry.ts` and `AgentEvent.ts` differ only by the `ChannelOrigin` import and the new `channel?: ChannelOrigin` fields.
   - Against the UI worktree's bindings (main plus the UI stub), only comment text and the order of one import line differ.

Generated:
```ts
// ChannelVia.ts
export type ChannelVia = "slack" | "signal" | "telegram";
// ChannelChat.ts
export type ChannelChat = "dm" | "group";
// ChannelOrigin.ts (JSDoc per field omitted here)
export type ChannelOrigin = { via: ChannelVia, chat: ChannelChat, from?: string, title?: string, };
```

## Deviations from the brief
- **`from` semantics:** see decision 3. The "label the text already uses" does not exist in any of the three channels, so `from` is the channel's own id or name for the sender.
- **Slack `title`:** always absent (decision 4).
- **Roster-agent addressing (`<agent>: …`):** in Slack (`to_agent`) and Signal/Telegram (`tell_agent`) this creates or mentions a *task* through the `tasks` actor, not a `SendInput`/`Steer`. That path has no user row of this kind to tag, so it carries no origin.
- **The bindings files carry JSDoc** for the enums and fields (decision 8). The type statements are exact.

## Open items
- A Slack channel title needs a `conversations.info` lookup, or a name cache the channel fills when it sees one.
- Remote replication: an engine still on a pre-0049 schema that receives a changeset with the new `messages.channel` column is the same schema-skew case as every earlier `messages` migration. No 172-specific handling was added.
- No live end-to-end run: the desktop app must not be launched on this Mac. The Agents `LoadSession` composition (`Store::channels` + `mark_channels`) is covered at store level, and the actor path by `loop_::channel_tests`.
- D7 items stay out of scope: TUI chip, push notifications, sidebar icons. The TUI destructuring of `HistoryEntry::User` now ends in `..` and ignores the field.
