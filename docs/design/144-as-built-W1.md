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
- (gates below; timings filled after the run)

## Gates
- (filled after the run)

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

## Hand-offs
- W2 (`index::on_document_changed`): called once per completed ingest with
  `DocumentChanged { doc, kind: Source, title, updated_ms: fetched_ms, deleted: false }`.
- Wave 2 insights: `ingest::insights::after_ingest(&Source)` is the hook; move/fill at will.
- C: `source_get` for a member is not membership-checked here (`api.rs` passes no principal to
  `ingest::source_get`); if §3.5 wants the check inside `document`, the RPC should pass `by` and `SourceGet` can
  take it.
