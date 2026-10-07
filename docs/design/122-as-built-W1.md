# 122 as-built — W1 (FETCH)

Branch `feat/122-w1`, worktree `~/Develop/theuth-122-w1`, base `529eec18` (C's contract commit, Revision 1
applied). Scope: D1 (+R1-D2), D2, D3, D4 as revised by R1-D1, D5 web side, D6, D12 web rails, the
`read_url` wiring against W4's stubs, `search_web` skip reasons, the §6.1 w2 sniff order. Only
`crates/web` files from the brief's "You own" list were touched.

## What was built

| file | lines | what |
|---|---|---|
| `crates/web/src/fetch/charset.rs` | whole file (109) | D3 `decode`: BOM (`Encoding::for_bom`) → `charset=` of `Content-Type` (`charset_param`, 78–87) → `<meta charset>` / `http-equiv` in the first 1 024 bytes (`meta_charset`, 89–108) → UTF-8 → lossy; unknown or `replacement` labels fall through; `label()` (44–75) lowercases `encoding_rs`' mixed-case names (`Shift_JIS` → `shift_jis`) |
| `crates/web/src/fetch/issue.rs` | 1–13 doc/imports, 54–58 `detail` doc, 83–169 tables + `IssueKind::next` + `retry_after`, 173–295 `detect`, 297–326 helpers | D1 marker table rows 1–16 in order with R1-D2's 7a–7d after Akamai; `CHARS_PER_WORD = 6`; `detail` = `HTTP <n> · <vocab>` on non-2xx, `<vocab>` on 2xx, bare `HTTP 401/403/429`; `retry_after` parsed as seconds or RFC 7231 date on 429/503; `next` table pinned by `IssueKind::next` |
| `crates/web/src/fetch/meta.rs` | whole file (344; tests 253–343) | D5 `extract`: tag scanner over the first 256 KiB stopping at `<body>` (`tags`, 166–191), attribute parser (`attrs`, 194–244), `<title>`, `<link rel=canonical>` joined against `base`, `<html lang>`, one JSON-LD article block ≤ 64 KiB (`json_ld`, 125–154; `ld_author`, 117–122), `published` through `date::normalize` |
| `crates/web/src/fetch/text.rs` | 17/45/134 visibility, 173–254 `MAX_OMITTED_HEADINGS`, `cut`, `char_to_byte`, `heading_text`; tests 278–280 | D6 `cut`: blank-line boundary in the last 15 % (cut lands *after* the `\n\n`), exact marker `[… N chars omitted; continue with offset=M]`, `Omitted sections: a · b · c` (≤ 12), `[offset M is past the end (N chars)]`; `hard_truncate` unchanged (http_request excerpt, raw `fetch_html` bodies) |
| `crates/web/src/fetch/ssrf.rs` | 115–146 hostname denylist, 148–153 doc, 174 `240/4`, 188–194 NAT64 | D12: `*.internal`, `*.lan`, `*.home.arpa`, `*.intranet`, `*.corp` (+ bare labels) via `INTERNAL_SUFFIXES`; `240.0.0.0/4`; `64:ff9b::/96` classified as the embedded v4 |
| `crates/web/src/fetch/mod.rs` | 21–31 imports/re-exports, 70–75 field docs, 183–192 constants (`MAX_ATTEMPTS`, `RETRY_DELAYS`, `RETRY_STATUSES`, `ACCEPT`, `STEALTH_NOT_BUILT`), 194–253 `PageOptions`/`Finished`/`finish`, 255–270 `Hop`/`Attempt`, 285–300 `Fetcher` fields, 312–405 constructors + `with_policy` + `total_budget`, 414–450 `fetch`/`fetch_page`/`fetch_html`, 457–491 `rungs`/`headers`, 493–553 `fetch_kind` (rung loop + escalation), 556–595 `attempts` (D2 loop), 598–678 `attempt` (redirect loop, pre-body binary refusal, capped read), 827–849 `inflate` (D12 flag), 857–956 `read_body` (sniff order, decode, convert, `meta`, `detect`, non-2xx-with-issue), 958–1018 `window`/`primary_type`/`sniffable`/`unsupported` | D2 retries under `2 × timeout`, D4 headers, R1-D1 `Impersonate` dispatch, D1 surfacing, D3/D5/D6 wiring, D12 rails. `request()` (696–778) unchanged: one attempt, plain `reqwest` |
| `crates/web/src/engines/mod.rs` | 63–69 | `USER_AGENT = concat!("theuth/", env!("CARGO_PKG_VERSION"), " (+https://github.com/skymoore/theuth)")` |
| `crates/web/src/lib.rs` | 84–87 `header_ladder` doc (R1-D5 wording), 114 UA default from the constant, 154–161 `Fetcher::new(..).with_policy(impersonate, header_ladder, accept_language)` | config fields read |
| `crates/web/src/render.rs` | 11–15 imports, 177–252 `fetch_text` + `snake`, tests 458–515 | §4.1 text: `ISSUE:` line (rungs `rendered: true` / `browser_import_cookies` / `desktop_handoff` / `retry in <n>s`, ` · client: wreq_chrome` when stealth), `# title`, URL (` · via <fetch_url>` when `tier == "api"`), `published · by · lang` line (+ `client:` on a ladder win), content with `cut`'s marker, `[body cut at max_fetch_bytes]` when only the raw body was cut |
| `crates/web/src/tools.rs` | 24–25 imports, 133 search description sentence, 181–207 `fetch_description` (§4.1 verbatim + provider sentence), 251–340 `read_page` (issue → `skipped`), 547–598 `FetchTool::call` (offset/compact/source, rendered path through `finish`), 601–686 `read_page_or_api` + `read_api` (D11 wiring against the stubs), test 950–1023 | `read_url` wiring and `search_web` skip reasons |
| `crates/web/src/aggregate.rs` | 64–67 | `skipped` doc comment (§4.2) |
| `crates/web/tests/{issue,charset,cut,fetch_retry}.rs` | new | contract tests (below) |
| `crates/web/tests/fetch_ssrf.rs` | 53–94 new denylist test, 199–209 `max_chars_truncation_sets_flag` updated to D6 | — |
| `crates/web/tests/fixtures/shift-jis-page.bytes` (274 B), `pdf-as-octet-stream.bin` (885 B) | new | synthesised: a Shift_JIS-encoded 夏目漱石 opening line page (header-only charset hint); a hand-built 2-page Helvetica PDF |

