# 153 — Datasets for the packaged technical dictionary (read-only web survey, 2026-10-09 16:50–16:54; child fv-datasets; every fact cited)

Scope: a read-only check of the web on 2026-10-09. I confirmed GitHub facts (licence, stars, last push) through the GitHub search API. I pulled files through the GitHub contents API, raw.githubusercontent and crates.io. Anything I could not confirm is marked "not verified" or "not found".

## 1. Summary — recommended composition of a packaged dictionary

**Recommended packaged dataset, all permissive and attribution-only:**

- **Spoken→written core: talonhub/community (MIT).** It is the most complete and best-maintained voice-coding table. Last pushed 2026-10-08, 879 stars [1].
  - `core/keys/symbols.py`: about 45 symbols, each with separate "dictation" and "command" spoken forms. Examples: "dot"/"point"/"period" → `.`, "down score"/"underscore" → `_`, "L paren"/"left paren" → `(`, "pipe", "caret", "tilde", "back tick", and currency [2].
  - `core/formatters/code_formatter.talon-list`: 17 casing formatters, e.g. snake, camel, hammer (PascalCase), kebab, dotted, dunder, constant, smash, slasher, packed (`::`) [3].
  - `core/abbreviate/abbreviate.py`: 436 spoken word → abbreviation pairs, e.g. "administrator" → admin, "acknowledge" → ack, "J peg" → jpg [4].
  - `core/numbers/numbers.py`: a number-word parser [5].
  - Also available: `core/homophones/homophones.csv` (8.7 KB), `core/file_extension/`, `core/top_level_domain/`, `core/text/*.talon-list` (currency, prose snippets) and per-language keyword/operator lists under `lang/<about 25 languages>` [6].
  - Note that talon's own spoken forms are terse ("hammer", "smash"). Pair them with the plain dictation forms ("camel case", "snake case", "open paren") that FluidVoice also accepts [7].
- **Command and tool names:**
  - Homebrew `formula.json` + `cask.json` (BSD-2-Clause API/site; the names are facts). Each entry has name, aliases, oldnames and desc [8][9].
  - tldr-pages page names from `pages/common/*.md`: more than 1,000 (the contents API caps a listing at 1,000). Pages are CC BY 4.0, scripts MIT [10].
  - oh-my-zsh plugin names: 371 directories under `plugins/`, MIT [11].
- **Languages:** GitHub linguist `lib/linguist/languages.yml`, MIT. About 830 top-level language entries (my regex count over the 166 KB file), with aliases [12].
- **Package names:** hugovk top-pypi-packages, the 15,000 most-downloaded PyPI packages, refreshed monthly as JSON [13]. The repo has no licence file (GitHub reports `None`), but these are bare package names, which are facts [14].
- **Cloud:** botocore `botocore/data/`: 440 entries, about 430 AWS service ids, Apache-2.0 [15][16]. Kubernetes kinds from the OpenAPI spec in kubernetes/kubernetes, Apache-2.0 [17]. Terraform provider names from the public registry API `registry.terraform.io/v1/providers` (paged JSON) [18].
- **AI vocabulary:** models.dev `api.json` (MIT, pushed 2026-10-09, 7.2k stars). It holds provider → model ids and names, e.g. `tencent/Hy3` [19][20]. Hugging Face `/api/models?sort=downloads&limit=N` gives top model ids and orgs [21]. Coding-agent and harness names (Claude Code, Codex, Cursor, OpenCode, Gemini CLI, omp…) need a hand-curated list: I found no maintained open list.
- **Phonetics:** CMUdict, under CMU's BSD-style licence (conditions 1 and 2 plus a disclaimer) [22][23], with `rphonetic` (Apache-2.0: Double Metaphone, Soundex and other commons-codec algorithms) and `strsim` (MIT) [24].

**Approximate size:** 30–40k canonical terms (brew ~8k formulae + casks — count not verified; tldr >1k; PyPI 15k; linguist ~830; AWS ~430; models.dev a few thousand ids, not verified; HF top N as chosen). Plus about 500 spoken-form rules from talon: 45 symbols, 17 formatters, 436 abbreviations, number words.

