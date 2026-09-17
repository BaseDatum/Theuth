# 38 — Scrape: a self-contained crawler from a vendored `firecrawl_rs`

Status: as-built v0 · 2026-09-17 (draft v0 2026-09-16) · extends `16-web.md`
§5 (rendered fetch) and **supersedes its decision 5** for the local path,
`23-browser.md` §3 (obscura isolates), `26-search.md` (aggregator),
`00-architecture.md` §1 (single binary), §4 (tools), §6 (risk). Measured
against `~/Develop/misc/firecrawl` at `c563a66` and this tree today. §11 is
the as-built status; §12 lists what review changed between draft and build.

## 0. Decisions in one screen

1. **Copy the crate, then edit the copy.** `cp -R` of Firecrawl's
   `apps/api/native/src` into `crates/firecrawl-native/src`, then strip the napi
   layer in place. This is *not* a rewrite and not a regeneration from reading
   their code — the edits are subtraction only (§2), with one inlining. Their
   heuristic tuning and the tests that encode it are the thing we want;
   anything re-typed loses it. As built: `crawler.rs` 1,308 lines and
   `html.rs` 1,023 lines survive; 22 upstream `#[test]`s run under `cargo test
   -p firecrawl-native` (`crates/firecrawl-native/src/crawler.rs:810`).
2. **Two new crates, one direction.** `firecrawl-native` (vendored, AGPL — §8)
   holds pure logic; `scrape` holds the orchestrator and the tools.
   `scrape → {firecrawl-native, web, browser}` (`crates/scrape/Cargo.toml:9-13`);
   `browser` already depends on `web`, so there is no cycle. Nothing in
   `agent` changes; `rpc` and `tui` depend on `scrape` only for the
   `ScrapeProgress` event type (`crates/rpc/src/bridge.rs:19`,
   `crates/tui/src/lib.rs:72`).
3. **Two-tier engine, not a ladder.** Tier one is `web::Fetcher::fetch_html`
   (raw body, SSRF-guarded per hop, `crates/web/src/fetch/mod.rs:164-170`) →
   `firecrawl_native::transform_html` (always: `<base>`/href/src absolutised;
   chrome removed when `only_main_content`) → `html-to-markdown-rs` → base64
   image strip → `post_process_markdown` (`crates/scrape/src/engine.rs:298-315`).
   An empty main-content result re-runs on the whole page
   (`engine.rs:311-313`). Tier two is obscura via `browser::Browsers`
   (`browser_navigate → browser_markdown → browser_close`, `engine.rs`
   `render_with`), taken when `rendered: true`, when the fetch failed, or
   when the fetch came back **thin**. `is_thin` (`engine.rs`) is
   **prose-based** since 2026-09-17 (§12.15): thin when
   `web::excerpt::prose_chars(markdown)` — the chars of the paragraphs 16 §4.7
   would score, after its noise filter drops nav, link lists, image-only and
   link-dense blocks — is < 800, or < 2500 together with an SPA root marker
   (`id="root"`, `id="app"`, `__next`) in the first 4 KiB of HTML. Until that
   day the gate measured raw markdown length (<200, or <800 with a marker),
   and rtings.com's server-rendered nav + product list — thousands of chars
   of link markdown around a client-rendered article — passed it as a real
   page. The browser actor is looked up at call time (`engine.rs`
   `browser_actor`), so there is no plugin start-order dependency and no read
   of `web`'s config (`plugins.md` §3.6). A domain with learned routes is the
   model's third option (42): picked by the prompt block, not by this
   engine. Their 16-engine ladder is a hosted-service dispatcher and did not
   port; `engpicker.rs` was dropped with it (§2).
4. **One converter per path, one shared post-processor.** Fetch path keeps
   `html-to-markdown-rs` (already a `web` dependency); render path uses
   obscura's own `browser_markdown`; both then run `post_process_markdown`
   from the vendored crate (`crates/firecrawl-native/src/html.rs:955`;
   `engine.rs:106,148,308`). Never chain two converters. The one place that
   looked like it needed to — sitemap XML — is fetched raw and parsed with
   `process_sitemap` instead (§12.1).
5. **Search depends on neither their API nor SearXNG.** The draft made a
   SearXNG instance the first provider; as built, `web`'s keyless engines
   (`google`, `bing`, `brave`, `qwant`, `wikipedia`, `arxiv`, `github`,
   `crates/web/src/engines/mod.rs:32-40`) answer `search_web` with no second
   process, and `search_providers.searxng` is **opt-in** (no default
   `base_url`, `crates/search-providers/src/lib.rs:36-44`). Alternative beaten:
   SearXNG as a required sidecar — a second process/container for a single-
   binary goal (00 §1). The SERP-scraper history and the port from SearXNG's
   engine modules are in 16 §3.2 and §13.
