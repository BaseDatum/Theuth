# 150 as built — PAGE-DRAFTS (drafts, `show{edits}`, `show` checks pages, `page_preview{path}`)

Branch `fix/150-page-drafts` off `main` `6d151b8a`. Crates touched: `artifact`, `surface`, `rpc` (one test harness).
UI untouched: `ArtifactCard` reads `name`/`path`/`content` off the args (`partialShow`) and `height` (`capFromArgs`);
a `show` carrying `name` + `edits`, or `check`, renders as the usual "showing x.html…" pending card and finishes on
`ArtifactShown`. No TS type, DB column, RPC method or client timing changed.

## The problem (main)

1. `show{content}` wrote nothing when it failed (argument checks, size, image inlining, PDF compile): the model's only
   handle on its page was to retype it.
2. On success the bytes became an immutable blob; the receipt had no path, and editing the blob would corrupt v1.
3. `page_preview` took only `html` ("exactly what you would pass to `show`"): the page was sent twice on the happy
   path and again for every fix, and the preview was invisible to the user.
4. Every fix therefore cost a whole page of tokens, and models regenerated pages rather than patching them.
5. Nothing checked a page before the user saw it unless the model remembered to call `page_preview`.

## Behaviour as built

### D1 — drafts (`crates/artifact/src/store.rs`)

- `State::drafts_dir(session)` = `<store dir>/drafts/<session id>/` — `<data_dir>/artifacts/drafts/…` for the
  durable store, the private tree's scratch `artifacts/drafts/…` for a private session (gone with the tree).
  `State::delete_session` removes `drafts/<session>/` first (missing dir = no-op, other errors logged).
  `State::sweep` only touches files directly in the store dir, so `drafts/` is never swept.
- `draft_file(name, source, content)`: the name itself when it is a plain basename with an extension on every
  platform (`safe_draft_name`: non-empty, ≤ 200 bytes, no leading `.` — which covers `..` — no trailing `.` or
  space, none of `/ \ : < > " | ? *`, no control characters, and not a Windows device stem `CON PRN AUX NUL
  COM1–9 LPT1–9` with or without an extension — `nul.html`, `Com1.txt`; on Windows `C:x` would replace the base
  dir and `x:stream` write an NTFS stream), else `draft-<sha8>.<ext|txt>` (`ext` only when alphanumeric). A `.pdf`
  name gets `.md`/`.typ` appended per `source` (`report.pdf.md`). `name` can never escape `drafts/<session>/`.
- `State::save_draft` writes `.<file>.<pid>.<seq>.part` then renames: atomic, last writer wins.
- Written by: `show{content}` as soon as the name is known and before the size/extension/inlining/compile/check/blob
  steps; `show{name, edits}` with the edited text; `page_preview{html}`. A successful `show{path}` of a text kind
  (Html, Markdown, Text, Mermaid, Csv, Svg, a `.md`/`.typ` compiled to PDF) refreshes the draft for its name after
  the insert, unless the path is that draft. Content over `MAX_BYTES` (32 MB) is not saved:
  `x.html: 33 MB exceeds 32 MB (not saved as a draft)`.
- The draft is the model's text: HTML before image inlining; PDF source, not the PDF.
- `show{content}` without a name, or with a name lacking an extension, still saves `draft-<sha8>.txt` and fails with
  `content requires name with an extension — saved at <path>; publish it with show{path: "<path>", name: "<name with extension>"}`.

### D1b — `show{path: x.md|x.markdown|x.typ, name: y.pdf}`

