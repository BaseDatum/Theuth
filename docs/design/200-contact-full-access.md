# 200 — CONTACT-FULL-ACCESS: the ultimate trust level for a contact

Status: Approved for build 2026-11-17 from Sky's rulings. Docs only.

Code read at main `852d43f1e`. Every `path:line` below holds at that sha. The rulings are in `~/.theuth/briefs/2026-11-17/200-contact-full-access/rulings.md`. The mock is [200-mock.html](200-mock.html). Read with [183](183-remote-sessions-and-a2a.md), [186](186-contact-collaboration.md), [190](190-drop-cautious.md), [191](191-mesh-contacts.md) with [191-notes](191-notes.md) (the notes win), and [199](199-contact-files.md), which this design does not change beyond its one share check (see "Interplay with 199").

## Sky's words

"I want there to be a full access mode for contacts, where the user can set a contact to full access and then any session can communicate without any restriction to any contacts session, and they can list each others sessions and stuff just like two machines in the same mesh, except nothing is synced, it's just any session to any session communication and listing a contacts sessions is enabled, so it's like the ultimate trust level for a contact."

- Scope: "Message + list only". Any of their sessions can message any of yours (files included, design 199), and they see your session list (titles, running/idle, activity). They cannot read your transcripts or open chats on your machine. Nothing syncs.
- Direction: "Each side grants its own". Setting a contact to full access opens YOUR sessions to THEM; it is two-way only when they also set you to full access.
- Private chats: "Private chats excluded". Never listed or reachable, even for full access.

## Today (verified at `852d43f1e`)

