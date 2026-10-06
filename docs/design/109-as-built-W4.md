# 109 as-built — W4 (ACP children: respawn on resume, delta handoff)

Worktree `theuth-109-w4`, branch `feat/109-w4` off the contract commit 8a5f8b75. Built 2026-10-06.
Scope: §7.3 — the boot trigger that brings an ACP child back when a resumed parent turn waits
on its result, the `resumed()` branch in the child actor, the delta handoff and its budget.
Nothing in `loop_.rs`, `store/**`, `crates/tools/**`, `supervisor/{resume,waits,drain}.rs`,
`crates/desktop/**` or `crates/acp/**` changed (`Harness::resumed()` already exposes what the
branch needs; `harness.rs` untouched).

## 1. Handoff — `crates/agent/src/durability/handoff.rs` (the contract stub, filled)

- `Handoff` (`:33`): the sections as plain data — `session`, `at`, `cwd`, `last_report`,
  `status`, `diff`, `queued: Vec<String>`, `todos: Vec<String>`.
- `render(&Handoff, max_chars) -> String` (`:75`), pure. Fixed header (the §7.3 wording: the
  restart at local `HH:MM:SS`, "could not be restored", "Keep the uncommitted diff: do not
  revert, stash or discard it") and fixed footer (`session_read("<child>")` pointer, "Continue
  from the uncommitted state; do not redo finished steps"). Sections in §7.3's order — last
  report, `git status --porcelain`, `git diff --stat`, queued messages, open todos — each with
  its count; an empty section is left out. Budget = `max_chars − header − footer − 120`
  (room for the omitted line, `OMITTED_RESERVE` `:23`); sections are added in order, the first
  one that does not fit keeps its head and ends `… [N chars cut]`, every later section is named
  in one `(omitted for the handoff budget: …)` line. The header/footer are never cut.
- `git(cwd, args)` (`:190`): `tokio::process::Command`, `kill_on_drop`, 5 s cap
  (`GIT_TIMEOUT` `:19`); non-zero exit / spawn failure / timeout → `""` (section absent),
  logged at debug/warn. The only two invocations are `status --porcelain` and
  `diff --stat HEAD` — no other shell.
- `compose_handoff(&Store, SessionId, max_tokens)` (`:218`, the contract signature): `cwd` from
  the session row; `last_report` = `Store::last_turn_text` (assistant text after the last user
  row — for an interrupted child that is what it had flushed so far); queued = the child's
  `peer_records` (32 newest) addressed to it in `accepted`, **plus those the boot pass failed with
  `interrupted_by_restart`** (a steer `fail_interrupted` failed at step 8 never reached the
  child, so it is still "queued for you"); todos = items not `done`/`dropped`, as
  `[status] text (reason)`; budget `max_tokens × 4` chars.
- `restored_note(at)` (`:59`): the first input of a child whose `session/load` succeeded — the
  restart, "has been restored", keep the diff, continue, report. No delta (§7.3: the harness
  replayed its own context).

## 2. Actor — `crates/agent/src/acp_child.rs`

- `pub struct Respawn(pub usize)` (`:380`), `tell`; field `respawn: Option<usize>` (`:136`).
  Remembered until `Connected`; acted on at once when the harness is already up, or failed
  immediately when the connection already died (`impl Message<Respawn>` `:1887`).
- `queue_respawn(tokens)` (`:1131`) — the `resumed()` branch: `Harness::resumed()` →
  `restored_note(now)`, else `compose_handoff(store, id, tokens)`; the text goes to the
  **front** of the input queue as a `synthetic` `Queued` (`:1115`: no `InputId`, no ticket, no
  peer), so `next` → `start` → `open` persists it as the turn's user message, publishes
  `TurnStarted`/`TurnFinished`, and `finish` reports `PeerTurnFinished` → `child_finished` →
  `hand_off` → the `result:<turn>` exchange — the same path a live child's result takes.
- `Connected` `Ok` (`:1572`): after `self.harness = Some(h)`, a pending respawn queues its
  input before `next`. The existing "previous harness session not restored; starting fresh"
  error (`:1496–1513` region) still fires on a failed load; `set_acp_session` still stores the
  new id.
- `Connected` `Err` (`:1581`): a pending respawn pushes `RESPAWN_FAILED_INPUT` (`:384`,
  `<runtime>respawn after a theuth restart</runtime>`) so the existing drain path opens one
  turn and finishes it `error` with the connect text — the parent's `child_result` wait is
  answered instead of hanging on a no-deadline wait.
- `CANCELLED_NOTE` mechanics untouched; `Note`s still ride on the wire ahead of the first
  prompt's text. A boot respawn never adds `CANCELLED_NOTE` (a parent waiting on a result is not
  a cancelled turn; `last_stop` is process-local and empty at boot).

