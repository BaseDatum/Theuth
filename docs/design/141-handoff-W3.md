# 141 hand-off — W3 (DESKTOP)

Branch `feat/141-w3`, cut from C `0b5f8aa2`, merged `feat/141-acp-subagents` before each gate. Everything
under `crates/desktop/ui/src/**` except `rpc/extra.ts` and `rpc/bindings/**` (C's, used as regenerated).
UI word everywhere: "Watching" / "watch-only"; `observed` appears only in code and the `kind` value.

## 1. What landed (§9 + Revision 1 D15 a/b)

### Plumbing (§9.1)

| file | change |
|---|---|
| `lib/session-tree.ts` | `sourceTag(o)` → `'omp task' \| 'opencode' \| 'ACP'`; `observedState(s): AgentState` (§5.2 table: running→running, requires_action→approval{tool:'permission'}, failed→error, cancelled→cancelled, else idle — Q1); `observedLabel` (pending / running / waiting on you / idle / finished / failed / stopped / lost contact); `harnessName(o, parentAgent)`; `ObservedDetail` + `detailOf(o)` (defensive read of `observed.detail`: agent, index, lastIntent, currentTool, toolCount, tokens, contextTokens, cost, completionPercent). `live()`/`stripRows()` unchanged — observed rows carry the mapped `AgentState`, so `running`/`requires_action` are live and `idle` is not (Q1). |
| `lib/money.ts` | `usd(n)` — a harness-reported float in `formatMoney`'s digits. |
| `store/sessions.ts` | `SessionSlice.kind?: SessionKind`, `observed?: ObservedMeta \| null`; `loadHistory` copies `Loaded.kind`/`observed`. `setChildren` unchanged (keeps observed rows). |
| `store/reduce.ts` | `route()`: `ObservedChildChanged` → child slice `{name, parent, kind:'observed', observed, harnessModel: model ?? prev}` + an `AgentInfo` row on every ancestor (walk `slice.parent`, depth++) with `state = observedState(observed.state)`, `agent: null`, `model`, `observed`, `started_at` kept from the previous row, `finished_at` set on terminal states; **no** `↳ spawned` interjection; `beat` preserved. Existing `trackRow`/`BEATS` keep beating on the child's deltas. |
| `lib/rpc.ts` | `on('ObservedChildChanged', sessions)` invalidates `list_sessions` like `SubagentSpawned`. |
| `components/sidebar/SessionItem.tsx` | `runningSubagents`: a row with `observed` (slice first, else the `SessionSummary`) counts iff `state ∈ {running, requires_action}`. |

### Surfaces (§9.2)

| surface | file | behaviour |
|---|---|---|
| shared bits | `components/chat/ObservedBar.tsx` | `WatchDot` (dot by state: success / warning / danger / grey; ring for pending+idle; dashed ring for unknown; `✓` for completed; blinks on `beat` only while live; `sr-only` state text), `EyeMark` (lucide `Eye`, `aria-label="watch-only"`), `SourceTag`, `observedNote`. |
| (a) tree row | `components/sidebar/AgentTree.tsx` `SubagentRow` | `WatchDot` + eye lead; source tag after the name; title `<name> · watching · <state>`; row at `opacity-60` while pending/idle/unknown; cost: `session_cost().children` as before (counted rows roll up), else greyed `detail.cost` for `inside_parent`, else nothing. `AgentSessionRows` inherits. |
| (b) roster | `components/chat/RosterCard.tsx`; dispatch at the top of `MessageTool` | `rosterRows(children, callId)` = children whose `observed.tool_call === callId`, sorted by `detail.index` then name; when non-empty the call renders `RosterCard`: header dot (live if any child live) · eye · mono tool name · `rosterFooter` ("2 subagents · 1 running · 1 done") · disclosure chevron to the raw args/output (Q4); `RosterRows`: one `Link` per child — `WatchDot` · name · `agent ?? description` · `lastIntent ?? currentTool ?? last` · Q9 progress bar when `completionPercent` present · `N tools · 9.1k tok` · cost by rule (`rosterCost`: counted → meter money from the parent's `session_cost`, else `detail.cost`; inside_parent → greyed; display_only/unreported → `—` with a title naming the rule) · `›`. Rows wrap (flex-wrap) on narrow panes. |
| (c) strip | `components/chat/SubagentStrip.tsx` | observed rows render `WatchDot`; title/aria `<name> · watching · <state> · <model>`. Membership via `stripRows` (live only). |
| (d) view | `routes/c.$session.tsx` | `watching = kind === 'observed'` → `ObservedBar` in the composer's place, no `Composer`. |
| (d) bar | `ObservedBar.tsx` | `role="status"`; `WatchDot` + state label; note `"<harness> runs this subagent, so you can watch it but not message it."` + caveat (opencode_task: "opencode 1.x does not stream subagent activity"; unknown: "theuth lost the harness; the last known state is shown"; idle: "idle — <harness> may resume it"); **Open parent** (Link); **Stop subagent** enabled only when `can_cancel` → `api.cancel(session,'root')`, else `disabled` + `aria-disabled` + title "<harness> cannot stop one subagent — stop the parent". `<harness>` = parent slice's `agent`, else by source (omp / opencode / the harness). |
| (d) header | `components/chat/Header.tsx` | eye before the title; the `parent` slot shows the `Watching` chip (`Badge outline`, accent border, eye) + crumb `inside <parent> · <source tag>` linking to the parent; compact menu row "Watching inside · <parent> · <tag>". Model and cost pills are the existing slots (below). |
| (d) model | `components/chat/ModelMenu.tsx` | `useModel` gains `watching`; `list_models`/`harness_options`/event refresh disabled; `resume` null; `ModelMenu` renders the plain label `harnessModel ?? '—'` (aria-disabled), `ModelRow` an eye row with no `onSelect`, `ModelDialog` nothing. |
| (h) cost | `components/chat/CostPill.tsx` | `Breakdown` children rows with `scope === 'observed'` get the eye; `CostDialog` passes the session's `observed` → one line `watch-only: <COST_RULE[cost_rule]>` (counted / inside parent / reported-not-counted / not reported). |
| D15 (a) | `components/library/Skills.tsx` | harness skill and command rows are buttons: `sendRaw(api, session, '/skill:<name>' \| '/<command>')` then navigate to the chat; never `draftSkill`. |
| D15 (b) | `components/chat/slash.ts`, `Composer.tsx` | `SlashItem {name, hint, desc, source?, raw?}`, `HarnessMenu`, `slashMenu(word, harness)`: built-ins (labelled `theuth`) then the harness's commands (labelled with the agent name, `raw: true`); a harness `skill:<x>` that theuth also has as a skill is listed twice — theuth's row first (hint `[words]`, desc "park theuth's x skill on the next message (typed ⏎)"), then the harness's raw row; `sendRaw(api, session, text)` = `markSent` + `send_input` when idle, `InputQueued` + `steer` mid-turn. Composer: `SlashList` takes items and shows the source chip; `pick(item)`: raw without hint → `sendRaw` at once; raw with hint → inserts `name ` and arms `rawCmd` so the next submit of that line goes `sendRaw` verbatim; typed `/skill:<name>` ⏎ still reaches `draftSkill` (unchanged branch). |

### Tests (§9.4 + D15)

`lib/session-tree.test.ts` (observed state mapping, strip/subtree membership, source tags, `detailOf`), `store/reduce.test.ts` (ObservedChildChanged: slice + ancestor rows, no interjection, in-place update, child deltas beat, terminal keeps `observed`), `components/chat/RosterCard.test.ts` (rows per call + order, footer, cost cell per rule, links/progress bar), `components/chat/ObservedBar.test.ts` (Stop gating by `can_cancel`, note per source/state, WatchDot), `components/sidebar/SessionList.test.ts` (`runningSubagents` with observed rows), `components/chat/slash.test.ts` (D15: `sendRaw` idle/busy paths; `slashMenu` merged + labelled list, shared skill name order).

## 2. Assumptions about other waves

- W2 publishes `ObservedChildChanged` before the child's `TurnStarted`/deltas/`TurnFinished` (§7.5) and again on every state change; the tree/strip/roster read `observed.state` for the glyph and `AgentState` (mapped) for liveness. If a `TurnFinished` on the child ever arrives after the terminal `ObservedChildChanged`, `trackRow` only moves `state` to idle/error/cancelled — the `observed` facts stay, so the check glyph is right.
- `observed.detail` keys per §6.4 (omp): `index, agent, lastIntent, currentTool, toolCount, tokens, contextTokens, cost, completionPercent` — C's `Progress::announce` carries **no `detail`** yet (hand-off C §3.4); W5 fills it. Until then the roster shows name/description/state and `—`/meter cost only.
- `session_cost(parent).children[]` carries `scope: "observed"` rows for `counted` children (C `CostScope::Observed`; W2 writes the meters).
- `Loaded.kind`/`observed` are optional in the bindings; `FRESH` and test fixtures were left as-is.
- `ObservedState.requires_action` → `AgentState{approval, tool:'permission'}`: the real tool name is not on `ObservedMeta`; the `PromptCard` keying on `state: approval` still fires.
- D15: harness commands that take arguments (`hint` set) are not sent on pick alone; the pick inserts the command and the submit sends the line raw. Harness commands without a hint go at once.

## 3. Not found / for the Closer

- `components/agents/OrgChart.tsx` `StripDot` duplicates the strip's dot logic for the org chart; left unchanged (not in §9's list) — observed rows there show the plain green dot without the eye/state colours.
- No fixture-driven story exists in this UI; screenshots of the four surfaces were not taken (vitest is `.ts`/SSR only and `vite dev` is barred inside build-lock). The Closer/Z smoke should capture tree, roster, strip and watch view against omp.
- `styles.css` untouched (Tailwind utilities only).

## 4. Gates (build-lock, `BUILD_LABEL=141-W3`)

(filled in below)
