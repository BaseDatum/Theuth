# 116 VENDOR — as built (2026-10-06, branch `feat/116-closest-vendor` on eaf1b8bd)

## The ask (Sky, 2026-10-06)

A litellm gateway model `zai-org/GLM-5.3-Flash` got a 400: `max_tokens 943717` with a 105 663-token prompt in a 1 048 576 window. For a gateway id the catalog has never seen, `Catalog::resolve` borrows the limits of `Catalog::closest()`; eight bundled entries tie at full name-match score for `[GLM, 5, 3, Flash]` and the old tie-break `(score, context, Reverse(pid))` picked the largest context — `openrouter/z-ai/glm-5.3-flash` {1 048 576, 943 717} over the vendor's `zai/glm-5.3-flash` {1 000 000, 131 072}. Sky: **"for B, the vendor provider on ties"**. (Piece A — clamping `max_tokens` to the window headroom in `crates/agent` — is the sibling's `116-max-tokens-headroom.md`.)

## D1–D3 as built

| | Decision | Where |
|---|---|---|
| D1 | `VENDORS: &[(&str, &str)]` family-token → vendor provider id, next to `ROUTABLE`; `vendor_of(family) -> Option<&'static str>` is case-insensitive | `crates/models/src/lib.rs:284-313` |
| D2 | `closest()` tie-break is `(score, is_vendor, context, Reverse(pid))`, `is_vendor = vendor_of(family) == Some(pid)`; exact `provider/model` hit still returns first; doc comment says "Ties go to the family's vendor provider (116), then the larger context, then the lower provider id." | `lib.rs:394-438` |
| D3 | Nothing else: `tokens()`, `resolve()`, `merge()`, `load()` (sibling fill inherits the order through `closest()`), `ROUTABLE`, `data/models.json`, `crates/agent` untouched | — |

### The table

| family token | vendor | | family token | vendor |
|---|---|---|---|---|
| claude | anthropic | | kimi | moonshotai |
| gpt, o1, o3, o4 | openai | | grok | xai |
| gemini, gemma | google | | qwen, qwen2, qwen3 | alibaba |
| glm | zai | | minimax | minimax |
| deepseek | deepseek | | command | cohere |

Families with no bundled vendor (llama, nemotron, mistral, lfm2, …) have no row and keep the larger-context-then-lower-pid rule.

## Tests (`crates/models/src/lib.rs` tests module)

| test | proves |
|---|---|
| `closest_prefers_vendor_on_tie_in_bundled_catalog` (`:692-721`) | bundled: `closest("litellm/zai-org/GLM-5.3-Flash")` is zai's `glm-5.3-flash` {1_000_000, 131_072}, output ≠ 943_717; `closest("gateway/claude-sonnet-4-5")` is in `anthropic`; `closest("gateway/gpt-5")` is in `openai`; `resolve("litellm/zai-org/GLM-5.3-Flash", …)` → output 131_072, `protocol`/`canonical` None. Provider found by pointer identity of the returned `&Model` against the catalog's own entry (`Model` has no `PartialEq`). |
| `closest_vendor_bit_is_below_score_and_absent_without_row` (`:723-748`) | fixture: anthropic `claude-sonnet-4` (vendor, score 5) loses to openrouter `anthropic/claude-sonnet-4-5` (score 8) → score dominates; `llama` (no row) picks the larger context (nebius 200k over meta/groq 100k) and, on equal context, the lower pid (`groq` over `meta`); `vendor_of("GLM") == Some("zai")`, `vendor_of("llama") == None`. |
| `vendors_table_matches_bundled_catalog` (`:750-765`) | every `VENDORS` row names a bundled provider with ≥1 model whose first `tokens()` token equals the family (case-insensitive) — a `just models` refresh that drops a provider fails here. |
| `gateway_models_inherit_from_models_dev_sibling` (`:595-621`, unchanged) | still passes; it only asserts the sibling id ends with `glm-5.3-flash` and the limits match, so no expectation changed. |
| `closest_matches_cursor_slugs_against_bundled_catalog`, `bundled_zen_protocols_and_sibling_strip` (unchanged) | still pass under the new order. |

## Gates (run in this worktree, `CARGO_TARGET_DIR=$HOME/Develop/theuth-target-116`, toolchain 1.98.1)

```
cargo test -p models --no-fail-fast 2>&1 | tail -20
```
```
     Running unittests src/bin/trim.rs (/Users/Sky.Moore/Develop/theuth-target-116/debug/deps/trim-645a826d7f91d4e2)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/ort_load_failure.rs (/Users/Sky.Moore/Develop/theuth-target-116/debug/deps/ort_load_failure-e19c538049317b24)

running 1 test
test a_failed_load_is_final_for_the_process ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

   Doc-tests models

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
`cargo test -p models --lib` (the lib target, scrolled off by `tail -20` above):
```
test tests::closest_vendor_bit_is_below_score_and_absent_without_row ... ok
test tests::vendors_table_matches_bundled_catalog ... ok
test tests::closest_prefers_vendor_on_tie_in_bundled_catalog ... ok
test tests::gateway_models_inherit_from_models_dev_sibling ... ok
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```
```
cargo clippy -p models --all-targets -- -D warnings 2>&1 | tail -5
```
```
    Checking half v2.7.1
    Checking ort v2.0.0-rc.13
    Checking platform v2.9.0 (/Users/Sky.Moore/Develop/theuth-116b/crates/platform)
    Checking models v2.9.0 (/Users/Sky.Moore/Develop/theuth-116b/crates/models)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.58s
```
```
cargo fmt -p models -- --check && echo FMT-OK
```
```
FMT-OK
```
```
cargo check -p agent 2>&1 | tail -3
```
```
    Checking knowledge v2.9.0 (/Users/Sky.Moore/Develop/theuth-116b/crates/knowledge)
    Checking agent v2.9.0 (/Users/Sky.Moore/Develop/theuth-116b/crates/agent)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 00s
```

## Deviations from the brief

- **`("codex", "openai")` row dropped.** No bundled openai model has `codex` as its first `tokens()` token (openai's are `gpt-5.3-codex`, `gpt-5.3-codex-spark` → family `gpt`), so the row would fail the brief's own table guard. The `gpt` row already covers them.
- **Second commit `style(models)` touches `onnx.rs` and `tests/ort_load_failure.rs`.** Both were unformatted on main; `cargo fmt -p models -- --check` cannot print `FMT-OK` with them in place. rustfmt-only, kept out of the `fix(models)` commit. Drop the commit if the orchestrator prefers the drift left alone (the fmt gate then fails on those two files, not on `lib.rs`).
- Bundled `claude-sonnet-4-5` test asserts the hit's *provider* is `anthropic` rather than pointer-equality with `anthropic/claude-sonnet-4-5`: anthropic also carries `claude-sonnet-4-5-20250929`, which ties on every key (the date token is noise) and wins the `max_by_key` last-element rule — pre-existing, and either entry is "anthropic's".

## Not verified

- `crates/agent` tests (`cargo test -p agent`) — brief says the orchestrator gates the agent crate on the merged tree; only `cargo check -p agent` was run here.
- The live litellm request (no gateway in this worktree); the proof is `resolve(...)` returning output 131_072 in the bundled-catalog test.
