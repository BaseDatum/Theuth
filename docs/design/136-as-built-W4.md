# 136 — As-built W4 (UI)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-w4`, branch `feat/136-w4`, cut from C's `f99f6d54` · owns `crates/desktop/ui/**` and this file. No Rust touched. Everything below renders against a fake daemon plus the dev fixture (§5); nothing here has met the real `dreaming.*` RPCs yet.

## 0. One screen

- **Settings › Dreaming** (`components/settings/Dreaming.tsx`, tab `dreaming` in `SettingsDialog.tsx` after Advisor): status card (last night · next window · budget left of budget · proposals pending · backlog fresh/untidied/open cases) · **Enabled** switch = the `dreaming` routine's own toggle (`list_schedules` row → `enable_schedule('dreaming', …)`; disabled with the config note when the row is absent, i.e. `DreamStatus.enabled: false`) · **Window** two `<input type="time">` · **Budget per night** number input with the `$` adornment from `moneyParts` · **Models** four `ModelCombobox` pickers (triage / tidy / propose / judge) over `list_models`, first entry `Default (<resolved>)` · **Auto-promote preferences** switch · **Weekly digest** day `Select` + time input over the one `digest` string · **Pause for 7 days** / **Resume** (`paused_until = now + 7 d` / `null`) · **Dream now** dialog (budget default 5, `One pass | Backlog` toggle group, the sentence "Backlog repeats passes until nothing is untidied or the budget is spent.") → `dreaming.run_now` → navigates to the run's chat · **Recent nights** (`dreaming.nights(10)`: night · mode badge · counts line · spend · `undone`) with Undo → AlertDialog → `dreaming.undo(night, null)` → toast with the `UndoReport` counts · **Proposals** (`dreaming.proposals`: title · kind · age · body · one button per option → `dreaming.proposal_act(id, choice)`).
- **Library › Knowledge**: `StatusBadge` beside the type badge in the tree, the search hits and the document header — `superseded` (a `superseded_by` pointer) > non-tidy status (`fresh`, `archived`, `merged`, `stale`, `retired`) > `verified <date>`; nothing for a plain tidy entry. Rail action **Changed by dreaming** (moon, `aria-pressed`) filters the tree to `source: dreaming ∨ status ≠ tidy`.
- **Routines**: the `dreaming` row's subtitle is `runs at night when the Mac is idle; last night: 212 retitled, 14 merged, …, $4.100` from `dreaming.status` (fallback: the trigger sentence while the status loads); hover shows the whole line. No new component beyond the inline `DreamingSentence`.
- **Api**: the nine `dreaming.*` methods typed with their dotted wire names (`api['dreaming.status']()`), in `METHODS` between `double_train` and `edit_turn`; `api.test.ts` now runs against `bindings/methods.json` (hand-written, below) and passes.
- **Dev fixture** `dev/dreaming-fixture.ts`: four states `empty · night · paused · disabled` behind `VITE_DREAMING_FIXTURE=<state>` or `localStorage['dreaming-fixture']` (localStorage wins so the state flips without a restart); the vitest reads the same `FIXTURES`.

## 1. File by file

