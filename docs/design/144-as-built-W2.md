# 144 as built — W2 (INDEX: block-level FTS5 + sqlite-vec, local ModernBERT embeddings, hybrid `nb_search`)

Worker 144-W2, branch `feat/144-w2`, cut from the contract skeleton `13bd3cc3`. Spec: `144-notebooks.md` §7 (all),
§13.2 `nb_search`, §14 `notebooks.embedding_model` / `chunk_*`, §16.

## What landed (file:line)

### Embeddings (`crates/decision`)
- `crates/decision/src/candle/embed.rs` (new): `Embedder::open(dir, Gpu)` — `config.json` through `EncoderConfig::from_value`
  (`model_type == "modernbert"`), `tokenizer.json`, `model.safetensors` mmaped into the vendored `ModernBert` **without** the
  decision head (a `model.`-prefixed save loads through `vb.pp("model")`, a bare `ModernBertModel` save as is); f16 on Metal /
  f32 on CPU (`Gpu::dtype`, CPU-f32 fallback with a WARN like `Loaded::open`). `embed(texts)`: `[CLS]`/`[SEP]` from the
  tokenizer's post-processor, truncation at `MAX_TOKENS = 512`, manual pad + u8 mask, `forward` → `hidden.i((.., 0))` →
  f32 → L2-normalise, `BATCH = 32` texts per forward. `count_tokens`. `write_tiny_checkpoint(dir)` (`#[doc(hidden)]`): the
  test double — 2 layers, 64 dims, 2 heads, whitespace `WordLevel` tokenizer over `TINY_WORDS` with a `TemplateProcessing`
  post-processor, random weights through `VarMap` → `model.safetensors`; same three files, same code path, no download.
