# 184 — W0 contract: notes on any machine

Status: FROZEN at the W0 commit on `feat/184-notes-any-machine` (from main 06ccf506). Design: `184-notes-any-machine.md` §14 (frozen contracts) and §15 (waves, ownership). This file is §14 as Rust and TS signatures plus the seams between the four W1 slices. A change to anything here needs the contract owner. Where each shape lives (file:line), the migrations and every deviation from §14 are in `184-notes.md` "Contract (W0)".

W0 is a compiling skeleton with no behaviour change: every new path answers today's behaviour or `"not built yet"` (`document::NOT_BUILT`), and every place a slice fills in carries a one-line marker `// 184: E-doc`, `// 184: E-cells`, `// 184: E-link` or `// 184: U` by §15.2 ownership (`rg '184: E-doc'` lists E-doc's work).

## 1. Document data (`crates/document`)

```rust
// meta.rs — meta.json and the wire DocMeta of doc_open (§9.1). Old files decode: all serde(default).
pub struct DocMeta {
    /* … unchanged fields … */
    #[serde(default)] pub home: Option<mesh::EngineId>,      // None = unhomed, hub-served as today
    #[serde(default)] pub home_ms: i64,                      // home's own clock (W-a)
    #[serde(default)] pub serve_epoch: i64,                  // fence (§4.4)
    #[serde(default)] pub served_by: Option<mesh::EngineId>,
}
impl DocKind {
    pub fn homed(self) -> bool;   // Note | Board | MeetingNote | TaskThread
    pub fn movable(self) -> bool; // Note only
}

// mirror.rs — merge: greater (home_ms, home) wins; greater (serve_epoch, served_by) wins;
// on a link present on both sides the seal that `opens` wins (§7.4). Tests pass `&|_| false`.
pub fn merge_meta(local: &mut DocMeta, incoming: &DocMeta, opens: &dyn Fn(&ShareLink) -> bool) -> bool;

// meta.rs — doc_open reply (§10.3)
pub struct DocOpen {
    pub meta: DocMeta, pub state: Vec<String>, pub awareness: String,
    #[serde(default)] pub server: Option<mesh::EngineId>,
    #[serde(default)] pub dual: Option<mesh::EngineId>,
    #[serde(default)] pub sv: String,          // server's base64 v1 state vector; "" = old server, no push
}
pub enum DocEvent { /* … */
    Server { server: mesh::EngineId, dual: Option<mesh::EngineId> }, // {"type":"Server",…}, sent when dual changes
    /* Closed { reason } gains "server_moved" */
}
#[serde(tag = "type")] pub struct CellLatestChanged { pub doc: DocId } // bus, bridged to subscribe_events

// meta.rs — doc_link_create reply
pub struct LinkCreated { pub url: String, pub link: ShareLink, #[serde(default)] pub via: LinkVia }
#[serde(rename_all = "snake_case")] pub enum LinkVia { #[default] Home, Mesh } // "home" | "mesh"
```

## 2. Routing and actor messages (`crates/document`)

