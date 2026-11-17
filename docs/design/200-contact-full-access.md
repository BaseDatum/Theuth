# 200 — CONTACT-FULL-ACCESS: the ultimate trust level for a contact

Status: Approved for build 2026-11-17 from Sky's rulings. Docs only.

Review round 1 applied (2026-11-17): one eligibility predicate (D5), the session-kind matrix, other contacts' inboxes excluded, the listing under the grant epoch, honest revocation, the 199 seams. Review round 2: the per-member continuation is cut and replaced by one global keyset cursor (D8); task control on paused trees, ancestry opt-outs, the paused D17 notice and the settled-card closure are specified.

Review round 3: `partial` reaches every page (`PeerPage.partial` and a text line), the lossless claim is qualified, ids are deduplicated before the limit, the walk keeps the caller's own restrictions, and page 1's prefix is bounded apart from the session page.

Code read at main `852d43f1e`. Every `path:line` below holds at that sha. The rulings are in `~/.theuth/briefs/2026-11-17/200-contact-full-access/rulings.md`. The recon is [200-recon.md](200-recon.md) (its §8 is reconciled below); the mock is [200-mock.html](200-mock.html). Read with [183](183-remote-sessions-and-a2a.md), [186](186-contact-collaboration.md), [190](190-drop-cautious.md), [191](191-mesh-contacts.md) with [191-notes](191-notes.md) (the notes win), and [199](199-contact-files.md). 200 is built on top of the merged 199 engine; the seams are named in Phases.

## Sky's words

"I want there to be a full access mode for contacts, where the user can set a contact to full access and then any session can communicate without any restriction to any contacts session, and they can list each others sessions and stuff just like two machines in the same mesh, except nothing is synced, it's just any session to any session communication and listing a contacts sessions is enabled, so it's like the ultimate trust level for a contact."

- Scope: "Message + list only". Any of their sessions can message any of yours (files included, design 199), and they see your session list (titles, running/idle, activity). They cannot read your transcripts or open chats on your machine. Nothing syncs.
- Direction: "Each side grants its own". Setting a contact to full access opens YOUR sessions to THEM; it is two-way only when they also set you to full access.
- Private chats: "Private chats excluded". Never listed or reachable, even for full access.

## Today (verified at `852d43f1e`)