Compiled exactly like `show{content, name: "y.pdf", source}` (source by the path's extension): `Kind::Pdf`, a source
blob, a page-1 preview, `origin` = the path, receipt `· file <path>`, and the draft `y.pdf.md|typ` refreshed with the
text so `show{name: "y.pdf", edits}` can follow. A real `.pdf` path behaves as before (sniffed, no source, no file).

### D2 — receipts and errors (`kind.rs`, `tools.rs`, `store.rs`)

- `Receipt.file: Option<PathBuf>` (draft for content/edits, the origin path for `show{path}`, nothing for binary
  kinds). `kind::receipt` appends ` · file <path>` to the first line; `ShowTool` inserts `"file": "<path>"` into the
  structured JSON (the `Artifact` struct and the DB are unchanged).
- Every failure after the draft was written ends with:
  `saved at <path> — fix it with show{name: "x.html", edits: [{old, new}]}, or edit() that file and show{path: "<path>"}; do not resend the page`
  (`content` instead of `page` for non-HTML; for PDFs the publish form is `show{path: "<path>", name: "x.pdf"}`).
- A failed PDF compile: `x.pdf: compile failed\n<diags>\nsaved at <stem>.pdf.md — …`.

### D3 — `page_preview{html | path, name?}` (`preview.rs`)

- `PreviewArgs { html: Option<String>, path: Option<PathBuf>, name: Option<String>, width, appearance }`; exactly one
  of `html`/`path` (an empty `html` counts as absent): `page_preview: pass either html or path, not both` /
  `page_preview: pass html (the page) or path (an existing .html file)`. Both forms obey the 1–512 000-char rule
  (`page_preview: the page must be 1–512000 chars (got N)`).
- `html` is saved as the draft for `name` (default `preview.html`); the `Done` line ends with
  ` · file <path> — show{path: "<path>"} publishes exactly this page` and structured gains `"file"`.
- `PagePreviewTool { rt, stores }`; constructed in `lib.rs` next to `ShowTool`.

### D4 — `show` checks HTML pages (`preview.rs::render`, `store.rs::check_page`)

- `preview::render(rt, session, html, width, appearance) -> Outcome { Done{content_height, console, loaded, client} | NoRenderer | Failed(reason) | TimedOut | Cancelled }`
  is the PENDING registration → `PagePreviewRequested` → wait (2 s for `Started`, 15 s total) → `PagePreviewResolved`
  round trip; `page_preview` and `show` both call it. The `FEED.receiver_count() == 0` short-circuit is kept inside it.
- The one-preview-per-session rule is **dropped**: it protected nothing (the client runner keys frames by id and
  renders several at once, `installPagePreview` in `page-preview.ts`), and keeping it would have refused the second
  of two page `show`s in one assistant message or a `page_preview` during a show check. `Pending` lost its `session`
  field; the rpc test `one_preview_per_session` became `two_previews_in_one_session_run_in_parallel`.
- For `Kind::Html` (content, edits or path), after inlining and before the blob write: `check_page` renders the
  inlined bytes at 728 px with the client's default appearance.
  - console has `error` lines → not published:
    `x.html: not published — the page logged N error(s):\n[error] <first>\n[error] <second>\n[error] <third>` (each
    ≤ 300 chars) + `\n` + the D2 fix line + `, or pass check: false to publish anyway` (without a draft — `show{path}`
    — the suffix is ` — pass check: false to publish anyway`).
  - clean `Done` → published; receipt ` · checked on <client>: height <N>` and ` · M warning(s)` when any;
    web + height 80 + empty console → ` · checked on web (not measured)`.
  - `Done{loaded: false}`, `NoRenderer`, `Failed`, `TimedOut`, `Cancelled` → published unchecked:
    ` · not checked: the page did not finish loading | no renderer connected | client failed: <reason> | timed out after 15 s | cancelled`.
  - `preview::feed_live()` (a subscriber exists) gates the whole thing: no rpc bridge or no subscriber → `not checked:
    no renderer connected` with no wait at all. With subscribers but no client answering, the existing 2 s window.
  - Pages that are not UTF-8 or over 512 000 chars after inlining are published unchecked (`not checked: page is not
    UTF-8` / `page over 512000 chars`) rather than pushed through the event feed.
- `ShowArgs.check: Option<bool>` (default true; `.html` only).
- `ShowTool` did not need the `Runtime`: `State` already holds `rt` and publishes the events from `check_page`.

### D5 — `show{name, edits}` (`tools.rs::ShowEdit`, `store.rs::edit_base`, `apply_edits`)

- `ShowArgs.edits: Option<Vec<ShowEdit { old, new, all: Option<bool> }>>` (the `edit` tool's `EditOp` shape; a local
  type because `EditOp` is not `Clone`). Exactly one of path/content/edits: `pass one of path, content or edits`;
  nothing: `pass path (an existing file), content + name, or name + edits`; `edits require name`.
- Base: the draft `drafts/<session>/<file for name>` (`.pdf.md` then `.pdf.typ` for a PDF, or the one matching
  `source`), else the latest version — HTML blob (receipt adds ` · edited the published page (images already
  inlined)`), PDF `source` file (Typst by its extension), other text kinds' blob; binary kinds →
  `x.png: edits apply to text artifacts only`; nothing → `x.md: no draft or version of x.md in this session`.
- Plain exact matching, in order, all or nothing: `x.md: edit 2 of 3: old not found`,
  `x.md: edit 1 of 1: old found 4 times — add context or pass all: true`, `x.md: edit 1 of 1: old is empty`. A failed
  edit writes nothing (draft untouched, no version).
- The edited text is saved as the draft first, then the normal pipeline runs (validation → inlining → check → blob →
  version), so a refused page leaves the edited draft for the next edits. `[]` republishes the base unchanged.
- Receipt: `shown x.html (v2 · … · 2 edits applied) · checked on … · file <draft>`.

### D6 — guidance

Rewritten, same length as before: `tools.rs` `PROMPT` (new middle line: write once, fix with `edits`, never resend),
`ShowTool::description`, `ShowArgs` field docs; `kind::receipt`'s HTML line (`To change it: show{name, edits} or
edit() the file above and show{path} — never resend the page.`); `preview.rs` tool description, `PreviewArgs` docs,
`NO_RENDERER_MSG` (`show` checks pages itself and publishes unchecked when no app is connected);
`crates/surface/src/prompt.rs` page paragraph and pick-table row; `crates/surface/visualize.md` row and `## Pages`
rule. Design-history docs (`docs/design/120-*.md`) were left as they were: not model-facing.

## Files

| file | change |
|---|---|
| `crates/artifact/src/store.rs` | `show_as` split into `resolve` (args → `Input`, draft saved) and `publish` (pipeline, `ShowError { msg, refused }`); `drafts_dir`, `save_draft`, `edit_base`, `check_page`; free fns `src_ext`, `safe_draft_name`, `draft_file`, `apply_edits`; `delete_session` removes the drafts dir; D1b in `resolve`; tests |
| `crates/artifact/src/preview.rs` | `PreviewArgs` (html optional, path, name), `Outcome`, `render`, `feed_live`, `PagePreviewTool { rt, stores }`, `done(.., file)`, `Client::as_str`, new texts; the `client` test harness and D3/D4 tests |
| `crates/artifact/src/kind.rs` | `Receipt { file, edits, base_inlined, check }`, `enum Check`, receipt text |
| `crates/artifact/src/tools.rs` | `ShowArgs { edits, check }`, `ShowEdit`, `PROMPT`, description, `"file"` in structured |
| `crates/artifact/src/export.rs` | `ShowArgs { ..Default }` for the new fields |
| `crates/artifact/src/lib.rs` | `PagePreviewTool { rt, stores }` |
| `crates/rpc/src/api.rs` | `page_preview` test harness builds a `Stores` on a temp dir; `one_preview_per_session` → `two_previews_in_one_session_run_in_parallel` |
| `crates/surface/src/prompt.rs`, `crates/surface/visualize.md` | page guidance + the tests pinning it |

## Tests

`crates/artifact/src/store.rs`: `html_show_fails_listing_missing_images_and_keeps_the_draft` (1),
`show_names_the_editable_file` (2), `edits_make_a_new_version_or_change_nothing` (3),
`draft_names_never_escape_the_drafts_dir` (4), `path_of_markdown_source_compiles_to_pdf` (5, offline Typst like the
existing compile tests), `delete_session_removes_its_drafts`, `apply_edits_is_exact_and_ordered`,
`validation_errors` (nameless content saved), `html_show_inlines_and_counts_images` (receipt tail).
`crates/artifact/src/preview.rs`: `page_preview_takes_a_path_or_saves_html_as_a_draft` (6),
`show_checks_pages_before_publishing` (7: errors block, clean Done publishes with `checked on`, web unmeasured,
`Failed` → unchecked, `check: false` sends no request, two parallel page shows both checked),
`show_publishes_unchecked_without_a_renderer` (7: the 2 s window), `done_message_counts_errors_and_lists_console`
(file line). `crates/artifact/src/kind.rs::receipts_by_kind` (new parts). `crates/rpc/src/api.rs::page_preview::*`.
`crates/surface/src/prompt.rs::block_names_pages_after_the_fences`.

The simulated client: `preview::watch_feed` with a forgotten receiver (so `feed_live()` holds in that test binary) and a
kameo subscriber that hands request ids to the test, which answers through `preview::answer` — the same shape as the
rpc tests. The store tests' `content()` helper passes `check: Some(false)` so they never wait for a renderer once the
feed is installed.

## Gates

In order, on this worktree (`mbx`, stable 1.96.1):

1. `rustfmt --edition 2024` on the nine changed `.rs` files — only the edited regions changed.
2. `BUILD_LABEL=150-drafts build-lock mbx test -p artifact -p surface -p rpc --no-run` — EXIT 0.
3. `mbx test -p artifact -p surface -p rpc` (bare, same set) — EXIT 0:
   artifact lib 84 passed · `tests/document.rs` 1 · `tests/read.rs` 7 · rpc lib 56 · `tests/prompts.rs` 1 ·
   surface lib 97 · doc-tests 0/0/0; 0 failed (re-run after R1 — Windows-safe draft names — with the same result).
4. `BUILD_LABEL=150-drafts build-lock mbx clippy -p artifact -p surface -p rpc --all-targets -- -D warnings` — EXIT 0,
   zero warnings (none pre-existing in these crates either).

Two test-only fixes surfaced on the way: `commit_same_bytes_two_sessions_one_blob_two_records` now expects the fix
line after `call c1 already recorded` (a draft is saved before the insert); the rpc `page_preview` harness opens its
sqlite store on real time (`tokio::time::resume()`/`pause()`) because a paused clock auto-advances past sqlx's pool
acquire timeout.

## Deviations from the brief

- **`ShowTool` gained nothing**: `State` already owns the `Runtime`, so the check publishes from the store. No new
  field, no constructor change for `ShowTool`.
- **Error order for a refused page**: brief said fix line + `or pass check: false…`; built as `…do not resend the
  page, or pass check: false to publish anyway` — one sentence, same content.
- **One-preview-per-session rule dropped** (allowed by the brief; reasoning above).
- **`feed_live()` gate**: with no rpc feed installed at all (no bridge: TUI-only builds, unit tests) `show` publishes
  unchecked immediately instead of waiting the 2 s window; `page_preview` keeps the 2 s wait in that case (its rpc
  tests depend on it). With a feed and zero subscribers both short-circuit as before.
- **Two extra "not checked" reasons**: non-UTF-8 pages and pages over 512 000 chars after inlining are not sent
  through the event feed (same limit `page_preview` has always had).
- **`content` without an extension** is treated like no name (draft `draft-<sha8>.txt`), one error text for both.
- **Exports** (`doc_export` → `show_as` with `EXPORT_SESSION`) now leave a draft under `drafts/<EXPORT_SESSION>/`
  like any `show{content}`; harmless, overwritten per name.

## Open items

- Not exercised live: the real desktop round trip of the D4 check (a `show` of a page with `console.error` against a
  running app), the `web (not measured)` special case against the web client, and the TUI-only path (no feed → no
  wait). The engine half is covered by the simulated-client tests; the client runner is unchanged.
- The desktop asset scope already covers `<data_dir>/artifacts/` recursively (`crates/desktop/src/lib.rs`
  `allow_directory(…, true)`), so `drafts/` is inside an existing scope; nothing links to it and no URL names it.
  Narrowing the scope would be a desktop change, out of scope here.
- `show{edits}` uses exact matching only; the `edit` tool's whitespace-tolerant ladder could be reused later if models
  miss on indentation.
