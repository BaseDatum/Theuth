# 109 as-built — C (the contract: seams, schema, store API, journal trait, stubs)

Worktree `theuth-109`, branch `feat/109-durability` off main b5aaf52d. Built 2026-10-06.
Scope: the skeleton every 109 wave codes against — no resume behaviour, no journal
writes from the loop, no jobs/questions writes, no ACP, no UI.

## 1. Migration

`crates/agent/migrations/0033_durability.sql` (0032 was the newest; 0033 taken as the design
says). The six tables of §3.1 verbatim (`turn_steps`, `tool_calls`, `questions`, `waits`,
`jobs`, `pty_screens`), the three indexes (`turn_steps_open`, `tool_calls_turn`,
`questions_pending`), the partial index `jobs_running … WHERE state = 'running'`, the
composite FK `pty_screens (session_id, job_id) → jobs ON DELETE CASCADE`, and the two §3.1
`ALTER TABLE sessions ADD COLUMN last_stop / finished_at` (ridden here so W1 adds no
migration). Applies on a fresh store (every store test) and on an existing one (sqlx
`migrate!` — the file only adds objects).

## 2. Types — `crates/agent/src/durability/mod.rs` (`pub mod durability;` in `lib.rs:36`)

- `DurabilityConfig` (+ `Default`: Auto, 4, 10, 1000, 5, 7, 4000), `ResumeMode`, `StepKind`,
  `CallState` (+ `as_str`/`parse`/`is_terminal`), `JournaledCall`, `LastStep`, `ResumeKind`
  (serde/JsonSchema/ts-exported — it rides in `AgentEvent::TurnResumed`), `PartialContent`,
  `ResumePlan` (§4.1 fields).
- `crash_point(step)` → `std::process::abort()` iff `THEUTH_CRASH_AT == step`; the §11.1
  names are constants in `durability::crash`, per-call points via
  `crash::call_point(i, crash::CALL_MID)` etc.
- Config: `AgentConfig.durability: DurabilityConfig` (`lib.rs:2262`), `#[serde(skip)]` and
  filled from the top-level `durability:` section in `supervisor::start`
  (`supervisor.rs:7329-7331`) exactly as `surface` is. Existing config files parse unchanged
  (test `durability::tests::config_defaults_and_partial_yaml`). `Agents.cfg` already carries
  it; nothing acts on it yet.
- Submodules: `durability::journal` (StoreJournal), `durability::jobs` (W2 stub),
  `durability::handoff` (W4 stub).

## 3. Store API — `crates/agent/src/store/durability.rs` (`pub mod durability;` in `store.rs:57`)

Free `*_in` helpers (join the caller's `WriteTx` through `&mut SqliteConnection`, the repo's
`append_in` style):

```rust
pub(crate) async fn step_in(conn, session, turn, kind: StepKind, call_id: Option<&str>, payload: &Value) -> Result<()>   // :54
pub(crate) async fn partial_delete_in(conn, session, turn) -> Result<()>                                                 // :81
pub(crate) async fn calls_planned_in(conn, session, turn, at_seq: i64, calls: &[(genai ToolCall, Risk)]) -> Result<()>  // :98
pub(crate) async fn delete_journal_in(conn, sid: &str) -> Result<()>                                                     // :127
```

`impl Store` (each one `WriteTx`; readers on the read pool):

