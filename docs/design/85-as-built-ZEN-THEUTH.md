# 85 — As built: ZEN-THEUTH (theuth side)

Branch `feat/85-zen-openrouter`, worktree `/Users/Sky.Moore/Develop/theuth-wt-85`, base `bba5b5e2` + design commit `6b0f0166`. genai stays pinned at `a08e55d0` (Cargo.lock untouched); everything below runs on that pin. Sibling note `85-as-built-ZEN-GENAI.md` is committed alongside.

## What changed

**Catalog — `crates/models`**
- `src/lib.rs:56-62` `Model.protocol: Option<Protocol>`, `Model.canonical: Option<String>` (`serde(default)`, skipped when `None`).
- `src/lib.rs:65-108` `Protocol { Anthropic, OpenaiResponses, Gemini, OpenaiChat }` (snake_case wire names `anthropic | openai_responses | gemini | openai_chat`), `Protocol::from_npm(Option<&str>)`, `Protocol::guess(slug)` (`claude-` → Anthropic; `gpt-|grok-|muse-` → OpenaiResponses; `gemini-` → Gemini; else chat).
- `src/lib.rs:110-117` `zen_override(id) -> Option<Option<Protocol>>`: `qwen3.7-max|plus` → Anthropic, `jev-*` → excluded.
- `src/lib.rs:258` `ROUTABLE` += `"opencode"`; `Catalog::adapter` already passes unknown ids through, so `opencode/x` → `opencode::x` (never reaches genai — see backend).
- `src/lib.rs:417-431` `Catalog::resolve`: a `provider/model` id that is **not** an exact catalog entry gets its sibling's limits as before but with `protocol`/`canonical` cleared (closest() is "for limits only; never for what is sent to the provider").
- `src/bin/trim.rs:42-53`: drops Zen `jev-*`, sets `protocol` from per-model `provider.npm` else the provider-level `npm` (so `anthropic/*` → anthropic, `openai/*` → openai_responses, `openrouter/*` → openai_chat), applies `zen_override` for `opencode`, copies `canonical_model_id`.
- `data/models.json` refreshed with `just models` (models.dev 2026-10-03): 116 `opencode` models — 53 openai_chat, 32 openai_responses, 23 anthropic, 8 gemini; +6722/−1540 lines.

**Dispatch — `crates/agent/src/backend.rs`**
- `:26` `ZEN_ENDPOINT = "https://opencode.ai/zen/v1/"`; `:29` `BUILTIN_HINT`.
- `:56-100` `ProtocolTarget { kind, endpoint, auth }`, `target_for(resolved, auth)` → `protocol_target(resolved, zen_key, gateway)`: `opencode/*` with a key → Zen endpoint, `AuthData::Key`, kind from the catalog protocol else `Protocol::guess(slug)`; a user gateway → its URL/key, kind `OpenAI` unless the exact catalog entry names a `protocol`; otherwise `None` (genai resolves the id).
- `:102-109` `kind_of(Protocol) -> AdapterKind` (Anthropic / OpenAIResp / Gemini / OpenAI).
- `:116-158` `openrouter_options(resolved, &OpenRouterConfig, opts)`: for `open_router::*` only — `extra_body.provider` from config, `extra_body.cache_control = {type: ephemeral}` when `canonical` starts with `anthropic/`, `extra_headers` `HTTP-Referer` / `X-OpenRouter-Title` when configured. The one place that knows genai's option names.
- `:177-191` `Backend::transport` → `Transport::of(target.kind)` for a protocol target (Zen Claude → Anthropic marks, Zen GPT → OpenAiLike, Zen Gemini → Plain); gateways therefore also follow their kind.
- `:194-227` `Backend::stream` builds the `ServiceTarget` from `target_for`, keeps the catalog `max_tokens`, applies `openrouter_options`.
- `:239-258` `provider_error`: any error text containing `ModelProtocolUnsupported` gets `BUILTIN_HINT` appended (open error and stream errors both route through it).

