# Backend recon for design 218 (Whiteboard)

I changed nothing. I had no shell, so every claim comes from reading code with `grep` and `read_file`, and nothing was built or run. Paths are relative to `/Users/sky/Develop/skymoore/theuth-218-whiteboard`. I checked the design's own anchors against the tree. Most are accurate; the exceptions are called out in the final section.

## 1. Document store (`crates/document`)

**Anchors and signatures**
- `crates/document/src/meta.rs#L87-L113`: `pub enum DocKind { Knowledge, Artifact, MeetingNote, Note, AgentCharter, TaskThread, Board, Notebook, Source }`.
  - It derives `Serialize` and `Deserialize` (`snake_case`), `schemars::JsonSchema`, and `ts_rs::TS` behind feature `ts`.
- `meta.rs#L115-L148`: `as_str`, `free_standing()` (Note | Board), `homed()` (Note | Board | MeetingNote | TaskThread), `movable()` (Note only).
- `meta.rs#L156-L227`: `pub enum Binding`, with `key()`, `keyless()` (Note | Notebook), `kind()` and `derived(&DocId)`.
  - `Note` derives `documents/{id}/note.md`; `Notebook` and `Source` follow the same pattern.
  - `DocId::for_binding` (`meta.rs#L30-L38`) gives keyless bindings a fresh UUIDv7. Keyed bindings get a UUIDv5 of `Binding::key()`.
- `meta.rs#L374-L440`: `DocMeta { id, kind, title, binding, derived, fragment: String, members, links, agent, last_export, authors, created_ms, updated_ms, home, … }`.
- `lib.rs#L90-L91`: `pub const FRAGMENT: &str = "document-store"`.
- `lib.rs#L96-L140`: `DocumentsConfig`, with `max_doc_bytes: 8 * 1024 * 1024` at `#L130`.
- `actor.rs#L1061`: `pub const MAX_UPDATE_B64: usize = 48 * 1024`. It is enforced at `actor.rs#L1096` and `relay.rs#L627`.
- `actor.rs#L64-L95`: `pub(crate) struct Open { doc: yrs::Doc, meta, … }`. The `yrs` version is pinned `=0.28.0` with feature `sync` (`Cargo.toml#L224`).
- `actor.rs#L393-L507`: `apply(&mut self, doc, update: Vec<u8>, by: Principal, from_helper: bool, touched: Vec<String>)` is the single write path. It does these things:
  - checks the `max_doc_bytes` cap and decodes the v1 update;
  - skips no-op updates, then `apply_update`, then `store.append`;
  - calls `helper.update` when the update did not come from the helper;
  - calls `schedule_export`, then `fan_out(DocEvent::Update)`.
- `actor.rs#L1063-L1108`: `ApplyUpdate` (the client path). It runs `admit`, then `source_writable`, then `apply`.
- `actor.rs#L743-L772`: `export()` always calls `helper.markdown(&doc)` (the JS BlockNote helper process) and writes the result to `meta.derived`.
  - `ExportDone` (`#L1424`) overwrites `meta.title` from the file text unless `kind.free_standing()`.
- `actor.rs#L1994-L2050`: `create_doc(binding, board_root: Option<&Path>, title, markdown, by, home, reply)`.
  - Kind is `Board` if `board_root` is set, else `binding.kind()`; `fragment` is hard-coded to `FRAGMENT`.
  - It always ends in `start_import(markdown)`, which goes through the helper.
- `actor.rs#L1861-L1918` is `CreateBoard`, which reuses `Binding::Note` plus `board_root`. `#L1818` is `CreateNote`; `#L1832` is `Create { binding, title, markdown, by, home }`.
- `relay.rs#L231-L264`: `enum CreateReq { Ensure, Create{…}, Notebook{…} }`. This is how a create is forwarded to the document's server over the `theuth/docs/1` leg. `relay.rs#L327-L412` is `enum Cmd`, which carries Open, Update, Edit, Create and so on over that leg.
- `rpc_documents.rs#L12-L70`: trait `DocumentDocumentsApi`. It exposes `doc_list(kind: Option<DocKind>, limit)`, `doc_open`, `doc_update(doc, update: String)`, `doc_awareness`, `doc_subscribe`, `doc_snapshots`, `doc_markdown` and others.
  - There is no create RPC. Creation is tool-only (`tools.rs#L934`) plus roster and agent callers.
