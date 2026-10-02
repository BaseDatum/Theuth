# 80 — `search_x`: X (Twitter) posts through xAI's `x_search` tool

Status: built 2026-10-02 (§13; proposed the same day, Sky). Extends 16 (web search tools) and 05 §2.2
(provider credentials); amends 00 §10 (crate table). Where this doc and 16
disagree about the `search_x` tool, this doc wins; everything about
`search_web`/`read_url` stays 16's.

Scope: one read-only tool, `search_x`, registered by the existing
`search-providers` plugin, that asks xAI's Responses API to run its built-in
`x_search` tool over a natural-language query and returns Grok's synthesis
plus the cited posts. It is the port of jarvis's `x_search`
(`~/Develop/jarvis/crates/jarvis-agent/src/tools/x_search.rs`, 409 lines)
onto theuth's credential, registry and output contracts.

Not in v1 (and the seams left for them): a Settings › Search row (the
`ProviderSettings` wire shape is a `web` provider's — base URL, secret name,
priority — and this tool is not a provider; a row needs a second shape, §11);
streaming the answer; `web_search`/`code_interpreter` siblings (same request
shape; a second tool would share `xai.rs`); a `Provider` impl feeding the
`search_web` aggregator (§0.3 says why not).

## 0. Decisions in one screen

1. **A tool, not a provider; in `search-providers`, not a crate of its own.**
   16 §0.1 draws the line: `web` is the keyless always-on half,
   `search-providers` carries vendor schemas, credentials and billing. This
   is vendor code with a credential and a bill, so it lives there, and it
   registers straight on the tool registry the way `web` registers its own
   tools (`crates/web/src/lib.rs:193-216`). *Rejected:* a new crate — 00 §1.6,
   one tool earns no crate; putting it in `web` — a machine with no xAI
   credential would still compile and ship the vendor client.
2. **Name `search_x`.** theuth names tools verb-first (`search_web`,
   `read_url`, `scrape_url`); the model sees one family. *Rejected:* jarvis's
   `x_search` — the vendor's name for *their* server-side tool, kept only in
   the request body.
3. **Not a `Provider` for the `search_web` aggregator.** What comes back is
   a synthesised answer with citations, not a ranked hit list; squeezing it
   into `RankedResult` throws away the answer and bills a Grok call on every
   `search_web`. The aggregator never sees it. *Rejected:* a `social`
   category provider — stubs are what 16 §0.3 forbids.
4. **Credentials are the `xai` provider's, in 05 §2.2 order: vault key
   `xai` → login (`oauth::XAI` grant) → `XAI_API_KEY`.** The LLM provider
   and the search tool are the same product behind the same bearer; one
   login, one key, one precedence. The tool builds the same
   `oauth::Store::new(data_dir).with_grants(vault)` the agent does
   (`crates/agent/src/supervisor.rs:7224-7231`) and calls
   `Store::access(&oauth::XAI)` per call (refresh is single-flight, hub-only,
   67 §4.4). *Rejected:* jarvis's separate `XaiOauthStore` with OAuth-first
   precedence — a second token store for the same account, and the opposite
   order to 05; a `secret:` config key naming a different vault entry — the
   key *is* the provider key.
5. **Advertised only while a credential exists.** 16 §0.3: nothing is
   registered that returns nothing. `SearchProviders::sync` registers or
   unregisters the tool (and its prompt block) from the same predicate it
   uses for keyed providers; it already re-runs on
   `secrets::SecretsEvent::Changed` (`crates/search-providers/src/lib.rs:597-605`),
   and a login lands as vault row `oauth/xai` (`crates/secrets/src/lib.rs:708-735`)
   whose write the vault refresh loop announces as `Changed`
   (`crates/secrets/src/lib.rs:819-833`) — so `/login xai` surfaces the tool
   within one `REFRESH_POLL`, no restart. The env var is read once at
   start (the environment does not change under a running process).
   *Rejected:* always registered, failing at call time with "sign in" —
   jarvis did that and the model learned to call a tool that could not work.
