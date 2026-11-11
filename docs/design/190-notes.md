# 190 — builder notes

## Engine

From `feat/190-engine`.

Decisions taken where `190-drop-cautious.md` or the recon left room, or where the contract could not hold as written. Each keeps D1–D10.

### Where the contract bends
1. **`CONTACT_GRANTABLE` and `contact_grantable()` stay, private to `tools::registry`, marked W-40.** The contract lists them as removed, but D4 keeps W-40 byte for byte, and a W-40 turn's allowlist (`origin.tools`, sent by another engine of the mesh) is admitted through them: `SetAllowOnly` filters it and `grant_key` admits only grantable names. Removing them would either widen what a paired engine can install in a capped turn or empty the W-40 allowlist. What goes: `tools::ContactGrantable`, `GrantableTool` (`GrantableTool.ts`), the `a2a_grantable_tools` RPC and every allowlist check on the owner's own edits (there are no edits any more).
2. **Doc comments of TS-exported types** stayed unchanged in 41377b9a except where a field went (ts-rs 12 emits them into the `.ts` files). Review 1 fixed the ones that had become false, so these `.ts` files now differ from the base in doc comments only: `CancelReason.ts`, `ShareMode.ts`, `ExportTarget.ts`, `OfferKind.ts`, `Contact.ts` (`full_capable`). `ContactPatch.ts` stays as it was.

### W-40 (D4) after D9
3. **W-40 changed its tools and cap source, on purpose, and it stays changed.** Design 191 deletes W-40 within days, so 190 does not restore either:
   - A W-40 turn's allowlist is always `config::default_contact_tools()` (`search_web`, `read_url`): `ExportEntry.tools` is read and dropped (D9). An export whose 186 file said `tools: []` (nothing) now gets those two; a custom list is gone too.
   - A W-40 turn's cap is `PeerEntry.cap` while the file still has it; the first RPC write of `peers.yaml` drops the key (D9), and the cap falls back to `a2a.contact_cap`, by default `read_only`. `contact_update`'s `cap` is accepted and ignored.
