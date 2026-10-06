# 116 — as built, HEADROOM (A + C, 2026-10-06)

Branch `feat/116-max-tokens-headroom` off main `eaf1b8bd`. Commits: `8e2926b0` reply_budget +
overflow_numbers, `22f6d081` field docs, `fbe57489` Frame.max_tokens + backend cap, `e3caf61d`
loop wiring + tests, plus the docs commit. Nothing pushed. Only `crates/agent` and `docs/design`
touched; `crates/models`, `Cargo.toml`, `Cargo.lock` untouched.

## What landed

| piece | where |
|---|---|
| D1 `Frame.max_tokens` | `crates/agent/src/frame.rs:189-190`; applied in `Frame::request` (`with_max_tokens` after the effort). Every `Frame { … }` literal updated (loop_.rs ×6, side.rs, frame.rs tests ×6, thinking_tests.rs ×3). |
| D2 backend respects a set budget | `backend.rs:247` `reply_cap(opts, resolved)`: `with_max_tokens(m.limit.output)` only when `opts.max_tokens.is_none()`; `Backend::stream` calls it at `backend.rs:230`. Bedrock-4096 comment kept. |
| D3 pure function | `compact.rs:359` `REPLY_FLOOR = 4_096`, `:361` `HEADROOM_PAD = 2_048`, `:369` `reply_budget(output_limit, window, projected, calibration, reasoning_floor)` with the decided semantics. |
| D6 parser | `compact.rs:398` `OverflowNumbers { window, prompt, max_tokens }`, `:413` `overflow_numbers(err)` — tolerant anchor scanner (`after` / `nums` / `num_before`); no regex. |
| D4 main call | `loop_.rs:3944-3954`: `self.reply_budget(&current, transport, self.est_frame())` → `Frame.max_tokens`. `Turn::reply_budget` at `:2671` (debug log when `budget < output`); `Turn::window_of` at `:2654` (`window()` without the log, so `&self` callers can use it). |
| D4 reasoning floor | `Current::reply_floor(transport)` at `loop_.rs:713`: Anthropic transport + effort → genai budget (Low/Minimal 1 024, Medium 8 000, High/XHigh/Max 24 000, `Budget(n)` n, from `rust-genai …/anthropic/ant_reasoning.rs:8-10,81-87`) + 1 024; `Zero`/no effort/other transports 0. `Current::effort()` at `:702` replaces the inline `effort.clone().or_else(default_effort)` in `stream_once`. |
| D5 side calls | `loop_/side.rs:91-106` (window/output from the `Resolved`, `est(message) + system/4`, calibration `None`, floor 0). `aux_call` (`loop_.rs:5547-5555`) same shape with `window_of`. Compaction fold `continuation` (`loop_.rs:5106-5116`) prices `full` + head + `system_tokens()` and goes through `Turn::reply_budget`. |
| D6 retry | `loop_.rs:3051-3080` in `call()`: after reasoning-rejected, before `classify`; `floor = REPLY_FLOOR.max(current.reply_floor(transport))`; `frame.max_tokens = Some((window − prompt).saturating_sub(HEADROOM_PAD).max(floor))`; `warn!` with session/window/prompt/sent/retry; `retries += 1`; `attempt += 1; continue`, no backoff; one shot (`shrunk`). |
| D7 | Not done: `call()` is `&self`, `state.calibration` needs `&mut`; would need a Cell/atomic on `Turn` — more than ten lines of plumbing. |
| D8 | Untouched: thresholds, needles, `window()` semantics, one-compaction-per-turn. |

## Tests added

