# 122 as-built — W3 (SCRAPE)

Branch `feat/122-w3`, worktree `~/Develop/theuth-122-w3`, base `529eec18` (wave C contract + R1 addendum).
Scope: 122 §7 row W3 — engine ladder (D1 scrape side, D8 render tier, D9), D5 scrape side, D6 scrape
`offset`/markers + crawl per-page cap, D10, `ScrapeResponse` filling, `compact`/`sources` wiring against
the stubs, §4.3–§4.5 descriptions, R1-D3's W3 row (`client` fields, robots matched for `*` and `theuth`).
Only `crates/scrape/**` changed.

## What was built

| file | lines | what |
|---|---|---|
| `crates/scrape/src/engine.rs` | 1–31 | module doc, imports, `ISLAND_MIN_PROSE = 800`, `Detector`/`Islands` seam types |
| | 103–160 | `Scrape.detect`/`islands` (default `web::fetch::issue::detect` / `firecrawl_native::extract_islands`), `with_detector`, `with_islands` (`#[doc(hidden)]` test seams) |
| | 164–230 | `fetch_source` (D11 switch: `auto`/`page`/`api`; `rendered: true` skips the API; API failure under `auto` → page + note, under `api` → error), `fetch_api` (`tier: "api"`, `source_kind`, `via`) |
| | 232–308 | `fetch_page` (fetch error without a browser keeps the HTTP error; with one, render and chain both errors) and `ladder`: no issue + not thin → done; no issue/`EmptyShell`/`ConsentWall` → island rung (≥ 800 prose chars → `tier: "island"`, `EmptyShell` cleared), else render; any other issue → render only when `next` has `Rendered`; render only when a browser actor is registered, else the page with its issue is surfaced/kept |
| | 310–366 | `fetch_http`: `Fetched` filled from `FetchResponse` (`issue`, `meta`, `charset`, `client`), D5 `page_meta`, detector run on 2xx body (`status 200`, empty headers, final URL, html, `prose_chars(markdown)`) when the fetcher set no issue |
| | 368–437 | `render(url, only_main_content)`, `render_with`: `trim_link_blocks` when `only_main_content`, issue recomputed from the rendered markdown, `meta.title` only, `client: ClientKind::default()` |
| | 463–490 | `Fetched` gains `issue`, `meta`, `charset`, `client`, `source_kind`, `via`, `notes` |
| | 582–688 | `page_meta` (firecrawl `ogTitle→title`, `description→ogDescription`, `author→article:author`, `publishedTime→date` through `web::date::normalize`, `language` lowercased, `ogSiteName`; `meta::extract` wins per field, then the fetcher's `meta`), `fill_meta`, `first_heading`, `trim_link_blocks`/`is_link_block`/`link_density`/`has_sentence_end` (excerpt's link-noise rule re-implemented; see Hand-offs) |
| | 690–716 | `to_markdown` calls `select_main` after `transform_html` (identity stub); the old empty-main fallback stays at 712–714 |
| | 942–995 | unit tests (3 new) |
| `crates/scrape/src/crawl.rs` | 1–46 | imports, `SKIPPED_ROWS = 200`, `PAGE_CAP = 20_000`, `COMPACT = { fold_links: false, keep_images: true }` |
| | 48–108 | `Page` gains `client`, `source_kind`, `via`, `notes`; `Crawled { pages, incomplete, reason, skipped, invalid_patterns }`; private `Walk { pages, frontier, skipped, reason }` |
| | 135–192 | `CrawlOpts.max_chars_per_page` (default `PAGE_CAP`) |
| | 198–260 | `scope_links` → `filter` twice when `robots.txt` names `theuth` (`names_theuth`): `theuth` group, then `*`; a disallow from either applies (R1-D1) |
| | 264–291 | `scrape_url(url, rendered, only_main_content, source, cx)` → `fetch_source` |
| | 293–331 | `crawl` → `Crawled`: `invalid_patterns = compile_path_patterns(include ++ exclude).1`, `skipped` capped at 200 + `… and N more` row |
| | 336–507 | `walk` → `Walk`: sitemap cut flag, batches bounded by the room left, every in-scope link queued (not just `room`), `skipped` rows (`error` = fetch error text or `timed out after N.Ns`), `stopped` when the caller ends the walk, final frontier = queued ∧ unvisited ∧ depth ok, `reason` = `deadline` (sitemap cut or deadline passed) \| `limit` (frontier left or stopped) \| `None` |
| | 515–605 | `map`: `found` carries `(url, title, source)`; `search`/`sitemap` rows, `page` for a fetched page, `frontier` for links seen; after the walk its `frontier` is appended until `limit` |
| | 651–703 | `sitemap_urls → (Vec<Url>, bool cut)`: each fetch under `tokio::time::timeout(remaining)`; timeout or exhausted deadline/URL cap → `cut = true`, what parsed is kept |
| | 757–820 | `push_hit(.., source)` (a fetched page upgrades `frontier` → `page`; `search`/`sitemap` keep provenance and gain the title), `page_from` (all `Fetched` fields), `cap_page` (`compact` then `cut(md, 0, max_chars)`; `truncated`, `next_offset`) |
| | 1177–1518 | tests (9 new) |
| `crates/scrape/src/tools.rs` | 18–26 | `PROMPT` updated (`result["pages"]`, ISSUE/next_offset/incomplete hints) |
| | 91 | `max_chars` description per §4.3 |
| | 233–264 | `scrape_url` description verbatim (§4.3); `call` passes `source`, builds `ScrapeResponse` + text |
| | 266–392 | `scrape_response` (compact → cut on markdown with `offset`; cut on html from the same offset; links verbatim), `scrape_text` (ISSUE line, `# title`, `url · via …`, meta line, blank, markdown), `issue_line` (D1 rungs, `retry in Ns`, `· client: wreq_chrome`), `meta_line` |
| | 453–576 | `crawl`: `RETURNS = "dict"`, description verbatim (§4.4), `max_chars_per_page` → `opts`, `crawl_result` (`CrawlPage` rows with `issue`, `meta`, `truncated`, `next_offset`, `html` for `format: html`, `client`), `crawl_text` (rows as before, ISSUE line under a walled row, final `incomplete: <reason> · skipped N · invalid patterns N` when any is non-zero) |
| | 609–643 | `map_site` description verbatim (§4.5), rows `{url, title, source}`, text `url  title  [source]` |
| | 653–843 | tests (5 new); `clip` removed |
| `crates/scrape/src/lib.rs` | 19 | `pub use crawl::Crawled` |
| `crates/scrape/Cargo.toml` | 27 | `chrono = { workspace = true }` (for `web::date::normalize(raw, Utc::now())`; already in the graph) |
| `crates/scrape/tests/engine.rs` | new | 7 wiremock ladder tests through the public API and the two seams |

