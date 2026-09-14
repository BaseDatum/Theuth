# 20 — Documents: PDF/Office files as Markdown

Status: as-built v0 · 2026-09-14 (draft same day) · extends `12` §10
(`read_file`), `16` (web fetch), `09` §7.4 (Signal attachments), `17`
(images: the `files` path the native tier widens).

Ported from `../jarvis/crates/jarvis-docconv`, minus the parts wth already
has or does not need (§7).

## 0. Problem

A PDF, DOCX, PPTX, XLSX, ODT, RTF, EPUB or legacy `.doc/.ppt/.xls` reaches
the model today in three ways, all dead ends:

- `read_file` (`tools/src/fs.rs:42-65`): `from_utf8_lossy` over the bytes →
  64 KiB of `%PDF-1.7 … stream x��`.
- `fetch` (`web/src/fetch/mod.rs:194-199`): `application/pdf` is in
  `is_binary` → `unsupported content type`.
- Signal (`signal/src/channel.rs:982-1017`): saved to
  `<data_dir>/signal/inbox/<ts>-<n>.pdf`, surfaced as
  `[attachment: path (application/pdf, 1.2 MB)]`; the model's only move is
  `read_file`, see above.

## 1. Facts relied on

- **`anydoc` 0.2.4** (Firecrawl; pure Rust, no external binaries): `Format::from_bytes(&[u8])
  -> Option<Format>` sniffs PDF header / RTF group / OLE stream names / ZIP
  package mimetype; `to_markdown_bytes(&[u8], Option<Format>) ->
  Result<String, ConvertError>` emits GFM. `ConvertError` is
  `#[non_exhaustive]` with `Unsupported | NeedsOcr { pages, page_count } |
  Malformed | Encrypted | ResourceLimit | MissingPart | Io`, implements
  `std::error::Error`, and `.code()` names the variant. CSV has no signature
  and is not detected — fine, CSV is text and stays on the text path.
- Conversion is synchronous; a multi-MB PDF is hundreds of ms (jarvis
  measured ~5 ms median on its fixtures, 30 s worst-case budget).
- Attachments and drops already arrive as **paths**, not bytes (17 §2), and
  the model already has `read_file`. One conversion point serves the TUI
  (a dropped `.pdf` path pastes as text — the model reads it), Signal, ACP
  `ResourceLink`s to local files, and `shell`-downloaded files.
- **genai already sends PDFs natively.** A `ContentPart::Binary` with
  `content_type: application/pdf` becomes Anthropic's `document` block
  (`anthropic/adapter_shared.rs:219-241`), Gemini's `document`, OpenAI
  Responses' `input_file` (`openai_resp/adapter_impl.rs:492-512`), Bedrock's
  `document`. The provider renders and OCRs scanned pages itself. wth's
  `user_message` (`loop_.rs:1960`) already builds `Binary::from_file` parts;
  only `check_files`' `image/*` gate, the flat `IMAGE_TOKENS` estimate and
  the text-only strip (17 §3) assume images. Cost: Anthropic bills a PDF
  page at ~1.5–3k tokens (text + rendered image), so a 40-page PDF is
  ~100k tokens against ~20k as Markdown — and it rides every request of
  the turn's history, as images do.
- anydoc returns **nothing** for a PDF with any scanned page
  (`NeedsOcr { pages, page_count }`): its documented choice, so partial
  output never passes as complete.

## 2. Shape: two tiers

1. **Text tier, default, every model**: `read_file` and `fetch` render
   documents to Markdown with anydoc (§2.1–2.3). Cheap, pageable.
2. **Native tier, the OCR path**: a PDF in `files` goes to a vision model as
   a provider document part (§2.5). Used when the text tier reports
   `NeedsOcr`, or when the user attaches the PDF outright.

No new crate, no new tool. The native tier is the §3.1/§3.3/§3.4 surface
of 17 widened from `image/*` to `image/* | application/pdf`.

### 2.1 `tools::doc` (new module, ~40 lines)