6. **Risk `ReadOnly`.** It reads public posts and writes nothing anywhere;
   that it costs money is true of `search_web` through Brave or Firecrawl
   too, and cost is not what `Risk` measures (plugins.md §3.5). Default
   `tools.policy` runs it without a prompt.
7. **One config block, three keys.** `search_providers.x: { enabled, model,
   timeout_ms }`, present by default like `firecrawl` and inert without a
   credential. No `base_url`: the login token is minted for
   `https://api.x.ai/v1` and nothing else; a proxy user amends this line.
   `model` defaults to `grok-4.7` — the id xAI's tool docs use today and a
   row in `crates/models/data/models.json`.
8. **Degraded, not silently empty.** As in 16 §4.6: filters were given and
   zero citations came back → `degraded: true` with the reason, so the
   model (and a routine) can tell "nothing on X" from "the filter ate it".

## 1. Why

jarvis had this and it was useful: "what are people on X saying about …",
"what did @handle post this week", reactions to a release. The web engines
in 16 do not index X; Grok's server-side `x_search` does, and the user's
existing SuperGrok login already pays for it. The port is small because
theuth already owns the three hard parts — the xAI device-grant login (05),
a vault that holds keys and grants (37, 67), and a registry that gates and
scrubs (00 §4) — so what jarvis built as a 409-line tool with its own token
store is here a request body, a response parser and a registration predicate.

## 2. Facts relied on

- `crates/oauth/src/lib.rs:281-292` — `oauth::XAI`: device grant, Grok CLI
  public client, bearer on `https://api.x.ai/v1`; `:424-438` `Store::new`,
  `with_grants`; `:780-847` `Store::access` (refresh, single-flight, hub-only).
- `crates/agent/src/supervisor.rs:6908-6917` — `Auth::key` (vault, live) and
  `Auth::login` (no key *and* a grant) = 05 §2.2 precedence;
  `:7047-7054` the model call's xAI bearer is `AuthData::Key(access_token)`;
  `:7224-7231` the store is `oauth::Store::new(&rt.data_dir).with_grants(vault)`.
- `crates/secrets/src/lib.rs:708-735` — grants are vault rows `oauth/<ns>`;
  `:644-650` `secrets::handle(&rt)` waits for the vault; `:819-833` the
  refresh loop publishes `SecretsEvent::Changed` when the vault file moves.
- `crates/search-providers/src/lib.rs:553-568` `sync`, `:597-605` the
  `SecretsEvent` handler, `:617-651` `start` (vault handle, `lookup_when`
  for `web`, spawn, register, subscribe); `Cargo.toml` already depends on
  `secrets`, `reqwest`, `chrono`, `serde_json`, `schemars`, dev `wiremock`.
- `crates/web/src/lib.rs:193-216` — `RegisterTool`/`RegisterPrompt` after
  `lookup_when::<ToolRegistry>(REGISTRY_ACTOR)`; `crates/web/src/tools.rs:83-89`
  the `structured(text, data, is_error)` output helper, `:358-361`
  `NAME`/`RISK`/`RETURNS` consts; `crates/tools/src/registry.rs:249-252,373-381`
  `RegisterTool`, `UnregisterTool(String)`, `RegisterPrompt`,
  `UnregisterPrompt(&'static str)`.
- xAI docs (read 2026-10-02): `https://docs.x.ai/developers/tools/x-search`
  — Responses API tool `{ "type": "x_search" }` with `allowed_x_handles`
  (max 20), `excluded_x_handles` (max 20; not together with allowed),
  `from_date`/`to_date` (`YYYY-MM-DD`), `enable_image_understanding`,
  `enable_video_understanding`; `…/tools/overview` — `POST
  https://api.x.ai/v1/responses`, `model: grok-4.7`, citations returned as
  source URLs (`…/tools/citations`).
- jarvis `x_search.rs` — response parsing: `output_text` at top level, else
  `output[].type == "message"` → `content[]` of type `output_text`/`text`;
  inline `annotations[].type == "url_citation"` → `{url, title,
  start_index, end_index}`; optional top-level `citations` array; its
  `MAX_HANDLES` of 10 is stale (docs say 20).

