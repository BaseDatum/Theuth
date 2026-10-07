# 122 as-built — W2 (EXTRACT: scoring, noise valves, islands, rails, corpus)

Branch `feat/122-w2`, worktree `~/Develop/theuth-122-w2`, base `529eec18` (wave C contract). Everything
lives in `crates/firecrawl-native/**`; no other crate and no `Cargo.toml` was touched. Every frozen
signature of §7.2 (`select_main`, `may_strip`, `is_noise`, `extract_islands`, `compile_path_patterns`)
kept its name, arguments and return type; bodies filled.

## What was built

| file | lines | what |
|---|---|---|
| `src/noise.rs` | 1–179 (tests 181–291) | D8 valves. `TEXT_VALVE_CHARS` 5 000, `FORM_NOISE_CHARS` 500, `FORM_NOISE_CLASS`, `STRUCTURAL_ID_SUFFIXES`, `CONSENT_PREFIXES` (14, ASCII case-insensitive prefix), `NOISE_TAGS`, `NOISE_ROLES`; `text_len`/`collapsed_len` (whitespace-collapsed char count, shared with the scorer); `has_consent_prefix` (new, consulted by `transform_html`); `may_strip` (:129) and `is_noise` (:149). The id/class token set of `is_noise` is derived from `html::EXCLUDE_NON_MAIN_TAGS` (now `pub(crate)`), so the 42 stay the single source. |
| `src/extract.rs` | 1–394 (tests 396–666) | D8 `select_main` (:340): step 1 `<main>`/`<article>`/`[role=main]`; step 2 one-pass scorer `collect_candidates` (:158) over `traverse_inclusive` with per-element text/anchor/paragraph frames bubbling to the parent, candidates `article, main, [role=main], div, section, td` outside noise at depth ≤ `MAX_DEPTH` 200; `score` (:96) is the D8 formula verbatim, `link_density` (:85) = anchor chars / text chars; ties by document order (`best_candidate`, :252); step 3 `<body>`; step 4 `recover_noscript` (:294) against `original`; invariant 1 (explicit scope / real `<main>` returned as is); invariant 2 body retry (< 200 words, body > 2× and > 50). `CandidateStats` is public so the formula is testable as data. |
| `src/islands.rs` | 1–230 (tests 232–323) | D9 `extract_islands` (:191): `__NEXT_DATA__`, `application/json`, `ld+json` → `articleBody` only (`Collector::walk` with a key filter), SvelteKit `kit.start(… data: […])` via a string-aware bracket matcher (`array_end`) and trailing-comma strip; strings ≥ 40 chars, not URL-like (`^[a-z]+://`, `^/`, `^www.`), containing whitespace; D7 fingerprint-A dedup; `MAX_CHUNKS` 1 000 / `MAX_CHARS` 500 000; joined with blank lines. |
| `src/html.rs` | 397, 476–484, 576–642, 775–784, 786–824, 825–863, 865–970, tests 1283–1410 | `EXCLUDE_NON_MAIN_TAGS` → `pub(crate)` (still 42). `transform_html` now delegates to **new** `transform_html_with_method(opts) -> Result<(String, Option<Method>)>` (:482). Under `only_main_content`: strip pass gated by `may_strip` (:588); consent-prefix pass via `has_consent_prefix` + `may_strip` (:595); `<form>` pass — noise forms detached, content forms unwrapped to `<div>` (:606); `select_main` hook on the serialised stripped document against `opts.html`, result re-parsed (:626). D12 rails for every call (:639): `collapse_wrapper_chains` (:789), `unwrap_layout_tables` (:829, `LAYOUT_CELL_BLOCKS`), `normalise_code_languages` (:946) with `code_language` (:927, public) and the `LANG_PREFIXES`/`BARE_LANGS` tables. `new_element` (:775) builds a fresh element by parsing (kuchikiki exposes no `QualName` constructor without an html5ever dependency). |
| `src/lib.rs` | 19 | re-exports `CONSENT_PREFIXES`, `has_consent_prefix` beside `is_noise`/`may_strip`; `transform_html_with_method`/`code_language` come through `pub use html::*`. |
| `tests/corpus.rs` | 1–158 | runs `transform_html_with_method` → h2m (engine options verbatim) → `post_process_markdown`; asserts golden, `min_words`, `must_contain`, `must_not_contain`, **`method`** (serde name of `Method`) and **`tier_hint`**: `"island"` ⇔ `extract_islands(html)` yields ≥ 800 chars (every other hint asserts it does not). `issue` is left to `web`'s `issue.rs`. |
| `tests/fixtures/*` | 22 HTML + 22 `.expected.md` + 22 `.expect.json` | §6.1 rows 1–16 (row 8 as two files: `consent-wall-onetrust` and `consent-wall-onetrust-wall`) + R1-D2 `px-block`, `incapsula-block`, `recaptcha-interstitial`, `kasada-block`, `recaptcha-form-benign`. All synthesised (template prose about a fictional harbour; marker strings only; no live fetch). `docs-page.html` edited: the bash fence is `<pre class="highlight"><code class="sh">`, the rust fence `<pre class="lang-rust">` — same golden, now exercising the language normalisation. |