**Effort — `crates/agent/src/frame.rs:150-165`** `default_effort`: Medium for a `reasoning: true` catalog model that is `anthropic::*`, Bedrock's Anthropic publisher, **or** (any namespace) a `claude-*` slug or `canonical` `anthropic/*`.

**Credentials / RPC — `crates/agent/src/supervisor.rs`**
- `:6987` `Auth.openrouter: Arc<OpenRouterConfig>` (from `cfg.openrouter` at `:7357`; `Auth::stub` defaults).
- `:7014-7025` `Auth::opencode_key()` (vault `opencode` → `OPENCODE_API_KEY`), `Auth::openrouter_key()` (vault `openrouter` → `open_router` → `OPENROUTER_API_KEY` → `OPEN_ROUTER_API_KEY`); `:7040` `env_key`.
- `:7096-7098` genai auth resolver: `AdapterKind::OpenRouter` → `openrouter_key()`.
- `:7200-7207` `configured`: `"opencode"` → `opencode_key().is_some()`, `"open_router"` → `openrouter_key().is_some()`; so `ListModels` lists `opencode/*` and `openrouter/*` once a key exists.
- `:6479-6524` `ListProviders`: two built-in rows first (`opencode` @ `ZEN_ENDPOINT`, `openrouter` @ `https://openrouter.ai/api/v1/`, `builtin: true`, `configured`, catalog model counts), then gateways sorted by name (`builtin: false, configured: true`). `AddProvider` reply sets the two new fields.
- `crates/agent/src/lib.rs:1288-1301` `ProviderInfo { …, builtin: bool, configured: bool }` (`serde(default)`); `:2179` `AgentConfig.openrouter: OpenRouterConfig`; `:2231-2239` `OpenRouterConfig { provider: Option<Value>, referer, title }`.
- `crates/agent/src/providers.rs:46-51` `validate_name` refuses `opencode | openrouter | open_router | zen` with "`<name>` is a built-in provider — set the `opencode` / `openrouter` secret instead".

**UI**
- `crates/desktop/ui/src/components/settings/providers.ts` (new): `BUILTIN_LABEL`, `splitProviders`, `builtinKeyNote`.
- `Connections.tsx:131-206` `BuiltIns` section ("Built-in providers", above Gateways): Dot = configured, `opencode/…` · N models · key state, inline masked `set_secret(name, value, note)` form (the Settings → Search `SecretRow` pattern; invalidates providers/models/secrets). `Gateways` filters `!builtin`; the engine's reserved-name error surfaces unchanged through `useAction`.
- `src/rpc/extra.ts:66-75` `ProviderInfo` type; `src/rpc/bindings/ProviderInfo.ts` regenerated by `just ts-desktop`.
- TUI `crates/tui/src/model.rs:1500` `/provider` help names the built-ins and `/secrets add`; `:4875-4898` the list shows `name  built-in  N models  key set | no key — /secrets add <name>`.

**Config docs** `crates/theuth/harness.md:49`, `crates/theuth/src/main.rs:813-815` (`agent.openrouter`). Design docs: 02 §1 (protocol/canonical, opencode routable, sibling strip), 05 §2.2 (credential names), 07 §3 (transport per protocol target), 85 §5 (learnings).

