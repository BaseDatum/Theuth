# 38 — Scrape: a self-contained crawler from a vendored `firecrawl_rs`

Status: draft v0 · 2026-09-16 · extends `16-web.md` §5 (rendered fetch) and
**supersedes its decision 5** for the local path, `23-browser.md` §3 (obscura
isolates), `26-search.md` (aggregator), `00-architecture.md` §1 (single binary),
§4 (tools), §6 (risk). Measured against `~/Develop/misc/firecrawl` at
`c563a66` and this tree today. Not yet built; §11 is the verification plan.

## 0. Decisions in one screen

1. **Copy the crate, then edit the copy.** `cp -R` of Firecrawl's
   `apps/api/native/src` into `crates/firecrawl-native/src`, then strip the napi
   layer in place. This is *not* a rewrite and not a regeneration from reading
   their code — the 3,090 lines in `src/` are the deliverable, the edits are subtraction
   only (§2). Their heuristic tuning and the 490 lines of tests that encode it
   are the thing we want; anything re-typed loses it.
2. **Two new crates, one direction.** `firecrawl-native` (vendored, AGPL — §8)
   holds pure logic; `scrape` holds the orchestrator and the tools.
   `scrape → {firecrawl-native, web, browser}`; `browser` already depends on
   `web`, so there is no cycle. Nothing in `agent` changes.
3. **Two-tier engine, not a ladder.** `web::Fetch` first (plain HTTP, already
   SSRF-guarded per hop, `crates/web/src/fetch/mod.rs:91-107`), obscura second
   for JS-dependent pages (`crates/browser/src/render.rs:102` already does
   `browser_navigate → browser_markdown → browser_close` on a warm isolate).
   Their 16-engine ladder (`apps/api/src/scraper/scrapeURL/engines/index.ts:51-67`)
   is a hosted-service dispatcher and does not port; obscura's `stealth`
   feature is our answer to their `fire-engine;tlsclient;stealth`.
4. **One converter per path, one shared post-processor.** Fetch path keeps
   `html-to-markdown-rs` (already a `web` dependency); render path uses
   obscura's own `browser_markdown`; both then run `post_process_markdown` from
   the vendored crate (`html.rs:962`). Never chain two converters. Diff the
   result against today's `read_url` output on the fixture set (§11) before
   making it the default.
5. **Search stops depending on their API.** `search-providers/src/searxng.rs`
   (keyless, and SearXNG is already in the cluster) becomes the first provider;
   `brave`/`tavily`/`openserp` stay as configured; `search-providers/src/firecrawl.rs`
   stays registered but is no longer required by anything. Their `/v2/scrape` is
   replaced locally, `/v2/search` by SearXNG + the existing RRF aggregator
   (`crates/web/src/aggregate.rs`, 439 lines). That is the whole "no API needed"
   claim, and it is a provider-order change plus the new crate.
6. **No queue, no Postgres, no Redis.** One process: `tokio` buffered
   concurrency over a BFS frontier, in-memory visited set, per-run page budget.
   Their fleet (nuq worker, prefetch worker, RabbitMQ, pg_cron reaper — the six
   pods in `~/Develop/skymoore/k8s/firecrawl`) exists to hand work between
   processes; we have one.
7. **Risk `ReadOnly`** for `scrape_url` and `map`; `crawl` is `ReadOnly` too but
   bounded by config defaults (§6) so a stray call cannot fan out across a whole
   domain unbounded. The per-hop SSRF re-check is not configurable off.

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
`apps/api/native` is 3,093 lines of logic, already Rust, already tested.

## 2. Vendoring procedure (the point of this doc)

Copy first, then edit. Do not retype the module from a reading of it, and do not
ask a model to regenerate it: `html.rs` is accumulated output-shaping tuning and
`crawler.rs` carries 490 lines of tests that pin those expectations, so a fresh
generation silently diverges from what their self-host actually emits.

```
mkdir -p crates/firecrawl-native/src
cp -R ~/Develop/misc/firecrawl/apps/api/native/src/. crates/firecrawl-native/src/
cp ~/Develop/misc/firecrawl/apps/api/native/Cargo.toml crates/firecrawl-native/Cargo.toml.upstream
```

Then, in the copy only:

1. `Cargo.toml`: `crate-type = ["lib"]` (upstream `["cdylib"]`,
   `Cargo.toml:9` — the only structural change). Drop `napi`, `napi-derive`
   from `[dependencies]`; delete `build.rs` (`napi_build::setup()` is its whole
   body). Keep `tinyvec = ">=1.12, <1.13"` **only** while `pdf.rs` is in the
   build: upstream commits no `Cargo.lock`, and
   `pdf-inspector → unicode-normalization → tinyvec 1.13` fails to compile.
