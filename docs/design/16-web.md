# 16 — Web: search and fetch as a plugin

Status: as-built v0 · 2026-09-12 (draft v0 same day) · revised 2026-09-17
(keyless SERP engines, `scrape` render provider — §0.4, §3.2, §5, §13.10-14) ·
extends `00-architecture.md` §1, §4, §6; `01-config-and-subagents.md` §1.1;
`07-context.md` §6 (spill). Source material:
`../jarvis/crates/jarvis-search` (19,268 lines) and its two `jarvis-agent`
tools, re-verified against the tree today; every claim about it cites
`file:line` in that crate. Deviations from the draft are in §13.

Verified 2026-09-12 headless against the real binary, a real model
(`claude-sonnet-4-5`), the real network and **no provider keys**: the
`search_web` description lists `duckduckgo (general, news; answer,
knowledge_panel, definition, related), wikipedia (general, science;
knowledge_panel), arxiv (science), github (it)` and ends with the sentence
telling the user how to enable Firecrawl; `"Ada Lovelace"` merges wikipedia
+ duckduckgo (rank 1 `[duckduckgo, wikipedia]`) with a `knowledge_panel`;
`"uuid"` yields an `answer` plus a knowledge panel; `category: it "tokio"`
returns GitHub repos, `category: science` arXiv; `site: docs.rs, fetch_top:
1` appends a `Fetched:` section truncated at 1500 chars; `read_url
https://example.com/` returns markdown and text; `http://localhost:7799/`
is `rejected: hostname `localhost` is not allowed`; `rendered: true` with no
provider errors naming `firecrawl or openserp under search_providers`.
Not exercised live: Firecrawl, Brave, SearXNG, OpenSERP (wiremock contract
tests from documented response shapes only), the `Images`/`News`/`Faq`/
`Videos`/`Discussions`/`Spelling`/`AiSummary` kinds, and `169.254.169.254`
(integration test `fetcher_rejects_loopback_and_metadata_without_allowlist`
covers it). Tests: web 17 unit + 7 ssrf + 7 url + 13 engines + 12
aggregate; search-providers 4 unit + 12 wiremock; clippy `-D warnings`.

2026-09-17 (HEAD `feat: in tree serp and firecrawl` + review fixes): the
default engine set became `google, bing, brave, qwant, duckduckgo, wikipedia,
arxiv, github` (`crates/web/src/engines/mod.rs` `BUILTIN`); verified by
wiremock tests on dated captures (`crates/web/tests/engines.rs:23-26`), clippy,
and a live run of the five general engines against `tokio async runtime rust`
from the author's network: brave 5 hits + `related`, duckduckgo 5 hits,
google `Blocked (HTTP 429 sorry)`, qwant `Blocked (captcha)`, bing `Blocked
(decoy results)` — the last only after `reject_decoys` landed; before it Bing
returned five confidently unrelated hits (§10).

## 0. Decisions in one screen

1. **Two plugins.** `web` owns the tools (`search_web`, `read_url`), the
   fetcher, the aggregator and the keyless built-in engines; it registers
   on the tool registry like `knowledge` does
   (`crates/knowledge/src/lib.rs:247-270`). `search-providers` owns every
   engine that needs a key or a base URL — **Firecrawl**, **SearXNG**,
   **OpenSERP**, **Brave** — and registers them with `web` over the bus
   (§3.1). Neither touches `agent`. Rationale: the built-in half is
   dependency-light and always on; the external half carries API schemas,
   credentials and billing, and changes on vendors' schedules. Keeping
   them apart means a Firecrawl schema change never touches the fetcher,
   and a machine with no keys never compiles the vendor code.
2. **Providers, not just engines.** One `Provider` trait in `web`; built-in
   and external implementations are indistinguishable to the aggregator.
   Same result and feature types out of all of
   them; the aggregator does not know which are local.
3. **SERP features are a first-class deliverable, complete.** Every feature
   type the tool can emit has at least one producing provider, a fixture
   test, and a documented source. Nothing is advertised that returns
   nothing: jarvis exposed `features`, `extract`, `mode=fast`,
   `extract_mode=rendered` and six result fields that were stubs
   (§7). Here a capability is either implemented or absent from the schema.
4. **Port 25 %, drop 75 % — then bring the SERP scrapers back on SearXNG's
   terms.** Keep the fetcher + SSRF guard, the fan-out/RRF aggregator, URL
   canonicalisation, and the engines that are APIs or proven scrapes. Drop
   jarvis's five raw-HTTP SERP scrapers, the dead layers, three of four
   output formats, and the twenty-parameter schema (§7). Revised 2026-09-17:
   `google`, `bing`, `brave`, `qwant` are back as built-ins — not jarvis's
   code, but the request shapes SearXNG's maintained engine modules use
   (Google WML endpoint + Nokia UA, Qwant v3 JSON + datadome cookie, Bing
   `/ck/a` unwrap, Brave `search.brave.com` HTML), each with per-engine
   `Blocked` detection and the §4.6 zero-yield detector as the drift alarm.
   Alternative beaten: SearXNG as a required sidecar — a second process for a
   single-binary goal (00 §1; 38 §0.5). Why the draft was wrong to drop them
   outright is in §3.2.
