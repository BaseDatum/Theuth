# 122 as-built — W4 (COMPACT + SOURCES)

Branch `feat/122-w4`, worktree `~/Develop/theuth-122-w4`, base `529eec18`. Two pure modules in
`crates/web`: D7 `compact` and D11 `sources`. No network, no I/O, no new dependencies; frozen
signatures (§7.2) unchanged.

## What was built

| file | lines | what |
|---|---|---|
| `crates/web/src/compact.rs` | 1–105 types, constants (`INVISIBLE`, `UTILITY_PREFIXES`, `UTILITY_EXACT`, `A11Y_CHROME`, thresholds); 117–143 `compact` (pass order); 146–161 `fingerprint` (A); 163–236 fence tracking (`split_fenced`, `fence_marker`), `blocks`, `retain_unmarked`; 238–249 pass 1; 251–264 pass 2; 266–321 pass 3; 323–368 pass 4; 370–401 pass 5; 403–417 pass 6; 419–437 pass 7; 439–474 pass 8; 476–505 pass 9; 507–607 pass 10 (`fold_links`, `unwrap_links`, `matching_open`, `closing_paren`); 609–659 unit tests | the ten D7 passes in D7's order, each skipping fenced lines (fence markers included) |
| `crates/web/src/sources.rs` | 1–57 types, constants, `WIKIPEDIA_NAMESPACES`; 59–82 `api_source`; 86–98 `follow_up` (new, see Decisions 8); 101–249 the eight matchers + `percent_decode`; 257–269 `render`; 271–328 shared helpers (`html_md`, `head`, `meta_line`, …); 330–615 one renderer per kind; 617–644 unit tests | D11 table and renderers |
| `crates/web/tests/compact.rs` | 213 lines | 17 contract tests (below) |
| `crates/web/tests/sources.rs` | 347 lines | 19 contract tests (below) |
| `crates/web/tests/fixtures/compact/chrome-heavy.md` | new | synthetic chrome-heavy page: 3× carousel of three testimonials, 12-logo image row, four Tailwind-leak lines, duplicated nav block, `Skip to content`, `(opens in new tab)`, U+200B/U+00AD/U+FEFF inside words, leaked entities, a comma list with repeats, empty and trailing headings, a fence with three identical lines |
| `crates/web/tests/fixtures/compact/clean-article.md` | new | synthetic docs page (seven `##` sections, four fences, one table); `compact` changes 0 chars |
| `crates/web/tests/fixtures/sources/hn_item.json` | new | Algolia `items/{id}` shape: story with text, 4-deep comment chain, a deleted (`text: null`) comment, a comment with a link |
| `crates/web/tests/fixtures/sources/npm_package.json` | new | registry document for a scoped package: `dist-tags`, two `versions`, object `repository`, top-level `readme` |
| `crates/web/tests/fixtures/sources/pypi_project.json` | new | `info` with `license_expression` + full licence text in `license`, empty `home_page`, `project_urls`, RST description |
| `crates/web/tests/fixtures/sources/crates_io_crate.json` | new | `crate` with `max_version` (rc) and `max_stable_version`, `keywords` strings |
| `crates/web/tests/fixtures/sources/arxiv_abs.xml` | new | `id_list` Atom feed: one entry, two `<author><name>`, multi-line title, `<link title="pdf">` |
| `crates/web/tests/fixtures/sources/wikipedia_extract.json`, `wikipedia_missing.json` | new | `action=query&prop=extracts` page keyed by id; the `missing` page |
| `crates/web/tests/fixtures/sources/stackoverflow_question.json`, `stackoverflow_answers.json` | new | `questions/{id}?filter=withbody` and `…/answers` with 5 answers (accepted one is not the top-scored) |

## Decisions beyond the design (each with reason)

1. **Fence lines are tracked per line, fences are block separators.** A `Line { text, fenced }` vector is the
   working form; the fence markers themselves are `fenced` so no pass can alter them. For passes 5/6 a block is a
   maximal run of non-blank, non-fenced lines, so a fence directly under a paragraph ends that block. An unclosed
   fence runs to the end (CommonMark).
2. **Pass 1 uses `engines::decode_entities`** (`crates/web/src/engines/mod.rs:262`), already in the crate: it decodes
   D7's six entities plus `&apos;` and numeric `&#…;`. One decoder in the crate rather than a second table; the
   superset is harmless on page text. Consequence: a page that *displays* `&amp;lt;` decodes one level per run, so
   idempotence is exact for the fixtures but not for text that is literally about entities.
3. **Trailing newline preserved, leading/trailing blank lines dropped.** Pass 9 joins with at most one blank line
   and emits exactly one trailing `\n` when the input ended with one, so a clean document round-trips to
   `text == input` (D7 target) and `removed_chars` stays exact.
