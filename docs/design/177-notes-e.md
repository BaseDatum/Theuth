# 177 — worker E notes (engine, RPC, TUI, Signal, Telegram)

Branch `fix/177-e`, base `b540592e`. Design: `177-local-runtimes.md`.

## Decisions (ambiguities resolved by the brief's "simpler option" rule)

1. **`LimitSource` lives in the models crate** (`models::LimitSource`, re-exported
   as `agent::LimitSource`), so the stored `limit.source` and the wire enum are
   one type. The models crate gained an optional `ts` feature (enabled by
   `agent/ts`) for the binding.
2. **`local` host rule = 137F E6's** (`local::private_url`, moved out of
   `compactor::is_local`, which now calls it). It is a superset of the brief's
   list: also `*.localhost`, `*.internal`, `*.home.arpa`, `*.ts.net` and
   single-label hosts. One rule for compactor locality and `ProviderInfo.local`.
3. **Keyless gateway auth — deviation forced by genai (Theuth, 17:09).** Every
   model call (turns, streaming, tools, compactor, titles, side calls) goes
   through genai. Its OpenAI adapter requires a key unless the adapter opts out
   (`allow_no_api_key`, only omlx does), so a keyless local gateway's calls carry
   the placeholder key `local` (`backend::KEYLESS_PLACEHOLDER`,
   `Authorization: Bearer local`), which servers that check no key ignore. This
   overrides the brief's "send NO Authorization header" for model calls; the
   Phase A `RequestOverride` to `{base}chat/completions` is gone. Raw HTTP is
   used only for the listing extras (`local::server_contexts`) and the probe,
   which send no header; the genai model listing uses `AuthData::None` (no header).
4. **Keyless re-add deletes a stored key** of the same name, so the vault copy
   (which `Auth::gateway` prefers) does not keep being sent.
5. **A cloud `providers.yaml` row without a vault key is still dropped at load**
   (as before); a local row loads keyless.
6. **`ProviderModel.id`** is the id within the provider; `set_model_limits`
   takes the same id. `provider_models` of a custom provider reads the user
   `models.json` (what the listing and the user stored, no sibling fill); of a
   catalog provider, the catalog. A stored source wins; an entry with limits
   but no source reports `user` (custom: a hand edit, like Sky's possible
   workaround) or `server` (bundled catalog).
7. **Legacy hand edits are user values.** A listed model whose limit has no
   `source` but a non-zero context is never overwritten by a server-reported
   window (E5); only `source: server` or unknown-and-0 entries are.
8. **Window field precedence** in a `/v1/models` entry: `loaded_context_length`,
   `n_ctx`, `max_model_len`, `context_length`, `context_window`,
   `max_context_length` (served window before the model's maximum).
   `/v1/models` fields are read for every custom provider; LM Studio
   `/api/v0/models`, llama.cpp `/props` and Ollama `/api/show` only for local ones.
9. **Probe identification is by response shape, not port** (so injected ports
   work). omlx is recognised by `owned_by` containing "omlx"; I could not verify
   what omlx 0.7.3 reports — an omlx that does not say so shows as `openai`. A
   closed default port reports the runtime usually found there.
10. **`ModelInfo.local`** is exact on `list_models` rows (gateway locality or
    a local catalog block); other `ModelInfo` replies (`set_model`, …) use the
    provider-name rule (`compactor::local_name`), since `From<&Resolved>` has no
    `Auth`.
11. **Ollama as a local provider** goes through its OpenAI-compatible `/v1/`
    (base URL `http://127.0.0.1:11434/v1/`), so it gets the E2 kwargs like the
    others; genai's native Ollama adapter (`ollama/<m>` with no provider of that
    name) keeps `think: false`.
12. **Out-of-scope crates touched mechanically:** `crates/roster` and
    `crates/slack` build `ModelInfo` literals; each got `local: false` so the
    workspace compiles. No behaviour change there.
13. **E2 "aux" = the vision fallback's aux calls** (`CallKind::Vision`); titles
    and side calls go through `Side::call`; the chat-compaction summariser is
    not a short call and is unchanged. Which calls get the kwargs is
    `Backend::local_openai`: a local gateway of kind OpenAI, or genai's own
    omlx adapter (no gateway); a cloud gateway never.
14. **E3 forced retry** doubles the cap; thinking is already off as far as the
    transport allows (kwargs on a local OpenAI-protocol server, `think:false` on
    genai's Ollama adapter). It does not add kwargs elsewhere: a priceless cloud
    gateway counts as local for 137F E6 and must not get unknown body fields.
    The retry happens inside the same attempt, so `test_summary_call` (one
    try) still shows it. The thinking error carries the panel hint in its own
    text (the panel prints `pending_error` verbatim, ≤ 300 chars) and counts as
    hard via `pump::hard_failure` ("reply on thinking").
15. **E3 batches unchanged:** the 2048 local cap is for a single unit; a batch
    keeps `600 + 600 × (lines − 1)`.
16. **E6 running chats:** cheap, so done. `Agents::reload_catalog` (add,
    refresh, remove, `set_model_limits`) sends every live native session
    `CatalogChanged`: the session's model takes its new catalog entry (next
    call) and its next turn reads the new catalog. The role models (title,
    polish, subagent, vision, escalate) and every endless session's model take
    their new entries too, and the compactor key includes the limits, so a
    limits change rebuilds the compactor.
