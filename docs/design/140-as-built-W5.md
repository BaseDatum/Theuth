# 140 as-built — W5 WAVES (parallel mode, design §10)

Worker 140-W5, branch `feat/140-w5` from the merged integration head e7b20015 (C, W1, W2, W3, W4, W6 real). Owns `crates/autoresearch/src/wave.rs` and the `slot` / `parallel > 1` branches of `actor.rs` (`begin_env` / `run_env` / `log_env`) and `tools.rs`. Every public signature C wrote is unchanged (`wave::{open, resolve, promote}` keep their shapes); the sequential paths of W3/W6 keep their behaviour, with the small insertions listed below.

## What was built, file by file

### `src/wave.rs`
- **Children.** A process-wide table `child session → (parent, slot)`. `on_agent_event` (called from `binding`'s `AgentEvent` subscription): `SubagentSpawned { parent, child }` with `parent` = a bound study session of a `parallel > 1` study registers the child and sends `agent::SetToolset(child, binding::toolset(1))` — the study toolset minus `spawn_subagent` (§8.3), no `tools` cap so the child keeps `shell` for its quick tests. `TurnFinished(child)` drops the binding and closes the child's slot as `abandoned` when it never logged (`measuring` is left to the settle). `TurnFinished(study session)` of a wave study settles off the mailbox (below). `slot_of(session)` is what the tools use to fill `slot`.
- **`open(a, slug, wave)` / `open_env`** (§10 step 1): `N = parallel` candidate worktrees `<study>/cand-<k>` via `git worktree add --detach <incumbent>` (an existing dir is removed first), `N` experiment rows with `wave`, `slot = k`, `parent_sha = incumbent`, `runs/<n>/` created, one `wave_open {wave, slots}` event. Categories are distinct per slot: the user's ★ open ideas first (one slot each, `idea_id` set), then the categories seen in the study (experiments oldest-first, then the backlog) rotated by `wave − 1`; slots past the known categories get `""` = "a category no other slot has".
- **`resolve(a, session, slot)` / `resolve_env`** (§10 step 2): the session is a registered child of the study session (mapped to its parent) or the study session itself; any other session gets `not an autoresearch session`. The slot's experiment is `dangling_experiment(slug, Some(slot))`; the worktree is `cand-<slot>`.
- **Supervisor / child brief blocks.** `supervisor_block` is appended to the study brief at the supervisor's begin: the spawn protocol (one native child per slot, `cwd` = the slot worktree, no `tools` cap, the same model; the verbatim text to give each child; wait for all) and one line per slot (experiment number, worktree, category, ★ idea). `child_block` is appended at `experiment_begin{slot}`.
- **`promote(a, slug, wave)` / `promote_env`** (§10 step 4): per-study `tokio::Mutex` + idempotent on the `wave_promoted` event. Slots still `open|measured` are abandoned, `measuring` ones `interrupted` (samples `lost`). Keep-candidates (`kept` rows with a candidate commit) are ordered by Δ then z. The best is `cherry-pick`ed onto the study branch, its slot samples are retagged to the pick, and one confirm sample (more only while the gate says `Confirm`, ≤ `1 + confirm_runs`) decides with W1's gate against the incumbent's samples; a keep moves the incumbent and runs W6's reviewer exactly as the sequential log does (`Flagged` + `on_suspicious: revert` → `git::reject`, incumbent back, `review` event, `rejected_by_review`). Each further candidate is picked only when its files (`files_of`) are disjoint from every promoted pick; a conflict or an overlap loses (`discarded`, line `lost wave w: overlaps experiment m on <files>` / `conflicts with …`, the candidate commit's diff saved as `runs/<n>/candidate.patch`). A combined pick is confirmed on its own samples only (its slot samples were measured against the old incumbent), with `checks` run again, and its `parent_sha` becomes the commit it was picked onto (the reviewer diffs `parent..commit`). A failed confirm resets the branch to the incumbent (`git::discard`, the pick's diff kept as the patch). Then every candidate worktree is removed, σ re-pooled, `wave_promoted {wave, promoted, incumbent}` recorded.
- **`settle`**: every unpromoted wave of the study is promoted (abandoned slots count as discard), stale slots of promoted waves are closed, then the §8.4 rules (`actor::apply_rules`); returns the stop reason and one note line per wave for the next brief.
- **`Promotion`**, `EVENT_OPEN = "wave_open"`, `EVENT_PROMOTED = "wave_promoted"`, `cand_dir`, `register_child`, `slot_of`.

### `src/actor.rs` (slot / `parallel > 1` branches only)
- `Backend` gained four git seams with W2 defaults: `worktree_add_detached`, `cherry_pick`, `head`, `files_of`.
- **Bench queue** (§5.5, §10 step 3): `bench_sample` wraps `Backend::sample` in a process-wide `tokio::Mutex` taken only when `bench.exclusive`; `run_env`'s candidate and incumbent samples and the promotion's confirm samples go through it (W1's machine slot is still taken inside). Smoke, checks and edits never queue.
- `begin_env`: `slot: Some(k)` → `begin_slot` (binds the child, re-renders the brief for the slot's `n`, appends `<slot>`). `parallel > 1` → `wave::settle` first (the previous wave; refused with "the study is finalizing" when the rules stop the study), the incumbent re-read, then the sequential prologue (budget, drift repair, re-measure, explore, code map) and `wave::open_env` instead of one row; `<wave>` block appended, `Brief.slots` filled, one `ExperimentStarted { slot }` per slot.
- `run_env`: the prologue resolves `(study, experiment, worktree)` through `wave::resolve_env` when `slot` is set; the rest of the path is the sequential one on the candidate worktree.
- `log_env`: same prologue; a slot keep is `git::keep` in the candidate worktree (a detached commit, samples retagged to it, state `kept`, no incumbent move, no review); discards are the sequential path on the candidate worktree. The stop rules wait for the wave: when every slot of the wave has logged, the logging child's call promotes (`wave::promote_env`), the report's `applied/commit/review/incumbent` are re-read, then the rules run.
- Helpers opened to `wave.rs` as `pub(crate)` (no behaviour change): `worktree_of`, `incumbent_of`, `gate_cfg`, `mean`, `clean_values`, `segment_sigma`, `secondary_means`, `mark_lost`, `retag_samples`, `metric_line`, `apply_rules`, `progress`, `Env::{runs_dir, evaluator_dir}`.
- The closer's uncommitted fix (`brief::Context { gate, crash_retries }`) is applied identically so the crate compiles on this branch.

### `src/tools.rs`
`experiment_begin` takes `BeginArgs { slot: Option<u8> }` (was `NoArgs`; `slot: 0` refused); `experiment_run` / `experiment_log` fill `slot` from `wave::slot_of(cx.session)` — the model never passes it (the brief's "filled by the plugin").

### `src/binding.rs` (one insertion, W3's file)
The `Message<AgentEvent>` handler calls `wave::on_agent_event` before the existing `TurnFinished` path. `on_turn_finished` is unchanged: slot experiments have `slot IS NOT NULL`, so its `dangling_experiment(slug, None)` never sees them.

## Gates

(see the end of this note; filled after the runs)

## Deviations (closest correct thing)

1. **Children call `experiment_begin{slot}` first** (the brief's protocol), which is how the plugin learns a child's slot; §10 step 2's `experiment_run{slot}` / `experiment_log{slot}` arguments are not model-facing — `slot` is filled from the session binding. `Begin.session` for a child is the child session; `resolve` maps it to the parent (C's doc says "or a wave child's parent": both are accepted).
2. **`tools: mutating` (§10 step 2) is not used**: a `mutating` cap removes `shell` (brief §"Build"); the child is confined by the study toolset minus `spawn_subagent` instead.
3. **Promotion runs inside the last child's `experiment_log` call** (its `LogReport` carries the promoted state and the new incumbent) and, for abandoned waves, off the mailbox at the supervisor's `TurnFinished` or at the next fire's `experiment_begin`. A slot child's own `LogReport` for a keep says `kept` with the candidate commit while the incumbent is still the old one; the wording "incumbent now …" of W6's `render_log` is right only for the last child.
4. **A combined pick's `parent_sha` is the commit it was picked onto**, not the wave's incumbent (§12.6 says "incumbent sha at the wave start"): the reviewer and the detail diff read `parent..commit`, and with the wave's incumbent the second pick's diff would include the first.
5. **Combined picks run `checks` again**; §10 only names the confirm sample. A combination was never tested as such.
6. **Confirm takes `1 + confirm_runs` samples at most** (one, then more only while the gate still says `Confirm`); an undecided gate after that discards.
7. **A `measuring` slot at settle time is marked `interrupted` without cancelling the bench** (the cancel token is per study and would kill the other slots); a run that outlives its wave is swept at the next settle.
8. **The supervisor's `experiment_begin` runs the drift repair / re-measure on the study worktree as the sequential path does** before opening the wave; `n` of the stray patch is the first slot's number.

## Hand-offs

- **Closer:** `binding.rs` has the one-line hook (`wave::on_agent_event`); `types.rs` doc on `Begin.session` could say "a wave child or the study session". The desktop/TUI do not need anything: `Brief.slots` and `ExperimentStarted { slot }` were already in C's types.
- **W6 (`render_log`):** for a slot keep before promotion the line could read "kept as a wave candidate; the harness promotes when every slot has logged" — `LogReport` has no field saying so; `applied == Kept && commit != incumbent.sha` is the tell.
- **W4 (UI):** `wave_open` / `wave_promoted` events and the `wave` / `slot` columns are there for a per-wave grouping if wanted.