- `compact::tests::reply_budget_clamps_to_the_remaining_window` — unknown window; incident numbers (`< 943_717`, `result + 105_663·5/4 + 2_048 ≤ 1_048_576`); calibrated > uncalibrated; near-window → `REPLY_FLOOR`; output 0 → remaining; both unknown → 0; reasoning floor 16 384 wins; never over a known output limit.
- `compact::tests::overflow_numbers_reads_each_provider_body` — fixtures (1)–(5), bare and inside the `Web stream error … Body: {"error":{"message":"litellm.BadRequestError: OpenAIException - …` wrapper.
- `backend::tests::reply_cap_keeps_a_budget_the_frame_set` — `Some(4_000)` kept; `None` → `Some(8_192)`; no model → `None`.
- `loop_::tests::overflow_with_numbers_is_resent_with_the_servers_reply_budget` — `fake_openai` now records request bodies. First attempt: 400 with body (1); wire shows `max_tokens: 943717` (the frame's budget reached the wire). Second attempt sent with `max_tokens == 1048576 − 105663 − 2048 = 940865`, within `RETRY_BACKOFF[0]` (no backoff), `retries == 1`. Through `call()` directly, so no compaction can run.
- `loop_::tests::overflow_of_the_prompt_alone_is_left_to_compaction` — body (4) → `Err(Abort::Model(e))` with `is_overflow(e)`, one hit, `retries == 0` (round()'s path).
- Brief test (a) "main call's opts carry `max_tokens ≤ window − projected`" through `round()`: not written — `gateway_turn` uses `model: None` with an empty catalog, so `window == 0` and no clamp applies; a catalog-backed harness would be a larger fixture. The wire check above plus the pure-function incident test cover the arithmetic and the plumbing.

## Gates (run from the worktree, `CARGO_TARGET_DIR=$HOME/Develop/theuth-target-116`)

`cargo test -p agent --no-fail-fast 2>&1 | tail -30` — final lines:

    test result: ok. 386 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 45.25s
       Doc-tests agent
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

`cargo clippy -p agent --all-targets -- -D warnings 2>&1 | tail -5` — exit 0:

        Checking agent v2.9.0 (/Users/Sky.Moore/Develop/theuth-116/crates/agent)
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 05s

`cargo fmt -p agent -- --check && echo FMT-OK` → `FMT-OK`.

No pre-existing red.

## Deviations from the brief

1. **D4 projection.** The brief says `projected = self.state.used + appended`. Used `self.est_frame()` instead. Reason: `state.used` is in provider units once a `usage` was reported, while `reply_budget` applies `calibrated(projected, calibration)` — scaling `used` again would over-reserve by the calibration ratio (up to 4×, pushing the budget to the floor) or under-reserve when `< 1`. `est_frame()` is the `est`-unit measure the ratio is derived from (`loop_.rs` calibration: `prompt / est_frame()`), covers the post-compaction history and head, and includes system/tools like `used` does.
2. **D6 floor plumbing.** Neither passed into `call()` nor stored on `Frame`: `Current::reply_floor(transport)` is computed where needed from inputs `call()` already has, so `call()`'s signature is unchanged.
3. **`window_of`.** `window()` split into a `&self` core (`window_of`) plus the once-per-turn log, so `continuation`/`reply_budget` (`&self`) can read the window. Behaviour of `window()` unchanged.
4. **`output == 0` placeholders.** `reply_cap` still sends `with_max_tokens(0)` for a catalog model with `output: 0` when the frame set nothing (pre-existing behaviour, D8); with a known window the frame now sets a real budget first, so such models get `max(remaining, floor)`.
5. Intermediate commit `fbe57489` (frame.rs/backend.rs/thinking_tests.rs) does not compile alone — loop_.rs/side.rs literals land in `e3caf61d`. The branch head builds.

## Not verified

- No live call against the litellm gateway / `zai-org/GLM-5.3-Flash`; the incident body is exercised only through `fake_openai`.
- `reply_floor` arithmetic against a real Anthropic request with `thinking` enabled (that `budget + 1_024` is enough answer room is a judgment, not a measured figure).
- Behaviour on OpenAI/Anthropic wire bodies (2)–(4) only through the parser fixtures; `>` arrives literal in the JSON (not `\u003e`) is assumed.
- The compaction fold's new budget was not exercised end to end (no test drives `continuation` with a catalog window).
