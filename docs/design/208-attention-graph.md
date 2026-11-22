# 208 — ATTENTION-GRAPH: a live canvas of every active agent tree above the Attention list

Status: decided 2026-11-21 (Sky's words and decisions 1–6 below), written by W0 on `feat/208-attention-graph` off main `fab7d5f7a` (which carries round 1 = 207). Number 208 was free: no `docs/design/2(08|09|1[0-9])-*` file on any local or remote branch (`git ls-tree` over every ref), no `feat/208-*` branch but this one. Round 2 of the attention redesign (plan `/tmp/theuth-207-plan.md` §3–§4): builders **B** (engine graph, `crates/agent` + wire) and **D** (canvas UI, `components/attention/graph/**`, on fixtures) work in parallel against the frozen contract `docs/design/208-contract.md`; **F** integrates. Every `path:line` below was re-opened in this worktree.

## 0 Decisions in one screen

Sky's words (binding):

> "Above the list and view area for an attention item, there is an expandable infinite canvas area that is a live view 2d graph of all active and recently active agent and subagent trees and communication (historical and live) between them. Everything has a ttl and subagents' ttl is the shortest. ttl is only activated when there is no activity for a while. so essentially ttl starts after most recent event." — "the goal is to be able to monitor and observe and drive many agents at once from one place without looking at 6 or 10 scrolling chat windows in a grid." — "when answering/reviewing a list item it should indicate in the graph which session this came from." — "on the graph items there should be indicators like we use in the sidebar to know what each agent and subagent is doing." — Layout: "tidy tree stable, it's the usual infinite canvas people use for workflows… dotted background usually, popular library in react for this."

| # | Ruling (owner) | Lands in |
|---|---|---|
| S1 | Renderer `@xyflow/react` with dotted `Background`, `Controls`, `MiniMap`; positions from our deterministic tidy tree, never from a force layout or the library; lazy chunk; amends 147 (and 73, 65 §0.4) for this canvas only | §7, §9 |
| S2 | Nodes = sessions (roots, native subagents, ACP harness children, observed watch-only children, desks) + boundary nodes (mesh peers, contacts). Solid = parent → child; dashed = peer messages, animated in flight, fading, merged per pair with a count; a child's result is an edge event; open attention items give a "needs you" ring + count | §2, §7.3 |
| S3 | Node indicators are the sidebar's own components and helpers | §7.2 |
| S4 | TTL restarts on every event; defaults child 10 min, message 5 min then folded into the pair's count, root/routine run 4 h, desk 24 h; never expires with an open item or a live descendant; fades over the last quarter | §3 |
| S5 | List ↔ graph: card selection lights its node and path to root; node click filters the list to the subtree and offers Open session; "Show in graph" on cards; collapsed strip "N need you · N working · N finished"; resizable, full screen, persisted; md: collapsed, opens as its own pane; `G` toggles | §7.4–§7.6 |
| S6 | One snapshot RPC and one coalesced delta topic; no refetch per event | §4, §5 |

W0's calls where the rulings left a choice (each marked "(W0)" where made; the owner may overrule any):

- **D1** The aggregate is a **new actor `Graph` in `crates/agent`** (`crates/agent/src/graph.rs`), not inside the `Agents` supervisor and not in `crates/roster`. Every input is in `agent` (sessions, `ActivityOf`, `peer_exchanges`, `waits`, the bus); `roster` depends on `agent`, not the reverse; a separate actor keeps the supervisor mailbox free (176 E1).
- **D2** **Attention counts are joined on the client**, from the 207 cache (`useAttention`, `item.session`), not carried on the graph wire. One source of truth for "needs you" (snoozed/dismissed/expired items already excluded there), no coupling of `agent` to `roster`, no second publisher of the same fact.
- **D3** **Lease, not always-on.** The actor is dormant until `agent_graph` is called; it seeds its model then and publishes deltas for `LEASE_SECS = 300` after the last call. The UI renews every 120 s with its cursor (`unchanged: true` reply, no payload). Dormant cost: one `HashMap` insert per bus event (the `last_event_ms` clock) and one batched upsert every 30 s.
- **D4** **Expiry is client-side.** The engine serves a window (default 24 h, max 7 d) and `last_event_ms` per node and never emits a removal for time; the client fades and drops. TTLs are view preferences (like the sidebar linger, a per-device setting), fading is per-frame time display (207 §7: time-dependent display is the client's), and a timer-driven delta stream would cost the engine work for nothing.
- **D5** `last_event_ms` persists in a **local side table `session_last_event`** (agent migration 0073), written in 30 s batches; fallback `max(updated_at, finished_at)`. No replicated column, **no `SESSIONS_WIRE_SCHEMA` bump** (stays 70, `crates/agent/src/store.rs:732`).
- **D6** Message edges come from `peer_exchanges` (window read, new index on `updated_at`) and live from `PeerMessageChanged`, which gains additive `from`, `to`, `result`. **A child's result already is a `peer_exchanges` row** (sender `session:<child>`, request id `result:<turn>`, `supervisor.rs:2644` `deliver_result`), so it needs no new event: it is an edge event of kind `result` on the parent–child pair.
- **D7** **No `AgentState` change.** "Waiting on you / on a job / on children" come from the durable `waits` journal (`0033_durability.sql:63-71`) as `GraphWait`; the child's outcome from `TurnFinished.stop_reason` / `sessions.last_stop`, mapped exactly as 207 §4 maps it.
- **D8** Edges aggregate per **unordered pair** with per-direction counts; tree edges are implicit in `node.parent` (no edge list for them).
- **D9** Node kinds `root | desk | child | observed | peer`. An ACP harness child is a `child` with `agent` set (the sidebar's `HarnessGlyph` tells harnesses apart); advisor and `ask` sessions and private (51 §4.5) sessions are not in the graph.
- **D10** Bounds: window ≤ 7 d; ≤ 1 500 nodes per snapshot (older idle trees fold into one `older` stub); ≤ 3 000 edges from ≤ 5 000 newest exchange rows; deltas ≤ 4/s (250 ms minimum spacing, trailing, not resetting — 65 §4.3's queue); ≤ 500 node upserts per delta, else `resync`.
- **D11** `@xyflow/react` **13.0.2, exact pin** (§9.1). Only APIs that are identical in 12.12 and 13 are used, so a fallback to 12.12.1 is a one-line change.
- **D12** Graph prefs (expanded, height, full screen, follow, TTLs) are a **client pref in `localStorage` key `attention.graph`**, beside `attention.view` (`components/attention/Page.tsx:43`) and `sidebar-active-linger-hours` (`lib/sidebar-linger.ts:7`); edited in a popover on the graph toolbar, not in `config.yaml` and not on the Settings page this round.
- **D13** Roots are packed in **creation order with sticky slots**: a tree keeps its x while present; new trees go to the right; gaps left by expired trees close only on "Tidy" (the fit button) or when the canvas is reopened.
- **D14** `G` is page-scoped (the Attention page, not while typing), not a global shortcut; `Shift+G` toggles full screen; `Esc` leaves full screen.
- **D15** Edge labels are counts only. Text is never on the graph wire: clicking a message edge opens a popover that loads the pair's last 20 messages through the existing `list_peer_messages(session, …)` (`crates/agent/src/rpc_messaging.rs:61-68`).
- **D16** Level of detail by zoom: ≥ 0.75 full card, 0.4–0.75 compact, < 0.4 dot. The box size never changes, so positions do not depend on zoom.
- **D17** A finished subtree (no live or pinned node, ≥ 8 descendants) folds into a "+N finished" chip under its parent; click expands it for the session.
- **D18** The viewport never moves by itself unless "Follow activity" is on (default off), and even then not while a pointer is down or within 3 s of the user's last pan or zoom.

## 1 Problem, with evidence

1. **No cross-root view exists.** `list_sessions` returns every row (2.39 MB measured, `176-live-under-load.md:25-27`); `list_agents(root)` is per root (`crates/agent/src/rpc_sessions.rs:28`); `list_activity` (`rpc_turns.rs:75`) is flat; nothing returns the forest with live state, and the sidebar tree (`components/sidebar/AgentTree.tsx:191`) shows one root at a time.
2. **Refetching per event stalls the engine.** 176 measured the `Agents` mailbox held 0.96–6.9 s per list call and `info` p99 25.6 s, max 29.6 s, from the macOS companion's per-event `Store.refetch()` firing six list RPCs (`176-live-under-load.md:20-27`). A graph built by the client from those RPCs would repeat it.
3. **Communication is invisible across sessions.** `peer_exchanges` (`crates/agent/migrations/0011_communication.sql:17-39`) is indexed only for pending intake, sender state and scope (`:40-42`); `list_peer_messages` is per session; `PeerMessageChanged{session,id,state}` (`crates/agent/src/communication.rs:771-782`) does not say who talked to whom.
4. **"Last activity" does not survive a restart.** `last_input`/`last_stop` are in-memory maps on `Agents` (`supervisor.rs:305-308`); `SessionActivity.last_active` falls back to `updated_at` when cold (`supervisor/view.rs:241-246`), which also moves on rename.
5. **Prior rulings forbid this canvas.** 65 §0.4 "one React renderer" (`65-graph-diagrams.md:14`), 73 rejects "react-flow/xyflow (a second renderer beside `GraphSvg`)" (`73-codemap.md:14`, `:482`) and "an infinite/pannable canvas as the primary surface" (`:391`, `:431`), 147 rejects "A second renderer (xyflow, dagre, cytoscape)" (`147-codemap-perspectives.md:985`), 19 §8 rejects a server-side derived board (`19-subagent-board.md:419-420`). §9 amends each, scoped.

## 2 Data model

### 2.1 Node

One `GraphNode` per session in the window, plus one per boundary peer. Exact Rust and TS in the contract §2/§7.

| Field | Source |
|---|---|
| `id` | session id; boundary: `peer:<name>` (`PeerAddress::Remote`, `communication.rs:895-920`; a contact address `peer:<contact>/session:<uuid>` folds into `peer:<contact>`) |
| `kind` | `root` (no parent, no desk) · `desk` (`SessionSummary.desk` set) · `child` (parent set, `kind = chat`; an ACP harness child has `agent`) · `observed` (`SessionKind::Observed`, 141 D2) · `peer` |
| `parent`, `root` | `SessionSummary.parent`, walked to the root |
| `title`, `name` | root/desk: title (empty → `<desk> desk` / short id, as `roster::attention::lineage_map` labels them, `crates/roster/src/attention.rs:708`); child: the peer name |
| `agent`, `model`, `project`, `desk`, `owner`, `remote` | `SessionSummary` (`crates/agent/src/lib.rs:3120`); `project` is the root's; `remote = !here` |
| `activity` | `SessionActivity` (`lib.rs:1732`) from the existing internal `ActivityOf(ids)` ask (`supervisor.rs:899`, handler `:8195-8202`, off the mailbox); remote-owned: `Running` when `SessionSummary.running`, else `Idle` |
| `waits` | `user` (a `waits` row of kind `question` — ask, hand-off, secret; an approval is `AgentState::Approval`), `job` (`job`, `pty_wait`), `children` (`child_result`); a peer wait is `AgentState::Awaiting{from}` already |
| `outcome` | when not live: `finished · failed · cancelled · timed_out` from the last stop reason, 207 §4's mapping; `interrupted`/`suspended`/`harness_continued` → none |
| `observed` | `ObservedMeta` for watch-only children (`lib.rs:3046-3082`) |
| `started_ms`, `last_event_ms` | `created_at`; the clock of §2.3 |

### 2.2 Message edge

One `GraphEdge` per unordered pair `{a, b}` (`a < b`) with `ab`, `ba` counts, `results` (exchanges with request id `result:*`), `open` (exchanges in `accepted` or `held`: in flight), `last_ms`, `last_from`, `last_id`. Live, the delta also carries `pulses` — one per exchange state change: `sent`, `delivered`, `replied`, `result`, `failed` — which the UI animates along the edge.

"Awaiting reply" is the sender's `AgentState::Awaiting{from}`; the edge animates while `open > 0` or either end awaits the other.

### 2.3 `last_event_ms`

The time of the session's last bus event of any kind (`AgentEvent` incl. deltas, `PeerMessageChanged`, `PeerDelivered`, approval/ask events, job events), kept in memory always (also while dormant) and upserted every 30 s into `session_last_event(session, at_ms)` with `max()`. Restart loses at most 30 s. A session without a row reads `max(updated_at, finished_at)`. Remote-owned sessions advance only from replicated rows, or live while some local subscriber follows them (`crates/sessionsync/src/events.rs:780-790` republishes their events on the local bus).

## 3 TTL

### 3.1 Rules (client, D4)

- `ttl(node)` by kind: `child`, `observed` → `ttl.child` (10 min); `root` → `ttl.root` (4 h, the sidebar linger default `lib/sidebar-linger.ts:10`); `desk` → `ttl.desk` (24 h); `peer` → `ttl.root`. Message activity: `ttl.edge` (5 min).
- `age = now − last_event_ms`. Any event moves `last_event_ms`, so the countdown restarts.
- **Pinned** — never fades, never expires — when the node has an open attention item (D2), is live (`activity.state` running/awaiting/approval, or any `waits`), or has a live or attention-holding descendant.
- Otherwise opacity is 1 while `age < 0.75·ttl`, then `1 − 3·(age − 0.75·ttl)/ttl` (1 → 0.25), and the node **expires** at `age ≥ ttl`.
- An expired node with a visible descendant stays as a **ghost** (opacity 0.25) so the tree is never cut; otherwise it is dropped.
- A message edge is **active** while `open > 0`, either end awaits the other, or `now − last_ms < ttl.edge`; afterwards it is **folded**: thin, neutral, the pair's count only. An edge is drawn only when both ends are visible.
- A boundary peer node is visible while one of its edges is (its `last_event_ms` is its newest edge's `last_ms`).

### 3.2 Worked timeline (defaults)

| Time | Event | `builder` (child) | `linter` (child of builder) | `Ship 2.5` (root) | edge builder–linter |
|---|---|---|---|---|---|
| 10:00 | root starts, spawns `builder` | live | — | live | — |
| 10:01 | builder spawns `linter`, sends it a message | live | live | live | open, animated |
| 10:03 | linter replies; builder's turn continues | live | live | live | active (`ab 1`, `ba 1`) |
| 10:04 | linter finishes, result to builder | live | idle, `finished`, age 0 | live | `result` pulse on the tree edge |
| 10:08 | — | live | 4 min | live | folded (last 10:03 + 5 min) |
| 10:11:30 | — | live | fading starts (7.5 min) | live | folded |
| 10:12 | builder asks for approval | approval → pinned, ring "1" | 8 min, opacity 0.85 | live | folded |
| 10:14 | — | pinned | expired, dropped | live | hidden (an end is gone) |
| 10:15 | you allow; builder finishes, result to root | idle, `finished`, age 0 | — | live | — |
| 10:25 | root's turn ends | 10 min → expired | — | idle, age 0 | — |
| 13:25 | — | — | — | fading starts (3 h) | — |
| 13:40 | a message to the root | — | — | age 0 again, opacity 1 | — |

## 4 Wire (all additive)

- **RPC** `agent_graph(window_ms: number | null, include: SessionId[], cursor: GraphCursor | null) -> AgentGraph`, trait `AgentGraphApi`, owner `agent`, new file `crates/agent/src/rpc_graph.rs`. `window_ms` null → 24 h, clamped to 7 d. `include`: sessions to show regardless of the window (the client passes the sessions of its open attention items, ≤ 200, with their ancestors). A `cursor` equal to the current `{epoch_ms, seq}` returns `unchanged: true` with empty lists (the lease renewal). Each call re-arms the lease.
- **Topic** `agent.graph`, `VERSION` 1, event `AgentGraphChanged`: `{epoch_ms, seq, at_ms, nodes (upserts), removed, edges (upserts), edges_removed, pulses, older, resync}`. `seq` is 1-based per activation; a new activation is a new `epoch_ms`. Policy `always_wanted: true` (an engine-wide event without a session, like `CommunicationChanged`, `communication.rs:755-765`); not a delta-class event, so 129's focus filter passes it (`129-event-focus.md:51-52`).
- **`PeerMessageChanged`** gains `from` and `to` (endpoint addresses `session:<uuid>` | `peer:<name>`) and `result: bool` (request id `result:*`); its publisher already holds both (`supervisor.rs:2204-2224`).
- Client rules, mirroring 207 §7: apply a delta only when `epoch_ms` equals and `seq = last + 1`; any gap, a new epoch, `resync`, or a reconnect → one new `agent_graph` call. Stale (`seq ≤ last`) deltas are ignored. Never a call per event.

## 5 Engine design and cost (B)

- **Actor.** `Graph` (kameo, `GRAPH_ACTOR = "graph"`), spawned in `agent`'s `Plugin::start` with `runtime::subscriber_mailbox()`; subscribes to `AgentEvent`, `PeerMessageChanged`, `PeerDelivered`, `tools::{ApprovalRequested, ApprovalResolved, AskRequested, AskResolved}` and the jobs event. Holds a `Store` clone; every store read runs in a spawned task.
- **Dormant** (no lease): per event `last_event.insert(session, now)` and a dirty flag; a 30 s tick upserts dirty rows. Nothing else.
- **Activation** (first `agent_graph` call): one in-process `ListSessions` (the store list, delegated off the supervisor since 176 E1), `session_last_event` rows, window selection (§5.1), one `ActivityOf` for the chosen ids, the `waits` rows for them, and the window's exchanges (`WHERE updated_at >= ? ORDER BY accepted_seq DESC LIMIT 5000`). Calls arriving during the seed wait for it.
- **Maintenance.** Each event marks its session(s) dirty; structure events update the model directly (`SubagentSpawned`, `ObservedChildChanged` add a child; a `TurnStarted` of an unknown session reads its one summary row; `SessionDeleted` → `removed`; `TitleChanged`, `ModelChanged`, `SessionOwnerChanged` patch fields; `TurnFinished` sets `outcome`). The delta-class events (`TextDelta`, `ReasoningDelta`, `ToolCallProgress`, `VisionProgress`) only move `last_event_ms` and make a node dirty when its published `last_event_ms` is ≥ 30 s old.
- **Flush** (≤ 4/s): one `ActivityOf(dirty)` ask, one `waits` read for the dirty ids, build nodes, drop those equal to the last published copy, publish `AgentGraphChanged` if anything is left. > 500 upserts → `resync: true` and nothing else.
- **Lease end** (300 s after the last call): drop the model, publish nothing; the next call is a new epoch.
- **Window selection.** A tree enters when one of its nodes is live, was active within the window, or is in `include`; inside a tree, a node enters when it is live, active within the window, included, or an ancestor of one that is. Over 1 500 nodes, whole trees without a live or included node fold into `older {roots, nodes, newest_ms}`, oldest first. An exchange whose ends are not in the set brings its session ends (with ancestors) in; ends that are `peer:` addresses become boundary nodes.
- **Cost.** Dormant: one map insert per event (the bus already delivers each event to the Attention actor too, `crates/roster/src/attention.rs:1765-1775`) and ≤ 1 small write per 30 s. Active: ≤ 4 small asks + reads + publishes per second, all off the `Agents` mailbox except the in-memory `View` clone `ActivityOf` already does. Snapshot: in memory; at 1 500 nodes ≈ 1 KB/node JSON ≈ 1.5 MB, the worst case; a typical day (≤ 200 nodes) ≈ 200 KB.

## 6 Mesh

Session rows replicate, so remote trees are complete; their state is last-known (`SessionSummary.running`, `finished_at`, `last_stop`) and the node carries `remote: true` and `owner`. Live events of a remote-owned session reach this engine's bus only while some local client follows that session (`sessionsync/src/events.rs:121-125`, `:780-790`), so a remote node may update live incidentally; the graph does not follow sessions itself this round. Cross-engine live relay is a follow-up (§13).

## 7 UI (D)

### 7.1 Region

`components/attention/Page.tsx` gets `<GraphRegion/>` between the 52 px header (`:284-292`) and the rail/detail row (`:293`), spanning both panes and rendered even when the list is empty. Collapsed: a 32 px strip — "3 need you · 5 working · 2 finished", an expand chevron, and, when a subtree filter is on, its chip. Expanded: the canvas, default 320 px high, resizable through `components/ui/resize-handle.tsx` (horizontal, 160 px – 80 % of the page), plus a full-screen button. Under `md` it starts collapsed and opening it shows the canvas as its own pane (full screen with a back button). Hidden entirely when the engine lacks `agent_graph` (`useQuerySupport`). The canvas chunk (`@xyflow/react` + its CSS) is `React.lazy`-loaded on first expand (`components/costs/Resident.tsx:12` is the pattern); the strip needs only the data hook.

### 7.2 Node (indicators reused, S3)

Reused as they are: `GLYPH` (state glyph) and `ago` from `components/sidebar/AgentTree.tsx:23,33`; `HarnessGlyph`, `shortModel` from `components/sidebar/harnessGlyph.tsx:79,89`; `stateText`, `modelLabel` from `components/agents/sessions.ts:15,18`; `counts` (jobs/ptys) from `components/chat/broadcast.ts:89`; `ActivityDot` from `components/chat/SubagentStrip.tsx:57`; `WatchDot`, `EyeMark` from `components/chat/ObservedBar.tsx:30,47`; `live`, `observedState`, `observedLabel` from `lib/session-tree.ts:14,30,42`. The sidebar's state colour (`AgentTree.tsx:94-104`) is inline; D extracts it into an exported `stateTone(state)` in the same file with no behaviour change, and both use it.

Anatomy (box 200 × 64 at every zoom, D16):
- **Full** (zoom ≥ 0.75): row 1 state glyph, harness glyph, name (root/desk: title), attention badge; row 2 the state text ("approval: bash", "awaiting builder", "waiting on you", "waiting on a job", "waiting on children", or the outcome word), `shortModel`, `counts(activity)` when jobs/ptys, `ago(last_event)`; a "remote · <owner>" chip; the eye mark for watch-only.
- **Compact** (0.4–0.75): glyph + name + badge.
- **Dot** (< 0.4): a 14 px dot in the state tone with the ring.
- "Needs you": a 2 px `ring-danger` around the box and the count badge; the minimap paints those nodes in the danger colour.
- Hover: a tooltip with `dotLabel`-style text and the path. Selection: accent border. Highlight (from the list): accent border on the node and the solid edges up to its root, others dimmed to 60 %.

### 7.3 Edges

Tree edges: smooth-step, solid, neutral; a `result` pulse runs child → parent along it. Message edges: bezier, dashed; active = accent and, while open or awaited, `animated`; folded = 1 px neutral at 50 %; label = count (`ab + ba`, "↔ 12"), never text (D15). Clicking a message edge opens a popover (the two names, counts per direction, last time, the last 20 messages loaded on demand); clicking a tree edge does nothing.

### 7.4 List ↔ graph

- The focused or open card's `session` (207 items carry `session`, `root_session`, `path`) is the highlighted node; the path comes from the graph's own parents.
- A card's "Show in graph" (lucide `Network`, in the actions row) expands the region, centres the node (no zoom change) and highlights it.
- Clicking a node selects it and shows a node bar: "Filter list to this tree" (default action of the click), "Open session" (`/c/$session`), "Clear". The subtree filter is a new `ListFilter.subtree` in `components/attention/model.ts` (contract §8.2): `{session, label, ids}`, `ids` recomputed from the graph as it changes; not persisted; shown as a removable chip in the toolbar; "Clear filters" clears it.

### 7.5 Viewport

`fitView` once on first expand; afterwards the viewport is the user's. Controls: zoom in/out, Fit (also re-packs roots, D13), Follow activity (toggle, D18: pans to the newest pulse or state change), Full screen, TTL settings. `minZoom 0.1`, `maxZoom 2`. Nodes are not draggable or connectable; pan by drag on the pane, zoom by wheel/pinch. Reduced motion: no dash animation (a static "in flight" dot), no pulses, instant viewport moves; fading still applies as static opacity.

### 7.6 Keys

On the Attention page, outside inputs: `G` expand/collapse, `Shift+G` full screen, `Esc` leaves full screen. The list's own keys (207 §6) are untouched.

### 7.7 Scale and performance

| Visible nodes | Behaviour |
|---|---|
| ≤ 50 | everything as above |
| ≤ 200 | `onlyRenderVisibleElements`; LOD; finished subtrees fold (D17) |
| ≤ 1 000 | as 200; message edges below 0.4 zoom only while active; the minimap stays |
| > 1 000 | the engine's `older` stub keeps the snapshot ≤ 1 500; the strip says "+N older trees" |

Rules: layout is pure and memoised per root on a structural key (child lists), so a state change re-renders one node and no layout runs; the reducer keeps object identity for untouched nodes; node components are `memo` on `(node, lod, ring, highlight)`; the TTL clock ticks every 5 s while a node is fading and otherwise at the next fade start (no per-second re-render of the canvas); time is passed as a bucketed `now`.

## 8 Settings

`localStorage['attention.graph']` = `{expanded, height, fullscreen: false (never restored), follow, ttl: {child_min, root_min, desk_min, edge_min}}` read and written like `attention.view` (try/catch, defaults on garbage). TTL popover: four number fields with the defaults shown, "Reset". The window requested from the engine is `max(ttl.*)` clamped to 7 d.

## 9 Amendments

### 9.1 Renderer (65 §0.4, 73 §0.4/§6.5/§6.10/§10, 147 §16, 19 §8)

`@xyflow/react` is admitted **for the Attention graph only** (`components/attention/graph/**`). Chat diagrams (`components/chat/surface/Graph.tsx`, ELK in a worker, 65 §0.3), CodeMap and boards keep `GraphSvg`/ELK; 73's "no force layout" and "an existing node never moves" are kept and are rules of this canvas too. The infinite canvas is the primary surface here because the subject is unbounded in count (many trees) and time, unlike a code map; it is never the only surface — the list stays, and the collapsed strip is the default under `md` (73's mobile rule, `73-codemap.md:82`). 19 §8's "no server-side derived board" is amended for this one aggregate: per 176 a client cannot assemble the forest from list RPCs without stalling the engine; the derived values are the supervisor's own (`ActivityOf`), not a second state machine.

Verified 2026-11-21 from primary sources:

| Fact | Value | Source |
|---|---|---|
| Latest | `13.0.2` (2026-11-17); `13.0.0` 2026-11-16; previous line `12.12.1` (2026-11-02) | `registry.npmjs.org/@xyflow/react` (`dist-tags`, `time`) |
| Licence | MIT ("Copyright (c) 2019-2025 webkid GmbH"); `@xyflow/system` MIT | registry `license`; `raw.githubusercontent.com/xyflow/xyflow/main/LICENSE` |
| Peers | `react`, `react-dom`, `@types/react`, `@types/react-dom` `>=18` (13) / `>=17` (12) — React 19 satisfies both | registry `peerDependencies` |
| Dependencies | 13: `zustand ^5.0.15` (the UI already has `^5.0.15`), `classcat`, `@xyflow/system 1.0.0` (d3-zoom/drag/selection/interpolate), `react-compiler-runtime ^1.0.0`; 12: `zustand ^4.4.0` (a second copy) | registry |
| Size | 13.0.2: 261.6 kB min / 88.4 kB gzip incl. deps; 12.12.1: 188.1 kB / 60.1 kB | `bundlephobia.com/api/size` |
| CSS | must import `@xyflow/react/dist/style.css` (or `base.css`); 13 wraps it in `@layer xyflow`, so Tailwind 4 utilities win without `!important` | 13.0.0 changelog (#5844, #5866); `dist/base.css` in the 13.0.2 tarball |
| v13 breaks | `useStore` → split stores; `colorMode` → `forceColorMode` (system by default); `fitView` padding in px/%; `nodeClickDistance` removed | `packages/react/CHANGELOG.md` 13.0.0 |
| API used | `ReactFlow`, `ReactFlowProvider`, `Background` (`BackgroundVariant.Dots`), `Controls`, `MiniMap` (`nodeColor`), `useReactFlow` (`setCenter`, `fitView`), `onMove`, `onNodeClick`, `onEdgeClick`, `onlyRenderVisibleElements`, `nodesDraggable`, `nodesConnectable`, `elementsSelectable`, `minZoom`, `NodeProps`, `EdgeProps`, `BaseEdge`, `getBezierPath`, `getSmoothStepPath`, `Handle`, `Position` — all present in 13.0.2's `dist/index.d.ts` and in 12.12 | tarball grep |
| Attribution | a "React Flow" link renders bottom-right; `proOptions.hideAttribution` exists; the project asks commercial users to subscribe to Pro before hiding it (not a licence term). Kept visible this round (Q1) | `reactflow.dev/learn/troubleshooting/remove-attribution` |

### 9.2 Others

- **176**: followed — one snapshot, one coalesced topic, renewals without payload.
- **207** §13 follow-up "The graph" is this note; the list gains `ListFilter.subtree`; no 207 wire change.
- **141** D9: watch-only children appear on the graph with the same eye mark and states.

## 10 Compat

- Wire: one new method, one new topic, three additive `PeerMessageChanged` fields (output adds required fields; receiver views `?:`), new definitions; the classifier must report only `Additive`/`Nonbreaking`.
- `PeerMessageChanged` loses its `Copy` derive (it gains `String`s); its one constructor (`supervisor.rs:2218`) and `sessionsync/src/events.rs:566-573` take it by value/reference already.
- Old engines: no `agent_graph` → the region is absent; the list is unchanged.
- Companions (Swift/C#) ignore the new topic and fields.
- Migration 0073 adds a local table and an index; `SESSIONS_WIRE_SCHEMA` stays 70; `store::capture::ATTACHED` (`store/capture.rs:19-40`) is unchanged.

## 11 Test plan

Engine (B): window selection (live, recent, included, ancestors, cap → `older`); kind/title/outcome mapping; `waits` → `GraphWait`; pair aggregation (directions, results, open, last); `include` + ancestors; dormant → active → lease end → new epoch; coalescing (N events in 250 ms → one delta; no delta for unchanged nodes; delta-class events throttled to 30 s); `resync` over 500; `last_event` persistence across a store reopen and the fallback; `PeerMessageChanged` from/to/result; wire fixtures, mirror, OpenRPC classifier.

UI (D): `layout.ts` (the contract's two exact cases, determinism, sticky slots, re-pack); `ttl.ts` (the contract's expected table at three instants, pinning by attention/live/descendant, ghosts, edge active/folded); `reducer.ts` (snapshot, delta, stale, gap, epoch, resync, identity preserved); `selectors.ts` (path to root, subtree ids, strip counts, attention join); `model.ts` subtree filter; components by `renderToStaticMarkup` (node at three LODs, strip text); no-emoji; the dev fixture page with the scripted simulation, seen in a browser.

## 12 Open questions for the owner

- **Q1** React Flow attribution: keep the small "React Flow" link (built), or subscribe to Pro and set `hideAttribution`?
- **Q2** `@xyflow/react` 13.0.2 is 5 days old; 12.12.1 is the mature line (60 kB gz vs 88 kB). Built on 13 with only APIs shared by both.
- **Q3** TTLs per device (client pref, built) or engine config shared by every client?
- **Q4** Roots in creation order with sticky slots (built), or most recently active first (moves trees)?
- **Q5** Private sessions (51) are excluded (built); show them?
- **Q6** The lease means a sleeping client's graph stops updating 5 min after its last renewal and resumes with a fresh snapshot; acceptable?
- **Q7** A node click filters the list immediately (built); or should the click only select and the filter be an explicit button?

## 13 Follow-ups (not in this round)

- Cross-engine live relay: the graph actor follows remote-owned sessions in the window (`sessionsync::events::follow`) or engines exchange graph deltas.
- macOS and Windows companions: a tree view from `agent_graph`.
- Drive from the graph: reply box / approve on the node bar (today it opens the session or filters the list).
- TTLs and prefs synced across devices (if Q3 says so).
- Advisor sessions as small satellites of their session.
- `StopOutcome` (roster) and `GraphOutcome` (agent) share one type once `roster` can re-export it without a wire change.