- `store.rs#L26`: `DIR = "documents"`.
- Blob storage: the only existing one is `ingest/mod.rs#L238` `save_blob(bytes, ext, data_dir) -> PathBuf`.
  - It writes `<data_dir>/attachments/<sha16>.<ext>`, content-addressed, local to the engine, and not replicated.

**BlockNote-shaped assumptions the review called out (confirmed)**
- Derived markdown always goes through the helper's `markdown` (`actor.rs#L761`).
- `Outline` and `Markdown` messages (`actor.rs#L1633`, `#L1773`) go to the helper.
- `load()` (`actor.rs#L320-L337`) opens a helper mirror on every doc and installs an `observe_deep` on the XML fragment for authorship.
  - For a whiteboard the fragment stays empty and the observer never fires.
- `index/mod.rs#L32-L38`: `INDEXABLE: [DocKind; 5]`. The indexer asks the actor for `Outline` blocks.
- `chunk.rs#L108-L115`: `kinds(blocks, doc_kind)` falls through to `"notebook"` for any unknown kind string.
- `share.rs#L271-L272`: DocKind → `ItemKind`.
- `tileview.rs#L540-L548`: a DocKind match; there is no `_` arm for new kinds in the hits I saw.
- `hub.rs#L937-L947`: `move_checks` has a `_` arm ("served by the hub").

**Non-BlockNote yrs doc.**
- The doc is a plain `yrs::Doc`, so top-level maps work: `open.doc.get_or_insert_map("cards")`, read inside `doc.transact()`.
- Existing code uses `get_or_insert_text("t")` in tests and `get_or_insert_xml_fragment` in `note_cells.rs#L114`. No non-fragment map reader exists yet.
- No Rust code reads Y.Maps with typed payloads today, so the yrs reader is greenfield.
- Writes: build a `transact_mut()` on a scratch clone, encode the update (`encode_diff_v1`), and push it through `apply(.., Principal::Agent{session}, false, vec![])`. This keeps log, fan-out, export and the cap consistent.

**How to add `DocKind::Whiteboard` end to end**
1. In `meta.rs`: add the variant, its `as_str` arm (`"whiteboard"`), and `free_standing` yes, `homed` yes, `movable` no.
   - Add `Binding::Whiteboard` as keyless (`keyless`, `kind()`, `derived` → `documents/{id}/whiteboard.md`).
   - `hub.rs` `move_checks` can rely on its `_` arm.
   - Update `tileview.rs#L540` and `share.rs#L271`.
   - Update `index/chunk.rs#L108` (otherwise whiteboard chunks are labelled `notebook`).
   - Add the kind to `INDEXABLE`.
2. Creation: add `Message<CreateWhiteboard>` next to `CreateBoard`.
   - It must not call `start_import`. It should make meta, call `store.create`, `publish(DocumentChanged)`, and seed the maps with one owner transaction.
   - Add a `CreateReq::Whiteboard` arm so non-serving engines forward (`relay.rs#L231`).
3. Editing and read-back: `apply`'s helper mirror (`helper.update`) and `export` (helper markdown) must be bypassed for the kind.
   - Branch inside `export()` on `kind`: call the new Rust outline renderer and write the outline to `derived`.
   - Skip `helper.open`/`helper.update` for the kind in `load()` and `apply()`.
   - Also check `unsynced_file` (`actor.rs#L556`), which assumes a derived file means a failed first import.
4. Tools: refuse `doc_*` on the kind.
   - `Docs::edit`, `ReadTool` and the writers go through `Edit`/`Outline`/`Markdown`.
   - Copy the `source_writable` pattern at `share.rs#L297`: a kind guard returning a fixed refusal string. Then add the same guard to `Edit`, `ImportMarkdown`, `Outline`, `Markdown` and `UndoLastAgentEdit`.
5. Blobs: call `save_blob` and add two RPCs (`wb_blob_put`, `wb_blob_get`) in a new `rpc_whiteboards.rs`. These are engine-local files. If the whiteboard lives on a different engine than the client, blob RPCs must route to the doc's server (see the risks section).

## 2. Attachment markers (design 66)

**Anchors**
- `crates/agent/src/attach.rs#L17-L41` is `PROMPT`, the standing block with the per-kind table.
- `attach.rs#L43-L76` holds `enum Kind { Note, Knowledge, Artifact, Session, Meeting, Notebook, Source }`.
  - It uses `serde(rename_all = "lowercase")` and `ts(export, rename = "AttachKind")`.
  - `Kind::ALL: [Kind; 7]` is at `#L68`.
  - `as_str` is at `#L79` and `Kind::parse` at `#L92` (it iterates `ALL`).