17. **E7 lives in the catalog:** `Catalog::load_with(dir, local)` marks local
    gateways (and blocks with `"local": true`), sets their unknown windows to
    16,384 in memory (never written), skips sibling fill for them, and keeps
    them out of `closest` (they are nobody's sibling; an unlisted id of a local
    provider, or of genai's `ollama`/`omlx`, borrows no cloud window).
    `tokens::window` and the turn's `window_of` fall back to 16,384 for a local
    runtime's id with no entry. So `list_models` shows `context: 16384` for such
    a model; `provider_models` shows the stored 0 plus `assumed_context`.
18. **E7 consequence, seen in the live proof:** theuth's own turn prompt (system
    prompt + 11 tools) is ≈ 31k tokens, over the 11.7k ceiling of an assumed
    16k window, so a chat turn on a never-configured local model is refused
    locally before anything is sent. The refusal names the fix:
    "the local window is assumed; set the real one under Local servers or
    `/provider limits <p> <m> <context>`". Summary, title and side calls fit.
    I kept the brief's "16,384 everywhere, preflight included"; the risk is
    listed in the DONE message.
19. **E9 accounting:** `test_summary_call` reserves and settles one
    `model_calls` row (kind `endless`) under a fixed cost owner
    `0177e9e9-0000-7000-8000-000000000177` (`SUMMARY_TEST_OWNER`, registered with `ensure_owner`): no
    session owns a test. It writes no tree rows.
20. **Probe ports for a scratch engine:** `THEUTH_LOCAL_PROBE_PORTS=18101,18102`
    (comma list) replaces the default port list; the host stays 127.0.0.1.
21. **Tests use wiremock on random loopback ports** (as every existing test
    does); only the live proof uses the 18000-18999 ports I picked.
22. **Probe identification is literal:** fake (a) reports `max_model_len`, so
    the probe calls it `vllm` (the brief's rule), although I added it with
    the `omlx` preset.

## Live proof (scratch engine, `target/debug/t` at a0fdc9cd)
Setup: `env -i HOME=/tmp/177-e/home PATH=/usr/bin:/bin THEUTH_LOCAL_PROBE_PORTS=18101,18102,18109 THEUTH_RPC__LISTEN=127.0.0.1:18200 t --headless --data-dir /tmp/177-e/data`
(`secrets.key_store: file`, `agent.model`/`title_model`/`endless.model: fakea/lfm-bare`).
Fakes `/tmp/177-e/fakes.py`: (a) :18101 omlx-like (no key, 422 on `max_tokens: 0`,
`reasoning_content` to the cap unless `enable_thinking == false`, `/control?ignore=1`
makes it ignore the kwarg, `lfm-tiny` reports `max_model_len` 32768, `lfm-bare` nothing);
(b) :18102 Ollama-shaped (`/api/tags`, `/api/show` → 8192). Driver `/tmp/177-e/proof.sh`
(Bun JSON-RPC client `/tmp/177-e/rpc.ts`), full output `/tmp/177-e/proof.txt`. Trimmed:
```
1 add_provider ["fakea","http://127.0.0.1:18101/v1",null,"omlx"] -> local true, runtime omlx, has_key false, models 2
  provider_models fakea -> lfm-bare 0/0 unknown assumed_context 16384; lfm-tiny 32768/0 source server
  add_provider ["ollama","http://127.0.0.1:18102/v1",null,"ollama"] -> qwen-local:1b context 8192 source server
  list_models -> fakea/lfm-bare context 16384 local true; fakea/lfm-tiny 32768; ollama/qwen-local:1b 8192
2 turn on fakea/lfm-bare (0/0): "prompt is too long: 31409 tokens > 11673 maximum (theuth preflight, not sent:
  … window 16384 assumed, reply 4096; the local window is assumed; set the real one under Local servers or
  `/provider limits fakea lfm-bare <context>`)" — the fake saw nothing
  set_model_limits ["fakea","lfm-bare",65536,null] -> 65536/0 source user; same chat again -> assistant answers;
  fake: turn max_tokens 24223 (tools 11, no kwargs), title max_tokens 63021 + {"enable_thinking": false}; no 422
  (re-run after the placeholder change: every chat request carries `Authorization: Bearer local`, see below)
3 endless session, 3 long messages -> stats calls 3, unbuilt 0, stalled none;
  fake: 3 × {max_tokens 2048, chat_template_kwargs {"enable_thinking": false}, reasoning_effort null, tools 0}
4 test_summary_call fakea/lfm-bare -> ok true, finish stop, thinking_off_sent true, line "Done: the weekly sync is now Thursday 15:00 …"
  fake ignores the kwarg -> ok false, finish length, reasoning_seen true, reasoning_tokens 4096,
  error "fakea/lfm-bare used its whole 4096-token reply on thinking (finish_reason=length). Pick a non-thinking …", hint same;
  fake: max_tokens 2048 then 4096 (the forced retry)
5 local_servers_probe (18101,18102,18109) -> vllm :18101 2 models added_as fakea; ollama :18102 1 model added_as ollama;
  openai :18109 reachable false "connection refused"
6 set_model_limits lfm-tiny 12345/2048 -> source user; refresh keeps it; clear (null,null) -> unknown, assumed 16384;
  refresh -> 32768 source server again
```
Re-run after the placeholder change (fresh data dir, same driver, `/tmp/177-e/proof2.txt`; the fake now logs the
header value): steps 1–6 give the same results; every chat request (turn with 11 tools, title, 3 compactor calls,
both `test_summary_call` attempts) carries `"authorization": "Bearer local"`; the listing `GET /v1/models` carries none.
Every process started for the proof was stopped.

## genai fork follow-ups (not changed here)

- Omlx adapter (`omlx/adapter_impl.rs:65-75`) maps any `reasoning_effort`,
  `Zero` included, to `enable_thinking: true`. It should map `Zero` to
  `enable_thinking: false` and drop `reasoning_effort`.
- The OpenAI adapter could accept `AuthData::None` for custom endpoints
  (`allow_no_api_key` on the public `ServiceTarget` path), which would let a
  keyless provider send no header instead of the placeholder in decision 3.
