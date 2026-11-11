# 191 — MESH-CONTACTS: a contact is a contact for every machine of the mesh

Status: contract. At main d10feaa8 (190 landed). Recon: `/tmp/d191/recon.md` (written at 57840336); every path:line below was re-read at d10feaa8. Short names: **A** = `crates/a2a/src`, **M** = `crates/mesh/src`, **R** = `crates/replica/src`, **N** = `crates/agent/src/communication/network.rs`, **F** = `crates/agent/src/communication/network/full.rs`, **S** = `crates/agent/src/store/communication.rs`, **UI** = `crates/desktop/ui/src`.

## Sky's words (2026-11-11)
- **14:23:** "shouldn't contact links just be replicated across the mesh and durable. this seems overcomplicated right now. A contact is a contact for every node in the mesh, not just a single node."
- **14:59**, five answers:
  1. A machine leaves the mesh: "Cut off that machine only". Contacts learn it through the 191 protocol.
  2. A contact on an old build: "we don't plan for this. we are developing now, not with a user base." **Standing rule:** no compatibility for older theuth builds: no relay for un-updated contacts, no version floors, no dark launch. Data this machine's previous build wrote must still load.
  3. A contact's mesh adds a machine: "Yes, automatically", vouched by a machine already trusted.
  4. A contact writes to a chat whose machine is off: "Refuse at once".
  5. Existing per-machine contacts: "Merge automatically".
- Earlier: contacts are for "intersession communication once a contact is trusted, and a session is shared … this is supposed to make things easier, not be frustrating."

## Today, in one paragraph (verified)
A contact is one engine's private state: the file `a2a/peers.yaml` (A/config.rs:20), one pinned endpoint id per entry (`PeerEntry.endpoint`, A/config.rs:154; "one contact per endpoint", :578-582), a gate map `Live.contacts: eid → Principal` (A/server.rs:116, built :187-191), a client that dials one eid (`Link::Iroh`, A/client.rs:337-342), and local tables: `peer_bindings` minted per engine (`Uuid::now_v7()`, S:933), `a2a_*`, `peer_exchanges`, `contact_threads` (never in the session log: `crates/agent/src/store/capture.rs:4-5`). A session on another machine cannot message a contact ("unknown peer", N:4035). A contact's message to a chat another machine owns runs W-40: a forwarded, capped, final-text turn (N:3680-3964).

## Decisions

