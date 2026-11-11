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
- **The other side's set is a signed, versioned roster, one chain per record.**
  - **Shape of one signed copy.** `{chain, version, members: [eid…], signer, sig}`.
    - `members` is the signing mesh's current membership: the mesh `peers` that are neither removed nor quarantined (M/store.rs:152-168), plus the signer itself.
    - **Cap of 256.** When the membership exceeds 256, the member signs the first 256: pinned machines first, then the oldest by `added_ms`. A cap never freezes removals. A contact refuses a roster of more than 256.
    - `chain` is the id of the signing side's contact record: a UUIDv7 that is already replicated and never changes. It is not an epoch and has no order. It only binds a copy to one record, so a roster we signed for contact A cannot be replayed to contact B.
    - `version` is a plain counter in that chain.
    - `sig` is an Ed25519 signature by the signer's own iroh key (`Net::sign`, M/net.rs:945-947) over `b"theuth/a2a/roster/v1\n"` followed by the JCS of `{chain, version, members (sorted), signer}`. The prefix separates it from the card JWS, which the same key signs.
    - The **content** is `{chain, version, members (sorted)}`, and its hash breaks ties. Every machine that signs the same membership at the same version signs the same content, so one version can carry several signed copies.
  - **Each record has exactly one chain.** A merge never touches a chain. A merged group admits the **union** of its records' current rosters (D2 merge).
  - **Signing (our side), checked in every reconcile.**
    - The reconcile runs when the `Net.peers_gen` watch bumps (M/net.rs:460-461; bumped inside `refresh_paired`, M/net.rs:1322, which `remove_peer` calls, M/net.rs:1275-1297), when `contact_rosters` rows arrive (they are in the reconcile hash, D2), and at boot.
    - A machine signs nothing before `contacts_ready` (D2), and never signs for a removed record.
    - For each live record, it compares the membership it computes with the **latest** roster of the record's chain (highest version; highest hash among equals), then:
      - if they differ, it signs a new roster at `version` = latest + 1;
      - if they are equal and it **has not signed it**, it **co-signs**: it signs the same content under its own key;
      - otherwise it does nothing.
    - Every member therefore signs the latest version once, and a contact that trusts only one of our machines still gets a copy that machine signed.
    - Our rows replicate (`contact_rosters`, D2). We keep **every version** of each live chain, so that we can serve a proof path (below). Each version keeps at most 16 signed copies, chosen oldest members first. Rosters are small.
  - **Delivery: the last 8 versions.**
    - The card a contact fetches is built for that caller (`ContactCaller`, A/contacts/reach.rs:174-209). Its extension `https://theuth.io/a2a/ext/members/v1` carries, for each of our records whose chain lists the caller, the last 8 versions of that chain.
    - Each version carries its content once and at most 16 signatures, in this order:
      1. signers who are members of the previous version, oldest member first;
      2. the pinned signers;
      3. the rest.
      
      The signers the contact trusts are therefore never crowded out by newcomers.
    - A removed record's chain is never carried.
      ```json
      { "uri": "https://theuth.io/a2a/ext/members/v1",
        "params": { "chains": [ { "chain": "<record uuid>", "versions": [
          { "version": 7, "members": ["<eid hex>", "<eid hex>"],
            "sigs": [ { "signer": "<eid hex>", "sig": "<b64url>" } ] } ] } ] } }
      ```
    - **Size.** The caps bound it: 8 versions × 256 members ≈ 130 KB at worst. A mesh of under 10 machines stays under 10 KB.
    - Each copy carries its own signature, so the same payload also travels in a push. The card's JWS covers `params` too (A/contacts.rs:473-481, :621-631).
    - The extension is omitted while no chain exists, so the JWS test vector (A/contacts.rs:2254-2341) keeps its value.
    - **Push.** After any signing, our machines push the payload to every **live** contact as the body of a `NOTICE_PATH` POST (A/contacts/offers.rs:25).
      - Today that route ignores the body and answers 204 (reach.rs:241-243). It now reads a body of up to 192 KiB and answers 413 above that.
      - A failed post is retried with exponential backoff, from 2 s up to a 10-minute cap, per contact. The backoff resets on each new signing.
      - The payload is sent again at boot, so nothing is persisted.
    - **Refresh.** The contact fetches the card from **every** member it trusts, not one: at pairing, at boot, on notice, and at most once per `OFFERS_TTL` of 60 s (N:327-328).
    - A card without the extension changes nothing.
- **The contact's accept rule.** Each machine of contact C holds, **for itself**, a current roster `R` at version `v` in the record's chain. It repeats this step:
  - Take the **lowest** carried version `w > v` that has a copy with the record's `chain`, signed by a member of `R`, with a valid `sig`.
  - At `w`, take the highest content hash among such copies. Carried copies and proof paths include every content signed at a version, so concurrent honest signings on our side resolve the same way on every machine of C.
  - Accept it: `R` and `v` become that roster and `w`. The next step must then be signed by a member of the new `R`.
  - **Gaps are allowed.** C may jump straight to the first version it can verify, so a contact that missed any number of versions catches up, provided one member it trusts still signs. Versions never go down, and anything below `v` is ignored. That is the whole replay protection: no timestamps, no skew rule.
  - **Equal version, higher hash.** A machine replaces its head at the **same** version with content of a higher hash, if that content is signed by a member of the roster that came **before** the head (its history holds that roster). An honest same-version fork on our side therefore converges on every C machine.
  - **Our side converges too.** A machine of ours signs against the latest roster: the highest version, then the highest hash. So once a same-version fork is visible, the signer that lost (lower hash) co-signs the winner's content when its membership matches it. If its own change is still missing, it signs v + 1 on top of the winner.
  - **The real limit** is that every member C trusts has been removed. The remedy is to remove the contact and pair fresh (below).