| File | What |
|---|---|
| `src/rpc/api.ts` | imports `DreamSettings`, `DreamStatus`, `Metrics`, `NightSummary`, `Proposal`, `RunRequest`, `UndoReport`; nine `'dreaming.*'` members on `Api` (positional params per C §3); nine names in `METHODS` |
| `src/lib/dreaming.ts` | NEW pure helpers: `DEFAULT_BUDGET_USD = 1`, `RUN_NOW_BUDGET_USD = 5`, `PAUSE_DAYS = 7`, `RUN_MODES`, `BACKLOG_SENTENCE`, `TIERS`/`TIER_LABEL`, `usd()` (f64 → `Money` nanos → `formatMoney`), `USD_SYMBOL`, `countsLine()`, `nightText()`, `nightLine()`, `DAYS`/`parseDigest()`/`formatDigest()`, `pauseUntil()`, `isPaused()`, `SettingsChange` + `reduceSettings()` (returns the whole `DreamSettings` for `set_settings`, `null` for no-op/invalid; never touches `enabled`), `resolvedDefault()` (§9.2 defaults from title/subagent/session models), `undoLine()` |
| `src/lib/dreaming.test.ts` | NEW vitest: counts/night lines per fixture state, `usd`, `undoLine`, digest round-trip, every `reduceSettings` arm (budget bounds, window tuple not aliased, model tiers, digest, pause/resume, `enabled` untouched), `resolvedDefault` |
| `src/components/dreaming/queries.ts` | NEW: `useDreamingApi()` (fixture wrapper), `KEY = ['dreaming']`, `useDreamStatus()` (invalidated on schedule `Fired`, `TurnFinished`, `KnowledgeChanged`), `useDreamSettings()`, `useDreamNights()`, `useDreamProposals()` |
| `src/components/settings/Dreaming.tsx` | NEW pane: `Dreaming`, `DreamNow` (dialog), `NightRow`, `ProposalRow`, `WideRow` (controls drop under the label below `md`), `Stat`, `TIER_HELP` |
| `src/components/settings/SettingsDialog.tsx` | `Moon` icon, `['dreaming', 'Dreaming', Moon]` tab, `Pane` line |
| `src/components/routines/RoutinesView.tsx` | `DreamingSentence` for `s.name === 'dreaming'` |
| `src/components/knowledge/status.ts` + `status.test.ts` | NEW `statusBadge()`, `changedByDreaming()` + precedence test |
| `src/components/knowledge/Tree.tsx` | `Tree({ active, dreaming })` filter, `StatusBadge` component, badge in `DocRow`, empty text for the filtered tree |
| `src/components/knowledge/Explorer.tsx` | `dreaming` state, `action(…, pressed)` with `aria-pressed`, the moon action, `<Tree dreaming>` |
| `src/components/knowledge/SearchBox.tsx`, `Document.tsx` | `StatusBadge` beside the type badge |
| `src/dev/dreaming-fixture.ts` | NEW fixture (above); one wrapper per connection (`WeakMap`) so pauses/answers survive re-renders; `disabled` rejects every method but `status`/`settings` with `NOT_RUNNING` as C §3 says |
| `src/components/routines/draft.ts`, `src/dev/routines-fixture.ts` | `budget: null` on the `ScheduleFile` literals (C added the key) |
| `src/lib/attach.test.ts` | the seven new `Meta` keys on the fixture |

## 2. Deviations from the contract (and why)

1. **Model pickers are `ModelCombobox`, not a raw Radix `Select`.** The brief says "reuse the routines form's model field"; that field is `ModelCombobox` (124 §5 replaced the Select with the Popover + `ModelPicker`). Same component, same `none={{ label }}` slot used for `Default (<resolved>)`. A Radix `Select` is used for the digest day.
2. **"Resolved default" is computed client-side** from `title_model`, `subagent_model` and `info.default_model` per §9.2 (`resolvedDefault`); `DreamSettings`/`DreamStatus` carry no resolved ids. If W3 resolves them server-side differently (e.g. `agent.title_model` unset → subagent → session), the label may differ from the model actually used — hand-off below.
3. **Dream now navigates to `/c/<session>`** like the Routines "Run now" does, rather than staying on the settings pane. Routine runs are `quiet: true`; the chat still shows the run.
4. **Undo is per night (`group: null`)**; §8's per-group undo belongs to the `Dreaming` note (document) which is not in W4's list.
5. **`KnowledgeStatus` badges include `merged` and `retired`** besides the contracted five — they are real `Status` values and hiding them would mislabel a merged stub as tidy.
6. **Budget amounts print with the existing formatter's three decimals** (`$1.000`, `$4.100`): "currency label via the existing money formatter" taken literally; no float formatter was added.

## 3. Hand-written binding stubs (gitignored, NOT committed)

`crates/desktop/ui/src/rpc/bindings/` had no files in this worktree (the whole dir is gitignored and produced by `just ts-desktop`). I hand-wrote the 136 files from C's §3 "TS contract (verbatim)" blocks, copied the 611 pre-136 bindings from the user's checkout's gitignored copy (read only; byte-identical to C's for every pre-136 file, checked with `cmp`), and verified every hand-written file's shape against C's generated `~/Develop/theuth-136/bindings/*` ignoring doc comments: identical, except `ScheduleFile.ts` where my `budget` field sits last instead of before `prompt` (field order only). Also `crates/desktop/ui/src/rpc/bindings/methods.json` and the root `bindings/methods.json` (base list + the nine `dreaming.*` names) so `api.test.ts` runs.

