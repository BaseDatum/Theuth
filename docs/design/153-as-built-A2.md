# 153 as-built — A2 dictionary: matcher + storage + packaged technical dataset

Branch `feat/153-a2`, cut from PIN `cbaf4259`. Design §5 (`153-voice-anywhere.md`), datasets sheet `153-datasets.md`.

## Files
- `crates/voice/src/dictionary/mod.rs` — `Dictionary` (three layers), `lookup` / `lookup_scoped` / `put` / `delete` / `list` /
  `negative` / `get` / `pairs` / `counts` / `packaged_version`; `matcher.rs` — `norm`, `metaphone`, `score`, `score_norm`, `Index`;
  `packaged.rs` — `include_bytes!` + sha256 verify + gunzip + index; `store.rs` — the user JSON file; `tests.rs`.
- `crates/voice/build-dictionary/build.py` (stdlib Python, run by hand, output committed; `cache/` gitignored) + `.gitignore`.
- `crates/voice/data/dictionary.v1.json.gz` (296 865 B), `dictionary.json` (record), `THIRD_PARTY.md`.
- `crates/voice/Cargo.toml` (+ `serde_json`, `flate2`, `sha2`, all workspace deps) and `Cargo.lock` — C's "Cargo deps" file; hunk is 3 lines.

## Packaged dataset (`dictionary.json`)
40 641 canonical terms, 492 spoken-form pairs, gz 290 KiB (budget 1.5 MiB → `include_bytes!`, as §5.2). Nothing skipped.
Per source (pre-merge; merge key = kind + `norm(canonical)`): curated 149 · talon symbols 37 / formatters 16 / abbreviations 436 /
numbers 40 / extensions 35 / TLDs 8 · models.dev 8 691 · Hugging Face top 2 000 (ids + orgs) 3 992 · linguist 822 · Kubernetes kinds 158 ·
botocore 1 006 (ids + full names + abbreviations as separate terms) · Homebrew formulae 8 641 / casks 7 780 · tldr `common` 4 697 ·
oh-my-zsh 370 · top-pypi 15 000. Terraform registry and CMUdict not used (terms unchecked / not needed — spoken forms come from
normalisation, not pronunciation). Share-alike and GPL sources excluded as §5.1.
Packed term shape `{c, k, s, t}`; `k` is serde `Kind`. Spoken forms that normalise to the canonical are not stored.

## What the design said vs what I built
- §5.2 `PackagedSet { terms, symbols, formatters, abbreviations, tlds, extensions }` → one `terms` list; symbols/formatters/abbreviations
  are entries with `kind`, TLDs/extensions `kind: path` + tag `tld`/`ext`, number words tag `number`. Read via `Dictionary::pairs(kind)`.
- §5.5 storage in `voice.db` → `<data_dir>/voice/dictionary.json` (per brief: self-contained, atomic temp+rename). A3's sqlite is untouched.
- §5.4 step 1 windows: the frozen `lookup(heard)` takes ONE window; the shaper (A1) iterates 1–3-token windows, longest first.
- §5.4 `norm`: lowercase alphanumerics, digit words → digits (`k eight s` → `k8s`, `python three` → `python3`). Spaces dropped, so
  `git hub` = `GitHub`. The spelled-letter fold is implicit (`T D T` → `tdt`); letter *names* (`tee`) are not folded.
- Candidates: exact bucket + Double Metaphone primary/alternate buckets (code length 8, ASCII letters only — `rphonetic` panics on
  multibyte input), ≤ 32 per layer. Score capped at 1.0; an exact hit ends the scan (User > Learned > Packaged, catalog order).
- **Deviation (quality, evidence in `tests::prose_is_left_alone`):** packaged entries match on the normalised form only — no fuzzy /
  phonetic acceptance at `ACCEPT_NEW`. With the design's rule on 40k names, 108 of 300 plain-English forms were rewritten (`and` → wand,
  `plan` → Plane, `this` → Straight Flush, `man` → Roff). Fuzzy stays for User (`ACCEPT_KNOWN`) and Learned (`ACCEPT_NEW`) entries.
  Likewise source aliases that rewrite in the replacement direction (brew `aliases`/`oldnames`, linguist aliases, cask tokens, bare-word
  model ids) are not packaged as spoken forms; curated, talon and digit/hyphen model ids are. Probe after: 0 of 300 rewritten.
