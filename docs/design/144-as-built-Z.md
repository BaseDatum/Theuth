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

### Live-proof seams (commit `f4e8dd3c`) — defects only a running merged tree showed
- `chat/MessageAssistant.tsx:62-70,315`: streamdown's rehype-sanitize schema allows http/https/mailto-class hrefs only
  and `harden` prints `[blocked]` for a stripped one; `rehypePlugins` now carries `defaultRehypePlugins` with `theuth`
  on `protocols.href`, so chat chips render (W4 saw none live — no bound session existed on Sky's engine).
- `lib/injected.ts:46` + `crates/tui/src/model.rs:1399-1400`: the persisted `<notebook …>` row (W3's harness note)
  is `HIDDEN` in both transcripts — it rendered as a full user bubble of the whole context block.
- `crates/agent/src/notebooks.rs:219-253`: `save_answer_as_note` / `promote_answer` resolve `turn` as the **opener seq**
  (`HistoryEntry.turn`, what the UI holds and `regenerate`/`fork_session` take) instead of the 1-based `session_read`
  ordinal; the provenance href keeps `#<ordinal>`. The UI button failed with `session has no turn 0` before.
- `crates/document/src/lib.rs:261-274`: the first index walk waits (≤ 10 s) for the `embed` actor — the decision plugin
  starts after document, so the walk ran with `model none` and would have left existing documents lexical-only until
  their next edit. Log after: `index: walked 8 documents (0 failed), model embed-gte-modernbert-base`.
- `lib/citations.ts:77` `isProvenanceLine` accepts the helper's round-trip `*Answer from* *[chat](…)**` (BlockNote
  splits the emphasis around the link), so the Notes column says `AI from chat · 3 citations kept`.
- `components/notebooks/useNotebook.ts:64-69` `firstLine` skips the `# <title>` heading: the notebook document leads
  with it, and the header/Library row showed the title twice instead of the description.
- `lib/search.ts:19-20` `CATEGORIES` gains `sources`/`notebooks` (C filled labels/limits; the dialog iterates this
  list, so the chips and groups were absent). `SearchDialog.tsx` meta omits a heading equal to the snippet.

### origin/main fold (commit `56462e9f`)
`origin/main 8cd9c323` (106 commits: 145 TALK-PRESENCE, 141 ACP subagents, 125 R1 CONSULT, 147 docs) merged; one
conflict — `crates/agent/src/store.rs` `session_cols!` keeps both `root_col!("notebook") AS notebook` and 141's
observed-child columns. 141 also shipped `migrations/0041_observed_children.sql`, so W3's migration is renamed
`0042_session_notebook.sql` (144 has not landed anywhere; `sqlx::migrate!` refuses duplicate versions).
W3's `loop_.rs` / `supervisor.rs` / `lib.rs` hunks merged clean beside 145's. Bindings regenerated again after
the fold (export 8 passed).

## Proof
Scratch engine `t --headless --data-dir /tmp/144-engine` (rpc 127.0.0.1:7145, `anthropic/claude-sonnet-4-5` through
the env key, `link.enabled` on the public relay, `tools.policy.mutating: allow`), the gte checkpoint under
`models/embed-gte-modernbert-base/` (298 MB fp16 `model.safetensors`), the helper built from this tree, UI via bare
`bunx vite --port 5245`. Screenshots in `/tmp/144/z/*.png` and `144-briefs/z-shots/`. Per §16 W1 item:
- **Notebook**: `notebook_create("DR failover", …, "dr-2026")` → Library tab row `6 sources · 2 notes · project · dr-2026`
  (`library-notebooks.png`); page header shows the description (`notebook-page-chips-references.png`).
- **Cards → `completed`**: pasted text (55 words, 0.7 s), PDF from `cupsfilter` (1 page, 44 words), URL
  (`en.wikipedia.org/wiki/Disaster_recovery`, 44 words — a disambiguation page; canonical set), the scratch engine's
  own chat as a past session (337 words), a meeting from W1's fixture copied under `meetings/fx144/` (30 min,
  `meta.segments` 3). **YouTube**: card `failed · retry` with `no transcript route: captions failed (EOF while parsing
  …) and yt-dlp is not on PATH` — YouTube's caption endpoint answered empty and `yt-dlp` is absent; not proven.
  **Confluence/Jira**: no MCP server on the scratch engine; not proven. **m4a/mp4**: no STT checkpoint in the scratch
  data dir (copying from `~/.theuth` is out of bounds); W1's `ingest_audio` test is the proof.
