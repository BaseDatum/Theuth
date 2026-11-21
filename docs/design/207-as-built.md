# 207 as built — integrated (A engine + C UI + main)

Branch `feat/207-attention`, worktree `theuth-attention`. Builder notes: `207-as-built-A.md`, `207-as-built-C.md`.
Nothing here has been run against a live engine or seen in the Tauri app.

## History

| commit | what |
|---|---|
| `faa987b51` | C (UI) and A (engine) merged on base `729093ffc` (before this round) |
| `7c46e0cc3` | merge of main (`012e280eb`, ~50 commits) — resolution below |
| `5ac9d5ada` | secrets: main's `receipt_lines` test carries 207's `waived` field and the `names` argument |
| `9ec8f976e` | tasks: leaving a decision never changes what episode closing does (gap 4a) |
| `1bb6a9961` | roster: actor-level tests for `DismissMany` and the stopped card (gaps 4b, 4c) |
| `3827cb97d` | merge of main again (`0350ea609`, 2 commits: plus-menu skills) — clean |
| `760df2fc2` | wire: the 207 openrpc baseline accepted on main `0350ea609` |
| (this doc's commit) | UI test literals for the generated bindings; this document |

## Merge resolution

- `crates/secrets` (main: secret-request expiry and multi-field `parse_request`/`run`/`receipt`; 207: `Waive` →
  `proceed_without`). `Outcome` keeps both `waived` (207) and `expires_ms` (main); `wait()` sets both. Main routed
  both request tools through one `run()` → `receipt()`; `run()` now also joins the requested field names and
  `receipt()` passes them to 207's `not_stored(out, timeout, names)`, so a waived request (single value or form) still
  answers ok "no secret is coming: the user left this to you — proceed without …". The resumed-row `Outcome` literal
  carries `waived: false, expires_ms: None`. 207's `waived_request_says_proceed_without` now goes through
  `receipt()`; main's `receipt_lines` got `waived: false` and the `names` argument.
- `crates/desktop/ui`: `hud/Hud.tsx` keeps C's split (main's only change there — `SecretRequestCard` gaining `title`,
  `eventFields(local)` and `expires` — was ported into the moved code: `attention/Card.tsx` `SecretInline` and
  `attention/PlainScreens.tsx` `SecretScreen`). `PlainScreens.tsx` imports: C's (`./actions`, `./labels`, `./Reply`)
  plus main's `eventFields`.
