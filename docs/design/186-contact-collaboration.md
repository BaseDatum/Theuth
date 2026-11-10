# 186 — CONTACT-COLLAB: contacts collaborate in full or cautiously

As built on `feat/186-engine` (engine) and `feat/186-ui` (UI), from main 19251272.

## Sky's words (2026-11-10)

> "I don't understand why we've made it a restrictive. their message must have a complete turn. is that built into A2A or something? The way I see it once the chat is shared and a message is allowed and such, then the two agents should be able to message freely as if they're in the same mesh or running on the same theuth harness. that's the whole point to make it where they can work together. So it seems like this has kinda missed the point, and was built too restrictive, unless you have a good reason for it. the message should arrive like any other message and the agent that received it should be free to reply with whatever information they want. I also don't know why we restricted links, images, etc. why wouldn't they be able to send those things. the trust is established, so why are we treating this like it's a danger zone?"

Ruling (ask_user): **"Choose when sharing"**. Every share picks FULL collaboration or CAUTIOUS when it is made. There is no default: the choice is required.

Nothing in A2A requires one turn per message: that was 183's choice (each message a task whose result is the turn's text). FULL keeps A2A as the transport and drops the task-per-turn shape.

## Decisions

- **D1. Per grant.** The mode belongs to a grant (contact × export): one agent export can go to several contacts in different modes. `ShareMode { Full, Cautious }`, serde `full | cautious`. In `peers.yaml` it sits beside `first_contact`: `PeerEntry.modes: BTreeMap<export, ShareMode>`. Staged as `Peer.full: BTreeSet<export>`.
- **D2. Migration.** A grant with no mode is CAUTIOUS (today's behaviour); nothing is rewritten. `modes` is serialized only when it holds a `full` entry (`skip_serializing_if = "no_full"`), because `PeerEntry` is `deny_unknown_fields` and an older build would refuse the whole file. Only `full` entries are stored (cautious = absent). A mode change is a grant change: `Peer` equality (`same_grant`) and `unchanged_exports` compare it, so it is a new grant epoch.
- **D3. Required choice.** `contact_update` refuses a newly granted export that names no mode: "choose full or cautious for X: every share picks its mode". A mode for an export not granted is refused. A hand-edited grant without a mode is cautious (fail closed).
- **D4. FULL delivery (receiver).** A contact's message arrives exactly like a mesh peer's: an unscoped, non-isolated `PeerInput` through `peer_wrapper` (`<message from="peer:<alias>" id=…>`), no `<contact_message>`, no "untrusted" note; `neutralise_delimiters` keeps the body data. It steers into a running turn or opens one. Its A2A task waits aside (`working`, `held = 1`: it takes no slot, a cautious task of the same chat keeps running) and completes when the message is consumed, `Task.metadata.theuth.receipt = true`, no artifact. The sender marks its message `consumed`; nothing is imported from turn text. The owner's typing never touches it.
- **D5. Replies and follow-ups (the return path).** The receiving agent answers with `send_message(to=peer:<alias>, reply_to=<id>)`, any time, any number of times; an agent inbox may too (through `reply_to`). The receiver's engine POSTs an A2A `SendMessage` to the sender's contact-only return route `/agents/~return/rpc` (never in a card) with `referenceTaskIds = [the answered message's task]`, the same `contextId`, `metadata.theuth.re = <the answered message's messageId>` and `metadata.theuth.chain`. The sender admits it by (contact binding, `contextId`) against its own conversation record — scoped to (contact, originating session, conversation) — and answers `TASK_NOT_FOUND` when the conversation is unknown, quiet for more than 7 days (sliding on every message either way), the share is no longer full in the contact's latest card, or the contact is gone. Admitted, it is delivered like `import_reply`'s tail: a non-isolated `<message from=peer:<alias>/<export> id reply_to>` into the originating session, or straight into the `send_message` awaiting it. The sender's agent answers back with `send_message(reply_to)`: a new task on the same conversation to the export. `contextId` is minted by the sender per (originating session, contact, export).
- **D6. Tools in FULL.** The chat's own policy applies, as for the owner's turns: no `ContactFrame` (so no turn cap, no allowlist, no `set_turn_limits`); the journal records `contact_full`, which a resume never caps (unsure still caps). An agent inbox shared in full clears the contact limits (`clear_contact_limits`; the exported agent's own `tools:` ceiling stays) and records both on its row (`a2a_contexts.inbox_full`, `inbox_ceiling`, migration 0057), so an actor start after a restart re-installs that ceiling and not the contact's limits; a reload that takes the share out of full re-caps the live inbox and clears the flag, so its next actor start is capped (default-deny) again. `CONTACT_GRANTABLE`, the default-deny allowlist and the per-export tools editor stay CAUTIOUS-only. Approvals (`tools.policy`) apply unchanged.
- **D7. Content parity.** FULL renders Markdown, links and image URLs exactly like a mesh peer card (UI). Attachments (files, raw/url parts) are out of scope — the mesh has none either; a later design.
- **D8. CAUTIOUS unchanged, but the owner never cancels a contact turn.** Plain Enter already queues; interject, flush and talk's barge-in cancel with `reason: "input"`, which the engine refuses while the session's running turn took a contact's incoming message (any mode; TUI, Slack, Signal, talk and the desktop all go through `Agents::Cancel`). The guard reads the turn's journal (`input_accepted` source `contact`, `contact_full` or `contact_inbox`), so a resumed turn keeps it; an answer to one of ours (journaled `peer`) is not a contact's turn. Stop (`reason: "stop"`, the default) still stops it, and the contact reads "stopped by the owner", not a bare "canceled" (only for an explicit cancel; a talk cut-in or the runtime cap reads as before).
- **D9. Bounds.** FULL turns are peer turns (journaled `contact_full`, bounded by wall clock and permits). The chain count rides `metadata.theuth.chain`, bounded at `CHAIN_MAX` (256) as a mesh peer's. The 10 req/s rate and the queue of 8 per (contact, export) remain; the queue counts full messages not yet consumed, and the return route answers busy once 8 of a contact's answers wait to be taken in by one session.
- **D10. Compatibility.** FULL needs both machines on this build. The card advertises the extension `https://theuth.io/a2a/ext/full/v1`; `Contact.full_capable` says the contact's last card had it. `contact_update` refuses a new or upgraded full entry for a contact without it ("update <machine> to share in full"); an existing full entry sent back unchanged passes (addendum 15:57). An old peer that 404s on `~return` reads "update needed: … cannot take answers in full".
- **D11. W-40.** The mode rides the `ContactEnvelope` (`ContactOrigin.mode`, `#[serde(default)]` = cautious). The owner engine may lower it, never raise it — and lowers it to cautious: in full a contact's answers travel the holder's return route, which a turn on the owner cannot reach.
- **D12. Downgrade.** FULL → CAUTIOUS is a new epoch: the conversations' answers are refused on the receiver ("… is no longer shared in full; nothing was sent") and the sender's return route answers `TASK_NOT_FOUND` once the contact's card says cautious. Held tasks fail as today; a running FULL turn finishes as an ordinary turn; later messages arrive cautious.

## Contract (fixed names)

Rust and ts-rs bindings (first commit `f3ac8703`, `/tmp/d186/CONTRACT_READY`):

- `ShareMode` (`agent::communication`, re-exported as `agent::ShareMode` and `a2a::config::ShareMode`), with `as_str()`.
- `ContactPatch.modes: Option<BTreeMap<String, ShareMode>>`.
- `Contact.modes: BTreeMap<String, ShareMode>` (every granted export), `Contact.full_capable: bool`.
- `ContactOffer.mode: ShareMode` (what they granted us; card skill tag `full` | `cautious`, none = cautious).
- `A2aExportStatus.mode: Option<ShareMode>` (contacts only).
- `FirstWaiting.mode: ShareMode`.
- `PeerOrigin.contact_mode: Option<ShareMode>` (null for mesh peers), from `peer_exchanges.contact_mode`; also `PeerRecord.contact_mode` (optional).
- `discover_peers` contact export rows tag `full` or `cautious` after `allowed` / `ask_first`.
- `send_message` description: "A contact shared in full (tag `full`) talks like a mesh peer: answer its <message> with send_message(to=its `from`, reply_to=its id), as often as you like."
- RPC `cancel(session, scope?, reason?)`, `reason: "input" | "stop"` (`CancelReason`, default `stop`).

## Wire

A2A 1.0 JSON-RPC over `theuth/a2a/1` (iroh), unchanged transport.

Sender → receiver's export (`/agents/<export>/rpc`), one per message:

```json
{ "message": { "messageId": "<sender exchange id>", "contextId": "<sender-minted>",
    "role": "ROLE_USER", "parts": [{ "text": "…" }],
    "metadata": { "theuth": { "chain": 3, "re": "<receiver messageId answered>" } } },
  "configuration": { "returnImmediately": true, "historyLength": 0, "acceptedOutputModes": ["text/plain"] } }
```

The receiver returns the task `working`; the sender follows it. On consumption: `TASK_STATE_COMPLETED`, `metadata.theuth.receipt = true` (in the snapshot and on the status event, which the follower folds), no artifact.

Receiver → sender's return route (`/agents/~return/rpc`), one per answer:

```json
{ "message": { "messageId": "<receiver exchange id>", "contextId": "<the conversation>",
    "referenceTaskIds": ["<task of the answered message>"],
    "parts": [{ "text": "…" }], "metadata": { "theuth": { "re": "<sender messageId>", "chain": 4 } } } }
```

Answered at once with a `Message` carrying `metadata.theuth.receipt = true` (the answer was taken in; the receiver marks it `consumed`), or `TASK_NOT_FOUND`. The gate admits `~return` only on a contact's own `theuth/a2a/1` stream (HTTP bearers never), under the same per-contact rate and concurrency bounds; every other method there is refused.

`re` is checked against the reversed endpoints of the named exchange; a message already answered is not named again (`reply_to` stays on the first answer, later ones travel on the conversation alone), so "any number of times" holds against the store's one-answer rule.

## Store (migration 0056)

- `peer_exchanges.context_id` (the conversation) and `peer_exchanges.contact_mode` (`full` | `cautious`, NULL for mesh and HTTP peers; set on every contact exchange: provenance and D8).
- `contact_threads (binding, context_id, session, export, outbound, last_ms)`, PK (binding, context_id): `outbound = 1` minted here for `session` → the contact's `export`; `0` the contact's conversation into our export. `last_ms` slides on each message.
- `a2a_contexts.inbox_full`; `a2a_contexts.inbox_agent` and `inbox_ceiling` (migration 0057, edited in review round 2 before landing: `'none'` = the agent has no ceiling, NULL = unknown and capped; a scratch store that ran the round-1 0057 must be made again).
- Conversations are collected (`gc_contact_threads`) before each fresh full send: rows quiet past the TTL and rows of revoked bindings.
- `Store::hold_a2a_task` takes the status (`auth_required` for a first-contact hold, `working` for a full message).
- New queries: `read_exchange`, `set_exchange_contact`, `exchange_thread`, `exchange_by_request_id` (an answer's `re` is the answered message's `messageId`, the sender's idempotency key), `accepted_from` (the return route's bound), `open_contact_thread`, `outbound_contact_thread`, `contact_thread`, `gc_contact_threads`, `a2a_inbox_full`, `unmark_a2a_inbox_full`; durability `turn_took_contact` (the D8 guard).

## Code map

- a2a: `config.rs` (`PeerEntry.modes`, `Peer.full`, grant comparison), `contacts.rs` (`EXT_FULL`, `card_full`, card mode tags, `contact_update` D3/D10, views), `offers.rs` (mode in offers, `Cached.full`), `server.rs` (`~return` mount and gate, `Target::Return`, `ContactOrigin.mode`), `client.rs` (metadata, `referenceTaskIds`, status-event metadata fold), `lib.rs` (`PeerRoute.full`, `A2aExportStatus.mode`, FULL agent exports skip `ContactLimits`).
- agent: `communication/network/full.rs` (wire, `full_shape`, `deliver_full`, `full_intake`, `full_consumed`, `full_return`, `contact_turn`), `network.rs` (admission, routing, receipts, owner stop, inbox limits, W-40 lowering), `supervisor.rs` (`Cancel` guard, consumption hooks, chain-carrying intake), `loop_.rs` (`PeerMeta.contact`, journal `contact_full` / `contact_inbox`), `store/durability.rs` (bounded sources, `turn_took_contact`).
- talk: barge-in cancels with `reason: input`.
- rpc: `cancel` reason; bindings `ShareMode`, `CancelReason`. tui: interject cancels with `reason: input`.

## Calls made (inside the decisions)

1. `ShareMode` lives in the agent crate (a2a depends on agent; `ContactOffer`/`PeerOrigin` are agent types).
2. The contact turn guard (D8) reads the running turn's journal: it took a contact's incoming message (`contact`, `contact_full`, `contact_inbox`), in either mode and through W-40; an answer to ours lands as `peer`.
3. The return route acknowledges on acceptance (a `Message`, not a followed task); its receipt is "taken in".
4. Return admission checks the contact's latest card for `full` (refreshed on its notice), so a downgrade reaches the sender within one notice; the receiver refuses locally at once.
5. W-40 (D11): the owner lowers full to cautious (no return route from there).
6. A bare `peer:<contact>` in full means its first export by name, as the contact's gate maps `/rpc`.
7. The return route admits answers only for `outbound` conversations (ones this engine opened).
8. A FULL message's task is `held` + `working` while it waits for consumption; a restart fails it like any interrupted task.
9. Existing a2a e2e tests that granted without a mode now name `cautious` (D3 makes the old call an error); their assertions are otherwise unchanged, except the new `cautious` tag in a discover row.
10. D9's queue of 8 per (contact, export) also counts full messages not yet consumed.
11. A contact's task is followed by polling (`theuth/a2a/1` has no streams; 2 s doubling to 30 s), so a full message's `consumed` receipt shows within that interval; the answers themselves are pushed (return route) and arrive at once.
12. The e2e test registers the sharer's supervisor as `AGENT_ACTOR` (the runners report consumption to the registered one); every other a2a e2e test keeps both engines unregistered.
13. Re-pointing an export (another chat or agent) drops its `full` entries: it is shared cautiously until the owner picks full again (review round 1, rather than refusing the re-put).
14. A talk cut-in (`stop_reason: interrupted`) is the owner speaking over a turn, not a restart; it no longer reads "stopped by the owner" and, sent as `input`, no longer cuts a contact's turn.
15. Review round 1, item 8 (tag `secret_request` / `desktop_handoff` / `message_user` cards raised during a contact's turn): no engine field; the UI already knows the running turn's opener (`contactTurn`, `PeerOrigin.contact_mode`) and can tag what that turn raises. Left to u186.
16. Review round 1, item 9: a cautious contact message admitted behind the chat's running turn is visible at once. `contact_first_waiting` lists it after the holds as a `FirstWaiting` row with `queued: true` and `position` (1 = next; agent `ContactsQueued`), never as an Attention item; the row goes when its turn starts (`ContactsHeldChanged` on queue and start). Its task's status message is `QUEUED_NOTE` ("queued: the chat is busy; it runs when the current turn ends"), which the sender's receipt carries in `error` while `accepted`, cleared once it runs. A full message never waits there (it takes no slot).
17. Review round 2 (agent inboxes in full): the row keeps the exported agent's name; every admission reads its live persona (no persona of an agent — the roster not joined, its file missing or invalid — refuses the full share: "the agent … is not available"; theuth's own inbox before the roster joins keeps its stored ceiling), every actor start runs the stricter of the stored and the live ceiling (stored when the roster has not joined; an agent gone or neither known: capped), and a roster re-read (`InboxPersonasChanged`, also when the roster joins) applies the live ceiling to running inboxes at once. A reload clears `inbox_full` for every inbox of a contact whose export is no longer granted in full (and caps those no longer granted at all); a contact blocked, unpaired or re-pinned has every inbox capped when its binding falls out, and an actor start caps a full row whose binding is revoked.
18. Review round 2, NITs: the return route's bound of 8 counts only answers on conversations this engine opened and lets a retry through (its dedup, with the first acceptance's `reply_to`: the message it answered reads answered by then); an answer naming a message whose conversation was collected reads "the conversation with … ended" instead of falling to the task path.

## Tests

- a2a e2e `a_chat_shared_in_full_talks_both_ways_like_a_mesh_peer`: D3 refusal, D10 refusal and the unchanged-entry pass, a peer row (`<message from="peer:`, never `<contact_message`), receipt `consumed`, awaited send settled by `pong 1`, a second answer to the same message, ping-pong ×3 each way on one conversation, downgrade refused locally and `TASK_NOT_FOUND` on the return route.
- agent `a_full_contact_message_is_a_peer_message_and_its_task_a_receipt` (no slot taken, conversation recorded, receipt on consumption, chain limit, a cautious contact may not mint a context), `the_return_route_admits_answers_only_on_a_live_full_conversation` (unknown, 7-day TTL, downgrade, unpair → `TASK_NOT_FOUND`; admitted → full row with `reply_to`), `an_owner_stop_tells_the_contact_so`, `input_never_cancels_a_contact_turn_and_stop_does`, durability `contact_full` never capped and bounded.
- a2a config `modes_default_to_cautious_and_are_written_only_for_a_full_grant`.
- Review round 1: agent `a_full_agent_inbox_keeps_its_ceiling_across_a_restart_and_a_downgrade_caps_it`; the return-route test also covers another contact on the same `contextId`, an inbound conversation on `~return`, `re` naming another contact's message (no `reply_to`) and the bound of 8; the W-40 test sends full and asserts the owner lowers it; the D8 test reads the journal (`peer` answer cancels, `contact_full` does not); the e2e test re-points the export and reads `cautious`.

## Proof

Rig (`/tmp/d186/evidence.txt`, scripts `/tmp/d186/rig`): two scratch engines `a` and `b`, each its own mesh (`mesh_status` peers `[]`), paired only as contacts by invite ticket, each with its own fake model that answers a peer `<message …>ROUND n` with its own `send_message(to=<from>, reply_to=<id>, message=ROUND n+1)` up to 6.

- D3: `contact_update` without a mode → "choose full or cautious for proj"; with `{proj: full, notes: cautious}` → `full_capable: true`; B's `contact_list` shows `proj:full`, `notes:cautious`.
- FULL: B `send_message(peer:sky-moore/proj, ROUND 1)`; ROUND 2–6 then go back and forth by the two models' own tool calls (3 each way), every record `mode=full`, every answer with `reply_to`; A's chat history has 3 `<message from="peer:sky-moore" …>` rows and no `<contact_message>`; the FULL turns were offered the chat's whole toolset (`python`, `spawn_subagent`, `send_message`, …).
- A FULL message nobody answers reads `consumed` on B (receipt, no reply).
- CAUTIOUS as today: `peer:sky-moore/notes` ran one capped `<contact_message … cap="read_only">` turn with no tools and B's message read `replied` with the turn text.
- D8: in both modes the owner's `cancel(reason=input)` left the contact's turn running; `cancel(reason=stop)` stopped it; the cautious contact read "remote task Canceled: stopped by the owner".
- D12: downgrading proj → A's answer on the old conversation refused ("proj is no longer shared in full; nothing was sent"); B's next message to proj ran as a cautious `<contact_message>` turn.

Not exercised live: an old build on the other side (D10 "update needed" path; covered by the refusal mapping), W-40 lowering (an exported chat owned by another engine of the mesh).

Gates: `check --tests` and `clippy --tests --no-deps -D warnings` clean on a2a, agent, tools, rpc, mesh, theuth, slack, telegram, signal; the suites (Homebrew off PATH, fresh `t`) twice: 1677 passed, 1 failed each — the known flake `repl::tests::capping_binds_out_name`.
