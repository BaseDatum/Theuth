# 104 — as built, ENG (`feat/104-eng`, base `d0f0f4be`)

Engine side of `docs/design/104-attention-inbox.md` A1, A3, A10. One code commit (`5d959923`) plus this note.

## What

- **Kinds** `AttentionKind::Ask` (`ask:<AskId>`) and `AttentionKind::Show` (`show:<call_id>`), serde `ask` / `show` — `crates/roster/src/attention.rs:57-59`.
- **`attention_shows`** — `crates/roster/migrations/0005_attention_shows.sql` (`call_id` PK, `session`, `title`, `name`, `kind`, `mime`, `bytes`, `version`, `at_ms`). The actor subscribes to `artifact::ArtifactShown` (`attention.rs:577`) and upserts a row (`record_show`, `:716`; same `call_id` → the newer row wins, `at_ms` = receive time). `shows()` (`:740`) loads rows `LEFT JOIN attention_state ON st.id = 'show:' || call_id WHERE dismissed_ms IS NULL`, newest first; snooze still applies in `derive`. No backfill.
- **Asks in gather** — `ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR)` → `tools::PendingAsks`; absent actor → none (`attention.rs:663-666`).
- **`AttentionItem`** gains `ask: Option<tools::AskRequested>` and `turn: Option<u32>` (both `#[serde(default)]`, `:89-94`). `session` is now filled for every kind that has one, `agent` falls back to the desk the session belongs to (`:79-82`):
  - approvals: `session` as before, `agent` = desk (`:231-235`);
  - asks / shows: from the request / row (`:246-305`);
  - hand-offs / secrets: from their rows (secrets also get the desk `agent` and the desk's title in `who`, `:506-516`);
  - decision / blocked / review: `task.checkout.session`, else `Inputs.task_sessions[task]` = the newest `task_events` row with a session, fetched in `gather` via the existing `tasks::TaskEvents(TaskId)` message for open tasks without a checkout (`:316-321`, `:643-660`);
  - hires: the proposing agent's open desk session (`proposed_by = agent:<n>` → `AgentSummary.desk`), else `None` (`:403-414`).
- **Desk resolution** — `Inputs.desks: HashMap<session, agent>` seeded from `AgentSummary.desk` and extended by `desk_map(&agent::ListSessions)` (`:697`), which walks `SessionSummary.parent` up to a session with `desk: Some(_)` (bounded 32 hops). Computed once per `gather`.
- **Items** — Ask: `"<who> asks: <head ≤ 140 of first question>"`, detail `"<n> questions"` when > 1 else the first question's option labels joined by ` · ` (≤ 140, `None` when empty), `at_ms = now`, `ask: Some(req)`. Show: `"<who> showed you <title>"`, detail `"<kind> · <bytes> · v<version>"` (`human_bytes`, `:542`: `12 B`, `1.2 MB`), `at_ms` from the row. `who` = agent title, else session short id (the 79 §6.6 closure, moved above the approvals loop, `:216-222`).
- **Ordering / clearing** unchanged: newest first, `attention_state` dismiss/snooze keyed by the new ids. An answered ask leaves `PendingAsks`; a dismissed show stays dismissed when re-shown under the same `call_id`.
- `crates/roster/src/lib.rs` untouched: the subscription lives in `Attention::start`, which `lib.rs:2139` already calls. No RPC method added (`crates/rpc/src/api.rs` untouched).

## Deviations

- `tools::AskRequested` now derives `PartialEq, Eq` (`crates/tools/src/ask.rs:77`) so `AttentionItem` keeps its `PartialEq`. One-line change in a crate the brief did not list as mine or as forbidden; no behaviour change.
- `turn` is always `None`: neither `ApprovalRequested`, `AskRequested` nor `ArtifactShown`/`Artifact` carries a turn. The field exists so the wire shape is final; fill it when a source grows one.
- Hire `session` = the proposing agent's *open desk*, not "the proposing session": the `proposals` table (`0001_roster.sql`) stores `proposed_by` (agent name) and `approval_id`, no session. `None` when the proposer has no desk open.
- `roster` gains a dependency on `artifact` (for the `ArtifactShown` type). `artifact` does not depend on `roster`; `cargo check --workspace` is green.
- Approval titles still say `<session uuid> wants to run <tool>` (not in the brief; the UI owns display per A5/A6).

## A10 — events the UI must add to `ATTENTION_EVENTS`

`crates/desktop/ui/src/components/hud/queries.ts:22` currently lists `ActivityAppended, ApprovalRequested, ApprovalResolved, NodeAdded, NodeOnline, NodeRemoved, NodeRevoked, SecretRequested, SecretResolved, Takeover`. Add: **`AskRequested`**, **`AskResolved`** (the ask-answered event, `crates/tools/src/ask.rs:94`), **`ArtifactShown`**. All three already reach the desktop (`crates/rpc/src/bridge.rs:168`, `crates/desktop/ui/src/rpc/events.ts`). Nothing new on the wire.

## Tests (`attention::tests`, `crates/roster/src/attention.rs:1396-1581`)

- `asks_and_shows_become_items_with_their_session_and_desk` — ask title/detail/session/agent/`ask`, multi-question detail, non-desk short id, show title/detail/session/agent, ordering, dismissed show + snoozed ask hidden.
- `task_items_carry_the_working_session` — `checkout.session`, `task_sessions` fallback, `None`.
- `desk_map_walks_to_the_desk_root` — grandchild of a desk resolves; a loose session does not.
- `shows_roundtrip_latest_version_wins_and_dismiss_hides` — `record_show` ×2 same `call_id` → one row, v2; `set_state(dismiss)` hides it from `shows()`. Runs the migration on a temp store.
- Existing tests untouched and green (`derives_every_kind_and_hides_dismissed_or_snoozed`, `decision_and_review_items_carry_their_packet`, `handoffs_and_secret_requests_become_items`, `state_roundtrip`).

## Gates (run in `~/Develop/theuth-wt-104-eng`, logs in `/tmp/mc-2026-10-05/104-eng-{test,check,ts}.log`)

```
export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
cargo test -p roster -p rpc --no-fail-fast   # roster: 55 passed, 0 failed; rpc: 36 passed, 0 failed; prompts doctest: 1 passed — EXIT 0
cargo check --workspace                       # EXIT 0; 1 warning (pre-existing future-incompat note for `block v0.1.6`, not ours)
just ts                                       # 8 passed, 0 failed — EXIT 0
```

Generated bindings (`bindings/`, gitignored):

```ts
// bindings/AttentionKind.ts
export type AttentionKind = "approval" | "decision" | "hire" | "blocked" | "review" | "budget" | "node" | "handoff" | "secret" | "ask" | "show";

// bindings/AttentionItem.ts (new / changed lines)
import type { AskRequested } from "./AskRequested";
agent: string | null,
session: string | null,
ask: AskRequested | null,
turn: number | null, };
```

## Not verified by test

- The live `ArtifactShown` subscription (`rt.subscribe::<Self, ArtifactShown>` → `Message<ArtifactShown>` handler): needs a running `Runtime`; the insert + load + derive path is covered with a hand-built event instead, as the brief allows.
- `gather`'s live lookups (`PendingAsks`, `ListSessions`, `TaskEvents`): actor-backed, exercised only through the pure `Inputs` fixtures.
