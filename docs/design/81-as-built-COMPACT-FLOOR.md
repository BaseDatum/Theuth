# 81 — as built: COMPACT-FLOOR

Branch `fix/compact-floor` off `95f6496a`. Brief:
`/tmp/mc-2026-10-02/omp-compact-floor.md`. Compaction must always be able to
get back under its own trigger: on 2026-09-28 a live-meeting session
(`at_tokens: 24_000`, ~7k system prompt) compacted on 65 of 83 turns, folding
one message each time, never shrinking, busting the prompt cache every call.
Cause (agent half): 70 COMPACT-SIZE made the kept tail an absolute 40k, so on
a 17k headroom `keep_from` kept everything but one message.

## Commits

1. `a8b4c3ef fix(agent): size compaction from the headroom; hysteresis and floor clamp`
2. `ea0c1d13 test(agent): 2026-09-28 meeting shape over 300 turns, floor clamp e2e, override clamp`
3. `0b67e537 docs: 07 §8.1/§8.2 headroom rule, hysteresis, floor; budgets are ceilings`
4. this note.

## What changed

### `crates/agent/src/compact.rs` (167–253: new pure helpers; tests 647–781)

- `MIN_HEADROOM = 8_000`, `SUMMARY_TOKENS = 6_000`, `TAIL_SHARE = 35`,
  `HEAD_SHARE = 25`, `REGROWTH_SHARE = 30` — named, documented constants.
- `headroom(limit, fixed) = limit − fixed`; `share(n, pct)`.
- `FoldBudget { avail, tail, head }` and `fold_budget(limit, fixed,
  head_ceiling, &CompactionConfig)`: `head = min(ceiling, 25 % avail)`,
  `tail = min(tail_tokens, 35 % avail, target_tokens − head)`. Invariant
  `head + tail ≤ 60 % avail` (stated in the doc comment, proved in
  `fold_budget_is_a_share_of_the_headroom` and the 300-turn sim).
- `clamp_trigger(at_tokens, fixed) -> (limit, clamped)`: raises an override
  to `fixed + MIN_HEADROOM`.
- `may_fire(projected, limit, last_after, avail)`: trigger plus hysteresis.
- `below_floor(limit, fixed)`: `avail < MIN_HEADROOM`.
- `inputs_moved((limit0, fixed0), limit, fixed)`: either moved by more than
  10 % of `limit0` — the re-enable test.
- `keep_from` unchanged; it is now called with the headroom-relative budget.

### `crates/agent/src/loop_.rs`

- `SessionState` 474–482 / 525–527: `last_after: Option<u32>`,
  `compaction_off: Option<(u32, u32)>`, `clamp_warned: bool`.
- `threshold()` 2611–2629: now `&mut self`; an override goes through
  `compact::clamp_trigger` against `fixed_tokens()`, `warn!` once per session
  (`clamp_warned`). Doc comment rewritten (the tail is sized from the
  headroom over it, the tool-batch budget from a quarter of it).
- `fixed_tokens()` 2631–2651 (new): provider-unit estimate of system prompt +
  tool definitions — `used − calibrated(Σ est(history[..since]) + est(head))`;
  before any usage (`used == 0`) the `est` of baseline + tools
  (`system_tokens()`). Units: everything in `compact()` and the trigger is
  provider tokens; `keep_from`'s `est` budget is produced by dividing the tail
  budget by the calibration ratio at that single boundary (comment at 4454).
- Auto trigger 3639–3694: `fixed`/`avail` computed; `compaction_off` cleared
  when `compact::inputs_moved`; fires only when `!off && may_fire(...)`;
  when `below_floor` → `compaction_off = Some((limit, fixed))`, one `warn!`
  ("compaction budget cannot be met; auto-compaction off for this session")
  and one `self.note("context_pressure", …)` naming the cause (override vs
  model window, `fixed`, `avail`, `MIN_HEADROOM`). The "compaction is near"
  pressure note is suppressed while off.
- `compact()` 4454–4477: `limit = threshold(window).unwrap_or(window | u32::MAX
  when the window is unknown)`, `fixed = fixed_tokens()`, `head_ceiling =
  image_tokens` (snap) or `SUMMARY_TOKENS`, `fold = compact::fold_budget(...)`;
  tail budget `fold.tail / ratio` (est units) with the uncalibrated message
  cap as before.
- Snap arm 4526–4540: `snap::budget_lines(lines, fold.head, …)`; `frame_tokens
  > fold.head` → summary, `warn!("snap head over cap; compacting as summary")`
  with `head_cap`/`avail`.
- 4742–4750: `state.last_after = Some(after)`; the `compaction` info line
  carries `fixed, avail, tail_budget, head_cap`; an Auto fold that freed under
  30 % of `avail` logs `warn!(before, after, fixed, avail, tail_budget)`.

### `crates/agent/src/lib.rs` 2139–2156

`CompactionConfig` doc comments: the three values are CEILINGS; the effective
tail/head budgets are 35 %/25 % of the headroom, `target_tokens` the absolute
cap on head + tail.

### Docs

- `docs/design/07-context.md` §8.1 (trigger pseudo-code gains `limit`,
  `fixed`, `avail`, hysteresis, floor; a paragraph per mechanism) and §8.2
  (budget table in headroom form, invariant, snap-over-cap, history of the
  rule naming the 2026-09-28 regression and the sim test).
