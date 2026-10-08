# 142 SHOW-CARD-RELAY-OPEN as built — a `show` card is openable on a relay client that slept through its event

Branch `fix/show-card-relay-open` on `589a72f2` (origin/main). Brief: `~/.theuth/briefs/2026-10-08/showcard.md`. UI only.
The ask — Sky, 2026-10-08 10:51/10:58 (iPad, Tauri iOS over the relay): "I can't open that show card" —
`137-endless-morning-report.md`, shown at 08:05 by a desktop session while the iPad was suspended.

## Mechanism (verified on the base)
1. `ArtifactCard.tsx`: a FINISHED `show` with no record in `slice.artifacts` is the receipt `<Frame onClick={draft && open}>`;
   `draft` exists for `app` only → no `onClick`/`role`/pointer. Tap does nothing.
2. The map is filled by `list_artifacts` on the slice's first `load()` only (`c.$session.tsx`; a failure landed as `[]`) or by
   a live `ArtifactShown`, which a relay socket parked while iOS slept loses (25 §2.1); `reload()`/`resync()` → `markStale` →
   `settle()` = `load_session(settled_only)` never re-listed.
3. Runner-up "could not load <name>": `useText` → `readText` → `get_attachment` rejects `not connected` in the reconnecting/probe
   window after foregrounding; the hook cached `''` until its `Content` remounted.

## What changed (all under `crates/desktop/ui/src`)
- `components/chat/ArtifactCard.tsx` — F1 `openShow(api, session, callId, name)`: record present → `setArtifactPanel`; none →
  `await get_artifact`, `applyEvents([ArtifactShown])` as the live event would (pattern `OpenUIFence.openAsApp`), then the
  panel; failure → `toast.error('could not open <name>')` (`name`/basename of `path` via `partialShow`, else the tool name),
  card untouched. All three `open`s route through it; receipt `onClick={draft || (result && !result.isError) ? open :
  undefined}` — `Frame` keeps `role="button"`/pointer; error and still-compiling cards stay inert. F2 `relistArtifacts(api,
  store, session)`: `list_artifacts → setArtifacts`, a rejection resolves and keeps the map (never `[]`); lives here, not in
  the route, because the route module pulls `Transcript → lib/nudges → localStorage` at import. F3 `useText` reads through
  `readTextOnline(api, client, path)` (`client` from `useConnection`): `''` only once the retry fails.
- `routes/c.$session.tsx`: `load()` uses `relistArtifacts` in its `Promise.all` (drops `.catch(() => [])` + unconditional
  `setArtifacts`); `settle()` fires it once per pass. `resync()` dispatches `theuth:reload-session {settled}` → that `settle()`;
  unmounted slices settle on mount via `contextStale`, so one helper covers reconnect, `Lagged` and later visits.
- `lib/files.ts readTextOnline`: `readText`; on `isTransportError` wait for the next `client.onState` `open` (one listener,
  removed when it fires; already-open retries at once), `readText` again; other errors / a second transport failure reject.
  `readText` already evicted failed promises — unchanged. `rpc/client.ts`: `isTransportError(e): e is RpcError` (code -32300).

## F4 (addendum, Sky 11:14): "after the overnight run I had to restart the app for the sidebar agent state icons to go back
to the correct state, many were showing subagents when none were running" (iPad, relay). Mechanism (verified): sidebar
liveness is slice memory advanced by events only — `slice.running`/`activity`, the subtree rows' `state` (`reduce.ts trackRow`
→ `AgentTree liveOnly`/`stripRows`), and `runningSubagents` = `slices[d.id]?.running || d.running`, so a refetched
`SessionSummary.running=false` never beats a stale slice. The parked socket lost the finishing events; `reload()`/`resync()`
only `markStale()`d (kept `running`). Change: `reduce.ts reseedRunning(slice, running(id), now)` resets own running/activity
and every child row's live bit to the engine's answer (unknown id untouched; identity kept on no-op); `sessions.ts
seedRunning(rows)` applies it from `list_sessions`; a whole-store `markStale()` (reconnect/`Lagged`) drops all marks via
`reseedRunning(s, () => false)` — `markStale(id)` for a session leaving event focus keeps them (its turn events still arrive);
`App.tsx reseedLiveness(st)` = `seedLive` + `list_sessions → seedRunning` from `reload()`, `resync()` and the new
`useForegroundReseed` (`visibilitychange` → visible, per-engine `Shell`). Desks need no `list_agents`: rows are keyed by
session id. Test `store/sessions.test.ts` (new): desk + two running subagents → `seedRunning([idle, idle, running])` clears
two, keeps one (`runningSubagents` 2 → 1); a lost `TurnStarted` re-marks; unlisted untouched; no-op keeps identity; bulk
`markStale()` drops every mark, `markStale('a')` does not.

## Tests (vitest, next to the files)
- `components/chat/ArtifactCard.test.ts` (new): finished non-error receipt renders `role="button"`, error/compiling do not;
  `openShow` fetches, applies, opens; present record skips the fetch; failed fetch toasts "could not open <name>" and leaves
  map/panel; `relistArtifacts` keeps the map on a rejected list. `lib/files.test.ts` (+2): not-connected read stays pending with
  one listener, resolves after `open`, listener removed, second read cached; already-open retries at once; other errors reject.

## Gates (`BUILD_LABEL=showcard build-lock`, in `crates/desktop/ui`)
- tsc before 337 `error TS` lines → after 337, identical once `(line,col)` is stripped (F4 shifted pre-existing lines). Zero in
  `ArtifactCard`, `c.$session`, `lib/files`, `rpc/client`, `App.tsx`; `reduce.ts`/`sessions.ts` keep only baseline stale-binding
  errors (`AdvisoryNote`, `LiveCountsRow`, …). vitest: `src/components/chat src/lib/files.test.ts src/rpc/client.test.ts` 51/52
  files (`surface/catalog.test.ts` fails on the stale gitignored `surface-catalog.json`, pre-existing); `src/store/sessions.test.ts
  src/store/reduce.test.ts src/lib/event-focus.test.ts src/components/sidebar` 4 files / 76 tests pass.
- Not verified without a device: iPad suspend → wake → tap → panel; the overnight sidebar case on a real relay; that
  `visibilitychange` fires on iOS resume (the RPC client already relies on it). `bun run build` not run (stale bindings).
