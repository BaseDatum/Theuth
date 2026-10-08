# 125 as-built — R1 W3 (desktop/mobile, CONSULT)

Branch `feat/125-r1-w3`, cut from C's contract commit `d243f863`. Spec: `docs/design/125-advisor.md` R1.2 (desktop/mobile
column), R1.7 (TS shapes), R1.8 W3 rows, R1.9 W3; built against `125-as-built-R1-C.md` and yesterday's `125-as-built-W3.md`.
Everything under `crates/desktop/ui/src`; `rpc/api.ts` and `rpc/bindings/**` (C's) used as is. No cargo.

## Built (file-level)

- `lib/advisor-format.ts` — `RANK.advice = 0` / `TONE.advice` (info tone; C's compile edit, kept); `cardTitle(count, question?)`
  returns the question for a consultation, `Advisor · N notes` otherwise; `isConsultation(a)` narrows the flattened
  `Advisory` union to `{ kind: 'consultation'; question }`; `consultsLine({ turn, total, cap })` → `2/6`, `2/6 · 9 total`,
  `off` when `cap === 0`.
- `store/sessions.ts` — the `advisory` `Entry` gains `question?: string`. `store/reduce.ts` — `questionOf(advisory)` sets it in
  `historyEntries` (`role === 'advisory'`, masked rows included — the card is drawn from the row regardless of `context.removed`)
  and in the `AdvisoryAdded` arm; `addAdvisory`'s `seq` idempotence is unchanged.
- `components/chat/AdvisorNoteCard.tsx` — `question?` prop: the summary line shows an outline `Consulted` badge + the question
  (truncated), the second line the answer's start; the body lists the answer without a severity chip (the `advice` chip would
  repeat the badge). Reviews render exactly as before. `components/chat/Message.tsx` — passes `entry.question`; `describe`
  says `consultation` for the card's context-remove label.
- `components/chat/AdvisorPill.tsx` — popover/sheet rows: the pairing `warning` as a `text-warning` note directly under `Model`;
  `Consults` row (`consultsLine`) after `State`.
- `components/settings/Advisor.tsx` — row `Consults per turn`: `Select` Off · 2 · 6 · 12 (`CONSULTS`, `consultsLabel`,
  `asConsults` 0–32) → `api.set_advisor_max_consults(n)` then `refreshInfo`; `consultOptions(cur)` appends a hand-edited
  off-list value (e.g. 32) so the select never shows blank. The row's description carries the static pairing note ("An advisor
  priced below the session's model is flagged in its pill").
- Tests: `lib/advisor-format.test.ts` (+2: `TONE.advice`, mixed-card `worstSeverity` never `advice`, `cardTitle` consultation
  form, `isConsultation`, `consultsLine`), `components/chat/AdvisorNoteCard.test.ts` (+1: `renderToStaticMarkup` — question as
  summary, `Consulted` badge, answer body, no chip; a review unchanged), `components/settings/Advisor.test.ts` (+1: options
  0/2/6/12, labels, off-list value, `asConsults` bounds), `store/advisor.test.ts` (+2: `AdvisoryAdded` consultation yields one
  card with `question`, idempotent on `seq`; the masked history row (`context.removed`) arrives as `advisory` with `question`).

## Gates (exact result lines)

All through `BUILD_LABEL=125R1-W3 /Users/Sky.Moore/.theuth/bin/build-lock …` in `crates/desktop/ui`, after `.ready` (16:06).

- `bun run check` → `$ tsc -b --noEmit`, exit 0 (no output).
- `bunx vitest run --maxWorkers=2 src/store src/lib src/components/chat src/components/settings src/rpc` →
  first run `Test Files  1 failed | 141 passed (142)` / `Tests  1 failed | 1112 passed | 1 skipped (1114)`: my own assertion
  that an advice-only card's `worstSeverity` is `advice` — the floor is `nit` by design (both tint info; `advice` never wins),
  the assertion was corrected to `nit`. Rerun of the four R1 suites (`lib/advisor-format`, `store/advisor`,
  `components/chat/AdvisorNoteCard`, `components/settings/Advisor`) → `Test Files  4 passed (4)` / `Tests  21 passed (21)`;
  the other 138 files (incl. `rpc/api.test.ts` against C's `methods.json`) were green in the first run and untouched since.
- `bun run build` → `✓ built in 5.34s`, exit 0 (the pre-existing >500 kB chunk warning only).
- Bindings and `node_modules`: cloned from `theuth-125-r1-int` by the setup script; no cargo run by this wave.

## Deviations from the design (and why)

1. **`consultsLine` prints the link total too** (`2/6 · 9 total`) when non-zero: R1.2 writes `Consults N/6`; the TUI's fifth line
   shows the total and the popover has the room. `cap === 0` prints `off` instead of `N/0` (the tool is absent).
2. **The Settings "card note" for the pairing warning is static text** in the `Consults per turn` description: Settings reads
   `Info.advisor` (defaults), not a session's `AdvisorStatus`, so there is no per-link warning to show there; the live warning
   is in the pill popover under `Model`.
3. **`set_advisor_max_consults` is not called for a value the UI cannot parse** (`asConsults` guards 0–32); the Select only
   offers the four choices plus the live value, so this is belt-and-braces like `asBacklog`.
4. **`reduce.ts` narrows the `kind` tag inline** (`questionOf`) rather than importing `isConsultation`: `advisor-format.ts`
   imports `k` from `components/chat/CostPill`, which would pull a React/TanStack module into the store graph.
5. The consultation card body hides the per-note severity chip; the `Consulted` badge in the header is the kind marker.
6. **`worstSeverity` keeps its `nit` floor**: an advice-only card tints as `nit` (the same info tone), so R1.7's "never promotes
   a card to Advice" holds trivially and `TONE.advice` is only reached through `TONE[n.severity]` chips, which the consultation
   body does not draw.

## Hand-offs for the closer

- None required. `rpc/api.ts` already exposes `set_advisor_max_consults` (C).

## Open items

- R1.11 #9 (`/advisor ask`) has no desktop surface; nothing here anticipates it.
