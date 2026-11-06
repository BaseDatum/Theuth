# 177 — local model servers (LOCAL-RUNTIMES)

Sky (2026-11-06) runs a small MLX model behind omlx as the endless-memory
compactor. Two failures: every chat is a 422 (`max_tokens: 0`), and summaries
come back empty ("compactor returned nothing") because the model thinks until
the 600-token cap. Nothing in the desktop app configures omlx, LM Studio or
Ollama. This design fixes the engine and adds a "Local servers" screen.

Worker E: engine, models crate, RPC, TUI, `/provider` in Signal and Telegram.
Worker U: desktop UI (adds the §UI section below). Decisions and evidence of
worker E: `177-notes-e.md`.

## Problems (recon at main `b540592e`)

- `backend::reply_cap` sent the catalog's `limit.output` even when it is 0
  (unknown); every other request path already guarded 0.
- Listed models of a custom provider are stored as `limit {context 0, output 0}`;
  unknown context became a 200k window (`tokens::window`), so a small local
  model got cloud-sized budgets.
- Nothing turns thinking off on an OpenAI-compatible local server; the
  compactor keeps only text, so a reply spent on `reasoning_content` is empty.
- `validate_name` refuses `omlx`/`ollama` as provider names, a key is mandatory,
  and the built-ins' refresh says "models come from the built-in catalog".

## Behaviour

- **E1** `reply_cap` sends `max_tokens` only when `limit.output > 0`.
- **E2** Compactor, title and aux (side) calls to a local provider over the
  OpenAI protocol send `extra_body {"chat_template_kwargs": {"enable_thinking": false}}`,
  merged last, and never a reasoning-effort keyword. Ollama's native adapter keeps
  `think: false`. Main chat turns are unchanged.
- **E3** The compactor's reply keeps the stop reason and whether reasoning was
  seen. A local single-unit cap is 2048 tokens (cloud 600). An empty reply with
  reasoning seen or `finish_reason=length` is the error
  "<model> used its whole <N>-token reply on thinking (finish_reason=<r>)"; a
  local provider retries that once with thinking off forced and twice the cap.
- **E4** A provider may be added without a key when it is local (no
  `Authorization` header). A local provider may take a runtime's own name
  (`ollama`, `omlx`, `lmstudio`, `llamacpp`, `vllm`) and then shadows genai's
  adapter for `<name>/<model>` ids. A provider is local when it was added with
  a runtime preset or its base URL host is loopback / private / `*.local` /
  `*.lan` (137F E6's host rule).
- **E5** Add and refresh read context windows from the server (≤ 2 s, failures
  ignored) and store them with `limit.source: "server"`; a user value is never
  overwritten.
- **E6** `set_model_limits` writes the user `models.json` atomically, reloads
  the catalog and re-resolves the endless compactors.
- **E7** An unknown local context is budgeted as 16,384 tokens everywhere;
  local providers never inherit a cloud sibling's window.
- **E8** `local_servers_probe` probes 127.0.0.1 only, never adds.
- **E9** `test_summary_call` runs one real summary call on a fixed sample.

## Contract

Rust types live in `crates/agent/src/lib.rs` (re-exported by `agent`), the TS
bindings are generated (`bindings/*.ts`, `methods.json`). Names are final.

### Types

```ts
type LocalRuntime = "ollama" | "lmstudio" | "omlx" | "llamacpp" | "vllm" | "openai"
// "openai" = any other OpenAI-compatible local server

type LimitSource = "server" | "user" | "unknown"   // models::LimitSource

type ProviderInfo = {                  // extended
  name: string, base_url: string, models: number,
  builtin: boolean, configured: boolean, refreshable: boolean,
  local: boolean,                      // runtime preset, or loopback/private/*.local/*.lan host
  runtime: LocalRuntime | null,        // the preset it was added with
  has_key: boolean,                    // a key is stored; false for a keyless local provider
}

type ModelInfo = { /* existing fields */, local: boolean }   // list_models rows

type LocalServer = {
  runtime: LocalRuntime,               // what answered; for a closed default port, the port's usual runtime
  base_url: string,                    // the URL to add: http://127.0.0.1:<port>/v1/ (Ollama too)
  reachable: boolean,
  models: number,
  added_as: string | null,             // an existing provider with the same base URL
  error: string | null,
}

type ProviderModel = {
  id: string,                          // model id within the provider (picker id = `${provider}/${id}`)
  name: string,
  context: number, output: number,     // 0 = unknown
  source: LimitSource,
  assumed_context: number | null,      // 16384 when a local model's context is unknown
}

type SummaryTest = {
  ok: boolean, model: string, latency_ms: number,
  tokens_in: number | null, tokens_out: number | null, reasoning_tokens: number | null,
  finish_reason: string | null, reasoning_seen: boolean, thinking_off_sent: boolean,
  line: string | null, error: string | null, hint: string | null,
}
```

### Methods (positional params, as every theuth method)

| method | params | result |
|---|---|---|
| `add_provider` (extended) | `name: string, base_url: string, key: string \| null, runtime: LocalRuntime \| null` | `ProviderInfo` |
| `local_servers_probe` (new) | — | `LocalServer[]` |
| `provider_models` (new) | `provider: string` | `ProviderModel[]` |
| `set_model_limits` (new) | `provider: string, model: string, context: number \| null, output: number \| null` | `ProviderModel` |
| `test_summary_call` (new) | `model: string` (`provider/model`) | `SummaryTest` |

- `add_provider`: `key` and `runtime` may be omitted (trailing params). A
  missing or empty key is refused for a non-local URL ("api key is empty").
  The 3-argument call of today still works.
- `provider_models`: a custom provider's rows come from the user
  `models.json` (what the listing and the user stored); a catalog provider's
  from the catalog. An entry without a stored source but with limits reports
  `user` (custom: a hand edit) or `server` (bundled catalog).
- `set_model_limits`: `null` clears that field to 0 (unknown); both 0 →
  source `unknown`, else `user`. The model must exist in the user file or the
  catalog. Errors: `no model <provider>/<model>`, an unreadable `models.json`.
- `local_servers_probe`: ports 11434, 1234, 8000, 8080 on 127.0.0.1, in
  parallel, 300 ms connect, 1.5 s total. Identification by what answers, not by
  port: `/api/tags` → ollama; `/api/v0/models` (with `/v1/models`) → lmstudio;
  `/props` with `default_generation_settings` → llamacpp; a `/v1/models` entry
  with `max_model_len` or `owned_by` "vllm" → vllm; `owned_by` containing
  "omlx" → omlx; else openai.
- `test_summary_call`: failures are in the reply (`ok: false`, `error`,
  `hint`), never an RPC error.
- `refresh_provider_models` on genai's built-in `ollama`/`omlx` (no provider of
  that name) errors "<name> is built in; add it under Local servers to list its models".

### Storage

- `providers.yaml`: `<name>: {base_url, runtime?}`; a local row loads with no
  vault key (keyless).
- user `models.json`: a provider block may carry `"local": true`; a model's
  `limit` may carry `"source": "server" | "user"` (absent = unknown). Files
  without these fields load as before.
