# 109 as-built — I (integration of C + W1 … W5)

Branch `feat/109-durability`, worktree `theuth-109`, from merge 1009a9a2 (C, main 48abf2cd,
W4, W1, W2, W3, W5 in that order). The merged tree did not compile (`/tmp/109-gate1.log`);
every wave's gate had been green on its own branch. This note lists what changed to make the
merged tree compile *and* behave as §4.6 / §7.2 / §9.2 say, which hand-offs between waves were
closed, and the gate results. Line numbers are of the tree at the last commit below.

## 1. Compile breakage (commit 8baeae6c)

| error | cause | fix |
|---|---|---|
| E0592 `spared_child_results` twice (`store/communication.rs`) | W1 and W3 both built the §7.2 store read | kept W3's (`communication.rs:1418-1432`, returns `Vec<PeerRecord>` — the delivery needs the record, not just ids); deleted W1's tuple-returning twin. W1's store-level scenario `spared_child_results_are_listed` (`loop_/durability_tests.rs:740`) adapted to `PeerRecord` (same assertions: the `result:` row is listed, the plain one is failed) |
| E0592 / E0034 `deliver_spared_results` twice (`supervisor/resume.rs:133`, `supervisor/waits.rs:282`) | both waves built the §7.2 boot pass | kept W3's (`waits.rs::deliver_spared_results`, `pub(super)`): it skips a result whose parent has an open turn with a `child_result` wait (the resumed call consumes it, §4.4) and has a supervisor-level test (`spared_child_results_delivered_once`). Deleted W1's. W1's one extra — `Agents::error(parent, "subagent result not delivered: …")` on an intake refusal, `hand_off`'s wording — was carried into W3's `deliver_spared` (`waits.rs:257`) |
| the pass ran twice at boot | W3 scheduled it by `try_send(DeliverSparedResults)` from `rebuild_waits`; W1's merge also called it at the end of the `ActivityRegistered` handler | the self-tell and the `DeliverSparedResults` message are gone; the one call is the last line of the handler, `supervisor.rs:1973`, in the §4.6 order `reap_jobs → rebuild_waits → resume_scan → respawn_acp_children → deliver_spared_results`. (The dedup set made the double pass harmless; the brief asked for exactly once.) |
| E0061 / E0308 `resume_job_call` (`loop_.rs:3106`) | W2 changed the contract signature to `(store, session, call, plan) -> Option<ToolOutput>` | call site passes `&self.shared.store, self.shared.id`; `None` (no `jobs` row — the kill fell between approval and spawn, §5.6 row 2) falls into the "run again" arm: the row goes back to `planned`, `attempt + 1`, and runs with the batch (`loop_.rs:3105-3110`) |
| W3's `session_of_turn` | the stubs carried no session, so the supervisor searched `open_turns()` for the turn (W3's open item) | `resume_question_call` / `resume_wait_call` take `session` (`loop_/resume_calls.rs:195, 245`); `ResumeQuestion { session, call_id }`, `ResumeWait { session, turn, call_id }`; `session_of_turn` deleted; `ResumeQuestion`'s reply is now `Option<QuestionRow>` (the session was only echoed back). W3's tests updated to pass the session |

## 2. Hand-offs between waves

### W2 → W5: `KillAll` after the drain window (§5.5, §9.2 step 6) — commit 42ab3f94

Checked: every path that ends the engine runs `drain_engine` **before** `rt.shutdown.cancel()`
(`main.rs:1532`, `rpc/src/api.rs:4584`, `update/src/lib.rs:663`), and `tools::jobs::start` tells
`KillAll` on the token — so the kill already came after the window; but `main.rs` exits ~100 ms
after `cancel()`, which is not a guarantee that the `lost` rows the handler writes first (§5.5)
reach the disk. The drain handler now tells `KillAll` itself, awaited (`ask`), after the window
and before it replies (`supervisor/drain.rs:130-136`); the token-fired one that follows finds
nothing running. W2's `killall_writes_lost_before_kill` covers the handler; the ordering is the
drain task's.

