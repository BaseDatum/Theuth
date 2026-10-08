# 125 as-built — R1 (closer, CONSULT)

Branch `feat/125-r1-consult`. Spec: `docs/design/125-advisor.md` Revision 1 (R1.0–R1.11), the closer row of R1.10 and the
closer block of R1.9. Starting HEAD `70e7b02c` = C `d243f863` + W1 (`50f8379e`, built in this tree) + merge W3 (`ffa424b2`) +
merge W2 (`81119e7f`). The four as-built notes `125-as-built-R1-{C,W1,W2,W3}.md` were read first; every hand-off in them is
closed below. Yesterday's `125-as-built-I.md` was the template for the scratch-engine smoke.

## Merge log

| Commit | What |
|---|---|
| `734e8380` | `docs/design/125-advisor.md` Revision history: an "As built" entry under Revision 1 — `ConsultSourceInfo.cap`, `Attach.warning`, `ConsultSource → Option<ConsultSourceInfo>`, `AdvisoryKind::or_review`, `CancelConsult → Err("cancelled")`, quarantine replay without the block, R1.11 #6 standing, the TUI/desktop surface facts |
| `a0bcba8a` | **fold** `origin/main` `5cddad8e` (40 commits: 136 dreaming, 144 research field, 140 autoresearch, studies). No conflicts; main touched no file of `agent`, `rpc`, `runtime`, `tui`, `slack`, `telegram`, `signal`; `crates/desktop/ui` changes were library/nav/popout files, none of W3's |
| `4d71c543` | `crates/roster/src/tools.rs` test: `frame::baseline` ninth (`consult`) argument — the only `--workspace --all-targets` red (the same test the 125 closer fixed yesterday for the eighth) |
| (last) | this note |

## What the merged tree contains, per wave

- **C** (`d243f863`): `AdvisorSeverity::Advice`, `AdvisoryKind { Review, Consultation { question } }` flattened into `Advisory.kind`
  (`or_review` for 0034 rows), `AdvisorConsultCounts`, `AdvisorStatus.consults/.warning`, `AdvisorDefaults.max_consults`,
  `SetAdvisorDefaults.max_consults`, `AppendAdvisory.model_visible`, `AdvisorConfig.max_consults` (6); `CallKind::Consult`;
  actor messages `Consult`, `ConsultSource`, `ConsultSourceInfo`, `CancelConsult`, `CONSULT_TIMEOUT`; `advisor/consult.rs`
  (`NAME`, `ConsultArgs`, `spec`, `parse`, `block`, `ONE_SHOT_SYSTEM` by `const fn` splice, the error strings);
  `Store::append_advisory(.., model_visible)` writing `excluded = {"whole":true}`; `Store::ancestors`; RPC
  `set_advisor_max_consults`; bindings; `UI/rpc/api.ts`; `DEFAULT_CONFIG` `advisor.max_consults`.
- **W1** (`50f8379e`): the three handlers (`Consult` → child path at depth 0 / one-shot `SideCall { kind: Consult }` at depth > 0,
  `ConsultSource` through `Store::ancestors`, `CancelConsult`), `send_update_with` (flush + `### Consultation` block on one
  `AdvisorInput`), `deliver_consultation` (masked `Consultation`/`Advice` card, `steer: false`), `finish_review` consult replies
  (clip 12 lines / 1 500 chars, `no objection`, quarantine → `Err("quarantined")` + replay without the block, no retry of a
  consultation, Pause/Halt refuse), parked consults, `OneShotDone`, counters (`consults_turn` reset at the advised
  `TurnStarted`, `consults_total` per link), the pairing warning (`supervisor/advisor.rs::pairing_warning` → `Attach.warning`),
  `render.rs` `pin_first` + `consult`, `system.md` `<consultation>`, `identity::one_shot`, `frame.rs` `CONSULT_PARAGRAPH` +
  `consult: bool`, `loop_.rs` `Turn::consult_advisor` + spec push when `cap > 0` + masked `AppendAdvisory` short-circuit,
  `SetAdvisorDefaults.max_consults` (0–32, `write_advisor_key`). Tests: `advisor::tests::consult_contract` (six scenarios),
  `loop_::advisor_tests::masked_card_lands_at_once_and_stays_out_of_context`, render/identity/frame tests.
