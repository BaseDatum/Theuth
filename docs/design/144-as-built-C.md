# 144 as built — C (CONTRACT + NOTEBOOK CORE)

Worker 144-C, branch `feat/144-c`, cut from `cb944b75`. Phase 1 = the skeleton every other worker codes against;
phase 2 = store CRUD + replication, `notebook_*` bodies, `DocKind::Notebook`/`Source` documents, the derived share
grant, `cite_resolve`, tests.

## What landed (file:line)

### Skeleton (phase 1)
- `crates/document/src/meta.rs`: `DocKind::{Notebook, Source}` after `Board` (serde/ts-rs like the others, `as_str`
  `notebook`/`source`); `Binding::{Notebook, Source(String)}` with `key()` unchanged (canonical JSON), new
  `Binding::keyless()` (`Note | Notebook`), `kind()`, `derived()` → `documents/<id>/notebook.md` /
  `documents/<id>/source.md`; `DocId::for_binding` = fresh v7 for keyless bindings, v5 otherwise;
  `free_standing()` unchanged (`Note | Board`); `SOURCE_READONLY = "source_readonly"` beside `Role`.
- `crates/document/src/notebook.rs` (new): §13.1 frozen shapes — `ItemKind`, `NotebookItem` (+ `row_id`),
  `Notebook`, `McpLabel`, `YoutubeRoute`, `SourceOrigin` (+ `key()` = origin key), `SourceStatus`, `Source`,
  `ContextLevel`, `ContextStatus`, `ContextItem`, `ContextEstimate`, `CiteKind`, `Citation` (reuses
  `codeindex::LocateStatus`), `SourceEvent`; reply shapes `NotebookDetail`, `SharedSource`, `DeletePreview`,
  `ItemRemoved`; actor messages `NotebookList/Get/Create/Update/DeletePreview/Delete`, `NotebookItemsList/Add/Remove`,
  `CiteResolve` with stub handlers (`144: not implemented yet`) on `DocumentActor`.
- `crates/document/src/store.rs`: `TABLES = ["docs", "notebooks", "notebook_items", "sources"]`; the §3.3 CREATE
  statements + `nbi_notebook`/`nbi_item` indexes run in `Store::open`; `NotebookRow`, `SourceRow`; CRUD
  `notebook_upsert/row/rows`, `item_add/remove`, `items`, `notebooks_of`, `source_upsert/set_status/row/rows`
  (real SQL; the replica engine stamps `hlc`); `create()` and `migrate_ids()` skip keyless bindings.
- `crates/document/src/lib.rs`: modules `cite_check`, `index`, `ingest`, `nb_tools`, `notebook`; `pub use notebook::*`;
  `nb_tools::register` beside `tools::register`. `Cargo.toml` `ts` feature adds `codeindex/ts`.
- `crates/document/src/tools.rs`: `Docs::resolve` accepts `source:<id>` / `notebook:<id>`; `Docs::edit` calls
  `cite_check::check_theuth_links` beside `check_excerpts` (no-op until W3); `doc_create(…, notebook?)` files the
  new note via `NotebookItemAdd`.
- `crates/document/src/nb_tools.rs` (new): tools `notebook_create/list/get/add/remove`, `source_add/list/get/refresh`,
  `nb_search` with schemas and risk classes (reads ReadOnly, writes Mutating), bodies delegating to the owning module.
- `crates/rpc/src/api.rs`: wave-1 methods `notebook_list/get/create/update/delete_preview/delete`,
  `notebook_items_list/item_add/item_remove`, `notebook_chats`, `session_set_notebook`, `source_add/get/list/refresh`,
  `cite_resolve`, `nb_search`, `save_answer_as_note`, `promote_answer`; writes `owner(ext)` + `doc_forward`
  (`NotHub` → forwarded); reads `principal(ext)`; `nb_result` maps `anyhow` errors (incl. `NotHub:`) like `doc_send`.
  `crates/rpc/src/bridge.rs`: `SourceEvent` forwarded on `subscribe_events`. `crates/rpc/src/export.rs`: the new
  types exported. `crates/rpc/src/search.rs`: `SearchGroup::{Sources, Notebooks}` over `document::index::BlockHit`,
  asked of `SearchSources`/`SearchNotebooks` (stub engines answer `unavailable`).
- `crates/search/src/lib.rs`: `SearchCategory::{Sources, Notebooks}`, `ALL` is 8. `crates/tui/src/search.rs`: label/wire
  arms; the two groups carry no TUI rows yet.
- `crates/agent/src/attach.rs`: `Kind::{Notebook, Source}` (`ALL` is 7) + two prompt rows;
  `crates/agent/tests/fixtures/attach_markers.json` + two cases; `crates/agent/src/notebooks.rs` (W3 stubs);
  `crates/desktop/ui/src/lib/attach.ts` `Kind`/`KINDS`/`KIND_LABEL`/regex extended.
- `crates/surface/src/anchor.rs`: `Anchor::Theuth(TheuthAnchor)`, `TheuthKind`, `Locator`, `TheuthAnchor::parse/href`,
  `THEUTH_SCHEME`; `parse()` routes `theuth://`; two unit tests.