**Share-alike risk:** keep the following out of the shipped data file:

- Wikipedia lists and glossaries. They are CC BY-SA 4.0 per Wikipedia's licence policy (from general knowledge; I did not fetch the page this run).
- Stack Overflow tag names. These are expected to be CC BY-SA 4.0 too, but the licensing page was behind Cloudflare and not verified.
- The Google/Sproat text-normalisation corpus. Kaggle lists it as **CC BY-NC-SA 4.0** [25], so it is non-commercial and unusable.
- GPL/AGPL/LGPL app tables: FluidVoice (GPL-3.0), VoiceInk (GPL-3.0), Whispering/epicenter (AGPL-3.0), Caster/dragonfly (LGPL-3.0).

Whether a bare list of names extracted from a CC BY-SA page counts as an "adaptation" is legally unsettled. Avoiding those sources costs little, because every domain above has a permissive alternative.

## 2. Spoken-form → written-form tables

| Source | What it holds | File paths | Licence | Size | URL |
|---|---|---|---|---|---|
| talonhub/community | Symbols/punctuation with dictation and command forms, plus currency | `core/keys/symbols.py` | MIT [1] | 6 KB, about 45 symbols | [2] |
| talonhub/community | Casing formatters | `core/formatters/code_formatter.talon-list` (+ `formatters.py`, 16 KB) | MIT | 17 formatters | [3] |
| talonhub/community | Spoken word → abbreviation | `core/abbreviate/abbreviate.py` | MIT | 436 pairs | [4] |
| talonhub/community | Number words, ordinals | `core/numbers/numbers.py`, `ordinals.py` | MIT | 11.6 KB parser | [5] |
| talonhub/community | Homophones, file extensions, TLDs, keys, per-language keywords and operators | `core/homophones/homophones.csv`, `core/file_extension/`, `core/top_level_domain/`, `core/keys/*.talon-list`, `lang/<lang>/` | MIT | csv 8.7 KB; about 25 languages | [6] |
| talonhub/community | Generator that derives spoken forms from identifiers and app names (splits case, expands digits and symbols) — an algorithm worth porting | `core/create_spoken_forms.py` | MIT | 17.6 KB | [6] |
| cursorless | Editor action/target spoken forms such as "chuck", "carve" — editor commands, of little use for dictation | `cursorless-talon/src/spoken_forms.json` | MIT, pushed 2026-10-06 [14] | 8.3 KB | [26] |
| Serenade | Voice-coding grammar (Java/Gradle; `core/`, `grammarflattener/`, `corpusgen/`) | `core/src/…` — exact table file not found | Apache-2.0; last push 2024-06-11, effectively dormant [14] | not found | [27] |
| Caster | Voice-coding rules for Dragon/Kaldi | not inspected | LGPL-3.0 for original code [28] | not found | [28] |
| dragonfly | Speech-grammar framework | n/a | LGPL-3.0 [14] | n/a | [14] |
| VoiceCode | Legacy voice-coding commands | not inspected | no licence detected; last push 2017-08 [14] | not found | [14] |
| Vocola | — | not checked | not found | not found | — |
| FluidVoice | Dictation punctuation table: "open/left parenthesis", "dot dot dot", "forward slash", "em dash", "long dash"…, plus target-app names (codex, claude, cursor, windsurf, ghostty, warp…) | `Sources/Fluid/Services/ASRService+SpokenPunctuationFormatting.swift` (40 KB) | GPL-3.0 [14] — read for ideas, do not copy | 40 KB | [7] |

## 3. Canonical technical term lists

