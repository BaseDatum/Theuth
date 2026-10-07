# 122 I — as built (closer: integration of W2, W1, W5, W4, W3)

Integration tree `~/Develop/theuth-122-web`, branch `feat/122-web-reading`, base `529eec18` (design + Revision 1 +
contract C). Toolchain `cargo 1.98.1`, target dir `~/Develop/theuth-122-target/C`, every compile through `build-lock`.

## Merge log

Merged by the orchestrator before this wave started, `git merge --no-ff`, in R1-D3's order; **no conflicts** in any
of the five merges (the two hand-edits §7.3 predicted — `fetch/mod.rs` doc comments and the `engine.rs` fallback —
were not textual conflicts; the second is closed below as a hand-off).

| order | branch | branch head | merge commit |
|---|---|---|---|
| 1 | `feat/122-w2` | `1c5c25e6` | `7d275081` |
| 2 | `feat/122-w1` | `1f11eedf` | `7549f0b5` |
| 3 | `feat/122-w5` | `45fc1f9f` | `2b38ff47` |
| 4 | `feat/122-w4` | `1ff6bb13` | `e6e69996` |
| 5 | `feat/122-w3` | `0713e7c7` | `34688ca5` |

Post-merge check at `34688ca5`: `build-lock cargo check -p web -p scrape -p firecrawl-native --tests` →
`Finished dev profile … in 40.25s`, no warnings.

Closer commits on top (`git log --oneline 34688ca5..HEAD`):

```
351afd78 122 I: live-smoke fixes — omitted headings unwrap links/anchor glyphs, API title not duplicated (take_title), reachable Shift_JIS hosts, notes/dump in live rows
6db05a54 122 I: live smoke tests (ignored; THEUTH_LIVE=1, THEUTH_LIVE_IMPERSONATE switch)
1475e2d0 122 I: tests meet the real waves — unique paragraphs for cut tests, fetcher-diagnosed cf-mitigated, WreqChrome default, meta precedence; compact keeps a headings-only page
eee280a4 122 I: harness.md §4.6 prose, 16 §6 config rows, design integration notes
65d6771e 122 I: close hand-offs — NotBuilt tests flipped, engine select_main/fallback removed, scrape fetcher follows web policy, SO follow_up, Rendered.issue, shared issue_line/is_noise
```

## Hand-offs closed (file:line at HEAD)