## Behaviours and their tests
| Behaviour | Test |
|---|---|
| npm → protocol, prefix guess, Zen overrides, serde name | `models::tests::protocol_from_npm_guess_and_zen_overrides` |
| bundled Zen protocols (claude → anthropic, gpt → responses, gemini, qwen3.8-flash → anthropic), openrouter canonical, sibling strip for `rwx/claude-opus-5-5` | `models::tests::bundled_zen_protocols_and_sibling_strip` |
| kind per Zen protocol + guess for unknown ids, endpoint, key; no key → `None`; gateway OpenAI unless explicit protocol; `Transport::of` per kind | `backend::tests::zen_targets_follow_catalog_protocol_then_prefix_guess` |
| on the wire (wiremock): `/zen/v1/messages` + `x-api-key`; `/zen/v1/responses` + Bearer; `/zen/v1/models/gemini-3.5-flash:streamGenerateContent` + `x-goog-api-key`; `/zen/v1/chat/completions` + Bearer; body `model` = slug | `backend::tests::zen_wire_path_and_auth_header_per_protocol` |
| OpenRouter: `provider` + `cache_control` in the body for an Anthropic canonical, header overrides on the wire, nothing added for other models/providers | `backend::tests::openrouter_passthrough_lands_in_body_and_headers` |
| `ModelProtocolUnsupported` → hint appended | `backend::tests::zen_protocol_unsupported_gets_builtin_hint` |
| Medium default for `opencode/claude-opus-5-5`, `openrouter/anthropic/claude-opus-5.5`; none for `opencode/gpt-5.6-sol`, `openrouter/openai/gpt-5.6-sol` | `loop_::thinking_tests::default_effort_is_medium_for_anthropic_thinking_models_only` |
| `configured("opencode")` via vault `opencode`; `configured("open_router")` via `open_router` or `openrouter` (preferred); genai resolver hands `openrouter` key to the `open_router` adapter | `supervisor::tests::builtin_keys_configure_opencode_and_openrouter_by_either_name` (real `secrets::Vault` in a temp dir; env fallbacks asserted only as "equal to the current environment") |
| reserved gateway names and message | `providers::tests::name_and_url_rules` |
| ProviderReported cost on a Zen target → `returned_nanos`, provider `opencode` | `loop_::tests::observe_partitions_money_by_provenance` (Zen case) |
| UI split/labels/notes, `ProviderInfo` shape | `components/settings/providers.test.ts` |

## Gate results (verbatim counts)
- `cargo test -q -p models -p agent -p rpc`: agent lib **366 passed**; models **19 passed**; rpc **36 passed** (+ doc/integration binaries 0/1/0 passed, 0 failed everywhere).
- `cargo clippy -q -p models -p agent -p rpc --all-targets --no-deps -- -D warnings`: clean (no output).
- `cargo test -q -p tui`: **122 passed** (the `/provider` rendering lives there).
- rustfmt: `backend.rs` formatted whole (I own it); other hunks hand-shaped to `rustfmt --check` output; untouched base drift left alone (`cargo fmt -p models` reformatted `onnx.rs`/`ort_load_failure.rs` — reverted).
- `just ts-desktop`: bindings regenerated (`ProviderInfo.ts` has `builtin`, `configured`); the unrelated `crates/rpc/bindings/Info.ts` drift it produced (82/83 `default_effort`) was reverted, not committed.
- `bun run check` (tsc -b): clean. `CI=1 bun run test < /dev/null`: **130 files, 713 tests passed**.
- No live calls to Zen/OpenRouter (no keys).

## Live smoke plan (orchestrator, after the genai pin bump)
Set secrets `opencode` and `openrouter` (or `OPENCODE_API_KEY` / `OPENROUTER_API_KEY`), start theuth with `RUST_LOG=genai=debug` or a capturing proxy, then one short turn per id:
| `provider/model` | assert URL path | assert auth header | assert cache marks | assert `model_calls.returned_nanos` |
|---|---|---|---|---|
| `opencode/claude-opus-5-5` | `POST https://opencode.ai/zen/v1/messages` | `x-api-key: <key>` (no `Authorization`) | system block + last message `cache_control`; `thinking` present (default Medium) | `> 0`, provider `opencode` (Zen `ping` cost frame; genai fork) |
| `opencode/gpt-5.6-sol` | `…/zen/v1/responses` | `Authorization: Bearer <key>` | `prompt_cache_key` = session id | `> 0` |
| `opencode/gemini-3.5-flash` | `…/zen/v1/models/gemini-3.5-flash:streamGenerateContent?alt=sse` | `x-goog-api-key: <key>` | none | `> 0` only if the fork captures the Gemini cost ping (ZEN-GENAI follow-up); else `estimated` from catalog cost |
| `opencode/deepseek-v4-pro` | `…/zen/v1/chat/completions` | `Authorization: Bearer <key>` | `prompt_cache_key` | `> 0` |
| `openrouter/anthropic/claude-opus-5.5` | `POST https://openrouter.ai/api/v1/chat/completions` | `Authorization: Bearer <key>`, `HTTP-Referer`, `X-OpenRouter-Title` | top-level `cache_control: {type: ephemeral}`; `reasoning: {effort: medium}` (fork) | `> 0` from `usage.cost`, provider `openrouter` |
Also: `/provider add opencode …` is refused with the built-in hint; `/provider` lists the two built-ins first with "key set"; Settings → Connections shows the Built-in providers table and "Set key…" stores the secret; `/model opencode/claude-opus-5-5` resolves once the key exists (and not before).

