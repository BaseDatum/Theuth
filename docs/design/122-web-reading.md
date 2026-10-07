# 122 — Web reading: fetch diagnosis, retries, charset, extraction scoring, compaction, honest crawl

Status: design v0 · 2026-10-07 · extends 16 (web: `search_web`/`read_url`/`http_request`), 38 (scrape:
`scrape_url`/`crawl`/`map_site`, vendored Firecrawl), 20 (documents), 23 (browser tier), 44 (routes).
Sources: `~/.theuth/briefs/2026-10-07/webclaw-vs-theuth-web.md` (the ranked borrow list B1–B14),
`recon-theuth-web-report.md` (survey of our stack) and `recon-webclaw-report.md` (study of
0xMassi/webclaw `3c32041`, v0.6.24). Every `file:line` in this document was re-read on
`~/Develop/theuth-122-web` at `ebac3603` (2026-10-07); where a survey pointer had drifted or was
imprecise the corrected statement is used here and noted in §2.3. Waves write their as-built notes
as `docs/design/122-as-built-<TAG>.md`. Calls the brief left open are marked `(call)` and listed in §9.

Sky's words (2026-10-07 00:21), in answer to "what can we learn from webclaw": *"New git worktree,
design doc for all waves, then orchestrated parallel implementation with omp agents."*

> **License rule.** webclaw is AGPL-3.0. This document describes *techniques* in our own words with our
> own types and constants; no line of webclaw code was read into this worktree and none may be. The
> only AGPL code in the tree stays `crates/firecrawl-native` (38 §8).

## 0. Summary

What changes for an agent that reads a page:

1. A wall is named, not served. `read_url` of a Cloudflare interstitial, a login page, a consent wall
   or an SPA shell starts with `ISSUE: access_challenge (…) — next: rendered: true | browser_import_cookies | desktop_handoff`
   instead of `HTTP 403` or 3 000 chars of cookie prose (D1).
2. A transient 429/5xx or a connection reset is retried once under one 16 s budget; `Retry-After` is
   honoured when it fits, surfaced when it does not (D2). A Shift_JIS page decodes (D3).
3. Honest UA first, `Accept`/`Accept-Language` always; one browser-realistic header attempt only after
   a diagnosed block, then the existing render tier (D4). No TLS fingerprinting.
4. `published · by · lang` on the second line, `meta` in the structured half, `canonical` resolved (D5).
5. Truncation is pageable: `offset`, a cut at a block boundary, `[… N chars omitted; continue with
   offset=M]` and the headings of what was omitted (D6). `scrape_url` and `crawl` stop clipping silently.
6. Repeated carousels, Tailwind class leaks, logo rows and invisible characters are removed *before*
   the cap, so 4 000 chars carry more page (D7).
7. Main content is chosen by a scored candidate when no `<main>`/`<article>` exists, with two pinned
   invariants and safety valves on the strip list; `only_main_content` applies on the render tier (D8).
8. A JSON data island is read before the browser is paid for (D9); `crawl` says `incomplete` and why,
   lists skipped pages and invalid patterns; `map_site` returns the unfetched frontier (D10).
9. Eight hosts agents read most (HN, Reddit, npm, PyPI, crates.io, arXiv, Wikipedia, Stack Overflow)
   are read from their JSON endpoint, `tier: "api"` (D11). Rails and the SSRF denylist widen (D12).
10. A synthesised 16-page fixture corpus and contract tests pin every marker, invariant and golden (D13).

Posture in one sentence: **honest fetch, diagnosed failures, truthful truncation** — theuth keeps its
identity on the wire, tells the model exactly why a page did not read and which rung comes next, and
never returns a cut page as if it were whole.

## 1. Why now

The 2026-10-06 comparison (`webclaw-vs-theuth-web.md` §1–§2) read both stacks along the same 15 axes.
Five findings carry this design:

- **Blocked pages.** webclaw classifies block screens with distilled markers (`cf-mitigated`,
  `_cf_chl_opt`, `h/{b,g}/orchestrate/`, size-gated captcha widgets) and machine codes; theuth has
  only prose-volume gates (`MIN_PROSE` 100, `is_thin` 800/2500) and returns `HTTP 403` as a bare string
  (`crates/web/src/fetch/mod.rs:227-229`) or a consent wall as content when it exceeds 800 chars of
  prose (`crates/scrape/src/engine.rs:334-352`). Yet theuth owns the escalation rungs webclaw lacks —
  stealth render, `browser_import_cookies`, `desktop_handoff` — and never names them to the model.
- **Retries and budget.** webclaw: two attempts under `2 × timeout`; theuth: "no retries of any kind"
  (`crates/web/src/http.rs:13`, one attempt per hop at `fetch/mod.rs:190-234`).
- **Extraction.** webclaw scores candidates and guards noise removal with a 5 000-char safety valve and
  two contract-tested invariants; theuth deletes 42 selectors (`crates/firecrawl-native/src/html.rs:397-440`)
  and ignores `only_main_content` on the render tier (`engine.rs:190-225`). theuth is ahead on passage
  selection (`crates/web/src/excerpt.rs`) and politeness (robots, `Crawl-delay`, `HostGate`).
- **Telling the truth.** webclaw has no output caps at all; theuth caps everywhere but `scrape_url`
  clips silently (`crates/scrape/src/tools.rs:124`, `:352-357`), `crawl` logs skips at debug only
  (`crates/scrape/src/crawl.rs:298-310`) and `extract_metadata` computes a dozen fields and surfaces
  `title` (`engine.rs:354-363`).
- **Two gaps neither has.** Charset decoding: both call `from_utf8_lossy` (ours at `fetch/mod.rs:434`).
  A fixture corpus: webclaw has six real pages, theuth one 789-byte file (`crates/web/tests/fixtures/fetch_sample.html`).

What is deliberately **not** borrowed (comparison §4): the wreq/BoringSSL TLS fingerprint stack, a
browser UA by default, ignoring robots.txt, hosted-API escalation, thirty vertical extractors,
LLM `extract`/`summarize` in the fetch tool. §8 lists the follow-ups.

## 2. Current state

### 2.1 Tool by tool

| tool | crate · entry | what happens today | gap |
|---|---|---|---|
| `read_url` | `web` · `FetchTool::call` (`crates/web/src/tools.rs:501-546`) | `max_chars` 200..=16000 (`:502-510`); `Fetcher::fetch` → `fetch_kind` (`fetch/mod.rs:190-234`): static guard per hop, one GET, 301/302/303/307/308 followed (`:330-339`), non-2xx → `Failed("HTTP n")` (`:227-229`); `read_body` (`:375-461`): `is_binary` refusal (`:484-497`), `read_capped` 2 MiB (`:342-373`), gzip-magic inflate (`:403-412`), anydoc sniff (`:415-433`), `from_utf8_lossy` (`:434`), `html_to_markdown` with `EXCLUDE` + Aggressive preset (`fetch/text.rs:10-37`), `markdown_to_text` (`:45-73`), `hard_truncate` (`:151-158`); text `# title\nurl\n\ncontent\n[truncated at N chars]` (`render.rs:178-189`) | no diagnosis, no retry, no charset, title-only metadata, head-only cut, relative links kept, no compaction |
| `search_web` reads | `web` · `read_page` (`tools.rs:220-295`) | `Read` through the `scrape` provider under `render_cap_ms`; provider error → raw fetch under the remaining budget; prose `< MIN_PROSE = 100` → `error: "no readable text (JavaScript-only page or bot wall)"` (`:205-208`, `:267-275`); `ReadPage { url, title, excerpts, error, skipped, took_ms, tier }` (`aggregate.rs:55-73`) | a wall is "no readable text" or a successful excerpt of cookie prose |
| `http_request` | `web` · `http.rs` | `Risk::Mutating` for every method (`http.rs:263`), 1 MiB / 30 s, no retries (`:13`) | kept as is (non-idempotent); risk class is a follow-up (§8) |
| `scrape_url` | `scrape` · `ScrapeUrlTool::call` (`crates/scrape/src/tools.rs:111-152`) → `Scrape::scrape_url` (`crawl.rs:171-183`) → `fetch_page` (`engine.rs:114-146`) | fetch tier via `fetcher.fetch_html` (`engine.rs:150-181`, same decode path as `read_url`), `is_thin` → render (`:334-352`), render failure keeps the thin page silently (`:135-138`); `to_markdown` = `transform_html` + h2m + `post_process_markdown` (`:369-387`), empty main → whole page (`:383-385`); output `json!({url, title, tier, markdown[, html, links]})` clipped by `clip` at 100 000 with no marker (`tools.rs:124`, `:134`, `:352-357`) | no `ScrapeResponse` type, silent clip, `only_main_content` ignored on render (`:190-225`), metadata dropped (`:354-363`) |
| `crawl` | `scrape` · `CrawlTool::call` (`tools.rs:222-287`) → `Scrape::crawl` | BFS in batches of `concurrency`; a failed or timed-out page is `tracing::debug!` and skipped (`crawl.rs:298-310`); the run deadline cut leaves no flag; invalid path regexes dropped by `filter_links` (`crates/firecrawl-native/src/crawler.rs:294-303`) though `validate_regexes` exists (`:490-501`); rows `[{url, title, markdown, tier[, html]}]`, no per-page cap (`PAGE_CHARS` 500 000, `engine.rs:14`) | the model cannot tell a finished crawl from a cut one |
| `map_site` | `scrape` · `MapTool::call` (`tools.rs:325-349`) → `Scrape::map` (`crawl.rs:355-…`) | search hits, sitemap URLs, a short walk; rows `{url, title}`; the sitemap walk already checks its deadline between fetches (`crawl.rs:492-495`) | the walk's unfetched frontier is thrown away; rows carry no provenance |
| `firecrawl-native` | `transform_html` (`html.rs:476-714`) | detaches `head/meta/noscript/style/script` (`:502-516`), `exclude_tags`, the 42-selector list unless a `FORCE_INCLUDE_MAIN_TAGS` match (`:568-584`), lazy `srcset`/`data-src` (`:586-676`), absolutises `img[src]`/`a[href]` (`:678-711`) | no scoring, no safety valve, `<noscript>` lost before anything can recover it |

### 2.2 Constants that matter here (verified)

| constant | value | where |
|---|---|---|
| `fetch::TIMEOUT` / `CONNECT_TIMEOUT` | 8 s / 2 s | `crates/web/src/fetch/mod.rs:23`, `:25` |
| `web.max_fetch_bytes` / `max_redirects` / `user_agent` | 2 097 152 / 5 / `theuth/0.1 (+https://github.com/skymoore/theuth)` | `crates/web/src/lib.rs:101-103` |
| `engines::USER_AGENT` (reused by scrape) | the same literal | `crates/web/src/engines/mod.rs:64`; `crates/scrape/src/lib.rs:86` |
| `fetch_max_chars` default / range | 4000 / 200..=16000 | `lib.rs:92`; `tools.rs:504` |
| `MIN_PROSE` | 100 | `tools.rs:208` |
| `PAGE_CHARS` / `HEAD_BYTES` | 500 000 / 4096 | `crates/scrape/src/engine.rs:14`, `:17` |
| `is_thin` | prose < 800, or < 2500 with an SPA marker | `engine.rs:334-352` |
| `scrape_url` default `max_chars` | 100 000 | `crates/scrape/src/tools.rs:124` |
| `SITEMAP_CHARS` / `SITEMAP_URL_CAP` / `SITEMAP_DEPTH` / `MAX_CRAWL_DELAY` | 8 000 000 / 5 000 / 3 / 10 s | `crates/scrape/src/crawl.rs:22-28` |
| `EXCLUDE_NON_MAIN_TAGS` / `FORCE_INCLUDE_MAIN_TAGS` | 42 / 13 | `crates/firecrawl-native/src/html.rs:397`, `:442` |
| path-regex limits | 256 KiB / 256 KiB / nest 50 | `crawler.rs:465-472` |
| SSRF hostname denylist | `localhost`, `*.localhost`, `local`/`*.local`, metadata aliases, `kubernetes.default*` | `crates/web/src/fetch/ssrf.rs:118-135` |
| `excerpt::link_density` | share of chars inside `](…)` | `crates/web/src/excerpt.rs:181-194` |

### 2.3 Corrections to the survey

- `fetch/mod.rs:434` is the **only** `from_utf8_lossy` on the page path: the scrape engine reuses
  `Fetcher::fetch_html` (`engine.rs:155-159`), so D3 has one call site, not two.
- The sitemap walk **does** check its deadline between fetches (`crawl.rs:492-495`) and returns what it
  parsed; what is missing is a flag that says the result is partial and bounding each sitemap fetch by
  the remaining time (D10).
- The gzip inflate is already capped post-inflate by `.take(max_bytes)` (`fetch/mod.rs:405-407`) and
  `read_capped` already refuses an oversize `Content-Length` before allocating (`:346-353`); D12's rail
  is the missing `truncated` flag when the inflate hits the cap, plus tests.
- The largest-`srcset` rule already exists on the scrape path (`html.rs:586-661`); `read_url` skips
  images (`fetch/text.rs:20`). D12 adds a test, not a mechanism.
- There is no `ScrapeResponse` struct; `scrape_url` builds a `serde_json::Value` (`tools.rs:126-130`).
  Wave C introduces the struct (§7).

### 2.4 The gap list, condensed

1. No blocked-page diagnosis on page fetches (walls are `HTTP 403` or content).
2. No retries, no `Retry-After`, no total budget.
3. No charset handling.
4. Selector-only extraction; `.menu`/`.widget`-classed content deleted; `only_main_content` ignored on render.
5. Head-only truncation; silent clip in `scrape_url`; `crawl` uncapped.
6. Metadata computed and discarded; no canonical, no JSON-LD, no page date.
7. No compaction: carousels and nav repeats spend the `max_chars` budget.
8. `crawl`/`map_site` hide failures, deadline cuts, invalid patterns and the frontier.
9. SPA shells go straight to the browser although their JSON island is in the HTML.
10. One 789-byte fixture; `transform_html`, `extract_metadata`, `post_process_markdown` untested.

## 3. Decisions

