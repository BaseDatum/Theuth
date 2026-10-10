# 159 as built — W2 PROMPTS (tuning prompts: prompt and skill studies on case sets)

Branch `feat/159-w2-prompts`, cut from C's skeleton 1a70e5fe. Implements §7 (W2) and §13 (dreaming and prompt studies) against the §18 contract.

## What was built

### crates/evalcore (except `src/lib.rs`, `src/pair.rs`)
| file | items |
|---|---|
| `src/case.rs` | `split` — bucket order **val first** (W2-a) so dreaming's two-way split is `Shares{66,34,0}` bit for bit; `Check` adjacently tagged (`{"kind":"exact","value":…}`, W2-f) |
| `src/judge.rs` | `SYSTEM` (dreaming's judge prompt verbatim), `GUARD` + `system()` (§7 Guard: the answer is data), `user(ideal, references, answer)` with the answer fenced `<<<ANSWER … ANSWER>>>` (fence inside the answer is broken), `parse`, `parse_with_reason` |
| `src/gate.rs` | `margin` (dreaming's rule, reasons name the failing rule), `paired` (mean, sd, t, `critical(n)`: 2.0 from 30 pairs, the one-sided Φ(2.0)-tail Student-t quantile below — computed with a regularised incomplete beta, tested against the 0.975 table), `decide` = margin + paired when n ≥ 10 (W-r); `t_quantile` public |
| `src/run.rs` | `Model` trait, `render` (`{{artefact}}`), `run_case` (target call → check or judge), `score_check` for `exact | regex | contains | command` (stdin answer, `METRIC score=`); tokens/cost stay 0 here — the caller's adapter sees the provider reply |
| `src/rows.rs` | unchanged shape |

### crates/dreaming (`replay.rs`, `cases.rs`, `edits.rs`, `Cargo.toml` + one dep line)
- `replay.rs`: `JUDGE_PROMPT = evalcore::judge::SYSTEM`; `judge_user`/`parse_judge`/`candidate_sha256` re-export the core; `replay()` runs `evalcore::run::run_case` with a `DreamModel` adapter (`CallKind::Dream`, the dreaming session); `gate()` is `evalcore::gate::decide`; **`gate_from_rows` counts val pairs only (§13.2)**; new `ReplayMeta {split, owner, reason}` flattened onto each line (§13.5), `log_row_meta`, `rows_with_meta`; `OWNER_DREAM`.
- `cases.rs`: `split_for` = `evalcore::split(SPLIT_SEED, id, Shares{66,34,0}, synthetic)`.
- `edits.rs`: `land_text(data_dir, night, skill, text, "study:<slug>")` (§13.3: backup once, atomic write, a `replace` record naming the origin).
- Tests added: `replay::gate_from_rows_counts_val_pairs_only`, `edits::record_pass_gates_on_val_cases_only`, `edits::land_text_backs_up_writes_and_records_the_origin`; every existing test unchanged and green (the judge-prompt-in-skill test included).

### crates/autoresearch
- `src/cases.rs` (all frozen signatures filled): `case_set` (custody snapshot `evaluator/<segment>/cases.jsonl`, written once from the `file` in the worktree or from `dreaming/cases.jsonl` filtered by `skill_hint` and state, hash added to the segment's custody), `shares`, `parse_cases`, `dreaming_cases`, `leak` (8-word runs), `target_model`/`judge_model` (judge defaults to another vendor than the target), `sample` (val + ≤16 train cases, `repeats`, 8 calls in flight through `Backend::side_call`; one `judgments` row per case per sample; the log `runs/<n>/sample-<k>[-role].log` with `CASE`/`MEAN`/`METRIC`/`SPEND` lines; `Sample.value` = val mean (holdout mean for the holdout role), `case_values[id] = {score, tokens_in, tokens_out, cost}`, `cases` = train ids, `asi["train:<id>"]` = failing train feedback, `spend_nanos = 0`), `step` (val pairs → `evalcore::gate::decide`; `Confirm{incumbent_sample}` when `incumbent_age ≥ gate.remeasure_after`; `GateDecision{rule: paired, stat: t, threshold: c_n, n}`; Reject with t ≤ −c_n → `Worse`, else `NoGain`; a `paired_gate` event carries the reason), `view`/`rows_from`/`rows_of`/`rows`, `brief_section` (failing train cases only), `headline` (`val score 0.50 → 1.00 on 38 cases` + holdout line), `card_line`, `SETUP_ROWS` (`cases`, `target`, `scoring`), `setup_value`, `setup_set`.
- `src/rpc_impl.rs` (reg line): `autoresearch.cases` = `cases::rows_of(Detail{slug}.view, n)` — no new actor message needed.
- `src/types.rs` (**deviation W2-b**): `CaseSetSpec {source, path, skill_hint, artefact, render, split, repeats, max_cases, train_shown}` and `CaseSource {file, dreaming}`, placed at the end of the file; `CaseSetSpec;` removed from `block_specs!`.
- `skill/prompts.md`: "## Prompt and skill studies".
- `fixtures/toy-prompt/`: `prompt.md`, `cases.jsonl` (90 cases, 9 classes × 10, exact checks; slug `toy-prompt` → 35 train · 38 val · 17 holdout), `gen_cases.py` (byte-stable, seed 159), `README.md` with the spec.

### crates/desktop/ui
- `components/autoresearch/CaseStrip.tsx`: val and holdout staircases over the experiment columns, then the case × experiment heat strip grouped val → holdout → train, cells 0 (red) → 1 (green), hover/title per cell, split counts in the caption; exports `splitMeans`, `experimentNumbers`.

## Mapping to §7

| §7 part | where |
|---|---|
| Artefact in the user's repo | `cases.artefact` read from the worktree each sample; keep = commit unchanged |
| Artefact in a plugin-made scratch repo | **not built** (prepare_steps is frozen) — hand-off H6 |
| Case set `file` / `dreaming`, custody snapshot | `cases::case_set`, `dreaming_cases` |
| Split 40/40/20, ≥ 10 val | `evalcore::split(slug, id, shares)`; `MIN_VAL = 10` is the gate's `min_pairs` (fewer → `NoGain` + a `paired_gate` event). Start-time refusal is a hand-off (H4) |
| Runner steps 1–4 | `evalcore::run::run_case`, `cases::sample` |
| Decision rules 1–4 | `evalcore::gate::decide` with `min_effect` from `metric.min_effect` (default 0.05), `max_regression` from the headline objective (default 0.2), `min_pairs` 10; `critical(n)` per W-n |
| Incumbent re-run every `remeasure_after` | `cases::step` → `Confirm{more: 0, incumbent_sample: true}` from `gate.remeasure_after` (the 140 config knob) |
| Train feedback (GEPA) | `asi["train:<id>"]` on each sample; `cases::brief_section` lists ≤ 8, lowest scores first |
| Holdout every `holdout.every` and the overfit flag | the 140 path: `holdout()` → `cases::sample` with role `Holdout` runs the holdout split; `check_overfit` unchanged |
| Ledger | `judgments` rows (`b_sha` NULL, score, split, model, cost), `samples.case_values` |
| Study view | `KindView::Cases` + `CaseStrip.tsx` |
| Setup rows | `cases`, `target`, `scoring`; `ProbeKind::Cases` **not built** (H3) |
| Draft card | `card_line` counts the file's cases by split when readable |
| Skill text | `skill/prompts.md`; the judge-only line of `skill.md` is a hand-off (H7) |
| Guards | split in code; `leak` refusal before any call (a crash sample whose tail says `refused`); case set in custody; judge fence + guard sentence; other-vendor judge default |

## Mapping to §13

| §13 | where |
|---|---|
| 13.1 shared core | evalcore as above; dreaming re-exports |
| 13.2 split fix | `replay::gate_from_rows` keeps val pairs only; a train replay logs, never counts; tests above |
| 13.3 who may write | `edits::land_text`; the Merge-press call is a hand-off (H5) |
| 13.4 skill studies read dreaming's cases | `cases::dreaming_cases` (`context` → input, `ideal` + `user_words` → judge references, synthetic → train, states open/reopened/closed) |
| 13.5 replay rows | `ReplayMeta` beside the unchanged `ReplayRow`; the reader derives the split from the case store |

## Deviations

- **W2-a** `evalcore::split` buckets are val-first (`< val → Val`, `< val+train → Train`, else Holdout) instead of C's train-first, so dreaming's existing split is bit-identical; shares and stability unchanged (C's test passes as written).
- **W2-b** `CaseSetSpec` fields and `CaseSource` live in `types.rs` (frozen). §5.1 assigns the block to W2; the skeleton's empty struct would silently drop every `cases` key. Added at the file's end; one two-line deletion in `block_specs!`.
- **W2-c** W-l (val share from dreaming's `gate.val_fraction`) is not wired: `cases::append` (called from `tools.rs`, frozen) has no config; `VAL_PERCENT = 34` stays and equals the default `val_fraction`.
- **W2-d** A replay row whose case is **not in the case store** and has no stored split counts as before 159. Only pre-159 rows can be there (`replay()` reads the case first), and the existing tests in `undo.rs`/`edits.rs` use such ids.
- **W2-e** A prompt study's `bench.command` is the word `cases`: `actor::create` and the setup panel still require a bench command (frozen); `custody::take` treats it as a program name and nothing runs it. `setup_set` fills it when a `cases.*` leaf is set.
- **W2-f** `evalcore::Check` is adjacently tagged (`kind` + `value`): serde cannot internally tag a newtype string.
- **W2-g** `ProbeKind::Cases` and the cost estimate from a probe are not built (types.rs/setup.rs frozen); see H3.
- **W2-h** `Sample.cases` carries the **train** ids only (the brief's "cases that moved" reader stays val-blind); `case_values` carries every scored case.
- **W2-i** Per-case tokens/cost are the sample total divided by the cases (calls run 8 in flight); the sample's totals are exact.
- **W2-j** `ReplayRow`'s shape is unchanged; the three new fields are a flattened `ReplayMeta` on the same line (test literals in `undo.rs`, not W2's, construct `ReplayRow`).
- **W2-k** One `judgments` row per case per sample (repeats averaged); `model = "check"` for checker-scored cases.
- **W2-l** Headline `n` counts the val cases scored on the base; `SplitCounts` in the view counts *scored* cases (train = the 16 replayed), not the set's shares — the card line gives the shares.

## Gates (all `BUILD_LABEL=159-W2 build-lock mbx …`, same `-p` set across steps where noted)

| gate | result |
|---|---|
| `mbx test -p evalcore --locked --no-run` + bare `mbx test -p evalcore --locked` | 10 passed, 0 failed |
| `mbx test -p dreaming --no-run` (`--offline` once to add the evalcore dep to Cargo.lock; `--locked` after) + bare `mbx test -p dreaming --locked` | 64 passed, 0 failed |
| `mbx test -p autoresearch --locked --no-run` + bare `mbx test -p autoresearch --locked` | 194 passed, 0 failed (`golden_ledger_140` included) |
| final set: `mbx test -p evalcore -p dreaming -p autoresearch --locked --no-run` + bare | 194 + 64 + 10 passed, 0 failed |
| `mbx clippy -p evalcore -p dreaming -p autoresearch --locked` | clean (two fixes: `rfind`, a named accumulator type) |
| `cargo fmt -p evalcore -p dreaming -p autoresearch` | no stray files |
| UI: `build-lock bun run check` | clean |
| UI: `build-lock bun run test -- CaseStrip.test.ts components.test.ts` | 12 passed (2 new in `CaseStrip.test.ts`) |

Cargo.lock: two lines regenerated by mbx (evalcore dev-dep tokio; dreaming → evalcore).

## Fixture evidence (P, on the fake target; `cases::tests::prompt_study_end_to_end_on_the_fixture`)

Scripted through the real actor (`create → begin/run/log`) on a temp git repo made from `fixtures/toy-prompt`, production git/custody/scope/novelty, a `Backend` whose `side_call` answers a class right only when the system prompt states that class's rule:
- Baseline: 2 passes × (38 val + 16 train) + 17 holdout = 125 target calls, 0 judge calls; val mean exactly 0.50 (19/38), `cases.jsonl` hashed in custody.
- #1 two rules stated → paired gate `Improved` (`GateDecision{rule: paired, n ≥ 10, stat, threshold}`), kept; holdout after the keep 0.647 vs val 0.658.
- #2 a val input + ideal pasted into the prompt → `Crash` with tail `refused … val or holdout`, **zero** model calls, `Crashed`.
- #3 the rule words dropped → `Worse` (t ≤ −c_n, `Call::Worse`), discarded.
- #4 every rule → `Improved`, kept; val 1.00, holdout 1.00, no `overfit` event; the brief after the keeps says every shown train case passed and contains no val/holdout input or id (checked on every brief).
- View: 71 scored rows (16 train · 38 val · 17 holdout), base column and columns at the keeps; `rows_of(_, Some(2))` is empty for the refused candidate. Headline `val score 0.50 → 1.00 on 38 cases`, holdout line `→ 1.000`. 125 `judgments` rows at n = 0 with cost; candidate samples carry 54 `case_values` with tokens and cost and `spend_nanos = 0`.

**A — cost against the estimate.** Checker-scored, 38 val + 16 train: 54 target calls per candidate (+ 17 holdout calls per keep), no judge. At §7's $0.002 target that is $0.11 per experiment (+ $0.03 per keep), against §7's $0.29 target share for 48 val — "checker-scored studies cost about half" holds; the session's own spend is unchanged.

**T** (live, a cheap target) is the closer's, per the brief; the fixture's spec is in its README.

## Replay-row report (§13.5)

On this machine (`~/.theuth/dreaming/`): `cases.jsonl` holds 4 cases (3 train, 1 val, all `prompt:standing-preferences`, open, real); there is **no `replays.jsonl` and no `edits.jsonl`**, so 0 past accepted edits had any pair in their gate and none would have been refused. For all 4 cases the stored `split` equals the one derived from the id (seed `"136"`, `% 100 < 34`), which is what the val-first bucket order (W2-a) preserves.

## Hand-offs (exact diffs or sketches; the orchestrator or Z applies them)

**H1 (optional)** `crates/autoresearch/src/actor.rs` `create`, lines 1856–1858 — lift the bench requirement for engine-side scoring (the `cases` word convention works without it):
```diff
-    if spec.bench.command.trim().is_empty() {
+    if spec.axes.scoring == Scoring::Bench && spec.bench.command.trim().is_empty() {
         return Err(Error::refused("bench.command is empty"));
     }
```

**H2** `crates/autoresearch/src/setup.rs` `present`, line 225 — a prompt study has no check command; without this the panel blocks Start with "Check is missing":
```diff
-        "checks" => spec.checks.as_ref().is_some_and(|c| non_empty(&c.command)),
+        "checks" => spec.checks.as_ref().is_some_and(|c| non_empty(&c.command)) || spec.axes.scoring == Scoring::Cases,
```

**H3** `ProbeKind::Cases` (§7 Data and UI): add `Cases` to `types::ProbeKind` and an arm in `setup::probe` that, for `scoring == Cases`, runs 10 val cases twice on the base through `cases::sample`-like code and reports `median` = the val mean, `noise_pct` = the per-case repeat disagreement and `duration_ms`; `kind_of_row("cases") → Cases`; fingerprint gains the case file's sha, `models.target`, `models.judge` when `cases` is set. Not built: both files are frozen for W2.

**H4** `crates/autoresearch/src/tools.rs` `validate_spec`, after `validate_axes(spec)?;` (line 258):
```rust
    if spec.axes.scoring == Scoring::Cases {
        let cs = spec.cases.as_ref().ok_or("a prompt study needs a `cases` block")?;
        if cs.artefact.trim().is_empty() {
            return Err("cases.artefact is empty (the text file the study edits)".into());
        }
        match cs.source {
            CaseSource::File if cs.path.trim().is_empty() => return Err("cases.path is empty".into()),
            CaseSource::Dreaming if cs.skill_hint.trim().is_empty() => return Err("cases.skill_hint is empty".into()),
            _ => {}
        }
        crate::cases::shares(cs)?;
    }
```
(and `use crate::types::CaseSource;`). Start's "fewer than 10 val cases" refusal belongs beside it once the file is readable at Start (`cases::parse_cases` + `evalcore::split`).

**H5** §13.3 landing: on `ControlOp::Merge` for a study whose `cases.source == Dreaming` and `cases.artefact` is under `skills/learned/`, call `dreaming::edits::land_text(data_dir, &night, skill, &final_text, &format!("study:{slug}"))` instead of `merge_into_checkout`, and write a `NightRecord` for that night so `dreaming_undo` finds the backup. `autoresearch` cannot depend on `dreaming` (dreaming → agent → …), so the call sits in `crates/app` or the RPC layer; `land_text` is ready and tested.

**H6** §7 "scratch git repo at `<root>/<slug>/artefact` seeded with a file outside any repo": in `prepare_steps` (actor.rs), when `spec.repo` is empty and `axes.artefact == File`, `git init` `<root>/<slug>/artefact`, copy the seed file (`cases.artefact` resolved against `data_dir`), commit, and set `spec.repo` to it before `resolve_base`. Not built: actor.rs is frozen.

**H7** `crates/autoresearch/skill.md` line 18:
```diff
-- A judge-only metric ("better prose", "nicer UI") → warn: the loop will optimise the judge, not the goal. Only proceed with a numeric proxy **and** a holdout; otherwise decline.
+- A judge-only metric ("better prose", "nicer UI") → warn: the loop will optimise the judge, not the goal. Only proceed with a numeric proxy **and** a holdout, or as a **prompt study** (below) whose judge scores each case against a reference with a holdout; otherwise decline.
```

**H8** TS bindings: `CaseSetSpec.ts` and `CaseSource.ts` change with W2-b; regenerate at merge (`BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings && cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`). The UI reads only `KindView`/`CaseRow`/`SplitCounts`, unchanged.