- `attach.rs#L105-L118`: `struct AttachRef { kind, id, title, meta }`.
- `attach.rs#L132`: `pub fn marker(&AttachRef) -> String`.
- `attach.rs#L155`: `pub fn parse(&str) -> Parsed`; `#L176` `parse_line`.
  - It stops at the first non-marker line (`#L163`).
- `attach.rs#L236`: `first_line`, which titles a marker-only first message from the first ref.
- TS twin: `crates/desktop/ui/src/lib/attach.ts#L16-L18` (`Kind` union and `KINDS`).
  - The `Source` union and `title`/`subtitle`/`idOf`/`toRef` switches are at `#L107`, `#L150-L226`.
- Shared fixture: `crates/agent/tests/fixtures/attach_markers.json`. It is read from Rust (`attach.rs#L359`) and from `attach.test.ts#L11`.
- Other Rust users of `Kind`: `crates/app/src/draft.rs`, `tasks/src/ledger.rs#L4069`, `tui/src/attach.rs`, `tui/src/components/search_attach/attach.rs`, `roster/src/desk.rs#L1011`.
- Tasks tool prose: `crates/tasks/src/tools.rs#L168-L170`. The doc comment reads `{kind: note|knowledge|artifact|session|meeting, id, title}`.
- Read tool per kind: `doc_read` for note/notebook/source/knowledge, `artifact_read`, `session_read`, `meeting_read` (all named in `PROMPT`).

**How to add `Kind::Whiteboard`**
1. Add the variant, `ALL: [Kind; 8]`, and the `as_str` arm in `attach.rs`.
2. Add one row to the `PROMPT` table: first look `wb_read(<id>)` or the `<whiteboard>` block. `PROMPT` is a pure constant, so the system-prompt cache is unaffected.
3. Mirror it in `attach.ts` (`Kind`, `KINDS`, the `Source` union with a whiteboard variant, and the three switches; they are exhaustive, so `tsc` will force them).
4. Add a fixture case to `attach_markers.json`.
5. Update the `tasks/src/tools.rs` doc comment, which is prose only.
6. Run `just ts` to regenerate `AttachKind.ts` (it is generated under `bindings/typescript`).
- The Rust matches on `Kind` were not all inspected, but adding a variant will show every non-exhaustive match as a compile error.

## 3. Notebook binding and `<notebook>` / `<notebook_changes>` injection (144/157)

**Binding**
- It is not a table. It is the column `sessions.notebook`, read as `Store::effective_notebook(&self, session: SessionId) -> anyhow::Result<Option<String>>` (`crates/agent/src/store.rs#L3665-L3676`).
  - It resolves to the root session's value through the `root_col!` macro (`store.rs#L476-L488`).
  - `notebook_chats` is at `#L3679`.
- Per-thread dial rows are the separate table `session_context(session, item, kind, level, hlc)` (`crates/agent/migrations/0045_session_context.sql`).
  - Accessors are at `store.rs#L3706-L3750` and `notebook_context.rs#L111-L330`.
- Migrations run up to `0076_session_title_guard.sql`.
- `SESSIONS_WIRE_SCHEMA: u32 = 70` (`store.rs#L735`).
  - It is pinned by a snapshot test at `store.rs#L4546-L4589`, `contacts.rs#L1205`, and the sessionsync tests.
  - The test lists the replicated tables via `capture::ATTACHED` (`store/capture.rs#L19`, 20 entries).
  - A new replicated table, such as `session_whiteboards` if sessions replicate, means a migration, an `ATTACHED` entry, a snapshot update and a bump.
  - A node-local table like `contact_files` avoids this (`store.rs#L4593`).

**Injection**
- The call site is `crates/agent/src/loop_.rs#L7503-L7565`.
  - It runs when `chat && round == 0`, the last history row is not already a notebook block, and `effective_notebook` is `Some`.
  - It computes `capacity`, picks the history (`state.history`, or the store frame under endless memory), calls `turn_message`, and then runs `self.persist(ChatMessage::user(text)).await`.
  - The ordering comment says: after `[attached …]`, before knowledge hints.
