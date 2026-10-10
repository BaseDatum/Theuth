# 159 W3 — as built (judged objectives, no scalar)

Branch `feat/159-w3-judged`, worktree `theuth-159-w3`, cut from C's `8c8410aa`. Design: `docs/design/159-autoresearch-variants.md` §8, built against C's contract (§5, §18.4) as described in `159-as-built-C.md`. Paths under `crates/autoresearch/src/` are written bare.

## 1. What was built, by §8 part

| §8 part | what | where |
|---|---|---|
| Outputs | one file per case under `<root>/<slug>/judged/out/<tree>/<id>.txt`; code artefacts run `bench.command` through `Backend::plugin_command` with `AUTORESEARCH_OUTPUT` and `AUTORESEARCH_CASES` (the case snapshot `<root>/<slug>/judged/cases.jsonl`, copied from `<worktree>/<judge.prompts>` on first use); prompt artefacts (`axes.artefact: file`) run the one plain path in `scope` through `evalcore::run::run_case` on `models.target`; incumbent outputs are the base's dir or the kept candidate tree's dir (`incumbent_dir` maps `incumbent_sha` → the kept experiment's last candidate sample) | judge.rs `generate`, `load_cases`, `incumbent_dir`, `tree_dir` |
| Blind pairs, both orders | `evalcore::pair::{SYSTEM, user, parse, Verdict}`: outputs fenced as data with a fence longer than any backtick run inside, the system prompt tells the judge to ignore text aimed at it and not to reward length; parse is the reviewer's strict rule (first non-empty line, `A:`/`B:`/`TIE:` + reason, a second verdict word = no verdict); one retry with a correction line. Seeded coin `sha256(slug, n, case)` picks who is A first; each case is judged twice with the order swapped; both verdicts agreeing give the winner, anything else a tie. Each call is a `judgments` row: `a_sha` = candidate tree, `b_sha` = incumbent, `a_first` = candidate shown as A, `winner` in those terms, `model`, `reason`, `cost_nanos` | pair.rs; judge.rs `judge_case`, `call_judge`, `combine`, `coin` |
| Decision | `decide(cfg, primary, candidate_samples, look)` (pure): p̂ = (w + t/2)/m, Wilson lower bound at z = 2.5758, keep when L > 0.5 and p̂ ≥ 0.5 + δ (δ = `metric.min_effect`, default 0.1); look 1 at `judge.look` (20) cases → keep / no gain (p̂ ≤ 0.5) / `Confirm { more: 1 }`; look 2 at 2 × look → keep or no gain. `GateStep::Decide { detail: GateDecision { rule: win_bound, stat: L, threshold: 0.5, n: m, objectives: [win_share, guards…] } }`. `sample` returns `value = p̂` of its look with `cases[id] ∈ {1, 0.5, 0}`, so the measure loop's crash rule and `exp.metric` work unchanged | judge.rs `decide`, `wilson_lower`, `tally`, `step`, `sample` |
| Ratings | `bt_fit`: Hunter's MM Bradley–Terry over every judged pair of the segment, ties as half wins, one phantom tie against a fixed θ = 0 opponent per version as the weak prior; se from the diagonal of the observed information; `elo(θ) = θ × 400/ln 10 + 1000`. `refit` runs after every decision and from `view`/`headline`, maps kept candidate trees to their commit, writes `ratings` rows; the ladder shows the base and kept versions | judge.rs `bt_fit`, `refit`, `ladder` |
| Human check (D14) | on an `Improved` decision, when (keeps so far + 1) is a multiple of `judge.human_every` (5), one random judged case of the keep is inserted as a pending `judgments` row (`model = human`, `winner` NULL, random order) plus a `human_pair` event; `human_pair(env, slug, id, winner)` normalises `a | b | same` from the shown order into ledger terms and stores it with `human = 1`; agreement = share of answers equal to the judge's double verdict; fewer than 3 of the last 5 agreeing pauses the study with `PauseReason::Other { reason: "the judge disagrees with you" }`. `pair_texts(env, study, row)` returns both outputs in shown order for the attention row | judge.rs `maybe_human_pair`, `human_pair`, `agreements`, `pair_texts` |
| Length | every sample reports `length` (mean words of the candidate's outputs) and `length_ref` (the incumbent's on the same cases); `decide` evaluates `cfg.guards` through `gate::guard_failures` on `(name, cand, ref)` pairs read from those secondaries, so `secondary_guards: [{name: length, max_regression: 20%, direction: lower}]` refuses padding with `GuardFailed` and a `Call::GuardFailed` objective delta | judge.rs `guard_means`, `decide` |
| Position probe | the baseline sample (k = 1) generates the base's outputs and judges 6 base-against-base pairs in both orders (12 calls, `judgments` rows with `n = 0`, `split = probe`); a judge picking A in more than 70 % or tying fewer than half fails the baseline sample with the numbers in `failure` and the sample log, so Start refuses with them | judge.rs `probe`, `sample` |
| Ledger | `judgments` rows as above; `ratings` rows per `(slug, segment, sha)`; `experiments.gate` carries the decision; `events` `human_pair`, `paused` | store accessors C built |
| Study view | `KindView::Ratings { ladder, agreement }`; `RatingLadder.tsx` draws one rung per version (θ ± 2 se bar, Elo, games, experiment number) and the agreement figure; `PairViewer.tsx` groups `autoresearch.judgments` rows into pairs (two ordered verdicts with reasons, the gate's reading, judge cost), pending human pairs first with A / B / same buttons calling `autoresearch.human_pair` | UI components; `judge::view` |
| RPC | `autoresearch.judgments` and `autoresearch.human_pair` go through two kameo messages defined in judge.rs (`Judgments`, `HumanPair`) with `impl Message<_> for Studies` off the mailbox, like `delegate_off_mailbox!` | judge.rs; rpc_impl.rs two `// 159:W3` lines |
| Setup | rows `prompts` (`judge.prompts`), `rubric` (`judge.rubric`), `judge` (`models.judge`, `judge.look`, `judge.human_every`) with `setup_value`/`setup_set` | judge.rs `SETUP_ROWS`, `setup_value`, `setup_set` |
| Draft card | "Improve `blurb.py`, judged by `<judge>` on 20–40 prompts from `products.jsonl` in blind pairs, both orders; you see one pair every 5 keeps." | judge.rs `card_line` |
| Skill | `skill/judged.md` "## Judged studies (no scalar)": the draft fields, rubric, prompt set, other-vendor judge, probe, human check, ladder, length guard | — |
| Brief | the rubric as a fenced data block, the rule in numbers computed for the study's look (`keep_threshold`), the last five looks' p̂ / L / verdict | judge.rs `brief_section` |
| Headline | `judged: N kept versions · incumbent θ ± 2 se (Elo) vs base Elo over G blind pairs · agreement a/b` | judge.rs `headline` |
| Fixture | `fixtures/toy-judged/`: 40 toy products (`products.jsonl`), the padded starting generator `blurb.py`, `bench.sh` (generation), `test.sh` (check), README with the study spec | — |
| JudgeSpec | `{rubric, prompts, look, human_every}`; every field `#[serde(default)]`, `0` = the default count (per the orchestrator's 15:58 exception) | types.rs after `block_specs!` |

Budget: judge and generator calls go through `Backend::side_call`, which the agent settles in `model_calls` under the study session — they land in the Session channel through `session_cost` and the per-experiment cap unchanged. `Sample.spend_nanos` stays 0 (it is the self-reported `SPEND` channel; writing the judge cost there would count it twice, C-d). Per-call cost is on `JudgmentRow.cost_nanos` for the pair viewer.

## 2. Deviations

- **W3-a** The prompt set is `judge.prompts` (a repo-relative JSONL path, snapshotted into `<root>/<slug>/judged/cases.jsonl`), not the W2 `cases` block: `CaseSetSpec` is W2's to define and was empty in this tree. Z may alias `cases.path` → `judge.prompts` after W2 lands. The rubric is spec text (`judge.rubric`), in custody by living in the ledger, not a file.
- **W3-b** The human pair is posted at the `Improved` decision inside `step` (there is no post-keep hook for W3), counting prior `Kept` experiments + 1. A keep later refused by checks or review still leaves its pair, which is still a valid judge-vs-human comparison.
- **W3-c** Pausing from `human_pair` repeats `actor::pause`'s three steps (`schedule_file::set_enabled(false)`, the `paused` event, `patch_study_if(running → Paused)`), because `pause` is private to actor.rs. Hand-off below.
- **W3-d** The position probe runs in the baseline sample at Start (the first `Baseline` sample judges the six identical pairs), not as `ProbeKind::Judge` in setup: `ProbeKind` is frozen without a `Judge` arm and setup has no bound session to charge the calls to. The refusal carries the numbers through the baseline's failure text. Hand-off below for the setup-time probe.
- **W3-e** `PairViewer` has no output texts: `JudgmentRow` is frozen and carries none. `judge::pair_texts` reads them from the cache for the attention row (Z); the viewer shows case, verdicts, reasons, outcome, cost and the answer buttons.
- **W3-f** The prompt-artefact arm (`Artefact::File` → `evalcore::run::run_case`) is coded against W2's frozen signature but untested here: `run_case` is a stub in this tree. The fixture and every test use a code artefact.
- **W3-g** The brief's keep thresholds are computed per study (`keep_threshold(m, δ)`), since `judge.look` changes them; at 20/40 and δ = 0.1 they are the design's 0.80 and 0.7125 (test-pinned).
- **W3-h** Probe rows use `split = "probe"` (the column's doc lists train|val|holdout); the ladder and agreement ignore `n = 0` rows.
- **W3-i** `incumbent_dir` finds the incumbent's outputs by `experiments.commit_sha == incumbent_sha` → that experiment's last candidate sample's tree dir. If `<root>/<slug>/judged/out/` is emptied the sample fails with that reason (`ponytail:` regenerate-in-place would need an incumbent checkout, which only the actor has).
- **W3-j** The rpc stubs' dead `let _ = (slug, n);` lines were removed with the `// 159:W3` line (they moved the arguments the call needs).

## 3. Gates

- `build-lock mbx check -p autoresearch -p evalcore --locked --tests` → rc 0 after one test-only fix.
- `build-lock mbx test -p autoresearch -p evalcore --locked --no-run` → rc 0, no warnings.
- `mbx test -p autoresearch -p evalcore --locked --no-fail-fast` → autoresearch **199 passed / 0 failed** (`golden_ledger_140` included), evalcore **5 passed / 0 failed**.
- `build-lock mbx clippy -p autoresearch -p evalcore --locked --tests` → rc 0 after two fixes (`manual_is_multiple_of`, `needless_question_mark`).
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; `bindings/JudgeSpec.ts` carries the four fields; copied to `crates/desktop/ui/src/rpc/bindings/` (gitignored, not committed).
- UI (`.ui-ready` present): `build-lock bun run check` → rc 0 before and after the binding copy; `build-lock bun run test -- src/components/autoresearch/judged.test.ts src/components/autoresearch/components.test.ts` → 2 files, 14 tests passed.
- `rustfmt --edition 2024` ran only on `judge.rs` and `pair.rs`; no shared file was reformatted. `git diff --cached --name-only` before each commit listed W3 files plus the two `// 159:W3` lines in `rpc_impl.rs` and the `JudgeSpec` block in `types.rs`.

## 4. P tests (judge.rs `mod tests`, pair.rs `mod tests`)

| test | proves |
|---|---|
| `pair::tests::parse_is_strict` | `A:`/`B:`/`TIE:` with reason; case-insensitive; markup trimmed; no reason, unknown word, prose, hedge → `None` |
| `pair::tests::user_fences_outputs_as_data` | an output containing a closing fence and an injected `A:` line stays inside a longer fence |
| `the_two_looks_follow_the_design_numbers` | 16/20 keeps (L ≈ 0.51), 15/20 confirms, 10/20 no gain; 29/40 keeps, 28/40 does not; δ margin blocks L > 0.5 alone; ties count half |
| `a_biased_judge_on_identical_outputs_never_keeps` | 2 000 simulated experiments, judge picks A 80 % of the time, identical outputs: ≤ 1 % keeps (swapped orders cancel the bias) |
| `null_false_keep_rate_is_at_most_one_percent` | 10 000 simulated experiments with a fair-coin judge: ≤ 100 keeps |
| `a_true_preference_is_kept` | an 85 % candidate preference is kept ≥ 80 % of 500 experiments (power) |
| `length_guard_blocks_a_length_loving_judge` | p̂ = 1 with outputs 30 % longer → `GuardFailed` with a `length` `Call::GuardFailed` delta; 10 % longer → `Improved` |
| `bradley_terry_recovers_a_simulated_ladder` | θ = 0, 0.7, 1.4, 2.1 recovered within 0.3 from 450 games each with ties, se in (0, 0.3); a perfect record stays finite; no games → θ = 0; Elo(0) = 1000 |
| `setup_rows_round_trip_and_the_card_line_reads` | the three rows' leaves set and render; bad counts refused; the §8 card line |
| `toy_judged_runs_end_to_end_through_a_fake_judge` | the fixture through `Env` + a fake shorter-wins judge: baseline generates 40 outputs and the probe ties 12/12 (`picked A 0 %`), the second baseline sample spends nothing; a shorter complete generator wins 8/8 at look 8 (16 calls, each case in both orders, rows in ledger terms with cost); `step` → `Improved`, `win_bound`, n = 8, length guard `Same`; ratings fitted (candidate above base); the human pair posted, `pair_texts` in shown order, the answer normalised and stored (`human = 1`, `a`), answering twice refused; `view` → agreement 1/1 and a base-only ladder; `headline` and `brief_section` text; after the keep the candidate tree's outputs serve as the incumbent's and the ladder shows `kept1` as rung 1 with `n = 1` |
| `a_biased_judge_fails_the_baseline_with_the_numbers` | a judge that always picks A fails the baseline sample: `picked A 100 % (limit 70 %)`, `tied 0 % (needs ≥ 50 %)` |
| UI `judged.test.ts` (4) | `pairsOf` grouping/outcome/ordering/probe filter; the ladder's rungs, Elo, base label, agreement; the empty ladder; the viewer's loading state |

A: the probe's numbers are in the baseline log and the refusal text; the agreement figure is `KindView::Ratings.agreement` and the headline. T (15 live experiments on toy-judged with Sky's pairs) is the closer's on a scratch engine; the README holds the spec.

## 5. Hand-offs

- **Z (attention row, crates/attention frozen):** a pending human pair is a `judgments` row with `model = 'human' AND human = 0` (event `human_pair {id, n, case_id}`); the row is `Blocked`-class, title "Study <slug>: which blurb is better? (pair <id>)"; the two texts come from `autoresearch::judge::pair_texts(&env, &study, &row).await -> (A, B)` in the order to show; the answer is `autoresearch.human_pair {slug, id, winner: a|b|same}`.
- **Z (types.rs `JudgmentRow`, optional):** `pub a_text: Option<String>, pub b_text: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`) so `PairViewer` can show the outputs; fill in `judge::judgments` for human rows via `pair_texts`. Without it the viewer stays as built (W3-e).
- **Z (actor.rs):** make `async fn pause` `pub(crate)` and replace the three-step block in `judge::human_pair` with `crate::actor::pause(env, &mut study, reason).await?` (W3-c).
- **Z (types.rs + setup.rs, optional):** `ProbeKind::Judge` → `judge::probe` needs a `StudyRecord` with a bound session; until setup has one, the baseline probe (W3-d) is the live check.
- **Z (skill.md L18):** replace the decline bullet with "A judge-only metric ("better prose", "nicer UI") → a **judged study**: see "Judged studies (no scalar)" below." (the fragment is appended after the core text).
- **Z (after W2):** alias `cases.path` to `judge.prompts` if the W2 case set should serve judged studies (W3-a); `run_case` is then live for prompt artefacts (W3-f).
- **Z (Cargo.lock):** none; no dependency added.