## Decisions taken beyond the design

1. **Two test seams on `Scrape`** (`with_detector`, `with_islands`, `#[doc(hidden)] pub`): the brief asked for one on
   the detector; the island rung depends on `extract_islands`, also a `None` stub, so the same pattern covers it.
   Defaults are the real functions; nothing in production code reads the seams otherwise.
2. **Render goes by `next`, not by kind.** Rule: no issue + thin, `EmptyShell`, `ConsentWall` → island first, then
   render; any other issue → render iff `issue.next` contains `Rendered`. That gives the brief's table
   (`AccessChallenge`/`ConsentWall` → render when a browser is registered, `LoginRequired` surfaced with its `next`,
   `RateLimited`/`ErrorPage` surfaced) from the D1 `next` table instead of a second kind table.
3. **Render-tier `only_main_content` = link-block trim only; compaction happens once, in the tool layer**
   (`scrape_url` honours `compact`, `crawl` always compacts in `cap_page`). Compacting inside `render_with` as well
   would double-run W4's passes and make `compacted_chars` undercount; the `search_web` provider path
   (`provider.rs` → `fetch_page`) therefore gets the trim but no compaction, which is W1's `read_page`'s business.
4. **`excerpt::is_noise`/`link_density` are private in `web`**; the link-only-block rule is re-implemented in
   `engine.rs:646–688` (≈ 30 lines) rather than editing W1's crate. Hand-off below.
5. **The render tier reports `client: reqwest`** (`ClientKind::default()`): the browser fetched the page, not an HTTP
   client, and the frozen enum has no third value. Documented in the field comment; the closer may add a variant.
6. **`rendered: true` skips the API source** under `auto` (an explicit browser request wins); `source: api` with
   `rendered: true` still tries the API.