- **W2** (`81119e7f`): TUI fifth status line `consults: N this turn · M total · cap C`, `/advisor consults <n>` →
  `set_advisor_max_consults` (no session needed), `◎ consulted: <question>` card collapsing by answer lines (SAND),
  `advisor: <warning>` once per link; 7 tests.
- **W3** (`ffa424b2`): desktop `Consulted` card (question as summary, answer body, no chip), `consultsLine` (`N/cap · M total`,
  `off` at cap 0) in the pill popover with the pairing warning under `Model`, Settings `Consults per turn` (Off/2/6/12 +
  off-list value), `Entry.question` from history and `AdvisoryAdded`; 6 tests.

## Hand-offs closed

| From | Hand-off | Closed by |
|---|---|---|
| C | `WorkerMsg::Agent(Box<AgentEvent>)` boxing and `kind: Default::default()` fixtures in slack/telegram/signal (verify only) | verified by read (`telegram/src/worker.rs:90`, `channel.rs:2316`, the eight fixtures); `cargo test --lib` of the three crates green below |
| C, W1 | record `ConsultSourceInfo.cap` / `Attach.warning` in the design | `734e8380` (Revision history "As built") |
| C, W1 | R1.11 #6 — `SideCall` has no `effort`; the one-shot runs at the provider's default effort while the root's consultation runs at the advisor's picked effort | **stands**; named here and in the design's "As built" entry. `ConsultSourceInfo.effort` is carried and unused |
| C | `AppendAdvisory.model_visible` through `append_advisory_idle`; the `125 R1 C placeholder` sites; `SetAdvisorDefaults.max_consults`; `<project-context>`/`<attention>` on the one-shot | all W1's (`125-as-built-R1-W1.md`); `grep '125 R1 C placeholder'` over `crates/` finds nothing |
| W2 | `/advisor status` fifth line once the engine fills `consults` | smoke (TUI, below): `consults: 1 this turn · 1 total · cap 6` |
| W3 | none | — |

## Gates (exact result lines)

All through `BUILD_LABEL=125R1-I /Users/Sky.Moore/.theuth/bin/build-lock nice -n 10 …`, `CARGO_BUILD_JOBS=4`,
`CARGO_INCREMENTAL=0`, `CARGO_TARGET_DIR=…/theuth-125-r1-int/target`, foreground (a Python subprocess per gate; logs in
`/tmp/125r1-gates/`). The tree is the fold `a0bcba8a` (+ `4d71c543` from the first gate on).

1. `BUILD_SLOTS=1 cargo check --workspace --all-targets` → first run `error[E0061]: this function takes 9 arguments but 8 arguments were supplied` ×2 at `crates/roster/src/tools.rs:418,419` (test-only; fixed in `4d71c543`); rerun → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1m 09s` (the pre-existing `block v0.1.6` future-incompat note only).
2. `cargo test -p agent` → `test result: ok. 536 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 167.82s` · doc `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`.
3. `cargo test -p rpc` → `test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 12.49s` · `tests/prompts.rs` `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s` · doc `0 passed`.
4. `cargo test -p tui` → `test result: ok. 152 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.13s` · `tests/no_emoji.rs` `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.48s` · doc `0 passed`.
5. `cargo test -p runtime -p slack -p telegram -p signal --lib` → runtime `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s` · signal `test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.47s` · slack `test result: ok. 144 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.57s` · telegram `test result: ok. 122 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.07s`.
6. `cargo clippy -p runtime -p agent -p rpc -p tui -p slack -p telegram -p signal --all-targets -- -D warnings` → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 3m 21s`.
7. `cargo fmt -p agent -p rpc -p tui -- --check` → exit 0, no output.
8. `BUILD_SLOTS=1 just ts-desktop` → `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 54 filtered out; finished in 5.22s`, then the `cp -R bindings/.` step; `git status` clean afterwards.
9. `cd crates/desktop/ui && bun run check` → `$ tsc -b --noEmit`, exit 0.
10. `cd crates/desktop/ui && BUILD_SLOTS=1 bun run test` → `Test Files  202 passed (202)` · `Tests  1400 passed (1400)`.
11. `cd crates/desktop/ui && BUILD_SLOTS=1 bun run build` → `$ tsc -b && vite build` … `✓ built in 13.24s`, exit 0.