6. **No queue, no Postgres, no Redis.** One process: `futures::stream::iter(batch).buffered(n)`
   over a BFS frontier (`crates/scrape/src/crawl.rs:278-294`), an in-memory
   visited set keyed by `normalize()` (`crawl.rs:599-620`), a per-run page
   budget and a hard run deadline on `CrawlOpts` (`crawl.rs:101,123`). Their
   fleet (nuq worker, prefetch worker, RabbitMQ, pg_cron reaper — the six pods
   in `~/Develop/skymoore/k8s/firecrawl`) exists to hand work between
   processes; we have one.
7. **Risk `ReadOnly`** for all three tools (`crates/scrape/src/tools.rs:103,214,317`);
   `crawl` is bounded by config defaults (§6) so a
   stray call cannot fan out across a domain unbounded. The per-hop SSRF
   re-check is not configurable off.
8. **Runtime functions, not direct tools** (`Tool::DIRECT = false`,
   `tools.rs:104,215,318` and the compile-time assertion at `tools.rs:359-363`).
   A crawl returns pages the model works on, so per 08 §0.4 its output lands
   in a variable (`pages = await crawl(url="…")`), never in the transcript.
   Alternative beaten: `DIRECT: true` "so the card shows up" — the desktop
   card and TUI row are driven by the `ScrapeProgress` bus event instead (§6).
9. **`map_site`, not `map`.** monty resolves Python builtins before host
   function lookup, so a runtime function named `map` was unreachable from a
   cell — `map(...)` was Python's `map`. Renamed in code, prompt and UI
   (`tools.rs:1,18-22`; `crates/tui/src/model.rs:151`). Alternative beaten:
   shadowing the builtin in the sandbox prelude — it would break any cell
   that legitimately uses `map()`.

## 1. Why

The self-host works — `~/Develop/skymoore/k8s/firecrawl` runs api + scrape
worker + nuq-worker(×2) + nuq-prefetch-worker + extract-worker + a playwright
sidecar + redis + rabbitmq + Postgres, pinned to `ghcr.io/firecrawl/firecrawl:2.11.162`,
with `PLAYWRIGHT_MICROSERVICE_URL`, a litellm gateway for `extract`, SearXNG for
search, and `CRAWL_CONCURRENT_REQUESTS=5 / NUM_WORKERS_PER_QUEUE=4 /
MAX_CONCURRENT_JOBS=5 / BROWSER_POOL_SIZE=5`. Quality comes from that whole set,
not from the API container. Costs: six pods, a queue hop before the first byte,
cold workers, and an HTTP round-trip from the agent to a cluster service.

For our use — read a docs site, a changelog, a handful of pages deep — we need
exactly three of their behaviours: fetch, convert to readable markdown, and walk
internal links with scope and limits. Obscura and the `web` fetcher already cover
fetch and convert. What is missing is the walk, and that is precisely what their
native crate implements. `apps/api/package.json` is the surrounding service;
`apps/api/native` is ~3,000 lines of logic, already Rust, already tested.

## 2. Vendoring procedure, and what survived

Copy first, then edit. Do not retype the module from a reading of it, and do not
ask a model to regenerate it: `html.rs` is accumulated output-shaping tuning and
`crawler.rs` carries the tests that pin those expectations, so a fresh
generation silently diverges from what their self-host actually emits.

```
mkdir -p crates/firecrawl-native/src
cp -R ~/Develop/misc/firecrawl/apps/api/native/src/. crates/firecrawl-native/src/
cp ~/Develop/misc/firecrawl/apps/api/native/Cargo.toml crates/firecrawl-native/Cargo.toml.upstream
```

Then, in the copy only — as done:

1. `Cargo.toml`: `crate-type = ["lib"]` (`crates/firecrawl-native/Cargo.toml:10`;
   upstream `["cdylib"]`), `license = "AGPL-3.0"` (`:7`). `napi`, `napi-derive`
   and `build.rs` gone. Dependencies that remain: `kuchikiki 0.8.2`, `psl 2.1`,
   `regex`, `roxmltree 0.20`, `serde*`, `sha2`, `texting_robots 0.2.2`,
   `thiserror`, `url` (`Cargo.toml:12-22`). Dropped with their modules:
   `anydoc`, `pdf-inspector`, `tinyvec`, `tracing-subscriber`, `strsim`.