- `crates/agent/src/notebooks.rs#L463-L512`: `pub async fn turn_message(store, session, notebook, budget, capacity, epoch, history: &[(i64, ChatMessage)]) -> Option<String>`.
- `crates/agent/src/notebook_context.rs#L866-L886`: `next_message(...)`.
  - It returns the full block when no baseline exists and `epoch == 0`, and the manifest after a compaction.
  - Otherwise it returns `notice(...)`, or `None`.
- `notebook_context.rs#L586-L674`: `struct Baseline`. `Baseline::of(texts)` resets on a message starting `<notebook `, and applies messages starting `<notebook_changes` only if a block was seen (`#L598-L612`).
  - `read` parses tag lines; `attrs` (`#L677`) unescapes attributes.
- `notebook_context.rs#L853`: `notice(id, &[Change])`.
- Block renderers: `notebooks.rs#L645` and `#L711`.
- The static prompt side is `notebooks::prompt_blocks(...)` (`loop_.rs#L5785`).

**Recipe for `<whiteboard>` / `<whiteboard_changes>`**
- Add a `whiteboards.rs` module in `crates/agent` with `Baseline::of` copied over `<whiteboard ` and `<whiteboard_changes`.
- Add a `turn_message(store, session, whiteboard_id, epoch, history)` that fetches the outline from the document crate through a new actor message (a text outline plus a structured snapshot).
- Add a second `if` block in `loop_.rs` after the notebook block and before knowledge hints. Its guard must include a check that the last history row does not already start with `<whiteboard `.
- The binding query returns a list rather than an `Option`. Per-session baselines come from `history` itself, which is why the "three most recent" cap stays stateless.
- Dependency note: `agent` can call the `document` crate (it already does for `doc_*` and notebooks), but `document` cannot import `agent` (`ingest/mod.rs#L236` says so). The yrs outline reader therefore lives in `document` and the baseline logic in `agent`.

## 4. Runtime tools and the `## Boards` block

**Anchors**
- `crates/tools/src/lib.rs#L366-L400`: `trait Tool { const NAME; const RISK: Risk; type Args: DeserializeOwned + JsonSchema; const RETURNS = "str"; const DIRECT = false; const CONSUMES_SECRETS = true; fn description(&self) -> String; async fn call(&self, args, cx: ToolCx) -> anyhow::Result<ToolOutput>; … }`.
- `lib.rs#L150-L157`: `Risk { ReadOnly, Mutating, Destructive }`.
- `lib.rs#L337-L351`: `ToolCx { session, call_id, quiet, resources, cancel, promote_after }`.
- `crates/document/src/tools.rs#L75-L95`: `pub async fn register(reg: &ActorRef<ToolRegistry>, actor, data_dir)`.
  - It sends `RegisterPrompt { plugin: "document", text: [PROMPT, "\n\n", BOARDS_PROMPT].concat() }`.
  - It sends one `RegisterTool(Arc::new(XTool(d.clone())))` per tool.
- Registration is called from `DocumentPlugin::start` (`crates/document/src/lib.rs#L364-L376`), beside `nb_tools::register`.
- `ReadTool` is at `tools.rs#L624-L676`. It is `ReadOnly`, `RETURNS = "dict"`, and builds results with `structured(text, json!)` (`#L581`).
- Mutating tools: `InsertTool` at `#L697`, plus Replace, Delete, Move and Create.
- Args structs derive `Serialize, Deserialize, JsonSchema`.
- `BOARDS_PROMPT` is `tools.rs#L43-L61`. `PROMPT` is `#L25-L37`.
  - The design's `#L39-L44` anchor covers the doc comment and the start of the const, not the whole const.
- Non-direct tools show up in the python runtime automatically, as `await name(...)`.
  - Nothing else is needed: no wire fixture, no UI whitelist. `crates/theuth/harness.md` lists tools for the harness skill, and Slack's deliverable list is optional.
- A `ToolCx` carries `session` only. It has no engine handle, so a tool needs a captured `ActorRef<DocumentActor>`, as `Docs` does.
- `nb_tools.rs` (`#L1-L40`) is the nearest template: a `Notebooks { actor }` holder with ReadOnly reads and Mutating writes.

