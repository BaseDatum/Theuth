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

### Phase 2 (own scope)
- `crates/document/src/notebook.rs`: real handlers — `NotebookList` (rows ⋈ `docs` + per-kind item counts),
  `NotebookGet { id, by }` / `NotebookItemsList { notebook, by }` (membership-checked: `admit` on the notebook),
  `NotebookCreate` (hub-gated `create_doc(Binding::Notebook)` with the description body + `notebooks` row;
  `ImportReply::CreatedNotebook`), `NotebookUpdate` (title via `write_meta` + `DocEvent::Meta` + `retitle_heading`;
  project / `archived_ms` on the row), `NotebookDeletePreview` (notes, exclusive vs shared sources with `also_in`),
  `NotebookDelete` (soft-removes items, revokes derived grants, trashes exclusive sources when asked, trashes the
  notebook → `Closed{deleted}`), `NotebookItemAdd` (kind checks `not_a_source`/`not_a_note`, revival keeps the
  first add), `NotebookItemRemove` (`not_in_notebook`, `Closed{revoked}` on derived subscribers, `also_in`),
  `MayRead { doc, by }` (the RPC gate for `source_get`), `CiteResolve` (below); `sha8()` for W3. Every write
  publishes the notebook's `DocumentChanged` so lists/pages refetch.
- `crates/document/src/store.rs`: `items_remove_all`, `item_counts`, `summary`; test
  `notebook_tables_items_revive_and_live_lookups`.
- `crates/document/src/actor.rs`: `admit` is async (derived grant needs the store); `create_doc` takes a reply
  closure and returns the id; `retitle_heading` factored out of `Rename`; `Rename` accepts `DocKind::Notebook`;
  `apply_update` and `start_edit` call `source_writable` → `source_readonly` for anyone but `Helper`/`File` on a
  `DocKind::Source` (the insight job writes as `Helper` — the §4.3 hook).
- `crates/document/src/share.rs`: `admit` = direct member ∨ `derived_role` (best role over the live notebooks
  holding the doc as a `note`/`source` item; sessions/artifacts never grant); `revoke_derived(doc)` closes
  subscriptions of members with no direct membership and no remaining grant with `Closed{revoked}`;
  `revoke_derived_of(notebook)` on `MemberRevoke` of a notebook; `close_derived_member` with `demoted` on a
  notebook `MemberSet` demotion; `source_writable`.