```rust
// actor.rs — §5.1
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Homing { #[default] Here, Hub, Engine(mesh::EngineId) }
pub struct CreateNote { pub title: String, pub markdown: String, pub by: Principal, pub home: Homing }
pub struct Create { pub binding: Binding, pub title: String, pub markdown: String, pub by: Principal, pub home: Homing }
// CreateBoard unchanged: always Homing::Here.

// hub.rs — §4.2, §4.6, §5.2
pub enum Route { Serve, Park, Forward(mesh::EngineId) }
impl DocumentActor {
    pub fn route_doc(&mut self, doc: &DocId, relayed: bool) -> Result<Route, DocError>; // may start a claim
    pub fn serves(&self, doc: &DocId) -> bool;                                        // never starts one
    pub fn server_of(&self, doc: &DocId) -> Result<Option<EngineId>, DocError>;       // never starts one; None = here
    pub fn gate_doc(&mut self, doc: &DocId, relayed: bool) -> Result<Route, DocError>; // Serve | Park; Forward(e) → Err(NotHub{Some(e)})
    pub fn route_home(&self, home: Option<&EngineId>) -> Result<Option<EngineId>, DocError>; // None = create here
}

// hub.rs — actor messages (§10.2). All `ask`.
pub struct Relayed<M>(pub M);                                     // reply = M's reply; generic impl for every M
pub struct Handback { pub doc: DocId, pub sv: Vec<u8>, pub from: EngineId }      // → Result<HandbackReply, DocError>
pub struct Handover { pub doc: DocId, pub meta: DocMeta, pub from: EngineId }   // → Result<i64, String>
pub struct MoveHome { pub doc: DocId, pub to: EngineId, pub checks: bool }      // → Result<DocSummary, String>
pub struct DocServer { pub doc: DocId }                                         // → Result<Option<EngineId>, DocError>

// cells.rs — §6.2
pub struct CodeBlockText { pub doc: DocId, pub key: NoteKey, pub seen: Option<String> } // → Result<CodeBlockTextReply, String>
pub struct CodeBlockTextReply { pub block: String, pub language: String, pub text: String,
    pub last_author: Principal, #[serde(default)] pub markdown: String }

// hub.rs — constants (§9.4)
pub const SERVER_MOVED: &str = "server_moved";
pub const NOT_HOME: &str = "not_home";
pub const NOT_OWNER: &str = "not_owner";     // HUB_MOVED = "hub_moved" unchanged
pub const HANDBACK_TIMEOUT_MS: u64 = 5000;
pub const JOIN_PARK_MS: u64 = 2000;
pub const BLOCK_WAIT_MS: u64 = 1000;
pub const PENDING_HOME_TTL: Duration = Duration::from_secs(600);
pub const NOT_BUILT: &str = "not built yet"; // W0 only; a slice deletes its uses

// share.rs — §4.3 step 4
pub const BUSY: &str = "busy";
impl JoinError { pub fn retryable(&self) -> bool; } // Other("busy")
```

`DocError::NotHub{hub}` and `-32011` keep their names; the id now names the document's server (W-b).

## 3. Docs wire `theuth/docs/1` (`crates/document/src/relay.rs`)

Appended to `Msg`; tags 0-9 unchanged. Each reply is one `Msg::Event` whose bytes are the reply JSON, or `Msg::Closed{reason}`.

| Tag | `Msg` | Reply JSON | Member stream |
|---|---|---|---|
| 10 | `Create { req: Vec<u8> /* CreateReq JSON */ }` | `Result<DocSummary, String>` (`Notebook`: the notebook summary) | `Closed{not_owner}` |
| 11 | `Handback { doc: String, sv: Vec<u8> }` | `Result<HandbackReply, String>` | `Closed{not_owner}` |
| 12 | `Block { doc: String, key: Vec<u8> /* NoteKey JSON */, seen: Option<String> }` | `Result<CodeBlockTextReply, String>` | `Closed{not_owner}` |
| 13 | `Handover { doc: String, meta: Vec<u8> /* DocMeta JSON */ }` | `Result<i64, String>` | `Closed{not_owner}` |
| 14 | `MayRead { doc: String }` | `Result<(), String>` | allowed |

```rust
pub enum CreateReq {
    Ensure { binding: Binding, markdown: String },
    Create { binding: Binding, title: String, markdown: String, by: Principal, home: Option<EngineId> },
    Notebook { title: String, description: String, project: Option<String>, by: Principal },
}
pub struct HandbackReply { pub update: String, pub meta: DocMeta,
    pub agents: BTreeMap<String, uuid::Uuid>, pub epoch: i64 }

pub enum Cmd { /* … */
    Create   { req: CreateReq, reply: ReplySender<Result<serde_json::Value, String>> },
    Handback { sv: Vec<u8>, reply: ReplySender<Result<HandbackReply, DocError>> },
    Block    { key: NoteKey, seen: Option<String>, reply: ReplySender<Result<CodeBlockTextReply, String>> },
    Handover { meta: DocMeta, reply: ReplySender<Result<i64, String>> },
    MayRead  { reply: ReplySender<Result<(), String>> },
}
```

W0: the front door maps every new `Cmd` onto its frame and parses its reply (`run_leg`), but no caller builds one, so nothing is sent. The hub side (`serve_stream`) answers tags 10-13 on a member stream with `Closed{not_owner}` and every new tag otherwise with `Closed{"not built yet"}`.

## 4. Cells (`crates/document`, `crates/cells`)

- `cell_latest` gains `payload TEXT NOT NULL DEFAULT ''` (whole `CellLatest` JSON, written whole). `CellLatest` gains `#[serde(default)] pub engine: String` (`""` = pre-184 row).
- Seam (E-cells, called by E-doc from `lib.rs`, already wired after `replica::attach`):

