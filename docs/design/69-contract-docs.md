# 69 — contract, documents (worker C2, 2026-09-25)

*The names M (mirror + migration bodies), Y (hub forwarding runtime) and Z2 (smoke) build on. Written once on branch `feature/69-docs` (cut from `feature/69-devices`, contract `69-contract.md`); not changed after C2's commit 4. Design: `59-documents.md`, `69-devices.md` §5. Line numbers are as of that commit.*

Rulings this contract implements: documents **replicate** (every engine holds every document's state); the yrs server is a **hub duty** (one engine serves editors, the lease says which); share links **name the mesh** (a device dials `/h/<mesh device id>`, whoever the hub is).

## 1. Ids

| What | Where | Rule |
|---|---|---|
| `DocId::for_binding(&Binding) -> DocId` | `crates/document/src/meta.rs:29` | uuid v5 (namespace `DocId::NAMESPACE` = `7a6e7c1e-59d0-4d0c-9c2b-1f3a0d0c5e69`, name = `Binding::key()` = the canonical JSON `{"knowledge":"facts/foo"}`) rendered as the usual 26 lowercase base32 chars. `Binding::Note` (no natural key) stays uuid v7 (`DocId::new`). Every engine mints the same id for the same knowledge concept / artifact / meeting note without talking. |
| Users | `actor.rs` `Ensure` (`:1026`), `create_doc` (`:1476`) | Nothing else mints ids. |
| One-time migration | `store.rs` `Store::migrate_ids` (`:126`, run by `Store::open`), `Store::rename` (`:168`) | For every live non-note row: several rows per binding → keep the newest `updated_ms`, load the others (snapshot + log), `apply_update` their full state into it, snapshot, trash them (`.trash/<id>`, row soft-deleted), one `info` line per merge; the kept row (or a lone one) whose id ≠ `for_binding` is **moved**: directory renamed, `meta.json` `id` (and `derived` for artifacts — `documents/<id>/artifact.md` moves with the dir) rewritten, index row `id` updated, mirror files rewritten, one `info` line per move. A tombstone already under the target id yields (its row is deleted locally). Idempotent. |
| Known ceiling | | Two engines that each imported the *same* concept before this branch hold two independent Yjs histories; the first mirror merge joins them → duplicated blocks once. Fix by hand or `Restore`; not automated. |

## 2. Tables

### `documents/index.sqlite` `docs` (`store.rs:33`)
```sql
CREATE TABLE docs (id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, binding TEXT NOT NULL,
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL, bytes INTEGER NOT NULL DEFAULT 0,
  members_n INTEGER NOT NULL DEFAULT 0, deleted_ms INTEGER, hlc TEXT);   -- hlc added by ALTER on old files
```
- Replicated: `store::TABLES = ["docs"]` (`store.rs:42`), attached in `DocumentPlugin::start` via `lookup_when::<replica::Replica>` → `replica::attach(store.pool(), TABLES)` then `announce_unstamped()` so pre-existing rows ship (`lib.rs:207`); `Err` → local only.
- **Soft delete only**: `Store::delete` sets `deleted_ms` and moves the dir to `.trash/<id>`; `sweep_trash` removes old `.trash` dirs and **never deletes the row** (a delete loses to any newer column write elsewhere). `Store::create` upserts (`ON CONFLICT (id) … deleted_ms = NULL`): recreating a deleted binding revives the same deterministic row.
- `Store::lookup` never marks a row deleted any more; a row whose directory is not here (replicated ahead of its mirror files) answers `None` with a warning.
- **Write cadence**: `Store::touch` is gone; `Store::touch_at_snapshot(id, updated_ms, bytes)` (`store.rs:398`) runs from the snapshot path only, `write_meta` (title, `updated_ms`, `members_n`) from exports and member changes. The per-update touch at `actor.rs` `apply` is removed: typing no longer produces a replication batch every 200 ms. **`bytes` lags** by up to one log's worth (≤ 1 MiB / 1000 records / 2 s quiet) — `doc_list` shows the last snapshot's size.

### `documents/mirror.sqlite` `doc_files` (replica's `files::mirror` shape)
`(path TEXT PK, bytes BLOB, mtime_ms, deleted INT, hlc TEXT)`, one row per file of `documents/mirror/`; created and driven by `replica::files::mirror_at(store.root().join("mirror.sqlite"), store.mirror_dir(), "doc_files", 60 s)` spawned from `DocumentPlugin::start` (`lib.rs:280`; the disclosed replica one-liner). A separate sqlite so the blob table never sits beside the index.

## 3. Mirror layout and merge

### 3.1 Layout (`crates/document/src/mirror.rs`, `store.rs` `MIRROR`)
`<data_dir>/documents/mirror/` — **flat**, two files per document:
```
<id>.meta.json       = documents/<id>/meta.json, byte for byte
<id>.snapshot.yjs    = documents/<id>/snapshot.yjs, byte for byte
```
Never mirrored: `updates.log` (fsync per update, truncated at each snapshot), `snapshots/` (history), `index.sqlite`, `.trash/`. `replica::files::mirror` is flat, non-recursive and LWW on whole bytes, so it must never see `documents/` itself.

Writers: `Store::mirror_write(id)` (`store.rs:265`) — called by `create`, `write_meta`, `snapshot`, `rename`; writes each file that exists via a `.`-prefixed temp + rename (`write_hidden`; the replica's scan skips dot files, so a half copy is never a row) and records `sha256` per file name in `Store::projected`. `Store::delete` removes both mirror files (replicates as `deleted = 1` rows). `Store::last_projected(id) -> Option<String>` = the snapshot sha this process last wrote; `Store::is_own_projection(name, bytes)`.

### 3.2 Merge-on-apply
`mirror::start(mirror_dir, actor)` watches the directory (non-recursive, 300 ms debounce per path) and tells the actor `MirrorChanged { path }` (`actor.rs:1882`). The actor:
1. own write (`is_own_projection`) → nothing;
2. off the hub with the document loaded (a `doc_read` cache) → `unload` first;
3. document **open** (hub only): `<id>.snapshot.yjs` → `store::diff_from(bytes, doc.state_vector())` (the peer's state minus ours as one v1 update; `[0,0]` = nothing) → `apply()` as `Principal::File` (log, fan-out to subscribers as `Who::File`, helper mirror) + `schedule_export`; `<id>.meta.json` → `mirror::merge_meta` into `o.meta`, `write_meta`, `DocEvent::Members` + `DocEvent::Meta`, `DocumentChanged`;
4. document **closed** → `Store::mirror_apply(path) -> Applied` (`store.rs:305`): `Ignored` (own / removed) | `Unchanged` (nothing new) | `Merged(id)` (snapshot: load → `merge_into` → re-snapshot + `touch_at_snapshot`; meta: `merge_meta` → `write_meta`) | `Created(id)` (no directory here: made from **both** mirror files; the `docs` row comes by replication, never written here) | `Deferred(id)` (one half not projected yet; the other's arrival retries). A removed `<id>.meta.json` whose row is a tombstone moves the local directory to `.trash`.

**Never overwrite**: foreign bytes never replace `documents/<id>/snapshot.yjs`; a snapshot is a Yjs state and states join. Re-snapshot only when the join changed the encoded state (`merge_into` compares before/after), so A → B → A converges in one round and cannot ping-pong.

**`meta.json` merge rule** (`mirror::merge_meta(local, incoming) -> changed`, `mirror.rs:107`):
- scalars (`title`, `kind`, `binding`, `derived`, `fragment`, `agent`, `created_ms`) from the side with the newer `updated_ms`; `updated_ms` = max;
- `members`: union by `pub`; both → greater `max(added_ms, last_seen_ms)`, ties local;
- `links`: union by `id`; both → a set `revoked_ms` wins (monotone), else greater `created_ms`, ties local; an empty `secret_hash` never replaces a set one;
- `authors`: union; both → the newer-`updated_ms` side;
- `last_export` stays local (this engine's derived file).

**Security, stated plainly**: `ShareLink.secret_hash` — the HMAC key of the §8.3 join proof — now sits in **plaintext `meta.json` on every engine of the mesh** (in `documents/<id>/meta.json`, `documents/mirror/<id>.meta.json` and the `doc_files` blob). Before 69 it never left the one engine. The mesh data path is Noise/QUIC; the files at rest are as protected as the rest of `<data_dir>`.

### 3.3 Quiet snapshot
`documents.snapshot_quiet_ms` (default `2000`, `lib.rs` `DocumentsConfig`): `Tick` snapshots an open document with `log_count > 0` once `idle_since` is that old — besides `LOG_MAX_BYTES` (1 MiB), `LOG_MAX_COUNT` (1000) and unload. Replicas lag by seconds, not by a megabyte.

## 4. Hub gate (`crates/document/src/hub.rs`)

| Name | Semantics |
|---|---|
| `hub::Hub { follower: mesh::lease::Follower, holder, watcher: Option<CancellationToken> }` (`DocumentActor::hub`) | `Follower::default()` = holding (no mesh → hub). Fed by `hub::SelfId(EngineId)` (plugin start: `lookup_when::<mesh::Mesh>` → `Status.self_id`) and `mesh::MeshEvent::Lease` (the actor is `rt.subscribe`d). `holder: None` = loss once the self id is known. |
| `DocumentActor::sync_hub()` | Slack's `sync_socket`: gain → `watch::start(data_dir, actor, token)` (the derived-file watcher now takes a `CancellationToken`); loss → cancel the watcher, fire every pending export, `unload` every open document (snapshot when the log is non-empty → mirror), subscribers get `DocEvent::Closed { reason: "hub_moved" }` (`hub::HUB_MOVED`). `hub::Reconcile` runs it at start. |
| **Hub-only** | `OpenDoc`, `ApplyUpdate`, `AwarenessUpdateMsg`, `Subscribe`, `Join`, `LinkCreate`, `LinkRevoke`, `MemberSet`, `MemberRevoke`, `SetName`, `MembersOf`, `Ensure`, `ImportMarkdown`, `Edit`, `UndoLastAgentEdit`, `CreateNote`, `Create`, `Rename`, `DeleteNote`, `Delete`, `Snapshot`, `SetDerived`, `Restore`, `FileChanged` (watcher imports) and `schedule_export` (every export). Rule: **every write is hub-only**. |
| **Anywhere** | `Lookup`, `ListDocs`, `Snapshots`, `Outline`, `Markdown` (`doc_read`), `doc_list`, `doc_resolve` (its `Ensure` fallback is refused off the hub), the tools' reads — from the local replica. |
| `hub::DocError` | `NotHub { hub: Option<EngineId> }` \| `Other(String)`; `Display` = `NotHub:<engine id>` (empty id = nobody holds the lease) / the message; `DocError::parse(&str)` reads it back; `From<String>`, `From<&str>`, `Into<String>`. Reply types: `OpenDoc`, `ApplyUpdate`, `AwarenessUpdateMsg`, `Subscribe`, `LinkCreate`, `LinkRevoke`, `MemberSet`, `MemberRevoke`, `SetName`, `MembersOf` → `Result<_, DocError>`; `Join` → `JoinError::NotHub(Option<EngineId>)` (same `Display`); the owner-plugin surface (`Ensure`, `Edit`, `Snapshot`, …) keeps `String` and answers the `Display` form (`Hub::gate_str`). |
| RPC | `crates/rpc/src/api.rs` `NOT_HUB = -32011` (`:1625`), `doc_send` (`:1631`): every `doc_*` handler goes through it; a `NotHub` (typed or parsed from the `String`) → `{"code": -32011, "message": "another engine holds the hub lease", "data": {"NotHub": {"hub": "<engine id>" | null}}}` (serde external tagging, like `OwnerUnreachable` `-32010`). Y turns it into forwarding over `theuth/docs/1`; until then the client sees the error. |

Deviation from the brief: `hub` is `Option<EngineId>`, not `EngineId` — `holder: None` is a loss with no hub to forward to, and the error must say so.

## 5. Relay stub (`crates/document/src/relay.rs`, `replica::ALPN_DOCS = "theuth/docs/1"`)
`ALPN_DOCS` sits beside the other `ALPN_*` in `crates/replica/src/lib.rs:55` and is in mesh `STREAM_ALPNS` (`crates/mesh/src/net.rs:132`, now 7). `relay::Msg` (postcard; `LengthDelimitedCodec` frames of tag byte + body; `relay::MAX_FRAME = 64 MiB`; `relay::framed(stream)`):

| tag | variant | direction | payload |
|---|---|---|---|
| 0 | `Open { doc, sv: Option<String> }` | front door → hub | first frame; answered by one `Event` carrying the `DocOpen` JSON |
| 1 | `Update { doc, bytes }` | → hub | base64 v1 update as UTF-8 (what `doc_update` carries) |
| 2 | `Awareness { doc, bytes }` | → hub | base64 awareness update |
| 3 | `Subscribe { doc }` | → hub | first frame of the event leg |
| 4 | `Event { doc, bytes }` | hub → | a `DocEvent` as JSON (what `doc_subscribe` serialises) |
| 5 | `Join { doc, guest_pub: [u8;32], handshake_hash, proof, name }` | → hub | the guest's `doc_join` for the connection behind this stream |
| 6 | `Closed { doc, reason }` | hub → | `DocEvent::Closed` reasons incl. `hub_moved`, or an error code |

No runtime behind it. Y's `forward.rs` streaming leg: one stream per (client conn, doc) — `forward.rs` is unary, `doc_subscribe` needs this.

## 6. Share URL (`crates/rpc/src/api.rs` `doc_link_create`, `:3055`)
`https://<relay origin>/d#1.<id>.<doc>.<secret>` where `<id>` = `LinkStatus.mesh.device_id` when present (the mesh identity; the hub — whichever engine — answers `/h/<mesh device id>`), else `harness_id` (a single engine before any device paired). Link must be `enabled`. Old engine-id links keep resolving: the engine uplink stays registered (69 decision 1). Guests arriving on the mesh uplink reach the hub, which is the only engine serving `doc_join` — no forwarding needed for them.

`link::DropGuest { key }`: `LinkActor` cancels every conn in `State::conns` with that static (`crates/link/src/lib.rs:357`); K registers mesh device conns in the same map, so one pass covers both uplinks (no code change; the comment names the contract). If K keeps a separate map, K extends that loop.

## 7. Config
`documents.snapshot_quiet_ms` (default 2000). No other keys.

## 8. Allow-lists
- **M**: `crates/document/src/{store.rs (mirror + migration bodies), mirror.rs}` + replica one-liners disclosed.
- **Y**: `crates/document/src/{hub.rs body, relay.rs runtime, actor.rs (gate hooks, forward client)}`, `crates/rpc/src/{api.rs doc_* handlers, forward.rs streaming leg}`, `crates/link/src/conn.rs` (guest path on the mesh uplink).
- **Z2**: smoke + fixes anywhere, each disclosed.
- Nobody changes the names in §1–§7 after C2's commit 4; additions go beside them.