Untouched: `client.rs`, `compact.rs`, `sources.rs`, `settings.rs`, `http.rs`, everything outside `crates/web`.

## Decisions taken beyond the design

1. **`Fetcher::new`/`new_with_timeouts` signatures kept** (scrape, routes, sensor and their tests call them). Policy is a builder: `with_policy(impersonate, header_ladder, Option<accept_language>)` (`fetch/mod.rs:365`). The constructors apply `WebConfig::default()` (`impersonate: always`, `header_ladder: true`, `accept_language: en-US,en;q=0.9`) so every fetcher is stealth-by-default per Sky's call; `lib.rs:154-161` overrides from config.
2. **`PageOptions { max_chars, offset, compact }` + `fetch_page`** beside the unchanged `fetch(url, format, max_chars)` (compaction on, offset 0) and `fetch_html(url, max_chars)`; `finish(markdown, format, &opts)` is the shared `compact → markdown_to_text → cut` pipeline used by `read_body`, the document path, the rendered path and the API path.
3. **Raw bodies (`fetch_html`, `format: None`) keep `hard_truncate`**, no marker: the content is raw HTML or JSON for scrape/crawl/API rendering; a `[… omitted]` marker inside it would break parsers. `cut` runs on every formatted path (`read_url` markdown/text, PDF→markdown, rendered).
4. **`fetch_html` of `application/xml`/`text/xml` is not converted or diagnosed** (`read_body`, `diagnose` at `:905`): sitemaps up to 8 MiB are not pages. Every other HTML body — including `fetch_html` of `text/html` for scrape — is converted once so `prose_chars` is real and `issue`/`meta` are filled; scrape converts again through its own pipeline (cost accepted, W3/closer may drop it if `fetch_html` callers never read `issue`).
5. **Binary refusals stay `Err(Failed("unsupported content type: …"))`** (D1 row 17 `Unsupported` is declared, never produced): today's shape, pinned by `fetch_ssrf::plain_text_and_binary_bodies`, and what scrape's `fetch_page` expects. The sniff (§6.1 w2) runs only for `application/octet-stream`/`application/zip` after the body is read (`attempt` :647–649, `read_body` :873–899); other binary types are still refused before download.
6. **Escalation trigger** = `AccessChallenge` on status 403 or 503 (R1-D1's "or `RateLimited` on 403/503 with a D1 marker" is unreachable: `RateLimited` is row 9, status 429 only). When the second rung is still a wall, the *first* diagnosis is returned with `attempts` updated; when the second rung errors, the first diagnosis is returned.
7. **Marker numbers are plain digits** (`offset=3986`, not the design's typographic `3 986`): the model copies the offset back.
8. **`cut` lands after the blank line**, so `s[offset..next_offset]` is exactly the kept text (trailing whitespace trimmed in `text`) and windows chain with no gap or overlap; the next window starts at a block.
9. **`consent:<platform>` vocabulary** uses `onetrust`, `cookiebot`, `trustarc`, `didomi`, `quantcast`, `sourcepoint`, `usercentrics`, `evidon` markers, else `consent:url` (row 12) / `consent:title` (row 13). Row 8 yields `cloudflare:1020` when `cf-ray`/`cloudflare` markup with "you have been blocked"/"error 1020" is present, `cloudflare` for other cf markup, `antibot-title` otherwise.
10. **`via <fetch_url>`** is computed at render time from `sources::api_source(url)` when `tier == "api"` (`render.rs:215-221`) instead of a new `FetchResponse` field — pure function, no schema change.
11. **`search_web` skip reasons apply to the raw-fetch fallback** (`read_page`, `tools.rs:285`): the provider path returns `Rendered { title, markdown, tier }` which has no `issue` (see Hand-offs).
12. **Default headers via `HttpClient::get`'s `headers` argument** (`Fetcher::headers`, `:477`) rather than client defaults — `client.rs` is W5's. Sent only on `ClientKind::Reqwest`; `WreqChrome` gets `&[]` (the profile's headers + `ClientConfig.accept_language`, W5).
13. **Pre-existing tests changed by the design**: `fetch_ssrf::max_chars_truncation_sets_flag` (D6: the window is ≤ `max_chars`, marker after it), `render::tests::fetch_text_marks_truncation` → `fetch_text_issue_meta_and_cut_shapes` (`[truncated at N chars]` replaced), `text::tests::title_scan_and_truncate` (stub `cut` assertions moved to `tests/cut.rs`).

## Tests

- `tests/issue.rs` (14): `chars_per_word_is_pinned`, `row_1_cf_mitigated_header_is_conclusive`, `row_2_orchestrate_paths_not_the_loose_substring` (github beacon benign), `row_3_turnstile_is_size_gated` (120 KB article benign), `rows_4_to_7_vendor_markers`, `rows_7a_to_7d_revision_1_vendors` (reCAPTCHA form benign), `row_8_antibot_titles_gated_on_prose`, `rows_9_to_11_status_rows` (403 JSON → `None`), `rows_12_to_13_consent`, `row_14_login_gated_on_prose` (sign-in title + 400 words → `None`), `rows_15_to_16_empty_shell`, `next_table_is_fixed`, `retry_after_seconds_date_and_junk`, `forbidden_with_cf_mitigated_through_the_fetcher` (wiremock; `ISSUE:` line).
- `tests/charset.rs` (5): `charset_parameter_decodes_shift_jis`, `meta_charset_without_header_parameter`, `bom_wins_over_a_wrong_charset_parameter`, `bogus_label_and_invalid_utf8_fall_through`, `shift_jis_fixture_through_the_fetcher`.
- `tests/cut.rs` (6): `fits_without_a_cut`, `cuts_at_the_last_blank_line_in_the_final_15_percent`, `hard_cut_when_no_blank_line_in_the_window_tail`, `omitted_headings_in_order_capped_at_twelve`, `offset_past_the_end`, `chaining_covers_the_docs_page_without_gaps_or_overlaps` (over C's `docs-page.expected.md`).
- `tests/fetch_retry.rs` (16): `transient_503_then_200_is_two_attempts`, `non_retryable_500_is_one_attempt`, `connection_reset_then_200_is_retried`, `retry_after_one_second_is_honoured`, `retry_after_beyond_the_budget_is_surfaced_not_waited`, `second_attempt_not_started_when_the_budget_cannot_fit_it`, `budget_is_twice_the_timeout_and_a_sleeping_server_times_out_inside_it`, `default_headers_on_the_reqwest_path`, `impersonate_always_falls_back_to_reqwest_with_a_note_until_w5_lands`, `on_challenge_escalates_once_on_a_diagnosed_403`, `header_ladder_off_and_never_do_not_escalate`, `request_keeps_one_attempt`, `content_length_precheck_refuses_before_reading`, `post_inflate_cap_sets_truncated`, `pdf_served_as_octet_stream_is_converted`, `offset_and_compact_window_through_fetch_page`.
- `tests/fetch_ssrf.rs` (+1): `internal_suffixes_nat64_and_reserved_v4_are_refused`.
- unit: `fetch::meta::tests::{each_source_row, fallbacks_and_json_ld, unparsable_date_and_empty_page, attr_parser_handles_quote_styles}`, `render::tests::fetch_text_issue_meta_and_cut_shapes`, `tools::tests::read_url_pages_with_offset_and_reports_source_api_without_a_match` (FetchTool end to end: title, `by Jane` meta line, marker, `next_offset` chaining, `source: api` error, `source: page`).

### Gate

```
$ cargo fmt -p web
(exit 0)
$ build-lock cargo test -p web --no-fail-fast > /tmp/122-W1-test.log 2>&1; echo EXIT=$?
EXIT=0
test result: ok. 92 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s   (web lib)
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s   (tests/aggregate.rs)
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (tests/charset.rs)
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (tests/cut.rs)
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   (tests/engines.rs)
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.01s   (tests/fetch_retry.rs)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    (tests/fetch_ssrf.rs)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s    (tests/http_request.rs)
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   (tests/issue.rs)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   (tests/request.rs)
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (tests/url.rs)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc-tests)
$ build-lock cargo clippy -p web --tests -- -D warnings
    Checking web v2.9.2 (/Users/Sky.Moore/Develop/theuth-122-w1/crates/web)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.12s   (exit 0)
```

Smoke (observed through `FetchTool::call` against wiremock, `max_chars: 600`):

```
# Long
http://127.0.0.1:59580/long
by Jane

## Part 1
…
## Part 3

[… 2811 chars omitted; continue with offset=575]
Omitted sections: Part 4 · Part 5 · Part 6 · Part 7 · Part 8 · Part 9 · Part 10 · Part 11 · Part 12
```

## Hand-offs

Call sites against sibling stubs (the closer checks each against the real function):

- **W4 `compact::compact`** — `crates/web/src/fetch/mod.rs:232-239` in `finish` (`CompactOptions { fold_links: format == Text, keep_images: false }`); reached from `read_body` (HTML and document paths), `FetchTool::call` rendered path (`tools.rs:574`) and `read_api` (`tools.rs:674`). `compacted_chars` is `removed_chars`. With the stub it is `0` everywhere.
- **W4 `sources::api_source`** — `crates/web/src/tools.rs:617` (dispatch; `source: page` skips it; `reddit-old` → URL rewrite, `source_kind` set, `tier` stays `fetch`) and `crates/web/src/render.rs:218` (the ` · via <fetch_url>` suffix when `tier == "api"`). **`sources::render`** — `tools.rs:672`; `None` → under `auto` the page pipeline runs with `notes: ["api source <kind> failed: body did not parse; read the page"]`, under `api` the tool errors `api source <kind>: …`. The API body is fetched with `fetch_html(fetch_url, usize::MAX)` (byte cap applies). Against the stubs the `source: api` branch always errors `no API source matches <url>` (test pins it). The `wiremock read_url of an HN item → tier == "api"` test of D11 belongs with W4's real table (`tests/sources.rs`).
- **W5 `client::build(ClientKind::WreqChrome, …)`** — `crates/web/src/fetch/mod.rs:380` in `with_policy`; `Err(_)` → `stealth: None` → every fetch that wanted stealth carries `notes: ["stealth client not built; fetched with reqwest"]` (`STEALTH_NOT_BUILT`, `:192`, `rungs` `:457-473`). Tests pinning today's fallback that the closer must flip when W5 lands: `fetch_retry::impersonate_always_falls_back_to_reqwest_with_a_note_until_w5_lands` (expects `client == Reqwest` + the note → becomes `WreqChrome`, no note), `on_challenge_escalates_once_on_a_diagnosed_403` (asserts the note is present; the wiremock loopback allowlist must also be honoured by the wreq client's resolver for that test's second rung to connect), `default_headers_on_the_reqwest_path` / `header_ladder_off_and_never_do_not_escalate` (assert `notes.is_empty()` under `never` — unaffected). `Fetcher::headers` (`:477`) sends `&[]` to a `WreqChrome` client: W5 adds `Accept-Language` from `ClientConfig` itself (R1-D1).
- **W3 / closer — scrape's fetcher**: `crates/scrape/src/lib.rs:86` builds with `new_with_timeouts` and therefore gets `Impersonate::Always` + default `Accept-Language`; to follow `web.impersonate`/`header_ladder`/`accept_language` it should chain `.with_policy(cfg.impersonate, cfg.header_ladder, Some(&cfg.accept_language))` from the `web` section. `FetchResponse` fields W3 reads are unchanged in name/type; `fetch_html` now fills `issue`, `meta`, `charset`, `attempts`, `client`, `notes` and leaves `next_offset: None`, `omitted_headings: []`, `compacted_chars: 0` (raw bodies are hard-cut, Decision 3).
- **Closer — `search_web` provider path**: `provider::Rendered` (`crates/web/src/provider.rs`, not W1's) carries no `issue`, so pages read through the `scrape` provider cannot yield `skipped: blocked|login|consent`; only the raw-fetch fallback does (`tools.rs:285`, `:295-308`). Adding `issue: Option<PageIssue>` to `Rendered` and filling it in W3's engine closes this.
- **Closer — docs**: `FetchResponse` doc comments updated here (`attempts` "every request of every rung", `ladder` R1 wording); §7.2 should note `cut` lands after the blank line and `Fetcher::with_policy`.

## Open items

- `fetch_html` of `text/html` converts to markdown once for diagnosis (Decision 4); if profiling shows it matters on crawl, pass a `diagnose: bool` through `PageOptions` and let scrape opt out.
- `Unsupported` issue kind is never produced (Decision 5); either drop the variant or switch binary refusals to `Ok { issue: Unsupported }` in a later design.
- The `connection_reset_then_200_is_retried` test uses a raw `TcpListener` thread (wiremock cannot drop a connection); it is loopback-only and deterministic but worth a look if it ever flakes.
