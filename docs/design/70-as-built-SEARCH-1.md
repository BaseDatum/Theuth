# 70 — as-built SEARCH-1: real search providers, honest reader, Settings › Search (2026-09-27)

Worktree `/Users/Sky.Moore/Develop/theuth-search`, branch `feat/search-providers-settings` off `b1ff179d`.
Brief `/tmp/69/omp-SEARCH-1.md`; design `16-web.md` §3.3, §6, §13.18. Scratch engine
`THEUTH_DATA_DIR=/tmp/69/search-data` (`rpc.listen 127.0.0.1:7813`), SearXNG `searxng/searxng:latest`
on `127.0.0.1:8089` (json format enabled, limiter off) and OpenSERP `karust/openserp:latest` on
`127.0.0.1:7089`, both in Docker Desktop (came up first try).

## What changed

| Area | Before (b1ff179d) | After |
|---|---|---|
| Providers | four modules, wiremock-only, `{base_url}` / `brave: {}` | `{enabled, base_url?, secret, timeout_ms, priority}` per block; `Provider::timeout()`; SearXNG + OpenSERP exercised live (10/10 probes ok), Brave/Firecrawl by contract; OpenSERP `recent` → `date=` range, "every engine failed" envelope is the failure; Brave `page_age` preferred |
| Registration | once at start | `SearchProviders` actor: `GetProviders` / `SetProviders` persist + re-register with `web`; `SecretsEvent::Changed` re-syncs (store a key in Settings → provider registered, delete → unregistered, both seen live) |
| Degraded | none; Wikipedia-only results looked fine | `SearchResponse.degraded` + `reasons` (`brave: rate_limited`, `qwant: captcha`, …); text starts `WARNING: degraded search …`; `web.degraded_is_error` |
| Health | none | `web::health::Health` → `ProviderStatus {ok, kind, detail, last_ok_at, last_error_at}` per provider, `search_status` RPC, Status section |
| Bing | in default engines; 14/22 "response shape may have changed" | out of `DEFAULT_ENGINES` (still in `BUILTIN`, opt-in); `class="b_no"` page classified `Blocked`. Fresh probe 2026-09-27: 3 of 4 queries got the empty page |
| Rows | `site=` only gated reads; Wikipedia panels, mirrors, forks, aggregators ranked freely | `rank::refine`: off-site, stale (`recent`), off-topic Wikipedia, mirrors dropped; forks and version aggregators sink; undated rows after dated; every drop a `notes[]` line |
| Reads | title-first two-pass (read ranks 2,5,7,8 for "bun"), `pages == []` on zero read, 25 s per page | rank order; one `pages[]` entry per row with `skipped: not-relevant \| budget \| error: …`; phase budget 6 s, render cap 3 s |
| `published` | verbatim ("2 weeks ago", snippet text once) | `date::normalize` → RFC 3339 UTC or absent |
| Settings | — | Settings › Search tab + `/settings/search`; RPCs `search_settings_get/set`, `search_provider_test`, `search_status`, `search_test` |

Config keys added: `web.read_budget_ms`, `render_cap_ms`, `recent_default`, `keyless_fallback`,
`degraded_is_error`; `search_providers.<block>.{enabled, secret, timeout_ms, priority}`. Removed:
`web.read_timeout_ms` (the phase budget replaces it). `WebActor::new` takes `SharedConfig` + config
path; `aggregate::search` and `select` gained `Health` / `keyless_fallback`.

## Evidence: 12-query set, SearXNG + OpenSERP on, same network, same hour

Before = `b1ff179d` through a throwaway `search-providers/examples/eval.rs` calling `SearchTool::call`
directly (the baseline has no RPC to invoke a tool); after = `search_test` RPC on the scratch engine.
`canonical rank` = rank of the first row on the query's canonical host; `-` = none / n.a.

Before:

| query | took_ms | rows | read | skipped | canonical rank | degraded | failed engines | notes |
|---|---|---|---|---|---|---|---|---|
| bun latest version | 4806 | 8 | 4 | - | 1 | no | qwant | - |
| iroh release notes recent=month | 6099 | 8 | 3 | - | 1 | no | duckduckgo, google | - |
| takumi-pdf fonts | 6986 | 8 | 5 | - | 1 | no | google, qwant | - |
| shadcn-labs pdfcn | 4548 | 8 | 5 | - | 1 | no | duckduckgo, google, qwant | - |
| xterm.js DOM renderer colors | 5655 | 8 | 4 | error:1 | 1 | no | bing, google, openserp, qwant | - |
| Ghostty COLORTERM | 4624 | 8 | 3 | - | 1 | no | bing, duckduckgo, google, qwant, wikipedia | - |
| PrintToPdf site=learn.microsoft.com | 2669 | 8 | 0 | - | 1 | no | bing, brave, google, qwant | - |
| monty python runtime sandbox rust | 6393 | 8 | 5 | - | 1 | no | bing, duckduckgo, google, qwant | - |
| Anthropic announcement recent=week | 4965 | 8 | 2 | - | 1 | no | brave, duckduckgo, google | - |
| Rust Tauri WebView2 Drucken als PDF | 5730 | 8 | 4 | error:1 | - | no | bing, brave, google, qwant | - |
| tokio JoinSet abort_all | 4179 | 8 | 2 | error:1 | 1 | no | brave, duckduckgo, google, qwant | - |
| xqzv plorf wibblesnatch 9938 | 4005 | 8 | 0 | - | - | no | brave, google, openserp, qwant | - |

median 4965 ms, max 6986 ms. Not visible in the table but in the JSON: 13 Wikipedia rows across the
set (`Greg_Baldwin`, `Avatar: The Last Airbender` for "iroh"), 3 off-site rows kept under
`site=learn.microsoft.com` (plus `maps.google.com` rows for pdfcn), one `published` holding snippet
text ("Summary Add a Code component to the pdfc…"), read ranks `[2, 5, 7, 8]` for "bun" (the #1
releases page unread), 2 calls with rows but zero reads and no per-row explanation, one page read at
9.9 s.

After:

| query | took_ms | rows | read | skipped | canonical rank | degraded | failed engines | notes |
|---|---|---|---|---|---|---|---|---|
| bun latest version | 5169 | 8 | 5 | budget:3 | 1 | no | brave, google, qwant | - |
| iroh release notes recent=month | 4591 | 6 | 5 | budget:1 | 2 | no | brave, duckduckgo, google | 2 unrelated Wikipedia rows dropped; dates unavailable for 6 rows (listed after dated ones) |
| takumi-pdf fonts | 4200 | 6 | 5 | budget:1 | 1 | no | brave, google, qwant | 2 unrelated Wikipedia rows dropped |
| shadcn-labs pdfcn | 3893 | 8 | 5 | budget:3 | 1 | no | brave, qwant | 1 version-aggregator rows listed last |
| xterm.js DOM renderer colors | 5723 | 7 | 3 | budget:2, error:2 | 1 | no | brave, duckduckgo, google, qwant | 1 mirror rows dropped (canonical repository present) |
| Ghostty COLORTERM | 8498 | 8 | 4 | budget:3, error:1 | 1 | no | brave, qwant, wikipedia | - |
| PrintToPdf site=learn.microsoft.com | 4954 | 2 | 2 | - | 1 | no | brave, duckduckgo, google, qwant | 6 off-site rows dropped (site=learn.microsoft.com) |
| monty python runtime sandbox rust | 3689 | 8 | 5 | budget:3 | 1 | no | brave | - |
| Anthropic announcement recent=week | 3783 | 5 | 5 | - | 1 | no | brave, duckduckgo, google | 3 unrelated Wikipedia rows dropped; dates unavailable for 5 rows (listed after dated ones) |
| Rust Tauri WebView2 Drucken als PDF | 3556 | 8 | 4 | not-relevant:4 | - | no | brave, duckduckgo, google, qwant | - |
| tokio JoinSet abort_all | 5705 | 8 | 3 | budget:3, error:2 | 1 | no | brave, qwant | - |
| xqzv plorf wibblesnatch 9938 | 3241 | 8 | 0 | not-relevant:8 | - | no | brave, duckduckgo, google, qwant | - |

