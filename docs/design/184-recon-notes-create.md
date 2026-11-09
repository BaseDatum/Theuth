# 184 recon: Why a non-hub engine cannot create notes, and how documents live across engines (NotesHubRecon)

Read-only recon, 2026-11-09, at main b23f877f/6b8d5084 (anchors re-checked on 6b8d5084 where noted). Reproduced verbatim from the recon's report.

(In this file, double braces are written with a space, as `{ {` and `} }`, so the harness does not try to expand secret placeholders.)

NotHub refusal found: a non-hub engine's `CreateNote`, `CreateBoard` and `Create` are refused by a blanket hub gate. Nothing was run, built or edited. I started on b23f877f. Main then moved to 6b8d5084 (the design 183 phase 2 commit, 10:55:37Z, `.git/logs/refs/heads/main:276`) while I read. Every gate and forwarding cite below is identical on both. Shifted lines on 6b8d5084:
- `rpc/src/api.rs`: +38 up to `doc_forward`, +52 from `doc_create` and `doc_link_create` on.
- `document/src/lib.rs`: +21 after :340.
- `document/src/share.rs`: gates 367/411/451/553/601/640/688 are now 434/478/518/620/668/707/755.
- `link/src/follow.rs`: 200/215 are now 213/228.
- `document/src/hub.rs` gains a reseal-on-gain at :234.

The 183 worktree (fb2388c4) has the same blocks.

VERDICT
Sky's guess is half right. The live-sharing rule is real. His 2026-09-25 ruling was "the yrs server becomes a hub responsibility too" (knowledge `decisions/theuth-devices-pair-to-mesh-rulings-2026-09-25`), which is `69-devices.md:71` decision 11, "one live Doc per document". But decision 11 names editors and `doc_insert/replace/delete`, not creation.
- Creation was swept in by the contract's blanket "every write is hub-only" (`69-contract-docs.md:71`). Worker C2 added the gate (`69-as-built-C2.md:21`).
- Forwarding was built only for editor legs, `Edit` and RPC admin. Tool and plugin creates are listed "Not built" (`69-as-built-Y.md:53`, `69-as-built-E2.md:45`). It is a gap, not a ruled ban.

Refusal code: `document/src/actor.rs:1773` (`CreateNote`), `:1750` (`CreateBoard`) and `:1791` (`Create`) each open with `self.hub.gate_str()` (`hub.rs:114-127`).
- The text is exactly `NotHub:<hub engine id>`, or `NotHub:` when nobody holds the lease (`hub.rs:44,69-73`).
- Non-hub means the lease holder is not this engine (`mesh/src/lease.rs:47-56`). An empty lease counts too, so the hub itself refuses while the lease is empty.

What each caller sees:
- **UI and MCP `doc_create`** (`rpc/src/api.rs:6719-6742`; `mcp-server/src/catalog.rs:899-902`):
  - The local refusal becomes -32011 "another engine holds the hub lease" (`api.rs:3499,3513-3521`).
  - `doc_forward` (`api.rs:3529-3548`, wired by 69 Y, `69-as-built-Y.md:25`) re-issues it to the hub over `theuth/rpc/1`. The hub serves the full API (`forward.rs:315-346,410-433`).
  - So New note on a non-hub succeeds, but the note is created on the hub.
  - The refusal stands only for `hub: null`, no transport, or hub not connected. -32010 means connected but no answer (`forward.rs:344`).
  - The UI has no -32011 handling (`desktop/ui/src/components/notes/create.ts:29-31`).
- **Agent tool `doc_create`**, which is the subagent case (`document/src/tools.rs:880` note, `:911` board, error returned `:927`):
  - It asks the local actor with no forwarding.
  - The model sees `NotHub:<id>` verbatim (kameo 0.22.2 `error.rs:297` prints the inner string; `Cargo.lock:10474`).
- **Plugins** ask the local actor too:
  - Meeting note (`meeting/src/note.rs:164-179`), task thread (`tasks/src/thread.rs:31-48`) and dreaming journal (`dreaming/src/journal.rs:173-188`) log a warn and carry on.
  - `autoresearch/src/finalize.rs:1170` and `document/src/ingest/insights.rs:423` ask it as well.
- **Cells running on the hub** is a separate, deliberate rule, design 171 W-d (`171-cells.md:359,1034-1036`). The route is `cells/src/note.rs:212-219` to `rpc/src/cells.rs:94-105`. It follows the lease, not where the note was made.

