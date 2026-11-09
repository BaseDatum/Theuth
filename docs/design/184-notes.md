# 184 — notes (as built)

## Contract (W0)

One skeleton commit on `feat/184-notes-any-machine` (from main 06ccf506): every shape of §14 and the §15.1 seams, compiling, with no behaviour change. Signatures are in `184-contract.md`. Paths below are under `crates/`; line numbers are at the W0 commit.

### Shapes, file:line

| Area | Shape | Where |
|---|---|---|
| DocMeta | `home`, `home_ms`, `serve_epoch`, `served_by`, all `#[serde(default)]` | `document/src/meta.rs:411-420` |
| Homed kinds | `DocKind::homed()`, `DocKind::movable()` | `document/src/meta.rs:137,145` |
| Merge | `merge_meta(local, incoming, opens: &dyn Fn(&ShareLink) -> bool)`; callers pass `&\|_\| false` | `document/src/mirror.rs:119`; callers `document/src/store.rs:620`, `document/src/actor.rs:2397` |
| `LinkCreated.via` | `via: LinkVia` (`"home" \| "mesh"`, default `home`) | `document/src/meta.rs:343,361` |
| Bus | `CellLatestChanged{doc}` (`#[serde(tag = "type")]`), bridged | `document/src/meta.rs:512`; `rpc/src/bridge.rs:182,256` |
| `DocEvent::Server` | `{type: "Server", server, dual}` | `document/src/meta.rs:678` |
| `DocOpen` | `+ server`, `+ dual`, `+ sv`, all `#[serde(default)]` | `document/src/meta.rs:697-714` |
| `Homing` | `enum Homing { #[default] Here, Hub, Engine(EngineId) }` | `document/src/actor.rs:1688` |
| Creates | `CreateNote.home`, `Create.home` | `document/src/actor.rs:1702,1716` |
| Constants | `SERVER_MOVED`, `NOT_HOME`, `NOT_OWNER`, `HANDBACK_TIMEOUT_MS`, `JOIN_PARK_MS`, `BLOCK_WAIT_MS`, `PENDING_HOME_TTL`, `NOT_BUILT` | `document/src/hub.rs:219-235` |
| Routing | `enum Route`; `route_doc`, `serves`, `server_of`, `gate_doc`, `route_home` on `DocumentActor` | `document/src/hub.rs:239,252,263,271,279,290` |
| Actor messages | `Relayed<M>` (generic impl), `Handback`, `Handover`, `MoveHome`, `DocServer` | `document/src/hub.rs:303,305,321,344,368,391` |
| Docs wire | `Msg::Create/Handback/Block/Handover/MayRead` = tags 10-14 | `document/src/relay.rs:144-182`, tags `:199-203` |
| Wire bodies | `CreateReq`, `HandbackReply` | `document/src/relay.rs:230,267` |
| Leg commands | `Cmd::Create/Handback/Block/Handover/MayRead`; `Pending` variants; front-door frame mapping | `document/src/relay.rs:376-410`, `:631-646` |
| Hub-side refusal | member stream → `Closed{not_owner}` (10-13); otherwise `Closed{"not built yet"}` | `document/src/relay.rs:877-887` |
| `CodeBlockText` | `+ seen: Option<String>`; reply `+ markdown: String` (`#[serde(default)]`) | `document/src/cells.rs:54,41` |
| `CellLatest` | `+ engine: String` (`#[serde(default)]`) | `document/src/cells.rs:152` |
| Seam | `watch_latest(handle: ReplicaHandle, rt: Runtime)`, called after `replica::attach` | `document/src/cells.rs:75`; `document/src/lib.rs:256` |
| `cell_latest.payload` | in `CREATE_CELL_LATEST`, plus `ADD_PAYLOAD` in `Store::open` | `document/src/store.rs:67-76,212-218` |
| Join | `BUSY`, `JoinError::retryable()` | `document/src/share.rs:200,204` |
| Mesh frame | `net::Msg::DocsSchema{schema, homes_on}` appended; received → ignored | `mesh/src/net.rs:174,1235` |
| Mesh constants | `DOCS_SCHEMA = 2`, `DOCS_STALE_MS = 7 days` | `mesh/src/lib.rs:75,79` |
| Mesh asks | `DocsGate` → `DocsGateState{floor_ok, homes_on}`, `RemovedPeers` → `Vec<EngineId>` | `mesh/src/lib.rs:432,437,1137,1147`; `mesh/src/types.rs:365` |
| `mesh_status` | `MeshStatus.docs_homes_on: bool` (`#[serde(default)]`) | `mesh/src/types.rs:360` |
| Link | ask `MeshLinkId` → `Result<String, String>`; `MESH_LINK_UNAVAILABLE` | `link/src/types.rs:197,201`; `link/src/lib.rs:193` |
| RPC | `doc_move_home(doc, to) -> DocSummary`, `link_mesh_id() -> String` | `rpc/src/api.rs:1099,1107`; impl `:6816,6835` |
| TS | bindings regenerated; `api.ts` methods; `events.ts` `CellLatestChanged`; `guest.ts` `HOME_DIAL_MS`, `ShareTarget.engineIds`; mocks gain `engine: ''` / `docs_homes_on: false` | `desktop/ui/src/rpc/{api,events}.ts`, `desktop/ui/src/link/guest.ts:10,17` |

