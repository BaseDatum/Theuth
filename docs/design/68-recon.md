# 68 recon: session store, replica, artifacts, device link, ownership blocks
Worktree `/Users/Sky.Moore/Develop/theuth-67` @ ecd050a (feature/67-agents). Read-only. Paths are relative to the repo unless they start with `~`.
**Tooling gap:** this subagent had no `shell`/`sqlite3` and no `write_file`. So A4 row counts, dbstat bytes and rows per day were NOT measured; the sizes below come from file headers and the KB fact of 2026-09-24. The report is delivered as this `show` artifact and in the reply, not at /tmp/68/recon.md.

## A. Session store (`crates/agent`)

### A1. Tables (22 migrations, `crates/agent/migrations/0001..0022`)
| table | PK | shape | columns updated after insert |
|---|---|---|---|
| sessions | `id TEXT` (0001:2) | **update-in-place heavy** | title (store.rs:309; 1176 with tags), pinned 319, archived 331, model 341, effort 351, agent+cwd 367, acp_session 378, harness_model 399, cwd 409, unattended 420, cut_at+context_revision+todo_version 768/776/871, updated_at+context_revision on every append 1792, context_revision 1821/1259, todo_version 1957, project 1538, scope/alias communication.rs:929, fork 1045. Deleted at 1470 |
| messages | `(session_id, seq)` composite (0001:13) | append, but: `excluded` UPDATE (remove_context store.rs:1235); tail DELETE on commit_cut (836) and delete_session (1450). **seq = MAX+1 in SQL (1781-1784), so seqs are reused after commit_cut** (comment 829-831). No timestamp column | excluded |
| epochs | `(session_id, n)` (0003) | append; excluded UPDATE 1247; DELETE on cut 843 | excluded |
| todo_snapshots | `(session_id, at_seq, k)` (0009) | append; DELETE on cut 850 | — |
| errors | `id INTEGER PRIMARY KEY` rowid (0013:7) | append; DELETE on cut 857 | — |
| inner_calls | `(session_id, parent_id, n)` (0020) | INSERT OR REPLACE at start (610), then upsert of the output (639-643) | output, is_error, files, at |
| reminders | `(session, id)` (0019) | insert/delete (1626/1642) | — |
| projects | `name` (0018) | delete+insert (1603/1610) | icon, color |
| cost_owners | `id` (0015) | insert-or-ignore (accounting.rs:79), tombstone UPDATE 98, DELETE 124; **no FK to sessions, by design** | deleted_at, revision |
| model_calls | `call_id` (0015) | insert at reserve (accounting.rs:268), then UPDATE dispatched 289 / settle 321 / not_dispatched 361 / unresolve on restart 382 | status, tokens, cost, timestamps |
| remote_meters | `id`, UNIQUE(origin, external_session, epoch) | insert 483 + four UPDATE paths 430-472 | meters |
| peer_exchanges | `accepted_seq INTEGER AUTOINCREMENT` + UNIQUE id (0011) | insert + about 10 state UPDATEs (communication.rs:649-1429) | state, reply, history_seq, answered_by |
| peer_bindings | `id` | insert, revoke UPDATE | revoked_at |
| a2a_contexts | `id` | insert-or-ignore | — |
| a2a_tasks | `seq INTEGER AUTOINCREMENT` + UNIQUE id | insert + UPDATE (1756, 1806, 1442) | status, revision, snapshot |
| a2a_task_work | `(task_id, key)` | insert + settle UPDATE | settled |

There are no FTS/virtual tables and no triggers in the migrations (the only trigger is test-only, store.rs:3549). No table has an `hlc` column. Tables with a single-column PK: sessions, cost_owners, model_calls, remote_meters, peer_bindings, a2a_contexts, projects, errors (int), peer_exchanges (int autoinc), a2a_tasks (int autoinc).