## 3. Shape

```
crates/search-providers/src/
  lib.rs      SearchProvidersConfig gains `x: Option<XConfig>` (default Some);
              SearchProviders gains `oauth: oauth::Store`, `env_key: Option<String>`,
              `x_registered: bool`; sync() registers/unregisters the tool + prompt
  xai.rs      XConfig, XSearchTool, Credential, request/response types, validation,
              `credential_source()` predicate, tests (wiremock)
```

`XSearchTool { http: reqwest::Client, model, timeout, vault: Option<Arc<secrets::Vault>>,
oauth: oauth::Store, env_key: Option<String> }`. Per call:

1. Validate args (§4). Errors are `ToolOutput::err` the model can act on.
2. Resolve the bearer: `vault.get("xai", 0)` → `("vault", key)`; else
   `oauth.load("xai").is_some()` → `oauth.access(&oauth::XAI).await` →
   `("login", access_token)` (a refresh error is returned verbatim — it
   already says `xai: login expired; run /login xai`); else `env_key` →
   `("env", key)`; else `err("No xAI credential: /login xai or add the `xai` secret")`
   (unreachable while registration is correct; kept for a vault emptied
   between `sync` and the call).
3. `POST https://api.x.ai/v1/responses` with
   `{ model, input: [{role: user, content: query}], tools: [tool_def], store: false }`
   where `tool_def` is `{type: "x_search"}` plus only the filters given.
   `Authorization: Bearer …`, `User-Agent` = the workspace's
   (`web::http` or `WebConfig::user_agent` pattern), timeout from config.
4. Non-2xx → `err("xAI HTTP <status>: <first 500 chars>")`; 401/403 add
   ` — /login xai again or set the xai secret` (403 on a login is the
   subscription tier gate 05 §2.1 notes).
5. Parse (§5), compute `degraded`, return `structured(text, &result, false)`.

## 4. Tool

`search_x` — `Risk::ReadOnly`, `RETURNS = "dict"`, `CONSUMES_SECRETS` default.

Description (pure function of config, names the model):
"Search X (Twitter) posts, profiles and threads through xAI's live index
(<model>): what people are saying on X, reactions, claims, a handle's recent
posts. Returns Grok's synthesis with the cited posts; cite them as links.
Not for general web pages — that is search_web."

Args (`XSearchArgs`, schemars):

| field | type | rule |
|---|---|---|
| `query` | string, required | trimmed, non-empty |
| `handles` | string[]? | only these handles (`@` stripped), ≤ 20, not with `exclude_handles` |
| `exclude_handles` | string[]? | never these handles, ≤ 20 |
| `from_date` | string? | `YYYY-MM-DD` UTC, not after today |
| `to_date` | string? | `YYYY-MM-DD` UTC, ≥ `from_date` |
| `images` | bool? | `enable_image_understanding` |
| `videos` | bool? | `enable_video_understanding` |

RETURNS (structured; text = the answer, then `Sources:` one URL per line):

```json
{ "query": "…", "model": "grok-4.7", "credential": "vault|login|env",
  "answer": "…", "citations": [{"url": "…", "title": "…"}],
  "degraded": false, "reasons": [], "took_ms": 4210 }
```

`citations` is the union of the top-level `citations` URLs and the inline
`url_citation` annotations, de-duplicated by URL in first-seen order, title
from the annotation when present. `degraded` is true iff any filter was set
and `citations` is empty; `reasons` then holds one line naming the filters.

Prompt block (registered with the tool, unregistered with it, plugin name
`search-providers`): one sentence telling the model when X beats the web.

## 5. Config

```yaml
search_providers:
  x:                          # design doc 80 — search_x through xAI; inert without a credential
    enabled: true
    model: grok-4.7           # any xAI model that supports agent tools
    timeout_ms: 90000         # one Grok search can take tens of seconds
```