2. `lib.rs` (`crates/firecrawl-native/src/lib.rs`): `pub use napi::…` deleted;
   `NativeError { reason }` + `Result<T, E = NativeError>` replace
   `napi::Result` (`lib.rs:23-42`), with `From<String>`/`From<&str>`/
   `From<url::ParseError>` so the `String` errors upstream already builds
   convert unchanged. Two `mod` + `pub use` pairs stay (`lib.rs:15-19`).
3. Per export: keep the private pure function, delete the `#[napi]` wrapper,
   rename — `_filter_links` → `filter_links` (`crawler.rs:280`),
   `_extract_metadata` → `extract_metadata` (`html.rs:226`), and so on.
4. `pub async fn` → `pub fn` throughout; nothing inside awaited.
5. `nodesig` (a git dependency) inlined into `html.rs` as `SignatureMode` +
   `get_node_signature` (`html.rs:18-131`) — **the one non-subtraction
   edit**, noted at `lib.rs:5`.
6. Dropped whole: `logging.rs` (bridged obscura-mcp's `tracing` registry),
   `pdf.rs` + `document.rs` (PDFs already go through `web`'s fetcher and
   `tools::doc`, 20 §2.3, `crates/web/src/fetch/mod.rs:283-301`),
   `engpicker.rs` (§0.3 picks by thinness instead), `utils.rs` (`lib.rs:6-8`).
7. Tests stay verbatim: `crawler.rs:810-1308` is a plain `#[cfg(test)] mod
   tests` (22 `#[test]`) asserting on `crawl_scope`, `filter_links`, path
   regexes and sitemap parsing — no Node, no network.
8. Clippy `-D warnings` clean with two crate-level allows (`lib.rs:10,13`:
   `missing_docs`, `collapsible_if`) rather than edits to vendored bodies.

Re-pull: upstream is a shallow clone at `~/Develop/misc/firecrawl`; refresh it,
`diff -ru` against `crates/firecrawl-native/src`, reapply steps 1–6 by hand. Keep
`Cargo.toml.upstream` beside `Cargo.toml` so dependency drift is visible.

## 3. What the crate hands us, and what we wrote

From `crawler.rs`: `crawl_scope` (`:140`) and `is_within_crawl_scope` (`:165`)
— root vs directory-seed scope, so `example.com/docs/guide.md` scopes to
`/docs/`; `build_robot` (`:264`) over `texting_robots`; `filter_links` (`:280`)
— dedupe, absolutise, depth cap, `includes`/`excludes` with
**character-class-aware** path regex (`compile_path_regex`, `:477`), robots
check per link, subdomain and external-link switches; `parse_sitemap_xml` /
`process_sitemap` (`:678`, `:736`) for `urlset` and `sitemapindex`.
From `html.rs`: `extract_base_href` (`:148`), `extract_links` (`:155`),
`extract_metadata` (`:226`), `transform_html` (`:476`, with
`TransformHtmlOptions` `:459`), `post_process_markdown` (`:955`: `<a>` text
newline split, escape `>` outside inline code, drop `](#…)`, strip *skip to
content* links).

What we wrote, `crates/scrape` (2,060 lines incl. tests):

| Piece | Where | Notes |
|---|---|---|
| fetch adapter | `engine.rs:94-125` | `Fetcher::new_with_timeouts` with an 8 MiB body cap and `page_timeout_ms` (`lib.rs:110-117`); `fetch_html` returns the raw body so `transform_html` sees the real DOM; a non-HTML body (already-converted PDF/Office markdown) skips straight to `post_process_markdown` |
| engine pick | `engine.rs` `fetch_page` | fetch → `is_thin` (prose gate, §0.3) → obscura; render failure keeps the thin fetch; `Fetched.tier` says which tier answered. Renders run on `RenderSlots` — a `Semaphore` over `scrape.concurrency` browser sessions (ids = `RENDER_SESSION` base + slot index) — so `search_web`'s five parallel reads and a crawl render concurrently; until 2026-09-17 one `render_lock` mutex serialised a single isolate (§12.15) |
| markdown pipeline | `engine.rs:298-373` | `transform_html` → `html-to-markdown-rs` → `strip_base64_images` (`](<base64-image-removed>)`, upstream `removeBase64Images`) → `post_process_markdown`; empty main-content retries whole-page |
| `normalize` | `crawl.rs:599-620` | visited key: fragment off, `www.` off, `/index.html` → `/`, `web::url::canonical_key` (tracking params, trailing slash), whole query off when `ignore_query_parameters` |
| scope | `crawl.rs:136-166` | one `FilterLinksCall` per batch: `base_url` = seed **origin**, `initial_url` = seed, `robots_user_agent: Some("wth")`, `max_depth: 100` (our own hop-depth cap is separate) |
| BFS walk | `crawl.rs:218-343` | frontier + visited + budget + hop depth, `buffered(concurrency)`, per-page timeout `min(page_timeout, remaining)`, `ScrapeProgress` after every batch change |
| robots | `engine.rs:163-176`, `crawl.rs:206-230` | fetched once per run (`{origin}/robots.txt`); `build_robot` for `Crawl-delay` and `Sitemap:` lines; `filter_links` applies the disallows |
| `HostGate` | `crawl.rs:643-671` | per-host slot reservation: N concurrent requests to one host go out `delay` apart, not together; `delay = max(delay_ms, Crawl-delay)` capped at 10 s (`host_delay`, `:547-555`) |
| deadline | `crawl.rs:101,259,285` | `CrawlOpts.deadline` shared across a `map_site` run's search / sitemap / walk phases |
| sitemap discovery | `crawl.rs:462-505` | candidates: the seed when it is `*.xml(.gz)`, robots `Sitemap:`, `{seed dir}/sitemap.xml`, `/sitemap.xml`, `/sitemap_index.xml`; `sitemapindex` recursed 3 deep; 5,000-URL cap; fetched raw, gzip inflated by the fetcher (`fetch/mod.rs:270-280`, `flate2`); results scoped through `filter_links` like any link |
| `<loc>` salvage | `crawl.rs:510-538` | a body `process_sitemap` rejects (truncated, odd namespaces) still yields what a plain `<loc>…</loc>` scan finds |
| `map_site` (`Scrape::map`) | `crawl.rs:349-416` | site-restricted `search_web` through the `web` actor first (`site_search`, `:420-456`), then sitemap URLs, then a link walk whose pages *and* outbound links count; titles only where a page was fetched |
| render provider | `provider.rs` | `web::Provider { name: "scrape", renders: true }`; `render` = forced browser, `read` = two-tier pick (§5) |
| tools + config + event | `tools.rs`, `lib.rs` | §6 |

## 4. Why the napi layer was not a problem

No `#[napi]` init macro, no `Env`, `JsObject` or `AsyncTask` anywhere in the
eight files; `napi::Result` and `napi::Error::new(Status::GenericFailure, String)`
were the only two touch points, and every error already carried the `String` we
need (`lib.rs:23-56` is the whole replacement). `crate-type` was the only line
that had to change for `cargo test` and `cargo doc` to work.

## 5. Search and render wiring

`scrape` registers a `web::Provider` named `scrape` with `renders: true` once
the `web` actor is up (`crates/scrape/src/provider.rs`). Two entry points
since 2026-09-17 (§12.15; 16 §5):

- `render` is `fetch_page(url, force_render: true, only_main_content: true)` —
  **always obscura**, `browser not available for rendered fetch` when there is
  none. `web`'s `Render` message calls it; `read_url {rendered: true}`
  (`crates/web/src/tools.rs`) is its consumer.
- `read` is `fetch_page(url, false, true)` — the two-tier pick, plain fetch
  first, obscura only when `is_thin` says the fetch was a shell (§0.3). It
  overrides `web::Provider::read`, whose default is `render`. `web`'s `Read`
  message calls it; `search_web`'s top-`read` hits (default 5) go through it
  in one call, with a plain markdown fetch as fallback when it fails.

Until that day there was one method, and it passed `force_render: false`: a
"rendered" read of a page whose plain fetch cleared the length gate never
opened the browser, so `read_url {rendered: true}` on rtings.com returned the
same nav shell as an unrendered read and only `scrape_url {rendered: true}`
(which calls `fetch_page` directly with the flag) ever rendered. Each `Fetched`
/ `Rendered` now carries `tier: "fetch" | "render"` so callers can see which
one answered; `search_web` shows it as ` · rendered` on the domain line.

`web`'s `Render` and `Read` both prefer this provider **by name**, then any
other renderer in registration order (`crates/web/src/actor.rs`); `browser`'s
`ObscuraRender` (`crates/browser/src/render.rs`) stays registered as the
fallback when `scrape` is disabled, and Firecrawl / OpenSERP remain optional
renderers behind `search_providers` (`crates/search-providers/src/firecrawl.rs`,
`openserp.rs`) — all three always report `tier: "render"`. With none:
`rendered fetch needs a rendering provider: scrape, browser, or firecrawl`
(`actor.rs`).

Alternative beaten: Firecrawl's engine order. Its `scrapeURL` runs engines by
quality — `chrome-cdp` at 50 above `fetch` at 5
(`apps/api/src/scraper/scrapeURL/engines/index.ts`) — so the browser goes
first and a shell can never be mistaken for a page; its per-engine success
check is just `markdown.trim().length > 0`. Rejected for latency: every
server-rendered page would pay a browser render, and a `search_web` five of
them in parallel. Kept fetch-first, and fixed the gate so the fallthrough
fires on a shell (§0.3).

Search: provider order is `web`'s built-ins (`engines/mod.rs:32-40`) → whatever
`search-providers` registers (`brave_api`, `firecrawl`, `searxng`, `openserp`,
each only when configured and keyed). Nothing requires their hosted API or a
SearXNG instance (§0.5). `map_site {search}` reuses the same aggregator with
`site: <seed host>` (`crawl.rs:434-444`).

## 6. Tools, config, event

All three: `Risk::ReadOnly`, `DIRECT: false` (§0.7-8), http(s) URLs only
(`tools.rs:48-54`). Defaults are the code's (`tools.rs`).

- `scrape_url {url, formats?: [markdown|html|links] (default [markdown]),
  only_main_content?: bool (true), max_chars?: u32 (100000), rendered?: bool
  (false)}` → `dict` (`tools.rs:78-94`).
- `crawl {url, limit?: u32 (scrape.max_pages), max_depth?: u32
  (scrape.max_depth; 0 = seed only), ignore_paths?: [regex], include_paths?:
  [regex], same_domain?: bool (scrape.same_domain), sitemap?: include|skip|only
  (include), crawl_entire_domain?: bool (false), allow_subdomains?: bool
  (false), regex_on_full_url?: bool (false), ignore_query_parameters?: bool
  (false), delay_ms?: u64 (scrape.delay_ms), only_main_content?: bool (true),
  format?: markdown|html|links (markdown)}` → `list[dict]` of `{url, title,
  markdown, tier[, html]}`, or `{url, title}` for `links`
  (`tools.rs:157-205,222-287`).
- `map_site {url, limit?: u32 (scrape.max_pages), search?: str,
  include_subdomains?: bool (true), ignore_query_parameters?: bool (true)}` →
  `list[dict]` of `{url, title?}`; results stay under the seed directory
  (`tools.rs:292-308`).

Prompt block (`tools.rs:18-22`, registered as the `scrape` plugin's
`RegisterPrompt`): when to reach for these over `read_url`.

`scrape:` config (`crates/scrape/src/lib.rs:52-88`; every field defaulted, the
section may be absent):

```yaml
scrape:
  enabled: true
  concurrency: 5          # concurrent page fetches in one crawl
  max_pages: 50           # default limit for crawl / map_site
  max_depth: 3            # default hop-depth cap for crawl
  same_domain: true
  page_timeout_ms: 8000
  run_timeout_ms: 60000   # hard deadline for one crawl / map_site run
  robots: true            # build_robot; disallows and Crawl-delay
  delay_ms: 100           # per-host floor between requests; a larger Crawl-delay wins (cap 10 s)
```

Event: `ScrapeProgress { session, call_id, inflight: [url], done: [url],
limit }` — since 2026-09-17 defined in `crates/web/src/progress.rs` (with
`ProgressCx` and `report`), not in `scrape`, so `search_web` publishes the same
event while it reads its top hits (§12.14); `crawl` publishes it after every
batch change (`crawl.rs`), forwarded by the RPC bridge (`crates/rpc/src/bridge.rs:101,136`)
and exported to TS (`crates/rpc/src/export.rs:97`). Surfaces: the desktop
`ScrapeCard` (`crates/desktop/ui/src/components/chat/ScrapeCard.tsx:69-81`:
stacked host avatars of in-flight URLs, `Crawling… n/limit`, expandable list of
`done`; letter avatars, no favicon fetch — §12.6) and the TUI spinner row
(`crates/tui/src/model.rs:5539`: `crawling… 3/50 · example.com`, `mapping…
120/500`, `reading example.com…`). `extract` is not built: one JSON-schema call
through `models` when wanted, no separate worker.

## 7. What we knowingly give up

Their `index` / `index;documents` engines answer from a pre-built site index —
ours is always cold on first visit. Their `fire-engine` is a hosted
TLS-fingerprint and proxy fleet; obscura's `stealth` covers most fronts, hard
Cloudflare challenges still fail more often. Their engine picker (text/HTML
ratio, status codes, `engpicker.rs`) was dropped; our thin-page heuristic
(`engine.rs:259-281`) is a `ponytail:` corner with that picker named as the
upgrade path. PDF table layout is whatever `anydoc` gives `web`'s fetcher
(20). Their `extract` quality rests on tuned prompts and model choice
(`transformers/llmExtract.ts`, 1,605 lines) — not built. None of these block
the docs-site case.

## 8. Licensing

Repo root `LICENSE` is AGPL-3.0 with a separate `LICENSE-MIT` scoped to the SDKs;
`apps/api/native/Cargo.toml` carries **no** license field, so the crate inherits
the repo's AGPL-3.0 — declared as such (`crates/firecrawl-native/Cargo.toml:7`).
That is why it is its own crate rather than code inside `scrape`: the boundary
stays legible and the file set is replaceable as a unit. The workspace is
`publish = false`; if a binary is ever distributed outside this machine, ship
this crate's source alongside it.

## 9. Effort

Draft estimate was two days; the build took one session plus a review pass
(§12). The vendoring itself (§2) was the cheap half, as predicted; the
expensive half was scope correctness — seed origin as `base_url`, sitemap
handling, and `normalize()` — found by review and unit tests, not by writing.

## 10. Alternatives considered

*Rewrite the logic in idiomatic Rust from reading their source* — rejected per §2:
the value is the tuned heuristics and the tests that pin them. *Keep the k8s
self-host as-is* — it works, and stays the reference implementation; it is six
pods and a network hop for what one binary can answer. *Skip the vendored crate
and build on `htmd` + our own crawler* — viable, but it re-derives the scope,
path-regex and link-filter rules that already ship tested, which is most of the
taste in this problem. *Keep their hosted API* — a dependency on someone else's
index and quota for something we can compute. *SearXNG as a required sidecar for
search* — see §0.5.

## 11. Status (as-built)

Verified by: unit/wiremock tests (`cargo test --workspace`), clippy
`-D warnings`, `bun run check && bun run test` in the desktop UI, and the live
smoke below (2026-09-17, author's network, debug build).

- `cargo test -p firecrawl-native`: 22 upstream tests (`crawler.rs:810-1308`):
  crawl scope from root and directory seeds, `filter_links` include/exclude,
  character-class path regex (`/\d{4}-\d{2}-\d{2}/` archive case), sitemap
  `urlset` / `sitemapindex` parsing.
- `cargo test -p scrape`: `crawl.rs` — six wiremock crawls
  (`crawl_honours_limit_and_depth`, `crawl_exclude_paths_skips_date_archive`,
  `deep_seed_stays_under_directory`, `crawl_honours_robots_disallow`,
  `sitemap_only_follows_index_and_keeps_scope`,
  `ignore_query_parameters_collapses_variants`) plus `normalize` and the
  truncated-XML `<loc>` salvage (`crawl.rs:731-966`); `engine.rs` — six unit
  tests on `is_thin`, base64 strip, markdown links, `to_markdown`
  (`engine.rs:425-509`); `lib.rs` — config defaults.
- `cargo test -p web`: `tests/engines.rs` — Google WML happy path (inline) and
  `sorry` redirect → `Blocked` (captured fixture); Brave rows + dates +
  `related` from the 2026-09-17 capture; the 2026-09-17 Bing capture is a
  **decoy SERP** (HTTP 200, ten rows for an unrelated query) and drives
  `reject_decoys` → `Blocked`; Qwant JSON with SearXNG's parameter order,
  datadome cookie replay, captcha → `Blocked`, recency → `Unsupported`;
  `tests/aggregate.rs` — RRF, feature dedupe, `Unsupported` skip, zero-yield
  detector.
- clippy `-D warnings` across the workspace.
- Live smoke, fetch tier (throwaway `cargo run --example`, deleted after):
  - `scrape_url https://docs.rs/tokio/latest/tokio/` → title `tokio - Rust`,
    22 670 chars of markdown (27 423 before the converter's nav heuristics
    joined `transform_html` under `only_main_content`), 190 links, 0.37 s.
  - `crawl` from that seed, `limit 12, max_depth 2` → 12 pages in 4.6 s, all
    under `/tokio/latest/tokio/`; the first run returned 1 page and exposed
    that duplicate outbound links (three back to the seed) were consuming
    `filter_links`' `limit` slots — fixed by deduping/dropping visited links
    before scoping (§12.9).
  - `crawl https://tokio.rs/tokio/tutorial`, `limit 12, max_depth 2` → 8 of
    the 11 tutorial pages in 2.1 s (`/tokio/topics`, `/blog/...` denied as
    `BACKWARD_CRAWLING`, external links as `EXTERNAL_LINK`).
  - `map_site https://tokio.rs/tokio/tutorial`, `limit 15` → 11 URLs with
    titles in 2.3 s; `map_site` on docs.rs → 2 URLs (`/` and `all.html`) in
    3.1 s (docs.rs has no sitemap; the walk found the rest but the seed page
    links exceeded the budget).
  - Headless `wth --headless --data-dir /tmp/…` over JSON-RPC: `list_tools`
    shows `scrape_url`/`crawl`/`map_site` as `direct: false, risk: read_only`
    and no `map`; `get_prompt` carries the scrape block; `read_url`'s
    description names `scrape` as the renderer. The `search_web` description
    listed `scrape ()`/`browser ()` (render-only providers with no category)
    — fixed to list searching providers only.
- Live smoke, SERP (`tokio async runtime rust`, five general engines): brave
  5 hits + `related` 0.35 s; duckduckgo 5 hits 0.87 s; google `Blocked (HTTP
  429 sorry)`; qwant `Blocked (captcha)`; bing `Blocked (decoy results)` —
  before `reject_decoys`, Bing returned five confidently unrelated hits
  (LinkedIn, zhihu) with HTTP 200 and the right `<title>`, reproducible with
  `curl` (16 §10).
- Live smoke, read tier (2026-09-17, an agent using `search_web best
  mechanical keyboard 2026` through a full harness turn): rtings.com came
  back as a navigation shell and is the live case the prose gate (§0.3) is
  measured against — its plain fetch is thousands of chars of nav/product
  markdown with < 800 chars of prose, so `is_thin` must now say thin and the
  read must show ` · rendered`.
  Live smoke 2026-09-17 (throwaway example with the `browser` actor spawned,
  debug build): `Read` on rtings.com logs `fetch thin; trying obscura`, comes
  back `tier=render`, 28 188 chars, **15 761 prose chars** (the shell had
  ~150); `Render` on the same URL is identical, so `read_url {rendered:
  true}` now renders. `search_web best mechanical keyboard 2026` one call:
  21-24 s, 5 pages read, rtings.com marked `· rendered` with the pick list
  ("Keychron V5 Max is our new mid-range pick", "Aula F99/F75 as our budget
  pick"), PCMag/Wirecutter `fetch`; Banco Best/Bestway rows shown but not
  read. Obscura settles rtings.com in 19.9-20.7 s under `load`,
  `domcontentloaded` and `networkidle0` alike - the per-page floor for a
  heavy client-rendered page and the reason `web.read_timeout_ms` is 25 000;
  the settle budget is an obscura knob, not ours. Wirecutter's compact
  keyboards page read fine from this network (41 400 prose chars), so the
  bot wall the agent hit is network-dependent; it now surfaces as `(could not
  read: no readable text …)` by construction (prose < 100).
- Not exercised: a `crawl`/`map_site` call from the Python runtime with a
  real model (scratch data dir has no provider key), gzip sitemaps against a
  real site, the desktop card and TUI progress row on a live run (unit tests
  only), `map_site search=` through the aggregator.

## 12. Revisions at build time

Review findings that changed the design between draft and HEAD:

1. **Sitemap XML never goes through the HTML converter.** The first cut fed
   sitemap bodies through `Fetcher::fetch(Markdown)`, which turned `<loc>` into
   prose. Now `fetch_html` returns the raw body (`fetch/mod.rs:164-170,306`),
   `process_sitemap` parses it, and a parse failure falls back to a `<loc>`
   scan (`crawl.rs:510-538`). Gzip containers (`sitemap.xml.gz`) are inflated
   in the fetcher (`fetch/mod.rs:270-280`; `flate2`).
2. **`base_url` is the seed origin, `initial_url` the seed.** Passing the seed
   as both made `filter_links` compute scope from the wrong base and drop
   sibling pages. Fixed at `crawl.rs:147-148`; `deep_seed_stays_under_directory`
   pins it.
3. **Robots user agent is `wth`.** `build_robot` and `filter_links` were called
   with `None`, matching only `User-agent: *`; now `Some("wth")`
   (`crawl.rs:20,155,228`), so a site can allow or deny us by name.
4. **`map` → `map_site`** (§0.9).
5. **No `web` config read from `scrape`.** The draft's engine read
   `rt.section("web")` for fetch limits; `plugins.md` §3.6 forbids reading
   another plugin's section. `scrape` builds its own `Fetcher` from
   `WebConfig::default()` constants and its own `page_timeout_ms`
   (`lib.rs:110-117`), and looks the browser actor up at call time
   (`engine.rs:202-206`).
6. **Favicon egress removed from the desktop card.** The first `ScrapeCard`
   fetched a favicon from a third-party lookup service for every crawled
   host, leaking the crawl target list; replaced with letter avatars, no
   network (`ScrapeCard.tsx:13-16`).
7. **`delay_ms`** added to `scrape:` (default 100) after review of the
   HostGate: a politeness floor independent of `Crawl-delay`, tool-overridable
   (`lib.rs:69-71`, `tools.rs:194-197`).
8. **SearXNG demoted to opt-in** (§0.5): the draft's "first provider" role went
   to the in-tree keyless engines; `SearchProvidersConfig::default()` has
   `searxng: None` (`crates/search-providers/src/lib.rs:40`).
9. **Outbound links are deduped and stripped of visited URLs before
   `filter_links`.** Found in live smoke: docs.rs's seed page links back to
   itself three times; `filter_links` counts every survivor against its
   `limit`, so those duplicates ate the remaining budget and a `limit 6`
   crawl returned one page. `walk` now dedupes by `normalize` and drops
   visited keys first (`crawl.rs`, the `seen_here` block); 12/12 pages after.
10. **The converter's navigation heuristics run under `only_main_content`.**
   `transform_html`'s fixed selector list misses docs.rs's `nav-container`
   header; `html-to-markdown-rs`'s `remove_navigation`/`remove_forms`
   (role/aria-label/class-token based) is switched on only in that mode, so
   `only_main_content: false` still returns the whole page (`engine.rs`
   `html_to_markdown(html, strip_chrome)`). 27 423 → 22 670 chars on the
   tokio docs index.
11. **Decoy SERPs are `Blocked`.** Bing served this network HTTP 200 pages
   whose ten rows belonged to another query; `engines::reject_decoys` fails a
   non-empty SERP that shares no query word with any hit (16 §10). Applied to
   google/bing/brave/qwant.
12. **`duckduckgo` is back in the default engine set** — with three of five
   general engines walled from one network, the two that answered were Brave
   and DuckDuckGo. Eight defaults: `google, bing, brave, qwant, duckduckgo,
   wikipedia, arxiv, github` (`web/src/engines/mod.rs` `BUILTIN`).
13. **`search_web`'s description lists searching providers only** — render-only
   `scrape`/`browser` (empty `categories`) showed up as `scrape ()` in the
   model-facing text (`web/src/tools.rs` `search_description`).
14. **Progress event moved to `web`** (2026-09-17). `ScrapeProgress`,
   `ProgressCx` and `report` live in `crates/web/src/progress.rs`; `scrape`
   imports them. `search_web` publishes the event with `limit` = number of
   hits it reads, so the desktop `ScrapeCard` and the TUI row show `reading
   n/N` for a search exactly as for a crawl. Alternative beaten: a second
   event type per tool — one card in every frontend needs one event.
15. **`read` vs `render`, a prose gate, parallel isolates** (2026-09-17).
   Field report from an agent using the tools (16 §13.17): `search_web best
   mechanical keyboard 2026` read rtings.com as a nav shell. Three causes in
   this crate. (a) `is_thin` (`engine.rs`) accepted any fetch ≥ 800 chars of
   markdown, and a server-rendered nav + product list is thousands of chars
   around a client-rendered article; now prose-based —
   `web::excerpt::prose_chars(markdown) < 800`, or `< 2500` with an SPA root
   marker in the first 4 KiB (§0.3). Alternative beaten: upstream's
   engine-picker heuristics (text/HTML ratio, status codes) — a ratio still
   counts link text as text, which is exactly what a nav shell is made of;
   the §4.7 paragraph filter already knows what is not prose. (b)
   `ScrapeProvider::render` passed `force_render: false`, so
   `read_url {rendered: true}` and `search_web`'s reads never used the browser
   — only `scrape_url {rendered: true}` did. Now `render` forces it and a
   separate `read` (overriding `web::Provider::read`) is the two-tier pick;
   `web` calls `Read` from `search_web` and `Render` from `read_url` (§5).
   Alternative beaten: Firecrawl's browser-first order (§5) — right by
   construction, five renders per search. (c) One `render_lock` mutex
   serialised every render on one isolate, so five thin reads in a search
   would have rendered one after another under a 8 s deadline. Replaced by
   `RenderSlots`: a `tokio::sync::Semaphore` over `scrape.concurrency`
   sessions with a free list of `SessionId`s derived from the `RENDER_SESSION`
   base; `call()` takes the session as a parameter (`engine.rs`). `Fetched`
   gained `tier` (`"fetch"` | `"render"`) and `ScrapeProvider` copies it into
   `web::Rendered.tier` (16 §5). `web.read_timeout_ms` default 8000 → 25000 (obscura settles rtings.com in ~20s, 16 §13.17)
   so a render after a thin fetch fits. Status: §11, read tier.
