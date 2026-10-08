# 125 as-built — R1 W1 (engine wave, CONSULT)

Branch `feat/125-r1-w1`, off C's contract commit `d243f863`. Spec: `docs/design/125-advisor.md` Revision 1,
R1.3–R1.6 (W1's sections) against R1.7 as C built it (`docs/design/125-as-built-R1-C.md`). Behaviour only:
the three stubbed handlers, the loop tool, the prompts, the frame paragraph, the masked card path, the
counters and the pairing warning. No `lib.rs` type, no RPC, TUI or desktop file touched.

## Built (file-level)

- `crates/agent/src/advisor.rs` — `Attach.warning` (D-R1.5, computed by the supervisor); `ConsultSourceInfo.cap`
  (the live `max_consults`, see deviation 1); `REASON_OFF` / `NO_OBJECTION`; `clip_answer` (12 lines + 1 500 chars,
  empty → `no objection`); `without_block`; `Review.consult`; `PendingConsult { reply, question, block, parked }`;
  `Link.consult / consults_total / consults_turn / warning`; `Link::status(cost, cap)` fills `consults` + `warning`;
  `Advisors.oneshots` (subagents with a one-shot in flight). `send_update_with(session, consult)`: the R1.4 step-3
  flush — trailing pending `Tool` rows (the asker's own batch) are left for the next delta, the block rides the same
  `AdvisorInput`, `(0 steps)` when nothing is pending, the trailer only when the last step said more follows.
  `deliver_consultation` (masked `Advisory { kind: Consultation, notes: [Advice] }`, `steer: false`,
  `model_visible: false`, `consults_total += 1`, publishes). `finish_review`: `Good` with `consult` replies the
  clipped text and writes the card; quarantine replies `Err("quarantined")` and replays the update **without** the
  block; a retriable error never retries a consultation (D-R1.9) and replies the first line; `Pause`/`Halt` refuse
  the pending consult with their detail; `cancelled` replies `Err("cancelled")`; a `Reset::Replay` keeps the
  consult flag on the re-run review. `maybe_next` sends a parked consult (its flush covers the backlog) before a
  plain backlog. Handlers: `Consult` (`DelegatedReply`; depth 0 → `consult_child`: gate → park or flush; depth > 0 →
  `consult_one_shot`: `ConsultSource` walk, `render_update` over the whole counted transcript with `pin_first` +
  the block, redacted, `SideCall { role: Named(link model), system: identity::one_shot, budget: CONSULT_TIMEOUT,
  kind: Consult }` to `Agents` from a detached task → internal `OneShotDone`); `OneShotDone` (clip, reply, card on
  the subagent, `total` on the root link; a 429/quota error pauses the link, any other error only warns);
  `ConsultSource` (own link first, then `Store::ancestors`); `CancelConsult` (drops the one-shot slot, replies
  `Err("cancelled")`, cancels the child's consultation turn when it is the one in flight). `AgentEvent` arms:
  the advised session's `TurnStarted` resets `consults_turn`; its `cancelled` `TurnFinished` refuses the pending
  consult and cancels the child as before; `Detach` refuses with `advisor off`.
- `crates/agent/src/advisor/render.rs` — `UpdateOpts.pin_first` (the first row survives row elision; the
  `_(N earlier rows elided)_` line moves after it) and `UpdateOpts.consult: Option<&str>` (the block after `---`,
  before the `[in progress …]` trailer, counted in the byte cap); the heading reads `(N steps)` for every `N != 1`.
- `crates/agent/src/advisor/system.md` — the `<consultation>` section after `<completeness>` (R1.6 text verbatim);
  C's `ONE_SHOT_SYSTEM` picks it up by construction.
- `crates/agent/src/advisor/identity.rs` — `identity()` unchanged in signature; its tail is `with_context()`, shared
  with the new `one_shot(data_dir)` (= `ONE_SHOT_SYSTEM` + `<project-context>` + `<attention>`).
- `crates/agent/src/advisor/{guard,advise}.rs` — C's `Advice` arms are the R1 behaviour (`rank = 0`, `parse` refuses
  `advice`, `severity_str = "advice"`); unchanged beyond the comment.
- `crates/agent/src/frame.rs` — `CONSULT_PARAGRAPH` (R1.6 text); `baseline` / `baseline_dated` gain `consult: bool`
  after `advised`; the slot right after `ADVISORY_PARAGRAPH`; tests updated + `consult_paragraph_follows_the_advisory_one_when_the_tool_is_listed`.
- `crates/agent/src/loop_.rs` — `Shared.consults: Arc<AtomicU32>`; `Turn.consult: Option<ConsultSourceInfo>` and
  `Turn.consults: u32`; `rebase` asks `ConsultSource` once per rebase for a Chat session when `depth > 0` or the
  advisor flag is set, keeps it only when `cap > 0`, passes `consult.is_some()` to `baseline_dated` and pushes
  `consult::spec()` (sorted); `Turn::consult_advisor` (parse → cap → `ask` under `CONSULT_TIMEOUT`; errors are `ok`
  results; timeout tells `CancelConsult`); the batch's `local` closure and dispatch arm (the arm is under
  `tokio::select!` with the turn's cancel token: the asker's Stop tells `CancelConsult`); `append_advisory_idle(..,
  model_visible)` and `AppendAdvisory`'s masked short-circuit (busy or idle, never queued as context).
- `crates/agent/src/supervisor/advisor.rs` — `SetAdvisorDefaults`: `max_consults` range 0–32, `write_advisor_key`,
  `advisor_cfg.max_consults`; `pairing_warning(advisor_cost, advised_cost)` + `PAIRING_WARNING`; `advisor_on`
  computes it from `resolved.model.cost` and the advised row's model and passes it in `Attach.warning`.
- `crates/agent/src/supervisor.rs` — `pub(crate) mod advisor` (tests reach `pairing_warning`); `ShowPrompt` passes
  `advised && advisor_cfg.max_consults > 0` as the new `consult` argument.
- Tests: `advisor/tests.rs` `consult_contract` (six scenarios, one rig: root end to end, parked consult + held nits,
  gates, quarantine + blocker, timeout + Stop, subagent one-shot); `loop_/advisor_tests.rs`
  `masked_card_lands_at_once_and_stays_out_of_context` + the advisor-kind spec check; `render.rs`
  `consult_block_and_pinned_first_row`; `identity.rs` one-shot assertions; `frame.rs` paragraph test.

## Gates (exact result lines)

All through `BUILD_LABEL=125R1-W1 /Users/Sky.Moore/.theuth/bin/build-lock nice -n 10 …`, `CARGO_BUILD_JOBS=4`,
`CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=…/theuth-125-r1-int/target`, foreground.

- `cargo test -p agent --lib -- advisor:: loop_::advisor_tests` →
  `test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 510 filtered out; finished in 22.76s`
  (`advisor::tests::consult_contract`, `advisor::tests::advisors_contract`, `advisor::tests::restart_reseeds_and_prefaces`,
  the 5 `loop_::advisor_tests`, the `advisor::{advise,consult,guard,identity,render}::tests`, the 7 `store::advisor::tests`).
- `cargo test -p agent` → `test result: ok. 536 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 110.88s`
  (lib) + `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (doc).
- `cargo clippy -p agent --all-targets -- -D warnings` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 03s`.
- `cargo fmt -p agent -- --check` → clean after `cargo fmt -p agent` (every hunk in W1's files).
- `cargo check -p rpc -p tui -p theuth` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 30s`.
- `cargo test -p tui --test no_emoji` → `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.48s`
  (R1.9 W1 (14): the new agent strings carry no emoji).

R1.9 W1 coverage: (1) `root_consult_end_to_end` (tool + paragraph present with the flag, absent at `max_consults = 0`),
`subagent_one_shot` (present through two hops, absent without a link), `advisor_kind_session_runs_advise_locally…`
(never for `kind == Advisor`); (2) `root_consult_end_to_end` + `parked_consult_and_held_nits` (`(2 steps)` heading);
(3) `parked_consult_and_held_nits`; (4) (5) (6) `root_consult_end_to_end` (12-line clip, `no objection`, masked
`Consultation`/`Advice` card, one `AdvisoryAdded`, the `→ consult(…)` tool line in the next review and no `<advisory`,
cap string without an actor call, reset on the next user turn) and `subagent_one_shot` (never resets); (7)
`consult_gates`; (8) `consult_timeout_and_stop`; (9) (10) `consult_quarantine_and_blocker`; (11) `subagent_one_shot`
(`ConsultSource` two hops / `None`, one one-shot request on the no-tools system prompt with the preface and the block,
one `model_calls` row `kind = consult` owned by the subagent, 400 → unavailable + link stays `Idle`, 429 → `Paused`,
`total` on the root link, no card for a failure); the 48 KiB pin is `render::tests::consult_block_and_pinned_first_row`;
(12) `root_consult_end_to_end` (`consults`, `warning: None` for unpriced fakes) + the `pairing_warning` table in
`consult_gates`; (13) `frame::tests::consult_paragraph_follows_the_advisory_one_when_the_tool_is_listed`; (14) above.
The masked `AppendAdvisory` path on a busy session is `loop_::advisor_tests::masked_card_lands_at_once_and_stays_out_of_context`.

## Deviations from the design (and why)

1. **`ConsultSourceInfo.cap: u8` added** (R1.7 declares four fields). The loop decides the spec list from one
   `ConsultSource` ask per rebase; the live `max_consults` lives in the actor's `cfg`, so the reply carries it
   instead of a second ask. Only W1 code constructs or reads the struct.
2. **`Attach.warning: Option<String>`** (pre-R1 struct, W1's file). The pairing warning needs the catalog and the
   advised session's model, both supervisor-side; the actor only stores and reports it.
3. **`REASON_OFF` / `NO_OBJECTION` / `OneShotDone`** are new names in `advisor.rs`; the first lets the loop map the
   actor's `Err("advisor off")` to `consult::ERR_OFF` without string-matching a second copy.
4. **`UpdateOpts.consult`** beside `pin_first`: R1.8 puts "the `### Consultation` block appended after `---`" in
   `render.rs`; carrying it in the opts keeps the byte cap honest (the block counts against `UPDATE_BYTES`).
5. **Pending tool rows are not flushed by a consult.** The asker's own `consult` call is a `Tool { result: None }` row
   at the tail when the flush runs; sending it would fingerprint differently once answered and trigger a replay on
   the next update. They stay past the cursor and arrive with the next delta as the ordinary tool line (R1.4 step 7).
6. **No trailer is forced on a consultation update**: `will_continue` is the last step's, as for any update (R1.9 (2)
   wants the text to end with the block). The one-shot (R1.5) does carry `will_continue: true` as specified.
7. **`Review.consult` survives a `Reset::Replay`** (re-prime or rewrite during a consultation): the replayed turn
   answers the consult, as R1.4 "Interactions" asks; the 90 s clock is the loop's.
8. **The quarantine replay strips the block** (`without_block`): replaying the consultation would re-ask a question
   whose asker already received `Err("quarantined")`.
9. **`CancelConsult` replies `Err("cancelled")`** instead of dropping the sender (the loop already returned the
   timeout string); deterministic for the actor tests.
10. **The advised `TurnStarted` resets `consults_turn`** in the actor so `AdvisorStatus.consults.turn` reads 0 at a
    new turn even before the first `Consult.n` of that turn arrives.

## Hand-offs for the closer

- R1.11 #6 stands: `SideCall` has no `effort`; the one-shot runs at the provider's default effort
  (`ConsultSourceInfo.effort` is carried and unused).
- `docs/design/125-advisor.md` R1.7 could record deviations 1–2 (two added fields) when the closer reconciles.

## Open items

- The scenario rig gained reply **lanes** (`Lane::{Any, Asker, Advisor, OneShot}`, matched on the request's system prompt and
  last message) and a title-call bypass: with a subagent in play the fake serves four callers (the asker, its advisor, the
  one-shot, the parent turn a child's result opens) plus detached title calls, and a FIFO script was non-deterministic.
  `advisors_contract` still uses `push` (= `Any`) and is unchanged.
- The one-shot's provider-error leg uses a 400 (deterministic); a 500 was absorbed below `Side::call` in the rig (the
  request reached a hand-off turn instead), so "any other error does not pause" is pinned with a 4xx.
- Not exercised here: a real provider's 429 text through the one-shot (the rig's 429 body carries `rate limit`, which
  `QUOTA` matches as the child path does); the closer's scratch-engine smoke (R1.9 closer legs 1–6) covers the live shape.
- `AdvisorStatus.consults.turn` is the root's own count as the asker reports it; a subagent's `n` is never folded in
  (R1.5 "Counting") — `/advisor status` shows `turn` for the root and `total` for the tree.