### A2. Write paths
- Pool: `Store::open` uses WAL, busy_timeout 5 s, `max_connections(4)` (store.rs:231-247). The private store is `open_memory` with 1 connection (254-270). The module header says "Only the agent writes it" (store.rs:2-3): every writer is in `crates/agent`.
- Write statements outside tests: store.rs **51** (in 31 fns), store/communication.rs **27**, store/accounting.rs **15**, so **93 in total**. Call sites outside tests: supervisor.rs about 30, loop_.rs about 17 (+3 accounting at 1995-2049), acp_child.rs about 12, plus communication/network.rs.
- Writers:
  - The **`Agents` supervisor actor**: create/fork/delete/rename/pin/archive/project/model/effort/reminders, inner_calls through the AgentEvent subscriber (supervisor.rs:1657-1689), record_error (883).
  - **Each `Session` actor** (loop_.rs:531): append 2507, append_with_todos 3878, append_vision 4788, compact 3402/3504/4334/4350, undo/redo 819, cut_to 1514, commit_cut 718, remove_context 1790, record_error 2422, model_calls 1995-2049. It also spawns tasks that write: the title task (3249, which calls set_title_at_revision at 3272) and the harvest task (4076).
  - **Each `AcpChild` actor** (acp_child.rs:1165): append 447/494, set_title 468, save_todos 919, record_error 1214/1254, set_acp_session 1224, apply_meter 434, harness_model 374.
  - **The communication network actor**: peer_exchanges/a2a.
- **No single choke point.** Many actors write at the same time through the shared pool, with one transaction per call (e.g. append at store.rs:438-443). `store_for(id)` (supervisor.rs:6866-6871) is the only routing point (private vs durable), and only the supervisor uses it; Session and AcpChild hold their own `Store` clones.

### A3. Read paths and derived data
- `load_session` RPC (rpc/src/api.rs:375-382) goes to `Agents::LoadSession` (supervisor.rs:4847+). It picks the store with `store_for`, asks the live actor (`ContextHistoryNow`) if there is one, else calls `store.context_history`, and renders every row per page (ponytail at 4890). `list_sessions` reads only the durable `self.store` (4761-4770). The `session_read` tool goes through `ReadSession` (4810; session_read.rs:1-5).
- Live stream: `subscribe_events(session, descendants)` (api.rs:1305-1315, impl 3805+) filters the local bus broadcast `self.events.subscribe()` (3815). Private-tree members are dropped for every peer except the owner (1318-1320).
- Search: **no FTS**. An in-memory nucleo index is rebuilt from full scans (`search_titles`/`search_bodies`, store.rs:719-757; search.rs:1-15) and invalidated on TitleChanged/Compacted/ContextChanged (supervisor.rs:1693-1695).
- Spill: tool output over the cap goes to `<data_dir>/spill/<session>/<call_id>.txt`, and the message body keeps head + tail + a marker with the **absolute path** (spill.rs:4-6, 46-49). Compaction writes `epoch_<n>.txt` + `.summary.md` in the same place (8-10, 84-95). A **7-day sweep** runs at start (supervisor.rs:6512-6515). These files are on disk, not in the DB; ~/.theuth/spill has 122 session dirs.
- Compaction writes an `epochs` row and bumps the revision (store.rs:1802-1826). Snap PNGs are **inlined as base64 in the epoch head** (loop_.rs:4172-4179, 4311). Images and screenshots are inline `ContentPart::Binary` in message bodies (loop_.rs:3807). These inline bytes are most of the DB size.

### A4. Sizes (partial: no sqlite3 available)
- `~/.theuth/sessions.sqlite` is 126,227,055 B, plus a 7,916,007 B WAL (read_file header, 2026-09-25).
- From the KB fact `facts/theuth-sessions-store-volume-2026-09-24` (sqlite3, 2026-09-24): 108,429,312 B; messages 21,157, model_calls 6,337, inner_calls 436, sessions 166; about 1 message per minute on average, bursts of tens per second.
- Growth: about +17.8 MB in roughly one day (a heavy build day).
- **Still to measure:** `sqlite3 -readonly ~/.theuth/sessions.sqlite "select name,sum(pgsize) from dbstat group by name"`, and rows per day grouped by `substr(created_at,1,10)` on sessions/model_calls/epochs, and by `inner_calls.at`. Messages have no timestamp.

