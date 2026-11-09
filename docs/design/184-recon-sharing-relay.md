# 184 recon: How live document sharing over the relay works, and what ties it to one engine (DocRelayRecon)

Read-only recon, 2026-11-09, at main b23f877f/6b8d5084 (anchors re-checked on 6b8d5084 where noted). Reproduced verbatim from the recon's report.

(In this file, double braces are written with a space, as `{ {` and `} }`, so the harness does not try to expand secret placeholders.)

DocRelayRecon report. Main b23f877f, read-only, nothing run. Paths are under `crates/` unless they start with `docs/` or `~`. Design-doc line numbers are real file lines.

P2 worktree (fb2388c4) differs from main only in `link/lib.rs`, `link/follow.rs`, `document/hub.rs` and `document/share.rs`. Those add a hub-only `Reseal` after vault rotation and the device-key reset. Everything I cite below is identical there. The rpc doc handlers sit 38–52 lines lower.

**Verdict:** the hub tie is the live yrs server, not the relay. Sky ruled it on 2026-09-25:
- `docs/design/69-devices.md:71` (decision 11): the hub holds the live Doc, "no cross-engine CRDT reconciliation".
- `docs/design/69-contract-docs.md:71`: "every write is hub-only", with `CreateNote` and `Create` in the list.
- `69-devices.md:72`: share links name the mesh id.

Reversing that is his call.

**1. Sharing as built**
- **Owner UI:**
  - `share/ShareDialog.tsx:30-38,63-70` → `api.doc_link_create` → `rpc/src/api.rs:6086-6132`.
  - It needs this engine's Link on (6099-6104; default off, `link/src/types.rs:32`).
  - Link id is the mesh device id if there is one, else the engine id (6105-6110). URL is `{relay}/d#1.{id}.{doc}.{secret}` (6128).
  - `LinkCreate` is hub-gated (`document/src/share.rs:367`) and stores the sealed sha256 of the secret (380).
  - Off the hub the call is re-issued to the hub over `theuth/rpc/1` (api.rs:3529-3548). If the hub is not a connected peer, the refusal stands (`rpc/src/forward.rs:335-337`, api.rs:3544-3547). Design 59 §8.2 is `59-documents.md:817`.
- **Relay:**
  - `/d` is only the SPA fallback (`relay/src/main.rs:115-121`, `web.rs:32,61-78`). The socket is `/h/{id}` (main.rs:116,181-227).
  - Guest and device sockets look the same (59 §8.7, :898). They park until a harness has registered that id (`relay/src/relay.rs:3-4,448-459`).
- **Guest page:**
  - `ui/src/link/guest.ts:20` treats path `/d` as the guest page. `:23-30` requires exactly 4 dot parts with tag `1`.
  - `components/share/GuestApp.tsx:121-173` dials `/h/<2nd field>` as an engine row (:124,135).
  - It then sends `doc_join` with an HMAC proof bound to the Noise hash (guest.ts:33-39; 59 §8.3, :831).
- **Engine:**
  - An unknown static completing IK becomes `Handshake::Guest`, "mode 2" (`link/src/conn.rs:159-173`).
  - Limits: 10 handshakes a minute (:408-430), 5 s to join (:555), and a method allow-list `GUEST_METHODS` (:538-552).
  - `run_guest` (:584-641) → api.rs:6152-6188 → `Join` (share.rs:445-532). The proof is checked against the sealed hash (73-89) and the member is added to `meta.json`.
  - Both uplinks admit guests (conn.rs:278-311).
- **Fan-out:**
  - A guest's `doc_subscribe` (api.rs:9449-9487) → `Subscribe` (`document/src/actor.rs:1022-1051`).
  - `apply` (:348-443) appends to `updates.log`, then `fan_out` (:324-344) sends base64 Yjs updates to every subscriber except the sender's conn.
  - It all travels as JSON-RPC inside Noise on the relay socket (59 §6.1-6.2, :615,648; client `ui/src/lib/doc/provider.ts:312-349`).
- **Which engine gets the join:**
  - The relay routes by the id in the link. A mesh id reaches the lease holder only. An engine id reaches that engine.
  - A non-hub engine acts as a front door. `OpenDoc`, `Update`, `Awareness`, `Subscribe`, `Join`, `SetName` and `Edit` ride `theuth/docs/1` legs to the hub (actor.rs:885-902,973-994,1025-1036,1068-1089,1351-1372; share.rs:451-457; `document/src/relay.rs:1-19,226-276,366-387,391-524`; hub side :545-714; `replica/src/lib.rs:56`; `mesh/src/net.rs:344-351`).

**2. Relay links**
- **Engine uplink `/h/<engine id>`:** every engine with `link.enabled` registers one (`link/src/lib.rs:445-456,513-515`; Enable :220-224; `uplink.rs:93-115`). The default is off.
- **Mesh uplink `/h/<mesh id>`:** hub only (`link/src/follow.rs:187-220`, gate at :191; `mesh_uplink.rs:1-14,73-76`). A loser stops and never redials, because the relay's `register` replaces the old holder (`relay/src/relay.rs:230-237`; map keyed by id, :153).
- **No owner or per-document limit** exists in relay or link. The relay has no owner notion, only one live uplink per id.
  - Caps are per engine: 16 devices (`types.rs:24,35`) and 10 guest handshakes a minute.
  - There is also a 64-member limit per doc (`document/src/lib.rs:90-91`).
  - "One engine per doc" lives in the link format, which carries a single id (guest.ts:23-30), plus the hub gate.
  - Election uses `mesh.hub` prefer, eligible or never (`mesh/src/config.rs:24-25`, `lease.rs:126-150`).