### Migrations

- Mesh: `crates/mesh/migrations/0008_peer_docs_seen.sql`, the next free number (0001-0007 on main). Design 183 P3 (branch `feat/183-p3`) adds no mesh migration, so 0008 does not collide. Local only: no `hlc`, not attached to the replica. **Superseded (Z184):** the table is created in `Store::open`, no numbered migration (a pre-184 build refuses a database with a migration it does not know); see "Closer (Z184)".
- `cell_latest.payload TEXT NOT NULL DEFAULT ''`: the document store has no numbered migrations (its tables are `CREATE TABLE IF NOT EXISTS` in `Store::open`, and `docs.hlc` is added by an `ALTER` tolerating "duplicate column"). The column is added the same way (§9.3), beside `ADD_HLC`. Old rows decode with `payload = ''`, and `Store::cell_latest` still reads the old columns (`engine = ""`).

### Stubs (W0 behaviour = today's)

- `route_doc` → `Hub::route()` mapped to `Serve`/`Forward`; `server_of`, `route_home` → `Hub::route()`; `serves` → `Hub::holding()`. No call site uses them yet: every handler keeps `hub.route()`/`gate()`/`gate_str()`/`holding()` with a marker above it.
- `Relayed<M>` handles `M` exactly as the bare message.
- `Handback`, `Handover`, `MoveHome` → `"not built yet"`; `DocServer` → `server_of` (today's holder).
- Docs tags 10-14 are never sent (nothing builds the new `Cmd`s); if received, refused cleanly with `Closed`.
- `DocsSchema` is never sent; received, it is logged and ignored.
- `DocsGate` → `{floor_ok: false, homes_on: false}` (homes off = today's paths); `RemovedPeers` → `[]`.
- `MeshLinkId` → the mesh identity this Link state already caches, else `"not built yet"`.
- `doc_move_home` → `MoveHome{checks: true}` → `ASK_FAILED "not built yet"`; `link_mesh_id` → `UNAUTHORIZED` unless `Forwarded`, then `"not built yet"`.
- `LinkCreated.via` = today's id choice: `mesh` when the link names the mesh device id, `home` when it names the engine link id.
- `mesh_status.docs_homes_on` = `false`. `DocOpen.{server, dual}` = `null`, `sv` = `""` (the client's push is skipped, as for an old server).
- No `todo!()`/`unimplemented!()` anywhere.

### Deviations from §14 and their reasons

1. `HandbackReply.agents` is `BTreeMap<String, uuid::Uuid>`, not `<String, SessionId>`: `agents.json` is that type in this crate (`Store::read_agents`, `document/src/store.rs:400`), and `document` has no `SessionId` type.
2. `Cmd::MayRead` gets its own private `Pending::MayRead` instead of `Pending::Unit`: `Unit` answers `Ok(())` without reading the reply and its sender types `DocError`, while §10.1 says the reply is `Result<(), String>`. `Pending` is private.
3. `gate_doc` signature, not given in §14: `gate_doc(&mut self, doc, relayed) -> Result<Route, DocError>`, whose `Ok` is only `Serve` or `Park`.
4. `route_home` returns `Result<Option<EngineId>, DocError>` (§5.2's table: `Ok(None)` create here, `Ok(Some(e))` forward, `Err` refuse); it takes the would-be home already resolved from `Homing`.
5. Wire constants are `u64` milliseconds (`HANDBACK_TIMEOUT_MS`, `JOIN_PARK_MS`, `BLOCK_WAIT_MS`, `DOCS_STALE_MS`); `PENDING_HOME_TTL` is a `Duration` (600 s), since §9.4 states it in seconds.
6. `DocsGate` and `RemovedPeers` answer the plain value (`DocsGateState`, `Vec<EngineId>`), as §10.2 writes them, not `Result`.
7. `LinkVia` is a new enum (`#[serde(rename_all = "snake_case")]`, TS `"home" | "mesh"`), with `#[serde(default)]` = `home` so a pre-184 `LinkCreated` decodes.
8. Added beyond §14, needed by it: `document::NOT_BUILT` (the W0 stub text, deleted with its last use), `share::BUSY` (the string `JoinError::Other("busy")` carries), `DocKind::homed`/`movable` (the "Homed kinds" row).
9. `net::Msg::DocsSchema` carries `#[allow(dead_code)]` until E-link sends it.
10. The `cell_latest` "migration" is an `ALTER` in `Store::open`, not a numbered file (see Migrations).

### Ambiguities decided (simplest option that keeps the design)

- **Ownership gap: `mesh/src/net.rs`.** §15.2 gives E-link `mesh/src/{types.rs, store.rs, lib.rs}`, but the `DocsSchema` frame lives in `net.rs` (`Net::drive`). Marked `// 184: E-link`; E-link owns `net.rs` for this.
- **Ownership gap: `rpc/src/bridge.rs`, `rpc/src/export.rs`.** Unowned; W0 bridges `CellLatestChanged` and exports `CellLatestChanged`/`LinkVia`, so no slice needs them.
- **`watch_latest` is already called** from `document/src/lib.rs` after `replica::attach` (a no-op body), so E-cells fills the body and E-doc has nothing to wire.
- **Front-door leg mapping for tags 10-14 is built** in `run_leg` (frame out, reply parse in). It sends nothing until a slice builds a `Cmd`; E-cells' `Cmd::Block` then works end to end once E-doc's server side answers.
- **Member streams** already get `Closed{not_owner}` for tags 10-13 (the frozen §10.1 rule, one guard); every other new tag gets `Closed{"not built yet"}`.
- **`doc_move_home` is wired** to `MoveHome{checks: true}` on the local actor; E-link adds the forward to `server_of`.
- **Callers' `Homing` values** follow §5.1: tools, UI/MCP `doc_create`, meeting, dreaming, autoresearch `Here`; insights and ingest sources `Hub`; task threads `Hub` with a marker (E-doc gives `Thread::create` its `home` and passes `Engine(task.home)`).
- **`agent/src/supervisor.rs:1732`** (E-cells' `(ran on <me>)` note) has no marker: a comment-only change would add the `agent` crate to every gate. §6.3 names the line.
- **Guest page:** `ShareTarget.engineId` → `engineIds: string[]` (v1 parses to `[id]`), `GuestApp` dials `engineIds[0]`, `HOME_DIAL_MS` exported. v2 parsing and rotation are U's (marked); v2 links are still refused as today.

### Seams

| Seam | Signature | Owner → user |
|---|---|---|
| Routing | `DocumentActor::route_doc(&mut self, &DocId, relayed: bool) -> Result<Route, DocError>`; `server_of(&self, &DocId) -> Result<Option<EngineId>, DocError>` | E-doc → E-cells |
| Block leg | `Cmd::Block{key: NoteKey, seen: Option<String>, reply: ReplySender<Result<CodeBlockTextReply, String>>}` | E-doc → E-cells |
| Latest watch | `document::cells::watch_latest(handle: replica::ReplicaHandle, rt: runtime::Runtime)` | E-cells → E-doc |
| Create homing | `CreateNote{.., home: Homing}`, `Create{.., home: Homing}` | E-doc → E-link |
| Move | `MoveHome{doc: DocId, to: EngineId, checks: bool}` → `Result<DocSummary, String>` | E-doc → E-link |
| Docs gate | `mesh::DocsGate` → `DocsGateState{floor_ok, homes_on}`; `mesh::RemovedPeers` → `Vec<EngineId>` | E-link → E-doc |
| Mesh link id | `link::MeshLinkId` → `Result<String, String>`; RPC `link_mesh_id() -> String` | E-link |

Markers: `rg '184: E-doc'` (50), `rg '184: E-cells'` (11), `rg '184: E-link'` (11), `rg '184: U' crates/desktop/ui/src` (11).

### Gates (W0)

Set: `-p document -p cells -p rpc -p mesh -p link -p tasks -p meeting -p dreaming -p autoresearch -p theuth -p artifact -p roster -p sandbox` (the last three hold test literals of `CreateNote`/`MeshStatus`).

- `mbx check … --tests`: `Finished`, 0 errors, 0 warnings.
- `mbx clippy … --tests --no-deps -- -D warnings`: `Finished`, 0.
- `mbx test -p document -p cells -p mesh -p rpc` (built `--no-run` first): every binary `ok` — cells 102, document 85 (+ actor 12, index_search 3, replicate 2, ingest 8, insights 2, cite 4), mesh 62, rpc 81 (+4); includes `relay::tests::frames_round_trip_with_matching_tag` (extended to tags 10-14), `mirror::tests::meta_merge_…`, `note_cells::tests::cell_latest_rows_…`, `replicate.rs` `cell_latest`, mesh `store::tests::*` (migrations through 0008).
- Old `cell_latest` table: `sqlite3` smoke — `ALTER … ADD COLUMN payload …` on a pre-184 table keeps the row with `payload = ''`; a second run fails "duplicate column name", which `Store::open` tolerates.
- Bindings: `mbx test -p rpc --features ts --lib export_bindings` ok (8 passed), 1012 files, copied to `crates/desktop/ui/src/rpc/bindings/`; `methods.json` lists `doc_move_home`, `link_mesh_id`.
- `bunx tsc -b --noEmit`: 0 errors. `vitest run` on the edited areas (link, rpc, cells, settings, share, lib/doc, notes, editor, tasks): 19 files, 120 tests passed.

## Contract changes after W0 (E-doc)

No frozen shape changed and nothing TS-visible moved (no bindings regeneration).

- Replies that W0 kept plain now delegate so a call can park (§4.3 step 4 names this): `Snapshot`, `Bind`, `Delete`, `DeleteNote`, `LinkCreate`, `LinkRevoke`, `MemberSet`, `MemberRevoke`, `MembersOf`, `MayRead`, and the 184 messages `Handover`, `MoveHome`. Their `ask` results are unchanged (`Result<T, E>` as frozen); only `Message::Reply` is now `DelegatedReply<Result<T, E>>`.
- `tasks::thread::Thread` (not in §14): `create(task, title, body, home: Option<&str>)` and a new `set_home(doc, home)`, as §5.1 and §4.8 describe.

## E-doc decisions (ambiguities, simplest option that keeps the design)

1. **Relayed.** `Relayed<M>` sets an actor flag `relaying` around `M`'s handler; `route_doc`/`route_home` read it, and a parked call re-sends as `Relayed` again. Handlers keep calling `route_doc(&doc, false)`.
2. **Claims.** `Claim::Pending{from, rest, epoch, parked}`: no `since` and no `done` watch. Parked calls are kept in the claim in arrival order and re-sent by one task (enqueued in order) when the claim ends or is dropped; a `Join` answers `busy` from its own timer at `JOIN_PARK_MS` and is then skipped.
3. **Actor-initiated legs** (`Handback`, `Handover`, `Create`, `MayRead`) get their reply sender from an internal `Leg<T, E>` ask (kameo has no public `ReplySender` constructor). `DocumentActor` now implements `kameo::Actor` by hand to keep a weak ref to itself, so tasks it spawns reach it rather than the registered name.
4. **Gate signal.** The actor asks `mesh::DocsGate` on every `MeshEvent` and every 30 s from `Tick` and applies the answer as an internal `HomesOn(bool)`; a flip reroutes. With W0's `DocsGate` (homes off) every path is today's.
5. **Homes off never sends tags 10-14:** `route_home` is then today's create gate (no `Cmd::Create`), claims never start, and `MayRead` forwards only with homes on.
6. **W-f idempotence** (a keyed create answers the existing document) runs only for a relayed create, so a lone engine keeps today's "already bound" refusal.
7. **§5.3 first snapshot** covers `CreateNote`, `CreateBoard` and `Create` (the `CreatedNote` reply), not `Ensure`/`NotebookCreate`.
8. **Handback acceptance:** from the document's home or the lease holder; a `Handover` target is the home by then (the old server wrote it first). The source does not fire a pending export before it unloads; the claimant's apply schedules one.
9. **Move copy names ids:** the document crate has no machine names, so `MoveHome` says `<id> is offline`, `already lives on <id>` and `moved to <id>; it has not taken the note yet`. The RPC layer may swap in names.
10. **`MoveHome` to this engine** writes the home and claims now only when the document is open (else on its next call); the unpair re-home is its own message (`Rehome`), which writes homes off-server and never claims.
11. **Re-home trigger:** the holder asks `RemovedPeers` on every reroute pass (homes on); `Rehome` scans `store.list` once per answer that is non-empty (memoised `home_of`). `ponytail:` a scan per holder reroute while a tombstone exists.
12. **Reseal without a vault** answers `holding()` (today's `true` on the hub, `false` elsewhere); with a vault every engine answers `true`.
13. **Leg lifetime:** a keyless forwarded create's one-shot leg is dropped once answered; keyed create, `Handover` and `MayRead` legs stay for later calls on the same key and go at the next reroute that points elsewhere.
14. **`pending_homes`** is written when the `Cmd::Create` is sent (not on reply), so the reroute pass keeps that leg and the creator's next call routes to the home.
15. **`serve_stream` `Handover`** is not wrapped in `Relayed` (§10.2's list omits it; it parks and answers itself).

## Gates (E-doc)

Set: `-p document -p tasks -p meeting -p dreaming -p autoresearch -p theuth`. Results are in `184-as-built-edoc.md`.

## Contract changes after W0 (E-cells)

None TS-visible; no bindings regenerated. Rust-only, inside the seams:

1. `CodeBlockText`'s handler reply is `DelegatedReply<Result<CodeBlockTextReply, String>>` (was the bare `Result`): the read may wait on a leg or an update. `ask` callers and `Relayed<CodeBlockText>` see the same reply type.
2. New `document::cells::UNKNOWN_AUTHOR = "?"`: `last_author = Member{pub_: "?"}` when a shared note's author cannot be told (§6.2). `cells::note::member_guard` needs no change: it answers `MemberAck("?")` and accepts `member_ack = "?"` for that author only.
3. `CodeBlockTextReply.markdown` is filled when `seen` is set (a press), `""` otherwise.
4. Cells-internal: `note::route`, `note::Route`, `Docs::hub` deleted (W-m); `Docs::code_block` and `note::source` take `seen: Option<&str>`.
5. `cells/Cargo.toml` gains `mesh` (for `mesh::self_id`, already a transitive dependency).

### Ambiguities decided (E-cells)

- **Block wait without an actor field.** §6.2 puts `block_waits` and `Tick` in `DocumentActor` (`actor.rs`, E-doc's). The wait is a spawned task subscribed to the document that re-reads on each `DocEvent::Update` until the digest matches or `BLOCK_WAIT_MS` passes: the same parked reply, no sleep, no change to `actor.rs`.
- **`relayed`.** The handler routes with `route_doc(doc, false)`; carrying `true` for `Relayed(CodeBlockText)` is E-doc's generic `Relayed<M>` impl. On the server the route is `Serve` either way.
- **`Park`.** Answered like `Serve` (local Doc, waiting on `seen`): the claim's hand-back lands as an update the wait re-checks. Swap in E-doc's re-send park if a press must outwait a whole claim.
- **Homes off** is not read: the read always tries the server, and an old (no tag 12) or unreachable server fails the leg into the replica fallback within `HANDBACK_TIMEOUT_MS`.
- **Any server error falls back**, including "no code block": the press's digest check still refuses a text the user did not see.
- **Legs without an engine.** The fallback's `Cmd::Open` to the holder rides `(Client::Conn(None), doc)`; W0's `Legs` map has no engine in its value, so after a timed-out `Block` the Open may go to the server instead. §9.2's `(EngineId, Sender)` legs (E-doc) fix it at integration.
- **Files touched outside §15.2**: `document/src/note_cells.rs` (unowned; `blocks_of` extracted for the fallback merge, the `cell_latest` test un-gated, the §6.2 tests beside its fixtures) and `cells/Cargo.toml` (unowned; `mesh`). `agent/src/supervisor.rs` is the §15.2 line.

### Tests and gates (E-cells)

Fail-before: each new test run against W0 behaviour (the W0 `CodeBlockText` body, `watch_latest` a no-op, `payload` ignored on read, `PutCellLatest` gated, no engine stamp, local markdown only), then restored:

| Test | W0 result | Now |
|---|---|---|
| `rpc cells::tests::note_scope_routes_here` | `[INFERENCE]` `Err` (W0's `note::route` asks the absent documents actor; the function is deleted, so not re-run) | ok |
| `document note_cells::tests::off_the_server_a_press_reads_the_servers_block` (fake server answers `Block`) | stale local text, `markdown ""` | ok |
| `…::nobody_answers_a_shared_note_reads_here_with_its_author_unknown` | author `Owner`, not `?` | ok |
| `…::an_old_server_the_holder_names_the_author` (server refuses tag 12, holder answers `Open`) | text without the holder's edit | ok |
| `…::the_server_reads_its_own_doc_and_waits_for_the_keystroke` (single engine; no-digest read is today's reply) | "waited for a matching update" fails | ok |
| `…::cell_latest_rows_upsert_anywhere_and_go_with_the_note` | `NotHub:other` | ok |
| `cells actor::tests::an_unknown_author_asks_to_confirm_and_the_servers_defaults_apply` | the confirmed press fails (no cwd: the server's defaults unused) | ok |
| `cells note::tests::hooks_against_a_fake_document_side` (engine) | `engine ""` | ok |
| `replicate.rs cell_latest_replicates_between_two_engines` (`CellLatestChanged` on B only) | times out | ok |
| `replicate.rs two_engines_runs_of_one_cell_never_mix` | mixed: `run=r_b exit=Some(1) text="from A"` | ok |

- `mbx check -p cells -p document -p rpc -p theuth --tests`: `Finished`, 0 errors, 0 warnings.
- `mbx clippy -p cells -p document -p rpc -p theuth --tests --no-deps -- -D warnings`: `Finished`, 0.
- `mbx test -p cells -p document -p rpc` (built `--no-run` first), twice: cells 103, document 89 (+ actor 12, cite 4, index_search 3, ingest 1+5, insights 2, replicate 3), rpc 82 (+4): all ok both runs.
- Environment note: `cells` `interp::tests::{nearest_venv…, venv_outside_home…, uv_pyenv_conda…}` and `shell::tests::resolve_prefers…` fail under load (their fake `--version` probes hit the 5 s `PROBE_TIMEOUT`): always under the default mbx test scheduler, once with `MBX_SCHEDULER_TESTS=0` during the fail-before run. They touch no E-cells code, pass run directly, and pass in both final runs (`MBX_SCHEDULER_TESTS=0`).
- Not tested: the `(ran on <me>)` note in `agent/src/supervisor.rs` (a one-line branch; the `agent` crate is outside the gate set).

## Contract changes after W0 (E-link)

None to a frozen shape; no TS-visible change (bindings not regenerated). Additions:
- `link::mesh_link_id(&secrets::Vault) -> Result<Option<String>, String>` (pub-only read) and `link::mint_mesh_link_id(&secrets::Vault, holder: bool) -> Result<String, String>` (the holder's mint), used by `MeshLinkId` and `rpc` `link_mesh_id` / `doc_link_create`.
- `mesh::store::{DocsSeen, Store::{docs_listed, docs_heard, docs_touch, docs_seen, removed_peers}}`.
- `net::Msg::DocsSchema` loses W0's `#[allow(dead_code)]`.

### Ambiguities decided (E-link)

- **Guest `MayRead` (tag 14)** needs no `api.rs` change: E-doc's actor `MayRead` (`document/src/notebook.rs:1042` marker) asks the server before answering, and the RPC callers count a refusal only after that answer, only for `not_member`/`viewer_readonly`/`sharing_disabled`.
- **The holder's mint** is `Vault::mesh_device_key` through `link::mint_mesh_link_id`, not `load_mesh_identity`: the holder's Link reconcile picks the new row up on `SecretsEvent::Changed` and registers the mesh uplink. "Holder" = `mesh::is_hub()`.
- **`link_mesh_id` from a forwarded `doc_link_create`** is sent with a fresh `Extensions`: it is a separate internal query the holder answers itself, not a second hop of the user's call.
- **v2's mesh link id** on a non-hub home with Link on is `LinkStatus.mesh` or else `MeshLinkId` (a non-hub's Link state never caches the mesh identity).
- **A `homes_on` flip is not a `MeshEvent`** (a new variant is TS-visible): E-doc's actor sees it on its next 30 s `DocsGate` poll or the next `MeshEvent`. §15.3 check 17 ("within 20 s") may need E-doc to poll faster, or a closer's call.
- **The gate pass loop** (5 s tick plus pokes) is skipped on an engine with no paired peer; `DocsGate` and `mesh_status` compute the gate per ask (a lone engine: on, nothing sent).

## Closer (Z184)

### Merge

`feat/184-cells` (0bdf78be), `feat/184-link` (92649055), `feat/184-ui` (4eaa7175) merged `--no-ff` in that order. The only conflicts were this file's appended sections (kept all). `origin/main` is 06ccf506, the branch's base: nothing to merge. The merged tree checked clean with no reconcile edits; bindings regenerated (no TS-visible change), tsc 0.

### Seams

- **`Relayed(CodeBlockText)`** routes with the actor's `relaying` flag; a relayed read this engine does not serve is refused (`NotHub:<server>`) instead of answered from the local replica, so the caller falls back to its own replica. Test `note_cells::tests::a_relayed_read_off_the_server_is_refused`.
- **`Park`** in `CodeBlockText` is E-doc's re-send park: the read waits for the claim and reads the claimed Doc. Test `relay::tests::a_block_read_during_a_claim_reads_the_claimed_doc`.
- **The fallback `Open`** to the holder rides E-doc's engine-keyed legs (`forward` replaces a `(Conn(None), doc)` leg that points at another engine); nothing to change. Test `note_cells::tests::the_fallback_open_reaches_the_holder_past_a_silent_home` (home silent on `Block`, the holder's `Open` answers on a leg of its own).
- **`homes_on` flips** are published on the engine bus as the Rust-only `mesh::DocsGateState` by the mesh's gate-pass loop (only on a change); the document actor subscribes. `poll_gate` answers take the same path. An "off" applies only if it still holds after `GATE_OFF_SETTLE` (1 s): when the holder's connection drops, the gate reads off for the milliseconds before the lease moves, and applied at once that blip unloaded every homed note at its own home (found in check 12). Test `relay::tests::a_published_gate_flip_reroutes_and_a_blip_does_not`.
- **`doc_move_home`** reaches E-doc's real `MoveHome` (W0's stub went with the E-doc merge); exercised live (check 14).
- **Move to copy**: `doc_move_home` swaps the engine id in the actor's refusal for the machine name from `mesh_status` (`self_name` / `peers[].name`, else the id's first 8 characters). Test `api::tests::move_refusals_name_the_machine`.
- **`NOT_BUILT`** is gone (its last use was a test fake's refusal text); every `184: E-…`/`184: U` marker is gone.
- **U's assumptions**: `mesh_link_unavailable`, `busy` and `not_member` match the engine's strings exactly (`link::MESH_LINK_UNAVAILABLE`, `share::BUSY`, the `not_member` refusal/close reason). The unknown-author confirm now reads a typed field: `cells_err` puts `member` (the pub8, `"?"` when unknown) in `data` beside `reason`, and `NoteCell` matches `data.member === '?'` instead of the message text. Test `rpc cells::tests::a_member_ack_names_its_member_in_data`.
- **Meeting test**: `meeting/tests/calendar.rs fastmail_floating_time_is_local_and_unknown_hosts_are_not_links` — `crates/meeting` differs from main 06ccf506 by one line (`home: Homing::Here` in `note.rs`), and the test and the calendar code are byte-identical, so it fails on main too (a local-time/DST expectation); 184 did not break it.

### Contract changes after W0 (Z184)

- **No mesh migration 0008.** `peer_docs_seen` is created by `Store::open` (`CREATE TABLE IF NOT EXISTS`), and the migrator ignores applied versions it does not know. A numbered migration made an engine rolled back to a pre-184 build refuse its mesh database (sqlx `VersionMissing`), so §8.3 rollback (check 17) could not run. Supersedes "Migrations" above and §9.3's file name.
- `DocStatus` (UI) `{ kind: 'connecting', resync?: boolean }`: a re-sync after a server move keeps the editor editable.
- `cells_err` data gains `member` for `member_ack` (data is untyped JSON; no binding).
- `mesh::DocsGateState` is now also a bus event (Rust-only, never bridged).

### Defects found live and fixed (each with a test)

1. Two control connections up at once (a pairing's and the connector's, or a restart) marked a 184 peer silent after 5 s and turned homes off until a reconnect → a frame counts while any connection that carried one lives (`net::tests::two_live_control_connections_are_not_a_silent_peer`, failed before).
2. The editor went read-only while a server-move re-sync ran, dropping the caret and the keystrokes (508 of 600 in check 4) → editable through `connecting{resync}` (`provider.test.ts` "Closed{…} re-syncs, keeps what is typed meanwhile").
3. A meta-only write (a merged fence or home) re-published the engine's older `snapshot.yjs` over a newer one in the mirror, so A's replica stuck at 439/600 tokens → `write_meta` mirrors `meta.json` alone (`store::tests::a_meta_write_never_republishes_an_older_snapshot`).
4. A fence that beats a server's claim at an equal epoch with a greater id went unnoticed → the merge's tie rule (`relay::tests::an_equal_epoch_fence_from_a_greater_id_flags_the_second_server`).
5. The gate blip (Seams above). 6. Migration 0008 (contract changes above).

### Calls (ambiguities, simplest option that keeps the design)

- **Link relay URL**: the brief's `http://127.0.0.1:18591` for `link.relay` fails the Link uplink ("URL scheme not supported"); Link takes `ws://127.0.0.1:18591` (same loopback relay; share URLs read `http://127.0.0.1:18591/d#…`). `mesh.relay` stays `http://127.0.0.1:18591`.
- **Fake models** on 18760-18762 (the hard limit), not the brief's 1847x third ports, which no engine needs.
- **Documents helper**: the scratch `t` needs `theuth-documents-59.5` beside it; built with `bun run scripts/build-helper.ts` in `packages/documents` (a `bun build --compile` of the helper, through build-lock's light pool; not a UI bundle), copied beside the scratch binaries.
- **v2 needs a mesh link id** (§7.1 "both ids exist"); a mesh without phones has none until the first Link-off share (or a phone) mints it, so check 13 ran before check 9 was repeated.
- **Partition without sudo**: a debug-only, loopback-only fault switch in the mesh (`THEUTH_DEV_MESH_DROP_FILE`, honoured only when `cfg!(debug_assertions)` and `mesh.bind` is loopback; release builds never read it).
- **Check 14 meeting note** not run live: the meeting plugin stays off on the scratch engines (it reaches the Mac's audio and calendar).
- **Check 16 ordering**: homes already existed when it ran (after 17, C on the old build).
- **Scratch databases**: the dev-only 0008 rows the earlier scratch builds wrote were deleted once before the rollback checks.

### HANDOFF

- **Lease warm-up after a restart** (mesh, pre-184): a restarted engine with stored peers holds "no holder" until its warm deadline (`mesh.lapse_secs`, 30 s) — no gossip heartbeat reached it sooner in the rig — and the docs gate is fail-closed without a holder, so a restarted home serves its notes ~30 s after start (check 3: 35.5 s, not ≤10 s; a Move to inside that window is refused "notes cannot move until every machine runs this version"). Owner: mesh lease (67 §4.3 / 182 D5).
- iroh binds an extra IPv6 wildcard UDP socket (`*:<port>`) beside `mesh.bind` (seen with lsof on the scratch engines); pre-184, worth a look for loopback-only rigs.

### Gates (Z184)

- `mbx check -p document -p cells -p rpc -p mesh -p link -p tasks -p meeting -p dreaming -p autoresearch -p agent -p theuth --tests`: `Finished`, 0 errors, 0 warnings.
- `mbx clippy` (same set) `--tests --no-deps -- -D warnings`: `Finished`, 0.
- `mbx test` on the set without `agent` (built `--no-run` first), twice, `MBX_SCHEDULER_TESTS=0`: 1110 passed, 1 failed both runs — the meeting calendar test above (pre-existing). Unit binaries: autoresearch 304, cells 103, document 104 (+ actor 12, cite 4, index_search 3, ingest 1+5, insights 2, replicate 3), dreaming 64, link 32, meeting 165 (+ note 1, oracle 1, calendar 15/16), mesh 70, rpc 86 (+4), tasks 119, theuth 12.
- `mbx test -p agent --lib`, `/opt/homebrew/bin` off PATH, `target/debug/t` fresh from this head: 807 passed, 0 failed, 4 ignored.
- `bunx tsc -b --noEmit`: 0 errors. `bunx vitest run` (full): 319 files, 2119 tests passed.
- Live proof: `/tmp/rs184/proof/evidence.txt` (19 lines), screenshots in `/tmp/rs184/proof/shots/`.

### Z184-R1 (origin/main eb2a2b3a, design 185)

- Merge: `git merge --no-ff origin/main` auto-merged Cargo.lock, agent/src/supervisor.rs, document/src/lib.rs and rpc/src/api.rs with no conflict hunks. 185's only edits there sit beside 184's: the new `replica::files` policy argument (`None`) at the skills and doc_files watchers, plus its own rpc methods. Dreaming (lib/night/signals) does not touch journal.rs.
- Bindings regenerated (export 8/8), tsc 0. check and clippy (full set incl. agent, `--tests`, `-D warnings`): 0. Suites once: 1115 passed, 1 failed (the pre-existing meeting calendar test). agent `--lib` with a fresh `t`: 807 passed, 0 failed. vitest (full): 319 files, 2120 passed.
- Live smoke on the merged build: check 1 (`/tmp/rs184/proof/r1-c1.txt`, copies on A after 959/1142/1072 ms, 0 NotHub), check 4 (`r1-c4.txt`: 600/600 on A, B, C, md5 equal; the 45 s Move again refused by B's lease warm-up; `r1-c4b.txt`: Move to C and back while typing, 300/300, md5 equal), check 6 (`r1-c6.txt`: B's HOME path, run row on B only) — all PASS.

### Z184-R2 (the engine, UI and security reviews of 43a74428)

MUSTs, each with a fail-before test:

- **Catch-all writes on every open** (engine F1, UI 1). Client (`provider.ts`): the remainder is pushed only when it carries structs or something was owed (queue, `#unconfirmed` or the lost list non-empty), never by a viewer or for a `source`. Engine (`actor.rs` `apply`): a client update that changes neither the Doc's state vector nor its delete set returns before the log, `updated_ms`, export and fan-out; helper updates always go through (an ingest refresh completes through them), and an update whose first struct per client is at or past the Doc's clock (every keystroke) skips the check. Tests: `provider.test.ts` "a fresh open of a note with a deletion pushes nothing", "a viewer of a note with a deletion ends readonly and never writes", "a lost delete-only update is still repaired by the catch-all"; `relay::tests::an_update_that_changes_nothing_is_not_applied`.
- **Keystrokes stranded behind the catch-all** (UI 2): the drain loop runs again after the catch-all and the status turns live only with the queue empty. Test: "keystrokes typed while the catch-all is in flight all arrive".
- **Revoke across engines** (security): `DocMeta.revoked` (pub → ms, omitted when empty) is a tombstone. `member_revoke` writes it, `merge_meta` unions it (greatest wins) and drops members not added after it, `admit` refuses a member no later than it, and a Join adds the member after it and clears it. MayRead asks the server whenever the note is served elsewhere and the local answer is Ok or `not_member` (a local `sharing_disabled` / `viewer_readonly` stands). Tests: `relay::tests::a_guest_revoked_on_the_server_is_refused_through_another_engine`, `mirror::tests::a_revoked_member_stays_revoked_until_it_joins_again`, the rejoin tail of `tests/actor.rs` `sharing_links_join_roles_demote_revoke`.

SHOULDs, all applied: the mint gate (`mint_as_holder`: hub, synced and the lease holder, then the id read back; `mesh_link_id` maps a failure to `MESH_LINK_UNAVAILABLE`); `cell_latest` blanks `engine` for guests; `join_outcome` counts only `not_member`, `viewer_readonly`, `sharing_disabled` and `bad_proof` (test extended) and the guest page re-joins on `hub_moved` / `server_moved` / `NotHub`; the author lookup asks the holder when homes are on even with no members; `claim` keeps the fence (no epoch bump, no meta.json rewrite) when it already names this engine and nobody is reachable, and `hold` marks the meta dirty only on a change; the silence timer and `docs_frame` serialise on one `tokio::sync::Mutex`; the `peer_docs_seen` migration caveat comment; `Rehome` scans once per tombstone set (`rehomed_for`); Move to's refusal is `aria-describedby`, the unsure-run confirm takes focus and Escape, the chip truncates with the full name as its title. No tests for the mint gate (needs a live mesh lease; exercised live, check 9) or the silence race (a timing window).

Found while gating or live, fixed:

- `ingest_job` `pasted_text_runs_to_completed_and_refresh_tracks_freshness` failed in 3 of 3 full runs (1-in-4 alone): the insight pass's whole-row `source_upsert` put back the status it had read over a refresh's `queued`. It now writes only `insights` and `meta` (`Store::source_set_insight`); 0 of 16 after.
- Live R2-b: an engine restarted under the page answers the first sync `NotHub{hub: null}` during its lease warm-up, the socket stays up, and the provider stayed offline with its queue for good. A failed sync with the socket up now retries every 1 s (test "a sync refused while the socket stays up … retries and delivers").
- `Cmd::Handover` boxes its `DocMeta` (clippy `large_enum_variant` once `revoked` grew it).

Gates: check and clippy (full set incl. agent, `--tests`, `-D warnings`): 0. Suites twice: 1118 passed, 1 failed both runs (the pre-existing meeting calendar test). agent `--lib` with a fresh `t`: 807 passed, 0 failed, 4 ignored. tsc 0. vitest (full): 320 files, 2129 passed. Bindings regenerated (`DocMeta` gains the optional `revoked`; the bindings are generated, not tracked). Live: checks 2, 4, 9, 10, 19 and R2-a…d PASS (`/tmp/rs184/proof/evidence.txt`, Z184-R2 section); the Move inside B's restart warm-up is still refused (HANDOFF above).

### Z184-R3 (re-checks of 1c7cc41e)

- **MUST (UI):** `UnsureRun` focuses Cancel, never Run (a second Enter after the user's own press must not run unread text); Escape cancels and focus returns as before. `UnsureRun.test.ts` asserts Cancel.
- **Revoke blocks the member's links:** `join` refuses a key whose tombstone is not older than the presented link's `created_ms` (`JoinError::MemberRevoked`, wire `member_revoked`, final on the guest page: "Your access was removed"); a link made after the revoke admits the key. The join no longer removes the tombstone (the merge would bring it back anyway; keeping it keeps every older link closed). `tests/actor.rs` `sharing_links_join_roles_demote_revoke`: the old link is refused, a new one admits. ShareDialog's Remove opens `RemoveMember`: "<name> can no longer join through any link made so far. Without revoking link «X», anyone else holding it can still join." with "Also revoke link «X»" (on by default) when the member's `via` link is live (`RemoveMember.test.ts`).
- `join_outcome` counts the join's own refusals (`link_unknown`, `link_revoked`, `link_expired`, `members_full`, `sharing_off`, `member_revoked`) with `bad_proof` and the member-call refusals; the guest page already ends on every non-move join error.
- Provider: the sync retry is 1 s for ten tries, then 5–10 s (jittered); `#owed` is a field cleared only once a catch-all ran without error, so a lost delete-only update survives a failed catch-all push (test "… repaired even when the first catch-all push fails", fail-before confirmed).
- `Rehome` records the tombstone set as scanned only when every `write_home` succeeded.
- Gates: clippy (full set, `--tests`, `-D warnings`): 0. Suites once: 1118 passed, 1 failed (the pre-existing meeting calendar test). tsc 0. vitest (full): 321 files, 2132 passed. Live: R2-c again plus a re-join through the old, still-live link after a member-only revoke — refused (`/tmp/rs184/proof/evidence.txt`, Z184-R3).
