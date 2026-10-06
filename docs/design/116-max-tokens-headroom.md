# 116 — `max_tokens` headroom: clamp the reply budget to the remaining window, retry an overflow from the server's own numbers

Status: A and C built on `feat/116-max-tokens-headroom` (crates/agent); B on `feat/116-closest-vendor` (crates/models). 2026-10-06.

## Root cause (verified at main `eaf1b8bd`)

A litellm gateway model `zai-org/GLM-5.3-Flash` failed with HTTP 400:

    This server's maximum context length is 1048576 tokens: this request needs a 1049380-token context (105663 prompt tokens plus max_tokens 943717), which exceeds the context window this server was started for; shorten the prompt or reply.

1. `Backend::stream` (`backend.rs:230-235`) did `opts.with_max_tokens(m.limit.output)` unconditionally for every catalog-known model; the prompt size took no part. Every model's usable prompt was therefore `context − limit.output`.
2. The gateway id is unknown to the catalog, so `Catalog::resolve` borrows limits from `Catalog::closest()`, whose tie-break is the largest context → `openrouter/z-ai/glm-5.3-flash` with models.dev `output: 943717` (OpenRouter's "max completion" ≈ 90 % of 1 048 576). Usable prompt: 104 859 tokens; the prompt was 105 663.
3. The error matches `OVERFLOW_NEEDLES` ("maximum context"), so `round()` forced one compaction and retried; the auto-compaction trigger is a fraction of `window = 1 048 576`, so it never fires near 100 k, and the second overflow in the same turn surfaced as `model: Web stream error …`.
4. Catalog-wide: 374 of 1 391 models list `output ≥ 50 %` of context (141 on openrouter; nebius lists `output == context` for GLM). vLLM/litellm, OpenAI and Anthropic all enforce `prompt + max_tokens ≤ context`.

## The three pieces

**A — reply budget.** `Frame` carries `max_tokens`; the loop sets it to the catalog output limit clamped so the projected prompt plus the reply fits the window (`compact::reply_budget`). The backend only fills in the catalog limit when the frame left it unset.

**B — closest() vendor tie-break** (sibling worker, crates/models): `closest()` prefers the family's vendor provider on a score tie, so an unknown gateway GLM borrows `z-ai`'s limits rather than OpenRouter's inflated `output`.

**C — retry from the server's numbers.** When a provider's overflow body quotes window, prompt and `max_tokens` (`compact::overflow_numbers`), and the prompt alone fits, `call()` resends once with `max_tokens = window − prompt − HEADROOM_PAD` (floored) before any backoff or compaction. A prompt that does not fit on its own falls through to the existing one-compaction path.

## Design

- **D1** `Frame { pub max_tokens: Option<u32> }` (`frame.rs`); `Frame::request` applies `with_max_tokens(n)` when `Some`.
- **D2** `backend::reply_cap`: `with_max_tokens(m.limit.output)` only when the incoming `ChatOptions.max_tokens` is `None`.
- **D3** `compact::REPLY_FLOOR = 4_096`, `compact::HEADROOM_PAD = 2_048`, `compact::reply_budget(output_limit, window, projected, calibration, reasoning_floor)`: `scaled = calibrated(projected)`; `margin = scaled/4` uncalibrated else `scaled/10`; `reserve = scaled + margin + PAD`; `remaining = window − reserve`; `floor = max(REPLY_FLOOR, reasoning_floor)`; result `min(output_limit, max(remaining, floor))`. `window == 0` → `output_limit`; `output_limit == 0` → `max(remaining, floor)` when the window is known, else 0 (caller leaves `max_tokens = None`).
- **D4** Main call: `Turn::reply_budget(current, transport, projected)` at the `Frame` for `self.call(...)`, after the trigger block, `projected = est_frame()` (post-compaction). `reasoning_floor` = `Current::reply_floor(transport)`: on `Transport::Anthropic` with an effort, genai's `thinking.budget_tokens` for the effort (low 1 024 / medium 8 000 / high 24 000 / `Budget(n)` n) + 1 024; 0 elsewhere. `debug!` when the clamp binds.
- **D5** Side calls (`loop_/side.rs`, `aux_call`): `reply_budget(output, window, est(message) + system/4, None, 0)` with the `Resolved`'s own limits. The compaction fold (`continuation`) prices `full` + head + system like the live frame and goes through `Turn::reply_budget`.
- **D6** `call()`: one-shot branch after reasoning-rejected and before `classify`: `!shrunk && text.is_empty() && !calls && is_overflow && overflow_numbers → Some(n) && n.max_tokens.is_some() && n.prompt + floor <= n.window` → `frame.max_tokens = Some((window − prompt) − PAD max floor)`, `warn!`, `retries += 1`, `attempt += 1; continue` (no backoff).
- **D7** Feeding the server's `prompt` into `state.calibration` from `call()`: not done (`call()` is `&self`).
- **D8** Nothing else: compaction thresholds, `window()`, needles, one-compaction-per-turn unchanged.

## Fixtures (`compact::tests::overflow_numbers_reads_each_provider_body`, bare and inside `Web stream error … Body: {"error":{"message":"litellm.BadRequestError: OpenAIException - …`)

| body | window | prompt | max_tokens |
|---|---|---|---|
| `This server's maximum context length is 1048576 tokens: this request needs a 1049380-token context (105663 prompt tokens plus max_tokens 943717), …` | 1048576 | 105663 | Some(943717) |
| `This model's maximum context length is 128000 tokens. However, you requested 130000 tokens (120000 in the messages, 10000 in the completion). …` | 128000 | 120000 | Some(10000) |
| `input length and max_tokens exceed context limit: 190000 + 64000 > 200000` | 200000 | 190000 | Some(64000) |
| `prompt is too long: 210000 tokens > 200000 maximum` | 200000 | 210000 | None (no retry) |
| anything without a recognisable window | — | — | `None` |

Incident check (`reply_budget_clamps_to_the_remaining_window`): `reply_budget(943_717, 1_048_576, 105_663, None, 0) < 943_717` and `result + 105_663·5/4 + 2_048 ≤ 1_048_576`.

## Gates

    export CARGO_TARGET_DIR=$HOME/Develop/theuth-target-116
    cargo test -p agent --no-fail-fast 2>&1 | tail -30
    cargo clippy -p agent --all-targets -- -D warnings 2>&1 | tail -5
    cargo fmt -p agent -- --check && echo FMT-OK

Output in `116-as-built-HEADROOM.md`.