Commit history: `a3518046` corpus · `51c2a071` code · `f54f4e5a` **provisional pre-D8 goldens** (generated with the C stubs, the reference the closer diffs against) · this commit: final goldens + as-built.

### Intentional golden changes (provisional `f54f4e5a` → final)

| fixture | change | cause |
|---|---|---|
| `docs-page` | ```` ``` ```` → ```` ```bash ```` on the curl fence | code-language normalisation (`code.sh` → `language-bash`) |
| `news-article` | `## Comments (3)` + three comments removed | `Method::Article` selection |
| `turnstile-embedded-benign` | `## Leave a comment` removed | `Method::Article` selection |
| `layout-table-page` | one-row pipe "table" with 3 huge cells → `## Overview` / `## What you get` (list) / `## Who it is for` sections; data table unchanged | layout-table pre-pass |
| `deep-dom-200` | empty → heading + paragraph | `collapse_wrapper_chains` (h2m's default `max_depth` is 64 — see Decisions 5) |
| `form-wrapped-aspnet` | empty → `# Mooring fees 2027` + article | `<form>` rule: content form unwrapped to `<div>` before h2m's `remove_forms` |
| `unclosed-header-wrapper` | empty → `# The lighthouse keeper's ledger` + article | text valve keeps the 6 000-char `div.header`; `div.post` wins the score |
| `spa-shell-empty` | empty → `You need to enable JavaScript to run this app.` | `<noscript>` recovery (9 words ≥ 5 and > 0) — correct per D8 step 4; still `empty_shell` for W1 (prose 47 chars) |

Unchanged: the eight wall fixtures, `spa-shell-next` (empty), `consent-wall-onetrust*`, `carousel-dupes`, `tailwind-leak`, `recaptcha-form-benign`, `login-page`.

### Final `method` per fixture (hand-off to W3's engine tests)

`main`: docs-page, layout-table-page, carousel-dupes, recaptcha-form-benign · `article`: news-article, turnstile-embedded-benign · `scored`: consent-wall-onetrust, deep-dom-200, tailwind-leak, unclosed-header-wrapper, form-wrapped-aspnet · `body`: spa-shell-next, spa-shell-empty, cf-challenge, cf-blocked-1020, consent-wall-onetrust-wall, login-page, akamai-challenge, px-block, incapsula-block, recaptcha-interstitial, kasada-block. `tier_hint: island` only for spa-shell-next (`extract_islands` → 1 300+ words).

## Decisions taken beyond the design

1. **A candidate must score > 0.** D8 says `ln(text_len)` is 0 below 50 chars; a zero-score element is not a candidate (else every page with an empty `<div id="__next">` would be `Scored`, and §6.1 expects `body` for the shells and walls). Wall fixtures therefore keep their short text directly under `<body>`, as the real pages mostly do.
2. **`<form>` rule made to hold end to end.** The 42 selectors never match a `<form>`, and html-to-markdown-rs's `remove_forms: true` (the engine's `strip_chrome` option) deletes every `<form>` downstream — so `may_strip`'s form rule alone could not keep `form-wrapped-aspnet`. `transform_html` now applies the rule itself under `only_main_content`: `is_noise(form)` → detach, else unwrap to a `<div>` (attributes dropped). `only_main_content: false` leaves forms alone.
3. **Explicit scope → `Method::Body`, `scoped: true`, input unchanged.** The enum has no "explicit" variant; the include-tags document is returned whole (not narrowed to a `<main>` it may contain — narrowing is as much "widening" as the invariant forbids).
4. **`<noscript>` recovery matches ancestors by `(tag, id, class)`** between the stripped and the original DOM (no node identity across two parses); when the selection is `<body>` every `<noscript>` qualifies. Recovered sections are **appended** to the selection; html5ever parses `<noscript>` as raw text (scripting enabled), so the text is re-parsed as `<section>…</section>` — element children (scripting disabled) are serialised the same way.
5. **`collapse_wrapper_chains` rail (D12 depth guard).** html-to-markdown-rs 3.12 silently drops content below `max_depth` = 64 (`NATIVE_STACK_SAFE_DEPTH`; the engine does not set `max_depth`), so `deep-dom-200`'s paragraph was lost regardless of my 200-level scorer guard. A `<div>` whose only non-blank child is another `<div>` is replaced by that child, after `select_main` (scoring sees the original structure) and before h2m. Siblings, text and non-div wrappers are untouched (`tests_122_depth`).
6. **Code language — finding on h2m 3.12.4** (`src/converter/handlers/code_block.rs:229-262`): it reads `language-X` then `lang-X` from `<pre class>`, falling back to the first `<code>` child's class; first match wins; **no normalisation** (`language-py` → `py`); `highlight-X`, `sp-X` and bare names are ignored. The pre-pass rewrites `<pre class>` to `language-<norm>` whenever `code_language` finds a language on the `<pre>` or its first `<code>`; the `<code>` class is left as is because h2m prefers the `<pre>`. `BARE_LANGS` has 44 common names; the normalisation map is exactly the design's (`py`, `sh|zsh|shell`, `yml`, `ts`) plus lowercasing.
7. **`is_noise` applies the text valve to every rule** (a `div.header` with 6 000 chars is not noise), otherwise `unclosed-header-wrapper` would have no candidate at all. `NOISE_ROLES` = navigation, banner, contentinfo, complementary, dialog, alertdialog, search (the design says "role" without a list).
8. **Structural-id exemption** applies to id/class rules only; a tag rule (`header|footer|nav|aside`) still strips `<nav id="root">`.
9. **Link-density "shared test vector".** `web::excerpt::link_density` measures URL chars inside `](…)` on markdown; the DOM rule measures anchor *text* chars — different quantities, so no single vector can equal across crates. What is pinned in `extract::tests::density_penalties` is the threshold/multiplier table (0.3/0.5/0.7, ×0.5/×0.1, ×0.5/×0.3) and `link_density(5, 15) = 1/3`; the closer may mirror the threshold assertions in `web`.
10. **`tier_hint` in `corpus.rs`** is asserted through `extract_islands` only (island ⇔ ≥ 800 chars); `fetch` vs `render` is the engine's call and is not observable here.
11. **Corpus has 22 HTML files, not 21**: §6.1 row 8 is "two HTML files" by design; both carry an `expect.json`.

