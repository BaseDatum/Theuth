# 213 — Settings › Sharing hub

Status: contract (build on `feat/213-sharing-hub`, worktree `../theuth-213-sharing`). Recon: settings UI scout and backend scout, 2026-11-23, at `main` 2512bd899.

## 1. What and why

Owner, 2026-11-23: Settings needs one central place to manage the **items** that use the network features: shared notes, published sites/apps on a domain, sessions/agents shared with contacts, the contacts themselves, and public shares over the edge. **Network** stays the place to switch features on or off and enable access (mesh, pairing, devices, relay, messaging/A2A listener, VMs). **Sharing** is for the things being shared.

Design 210 already registered a `sharing` category with one tab ("Public sharing", `ui/src/components/settings/Sharing.tsx`). This design grows that category into the hub and moves the Contacts tab out of Network.

Non-goals: Source::Artifact public shares (210 D9, P2); a native iOS settings UI (iOS is the same Tauri bundle); a settings search index.

## 2. Settings layout (UI)

Category `sharing` (`ui/src/components/settings/categories/sharing.ts`), tabs in this order (6 = the registry maximum):

| # | tab id | label | content | source |
|---|---|---|---|---|
| 1 | `shared` | Overview | counts per kind + last changes; each count links to its tab; one line linking to Network for the switches | new, reads `sharing_list` |
| 2 | `contacts` | Contacts | the existing `ContactsTab` unchanged (list, Add contact on desktop, waiting first messages, Access, Full access) | **moved** from `categories/network.tsx`; id kept |
| 3 | `shared-chats` | Chats and agents | flat list of exports (`a2a_exports`) with the contacts granted each; Pause/Resume, Stop sharing with X, Remove export | new; reuse `useExports`, `useContacts`, `useGrant`, `useUngrant` from `Contacts.tsx` / `lib/contacts.ts` |
| 4 | `shared-notes` | Notes and notebooks | documents with members or links; per row: members (role, last seen, via link), links (role, name, created, expires, state); Remove member, Revoke link, Revoke link and remove members who joined through it; Open document | new, reads `sharing_list{kinds:[document_member,document_link]}` |
| 5 | `sharing` | Public shares | today's `SharingTab` minus the names/machines sections: switch, live shares (sites and apps on the domain) with Stop / Stop all, history, defaults, terms, disclosures | existing, trimmed; **id kept** (`PublicShareStrip` links to `/settings/sharing`) |
| 6 | `domains` | Domains | `NameSection` (held names), `MachinesSection`, edge choice, certificate/clock warnings | split out of `Sharing.tsx` |

Rules:
- `TAB_IDS` in `registry.ts` follows the new category order; `registry.test.ts`, `Network.test.ts`, `Contacts.test.ts`, `Sharing.test.ts`, `route-target.test.ts` updated accordingly.
- The first shown tab of the category is `shared` (Overview). The Overview is gated `required:{methods:['sharing_list']}`; when `sharing_list` is unavailable (older engine) it is hidden and Contacts becomes the first tab.
- `contacts` keeps its existing `required` literal and `?focus=access:<name>` deep link; `toContacts()` keeps working. No `MOVED_TABS` entry is needed (id unchanged).
- Network category keeps `machine`, `machines`, `devices`, `messaging`, `vms` (5 tabs).
- Cross-links: Communication (A2A) tab gets "Contacts and what they may reach → Settings › Sharing › Contacts"; Sharing Overview gets "Turn features on or off → Settings › Network".
- Revoke/stop actions are never behind a confirmation dialog beyond what exists today (stop is ungated in 210); destructive Remove of a contact keeps its existing confirm.
- Rows that the backend reports in `partial` show a one-line muted note ("Contacts are off — turn on agent messaging in Network") instead of an error.
- No emoji; register any new disclosures in `components/disclosure.test.ts`.
- Live refresh: refetch `sharing_list` on `share.changed@1`, `a2a.changed@1`, `document.changed@1`, and the new `document.sharing@1` (§4).

## 3. Backend contract (RPC)

Both new methods live in `crates/rpc-compose/src/api.rs` (`CompositionApi`, next to `doc_resolve`), owner-only (`with_extensions` + `rpc_core::owner(ext)?`), audience owner, and **must not** be added to `GUEST_METHODS` (add a refusal test next to `guest_gate_admits_member_methods_only`, `crates/link/src/conn.rs`).