5. **Rendered pages come from a provider.** Draft: Firecrawl `/scrape` or
   OpenSERP. As built (23, then 38): the `scrape` plugin's two-tier engine is
   the preferred renderer, `browser`'s obscura isolate the fallback, Firecrawl
   and OpenSERP optional behind `search_providers` (§5); without any the call
   fails naming the three (`crates/web/src/actor.rs:22-23`).
6. **Risk `ReadOnly`** for both tools: under the default `tools.policy`
   they run without prompting. The SSRF guard is mandatory and not
   configurable off (00 §1: security is never simplified away).

## 1. Why

wth has no way to read the web: no search, no URL fetch. Every agent needs
both daily; it is the single biggest hole against oh-my-pi's tool set
(URL `read`, web search). jarvis built it, then buried a working core
under a roadmap towards OpenSERP that never landed (§7).

## 2. Tool surface

Two tools, small schemas. Every field is implemented.

```rust
// search_web - Risk::ReadOnly
struct SearchArgs {
    query: String,
    limit: Option<u8>,              // 1..=20, default 8
    category: Option<Category>,     // general | news | it | science; default general
    site: Option<String>,           // hostname restriction
    recent: Option<Recency>,        // day | week | month | year (maps to each provider's date filter)
    providers: Option<Vec<String>>, // subset of configured providers; default all for the category
    fetch_top: Option<u8>,          // 0..=3: inline the cleaned text of the top N results (default 0)
}
// -> text: ranked list "1. title — url\n   snippet"; then "Features:" blocks (§4); then
//    "Fetched:" sections when fetch_top > 0; then "Failed: brave (rate limited)" if any.
//    structured: SearchResponse (JSON, for RPC/TUI consumers).

// read_url - Risk::ReadOnly
struct FetchArgs {
    url: String,
    max_chars: Option<u32>,         // 200..=16000, default 4000
    format: Option<FetchFormat>,    // text | markdown; default markdown
    rendered: Option<bool>,         // needs a rendered provider (§5); errors naming `search-providers` otherwise
}
// -> text: "# title\nurl\n\n<content>" (truncated at max_chars with a marker); structured: FetchResponse.
```

Dropped from jarvis's search tool schema (`jarvis-agent/src/tools/web_search.rs:197-329`):
`snippet_max`, `format`, `lang/language`, `region`, `date` (replaced by
`recent`), `file`, `start`, `mode`, `features` (always on), `extract*`
(replaced by `fetch_top`), `min_runes`, `output_format`. Models used
`query`, `limit`, `site`, `date` in jarvis transcripts; the rest was
OpenSERP's HTTP API surface, not an agent's.

Tool description is a pure function of config (00 §4): lists the configured
providers and the feature types they can produce, so the model knows what
"news" or "answer" means on this installation. With no keyless engine
enabled and no provider configured, the plugin does not register the
search tool at all (fetch still registers).

Output goes through the registry's spill (07 §6) like every tool; the
`max_chars` default keeps fetch under the cap.

## 3. Providers

One trait in `web`, eight implementations across two crates.
`Provider::search` returns `(Vec<RawResult>, Vec<Feature>)`;
`Provider::render` (fetch a URL through a browser) is optional and only
Firecrawl and OpenSERP implement it.

```rust
// crates/web/src/provider.rs - the cross-plugin contract
#[async_trait]
pub trait Provider: Send + Sync {
    fn meta(&self) -> ProviderMeta;     // name, categories, feature kinds it can produce, renders: bool
    async fn search(&self, client: &reqwest::Client, q: &Query) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError>;
    async fn render(&self, client: &reqwest::Client, url: &Url) -> Result<Rendered, ProviderError> { Err(ProviderError::Unsupported) }
}
```

### 3.1 Registration over the bus

`web` spawns one kameo actor, registered by name `"web"`, with
`RegisterProvider { provider: Arc<dyn Provider> }` and
`UnregisterProvider { name }` — the `ToolRegistry`/`RegisterTool` shape
(00 §4). `search-providers` does `rt.lookup_when::<WebActor>("web")` at
start and registers what its config enables, exactly as `knowledge`
waits for the tool registry. Tool descriptions are recomputed from the
live provider set at every `ListTools` (they are `fn description(&self)
-> String`, and the loop re-lists once per turn — 12 §4), so a provider
registered after the first turn shows up on the next. Schemas are static
(`JsonSchema` derive): `rendered` is always a field and errors clearly
when nothing renders, rather than appearing and disappearing.

