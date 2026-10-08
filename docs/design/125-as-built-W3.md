# 125 as-built — W3 (desktop/mobile)

Branch `feat/125-w3`, cut from the contract commit `d3c5b682`. Spec: `docs/design/125-advisor.md` §3, §8, §10 "W3", §11 "W3";
built against C's as-built (`125-as-built-C.md`). Everything under `crates/desktop/ui/src`; nothing outside it was touched.

## Built (file-level)

- `store/sessions.ts` — `Entry` gains `{ kind: 'advisory'; notes: AdvisoryNote[]; seq?: number }` (+ the shared `at`/`context`);
  `SessionSlice.advisorModel` (the last `AdvisorChanged` model, for the `advisor → …` line); `setChildren` drops rows with
  `kind === 'advisor'` (the one entry point of `list_agents` for the tree, the strip and the Agents page live rows).
- `store/reduce.ts` — `historyEntries` maps `role === 'advisory'` to one advisory card (replaces C's placeholder);
  `apply` handles `AdvisoryAdded` (`addAdvisory`: one card per stored row, matched by `seq`, else by its notes and then
  stamped — the event, the `<advisory>` wrapper echo of a steer's `InputInjected` and the settled reload all describe the
  same row) and `AdvisorChanged` (one `advisor → <model | off>` info line when the model moves; state ticks print nothing);
  `userEntries` folds the `advisory` wrappers of one input into one card; `signature` keys advisory cards by `seq`
  (notes when unstored) so `adopt` keeps ids across reloads.
- `lib/injected.ts` — `Injected` gains `{ kind: 'advisory'; severity; text }`; `<advisory severity="…">` is a `CARD`
  wrapper, so a `TurnStarted` made of advisories is `wrapped` and never a `you` bubble (the no-echo rule).
- `lib/advisor-format.ts` — new: `worstSeverity`, `TONE` (nit → info, concern → warning, blocker → danger tokens),
  `cardTitle`, `NOTES_SHOWN = 3`, `pillLabel`, `noteCount`, `STATE_FG`, `contextLine`, `tokensLine`, `detailLine`.
- `components/chat/advisor.ts` — new: `useAdvisor(session)` (`advisor_status` under `queryKeys.advisorStatus`, `retry: false`)
  and `useSetAdvisor(session)` (`set_advisor`; the reply is written into the query; errors toast).
- `rpc/extra.ts` — `Info.advisor?: AdvisorDefaults` (the hand-written `Info` the connection store carries).
- `lib/rpc.ts` — `queryKeys.advisorStatus(id)`; `useInvalidateOn` writes every `AdvisorChanged.status` into that query
  (`setQueryData`, no refetch).
- `components/chat/AdvisorNoteCard.tsx` — new: the `EngineNotice` shell tinted by the worst severity, header
  `Advisor · N notes` + first note + time (`formatAt`/`fullAt`), one row per note with a lower-case outline `Badge`,
  three shown, `Show more (N)` / `Show less`; `collapseNotes` exported for the test. `components/chat/Message.tsx` —
  `body` arm, `describe` ("advisor note"), `Card`-wrapped (context-remove button) like peer/engine cards.
- `components/chat/rows.ts` / `ToolRunCard.foldRuns` — **no change needed**: `foldRuns` only rides `tool`/`reasoning`
  rows, so an advisory card already splits a run (pinned by `AdvisorNoteCard.test.ts`).
- `components/chat/ModelMenu.tsx` — additive only: `Advisor · <model name | Off> ›` row under 124's `ModelPicker`
  (sessions only, never the draft), opening an Advisor level = `← Back` row + `ModelPicker` with
  `none={{ value: 'off', label: 'Off' }}`, `extra` for an off-catalog advisor model and the advisor model's effort
  levels; picks call `set_advisor` only. `openAdvisorPicker(session)` dispatches `theuth:open-advisor` (the pill's
  `Change model…`), handled by `useOpenAdvisor` in `ModelMenu` and `ModelDialog`. `Model.done` exposed.
- `components/chat/SubagentStrip.tsx` — `trailing?: ReactNode`; the row renders when dots or `trailing` exist.
  `routes/c.$session.tsx` passes `<AdvisorPill/>` as `trailing` only while `useAdvisor(session).state !== 'off'`, so the
  row stays absent with no advisor and no live subagent.
- `components/chat/AdvisorPill.tsx` — new: `Eye`/`EyeOff` pill, `idle` · `reviewing…` (`motion-safe:animate-pulse`) ·
  `N notes` · `paused` · `halted`, colours by `STATE_FG`, `aria-label="Advisor: <state>"`; tap → `Popover` (desktop) /
  bottom `Sheet` (`useIsMobile`) with Model · Effort · State · Context · Tokens · Cost (`costShort`) · the detail line ·
  last note · `Change model…` · `Turn advisor off`.
- `components/settings/Advisor.tsx` — new tab `advisor` **Advisor** after Agent loop (`SettingsDialog.tsx`): Default effort
  (`Select` Default + `EFFORTS`), Notes per update (`Count` 1–32), Immune turns (`Count` 0–10), Catch-up (`Select` Off · 1
  · 3 · 5) over `Info.advisor` with `refreshInfo` after each `set_advisor_*`. `components/settings/shared.tsx` — `Count` (moved
  from `Loop.tsx`, gains `max`) and `bounded(raw, min, max)`; `Loop.tsx` imports `Count` from there.
- Exclusions (§8.5): `lib/session-tree.ts` `notAdvisor` used by `stripRows`, `childrenOf`, `subtrees`;
  `components/sidebar/SessionList.buildRows` roots; `components/chat/broadcast.groupRows`; `rpc/extra.ts` `AgentInfo.kind?`.
- Tests: `store/advisor.test.ts` (7), `lib/advisor-format.test.ts` (4), `components/chat/AdvisorNoteCard.test.ts` (2),
  `components/settings/Advisor.test.ts` (2), `lib/session-tree.test.ts` (+1 exclusions).

## Gates (exact result lines)

- `cd crates/desktop/ui && BUILD_LABEL=125-W3 build-lock nice -n 10 bun run check` → `$ tsc -b --noEmit`, exit 0.
- `cd crates/desktop/ui && BUILD_LABEL=125-W3 build-lock nice -n 10 bunx vitest run --maxWorkers=2 src/store src/lib src/components/chat src/components/settings src/components/sidebar src/rpc`
  → `Test Files  124 passed (124)` / `Tests  991 passed (991)` (the 16 new tests included; `rpc/api.test.ts` green against
  the copied `bindings/methods.json`).
- Bindings: copied from the integration tree (`theuth-125-advisor`, C's regeneration at `d3c5b682`) per Sky's 11:xx note
  instead of `just ts-desktop`; no cargo was run by this wave. Both binding dirs are gitignored and uncommitted.
- First `tsc` run was red on two points, both mine: `Info.advisor` missing from the hand-written `rpc/extra.ts` `Info`
  (the bindings' `Info.ts` has it; the UI reads `extra.ts`'s) and a test fixture typing nit. First vitest run failed one
  suite: `Advisor.test.ts` imported `Count` from `Loop.tsx`, whose `lib/nudges` import touches `localStorage` at module
  init under node — `Count` moved to `settings/shared.tsx` (where `bounded` lives), `Loop.tsx` imports it from there.

## Deviations from the design (and why)

1. **`Entry.advisory` carries `notes` + `seq`, not the whole `Advisory`.** The card only draws the notes and the time;
   `seq` is the idempotence key `AdvisoryAdded` and the reload share. Carrying `Advisory` would force a fake `advisor`/`step`
   for the card parsed from `<advisory>` wrapper text (an older engine's `user` row, or a steer's `InputInjected`).
2. **The Advisor row sits under 124's picker, not inside its `Command` as a cmdk group.** `ModelPicker` owns its
   `Command` and is 124's file (not to be edited); a plain lead-row button below it is the smallest additive change and
   leaves the picker's structure alone for 127's favourites merge.
3. **`route()` does not filter `SubagentSpawned` by kind**: the event carries no `kind` (`lib.rs:2144-2155`), and the
   engine publishes it only from the `SpawnChild` handler (`supervisor.rs:3561`), which the advisor's creation
   (`Agents::session` directly, §4.1) never goes through. `setChildren` / `stripRows` / `childrenOf` filter `AgentInfo.kind`.
4. **`AdvisorChanged` is held in the TanStack query, not a slice field.** `useInvalidateOn` writes the event's status
   into `queryKeys.advisorStatus(session)`; the slice keeps only `advisorModel` for the `advisor → …` info line.
5. **Reviewing pulse** is Tailwind's `motion-safe:animate-pulse` on the icon rather than a `subagent-blink` clone keyed on
   `step` (no new CSS; reduced motion respected).
6. **`AdvisorNoteCard` props** are `{ notes, at }` (see 1), summary shows the first note truncated so a collapsed card says
   what it is about.

## Hand-offs for the closer

- None required to compile or run. `rpc/api.ts` and the bindings are C's and were used as is.
- Optional: `useInvalidateOn` could also refetch `queryKeys.sessionCost(session)` on `AdvisorChanged` if W1 does not emit
  `ModelUsageChanged` for the advisor child's calls (the advised total should include it, §4.14).

## Open items

- `Change model…` from the pill opens the header picker (`theuth:open-advisor`); in a split pane layout only the pane whose
  `ModelMenu`/`ModelDialog` is mounted for that session responds (both filter by session id).
- No desktop `/advisor` slash command (design §13 item 6: none in v1).

## F2 (phone picker footer)

- **What/why.** On touch the model dialog is capped to `--visible-band` (127 §6) as a flex column; the shadcn `Command`
  root is `h-full`, so the picker claimed the whole band and the sibling `Advisor · …` row was pushed past the cap and
  clipped by `overflow-hidden` — under the keyboard the row was invisible. Now `dialogPlacement(touch)` also returns
  `picker: 'h-auto min-h-0 flex-1'` (merged into the `ModelPicker` root, both the model and the Advisor level) and
  `footer: 'shrink-0'` (new optional `className` on `AdvisorRow`), so the picker shrinks and the row is the band's last row.
  Desktop / popover paths get empty strings and are unchanged.
- **Files.** `src/components/chat/ModelMenu.tsx`, `src/components/chat/ModelMenu.test.ts` (placement assertions).
- **Gates (build-lock, label 125-F2).** `bun run check` exit 0 (`tsc -b --noEmit`; the setup baseline's 958 errors were
  missing generated bindings, cleared by copying them) · `vitest run src/components/chat src/store src/lib`:
  `Test Files 133 passed (133)`, `Tests 1067 passed (1067)` · `bun run build`: `✓ built in 6.67s`.
- **Proof.** Headless Chrome 390×844 mobile against the live engine, `--visible-band` forced to 420 px, existing chat,
  picker opened from the compact header menu: `/tmp/125-f2-before.png` (picker 9–411 px, Advisor row not visible) →
  `/tmp/125-f2-after.png` (picker 9–358 px, `Advisor · Off` at 374–411 px inside the band).
