# TOOL-COLLAPSE as built — consecutive tool calls render as one collapsed status card

Branch `feat/tool-call-collapse`, worktree `theuth-toolc`, base `9f772b9`. Desktop UI only; no engine or wire change.

## What the user sees

- In a reply, a run of two or more consecutive tool calls is one card. Collapsed (default) its header reads
  `running \`read_file\` · 3 of 5` with a spinner while a call is unfinished, then `5 tools · 2.1 s · 1 failed (edit)`
  once every call has a result. Under the header: the last completed call's `✓/✗ first line (+n lines)`.
  A failed call tints the header and border with the danger tokens so it is not hidden.
- Click/chevron toggles. Expanded, the run renders exactly the cards the transcript showed before this change, one
  `Message` per entry (plain, artifact, scrape, reminder, nested `↳` python children, ACP rows) with their own expand behaviour.
- Approvals / `ask_user` / `secret_request` prompts waiting on a call inside the run render under the header whether or
  not the run is open: a prompt never hides inside a collapsed run.
- A lone tool call renders as today's single card: a header wrapping one card would add a click and hide nothing.
- Settings › Chat › **Collapse tool calls by default** (on). Off is exactly the previous transcript.

## Grouping rule (`foldRuns`)

A run starts at a tool row and extends over following `tool` and `reasoning` rows; it ends at the last tool before any
other row (assistant text, user, peer, engine, vision, compacted, …). Reasoning between two calls rides inside the run
(hidden while collapsed, in place when expanded); a trailing reasoning row stays outside so a live `Thinking…` after a run
is never swallowed. This is the brief's "no assistant text between them" applied to the flat `Entry[]`: entries carry no
per-turn marker client-side, and a run cannot cross a user row because a user row is not `tool`/`reasoning`.

## Files

- `crates/desktop/ui/src/components/chat/ToolRunCard.tsx` (new)
  - `foldRuns(entries): (Entry | Run)[]` — the rule above; `Run = Entry[]`.
  - `summarize(run): { text, running, failed, hint }` — header text, live vs done, failure flag, last result hint.
    Duration is `last.finishedAt − first.startedAt`, `x.x s` under a minute, `n min` above; omitted when either stamp is
    missing (history rows).
  - `ToolRunCard({ run, prompts, session })` — header button (`aria-expanded`), `ChevronRight` rotating, `⚙`, spinner,
    danger tint; open → `run.map(Message)`; then the prompts.
- `crates/desktop/ui/src/lib/tool-runs.ts` (new) — `useToolRuns`: `collapse` (localStorage `collapse-tool-calls`,
  default on, same shape as `lib/nudges.ts`) and `open: Record<callId, boolean>` + `toggle(id)`, in memory only.
  Keyed by the first call id, not the entry id, so an open run stays open across the settled reload that replaces live
  entries with history rows.
- `crates/desktop/ui/src/components/chat/Transcript.tsx` — `Row` gains `{ key, run, prompts }`; `buildRows` takes
  `collapse` and works over `foldRuns(entries)` (or `entries`), attaching a prompt to the run that holds its unfinished
  same-named call; the virtual row renders `ToolRunCard` for run rows. Row key = first entry id, so a growing run keeps
  its virtualizer row and `measureElement` re-measures as calls land. `Message`, `ticksOf`, paging, ruler untouched.
- `crates/desktop/ui/src/store/sessions.ts` — tool `Entry` gains optional `startedAt` / `finishedAt` (ms).
- `crates/desktop/ui/src/store/reduce.ts` — `ToolCallStarted` stamps `startedAt: Date.now()`, `ToolCallFinished`
  stamps `finishedAt`. The wire events and `HistoryEntry` carry no times, so reloaded runs show no duration.
- `crates/desktop/ui/src/components/settings/Chat.tsx` (new) + `SettingsDialog.tsx` — `chat` tab (`MessageCircle`)
  after General with the one switch.
- `crates/desktop/ui/src/components/chat/ToolRunCard.test.ts` (new) — `foldRuns` (split by assistant text, reasoning
  rides along, trailing reasoning stays out, lone call stays plain), `summarize` (live position text and hint; done
  count/duration/failure naming; no duration without stamps), and a static render of a 3-call run: collapsed → one
  header, no tool cards; open → three cards. The render test mocks `@/lib/tool-runs` with a plain object because
  `renderToStaticMarkup` reads zustand's server snapshot (the initial state), never a later `setState`.

## Not changed / out of scope

- `SearchSession`'s read-only "Earlier messages" page renders `Message` rows directly; it is unaffected (no grouping there).
- Nested python children (`call_id` with `.`) and ACP rows are ordinary `tool` entries and fold like any other; expanded
  they render with their `↳` glyph as before (06 §3.6 / 08 as-built 2026-09-24 reload behaviour untouched).
- Popped-out chat windows and the note panel's `compact` transcript are the same `Transcript` and get the same card.
- Open state is not persisted; a new window starts every run collapsed.

## Verification

- `bunx tsc --noEmit` 0; `bunx vite build` ok; `CI=1 bunx vitest run` 83 files / 502 tests green.
- Browser smoke (Vite dev at :5185 against the live headless engine, session `01a0d418-…`, read-only): collapsed runs
  `15 tools`, `10 tools`, `6 tools · 1 failed (python)` (danger tint); expanding the failed run shows its 6 cards incl.
  four `↳ screen` children; a client-side synthetic turn (events applied to this tab's store only) showed
  `running \`read_file\` · 3 of 5` with spinner and the last hint, then updated in place to
  `5 tools · 33.6 s · 1 failed (edit)` without opening; 390 px viewport renders the same header; Settings › Chat switch off
  → 0 run headers, individual cards back. Screenshots `/tmp/69/toolc-{collapsed,running,expanded,mobile}.png`.