7. **Walk honesty required two behaviour changes:** (a) every in-scope link is queued (`scope_links(.., usize::MAX)`),
   not `limit − pages`, because the queue is what distinguishes a finished crawl from one cut at `limit`
   (`limit: 2` on a 5-page site used to end with an empty frontier); (b) batches are bounded by the room left so no
   page is fetched past `limit`. Memory is bounded by `SITEMAP_URL_CAP` and the links of ≤ `limit` pages.
8. **`map_site` sources:** `search`/`sitemap` rows keep their provenance when the walk later fetches them (they gain
   the title); a link first seen on a page is `frontier` and becomes `page` once fetched; the walk's leftover queue is
   appended as `frontier` until `limit`. A link that was fetched and failed stays `frontier` (seen, never read).
9. **Sitemap cut counts as `reason: "deadline"` and `incomplete: true`** even when the page loop then drains —
   sitemap URLs unread are pages unread. Hitting `SITEMAP_URL_CAP` sets the same flag.
10. **`Page` gains `client`, `source_kind`, `via`, `notes`** beyond the frozen list (`issue, meta, charset, truncated,
    next_offset`): `CrawlPage.client` (R1) and `ScrapeResponse.source_kind/notes` + the `· via` line need them.
11. **`skipped` overflow row** is `Skipped { url: "…", error: "… and N more" }`.
12. **`scrape_url` text now has a header** (ISSUE line, `# title`, URL, meta line) in the §4.1 shape; before it was the
    bare markdown. The structured half is what the Python runtime consumes; the text is the model's transcript.
