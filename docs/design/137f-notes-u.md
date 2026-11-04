# 137F-U notes — failing settle, Retry, live compactor copy (desktop UI)

Decisions (brief ambiguities, simpler option taken):
- **Footer, not a new row in the tree.** The failing block sits in the panel footer above the settle/backfill bar (`MemoryFooter`); the bar stays, so a non-blocking failure under a backfill keeps its progress.
- **Badge tone only.** "Same tone" is applied to the `∞` pill and the compact menu row; the tooltip keeps the stock tooltip colours and says `… · summaries failing`. Failing outranks backfill/settling in the tip and stops the pulse.
- **Next try.** `ago()` from `lib/costs/format.ts` (`in 40 s`, `in 3 min`); under 5 s or past due → "in a moment" (the pump is late, not "ago"). A 1 s interval ticks only while a retry is scheduled and the panel shows it.
- **Change model.** Reuses `settings()` from `chat/slash.ts` (added `'memory'` to `SettingsTab` and an optional `focus`). The route accepts `?focus=`; `SettingsDialog` passes it to `Memory` as `focusModel`, which focuses `#compactor-model` once the settings load (a disabled button takes no focus). Same shape as the existing `?peer=` focus on Communication.
- **Live progress (U3).** `EndlessProgress` still invalidates `session_memory` (cadence unchanged); it now also writes `e.stalled` into the cached memory first, so the failing row appears/clears with the event rather than one refetch later. No extra requests.
- **Retry.** `retryMutation(api, qc, session)` = the view-size Select's pattern (`setQueryData` on success, `toast.error` on error); the button is disabled with a spinner while in flight.
- **U4 advanced keys skipped.** `EndlessConfig` in the base bindings (E's contract `ae62682d`) has no `attempt_secs` / `local_attempt_secs` / `local_ctx_bytes` / `local_jobs`; nothing to bind. Add an "Advanced" disclosure (SandboxSheets pattern) once E's keys land.
- **U6.** No UI string claimed the model applies "after restart" (grep over `src/`); the only change is the new "no restart" copy. `useCompactorLabel` showing the chosen model at once is now correct, as the model is live.
- **Fixture.** `localStorage.endless-fixture = "stalled"` (blocking frontier, next try +40 s) or `"retrying"` (`next_retry_at: null`); fixture `endless_retry` answers the retrying shape after 800 ms.
