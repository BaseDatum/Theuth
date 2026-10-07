# 132 — ATTENTION-ROWS-AND-ORDER

Status: contract, 2026-10-07 18:45, written by the orchestrator off origin/main `1354b6d8` (v2.12.0). Source: the agentbox recon (`~/.theuth/briefs/2026-10-07/agentbox/`, knowledge `facts/theuth-borrow-list-from-agentbox-attention-2026-10-07`); techniques only, nothing copied. `UI/` = `crates/desktop/ui/src/`. Neighbours: 130 COMPANION (built; W6 fixes in flight in `crates/desktop/companion` — not touched here), 128 HUD-SCENE (`/`, not touched), 131 RUNTIME-RESET (landed). Two workers: ENG (every `.rs` + `board.md`) and UI (`UI/**`); the orchestrator integrates.

## 0. Decisions in one screen

| # | Decision | Owner |
|---|---|---|
| D1 | A decision row leads with the question, sentence-clipped to 112 chars; a review row leads with the outcome (first sentence of the summary); the engine-composed "‹who› asks for a decision on …" becomes the byline in `detail` | ENG derive · UI Row |
| D2 | `Packet.if_wrong` (≤ 200 chars): one honest line on the cost of a wrong answer; REQUIRED for decisions to the user (refusal is the recipe), optional on reviews; inherited on forward; mirrored in the thread; shown above the options | ENG · UI |
| D3 | Review packets may carry `options` = the next moves after approval (≤ 4, one recommended); the Review screen offers them; approving with a pick writes `**next** ‹label›` to the thread and wakes the owner with `Answer` | ENG · UI |
| D4 | Tasks gain `priority` (urgent · high · medium · low, default medium) that only the user may set; `tasks.project_order` in config; one `rank()` orders the inbox AND an agent's wake dispatch; Urgent items form their own group at the top of the rail | ENG · UI |
| D5 | Dedupe before you file: a second escalation to the user by the same actor on a task already blocked on a pending user decision is refused with `ALREADY_WAITING`; new read-only tool `task_waiting_on_user` lists what of the caller's line already waits on the user | ENG |
| D6 | A late `answer_ask` (question no longer pending) is delivered to the session as a user message instead of being dropped; the RPC still returns `false`; the toast says where the answer went | ENG · UI toast |
| D7 | The tray `●N` counts attention items, the same number as the sidebar badge | UI |
| D8 | Budget and hire items carry their real time (budget crossing, proposal time) instead of `now_ms` | ENG |
| D9 | Teaching text (desk manual, tool descriptions, board skill): line one is the thing to say back; options answer it in the same words; changed anything → something to decide; never offer what the run already did; reading changes nothing; retitle on hand-over; show what it does; stuck is the bar; `if_wrong` | ENG |
| D10 | A `?mock=crowded` Attention world (60 items, every kind, every priority) and model tests for grouping/ordering | UI |

## 1. Problem, with evidence