```rust
//! Documents (PDF, Office, ODF, RTF, EPUB) as GitHub-Flavored Markdown via
//! `anydoc`; sync and occasionally slow, so it runs on the blocking pool
//! under a deadline (design doc 20).

/// Largest input handed to anydoc; above it the caller says so instead.
pub const MAX_BYTES: usize = 25 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);

/// The format `bytes` carry, if `anydoc` recognizes one. Cheap for PDF/RTF/OLE;
/// walks the ZIP central directory for OOXML/ODF.
pub fn format(bytes: &[u8]) -> Option<anydoc::Format> { anydoc::Format::from_bytes(bytes) }

/// `bytes` as Markdown. `Err` names the reason in model-readable words:
/// `needs OCR (pages 3, 4 of 12)`, `encrypted`, `malformed: …`, `timed out after 30s`.
pub async fn to_markdown(bytes: Vec<u8>, format: anydoc::Format) -> anyhow::Result<String> {
    anyhow::ensure!(bytes.len() <= MAX_BYTES, "{} MB exceeds the 25 MB conversion cap", bytes.len() >> 20);
    let work = tokio::task::spawn_blocking(move || anydoc::to_markdown_bytes(&bytes, Some(format)));
    match tokio::time::timeout(DEADLINE, work).await {
        Ok(Ok(Ok(md))) => Ok(md),
        Ok(Ok(Err(e))) => Err(describe(e)),
        Ok(Err(join)) => Err(anyhow::anyhow!("converter panicked: {join}")),
        Err(_) => Err(anyhow::anyhow!("conversion timed out after {DEADLINE:?}")),
    }
}
```

`describe` matches `NeedsOcr` (the one variant whose fields carry
something the model can act on: which pages) and falls back to `Display`
for the rest plus the `_` arm the non-exhaustive enum requires. No error
enum of our own: anyhow strings are what a `ToolOutput` carries anyway.
The timeout abort drops the `JoinHandle`; the blocking thread finishes on
its own (same as jarvis; the pool is sized for it).

### 2.2 `read_file`

`fs.rs:42-46` becomes:

```rust
let bytes = tokio::fs::read(&a.path).await…?;
let text: String = match doc::format(&bytes) {
    Some(fmt) => {
        let md = doc::to_markdown(bytes, fmt).await
            .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
        format!("[{}: {fmt:?} rendered as Markdown; offset/limit count Markdown lines]\n{md}", a.path.display())
    }
    None => String::from_utf8_lossy(&bytes).into_owned(),
};
```

Everything after (offset/limit over lines, 64 KiB cap with its
`use offset/limit` hint) is unchanged and now addresses Markdown lines, so a
long PDF is paged the same way a long source file is. The header line is
what tells the model that.

A detected document that fails to convert is a **tool error**, not a
fallthrough to lossy text: `report.pdf: needs OCR (pages 3, 4 of 12)` is
useful; 64 KiB of PDF object streams is not. Undetected bytes (text, CSV,
unknown binary) behave exactly as today.

Description: `Read a text file, or a PDF / Word / PowerPoint / Excel / ODF /
RTF / EPUB document rendered as Markdown. Optional 1-based offset line and
limit line count …`.

### 2.3 `fetch`

`web/src/fetch/mod.rs::read_body`: drop `application/pdf` from `is_binary`
(the other entries stay). After the body is read, before `from_utf8_lossy`:

```rust
if let Some(fmt) = tools::doc::format(&raw) {
    if body_truncated { return Err(FetchError::Failed(format!("{fmt:?} larger than max_fetch_bytes; not converted"))); }
    let md = tools::doc::to_markdown(raw, fmt).await.map_err(|e| FetchError::Failed(e.to_string()))?;
    let title = text::first_heading(&md);
    let (content, cut) = hard_truncate(&md, max_chars);
    return Ok(FetchResponse { url, title, content, truncated: cut, took_ms });
}
```

The body is sniffed rather than trusting `Content-Type`, so a PDF served as
`text/plain` converts; one served as `application/octet-stream` is still
refused by `is_binary` before the body is read (add a sniff-first path when
that shows up). A truncated container is unparseable, hence the refusal
rather than a partial render. `web` already depends on `tools`
(`web/Cargo.toml:10`).

### 2.4 Signal, ACP, TUI paths