`enabled: false` or a missing block → no tool, no prompt block, nothing
else changes. No new vault names: the credential is the provider's `xai`
key or login (05 §2.2). No new environment variables beyond the adapter's
own `XAI_API_KEY`. Add the block, commented, to `DEFAULT_CONFIG` in
`crates/theuth/src/main.rs` and a bullet to README's config list.

## 6. Threats

| threat | answer |
|---|---|
| Prompt injection in post text | Tool output is untrusted content like every `search_web` passage; 28's guard sees it at the same seam. Nothing here executes. |
| Bearer leaks into output | The bearer never enters the result; `tools::secrets` scrubs any vault value echoed back (37 §3.4). The token from a login is not a vault *secret* row and is never echoed. |
| Billing surprise | Registered only with a credential the user chose; one Grok call per tool call; `timeout_ms` bounds it. |
| Non-hub engine with an expired grant | `Store::access` refuses to refresh off-hub and errors; the model sees the error text, same as a model call there. |

## 7. What we knowingly give up

Streaming (the answer arrives whole), image/video understanding by default
(opt-in flags; they cost more), a Settings row, raw post objects (xAI's
tool returns a synthesis plus URLs, not tweets — the citations *are* the
retrieval).

## 8. Alternatives

X's own API v2 (bearer from a developer app, `tweets/search/recent`): real
post objects, but a developer account, a paid tier and a second credential
nobody has; xAI's tool rides the login the user already made. A headless
scrape of x.com: 23's browser could, but x.com is login-walled and hostile
to automation; 00 §1 says security is never simplified away.

## 9. Amendments to 00

§10 crate table, the `search-providers/` row becomes: `plugin: optional
Firecrawl, SearXNG, OpenSERP, Brave API (`brave_api`) registered with web
over the bus; only firecrawl present by default, key-gated (16); plus the
`search_x` tool over xAI's `x_search`, registered while an `xai` credential
exists (80)`. §10 dependency line: `search-providers` now also depends on
`oauth` (a library; no cycle — `secrets` already does).

## 10. Build steps

| step | done when |
|---|---|
| 1 `xai.rs`: config, args, validation, request body, response parsing, `credential_source` | unit tests: both handle lists → error; 21 handles → error; bad/ future/inverted dates → error; wiremock 200 with annotations → answer + citations; 200 with filters and no citations → `degraded`; 401 → error naming `/login xai`; precedence vault > login > env |
| 2 wire into `lib.rs`: config block, `Store`, env read, `sync` registers/unregisters tool + prompt | tests: `sync` with no credential registers nothing; with a vault key registers `search_x`; after the key is deleted it is unregistered |
| 3 docs: this doc's as-built, 00 §10, 16 §2 cross-reference, `DEFAULT_CONFIG`, README | `grep -n search_x docs/design/00-architecture.md docs/design/16-web.md crates/theuth/src/main.rs README.md` all hit |
| 4 gates | `cargo check --workspace --tests`, `cargo clippy -p search-providers --all-targets -- -D warnings`, `cargo test -p search-providers`, `cargo fmt --check` |

## 11. Open

- Settings › Search row for `search_x` (credential state = key / login /
  env; `enabled`; `model`) needs a second wire shape or a `kind` on
  `ProviderSettings`. `persist` already writes the `x:` block back, so a
  row only needs the wire shape and `apply_settings`.
- `search_web` could learn an `x` provider *name* that routes to this tool
  when the query names a handle — only if use shows the model picking wrong.
- Siblings `web_search` / `code_interpreter` through the same request shape.
- Live smoke against `api.x.ai` (a real `xai` login or key on a dev box):
  the response shape is from xAI's docs and jarvis's parser, not observed.

## 12. Status

Built 2026-10-02 (§13). Exercised: validation, credential precedence
(pure + a file-backed `oauth::Store`), request body and response parsing
against wiremock, the 401 text, the registration predicate, `persist`
round-trip of the `x:` block. Not exercised: a live xAI call, the kameo
actor's register/unregister through a real `ToolRegistry`, `/login xai`
surfacing the tool through `SecretsEvent::Changed` on a running harness.

## 13. As built — 2026-10-02