- **C-side roster state is per machine and never replicated.**
  - The contact **record** still replicates in C's mesh (alias, grants, pins; P1). Its **roster trust state** does not.
  - Each C machine keeps its own state in a local table, `contact_roster_local`, which is not attached: the current accepted version and roster, plus its accepted history as `(version, hash, roster)` entries, the last 64 kept.
  - That state is learned only from **our** signed copies, delivered to that machine by a card fetch, a push or a proof path. A C machine never adopts another C machine's roster.
- **Bootstrap from the pin: the proof path.** This covers a C machine with no state: a new machine, a wiped table, or the P2 upgrade.
  - **The trust root is always the record's pin**, the endpoint of ours that the ceremony pinned. It replicates with the record.
  - **Dial hints are untrusted.** Each `contact_roster_heads` row also carries the member endpoint ids of that machine's head. A fresh machine uses them, together with the pin, only to choose whom to dial. They never count toward verification.
  - It asks our machines for a **proof path** on the contact-only route `GET ROSTER_PATH?chain=<id>&pin=<eid>&from=<version>`.
    - It asks **at least two** of our machines when two are reachable, and takes the highest path it can verify.
    - The route runs under the contact's bounds (A/contacts/reach.rs:211-246).
    - It serves only the chains of the **caller's own** records, and only if `pin` is one of that record's pins: a machine of ours that ran a ceremony into it.
    - One answer holds at most 64 hops and 256 KiB; `from` continues a longer path.
  - **The path.** It runs from the first version the pin signed (a copy listing the pin) up to the latest. Each hop is signed by a member of the previous hop. We hop greedily, to the highest version that hop can verify, and include every content signed at each hop version.
  - **Verification.** C verifies the path itself, from the pin. The same request serves a machine that cannot verify any of the 8 carried versions, and a merely stale machine that is still behind after 3 refreshes.
  - **What our side keeps.** For each pin of a live record, the minimal **hop chain**: versions chosen greedily from the pin's first signed version, each signed by a member of the previous one, with its qualifying copies. Plus the last 8 versions. Everything else is pruned. So a path survives the pinned machine's retirement.
  - **A fresh machine that cannot bootstrap denies the record.** This applies to a record that any C machine has already based, where no proof path can be verified (the pin is retired and no other machine of ours is reachable, or every path fails). The machine denies the record and shows `Contact.problem` "cannot verify this contact's machines". It **never** falls back to admitting the pin alone. This changes the earlier text: "admits exactly its pins" now applies only to a record that no C machine has based yet (no `contact_roster_heads` row).