```rust
pub async fn step(&self, session, turn, kind, call_id, payload) -> Result<()>                       // :288  step_in in its own tx
pub async fn partial_upsert(&self, session, turn, call_id: &str, content: &MessageContent, thinking_ms: Option<u64>) -> Result<()>  // :305
pub async fn call_started(&self, session, call_id: &str) -> Result<bool>                            // :346
pub async fn call_finished(&self, session, call_id: &str, out: &ToolOutput) -> Result<bool>         // :364
pub async fn call_interrupted(&self, session, call_id: &str, note: &str) -> Result<bool>            // :390
pub async fn calls_of(&self, session, turn) -> Result<Vec<JournaledCall>>                           // :414
pub async fn open_turns(&self) -> Result<Vec<(SessionId, TurnId, DateTime<Utc>)>>                   // :431
pub async fn resume_plan(&self, session, turn) -> Result<ResumePlan>                                // :446
pub async fn question_put(&self, q: &QuestionRow) -> Result<()>                                     // :557  upsert by id
pub async fn question_resolve(&self, id: &str, state: QuestionState, answer: Option<&Value>) -> Result<bool>  // :584
pub async fn questions_pending(&self, session) -> Result<Vec<QuestionRow>>                          // :610
pub async fn wait_put(&self, w: &WaitRow) -> Result<()>                                             // :623  upsert
pub async fn wait_clear(&self, session, call_id: &str) -> Result<bool>                              // :643
pub async fn waits_of(&self, session, turn) -> Result<Vec<WaitRow>>                                 // :656  via tool_calls.turn_id
pub async fn job_put(&self, j: &JobRow) -> Result<()>                                               // :672  upsert
pub async fn job_settle(&self, session, id: &JobId, state: JobState, exit: &JobExit, settled_ms: i64) -> Result<bool>  // :702
pub async fn jobs_open(&self) -> Result<Vec<JobRow>>                                                // :729  state = running
pub async fn jobs_of(&self, session) -> Result<Vec<JobRow>>                                         // :739
pub async fn pty_screen_put(&self, s: &PtyScreenRow) -> Result<()>                                  // :751  upsert
pub async fn pty_screen(&self, session, job_id: &JobId) -> Result<Option<PtyScreenRow>>             // :776
pub async fn journal_prune(&self, keep_days: u32) -> Result<u64>                                    // :810  §3.6
```

Semantics worth knowing:

- `step_in` computes `n = MAX(n)+1` per turn and `at_seq = MAX(messages.seq)` for the session
  (`-1` before the first message).
- `partial_upsert` UPDATEs the one `assistant_partial` row of `(turn, call_id)` and only
  INSERTs when none exists; `partial_delete_in` is the DELETE W1 folds into the final
  persist (§8.1).
- `call_started` / `call_finished` / `call_interrupted` only move rows in `planned`/`started`
  and return `false` otherwise — so calls the loop never planned (app invocations,
  RPC-started jobs, inner `toolu_1.3` calls) are no-ops at the registry hook.
- `resume_plan`: the newest row that is not `assistant_partial`; a `suspended` row sets
  `kind = Suspended` and defers to the row before it (§4.2 row 6). `ModelCall` carries the
  partial whose `call_id` matches the step's; `ToolBatch` returns the rows in
  `payload.call_ids` order and `all_finished` = every row terminal. No journal → `Err`.
- `open_turns`: window query (`ROW_NUMBER() OVER (PARTITION BY session_id, turn_id ORDER BY n
  DESC)`, partial rows excluded), `kind != 'turn_finished'`, ordered by the turn's `MAX(at)`
  descending.
- `journal_prune(keep_days)`: for every **finished** turn whose last `at` is older than the
  cutoff, deletes its `waits` (by call id), `tool_calls`, `turn_steps`; plus every terminal
  `questions` row with `COALESCE(answered_at, deadline) < cutoff`. Open turns and pending
  questions are never pruned. Returns the `turn_steps` rows removed.
- `delete_session` (`store.rs:2348`) now calls `delete_journal_in` (waits, questions,
  tool_calls, turn_steps, jobs; `pty_screens` cascade), so the FK (`foreign_keys` is on by
  sqlx default) does not refuse deleting a session with a journal.
- `capture::ATTACHED` (`store/capture.rs:16-31`) gained `turn_steps`, `tool_calls`,
  `questions`, `waits`, `jobs` — not `pty_screens` (§3.5). `questions` has `session_id`
  outside its PK, so an UPDATE on it goes through `WriteTx::commit`'s existing by-pk lookup.
