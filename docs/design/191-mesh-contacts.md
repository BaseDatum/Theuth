# 191 — MESH-CONTACTS: a contact is a contact for every machine of the mesh

Status: contract. D1 is final after nine review rounds; D3 to D8 and the phases were revised after review p34. Written at main d10feaa8, after 190 landed. Recon: `/tmp/d191/recon.md` (written at 57840336). D1 and D2 were re-read at d10feaa8. D3 to D8, the Contract, the Phases and the tests were re-read at main 3de3b590, and at the P1 head a4915b97 for what P1 built. Their line numbers are hints: P1 and P2 move them, so a builder re-finds each by symbol. "P1 notes #N" below are the numbered entries of P1's as-built notes (`docs/design/191-notes.md` on branch `feat/191-p1`).

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
- **P2 notes from the final review.** These add to the rules above and change nothing in them.
  1. **Co-signing.**
     - A loser's co-sign counts as a signing, so it is pushed like any other.
     - A machine that jumped or bootstrapped verifies the winner through an old member's co-signature.
     - For an equal-version replacement, a signer from the head's **own** roster is accepted as well as one from the roster before it. That gives no extra power.
     - At most one equal-version flip per record per refresh, so hash grinding cannot flush the 64-entry history.
  2. **Inflation alert (our side).** A machine of ours that is synced (`contacts_ready`, with no parked rows pending) can see either of two things, for two refreshes:
     - a proof-path request `to=<V>` above its latest version;
     - a head hash it never signed.
     
     It then raises `Contact.problem` and an Attention item: "a machine of <contact> follows a roster we never signed; one of ours may be compromised". It **only alerts** and never denies, so a partition cannot lock out an honest contact.
  3. **Fresh-machine adoption.** A machine with no state adopts a verified head only if one of these holds:
     - it equals a peer's head;
     - it appears in a peer's history;
     - two independent sources of ours agree on it.
     
     If peers are present and none of these holds, it denies with "cannot verify this contact's machines". With no peers at all, it uses its verified path. That is in the limit class: a pin-holder serving an inflated path wins there.
  4. **Heads rows.** Rows from machines no longer live in C's mesh are ignored. A `conflict` flag that is not refreshed within 3 refreshes expires, so a retired machine's flag cannot deny the record forever.
  5. **Pruning.** The hop chain is recomputed before any version is dropped, so continuity survives the 8-version window.

### D2. Replication: last-writer-wins tables in `sessions.sqlite`
- **Mechanism.** `replica::attach`: per-column LWW under HLC, tombstones, version-vector catch-up (R/lib.rs:1-14, :104-121; R/sync.rs:1-14). The record tables are appended to `replicate_records` beside `projects` and `cost_owners` (`crates/agent/src/store.rs:903-915`, attach at :913).
- **No floor gate is needed.** A machine without these tables parks their batches: up to 65,536 change sets for up to 7 days, replayed at attach (R/apply.rs:27, :29, :75-93).
- **`SESSIONS_WIRE_SCHEMA` is set to the number of 191's own migration**, for the inbox marker (D4). Main has 53 (`crates/agent/src/store.rs:670`) and P1 sets 59: the rule is "set this to a new migration's number" (the bump rule is the doc comment on the constant, :659-669; the snapshot test `sessions_wire_schema_tracks_the_replicated_columns` is at :4258-4305).
  - Any other value is wrong. With 54, an unrelated 0058 bump would read as newer, and this build would ship `contact_inbox` rows to a peer that lacks the column. `sqlite3changeset_apply` would skip the table while the cursor advances (`crates/sessionsync/src/apply.rs:80-109`): silent loss.
  - `0058_image_refusals.sql` has landed on main. 191's migrations are 0059, 0060 and 0061 only if those are still the next free numbers when 191 lands: check `crates/agent/migrations` then. If they are not, the bump follows the number of the migration that adds `sessions.contact_inbox`.
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
| `contact_holds` (P3) | the A2A task id | `contact`, `machine`, `export`, `session`, `owner` (engine id), `since_ms`, `waiting`, `preview` (cut to 200 bytes), `until_ms` | the owner only. The row is deleted when the hold ends (D4). |

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
    5. Release holds whose grant changed: allow → start; block or removal → deny (N:3497-3527). The release checks the owner predicate first: a session this machine no longer owns fails the task (D3).
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
  - chat export → `a2a_exports.session` → `sessions.owner`;
  - agent export → the session whose `contact_inbox = <binding>|<export id>` → its owner; with no marked session, the export's home (D4). Two marked sessions for one key are refused, never chosen between (D4);
  - `~return` → `contextId` → `contact_threads` → session → owner;
  - `GetTask` and `CancelTask` → the engine named by the task id prefix (see below).
  - The owner is read with the strict predicate below. A missing row, an unreadable row and an empty owner are neither "here" nor "elsewhere": they refuse.