2. `lib.rs` (20 lines): delete `pub use napi::bindgen_prelude::*;` (`lib.rs:19`)
   and add `pub use serde::{Deserialize, Serialize};` if not already there. The
   seven `mod` + `pub use` pairs stay.
3. Replace `napi::Result<T>` with a local `pub type Result<T> =
   std::result::Result<T, NativeError>;` (`thiserror`, one variant carrying the
   `String` they already build). Then per export: keep the private pure
   function, delete the wrapper, rename. The pattern is mechanical and identical
   in all 40 places — e.g. keep `fn _extract_metadata(...) ->
   Result<HashMap<String,Value>, ()>` (`html.rs:144`), delete `#[napi] pub async
   fn extract_metadata(...)` (`html.rs:317`), rename `_extract_metadata` →
   `extract_metadata`. Same for `html.rs:38/49`, `64`, `415/661`, `674/680`,
   `714/764`, `780/947`, `962`; `crawler.rs:294/461`, `533/707`, `720/778`,
   `796/871`; `engpicker.rs:90/67`. 40 attributes total (crawler 17, html 12,
   engpicker 5, pdf 3, logging 2, document 1), ≈250–300 lines of the boilerplate
   is those wrappers.
4. `pub async fn` → `pub fn` where the body is sync (all of `html.rs`,
   `crawler.rs`, `engpicker.rs`); nothing inside awaits. Keep `async` only where
   it genuinely does.
5. `nodesig`: one import (`html.rs:7`, `get_node_signature(html, idx,
   SignatureMode::TextDepthClass)`), used as the dedupe key in `extract_attributes`'
   `seen` hash set. Inline ~40 lines (tag + depth + class list + first 64 chars
   of text) and drop the git dependency.