## 3. Trigger — `crates/agent/src/supervisor/acp_resume.rs` (new, `impl Agents`)

- `respawn_acp_children` (`:24`): `durability.resume != auto` → nothing. For every
  `Store::open_turns` row, `waits_of(parent, turn)` filtered to `WaitKind::ChildResult`, target
  parsed as the child id, each child once (`HashSet`).
- `respawn_acp_child(parent, child) -> Result<bool, String>` (`:63`): no row → `false`; row
  without `agent` (native child) → `false` (§4.6/D5 covers it); engine not in `agents.yaml`
  (a node's child, or no entry) → `false` + warn; a live actor → `false`; a spared `result:`
  exchange — any `pending_exchanges(parent)` row from `session:<child>` (after step 8 only
  `result:` rows from sessions can still be `accepted`) → `false` + info (§7.2 delivers it).
  Otherwise `running.remove(child)`, `Agents::session(Some(child), …)` (→ `acp_session` with
  `resume = store.acp_session(child)`, stored `cwd`, skills written back, the usual
  `Live`/ledger registration) and `tell(Respawn(durability.handoff_tokens))`. A spawn error is
  recorded/published on the child session as today's `Agents::error` and returned.
- Hooked at the end of the `ActivityRegistered` handler, after `resume_scan`
  (`supervisor.rs:1960`, `// 109 W4 §7.3`) — the ordering invariant (`reap_jobs` before any
  resume) holds, and W1's scan is untouched. The handoff's `git` reads run on the child actor's
  mailbox at boot only, ≤ 10 s worst case per child.

## 4. Tests (9, all in `cargo test -p agent`)

- `durability::handoff::tests` (4): `empty_sections_are_omitted_and_full_ones_rendered`
  (bare handoff has header/footer only; full one has every section in §7.3 order);
  `budget_truncates_tail_first_with_markers` (`len ≤ max`, the earlier section whole, the
  truncated one keeps its head and loses its tail with the `chars cut` marker, the later ones
  named as omitted, footer intact; a sufficient budget reproduces the unbounded text);
  `restored_note_has_no_delta`; `compose_reads_git_store_and_omits_without_cwd` — a temp git
  repo with one commit, a modified and an untracked file: `M a.txt`/`?? new.txt` and
  `a.txt | 1 +` appear, the last assistant row is the report, a `done` todo is not open, both an
  `accepted` steer and one failed by `fail_interrupted("interrupted_by_restart")` are listed,
  the `session_read` pointer names the child; a row without `cwd` renders no git section.
- `acp_child::tests` (4), the decision table on the existing `Fake` dialled harness, never a
  real omp/opencode: `respawn_after_successful_load_prompts_restored_note` (`acp_session`
  present × load ok: `session/load s1`, first prompt = restored note, no `session_read`, no
  error, the note is the persisted user row, `end_turn` closes the turn);
  `respawn_after_failed_load_prompts_delta_handoff` (present × load fails: `session/load`
  errors → `session/new s2`, the "not restored; starting fresh" error, prompt = handoff with
  the child's last report and pointer, no git section for a non-repo cwd, ≤ 16 000 chars,
  `acp_session` now `s2`); `respawn_without_stored_session_prompts_delta_handoff` (absent:
  `session/new`, handoff, no error); `respawn_connect_failure_is_the_turns_error` (a refusing
  dialer: one turn with `RESPAWN_FAILED_INPUT`, `stop_reason = error`, the dial error as the
  assistant row, the actor stops).
- `supervisor::acp_resume::tests::respawn_decision_table`: a parent open turn
  (`input_accepted` + `tool_batch` of six planned calls) with `child_result` waits on a native
  child, a node's child (`omp@box`), a child whose `result:` exchange is spared, a session with
  no row and — twice — a local `omp` child: each of the first four → `Ok(false)`, nothing
  spawned; `resume: off` → nothing; `resume: auto` → exactly one `LiveActor::Acp` for the ACP
  child.

## 5. Deviations from the design / brief

| what | design/brief says | built | why |
|---|---|---|---|
| how the handoff reaches the harness | §7.3: "the `Note` mechanism carries it before the user's text" | the handoff (or restored note) **is** the first input: persisted as the turn's user row, `TurnStarted.text`, visible in the transcript | at boot there is no user text to ride on — the parent is blocked waiting — so the child must be prompted; a wire-only `Note` would also hide from Sky what the child was told. `Note`/`CANCELLED_NOTE` are unchanged for the live `resume_child` path |
| restored child | "gets no handoff" | gets `restored_note` (restart time + "continue, report") as its first input | same reason: `session/load` replays context but does not re-run the interrupted prompt, so without an input the parent's no-deadline wait never ends `[INFERENCE — not verified against a real omp]` |
| trigger 2 of §7.3 ("`resume_scan` finds an ACP child session whose own `turn_steps` are open") | ACP child turns journaled, scan resumes them | **not built**; only the parent-wait trigger of the brief | journaling ACP turns would make W1's `resume_scan`/`ResumeTurn` act on `LiveActor::Acp` sessions, which W1 owns and I cannot see; the brief's trigger covers the case that matters (a waiting parent) |
| composition location | §7.3: "a pure function in `acp_child.rs`" | `render` is the pure function in `durability/handoff.rs` (the contract's stub file) | the contract put the stub there |
| `git diff --stat` | "since the last commit" | `git diff --stat HEAD` (staged + unstaged) | literal reading; a repo with no commit has no `HEAD` → section absent |
| queued steers | "queued `accepted` exchanges to the child" | `accepted` **or** `failed` with `interrupted_by_restart`, last 32 records | by the time the respawn runs, step 8 has already failed every accepted steer; those are exactly the ones the child never saw |
| "turns N–M" in the pointer | example text names a turn range | pointer only | the range would need the harness's view of what it saw; `session_read` lists the outline |
| connect failure on respawn | not covered | one `error` turn with the connect text so the wait is answered | a no-deadline wait otherwise hangs the parent for good |
| trigger order | §12 W4 row lists the trigger; §4.6 lists `reap_jobs → rebuild_waits → resume_scan` | appended after `resume_scan` | keeps W1's and W3's hooks untouched; nothing in the trigger needs their in-memory state (it reads rows) |

## 6. Cross-seam touches

- `crates/agent/src/supervisor.rs:108` — `mod acp_resume; // 109 W4` (the trigger's `impl
  Agents` needs `Agents::session`, private to `supervisor`; the file follows the "handler
  bodies in their own files" convention of `resume.rs`/`waits.rs`/`drain.rs`).
- `crates/agent/src/supervisor.rs:1960` — `self.respawn_acp_children().await; // 109 W4 §7.3`
  in the `ActivityRegistered` handler, after `resume_scan`.

The `acp_session` function itself needed no edit.

## 7. Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-target`, cargo 1.98.1:

1. `cargo test -p agent` — `398 passed; 0 failed` (lib) + `0 passed` (doc). The 9 W4 tests:
   `cargo test -p agent --lib -- acp_child::tests::respawn acp_resume durability::handoff` →
   `9 passed`.
2. `cargo test -p acp` — `23 passed; 0 failed` (lib) + `0 passed` (doc); no file in the crate changed.
3. `cargo clippy -p agent --tests` — `0` warnings/errors in the whole crate (so none in the
   touched files); touched files formatted with `rustfmt --edition 2024` (diff confined to W4 code).

## 8. Not tested / manual check for Sky (§11.3 step 5)

Not reachable without a real harness: that omp's `session/load` really restores the context and
that an omp prompted with `restored_note` continues the interrupted work rather than restarting
it; that `compose_handoff`'s text makes an opencode/omp keep the diff. Both texts say so
explicitly; the wording is in `handoff.rs:59` and `:75` if a real run shows it needs tuning.

Manual check (needs W1's `ResumeTurn`/journal writes and W3's `child_result` wait rows to be
present in the integrated build — without them `open_turns`/`waits_of` return nothing and the
trigger is a no-op):

1. In a chat, `spawn_subagent` an omp child on a worktree with a task that edits files; wait
   until `git status` in that worktree shows changes.
2. `kill -9 $(pgrep -f 't --headless')`; reopen the app.
3. Expect in the child session: a user row `<resume_handoff> … has been restored …`
   (omp's `session/load` succeeded), the child continuing, and the parent's `send_message` /
   `spawn_subagent` call returning the child's result when it finishes. Engine log:
   `109: ACP child respawned; prompting restored=true`.
4. Break the load (`rm` omp's session file under its data dir, or start with a stale
   `acp_session`), repeat 1–2. Expect the "omp: previous harness session not restored; starting
   fresh" error on the child, then a user row `<resume_handoff> … could not be restored …
   Uncommitted work in <worktree> (git status --porcelain, N lines) … Diff since the last
   commit …`, the child keeping the files, and the parent's call answered.
5. `durability: { resume: off }` → neither happens; the parent's next input resumes lazily.

## 9. Not done (by brief)

`ResumeTurn`/`resume_scan` (W1), the `child_result` wait rows and their re-arm (W3), the boot
pass over spared `result:` exchanges (W3), journaling of ACP child turns (§7.3 trigger 2, see
deviations), any UI.

Files touched: `crates/agent/src/acp_child.rs`, `crates/agent/src/durability/handoff.rs`,
`crates/agent/src/supervisor/acp_resume.rs` (new), `crates/agent/src/supervisor.rs`
(2 lines, cross-seam), `docs/design/109-as-built-W4.md` (this note).
