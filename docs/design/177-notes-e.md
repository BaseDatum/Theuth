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
17. **E7 (as changed by R1-1): the assumed window is a memory hint.**
    `Catalog::load_with(dir, local)` marks local gateways (and blocks with
    `"local": true`), skips sibling fill for them, keeps them out of `closest`
    (an unlisted id of a local provider, or of genai's `ollama`/`omlx`, borrows
    no cloud window), and gives their unknown windows
    `limit.assumed = 16,384` (`#[serde(skip)]`, never written; `context` stays 0).
    `tokens::memory_window` (known window, else `assumed`, else 16,384 for a
    local runtime's id with no entry) sizes `tokens::budget(Kind::Compactor)`
    only — the endless compactor's context and target (so `test_summary_call`
    too). The turn's view allowance (`Kind::Turn`, R1 19:20), `tokens::window` (preflight, `ceiling`) and the turn's
    `window_of` (auto-compaction, reply budget) use known windows only, so a chat
    turn on an unknown local window is sent as before 177: auto-compaction off,
    `max_tokens` omitted. `list_models` shows `context: 0`; `provider_models`
    shows 0 plus `assumed_context: 16384`.
18. **The unknown-window notice is a log line**: there is no user-facing notice
    event (only the `tracing` line "context window unknown; auto-compaction
    off"), so for a local model the turn logs once per session (a
    `Shared.window_noted` flag), at warn: "<model>'s context window is unknown;
    set it under Settings › Connections › Local servers (or /provider limits)
    so theuth can size requests". A cloud model keeps the old per-turn info line.
    Before R1-1 the assumed window also refused turns (theuth's own turn prompt is
    ≈ 31k tokens); that regression is gone.
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
23. **`local::private_url` vectors (UI review, 18:02), mirrored by the desktop
    UI** (`local::tests::private_url_vectors`). The host is parsed by the
    `url` crate (lower-cased; userinfo and port ignored); the scheme is not checked.

    | URL | local |
    |---|---|
    | `HTTP://LOCALHOST:8080/V1/` | true (single label) |
    | `http://u:p@127.0.0.1:8080/v1/` | true |
    | `http://[::]:8000/v1/`, `http://0.0.0.0:8000/v1/` | true (unspecified) |
    | `http://[::ffff:7f00:1]:8000/v1/` | true (v4-mapped loopback) |
    | `http://[::1]:8000/v1/`, `http://127.0.0.1:8000/v1/` | true |
    | `10.1.2.3`, `172.16.0.1`, `172.31.255.255`, `192.168.1.5` | true (RFC 1918) |
    | `172.32.0.1` | false |
    | `100.64.0.1`, `100.127.255.255` | true (CGNAT 100.64/10) |
    | `100.128.0.1` | false |
    | `169.254.1.1`, `[fe80::1]` | true (link-local) |
    | `[fd00::1]` | true (ULA) |
    | `[2001:db8::1]`, `8.8.8.8` | false |
    | `gpu-box` (single label) | true |
    | `studio.local:1234`, `studio.LAN`, `foo.localhost`, `foo.internal`, `nas.home.arpa`, `mac.tail1234.ts.net` | true (suffix) |
    | `api.openai.com`, `local.example.com`, `evil.local.example.com` | false |
    | `not a url` | false |
24. **Probe of a keyed server (UI review, 18:02):** a server that answers 401
    or 403 on `/v1/models`, `/api/v0/models` or `/api/tags` and lists nothing
    else is `reachable: true`, `models: 0`, `error: "answered 401: needs an API
    key"` (or 403), with the port's usual runtime (omlx on :8000).

## Engine review R2 (18:34)
- **R2-1 redirects:** the probe client and the `server_contexts` client use
  `redirect::Policy::none()`; the probe keeps `no_proxy` (loopback only). The
  window reader keeps the system proxy, because it also reads cloud custom
  providers. A 302 is just an answer: the probe row is reachable with no models.
- **R2-2 served windows:** see the E5 line of the design doc. `max_context_length`
  is no longer read anywhere, and Ollama's `model_info.<arch>.context_length` is
  never stored. Each source writes as it arrives, and a lower rank wins whatever
  the arrival order (`/v1/models` < LM Studio < llama.cpp `/props` < Ollama).