### A5. Private / never-leave markers
- Each private root tree gets its own `Store::open_memory()` (supervisor.rs:7228), routed by the `private_member` map in `store_for` (6866). `Retention::{Durable,Private}` is defined at tools/src/resources.rs:14-26. The scratch dir is mode 0700 under the OS temp dir and held by an OS lock (private.rs:1-5). The private artifact store is removed on close (artifact/src/store.rs:574-586). The event filter fails closed for non-owners (api.rs:1318-1322). `get_attachment` serves private files only when the session is named (api.rs:920-926, 3747-3752).
- Other markers: `sessions.scope` owner|isolated (0011), `unattended` (0022). 67 §5.4 says two things are never replicated: `signal/presage.db` and the iroh identity key (67-agents.md:251). Durable sessions have no "local-only" flag.

## B. Replica engine (`crates/replica`)
- `attach(pool, tables)` (lib.rs:93-109, then engine.rs:169-211): `inspect` requires exactly one PK column (a composite key is an error, engine.rs:330-333) and an `hlc TEXT` column (341). It installs `_replica_log` (345-361) and AFTER INSERT/UPDATE/DELETE triggers that write one log row per changed column; a delete is one row with no clock (365-411). A PK rewrite is not handled (407-408). The log is polled every 200 ms (POLL, :29), BATCH_ROWS 5000.
- Capture: `capture_locked` stamps each row with an HLC (the row's `hlc` if it parses, else a tick) (221-273). `emit_locked` then assigns the next per-origin seq, encodes with postcard, inserts into the outbox, updates col_clock/tombstones, saves the clock and broadcasts on the feed (277-312). The origin is a uuid v7 owned by the replica (`replica.sqlite identity`), which is **not** the mesh EngineId (67-as-built-R.md:47).
- `replica.sqlite` holds `outbox(origin, seq, batch BLOB)`, `col_clock`, `tombstone`, `identity`, `clock`, and an unused `sync_state`. The version vector is `MAX(seq)` per origin over the outbox (as-built-R:48).
- Apply (apply.rs:28-101):
  - Skips its own batches and batches it already holds.
  - Captures pending local writes first (R2 fix, 42-51).
  - Opens one `BEGIN IMMEDIATE` per table and does per-row, per-column LWW against col_clock.
  - A delete wins only if it is above every column; a losing delete triggers a whole-row re-announce (91-99).
  - Deletes the echo log rows (78-81), then commits outbox + clock and publishes on the feed.
- **Only row/column LWW exists.** There is no raw commit-ordered log path and no grouping by transaction. A batch is whatever one 200 ms poll found, and apply uses one transaction per table (58-83), so a multi-table transaction is not applied atomically.
- Catch-up (sync.rs:147-206): both sides send `Hello{origin, version_vector}`, then every outbox batch above the peer's vector for each origin, then the live feed. A lagging feed is treated as an error and resyncs. **There is no snapshot**, and the outbox is **never pruned** (67-agents.md:517; loose-ends 2.4). MAX_FRAME is 64 MiB (:32); reconnect backoff is 1 → 60 s.
- Transport (types.rs:116-126): `peers()`, `open(peer, alpn)`, `accept(alpn)`, `subscribe()` → `PeerEvent{Connected, Disconnected}`. Framing is `LengthDelimitedCodec` + 1 tag byte + postcard (sync.rs:1-7, 139-144).
- ALPNs (lib.rs:40-49): `theuth/records/1`, `theuth/acp/1`, `theuth/sessions/1`, `theuth/knowledge/1`, `theuth/gossip/1`. `theuth/sessions/1` is reserved: it is registered in the mesh STREAM_ALPNS (mesh/src/net.rs:131-136) but nothing uses it. The mesh implements `Transport` at net.rs:731-760.
- Session extension and SQLite build:
  - rusqlite 0.38 with `["session","bundled-sqlcipher"]` is a **dev-dependency only** (crates/replica/Cargo.toml:28-33), used by the spike test.
  - The vendored `libsqlite3-sys` 0.36.0 has `preupdate_hook`/`session` features (vendor/libsqlite3-sys/Cargo.toml:61-65) and is patched in for the whole workspace (Cargo.toml:385-394).
  - Cargo.lock has **one** libsqlite3-sys (:10564), used by both sqlx-sqlite 0.8.6 and 0.9.0 (18606, 18633). So **sqlx links the same SQLCipher-on-BoringSSL build**.
  - A release build does NOT compile SQLITE_ENABLE_SESSION today (as-built-R:52). sqlx 0.9 (Cargo.toml:193) exposes no session API.
- Spike numbers (67-replica-spike.md:26-29): +0.45 µs/row capture, 40 B/row changeset, 10k-row apply in 22 ms. Limits found: capture is per connection (:35), and a DELETE carries no clock (:36).

## C. Artifacts (`crates/artifact`)
- Layout: blobs by content hash in `<data_dir>/artifacts/`, an `artifacts.sqlite` index, and locks in `artifacts.owners/` (artifact/src/store.rs:1-5). The blob name is `sha256[..16]` + extension (386-397); writes are atomic, and the same hash means the same bytes (597-620).
- `artifacts(call_id PK, id, session, name, …, version, origin, created_at)` (migrations/0001_artifacts.sql). A session references its artifacts through `session` + the tool `call_id`. Message bodies also carry absolute paths (spill markers, attachments).
- This machine has 72 blobs in ~/.theuth/artifacts (bytes not measured).
- Fetching is local RPC only: `get_artifact(call_id)` (api.rs:1266), `get_artifact_listing`, `export_artifact`, and `get_attachment(path)`, which only serves files under `<root>/{attachments,browser,computer,artifacts}` (920-926, 3741-3765). **There is no mesh ALPN for artifacts or attachments.**

## D. Device link
- `crates/link` pairs devices through a stateless relay with Noise:
  - The uplink is one WebSocket to `<relay>/h/<id>?role=harness` with NK registration (uplink.rs:1-2).
  - Each connection does an IK/IKpsk2 handshake; mode 0 carries JSON-RPC, mode 1 carries ACP (conn.rs:1-5).
  - Paired devices are listed in `<data_dir>/link/devices.yaml` (devices.rs:1).
  - `initiator.rs` makes this harness a device of another one, driven by `crates/nodes`.
- Devices use the same RPC API (`load_session`, `subscribe_events`; desktop rpc/client.ts:117). Events come only from the engine's **local bus** (api.rs:3815).
- Pairing is **per engine id** (relay path `/h/<id>`), and devices.yaml is not replicated. To follow a session whose owner moved, a device would need one of: pairing with or a route to the new owner, or the old engine acting as a proxy. It would also need a session → owner lookup and to resubscribe. Moving devices onto iroh is still open (67-loose-ends.md:38, item 3.2).

## E. Ownership/lease building blocks
- The mesh hub lease is a **pure election**, `elect(candidates, incumbent, now, probation, handover)` (mesh/src/lease.rs:65-89). It has **no epoch, term or fencing token**. `LeaseState{holder, since_ms, reason}` is at mesh/src/types.rs:109-120, and `lease_log` is local only (mesh 0001:35-39). `EngineId(String)` is the iroh endpoint public key (types.rs:10-16).
- `roster::lease::Follower{self_id, holding, seen}` is a pure gain/loss state machine (roster/src/lease.rs:8-57), with a `Duties` trait and `apply` (59-72).
- Tasks: `Checkout{session, engine, epoch, progress_at_ms, orphaned}` (tasks/src/types.rs:112).
  - `checkout` reads locally, returns a conflict if the task is held and not orphaned, and sets `epoch + 1` (ledger.rs:589-623).
  - `sweep` marks a checkout orphaned when its engine has been unreachable for `orphan_secs` or its progress is older than `stale_secs`, bumps the epoch, and wakes the owner with Resume (870-918).
  - Task rows are LWW-replicated, so the checkout guard is **not a distributed compare-and-set**: two engines can both win locally, and LWW decides afterwards.

## F. Ids, owner columns, ACP load
- `sessions` has no owner/engine/origin column (all of 0001-0022; `SessionRow` store.rs:104-121). `SessionSummary` (agent/src/lib.rs:1237), `AgentInfo` (:720) and `PeerInfo` (communication.rs:355) have no engine field. The session → agent map is owned by roster (67-agents.md:523). `remote_meters.origin` is the ACP harness origin, not an engine.
- `SessionId::new()` is `Uuid::now_v7()` (runtime/src/lib.rs:224-229), so ids are unique across engines.
- ACP `session/load` is served at rpc/src/acp.rs:322-337. It calls `api_load.load_session(session, None, None, Some(false))` on the local ApiClient, which goes to `Agents::LoadSession` and `store_for`, i.e. the durable or private local store only. It replays the history as notifications, then binds the session. **There is no path to load from another store or engine.** On the client side, acp/src/harness.rs:355-368 sends a `LoadSessionRequest` only when the agent advertises `load_session`. The mesh ACP server is `serve_mesh`, which accepts on `ALPN_ACP` (rpc/src/acp.rs:163-174).

## Constraints for the design
1. **The transcript tables use composite PKs**: messages (0001:13), epochs, todo_snapshots, inner_calls, reminders. `replica::attach` refuses composite keys (engine.rs:330-333) and needs an `hlc TEXT` column (341), which no agent table has. Stage 2 cannot use `attach` unchanged.
2. **`messages.seq` is MAX+1 and is reused after commit_cut** (store.rs:1781-1784, 829-831), so (session_id, seq) is not a stable identity.
3. **Rows are changed after insert, not only appended**: messages.excluded (1235), the inner_calls upsert (639-643), model_calls UPDATEs (accounting.rs:289-382), about 20 sessions columns, commit_cut deletes (832-879), and delete_session (1425-1470). A "commit-ordered log" must carry updates and deletes, not just inserts.
4. **No write choke point**: Agents, every Session actor with its title/harvest tasks, every AcpChild, and the communication actor all write through the 4-connection pool (store.rs:237-238). Session-extension capture works per connection (spike §3.1), so it needs a single writer connection or actor first.
5. Ownership today effectively means "which engine runs the live Session/AcpChild actor" (`live(id)`, supervisor.rs:4861). An epoch guard must cover the 93 write statements or sit in front of them in one new writer.
6. Some integer PKs are local and will collide across engines: errors.id rowid (0013), peer_exchanges.accepted_seq and a2a_tasks.seq AUTOINCREMENT (0011).
7. **Bytes are inline**: base64 images and snap PNGs sit in message and epoch JSON (loop_.rs:3807, 4311), which is why the file is 126 MB. First sync needs a snapshot, and none exists (sync.rs:164-176; the outbox is unpruned, 67-agents.md:517).
8. **Transcripts contain absolute local paths** (the spill marker at spill.rs:46-49, attachment/artifact paths). Spill files are swept after 7 days (supervisor.rs:6512-6515), and there is no ALPN to fetch artifacts or attachments.
9. Private trees must never replicate. Each has a separate in-memory Store (supervisor.rs:7228, store_for 6866). Durable sessions have no local-only flag.
10. Replica batches have no transaction boundary across tables (apply.rs:58-83; 200 ms poll, engine.rs:29). A message insert and its sessions.updated_at/context_revision update (store.rs:1791-1797) can arrive split.
11. Neither existing mechanism fences: the mesh lease has no epoch (mesh/src/lease.rs:65-89), and the tasks checkout epoch is LWW, not compare-and-set (ledger.rs:589-623). A per-session owner+epoch needs its own guard: reject writes with a stale epoch when applying.
12. The replica origin is not the EngineId (67-as-built-R.md:47). An `owner` column must pick one id space.
13. `load_session`, ACP `session/load`, `list_sessions` and `subscribe_events` all use the one local store and the local bus (supervisor.rs:4761, 4855; rpc/src/acp.rs:325; api.rs:3815). Sessions owned elsewhere need either a proxy or a replica-backed read path, plus event forwarding between engines.
14. Devices pair with one engine through `/h/<id>` (link/src/uplink.rs:1-2), and devices.yaml is local (devices.rs:1). Following a session whose owner moved means pairing with several engines, proxying, or moving devices onto iroh (loose-ends 3.2).
15. SQLITE_ENABLE_SESSION is compiled only in test builds (replica/Cargo.toml:28-33; as-built-R:52), and sqlx has no session API. Using it means turning the feature on for the whole workspace (build-time bindgen) and writing through a raw handle.
