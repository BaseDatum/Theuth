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

## Review findings E1–E7 (orchestrator request, 18:14)

- **E1 (external ACP approval dismissed → reject option): not changed; owner decision.** Confirmed by reading:
  `tools/src/registry.rs` `ExternalApproval` maps the `ApprovalResolved` to a bare `Decision`
  (`map_or(Deny, |r| r.decision)`, `dismissed` dropped); `agent/src/acp_child.rs` `ask_permission` → `outcome()`
  answers `Deny` with the harness's `RejectOnce`/`RejectAlways` option. The only alternative ACP offers is
  `RequestPermissionOutcome::Cancelled`, documented in `agent-client-protocol-schema` 1.13
  (`src/v1/client.rs:2394`) as "the prompt turn was cancelled before the user responded" — the answer a client MUST
  give after `session/cancel`; acp_child itself sends it only when the turn's cancel token fires. A harness
  following the spec therefore treats `cancelled` as the end of its turn, which would make a dismissal cancel the
  external agent's turn (the opposite of "leave it to the agent"). Not verified against a harness's own code (none
  in the tree). `PermissionOptionKind` has only allow/reject kinds and `Selected` carries no text, so the protocol
  has no channel for the neutral "not run, the user left this to you" wording. Per the instruction, the reject
  option stays; the owner should decide between "reject (harness may report it as denied)" and "cancelled
  (harness ends its turn)".
- **E2:** done as gap 4a (commit `9ec8f976e`), by filtering `left_to_agent:` out of the disposition `episode_closing`
  reads rather than by a new event kind; the only reader of the last disposition is `episode_closing`
  (`roster::desk::continued` uses `any`). The test covers the reviewer's scenario (agent said `continuing`, decision
  escalated with a live checkout, left live, episode closes → task stays `doing`, no `no continuation`, no wake).
  The event-kind change was not made.
- **E3–E7: not done** (time): parked left decision stays invisible in Attention and the Escalate/AwaitDecision race is
  open (E3); Signal/Slack/Telegram cards still render a dismissal as deny / "answered elsewhere" (E4); the double
  card's title still carries the countdown (E5); `score.rs` u32 arithmetic unchanged (E6); `RescoreTick` always
  gathers (E7).

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

## Review fixes (second integrator)

On top of `da2a3301c` (the UI merge `cc84cecca` plus the status above). Supersedes "Review findings E1–E7" for E1
and E3–E7. Gate script `/tmp/theuth-207-i2-gate.sh`, log `/tmp/theuth-207-i2-gate.log`.