- `crates/decision/src/embed.rs` (new): `EMBED_ACTOR = "embed"`, `DEFAULT_MODEL = "embed-gte-modernbert-base"`,
  `EmbedSection { embedding_model }` (the one `notebooks` key this crate reads), `check_name` / `model_dir(data_dir, name)`
  (D9: a `/`, `\`, empty, `.`/`..` is refused naming the rule), `installed(dir)`. `EmbedActor::new(dir, gpu)`: messages
  `Info → Option<EmbedInfo { model, dims }>` (reads `config.json`, never the weights), `Tokens { texts } → Result<Vec<usize>,
  String>` (tokenizer only, cached), `Embed { texts } → Result<Vec<Vec<f32>>, String>` (lazy load in `spawn_blocking`,
  `touch`/`arm`/`Unload(epoch)` idle watchdog copied from `Engine`, `IDLE_UNLOAD` 180 s, `with_idle` for tests), `IsLoaded`.
  A missing directory: `Info` = `None`, `Embed`/`Tokens` = `Err("… not installed …")`, **one WARN** per actor.
- `crates/decision/src/lib.rs:708-717`: the plugin registers `EmbedActor` as `"embed"` beside the engines (even with no
  checkpoint installed, so the index learns `None` from `Info`), on `decision.device` → `Gpu` (`Auto` = Metal on a Mac); a bad
  `notebooks.embedding_model` is an `error!` naming the rule and no actor. `candle/mod.rs:7` `pub mod embed`.

### The index (`crates/document/src/index/**`)
- `index/db.rs` (new): the §7.1 tables in `documents/index.sqlite` on the index's **own sqlx pool** (`vec0` is a
  process-global auto-extension — `register_vec()` = `sqlite3_auto_extension(sqlite3_vec_init)` once — that only reaches
  connections opened after it; the store's pool is already open, so the index opens its own two connections, WAL,
  5 s busy). `nb_blocks` FTS5 (`porter unicode61 remove_diacritics 2`), `nb_index_state`, `nb_vec` `vec0(rowid, embedding
  float[<dims>], doc TEXT, block TEXT, chunk INTEGER, sha TEXT)` created at the installed model's dims; other dims on disk
  → `DROP` + recreate and `nb_index_state.model = NULL` (every vector stale, FTS5 rows untouched). `replace(doc, rows,
  vectors, state)` one transaction; `delete`; `lexical(fts, docs?, kinds?, k)` = `MATCH` + `bm25(nb_blocks, 0,0,0,0, 3.0,
  1.0)` + `snippet()` of text and heading with `\u{1}`/`\u{2}` markers; `nearest(blob, docs?, k)` = `embedding MATCH ? AND k
  = ?` (+ `doc IN (…)` metadata filter); `fts_query` quotes every word and ORs them. One `Index` per root per process.
- `index/chunk.rs` (new): `Block { id, heading: Option<level>, text }`, `Params { max_tokens, overlap, min_tokens }`,
  `chunk(blocks, tokens, params) -> Vec<Chunk { first, last, text, tokens }>` — a heading starts a chunk and is prepended to
  every chunk under it, blocks append until the budget, the next chunk starts with the previous one's tail blocks within
  `overlap × max_tokens` (never the heading), chunks under `min_tokens` dropped; `kinds(blocks, doc_kind)` marks a source's
  `## Insights` subtree `insight`; `estimate_tokens` = `chars / 4`. Unit tests: boundaries, overlap, minimum, insight kinds.
- `index/mod.rs`: `IndexConfig` (section `notebooks`: `embedding_model`, `chunk_tokens` 400, `chunk_overlap` 0.15,
  `chunk_min_tokens` 5; `#[serde(default)]`, unknown keys ignored so W1/W3 read theirs from the same section);
  `INDEXABLE = [Note, Board, MeetingNote, Source, Notebook]`. Entry points keep the skeleton signatures: `nb_search(actor,
  args)` → `ask(NbSearch)`, `reindex(actor)` → `ask(Reindex)`, `on_document_changed(ev)` → registry lookup + `tell(Changed)`.
  Messages on `DocumentActor` (handled here, off the mailbox): `Changed`, `NbSearch`, `Reindex`, `SearchSources`,
  `SearchNotebooks` (now `DelegatedReply`). `DocumentActor::indexer(me)` builds the stateless `Indexer { actor, store, cfg }`.
  - `Indexer::index_doc`: `read_meta` (gone → drop rows) → `Info` → `Index::open(root, dims)` → blocks = `Outline { depth: 1 }`
    ids/types + `Markdown { annotate: true }` text joined on `<!-- id -->` / `<!-- id by name -->` (`assemble`) →
    `content_sha` over title + block shas; unchanged text **and** same model → return. Rows: one `block = ''` title row
    (so title hits survive the move off nucleo) + one per top-level block with the nearest heading, kind, `text_sha`, tokens
    (`Tokens` from the model's tokenizer, else `chars/4`). Vectors: `chunk()`; a chunk whose text sha already has a vector
    **from the same model** keeps it, the rest go through `Embed` 32 texts per message; embed failure → the row set is written
    lexical-only with `model = NULL`. All writes under one global `tokio::sync::Mutex` so a twice-changed document never
    ends on the older text.
  - `Indexer::reindex` (§7.1 first run / §7.5): `store.list` of the five kinds newest first, rows of documents no longer live
    dropped, every stale document (text or model) re-indexed.
  - `Indexer::search` (§7.4): scope = `docs` ∪ live `note`/`source` items of `notebooks` ∪ the notebooks themselves; FTS5
    top-`4×limit` ∪ vec0 top-`4×limit` (`16×` when a kinds filter is applied after the join), vectors counted only for
    documents whose `nb_index_state.model` is the current one (§7.5 per-document fallback), RRF `Σ 1/(60+rank)`, de-dup by
    `(doc, block)`, sorted score desc then doc/block, `limit + 1` rows. `BlockHit`: title + live notebooks per document
    (`notebooks_of(Note|Source)`, a notebook document lists itself), `match.field` = `title` (title row) / `text` /
    `heading` with scalar ranges from the snippet markers, vector-only hits = first 160 chars, `href` = `TheuthAnchor`
    (`sources/` for `DocKind::Source`, else `notes/`; `#block?s=<sha8>` unless the title row).
- `crates/document/src/search.rs` (rewritten): the per-query `note.md` read is gone. `NoteSearchHit { doc, match, block,
  heading, href }`; `SearchNotes` → `Indexer::search` over kinds `note | board`; blank → ready empty; `limit + 1` → `has_more`.
- `crates/rpc/src/search.rs:429-437, 646-650`: `search()` prompt and tool description name the new identities (note/source
  document id + block as the `theuth://` href, notebook id); `:893-906` the note line carries `#block (href)`.
- `Cargo.toml:204-207` `sqlite-vec = "0.1"` (0.1.9, `sqlite3_vec_init` static, compiled with `SQLITE_CORE` against the
  vendored libsqlite3-sys 0.36); `crates/document/Cargo.toml:31-36` `sqlite-vec`, `libsqlite3-sys 0.36` (the workspace
  patch), `decision`, `parking_lot`.

### Tests
- `crates/decision/src/candle/embed.rs` tests: `tiny_checkpoint_embeds_unit_vectors_in_order`.
- `crates/decision/tests/embed_actor.rs`: `tiny_checkpoint_round_trip_and_idle_unload` (Info without weights, Tokens, 33
  texts across the batch boundary, unit norm, same text same vector, idle release; prints ms per 400-token chunk),
  `missing_directory_is_lexical_only_not_a_crash`, `model_name_rule`.
- `crates/document/src/index/{chunk,db,mod}.rs` tests: chunk boundaries/overlap/minimum, insight kinds, `fts_query`,
  `vec0` KNN + dims change, `assemble`, `strip_marks`.
- `crates/document/tests/index_search.rs` (helper-gated like `tests/actor.rs`): `note_blocks_index_and_hybrid_search_hits_the_block`
  (round trip with the tiny model: block id, heading, snippet ranges, href, fusion puts the lexical∪vector block first, scopes,
  state model), `notes_category_pages_title_and_block_hits_and_skips_deleted` (FTS5-only: title row, body block, deleted
  dropped via `on_document_changed`, `has_more`, `model = NULL`), `model_change_rebuilds_vectors_and_edits_reindex_blocks`
  (no model → vectors appear once installed → a different name re-embeds with FTS5 untouched → edit + `DocumentChanged`
  finds the new text, not the old).

## Proof

(gates below; timings in Gates)

## Gates

(filled after the runs)

## Deviations

- `nb_vec` carries a fourth metadata column `sha TEXT` (the chunk text's sha256) so "re-embed only changed chunks" has
  something to compare — §7.1's DDL has no chunk identity. Its dims follow the installed model (`float[<hidden_size>]`,
  768 for gte) instead of the literal `float[768]`, which is what lets the 64-dim tiny checkpoint prove the pipeline.
- The index opens its own sqlx pool on `documents/index.sqlite` (same file, §7.1) rather than `Store::pool()`: `vec0` is
  registered through `sqlite3_auto_extension`, which only applies to connections opened afterwards, and `Store::open`
  (C's) runs first. Not replicated: the tables are not in `store::TABLES`.
- `NoteSearchHit` keeps its name and `doc: DocSummary` (rpc/tui/desktop read `doc.title`/`updated_ms`) and **gains**
  `block`, `heading`, `href`; `match.field` is `title | heading | text` (was `title | body`). Title hits are a `block = ''`
  row in `nb_blocks` so `search("Groceries")` still finds the note by name without a file read.
- Insight sections over 400 tokens are chunked normally (several vectors), not mean-pooled into one — the insight job is
  wave 2 and the mean adds a code path nothing exercises yet.
- Notebook documents cite as `theuth://notes/<id>` (`TheuthKind` has no `notebooks`; a notebook is a document).
- Config: this module reads `IndexConfig` from `notebooks` (not a crate-wide `NotebooksConfig`), so W1's section struct can
  live beside it without a name clash; serde ignores unknown keys on both sides.
- `fts_query` ORs the words (recall; bm25 still ranks rows holding more of them first, the vector side carries meaning) —
  10 §3.3's knowledge search ANDs; different job.

## Hand-offs

- **C / closer — `crates/document/src/lib.rs` plugin start** (two hunks, after `actor.register(DOCUMENT_ACTOR)?`):
  ```rust
  // 144 §7.1: the block index — first run / model change in the background.
  let a = actor.clone();
  tokio::spawn(async move {
      if let Err(e) = index::reindex(&a).await {
          tracing::warn!("index: rebuild failed: {e:#}");
      }
  });
  ```
- **C / W1 — `crates/document/src/actor.rs`**: at every `self.rt.publish(DocumentChanged::of(&meta, …)).await` site
  (`:774, :1283, :1798, :1864, :2063, :2235, :2315`) call the hook first:
  ```rust
  let ev = DocumentChanged::of(&meta, deleted);
  crate::index::on_document_changed(&ev);
  self.rt.publish(ev).await;
  ```
  (the hook is a registry lookup + `tell`; nothing awaits the index). Alternatively one `rt.subscribe::<DocumentActor,
  DocumentChanged>(&actor)` in `lib.rs` with `impl Message<DocumentChanged>` forwarding to `Changed` — say which and I add it.
- **W4 — desktop**: `NoteSearchHit` now has `block` (empty = title hit), `heading`, `href`; the notes row opens the editor at
  `block` when set (59 §15); `match.field` values `title | heading | text`. `BlockHit` for the `sources`/`notebooks` groups
  unchanged from the skeleton. Bindings regenerate with `just ts-desktop` (closer).
- **Closer — the real checkpoint**: `<data_dir>/models/embed-gte-modernbert-base/{config.json, model.safetensors,
  tokenizer.json}` from `Alibaba-NLP/gte-modernbert-base`; nothing downloads it. Verify the safetensors key prefix on first
  load (`embed: … loaded (768 dims)` in the log; a load error names the missing tensor).