**Recipe for `wb_*`**
- Create `crates/document/src/wb_tools.rs`. Hold `Docs`-like state and register in `lib.rs#L372-L374` beside `nb_tools::register`.
- Reads (`wb_read`, `wb_card`, `wb_crop`) are `ReadOnly`; writes (`wb_create`, `wb_place`, `wb_update`, `wb_link`, `wb_frame`, `wb_dim`, `wb_resolve`, `wb_remove`, `wb_focus`, `wb_export`) are `Mutating`. Use `RETURNS = "dict"` with `structured(...)`.
- Add a `## Whiteboards` prompt by extending the `RegisterPrompt` text, plus one sentence appended to `BOARDS_PROMPT`.
  - `BOARDS_PROMPT` is a `pub const` string pinned by tests (`tools.rs#L1302-L1320`, which checks the skill, not the prompt). Check other tests that grep the prompt before editing.
- Writes should go through a new actor message that routes via `route_doc` (Serve / Park / Forward), as `Edit` does (`actor.rs#L1486`). That means a new `Cmd::Wb…` variant for non-serving engines.
- `ToolOutput` for `wb_place` should carry `{card, alias}`, with the alias assigned in the engine.

## 5. Built-in skills

**Anchors**
- Source markdown is embedded with `include_str!` from the owning crate. `document` ships `crates/document/board.md` via `pub const BOARD_SKILL: &str = include_str!("../board.md")` (`tools.rs#L65`).
  - Other examples: `computer::NATIVE_APPS_SKILL`, `double::tool::DOUBLE_SKILL`, `cells::prompt::SKILL`.
- Installation is in `first_run` (`crates/theuth/src/main.rs#L1356-L1378`).
  - It rewrites `<data_dir>/skills/<name>.md` whenever it differs. The binary owns these files, so user edits are overwritten.
  - The doc comment at `#L1349-L1355` lists the names and must be updated by hand.
- Format: front matter `---\nname: board\ndescription: "…"\n---`. `board.md#L1-L4` is the example.
- Index: `crates/agent/src/skills.rs#L351-L368` `prompt_index(&[SkillMeta])` emits `- {name}: {description}` under `## Skills`.
  - `always: true` skills are injected in full instead (`always_bodies`, `#L371`).
- Loader: `LoadSkillTool` (`skills.rs#L679-L705`), `LoadSkill` actor message (`lib.rs#L2620`), and the `load_skill` RPC (`rpc_skills.rs#L24`).
- The board skill test is at `tools.rs#L1302-L1320`. It checks `surface::check` and `check_fences`, requires `starts_with("---\nname: board\n")`, and checks for required phrases.
  - A whiteboard skill needs its own test of the same shape.
- The skill credit "dev.fast Whiteboard" is at `board.md#L6`. It is also asserted by the test string at `tools.rs#L1313`, not in the skill-loading code.

**How to add the `whiteboard` skill**
- Put `crates/document/whiteboard.md` (front matter `name: whiteboard`) next to `board.md`.
- Add `pub const WHITEBOARD_SKILL: &str = include_str!("../whiteboard.md")` in `tools.rs`.
- Add `("whiteboard.md", document::tools::WHITEBOARD_SKILL.to_owned())` to the `first_run` array and update the doc comment.
- Add a unit test mirroring the board one.
- Check that `grep -rn "skills/board.md"` and the harness skill (`crates/theuth/harness.md`) do not enumerate skills in a way that pins the list. I found no test pinning `first_run`'s list.

## 6. RPC and wire; TS bindings

**Anchors**
- An RPC trait is declared with `#[rpc_core_macros::rpc_descriptors(owner = "document", descriptors = rpc_descriptors, audience = "owner")]` and `#[rpc(server, client)]`.
  - `rpc_documents.rs#L12-L70` is the example. Impl is `impl DocumentDocumentsApiServer for Server` (`#L73`).
  - Handlers use `doc_send(&documents()?, Msg { … })`, `principal(ext)?`, and `with_extensions` for connection context.
  - The module function is `rpc_module(server)` (`#L233-L236`).
- Composition: `document::rpc_module` (`lib.rs#L28-L35`) uses `rpc_core::merge_described([...])`. A new `rpc_whiteboards.rs` module is added to that array.
- The typed client must be re-exported at `crates/rpc-compose/src/api/clients.rs#L1-L30` (for example `pub use document::rpc_whiteboards::DocumentWhiteboardsApiClient`).
- Subscriptions: `#[subscription(name = "doc_subscribe", unsubscribe = …, item = DocEvent)]` (`rpc_documents.rs#L68`). Other events use `wire::WireEvent` types, republished to clients in `crates/rpc-compose/src/client_mirror.rs` `republish!`.
  - The details are in knowledge `procedure/theuth-add-a-tool-with-a-wire-event` (seven steps).
  - I confirmed the `rpc_module`, `rpc_composition` and descriptor-validation structure, but not the `republish!` line numbers.