### The drain did not suspend — it cancelled (found while wiring the above; same commit + fdfe43d7)

Every turn's cancel token is `rt.shutdown.child_token()`. After the window `cancel()` fired, the
running batch returned `Round::Stop("cancelled")`, `finish_batch` persisted the batch message with
`cancelled by user` for the cut rows, and `drive` wrote `turn_finished(cancelled)` — the turn was
**closed as a user cancel**, so the next start had nothing to resume. W5's `suspended` row and
`TurnSuspended` event were written, but the journal ended `turn_finished`. Fixed in `loop_.rs`:

- `run_batch` (`loop_.rs:5000-5003`): a batch cut while `rt.shutdown.is_cancelled()` returns
  `Round::Stop("suspended")` before `finish_batch` — the rows stay as the journal has them
  (`started` → the per-call rule, `planned` → run on resume), no batch message is written.
- `drive` (`loop_.rs:4141-4147`, `4186-4189`): a `cancelled` stop with the shutdown token
  cancelled is `suspended`; a suspended turn writes no `turn_finished`, publishes no
  `TurnFinished` / `PeerTurnFinished` (a child's suspended turn is not a result), titles nothing;
  the repl is dropped (the previous dump stands, §9.2 step 5).
- `round` (`loop_.rs:4429-4446`): §9.2 step 2 — no new model call opens while the session is
  draining: the turn writes `turn_steps(suspended, {"reason": "drain"})` and stops `suspended`.
  So a drained turn's newest real step is the one before the drain (`resume_plan` skips
  `suspended` rows and sets `kind = Suspended`), and a new input accepted during the window
  (W5's "not gated" deviation) is journaled `input_accepted` + `suspended` and resumes as §4.2
  row 1 — the user row is persisted by the loop as before, nothing is held in memory.
- The flag is per session: `Shared.draining` (`loop_.rs:681`), set by a new `Draining` tell on
  `Session` (`loop_.rs:421-434`) that the drain task sends to every live native actor before it
  waits (`drain.rs:105-115`). A first version read W5's process-global `DRAINING`; W5's drain
  test sets it and never clears it, so every later turn in the `agent` test binary stopped
  `suspended` (8 `loop_::*` failures). A session created *during* the window is not flagged;
  its turn is cut by the token like any other and resumes `kind = Interrupted` (wording only).

### W1 → W5: `Loaded.resume.report`

Not populated. `ResumeReport` is published by the resumed turn after its batch message
(`loop_.rs::Turn::report`) and nothing persists it; `Loaded.resume` describes a turn that is
*still open* (`open_turns` + `resume_plan`), i.e. one whose report does not exist yet. The live
event is the report. Left as W5 documented.

### W1 "ask publishes nothing" vs W5 `resume: ask` Attention item

Verified by reading the path: `resume_scan` (`supervisor/resume.rs:82`) runs nothing in `ask`;
the attention actor derives `resume:<session>:<turn>` items from `OpenTurns`
(`supervisor/drain.rs:55`); the desktop's Resume calls rpc `resume_turn(session)`
(`rpc/src/api.rs:5508`) → `OpenTurn` → `ResumeTurn { session, turn }` → `Agents::resume_turn`
(`resume.rs:95`: `resume_plan`, lazy `Agents::session`, `actor.ask(Resume { plan, slots: None })`)
→ `Turn::resume`. W5's smoke (its note §Smoke step 3) exercised the first half against the
contract stub; the second half is W1's `Resume` message, exercised by W1's scenarios and the W6
matrix. An input that arrives before the user presses Resume settles the open turn lazily
(`Turn::settle_open_turns`) and the item is cleared by the turn closing, as W5 says.

### W5's desktop `EXIT_GRACE 2 s < drain_secs 10 s` — commit 42ab3f94

`crates/desktop/src/launch.rs:35`: `EXIT_GRACE` 2 s → 12 s (= default `drain_secs` 10 s + the
one second `drain_engine` allows past the deadline + slack), with a doc comment naming the
dependency. A daemon with nothing to drain still exits in well under a second (`stop` polls every
100 ms and returns on exit), so a normal quit is unchanged. Not made configurable: the desktop
does not read the engine's `durability` section and 12 s is the ceiling, not the wait.

## 3. Deviations from the design / brief

| what | design / brief | built | why |
|---|---|---|---|
| `suspended` row for the running turn | written at the deadline (§9.2 step 4) | W5's at drain start (kept) **plus** one per session at the first `round` after `Draining` | the first survives a SIGKILL inside the window; the second keeps `kind = Suspended` for a turn that was mid-batch when the drain began and finished its batch inside the window (a new `model_call` step after W5's row would have reverted the kind to `Interrupted`) |
| `Round::Stop("suspended")` written by `round` | "a `round` that would open one writes `suspended`" | same, plus the token cut of a batch and of a model stream also stop `suspended` without `turn_finished` | the design assumed the token cut would be the drain's; in the code it was `cancelled`, which closed the turn |
| `TurnFinished` for a suspended turn | not stated | not published (`TurnSuspended` is W5's, published by the drain before the token) | a `TurnFinished(suspended)` would settle the schedule run / the parent's child wait with a non-result |
| `ResumeQuestion` reply | `(SessionId, Option<QuestionRow>)` (W3) | `Option<QuestionRow>` | the session is the caller's |

## 4. Cross-seam touches

All files are in the integration scope by the brief; the ones outside any wave's "owns" list:
`crates/desktop/src/launch.rs` (`EXIT_GRACE`, named by the brief). No migration, no RPC shape,
no TypeScript binding changed (`just ts-desktop` wrote the same `bindings/`).

## 5. Gates

All with `CARGO_TARGET_DIR=$HOME/Develop/theuth-109-integ-target`, cargo 1.98.1:

1. `cargo test -p tools` — `165 passed; 0 failed` (lib) + `0` (doc).
2. `cargo test -p agent` — first run `405 passed; 8 failed` (the process-global `DRAINING`, §2
   above); after fdfe43d7 `413 passed; 0 failed; 1 ignored` (`crash_child`, spawned by the
   matrix) + `0` (doc). `supervisor::tests::private_root_is_owned_and_unlisted` passed first time.
3. `cargo test -p acp -p secrets -p roster -p schedule -p rpc -p tui` — acp `23`, secrets `23`,
   roster `58`, schedule `69`, rpc `37` + `1` (prompts), tui `124` passed, `0` failed;
   `tui/tests/no_emoji` fails on the pre-existing `surface/src/slack.rs:177` (not 109's).
4. `cargo check --workspace --tests` — clean (run at 8baeae6c and again at fdfe43d7).
5. `cargo clippy -p agent -p tools --tests` — no warnings in any file the wave touched
   (`loop_`, `supervisor/*`, `store/*`, `durability/*`, `tools/src/{jobs,pty,registry,approval,ask,journal}.rs`,
   `desktop/src/launch.rs`).
6. `just ts-desktop` — bindings regenerated, `git status` clean (no tracked binding changed).
7. `bun run check` in `crates/desktop/ui` — `tsc -b --noEmit` clean (after `bun install
   --frozen-lockfile` there and in `packages/apps`, which this worktree lacked).
8. `bun run test` in `crates/desktop/ui` — `Test Files 146 passed (146)`, `Tests 848 passed (848)`.
9. `rustfmt --edition 2024 --check` on every touched `.rs` file — clean.

## 6. Not done / open for W6

- The drain-suspends-not-cancels path (§2) has no automated test yet beyond compiling and the
  `loop_` suite staying green; W6's "drain → suspended → resume" scenario covers it.
- `Loaded.resume.report` (above). `seen` dedup by `input_accepted.payload.input_id` (W1's open
  item) unchanged.

Files changed: `crates/agent/src/{loop_.rs, store/communication.rs}`,
`crates/agent/src/supervisor/{resume.rs, waits.rs, drain.rs}`,
`crates/agent/src/loop_/{resume_calls.rs, durability_tests.rs}`, `crates/desktop/src/launch.rs`,
`docs/design/109-as-built-I.md` (this note).
