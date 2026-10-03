# 87 — as built: COMPACT-NONBLOCK (the composer stays usable during a compaction)

Branch `fix/87-compact-nonblock` off main 4e618ae9, 2026-10-03. Wave 87 (sibling: HARVEST-DETACH in
`crates/agent`). UI-only: no RPC method, event type or binding changed.

Observed: `/compact snap` on a 640k-token session took 3 min 17 s and the desktop composer was locked the
whole time — nothing could be typed or queued. COMPACT-QUEUE (70) fixed the *mis-send* during a compaction
(input queues, never cancels; the engine delivers it after the summary commits). What remained was the
composer lock: `submit()` set `busy` and awaited `run(line, ctx)`, and `compact()` awaited `api.compact`,
which resolves only once the new epoch has landed (harvest, summariser or snap rasterisation). While
`busy`, the textarea and send button are disabled and `submit` returns early.

## What changed (file:line)

- `crates/desktop/ui/src/components/chat/slash.ts:617-633` — `compact(api, session, s)` is synchronous:
  dispatches an optimistic `Compacting` (`useSessions.getState().applyEvents([{ type: 'Compacting', session,
  turn: null }])`, the binding's field set), fires `api.compact(session, s)` and attaches the continuation —
  success: `toast('context compacted (epoch N · style)')` / `(epoch N)`, nothing for `clear` (the `Compacted`
  event announces it); rejection: `store.settleCompacting(session)` then `toast.error(message)`. The guard
  comment states the rule: a command whose RPC waits on a model call must not be awaited by `submit`. `run()`
  therefore returns at once for `/compact [style]` and `/clear`; every other command still awaits.
- `crates/desktop/ui/src/store/reduce.ts:937-940` — `case "Compacting"` returns `undefined` (slice and `live`
  untouched) when `activity` is already `compacting`: the engine's own `Compacting`, arriving after the
  optimistic one, is a no-op. Before, it always produced a new slice (and bumped `live`).
- `crates/desktop/ui/src/store/reduce.ts:89-98` — `settleCompacting(s)`: `activity` back to its resting state
  only if still `compacting`; otherwise returns `s` itself.
- `crates/desktop/ui/src/store/sessions.ts:290-291, 473-479` — store action `settleCompacting(id)` wrapping the
  reduction; no `set` when nothing changed.
- Tests: `components/chat/slash.test.ts` (+4), `store/reduce.test.ts` (+2).
- Not edited, verified by reading: `Composer.tsx:245-247` (`running = turnRunning || activity ===
  'compacting'`), `670-676` (send while running → `InputQueued` locally + `api.steer`), `912-915` (Stop chord →
  `api.cancel`), `739-741` (`finally setBusy(false)`); `SessionMenu.tsx:238-241` `cmd(line)` → `void run(line,
  ctx).catch(toast.error)` — the Compact submenu gets the behaviour for free.

## Semantics

1. Typing `/compact snap` (or `/clear`, or the SessionMenu's Compact submenu): the session flips to
   `compacting` synchronously, the RPC is issued, `run()` resolves, `submit`'s `finally` clears `busy` within a
   tick. Placeholder reads "Send to steer…", a plain send goes out as `api.steer` with the queued chip
   (COMPACT-QUEUE delivers it after the summary commits; the chip self-heals via `InputInjected`), Stop/Esc
   cancels, ⌘↩ cancels then steers. Steer/queue mechanics are untouched.
2. Engine `Compacting` moments later: no-op. Engine `Compacted`: resets `activity`, sets `context`, adds the
   divider (or the clear info line) — as before (reduce.ts 941-962). The RPC's resolution only toasts.
3. Refused RPC (`nothing to compact`, `turn in flight`, `snap needs a vision model (this one is text-only)`,
   transport error): `settleCompacting` returns `activity` to idle **only if still `compacting`**; a
   `Compacted`, `TurnStarted`, `Error` or any other activity change that landed meanwhile wins. Then
   `toast.error(message)`.
4. Success needs no settle: `Compacted` already resets to idle.

## Verification

Inside `crates/desktop/ui` (bindings copied from Sky's checkout per the brief, never regenerated):

- `bun install` → `617 packages installed`.
- `bun run check` → 92 errors, **all pre-existing**: 44 × `Cannot find module '@/rpc/bindings/<X>'`
  (`Preflight`, `SandboxEvent`, `SandboxInfo`, `SecretRequest`, `VmMetrics`, … — Sky's binding copy dated
  Sep 29 predates main's sandbox/secret code) plus their implicit-`any` cascade. Compared the sorted error
  list with the change stashed vs applied, line numbers stripped: identical (`IDENTICAL-MODULO-LINES`). My
  files add no error.
- `CI=1 bun run test < /dev/null 2>&1 | tail -30` →
  `Test Files  1 failed | 129 passed (130)`, `Tests  1 failed | 718 passed (719)`.
  The one failure is `src/rpc/api.test.ts` "METHODS equals the server method list": the copied
  `bindings/methods.json` lacks `secret_requests_list`, which main's `api.ts` lists. Same stale-bindings cause,
  independent of this change (the test reads only `api.ts` and `methods.json`, neither touched).
- Targeted: `CI=1 bun run test src/components/chat/slash.test.ts src/store/reduce.test.ts` →
  `Test Files  2 passed (2)`, `Tests  59 passed (59)` (8 in slash, 51 in reduce).

## Deviations

- `settleCompacting` rests at `s.running ? "thinking" : "idle"`, the same `idle` convention `Compacted` and
  every other reduction in `turn()` use (reduce.ts 709), rather than a literal `"idle"`: a `/compact` refused
  with `turn in flight` has a running turn whose next delta relabels it. With no turn running it is `"idle"`
  exactly as the brief specifies.
- Idempotency is implemented by returning `undefined` from `turn()` (slice and `live` kept), not by
  returning `s`: `apply` wraps any defined return in `{ ...next, live: live + 1 }`, which would still have
  produced a new slice and bumped `live`.
- Tests race `run()` against one `setImmediate` hop (`flush()`), not a 100 ms timer: the repo rule forbids
  wall-clock timers in tests; a macrotask hop deterministically drains every queued promise continuation.
- `/undo` with a never-resolving `api.undo` is the rule-4 regression guard (brief: "a non-compact command that
  calls an `api` method and toasts").
- Ran `bun install` inside `packages/apps` (node_modules only, no source touched): `tsc -b` resolves
  `@openuidev/*` through `packages/apps/node_modules` and otherwise fails on every file.
- Gate 3 cannot be clean on this base with the prescribed (non-regenerated) bindings; reported as the
  before/after error-set diff above instead.

## Follow-ups

- Refresh the binding copy at `/Users/Sky.Moore/Develop/theuth/bindings` (`just ts-desktop` on main) so
  `bun run check` and `api.test.ts` are green again for every UI worktree.
- `/knowledge harvest` also awaits a model-bound RPC and holds `busy` the same way; out of scope here
  (HARVEST-DETACH moves the harvest off the compaction path in the engine).