- The inbox row is written by the engine, not by the agent: "‹by› asks for a decision on “‹task›”" with `head()` of the question as detail (`crates/roster/src/attention.rs:302-306`; `crates/tasks/src/packet.rs:208-211`, `:646-660`). The person reads the byline first and the ask second. agentbox's rule — line one is the thing you have to say back, the options are its answers in the same words — makes a row decidable without opening it.
- Ordering is newest-first only (`attention.rs:696`); the rail re-sorts newest-first inside each kind (`UI/components/attention/model.ts:26-31`); there is no priority field and the run queue (oldest unclaimed wake per agent, `crates/tasks/src/ledger.rs:1763-1800`) knows nothing of the inbox. Sky's live inbox held 40 rows, the queue 51 (`docs/design/115-tasks-queue-collapse.md:13`). agentbox keeps ONE score for both, imported by the inbox and the scheduler, with the user's tag the only one that counts.
- Reviews end in Approve / Send back (`UI/components/attention/ReviewScreen.tsx:110-124`); the next move has to be typed. agentbox: "if your run changed anything there is something to decide" — a review's options are the next moves, one keystroke.
- A packet says why it matters but never what a wrong answer costs; the decider has no automation-level cue.
- A second escalation on a task already waiting on the user is accepted when the same actor raises it (`ledger.rs:1522-1545` only treats another actor's packet as a parent); a desk cannot see what already waits (no attention tool for agents).
- `answer_ask` on a closed question returns `false` and the typed answer is lost (`crates/rpc/src/api.rs:604`; UI toast).
- Tray badge counts approvals, sidebar badge counts items (`UI/App.tsx:420-427`).

## 2. Contract (frozen signatures)

### 2.1 Rust — `crates/tasks`
```rust
// types.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Priority { Urgent, High, #[default] Medium, Low }
impl Priority {
    /// agentbox weights: 9 / 7 / 5 / 2.
    pub const fn weight(self) -> u32;
    pub const fn as_str(self) -> &'static str;   // column value
    pub fn parse(s: &str) -> Option<Self>;
}
// Task gains:  #[serde(default)] pub priority: Priority,
// TaskCreate gains: #[serde(default)] pub priority: Option<Priority>,
// TaskPatch  gains: #[serde(default)] pub priority: Option<Priority>,   // user only (ledger refuses agents)

// rank.rs (new)
pub const RANK_STEP: u32 = 100;
/// `len - index` for a project in `order`, 0 for an unranked or absent project.
pub fn project_score(project: Option<&str>, order: &[String]) -> u32;
/// `project_score * RANK_STEP + priority.weight()`; higher sooner.
pub fn rank(priority: Priority, project: Option<&str>, order: &[String]) -> u32;

// config.rs
// TasksConfig gains: #[serde(default)] pub project_order: Vec<String>,   // config.yaml `tasks.project_order`

// packet.rs
pub const IF_WRONG_MAX: usize = 200;
pub const ROW_BUDGET: usize = 112;
pub const NEEDS_IF_WRONG: &str = /* "escalating to the user needs `if_wrong`: one honest line on the cost of a wrong answer — \"low, one-line revert\" or \"high, real users get this email\"" */;
/// `text` (first non-blank line, `#` stripped) cut to `budget` on a sentence end (`. ` `? ` `! `) when one lies past
/// char 40, else on the last space past char 40 with `…`; shorter text verbatim.
pub fn head_sentence(text: &str, budget: usize) -> String;
// Packet gains: #[serde(default)] pub if_wrong: Option<String>,
// Ask (escalate args) gains: pub if_wrong: Option<String>,
// validate(): if_wrong ≤ IF_WRONG_MAX chars after trim (error names the field and the cap).
// require_for_user(): Decision without a non-blank `if_wrong` → Err(NEEDS_IF_WRONG) (checked after board + summary).
// Review packets: `options` (≤ 4 here, labels ≤ 80) and `recommended` are accepted by the review gate; validate() caps review options at 4.

// ledger.rs
pub const ALREADY_WAITING: &str = /* "decision:<id> is already waiting on the user for this task — add what you learned with `task_note` and go on with work the answer does not gate; a new escalation needs a new option, a fact that changes the decision, or a deadline" (the id is interpolated) */;
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Waiting { pub task: TaskId, pub title: String, pub kind: packet::PacketKind, pub key: String,
                     #[cfg_attr(feature = "ts", ts(type = "number"))] pub since_ms: u64, pub head: String }
/// Decisions and reviews addressed to the user on tasks in `actor`'s reporting line (every task for the user); oldest first.
pub async fn waiting_on_user(&mut self, actor: &str) -> R<Vec<Waiting>>;
// escalate(): before writing, if the task has a pending `decision:` packet with `by == actor` addressed to the user → Err(ALREADY_WAITING).
// task_move(): `next: Option<u32>` — valid only when state == done from review and the review packet has options; writes
//   `**next** (<n+1>) <label>` to the thread and wakes the owner with WakeReason::Answer; out-of-range → error naming the count.
// create()/patch(): `priority` from a non-user actor → Err("priority is the user's to set; say why in task_note and the user decides").
// dispatch(): among an agent's unclaimed, non-backed-off wakes pick by (rank desc, Answer|BlockersResolved first, oldest first).

// tools.rs — new tool
// name "task_waiting_on_user", RISK ReadOnly, args { project: Option<String> }, returns Vec<Waiting> rendered one line each:
//   "<kind> <key> · <title> · waiting <age> · <head>"; empty → "nothing of yours is waiting on the user".
// TaskMoveArgs gains `next: Option<u32>`; task_create/task_update tool args gain `priority: Option<Priority>`.
```

### 2.2 Rust — `crates/roster/src/attention.rs`
```rust
// AttentionItem gains:
#[serde(default)] #[cfg_attr(feature = "ts", ts(optional = nullable))] pub priority: Option<tasks::Priority>,  // task-born items and session items whose session is a task checkout
#[serde(default)] pub rank: u32,   // tasks::rank::rank(...) for task-born items; Priority::Medium.weight() (5) otherwise
// derive(): decision title = head_sentence(question, ROW_BUDGET), detail = "<who> · <task title>"; review title =
//   head_sentence(summary, ROW_BUDGET), detail = "“<task title>” · by <who>"; legacy items without a packet keep today's strings.
// derive(): final order = rank desc, then waiting_since_ms asc (None last), then at_ms desc.   (replaces :696)
// budget items: at_ms/waiting_since = the crossing time when the ledger has it; hire items: the proposal time. `now_ms` only as the last fallback.
```

### 2.3 Rust — `crates/tools/src/ask.rs` (+ the minimal agent-crate hook it needs)
A `Settle`/answer for an id that is no longer pending, with `Some(answers)`: look the question up in the journal (`questions` table, 109), build `Answer to your earlier question “<q>”: <a>` (one line per question), hand it to the session as a user message through the same path the chat `prompt` uses (additive: one message type on the supervisor if none exists), return `false`. No answers (`None`) → unchanged.

### 2.4 TS contract (UI writes these stubs into the gitignored `UI/rpc/bindings/`, never commits them; `just ts-desktop` regenerates)
```ts
// bindings/Priority.ts
export type Priority = "urgent" | "high" | "medium" | "low";
// bindings/Task.ts           gains  priority: Priority;
// bindings/TaskCreate.ts     gains  priority?: Priority | null;
// bindings/TaskPatch.ts      gains  priority?: Priority | null;
// bindings/Packet.ts         gains  if_wrong?: string | null;
// bindings/AttentionItem.ts  gains  priority?: Priority | null;  rank: number;
// bindings/Waiting.ts        (tool result; UI does not use it)
// task_move RPC params gain  next?: number | null   (hand-written in UI/rpc/api.ts + METHODS stays unchanged: same method)
```

## 3. Work split (files)
**ENG owns:** `crates/tasks/**` (incl. a new migration `0008_priority.sql`: `ALTER TABLE tasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'medium'`), `crates/roster/src/attention.rs`, `crates/roster/src/desk.rs` (manual text), `crates/tools/src/ask.rs`, `crates/agent/**` only for the additive hook of D6, `crates/rpc/src/api.rs` only where `task_move` gains `next` and the ts export list gains `Priority`/`Waiting`, `crates/document/board.md` (skill text), `docs/design/132-as-built-ENG.md`. Gates: `cargo test -p tasks -p roster -p tools`, `cargo clippy -p tasks -p roster -p tools`, `cargo test -p rpc --features ts --lib export_bindings` (bindings regenerate; nothing committed from them). MUST NOT touch `UI/**`, `crates/desktop/companion/**`, `crates/desktop/src/**`.
**UI owns:** `UI/components/attention/**`, `UI/components/hud/queries.ts`, `UI/components/hud/Hud.tsx` only for the KIND_LABEL/urgent chip if needed, `UI/App.tsx` (tray badge lines only), `UI/rpc/api.ts` (`task_move` `next` param), `UI/components/settings/sandbox-queries.ts` (the crowded world), `docs/design/132-as-built-UI.md`. Hand-writes the §2.4 stubs into the gitignored bindings dir. Gates: `bun run check` (tsc), `bunx vitest run src/components/attention src/components/hud src/rpc`, `bun run build`. MUST NOT touch any `.rs`, `board.md`, `companion/**`.
**Orchestrator:** merges UI then ENG into `feat/132-attention-rows`, folds `origin/main`, runs `just ts-desktop`, re-runs tsc/vite/vitest and the three crate tests on the merged tree, lands with `merge --no-ff`, pushes origin + basedatum.

## 4. Merge rules
- One owner per file (above). The only shared seam is the TS contract (§2.4): UI's stubs are gitignored; the regenerated bindings are the truth at integration. A name mismatch is ENG's to fix (the contract is this file).
- `UI/rpc/api.ts`: `task_move` keeps its method name; the UI adds the optional `next` param and tolerates an engine that ignores it (older engine) — no -32601 handling needed.
- Row text: ENG decides the strings (D1); the UI renders `title` then `detail`, nothing composed client-side.

## 5. Proof list (the orchestrator checks these on the merged tree)
1. `cargo test -p tasks`: `head_sentence` (short verbatim; clip at a sentence end past 40; word clip with …; `#` stripped); `rank` (unranked 0; ranked beats unranked; project step dominates priority; weights 9/7/5/2); `escalate` refuses the same actor's second escalation with `ALREADY_WAITING` and still allows another actor's forward; `if_wrong` required for Decision-to-user, optional for Review, inherited on forward, over-cap refused before writing; review `options` accepted (≤ 4) and `task_move done next=i` writes the thread line and wakes the owner with Answer; `priority` from `agent:x` refused, from `user` accepted; `dispatch` picks the higher-ranked task's wake first and Answer-class before older Assigned on equal rank; `waiting_on_user` lists only the caller's line.
2. `cargo test -p roster`: decision row title is the clipped question and detail the byline; review row title the outcome; order rank desc → waiting_since asc → at_ms desc; `priority`/`rank` filled; budget/hire times not `now_ms` when known.
3. `cargo test -p tools`: a late answer builds the message text and returns false; `None` unchanged.
4. UI: `model.test.ts` — Urgent group first across kinds, groups keep engine order (no client re-sort by at_ms), crowded world renders 60 rows; Decision screen shows `If wrong` above the options when present; Review screen renders next-move options with the recommended pre-picked and `Approve → <label>` sending `next`; tray badge = attention count; late-answer toast text.
5. `just ts-desktop` → tsc 0 · vite ok · vitest green on `attention`, `hud`, `rpc`.

## 6. Out of scope (recorded, not built)
Urgent pre-emption of running children; a UI for `project_order` (config key only); companion-side undo/advance/banner rules (handed to 130 W6: `~/Develop/theuth-130-briefs/input-from-agentbox-recon-for-W6.md`); channel cards (unchanged since 101); `if_wrong` as a board section (the field is enough).
