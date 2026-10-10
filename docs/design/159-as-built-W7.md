# 159 as-built — W7 META (the loop improves the loop)

Branch `feat/159-w7-meta`, cut from C's skeleton 1a70e5fe. Implements §12 and the W7 part of §14 on the frozen contract of §5/§18. Six waves ran in parallel; W2's `evalcore::gate::{paired, decide}` and W6's `world::{wait, resume}` are neutral stubs in this tree (see W7-b and the test seam).

## What was built

| file | what |
|---|---|
| `crates/autoresearch/src/mechanism.rs` | The mechanism as data (D13). `PROTOCOL`, `FIRE`, `ADVISOR` (the compiled texts, byte for byte), `Params` (`mechanism.yaml`: `version`, `brief.{max_bytes,recent,dead_ends,ideas,playbook_bytes}`, `plateau.window`, `novelty.threshold`, `defaults.crash_retries`, `categories`), every struct `deny_unknown_fields`. `defaults()`, `export()`, `parse()` (ranges: plateau 4–48, novelty 0.3–0.95, brief caps 1..=compiled, crash_retries ≤ 5, ≤ 12 categories, texts ≤ 8 KiB and non-empty). `ensure(root)` creates `<root>/mechanism/` (git, one commit, registered as v1 "compiled defaults" in `<root>/mechanism.json`). `files_at(root, sha)` (`git show`), `load(root, sha)`, `active(root)`, the three readers `protocol` / `fire_prompt` / `advisor_prompt` (`None` = compiled; `{crash_retries}` filled from `mechanism.yaml`; a missing or invalid mechanism warns and falls back), `overlay(root, sha, cfg)` (the parameters laid over an `AutoresearchConfig`), `snapshot(worktree, slug, n)` (the outer candidate as a `git stash create` commit under `refs/autoresearch/<slug>/mechanism/<n>`, validated before any inner study starts), `register(root, sha, summary)` (next version, idempotent per sha), `rpc(root, op)` (`list` / `show {version?}` / `adopt {version}`; `adopt {version: 0}` = Revert to the compiled defaults; adopt refuses a version whose files no longer parse). |
| `crates/autoresearch/src/suite.rs` | The `suite` scoring kind. `FIXTURES` (toy-sort, toy-dedupe, toy-wordfreq, toy-json, toy-lru embedded with `include_str!`, each with its `reference/`), `materialise()`. `Suite`/`suite_of(spec)` (reads `spec.suite`, W7-a), `preset(repo)`, `card_line`, `SETUP_ROWS` (`suite`: `suite.training`, `suite.held_out`, `suite.seeds`, `suite.inner_experiments`, `suite.executor`), `setup_value`, `setup_set`. `sample()`: one outer sample = one suite run — snapshot or tree sha → `best_known` per fixture (the reference solution's holdout through `Backend::plugin_command`, cached in `suite/best_known.json`) → one inner study per fixture × seed (`created_by = meta:<outer>/<fixture>/<seed>`, `mechanism` pinned, origin none, two in flight; a fixture absent from this build is skipped and named in the event) → wait for them → `score_inner()` from their `role = holdout` rows only → `Sample { value: mean s, cases: {fixture/seed: s}, case_values: {gain, base, final, best, keeps}, asi.mechanism, spend_nanos: Σ inner study spend }` plus a `suite` event. `score(base, final, best)` = (base − final)/(base − best), relative gain without a reference. `step()`: W2's paired gate over the fixture × seed pairs (`min_mean_delta` = `metric.min_effect`, `max_case_regression` 0.25, `min_pairs` 2) → `Decide { detail: GateDecision { rule: Paired, stat: t, threshold: c_n, n: pairs } }`; `Confirm { more: 0, incumbent_sample: true }` once when the incumbent's suite is two experiments stale (the "every third experiment" re-run). `view()` (`KindView::Suite`: training rows incumbent vs newest candidate; held-out rows base holdout vs latest holdout), `brief_section()` (training scores and what the four files are; never a held-out number), `headline()` (`suite +0.12 over mechanism v1 (held out +0.07) · mechanism v2 ready to adopt`; registers the final mechanism as the next version). `PairedGate` seam (`set_gate` in tests). |
| `crates/autoresearch/fixtures/{toy-dedupe,toy-wordfreq,toy-json,toy-lru}/` | §14 fixtures in toy-sort's shape: naive scope file, `main.py`, `bench.sh` (`CASE`, `METRIC time_ms`, `METRIC peak_kb`, `ASI`), `test.sh` (bait in `fixtures/`), `holdout.sh` (other seeds, self-checked against the stdlib), `reference/<scope>.py`, README with the spec. Measured on this Mac: dedupe 487 → 1.4 ms, wordfreq 236 → 5.4 ms, json 143 → 1.2 ms, lru 1024 → 4.7 ms (bench, naive → reference); every sample under 1.3 s. |
| `crates/autoresearch/skill/meta.md` | "## Meta studies — the loop improves the loop": the preset, the three questions, the draft, what the mechanism is and is not, the adopt press. |
| `crates/desktop/ui/src/components/autoresearch/SuiteChart.tsx` (+ `SuiteChart.test.ts`) | Grouped bars per fixture (incumbent, newest candidate), training left, held-out right of a divider, base line and reference line, regressions hang below the base. |
| `crates/autoresearch/src/rpc_impl.rs` (the `// 159:W7` line) | `autoresearch.mechanism` → `mechanism::rpc(&ask(DraftRoot).await?, op)` (fmt wraps the one statement over three lines). |
| `crates/autoresearch/src/types.rs` (R1: my block spec only) | `SuiteSpec { training, held_out, seeds, inner_experiments, executor }` as a `wire!` after the `block_specs!` invocation; its line removed from the invocation. |

Registration lines filled: `rpc_impl.rs` (above). The other `// 159:W7` lines (actor.rs `kind_sample`/`kind_rule`/`kind_view`/`brief_context`, brief.rs, finalize.rs, setup.rs, study_draft.rs, schedule_file.rs, review.rs, lib.rs, StudyView.tsx) already called my module's frozen signatures and needed no edit.

## Map to §12

| §12 part | where |
|---|---|
| Prerequisite: the mechanism as data | `mechanism.rs`; the repo `<root>/mechanism/`, seeded by `ensure`; the schema is `Params` (+ three texts); forbidden keys are unknown keys |
| `mechanism::load(root, sha)` with ranges and 8 KiB | `load`/`parse` |
| Pinning at Start, custody | inner studies: `start_inner` sets `study.mechanism`; ordinary studies: hand-off H2 (actor `create` is frozen). The pin is honoured by `brief_context` (frozen W7 line → `protocol(&env.root, study.mechanism)`) |
| Outer artefact `mechanism`, scope `mechanism/**` | `preset()`: repo `<root>/mechanism`, scope = the four files; `kind()` = meta (C-b) |
| Scoring `suite`, inner studies, `Waiting` | `suite::sample` (W7-b: synchronous) |
| Score = normalised holdout gain | `score`, `score_inner` (holdout rows only; `keeps` reported, never scored) |
| Decision: W2's paired gate, incumbent re-run every third | `suite::step` |
| Holdout: held-out suite every second keep and at finalize | the outer spec's `holdout: {command: <held-out names>, every: 2}`; the actor's holdout path calls `suite::sample` with `role = holdout` |
| Landing: "Adopt mechanism v<n>", a press | `headline` registers; `autoresearch.mechanism {op: adopt}`; Settings pane is Z's (C-e) |
| Ledger | `studies.mechanism` (inner), `created_by = meta:…`, `samples.cases`/`case_values` per inner study, `experiments.gate` rule `paired`, `suite` events |
| Study view, setup chat, draft card, skill | `SuiteChart`, `SETUP_ROWS`/`setup_*`, `card_line`, `skill/meta.md` |
| Guard | `deny_unknown_fields` + ranges; snapshot validation before inner studies; fixtures embedded (W7-c); held-out never in `brief_section`; adoption only by press; inner studies run the compiled gate (`axes` default) |
| Cost | `card_line`: 4 × 2 inner studies × $1.20 × 4/3 ≈ $13; `INNER_USD` 2, `INNER_EXPERIMENTS` 8, `parallel: 1` |

## Deviations

- **W7-a `SuiteSpec` defined under the orchestrator's R1 exception** (types.rs, my block only): `{training, held_out, seeds, inner_experiments, executor}`, every field `#[serde(default)]`, lists and `executor` skipped when empty, so a 140 spec is byte-identical (golden ledger green, round trip asserted in `setup_rows_round_trip_and_refuse_unknown_fixtures`). Empty lists = the §14 suite, 0 = the §12 constants (2 seeds, 8 experiments). `bench.repeats` is still the incumbent's suite runs at Start, `holdout.every`/`repeats` the held-out cadence, `bench.timeout` one suite run's limit (it sizes `run_timeout`); the two `command`s are not run for this kind (`validate_spec` does not require them for `scoring: suite`; the preset writes `suite`). The earlier command-string encoding is gone. Bindings regenerated locally (`SuiteSpec.ts`), not committed.
- **W7-b Inner studies run inside the outer sample, not through `Waiting`.** `world::wait`/`resume` are W6 stubs here, `resume` has no per-kind dispatch into suite, and `finish_measure`'s CAS (`Measuring` → `Measured`) means a sample cannot move its experiment to `Waiting` mid-measure (C-g). So `suite::sample` starts the inner studies and polls until they are terminal; the outer `experiment_run` call blocks meanwhile, like a long bench, and `run_timeout` grows with `bench.timeout` (4 h in the preset → 24 h). The measure call runs off the mailbox (`delegate_off_mailbox!`), so the inner studies' own `begin/run/log` flow freely. Hand-off H3 is the one-line switch to W6's path when `world::resume` dispatches by kind.
- **W7-c The fixtures are embedded, not evaluator paths of the outer repo.** `FIXTURES` carries every file (`include_str!`); each inner study gets a fresh copy from the binary, so the outer session cannot reach a fixture at all (stronger than custody). Inside each inner study the fixture files are `evaluator_paths` and `off_limits` as in 140. toy-pareto (W1) and toy-route (W4) are not in this tree → H5.
- **W7-d The mechanism's numeric parameters are validated and stored but inert in this tree.** Their readers (brief caps in `brief::render`'s `cfg`, `stop::decide`'s `plateau.window`, `novelty.threshold` in `measure_env`, `crash_retries` in the run path) are frozen files with no `// 159:W7` line. `mechanism::overlay` is the one call to wire (H6). `defaults.crash_retries` does fill the protocol text today.
- **W7-e `fire.md` and `advisor.md` follow the active version, `protocol.md` the study's pin.** `schedule_file::text` and `review::advise` pass `sha = None` (C-j, C-k: no `StudyRecord` there); only `brief_context` passes `study.mechanism`. An inner study pinned to a candidate therefore reads the candidate's protocol and the active fire/advisor texts.
- **W7-f Versions and adoption live in `<root>/mechanism.json`**, outside the repo (so the repo stays clean for the outer worktree): `{active, versions: [{version, sha, at, summary}]}`. v1 = the seeded defaults; `active: None` = compiled (readers return `None`, the 140 texts are untouched — proved byte for byte through `schedule_file::text` and `brief::render`).
- **W7-g `/autoresearch meta` is skill text + `suite::preset`, not a plugin preset mechanism** (none exists). The repo must exist before the draft (`create` checks `is_git_repo`); `ensure` runs from `mechanism::rpc` (Settings › Autoresearch, Z) and from `register`. H2 adds the `ensure` call to `create` for `artefact: mechanism`.
- **W7-h Missing inner scores are missing cases, not zeros.** A failed inner study (prepare failed, over its hours) is logged in the `suite` event with `state: failed` and dropped from `cases`; a sample with no scored case is a crash (`value: None`). A cancelled outer run returns `Err` and leaves running inner studies to their own budgets.
- **W7-i `KindView::Suite` carries no mechanism diff** (`SuiteRow` is frozen); each keep's diff is in the experiment detail as for any study.

## Gates (this worktree, BUILD_LABEL=159-W7)

- `build-lock mbx test -p autoresearch --locked --no-run` — ok.
- `mbx test -p autoresearch --locked` — **202 passed, 0 failed** (188 before + 14 W7: 7 `mechanism::tests`, 7 `suite::tests`, among them the golden ledger unchanged).
- `build-lock mbx clippy -p autoresearch --locked` (and `--tests`) — clean.
- `cargo fmt -p autoresearch` — touched only `mechanism.rs`, `suite.rs`.
- UI: `build-lock bun run check` (tsc) — clean; `build-lock bun run test -- src/components/autoresearch` — 5 files, 37 tests passed (2 new in `SuiteChart.test.ts`).

## Fixture evidence (P and the automated T)

- P: `export_then_parse_is_the_identity`; `forbidden_and_unknown_keys_are_refused` (`gate: {z: 1.0}`, `budget`, `noise`, `brief.bogus`); `ranges_and_sizes_are_enforced`; `readers_are_silent_until_a_version_is_adopted` (and a reader never creates the repo); `adopted_defaults_change_no_byte_of_the_schedule_file_or_the_brief` (through the frozen `schedule_file::text` and `brief::render` call sites); `adopt_switches_the_active_version_and_running_studies_keep_their_pin` (register idempotent, list newest first, Show, Revert, unknown version refused); `snapshot_captures_the_worktree_and_refuses_a_bad_candidate` (clean tree = HEAD, dirty tree = stash commit, worktree left uncommitted, `gate:` refused as "mechanism refused").
- P (suite): `score_normalises_the_holdout_gain_against_the_reference`; `pairs_use_the_cases_both_trees_scored_and_average_the_reference`; `step_asks_for_the_incumbent_rerun_and_maps_the_decision` (Confirm at age 2, Improved with `rule: paired`, a per-fixture regression past 0.25 rejected, no shared cases = n 0); `setup_rows_round_trip_and_refuse_unknown_fixtures`; `inner_slugs_fit_the_slug_rule`; `every_embedded_fixture_materialises_and_its_reference_is_the_scope_file` (+ inner custody lists).
- T (automated, `outer_suite_study_runs_inner_studies_decides_and_registers_a_version`): a `meta` study on the real mechanism repo through a test `Backend` (scripted samples: inner holdout 100 → 50 under the base mechanism, 100 → 40 under a candidate, reference 10 via `plugin_command`) and a fake scheduler standing in for the schedule actor. Observed: baseline = 2 suite runs, each `cases {toy-dedupe/1, toy-dedupe/2} = 0.556`, `asi.mechanism = base0000`; held-out holdout run at Start with `toy-lru/*`; experiment 1 `Improved`, kept, `experiments.gate = {rule: paired, n: 2, v: 2}`, candidate cases 0.667 and `best = 10`, `spend_nanos > 0` and `bench_spend(meta) > 0` (the inner spend rolled up); experiment 2 against the new incumbent `NoGain`, discarded; budget stop → Finished. 12+ inner studies, all `Finished`, 140 axes, 8 experiments each, `created_by = meta:meta/toy-…`, pins `{base0000, exp:1}`. `view` = 2 rows (training toy-dedupe 0.667 candidate; held-out toy-lru both sides); `brief_section` names toy-dedupe and never toy-lru; `headline` = `suite +0.11 over the compiled mechanism (held out +0.00) · mechanism v2 ready to adopt`; `mechanism.json` lists v2 = the outer incumbent sha with `active: None`.
- Live T (6 outer experiments on a scratch engine) is the closer's; the preset is `suite::preset` and the skill's draft.

## Hand-offs (exact diffs; files frozen for W7)

**H1 — `evalcore::gate::{paired, decide}` (W2).** Consumed through `suite::GATE`; with W2's stubs every outer decision is `NoGain` (`Unverified`). Nothing to change on merge; the seam defaults to `evalcore::gate::paired` / `decide`.

**H2 — pin the active mechanism at Start and seed the repo for a meta draft** (`crates/autoresearch/src/actor.rs`, `create`, after `study.created_by = Some("user".into());`):
```rust
    study.mechanism = crate::mechanism::active(&env.root).map(|(_, sha)| sha); // 159:W7 pin
```
and before the `is_git_repo` check:
```rust
    if m.spec.axes.artefact == Artefact::Mechanism {
        crate::mechanism::ensure(&env.root)?; // 159:W7 the repo exists before the draft is checked
    }
```
Also `setup::view_of` (the draft panel's `is_git_repo`): the same `ensure` when `d.spec.axes.artefact == Artefact::Mechanism`.

**H3 — W6's `Waiting` path (optional, when `world::resume` dispatches by kind).** `suite::sample` is already start-then-wait; to go async, split at `wait_terminal`: return `Sample { value: None, failure: None, .. }` after `start_inner` and `GateStep::Wait { until: <now + 3 h>, why: "inner" }` from `step`, and have `world::resume` call a `suite::resume(env, slug, n)` that runs the scoring half (`score_inner` over the `created_by = meta:<slug>/…` studies of that `n`) and closes the experiment. Not done here: `resume` has no kind dispatch and `finish_measure` cannot be entered from `Waiting` (C-g).

**H4 — done under R1 (no hand-off).** `SuiteSpec` is defined in types.rs (W7-a); the closer unions the adjacent `block_specs!` edits with W2's and W6's.

**H5 — toy-pareto and toy-route rows in `suite::FIXTURES`** once W1 and W4 land (my file; Z adds two `Fixture { .. }` entries with `fixture_files!("toy-pareto", "<scope>", …)` and a `reference/`). Until then `setup_set("suite.training", "toy-pareto")` is refused with "unknown fixture", and a suite run skips a fixture this build lacks (a `tracing::warn!` and `missing: […]` in the `suite` event; a suite with no known fixture is refused), so the §14 preset trains on three fixtures and holds out two in this tree. Also toy-sort ships no `reference/`; W7 embeds `sorted` as its reference — C may add `fixtures/toy-sort/reference/sorter.py` for symmetry.

**H6 — apply the mechanism's parameters** (`mechanism::overlay`), one line each:
- `actor.rs` `brief_context`/the `brief` call: `let cfg = crate::mechanism::overlay(&env.root, study.mechanism.as_deref(), &env.cfg);` and pass `&cfg.brief`, `cfg.defaults.crash_retries`.
- `actor.rs` `after_experiment` (the `stop::decide(view, spec, cfg)` call): pass `&overlay(..)`.
- `actor.rs` `measure_env` novelty: `overlay(..).novelty.threshold` instead of `env.cfg.novelty.threshold`.
- `wave.rs` `assign_categories`: seed `seen` with `overlay(..)`-independent `mechanism::load(..).params.categories` when non-empty.

**H7 — hide `meta:` studies in the list unless asked** (`actor.rs` `status_env` or the UI `StudyList.tsx`): filter `created_by.starts_with("meta:")` by default.

**H8 — `review::advise` root** (`review.rs`): `rt.data_dir.join(crate::DIR)` ignores a configured `root:`; use the actor's `env.root` when `advise` gains an `Env`.

**H9 — Settings › Autoresearch › Mechanism pane (Z, C-e):** `autoresearch.mechanism {op: list}` → active version and the list; Show per version; Adopt / Revert (`adopt {version: 0}`) presses.
