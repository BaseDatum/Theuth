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
- **E2** Compactor, title and vision-aux calls to a provider ON THIS MACHINE
  (a runtime preset or a loopback host — R2-5; not a LAN gateway in front of
  cloud models) over the OpenAI protocol send
  `extra_body {"chat_template_kwargs": {"enable_thinking": false}}`, merged last,
  and never a reasoning-effort keyword. Other side calls (consult, advisor,
  notebook insights, ask, polish, escalate) and main chat turns are unchanged
  (R2-6). Ollama's native adapter keeps `think: false`.
- **E3** The compactor's reply keeps the stop reason and whether reasoning was
  seen. A local single-unit cap is 2048 tokens (cloud 600). An empty reply with
  reasoning seen or `finish_reason=length` is the error
  "<model> used its whole <N>-token reply on thinking (finish_reason=<r>)"; a
  local provider retries that once at twice the cap, clamped to the model's
  output limit and to what the window leaves beside the prompt (R2-4); when the
  cap cannot grow the error comes at once.
- **E4** A provider may be added without a key when it is local. Its model
  calls still go through genai, with the placeholder key `local`
  (`Authorization: Bearer local`, ignored by servers that check no key); its
  listing, limit and probe requests send no header. A local provider may take a runtime's own name
  (`ollama`, `omlx`, `lmstudio`, `llamacpp`, `vllm`) and then shadows genai's
  adapter for `<name>/<model>` ids. A provider is local when it was added with
  a runtime preset or its base URL host is loopback / private / `*.local` /
  `*.lan` (137F E6's host rule).
- **E5** Add and refresh read SERVED context windows from the server (≤ 2 s,
  what arrived is kept, failures ignored, redirects not followed) and store
  them with `limit.source: "server"`; a user value is never overwritten. Never a
  model's maximum (R2-2): vLLM `max_model_len`, llama.cpp `n_ctx`, LM Studio
  `loaded_context_length` (a loaded model only), Ollama `num_ctx` from
  `/api/show` `parameters`, else `/api/ps` `context_length` of a loaded model,
  else unknown. The genai listing itself is bounded at 10 s (R2-7).
- **E6** `set_model_limits` writes the user `models.json` atomically, reloads
  the catalog and re-resolves the endless compactors. Every catalog reload
  (add, refresh, remove, limits) reaches running chats: their model takes the
  new entry at its next call, so the UI need not say "applies to new chats".
- **E7** An unknown local context is assumed to be 16,384 tokens for MEMORY and
  SUMMARY budgets only: the endless compactor's context and targets,
  `test_summary_call`, and the endless view allowance (`tokens::memory_window`,
  `models::Limit::assumed`, never written). It never refuses a request and
  never drives chat auto-compaction: a chat turn on such a model is sent as
  before 177 (auto-compaction off, `max_tokens` omitted), and the session logs
  once "<model>'s context window is unknown; set it under Settings ›
  Connections › Local servers (or /provider limits) so theuth can size
  requests". Only a known window (bundled, server- or user-set) refuses.
  Providers on this machine (a runtime preset or a loopback host, R2-5) never
  inherit a cloud sibling's window; a LAN gateway keeps sibling windows.
- **E8** `local_servers_probe` probes 127.0.0.1 only, follows no redirect,
  never adds (`THEUTH_LOCAL_PROBE_PORTS` replaces the port list for a scratch engine).
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
  The 3-argument call of today still works. A keyless local provider's model
  calls (turns, streaming, tools, compactor, titles, side calls) go through
  genai with the placeholder key `local`, so they DO send
  `Authorization: Bearer local` (genai's OpenAI adapter needs a key); only the
  raw listing, limit and probe requests send no `Authorization` header.
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
  "omlx" → omlx; else openai. A server that answers 401/403 on its model list
  is `reachable: true`, `models: 0`, `error: "answered 401: needs an API key"`.
- `test_summary_call`: failures are in the reply (`ok: false`, `error`,
  `hint`), never an RPC error.
- `refresh_provider_models` on genai's built-in `ollama`/`omlx` (no provider of
  that name) errors "<name> is built in; add it under Local servers to list its models".

### Storage

- `providers.yaml`: `<name>: {base_url, runtime?, owns_key?}`; a local row loads
  with no vault key (keyless). `owns_key: false` marks a keyless add: its
  remove (or a later keyless add) never deletes the vault secret of the same
  name, which may be the user's own (genai's `omlx` key). Rows without the
  field (pre-177) own their key.
- user `models.json`: a provider block may carry `"local": true`; a model's
  `limit` may carry `"source": "server" | "user"` (absent = unknown). Files
  without these fields load as before.