- Bindings pipeline (`justfile`):
  - `wire-schema`: `cargo test -p theuth --bin t --quiet export_bindings`. This is the test at `crates/theuth/src/main.rs#L2115-L2119`, which writes `bindings/openrpc.json`, `methods.json` and `client-auxiliary.json`.
  - `ts`: runs `wire-schema`, then `cd crates/desktop/ui && bun install && bun run wire:generate`. That is `scripts/generate-wire.mjs`, which uses `json-schema-to-typescript` over the OpenRPC schemas.
  - `ts-desktop`: copies `bindings/typescript/` and the catalogs into `crates/desktop/ui/src/rpc/bindings/`.
  - `wire-schema-accept`: copies `bindings/openrpc.json` to `crates/theuth/tests/fixtures/wire/openrpc-v1.json`, which is the committed golden schema fixture.
  - `bindings/typescript/` and `crates/desktop/ui/src/rpc/bindings/` are generated and git-ignored (knowledge warns about stale bindings). Event files are named `event.<topic with . as _2e>.output.ts`.
- Schemas come from `schemars::JsonSchema` derives. `ts_rs` derives are `cfg_attr(feature = "ts")` and not what the OpenRPC pipeline reads.
- The test at `main.rs#L2121-L2160` validates that described methods equal served methods.

**Recipe for new RPCs/events**
- Declare the trait in `crates/document/src/rpc_whiteboards.rs` with the descriptors macro and implement it on `rpc_core::Server`.
  - Methods: `wb_blob_put(doc, key, bytes_b64)`, `wb_blob_get(doc, key)`, `wb_list`, and `wb_create` (the client needs create; there is no `doc_create` RPC today).
  - Compose it into `document::rpc_module`. Add the client re-export in `rpc-compose`.
- If a wire event is needed (for example "reply bubble" or `wb_focus`), follow the 205 procedure: `WireEvent` struct, `register_wire_event`, `republish!`, goldens in `wire_golden.rs` and `wire_parity_inc5.rs`, and a fixture.
- Run `just wire-schema-accept` and `just ts-desktop`.
- Both `openrpc-v1.json` and the TS bindings are touched by every new type, including a new `DocKind` variant, which changes the `DocKind` and `Binding` enum schemas (the fixture already mentions `DocKind::Source`). The wire snapshot test is `cargo test -p theuth --test wire_schema_snapshot`.

## 7. Model selection per session (design 215)

**Anchors**
- Design: `docs/design/215-composer-model-picker.md`. A new component `ComposerModel(props: { session: string | null; own?: boolean; slash?: boolean })` lives in `components/chat/ModelMenu.tsx`.
- Set: `crates/agent/src/rpc_settings.rs#L169-L179`.
  - `set_model(&self, session: Option<SessionId>, model: String, manual: Option<bool>) -> RpcResult<ModelInfo>` (`with_extensions`). With a `session` it switches that session only. Without one it sets the default for new sessions.
  - The handler is at `#L558-L581`, with `authorize_opt_fwd` and `forward::route` for sessions on other engines.
- List: `list_models(&self, engine: Option<String>) -> RpcResult<Vec<ModelInfo>>` (`#L161-L162`, handler `#L536`).
- Effort: `set_effort(session, effort, manual)` (`#L191-L197`).
- Get: there is no `get_model(session)` RPC. The per-session model is `SessionSummary.model: String` (`crates/agent/src/lib.rs#L3248-L3268`). Clients read it from session list and events (`DefaultModelChanged`, and effort changes via `EffortChanged`).
- `favorites`: `set_model_favorite`.

For a whiteboard composer, the bound session id is the argument to `set_model(Some(session), model, None)`. Nothing new is needed on the backend.

## 8. `project_profile` and justfile commands

- `project_profile` reports cargo, `check: just check`, `test: just test`, `lint: just clippy`, `format: just fmt`, `build: just build`.
- `just check` runs `apps-bundle-check` and `documents-bundle-check` first, then `cargo check --workspace --all-targets`.
  - The bundle checks only matter if `packages/documents` or `crates/js-runtime/js` inputs change.