- `crates/rpc/src/api.rs`: `notebook_get`/`notebook_items_list` pass the principal; `source_get` gates through
  `MayRead` first (W1's `source_get` has no principal). `crates/link/src/conn.rs`: `GUEST_METHODS` +
  `notebook_get`, `notebook_items_list`, `source_get`, `cite_resolve` (test `guest_gate_admits_member_methods_only`
  iterates the list). `notebook_chats` stays owner-only and off the list (D13).
- `cite_resolve` (§6.4): `TheuthAnchor::parse` per href; notes/sources: live `docs` row of the right kind and
  `admit(by)` else `available: false`; whole-document → `same`; `#<block>` → helper `markdown_of([block])`:
  text + no `?s=` → `same`, hash equal → `same`, differs → `changed`, block gone + `?s=` → outline walk, a block
  hashing equal → `moved` with `locator = #<new id>`, else `missing`; `@mm:ss` beyond `sources.duration_ms` →
  `missing`; knowledge → `<data_dir>/knowledge/<id>.md` exists (title from its frontmatter); meetings →
  `<data_dir>/meetings/<id>/` exists (title from the meeting note); sessions → see Deviations.
- `crates/surface/src/anchor.rs`: `Locator::text()` (used by `href()` and `Citation.locator`).
- `packages/documents/src/schema.ts`: `isValidLink` accepts `theuth:`.
- `crates/document/tests/replicate.rs`: two engines over a duplex transport (the `replica/tests/sync_loopback.rs`
  harness): notebook row, item, source row and the `docs` row reach B; a removal on A reaches B; a re-add on B
  revives on A keeping the first `added_ms`/`added_by` (column-write merge, §3.3).

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
- Phase 1: compile + unit tests only (see Gates).
- Phase 2: `cargo test -p document` — `notebook::tests::derived_grant_revocation_source_readonly_and_delete`
  (grant appears with the item, `not_member` for strangers, `viewer_readonly`, `source_readonly` on
  `ApplyUpdate` and `Edit`, list counts, archive filter, `Closed{revoked}` on item removal and on notebook delete,
  `also_in`, preview, exclusive-source trash, notes kept), `notebook::tests::cite_resolve_statuses_without_blocks`
  (same / missing / unavailable / time-beyond-duration / knowledge / sessions / malformed, member unavailability,
  `sha8` trims), `store::tests::notebook_tables_items_revive_and_live_lookups`, `tests/replicate.rs
  notebook_tables_replicate_between_two_engines`. Block-level `changed`/`moved` need the helper binary
  (`helper_or_skip!` harness) — exercised by W3's chip work; the status logic is the `(text, sha)` match in
  `CiteResolve`.

## Gates
- Phase 1 (skeleton `3b107a19`): `cargo fmt --check` clean; `cargo clippy -p document -p rpc -p agent -p surface
  -p search -p tui --tests -- -D warnings` clean; `cargo test -p surface --lib` 93 passed (incl.
  `anchor::tests::theuth_grammar_accepts_the_five_kinds_and_their_locators`,
  `theuth_grammar_rejects_bad_kinds_locators_and_hashes`); `cargo test -p search --lib` 6 passed;
  `cargo test -p document --lib notebook:: meta::` 6 passed (`origin_keys_follow_the_grammar_and_dedupe_media_by_sha`,
  `wire_tags_are_the_frozen_words`, 4 meta); `cargo test -p agent --lib attach::` 5 passed (fixture with the two new
  cases); `just ts-desktop` export 8 passed; `bun run check` (tsc) 0 errors.
- Phase 2: `cargo fmt --check -p document -p rpc -p agent -p surface -p search -p link` clean (`crates/tui/src/lib.rs`
  is unformatted on main and untouched here); `cargo clippy -p document -p rpc -p agent -p surface -p search -p link
  -p tui --tests -- -D warnings` clean; `cargo test -p document` 50 lib + 11 `tests/actor.rs` + 1 `tests/replicate.rs`
  passed; `cargo test -p link` 27 passed (incl. `guest_gate_admits_member_methods_only` over the grown list);
  `cargo test -p rpc` 54 + 1 passed; `cargo test -p relay` 8 passed; `cargo test -p replica` 9 passed, 4 failed —
  `files::tests::{hand_edit_becomes_row, row_becomes_file_and_nothing_ping_pongs,
  scan_between_apply_and_projection_does_not_revert, schedules_skills_mcp_round_trip}` time out in the test's own
  5 s `until` on the file watcher; `crates/replica` is untouched by this branch and depends on nothing this branch
  changed (document/rpc/link depend on it, not the reverse), so the failures are environmental on the shared
  machine (fsevents under load), not a regression of this work. `bun run check` (tsc) 0 errors; `isValidLink`
  smoke: `theuth://…` and `code:` pass, `javascript:`/`file:` refused, relative hrefs pass.
- Untouched existing share/guest/replication tests: every test in `crates/document/tests/actor.rs`
  (`sharing_links_join_roles_demote_revoke`, `member_text_is_labelled_for_the_agent`, `sharing_off_and_disabled`,
  `hub_gate_follows_the_lease`, …), `relay.rs` forward tests, `link` conn tests — all green above.

## Deviations
- `Notebook.chats` is 0: `sessions.notebook` is W3's column in the agent crate; the document crate cannot count
  it. Hand-off below.
- `cite_resolve` for `theuth://sessions/<id>#<turn>` answers `available: true, status: same, title: "Chat"`
  without checking: sessions live in the agent crate. Hand-off below.
- `Citation` has no `block` field (§6.4 sketch) — a `moved` citation puts the new id in `locator` (`#<id>`).
- `NotebookGet` / `NotebookItemsList` actor messages carry `by: Principal` (not §13 shapes; the RPC signatures
  are unchanged).
- `SourceAddArgs` / `NbSearchArgs` are single struct parameters on the RPC (`source_add(args)`, `nb_search(args)`)
  rather than positional scalars — one shape for tool and RPC.
- `DeletePreview.sources_shared_elsewhere` is `Vec<SharedSource { source, title, also_in }>`, the design's `[(title, n)]`
  with the id added so the UI can link.
- `crates/tui/src/search.rs` (not in any 144 brief) got the two category arms so the workspace compiles; the TUI shows
  no rows for them.
- Phase 2 (orchestrator R-item): `packages/documents/src/schema.ts:34` `isValidLink` gains the `theuth:` scheme so
  BlockNote keeps `theuth://` links inside notes.

## Hand-offs
- W3 (`crates/agent`, `rpc` notebook methods): fill `Notebook.chats` in `notebook_list`/`notebook_get` from
  `sessions.notebook` (a `COUNT(*) GROUP BY notebook` the RPC layer merges), and resolve `sessions/<id>#<turn>`
  citations (existence + title) in the RPC `cite_resolve` after `doc_send(CiteResolve)` — both are agent-crate
  reads the document crate cannot make.
- W4 (`crates/desktop/ui`): to keep tsc at 0 after `Kind` grew, the skeleton touched `components/chat/AttachChip.tsx`
  (`KIND_ICON` + `popoutOf` cases) and `lib/attach-picker.ts` (`TAB_LABEL`, `categoriesFor`); `useOpenRef` has no
  `notebook`/`source` navigation yet (routes `/notebooks/$id`, `/sources/$id` are W4's). Search UI maps keyed by
  `SearchCategory` (`lib/search.ts` `CATEGORY_LABEL`/`DEFAULT_LIMITS`, `sidebar/SearchDialog.tsx` `ICON` + switches,
  `chat/AttachPicker.tsx`) need the `sources`/`notebooks` entries — see the Gates section for what the skeleton added.
