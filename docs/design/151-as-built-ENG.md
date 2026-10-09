# 151 — as built, ENG (engine half)

Branch `feat/151-eng` (cut from `feat/151-c` = design `docs/design/151-autoresearch-setup.md` + C's contract). Everything in §5 and §6 of the design plus the brief's additions A–C. UI151 owns `crates/desktop/ui`; nothing there was touched.

## UI must regenerate

**No wire type changed.** `DraftView`, `DraftField`, `FieldState`, `DraftSummary`, `ProbeKind`, `ProbeResult`, `DraftArgs`, `StudyInfo.origin` and the five `autoresearch.draft*` methods are exactly C's §12 contract; the TS bindings on `feat/151-c` stay valid. New actor messages (`DraftRoot`, `StudySlugs`) and the new tool (`autoresearch_discard`) are engine-internal, not on the RPC wire.

Two things UI151 should know (both inside the contract):
- `DraftField.value` strings (§5 one-liners): metric `time_ms · ms · lower · keep ≥ 0.5%`, bench `` `bench.sh` · 3× · 600s each · exclusive``, budget `$20 · 120 experiments · 10 h · $1.5 per experiment`, repo `<path> · base HEAD`, window `idle window `idle.py`` (empty = start now), parallel `1` / `waves of 3`, models `defaults` or `review off`.
- `DraftField.evidence`: repo `<sha8> · clean tree` / `<sha8> · N uncommitted files`; bench/holdout `3 runs · 412 ms · ±1.6%`; check `passed in 1 m 50 s`. The budget row's evidence is always `None` (addition B, below).

## Built, file by file

- `crates/autoresearch/src/setup.rs` (C's stub → the module, ~1500 lines incl. tests)
  - Rows/leaves: `MAIN_ROWS`, `ADVANCED_ROWS`, `LEAVES` (31 of §4.3 + `include_dirty`, R1 below), `label`, `row_of` (`base → repo`, `off_limits → scope`), `present`, `row_json` (the sticky comparison), `value` (one-liners).
  - §5 `view(root, draft_id, started)` / `view_of(root, Draft, started)`: the six rules in order; repo verified through `git::is_git_repo` + `resolve_base(repo, base, true)`, dirty count from `git::dirty_paths` (made `pub(crate)`); bench/check/holdout verified from `probe.json` entries that are `ok` and whose `fingerprint == fingerprint(kind, spec, base_sha)`; `settled`, `total = 9`, `blocking` in row order (`<Label> is missing`, `<Label> needs your OK`, then the `validate_spec` message), `ready = blocking.is_empty()`, `dirty`, `probes` = fingerprint-matching entries (ok or not — the failed one is kept for the sparkline but never verifies).
  - §6.1 `validate_partial` (slug rule + absolute repo).
  - `ProbeFile {bench, check, holdout}` read/write/merge; `fingerprint` = sha256 over `{kind, repo, base_sha, warm, bench{command,timeout,repeats,exclusive,smoke}, metric_name, checks_command, holdout{command,repeats}}`.
  - §6.2 `probe(root, cfg, draft_id, what, repeats, cancel)`: `read_draft` → command present → `resolve_base(…, true)` → `worktree_remove` leftover → `worktree_add_detached` under `<root>/drafts/<id>/probe/wt` → `Cleanup` Drop guard armed (removes the worktree + `probe/` on ok, error and cancel) → `warm` once via `runner::command` (failure = `ok: false`) → bench: `runner::sample` × `repeats` (arg or `bench.repeats`, clamp 3..=10; stops at the first crash with `exit` + tail), check: `runner::command` once, holdout: `sample` with `bench.command = holdout.command` and `holdout.repeats` → `median`, `noise_pct = 100·gate::pooled_sigma/|mean|`, `at`, `duration_ms` → merged into `probe.json`. `SampleRequest { slug: draft_id, n: 0, role: Baseline, worktree: wt, evaluator: drafts_dir, runs_dir: probe/runs }`. `render_probe(r, z)` = the §6.2 texts (`smallest detectable ≈ z × noise`). `ProbeTool::call`: `refuse_study_sessions` → `live_cfg` → `probe` → `structured(text, ProbeResult)`; honours `cx.cancel` (the harness promotes long calls).
  - §6.3 `set_leaf` (the §4.3 grammar: lists on `,`/newline, bools, numbers, `lower|higher`, humantime timeouts validated, `metric.min_effect`/guards parsed with `Effect::parse`, `secondary_guards` one `name max_regression [lower|higher]` per line, empty `checks.command`/`holdout.command`/`window`/`warm`/`models.*`/`bench.smoke` = `None`), `edit` (sets, `validate_partial`, adds the row to `user_set` + `confirmed`, bumps `version`), `confirm` (refused for a missing or unknown row), `discard` (removes `drafts_dir`), `drafts(root, started_slugs)` (addition C: unreadable dirs skipped), `sticky_rows`, `still_confirmed`, `sticky_note`.
- `crates/autoresearch/src/tools.rs`: `write_draft` returns `(token, sticky_rows)` and computes `confirmed` = caller's list ∪ unchanged previously-confirmed rows − sticky rows; `check_slug` (slug `new` reserved) used by `validate_spec`; `refuse_study_sessions(session, tool)` is `pub(crate)` and names the refusing tool; addition A `DiscardArgs`, `discard_with_token`, `DiscardTool` (`autoresearch_discard`, `Risk::Mutating`), registered after `autoresearch_start`.
- `crates/autoresearch/src/types.rs`: actor messages `DraftRoot` and `StudySlugs` (plain structs like `Settings`, not wire types).
- `crates/autoresearch/src/lib.rs`: `Message<DraftRoot>` → `PathBuf` (`self.root`), `Message<StudySlugs>` → `Vec<String>` (`store.list_studies`).
- `crates/autoresearch/src/rpc_impl.rs`: the five bodies — `draft_ctx` asks `DraftRoot` + `StudySlugs` (started = slug ∈ studies); `drafts` scans through `setup::drafts`; `draft_edit`/`draft_confirm` write then return `view`; `discard_draft` → `setup::discard`. The "is not running" error propagates (api.rs already maps `drafts` to `[]`).
- `crates/autoresearch/src/git.rs`: `dirty_paths` `pub(crate)`.
- `crates/autoresearch/skill.md`: new section "Setup in the Autoresearch tab" after "The draft" (D8 content).
- `crates/app/src/study_draft.rs`: `partial` → `validate_partial` + `source_compact(slug, settled, total)` + `compact_text(view, sticky)`; final → `validate_spec` + full card; both read `setup::view` after the write (settled/version/missing/needs-OK rows) and append sticky notes; the full card's Discard = `Action([@Run(discard), @ToAssistant(…)])` with `discard = Mutation("autoresearch_discard", {draft_id, token})`; the final text says "discarded → the draft is deleted"; structured result gains `partial, settled, total, ready, sticky`.

## Deviations (closest correct thing; design text where it conflicts)

1. **Rule 4 reads `confirmed` only, not `confirmed ∪ user_set`.** With the union, the sticky rule ("a changed user-set row is removed from `confirmed`, stays in `user_set`") could never make the row read `proposed`. `draft_edit` adds the row to both lists (as the design says), so an untouched user edit still reads Confirmed; `user_set` is the stickiness memory alone.
2. **Previously confirmed rows survive a re-draft when their value is unchanged.** C's `write_draft` replaced `confirmed` with the tool's argument; a panel OK (`draft_confirm`) sends nothing to the chat (D3), so the agent cannot re-list it and the next re-draft would silently drop it. Now `confirmed` = argument ∪ `{r ∈ old.confirmed : row_json unchanged}` − sticky rows. Test `setup::tests::user_set_rows_are_sticky` covers both directions.
3. **Bare repo scripts in the probe.** A study resolves `bench.sh` through its custody copy; the probe has no custody, so `repo_scripts` names a relative token that is a file in the scratch worktree (and not shadowed by a draft script) by its worktree path before `runner::sample`/`command`. Draft scripts (`<root>/drafts/<id>/x.sh`, or `x.sh` beside spec.json) resolve through the runner exactly as in a study (`evaluator = drafts_dir`).
4. **`repeats` arg also applies to `holdout`** (`repeats.or(holdout.repeats).unwrap_or(bench.repeats)`, clamped 3..=10); the design only mentioned it for bench.
5. **Addition B — no estimate.** No cost/time estimate function exists in Rust (`grep estimat` across `crates/autoresearch`, `crates/app/src/study_draft.rs`: only the skill's prose formula and the ledger's `estimated_for_unavailable`). Inventing a model was out of scope; the budget row's `evidence` is `None`.
6. `addition A` test: `discard_with_token` (wrong token refused, dir untouched; right token removes the dir; `read_draft` — the first thing `autoresearch_start` does — then says "no draft") in `tools::tests::card_discard_needs_the_token_and_removes_the_draft`. `DiscardTool::call`/`StartTool::call` themselves need the live actor (`live_cfg`), so the tool bodies are exercised by the same plain fn, not through the registry.
7. `probe_is_refused_in_a_study_session` uses a registered wave child (recognised without an actor) and goes through `ProbeTool::call`; the refusal text is `autoresearch_probe is not available inside a study session` (`autoresearch_control` keeps its own name through the same fn).
8. The compact card has only **Open in Autoresearch** (no Discard), as §4.6 says; discarding a partial draft happens in the panel.

## Tests added

`setup::tests::{field_states_follow_the_rules, user_set_rows_are_sticky, edit_parses_every_leaf, validate_partial_accepts_a_bare_spec, probe_runs_in_a_detached_scratch_worktree_and_cleans_up, fingerprint_mismatch_drops_verified, durations_and_values_read_well}`, `tools::tests::{slug_new_is_reserved, card_discard_needs_the_token_and_removes_the_draft, probe_is_refused_in_a_study_session}`, `rpc_impl::tests::drafts_skip_started_and_discarded` (also addition C: an empty dir and a `{not json` spec.json are skipped), `study_draft::tests::partial_draft_renders_the_compact_card` (+ the Discard mutation asserted in `source_wires_presses_through_the_token`). The probe test runs the real toy-sort bench three times in a detached worktree, checks `probe/` is gone after ok and after a crash, `git status` of the user's repo unchanged, one worktree, HEAD on `main`.

## Gates

Stable 1.96.1, `BUILD_LABEL=ENG151 build-lock mbx`, `CARGO_BUILD_JOBS=4`, crate-level, foreground:

- `mbx test -p autoresearch` — **173 passed**, 0 failed (161 on C's branch + 12 new; the probe test runs the real toy-sort bench; suite ~30 s).
- `mbx test -p app --lib` — **47 passed** (46 + 1).
- `mbx test -p rpc --lib` — **56 passed**, 0 failed (C's flaky `engine::tests::second_acquire_in_one_process_is_running` passed this run).
- `mbx clippy -p autoresearch -p app -p rpc --all-targets -- -D warnings` — clean (two `collapsible_if` fixed with let chains).
- `mbx check -p theuth` — green (1 m 20 s).
- `cargo fmt -p autoresearch -p app -p rpc --check` — clean (`cargo fmt` also reflowed C's `types::tests::draft_args_flatten_the_spec_with_default_flags`, an ENG-owned file).
- TS bindings: not regenerated — no wire type changed.

## Hand-offs

- UI151: nothing to regenerate; `DraftView.fields[i].value` / `evidence` formats above; `autoresearch.draft` returns Err `no draft `<id>` (…)` after discard; `draft_edit` errors are `"<leaf>: <reason>"` (e.g. `bench.timeout: `soon`: …`), `draft_confirm` on a missing row `"<Label> is missing; nothing to confirm"`.
- **R1 (orchestrator, 15:08, for UI151's "Start with my uncommitted changes")**: `autoresearch.draft_edit {draft_id, field: "include_dirty", value: "true"}` is accepted (bool grammar `true|false|on|off|yes|no`) with the effect of `tools::mark_include_dirty`: `spec.include_dirty` flips, token and origin stay, **no row** becomes user-set or confirmed, `settled` is unchanged (it is not a row; `version` bumps like every write). The UI then calls `autoresearch.create {draft_id, start}`. Covered in `setup::tests::edit_parses_every_leaf` (grammar) and `user_set_rows_are_sticky` (on disk: token/user_set/confirmed/settled unchanged, `no` flips it back). Gates after R1: `mbx test -p autoresearch` 173 passed, clippy `-p autoresearch --all-targets -D warnings` clean, fmt clean.
- Closer (live proof §10): the card's Discard now deletes the draft; the rail row disappears on the next `autoresearch.drafts` fetch.