Post-fold crate re-runs: the fold touched none of the crates this revision touched (see merge log), so gates 2–7 are the
re-runs. `cargo build -p theuth` (debug, `BUILD_SLOTS=1`) → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 12m 35s`.

## Scratch-engine smoke (R1.9 closer block, legs 1–6)

Engine: `env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH target/debug/t --data-dir /tmp/125r1-smoke --headless`, cwd = this
worktree (so `justfile` / `README.md` exist), fresh data dir holding only a hand-written `config.yaml` (not a copy of
`~/.theuth/config.yaml`): `agent.model: anthropic/claude-haiku-4-5`, `agent.communication: true` (for `spawn_subagent`),
`advisor.max_consults: 6`, `rpc.listen: 127.0.0.1:7926`, `mesh: {relay: none, discovery: false, bind: 127.0.0.1:0, hub: never}`,
`link`/`a2a`/`nodes`/`dreaming`/`sandbox`/`update`/`signal`/`slack`/`telegram`/`documents`/`codeindex` `enabled: false`.
`engine.json` → `{listen: "127.0.0.1:7926", version: "2.13.0", host: "headless"}`. The provider key came from the shell
(`ANTHROPIC_API_KEY`, name only checked), as yesterday. Driver: a throwaway Bun script, native `WebSocket` on
`ws://127.0.0.1:7926/?token=<data_dir>/token`, positional params in `api.rs` order, `subscribe_events(session, true)`,
`sqlite3 -readonly -json` for the store. Session model `anthropic/claude-haiku-4-5` (`set_model(s, …, false)`), advisor
`anthropic/claude-sonnet-4-5` (both in `list_models`, 216 entries). Four driver iterations on fresh dirs (driver bugs: note
field, body shape, over-strict asserts — see deviations); the recorded run is the last, 21/21 asserts green, ~3 min. Session
`01a11d13-19c3-…`, advisor child `01a11d13-1b01-…`, subagent `01a11d13-9853-…`.

