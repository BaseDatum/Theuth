# As-built: LIVE-GLYPHS + COMPOSER-FOCUS + ASK-UNFOLDED (2026-10-07)

Branch `fix/live-glyphs-focus-ask` off main `0c605cc7`. Three user-facing fixes Sky asked for on 2026-10-07; no design
doc. Commits: `260c7b81` (A engine), `e5ee4c4d` (A desktop), `087f5499` (C), `7b52df58` (B).

## A. Sidebar shows what an idle session is still waiting on

Sky: "when a session has a reminder set or has background jobs running but is otherwise doing nothing we need separate
icon for each of those in the left sidebar on that session". Before: the cog/terminal glyphs were fed by live
`JobEvent`s only (empty after a reload, reconnect or engine switch) and the desktop never heard about reminders.

### Engine (`260c7b81`)
- `crates/tools/src/reminder.rs`: `ReminderEvent::Fired { session, id }` (wire `ReminderFired`), published from
  `impl Message<Fired> for Jobs` right after the row leaves `pending` and its guard settles. `ReminderEvent` now has the
  ts-rs export. `Reminder.id` / `SessionReminders.pending` are `pub(crate)` for the snapshot.
- `crates/tools/src/jobs.rs`: `LiveCounts` message → `Vec<LiveCountsRow { session, jobs: Vec<JobId>, ptys: Vec<JobId>,
  reminders: Vec<String> }>` — running non-PTY jobs, running PTY jobs, pending reminder ids; only sessions with
  something live. Test fixture `Fx` also records `ReminderEvent`s (`reminder_events`).
- `crates/tools/src/lib.rs`: re-exports `LiveCounts`, `LiveCountsRow`.
- `crates/rpc/src/bridge.rs`: `ReminderEvent` in the `forward!` list and subscribed. `client.rs` republishes it,
  `export.rs` exports `ReminderEvent` and `LiveCountsRow` (→ `ReminderEvent.ts`, `LiveCountsRow.ts`).
- `crates/rpc/src/api.rs`: `list_live_counts()` → `Vec<LiveCountsRow>` after `list_jobs`; no session, no
  authorization beyond the connection (ids only), like `list_sessions`. Not a delta, not focus-filtered.
- `crates/agent/src/supervisor.rs`: `ReminderEvent::Fired { .. } => Ok(())` in the store mirror (the row is already
  deleted with its `Settle::Reminder`).
- Tests: `reminder::tests::fired_and_cancelled_publish_their_events` (Set, Set, Cancelled r2, Fired r1 — never Fired
  for the cancelled one; serde shape `{"type":"ReminderFired","session":…,"id":"r1"}`),
  `reminder::tests::live_counts_lists_pending_reminders_only`, `jobs::tests::live_counts_rows_running_jobs_and_pending_reminders`,
  `rpc api::tests::reminder_fired_wire_shape`.

### Desktop (`e5ee4c4d`)
- `store/sessions.ts`: `SessionSlice.reminders: Set<string>`; action `seedLive(rows)` — `ensure`s each row's session and
  replaces its `jobs`/`ptys`/`reminders`; every other slice with a non-empty set is emptied. Nothing else touched.
- `store/reduce.ts`: `reminders: new Set()` in `empty()`; `jobGone` generalised to `"ptys" | "jobs" | "reminders"`;
  arms `ReminderSet` (add, no-op when present), `ReminderFired`/`ReminderCancelled` (drop, no-op when absent) right
  after `JobFinished`. No transcript row, no `live` bump.
- `rpc/events.ts`: `ReminderEvent` at the end of `WireEvent`. `rpc/api.ts`: `list_live_counts()` + `METHODS` entry.
- `App.tsx`: `seedLive(st)` helper (`api.list_live_counts()` → `sessions.seedLive`, failures `console.warn`) called
  (1) on the stream's first `PendingPrompts` item in `attach()` — the subscription is up; also fires after a
  reconnect's re-issue — (2) in `resync()`, (3) in `reload()`.
- `components/sidebar/SessionItem.tsx`: `SessionJobs` → `SessionPending`; lucide `Timer` (`size-3.5 shrink-0 text-info`,
  no pulse, `role="img"`, `aria-label="N reminder(s) pending"`) after the terminal glyph. Row order: activity · cog ·
  terminal · timer · subagents · title. `AgentTree.tsx`/`AppSidebar.tsx` did not render `SessionJobs`; untouched.
