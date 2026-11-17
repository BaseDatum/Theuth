# cells-midturn — a finished chat cell runs at once, even while the turn runs

Branch `fix/cells-runnable-mid-turn`, from `452a0f8d6`.

## Sky's words (2026-11-17 11:47)

> another small bug, when an agent puts a cell in the chat, I can't execute it or anything in it until the agent's turn
> stops, that doesn't make any sense. if the cell is completed, I should be able to immediately execute it if I want to.

Overrules 171 §4.1/§5 ("ready when the reply ends"; "Runnable only when … `running` is false for the turn that wrote
the message").

## What changed

- **D1, no turn gate** — `components/cells/CellFence.tsx`: `pending = !!session && (isIncomplete || !ordinals.entry)`.
  Run, the line ▶, edit, Run all (`register`, `RunbookBar runnable`) all follow `pending`, so all open together.
  `CellOrdinals.turnRunning` stays (MessageAssistant, off-limits, still passes `open || turnRunning`); its doc says it
  gates nothing. No other cell code read `turnRunning` (grep).
- **D2, the stored seq while the turn runs**
  - `store/reduce.ts` `stampStored(entries, history, turn)` (pure), `unstampedKey(slice)` and the R1 give-up
    `stampWanted` / `stampTried`.
  - `store/sessions.ts` action `stampTail(id, page, fresh)`: applies it, dropped when `historyGeneration` moved.
  - `routes/c.$session.tsx` `useSessionLoad`: `stamp` reads `api.load_session(session, null, 16)` (plain read, 4th
    argument omitted = not `settled_only`, allowed mid-turn: `loop_.rs` `context_history` refuses only
    `settled_only` while busy) on `ContextUsed`, `ToolCallStarted`, `ToolCallFinished`, `InputInjected` of the
    session, only when `unstampedKey` is not null (closed live replies of the running turn without `context`). One
    read in flight; calls meanwhile coalesce into one more (the `settle` pattern).
  - **R1 (review), bounded reads.** A reply `stampStored` refuses for good (text mismatch, a salvage continuation —
    one live reply, two stored rows — a tool batch wider than the page) used to keep every later event of the turn
    reading. Now the attempts are keyed by `unstampedKey` (`<turn>:<ids of the closed unstamped replies>`); a read
    that leaves the same key is a miss (`stampTried`), and after 2 misses for one key `stampWanted` says no until the
    key changes (a reply closes or gets stamped, or another turn runs). The turn-end settled reload still stamps
    everything. The normal round costs one miss (the `ContextUsed` read before the store) and then stamps, so it
    never hits the limit; the fixture rerun still shows rows=1 (miss), rows=2 (stamped), then no more reads.
- **D3, copy** — "ready when the reply ends" → "ready when this message is saved" in `Cell.tsx` (doc, comment, button
  reason, notice) and `RunbookBar.tsx` (doc, `aria-description`); the two docs now say "not stored yet".
- **D4** — no Rust change (evidence below).
- **D5** — notes cells, `after:` gates, danger confirmation, private cells, `<cell_result>` delivery and the wake rule
  untouched.

## Decisions

1. **Trigger events.** The brief assumed the engine stores the round and then emits `ContextUsed`. The code says the
   opposite: `loop_.rs` `round()` publishes `ContextUsed` at ~6744 and stores the row afterwards (`persist_timed`
   ~6798, `append_final` ~6804, `append_tool_use` ~6879). So `ContextUsed` alone would read too early and miss the
   row. The trigger also listens to what follows the store: `ToolCallStarted`/`ToolCallFinished` (`run_batch`, after
   `append_tool_use`) and `InputInjected` (after a correction round's `persist_timed`). `ContextUsed` still asks (cheap
   when nothing is unstamped) and the coalescing turns the race into "first read misses, second read stamps" — the
   fixture shows exactly that (rows=1 then rows=2). The final round needs nothing new: `TurnFinished` + the turn's own
   `ContextChanged` run the existing settled reload.
2. **Matching.** Within the running turn (`slice.turnSeq`) only. The page must reach back to the turn's last stamped
   reply (its seq; the LAST row with that seq, since its reasoning row shares it) or, with none stamped, to the turn's
   opener (`role: user`, `turn === turnSeq`); else nothing. The stored assistant rows after that point (non-blank text,
   as `fromHistory` keeps them) pair in order with the closed live replies lacking `context`; every pair must have
   identical text and the stored count must not exceed the live count; any doubt → nothing stamped. Open replies never
   pair. Only `context` is added (`{ ...entry, context }`); ids, order, other fields and other entries keep identity.
   A salvage continuation (one live reply, two stored rows) or a text mismatch waits for the settled reload, as before.
3. **Page size 16.** A round is its reply plus its tool rows; stamping runs every round, so the page only has to reach
   the previous stamped reply. A batch wider than ~14 tools simply misses the anchor and stamps nothing (no wrong
   stamp); the settled reload covers it.
4. **Freshness.** `stampTail` drops a reply when `historyGeneration` moved since the request (a newer page or a context
   edit installed meanwhile). `live` is not compared: deltas move it constantly, and stamping only touches entries
   whose text matches, so a later delta cannot be mis-stamped.
5. **Errors** of the stamp read go through the hook's `report` like every other read of the route.
6. **Mid-turn removal controls.** A stamped reply now has `context`, so `MessageAssistant`'s shake controls appear on
   it mid-turn — the same state a reload during a running turn already produces (brief D2).

## D4 evidence — the engine already runs a running turn's stored cells

- `crates/cells/src/actor.rs:849` `run` → `prepare` (716) → `chat::transcript(session)` (728) → `inspect`; no
  activity/turn check on the path. The only `turn_running` use is delivery's wake rule (`actor.rs:1530`, fn 2264),
  171 D12 "a result joins a running turn" — unchanged.
- `crates/cells/src/actor/chat.rs:43-81` `transcript` asks `agent::ReadSession` and keeps every
  `HistoryEntry::Assistant` with a message seq.
- `crates/agent/src/supervisor.rs:8307-8350` `ReadSession` → `read_session` → `store.context_history(session)`: a plain
  store read, no idle/busy check (unlike `LoadSession { settled_only: true }`, `loop_.rs:2925`), so rows of the
  running turn stored by `append_tool_use`/`persist_timed` are included.
- `crates/cells/src/cells_guard.rs` (press/read guards: peer, guest, internal) and `rpc.rs:117` `cell_run` →
  `kernel_rpc::run`: no turn check.

## Tests

- `src/store/stored-tail.test.ts` (new): stamps in order, identity of untouched entries; incremental (anchor) page
  without the opener; the not-yet-stored round changes nothing; text mismatch / too many stored / page not reaching
  back → nothing; other turns and `turn: undefined` untouched; `unstampedKey`; R1 table: two misses for one key stop
  the reads, a new reply reads again, stamping one of two is progress, another turn resets, no key never reads.
- `src/components/cells/CellFence.test.ts`: a stored cell is runnable (button enabled, Run all) with and without a
  running turn; without a stored seq it is pending with "ready when this message is saved"; the incomplete-fence test
  keeps pending with the new copy.

## Proof

`/tmp/cells-midturn/proof.md`, shots `/tmp/cells-midturn/shots/{base,fix,fix-webkit}-{closed,unsaved,saved,ran,ended}.png`,
fixture `src/dev/cells-midturn-fixture.{html,tsx}` (real `useSessionLoad` + reducer, stub `load_session`). Not done: a
real mid-turn run against a scratch engine.

## Gates

All from `crates/desktop/ui` under `~/.theuth/bin/build-lock`:

- `bunx tsc -b --noEmit`: 0 `error TS` (baseline before the change also 0; `/tmp/cells-midturn/tsc-base.txt`, `tsc.txt`).
- targeted `bunx vitest run src/store/stored-tail.test.ts src/components/cells/{CellFence,Cell,RunbookBar}.test.ts
  src/store/{reduce,sessions}.test.ts src/components/disclosure.test.ts src/lib/no-emoji.test.ts`: 8 files, 167 tests
  passed after R1 (166 before; `/tmp/cells-midturn/vitest.txt`).
- full `BUILD_SLOTS=1 … bunx vitest run src` after R1: 359 of 360 files, 2531 of 2532 tests passed; the one failure,
  `src/link/pairing.test.ts` ("RpcError: connection changed", a relay round trip, nothing this branch touches),
  passed alone (7/7, `/tmp/cells-midturn/vitest-pairing.txt`) — a load flake. Before R1 the full suite was 360 files,
  2531 tests, all green (`/tmp/cells-midturn/vitest-full.txt` holds the R1 run).
- No Rust touched: no cargo/mbx gates.