Each decision: rationale, the exact types a worker compiles against, constants, what the model sees,
tests. Types marked *frozen* are landed by wave C as stubs (§7.2) and may not change signature.

### D1 Page issues as a structured outcome (B1)

**Rationale.** A diagnosed wall costs one string comparison and saves a render or a wasted turn; the
rung that comes next (`rendered: true` → `browser_import_cookies` → `desktop_handoff`) already exists
and is what webclaw's hosted escalation only sells. Markers are the ones tightened by false positives
on github.com, pypi.org and IBM docs, so they are keyed on the challenge *orchestrate* path and the
conclusive `cf-mitigated` header, never on the loose `challenge-platform` substring.

**Types** (`crates/web/src/fetch/issue.rs`, *frozen*):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind { AccessChallenge, RateLimited, LoginRequired, ConsentWall, ErrorPage, EmptyShell, Unsupported }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Next { Retry, Rendered, ImportCookies, Handoff }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageIssue {
    pub kind: IssueKind,
    /// Human-readable cause: `"HTTP 403 · cloudflare challenge"`, `"consent url"`, `"login title"`.
    pub detail: String,
    /// From `Retry-After` on 429/503; seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<std::time::Duration>,
    pub next: Vec<Next>,
}

/// `prose_chars` is `excerpt::prose_chars(markdown)` of the converted body; word thresholds below
/// use `CHARS_PER_WORD = 6`.
pub fn detect(status: u16, headers: &reqwest::header::HeaderMap, final_url: &url::Url, body: &str, prose_chars: usize) -> Option<PageIssue>
```

`detail` always begins with `HTTP <status>` when the status is not 2xx. `retry_after` serialises as
seconds (`serde` with a `duration_secs` helper). `(call)` the brief's thresholds are in words while
the signature carries `prose_chars`: `CHARS_PER_WORD = 6` converts (200 words → 1 200 chars, 50 →
300, 800 → 4 800); the constant is pinned by the `issue.rs` tests.

**Signals.** `detect` lowercases the body once; "body" sizes are UTF-8 byte lengths of the decoded
body. Rows are tried top to bottom; the first match wins.

| # | kind | signal | gate |
|---|---|---|---|
| 1 | `AccessChallenge` | response header `cf-mitigated` present | conclusive, any status |
| 2 | `AccessChallenge` | body contains `_cf_chl_opt`, or `challenge-platform/h/b/orchestrate/`, or `challenge-platform/h/g/orchestrate/` | exact substrings; **not** `challenge-platform` alone |
| 3 | `AccessChallenge` | Turnstile: `cf-turnstile` or `challenges.cloudflare.com/turnstile` | body < 100 000 bytes |
| 4 | `AccessChallenge` | hCaptcha: `hcaptcha.com` **and** `h-captcha` | body < 50 000 bytes |
| 5 | `AccessChallenge` | AWS WAF: `awswaf-captcha` or `aws-waf-client-browser`; interstitial: `interstitial-spinner` **and** `verifying your connection` | interstitial only when body < 10 000 bytes |
| 6 | `AccessChallenge` | DataDome: `geo.captcha-delivery.com` or `captcha-delivery.com/captcha` | — |
| 7 | `AccessChallenge` | Akamai: `<title>challenge page</title>`; or `bazadebezolkohpepadr` | the token only when body < 5 000 bytes |
| 8 | `AccessChallenge` | `<title>` starts with one of `ANTIBOT_TITLES`: `just a moment`, `attention required`, `access denied`, `checking your browser`, `please wait`, `one more step`, `verify you are human`, `bot verification`, `security check`, `ddos protection` | prose < 1 200 chars |
| 9 | `RateLimited` | status 429 | always; `retry_after` parsed |
| 10 | `AccessChallenge` | status 401 or 403 with no row 1–8 match | `detail = "HTTP 403"`; only when the body is HTML or empty |
| 11 | `ErrorPage` | status 503 without a challenge marker, or body contains `this is taking longer than usual` **and** `refresh` | `retry_after` parsed on 503 |
| 12 | `ConsentWall` | final URL contains one of `://consent.`, `/consent?`, `/consent/`, `collectconsent`, `consentcheck`, `/cmp/`, `guce.advertising.com` | — |
| 13 | `ConsentWall` | prose ≤ 300 chars and `<title>` starts with one of `before you continue`, `your privacy choices`, `we value your privacy`, `we care about your privacy`, `cookie consent`, `consent required` | — |
| 14 | `LoginRequired` | `<title>` starts with `sign in`, `log in` or `login` | prose < 1 200 chars |
| 15 | `EmptyShell` | prose < 300 chars and body > 5 000 bytes and body contains `<script` | — |
| 16 | `EmptyShell` | prose < 4 800 chars and body > 50 000 bytes and an SPA marker: `id="__next"`, `id="root"`, `id="app"`, `__next_data__`, `nuxt`, `ng-app`, `react-app` | quotes normalised (`'` and `"`) |
| 17 | `Unsupported` | reserved for the caller: `is_binary` refusals and refused document containers map to it in `read_body` | — |

`Retry-After` is parsed as integer seconds or an HTTP-date (`httpdate`-style RFC 7231 via `chrono`);
negative or unparsable → `None`.

**`next` per kind** (fixed table, pinned by test):

| kind | next |
|---|---|
| `AccessChallenge` | `[Rendered, ImportCookies, Handoff]` |
| `RateLimited` | `[Retry]` |
| `LoginRequired` | `[ImportCookies, Handoff]` |
| `ConsentWall` | `[Rendered, ImportCookies]` |
| `EmptyShell` | `[Rendered]` |
| `ErrorPage` | `[Retry]` |
| `Unsupported` | `[]` |

**Where it runs.** In `Fetcher::fetch_kind` after the retry loop (D2) settles: a 2xx HTML body is
converted first so `prose_chars` is real; a non-2xx body is read (capped) when its content type is
HTML or absent, converted, and `detect` runs on it. `(call)` A non-2xx response **with** a detected
issue returns `Ok(FetchResponse { issue: Some(..), content: <converted body or empty>, .. })` — the
model gets the ISSUE line and whatever prose the wall carried; a non-2xx **without** an issue stays
`Err(Failed("HTTP n"))` as today, so nothing that worked before changes shape. 2xx with an issue
(consent wall, empty shell, soft challenge) is `Ok` with `issue` set and the content kept.

**Surfacing.**

- `FetchResponse.issue: Option<PageIssue>`; `ScrapeResponse.issue`; each crawl page's `issue`.
- `read_url` text starts with
  `ISSUE: <kind> (<detail>) — next: <rungs>` where rungs render as `rendered: true`,
  `browser_import_cookies`, `desktop_handoff`, `retry in <n>s` (joined by ` | `); a blank line; then
  the page text if any. The model never mistakes a wall for content because the first line says so.
- `search_web` page rows: `read_page` maps an issue to `skipped: "blocked: <detail>"` for
  `AccessChallenge`/`RateLimited`/`ErrorPage`, `skipped: "login"` for `LoginRequired`,
  `skipped: "consent"` for `ConsentWall`; `EmptyShell` keeps today's `error: "no readable text …"`.
  The rendered row line prints the skip reason as it does today.
- Scrape engine ladder (W3): `fetch → issue? → island rung (D9) → render`. An `AccessChallenge` or
  `LoginRequired` on the fetch tier still goes to render when the browser is up (stealth sometimes
  passes), and the render result's `issue` is recomputed from its markdown (`status = 200`, empty
  headers); `ConsentWall`/`EmptyShell` go to the island rung first.

**Tests.** `crates/web/tests/issue.rs`: one case per row 1–16 (positive), the benign cases
(`challenge-platform` beacon alone; Turnstile embedded in a 120 KB article; a 403 JSON API body →
`None`; `sign in` title with 400 words of prose → `None`), the `next` table, `Retry-After` seconds and
date, the rendering of the ISSUE line; wiremock: a 403 with `cf-mitigated` through `read_url` yields
`issue.kind == access_challenge` and the text starts with `ISSUE:`.

### D2 Retries under one total budget (B2)

**Rationale.** A connection reset or a 503 on the first attempt is the most common transient failure
and today fails the read. One retry recovers most of them; a total budget keeps a "retry" from turning
an 8 s fetch into 40 s across redirects.

**Mechanism** (`crates/web/src/fetch/mod.rs`, `Fetcher`):

| constant | value |
|---|---|
| `MAX_ATTEMPTS` | 2 |
| `RETRY_DELAYS` | `[0 s, 1 s]` |
| `RETRY_STATUSES` | 429, 502, 503, 504, 520, 521, 522, 523, 524 |
| retryable errors | transport (`reqwest::Error` that is not a guard refusal) and body decode/read errors |
| `TOTAL_BUDGET` | `2 × TIMEOUT` = 16 s, started before the first attempt, covering attempts, redirects and body |
| attempt admission | not started when `remaining < CONNECT_TIMEOUT` (2 s); the result so far is returned |
| `Retry-After` | honoured when `delay ≤ remaining − CONNECT_TIMEOUT`, else no retry and `issue = RateLimited { retry_after }` |

`fetch_kind` becomes: `for attempt in 0..MAX_ATTEMPTS { sleep(delay); run the redirect loop under
tokio::time::timeout(remaining) }`; a per-hop request keeps the client's 8 s. `Rejected` is never
retried. `FetchResponse.attempts: u8` (1 or 2). `request` (route replay) and `http_request` keep
**no retries**: a replayed POST or a tool call may be non-idempotent, and `http.rs:13` stays true.

**Tests** (`crates/web/tests/fetch_retry.rs`, wiremock): 503 then 200 → `attempts == 2`; 500 → one
attempt, `Failed("HTTP 500")`; connection reset then 200; `Retry-After: 1` on 429 → retried after
≥ 1 s; `Retry-After: 120` → not retried, `issue.retry_after == 120 s`; a server that sleeps 7 s per
attempt → second attempt not started (`remaining < CONNECT_TIMEOUT` after redirects), result returned
within 16.5 s.

### D3 Charset decoding (B3)

**Rationale.** Every legacy-encoded page (.jp/.cn/.ru/.de sites) arrives as U+FFFD soup today.

**Type** (`crates/web/src/fetch/charset.rs`, *frozen*):

```rust
/// Decode `bytes` to text: BOM → `charset=` of `content_type` → `<meta charset>` /
/// `<meta http-equiv="Content-Type" content="…charset=…">` in the first 1 024 bytes → UTF-8 →
/// lossy UTF-8. Returns the text and the label actually used (`"utf-8"`, `"shift_jis"`, …).
pub fn decode(bytes: &[u8], content_type: Option<&str>) -> (std::borrow::Cow<'_, str>, &'static str)
```

`encoding_rs` 0.8 is already in `Cargo.lock` as a transitive dependency of reqwest and anydoc
(`Cargo.lock:5427`); C adds `encoding_rs = "0.8"` to `[workspace.dependencies]` and
`encoding_rs = { workspace = true }` to `crates/web/Cargo.toml`. `Encoding::for_label` resolves the
label; an unknown label falls through to UTF-8. `read_body` keeps the full `Content-Type` string
(today it drops parameters at `fetch/mod.rs:387-393`) and passes it to `decode`, replacing
`String::from_utf8_lossy(&raw)` at `:434`. `FetchResponse.charset: String` carries the label; scrape
inherits it through `fetch_html`.

**Tests** (`crates/web/tests/charset.rs`): Shift_JIS bytes with `charset=shift_jis` → the expected
Japanese string; windows-1251 with `<meta charset="windows-1251">` and no header parameter; UTF-8 BOM
wins over a wrong `charset=`; a bogus label → `"utf-8"`; invalid UTF-8 with no hint → lossy, label
`"utf-8"`; `shift-jis-page` bytes fixture through wiremock → `charset == "shift_jis"`.

### D4 Headers and the header ladder (B10)

**Rationale.** Some servers vary on `Accept`/`Accept-Language` and treat their absence as a bot
signal; sending them is honest. The first attempt keeps theuth's identity (robots, politeness). Only a
*diagnosed* block earns one browser-realistic header attempt — still one request, still guarded, no
cookies — before the stealth render tier.

**Mechanism.**

- Default headers on every page fetch (`Client::builder().default_headers`):
  `Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8` and
  `Accept-Language: <web.accept_language>` (config, default `en-US,en;q=0.9`).
- UA becomes `theuth/<CARGO_PKG_VERSION> (+https://github.com/skymoore/theuth)`: `engines::USER_AGENT`
  (`crates/web/src/engines/mod.rs:64`) becomes
  `concat!("theuth/", env!("CARGO_PKG_VERSION"), " (+https://github.com/skymoore/theuth)")`, and the
  `web.user_agent` default (`lib.rs:103`) uses the constant. `crates/scrape/src/lib.rs:86` keeps
  reading the constant (now versioned) — W3 does not touch it.
- Ladder: when the first attempt's `detect` says `AccessChallenge` and the status is 403 or 503 and
  `web.header_ladder` is `true` (default), one extra attempt within the same `TOTAL_BUDGET` with
  `LADDER_HEADERS`: a current Chrome UA for macOS (`(call)` pinned as a constant with a date comment,
  bumped by hand; initial value Chrome 141), `sec-ch-ua`, `sec-ch-ua-mobile: ?0`,
  `sec-ch-ua-platform: "macOS"`, `sec-fetch-dest: document`, `sec-fetch-mode: navigate`,
  `sec-fetch-site: none`, `sec-fetch-user: ?1`, `upgrade-insecure-requests: 1`, the same `Accept`/
  `Accept-Language`. Built with `RequestBuilder::headers` on the same client (same guarded resolver).
  If the ladder attempt succeeds the response says `attempts: 2` and `ladder: true`
  (`FetchResponse.ladder: bool`, `(call)` added so the as-built smoke can count ladder wins); if it is
  still an issue the first attempt's `PageIssue` is returned.
- No TLS fingerprinting, no proxy pool, no cookie jar (§5).

