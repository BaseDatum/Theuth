# 190 — builder notes (engine)

Decisions taken where `190-drop-cautious.md` or the recon left room, or where the contract could not hold as written. Each keeps D1–D10.

## Where the contract bends
1. **`CONTACT_GRANTABLE` and `contact_grantable()` stay, private to `tools::registry`, marked W-40.** The contract lists them as removed, but D4 keeps W-40 byte for byte, and a W-40 turn's allowlist (`origin.tools`, sent by another engine of the mesh) is admitted through them: `SetAllowOnly` filters it and `grant_key` admits only grantable names. Removing them would either widen what a paired engine can install in a capped turn or empty the W-40 allowlist. What goes: `tools::ContactGrantable`, `GrantableTool` (`GrantableTool.ts`), the `a2a_grantable_tools` RPC and every allowlist check on the owner's own edits (there are no edits any more).
2. **Doc comments of TS-exported types are unchanged** except where a field is removed: ts-rs 12 emits doc comments into the `.ts` files, and the bindings diff must equal the contract table. The now-stale wording on `ContactPatch.cap`/`.modes`, `Contact.cap`/`.full_capable`, `FirstWaiting.queued` and `ExportTarget` stays; a plain `//` note beside each says what 190 does.

## W-40 (D4) after D9
3. **A W-40 turn's allowlist is `config::default_contact_tools()` (`search_web`, `read_url`).** `ExportEntry.tools` is read and dropped (D9), so the holder stamps the default into `ContactOrigin.tools`. A 186 file's custom per-export list no longer reaches a W-40 turn.
4. **A W-40 turn's cap is still `PeerEntry.cap` or `a2a.contact_cap`.** `PeerEntry.cap` is read (never written, D9), so it holds until the next RPC write of `peers.yaml` drops it; then the default `read_only` applies. `contact_update`'s `cap` is accepted and ignored (no validation either: the field is inert).
5. **The W-40 queue keeps working without its visibility rows.** A W-40 message still queues on the holder behind the chat's running A2A task (generic slot logic); `QUEUED_NOTE`, `ContactsQueued`, `note_queued_exchange` and the sender's import of a queued note are gone per D3 and the contract ("a 190 engine never sets them"). `FirstWaiting.queued`/`.position` are always `false`/`0` here; `HeldContact.position` is gone.
6. **`network_contact_frame` keeps one fail-closed fallback**: a contact task that reaches local intake with a scope (its chat's remote owner vanished between the two ownership checks) still opens a capped `<contact_message>` turn rather than an uncapped scoped one. `ContactFrame::Inbox` and `contact_inbox()` are gone.
7. `PeerMeta.cap/.tools`, the start-of-turn install, `ContactFrame::Chat`, `contact_message()` and the journal source `contact` stay: W-40's capped turn needs them (recon §2.2 listed the start-of-turn install as cautious-only; W-40 uses it).

## Fail-closed (D6)
8. `inbox_limits` stays (it only ever holds the fail-closed pair now) so the S1–S10 tests run unchanged. `apply_contact_limits` keeps its first half: an inbox whose export is no longer granted is unmarked and capped. `ContactLimits` shrinks to `{contact, full}`; `full` = the agent exports the contact holds.
9. `PeerRoute.full`/`RemotePeer.full` keep their names and now carry every export of a contact (none for an HTTP peer): F:199's "still shared" check reads them.

## Smaller calls
10. **D10 notice** logs one line per grant of every non-blocked contact at the first good reload with no marker, then writes `a2a/.190-full-notice` (creating `a2a/` if needed). Test 1's "the last variant logs" holds because every variant logs: the notice is per grant, as D10 says.
11. **`CONTACT_RULE`** loses its last clause ("your final text is your answer …"): it is true in neither a fail-closed inbox (no answer path) nor a full share. A W-40 turn's `<contact_message>` wrapper already says the reply goes back to the contact.
12. **RPC `cancel`** forwards the plain two-parameter call whatever `reason` says, so a 186 owner engine of the mesh takes a forwarded interject as a Stop instead of refusing it (D5 mesh-wide). The TUI's interject and talk's barge-in send the default reason.
13. **Re-pointing (D8)** writes no `modes`; the grant epoch rotates through `Staged::same_grant` (the export's session or agent is part of it), so held first messages fail with `HOLD_CHANGED`. `unchanged_exports` drops the tools and mode terms (both gone) and keeps the cap term (W-40 still reads the cap).
14. `contact_update` refuses `cautious` before touching the file; `full` and a `modes` entry for an export not granted are ignored. The version gate fires on any newly granted export when the contact's card lacks `EXT_FULL` ("update <machine> to share").