| Source | Domain | Count | Format | Licence | Extraction route | URL |
|---|---|---|---|---|---|---|
| Homebrew formulae/casks | CLI tools, apps | not verified (thousands) | JSON array; `name`, `full_name`, `aliases`, `oldnames`, `desc` | BSD-2-Clause (brew and formulae.brew.sh repos) [14] | `curl formulae.brew.sh/api/formula.json` and `cask.json` [9] | [8] |
| tldr-pages | Command names | >1,000 in `pages/common` (listing capped) | `pages/common/<cmd>.md` (plus linux/osx/windows dirs) | CC BY 4.0 pages, MIT scripts [10] | file names via git clone or the archive | [10] |
| oh-my-zsh | Shell plugin and tool names | 371 | directory names under `plugins/` | MIT [14] | directory listing | [11] |
| GitHub linguist | Programming languages and aliases | about 830 | YAML `languages.yml` | MIT [14] | parse top-level keys plus `aliases` | [12] |
| top-pypi-packages | PyPI names | 15,000 | JSON (`top-pypi-packages.min.json`), monthly | no licence file [14]; names are facts | GET the JSON [13] | [13] |
| botocore | AWS service ids | 440 entries in `botocore/data` (about 430 services) | dir per service; `service-2.json` metadata holds `serviceFullName` | Apache-2.0 [14] | list `botocore/data`, read metadata | [15] |
| Kubernetes | API kinds | not counted | OpenAPI `api/openapi-spec/` | Apache-2.0 [14] | parse `x-kubernetes-group-version-kind` | [17] |
| Terraform registry | Provider namespace/name | not counted; paged | JSON API | registry terms not checked | `GET /v1/providers?limit=&offset=` | [18] |
| models.dev | LLM provider and model ids and names | not counted (one large JSON) | `api.json` provider → models | MIT [20] | GET `models.dev/api.json` | [19] |
| Hugging Face Hub | Model ids and orgs | top N by downloads | JSON API | HF terms not checked | `/api/models?sort=downloads&limit=N` | [21] |
| crates.io | Rust crate names | n/a | db dump `static.crates.io/db-dump.tar.gz` (from general knowledge) | dump terms not found (the data-access page is JS-only) | API also gives per-crate records [24] | [29] |
| Go module index | Go module paths | n/a | streaming JSON feed `index.golang.org/index` | Google service terms not checked | page through by `since=` | [30] |
| npm most-depended | — | not found | — | — | — | — |
| Debian/Arch package lists, man-page names | — | not checked | — | — | — | — |
| Google Cloud / Azure product lists | — | no machine-readable open list found | — | — | — | — |
| Coding agents/harnesses | — | no maintained open list found; curate by hand | — | — | — | — |
| Wikipedia glossaries / list of programming languages | CS terms | — | — | CC BY-SA 4.0 (general knowledge, not fetched) — avoid | — | — |
| Stack Overflow tags | Dev terms | — | — | believed CC BY-SA 4.0; licensing page blocked [31] | — | [31] |
| Jargon File | Hacker slang | — | — | licence not found (catb pages returned only stubs) [32] | — | [32] |
| Microsoft Terminology | IT glossary, ~100 languages | not found | TBX download | page names the download but no licence terms; treat as unusable until checked [33] | Download Center | [33] |
| Apple style glossary | — | not checked; presumed not reusable | — | — | — | — |

## 4. Pronunciation and phonetic matching resources

| Resource | What | Licence | Freshness | URL |
|---|---|---|---|---|
| CMUdict | English pronouncing dictionary (ARPAbet) | CMU BSD-style: keep the notice in source and binary redistributions [22] | last push 2025-10-24 [14] | [22][23] |
| `cmudict-fast` crate | Rust CMUdict loader | MIT/Apache-2.0; 0.8.0; last update 2021 [24] | stale | [24] |
| `rphonetic` crate | Port of Apache commons-codec: Double Metaphone, Soundex and others | Apache-2.0; 4.0.0; updated 2026-09-03; 454k downloads [24] | active | [24] |
| `ttaw` crate | Metaphone, alliteration, CMUdict-based rhyme | MIT; 0.3.1 [24] | 2025-12 | [24] |
| `strsim` | Levenshtein, Jaro-Winkler and others | MIT; 0.11.1; 1.1 B downloads [24] | stable | [24] |
| `triple_accel` | SIMD edit distance | MIT [24] | 2021 | [24] |
| `fst` | Compact FST sets/maps for a large term list | Unlicense/MIT [24] | 2021, stable | [24] |
| `phonetics` | Metaphone, NYSIIS | MIT; 882 downloads [24] | tiny | [24] |
| `grapheme_to_phoneme` | G2P | BSD-4-Clause (advertising clause) [24] — avoid | 2020 | [24] |
| `metaphone` | Metaphone | non-standard licence [24] — avoid | 2018 | [24] |
| phonemizer (Python) | G2P via espeak | GPL-3.0 [14] — avoid | active | [14] |
| Prior art: Handy | Custom-word snapping uses `natural::phonetics::soundex` + `strsim::levenshtein` + n-gram match keys, with case preserved (`apply_custom_words`, `find_best_match`) | MIT | active | [34] |