6. `logging.rs` (267 lines, bridges obscura-mcp's `tracing` registry): replace
   with direct `tracing` calls, or drop for the first cut.
7. Tests stay verbatim. `crawler.rs:883-1373` is a plain `#[cfg(test)] mod tests`
   (23 `#[test]`) calling `crawl_scope(&Url::parse(url).unwrap())` and asserting
   on `scope.prefix`/`scope.exact` — it runs under `cargo test` with no Node and
   no network. Those are the expectations we must keep passing.

Re-pull: upstream is a shallow clone at `~/Develop/misc/firecrawl`; refresh it,
`diff -ru` against `crates/firecrawl-native/src`, reapply steps 1–6 by hand. Keep
`Cargo.toml.upstream` beside `Cargo.toml` so dependency drift is visible.

Per file, what survives: `crawler.rs` 1,373 (883 logic + 490 tests) and `html.rs`
1,041 are the core and ship whole; `engpicker.rs` 222 ships (engine verdict
helper, useful for the two-tier pick); `pdf.rs` 143 and `document.rs` 19 ship
only if we want PDF/DOCX (they pull `pdf-inspector` and `anydoc`); `logging.rs`
267 and `utils.rs` 5 are replaced.

## 3. What the crate hands us, and what we write

From `crawler.rs`: `crawl_scope` (`:155`) and `is_within_crawl_scope` (`:180`) —
root vs directory-seed scope, so `example.com/docs/guide.md` scopes to `/docs/`;
`get_url_depth` (`:185`); `is_internal_link` (`:193`), `is_subdomain` (`:253`);
`is_non_web_protocol` (`:223`), `is_social_media_or_email` (`:234`),
`no_sections` (`:209`); `_filter_links` (`:294`) — dedupe, absolutize, depth cap,
`include_paths`/`exclude_paths` with **character-class-aware** path regex
(`compile_path_regex`, `:506`) and `validate_regexes` (`:520`); `_filter_url`
(`:533`); `build_robot` (`:278`) over `texting_robots`; `parse_sitemap_xml` /
`process_sitemap` (`:720`, `:796`) for `urlset` and `sitemapindex`.
From `html.rs`: `extract_links` (`:64`), `extract_metadata` (`:144`),
`extract_images` (`:780`), `extract_attributes` (`:714`), `transform_html`
(`:415`), `get_inner_json` (`:674`), `extract_base_href` (`:38`),
`post_process_markdown` (`:962`: `<a>` text newline split, escape `>` outside
inline code, drop `](#…)`, strip *skip to content* links).

What we write, ~600 lines total in `crates/scrape`:

| Piece | Notes |
|---|---|
| `fetch` adapter | reuse `web::Fetch`; add a constructor with a large `max_chars` — today's defaults are 8 s total / 2 s connect and a tool-sized truncation (`fetch/mod.rs:91-107`) |
| absolutize + normalise | `crawler.rs:445` `absolutize_link` does it; feeding `baseUrl` and stripping fragments is ours |
| BFS orchestrator | frontier + visited set + page budget + depth, `StreamExt::buffered(n)`, per-domain politeness delay, per-page timeout with a hard run deadline |
| engine pick | fetch → inspect (empty body / SPA marker / `rendered: true`) → obscura render; log which tier answered |
| `robots` | `build_robot` + one `texting_robots` check per host per run |
| tools + config | §6 |

## 4. Why the napi layer is not a problem

No `#[napi]` init macro, no `Env`, `JsObject` or `AsyncTask` anywhere in the
eight files; `napi::Result` and `napi::Error::new(Status::GenericFailure, String)`
are the only two touch points, and every error already carries the `String` we
need. `crate-type` is the only line that must change for `cargo test` and
`cargo doc` to work. Nothing checks for a Node runtime.

## 5. Search and render wiring

`web::Provider::scrape_render` (`crates/web/src/provider.rs:204` `Rendered`,
`:284`) is currently satisfied by a remote provider. Register a local provider
implemented on `scrape` so `read_url {rendered: true}` and `search_web
{fetch_top: n}` resolve in-process; the remote `firecrawl` provider stays behind
its config key as an optional second provider in the RRF mix. Provider order:
`searxng` (in-cluster, no key) → built-ins (`duckduckgo`, `wikipedia`, `arxiv`,
`github`) → keyed providers. No behaviour today requires their hosted API.

## 6. Tools and config

`scrape_url {url, formats:[markdown|html|links], only_main_content?, max_chars?,
rendered?}` · `crawl {url, limit?, max_depth?, ignore_paths?, include_paths?,
same_domain?, only_sitemap?, format}` · `map {url, limit?}` (discovery only:
sitemap-first, then link walk, returns URLs + titles). Defaults in a `scrape:`
config section: `concurrency: 5`, `max_pages: 50`, `max_depth: 3`,
`same_domain: true`, `page_timeout_ms: 8000`, `run_timeout_ms: 60000`,
`robots: true`. `extract` is a later addition: one JSON-schema call through
`models`, no separate worker.

## 7. What we knowingly give up

Their `index` / `index;documents` engines answer from a pre-built site index —
ours is always cold on first visit. Their `fire-engine` is a hosted TLS-fingerprint
and proxy fleet; obscura's `stealth` covers most fronts, hard Cloudflare challenges
still fail more often. PDF table layout (`pdf.rs` + `pdf-inspector`) is coarse.
Their `extract` quality rests on tuned prompts and model choice
(`transformers/llmExtract.ts`, 1,605 lines) — we start with a plain schema call
and tune against our own gateway. None of these block the docs-site case.

## 8. Licensing

Repo root `LICENSE` is AGPL-3.0 with a separate `LICENSE-MIT` scoped to the SDKs;
`apps/api/native/Cargo.toml` carries **no** license field, so the crate inherits
the repo's AGPL-3.0. That is why it is its own crate rather than code inside
`scrape`: the boundary stays legible and the file set is replaceable as a unit.
The workspace is `publish = false`; if a binary is ever distributed outside this
machine, ship this crate's source alongside it.

## 9. Effort

½ day steps 1–7 in §2 until `cargo test` is green in `firecrawl-native` (their 26
tests plus ours). ½ day for `scrape_url`. One day for `crawl` + `map` with the
budget and scope rules. Then the only open-ended part: the fidelity pass in §11.

## 10. Alternatives considered

*Rewrite the logic in idiomatic Rust from reading their source* — rejected per §2:
the value is the tuned heuristics and the tests that pin them. *Keep the k8s
self-host as-is* — it works, and stays the reference implementation; it is six
pods and a network hop for what one binary can answer. *Skip the vendored crate
and build on `htmd` + our own crawler* — viable, but it re-derives the scope,
path-regex and link-filter rules that already ship tested, which is most of the
taste in this problem. *Keep their hosted API* — a dependency on someone else's
index and quota for something we can compute.

## 11. Verification plan

Fixture set of ~15 pages: this docs site (directory-seed scope), a sitemap-only
site, a JS-rendered SPA, a site with a `/blog/YYYY-MM-DD/` archive that must not
be swallowed by a naive `/\d{4}-\d{2}-\d{2}/` regex (`compile_path_regex`'s
character-class rule), one PDF, one Cloudflare-fronted page, one with
`Skip to content` links and one with nested `<a>` text. Assert: markdown equals or
beats today's `read_url` output on each; `crawl` respects `limit`, `max_depth`,
`exclude_paths`; scope from a deep seed stays under its directory; `robots.txt`
disallows are honoured; the render tier fires only when the fetch tier came back
thin. Cross-check against the running self-host for the same URLs. Also:
`cargo test -p firecrawl-native` (their 22 + 4 tests) and clippy `-D warnings`.
