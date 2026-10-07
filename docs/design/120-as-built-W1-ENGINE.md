# 120 as-built — W1 ENGINE (artifact store inliner, kind sniffing, `show{height}`, receipts, prompt rules)

Branch `feat/120-eng` off `fd193e7e`. Design `docs/design/120-inline-pages.md` §8, §9a, §10.1–10.3,
§12.1 (non-W4 rows), §13.1 (non-`rpc` tests), `ShowArgs.height` (§6, D1), receipt (§3.9).

## What landed, where

| file | change |
|---|---|
| `crates/artifact/src/inline.rs` (new) | §8. `find_local_images` (`:143`): byte scan, a whole quoted string (`"` `'` `` ` ``) or an unquoted `url( … )` whose content is an absolute path — POSIX `/…` not `//…`, Windows `X:\…`/`X:/…` with JS-doubled backslashes un-escaped — ≤ 2048 bytes, no newline, extension in `png jpg jpeg gif webp avif svg bmp ico`; `raster_mime` (`:203`) PNG/JPEG/GIF/WebP/AVIF(`ftyp avif|avis|mif1`)/BMP(+reserved zeros)/ICO; `has_svg_root` (`:226`) one forward pass over BOM, `<?…?>`, comments, `<!DOCTYPE … [ … ]>`; `is_image_bytes` (`:269`); `inline_images` (`:282`) pure over `stat`/`read` closures: all missing paths listed at once → per-image ≤ 10 MiB → page budget from lengths before any read (`base64::encoded_len`) → read + magic check → splice at byte offsets → re-check ≤ 25 MiB. `Inlined.html` is `Cow::Borrowed` when the page names no image (no copy). `InlineError` `Display` carries the §8 wording. |
| `crates/artifact/src/sniff.rs` (new) | §9a `demote(kind, bytes) -> (Kind, Option<&'static str>)` (`:11`): `Pdf` needs `%PDF-` at 0; `Image` needs PNG/JPEG/GIF/WebP magic; `Svg` needs the SVG root; `Html`/`Text`/`Markdown`/`Csv`/others untouched. Reasons: `bytes are not a PDF (no %PDF- header); shown as a downloadable file`, `bytes are not a PNG, JPEG, GIF or WebP image; …`, `bytes are not an SVG (no <svg> root element); …`. |
| `crates/artifact/src/store.rs` | `sniff::demote` right after `kind::of` (`:691-695`); a compiled `.pdf` (`content` + `.pdf` name) is exempt — its bytes are markdown until Typst runs. `Receipt { demoted, height: args.height.map(|h| (h, h.clamp(80, 2000))) }` (`:696-700`). `Kind::Html` arm (`:742-760`) runs the inliner in `spawn_blocking` with `fs::metadata`/`fs::read`, fails `show` with `<name>: <InlineError>`, sets `receipt.images`, keeps `lines`. Mime (`:819-826`): `Image` follows the bytes (`raster_mime`); a demoted `File` records `application/octet-stream` (the name lies about the type, so the mime says nothing). Blob written only after inlining (unchanged order). |
| `crates/artifact/src/kind.rs` | `Receipt.images`, `.height: (asked, used)`, `.demoted` (`:108-113`); receipt line adds `N image(s) inlined` (omitted at 0) and `height 600` / `height 3000 → 2000` inside the parens, and for `Kind::Html` the §3.9 audience line on its own line (`:171-176`). |
| `crates/artifact/src/tools.rs` | `ShowArgs.height: Option<u32>` with the §12.1 doc (`:30-31`); description names live pages + `height` (`:57-64`); `PROMPT` gains the §10.2 sentence verbatim (`:37`); the `File` note reads `receipt.demoted` or the old `unknown kind…` text (`:84-92`). |
| `crates/artifact/src/export.rs:177` | `height: None` in the one exhaustive `ShowArgs` literal (not in my list, unavoidable: the field is new). |
| `crates/artifact/src/lib.rs:18-19` | `pub mod inline; pub mod sniff;` — rustfmt (`reorder_modules`) places them alphabetically: `inline` before `kind`, `sniff` after `search`; the brief's "directly after `pub mod kind;`" would fail `cargo fmt --check`. W4's `pub mod preview;` before `pub mod search;` is alphabetical too, so no adjacent-line clash with `sniff` (which lands after `search`). |
| `crates/artifact/Cargo.toml:35`, `Cargo.lock` | `base64 = { workspace = true }` — already resolved in the tree (`base64 0.22.1`); the lock change is one line in the `artifact` package's dependency list. |
| `crates/surface/src/prompt.rs` | §10.1 paragraph as one new line after the mermaid fence line (`:185`), the table row after `Image` (`:210`); nothing else reflowed. Test `block_names_pages_after_the_fences` (`:405`) checks order (mermaid < page < pick table; Image < row < Person) and that the skill carries `## Pages`, the §10.3 row before the board row, and the `html` example. |
| `crates/surface/visualize.md` | §10.3 row before the board row (`:43`); `## Pages` appended at the end of the file (`:185-234`, per the brief, not after `## json-render rules` as the design says): §10.1 rules, the 40 §7.1 variable names, a Chart.js CDN bar chart with `<canvas height="320">`, `:root` fallbacks, no outer card. |

Tests (§13.1): `artifact::inline` — `absolute_paths_only`, `windows_paths_unescape_in_js_strings`,
`extension_allow_list`, `svg_root_detected_after_prolog_comment_doctype` (also pins every raster magic),
`renamed_secret_is_not_an_image`, `oversize_image_fails_before_read` (asserts zero reads),
`page_limit_counts_base64_growth` (3 × 7 MiB file → 28.0 MiB projected, zero reads; 2 refs pass),
`missing_file_lists_every_path_once`, `offsets_splice_in_original_text` (exact output, borrowed when
nothing matches). `artifact::sniff` — `pdf_requires_magic_at_zero`,
`image_kinds_accept_any_image_magic_and_record_mime`, `svg_requires_root`, `html_is_never_sniffed`,
`mismatch_demotes_to_file_with_reason`. `artifact::store` — `html_show_inlines_and_counts_images` (real
tempdir store: two references to one PNG → 2 data URIs, no path left, `bytes` = stored length; a page
without images stored byte for byte), `html_show_fails_listing_missing_images` (missing + non-image,
nothing in the blob dir), `height_is_clamped_and_echoed` (600, 3000→2000, 10→80, none),
`pdf_named_html_bytes_is_a_file` (S0 ingest half: `File`, octet-stream, reason; JPEG under `.png`
stays `Image` with `image/jpeg`; text under `.svg` → `File` before the rasterizer; compiled `.pdf` not
demoted). `artifact::kind::receipts_by_kind` pins the Html receipt line. `surface::prompt` —
`skill_is_generated_and_valid` (unchanged, green), `block_names_pages_after_the_fences`.

Existing test changed: `store::tests::sweep_and_delete_session` wrote `b"not really png"` under
`shot.png` and asserted `Kind::Image`; it now writes PNG magic (the old bytes are a `File` by §9a).

## Gates (worktree, `CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-120-eng-target`, `CARGO_INCREMENTAL=0`)

```
cargo check -p artifact -p surface -p rpc --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 09s
cargo test -p artifact
    unittests src/lib.rs: test result: ok. 71 passed; 0 failed; 0 ignored
    tests/document.rs:   test result: ok. 1 passed; 0 failed
    tests/read.rs:       test result: ok. 7 passed; 0 failed
    Doc-tests artifact:  test result: ok. 0 passed
cargo test -p surface
    unittests src/lib.rs: test result: ok. 58 passed; 0 failed; 0 ignored
    Doc-tests surface:   test result: ok. 0 passed
cargo clippy -p artifact -p surface --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 19.72s   (exit 0)
cargo fmt -p artifact -p surface -- --check
    (no diff, exit 0)
```

## Deviations from the design, with reasons

- **Inliner reads via two closures (`stat`, `read`), not one.** §12.1 says "pure over a `read`";
  §8 also requires the page budget "computed *before* reading" and an oversize image to fail before
  its bytes are read. A single `read` cannot report a length without reading, so `stat` gives the
  length and `read` the bytes. The store passes `fs::metadata`/`fs::read`.
- **Reads are not truncated at 10 MiB + 1.** The `stat` check rejects oversize files before any
  read; a file that grows between `stat` and `read` is caught by the post-read length check. The
  T3 "stop one byte past the limit" stream is not needed with a stat first.
- **Image occurrences, not unique files, are counted** in `receipt.images` and in the page
  budget (each occurrence embeds its own data URI). `0 images inlined` is omitted from the receipt.
- **Compiled `.pdf` is exempt from the sniff** (design says "right after `kind::of`" with no
  exception): for `show{content, name: "x.pdf"}` the bytes at that point are markdown/Typst source,
  not a PDF; Typst's output is the stored PDF.
- **Demoted `File` mime is `application/octet-stream`**, not `mime_guess` of the name (which would
  record `application/pdf` for bytes the sniff just said are not a PDF).
- **`## Pages` is at the end of `visualize.md`** (brief) rather than after `## json-render rules`
  (design §10.3): the design-123 wave edits the file tonight; append-only avoids a conflict.
- **`lib.rs` module order** is rustfmt's alphabetical order, not "directly after `pub mod kind;`"
  (see the table). Content identical.
- **The `ShowArgs` doc on `name`** still lists `page.html`; nothing removed.

## Contract notes

`ShowArgs.height?: u32 (80–2000)` is unchanged from §14: the tool argument is persisted with the call;
the engine clamps for the receipt only and the UI reads `tool.args.height` raw. No contract change.

## Open items

- `.md` artifacts with local images (§15): the inliner's grammar is HTML/CSS/JS strings; markdown
  `![](/abs/x.png)` needs its own matcher — not built.
- The sniff's known false negative (a PDF with a PJL preamble) becomes a download, as §9a accepts.
- Windows paths on a POSIX host (`C:\…`) are matched and then reported unreadable; correct, and
  the error names the path.
- `tests/read.rs` and the TUI/channel crates were not touched: the receipt text is the only change
  they see, and `Kind::Html` artifacts flow through the same `ArtifactShown` as before.
