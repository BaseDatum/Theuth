# 16 — Web: search and fetch as a plugin

Status: draft v0 · 2026-09-12 · extends `00-architecture.md` §1, §4, §6;
`01-config-and-subagents.md` §1.1; `07-context.md` §6 (spill). Source
material: `../jarvis/crates/jarvis-search` (19,268 lines) and its two
`jarvis-agent` tools, re-verified against the tree today; every claim about
it cites `file:line` in that crate.

## 0. Decisions in one screen

1. **Two plugins.** `web` owns the tools (`web_search`, `web_fetch`), the
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
4. **Port 25 %, drop 75 %.** Keep the fetcher + SSRF guard, the fan-out/RRF
   aggregator, URL canonicalisation, and the engines that are APIs or
   proven scrapes. Drop the five raw-HTTP SERP scrapers, the dead layers,
   three of four output formats, and the twenty-parameter schema (§7).
5. **Rendered pages come from a provider, not a browser in wth.** Firecrawl
   `/scrape` (or OpenSERP extraction) is the `rendered` path; without one
   registered the call fails with a message naming the plugin to configure.
6. **Risk `ReadOnly`** for both tools: under the default `tools.policy`
   they run without prompting. The SSRF guard is mandatory and not
   configurable off (00 §1: security is never simplified away).

## 1. Why

wth has no way to read the web: no search, no URL fetch. Every agent needs
both daily; it is the single biggest hole against oh-my-pi's tool set
(`read` of URLs, `web_search`). jarvis built it, then buried a working core
under a roadmap towards OpenSERP that never landed (§7).

## 2. Tool surface

Two tools, small schemas. Every field is implemented.

```rust
// web_search - Risk::ReadOnly
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

// web_fetch - Risk::ReadOnly
struct FetchArgs {
    url: String,
    max_chars: Option<u32>,         // 200..=16000, default 4000
    format: Option<FetchFormat>,    // text | markdown; default markdown
    rendered: Option<bool>,         // needs a rendered provider (§5); errors naming `search-providers` otherwise
}
// -> text: "# title\nurl\n\n<content>" (truncated at max_chars with a marker); structured: FetchResponse.
```

Dropped from jarvis's `web_search` schema (`jarvis-agent/src/tools/web_search.rs:197-329`):
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
| `duckduckgo` | web | HTML `html.duckduckgo.com` + lite fallback (`engines/duckduckgo.rs:29-33,158-192`) **and** Instant Answer JSON `api.duckduckgo.com/?q&format=json&no_html=1` | none | general, news | `answer` (Answer), `knowledge_panel` (Abstract + Infobox), `related` (RelatedTopics), `definition` | the JSON endpoint is stable and keyless; the HTML endpoint is the one keyless general web index |
| `wikipedia` | web | MediaWiki search API (`engines/wikipedia.rs:3-9`) + REST `page/summary/{title}` | none | general, science | `knowledge_panel` (summary extract, thumbnail, description) | summary replaces jarvis's `answer_from_top_hit` hack (`aggregate/mod.rs:281-290`) |
| `arxiv` | web | Atom API (`engines/arxiv.rs:3-8`) | none | science | — | keep the hand-rolled Atom scan (no XML dep) |
| `github` | web | `api.github.com/search/repositories` (`engines/github.rs:26-32`) | optional `github` (raises the rate limit) | it | — | keyless by default, so it stays in `web` |
| `brave` | search-providers | `api.search.brave.com/res/v1/web/search` (`engines/brave.rs:9-19`) | `brave` | general, news, it | `knowledge_panel` (infobox), `faq`, `news`, `videos`, `discussions`, `spelling` (query.altered) | JSON fields, no scraping; free tier exists |
| `firecrawl` | search-providers | hosted or self-hosted, `POST {base}/v2/search`, `sources: [web|news|images]`, `tbs`, `includeDomains`; `POST {base}/v2/scrape` | `firecrawl` | general, news, science (`categories: research`), it (`developer`) | `news`, `images`; per-result `markdown` when `scrapeOptions` set | **renders**; `base_url` for self-hosted |
| `searxng` | search-providers | self-hosted, `GET {base}/search?q&format=json&categories&time_range` | none (instance-local) | all | `answer` (answers[]), `knowledge_panel` (infoboxes[]), `related` (suggestions[]), `spelling` (corrections[]) | the off-the-shelf metasearch; 70+ upstream engines with anti-bot handled by a maintained project |
| `openserp` | search-providers | self-hosted, `GET {base}/mega/search?engines&text&extract` | optional (hosted) | general, news, images | everything in its `serp_features[]`: `ai_summary`, `answer`, `people_also_ask`, `related`, `top_stories`, `knowledge_panel` … | **browser-rendered** Google/Bing/Yandex/Baidu — the only way to get JS-only SERP modules honestly; **renders** via `extract` |

