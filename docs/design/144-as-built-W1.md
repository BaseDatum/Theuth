# 144 as built — W1 (INGEST: nine source origins, the ingest job, source_* bodies)

Worker 144-W1, branch `feat/144-w1`, cut from the contract skeleton `13bd3cc3`. Spec: `144-notebooks.md` §4 (routes,
job, representation), §13.1 shapes (frozen, untouched), §14 `notebooks.*` keys.

## What landed (file:line)

### The module `crates/document/src/ingest/`
- `mod.rs` — `SourceAddArgs` / `McpRef` (C's shapes, unchanged); `NotebooksConfig` (§14 ingest keys: `max_source_bytes`
  26214400, `max_source_chars` 2000000, `ingest_concurrency` 2, `youtube_route` auto, `youtube_langs` [en],
  `youtube_ytdlp` ~, `mcp.{confluence_tool,jira_tool}`; `serde(default)`, unknown keys ignored so W2/W3 add theirs);
  `Extracted` (what a route returns); the permanent-error words `UNSUPPORTED/TOO_LARGE/REFUSED/NO_ROUTE` + `permanent()`;
  helpers `mmss`, `word_count`, `cap_chars` (W-e notice block), `timed_blocks` (`## [mm:ss]` per ~60 s), `save_blob`
  (the `put_attachment` layout, `attachments/<sha16>.<ext>`), `provisional_title`, `project` (row + summary → `Source`).
  Reads are `DocumentActor` messages served on any engine: `SourceGet`, `SourceList`, plus `HubGate` (→ `NotHub:<id>`
  off the hub). Writes are the `IngestActor` (`#[derive(kameo::Actor)]`, registered `INGEST_ACTOR = "ingest"`):
  `Add { args, by }` (pre-flight → `Lookup`/`Create` the `Source` document under `Binding::Source(key)` → `sources`
  row `new` → `item_add` → `queued` → job), `Refresh { id }` (refused while queued|running), `Done`, `Active`.
  Concurrency: one `tokio::sync::Semaphore(ingest_concurrency)`; the in-memory `active` set is the truth for
  "queued|running" so a row left `running` by a crash can still be refreshed. The four entry points delegate.
