# 85 — As built: ZEN-GENAI (genai fork)

Branch `feat/85-zen-openrouter` on https://github.com/skymoore/rust-genai, pushed; head `ae57baf81eb9638bb97cfbb4e4b514556b3b4f64` (5 commits on top of `a08e55d`). Not merged to `main`. Worktree: `/Users/Sky.Moore/Develop/rust-genai-wt-85`.

## Options added (all default `None`; wire bytes unchanged unless set, and only read by `AdapterKind::OpenRouter`)
| Option | Builder | Effect (OpenRouter only) |
|---|---|---|
| `ChatOptions.openrouter_provider: Option<serde_json::Value>` | `with_openrouter_provider(Value)` | top-level `"provider": <value>` verbatim |
| `ChatOptions.openrouter_cache_control: Option<bool>` | `with_openrouter_cache_control(bool)` | `true` → top-level `"cache_control": {"type":"ephemeral"}`; `false`/unset → nothing |
| (reused) `ChatOptions.extra_headers` | `with_extra_headers(...)` | overrides the default attribution headers (client merges extra headers after adapter headers; use the exact names `HTTP-Referer` / `X-OpenRouter-Title`, `Headers` is case-sensitive) |
| (reused) `ChatOptions.reasoning_effort` | `with_reasoning_effort(..)` | see wire changes |