| Leg | Evidence |
|---|---|
| 1 | `set_advisor(s, "anthropic/claude-sonnet-4-5", null)` → `{state: "idle", advisor: 01a11d13-1b01-…, model: "anthropic/claude-sonnet-4-5", consults: {turn: 0, total: 0, cap: 6}, warning: null}` (sonnet prices above haiku — no pairing warning, correct). `info().advisor` → `{model: "anthropic/claude-sonnet-4-5", effort: null, max_notes_per_update: 4, immune_turns: 3, sync_backlog: "off", max_consults: 6}`. |
| 2 | Prompt verbatim. Tool calls on the root in order: `consult`, `python`, `read_file`. `consult` args `{question: "Should I read the \`justfile\` or \`README.md\` first to understand the working directory structure and project setup?", context: "User wants me to read one of these files first. …"}`. |
| 3 events | one `ToolCallStarted`/`ToolCallFinished` pair `name = "consult"`, `is_error: false`, output `"Read \`README.md\` first.\n\nREADME typically explains what the project is, its purpose, architecture and setup context. The justfile shows how to run tasks but assumes you already understand the project structure.\n\nStart with the overview and intent (README), then consult the automation commands (justfile) once you know what you're working with."` (none of the error strings). One `AdvisoryAdded { session: s, seq: 2 }` with `advisory.kind = "consultation"`, `question` = the args' question, `notes = [{severity: "advice", note: <the output>}]`. `AdvisorChanged.status.consults` sequence `{1,0,6}` (the `Consult.n` arrived) → `{1,1,6}` ×5. |
| 3 status | `advisor_status(s).consults` → `{turn: 1, total: 1, cap: 6}` after the turn; after a second prompt with no tool call (`Reply with the single word OK …`, `ToolCallStarted` set empty) → `{turn: 0, total: 1, cap: 6}`. |
| 3 sqlite | `SELECT seq, json_extract(advisory,'$.kind'), json_extract(excluded,'$.whole') FROM messages WHERE session_id = s AND advisory IS NOT NULL` → `[{seq: 2, kind: "consultation", whole: 1}]`. Advisor transcript (`messages.body` is `{"role": "User"|"Assistant"|"Tool", "content": [{"Text": …} | {"ThoughtSignature": …} | {"ToolCall": …} | {"ToolResponse": …}]}`): seq 0 `User` = `### Session update` … `### Consultation` … `_agent's current step (not yet in the transcript):_\n(none)\n\n**question**: Should I read the \`justfile\` …\n**context**: User wants me …` (block + question present); seq 1 `Assistant` text equals the tool result (prefix compared after whitespace normalisation; the stored row is the full text, the tool result is the 12-line clip — identical here). `SELECT kind, owner_id, status, requested_model FROM model_calls WHERE owner_id = advisor ORDER BY created_at` → 4 rows, all `round` / `settled` / `anthropic/claude-sonnet-4-5` (polled until no `dispatched` row remained; the first is the consultation turn). `session_cost(s).descendants` → `calls 0 → 4`, `tokens.input 0 → 27118`, `estimated_for_unavailable [] → [$0.027]`. |
| 3 surfaces | `load_session(s).history` has one `role: "advisory"` entry, `advisory.kind = "consultation"`, `question` set, `context.removed = true` — the row the desktop `Consulted` card is drawn from (W3 `store/advisor.test.ts` pins the card; no browser surface in this smoke). TUI: `t --data-dir /tmp/125r1-smoke --resume <s>` in a pty, attached to the running engine, after one more consult on a re-attached link (leg 6 had disposed the first): the transcript shows the `◎ consulted: one-line answer: is the justfile or the README the better first read?` card with the answer line under it; `/advisor status` prints `advisor: idle · anthropic/claude-sonnet-4-5 · —` / `notes: 0 nit · 0 concern · 0 blocker · step 2 · backlog 0` / `context: 5.8k / 200k (2%) · in 16.6k · out 790 · cost ~$0.019` / `last: —` / **`consults: 1 this turn · 1 total · cap 6`** (fifth line; `1 this turn` because no user turn had started since that consult — the spec's `0 this turn` is the post-next-turn reading, which leg 3's RPC assert covers). Slack-bound variant: no channel enabled; `Advisory::channel_line` is `None` for an `Advice` card (`advisor_wire_tests::advice_ranks_lowest_and_is_never_posted`) and the three channel `--lib` suites are green above. |
| 4 | Prompt: spawn one native subagent with the verbatim instruction. Root tools `spawn_subagent`, `remind_me_in`; one `SubagentSpawned { child: 01a11d13-9853-… }`; the child's only tool call `consult {question: "Is 2 + 2 = 4?"}` → `"Yes, 2 + 2 = 4.\n\nThis is basic arithmetic; the agent should answer directly without consultation. …"`. `model_calls WHERE owner_id = child` → `[round haiku settled, consult anthropic/claude-sonnet-4-5 settled, round haiku settled]` — exactly one `kind = 'consult'` row, owned by the child, `requested_model` = the advisor's model id. `AdvisoryAdded` on the stream for this leg: exactly one, `session = child`, `kind = "consultation"`, `severity = "advice"`. Root `advisor_status.consults` → `{turn: 0, total: 2, cap: 6}` (`.turn` unchanged at 0). `list_sessions` before 1 → after 2; the new row is `{kind: "chat", parent: s}` (the subagent); no `advisor` row, the advisor id absent. |
| 5 | `set_advisor_max_consults(1)` → `AdvisorDefaults { …, max_consults: 1 }`. Prompt verbatim → two `consult` pairs: the first answered (`"The primary purpose is to document theuth: …"`), the second `"consult: cap reached (1/1 this turn)"`; `consults` → `{turn: 1, total: 3, cap: 1}`. `set_advisor_max_consults(0)` → `max_consults: 0`. Same prompt again on the same session: the tool is absent from the spec list; haiku nevertheless emitted two `consult` tool_use blocks (copied from its own transcript) which the loop refused with `consult is not available in this session (not a direct tool and not a runtime function you can see)` (stored as `ERROR: …`); nothing reached the actor (`total` stayed 3, `cap: 0`); the reply text: `"Consult is not available in this session. It appears it was only accessible in the earlier turn and is not a persistent tool."`. Fresh session `s0` with the advisor on at cap 0 (the literal R1.9 reading): `ToolCallStarted` set empty, reply text `"I don't have a tool to proactively consult an advisor. According to the system, an \`<advisory>\` is a note from a reviewer model … I can't call it myself. Could you clarify …"`. Cap restored to 6 afterwards. |
| 6 | Prompt asking for one consult; on the `ToolCallStarted { name: "consult" }` event the driver called `set_advisor(s, null, null)` → `{state: "off", advisor: null}`; the pair's output: `"consult: advisor off"`. `advisor_status(s)` → `state: "off"`, `consults: {0, 0, 0}`. The advisor's last `model_calls` row for the disposed link is `cancelled` (seen on the previous iteration's dir). |

Stream totals for the run: `TurnStarted/TurnFinished` 10, `ToolCallStarted/Finished` 11, `AdvisoryAdded` 3 (legs 2, 4, 5),
`AdvisorChanged` 41, `SubagentSpawned` 1, `Error` 1 (not captured by the driver; the engine log for the window has only
`skills: skill front matter invalid … skills/pdf.md` warnings). On an earlier iteration (engine cwd `/tmp/125r1-smoke`, no
`README.md`) the advisor's **push** path fired live for the first time: `AdvisoryAdded { kind: "review", notes: [{concern,
"Neither README.md nor justfile exists in this directory …"}] }` on the root — the first live `concern` card of design 125.

Engine stopped; `/tmp/125r1-smoke`, the driver and the evidence json removed; the keychain item `theuth` /
`secrets:/tmp/125r1-smoke` deleted (`security delete-generic-password`, then `find` → gone). Gate logs and a copy of the
evidence json remain in `/tmp/125r1-gates/` (outside the repo).

## Deviations (and why)

1. **Engine cwd = the worktree, scratch config written by hand** — rule 9 forbids reading `~/.theuth/config.yaml`; a 25-line
   scratch config is smaller and safer than a scrubbed copy. The first engine start (cwd `/tmp/125r1-smoke`) found no
   `README.md`; the legs still ran but the advisor's answer could not be followed, so the recorded run was launched from the
   worktree. `mesh` has no `enabled` key (`crates/mesh/src/config.rs`); `relay: none` + `discovery: false` + loopback bind is
   the "never dials the live engine" form. The default-on `signal` channel started provisioning on the first start — hence the
   explicit `enabled: false` for the three channels.
2. **Leg 5, cap 0 on the same session**: R1.9 writes "the next turn's `ToolCallStarted` set has no `consult`". The tool *is*
   absent from the spec list (the fresh-session variant shows an empty set), but a model whose transcript holds earlier
   `consult` calls may still emit the name; the loop refuses it and no consultation happens. Asserted as "no consult reaches the
   advisor + text explanation" on the same session and literally on a fresh session.
3. **Leg 3, "the following assistant row's text equals the tool result"**: the advisor child has read-only tools (R1.4) and on
   one iteration listed the directory before answering, so the answer is the first assistant row *with text* after the block.
   The equality is a whitespace-normalised prefix match (the tool result is the 12-line / 1 500-char clip of the stored text).
4. **Leg 3, `model_calls` rows settled**: queried right after the turn, the review of the step after the consult was still
   `dispatched`; the driver polls until no row is `dispatched` (≤ 60 s) and then asserts every row `settled`.
5. **Leg 4, `AdvisoryAdded`**: asserted on `session == child` only — on one iteration the root also received a live review
   `concern` in the same window (correct behaviour, outside the leg's assertion).
6. **TUI fifth line reads `1 this turn · 1 total`**, not `0 this turn · 1 total`: the TUI check ran right after the consult, before
   any new user turn (which is what resets `turn`); the RPC assert in leg 3 shows the `{0, 1, 6}` reading after a consult-free prompt.
7. **R1.11 #6 stands** (W1/C hand-off): `SideCall` has no `effort`; a subagent's one-shot runs at the provider's default effort.
8. **The build-lock `waiting` line prints only at `waited % 60 == 0`** with 5–10 s random steps — a long queue looks silent;
   cosmetic, not changed (not this design's file).

## Open items

- Desktop surface not rendered live (no browser in this smoke): the `Consulted` card is pinned by W3's `AdvisorNoteCard.test.ts`
  and `store/advisor.test.ts`; `load_session` carries the row with `question` and `context.removed`.
- The one `Error` event on the recorded run's stream was not captured (the driver kept only the asserted event kinds).
- R1.11 #7 (the child re-reads its own answer as the asker's `consult` tool line in the next review delta) is visible in the
  advisor transcript (seq 4 `→ consult(…) ⇒ ok · 3 lines` + the result) — tokens, not behaviour.
- R1.11 #12: this run gives the rate/cost shape — one consultation = one advisor `round` row (~$0.01 at sonnet prices here);
  measuring whether it helps is still by hand.
- `crates/rpc/bindings/*.ts` and `crates/desktop/ui/src/rpc/bindings/**` match the regenerated bindings (`git status` clean after
  `just ts-desktop`).