HOW DOCUMENTS LIVE ACROSS ENGINES TODAY
- **Store:** `documents/<id>/{meta.json,snapshot.yjs,updates.log,snapshots/,note.md}` plus `documents/index.sqlite` (`store.rs:1-4,26-36`). Notes, notebooks, sources, knowledge, artifacts, meeting notes, charters and task threads share it (`meta.rs:147-170`). `meta.json` carries no engine id.
- **Rows:** `docs`, `notebooks`, `notebook_items`, `sources`, `cell_latest` (`store.rs:76-82`) go through `replica::attach` (`document/src/lib.rs:245`). That is per-column last-writer-wins under an HLC over `theuth/records/1` (`replica/src/lib.rs:1-5,42`).
- **Bytes:**
  - Flat `documents/mirror/<id>.meta.json` and `<id>.snapshot.yjs` map to `doc_files` rows in `mirror.sqlite` (`lib.rs:336-341`; `replica/src/files.rs:1-14`).
  - Foreign copies are merged on arrival and never overwritten: Yjs states join, and meta merges by field rule (`69-contract-docs.md:45-60`; `actor.rs:2261-2290`).
  - `updates.log`, `snapshots/` and `agents.json` stay local (contract:40; `store.rs:31`).
  - It is not sessionsync, and there is no engine-to-engine Yjs update stream. Engines swap whole snapshots only.
- **Live editing:**
  - Only the lease holder loads the live Doc and the helper, fans out, and exports derived files (`hub.rs:164-206`; `actor.rs:447`).
  - A non-hub UI talks to its own engine. Open, Update, Subscribe, Awareness, Join, SetName and Edit are relayed over `theuth/docs/1`, one stream per (client, doc) (`actor.rs:888-900,1357-1370`; `relay.rs:1-19,127-138`; `replica/src/lib.rs:55-56`).
  - The editor is a proxy to the hub's doc, with a Y.Doc in the browser (`ui lib/doc/provider.ts:86-90,318-341`).
  - On a lease move the old hub snapshots and unloads, subscribers get `Closed{hub_moved}`, and editors reopen on the new holder (`hub.rs:182-210`).
- **Reads** (`Lookup`, `Outline`, `Markdown`, `ListDocs`; `actor.rs:1122,1500,1640,1988`) are local on every engine. They are as fresh as the last quiet snapshot (2 s, `lib.rs:116`; `actor.rs:2461`) plus replication. A loaded doc off the hub is a throwaway cache (`actor.rs:2274-2276`).
- **Authority:**
  - Every engine holds the full Yjs state, and the creating engine has no special role.
  - The lease holder is the only live writer and exporter.
  - Share URLs name the mesh id (`api.rs:6105-6128`), which only the hub registers at the relay (`link/src/follow.rs:200,215`).
  - Every engine keeps its own `/h/<engine id>` uplink, and a guest landing there is proxied to the hub (`69-as-built-Y.md:30`).

HUB-ONLY LIST (L = load-bearing, I = incidental)
- **L live Doc serving:** forwarded, never refused.
- **L derived-file export, watch and import** (`actor.rs:447,2219`; `hub.rs:173-181`): two exporters echo (`69-as-built-E2.md:3`). Knowledge and artifact files sync by their owners.
- **L `Ensure`/`ImportMarkdown` for knowledge and artifacts** (`actor.rs:1149,1233`):
  - Ids are deterministic uuid-v5 (`meta.rs:30-38`), so two engines creating one give two Yjs lineages and duplicated blocks (`69-contract-docs.md:14`).
  - Noise today: "knowledge → document import failed" (`knowledge/src/store.rs:384`).