## Tests

- `noise::tests`: `text_valve_keeps_large_matches`, `form_rule`, `structural_id_exemption`, `consent_prefixes`, `noise_by_tag_role_and_token`, `collapsed_len_counts_single_spaces`.
- `extract::tests`: `score_matches_formula`, `density_penalties`, `scored_candidate_wins_by_class_hint_and_paragraphs`, `ties_keep_document_order`, `semantic_regions_first`, `invariant_1_explicit_scope_is_final`, `invariant_2_body_retry` (190 vs 400 → body; 190 vs 300 → not; scoped never), `no_candidate_is_body`, `noscript_recovery_thresholds`, `depth_guard_200`, `noise_and_inside_noise_are_not_candidates`.
- `islands::tests`: `next_data_island`, `sources_json_ld_and_sveltekit`, `filters_short_urls_keys_and_duplicates`, `caps` (2 000-string island → 1 000 chunks; 600 × 1 000-char strings → ≤ 500 000 chars), `fingerprint_collapses`.
- `html::tests_122`: `code_language_table`, `pre_classes_are_rewritten_for_the_converter`, `layout_tables_become_sections_data_tables_stay`, `nested_layout_table_keeps_inner_data_table`, `forms_follow_the_form_rule_only_when_only_main_content`, `consent_overlays_stripped_and_valves_hold`, `main_wins_and_srcset_largest_is_chosen`; `html::tests_122_depth::wrapper_chains_collapse_but_siblings_stay`.
- `crawler::tests` (22 upstream + C's `compile_path_patterns_reports_invalid_and_keeps_rest`): unchanged, green; `compile_path_patterns` not touched.
- `tests/corpus.rs::corpus_goldens_and_expectations`: 22 fixtures, goldens + every `expect.json` key but `issue`.

### Gate

```
$ cargo fmt -p firecrawl-native
(exit 0)
$ build-lock cargo test -p firecrawl-native --no-fail-fast
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s   (lib)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s    (tests/corpus.rs)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc-tests)
TEST_EXIT=0
$ build-lock cargo clippy -p firecrawl-native --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.40s   (exit 0)
```

## Hand-offs

- **W3 / closer — `engine.rs:383-385` fallback.** `select_main` is real: an empty `<main>` is now returned empty (invariant 1), a thin scored result retries with `<body>` (invariant 2). The old "empty main → whole page" fallback in `scrape`'s `to_markdown` can go; if it stays it only re-runs the page without `only_main_content` when a real `<main>` is empty, which invariant 1 says should stay empty.
- **W3 / closer — method is observable.** `firecrawl_native::transform_html_with_method(opts) -> Result<(String, Option<Method>)>` (`html.rs:482`); `transform_html` is unchanged in shape. `Method` serialises lowercase (`main|article|scored|body`); the per-fixture table above is what the engine tests may assert.
- **W3 / closer — h2m `max_depth`.** `scrape/src/engine.rs::html_to_markdown` does not set `ConversionOptions.max_depth`; h2m's default is 64 and content below that depth is dropped. `collapse_wrapper_chains` covers div-chain soup; a page with genuinely deep mixed nesting would still need `max_depth: Some(…)` (clamped to 1 024 by h2m) in the engine's options. `corpus.rs` copies the engine's options verbatim, so the goldens reflect the engine as it is.
- **W3 — island rung.** `extract_islands` returns `Some` only when ≥ 1 chunk qualifies; `spa-shell-next` yields ≥ 1 300 words, `spa-shell-empty`/every other fixture `None` or < 800 chars (asserted in `corpus.rs`). Output is paragraphs joined by blank lines, ready for `post_process_markdown`.
- **W1 — `issue` / `tier_hint`.** `expect.json` carries them for every fixture; `corpus.rs` does not assert `issue`. The fixtures that must yield an issue by relative path: cf-challenge (rows 2/8), cf-blocked-1020 (row 8 `attention required`), akamai-challenge (row 7, 467 bytes), consent-wall-onetrust-wall (row 13: title + prose 0), login-page (row 14), spa-shell-next/spa-shell-empty (rows 15/16), px-block (7a), incapsula-block (7b), recaptcha-interstitial (7c, prose ≈ 80 chars), kasada-block (7d); `null` for turnstile-embedded-benign (103 711 bytes ≥ 100 000), recaptcha-form-benign (prose ≈ 2 900 chars), consent-wall-onetrust (prose ≈ 2 600 chars) and the rest.
- **W4 — compact targets.** `carousel-dupes` (3× carousel, 12-logo row, two U+200B), `tailwind-leak` (five utility-class lines) and `news-article` are in place; `docs-page`/`layout-table-page` goldens should compact to 0 removed chars.
- **Closer — call sites against sibling stubs:** none. W2 calls no `web`/`scrape` code; `corpus.rs` depends on `html-to-markdown-rs` (C's dev-dependency) only.
- **Closer — design text.** §7.2 could note `transform_html_with_method`; D8's step 2 could state "a candidate scores > 0"; D12's depth row could name h2m's 64-level ceiling.

## Open items

None blocking. Observations: (a) `turnstile-embedded-benign.html` is 103 711 bytes, 92 KB of which is generated CSS — the size gate needs ≥ 100 000 bytes and the golden stays 9.7 KB; (b) the `(tag, id, class)` ancestor match of `<noscript>` recovery is approximate when a page repeats the same wrapper signature — acceptable for a < 30-word selection.
