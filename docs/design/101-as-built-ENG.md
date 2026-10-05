# 101 — as built (ENG: crates/tasks, crates/roster, crates/rpc)

Branch `feat/101-eng`, base `56126069`. Design: `docs/design/101-decision-review-surface.md`; contract §3 binding.

## What was built

- `crates/tasks/src/packet.rs` (new): `PacketKind` (:40), `PointerKind` (:61), `PacketOption` (:85), `Check` (:95),
  `Pointer` (:107, `#[serde(rename = "ref")] ref_` + `ts(rename = "ref")`), `Packet` (:128, `at_ms` as TS `number`),
  `Ask` (:155, the `escalate` request before key/actor/time are known), `review_key` (:181), `validate` (:186, every
  §3 cap and ref rule; errors name the field and the cap; colon-form code refs get a `path#L12` hint), `code_ref`
  (:288), `require_for_user` (:324, D1 texts `NEEDS_SUMMARY` / `NEEDS_POINTERS`), `thread_markdown` (:336, D3 block:
  question, summary, options with `(recommended)`, checks as `- [x]`, "Look at:" title + ref + note; evidence `text`
  stays out of the thread), `head` (:403, first non-blank line, `#` stripped, ≤ 140 chars with `…`).
- `crates/tasks/migrations/0007_packets.sql` (new): `packets(key PK, task, kind, json, at_ms, hlc)` + `packets_task`
  index; replicated via `replica::attach(["tasks","wakes","answered","packets"])` (`lib.rs:1153`, D5).
- `crates/tasks/src/store.rs`: `put_packet` (:575, `INSERT OR REPLACE`, `hlc(at_ms)`), `packet(key)` (:592),
  `packets_for(&[TaskId])` (:602, `json_each`, newest first). `delete_packets_of` skipped (brief: optional).
- `crates/tasks/src/lib.rs`: `Escalate` gains `summary`, `options: Vec<PacketOption>`, `recommended`, `pointers`
  (:252); new messages `GetPacket` (:273), `ListPackets` (:277), `EffectiveReviewer` (:283), `ReviewPacket` (:293)
  with handlers after `GetTask`; `ask_user` passes option descriptions and `recommended` into `::tools::Question`
  (D4) and maps a `Choice` answer back to the label.
- `crates/tasks/src/ledger.rs`: `escalate(id, Ask, actor, session)` (:1449) builds the `decision:<id>` packet,
  `require_for_user` when the manager is `Owner::User`, `validate`, **then** stores it (user audience, or an agent
  audience that gave a summary/pointers), writes `thread_markdown` + the existing "Manager: answer with `decide …`"
  tail, blocker, wake — nothing is written on refusal. `effective_reviewer(&Task, Option<&Owner>)` (:1514).
  `review_packet(id, summary, pointers, checks, actor)` (:1528): validate, store (replace), `**review requested**`
  block. `packet` / `packets_for` (:1557/:1562).
- `crates/tasks/src/tools.rs`: `OptionArg` (:1167, `#[serde(untagged)]`, schemars 1 generates it); `EscalateArgs`
  gains `summary`, `recommended`, `pointers`, `options: Vec<OptionArg>`; `TaskUpdateArgs` / `TaskMoveArgs` gain
  `summary`, `pointers`, `checks`; `review_gate` (:719) runs before any write for `disposition: review {reviewer}`
  and `task_move {state: review}`: asks `EffectiveReviewer`, applies D1 when it is the user, validates caps for every
  audience, and returns the `ReviewPacket` to send once the task is in review (`Ok(None)` for an agent audience with
  nothing given). The three tool descriptions say what a packet is, the fences, the inline evidence forms, the caps,
  and when the tool refuses.
- `crates/roster/src/attention.rs`: `AttentionItem.packet: Option<String>` (:81); `Inputs.packets` (:121); the
  decision item sets `packet` and `detail = head(question)` (falls back to the summary's head when the packet has
  no question), the review item sets `packet = review:<task>` and `detail = head(summary)`; without a packet
  today's strings stay. `gather` asks `ListPackets` over the open tasks it already loads.
- `crates/roster/src/desk.rs` `manual`: step 2 (iv) — escalating to the user needs a packet; step 4 — handing work
  to the user's review needs the review packet; both say the tool refuses without.
- `crates/rpc/src/api.rs`: `packet_get(key) -> Option<Packet>` (trait :1724, impl :5476); `export.rs` exports the
  six packet types. `just ts` regenerated `bindings/` (gitignored): `Packet.ts`, `PacketKind.ts`, `PacketOption.ts`,
  `Pointer.ts` (field `ref`), `PointerKind.ts`, `Check.ts` match §3 field-for-field; `AttentionItem.ts` has
  `packet: string | null`; `methods.json` lists `packet_get`.

