# 70 — As built: HUD-ATTENTION (the THEUTH mark opens an overview; Attention inbox + badge; activity feed)

*Branch `feat/hud-attention-activity`, worktree `theuth-hud`, 2026-09-26. Five commits.*

## Per commit

### 1. `tasks: activity log — append-only feed tailed from task_events, roster/schedule hooks, ActivityAppended on the bus`
- `crates/tasks/migrations/0003_activity.sql`: `activity(id, ts_ms, kind, actor, subject, summary, cost)` + `activity_cursor(id=1, last_event)`. Engine-local (not attached to replica).
- `crates/tasks/src/activity.rs` (new): actor `Activity` registered as `tasks::activity::ACTIVITY_ACTOR = "activity"`.
  - **Feed 1 — task events, no ledger edits.** Every task mutation already writes `task_events` (created/state/owner/checkout/comment/blockers/…), including ledger-internal ones (orphan sweep, episode-closing blocks, schedule creates). The actor tails that table every 1 s from `activity_cursor` (500 rows/pass) and maps rows with `from_task_event(title, parent, &TaskEvent) -> Option<(ActivityKind, summary)>`: `created` → `task_created` / `child_spawned` (parent set); `state` → `task_moved`, `task_done`, `child_returned`; `owner` → `task_reassigned`; `checkout` → `task_checked_out`; `comment` → `task_commented`; `blockers` gaining / losing a `decision:` → `escalation_raised` / `escalation_answered`; `nudge`|`nudged` → `task_nudged`; `note`, `reviewer`, `where`, `harness`, `title`, `disposition` → not feed. A store that predates the feed starts at its head (no history backfill).
  - **Feed 2 — `activity::Record` bus message** (`record(rt, kind, actor, subject, summary)` one-liner) for events with no task row: roster publishes `agent_proposed`, `agent_hired`, `agent_paused` (both directions), `agent_removed`, `budget_threshold`.
  - **Feed 3 — `schedule::ScheduleEvent::Fired`** subscribed directly → `routine_fired` (subject = schedule name).
  - Every appended row is published as `ActivityAppended { entry }` (`#[serde(tag="type")]`, forwarded by `rpc::bridge`).
  - `ListActivity(ActivityFilter { since_ms?, kinds?, limit? })` → newest first, default 50, max 500; `kinds` becomes `kind IN (…)`.
- `crates/tasks/src/lib.rs`: `pub mod activity;` + `Activity::start(rt, &store)` after the tasks actor registers (3 lines).
- `crates/roster/src/lib.rs`: five one-line `tasks::activity::record(...)` calls (in `charge` at the threshold, `PauseAgent`, `ApproveAgent`, `RemoveAgent`, `ProposeAgent`).
- `crates/rpc/src/bridge.rs`: `ActivityAppended` in `forward!` and `subscribe`.

### 2. `roster: attention inbox — derived items, snooze/dismiss state; rpc: activity_list, attention_*, task_decide`
- `crates/roster/migrations/0003_attention.sql`: `attention_state(id, dismissed_ms, snoozed_until_ms)`, engine-local.
- `crates/roster/src/attention.rs` (new): actor `Attention` as `roster::attention::ATTENTION_ACTOR = "attention"`, subscribed to `tools::ApprovalRequested` / `ApprovalResolved` (keeps the pending permission prompts; the `Approval` actor stores only oneshots, so nobody else has the list).
  - `derive(&Inputs, &states, now_ms) -> Vec<AttentionItem>` — pure. Kinds: `approval` (pending tool call; detail = `args.command` when present), `decision` (task `blocked` with a `decision:` blocker; id `decision:<task>:<decision>`), `blocked` (task `blocked` on a person / task, detail = blockers), `review` (task `review` with `reviewer == user`), `hire` (agent `proposed` — one item per agent; the open `Hire agent <name>` task is linked, not listed twice), `budget` (monthly budget, `spent/amount ≥ 0.8`; detail says paused at cap vs 80 %), `node` (mesh peer `reachable == false`). Newest first by `at_ms`; dismissed and live-snoozed removed; an expired snooze returns.
  - `Inputs` gathered per `ListAttention`: `tasks::ListTasks` (todo/doing/review/blocked), `roster::ListAgents`, pending approvals, `mesh::Status.peers` — each `unwrap_or_default` when its plugin is down.
  - `SnoozeAttention { id, until_ms }` / `DismissAttention(id)` upsert `attention_state`.
