# 149 — COMPANION-NATIVE — as built (W0, engine: E6 `AttentionItem.context`, `feat/149-w0-engine`)

Base `77fa225f` (integrate/149). Contract: `149-companion-native.md` §6, Revision 1 row 5. Crates touched: `roster`,
`agent`, plus the two literal `AttentionItem` constructors in `autoresearch` and `double` (`context: None`). No schema
change, no migration. Bindings regenerated locally (gitignored, not committed). Rust only; no Swift/UI file touched.

## What landed

### E6 `AttentionItem.context`
- `crates/roster/src/attention.rs:122-127` `AttentionItem.context: Option<ItemContext>` (`#[serde(default)]`, TS
  `context?: ItemContext | null`), placed after `next_steps` as §6 specifies. `:139-151` `ItemContext { request:
  Option<String>, last_reply: Option<String> }` (Revision 1 row 5 names; `Clone, Debug, Default, PartialEq, Eq, Serialize,
  Deserialize, JsonSchema` + ts-rs export). `:154` `CONTEXT_MAX = 280` (the brief's ~280; the design's 160 is the
  companion's render cap — the Swift/C# bubble clips to 160 itself).
- `Inputs.session_context: HashMap<String, ItemContext>` (`:317-319`; keyed by the session id string like its
  neighbours `desks` / `task_sessions`, so `derive` copies by `item.session` without a parse).
- `derive` (`:817-825`) stays pure: one post-pass copies `session_context[session]` onto items whose kind is in
  `context_kind` (`:850`) — Ask, Approval, Finished, Resume, Blocked. An all-`None` `ItemContext` is not attached
  (`context: None`, never `{}`). Show / Secret / Handoff / Decision / Review / Hire / Budget / Node / Double never carry it.
- `gather()` (`:1136-1174`): the wanted session set = pending asks + pending approvals + `attention_finished` rows +
  resumes (reports and `OpenTurns` under `resume: ask`) + Blocked tasks' checkout session or `task_sessions` entry; one
  `agent::DurableStore` ask, then one `Store::last_exchange(id)` per session; a read error is a `warn!` and the item simply
  has no bubbles. `clip_context` (`:864`) drops ``` fenced blocks, collapses whitespace to one line, and cuts with
  `tasks::packet::head_sentence(_, CONTEXT_MAX)` (sentence end past char 40, else a word, `…`).
- `crates/agent/src/store.rs:2981 Store::last_exchange(id) -> (Option<String>, String)` — the READ-ONLY accessor: the
  last non-note user row (vision / advisory notes skipped, as `last_turn_text` already did) with `clock::strip` and
  `attach::parse(..).body` applied (a markers-only input names its first attachment), and the assistant text after it
  joined by `\n`. `last_turn_text` (`:2973`, what `AgentInfo.last` reads) now delegates to it — same rows, same answer.
  **The last user input IS retrievable without a schema change**, so `request` is filled; the §6 "ship `request: None`"
  clause was not needed.
- Caveat (not a bug, named): the "last reply" is the assistant text after the last user input. For an Ask / Approval
  item that is the text the agent wrote *in the turn that raised the item* (before the question); for a Finished item
  it is the finished turn's answer, which overlaps `summary` (first line) — the bubble is the longer, sentence-clipped
  form. For a Resume item on a turn with no assistant text yet, `last_reply` is `None`.

### Tests
- `attention.rs:1817 derives_every_kind_and_hides_dismissed_or_snoozed` — the `:1707` case extended: the `migrate`
  Blocked task's `task_sessions` entry names `s-blocked`, whose exchange is in `session_context`; the Blocked item carries
  the request and the Decision item (task-born) carries `None`. Order and hide assertions unchanged.
- `attention.rs:2983 context_lands_on_session_born_kinds_only` — one session with an ask, an approval, a finished row,
  a resume and a show: empty `session_context` → all five `None`; filled → Ask/Approval/Finished/Resume equal the
  context, Show `None`; an all-`None` context → `None`.
- `attention.rs:3060 clip_context_drops_fences_and_clips_on_a_sentence` — fences dropped and lines joined; code-only →
  `None`; empty → `None`; a 300-char text clips ≤ 280 on a sentence or word.
- `store.rs:4241 last_turn_text_joins_assistant_text_after_last_user` — the last user row now carries a `<time …/>`
  stamp and an `[attached note …]` marker; `last_exchange` returns `(Some("task"), "first\nsecond")` and
  `last_turn_text` is unchanged.

### Bindings
- `BUILD_SLOTS=1 BUILD_LABEL=149-W0 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 export
  tests ok (`/tmp/149-W0-bindings.log`); copied to `crates/desktop/ui/src/rpc/bindings/` (796 files). New:
  `ItemContext.ts` (`request?: string | null`, `last_reply?: string | null`); changed: `AttentionItem.ts` (import +
  `context?: ItemContext | null` after `next_steps`). `grep -rl ItemContext bindings` = exactly those two files; neither
  bindings tree is tracked by git (`/bindings` ignored; the UI copy is untracked and absent on a fresh worktree), so
  "diff shows only" was checked by that grep, not by `git diff`.
- `cd crates/desktop/ui && BUILD_LABEL=149-W0 build-lock bunx tsc -b --noEmit` → exit 0 (`/tmp/149-W0-tsc.log`;
  `bun install --frozen-lockfile` first in `crates/desktop/ui` and `packages/apps`).

### E7 deferred
- **E7 not built.** `list_activity(scope)` stays local-only; the §6 fallback rule stands: a row whose `SessionSummary.owner`
  is not this engine is drawn with a hollow sand dot + "on ‹node›", no spinner, mic and ✕ disabled ("open it on ‹node›"),
  counts from `sessions_status`. Reason: the cold mbx build of this fresh worktree cost ~75 min of lock time before the
  first test ran, and §12.3 Q3 (mesh aggregate vs honest hollow dot) is Sky's open question — building it tonight would
  preempt his answer. Shape when wanted: `forward::to_peer(ext, &peer, "list_activity", ())` per `mesh_status.peers`
  under a 1 s `tokio::time::timeout`, rows stamped with a new `AgentInfo.owner`, unreachable peers in a `partial` field;
  `to_peer` already refuses a second hop (`Forwarded`), so no loop.

## Gates run (what, where the log is)
| Gate | Result |
|---|---|
| `build-lock mbx test -p roster --no-run` → `mbx test -p roster` bare | 70 passed, 0 failed (`/tmp/149-W0-test-roster.log`) |
| `build-lock mbx test -p agent --lib --no-run` → `mbx test -p agent --lib` bare | 562 passed, 0 failed, 2 ignored (`/tmp/149-W0-test-agent.log`) |
| `build-lock mbx test -p rpc --no-run` → `mbx test -p rpc` bare | 56 + 1 passed, 0 failed (`/tmp/149-W0-test-rpc.log`) |
| `build-lock mbx clippy -p roster -p agent -p rpc --all-targets -- -D warnings` | exit 0, no warnings (`/tmp/149-W0-clippy.log`) |
| `cargo fmt -p roster/agent/autoresearch/double` bare | clean |
| bindings regen + `tsc -b --noEmit` | ok / 0 |

## Deviations
- `Inputs.session_context` is keyed by `String`, not `SessionId` (the brief's sketch): every other session map in
  `Inputs` is `String`-keyed and `AttentionItem.session` is a `String`.
- `CONTEXT_MAX` is 280 (brief) rather than §6's 160; both companions clip to their own render width.
- `autoresearch/src/actor.rs:4962` and `double/src/actor.rs:809` gained `context: None` — the only way to add a
  non-`Default` struct field without breaking their literal constructors; no behaviour change there.

## Open items
- E7 (above). Live proof against Sky's engine is owed with the morning rebuild (R5: his engine is the shipped 2.13.0
  binary, so `context` is invisible until then; W1 ships on the interim `SessionSummary.title` + `AgentInfo.last` join).
- `gather()` now does one `rows_in` replay per session behind an item on every recheck; same cost class as
  `info_row`'s `last_turn_text` per roster row. ponytail: a `last_text` column if the inbox recheck ever shows up in a profile.