- `just test` is `cargo test --workspace`. `just clippy` is `cargo clippy --workspace --all-targets -- -D warnings`.
- Per-crate gates for this work:
  - `cargo test -p document`, `cargo test -p agent --lib`, `cargo test -p theuth --test wire_schema_snapshot`, `cargo test -p theuth --bin t export_bindings`, `cargo clippy -p document -p agent --all-targets --no-deps`.
  - UI: `cd crates/desktop/ui && bun run check` (`tsc -b --noEmit`) and `bun run test` (vitest).
  - The UI needs regenerated bindings (`just ts-desktop`) before `check` passes in a fresh worktree.
- Gate rule from the harness prompt: cap parallelism (`CARGO_BUILD_JOBS`, `--test-threads`), use a per-worktree target dir, and run no workspace-wide or release build while another session is building.
- Knowledge `facts/theuth-known-red-tests-nov-2026` lists tests already red on main.

## Risks and surprises against the design text

1. **The notebook binding is a column, not a table.** The design cites `store.rs#L3665-L3672` as "the notebook binding's shape". It is `sessions.notebook`, resolved through `root_col!` to the root session. `session_whiteboards(session_root, whiteboard)` is a genuinely new table. If it is created on the replicated session DB it needs a migration (`0077+`), a `capture::ATTACHED` entry, the `SESSIONS_WIRE_SCHEMA` bump from 70, and the snapshot and pinned-test updates (knowledge `warning/theuth-sessions-wire-schema-is-pinned-in-several-tests`). Decide whether it is node-local or replicated first.
2. **There is no whiteboard create path.** `create_doc` always imports markdown via the helper and hard-codes `FRAGMENT`. There is also no `doc_create` RPC, only the tool. A new message, a `CreateReq` variant for forwarding, and an RPC for clients are all needed.
3. **Every write path touches the JS helper.** `load()` opens a helper mirror, `apply()` forwards every update to it, and `export()` renders only through it. For a map-only doc this wastes work, and the export would write an empty or odd file. `unsynced_file` (`actor.rs#L556`) can also misread a whiteboard's derived file as a failed import. Branch on kind in all of these.
4. **Non-serving engines.** Document writes are hub-only and forwarded over `theuth/docs/1` `Cmd` legs. Engine-applied `wb_*` writes, `wb_blob_*` and the outline reader need a route (`Cmd` variants and `route_doc`), or they fail with `NotHub` on a machine that is not the document's server. The blob files under `attachments/` are local to one engine and are not replicated, so a client on another engine cannot read them without an RPC to the server.
5. **Quiet fall-through traps.** `index/chunk.rs#L108` labels unknown kinds `notebook`; `INDEXABLE` expects helper block outlines; `tileview.rs` and other kind matches need an arm. `summary_row` (`store.rs#L74`) drops docs with an unparseable kind/binding, so old engines will not list whiteboards.
6. **Wire fixture is a golden.** Adding `DocKind::Whiteboard`/`Binding::Whiteboard` changes `crates/theuth/tests/fixtures/wire/openrpc-v1.json`; run `just wire-schema-accept`. `bindings/typescript` and `src/rpc/bindings` are git-ignored; a parallel UI builder needs regenerated or hand-patched bindings (knowledge `procedure/theuth-parallel-ui-builders-against-hand-patched-bindings`).
7. **Size limits:** 8 MiB per doc, 48 KiB base64 per update (`MAX_UPDATE_B64`), 4 MiB IndexedDB on link devices. Chunk long stroke batches under 48 KiB.
8. **Design anchor errors:** "dev.fast Whiteboard" credit is `crates/document/board.md#L6` (tools.rs#L1313 is a test assertion). `BOARDS_PROMPT` spans `tools.rs#L43-L61`.
9. **Skill files are overwritten at every start** by `first_run`; `whiteboard.md` must be a compiled-in constant.
10. **`ToolCx` has no engine handle.** Tools capture `ActorRef<DocumentActor>` at registration; injection lives in `agent` and calls a document actor message (agent → document dependency is fine, not the reverse).
11. **Authorship:** `apply` stamps authors via a fragment observer; for maps it does nothing — `by` stays display-only (consistent with design).
12. **Title/move guards:** `ExportDone` and `Rename` (`actor.rs#L2081`) need `free_standing()` to include Whiteboard; `hub.rs#L937` `move_checks` refuses via `_` arm ("served by the hub").