4. **Pass 3 accepts link-wrapped and list-item images** (`[![alt](src)](href)`, `- ![alt](src)`): h2m emits logo
   rows that way. `[N images]`/`[N images: alt]` is kept under `keep_images: false` (it is a summary, not an image
   line). Inline images in prose are untouched by pass 3 (pass 10 folds them to their alt).
5. **Pass 4 also drops the parens the phrase sat in** (`(opens in new tab)` → nothing, not `()`), removes `[](url)`
   links left by a removed label, and treats a line reduced to whitespace/list markers as empty.
6. **Pass 5/6 skip blocks and lines whose fingerprint A is empty** (table separators, rules): a second table's
   `|---|---|` row must survive.
7. **`removed_chars` saturates at 0**: `fold_links` can lengthen a link-dense page by its `Links:` list (the
   contract type is `usize`). Pinned by `fold_links_lists_deduped_capped_links` on a 70-link input.
8. **`sources::follow_up(&ApiSource) -> Option<Url>` added beside the frozen signatures.** D11's Stack Overflow row
   needs two endpoints (question, then answers) but `ApiSource` carries one `fetch_url` and `render` one `body`.
   `fetch_url` is the question endpoint; `follow_up` names the answers endpoint for `stackoverflow` and `None`
   otherwise; `render("stackoverflow", body)` accepts the two response bodies concatenated (newline-separated JSON
   stream) **or** wrapped in one JSON array, and also renders the question alone. Signatures unchanged.
9. **`render("reddit-old", _) == None`** as D11 says (n/a): the body is HTML for the normal page pipeline — see
   Hand-offs.
10. **HTML fragments (HN texts, Stack Exchange bodies) go through `html_to_markdown_rs::convert` with default
    options** (fenced code, autolinks) from `sources.rs` directly: `fetch::text::html_to_markdown` is `pub(super)`
    and strips `form/nav/footer/…`, which fragments do not have. Same crate, no new dependency.
11. **arXiv rendering reuses `engines::arxiv::parse_atom`** for title/url/abstract/published; authors (`<name>`) and
    the `title="pdf"` link are two small scanners (`RawResult` has no author field). The PDF link falls back to
    `/abs/` → `/pdf/` of the entry URL.
12. **Wikipedia title → namespace filter**: `Special:`, `File:`, `Talk:`, `User_talk:` … (table `WIKIPEDIA_NAMESPACES`,
    plus any `*_talk`) return `None`; a colon inside an article title (`Star_Wars:_Episode_IV`) still matches.
    `fetch_url` is the page URL with path/query/fragment/port replaced (`set_path("/w/api.php")` + `query_pairs_mut`),
    so no user string is formatted into a URL. Hosts: exactly `<lang>.wikipedia.org`, `lang` lowercase letters/`-`,
    not `m`; `www.` is stripped first for every row.
13. **Percent-decoding is a 20-line local function** (`percent_decode`): `path_segments()` yields encoded segments and
    re-pushing them through `url` would double-encode `%`; `url` does not re-export its `percent-encoding` dependency
    and the brief forbids new ones.
14. **Per-kind details not fixed by D11**: npm prefers the `dist-tags.latest` version object's `description`/`license`/
    `homepage`/`repository` over top-level, strips `git+`/`.git`; PyPI uses `license_expression` and ignores a `license`
    longer than 80 chars (PyPI stores whole licence texts there), `home_page` → `project_urls.Homepage` → `Source`;
    crates.io prefers `max_stable_version` over `max_version`; HN skips comments with `text: null` (deleted/dead) and
    collapses each comment to one line so the nested list stays a list; heads (`README`, PyPI description 3 000 chars,
    Wikipedia extract 12 000 chars) end with `…` when cut.
15. **`render` returns `None` for an empty result** (after trimming) so the `auto` fallback runs the page pipeline.

## Tests

`crates/web/tests/compact.rs` (17): `pass1_strips_invisible_chars_without_joining_words`, `pass1_decodes_leaked_entities`,
`pass2_drops_utility_class_lines_only_past_half`, `pass3_folds_image_runs_keeping_first_alt`,
`pass3_drops_remaining_images_without_keep_images`, `pass4_removes_a11y_chrome_and_emptied_lines`,
`pass5_dedups_blocks_by_full_and_prefix_fingerprint`, `pass6_dedups_lines_inside_one_block_only`,
`pass7_dedups_comma_lists_of_four_or_more`, `pass8_drops_empty_and_trailing_headings`,
`pass9_collapses_blank_runs_and_trailing_whitespace`, `fenced_code_is_untouched_by_every_pass`,
`fold_links_lists_deduped_capped_links`, `links_stay_inline_without_fold_links`,
`chrome_heavy_page_loses_at_least_15_percent` (also checks what every pass did on that page),
`clean_article_is_unchanged`, `compaction_is_idempotent` (both fixtures, default and `fold_links + !keep_images`).