Hand-written (58): `ApplyReport ApplyStatus Backlog Case CaseKind CaseOrigin CaseState DreamCaps DreamConfig DreamConfigModels DreamGate DreamHeavyDay DreamLifetimes DreamModels DreamNightFinished DreamNightStarted DreamPreferences DreamSettings DreamState DreamStatus DreamUndone DryRunReport EditRecord GateResult GateVerdict KnowledgeOp KnowledgeStatus LearnedEditOp Ledger LedgerCount LedgerHintUse LedgerQuestion LedgerRewind LedgerSession LedgerStop LedgerToolError LedgerUserMessage Meta Metrics NightCounts NightReceipt NightSummary Proposal ProposalKind Put Receipt RejectedEdit RunMode RunRequest ScheduleFile SessionBudget SignalsSnapshot Source Split UndoReport Uses VerifyOutcome`.ts + `methods.json`. The closer's `just ts-desktop` replaces all of it.

## 4. Hand-offs

- **Z**: run `just ts-desktop` then `bun run check` + `bunx vitest run` in `crates/desktop/ui`; the only expected drift is doc comments and `ScheduleFile` field order (no code change).
- **Z / W3**: `DreamNightStarted/Finished/Undone` are not in the desktop's `WireEvent` union binding, so `useDreamStatus` refreshes on schedule `Fired`, `TurnFinished` and `KnowledgeChanged` instead. Once the events reach the union, replace those three subscriptions in `components/dreaming/queries.ts` with `on('DreamNightFinished', …)`/`on('DreamUndone', …)`.
- **W3**: the Enabled switch reads the routine row named `dreaming` from `list_schedules` (C: `schedules/dreaming.md`). If the installed file is named differently, change the one literal in `Dreaming.tsx` (`s.name === 'dreaming'`) and `RoutinesView.tsx`.
- **W3**: `resolvedDefault` mirrors §9.2 client-side (deviation 2). If the server's resolution differs, expose it (e.g. a `resolved: DreamModels` on `DreamSettings`) and the pane shows that instead.
- **Routines form**: `ScheduleFile.budget` is now a wire key; the routines form (`RoutineDetail.tsx`) does not expose it (out of W4's deliverables; `draft.ts`/fixture just carry `null`).
- **Pre-existing**: `src/rpc/api.test.ts` resolves `methods.json` at the repo root `bindings/`, not the ui copy; worth pointing at `src/rpc/bindings/methods.json` so the test runs wherever the ui bindings exist.

## 5. Verified / not verified

Verified (through `build-lock`, label `136-W4`): `bun run check` (tsc -b) green; `bunx vitest run` over `lib/dreaming.test.ts`, `components/knowledge/status.test.ts`, `rpc/api.test.ts`, `lib/attach.test.ts`, `components/routines/helpers.test.ts`, `lib/money.test.ts` — 6 files, 90 tests green; `bun run build` (tsc -b && vite build) green. Smoke run in headless Chromium against a throwaway Bun WebSocket daemon (answered `info`, `list_models`, `title_model`, `subagent_model`, `list_schedules` with a `dreaming` row, `knowledge_list` with seven statuses; deleted afterwards) with the dev fixture: all four states render; Dream now (budget 12, Backlog) → `/c/ses-dream-now`; Undo → confirm → toast `Undid 2026-10-08 · 2 knowledge, 1 skill, 1 edit restored` and the row flips to `undone`; a proposal's first option removes it and `proposals_pending` drops; Pause → `Paused until …`/`Resume` and the card's next window says paused; Enabled switch flips through `enable_schedule`; budget `3.25` → card `$0.400 of $3.250`; window end `05:30` sticks; `disabled` state shows the config note, a disabled switch and `NOT_RUNNING` in nights/proposals; mobile 390 px: no horizontal overflow, controls stack under labels; Routines row shows the dreaming subtitle; Knowledge tree shows `stale · fresh · superseded · verified 2026-10-07 · archived` badges and the moon filter leaves the five dreaming-touched entries.

Not verified: any real `dreaming.*` RPC (C's actor answers `status`/`settings`/`nights`/`proposals` from files and the rest with `not built: W3`); the Tauri shell; `just ts-desktop` output against my stubs; a full `vitest run` (crate-level gates only, per the brief).
