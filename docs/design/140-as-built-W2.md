# 140 as-built — W2 RATCHET (git, finalize, knowledge record)

Worker 140-W2, branch `feat/140-w2` from C's skeleton 4aa41bdc. Owns `crates/autoresearch/src/{git,finalize,record}.rs`. Every public signature C wrote is unchanged; additions are listed under hand-offs.

## git.rs (§6, §5.3 scope, §9.1 revert, §11.5 landing)

One writer: `git -C <dir> -c core.hooksPath=/dev/null -c commit.gpgsign=false …` with `GIT_TERMINAL_PROMPT=0`, `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` unset, author/committer `theuth autoresearch <noreply@theuth.local>` unless the caller passes `autoresearch.git.author`. Reads use the CLI too (gix for reads was the design's text; one tool, one spelling of paths — recorded as a deviation).

- `resolve_base` — `rev-parse --verify <base>^{commit}`; refuses a non-repo, an unknown ref, a dirty tree without include-dirty, and include-dirty with `base != HEAD`.
- `worktree_add` — `worktree add -q -b autoresearch/<slug> <wt> <base>`; when the checkout is dirty and `base == HEAD` the tracked diff (`diff --binary HEAD`, applied in the worktree) and the untracked non-ignored files (`ls-files --others --exclude-standard`, copied) are committed as `autoresearch(<slug>): base snapshot` on the study branch only. Returns the worktree's HEAD (the study's base).
- `worktree_remove`, `worktree_add_detached` (finalize scratch, wave candidates), `head`, `branch`, `is_git_repo`, `study_branch`, `rejected_ref`.
- `keep` — `add -A -- . :(exclude)<evaluator path>…` (trailing `/` stripped), refuses when nothing is staged, commits `KeepMeta::message()` (subject + the five trailers of §6).
- `discard` — `add -A -N` (intent-to-add, so untracked files are in the diff) → `diff --binary <incumbent>` → `runs/<n>/candidate.patch` → `reset -q --hard <incumbent>` → `clean -fd -q`; returns the patch's sha256. `apply_patch` re-applies a saved patch (`git apply --binary`).
- `repair` — HEAD/branch/clean check; on drift saves `stray.patch` the same way, resets, cleans, `checkout -q -B autoresearch/<slug> <incumbent>` when the branch was wrong; returns the incident sentence.
- `reject` — `update-ref refs/autoresearch/<slug>/rejected/<n> HEAD` then reset to the previous incumbent. `restore_rejected` — `cherry-pick --no-commit <ref>` + `reset -q` → the rejected change as a new uncommitted candidate; a conflict is refused and the tree left clean.
- `scope_diff` → `ScopeDiff { touched, deviations, off_limits, evaluator, added, removed, hash }` from `diff --name-only <incumbent>` (after intent-to-add; equivalent to `status --porcelain=v2` ∪ commits past the incumbent) and `diff -w --numstat` (whitespace-only lines excluded); `ScopeDiff::simpler()` is keep-simpler's line rule. `patch_hash` — `diff -w`, `index`/`@@` lines dropped, trailing whitespace trimmed, sha256. `path_matches` — the glob dialect of `scope:`/`off_limits:`/`evaluator_paths:` (`**`, `*`, `?`, bare dir = prefix) in ~30 lines; `globset` is in the workspace but not in the crate's manifest (C's file) — swap in if W1's custody wants the same matcher.
- `diff_text` (capped), `files_of` (`diff-tree --root`), `cherry_pick` (`Ok(false)` on CONFLICT after `--abort`), `branch_at` (`branch -f`, local).
- Landing: `merge_into_checkout` (`merge --no-ff -m …`, refused when the user's tree is dirty or detached, `merge --abort` on conflict — the one write in the user's checkout, on the explicit press only); `discard_study` (removes every worktree under the study dir, `autoresearch/<slug>`, `autoresearch/<slug>/*`, `refs/autoresearch/<slug>/*`); `checkout_state`.

## finalize.rs (§11)

`run(store, study, reason, rt)` reads `autoresearch:` via `rt.section` (defaults when absent), root = `cfg.root(&rt.data_dir)`, cancel = `rt.shutdown`. Steps are `events` rows `kind = "finalize"`, `payload.step ∈ {confirm, changesets, report, record}`; a step whose row exists is skipped (restart resumes).

1. `confirm` — scratch worktree `finalize/base` at the base; `bench.repeats` rounds of base-then-final through `runner::sample` (role `Final`, n = 0, logs in `runs/final/`); holdout likewise with `spec.bench.command` swapped for `holdout.command` (role `Holdout`). Noisy/lost samples excluded. Headline `name base → final unit (±x %, CI ±y %)` with CI = 1.96·σ_pooled·√(1/n_b+1/n_f); a tree whose every sample crashed falls back to the ledger (segment baseline / incumbent mean) with a note.
2. `changesets` — `group()` (union-find over `files_of` of kept, non-reverted commits; `k` by first experiment; name = common directory's last segment or first file's stem, `[a-z0-9-]`, ≤ 24). `resolve()` cherry-picks each changeset alone onto the base in `finalize/cs-<k>`; a CONFLICT merges it into `partner()` (the changeset holding the newest earlier keep, else the oldest later one) and restarts; bounded by kept+1 rounds. Each resolved changeset is measured `bench.repeats` times in its scratch tree; Δ alone vs the confirmed base; "clears the gate alone" = Δ > max(z·σ·√(1/n+1/n_base), min_effect) with σ = confirm σ else the study's; a miss sets `only_with` to every other changeset (ponytail: pairwise measurement would name the exact partner). Δ together = Σ of its experiments' ledger `delta_pct`. Review = worst verdict of its keeps. `branch_all()` → `autoresearch/<slug>/<k>-<name>` at each scratch HEAD, scratch removed. Changesets persist in the step's payload; `changesets(store, slug)` reads them back for `StudyDetail`.
3. `report` — `assemble()` builds `FinalReport` (headline, changesets, needs-your-eyes: review flags/errors, reverted keeps, deviations with justification, holdout moving against the metric, resolve notes; dead ends grouped by category; experiments/kept/cost/hours with paused time excluded). `FinalReport::tables()` is the markdown the plugin owns; it is written to `<slug>/report.md` and to a `CreateNote` document `Autoresearch: <slug>` when the documents actor runs (the returned id; else the file path). `report(store, slug)` reads the value back.
4. `record` — `record::write`; a failure is logged in the step payload, finalize still completes.

Not here (W3): the Deliver message, the attention row, the finalize turn's prose, the chart block. W3 inserts `tables()` after the prose turn (`ImportMarkdown`).

## record.rs (§7.3)

`write` → `knowledge::Put { id: records/autoresearch-<repo-name>-<slug>-<date>, type: record, tags [autoresearch, slug, repo-name], scope: repo, status tidy }` through `KnowledgeActor` → `Handle` as dreaming does; `Source::Harvest` (the harness wrote it; there is no autoresearch source variant and adding one is a knowledge-crate change). Body: goal, metric, result, base → final, kept changesets (from the finalize step; the kept rows when finalize has none), ≤ 15 dead ends with lessons, `## Playbook`; capped at `knowledge::BODY_MAX`. `seed_playbook(repo)` lists `records/autoresearch-<repo-name>-` with type/tag filters, newest date last, returns the `## Playbook` section of that body. Without the knowledge actor: `write` refuses, `seed_playbook` is `None`.

## Gates

See the DONE message for the numbers of the final run: `cargo test -p autoresearch` (my 17 tests: 10 git, 5 finalize, 2 record), `cargo clippy -p autoresearch --all-targets`, `cargo fmt -p autoresearch` (only my three files changed).

## Deviations

- Reads through the git CLI, not gix (design §6 text; brief allows it).
- Untracked files reach a diff through intent-to-add on the study worktree's index (never the user's); the design's "`status --porcelain=v2` + `diff --numstat`" becomes `diff --name-only` + `diff -w --numstat` after that step — same information, one pass, commits past the incumbent included.
- Glob matching is a local matcher, not `globset` (manifest is C's).
- Changeset "only helps together with #k" names every other changeset rather than the exact one.
- Finalize's measurement path (`runner::sample`) is W1's; its tests here cover grouping, dependency merge, branches and resumable steps with real git, not a fake bench — the closer's live smoke covers the measured path.

## Hand-offs

- W3 (actor): call order at Start — `git::resolve_base` → `git::worktree_add` (its return is the base sha); per experiment `git::repair` at begin, `git::scope_diff` at run, `git::keep`/`git::discard` at log, `git::reject` on SUSPICIOUS, `git::restore_rejected` or `git::apply_patch` for Restore; `finalize::run` on stop (then Deliver + attention + the prose turn with `finalize::report(...).tables()`); `finalize::changesets` for `StudyDetail.changesets`; `git::merge_into_checkout` / `git::discard_study` for the landing buttons; `record::seed_playbook` at Create.
- New public items (my files only): `git::{AUTHOR, study_branch, rejected_ref, apply_patch, restore_rejected, path_matches, ScopeDiff, scope_diff, worktree_add_detached, checkout_state, merge_into_checkout, discard_study}`, `KeepMeta::message`; `finalize::{EVENT, FinalReport, resolve, branch_all, changesets, report, assemble}`, `Headline` gained `sigma`, `n` and serde derives; `record::{TAG, PREFIX, repo_name, record_id, body, playbook_of}`.
- If a `Source::Autoresearch` variant is wanted in `crates/knowledge`, `record::write` is the one line to change.