- **Serving.** When the owner is this machine, the request is served as today. When the owner is elsewhere, **`a2a_forward`** (see Contract) carries it over `theuth/rpc/1`, the way `peer_deliver` travels (`crates/agent/src/communication/mesh.rs`, `forward` :94-119; marker `Forwarded(peer)`, `crates/rpc/src/forward.rs:126`, set by `serve` at :495).
- **What `a2a_forward` carries.** One request line and one answer line: it is unary.
  - **Methods and paths.** A JSON-RPC `POST` to `/rpc`, `/agents/<export>/rpc` or `/agents/~return/rpc` (`RETURN_ROUTE`, F:27), whose method is `SendMessage`, `GetTask` or `CancelTask`. The receiving machine reads the method and the ids it routes by (`params.id`, the message's `contextId`) and sends the body on unchanged. `ListTasks` is answered by the receiving machine, for its own tasks (below).
  - **Answered locally, never forwarded.** The card (`GET CARD_PATH`), the notice (`POST NOTICE_PATH`) and the proof path (`GET ROSTER_PATH`, D1). Each runs on the receiving machine from its own state, under the contact's bounds.
  - **Streams are refused at the forwarder.** The router still serves SSE (`send_streaming_message` and `subscribe_to_task`, A/server.rs). Only our own client refuses it (`no_streams`, A/contacts/reach.rs:370-372), so a contact of modified code can ask for a stream, and a unary call cannot carry one. The receiving machine answers `SendStreamingMessage`, `SubscribeToTask`, the push-config methods and every other method with `UNSUPPORTED_OPERATION` and our client's own words for streams ("streaming is not carried to contacts yet; poll GetTask").
  - **The owner's check is the one `serve` makes.** `gate(caller)` must be `IrohGate::Contact` (A/contacts/reach.rs:161) and `admit_contact` must give a permit (rate and concurrency). `ContactAlpn::admit` is only `!= Refuse` (reach.rs:116-121): it also admits a stranger inside an open Add contact window, so a forward never rests on it. Both run on the owner's own tables, after its `contacts_ready` (D2). An owner that is not ready answers `OWNER_UNREACHABLE`, which the receiving machine reads as an offline owner.
  - **Which failure is which.** RPC errors of the forward call: `NOT_OWNER` (-32013) and `OWNER_UNREACHABLE` (-32010). A transport failure of the call (not connected, a stream error, the deadline, a malformed line) is `OwnerOffline`. Everything the owner's router decides is the answer, `{status, body}`: 401, 404, 429, 503, or a JSON-RPC error inside a 200. The receiving machine sends `{status, body}` on unchanged, except a 401 (below). Any other RPC error becomes a refusal that carries the owner's words.
  - **Deadline.** The forward ends after 10 s, a new constant. The contact's own `CALL_TIMEOUT` is 30 s for dial, request and answer (reach.rs:39), and `mesh::TIMEOUT` is 30 s too (mesh.rs:32): with it, the contact would time out first and know nothing. At the deadline the receiving machine answers `OwnerOffline` itself. The request may have reached the owner; the contact's retry carries the same `messageId`, which admission dedups (it answers from the first message's task). Our own client never blocks (`returnImmediately: true`, A/client.rs:794), so an ordinary `SendMessage` is answered as soon as the owner has admitted it; a caller that does block on a busy chat meets the deadline.
  - **Size.** The answer body is capped where the contact's own client caps it, `a2a_client::MAX_RESPONSE_BYTES` (2 MiB, A/client.rs:13); a larger one is a failed forward, never a cut one. `mesh::call` parses its answer as a `PeerReceipt` and caps a line at 1 MiB (`MAX_LINE`, mesh.rs:35; `call`, :166-217). P3 makes it generic over the result type and gives `a2a_forward` a line cap above the body cap.
  - **Off the a2a mailbox.** The handler runs beside the network leg, like `peer_deliver`. It reaches the a2a context (gate, router) through a handle the plugin publishes once, as `Gate` holds a `Weak<Ctx>` (reach.rs:114). It never asks the a2a actor per call, which is what the contact RPCs do (`contacts()`, `crates/rpc/src/api.rs:4204`): that mailbox runs the reconcile, which hashes the tables and asks the agents (D2), and a forward queued behind it would spend its deadline waiting.
- **What the owner does with a forward.**
  - It refuses a call without the marker, as `contact_deliver` does (`crates/rpc/src/api.rs:4524-4530`).
  - It runs the check above on `caller`, against **its own** tables.
  - It serves the request in its own router with `ContactCaller(eid)` (A/contacts/reach.rs:178-209).
  - Admission, the first-contact hold, the A2A task, `deliver_full`, inbox limits and receipts all run on the machine that holds the session. There is no envelope, no capped turn and no clamp.
  - **A caller the owner has not learned yet.** The receiving machine can know a new machine of the contact before the owner does (a push or a card fetch reached it first, D1). The owner's gate then answers 401. The receiving machine does not pass that on as "not a contact there": it answers a transient refusal, "the chat's machine has not accepted this machine of yours yet; try again shortly" (`UNSUPPORTED_OPERATION`, so the contact records it as refused, and its retry works once the owner has the roster). Nothing ran and nothing is stored. The same answer covers a caller the owner has already dropped: whichever machine is behind catches up (D1).
- **Owner offline: refused at once (answer 4).**
  - The receiving machine checks `transport.peers()`, the live control connections (M/net.rs:978-986). If the owner is not among them, the contact gets `ContactRefusal::OwnerOffline`, "the chat's machine is offline" (`crates/agent/src/communication.rs:1100`), with no wait (mesh.rs:176-178).
  - An owner that is still listed but dead (a frozen process, a half-open connection) gives the same text when the forward deadline passes: 10 s, below the contact's 30 s (above).
  - If the chat moved meanwhile, the owner answers `NOT_OWNER` (-32013, mesh.rs:126), and the receiving machine resolves again, once. **A second `NOT_OWNER` goes back to the contact**, whose client tries its next member (below). A call the owner received as a forward is never forwarded again (as `rpc::forward::route`: a `Forwarded` call is served here or refused `NOT_OWNER`, forward.rs:327-330), so a stale view costs one extra hop and never a loop.
  - `OwnerOld` goes, under the standing rule.
- **Outbound: from a session on any machine.**
  - Every machine commits a route for every admitted contact, so "unknown peer" (N:4035) goes for contacts.
  - The sender dials with **its own** key. `PeerClient::contact` (A/client.rs:388, built in the a2a reconcile pass, A/lib.rs `pass`) and `IrohTransport::call` (A/contacts/reach.rs:277-289) take the live members of **this machine's own accepted roster** of the contact (D1: local state, never replicated). They try first a member with a live connection, then the last one that answered, then the rest, all within `CALL_TIMEOUT` of 30 s (reach.rs:39).
  - A `NOT_OWNER` answer moves on to the next member.
  - Each machine keeps its own card cache (`full_shape` needs the offer, F:226-238) and its own hints (`contact_addrs` behind `ContactAlpn::hints`, reach.rs:137-148).
  - **A machine of ours the contact has not accepted yet.** A new machine X of our mesh can dial before the contact's roster of us lists it (D1: seconds while the contact is online). The contact's gate refuses X and closes the connection with the reason `not a contact` (`A2aGate::accept`, M/net.rs:2038-2041), which X's dial sees as a refusal, not as an unreachable machine. When every member of the contact that answered refused, the send's receipt is `failed` with "<contact> has not accepted this machine yet". There is no relay through the pairing machine (standing rule), and no retry beyond the contact's own members. The text goes away on the first send after the contact's next card fetch or push lists X. It reads the same when the contact has blocked or removed us: the dialer cannot tell the causes apart, and is not told.
  - **No relay**, under the standing rule. A machine that reaches no member reads "cannot reach <contact> from <machine>". With the default relay (`crates/mesh/src/config.rs:62`) that only happens with `mesh.relay: none` and no shared LAN (M/net.rs:65-67).
- **The return route, and `contextId` across machines.**
  - The sender mints `contextId` for a fresh conversation (F:239-257). Its `contact_threads` row (outbound = 1) replicates.
  - The contact answers on `~return` at whichever of our members it dials. That machine resolves `(binding, contextId)` and forwards to the session's owner.
  - The owner runs `full_return` against its own thread row, binding and offers (F:621-742). `full_intake` enqueues locally (F:366-399), which is correct because the owner holds the session and the predicate below has said so.
  - Inbound conversations (outbound = 0) work the same way.
  - A thread row that has not replicated yet reads `TASK_NOT_FOUND`, which fails closed. The replica polls every 200 ms (R/engine.rs:30), and an answer follows a model turn.
- **The owner predicate on the contact paths (strict; from P2 on).** `enqueue_peer` (`crates/agent/src/supervisor.rs:2228-2322`) does not check the owner, and `Agents::session` (:925-1105) loads actors without one. So a contact path serves a session only when **its row exists and `sessions.owner` is this engine** (`Store::engine()`, which is `''` on a lone engine and so matches that engine's own `''` rows). Anything else is `NOT_OWNER`.
  - **Where.** In `deliver_full`, `full_intake` and `full_return` themselves (F:272, :366, :621), not in a caller: no route to a session can skip it.
  - **Today's helpers all read "unknown" as "here", so none of them is this check.**
    - `Agents::session_owner` answers `Ok(None)` for a missing row and for an empty owner (supervisor.rs:5947-5954).
    - `rpc::forward::admit_forwarded` ends in `_ => Ok(())`, so an unknown row passes (forward.rs:300-309).
    - `SessionSummary.here` is `owner.is_empty() || owner == me` (store.rs:502).
    - `start_or_queue` takes membership of `NetworkState.inboxes` as proof of locality (N:1640). That set is insert-only: inserted in `handle_inbox`, `install_contact_limits` and `clear_contact_limits`, never removed, so an inbox that was drained still reads as local.
  - **Unknown and unreadable refuse.** A missing row, a store error and an empty owner on a mesh engine are all `NOT_OWNER` (a store error is logged as well). Nothing is read as local on a guess.
  - **No second hop.** A call that arrived as a forward is served here or answered `NOT_OWNER`; it is never forwarded again. A second `NOT_OWNER` goes back to the contact (above).
  - Before P3, the contact's client moves on to its next member on `NOT_OWNER`. From P3 on, the receiving machine forwards first.
  - A session is never run here on a contact's behalf without this check.
- **Task ids name their machine.**
  - Today A2A task ids are UUIDv7 (N:1532), and `GetTask` reads the local `a2a_tasks` (N:2560-2581).
  - New ids take the form `<first 16 hex of the admitting engine id>-<uuidv7>`. `GetTask` and `CancelTask` are forwarded to the engine the prefix names.
  - An id with no matching prefix (one minted before 191) is served locally.
  - `ListTasks` answers this machine's tasks only. No theuth code calls it (no `.list_tasks(` caller in `crates/`).
- **A session taken away: its waiting tasks and its holds.** Nothing in the agent network reacts to an owner change today (`SessionDrained`, `crates/agent/src/supervisor/transfer.rs:223-243`).
  - On drain, the old owner completes the session's `full_inbound` tasks (N:625) as receipts: the message already sits in the queue that moved.
  - **A drain never releases a first-contact hold.** A hold waits for the user's Allow, and the message was never admitted into the chat, so there is nothing to forward: no `a2a_forward`, no `peer_deliver`, and a delivery from here would skip the Allow. On drain the session's holds **fail** with "the chat moved; send again", and their `contact_holds` rows go. The contact's resend reaches the new owner, which holds it afresh if the answer is still open (D4).
  - **A hold is released only by a grant change** (D2 reconcile, step 5: allow → start, block or removal → deny). `start_held` (N:3531-3574) checks the owner predicate first: a release whose session is no longer owned here **fails the task** with "the chat moved; send again" instead of starting it. This also covers a session taken without a drain: a Write take from an unreachable owner reaches only `demoted`, which clears the draining mark and drops no hold (`transfer.rs:264-271`). `HOLD_TTL` (24 h) ends whatever is left.

### D4. The agent-export inbox: one home, a marker that travels with the session
- **Home.** An agent export names its machine as `<agent>@<engine>` (N:3043-3045), stored as `a2a_exports.home`. For `theuth`, the home is the machine whose UI made the export; on import, it is the importing machine.
- **Only the home creates inboxes** (`handle_inbox`, N:3037-3140), so creation never races. A message for an agent export with no marked session is forwarded to the home. If the home is offline, the message is refused at once.
- **Marker: `sessions.contact_inbox TEXT NULL`** (`<binding>|<export id>`). The home sets it when it creates the inbox session. `sessions` is a session-log table (capture.rs:19-40), so the marker reaches every machine in the same changeset as the session row. A record-replica map would arrive on a different stream than the session and could lose a race with a take.
  - Routing reads the marker; there is no separate map table. P3 adds an index for that lookup (`sessions(contact_inbox)`, partial, in 0061).
  - A deleted inbox takes its marker with it. The next message goes to the home, which makes a new inbox.
  - **The marker is best-effort, so it is trusted only where it is there.** P1's `handle_inbox` writes it only when the export's id is known and only logs a failed write. The backfill (`backfill_contact_inboxes`) runs only in a pass after the latch, up to 150 s after boot (D2). So an inbox can exist unmarked, and a marker can hold the export's name instead of its id (an inbox imported for an export no longer known by name keeps the name, P1 notes #8). A name-valued marker caps a moved inbox and is never routed by or adopted. The rest of this section treats "set, readable and names a live `a2a_exports` row by id" as marked.
- **The moved-inbox fail-open.**
  - Today inbox caps live only in the local `a2a_contexts` (capture.rs:4-5). On a machine that took the inbox, `a2a_inbox_identity` finds nothing and `reinstall_contact_limits` returns `Ok(())` with no limits (N:3192-3195). `ContactScoped` then says false (`crates/agent/src/loop_.rs:5036-5049`).
  - The separate fix on main (`INBOX_STAYS`, landed in 4bc0dddd) makes `sessionsync::owner::move_to` and `answer_release` refuse inbox sessions, so an inbox cannot move by a user action.
  - The case left is a Write take from an unreachable owner, which the owner cannot refuse. P1's marker closes it for a marked inbox: a session with `contact_inbox` set and no local `a2a_contexts` row runs **capped** (`INBOX_FAIL_CLOSED`, N:3146, and an empty allowlist), with the contact prompt. An unmarked inbox cannot be told from any chat by its taker: that is the residual window in Risks.
- **P3 lifts `INBOX_STAYS` last, and only for a marked inbox.**
  - **Why not for every inbox.** The marker is best-effort (above). An unmarked inbox that moved would reach its taker with no row and no marker, and `reinstall_contact_limits` returns `Ok(())` there (`Ok(None) => return Ok(())` when `contact_inbox` is unset, in P1): it would run with no limits at all. That is the moved-inbox fail-open again, so the lift cannot be unconditional.
  - **The rule.** `move_to` and `answer_release` keep refusing with `INBOX_STAYS` unless the session's marker is set, readable and names a live export by id. An unmarked inbox, a name-valued marker and a failed marker read all stay home. The lookup error arm of `answer_release` stays as it is. `INBOX_STAYS` (`owner.rs:40`), the wire frame `Msg::Refused` (`crates/sessionsync/src/types.rs:184`, kept generic) and the two checks (`owner.rs:556-563`, :676-688) all stay; only their condition narrows.
  - **Order.** The lift is the **last commit of P3**. Forwarding, `NOT_OWNER`, adoption (next) and the capped-then-full path are in, and tested, before it, so no commit of P3 lets an inbox move before they hold.
  - **Proof.** A marked inbox is moved: its next contact message finds it capped, then full. An unmarked one still cannot move (unit tests and rig case 15). The two tests that pin the refusal today, `move_of_a_contact_inbox_is_refused` and `take_of_a_contact_inbox_is_refused_by_its_owner` (`owner.rs:1408-1494`, helper `mark_inbox` :1393), keep their unmarked case and gain a marked one.
- **Adopt by marker (P3).** Today `handle_inbox` finds an inbox only through the node-local `a2a_contexts` row, keyed `(principal, export name)` (`a2a_inbox`, S:2015-2029), and otherwise mints one (`new_root`). On a machine that took a marked inbox there is no such row. Without adoption the taker would mint a second session with the same marker, and the marker lookup of D3 would become ambiguous. So `handle_inbox` runs, in order:
  1. **The local row**, as today. If it names a session whose row says another machine owns it, the answer is `NOT_OWNER` when that session carries this marker (the inbox simply moved), and the home-only case below when it does not.
  2. **The marker.** Otherwise it reads `sessions` by `contact_inbox = <binding>|<export id>`:
     - exactly one session, and its owner is this engine: **adopt** it. The live persona must be known (S6, as for a fresh inbox; otherwise `export unavailable`). Then the local `a2a_contexts` row is written (`create_a2a_context`, then `mark_a2a_inbox` with the live persona and ceiling), the limits are re-read, and the inbox runs full. This machine need not be the home.
     - exactly one session owned by another machine: `NOT_OWNER`, and the router resolves again (D3).
     - **more than one session**: refused with `export unavailable`, logged with both ids, **never chosen between**. Each keeps running capped (marker, no local row) until the user deletes the extra one.
     - none: only the home mints (`new_root`, then the marker). Any other machine answers `NOT_OWNER`, which sends the router to the home.
  3. A name-valued marker is never a match (above).
  The adoption write is the whole re-admission. Until it, the moved inbox is capped, and a message that arrives while `contacts_ready` is false is refused as for any inbox (D7).
- **Residual: an inbox that moved before 191.** A session moved by a take in a build without `INBOX_STAYS` cannot be marked, because only its owner writes its row (`set_contact_inbox` touches only rows this engine owns). Its home still holds the old `a2a_contexts` row. When the contact's next message reaches the home and finds that row's session owned elsewhere and unmarked, the home unmarks its own row (`unmark_a2a_inbox_full`, S:2080-2088) and opens a new, marked inbox locally, so contact messages never reach the moved session again. **191 never caps the moved session.** It has no marker and no local row on its taker, so it runs there as the taker's own ordinary session, and it receives no contact input.
- **Home removed.** An export whose `home` is tombstoned in the mesh `peers` table is listed paused, `Export.paused` = "its machine left the mesh" (`Export` has no `problem` field; P1's `a2a_exports` already uses `paused` for "chat not on this machine").
  - It gets no new inboxes.
  - Existing inbox sessions that were moved elsewhere keep working through their owner.
  - The UI offers to rehome the export: `a2a_export_put` with `home` set to this machine.
- **First-contact Attention (P3).**
  - Holds stay on the machine that admitted the message: the owner (`hold_task` and `release_held`, N:3403-3527, in memory). They are holds on a *task*. P1's `CommitPeerConfig.hold` keeps the *binding* of a record that is quarantined for replication lag. The two share a word and nothing else.
  - The owner mirrors each hold as a `contact_holds` row, carrying the `FirstWaiting` fields (A/contacts.rs:107-134). It deletes the row when the hold ends, and deletes all its own rows at boot.
  - `contact_first_waiting` and the `contact_first` Attention source (`Ctx::first_waiting` and `FirstContactItems`, A/contacts.rs:2087-2191) read that table, not the agent's in-memory holds. They drop rows past `until_ms` (`HOLD_TTL` is 24 h, N:632).
  - **Rows from another machine are data.** Their `machine` and `preview` are cleaned on read before they reach the strip or Attention: control characters dropped, `machine` cut to 64 characters (as `held_line` does, communication.rs:1123-1133), `preview` to 200 bytes (as `preview_of` does, N:3635-3643). A row whose owner is offline stays until its `until_ms`; its Allow still writes the grant, so the contact's resend passes at once.
  - **Other machines refresh.** A replicated `contact_holds` row fires no `ContactsHeldChanged` here (the agent publishes it only for the holds it owns), and `A2aChanged` comes only from that event (A/lib.rs:956-970). So on another machine the strip (`ContactAllowStrip`) and Attention would never refetch. The reconcile already hashes `contact_holds` (D2): when that hash changes it publishes `A2aChanged` and nudges Attention (`nudge_attention`, A/contacts.rs:2077), whoever wrote the rows.
  - An answer from any machine is a grant write. `contact_first_answer` already writes `first_contact` or `blocked` (A/contacts.rs:1927-1988). **Only the owner releases the hold, and only on that grant change** (D2 step 5): the owner checks the owner predicate first, and a session no longer owned here fails the task (D3). A drain never releases a hold (D3). P1 left this release for P3 (P1 notes #13).
  - Until P3, an unanswered hold on another machine just waits, which fails closed.

### D5. W-40 is deleted (P3)
`grep -rn "W-40 only; removed by design 191" crates` lists 11 markers (main 3de3b590). They and the unmarked W-40 code go. After P3, that grep lists nothing, and `grep -rn "W-40" crates` finds no live branch: only comments that name the removal, if any. Line numbers are main 3de3b590 and only a hint: P1 and P2 move them, so the builder **re-finds each row by symbol**.

| Goes | Where |
|---|---|
| `default_contact_tools` and its users | A/config.rs:219-223; A/server.rs:609-610; `A/contacts/e2e_tests.rs:443`, :670, :719 |
| `ContactOrigin.cap` and `.tools` stamping; `ContactOrigin.holder`, which only W-40 reads; `ContactFacts.cap`, which P1's `stage_records` fills from `contact_cap` | A/server.rs:601-612; fields at `crates/agent/src/communication.rs:701-726`; `contacts/records.rs` (P1) |
| `PeerMeta.cap` and `.tools`, `ContactFrame` and `PeerInput.contact`, the turn-cap install for a new contact turn (and its clear when the message was refused), the PeerInput frame branch, `contact_message()` | `crates/agent/src/loop_.rs:220-226`, :232-245, :272-273, :1334-1348, :1381-1388, :1846-1862, :9099-9114 |
| `network_contact_frame` (a contact task is served here or refused, never run here for another machine) | N:3325-3351; called at `crates/agent/src/supervisor.rs:2287` |
| `CONTACT_GRANTABLE`, `contact_grantable`, their filter in the `SetAllowOnly` handler, and `grant_key`, which reads the same list (`MCP_GRANT` with it, if nothing else reads it). The guard below stays | `crates/tools/src/registry.rs:619-659`, :684-699, :1079-1089 (read by `allowlist_admits`, :1053-1077) |
| `CONTACT_NOT_OWNER`, `ContactForwarded`, `Message<ContactDeliver>`, `Message<ContactForwarded>`, `forward_contact`, `contact_deliver` and its owner clamp, `contact_turn_finished` | N:3680-3965 |
| the W-40 test in `start_or_queue` (the `local` test over `NetworkState.inboxes` and `session_owner`). A contact's message always goes to `deliver_full`, whose owner predicate decides (D3) | N:1632-1654 |
| the W-40 branch of `start_task` | N:1729-1750 |
| the W-40 branch of `network_turn_finished`; `resumed_contact_envelope` | N:2310-2323; N:2346-2378 |
| `NetworkState.contact_waits` and `contact_replies`, with their test helpers | N:608-614, :875-891 |
| `CONTACT_METHOD`, `METHOD_NOT_FOUND`, `ContactFailure`, `contact_forward` (the refusal mapping moves to `a2a_forward`). `NOT_OWNER` and `OWNER_UNREACHABLE` stay | `crates/agent/src/communication/mesh.rs:121-124`, :128-163 |
| `ContactEnvelope` and `ContactDeliver`, with their re-exports; `ContactRefusal::OwnerOld` | `crates/agent/src/communication.rs:728-758`; `crates/agent/src/lib.rs:163`; communication.rs:1086, :1101 |
| RPC `contact_deliver` | `crates/rpc/src/api.rs:46`, :247-251, impl at :4519-4550; `crates/rpc/src/export.rs:87`; UI `rpc/api.ts` :273, :1088-1089 and the `'contact_deliver'` entry at :1537 |
| `a2a.contact_cap`, the `PeerEntry.cap` readers, the `contact_cap` parameter of `stage` and `stage_records`, the config template line | A/config.rs:43, :57, :161-164, :554-583, :629, :711; A/lib.rs:170, :388; `contacts/records.rs` (P1); `crates/theuth/src/main.rs:1270` |
| `Contact.cap`, `ContactPatch.cap` and `ContactPatch.modes` (P1 left them for P3, P1 notes #13), **with the UI one-liner in the same commit**: `NO_PATCH` and `useGrant` in `Contacts.tsx` still name them, so `tsc` would break until P4 | P1 `crates/a2a/src/contacts.rs:88`, :156, :165; UI `components/settings/Contacts.tsx:43`, :645 |
| W-40 words in comments | `crates/agent/src/frame.rs:503`, :522; `crates/agent/src/communication.rs:677`; `crates/agent/src/supervisor.rs:5832`, :5940; `crates/a2a/src/config.rs:162` |
| Tests | `serve_contact_deliver` and `an_exported_chat_elsewhere_runs_one_capped_turn_and_answers_only_it` (`crates/agent/src/supervisor.rs:16307-16593`); the W-40 halves of the tests at `loop_.rs:10308-10360` and of `contact_sessions_see_only_their_allowlist` and `an_allowlist_from_a_peer_holds_only_grantable_tools` (`registry.rs:3384`, :3621), which move to the empty list |

- **What stays.** The fail-closed states use these, so they are kept:
  - `SetCap`, `SetAllowOnly`, `SetTurnCap`;
  - `install_contact_limits`, `reinstall_contact_limits`, `clear_contact_limits`;
  - the Resume cap and the journal source `contact`: old journals and "unsure" still cap;
  - `contact_baseline` (`crates/agent/src/frame.rs:502-518`);
  - `INBOX_STAYS`, `Msg::Refused` (`crates/sessionsync/src/types.rs:184`, generic) and the two refusals in `move_to` and `answer_release`: only their condition narrows (D4);
  - the UI's parse and cards for `<contact_message>` rows (`lib/injected.ts`, `HarnessCards.tsx`): transcripts the previous build wrote still hold them, and that data must still load.
- **`SetAllowOnly` keeps a guard when `contact_grantable` goes.** The filter in its handler keeps a list that came from another engine from granting more than the public web, and `grant_key` reads the same list. After W-40 every caller passes the empty list: the inbox callers of `install_contact_limits` (F:475, :496; N:3313) pass `Vec::new()`, and `reinstall_contact_limits` re-installs only what they stored. So the handler installs **only an empty allowlist**. A non-empty one is installed as the empty list and logged. It is never skipped, because a session with no allowlist entry drops out of `contact_scoped`, which the contact prompt and the LAN rule read (`registry.rs:1093`, `lan_allowed` :1114), and never installed as given. Under an empty list nothing is admitted whatever its key, so `grant_key` has no reason left.
- **Size.** About 600–650 production lines and 360 test lines, not the recon's 420.
- **What replaces W-40.** `a2a_forward` (D3) carries the request itself, not a capped turn. D4 covers agent exports.

### D6. Mesh to mesh
- **Both sides run the same code.** Each side's record of the other replicates the other's pins. Each machine keeps its own accepted roster of the other side as local state (D1), so two machines of one side can differ for a moment.
- **Any machine can dial any machine.** The receiving machine forwards to the owner (D3).
- **Bounds.** The rate and concurrency bounds are per principal **on each machine** (`admit_contact`, A/server.rs:290-302: 10 requests a second and 8 at once, `RATE_PER_SEC` and `CONCURRENT_PER_PEER`, :56-58). They are not shared: a contact that reaches N of our machines has N times the budget. A forwarded call spends a permit on the receiving machine and again on the owner (the owner re-admits it, D3), so the owner's own bound holds however many machines relay for it. Connection and stream budgets stay per endpoint (M/net.rs:47-49).
- **Pairing.** One ceremony runs between one machine on each side. Each side signs its record's roster at version 1, which covers every other machine of its mesh, and its other machines co-sign it. Both humans press Accept once. A side that is a single engine has a roster of one.
- **Family.** A machine of our own mesh is still refused, both ways (dial: M/net.rs:839-844; gate: M/net.rs:2029-2032).
- **A contact that joins our mesh.** Its record shows `problem: "now one of your machines"` (`problem_kind` `refused`, D8) and stays denied until it is removed.

### D7. Fail-closed, path by path
These hold as built (190 D6). Lines are main 3de3b590; re-find by symbol:
- S1: the `reinstall_contact_limits` fallback (N:3192-3237);
- S2 and S10: `full_ceiling` (F:575-591; S:283-302);
- S3: a revoked binding at actor start;
- S4: `apply_contact_limits` (N:3296-3320);
- S5: `drop_unrouted` (F:507-518);
- S6: the persona refusal (N:3047-3066);
- S7: the resume cap (`crates/agent/src/loop_.rs:1764-1832`; `crates/agent/src/store/durability.rs:1085-1110`);
- S8: no actor without limits (`crates/agent/src/supervisor.rs:1100-1105`);
- S9: the contact prompt (`loop_.rs:5034-5049`).

The new paths:

| Path | Stays closed by |
|---|---|
| Boot before sync | Gate and routes stay closed until `contacts_ready`: the mesh latch, then the first reconcile (D2). After that, the accepted cost under Risks applies. |
| An actor starts while not `contacts_ready` | `reinstall_contact_limits` trusts a local live binding (N:3202-3220), and a contact removed elsewhere still has one locally. So until `contacts_ready` every inbox starts capped (S1), whatever `inbox_full` says. That includes the gap between the latch and the end of the first reconcile. Only after the flip does `refresh_full_inboxes` lift the caps that are still due. |
| Boot before the roster joins (agent inbox) | S6 is unchanged, on the inbox's owner. |
| Missing or invalid agent file | S6 and S2 are unchanged, read on the owner. |
| Revoked binding | Revoked on every machine by the reconcile. An unknown binding resolves as missing, which caps (S3). It is never un-revoked. |
| Removal or block while this machine was down | The first reconcile after the latch runs `revoke_peer_bindings_except` (S:1134-1158) over whatever has synced. A change that did not sync, because every other machine was off, is the accepted cost under Risks. |
| Receiving machine and owner disagree on a removal | Both check: the receiving gate first, then the owner's own check on `a2a_forward` (`IrohGate::Contact`, a permit, `contacts_ready`, D3). A caller the owner does not list answers 401, which the receiving machine turns into a transient refusal; nothing runs (D3). |
| `a2a_forward` from outside the mesh | Refused without `Forwarded(peer)`, which only paired engines get (`crates/rpc/src/forward.rs:485-508`). |
| Owner offline | Refused at once (D3). A listed owner that never answers is refused when the 10 s forward deadline passes, below the contact's 30 s (D3). |
| Owner unknown or unreadable (missing row, store error, empty owner on a mesh engine) | Refused: the strict predicate reads none of them as local (D3). `session_owner`, `admit_forwarded`, `SessionSummary.here` and the `inboxes` set are not trust checks. |
| Contact paths reach a session that is not owned here (from P2 on) | `deliver_full`, `full_intake` and `full_return` serve a session only when its row exists and its owner is this engine, else `NOT_OWNER`. A forwarded call is never forwarded again, and a second `NOT_OWNER` goes back to the contact (D3). |
| A forward that is not a unary method of a JSON-RPC path (a stream, a push-config call, the card, the notice, the proof path) | Refused at the forwarder; the card, the notice and the proof path are answered on the receiving machine and never carried (D3). |
| A forward that outlives the contact's own timeout | The forward deadline (10 s) is below the contact's `CALL_TIMEOUT` (30 s), so the contact hears our answer. Its retry carries the same `messageId`, which admission dedups (D3). |
| One of our machines the contact has not accepted yet | Refused at the contact's gate (`not a contact`); the receipt is `failed` with "<contact> has not accepted this machine yet"; no relay, no retry beyond the contact's members (D3). |
| A drain, a take or a grant change while a first-contact hold is open | A drain fails the hold ("the chat moved; send again") and never releases it. A hold is released only by a grant change, and a release whose session is no longer owned here fails the task. Nothing is delivered from a machine that does not own the session: no `a2a_forward`, no `peer_deliver` (D3). |
| A replicated hold row | Cleaned and cut on read; the rows of an offline owner live until `until_ms`; other machines refresh when the holds hash changes (D4). |
| `SetAllowOnly` after `contact_grantable` goes | It installs only the empty list. A non-empty one is installed empty and logged, never skipped and never installed as given (D5). |
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
| Inbox moved, by a take or a Write | A marked inbox: `sessions.contact_inbox` travels with the row, so it runs capped (marker, no local row) until adoption re-admits it (D4). An unmarked inbox, a name-valued marker or an unreadable one never moves by a user action: `INBOX_STAYS` still refuses it. A Write take of an inbox inside the window before its marker is written is the stated residual (Risks). |
| Two sessions carry one marker | Refused with `export unavailable`, never chosen between; both stay capped until the user deletes the extra one (D4). |
| Inbox deleted and re-created | The marker goes with the deleted session; the next message goes to the home. |
| Export home removed | Listed paused ("its machine left the mesh"); no new inbox (D4). |
| Resumed turn after a take | The journal travels (`turn_steps`, capture.rs:29); an empty one caps (durability.rs:1103-1105). |
| A grant change still replicating | Bounded by replication lag. The receiving and the owning machine each check their own copy; only the owner releases a hold, and only on that grant change (D3, D4). |

### D8. UI (P4)
- **Every machine lists the same contacts,** because every RPC reads the replicated tables. Three fields are this machine's own view and not replicated state: `path` (its live connections), `machines` (its accepted roster of the contact, `contact_roster_local`, D1: never replicated) and `problem` with `problem_kind` (its own reconcile). Two machines of ours can show different `machines` for a moment while a roster reaches them.
- **Styling:** theme palette only, from the tokens in `UI/styles.css` (light `:root` :11-62, dark `.dark` :64-113, Tailwind `@theme inline` :205-269). No emojis.
- **Network › Contacts list** (`UI/components/settings/Contacts.tsx`: `ContactsTab` :134, `Contacts` :147, each row :172-228):
  - A `Paired on <machine>` line, from the record's `paired_on` (an engine id, shown by the machine's name; its short id when that machine has left the mesh).
  - `Their machines: laptop · studio`, from `Contact.machines` (Contract): this machine's accepted roster of the contact. Names come from this machine's card cache, which is in memory (D2), so after a restart a name fills in as its card is fetched, and the short endpoint id shows until then. A record with no roster yet (a pin-only record) lists its pins, so the line is never empty. A machine that this machine's accepted history lists but its current roster does not is shown struck through, labelled "left", in `text-muted-foreground`.
  - The `PathChip` (:54-62) shows this machine's path.
  - **A row with a `problem`** shows it, and `problem_kind` says what the row offers:
    - `refused`: this machine refuses the record and it will not mend itself (an unsound record, one removed on another machine, a roster conflict, "cannot verify this contact's machines", "now one of your machines"). The text is in `text-destructive`, and **Remove** is the action.
    - `waiting`: the record is held while replication catches up (P1 notes #3: a name taken until the rename lands, a pointer into a record not here yet). The text is in `text-muted-foreground`, with no action. It clears by itself, and Remove would delete a sound record.
    - `alert`: D1's inflation alert; the contact still works. The text is in `text-destructive`, with **Remove** and a link to Network › Machines, because the fix may be to unpair one of ours. Remove is not the only way out.
- **Remove and Block confirms:** "… on all your machines".
- **Removing a contact because of a rogue machine.** The Remove confirm on a contact, and the unpair dialog when Unpair is chosen for a lost or stolen machine, show: "Ask <contact> to remove you too, then pair again. Until they do, the removed machine may still reach them." The remedy needs both sides (D1).
- **Network › Machines, the unpair dialog:** adds "Your contacts are told this machine left. A contact that is offline learns it when it comes back."
- **Access** (`ContactAccess` :688): no change. The picker already lists chats on other machines (`useShareable` :667).
- **Export names:** made unique per mesh. A second `Notes` export reads `Notes-2` (names come from chat titles, `useGrant` :634-651).
- **Share dialog** (`ShareDialog` :828; `SharingItems` :808-822): no change. `useGrant` stopped sending `modes` in P3, in the commit that removes `ContactPatch.modes` and `.cap` (D5).
- **First-contact surfaces:** `ContactAllowStrip` (`UI/components/chat/ContactAllowStrip.tsx:28-31`), `ContactFirstScreen` (`UI/components/attention/PlainScreens.tsx:152-168`) and the HUD (`UI/components/hud/Hud.tsx:179`) keep their components. From P3 their data covers the whole mesh, and they refetch when another machine's hold row arrives (D4).
- **TUI, Slack, Signal, Telegram:** these have `/a2a` status only (`crates/tui/src/model.rs:7189-7193`). Nothing new.

## Contract
| Item | 191 |
|---|---|
| `Contact` (ts) | P1 (built): + `id`, + `problem: string \| null`. P3: − `cap`, in the commit that also changes the UI's `NO_PATCH` (`Contacts.tsx`), or `tsc` breaks until P4. P4: + `paired_on: string \| null` (the machine's name, looked up through `mesh::Status` as `session_move` does, `crates/rpc/src/api.rs:5433-5446`; this machine's own name from `self_name`; the first 8 characters of the id when that machine has left), + `machines: ContactMachine[]`, + `problem_kind: 'refused' \| 'waiting' \| 'alert' \| null`. `path` stays, for this machine. `path`, `machines` and `problem` are this machine's own view (D8). |
| `ContactMachine` (new ts, P4) | `{ eid: string, name: string, left: boolean }`. Its source is this machine's local state, never a replicated row (a roster carries endpoint ids and nothing else, D1): the accepted roster in `contact_roster_local` gives the `eid`s, an `eid` that is in its accepted history but not in the current roster is `left`, and `name` is the `machine` of that endpoint's card in this machine's in-memory card cache, else the short endpoint id. A record with no roster yet lists its pins (`contact_endpoints`). |
| `Export` (ts) | P1 (built): + `id`; `name` stays the card name. There is no `problem` field: an export's state is its existing `paused` text (D4). |
| `ContactPatch` | P3: − `modes` (190 kept it "until 191"), − `cap`, with the UI one-liner (`NO_PATCH`, `useGrant`) in the same commit |
| `FirstWaiting` | same shape; from P3 it is read from `contact_holds` (D4) |
| `ContactEnvelope`; `ContactOrigin.{cap,tools,holder}` | removed (the ts row for `ContactEnvelope` at `crates/rpc/src/export.rs:87` goes) |
| RPC `contact_deliver` | removed |
| RPC `a2a_forward` (mesh-internal; `Forwarded` only) | `{ caller: eid, path, body } → { status, body }`, always a `POST`. Unary: `path` is `/rpc`, `/agents/<export>/rpc` or `/agents/~return/rpc`, and the JSON-RPC method is `SendMessage`, `GetTask` or `CancelTask`. Streams, push config, the card, the notice and the proof path are never carried (D3). The owner re-checks `caller` itself: `IrohGate::Contact` (not `ContactAlpn::admit`), `admit_contact` and `contacts_ready`. RPC errors: `NOT_OWNER` (-32013) and `OWNER_UNREACHABLE` (-32010); every HTTP outcome of the owner's router is `{status, body}`. The forward ends after 10 s, below the contact's 30 s; an answer body is at most 2 MiB; `mesh::call` becomes generic over its result. It is served off the a2a mailbox. |
| RPCs `contact_list`, `contact_update`, `contact_remove`, `a2a_exports`, `a2a_export_put`, `a2a_export_remove`, `contact_first_answer`, `contact_first_waiting` (`crates/rpc/src/api.rs:2678-2710`; impls :9631-9679) | same names; they read and write the tables, so they answer for the whole mesh. A write to a merged contact writes every record of the group. Export RPCs take the export id. |
| Card | + extension `https://theuth.io/a2a/ext/members/v1` `{chains: [{chain, versions: [{version, members, sigs: [{signer, sig}]}]}]}`: for each record that lists the caller, the last 8 versions, each with up to 16 signatures (members of the previous version first, oldest first, then pinned signers). Omitted while no chain exists. The same payload is the body of a `NOTICE_PATH` push, which is now read up to 192 KiB (today the body is ignored and the route answers 204, `A/contacts/reach.rs:241-243`). |
| Contact-only route `GET ROSTER_PATH?chain=<id>&pin=<eid>&from=<version>` | The proof path: the signed copies from the pin's first signed version to the latest, in greedy hops, with every content at each hop version. At most 64 hops and 256 KiB per answer; `from` continues. It serves only the caller's own records, and `pin` must be one of that record's pins. It runs under the contact's rate and concurrency bounds (D1). |
| A2A task id | `<16 hex engine prefix>-<uuidv7>` |
| Migration | **`0059_mesh_contacts.sql`** (P1): `contacts`, `contact_endpoints`, `contact_grants`, `a2a_exports`, `contact_addrs`, `sessions.contact_inbox`, and `contact_threads` rebuilt (new table, copy with the new `id`, drop, rename; index kept). **`0060_contact_rosters.sql`** (P2): `contact_rosters`, `contact_roster_heads`, and `contact_roster_local` (local); it drops `contact_votes` if 0059 created it. **`0061`** (P3): `contact_holds`, and a partial index `sessions(contact_inbox) WHERE contact_inbox IS NOT NULL` for the marker lookup (an index adds no column, so `SESSIONS_WIRE_SCHEMA` does not move for it). `0058_image_refusals.sql` has landed on main, so 0059 to 0061 are the next free numbers **at landing**: check `crates/agent/migrations` then, and renumber together with `SESSIONS_WIRE_SCHEMA` (D2). |
| Attach | `replicate_records` (`crates/agent/src/store.rs:913`) + `contacts`, `contact_endpoints`, `contact_grants`, `a2a_exports`, `contact_threads`; + `contact_rosters` and `contact_roster_heads` in P2; + `contact_holds` in P3. `contact_roster_local` is never attached. |
| `SESSIONS_WIRE_SCHEMA` | 53 → **59**, the number of 191's migration that adds `sessions.contact_inbox`. At landing it follows that migration's number (D2). |
| `peers.yaml` | after import: HTTP peers only. The structs are unchanged, so an old file still loads. |
| Config | − `a2a.contact_cap`. `A2aConfig` is not `deny_unknown_fields` (A/config.rs:27-28), so an old `config.yaml` still loads. No new key. Constants: a roster holds at most 256 members; a card carries 8 versions per chain and 16 signatures per version; the roster signature's domain prefix is `theuth/a2a/roster/v1`; the notice body is capped at 192 KiB; pushes back off up to 10 minutes; the forward ends after 10 s; a forwarded answer body is at most 2 MiB. |

## Phases
- **The order differs from the recon's** (there: P2 routing, P3 endpoint sets). Here endpoint sets come first (P2), and routing follows (P3). With sets first, a session on any machine dials with its own key. Routing first would need an outbound relay through the pairing machine, which the standing rule rules out, and P3 would only delete it again.
- **P1 and P2 land on main together; P3 and P4 each land alone.** P1 alone is not safe: it has no owner check, yet it replicates `contact_threads` and admits every machine a contact pinned (P1's rig case 6 pins two). A `~return` can then reach a machine that does not own the session, and `full_return`, `full_intake` and `deliver_full` would run it there. P2 carries the strict owner predicate of D3 (a contact path serves a session only when its row exists and its owner is this engine, else `NOT_OWNER`), so a reply that lands on the wrong machine is refused and the client tries the next member. P1 plus P2 is the first unit that is safe on its own. P3 forwards where P2 refuses, and its last commit lifts `INBOX_STAYS` (D4). P4 is UI only.
- **Attention from every machine moves to P3.** Before P3, a hold on another machine waits; it is not released.
- **Why not three phases:** one phase would grow to about 2,000 production lines, too much for one review round.

| Phase | What | Prod | Tests | Proof |
|---|---|---|---|---|
| **P1 RECORDS** (lands with P2) | 0059 and the attach; import and pointer merge; staging from the tables with per-row quarantine; reconcile on the content hash; `contacts_ready`, including the actor-start cap; the replicated binding id, never un-revoked; contact RPCs write the tables; export ids; `sessions.contact_inbox` | 1,200–1,500 | 1,000–1,200 | Rig cases 5 and 6; a contact paired on n1 is listed on n2 |
| **P2 ROSTERS** | 0060. Per-record chains: signing and co-signing in the reconcile; `contact_rosters` keeping the hop chain plus the last 8 versions. Per-machine C-side state (`contact_roster_local`): the jump-and-replay accept rule with the equal-version switch; bootstrap from the pin through the proof path (`ROSTER_PATH`), using untrusted hints and at least two sources. Divergence detection through `contact_roster_heads`, recomputed at every refresh. The card extension and the `NOTICE_PATH` push, with its body cap and backoff. Refresh from every trusted member. Gate and dialing over the group's union, with per-endpoint hints. Merge proof and attribution. The ceremony-only deny lift. The `contacts_ready` check before a ceremony. The strict owner predicate on the contact paths: `deliver_full`, `full_intake` and `full_return` serve a session only when its row exists and its owner is this engine, else `NOT_OWNER`; no helper that reads an unknown or empty owner as local (D3, D7). | 850–1,050 | 700–900 | Rig cases 2, 4a, 4b and 7–12 |
| **P3 ROUTING** | `a2a_forward`: unary, `SendMessage`, `GetTask` and `CancelTask` only, the owner's own contact check, a 10 s deadline below the contact's 30 s, a generic `mesh::call`, off the a2a mailbox; strict owner resolution; offline refusal; forwarding of `~return` and task ids; the not-accepted-yet receipt and the transient refusal; inbox home and **adopt by marker**; `contact_holds` and the `sessions(contact_inbox)` index (0061); Attention from every machine (`A2aChanged` on a holds-hash change, rows cleaned on read); hold release only on a grant change (P1 left it), holds failed on drain; completing tasks on drain; `Contact.cap`, `ContactPatch.cap` and `.modes` and the `SetAllowOnly` grantable filter removed with the UI one-liner, the empty-only guard kept; **W-40 deleted**; **last commit: the `INBOX_STAYS` lift, for marked inboxes only** (D4) | +1,200–1,450, −600–650 | +1,250–1,550, −360 | Rig cases 1, 3 and 15–18; `grep -rn 'W-40 only' crates` lists nothing |
| **P4 UI and docs** | D8, with `Contact.paired_on`, `.machines` (this machine's local roster state) and `.problem_kind`; site docs; as-built | 450–650 | 450–550 | Vite fixture screenshots; the whole rig |
| Total | | ≈ 3,700–4,650 gross; ≈ 3,100–4,000 net | ≈ 3,400–4,200 gross | |

The P3 and P4 rows grew after review p34: adoption by marker, the hardened forward, the holds refresh, the `SetAllowOnly` guard and `problem_kind`.

## Tests and proof
**Rig.** Three scratch engines on loopback, discovery off, with a fake model server (the 186 rig `/tmp/d186/rig`, if present).
- `n1` and `n2` are paired as one mesh. `C` is a lone engine, paired as a contact with `n1` only.
- C's fake model answers `<message …>PING n` with `send_message(to=<from>, reply_to=<id>, PONG n)`.
- **To hold n2 unready without a race**, stop n1 before n2 boots. n2 cannot catch up with any peer, so `mesh::synced()` waits the full 30 s (`lapse_secs`). Probes inside that window are deterministic.

The cases:
1. **A chat on n2 answers a contact paired on n1.** n2 owns chat X, and X is shared to C from n1's UI.
   - **Force C to dial n1.** C tries a member with a live connection first, then the last one that answered (D3). So C and n1 first exchange one message about a chat Y that n1 owns: n1 is then C's live and last-answered member, and C has no connection to n2. Assert in n1's log that the `a2a_forward` to n2 happened, so the case cannot pass by C dialing n2 directly.
   - C sends `PING 1` to n1, which forwards it to n2.
   - X shows `<message from="peer:…">` and no `<contact_message>`.
   - C gets `PONG 1`.
   - The task id's prefix names n2, and C's `GetTask`, sent to n1, reaches n2.
2. **Sending from n2.** A session on n2 calls `send_message(peer:c/<export>)`. n2 dials C with n2's key, and C admits n2 because n2 is in our roster. The answer returns through `~return` to the session on n2.
3. **n2 off.** Stop n2. C's message to X reads "the chat's machine is offline" in under 1 s, and nothing is queued.
   - **Listed but dead.** Freeze n2 (`kill -STOP`) instead of stopping it, so that n1 still lists it. C's message to X reads "the chat's machine is offline" when n1's forward deadline passes (10 s), before C's own 30 s `CALL_TIMEOUT`: assert that C's error is n1's refusal and not "no answer from the contact in time". Thaw n2 and resend with the same `messageId`: one task, not two.
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
13. **A fork with a jumped machine.** As in case 12, but c2 is offline during both version-2 signings and later jumps from version 1 to version 3.
    - Assert that c2 verifies version 3 through an old member's co-signature on the winner.
    - Assert that all of C's machines agree, and that no conflict is set.
14. **A fresh machine offered an inflated path by a pin-holder.**
    - A modified n1, the pin, serves a proof path to version 20, above our latest version.
    - With c1 present at our real head, a fresh machine c4 adopts nothing. It denies with "cannot verify this contact's machines".
    - The synced n2 raises the inflation alert within two refreshes and denies nothing.
    - With no peer present, c4 follows the verified path: the limit class.
15. **A moved inbox.** n1 is the home of agent export E, shared with C, and holds its marked inbox S (`sessions.contact_inbox` set, a local row, running full) after C's first message. C dials n1, as in case 1.
    - **An unmarked inbox cannot move.** Clear S's marker (a data edit that stands in for an inbox no pass has marked yet) and try to move S to n2: refused with "This chat is a contact's inbox, so it stays on this machine.", and so is a take from n2. Put the marker back.
    - **A marked inbox moves.** Move S to n2. On n2, S has the marker and no local row, so its actor starts capped (log: `contact inbox starts capped`, an empty tool list).
    - **Capped, then full.** A turn started in S on n2 before any contact message runs capped. Then C messages E: n1 finds S by its marker, sees that n2 owns it and forwards. n2 adopts S (persona known, local row written) and S runs full: its next turn lists the agent's own tools. Assert that exactly one session carries the marker, and that the answer comes from S.
    - **A message before `contacts_ready` is refused** on n2 (boot it as in case 6): S stays capped.
    - **Ambiguity.** Copy S's marker onto a second session on n2 (a data edit). C's next message is refused with `export unavailable`, nothing runs, and both sessions stay capped.
16. **An answer to a session that moved.** A session on n1 messages C (`send_message(peer:c/<export>)`), and C's fake model holds its `PONG` behind a latch. Move the session to n2, then release the latch: C answers on `~return` at n1 (forced as in case 1). n1 resolves `(binding, contextId)` through the replicated thread to the session and forwards to its owner, n2. n2 runs `full_return`, and the answer reaches the session as `<message from="peer:c/…">`. Assert that n1 started nothing for the session.
    - **Stale view.** Move the session back to n1 before C answers a second message. n1's copy still says n2, n2 answers `NOT_OWNER`, and n1 resolves again, once, and serves it itself.
    - **Never run where it is not owned.** Make n1 and n2 each read the other as the owner (a data edit). n1 forwards, n2 answers `NOT_OWNER`, n1 resolves once more and the second `NOT_OWNER` goes back to C. C tries its next member and is refused the same way. Assert that neither machine started an actor for the session.
17. **A hold answered on another machine.** n2 owns chat X, shared to C with first contact `ask`. C's first message to X is held on n2: n2 writes a `contact_holds` row, and C's task is `auth_required`.
    - **Attention refreshes on n1.** n1 shows the strip for X and the Attention item within one reconcile tick, with no other event: a fresh `contact_first_waiting` on n1 lists the hold, and an `A2aChanged` reached its UI.
    - **Allow on n1** is a grant write. It replicates, and n2's reconcile releases the hold: X shows the message, C's task completes with its receipt, the row is deleted and the strip goes on both machines. Nothing was delivered from n1.
    - **Drain.** Hold a new message, then move X to n1 before anyone answers. On n2 the hold fails with "the chat moved; send again", its row goes, and n1's strip clears. C's resend reaches n1 and is held there afresh, since the answer is still open.
    - **Release after a move.** Hold a message, freeze n2 (`kill -STOP`), take X from n1 with `Write`, thaw n2, then answer Allow. The release finds that n2 no longer owns X and fails the task. Nothing is delivered from n2.
18. **n2 before C has accepted it.** n2 is a machine of our mesh that C's roster of us does not list yet.
    - Stop C. Add n2 to our mesh: n1 signs the version that lists it, and its push to C fails. Freeze n1 (`kill -STOP`), then start C: C's boot fetch from n1 gets no answer, so C's roster of us still lacks n2.
    - A session on n2 calls `send_message(peer:c/<export>)`. C's gate refuses n2 (C's log: `not a contact`), and the receipt on n2 is `failed` with "<C> has not accepted this machine yet". Nothing is relayed through n1, and nothing is retried.
    - Thaw n1 (`kill -CONT`). Its push retry reaches C within the backoff, and n2's next send is accepted.

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
  - **`a2a_forward`:**
    - without the `Forwarded` marker it is refused;
    - a stream method (`SendStreamingMessage`, `SubscribeToTask`), a push-config method, any other method and any path outside `/rpc`, `/agents/<export>/rpc` and `/agents/~return/rpc` are refused at the forwarder, and the card, the notice and the proof path are answered on the receiving machine and never carried;
    - the owner refuses a caller that its own copy blocks, and one that is only `IrohGate::Pair` (a stranger inside an open Add contact window, whom `ContactAlpn::admit` would let through);
    - a caller the owner does not list yet (an unlisted sender) gets a 401 from the owner and the transient refusal from the receiving machine, and nothing runs;
    - an owner that is not `contacts_ready` answers `OWNER_UNREACHABLE`, which the contact reads as an offline owner;
    - the forward's deadline is below the contact's `CALL_TIMEOUT`: with a listed owner that never answers, the contact hears `OwnerOffline` first, and its retry with the same `messageId` makes one task;
    - an answer body over 2 MiB fails the forward and is never cut;
    - the handler answers while the a2a actor's mailbox is held (it never asks the actor).
  - **Owner resolution:**
    - an owner that is not connected is refused at once;
    - an unknown row, an unreadable row and an empty owner on a mesh engine all refuse, and none is read as local; an inbox that was drained is still in `NetworkState.inboxes` and is still refused;
    - the first `NOT_OWNER` resolves again, once; the **second `NOT_OWNER` goes back to the contact**, with no third resolve;
    - a call that arrived as a forward and whose owner is elsewhere answers `NOT_OWNER` and is **not forwarded again** (no second hop);
    - `~return` is forwarded by its thread.
  - **Outbound:** a machine the contact has not accepted gets the `failed` receipt "<contact> has not accepted this machine yet", with no relay and no retry; a `NOT_OWNER` answer moves the client to its next member.
  - **Drain and holds:**
    - on drain, waiting full tasks complete as receipts;
    - on drain, a hold fails with "the chat moved; send again" and its `contact_holds` row is deleted; it is never started and never forwarded (no `a2a_forward`, no `peer_deliver`);
    - an answer from n1 releases a hold on n2 only through a grant change; a release whose session is no longer owned here fails the task; a drain or a take with no grant change releases nothing;
    - a `contact_holds` row written by another machine makes `A2aChanged` fire here and the Attention item appear; its `machine` and `preview` are cleaned and cut on read; a row past `until_ms` is dropped, and an offline owner's row stays until then.
  - **Inbox:**
    - an export whose home was removed opens no inbox and lists `paused` (S5);
    - **adoption:** a marked inbox owned here with no local row is adopted (the row is written, the persona re-read, the inbox runs full); one marked session owned elsewhere is `NOT_OWNER`; **two marked sessions refuse** and both stay capped; a name-valued marker is never adopted and still caps; with no match, the home mints and any other machine answers `NOT_OWNER`;
    - **a moved inbox's message is capped, then full:** before the adoption write the inbox runs capped (marker, no row), after it full; a message while `contacts_ready` is false is refused;
    - **`INBOX_STAYS` after the lift:** an **unmarked inbox still refuses to move**, and so do a name-valued marker and an unreadable marker, in `move_to` and in `answer_release` (the `Msg::Refused` path); a marked inbox moves;
    - the home that finds an unmarked pre-191 session owned elsewhere unmarks its own row and mints a new marked inbox.
  - **Guard and removals:**
    - `SetAllowOnly` installs only the empty list: a non-empty one is installed empty and logged, never skipped, so `contact_scoped` stays true and no tool is grantable;
    - `ContactPatch` has no `cap` or `modes`, and the UI builds with the `NO_PATCH` change (`tsc`, `Contacts.test.ts`);
    - `grep -rn 'W-40 only' crates` lists nothing.

The S1–S10 tests stay.

## The recon's "Not verified" list, resolved
- **The journal after a take:** it travels. `turn_steps` is a session-log table (capture.rs:29), and resume reads it on the new owner (`crates/agent/src/supervisor/resume.rs:160-170`, :195-216). S7 holds.
- **`contact_card`'s pause check for a remote chat:** it already answers from the replicated row (N:3004-3024; `crates/agent/src/supervisor.rs:1206-1219`; A/contacts/offers.rs:211-236).
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
- **An inbox with no marker (D4).** The marker is best-effort: written at every admission and by every pass after the latch, never before it (up to 150 s after boot), and not at all when the export's id is unknown or the write fails. A taker cannot tell such an inbox from a plain chat, so a Write take from an unreachable owner inside that window would run it with no limits. A user's move or take never moves it (`INBOX_STAYS` still refuses it, D4). This window is the stated residual.
- **A merge ends conversations, once.** The next message starts fresh.
- **Write contention.** Replica apply takes `BEGIN IMMEDIATE` on the single `sessions.sqlite` write connection (R/apply.rs:454). The rows are tiny and the writes rare. Watch it in the P1 rig.
