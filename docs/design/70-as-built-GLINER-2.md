# 70 as built — GLINER-2: GLiNER2.5-Decide preprocessing in Rust + golden token fixtures

Branch `feat/gliner-gliner` off `95f6496a`. Brief: `/tmp/69/brief-GLINER-2.md`; report with the full
gliner2 call path, marker table and head rule: `/tmp/69/report-GLINER-2.md`. Reference: PyPI `gliner2==2.0.0`
(= fastino-ai/GLiNER2 @ 55656fb), model `fastino/GLiNER2.5-Decide` @ `5a7adf72` — tokenizer only
(`tokenizer.json` 8,333,952 B, `tokenizer_config.json`, `config.json` under
`~/.theuth/models/decision-gliner/`), no weights. Second checkpoint for 28 v1's guard; 57 §2 crate layout.

## What changed

- **`crates/decision/src/gliner/mod.rs`** — `pub mod prompt;` and a two-line doc, nothing else (GLINER-1
  owns anything further in `mod.rs`).
- **`crates/decision/src/gliner/prompt.rs`** — the port of `gliner2/processor.py`
  `_collate_batch` → `_transform_record` → `_format_input_with_mapping` for one classification task:
  - `PAD SEP_STRUCT SEP_TEXT P C E R L EXAMPLE OUTPUT DESCRIPTION` consts (`:28-49`) and
    `GlinerSpecial` (`:54-78`) with `DECIDE` (`:82-94`) and `from_tokenizer` (`:98-123`): every marker
    looked up by string, any other id is an `Err`.
  - `GlinerConfig { max_len: 512, cut_ok: true }` (`:126-141`); `GlinerRow` (`:144-165`) with laya's
    three flags.
  - `labels(q)` (`:168-184`): Choice → candidate `text` (id when empty), Bool → `["yes","no"]`, Score →
    levels. `task_and_state(q)` (`:187-202`): instructions / statement, state.
  - `words(state)` (`:216-228`): gliner2's terminal `.` rule + `WhitespaceTokenSplitter._PATTERN` as a
    `regex` `LazyLock` (`:207-212`, `(?i)`) + per-word `to_lowercase()`.
  - `build_row(tok, q, cfg)` (`:238-298`): `( [P] task ( [L] l₀ [L] l₁ … ) ) [SEP_TEXT] words…`, every
    piece `Tokenizer::encode(piece, false)`, `label_pos` = position of each `[L]`. Errors: no labels;
    schema past `max_len` (`truncated: the schema …` — a label could not be scored, always hard); state
    cut with `cut_ok == false` (`truncated: the state …`). Otherwise the state is right-truncated at
    subword granularity and `state_cut`/`truncated` say so.
  - Tests (`:300-460`): `marker_table_matches_python`, `labels_match_python_mapping`,
    `word_split_is_pythons` (no tokenizer needed); `golden_rows_byte_for_byte` and
    `schema_past_budget_is_an_error` load `$THEUTH_MODELS_DIR/decision-gliner/tokenizer.json` or
    `~/.theuth/models/decision-gliner/tokenizer.json` and print `SKIP: … missing` when absent.
- **`crates/decision/src/gliner/fixtures/golden.json`** — 13 cases from the Python reference: question,
  labels, `input_ids`, `attention_mask`, `label_pos`, decoded string, plus the marker table.
- **`scripts/tests/gliner-golden.py`** — the generator (`uv run`, inline metadata `gliner2[local]==2.0.0`):
  builds each case through `SchemaTransformer._collate_batch` exactly as `classify_text` does, asserts the
  per-piece `tokenizers` encode equals gliner2's `tokenize→convert_tokens_to_ids`, writes the JSON;
  `--check` regenerates in memory and diffs (exit 1 on drift). Downloads the three tokenizer files at the
  pinned revision when missing.
- **`crates/decision/Cargo.toml`** — `regex = { workspace = true }`. **`lib.rs`** — `pub mod gliner;`.

## Facts the port rests on (gliner2 2.0.0 wheel, file:line)

- Layout and per-piece tokenisation: processor.py:1247-1344; schema tokens :1096-1131; marker slots
  `offset+1`, `offset+4,+6,…` :1267-1275. Terminal `.`: :521-525. Word split + lower: :571 and
  processing/word_splitter.py:25-42. No `[CLS]`/`[SEP]`, no subword truncation anywhere (`max_len` is a
  word count, only set by chunking: runtime.py:109-114, 1253).
- Head: runtime.py:382-424 — `embs[1:]` (drop `[P]`) → `classifier` MLP → one logit per `[L]`;
  `multi_label=False` → softmax; `multi_label=True` → sigmoid ≥ `cls_threshold`.
- `[SEP_STRUCT]` separates several tasks over **one** text; questions with their own `state` are separate
  padded rows (`_pad_batch` :612-704, `[PAD]`=0). Single-question rows kept.
- `none`: `engine::with_none` (engine.rs:570-592) appends `Candidate { id: "none", text: "none of these" }`
  when the gate has `none`; GLiNER sees the text only → label `none of these`, last `[L]`.
- Encoder (`encoder_config/config.json`): deberta-v2, `max_position_embeddings` 512, relative attention,
  `position_biased_input: false`, `vocab_size` 128011.

## How verified

- `cargo test -p decision gliner -- --nocapture` (tokenizer present): **5 passed, 0 failed** — `golden: 13 cases
  reproduced` (input_ids, attention_mask, label_pos byte-for-byte; the 923-token `long_state` case also cut to
  512 with `state_cut`, and `Err("truncated: the state …")` with `cut_ok: false`).
- Same with `THEUTH_MODELS_DIR=/nonexistent`: **5 passed**, two `SKIP: /nonexistent/decision-gliner/tokenizer.json
  missing — run scripts/tests/gliner-golden.py once` lines (CI without the file stays green).
- `cargo check -p decision --tests`: 0 warnings, 0 errors.
- `cargo test -p decision`: 2 suites — lib **69 passed, 0 failed**; `tests/model.rs` **3 passed, 0 failed**
  (+ doc-tests 0).
- `cargo check --workspace --tests`: `Finished`; first attempt failed in `nodes` on the gitignored
  `bindings/methods.json` the fresh worktree lacked — generated with `just ts`
  (`cargo test -p rpc --features ts export_bindings`), then clean except cargo's future-incompat note for `block v0.1.6`.
- `uv run scripts/tests/gliner-golden.py --check`: `ok: 13 cases match crates/decision/src/gliner/fixtures/golden.json`,
  exit 0 (inline-metadata env resolved by uv; ran twice).

## Deviations / ceilings

- `GlinerRow.markers_lost` is always `false` on a returned row — the brief makes a lost marker a hard
  error, so `build_row` returns `Err` instead of a row; the field stays for the laya-shaped consumer.
- The word splitter's `\w` is Rust `regex`'s, not Python `re`'s: marks (Mn/Mc) and connector punctuation
  other than `_` are word characters here and not there, Nl/No digits the reverse. Every golden
  (CJK, `İ`→`i̇`, `ß`, `—`, `…`→`[UNK]`) agrees; `ponytail:` comment names the swap if one ever differs.
- State truncation is laya's (subwords, right), not gliner2's (words, only when asked).
- `uv run scripts/tests/gliner-golden.py` pulls torch on first run (inline metadata); the venv used here
  was `/tmp/69/gliner-venv`.