## Tests added

- `packet::tests` (5): every cap with its boundary (16 KiB summary, 12 pointers, 32 KiB text, 6 options, 80-char
  label counted in chars, 128 KiB JSON, `recommended` range), ref rules (`ftp://` url, code without text, colon form →
  `src/x.rs#L12` / `#L12-L20` hint, backwards range), D1 for both kinds, thread block + `head`, wire shape (`ref`).
- `ledger::tests`: `escalate_to_user_needs_a_summary` (behaviour 1: Err "needs a `summary`", no thread line, no
  blocker, no wake, no packet), `escalate_with_a_packet_stores_and_mirrors_it` (2 + 7 at the ledger + 8: answer
  path unchanged), `escalate_to_an_agent_keeps_the_packet_optional` (3), `escalate_refuses_over_cap_packets_before_writing`
  (6), `review_packet_replaces_and_mirrors` (4: replace = one row, two blocks; `effective_reviewer` cases).
  `escalate_user_path` / `escalate_agent_manager_path` / `user_unblock_keeps_decision_and_task_blockers` updated to the
  `Ask` signature (the user path now passes a summary, as D1 requires).
- `tools::review_gate_tests`: `review_gate_enforces_the_user_audience` (4 + 5 at the tool boundary against a spawned
  `Tasks` actor: user reviewer refused naming `pointers` with nothing written; summary alone refused; agent reviewer
  → no packet; explicit `reviewer: user` refused; caps for every audience; gate → `MoveTask review` → `ReviewPacket`
  → `GetPacket` by key, replaced on the second hand-over, `ListPackets` one row, unknown key → `None`),
  `escalate_options_take_both_shapes` (D2 + schema generates).
- `attention::tests::decision_and_review_items_carry_their_packet` (2 + 4 at `derive`: with/without packet, `detail`
  head of question / summary cut to 140 chars, a stray packet makes no item).

## Gates (run in this worktree, toolchain 1.98.1-aarch64-apple-darwin)

```
cargo test -p tasks -p roster -p rpc --no-fail-fast      # EXIT=0
  roster  unittests: 51 passed, 0 failed
  rpc     unittests: 36 passed, 0 failed; tests/prompts.rs: 1 passed
  tasks   unittests: 76 passed, 0 failed
cargo check --workspace                                   # EXIT=0, Finished in 8m 32s (one dependency future-incompat note: block v0.1.6)
just ts                                                   # EXIT=0 (8 export tests passed); bindings/ is gitignored, nothing to commit
```

rustfmt (edition 2024) on the touched files only; `crates/tasks/src/place.rs` was reformatted as a side effect and
reverted (not touched by this wave).

## Deviations

- **Effective reviewer** (`ledger.effective_reviewer`): `reviewer` named by the call, else `t.reviewer`, else the
  owner's manager; a desk naming itself without a `done_when` resolves to its manager, exactly as `update` does. A
  task with a `done_when` and no reviewer resolves to the owner's manager (brief rule), although the `Review → Done`
  guard treats a missing reviewer as the user; pre-existing inconsistency, left alone.
- **Agent audiences**: an escalation to an agent stores a packet only when `summary` or `pointers` were given;
  a review hand-over to an agent stores one when any of `summary` / `pointers` / `checks` was given. The user
  audience always stores one (D1 guarantees the content).
- **Decision `detail`** falls back to `head(summary)` when a packet has no question (cannot happen through
  `escalate`, which always sets it; defensive only).
- **Tool wiring** uses three actor messages (`EffectiveReviewer` → write → `ReviewPacket`) instead of one, so the D1
  refusal and the cap check happen before any write while a refused state move (guard) never leaves a packet behind.
  If storing the packet fails after the move, the tool reports "task is in review but its packet was not stored".
- `thread_markdown` lists the decision options as `- label (recommended) — description` (previously `- label`);
  the `**decision requested**` header and the `Manager: answer with …` tail are unchanged.

## Not verified

- The `packet_get` RPC method itself (one line over `GetPacket`); the actor message is tested, the jsonrpsee wrapper
  is not (no rpc actor tests exist in the crate). `methods.json` contains `packet_get` after `just ts`.
- The `escalate` / `task_update` / `task_move` tool `call` bodies end-to-end through the tool registry (they need a
  `ToolCx` with session resources); the shared `review_gate` and the ledger paths they call are tested.
- Replication of the `packets` table across engines (the attach list is extended; the replica crate's tests cover
  the mechanism).
- The channel ask card rendering of option descriptions / `recommended` (passed through to `::tools::Question`).