## 5. Text-normalisation (numbers, dates, units) resources

| Resource | What | Licence | Freshness | URL |
|---|---|---|---|---|
| NVIDIA NeMo-text-processing | WFST (pynini) grammars for text normalisation and **inverse** text normalisation. ITN is the spoken→written direction | Apache-2.0 [14] | pushed 2026-10-06 | [35] |
| Google Sparrowhawk | TN runtime for Kestrel-style grammars | Apache-2.0, **archived** 2018 [14] | dead | [14] |
| Sproat & Jaitly TN data (Kaggle "google-nlu/text-normalization", ~9.8 GB) | Written ↔ spoken aligned corpus (EN/RU/PL) | **CC BY-NC-SA 4.0** [25] — unusable in a product | 2016 | [25][36] |
| talonhub `core/numbers/numbers.py` | Small Python number-word parser, easy to port | MIT | active | [5] |

Recommendation: port the shape of NeMo's English ITN classes (cardinal, ordinal, decimal, money, measure, time, date, electronic) to Rust by hand, or compile its grammars to FSTs offline. Apache-2.0 needs the licence text and a NOTICE. I did not open the per-class file layout in this run.

## 6. What open dictation apps ship today

| App | What ships (dictionary/punctuation) | Licence | URL |
|---|---|---|---|
| Handy (cjpais) | User "custom words" list (`CustomWords.tsx`), fuzzy and Soundex snapping in `src-tauri/src/audio_toolkit/text.rs`, filler-word removal, stutter collapse. Dictionary management also exposed via its Raycast extension. No packaged technical vocabulary found. | MIT, 33k stars [14] | [34][37] |
| FluidVoice | Spoken-punctuation formatter with a large synonym table; custom dictionary (`CustomDictionaryView.swift`, `DictionaryCorrectionEditPolicy.swift`); target-app name list | GPL-3.0 [14] | [7][38] |
| VoiceInk | Personal dictionary/word replacements (from the README features list; table contents not inspected) | GPL-3.0 [39] | [39] |
| Whispering (EpicenterHQ/epicenter) | "Transformations", with ADR 0099 moving to "a dictionary polish and a portable recipe library" | AGPL-3.0-or-later [40] | [38][40] |
| Buzz | Not inspected | MIT [14] | [14] |
| Speech Note (mkiol/dsnote) | Not inspected | MPL-2.0 [14] | [14] |

None of them ships a packaged technical/dev vocabulary that I found. Theuth's dataset would be new.

## 7. Licence notes

- **Ship with attribution only (keep the licence/notice in a THIRD_PARTY file):**
  - MIT: talonhub/community, cursorless, linguist, oh-my-zsh, models.dev, strsim, ttaw, Handy code ideas.
  - BSD-2: Homebrew API data.
  - CMU BSD-style: CMUdict.
  - Apache-2.0: botocore, Kubernetes, NeMo-text-processing, rphonetic, Serenade. Include the licence plus any NOTICE.
  - CC BY 4.0: tldr-pages. Name names-only use in the credits.
  - Bare name lists (PyPI top 15k, HF ids, Terraform providers, Go modules, crates.io names) are facts. Credit the source anyway; service terms not verified.
- **Share-alike — do not mix into the shipped file:**
  - Wikipedia (CC BY-SA 4.0) and Stack Overflow tags (believed CC BY-SA 4.0): the derived data file would have to be CC BY-SA too.
  - LGPL-3.0: Caster, dragonfly.
  - MPL-2.0: Speech Note. File-level copyleft, so copied tables stay MPL.