13. **`crawl` `format: links` rows** are full `CrawlPage`s with an empty `markdown` (one row type), not `{url, title}`.
14. **`fetch_page` on a fetch error without a browser returns that error** (was: "browser not available for rendered
    fetch"), so `skipped[].error` reads `HTTP 500`; with a browser the render error is chained (`HTTP 500; render: …`).
15. **`chrono` dev→normal dependency of `scrape`** for `web::date::normalize`'s `now` argument.

## Tests

`crates/scrape/src/engine.rs`: `render_trim_drops_leading_and_trailing_link_blocks`, `page_meta_maps_firecrawl_fields`,
`first_heading_reads_atx_only` (+ 7 pre-existing).
`crates/scrape/src/crawl.rs`: `crawl_cut_at_limit_says_incomplete_limit`, `crawl_deadline_cut_says_incomplete_deadline`,
`crawl_lists_skipped_pages_with_the_error` (HTTP 500 row), `crawl_reports_invalid_patterns_and_still_runs`,
`crawl_pages_are_compacted_and_cut_per_page`, `robots_disallow_applies_for_star_and_theuth`,
`map_rows_say_where_each_url_came_from` (sitemap/page/frontier, ≤ limit), `sitemap_walk_keeps_what_parsed_at_the_deadline`
(10 s child, 300 ms deadline → parent URLs kept, `cut`, `reason: deadline`), `scrape_url_source_api_without_a_match_is_an_error`
(+ 8 pre-existing, all green; `crawl_honours_limit_and_depth` etc. now read `.pages`).
`crates/scrape/src/tools.rs`: `scrape_response_cuts_with_offset_and_chains`, `scrape_response_html_and_links_formats`,
`scrape_text_has_issue_header_url_meta_then_body`, `crawl_result_rows_and_tail_line`, `map_line_prints_source`.
`crates/scrape/tests/engine.rs` (wiremock): `access_challenge_is_surfaced_when_no_browser_is_registered` (200 +
`cf-mitigated`, injected `PageIssue`), `login_required_is_surfaced_with_its_next`,
`island_rung_runs_before_render_and_clears_empty_shell` (+ thin island → not used), `thin_page_without_issue_tries_the_island_too`,
`full_page_skips_island_and_render` (island hook panics if called), `fetch_error_without_browser_keeps_the_http_error`,
`crawl_rows_carry_issue_and_client`.

Every assertion holds against both the stubs and the real functions: issues and islands are injected; `cut`
assertions use `truncated`/`next_offset`/window-start, never the stub's exact `…` shape; `compact` is only asserted
to leave `compacted_chars == 0` with `compact: false`.

Not testable here: the render rung itself (no `browser::Browsers` actor can be registered in a test); covered is the
"no browser → surface/keep" branch and the trim/issue logic as pure functions.

### Gate

```
$ cargo fmt -p scrape
(exit 0)
$ build-lock cargo test -p scrape --no-fail-fast
test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.18s   (unittests src/lib.rs)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s    (tests/engine.rs)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc-tests)
EXIT=0
$ build-lock cargo clippy -p scrape --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 17s
EXIT=0
```

## Hand-offs

Call sites against sibling stubs (the closer checks each against the real function):

| sibling | call site | what W3 relies on |
|---|---|---|
| W1 `web::fetch::issue::detect` | `engine.rs:127` (default detector), run at `engine.rs:341–349` (2xx body, `status 200`, empty headers) and `engine.rs:407–413` (rendered markdown) | signature §7.2; W3 reads `PageIssue.kind`, `.next`, `.retry_after`, `.detail` |
| W1 `FetchResponse` | `engine.rs:201–222` (api), `311–363` (`fetch_http`) | fields read: `url`, `title`, `content`, `issue`, `meta`, `charset`, `client`. **Not read:** `attempts`, `ladder`, `notes`, `truncated`, `compacted_chars`, `next_offset`, `omitted_headings`, `tier`, `source_kind` — if W1 sets `notes` (R1 `NotBuilt` note) or `truncated` on `fetch_html`, the closer may want them forwarded into `Fetched.notes` |
| W1 non-2xx-with-issue path | `engine.rs:232–256` | today a 403 is `Err("HTTP 403")` → no browser → `Err` (crawl: `skipped`). Once W1 returns `Ok { issue: Some(..) }` for a diagnosed non-2xx, it flows through `ladder` like the 200 + `cf-mitigated` test; nothing to change in `scrape` |
| W1 `web::fetch::meta::extract` | `engine.rs:603` | wins per field over the fetcher's `meta` and the firecrawl mapping |
| W1 `web::fetch::cut` | `tools.rs:293, 300`; `crawl.rs:813` | `Cut.text/truncated/next_offset/omitted_headings`; W3 appends nothing (the marker is `cut`'s) |
| W4 `web::compact::compact` | `tools.rs:287`; `crawl.rs:812` | `COMPACT = { keep_images: true, fold_links: false }` (`crawl.rs:43`) |
| W4 `web::sources::api_source` / `render` | `engine.rs:174`, `engine.rs:206` | `ApiSource.kind/fetch_url`; `render → None` is "body did not parse" (note under `auto`, error under `api`) |
| W2 `firecrawl_native::select_main` | `engine.rs:700–707` (`SelectOptions { explicit_scope: false, only_main_content }`, `original = html`) | **Closer:** D8 puts the real selection inside `transform_html`; if W2's hook lands there, this call runs the selector a second time on the already-selected region — decide whether to keep it or drop it; either way remove the old empty-main fallback at `engine.rs:712–714` once invariant 2 is real |
| W2 `firecrawl_native::extract_islands` | `engine.rs:128` (default), run at `engine.rs:273` | `Some(text)` with ≥ 800 `prose_chars` → `tier: "island"` |
| W2 `compile_path_patterns` | `crawl.rs:306` | real in C; `.1` is `invalid_patterns` |

Other hand-offs:

- **Closer / W1:** `excerpt::is_noise` and `link_density` are private; `engine.rs:646–688` duplicates the link rule.
  Making them `pub` in `web::excerpt` lets the closer delete the copy.
- **Closer:** the ISSUE line and meta line are rendered in `tools.rs:346–392` with the D1/D5 wording; W1 renders the
  same strings in `render::fetch_text`. If W1 exposes a `pub fn issue_line(&PageIssue) -> String`, replace W3's.
- **Closer:** `ClientKind` has no value for "the browser fetched it"; the render tier says `reqwest`
  (`engine.rs:430–431`). A `Browser` variant would be truthful.
- **Closer:** `CrawlResult.reason = "cancelled"` is declared, never set — no cancellation signal reaches the crawl
  future in this tree (§9 q5).
- **Closer:** `search_web`'s page reads (`provider.rs` → `fetch_page`) now see the ladder (island rung, issue surfaced
  in `Fetched.issue`) but `Rendered` has no `issue` field; W1's `read_page` maps issues from `FetchResponse` only.

## Open items

- The render rung is unexercised by tests (no browser actor in a test); the live smoke (§6.3) covers it.
- `fetch_api` does not run `post_process_markdown` on `sources::render`'s output (it is already compact markdown); if
  W4's renderers emit base64 images or stray whitespace, add the two calls at `engine.rs:206`.
- `prose_chars` is computed on the markdown of the *selected* main content; a page whose chrome carries the prose
  reads as thin and goes to the island/render rungs — same as before this wave.