**Tests** (wiremock, `fetch_retry.rs`): the default headers are present on a plain fetch; a 403 with
`cf-mitigated` then 200 → `ladder == true`, the second request carries `sec-fetch-mode`; with
`header_ladder: false` → one request, `issue` set; a 403 JSON body (no issue) → no ladder attempt.

### D5 Metadata surfaced (B8)

**Rationale.** The model cannot cite a date or author today without a second read; `extract_metadata`
already computes most of it on the scrape path and throws it away (`engine.rs:354-363`).

**Type** (`crates/web/src/fetch/meta.rs`, *frozen*):

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    /// RFC 3339 UTC, normalised by `crate::date::normalize`.
    pub published: Option<String>,
    pub lang: Option<String>,
    /// `link[rel=canonical]`, absolutised against `base`.
    pub canonical: Option<String>,
    pub site_name: Option<String>,
}

pub fn extract(html: &str, base: &url::Url) -> PageMeta
```

**Sources, in priority order per field** (first non-empty wins):

| field | sources |
|---|---|
| `title` | `og:title` → `<title>` |
| `description` | `meta[name=description]` → `og:description` |
| `author` | `meta[name=author]` → `article:author` → JSON-LD `author.name` (or `author[0].name`) |
| `published` | `article:published_time` → JSON-LD `datePublished` → `meta[name=date]`; then `date::normalize(raw, Utc::now())` (`crates/web/src/date.rs:30`); unparsable → `None` |
| `lang` | `<html lang>` (trimmed, lowercased) |
| `canonical` | `link[rel=canonical][href]` joined against `base` |
| `site_name` | `og:site_name` |

JSON-LD pass: the first `<script type="application/ld+json">` block whose `@type` (string or first of
an array) is one of `Article`, `NewsArticle`, `BlogPosting`, `TechArticle`, `ScholarlyArticle`; blocks
over 64 KiB are skipped; parse errors are ignored. `extract` is a string scanner over the first
256 KiB of the HTML (no DOM): `<head>` content is what matters and `web` has no DOM crate. The scrape
engine (W3) maps `firecrawl_native::extract_metadata` output (`title`, `ogTitle`, `ogDescription`,
`ogSiteName`, `language`, `publishedTime`, `html.rs:226-395`) into the same struct and fills
`author`/`canonical` by calling `web::fetch::meta::extract` on the fetched HTML; the render tier has
no HTML, so `meta` there carries `title` only.

**What the model sees.** One line after the URL in `read_url`/`scrape_url` text:
`published 2026-09-30T08:00:00Z · by Jane Doe · lang en` — fields omitted when empty, the line
omitted when all are empty. `FetchResponse.meta: PageMeta`, `ScrapeResponse.meta: PageMeta`.

**Tests.** Unit tests in `meta.rs` on inline HTML (each source row; relative canonical absolutised;
`datePublished` normalised; a non-content `@type` ignored); `news-article` fixture → `author`,
`published`, `site_name` set.

### D6 Pageable, honest truncation (B6)

**Rationale.** `read_file` has `offset`; the web tools cut the head and stop. A cut must say how much
was dropped, where to continue, and what sections were lost so the model can ask for one by name.

**Type** (`crates/web/src/fetch/text.rs`, *frozen*):

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cut {
    pub text: String,
    pub truncated: bool,
    /// Char offset to pass as `offset` for the next window; `None` when nothing was omitted.
    pub next_offset: Option<usize>,
    /// Up to `MAX_OMITTED_HEADINGS` (12) headings of the omitted tail, in order, `#` markers stripped.
    pub omitted_headings: Vec<String>,
}

/// Window `s` from char `offset` to at most `max_chars` chars. When the window does not reach the
/// end, the cut prefers the last blank line inside the final 15 % of the window; the marker
/// `[… N chars omitted; continue with offset=M]` is appended to `text` (it does not count against
/// `max_chars`), followed by `Omitted sections: a · b · c` when the tail has headings.
pub fn cut(s: &str, offset: usize, max_chars: usize) -> Cut
```

Rules: `offset ≥ len` → `text` is `[offset M is past the end (N chars)]`, `truncated: false`,
`next_offset: None`. The block-boundary search runs only when a blank line exists in the last 15 % of
the window; otherwise the cut is at `max_chars` exactly. Fenced code is not special-cased (a window
can end inside a fence; the marker makes that visible). `hard_truncate` (`text.rs:151-158`) stays
for `http_request`'s `TEXT_CHARS` excerpt only (`(call)` bodies there are often JSON, where a
heading outline means nothing); `read_url` and `scrape_url` call `cut`.

**Interface.** `FetchArgs.offset: Option<u32>` (chars, default 0), `ScrapeArgs.offset`, both
described as `Start at this character offset (default 0); use next_offset from a truncated result.`
`FetchResponse.next_offset: Option<usize>`, `omitted_headings: Vec<String>`; `ScrapeResponse`
likewise. `read_url` keeps `max_chars` 200..=16000. `scrape_url`'s `clip` (`tools.rs:352-357`) is
replaced by `cut` on `markdown` (and on `html` with `offset` applied to both). `crawl` gains
`max_chars_per_page: Option<u32>` (default 20 000) and each page row carries `truncated`,
`next_offset` and the marker in its markdown; the `[truncated at N chars]` marker in
`render::fetch_text` (`render.rs:186`) is replaced by `cut`'s marker (the structured `truncated` flag
keeps meaning "content or raw body was cut").

**Tests** (`crates/web/tests/cut.rs`): no cut when it fits; cut at a blank line within the last 15 %;
cut at `max_chars` when no blank line; `next_offset` chains to the end with no gaps or overlaps
(property-style loop over the `docs-page` golden); `omitted_headings` lists the tail's `##` in order,
capped at 12; offset past the end; marker text exact.

### D7 Compaction before the cap (B7)

**Rationale.** A carousel repeated three times, a 40-logo row, Tailwind class strings leaked into text
and zero-width characters all spend `max_chars` before the article does. webclaw's `llm` format shows
the passes that pay; the rest (English UI heuristics, emphasis stripping) is left out because agents
navigate by links and read markdown.

**Types** (`crates/web/src/compact.rs`, *frozen*):

```rust
#[derive(Clone, Copy, Debug, Default)]
pub struct CompactOptions {
    /// `[text](url)` → `text` plus a trailing `Links:` list (used by `format: text`).
    pub fold_links: bool,
    /// Keep image lines (scrape); `false` drops them after the collapse pass.
    pub keep_images: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compacted {
    pub text: String,
    pub removed_chars: usize,
}

pub fn compact(markdown: &str, opts: &CompactOptions) -> Compacted
```