### D1. Identity: each machine keeps its own key; a contact pins a set of endpoints
- **Why not one shared key.** One Ed25519 key terminates TLS for every ALPN (M/net.rs:537-546), signs the card (M/net.rs:945-947; `kid` = `net.id`, A/contacts.rs:826-844) and is the mesh engine id (M/identity.rs:1-41). It cannot be live on two machines:
  - the relay keeps one active client per endpoint id and demotes the older with `SameEndpointIdConnected` (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iroh-relay-1.4.1/src/server/clients.rs:81-94`; the lock pins iroh-relay 1.4.1, `Cargo.lock:9869`, not the 1.2.0 the recon read);
  - discovery flaps: pkarr keeps the newest signed record (`iroh-dns-1.4.1/src/pkarr.rs:270-276`, republished every 5 min and on change), and the mDNS cache keeps the last announcement per id (`iroh-mdns-address-lookup-0.6.1/src/lib.rs:566-578`); both run when `mesh.discovery` is on (M/net.rs:752-787).
  - A shared signing key would also outlive a removed machine: the contact's pin is external, so a leak means every contact pairs again.
- **Decision (option c).** Each machine keeps `mesh/identity`. A contact record holds a **set** of the other side's endpoints. Grants, first-contact answers, block and removal belong to the record, never to one endpoint. Nothing is secret, so nothing rotates.
- **The vouching statement is a signed card extension.** No new route and no new signature scheme.
  - Every card carries `https://theuth.io/a2a/ext/members/v1`, with `params.members` listing the signer's mesh as it sees it. The signer is left out: it vouches for the others, never for itself.
    ```json
    { "uri": "https://theuth.io/a2a/ext/members/v1",
      "params": { "members": [
        { "id": "<endpoint id hex>", "name": "xeus", "added_ms": 1762800000000 },
        { "id": "<endpoint id hex>", "name": "old-mac", "added_ms": 1762700000000, "removed_ms": 1762850000000 } ] } }
    ```
  - Rows are the mesh `peers` rows of the paired set, tombstones included and quarantined rows left out (`mesh::pairing::PeerEntry{id, name, added_ms, removed_ms}`, M/pairing.rs:97-129; quarantine M/store.rs:152-154), capped at 64 (newest stamps first).
  - **Signature:** the card's existing JWS. Extension `params` round-trip through parsing and are covered by it (A/contacts.rs:473-481, :621-631). `verify_card` requires `kid == eid` of the endpoint the card was fetched from over its TLS-authenticated stream (A/contacts.rs:587-632). The members extension is omitted when the list is empty, so the JWS test vector (A/contacts.rs:2254-2341) keeps its value.
  - **Who may speak:** a statement counts only if its signer is a live endpoint of that record at the moment it is read.
  - **Replay and ordering:** each row is ordered by its own stamp, `max(added_ms, removed_ms)`, and a removal wins a tie: the mesh's own `pairing::newer` (M/pairing.rs:131-138), reused as is. Replaying an old card changes nothing, because its stamps are not newer. A row stamped more than `MEMBER_SKEW_MS` = 10 min ahead of the reader's clock is ignored. Without that bound, a member could stamp an add in the future and make an endpoint irremovable.
  - **No self-revival:** a statement never revives its own signer. The signer is absent from its own list, and a signer that is not live is not heard.
- **Adding an endpoint needs both sides (answer 3, fail-closed).** Endpoint Y joins record R only when:
  - a live endpoint E of R lists Y as live; and
  - Y's own card, fetched from Y, lists E as live.
  
  Without the second check, contact A could claim contact B's machine and inherit B's grants. A ceremony pin is the other way in (D2 `stamp 0` row).
- **Removal needs one side (answer 1).** One row from one live endpoint is enough, because dropping an endpoint only takes access away. When a contact's set no longer lists Y, its gate refuses Y on the next stream (the per-stream re-check, M/net.rs:2071-2078) and the reconcile closes Y's connections (D3).
- **How our machines tell contacts.** `Net.peers_gen` bumps on every change of the paired set, replicated tombstones included (M/net.rs:460-461, picked up by the 5 s `keys::tick`).
  - On every bump, and once at boot, each machine POSTs `NOTICE_PATH` (A/contacts/offers.rs:25; routed at A/contacts/reach.rs:211-246) to every contact, retried on the 2 s a2a tick until one post succeeds. The contact then refetches the card and applies the rows.
  - Nothing is persisted, because the boot pass replays everything. A crash between the unpair and the notice loses nothing.
  - The contact also reads members on every card refresh: at pairing, at boot, on notice, and at most once per `OFFERS_TTL` of 60 s (N:327-328).
- **Residual risk (accepted, same as 183 D11's device key).** Until one remaining machine reaches a contact, the removed machine still passes as us to that contact. A *malicious* removed machine that reaches a contact first could post rows removing our other machines. That is denial of service, not access. Recovery: re-pair that contact from any machine (`pin()` keeps a known endpoint's alias and settings, A/contacts.rs:1052-1056).
- **A removed machine learns nothing.** The mesh sends no "you were removed" frame (M/net.rs:1272-1297). It keeps its copy of the records and goes on posting statements, but no contact hears it once it is out of the contact's set.

### D2. Replication: last-writer-wins tables in `sessions.sqlite`
- **Mechanism:** `replica::attach` (per-column LWW under HLC, tombstones, version-vector catch-up: R/lib.rs:1-14, :104-121; R/sync.rs:1-14). The tables are appended to `replicate_records` beside `projects` and `cost_owners` (`crates/agent/src/store.rs:903-915`, attach :913).
  - They are outside the session log (capture.rs:18-40), so `SESSIONS_WIRE_SCHEMA` (store.rs:659-670) does not bump.
  - Every writer (S, N, F) already lives on that database.
- **Attach rules the schema must obey (verified):**
  - one primary-key column plus `hlc TEXT`, composite keys refused (R/engine.rs:617-646, :636);
  - **no UNIQUE, CHECK or FK**: a failed INSERT of a remote row is skipped silently and forever (R/apply.rs:615-631), and a failed UPDATE parks that table's slice (256 cap, 7 days: R/apply.rs:102-121, :23-29);
  - removal is a **column**, not a row delete: a delete wins only if it is newer than every column write, and any later column write revives the row (R/apply.rs:521-546). Uniqueness and validity are enforced in the reconcile (below).
- **Tables (migration `0059_mesh_contacts.sql`):**

| Table (replicated) | Key | Columns | Written by |
|---|---|---|---|
| `contacts` | `id` (UUIDv7 minted at pairing) | `alias`, `name`, `machine`, `binding` (the `peer:<uuid>` id), `paired_ms`, `paired_on` (our engine id that ran the ceremony), `blocked` (0/1), `removed_ms` (tombstone, never cleared), `merged_into` (record id or NULL) | pairing, every contact RPC, import, merge |
| `contact_endpoints` | `id` = `<contact>\|<eid>\|<stamp>\|<a\|r>` | `contact`, `eid`, `name`, `stamp`, `removed` (0/1) | ceremony (stamp 0), the members rule (D1); **rows are immutable**, so LWW never regresses a membership |
| `contact_grants` | `id` = `<contact>\|<export>` | `contact`, `export`, `granted` (0/1), `first_contact` (`ask`/`allow`/`block`, NULL = ask) | contact_update, contact_first_answer |
| `a2a_exports` | `name` | `session`, `agent`, `title`, `description`, `home` (engine id), `removed_ms` | a2a_export_put/remove, import |
| `contact_inboxes` | `id` = `<binding>\|<export>` | `binding`, `export`, `session` | the inbox's home machine (D4) |
| `contact_holds` | `id` = A2A task id | `contact`, `export`, `session`, `owner` (engine id), `until_ms` | the owner only; deleted when the hold ends |
| `contact_threads` (rebuilt) | `id` = `<binding>\|<context_id>` | the 0056 columns (`binding`, `context_id`, `session`, `export`, `outbound`, `last_ms`) | as today (S:1591-1676) |

  Local, not attached:
  - `contact_addrs(eid PK, addrs, at_ms)`: the direct addresses one machine saw, rewritten at most once a minute (A/contacts.rs:1990-2046). They differ by LAN, so they would flap if replicated.
  - `peer_bindings`, `a2a_contexts` (including `inbox_full` and `inbox_ceiling`), `a2a_tasks`, `peer_exchanges`: unchanged.
  - The card cache, held tasks and ceremony state stay in memory, as today.
- **What stays per machine and why.**
  - Inbox fail-closed flags and ceilings: a replicated "full" could outlive the live roster ceiling it was read from (S7–S10 below).
  - Tasks and exchanges: they live where the message was admitted.
  - Addresses: see `contact_addrs` above.
  - Inert fields are dropped at import and not replicated: `cap`, `modes`, `ExportEntry.tools` (A/config.rs:161-171, :213-216).
- **Secrets: none travel, none to lose.**
  - A contact holds no vault secret; a contact entry may not even name one (A/config.rs:568-577). Authority is the machine's own key plus the other side's set.
  - A machine that leaves keeps copies of public rows and its own key. It is cut off by the contacts' sets (D1), not by a rotation.
  - The vault and its unpair rotation are untouched.
- **Binding identity.** The record's `binding` column is the `peer:<uuid>` everywhere. `bind_peer_in` (S:905-946) takes the id instead of minting it:
  - a live row with that id updates its alias in place, so a rename no longer revokes;
  - otherwise the live row of that alias is revoked and the id inserted;
  - an id already revoked here **stays revoked**, and its contact is quarantined on this machine. A replica never un-revokes.
  - Unblock and re-pair write a fresh `binding`, because a revoked binding is dead.
- **Reconcile: one pass, run on every change.**
  - **Trigger:** the existing 2 s a2a tick (A/lib.rs:712) hashes every row's `(id, hlc)` of the six staged tables. `contact_threads` is read per request, never staged, so it stays out. A `max(hlc)` digest would miss a remote write older than the newest row. Every applied batch changes that hash, parked ones included, so every apply is reconciled within one tick. The pass also runs right after every local RPC write. Polling, not `ReplicaHandle::subscribe`, because parked changes that apply later are never published on the feed (R/engine.rs:748-750; R/apply.rs:275-361).
  - **Steps, in order:**
    1. Build `Staged` from the tables row by row.
    2. Commit routes (`CommitPeerConfig`, A/lib.rs:553-556 → N:2843-2916).
    3. Revoke every binding outside the set and cap its inboxes (`drop_unrouted` → `revoke_peer_bindings_except`, F:507-518, S:1134-1162).
    4. Release holds whose grant changed: allow → start, block or removed → deny (N:3497-3528).
    5. Close the connections of every endpoint no longer admitted.
- **Per-row quarantine (a bad row never freezes `stage()`, a block always applies).** Today `stage` is all-or-nothing (A/config.rs:625-809, `Err` at :804-808; caller A/lib.rs:388-389).
  - The new stage validates each record alone. A bad record is skipped and reported as `Contact.problem`, and **its endpoints go into a deny set** that the gate checks first.
  - Bad means: malformed alias or eid, an export it names missing, a revoked `binding` here, an endpoint that is one of our own mesh machines (family), or an endpoint live in two unmerged records.
  - Blocked, removed and merged-away records only ever add to the deny set, so they apply whatever else is wrong with them.
  - Alias collisions between different records: the older `(paired_ms, id)` keeps the alias, and the reconcile writes `<alias>-2` into the newer one. Every machine computes the same value, so the writes converge.
- **Admission waits for the first sync.** The gate (`ContactAlpn::admit`, A/contacts/reach.rs:116-121; `iroh_gate`, A/server.rs:916-935) refuses every contact until `mesh::synced()` (M/lib.rs:123-137), and no route is committed before it either.
  - The latch is bounded: true at once with no paired peer, after 30 s with peers paired but none connected, after 150 s at worst (M/lib.rs:1478-1520).
  - So a machine that was down during a block or removal never admits from its stale copy. Tombstones arrive in the catch-up (R/sync.rs:217-325), and the first reconcile revokes (186 R3).
- **Merge (answer 5): a pointer, never a copy.**
  - **Same contact** = two live records sharing a live endpoint. That happens when two of our machines each paired the same machine (two `stamp 0` rows for one eid), or when mutual vouching (D1) adds an endpoint that another record already holds.
  - **Write:** the survivor is the lower `(paired_ms, id)`; every machine writes `merged_into = <survivor>` on the other record, the same value from all, so LWW is harmless.
  - **Read** (the effective contact):
    - endpoints: the union over the group;
    - grant per export: the strictest over the group (`block` > `ask` > `allow`, ungranted beats granted);
    - blocked: any record blocked;
    - alias and binding: the survivor's.
  - Copying values instead would race: a machine missing the loser's `block` could write `allow` last.
  - **Edits** to a merged contact write every record of the group, so the strictest-wins read never pins an old value.
  - **Merged-away bindings:** revoked by the reconcile, so their inboxes are capped (S3/S5) and their open conversations end ("the conversation with … ended", F:157-268). The next message starts fresh. Grants, exports and first-contact answers carry over through the group read.
- **Import (data the previous build wrote must still load).**
  - **When:** at the first reconcile after `mesh::synced()`, each machine imports its `peers.yaml` contact entries and its exports. A contact entry is a `PeerEntry` with `is_contact()` (A/config.rs:184-195); `PeersFile` and `PeerEntry` stay readable as they are (`deny_unknown_fields`, A/config.rs:111, :138).
  - **Contacts:** an existing record with a `stamp 0` endpoint for the same eid is reused; otherwise a new record is made. `binding` takes this machine's live `peer_bindings` id for that alias, so its history keys go on; `paired_on` is this machine. First-contact answers and grants become `contact_grants` rows.
  - **Exports:** `a2a_exports` rows with `home` = this machine.
  - **Then** the file is rewritten without contacts and exports. Only HTTP peers stay; they are out of scope and keep their vault names.
  - **Safe to repeat:** a crash before the rewrite imports again onto the same rows. Duplicates across machines meet in the merge.
  - `contact_threads` rows are copied into the rebuilt table by the migration, with the new `id`.

### D3. Routing
- **Inbound: any machine accepts, the owner serves (R1).** The gate admits a stream from any live endpoint of an admitted contact. The router then resolves the owner:
  - chat export → `a2a_exports.session` → `sessions.owner` (`session_owner`, `crates/agent/src/supervisor.rs:5937-5952`);
  - agent export → `contact_inboxes[(binding, export)].session` → its owner; with no inbox yet, the export's home (D4);
  - `~return` → `contextId` → `contact_threads` → session → owner;
  - `GetTask`/`CancelTask` → the engine named by the task id prefix (below).
  
  Owner = this machine: served as today. Owner elsewhere: **`a2a_forward`** (contract) carries the request over `theuth/rpc/1` as `peer_deliver` does (`crates/agent/src/communication/mesh.rs:94-118`; `Forwarded(peer)` marker, `crates/rpc/src/forward.rs:126`, :485-505). The owner:
  - refuses a call that lacks the marker (as `contact_deliver` does, `crates/rpc/src/api.rs:4492-4498`);
  - re-checks `caller_eid` against **its own** reconciled gate, synced latch included;
  - serves the request in its own router with `ContactCaller(eid)` (A/contacts/reach.rs:178-209).
  
  Admission, the first-contact hold, the A2A task, `deliver_full`, inbox limits and receipts then all run on the machine that holds the session. There is no envelope, no capped turn and no clamp. Bodies are bounded as today (one JSON-RPC line on `theuth/rpc/1`).
- **Owner offline: refuse at once (answer 4).** The receiving machine checks `transport.peers()`, the live control connections (M/net.rs:980-986). An owner not in it gets `ContactRefusal::OwnerOffline`, "the chat's machine is offline" (`crates/agent/src/communication.rs:1100`), with no wait (mesh.rs:174-176).
  - An owner still listed but dead fails within the RPC timeout of 30 s (mesh.rs:31, :205-213) with the same text.
  - Owner moved meanwhile: the owner answers `NOT_OWNER` (-32013, mesh.rs:126) and the receiving machine re-resolves once.
  - `OwnerOld` goes (standing rule).
- **Outbound: a session on any machine.** Every machine commits a route for every admitted contact (D2 reconcile), so "unknown peer" (N:4035) goes for contacts.
  - **Dial:** the sender dials with this machine's own key. `PeerClient::contact` (A/lib.rs:479-500) and `IrohTransport::call` (A/contacts/reach.rs:277-289) take the contact's live endpoint set: an endpoint with a live connection first, then the last one that answered, then the rest. The whole call stays inside `CALL_TIMEOUT` (30 s, reach.rs:39).
  - **Each machine keeps:** its own card cache (`full_shape` needs the offer, F:226-238) and its own hints (`contact_addrs` behind `ContactAlpn::hints`, reach.rs:137-149).
  - **No relay through another machine**, by the standing rule. A machine that reaches no endpoint reads "cannot reach <contact> from <machine>". With the default relay (`crates/mesh/src/config.rs:62`) that only happens with `mesh.relay: none` and no shared LAN (M/net.rs:65-67).
- **Return route and `contextId` across machines.**
  - The sender mints `contextId` for a fresh full conversation (F:241-258) and writes its `contact_threads` row (outbound = 1), which replicates.
  - The contact answers on `~return` at whichever of our endpoints it dials. That machine resolves `(binding, contextId)` through the replicated row and forwards to the session's owner. The owner runs `full_return` against its own thread row, binding and offers (F:621-742), and `full_intake` enqueues locally (F:366-399), which is correct because the owner holds the session.
  - Inbound conversations (outbound = 0) work the same way. A session that moves needs nothing: the owner is looked up on every request.
  - Race: an answer that arrives before its thread row replicated reads `TASK_NOT_FOUND` (fail-closed). This is not a practical risk: answers follow a model turn, and the replica polls every 200 ms (R/engine.rs:30).
- **Task ids name their machine.** A2A task ids are UUIDv7 today (N:1532), and `GetTask` reads the local `a2a_tasks` (N:2561-2580), so a poll landing elsewhere gets `TASK_NOT_FOUND`.
  - New ids: `<first 16 hex of the admitting engine id>-<uuidv7>`. The receiving machine forwards `GetTask`/`CancelTask` to the engine the prefix names.
  - A prefix that names nothing (an id from before 191) is served locally.
  - `ListTasks` answers the receiving machine's tasks only; no theuth code calls it (no `.list_tasks(` caller in `crates/`).
- **A session taken away completes its waiting full tasks.** Today nothing in the agent network reacts to an owner change (`SessionDrained`, `crates/agent/src/supervisor/transfer.rs:223-244`): a full message's task (`held`, `full_inbound`, N:625) on the old owner would wait for a consumption signal that now fires elsewhere.
  - On drain, the old owner completes those tasks as receipts: the message is in the session's queue, which moved.
  - Held first-contact tasks of that session are released through `start_or_queue` (N:3530-3575), which now forwards.

### D4. The agent-export inbox: one home
- An agent export names its machine: `<agent>@<engine>` (N:3043-3045). `a2a_exports.home` stores that engine's id. For `theuth`, it is the machine whose UI made the export; on import, the importing machine.
- **Only the home creates inboxes** (`handle_inbox`, N:3037-3140), so two machines never race to create one. A message for an agent export with no inbox row is forwarded to the home. Home offline: refused at once, as in D3.
- The home writes `contact_inboxes[(binding, export)] = session` when it creates the inbox. Later messages go to that session's owner, wherever it moved.
- **Fixes a fail-open that exists today.** Inbox caps live in the local `a2a_contexts` (capture.rs:4-5). On a machine that took the inbox, `a2a_inbox_identity` finds nothing, so `reinstall_contact_limits` returns `Ok(())` with no limits (N:3192-3195), and `ContactScoped` says false (`crates/agent/src/loop_.rs:4949-4964`).
  - With 191, a session listed in `contact_inboxes` and without a local row runs **capped** (`INBOX_FAIL_CLOSED` and an empty allowlist, N:3146), with the contact prompt.
  - The owner re-admits it full on the contact's next message. `handle_inbox` adopts the mapped session instead of creating one, reads the live persona there (S6) and marks the local row (S:2030-2061).
- **First-contact Attention.** Holds stay where the message was admitted, the owner (N:3406-3587, in memory).
  - The owner also writes a `contact_holds` row, deleted when the hold ends. At boot it deletes its own rows, because holds do not survive a restart.
  - `contact_first_waiting` and the `contact_first` Attention source (A/contacts.rs:2088-2122, :2141-2196) read the table, so the item shows on every machine. Readers drop rows past `until_ms` (`HOLD_TTL` 24 h, N:632).
  - An answer from any machine is a grant write (`contact_first_answer` writes `first_contact` or `blocked` today, A/contacts.rs:1927-1988). The owner's reconcile releases the hold. On the owner itself it also releases at once, as today.

### D5. W-40 is deleted (P3)
`grep -rn "W-40 only; removed by design 191" crates` lists 11 markers. They and the unmarked W-40 code go:

| Goes | Where |
|---|---|
| `default_contact_tools` | A/config.rs:219-223 |
| `ContactOrigin.cap` / `.tools` stamping; `ContactOrigin.holder` (read only by W-40) | A/server.rs:601-612; fields `crates/agent/src/communication.rs:701-726` |
| `PeerMeta.cap` / `.tools`, `ContactFrame`, the turn-cap install for a new contact turn, the PeerInput frame branch, `contact_message()` | `crates/agent/src/loop_.rs:203-209`, :215-231, :1278-1291, :1784-1798, :8744-8757 |
| `network_contact_frame` (its fallback is moot: a non-local contact task is forwarded, never run locally) | N:3326-3351; call `crates/agent/src/supervisor.rs:2287` |
| `CONTACT_GRANTABLE`, `contact_grantable` and their filter | `crates/tools/src/registry.rs:622-656` |
| `CONTACT_NOT_OWNER`, `ContactForwarded`, `Message<ContactDeliver>`, `Message<ContactForwarded>`, `forward_contact`, `contact_deliver` (with the owner clamp), `contact_turn_finished` | N:3680-3964 |
| the W-40 fall-through of `start_or_queue` (non-local now forwards instead) | N:1632-1654 |
| the W-40 branch of `start_task` | N:1729-1750 |
| the W-40 branch of `network_turn_finished`; `resumed_contact_envelope` | N:2310-2323; N:2346-2375 |
| `NetworkState.contact_waits` / `contact_replies` and test helpers | N:611, :614, :875-891 |
| `CONTACT_METHOD`, `ContactFailure`, `contact_forward` (the refusal mapping moves to `a2a_forward`) | `crates/agent/src/communication/mesh.rs:121-163` |
| `ContactEnvelope`, `ContactDeliver`; `ContactRefusal::OwnerOld` | `crates/agent/src/communication.rs:728-758`; :1086 |
| RPC `contact_deliver` | `crates/rpc/src/api.rs:247-251`, impl :4487-4514; UI `rpc/api.ts:1085-1086` |
| W-40 words in comments | `crates/agent/src/frame.rs:500`, :519-520 |
| `a2a.contact_cap`, `PeerEntry.cap` readers, the template line | A/config.rs:43, :161-164; `crates/theuth/src/main.rs:1259-1267` |
| tests | `serve_contact_deliver` and `an_exported_chat_elsewhere_runs_one_capped_turn_and_answers_only_it`, `crates/agent/src/supervisor.rs:16305-16591`; the W-40 halves of the `loop_.rs` test at :9948 and of `registry.rs` :3495 |

Kept, because the fail-closed states use them: `SetCap`, `SetAllowOnly`, `SetTurnCap`, `install`/`reinstall`/`clear_contact_limits`, the Resume cap and the journal source `contact` (S7: old journals and "unsure" still cap), and `contact_baseline` (`crates/agent/src/frame.rs:503-516`).

The recon said about 420 production lines. Re-counted at d10feaa8 it is **about 600–650 production and 360 test lines**.

Replaced by: D3 `a2a_forward` (the request, not a turn) and D4.

### D6. Mesh to mesh
Both sides run the same code, so each side's record of the other holds the other's endpoint set.
- Any of our machines may dial any of theirs, and the receiving machine forwards to its owner (D3). Rate and concurrency bounds are per principal (`admit_contact`, A/server.rs:290-302), so several endpoints of one contact share them; connection and stream budgets stay per endpoint (M/net.rs:47-49).
- One ceremony between one machine on each side: each side pins one endpoint (`stamp 0`). Every other machine joins by mutual vouching on the next card refresh. Both humans press Accept once.
- A side that is a single engine is a set of one.
- Family stays refused both ways (dial M/net.rs:839-844, gate M/net.rs:2029-2032). A contact machine that joins our mesh marks its record `problem: "now one of your machines"`. The record stays denied until it is removed.
- Both meshes changing at once: rows are immutable events under `pairing::newer`, so the order of arrival does not matter.

### D7. Fail-closed, path by path
These hold as built (190 D6): S1 `reinstall_contact_limits` fallback (N:3192-3237), S2/S10 `full_ceiling` (F:575-592, S:283-302), S3 revoked binding at actor start, S4 `apply_contact_limits` (N:3296-3320), S5 `drop_unrouted` (F:507-518), S6 persona refusal (N:3047-3066), S7 resume cap (`crates/agent/src/loop_.rs:1701-1771`, `crates/agent/src/store/durability.rs:1085-1112`), S8 no actor without limits (`crates/agent/src/supervisor.rs:1104-1109`), S9 prompt (`loop_.rs:4949-4964`). The new paths:

| Path | Stays closed by |
|---|---|
| Boot before the replica's first sync | Gate and routes closed until `mesh::synced()` (D2). |
| Boot before the roster joins (agent inbox) | Unchanged S6 on the inbox's owner. `theuth`'s inbox keeps its stored ceiling; any other agent is refused. |
| Missing or invalid agent file | Unchanged S6/S2, read on the owner. |
| Revoked binding | Revoked on every machine by the reconcile. A binding unknown here resolves as missing, which is capped (S3). A replica never un-revokes (D2). |
| Removal or block while a machine was down | Sync gate, then the first reconcile runs `revoke_peer_bindings_except` (S:1134-1162) over the synced tables. |
| Removal applied on the receiving machine but not yet on the owner, or the reverse | Both check: the receiving gate, then the owner's own gate on `a2a_forward`. The stricter view wins. |
| `a2a_forward` from outside the mesh | Refused without `Forwarded(peer)`, which `serve` sets only for paired engines (`crates/rpc/src/forward.rs:485-505`). |
| Owner offline or unknown | Refused at once (D3). `session_owner` errors refuse (N:1641-1650). |
| Invalid or conflicting replicated row | Row quarantined and its endpoints denied; the rest still stage (D2). |
| Members statement | Only from a live endpoint; additions need mutual proof; stamps bounded by `MEMBER_SKEW_MS`; no self-revival (D1). |
| An endpoint live in two unmerged records | Denied in both until the merge. |
| Merged-away record | Binding revoked; inboxes capped (S3/S5). |
| Inbox moved to another machine | Capped until re-admitted full there (D4). |
| Resumed turn after a take | The journal travels: `turn_steps` is a session-log table (capture.rs:28). A missing row still caps (durability.rs:1103-1105). |
| Grant change still replicating | Bounded by replication latency. The receiving and the owning machine each check their own copy, and a hold is released only by the owner's reconcile. |
| First-contact hold on another machine | Released only by its owner's reconcile; an expired summary row is ignored. |

### D8. UI (P4)
Contacts appear once, on every machine, because every RPC reads the replicated tables. Theme palette only: tokens in `UI/styles.css` (light :14-38, dark :69-93, Tailwind :232-256). No emojis.
- **Network › Contacts list** (`UI/components/settings/Contacts.tsx`: `ContactsTab` :134, `Contacts` :147, row :175-232):
  - under the name, one line: `Paired on <machine>` (from `Contact.paired_on`);
  - `Their machines: laptop · studio`, with a left machine struck through and the word "left" in `text-muted-foreground`;
  - `PathChip` (:54-62) stays and means this machine's path to the contact;
  - a row with `problem` shows it in `text-destructive`, with **Remove** as the only action.
- **Remove / Block confirm:** "… on all your machines".
- **Network › Machines unpair dialog:** one more line: "Your contacts are told this machine left. Until a contact hears from another of your machines, this machine can still reach it."
- **Access** (`ContactAccess` :688): unchanged. The picker already lists chats on other machines with their engine label (`useShareable` :679).
- **Share dialog** (`ShareDialog` :828, `SharingItems` :808-822): unchanged, except `useGrant` stops sending `modes` (190 UI note 3).
- **First-contact:** `ContactAllowStrip` (`UI/components/chat/ContactAllowStrip.tsx:28-31`), Attention `ContactFirstScreen` (`UI/components/attention/PlainScreens.tsx:152-168`) and the HUD (`UI/components/hud/Hud.tsx:179`) are unchanged components. Their data is now mesh-wide, so a chat on xeus shows its strip on 333.
- **TUI, Slack, Signal, Telegram:** only `/a2a` status exists (`crates/tui/src/model.rs:7189-7193`). It lists the replicated contacts unchanged; no new surface.

## Contract
| Item | 191 |
|---|---|
| `Contact` (ts) | + `id` (record id), + `paired_on: string \| null` (machine name, via `mesh::Status` peers as `session_move` does, `crates/rpc/src/api.rs:5400-5414`), + `machines: ContactMachine[]`, + `problem: string \| null`; − `cap`. `path` stays (this machine's). |
| `ContactMachine` (new ts) | `{ eid: string, name: string, left: boolean }` |
| `ContactPatch` | − `modes` (190 kept it "until 191"), − `cap` |
| `FirstWaiting` | unchanged shape; rows come from `contact_holds` (every machine) |
| `ContactEnvelope`, `ContactOrigin.{cap,tools,holder}` | removed (ts row of `ContactEnvelope` at `crates/rpc/src/export.rs:87` goes) |
| RPC `contact_deliver` | removed |
| RPC `a2a_forward` (mesh-internal, `Forwarded` only) | `{ caller: eid, method: "GET"\|"POST", path, body } → { status, body }`; the owner re-gates the caller |
| RPCs `contact_list`, `contact_update`, `contact_remove`, `a2a_exports`, `a2a_export_put`, `a2a_export_remove`, `contact_first_answer`, `contact_first_waiting` (`crates/rpc/src/api.rs:2648-2696`, impls :9547-9625) | same names; read and write the tables, so they answer mesh-wide; writes to a merged contact touch every record of its group |
| Card | + extension `https://theuth.io/a2a/ext/members/v1` (D1); omitted when empty |
| A2A task id | `<16 hex engine id prefix>-<uuidv7>` |
| Migration | **`0059_mesh_contacts.sql`**: the seven tables of D2 plus `contact_addrs`; `contact_threads` rebuilt (new table, copy with `id = binding\|context_id`, drop, rename; index kept). 0058 is skipped on purpose: another branch may hold it for `image_refusals`. Highest today: `0057_inbox_ceiling.sql`. |
| Attach | `replicate_records` (`crates/agent/src/store.rs:913`) + `contacts`, `contact_endpoints`, `contact_grants`, `a2a_exports`, `contact_inboxes`, `contact_holds`, `contact_threads` |
| `peers.yaml` | after import: HTTP peers only. Structs unchanged, so an old file loads. |
| Config | − `a2a.contact_cap` (`A2aConfig` is not `deny_unknown_fields`, A/config.rs:27-28, so an old `config.yaml` still loads). No new key. Constants: `MEMBER_SKEW_MS` = 10 min, members cap 64. |

## Phases
Order differs from the recon's (P2 routing, P3 siblings): **endpoint sets (P2) before routing (P3)**. With sets first, a session on any machine dials the contact with its own key. Routing first would need an outbound relay through the pairing machine, code P3 would delete again and which the standing rule rules out. Each phase lands alone; three phases would make one phase of about 2,000 production lines and one review round too large.

| Phase | What | Prod | Tests | Proof |
|---|---|---|---|---|
| **P1 RECORDS** | 0059; attach; import + pointer merge; stage from tables with per-row quarantine; reconcile on the digest tick; sync gate; binding id replicated, never un-revoked; contact RPCs write tables; inbox fail-closed fix (D4); `contact_holds` | 1,300–1,600 | 1,000–1,200 | Rig: contact paired on n1 is listed on n2; block on n2 refuses at n1's gate; n2 stopped, block on n1, n2 restarted: refused before and after sync; two machines' `peers.yaml` with the same contact merge into one row. |
| **P2 MEMBERS** | Members extension; notice on `peers_gen` and at boot; mutual proof; immutable endpoint rows; gate over the set; dial over the set; per-endpoint hints | 900–1,100 | 700–900 | Rig: outbound from n2 works without n2 ever pairing; n2 removed: C refuses n2 within one notice; hijack (a member listing a stranger's endpoint) refused. |
| **P3 ROUTING** | `a2a_forward`; owner resolution; offline refusal; `~return` and task-id forwarding; inbox home; drain completes tasks; **W-40 deleted** | +800–1,000, −600–650 | +800–1,000, −360 | Rig: chat on n2 answers C paired on n1; n2 off: refused at once. |
| **P4 UI and docs** | D8; site docs; as-built | 400–600 | 400–500 | Vite fixture screenshots, plus the rig for all five scenarios below. |
| Total | | ≈ 3,400–4,300 gross, ≈ 2,800–3,650 net | ≈ 2,900–3,600 gross | |

## Tests and proof
**Rig:** three scratch engines on loopback, discovery off, a fake model server (the 186 rig, `/tmp/d186/rig`, if present). `n1` and `n2` are paired as one mesh; `C` is a lone engine paired with `n1` only as a contact. C's fake model answers a `<message …>PING n` with `send_message(to=<from>, reply_to=<id>, PONG n)`.
1. **Chat on n2 answers a contact paired on n1.** n2 owns chat X; X is shared to C from n1's UI. C sends `PING 1`, dialing n1. n1 forwards to n2; X's transcript on n2 shows `<message from="peer:…">`, never `<contact_message>`. n2's agent answers; C receives `PONG 1`. The task id's prefix names n2, and C's `GetTask` poll is forwarded to n2.
2. **Outbound from n2.** A session on n2 sends `send_message(peer:c/<export>)`. It dials C with n2's key (C admits n2 through n1's vouching and n2's card), and C's answer comes back through `~return` to the session on n2.
3. **n2 off.** Stop n2. C's message to X reads "the chat's machine is offline" in under 1 s; nothing is queued.
4. **n2 removed.** `mesh_remove(n2)` on n1. Within one notice, C's record shows n2 left, and C refuses n2's stream (log: gate refused). n2's send to C fails. C's sends go to n1.
5. **Block everywhere, one machine offline.** Stop n2; block C on n1; C's message to n1 is refused at the gate. Start n2: C's dial to n2 is refused while n2 is unsynced, and stays refused after sync, now as blocked.

**Unit tests (one per plausible regression):**
- **P1**
  - a malformed row next to a block: the block applies;
  - a stale boot: gate closed until synced, then refused;
  - a revoked binding id arriving from the replica stays revoked;
  - a moved inbox runs capped;
  - an import merges by eid and keeps the local binding;
  - strictest-wins over a merged group;
  - an alias collision converges.
- **P2**
  - a statement from a non-member is ignored;
  - a future stamp is ignored;
  - a replayed old card changes nothing;
  - an add without mutual proof is refused;
  - a removal applies without proof;
  - no self-revival;
  - an endpoint live in two records is denied.
- **P3**
  - `a2a_forward` without the marker is refused;
  - the owner refuses a caller its own copy blocks;
  - an owner not connected is refused with no wait;
  - `~return` is forwarded by thread;
  - a drain completes waiting full tasks;
  - an answer from n1 releases a hold on n2.

The S1–S10 tests stay.

## The recon's "Not verified" list, resolved
- **The journal after a take:** it travels. `turn_steps` is a session-log table (capture.rs:28), and resume reads it on the new owner (`crates/agent/src/supervisor/resume.rs:165-171`, :198-216). S7 holds, and an empty journal still caps.
- **`contact_card`'s pause check for a remote chat:** it already answers from the replicated row (`archived`, `sessions.communication`: N:3004-3024, `crates/agent/src/supervisor.rs:1206-1219`; called at A/contacts/offers.rs:211-237).
- **Discovery with one key on two machines:** it would flap (D1).
- **TUI, Slack, Signal, Telegram:** `/a2a` status only, no contact commands (D8).
- **Where the tables live:** `sessions.sqlite` (D2).
- **Found besides:**
  - the moved-inbox fail-open (D4);
  - replica INSERT failures are permanent and silent, so no UNIQUE on replicated tables (D2);
  - `mesh::synced()` is a one-shot latch, bounded at 30 s and 150 s;
  - block also sends `Revoke` (A/contacts.rs:1702-1732), and the receiver only logs it (`A/contacts/ceremony.rs:148-151`);
  - W-40 is about 600–650 production lines, not 420;
  - `frame.rs` has no chat variant of the contact prompt left, only comments.

## Risks and open questions
- **Risk: the removed-machine window (D1).** Bounded by the first remaining machine that reaches each contact, and the same window 183 accepted for phones. A malicious leaver can evict, never gain access.
- **Risk: conversations end once at the merge.** One-time; the next message starts fresh.
- **Risk: contention.** Replica apply takes `BEGIN IMMEDIATE` on the single `sessions.sqlite` write connection (R/apply.rs:454). The rows are tiny and the writes rare. Watch it in the P1 rig.
- **Open questions for Sky:** none. The five answers settle every fork this design meets.