`crates/web/tests/sources.rs` (19): `hn_item_table_row`, `reddit_thread_table_row`, `npm_table_row`, `pypi_table_row`,
`crates_io_table_row`, `arxiv_table_row`, `wikipedia_table_row`, `stackoverflow_table_row` (each positive + negative
URLs incl. `news.ycombinator.com/news`, `reddit.com/r/x`, `Special:Search`, look-alike hosts),
`unrelated_hosts_match_nothing`, `render_hn_item` (depth 4 cut, deleted comment skipped), `render_hn_caps_comments_at_60`,
`render_npm`, `render_npm_cuts_readme_head`, `render_pypi`, `render_crates_io`, `render_arxiv` (both the `abs` fixture and
the existing search fixture), `render_wikipedia` (incl. `missing` → `None`, 12 000-char head),
`render_stackoverflow_top_three_accepted_first` (5 → 3, accepted first, question alone, array-wrapped),
`render_rejects_garbage_and_unknown_kinds`.

Unit tests in the modules: `compact::tests::{fingerprint_is_lowercase_alphanumerics_with_single_spaces,
fence_state_tracks_openers_and_closers, image_only_lines_are_recognised, links_unwrap_innermost_first_with_balanced_parens,
heading_levels}`, `sources::tests::{percent_decoding, wikipedia_language_labels, head_marks_a_cut}`.

The wiremock `read_url` test of D11 ("Algolia host pointed at the mock → `tier == "api"`") needs W1's wiring and the
closer's `fetch_url` host override; not in W4's files.

## Gate

```
$ cargo fmt -p web
(exit 0)
$ build-lock cargo test -p web --no-fail-fast > /tmp/122-W4-test.log 2>&1; echo EXIT=$?
EXIT=0
     Running unittests src/lib.rs (web-de60e2c87bc2da22)
test result: ok. 95 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s
     Running tests/aggregate.rs
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
     Running tests/compact.rs
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running tests/engines.rs
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
     Running tests/fetch_ssrf.rs
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/http_request.rs
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
     Running tests/request.rs
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/sources.rs
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
     Running tests/url.rs
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests web
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
$ build-lock cargo clippy -p web --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 58s   (exit 0)
```

### Pre-existing

None red in `cargo test -p web`.

## Hand-offs

- **Closer / W1 (`read_url` wiring) and W3 (`scrape_url`)** — call shapes are C's: `compact(md, &CompactOptions { fold_links: format == text, keep_images: false })`
  for `read_url`, `{ keep_images: true, .. }` for scrape/crawl; `api_source(&url)` then `render(kind, &body)`.
- **Closer — `reddit-old`**: `render("reddit-old", _)` is `None` by design (D11 "n/a"). The wiring must treat this
  kind specially: fetch `fetch_url` and run the **HTML page pipeline** on the body (title, meta, `html_to_markdown`),
  set `source_kind: "reddit-old"`, `tier` stays `fetch`; do **not** treat the `None` as "api source failed".
  (`crates/web/src/sources.rs:257-269`.)
- **Closer — `stackoverflow`**: after fetching `source.fetch_url` (question), fetch `sources::follow_up(&source)`
  (answers, `crates/web/src/sources.rs:86-98`) and pass `format!("{question_body}\n{answers_body}")` to
  `render("stackoverflow", ..)`. If the second fetch fails, rendering the question body alone still works. Both URLs
  go through the SSRF guard like any other.
- **Closer — `follow_up` is a new public function** beside the frozen three; reflect it in §7.2 when the design is
  updated (Decision 8).
- **W1/closer — `fetch::text::html_to_markdown` visibility**: `sources.rs:289-301` converts HTML fragments with
  `html_to_markdown_rs` directly because the fetch helper is `pub(super)` and strips page chrome. If the closer wants a
  single converter, widening the helper and passing the fragment options is a two-line change in W1's file, not W4's.
- **Sibling stubs called**: none. `compact.rs` and `sources.rs` call only `engines::decode_entities`
  (`sources.rs:13`, `compact.rs:10`), `engines::arxiv::parse_atom` (`sources.rs:12`, `:455`) and `fetch::collapse_ws`
  (`sources.rs:14`) — all pre-existing real code, not 122 stubs.
- **Design note for the closer's smoke**: the arXiv `id_list` endpoint and `at()`-built registry URLs encode `/` in
  old-style arXiv ids as `%2F` (`id_list=hep-th%2F9901001`); verify once live that export.arxiv.org accepts it (the
  API documents `id_list` as comma-separated ids; percent-encoding is standard, unverified here — no network in W4).

## Open items

- `removed_chars` cannot express growth (Decision 7); if the closer wants `format: text` to report the `Links:` list
  cost, that is a `FetchResponse` field, not a `compact` change.
- Pass 1 entity decoding is one level per run (Decision 2); exact idempotence on text literally about HTML entities
  would need a "decode only when the entity could not have been intended" heuristic, which D7 does not ask for.
- D11 renders HN `created_at`/arXiv `published` verbatim (ISO strings from the APIs); `date::normalize` is W1's
  meta line concern, not the body.