- `fail_interrupted` (`store/communication.rs:1447-1455`) narrowed per §7.2:
  `AND NOT (sender LIKE 'session:%' AND request_id LIKE 'result:%')`; return shape unchanged.

Tests (`store/durability.rs::tests`, 5; `durability::tests`, 3; `communication.rs`
`suppression_follows_subtree_and_restart_fails_pending` extended): plan per `LastStep`
shape incl. partial upsert-in-place, `suspended` wording, `turn_finished` closing; batch
state transitions and no-op on unknown ids; `open_turns` order + prune (open turn never
pruned, 7-day keep keeps fresh rows, 0-day keep takes a finished turn's 4 steps/3 calls/1
wait and leaves the other session alone); questions/waits/jobs/screens round trip incl.
`delete_session` cascade; `session_log` grows for a step and a job row and not for a
`pty_screens` row; the spared `result:` exchange stays `accepted` while a `result:` from a
non-`session:` sender still fails.

## 4. Journal seam — `crates/tools/src/journal.rs` (re-exported from `lib.rs:62-65`)

- `pub trait Journal` exactly as §10.2 (BoxFuture returns): `call_started`, `call_finished`,
  `question(QuestionRow)`, `wait(Option<WaitRow>, session, call_id)`, `job(JobRow)`,
  `pty_screen(PtyScreenRow)`.
- Row types, plain data, no sqlx: `QuestionRow` (+ `QuestionKind`, `QuestionState`),
  `WaitRow` (+ `WaitKind`), `JobRow` (+ `JobKind`, `JobState`), `PtyScreenRow`; each enum has
  `as_str`/`parse` matching the column spellings.
- `InstallJournal(pub Arc<dyn Journal>)`, `NoopJournal`. Handlers: `ToolRegistry`
  (`registry.rs:1345` — stores it and forwards to the jobs/approval/ask actors by registry
  name, like `RegisterActivity` forwards `InstallActivity`), `Jobs` (`jobs.rs:1038`),
  `Approval` (`approval.rs:113`), `AskUser` (`ask.rs:155`); each actor holds a
  `pub(crate) journal: Arc<dyn Journal>` field initialised to `NoopJournal`.
- `JobExit::Lost` (`jobs.rs:90`), `Display` → `lost`, new `FromStr` (inverse of `Display`,
  for the `jobs.exit` column).
- Hooks in `registry.rs::invoke`: `journal.call_started(session, &call_id)` at `:1596`,
  immediately before the body is spawned (after approval, the revoked-admission check and
  secret expansion — every early return before it leaves the row `planned`, so a refused
  call is never reported "effect unknown"); `journal.call_finished(session, &call_id, &out)`
  at `:1618`, after `redact_output`, so the row holds what the model sees. `invoke` takes
  the journal as a ninth argument from `dispatch` (`:1073`, `:1109`).

## 5. Agent-side impl — `crates/agent/src/durability/journal.rs`

`pub struct StoreJournal(pub Arc<Store>)` implementing `tools::Journal` over the store API;
failures are `tracing::error!`-logged, never surfaced. Installed in `supervisor::start`'s
registry task (`supervisor.rs:7555-7562`, comment `// 109: journal seam`) right after
`RegisterActivity` acks and **before** `ActivityRegistered` is told, so nothing the resume
path runs can precede the install.

## 6. Supervisor skeleton (`supervisor.rs:107-112` declares the modules and re-exports)

- `supervisor/resume.rs`: `ResumeTurn { session, turn }` (tell; stub
  `warn!("109: ResumeTurn not implemented")`), `OpenTurn(SessionId)` → `Option<TurnId>`
  (implemented via `open_turns`), `Agents::resume_scan` (stub: logs the open-turn count).
- `supervisor/drain.rs`: `Drain { deadline: Instant }` → `DrainReport { suspended, finished }`
  (stub: zero report, warns).
