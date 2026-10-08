# 144 as built — Z (CLOSER: integrate wave 1, gate, live proof)

Closer 144-Z, branch `integrate/144`. Brief: `144-briefs/144-Z.md`; hand-offs: `144-briefs/144-HANDOFFS.md`.

## Integration order and conflicts
`integrate/144` = contract C `0212b28d` → W4 `097d2c14` → W3 `00e64e69` → W1 `dac1e5c3` → W2 `6603f730` → `origin/main
5cddad8e` folded → `ce9fc5d8` (the orchestrator; every merge conflict-free). Z's commits follow from there.

## What landed (file:line)

### Cross-worker hand-offs applied (commit `0e04f5d3`)
- W2 → `crates/document/src/lib.rs:261-267` spawns `index::reindex` after the ingest actor starts (first run walks the
  five indexable kinds; a model change rebuilds vectors). W2's second hunk (the hook beside each of the 8
  `rt.publish(DocumentChanged::of(…))` sites in `actor.rs`/`notebook.rs`) is replaced by W2's own alternative: one
  `rt.subscribe::<DocumentActor, DocumentChanged>(&actor)` (`lib.rs:308-309`) and `impl Message<DocumentChanged> for
  DocumentActor` (`index/mod.rs:190-198`) delegating to the `Changed` handler — every publish site, present and future,
  reaches the index. W1's ingest job keeps its direct `on_document_changed` call (it publishes no `DocumentChanged`).
- W3 → `notebook: None,` in the four test literals: `crates/slack/src/ui.rs:798`, `crates/telegram/src/commands.rs:1460`,
  `crates/tui/src/attach.rs:274`, `crates/tui/src/model.rs:17563`.
- W3 → `crates/agent/src/attach.rs:33-35`: the `notebook` prompt row now reads *its items are already in the `<notebook>`
  block at the levels the user chose; `notebook_get(<id>)` for the item list; …* (design §5.3).
- W2 → `crates/decision/src/candle/lora.rs:672` `const { assert!(!TRAIN_BUILD) }` so `clippy -p decision --tests` passes.

### Bindings and the desktop seam (commit `ea3bc906`)
- `just ts-desktop` regenerated `bindings/` (8 export tests) and swapped `crates/desktop/ui/src/rpc/bindings` wholesale
  (`methods.json` 399 names incl. the 19 wave-1 methods; `NoteSearchHit.{block,heading,href}`, `SessionSummary.notebook`,
  `CreateSessionOptions.notebook`, `SavedAnswer`).
- `rpc/api.ts:142-153,378-415,1022-1338`: the 19 wave-1 methods on `Api`/`METHODS` with C's signatures
  (`notebook_get → NotebookDetail`, `notebook_update → null`, `notebook_delete_preview → DeletePreview`,
  `notebook_item_remove → ItemRemoved`, `source_add(args: SourceAddArgs)`, `nb_search(args)`, `save_answer_as_note →
  SavedAnswer`). `api.test.ts` (METHODS = methods.json) passes.
- `lib/notebooks-rpc.ts` shrank to `NotebooksApi = Pick<Api, …19>`, `notebooksApi(client) = makeApi(client)` (the `/d`
  guest), `useNotebooksApi() = useApi()`, `notebookKeys`. Deleted: `lib/notebooks-types.ts` (W4's mirror) and
  `dev/notebooks-fixture.ts` (the pre-engine screenshot fixture — the engine answers now); 15 importers point at
  `@/rpc/bindings/*` with `Source → NotebookSource` (`origin.ts`, `SourceCard.tsx`, `SourcePage.tsx`, `useNotebook.ts`,
  three tests). `ContextItem` was never imported by W4 (no rename needed).
- Call sites aligned: `useNotebook` unwraps `.notebook` from `NotebookDetail`; `useDescription` reads `doc_markdown`
  directly; `NotebooksList` reads `sources_exclusive`; `AddSourceMenu` composes one `SourceAddArgs`
  (`{ notebook, path|url|text|meeting|session|mcp, title }`); `MessageAssistant` toasts “Saved as a note” (`SavedAnswer`
  has no title); `AttachChip.useOpenRef` `notebook`/`source` are `case`s; the `'notebook' as Kind` / `'source' as Kind`
  casts in `ChatColumn.tsx`/`SourceCard.tsx` are gone.
- Search rows for the two new categories (C's exhaustiveness arms filled): `lib/attach.ts` `Source` gains
  `{ kind: 'notebook' | 'source'; hit: BlockHit }` (title/subtitle/idOf); `lib/search.ts` `SearchDestination` gains
  `sources`/`notebooks` `{ id, block }` and `notes` carries `block`; `sidebar/SearchDialog.tsx` `BlockRow` + `items()` +
  `go()` (`/sources/$id#<block>`, `/notebooks/$id`, notes `/notes/$id#<block>`); `chat/AttachPicker.tsx` rows.
- `notebook: null` on the `SessionSummary`/`CreateSessionOptions` literals the regenerated bindings made required
  (`Composer.tsx:89`, five tests, `hud/scene/fixtures.ts`); `nav/SectionBand.test.ts` counts six Library tabs.
- `crates/tui/src/model.rs:9878-9926` local `Act::Send(Box<Command>)`: `Command` crossed clippy's
  `large_enum_variant` threshold once `CreateSessionOptions` grew `notebook`.

## Proof

## Gates
- `just ts-desktop` (BUILD_SLOTS=1 + mkdir lock): export 8 passed. First attempt died with ENOSPC (root disk 100%);
  freed by deleting this target's own `debug/incremental` (29 GB), retried clean.
- `bun run check` (tsc): 0 errors.
- `bunx vitest run` (full): 209 files, 1416 tests — 1415 passed + `SectionBand.test.ts` (5-tab pin) fixed → all green.
- `bunx vite build`: built in 7.6 s (chunk-size warnings pre-existing).
- `cargo fmt --check`: the 144 crates clean after formatting `crates/document/tests/ingest_audio.rs`; the remaining
  diffs (`app`, `mesh`, `sandbox`, `signal`, `slack`, `talk`, `tasks`, `telegram`, `theuth`, `tui/src/lib.rs`) are
  pre-existing on `origin/main` and untouched here.
- `cargo clippy -p document -p decision -p agent -p rpc -p search -p surface -p voice -p tui --tests -- -D warnings`:
  clean (after the `Act::Send` box above).
- `cargo test -p document -p decision -p agent -p rpc -p search -p surface -p voice`: agent 522 (+2 ignored
  pre-existing) + 1 + 5; decision 93 + 2 + 3 + 3 + 4; document 71 lib + actor 11 + cite_check_links 4 + index_search 3
  + ingest_audio 1 + ingest_job 5 + replicate 1; rpc 54 + 1; search 6; surface 93; voice 25 — 0 failed.

## Deviations

## Hand-offs
