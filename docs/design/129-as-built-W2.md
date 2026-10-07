# 129 — as built, W2 (UI)

Branch `feat/129-w2-ui`, `crates/desktop/ui` only. Contract: `129-event-focus.md` §3.4 and D4.

## What was built

- **`set_event_focus`** — `src/rpc/api.ts`: interface entry after `sessions_status` (positional, `Promise<null>`), `METHODS` entry in sorted position. Hand-written; the integrator regenerates `bindings/methods.json`.
- **Focus derivation** — `src/lib/event-focus.ts`:
  - `deriveFocus({ pathname, leaves, popout, desk })`: the `/c/<s>` or `/talk/<s>` route session, every pane leaf (drafts excluded via `isDraft`), the popout's session (`popoutSession(POPOUT)`), the island's desk (`useVoiceKey.desk`, which renders `TextDelta` subtitles). Sorted, de-duplicated. The island with no desk → `[]`.
  - `FocusTracker`: last set sent on this connection; `update()` says whether to send and which sessions left; `reset()` after a reconnect.
  - `Throttle(fn, everyMs, trailing)`: the 5 s resync (trailing, coalesces a burst into one later run) and the 60 s toast (non-trailing).
  - `typeCounts()`: the D4 histogram.
- **Wiring** — `src/App.tsx` `useEventFocus()` (mounted in `Shell` beside `useEventSideEffects`): recomputes on `router.subscribe('onResolved')`, `usePanes` root change, `useVoiceKey.desk` change, debounced 100 ms; sends immediately (tracker reset) on every connection `open`; only when the set changed; sessions that left get `sessions.markStale(id)`; the RPC's rejection (-32601 on an engine without W1) is ignored.
- **Entering focus** — unchanged path: `routes/c.$session.tsx` `useSessionLoad` mount effect sees `contextStale` and calls `settle()`.
- **`Lagged` resync** — `App.tsx` `resync()`: `resetLagged()`, `markStale()` (every slice), invalidate `queryKeys.sessions` only, `theuth:reload-session` with `{ settled: true }`. Driven by `on('Lagged')` through the 5 s `Throttle` in `useEventSideEffects` (main and AUX); the main window's toast ("Events dropped, resyncing") through the 60 s `Throttle`. `unload()` is no longer called anywhere on the event path.
- **Reconnect** — `App.tsx` `reload()` (`reconnecting → open`): keeps the nine invalidations and `loadInfo`, now `markStale()` instead of `unload()`, still the bare `theuth:reload-session` (full page for mounted chats).
- **Store** — `src/store/sessions.ts` `markStale(id?)`: on loaded slices only, sets `contextStale`/`todosStale`, bumps `historyGeneration` (supersedes in-flight pages), leaves `loaded` and `entries`. `unload()` stays for the engine switch (`Main`'s `[active]` effect).
- **D4** — `App.tsx` `attach()`: `performance.now()` around `dispatch`; `> 50 ms` → `console.warn('[events] slow drain', { ms, events, types })`. `watchLongTasks()` installs one `PerformanceObserver` on `longtask` when `PerformanceObserver.supportedEntryTypes` lists it (Chromium; WKWebView does not), warning `[events] long task { ms }`.
- **`lib/rpc.ts`** `useInvalidateOn`: the models/title/subagent/vision/escalate invalidations on `theuth:reload-session` skip `{ settled: true }` events (the resync and a context edit touch one transcript, not providers).

## Children / sidebar dependence on deltas (checked)

- Children's live deltas are **not** rendered inside a parent's view. `SubagentStrip` and `Transcript`'s agent rows read `slice.children` (`AgentInfo` rows), which `reduce.ts` `trackRow()` patches from `TurnStarted`/`TurnFinished`/`ToolCallStarted(send_message)`/`ToolCallFinished`/`Error` only — all non-delta, still flowing. So the focus set does **not** include children.
- Sidebar (`SessionItem` `SessionActivity`) shows `slice.activity`; `TextDelta`/`ReasoningDelta` set it to `streaming`/`reasoning`, but `TurnStarted` sets `thinking` and `TurnFinished` → `idle`, so an unfocused session still shows a live glyph (thinking rather than streaming). Acceptable; no change.
- Other delta consumers: `ArtifactPanel` `on('ToolCallProgress')` (only for the panel's own session, which is a visible chat's) and `voice-key.ts` `on('TextDelta')` for the desk (covered by `desk` in the focus set).

## Deviation from the existing settle contract (not from §3.4)

`settle()` (`c.$session.tsx`) no longer breaks on the slice's `running`, and `loadHistory(..., 'settled')` no longer refuses on `cur.running`. After a gap the lost `TurnFinished` leaves `running: true`, and with both guards the slice could never re-settle (the design's whole recovery path). The engine remains authoritative: a busy actor answers `settled: false` (refused, waits for its `ContextChanged`); a turn that raced the read moved `live` (refused). Covered by `event-focus.test.ts` "settled page after a gap".

## Tests (`src/lib/event-focus.test.ts`)

- deriveFocus: chat or talk route + pane leaves + popout + desk, drafts out, sorted and unique
- FocusTracker: unchanged sets are not re-sent, a reset (reconnect) re-sends, leavers are named
- Throttle: a trailing resync coalesces a burst into one run per window; a non-trailing toast drops the rest
- markStale: Lagged / a session leaving focus keep `loaded` and the transcript, set both stale flags, bump the generation
- settled page after a gap: a `running` left true by a lost TurnFinished is corrected, not refused
- typeCounts: the per-drain histogram

## Open items

- A running session re-entering focus shows the text it had before leaving plus the deltas after re-entering (the middle is not in committed history, and the actor refuses a settled read while busy); the `TurnFinished → ContextChanged(settled)` settle fills it in. Fixing it live needs an engine-side "current partial" read.
- `bindings/methods.json` absent in this worktree: `api.test.ts`'s server comparison is skipped; integrator regenerates.
- `longtask` entries never fire on macOS WKWebView; the drain timer is the instrumentation there.
