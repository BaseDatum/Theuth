# 136 F3 — as-built: UI fixes from review R

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-w4`, branch `feat/136-f3` (cut from `aa9696fe`) · owns `crates/desktop/ui/**` and this file. No Rust touched. Commits: `970ab838` (the fixes) and the closing commit named in the final message.

## 1. Findings fixed

| Finding | Where | What |
|---|---|---|
| R-14 (UI) | `src/lib/dreaming.ts:161` `resolvedDefault`, `src/components/settings/Dreaming.tsx:176-186` | The picker's `Default (<model>)` label reads the server's `DreamSettings.resolved.<tier>` when present (judge = propose renders `, same as Propose`); without it, Tidy falls back to the routine row's `model:` (`ScheduleInfo.model`, only while no pick overrides it), then the title model, then the subagent model (the W4 rule). The Tidy pick itself was already persisted through `dreaming.set_settings` (`reduceSettings` `model` arm → `save.mutate`) and shown back from `settings.models.tidy`; smoke-verified across an SPA navigation. `bindings/DreamSettings.ts` carries a hand-written optional `resolved?: { tidy, propose, judge, target }` (gitignored, uncommitted) until F2's ts-rs export lands. |
| R-34 | `src/lib/dreaming.ts:33` `budgetFromInput`, `:124` reducer, `Dreaming.tsx:134-172` | A budget is `0 < usd ≤ 1000` (`MAX_BUDGET_USD`, F2's `SetSettings` bound). The pane's field validates on blur/Enter (as `Count` in `shared.tsx`): invalid → the saved value is written back into the field, `aria-invalid`, inline `BUDGET_PROBLEM` under the description; valid → save and the error clears. The reducer refuses `≤ 0` and `> 1000` too, so no caller can save `$0`. The Dream now dialog shares `budgetFromInput` (Start disabled on empty/0/over). |
| R-46 (UI) | `src/lib/dreaming.ts:95` `dreamNowBlocker`, `:105` `dreamNowUnavailable`, `Dreaming.tsx:93, 97, 115, 310-313` | Dream now is disabled when `status.enabled` is false, the routine row is switched off, `paused_until` is in the future, or the routine row is `running`; precedence in that order. The reason is the button wrapper's `title` and a note under the status card (`Dream now is unavailable: …`), so it is visible on mobile where there is no tooltip. The dialog's budget opens at `settings.budget_usd` (`DEFAULT_BUDGET_USD` until settings load); `RUN_NOW_BUDGET_USD` ($5) removed. Mode stays `pass`: settings carry no mode. |
| R-48 | `src/lib/dreaming.ts:30-31` `usd` | `Intl.NumberFormat('en', { style: 'currency', currency: 'USD' })`: `$1.00`, `$4.10`, `$1,000.00` in the status card, the budget row, the Recent nights list and the Routines row subtitle (`nightLine`). Doc comments updated from `$4.100`. |
| POLISH wording | `Dreaming.tsx:138`, blocker strings | "default $1.00, up to $1,000.00." replaces "default $1.000. Any amount."; blocker/notes use the "night" vocabulary. No other POLISH finding names a UI file. |

## 2. Fixture (`src/dev/dreaming-fixture.ts`)
- `RESOLVED` sample (`tidy`/`propose` haiku, `judge`/`target` gpt-5.6-sol) on the `night` and `running` states; `empty` has none so the client fallback path renders too.
- New state `running`: `RUNNING_NIGHT` (`finished_at: null`, partial counts, $0.27) and the routine row `running: nights.some(finished_at === null)`.

## 3. Tests (`src/lib/dreaming.test.ts`, 15 tests)
- R-48: `usd(1) === '$1.00'`, `usd(4.1) === '$4.10'`, half-up at two decimals, `$1,000.00`; night/undone/quiet/running lines at two decimals.
- R-34: reducer refuses `0`, `-1`, `1000.01`, `NaN`, no-op; accepts `0.01`, `1000`. `budgetFromInput('')`, `'   '`, `'0'`, `'-2'`, `'abc'`, `'1e9'` → `null`; `'0.5'`, `'1000'` → numbers.
- R-46: `dreamNowBlocker` null when live; each reason; expired pause is not a blocker; config-off beats running; `dreamNowUnavailable` sentence.
- R-14: `resolvedDefault` prefers `resolved.*` (incl. judge = propose → `same`), ignores `routine` when `resolved` is present; without it the routine `model:` wins for Tidy, then title/subagent/session rules.
- No `Dreaming.test.ts`: the pane exports nothing pure; every decision lives in `lib/dreaming.ts`.

## 4. Verification
Gates through `build-lock` (label `136-F3`, from `crates/desktop/ui`): `bun run check` green, `bunx vitest run src/lib/dreaming.test.ts` 15/15, `bun run build` green.
Smoke in headless Chromium (vite dev + a throwaway Bun JSON-RPC daemon answering `info`/`list_models`/`title_model`/`subagent_model`, deleted afterwards; the dev fixture for the `dreaming.*` methods):
- Desktop 1280 px, `night`: card `$4.10` / `$0.40 of $2.50`; Propose shows `Default (Claude Haiku 4.5)` from `resolved`; budget cleared + Enter → field back to `2.5`, `aria-invalid`, inline error; `5000`/`0` → same; `3` → saved, card `$0.40 of $3.00`; Dream now opens at `3`, Start disabled on empty/`0`, enabled on `12`.
- Blockers: `paused` → disabled, note "Dream now is unavailable: dreaming is paused until 10/13/2026."; Resume → enabled; routine switch off → "the dreaming routine is switched off"; `running` → "a night is already running"; `disabled` → "dreaming is off in config (dreaming.enabled: false)"; `empty` → enabled, no note.
- Tidy pick → Claude Fable 5.1, persists across `/settings/general` → `/settings/dreaming` (SPA).
- Mobile 390 px (`paused`, `night`): no horizontal overflow (`scrollWidth === 390`), the blocker note wraps under the card instead of crowding the header, undo list `$4.10`/`$0.93`/`$0.00`.

## 5. Hand-offs
- **F2**: `DreamSettings.resolved: { tidy, propose, judge, target }` (strings) — the UI reads it optionally; regenerate the bindings and the hand-written line in `bindings/DreamSettings.ts` is replaced. If F2 marks judge-same-model differently from `judge === propose`, the `, same as Propose` suffix follows `resolvedDefault`.
- **F2**: the UI bound `0 < budget ≤ 1000` mirrors D13; change `MAX_BUDGET_USD` if the server bound changes.
- `routine.running` (`ScheduleInfo.running`) is the "night in flight" signal; an unfinished `NightSummary` (R-49) is deliberately not used so a stale record cannot pin Dream now off.

## 6. Skipped
- Dream now mode default from settings: settings carry no mode; `pass` stays.
- A DOM-level pane test (no jsdom/testing-library in the UI package); covered by the lib tests plus the smoke above.