| from | hand-off | closed how |
|---|---|---|
| W1, W5, R1-D3 | `NotBuilt` fallback note must not appear once W5 is real | `crates/web/tests/fetch_retry.rs:316` `impersonate_always_uses_the_wreq_chrome_client_without_a_note` (was `…_until_w5_lands`: now asserts `client == WreqChrome`, `notes` empty), `:348` `on_challenge_escalates_once_on_a_diagnosed_403` asserts `WreqChrome` + no note; `STEALTH_NOT_BUILT` import dropped from the test. The fallback stays as code (`crates/web/src/fetch/mod.rs:191-193`, `:289-291`, `:361-364` doc comments now say "build failed", not "W5 not landed"). `grep -c "stealth client not built" /tmp/122-I-test.log` → `0` on the full crate run; no live row carried it |
| W2, W3, design §7.3 | remove the old empty-main fallback (`engine.rs:383-385` in the design; `712-714` in W3's tree) and decide the second `select_main` | `crates/scrape/src/engine.rs:652` `to_markdown` is now `transform_html` → h2m → `post_process_markdown`; W3's second `select_main` call on the already-selected region is gone (W2's hook inside `transform_html`, `firecrawl-native/src/html.rs:626-636`, is the one selection) and so is the "empty main → whole page" recursion. `engine::tests::all_chrome_page_is_not_widened_under_only_main_content` replaces `empty_main_content_falls_back_to_whole_page` and pins D8 invariant 1 |
| W3 | call sites against W1's final `FetchResponse`/`PageIssue` | compile-clean at the merge; `engine.rs:235`, `:346`, `:373` now forward `resp.notes` into `Fetched.notes` (W3's "closer may want them forwarded") |
| W3, R1-D3 | `ScrapeResponse.client`/`CrawlPage.client` from `FetchResponse.client` | verified: `engine.rs:231/344/371` (`client: resp.client`) → `crawl.rs:802` → `tools.rs:282` (`ScrapeResponse`) and `:522` (`CrawlPage`); `tests/engine.rs:271` `crawl_rows_carry_issue_and_client` now expects `WreqChrome` under the default policy |
| W1, W5 | scrape's fetcher ignored `web.impersonate`/`header_ladder`/`accept_language` | `crates/scrape/src/lib.rs:85-99`: reads the `web` section (`rt.section("web").unwrap_or_default()`) and chains `.with_policy(web_cfg.impersonate, web_cfg.header_ladder, Some(&web_cfg.accept_language))`; also uses `web_cfg.user_agent` instead of the `USER_AGENT` constant |
| W4 | `reddit-old` must run the page pipeline, not count as "api source failed" | already so in W1's wiring (`crates/web/src/tools.rs:622-624`) and W3's (`engine.rs:171-180`); verified live: `source_kind=reddit-old`, `tier: fetch` |
| W4 | `stackoverflow` needs `follow_up` (answers endpoint) appended before `render` | `crates/web/src/tools.rs:674-682` and `crates/scrape/src/engine.rs:206-214`: fetch `follow_up(src)`, append newline-separated; a failed second fetch leaves the question and pushes a note. Live: 3 421 chars (`read_url`) / 3 365 (`scrape_url`) with answers |
| W1 | `search_web` pages read through the `scrape` provider could not yield `skipped: blocked|login|consent` | `crates/web/src/provider.rs:216-219` `Rendered.issue: Option<PageIssue>` (serde default, skipped when `None`); filled in `crates/scrape/src/provider.rs:74`; consumed at `crates/web/src/tools.rs:273`. `issue: None` added at the three other constructors (`browser/src/render.rs:127`, `search-providers/src/firecrawl.rs:276`, `search-providers/src/openserp.rs:320`) |
| W3 | `excerpt::is_noise`/`link_density` private; link rule duplicated in `engine.rs:646-688` | `crates/web/src/excerpt.rs:170` `is_noise` is `pub`; `engine.rs:634` `trim_link_blocks` uses it; the 40-line copy (`is_link_block`, `link_density`, `has_sentence_end`) deleted |
| W3 | ISSUE line rendered twice (W1 `render::fetch_text`, W3 `tools.rs:346`) | `crates/web/src/render.rs:229` `pub fn issue_line(&PageIssue, ClientKind) -> String`, used by `fetch_text` and by `crates/scrape/src/tools.rs:318`, `:515`; scrape's copy and its duplicate pin deleted. One behavioural unification: `— next:` is omitted when `next` is empty (W3's shape; W1 printed a dangling `— next: `) |
| W3 | meta precedence test written against the `meta::extract` stub | `engine::tests::page_meta_maps_firecrawl_fields` (`engine.rs:914`) pins the documented order: page scanner wins per field, the fetcher's meta fills the rest (`canonical`) |
| W3 | `cf-mitigated` test expected the seam's detail | the real fetcher diagnoses the 200 + `cf-mitigated` page itself (`detail: cloudflare`, no HTTP prefix on a 2xx) and the engine keeps the fetcher's issue; `tests/engine.rs:84` updated |
| W1, W3, W4 | cut/offset tests used identical repeated paragraphs | the real `compact` (pass 5) deduplicated them under the cap, so nothing was ever cut; `scrape/src/tools.rs:641-650`, `scrape/src/crawl.rs:1307-1313`, `web/tests/fetch_retry.rs:533-542`, `web/src/tools.rs:976-984` now generate per-section-unique prose. W1's `pdf_served_as_octet_stream_is_converted` exposed a real gap, fixed below (compact guard) |
| W4 | `removed_chars` and the headings-only case | `crates/web/src/compact.rs:136-143`: when every pass together empties a non-empty page (a two-line PDF rendered by anydoc is two `##` headings; pass 8 drops both) `compact` returns the input unchanged with `removed_chars: 0`. Pinned by `tests/compact.rs::headings_only_document_is_kept_rather_than_emptied` |
| W2 | h2m `max_depth` (64-level default) | not changed: W2's `collapse_wrapper_chains` covers div-chain soup and the design's depth guard is the scorer's; raising `max_depth` would need the same edit in `corpus.rs` and a goldens pass — Open items |
| W4 | arXiv `id_list` `%2F` for old-style ids | tested live, see Open items (the API hangs on old-style ids whichever way the slash is encoded; the `auto` fallback reads the page with the note) |
| C, W1, W2, W4 | design text for `charset::decode`'s lifetime, `with_policy`, `cut` landing after the blank line, `transform_html_with_method`, "candidate scores > 0", h2m 64, `follow_up`, `Rendered.issue`, `issue_line`/`is_noise`, `NotBuilt`, render-tier `client` | `docs/design/122-web-reading.md` "Integration notes (closer, 2026-10-07)" table at the end |