- `docs/design/70-as-built-COMPACT-SIZE.md`: "Regression found 2026-10-02".
- `crates/theuth/harness.md:56`: `compaction:` comment says ceilings + shares.

## Behaviours and their tests

| behaviour | test |
|---|---|
| (a) 2026-09-28 shape, 300 turns: no two consecutive compactions, every fold leaves ≥ 40 % of `avail`, snap always degrades (4,500 > `head_cap` 4,250), count ≤ bound + 1 (bound = 300 / ⌈0.4·17000 / 700⌉ = 30) | `compact::tests::meeting_shape_never_loops` |
| (b) `keep_from` folds more than one message when the budget is under the history | `compact::tests::keep_from_folds_more_than_one_when_over_budget` |
| (c) headroom under `MIN_HEADROOM` → off once, one `<context_pressure>` note saying compaction cannot help, no `Compacting`, next over-trigger turn silent | `loop_::compact_floor_tests::floor_turns_auto_compaction_off_once` (real `Session` actor, fake provider, new `openai/tiny` 9k-window model in the Harness catalog) |
| (d) override clamp in `threshold()`: 4k → `fixed + 8k`, `clamp_warned` set, 40k passes through | `loop_::tests::context_edit_is_idle_only_and_installs_committed_frame` (extended) |
| budgets as headroom shares, ceilings, `target_tokens` cap, zero headroom | `compact::tests::fold_budget_is_a_share_of_the_headroom` |
| clamp / floor / hysteresis / re-enable arithmetic | `compact::tests::trigger_clamp_floor_and_hysteresis` |
| auto-compaction through the actor still fires once the frame projects past the clamped override | `compact_queue_tests::auto_compaction_mid_turn_commits_and_the_queue_follows` (input grown to 36k chars) |

## Gates (this worktree, agent crate only)

```
cargo test -p agent
test result: ok. 355 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 24.55s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

cargo clippy -p agent --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 15s   (exit 0)

cargo fmt --all -- --check
exit 1 — PRE-EXISTING at base 95f6496a: the same 50 files fail there
(sandbox, mesh, schedule, tui, …; in agent: acp_child.rs, sensor/mod.rs,
store*.rs, supervisor.rs, loop_.rs:3893, vision_fallback_tests.rs:172).
Verified by `git stash` → check → `git stash pop`: the diff set is identical
before and after this branch. Every hunk this branch added is rustfmt-clean
(`rustfmt --check` on the touched files reports only the two pre-existing
hunks in loop_.rs:3893 and vision_fallback_tests.rs:172, both outside my seams
and left untouched).
```

## Deviations

- **Floor condition**: the brief's `fixed + head_cap ≥ limit` is degenerate
  once `head_cap ≤ 25 % of avail` (it reduces to `avail == 0`). Implemented
  as `avail < MIN_HEADROOM` (`compact::below_floor`), which includes the
  brief's condition and uses the same constant as the clamp: under 8k of
  headroom the head + tail share is 4.8k and a fold cannot keep the current
  exchange. With the clamp in place an override can never be below the
  floor; only a window-derived trigger can (small model, large system prompt).
- **Re-enable rule**: "either differs by more than 10 %" is measured as 10 %
  of the trigger at disable time (`inputs_moved`), not 10 % of each input —
  a `fixed` of 30 tokens moving to 0 is not a change in the trigger's terms.
- **`fixed` before any provider usage**: `system_tokens()` (`est` of the
  rebased baseline + tool JSON), which is what `Turn` already knows; no new
  reach into `Current`.
- **Test fixture edits outside `compact.rs`/`loop_.rs`**:
  `loop_/vision_fallback_tests.rs:309–320` adds the `openai/tiny` model to
  the fake catalog; `loop_/compact_queue_tests.rs:171–185` grows the auto
  scenario's input past the clamped override. Both are test fixtures in the
  agent crate, named by the brief as the ones to reuse.
- `threshold()` became `&mut self` (the once-per-session clamp warning
  flag); its three callers (trigger, `compact()`, tool-batch budget) already
  held `&mut self`.
- `compact()` now calls `self.window(&current.model)` itself to derive
  `limit`; with no window and no override the ceilings apply unchanged
  (`limit = u32::MAX`), so `/compact` on an unknown-window model behaves as
  before.

## Open follow-ups

- The pre-existing `cargo fmt --all -- --check` failure at base (50 files)
  — a toolchain/rustfmt drift, not this branch's; whoever owns the base
  should run `cargo fmt --all` once.
- `fixed` steps from the uncalibrated `est` to the calibrated provider-derived
  figure at the first usage of a process (≈ the calibration ratio, 1.3–2×).
  A session disabled on its very first round could be re-checked once after
  that step and emit a second note; bounded to one extra note, not seen in
  tests.
- `SUMMARY_TOKENS = 6_000` is an allowance, not a cap on the summariser's
  output; a summary over `head_cap` eats into the 40 % growth margin. If
  logs show `compaction freed under 30 %` with `style=summary`, cap the
  summariser's `max_tokens` from `head_cap`.
- The meeting half (per-session `at_tokens` default, hints) is the
  MEETING-HEAD sibling's.