If `search-providers` is absent or has nothing configured, `web` is
complete on its own with the built-ins. If `web` is absent,
`search-providers` logs one `warn` ("web plugin not running; nothing to
register with") and does nothing — it owns no tools.

### 3.2 The set

| provider | plugin | kind | key | results | features it produces | notes |
|---|---|---|---|---|---|---|
| `google` | web | `GET www.google.com/wml/search` with one of SearXNG's Nokia user agents per request (`engines/google.rs:23-32,94-99`); `hl=en`, `lr=lang_en`, `tbs=qdr:*` for `recent` | none | general | `related` (`ZWRArf` anchors in the `HExoMb` table, `google.rs:189-216`) | class-token scan of `zMzFAb` rows, `/url?q=` unwrapped (`:144-228`); a `sorry.google.com` / `/sorry` landing, 403 or short `/sorry/` stub is `Blocked` (`:124-142`) |
| `bing` | web | `GET www.bing.com/search` with a browser UA (`engines/bing.rs:24-26,81-88`) | none | general | — | `li.b_algo` rows, `/ck/a?u=a1…` click tracking base64url-decoded (`crate::url::unwrap_bing_redirect`); `/rewardsapp`, `/challenge`, `b_captcha` or 403 is `Blocked` (`:100-115`); no time filter → `recent` is `Unsupported` (`:74-76`) |
| `brave` | web | `GET search.brave.com/search?q&source=web[&tf=]` with a browser UA and SearXNG's region/SafeSearch cookies (`engines/brave.rs:24-30,81-93`) | none | general | `related` (`a.related-query`, `brave.rs:191-210`) | `div.snippet` rows with `title`/`content`, `t-secondary` date split off (`:129-189`); 403 `Blocked`, 429 `RateLimited`, empty 2xx is `Ok([])` (`:113-121`) |
| `qwant` | web | `GET api.qwant.com/v3/search/web` JSON, SearXNG's exact parameter order (the API fingerprints on it), `datadome` cookie replayed from the last response (`engines/qwant.rs:79-103`) | none | general | — | `mainline[].type == "web"` rows only (`:185-210`); `error_code 24` → `RateLimited`, a captcha redirect body → `Blocked` (`:135-155`); no time filter → `recent` is `Unsupported` |
| `duckduckgo` | web | HTML `html.duckduckgo.com` + lite fallback **and** Instant Answer JSON `api.duckduckgo.com/?q&format=json&no_html=1` | none | general, news | `answer`, `knowledge_panel`, `related`, `definition` | back in the default `web.engines` (2026-09-17 live smoke: it and Brave were the two general indexes that answered this network while Google, Bing and Qwant walled it); five general indexes fan out per query |
| `wikipedia` | web | MediaWiki search API (`engines/wikipedia.rs:3-9`) + REST `page/summary/{title}` | none | general, science | `knowledge_panel` (summary extract, thumbnail, description) | summary replaces jarvis's `answer_from_top_hit` hack (`aggregate/mod.rs:281-290`) |
| `arxiv` | web | Atom API (`engines/arxiv.rs:3-8`) | none | science | — | keep the hand-rolled Atom scan (no XML dep) |
| `github` | web | `api.github.com/search/repositories` (`engines/github.rs:26-32`) | optional `github` (raises the rate limit) | it | — | keyless by default, so it stays in `web` |
| `brave_api` | search-providers | `api.search.brave.com/res/v1/web/search` (`crates/search-providers/src/brave.rs:18`) | `brave` | general, news, it | `knowledge_panel` (infobox), `faq`, `news`, `videos`, `discussions`, `spelling` (query.altered) | JSON fields, no scraping; free tier exists. Provider name `brave_api` because the keyless scraper owns `brave`; config block `brave:` and secret `brave` unchanged (§13.14) |
| `firecrawl` | search-providers | hosted or self-hosted, `POST {base}/v2/search`, `sources: [web|news|images]`, `tbs`, `includeDomains`; `POST {base}/v2/scrape` | `firecrawl` | general, news, science (`categories: research`), it (`developer`) | `news`, `images`; per-result `markdown` when `scrapeOptions` set | **renders**; block present by default, inert without its secret (`search-providers/src/lib.rs:36-44`) |
| `searxng` | search-providers | self-hosted, `GET {base}/search?q&format=json&categories&time_range` | none (instance-local) | all | `answer` (answers[]), `knowledge_panel` (infoboxes[]), `related` (suggestions[]), `spelling` (corrections[]) | **opt-in** (`searxng: None` by default, `lib.rs:40`); the off-the-shelf metasearch when you already run one — its engine modules are also where our four scrapers' techniques come from |
| `openserp` | search-providers | self-hosted, `GET {base}/mega/search?engines&text&extract` | optional (hosted) | general, news, images | everything in its `serp_features[]`: `ai_summary`, `answer`, `people_also_ask`, `related`, `top_stories`, `knowledge_panel` … | **browser-rendered** Google/Bing/Yandex/Baidu — the only way to get JS-only SERP modules honestly; **renders** via `extract` |

**On the SERP scrapers.** The 2026-09-12 build dropped jarvis's raw-HTTP
`google`, `bing`, `yandex`, `baidu`, `ecosia` scrapers
(`src/engines/{google,bing,yandex,baidu,ecosia}/`, ~3,000 lines): synthetic
fixtures (`bing/tests/fixtures/basic_search.html:4`), anti-bot handling that
was a string match for "captcha", and empty selector tables (`traits.rs:104`)
— a fallback that returns consent pages is worse than a missing provider. That
reasoning still holds for *jarvis's* scrapers. What changed on 2026-09-17: the
goal is one binary with no external API, and one keyless HTML endpoint
(DuckDuckGo) was not enough of an index. SearXNG solves the same problem in
the open with maintained, keyless engine modules, so their request shapes were
ported from `~/Develop/searxng` — Google's WML endpoint served only to Nokia
user agents (`google.rs:23-32`), Qwant's fingerprinted parameter order and
datadome cookie (`qwant.rs:3-6,84-97`), Bing's `/ck/a` unwrap, Brave's
cookies (`brave.rs:30`) — with per-engine `Blocked` classification and
captured, dated fixtures (`crates/web/tests/fixtures/*-2026-09-17.*`). The
features they were "supposed to feed" are still not scraped: only `related`
(Google, Brave) comes from the SERP HTML. Alternative beaten: run SearXNG as a
required sidecar (38 §0.5). Not revisited: yandex, baidu, ecosia.

Selection per query: configured providers whose categories include the
requested one; `providers:` narrows. Fan-out is jarvis's
(`aggregate/fanout.rs:77-235`): `JoinSet`, semaphore, per-provider and
overall timeouts, partial failure reported, never fatal unless all fail.

## 4. SERP features — the contract

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKind {
    Answer,          // direct answer / calculation / conversion
    KnowledgePanel,  // entity card: title, text, attributes, source url, image url
    Faq,             // question -> answer pairs (Brave faq, OpenSERP people_also_ask)
    Related,         // related searches / topics
    Spelling,        // "did you mean" / altered query
    News,            // dated headlines with source
    Videos,
    Images,
    Discussions,     // forum threads (Brave)
    AiSummary,       // engine-generated summary with citations (OpenSERP only)
    Definition,      // dictionary sense (DDG)
}

pub struct Feature {
    pub kind: FeatureKind,
    pub provider: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub items: Vec<FeatureItem>,   // { title, text, url?, date? } - FAQ pairs, related terms, headlines
    pub links: Vec<Link>,          // citations / sources
    pub image: Option<String>,
    pub attributes: Vec<(String, String)>,  // infobox rows
}
```

"Complete" means, checkable at review:

1. **Every `FeatureKind` has at least one producing provider in §3's
   table**, with a fixture test built from a **captured live response**
   (dated file header), not a hand-written one. A kind with no producer is
   deleted from the enum, not kept "for later".
2. **Producers are parsers of structured responses**, not CSS selectors
   over rendered HTML — except OpenSERP, which does the rendering and
   hands us JSON. The selector walker (`features.rs`, 1,099 lines) and
   `SerpFeatureSelector` are not ported; they solved OpenSERP's problem,
   which we delegate to OpenSERP.
3. **Cross-provider dedupe** by `(kind, normalised title, first item)`
   — jarvis's `deduplicate_serp_features` (`features.rs:343`) keyed the
   same way; keep the key, drop the walker.
4. **Rendered in the tool text** in a fixed order after the ranked list:
   `Answer`, `Spelling`, `KnowledgePanel`, `AiSummary`, `Faq`, `News`,
   `Definition`, `Related`, then the rest; each capped (`feature_max_chars`,
   default 600) so features never crowd out results.
5. **The description names the configured kinds.** No provider producing
   `News` → the word does not appear in the description.
6. **Drift is a failure, not silence.** A provider whose response parses
   but yields zero results *and* zero features for a non-empty query on
   three consecutive calls is reported as `Failed: <provider> (no results;
   response shape may have changed)` and logged at `warn` — jarvis's
   roadmap called this `selector_drift` and never built it.

## 5. Fetch

Port `fetch/mod.rs` + `fetch/ssrf.rs` (and the tests) nearly verbatim; they
are the best-tested part of the crate. `fetch/clean.rs` is replaced by
`fetch/text.rs` (§13).

- SSRF: scheme/userinfo/host checks, hostname denylist, full private/
  reserved v4+v6 ranges, **DNS resolve-and-check every address** through a
  custom reqwest `dns::Resolve` (`ssrf::GuardedResolver`) that hands the
  client only checked addresses, and the static guard re-run on every
  redirect hop (redirects are followed manually). Not configurable off.
  jarvis's TOCTOU (`ssrf.rs:4-6`) does not exist here (§13).
- Limits: 8 s total, 2 s connect, 2 MiB body, 5 redirects, binary
  content-types rejected — except documents (PDF, Office, …), which are
  sniffed from the body and rendered as Markdown (20 §2.3)
  (`fetch/mod.rs:29-33,189-193,225-259`).
- One HTML parser: `format: markdown` = `html-to-markdown-rs` 3.12
  (default features off), keeping headings and links so the model can
  follow up; `format: text` is a projection of that markdown (headings,
  links and emphasis stripped), not a second parse. Agents get links
  (markdown) or token-cheap prose (text) from one tree.
- `rendered: true` → `WebActor`'s `Render` picks the provider named `scrape`
  when it has `renders: true`, else the first registered renderer in
  registration order (`crates/web/src/actor.rs:154-159`). Order as built:
  `scrape` (38 §5: plain fetch → `transform_html` → markdown, obscura only
  when the page comes back thin or the fetch fails — so a rendered read of a
  static page never opens a browser), then `browser` (23: the embedded
  obscura isolate, `browser_navigate` → `browser_markdown` → `browser_close`,
  `crates/browser/src/render.rs:86-89`), then Firecrawl (`/v2/scrape` with
  `formats: ["markdown"]`) and OpenSERP extract when `search-providers`
  registers them. The SSRF check still runs on the URL before it is sent
  anywhere (`tools.rs:313-317`); the browser's own private-network guard is a
  separate setting (`browser.allow_private_network`, 23 §0.8) and does not
  loosen this fetcher's. With none registered: `rendered fetch needs a
  rendering provider: scrape, browser, or firecrawl` (`actor.rs:22-23`).
- `fetch_top` on search (0..=5, `tools.rs:195-200`) sends each of the top N
  ranked URLs through the same `Render` path concurrently, falling back to a
  plain text fetch when render fails (`tools.rs:222-262`); each page is capped
  at `fetch_top_chars` (default 4000, `crates/web/src/lib.rs:65`).

## 6. Config and credentials

Two sections, one per plugin (00 §5: every plugin reads its own).

```yaml
web:                          # design doc 16 - built-in engines, fetch, tools
  enabled: true
  engines: [google, bing, brave, qwant, duckduckgo, wikipedia, arxiv, github]   # engines::BUILTIN
  timeout_ms: 4000            # per provider, built-in or registered
  overall_timeout_ms: 6000
  fetch_max_chars: 4000
  fetch_top_chars: 4000
  feature_max_chars: 600
  user_agent: "wth/0.1 (+https://github.com/skymoore/wth)"

search_providers:             # design doc 16 §3 - external engines; each block's presence enables it
  firecrawl:                  # the one block present by default; inert without the `firecrawl` secret
    base_url: https://api.firecrawl.dev   # self-hosted: http://127.0.0.1:3002
  # searxng:                  # opt-in: no default base_url (search-providers/src/lib.rs:36-44)
  #   base_url: http://127.0.0.1:8080
  # openserp:
  #   base_url: http://127.0.0.1:7000
  #   engines: [google, bing]
  # brave: {}                 # Brave Search API; registers as `brave_api`, needs the `brave` secret
```

Defaults are `WebConfig::default()` (`crates/web/src/lib.rs:57-72`) and
`SearchProvidersConfig::default()` (`crates/search-providers/src/lib.rs:36-44`):
only `firecrawl` is on, and only once its secret exists. The built-in engines
need no block under `search_providers`.

Keys are secrets in the encrypted vault next to the provider keys (37;
`/secrets add`, Settings › Secrets or `wth --secret-set`), under the names
`brave`, `firecrawl`, `github`, `openserp` (hosted only); `search-providers`
reads them through `secrets::Handle` at start (37 §3.6). No new environment variables; jarvis's
`JARVIS_USE_SYSTEM_ROOTS` / `JARVIS_EXTRA_CA_CERTS` (`http_tls.rs:14,32`)
are dropped — the workspace `reqwest` already uses rustls with native
roots (`Cargo.toml:39`), the same client every other plugin builds. A
configured block without its key logs a `warn` at `search-providers`
start and is skipped, mirroring how `mcp/` handles a broken entry.

## 7. What is not ported, and why

| jarvis piece | lines | disposition |
|---|---|---|
| `engines/{google,bing,yandex,baidu,ecosia}` | ~3,000 | dropped 2026-09-12 (§3.2): raw-HTTP scrapes with synthetic fixtures and no anti-bot classification. **Superseded 2026-09-17**: `google`, `bing`, `brave`, `qwant` rewritten in `web` from SearXNG's engine modules — different endpoints (Google WML, Qwant v3 JSON), per-engine `Blocked`, captured fixtures. jarvis's code was still not ported; yandex/baidu/ecosia were not revisited |
| `features.rs` selector walker | 1,099 | dropped (§4.2): never had a production selector; providers hand us structured features |
| `extract.rs` Auto/Rendered modes | 916 | dropped: `Rendered` always errored "browser not available" (`extract.rs:283-289`); replaced by the Firecrawl/OpenSERP fetch path |
| `aggregate/clusters.rs` | 305 | dropped: duplicates dedupe's grouping; no consumer |
| `Mode::{Fast,Any}` (`aggregate/mod.rs:170-181,300-355,424-437`) | ~250 | dropped: `Fast` ranked by hit count pretending to be latency; `Any` was serial with a hardcoded 8 s |
| `format.rs` json/text/ndjson | ~400 | dropped: one text renderer for the model; JSON is the `structured` field |
| `RankedResult` v2 fields `kind, display_url, domain, favicon, position, classification` (`types.rs:448-491`) | — | dropped: never set (`dedupe.rs:196`) |
| `SearchSettings` mirror + `from_jarvis_settings` (`config.rs:229-294`) | ~150 | dropped: `rt.section("web")` deserialises the struct directly |
| `SearchQuery` 20 fields (`types.rs:159-233`) | — | replaced by §2 |
| `http_tls.rs` | ~60 | dropped (§6) |
| Kept: `fetch/*`, `aggregate/{fanout,dedupe,rank}`, `normalize/url.rs`, `engines/{duckduckgo,wikipedia,arxiv,github,brave}` minus their `features()` stubs, `error.rs` | ~4,500 incl. tests | ported with branding removed (§9) |

Net: ~4.5k ported + ~1.2k new (Firecrawl, SearXNG, OpenSERP providers,
DDG instant answers, Wikipedia summary, `Feature` rendering, plugin glue).

## 8. Crates and dependencies

```
crates/web/                       plugin "web" - depends on runtime, tools
  src/lib.rs        WebPlugin: reads `web`, spawns WebActor ("web"), registers tools (knowledge pattern)
  src/actor.rs      WebActor: provider set; RegisterProvider / UnregisterProvider / ListProviders / Search / Render
  src/provider.rs   Provider trait, ProviderMeta, Query, RawResult, Feature, FeatureKind, Rendered, ProviderError (pub - the contract)
  src/tools.rs      SearchTool, FetchTool (tools::Tool impls, description from the live provider set)
  src/aggregate.rs  fan-out (JoinSet, semaphore 6, per-provider + overall timeout), RRF k=60 α=0.001, feature dedupe/order, zero-yield detector
  src/render.rs     result list + features -> tool text
  src/fetch/        mod.rs (limits, manual redirects), ssrf.rs (guard + GuardedResolver), text.rs (markdown -> text)
  src/url.rs        canonicalize_url, DisplayUrl (ported normalize/url.rs)
  src/engines/      mod.rs (BUILTIN, Elem class-token scanner, entity decode) google.rs bing.rs brave.rs qwant.rs duckduckgo.rs wikipedia.rs arxiv.rs github.rs
  tests/            ssrf matrix, rank/dedupe, url normalisation, engines (dated fixtures under tests/fixtures/), aggregate

crates/search-providers/          plugin "search-providers" - depends on runtime, web (library half)
  src/lib.rs        SearchProvidersPlugin: reads `search_providers`, builds clients, lookup_when("web"), registers
  src/{brave,firecrawl,searxng,openserp}.rs   (brave.rs registers as `brave_api`)
  tests/            wiremock contract tests per provider (documented response shapes)
```

`web` dependencies: `reqwest` (workspace, now with the `query` feature),
`schemars`, `serde*`, `tokio`, `thiserror`, `tracing`, `kameo`. New to the
workspace: `url`, `html-to-markdown-rs` 3.12 (default features off; the
only HTML parser - §13). Added 2026-09-17: `rand` (already in the workspace;
Google's per-request Nokia UA pick) and `flate2` (gzip sitemaps for 38,
inflated in the fetcher) — `crates/web/Cargo.toml:24-25`. Dev: `wiremock`.
`scraper`, `bitflags`, `base64`, `chrono`, `parking_lot`, `html-escape` from
jarvis are not used; Bing's `/ck/a` base64url is decoded by hand in
`url.rs`. Keys come from the secrets vault through `secrets::Handle`
(37 §3.6; `runtime::credentials` is gone).

`search-providers` dependencies: `reqwest`, `serde*`, `web`. No HTML
parsing: every external provider returns JSON.

## 9. Branding removed

Crate `jarvis-search` → `web`; `DEFAULT_USER_AGENT` (`config.rs:51-52`) →
`wth/0.1 (+…)` (the binary name is the one permitted product name, 00 §1);
`JARVIS_*` env vars gone (§6); `from_jarvis_settings` and its tests gone;
fixture headers and doc comments rewritten. Tool names changed for a
different reason (§13).

## 10. Risks and known gaps

- **Keyless SERP scrapes can be blocked, and one blocks by lying.** Live on
  2026-09-17 from the author's network: Google answered `/wml/search` with
  its `sorry` wall (HTTP 429), Qwant with a DataDome captcha
  (`crates/web/tests/fixtures/google_wml-blocked-2026-09-17.html`,
  `qwant_web-blocked-2026-09-17.txt`), and **Bing returned HTTP 200 with the
  right `<title>` and ten cached results for an unrelated query** — the
  decoy SERP flagged clients get, also reproducible with `curl`
  (`bing_search-decoy-2026-09-17.html`). Brave and DuckDuckGo answered
  correctly. Defences: per-engine `Blocked` classification (`Failed: google
  (HTTP 429 sorry)`), `engines::reject_decoys` — a non-empty SERP in which
  no hit's title, snippet or URL contains any query word ≥ 3 chars is
  `Blocked("decoy results…")` rather than handed to the model
  (`crates/web/src/engines/mod.rs`), and §4.6's zero-yield detector for a
  SERP that parses but yields nothing. Five general indexes fan out per query
  so one wall does not empty the result; when all fail, search returns a
  clear failure and the description points at SearXNG or a keyed provider
  (`NO_EXTERNAL_PROVIDER`, `crates/web/src/tools.rs:25`).
- ~~**DNS rebinding TOCTOU** in the SSRF guard (`ssrf.rs:4-6`)~~ — closed
  at build time by `GuardedResolver` (§5, §13): the client never resolves
  on its own.
- **Provider response drift** (Brave/Firecrawl schema versions,
  OpenSERP's `serp_features` shapes): contract tests against dated
  fixtures, and §4.6's zero-yield detector at runtime.
- **Cost**: Firecrawl bills per search/scrape; `fetch_top` multiplies it.
  Default `fetch_top: 0`; the description says fetch is billable when the
  configured rendered provider is hosted.
- **Multi-user (15)**: these tools make network egress from the control
  plane and touch no host state, so they stay control-plane side; the
  provider keys are the operator's.

## 11. Implementation order

Each step leaves `wth` runnable.

1. `web` crate: fetch + SSRF + url canonicalisation ported with tests;
   `read_url` registered. Checked: `https://example.com/` as markdown
   (`# Example Domain`, `[Learn more](…)`) and text; `http://localhost:7799/`
   rejected by hostname; `169.254.169.254` and a redirect onto loopback by
   integration test, not live.
2. Provider trait + aggregator + `duckduckgo` (HTML + instant answers),
   `wikipedia` (+ summary), `arxiv`, `github`; `search_web` registered;
   `Feature` rendering. Checked: `"Ada Lovelace"` merges wikipedia +
   duckduckgo with a `knowledge_panel`; `"uuid"` yields an `answer` (DDG
   IA `Answer` - §13; `"2^32"` does not); `category: it "tokio"` → GitHub,
   `category: science` → arXiv; `fetch_top: 1` appends `Fetched:`. Not
   checked: network unplugged.
3. `search-providers` crate: `brave`, `firecrawl` (search + render),
   `searxng`, `openserp`; registration over the bus; descriptions reflect
   the live set. Checked: wiremock contract test per provider (documented
   shapes, no captured responses - no keys or instances); with only `web`
   the description lists four engines and names Firecrawl as the way in;
   `rendered: true` with no provider errors naming `search_providers`;
   startup with `firecrawl` enabled and no key logs `firecrawl: no key …;
   add `firecrawl: <key>` (free tier at firecrawl.dev) to enable web search
   through it` naming where the key goes (the secrets vault since 37). Not checked live: a key on any
   hosted provider, a JS-only page through `rendered: true`.
4. Docs: 00 §10/§12 rows and deps, 01 §1.1 credentials keys, README, this
   §13.

## 12. Open questions

1. Default `format` for fetch: markdown (links, more tokens) vs text. Leaning
   markdown; measure token cost on ten typical pages during step 1.
2. Should `fetch_top` default to 1 when a self-hosted (free) rendered
   provider is configured? Leaning no: one knob, explicit.
3. Per-session result cache (same query within a turn is common with
   subagents fanning out): none in v0; add if transcripts show repeats.
4. `Images` kind: rendered as URLs only; is it worth carrying at all
   without a vision consumer? Keep because Firecrawl/OpenSERP give it for
   free; drop if unused by the first live check.

## 13. Revisions at build time

1. **Tools are `search_web` / `read_url`, not `web_search` / `web_fetch`.**
   genai 0.7.0-beta.23 maps any tool literally named `web_search` onto
   Anthropic's server tool `web_search_20250305`
   (`adapter/adapters/anthropic/adapter_shared.rs:736-748`); the model then
   emitted `server_tool_use` and the turn died with `unhandled content
   type: server_tool_use`. Renamed in code; crate `web`, config sections
   `web:` / `search_providers:` unchanged. §9's "already unbranded" claim
   was true and irrelevant.
2. **One HTML parser.** §8's rule fired: `scraper` was not added.
   `html-to-markdown-rs` 3.12 (default features off) is the only parser;
   `format: text` is derived from the markdown (`fetch/text.rs`), and
   jarvis's `clean.rs` was not ported.
3. **TOCTOU closed.** `ssrf::GuardedResolver` implements reqwest's
   `dns::Resolve`: it rejects when any resolved address is non-global and
   returns only checked addresses, so one shared client is enough (no
   per-fetch client with `resolve()` pinning). Redirects are followed
   manually with the static guard per hop. §10's bullet is struck.
4. **DDG `Answer` scope.** The Instant Answer `Answer` field fires for its
   answer types (guid/uuid, unit conversions, "my ip"), **not** arithmetic:
   `"2^32"` yields only a knowledge panel. §11's check was rewritten to
   `"uuid"`.
5. **OpenSERP.** `recent` is not forwarded (its API wants civil dates, not
   relative ranges); only `[General]` is requested as a category; render is
   `GET /extract?url=`.
6. **Firecrawl.** `/v2/search` general requests `sources: [web, news,
   images]`; `/v2/scrape` renders; a 402 is surfaced as `payment required:
   free credits exhausted`. `firecrawl: { hosted, enabled }` is present in
   `SearchProvidersConfig` **by default**, gated on the key: without one
   the plugin logs how to get a free key and skips it, and the search tool's
   description ends with the same pointer.
7. **Brave.** Snippet cleanup is HTML entity decoding only; no tag
   stripping was needed on the documented shapes.
8. **`rendered` always in the schema.** Confirmed as §3.1 states: the field
   is static and, with no rendering provider, fails with a message naming
   the renderers — since 2026-09-17 `rendered fetch needs a rendering
   provider: scrape, browser, or firecrawl` (`crates/web/src/actor.rs:22-23`).
9. **`ListProviders`** was added to `WebActor` so the tool description and
   `search-providers`' startup can read the live set without a `Search`.
10. **`NO_EXTERNAL_PROVIDER` rewritten** (2026-09-17). The built-ins are real
    indexes now, so the description no longer says "add Firecrawl to get web
    search"; it says search runs on the keyless engines and that a SearXNG
    instance or a `brave_api`/`firecrawl` secret adds SERP features
    (`crates/web/src/tools.rs:25`; appended only when no provider came from
    outside `BUILTIN`, `tools.rs:126-130`).
11. **`fetch_top` is 0..=5 and goes through `Render`.** Draft: 0..=3, plain
    fetch, 1500 chars. Built: `fetch_top` 0..=5 (`tools.rs:195-200`), each URL
    through `WebActor::Render` — the `scrape` provider's two-tier pick, so a
    JS-only hit reads correctly — with a plain text fetch as fallback
    (`tools.rs:222-262`); `fetch_top_chars` default 4000 (`lib.rs:65`), the
    same as `fetch_max_chars`, because the Firecrawl-billing reason for a
    small cap no longer applies to the default renderer.
12. **`ProviderError::Unsupported` from `search` is a silent skip.** A
    provider that cannot serve the query shape (Bing and Qwant on `recent`,
    `bing.rs:74-76`, `qwant.rs:72-74`) appears in neither `ok` nor `failed`
    (`crates/web/src/aggregate.rs:74-76,271-273`;
    `tests/aggregate.rs:261`). Alternative beaten: reporting it under
    `Failed:` — the model would read a capability gap as an outage.
13. **Engine parsers share one scanner.** `elements_with_class`, `attr`,
    `http_url`, `decode_entities` in `engines/mod.rs:85-228` serve Google,
    Bing, Brave and DDG; no DOM crate (§13.2 still holds).
14. **Brave API is `brave_api`.** The keyless `search.brave.com` scraper in
    `web` took the name `brave` (`engines/brave.rs:22`); the keyed
    `search-providers` provider is `brave_api` (`search-providers/src/brave.rs:18`).
    Config block `brave:` and vault secret `brave` are unchanged so existing
    installs keep working; only the provider name in descriptions,
    `providers: [...]` and `Failed:` lines changed.
15. **SearXNG is opt-in.** Draft §5 of 38 made it the first provider; built
    with no default block (`search-providers/src/lib.rs:40`) once the in-tree
    engines covered `general` without a sidecar.