- Auto-merged files both sides touched, re-checked by reading: `telegram/src/channel.rs` (main added `expires: None`
  to two `SecretRequested` test literals; 207 added `dismissed`/`left_to_agent` to others — disjoint),
  `tools/src/{lib,registry}.rs` (main: builtins/`HarnessInvoke`; the 207 `r.dismissed` arm in the approval match is
  intact and `HarnessInvoke` goes through the same path), `tui/src/model/tests.rs`, `roster/src/attention.rs` (main:
  `note`/`expires` on a test `SecretRequest` literal). Main added no `AttentionItem` / `ApprovalResolved` /
  `AskResolved` / `Packet` literal or match (grep over main's diff); the compiler agreed.
- `crates/theuth/tests/fixtures/wire/openrpc-v1.json`: taken from main verbatim (not hand-merged), then regenerated
  and accepted (below).

## Gates

All under `/tmp/theuth-build.lock` through `/tmp/theuth-207-i-gate.sh`, `CARGO_BUILD_JOBS=4`, `--test-threads=4`.

Engine hold (18:21:48–18:46:18, tree `7c46e0cc3` + the step-4 edits):

| step | rc | counts |
|---|---|---|
| clippy `-D warnings --all-targets` roster tools tasks secrets double autoresearch a2a agent rpc-compose sessionsync signal slack telegram (`-A clippy::chunks_exact_to_as_chunks -A clippy::single_match`, as A ran it) | 101 | only error: `roster` lib test did not compile — the new test named `tools::…`, which inside `roster` is the crate's own `tools` module; fixed to `::tools::…` and re-run green below. No other crate had a warning. |
| clippy tui (`-A clippy::large_enum_variant` + the two above) | 0 | |
| `cargo test --no-fail-fast` roster tools tasks secrets double rpc-compose | 101 | roster 129/129 (2 new), tools 294/294 (3 ignored), tasks 124/124 (1 new), double 81/81, rpc-compose lib 65/65 + integration tests all green except `wire_golden::legacy_event_families` and `wire_parity_inc4::agent_golden_parity`; secrets 63/64 (`store::tests::concurrent_first_opens_all_succeed`) |
| `cargo test -p agent --lib supervisor::waits` | 0 | 6 passed |
| `cargo test -p autoresearch --lib actor` | 0 | 59 passed |
| `cargo test -p a2a --lib contacts` | 101 | 122 passed, 1 failed (`e2e_tests::forward::a_call_for_a_session_owned_elsewhere_is_carried_to_its_owner`) |

Wire hold (18:52:56–19:01:37, tree `3827cb97d` = after the second main merge):

| step | rc | counts |
|---|---|---|
| `just wire-schema` | 0 | 1 passed |
| classifier (`cargo test -p theuth --test wire_schema_snapshot`) vs main `0350ea609`'s accepted baseline | 101 (snapshot differs, `report.fails()` false) | 44 Additive, 6 Nonbreaking, 0 ReviewRequired, 0 Breaking. Paths: `attention_dismiss_many`, `attention_snooze_many`; `attention_list` result (schema and receiver schema): `blocks deadline_ms expires_ms parent_session path project reasons risk root_session score session_live stop tool` + `id` description; `packet_get` `left_to_agent_ms`; `secret_requests_list` `outcome` description; `roster.attention@1` `alerts_changed epoch_ms items order seq` + description; `tools.approval-resolved@1` `dismissed`; `tools.ask-resolved@1` `left_to_agent`. Nothing outside 207. |
| accept (`cp bindings/openrpc.json …/openrpc-v1.json`, the body of `just wire-schema-accept`, whose prerequisite had just run) | 0 | |
| classifier after accept | 0 | 1 passed |
| `just ts-desktop` body without its `bun install` (node_modules were APFS clones from `theuth-attention-ui`): `bun run wire:generate`, then the recipe's `rm`/`cp` into `src/rpc/bindings` | 0 / 0 | generated names match contract §7 (checked by grep: every §7.1/§7.2 type, `client.ts` `MethodMap`/`METHODS`/`ReceiverResults`, `AttentionChanged` fields, `dismissed`/`left_to_agent` required in the outputs) |
| clippy `-D warnings --all-targets` roster tasks secrets agent (post-merge) | 0 | |
| `cargo test --no-fail-fast` roster tasks secrets (post-merge) | 101 | roster 129/129, tasks 124/124, secrets 63/64 (same `concurrent_first_opens_all_succeed`) |

UI hold: see "UI on generated bindings".

## Gaps closed (step 4)

a. **Episode closing vs a left decision** (A's deviation 12 risk). `Ledger::leave_decision` writes a `disposition`
   event `left_to_agent:<decision>`; `episode_closing` read the *last* `disposition` and spared only `continuing…`,
   so a live task whose agent had said `continuing` was blocked `no continuation` once a decision was left to it.
   Fix (`crates/tasks/src/ledger.rs`, `episode_closing`): look for the last `disposition` that is not
   `left_to_agent:`. `roster::desk::continued` (the turn-end nudge) uses `any(...)` and was not affected. Test
   `tasks::ledger::tests::left_decision_does_not_change_episode_closing`: two tasks of `lead` checked out by `s-lead`,
   each escalated to the user and left live; the one whose agent said `continuing` stays `doing`, the silent one is
   blocked (exactly as without the leave). Without the fix the `continuing` task would be in the blocked list too
   (by reading; the pre-fix run was not executed).
b. **`DismissMany` through live actors.** `roster::tests::dismiss_many_mixed_batch_publishes_once_and_wakes_nobody`
   (in `crates/roster/src/lib.rs`, under the module's `with_engine_lock`): real `Approval` and `AskUser` actors
   registered under their names, `agent::spawn_test_agents`, `Attention::start`; an approval request and an ask
   parked by spawned callers, a `TurnFinished` for a stopped card. One `DismissMany` over the three ids returns
   `not_run` / `left_to_agent` / `hidden`; the approval's caller gets `dismissed: true` (not a plain deny); the ask's
   caller gets `left_to_agent: true`, no answers, not timed out (the turn is not cancelled: the call returns ok); a
   subscriber counts exactly one `AttentionChanged` across the batch and the trailing
   `ApprovalResolved`/`AskResolved`, and zero `AgentEvent`s (no turn started or cancelled). "No wakes row" is not
   observable here (no tasks actor in this rig); for decisions it is pinned by A's
   `leave_decision_*_without_a_wake` tests.
c. **Stopped card through the actor.** `roster::tests::stopped_card_through_the_attention_actor`: a `TurnFinished`
   (`end_turn`) for a root session yields one `finished:` item with `stop` (`finished`, `end_turn`) and
   `expires_ms = at_ms + stopped_ttl_mins`; a `TurnStarted` for the session removes it; a second card survives a
   `RescoreTick` while fresh, and after its row is back-dated past the TTL the tick deletes the
   `attention_finished` row (row count 0, not only filtered from the list).

## UI on generated bindings (step 3)

The generated bindings (`bun run wire:generate` on the merged tree, copied into the git-ignored
`crates/desktop/ui/src/rpc/bindings/`) carry every §7 name and shape C hand-patched; no UI source change was needed
for a name or shape mismatch, and no Rust shape contradicts §7. With `dismissed` / `left_to_agent` now required in
the `*.output.ts` events (C's deviation 1), the two literals outside C's ownership gained them:
`src/components/chat/ApprovalCard.test.ts:50` (`dismissed: false`) and `src/store/approvals.test.ts:50`
(`left_to_agent: false`); no other `ApprovalResolved` / `AskResolved` literal exists in `src/` (grep).
`receive.ts` is the generated one (C's deviation 4 still holds: it fills no defaults for the new item fields).

UI hold (19:05:38–19:06:08), in `crates/desktop/ui`, node_modules APFS-cloned from `theuth-attention-ui` (no
`bun install`): `bun run check` rc 0; `bun run test` rc 0 — 381 files, 2728 tests passed; `bun run build` rc 0 (only
the existing > 500 kB chunk warning).

## Not done / not verified

- Nothing ran against a live engine; nothing was seen in the Tauri app (C's visual pass was headless Chromium on the
  dev fixture).
- Crate-scoped gates only; no workspace `just clippy` / `just test`.
- `sessionsync`, `signal`, `slack`, `telegram` were clippy-built with `--all-targets` (tests compiled) but their tests
  were not run; `tui` likewise.
- The second main merge (`3827cb97d`, plus-menu skills: `agent`, UI composer) landed after the engine gate; it was
  covered by the wire gate's `theuth` build, the post-merge clippy/test of `roster tasks secrets (agent clippy)` and
  the UI gate, not by a re-run of `a2a`/`autoresearch`/`agent` tests.
- Gap 4a's test was not run against the pre-fix code.

## Reds (all pre-existing; evidence by reading main)

Seen in this round, each with the branch's relation to it:

- `secrets` `store::tests::concurrent_first_opens_all_succeed` — `database is locked` (SQLite 5) at `store.rs:141`
  opening one temp vault from several tasks at once; failed in both holds. `git diff main HEAD -- crates/secrets/src/store.rs`
  is empty: the code under test is main's; 207 touched only `lib.rs`/`tools.rs` (Waive, outcome text). Timing under a
  loaded machine (inference).
- `rpc-compose` `wire_golden::legacy_event_families` and `wire_parity_inc4::agent_golden_parity` — `agent.events[37]`
  `SubagentSpawned` serialises `agent: null, model: null`, the fixture lacks them. Main's
  `crates/rpc-compose/tests/fixtures/wire/agent.events.json` (line 1062) has the event without `agent`/`model`, and
  main's `SubagentSpawned` has `agent: Option<String>` / model fields; the branch changes neither the fixture nor the
  event (`git diff main HEAD -- …/agent.events.json` empty). Fails on main's code.
- `a2a` `contacts::e2e_tests::forward::a_call_for_a_session_owned_elsewhere_is_carried_to_its_owner` — at
  `forward.rs:295`: got `"contact answered HTTP 429 Too Many Requests"`, expected `"the chat's machine is offline"`.
  `e2e_tests/` and the forwarding code are not in the branch's diff against main; 207's a2a change is item
  construction in `contacts.rs` only. A wall-clock/rate race (inference), as A reported.
- clippy `large_enum_variant` (tui, `components/mcp.rs:24`) — not re-checked without the allow; the file is identical
  to main's.
- New this round, not from 207: clippy `chunks_exact_to_as_chunks` in `crates/voice/src/history/mod.rs:465` (a lint of
  the current toolchain, `voice` untouched by the branch) — allowed in the gate as A did, with `single_match`.

No longer red: `tools` `git::tests::clone_pull_and_list_against_a_local_bare_repo` passed (294/294), and `double`
`take_over_cancels_grace_and_override_demotes` passed (81/81).

## Process notes

- The worktree's `target/` was APFS-cloned from `theuth-attention-be`; build scripts that cache absolute paths
  (`mlx-sys`, `opusic-sys`, `apple-metal`, `screencapturekit`, `apple-cf` — CMake caches and Swift module caches
  pointing at the other worktree / the main checkout) failed on first use. Their `target/debug/build/<crate>-*` dirs
  were deleted and rebuilt. Two engine holds were lost to this (killed early, lock released by the script's trap).
- The gate script was edited while a hold was running; bash reads scripts incrementally, so that hold ran the
  (bindings-less) `bun test` / `bun build` lines after its last cargo step (both failed, harmless: no bindings yet,
  `tsc -b` failed before `vite build`). Those two results are not counted.
- One launch came 20 s (not 45 s) after my own release at 18:19:48; the lock was held by another worker at the time,
  so the hold itself started at 18:21:48.