- `job.rs` — `run(ctx, id, refresh)`: `running` → `routes::extract` → `cap_chars` → sha → `ImportMarkdown` as
  `Principal::Helper` (`Replace` first, `Diff` on refresh) → `Rename` when the origin knows its title and the card
  still wears the provisional one → `Snapshot("ingest <n>")` → row (`fetched_ms`, `content_sha`, `bytes`, `words`,
  `pages`, `duration_ms`, `blob`, `meta`, `changed_since = old fetched_ms` when the sha changed on refresh) →
  `completed` → `SourceEvent::Changed` → `index::on_document_changed` (W2's stub) → `insights::after_ingest`.
  Same sha on refresh: row only, no import. Errors → `failed` with the text; transient ones end with
  `(transient; refresh to retry)`. `SourceEvent::Progress` relays what a route reports (STT pieces).
- `insights.rs` — `after_ingest(&Source)` no-op hook for wave 2.
- `captions.rs` — YouTube without the network: `video_id` (watch/youtu.be/shorts/embed/live; id validated
  `[A-Za-z0-9_-]{11}` before anything is spawned), `tracks` (`captionTracks` out of the player JSON by a balanced
  scan), `details` (`videoDetails` title/author/lengthSeconds), `pick` (langs in order, human beats `asr`, else
  first), `json3_words`.
- `routes/mod.rs` — `preflight` (exactly-one-of, the origin, the permanent refusal), `extract` dispatch, `AUDIO_EXTS`
  / `VIDEO_EXTS` / `TEXT_EXTS`, `registry()` + `invoke()` (a quiet `tools::Invoke(ToolCall)` under a fresh session
  with a deadline — the path a plugin uses to call a registered tool).
- `routes/file.rs` — size/ext pre-flight (`415 unsupported: <ext>` from `anydoc::Format::from_extension` + text exts;
  `too large`), streaming sha256, `load` (reads `blob` or the added path, copies into `attachments/` when outside),
  `tools::doc::to_markdown`, txt/md verbatim, `pdf_pages` (page-object scan; the converter reports none).
- `routes/url.rs` — pre-flight `web::fetch::check_url` (SSRF guard → `refused: private address (<why>)`),
  `canonical = web::url::canonicalize(url).key`; extraction through the `scrape_url` ladder
  (`only_main_content`, `formats: [markdown]`, `max_chars`), the `web` fetcher alone when no registry runs (tests);
  `PageMeta` → `meta.meta`; PDF/Office bodies arrive as markdown from the fetcher's `read_body`.
- `routes/meeting.rs` — `<data_dir>/meetings/<id>/{manifest,transcript}.json` through a serde mirror (01 §5 contract;
  the `meeting` crate depends on `document`); `## [mm:ss] <speaker>` per segment, blank segments dropped;
  `duration_ms` from the manifest; `meta.segments = [{t0_ms, t1_ms, speaker}]`; title from the manifest.
- `routes/text.rs` — `text:<sha256>`; title = caller's or first line; the paste is saved as `attachments/<sha16>.md`
  (`blob`) so the job and a refresh read it like a file.
- `routes/session.rs` — `session_read` outline (title, N) then `turns=[from, N]` pages (64 KiB cut handled by
  re-paging from the cut turn) → `## Turn N — user | theuth` blocks, tool calls `*tool x: …*` one line.
- `routes/mcp.rs` — registered name via `tools::Mcp` `ListServers` (`McpToolInfo.registered`; off/disconnected are
  errors), `invoke`; Jira JSON → `# KEY summary`, fields table, `## Description`, `## Comments` one block each
  (ADF trees flattened), `meta {url,status,updated}`; Confluence object → body (storage/view HTML →
  `html-to-markdown-rs`), `meta {url,space,version}`; anything else: text as returned.
- `routes/youtube.rs` — pre-flight by `notebooks.youtube_route` (`audio` without `yt-dlp` → `no transcript route`
  naming `notebooks.youtube_ytdlp`); captions: watch page via the stealth fetcher (`web::WebConfig` policy as
  `scrape` builds it) → `captionTracks` → track as `json3` → blocks; `auto` falls back to `yt-dlp -x --audio-format
  m4a --no-playlist -o <attachments>/<id>.m4a` (fixed argv, 15 min deadline) + the audio route and rewrites
  `origin.route = audio`; `meta {route, lang, kind, channel, captions_error}`.
- `routes/audio.rs` / `routes/video.rs` — `voice::TranscribeWords` on the `TRANSCRIBER_ACTOR` (progress = pieces
  done) → `## [mm:ss]` per ~60 s; video = the same call (symphonia demuxes mp4/mkv/webm).

### Outside the module
- `crates/voice/src/stt.rs` (new items only): `TranscribeWords { path, progress }` → `WordTranscript { words,
  duration_ms }`, `Transcriber::transcribe_words` (`Engine::transcribe(piece, true)` per 180 s piece, times shifted);
  `lib.rs` re-exports the two.
- `crates/document/src/lib.rs:257-260`: `ingest::IngestActor::start(rt, store.clone(), actor.clone())` after the
  document actor registers (the one line).
- `crates/document/Cargo.toml`: `web`, `voice`, `url`, `html-to-markdown-rs` (workspace deps; cycle-free — `scrape`,
  `agent`, `meeting`, `browser` all reach `document` and are not importable).
- Fixtures `crates/document/tests/fixtures/ingest/`: `two-pages.pdf` (859 B, hand-built), `watch.html` (saved
  watch-page shape with three caption tracks and `videoDetails`), `captions.json3`, `transcript.json` +
  `manifest.json`, `tone-3s.wav` (16 kHz, 3 s).
- Tests: unit tests in `captions.rs`, `routes/{file,meeting,session,mcp}.rs`, `mod.rs`; integration
  `crates/document/tests/ingest_job.rs` (five tests, helper-gated like `tests/actor.rs`).

## Proof
- `tests/ingest_job.rs` (helper `theuth-documents 59.5`): pasted text → `completed` in **666 ms** end to end
  (row `new` → `queued` → job → helper `Replace` import → snapshot → row), `words 13`, `fetched_ms`/`content_sha`
  set, filed as `source` item `added_by owner`, listed per notebook and globally; refresh same bytes → attempt 2, same
  sha, `changed_since` none; blob rewritten → attempt 3, new sha, `changed_since` = previous `fetched_ms`, Diff import
  shows the new text. Four pre-flight refusals land as `failed` cards with no job (`415 unsupported: xyz`,
  `too large: …`, `refused: private address (…)`, `no transcript route: …`), `attempts 0`, all filed. The two-page
  PDF → `completed` in **870–930 ms** (anydoc 49–92 ms of it), `pages 2`, `bytes 859`, both pages in the document,
  original copied under `attachments/`. The meeting fixture → `## [mm:ss] <speaker>` blocks, `duration_ms` from the
  manifest, title "Failover sync" from the manifest via the heading/export path, `meta.segments` 3. Same paste in two
  notebooks → one document id, two item rows, a completed card reused not re-run, caller's title kept. Audio
  without the voice plugin and session without a registry → `failed … (transient; refresh to retry)`, re-adding a
  failed card re-runs (`attempts 2`).
- `tests/ingest_audio.rs` with `THEUTH_STT_MODEL_DIR=~/.theuth/models/parakeet-tdt`: the 3 s wav decodes to 3000 ms;
  Parakeet TDT load **756 ms**, word-timed inference of 3 s **166 ms** on this Mac (a pure tone → 0 words, as
  expected). Without the variable the inference half skips loudly.
- Unit tests: `captions` (video ids incl. path-traversal and 10/12-char rejects; the saved watch page → 3 tracks,
  `en` picks the human `en-GB` over `asr en`, details title/channel/2 461 000 ms; json3 → 12 words → three
  `## [mm:ss]` blocks), `meeting::render`, `session` header/turn/cut parsing, `mcp` Jira + Confluence renderers,
  `job::headed`, `cap_chars`, `timed_blocks`, `permanent`, config defaults + unknown-key tolerance.
- Not provable offline (no network in tests, by rule): the live URL ladder, YouTube fetches, `yt-dlp`, MCP servers,
  `session_read` through a running registry. Their parsers/renderers are unit-tested on fixtures; the glue is the
  same `invoke()` path for all three registry calls.

## Gates
- `cargo fmt -p document -p voice` clean.
- `cargo clippy -p document -p voice --tests -- -D warnings` clean.
- `cargo test -p document -p voice --tests`: document lib 63 passed (15 ingest), `tests/actor.rs` 11, `tests/ingest_audio.rs` 1,
  `tests/ingest_job.rs` 5; voice lib 25 passed — `live::tests::utterer_returns_the_spoken_sentence_punctuated`
  (macOS `say` + Nemotron streaming, untouched by W1) failed once in the full run and passed on rerun: environmental.
- Regression set `cargo test -p document -p link -p rpc`: document 63 + 11 + 1 + 5, link 27, rpc 54 + 1 (prompts), all green;
  no existing share/guest/replication test touched.

## Deviations
- `meta.segments` for meetings is `[{t0_ms, t1_ms, speaker}]`, not `[{t0_ms, block}]`: block ids are minted by the
  helper import after the markdown exists; `@mm:ss` resolves by the `## [mm:ss]` heading text (what `cite_resolve`
  scans), index-aligned with this list.
- `SourceOrigin::Url.canonical` is `web::url::canonicalize(url).key` at add time (the id must exist before the fetch);
  the page's own `<link rel=canonical>` lands in `meta.meta.canonical`, never in the key.
- The pasted-text route keeps the paste as an attachment (`blob`) rather than in the row — a 2 MB paste in a
  replicated column was the alternative.
- Filing the item uses `Store::item_add` directly (C's `NotebookItemAdd` handler is a stub on this branch); after
  merge the one call can route through the message if C's handler does more than the row (grant notification).
- `NotebooksConfig` lives in `ingest/mod.rs` (W1 owns the config struct per the brief); W2/W3 keys go there.
- No automatic retry of transient failures (none specified); `attempts` counts runs, `permanent()` classifies.
- Hub status for the ingest actor is a `HubGate` message on `DocumentActor` (defined in `ingest/mod.rs`); no such
  query existed.
- Title of a `Source` is not set by `Rename` (refused: `title_owned_by_binding` for bound kinds) — the job heads the
  markdown with `# <title>` (`job::headed`) and the title follows at export (`ExportDone`, the Knowledge rule). A
  caller's `title` wins over the origin's; the origin's replaces the provisional one (file name / URL / id).
- `webm` is classified audio (both §4.1 rows list it; the audio row first); `mov`/`m4v`/`flac`/`aac` are accepted
  beside the listed extensions since symphonia demuxes them.
- `pages` for PDFs is a page-object scan (`file::pdf_pages`), not a converter figure: anydoc reports pages only
  inside the OCR error.

## Hand-offs
- W2 (`index::on_document_changed`): called once per completed ingest with
  `DocumentChanged { doc, kind: Source, title, updated_ms: fetched_ms, deleted: false }`.
- Wave 2 insights: `ingest::insights::after_ingest(&Source)` is the hook; move/fill at will.
- C: `source_get` for a member is not membership-checked here (`api.rs` passes no principal to
  `ingest::source_get`); if §3.5 wants the check inside `document`, the RPC should pass `by` and `SourceGet` can
  take it.