Not ported: jarvis's raw-HTTP `google`, `bing`, `yandex`, `baidu`, `ecosia`
scrapers (`src/engines/{google,bing,yandex,baidu,ecosia}/`, ~3,000 lines).
Their fixtures are synthetic (`bing/tests/fixtures/basic_search.html:4`),
their anti-bot handling is a string match for "captcha", and the SERP
modules they were supposed to feed are JS-rendered on those engines —
which is exactly why jarvis's selector tables stayed empty
(`traits.rs:104`; only `#[cfg(test)]` selectors in `features.rs:614-680`).
OpenSERP exists to solve that with a browser pool; we configure it rather
than reimplement a quarter of it. Rejected alternative: keep the scrapers
"as a fallback" — a fallback that returns consent pages is worse than a
missing provider the description does not list.

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

Port `fetch/mod.rs` + `fetch/ssrf.rs` + `fetch/clean.rs` (1,158 lines) and
their tests nearly verbatim; they are the best-tested part of the crate.

- SSRF: scheme/userinfo/host checks, hostname denylist, full private/
  reserved v4+v6 ranges, **DNS resolve-and-check every address, re-checked
  on every redirect hop** (`ssrf.rs:47-62,91-118,161-266`; `fetch/mod.rs:175`).
  Not configurable off. Known TOCTOU on rebinding (`ssrf.rs:4-6`) recorded
  in §10.
- Limits: 8 s total, 2 s connect, 2 MiB body, 5 redirects, binary
  content-types rejected (`fetch/mod.rs:29-33,189-193,225-259`).
- `format: text` = jarvis `clean_html` via `scraper` (main/article/body
  preference, noise tags stripped — `clean.rs:82-148`). `format: markdown`
  = `html-to-markdown-rs` (jarvis `extract.rs:20`), keeping headings and
  links so the model can follow up. Both stacks exist in jarvis; we keep
  both because agents need links (markdown) and token-cheap prose (text).
- `rendered: true` → the first registered provider whose `meta().renders`
  is true (Firecrawl `/v2/scrape` with `formats: ["markdown"]`, else
  OpenSERP extract), in the order `search-providers` registered them. The
  SSRF check still runs on the URL before it is sent anywhere. With none
  registered: `Err("rendered fetch needs a rendering provider: configure
  firecrawl or openserp under search_providers")`.
- `fetch_top` on search reuses the same path for the top N ranked URLs,
  concurrently, each capped at `fetch_top_chars` (default 1500).

## 6. Config and credentials

Two sections, one per plugin (00 §5: every plugin reads its own).

```yaml
web:                          # design doc 16 - built-in engines, fetch, tools
  enabled: true
  engines: [duckduckgo, wikipedia, arxiv, github]
  timeout_ms: 4000            # per provider, built-in or registered
  overall_timeout_ms: 6000
  fetch_max_chars: 4000
  feature_max_chars: 600
  user_agent: "wth/0.1 (+https://github.com/skymoore/wth)"

search_providers:             # design doc 16 §3 - external engines; each block's presence enables it
  firecrawl:
    base_url: https://api.firecrawl.dev   # self-hosted: http://127.0.0.1:3002
  searxng:
    base_url: http://127.0.0.1:8080
  openserp:
    base_url: http://127.0.0.1:7000
    engines: [google, bing]
  brave: {}
```