**Passes, in order; every pass skips fenced code (```` ``` ```` and `~~~` fences) verbatim:**

| # | pass | rule |
|---|---|---|
| 1 | invisible chars + entities | remove U+200B, U+200C, U+200D, U+00AD, U+FEFF; decode `&amp;` `&lt;` `&gt;` `&quot;` `&#39;` `&nbsp;` (→ space) that leaked |
| 2 | utility-class lines | drop a line when > 50 % of its ≥ 3 whitespace tokens match `UTILITY_PREFIXES` (`text-`, `bg-`, `px-`, `py-`, `pt-`, `pb-`, `pl-`, `pr-`, `mt-`, `mb-`, `ml-`, `mr-`, `mx-`, `my-`, `w-`, `h-`, `max-w-`, `min-h-`, `gap-`, `rounded`, `border-`, `font-`, `leading-`, `tracking-`, `shadow`, `hover:`, `focus:`, `sm:`, `md:`, `lg:`, `xl:`, `dark:`) or `UTILITY_EXACT` (`flex`, `grid`, `hidden`, `block`, `inline`, `relative`, `absolute`, `fixed`, `sticky`, `container`, `truncate`, `uppercase`, `italic`) |
| 3 | image runs | ≥ 3 consecutive image-only lines → `[N images]` keeping the first non-empty alt as `[N images: <alt>]`; with `keep_images: false` the remaining image lines are dropped |
| 4 | a11y chrome | remove the substrings `opens in new tab`, `(opens in a new window)`, `opens in a new window`, `skip to content`, `skip to main content` (case-insensitive) and the line when it becomes empty |
| 5 | block dedup | blocks (split on blank lines) ≥ 20 chars: fingerprint A = lowercase alphanumerics with collapsed whitespace; fingerprint B = first 10 words of A; a block whose A **or** B was seen is dropped |
| 6 | line dedup in a block | inside one block, a line identical (fingerprint A) to an earlier line of the same block is dropped |
| 7 | comma-list dedup | on a line with ≥ 4 comma-separated items, repeated items (fingerprint A) are dropped |
| 8 | empty headings | a heading followed by another heading of the same or higher level, or by the end, is dropped; repeated trailing headings likewise |
| 9 | blank runs | ≥ 2 blank lines → 1; trailing whitespace trimmed |
| 10 | link folding (`fold_links`) | `[text](url)` → `text`; `![alt](src)` → `alt`; a trailing `Links:` list of `- label: href`, deduped by href, at most 60, `#`/`javascript:` hrefs skipped |

`removed_chars = input.chars().count() − text.chars().count()`.

**Wiring.** `read_url`: `html_to_markdown → compact(fold_links = format == text) → markdown_to_text
(text only) → cut`; `scrape_url`: `to_markdown → compact { keep_images: true } → cut`. Controlled by
`FetchArgs.compact: Option<bool>` / `ScrapeArgs.compact` (default `true`; description
`Remove repeated blocks, image runs, class leaks and invisible characters before the cap (default true).`).
`FetchResponse.compacted_chars: usize`, `ScrapeResponse.compacted_chars`. `crawl` pages are compacted
with the same options as `scrape_url`.

**Target and test** (`crates/web/tests/compact.rs`): on the chrome-heavy fixtures (`carousel-dupes`,
`tailwind-leak`, `news-article`) `removed_chars ≥ 15 %` of the input; on the clean fixtures
(`docs-page` golden, `layout-table-page` golden) `removed_chars == 0` and `text == input`; a fenced
block containing three identical lines survives untouched; `fold_links` yields the `Links:` list
deduped and capped at 60; U+200B inside a word is removed without joining words.

### D8 Main-content selection with scoring and two invariants (B4, B5)

**Rationale.** Selector deletion leaves sidebars without the listed classes in and removes content
that happens to live in a `.menu`/`.widget`-classed container. A cheap scorer backstops the strip list
when no semantic main region exists; two invariants keep the scorer from being clever where the page
or the caller already said what the content is; safety valves keep the strip list from eating a page.

**Types** (`crates/firecrawl-native/src/extract.rs`, *frozen*):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method { Main, Article, Scored, Body }

#[derive(Clone, Debug, Default)]
pub struct SelectOptions {
    /// The caller passed `include_tags` (explicit scope): never widen.
    pub explicit_scope: bool,
    pub only_main_content: bool,
}

#[derive(Clone, Debug)]
pub struct Selection {
    /// Serialised HTML of the chosen region (or the whole stripped document).
    pub html: String,
    /// `true` when a `<main>`/`<article>`/`[role=main]` or explicit scope decided.
    pub scoped: bool,
    pub method: Method,
}

/// Run after the strip list on the already-stripped document; `original` is the untouched page
/// (for `<noscript>` recovery, which `transform_html` detaches early at `html.rs:508-510`).
pub fn select_main(html: &str, original: &str, opts: &SelectOptions) -> Selection
```

**Algorithm** (inside `transform_html`, after the `EXCLUDE_NON_MAIN_TAGS` pass at `html.rs:568-584`,
only when `only_main_content`):

1. `<main>` → `Method::Main`, `scoped: true`; else first `<article>` → `Article`, `scoped: true`;
   else `[role=main]` → `Main`, `scoped: true`. (Today none of these is preferred.)
2. Otherwise score every candidate `article, main, [role=main], div, section, td` that is not noise
   and not inside noise (`noise::is_noise`):
   `score = ln(text_len)` (0 when `text_len < 50`) `+ 50` if tag ∈ {article, main} `+ 50` if
   `role=main` `+ 25` if class contains `content|article|post|entry` `+ 25` if id contains
   `content|article|post|main` `+ 3 × count(<p> descendants)`; then `× penalty` from
   `link_density(text)` = share of chars inside anchors: semantic nodes (article/main/role=main)
   `×0.3` when density > 0.7, `×0.5` when > 0.5; generic nodes `×0.1` when > 0.5, `×0.5` when > 0.3.
   Highest wins; ties keep document order (first). `Method::Scored`, `scoped: false`.
   The link-density test is the pure function `web::excerpt::link_density`'s rule re-implemented over
   DOM text in `extract.rs` (`firecrawl-native` cannot depend on `web`; the two are pinned to the same
   numbers by a shared test vector in both crates).
3. No candidate → `<body>` → `Method::Body`.
4. `<noscript>` recovery: when the selection's text has < 30 words, each `<noscript>` in `original`
   whose nearest ancestor is inside the selected region is re-parsed as a `<section>`; used when it
   yields ≥ 5 words **and** more words than the current selection.
5. **Invariant 1 — explicit scope is final.** `opts.explicit_scope` or a real `<main>` → steps 2–4 and
   invariant 2 never run; the result is what the scope says even when empty.
6. **Invariant 2 — body retry.** Only when `!scoped`, the result has < 200 words and `<body>` of the
   stripped document yields > 2× the words **and** > 50 words: take `<body>`, `Method::Body`.
   (Replaces today's "empty main → whole page" at `engine.rs:383-385`, which W3 removes once W2's
   selection lands; until then the stub returns the input unchanged and the old fallback stays.)

Scoring walks the DOM with a depth guard of 200 levels (D12): deeper subtrees count as text only.

**Noise safety valves** (`crates/firecrawl-native/src/noise.rs`, *frozen*), applied to every match of
`EXCLUDE_NON_MAIN_TAGS` before `detach()` at `html.rs:580`:

```rust
/// Whether a strip-list match may be removed.
pub fn may_strip(node: &kuchikiki::NodeRef) -> bool
/// Whether `node` is noise for candidate scoring (tag, role, class token, id, consent prefix).
pub fn is_noise(node: &kuchikiki::NodeRef) -> bool
```

| valve | rule |
|---|---|
| text valve | a matched element with > 5 000 chars of text is content: kept |
| `<form>` | noise only when its text < 500 chars **or** its class contains `login|search|subscribe|signup|newsletter|contact` |
| structural ids | an id with suffix `portal|root|container|wrapper|mount|app` is never stripped by an id/class rule |
| consent platforms | id/class **prefixes** `onetrust`, `optanon`, `ot-sdk`, `cookiebot`, `CybotCookiebot`, `cc-`, `cookie-law`, `gdpr`, `consent-`, `cmp-`, `sp_message`, `qc-cmp`, `trustarc`, `evidon` are added to what `only_main_content` strips (as a `CONSENT_PREFIXES` table consulted by `may_strip`'s caller, not by appending CSS selectors) |
| class matching | the 42 selectors stay CSS selectors (exact class token by construction); nothing matches by substring |

**Render tier.** `Scrape::render_with` (`engine.rs:190-225`) ignores `only_main_content` today; W3
passes the rendered markdown through `compact` and, when `only_main_content`, through a markdown-level
chrome trim (drop leading/trailing link-only blocks by `excerpt`'s `is_noise` rule) — the DOM is not
available on that tier, so this is the honest minimum, stated in the tool description.

**Tests.** `crates/firecrawl-native/tests/corpus.rs` goldens (`docs-page` → `Method::Main`,
`news-article` → `Article`, `unclosed-header-wrapper` → `Scored` with the article kept by the text
valve, `form-wrapped-aspnet` → the form is content); contract tests in `extract.rs`: the score of a
synthetic candidate equals the formula, ties keep order, density penalties, invariant 1 (an explicit
scope with 0 words is returned empty, never widened), invariant 2 (190 words vs body 400 → body;
190 vs 300 → not), noscript recovery thresholds, the 200-level depth guard on `deep-dom-200`.

### D9 Data-island rung (B11)

**Rationale.** An SPA shell often ships its content as JSON in the HTML (`__NEXT_DATA__`, Nuxt/
SvelteKit payloads, `ld+json` `articleBody`). Reading it costs a JSON walk, not a browser slot.

**Type** (`crates/firecrawl-native/src/islands.rs`, *frozen*):

```rust
/// Prose recovered from JSON data islands, as paragraphs in document order; `None` when nothing
/// qualifies. No JavaScript is evaluated.
pub fn extract_islands(html: &str) -> Option<String>
```

Sources: `<script id="__NEXT_DATA__">`, `<script type="application/json">` (any id), `<script
type="application/ld+json">` → `articleBody` only, SvelteKit `kit.start(…, {data: […]})` → the JSON
array literal after `data:` when it parses (no JS object-literal repair beyond trailing commas).
Walk every JSON string: keep strings ≥ 40 chars that are not URL-like (`^[a-z]+://`, `^/`, `^www.`)
and not key-like (no whitespace); dedup by fingerprint A (D7); keep order; join with blank lines;
caps `MAX_CHUNKS = 1 000`, `MAX_CHARS = 500 000`.

**Rung** (W3, `engine.rs:fetch_page`): after `is_thin` says thin and before `render_with`:
`if let Some(text) = extract_islands(&page.html) && prose_chars(&text) ≥ 800 { tier: "island",
markdown = post_process_markdown(text) }`. `Fetched.tier` gains `"island"`; `ScrapeResponse.tier`
and crawl rows say it. When the island is used, `issue` is cleared if it was `EmptyShell`.

**Tests.** `spa-shell-next` fixture → `Some` with ≥ 1 200 words, `tier_hint: "island"`;
`spa-shell-empty` → `None`; strings under 40 chars, URLs and keys excluded; cap behaviour on a
synthetic 2 000-string island; the engine test `island_rung_before_render` with a stubbed browser
lookup (no actor registered → island used, no render attempted).

### D10 Crawl and map that admit what they missed (B9)

**Rationale.** A 60 s deadline cut, a bad regex and a timed-out page are invisible today; the model
reads 23 pages and believes the site has 23 pages.

**Shapes** (W3; the dict type is landed by C as `CrawlResult` in `crates/scrape/src/tools.rs`):

```rust
#[derive(Clone, Debug, Default, Serialize)]
pub struct CrawlResult {
    pub pages: Vec<CrawlPage>,          // {url, title, markdown, tier, issue, meta, truncated, next_offset[, html]}
    pub incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,   // "deadline" | "limit" | "cancelled"
    pub skipped: Vec<Skipped>,          // {url, error}
    pub invalid_patterns: Vec<String>,
}
```

- `incomplete = frontier non-empty when the loop ended`; `reason = "deadline"` when
  `Instant::now() ≥ opts.deadline`, `"limit"` when `pages.len() ≥ opts.limit`; `(call)` `"cancelled"`
  is reserved for a cancellation signal the crawl can observe — W3 sets it only if one exists at
  integration time, otherwise the variant stays declared and unused.
- `skipped`: every page that `tracing::debug!`s at `crawl.rs:300-309` today (`error` = the fetch
  error text or `"timed out after Ns"`), capped at 200 rows (then `… and N more` as a last row).
- `invalid_patterns`: `firecrawl_native::compile_path_patterns(&opts.include_paths ++
  &opts.exclude_paths).1`, computed once at crawl start. New in `crawler.rs` beside
  `compile_path_regex` (unchanged):

```rust
/// Compile path patterns with the untrusted-input limits; returns the compiled set and the
/// patterns that failed with their error text (`"<pattern>: <error>"`).
pub fn compile_path_patterns(patterns: &[String]) -> (Vec<regex::bytes::Regex>, Vec<String>)
```

- `map_site`: `MapHit.source: &'static str` ∈ `"search" | "sitemap" | "page" | "frontier"`; after
  the walk, the walk's unfetched frontier (in-scope URLs popped from no batch) is appended as
  `{url, title: null, source: "frontier"}` until `limit`; rows print `url  title  [source]`.
- Sitemap walk: each `fetch_html` is wrapped in `tokio::time::timeout(remaining)`; the result keeps
  what parsed (already the loop's shape at `crawl.rs:492-513`); `CrawlResult.reason = "deadline"`
  when the sitemap phase itself hit the deadline.
- `crawl` `RETURNS` becomes `"dict"`; text output unchanged in shape, plus a final line
  `incomplete: <reason> · skipped N · invalid patterns N` when any is non-zero.
- `search_web` is unchanged except the skip reasons (D1).

**Tests** (wiremock in `crates/scrape/src/crawl.rs` tests): a deadline of 1 ms → `incomplete,
reason: "deadline"`, pages ≤ 1; `limit: 2` on a 5-page site → `incomplete, reason: "limit"`; one
500 page → in `skipped` with `HTTP 500`; `include_paths: ["(unclosed"]` → `invalid_patterns` has one
entry and the crawl still runs; `map_site` with `limit: 50` on a 3-page site with 40 links →
`source: "frontier"` rows present, total ≤ 50; sitemap with a 10 s sleeping child and a 100 ms
deadline → parent URLs kept.

### D11 Known JSON sources (B12)

**Rationale.** A handful of hosts are read constantly and have a public JSON endpoint that is
smaller, cleaner and not rate-limited by bot walls. Eight rows, not thirty extractors.

**Types** (`crates/web/src/sources.rs`, *frozen*):

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiSource {
    pub kind: &'static str,
    /// The URL to fetch instead of the page; always re-checked by the SSRF guard.
    pub fetch_url: url::Url,
}

/// The table below, matched on host and path; `None` for everything else.
pub fn api_source(url: &url::Url) -> Option<ApiSource>
/// Render a fetched body of `kind` as compact markdown; `None` when the body does not parse.
pub fn render(kind: &str, body: &str) -> Option<String>
```

| kind | page URL shape | fetch URL | rendering |
|---|---|---|---|
| `hn-item` | `news.ycombinator.com/item?id={id}` | `https://hn.algolia.com/api/v1/items/{id}` | `# title` · `by author · points · created_at` · text · comment tree depth ≤ 3, ≤ 60 comments as nested `- **author**: text` |
| `reddit-old` | `*.reddit.com/r/{sub}/comments/{id}/…` | `https://old.reddit.com/r/{sub}/comments/{id}/…` (HTML through the normal pipeline) | n/a — the URL rewrite alone; `source_kind: "reddit-old"` |
| `npm` | `npmjs.com/package/{name}` (scoped names kept) | `https://registry.npmjs.org/{name}` | `# name version` · description · `license · homepage · repository` · README head (first 3 000 chars) |
| `pypi` | `pypi.org/project/{name}/` | `https://pypi.org/pypi/{name}/json` | `# name version` · summary · `requires_python · license · home_page` · description head |
| `crates-io` | `crates.io/crates/{name}` | `https://crates.io/api/v1/crates/{name}` | `# name max_version` · description · `downloads · repository · documentation` · keywords |
| `arxiv` | `arxiv.org/abs/{id}` | `https://export.arxiv.org/api/query?id_list={id}` (Atom; `crates/web/tests/fixtures/arxiv_atom.xml` shows the shape) | `# title` · authors · published · abstract · PDF link |
| `wikipedia` | `{lang}.wikipedia.org/wiki/{title}` | `https://{lang}.wikipedia.org/w/api.php?action=query&prop=extracts&explaintext=1&format=json&titles={title}` | `# title` · the extract as paragraphs (first 12 000 chars) |
| `stackoverflow` | `stackoverflow.com/questions/{id}/…` | `https://api.stackexchange.com/2.3/questions/{id}?site=stackoverflow&filter=withbody` then `…/questions/{id}/answers?site=stackoverflow&filter=withbody&sort=votes` | `# title` · score · body (HTML → markdown with `fetch::text`) · top 3 answers by score, accepted first |

Exactly these; `kind` strings are the table's. `fetch_url` is built with `url::Url` methods (no string
formatting of user input into a URL); `{name}`/`{title}` are percent-encoded path segments.

**Wiring.** `read_url`/`scrape_url` consult `api_source` first unless `source: "page"`:
`FetchArgs.source: Option<Source>` with `Source::{Auto, Page, Api}` (default `auto`; description
`auto (default): known hosts are read from their JSON API; page: always the HTML; api: fail if no API
source matches.`). The API body goes through `Fetcher::fetch_html` (same guard, same caps, same
retries); `render(kind, body)` produces the markdown, which then flows through `compact` and `cut`.
Response: `tier: "api"`, `source_kind: <kind>`; the text's URL line shows the page URL and
`via <fetch_url>` on the meta line. `(call)` If the API fetch fails or `render` returns `None` under
`auto`, the page pipeline runs as if no source matched (the response carries `notes: ["api source
<kind> failed: <err>; read the page"]`); under `api` it is an error.

**Tests** (`crates/web/tests/sources.rs`, saved JSON samples under
`crates/web/tests/fixtures/sources/*.json`, no network): `api_source` positive and negative per row
(including `news.ycombinator.com/news` → `None`, `reddit.com/r/x` listing → `None`,
`en.wikipedia.org/wiki/Special:Search` → `None`); `render` on each sample yields the documented
headings; the HN comment cap and depth; a Stack Overflow sample with 5 answers → 3, accepted first;
wiremock `read_url` of an HN item URL with the Algolia host pointed at the mock → `tier == "api"`.

### D12 Rails and security (B14 + comparison §5)

| rail | today | change | wave |
|---|---|---|---|
| Content-Length pre-check | exists (`fetch/mod.rs:346-353`) | test added | W1 |
| post-inflate cap | gzip inflate `.take(max_bytes)` (`:405-407`) silently cuts | set `truncated: true` when the inflate reaches the cap; reqwest (no `gzip` feature) counts wire bytes, which equal body bytes, so the streamed cap is already post-inflate for everything the client decodes (nothing) | W1 |
| DOM depth guard | none in `firecrawl-native` walks | 200 levels in `select_main`/`is_noise` traversal; deeper subtrees are text | W2 |
| layout-table pre-pass | none | before h2m, a `<table>` with any cell containing `p, div, ul, ol, h1-h6, table, blockquote` becomes `<section>`s per cell (`html.rs` pre-pass in `transform_html`) | W2 |
| code language | h2m 3.12 reads `language-X`/`lang-X` on `<pre>`/`<code>` (its own `src/converter/handlers/code_block.rs:229-249`, outside this tree) | W2 first checks what h2m already covers; adds normalisation of `highlight-X`, `sp-X` and bare names (`js`, `bash`, `yaml`, `py`→`python`, `sh|zsh|shell`→`bash`, `yml`→`yaml`, `ts`→`typescript`) by rewriting the class to `language-<norm>` in the pre-pass | W2 |
| largest `srcset` when `src` missing | exists (`html.rs:586-661`) | golden test on `news-article` | W2 |
| SSRF hostname denylist | `ssrf.rs:118-135` | add `*.internal`, `*.lan`, `*.home.arpa`, `*.intranet`, `*.corp` (suffix match, also the bare label); IPv6 NAT64 `64:ff9b::/96` (classified as the embedded v4) and IPv4 `240.0.0.0/4` in `is_blocked_ipv4`/`is_blocked_ipv6` | W1 |
| no shell with page-derived strings | nothing in these crates shells out | stated as a rule in §5; a clippy-free grep in the closer's checklist | closer |

Follow-ups listed, not built (§8): reconcile `browser.allow_private_network` with the fetcher's
denylist; `http_request` GET/HEAD risk class; proxy config for plain fetches.

### D13 Fixture corpus and contract tests (B13) — precondition

**Rationale.** Nothing in D1/D7/D8/D9 can be tuned or kept without pages to run on. The corpus is
**synthesised** by W2 from the marker tables — no live fetching, no copyrighted page bodies — so it is
ours, small, and exact about what it exercises.

Layout: `crates/firecrawl-native/tests/fixtures/<name>.html` + `<name>.expected.md` (golden) +
`<name>.expect.json`:

```json
{ "issue": "access_challenge" | null, "tier_hint": "fetch" | "island" | "render" | null,
  "method": "main" | "article" | "scored" | "body" | null,
  "min_words": 0, "must_contain": ["…"], "must_not_contain": ["…"] }
```

`crates/firecrawl-native/tests/corpus.rs` runs every fixture through `transform_html` →
h2m → `post_process_markdown` (the same calls `engine.rs:369-387` makes; W2 duplicates the three
lines rather than depending on `scrape`), compares with the golden (`UPDATE_GOLDEN=1` rewrites), and
asserts the expect.json fields; `issue` is asserted by `web`'s `issue.rs` reading the same HTML file
by relative path (`../firecrawl-native/tests/fixtures/…`), so the two crates share one corpus. The
table is §6.1. Byte fixtures (`shift-jis-page.bytes`, `pdf-as-octet-stream.bin`) live under
`crates/web/tests/fixtures/` because they exercise the fetcher, not extraction.

## 4. Tool interface changes

All new fields are `#[serde(default)]` so existing callers and the Python runtime signatures keep
working; descriptions below are the doc comments as they will appear in the schema.

### 4.1 `read_url` (`crates/web/src/tools.rs`, `FetchArgs` / `FetchResponse`)

| field | kind | type | default | description (verbatim) |
|---|---|---|---|---|
| `offset` | arg | `u32` | 0 | Start at this character offset (default 0); use next_offset from a truncated result. |
| `compact` | arg | `bool` | true | Remove repeated blocks, image runs, class leaks and invisible characters before the cap (default true). |
| `source` | arg | `auto \| page \| api` | auto | auto (default): known hosts (HN, Reddit, npm, PyPI, crates.io, arXiv, Wikipedia, Stack Overflow) are read from their JSON API; page: always the HTML; api: fail if no API source matches. |
| `issue` | response | `PageIssue?` | null | Why the page did not read as content, with the next rung. |
| `meta` | response | `PageMeta` | {} | title, description, author, published (RFC 3339), lang, canonical, site_name. |
| `attempts` | response | `u8` | 1 | HTTP attempts made (1 or 2). |
| `ladder` | response | `bool` | false | The browser-realistic header attempt produced this response. |
| `charset` | response | `string` | "utf-8" | Encoding label used to decode the body. |
| `compacted_chars` | response | `usize` | 0 | Characters removed by compaction before the cap. |
| `next_offset` | response | `usize?` | null | Pass as `offset` to continue; null when nothing was omitted. |
| `omitted_headings` | response | `string[]` | [] | Headings of the omitted tail (≤ 12). |
| `tier` | response | `string` | "fetch" | `fetch`, `render` or `api`. |
| `source_kind` | response | `string?` | null | The API source used (`hn-item`, `npm`, …). |
| `notes` | response | `string[]` | [] | Non-fatal remarks (an API source that failed, a ladder attempt that did not help). |

Description (`fetch_description`, `tools.rs:158-177`) becomes: *"Fetch one http(s) page and return
its readable content as markdown (default) or plain text, capped at max_chars; offset pages through
a long page and a cut result names the omitted sections. A page behind a bot wall, login or consent
screen starts with `ISSUE:` and names the next rung (rendered: true, browser_import_cookies,
desktop_handoff). Known hosts (HN, Reddit, npm, PyPI, crates.io, arXiv, Wikipedia, Stack Overflow)
are read from their JSON API unless source: page. Private, loopback and link-local destinations are
refused."* + the existing rendering-provider sentence.

**Text rendering** (`render::fetch_text`):

```
[ISSUE: <kind> (<detail>) — next: <rungs>]        (only when issue is set, then a blank line)
# <title>
<url>[ · via <fetch_url>]
[published … · by … · lang …]
                                                   (blank line)
<content>
[[… N chars omitted; continue with offset=M]]
[Omitted sections: a · b · c]
```

Example — a wall:

```
ISSUE: access_challenge (HTTP 403 · cloudflare challenge) — next: rendered: true | browser_import_cookies | desktop_handoff

# Just a moment…
https://example-shop.test/catalog

Enable JavaScript and cookies to continue.
```

Example — a cut:

```
# Deploying the engine
https://docs.example.test/deploy
lang en

## Prerequisites
…
## Install
…

[… 9 214 chars omitted; continue with offset=3 986]
Omitted sections: Configure · Run as a service · Upgrade · Troubleshooting
```

Example — an API source:

```
# Show HN: A tiny Rust HTML compactor
https://news.ycombinator.com/item?id=41234567 · via https://hn.algolia.com/api/v1/items/41234567
published 2026-10-01T14:02:11Z · by throwaway_dev

212 points. We built …

- **alice**: Does it keep code fences?
  - **throwaway_dev**: Yes — fences pass through verbatim.
```

### 4.2 `search_web`

No argument changes. `pages[].skipped` gains the values `blocked: <detail>`, `login`, `consent`
(`aggregate.rs:64-67` doc comment updated). The description's sentence *"every unread row says why
(not-relevant, budget)"* becomes *"(not-relevant, budget, blocked, login, consent)"*.

### 4.3 `scrape_url` (`crates/scrape/src/tools.rs`)

New `pub struct ScrapeResponse { url, title, tier, markdown?, html?, links?, issue, meta, charset,
compacted_chars, truncated, next_offset, omitted_headings, source_kind, notes }` (serde, replaces the
`json!` at `tools.rs:126-130`). Args: `offset`, `compact`, `source` as for `read_url`; `max_chars`
description becomes *"Markdown cap in characters (default 100000); a cut result carries next_offset
and the omitted headings."* Description: *"Fetch one page to markdown. Returns {url, title, tier,
markdown} plus html/links when asked in formats, with issue (bot wall / login / consent and the next
rung), meta (author, published, canonical) and a truthful cut (next_offset). only_main_content drops
chrome on the fetch tier and trims link-only blocks on the render tier; rendered: true forces the
embedded browser; tier says fetch, island, render or api. Use crawl for a site, map_site for its URLs."*

### 4.4 `crawl`

`RETURNS: "dict"`; `max_chars_per_page: Option<u32>` (default 20 000) — *"Per-page markdown cap
(default 20000); a cut page carries next_offset for scrape_url."* Rows gain `issue`, `meta`,
`truncated`, `next_offset`. Description: *"Crawl a site from a seed URL. Returns {pages: [{url,
title, markdown, tier, issue, truncated}], incomplete, reason (deadline | limit), skipped: [{url,
error}], invalid_patterns}. Bounded by limit, max_depth and max_chars_per_page; include_paths/
ignore_paths scope the walk (a pattern that does not compile is reported, not ignored); sitemap
include|skip|only; honours robots.txt. Stays under the seed directory unless crawl_entire_domain."*

