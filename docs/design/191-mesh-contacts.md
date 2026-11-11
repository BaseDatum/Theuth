# 191 — MESH-CONTACTS: a contact is a contact for every machine of the mesh

Status: contract, review round 1. Written at main d10feaa8, after 190 landed. Recon: `/tmp/d191/recon.md` (written at 57840336). Every path:line below was re-read at d10feaa8.

Short names: **A** = `crates/a2a/src`, **M** = `crates/mesh/src`, **R** = `crates/replica/src`, **N** = `crates/agent/src/communication/network.rs`, **F** = `crates/agent/src/communication/network/full.rs`, **S** = `crates/agent/src/store/communication.rs`, **UI** = `crates/desktop/ui/src`.

## Sky's words (2026-11-11)
- **14:23:** "shouldn't contact links just be replicated across the mesh and durable. this seems overcomplicated right now. A contact is a contact for every node in the mesh, not just a single node."
- **14:59**, five answers:
  1. When a machine leaves the mesh: "Cut off that machine only". Contacts learn of it through the 191 protocol. This must hold even when that machine misbehaves.
  2. A contact on an old build: "we don't plan for this. we are developing now, not with a user base." **This is a standing rule:** no compatibility for older theuth builds. No relay for un-updated contacts, no version floors, no dark launch. Data this machine's previous build wrote must still load.
  3. When a contact's mesh adds a machine: "Yes, automatically", vouched by a machine already trusted.
  4. When a contact writes to a chat whose machine is off: "Refuse at once".
  5. Existing per-machine contacts: "Merge automatically".
- **Earlier that day:** contacts are for "intersession communication once a contact is trusted, and a session is shared … this is supposed to make things easier, not be frustrating."

## Today, in one paragraph (verified)
- **A contact is one engine's private state:**
  - the file `a2a/peers.yaml` (A/config.rs:20);
  - one pinned endpoint id per entry (`PeerEntry.endpoint`, A/config.rs:154; "one contact per endpoint", :578-582);
  - a gate map `Live.contacts: eid → Principal` (A/server.rs:116, built at :187-191);
  - a client that dials one eid (`Link::Iroh`, A/client.rs:337-342);
  - local tables: `peer_bindings`, minted per engine (`Uuid::now_v7()`, S:933), plus `a2a_*`, `peer_exchanges` and `contact_threads`. None of them is in the session log (`crates/agent/src/store/capture.rs:4-5`).
- **Sending from another machine fails.** A session on another machine gets "unknown peer" when it messages a contact (N:4035).
- **W-40.** When a contact messages a chat that another machine owns, the message takes the W-40 path: it is forwarded, runs one capped turn there, and only the final text comes back (N:3680-3964).

## Decisions

