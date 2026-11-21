# 208 as built — integrated (B engine + D UI)

Branch `feat/208-attention-graph`, worktree `theuth-attention`, on main `fab7d5f7a` (main has not moved). Builder notes:
`208-as-built-B.md`, `208-as-built-D.md`. Nothing here has been run against a live engine or seen in the Tauri app;
the UI was checked on the dev fixture page in headless Chromium (below).

## History

| commit | what |
|---|---|
| `33e6fc1f9` | B (engine, `1fe77acd3`) and D (UI, `e26b966ef` + `b870c8e10`) merged |
| `769fffcd2` | B's as-built note, final |
| `302f47124` | UI on the generated bindings; stream only while shown; viewport height, path fit, readable dim (F) |
| `472ff17ff` | engine: shared observed-state table, wider-window re-seed, `peer_changed` test, tui literal, `Buffered` boxed (F) |
| `811e4f53c` | UI: resize clamp (F) |
| (this doc's commit) | this document |

## What F changed

### Bindings (step 1)

`just wire-schema`, `bun run wire:generate`, the bindings copy, and `bun install --frozen-lockfile` (adds
`@xyflow/react` 13.0.2 to this worktree's `node_modules`) in hold 1. The first `bun run check` failed on two shape
differences from contract §7, both on the import side only (names and fields are as §7):

- `GraphCursor` is generated in `method.agent.input.ts` only (it is a param type; §7.1 also listed it in the output
  file). Imports in `queries.ts` and `reducer.ts` now come from the input file.
- `GraphPulse` / `GraphPulseKind` are generated only in `event.agent_2egraph.output.ts` (§7.1 allows this). Imports in
  `Canvas.tsx`, `reducer.ts` and the dev fixture now come from the event file.
- `AgentGraphParams` is generated as `[(number | null), SessionId[]] | [(number | null), SessionId[], (GraphCursor |
  null)]` and `MethodMap.agent_graph.params` with an optional third element (B's warning). The UI always passes three
  arguments; no change needed. `METHODS` and `TOPICS` now carry `agent_graph` and `agent.graph@1` (D's deviation 10 is
  closed by the regeneration).

No Rust change was needed for the UI.

### Engine (step 2)

- `supervisor/view.rs`: the 141 §5.2 table is `pub(crate) fn observed_state(o, live)` (re-exported as
  `supervisor::observed_state`); `View::state_of_row` calls it; `graph.rs`'s copy is deleted.
- `crates/tui/src/model/tests.rs`: the `PeerMessageChanged` literal gains `from`, `to`, `result`.
- **Wider window** (`graph.rs`, `Message<Snapshot>`): a call whose window is wider than the one the model was seeded
  with drops the model and seeds again over the wider window, keeping the old model's `include` set; the new epoch is
  strictly later than the old one, so every client re-snapshots on its next delta or renewal (the asker gets a full
  reply). A wider ask that arrives while a seed runs makes `Seeded` seed again before answering anyone. A flush begun
  on a replaced model is discarded (`Graph::flush_epoch`). Test `graph::tests::wider_window_reads_older_trees`: a
  root whose last event is 3 h old is absent at 1 h, a narrower renewal stays `unchanged`, the 4 h call returns it in
  a later epoch.
- Test `graph::tests::peer_changed_fills_ends_and_result`: two exchanges accepted through `Store::accept_exchange`
  (a normal message `r1` parent → child, a `result:t1` child → parent), then `Agents::peer_changed` (the publisher every
  send, delivery and reply path calls): both local parties receive `PeerMessageChanged` with `from`/`to` as stored and
  `result` false / true.
- `graph.rs` `Buffered::Agent` boxes its `AgentEvent`: `clippy::large_enum_variant` (B's gate allowed it for every crate;
  this round's gate allows it for `tui` only).

### Wiring review (step 3)

- `useAgentGraph(ttl, include, active)`: the delta subscription, the 120 s cursor renewal and the include/window asks
  run only while the region is expanded or full screen (`open || full` in `Region.tsx`); collapsing or unmounting the
  page unsubscribes and stops renewing, so the engine's lease runs out after 300 s and the actor goes dormant.
  Re-activation starts from a full snapshot. An activation counter drops replies to calls from an earlier activation
  and blocks follow-up calls while inactive.
- The include/window debounce was re-armed by every state change, so a stream with deltas closer than 1 s apart never
  sent it; it now fires 1 s after the first need and re-reads the need at that moment.
- Capability gating: `graphRequirements = { methods: ['agent_graph'], topics: ['agent.graph@1'] }` in
  `components/attention/requirements.ts`, checked with `useSupported` (the panel-requirements convention; legacy
  discovery does not advertise it, so the region stays hidden there). D's fallback (hidden when the first snapshot
  is rejected) stays.
- Gap/epoch handling (`applyDelta` → one full re-snapshot, collapsed by the pending/again rule), no refetch per event,
  the needs-you join (`attentionBySession`, items by session id, unexpired), the subtree filter (not persisted; cleared
  by "Clear filters"; select-all and the bulk bar act on the filtered entries; the selection drops items the filter
  hides, as for every filter) and the highlight (focused entry's head session, else the open item's; when the focused
  card leaves the list, the list's focus moves to the next card and the highlight follows it) were read and left as
  they are.

### Visual fixes (step 4)

- (a) Default expanded height: `GraphPrefs.height` is `null` until the user resizes; the region then uses
  `defaultGraphHeight(innerHeight)` = 40 % of the viewport, at least 280, at most 60 % (360 px at 900). Reset
  (double-click on the handle) returns to it. The drag was clamped to 160 px: `maxH` read the height of the region's
  parent, a `display: contents` wrapper (0); it now reads the page (`article`).
- (b) A highlight alone never moves the viewport (checked: transform unchanged after a focus change). "Show in graph"
  fits the whole path root → node (`fitView({ nodes: path, maxZoom: current zoom })`, so it never zooms in). Every fit
  keeps 48 px above the top row (`FIT_PADDING`); 350 ms after mount the first fit (clamped to the compact zoom) pulls a
  cut top row into view.
- (c) Off-path nodes dim to 0.5 (fold chips too), without multiplying a TTL fade (the lower of the two wins); nodes
  with a red ring (attention > 0) are never dimmed.
- (d) The strip's "need you" was the number of graph nodes needing you (4 at the fixture instant) while the list header
  counts live items (34). The strip now shows the header's number (`live.length` from `Page.tsx`); working / finished
  stay node counts and appear once the graph has loaded (collapsed on a fresh page the strip reads "34 need you",
  because a collapsed region does not call the engine).

## Seen in the browser

Headless Chromium, `bunx vite --port 1439`, `src/dev/attention-graph-fixture.html`. Screenshots in
`/tmp/theuth-208-shots/f-*.png`:

| file | what was checked |
|---|---|
| `f-01-expanded-light-1280` | region 360 px high at 900 px viewport; strip "34 need you · 11 working · 8 finished" with header "Attention 34" |
| `f-02-show-in-graph-path-light-1280` | Show in graph on the first card: `builder` and its root `Ship 2.5 release` lit, root 127 px below the canvas top (canvas 328 px); off-path opacity 0.5; the four red-ring nodes at 1 |
| `f-03-show-in-graph-path-dark-1280`, `f-04-dim-full-lod-dark-1280` | same in dark; zoomed in two steps, dimmed labels legible |
| `f-05-resized-dark-1280` | the handle dragged 120 px down: 360 → 477 px, stored `"height":477`; after reload and re-expand still 477 |
| `f-06-collapsed-dark-1280` | collapsed: canvas unmounted, strip "34 need you · 11 working · 9 finished" (last loaded state) |
| `f-07-mobile-collapsed-light-390`, `f-08-mobile-fullscreen-light-390` | 390 × 844: starts collapsed reading "34 need you" (no engine call yet); opening goes full screen with counts |

Also: focusing another card moved the highlight without changing the viewport transform; after a reload with the
region collapsed, the strip reads "34 need you" only (no graph call was made).

## Gates

Script `/tmp/theuth-208-f-gate.sh <steps>`, log `/tmp/theuth-208-f-gate.log`, each step's output in
`/tmp/theuth-208-f-<step>.out`. `CARGO_TARGET_DIR=<worktree>/target`, `CARGO_BUILD_JOBS=4`, `--test-threads=4`. Clippy:
`-D warnings --all-targets -A clippy::chunks_exact_to_as_chunks -A clippy::single_match` (tui also
`-A clippy::large_enum_variant`).

| hold | step | rc | counts |
|---|---|---|---|
| 1 (21:46) | `just wire-schema` | 0 | export 1 passed |
| 1 | `bun install --frozen-lockfile` | 0 | |
| 1 | `bun run wire:generate` + copy | 0 | |
| 1 | `bun run check` | 1 | 6 TS2305 (import locations, fixed in `302f47124`) |
| 2 (21:54) | `bun run check` / `test` / `build` | 0 / 0 / 0 | 390 files, 2790 tests; `Canvas-*.js` 207.92 kB (gzip 72.19 kB), `Canvas-*.css` 15.99 kB (gzip 2.70 kB) |
| 2 | clippy agent, rpc-compose, sessionsync | 101 | `large_enum_variant` on `graph.rs` `Buffered` (fixed in `472ff17ff`) |
| 2 | clippy tui | 0 | |
| 2 | test agent `--lib graph` | 0 | 16 passed (14 of B + 2 new) |
| 2 | test agent `--lib store` | 0 | 164 passed |
| 2 | test agent `--lib communication` | 101 | 167 passed, 1 failed: `notices_are_automatic_and_stop_at_the_chain_limit` (red, predates the branch) |
| 2 | test rpc-compose | 101 | 82 passed, 2 failed: `legacy_event_families`, `agent_golden_parity` (known reds) |
| 2 | test sessionsync | 0 | 65 passed |
| 2 | test roster + tasks | 0 | 130 + 124 passed |
| 2 | `just wire-schema` + classifier (`wire_schema_snapshot`) | 0 / 0 | 1 passed: the export equals B's accepted fixture, nothing changed since B's accept |
| 3 (22:08) | clippy agent, rpc-compose, sessionsync | 0 | |
| 3 | test agent `--lib graph` | 0 | 16 passed |
| 3 | `bun run check` / `test` / `build` | 0 / 0 / 0 | 390 files, 2790 tests; Canvas chunk as above |
| 4 (22:35) | classifier against main's `fab7d5f7a` fixture | 101 (expected: it differs) | 8 `Additive`, 0 `Nonbreaking` / `ReviewRequired` / `Breaking`: `/methods/agent_graph`, `/x-theuth/events/agent.graph@1`, `agent.peer-message@1` `from` / `to` / `result` in `schema` and `receiverSchema` |
| 4 | classifier against the accepted fixture (restored, tree clean) | 0 | 1 passed |
| 5 (22:45, UI only, review fixes) | `bun run check` / `test` / `build` | 0 / 0 / 0 | 391 files, 2793 tests; Canvas chunk unchanged (207.92 kB, gzip 72.19 kB) |

## Reds (all predate the branch)

- rpc-compose `wire_golden::legacy_event_families`, `wire_parity_inc4::agent_golden_parity`: the stale `agent.events`
  fixture (fail at `agent.events[37]` `SubagentSpawned`; known red on main).
- agent `communication::files::notices::tests::notices_are_automatic_and_stop_at_the_chain_limit` (`notices.rs:593`):
  known red on main; B also ran it with `crates/agent` at base `98bc9b58b` and it failed the same way
  (`208-as-built-B.md`, hold 3).
- Not run this round (outside the gate list): secrets `concurrent_first_opens_all_succeed`, the tools git clone test
  (gpg), the a2a forward race.

## Reviewer findings

The UI review (orchestrator message stamped 21:47) reached F at 22:39, after the first final report. All findings were
checked by reading and confirmed. Fixes were verified in a fifth, UI-only hold (see Gates; one over the four-hold
limit, announced to the orchestrator first).

| finding | what F did |
|---|---|
| U1 major, clock skew | Fixed. New pure `graph/clock.ts`: `noteEngineTime(at_ms)` on every snapshot and delta sets skew = engine `at_ms` − local clock; `graphNow()` = local + skew; the fixture clock goes through the same module. Test `clock.test.ts` |
| U2 major, full screen remounted the canvas | Fixed. `Region.tsx` renders one tree: the wrapper's class switches (`fixed inset-0` vs inline), the header slot swaps strip ↔ full-screen bar, and the canvas stays in its slot, so viewport, LOD and open folds survive Shift+G / Esc. Not seen in the browser after the change |
| U3 major, snapshot/delta ordering | Fixed. Deltas arriving while a call is in flight are buffered (cap 500, past it the next call is full) and applied after the reply through the new pure `settleSnapshot`; `mergeSnapshot` drops a reply older than the state (earlier epoch, or same epoch and lower seq). Two reducer tests |
| U4, lease lifetime, hidden tab, retry, stale marker, single timer | Not done. The lease now follows the canvas (expanded or full screen), not the page; see the collapsed-strip trade-off above. No visibility pause, no backoff retry, no stale marker. One timer per `useAgentGraph` caller (only `GraphRegion` calls it) |
| U5, Escape and `g` | Fixed. The capture-phase Esc also yields to `[role=dialog]` / `[role=alertdialog]`; `g` is ignored while full screen is up |
| U6, subtree node left the graph | Fixed. The strip shows the last known label plus "(left the graph)", never a raw id; the filter and its clear button stay. The toolbar chip already used the filter's own label |
| U7, window widening | Fixed. `mergeSnapshot` takes `window` from an `unchanged` reply too; the asked window resets when a reply replaces the state. Covered by the U3/U7 reducer test |
| U8, double derivation and peers × edges loop | Not done |
| prefs validation (quick read asked) | `readGraphPrefs`: non-numbers, NaN and missing parts fall back to defaults; out-of-range numbers are clamped to the range rather than reset to defaults (height [160, 2000], TTL minutes per kind). Left as is |

No engine review findings arrived.

## Not verified

- Nothing ran against a live engine: no `subscribe_topics` delivery of `agent.graph@1` (the `always_wanted` policy), no
  real `agent_graph` reply in the UI, no lease expiry observed after collapsing, no re-seed observed through the RPC.
- Not seen in the Tauri app.
- `useAgentGraph`'s activation, renewal and debounce logic has no unit test (React hook; read and checked on the
  fixture only: collapse unmounts the canvas, a collapsed reload makes no call).
- Folding (D17), `onlyRenderVisibleElements` above 50 nodes, "Load messages" with real records: as D left them.
- The wider-window re-seed resets every client to a new epoch; clients whose window did not change re-snapshot once.
  Not measured for cost on a large store.
