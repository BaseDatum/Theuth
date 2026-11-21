# 207 as built — builder A (Rust attention engine)

Branch `feat/207-attention-be`, base `d81a7d3de` (main `729093ffc` + the 207 docs). Contract: `docs/design/207-contract.md`.

## Wire shape (§7) — no change from the contract

Every type, field, serde attribute, method name and resolution value of contract §2 and §5.3 is implemented as written, so the TypeScript of §7 stands as specified. Additive only: `AttentionItem` gains 13 `#[serde(default)]` fields, `AttentionChanged` 5, `ApprovalResolved.dismissed`, `AskResolved.left_to_agent`, `Packet.left_to_agent_ms`; new definitions `AttentionSignal`, `AttentionReason`, `StopOutcome`, `AttentionStop`, `AttentionTarget`, `AttentionFilter`, `AttentionResolution`, `AttentionOutcome`; new methods `attention_dismiss_many`, `attention_snooze_many` (owner `roster`). No `AttentionKind` variant, no `sessions` column, `SESSIONS_WIRE_SCHEMA` untouched. Classifier result: see Verification.

`attention_dismiss` became `with_extensions` (server-side only; its params and result are unchanged).

## Deviations from the contract (code wins)

1. Ownership (§1 vs §2.4). §2.4 requires every `ApprovalResolved { … }` / `AskResolved { … }` literal in the workspace to gain the new field; some sit in files §1 does not list: `crates/sessionsync/src/events.rs`, `crates/signal/src/{approvals.rs, channel.rs}`, `crates/slack/src/channel.rs`, `crates/telegram/src/channel.rs`, `crates/tui/src/model/tests.rs` (`dismissed: false` / `left_to_agent: false`, nothing else), and `crates/tasks/src/tools.rs` (`left_to_agent_ms: None` in a `Packet` literal). Without them the workspace does not compile.
2. `AttentionSignal` derives exactly the contract's list (no `Ord`); the reason sort ties on the declaration order through `signal as u8`.
3. `StopOutcome` gains inherent helpers `of(stop_reason)`, `as_str()`, `parse()` (not on the wire). `attention::stop_card(stop_reason) -> Option<StopOutcome>` (None for `interrupted` / `suspended`) is the testable form of §4's rule.
4. `alerts_changed` (§5.5) is also set when an item that carried an alert appears or leaves the list ("an alert appeared or went away"), not only when an alert differs on an id present in both lists.
5. Dismissing an approval also drops it from the actor's pending map at once, so the batch's single `AttentionChanged` already lacks it (the `ApprovalResolved` that follows changes nothing). The activity row's subject is the item's session (the approval id is in the text).
6. `wake_text` (§2.4): the "The user left …" lines are inside the `<task>` record, right after the `Blockers:` line, one per `left:` blocker.
7. The `Blocked` item's detail still lists every blocker (a `left:` one included) when another blocker makes the item; only the item condition ignores `left:` blockers, as §3.5 says.
8. `lineage_map` takes a desk root's title first and uses `<desk> desk` only when the title is empty, exactly as §3.5 orders it (the §9 fixture's `ops desk` assumes an untitled desk root).
9. Deadlines (§3.5) are cached per item id on first sight; the journal row (`question_get`) is read once per id, the cache is retained to the pending ids on every gather.
10. The rescore tick is a spawned `tokio::time::interval` holding a weak actor ref; it stops with the actor.
11. tools (worker notes): `approval::Dismiss` returns true when the verdict reached the parked call (same rule as the existing resolve), which equals "was pending" except when the waiter already left. `ask.rs` also resumes a journal row answered `{"left_to_agent": true}` as left to the agent (otherwise a replay would read it as a dismissal). `outcome()` builds `ToolOutput::ok(..)` then sets `structured` (no builder exists).
12. tasks (worker notes): `leave_decision` writes the row itself instead of going through `update` (which wakes); it therefore skips `charge` / `check_epoch` and does not reset the no-progress back-off. `left:` blockers count as system blockers (`system_blocker()`), so a manual unblock clears them. Risk: the `left_to_agent:` disposition becomes the task's last disposition; `episode_closing` only spares tasks whose last disposition starts with `continuing`, so a live task that said `continuing` and then had a decision left to it could be blocked `no continuation` at episode close.
13. secrets: there were no outcome constants (`stored` / `declined` / `timeout` are literals); `PROCEED_WITHOUT` sits beside `Outcome`. The journal row of a waived request records `{"outcome": "proceed_without"}`.
14. double: the card's arrival is the existing `PendingCard.asked_at`; no `since` field was added. a2a: the first-contact item's hard-coded `rank: 5` went too (with the roster-alert item's `rank: 4`).
15. Formatting: `cargo fmt --all` rewrites about 250 files on the base (the `tui` crate, `rpc-compose/src/api/tests.rs` and a few others are not rustfmt-clean on main). Only this branch's lines are formatted; the unrelated reformatting was reverted, so `cargo fmt --all --check` still reports those pre-existing files.

## Out-of-list files (§1) — checked by the continuation

`git diff d81a7d3de` over `crates/sessionsync/src/events.rs`, `crates/signal/src/{approvals,channel}.rs`, `crates/slack/src/channel.rs`, `crates/telegram/src/channel.rs`, `crates/tui/src/model/tests.rs`, `crates/tasks/src/tools.rs`: every hunk is one added `dismissed: false` / `left_to_agent: false` / `left_to_agent_ms: None` line in a struct literal (all in test modules except the `Packet` literal in `tasks/src/tools.rs`). No wording, match arm or behaviour changed; nothing reverted.

## Dismissal never wakes, never reads as a denial (§5.2) — by reading and by test

- approval: `tools::approval::Dismiss` sends `Verdict::Dismissed`; the waiter publishes `ApprovalResolved { decision: Deny, dismissed: true }` and `registry.rs` matches `r.dismissed` before any deny/timeout arm, returning the not-run text (no "denied"). Replay: `waits.rs` maps the journal answer `"dismissed"` to `HonourAnswer { dismissed: true }`. Tests `tools::approval::tests::dismiss_leaves_the_call_to_the_agent`, `dismissed_answer_honoured_on_replay`, `tools::registry::tests::dismissed_approval_is_not_run`.
- ask: `LeaveAsk` resolves the waiter with `ToolOutput::ok`; it never goes through `agent::answer_ask`, the only path that cancels the turn; the event has `dismissed: false`. Test `tools::ask::tests::leave_ask_hands_the_questions_back`.
- decision: `Ledger::leave_decision` calls `say`, `put_packet`, `event`, `save` and (live, no blocker left) `resume` — none of them calls `wake`. Tests `tasks::ledger::tests::leave_decision_live_unblocks_without_a_wake`, `leave_decision_parked_keeps_a_left_blocker` (assert no `wakes` row).
- secret / hand-off: `Waive` and `handoff::resolve(.., PROCEED_WITHOUT, ..)` only resolve the tool's own waiter with ok text. Tests `secrets::tests::waive_proceeds_without`, `secrets::tools::tests::waived_request_says_proceed_without`, `roster::handoff::tests::proceed_without_is_an_ok_result`.

## Tests added by the continuation

- `roster::attention::tests::waiting_since_for_approvals_and_asks`: an ask's `at_ms` is stable across derives at different `now_ms` (§11.1 "ask `at_ms` stable across derives").

## Verification

Gate script `/tmp/theuth-207-a-gate.sh test` (17:15–):
- `cargo clippy -p tui --all-targets -- -D warnings -A clippy::large_enum_variant …`: rc 0.
- `cargo test -p roster -p tools -p tasks -p secrets -p double -p rpc-compose`: stopped at `double` (cargo without `--no-fail-fast`): 80 passed, 1 failed — `actor::tests::take_over_cancels_grace_and_override_demotes` (`actor.rs:1876`, see pre-existing below).
- `cargo test -p agent --lib supervisor::waits`: 6 passed.
- `cargo test -p autoresearch --lib actor`: 59 passed.
- `cargo test -p a2a --lib contacts`: 122 passed, 1 failed — `contacts::e2e_tests::forward::a_call_for_a_session_owned_elsewhere_is_carried_to_its_owner` (see pre-existing below).

Gate script `/tmp/theuth-207-a2-gate.sh final` (17:37–, on the tip of the split history):
- `cargo clippy --keep-going -p roster -p tools -p tasks -p secrets -p double -p autoresearch -p a2a -p agent -p rpc-compose -p sessionsync -p signal -p slack -p telegram --all-targets -- -D warnings -A clippy::chunks_exact_to_as_chunks -A clippy::single_match`: rc 0.
- `cargo test --no-fail-fast -p roster -p tools -p tasks -p secrets -p double -p rpc-compose`: roster 127/127, tasks 123/123, double 81/81 (the take-over test passed this time), rpc-compose lib 65/65 + every integration test except two, secrets 44/45, tools 279/280 (3 ignored). Failures, all pre-existing (below): `rpc-compose --test wire_golden legacy_event_families`, `rpc-compose --test wire_parity_inc4 agent_golden_parity`, `secrets store::tests::concurrent_first_opens_all_succeed`, `tools git::tests::clone_pull_and_list_against_a_local_bare_repo`.
- `just wire-schema` (`cargo test -p theuth --bin t --quiet export_bindings`): 1 passed. Classifier (`cargo test -p theuth --test wire_schema_snapshot` against the old baseline): 48 Additive, 6 Nonbreaking, no ReviewRequired / Breaking; every path is a 207 one (`attention_dismiss_many`, `attention_snooze_many`, the 13 `AttentionItem` properties on `attention_list` output and receiver schemas, `packet_get` `left_to_agent_ms`, the five `roster.attention@1` properties, `tools.approval-resolved@1` `dismissed`, `tools.ask-resolved@1` `left_to_agent`; Nonbreaking = descriptions). Accepted by copying `bindings/openrpc.json` to `crates/theuth/tests/fixtures/wire/openrpc-v1.json` (the body of `just wire-schema-accept`, whose `wire-schema` prerequisite had just run).

## Not done / not verified

- `just ts-desktop` not run (orchestrator's step; UI bindings are git-ignored).
- Only crate-scoped gates ran (shared machine): no `just clippy` / `just test` over the workspace. Crates whose sources changed only by a struct-literal field (`sessionsync`, `signal`, `slack`, `telegram`, `tui`) were clippy-built with `--all-targets` (test code compiled) but their tests were not run.
- No actor-level test drives `DismissMany` through a running `Attention` actor with live `Approval`/`AskUser`/tasks/secrets actors; each resolver is tested in its owning crate and the dispatch (`dismiss_one`) was checked by reading. "One `AttentionChanged` per batch" rests on `DismissMany` calling `recheck_quietly` once after the loop (reading) and the pure diff test `attention_changed_tracks_ids_order_content_and_alerts`.
- `TurnStarted` deletion and the TTL prune are tested on the store helpers (`clear_finished`, `prune_finished` in `stopped_rows_roundtrip_expire_and_clear`), not through the actor's `AgentEvent` handler / `RescoreTick`.
- Deviation 12's risk (a `left_to_agent:` last disposition vs `episode_closing`) is not covered by a test.
- History: the checkpoint was split into five commits by area; the intermediate commits were not built one by one (only the tip was gated).

## Pre-existing reds (not caused by this branch, not fixed)

- clippy `large_enum_variant` at `crates/tui/src/components/mcp.rs:24`: the branch touches only `crates/tui/src/model/tests.rs` (two literal fields); tui is clippy-gated with `-A clippy::large_enum_variant`.
- `double::actor::tests::take_over_cancels_grace_and_override_demotes` (panics at `actor.rs:1876`, the `class.rung == 1` assert; the full panic text was not kept): `owner_answered` writes the label, then `reladder` writes the rung; the test waits for the label (`labels_when`) and reads the class at once, so under load it can read before the demotion (inference from reading, a timing race). Neither function nor the test is in this branch's diff (the branch's double changes are the `dismissed`/`left_to_agent` early returns, `drop_pending` and `item()`), and this path is a plain `Resolve { Deny }` with `dismissed: false`.
- `a2a::contacts::e2e_tests::forward::a_call_for_a_session_owned_elsewhere_is_carried_to_its_owner` (panics at `e2e_tests/forward.rs:295`, the `OwnerOffline` assert after an `Answer::Silent` owner; it depends on the forward's deadline beating the caller's own timeout, a wall-clock race under a loaded machine — inference from reading). The branch's only a2a change is item construction in `contacts.rs` (`AttentionItem::new`, `rank` literals dropped); `forward.rs` and the forwarding code are untouched.
- `rpc-compose` `wire_golden::legacy_event_families` and `wire_parity_inc4::agent_golden_parity`: `agent.events[37]` `SubagentSpawned` now serialises `agent` and `model` (added on main by `ddcf409f9 fix(agent): roster rows carry the child's real model`) and the agent golden fixture lacks them. The branch touches neither `SubagentSpawned` nor the agent fixtures.
- `secrets` `store::tests::concurrent_first_opens_all_succeed`: `database is locked` (SQLite code 5) opening a temp vault concurrently; `store.rs` is not in the diff.
- `tools` `git::tests::clone_pull_and_list_against_a_local_bare_repo`: `gpg failed to sign the data` — the test commits with the machine's global `commit.gpgsign`; environment, `git.rs` not in the diff.
- `cargo fmt --check` on the touched files reports only hunks outside this branch's lines (module order in `secrets/src/lib.rs:19`, `rpc_support.rs`, `rpc-compose/src/api/tests.rs`, …); none overlaps a changed line.