### 4.5 `map_site`

Rows gain `source: "search" | "sitemap" | "page" | "frontier"`. Description: *"List a site's URLs
without fetching bodies: sitemap, then links from a short walk, then the walk's unfetched frontier;
search="…" puts URLs relevant to a query first. Returns one {url, title, source} per hit (title
only where a page was fetched)."*

### 4.6 `crates/theuth/harness.md` (closer)

- Line 65 config example: `web: {…, fetch_max_chars: 4000, read_top: 5, accept_language: en-US,en;q=0.9, header_ladder: true}`.
- After the `browser_*` paragraph (`harness.md:216-229`) add one paragraph: *"`read_url` tells you
  when a page is a wall: a result that starts with `ISSUE:` names the kind and the next rung —
  `rendered: true`, then `browser_import_cookies`, then `desktop_handoff`; do not quote the wall as
  content. A cut result ends with `[… N chars omitted; continue with offset=M]` and the omitted
  section names: call again with `offset=M` or ask for the section. `scrape_url` and `crawl` cut the
  same way; `crawl` returns `{pages, incomplete, reason, skipped, invalid_patterns}` — an
  `incomplete` crawl is not the whole site. `map_site` rows say where each URL came from
  (`sitemap`, `page`, `frontier`, `search`)."*
- The orchestration-lessons note `read_url(max_chars) 200..=16000` stays true; add `offset`.

### 4.7 Config (`WebConfig`, `crates/web/src/lib.rs:44-80`)

| key | default | effect |
|---|---|---|
| `web.accept_language` | `en-US,en;q=0.9` | `Accept-Language` on every page fetch |
| `web.header_ladder` | `true` | one browser-realistic header attempt after a diagnosed 403/503 challenge |

Neither is exposed in Settings › Search (`settings.rs` untouched); both are documented in 16 §6's
table by the closer.

## 5. Security

- **SSRF.** Every request this design adds — the retry, the ladder attempt, the API source fetch,
  the sitemap fetch under a timeout — goes through the same `Fetcher` with the guarded resolver
  (`ssrf.rs:104-113`) and the per-hop static check (`fetch/mod.rs:200`). `api_source` builds
  `fetch_url` from a fixed host table and percent-encoded segments; the result is still checked like
  any URL (a table entry can never point at a private host, but the guard does not trust the table).
  The hostname denylist widens (D12); the IP ranges add NAT64 and `240/4`.