| commit | what |
|---|---|
| `1cc93befd` | E1 |
| `adc6710a9` | E3 |
| `b69014ca7`, `2767738f0` | E4 (the second: a telegram test call the first missed) |
| `9da6240b9` | E5 |
| `9c7afba1c` | E6 |
| `0afafa5e7` | `207-attention-flat-list.md` §5.2 "Decided 2026-11-21" |
| (this doc's commit) | this section |

- **E1 fixed as far as ACP allows; the rest is an owner decision.** `ExternalApproval` now replies
  `tools::ExternalVerdict { decision, dismissed }` (`registry.rs`; a plain allow/deny is `ExternalVerdict::plain`),
  carrying the approval's `dismissed` instead of flattening it into `Deny`. `acp_child::outcome` answers a dismissal
  with the harness's reject option (`reject_once` first) and attaches
  `_meta.theuth = {left_to_agent: true, text: "not run: the user left this to you, choose another way or continue
  without it"}`; a plain deny carries no `_meta`. Not `cancelled`: `agent-client-protocol-schema` 1.13
  `src/v1/client.rs:2394-2402` defines `Cancelled` as "the prompt turn was cancelled before the user responded", the
  answer a client MUST give to pending permission requests after `session/cancel`, and acp_child itself sends it only
  when the turn's cancel token fires (`acp_child.rs` `permission`, the doc comment above it) — a harness following
  the spec ends the turn on it, which would make a dismissal stop the session. `SelectedPermissionOutcome` has only
  `option_id` and `_meta` (`client.rs:2415-2428`), and `_meta` is documented "Implementations MUST NOT make
  assumptions about values at these keys" — so no harness is obliged to show the text to its model.
  **An external harness that does not read `_meta.theuth` cannot be told the difference: it sees `reject_once`, as
  for a denial.** No harness source is in the tree, so how Claude Code / Codex / omp word a `reject_once` to their
  model was not verified. Owner decision: keep reject + `_meta` (the session goes on, the model may read it as a
  refusal) or use `cancelled` (the harness may end its turn). Tests
  `tools::registry::tests::dismissed_external_approval_is_marked_dismissed`,
  `agent::acp_child::tests::dismissed_permission_is_reject_once_marked_left_to_agent` (and the updated
  `permission_outcome_prefers_once_and_cancels_without_decision`).
- **E2:** confirmed, `tasks::ledger::tests::left_decision_does_not_change_episode_closing` exists (`ledger.rs`) and
  passed in hold 2.
- **E3 fixed.** `attention::derive`: a Blocked task whose blockers are all `left:` makes `blocked:<task>` with title
  "“<task>”: decision left to the agent" and detail `attention::LEFT_DECISION` ("Decision left to the agent; it will
  act when it next runs"). `score::left_decision(item)` (kind Blocked, id `blocked:`, that detail) makes
  `base_class` Notice and `base_text` "decision left to the agent"; `alert::class_of` gives it no alert. Dismiss is the
  existing hidden (versioned) path; Unblock is the existing blocked action (`left:` blockers are system blockers).
  No wake anywhere. Race: `AwaitDecision` first calls the new `Ledger::take_left_decision(decision)`; when a task
  carries `left:<decision>` (the leave landed after `Escalate`, before the wait subscribed, so it was recorded as
  parked) it drops that blocker, resumes the task when no blocker is left (as a live leave does; events under actor
  `system`, no wake) and answers `LEFT_ANSWER` at once. Tests `roster::attention::tests::left_blocker_is_a_quiet_notice`
  (replaces `left_blocker_makes_no_item`), `tasks::tests::leave_decision_reports_live_and_answers_the_waiter`
  (extended: a late `AwaitDecision` on the parked-left decision returns `LEFT_ANSWER` within 5 s and the task is
  unblocked). The score fixture had no `left:` item; no expectation changed. No UI label change: the chip stays
  "blocked", the detail line carries the text.
- **E4 fixed.** Signal: `reaction_for` returns `Option` and `None` for a dismissal (no reaction in that file reads as
  "left to the agent"); the channel sends "`<code>` left to the agent (not run, not refused)" instead; tombstones keep
  `dismissed`, so a late `/approve`/`/deny` reads "was already left to the agent …"; asks `closing_note` says
  "questions left to the agent". Slack `approvals::resolved_card` / Telegram `approvals::resolved` take `dismissed`:
  "**tool** left to the agent (not run, not refused)", no "by"; Telegram's disabled button "left to the agent".
  Slack `asks::closed_card` / Telegram `asks::closed` take `left_to_agent`: "**Questions left to the agent** (not
  answered; the turn goes on)". No new emoji. Tests `signal::approvals::tests::dismissed_approval_reads_left_to_the_agent`,
  `slack::approvals::tests::resolved_card_drops_buttons_and_names_user`, `slack::asks::tests::answered_and_closed_cards`,
  `telegram::approvals::tests::resolved_card_rewrites_the_verdict_line_and_disables_the_buttons`,
  `telegram::asks::tests::card_steps_through_the_questions_and_rejects_stale_presses` (each extended).
- **E5 fixed.** `double::actor` `item()` drops its `now` parameter; the grace title is "double answers <choice> for
  <what> — take over" (no seconds); `deadline_ms` / detail carry the deadline. Test
  `double::actor::tests::acts_at_l2_after_grace` (asserts no countdown in the title, `deadline_ms` set, detail =
  `will_act_at`).
- **E6 fixed.** `score.rs`: blocks and task points computed in u64, the total summed in u64, all through
  `clamp(u64) -> u32` (saturating at `u32::MAX`). Test `roster::score::tests::huge_weights_saturate` (every weight
  `u32::MAX`, `blocks = u32::MAX`, rank 199).
- **E7 not fixed (pushed back).** The 60 s `RescoreTick` is not only a rescore: `gather` is the only path that sees
  items with no event behind them — `mesh::Status` (node / home offline), `secrets::ListRequests` (Attention has no
  secret-request handler), `tasks::HomesOffline`, the registered `Source`s (double, a2a contacts, autoresearch;
  `RegisterSource` only pushes to `self.sources`, polled in `gather`), budgets via `ListAgents`, and snooze expiry
  (`derive` filters `snoozed_until_ms > now`). Skipping the gather when the last list has no `deadline_ms` /
  `expires_ms` would leave those unseen until some unrelated event. A safe version needs those producers to notify
  Attention first; not done.

### Gates (second integrator)

Hold 1 (19:12:32–19:17:24, tree `adc6710a9`):

| step | rc | counts |
|---|---|---|
| clippy `-D warnings --all-targets` roster tools tasks agent (+ the two allows) | 0 | |
| `cargo test --no-fail-fast` roster tools tasks double | 101 | not counted: roster did not compile — my E6 edit (`clamp` not yet defined) landed in the working tree while the hold was building. Re-run in hold 2. |
| `cargo test -p agent --lib acp_child` | 0 | 53 passed |
| `cargo test -p agent --lib supervisor::waits` | 0 | 6 passed |

Hold 2 (19:18:42–19:34:09, tree `9c7afba1c`, plus `2767738f0` for the telegram test steps):

| step | rc | counts |
|---|---|---|
| clippy `-D warnings --all-targets` roster double signal slack telegram | 101 | only error: `telegram` lib test did not compile (a `resolved(...)` test call missed by E4); fixed in `2767738f0`; no lint from roster, double, signal, slack |
| `cargo test --no-fail-fast` roster tools tasks double | 101 | roster 130/130, tasks 124/124, double 81/81, tools 294 passed 1 failed 3 ignored: `git::tests::clone_pull_and_list_against_a_local_bare_repo` (`gpg failed to sign the data`, pinentry cancelled — the machine's global `commit.gpgsign`, as A reported) |
| `cargo test -p signal --lib approvals` / `asks` | 0 / 0 | 9 / 4 passed |
| `cargo test -p slack --lib approvals` / `asks` | 0 / 0 | 5 / 5 passed |
| `cargo test -p telegram --lib approvals` / `asks` | 0 / 0 | 4 / 5 passed |
| `just wire-schema` | 0 | 1 passed; no tracked file changed (`git status` clean outside this doc) |
| classifier (`cargo test -p theuth --test wire_schema_snapshot`) vs the accepted baseline | 0 | 1 passed, no difference reported (0 `class:` lines); nothing accepted or regenerated |

Hold 3 (19:59:38–20:02:28, tree `559f45859`; a first launch at 19:34:58 waited on another worker's lock and was
killed by the engine restart at 19:59:31 before it started): clippy `-D warnings --all-targets` telegram rc 0 (after
`2767738f0`); in `crates/desktop/ui`: `bun run check` (`tsc -b --noEmit`) rc 0; `bun run test` rc 0 — 381 files,
2736 tests passed; `bun run build` rc 0 (only the existing > 500 kB chunk warning). The three merged UI commits
type-check and pass against the generated bindings; no UI source was changed. `Card.tsx` `SecretInline` still passes
main's `title` / `eventFields(local)` / `expires` (read at lines 154-168).

### Not verified

- Nothing ran against a live engine; nothing was seen in the Tauri app.
- How an external harness (Claude Code, Codex, omp) reacts to `reject_once` + `_meta.theuth`: no harness code in the
  tree; reasoning from the ACP schema only.
- Signal/Slack/Telegram changes are tested on their pure card/text functions; the channel call sites compiled
  (clippy / test builds) but no channel-level test drives a dismissed `ApprovalResolved` / left `AskResolved`.
- E3's `AwaitDecision` race test drives the actor in order (leave, then wait); the real interleaving with the
  `escalate` tool was not reproduced.

### Process slips

- Hold 1's four-crate test step compiled a half-edited `score.rs` (E6 in progress): I edited a crate under test while
  the hold ran. After that, only crates outside the running step were edited during a hold (the telegram test fix
  while hold 2 ran the four-crate tests).