- `crates/rpc/src/api.rs`: `activity_list(filter)`, `attention_list()`, `attention_snooze(id, until_ms)`, `attention_dismiss(id)`, **`task_decide(decision, answer)`** (→ `tasks::AnswerDecision` as `user`; needed for the inline decision — no user path existed without a chat session). `attention_actor()` helper.
- `crates/rpc/src/export.rs`: `ActivityKind`, `ActivityEntry`, `ActivityFilter`, `ActivityAppended`, `AttentionKind`, `AttentionItem`.
- `crates/roster/Cargo.toml`: `tempfile = "3"` dev-dependency (store round-trip test).

### 3. `desktop: HUD at / …; New chat moves to /new; /attention view`
- `routes/index.tsx` → `components/hud/Hud.tsx`. New chat (`NewChat`, draft-pane ruling, `?project=`) is byte-identical at **`routes/new.tsx`**; `openNewChat` (single pane → whole window), `/new` slash command, Knowledge "Ask theuth…" and the notes ChatPicker now navigate to `/new`. Session delete / link / settings-back still go to `/` (= the overview).
- `Hud.tsx` — Sky's order: headline row (agents active · tasks in progress · month spend · pending attention; the first two link to `/agents` and `/tasks?state=doing`) · **Attention** cards with the decision inline (approval: Allow/Deny → `resolve_approval`; decision: text + Answer → `task_decide`; hire: Hire/Decline → `agent_approve` / `agent_remove`; review: Move done / Send back → `task_move done|doing`; blocked: Unblock → `task_move todo`; budget: Un-pause → `agent_pause false`; every card: Snooze (09:00 tomorrow) / Dismiss) · **In flight** (`doing` tasks longest-running first: agent · task · session@engine · elapsed since `checkout.progress_at_ms`; running routines) · **Board pulse** chips per state → `/tasks?state=…&project=` (all projects) · **Activity** (last 50, live: `ActivityAppended` prepends into the query cache and marks task / roster lists stale) · **Spend** (per agent vs budget, bar turns warning at 80 %, danger at cap) with the **mesh chip** (reachable/total nodes, hub here/elsewhere → Settings › Mesh) · the **Composer** docked at the bottom (same component as New chat, `session=null`; sending navigates to `/c/$session` as today).
- Queries run in parallel (one `useQuery` per section, skeletons per section, per-section error text); each section has an empty state saying what will appear.
- `components/hud/model.ts` (pure): `headline`, `badgeCount`, `appendActivity` (dedupe by id, order by id, cap 50), `inFlight`, `runningRoutines`, `boardPulse`, `spend`, `snoozeUntil`. `queries.ts`: `useAttention` (re-derived on `ActivityAppended`, `ApprovalRequested/Resolved`, `Node*`; 60 s refetch so expired snoozes / budgets move), `useActivity`, `useAttentionActions`.
- `routes/attention.tsx` → `components/hud/AttentionView.tsx`: the inbox on its own (same `AttentionList`).
- `rpc/api.ts`: five `Api` methods + `METHODS` entries (sorted; `api.test.ts` matches `bindings/methods.json`). `rpc/events.ts`: `ActivityAppended` in `WireEvent`.

### 4. `desktop: Attention sidebar row with badge`
- `AppSidebar.tsx`: three lines — `Bell` in the lucide import, `import { AttentionBadge }`, one row under Routines: `<Link to="/attention"><Bell /> Attention<AttentionBadge /></Link>`. `components/hud/AttentionBadge.tsx` renders the count (hidden at zero) from `useAttention`.

### 5. this report (`docs/design/70-as-built-HUD-ATTENTION.md`).