- **Unusable or unclear:**
  - Kaggle TN corpus (NC).
  - GPL-3.0: FluidVoice, VoiceInk, phonemizer. AGPL: Whispering.
  - BSD-4-Clause: grapheme_to_phoneme. Non-standard: metaphone crate.
  - Licence not found: Jargon File, Microsoft Terminology, crates.io dump terms.
  - Apple glossary: not checked.

## 8. Sources
1. https://github.com/talonhub/community (GitHub search API: MIT, 879★, pushed 2026-10-08)
2. https://github.com/talonhub/community/blob/main/core/keys/symbols.py
3. https://github.com/talonhub/community/blob/main/core/formatters/code_formatter.talon-list
4. https://github.com/talonhub/community/blob/main/core/abbreviate/abbreviate.py
5. https://github.com/talonhub/community/blob/main/core/numbers/numbers.py
6. https://github.com/talonhub/community/tree/main/core
7. https://github.com/altic-dev/FluidVoice/blob/main/Sources/Fluid/Services/ASRService+SpokenPunctuationFormatting.swift
8. https://formulae.brew.sh/api/formula.json
9. https://formulae.brew.sh/docs/api/
10. https://github.com/tldr-pages/tldr/blob/main/LICENSE.md
11. https://github.com/ohmyzsh/ohmyzsh/tree/master/plugins
12. https://github.com/github-linguist/linguist/blob/main/lib/linguist/languages.yml
13. https://github.com/hugovk/top-pypi-packages (README; https://hugovk.dev/top-pypi-packages/top-pypi-packages.min.json)
14. GitHub search API repo metadata (licence, stars, pushed_at), queried 2026-10-09: https://api.github.com/search/repositories?q=repo:OWNER/NAME
15. https://github.com/boto/botocore/tree/develop/botocore/data
16. https://github.com/boto/botocore
17. https://github.com/kubernetes/kubernetes/tree/master/api/openapi-spec
18. https://registry.terraform.io/v1/providers?limit=1
19. https://models.dev/api.json
20. https://github.com/anomalyco/models.dev
21. https://huggingface.co/api/models?sort=downloads&limit=3
22. https://github.com/cmusphinx/cmudict/blob/master/LICENSE
23. https://github.com/cmusphinx/cmudict/blob/master/README
24. https://crates.io/api/v1/crates/{rphonetic,ttaw,metaphone,cmudict-fast,strsim,triple_accel,fst,phonetics,grapheme_to_phoneme}
25. https://www.kaggle.com/api/v1/datasets/view/google-nlu/text-normalization (licenseName "CC BY-NC-SA 4.0", 9.77 GB)
26. https://github.com/cursorless-dev/cursorless/blob/main/cursorless-talon/src/spoken_forms.json
27. https://github.com/serenadeai/serenade
28. https://github.com/dictation-toolbox/Caster/blob/master/LICENSE
29. https://crates.io/data-access (JS-only; terms not readable)
30. https://index.golang.org/
31. https://stackoverflow.com/help/licensing (Cloudflare challenge; not verified)
32. http://catb.org/jargon/ (stub only; licence not found)
33. https://learn.microsoft.com/en-us/globalization/reference/microsoft-terminology
34. https://github.com/cjpais/Handy/blob/main/src-tauri/src/audio_toolkit/text.rs
35. https://github.com/NVIDIA/NeMo-text-processing
36. https://github.com/rwsproat/text-normalization-data
37. https://github.com/cjpais/Handy (README)
38. GitHub code search: repo:altic-dev/FluidVoice punctuation; repo:cjpais/Handy custom_words; repo:EpicenterHQ/epicenter transformation (incl. docs/adr/0099-replace-transformations-with-a-dictionary-polish-and-a-portable-recipe-library.md)
39. https://github.com/Beingpax/VoiceInk (README, LICENSE = GPL-3.0)
40. https://github.com/EpicenterHQ/epicenter/blob/main/LICENSE