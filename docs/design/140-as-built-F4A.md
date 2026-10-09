# 140 as-built — F4A LOOP POLISH (the live proof's open observations, decided)

Worker 140-F4A, 2026-10-09, branch `feat/140-f4a` from c96bcf1d (`fix/140-tab-agents`). Brief: `~/.theuth/briefs/2026-10-08/140/140-F4A.md`. Design amendments are in `140-autoresearch.md` as "Revision 2 (2026-10-09)": §5.2 item 6, §5.3 "Regime change", §6 "Build junk", §13 Study view / Attention, the `gate:` example.

## Rulings, file by file

### 1. Regime change opens a segment automatically
- `config.rs` — `GateConfig.regime_sigma: f64` (default 10; 0 disables), in the `wire!` struct so it reaches `AutoresearchGateConfig` on the wire; `defaults_match_the_design` pins it.
- `actor.rs` — `segment_task` is now a thin wrapper over `open_segment(env, study, reason, by, SegmentBaseline)`; `SegmentBaseline::Measure` is the user segment (fresh `repeats` baseline), `SegmentBaseline::Keep {n, sigma, delta}` the regime segment: `opened_at` = the keep's first candidate sample (`incumbent_samples` / `segment_sigma` cut at `opened_at`, so the keep's samples are the segment's), baseline = the keep's clean samples, `opened_by: By::Plugin`, `reason` = `regime change at #n: Δ 50.0 ms ≥ 10σ (σ 1.00)`, event `segment_auto {segment, n, sigma, delta, reason}`. When the keep has fewer than two clean samples, one incumbent re-measure (`SampleRole::Incumbent`, `k = 150`, `runs/<n>/`) is taken at once so σ is estimable (σ = 0 would read as a deterministic metric in `gate::step`). `regime_segment(env, study, exp)` applies the rule (σ > 0, `|exp.delta| ≥ k·σ`) and turns a failure into an `incident` event like the user segment does. Called from `log_env` after the holdout block for a non-slot keep; `detail_env` renders the event as a Needs-your-eyes line `segment N opened automatically at #n — <reason>; σ restarted from the keep's samples`.
- `wave.rs` — `promote_env` judges the last promoted keep against the σ the wave was measured under (`sigma_before`), after the promotion's σ refresh.
- `brief.rs` — line after Noise: `Segment N: opened automatically|by the user — <reason>. Earlier experiments stay listed but left the statistics.` for any segment > 0.
- UI `SidePanel.tsx` — the auto-segment line gets the `info` tone.
- Tests: `actor/tests.rs::a_regime_change_opens_a_segment_at_the_keep` (σ 1 baseline, Δ 50 keep → segment 1 by the plugin, baseline (50.25, 2), σ 0.5/√2 from the keep's sample plus the top-up, the `segment_auto` payload, the detail line; Δ 5 and `regime_sigma: 0` leave segment 0 and spend no extra sample).

### 2. One Δ convention
- `gate.rs::step` — `GateStep::Decide.delta_pct = pct(x_c − x_i, x_i)` (raw); `delta` stays normalised. `deterministic_metric_uses_min_effect` pins −1 % for a lower-is-better win and +1 % for a higher-is-better one.
- `actor.rs` — `info_from`: `StudyInfo.delta_pct = pct(i − b, b)`; `metric_line` fallback no longer flips for `Lower`.
- `finalize.rs` — `changesets`: `delta_together_pct` = Σ ledger `delta_pct`, the `raw_sign` flip is gone.
- `brief.rs` — `metric_line` renders `Incumbent 39 ms = −99.6 % vs base (better)` through `direction_word(direction, raw_pct)`; test `metric_line_reads_the_raw_change_with_the_direction_word` pins lower/higher × better/worse; the fixture needle updated.
- `actor/tests.rs` — `Fake::decide(verdict, delta, z)` writes `delta_pct: −delta` (the harness studies are lower-is-better); `one_experiment_kept_moves_the_incumbent` asserts −10 %; `metric_line_reads_the_raw_change` uses −99.6.
- UI `model.ts` — `delta(direction, delta_pct) → {text, word, tone}`; `StudyList.tsx` exports the `Delta` component (raw number, colour by direction, the word on hover, `muted` for rows that were not kept); used by `StudyView` (Incumbent, Holdout), `StudyList` row, `RunTable` (new `direction` prop), `ExperimentDetail` (Δ vs incumbent; the per-case list no longer colours by raw sign), `Finished` (headline `(−20.9 %, better)`, Δ alone / together). `model.test.ts` pins the four direction × sign cases; `components.test.ts` checks the kept row's class/title.

### 3. Holdout summary never shows base → base
- `types.rs` — `StudyDetail.final_holdout: Option<String>`; `actor.rs::detail_env` fills it from `finalize::report(…).headline.holdout` for terminal studies.
- `actor.rs::chart_series` — finalize writes its holdout samples at `n = 0` (base tree and final tree alike), so the base point keeps only `n == 0 && tree_sha == base_sha`; the final tree's result is the headline's.
- UI `model.ts::holdoutSummary` returns `null` with a single checkpoint; `StudyView` Holdout card: finished → `final_holdout` with sub "base → final, measured at finalize", else `holdout not measured yet (every N keeps)`; `Finished.tsx` headline uses `final_holdout` when the series has only the base.
- Tests: `finalize_writes_the_report_prose_before_the_tables` (Fake headline now carries `holdout: Some("5 → 4 (−20.0 %)")`; `final_holdout` on the detail; a final-tree sample at n = 0 does not enter the base point), `model.test.ts` (base-only → null), `components.test.ts` (Finished headline with/without `final_holdout`).

### 4. Keeps never commit build junk
- `custody.rs` — `BUILD_JUNK = ["**/__pycache__/**", "**/*.pyc", "**/.DS_Store", "**/node_modules/**"]`; `build_junk(spec)` is empty when a `scope` entry names a junk kind; `is_build_junk(path)` (spec-independent); `scope_diff` drops junk from `changed`/`untracked` before classification and line counts; `globs` is generic over `AsRef<str>`.
- `git.rs` — `keep` adds `:(exclude,glob)` pathspecs from `build_junk(spec)`; `stage_intent` (discard/stray patches, novelty hash) excludes `BUILD_JUNK`; `dirty_paths` (drift repair, the clean-checkout check, `checkout_state`) ignores junk — otherwise every begin after a keep would see the untracked `__pycache__/` as drift.
- Test `git.rs::keeps_never_commit_build_junk_unless_scoped`.

### 5. Study sessions out of the chat sidebar and the generic attention rows
- Marker: the session title `autoresearch-<slug>` — the schedule crate names the session after the schedule file (`schedule/lib.rs new_session` → `RenameSession`), it reaches both `SessionSummary.title` and the roster's `AncestorInfo.title`; `sessions.unattended` reaches neither and `SessionKind` has no study variant.
- UI `lib/session-tree.ts::notStudy`; `SessionList.tsx::buildRows` root filter; test in `SessionList.test.ts` (study root and its wave child hidden even when open; a chat titled "autoresearch plans" stays).
- `crates/roster/src/attention.rs` — `reports_finished(chain)` (roots only, never `autoresearch-*`) replaces the inline `chain.len() != 1` check in `finish_turn`; test `study_sessions_and_subagents_report_no_finished_row`. Nothing else in the roster touched.
- `StudyView.tsx` header links the study session (`/c/$session`) so it stays reachable.

## Deviations and notes
- **Title durability.** The title model re-titles only on user-typed input (`title::due`/`user_chars` skip `<…>` envelopes) and is told to keep an accurate title; the study session's inputs are schedule envelopes, child results and the plugin's. A user typing into the study session itself could in principle re-title it and the two rules would stop applying to it. `PinTitle` is in-memory only and `binding::on_fired` (the fire path) is 140-F4E's territory tonight, so no pin was added — hand-off: pin or re-rename at `on_fired` once F4E lands.
- **Drift repair has no scope exemption** for junk (`dirty_paths` has no spec); a user who scopes `*.pyc` gets them committed and counted, but untracked `.pyc` files are still never "drift".
- **Wave regime rule** uses the last promoted keep's own Δ, not the combined move of the wave.
- **Brief kept-row fixture values** (`+2.4 %` for a kept row of a lower-is-better fixture) are hand-set test data, left as they were.
- **Deterministic metrics** (σ = 0) never open a regime segment.
- **`store::now()` is millisecond RFC 3339** (`…T01:02:03.456Z`, was whole seconds). Segments cut the sample series at `opened_at` by string order (`samples` has no segment column) and a regime segment opens at the keep's first sample; at whole seconds the old baseline's samples shared the cut in the actor test (σ pooled 0.84 instead of 0.35) and a sub-second bench would share it live. Every reader parses RFC 3339 (`parse_from_rfc3339`, `Date.parse`); old second-resolution rows sort after new rows within their own second only.
- Not done: a `samples.segment` column (the exact cut) — hand-off if the timestamp cut ever bites again.

## Gates (mbx 1.22.0, stable 1.96.1, `BUILD_LABEL=140-F4A`)
- `mbx test -p autoresearch -p roster` — autoresearch **159 passed, 0 failed** (156 + `a_regime_change_opens_a_segment_at_the_keep`, `metric_line_reads_the_raw_change_with_the_direction_word`, `keeps_never_commit_build_junk_unless_scoped`); roster **69 passed** (+ `study_sessions_and_subagents_report_no_finished_row`). `/tmp/140f4a/test3.log`.
- `mbx clippy -p autoresearch -p roster --tests -- -D warnings` — clean (`/tmp/140f4a/clippy.log`).
- `cargo fmt -p autoresearch -p roster` — applied.
- Bindings regenerated (`mbx test -p rpc --features ts --lib export_bindings`, 8 passed): `StudyDetail.final_holdout`, `AutoresearchGateConfig.regime_sigma`.
- `bun run check` — 0 errors. `bunx vitest run src/components/autoresearch src/components/sidebar src/lib/session-tree` — **41 passed** in 4 files (`/tmp/140f4a/vitest2.log`).
- `mbx check -p theuth --tests` — clean (`/tmp/140f4a/check-theuth.log`).
- Not run: a browser/dev-server pass over the Study view (the paints are `renderToStaticMarkup` in vitest); the live engine proof of a regime change (needs a scratch engine; the actor test drives the same path with the Fake backend).
