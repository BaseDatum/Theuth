# 106 — as built (DECISION-FIXES: crates/tasks, crates/roster/src/attention.rs)

Branch `fix/106-decision-fixes`, base `27912810`. Brief: `/tmp/mc-2026-10-05/omp-106-DECISION-FIXES.md`. Spec: `docs/design/101-live-proof-2026-10-06.md` F-2, F-3, F-4. One code commit (`6d27d4a9`) plus this note. No UI, rpc or tools code changed.

## Fix 1 — F-4: an `Err` reply to a `tell` stopped the Tasks actor

- `crates/tasks/src/lib.rs:493–516` — `#[derive(kameo::Actor)]` replaced by a manual `impl Actor for Tasks` (`Args = Self`, `Error = Infallible`, `name() = "Tasks"` so the lifecycle span keeps its name, `on_start` = identity). `on_panic` (`:508`) logs `tracing::error!(%err, "tasks actor: handler failed; continuing")` and returns `Ok(ControlFlow::Continue(()))` — signature from `kameo-0.22.2/src/actor.rs:262–284` (`WeakActorRef<Self>, PanicError → Result<ControlFlow<ActorStopReason>, Self::Error>`).
- `lib.rs:834–847` — `ask_user` sends `AnswerDecision` with `ask`, not `tell`; an `Err` is logged at warn: `stale ask card: decision already answered elsewhere` with the decision key and the error text.
- `lib.rs:850–874` `drop_card(decision)` + `lib.rs:905` — the `AnswerDecision` handler spawns it after a successful `answer_decision`: it asks the ask actor for `PendingAsks`, and every pending request whose question id equals the decision key (the card's question id *is* the decision key, `lib.rs:802`) is resolved with `AnswerAsk { answers: None }` — the public dismiss message, which resolves `ask_user`'s waiter with no answer (so it sends nothing) and publishes `AskResolved { dismissed: true }` so UIs drop the card. Fixes F-1's stale Question row as a side effect.
- **Tell audit** (`.tell(` across `crates/`, messages whose handler is on `Tasks`): `Dispatch`, `Sweep`, `Delivered`, `SetEngine` (tasks internal, `lib.rs`), `ReleaseClaims`, `FinishWake`, `ReleaseWake` (`crates/roster/src/lib.rs:419, 685, 750`) — all `Reply = ()`, infallible. `AnswerDecision` at the old `lib.rs:807` was the only fallible tell; switched to `ask`. No other crate tells into `Tasks` (`mesh`, `theuth/main.rs` tells target other actors).

## Fix 2 — F-2: the inbox showed the first decision, not the user's

- `crates/tasks/src/packet.rs:229–233` — `Packet.to: Option<String>`, `#[serde(default)]`; rows written before 106 read `None` (packets are a JSON blob, no migration). `bindings/Packet.ts:75`: `to: string | null, };`.
- `crates/tasks/src/ledger.rs:1527` — `escalate` sets `to = manager.to_string()` (`Owner` displays as `user` | `agent:<name>`). `ledger.rs:1612` — `review_packet` sets `to = effective_reviewer(&t, None)` (the task is already in review with its reviewer resolved when the packet is stored, `tools.rs:599–608` → `:717–719`).
- `crates/tasks/src/tools.rs:775` — the validation-only literal in `review_gate` gets `to: None` (never stored).
- `crates/roster/src/attention.rs:330–367` — `derive` walks the task's `decision:` blockers newest-first and takes the first whose packet has `to == "user"`, or no `to`, or no stored packet (legacy / packet-less rows); a packet with `to == "agent:…"` is skipped. Item id / `decision` / `packet` / detail follow the chosen decision; the title names the packet's `by` (`attention.rs:356`, falls back to the owner without a packet) so a forward reads "agent:proofmgr asks …". `attention.rs:368` — the Blocked item is made only when a non-`decision:` blocker exists (same set as before: previously "no decision blocker and blockers non-empty"), so a task blocked solely on agent-addressed decisions makes no item at all.

## Fix 3 — F-3: a parked forwarder was never re-woken

- `crates/tasks/src/ledger.rs:1684–1690` — after `answer_decision` clears the blocker: when the answered packet's `by` parses as `agent:<x>`, `agent:<x> != t.owner` and `by != actor`, `wake(&t, agent:<x>, Answer, actor)` is raised (`wakes` row, `wake` event, `woke <x> (answer)` thread line). The owner wake on an empty blocker list (`update`, `ledger.rs:1063–1068`) is unchanged.

## Docs

- `docs/design/103-decision-packets-v3.md:26` — "Revision 1 (106)" paragraph: `Packet.to`, the inbox rule, the forwarder wake, the tell→ask rule.

## Tests

- `tasks::tests::stale_card_and_fallible_tell_leave_the_actor_alive` (`crates/tasks/src/lib.rs:1419`): real `AskUser` actor registered as `ASK_ACTOR` + real `Tasks` actor over the ledger rig; `Escalate` by `agent:lead` (manager = user) → one card in `PendingAsks` whose question id is the decision; `AnswerDecision` (`decide` path) → the card leaves `PendingAsks`, blockers empty; a second `ask_user` for the already-answered packet (the F-4 race, deterministic) answered through `AnswerAsk` → `ask_user` returns, the actor still answers `GetTask`; then `tell(AnswerDecision { "decision:nope" })` → `on_panic` → the actor still answers `GetTask`.
- `ledger::tests::escalate_forwards_the_parent_packet_with_a_recommendation` extended (`crates/tasks/src/ledger.rs:5178–5257`): `d2.to == "user"`, `d1.to == "agent:lead"`; wakes drained (dispatch + finish), the user answers d2 → wakes are exactly `[("lead", Answer)]` (no owner wake, d1 still blocks); lead answers d1 → an `ops`/`Answer` wake, none for lead.
- `roster::attention::tests::decision_item_is_the_newest_one_addressed_to_the_user` (`crates/roster/src/attention.rs:1278`): forward chain (d1 → agent:lead, d2 → user) → one Decision item with id `decision:<task>:decision:d2`, `decision`/`packet` = d2, title `agent:lead asks …`; agent-audience decision alone → no item; agent-audience decision + `no continuation` → the Blocked item; legacy packet without `to` behind an agent-addressed one → item for the legacy decision.
- Fixtures: `to: None` added to the `Packet` literals in `lib.rs`, `packet.rs`, `attention.rs` tests.

## Gates (this worktree, toolchain 1.98.1-aarch64-apple-darwin; logs `/tmp/mc-2026-10-05/106-{test,check,ts}.log`)

```
export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
cd ~/Develop/theuth-wt-106
cargo test -p tasks -p roster -p rpc -p tools --no-fail-fast   # EXIT 0
  roster  unittests: 57 passed (56 before)
  rpc     unittests: 36 passed; tests/prompts.rs: 1 passed
  tasks   unittests: 87 passed (86 before)
  tools   unittests: 153 passed
cargo check --workspace                                        # EXIT 0, Finished in 5m 21s; 1 warning (pre-existing future-incompat note for `block v0.1.6`)
just ts                                                        # EXIT 0, 8 passed
```

`bindings/Packet.ts:70–75` (gitignored):

```ts
/**
 * 106: who the packet is addressed to — `user` | `agent:<name>` (the
 * resolved manager for a decision, the effective reviewer for a review).
 * `None` on rows written before 106.
 */
to: string | null, };
```

rustfmt (edition 2024) per touched file; `crates/tasks/src/place.rs` reformatted as a side effect and reverted (as in 101/103).

## Deviations

- **No `decision → AskId` map in `Tasks`.** The brief asked for a `HashMap` filled when `ask_user` raises the card. The ask actor mints the `AskId` inside `Request` and only returns a `DelegatedReply` once the card is answered, so `ask_user` never sees the id before the answer; recording it would have needed a caller-supplied id on `tools::ask::Request`. Instead `drop_card` queries the existing `PendingAsks` and matches on the question id, which already is the decision key. Smaller, no `crates/tools` change, same observable result (the test asserts the card is gone from `PendingAsks`).
- **`AnswerDecision` handler does not swallow "no task is blocked on …".** The proof suggested making it a no-op; it stays an `Err` (the RPC `task_decide` / `decide` tool should still tell the caller the decision was already answered). The only `tell` sender is gone, and `on_panic` covers any future one.
- **Forwarder-wake dedupe key.** The brief said "dedupe key the decision id"; the wake store coalesces on `(agent, task, reason)` (`Wake::key_of`), so the wake reuses that key. Two user answers on the same task for the same forwarder coalesce into one `answer` wake with `count = 2`, which the desk reads from the thread anyway (D3). No new store path.
- **`by != actor` guard** on the forwarder wake (not in the brief): a manager that `decide`s its own forwarded decision is not woken about its own answer.
- **Decision-item title** names the packet's `by` instead of the task owner when a packet exists (not in the brief; one expression). Without a packet the owner is named as before. `derives_every_kind_and_hides_dismissed_or_snoozed` is unchanged because there `by == owner`.
- **No packet ⇒ qualifies.** A `decision:` blocker without a stored packet (a bare agent→agent `escalate` with no summary/board/pointers/items, or pre-101 rows) is treated like a legacy `to == None` row and listed, as today; the audience cannot be read without a packet.
- `crates/tasks/src/tools.rs` touched for one line (`to: None` in the validation-only `review_gate` literal); it would not compile otherwise.

## Not verified

- A live engine run of the three paths (the proof's scratch engine was not rebuilt); every path is covered by actor-level or ledger-level tests instead.
- The `AskResolved { dismissed: true }` broadcast reaching the desktop when `drop_card` dismisses a card (the ask actor's existing publish path; no UI run).
- The desktop `/attention/<id>` route rendering the forwarded packet's recommendation callout — depends only on the id now being in `attention_list` (`routes/attention.$id.tsx` looks the id up in the list, per the proof).
