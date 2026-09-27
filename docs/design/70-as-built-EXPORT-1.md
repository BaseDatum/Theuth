# EXPORT-1 as built — notes as Markdown / PDF, boards as PDF through the Typst engine

Branch `feat/export-notes-boards`, worktree `theuth-export`, base `1af0df84`. Design: `72-export.md`.

## Commits

### `a634913b` surface: print lowering for json-render fences

- `crates/surface/src/export.rs` (new, `pub mod export`): `print_markdown(text, images)`, `heading_count(text)`, `lower(spec)`. Fences are located with the crate's own `fences()` scanner (byte ranges), so replacement is in place and everything else is byte-identical, like `flatten`. The block id comes from the `<!-- id [by …] -->` annotation the document actor writes before each block (`Markdown { annotate: true }`); annotations are stripped from the output. `code:` links flatten in prose only, never inside fences.
- Lowering table in 72 §3. Containers (Card / Stack / Grid / Tabs / Accordion) recurse so a Card holding a Table keeps its table; depth capped at `MAX_DEPTH`.
- Tests (`export::tests`): one per fence family over the crate's fixtures, every `valid-*`/`graph-*` fixture lowers to something, `print_markdown` swaps an asset in with its caption, falls back to text, strips annotations, flattens `code:` links, leaves Mermaid alone; `heading_count` ignores headings inside fences.

### `ec761d65` export: doc_export RPC + model tool, Newsreader, desktop Export menu