## Deviations
- `target_for(resolved, auth)` has no `catalog` parameter (brief: `target_for(resolved, catalog, auth)`): exactness is decided in `Catalog::resolve`, which now strips `protocol`/`canonical` from a sibling-derived model, so `Resolved.model.protocol` is authoritative and the backend needs no second catalog lookup. Key/gateway resolution is split into `protocol_target(resolved, zen_key, gateway)` so the dispatch is testable without a vault.
- `Model::protocol_or_guess()` was added, then removed: the guess must run on the requested slug, not on the (possibly sibling) `Model.id`; `Protocol::guess(slug)` is called from the backend instead.
- OpenRouter passthrough uses the pinned genai's existing `ChatOptions::with_extra_body` / `with_extra_headers` (same wire bytes as the fork's `with_openrouter_provider` / `with_openrouter_cache_control`) instead of a no-op marker — it works today; the integrator may retarget `backend::openrouter_options` onto the dedicated options in one place after the bump. Header overrides are only sent when configured (defaults come from the fork).
- `trim` derives `protocol` for every ROUTABLE provider from per-model `provider.npm` **else the provider's `npm`** (brief: per-model only) so `anthropic/*`, `openai/*`, `google/*` carry a truthful protocol instead of a false `openai_chat`.
- No `Model.deprecated` field: upstream `status` (35 Zen models `deprecated`, some `beta`) is not mirrored; the models stay listed as the brief requires.
- `OpenRouterConfig` is carried on `supervisor::Auth` (`auth.openrouter`) rather than threaded through `Backend::for_model`/`Current::new` (5 call sites) — it is per-request provider settings beside the gateway map.
- Outside my seams, minimal and unavoidable: `crates/agent/src/loop_.rs` test `Model` literals gained `protocol: None, canonical: None` (6640-6641, 7111-7112) and the Zen cost case was added to `observe_partitions_money_by_provenance` (7081-7105, the only module where `observe` is reachable); `crates/agent/src/loop_/thinking_tests.rs:167-169, 182-183` extended the effort test; `crates/tui/src/model.rs:4875-4898` renders the built-in rows (the list is never empty now).
- models.dev currently has no `qwen3.7-max|plus` or `jev-*` on Zen; the override table is in place but inert (tested on the function, and `qwen3.8-flash` is asserted anthropic from upstream data).

## Open follow-ups
- After the genai bump: retarget `openrouter_options` to the dedicated options if the integrator prefers; the reasoning→`reasoning{effort}` and default attribution headers arrive with the fork.
- `AddProvider` still lists gateway models with `AdapterKind::OpenAI`; a gateway that is really Zen-shaped should use the built-in (hinted). A per-model `protocol` on a gateway is only honoured when written into `<data_dir>/models.json` by hand.
- Catalog refresh UX (85 §0.7) and `Model.deprecated` surfacing in `/model` if wanted.
- `crates/theuth` was not compiled (workspace-wide build forbidden); the `DEFAULT_CONFIG` edit is inside a YAML comment block only.