```rust
pub fn watch_latest(handle: replica::ReplicaHandle, rt: runtime::Runtime); // document::cells, spawns and returns
```

## 5. Mesh (`crates/mesh`)

```rust
// net.rs — appended to the control-connection Msg (JSON); an older engine ignores it
DocsSchema { schema: u32, homes_on: bool }
// lib.rs
pub const DOCS_SCHEMA: u32 = 2;
pub const DOCS_STALE_MS: u64 = 7 * 86_400_000;
pub struct DocsGate;      // ask → DocsGateState
pub struct RemovedPeers;  // ask → Vec<EngineId>
// types.rs
pub struct DocsGateState { pub floor_ok: bool, pub homes_on: bool }
pub struct MeshStatus { /* … */ #[serde(default)] pub docs_homes_on: bool }
```

Migration `crates/mesh/migrations/0008_peer_docs_seen.sql` (local, never replicated):
`peer_docs_seen(id TEXT PRIMARY KEY, docs_schema INTEGER NOT NULL DEFAULT 0, homes_on INTEGER NOT NULL DEFAULT 0, first_ms INTEGER NOT NULL, heard_ms INTEGER)`.

## 6. Link and RPC

```rust
// link/src/types.rs
pub struct MeshLinkId;                                   // ask → Result<String, String>; pub-only read, never mints
pub const MESH_LINK_UNAVAILABLE: &str = "mesh_link_unavailable";

// rpc/src/api.rs
#[method(name = "doc_move_home", with_extensions)]
async fn doc_move_home(&self, doc: DocId, to: String) -> RpcResult<DocSummary>; // owner only
#[method(name = "link_mesh_id", with_extensions)]
async fn link_mesh_id(&self) -> RpcResult<String>;       // mesh-internal (Forwarded only); mints on the holder only
```

## 7. TS (generated bindings plus hand-written UI types)

```ts
// bindings (crates/desktop/ui/src/rpc/bindings/)
type DocMeta = { …; home: EngineId | null; home_ms: number; serve_epoch: number; served_by: EngineId | null }
type DocOpen = { meta: DocMeta; state: string[]; awareness: string; server: EngineId | null; dual: EngineId | null; sv: string }
type DocEvent = … | { type: 'Server'; server: EngineId; dual: EngineId | null }
type CellLatestChanged = { type: 'CellLatestChanged'; doc: DocId }
type LinkCreated = { url: string; link: ShareLink; via: LinkVia }
type LinkVia = 'home' | 'mesh'
type CellLatest = { …; engine: string }
type MeshStatus = { …; docs_homes_on: boolean }
// rpc/api.ts
doc_move_home(doc: DocId, to: string): Promise<DocSummary>
link_mesh_id(): Promise<string>
// rpc/events.ts: BusEvent gains CellLatestChanged
// link/guest.ts
export const HOME_DIAL_MS = 5000
export type ShareTarget = { relayHost: string; secure: boolean; engineIds: string[]; doc: string; secret: Uint8Array<ArrayBuffer> }
```

## 8. Seams between slices

| Seam | Owner → user | Contract |
|---|---|---|
| `DocumentActor::route_doc`, `server_of` | E-doc → E-cells | §2; E-cells' `CodeBlockText` handler matches `route_doc(&doc, relayed)` and on `Forward(e)` sends `Cmd::Block` |
| `Cmd::Block` | E-doc (leg, server side) → E-cells | §3; front-door mapping and reply parse are built; the server-side handler is E-doc's in `serve_stream`, calling `Relayed(CodeBlockText{..})` |
| `document::cells::watch_latest(handle, rt)` | E-cells → E-doc | §4; already called from `lib.rs` after `replica::attach` |
| `Homing` on `CreateNote` / `Create` | E-doc → E-link | §2; `doc_create` sends `home: Here` (wired) |
| `MoveHome{doc, to, checks}` | E-doc → E-link | §2; `doc_move_home` sends `checks: true` (wired); E-link adds the forward to `DocServer` |
| `mesh::DocsGate`, `mesh::RemovedPeers` | E-link → E-doc | §5; W0 answers homes off and no removed peers |
| `link::MeshLinkId` | E-link (internal) | §6; W0 answers the cached mesh identity or `"not built yet"` |
| `LinkCreated.via`, `MeshStatus.docs_homes_on`, `DocEvent::Server`, `DocOpen.{server,dual,sv}`, `CellLatestChanged`, `CellLatest.engine` | engine → U | §7 |
