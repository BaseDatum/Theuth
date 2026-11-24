# 214 — Routine graph and live project task graph

Status: contract (fixed before builders start). Owner request 2026-11-24.

## 0. Owner's ask

1. Routines view: opening a routine shows a **graph** of what it is and how it operates — trigger, gates, stop conditions, the run (agent/model/session), the prompt, delivery, history, next fire — as cards, a clear picture of what the routine IS.
2. Tasks view: a **project task graph** showing dependency relationships, updating **live**, that can be **popped out into its own window** (and opened as a tile beside a chat), so the owner plans a project with an agent and watches the graph grow. Clicking a task card opens an **inline editor** to update/tweak the task without leaving the graph.
3. Same graph library as the Attention view: `@xyflow/react` 13.0.2 (`components/attention/graph/Canvas.tsx`). Layout of DAGs via the existing ELK worker (`lib/graph-layout.ts` `useElkLayout`, `lib/diagram/elk.ts` `ELK_OPTIONS`, `flattenElk`). No new graph dependency.

## 1. Decisions

- D1. Dependencies are the existing `Task.blockers` (task ids; `decision:`/`person:` entries are non-task blockers). Edge `B → A` (B blocks A) when `A.blockers ∋ B`. `parent` drawn as a dashed secondary edge. **No new `depends_on` field, no migration.**
- D2. `task_create` (agent tool + `TaskCreate` RPC input) gains `blockers: Vec<String>` (`#[serde(default)]`), validated like `task_update` (unknown task ids refused, `check_cycle`). Fixes the known "create silently drops blockers" bug so a planning agent can create a dependent task in one call. `task_draft` text that says "set blockers with task_update afterwards" is updated.
- D3. New wire event, topic `tasks.changed`, VERSION 1:
  ```rust
  pub struct TaskChanged { pub id: TaskId, pub project: Option<String>, pub kind: TaskChangeKind }
  #[serde(rename_all = "snake_case")] pub enum TaskChangeKind { Created, Updated, Moved, Commented, Deleted }
  ```
  Published by the tasks ledger after every committed create / update (any field, incl. blockers, title, brief, priority, project) / move / comment / delete, including changes made by agents, by the RPC (`"user"`) and by replication-applied rows if feasible (best effort; not required). Registered like `ActivityAppended`: plugin registration, `republish!` in `crates/rpc-compose/src/client_mirror.rs`, `family::<TaskChanged>` in `wire_golden.rs` + `wire_parity_inc7.rs`, fixture `crates/rpc-compose/tests/fixtures/wire/tasks.changed.json`. TS binding generated as event `TaskChanged` under `event.tasks_2echanged.output` (exact module name is whatever `just ts-desktop` produces — UI must not hard-depend on it, see D5).
