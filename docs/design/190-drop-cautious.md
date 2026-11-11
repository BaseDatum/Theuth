# 190 — DROP-CAUTIOUS: every contact share is full

Status: contract. Recon: `docs/design/190-recon.md`, written at main 8f879fbd. Its section numbers (§) and path:line are cited below; its title still says 189, which was renumbered.

## Sky's words (2026-11-11)
- **13:47:** "harness A's response is linked to one turn So if anything goes wrong in that turn it just can't respond … the whole point of the contacts flow is intersession communication once a contact is trusted, and a session is shared … harness B can respond at any time, or send messages to the sessions that contacted it at will, whenever it wants, no stupid bound to a turn logic."
- **~13:50:** "original shares pre the last change were defaulted to cautious, but it still seems like it's bound to a single turn which doesn't make sense."
- **~13:51:** "the full share mode works correctly, I just don't understand why I have this UI, after so and so's turn or interrupt so and so's turn. that part seems over complicated, user messages work exactly as normal."
- **13:53, ruling:** "Drop cautious." Every share is full and the mode choice goes away.
- **14:23, on a contact's chat that lives on another engine of the mesh (W-40):** "shouldn't contact links just be replicated across the mesh and durable. this seems overcomplicated right now. A contact is a contact for every node in the mesh, not just a single node." That is design 191. W-40 stays exactly as it is until 191.

## Decisions
- **D1. One mode.**
  - Every grant (contact × export) is FULL, as defined by 186 D4–D7: a contact's message arrives like a mesh peer's, and the agent answers with `send_message(reply_to)` at any time, any number of times. A chat uses its own tools and approvals; an agent inbox uses the agent's own `tools:` ceiling.
  - `ShareMode` stays as a wire and stored type. The engine only writes and offers `full`; `cautious` is still read from old rows and old cards.
- **D2. The choice goes.** See the contract below.
  - `contact_update` takes no mode. `ContactPatch.modes` stays, deprecated, for one release: `full` is accepted and ignored, and `cautious` is refused with "cautious shares are gone: every share is full".
  - The 190 UI keeps sending `modes: {<newly granted export>: "full"}` so a 186 engine in a mixed mesh still accepts the share (§5.5).
- **D3. Receiver: the full path for every local export.**
  - The cautious receiver branch is deleted: one A2A task and one capped turn per message; the final text as the answer; `ContactFrame` for local exports; `PeerMeta.cap/tools`; the `<contact_message>`/`<contact_inbox>` wrappers for local exports; the start-of-turn cautious install (`L:1285-1298`); the cautious queue rows (`QUEUED_NOTE`, `ContactsQueued`, `note_queued_exchange`); the `CONTACT_GRANTABLE` filter.
  - That is §2.2 "Delete" and §6.1 "Rust internals", minus D4.
- **D4. W-40 unchanged until 191.**
  - A contact's message to a chat that lives on another engine of the holder's mesh keeps today's forwarded, capped, final-text path, byte for byte: `ContactEnvelope`, `contact_forward`/`contact_deliver`, the owner's clamp at `N:4013-4019`, `network_contact_frame` for W-40, `contact_turn_finished`, `resumed_contact_envelope`, `ContactOrigin.{cap,tools,mode}` (§1.7).
  - Keep exactly the cautious pieces W-40 needs (`ContactFrame::Chat` and its wrapper included), each marked `// W-40 only; removed by design 191 (contacts are mesh-wide)`. W-40's tests keep passing unchanged.
- **D5. No owner-turn guard.**
  - 186 D8 is removed. Interject, flush and talk's barge-in cancel a turn that took a contact's message exactly like any other turn. `CancelReason` and `Cancel.reason` are accepted and ignored; the RPC signature stays.
  - Delete `turn_took_contact`, the `contact_turn` guard and their tests. A W-40 capped turn the owner interrupts fails its task the way an explicit stop did in 186 (temporary, until 191).
- **D6. The fail-closed cap stays.**
  - §2.1 S1–S10 and §2.2 items 1–7 stay as they are. `inbox_full` keeps its column and default 0, and now means "this inbox may run uncapped under the agent's ceiling".
  - `apply_contact_limits` keeps only its first half (`N:3339-3363`); the cautious allowlist push (`:3364-3380`) goes with `ContactLimits.cap/.exports`. `inbox_limits` may shrink to the fail-closed install, but the call stays.
  - The S1–S10 tests are NOT deleted. Review rounds of 186 found three fail-open bugs in exactly this code; see `warnings/theuth-contact-inbox-limits-must-persist-and-fail-closed-2026-11-10`.