## Deviations from the brief
- **Hooks are a tail of `task_events`, not calls inside the ledger.** `ledger.rs` is off-limits and DESK-PROTOCOL is editing tasks; tailing the field-level log the ledger already writes catches every task path (including sweep / episode-closing / schedule) with zero edits to their files. Cost: up to 1 s latency on the live feed; a fresh cursor starts at the head (no backfill).
- **Nudges**: DESK-PROTOCOL's field name is unknown; `task_nudged` is keyed on `field == "nudge" | "nudged"`. Adjust in `from_task_event` when their commit lands (one match arm).
- **`cost?`** is carried in the schema and `Record`, but no feed today sets it (child cost is not on the task record). `child_returned` is derived from a parented task reaching `done`.
- **Agent hired/paused/removed/proposed and budget thresholds** are five one-line `record()` calls in `roster/src/lib.rs` (disclosed; not desk.rs). They ride the bus, not a direct actor ref.
- **Attention lives in `crates/roster`** (has the sqlite file, depends on tasks/mesh/tools); a new crate was avoided. Permission prompts come from subscribing to `ApprovalRequested/Resolved` — pending approvals are lost on engine restart, as they are in the `Approval` actor itself.
- **Decision items are every task blocked on a `decision:`**, not only those whose manager is the user: the escalation question lives in the thread document, and the user is the top of every line. The card links to the task; the answer is a free-text input (`task_decide`, new RPC). `AskRequested` cards are not duplicated into attention (they already render in chat).
- **Hire vs proposed** collapsed into one `hire` item per proposed agent (the hire task is the approval; listing both would double it). User-owned `Hire agent …` tasks in `todo` are therefore not separately listed.
- **Owner-offline sessions** are not derived (no session→engine owner field in stage 1); unreachable mesh peers are.
- **Badge/list semantics**: the engine filters dismissed and live-snoozed items, so `attention_list().length` is the badge; snoozed items are not returned (no "snoozed" tab — YAGNI).
- **New chat moved to `/new`** since `/` is the HUD. The sidebar's New chat `href` still says `/` (its onClick calls `openNewChat`, which now goes to `/new`); left untouched to keep the AppSidebar diff to the three lines UI-FIXES-2 asked for.
- Migrations are `0003_*.sql` in both crates; if DESK-PROTOCOL also adds a `0003`, sqlx needs distinct versions — rename mine to `0004`.
- Paths: the brief's header said `/tmp/67/…` and `67-as-built`; its Build/Verify line says `/tmp/69/report-…` and `70-as-built-…` — the specific line wins. State file at `/tmp/69/state-HUD-ATTENTION.md`.
- No `Composer*`, `components/agents/**`, `Board.tsx`, `TaskDetail.tsx`, `desk.rs`, `ledger.rs` touched.

## Not built
- Browser smoke against `t --headless` (a full desktop/engine build was not run in this worktree; the UI was verified by `tsc`, vitest and `vite build` only).
- Per-child cost on `child_returned`; a snoozed-items view; 14-day run-health charts (paperclip) — out of brief.

## Tests written
- `crates/tasks/src/activity.rs`: `task_events_map_to_kinds` (every mapping arm + a summary), `tail_appends_once_and_list_filters` (tempfile store: head-start, tail, cursor advance, `Record`-style append, `kinds` / `since_ms` / `limit` filters).
- `crates/roster/src/attention.rs`: `derives_every_kind_and_hides_dismissed_or_snoozed` (all seven kinds, order, hire dedupe, decision id, budget ranking, dismiss/snooze/expired snooze), `state_roundtrip` (tempfile store upserts).
- `crates/desktop/ui/src/components/hud/model.test.ts`: headline numbers, badge count, activity reducer (prepend/dedupe/order/cap), in-flight ordering, board pulse, spend ranking, snooze presets — 7 tests.

## Verification run
- `cargo check -p tasks -p roster -p rpc --tests` → `Finished dev … in 8m 08s`, 0 errors (missing-docs warnings fixed afterwards).
- `cargo test -p tasks --lib -- activity` → 2 passed; `cargo test -p roster --lib -- attention` → 2 passed.
- `just ts-desktop` (`cargo test -p rpc --features ts export_bindings`) → 5 passed; bindings include `Activity*`, `Attention*`; `methods.json` lists `activity_list`, `attention_dismiss`, `attention_list`, `attention_snooze`, `task_decide`.
- `bun install` (ui + packages/apps), `bun run check` (`tsc -b --noEmit`) → 0; `bun run test` → 71 files, 420 tests passed; `bunx vite build` → exit 0.
- `cargo check --workspace --tests` → one pre-existing error, not mine: `crates/tui/src/model.rs:12883` builds a `schedule::ScheduleInfo` literal missing `agent` and `project` (fields another 67 worker added; `tui` is not in my allow-list). Every other crate, lib and test target checks clean (`EXIT 101` only from `tui (lib test)`).

DONE HUD-ATTENTION