- D4. Routine graph is **frontend-only** in v1, built from `list_schedules` (`ScheduleInfo`) + `get_schedule` (`ScheduleFile`) + `schedule_history` + `list_sensors`. No new routine RPC.
- D5. The task graph refreshes by invalidating `keys.tasks` (debounced ~250 ms) on `tasks.changed@1` / `TaskChanged` **and** on `tasks.activity@1` / `ActivityAppended` (fallback for older backends). `tasks.changed@1` is NOT added to `requirements.ts` required topics (optional). The listener is a reusable hook `useLiveTasks()` in `components/tasks/queries.ts` and is also mounted by the existing Board so the board goes live too.
- D6. Graph route `/tasks/graph` (file `routes/tasks.graph.tsx`), search `{ project?: string }` (validateSearch tolerant of non-strings). Reached from a "Graph" toggle/button in the Board header (keeps current project) and pop-out via `openPopout({ kind: 'page', path: '/tasks/graph?project=…', key: 'tasks-graph-' + project })`. Also a tile kind `tasks-graph` (`TILE_KINDS`, `tiles/kinds/tasksGraph.tsx`, `open.ts` `openTasksGraphTile(host, project)`), props `{ project: string | null }`; the tile pop-out comes free via `TileFrame popOut`.
- D7. Clicking a task node selects it and opens an inline side panel (right edge of the canvas, not a route change) editing: title, brief (markdown textarea), done_when, priority, state (via `task_move`; done requires `done_because` → offer "criteria met"), owner (`OwnerSelect`), blockers (add via `TaskPicker`, remove chip), project. Saves with `api.task_update(id, patch)` / `api.task_move`. A "+ task" action creates a task in the current project (title only, optional "blocked by selected"). "Open full page" link to `/tasks/$id`. Esc / pane click closes.
- D8. Task nodes: card ~240×84: title (2 lines), state badge (colour by state), owner, priority marker, done_when ✓ hint, `#id` mono. Done/cancelled dimmed; blocked tasks with an open blocker edge marked. Non-task blockers (`decision:`/`person:`) appear as small pill nodes. Layout ELK layered `'elk.direction': 'RIGHT'`, hash = sorted ids + edges (NOT states) so a state change never moves a node; new nodes animate in (fade). Fit view on first load and when node count grows; keep viewport otherwise. Filter chips: hide done, show parent edges.
- D9. Routine graph: shown at the top of the routine **overview** section when a routine is opened (above Facts), height ~420 px, plus a full-height `graph` section is NOT added (keep SECTIONS unchanged). Left-to-right flow of cards:
  `Trigger` (clock: cron/every/at + human sentence + next fire; watcher: sensor file + args, `fire`, `for`, `cooldown`, `timeout`; manual "Run now") → `Gate` (mode skip/queue/restart, approvals, cap, budget, engine/hub, enabled/expires) → `Until check` (loop only: command, last check exit, runs / max_runs) → `Run` (agent → task, or session; model/effort, cwd, project, memory, runtime, run_timeout) with a `Prompt` card (first ~12 lines, expandable) attached → `Deliver` (quiet ⇒ only message_user; else chat channels) → `End` (done/max_runs/expired) with a loop-back edge to Trigger for repeating routines. A side `History` card with the last ~8 runs (kind, result, time; click → session) and status (running/next/last_result/error). Cards that don't apply are omitted, not greyed. Card colours by role; error/broken states use danger. Pure builder `routineGraph(info, file, history, sensors) → {nodes, edges}` with unit tests.

## 2. File ownership

- **B-ENG (omp Opus)**: `crates/tasks/**`, `crates/roster/**`, `crates/app/src/draft.rs`, `crates/rpc-compose/**`, `crates/tools/**` (only if needed). Does NOT touch `crates/desktop/**`, the openrpc fixture (orchestrator accepts it).
- **B-ROUTINE (omp Opus)**: `crates/desktop/ui/src/components/routines/**` (new `components/routines/graph/*`), `crates/desktop/ui/src/dev/routines-fixture.ts`.
- **B-TASKS (omp Opus)**: `crates/desktop/ui/src/components/tasks/**`, `crates/desktop/ui/src/routes/tasks.graph.tsx`, `crates/desktop/ui/src/routeTree.gen.ts` (generated), `crates/desktop/ui/src/components/tiles/**`, `crates/desktop/ui/src/lib/popout*` only if needed.
- Shared, read-only for both UI builders: `components/attention/graph/*`, `lib/graph-layout.ts`, `lib/diagram/elk.ts`, `styles.css` (append-only under a `/* 214 */` comment block if a builder needs classes; B-ROUTINE prefixes `.rg-`, B-TASKS prefixes `.tg-`).

## 3. Checks

- Rust: `cargo test -p tasks`, `cargo test -p roster`, `cargo test -p rpc-compose --test wire_golden`, clippy `--no-deps` on touched crates. Orchestrator: `just wire-schema-accept`, `just ts-desktop`.
- UI: `bun run check`, `bun run test` (vitest, `*.test.ts` only). Pure builders tested: `routineGraph`, task `graphModel` (edges from blockers, non-task blockers, cycle-safe, stable hash).

## 4. Amendment A1 (owner, 2026-11-24 15:20) — finished dependencies stay visible

- `Task` gains `#[serde(default)] pub resolved_blockers: Vec<TaskId>`: task ids that used to be in `blockers` and were removed because that task closed (done/cancelled). Stored (migration `crates/tasks/migrations/0010_resolved_blockers.sql`, `resolved_blockers_json TEXT NOT NULL DEFAULT '[]'`). `on_closed` appends the closing id (deduped) when it strips it from a dependent's `blockers`. A manual removal via task_update does NOT add to resolved_blockers. If a closed task is reopened, its id is not put back into blockers (unchanged behaviour). Emits `TaskChanged{Updated}` for each dependent touched.
- Graph: edge B→A drawn for `A.blockers ∋ B` (open, solid) and for `A.resolved_blockers ∋ B` (done, faded). The layout hash is built from the union of both sets, so a blocker resolving never relayouts.
- Replication: `tasks` is a replicated table — B-ENG checks whether a column add needs a replica/wire schema bump and does it.
