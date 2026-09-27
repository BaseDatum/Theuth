# 72 — Export: notes as Markdown / PDF, boards as PDF (Typst pipeline)

Status: shipped 2026-09-27 (EXPORT-1). Extends 35 (`show`, the in-process Typst engine), 59 (documents), 71 (boards, `json-render` fences), 30 (surface catalog).

## 1. Decision

Path C (Sky, 2026-09-27): reuse the Typst PDF engine the `show` tool already runs (`artifact::pdf::compile`, cmarker template `typst/document.typ`) for notes and boards. The board's Graph / Sequence fences arrive as vector SVGs the *client* rendered (the same `standalone()` the "Save SVG…" button uses), everything else is lowered to markdown on the daemon. A screen-faithful takumi-pdf path is §7.

## 2. RPC

```
doc_export(doc: DocId, format: 'markdown' | 'pdf', dest: Option<PathBuf>, assets: Vec<ExportAsset>) -> ExportResult
ExportAsset  { id: block id of the fence, svg: standalone SVG markup }
ExportResult { path?, artifact_id?, call_id?, pages?, warnings: [String] }
```

- Principal: `principal(ext)` like `doc_open`; the document actor's `MetaOf { doc, by }` admits members and yields the title. `dest` requires the owner (`owner(ext)`) and must be absolute, under `dirs::home_dir()`, with no `..` component. (`export_artifact` had no such guard; this one does.)
- `markdown`: `document::Markdown { annotate: false }` verbatim → `platform::files::write_private(dest)` (0600, tmp + rename, parent created), or an artifact `<title>.md` of `Kind::Markdown` that is **not** bound to a document (`State::show_as(.., bind: false)`).
- `pdf`: `document::Markdown { annotate: true }` (each top-level block preceded by `<!-- id -->`) → `surface::export::print_markdown` (§3) under a front matter `title` (the note title), `date`, `toc: true` when ≥ 3 headings, `font: Newsreader` → `pdf::compile(Source::Markdown)`. `dest` → bytes written as above; `None` → the `show` path (`Kind::Pdf`, source `.md`, page-1 preview PNG, `pages`). Typst warnings → `warnings`; diagnostics → error string (`format_diags`).
- Stored exports sit under `EXPORT_SESSION` (the nil uuid) with `call_id = export-<uuid7>`; the artifact viewer route is `/artifacts/$callId`. The agent's tool records under its own session and call id instead, so the export is a card in its chat.

## 3. Lowering (`crates/surface/src/export.rs`)

`print_markdown(text, images: BTreeMap<block id, path>)`: every closed `json-render` fence is replaced; `<!-- id [by …] -->` lines go; `[label](code:path#La-Lb)` → `label (path#La-Lb)` in prose; Mermaid fences and unparsable fences stay byte-identical.

| Root component | With asset | Without asset |
| --- | --- | --- |
| Graph | `![](<svg>)` + `*Graph: n nodes, m edges*` | `Diagram: n nodes, m edges` then `- **Label** → Target (edge label), …` grouped like the TUI adjacency list |
| Sequence | `![](<svg>)` + `*Sequence: title \| n steps*` | `**title**` then `1. From → To: label` |
| Excerpt | — | fenced block in `lang`, first line a comment `path#La-Lb` (comment syntax by language), `*caption*` under |
| Steps | — | `1. **title**` + indented detail |
| Table | — | GFM table (`\|` escaped) |
| Checklist | — | `- [x] text` |
| KeyValueList / Metric / MetricGrid | — | `- **key:** value` (metric delta in parentheses) |
| Timeline | — | `- **when** title: detail` |
| Callout / Alert / Quote | — | blockquote, `> **title**` first |
| Code | — | fenced block |
| Card / Stack / Grid / Tabs / Accordion | — | `**title**`, description, children lowered in order (tab / accordion labels bold above each child) |
| Heading / Text | — | `**text**` / paragraph |
| anything else | — | `*TypeName*` |

An SVG is only substituted when the fence's root is a Graph or Sequence; an asset for any other block is ignored.

## 4. Fonts

Newsreader (the app's serif, OFL) is embedded in the artifact crate: `crates/artifact/fonts/Newsreader.ttf` (variable, wght 400–600), `Newsreader-Italic.ttf`, `OFL.txt` — decompressed from the app's woff2 with `wawoff2`, the name table's family rewritten from Google's optical-size instance name `Newsreader 16pt 16pt` to `Newsreader` so CSS and Typst agree. `pdf.rs` passes them through `TypstEngine::builder().fonts(..)` beside the typst-kit embedded set and `artifact.fonts` dirs. The template reads `meta.font` (front matter `font`, default `Libertinus Serif`, so `show` output is unchanged); exports set `font: Newsreader`. Mono stays DejaVu Sans Mono. Images wider than the text block already shrink to it (`document.typ`), so SVGs are exported at their laid-out size.

## 5. Client (`ExportMenu.tsx`, `lib/doc-export.ts`)

**Export** in the note bar opens **Markdown… / PDF…**. Local daemon on desktop: `save({ defaultPath: <title>.md|.pdf, filters })` → `doc_export(id, format, dest, assets)` → `toast.success("Saved …", { action: Reveal → revealItemInDir })`. Web, iOS, a remote engine: `dest: null` → the artifact viewer opens on `call_id` (download from there). Warnings toast individually.

Assets for PDF: `mountedDiagrams(document)` reads every `.board-diagram[data-block]` frame's `<svg>` (BoardReader's `BoardDiagram`), `exportAssets` recolours it from the current `diagramTokens()` to `lightTokens()` — the light palette read by lifting `.dark` for one synchronous style resolution — in one regex pass, then `standalone()` sizes it and paints white behind. Fences that are not mounted (edit mode, a board still connecting) get no asset and the daemon lowers them to text.

## 6. What iOS / web get

The same RPC with `dest: null`: a `Kind::Pdf` (or `Kind::Markdown`) artifact with a preview, opened in the viewer; the viewer's own save/download is the byte sink. Diagrams still arrive as vectors when the board is mounted in the reader.

## 7. Later

- takumi-pdf (screen-faithful board render, Excerpts with the gutter and shiki colours, Newsreader prose exactly as on screen) as a second `format` — not added now, no `takumi`/`pdfcn` dependency.
- Off-screen rendering of unmounted fences (collapsed sections) on the client before collecting assets.
- `doc_export` for the agent with assets (would need a headless ELK layout on the daemon).