Live-smoke-driven fixes (Phase II, `351afd78`):

- `crates/web/src/fetch/text.rs:262` `heading_text`: omitted headings unwrap `[label](url)` and drop `§`/`#`/`¶`
  anchor glyphs — docs.rs produced `"[§](#tls)TLS"`; now `"TLS"`.
- `crates/web/src/fetch/text.rs:145` `pub fn take_title(md) -> (Option<String>, &str)` (re-exported from
  `web::fetch`): API renderers start their markdown with `# title`; `read_api` (`web/src/tools.rs:685`) and
  `fetch_api` (`scrape/src/engine.rs:217`) take it as the title and drop the line, so the `# title` header is no
  longer followed by the same heading as the first content line (HN row showed `# Y Combinator` twice). Scrape's
  private `first_heading` deleted.

## `crates/theuth/harness.md` (§4.6)

- `harness.md:65` config example: `…, fetch_max_chars: 4000, read_top: 5, accept_language: en-US,en;q=0.9, header_ladder: true, impersonate: always}` — §4.6 verbatim plus `impersonate: always` (R1-D5; `header_ladder` alone is misleading since it only applies under `on_challenge`).
- `harness.md:230-238`: the §4.6 paragraph, verbatim, as a new bullet after the `browser_*` bullet and before the cookie-import bullet.
- The "orchestration-lessons note `read_url(max_chars) 200..=16000`" is not in this tree (nothing under `crates/theuth`, `docs/` or the repo matches `200..=16000` except `docs/design/16-web.md:155`'s struct comment); it lives in the knowledge store under `~/.theuth`, which this wave must not touch — Open items.
- `docs/design/16-web.md:541-543`: `accept_language`, `impersonate`, `header_ladder` rows in the §6 config block (the design's "16 §6 config table rows").

Tool descriptions checked word for word against §4.1–§4.5: `fetch_description` (`web/src/tools.rs:186-192`),
`FetchArgs.offset/compact/source` docs (`:98-106`), every `FetchResponse` field doc (`web/src/fetch/mod.rs:64-99`,
`ladder` in its R1 wording), `search_web`'s "(not-relevant, budget, blocked, login, consent)" (`:135`),
`scrape_url` (`scrape/src/tools.rs:233`, `max_chars` `:90`), `crawl` (`:423`, `max_chars_per_page` `:406`),
`map_site` (`:573`). All match.

## §5 grep

```
$ grep -rn -E 'sh -c|bash -c|Command::new|std::process' crates/web crates/scrape crates/firecrawl-native
grep exit=1 (1 = no matches)
```

## Gates (final runs, `build-lock`, `CARGO_BUILD_JOBS=4`)

```
build-lock cargo check -p web -p scrape -p firecrawl-native --tests            (34688ca5, post-merge)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 40.25s
cargo fmt -p web -p scrape -p firecrawl-native                                 (clean)
build-lock cargo test -p web -p scrape -p firecrawl-native --no-fail-fast      (351afd78) EXIT=0
    web lib:            test result: ok. 101 passed; 0 failed; 0 ignored
    web fetch_retry:    test result: ok. 16 passed; 0 failed; 0 ignored
    web compact:        test result: ok. 19 passed; 0 failed; 0 ignored
    web live:           test result: ok. 0 passed; 0 failed; 2 ignored
    scrape lib:         test result: ok. 33 passed; 0 failed; 0 ignored
    scrape engine:      test result: ok. 7 passed; 0 failed; 0 ignored
    scrape live:        test result: ok. 0 passed; 0 failed; 2 ignored
    firecrawl-native:   test result: ok. 53 passed; 0 failed; 0 ignored   (corpus: 1 passed)
    (every other test binary: ok, 0 failed — 21 result lines, all `0 failed`)
build-lock cargo clippy -p web -p scrape -p firecrawl-native --tests -- -D warnings   (351afd78) EXIT=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 40.48s   (no warnings)
build-lock cargo test -p tools -p theuth --no-fail-fast                        (351afd78) EXIT=0
    test result: ok. 3 passed; 0 failed      (tools)
    test result: ok. 165 passed; 0 failed    (theuth)
    test result: ok. 0 passed; 0 failed      (theuth doc)
mkdir /tmp/theuth-build.lock && … BUILD_SLOTS=1 BUILD_LABEL=122-I-workspace build-lock cargo check --workspace --tests   (6db05a54) EXIT=101
    error: couldn't read `crates/nodes/src/../../../bindings/methods.json`: No such file or directory (os error 2)
    error: could not compile `nodes` (lib test) due to 1 previous error
```

The only red is the known pre-existing `nodes` one (`bindings/methods.json` absent). Because cargo stops scheduling
units after an error, the log was checked for coverage: the 25 crates cargo re-checked (`web`, `scrape`,
`firecrawl-native`, `browser`, `search-providers`, `agent`, `routes`, `rpc`, `tui`, `theuth`, `app`, `a2a`,
`artifact`, `meeting`, `roster`, `sandbox`, `schedule`, `sessionsync`, `signal`, `slack`, `talk`, `tasks`,
`telegram`, `update`, `nodes`) are exactly the reverse-dependency closure of the five changed crates
(`cargo tree -i <crate> --workspace`); the 22 others were fresh from C's run of the same target dir. The `nodes`
failure is its lib **test** target, which nothing depends on. `cargo test -p theuth` (above, after the last code
change) builds the whole binary's dependency closure. No `ts-rs` type changed
(`git diff 529eec18..HEAD --stat | grep -iE 'ts|bindings'` → only `.rs`/`tests/`/`docs/` paths), so `just ts-desktop`
was not run.

## Live smoke (§6.3; `THEUTH_LIVE=1`, 2026-10-07, this MacBook)

How to run (documented in both files' module docs):

```
THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=always build-lock cargo test -p web    --test live -- --ignored --nocapture --test-threads=1
THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=never  build-lock cargo test -p web    --test live -- --ignored --nocapture --test-threads=1
THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=always build-lock cargo test -p scrape --test live -- --ignored --nocapture --test-threads=1
THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=never  build-lock cargo test -p scrape --test live -- --ignored --nocapture --test-threads=1
```

`THEUTH_LIVE_IMPERSONATE` (`always` | `on_challenge` | `never`) is the test-local switch for `web.impersonate`
(it builds the `Fetcher` with `with_policy(mode, true, Some("en-US,en;q=0.9"))`); `THEUTH_LIVE_DUMP=1` prints the
first 240 chars of each `read_url` result. Tests assert shape only (tier ∈ fetch/render/api, charset and attempts
present, client present); no browser actor is registered, so the render rung surfaces the issue instead of
rendering. All four runs: `test result: ok. 2 passed; 0 failed`.

### `read_url` — `impersonate: always` (every row `client = wreq_chrome`, `ladder=false`, `charset=utf-8` unless noted)

| url | tier | issue | chars | compacted | took_ms | notes |
|---|---|---|---|---|---|---|
| https://example.com | fetch | - | 157 | 0 | 39 | attempts=1, next_offset=null |
| https://docs.rs/reqwest/latest/reqwest/ | fetch | - | 4155 | 62 | 72 | next_offset=3957; omitted_headings=[TLS, WASM, Optional Features, Unstable Features, Sponsors, Re-exports, Modules, Structs, Traits, Functions, Type Aliases]; `offset=3957` → 4114 chars, next_offset=7926 (chaining) |
| https://news.ycombinator.com/item?id=1 | **api** | - | 242 | 0 | 355 | source_kind=hn-item, `· via https://hn.algolia.com/api/v1/items/1` |
| https://en.wikipedia.org/wiki/Rust_(programming_language) | **api** | - | 4051 | 17 | 308 | source_kind=wikipedia, next_offset=4000 |
| https://httpbin.org/status/403 | fetch | access_challenge (HTTP 403) | 0 | 0 | 451 | attempts=1 |
| https://httpbin.org/status/429 | fetch | rate_limited (HTTP 429) | 0 | 0 | 1405 | **attempts=2** (D2 retry) |
| https://www.jalan.net/ | fetch | empty_shell (empty-shell) | 4001 | 3 | 1240 | **charset=shift_jis**, readable: `# 宿・ホテル予約 - 旅行ならじゃらんnet`, `lang ja`; next_offset=3952 |
| http://abehiroshi.la.coocan.jp/ | fetch | - | 0 | 0 | 526 | **charset=shift_jis**, title `阿部寛のホームページ` decoded; body is a `<frameset>` → 0 chars, no issue |
| https://nowsecure.nl | fetch | empty_shell (empty-shell) | 61 | 0 | 220 | CF-fronted bot-check demo; 200 + JS shell, no vendor marker → `next: rendered: true` |
| https://www.g2.com/ | fetch | access_challenge (HTTP 403 · datadome) | 44 | 0 | 72 | R1-D2 vendor row seen live: `Please enable JS and disable any ad blocker` |
| https://www.reddit.com/r/rust/comments/1g0x9zl/… | fetch | access_challenge (HTTP 403) | 232 | 0 | 483 | source_kind=reddit-old (old.reddit rewrite); "You've been blocked by network security" |
| https://stackoverflow.com/questions/26960537/… | **api** | - | 3421 | 0 | 441 | source_kind=stackoverflow, question + answers via `follow_up` |
| https://www.npmjs.com/package/left-pad | **api** | - | 1027 | 0 | 62 | source_kind=npm |
| https://pypi.org/project/requests/ | **api** | - | 2481 | 545 | 57 | source_kind=pypi |
| https://crates.io/crates/reqwest | **api** | - | 196 | 0 | 636 | source_kind=crates-io |
| https://arxiv.org/abs/hep-th/9901001 | fetch | - | 3964 | 46 | 16042 | notes=["api source arxiv failed: fetch budget of 16.0s exceeded; read the page"], next_offset=3916 — see Open items |
| https://arxiv.org/abs/2106.09685 | **api** | - | 1624 | 9 | 9 | source_kind=arxiv |

### `read_url` — `impersonate: never` (every row `client = reqwest`)

| url | tier | issue | chars | compacted | took_ms | difference from `always` |
|---|---|---|---|---|---|---|
| https://example.com | fetch | - | 157 | 0 | 56 | none |
| https://docs.rs/reqwest/latest/reqwest/ | fetch | - | 4155 | 62 | 77 | none (same next_offset=3957 → 7926 chaining) |
| https://news.ycombinator.com/item?id=1 | api | - | 242 | 0 | 285 | none |
| https://en.wikipedia.org/wiki/Rust_(programming_language) | api | - | 4051 | 17 | 370 | none |
| https://httpbin.org/status/403 | fetch | access_challenge (HTTP 403) | 0 | 0 | 439 | none |
| https://httpbin.org/status/429 | fetch | rate_limited (HTTP 429) | 0 | 0 | 1224 | none (attempts=2) |
| https://www.jalan.net/ | fetch | empty_shell (empty-shell) | 4001 | 3 | 2363 | none; charset=shift_jis |
| http://abehiroshi.la.coocan.jp/ | fetch | - | 0 | 0 | 506 | none; charset=shift_jis |
| https://nowsecure.nl | fetch | empty_shell (empty-shell) | 61 | 0 | 200 | none |
| https://www.g2.com/ | fetch | access_challenge (HTTP 403 · datadome) | 44 | 0 | 76 | none — DataDome blocks both identities |
| https://www.reddit.com/r/rust/comments/1g0x9zl/… | fetch | **empty_shell (empty-shell)** | 38 | 0 | 865 | `always` got a 403 wall, `never` a 200 shell: reddit treats the honest UA and the Chrome fingerprint differently; neither reads |
| https://stackoverflow.com/questions/26960537/… | api | - | 3421 | 0 | 380 | none |
| https://www.npmjs.com/package/left-pad | api | - | 1027 | 0 | 62 | none |
| https://pypi.org/project/requests/ | api | - | 2481 | 545 | 63 | none |
| https://crates.io/crates/reqwest | api | - | 196 | 0 | 982 | none |
| https://arxiv.org/abs/hep-th/9901001 | fetch | - | 3964 | 46 | 16053 | same API budget note |
| https://arxiv.org/abs/2106.09685 | api | - | 1624 | 9 | 9 | none |

### `scrape_url` (`only_main_content: true`, `Source::Auto`; chars = markdown before the tool's cut)

| url | always: client · tier · issue · chars · compacted · took_ms | never: client · tier · issue · chars · compacted · took_ms |
|---|---|---|
| https://example.com | WreqChrome · fetch · - · 157 · 0 · 47 | Reqwest · fetch · - · 157 · 0 · 43 |
| https://docs.rs/reqwest/latest/reqwest/ | WreqChrome · fetch · - · 15394 · 0 · 165 | Reqwest · fetch · - · 15394 · 0 · 171 |
| https://news.ycombinator.com/item?id=1 | WreqChrome · api · - · 226 · 0 · 2353 | Reqwest · api · - · 226 · 0 · 1110 |
| https://en.wikipedia.org/wiki/Rust_(programming_language) | WreqChrome · api · - · 12001 · 17 · 335 | Reqwest · api · - · 12001 · 17 · 340 |
| https://httpbin.org/status/403 | WreqChrome · fetch · AccessChallenge (HTTP 403) · 0 · 0 · 440 | Reqwest · fetch · AccessChallenge (HTTP 403) · 0 · 0 · 442 |
| https://httpbin.org/status/429 | WreqChrome · fetch · RateLimited (HTTP 429) · 0 · 0 · 1222 | Reqwest · fetch · RateLimited (HTTP 429) · 0 · 0 · 1224 |
| https://www.jalan.net/ | WreqChrome · fetch · EmptyShell · 11605 · 4167 · 1510 · charset=shift_jis | Reqwest · fetch · EmptyShell · 11605 · 4167 · 2369 · charset=shift_jis |
| http://abehiroshi.la.coocan.jp/ | WreqChrome · fetch · - · 0 · 0 · 510 · charset=shift_jis | Reqwest · fetch · - · 0 · 0 · 530 · charset=shift_jis |
| https://nowsecure.nl | WreqChrome · fetch · EmptyShell · 61 · 0 · 215 | Reqwest · fetch · EmptyShell · 61 · 0 · 236 |
| https://www.g2.com/ | WreqChrome · fetch · AccessChallenge (HTTP 403 · datadome) · 44 · 0 · 74 | Reqwest · fetch · AccessChallenge (HTTP 403 · datadome) · 44 · 0 · 68 |
| https://stackoverflow.com/questions/26960537/… | WreqChrome · api · - · 3365 · 0 · 379 | Reqwest · api · - · 3365 · 0 · 346 |

### `crawl` / `map_site` (seed `https://docs.rs/reqwest/latest/reqwest/`)

| run | result |
|---|---|
| crawl `limit: 2, max_depth: 1, sitemap: skip` (always) | pages=2 **incomplete=true reason="limit"** skipped=0 invalid_patterns=[] took_ms=693; rows `…/reqwest/` (15394 chars) and `…/reqwest/all.html` (14719), `truncated=false` under the 20 000 per-page cap, `client=WreqChrome` |
| same (never) | pages=2 incomplete=true reason="limit" took_ms=1023, `client=Reqwest` |
| map `limit: 10, max_depth: 1, sitemap: include` | hits=10, by_source={"page": 10} (docs.rs has no sitemap at the seed; with `limit` ≤ pages fetched every hit is a fetched page, so no `frontier` row was produced — `frontier`/`sitemap`/`search` provenance is covered by `crawl.rs`'s wiremock tests) took_ms=5058 / 5616 |

Not reachable from this machine: `https://www.aozora.gr.jp/` (the design's Shift_JIS example) — `curl: (6) Could
not resolve host`, NXDOMAIN under both clients; replaced in the URL lists by `https://www.jalan.net/`
(`Content-Type: text/html;charset=Windows-31J`) and `http://abehiroshi.la.coocan.jp/` (`<meta charset=Shift_JIS>`).

## Deviations from the design

- `harness.md:65` carries `impersonate: always` in addition to §4.6's two keys (R1-D5 added the key after §4.6 was written).
- `provider::Rendered` gained `issue` (not in §7.2); three constructors outside the 122 crates (`browser`, `search-providers` ×2) got `issue: None`.
- `web::render::issue_line` and `web::excerpt::is_noise` are `pub`; `web::fetch::take_title` is new — all small seams that let a duplicate be deleted (W3's `issue_line`, link-rule copy, `first_heading`).
- `compact` returns its input when the passes would empty a non-empty page (D7 lists no such rule; without it the `pdf-as-octet-stream` fixture read as nothing).
- `heading_text` (D6 omitted headings) unwraps links and strips anchor glyphs.
- The `— next:` segment of the ISSUE line is omitted when `next` is empty (W3's shape adopted for both renderers).
- Pre-existing tests changed because the real waves changed the inputs they relied on (all named above): `empty_main_content_falls_back_to_whole_page` → `all_chrome_page_is_not_widened_under_only_main_content`; `page_meta_maps_firecrawl_fields` (precedence); `scrape_response_cuts_with_offset_and_chains`, `crawl_pages_are_compacted_and_cut_per_page`, `offset_and_compact_window_through_fetch_page`, `read_url_pages_with_offset_and_reports_source_api_without_a_match` (unique paragraphs); `access_challenge_is_surfaced_when_no_browser_is_registered` (detail `cloudflare`); `crawl_rows_carry_issue_and_client` and the two W1 stealth tests (`WreqChrome`); scrape's `issue_line` retry pin and `first_heading_reads_atx_only` deleted with their functions.

## Open items (for Sky)

1. **Cloudflare challenge never seen live.** Both `nowsecure.nl` and the other CF-fronted hosts answered 200 with a JS shell (`empty_shell`, `next: rendered: true`) to both clients; no `_cf_chl_opt`/`cf-mitigated`/`Just a moment…` page was served, so D1 rows 1–2/8 and the `always`-vs-`never` difference on a CF challenge are verified by fixtures only. DataDome (R1-D2 vocabulary `datadome`) was seen live on g2.com and blocks both identities; reddit blocks `wreq_chrome` with a 403 wall and serves the honest UA an empty shell.
2. **`export.arxiv.org` hangs on old-style ids.** `id_list=hep-th%2F9901001` and `id_list=hep-th/9901001` both time out (curl, 30 s, no bytes) while `id_list=2106.09685` answers in 25 ms, so W4's `%2F` question is moot — the API itself does not answer old-style ids right now. Under `auto` the page is read after the 16 s API budget with the note `api source arxiv failed: fetch budget of 16.0s exceeded; read the page`. If this persists, `sources::arxiv` could return `None` for two-segment ids (page pipeline directly, no 16 s wait).
3. **`http://abehiroshi.la.coocan.jp/` reads as 0 chars with no issue**: a `<frameset>` page has no body text and no script, so neither `empty_shell` nor any other D1 row fires. A `frameset` row ("content is in frames: read the `src` URLs") would be a one-marker addition to `issue.rs`.
4. **Render tier reports `client: reqwest`** (W3 decision 5): `ClientKind` is frozen with two values; a `browser` variant would make the field truthful (the live smoke had no browser actor, so no row shows it).
5. **h2m `max_depth`** stays at the library default (64); genuinely deep mixed nesting below that is dropped (W2 hand-off). Raising it means `ConversionOptions.max_depth` in `scrape/src/engine.rs::html_to_markdown` **and** `firecrawl-native/tests/corpus.rs` plus a goldens pass.
6. **The orchestration-lessons knowledge note `read_url(max_chars) 200..=16000`** lives under `~/.theuth` (not in this tree): add "`offset` pages through a long page; a cut result names `next_offset`" when you next touch it.
7. **`CrawlResult.reason = "cancelled"`** is declared and never set (W3): no cancellation signal reaches the crawl future (§9 q5).
8. **`ClientError::NotBuilt`** is declared and never produced since W5 (frozen signature kept); `IssueKind::Unsupported` likewise (W1 decision 5).
9. **`map_site` live run produced only `page` rows** (limit reached by fetched pages; no sitemap on docs.rs). `frontier`/`sitemap`/`search` are pinned by `crawl.rs`'s wiremock tests, not seen live.
10. **Workspace check**: the known `nodes` red (`bindings/methods.json`) still stands; a `--keep-going` run once the bindings exist would close the coverage argument above by construction.
