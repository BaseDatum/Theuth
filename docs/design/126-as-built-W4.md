# 126 as-built — wave W4 (desktop UI)

Branch `feat/126-w4`, worktree `/Users/Sky.Moore/Develop/theuth-126-w4`, base = C's contract commit `5cdcbfde`. Everything under `crates/desktop/ui/`; no `.rs` touched, no cargo run. Design: `docs/design/126-double.md` §10 (desktop column) + Revision 1 (grace 30 s); contract: `docs/design/126-as-built-C.md` "TS contract (verbatim)".

## What was built, where

### The chip, the countdown, the frame → `crates/desktop/ui/src/components/chat/DoubleChip.tsx`
- Pure helpers (tested): `rungGlyph` :19 (○ ◐ ● ●●), `chipText` :26 ("double · Allow · 91 %"), `secondsLeft` :29, `countdownText` :36 ("double answers Allow in 28 s — tap to take over"), `preselect` :43 (L1+ pick from `DoublePredicted.index`, else the option whose label equals `choice` case-insensitively — the wire says `Allow`, the `Decision` is `allow`; never at L0, never for `none`), `disagrees` :52 (model's `recommended` ≠ double's `choice` at L1+), `takeOver` :56 (`double_take_over(id)`; `true` marks the card taken in the store, `false` toasts "double had already answered").
- `useCountdown` :69 — a plain `setInterval` at 1 s while the RFC 3339 instant lies ahead; no animation library.
- `useDoubleCard(ids)` :85 — prediction / answer / grace seconds / undo seconds / taken for a card that may be keyed by several ids (approval id, ask id, packet key).
- `DoubleChip` :105 — the chip is a `Popover` trigger; rung styles: L0 muted (`text-fg-tertiary`, no border), L1 outlined, L2 filled (`border-fg-accent bg-accent`). At L2 with a running grace the countdown text replaces the chip text (design §10). When the model's `recommended` disagrees a `model recommends <label>` badge sits in the chip. After `DoubleAnswered` the chip becomes "answered by double · <choice>" with an `undo · N s` button while `undo_until` lies ahead (undo = `double_take_over`, see Hand-offs). `stopPropagation` on the chip so a tap on it never counts as a take-over.
- `Explain` :153 — popover body: class key, rung + name, choice + p + version, every `probs` entry, the head's parts (oracle / memory / prior), `promote_hint`, the ≤ 3 `neighbours` (date via `formatAt`, what the owner chose, excerpt), and the "don't learn from this one" `Switch` → `double_forget({ target: 'examples', ids: [question_id] })` (see Hand-offs: the prediction carries no `examples.id`).
- `DoubleCard` :203 — the card frame: during an L2 grace the whole card is `role="button"`, `tabIndex=0`, `aria-label` = the countdown text, ring highlight, click / Enter / Space → `takeOver`; a click on a control (`button,a,input,textarea,select,[role=button],[role=dialog]`) is ignored by the frame so the card's own buttons stay live (design §9 L2: answering is not a take-over, the answer is).
- `DoubleUndo` :231 — the inbox row's / screen's `Undo · N s` button (shown only while the undo window is open and the card was not already taken over).