```rust
#[method(name = "sharing_list", with_extensions)]
async fn sharing_list(&self, kinds: Option<Vec<SharingKind>>) -> RpcResult<SharingList>;

#[method(name = "sharing_revoke", with_extensions)]
async fn sharing_revoke(&self, kind: SharingKind, id: String, sub: Option<String>) -> RpcResult<SharingRevoked>;
```

```rust
#[serde(rename_all = "snake_case")]
pub enum SharingKind { DocumentMember, DocumentLink, PublicShare, Export, Contact }

pub struct SharingRow {
  pub kind: SharingKind,
  pub id: String,               // see id grammar below
  pub title: String,            // doc title | host+path | export name | contact name
  pub audience: String,         // human text: member name | "anyone with the link" | "public" | "password" | contact names
  pub permission: Option<String>, // doc role | share access | export first-contact policy | contact first_contact
  pub state: String,            // live|pending|offline|stopping|stopped | active|expired|revoked | paused | blocked
  pub created_ms: Option<i64>,
  pub expires_ms: Option<i64>,
  pub last_access_ms: Option<i64>, // best effort; None when unknown
  pub count: Option<u64>,       // members joined via a link | visitors | contacts granted an export | exports a contact may reach
  pub url: Option<String>,      // public share URL; None for doc links (secret shown only at creation)
  pub engine: Option<String>,   // hosting machine name when known
  pub parent: Option<String>,   // doc id for document_* rows; export name for grants
  pub revocable: bool,
  pub open: Option<String>,     // what the UI opens: doc id | session id | agent name | contact name
}
pub struct SharingList { pub rows: Vec<SharingRow>, pub partial: Vec<SharingPartial> }
pub struct SharingPartial { pub kind: SharingKind, pub reason: String } // plugin off / not running / error text
pub struct SharingRevoked { pub kind: SharingKind, pub id: String, pub pending: bool } // pending = edge has not confirmed (share_stop edge_pending)
```

Id grammar: `document_member` = `<doc>|<member pub>`, `document_link` = `<doc>|<link id>`, `public_share` = share id, `export` = export name, `contact` = contact name.

Revoke dispatch (call the existing handler bodies, not the wire, so forwarding/consent are unchanged; revoke is never consent-gated):

| kind | sub | calls |
|---|---|---|
| document_member | – | `doc_member_revoke(doc, member)` (forwarded to the doc's server engine as today) |
| document_link | none | `doc_link_revoke(doc, link)` |
| document_link | `"members"` | `doc_link_revoke` then `doc_member_revoke` for every member whose `via` is that link |
| public_share | – | `share_stop(id)`; `pending` from `Stopped.edge_pending` |
| export | none | `a2a_export_remove(name)` |
| export | `<contact>` | `contact_update(contact, revoke:[export])` — stop sharing this export with one contact |
| contact | – | `contact_remove(name, block=false)` |

Listing:
- Documents: every non-deleted doc with members or live links. Prefer an index column (`links_n`, kept like `members_n` on meta write) over scanning `meta.json`; a scan of the docs dir is acceptable for the first cut if a migration is disproportionate — say which in the report. Expired links get state `expired`, revoked ones `revoked` (kept 30 days, then omitted). Rows from mirrored meta on a non-serving machine are fine (eventually consistent).
- Public shares: `share_list` (mesh-wide, 30-day history); traffic `last_ms`/`visitors` when this engine hosts it.
- Exports: `a2a_exports` + contacts' grants; `count` = contacts granted.
- Contacts: `contact_list`; `count` = exports it may reach; `state` blocked/active.
- A kind whose plugin is off or not running → `partial`, not an error.

## 4. Event

New wire event in the document crate: `document.sharing@1` `{ doc: String }`, emitted from `members_changed` (`crates/document/src/share.rs`) whenever members or links change (create, revoke, join, role change). Registered in the document plugin's `declare_wire`. No new aggregate topic; the UI refetches on the four topics in §2.

## 5. Pinned artifacts

New methods/types change `crates/theuth/tests/fixtures/wire/openrpc-v1.json` — accept with `just wire-schema-accept` in the backend commit; regenerate TS bindings (`just ts-desktop`) so `ui/src/rpc/bindings/client.ts` has `sharing_list`, `sharing_revoke`, `SharingRow`, `DocumentSharing` event.

## 6. Ownership (two builders, one worktree)

- **Backend**: `crates/rpc-compose`, `crates/document`, `crates/agent`, `crates/link` (guest test only), `crates/share` and `crates/a2a` only if a public helper is needed, `crates/theuth/tests/fixtures`, generated `bindings/` and `crates/desktop/ui/src/rpc/bindings/`.
- **UI**: `crates/desktop/ui/src` except `rpc/bindings/`.
