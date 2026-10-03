# 90 — as built (THEUTH): recover from a rejected reasoning replay; name the connection

Branch `feat/90-thought-provenance`, worktree `theuth-wt-90`, base main 88305477, genai pin e85c58ab unchanged. Implements design doc 90 decisions 4 and 5 on the harness side; the fork half (decisions 1–3) is the sibling GENAI worker's.

Commits (not pushed):

- `170e8206` feat(agent): Frame.strip_reasoning removes replayed reasoning parts (90 §1.5)
- `448cbf40` feat(agent): compact::is_reasoning_rejected needle table (90 §1.5)
- `a744f922` feat(agent): connection_label and thought_connection seam (90 §1.4)
- `d4ff58a8` feat(agent): resend once without thinking blocks after a rejected reasoning replay (90 §1.5)

## What changed, per file

### `crates/agent/src/frame.rs`
- `Frame.strip_reasoning: bool` (`:184-188`), doc per brief A.
- `Frame::request` (`:212-218`): when set, `strip_reasoning` is applied to every message (head and history) after `strip_images`, then assistant messages left with no parts are removed (`messages.retain(...)`, comment explains why alternation holds: a reasoning-only reply carries no tool call, so no tool result depends on it). Runs before the Anthropic last-message cache mark, so the mark lands on a message that is actually sent.
- `pub fn strip_reasoning(m: &mut ChatMessage)` (`:286-309`), next to `strip_images`: rebuilds `m.content` via `into_parts()` / `from_parts()` (genai's `MessageContent` has no `retain`), dropping `ContentPart::ThoughtSignature(_)` and `ContentPart::ReasoningContent(_)` (wildcard patterns only — survives the fork's payload-type change), dropping `ContentPart::Custom(c)` where `c.typ()` starts with `reasoning`, and setting `thought_signatures = None` on every `ToolCall`. Text, tool calls, tool responses, binaries, other `Custom` parts untouched.
- Import: `MessageContent` added to the `genai::chat` use list (`:9-12`).
- Tests: `strip_reasoning_removes_every_reasoning_part` (`:645-700`) — one assistant message with all six kinds of part (signature, reasoning, `reasoning.encrypted` custom, `citation` custom, text, tool call with `thought_signatures`) plus a reasoning-only assistant reply; with `strip_reasoning: true` the request keeps only `citation` + text + tool call (signatures cleared), the tool response and the following user message, and the reasoning-only message is gone (5 → 4 messages); with `false` the serialised messages equal the input bytes. The module's only `ThoughtSignature(String)` construction is the `sig()` helper at `:640`.
- Every `Frame { … }` literal in the crate gained `strip_reasoning: false`: `frame.rs` tests ×5, `loop_.rs` (`:3802` main round, `:4952` summariser, `:5377` `aux_call`, three accounting tests), `loop_/side.rs:97`, `loop_/thinking_tests.rs` ×3.