- Tests (`store/reduce.test.ts`): reminder arms incl. unknown-id no-op returning the same slice; `seedLive` installs a
  snapshot for a session with no slice and clears a stale set for one not in it.

### Live check (scratch engine `target/debug/t --headless`, data dir `/tmp/live-glyphs-data`, vite :5181, headless Chromium)
Haiku was told to `remind_me_in(90)` + `shell sleep 120`. Idle row showed cog (pulsing) + timer (static) — screenshot
taken; `list_live_counts` = `[{jobs:["j1"], ptys:[], reminders:["r1"]}]`. After a page reload both glyphs were back
within the seed (sets are empty until `list_live_counts` lands). When r1 fired: timer gone, row showed the activity
glyph ("Reasoning"), then cog only; after the job exited, no glyphs and an empty snapshot. PTY glyph not exercised live.

## B. Composer keeps the caret after a send (`7b52df58`)

Diagnosis (B1), in the running UI with focusin/focusout tracers: an **Enter** send never loses focus (no `focusout` at
all). A **mouse** send moves focus to the Send button (`focusout textarea → BUTTON[Send]`), `setBusy(true)` disables it
6 ms later and Chromium drops focus from a disabled element to `document.body` (`focusout BUTTON[Send](disabled) →
null`); `running` then replaces it with Stop. Esc (`keys.stop`) lives only in the textarea's `onKeyDown`, so it was dead
until a click back in. No remount of `RecordingBox`/`Panes`/the route and no `TerminalViewer` focus was involved.
- `lib/composer-focus.ts` `shouldRefocus(active, box, touch, body = document.body)`: `touch` → false; `active` null or
  body → true; inside `box` → true; anything else (another input, a dialog) → false. `lib/composer-focus.test.ts`.
- `components/chat/Composer.tsx`: `refocus()` in `submit()` and `flush()` `finally` after `setBusy(false)`; and in a
  `useEffect` keyed on `focused` (pane focus). Mount-only did not work for a new chat: the `/c/$session` composer mounts
  before the route's `usePanes.show(session)` effect, so `focused` was still the draft pane's.
- Verified: mouse send → `focusin TEXTAREA` 3 ms after the button went disabled; Esc on the textarea while streaming
  cancelled the turn (Stop gone, partial dropped). New chat → `/c/…` composer takes the caret. Mobile untouched
  (`touch()` short-circuits); not exercised on a device.

## C. Answered `ask_user` cards are never folded (`087f5499`)
`components/chat/ToolRunCard.tsx`: `ask_user: true` in `DELIVERABLE_TOOLS` with Sky's sentence in the doc comment;
`foldRuns` already splits around deliverables, `AnsweredAskCard` already draws it. Not `secret_request`,
`desktop_handoff` or approvals. Test: `[a, b, ask_user, c, d]` → `[[a, b], ask_user, [c, d]]`; a lone one between text
rows stays a plain entry.

## Gates (all via `build-lock`, crate-level)
- `cargo fmt -p tools -p rpc -p agent`; `cargo clippy -p tools -p rpc -p agent --tests -- -D warnings`: clean.
- `cargo test -p tools`: 173 passed, 1 ignored. `cargo test -p rpc`: 50 + 1 passed. `cargo test -p agent reminder`: 3 passed.
- Bindings regenerated (`cargo test -p rpc --features ts --lib export_bindings`, 8 passed) and copied; not committed.
- `bun run check` (tsc): clean (probe with a deliberate error confirmed it checks `src`).
- Scoped vitest (`src/store src/components/sidebar ToolRunCard.test.ts composer-focus.test.ts`): 11 files, 91 tests passed.
- No biome config in the repo; skipped.

## Not verified live / open items
- PTY glyph after a seed (the snapshot's `ptys` list) — code path only; `JobStarted.pty` keeps working as before.
- Engine switch seed relies on `attach()` seeing the new stream's `PendingPrompts`; not exercised with two engines.
- Phone/tablet behaviour of B is by the `touch()` guard; not run on a device.
- Older engines without `list_live_counts` log one `console.warn` per seed; the live events still work.
- `biome-ignore`-style directives are not used (no biome); the `[focused]` effect intentionally omits `refocus`.