- **L `Restore`, `UndoLastAgentEdit`, `Rename`** (`actor.rs:2144,1438,1870`): they rewrite the live doc, and only the RPC forwards them (`api.rs:6691-6772`).
- **L Join/admission and ingest/insights** (`share.rs:451`; `ingest/mod.rs:367,474,617`; `insights.rs:343,385`): live members, and source blobs are per engine (`144-notebooks.md:230`).
- **I `CreateNote`/`CreateBoard` of keyless notes:** the id is a fresh v7, so nothing can collide (`meta.rs:22-33,180-182`). Boards embed the machine's absolute root (`actor.rs:1722-1738`; `tools.rs:902`), so create them where the repo is.
- **I `Create` of meeting note and task thread** (`actor.rs:1791`): ids are deterministic but one engine owns each (recording engine; task home, design 182). Today they silently do not exist when made on a non-hub.
- **I `Delete`, `DeleteNote`, `Snapshot`, `SetDerived`, `Bind`** (`actor.rs:1962,2201,2024,2047,2094`): soft-delete tombstones replicate (`69-contract-docs.md:25`).
- **I link/member CRUD** (`share.rs:367,411,553,601,688`): meta merges by union (contract:53-58) and the RPC forwards. Same for notebook CRUD (`notebook.rs:681,743,865,938,1005`), whose rows replicate (`144-notebooks.md:226-228`).
- **Cells** are a policy gate (`document/src/cells.rs:255` `PutCellLatest`, plus the routing) with three load-bearing things behind it:
  - Run text is read from the live doc. `CodeBlockText` (`cells.rs:50-60`) reads the local replica off-hub, stale by 2 s or more, so D3's digest check would refuse (`171-cells.md:15`).
  - The `cell_latest` id `<doc>:<cell>` has no engine column and is single-writer by design (`171-cells.md:1005,1014`).
  - The who-wrote-it map `agents.json` is engine-local (`store.rs:31` → `cells/src/note.rs:230-233`).
  - The UI python chip already asks the local engine (`NoteCell.tsx:160`) while the run goes to the hub.

LIFTING IT SAFELY NEEDS
1. **Smallest fix, covers subagent and plugin refusals:** forward creates like `Edit`.
   - Add `relay::Msg` tag 10 plus a `Cmd` (`relay.rs:127-138,226-283`).
   - Use `match self.hub.route()` at `actor.rs:1750/1773/1791`, as at `:1357`.
   - The note is still born on the hub, which is what the RPC does now.
2. **Create on the engine itself (Sky's ask):** ungate keyless Note and Board only, plus three things.
   - (a) Snapshot right after the first import off-hub. The quiet snapshot is hub-only (`actor.rs:2461`), so otherwise the mirror waits for the 600 s unload (`lib.rs:114`).
   - (b) The creator's `doc_open` must wait or retry until the hub has the row and files (`Store::lookup` is None for a row without a directory, contract:26).
   - (c) Keep `Ensure` and shared deterministic bindings hub-only or forwarded.
   - There is no creation race: the id is private until it replicates, and the hub then loads it via `MirrorChanged` Created (`actor.rs:2277-2283`).
3. **Share live from any engine:** guests on a non-hub's own uplink already work by proxy. Only the URL builder stamps the mesh id (`api.rs:6105-6110`). Serving the live Doc from several engines is a new design and Sky's call. It needs Yjs update exchange between engines, exporter election, an awareness and `agents.json` merge, and `hub_moved` retired. Link secrets already open on any engine (contract:60).
4. **Cells where viewed:**
   - Change the route (`cells/src/note.rs:212`, `rpc/src/cells.rs:94,120,397`).
   - Fetch the block text from the hub over a new relay message, or wait for the replica.
   - Key `cell_latest` per engine, and ungate `document/src/cells.rs:255`.

LOG CHECK, `theuth.log.2026-11-01..09` on this Mac
- `NotHub` is 0 lines and `-32011` is 0. Plugin warns (meeting note, task thread, journal, knowledge import) are 0. Raw greps of `sessions.sqlite` (+wal), `agent.sqlite` and `tasks.sqlite` find no `NotHub`.
- "another engine holds the hub lease" appears 6 times (11-04 ×3, 11-06 ×2, 11-07 ×1). All are the lease-loss notice at `hub.rs:198-201`, not refusals. Each shows `hub=None` and is followed 15-21 s later by "serving (hub lease held here)". A create in those windows would get `NotHub:`.
- Sample: `theuth.log.2026-11-04:1738: 2026-11-04T05:49:19.452304Z INFO actor.handle_message{actor.name="DocumentActor" ...}: document::hub: documents: another engine holds the hub lease; serving stopped hub=None closed=0`
- This Mac holds the lease and `mesh_peers()` lists no other engine, so no refusal is expected here. The real ones are on the non-hub engine, which can run `grep -c 'NotHub:' ~/.theuth/logs/theuth.log.2026-11-0*`. The plugin warns carry the text; tool refusals sit in that engine's session DB.