- `crates/artifact/src/export.rs` (new): `ExportFormat`, `ExportAsset`, `ExportResult` (ts-rs exported, in `rpc/src/export.rs`'s list), `ExportDoc` message, `EXPORT_SESSION` (nil uuid), `State::export_doc`. Asset SVGs land in `<artifacts>/export/<uuid7>/<id>.svg` for the compile and are removed after. Front matter: `title` (JSON-quoted), `date`, `toc` when ≥ 3 headings, `font: Newsreader`. `dest` → `platform::files::write_private` (0600, atomic, parent created). No `dest` → `State::show_as(session, call_id, args, bind: false)`, the `show` pipeline with document binding off — a Markdown export must not become a second document. `record: Some((session, call_id))` lets the agent tool file the export under its own chat.
- `crates/artifact/src/store.rs`: `show` → `show_as(.., bind)`; `get()` lets `EXPORT_SESSION` records through the alive-session check (found in the manual check: the viewer said "owning session no longer exists").
- `crates/artifact/src/pdf.rs`: `NEWSREADER` embedded via `TypstEngine::builder().fonts(..)`; `Matter.font` → `meta.font`, default `Libertinus Serif` so `show` is unchanged. `typst/document.typ` reads `meta.font`. `pdf.md` documents the key.
- `crates/artifact/fonts/`: `Newsreader.ttf`, `Newsreader-Italic.ttf`, `OFL.txt`. Decompressed from `ui/public/fonts/*.woff2` (wawoff2); the `name` table rewritten from `Newsreader 16pt 16pt` to `Newsreader` (Typst matched nothing otherwise — the first compile warned `unknown font family: newsreader`).
- `crates/artifact/src/tools.rs`: `DocExportTool` (`doc_export`, `Risk::Mutating`, args `doc`, `format`, `dest`). Registered in `artifact`'s plugin start beside `show`, **not** in `document::tools::register` as the brief said: `artifact` depends on `document`, so the reverse edge would be a cycle. Same `doc_*` name family, same registry. Agent exports carry no assets (nothing renders SVG on the daemon) → text fallbacks; documented in the tool description and `board.md`'s closing paragraph.
- `crates/document/src/actor.rs`: `MetaOf { doc, by }` → `DocMeta`, admitting members like `OpenDoc`; re-exported.
- `crates/rpc/src/api.rs`: `doc_export(doc, format, dest, assets)` with `with_extensions`: `principal(ext)`, `MetaOf` (+ `member_outcome`), owner-only + home-dir + no-`..` guard on `dest`, `Markdown { annotate: format == Pdf }`, `ExportDoc` to the artifact actor. Bindings regenerated (`just ts-desktop`): `ExportAsset.ts`, `ExportFormat.ts`, `ExportResult.ts`, `methods.json`.
- `crates/desktop/ui/src/rpc/api.ts`: `doc_export` in `Api` and `METHODS`.
- `crates/desktop/ui/src/components/notes/ExportMenu.tsx` (new): the **Export** popup in the note bar (Download icon, tooltip) with **Markdown… / PDF…**; `save()` dialog on the local desktop daemon, else artifact + viewer. Toast with **Reveal** (`revealItemInDir`) — first `action:` toast in the UI. `NotePage.tsx` mounts it after Snapshots.
- `crates/desktop/ui/src/lib/doc-export.ts` (new): `mountedDiagrams(root)` — `.board-diagram[data-block]` frames, the first `<svg>` with a `viewBox` outside a `button` (the frame's Tour button is a lucide svg; the first cut picked it and exported an icon — caught in the manual check); `exportAssets(mounted, from, to)` — one-pass recolour over the ten colour tokens (no chaining when palettes cross), then `standalone()`. `lib/diagram-theme.ts`: `lightTokens()` lifts `.dark` off the root for one synchronous `getComputedStyle`.
- Unmounted fences (edit mode, a board still connecting) get no asset; the daemon lowers them to text. No off-screen rendering.
- `components/artifacts/List.tsx`: the nil session groups as **Exports** instead of "Unavailable session / Unknown chat".
- Tests: `export::pipeline_tests` (artifact) — fixture board `surface-fence-render.md` annotated like the actor does, with a real SVG asset → print source has the `![](…/f1.svg)` figure + caption, Excerpts as `rust` fences with the `// path#La-Lb` line, no `root:`, no annotations, `toc: true`; compiled to `dest` → `%PDF`, pages ≥ 1, no warnings, scratch dir empty, file 0600; PDF without `dest` → `Kind::Pdf` record under `EXPORT_SESSION` with preview and pages; Markdown round-trips byte-for-byte to a file and to a `Kind::Markdown` artifact. Unit tests for `file_stem`, `strip_front_matter`, `without_title_heading`. `lib/doc-export.test.ts` — mounted-svg selection, recolour, standalone, identity when palettes match.

### (this commit) fixes from the manual check + docs

- `without_title_heading`: a note's first block is `# <title>`; the PDF title block already shows it, so it is dropped from the body (it was also the first TOC entry).
- `mountedDiagrams` svg selection and `store::get` as above; **Exports** group label.
- `docs/design/72-export.md`, this file.

## Gates

`cargo check --workspace --tests` clean; `cargo test -p artifact -p surface -p document -p rpc` green (artifact 52, surface 49, document 30 + 11, rpc 44 + 7 …); `tsc --noEmit` 0; vitest 96 files / 565 tests + the new file (no-emoji lint and surface parity included).

## Manual check

Scratch engine `THEUTH_DATA_DIR=/tmp/69/export-data` (port 7791, `secrets.key_store: file` — the rebuilt binary otherwise blocks on a Keychain prompt). Screenshots in `/tmp/69/export-*.png`.

- RPC (`bun /tmp/69/export-rpc.ts`): note → `note.md` (verbatim `doc_markdown`, mode 600), `note.pdf` (2 pages), board → 4 pages, `dest` outside home refused (`not under /Users/Sky.Moore`), `dest: null` → artifact id + call id.
- UI: the desktop app (`cargo tauri dev` against the scratch dir) ran, but macOS refused to bring its window forward while Sky's own theuth and terminal were active and the occluded WKWebView paints nothing to capture (AX confirmed the **Export** popup exists in the bar). The visual run therefore used the same bundle in Chromium via `VITE_THEUTH_URL/TOKEN` (browser dev mode = the web/iOS path, `dest: null`): board in dark mode → **Export ▸ PDF…** → 5-page PDF, Graph (`export-04-board-pdf-p2.png`) and Sequence (`export-05-…-p3.png`) as vector figures in the light palette, Excerpts as numbered code (`-p4`), viewer opens under **Exports** (`export-03-viewer.png`); **Export ▸ Markdown…** on the note → markdown artifact rendered in the viewer (`export-07-…`). The desktop `save()` + Reveal branch is exercised by the RPC `dest` runs and code review only — not clicked.
- Board seeding: `doc_create` cannot make a board (that is the agent's `doc_create(kind="board")`), so the fixture body was created as a note and its `kind` flipped to `board` in `meta.json` + `documents/index.sqlite` with the daemon stopped.

## Deviations from the brief

- Tool registered in `artifact`, not `document` (dependency direction).
- `ExportResult.call_id` added: the artifact viewer routes by `call_id`, not artifact id.
- No off-screen rendering of unmounted fences (daemon text fallback instead).
- `document.typ` image handling unchanged (wide images already shrink to the text block); the parameter added is `font`.
