# As-built — GENAI — wave 90 — thought-signature provenance

Branch `feat/90-thought-origin` on https://github.com/skymoore/rust-genai, based on fork main `e85c58ab`, pushed. Head: `3cec43286492f2bbfa9b50c7df70d23e38d08cde`. Not merged.

Commits (oldest first):
- `4b6e1e7` feat(chat): ThoughtSignature carries its issuing ThoughtOrigin; adapters replay only their own
- `05dd68e` test: provenance coverage for Anthropic, OpenAI Responses, Gemini and Gemini IX; ModelName serde as plain string
- `da9135a` test(stream): OpenAI Responses streamer tags encrypted_content with origin; examples/yakbak on typed ThoughtSignature
- `3cec432` docs: changelog for thought-signature provenance

## What changed, per file

**Type / options**
- `src/chat/content_part/thought_signature.rs` (new): `ThoughtOrigin { adapter_kind, model_name, connection: Option<String> }` (+ `ThoughtOrigin::new(&ModelIden, Option<&str>)`), `ThoughtSignature { signature, origin: Option<ThoughtOrigin> }` (+ `new`, `with_origin`, `readable_by(kind, connection)` = `origin.is_some_and(|o| o.adapter_kind == kind && o.connection.as_deref() == connection)`). Custom `Deserialize`: `deserialize_any` visitor accepting a bare string (→ `origin: None`) or the object. Tests: legacy string + object + legacy `ChatMessage` row, round trip (with and without `connection`), `readable_by` truth table.
- `src/chat/content_part/mod.rs`: module + re-export (reaches `crate::chat` via the existing `pub use content_part::*`).
- `src/chat/content_part/common.rs`: `ContentPart::ThoughtSignature(ThoughtSignature)` with `#[from]` (so `From<ThoughtSignature> for ContentPart`; no `From<String>`); `as_thought_signature() -> Option<&str>` unchanged in contract, new `as_thought_signature_part() -> Option<&ThoughtSignature>`, `into_thought_signature()` returns the raw string, `size()` uses `signature.len()`.
- `src/chat/message_content.rs`: `thought_signature_parts() -> Vec<&ThoughtSignature>`; `thought_signatures() -> Vec<&str>` unchanged.
- `src/chat/chat_message.rs`: `assistant_tool_calls_with_thoughts(Vec<ToolCall>, Vec<ThoughtSignature>)`. Deleted the two tests that pinned `ToolCall.thought_signatures` as a replay source (`from_tool_calls_does_not_duplicate_openai_reasoning_items`, `from_tool_calls_keeps_gemini_signatures_with_their_calls`); `from_tool_calls_keeps_signatures_on_their_calls` (field is preserved on the message) kept.
- `src/chat/chat_options.rs`: `ChatOptions.thought_connection: Option<String>`, `with_thought_connection(impl Into<String>)`, `ChatOptionsSet::thought_connection() -> Option<&str>` (chat overrides client, same as `extra_headers`). Documented as the brief specifies.
- `src/common/model_name.rs`: `Serialize` for `ModelName` hand-written as a plain string; `Deserialize` now reads an owned `String` (was `&str`, which fails from `serde_json::Value`/readers). See Deviations.

**Streaming plumbing**
- `src/adapter/inter_stream.rs`: `InterStreamThoughtBlock.signature: ThoughtSignature`, `InterStreamEnd.captured_thought_signatures: Option<Vec<ThoughtSignature>>`.
- `src/adapter/adapters/support.rs`: `StreamerOptions.thought_origin: ThoughtOrigin` computed in `StreamerOptions::new` from `model_iden` + `options_set.thought_connection()`; `StreamerCapturedData.thought_signatures: Option<Vec<ThoughtSignature>>`.
- `src/chat/chat_stream.rs`: `From<InterStreamEnd> for StreamEnd` unchanged in logic (typed values flow through `ContentPart::ThoughtSignature(..)`; the `ToolCall.thought_signatures` mirror is still filled from the raw strings). Test fixture updated.

