---
name: autoresearch
description: Set up an autoresearch study (design 140) — a measured experiment loop that optimises one benchmark number in its own git worktree, keeps only changes that beat the noise and pass a check, and reports what helped. Interview the user, write and run the benchmark once, search knowledge for earlier studies, then call `autoresearch_draft`; later answer "how is it going" with `autoresearch_status` and steer with `autoresearch_control`.
---
# Autoresearch: set up a study

You are in the user's chat. Your job is a conversation that ends in one `autoresearch_draft` call; the user's **Start** press on the card creates the study — you never start it yourself, never run experiments here, and never write into the user's checkout. The study runs unattended later in its own session with its own tools (`experiment_begin|run|log|notes`); those are not yours.

## When autoresearch fits — and when it does not

Fits when all three hold:
- **A scalar the user trusts.** One number a command prints that the user would merge a change for: wall time, allocations, binary size, tokens, a benchmark score, a loss. Direction known.
- **A benchmark under ~15 minutes** per run (ideally 1–5), deterministic inputs, reproducible on this machine.
- **A correctness check**: an existing test suite (or a filtered part of it) that fails when the change breaks behaviour.

Does not fit:
- No metric → this is a normal task; say so and do it (or offer to help define a metric first).
- A judge-only metric ("better prose", "nicer UI") → warn: the loop will optimise the judge, not the goal. Only proceed with a numeric proxy **and** a holdout; otherwise decline.
- The thing to optimise is the benchmark itself, or a flaky one → fix the benchmark first.
- A pure jj repo without git colocation, or a repo that is not git → refused at Start.

## The interview (short; defaults are fine)

Ask only what you cannot see. Read the repo first (`list_dir`, `read_file`, `code_map`, existing bench/test commands in `package.json`, `Cargo.toml`, `Makefile`, `Rakefile`, CI files). Then settle, in this order:

1. **Goal** — one sentence ("make parse+render of the fixture set faster without changing output").
2. **Repo and base** — the absolute path; `base: HEAD` unless told otherwise. Start refuses a dirty tree unless the user chooses to snapshot it: the card's **Start with my uncommitted changes** button (or `include_dirty: true` in the spec) commits the dirty tree as the base on the study branch only.
3. **Metric** — `name` (the `METRIC <name>=` line), `unit`, `direction` (`lower`|`higher`), `min_effect`: the smallest gain the user would bother to merge (`0.5%` or an absolute like `2` ms). Below it nothing is kept.
4. **Benchmark** — the command (see below), `timeout` per sample, `repeats` (3 default), `exclusive` (true for wall-clock metrics), optional `smoke`.
5. **Check** — the test command filtered to the scope when the whole suite is slow; its timeout.
6. **Holdout** — optional but strongly recommended: the same measurement on inputs the loop never scores (other fixtures, other seeds). The study session never sees its numbers.
7. **Scope and off-limits** — globs where edits are expected (`lib/liquid/**`) and what can never be kept: tests and fixtures are off-limits by default (`test/**`, `spec/**`, `fixtures/**`, `benches/**`); `evaluator_paths` lists the repo files the benchmark reads (fixtures, test data, a `Rakefile`), which are hashed and refused if changed.
8. **Budgets** — pre-filled `$20 · 120 experiments · 10 h · $1.50 per experiment`; show the estimate (below) and let the user change them.
9. **Models and parallel** — leave `models` null (defaults: executor = the session default, reviewer = a model of another vendor, advisor = the configured advisor). `parallel: 1` unless the metric is deterministic or the user asks for waves.

## Where scripts go

