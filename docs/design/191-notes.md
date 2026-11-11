# 191 — as-built notes

## P1 RECORDS

Built on `feat/191-p1` from main 10f651e1 (design at review round 2, f568f2bc). Paths are at the P1 head.

### What landed
- **Migration `crates/agent/migrations/0059_mesh_contacts.sql`**: `contacts`, `contact_endpoints`, `contact_votes`, `contact_grants`, `a2a_exports` (replicated: one key column, `hlc TEXT`, no NOT NULL/UNIQUE/CHECK/FK), `contact_addrs` (local), `sessions.contact_inbox`, and `contact_threads` rebuilt with `id` = `<binding>|<context_id>` (rows copied, index kept).
- **Attach**: `Store::replicate_records` (`crates/agent/src/store.rs:911-922`) attaches `contacts::RECORD_TABLES` beside `projects` and `cost_owners`.
- **`SESSIONS_WIRE_SCHEMA` = 59** (`crates/agent/src/store.rs:674`); the snapshot test (`sessions_wire_schema_tracks_the_replicated_columns`) takes `contact_inbox`.
- **Record store**: `crates/agent/src/store/contacts.rs` (`Records`, `content_hash`, column-only `RecordWrite`s, `contact_addrs`, the inbox marker and its backfill); agent messages `ContactRecords`, `WriteContactRecords`, `BackfillContactInboxes`, `SetContactAddrs` (`crates/agent/src/communication/network.rs`).
- **Staging, merge, import**: pure functions in `crates/a2a/src/contacts/records.rs` (`stage_records`, `reconcile_writes`, `import`, `analyze`).
- **Reconcile**: `Ctx::reload_as` (`crates/a2a/src/lib.rs`) is the one pass; `Ctx::tick` runs it on the 2 s tick when the file stamp, the records' content hash, or the first post-latch pass asks; every contact RPC runs it right after its write (`Ctx::commit_records` → `reconcile_now`).
- **Replicated binding id**: `bind_peer_in(…, Some(id))` (`crates/agent/src/store/communication.rs:904-995`) takes the record's id, renames a live row in place, refuses a revoked one (`BINDING_REVOKED`); `peer_bindings.endpoint` = `contact:<record id>` (`contact_record_endpoint`).
- **`contacts_ready`**: `agent::communication::network::CONTACTS_READY` (a `LazyLock<Arc<AtomicBool>>`, false at start). The a2a plugin holds the same `Arc` (`Ctx.ready`) and sets it at the end of the first successful pass after `mesh::synced()`; the agent only reads its `Agents.contacts_ready` copy, never asks the a2a actor.
- **Contact RPCs** (`crates/a2a/src/contacts.rs`): `contact_list`, `contact_update`, `contact_remove`, `a2a_exports`, `a2a_export_put`, `a2a_export_remove`, `contact_first_answer`, and the ceremony's pin write the tables; a write to a merged contact writes every record of its group that has a row (else the root).

