# 142 SHOW-CARD-RELAY-OPEN as built — a `show` card is openable on a relay client that slept through its event

Branch `fix/show-card-relay-open` on `589a72f2` (origin/main). Brief: `~/.theuth/briefs/2026-10-08/showcard.md`. UI only.

## The ask
Sky, 2026-10-08 10:51/10:58 (iPad, Tauri iOS over the relay): "I can't open that show card" — the card of
`137-endless-morning-report.md`, shown at 08:05 by a desktop session while the iPad was suspended.

## Mechanism (verified on the base)
1. `ArtifactCard.tsx` renders a FINISHED `show` whose record is missing from `slice.artifacts` as the receipt
   `<Frame onClick={draft && open}>`; `draft` exists for `app` only → no `onClick`, no `role`, no pointer. Tap does nothing.
2. The map is filled by `list_artifacts` on the slice's first `load()` only (`routes/c.$session.tsx`, a failure landed as
   `[]`) or by a live `ArtifactShown`. A relay socket parked while iOS slept loses the event (`rpc/client.ts`, 25 §2.1);
   recovery (`App.tsx reload()`/`resync()` → `markStale` → `settle()` = `load_session(settled_only)`) never re-listed.
3. Runner-up "could not load <name>": the panel's `useText` → `readText` → `get_attachment` rejects `not connected`
   inside the reconnecting/probe window after foregrounding; the hook cached `''` until its `Content` remounted.

## What changed (all under `crates/desktop/ui/src`)
- `components/chat/ArtifactCard.tsx`
  - F1 `openShow(api, session, callId, name)`: record in store → `setArtifactPanel`; none → `await api.get_artifact(callId)`,
    `applyEvents([{ type: 'ArtifactShown', … }])` exactly as the live event (pattern `OpenUIFence.openAsApp`), then the
    panel. Failure → `toast.error('could not open <name>')`, card untouched. `<name>` = the call's `name`/basename of
    `path` via the existing `partialShow`, else the tool name. The card's `open` now routes through it (all three
    branches); the receipt's `onClick={draft || (result && !result.isError) ? open : undefined}` — `Frame` keeps its
    `role="button"`/pointer semantics; error results and still-compiling calls stay inert.
  - F2 `relistArtifacts(api, store, session)`: `list_artifacts(session, null)` → `setArtifacts`; a rejection resolves and
    leaves the map as it is (never `[]`). Lives here (not in the route) so it is unit-testable: the route module pulls
    `Transcript` → `lib/nudges` → `localStorage` at import time.
  - F3 `useText` reads through `readTextOnline(api, client, path)` (`client` from `useConnection`), so a transport
    rejection resolves after the retry instead of caching `''`; `''` only once the retry fails.
- `routes/c.$session.tsx`: `load()` uses `relistArtifacts` in its `Promise.all` (drops the silent `.catch(() => [])` +
  unconditional `setArtifacts`); `settle()` fires `relistArtifacts` once per settle pass. The `Lagged` resync
  (`App.tsx resync()`) dispatches `theuth:reload-session {settled: true}` → that same `settle()`, and unmounted slices
  settle on mount via `contextStale`, so one helper covers reconnect, `Lagged` and later visits. `App.tsx` untouched.
- `lib/files.ts`: `readTextOnline`: `readText`; on `isTransportError` wait for the next `client.onState` `open` (one
  listener, removed when it fires; an already-open client retries at once), then `readText` again; any other error or a
  second transport failure rejects. `readText` itself already dropped failed promises from its cache — unchanged.
- `rpc/client.ts`: `export const isTransportError(e): e is RpcError` (code -32300: `not connected`/`connection closed`).

## Tests (vitest, next to the files)
- `components/chat/ArtifactCard.test.ts` (new): receipt with finished non-error result renders `role="button"`, error
  result and compiling do not; `openShow` fetches, applies, opens; present record skips the fetch; failed fetch toasts
  "could not open 137-endless-morning-report.md" and leaves map/panel; `relistArtifacts` installs the list and a rejected
  list keeps the existing map.
- `lib/files.test.ts` (+2): not-connected read keeps the promise pending with one state listener, resolves after `open`,
  listener removed, second read cached; already-open client retries at once; a non-transport error rejects without waiting.

## Gates (`BUILD_LABEL=showcard build-lock`, in `crates/desktop/ui`)
- tsc before: 337 `error TS` lines (stale bindings: attention/*, surface/Graph, rpc/api.ts, …) → after: 337; `diff` of
  sorted error lines empty; none in `ArtifactCard`, `c.$session`, `lib/files`, `rpc/client`.
- `vitest run src/components/chat src/lib/files.test.ts src/rpc/client.test.ts`: 51 files / 493 tests pass; the one
  failing file is `surface/catalog.test.ts` (7 tests against the gitignored `bindings/surface-catalog.json`, pre-existing,
  not touched).

## Not verified without a device
The iPad path end to end (suspend → wake → tap the receipt → panel), the toast copy on a real failure, and that the
≤5 s probe window is the only source of `not connected` there. `bun run build` not run (stale bindings; orchestrator gate).