- `supervisor/waits.rs`: `Agents::rebuild_waits` (no-op stub).
- `durability/jobs.rs`: `pub async fn reap_jobs(&Store) -> ReapReport` (empty stub;
  `ReapReport { killed, lost, orphaned }`).
- `durability/handoff.rs`: `pub async fn compose_handoff(&Store, SessionId, max_tokens) -> String`
  (empty stub).
- Hooks, in §4.6 order, inside the `ActivityRegistered` handler right after
  `rearm_reminders` (`supervisor.rs:1952-1958`): `reap_jobs` → `rebuild_waits` →
  `resume_scan`.

## 7. Loop stubs — `crates/agent/src/loop_/resume_calls.rs` (`loop_.rs:5720-5722`)

`resume_job_call` (W2), `resume_question_call` (W3), `resume_wait_call` (W3), each
`(call: &JournaledCall, plan: &ResumePlan) -> ToolOutput` and returning the §4.3
interrupted note (`interrupted_note`, local `HH:MM:SS`). Declared `pub(crate)` with
`#[allow(dead_code)]` until W1's per-call rule wires them. Nothing else in `loop_.rs`
changed.

## 8. Events (`lib.rs:2083-2132`, `session()` at `:2172-2175`)

`AgentEvent::TurnResumed { session, turn, kind: ResumeKind, interrupted_at, resumed_at,
partial_dropped }`, `ResumeReport { session, turn, replayed, interrupted, lost_jobs:
Vec<JobId>, children_restored, children_handed_off }`, `TurnSuspended { session, turn,
reason }`, `PartialDropped { session, turn, call_id }`. ts-exported like their siblings. The repo's
`just ts` (`cargo test -p rpc --features ts --lib --quiet export_bindings`) was run: it writes
to the git-ignored root `bindings/` (`bindings/AgentEvent.ts` now carries the four variants,
`bindings/ResumeKind.ts` is new, `bindings/JobExit.ts` gained `"lost"`), so nothing tracked
changed; the tracked `crates/agent/bindings/AgentEvent.ts` is a stale artifact from b058019d
that `just ts` does not write and was left alone. `just ts-desktop` was not run.
`JobEvent::Finished` carries `exit: "lost"` through the existing `JobExit` serde.

## Deviations from the design / brief

| what | design says | built | why |
|---|---|---|---|
| `step_in` / `calls_planned_in` | associated fns on `Store` taking `&mut WriteTx` | free `pub(crate)` fns taking `&mut SqliteConnection` (`WriteTx` derefs to it) | the repo's existing `*_in` convention (`append_in`, `ensure_owner_in`) |
| `call_started/finished/interrupted` return | `Result<()>` | `Result<bool>` (row moved?) | lets W1 tell a planned call from one it never journaled; the `Journal` impl ignores it |
| extra store fns | not in §10.2 | `step` (own tx), `partial_delete_in`, `calls_of`, `jobs_of`, `delete_journal_in` | §8.1 needs the delete, §3.3 step 8 and §5.1 need the reads; `delete_session` needs the FK cleanup |
| `PartialDropped` fields | §8 names none | `{ session, turn, call_id }` | smallest set that identifies the dropped row |
| `QuestionRow.answer` / `payload` | "JSON" | `serde_json::Value` (`Option<Value>` for answer) | plain data, no string-typing |
| `ALTER TABLE sessions` | §3.1 "ride in the same migration" (W1 owns `last_stop`) | included in 0033 | the brief forbids later migrations; the columns are unused until W1 |
| `call_started` placement | "before anything is expanded or dispatched" | after secret expansion, immediately before the body | an early return after `started` (revoked route, bad placeholder) would be misreported "effect unknown" on resume; the window moved is microseconds and effect-free |
| `JobExit::Lost` in `jobs.rs` settlement match | never produced in-process | a total arm (`…lost` output) rather than `unreachable!` | no panic path added |
| `resume_scan` | W1 | stub that only logs the count | brief: W1's stub; `open_turns` is ready for it |