- **R2-3 key provenance:** `providers.yaml` rows carry `owns_key` (false for a
  keyless add, true for a keyed add; absent on pre-177 rows, which always
  stored their key). A keyless add deletes the secret of its name only when it
  replaces a row that owned it; remove deletes it only when the row owned it.
  I chose this over renaming secrets to `provider.<name>`, because it needs no
  migration and Sky's `mlx` row keeps working as is. Known edge: a KEYED add
  named `omlx` overwrites a genai `omlx` secret with the provider's key (the
  user typed it for that name); its remove then deletes it. A keyless gateway
  named `omlx` whose vault already holds the user's `omlx` key sends that key
  (`Auth::gateway` prefers the vault), which is the same server's key.
- **R2-4 forced cap:** `min(2 × cap, limit.output, 0.95 × memory_window −
  estimated prompt)`. `Compactor::can_grow` (default true for the scripted
  fakes) tells `build_one` whether the retry could ask for more; if not, the
  thinking error comes at once after one call.
- **R2-5 narrow locality:** `Gateway::on_this_machine` = runtime preset or a
  loopback host (`local::loopback_url`: 127/8, `::1`, v4-mapped loopback,
  `0.0.0.0`, `::`, `localhost`, `*.localhost`). It decides the E2 kwargs
  (`Backend::local_openai`), the E7 assumed window and the skipped sibling
  fill (`load_catalog`, the listing's block `local` flag), and
  `ProviderModel.assumed_context`. `Gateway::local` (broad, 137F's host rule)
  stays for `ProviderInfo.local`, `ModelInfo.local`, the optional key, the
  runtime-name rule and the E5 extra endpoints. The 137F compactor locality
  (2048 cap, attempt bounds, queue) is unchanged. Sky's `mlx` at 127.0.0.1
  qualifies.
- **R2-6:** `Side::short(true)` marks a title. The kwargs go to the session
  title, to `SideCall`s with kind `Title` AND role `Title` (notebook note
  titles), and to the vision aux call. Not to notebook insights (kind Title,
  another role), consult, ask, polish or escalate.
- **R2-7:** the genai listing in add and refresh is bounded at 10 s
  (`Agents::LISTING_TIMEOUT`); add then keeps the existing list, refresh errors
  "listing <name>'s models: timed out after 10s".
- **R2-8:** TUI `/provider limits p m <context>` keeps the stored output (read
  through `provider_models`, as the desktop does); an explicit `0` clears it. A
  timed-out `server_contexts` keeps what it read. E9 spend: `cost_owners` has no
  label column, so the test calls stay under the fixed owner
  `0177e9e9-0000-7000-8000-000000000177` with kind `endless` and request ids
  `endless:test:<uuid>`. That request-id prefix is the recognisable label.

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
Re-run after R1-1 (`/tmp/177-e/proof3.txt`, fresh data dir): step 2's first turn on the 0/0 model is now SENT —
fake: `{"max_tokens": "absent", "tools": 11, "authorization": "Bearer local"}` → "Hello from fake A …"; the title call
likewise has no `max_tokens` (+ `enable_thinking: false`). The engine log has, once for the session:
`WARN agent::loop_: fakea/lfm-bare's context window is unknown; set it under Settings › Connections › Local servers
(or /provider limits) so theuth can size requests`. After `set_model_limits(65536)` the next turn carries
`max_tokens: 28486`. Steps 3–6 unchanged: 3 compactor calls at `max_tokens: 2048` with the kwarg, `unbuilt 0`;
`test_summary_call` ok, then the thinking error; probe and limits round trip as before.
Re-run after R2 (`/tmp/177-e/proof4.txt`, fresh data dir; fake (b)'s `/api/show` now returns
`parameters: "num_ctx 8192"` and `model_info.llama.context_length: 131072`): `ollama/qwen-local:1b` is stored
at 8192 (the served `num_ctx`, not the 131072 maximum), source server. Steps 2–6 unchanged (turn sent without
`max_tokens`, 3 compactor calls at 2048 with the kwarg, summary test ok then 2048 → 4096 on the thinking fake,
probe rows, limits round trip).
Every process started for the proof was stopped.

## genai fork follow-ups (not changed here)

- Omlx adapter (`omlx/adapter_impl.rs:65-75`) maps any `reasoning_effort`,
  `Zero` included, to `enable_thinking: true`. It should map `Zero` to
  `enable_thinking: false` and drop `reasoning_effort`.
- The OpenAI adapter could accept `AuthData::None` for custom endpoints
  (`allow_no_api_key` on the public `ServiceTarget` path), which would let a
  keyless provider send no header instead of the placeholder in decision 3.
