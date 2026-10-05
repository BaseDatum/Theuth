# 98 — As built: RENDER (worker, bubble, cards, board)

Branch `feat/98-render`, cut from `a46fc577` (stage-1 contract). Design 98 §8, §9.3, §11;
contract §2.6 (`worker.rs`) and §4 Deviations.

## Files

| file | what |
|---|---|
| `crates/telegram/src/worker.rs` | `spawn` → one task per thread: `select! { rx.recv() => apply, tick(1 s) => flush }`. `Worker { turn, marks, voice, title_sent, context_size, named_by_user, board, outputs }`, `Turn { started, model, voice, clip_tail, bubble, card, draft_seq, tools, unspoken, names }`. Events (§8.9 table), marks/reactions (`REACTIONS`), typing every 4 s, title follow, voice clips, `out:` output store (1 h), board routing, `Shutdown`. Tests (design §15 item 3) drive `Worker::event/apply/tick` directly under `start_paused`. |
| `crates/telegram/src/bubble.rs` | `Bubble { mode: Draft{id} \| Edit{msg}, text: markdown::Text, sent, dirty, last_call, opened, reasoning }`; `flush` (draft re-send / keepalive / edit-mode committed sends), `close` (finish + one `send_rich` with footer), `split_at_cap` (30 000 B, block boundary or fence re-open), `Reasoning` (indicator + `<blockquote expandable>`), `send_rich_or_plain` (400 → `markdown::plain` + `chunks(4096)` + `send_text`), `fallback` (once per process per reason), `note_open`, `report`, `footer_line`/`footer`/`fmt_*`, `clip`. |
| `crates/telegram/src/cards.rs` | `Card { msg, items, dirty, last_edit }`; `pending`/`started`/`finished`/`think`/`adopt`; `render` (§8.4 shapes, `<details open>` while running, `… n more chars` + `<tg-button … data="out:<call_id>">`, ≤ 3 URLs); `flush` (send once, edits ≥ 1.5 s apart when dirty, 30 s running refresh, `last` forces the final edit); `describe` (TUI rule, copied from slack); `TOOL_OUTPUT_CHARS = 2_500`. |
| `crates/telegram/src/board.rs` | `Board` over `Vec<(SessionId, Row, idle)>`; `render` via `ui::table`, 8 rows + `… n more`, `**agents** · all idle`; `flush` (send once, edits ≥ 2 s apart; the all-idle edit releases the message); `close`. |
| `crates/telegram/src/lib.rs` | three `pub mod` lines (`board`, `bubble`, `cards`). |

## Judgement calls

- **Bubble opens with its first draft call** (not on the next tick): the first `TextDelta`/`ReasoningDelta` sends `draft(raw, can_stop: true)` at once; later deltas coalesce until the tick. A `dirty` flag (not only `raw().len() > sent`) drives re-sends so reasoning-only changes also go out.
- **Reasoning placement**: a run that streams while no card is live lives in the bubble (`Bubble.reasoning`) and renders above the prose; while a card is live it is a `<details><summary>thinking</summary>` item. A reasoning-only bubble (no prose) at the first `ToolCallPending` is moved into the card (`Card::adopt`) instead of being persisted alone; at `TurnFinished` it is persisted as the quote alone. `cfg.reasoning == false` keeps `Reasoning` with empty text (indicator only).
- **`tools: errors` / `none` do not close the bubble at `ToolCallPending`**: a card is a block boundary only when it actually opens (the error result). With `none` the prose before and after a tool batch is one bubble.
- **Footer**: on the bubble closed at `TurnFinished`; else on the card's final edit (covers "text, then tools, then end" too, since a batch ends at `TurnFinished`). No bubble and no card → no footer (the reaction is the status).
- **Rotation** is byte-based (`raw().len() >= 30_000`), conservative vs the char cap. `committed > 0` and the tail under the cap → split at `committed()`; otherwise cut at the last newline before the cap, closing and re-opening an open fence. No newline at all → no split (ponytail ceiling; Telegram's 32 768 cap would refuse the send and the plain fallback takes it).
- **Edit mode plain fallback**: after a 400 on the first committed send the bubble flags `plain` and sends only `raw[sent..committed]` as plain chunks per tick (no message to edit).
- **Edit mode gone message** (`is_gone_message`): `msg = None`, `sent = 0`, re-sent whole on the same tick (`rotate`).
- **Mark before `TurnStarted`**: `Worker.voice` remembers the last mark's `voice` and seeds the next `Turn`.
- **`Error` event**: closes bubble (no footer) and card, posts `⚠️ …`, reacts 😱 but keeps the marks (the following `TurnFinished { error }` reacts again, idempotent).
- **Child topics** rename to `↳ <name> · <title>` (≤ 128 chars) when `child_name` is set.
- **Reactions** failures are logged at `debug`, not tracked per chat (no state for "once per chat").
- **`WorkerCtx.rt`** is unused by the renderer (kept for the contract).
- `OpenMsg` notes: bubbles (Edit mode) and cards and the board report `open: true` on send and `open: false` on the final edit / release.
- Typing: `last_typing` seeded `now - 4 s` so the first idle tick after `TurnStarted` may type; `TurnStarted` itself types.

## Contract changes

None to signatures. Added pub consts (`TICK`, `TYPING_GAP`, `OUTPUT_TTL`, `DELIVERED_MAX`, `TITLE_MAX`, `REACTIONS`) and `pub fn brand(ctx, md)` in `worker.rs`; new pub modules `bubble`, `cards`, `board`.

## NEEDS

- `NEEDS(ROUTE)`: reply-threading of a child's messages to its marker (§9.2) — the worker brands the first line only; `reply_to` stays `None`.
- `NEEDS(ROUTE)`: `channel::repair_open` can append `· interrupted` only if it keeps text; the worker reports `OpenMsg` ids only (unchanged from the CONTRACT note).
- `NEEDS(COMMANDS)`: `ui::guard` is the identity in stage 1; the worker sends `raw()` unguarded (design §8.1 (a) is COMMANDS' seam).

## Gates

(filled below)

## Remaining

(filled below)