### The store → `crates/desktop/ui/src/store/double.ts`
- `doubleStore` (zustand `createStore`) keyed by `question_id`: `predicted`, `answered`, `takenOver`; subscribed at module load with `on('DoublePredicted')` / `on('DoubleAnswered')` from `store/sessions` (the live engine's frames). `useDouble(selector)` is a `useSyncExternalStore` whose server snapshot is the live state — zustand's own hook serves the *initial* state to `renderToStaticMarkup`, which is how the UI tests render.
- `usePrediction(...ids)` / `useAnswered(...ids)`: the first id the double knows.

### Wire → `crates/desktop/ui/src/rpc/api.ts`, `crates/desktop/ui/src/rpc/events.ts`, `crates/desktop/ui/src/lib/rpc.ts`
- `Api` gained the 13 `double_*` methods (api.ts:652-678, positional params in C's order) and `METHODS` the 13 names (api.ts:955-967, sorted). `WireEvent` gained `DoublePredicted | DoubleAnswered | DoubleTrainingState | DoubleVersionActivated` (events.ts:113-117).
- `queryKeys.doubleStatus / doubleClasses / doubleClass(key) / doubleSettings / doubleAll` (rpc.ts).

### Render sites (every decision card on this base)
| card | file:line | keyed by | L1 pre-select |
|---|---|---|---|
| `ApprovalCard` (chat transcript bottom via `Message.tsx:197 PromptCard`; `ApprovalsSheet`) | components/chat/ApprovalCard.tsx:56 frame, :61 chip | `approval.id` | the predicted button is the filled one (:68, :78) |
| `AskCard` | components/chat/AskCard.tsx:216 frame, :221 chip | `ask.id` | `withDoublePick` :37 via effect :194-196; the model's `recommended` badge stays, so both show |
| `RelayCard` (the decision/relay branch of `AskCard`) | AskCard.tsx:261 frame, :264 chip | `ask.id`, `packet.key`, `decision:<task>:<key>` | — (relay has no option buttons; `recommended` = the packet's backed option for the disagreement badge) |
| attention `ApprovalScreen` | components/attention/ApprovalScreen.tsx:43 chip | `item.approval` | filled Allow/Deny (:56-59) |
| attention `AskScreen` | components/attention/AskScreen.tsx:43 chip | `item.ask.id` | `withDoublePick` effect :25-28 |
| attention `DecisionScreen` (101/103 packet options) | components/attention/DecisionScreen.tsx:73 chip | `item.packet`, `item.decision`, `item.id` | `preselect` → `picked` while untouched (:40-45) |
| attention rail `Row` | components/attention/Row.tsx:25 | `doubleIds(item)` | — |
| HUD inbox `AttentionCard` | components/hud/Hud.tsx:235 | `doubleIds(item)` | — |
| draft cards | — | — | there is no task/agent/routine *draft card* surface on this base (`HireWizard` is a wizard, not a card raised to the owner); `DoubleChip ids={[draftId]}` drops in when one exists |

`doubleIds(item)` (Hud.tsx:250) = `[item.approval, item.ask?.id, item.packet, item.decision, <item.id minus "double:">]`.

### Attention inbox rows for `AttentionKind::Double`
- `KIND_LABEL.double = 'double'` (components/hud/Hud.tsx:184); `KindActions` renders `DoubleUndo` for `kind === 'double'` before the shared Snooze / Dismiss (Hud.tsx:269) — undo during the window only, Dismiss always (119's `attention_dismiss` by stable item id; the engine's versioned dismissal semantics are unchanged, this wave adds a kind).
- Rail group `Answered by double` after Mesh (components/attention/model.ts:22); `Screen.tsx:15,31` → `DoubleScreen` (components/attention/PlainScreens.tsx:188-205: title/detail large, "Double chose" + class from the `DoubleAnswered` the client saw, Undo / Snooze / Dismiss in the bar).
- The L3 daily digest is a `double` item without a matching `DoubleAnswered`: the same row and screen render it with no undo (test `attention/double.test.ts` covers `double:digest:<date>`); nothing shows unless `attention_list` carries items.
- `useAttention` invalidates on `DoubleAnswered` (components/hud/queries.ts:24) so the new row appears without the 60 s poll.

### Settings › Double → `crates/desktop/ui/src/components/settings/Double.tsx`, `SettingsDialog.tsx:58,96`
- Tab `['double', 'Double', UserRoundCheck]` after Labs (SettingsDialog.tsx:58); pane at :96.
- `Status` :165 — enabled dot, Pause learning (`double_pause`), active version (`baseline` until the first Tier A run), last run (`runLine` :103 = tier · venue · when · duration · result), next window + venue, the §6.6 approvals-off count.
- `Improvement` :187 — **design 123's `LineChartView`** (`components/chat/surface/diagrams/LineChart.tsx`, present on this base) with `chartProps` :82: labels = each week's Monday (`D Mon`, UTC getters), four series — accuracy, accuracy · baseline (muted), coverage @ 2 %, coverage · baseline (muted), unit `%`. Data: `mergeCurves` :56 — the n-weighted merge of every class's `double_class(key).curve` (the contract has no overall curve; see Hand-offs). Empty state text until a labelled week exists.
- `Kpis` :200 — Asks avoided · Minutes saved · Overrides (+ undo rate, n) from `double_status.kpis`.
- `Classes` :220 — table class · n · rung (glyph, `cap_reason` as tooltip) · acc · ECE · cap (`Select` L0–L3 → `setCap` :109 = `double_set_cap(key, rung, mutating_opt_in)`) · mutating opt-in (`Switch` → `setOptIn` :110 = `double_set_cap(key, cap, on)`) · last decision · actions: `examples` (expands `Examples` :314 — `double_class(key)`: `promote_hint`, last 50 rows with per-example forget) and `forget` (AlertDialog → `forgetClass` :111 = `double_forget({ target: 'class', key })`).
- `Training` :342 — Train now: Tier A (`double_train('a', {venue:'local'})`), Tier B local, Tier B on node (reachable `mesh_status` peers by name → `{venue:'node', name}`), Rebuild (tier C) disabled with a tooltip until W3; Roll back (version input → `double_rollback`); Window and venue inputs → `double_settings_set` (live, C's addendum); Capture toggles ×5 and Share dataset → `double_settings_set`; Export → `double_export` (toast with path and count); Delete all → `AlertDialog` (the Channels "Unlink" pattern) → `double_delete_all`.
- `RunLog` :444 — `double_status.last_run` plus this session's `DoubleTrainingState` events (newest first, ≤ 20). `on('DoubleTrainingState')` / `on('DoubleVersionActivated')` invalidate every `double` query; a version switch toasts.
- Every control is disabled while `enabled` is false (the stub state) except Pause/Delete/consents, which the engine itself refuses or accepts; a stub's `NOT_RUNNING` rejection surfaces as the `useAction` toast.

### Hand-written bindings (gitignored, NOT committed) → `crates/desktop/ui/src/rpc/bindings/`
Seeded from Sky's checkout (464 files, stale), then written verbatim from C's as-built: `DoublePredicted`, `DoubleAnswered`, `DoubleTrainingState`, `DoubleVersionActivated`, `Explain`, `Neighbour`, `Parts`, `CardKind`, `Tier`, `RunState`, `LabelSource`, `Outcome`, `RunId`, `Kpis`, `DoubleStatus`, `DoubleRun`, `DoubleClass`, `CurvePoint`, `DoubleExample`, `DoubleClassDetail`, `ForgetTarget`, `Venue`, `DoubleExport`, `DoubleConfig`, `Capture`, `Ladder`, `Caps`, `Training`, `Triggers`, `Tiers`, `TierA`, `TierB`, `TierC`, `Model`, `Answerer`, `Accelerator` (36 files). Also patched by hand to the Rust truth so the attention code type-checks: `AttentionKind.ts` (`ask | show | resume | double` — the seed lacked all four), `AttentionItem.ts` (`packet`, `ask?`, `turn?` from crates/roster/src/attention.rs:75-105), and `methods.json` (+13 `double_*` names, sorted). The integrator regenerates all of it with `just ts-desktop`.

### Tests
- `components/chat/DoubleChip.test.ts` (6): glyphs + chip text; countdown at 30 / 10 / 0 s and bad input; `preselect` rules; disagreement + `withDoublePick` (owner-touched draft left alone; two-question ask untouched); a static render of the chip at L1 (glyph, text, badge), at L2 (countdown replaces text), after `DoubleAnswered` (undo line); `takeOver` calls `double_take_over(question_id)` and marks the card only on `true`.
- `components/settings/Double.test.ts` (3): the empty stub state renders every section; a class row shows n / rung / acc / ECE / version / run line; row actions hit `double_set_cap(key, rung, opt_in)` / `double_set_cap(key, cap, on)` / `double_forget({target:'class'})`; `mergeCurves` weighting + week order and `chartProps` percent series + labels; `runLine`.
- `components/attention/double.test.ts` (3): `doubleIds`; undo shows during the window only (none before the answer, none after `undo_until`, none without a window, none once taken over, none for a digest row); the rail group.
- No json-render / catalog component added (catalog.test.ts untouched).

## Chart-kernel decision
Design 123's `LineChart` kernel **is** on this base (`src/components/chat/surface/diagrams/LineChart.tsx`, `LineChartView`); used as is, no chart library, no hand-drawn SVG.

## Deviations from the design / brief
1. **Undo = `double_take_over`.** The contract has no `double_undo`; the design says the grace *is* the undo for mutating approvals (§9 L2) and `double_take_over` "cancels a grace". The chip's `undo` and the inbox `Undo` call `double_take_over(question_id)` while `DoubleAnswered.undo_until` lies ahead. If W1 wants a distinct method, it is one call site (`DoubleChip.tsx:56 takeOver`).
2. **"Don't learn from this one" sends the `question_id`.** `DoublePredicted` carries no `examples.id`; the store has one example per `question_id` (`examples_question` unique index). Sent as `ForgetTarget { target: 'examples', ids: [question_id] }` — W1 should accept a question id there, or add `example_id` to `DoublePredicted` (then `DoubleChip.tsx:160` changes one field).
3. **Overall improvement curve is merged client-side** from `double_class(key).curve` per class (`mergeCurves`), n-weighted — the contract has no `double_status.curve`. Tens of classes = tens of cheap calls per page open (`ponytail:` comment at `Double.tsx:189`).
4. **Training log** = `double_status.last_run` + the session's live `DoubleTrainingState` events; the brief's "`double_status.runs`" does not exist in the contract (`last_run?: DoubleRun` only).
5. **KPI tiles are "last 30 days"** (what `Kpis` is defined over, `DoubleStatus.kpis` doc), not "this week"; labelled so.
6. **Multi-question asks get no pre-selection**: `DoublePredicted.index` maps `choice` to one option index and the §3 `ask_hash` keys the option set; with two or more questions the index is ambiguous, so `withDoublePick` acts on one-question asks only. The chip still shows.
7. **Draft cards**: none exist on this base (see render-site table); nothing rendered, nothing stubbed.
8. **`Tier B on node`** lists `mesh_status` peers by `reachable` only — `Peer` carries no `gpu`/`accelerator` (those live on `EngineStats`, not exposed by `mesh_status` on this base). W2's venue selection does the real filtering.
9. The design's `grace_until` field is `will_act_at` on the wire (C's contract); the countdown reads `will_act_at`.

## Gates (through `BUILD_LABEL=126-W4 /Users/Sky.Moore/.theuth/bin/build-lock`, foreground)

| # | command | result |
|---|---|---|
| 0 | `bun install` in `crates/desktop/ui` (622 pkgs) and `packages/apps` (484 pkgs — the `@openuidev/*` false-red) | ok |
| 1 | `bunx tsc --noEmit -p .` (crates/desktop/ui) | **198 errors, all pre-existing**; baseline before this wave's code: 200 (the two that disappeared were the `Record<AttentionKind, …>` literals that now have `double`). Diff of error lines vs baseline: 0 new. Every remaining error is stale-binding drift in files this wave does not own: missing `Packet`, `Item`, `Pointer`, `SensorInfo`, `RunKind`, `SensorArg`, `PairingLink`, `TelegramEvent/Status`, `PagePreview*`, `Client`, `ScreenContext*`, `PreviewId` bindings; `ScheduleInfo`/`RunInfo`/`Loaded`/`GraphNode|Edge|Group`/`GraphData` field drift (routines, store/reduce, surface/Graph, graph-layout); `MessageTool.tsx` `JobExit 'lost'`; `speech.ts:483`. New files (`DoubleChip.tsx`, `Double.tsx`, `store/double.ts`, the three tests) report 0 errors. |
| 2 | `bun run build` | fails at its `tsc -b` step on the same stale-binding errors; `bunx vite build` (the bundle itself): **✓ 13979 modules transformed, built in 5.11 s**, only the usual chunk-size warning |
| 3 | `CI=1 bunx vitest run src/components/settings src/components/chat src/components/attention` | **51 files: 50 passed, 1 failed; 496 tests: 489 passed, 7 failed** — the 7 are `chat/surface/catalog.test.ts` comparing `catalog.ts` (57 components) against the stale `bindings/surface-catalog.json` (38): pre-existing bindings drift, no file of this wave touches `surface/`. The 12 new tests pass. |
| 4 | smoke (throwaway vitest file, removed): `ApprovalCard` with an L2 `DoublePredicted` renders `role="button" aria-label="double answers Deny in 30 s — tap to take over"`, chip `● double answers Deny in 30 s — tap to take over`, Deny as the filled button, Allow outline; at L1 the chip reads `double · Deny · 88 %` and the frame has no `role` | ok |

Not run: the app against a live engine (no engine start allowed in this wave; `useApi` needs a connection) — the static-render tests and smoke are the visual evidence; `pairing.test.ts` / the full suite (out of scope).

Incident: a `git stash` / `git stash pop` pair I ran to re-check a pre-existing failure popped the shared `stash@{0} "pre-51-merge"` entry (my tree had nothing to stash); it conflicted on `bun.lock` and `docs/design/00-architecture.md`, was reverted with `git checkout -- <the two files>`, the stash entry is still kept, nothing of it committed. Same trap C hit; do not `git stash` in these worktrees.

## Open items / hand-offs
- **W1 (actor):** (a) `ForgetTarget::Examples.ids` must accept a `question_id` (or `DoublePredicted` gains `example_id`) — deviation 2; (b) `double_take_over(question_id)` during `undo_until` must undo an L2 answer (deviation 1) or a `double_undo` method must be added; (c) the `AttentionKind::Double` item id should be `double:<question_id>` (the UI strips `double:` to find the `DoubleAnswered`; `Hud.tsx:250 doubleIds`), and the digest `double:digest:<date>`; (d) `DoublePredicted.question_id` for a packet card: the UI tries `ask.id`, then `packet.key`, then `decision:<task>:<key>` — pick one and the others are harmless.
- **W2 / integrator:** `DoubleStatus.curve: Vec<CurvePoint>` (overall weekly curve) and `DoubleStatus.runs: Vec<DoubleRun>` would replace `mergeCurves` and the session-local run log with one call each (deviations 3, 4).
- **Integrator:** run `just ts-desktop`; afterwards `bun run build`'s `tsc -b` step and `catalog.test.ts` should go green on their own (every remaining red is bindings drift). `api.test.ts` checks `METHODS` against `bindings/methods.json` — the 13 names are in `METHODS`.
- **Mobile check (112):** the chip wraps (`break-words`, `max-w-full`), the L2 text is `text-sm` inside the chip and the whole card is the target; not exercised on a 390 px device in this wave.