- Casing (§5.4 step 8): a case-only hit applies when the entry is User/Learned, tagged `curated`, or mixed-case (`GitHub`, `macOS`);
  Title-case cask names (`Play`, `Tomorrow`) and ALLCAPS language names (`STAR`) leave prose alone. `keep_case` (every all-lowercase
  packaged command/package name) never recases. Sentence-initial capital re-applied to lowercase non-`keep_case` canonicals.
- `lookup` never returns Symbol / Formatter / Abbrev / Path entries or `number`-tagged terms (`replaces()`): those are the shaper's own
  stages' pairs (§4.2 2–5), exposed via `pairs(kind)`. `Match` has no `kind`; `get(&match.entry)` gives the `Entry`.
- Negatives: `Negative` has no scope field (frozen) → veto is global per `(heard_lc, canonical)`; `negative()` lowercases `heard_lc`.
- Load cost: 40k entries indexed at startup (two hash maps; the optional `fst` is not used) — ≈ 4 s debug, [INFERENCE] ≈ 0.2 s release.
  Tests share one `LazyLock` load.

## Tests (`mbx test -p voice dictionary::`, 8 new; whole crate 35 passed, 0 failed)
`packaged_loads_and_hashes` (row 3: record sha256 = embedded file, counts, `git hub`→GitHub, `k eight s`→k8s, pairs present, pairs not
replacements) · `match_thresholds` (row 4: 0.81 rejected new / accepted user, learned variant exact, negative vetoes and never creates,
self-expansion guard incl. possessive) · `possessive_and_casing` · `scope_filters` · `bucket_prefilter_matches_brute_force` (1 000
sampled terms as learned entries, exact + lev-1 typo vs full scan under the §5.4 candidate rule: 995 exact / 388 fuzzy hits, all equal) ·
`user_entries_round_trip` (put/upsert/delete/negative, reload from file, no `.tmp` left) · `lookup_timing` (1 000 lookups 8.5 ms debug,
limit 50 ms) · `prose_is_left_alone` (300 forms, 0 rewrites).

## Gates
- `BUILD_LABEL=153-A2 build-lock mbx check -p voice --tests` → `Finished dev profile … in 4m 32s` (first build), 0 warnings after docs.
- `build-lock mbx clippy -p voice --tests` → `Finished dev profile [unoptimized + debuginfo] target(s) in 11.46s`, no warnings.
- `build-lock mbx test -p voice --no-run` then `MBX_SCHEDULER_TESTS=0 mbx test -p voice` → `test result: ok. 35 passed; 0 failed`.
- `cargo fmt -p voice` before each commit.

## Hand-offs
- **A1 (shaper, §4.2 stage 7):** per 1–3-token window, longest first: `dict.lookup(window)` (or `lookup_scoped(window, Some(surface),
  app_id)` for Surface/App-scoped user terms) → `Match { entry, canonical, score, tokens }`; splice `canonical` (possessive and
  sentence capital already applied), record the `Replaced` span, skip windows inside spans this stage produced. Symbols / formatters /
  TLDs / extensions / number words: `dict.pairs(Kind::Symbol | Formatter | Path | …)` → `Entry { canonical, spoken, tags }`.
- **A3 (learn → Add):** `dict.put(Entry { id: EntryId(String::new()) /* assigned */, canonical: typed, kind: Kind::Term, spoken: vec![],
  variants: vec![Variant { heard, metaphone: (String::new(), String::new()) /* filled */, seen: 1, last_ms }], scope, source:
  Source::Learned, tags: vec![] })`; `Never` → `dict.negative(Negative { heard_lc: heard, canonical })`; `NotNow` is A3's cooldown only.
- **A4 (actor + RPC):** `Dictionary::load(&rt.data_dir)` once at startup (holds the user file path; `put`/`delete`/`negative` persist and
  re-index); `snapshot()` per dictation (shares the packaged `Arc`, clones the small user layer); `dictionary_list(scope)` → `list`,
  `dictionary_put` → `put`, `dictionary_delete` → `delete`; `voice_status.dictionary` → `packaged_version()` + `counts()`.
- Rebuilding the data: `python3 crates/voice/build-dictionary/build.py` (network; `--offline` reuses `cache/`), then commit `data/*`;
  the loader fails closed on a sha256/size mismatch.