**Anthropic** (`src/adapter/adapters/anthropic/`)
- `adapter_shared.rs`: `into_anthropic_request_parts(chat_req, &ModelIden, &ChatOptionsSet)` (was `(chat_req, Option<CacheControl>)`); callers in `anthropic/adapter_shared.rs`, `vertex/adapter_impl.rs`, `opencode_go/adapter_impl.rs` updated — so Vertex-Claude and OpenCode-Go-Claude filter on *their own* `AdapterKind`, matching what their captures are stamped with. Assistant branch: the two-list zip + mirror fallback replaced by an ordered walk with a local `Pending { None, Readable(sig), Dropped }`: a readable `ThoughtSignature` is held until the next `ReasoningContent` → `thinking` block (pushed first, before text/tool_use); a non-readable one sets `Dropped` so its `ReasoningContent` is swallowed; `ReasoningContent` with nothing pending, or a readable signature with no reasoning, counts as `unpaired` → `warn` (today's warn, now with `unpaired` count); foreign/untagged drops → `debug` with count. `Custom` parts in user/assistant/tool roles forwarded only when `custom.adapter_kind()` is `None` or equals this request's adapter kind. `build_chat_response(model_iden, web_response, thought_connection)` stamps `ThoughtOrigin` on every `thinking` block's signature.
- `adapter_impl.rs`: passes `options_set.thought_connection()` to `build_chat_response`.
- `streamer.rs`: `ThinkingBlock::into_thought_block(&ThoughtOrigin)` tags the block signature with `self.options.thought_origin`. New test `captured_thinking_signature_carries_origin_and_connection` drives a handcrafted SSE thinking+text stream through `AnthropicStreamer` → `StreamEnd` and asserts `origin.adapter_kind == Anthropic`, `model_name == "claude-x"`, `connection == Some("work")`.
- `adapter_shared_tests.rs`: fixtures tagged via `own_signature(..)`; `build_chat_response` callers pass `None`; the unpaired test rewritten for positional semantics; new `assistant_openai_responses_reasoning_is_not_replayed_as_thinking`, `assistant_anthropic_signature_from_other_connection_is_dropped` (also: unlabelled capture vs labelled request), `assistant_untagged_signature_is_dropped`, `assistant_same_origin_thinking_replays_unchanged` (string-equal body), `foreign_custom_part_is_not_forwarded` (OpenRouter-tagged dropped; Anthropic-tagged and untagged forwarded).

**OpenAI Responses** (`src/adapter/adapters/openai_resp/`)
- `adapter_impl.rs`: `to_chat_response` now captures `output[].type == "reasoning"` → `encrypted_content` as a tagged `ThoughtSignature` part (it captured nothing before). `into_openai_request_parts(.., thought_connection)` emits a `{"type":"reasoning","encrypted_content":..,"summary":[]}` item only for `readable_by(model_iden.adapter_kind, thought_connection)`; the `ToolCall.thought_signatures` pre-pass is removed; drops logged at `debug`. Summary rules unchanged (`summary: []`; `ReasoningContent` text is not sent, as before). Tests `anthropic_signature_is_not_sent_as_encrypted_content` (foreign kind, other connection, untagged) and `same_origin_encrypted_content_round_trips` (non-stream capture → request).
- `streamer.rs`: both capture sites (`output_item.done`, `response.completed` fallback) tag with `self.options.thought_origin`. New test `captured_encrypted_content_carries_origin_and_connection` (SSE fixture → `StreamEnd`).

**OpenAI chat / OpenRouter** (`src/adapter/adapters/openai/adapter_shared.rs`): `reasoning_details` `Custom` parts echoed only when `custom.adapter_kind()` is `None` or equals this request's `model_iden.adapter_kind` (OpenRouter captures are stamped `AdapterKind::OpenRouter` by the response path). Plain `ReasoningContent` → `reasoning_content` echo unchanged. Capture side (`openai/adapter_impl.rs:108-125`) already tagged; untouched.

**Gemini** (`src/adapter/adapters/gemini/`)
- `adapter_impl.rs`: `to_chat_response` stamps every `ThoughtSignature` part (five push sites via one `tag` closure); the per-call `thought_signatures` mirror is still set on the response (informational). `into_gemini_request_parts(model_iden, chat_req, thought_connection)`: user/assistant/tool roles send `thoughtSignature` only for readable ones; the assistant mirror fallback (`tool_call.thought_signatures.first()`) is removed, so an unreadable or absent signature falls straight to the existing Gemini-3 `skip_thought_signature_validator` branch; drops at `debug`. Tests: fixtures tagged; `a_mirrored_signature_on_the_call_is_honoured` replaced by `foreign_signature_not_sent_as_thought_signature` (foreign kind / other connection / untagged, mirror present → validator stand-in, neither string on the wire); new `same_origin_thought_signature_round_trips` (captured under `acct`, replayed with `acct` → `sig-a`; replayed with `None` → stand-in).
- `streamer.rs`: capture tagged with `self.options.thought_origin`.

**Gemini Interactions** (`src/adapter/adapters/gemini_ix/`)
- `ix_types/ix_step.rs`: `into_content_parts(&mut reasoning, &ThoughtOrigin)` tags `thought` step signatures.
- `adapter_impl.rs`: `to_chat_response` passes `ThoughtOrigin::new(&model_iden, options_set.thought_connection())`; the first-call mirror is still set from the raw strings. `into_ix_input_steps(.., thought_connection)`: `thought` steps only for readable signatures; the `tool_call.thought_signatures` loop and the `emitted_signatures` dedup set are removed; drops at `debug`; the `SKIP_THOUGHT_SIGNATURE_VALIDATOR` stand-in logic unchanged.
- `streamer.rs`: capture tagged; the first-call mirror now maps `sig.signature` out of the typed vector.
- `adapter_impl_tests.rs`: fixtures tagged; `test_gemini_ix_tool_call_signature_emits_its_thought_step` (mirror-only replay) replaced by `test_gemini_ix_foreign_or_mirrored_signature_is_not_replayed`.

**Bedrock**: untouched. `converse.rs` non-stream reads only `reasoningText.text` (no signature capture); `streamer.rs` emits `ThoughtSignatureChunk` string events but never captures into `InterStreamEnd`; request path (`assistant_content_to_converse_blocks`) ignores `ThoughtSignature`/`ReasoningContent`. It neither captured nor replayed, so nothing to filter — left as is per the brief.

**Examples / integration tests**: `examples/c11-openai-resp-reasoning-roundtrip.rs` now carries `Vec<ThoughtSignature>` taken from `captured_content.thought_signature_parts()` (so the round trip stays tagged); `examples/c21-tooluse-streaming.rs` type annotation; `tests/tests_yakbak_anthropic.rs` matches on `value.signature`.

**CHANGELOG.md**: entries under `v0.7.0-beta.x` — API change (`ContentPart::ThoughtSignature` payload, accessors, `assistant_tool_calls_with_thoughts`, `ModelName` serde), `with_thought_connection`, the replay rule, mirror no longer replayed, positional pairing, OpenAI Responses non-stream capture.

## The replay rule as implemented

A `ThoughtSignature` part is sent iff `sig.readable_by(model_iden.adapter_kind, options_set.thought_connection())`, i.e. `origin.is_some_and(|o| o.adapter_kind == kind && o.connection.as_deref() == connection)`. Both `None` connections are equal; one side `None` is not; untagged → never. `model_name` is recorded, not filtered on. Applied in Anthropic (incl. Vertex-Claude and OpenCode-Go-Claude, each against its own kind), OpenAI Responses, Gemini, Gemini IX. On Anthropic a dropped signature takes its following `ReasoningContent` with it; on the other adapters `ReasoningContent` was never sent as text, which is unchanged. Drops are logged at `debug` with a count, never the token. `Custom` parts with a `model_iden` are forwarded only by the matching adapter kind (Anthropic all roles; OpenAI-compatible `reasoning_details`); untagged `Custom` parts are forwarded as before. `ToolCall.thought_signatures` is still populated by response paths and `From<InterStreamEnd>` but is read by no request builder.

Anthropic pairing (decision 3, relaxed — see Deviations): walking the message in order, a readable signature is "pending" until the next `ReasoningContent`, which it signs; a second signature before that makes the first unpaired; `ReasoningContent` with nothing pending is unpaired. `[Sig, R, Sig, R, Text, ToolCall]` (streaming order) and `[Sig, ToolCall, R]` (`assistant_tool_calls_with_thoughts(..).with_reasoning_content(..)`) both yield the same `thinking` blocks as before.

## How the origin reaches `StreamEnd`

Option "change the inter types": `StreamerOptions::new(model_iden, options_set)` computes `thought_origin: ThoughtOrigin` once per stream (every streamer already holds `StreamerOptions`). Each capturing streamer (Anthropic `ThinkingBlock::into_thought_block(&origin)`, OpenAI Responses two capture sites, Gemini, Gemini IX) stores `ThoughtSignature::new(s).with_origin(origin.clone())` into `StreamerCapturedData.thought_signatures: Option<Vec<ThoughtSignature>>` / `InterStreamThoughtBlock.signature: ThoughtSignature`, so `InterStreamEnd` carries typed values and `From<InterStreamEnd> for StreamEnd` builds the parts untouched. Streamers that never capture (Bedrock, Cohere, Ollama, OpenAI chat) needed no change. The public `ChatStreamEvent::ThoughtSignatureChunk(StreamChunk)` string event is unchanged.

## `redacted_thinking` (check only)

Not captured anywhere. `grep -i redacted src/` → no matches. Anthropic non-stream `build_chat_response` matches `"text" | "thinking" | "tool_use"`, so a `redacted_thinking` block falls into the `other_typ` arm and becomes a `ContentPart::Custom` tagged with the Anthropic `ModelIden` (and is therefore forwarded back by the Anthropic builder, and now dropped by every other adapter). The Anthropic streamer handles `content_block_start` of type `thinking`/`tool_use` only; a `redacted_thinking` block start falls through to the text path (no `data` field is captured for it) — i.e. streamed redacted blocks are silently lost. Not built.

## Test counts

- `cargo test --lib`: 365 → 376 (+3 serde/readable_by, +5 Anthropic adapter, +1 Anthropic streamer, +2 OpenAI Responses adapter, +1 OpenAI Responses streamer, +2 Gemini (one replaces the mirror test), Gemini IX 1 replaced; −2 `chat_message` mirror-replay tests).
- `cargo test --test 'tests_yakbak_*'`: 38 passed, 18 ignored (record targets), 0 failed.
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo doc --no-deps`: no warnings on new items (6 pre-existing "URL is not a hyperlink" warnings in bedrock/vertex module docs, untouched).
- `rustfmt --check` on every touched `.rs` file: clean (formatted touched files; each diff hunk was mine).

## Deviations

1. **Anthropic pairing relaxed from "immediately follows" to "next `ReasoningContent` before another signature".** The brief's own fixture (`adapter_shared_tests.rs:188-230`, `assistant_tool_calls_with_thoughts(..).with_reasoning_content(..)` → `[Sig, ToolCall, Reasoning]`) is required to produce the same body, which strict adjacency cannot. Only `ThoughtSignature`/`ReasoningContent` parts advance the pairing state, so wire order from `From<InterStreamEnd>` and the hand-built form both pair correctly.
2. **`ModelName` serde fixed.** `ThoughtOrigin.model_name: ModelName` exposed that `ModelName`'s derived `Serialize` emitted `{"Shared":"…"}`/`{"Static":"…"}` and its `Deserialize` demanded a borrowed `&str`, so the type never round-tripped through JSON (this also affected `CustomPart.model_iden`). Now a plain string both ways, owned on read. Old `{"Shared":..}` rows did not load before either; no regression. Listed in the CHANGELOG as an API change.
3. **Custom-part filter generalised to "this request's adapter kind"** rather than the literal `Some(AdapterKind::Anthropic)`, because `into_anthropic_request_parts` is shared with Vertex-Claude and OpenCode-Go-Claude whose captures are stamped with their own kinds. For the Anthropic adapter this is exactly `None | Some(Anthropic)`.
4. **Bedrock replay**: nothing to apply (never replayed signatures); reported above.
5. Out of scope, noted as asked: Gemini 3 current-turn signature requirement / `skip_thought_signature_validator` dummy unchanged; `thinking-binding-controls` header not added; `redacted_thinking` capture not built.

DONE GENAI
