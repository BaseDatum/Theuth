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
3. **Keyless gateway auth.** genai's OpenAI adapter requires a key unless the
   adapter opts out (`allow_no_api_key`, only omlx does). A keyless local
   gateway therefore uses `AuthData::RequestOverride { url: "{base}chat/completions",
   headers: {} }`: same OpenAI adapter, no `Authorization` header. Only for the
   OpenAI-chat protocol; a keyless gateway whose `models.json` entry names
   another protocol still sends `Bearer ` (empty) — no local runtime speaks those.
   Model listing with no key uses `AuthData::None` (genai omits the header there).
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

## genai fork follow-ups (not changed here)

- Omlx adapter (`omlx/adapter_impl.rs:65-75`) maps any `reasoning_effort`,
  `Zero` included, to `enable_thinking: true`. It should map `Zero` to
  `enable_thinking: false` and drop `reasoning_effort`.
- The OpenAI adapter could accept `AuthData::None` for custom endpoints
  (`allow_no_api_key` on the public `ServiceTarget` path), which would remove
  the `RequestOverride` in decision 3.