### Delegation map (file → owner → stub signatures)
| file | owner | signatures |
|---|---|---|
| `crates/document/src/ingest/mod.rs` | W1 | `pub struct SourceAddArgs { notebook: DocId, path?, url?, text?, meeting?, session?, mcp: Option<McpRef>, title? }`, `pub struct McpRef { server, tool, args }`, `pub struct IngestActor;`, `pub async fn source_add(&ActorRef<DocumentActor>, SourceAddArgs, Principal) -> anyhow::Result<Source>`, `pub async fn source_get(&ActorRef<DocumentActor>, DocId) -> anyhow::Result<Source>`, `pub async fn source_list(&ActorRef<DocumentActor>, Option<DocId>) -> anyhow::Result<Vec<Source>>`, `pub async fn source_refresh(&ActorRef<DocumentActor>, DocId, Principal) -> anyhow::Result<Source>` |
| `crates/document/src/index/mod.rs` | W2 | `pub struct NbSearchArgs { query, notebooks?, docs?, kinds?, limit? }`, `pub struct BlockHit { doc, kind, title, block, heading?, score, r#match: TextMatch, href, notebooks }`, `pub async fn nb_search(&ActorRef<DocumentActor>, NbSearchArgs) -> anyhow::Result<Vec<BlockHit>>`, `pub async fn reindex(&ActorRef<DocumentActor>) -> anyhow::Result<()>`, `pub fn on_document_changed(&DocumentChanged)`, messages `SearchSources { query, limit }` / `SearchNotebooks { query, limit }` → `Result<SearchPage<BlockHit>, String>` on `DocumentActor` |
| `crates/document/src/cite_check.rs` | W3 | `pub fn check_theuth_links(markdown: &str) -> Vec<surface::anchor::TheuthAnchor>`; call site `tools.rs` `Docs::edit` |
| `crates/agent/src/notebooks.rs` | W3 | `pub struct SavedAnswer { doc: String, url: String }`, `pub async fn notebook_chats(&ActorRef<Agents>, String) -> anyhow::Result<Vec<SessionSummary>>`, `pub async fn session_set_notebook(&ActorRef<Agents>, SessionId, Option<String>) -> anyhow::Result<()>`, `pub async fn save_answer_as_note(&ActorRef<Agents>, SessionId, u32, String) -> anyhow::Result<SavedAnswer>`, `pub async fn promote_answer(&ActorRef<Agents>, SessionId, u32, String) -> anyhow::Result<()>` |
| `crates/document/src/notebook.rs` handlers, `store.rs` CRUD, `share.rs` grant, `cite_resolve` | C (phase 2) | the messages above |

Stub bodies return `Err(anyhow::anyhow!("144: not implemented yet"))` / `Err("144: not implemented yet")`;
the two search engines answer `SearchPage::unavailable`.

## Proof
- Phase 1: compile + unit tests only (see Gates). Phase 2: filled below.

## Gates
- Phase 1 (skeleton `3b107a19`): `cargo fmt --check` clean; `cargo clippy -p document -p rpc -p agent -p surface
  -p search -p tui --tests -- -D warnings` clean; `cargo test -p surface --lib` 93 passed (incl.
  `anchor::tests::theuth_grammar_accepts_the_five_kinds_and_their_locators`,
  `theuth_grammar_rejects_bad_kinds_locators_and_hashes`); `cargo test -p search --lib` 6 passed;
  `cargo test -p document --lib notebook:: meta::` 6 passed (`origin_keys_follow_the_grammar_and_dedupe_media_by_sha`,
  `wire_tags_are_the_frozen_words`, 4 meta); `cargo test -p agent --lib attach::` 5 passed (fixture with the two new
  cases); `just ts-desktop` export 8 passed; `bun run check` (tsc) 0 errors.

## Deviations
- `Notebook.chats` is served by the document crate from… nothing yet: `sessions.notebook` is W3's; phase 2 fills it
  through the RPC layer (`notebook_chats` count) or leaves 0 until W3 lands — recorded here, not a shape change.
- `SourceAddArgs` / `NbSearchArgs` are single struct parameters on the RPC (`source_add(args)`, `nb_search(args)`)
  rather than positional scalars — one shape for tool and RPC.
- `DeletePreview.sources_shared_elsewhere` is `Vec<SharedSource { source, title, also_in }>`, the design's `[(title, n)]`
  with the id added so the UI can link.
- `crates/tui/src/search.rs` (not in any 144 brief) got the two category arms so the workspace compiles; the TUI shows
  no rows for them.
- Phase 2 (orchestrator R-item): `packages/documents/src/schema.ts:34` `isValidLink` gains the `theuth:` scheme so
  BlockNote keeps `theuth://` links inside notes.

## Hand-offs
- W4 (`crates/desktop/ui`): to keep tsc at 0 after `Kind` grew, the skeleton touched `components/chat/AttachChip.tsx`
  (`KIND_ICON` + `popoutOf` cases) and `lib/attach-picker.ts` (`TAB_LABEL`, `categoriesFor`); `useOpenRef` has no
  `notebook`/`source` navigation yet (routes `/notebooks/$id`, `/sources/$id` are W4's). Search UI maps keyed by
  `SearchCategory` (`lib/search.ts` `CATEGORY_LABEL`/`DEFAULT_LIMITS`, `sidebar/SearchDialog.tsx` `ICON` + switches,
  `chat/AttachPicker.tsx`) need the `sources`/`notebooks` entries — see the Gates section for what the skeleton added.
