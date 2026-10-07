# 128 — HUD scene: the picture at `/` (needs-you → agents → today's clock → outcomes)

## 0. Status

Status: design · 2026-10-07 · author: the orchestrator of Sky's 09:24 conversation (this text written by its design
writer from the decided implementation brief, `~/Develop/theuth-128-briefs/brief-HUD-SCENE.md`). Build: worktree
`~/Develop/theuth-128-hud`, branch `feat/128-hud-scene`, cut from main `e04f1d1a`; one omp worker, UI only
(`crates/desktop/ui`), no engine or RPC change; its report is `128-as-built-HUD-SCENE.md`. Every `file:line` below was read
on main at `e04f1d1a`; `UI/` abbreviates `crates/desktop/ui/src/`. New code has no cites — it is named by its planned path.
The approved mockups travel with this document as `docs/design/assets/128-hud-mock-dark.svg` and
`128-hud-mock-light.svg` (1920×1080, synthetic data, every colour a `styles.css` token); every coordinate in §6 is taken
from them.

Extends 70-as-built HUD-ATTENTION (the `/` route and today's `Hud.tsx`: `docs/design/70-as-built-HUD-ATTENTION.md:30-33`),
71 (the cognitive-load lens — "the reader's mechanisms first, the machinery second", `docs/design/71-whiteboard-visuals.md:1-7`),
73 (node caps and the refusal of draw-everything visualisers, `docs/design/73-codemap.md:30`, `:391`), 97 (the rail: the
THEUTH row is `/`, `docs/design/97-as-built-SIDEBAR-SIMPLIFY.md:7`, `:41`; `UI/lib/nav.ts:41`) and 123 (the diagram kernel
and the unit contract, `docs/design/123-diagram-grammars.md:307-320`, `:329-339`). Amends 70-as-built §3: the four
`Metric` cards at the top of `/` (`UI/components/hud/Hud.tsx:64-69`) are replaced by the scene; everything under them
stays. Nothing else in 70, 71, 73, 97 or 123 changes.

Every number in the implementation brief is a decision; this document explains the decisions and names the alternatives
that were declined (§9). It does not reopen them.

## 1. Sky's words and the reference

### 1.1 Sky's words, verbatim, in order (2026-10-07)

- 09:24 — "In ~/Develop/Theuth i want the main hud in the desktop/mobile app to have more of this type of visual interface.
  Reduce cognitive load to understand what the overall state is is the goal. It needs to be beautiful and communicate
  visually. What is this called, how can we create something like this for the theuth hud, the main view when you open the
  app or click theuth on the left side bar."
- 09:45 — "ok the colors are all wrong, it's the right idea, but you need to use the design colors of theuth."
- 09:5x — "ok but I want the red lines to be the gold color"
- 10:25 — "build it. new git worktree, omp subagent for implementation"

### 1.2 The reference he attached

A dark FUI-style dashboard titled AURORA. Top-left, an issues worklist (a table of ids, kinds, ages, titles). On the left,
the same issues as red-ringed dots; each dot's line flows rightwards through one of a column of owner rings, crosses a
curved date ruler (an arc with ticks), and fans out to the right as a labelled line ending in a small disc and a caption.
Hundreds of thin grey hairlines run the same way underneath as texture. Along the bottom, a row of small panels: a catalog
(an isometric scatter), owners (one dot strip per owner), arrivals (a weekday × hour punch card), verdicts (a dotted sphere
with a legend), and a stream (a log ticker).

What the picture does well, and what this design keeps: one reading direction (left to right = from what needs a person to
what came out), position as time (the ruler), a single alarm colour that is absent when nothing is wrong, texture that says
"alive" without asking to be read, and a row of small multiples that each answer one question. What it does badly, and
what §2.3 refuses: two of the five panels decorate rather than inform.

### 1.3 What `/` is today

`routes/index.tsx` renders `Hud` (`UI/routes/index.tsx:1-5`). `Hud` (`UI/components/hud/Hud.tsx:33-131`) is a paperclip-style
dashboard in Sky's 70 order: a 52 px `Overview` header with the sidebar toggle (`:48-61`), four `Metric` cards — Agents
active, Tasks in progress, Month spend, Pending attention (`:64-69`, the component at `:133-144`) — then `Section`s
Attention (`:71-73`), In flight (`:75-77`), Board pulse (`:79-95`), Activity (`:97-106`) and Spend with the mesh chip
(`:108-123`), and the new-chat `Composer` docked at the bottom (`:126-128`). The lists are the action surface: an
`AttentionCard` carries its decision inline through `KindActions` (`:201-244`, `:250-361`); `components/attention/*` import
`KIND_LABEL`, `ATTENTION_EMPTY` and `KindActions` from this file, and the brief fixes all five exports (`Section`,
`KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList`, `KindActions`) and their behaviour. The numbers come from pure functions in `model.ts` — `headline`, `badgeCount`, `appendActivity`,
`inFlight`, `runningRoutines`, `boardPulse`, `spend`, `snoozeUntil` (`UI/components/hud/model.ts:12-86`) — and from the
hooks in `queries.ts` (`useAttention` `UI/components/hud/queries.ts:27-43`, `useActivity` `:80-95`, `useAttentionActions` `:98-139`). Four numbers in
four boxes answer "how many"; nothing on the page answers "what is the shape of today" at a glance. That is the gap.

## 2. What it is called — vocabulary and principles

### 2.1 The name

The look is **FUI** — *fictional user interface*, the sci-fi HUD / mission-control style that film and game studios
(Territory Studio, GMUNK, §11) draw for screens that have to look like they know everything — applied here to real data.
FUI is an aesthetic, not a chart type; what the reference actually draws, in the vocabulary of information design, is:

| Idiom in the reference | Name | What it encodes here |
|---|---|---|
| items → owner rings → outcomes, as curved lines | alluvial / Sankey flow (one ribbon per item, not weighted) | who each needs-you item and each outcome passed through |
| the curved date ruler the lines cross | radial (arc) timeline, position on the arc = time of day | when each thing happened today |
| lines fanning to captions on the right | edge fan with labelled ends (Holten's bundled edges, unbundled at the labels) | the outcome, named |
| owners dot strips, punch card, legend counts, ticker | Tufte small multiples: dot strips, punch card, waffle, ticker | one question per panel |
| hundreds of grey hairlines | texture — the only non-semantic ink allowed | "today was busy", nothing more |

### 2.2 The principles the design is built on

1. **Preattentive semantic colour.** One hue means one thing on the whole screen, and the hue that means "needs you" is
   absent when nothing does (§4.1). The eye finds a colour before it reads a word; the screen should be decoded by colour
   first and read second.
2. **Shneiderman's mantra** (1996): overview first, zoom and filter, then details on demand. The picture is the overview;
   the lists under it (unchanged from 70) are the details; hover and the links between them are the zoom.
3. **Calm technology** (Weiser & Brown, 1996): the display lives in the periphery until something needs the centre. A quiet
   day must look calm, not broken (§4.5); motion is reserved for things that are actually happening (§7).
4. **The 71 lens:** judge every mark by what it does for the reader, not by how easy it is to draw
   (`docs/design/71-whiteboard-visuals.md:5`). §3 states the reader mechanisms first; §4–§6 are the machinery that serves them.
5. **The 73 rule against draw-everything** (`docs/design/73-codemap.md:30`, `:79-80`, `:391`): hard caps on every class of
   mark, and a count in place of what the cap cut — never a silent omission.

### 2.3 Refused from the reference

- **The dotted sphere** (verdicts): a globe of dots carries no fact that a legend with counts does not carry better; it is
  chartjunk in Tufte's sense. Replaced by a waffle of 60 dots whose fills *are* the counts (§4.3, §6.3 OUTCOMES).
- **The isometric catalog**: a 3-D scatter of nothing in particular. Dropped; the fifth panel is SPEND, which is a real
  question (§6.3).
- The rule that decides both, Sky's and the orchestrator's: **every coloured mark must decode to a fact; grey may be
  texture.** The hairline field is the one place ink exists for feel, and it is grey at ≤ 26 % opacity (§4.1).

## 3. Reader mechanisms

71 stated boards as reader mechanisms (M1–M14) before any architecture (`docs/design/71-whiteboard-visuals.md:22-24`).
The same shape here: the screen must answer four questions, one glance each, **in this order**, and each question has one
mark that answers it. Everything in §4–§6 exists to serve one of these rows; anything that serves none is not drawn.

### 3.1 The four glance questions

| # | Question | The mark that answers it | Check |
|---|---|---|---|
| G1 | Does anything need me? | **Gold.** The NEEDS YOU table and its count (top-left), the cascade of gold-ringed dots on the left, their gold lines through the rings to the arc and on to a right-edge label, the 3×10 gold bar beside the clock, and the gold footer segment `N NEED YOU`. If nothing on the screen is gold, nothing needs you. | With `attention_list()` empty, no element carries `var(--fg-accent)` except the wordmark; the table says "Nothing needs you". |
| G2 | Who is working, on what? | **The ring column.** One ring per agent (and one for theuth's own chats, one per running routine): filled faience disc = streaming now, hollow = idle, gold = waiting on you, dashed sand = paused/lapsed, dashed carnelian = unreachable or invalid. Name left of the ring, `open tasks · age` under it; faience lines from a ring to the arc and on to `title · running · elapsed`. | A ring's style is a pure function of `AgentStatus` + attention + recency (§5.3); the fixture renders all five styles. |
| G3 | What landed today? | **The malachite fan.** One line per outcome today from the actor's ring, through the arc at its time, to a labelled end on the right (`summary · $cost`), ordered by time down the right edge; the OUTCOMES waffle and `N OUTCOMES TODAY` in the footer give the count. | Outcome count on the fan + `more` = the number of qualifying activity rows (§5.4); no two end labels overlap (§6.1). |
| G4 | When did things happen? | **The arc.** Where a line crosses the band is its time of day; hour ticks and labels every 2 h; routine firings as filled squares on the outside, the next fire as a hollow square; the NOW marker. ARRIVALS (weekday × hour) gives the week's rhythm; STREAM gives the last seven events with clock times. | `arcPoint(hour)` is the only mapping from time to position; the NOW marker sits at `θ(now)` and moves once a minute. |

Reading order is also tab order (§7.3): needs-you, rings, outcomes, panels.

### 3.2 What the picture does for the reader that the cards did not

- **Shape before number.** Four `Metric` cards (`UI/components/hud/Hud.tsx:64-69`) say "6 pending, 5 in progress"; the
  scene shows six gold dots converging on two of five rings — the reader sees *where* the load is before counting it.
- **Time without a timestamp column.** `ActivityRow` prints an age per row (`:421`); the arc puts every event of the day on
  one clock, so "quiet morning, everything after 14:00" is one look.
- **Texture as a pulse.** ~230 hairlines on a busy day, a few dozen on a quiet one. The field thins and thickens with the
  day without anyone reading it (§5.6).
- **One alarm colour, two tiers.** Gold calls; carnelian means broken (over cap, unreachable, failed). Today's cards use
  `text-warning` for pending and the spend bar flips warning → danger (`:68`, `:439`); the scene keeps the two tiers but
  gives each one hue, everywhere.

### 3.3 What the picture must not do

1. **Not be the controls.** The scene replaces only the four cards. Attention (with `KindActions`), In flight, Board
   pulse, Activity and Spend stay below it, unchanged, as the action surface; a needs-you mark *links* to
   `/attention/$id`, it does not approve or dismiss anything.
2. **Not encode by colour alone.** Every state also has a shape (filled / hollow / dashed ring, filled / hollow dot or
   square) or a word in its label (`· running`, `· done`, `paused`, the legend) — §4.5.
3. **Not draw everything.** Caps: 12 needs-you dots, 9 rings, 16 outcomes, 8 running flows, 600 hairlines (§5.2); what a
   cap cuts is counted (`+N more`, a `more` count), never dropped silently (73's rule, `docs/design/73-codemap.md:391`).
4. **Not move for effect.** Three motions only — the live ring's inner disc pulses, a new flow fades in, the NOW marker
   steps each minute — and none under `prefers-reduced-motion` (§7).
5. **Not spin.** While queries load, the chrome (bar, arc, panels) renders with no marks; never a spinner over the field.
6. **Not ask to be configured.** No settings toggle, no density switch, no theme of its own: both themes come from the
   same tokens (§4.1).
7. **Not label the texture.** Hairlines have no label, no tooltip, no link; they are not units (123 §5.3) and never
   intercept a pointer.

## 4. The grammar

### 4.1 Colour semantics — role → token → where

Both themes come from the same tokens (`UI/styles.css:11-53` bone, `:55-96` soot). The scene uses CSS variables inside the
SVG (`stroke="var(--fg-accent)"`) and never a literal hex in a component; the canvas layer reads the same variables once per
theme change (§5.6). The mockups are the proof: the dark file uses exactly eleven hex values, every one a `.dark` token,
and the light file the same eleven roles from `:root`.

| Role | Token | Soot (`.dark`) | Bone (`:root`) | Where it appears |
|---|---|---|---|---|
| **needs you** — the only accent on screen | `--fg-accent` | gold-ochre `#c47a2c` (`:68`) | ochre `#b4531e` (`:22`) | NEEDS YOU table + count, cascade dots and their lines, waiting ring, `blocked` outcomes, the `N NEED YOU` footer segment, the gold bar by the clock, the wordmark |
| done / landed / healthy | `--status-success-fg` | malachite-bright `#4a8a6e` (`:81`) | malachite `#2f5d4a` (`:35`) | outcome fan, done dots in AGENTS and OUTCOMES, spend bars under 85 % |
| in flight | `--status-info-fg` | faience-bright `#5b8f8a` (`:78`) | faience `#3d6b66` (`:32`) | live and idle rings, running flows, hollow in-progress dots, the routine ring |
| errors only | `--status-danger-fg` | carnelian-bright `#c45a48` (`:75`) | carnelian `#9b2d1f` (`:29`) | spend over 85 % of cap, unreachable/invalid agent ring, a failed routine result, the one-line query-error note |
| text, NOW marker, routine squares | `--fg-primary` | bone `#e8dcc8` (`:64`) | soot `#1a1612` (`:18`) | ring names, row text, the NOW line, filled routine squares, panel titles |
| metadata | `--fg-secondary` | bone-dim `#c9b89a` (`:65`) | ash `#6b5e4f` (`:19`) | the clock, stream rows of neutral kinds |
| dim axes and labels | `--fg-tertiary` | sand `#9a8b72` (`:66`) | dust `#8f7f6a` (`:20`) | arc ticks and hour labels, column heads, panel meta, lapsed rings, "Nothing needs you" |
| hairline rules | `--border-default` | basalt `#2a241c` (`:71`) | limestone `#d4c4a8` (`:25`) | table rules, panel strokes, frame corners |
| big dim numbers, tracks, empty waffle dots | `--border-strong` | char `#3a3228` (`:72`) | papyrus `#c9b48a` (`:26`) | the 18 px panel numbers, spend tracks, hollow waffle dots |
| panels | `--bg-elevated` | soot-2 `#1c1915` (`:63`) | bone-2 `#f3eadf` (`:17`) | the five panel fills |
| field | `--bg-page` | soot `#12100e` (`:59`) | bone `#e8dcc8` (`:13`) | the page; the fill inside needs-you dots |
| texture | `--fg-primary` at 7–26 % | — | — | the hairline field only |

Sky's ruling of 09:5x replaced carnelian with gold for the needs-you lines: the brand accent is the colour that calls you,
and carnelian is kept for the rarer, worse case. One consequence the mockup made visible: gold and faience are closer in
luminance than red and teal were, so §4.5 is load-bearing, not a nicety.

### 4.2 Mark styles

- **Rings** (r 13): `live` = stroke 1.6 in faience + inner disc r 5; `idle` = stroke 1.2, hollow; `waiting` = gold
  stroke 1.6 + gold disc; `lapsed` = dashed `3 3` in `--fg-tertiary`; `error` = dashed in carnelian. Name right-aligned at
  x−22, y−1 (11 px, `--fg-primary`, `--fg-tertiary` when lapsed); sub-label at y+12 (9 px, in the ring's colour).
- **Needs-you dots**: circle r 7, gold stroke 1.2, fill `--bg-page` (so lines passing behind do not show through), inner
  disc r 1.6; the short id to the left (x−13, y+4, 10 px, right-aligned).
- **Lines**: coloured flows stroke 1.1 at opacity 0.95, a 1.8 px disc at the end, label at (ex+9, ey+3.5) in the flow's
  colour (10 px); hairlines stroke 0.6, `--fg-primary` at a seeded opacity 0.07–0.26, no end disc, no label.
- **Squares**: routine marks 5×5 on the arc's outside — filled bone = fired today, hollow = the next fire later today;
  panel header squares 5×5 in `--fg-tertiary`.
- **Waffle**: 60 dots, 10 per row, 14 px pitch, filled in legend order (done malachite, running faience, review bone,
  blocked gold, cancelled `--fg-tertiary`), the remainder hollow in `--border-strong`.
- **Punch card**: radius 0.6 + 2.6·v and opacity 0.25 + 0.75·v where v = count / max of the week; a 6 px bone ring around
  the current cell.
- **Dot strips** (AGENTS): one 2.6 px dot per activity entry today by that actor, filled malachite for `task_done` /
  `child_returned`, hollow faience for `task_checked_out` / `child_spawned`, gold for `escalation_raised`; a lapsed ring
  shows one dashed dot.

### 4.3 Type scale

All labels monospace (Tailwind `font-mono`, as the mockups: `ui-monospace, 'JetBrains Mono', Menlo, monospace`). Sizes are
SVG user units in the 1920×1080 viewBox, so they scale with the container.

| Size | Tracking | Used for |
|---|---|---|
| 18 px | — | the big dim number bottom-right of each panel (`--border-strong`) |
| 11 px | — | NEEDS YOU rows (gold), ring names |
| 10 px | 2 px | the wordmark `T H E U T H` (gold) |
| 10 px | — | needs-you dot labels, flow end labels, AGENTS row names |
| 9 px | 1.5 px | panel titles, NEEDS YOU header, footer segments, column heads |
| 9 px | — | ring sub-labels, the NOW label, STREAM times, the clock |
| 8 px | — | arc hour labels (every 2 h), panel meta (right-aligned) |
| 7 px | — | routine names along the arc |

### 4.4 The quiet-day rule

A quiet day must look calm, not broken. With no attention items the NEEDS YOU panel says "Nothing needs you" in
`--fg-tertiary` and nothing on the screen is gold except the wordmark; with no agents working every ring is hollow; with
little activity the field thins to a few hairlines; the footer still reads (`0 NEED YOU`, `$X THIS MONTH`, the clock). No
empty-state illustration, no "get started" copy — the page's lists below already carry their own empty notes
(`UI/components/hud/Hud.tsx:92`, `:98`, `:367`, `:428`).

### 4.5 Colour is never the only channel

Every state has a second carrier: ring style (filled / hollow / dashed), dot or square fill (filled / hollow), a state
word in the label (`· running · 12m`, `· done`, `paused`, `unreachable`), the legend words beside the waffle, and the
`<title>` on every interactive mark. A monochrome print of the scene still reads.

## 5. Data contract

UI-only. Every value the scene draws comes from six RPCs the HUD or its neighbours already call; no engine or RPC change.
Everything is computed by pure functions of `(data, now)` in `UI/components/hud/scene/model.ts` so the whole picture is
testable without React or a DOM (today's `model.ts` is the pattern: `UI/components/hud/model.ts:12-86`, tests
`UI/components/hud/model.test.ts:48-122`). In §5.1 a bare `<Name>.ts` is `UI/rpc/bindings/<Name>.ts`.

### 5.1 RPCs, hooks and fields

| RPC | Hook | Fields used | Windows / caps |
|---|---|---|---|
| `attention_list()` (`UI/rpc/api.ts:540`) | `useAttention()` (`UI/components/hud/queries.ts:27-43`; re-derived on 13 events `:24`, 60 s poll `:42`) | `AttentionItem` `id, kind, title, at_ms, task, agent` (`UI/rpc/bindings/AttentionItem.ts:9-41`) | newest first; 12 drawn + `more`; 6 table rows + `+N` |
| `agents_list()` (`UI/rpc/api.ts:508`) | `useAgents()` (`UI/components/tasks/queries.ts:55-58`, key `['roster']` `UI/lib/rpc.ts:21`) | `AgentSummary` `file.name, file.budget, file.paused, status, desk, open_tasks, spent_month` (`AgentSummary.ts:8-32`; `AgentStatus.ts:6`; `Budget.ts:6-14`) | one ring per agent that is not `proposed`; 9 rings |
| `tasks_list(filter)` (`UI/rpc/api.ts:522`) | `useTasks()` (`UI/components/tasks/queries.ts:33-39`, key `['tasks','list',f]` `:18`) | `Task` `id, title, owner, state, checkout, updated_ms` (`Task.ts:11-31`, `:68-71`, `:84-87`); `Checkout.progress_at_ms` (`Checkout.ts:19-22`) | `doing` → running flows (8); `updated_ms ≥ midnight` → outcome mix |
| `activity_list(filter)` (`UI/rpc/api.ts:537-538`) | existing `useActivity()` (last 50, live-prepended: `UI/components/hud/queries.ts:80-95`), **new** `useActivityToday()` (`since_ms` = local midnight, `limit` 500) and `useActivityWeek()` (`since_ms` = now − 7 d, `limit` 500) beside it | `ActivityEntry` `id, ts_ms, kind, actor, subject, summary, cost` (`ActivityEntry.ts:7-35`); `ActivityFilter` (`ActivityFilter.ts:7-19`, max 500 `:17-19`) | today → outcomes, hairlines, AGENTS strips; week → ARRIVALS |
| `list_schedules()` (`UI/rpc/api.ts:454`) | `useRoutines()` (`UI/components/routines/queries.ts:23-38`) | `ScheduleInfo` `name, next, last, last_result, session, running` (`ScheduleInfo.ts:9-48`) | running → rings; fired today → filled squares; `next` later today → hollow squares |
| `list_sessions()` (`UI/rpc/api.ts:422`) | the shared `['sessions']` query (`UI/lib/rpc.ts:19`; `useSessionList()` `UI/components/attention/source.tsx:17-21` is the hook to reuse, not a second fetch) | `SessionSummary` `id, updated_at, parent, desk, archived` (`SessionSummary.ts:17-20`, `:34-37`, `:62-66`, `:71-75`) | touched today → the theuth ring and hairlines; last 15 min → `N SESSIONS LIVE` |

`today` = local midnight → `now`; `week` = `now − 7 d → now`. The activity poll stays as it is: `useActivity` already
prepends `ActivityAppended` into its cache and marks task/roster lists stale (`UI/components/hud/queries.ts:83-93`); the two new hooks are
plain queries with the same key prefix and refetch when the 50-row feed changes.

### 5.2 Caps

| Class | Cap | What the cap cuts goes where |
|---|---|---|
| needs-you dots | 12 | `more` count beside the table's `+N`; the table itself shows 6 rows then `+N` |
| rings | 9 | one extra ring `+N more` (idle, dashed) — never dropped silently |
| outcomes (malachite) | 16 | `more` count; oldest beyond the cap are the ones dropped |
| running flows (faience) | 8 | `more` count |
| hairlines | 600 | the rest not drawn; the field is texture, so no count |
| activity today / week | 500 each | the engine's `activity_list` maximum (`UI/rpc/api.ts:537`) |

### 5.3 Rings — `rings(agents, sessions, routines, attention, now) → Ring[]`

In this order, then capped at 9:

1. **`theuth`** — every top-level, non-desk, non-archived session touched today (`parent === null`, `desk` unset,
   `!archived`, `updated_at ≥ midnight`). Sub-label `N chats · <age of newest>`. Style `live` if any was updated within
   2 min, else `idle`. There is no global "a turn is streaming" flag on the summary that the HUD can rely on across
   engines; recency is the proxy, and the code says so in a comment (open item §10.5).
2. **One ring per roster agent** that is not `proposed`. Label `file.name`; sub-label `open_tasks · <age of the desk's
   last activity>`. Style from `AgentStatus` (`AgentStatus.ts:6`): `working` → live; `idle` → idle; `blocked` → waiting
   (gold) when an attention item names the agent (`item.agent === name`, `AttentionItem.ts:34-37`), else idle;
   `paused` → lapsed (dashed, sub-label `paused`); `unreachable` / `invalid` → error (dashed carnelian, sub-label the
   status word). Order: working first, then idle, waiting next to working, lapsed and error last.
3. **Running routines** (`ScheduleInfo.running`, `ScheduleInfo.ts:45-48`) → ring `routine · <name>`, idle style, sub-label
   `running · <elapsed since last>`.

### 5.4 Flows

- **`needsYou(attention, now) → Need[]`** — newest first by `at_ms`, 12 drawn. `shortId(item)` (≤ 10 chars) reads the id
  grammar documented on the binding (`AttentionItem.ts:10-13`): `approval:<id>` → `approval`, `decision:<task>:<d>` →
  `decision`, `blocked:<task>` → the task's short id (`shortId` from `UI/components/tasks/helpers.ts:111` is the six-char
  form the chips use). `via` = the ring whose name is `item.agent`, else none. Flow time = `at_ms` clamped to today's
  window — an older item sits at the arc's 00:00 end, and its long line is the point. End label `<shortId> · <age>` with
  `age()` as the Attention list uses it (`helpers.ts:99-108`). Link `/attention/$id`, the same target as today's card title
  (`UI/components/hud/Hud.tsx:233-236`).
- **`outcomes(activity_today, tasks, rings) → Flow[]`** — malachite, newest first, 16 drawn. Kinds (`ActivityKind.ts:6`):
  `task_done`, `child_returned`, `escalation_answered`, and `routine_fired` whose schedule's `last_result` is not an
  error. `via` = the actor's ring (`agent:<name>`), the theuth ring for `user`, none for `system` (`ActivityEntry.ts:20-23`).
  Label = `summary` ≤ 28 chars, plus ` · $<cost>` when `cost` is non-null (`:32-35`; 70 noted no feed sets it yet, §10.4).
  Link `/tasks/$id` when the subject is a task, `/routines/$name` for `routine_fired`, else none.
- **`running(tasks, rings, now) → Flow[]`** — faience, 8 drawn: tasks in `doing`, via the owner's ring, time =
  `checkout.progress_at_ms ?? updated_ms` — the same clock `inFlight` already uses (`UI/components/hud/model.ts:45`) —
  label `<title ≤ 24> · running · <elapsed>`, link `/tasks/$id`.
- **`layoutEnds(flows)`** — sort by time ascending, spread end y evenly over 60–775 with ≥ 22 px spacing; when there are
  more flows than slots the oldest beyond the cap are dropped and counted in `more`; end x cycles 1500, 1580, 1660, 1740
  so labels stagger. Test: no two labels in the same x column closer than 22 px in y.

### 5.5 Panels

- **`punchCard(activity_week, now) → number[7][24]`** — counts by local weekday (Mon = 0) × hour; the cell for `now` is
  marked.
- **`outcomeMix(tasks, now) → { done, running, review, blocked, cancelled }`** — over tasks with `updated_ms ≥ midnight`;
  `running` = `state === 'doing'` regardless of date (`TaskState.ts:6`).
- **`footer(…)`** — `N SESSIONS LIVE` (sessions touched in the last 15 min), `N AGENTS WORKING` (`status === 'working'`),
  `N NEED YOU` (gold when > 0), `$X THIS MONTH` (sum of `spent_month`, as `headline` does, `model.ts:16`), and on the right
  `N OUTCOMES TODAY · N IN FLIGHT`.
- SPEND keeps `spend()` from `../model.ts` (`model.ts:60-76`; share = spent / cap at `:71`) — bars go carnelian at
  share > 0.85, a tighter line than the list's 0.8 warning (`Hud.tsx:439`), because here carnelian means "broken", not
  "watch". STREAM keeps `appendActivity` (`model.ts:28-31`) and shows the last 7 of the 50 live rows.

### 5.6 Hairlines — `hairlines(activity_today, sessions_today, drawnIds, seed) → Hairline[]`

One per activity entry today that is **not** already a coloured flow, plus one per session touched today (children
included), capped at 600. Source: 62 % from a ring (the actor's ring when known, else a seeded choice) at
(874, ring.y ± 3), 38 % from a seeded left point (x 180–640, y 250–690); time = `ts`; end seeded in x 1380–1900,
y 60–775; opacity seeded 0.07–0.26. "Seeded" means a tiny deterministic PRNG (mulberry32 or xorshift over a string hash of
the id), so the picture is identical between renders and between the two themes — the same rule 123 §5.3 (9) sets for
diagrams ("same props → same SVG", `docs/design/123-diagram-grammars.md:339`). Tests assert determinism and the cap.

The field is drawn on a `<canvas>` under the SVG (§6.1), redrawn on data change and at most once per second, with colours
read from `getComputedStyle(document.documentElement)` once per theme change — the way `diagramTokens()` already reads the
palette (`UI/lib/diagram-theme.ts:21-36`), subscribed through the `.dark` class observer `subscribeDark`
(`UI/lib/theme.ts:31-36`).

## 6. Layout

### 6.1 Desktop — `Scene` (≥ md, 768 px: `useIsMobile()` is the one breakpoint the app has, `UI/hooks/use-mobile.ts:3-5`, `:21-23`)

An `<svg viewBox="0 0 1920 1080" width="100%">` scaled to the container with the aspect locked (minimum rendered height
540 px), over an absolutely positioned `<canvas>` in the same coordinate space. Layers bottom-up: (a) the canvas hairline
field; (b) the arc band and ticks; (c) coloured flows; (d) needs-you dots; (e) rings; (f) text. The geometry below is the
one source for every number; a future change edits this table and the mockups together.

| Element | Geometry (viewBox units) |
|---|---|
| Top bar | wordmark `T H E U T H` at (24, 18); clock `● HH:MM:SS` right-aligned at (1896, 18) in `--fg-secondary`; a 3×10 gold bar at (1852, 9) when needs-you > 0 |
| Frame | 14 px L-shaped corner marks at the four corners of the flow field, y 40 and 772 |
| NEEDS YOU table | origin (28, 60); header `NEEDS YOU` + count right-aligned at x 430 (gold); rule at y 66; column heads `ID KIND AGE TITLE` at y 82 (9 px, `--fg-tertiary`); rows every 17 px from y 100 (11 px gold): shortId · kind · age · title ≤ 38 chars + `…`; 6 rows then `+N` |
| Needs-you dots | dot i at x = 610 − 60·i, y = 262 + 52·i + jitter(−6…6, seeded by item id); label at (x−13, y+4) right-aligned |
| Rings | x = 860, y = 265 + 53·i, r = 13; name at (x−22, y−1); sub-label at (x, y+12); flows leave at x+14 and arrive at x−14 |
| Arc (today's clock) | centre (−300, 420), R = 1390; θ(hour) = −14° + 28°·hour/24 (hour = local hours since midnight, fractional); `arcPoint(hour, r = R) = (cx + r·cos θ, cy + r·sin θ)`; band R ± 18 (outer arc sweep 1, inner sweep 0) |
| Hour ticks and labels | ticks every hour from R−18 to R−12 (`--fg-tertiary`); labels every 2 h at `arcPoint(h + 0.55, R − 4)`, 8 px, `text-anchor="middle"`, `transform="rotate(θ+90 x y)"` |
| Routine marks | filled 5×5 squares (bone) at `arcPoint(hour, R + 9)` rotated θ for each `routine_fired` today; hollow squares for a `next` later today; name at `arcPoint(hour + 1.1, R + 10)`, 7 px, rotated θ+90 |
| NOW marker | a bone line from R−22 to R+22 at θ(now), stroke 1.5; label `now` at `arcPoint(now + 0.9, R + 30)`, 9 px, rotated |
| `flowPath(src, via, hour, end)` | src → ring edge (via.x − 14) as a cubic with horizontal tangents (control x at 50 % of the span); (via.x + 14) → `arcPoint(hour)` as a second horizontal-tangent cubic; then a quadratic to `end` with the control at (ax + 0.42·(ex − ax), ay + 0.30·(ey − ay)). Without a via the first cubic goes straight to the arc |
| End layout | y spread evenly over 60–775 (≥ 22 px apart) by time ascending; x cycles 1500 / 1580 / 1660 / 1740; disc 1.8 px; label at (ex+9, ey+3.5) |
| Hairlines | 62 % from (874, ring.y ± 3), 38 % from a seeded point in x 180–640 × y 250–690; through `arcPoint(ts)`; end in x 1380–1900 × y 60–775 |
| Footer | rule at y 778; five segments at y 795 (9 px, tracking 1.5) at x 28, 190, 356, 470 and right-aligned at 1896 |
| Panel grid | y 812–1060; five panels 352×248 at x 24, 404, 784, 1164, 1544; fill `--bg-elevated`, stroke `--border-default` |
| Panel chrome | 5×5 square at (+12, +13); title at (+24, +19) 9 px tracked; meta right-aligned at (+340, +19) 8 px `--fg-tertiary`; rule at +28; big dim number bottom-right, 18 px `--border-strong`, baseline +236 |

Why these shapes: the arc is a circle of radius 1390 whose centre sits 300 px off the left edge, so 24 hours span only
28° — the band reads as a gently curved ruler, not a dial, and a line's crossing point is legible to the quarter hour
without a label. The horizontal-tangent cubics are the Sankey ribbon shape (`sankeyLinkHorizontal`,
`UI/components/chat/surface/diagrams/Sankey.tsx:104`) drawn as a 1.1 px stroke instead of a filled ribbon: a flow here is
one item, so width would encode nothing. The quadratic after the arc bends every line towards its label with the same
curvature, which is what makes the right side read as a fan rather than a tangle.

### 6.2 The scene's place in `Hud.tsx`

`Scene` (or `SceneCompact`) replaces the `grid grid-cols-2 … sm:grid-cols-4` of four `Metric` cards
(`UI/components/hud/Hud.tsx:64-69`). The `Overview` header with the sidebar toggle stays (`:48-61`); everything below the
scene stays, unchanged, in the same order (`:71-123`); the `Composer` stays docked (`:126-128`). Data: the hooks `Hud`
already calls (`:36-41`) plus `useActivityToday()`, `useActivityWeek()` and the shared sessions query (§5.1). Loading: while
queries are pending the scene renders its chrome — bar, arc band, panel frames — with no marks. Errors: one line in
`--status-danger-fg` under the footer naming the failed query; the sections below keep their own per-section error text
(`:160`). The `Metric` component itself is deleted with its grid; `headline()` stays in `model.ts` (the footer reuses its
sums), and `Section`, `KIND_LABEL`, `ATTENTION_EMPTY`, `AttentionList` and `KindActions` keep their exports.

### 6.3 The five panels (bottom band, left to right)

- **AGENTS** (`N LIVE`): one row per ring (y +48 + 32·i, max 6): name 10 px, meta right; under it the activity dot strip
  (§4.2). Big number: rings that are live.
- **ARRIVALS** (`UTC±h`): the punch card — rows `m t w t f s s` at y +52 + 24·d, 24 dots per row at x +34 + 13.2·h; hour
  axis `00 06 12 18` at y +226; a 6 px bone ring around the current cell. Big number: the current `HH:MM`.
- **OUTCOMES** (`OF N`): the waffle at (+20, +50), 14 px pitch; legend at x +186 (dot) / +196 (word) with counts
  right-aligned at +338. Big number: outcomes today.
- **SPEND** (`MONTH`): rows y +50 + 32·i (max 5) from `spend()`: name, `$spent / $cap` or `· no cap`; a 2 px track
  (`--border-strong`) at y+9 from +14 to +338, fill malachite, carnelian when share > 0.85. Big number: `$total`.
- **STREAM** (`LIVE`): the last 7 activity rows at y +50 + 20·i: time 9 px `--fg-tertiary` (+14), actor (+76), summary
  (+160, ≤ 30 chars) coloured by kind — done kinds malachite, `escalation_raised` gold, checked-out / spawned / routine
  faience, else `--fg-secondary`. Big number: `N/50` rows loaded.

### 6.4 Compact — `SceneCompact` (< md, phones)

No flow and no arc: both need width, and a 1920-unit viewBox squeezed to 390 px makes 8 px type unreadable. Stack, top to
bottom: the footer segments as a wrapping row of pills; the NEEDS YOU rows (gold, full width, tap → `/attention/$id`); the
rings as a vertical list (ring glyph + name + sub-label, tap → the ring's link); then the five panels at container width in
the order AGENTS, OUTCOMES, SPEND, ARRIVALS, STREAM, each an SVG with `viewBox="0 0 352 248"` scaled to the width. The
glance order (§3.1) survives: gold first, then who, then what landed; only "when" loses its arc and keeps ARRIVALS and
STREAM.

### 6.5 Links

Every mark that stands for a record links to it, with the routes `Hud.tsx` already uses: needs-you dots, table rows and
end labels → `/attention/$id` (`UI/components/hud/Hud.tsx:234`); outcome ends → `/tasks/$id` (`:375`, `:414`) or
`/routines/$name` (`:391`); rings → `/agents/$name/$tab` (`:433`), a routine ring → `/routines/$name`, the theuth ring → `/c/$session` of the
newest chat (`:379`); footer segments → `/agents`, `/tasks?state=doing`, `/attention`, `/agents` (spend) as the `Metric` cards did
(`:65-68`). A link is a TanStack `Link` wrapping the SVG element (or an `<a role="link">` inside a `<g>`), keyboard-focusable,
with a `<title>` carrying the full text.

## 7. Motion and accessibility

### 7.1 Motion — three movements, nothing else

- A **live ring's inner disc pulses**: scale 1 → 1.25, 1.6 s, ease-in-out, infinite. It is the one continuous motion on
  the screen and it means exactly "streaming now".
- A **newly arrived coloured flow fades in** over 600 ms (a `stroke-opacity` transition keyed on the flow id), so an
  outcome landing while you watch is noticed without a flash.
- The **NOW marker moves once a minute**; the clock in the top bar ticks each second and stops when the tab is hidden
  (`document.visibilityState`).

All three are CSS keyframes in `styles.css`, following the file's own pattern — a class animates, and a
`@media (prefers-reduced-motion: reduce)` block sets `animation: none` for it (`UI/styles.css:233-254` for `pty-live`,
`:262-284` for `.shimmer`, `:417-431` for the talk orb). Under reduced motion: no pulse, no fade (the flow appears), the
NOW marker still steps (a position change, not an animation). No motion library (the brief's rule): the three motions
are CSS keyframes, like the PTY icon's, the shimmer's and the talk orb's. Hover raises one flow to opacity 1 and the others to 0.5 through CSS on a `data-flow`
attribute — no React state per mouse move. Nothing animates on first paint, nothing animates while idle
(73 §6.5, `docs/design/73-codemap.md:391`).

### 7.2 Accessibility

- The scene SVG has `role="img"` with `<title>` and `<desc>` as its first two children, the desc summarising the footer
  numbers ("6 need you · 5 agents working · 54 outcomes today · 7 in flight · $31.20 this month") — the kernel's `svgA11y`
  shape (`UI/lib/diagram/README.md:56-59`), so a screen reader gets the glance in one sentence.
- Every interactive mark is a focusable link in reading order (needs-you, rings, outcomes, panels) with a `<title>` giving
  the full text the mark abbreviates; the 123 unit contract's hit-area rule (≥ 18 px, `docs/design/123-diagram-grammars.md:331`)
  applies to dots and ring glyphs — a transparent padded circle takes the pointer.
- Colour contrast: text tiers are the theme's own (`--fg-primary` on `--bg-page`); the dim axes in `--fg-tertiary` are
  labels of marks that also carry their value in a `<title>`, never the only copy of a fact.
- The hairline canvas is `aria-hidden` and `pointer-events: none`; it is texture, not content (§3.3 (7)).
- Theming under high-contrast and forced-colours modes is an open item (§10.6): the SVG follows the tokens, the canvas
  reads them once and would need a `forced-colors` media listener to follow a system override.

## 8. Implementation map

### 8.1 Files

| Path (`UI/components/hud/` unless noted) | New / changed | What |
|---|---|---|
| `scene/model.ts` | new | the pure functions of §5: `rings`, `needsYou`, `outcomes`, `running`, `hairlines`, `arcPoint`, `flowPath`, `layoutEnds`, `punchCard`, `outcomeMix`, `footer`, `shortId`, the seeded PRNG |
| `scene/model.test.ts` | new | determinism and caps of `hairlines`; ring style for every `AgentStatus`; `shortId` over the three id grammars; `layoutEnds` 22 px rule; `punchCard` and `outcomeMix` over the fixture |
| `scene/Scene.tsx` | new | the desktop SVG + canvas of §6.1–§6.3, links of §6.5, tooltips, the clock, the theme observer |
| `scene/SceneCompact.tsx` | new | the phone stack of §6.4; each panel is the same panel SVG at `viewBox="0 0 352 248"` |
| `scene/fixtures.ts` | new | synthetic data that reproduces the mockup: 6 needs-you items, 7 rings, 8 done, 3 running, ~230 field lines from the seed, punch card, spend rows, stream |
| `scene/Scene.test.ts` | new | render test over the fixture with the repo's `renderToStaticMarkup` + mocked `Link` pattern (`UI/components/hud/Hud.test.ts:10-32`): element counts, colours as `var(--…)` strings, link hrefs, no animation class under reduced motion |
| `queries.ts` | changed | `useActivityToday()` and `useActivityWeek()` beside `useActivity` (`UI/components/hud/queries.ts:80-95`); the `sim` switch serves the fixture under the existing `?sim=` gate (`simFlag`, `UI/components/settings/sandbox-queries.ts:24-28`; `ATTENTION_SIM` `:190`) — no new URL flag |
| `Hud.tsx` | changed | the `Metric` grid (`UI/components/hud/Hud.tsx:64-69`) and the `Metric` component (`:133-144`) go; `Scene` / `SceneCompact` chosen by `useIsMobile()` take their place; three hooks added beside `:36-41`; every export kept |
| `UI/styles.css` | changed | keyframes for the ring pulse and the flow fade-in, each with its `prefers-reduced-motion` block, in the style of `UI/styles.css:233-254` |
| `docs/design/128-as-built-HUD-SCENE.md` | new | the worker's report: per-file change, deviations, not built, tests, gates and results |

Not touched: `components/attention/*`, the sidebar, `SectionBand`, routes other than reading `routes/index.tsx`, every
`.rs` file, `package.json` (no new dependency: the field is canvas 2D, the flows are hand-written path strings, the
seeding is ~15 lines).

### 8.2 Tests and gates

- `bunx vitest run src/components/hud` — the two new test files plus today's `model.test.ts` and `Hud.test.ts`
  (`UI/components/hud/model.test.ts:48-122`, `UI/components/hud/Hud.test.ts:34-46`) still green.
- `bun run check` (tsc) and `bunx vite build`, from `crates/desktop/ui`.
- Visual check: the dev UI under `?sim=` renders the fixture; a screenshot in each theme beside the mockups is the
  acceptance picture the as-built attaches.

### 8.3 Machine rules (one shared MacBook, several sessions)

Every gate runs under the shared semaphore — `BUILD_LABEL=128-hud ~/.theuth/bin/build-lock <cmd>` — in the foreground; if
the lock waits, it waits. No cargo, no Tauri CLI, no engine change, no `bun install` unless `node_modules` is missing (then
once). The orchestrator pre-copied the gitignored `UI/rpc/bindings` into the worktree; it is never regenerated by the worker.
Commit after each numbered step of the brief, so a reboot loses nothing; the final message is `DONE HUD-SCENE <sha>`.

## 9. Decisions and alternatives declined

Each decision names the alternative that was declined and why. D1–D7 are the orchestrator's calls for Sky, made from his
four lines (§1.1); D8–D12 are the brief's calls that follow from them.

| # | Decision | Declined | Why |
|---|---|---|---|
| D1 | **Vocabulary: FUI applied to real data** — alluvial flow through owner nodes into a radial timeline with an edge fan, plus Tufte small multiples; principles: preattentive colour, overview-first, calm technology (§2) | copying the reference's panel set; a generic "dashboard" vocabulary | two of the reference's panels decorate; the names in §2.1 are what lets a later change ask "what does this mark decode to" |
| D2 | **Refuse the dotted sphere and the isometric catalog** (§2.3) | keeping them "for beauty" | every coloured mark must decode to a fact; grey may be texture — beauty here is the restraint |
| D3 | **Colour semantics on theuth tokens, gold-ochre `--fg-accent` = needs you** (§4.1) | carnelian for needs-you (the first mockup); a palette of the scene's own | Sky's rulings of 09:45 and 09:5x verbatim; one token table serves both themes |
| D4 | **Spine: needs-you → agents/rings → today's clock → outcomes** (§3.1) | a sessions-centric spine (one ring per session); a tasks-centric spine (one lane per task) | the four glance questions are about people and agents, not records; sessions and tasks are the lists below |
| D5 | **Live hairline field on a canvas 2D layer, ≤ 600 lines, ≤ 1 Hz, deterministic seeding** (§5.6) | WebGPU — the orbs renderer exists (`UI/components/orbs/renderer.ts:1-3`, `UI/components/orbs/canvas.tsx:13`) and is a later option (§10.2); a static texture; no texture | 600 strokes once a second is nothing for canvas 2D and needs no GPU path or fallback; a static texture lies about the day; no texture loses the pulse (§3.2) |
| D6 | **The picture is the map; Attention stays the controls** (§3.3 (1)) | actions on the marks (approve from a dot); replacing the lists | `KindActions` and the attention screens are the decision surface (101/104); a glance surface that can also mutate state needs confirmations it should not have |
| D7 | **No settings toggle** | "classic cards" switch | two layouts to maintain and test; the lists below are the fallback for anyone who wants text |
| D8 | **Phones get `SceneCompact` without flow or arc** (§6.4) | the full scene scaled down; a portrait flow layout | 8 px type at 390 px is unreadable; a portrait flow is an open item (§10.3), not a v1 guess |
| D9 | **UI-only wave; no RPC change** | a `turn_pulse` feed for per-tool-call hairlines | the six RPCs already carry the four questions; the finer feed is the first open item (§10.1) |
| D10 | **Geometry fixed in one table** (§6.1) and taken from the mockups | geometry in code comments only | a future change has one source and two pictures to check against |
| D11 | **Recency as the streaming proxy for the theuth ring** (2 min) | a per-session `running` scan across engines | `SessionSummary.running` is the local supervisor's view (`UI/rpc/bindings/SessionSummary.ts:85-89`); a proxy that is honest in a comment beats a flag that is right on one engine (§10.5) |
| D12 | **Carnelian spend threshold at 85 %** (§5.5) | the list's 80 % warning tier (`UI/components/hud/Hud.tsx:439`) | on the scene carnelian means broken; 80 % is "watch", which the list below already says in its own warning colour |
| D13 | **Hand-written flow paths, not `d3-sankey`** (§6.1) | reusing `Sankey.tsx`'s layout (`UI/components/chat/surface/diagrams/Sankey.tsx:65-103`) | d3-sankey computes node heights from weights and columns from stages; here every flow is one item, nodes are fixed, and the third segment (arc → fan) is not a Sankey link at all |

## 10. Open items

1. **Turn-pulse feed.** Per-tool-call hairlines ("every turn of the day", as the mockup's 230 lines suggest) need an engine
   event or a `turn_pulse` RPC the UI does not have; v1 draws one line per activity row and per session touched, which
   under-counts a busy agent. Engine-side; a later wave.
2. **WebGPU field.** If the field ever wants thousands of lines or per-frame motion, the orbs renderer
   (`UI/components/orbs/renderer.ts`, typegpu/vgpu) is the path; it needs a WebGPU-absent fallback that canvas 2D does not.
3. **A flow layout for tablets in portrait.** Between 768 and ~1000 px the desktop scene is legible but tight; a two-row
   variant (flow above, panels in a 2+3 grid) is unspecified.
4. **Per-child cost on outcomes.** 70 carried `cost` in the schema but no feed sets it (`docs/design/70-as-built-HUD-ATTENTION.md:45`);
   the ` · $<cost>` suffix on an outcome label is dormant until it does.
5. **A streaming flag engine-side.** The theuth ring's `live` is a 2-minute recency proxy (D11); a mesh-wide "a turn is in
   flight on this root" field on `SessionSummary` would make it exact.
6. **Theming the canvas under high-contrast and forced-colours modes.** The SVG follows the tokens; the canvas snapshot
   needs a `forced-colors` / `prefers-contrast` listener and a decision on whether texture should vanish there.
7. **Right-edge label collisions at scale.** `layoutEnds` guarantees 22 px in y per x column; with more than ~32 labelled
   ends (16 + 8 + 12 is possible) the fan will cap and count. Whether the cap should prefer needs-you over outcomes is
   not decided.

## 11. References

### Prior theuth documents

- `docs/design/70-as-built-HUD-ATTENTION.md` — today's `/`: the HUD, the attention inbox and badge, the activity feed (`docs/design/70-as-built-HUD-ATTENTION.md:30-33`); `cost` unset (`:45`).
- `docs/design/71-whiteboard-visuals.md` — the cognitive-load lens and the reader-mechanisms-first shape (`docs/design/71-whiteboard-visuals.md:1-7`, `:22-24`).
- `docs/design/73-codemap.md` — node caps, "never drawn", the refusal of draw-everything visualisers (`docs/design/73-codemap.md:7`, `:30`, `:79-80`, `:391`).
- `docs/design/97-as-built-SIDEBAR-SIMPLIFY.md` — the rail; the THEUTH row is `/` (`docs/design/97-as-built-SIDEBAR-SIMPLIFY.md:7`, `:41`).
- `docs/design/123-diagram-grammars.md` — the SVG kernel, the unit contract, determinism (`docs/design/123-diagram-grammars.md:307-320`, `:329-339`); the kernel's frozen exports in `UI/lib/diagram/README.md:1-6`.
- `docs/design/125-advisor.md:3-14` — the house shape for a design document's Status paragraph followed here.

### External works (named, not linked)

- Edward R. Tufte, *The Visual Display of Quantitative Information* (1983): small multiples, data-ink, chartjunk — the test applied to the sphere and the catalog (§2.3) and the reason the panels are five small pictures with one question each.
- Ben Shneiderman, "The Eyes Have It: A Task by Data Type Taxonomy for Information Visualizations" (IEEE Symposium on Visual Languages, 1996): the visual information-seeking mantra — overview first, zoom and filter, details on demand (§2.2 (2)).
- Mark Weiser and John Seely Brown, "Designing Calm Technology" (Xerox PARC, 1995/1996; also as "The Coming Age of Calm Technology"): periphery and centre — the quiet-day rule and the motion budget (§2.2 (3), §4.4, §7.1).
- Danny Holten, "Hierarchical Edge Bundles: Visualization of Adjacency Relations in Hierarchical Data" (IEEE TVCG 12(5), 2006): the bundled-then-fanned edge shape the right side borrows (§2.1, §6.1).
- Colin Ware, *Information Visualization: Perception for Design* (1st ed. 2000): preattentive processing — why one hue can mean one thing before reading (§2.2 (1)).
- The FUI lineage: Territory Studio (screen graphics for *Blade Runner 2049*, *The Martian*, *Ex Machina*) and GMUNK (Bradley G. Munkowitz; *TRON: Legacy*, *Oblivion*) — the aesthetic the reference belongs to (§2.1); techniques and look only, no assets.
- A 2020 controlled study on node-link readability cited by 73 (Yoghourdjian et al., arXiv:2008.07944, `docs/design/73-codemap.md:79`) is the evidence behind the caps in §5.2.

## Revision 1 — routine origins (Sky, 2026-10-07 14:37/14:40)

> 14:37 — "why aren't routines drawing lines to their starting points on the ruler?"
>
> 14:40 — "each routine should have its own origin. don't lump them all together."

**Cause.** `outcomes()` set `via: ringByActor(rings, e.actor)`; a `routine_fired` row's actor is `system`, so `via` and `src` were
both null and `flowPath` took its last branch — the line started AT the arc (`M ax ay`): a square on the ruler and a right-hand
segment, nothing leading in. `rings()` made a ring only for running schedules (`routine · <name>`, `actor: null`), so even those
were never the origin of their firings.

**Rules** (as built in `hud/scene/model.ts`; as-built: `docs/design/128-as-built-ROUTINE-ORIGINS.md`):

1. **One ring per routine with a presence today**, in a second group under the agent rings, one extra 20 px gap before the group,
   each linking to `/routines/$name`. Presence today = fired today (a `routine_fired` row in `activityToday`, subject = schedule
   name) OR running now (`ScheduleInfo.running`) OR due later today (`ScheduleInfo.next` within today's window). Order: running
   first, then fired today newest-first, then due soonest-first. Label `routine · <name>`. Styles keep the grammar: running → live
   (filled faience), sub `running · <elapsed since last>`; fired today and finished → hollow faience, sub `ran HH:MM`, or hollow
   carnelian (`failed`, not dashed) with sub `failed HH:MM` when the schedule's `last_result` is an error (`isError`); due later today →
   dashed `--fg-tertiary` ring, sub `next HH:MM`.
2. **Flows.** Every `routine_fired` outcome's `via` is ITS routine's ring (`ring:routine:<name>`), so the line runs ring → the square on
   the arc at the fire hour → the outcome label. A running routine also gets a faience in-flight line from its ring to the arc at its
   start time (`last`, else now) with label `running · <elapsed>` (in `running()`; counted in the footer's IN FLIGHT as before). A
   due-later routine gets a faint connector only: dashed (3 3) `--fg-tertiary` at opacity 0.5 from its ring to its hollow square, no
   right-hand segment, no label (a `kind: 'due'` flow that `layoutEnds` never sees).
3. **The connector ends exactly where the square sits**: the same `hourOf(ts)` / `arcPoint(·, R + 9)` as `arc()`'s marks (fired →
   `e.ts_ms`, due → `next`, both through `dueAt`). Marks are unchanged.
4. **Capacity.** The ring column holds `max(9, ⌊(hv − 150) / 53⌋)` rings (`RING.cap`): agent rings first (their `+N more` fold rule
   unchanged), then routine rings; routines beyond the capacity fold into ONE dashed `+N routines` ring → `/routines` — the shown ones
   are always distinct origins (Sky's rule). Rings stay centred vertically, the group gap included. Known ceiling: a full agent
   column (9 agent rings at hv 600) leaves no slot and the routine group is not drawn there.
5. `ringByActor` keeps working for agents; `ringByRoutine(rings, name)` added. Hairlines sourced from `routine_fired` rows start at
   that routine's ring too (the 62 % ring-sourced rule otherwise unchanged).
6. `SceneCompact`'s rings list shows the routine rings as well (same model). Scene: nothing else changes.
7. Fixtures: 2 fired-today routines (one with an error result), 1 running, 1 due later today; tests in `model.test.ts` and
   `Scene.test.ts` cover the ring set and order, `via.id === 'ring:routine:<name>'` and `d` starting at `M <via.x+14>` for every
   routine fire, the dashed label-less due connector landing on the square, the capacity fold at hv 600, and the IN FLIGHT count.

## Revision 2 — context gauge on the desk rings (Sky, 2026-10-07 15:05)

> "can we make the desk icons outer ring on the hud be a context window indicator of the desk session?"

**What.** Every agent ring (a desk) carries an outer gauge: a faint full track (r 17, `--border-default`, width 1) and a sweep from
12 o'clock clockwise over `fill` of the circle (width 2, round cap, 1.6 px dot at its end). The sweep is purely length-coded; its
colour is the ring's state ink — faience when the ring is live, the secondary ink otherwise — never gold (needs-you) and never
carnelian (errors). Routine, overflow and desk-less rings draw nothing. The ring's sub-label appends ` · ctx 62 %`; its tooltip
adds `context ≈ 118k of 200k (59 %)` (k = thousands, one decimal under 10k). The compact list shows the same sub-label text and no
gauge. Flow anchors stay at ±14; a line may cross the track. 53 − 2·17 = 19 px stays between neighbouring tracks.

**Data path** (no engine change). The desk session of agent `<name>` is the roster row's `desk` session id, else the sessions-list
row with `SessionSummary.desk === '<name>'`. `useDeskContexts(sessions)` (`hud/queries.ts`) runs `useQueries` over the desk sessions
(`['context', 'breakdown', id]`, `session_context_breakdown`, staleTime 20 s, refetchInterval 30 s) and invalidates a session's key on
its `TurnFinished`, `Compacted` and `ContextUsed` events; it folds each breakdown with `list_models` (`useModels`, `queryKeys.models`)
through the pure `contextFill` into `SceneData.contexts: Map<SessionId, { fill, used, window, live }>`. `rings()` attaches the entry of
the desk session as `Ring.ctx`. Everything works without the map (`ctx: null` everywhere).

**Fill.** `used = last_provider_context ?? total_estimate`; `window = SessionSummary.context_size ?? ModelInfo.context` for the
session's model (the harness's reported window first — the same precedence the chat's ContextMeter uses — the catalog otherwise);
`fill = clamp(used / window, 0, 1)`.

**No fake value.** A desk whose window is unknown (no `context_size` and a model missing from the catalog or with `context: 0`), or
whose breakdown has not arrived, has `ctx: null`: no track, no sweep, no `ctx` text. Absence means unknown.

`gaugePath(cx, cy, r, fill)`: `''` at 0; one arc with the large-arc flag set when `fill > 0.5`; at ≥ 1 a full circle as two half arcs.

## Revision 4 — root-session rings with subagent satellites (Sky, 2026-10-07 15:47/15:49)

> 15:47 — "why aren't regular active sessions showing up on that hud?"
>
> 15:49 — "only root sessions should show up btw with context rings, and maybe an indicator for their subagents."

(Revision 3 is the SPEND-LEDGER wave, documented separately.)

**Why they were missing.** Rings existed for agents (desks) and routines only; every user chat folded into the theuth ring's
`N chats` count, appeared as a hairline and was counted in SESSIONS LIVE — a running root session had no origin of its own.

**Rules.**

1. **Third ring group `sessions`**, under the routine group with the same 20 px group gap: one ring per ACTIVE ROOT session —
   `parent === null`, `kind !== 'advisor'`, no `desk`, not `archived`; active = `running`, OR `updated_at` within 30 min, OR any
   descendant (any depth, not archived) is running or updated within 30 min. Order: self-running first, then rings with running
   descendants, then `updated_at` newest-first. Capacity as before (agents → routines → sessions); the overflow folds into one dashed
   `+N sessions` ring (no link — there is no sessions-list route). Ring id `ring:session:<id>`, link → `/c/<id>`.
2. **Label** the session `title` truncated to 22 (else `name`, else the 8-char id). **Sub**: the pieces that apply, ` · `-joined —
   `<k> running` (running descendants), `<n> children` (live descendants), the age of `updated_at`, then the ctx suffix. **Style**
   the root's own state: `live` when `running`, else `idle`. **Satellites**: one dot (r 2.2) per live descendant on a circle of radius
   23 (outside the gauge track), 40° apart from 3 o'clock clockwise, filled faience when running, hollow secondary ink when idle,
   titled with the child's name/title; at most 8, then a 7 px `+N` at the ninth position. Nothing when there are no live children.
   A ring with satellites moves its labels from x − 22 to x − 30 so the 160°/200° dots clear the text.
3. **Context gauge on roots too**: `useDeskContexts` became `useSessionContexts(sessions)` over desk sessions AND active roots (≤ 24
   queries, same 20 s / 30 s, same `TurnFinished` / `Compacted` / `ContextUsed` invalidation, `enabled: !sim`); the map is keyed by
   session id, so a root's `ctx` is `contexts.get(root.id)`.
4. **Flows.** A needs-you item whose `AttentionItem.session` is a ringed root leaves that ring (session first, then the agent lookup
   for desks). A session with a ring starts its hairline there. A `running` root gets a faience in-flight line from its ring to the
   arc at `updated_at`, labelled `<title 28> · running`, counted in IN FLIGHT. Running children draw no lines — they are the
   satellites.
5. **Liveness proxy.** Wherever "updated within N min" stood for live (the theuth ring's 2 min, SESSIONS LIVE's 15 min), `running
   === true` comes first and the recency stays as the fallback; a desk session that is `running` makes its agent ring live.
6. **SceneCompact** lists the session rings with the same `k running · n children · age` sub-label; no satellites there.
7. **Fixtures**: four active roots (running + 3 live children of which 2 running; running alone; idle with one running child;
   idle-but-recent), one old root, one advisor, one archived chat — the last three never drawn; contexts for two roots.