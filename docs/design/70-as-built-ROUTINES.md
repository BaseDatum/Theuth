# 70 — As built, worker ROUTINES: schedules become a sidebar area "Routines"

*Branch `feat/routines-sidebar`, 2026-09-25. Sky's ruling: "build real schedules UI (paperclip calls them routines) and move that into the sidebar and out of settings." Reference studied: paperclip `ui/src` (`Routines.tsx`, `RoutineList`, `RoutineOverview`, `RoutineHistoryTab`, `RoutineContextualSidebar`, `ScheduleEditor`, `api/routines.ts`).*

Vocabulary: **Routine** in the UI; `schedule` stays in code, RPC names, files, and query keys.

## What paperclip does, and what was copied

- List rows (`RoutineListRow`): title + a muted state word (`paused` / `draft` / `archived`), a second line with project · agent · last run, and a right-hand cluster **Run now** + on/off toggle; the row is a link. Copied: name + state word (`paused` / `broken`), second line cadence · next · last · chips, Run now + Switch on the right.
- Detail (`RoutineDetail.tsx`): header with title, Run button and toggle; overview facts (state · triggers · next run · last run); sections as tabs bound to the URL (`/routines/:id/:section`); runs table linking each run to its task; Version history. Copied: the four facts (Cadence · Next run · Last run · Session/Loop runs), URL-bound sections **Overview · Prompt · History · Danger**, run rows linking to the run's session.
- `ScheduleEditor`: preset select + live text preview of the cron. Copied as kind select (`cron` / `every` / `at`) + input + live words preview (`cronWords` for common shapes, else the engine's croner text after save); `at` uses the native `datetime-local` input.
- Not copied (paperclip only, no theuth counterpart): folders/grouping, variables/secrets/delivery sections, webhook triggers, revision history, archive state. Paperclip has no delete; theuth's Danger tab deletes the file because the file *is* the routine.

## What was built, per commit

### `feat(schedule): get/put/delete schedule RPCs + ScheduleFile binding for the routines editor`
- `crates/schedule/src/file.rs`: `ScheduleFile` (serde + schemars + `ts` export) — every front-matter key **as written** (`cron`/`every`/`at`, `quiet`, `missed_run`, `model`, `enabled`, `until`, `max_runs`, `cwd` unexpanded, `approvals`, `agent`, `project`) plus `prompt`. `ScheduleFile::read(name, text)` parses only the YAML (lenient: a file that fails `parse` — no clause, empty prompt, bad cron — still opens in the editor so it can be repaired). `ScheduleFile::args()` borrows it as `FileArgs`. `FileArgs`/`render` gained `missed_run`, `agent`, `project` so a save never drops those keys (they were parsed but never rendered before). Test `schedule_file_round_trip`.
- `crates/schedule/src/lib.rs`: messages `Get { name } → Result<ScheduleFile, String>`, `Put(ScheduleFile) → Result<ScheduleInfo, String>` (name rule → `render` → `parse` validates → `write_atomic` → reconcile → current row), `Delete { name } → Result<(), String>` (`remove_file` → reconcile, so the history gets its `removed` row exactly as a manual delete would). The `Reconcile` handler body moved into `Schedule::reconcile(&ActorRef<Self>)` so Put/Delete reuse it. `ScheduleInfo` gained `agent` and `project` (the list chips).
- `crates/schedule/src/tools.rs`: the `schedule` tool's `FileArgs` literal passes `missed_run: false, agent: None, project: None` (3 lines).
- `crates/rpc/src/api.rs`: `get_schedule(name)`, `put_schedule(file)`, `delete_schedule(name)`; `crates/rpc/src/export.rs` exports `ScheduleFile`.
- `crates/desktop/ui/src/rpc/api.ts`: the three methods + `METHODS` entries.

### `feat(ui): Routines in the sidebar + /routines list`
- `components/sidebar/AppSidebar.tsx`: one `SidebarMenuItem` line (`CalendarClock` · Routines) after Agents, plus the icon in the existing lucide import line.
- `components/routines/helpers.ts`: `relative` (moved from Settings › Schedules; `chat/Peers.tsx` re-pointed), `took`, `runCount`, `cronWords` (`every day 09:00`, `weekdays 09:00`, `every Friday 18:00`, `every 15 min`, `hourly at :05`, `monthly on day 1 09:00`, else `null`), `cadence(info)` (`every 5m` · `once at 16 Sep 15:00` · cron words or the engine's `describe`), `resultKind(last_result)` → `ok | failed | skipped` (`end_turn`, `done…`, `max_runs…` ok; `skipped…` skipped; the rest failed), `sortRoutines` (broken first, then enabled by soonest `next`, then paused; name ties), `SECTIONS`/`isSection`, `movedSettingsTab`.
- `components/routines/queries.ts`: `KEY = ['schedules']`, `useRoutines()` (list + invalidation on `Fired`/`Skipped`/`Changed` and on `TurnFinished` of a routine's session), `useRoutineFile`, `useRoutineHistory` (50 rows), `useNotHub()` (`mesh_status`: lease held by another engine → the **not hub** chip; no mesh = hub).
- `components/routines/RoutinesView.tsx` + `routes/routines.index.tsx`: header with **New routine**; rows: name, `broken`/`paused` word, `loop · runs 1/5` for loops, cadence in words (title = the raw clause), `next in 2d`, `last 3m ago`, chips (running pulse, result, not hub, `@agent`, project, approvals off), **Run now** (opens the session), enabled Switch. Empty state and the one-line explanation of the file model.
- `helpers.test.ts`: cadence words, list sorting, result chip, settings redirect mapping.

### `feat(ui): /routines/new and /routines/$name/{overview,prompt,history,danger}; Settings › Schedules removed`
- `components/routines/RoutineDetail.tsx`: header (back, name, Run now, Switch), facts grid, section tabs as links. **Overview** = cadence fieldset (kind select, cron/interval/`datetime-local` input, live preview, `missed: run` switch for cron) · enabled / quiet / approvals switches · Agent (roster select; an agent not on the roster stays selectable) · Project (input with a `datalist` of board projects) · Model · Directory · Loop fieldset (until, max_runs; hidden for `at`, which cannot loop; shows runs so far). **Prompt** = markdown textarea. Overview and Prompt are one form: edits survive the tab switch, Save writes the whole file via `put_schedule`, Revert restores. **History** = runs newest first: fired · took · result (failed in red) · report (first line, click to unfold) · `session` link. **Danger** = delete behind an `AlertDialog`, then back to `/routines`.
- `routes/routines.new.tsx` (`RoutineDetail name={null}` → name field + all fields + prompt, **Create** navigates to the new routine's overview), `routes/routines.$name.$section.tsx` (unknown section → overview), `routes/routines.$name.index.tsx` (→ overview).
- `components/settings/SettingsDialog.tsx`: `schedules` tab removed; `components/settings/Schedules.tsx` deleted; `chat/slash.ts` `SettingsTab` union loses `'schedules'`.
- `routes/settings.$tab.tsx`: `beforeLoad` sends `/settings/schedules` to `/routines` (replace) before the unknown-tab → general rule.
- `components/agents/AgentDetail.tsx`: `Field` and `NativeSelect` exported (two `export` keywords) and reused.

### `fix(ui): routine editor keeps an off-roster agent selectable; shorter directory hint`
Found in the smoke run: a file with `agent: alice` while `alice` is not on the roster showed "run in its own session".

## Deviations from the brief
1. **Verification was run** (the brief's first block says "verification is not your job", its last block lists the checks to run; the latter is the 69 brief). Ran: `just ts-desktop`, `bun install --frozen-lockfile` (ui + packages/apps), `bun run build` (exit 0), `bunx tsc --noEmit` (0), `bunx vitest run` (66 files, 397 tests green), `cargo test -p schedule -p rpc --lib` (30 + 24 green), plus a browser smoke against a fresh `t --headless` (below).
2. **`cargo check` ran twice** for the first commit: the first found a closure-lifetime error and `ctx.actor_ref()` being a reference; the second (via `just ts`) was clean.
3. **Redirect test is on a pure mapping** (`movedSettingsTab`), not on the route's `beforeLoad`: vitest runs `src/**/*.test.ts` in node without a DOM, and route files import the whole settings tree. The browser smoke exercised the real redirect.
4. **Run now for an `agent:` routine** still surfaces the engine's `Err("created task <id>")` as an error toast — pre-existing `fire_schedule` contract, untouched.
5. **No RPC for enable/disable was added**: `enable_schedule` already rewrites only the `enabled:` line.
6. **`at` in RFC 3339 form**: the `datetime-local` input shows blank for an existing RFC 3339 `at:`; typing a new instant fixes it. `ponytail:` left as is; the model writes `YYYY-MM-DDTHH:MM`.
7. Files outside the routines area touched (disclosed): `AgentDetail.tsx` (2 exports), `chat/Peers.tsx` (import path), `chat/slash.ts` (one union member removed), `tools.rs` (3 literal lines), `routeTree.gen.ts` (regenerated by the router plugin).

## Not built
- Cron next-fire preview in the editor (paperclip's `nextCronFires`): the words preview covers the common shapes; the exact `next` appears in the facts after save.
- A dedicated markdown editor for the prompt: a monospace `Textarea`, like the agent charter.

## Smoke run (browser against `target/debug/t --headless --data-dir /tmp/69/data`, vite dev on :5199)
- `/routines`: broken file first with its parse error, loop row `loop · runs 1/5` + running pulse, `weekdays 09:00 · next in 2d · @alice · web`, paused row dimmed with `paused`.
- `/routines/digest/overview`: facts + form populated; Prompt tab → edit → Save → `digest.md` rewritten with `agent: alice`, `project: "web"`, new body.
- `/routines/watch-build/history`: one running row with `session` link.
- `/routines/paused/danger`: AlertDialog → Delete → back at `/routines`, file gone.
- `/routines/new`: name `nightly-notes`, kind `every` `12h`, max runs 3, prompt → Create → `/routines/nightly-notes/overview`, file `every: 12h\nmax_runs: 3`.
- `/routines/broken/overview`: set cron `0 7 * * *` → Save → file repaired.
- Redirects: `/settings/schedules` → `/routines`; `/routines/digest` and `/routines/digest/bogus` → `/routines/digest/overview`. Settings dialog no longer lists Schedules.
- List toggle: Pause digest → `enabled: false` on disk.

## Tests written
- `crates/schedule/src/file.rs::schedule_file_round_trip` — render/read round trip of every key incl. agent/project/missed; lenient read of a clause-less file.
- `crates/desktop/ui/src/components/routines/helpers.test.ts` — cadence words (7 shapes + fallback + once-at + `—`), list order, result chip, settings redirect mapping.

## `cargo check` result
`cargo check -p schedule -p rpc`: first run 4 errors (fixed: closure lifetime in `ScheduleFile::args`, `reconcile` takes `&ActorRef<Self>`); `just ts` (= `cargo test -p rpc --features ts export_bindings`) then compiled clean, 5 tests passed; `cargo test -p schedule -p rpc --lib`: 30 passed, 24 passed.

DONE ROUTINES