Signal's `[attachment: …]` line already carries the path;
`effective_mime` already sniffs `%PDF`. ACP `ResourceLink` already pastes
the uri (`acp.rs:208`). A `.pdf` dropped on the TUI pastes as its path
today. The new `read_file` description is what teaches the model to open
them. The changes below make the same inputs also attach natively.

### 2.5 Native tier: PDFs in `files`

`agent`:

- `check_files` (`supervisor.rs:1986-2009`): accept `mime.type_() == IMAGE
  || mime == APPLICATION_PDF`; cap stays 5 MB for images, 32 MB for PDF
  (Anthropic's documented limits: 32 MB, 100 pages — the page cap is left
  to the provider's error, which already surfaces as `AgentEvent::Error`).
- `compact::est` (`compact.rs:54`): `Binary` with `is_pdf()` →
  `pages × 2000`, pages = count of `/Type /Page` (not `/Pages`) in the
  decoded bytes, minimum 1. `// ponytail: byte scan, not a parse; lopdf if
  it is ever off by more than compaction cares about.` Images unchanged.
- `Frame::request` text-only strip (17 §3.3): `[document omitted: model is
  text-only]` for a PDF part, alongside the image marker. `text_of`
  (`loop_.rs:1982`): `[document: name]`.
- `user_message` error text `[image … unreadable]` → `[file … unreadable]`.

Producers:

- **TUI** `clip::dropped_file` (`clip.rs:98`): extension list gains `pdf`;
  token `[⎘ file 2: report.pdf]`. The `attach` text-only pre-check (17 §4.4)
  already fires the right notice.
- **Signal** `save_attachments` (`channel.rs:1012`): push `application/pdf`
  into `files` too, when ≤ 32 MB. The `[attachment: path]` line stays, so
  the model can also `read_file` it for a cheap re-read later.
- **ACP** `prompt_parts`: `BlobResourceContents` with `mimeType:
  application/pdf` → saved like `save_image` under `attachments/<sha>.pdf`,
  pushed to `files`. Other blobs keep the `[binary resource uri]` marker.

Consumer of the OCR signal: the `read_file` error for `NeedsOcr` reads
`report.pdf: pages 3, 4 of 12 are scanned (need OCR); ask for the file as
an attachment — a vision model reads the pages itself`. The model relays
that; the human drops the path / sends it on Signal; the native tier takes
it. No tool can do this step itself: tool results are text
(`ToolResponse.content`), so a tool cannot put a `Binary` part in the frame.

Why not attach natively by default and skip anydoc: cost (§1: ~5× tokens,
on every request of the turn), text-only models and non-genai backends
(`Backend::Cursor`, ACP children) get nothing, and DOCX/XLSX/PPTX have no
native path anywhere — only PDFs do.

## 3. Not in v0

- **Inline Markdown at input time** (jarvis
  `conversation.rs::load_document_markdown`: the attachment becomes `---
  begin file … ---` text in the user message). Saves one tool round-trip
  per document. With §2.5 in place the marginal code is small — a Signal or
  dropped `.docx` could be rendered there instead of attached — but the
  attachment/`read_file` pair covers it; add when the round-trip bothers
  someone.
- **Auto-escalation** from `NeedsOcr` to the native tier without the human
  in the loop: would need the turn loop to inject a `Binary` part on a tool
  error, a new input kind. The human step is one drop; revisit if it
  happens often.
- **Local OCR.** No pure-Rust OCR worth shipping; the provider does it.
- **CSV → table.** Text already; a 50k-row CSV as a GFM table is worse.
- **A `format` argument** on `read_file` to force/skip conversion. The
  sniff is content-based and has no false positives worth an option; add
  if one shows up.
- **Per-call size/timeout config.** Constants; `ToolsConfig` gets a key
  when a real document trips them.

## 4. Dependencies

`anydoc = "0.2"` in the workspace table, used by `tools` only. Pulls
`pdf-inspector`/`lopdf`, `calamine`, `cfb`, `zip`, `quick-xml`, `encoding_rs`.
`[patch.crates-io]` points `anydoc` and `pdf-inspector` at their GitHub tags
(v0.2.4, v1.15.0 — identical to crates.io) because a corporate proxy on the
build machine 403s static.crates.io for those crates and for lopdf 0.44's
`aes 0.9`; 1.15.0 keeps lopdf 0.42. Drop the patch when that clears.

## 5. Build order and verification

1. `tools`: `doc.rs`, `read_file` change, description. Unit: a tiny
   generated PDF (or the `%PDF-` header alone → `Malformed`/`Unsupported`
   error string contains the file name); `format(b"plain text")` is `None`
   so the text path is untouched.
2. `web`: `fetch` change. Existing `is_binary` test loses the PDF row.
3. `agent`: `check_files` PDF arm + 32 MB cap, `est` page count, strip
   marker, `text_of`. Unit: `check_files` accepts `x.pdf`, rejects `x.zip`;
   `est` of a 3-page PDF byte blob ≈ 6000; `request(vision=false)` strips
   the PDF part.
4. `tui` `dropped_file` + token text; `signal` `save_attachments`; `rpc`
   `prompt_parts` blob arm. One-line changes each plus the existing tests'
   fixture rows.
5. Smoke, not tests: `wth` TUI, `read_file` on a real PDF, a `.docx`, an
   `.xlsx`; `offset`/`limit` page through the PDF's Markdown; `fetch` of a
   public arXiv PDF URL returns headings; Signal-attached PDF from the
   phone → `[attachment: …]` → model reads it in one call; a scanned PDF →
   `read_file` says `need OCR` → dropped on the TUI → `claude-sonnet`
   reads the pages. Record results in §6 as 17 does.
6. Docs: this file's status line; `12` §10 tool table row for `read_file`;
   `16` fetch content-type list; `09` §7.4 and `17` §3.1 one sentence each.

## 6. Verified

2026-09-14, against the debug binary:

- `tools::doc` on real files: a 416-line Markdown from a 5-page report PDF
  in 308 ms (headings, a table); a 1346-line transcript `.docx` in 163 ms;
  `scan_with_native_header_text.pdf` (pdf-inspector fixture) →
  `pages 1 of 1 are scanned (need OCR); ask for the file as an attachment …`
  in 129 ms; `encrypted-secret123.pdf` → `document is encrypted`; a `.rs`
  source → not a document (text path).
- Native tier over ACP (`wth --acp`, throwaway Bun client): `session/prompt`
  with a text block and a `resource` block `{ mimeType: application/pdf,
  blob }` of that same scanned PDF, `anthropic/claude-opus-4-8` → the
  `messages.body` row holds `[Text, Binary{application/pdf}]`, reply quotes
  the page's text (`Order Detail Report by Account … WIDGET ASSEMBLY …`),
  `end_turn`, `usage_update used: 15535`. The page anydoc cannot read, the
  provider read.
- Unit: `doc::format` text/CSV → `None`; `check_files` accepts `x.pdf`,
  32 MB cap message, rejects `text/plain … not an image or PDF`; `est` of a
  3-page/1-`Pages` PDF = `3 × PDF_PAGE_TOKENS`; ACP PDF blob → sha-named
  `.pdf` in `files`, `.zip` blob stays a marker; `fetch` of a broken PDF →
  `Failed("Pdf: …")`, `application/zip` still rejected.
- Not exercised: `fetch` of a real PDF URL (no egress to test against here),
  Signal PDF from a phone, TUI drop of a `.pdf` (unit-level `dropped_file`
  only), Gemini/OpenAI-Responses document bodies.

## 7. What jarvis has that this drops, and why

| jarvis-docconv | here | reason |
|---|---|---|
| `detect.rs` ext → MIME alias → sniff | `Format::from_bytes` | inputs are files/bodies, not browser drops with lying MIMEs; anydoc's own sniff covers every signature-bearing format |
| `DocConvError` + `is_permanent()` | anyhow string | one caller per path, no retry loop |
| `ConvertOptions` (`max_input_bytes`, `timeout`, `format`) | two consts | nothing varies them |
| head+tail truncation | existing 64 KiB cap + `offset`/`limit` | the model pages; jarvis had no paging on the attachment path |
| fail-open to raw bytes | tool error | raw PDF bytes are never what the model wanted |
| `to_markdown_path` | — | `read_file` already has the bytes |