Keys live in `credentials.yaml` next to provider keys (01 §1.1: "the file
beats the environment"): `brave:`, `firecrawl:`, `github:`, `openserp:`
(hosted only). No new environment variables; jarvis's
`JARVIS_USE_SYSTEM_ROOTS` / `JARVIS_EXTRA_CA_CERTS` (`http_tls.rs:14,32`)
are dropped — the workspace `reqwest` already uses rustls with native
roots (`Cargo.toml:39`), the same client every other plugin builds. A
configured block without its key logs a `warn` at `search-providers`
start and is skipped, mirroring how `mcp/` handles a broken entry.

## 7. What is not ported, and why

| jarvis piece | lines | disposition |
|---|---|---|
| `engines/{google,bing,yandex,baidu,ecosia}` | ~3,000 | dropped (§3): raw-HTTP scrapes of JS-first SERPs; synthetic fixtures; OpenSERP provider covers the need |
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
  src/actor.rs      WebActor: provider set; RegisterProvider / UnregisterProvider / Search / Render
  src/provider.rs   Provider trait, ProviderMeta, Query, RawResult, Feature, FeatureKind (pub - the contract)
  src/tools.rs      SearchTool, FetchTool (tools::Tool impls, description from the live provider set)
  src/aggregate.rs  fan_out + merge_rank_dedupe (RRF k=60, α=0.001 - rank.rs:4-15)
  src/render.rs     result list + features -> tool text
  src/fetch/        mod.rs, ssrf.rs, clean.rs (ported)
  src/url.rs        canonicalize_url, DisplayUrl (ported normalize/url.rs)
  src/engines/      duckduckgo.rs wikipedia.rs arxiv.rs github.rs
  tests/            ssrf matrix, rank/dedupe, url normalisation (ported), one wiremock contract test per engine
  tests/fixtures/   captured responses, dated

crates/search-providers/          plugin "search-providers" - depends on runtime, web (library half)
  src/lib.rs        SearchProvidersPlugin: reads `search_providers`, builds clients, registers with "web"
  src/{brave,firecrawl,searxng,openserp}.rs
  tests/            one wiremock contract test per provider from a captured response
```

`web` dependencies: `reqwest` (workspace), `url`, `schemars`, `serde*`,
`tokio`, `thiserror`, `tracing`, `kameo` — all already in `Cargo.lock`.
New: `scraper` 0.23 (html5ever tree + selectors; the HTML→text path) and
`html-to-markdown-rs` 3.x (markdown path). Dev: `wiremock`.
`async-trait`, `bitflags`, `base64`, `chrono`, `parking_lot`,
`html-escape` from jarvis are not needed. Check at implementation whether
`html-to-markdown-rs` pulls its own html5ever version; if two coexist,
drop `scraper` and derive text from the markdown converter's tree (one
parser, one dependency).

`search-providers` dependencies: `reqwest`, `serde*`, `web`. No HTML
parsing: every external provider returns JSON.

## 9. Branding removed

Crate `jarvis-search` → `web`; `DEFAULT_USER_AGENT` (`config.rs:51-52`) →
`wth/0.1 (+…)` (the binary name is the one permitted product name, 00 §1);
`JARVIS_*` env vars gone (§6); `from_jarvis_settings` and its tests gone;
fixture headers and doc comments rewritten. Tool names `web_search`,
`web_fetch` were already unbranded.

## 10. Risks and known gaps

- **Keyless general search is one HTML endpoint** (`html.duckduckgo.com`).
  It blocks occasionally (jarvis handles 403/202 with a lite retry,
  `duckduckgo.rs:53-61,232-257`). When it fails and no provider is
  configured, search returns a clear failure, and the description already
  told the user how to add a provider. SearXNG is the recommended
  self-hosted answer; Brave the cheapest hosted one.
- **DNS rebinding TOCTOU** in the SSRF guard (`ssrf.rs:4-6`): the fetch
  client resolves again after our check. Mitigation available in reqwest
  (`resolve()` pinning to the checked addresses); do it in the port.
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

1. `web` crate: fetch + SSRF + clean + url canonicalisation ported with
   tests; `web_fetch` registered. Check: fetch a docs page as markdown and
   text; `http://169.254.169.254/` and a hostname resolving to loopback
   are rejected; a 3-hop redirect chain ending on 127.0.0.1 is rejected.
2. Provider trait + aggregator + `duckduckgo` (HTML + instant answers),
   `wikipedia` (+ summary), `arxiv`, `github`; `web_search` registered;
   `Feature` rendering. Check: `web_search{query:"rust async trait"}`
   returns a merged list with `engines` per result; `"2^32"` yields an
   `Answer` from DDG; `"Ada Lovelace"` yields a `KnowledgePanel` from
   Wikipedia; unplugging the network yields `Failed:` lines, not an error.
3. `search-providers` crate: `brave`, `firecrawl` (search + render),
   `searxng`, `openserp`; registration over the bus; descriptions reflect
   the live set. Check: each with a wiremock contract test from a captured
   response; start with only `web` → description lists four engines; add
   a `brave` key and restart → five, `Faq`/`News` named; Firecrawl live on
   a key: `web_fetch{url, rendered:true}` on a JS-only page returns
   content the raw path did not; with `search-providers` removed from the
   plugin list the same call errors naming it.
4. Docs: 00 §4/§10/§12 rows, 01 §1.1 credentials keys, README.

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