| What | Where | Fact |
|---|---|---|
| Address parsing | `crates/agent/src/communication.rs:97-98`, `:879-903`; `crates/agent/src/communication/network.rs:1027-1043`, `:4549-4556`; `crates/a2a/src/contacts/reach.rs:494-497` | `PeerAddress::parse` knows `session:<uuid>` (our own mesh), `peer:<name>` with the WHOLE name ≤ `ALIAS_MAX` = 64, and relative names. `peer_by_name` maps `<contact>/<export>` to the contact only when the suffix is `[a-z0-9_-]`, so `…/session:<uuid>` fails both today. The export becomes `/agents/<export>/rpc`. |
| Export lookup and share gate | `crates/a2a/src/server.rs:244-278`, `:378-427`, `:655-709`; `network.rs:1456-1511` | One router per export. `dispatch` admits a contact only to `principal.exports` (or `~return`), else 404; inbound copies of the principal header are stripped (`:381`). `Handler::admit` copies `self.export` into the `A2aPrincipal` (`:701-704`). `admit_principal` refuses any ACP-backed session (`:1475-1479`) and `chat_paused` (archived, opted out, `:1489-1504`). |
| First-contact hold | `server.rs:666-676`; `network.rs:1759-1769`, `:3725-3735` | Absent/`ask` holds the first task per (contact, export) until the owner answers. |
| Contact bounds | `server.rs:428-442`; `network.rs:1624-1653`, `:3474-3476` | 10 req/s, 8 handlers per contact; `CONTACT_QUEUED` = 8 waiting per (contact, export). |
| 191 ownership and forwarding | `server.rs:711-721`; `crates/agent/src/communication/network/full.rs:143-160`; `crates/agent/src/store/contacts.rs:498-509`; `crates/a2a/src/contacts/forward.rs:78-84`, `:127-228`, `:336-394`; `docs/design/191-notes.md:267` | `owner_strict` refuses `not owner` (-32030) before any task exists, naming the owner when it is ours; the receiving machine forwards (unary `SendMessage`/`GetTask`/`CancelTask`, two resolves, `carried` allows only `/agents/<[a-z0-9_-]>/rpc` or `~return`). An owner that is no machine of the mesh is plain `not owner`. |
| Delivery | `network.rs:1785-1799`; `full.rs:315-408`, `:413-452`; `crates/agent/src/supervisor.rs:2257-2265`, `:2289-2303` | `deliver_full` records the conversation inbound under `task.export` (`full.rs:378-380`) and enqueues unscoped (`isolated = false`). Ordinary peer intake refuses advisors and observed children, and an external harness only when `isolated`. |
| Context binds the session | `crates/agent/src/store/communication.rs:2273-2312`, `:2460-2483` | A context belongs to (principal, export) and names one session; a new task lands in the context's session. |
| Return path | `server.rs:279-297`, `:792-807`; `full.rs:197-311`, `:681-830` | Export-keyed throughout: a fresh send needs an offer from the card (`full.rs:263-309`); a reply on an inbound conversation needs the export still shared (`:239-250`); `full_return` closes on `share_closed(alias, export)` (`:726-728`). |
| Own-mesh policy | `communication.rs:107-114`, `:1426-1449`, `:1464-1471`; `crates/agent/src/store/communication.rs:1024-1027`; `crates/agent/src/store.rs:3262-3284` | `Policy::route` refuses advisors (hidden), needs both ends enabled (global ceiling, no self/ancestor opt-out) and, across trees, both roots `Owner`. Roots default to `Isolated`; the scope is on the durable row. `list_sessions` hides advisors. |
| discover_peers | `crates/agent/src/peers.rs:617-730`; `supervisor.rs:6879-6956`, `:7019-7171`, `:7227-7292`; `network.rs:878-942` | Own sessions pass `Policy::route`; foreign-owned rows are `remote` with no activity (`supervisor.rs:7098-7109`); titles only for the caller's own tree (`:7110-7113`); `send`/`reply` per `:7120-7124`. Contact rows (kind `a2a`, first tag `contact`) group and cap at 8; a contact's inbox sees no contacts (`network.rs:880-906`). The cursor is the last address (`supervisor.rs:6941-6951`). |
| Session kinds and pauses | `crates/agent/src/lib.rs:2963-2988`; `crates/agent/src/supervisor/pause.rs:264-273`; `supervisor.rs:2318-2322`; `crates/schedule/src/lib.rs:2123-2131` | Kinds: chat, advisor (hidden), observed (never messaged), ask. A paused tree takes no actor; a peer message waits for resume. Schedule roots are `Isolated`. |
| Contact record and grant | `crates/agent/migrations/0059_mesh_contacts.sql:1-19`; `store/contacts.rs:17-27`, `:40-59`, `:292-304`; `crates/a2a/src/contacts/records.rs:1030`, `:1072-1103`; `crates/a2a/src/config.rs:240-261`, `:280-304`, `:405-421`; `crates/a2a/src/lib.rs:935-937`, `:1509-1532` | `contacts` is a replicated record table. Staging makes a `Peer` (the grant) and `ContactFacts`. `same_grant` compares `Peer` whole; a changed grant gets a fresh epoch, the old one cancelled once the new configuration is live. |
| Inbox authority | `network.rs:3590-3604`; `crates/tools/src/registry.rs:1058-1080` | `clear_contact_limits` changes caps and marks the session an inbox; `contact_scoped` tests allowlist state. Neither is an authorization check. |
| Contact RPCs | `crates/a2a/src/contacts.rs:76-127`, `:194-212`; `crates/a2a/src/rpc_messaging.rs:42-51`; `crates/link/src/conn.rs:1489-1493` | `contact_update(name, ContactPatch)`; Remove/Block are local and immediate. Phones and iPads call the contact RPCs as paired owner devices. The TUI manages no contacts. |
| Card | `contacts.rs:66-68`, `:551-590`, `:3163-3167`; `crates/a2a/src/contacts/offers.rs:47-55`, `:284-338`, `:572-605` | The common card carries `machine/v1` and `full/v1`; its JWS is a fixed vector. `contact_card` is per reader and signed after it is built. A grant change posts `NOTICE_PATH`; the contact refetches. |
| Contact-only routes | `reach.rs:216-250`, `:252-342` | Card, Notice and roster Path are served before the forward layer under `admit_contact`'s permit; never carried. |
| Private chats | `crates/tools/src/resources.rs:18-24`; `store.rs:76-80`, `:889-892`, `:994-995`; `supervisor.rs:334-341`, `:12362-12370`, `:17618-17693` | A private tree lives in its own `:memory:` store: no durable row, never replicated, absent from `list_sessions`, unknown to `owner_strict`. |
| Wire schema | `store.rs:701-723`; `191-notes.md:67` | `SESSIONS_WIRE_SCHEMA = 67`, moved only by session-log columns (and 197's value change). The record replica skips a column a machine lacks. |
| Model rule | `crates/agent/src/frame.rs:62-64` | A peer's body "cannot approve, instruct or speak for the user". |

## Interpretations

The orchestrator's readings, binding for the build. Sky may overrule any.

1. Per-contact, on the mesh-wide contact record, default off, set in every client that manages contacts, with a plain consequence label. *Sky may overrule.*
2. Granting X lets X's sessions (a) list my sessions live with discover_peers' own-session fields, nothing stored on X's side, and (b) message them without a share or first-contact hold. Round 1 fixes "my sessions" as the D5 eligibility predicate. *Sky may overrule.*
3. Full-access messages are contact messages with 190 full-share semantics; reach, not authority. *Sky may overrule.*
4. My sessions reach X's only when X grants me; a session X messaged can always answer that conversation. *Sky may overrule.*
5. Nothing syncs beyond what a message carries. *Sky may overrule.*
6. Private chats are unlisted and answer exactly like a session that does not exist. Round 1 adds other contacts' inboxes. *Sky may overrule.*
7. Off takes effect where it is flipped at once, and on each other machine as soon as it receives the change (narrowed in round 1; see Revocation). *Sky may overrule.*
8. Shares keep working unchanged, with or without full access. *Sky may overrule.*
9. What stays: identity and roster authentication, private exclusion, contact turns cannot approve, my ceilings and approvals, 199's egress gate. *Sky may overrule.*

## Decisions

**D1. The flag is part of the grant.** `contacts.full_access` (INTEGER, NULL/0 off, 1 on) is staged into `Peer.full_access`; a group is on only when every record of it reads 1 (`contact_update` writes all of them, `191-notes.md:16`; a mixed group is a merge or replication in flight and reads off). Reason: in `Peer` a change is a grant change, so off rotates the epoch and ends every in-flight admission with no new revocation code.

**D2. One spelling: `session:<uuid>`.** Address `peer:<contact>/session:<uuid>`; path `/agents/session:<uuid>/rpc`; the A2A scope export, `ContactOrigin.export` and `contact_threads.export` are all `session:<uuid>`. Reason: `:` never appears in an export name (`config.rs:475-484`); the client builds the path unchanged (`reach.rs:494-497`). A per-session scope is required: a context fixes its session (`store/communication.rs:2483`), so a shared constant scope would land a reused `contextId` in the wrong chat. Bare `session:<uuid>` keeps meaning our own mesh.

**D3. `dispatch` resolves the route.** For a `ContactCaller` only, `dispatch` accepts `session:<uuid>`, removes any inbound `x-theuth-session`, rewrites to the one mounted `/agents/~session/rpc` handler (`Target::Session`) and sets the header to the UUID. A path naming `~session` directly, or a bearer (HTTP) principal on a `session:` path, is 404. `Target::Session` builds the principal with `export = "session:<uuid>"` for every method, never the mounted name. It serves `SendMessage`, `GetTask` and `CancelTask`; every other method is `UNSUPPORTED_OPERATION`.

**D4. Admission order.** On whichever machine receives the request: (1) the gate (a current, unblocked roster member of the contact; unchanged); (2) `ContactGrant.full_access`, else `full access is off`; (3) the row half of D5 on this machine's replicated row: not visible → plain `not owner`; (4) strict owner resolve: forwarded per 191 when another of our machines owns it, and the owner repeats (1)–(3); (5) on the owner, the live half of D5: not visible → plain `not owner`; for `SendMessage` only, visible but not sendable → `this session does not take messages`; (6) `admit_send` with `ask_first = false`. `GetTask` and `CancelTask` need `visible` and a task already accepted in the scope (`peer:<binding>`, `session:<uuid>`), never `sendable`: sendability governs new messages, not task control, so a task stays readable and cancellable after its tree is paused. Reason: a hidden session never reaches forwarding, so a private chat, another contact's inbox, an advisor and a random UUID all end in plain `not owner` on every machine, offline owners included.

**D5. One eligibility predicate.** `access::full_access_eligibility(session) -> Eligibility { visible, sendable }`. A full-access contact sees and reaches exactly what a session of an `Owner`-scope root on another of my own machines could see and message under own-mesh policy (`Policy::route`, `communication.rs:1428-1444`), with titles and live state, minus private chats and other contacts' inboxes. A durable row alone is not enough.
- **Row half** (any machine, from its replicated store): a durable row exists (private trees have none); `kind != advisor`; no session on its replicated ancestry (`Store::lineage` chain, `crates/agent/src/store/communication.rs:106-113`, `:1024-1027`) carries a `contact_inbox` marker or an opt-out (`sessions.communication = 0`); the root's scope is `Owner`.
- **Live half** (owner only): the row is strictly ours (`owner_strict`); lineage mirrored into the policy as `directory_from` does (`supervisor.rs:7030-7032`); the `to` half of `Policy::route` passes, through a new `Policy::reachable_from_owner(to)` (hidden, `enabled(to)`, root scope `Owner`). `enabled` walks every ancestor's opt-out (`Policy::blocker_of`, `communication.rs:1382-1402`), so an opted-out ancestor hides the whole subtree on both halves. No principal is invented: the contact never becomes an owner root.
- **sendable** = visible, and ordinary unscoped peer intake would accept it: not observed (`supervisor.rs:2298-2300`), not in a paused tree (`pause.rs:264-273`), and never refused for being an external harness, since the path is unscoped (`supervisor.rs:2301-2303`, `:2264`).
- One predicate serves the listing, direct-session admission (D4) and the full-access branch of 199's D17 notice re-check. The caller always stays a contact: provenance `peer:<contact>`, contact turns, the target's own scopes, caps, toolsets and inbox markers untouched. `clear_contact_limits`, `mark_a2a_inbox` and `contact_scoped` are never called or consulted for it.

| Session kind | visible | sendable | Note |
|---|---|---|---|
| Private chat | no | no | No durable row (structural). |
| Another contact's inbox (marker on it or its root) | no | no | Same answer as private (round 1 decision; Sky may overrule). |
| Advisor | no | no | Hidden from every route. |
| Root `Isolated` (the default) and its tree; opted out; global ceiling off | no | no | Own-mesh policy (`communication.rs:1409-1444`). |
| Chat, ask, agent desk in an `Owner` tree, and their native children | yes | yes | A desk is the real desk, not a per-contact inbox. |
| Observed child | yes | no | `send: false`, `reply: false` (`supervisor.rs:7120-7124`). |
| Paused tree | yes | no | Paused stays paused; nothing is queued for resume. |
| Routine/schedule run, meeting | per policy | per policy | Schedule roots are `Isolated` unless the owner promoted them; caps, pins and the meeting toolset are never altered. |
| ACP / external harness | per policy | yes | Unscoped intake accepts it; export admission's blanket refusal (`network.rs:1475-1479`) does not apply. `reply: false`. |
| Node child | per policy | per policy | `send`/`reply` per `supervisor.rs:7120-7124`. |
| Archived chat | per policy | per policy | Own-mesh policy does not check archive; the export-only `chat_paused` does not apply. |
| Session on a sandbox VM engine of my mesh | per policy | per policy | It is one of my machines; no class filter. |

**D6. Titles and live state, nothing more.** The listing fills titles for every visible row (Sky's ruling), unlike own-mesh discovery, which fills them only for the caller's own tree (`supervisor.rs:7110-7113`); tags are not sent. "No transcripts" means no transcript endpoint and no replication. A message can still ask the target session's agent to use its own tools, which is the existing 190 posture.

**D7. Listing: a contact-only route; each machine answers for its own rows; the contact fans out.** `GET /theuth/sessions` beside Card and Notice (never carried). Each machine lists the visible sessions it owns, filtered and sorted by itself (D8), with live state; the contact's engine asks every admitted member of our roster at once (`191-notes.md:73`) and merges. The request runs inside the admitted epoch: the handler takes the grant and the principal's epoch from one `Live` snapshot and races the answer against `epoch.cancelled()` as `dispatch` does (`server.rs:448-451`); a listing in flight when full access goes off ends `401` and never returns titles. `admit_contact`'s permit alone is not enough.

**D8. One global keyset cursor.** (Round 2 cut the per-member cursors and `CURSOR_MAX`.)
- Every member sorts its eligible rows by session id descending. Session ids are UUIDv7 (`crates/runtime/src/lib.rs:436-440`), so the key is stable and follows creation; never `last_active`, which moves.
- A request carries `{after: Option<session id>, limit, query, kind, active_only}`. The member applies the filters itself (Contract), so the caller never trims after the fact, and returns up to `limit` rows with `id < after`.
- The caller merges every answering member's rows, drops duplicate ids (an ownership move can make two answers overlap), sorts by id descending and emits the first `limit`. The cursor is the id of the last emitted row; it is `null` only when every answering member returned fewer than `limit` rows and nothing was trimmed.
- Why it is lossless, and when: for a fixed eligible and filtered set, each member's answer holds its own top `limit` rows below `after`, so the merged top `limit` is the top `limit` of the union; the next page is strictly below the last emitted id; ids are unique after dedup. This holds for every member that answers each page, and the cursor is one id whatever the roster size (256 members included). It is not a snapshot: a change of eligibility, of what the query matches or of activity (`active_only`) during a walk can move rows in or out, and UUIDv7 gives creation order per id, not a globally synchronised order across machines.
- `partial` lists the members that did not answer within 2 s, on every page, and reaches the model and the UI on every page through `PeerPage.partial` (Contract). It is reporting only: the next call asks every current roster member again with the same `after`. A member in `partial` on one page can have rows above the next cursor that the walk skips, and a page can end with a `null` cursor while a member timed out; repeating the same call retries it, and a fresh walk lists everything.
- Sessions created after the first page sort above it and appear on a fresh walk, not in later pages, as in any live list.
- Nothing is cached; every call is live within the 2 s deadline.

**D9. Delivery is 190's full path.** A full-access message is a contact task through `deliver_full`: mesh-peer wrapper, consumed receipt, the chat's own tools and the agent's ceiling, approvals to the owner, the frame rule (`frame.rs:62-64`). Rate, the 8-waiting bound (now per contact × session) and the chain limit stay.

**D10. Return path, both ways, keyed by `session:`.** `full_shape` gains a session branch before the offer lookup (`full.rs:263`): no offer needed; the outbound conversation is opened with export `session:<uuid>`. On an inbound `session:` conversation (`:239-250`) a reply needs MY grant still on. `full_return` (`:726-728`) uses the settled-card closure below for an outbound `session:` conversation. Reason: interpretation 4 with share semantics on revocation.
- **Settled-card closure** (extends `offers::settled`, `crates/a2a/src/contacts/offers.rs:110-125`): a contact is settled only when every machine of its current roster served a verified card at its last refresh, as today. Then `SettledCards.full_access` = some verified card's `full-access/v1` says `granted: true`. A verified card with no extension and a verified card with `granted: false` both count as verified "not granted". `full_return` closes a `session:` conversation exactly when the contact is settled and `full_access` is false. A machine never verified this run, or whose last refresh failed, leaves the contact unsettled: nothing closes, and the answering engine still decides (D12), as 186 D12 keeps for shares (`network.rs:983-994`).
- **Sender pre-check** (new sends, from the latest verified card of the machine read last): extension absent → `does not support full access yet`; `granted: false` → `has not given you full access`; no verified card this run → the message leaves and the card is asked for (`ContactOffersStale`), as unknown-card sends do today (`full.rs:263-276`); the owner's answer is mapped.

**D11. Advertised per reader.** `https://theuth.io/a2a/ext/full-access/v1`, `params: {"granted": bool}`, only on `contact_card`: present = supported, `granted` = this reader has it. The common card and its JWS vector do not change. `offers::changed` compares it, so a toggle notifies the contact.

**D12. The answering engine decides.** Every refusal that matters is made on the owner from its own records; cards only avoid a doomed send and word the error (as 186 D12, `network.rs:983-988`).

**D13. Clients.** Desktop: a Full access switch per contact in Access (`crates/desktop/ui/src/components/settings/Contacts.tsx:844-955`), confirm on turning on. Phones and iPads: the same UI through `contact_update`. TUI: no switch (it manages no contacts); `/peers` renders the rows unchanged.

## Contract

**Migration** `crates/agent/migrations/0068_contact_full_access.sql` (0067 is 199's `contact_files` on `theuth-199-build`; no `theuth-*` worktree has 0068; re-check before naming):

```sql
-- 200 CONTACT-FULL-ACCESS: a per-contact trust level on the mesh-wide contact record.
-- `contacts` rides the record replica (191), not the session log: SESSIONS_WIRE_SCHEMA stays 67.
-- NULL or 0 = off (fail closed), 1 = on. No constraint (record-replica rules, 0059).
ALTER TABLE contacts ADD COLUMN full_access INTEGER;
```

Record-replica only: `SESSIONS_WIRE_SCHEMA` stays 67. An older machine skips the column and has no session route: it never admits full access. `ContactRow.full_access: Option<i64>`, its SELECT, and `RecordTable::Contacts` columns gain it.

**Types and RPC.**
- `config::Peer.full_access: bool`; `server::ContactGrant.full_access: bool`; `PeerRoute.full_access` / `RemotePeer.full_access` (my grant, for D10).
- `ContactPatch.full_access: Option<bool>` (single-column `RecordWrite` on every record of the group, then `Ctx::commit_records`). No new method. Desktop `NO_PATCH` gains `full_access: null`.
- `Contact.full_access: bool`; `Contact.full_access_from: Option<bool>` (`None`: their card lacks the extension). Both `#[serde(default)]`.
- `ContactOffers.full_access: BTreeMap<String, bool>`; `SettledCards.full_access: bool`; `offers::Cached.full_access: Option<bool>`.
- New `crates/agent/src/communication/network/access.rs`: `SESSION_PREFIX = "session:"`, `SESSION_ROUTE = "~session"`, `session_route(&str) -> Option<SessionId>`, `Eligibility`, `full_access_eligibility` (row half `row_eligibility(&Store, SessionId)`), the listing answer, the D10 helpers. `Policy::reachable_from_owner` in `communication.rs`.
- New `crates/a2a/src/contacts/sessions.rs`: `EXT_FULL_ACCESS`, `SESSIONS_PATH = "/theuth/sessions"`, `SESSION_HEADER = "x-theuth-session"`, `PAGE_MAX = 200`, `PREVIEW = 8` (= `CONTACT_ROWS`, `supervisor.rs:7228`), `LIST_DEADLINE = 2 s`, `TITLE_MAX = 120`, the route, the fan-out and the merge.
- `PeerSender::list_sessions(&self, req: ListRequest) -> Result<ContactListing, String>`, default `Err("not a contact")`; `ListRequest { after: Option<SessionId>, limit: u16, query: Option<String>, kind: Option<DirectoryKind>, active_only: bool }`.
- `PeerPage.partial: Vec<String>` (`communication.rs:438-443`), `#[serde(default)]`, additive (TS optional): the machines that did not answer for this page, each `<contact>/<machine name>`, from previews and walks alike. `DiscoverPeersTool::call` (`peers.rs:673-727`) appends one line whenever it is not empty, on every page, empty and final pages included: `partial: no answer in time from <contact>/<machine>, …; their sessions may be missing from this page (repeat this call to retry)`. The desktop Peers panel and TUI `/peers` show the same line under the list.
- `PeerAddress::parse`: `peer:<contact>/session:<uuid>` with the contact ≤ 64 `[a-z0-9_-]` and the suffix a UUID (today's whole-name limit applies to every other form). `peer_by_name` accepts the same suffix. `forward::carried` accepts a `session:<uuid>` segment, never `~session`.

**Listing, one member** (`GET /theuth/sessions?limit=<1..200>[&after=<uuid>][&query=<text>][&kind=<k>][&active_only=1]`, under `admit_contact` and D7's epoch race):
- `403 full access is off`; `503` before `contacts_ready`; `400 bad request` for a malformed `after`, `limit` or `kind`. `after` is never looked up: any UUID is a key.
- The member keeps its visible rows that pass every filter, sorts them by session id descending, and returns the first `limit` with `id < after`.
- Filters, applied by the member so the caller never trims: `query` (lowercase substring) against the title, `session:<uuid>`, the backend kind and the machine name; `kind` with discover_peers' own `DirectoryKind::admits` (`communication.rs:550-559`) on the row as the caller renders it (kind `a2a`, first tag `contact`), so `contact` and `a2a` keep every row and any other kind keeps none (the caller then asks no member); `active_only` keeps rows whose activity is active (`PeerActivity::active`, `communication.rs:381-385`).

```json
{ "machine": "Sky's MacBook Pro",
  "sessions": [ { "session": "0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f", "title": "Port layout review",
      "kind": "native", "state": "running", "send": true, "reply": true,
      "activity": { "state": "running", "last_active": "2026-11-17T14:31:00Z", "turn_secs": 42,
                    "jobs": 0, "ptys": 0, "children": 1 } } ] }
```

`title` has control and bidi characters removed and is cut to 120. The fields are discover_peers' own-session fields (address from `session`, title, state, activity, kind, engine = `machine`) plus `send`/`reply`; nothing else.

**Listing, merged on the asking side** (`ContactListing`, from `PeerSender::list_sessions`): every current roster member is asked at once with the same request; answers within 2 s are merged by session id descending and the first `limit` emitted.

```json
{ "rows": [ { "machine": "Sky's MacBook Pro", "session": "0193f0a2-…", "title": "…", "kind": "native",
              "state": "running", "send": true, "reply": true, "activity": { "…": "…" } } ],
  "partial": ["Sky's Mac mini"],
  "cursor": "0193e811-02aa-7c6f-b1d2-8e4f5a6b7c80" }
```

- `cursor`: the id of the last emitted row, or `null` when every answering member returned fewer than `limit` rows and nothing was trimmed. `partial`: members that did not answer in time, by machine name (first 8 of the endpoint id when unnamed); reporting only.
- Query mapping on the asking side: a query equal to `<alias>` or `peer:<alias>` names the contact and sends no `query`; `<alias>/<rest>` or `peer:<alias>/<rest>` names it and sends `<rest>`; any other query is sent as is.

**discover_peers, default.** For each contact whose cards grant me, every member is asked `{after: null, limit: 8, query, kind, active_only}`; the first 8 merged rows are its preview, listed under its `peer:<contact>` row with its export rows and capped by `arrange` at 8 per contact as today. No cursor is produced for previews. The `peer:<contact>` row's `title` is:
- `full access` when the preview is the whole list (every answering member returned fewer than 8 rows, nothing trimmed, nothing hidden by `arrange`);
- otherwise `full access · more: discover_peers(kind="contact", query="<contact>")`, replacing `arrange`'s `+N more: …` text for that contact (N is unknown without walking);
- either followed by ` · did not answer: <machine>, <machine>` when `partial` is not empty.

**discover_peers, the walk.** Entered when the query names exactly one contact whose cards grant me (and `kind` is `contact` or absent), or when `cursor` is `full:<contact>:<uuid>` (then the query must still name that contact, else `unknown cursor`). Before either entry, the caller's own restrictions hold as for every listing: the calling session's communication is enabled (`directory_from`'s `enabled` check, `supervisor.rs:7033-7036`), and a contact's inbox root never sees the contact list (`network.rs:883`, `:906`), so from an inbox a `full:` cursor answers `unknown cursor` like any cursor it could not have been given. The member `limit` is discover_peers' `limit` (default 25, max 100).
- Page 1: a bounded prefix, then the merged page. The prefix is the `peer:<contact>` row (`title` `full access`, plus ` · did not answer: …` when partial) and its export rows as today, at most `1 + OFFERS_MAX` (65) rows (`crates/a2a/src/contacts/offers.rs:27`), not counted against `limit`. The merged session page is never trimmed to make room for the prefix.
- Later pages: the merged page only.
- The session rows are exactly the merged page, in id-descending order: no `arrange` re-sort, no grouping cap, no further filtering.
- `PeerPage.cursor` = `full:<contact>:<ContactListing.cursor>` or `null`.
- `kind="contact"` with no query, or a query naming no single contact, lists every contact and export row uncapped as today, plus each full-access contact's preview and title as in the default.

**discover_peers rows**: `address` `peer:<contact>/session:<uuid>`, `name` `<contact>/<title or first 8 of the id>` (cut to 40), `kind` `a2a`, `relationship` `remote`, `title`, `state`, `activity`, `send`, `reply` as answered, `tags` `["contact", "session", <their kind>]`, `engine` = their machine. A contact's inbox still sees no contacts. Description adds: "A contact that gave you full access lists its newest sessions under `peer:<contact>`; discover_peers(kind=\"contact\", query=\"<contact>\") walks all of them with `cursor`. Each is `peer:<contact>/session:<uuid>`; send_message reaches the ones with `send`."

**Messaging.** `send_message(to="peer:<contact>/session:<uuid>", …)`, files included. The sender checks its cached card (D10), then posts in 190's full shape. No new metadata.

**Wire events and UI.** No new event: `contact_update` ends in `A2aChanged`; the contact refetches our card on the notice. Labels:
- Off: "Full access is off. Maya reaches only the chats you share."
- On: "Full access: any of Maya's sessions can message your sessions and see their list (titles, running or idle). Private chats and your other contacts' conversations stay hidden. Nothing is synced."
- Confirm: "Give Maya full access?", the On text, "Cancel" / "Give full access".
- Their side: "Maya has given you full access: your sessions can list and message hers." · "Maya has not given you full access." · "Maya's theuth does not support full access yet."
- `SHARE_RULE` (`crates/desktop/ui/src/lib/contacts.ts:15-16`) gains: "…unless you give them full access."

**Refusal and error texts.**

| Case | Who | Text |
|---|---|---|
| Not granted | owner, `UNSUPPORTED_OPERATION` | `full access is off` |
| Not granted (card, or the server text) | sender | `peer:maya/session:<uuid>: maya has not given you full access; message a chat maya shares instead (discover_peers kind="contact")` |
| Their engine lacks it | sender, before sending | `peer:maya/session:<uuid>: maya's theuth does not support full access yet` |
| Not visible (private, another contact's inbox, advisor, isolated, unknown, foreign owner) | every member `not owner`; sender maps | `peer:maya/session:<uuid>: no such session` |
| Visible, not sendable | owner | `this session does not take messages` (sender: `peer:maya/session:<uuid>: this session does not take messages`) |
| Bad id | sender | `peer:maya/session:<x>: not a session id` |
| Visible session, owner machine offline | receiving machine (191) | `the chat's machine is offline` |
| Owner on an older build (404) | sender maps | `peer:maya/session:<uuid>: the machine that holds this session needs updating` |
| Too many waiting | owner (unchanged) | `busy: <n> tasks already wait here; retry later` |
| Listing not granted | `403` | `full access is off` (row title: `maya has not given you full access`) |
| My reply after I turned it off | my engine | ``peer `maya`: full access is off; nothing was sent`` |

## Revocation

- **Where it is flipped:** off writes `full_access = 0` on every record of the group and reconciles at once (`Ctx::commit_records`): the epoch rotates (`lib.rs:1509-1532`, cancelled at `:935-937`). Requests in flight end `401` (`server.rs:448-451`), listings in flight end `401` (D7), new listings `403`, new messages `full access is off`.
- **Other machines:** each applies it as soon as it receives the record change, normally within seconds (about a second in the 191 rigs, `191-notes.md:55`).
- **Limit:** a machine partitioned from the change keeps the old setting until it reconnects, as every 191 grant does (`191-notes.md:49`). Blocking or removing the contact on that machine cuts it off there at once (local and immediate, `rpc_messaging.rs:49-51`). Revocation is not instantaneous mesh-wide.
- **Tasks:** exactly what any grant change does today: a queued task fails when its epoch ends (`network.rs:1784`); full access never holds; a message already handed to the session is consumed and its turn finishes as an ordinary turn (186 D12).
- **Conversations:** my replies on full-access conversations are refused locally (D10). The contact's `~return` refuses its conversations into my sessions once my refreshed card says `granted: false` (the notice makes that quick; else within `OFFERS_TTL`).
- **Shares:** the rotation also ends that contact's in-flight share tasks, as any grant change does; shares keep working afterwards (interpretation 8).

## Interplay with 199

Files to any sendable session use 199 unchanged: a full-access message is a contact message through `deliver_full`, where 199 writes its file rows, and answers come through `full_return`. One change, in 199's D17 share re-check for `contact_file_notice`: the export grant, a live `~return` conversation, **or full access on for that contact and `full_access_eligibility(session).visible`** (the same predicate as listing and admission; a durable row alone is not enough).

A notice for a session whose tree is paused passes that re-check (it is `visible`) but must not start a turn while the tree is paused. The owner answers `accepted` and holds the notice for the tree exactly as `enqueue_peer` holds peer input for a paused tree (`supervisor.rs:2318-2322`, `:2348-2353`): no actor, no turn, no row. On `ResumeTree` the held notice is consumed like any automatic input, and only then committed (one row and one `input_accepted` step with `input_id = notice_id`). Until then `contact_file_notice_status` reads `unknown`, the holder re-sends on its backoff, and 199's commit-time dedup keeps exactly one row. Nothing else in 199 changes.

## Failure modes

| Cause | Asking side | Owner side | Notes |
|---|---|---|---|
| Not granted | Local refusal, or `full access is off` mapped | Refused at D4 (2) | Stale card: server refuses anyway |
| Hidden session (private, inbox, advisor, isolated, unknown) | `no such session` | Plain `not owner`, never forwarded | Identical bytes; offline owners included |
| Row owner no machine of the mesh | `no such session` | Plain `not owner` (191) | — |
| Visible session, owner offline | `the chat's machine is offline`; its machine in `partial` | — | Nothing queued |
| Member slow or unreachable during a listing | Its rows missing from that page; named in `partial` | — | 2 s; the next page asks it again with the same `after`, and rows of it above the cursor are skipped (restart the walk) |
| Sessions created during a walk | Not in later pages | — | They sort above page 1; a fresh walk shows them |
| Off during a listing | `401` | Epoch cancelled | No titles after the change |
| Partitioned machine at toggle | Old setting there until it reconnects | — | Block or remove there cuts at once |
| Mixed versions in my mesh | `needs updating` (404) | Old engine: no route, no column | Fail closed |
| Contact's engine without 200 | UI: "does not support full access yet" | My grant harmless | — |
| Merged group with mixed values | Off | Off | Re-toggle writes all records |
| Observed child, paused tree | Listed `send: false`; `does not take messages` | — | Pause untouched |
| Rate or waiting bound | `429`/`503` on listing; `busy` on send | Unchanged | — |
| Chat moves mid-message | `the chat moved; send again` | `HOLD_MOVED` (191) | Unchanged |

## Phases

Built on the merged 199 engine. Logic lives in the two new modules; shared files get one-line hooks in these functions only:
- **With 199 E1a** (`crates/a2a`): `reach.rs` `Route::of` and `contact_route` (a `Sessions` arm; E1a edits `serve`, before `serve_http`); `server.rs` `dispatch` (session branch, header strip), `Live::build` (mount `~session`, `ContactGrant.full_access`), `Target`, `Handler::admit` (canonical export), `Handler::send_message`/`get_task`/`cancel_task` routing (E1a edits `validate_send` and the body layer); card: `offers.rs` `Ctx::contact_card` and `offers::changed` (E1a edits the common `contacts.rs` `card()`); `forward.rs` `carried`. 200 does not touch `lib.rs` (E1a's `transfers.cut`).
- **With 199 E1b** (`crates/agent`): `full.rs` `full_shape` (session branch, D10) and `full_return` (one `share_closed` call replaced by `access::conversation_closed`), on top of E1b's `full_return`; `deliver_full` unchanged (E1b's); `rpc_messaging.rs` `contact_file_notice`'s share re-check (the D17 branch through `access::full_access_eligibility`); `peers.rs` `DiscoverPeersTool::description` and `DiscoverPeersTool::call` (the `partial` line), never `SendMessageTool` (E1b's).
- **200 alone:** `network.rs` `peer_by_name`, `remote_peers`, `admit_principal` (session branch: binding and epoch as today, then D5 instead of the native and `chat_paused` checks), the remote error mapping; `communication.rs` `PeerAddress::parse`, `Policy::reachable_from_owner`, `PeerPage.partial`; `supervisor.rs` `ListPeers`/`PeerRead::fetch`/`directory_from` (previews, the walk, the `full:` cursor, `partial`) and `arrange` (the full-access title replaces `+N more`); `offers.rs` `settled` (the closure); `config.rs`, `records.rs`, `store/contacts.rs`, `contacts.rs` `ContactPatch`/`Contact`/`contact_update`; the desktop Peers panel and TUI `/peers` (the `partial` line).

| Phase | Gate | Tests (minimum) |
|---|---|---|
| **P0 contract** | Migration 0068 and the column; every type/field above; constants and wire types; `PeerSender::list_sessions` default; `Eligibility` and the predicate's signature; TS bindings; OpenRPC baseline accepted with Breaking 0. | Group staging: one record 0/NULL → off, all 1 → on; toggling changes `same_grant`. `SESSIONS_WIRE_SCHEMA` snapshot unchanged. `session_route` refuses non-UUIDs and export names; `PeerAddress::parse` takes a 64-char contact plus the suffix. The member request parser: any UUID as `after`, garbage → `400 bad request`. The merge function alone (pure): merged order, cursor and `null` rules. |
| **P1 engine** | Staging and `contact_update`; card; `dispatch` + `Target::Session` + D4; D5 with the matrix; listing route under the epoch, fan-out, member filters and the global cursor; discover default and walk; `PeerPage.partial` and its line; sender path and error mapping; D10 with the closure; 199 D17 branch with the paused hold. | Unit, the matrix row by row through `full_access_eligibility`, plus an opted-out grandparent hiding its grandchild on both halves. **Builder checks:** a forged `x-theuth-session` with a share path, a direct `/agents/~session/rpc`, and an HTTP bearer on a `session:` path are refused; `SendMessage`, `GetTask` and `CancelTask` all carry `export = "session:<uuid>"`, never `~session`; after the tree is paused a new `SendMessage` reads `this session does not take messages` while `GetTask` and `CancelTask` on the already-accepted task still work; the `full_shape` branch (send, reply on inbound, refused after my off) and the `full_return` branch; the closure: settled with a verified card lacking the extension → closed, settled with `granted: false` → closed, one member never verified or its refresh failed → not closed; Y without full access keeps its ordinary share working while Y's direct and list requests fail; an endpoint R in X's roster gets X's grant only (never Y's; none when 191 attribution admits R for no contact); **indistinguishability**: a private chat, an excluded inbox, an advisor, a session whose root is `Isolated`, a row owned by a machine no longer in the mesh, an excluded inbox on an offline owner, and a random UUID give byte-identical answers; `after` set to any of their ids pages exactly like a random id. **Listing:** three members with `limit = 1`, walked to the end: every eligible row exactly once, none lost or duplicated; default discovery shows the preview of 8 plus `full access · more: discover_peers(kind="contact", query="maya")`, and no cursor; `kind="contact"` with `query="maya/port"` and `active_only` walked to the end equals the filtered set, nothing trimmed by the caller; a final page where the members return more rows than remain in the budget (5 left across members, `limit = 3`) → 3 rows and a cursor, then 2 rows and `null`; 300 simulated members with one offline → the offline one in `partial`, asked again with the same `after` on the next call, the cursor still one id; **the later-page timeout:** A holds [100, 80] and B [90, 70] (ids as sort keys), `limit = 2`: page 1 → [100, 90], cursor 90; page 2 with B timing out → [80], cursor `null`, `PeerPage.partial = ["maya/B"]` and the `partial:` line in the tool text; repeating page 2 with B answering → [80, 70], `null`, no `partial`; an empty page with a timed-out member still carries the line; two members answering the same id (an ownership move) → it appears once; from a contact's inbox root a `full:` cursor answers `unknown cursor`; a walk's page 1 with 64 export rows still carries `limit` session rows. **199 D17:** a notice for a paused tree → `accepted`, no turn, no row, status `unknown`; resend → still one pending; `ResumeTree` → exactly one row, one turn. Two-engine e2e: B lists A's and A2's visible sessions with titles and live state; messages an unshared chat with no hold; A answers with `reply_to`; A without B's grant is refused; off mid-listing → `401` and no titles; a reused context id lands in each session's own context; ACP session sendable, observed child `send: false`, paused tree `send: false` and still paused. |
| **P2 UI** | Desktop switch with confirm and labels; their-side line; `NO_PATCH`; `SHARE_RULE`; phone/iPad at 390 px with 44 px targets; Peers rows without Open; site docs (`site/content/docs/automation/mesh.mdx`). | Vitest: on confirms and sends `full_access: true`; off sends `false` without confirm; label per state; `full_access_from` `None`/false/true. Fixture screenshots. TUI: a contact session row through `PeerRow::line`. |

## Verification (the closer's live proof)

Two scratch engines A and B paired as contacts over real iroh (relay none, fake models, scratch data dirs); A2 in A's mesh. A log or screenshot for each step.

1. Before any grant: B sees A's exports only; `send_message(peer:a/session:<A chat>)` reads `a has not given you full access…`.
2. A turns full access on for B (confirm shown). B's `discover_peers` lists A's and A2's visible sessions under `peer:a` with titles and live state; A's private chat, an inbox of another contact C, an advisor and an `Isolated` schedule root are absent.
3. B messages an unshared A chat and an A2 chat: both arrive as `<message from="peer:b" …>` with no Allow strip; A answers with `reply_to`; B's session receives it. B sends a file (199) to the A2 chat: text first, then the notice, on A2.
4. B sends to A's private chat id, C's inbox id and a random UUID, with A2 stopped for a fourth try at an inbox on A2: all four `no such session`.
5. With 150 visible sessions across A and A2 and A2 frozen: `discover_peers(kind="contact", query="a", limit=100)` returns A's newest 100 eligible rows with `did not answer: A2` and a cursor. A2 thawed, following the cursor to `null` lists the remaining rows of both below that cursor, each once; A2's rows above it are absent, as D8 states. A fresh walk lists all 150, each once.
6. A's sessions try B's: refused. B grants A: A lists and messages B's sessions.
7. A turns full access off during a slow listing: it ends `401`; B's next send reads `has not given you full access`; the running turn finishes; A's reply on that conversation is refused locally; A's shares to B still work. With A2 partitioned during the toggle, A2 keeps admitting B until it reconnects; blocking B on A2 cuts B off there at once.
8. Captures: desktop Access row on and off, the confirm, Peers with B's sessions, a received message, the refusal, TUI `/peers`.
