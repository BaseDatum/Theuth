# 140 as-built — Z CLOSER (integration, cross-seam hand-offs, gates)

Worker 140-Z, 2026-10-08, branch `feat/140-autoresearch` from c478b973 (C + W1 (R1) + W2 + W3 + W4 + W6 merged by the orchestrator). W5 WAVES was still running in parallel; `src/wave.rs` and the `slot`/`parallel > 1` branches of `actor.rs`/`tools.rs` are untouched here (the orchestrator merges W5 later; a second gate turn follows).

## Hand-offs, each with its resolution

| # | hand-off | resolution |
|---|---|---|
| 1 | merged tree did not build | one seam: `brief::Context` gained `gate`/`crash_retries` (W6) — `actor::begin_env` fills them from `cfg.gate` / `cfg.defaults.crash_retries`. Everything else compiled as merged. `Cargo.lock` committed (W3's `roster`, `parking_lot`). |
| 2 | two `scope_diff`s | **W1's `custody::scope_diff` survives** (an `evaluator_paths` hit lands in `off_limits`, so a keep is impossible — §5.3; globset; untracked counted; `-w --ignore-blank-lines`). W2's `git::scope_diff`, `git::ScopeDiff` and its local `path_matches` matcher deleted; its two tests ported onto the survivor in `git.rs` (the simulated 20-experiment study and the whitespace/hash test, with `patch_hash` asserted directly). One semantic fix found by W2's test: after `patch_hash`'s intent-to-add, a file the model then deletes showed up as changed (`status` lists the dead index entry); tracked changes now come from `git diff --name-only --no-renames -z HEAD` (both halves of a rename), untracked/unmerged from `status --porcelain=v2`. Every call site goes through `Backend::scope_diff`. |
| 3 | W6 → W3 | `brief::Context` filled (above). Steers are marked seen in exactly one place, `begin_env` after the render (`brief.rs` never writes). `start_draft` reads `tools::read_draft`; the draft file is now `{spec, origin}` as the brief says: `tools::Draft`, `write_draft(root, spec, origin)` takes the drafting session (`crates/app/src/study_draft.rs` passes `cx.session`), so the start/result cards reach the chat the card lived in whether Start came from the card press or `autoresearch.create`. `novelty::check(.., flagged_before)` is fed from the ledger (`novelty_flag {n-1}` event). `review::review` → `Flagged` becomes `Reverted` + `git::reject` under `on_suspicious: revert`, mark-only under `flag` (W3 had it right; verified). Refusal text `not an autoresearch session` is `binding::not_a_study_session()` for every study tool. |
| 4 | W1 → W3 holdout | New in `actor.rs`: `holdout()` runs `holdout.command` through `runner::sample` (a spec clone with `bench.command` swapped, role `Holdout`, logs `runs/<n>/holdout/`), `holdout.repeats` (default `bench.repeats`) samples, through the slot. Base holdout at `prepare` (n = 0); every `holdout.every` keeps on the new incumbent inside `Log` (after `ExperimentFinished`, before the stop rules). `check_overfit`: incumbent holdout worse than the base's beyond 2σ_holdout while the search metric improved → `overfit` event (the stop table already treats it as a plateau; `needs_eyes` already lists it). Never in the brief or a tool result. |
| 5 | W1 → W3 confirm / drift | The sample loop now honours `GateStep::Confirm.more`: it takes `more` candidate samples (plus the incumbent sample when asked) before consulting the gate again (`need`), where W3 consulted the gate after every sample (the gate decides on means at n_c ≥ 2, so `confirm_runs: 2` was effectively 1). After a `drift` event the next confirm takes 2× `more` (`confirm_doubled {n}` event marks it consumed). At the cap the gate decides on what was measured. Also fixed: the gate's candidate vector included the incumbent re-sample (`clean_values(&taken)`); it is candidate-only now. |
| 6 | Restore harness-driven | `ControlOp::Restore {n}` records `restore {n}` and the ★ priority-3 idea (now only the explanation). At the next `experiment_begin` (after drift repair) the actor applies W2's `git::restore_rejected` itself (`Backend::restore_rejected`), records `restore_applied {n, of}` (or `failed`), and the brief's incident line tells the model the change is already in the worktree and to `experiment_run` it as is. `experiment_run` of that experiment skips the novelty duplicate refusal (`restoring()`), so the identical patch hash is not refused. Test `review_reverts_a_suspicious_keep_and_restore_queues_it` extended: begin applies, brief says so, run is not refused. |
| 7 | `cost_nanos` | Filled: `Begin` records `begin {n, spend_usd}` (the `SessionCostQuery` it already makes for the money cap); `Log` queries once more (it already did, for `budget_left`) and writes `spend_now − spend_at_begin` into the row. No extra ledger query. |
| 8 | changeset branch names | `types.rs` `Changeset.branch` doc, `finalize::branch_all` doc, design §11.2 and §11.5 say `autoresearch/<slug>+<k>-<name>` with the reason. |
| 9 | W4 → engine | `StudyChartPoint.verdict` = the ledger state tag (`ExperimentState::tag()`, new), a discard refined by its verdict (`worse` / `guard_failed` / `no_gain`) — the UI's `chartOutcome` vocabulary. `autoresearch.experiments` already includes the open/measuring row (`list_experiments` has no state filter) and `StudyDetail.steers` was already newest first (`ORDER BY id DESC`) — verified, unchanged. |
| 10 | contract additions | `ControlOp::Merge {changeset}` → W2's `git::merge_into_checkout` (finished studies only; refuses a dirty or detached user tree; `merged` event; never pushes). `ControlOp::KeepReverted {n}` → `kept_reverted` event; `dismissed_reverts()` (restore ∪ kept_reverted) drops the needs-eyes line and the attention row. `StudyDetail.tail` = `runner::tail` of the newest `*.log` under `runs/<n>/` while `PHASES` says an experiment is in flight. `autoresearch_control` accepts `keep_reverted` and `merge` (n = the changeset). UI: `Finished.tsx` Merge is the real op; **Open PR stays a copy button** and now copies `git push origin <branch>` + `gh pr create --head <branch> …` (`prCommands`); the branch footnote shows the real branch. `SidePanel.tsx` needs-eyes rows are the engine's lines only (no synthesized reverted rows) with Restore (live studies) + Keep reverted. `StudyView.tsx` `Current` shows the tail; `queries.ts` refetches the detail on `ExperimentProgress`. Fixture mirrors `tail`, `keep_reverted`, `merge`. |
| 11 | W6 → UI | `autoresearch_draft` added to the `ArtifactCard` list in `components/chat/MessageTool.tsx`; `slash.ts`: bare `/autoresearch` navigates to `/autoresearch` (the Library tab), with words it hands the `autoresearch` skill to the model as before. |
| 12 | side-call kind | `CallKind::Autoresearch` added in `crates/runtime/src/side_call.rs` (`as_str`/`from_str`); `model_calls.kind` is plain TEXT (migration 0015, no CHECK), no exhaustive match elsewhere, no UI string use → plain enum addition. `review.rs` uses it for reviewer and advisor calls. |
| 13 | design text | §8.1: `memory: clear` is commented with the as-built rule (only with `when:`; a clock-only fire clears the frame anyway, 34 D4) and the `when:` example is a bare sensor file name. §10 item 2: children are bound to the study `Toolset`, not `tools: mutating`. §11.2/§11.5: branch names and the landing buttons as built. |
| 14 | lints / fmt | `derivable_impls` ×4 and the review `too_many_arguments` were already gone on the merged tree (C's `ff687b57` fixed its files; W6 had an `allow`). The `allow` is removed anyway: `review::prompt(study, &ExperimentRecord, diff)` — the row already carries every placeholder. `cargo clippy -p autoresearch --all-targets -- -D warnings` clean; `cargo fmt -p autoresearch` applied (touched only files already in the diff; `wave.rs`/`slot.rs` untouched). |

## Other changes

- `crates/autoresearch/skill.md`: landing (`merge`, Open PR copies commands, branch names) and Restore · Keep reverted as built.
- `tools.rs` `ControlTool` description lists `keep_reverted` and `merge` (W6's text already extended C's §12.4 wording with `start`/`start_window`; recorded here as a second deviation from "verbatim").

## Gates (merged tree, stable rustc 1.96.1, `BUILD_LABEL=140-Z build-lock`, `CARGO_BUILD_JOBS=4`)

Interactive, before the detached run:

- `cargo check -p autoresearch -p rpc -p app -p tui -p theuth` — Finished (1 m 37 s after the seam fixes).
- `cargo test -p autoresearch` — **106 passed, 0 failed** (C 9 + W1 35 + W2 17 + W3 23 + W6 20 + the ported scope-diff tests + the extended restore test).
- `cargo test -p app` — **46 unit + 1 integration passed**. One earlier run had `helper::tests::wrong_hash_or_missing_binary_never_serves` time out (a pre-140 test spawning a helper binary; the machine was under three other builds); it passed on the rerun.
- `cargo clippy -p autoresearch --all-targets -- -D warnings` — clean.

Detached gate (`/tmp/140-z-gate.sh` → `/tmp/140-z-gate.log`), each step through build-lock:

| step | rc | time | notes |
|---|---|---|---|
| `cargo fmt -p autoresearch -- --check` | 0 | 247 s (queued behind two other sessions) | |
| `cargo clippy -p autoresearch -p app --all-targets -- -D warnings` | 0 | 83 s | |
| `cargo test -p autoresearch -p app` | 101 → **0** | 50 s / 172 s | first run: `helper::tests::wrong_hash_or_missing_binary_never_serves` (pre-140 helper-spawn test, 2 s deadline) timed out under load; rerun **106 + 46 + 1 passed** |
| `cargo test -p rpc --lib` | 0 | 408 s | |
| `BUILD_SLOTS=1 just ts-desktop` | 0 | 320 s | 746 bindings copied (C's 756 minus the 10 Rust-named duplicates ts-rs now skips); `ControlOp.ts` has `keep_reverted`/`merge`, `StudyDetail.ts` has `tail` |
| `bun run check` (crates/desktop/ui) | 127 → 1 → **0** | 149 s / 20 s | this worktree had no `node_modules`: `bun install --frozen-lockfile` in `crates/desktop/ui` and `packages/apps`; then two real errors — `components.test.ts` lacked the new `tail` prop (fixed) |
| `bunx vitest run` (whole UI, BUILD_SLOTS=1) | 1 → **0** | 100 s / 187 s | **201 files, 1401 tests passed**; the one real failure was `nav/SectionBand.test.ts` pinning seven Library tabs (R1-D4 makes eight; re-pinned with the Autoresearch tab) |
| `bun run build` (tsc -b && vite build) | 127 → **0** | 8 s | |
| `BUILD_SLOTS=1 cargo check --workspace --all-targets` | 0 | 407 s | the single workspace-class run |

## Second turn — W5 WAVES merged (fc85c76c, `140-as-built-W5.md`)

- `git merge --no-ff feat/140-w5`: one conflict (`actor.rs` begin_env, allocate-n): kept W5's `wave::open_env`-or-one-row block and Z's `begin {n, spend_usd}` event after it. W5's identical seam fixes (`..Headline::default()`, `brief::Context { gate, crash_retries }`) merged to one copy.
- W5's test-double `scope_diff` override in `wave.rs` (it routed to W2's `git::scope_diff`, deleted in turn one) removed; the real `custody::scope_diff` serves the wave tests.
- W5 hand-off (`render_log`): a slot keep before promotion (`applied == kept && commit != incumbent.sha`, W5's tell) now reads "This keep is a wave candidate: it awaits the wave's promotion once every slot has logged. Incumbent still …" instead of the sequential incumbent line; test added in `tools.rs`.
- Harness Restore is applied only when the study is not a wave (`wave.is_none()`): a wave's candidates live in slot worktrees, so the ★ idea alone carries it there (recorded limit).
- Shared-target incident: after the merge `review.rs` failed on `CallKind::Autoresearch` although `crates/runtime/src/side_call.rs` had it — a sibling worktree had rebuilt `runtime` into the shared target from its own source; `touch` of the file rebuilt it (same class of incident W1/W2/W6 recorded).

Gates after the merge: GATE2_TABLE

## Still open

- Live proof on `fixtures/toy-sort` (single and 2-slot wave) and the gaming probe are the orchestrator's next step.
- The attention row text still says "· Restore · Keep reverted"; the inbox UI has no per-source action buttons, so both are pressed from the Study view (or `autoresearch_control`).
- `Phase` has no `holdout` chip; the in-study holdout runs after `ExperimentFinished` with no progress event (it is invisible to the brief by design and short).