Not done (by brief): `commit_cut`/fork/undo treatment of journal rows (§3.2, W1);
`turn_steps`/`tool_calls` writes in `loop_.rs` (W1); `jobs`/`pty_screens` writes and the
32-row cap (W2); `questions`/`waits` writes (W3); `last_stop`/`finished_at` writes (W1);
the boot pass over spared `result:` exchanges (W3); `Drain`/`main.rs` (W5); UI/RPC (W5).

Open item for W1: the registry hook stores `ToolOutput.content` as returned (redacted,
not spill-capped); the batch-end path should overwrite `output` with the spill-capped text
(§3.3 step 7) or cap it in `StoreJournal::call_finished`.

## Cross-seam touches

- `crates/tools/src/pty.rs:1951` — `JobExit::Lost` added to the exit match arm.
- `crates/agent/src/supervisor.rs:2345` — `JobExit::Lost => "lost"` in `job_result`.
- `crates/tools/src/jobs.rs:1392` — `(JobExit::Lost, None)` arm in the settlement match.
- `crates/slack/src/channel.rs:2554-2559` — the four new `AgentEvent` variants added to the
  thread worker's ignore arm (exhaustive match).
- `crates/tui/src/model.rs:5938-5943` — same, in `apply_agent`'s exhaustive match.
- No rpc/desktop/schedule edits were needed: `cargo check --workspace` found no other
  exhaustive matches on `JobExit` or `AgentEvent`.

## Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-target`, cargo 1.98.1, in this order:

1. `cargo test -p tools` — `154 passed; 0 failed` (lib) + `0 passed` (doc).
2. `cargo test -p agent` — first run `388 passed; 1 failed`
   (`supervisor::tests::private_root_is_owned_and_unlisted`, a `CreateSession … .unwrap()`
   at `supervisor.rs:10642`); passes alone and on two full reruns:
   `389 passed; 0 failed` (lib) + `0 passed` (doc). The test does not touch any 109 code
   path (private `:memory:` store, no registry); recorded as a flake, not fixed.
   The 109 tests alone: `cargo test -p agent --lib durability` → `8 passed`.
3. `cargo check --workspace` — two exhaustive `AgentEvent` matches fixed (slack, tui; see
   cross-seam touches), then `Finished dev profile`.
4. `cargo clippy -p tools -p agent` — 0 warnings in files touched by this wave (two
   `pub(crate)` helpers W1 wires carry `#[allow(dead_code)]`); pre-existing warnings
   elsewhere left alone.
5. `cargo test -p rpc --features ts --lib --quiet export_bindings` (`just ts`) — `8 passed`
   (the first attempt died in `wavekat-vad`'s `build.rs`, a dependency build unrelated to
   this change; the rerun built and passed).
6. Smoke, migration on an existing database: a `.backup` copy of the user's 598 MB
   `sessions.sqlite` (at migration 32) opened through `Store::open` in a throwaway
   integration test; `open_turns` / `jobs_open` / `journal_prune(7)` ran against it and
   `PRAGMA table_info(sessions)` showed `last_stop` / `finished_at`. Test file and copy
   removed afterwards.

Files touched: `crates/agent/migrations/0033_durability.sql` (new);
`crates/agent/src/{lib.rs, loop_.rs, supervisor.rs, store.rs}`;
`crates/agent/src/durability/{mod.rs, journal.rs, jobs.rs, handoff.rs}` (new);
`crates/agent/src/store/{durability.rs (new), capture.rs, communication.rs}`;
`crates/agent/src/supervisor/{resume.rs, drain.rs, waits.rs}` (new);
`crates/agent/src/loop_/resume_calls.rs` (new);
`crates/tools/src/{journal.rs (new), lib.rs, jobs.rs, registry.rs, approval.rs, ask.rs, pty.rs}`;
`crates/slack/src/channel.rs`, `crates/tui/src/model.rs` (cross-seam);
`docs/design/109-as-built-C.md` (this note).