### D1. Identity: each machine keeps its own key; a contact holds a set of endpoints
- **Why there is no shared key.** One Ed25519 key does three jobs: it terminates TLS for every ALPN (M/net.rs:537-546), it signs the card (M/net.rs:945-947; `kid` = `net.id`, A/contacts.rs:826-844), and it is the mesh engine id (M/identity.rs:1-41). That key cannot be live on two machines:
  - The relay keeps one active client per endpoint id. A second one demotes the older with `SameEndpointIdConnected` (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iroh-relay-1.4.1/src/server/clients.rs:81-94`). The lock pins iroh-relay 1.4.1 (`Cargo.lock:9869`), not the 1.2.0 the recon read.
  - Discovery would flap. pkarr keeps the newest signed record (`iroh-dns-1.4.1/src/pkarr.rs:270-276`), and the mDNS cache keeps the last announcement per id (`iroh-mdns-address-lookup-0.6.1/src/lib.rs:566-578`). Both run when `mesh.discovery` is on (M/net.rs:752-787).
  - A shared signing key would also outlive a removed machine. The contact's pin is external, so a leaked key means every contact must pair again.
- **Decision: option (c).**
  - Each machine keeps its own `mesh/identity`.
  - A contact record holds the **set** of the other side's endpoints.
  - Grants, first-contact answers, block and removal belong to the record, never to one endpoint.
  - No secret is involved, so nothing has to rotate.
- **The statement is a signed card extension.** This adds no new route and no new signature scheme.
  - Every card carries the extension `https://theuth.io/a2a/ext/members/v1`. Its `params` hold `at` and `members`, the signer's mesh as it sees it. The signer leaves itself out. `at` is the signer's clock when the list content last changed. The signer keeps the list's hash and its `at` in memory, and bumps `at` to `max(now, at + 1)` on every change of content, a quarantine included (a quarantine stamps no `peers` row, M/store.rs:152-168). At boot `at` = now.
    ```json
    { "uri": "https://theuth.io/a2a/ext/members/v1",
      "params": { "at": 1762850000000, "members": [
        { "id": "<endpoint id hex>", "name": "xeus" },
        { "id": "<endpoint id hex>", "name": "old-mac", "removed": true } ] } }
    ```
  - **What the list holds.**
    - Rows come from the mesh's `peers` table (`mesh::pairing::PeerEntry`, M/pairing.rs:97-129).
    - **Every live row** of the paired set is listed, uncapped: the mesh's real size, sandbox VMs included, since they are mesh peers. Quarantined rows are left out (M/store.rs:152-168).
    - Tombstones are listed as `removed`, the newest 64 only. Tombstones never expire, so a cap that counted live rows would one day drop them.
    - A tombstone that falls off the list stays in force, because accusations are sticky at the reader (next bullets).
    - Names are clipped to 64 characters.
  - **Signature.** It uses the card's existing JWS. Extension `params` survive parsing and are covered by the signature (A/contacts.rs:473-481, :621-631).
  - **The card is only ever fetched from its signer.** It travels over the signer's own TLS-authenticated stream and is checked with `kid == eid` (A/contacts.rs:587-632). Cards are never relayed, so an old card cannot be replayed by anyone else.
  - **Write order.**
    - Our machines fold each fetched card into the voter's *vote*. Live entries come from the newest card. `removed` entries stay from every earlier card until the same voter lists that endpoint live again. A tombstone the signer has trimmed therefore never withdraws an accusation.
    - The fold is written as an immutable vote row, keyed by voter and `at`, and **only when the folded content differs** from the voter's current vote. Rows replicate, so this avoids churn.
    - The reader uses each voter's highest `at`. `at` is clamped to the reader's clock plus `MEMBER_SKEW_MS` (10 min), so a future stamp cannot outrank the same voter's later cards for long.
  - **A card without the extension** means "no new vote": the voter's previous vote stands. It never withdraws everything.
  - The extension is omitted only when the list is empty, so the JWS test vector (A/contacts.rs:2254-2341) keeps its value.
  - **Whose cards are fetched.** The anchors, every endpoint that ever had a vote row in the group, and every endpoint named (live or removed) in a vote, up to 64 per group. Denied endpoints and non-members included.
  - We dial them outbound with our own key, so the gate's refusal of an inbound stream from them (reach.rs:161-163) does not stop the fetch.
  - They are fetched on notice, at boot, and once per `OFFERS_TTL` (60 s, N:327-328).
- **Membership is computed from the votes on record, and it fails closed.** The same algorithm runs on both sides and on every machine, per contact group (D2 merge).
  1. **Voters** are every endpoint of the group with a vote row, except barred ones (step 6).
  2. **Accusers of X** are the voters whose current vote lists X as `removed`. Every voter counts, members or not, so an anchor that cut the honest machines out of its list is still accused by them.
  3. **Uncontested (U)** are the voters with no accuser.
  4. **An accusation does not count** if its author has an accuser in U.
  5. **Denied** is every endpoint with a counting accusation, plus every barred endpoint. This step does not depend on membership, so it needs no iteration.
  6. **Anchors** are the endpoints a human pinned in a ceremony (`contact_endpoints`). A ceremony into a record adds its endpoint as an anchor and **bars every anchor that is denied at that moment**. Barred anchors keep a row with `barred_ms`. Their votes no longer count, and they are never live again in this record. A live anchor stays. So a re-pair after "Pair again" replaces a rogue or contested anchor.
  7. **Members (M)** start as the non-denied anchors. Add Y, if Y is not denied, when some E in M lists Y as live in E's current vote *and* Y's current vote lists E as live: both sides have signed. Repeat until M stops changing. This is answer 3 ("automatically, vouched"). It is also why contact A cannot claim contact B's machine: B's card would have to list A. Omission withdraws a vote: Y stays only while a current vote of a member, or an anchor, holds it. An endpoint vouched for only by denied endpoints drops out.
  8. **Live** = M. The gate admits only these.
- **What the rule gives (answer 1, fail-closed).**

| Case | Result at the contact |
|---|---|
| An honest machine is removed (it never learns this: M/net.rs:1272-1297 sends nothing) | The remaining machines accuse it, and nobody accuses them, so it is denied. Its own vote stops counting. Anything only it vouched for drops out. |
| A rogue member or anchor R accuses every honest machine, or omits them | Once R is removed from the mesh, every honest machine's card accuses R, and those cards are fetched even though they are not members. If R accuses them back, U is empty and all are denied. If R only omits them, U holds them and R is denied. Either way R is denied. If R was the only anchor, nobody is live until a fresh ceremony from an honest machine, which bars R and admits the rest through their votes. R never keeps access. |
| Contested: A accuses B and B accuses A, and an uncontested C also accuses B | B's accusations no longer count, so A is live. B is denied. |
| Contested with no uncontested member | Both are denied. A fresh ceremony bars the denied anchors, and the new anchor's mesh is admitted through the votes again. |
| An endpoint is omitted from a list (not marked removed) | That voter has withdrawn its vote. The endpoint stays a member only while another current vote, or an anchor, holds it. |
| A machine is re-paired into its mesh | Its `peers` row is live again, so the next cards list it live. The accusation is lifted, because the same voters now list it live, and it is a member again. |
| A machine is cut wrongly (transiently) | It cannot notify, because the gate refuses it. It is restored at the contact's next card refresh: at most `OFFERS_TTL` (60 s), sooner if another live member notifies. |
- **How our machines tell contacts.**
  - The a2a actor awaits `Net.peers_gen.changed()` (M/net.rs:460-461). That watch is bumped inside `refresh_paired` (M/net.rs:1322), which `remove_peer` calls (M/net.rs:1275-1297).
  - So the machine where the user pressed Unpair notices at once, not on the 2 s tick. The others notice when the tombstone arrives: by the replica, by `KeyGrant`, or on the 5 s `keys::tick`.
  - On every bump, and once at boot, each machine POSTs `NOTICE_PATH` (A/contacts/offers.rs:25; routed at A/contacts/reach.rs:211-246) to every contact. It retries every 2 s until one post succeeds.
  - The contact then fetches the cards of every member of our group and writes the votes.
  - Nothing is persisted, because the boot pass sends everything again.
  - The contact also reads members on every card refresh: at pairing, at boot, on notice, and at most once per `OFFERS_TTL` of 60 s (N:327-328).
- **The window.** Until a contact applies the new votes, a removed machine can still pass as us to that contact.
  - While the contact is online, that lasts seconds: one notice plus one card fetch.
  - While the contact is offline, the window has no bound: it stays open until the contact comes back and fetches a remaining machine's card. (Until then nobody reaches the contact at all.)
  - Within the window a rogue machine can do no more than it could the second before it was removed. Once the votes are applied it is denied, as in the table above, and it never keeps access. The worst case is that the contact must pair again.

### D2. Replication: last-writer-wins tables in `sessions.sqlite`
- **Mechanism.** `replica::attach`: per-column LWW under HLC, tombstones, version-vector catch-up (R/lib.rs:1-14, :104-121; R/sync.rs:1-14). The record tables are appended to `replicate_records` beside `projects` and `cost_owners` (`crates/agent/src/store.rs:903-915`, attach at :913).
- **No floor gate is needed.** A machine without these tables parks their batches: up to 65,536 change sets for up to 7 days, replayed at attach (R/apply.rs:27, :29, :75-93).
- **`SESSIONS_WIRE_SCHEMA` is set to the number of 191's own migration**, for the inbox marker (D4). That is 59 today: the rule is "set this to a new migration's number" (`crates/agent/src/store.rs:659-670`; test text at :4249-4253).
  - Any other value is wrong. With 54, an unrelated 0058 bump would read as newer, and this build would ship `contact_inbox` rows to a peer that lacks the column. `sqlite3changeset_apply` would skip the table while the cursor advances (`crates/sessionsync/src/apply.rs:80-109`): silent loss.
  - Another branch adds `0058_image_refusals` and lands first. 191's migrations are 0059 and 0060; if that changes at landing, the bump follows 191's own migration number.
  - Mixed versions are not a concern (standing rule). `store::tests::sessions_wire_schema_tracks_the_replicated_columns` snapshots the 20 tables of `capture.rs:19-40`.
- **Rules the schema obeys (verified).**
  - Each table has one primary-key column plus `hlc TEXT`. Composite keys are refused (R/engine.rs:617-646, :636).
  - **No UNIQUE, CHECK or FK constraints.** A failed INSERT of a remote row is skipped silently and forever (R/apply.rs:615-631). A failed UPDATE parks that table's slice of the batch (R/apply.rs:102-121).
  - **Removal is a column, not a row delete.** A delete wins only if it is newer than every column write. Any later column write revives the row (R/apply.rs:521-546).
  - Uniqueness and validity are enforced in the reconcile.
- **Tables (migration `0059_mesh_contacts.sql`).**

| Table (replicated) | Key | Columns | Written by |
|---|---|---|---|
| `contacts` | `id`: UUIDv7 minted at pairing | `alias`, `name`, `machine`, `binding` (the `peer:<uuid>` id), `paired_ms`, `paired_on` (the engine id of our machine that ran the ceremony), `blocked` (0/1), `removed_ms` (tombstone, never cleared), `merged_into` (record id, or NULL) | pairing, every contact RPC, import, merge |
| `contact_endpoints` | `<contact>\|<eid>` | `contact`, `eid`, `paired_ms`, `barred_ms` (NULL = anchor) | the ceremony only: anchors, and the anchors a later ceremony barred (D1 step 6) |
| `contact_votes` | `<contact>\|<voter eid>\|<at>` | `contact`, `voter`, `at`, `members` (the folded vote, JSON) | any of our machines after fetching a card, and only when the fold changed (D1). Rows are **immutable**, so LWW cannot roll a vote back. Older rows of a voter are deleted by the reconcile. |
| `contact_grants` | `<contact>\|<export id>` | `contact`, `export`, `granted` (0/1), `first_contact` (`ask`/`allow`/`block`; NULL = ask) | contact_update, contact_first_answer |
| `a2a_exports` | `id`: UUIDv7 minted when the export is made | `name` (the card skill and the `/agents/<name>` path; made unique by the reconcile), `session`, `agent`, `title`, `description`, `home` (engine id), `removed_ms` | a2a_export_put and a2a_export_remove, import |
| `contact_threads` (rebuilt) | `<binding>\|<context_id>` | the 0056 columns: `binding`, `context_id`, `session`, `export`, `outbound`, `last_ms` | as today (S:1591-1676) |
| `contact_holds` (P3) | the A2A task id | `contact`, `machine`, `export`, `session`, `owner` (engine id), `since_ms`, `waiting`, `preview` (clipped to 200 characters), `until_ms` | the owner only. The row is deleted when the hold ends (D4). |

- **Local tables (not attached) and in-memory state.**
  - `contact_addrs(eid PK, addrs, at_ms)` holds the direct addresses this machine saw. It is rewritten at most once a minute (A/contacts.rs:1990-2046). These differ by LAN, so they would flap if replicated.
  - `peer_bindings`, `a2a_contexts` (with `inbox_full`, `inbox_ceiling`), `a2a_tasks` and `peer_exchanges` are unchanged.
  - The card cache, held tasks and ceremony state stay in memory.
- **What stays per machine, and why.**
  - Inbox fail-closed flags and ceilings: a replicated "full" could outlive the live roster ceiling it was read from.
  - Tasks and exchanges: they live where the message was admitted.
  - Addresses.
  - Inert fields are dropped at import: `cap`, `modes`, `ExportEntry.tools` (A/config.rs:161-171, :213-216).
- **Secrets: none travel, none to lose.**
  - A contact entry may not name a vault secret (A/config.rs:568-577).
  - A machine that leaves keeps copies of public rows and its own key. It is cut off by the votes (D1), not by a rotation. The vault is untouched.
- **Binding identity.**
  - The record's `binding` column is the `peer:<uuid>` everywhere.
  - `peer_bindings.endpoint` becomes `contact:<record id>`. It is constant for a record, so endpoint-set changes never revoke.
  - `bind_peer_in` (S:905-946) takes the id instead of minting it. It works in three cases:
    - A live row with that id has its alias updated in place, so a rename no longer revokes.
    - Otherwise, the alias's live row is revoked and the id is inserted.
    - An id already revoked here **stays revoked**, and its record is quarantined on this machine. A replica never un-revokes.
  - Unblock and re-pair write a fresh `binding`.
- **Reconcile: one pass.**
  - **Trigger.** On the existing 2 s a2a tick (A/lib.rs:712), hash the full content of every row of the staged tables. There are five in P1–P2 (`contacts`, `contact_endpoints`, `contact_votes`, `contact_grants`, `a2a_exports`) and six from P3 (+ `contact_holds`). `contact_threads` is read per request and never staged. The tables are tiny.
    - A row's `hlc` is not enough. It is the max over its columns (R/apply.rs:582-604). A remote column write wins per column (R/apply.rs:558-562), so the row can change without its `hlc` changing.
    - Content hashing also catches parked batches applied later.
    - The pass also runs right after every local RPC write.
    - Hashing rather than `ReplicaHandle::subscribe`, because late parked applies are never published on the feed (R/engine.rs:748-750; R/apply.rs:275-361).
  - **Steps, in order.**
    1. Build `Staged` from the tables, one row at a time.
    2. Compute membership (D1).
    3. Commit routes (`CommitPeerConfig`, A/lib.rs:553-556 → N:2843-2916).
    4. Revoke every binding outside the set, and cap its inboxes (`drop_unrouted` → `revoke_peer_bindings_except`, F:507-518, S:1134-1162).
    5. Release holds whose grant changed: allow → start; block or removal → deny (N:3497-3528).
    6. Close the connections of every endpoint that is no longer admitted.
- **Per-row quarantine.** A bad row never freezes `stage()`, and a block always applies.
  - Today `stage` is all-or-nothing (A/config.rs:625-809; `Err` at :804-808; caller A/lib.rs:388-389).
  - The new stage validates each record alone. A bad record is skipped and reported as `Contact.problem`, and **its endpoints go into a deny set** that the gate checks first.
  - A record is bad when it has:
    - a malformed alias or eid;
    - a grant that names a missing export;
    - a `binding` that is revoked here;
    - an endpoint that is one of our own mesh machines (family);
    - an endpoint that is live in two unmerged records.
  - Blocked and merged-away records only ever add to the deny set.
  - A removed record's endpoints stay denied until a newer ceremony pins them in a live record (an anchor with `paired_ms` greater than its `removed_ms`). A re-pair therefore works.
  - **Uniqueness by reconcile.** Two records with the same alias, or two exports with the same name: the older `(paired_ms or id, id)` keeps the name, and the reconcile writes `<name>-2` into the newer one. Every machine computes the same value, so the writes converge.
- **Admission waits for the mesh's latch and the first reconcile (Sky, 17:59: option (b)).**
  - `mesh::synced()` is the bounded latch task work already uses. It flips true when:
    - every *connected* peer has caught up; or
    - 30 s pass (`lapse_secs`) with no peer connected; or
    - 150 s pass at worst (M/lib.rs:1478-1520).
  - It cannot be used raw. `SYNCED` starts true (M/lib.rs:126), and the mesh sets it false only at the end of its start, when this machine has paired peers (M/lib.rs:1481-1482). The mesh starts before a2a (`crates/theuth/src/main.rs:2023`, :2034).
  - So contacts get their own flag, `contacts_ready`. It **starts false**.
  - It flips only after `mesh::synced()` is true **and** the first contact reconcile has finished all six steps: the revokes, the inbox caps and the closes, on the post-latch tables.
  - **Until `contacts_ready`:**
    - the gate (`ContactAlpn::admit`, A/contacts/reach.rs:116-121; `iroh_gate`, A/server.rs:916-935) refuses every contact;
    - no route is committed, and outbound sends read "waiting for your other machines";
    - every inbox actor starts capped (D7), whatever its local `inbox_full` and binding say.
  - Only after the flip does `refresh_full_inboxes` (F:524-562) lift the caps that are still due. No actor can run full from a stale local binding in the gap between the latch and the reconcile.
  - A machine with no paired peer is latched at once; it reconciles, then it is ready.
  - **The accepted cost** is stated under Risks.
- **Merge (answer 5): a pointer, never a copy.**
  - **What counts as the same contact.** Two live records that share a member, under the vote rule of D1. This covers two of our machines having paired the same machine (one eid in two records' anchors).
  - **The survivor** is the lower `(paired_ms, id)`.
  - **The write.** Each machine writes `merged_into = <root>` on every other record of the group. The root is the group's survivor, so the value is the same from every machine. Pointers always point at a lower key, so chains (C→B→A) resolve by following them to the record whose `merged_into` is NULL. The reconcile rewrites every pointer to the root. A chain that does not end within 16 hops, or that loops, quarantines the whole group.
  - **The effective contact.**
    - Endpoints: union over the group.
    - Blocked: true if any record is blocked.
    - Alias and binding: the survivor's.
    - Grants are taken per export over the records **that have a row for it**. A record with no row abstains, so a grant held by only one duplicate carries over.
    - Among the rows present: `granted = 0` beats `granted = 1`, and `first_contact` takes the strictest, `block` > `ask` (NULL) > `allow`.
  - **Why not copy.** Copying values instead would race: a machine missing a `block` could write `allow` last.
  - **Edits** to a merged contact write every record of the group that has a row, so the strictest read never pins an old value. A grant that no record of the group has yet (the group's first grant for an export) is written to the survivor.
  - **What a merge ends.** Merged-away bindings are revoked, so their inboxes are capped (S3/S5). Their open conversations end ("the conversation with … ended", F:157-268), and the next message starts fresh.
- **Import.** Data the previous build wrote must still load.
  - **When:** at the first reconcile after `contacts_ready`. Each machine imports its `peers.yaml` contact entries (`is_contact()`, A/config.rs:184-195) and its exports.
  - **The file still parses as is:** `PeersFile` and `PeerEntry` stay `deny_unknown_fields` (A/config.rs:111, :138).
  - **Matching a contact.** If any record (live, blocked or removed) has an anchor for the same eid, the entry attaches to the newest such record:
    - The entry adds only grants that record lacks.
    - It never writes `blocked`, `removed_ms` or `binding` on reuse.
    - It never revives a removed record. A file left behind on a machine that was down skips that entry and logs it.
  - **Creating a contact.** With no match, a new record is made. Its `binding` is this machine's live `peer_bindings` id for that alias, so its history keys go on. `blocked` is copied from the file.
  - **Exports.**
    - A file export with the same name and the same target as an existing row reuses that id.
    - Otherwise it gets a new id with `home` = this machine. The name rule then makes it unique.
    - Grants map export names to ids through this machine's own file.
  - **Inbox markers.** Local `a2a_contexts` rows with `inbox = 1` set `sessions.contact_inbox` on the sessions this machine owns (D4). The marker cannot be set on an inbox that has already moved, because the session log has one writer, its owner. D4 covers that case.
  - **The file.** It is rewritten without contacts and exports; HTTP peers stay. A crash before the rewrite imports again onto the same rows.
  - **Threads.** The migration copies `contact_threads` rows into the rebuilt table, with the new `id`.

### D3. Routing
- **Inbound: any machine accepts, and the owner serves.** The gate admits a stream from any live member of an admitted contact. The router resolves the owner:
  - chat export → `a2a_exports.session` → `sessions.owner` (`session_owner`, `crates/agent/src/supervisor.rs:5937-5952`);
  - agent export → the session whose `contact_inbox = <binding>|<export id>` → its owner; with no inbox, the export's home (D4);
  - `~return` → `contextId` → `contact_threads` → session → owner;
  - `GetTask` and `CancelTask` → the engine named by the task id prefix (see below).
- **Serving.** When the owner is this machine, the request is served as today. When the owner is elsewhere, **`a2a_forward`** (see Contract) carries it over `theuth/rpc/1`, the way `peer_deliver` travels (`crates/agent/src/communication/mesh.rs:94-118`; marker `Forwarded(peer)`, `crates/rpc/src/forward.rs:126`, set by `serve` at :485-505).
- **What the owner does with a forward.**
  - It refuses a call without the marker, as `contact_deliver` does (`crates/rpc/src/api.rs:4492-4498`).
  - It checks `caller_eid` again against **its own** gate, including `contacts_ready`.
  - It serves the request in its own router with `ContactCaller(eid)` (A/contacts/reach.rs:178-209).
  - Admission, the first-contact hold, the A2A task, `deliver_full`, inbox limits and receipts all run on the machine that holds the session. There is no envelope, no capped turn and no clamp.
- **Owner offline: refused at once (answer 4).**
  - The receiving machine checks `transport.peers()`, the live control connections (M/net.rs:980-986). If the owner is not among them, the contact gets `ContactRefusal::OwnerOffline`, "the chat's machine is offline" (`crates/agent/src/communication.rs:1100`), with no wait (mesh.rs:176-178).
  - An owner that is still listed but dead gives the same text within the RPC timeout of 30 s (mesh.rs:32, :206-215).
  - If the chat moved meanwhile, the owner answers `NOT_OWNER` (-32013, mesh.rs:126), and the receiving machine resolves again, once.
  - `OwnerOld` goes, under the standing rule.
- **Outbound: from a session on any machine.**
  - Every machine commits a route for every admitted contact, so "unknown peer" (N:4035) goes for contacts.
  - The sender dials with **its own** key. `PeerClient::contact` (A/lib.rs:479-500) and `IrohTransport::call` (A/contacts/reach.rs:277-289) take the contact's live member set. They try first a member with a live connection, then the last one that answered, then the rest, all within `CALL_TIMEOUT` of 30 s (reach.rs:39).
  - A `NOT_OWNER` answer moves on to the next member.
  - Each machine keeps its own card cache (`full_shape` needs the offer, F:226-238) and its own hints (`contact_addrs` behind `ContactAlpn::hints`, reach.rs:137-149).
  - **No relay**, under the standing rule. A machine that reaches no member reads "cannot reach <contact> from <machine>". With the default relay (`crates/mesh/src/config.rs:62`) that only happens with `mesh.relay: none` and no shared LAN (M/net.rs:65-67).
- **The return route, and `contextId` across machines.**
  - The sender mints `contextId` for a fresh conversation (F:241-258). Its `contact_threads` row (outbound = 1) replicates.
  - The contact answers on `~return` at whichever of our members it dials. That machine resolves `(binding, contextId)` and forwards to the session's owner.
  - The owner runs `full_return` against its own thread row, binding and offers (F:621-742). `full_intake` enqueues locally (F:366-399), which is correct because the owner holds the session.
  - Inbound conversations (outbound = 0) work the same way.
  - A thread row that has not replicated yet reads `TASK_NOT_FOUND`, which fails closed. The replica polls every 200 ms (R/engine.rs:30), and an answer follows a model turn.
- **The owner check on the contact paths (needed from P2 on).** `enqueue_peer` (`crates/agent/src/supervisor.rs:2228-2322`) does not check the owner, and `Agents::session` (:925-1105) loads actors without one.
  - So `full_return`, `full_intake` and `deliver_full` refuse a session whose `sessions.owner` is another machine, answering `NOT_OWNER`.
  - Before P3, the contact's client moves on to its next member. From P3 on, the receiving machine forwards.
  - A session is never run here on a contact's behalf without this check.
- **Task ids name their machine.**
  - Today A2A task ids are UUIDv7 (N:1532), and `GetTask` reads the local `a2a_tasks` (N:2561-2580).
  - New ids take the form `<first 16 hex of the admitting engine id>-<uuidv7>`. `GetTask` and `CancelTask` are forwarded to the engine the prefix names.
  - An id with no matching prefix (one minted before 191) is served locally.
  - `ListTasks` answers this machine's tasks only. No theuth code calls it (no `.list_tasks(` caller in `crates/`).
- **A session taken away completes its waiting full tasks.** Nothing in the agent network reacts to an owner change today (`SessionDrained`, `crates/agent/src/supervisor/transfer.rs:223-244`).
  - On drain, the old owner completes the session's `full_inbound` tasks (N:625) as receipts: the message already sits in the queue that moved.
  - Held first-contact tasks are released through `start_or_queue` (N:3530-3575), which forwards them.

### D4. The agent-export inbox: one home, a marker that travels with the session
- **Home.** An agent export names its machine as `<agent>@<engine>` (N:3043-3045), stored as `a2a_exports.home`. For `theuth`, the home is the machine whose UI made the export; on import, it is the importing machine.
- **Only the home creates inboxes** (`handle_inbox`, N:3037-3140), so creation never races. A message for an agent export without an inbox is forwarded to the home. If the home is offline, the message is refused at once.
- **Marker: `sessions.contact_inbox TEXT NULL`** (`<binding>|<export id>`). The home sets it when it creates the inbox session. `sessions` is a session-log table (capture.rs:19-40), so the marker reaches every machine in the same changeset as the session row. A record-replica map would arrive on a different stream than the session and could lose a race with a take.
  - Routing reads the marker; there is no separate map table.
  - A deleted inbox takes its marker with it. The next message goes to the home, which makes a new inbox.
- **The moved-inbox fail-open.**
  - Today inbox caps live only in the local `a2a_contexts` (capture.rs:4-5). On a machine that took the inbox, `a2a_inbox_identity` finds nothing and `reinstall_contact_limits` returns `Ok(())` with no limits (N:3192-3195). `ContactScoped` then says false (`crates/agent/src/loop_.rs:4949-4964`).
  - The separate fix on main refuses `session_move` and the owner's release for inbox sessions. From now on, an inbox cannot move by a user action.
  - The case left is a Write take from an unreachable owner. P1's marker closes it: a session with `contact_inbox` set and no local `a2a_contexts` row runs **capped** (`INBOX_FAIL_CLOSED` and an empty allowlist, N:3146), with the contact prompt.
  - The owner re-admits the inbox full on the contact's next message. `handle_inbox` adopts the marked session, reads the live persona there (S6) and marks its local row (S:2030-2061).
  - **An inbox that moved before 191**, by a take, cannot be marked: only its owner writes its row. Its home still holds the old `a2a_contexts` row. When the contact's next message reaches the home and finds that row's session owned elsewhere, the home unmarks it and caps it there (`unmark_a2a_inbox_full`, S:2080-2088). It then opens a new, marked inbox locally, so contact messages never reach the moved session again.
  - The moved session itself is the residual caveat. It is now the taker's own isolated session and receives no contact input; until 191 lands, it runs as today.
- **Home removed.** An export whose `home` is tombstoned in the mesh `peers` table is quarantined with `problem: "its machine left the mesh"`.
  - It gets no new inboxes.
  - Existing inbox sessions that were moved elsewhere keep working through their owner.
  - The UI offers to rehome the export: `a2a_export_put` with `home` set to this machine.
- **First-contact Attention (P3).**
  - Holds stay on the machine that admitted the message: the owner (N:3406-3587, in memory).
  - The owner mirrors each hold as a `contact_holds` row, carrying the `FirstWaiting` fields (A/contacts.rs:107-134). It deletes the row when the hold ends, and deletes all its own rows at boot.
  - `contact_first_waiting` and the `contact_first` Attention source (A/contacts.rs:2088-2122, :2141-2196) read that table. They drop rows past `until_ms` (`HOLD_TTL` is 24 h, N:632).
  - An answer from any machine is a grant write. `contact_first_answer` already writes `first_contact` or `blocked` (A/contacts.rs:1927-1988). The owner's reconcile releases the hold.
  - Until P3, an unanswered hold on another machine just waits, which fails closed.

### D5. W-40 is deleted (P3)
`grep -rn "W-40 only; removed by design 191" crates` lists 11 markers. They and the unmarked W-40 code go:

| Goes | Where |
|---|---|
| `default_contact_tools` and its users | A/config.rs:219-223; A/server.rs:610; `A/contacts/e2e_tests.rs:443`, :670, :719 |
| `ContactOrigin.cap` and `.tools` stamping; `ContactOrigin.holder`, which only W-40 reads | A/server.rs:601-612; fields at `crates/agent/src/communication.rs:701-726` |
| `PeerMeta.cap` and `.tools`, `ContactFrame`, the turn-cap install for a new contact turn, the PeerInput frame branch, `contact_message()` | `crates/agent/src/loop_.rs:203-209`, :215-231, :1278-1291, :1784-1798, :8744-8757 |
| `network_contact_frame` (a contact task that is not local is forwarded, never run here) | N:3326-3351; called at `crates/agent/src/supervisor.rs:2287` |
| `CONTACT_GRANTABLE`, `contact_grantable` and their filter | `crates/tools/src/registry.rs:622-656` |
| `CONTACT_NOT_OWNER`, `ContactForwarded`, `Message<ContactDeliver>`, `Message<ContactForwarded>`, `forward_contact`, `contact_deliver` and its owner clamp, `contact_turn_finished` | N:3680-3964 |
| the W-40 fall-through in `start_or_queue` (a message that is not local is now forwarded) | N:1632-1654 |
| the W-40 branch of `start_task` | N:1729-1750 |
| the W-40 branch of `network_turn_finished`; `resumed_contact_envelope` | N:2310-2323; N:2346-2375 |
| `NetworkState.contact_waits` and `contact_replies`, with their test helpers | N:611, :614, :875-891 |
| `CONTACT_METHOD`, `ContactFailure`, `contact_forward` (the refusal mapping moves to `a2a_forward`) | `crates/agent/src/communication/mesh.rs:121-163` |
| `ContactEnvelope` and `ContactDeliver`, with their re-exports; `ContactRefusal::OwnerOld` | `crates/agent/src/communication.rs:728-758`; `crates/agent/src/lib.rs:163`; communication.rs:1086 |
| RPC `contact_deliver` | `crates/rpc/src/api.rs:247-251`, impl at :4487-4514; UI `rpc/api.ts:1085-1086` |
| `a2a.contact_cap`, the `PeerEntry.cap` readers, the `stage` parameter, the config template line | A/config.rs:43, :161-164; A/lib.rs:170, :388; `crates/theuth/src/main.rs:1259-1267` |
| W-40 words in comments | `crates/agent/src/frame.rs:500`, :519-520 |
| Tests | `serve_contact_deliver` and `an_exported_chat_elsewhere_runs_one_capped_turn_and_answers_only_it` (`crates/agent/src/supervisor.rs:16305-16591`); the W-40 halves of the tests at `loop_.rs:9948` and `registry.rs:3495` |

- **What stays.** The fail-closed states use these, so they are kept:
  - `SetCap`, `SetAllowOnly`, `SetTurnCap`;
  - `install_contact_limits`, `reinstall_contact_limits`, `clear_contact_limits`;
  - the Resume cap and the journal source `contact`: old journals and "unsure" still cap;
  - `contact_baseline` (`crates/agent/src/frame.rs:503-516`).
- **Size.** About 600–650 production lines and 360 test lines, not the recon's 420.
- **What replaces W-40.** `a2a_forward` (D3) carries the request itself, not a capped turn. D4 covers agent exports.

### D6. Mesh to mesh
- **Both sides run the same code.** Each side's record of the other holds the other's member set.
- **Any machine can dial any machine.** The receiving machine forwards to the owner (D3).
- **Bounds.** The rate and concurrency bounds are per principal (`admit_contact`, A/server.rs:290-302), so all members share them. Connection and stream budgets stay per endpoint (M/net.rs:47-49).
- **Pairing.** One ceremony runs between one machine on each side, and each side anchors one endpoint. Every other machine joins through mutual votes on the next card refresh. Both humans press Accept once. A side that is a single engine is a set of one.
- **Family.** A machine of our own mesh is still refused, both ways (dial: M/net.rs:839-844; gate: M/net.rs:2029-2032).
- **A contact that joins our mesh.** Its record shows `problem: "now one of your machines"` and stays denied until it is removed.

### D7. Fail-closed, path by path
These hold as built (190 D6):
- S1: the `reinstall_contact_limits` fallback (N:3192-3237);
- S2 and S10: `full_ceiling` (F:575-592; S:283-302);
- S3: a revoked binding at actor start;
- S4: `apply_contact_limits` (N:3296-3320);
- S5: `drop_unrouted` (F:507-518);
- S6: the persona refusal (N:3047-3066);
- S7: the resume cap (`crates/agent/src/loop_.rs:1701-1771`; `crates/agent/src/store/durability.rs:1085-1112`);
- S8: no actor without limits (`crates/agent/src/supervisor.rs:1104-1109`);
- S9: the contact prompt (`loop_.rs:4949-4964`).

The new paths:

| Path | Stays closed by |
|---|---|
| Boot before sync | Gate and routes stay closed until `contacts_ready`: the mesh latch, then the first reconcile (D2). After that, the accepted cost under Risks applies. |
| An actor starts while not `contacts_ready` | `reinstall_contact_limits` trusts a local live binding (N:3202-3220), and a contact removed elsewhere still has one locally. So until `contacts_ready` every inbox starts capped (S1), whatever `inbox_full` says. That includes the gap between the latch and the end of the first reconcile. Only after the flip does `refresh_full_inboxes` lift the caps that are still due. |
| Boot before the roster joins (agent inbox) | S6 is unchanged, on the inbox's owner. |
| Missing or invalid agent file | S6 and S2 are unchanged, read on the owner. |
| Revoked binding | Revoked on every machine by the reconcile. An unknown binding resolves as missing, which caps (S3). It is never un-revoked. |
| Removal or block while this machine was down | The first reconcile after the latch runs `revoke_peer_bindings_except` (S:1134-1162) over whatever has synced. A change that did not sync, because every other machine was off, is the accepted cost under Risks. |
| Receiving machine and owner disagree on a removal | Both check: the receiving gate first, then the owner's own gate on `a2a_forward`. |
| `a2a_forward` from outside the mesh | Refused without `Forwarded(peer)`, which only paired engines get (`crates/rpc/src/forward.rs:485-505`). |
| Owner offline, or unreadable | Refused at once (D3). An error from `session_owner` refuses (N:1641-1650). |
| Contact paths reach a session that is not owned here (from P2 on) | Refused with `NOT_OWNER` (D3). |
| Invalid or conflicting replicated row | That row is quarantined and its endpoints denied; the rest stage (D2). |
| A rogue or contested member or anchor | Accusations count from every voter on record, members or not, and the cards of denied machines and non-members are fetched too. A contest with no uncontested voter denies both sides. A fresh ceremony bars denied anchors (D1). |
| An endpoint vouched for only by a removed member | It loses its vote (D1, step 7). |
| A card without the extension, or a tombstone the signer trimmed | The previous vote stands, and a `removed` entry stays until the same voter lists the endpoint live again (D1). |
| An endpoint live in two unmerged records | Denied in both until they merge. |
| A merged-away record, or a broken chain | The binding is revoked and its inboxes capped; a broken chain quarantines the group. |
| Export name collision | Exports are keyed by id; the reconcile makes the name unique; a grant names the id (D2). |
| Import of a stale `peers.yaml` | Never revives a removed record, never writes `blocked` on reuse (D2). |
| Inbox moved, by a take or a Write | `sessions.contact_inbox` travels with the row, so the inbox runs capped until it is re-admitted (D4). |
| Inbox deleted and re-created | The marker goes with the deleted session; the next message goes to the home. |
| Export home removed | Quarantined; no new inbox (D4). |
| Resumed turn after a take | The journal travels (`turn_steps`, capture.rs:28); an empty one caps (durability.rs:1103-1105). |
| A grant change still replicating | Bounded by replication lag. The receiving and the owning machine each check their own copy; only the owner releases a hold. |

### D8. UI (P4)
- **Every machine shows the same contacts,** because every RPC reads the replicated tables.
- **Styling:** theme palette only, from the tokens in `UI/styles.css` (light :14-38, dark :69-93, Tailwind :232-256). No emojis.
- **Network › Contacts list** (`UI/components/settings/Contacts.tsx`: `ContactsTab` :134, `Contacts` :147, each row :175-232):
  - A `Paired on <machine>` line.
  - `Their machines: laptop · studio`. A denied machine is struck through and labelled "left" in `text-muted-foreground`.
  - The `PathChip` (:54-62) shows this machine's path.
  - A row with a `problem` shows it in `text-destructive`, with **Remove** as its only action.
  - A group that is fully denied reads "Pair again: this contact's machines disagree".
- **Remove and Block confirms:** "… on all your machines".
- **Network › Machines, the unpair dialog:** adds "Your contacts are told this machine left. A contact that is offline learns it when it comes back."
- **Access** (`ContactAccess` :688): no change. The picker already lists chats on other machines (`useShareable` :679).
- **Export names:** made unique per mesh. A second `Notes` export reads `Notes-2` (names come from chat titles, :631-643).
- **Share dialog** (`ShareDialog` :828; `SharingItems` :808-822): no change, except that `useGrant` stops sending `modes`.
- **First-contact surfaces:** `ContactAllowStrip` (`UI/components/chat/ContactAllowStrip.tsx:28-31`), `ContactFirstScreen` (`UI/components/attention/PlainScreens.tsx:152-168`) and the HUD (`UI/components/hud/Hud.tsx:179`) keep their components. From P3 their data covers the whole mesh.
- **TUI, Slack, Signal, Telegram:** these have `/a2a` status only (`crates/tui/src/model.rs:7189-7193`). Nothing new.

## Contract
| Item | 191 |
|---|---|
| `Contact` (ts) | + `id`, + `paired_on: string \| null` (the machine name, looked up through `mesh::Status` as `session_move` does, `crates/rpc/src/api.rs:5400-5414`), + `machines: ContactMachine[]`, + `problem: string \| null`; − `cap`. `path` stays, for this machine. |
| `ContactMachine` (new ts) | `{ eid: string, name: string, left: boolean }` |
| `Export` (ts) | + `id`; `name` stays the card name |
| `ContactPatch` | − `modes` (190 kept it "until 191"), − `cap` |
| `FirstWaiting` | same shape; from P3 it is read from `contact_holds` |
| `ContactEnvelope`; `ContactOrigin.{cap,tools,holder}` | removed (the ts row for `ContactEnvelope` at `crates/rpc/src/export.rs:87` goes) |
| RPC `contact_deliver` | removed |
| RPC `a2a_forward` (mesh-internal; `Forwarded` only) | `{ caller: eid, method: "GET"\|"POST", path, body } → { status, body }`; the owner re-checks the caller at its own gate |
| RPCs `contact_list`, `contact_update`, `contact_remove`, `a2a_exports`, `a2a_export_put`, `a2a_export_remove`, `contact_first_answer`, `contact_first_waiting` (`crates/rpc/src/api.rs:2648-2696`; impls :9547-9625) | same names; they read and write the tables, so they answer for the whole mesh. A write to a merged contact writes every record of the group. Export RPCs take the export id. |
| Card | + extension `https://theuth.io/a2a/ext/members/v1` `{at, members[{id, name, removed?}]}`, omitted when empty |
| A2A task id | `<16 hex engine prefix>-<uuidv7>` |
| Migration | **`0059_mesh_contacts.sql`**: `contacts`, `contact_endpoints`, `contact_votes`, `contact_grants`, `a2a_exports`, `contact_addrs`, `sessions.contact_inbox`, and `contact_threads` rebuilt (new table, copy with the new `id`, drop, rename; index kept). `contact_holds` arrives in P3 as `0060`. 0058 is skipped: another branch may hold it for `image_refusals`. The highest today is `0057_inbox_ceiling.sql`. |
| Attach | `replicate_records` (`crates/agent/src/store.rs:913`) + `contacts`, `contact_endpoints`, `contact_votes`, `contact_grants`, `a2a_exports`, `contact_threads` (+ `contact_holds` in P3) |
| `SESSIONS_WIRE_SCHEMA` | 53 → **59**, the number of 191's migration that adds `sessions.contact_inbox`. At landing it follows that migration's number (D2). |
| `peers.yaml` | after import: HTTP peers only. The structs are unchanged, so an old file still loads. |
| Config | − `a2a.contact_cap`. `A2aConfig` is not `deny_unknown_fields` (A/config.rs:27-28), so an old `config.yaml` still loads. No new key. Constants: `MEMBER_SKEW_MS` = 10 min; the list carries every live member plus the newest 64 tombstones; card fetches are capped at 64 per group; names are clipped to 64 characters. |

## Phases
- **The order differs from the recon's** (there: P2 routing, P3 endpoint sets). Here endpoint sets come first (P2), and routing follows (P3). With sets first, a session on any machine dials with its own key. Routing first would need an outbound relay through the pairing machine, which the standing rule rules out, and P3 would only delete it again.
- **Every phase is safe on its own.** P2 adds the owner check of D3, so a reply that lands on the wrong machine is refused and the client tries the next member.
- **Attention from every machine moves to P3.** Before P3, a hold on another machine waits; it is not released.
- **Why not three phases:** one phase would grow to about 2,000 production lines, too much for one review round.

| Phase | What | Prod | Tests | Proof |
|---|---|---|---|---|
| **P1 RECORDS** | 0059 and the attach; import and pointer merge; staging from the tables with per-row quarantine; reconcile on the content hash; `contacts_ready`, including the actor-start cap; the replicated binding id, never un-revoked; contact RPCs write the tables; export ids; `sessions.contact_inbox` | 1,200–1,500 | 1,000–1,200 | Rig cases 5 and 6; a contact paired on n1 is listed on n2 |
| **P2 MEMBERS** | Members extension; notice on `peers_gen` and at boot; folded vote rows and the membership algorithm, barring included; gate over the live set; dialing over the set; per-endpoint hints; owner refusal on contact paths | 1,000–1,200 | 800–1,000 | Rig cases 2 and 4 (a rogue anchor included) |
| **P3 ROUTING** | `a2a_forward`; owner resolution; offline refusal; forwarding of `~return` and task ids; inbox home; `contact_holds` (0060) and Attention from every machine; completing tasks on drain; **W-40 deleted** | +900–1,100, −600–650 | +850–1,050, −360 | Rig cases 1 and 3 |
| **P4 UI and docs** | D8; site docs; as-built | 400–600 | 400–500 | Vite fixture screenshots; the whole rig |
| Total | | ≈ 3,500–4,400 gross; ≈ 2,900–3,750 net | ≈ 3,050–3,750 gross | |

## Tests and proof
**Rig.** Three scratch engines on loopback, discovery off, with a fake model server (the 186 rig `/tmp/d186/rig`, if present).
- `n1` and `n2` are paired as one mesh. `C` is a lone engine, paired as a contact with `n1` only.
- C's fake model answers `<message …>PING n` with `send_message(to=<from>, reply_to=<id>, PONG n)`.
- **To hold n2 unready without a race**, stop n1 before n2 boots. n2 cannot catch up with any peer, so `mesh::synced()` waits the full 30 s (`lapse_secs`). Probes inside that window are deterministic.

The cases:
1. **A chat on n2 answers a contact paired on n1.** n2 owns chat X, and X is shared to C from n1's UI.
   - C sends `PING 1` to n1, which forwards it to n2.
   - X shows `<message from="peer:…">` and no `<contact_message>`.
   - C gets `PONG 1`.
   - The task id's prefix names n2, and C's `GetTask` reaches n2.
2. **Sending from n2.** A session on n2 calls `send_message(peer:c/<export>)`. n2 dials C with n2's key: C admits n2 through the mutual votes of n1 and n2. The answer returns through `~return` to the session on n2.
3. **n2 off.** Stop n2. C's message to X reads "the chat's machine is offline" in under 1 s, and nothing is queued.
4. **n2 removed.**
   - Honest leaver: `mesh_remove(n2)` on n1. Within one notice, C shows n2 as left and refuses its streams (log: gate refused). n2's sends to C fail.
   - **Rogue anchor.** C's anchor is n1. Remove n1 with `mesh_remove(n1)` on n2. Stop n1, write a tombstone for n2 straight into n1's `mesh.sqlite` (as 183's Q5b did), and restart n1.
     - n1's card now accuses n2, and n2's card accuses n1. n2 is not an anchor, but C fetches its card because n2 has a vote on record.
     - U is empty, so C denies both, refuses both, and shows "Pair again". n1 does not keep access.
     - A fresh ceremony from n2 bars n1 and makes n2 the anchor. n2 is live again, and n1 stays refused.
5. **A block made while n2 was down (the accepted cost).**
   - Stop n2. Block C on n1, then stop n1. Start n2.
   - For the first 30 s, C's dial to n2 is refused (not ready).
   - After the latch and the first reconcile, n2 admits C from its stale copy. This is the accepted cost.
   - Start n1. Within one tick of catching up, n2 refuses C as blocked and closes C's connection.
6. **An actor starting before `contacts_ready`.**
   - n1 removed contact C while n2 was down. A full inbox for C exists on n2.
   - Boot n2 with n1 off. The inbox actor starts inside the first 30 s, capped: read-only, empty allowlist.
   - After the flip it runs full from the stale binding (the accepted cost).
   - After n1 returns, the binding is revoked and the inbox is capped again.

**Unit tests**, one for each plausible regression:
- **P1**
  - A remote `blocked` write older than the row's newest column still triggers a reconcile (M1).
  - A malformed row next to a block: the block applies.
  - `contacts_ready` flips only after the latch and the end of the first reconcile. An actor that starts in between runs capped, and the gate stays closed in between (M2).
  - A revoked binding id that arrives by replica stays revoked.
  - An inbox taken by a Write runs capped (marker without a local row).
  - Two exports with one name and different chats get two ids. Grants reach the right chat, and the newer is renamed (M4).
  - Import attaches to a removed or blocked record without reviving it or writing `blocked`. A later ceremony re-pairs the contact (S2).
  - Import backfills `contact_inbox`.
  - A grant held by one of two duplicates carries over. `granted = 0` and `block` win when present. A chain C→B→A resolves, and a loop quarantines.
  - An alias collision converges.
- **P2**
  - Adding needs both signatures, and a stranger listed by one member is not added.
  - Omitting an endpoint withdraws the vote.
  - An endpoint vouched for only by a removed member drops out.
  - A rogue **anchor** that accuses the honest machines denies everyone; one that omits them is denied alone. In both cases a fresh ceremony bars it and restores the honest machines (MUST 1).
  - A card without the extension keeps the previous vote. A tombstone the signer trimmed keeps its accusation.
  - A quarantine-only list change bumps `at` and writes a new vote. An unchanged list writes no row.
  - A contest with an uncontested accuser resolves to the honest side.
  - A future `at` is clamped, and a later card from the same voter still applies (S3).
  - `NOT_OWNER` on `~return` makes the client try the next member (S4).
- **P3**
  - `a2a_forward` without the marker is refused.
  - The owner refuses a caller that its own copy blocks.
  - An owner that is not connected is refused at once.
  - `~return` is forwarded by its thread.
  - On drain, waiting full tasks complete.
  - An answer from n1 releases a hold on n2.
  - An export whose home was removed opens no inbox (S5).

The S1–S10 tests stay.

## The recon's "Not verified" list, resolved
- **The journal after a take:** it travels. `turn_steps` is a session-log table (capture.rs:28), and resume reads it on the new owner (`crates/agent/src/supervisor/resume.rs:165-171`, :198-216). S7 holds.
- **`contact_card`'s pause check for a remote chat:** it already answers from the replicated row (N:3004-3024; `crates/agent/src/supervisor.rs:1206-1219`; A/contacts/offers.rs:211-237).
- **One key on two machines:** discovery would flap (D1).
- **TUI, Slack, Signal, Telegram:** `/a2a` only (D8).
- **Where the tables live:** `sessions.sqlite` (D2).
- **Found besides:**
  - The moved-inbox fail-open (D4).
  - Replica INSERT failures are silent and permanent (D2).
  - A row's `hlc` is a max over its columns, so it does not show every change (D2).
  - `SYNCED` starts true, and the latch is bounded at 30 s and 150 s (D2).
  - Block also sends `Revoke` (A/contacts.rs:1702-1732), which the receiver only logs (`A/contacts/ceremony.rs:148-151`).
  - W-40 is about 600–650 production lines.
  - `frame.rs` no longer has a chat variant of the contact prompt.

## Risks
- **Accepted cost of the bounded latch (Sky, 17:59, option (b)).**
  - A block or removal made on another machine while this one was down is missed until this machine next syncs with a peer. With every other machine off, that has no bound.
  - A machine removed from the mesh is never told (M/net.rs:1272-1297). After 30 s it would serve contacts from its copy, until those contacts hear of the removal from our other machines: seconds while they are online (D1).
- **The window after a removal (D1).** It lasts seconds while the contact is online. While the contact is offline, it stays open until the contact returns. A rogue member or anchor can force the contact to pair again, but it never keeps access.
- **A wrongly cut machine** waits up to `OFFERS_TTL` (60 s) to be restored, because it cannot notify.
- **A merge ends conversations, once.** The next message starts fresh.
- **Write contention.** Replica apply takes `BEGIN IMMEDIATE` on the single `sessions.sqlite` write connection (R/apply.rs:454). The rows are tiny and the writes rare. Watch it in the P1 rig.