### `crates/agent/src/compact.rs`
- `REASONING_REJECTED_NEEDLES: [&str; 6]` (`:77-90`) and `REASONING_REJECTED_PAIR` (`:93`) next to the existing needle tables.
- `pub fn is_reasoning_rejected(err: &str) -> bool` (`:359-363`), next to `is_overflow`: lower-cases, strips backticks, then any needle OR the Gemini pair (both words present).
- Test `reasoning_rejected_table` (`:900-925`): the exact Anthropic body from the design doc (wrapped in genai's `Request failed with status code '400'. Response body:` frame), the backtick-free spelling, "thinking blocks cannot be modified", "bound to a different conversation", the exact OpenAI `invalid_encrypted_content` body, a Gemini `thought_signature … invalid` line; negatives: "thinking block requires a signature field" (co-occurrence alone), "thought_signature missing on function call part", `prompt is too long`, a 429, an unrelated 422 schema error, a connection reset. All negatives are `false`.

Needle table (matched against the lower-cased, backtick-stripped error):

| provider | needle |
|---|---|
| Anthropic | `invalid signature in thinking block` (covers the body's `` Invalid `signature` in `thinking` block ``) |
| Anthropic | `thinking blocks cannot be modified` |
| Anthropic | `bound to a different conversation` |
| OpenAI Responses | `invalid_encrypted_content` |
| OpenAI Responses | `encrypted content could not be` |
| OpenAI Responses | `could not be decrypted` |
| Gemini | `thought_signature` **and** `invalid` both present |

`thinking block` + `signature` co-occurring is not a match on its own.

### `crates/agent/src/loop_.rs`
- `Session::call` (`Turn::call`, `:2906-2996`): `frame` is now `mut`; `let mut stripped = false;` (`:2917`). After `stream_once` fails with `Abort::Model(e)` and the attempt is settled `failed`, before `classify` (`:2955-2967`): if `!stripped && text.is_empty() && !calls && compact::is_reasoning_rejected(&e)` → `tracing::warn!(session, "provider rejected replayed reasoning; resending without thinking blocks: {e}")`, `frame.strip_reasoning = true`, `stripped = true`, `self.retries += 1`, `attempt += 1`, `continue` — no sleep, no backoff. Doc comment on `call` updated (`:2896-2900`).

Resend semantics:
- Exactly one stripped resend per `call` (per logical `request_id`); it is attempt N+1 of the same request, so `Reserved::open … settle` records two `model_calls` rows sharing `request_id` with distinct `call_id`s (first `failed`, second `settled` or `failed`).
- Only when nothing streamed; a rejection after text or a tool-call chunk falls through to today's path (`classify` → Deterministic → `Abort::Model`, or `Abort::Partial`).
- A second rejection takes today's path: `classify` sees `invalid_request` / a 400 → Deterministic → `Abort::Model(e)`. No third request.
- The resend counts against `RETRY_MAX_PER_TURN` via `self.retries` like any other resend; `RETRY_BACKOFF` indexing is unaffected because the stripped resend never sleeps and `attempt >= RETRY_BACKOFF.len()` is checked only on the transient path.

### `crates/agent/src/backend.rs`
- `pub(crate) fn connection_label(resolved: &Resolved, kind: AdapterKind) -> String` (`:255-260`): `resolved.provider` when `Some`, else `kind.as_lower_str()` (genai's lower-case adapter name, e.g. `openai_resp`).
- `fn thought_connection(opts: ChatOptions, _label: &str) -> ChatOptions` (`:262-265`), a no-op carrying the integration comment.
- `Backend::stream` (`:206-237`): the `Genai` arm now computes `(spec, label)` together — `label` from `target_for(...)`'s `kind` when there is a protocol target, else from `client.resolve_service_target(resolved.genai)`'s `model.adapter_kind` (same as `transport()`); after the `max_tokens` override and before `openrouter_options`: `opts = thought_connection(opts, &label);` (`:236`).
- Test `connection_label_is_provider_then_kind` (`:386-406`): Zen `opencode/claude-opus-5-5` → `opencode`; `anthropic/claude-opus-5-5` → `anthropic`; a bare `gpt-9` with no provider and `AdapterKind::OpenAIResp` → `openai_resp`.

**The one-line swap left for integration:** `crates/agent/src/backend.rs:263-265` — replace the body of `thought_connection` with `opts.with_thought_connection(label)` (and rename `_label` → `label`) once the fork's `feat/90-thought-origin` lands. Caller at `backend.rs:236` is already in place.

### `crates/agent/src/loop_/thinking_tests.rs`
- `Fake` is now `{ bodies, reject: AtomicUsize }` (`:29-32`); `respond` answers the first `reject` requests with `400` + the exact body `REJECTED` (`:35`, the design-doc body with `messages.3.content.0`), then the normal signed-thinking + `tool_use` stream. `Harness::new()` delegates to `Harness::rejecting(0)` (`:98-106`); `Harness` gained `dir`.
- `Harness::turn_call(model, history)` (`:182-245`): builds a real `Session` (in-memory `Store` with a created session row, `Auth::stub`, the harness `Client` whose resolver points every target at the wiremock) and a `Turn`, resolves `(Current, Transport)` through `Backend::transport`, and runs `Turn::call(…, CallKind::Round)`; returns the result and the `model_calls` rows `(call_id, request_id, kind, status)` by attempt — the same read the accounting tests use.
- `fn sig(s) -> ContentPart` (`:250-252`): the module's single `ThoughtSignature` construction. `foreign_thinking_history()` (`:256-266`): user, assistant (`sig` + `ReasoningContent` + `ToolCall toolu_1`), tool result.
- `rejected_thinking_signature_is_resent_stripped` (`:425-470`): `rejecting(1)`; the call succeeds with the fake's thinking captured; two bodies recorded; body 0 replays a `thinking` block, body 1 has no `thinking` block and keeps `tool_use toolu_1` and the `tool_result`; two ledger rows, distinct `call_id`, same `request_id`, statuses `failed`, `settled`.
- `second_rejection_is_a_turn_error` (`:474-490`): `rejecting(2)`; `Err(Abort::Model(e))` with the Anthropic message in `e`; exactly two bodies recorded; two rows `failed`, `failed`, same `request_id`.

## Tests and gates
- New tests (5): `frame::tests::strip_reasoning_removes_every_reasoning_part`, `compact::tests::reasoning_rejected_table`, `backend::tests::connection_label_is_provider_then_kind`, `loop_::thinking_tests::rejected_thinking_signature_is_resent_stripped`, `loop_::thinking_tests::second_rejection_is_a_turn_error`.
- `cargo test -p agent`: **374 passed, 0 failed, 0 ignored** (369 at base + 5).
- `cargo clippy -p agent --no-deps --all-targets -- -D warnings`: clean.
- `rustfmt --check` on touched files: `frame.rs`, `backend.rs`, `side.rs` clean; `compact.rs` and `thinking_tests.rs` were fmt-clean at base so whole-file `rustfmt` touched only my hunks; `loop_.rs` has the same six pre-existing drift sites before and after (none in my hunks).
- Not touched: `Cargo.lock`, `Cargo.toml` dependency lines, anything under `rust-genai*`, store schema, stored message format, UI, desktop bindings, the existing `ThoughtSignature(String)` constructions in `context.rs`, `vision.rs`, `store.rs`, `snap.rs`.

## Deviations
- "Drive a turn" in E is implemented as `Turn::call` on a real `Session`/`Turn` (the loop's retry path, ledger included), not a full `UserInput` turn: the fake's `echo` tool is not a registered tool, so a full turn would fail at dispatch rather than exercise the resend. The assertions the brief names (second body, tool_use/tool_result intact, completion, two attempts of one `request_id`) are all made against that call.
- The Gemini two-needle case is a `(&str, &str)` const checked inline in `is_reasoning_rejected` rather than a separate helper fn — one expression, same behaviour.
- The `None` (genai-resolved) arm of `Backend::stream` now calls `client.resolve_service_target` once per stream to learn the adapter kind for the label (the brief's instruction); this is a local resolver call, no network.
- Commits 1–3 do not each build in isolation (commit 1 adds the `Frame` field; the literal updates in `loop_.rs`/`thinking_tests.rs` land in commit 4). HEAD builds and passes all gates; no amend/squash per the working rules.
- `Frame::request` strips reasoning before placing the Anthropic trailing cache mark, so a reasoning-only last assistant message does not receive a mark and then vanish; this ordering is not in the brief but is the only one that keeps the mark on a sent message.

DONE THEUTH
