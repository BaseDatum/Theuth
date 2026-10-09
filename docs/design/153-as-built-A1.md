# 153 as-built — A1 shape (`voice::shape`, §4 + §7 parser)

## Files
`crates/voice/src/shape/{mod.rs, tokens.rs, punctuation.rs, literals.rs, numbers.rs, dictionary_stage.rs, end.rs, tests.rs}`.
Nothing outside `shape/` touched. Frozen §14.1 public shapes unchanged (`ShapeConfig`, `Shaped`, `EndCommand`, `Action`, `Replaced`,
`RuleId`, `EndPhrases`, `Preset`, `ShapePresets`, `Shaper::new/shape`, `EndParser::new/feed/reset`).

## Design vs built
- **Token list** (`tokens.rs`): `Tok { text, kind: Word|Sym|Act|Hidden, asr, glue_left, glue_right, literal, spelled, replaced }`. The
  spacing class is the pair of glue flags; `render` joins with one space unless a side glues, collects `Action`s and `Replaced` byte ranges.
  Tokeniser peels the model's edge marks (`hello,` → `hello` + asr `,`); interior marks stay (`I'll`, `2.5`).
- **Stage order** as §4.2: literal escape → spoken phrases/actions → spelled letters → gates → literals → numbers → clean-up →
  dictionary → end command → terminal spacing. Each `cfg` switch skips its stage; `Preset::Off` returns the raw text re-joined.
- **Gates**: "dot"/"point" need left alnum and right ∈ TLD | extension | spelled letters | (dot) number. "point" between numbers is the
  numbers stage's decimal. "slash" needs path context (`~ . ..`, contains `/`, chain after a converted `/` or `.`, "dot slash" → `./`);
  `/command` is stage 4 only (so "please slash commit" stays prose, "run slash commit" → `run /commit`). "dash dash" → `--` + flag join;
  "dash" between alnums → tight `-`; "dash" with no left word → prefix `-` (`-V`). Operators convert only between number/single-letter/
  code-looking operands ≤ 12 chars — plain prose words are not operands (row 3).
- **Partial vs final** (W-a): a gate that reads the right token requires one more token after it on a partial (the last word may still
  be growing); on the final the right token suffices.
- **Literal** hides the escape word (kept as `Hidden` so the end parser sees it) and marks the following phrase (end phrase, symbol
  phrase, action or one word) untouchable.
- **Numbers**: cardinals ≤ 6 digits with well-formedness checks ("eighteen hundred", "one million" stay), digit strings ("one five
  three" → `153`), "point" decimals, `%`, ordinals only after "number", a lone "one" stays unless beside a symbol/"point" (pronoun guard).
- **Clean-up**: a model mark beside a converted symbol, or right after an action, is dropped; a model period *before* an action stays
  (`done.\n\n`).
- **Dictionary**: 1–3-word windows longest first through `pub(crate) trait Lookup` (`Dictionary` implements it by delegating to
  `Dictionary::lookup`); words fused to a joiner symbol are skipped (`parakeet.rs` is not rewritten); a match whose canonical equals the
  window is a completed occurrence (row 30's guard); rewritten tokens are not rescanned in the pass. `Replaced { start, end, heard,
  rule: RuleId(entry.id) }` per rewrite.
- **End commands** (`end.rs`): `EndParser` as §7.1 — exact phrase at the tail arms (partial) or fires (final); once armed, up to two
  re-decoded words may follow the phrase; sticky on partials; final near miss within `max(1, len/5)` for phrases ≥ 5 chars (`strsim::
  levenshtein`); "literal" before the phrase disarms; `consumed()` tells the shaper how many trailing words to drop (phrase + what
  follows, then a trailing model mark). `pub const SETTLE_MS = 1500`, `SILENCE_MS = 350`, `NEAR_MISS_MIN_LEN = 5` (re-exported from
  `shape`). The parser is **owned by the actor**, one per utterance: `Shaper::shape_with(&mut parser, prev, raw, is_final)`;
  `Shaper::shape` is the stateless form (fresh parser → exact phrases only).
- **Terminal spacing**: on `Surface::Tui` or `AppProfile.terminal`, a final ending in `/cmd` or `@name` has trailing whitespace trimmed.
  `Shaped.text` never carries the splice separator — that is the client's (`crates/tui/src/dictation.rs::trail`), see hand-offs.
- **Sentence case**: after "new line" and after a `prev` ending in `\n` (row 21). Not after "new paragraph" — row 22 is written lowercase
  and the table is the fixture; flagging it (one `==` in `tokens::render` flips it).

## Additions (private items / additive methods, no frozen signature changed)
`Shaper` gained private fields `surface, commands, mentions, tlds, exts` and builders `with_surface`, `with_commands`, `with_mentions`;
`shape_with`. `Ctx` (pub(crate)) is what the stages read; tests build it with a fake `Lookup`. Built-in TLD/extension lists in `mod.rs`
plus whatever `Dictionary::list(None)` returns tagged `tld` / `ext` / `mention` (user + learned layers only — the stub exposes no packaged
tag lists).

## Tests (crates/voice/src/shape/tests.rs, end.rs) — 24 under `shape::` (18 in `tests.rs`, 6 `end::tests::*`)
`examples_table_rows_1_to_35` (table-driven, names the failing row) + the §4.7 list + `shaper_builders_reach_the_stages` (public
`Shaper` API with the stub `Dictionary`). Fuzz: 10 000 seeded xorshift strings (pool of trigger words, marks, combining marks, emoji,
random code points) through partial/final with a live parser; asserts `Replaced` offsets are char boundaries.

## Gates (2026-10-09)
- `BUILD_LABEL=153-A1 build-lock mbx check -p voice --tests` → clean, 0 warnings.
- `BUILD_LABEL=153-A1 build-lock mbx test -p voice --no-run` then `MBX_SCHEDULER_TESTS=0 mbx test -p voice` →
  `test result: ok. 50 passed; 0 failed; 0 ignored` (24 shape::/end:: + 26 pre-existing); `shape::` filter → `24 passed`.
- `BUILD_LABEL=153-A1 build-lock mbx clippy -p voice --tests` → `Finished dev profile`, 0 warnings.

## Hand-offs
- **A4 (actor)**: per dictation `Shaper::new(cfg, &phrases, &dict, app).with_surface(opts.surface).with_commands(opts.commands)
  .with_mentions(agent names)`; per utterance one `EndParser::new(&phrases)` (or `reset()` after each final); per piece
  `shaper.shape_with(&mut parser, &prev_finals_joined, &t.text, t.is_final)`; `prev` matters only for its trailing `\n`. `SETTLE_MS` /
  `SILENCE_MS` from `voice::shape`. `Send` on a partial = armed (start the settle); on the final = fire. The TUI should skip its
  `trail()` separator when `shaped.text` ends in `/cmd` or `@name` on a final (row 31's "no trailing space" is the splice's call).
- **A2 (dictionary)**: `Dictionary::lookup` must return `None` (or `canonical == heard`) for a window already equal to a canonical;
  expose the packaged TLD/extension/mention/path lists (e.g. `tagged(tag)`) and A1 drops `TLDS`/`EXTS` in `mod.rs`.
- **Integrator**: `Lookup` trait is `pub(crate)`; row 29/30 tests run against a fake, not A2's matcher.