Also: `ChatOptionsSet::openrouter_provider()` / `openrouter_cache_control()` accessors (crate-private set type). Pass-through adapter macro gained the optional `key_env_alt: ["…"]` parameter. New `pub(crate)` helpers: `UsageCost::provider_reported(&Value)`, `chat::deserialize_cost`, `adapters::support::pick_key_env_name`. No public signature removed or changed; `ChatOptions` gained two pub fields (struct has `Default`; the fork's changelog already lists such additions under "API Minor Changes").

## Wire / behaviour changes per adapter
**Usage (all three)** — `Usage.cost` now also deserializes from a decimal string (`"0.00123400"`); Zen's `"0"` becomes `Some(Cost{amount: 0.0})` (Zen sends `"0"` when the call is not billed against balance, e.g. subscription — recorded as a provider-reported zero).

**Anthropic** (`anthropic/adapter_shared.rs`, `anthropic/streamer.rs`)
- Non-stream: top-level `cost` on the messages body → `usage.cost = Some(ProviderReported USD)`.
- Stream: `message_stop` no longer emits `End` immediately; the built `End` is held (`pending_end`) and emitted when (a) an `event: ping` whose data has `cost` arrives → cost patched in, `End` emitted, stream done; or (b) the body ends (`None` from the SSE stream). A `ping` without `cost` is still `Heartbeat` before `message_stop` and ignored after it. Zen's gateway emits the cost chunk after upstream `done` (handler.ts `pump()` → `buildCostChunk`), i.e. always after `message_stop`, which forced the hold.

**OpenAI chat** (`openai/adapter_impl.rs`, `openai/streamer.rs`) — used by OpenRouter and gateways
- Non-stream: top-level `cost` (Zen) wins over `usage.cost` (OpenRouter) when both exist.
- Stream: `[DONE]` builds `End` into `pending_end`; a later chunk with top-level `cost` (Zen `{"choices":[],"cost"}`) patches it and emits `End`; otherwise `End` is emitted at body end. A top-level `cost` before `[DONE]` is attached to the captured usage; `set_usage` keeps an already-captured cost when a later token-count snapshot has none. Empty-`choices` chunks were already tolerated (usage tail path).
- Mid-stream `{"error":{code,message,metadata{error_type,…}}}` at HTTP 200 → `Error::ChatResponse{body}` (pre-existing); its `Display` prints the whole body, so `error_type` and `message` are in the text (test added). SSE comment lines (`: OPENROUTER PROCESSING`) were already dropped by `webc/event_source_stream.rs` (test added through the real parser). Trailing usage chunk with a repeated choice/empty delta/`finish_reason`: usage captured, no duplicate content (test).
- Usage mapping already present via serde aliases: `prompt_tokens_details.cached_tokens` → `cached_tokens`, `cache_write_tokens` → `cache_creation_tokens` (0 → `None` like every counter), `completion_tokens_details.reasoning_tokens` → `reasoning_tokens`, `usage.cost` → `ProviderReported`. Test with OpenRouter's exact object added.

**OpenAI Responses** (`openai_resp/*`)
- `RespUsage.cost` and `RespResponse.cost` (serde, `deserialize_cost`); non-stream top-level `cost` wins over `usage.cost`.
- Stream: new `RespStreamEvent::Ping{cost}`; `response.completed` / `response.incomplete` set `pending_end`, emitted on cost ping or body end (same rule as above). `response.failed`/`error` unchanged.

**OpenRouter** (`openai/adapter_shared.rs::util_to_web_request_data`, keyed on `AdapterKind::OpenRouter`; `all_adapters.rs` macro call)
- Headers: `Authorization: Bearer …` + `HTTP-Referer: https://theuth.io` + `X-OpenRouter-Title: theuth`.
- Reasoning: `reasoning_effort` is NOT sent; `{"reasoning":{"effort":"low|medium|high|xhigh|max|minimal|none"}}` (`Zero` → `"none"`), `Budget(n)` → `{"reasoning":{"max_tokens":n}}` (Budget was silently dropped before). Plain `AdapterKind::OpenAI` unchanged (byte-equality test).
- `cache_control` / `provider` top-level objects from the two options. `prompt_cache_options` is never emitted to OpenRouter (cache policy is gated to `OpenAI|OpenAIResp`, verified in `openai/cache_policy.rs:37-43`; asserted in test). Pre-existing `prompt_cache_key`/`prompt_cache_retention` emission for OpenRouter is unchanged (not in scope).
- Credentials: `default_auth` tries `OPENROUTER_API_KEY` then `OPEN_ROUTER_API_KEY` (first non-empty wins); if neither is set it returns `FromEnv("OPEN_ROUTER_API_KEY")` so the not-found error names the canonical name. `DEFAULT_API_KEY_ENV_NAME` / `AdapterKind::OpenRouter.default_key_env_name()` still return `OPEN_ROUTER_API_KEY` (theuth `configured()` reads this; the sibling adds the `OPENROUTER_API_KEY` alias there). Implemented as the macro's new `key_env_alt` list + `support::pick_key_env_name`.
- Non-stream 402/429: `Error::WebModelCall{ResponseFailedStatus{status, body, headers}}` already carries everything; `Display` includes status and the raw body (code, message, `metadata.reason`), `Error::headers()` exposes `Retry-After`. No new error variant; test pins it.

**Gemini** — already sends `x-goog-api-key` header and no `?key=` (both default endpoint and a custom `https://opencode.ai/zen/v1/` endpoint). Only the stale `get_service_url` doc comment was wrong; fixed. Test added.

## Tests added (17; `cargo test --lib`: 365 pass)
- Zen cost, one per adapter per mode (6): `openai::adapter_impl::test_to_chat_response_lifts_zen_top_level_cost`, `anthropic::adapter_shared::tests::test_non_stream_lifts_zen_top_level_cost`, `openai_resp::adapter_impl::test_to_chat_response_lifts_zen_top_level_cost`, `openai::streamer::test_zen_chat_stream_cost_frame_after_done_lands_in_usage`, `anthropic::streamer::test_zen_anthropic_stream_cost_ping_after_message_stop_lands_in_usage`, `openai_resp::streamer::test_zen_responses_stream_cost_ping_after_completed_lands_in_usage`; regression `anthropic::streamer::test_anthropic_plain_ping_changes_nothing`.
- Streamer tests drive handcrafted SSE bytes through a one-shot local HTTP server + the real `EventSourceStream` (`adapters::support::test_support`), so the "End on connection close" path is exercised for real.
- OpenRouter request building (`all_adapters_tests.rs`): default headers, extra-header override direction, unified reasoning for 6 efforts, Budget → max_tokens, cache_control on/off, provider verbatim, OpenAI byte-equality regression with the new options set, env fallback order (`unsafe_code` is forbidden crate-wide, so the env is not mutated; the selection fn is tested through its lookup hook).
- OpenRouter streaming (`openai/streamer.rs`): comment lines + trailing usage chunk + no duplicate content; mid-stream error chunk → `ChatResponse` with `error_type` and message in the text. Usage object test (`adapter_shared_tests::test_into_usage_maps_openrouter_usage_object`). 402 error surface (`error.rs`). Gemini header (`gemini/adapter_impl.rs`).

## Gate results (fork, toolchain stable-aarch64-apple-darwin 1.98)
- `cargo test --lib`: 365 passed, 0 failed.
- `cargo test --test tests_yakbak_*` (offline cassette replays, 11 suites incl. anthropic 10, openai_resp 10, opencode_go 4): all pass — the End-hold does not change replayed stream outcomes.
- `cargo test` (everything): `tests_p_*` live suites fail without provider keys, same as at base (not run against providers).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo fmt -- --check`: base was NOT fmt-clean (`bedrock/sigv4.rs`, `openai/adapter_shared_tests.rs`); formatted only touched files — the only remaining diff is the untouched `bedrock/sigv4.rs`.

## Deviations / decisions
- End-of-stream timing: for Anthropic, Responses and chat streams, `End` is now emitted when the HTTP body ends (or the cost frame arrives) instead of at the terminal frame. Compliant servers close right after the terminal frame, so the difference is the connection-close latency; a non-compliant server that keeps the body open after `[DONE]`/`message_stop` would now delay `End` until it closes (previously `End` fired immediately). Required by the brief's "if the cost frame arrives after the usage event, update the captured usage before the stream ends" — Zen's cost chunk is always after the upstream close.
- Cost options were not reused from `CacheControl`: theuth already sets `with_cache_control(Ephemeral24h)` for every OpenAI-like transport (`frame.rs:223`), so reusing it would have flipped every OpenRouter request to top-level `cache_control`; a dedicated bool keeps the sibling in control per model. `extra_body` would also have covered `provider`/`cache_control` verbatim; dedicated options were kept because the brief asked for them and they are kind-gated.
- `Zero` effort on OpenRouter → `{"effort":"none"}` (brief lists low…minimal; `none` mirrors the chat keyword and is what OpenRouter documents for disabling).
- Env fallback test does not touch the real environment (crate forbids `unsafe`, and `set_var` is unsafe in edition 2024).
- Gemini: no code change needed (header already used); Zen's Gemini-format cost frame (`data: {"type":"ping","cost"}`, no `event:`) is NOT captured — out of the brief's three adapters.

## Follow-ups
- Integration: merge G to fork `main`, bump theuth's Cargo.lock pin, live smoke per design §3 (Zen `/messages`, `/responses`, Gemini, OpenRouter) recording headers/usage/`returned_nanos`; verify Zen really appends the cost frame on each format (recon read it from gateway source `handler.ts:397-398`, `provider.ts:169-180`).
- Gemini streamer: capture Zen's cost ping if theuth routes Gemini models through Zen and wants exact billing (same `pending_end` pattern).
- OpenRouter: consider dropping `prompt_cache_retention` emission for `AdapterKind::OpenRouter` (OpenAI-specific field, currently forwarded unchanged).
- Attribution header values are hard-coded to theuth (`https://theuth.io` / `theuth`) per the brief; a fork-neutral default would need a client-level option.