- **`doc_read(source)`**: the agent, asked in the notebook chat, reported `doc_read("source:4ftli…")` outline = 4 blocks,
  first id `e9b5209f…` — the same 4 `.bn-block`s the source page renders.
- **`nb_search`**: lexical `failover drill DNS` → 5 block hits across the paste, the PDF and the notebook description
  with `theuth://sources/<id>#<block>?s=<sha8>` hrefs; the paraphrase `how long did switching to the backup site take`
  (no lexical overlap) ranks the RTO block first — vector hits with the real model (`nb_index_state.model =
  embed-gte-modernbert-base` for every document; `embed: … loaded (768 dims) in 8.4 s` cold, 1.1 s warm).
- **Ask in the notebook chat**: the `<notebook>` block was persisted with the description and the three sources at
  `full`; the answer cites two blocks; chips `(1)(2)` inline + References rows with title and `#block`
  (`notebook-page-chips-references.png`, `phone-notebook.png`).
- **Chip → source at the pulsed block**: click → `/sources/4ftli…#a7ced2a3…`, `.theuth-cite-pulse` on that block within
  200 ms (`source-page-pulse.png`); the same from a search-dialog `sources` row (`search-open-at-block.png`).
- **Save answer as note**: via the UI button after the turn fix → `Notes · 2`, rows `AI from chat · 3 citations kept`;
  the note carries paper chips 1–3 (BlockNote kept the `theuth://` hrefs — C's `isValidLink`) and the provenance line
  (`saved-note-chips.png`, `notebook-saved-note.png`). The title fell back to the answer's first words (the session
  actor was not live after the engine restart — W3's documented fallback).
- **`search("failover", [notes, sources, notebooks])`**: Sources `10+ matches`, Notebooks `3 shown`, notes row opens at
  `#block` (`search-sources-notebooks.png`).
- **Second mesh engine**: not run (pairing a second scratch engine through the relay is outside this proof);
  `tests/replicate.rs` covers the four tables.
- **`notes` search reads no files**: `crates/document/src/search.rs` is W2's rewrite over the index (no `note.md` read).

### Must-not-regress (§16)
- Tests in the regression set, all green: `crates/document/tests/actor.rs` (`sharing_links_join_roles_demote_revoke`,
  `member_text_is_labelled_for_the_agent`, `sharing_off_and_disabled`, `hub_gate_follows_the_lease`, … 11),
  `crates/link` 27 (incl. `guest_gate_admits_member_methods_only`), `crates/relay` 8, `rpc` 54 + 1.
- Live: `doc_link_create(note, editor)` → `https://relay.theuth.io/d#1.<engine>.<doc>.<secret>`; a second Chromium
  tab (no theuth) opened it, entered a name, saw the note live with the owner (`Chrome`) in Participants, typed a
  line — it appeared on the owner's page within the 5 s sample with the guest's orange caret and `z-guest` in the
  header (`share-owner-live.png`, `share-guest.png`). The deployed `/d` bundle on the relay predates this tree, so the
  notebook guest view and `theuth:` links inside the guest editor could not be exercised there.

## Gates
- `just ts-desktop` (BUILD_SLOTS=1 + mkdir lock): export 8 passed, twice (before and after the fold). The first attempt
  died with ENOSPC (root disk 100%); freed by deleting this target's own `debug/incremental` (29 GB), retried clean.
- `bun run check` (tsc): 0 errors (three runs; last after the fold).
- `bunx vitest run` (full): 209 files / 1416 tests → 1 pre-144 pin fixed (`SectionBand` 5 → 6 Library tabs); after the
  fold 211 files / 1436 tests, all passed.
- `bunx vite build`: built in 7.6 s (chunk-size warnings pre-existing).
- `cargo fmt --check`: the 144 crates clean (formatted `document/tests/ingest_audio.rs`; restored the trailing newline
  main dropped in `document/src/tools.rs`); the remaining diffs (`app`, `mesh`, `sandbox`, `signal`, `slack`, `talk`,
  `tasks`, `telegram`, `theuth`, `tui/src/lib.rs`, `decision/{src/tool.rs,tests/decide.rs}`) are `origin/main`'s own.
- `cargo clippy -p document -p decision -p agent -p rpc -p search -p surface -p voice -p tui --tests -- -D warnings`:
  clean before the fold (after the `Act::Send` box) and after it (one `field_reassign_with_default` in 141's new
  `tui/src/model.rs` test fixed by struct init).
- Post-fold `cargo test -p agent -p document -p rpc -p link`: agent 562 (+2 ignored) + 1 + 5, document 71 + 11 + 4 + 3
  + 1 + 5 + 1, link 27, rpc 55 + 1 — 0 failed; post-fold full vitest 211 / 1436 passed; tsc 0.
- `cargo test -p document -p decision -p agent -p rpc -p search -p surface -p voice`: agent 522 (+2 ignored,
  pre-existing) + 1 + 5; decision 93 + 2 + 3 + 3 + 4; document 71 lib + actor 11 + cite_check_links 4 + index_search 3
  + ingest_audio 1 + ingest_job 5 + replicate 1 (helper-gated ones with the helper built from this tree); rpc 54 + 1;
  search 6; surface 93; voice 25 — 0 failed.
- Regression set `cargo test -p document -p link -p rpc -p relay -p replica`: document 71 + 11 + 4 + 3 + 1 + 5 + 1,
  link 27, rpc 54 + 1, relay 8 green; replica 9 passed, **4 failed** — `files::tests::{hand_edit_becomes_row,
  row_becomes_file_and_nothing_ping_pongs, scan_between_apply_and_projection_does_not_revert,
  schedules_skills_mcp_round_trip}` time out in their own 5 s `until` on the file watcher, serially too; `crates/replica`
  has no 144 diff (`git diff 5cddad8e..HEAD -- crates/replica` is empty) and C saw the same four on their branch —
  environmental on this machine, not a regression of this work.
- `cargo check --workspace --all-targets` (BUILD_SLOTS=1 + mkdir lock): clean, 16 m 14 s before the fold, 5 m 50 s after.
- `cargo build -p theuth` (debug) twice for the live proof.

## Deviations
- W2's actor-site hook replaced by the bus subscription (above) — one hunk instead of eight, future publish sites
  covered.
- W4's dev fixture (`dev/notebooks-fixture.ts`) and type mirror deleted rather than ported: the engine answers now.
- `save_answer_as_note` / `promote_answer` `turn` = opener seq (W3 had the `session_read` ordinal); RPC doc updated.
- W3's migration renumbered `0042` behind 141's `0041`.
- `SearchDestination.notes` carries `block` (empty for a title hit) so a notes row opens at the block.
- Not fixed, recorded: the model cited the wrong block once (the RTO sentence lives in `fc75527b…`, it cited
  `a7ced2a3…`) — a model choice; `cite_check` only verifies existence. Vite's file watcher does not fire in this
  worktree (W4 saw the same); every UI change needed a vite restart.

## Hand-offs (wave 2)
- W3's `SideCall{Title}` for Save needs a live session actor; after an engine restart the title is the answer's first
  12 words. A store-side title (or loading the session) would make saved notes read better.
- `NotebookDetail.items[].title` (W3's ask) would spare one `doc_read`/`source_get` per item in the `<notebook>` block.
- The relay's deployed `/d` bundle must ship this tree's `packages/documents` schema (`theuth:` in `isValidLink`) and
  `NotebookGuest` before the notebook guest pair can be proven.
- `firstLine`/description: the notebook document body is `# title\n\ndescription`; a dedicated `description` on
  `Notebook` would avoid the markdown read on every Library row.
- YouTube captions: the `json3` track fetch returned an empty body for a public video; the audio route needs `yt-dlp`
  (`notebooks.youtube_ytdlp`). Worth a look at the player-response fetch (consent/`pot` token) in wave 2.
- The index first-walk wait is a bounded `lookup_when` on the `embed` actor (10 s); a start-order hook between the
  document and decision plugins would be cleaner than a timeout.
