# OpenCode Go as a built-in provider — notes

Sky, 2026-11-18: "we also need to add opencode go as a built in provider:
https://opencode.ai/docs/console/go/". Correction the same day (13:30): "opencode
zen pay as you go api is in the code. opencode go has different usage and a
different api endpoint. it's the /inference endpoint that's the new one."
Docs read and endpoints probed 2026-11-18 (keyless; no Go key exists).

## 1. What existed on main (7bb2acb4c), before this change

- **Catalog.** `opencode-go` is bundled (`crates/models/data/models.json`, 42 models,
  name "OpenCode Go", models.dev `env: [OPENCODE_API_KEY]`), in `ROUTABLE`, and
  `Catalog::adapter("opencode-go") == "opencode_go"`, so a pick resolves to
  `opencode_go::<id>`. Every model carries a `protocol` (27 openai_chat, 6
  openai_responses, 7 anthropic, 2 gemini). 9 ids end in `-free`.
- **Dispatch.** `backend::target_for` built a protocol target for `opencode` (Zen)
  and user gateways only; `opencode-go/*` fell through to genai's own `opencode_go`
  adapter (fork rev d2fd71e, `src/adapter/adapters/opencode_go/adapter_impl.rs`):
  - endpoint `https://opencode.ai/zen/go/v1/` (the old base);
  - protocol by name prefix only: `minimax-*` → `/messages`, everything else
    `/chat/completions`; the catalog `protocol` was ignored;
  - key: vault `opencode_go` (the resolver's `adapter_kind.as_lower_str()`), else
    env `OPENCODE_GO_API_KEY`;
  - no `User-Agent` (genai/reqwest set none) and no `x-opencode-session`.
- **What that does today.** Every Go path without `x-opencode-session` answers
  **400 `MissingSessionID`** ("Request is missing x-opencode-session and cannot be
  routed efficiently"), the old `/zen/go/v1/` base included. So with a key every
  `opencode-go/*` call was a 400: listed, unusable.
- **Settings › Providers.** Not listed: `ListProviders` returned `opencode`,
  `openrouter` and custom gateways. No row, no key form, no Refresh models;
  `refresh_provider_models("opencode-go")` answered "opencode-go's models come from
  the built-in catalog". The key could only be set as a raw secret `opencode_go`
  or the env var. A custom provider named `opencode-go` was accepted.
- **Picker.** Models under provider id `opencode-go` once `configured("opencode_go")`
  held; the picker shows raw provider ids everywhere (Zen as `opencode`).
- **Costs.** Go calls went through genai's adapter; any `usage.cost` on chat
  completions was read by genai's OpenAI usage parser, nothing else.
- **Limits.** `limits/mod.rs` row `opencode-go:api_key` ("OpenCode Go") and
  `limits/poll.rs` `Source::OpencodeGo` poll `GET https://opencode.ai/zen/go/v1/usage`
  with `key(auth, "opencode_go", "OPENCODE_GO_API_KEY")`. `opencode-go:api_key` is
  the limits row id (`provider:auth`), not the vault name.

## 2. Facts from the docs and probes (2026-11-18)

- Go's base is `https://opencode.ai/inference/go`; the API family is in the path
  (console/harnesses § Base URLs): `/openai/v1/chat/completions`,
  `/openai/v1/responses`, `/anthropic/v1/messages` (`max_tokens` required),
  `/google/v1beta/models/<model>:generateContent|:streamGenerateContent`. Every API
  serves every paid/Go text model (Console translates). Pay-as-you-go is the same
  shape under `https://opencode.ai/inference`.
- Auth `Authorization: Bearer <Console API key>` (`oc_sk_…`), on every family.
- Go requires `x-opencode-session` (a stable id per conversation) and wants the
  client's own `User-Agent`. Keyless probe with the header: each family path
  answers 401 "OpenCode Go requires an API key" (so the routes exist); without it,
  400 `MissingSessionID`. Unknown paths 404.
- Free-tier chat models (`*-free`) answer 403 to every client except OpenCode,
  key or not.
- `GET https://opencode.ai/inference/go/v1/models` is public: 200 keyless (32
  models), 401 with a bogus bearer, 200 without a `User-Agent`. Rows carry
  `capabilities.tools`, `capabilities.reasoning.effort`, `modalities`, `limit`,
  `cost`, and `api.<family>.base_url` (the per-family bases above).
- Cost (console/api/inference § Cost): every response reports `usage.cost` (USD
  charged; 0 for Go within its limits), `usage.opencode.billing_source`
  (`free|balance|go|go-plus|byok`), `usage.opencode.list_cost`,
  `usage.opencode.allowance` (Go: dollars of the plan's monthly limit used) —
  `usageMetadata` on Gemini; in streams on the final usage event (chat
  `include_usage` chunk, Responses `response.completed`, Messages
  `message_delta`, Gemini last chunk).
- Usage limits: 5-hour (20 % of monthly), weekly (50 %, Monday 00:00 UTC),
  monthly allowance; optional Extra Usage falls back to the balance.
- Usage endpoint: none documented. `GET /inference/go/v1/usage` keyless → **401**
  `{"type":"error","error":{"type":"AuthError","message":"Unauthorized"}}`;
  `GET /zen/go/v1/usage` → 401 too; `/inference/go/v1/nonsense`,
  `/zen/go/v1/nonsense` and `/inference/go/usage` → 404. Both usage routes exist;
  neither response shape can be seen without a key.
- Zen (report only, not changed): `GET https://opencode.ai/zen/v1/models` still
  answers 200 keyless (`big-pickle`, …); `GET https://opencode.ai/inference/v1/models`
  answers 200 as well.

## 3. What this change does

- **Key.** `Auth::opencode_go_key()`: vault `opencode-go` (the Settings row's name,
  as Zen's is `opencode`), then the old genai-namespace name `opencode_go`, then env
  `OPENCODE_GO_API_KEY`. Used by dispatch, `configured("opencode_go")`, the
  Providers row, and both limits lookups, so a key under either vault name works
  everywhere.
- **Dispatch** (`backend::go_target`). `opencode-go/*` with a key → a protocol
  target: kind from the catalog `protocol`, else OpenAI chat completions (no slug
  guess: Go translates, chat is the safe default); endpoint per family
  (`go_endpoint`): `…/inference/go/openai/v1/` (chat, Responses),
  `…/inference/go/anthropic/v1/`, `…/inference/go/google/v1beta/`. genai's
  `opencode_go` adapter is no longer reached with a key. Anthropic's `max_tokens`
  comes from `reply_cap` (the catalog output limit; every Anthropic-family Go
  target is a catalogued model).
- **Headers** (`go_headers`, Go only). `Authorization: Bearer <key>` on every family
  (genai's Anthropic and Gemini adapters send only `x-api-key` /
  `x-goog-api-key`; those stay too), `User-Agent: theuth/<version>`,
  `x-opencode-session: <theuth session id>` (the loop's conversation id).
- **Cost.** genai reads `usage.cost` on chat completions and Responses (both parse
  the usage object with `deserialize_cost` → `ProviderReported`), but on Anthropic
  only from Zen's `ping` frame and on Gemini not at all. `GoCost` wraps the limits
  `HeaderTap` as the call's frame sink, keeps the last `usage.cost` /
  `usageMetadata.cost` it sees, and fills the `End` event's usage cost when genai
  left it empty. `loop_::observe` then records it as the returned (billed) amount:
  Go within limits is $0, not the catalog list price. Limits attribution:
  `opencode-go:api_key`.
- **Not captured: `billing_source`, `allowance`, `list_cost`.** `CostObservation`
  and the calls ledger have no field for them (adding one is a store migration and
  an RPC change); they are dropped. The limits row's 5-hour/weekly/monthly windows
  still come only from the usage poll.
- **Usage poll.** Moved from `/zen/go/v1/usage` to `/inference/go/v1/usage` (the
  route exists on the new base: 401, not 404). Flag: undocumented; the parser
  (`parse_opencode_go`, `usage.{rolling,weekly,monthly}`) is from the old route's
  recon and unverified on the new one — a shape change surfaces as "OpenCode Go
  usage: unexpected response shape" on the limits row.
- **Settings › Providers.** Built-in row `opencode-go` ("OpenCode Go") between Zen
  and OpenRouter, base `https://opencode.ai/inference/go/`, Set key writes vault
  `opencode-go`; the model count leaves out `*-free`.
- **Refresh models.** `GET https://opencode.ai/inference/go/v1/models` anonymously
  (public; the row is refreshable with or without a key) and merges new ids like
  Zen's refresh (`Listing::Builtin`: add ids the catalog lacks, never remove). That
  path stores ids only: a new id takes a sibling's limits/reasoning/efforts and
  goes out on chat completions. The listing's `capabilities`, `modalities` and
  `api` are not mapped — the built-in refresh path has no slot for them.
- **Free models.** `*-free` Go ids are excluded from the picker (`ListModels`) and
  the row's count (`go_free`). Chosen over "mark unavailable": `ModelInfo` has no
  such flag, and a hidden id is still reachable by typing it (it then 403s).
- `opencode-go` is reserved: a custom provider may not take the name.
- **Picker.** Built-in key providers show their label (OpenCode Zen, OpenCode Go,
  OpenRouter) on the provider row and the models header; ids stay
  `opencode-go/<id>`.
- **Zen unchanged.** Endpoint, protocol choice and headers as before (no session
  header, no user agent added).

## 4. Not proven live

No OpenCode Go key exists. Not observed: a successful Go chat call on any family,
whether Go's Anthropic/Gemini paths accept the duplicated key header, the real
`usage.cost` frames, the `/inference/go/v1/usage` response shape, an authenticated
`/v1/models`. Observed: the public model listing, the per-family 401s, the 400
`MissingSessionID`, the usage-route 401s. Tests cover the wire (paths, auth,
headers against a fake server), target choice, the cost fill on a recorded-shape
Anthropic stream, the Providers row and the key lookup.