- **D7. Prompts and descriptions.**
  - `CONTACT_RULE` ("your final text is your answer …") is reworded or scoped to W-40 turns.
  - The `send_message` description sentence naming tag `full` (`peers.rs:437`) becomes: a contact talks like a mesh peer; answer its `<message>` with `send_message(to=its from, reply_to=its id)`, as often as you like.
  - The `discover_peers` mode tag goes.
- **D8. Re-pointing keeps full.** Re-pointing an export to another chat or agent no longer drops it to cautious (`A/contacts.rs:1903-1913`). The grant epoch still rotates, so held first messages still fail with `HOLD_CHANGED`.
- **D9. Compatibility (§5).**
  - **Files:** `PeerEntry.modes`, `ExportEntry.tools` and `PeerEntry.cap` become `#[serde(default, skip_serializing)]` read-and-drop. 190 never writes `modes` and adds NO new key to `peers.yaml`, because a 186 build refuses unknown keys (`deny_unknown_fields`).
  - **Cards:** keep emitting the card skill tag `full`, `EXT_FULL` and `card_full`; the JWS vector `A/contacts.rs:2402-2406` must not change.
  - **Senders:** keep the card-tag shape choice (`offers.rs:161-166`, `F:226-238`) and the final-text import (`apply_remote_task`/`import_reply`). An older receiver with a cautious grant, and every generic A2A peer, still work.
  - **Version gate:** a newly granted export to a contact without `full_capable` is refused with "update <machine> to share" (redefine 186 D10's "upgraded" as "newly granted").
  - **Store:** no migration, and no column dropped (§5.6).
- **D10. Owner notice: none in the UI.** At the first load after the upgrade, log one INFO line per grant: `a2a: share <export> to <contact> is full (cautious mode removed)`. Use a marker file `a2a/.190-full-notice` beside `peers.yaml`, never a `peers.yaml` key, so it logs once.

## Contract (ts-rs and RPC)
| Item | 190 |
|---|---|
| `Contact.modes` (`Contact.ts`) | removed |
| `FirstWaiting.mode` (`FirstWaiting.ts`) | removed |
| `FirstWaiting.queued`, `.position` | stay optional; a 190 engine never sets them (no queued rows). The UI keeps hiding a row with `queued === true` on every surface, because a 186 engine in the mesh still sends them |
| `A2aExportStatus.mode` | removed |
| `Export.tools` (RPC rows, `Export.ts`) | removed (files still parse `ExportEntry.tools`, D9) |
| RPC `a2a_grantable_tools`, `GrantableTool.ts`, `tools::ContactGrantable`, `CONTACT_GRANTABLE`, `contact_grantable()` | removed; `bindings/methods.json` loses the method |
| `ContactPatch.modes` | stays, deprecated (D2) |
| `ContactPatch.cap`, `Contact.cap`, `PeerEntry.cap`, `a2a.contact_cap` | stay, inert; the UI hides them |
| `ShareMode`, `PeerOrigin.contact_mode`, `PeerRecord.contact_mode`, `ContactOffer.mode`, `ContactOrigin`, `ContactEnvelope`, `Contact.full_capable`, `CancelReason` | unchanged. `contact_mode` is `full` for a contact's message and `null` otherwise, and the UI's "asked during X's message" tag keys on it |
| DB columns | unchanged |

## Tests (minimum)
**Engine:**
1. A 186-written `peers.yaml` loads in three variants: a grant with no `modes`, `modes: {x: full}`, and `modes: {x: cautious}`; every grant is full and the last variant logs. A file 190 writes has no `modes` key and adds no key.
2. A contact's message to a local chat export arrives as a peer message, with no `<contact_message>`, and its task completes as a receipt.
3. During a contact's turn the owner's interject cancels that turn with no refusal; a later `send_message(reply_to)` still reaches the contact.
4. An agent export runs full under the agent's ceiling, and all S1–S10 tests still pass.
5. `contact_update`: `cautious` is refused, `full` is accepted, and a newly granted export to a contact without `full_capable` gets "update <machine> to share".
6. Re-pointing an export keeps it full.
7. The W-40 tests pass unchanged.
8. Sender side: a cautious-tagged older card still gets the 183 shape, and its final-text import works.

**UI:** see the UI brief.

## Proof
- Engine: crate gates, plus a two-engine scratch rig reusing `/tmp/d186/rig` if present. The rig shows:
  - a pre-190 cautious grant now delivering full;
  - two answers by `send_message(reply_to)`;
  - an owner interject cancelling the contact turn, and a third answer arriving afterwards.
- UI: vite fixture screenshots.