### Deviations from the doc
1. **When the import runs** (D2 "at the first reconcile after `contacts_ready`"): inside the first post-latch reconcile, before it stages (`crates/a2a/src/lib.rs`, `reload_as`, the `imported` block), and again whenever `peers.yaml` holds contact or export entries (a crash before the rewrite, a hand edit). Running it after the flip would let that same first pass revoke the bindings the import keeps (`revoke_peer_bindings_except` sees no record for them). The import is still post-latch and pre-flip, so it cannot admit anything early.
2. **Removed records** do not go into the deny set but into `Staged.removed` (`crates/a2a/src/config.rs`): the gate never admits them as a contact, but they may pair again inside an open window (D2 "a re-pair therefore works"). Quarantined and blocked records' endpoints are in `deny`, refused before anything else, a ceremony included.
3. **Quarantine is per merge group**, not per record: any failed check on a group (its root's alias or binding, an anchor that is malformed, one of our machines, or anchored by another unmerged group, a grant naming an export row that does not exist) quarantines the group and denies all its anchors. A chain that loops or runs past 16 hops quarantines every record on it and the root it reaches.
4. **Alias collision**: until the reconcile's rename (`<alias>-2`) lands, the newer record is quarantined ("its name is taken by another contact (renaming)"); the older keeps the alias. Fail closed for the seconds it takes.
5. **An answer without a grant** (a `first_contact` entry for an export the contact is not granted) is a grant row with `granted` NULL: it abstains on the grant and takes part in the strictest answer.
6. **Exports this machine cannot publish** (the chat's session not replicated here yet, or not native) are left out of this machine's routes with their grants; no contact is quarantined for it. `a2a_exports` lists such a chat export paused `"chat not on this machine"`; a deleted chat's export is tombstoned only by its home (`exports()`), since lag and deletion look alike elsewhere.
7. **One principal per exported chat** stays (§6.3): staging skips the later contact's grant (key order, the same on every machine); `contact_update` refuses it up front as before.
8. **Inbox marker** is set on every admission by `handle_inbox` (`ContactInbox.export_id`, from the export's id): in P1 the admitting machine is the inbox's home. The import backfills local inboxes of sessions this machine owns.
9. **Before `contacts_ready` the gate still offers a ceremony** to a stranger inside an open window (both humans must accept); it never admits a contact (`iroh_gate(…, ready)`).
10. **`Ctx::edit` is gone**: `peers.yaml` holds HTTP peers only and is hand-edited. An HTTP peer named like a contact, or a file export named like a mesh export, is a staging error; the reconcile then runs with the last good HTTP peers (`reconcile_now`, `tick`), so a broken file never stops a block from applying.
11. Not in P1, as the doc phases them: `paired_on`/`machines` on `Contact` (the column is written), `ContactPatch.modes`/`cap` and `Contact.cap` (W-40, P3), release of a hold on a replicated Allow (P3), votes (P2).

### Fail-closed, path by path
| Path | How it stays closed |
|---|---|
| Boot before `contacts_ready` | `reload_as` stages nothing before `mesh::synced()`: `Live` stays empty (no route, the gate admits no contact); `iroh_gate` also refuses a contact while `ready` is false; `network_send` to an unrouted peer reads "waiting for your other machines". |
| Gap between the latch and the end of the first reconcile | `CONTACTS_READY` is set only after `CommitPeerConfig` returned (revokes, `drop_unrouted`, inbox caps) and the swap and closes ran. `handle_inbox` refuses while not ready. |
| An actor start while not ready | `reinstall_contact_limits` caps every inbox (`INBOX_FAIL_CLOSED`, empty allowlist) and leaves its row marked full; `refresh_full_inboxes` lifts nothing while not ready; the flip tells `InboxPersonasChanged`, which lifts the caps still due. Logged `191: contact inbox starts capped`. |
| Replication lag | A record is admitted only from rows this machine holds; a grant naming a missing export row quarantines the group; an export whose chat is not here is not published; a pointer into a record this machine lacks quarantines the chain. |
| A quarantined row | Its group's anchors go into `deny`, which the gate checks first; the rest stage. |
| A merge or a pointer chain | Two unmerged groups sharing an anchor are both quarantined until the pointers land; merged-away records are never admitted on their own; a loop or a chain past 16 quarantines the group; a block on any record of a group blocks it. Merged-away bindings fall out of the route set and are revoked (`drop_unrouted`), their inboxes capped. |
| An import | Post-latch only; never revives a removed record, never writes `blocked`/`removed_ms`/`binding` on an existing one, adds only missing grants; a contact the mesh removed is skipped and logged. |
| A removed or blocked record | Blocked: no route, anchors denied, held tasks released as failed, connections closed. Removed: tombstoned on every record of its group, never cleared; its endpoints refused as a contact until a newer ceremony pins them. Both: the binding falls out of the routes and is revoked. |
| The replicated binding id | An id revoked here stays revoked: `bind_peer_in` refuses it and staging quarantines its record ("its binding was revoked on this machine"). Unblock and re-pair write a fresh binding. |
| A moved inbox (Write take) | `sessions.contact_inbox` travels with the row; with no local `a2a_contexts` row the actor start caps it (`reinstall_contact_limits`). |
| A resumed turn | Unchanged (S7); every actor start goes through `reinstall_contact_limits` above. |
| Accepted cost | A block or removal made elsewhere while every other machine was off is missed until this machine syncs with one (option b). |

### Proof
Unit tests: `crates/a2a/src/contacts/records.rs` (merge, export ids, quarantine, removal, alias convergence, import), `crates/agent/src/store/contacts.rs` (content hash, marker backfill and its session-log write), `crates/agent/src/store/communication.rs` (`a_replicated_binding_id_renames_in_place_and_is_never_unrevoked`), `crates/agent/src/communication/network.rs` (`an_inbox_starting_before_contacts_ready_runs_capped_until_the_flip`, `an_inbox_taken_by_a_write_runs_capped_from_its_marker`), in-process e2e in `crates/a2a/src/contacts/e2e_tests.rs` (listed on another machine, a block at the next tick, refused until the latch and the first reconcile). Multi-engine rig: `/tmp/d191/p1/evidence.txt`.