5. **The W-40 queue keeps working without its visibility rows.** A W-40 message still queues on the holder behind the chat's running A2A task (generic slot logic); `QUEUED_NOTE`, `ContactsQueued`, `note_queued_exchange` and the sender's import of a queued note are gone per D3 and the contract ("a 190 engine never sets them"). `FirstWaiting.queued`/`.position` are always `false`/`0` here; `HeldContact.position` is gone.
6. **`network_contact_frame` keeps one fail-closed fallback**: a contact task that reaches local intake with a scope (its chat's remote owner vanished between the two ownership checks) still opens a capped `<contact_message>` turn rather than an uncapped scoped one. `ContactFrame::Inbox` and `contact_inbox()` are gone.
7. `PeerMeta.cap/.tools`, the start-of-turn install, `ContactFrame::Chat`, `contact_message()` and the journal source `contact` stay: W-40's capped turn needs them (recon §2.2 listed the start-of-turn install as cautious-only; W-40 uses it).

### Fail-closed (D6)
8. `inbox_limits` stays (it only ever holds the fail-closed pair now) so the S1–S10 tests run unchanged. `apply_contact_limits` keeps its first half: an inbox whose export is no longer granted is unmarked and capped. `ContactLimits` shrinks to `{contact, full}`; `full` = the agent exports the contact holds.
9. `PeerRoute.full`/`RemotePeer.full` keep their names and now carry every export of a contact (none for an HTTP peer): F:199's "still shared" check reads them.
9b. Review 1: a contact message whose chat row cannot be read (`Store::session` errs) is refused (`export unavailable: the chat could not be read`); `Agents::session_owner` keeps the error that `remote_owner` swallows, and only `start_or_queue` uses it. The second check in `start_task` already falls back to the capped `<contact_message>` turn (6).

### Smaller calls
10. **D10 notice** logs one line per grant of every non-blocked contact at the first good reload with no marker, then writes `a2a/.190-full-notice` (creating `a2a/` if needed). A marker it cannot write is a warning; the notice is tried once per process, so an unwritable `a2a/` neither fails a reload nor repeats the lines on every reload (it repeats once per restart). Test 1's "the last variant logs" holds because every variant logs: the notice is per grant, as D10 says.
11. **`CONTACT_RULE`** loses its last clause ("your final text is your answer …"): it is true in neither a fail-closed inbox (no answer path) nor a full share. A W-40 turn's `<contact_message>` wrapper already says the reply goes back to the contact.
12. **RPC `cancel`** forwards the plain two-parameter call whatever `reason` says, so a 186 owner engine of the mesh takes a forwarded interject as a Stop instead of refusing it (D5 mesh-wide). The TUI's interject and talk's barge-in send the default reason.
13. **Re-pointing (D8)** writes no `modes`; the grant epoch rotates through `Staged::same_grant` (the export's session or agent is part of it), so held first messages fail with `HOLD_CHANGED`. `unchanged_exports` drops the tools and mode terms (both gone) and keeps the cap term (W-40 still reads the cap).
14. `contact_update` refuses `cautious` before touching the file; `full` and a `modes` entry for an export not granted are ignored. Review 1 (Sky, 14:59: no compatibility for older theuth versions): the D9 version gate is gone; a share is never refused for the contact's version, its card unread or offline included. `Contact.full_capable` stays, display only.
15. `HeldContact.position` is gone (always 0); `inboxes` is a set (the "first row owes the `<contact_inbox/>` notice" flag went with the wrapper). `Agents::turn_of` (only the D8 guard read it) is gone.

### Tests
Deleted, each cautious-only:
- `config::ungrantable_tools_are_stripped_when_the_file_loads`: an export's allowlist bound only a cautious turn; `ExportEntry.tools` is read and dropped.
- `network::a_cap_or_allowlist_change_reaches_a_live_inbox_at_once`: pushed the contact cap and export allowlist into a cautious inbox.
- `network::a_queued_contact_message_is_visible_until_its_turn_starts`: the cautious queue rows and `QUEUED_NOTE`.
- `e2e::a_put_without_tools_keeps_the_exports_allowlist`: the per-export tools editor.
- The `ContactGrantable` half of `registry::an_allowlist_from_a_peer_holds_only_grantable_tools` (the editor's RPC) and the `contact_inbox()` half of `loop_::contact_message_wraps_and_neutralises_the_body`.

Rewritten: `config::modes_default_to_cautious…` → `a_186_grant_loads_full_in_every_variant_and_190_writes_no_new_key` (test 1); the D8 test → `supervisor::an_interject_cancels_a_contact_turn_like_any_turn` (test 3); `network::first_contact_holds_until_the_owner_answers`, `a_contact_never_runs_out_of_contexts`, `a_contact_inbox_is_one_root_reused_per_contact_and_export`, `an_agent_export_inbox_runs_the_exported_agents_persona_model_and_ceiling`, and three a2a e2e tests (no mode in a grant; the cap change of the held-message test is now a re-point; the full-chat test covers D2, D8, D9, unsharing). `a_full_contact_message_is_a_peer_message…` loses its "a cautious contact may not name a context" assertion.

S1–S10: none deleted. `an_inbox_is_capped_and_default_deny_at_every_actor_start` now expects the fail-closed pair right after admission too (theuth with no ceiling known runs capped: stricter than before); `ContactLimits` literals in the `full_inbox_tests` lose `cap`/`exports`.

New: `network::a_contact_message_is_full_and_answerable_after_its_turn_is_cancelled` (tests 2, 3), `full_inbox_tests::an_agent_export_runs_full_whatever_the_origin_says` (test 4), `network::a_cautious_tagged_card_gets_the_183_shape_and_its_final_text_reply` (test 8); tests 5 and 6 live in `e2e::a_chat_shared_in_full_talks_both_ways_like_a_mesh_peer` and `a_grant_change_keeps_a_waiting_first_message_and_always_releases_it`; test 7 is the W-40 supervisor tests, unchanged.

## UI

From `feat/190-ui`.

Decisions the UI brief left open, each the simpler option that keeps 190-drop-cautious D1–D10.

1. **"Messaging… sheet" = the chat's share dialog.** The Messaging… sheet (`MessagingSheet.tsx`) carries no share or mode UI since 188; the chat's Full/Cautious picker lived in `ShareDialog` (chat ⋯ › Share with contact…). That dialog is what changed: one **Share** per contact, **Shared · Stop** once shared.
2. **Sharing is one action.** In Network › Contacts › Access, picking an agent or chat in the Share… list grants it at once (the old second step existed only to choose a mode). Share is always on for a paired contact (review round 2: the engine drops its `full_capable` gate; the flag is also false for an unread card or an offline contact, so "update <machine> to share" misled). Both the picker and the share dialog show `SHARE_RULE` at the moment of sharing (review round 1: the consent line says the agent works with its normal tools and your approvals).
3. **`modes` on a grant.** `useGrant` sends `modes: { <new export>: 'full' }` only when the export is new to that contact; ungrant, rename, block and first-contact answers send `modes: null` (D2). The engine keeps `ContactPatch.modes` until 191.
4. **Cancel reasons unchanged.** Interject and flush still send `'input'`, Stop `'stop'`; a 190 engine ignores the reason (D5). Sky, 2026-11-11 14:59: no compatibility with older theuth builds, so no 186-engine workaround here.
5. **Composer state.** `contactTurn()` and the `SessionSlice.contactNote` field it alone read are deleted; `interrupt()` loses its `contact` parameter; `StopButton` loses its `label` prop. `turnBy` (the "asked during" tag) is unchanged.
6. **Queued rows.** `useFirstWaiting()` always drops `queued === true` rows, so no surface (strip, Contacts, Messaging, Attention, HUD) ever shows one; the strip lost `QueuedRow`.
7. **Peer cards.** A contact message (`contact_mode: 'full'`) keeps the mesh-peer card; its `Contact · full` tag is gone (the sender line already says "Contact · on <machine> · via <export>"). An old `contact_mode: 'cautious'` row, and an old `<contact_message>` / `<contact_inbox/>` row, keep today's rendering: the words in the plain outside card with the wrapper's facts in Details, never a raw tag. Old wrapper cards keep their cap words (history).
8. **Communication › A2A exports** lost the mode badge; a contact's export reads one line ("its agent and this chat's agent talk freely, like mesh peers").
9. **Bindings** (gitignored): generated from this branch, then hand-edited to the 190 contract (`GrantableTool.ts` gone; `Contact.modes`, `FirstWaiting.mode`, `A2aExportStatus.mode`, `Export.tools` gone; `a2a_grantable_tools` out of `methods.json`; `ContactPatch.modes` kept). The orchestrator regenerates them from the engine branch.