Branch `feat/search-x` on `95f6496a`. `crates/search-providers/src/xai.rs`
(749 lines, 7 tests) and `lib.rs` (+~110 lines, +1 test, +1 extended);
`crates/search-providers/Cargo.toml` gains `oauth`, `tools`. Tests:
12 unit (7 `xai`, 5 `lib`) + 4 wiremock provider contracts. Gates run
per crate: `cargo fmt -p search-providers -- --check`,
`cargo clippy -p search-providers --all-targets -- -D warnings`,
`cargo test -p search-providers`, `cargo check -p theuth`.

Pointers: `xai.rs:36` `XConfig`; `:57` `XSearchArgs`; `:83-104`
`Credential` + `credential()` (pure precedence); `:144` `tool_def`
(validation + `x_search` definition + names of filters set); `:245`
`real_title` (blank or footnote-label titles → `None`); `:251`
`top_citation` (URL string or `{url, title?}`; anything else skipped);
`:264` `parse` (answer, citations union, title back-fill); `:315-446`
`XSearchTool` (`bearer`, `run`); `:449` `Tool` impl. `lib.rs:53` the `x`
block (default `Some`); `:477-481` `persist` writes it; `:532`
`advertise_x` (the §0.5 predicate); `:596-628` `sync_x` (register tool +
prompt / unregister both, idempotent on `x_registered`); `:694-708` the
actor now holds `oauth::Store`, the env key read once, and the registry
from `lookup_when`.

Hardening after review (`fix(search-providers): tolerant citation shapes`):
top-level `citations` is `Vec<Value>` so an object-form citation cannot
make the whole response undeserialisable; `url_citation.title` in the
Responses API is the footnote label (`"1"`, `"2"`), so a blank or purely
numeric title is dropped and `citations[].title` is only ever a real title.

Deviations from §3-§5, with reasons:

1. **Cited lines were off.** `supervisor.rs` precedence is at `:6908-6917`
   as cited, the store construction at `:7224-7231` as cited; `web/src/lib.rs`
   registration is `:191-221` (doc said 193-216); `web/tools.rs` consts
   `:358-360`, `structured` `:83-89` as cited; `search-providers/lib.rs`
   `sync` was `:553-568`, `start` `:617-651` before this change.
   `oauth::Store::access` is `:780-847` as cited.
2. **`images`/`videos` are not "filters" for `degraded`.** They are
   understanding flags passed through when given (`true` or `false`);
   `degraded` counts only `handles`, `exclude_handles`, `from_date`,
   `to_date` — the ones that can eat results. §4's table lists them apart
   from the handle/date rules, so this reads as its intent.
3. **`credential_source()` became `credential()` + `advertise_x()`.** The
   pure precedence function takes `(vault key, login result, env key)` so
   it is testable without a vault; the registration predicate is
   `advertise_x(&cfg, has_credential) -> Option<&XConfig>` in `lib.rs`
   (the actor computes `has_credential` from vault, store and env).
4. **`persist` writes the `x:` block.** `open_flow_blocks` turns any
   `x: {…}` into `x:` (null → `None`), so leaving the block unwritten would
   silently disable the tool after a Settings › Search save; writing the
   three scalars costs five lines and the round-trip test covers it.
5. **Step 2's actor tests were not written as actor tests.** The crate has
   never spawned the kameo actor in tests (web/registry handles needed);
   the predicate is unit-tested, `sync_x` is a thin match on it.
6. **`User-Agent`** is `web::WebConfig::default().user_agent` — the
   workspace string, not the user's `web.user_agent` (plugins.md §3.6: a
   plugin does not read another plugin's section).
7. **`cargo fmt --all -- --check`** fails on the untouched base (`95f6496a`
   is not rustfmt-clean under the pinned 1.98.1 toolchain: ~50 files in
   `agent`, `mesh`, `sandbox`, `tui`, …); the gate was run as
   `cargo fmt -p search-providers -- --check`, green. Nothing outside this
   change was reformatted.
8. **Doc 16 §3.1** said `search-providers` "owns no tools"; amended to say
   `search_x` is doc 80's and needs only the tool registry.