Scripts you write for the study — the benchmark, a holdout, a check wrapper — go to **`<data_dir>/autoresearch/drafts/<slug>/`** (`data_dir` is theuth's data directory, e.g. `~/.theuth`), **never into the user's checkout**: nothing you write may show up in a changeset or trip Start's clean-tree check. Run them from there with the repo as `cwd`. At Start the harness copies them into custody (`<data_dir>/autoresearch/<slug>/evaluator/0/`) and the study always runs the custody copy. Repo files the benchmark reads stay where they are and go in `evaluator_paths`. Set `bench.command` to the script's absolute path under the drafts directory (or to a repo command such as `cargo bench --bench parse`, whose inputs are then in `evaluator_paths`).

## Writing the benchmark

The contract (compatible with pi-autoresearch and omp):
- Runs with `cwd` = the study worktree through `/bin/bash -lc`; env has `AUTORESEARCH=1`, `AUTORESEARCH_STUDY`, `AUTORESEARCH_RUN`, `AUTORESEARCH_SAMPLE`, `AUTORESEARCH_WORKTREE`, `AUTORESEARCH_EVALUATOR` (the custody dir — read inputs you ship from there).
- Prints **`METRIC <name>=<number>`** for the primary (and any secondaries you want guarded); last occurrence wins; exit 0 with the primary present = a sample; anything else = a crash.
- Optional **`CASE <id> <name>=<number>`** per input (fixture, query, prompt): the brief shows the cases that moved most; optional **`ASI <key>=<text>`** facts (timing breakdown, cache hit rate).
- **Deterministic inputs**: fixed seeds, fixed fixture set, fixed iteration counts. **Warm up** once before timing. Take the median of several inner iterations if the measurement is noisy. **No network**, no clock-dependent inputs, no reading the user's checkout.
- Keep stdout small (the full log is kept; only a tail reaches the model).
- A seconds-long **`smoke`** command (build + one tiny input) saves a full sample on every broken edit.

A minimal shape:

```bash
#!/bin/bash
set -euo pipefail
# build quietly, warm once, time N iterations on fixed inputs
make -s build >/dev/null
./bin/parse "$AUTORESEARCH_EVALUATOR/inputs/warm.txt" >/dev/null
start=$(python3 -c 'import time;print(time.perf_counter())')
for f in "$AUTORESEARCH_EVALUATOR"/inputs/*.txt; do ./bin/parse "$f" >/dev/null; done
end=$(python3 -c 'import time;print(time.perf_counter())')
python3 -c "print(f'METRIC time_ms={($end-$start)*1000:.1f}')"
```

**Run it once by hand** (`shell`, from the drafts directory, `cwd` = the repo) and show the user its output and how long it took before drafting. Run it twice if you can and mention the spread: a benchmark whose two runs differ by more than ~5 % needs steadier inputs or more `repeats`, or Start will refuse it (σ > `noise.max_cv`).

## The check and the holdout

- **Check**: the existing suite is usually right; filter it to the scope when it is slow (`cargo test -p parser`, `bundle exec rake test TEST=test/parser/*`). It runs only when a change would be kept, so its cost is per keep, not per experiment. Exit 0 = pass.
- **Holdout**: the same objective on inputs the loop never scores — the other half of the fixtures, other seeds, another dataset. Same `METRIC` contract. Runs every `holdout.every` keeps (5) and at the end. Its numbers never reach the study session; a holdout that gets worse while the metric improves raises an overfit flag.

## Budgets and the estimate

Cost per experiment ≈ the executor's price × 30–60k tokens (reading, editing, two or three tool calls) plus the benchmark's wall time × 1.4 samples on average. Say it in numbers: *"about $0.30–0.60 and 4–6 minutes per experiment with a 3-minute benchmark; 120 experiments ≈ $40–70 worst case, capped at $20 → roughly 40–60 experiments overnight."* The per-experiment cap (`budget.per_experiment_usd`) ends a runaway experiment; the study total ends the study. A budget above the configured confirmation threshold shows a warning on the card.

## Memory from earlier studies

Before drafting, `knowledge_search` for the repo path, the metric name and "autoresearch": finished studies leave a record (`records/autoresearch-<repo>-<slug>-<date>`) with what was kept, the dead ends and the playbook. Show the hits in one or two lines; the first brief's playbook is seeded from the most recent record for the same repo automatically.

## The draft

Call `autoresearch_draft` with the spec:

```yaml
slug: sort-speed                     # [a-z0-9-]{1,32}
goal: "Make parse+render of the fixture set faster without changing output."
repo: /Users/me/Develop/liquid
base: HEAD
metric: {name: time_ms, unit: ms, direction: lower, min_effect: 0.5%}
secondary_guards: [{name: alloc_mb, max_regression: 5%}]
bench: {command: /Users/me/.theuth/autoresearch/drafts/sort-speed/bench.sh, timeout: 600s, repeats: 3, exclusive: true, smoke: "make -s build"}
checks: {command: "bundle exec rake test", timeout: 300s}
holdout: {command: /Users/me/.theuth/autoresearch/drafts/sort-speed/holdout.sh, every: 5}
evaluator_paths: [fixtures/, test/, Rakefile]
scope: [lib/liquid/**]
off_limits: [fixtures/**, test/**]
warm: "bundle install"
budget: {usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120}
parallel: 1
models: {executor: null, review: null, advisor: null}
window: null                         # or a bare `when:` sensor file name (`idle.py`, no path) for "Start in idle window"
```

The card shows five plain lines plus warnings (no check, no holdout, wall-clock without `exclusive`, empty scope, budget above the threshold) and the buttons **Start now · Start in idle window · Start with my uncommitted changes · Edit · Discard** (the third only when the spec does not already say `include_dirty: true`). Then **stop and wait**. The press arrives as the user's next message beginning `[app study-drafts …]`:
- `study draft <slug>: started` — the study is preparing (worktree, custody, baseline). Answer one line; the start card follows when the baseline is in.
- `study draft <slug>: edit` — ask what to change, then call `autoresearch_draft` again with the amended spec.
- `study draft <slug>: discarded` — nothing exists; answer one line.

If the card reports a failed start (dirty tree, benchmark crash, σ too large), relay the reason and fix the cause with the user — do not retry blindly.

## While it runs

- **"How is it going?"** → `autoresearch_status {slug}`: state, headline (metric now vs base), experiments and keeps, budget left, the current phase, the last five experiments. Summarise in two or three lines; the Study view has the chart.
- **Steering** ("tell it to try SIMD", "stop touching the lexer") → `autoresearch_control {slug, op: steer, text}`: queued, read by the next experiment, the loop does not pause. A concrete idea for the backlog → `op: idea` with `text` (and the category in `reason`); user ideas get the ★ and top priority.
- **Pause / resume / stop** → `op: pause | resume | stop_after | stop_now`. `stop_now` discards the experiment in flight; say so. **Finalize** runs when any stop fires: base vs final re-measured, independent changesets measured alone, a report document, one result card. Landing is the user's: per-changeset branches `autoresearch/<slug>+<k>-<name>`; `op: merge, n: <k>` merges one into the repo's current branch (`git merge --no-ff`, refused on a dirty tree); Open PR in the Study view copies the push + `gh pr create` commands — nothing is ever pushed by the harness.
- A keep the reviewer reverted as possible gaming shows an attention row with **Restore · Keep reverted**; `op: restore, n` makes the harness re-apply its change at the next experiment and measure it again; `op: keep_reverted, n` dismisses the row.
- The study's benchmark, check and fixtures are frozen in custody for the whole study; if the user wants to change the evaluator, `op: segment` with a `reason` opens a new segment (new custody, new baseline).

## Never

- Never start the study yourself: the press is the approval. There is no tool that starts a drafted study (`autoresearch_start` is the card's own press and needs a token you do not have); a user who asks in words after the card was shown is asked to press Start.
- Never write benchmark scripts, fixtures or state into the user's checkout; never commit there.
- Never promise discovery: this is disciplined metric search over the user's own benchmark. Most keeps are small; the gate makes them real.