- **Divergence detection, not adoption.**
  - Each C machine writes one replicated row per record, which no other machine writes: `contact_roster_heads(record|machine) = {version, hash, history (the last 64 (version, hash) pairs), members (untrusted dial hints), conflict}`.
  - **Comparison.** A C machine compares its own state with every peer row of the same record:
    - **The same head:** they agree.
    - **Different heads, and the lower head appears in the higher one's history:** the lower machine is merely stale. It catches up by jump and replay. There is no conflict.
    - **Otherwise** (different hashes at one version, or a lower head missing from the higher one's history): the machine with the lower head (both machines, when the versions are equal) fetches a proof path from at least two of our machines up to the other head's version, then applies the equal-version rule.
  - **Conflict.** A machine sets `conflict = 1` **only when a path it verified reaches the other head's version, and the heads still differ there.**
    - A path that stops short, for example one served by a machine of ours that is lagging, is never a conflict.
    - Every machine recomputes `conflict` at every refresh and clears it once the heads agree.
    - While any row has the flag set, every C machine **denies the record** and shows `Contact.problem`: "this contact's machines disagree about yours; remove and pair again".
  - **A poisoned machine** is a C machine that accepted a rogue roster while it was stale.
    - It does not convert the others.
    - Its head becomes a conflict once our chain reaches the rogue head's version with different content, and every machine of C then denies the contact.
    - Until our chain reaches that version (a rogue that inflated its counter), the poisoned machine alone follows the rogue roster: the stated limit.
    - Removing the contact and pairing fresh clears it.
  - **A rogue can force a deny.** A modified member can keep C in conflict on purpose. That is denial of service, not access (D7).
  - **No peer to compare.** A machine with no peer rows yet, or with none of our machines reachable, uses its own verified state and flags nothing.
- **Where a chain starts (the base).**
  - **At a ceremony.** It is the human step on both sides, as today. The pairing machine must be `contacts_ready` (D2): the Add contact window refuses with "waiting for your other machines" until it is, so version 1 never comes from a stale view.
    - On a new record, the pairing machine signs version 1, and every member co-signs.
    - A ceremony with an endpoint that already belongs to a live record only adds a pin to it (`pin()` keeps the alias of a known endpoint, A/contacts.rs:1052-1056). The chain is unchanged: **Pair again is not a reset.**
  - **The contact's base.** The base is the first copy **signed by one of the record's pinned endpoints that lists that endpoint**. The pin was a human ceremony. A machine with no state reaches the base through the proof path, so it does not need the pinned machine to still be running. It does need one of our machines to be reachable.
    - If copies of several **live** chains qualify (our side has duplicate live records), C takes the lowest `chain`, and its machines converge on it. A removed record's chain is never carried, so it is never a candidate.
    - Records from before any roster (P1 records and imports, with pins only in `contact_endpoints`) use the same rule. Because every member co-signs, the pinned machine's copy arrives as soon as that machine runs.
    - Until any C machine has a base, the record admits exactly its pins, as today (D7). After that, a machine without a base denies the record (above).
    - Residual: if the pinned machine was lost before it ever signed, remove the contact and pair fresh.
- **A removed record is inert.** Once a record has `removed_ms`, on either side:
  - it never merges;
  - it never leads a group as survivor;
  - its chain is never carried, signed or pushed;
  - its chain is never chosen as a base;
  - its roster rows and `contact_roster_heads` rows are ignored.
  
  A fresh pairing with the same machines therefore makes a new record, with its own fresh binding, no grants and its own chain at version 1. It never joins the tombstone. Only the deny entries that the removed record left remain, and only that newer ceremony can lift them (D2).
- **Merge proof and attribution (D2).**
  - **Merge proof.** Two records merge only if each record's current roster lists the **other** record's pinned endpoint or base signer. Each roster is signed by its own mesh, and B's honest roster never lists a machine of contact A, so A cannot merge into B by listing one of B's machines.
    - Two of our machines that each paired the same machine share a pin, so their records merge.
    - Mesh to mesh: a record pinned to c1 and a record pinned to c2 merge, because each mesh's roster lists its own other machine.
    - The survivor keeps its binding; merged-away bindings are revoked (D2).
  - **Attribution.** When an endpoint Y appears in the current rosters of two unmerged records, it is admitted only for the record that **pinned Y or holds a copy signed by Y**. It is never denied there because of the overlap, so another contact cannot evict a machine by listing it. If neither record qualifies, Y is not admitted until Y co-signs.
- **What the rule gives (answer 1).** Every case below is automatic under honest code.

| Case | Result at the contact |
|---|---|
| A lost or retired machine, the pairing machine included | Any remaining member signs the next version without it, and the others co-sign. C accepts it, because the signer is in C's current roster, and drops the machine. The removed machine is never told (M/net.rs:1272-1297), and it does not need to be. |
| A new machine in our mesh | A member signs a version that includes it. No co-signature from the contact's side is needed: mesh pairing already proved key possession. |
| A contact that was offline during any number of changes | The removed machine keeps reaching it until C next fetches a card or gets a push: the stated window. Then C jumps to the first carried version a member it trusts signed, and replays forward. |
| Two different rosters at the same version | The content hash picks one. The next signing converges. |
| An existing (pin-only) contact | Our machines sign version 1 and co-sign it. C adopts the copy its pin signed. |
| A machine that is dead but not removed | It stays in the roster. The user removes it in Settings › Network › Machines (Unpair, `mesh_remove`); the next version drops it. |
| A member running **modified (malicious) code**, or an **unmodified machine operated by a thief** (who can `mesh_remove` the honest machines from its UI) | **This is the limit.** Such a member can sign a roster that excludes the honest machines, and C follows it. **Remedy, manual, and needing both sides:** (1) remove the machine from our mesh; (2) **both sides remove the contact**, which tombstones every record of the merged group and makes the group inert (above); (3) pair fresh as a **new** contact, with a new record and a new chain at version 1. Until C's own user removes the contact, C keeps admitting the rogue machine: our removal cannot reach C's records. D8 prompts our user to ask the contact to do so. A removed record is never revived, and its deny entries lift as D2 says. The user must review the grants and exports the rogue machine wrote. Sky's answer 1 is automatic for lost, retired or honest-code machines. |
| C's machines hold different rosters (one took a rogue roster while it was stale) | The rogue roster is not adopted, and the difference is detected through `contact_roster_heads`. Once a path verified from our side reaches the other head's version and the heads still differ there, every machine of C denies the record and shows "this contact's machines disagree about yours; remove and pair again". A merely stale machine is not a conflict, and neither is an honest same-version fork: it converges to the higher hash. |
| A C machine with no state (new, wiped, or upgraded) | It bootstraps from the record's pin, through a proof path from at least two of our machines, dialed by untrusted hints. If it cannot verify a path, it denies the record; it never admits the pin alone. |
| Every member C trusts was removed while C was offline | C cannot verify any newer version. Remedy: remove the contact and pair fresh. |
- **The window.** Until a contact accepts the new version, a removed machine can still pass as us to that contact.
  - While the contact is online, that lasts seconds: one push.
  - While the contact is offline, it stays open until the contact comes back and fetches a card.

### D2. Replication: last-writer-wins tables in `sessions.sqlite`
- **Mechanism.** `replica::attach`: per-column LWW under HLC, tombstones, version-vector catch-up (R/lib.rs:1-14, :104-121; R/sync.rs:1-14). The record tables are appended to `replicate_records` beside `projects` and `cost_owners` (`crates/agent/src/store.rs:903-915`, attach at :913).
- **No floor gate is needed.** A machine without these tables parks their batches: up to 65,536 change sets for up to 7 days, replayed at attach (R/apply.rs:27, :29, :75-93).
- **`SESSIONS_WIRE_SCHEMA` is set to the number of 191's own migration**, for the inbox marker (D4). That is 59 today: the rule is "set this to a new migration's number" (`crates/agent/src/store.rs:659-670`; test text at :4249-4253).
  - Any other value is wrong. With 54, an unrelated 0058 bump would read as newer, and this build would ship `contact_inbox` rows to a peer that lacks the column. `sqlite3changeset_apply` would skip the table while the cursor advances (`crates/sessionsync/src/apply.rs:80-109`): silent loss.
  - Another branch adds `0058_image_refusals` and lands first. 191's migrations are 0059, 0060 and 0061; if that changes at landing, the bump follows the number of the migration that adds `sessions.contact_inbox`.
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
| `contact_endpoints` | `<contact>\|<eid>` | `contact`, `eid`, `paired_ms`, `barred_ms` (unused since D1's roster) | the ceremony only: the pinned endpoints, which D1 uses until a roster exists |
| `contact_votes` | — | — | superseded by D1's roster. P1 may leave it out. If 0059 already created it, P2's 0060 drops it. |
| `contact_rosters` (P2, 0060) | `<contact>\|<chain>\|<version>\|<signer>` | `contact`, `chain`, `version`, `signer`, `hash` (of the content), `roster` (the signed copy, JSON) | **Ours only.** Written by any of our machines when it signs or co-signs. Kept per chain: the hop chain from each pin's first signed version, plus the last 8 versions, with at most 16 copies per version, oldest members first. Everything else is pruned. Nothing is written for a removed record. Rows are immutable. |
| `contact_roster_heads` (P2, 0060) | `<record>\|<machine eid>` | `version`, `hash`, `history` (the last 64 `(version, hash)` pairs, JSON), `members` (our endpoint ids at that head: **untrusted dial hints**), `conflict` (0/1) | the C-side machine named in the key, and only it. It publishes its own head so that divergence can be **detected**, and so that a fresh machine knows whom to dial. It is never adopted, and the hints never count toward verification (D1). |
| `contact_grants` | `<contact>\|<export id>` | `contact`, `export`, `granted` (0/1), `first_contact` (`ask`/`allow`/`block`; NULL = ask) | contact_update, contact_first_answer |
| `a2a_exports` | `id`: UUIDv7 minted when the export is made | `name` (the card skill and the `/agents/<name>` path; made unique by the reconcile), `session`, `agent`, `title`, `description`, `home` (engine id), `removed_ms` | a2a_export_put and a2a_export_remove, import |
| `contact_threads` (rebuilt) | `<binding>\|<context_id>` | the 0056 columns: `binding`, `context_id`, `session`, `export`, `outbound`, `last_ms` | as today (S:1591-1676) |
| `contact_holds` (P3) | the A2A task id | `contact`, `machine`, `export`, `session`, `owner` (engine id), `since_ms`, `waiting`, `preview` (clipped to 200 characters), `until_ms` | the owner only. The row is deleted when the hold ends (D4). |

Local and not attached (P2, 0060): `contact_roster_local(record PK, version, hash, roster, history)`, which is each C-side machine's own verified roster state (D1).

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
  - A machine that leaves keeps copies of public rows and its own key. It is cut off by the next roster (D1), not by a rotation. The vault is untouched.
- **Binding identity.**
  - The record's `binding` column is the `peer:<uuid>` everywhere.
  - `peer_bindings.endpoint` becomes `contact:<record id>`. It is constant for a record, so endpoint-set changes never revoke.
  - `bind_peer_in` (S:905-946) takes the id instead of minting it. It works in three cases:
    - A live row with that id has its alias updated in place, so a rename no longer revokes.
    - Otherwise, the alias's live row is revoked and the id is inserted.
    - An id already revoked here **stays revoked**, and its record is quarantined on this machine. A replica never un-revokes.
  - Unblock and re-pair write a fresh `binding`.
- **Reconcile: one pass.**
  - **Trigger.** On the existing 2 s a2a tick (A/lib.rs:712), hash the full content of every row of the mutable staged tables: `contacts`, `contact_endpoints`, `contact_grants`, `a2a_exports`, `contact_roster_heads` (P2) and `contact_holds` (P3). `contact_rosters` rows are immutable, so `(count, max hlc)` is enough for them. `contact_threads` is read per request and never staged. The tables are bounded:
    - one row per contact record, per pin, per grant, per export;
    - `contact_rosters`: per live chain, the hop chain plus the last 8 versions, × at most 16 copies; our side only;
    - `contact_roster_heads`: one row per C-side machine per record;
    - in `contact_holds`, one row per waiting hold.
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
    - an endpoint pinned by two unmerged records (until they merge). From P2 an endpoint that two records' rosters list follows D1's attribution rule instead, and the overlap never denies it.
  - Blocked and merged-away records only ever add to the deny set.
  - A removed record's endpoints stay denied. They lift **only** through a pairing ceremony that is **newer than the removal** (`paired_ms > removed_ms`) and pins an endpoint that the removed record pinned or listed. Even then, only the endpoints that the new record's own current roster lists are lifted. A roster alone, a listing by another contact, and an older pin (a late duplicate record, or an import) never lift a deny.
  - **Uniqueness by reconcile.** Two records with the same alias, or two exports with the same name: the older `(paired_ms or id, id)` keeps the name, and the reconcile writes `<name>-2` into the newer one. Every machine computes the same value, so the writes converge.
- **Admission waits for the mesh's latch and the first reconcile (Sky, 17:59: option (b)).**
  - `mesh::synced()` is the bounded latch task work already uses. It flips true when:
    - every *connected* peer has caught up; or
    - 30 s pass (`lapse_secs`) with no peer connected; or
    - 150 s pass at worst (M/lib.rs:1478-1520).
  - It cannot be used raw. `SYNCED` starts true (M/lib.rs:126), and the mesh sets it false only at the end of its start, when this machine has paired peers (M/lib.rs:1481-1482). The mesh starts before a2a (`crates/theuth/src/main.rs:2023`, :2034).
  - So contacts get their own flag, `contacts_ready`. It **starts false**. It is a static atomic in the agent crate, like `mesh::SYNCED` (M/lib.rs:126). `reinstall_contact_limits` and the gate read it directly. An ask from the agent to the a2a actor could deadlock with the reconcile, which itself asks the agents (`CommitPeerConfig`).
  - It flips only after `mesh::synced()` is true **and** the first contact reconcile has finished all six steps: the revokes, the inbox caps and the closes, on the post-latch tables.
  - **Until `contacts_ready`:**
    - the gate (`ContactAlpn::admit`, A/contacts/reach.rs:116-121; `iroh_gate`, A/server.rs:916-935) refuses every contact;
    - no route is committed, and outbound sends read "waiting for your other machines";
    - every inbox actor starts capped (D7), whatever its local `inbox_full` and binding say.
  - Only after the flip does `refresh_full_inboxes` (F:524-562) lift the caps that are still due. No actor can run full from a stale local binding in the gap between the latch and the reconcile.
  - A machine with no paired peer is latched at once; it reconciles, then it is ready.
  - **The accepted cost** is stated under Risks.
- **Merge (answer 5): a pointer, never a copy.**
  - **What counts as the same contact.** Two **live** records that pinned the same endpoint, or that pass D1's merge proof: each record's current roster lists the **other** record's pin or base signer. The first case covers two of our machines having paired the same machine. A merge never touches either record's chain; the group admits the union of its records' current rosters (D1). A removed record never merges and never leads (D1).
  - **The survivor** is the live record with the lower `(paired_ms, id)`.
  - **The write.** Each machine writes `merged_into = <root>` on every other record of the group. The root is the group's survivor, so the value is the same from every machine. Pointers always point at a lower key, so chains (C→B→A) resolve by following them to the record whose `merged_into` is NULL. The reconcile rewrites every pointer to the root. A chain that does not end within 16 hops, or that loops, quarantines the whole group.
  - **The effective contact.**
    - Endpoints: union over the group.
    - Blocked: true if any record is blocked.
    - Alias and binding: the survivor's. The survivor keeps its binding unchanged; only the merged-away bindings are revoked.
    - Grants are taken per export over the records **that have a row for it**. A record with no row abstains, so a grant held by only one duplicate carries over.
    - Among the rows present: `granted = 0` beats `granted = 1`, and `first_contact` takes the strictest, `block` > `ask` (NULL) > `allow`.
  - **Why not copy.** Copying values instead would race: a machine missing a `block` could write `allow` last.
  - **Edits** to a merged contact write every record of the group that has a row, so the strictest read never pins an old value. A grant that no record of the group has yet (the group's first grant for an export) is written to the survivor.
  - **What a merge ends.** Merged-away bindings are revoked, so their inboxes are capped (S3/S5). Their open conversations end ("the conversation with … ended", F:157-268), and the next message starts fresh.
- **Import.** Data the previous build wrote must still load.
  - **When:** at the first reconcile after `contacts_ready`. Each machine imports its `peers.yaml` contact entries (`is_contact()`, A/config.rs:184-195) and its exports.
  - **The file still parses as is:** `PeersFile` and `PeerEntry` stay `deny_unknown_fields` (A/config.rs:111, :138).
  - **Matching a contact.** If any record (live, blocked or removed) has a pin for the same eid, the entry attaches to the newest such record:
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
  - The separate fix on main (`INBOX_STAYS`, landed in 4bc0dddd) makes `sessionsync::owner::move_to` and `answer_release` refuse inbox sessions, so an inbox cannot move by a user action. **P3 lifts that refusal, together with forwarding, and not earlier.** Before P2's `NOT_OWNER` check and P3's forward, a moved inbox's contact messages would be delivered to a machine that no longer owns it.
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
- **Pairing.** One ceremony runs between one machine on each side. Each side signs its record's roster at version 1, which covers every other machine of its mesh, and its other machines co-sign it. Both humans press Accept once. A side that is a single engine has a roster of one.
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
| A removed member | Dropped by the next version any remaining member signs. C accepts the lowest newer version of the record's `chain` that carries a valid copy signed by a member of its current roster (D1). |
| A replayed or forged roster | Refused if it has an equal or older version, another record's `chain`, a signer outside the current roster, a bad signature (domain-separated: `theuth/a2a/roster/v1`), or more than 256 members (D1). |
| A mesh removal made while one of our machines was down | That machine signs nothing before `contacts_ready`. If every other machine was off and it signed from a stale view, the next version a synced member signs supersedes it once they meet, within the accepted latch cost (D2). |
| A pin-only legacy record that no C machine has based yet | It admits exactly its pins, as today. It adopts a base only from a copy signed by one of its pins that lists that pin. If the pin was lost before it ever signed, the contact must be removed and paired fresh (D1). |
| A contact offline across any number of changes | It jumps to the lowest carried version signed by a member it trusts and replays forward from there. If every member it trusts was removed, the user removes the contact and pairs fresh (D1). |
| A ceremony run from a stale machine | Refused until that machine is `contacts_ready` (D1). |
| A member running modified code, or an unmodified machine operated by a thief | This is the stated limit (D1): such a machine can sign a roster that excludes the honest machines. **Remedy, which needs both sides:** remove it from our mesh, remove the contact on both sides, then pair fresh as a new contact. Until C's own user removes the contact, C keeps admitting the rogue machine. The user reviews the grants and exports the rogue machine wrote. |
| A deny entry left by a removed record | It lifts only through a newer ceremony (`paired_ms > removed_ms`) that pins an endpoint the removed record pinned or listed, and only for the endpoints the new record's own roster lists. A late duplicate pinned to the rogue machine, an import, or another contact's roster listing that machine never lifts it (D2). |
| A C-side machine with no roster state (new, wiped, or upgraded to P2) | It bootstraps from the record's pin through a proof path, verified by itself. It asks at least two of our machines when two are reachable, and dials them using the untrusted hints in `contact_roster_heads`. It never adopts another C machine's state (D1). |
| A fresh C-side machine that cannot bootstrap | On a record that any C machine has already based, it **denies** and shows "cannot verify this contact's machines". It never falls back to admitting the retired pin alone (D1). This changes the earlier text. |
| C-side machines that hold different rosters | Detected through `contact_roster_heads`. A conflict is set only when a verified path reaches the other head's version and the heads still differ there; a path that stops short (a lagging machine of ours) never sets one. `conflict` is recomputed at every refresh and cleared once the heads agree (D1). |
| An honest same-version fork on our side | C machines move to the higher hash at the same version (signed by a member of the roster before the head), and our losing signer co-signs the winner. Everything converges, with no deny (D1). |
| A rogue that forces a conflict on purpose | Denial of service, not access. The remedy is to remove the contact and pair fresh (D1). |
| A poisoned C-side machine (it accepted a rogue roster while stale) | It does not convert the others. Its head is flagged as a conflict once our chain reaches the rogue head's version with different content, and every machine of C then denies the contact until it is removed and paired fresh. Until then, it alone follows the rogue roster: the stated limit (D1). |
| A removed record | Inert: it never merges, never leads a group, its chain is never carried, signed, pushed or chosen as a base, and its rows are ignored. A fresh pairing with the same machines makes a new record with a fresh binding and no grants (D1). |
| Pushes | Sent only to live contacts, backing off exponentially from 2 s to a 10-minute cap per contact. The notice body is capped at 192 KiB; larger bodies get 413 (D1). |
| An endpoint listed by two unmerged records | Admitted only for the record that pinned it or holds a copy it signed. It is never denied there because of the overlap, and a planted listing neither merges records nor evicts the machine (D1). |
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
  - `Their machines: laptop · studio`, from the contact's current roster. A machine that a newer roster dropped is shown struck through, labelled "left", in `text-muted-foreground`.
  - The `PathChip` (:54-62) shows this machine's path.
  - A row with a `problem` shows it in `text-destructive`, with **Remove** as its only action.
- **Remove and Block confirms:** "… on all your machines".
- **Removing a contact because of a rogue machine.** The Remove confirm on a contact, and the unpair dialog when Unpair is chosen for a lost or stolen machine, show: "Ask <contact> to remove you too, then pair again. Until they do, the removed machine may still reach them." The remedy needs both sides (D1).
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
| Card | + extension `https://theuth.io/a2a/ext/members/v1` `{chains: [{chain, versions: [{version, members, sigs: [{signer, sig}]}]}]}`: for each record that lists the caller, the last 8 versions, each with up to 16 signatures (members of the previous version first, oldest first, then pinned signers). Omitted while no chain exists. The same payload is the body of a `NOTICE_PATH` push, which is now read up to 192 KiB (today the body is ignored and the route answers 204, `A/contacts/reach.rs:241-243`). |
| Contact-only route `GET ROSTER_PATH?chain=<id>&pin=<eid>&from=<version>` | The proof path: the signed copies from the pin's first signed version to the latest, in greedy hops, with every content at each hop version. At most 64 hops and 256 KiB per answer; `from` continues. It serves only the caller's own records, and `pin` must be one of that record's pins. It runs under the contact's rate and concurrency bounds (D1). |
| A2A task id | `<16 hex engine prefix>-<uuidv7>` |
| Migration | **`0059_mesh_contacts.sql`** (P1): `contacts`, `contact_endpoints`, `contact_grants`, `a2a_exports`, `contact_addrs`, `sessions.contact_inbox`, and `contact_threads` rebuilt (new table, copy with the new `id`, drop, rename; index kept). **`0060_contact_rosters.sql`** (P2): `contact_rosters`, `contact_roster_heads`, and `contact_roster_local` (local); it drops `contact_votes` if 0059 created it. **`0061`** (P3): `contact_holds`. 0058 is skipped: another branch holds it for `image_refusals`. The highest today is `0057_inbox_ceiling.sql`. |
| Attach | `replicate_records` (`crates/agent/src/store.rs:913`) + `contacts`, `contact_endpoints`, `contact_grants`, `a2a_exports`, `contact_threads`; + `contact_rosters` and `contact_roster_heads` in P2; + `contact_holds` in P3. `contact_roster_local` is never attached. |
| `SESSIONS_WIRE_SCHEMA` | 53 → **59**, the number of 191's migration that adds `sessions.contact_inbox`. At landing it follows that migration's number (D2). |
| `peers.yaml` | after import: HTTP peers only. The structs are unchanged, so an old file still loads. |
| Config | − `a2a.contact_cap`. `A2aConfig` is not `deny_unknown_fields` (A/config.rs:27-28), so an old `config.yaml` still loads. No new key. Constants: a roster holds at most 256 members; a card carries 8 versions per chain and 16 signatures per version; the roster signature's domain prefix is `theuth/a2a/roster/v1`; the notice body is capped at 192 KiB; pushes back off up to 10 minutes. |

## Phases
- **The order differs from the recon's** (there: P2 routing, P3 endpoint sets). Here endpoint sets come first (P2), and routing follows (P3). With sets first, a session on any machine dials with its own key. Routing first would need an outbound relay through the pairing machine, which the standing rule rules out, and P3 would only delete it again.
- **Every phase is safe on its own.** P2 adds the owner check of D3, so a reply that lands on the wrong machine is refused and the client tries the next member.
- **Attention from every machine moves to P3.** Before P3, a hold on another machine waits; it is not released.
- **Why not three phases:** one phase would grow to about 2,000 production lines, too much for one review round.

| Phase | What | Prod | Tests | Proof |
|---|---|---|---|---|
| **P1 RECORDS** | 0059 and the attach; import and pointer merge; staging from the tables with per-row quarantine; reconcile on the content hash; `contacts_ready`, including the actor-start cap; the replicated binding id, never un-revoked; contact RPCs write the tables; export ids; `sessions.contact_inbox` | 1,200–1,500 | 1,000–1,200 | Rig cases 5 and 6; a contact paired on n1 is listed on n2 |
| **P2 ROSTERS** | 0060. Per-record chains: signing and co-signing in the reconcile; `contact_rosters` keeping the hop chain plus the last 8 versions. Per-machine C-side state (`contact_roster_local`): the jump-and-replay accept rule with the equal-version switch; bootstrap from the pin through the proof path (`ROSTER_PATH`), using untrusted hints and at least two sources. Divergence detection through `contact_roster_heads`, recomputed at every refresh. The card extension and the `NOTICE_PATH` push, with its body cap and backoff. Refresh from every trusted member. Gate and dialing over the group's union, with per-endpoint hints. Merge proof and attribution. The ceremony-only deny lift. The `contacts_ready` check before a ceremony. Owner refusal on contact paths. | 850–1,050 | 700–900 | Rig cases 2, 4a, 4b and 7–12 |
| **P3 ROUTING** | `a2a_forward`; owner resolution; offline refusal; forwarding of `~return` and task ids; inbox home; lifting the main-branch `INBOX_STAYS` refusal (4bc0dddd) together with forwarding (D4); `contact_holds` (0061) and Attention from every machine; completing tasks on drain; **W-40 deleted** | +900–1,100, −600–650 | +850–1,050, −360 | Rig cases 1 and 3 |
| **P4 UI and docs** | D8; site docs; as-built | 400–600 | 400–500 | Vite fixture screenshots; the whole rig |
| Total | | ≈ 3,350–4,250 gross; ≈ 2,750–3,600 net | ≈ 2,950–3,650 gross | |

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
2. **Sending from n2.** A session on n2 calls `send_message(peer:c/<export>)`. n2 dials C with n2's key, and C admits n2 because n2 is in our roster. The answer returns through `~return` to the session on n2.
3. **n2 off.** Stop n2. C's message to X reads "the chat's machine is offline" in under 1 s, and nothing is queued.
4. **A machine removed.**
   - **(a) Honest removal of the pairing machine n1.**
     - `mesh_remove(n1)` on n2. n2 signs a roster without n1 and pushes it.
     - C accepts it (signer n2 is in C's current roster, version + 1).
     - C refuses n1's streams (log: gate refused), and C→n2 works.
   - **(b) Modified n1.** Stand in for modified code by data edits, then sign first:
     - stop n1 and n2; start a fourth scratch engine Z1 and pair it with n1 only;
     - write a tombstone for n2 straight into n1's `mesh.sqlite` (as 183's Q5b did);
     - start n1 first, so it signs and pushes the roster {n1, Z1} at a higher version.
     - **Assert that C is confused:** it admits n1 and Z1 and refuses n2. This is the stated limit.
     - **The remedy.** Remove n1 from the mesh, on n2. On both sides, remove the contact: every record of the group gets a tombstone. Then pair fresh from n2, which makes a new record with a new chain at version 1. Assert that:
       - n1 and Z1 are refused;
       - the old record's roster rows are ignored;
       - the removed record is never revived, and its deny entries do not block the new pairing;
       - n2 works.
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
7. **An existing contact gets its roster.**
   - Start from a P1 record: C pinned n1 only, and no chain exists. Start n2 first, so n2 signs version 1. C ignores n2's copy, because n2 is not its pin.
   - Start n1. It co-signs version 1, and C adopts n1's copy as its base. From then on, C admits n1 and n2.
8. **A contact that missed 9 or more versions.**
   - Stop C. Make 10 membership changes: add a fifth scratch engine n5, remove it, and repeat five times. That is versions 2 to 11. Our machines carry only versions 4 to 11.
   - Start C, still at version 1. It jumps to version 4, signed by n2, a member of its version-1 roster, then replays to version 11. It ends with n1 and n2 admitted and n5 refused.
9. **A new C machine after the pinned machine is retired.**
   - C pinned n1. Remove n1 from our mesh, let C apply the removal, then **stop n1** (retired and offline).
   - Add a new machine, c2, to C's mesh. It receives the record and c1's `contact_roster_heads` row, but no roster state.
   - c2 dials n2, which it found only through the untrusted hints in c1's row, and asks for a proof path from n1's first signed version. It verifies each hop itself from the pin and ends at our latest version: n2 admitted, n1 refused.
   - Stop n2 too, and add a third machine c3. c3 cannot bootstrap: it denies the record and shows "cannot verify this contact's machines". It never admits n1.
10. **A divergence on C's side.** C's mesh has c1, c2 and c3.
    - Stop c3. Remove n1 from our mesh (`mesh_remove(n1)` on n2); c1 and c2 apply it.
    - Make n1 modified, with data edits as in case 4b, so that it signs version 10 as {n1, Z1} while our chain is at version 9.
    - Start c3 while c1, c2 and n2 are unreachable from it, so that c3 accepts n1's version 10. Then reconnect everyone.
    - Assert that no conflict is set yet: our chain is at 9, so the paths c1 and c2 fetch stop short of version 10, and c3 alone follows the rogue roster (the stated limit).
    - Make one membership change on our side, so that we sign an honest version 10. Assert that c1 and c2 now verify a path to version 10, find a different hash there, and set `conflict`. **All three** machines deny the record and show "this contact's machines disagree about yours; remove and pair again".
11. **Deny lifts.**
    - **A late duplicate pinned to the rogue machine.** After the remedy of case 4b, write an older-`paired_ms` record pinned to n1 into C's tables (a stale import). n1 stays denied.
    - **A listing by another contact.** Pair a second contact A whose roster lists n1. n1 stays denied, and is never admitted as A.
    - A fresh ceremony from n2 lifts only the endpoints that the new record's roster lists.
12. **An honest same-version fork.**
    - Partition n1 from n2. Pair a fifth scratch engine n5 through n1 and a sixth, n6, through n2, so that n1 and n2 each sign a different version 2.
    - Let c1 fetch only from n1 and c2 only from n2. Heal the partition.
    - Assert that c1 and c2 both end on the higher-hash version 2, that `conflict` is never set, and that our side signs version 3 with both n5 and n6. Neither contact machine denies the record.

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
  - **Accept rule:**
    - a copy of another record's `chain` is refused;
    - a copy whose signer is not a current member is refused;
    - an equal or lower version is refused;
    - at an equal version, the higher content hash wins;
    - a bad signature is refused, and so is a signature made without the `theuth/a2a/roster/v1` prefix (for example a card JWS signature);
    - a roster over 256 members is refused (our side never sends one; it signs the first 256).
  - **Jump and replay:** a contact at v1 that is offered only v4 to v11, with v4 signed by an old member, ends at v11. Each step needs a signer from the roster current at that step. Offered only copies signed by machines it does not trust, it stays at v1.
  - **Co-signing:** every member co-signs the latest version once, and a machine whose view differs signs v + 1 instead. A legacy record adopts the copy signed by its pin, not one signed by another member.
  - **Per-machine state:** a C machine moves forward only on our signed copies, verified against its own state. It never adopts a replicated row from another C machine. Our side keeps the hop chain plus the last 8 versions, and signs nothing for a removed record.
  - **Bootstrap:** a machine without state verifies a proof path from the pin, including when the pinned machine has been retired.
    - A path with a hop not signed by a member of the previous hop is refused.
    - Hints from `contact_roster_heads` are used only to dial; a forged hint list cannot change the result.
    - With two of our machines reachable, it asks both and takes the highest verified path.
    - With no verifiable path, it denies the record and never admits the pin alone.
  - **Proof-path route:** it serves only the caller's own records; a `pin` that is not one of that record's pins is refused. An answer holds at most 64 hops and 256 KiB, and `from` continues it.
  - **Divergence:** a conflict is set only when a verified path reaches the other head's version and the heads still differ there. A path that stops short sets nothing. `conflict` clears once the heads agree. A lower head that appears in the other machine's history is merely stale; still behind after 3 refreshes, it fetches a proof path. A machine with no peer rows flags nothing.
  - **Honest tie:** at an equal version, a machine replaces its head with higher-hash content signed by a member of the roster before the head. The losing signer on our side co-signs the winner, and its next change signs v + 1 from the winner. No conflict.
  - **Removal:** removing the pairing machine leaves the others admitted.
  - **Remedy:** removing the contact tombstones every record of the merged group, and its roster rows are then ignored.
  - **Remove, then pair fresh with the same endpoints:** the new record does not merge into or join the tombstone. Its binding is fresh and it has no grants. C bases it on the new chain, not the removed one, which is never carried. A later removal reaches it.
  - **Deny lift:** only a ceremony newer than the removal lifts a deny, and only for endpoints in the new record's own roster. A roster alone, another contact's listing, or an older pin never lifts one.
  - **Version gap with newcomers:** a contact at v1 is offered v4 to v11, where v4 is signed by 20 newcomers and one member of v3 that was already in v1. The 16 carried signatures still include that member (previous-version members first, oldest first), and the contact reaches v11.
  - **Carried signatures:** members of the previous version come first, oldest first; then pinned signers; at most 16.
  - **Cap:** a membership of 300 is signed as its first 256, pinned machines first, then the oldest; a removal still propagates.
  - **Bounds:** C keeps one local state row and one heads row per record per machine. It never stores a copy that did not qualify. A notice body over 192 KiB gets 413. Pushes go only to live contacts and back off up to 10 minutes.
  - **Merge proof:** two records pinned to c1 and c2 of one mesh merge. A contact that lists another contact's pinned machine merges nothing, and that machine stays admitted for its own record.
  - **Attribution:** an endpoint listed by two records is admitted only for the one it pinned or signed for.
  - **Missing extension:** a card without it changes nothing.
  - **Re-check on arrival:** a roster row that arrives by replica triggers the signing check, as well as a `peers_gen` bump does.
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
  - A machine removed from the mesh is never told (M/net.rs:1272-1297). After 30 s it would serve contacts from its copy, until those contacts get the next roster from our other machines: seconds while they are online (D1).
- **The window after a removal (D1).** It lasts seconds while the contact is online. While the contact is offline, it stays open until the contact next fetches a card.
- **The limit of automatic membership (D1).** A member running modified (malicious) code, or an unmodified machine operated by a thief (who can `mesh_remove` the honest machines from its UI), can sign a roster that excludes the honest machines, and the contact follows it. A contact machine that accepted a rogue roster while it was stale is detected by divergence: every machine of that contact denies the record and shows a problem. **Remedy: manual, and both sides must act.** Until the contact's own user removes the contact, the contact keeps admitting the rogue machine (or, after a divergence, denies us altogether):
  - remove the machine from the mesh;
  - remove the contact on both sides, which tombstones the whole group;
  - pair fresh as a new contact;
  - review the grants and exports the rogue machine wrote.
  
  Sky's answer 1 is automatic for lost, retired or honest-code machines.
- **Residuals of the roster (D1).** A legacy record whose pinned machine was lost before it ever signed, or a contact for which every trusted member was removed, needs the contact removed and paired fresh.
- **A merge ends conversations, once.** The next message starts fresh.
- **Write contention.** Replica apply takes `BEGIN IMMEDIATE` on the single `sessions.sqlite` write connection (R/apply.rs:454). The rows are tiny and the writes rare. Watch it in the P1 rig.