median 4591 ms, max 8498 ms (fan-out ≤ 6 s + read budget ≤ 6 s bound the worst case at 12 s). JSON:
1 Wikipedia row (the on-topic `Bun (software)`), 0 off-site rows, 0 snippet-as-published, read ranks
`[1, 2, 3, 4, 5]` for "bun" and "pdfcn", the only zero-read call is the junk query (all 8 rows
`not-relevant`), 0 pages over 3.5 s (`error:` entries are `render cap of 3.0s exceeded` on
github.com issue pages and x.com, or `no readable text` on JS-only pages). `budget:N` = rows past
the `read` slots. "iroh" canonical at rank 2: under `recent=month` a dated fork row precedes the
undated releases page — by design, and the note says so.

Degraded, live: with SearXNG/OpenSERP disabled through `search_settings_set`, the second call
("ghostty COLORTERM") returned `degraded: true`, `reasons ["brave: rate_limited", "google:
rate_limited", "qwant: captcha"]`, text starting `WARNING: degraded search …`; re-enabling through the
same RPC re-registered both without a restart. Two relevance-gate iterations were needed on the live
data: "Anthropic announcement" rows never say "announcement" and "PrintToPdf" pages say "Print to
PDF" (half the tokens, spaceless match), and the Bun releases page says neither "latest" nor
"version" (a row two providers agree on needs one token). Both are regression tests in `tools.rs`.

## Settings › Search, live (vite dev server against the scratch engine, headless Chromium)

Cards render from `search_settings_get` (SearXNG, OpenSERP registered; Firecrawl `Key "firecrawl"
not set`; Brave disabled). Save disabled until an edit; toggling Brave → Save → `config.yaml` gained
the four blocks with explicit `enabled/secret/timeout_ms/priority` and `web` gained the five
default keys, the toast said "saved and applied". SearXNG Test: `ok · 3012 ms · 5 rows · The Async
Ecosystem …`. "Set key…" on Brave stored `brave` through `set_secret` (value absent from the DOM),
`SecretsChanged` registered `brave_api` within ~2 s, Test → `failed · 526 ms · HTTP 422` (the real
API refusing the fake token; not a proxy page); `delete_secret` unregistered it. Status listed
google `ok · 0 rows`, brave `rate_limited`, qwant `captcha`, searxng `external ok · 5 rows`, render-only
`scrape`/`browser` hidden. "Run a test search" printed the tool text. At 390 × 844 the tab list is
the mobile sheet, the pane has `scrollWidth == innerWidth` (no horizontal overflow), cards wrap.

## Runbook for Sky (keys)

Brave Search API and Firecrawl were not exercised live: the scratch vault had no key and none was
requested. To turn either on: Settings › Search → the card → "Set key…" (or `secret_request` in chat,
or `t --secret-set brave` / `firecrawl`), leave the card enabled, Save; the provider registers on the
`SecretsChanged` event and Test shows ok/latency. Hosted OpenSERP: set the URL to
`https://api.openserp.org/v1` and its key under `openserp`.

## Tests and gates

Rust: web 85 unit (date, rank, health, tools incl. `ten_second_render_is_cut_at_the_cap`, render) +
17 aggregate integration (degraded rule, keyless fallback, published normalisation, site on rows) +
engines/url/ssrf; search-providers 4 unit (config round trip through YAML) + 4 wiremock contracts
(OpenSERP now also covers `date=` and the all-engines-failed envelope; fixture meta re-checked against
a live 2.1 server). `cargo check --workspace --tests`, `cargo test -p web -p search-providers -p rpc`
green. UI: `tsc` 0; vitest 99 files / 581 tests green incl. no-emoji, `rpc/api.test.ts` method parity
(`just ts-desktop` regenerated `bindings/`), and `settings/Search.test.ts` (reorder/validate/dirty,
`providerState`, pane render against mocked RPCs with no password field in the DOM).

## Not done / notes

- `published` for "5 days ago"-style ages that SearXNG/OpenSERP put in the snippet, not in a date
  field, stays absent ("dates unavailable for N rows") — parsing snippets is what the brief forbade.
- "rate-limited until …" is not shown: engines send no reset time; Status shows the kind and the
  time of the last failure.
- `bing` is opt-in, not fixed: the empty page is indistinguishable from a real empty SERP except by
  knowing the query has results.
- ponytail: host lists in `rank.rs` (`MIRROR_HOSTS`, `LOW_QUALITY_HOSTS`) are static; a config knob
  if they need per-install tuning.