| What | Where | Fact |
|---|---|---|
| Address parsing | `crates/agent/src/communication.rs:879-903`; `crates/agent/src/communication/network.rs:1025-1044`, `:4549-4556`; `crates/a2a/src/contacts/reach.rs:494-497` | `PeerAddress::parse` knows `session:<uuid>`, `peer:<name>` (≤ `ALIAS_MAX` = 64, `communication.rs:98`) and relative names. `peer_by_name` maps `<contact>/<export>` to the contact's route only when the export is `[a-z0-9_-]`. `network_send` keeps the part after `/` as the export, and the contact transport posts to `/agents/<export>/rpc` (bare: `/rpc`). |
| Export lookup | `crates/a2a/src/server.rs:244-278`, `:378-427`, `:335-339` | `Live::build` mounts one JSON-RPC router per export at `/agents/<name>`. `dispatch` takes the first path segment as the export; a contact's bare `/rpc` goes to its first granted export; anything not granted, missing or disabled is 404. |
| Share gate | `server.rs:419-427`, `:655-709`; `network.rs:1456-1511` | `dispatch` admits a contact only to `principal.exports` (or `~return`). `Handler::admit` builds the `A2aPrincipal` (binding, export, session, epoch, `ContactOrigin`, `ask_first`). `admit_principal` checks the live binding, the epoch, a native session row, `chat_paused` (archived, agent messages off) and `Policy::contact`. |
| First-contact hold | `server.rs:666-676`; `network.rs:1759-1769`, `:3725-3735` | `first_contact` absent or `ask` sets `ask_first`; `admit_send` then parks the task with `hold_task` (`AUTH_REQUIRED`) until the owner answers; `block` refuses. |
| Contact bounds | `server.rs:428-442`; `network.rs:1624-1653`, `:3474-3476` | 10 req/s and 8 handlers per contact; at most `CONTACT_QUEUED` = 8 waiting tasks per (contact, export). |
| 191 ownership | `server.rs:711-721`; `crates/agent/src/communication/network/full.rs:143-160`; `crates/agent/src/store/contacts.rs:498-509` | Before any task exists, the router's strict resolve (`owner_strict`) answers `not owner` (-32030) for an unknown row, an empty owner, or another machine's row, naming the owner in a detail when it is ours. |
| 191 forwarding | `crates/a2a/src/contacts/forward.rs:78-84`, `:127-228`, `:336-394` | The receiving machine runs its own router; a `not owner` naming one of our machines is carried over `theuth/rpc/1` (unary `SendMessage`/`GetTask`/`CancelTask`, at most two resolves). `carried` admits only `/agents/<[a-z0-9_-]>/rpc` or `~return`, on both sides. |
| Delivery | `network.rs:1785-1799`; `full.rs:315-408` | A contact's task goes to `deliver_full`: owner check, then the message enters like a mesh peer's (`from = peer:<contact>`), and the conversation is recorded inbound (`open_contact_thread(…, task.export, false)`, `full.rs:378-380`). |
| Context binds the session | `crates/agent/src/store/communication.rs:2273-2312`, `:2460-2483` | A context row belongs to (principal, export) and names one session; a new task's recipient is the context's session. |
| Return path | `server.rs:279-297`, `:792-807`; `full.rs:197-311`, `:681-830` | `~return` is mounted for contacts only. An answer to an inbound conversation goes back over the contact's `~return`, refused locally once the export is no longer shared (`full.rs:239-250`). The sender admits it for a live outbound conversation unless every verified card of the contact closes it (`share_closed`, `full.rs:726-728`; `network.rs:983-994`). The admitted answer shows `from = peer:<contact>/<export>` (`full.rs:798-801`). |
| discover_peers | `crates/agent/src/peers.rs:617-730`; `crates/agent/src/supervisor.rs:6879-6956`, `:7019-7171`, `:7227-7292`; `network.rs:878-942` | `ListPeers` reads `store.list_sessions()` off the mailbox, then lists our mesh's sessions: rows owned elsewhere are `state: remote` with no activity (`supervisor.rs:7098-7109`); titles only for the caller's own tree (`:7113`). Contacts follow as `peer:<contact>` plus one row per export from its cached card (`network.rs:921-939`); a contact's inbox sees no contacts (`:883`, `:906`). `arrange` groups contact rows (kind `a2a`, first tag `contact`) and caps them at 8 per contact (`supervisor.rs:7228`). |
| Contact record | `crates/agent/migrations/0059_mesh_contacts.sql:1-19`; `store/contacts.rs:17-27`, `:40-59`, `:292-304`; `crates/a2a/src/contacts/records.rs:1030`, `:1072-1103` | `contacts` is a replicated record table (one key, `hlc`, no constraints). Staging turns a group into a `Peer` (the grant) and `ContactFacts`; `blocked` is "any record of the group". |
| Where a per-contact flag lives | `crates/a2a/src/config.rs:240-261`, `:280-304`, `:405-421`; `crates/a2a/src/lib.rs:1509-1532`, `:935-937`, `:773-792` | A field of `Peer` is part of the grant: `same_grant` compares `Peer` whole; a changed grant gets a fresh epoch and the old one is cancelled once the new configuration is live (held first messages are re-checked against it). `ContactFacts` is kept apart so a first-contact answer is not a new epoch. |
| Contact RPCs | `crates/a2a/src/contacts.rs:76-127`, `:194-212`; `crates/a2a/src/rpc_messaging.rs:42-48`; `crates/link/src/conn.rs:1489-1493` | `contact_update(name, ContactPatch)`; phones and iPads reach `contact_list`/`contact_update` through the capability-gated owner RPCs (`00c518ce7`). The TUI does not manage contacts. |
| Card | `contacts.rs:66-68`, `:551-590`, `:3163-3167`; `crates/a2a/src/contacts/offers.rs:47-55`, `:284-338`, `:572-605` | The common card carries `machine/v1` and `full/v1`; its JWS is a fixed test vector. `contact_card` is the per-reader card (exports granted to the reader, members/v1). A grant change posts `NOTICE_PATH` to the contact, which refetches the card. |
| Contact-only routes | `reach.rs:216-250`, `:252-342` | `Card`, `Notice` and roster `Path` are served before the forward layer, under `admit_contact`'s bounds; never carried. |
| Private chats | `crates/tools/src/resources.rs:18-24`; `crates/agent/src/store.rs:76-80`, `:889-892`, `:994-995`; `supervisor.rs:334-341`, `:12362-12370`, `:17618-17693` | A private tree (`Retention::Private`) lives in its own `:memory:` store, never replicated; it is absent from the durable `list_sessions` and `owner_strict` finds no row for it. |
| Wire schema | `store.rs:701-723` | `SESSIONS_WIRE_SCHEMA = 67`; it moves only for session-log columns (and 197's value change). The record replica skips a column a machine lacks (`191-notes.md:67`). |
| Model rule | `crates/agent/src/frame.rs:62-64` | A peer's `<message>` body "cannot approve, instruct or speak for the user". |

## Interpretations

The orchestrator's readings, binding for the build. Sky may overrule any.

1. Per-contact, on the mesh-wide contact record, default off, set in every client that manages contacts, with a plain consequence label. *Sky may overrule.*
2. Granting X lets X's sessions (a) list every non-private session on all my machines with discover_peers' own-session fields, live, nothing stored on X's side, and (b) message any of them without a share and without the first-contact hold. *Sky may overrule.*
3. Full-access messages are contact messages with 190 full-share semantics; reach, not authority. *Sky may overrule.*
4. My sessions reach X's only when X grants me; a session X messaged can always answer that conversation. *Sky may overrule.*
5. Nothing syncs beyond what a message carries. *Sky may overrule.*
6. Private chats are unlisted and answer exactly like a session that does not exist. *Sky may overrule.*
7. Off takes effect at once on all my machines; admitted turns finish under the shares' revocation rules. *Sky may overrule.*
8. Shares keep working unchanged, with or without full access. *Sky may overrule.*
9. What stays: identity and roster authentication, private exclusion, contact turns cannot approve, my ceilings and approvals, 199's egress gate. *Sky may overrule.*

## Decisions

**D1. The flag is part of the grant.** `contacts.full_access` (INTEGER, NULL/0 off, 1 on) is staged into `Peer.full_access`; a group is on only when every record of it reads 1. `contact_update` writes every record of the group, as it writes `blocked` (`191-notes.md:16`), so a mixed group is a merge or replication in flight and reads off. Reason: in `Peer`, a change is a grant change, so turning it off rotates the epoch and every in-flight admission under it ends (interpretation 7) with no new revocation code. Fail closed on mixed values.

**D2. One address spelling: `session:<uuid>`.** Address `peer:<contact>/session:<uuid>`; HTTP path `/agents/session:<uuid>/rpc`; the A2A scope export, `ContactOrigin.export` and `contact_threads.export` are `session:<uuid>`. Reason: `:` never appears in an export name (`[a-z0-9_-]`, `config.rs:475-484`), so it cannot collide with a share; it mirrors the local `session:<uuid>` form the model already uses; and the existing client builds the path from the export unchanged (`reach.rs:494-497`). A per-session scope export is required, not cosmetic: a context fixes its session (`store/communication.rs:2483`), so one constant route would let a reused `contextId` land in the wrong chat.

**D3. A path-segment route, resolved by `dispatch`.** `dispatch` accepts `session:<uuid>` for any contact caller, rewrites the URI to the one mounted `/agents/~session/rpc` handler (`Target::Session`), and sets the trusted header `x-theuth-session` (any inbound copy removed first, as `PRINCIPAL_HEADER` is at `server.rs:381`). Reason: the router is static per reload; one handler serves every session, and `GetTask`/`CancelTask` carry the session in the path like `SendMessage`.

**D4. Admission order on `Target::Session`.** (1) gate: a current, unblocked roster member of the contact (`iroh_gate`, unchanged); (2) `ContactGrant.full_access`, else `full access is off`; (3) strict owner resolve, before anything else touches the session: `not owner`, forwarded per 191 when another of our machines owns it; (4) `admit_send` as for any contact message, with `ask_first = false`. Reason: the owner check before `admit_principal` means an unknown session and a private chat (no durable row anywhere) both read `not owner` on every machine; no text, timing class or status tells them apart (interpretation 6).

**D5. Listing: a contact-only route, each machine answers for itself, the contact fans out.** `GET /theuth/sessions` sits beside the card and the notice in `reach::Route` (never carried). A machine lists only sessions it owns (`owner_strict` = here) in its durable store, so private chats are never in it. The contact's engine asks every admitted member of our roster at once (it already dials them, `191-notes.md:73`) and merges. Reason: each answer carries live running/idle for its own rows, which a single node cannot know for the others (`supervisor.rs:7098-7100`), and no mesh-internal call is added. A machine that does not answer simply has its sessions missing, which is honest: they are unreachable then too.

**D6. The list is every non-private session, with `send` telling the truth.** Native, ACP and node sessions are all listed (ruling: every non-private session); `send` is true only when a contact message would be admitted now (native, not paused, communication on: `network.rs:1475-1510`). Messages to a `send: false` row get today's refusal texts. Reason: the ruling asks for the whole list, and a contact message cannot run in an ACP session today (`network.rs:1475-1477`); this mirrors the watch-only child rule (`supervisor.rs:7120-7121`).

**D7. Live and uncached on the asking side.** Each `discover_peers` / `list_peers` call that lists a full-access contact asks its machines then, within `LIST_DEADLINE` (2 s), first page only (100 rows per machine); `kind: "contact"` pulls up to 5 pages per machine. Nothing is kept after the call. Reason: interpretation 2a says live, nothing stored. ponytail: one request per member per listing; add a short in-memory TTL only if contacts hit the 10 req/s bound.

**D8. Delivery is 190's full path.** A full-access message is a contact task through `deliver_full` (`network.rs:1793-1799`): mesh-peer wrapper, consumed receipt, contact provenance, the chat's own tools and the agent's ceiling, approvals to the owner, the frame rule that a body cannot approve (`frame.rs:62-64`). No hold, no share needed. The rate, the 8-waiting bound per (contact, session) and the chain limit stay: they are control bounds, not trust restrictions. Reason: interpretation 3, with the least new code.

**D9. Return path, both ways.** The receiver records the conversation inbound with export `session:<uuid>` (unchanged code), so my session answers with `send_message(to=peer:<contact>, reply_to=id)` over the contact's `~return`. The D12 checks gain one branch each: replying on an inbound `session:` conversation needs MY grant still on (`full.rs:239-250`); admitting an answer on an outbound `session:` conversation needs THEIR verified cards still granting me (`full.rs:726-728`). Reason: interpretation 4 (the answerer needs no grant from the asker) with share semantics on revocation.

**D10. Advertised per reader.** `https://theuth.io/a2a/ext/full-access/v1`, `params: {"granted": true|false}`, only on the per-reader `contact_card`. Present = this engine supports full access; `granted` = this reader has it. The common card and its JWS vector (`contacts.rs:3163-3167`) do not change. Toggling is a change of `granted`, so `offers::changed` notifies the contact (`offers.rs:588-605`). Reason: the sender pre-checks and the UI tells the user what the other side granted, from data already fetched.

**D11. The answering engine decides; cards are hints.** Every refusal that matters is made by the machine that owns the session, from its own replicated record. The asking side's card check only avoids a doomed send and words the error. Reason: same rule as 186 D12 (`network.rs:983-988`).

**D12. Clients.** Desktop: a Full access switch in each contact's Access row (`crates/desktop/ui/src/components/settings/Contacts.tsx:844-955`), with a confirm on turning on. Phones and iPads get it through the same UI and `contact_update` (already gated for paired owner devices, `conn.rs:1489-1493`). The TUI manages no contacts, so it gets no switch; it renders the rows in `/peers` unchanged (`crates/tui/src/components/peers.rs:643-662`). Reason: interpretation 1 without a new RPC.

## Contract

**Record and migration.** `crates/agent/migrations/0068_contact_full_access.sql` (0067 is 199's `contact_files`, present on `theuth-199-build`; no `theuth-*` worktree has a 0068; re-check before naming):

```sql
-- 200 CONTACT-FULL-ACCESS: a per-contact trust level on the mesh-wide contact record.
-- `contacts` rides the record replica (191), not the session log: SESSIONS_WIRE_SCHEMA stays 67.
-- NULL or 0 = off (fail closed), 1 = on. No constraint (record-replica rules, 0059).
ALTER TABLE contacts ADD COLUMN full_access INTEGER;
```

It touches a record-replica table only, so `SESSIONS_WIRE_SCHEMA` stays 67. An older machine of my mesh skips the column (`191-notes.md:67`) and has no session route: it never admits full access (fail closed). `ContactRow.full_access: Option<i64>`; `RecordTable::Contacts` columns gain `"full_access"` (`store/contacts.rs:294-304`); `records.rs` reads it beside `blocked`.

**Types and RPC.**
- `config::Peer.full_access: bool` (D1); `server::ContactGrant.full_access: bool`; `agent::communication::network::PeerRoute.full_access: bool` and `RemotePeer.full_access` (my grant, for D9).
- `ContactPatch.full_access: Option<bool>`: `contact_update` writes the column on every record of the group, answers the updated `Contact`, and reconciles at once (`Ctx::commit_records`). No new method, so no capability change.
- `Contact.full_access: bool` (my grant) and `Contact.full_access_from: Option<bool>` (their latest verified card: `None` = no extension, their theuth does not support it; `Some(granted)`). TS: both `#[serde(default)]`, optional.
- `ContactOffers.full_access: BTreeMap<String, bool>` (contacts whose verified cards grant me) and `SettledCards.full_access: bool` for D9.
- `offers::Cached.full_access: Option<bool>`, read from the extension.
- New module `crates/agent/src/communication/network/access.rs`: `SESSION_PREFIX = "session:"`, `SESSION_ROUTE = "~session"`, `fn session_route(export: &str) -> Option<SessionId>`, the listing answer and the D9 helpers.
- New module `crates/a2a/src/contacts/sessions.rs`: `EXT_FULL_ACCESS`, `SESSIONS_PATH = "/theuth/sessions"`, `SESSION_HEADER = "x-theuth-session"`, `PAGE_DEFAULT = 100`, `PAGE_MAX = 200`, `PAGES_MAX = 5`, `LIST_DEADLINE = 2 s`, `TITLE_MAX = 120`, the route and the fan-out client.
- `PeerSender::list_sessions(&self, all_pages: bool) -> Result<Vec<ContactSessionPage>, String>`, default `Err("not a contact")`, so HTTP peers need nothing.
- `PeerAddress::parse` and `peer_by_name` accept `<contact>/session:<uuid>` (contact ≤ 64 chars, the suffix a UUID). `forward::carried` accepts a `session:<uuid>` segment (one edit covers both sides, `forward.rs:78-84`, `:351`).

**Listing operation.** `GET /theuth/sessions?limit=<1..200>&after=<session uuid>` on `theuth/a2a/1`, served in `reach::contact_route` under `admit_contact`.
- Admission: gate `Contact` (as every route), then `Peer.full_access`, else `403` with body `full access is off`; `contacts_ready` false is `503`. Unknown `after`: `400 unknown cursor`.
- Rows: sessions this machine owns, from the durable store, ordered `(created_at, id)`; `after` is the last row's id.
- Answer (`application/json`, at most 256 KiB):

```json
{ "machine": "Sky's MacBook Pro",
  "sessions": [ { "session": "0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f", "title": "Port layout review",
      "kind": "native", "state": "running", "send": true,
      "activity": { "state": "running", "last_active": "2026-11-17T14:31:00Z", "turn_secs": 42,
                    "jobs": 0, "ptys": 0, "children": 1 } } ],
  "cursor": "0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f" }
```

- Fields are exactly discover_peers' own-session fields: the address (built by the reader from `session`), `title` (control and bidi characters removed, cut to 120), `state` (`PeerState`), `activity` (`PeerActivity`), `kind`, and `engine` (= `machine`). Plus `send` (D6). No tags, no lineage, no cwd, no model, no transcript.
- The reader's fan-out: every admitted member at once, each within `LIST_DEADLINE`; `403` from any member means "not granted" for that call; a member that does not answer is noted.

**Messaging.** `send_message(to="peer:<contact>/session:<uuid>", …)`, files included (199). The sender checks its cached card: no extension refuses, `granted: false` refuses, else it opens an outbound conversation with export `session:<uuid>` (`open_contact_thread`) and posts to `/agents/session:<uuid>/rpc` in 190's full shape (no new metadata).

**Admission (D4)** runs on whichever machine receives the request; `not owner` naming another of our machines is forwarded per 191 and the owner repeats D4 on its own tables. A context is minted per (contact, `session:<uuid>`).

**discover_peers rows** for a contact whose cards grant me, after its `peer:<contact>` row:
- `address` `peer:<contact>/session:<uuid>`, `name` `<contact>/<title, or the first 8 of the id>` (cut to 40), `kind` `a2a`, `relationship` `remote`, `title`, `state` and `activity` as answered, `send` as answered, `reply` = native, `tags` `["contact", "session", <their kind>]`, `engine` = their machine.
- They group and cap like export rows (`arrange`); a contact's inbox still sees no contacts.
- The `peer:<contact>` row's title notes `full access · <n> sessions`, plus `<machine> did not answer` when one did not.
- The tool description gains: "A contact that gave you full access lists its sessions as `peer:<contact>/session:<uuid>` rows (tags `contact`, `session`); send_message reaches any of them."

**Wire events and UI.** No new event. `contact_update` already ends in `A2aChanged`, which refetches `contact_list`; the contact refetches our card on the notice. Desktop `Peers.tsx` and TUI `/peers` render the new rows as returned; Enter on one in the TUI keeps "remote peer: no transcript here" (`peers.rs:208-209`). Labels:
- Off: "Full access is off. Maya reaches only the chats you share."
- On: "Full access: any of Maya's sessions can message any of your sessions and see your session list (titles, running or idle). Private chats stay hidden. Nothing is synced."
- Confirm on turning on: title "Give Maya full access?", the On text, buttons "Cancel" and "Give full access".
- Their side: "Maya has given you full access: your sessions can list and message hers." · "Maya has not given you full access." · "Maya's theuth does not support full access yet."

**Refusal and error texts.**

| Case | Who says it | Text |
|---|---|---|
| Not granted (server) | owner, JSON-RPC `UNSUPPORTED_OPERATION` | `full access is off` |
| Not granted (sender's card, or the server text) | sender receipt / tool error | `peer:maya/session:<uuid>: maya has not given you full access; message a chat maya shares instead (discover_peers kind="contact")` |
| Their engine lacks it | sender, before sending | `peer:maya/session:<uuid>: maya's theuth does not support full access yet` |
| Unknown or private session | every member `not owner`; sender maps it | `peer:maya/session:<uuid>: no such session` |
| Bad id in the address | sender | `peer:maya/session:<x>: not a session id` |
| Owner machine offline | receiving machine (191) | `the chat's machine is offline` (unchanged) |
| Owner on an older build (404) | sender maps it | `peer:maya/session:<uuid>: the machine that holds this session needs updating` |
| Session paused, communication off, not native | owner (unchanged `ContactRefusal`/admission texts) | e.g. `export paused: chat archived` |
| Too many waiting | owner (unchanged) | `busy: <n> tasks already wait here; retry later` |
| Listing not granted | `403` body | `full access is off` (row title: `maya has not given you full access`) |
| My reply after I turned it off | my engine, local | ``peer `maya`: full access is off; nothing was sent`` |

## Revocation

- **Off** writes `full_access = 0` on every record of the group. Each of my machines restages within one reconcile tick of receiving the row (about a second in the 191 rigs, `191-notes.md:55`). `Peer` changed, so the epoch rotates (`lib.rs:1509-1532`, cancelled at `:935-937`).
- **In flight:** a request in flight ends `401` (`server.rs:448-451`). Tasks admitted under the old epoch meet exactly what any grant change does to a contact's tasks today: a queued or held one fails when its epoch ends (`network.rs:1784`, `:3677-3696`; full access never holds), and a message already handed to the session is consumed and its turn finishes as an ordinary turn (186 D12). New listings are `403` and new messages `full access is off` from the same swap.
- **Conversations:** my replies on full-access conversations are refused locally (D9). The contact's `~return` refuses its own conversations into my sessions once my refreshed card says `granted: false` (the notice makes that immediate; otherwise within `OFFERS_TTL`).
- **Shares:** the epoch rotation also ends in-flight tasks of that contact's shares, exactly as any grant change does today; the shares keep working afterwards (interpretation 8).
- **Removal or block** ends everything as today (the gate refuses, the binding is revoked).

## Interplay with 199

Files to any session use 199 unchanged: a full-access message is a contact message through `deliver_full`, where 199 writes its file rows, and answers come through `full_return`, which 199 covers too. The one change is in 199's D17 "share re-check" for `contact_file_notice`: the export grant, a live `~return` conversation, **or full access on for that contact and a durable row for the session** (a private chat has none, so it never gets a notice). Nothing else in 199 changes: its file route authenticates by contact record id, never by share.

## Failure modes

| Cause | Asking side sees | Owner side | Notes |
|---|---|---|---|
| Not granted | Local refusal, or `full access is off` mapped | Refused at D4 step 2, nothing stored | Card stale: server refuses anyway (D11) |
| Private or unknown session | `no such session` | `not owner` on every machine | Same answer for both (D4) |
| Owner machine offline | `the chat's machine is offline`; its rows missing from the list | — | Nothing queued (191) |
| A member does not answer the listing | Its sessions missing; `<machine> did not answer` | — | 2 s deadline per call |
| Mixed versions in my mesh | `needs updating` (404 from an old owner) | Old engine: no route, no column | Fail closed |
| Contact's engine without 200 | UI: "does not support full access yet" | My grant harmless | — |
| Turned off mid-message | `401` for a request in flight; tasks as on any grant change | Epoch rotated | Consumed turns finish |
| Replication lag after a toggle | Up to one tick of the old answer per machine | Each machine decides from its own record | As every 191 grant |
| Merged group with mixed values | Off | Off | Re-toggle writes all records |
| Session archived, messaging off, ACP | Listed `send: false`; today's refusal on send | — | D6 |
| Rate or waiting bound | `429`/`503` on listing; `busy` on send | Unchanged bounds | D8 |
| Chat moves mid-message | `the chat moved; send again` | `HOLD_MOVED` (191) | Unchanged |
| Contact removed or blocked | Gate refusal | Binding revoked | Unchanged |

## Phases

Built on top of 199's integration branch. Shared seams with 199: `crates/a2a/src/contacts.rs`, `contacts/reach.rs`, `server.rs`, `crates/agent/src/communication/network/full.rs`, `peers.rs`, `store.rs`. 200's logic goes in the two new modules (`contacts/sessions.rs`, `communication/network/access.rs`); edits in shared files are one-line hooks: a `Route` arm, a `dispatch` branch, a `Target` variant, two D9 branches, one `carried` clause.

| Phase | Gate | Tests (minimum) |
|---|---|---|
| **P0 contract** | Migration 0068; `ContactRow`/`RecordTable` column; `Peer`, `ContactGrant`, `PeerRoute`, `ContactPatch`, `Contact`, `ContactOffers`, `SettledCards`, `Cached` fields; constants and wire types; `PeerSender::list_sessions` default; TS bindings; OpenRPC baseline accepted with Breaking 0. | A group with one record at 0 or NULL stages off; all 1 stages on; toggling changes `same_grant`. `SESSIONS_WIRE_SCHEMA` snapshot test unchanged. `session_route` refuses non-UUIDs and export names. |
| **P1 engine** | Staging and `contact_update`; card extension; `dispatch` + `Target::Session` + D4; `carried`; listing route and fan-out; discover rows; sender path and error mapping; D9 branches; 199 D17 hook. | Two-engine e2e: B lists A's sessions (titles, running/idle); a private chat of A is absent and its id answers `no such session` byte-identical to a random UUID; B messages an unshared chat of A with no hold; A answers with `reply_to`. A without B's grant cannot message B (`has not given you full access`). Owner forwarding: a session on A2 reached through A. Off: listing 403, send refused, a request held open across the swap ends 401, a consumed turn finishes, A's reply refused locally. A context id reused across two sessions lands in each its own. ACP session listed `send: false`. Rate bound untouched. |
| **P2 UI** | Desktop switch with confirm and labels; their-side line; phone/iPad layout at 390 px with 44 px targets; Peers rows; site docs (`site/content/docs/automation/mesh.mdx`). | Vitest: switch on asks to confirm and sends `contact_update({full_access: true})`; off sends `false` with no confirm; label per state; `full_access_from` `None`/false/true lines. Fixture screenshots of each state. TUI: a contact session row renders through `PeerRow::line`. |

## Verification (the closer's live proof)

Two scratch engines A and B paired as contacts over real iroh (relay none, fake models, scratch data dirs); A2 in A's mesh. A log or screenshot for each step.

1. Before any grant: B's `discover_peers` shows A's exports only; `send_message(peer:a/session:<A chat>)` reads `a has not given you full access…`.
2. A turns on full access for B (desktop, confirm shown). Within seconds B's card reads `granted: true`; B's `discover_peers` lists A's and A2's sessions under `peer:a` with titles and live running/idle; A's private chat is absent.
3. B messages an unshared A chat and an A2 chat: both arrive as `<message from="peer:b" …>` with no Allow strip; A answers with `reply_to`; B's session receives it.
4. B sends a file (199) to the A2 chat: text first, then the notice, on A2.
5. B sends to A's private chat id and to a random UUID: both `no such session`; the a2a logs on A and A2 show `not owner` for both.
6. A's sessions try B's: refused (B never granted). B grants A: A lists and messages B's sessions.
7. A turns full access off mid-flight: B's next listing is refused, B's next send reads `has not given you full access`, the running turn finishes, A's reply on that conversation is refused locally; A's shares to B still work.
8. Captures: desktop Access row on and off, the confirm, the Peers panel with B's sessions, a received message, the refusal, TUI `/peers`.

## Flags for Sky

1. **Other contacts' inbox sessions.** The ruling lists every non-private session, so an agent export's inbox for contact Y is listed (with its title) and reachable by full-access contact X. That tells X whom else you talk to, which 183 kept from contacts ("a contact never learns our contact list", `network.rs:880-881`), and puts X's text into Y's conversation. Built as ruled. Recommendation: also leave out sessions carrying a `contact_inbox` marker (one filter in `access.rs`, same `no such session` answer).