- **Evidence:** this Mac's log has "mesh uplink registered: serving paired devices" (`~/.theuth/logs/theuth.log.2026-11-09:584,1638`), so this Mac is the hub.

**3. Co-editing today**
- Only the hub applies edits. It holds the one live `yrs::Doc` (actor.rs:277-321). The other engine proxies editor sessions over the docs legs and keeps its own loaded Doc only as a read cache.
- At rest both engines hold the state:
  - The `docs` row replicates (`document/src/lib.rs:235-250`).
  - `snapshot.yjs` and `meta.json` replicate as a mirror (lib.rs:335-341; `69-contract-docs.md:44-51`, "states join").
  - Arriving mirror state is Yjs-joined in (actor.rs:2261-2364). Off the hub the cache is dropped on a mirror change (:2274-2276).
- `updates.log` is not mirrored, and the hub snapshots after 2 s quiet (lib.rs:98-100).
- On lease loss, `sync_hub` snapshots and unloads, and subscribers get `Closed{hub_moved}` (`document/src/hub.rs:164-206`).

**4. Why off-hub creation fails (symptom 1)**
- `CreateNote`, `CreateBoard` and `Create` gate on the lease (actor.rs:1750,1773,1791).
- Only the owner RPC `doc_create` re-issues to the hub (api.rs:6719-6742).
- These call the local actor directly and return `NotHub:<hub>`:
  - agent tool: `document/src/tools.rs:877-886`
  - meeting note: `meeting/src/note.rs:164-179`
  - dreaming journal: `dreaming/src/journal.rs:174`
  - autoresearch: `finalize.rs:1170`
  - insights: `document/src/ingest/insights.rs:423`
- Only `Edit` forwards (actor.rs:1351-1372).
- The UI path fails with `-32011` "another engine holds the hub lease" when the hub is not a connected peer (api.rs:3516). I did not check what the UI shows.
- **Cells (symptom 2, NoteCellsRecon's job):**
  - `cells/src/note.rs:212-219` `route()` ignores the doc and returns the lease holder.
  - `rpc/src/cells.rs:94-105` forwards every `Scope::Doc` call to the hub.
  - `PutCellLatest` is hub-gated (`document/src/cells.rs:255`).

**5. Changes needed**

Recommended (A): a per-document "home" engine for notes, boards and meeting notes, instead of one mesh-wide hub. It keeps one live Doc per document, so no new live CRDT sync, and it reuses the legs. Knowledge and artifact docs keep the hub because of derived files and git.
- **A1, un-gate creation** (actor.rs:1750,1773,1791).
  - `create_doc` already writes meta, row and mirror (actor.rs:1806-1850), and note ids are uuid v7.
  - Risk: a first `doc_open` sent to the hub races mirror replication, and an off-hub live doc is unloaded on a mirror change (:2274-2276). Both need A2.
- **A2, add `DocMeta.home`:** set at create and merged like the other meta scalars (`mirror::merge_meta`).
  - Replace `Hub::route` (hub.rs:131-137) with `route(doc)` in the hub-only handlers (the `69-contract-docs.md:71` list; share.rs:367,411,448-462,553,601,640,688).
  - Risk: with the home offline the doc is not writable elsewhere. Today the lease re-elects in about 30 s.
  - It needs a take/move (cf. 181) and an epoch fence so two engines never both serve one doc, since `updates.log` is unreplicated.
  - `hub: never` engines would start serving notes, which is a policy call.
- **A3, exports and the file watcher follow the home** (actor.rs:447,2219). Low risk.
- **A4, sharing, with no relay change:** every Link-on engine already registers and admits guests. Choose the link id from the home engine:
  - (i) the home's engine id (api.rs:6105-6110), which the guest dials directly; or
  - (ii) keep the mesh id and let the hub forward to the home, at the cost of an extra hop.
  - Best is a link v2 carrying [home engine id, mesh id], with the guest trying them in order (guest.ts:23-30, GuestApp.tsx:124-135).
  - Risk: a format bump and a `/d` bundle deploy on the relay (old links must still parse). Engine-id links die with that engine, and 69 decision 13 (`69-devices.md:73`) meant to retire engine ids.
- **A5, link admin and join on the home:**
  - `doc_link_create` must check the home's Link state, not the local one (api.rs:6099-6104).
  - Join, link and member admin go through `route(doc)`.
  - P2's `Reseal` runs only when hub (worktree `share.rs` and `hub.rs`), so it must run wherever docs are homed. This is a seam with P2.
- **A6, cells:** make `route()` local and un-gate `PutCellLatest` (`cell_latest` already replicates, `document/src/store.rs:65-82`). Risk: the source is read from the local replica Doc, up to about 3 s behind (2 s quiet snapshot plus 300 ms mirror debounce, `replica/src/files.rs:29`).
- **A7, UI:** the Share copy says "this Mac" (ShareDialog.tsx:88-90), and there is no NotHub handling for notes.
- **Smallest fix for symptom 1 alone:** a `Cmd::Create` leg like `Cmd::Edit` (E2), so tools and meetings forward to the hub. Low risk, but notes stay hub-bound.

Not first (B): every engine a Yjs peer with live engine-to-engine streams.
- It converges, since the mirror already joins snapshots.
- It reverses Sky's ruling, needs echo guards under his no-storm rule, and adds N² legs and a helper mirror per engine.
- Concurrent agent block edits can double-apply, and `agents.json` is engine-local (`document/src/store.rs:31-32`).