- **Header ladder constraints.** One attempt, only after `detect` returned `AccessChallenge` with
  status 403/503, only when `web.header_ladder`, inside the same total budget, same guard, **no
  cookies, no `Referer`**, and never on `http_request`/`request` (caller headers there are the
  caller's). The ladder changes headers, not identity on the wire beyond the UA string; TLS stays
  rustls with the platform verifier. The honest UA is the first attempt on every host, so robots and
  rate-limit policies see `theuth/<version>` first.
- **Cookie rule.** No cookie jar in this design: the `cookies` feature stays off, `read_url`/
  `scrape_url`/`crawl` carry none, and the ladder sends none. Cookies exist only in the browser
  isolate (`crates/browser/src/session.rs`) and as the user-approved `browser_import_cookies`; the
  ISSUE line *names* that rung, it does not take it.
- **API sources through the guard and the caps.** Same `max_fetch_bytes`, same `read_capped`, same
  `TOTAL_BUDGET`; the Algolia/Stack Exchange/registry bodies are JSON parsed with `serde_json` under
  the existing body cap — no unbounded recursion (`serde_json`'s default 128-level limit stands).
- **No shell with page-derived strings.** Nothing in `web`, `scrape` or `firecrawl-native` runs a
  process; the rule is written here for sensors and routines that act on page content (webclaw's P0
  of 0.3.14 was exactly that): a page-derived or config string never reaches `sh -c`.
- **Cross-origin headers.** `Fetcher::request`'s refusal of a cross-origin redirect with caller
  headers (`fetch/mod.rs:294-298`) is untouched; the retry loop wraps the existing hop loop and
  cannot bypass it.
- **Fixtures are synthesised.** No real page body enters the tree; marker strings (`_cf_chl_opt`,
  `bazadebezolkohpepadr`) are identifiers, not copyrightable prose.

## 6. Testing

### 6.1 The corpus (`crates/firecrawl-native/tests/fixtures/`, synthesised by W2)

| # | fixture | what it exercises | expected `issue` | `tier_hint` | `method` | notes |
|---|---|---|---|---|---|---|
| 1 | `docs-page` | sidebar nav + `<main>` article + `<pre class="language-rust">`/`lang-bash` code | null | fetch | main | golden has fenced code with normalised languages; `compact` must remove 0 chars |
| 2 | `news-article` | `<article>`, JSON-LD `NewsArticle` (author, datePublished), byline, related-links sidebar, comments, `og:*`, canonical, lazy `srcset` | null | fetch | article | `meta.author`, `meta.published`, `meta.canonical` set; related links absent from golden |
| 3 | `spa-shell-next` | `<div id="__next"></div>` + `__NEXT_DATA__` with 1 200 words | `empty_shell` on the raw fetch | island | body | `extract_islands` ≥ 1 200 words |
| 4 | `spa-shell-empty` | `id="root"`, 60 KB of script, no island | `empty_shell` | render | body | `extract_islands == None` |
| 5 | `cf-challenge` | orchestrate script path + `<title>Just a moment…</title>` + `_cf_chl_opt` | `access_challenge` | render | body | status 403 in the web test |
| 6 | `cf-blocked-1020` | body "Sorry, you have been blocked" + `cf-ray` style markup; the `cf-mitigated` header case is a web wiremock test | `access_challenge` (row 8 title `attention required`) | render | body | — |
| 7 | `turnstile-embedded-benign` | a 120 KB real-shaped article that embeds `cf-turnstile` in a comment form | **null** | fetch | article | the size gate; the article golden is produced |
| 8 | `consent-wall-onetrust` | `#onetrust-consent-sdk` overlay with 40 words + title "We value your privacy", article underneath | `consent_wall` (row 13) on the wall-only variant; the overlay stripped by the consent prefixes | fetch | scored | two HTML files: `-wall` (only the overlay) and the page |
| 9 | `login-page` | `<title>Sign in</title>`, a `<form class="login-form">`, 80 words | `login_required` | render | body | form stripped by the `<form>` rule |
| 10 | `akamai-challenge` | `<title>challenge page</title>`, 2 KB body with `bazadebezolkohpepadr` | `access_challenge` | render | body | — |
| 11 | `layout-table-page` | one 3-cell layout table with `<p>`/`<ul>` cells + one 4×3 data table | null | fetch | main | golden: sections then a pipe table |
| 12 | `deep-dom-200` | 260 nested `<div>` around a 300-word paragraph | null | fetch | scored | depth guard: paragraph present in golden |
| 13 | `carousel-dupes` | the same 3-slide carousel repeated 3× + a 12-logo row + U+200B in text | null | fetch | main | `compact` removes ≥ 15 %; golden is post-compaction for this row (compact test), extraction golden pre-compaction |
| 14 | `tailwind-leak` | class strings leaked as text lines between paragraphs | null | fetch | scored | pass 2 removes them |
| 15 | `unclosed-header-wrapper` | `<div class="header">` left unclosed so it swallows the article (6 000 chars) | null | fetch | scored | text valve keeps it |
| 16 | `form-wrapped-aspnet` | whole page inside `<form id="aspnetForm">` (2 000 chars) | null | fetch | scored | `<form>` rule keeps it |
| w1 | `shift-jis-page.bytes` (web) | Shift_JIS bytes, `charset=shift_jis` header | null | fetch | — | `charset == "shift_jis"`; expected string pinned |
| w2 | `pdf-as-octet-stream.bin` (web) | a 2-page PDF served `application/octet-stream` | today `Failed("unsupported content type")`; `(call)` the sniff now runs **before** `is_binary` for `application/octet-stream` and `application/zip` only when the first bytes are a known container (`tools::doc::format`), so the PDF converts | fetch | — | `is_binary` otherwise unchanged |

### 6.2 Contract tests

| file | pins |
|---|---|
| `crates/web/tests/issue.rs` | every D1 row, the benign cases, the `next` table, `Retry-After` forms, the ISSUE line |
| `crates/web/tests/charset.rs` | D3 order and labels |
| `crates/web/tests/cut.rs` | D6 rules, chaining, headings |
| `crates/web/tests/compact.rs` | D7 passes, the ≥ 15 % / 0 % targets, fence exemption |
| `crates/web/tests/sources.rs` | D11 table positive/negative, renderers on saved samples |
| `crates/web/tests/fetch_retry.rs` | D2 attempts/budget/`Retry-After`, D4 default headers and ladder, D12 inflate flag |
| `crates/web/tests/fetch_ssrf.rs` (extended) | new hostname suffixes, `64:ff9b::7f00:1`, `240.0.0.1` refused |
| `crates/firecrawl-native/tests/corpus.rs` | every fixture: golden + expect.json |
| `crates/firecrawl-native/src/extract.rs` tests | score formula, penalties, ties, invariant 1, invariant 2, noscript, depth guard |
| `crates/firecrawl-native/src/noise.rs` tests | each valve, consent prefixes, structural ids |
| `crates/firecrawl-native/src/islands.rs` tests | sources, filters, caps |
| `crates/firecrawl-native/src/crawler.rs` tests | `compile_path_patterns` reports the bad one and compiles the rest |
| `crates/scrape/src/crawl.rs` tests (wiremock) | D10 dict, skipped, invalid patterns, frontier rows, sitemap partial |
| `crates/scrape/src/engine.rs` tests | island rung before render, issue carried from fetch, `only_main_content` on render trims link-only blocks |

### 6.3 Live smoke (closer, by hand; recorded in `122-as-built-I.md` as a result matrix)

`https://example.com` (plain, `attempts: 1`, no issue) · a docs site (`https://docs.rs/url/latest/url/`
→ `method: main`, cut with headings) · an HN item (`tier: api`) · a Wikipedia article (`tier: api`,
extract) · a known Cloudflare-challenged host (expect `ISSUE: access_challenge`, then `rendered: true`)
· a Shift_JIS page (`https://www.aozora.gr.jp/` or any `charset=shift_jis` page → readable Japanese)
· `read_url(offset=next_offset)` chaining on the docs page · `crawl` of a 3-page site with
`limit: 2` → `incomplete: true, reason: "limit"`.

## 7. Waves

Contract-first (109 §12's shape): wave C lands only seams, four workers build against them in
parallel, the closer integrates. Every design section is owned by exactly one wave (the table's
"builds" column is exhaustive; a section named nowhere else belongs to the closer).

| wave | builds (design sections) | owns (may edit) | must not touch | gate |
|---|---|---|---|---|
| **C** contract | the frozen stubs of §7.2, `mod` hooks, response-struct fields with serde defaults, `encoding_rs` dependency, `web.accept_language`/`web.header_ladder` with defaults, `CrawlResult`/`ScrapeResponse`/`MapHit.source` types, `FetchArgs.offset/compact/source`, `ScrapeArgs.offset/compact/source`, `CrawlArgs.max_chars_per_page`, the fixture directory with **one** fixture (`docs-page`) and an empty `corpus.rs` that loads it; stubs return `None`/identity/`Default` so everything compiles and today's tests pass | the integration tree `~/Develop/theuth-122-web` on `feat/122-web-reading` | behaviour of any kind | `cargo test -p web -p scrape -p firecrawl-native --no-fail-fast` + one `cargo check --workspace --tests`; writes `122-as-built-C.md` |
| **W1** FETCH | D1 `detect` + surfacing in `read_url` and `search_web` rows; D2; D3; D4; D5 (web side: `meta.rs`, header line); D6 (`cut`, `offset`, `render::fetch_text`); D12 web rails + SSRF denylist; wiring the `compact`/`sources` **stubs** into `read_url` (calls only); `pdf-as-octet-stream` sniff order (§6.1 w2) | `crates/web/src/fetch/**`, `crates/web/src/tools.rs`, `crates/web/src/render.rs`, `crates/web/src/aggregate.rs` (the `skipped` doc comment), `crates/web/src/settings.rs`, `crates/web/src/http.rs`, `crates/web/src/lib.rs` (config fields' use), `crates/web/src/engines/mod.rs` (UA constant only), `crates/web/tests/{issue,charset,cut,fetch_retry,fetch_ssrf}.rs`, `crates/web/tests/fixtures/{shift-jis-page.bytes,pdf-as-octet-stream.bin}` | `crates/web/src/compact.rs`, `crates/web/src/sources.rs`, `crates/web/tests/{compact,sources}.rs`, `crates/scrape/**`, `crates/firecrawl-native/**` | `cargo test -p web --no-fail-fast`; `cargo clippy -p web`; `cargo fmt -p web` |
| **W2** EXTRACT | D8 (`extract.rs`, `noise.rs`, the hook in `transform_html`), D9 (`islands.rs`), D12 extraction rails (layout-table pre-pass, code-language normalisation, depth guard, `srcset` golden), `compile_path_patterns`, D13 corpus (all 16 HTML fixtures + goldens + expect.json) and `corpus.rs` | `crates/firecrawl-native/**` | `crates/web/**`, `crates/scrape/**` | `cargo test -p firecrawl-native --no-fail-fast`; clippy; fmt -p |
| **W3** SCRAPE | engine ladder `fetch → issue → island rung → render` and `only_main_content` on render (D1 scrape side, D9 rung, D8 render tier); D5 scrape side (`extract_metadata` → `PageMeta`); D6 scrape `offset`/markers + crawl per-page cap; D10 (`CrawlResult`, `skipped`, `invalid_patterns`, `MapHit.source` + frontier, sitemap timeout); `ScrapeResponse` filling; wiring `compact`/`sources` **stubs** into `scrape_url` (calls only); tool descriptions of §4.3–§4.5 | `crates/scrape/**` | `crates/web/**`, `crates/firecrawl-native/**` | `cargo test -p scrape --no-fail-fast`; clippy; fmt -p |
| **W4** COMPACT+SOURCES | D7 `compact` and D11 `api_source`/`render` as pure functions with tests and saved JSON samples | `crates/web/src/compact.rs`, `crates/web/src/sources.rs`, `crates/web/tests/{compact,sources}.rs`, `crates/web/tests/fixtures/sources/*.json` | every other file (including `crates/web/src/lib.rs` — C already declares `pub mod compact; pub mod sources;`) | `cargo test -p web --no-fail-fast` (the whole crate; W4's files are the only ones it changes); clippy; fmt -p |
| **Closer** | merge W2, W1, W4, W3 in that order; close every hand-off the as-builts list (W3's call sites against W1's final `FetchResponse`; W1's `read_url` wiring against W4's real `compact`/`sources`; W3's engine against W2's real `select_main`/`extract_islands`, removing the old `engine.rs:383-385` fallback); `crates/theuth/harness.md` prose (§4.6); 16 §6 config table rows; the "no shell with page-derived strings" grep; full gates; live smoke (§6.3); `122-as-built-I.md` with the result matrix | the integration tree only | other worktrees | `cargo check --workspace --tests`; `cargo test -p web -p scrape -p firecrawl-native -p tools -p theuth --no-fail-fast`; `cargo clippy -p web -p scrape -p firecrawl-native`; `just ts-desktop` **only if** a TS binding changed (none expected — no `ts-rs` type is touched) |

Rules for every wave: gates per crate with `--no-fail-fast`; never `cargo fmt --all` (only
`cargo fmt -p <own crate>`); never run the Tauri CLI; each worker writes
`docs/design/122-as-built-<TAG>.md` (what / where / tests / gate output / open items) and ends with
the line `DONE <TAG> <sha>`. Open items from an as-built go into the closer's list, not into a
sibling's tree.

### 7.1 Per-wave bullets

**C — contract**
- New files: `crates/web/src/fetch/{issue,charset,meta}.rs`, `crates/web/src/{compact,sources}.rs`,
  `crates/firecrawl-native/src/{extract,noise,islands}.rs`; `mod` lines in `fetch/mod.rs`
  (`pub mod issue; pub mod charset; pub mod meta;`), `crates/web/src/lib.rs` (`pub mod compact; pub
  mod sources;`), `crates/firecrawl-native/src/lib.rs` (`pub mod extract; pub mod noise; pub mod
  islands;` with `pub use`).
- `compile_path_patterns` in `crawler.rs` (real implementation — it is nine lines over
  `compile_path_regex`; C may land it complete).
- `FetchResponse` fields (all `#[serde(default)]`): `issue`, `meta`, `attempts` (default 1),
  `ladder`, `charset` (default `"utf-8"`), `compacted_chars`, `next_offset`, `omitted_headings`,
  `tier` (default `"fetch"`), `source_kind`, `notes`. `FetchArgs`: `offset`, `compact`, `source`
  (`Source` enum). `ScrapeResponse` struct and `ScrapeUrlArgs.offset/compact/source`;
  `CrawlArgs.max_chars_per_page`; `CrawlResult`, `CrawlPage`, `Skipped`; `MapHit.source` (default
  `"page"`); `Page.issue/meta/charset/truncated/next_offset` in `crawl.rs`.
- `encoding_rs = "0.8"` workspace dep + `crates/web/Cargo.toml` line; `WebConfig.accept_language`
  and `header_ladder` with defaults and 16 §6 doc comments.
- Fixture skeleton: `crates/firecrawl-native/tests/fixtures/docs-page.{html,expected.md,expect.json}`
  and `tests/corpus.rs` that runs the pipeline and compares the golden (so W2 extends a working
  harness).
- Stubs: `detect → None`, `decode → (from_utf8_lossy, "utf-8")`, `meta::extract → PageMeta::default()`,
  `cut → Cut { text: hard_truncate(..).0, truncated, next_offset: None, omitted_headings: vec![] }`,
  `compact → Compacted { text: input, removed_chars: 0 }`, `api_source → None`, `render → None`,
  `select_main → Selection { html: input, scoped: false, method: Body }`, `may_strip → true`,
  `is_noise → false`, `extract_islands → None`.

**W1 — FETCH** (reads §3 D1–D6, D12, §4.1–§4.2, §5)
- Order: D3 (`decode`, so `detect` and `meta` see real text) → D2 retry loop + budget → D1 `detect`
  in `fetch_kind`/`read_body` and the non-2xx-with-issue path → D4 headers + ladder → D5 `meta.rs`
  + header line → D6 `cut` + `offset` + `fetch_text` → D12 rails + denylist → `search_web` skip
  reasons → the §6.1 w2 sniff-order change.
- `read_url` wiring: `compact(..)` and `api_source(..)`/`render(..)` are called where §3 D7/D11 say,
  against C's stubs; W1 never edits those two files.
- Hand-offs to the closer: the `FetchResponse` fields' final doc comments; any renamed helper W3
  might call (W3 codes against C's names).

**W2 — EXTRACT** (reads §3 D8, D9, D12, D13, §6.1)
- Order: corpus first (all 16 fixtures with provisional goldens) → `noise.rs` valves → `extract.rs`
  scoring + invariants → the `transform_html` hook (after `html.rs:584`, before image resolution)
  → `islands.rs` → layout-table + code-language pre-pass → depth guard → `compile_path_patterns`
  tests → regenerate goldens with `UPDATE_GOLDEN=1` and read every diff.
- Must keep every existing `crawler.rs` test green (22 upstream tests, `crawler.rs:810-1308`).
- Hand-offs: the final `Method` for each fixture (`expect.json`) so W3's engine tests can assert
  `tier`/`method`; the `select_main` behaviour the closer needs to remove `engine.rs:383-385`.

**W3 — SCRAPE** (reads §3 D1 scrape side, D5, D6, D8 render tier, D9, D10, §4.3–§4.5)
- Order: `ScrapeResponse` filling + `cut` on `scrape_url` → engine ladder with the island rung →
  `only_main_content` on render → `meta` mapping → `CrawlResult` + per-page cap + `skipped` +
  `invalid_patterns` → `MapHit.source` + frontier → sitemap timeout → descriptions.
- Builds against C's stubs only: `detect` returns `None` in W3's tree, so the ladder tests use a
  `Fetched` constructed with `issue: Some(..)` directly.
- Hand-offs: the list of `FetchResponse` fields W3 reads (so the closer checks them against W1's).

**W4 — COMPACT+SOURCES** (reads §3 D7, D11)
- Pure functions, no network; the saved JSON samples are hand-written minimal documents in the
  documented API shapes (no captured copyrighted bodies beyond field names and short synthetic text).
- Hand-offs: none expected; W4's two files are called from W1's and W3's wiring by C's names.

### 7.2 Frozen stub signatures (what workers compile against)

`crates/web/src/fetch/issue.rs`
```rust
pub enum IssueKind { AccessChallenge, RateLimited, LoginRequired, ConsentWall, ErrorPage, EmptyShell, Unsupported }
pub enum Next { Retry, Rendered, ImportCookies, Handoff }
pub struct PageIssue { pub kind: IssueKind, pub detail: String, pub retry_after: Option<std::time::Duration>, pub next: Vec<Next> }
pub fn detect(status: u16, headers: &reqwest::header::HeaderMap, final_url: &url::Url, body: &str, prose_chars: usize) -> Option<PageIssue>
```

`crates/web/src/fetch/charset.rs`
```rust
pub fn decode(bytes: &[u8], content_type: Option<&str>) -> (std::borrow::Cow<'_, str>, &'static str)
```

`crates/web/src/fetch/meta.rs`
```rust
pub struct PageMeta { pub title: Option<String>, pub description: Option<String>, pub author: Option<String>, pub published: Option<String>, pub lang: Option<String>, pub canonical: Option<String>, pub site_name: Option<String> }
pub fn extract(html: &str, base: &url::Url) -> PageMeta
```

`crates/web/src/fetch/text.rs` (added beside `hard_truncate`)
```rust
pub struct Cut { pub text: String, pub truncated: bool, pub next_offset: Option<usize>, pub omitted_headings: Vec<String> }
pub fn cut(s: &str, offset: usize, max_chars: usize) -> Cut
```

`crates/web/src/compact.rs`
```rust
pub struct CompactOptions { pub fold_links: bool, pub keep_images: bool }
pub struct Compacted { pub text: String, pub removed_chars: usize }
pub fn compact(markdown: &str, opts: &CompactOptions) -> Compacted
```

`crates/web/src/sources.rs`
```rust
pub struct ApiSource { pub kind: &'static str, pub fetch_url: url::Url }
pub fn api_source(url: &url::Url) -> Option<ApiSource>
pub fn render(kind: &str, body: &str) -> Option<String>
```

`crates/web/src/fetch/mod.rs` (response and args; every new field `#[serde(default)]`)
```rust
pub struct FetchResponse {
    pub url: String, pub title: Option<String>, pub content: String, pub truncated: bool, pub took_ms: u64,
    pub issue: Option<issue::PageIssue>, pub meta: meta::PageMeta, pub attempts: u8, pub ladder: bool,
    pub charset: String, pub compacted_chars: usize, pub next_offset: Option<usize>,
    pub omitted_headings: Vec<String>, pub tier: String, pub source_kind: Option<String>, pub notes: Vec<String>,
}
```
`crates/web/src/tools.rs`
```rust
#[serde(rename_all = "lowercase")] pub enum Source { #[default] Auto, Page, Api }
pub struct FetchArgs { pub url: String, pub max_chars: Option<u32>, pub format: Option<FetchFormat>, pub rendered: Option<bool>,
    pub offset: Option<u32>, pub compact: Option<bool>, pub source: Option<Source> }
```

`crates/firecrawl-native/src/extract.rs`
```rust
pub enum Method { Main, Article, Scored, Body }
pub struct SelectOptions { pub explicit_scope: bool, pub only_main_content: bool }
pub struct Selection { pub html: String, pub scoped: bool, pub method: Method }
pub fn select_main(html: &str, original: &str, opts: &SelectOptions) -> Selection
```

`crates/firecrawl-native/src/noise.rs`
```rust
pub fn may_strip(node: &kuchikiki::NodeRef) -> bool
pub fn is_noise(node: &kuchikiki::NodeRef) -> bool
```

`crates/firecrawl-native/src/islands.rs`
```rust
pub fn extract_islands(html: &str) -> Option<String>
```

`crates/firecrawl-native/src/crawler.rs` (beside `compile_path_regex`, which does not change)
```rust
pub fn compile_path_patterns(patterns: &[String]) -> (Vec<regex::bytes::Regex>, Vec<String>)
```

`crates/scrape/src/tools.rs`
```rust
pub struct ScrapeResponse { pub url: String, pub title: Option<String>, pub tier: String,
    pub markdown: Option<String>, pub html: Option<String>, pub links: Option<Vec<String>>,
    pub issue: Option<web::fetch::issue::PageIssue>, pub meta: web::fetch::meta::PageMeta, pub charset: String,
    pub compacted_chars: usize, pub truncated: bool, pub next_offset: Option<usize>, pub omitted_headings: Vec<String>,
    pub source_kind: Option<String>, pub notes: Vec<String> }
pub struct Skipped { pub url: String, pub error: String }
pub struct CrawlPage { pub url: String, pub title: Option<String>, pub markdown: String, pub tier: String,
    pub issue: Option<web::fetch::issue::PageIssue>, pub meta: web::fetch::meta::PageMeta, pub truncated: bool,
    pub next_offset: Option<usize>, pub html: Option<String> }
pub struct CrawlResult { pub pages: Vec<CrawlPage>, pub incomplete: bool, pub reason: Option<&'static str>,
    pub skipped: Vec<Skipped>, pub invalid_patterns: Vec<String> }
// ScrapeUrlArgs gains offset: Option<u32>, compact: Option<bool>, source: Option<web::tools::Source>;
// CrawlArgs gains max_chars_per_page: Option<u32>.
```
`crates/scrape/src/crawl.rs`
```rust
pub struct MapHit { pub url: String, pub title: Option<String>, pub source: &'static str }
// Page gains: issue, meta, charset, truncated, next_offset (same types as CrawlPage).
```

`crates/web/src/lib.rs` (`WebConfig`)
```rust
pub accept_language: String,   // "en-US,en;q=0.9"
pub header_ladder: bool,       // true
```

### 7.3 Merge order and integration

`W2 → W1 → W4 → W3`. W2 first because its crate has no dependants among the others' edits and its
goldens are the reference the closer reads diffs against; W1 next (the fetcher everything calls);
W4 before W3 so `scrape_url`'s wiring meets the real `compact`/`sources`; W3 last because it reads
every other wave's types. Same-new-file conflicts are impossible by ownership; the two expected
hand-edits are `engine.rs:383-385` (remove the old fallback once `select_main` is real) and the
`FetchResponse` doc comments. The closer re-runs `cargo test -p scrape` after each of the last two
merges, then the full gate list. Landing on `main` is Sky's `merge --no-ff` as in 109/110.

## 8. Out of scope / follow-ups

- **TLS fingerprinting** (wreq/BoringSSL impersonation): rejected (comparison §4); the render tier is
  theuth's answer to JS challenges.
- **Proxy pools and proxy config for plain fetches**: not in this design; `browser.proxy` exists for
  the render tier.
- **Cookie jar on the plain path**: not built; §5 cookie rule. A later design may add an opt-in jar
  scoped per host and per session, behind the same import approval as `browser_import_cookies`.
- **`http_request` risk class**: `Risk::Mutating` for GET/HEAD without a body pushes agents to
  `read_url`; a `ReadOnly` classification for safe methods is a one-line policy change with a
  security review of its own.
- **`browser.allow_private_network` vs the fetcher's denylist**: the render tier can reach what the
  fetcher refuses when that flag is set; reconcile in the browser design.
- **Caching / conditional GET**: no per-session page cache, no ETag/If-Modified-Since on
  `read_url` (16 §12.3 stands); `rss.py` does it through `http_request`.
- **Page change tracking**: snapshot-and-diff of pages is a routine/sensor concern (40/108).
- **Batch tool** (`read_urls` with shared per-host politeness): not built; `search_web` and `crawl`
  remain the bulk readers.
- **Readability-style sibling/parent propagation** in the scorer: deliberately not — one candidate,
  two invariants; revisit if the corpus grows pages whose content is split across siblings.
- **JSON-LD as structured output**: `meta` takes `author`/`datePublished`/`articleBody` only; no
  `structured_data` field.
- **`read_url` link absolutisation** (survey gap 7): a small follow-up in `fetch/text.rs` once the
  corpus exists; not in these waves to keep W1's scope to the brief.

## 9. Open questions (none blocks a wave)

1. `(call)` `CHARS_PER_WORD = 6` for the word thresholds of D1 — the corpus will say whether 5 or 6
   fits English pages better; the constant is in one place.
2. `(call)` The ladder's pinned Chrome UA/`sec-ch-ua` values: a yearly bump by hand, or read from
   obscura's UA at runtime once `web` can see it without a dependency cycle.
3. `(call)` Whether `ConsentWall`/`EmptyShell` on a 2xx should also set `truncated`-style prominence
   in `search_web`'s text rows (today: the skip reason only).
4. Should `crawl`'s `skipped` cap (200 rows) be configurable? Left fixed until a crawl needs it.
5. The `"cancelled"` reason: wire it if the tool invocation exposes a cancellation token to the
   crawl future at integration time; otherwise drop the variant in the as-built.
6. `map_site` frontier rows have no title and no `lastmod`; sitemap `lastmod` ordering is a cheap
   later addition.
7. Whether `format: text` should fold links by default (D7 pass 10 does) or keep bare URLs — the
   token cost comparison 16 §12 asked for is still unmeasured.

## Revision 1 (2026-10-07 00:45) — stealth fetch client

Sky's words (2026-10-07 00:45, relayed from his design-123 session): *"I want the scraping fetching
etc tools to be stealth as well."*

This revision supersedes D4's "no TLS fingerprinting, no proxy pool" posture for the fetch tools,
extends D1's vendor table, adds wave W5 and changes the C/W1 rows of §7. Everything else in the
document stands; the "Supersedes" list at the end of this section names each replaced statement.

### R1.0 Verified facts

- The browser tier is already stealth: obscura's `stealth` feature is wreq/BoringSSL TLS fingerprint
  emulation plus a tracker blocklist, enabled unconditionally in the workspace
  (`Cargo.toml:275-278`), and `BrowserConfig.stealth` defaults to `true`
  (`crates/browser/src/lib.rs:45-46`, `:78`).
- `wreq 6.0.0-rc.29` and `wreq-util 3.0.0-rc.12` are already in `Cargo.lock` through obscura
  (`Cargo.lock:25553-25555`, `:25618-25620`). The two-BoringSSL symbol collision with libsignal's
  `boring-sys` is already solved by the `btls-sys` fork with `prefix-symbols` (`Cargo.toml:391-399`).
  Adding `wreq` as a direct dependency of `crates/web` at exactly those versions adds no native code
  and no second BoringSSL build; `cargo tree -p web -i wreq` must show one version (C records it).
- Today every page fetch of `read_url`/`scrape_url`/`crawl`/`map_site` goes out as plain
  `reqwest 0.13` + rustls (`Cargo.toml:169`; `crates/web/src/fetch/mod.rs:136-142`) — a non-browser
  TLS/HTTP-2 fingerprint that Cloudflare-class vendors score as a bot regardless of headers. D4's
  header ladder could therefore never pass a fingerprint-class block; it only helped header-varying
  servers.
- `antibot-rs` 0.3.4 (crates.io, MIT, created 2026-04-14, last release 2026-07-05, 481 downloads,
  one maintainer) was vetted and **rejected** as a dependency: its `detect.rs` is ~7.6 KB of marker
  matching, but the crate is an auto-managed Byparr/FlareSolverr client (`docker.rs` 20 KB,
  `client.rs` 43 KB, tokio `process`, dashmap) and depends on `reqwest` with default features
  (native-tls → openssl-sys), while this tree deliberately has no OpenSSL. Its vendor taxonomy
  (Cloudflare, Turnstile, DataDome, PerimeterX, Akamai, Imperva, reCAPTCHA) is adopted into D1's
  table (R1-D2) instead.

### R1-D1 Stealth fetch client (supersedes D4's "no TLS fingerprinting"; D4's header rules otherwise stand)

**Rationale.** The browser tier already presents a Chrome fingerprint; the plain fetch tools present
rustls. Sky wants the fetch tools stealth as well. One seam — an `HttpClient` trait with two
implementations — lets the `Fetcher` keep its redirect loop, guard, caps and retries unchanged and
swap only the wire client.

**Types** (`crates/web/src/fetch/client.rs`, *frozen*, landed by C as the contract addendum):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind { Reqwest, WreqChrome }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Impersonate { Never, OnChallenge, #[default] Always }

pub struct ClientConfig { pub user_agent: String, pub accept_language: String, pub connect_timeout: Duration, pub read_timeout: Duration, pub max_body: usize }

pub enum ClientError { NotBuilt, Transport(String), Build(String) }

pub struct ClientResponse { pub status: u16, pub headers: HeaderMap, pub final_url: url::Url, pub body: BoxStream<'static, Result<Bytes, ClientError>> }

#[async_trait]
pub trait HttpClient: Send + Sync {
    fn kind(&self) -> ClientKind;
    async fn get(&self, url: &url::Url, headers: &[(HeaderName, HeaderValue)]) -> Result<ClientResponse, ClientError>;
}

pub fn build(kind: ClientKind, cfg: &ClientConfig) -> Result<Arc<dyn HttpClient>, ClientError>;
```

- `Reqwest` = today's client behind the trait (C lands it real): `user_agent`, `connect_timeout`,
  `read_timeout` as the whole-request timeout, `redirect::Policy::none()`, `ssrf::GuardedResolver`
  (`fetch/mod.rs:136-142` moved behind `build`). Redirects are never followed by a client; the
  `Fetcher` loop re-validates SSRF per hop exactly as today (`fetch/mod.rs:199-233`), so
  `ClientResponse.final_url` is the requested URL.
- `WreqChrome` = W5: a `wreq::Client` with `wreq_util` Chrome emulation — the newest desktop Chrome
  profile that wreq-util 3.0.0-rc.12 offers, macOS platform (W5 names it in its as-built);
  redirects **disabled**; the same SSRF-guarded DNS resolution — route A: wreq's resolver hook
  carrying `ssrf::resolve_checked`; route B, if wreq cannot take a custom resolver: resolve first
  with the guarded resolver and connect by IP with `Host`/SNI preserved — W5 picks the first route
  that works and documents it; `connect_timeout`/`read_timeout` from `ClientConfig`; no cookie
  store; HTTP/2 via the profile; `Accept-Language` from config appended after the profile headers;
  UA = the profile's own Chrome UA (a Chrome fingerprint with a `theuth/…` UA is self-defeating; the
  honest UA is sent only under `Impersonate::Never`). The body streams with the same post-inflate cap
  semantics as the reqwest path (`read_capped`, `fetch/mod.rs:342-373`, now over `ClientResponse.body`).
- `ClientConfig` is built from `WebConfig` (`user_agent`, `accept_language`, `CONNECT_TIMEOUT`,
  `TIMEOUT`, `max_fetch_bytes`); the scrape crate builds its own with its 8 MiB cap and
  `page_timeout_ms` as it does for the fetcher today (`crates/scrape/src/lib.rs:85-90`).

**Config.** `web.impersonate: never | on_challenge | always`, default **`always`** (Sky's words).

| value | behaviour |
|---|---|
| `always` | the first request of every page fetch uses `WreqChrome` |
| `on_challenge` | `Reqwest` first; on a detected `AccessChallenge`, or `RateLimited` on 403/503 with a D1 marker, one `WreqChrome` attempt within the same `TOTAL_BUDGET` |
| `never` | `Reqwest` only |

`http_request` (the API tool) and the SERP engines keep `Reqwest` regardless — APIs do not
fingerprint and the engines keep their own identity (`engines/mod.rs:64`). `search_web`'s page
reads (the excerpt fetches through the `scrape` provider and the raw-fetch fallback,
`tools.rs:220-295`) follow `web.impersonate` like `read_url`.

**D4 collapses into this rung.** Under `always` the header set *is* the Chrome profile's; under
`on_challenge` the second attempt is the `WreqChrome` attempt. There is no separate "Chrome headers
on reqwest" attempt and no dated Chrome header constant — both are removed from D4. D4's remaining
rules stand: `Accept`/`Accept-Language` always sent (on `Reqwest` as defaults, on `WreqChrome` as the
profile's plus the configured `Accept-Language`), the versioned honest UA under `never`, no proxy
pool, no cookies. `web.header_ladder` is kept as a config key and now means "escalate from `Reqwest`
to `WreqChrome` on a challenge" — it is what `on_challenge` consults; `header_ladder: false` under
`on_challenge` behaves as `never`. `(call)` Under `always` the key is ignored.

**Response fields.** `FetchResponse.client: ClientKind` (default `reqwest`; C's addendum), also on
`ScrapeResponse` and `CrawlPage`. `attempts` counts every request made, so an `on_challenge`
escalation reads `attempts: 2`; `ladder: bool` keeps its meaning "a rung after the first succeeded".
`(call)` When C's stub returns `ClientError::NotBuilt`, the `Fetcher` falls back to `Reqwest` and
appends `notes: ["stealth client not built; fetched with reqwest"]` — W1's tests pass against the
stub and the closer watches that note disappear once W5 lands.

**What the model sees.** The text rendering adds nothing unless `client != reqwest` **and** an issue
or a ladder occurred; then ` · client: wreq_chrome` is appended to the ISSUE line (issue) or to the
meta line (ladder). `(call)` The structured field always carries `client`, so every fetch is auditable.

**The escalation ladder as a whole:**

1. `WreqChrome` fetch (or `Reqwest` first under `on_challenge`/`never`), retries and budget per D2;
2. the data-island rung (D9) for thin pages;
3. obscura stealth render (`rendered: true`, D1 `Next::Rendered`);
4. `browser_import_cookies` — the user's own consented cookies (`Next::ImportCookies`);
5. `desktop_handoff` (`Next::Handoff`); then stop honestly with the `PageIssue`.

Policy line: **"escalate only on a detected challenge, never past a robots disallow, always report
which client fetched the page."**

**robots.txt.** `crawl`/`map_site` keep honouring robots.txt and `Crawl-delay` (`crawl.rs:19-20`,
`:555-565`). Under `always` the robots file is fetched with the same client; `(call)` rules are
matched for `*` **and** for `theuth` (`ROBOTS_UA`, `crawl.rs:20`) and a disallow from either applies
— the Chrome UA token is not a bot token, and matching only `*` would silently loosen a site's
explicit `theuth` rule. `read_url`/`scrape_url` single-page reads do not consult robots (unchanged;
a browser visit does not either).

**Tests for W5** (`crates/web/tests/client_wreq.rs`, wiremock over plain HTTP, no network): the
`WreqChrome` client sends the profile's UA, `sec-ch-ua` and the configured `accept-language`; it does
not follow a 302 (the response is returned with `status: 302`); a 5 MiB body is streamed and capped
at `max_body` with the cap flagged; a private-IP target is refused through the guard (route A or B);
`connect_timeout`/`read_timeout` are honoured against a sleeping mock; a unit test that
`build(WreqChrome, &cfg)` succeeds; `cargo tree -p web -i wreq` shows one version (asserted in the
as-built, not in a test). W1's dispatch tests (`fetch_retry.rs`): `always` → one request, `client ==
wreq_chrome` (against C's stub: `reqwest` plus the `NotBuilt` note); `on_challenge` → a 403 with
`cf-mitigated` then 200 → `attempts: 2`, `ladder: true`; `never` → `reqwest` and no second attempt.

**Follow-ups (not built):** a FlareSolverr/Byparr solver behind a `web.solver_url` (needs its own
security review — it ships page content to another process); proxy pools.

### R1-D2 Detector vendor table extended (amends D1)

Four vendor rows are added to D1's marker table, inserted after row 7 (Akamai) with the same size
gating and false-positive discipline (exact substrings, size gates, title-only rows gated on prose):

| # | kind | signal | gate |
|---|---|---|---|
| 7a | `AccessChallenge` | PerimeterX/HUMAN: `_pxhd`, or `px-captcha`, or `perimeterx` | body < 50 000 bytes |
| 7b | `AccessChallenge` | Imperva/Incapsula: `_Incapsula_Resource`, or `incapsula`, or `visid_incap` | body < 50 000 bytes |
| 7c | `AccessChallenge` | reCAPTCHA interstitial: `google.com/recaptcha/api.js` **and** `g-recaptcha` | body < 30 000 bytes **and** prose < 300 chars (50 words) — a normal form page embedding reCAPTCHA is not an issue |
| 7d | `AccessChallenge` | Kasada: `kpsdk`, or `/ips.js` | body < 30 000 bytes |

`detail` vocabulary (fixed; the part after `HTTP <n> · `): `cloudflare`, `cloudflare:turnstile`,
`cloudflare:1020`, `akamai`, `datadome`, `aws-waf`, `hcaptcha`, `perimeterx`, `imperva`,
`recaptcha`, `kasada`, `consent:<platform>` (`onetrust`, `cookiebot`, `trustarc`, … or `url`),
`login`, `error-page`, `empty-shell`; row 8's antibot titles use the vendor when a vendor row also
matched, else `antibot-title`. The D1 example `HTTP 403 · cloudflare challenge` becomes
`HTTP 403 · cloudflare`.

antibot-rs vetting, for the record: its taxonomy is what these rows adopt; the crate itself was
rejected because it is a solver client with default-feature `reqwest` (OpenSSL) in a tree that has
none, not because its markers are wrong.

**Fixtures and tests.** W2 adds four synthesised fixtures to §6.1 — `px-block`, `incapsula-block`,
`recaptcha-interstitial`, `kasada-block` (expected `issue: access_challenge`, `tier_hint: render`,
`method: body`) — plus the benign twin `recaptcha-form-benign` (a 400-word contact page embedding
reCAPTCHA → `issue: null`). W1 adds the matching `issue.rs` rows and the benign case.

### R1-D3 Wave changes (supersedes the §7 table rows for C and W1; adds W5)

| wave | change |
|---|---|
| **C addendum** (already requested of the running contract worker) | `crates/web/src/fetch/client.rs` with the `Reqwest` implementation real and `WreqChrome` → `Err(ClientError::NotBuilt)`; `FetchResponse.client` / `ScrapeResponse.client` / `CrawlPage.client` (serde default `reqwest`); `WebConfig.impersonate` (default `always`); `wreq = "=6.0.0-rc.29"` and `wreq-util = "=3.0.0-rc.12"` in the workspace table and `crates/web/Cargo.toml`; `cargo tree -p web -i wreq` output in `122-as-built-C.md` |
| **W1 FETCH** | drops the dated Chrome header constant and the "Chrome headers on reqwest" attempt; implements the `Impersonate` dispatch in the `Fetcher` (`always`/`on_challenge`/`never`) against the `HttpClient` trait, treating `ClientError::NotBuilt` as "fall back to `Reqwest` with a note"; adds rows 7a–7d and the benign reCAPTCHA case to `issue.rs` and its tests; the `detail` vocabulary. **Must not touch `client.rs`.** Owns list otherwise unchanged |
| **W5 STEALTH** (new) | owns `crates/web/src/fetch/client.rs` **only**, plus `crates/web/tests/client_wreq.rs`; builds the `WreqChrome` arm as R1-D1 specifies; must not touch any other file (dependencies are in place from C). Gate: `cargo test -p web --no-fail-fast`, `cargo clippy -p web --tests -- -D warnings`, `cargo fmt -p web`, `cargo tree -p web -i wreq` (one version). As-built `122-as-built-W5.md` names the emulation profile and the resolver route taken |
| **W2 EXTRACT** | adds the four vendor fixtures and the benign twin (R1-D2) |
| **W3 SCRAPE** | fills `ScrapeResponse.client`/`CrawlPage.client` from `FetchResponse.client`; robots fetch matched for `*` and `theuth` (R1-D1) |
| **Closer** | merge order becomes **`W2 → W1 → W5 → W4 → W3`**; removes W1's `NotBuilt` fallback note once W5 is merged; runs the §6.3 live smoke under `impersonate: always` **and** `never` for the same URL list, recording `client` per row in the result matrix; 16 §6 config table gains `impersonate` |

W5 runs in parallel with W1–W4 (disjoint files); it needs C's `client.rs` stub and the pinned
dependencies, nothing from the other workers.

### R1-D4 Security additions to §5

- The `WreqChrome` client passes through the identical SSRF guard — guarded resolution (route A or
  B) plus the per-hop re-check in the `Fetcher` loop (`fetch/mod.rs:200`) — the identical size and
  time caps (`read_capped`, `TOTAL_BUDGET`), no cookies, no proxy.
- Impersonation never bypasses `robots.txt` in `crawl`/`map_site`; a disallow for `*` or `theuth`
  applies whichever client fetched the file.
- `Impersonate::Never` remains available per installation; the posture is a config choice, not a
  build choice.
- The surfaced `client` field makes every fetch auditable: a transcript shows which identity went
  out for each page.
- The honest UA is still what `http_request`, the SERP engines and `never` send; `robots.txt` still
  sees `theuth` rules honoured (R1-D1).

### R1-D5 Config table (§4.7) addition

| key | default | effect |
|---|---|---|
| `web.impersonate` | `always` | `never`: reqwest + rustls with the honest UA on every page fetch; `on_challenge`: honest first, Chrome-fingerprint `wreq` client on a detected challenge; `always`: Chrome-fingerprint client first |
| `web.header_ladder` | `true` | restated: "escalate from `Reqwest` to `WreqChrome` on a challenge" — consulted only under `on_challenge`; ignored under `always` and `never` |

Neither is exposed in Settings › Search; both are documented in 16 §6's table by the closer.

### Supersedes

| earlier statement | replaced by |
|---|---|
| D4: "No TLS fingerprinting, no proxy pool, no cookie jar"; the browser-realistic `LADDER_HEADERS` attempt on reqwest with a dated Chrome UA constant | R1-D1: the `WreqChrome` client behind the `HttpClient` trait; `web.impersonate` (`always` default); the ladder is the `on_challenge` escalation; no proxy pool and no cookie jar still hold |
| D4 tests: "a 403 with `cf-mitigated` then 200 → `ladder == true`, the second request carries `sec-fetch-mode`" | R1-D1 W1 dispatch tests: `on_challenge` → `attempts: 2`, `ladder: true`, `client: wreq_chrome`; W5 header tests in `client_wreq.rs` |
| D1 row 8 / example `detail` `HTTP 403 · cloudflare challenge` | R1-D2 `detail` vocabulary: `HTTP 403 · cloudflare` |
| D1 marker table rows 1–17 | R1-D2 adds rows 7a–7d (PerimeterX, Imperva, reCAPTCHA interstitial, Kasada) |
| §4.1 `ladder` description "The browser-realistic header attempt produced this response" | "A rung after the first attempt produced this response"; new field `client` |
| §4.7 config table (`accept_language`, `header_ladder`) | R1-D5: `impersonate` added, `header_ladder` restated |
| §5 "Header ladder constraints … TLS stays rustls with the platform verifier" | R1-D4: TLS is BoringSSL via `wreq` under `always`/`on_challenge`, rustls under `never`; same guard, caps, no cookies |
| §6.1 corpus: 16 HTML rows | + `px-block`, `incapsula-block`, `recaptcha-interstitial`, `kasada-block`, `recaptcha-form-benign` (21 HTML rows) |
| §7 table rows C and W1; merge order `W2 → W1 → W4 → W3` | R1-D3 rows; new W5; order `W2 → W1 → W5 → W4 → W3` |
| §7.2 `FetchResponse`/`ScrapeResponse`/`CrawlPage` signature blocks | each gains `pub client: ClientKind`; `WebConfig` gains `pub impersonate: Impersonate` |
| §8 first bullet "TLS fingerprinting (wreq/BoringSSL impersonation): rejected" | built as R1-D1 for the fetch tools; what stays rejected is importing webclaw's stack or a proxy pool |
| §9 question 2 (the ladder's pinned Chrome UA) | moot: the UA is the wreq-util profile's |

### Updated frozen signatures (addendum to §7.2)

`crates/web/src/fetch/client.rs`
```rust
#[serde(rename_all = "snake_case")] pub enum ClientKind { Reqwest, WreqChrome }
#[serde(rename_all = "snake_case")] pub enum Impersonate { Never, OnChallenge, #[default] Always }
pub struct ClientConfig { pub user_agent: String, pub accept_language: String, pub connect_timeout: Duration, pub read_timeout: Duration, pub max_body: usize }
pub enum ClientError { NotBuilt, Transport(String), Build(String) }
pub struct ClientResponse { pub status: u16, pub headers: HeaderMap, pub final_url: url::Url, pub body: BoxStream<'static, Result<Bytes, ClientError>> }
#[async_trait] pub trait HttpClient: Send + Sync { fn kind(&self) -> ClientKind; async fn get(&self, url: &url::Url, headers: &[(HeaderName, HeaderValue)]) -> Result<ClientResponse, ClientError>; }
pub fn build(kind: ClientKind, cfg: &ClientConfig) -> Result<Arc<dyn HttpClient>, ClientError>;
```

`crates/web/src/fetch/mod.rs`, `crates/scrape/src/tools.rs`, `crates/web/src/lib.rs` (additions)
```rust
// FetchResponse, ScrapeResponse, CrawlPage each gain:
pub client: client::ClientKind,          // serde default: Reqwest
// WebConfig gains:
pub impersonate: fetch::client::Impersonate,   // default Always
```
