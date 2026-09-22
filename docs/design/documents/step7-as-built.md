# 59 — As built, step 7: sharing (links, members, guest connections, `/d`, revocation) — 2026-09-22

Branch `feature/documents-sharing` off `feature/documents` 3005e67. Implements §8.1–8.8 with the §0 decisions 9–13, 16 (no content key, no `doc_share` tool, one relay route, mode-2 conns). Everything here overrides §8 where they differ.

## 1. Built

### document (`crates/document`)
- `meta.rs`: `Role{editor,viewer}`, `Member{pub, name, color, role, added_ms, via, last_seen_ms}`, `PALETTE` (12 colours), `ShareLink{id "lnk_…", role, secret_hash (never serialised on the wire), name, created_ms, expires_ms, revoked_ms}`, `DocMeta.members/links` filled in (were placeholders from step 1), `Principal::Member{pub, device}`, `Who::Member{pub, name}`, `DocEvent::Members{members, links}`, `DocEvent::Closed{reason}` gains `demoted` and `revoked`, `Members`, `Joined{role,name,color}`, `LinkCreated{url, link}` (ts-rs exported).
- `share.rs` (new): proof + membership logic. `admit(doc, by, write)` is the one gate every member-callable message goes through (`OpenDoc`, `Subscribe`, `ApplyUpdate`, `AwarenessUpdateMsg`, `SetName`) → wire errors `not_member`, `viewer_readonly`, `sharing_disabled`. Messages `LinkCreate → (public ShareLink, secret)`, `LinkRevoke`, `Join → Joined | JoinError{link_unknown, link_revoked, link_expired, bad_proof, members_full, sharing_off}`, `MemberSet` (editor→viewer sends `Closed{demoted}` to that member's subscriptions), `MemberRevoke` (`Closed{revoked}`), `SetName`, `MembersOf`. Every membership change writes meta.json and fans `Members` out to every subscriber of the document (owner's list is live).
- `store.rs`: update-log tag 4 = member-authored update, followed by the 32-byte static (attribution for step 8).
- `actor.rs`: `OpenDoc/Subscribe/ApplyUpdate/AwarenessUpdateMsg` carry `by: Principal`; `OpenDoc` bumps `last_seen_ms` and strips `links` for members; `Who::Member.name` is filled from `meta.members` when an update is applied.
- `DocumentsConfig.max_members` (64) and `DocumentsConfig.sharing` = `"on"` | `"off"` (no new links; members keep working) | `"disabled"` (members refused with `sharing_disabled`).

### link (`crates/link`) — mode 2
- `conn.rs`: an unknown static that completes `Noise_IK` no longer gets `CLOSE 3`; `respond()` returns `Handshake::Guest{…, handshake_hash}` (the final `h`, equal to the browser's `Session.handshakeHash`). `run()` takes a token from `State.guests` (`Bucket`, `LinkConfig.guest_handshakes_per_minute` = 10); over budget → `CLOSE 3` **before message 2** (the DH already happened, as §8.3 concedes). Otherwise message 2 is sent and `run_guest()` runs the conn.
- `run_guest()`: JSON-RPC like mode 0 but through `guest_gate()`: methods outside `GUEST_METHODS` = {`doc_join`, `doc_open`, `doc_subscribe`, `doc_unsubscribe`, `doc_update`, `doc_awareness`, `doc_set_name`} answer `-32601` without reaching the dispatcher. The conn is `CLOSE 3`d when `!guest.joined()` 5 s after IK (`JOIN_DEADLINE`) or after 3 membership refusals (`MAX_REFUSALS`).
- `types.rs`: `Guest{key, handshake_hash, joined, refusals}` hangs off `RpcPeer.guest`; the rpc layer reads it. `DropGuest{key}` message cancels every conn of that static (used by `doc_member_revoke`). `ConnHandle.device = Some(key)` for guest conns so the drop finds them.

### relay (`crates/relay`)
- No code change: the `--web` fallback already serves `index.html` at `/d` and `/d/` with the browser-container CSP. Test `web::d_serves_the_bundle_with_csp` pins it. Verified (see §4) the relay logs one line at start and nothing per request — the fragment cannot appear in a log.
- Settings › Link gained the §8.7 disclosure sentence.

### rpc (`crates/rpc/src/api.rs`)
- Owner: `doc_members(doc) → Members{members, links}`, `doc_member_set(doc, member, role?, name?)`, `doc_member_revoke(doc, member)` (also `link::DropGuest`), `doc_link_create(doc, role, name?, expires_ms?) → LinkCreated{url, link}` (URL built from `link::Status`: relay `ws→http`/`wss→https`, `/d#1.<harness_id>.<doc>.<secret>`; refused when link is off), `doc_link_revoke(doc, link)`.
- Member: `doc_join(doc, proof, name?) → Joined` (guest conns only), `doc_set_name(doc, name)`. `doc_open/doc_subscribe/doc_update/doc_awareness` accept guests via `principal()` → `Principal::Member`.
- Helpers `principal()`, `owner()` (refuses guests with `not_owner`), `member_outcome()` (any successful member call marks the guest peer joined; `not_member`/`viewer_readonly`/`sharing_disabled` count as refusals).

### Desktop UI (`crates/desktop/ui`)
- `components/share/ShareDialog.tsx`: "Share…" action on `DocumentPage` (notes and knowledge — the only `DocumentPage` users). Create link (role, expiry 1d/7d/30d/never, preset name) → URL shown once with QR (`qrcode.react`, as pairing) and Copy; links list with Revoke; members list (colour dot, name, "via link <name>", last seen, role select, Remove). Live via the provider's `Members` event.
- `components/share/ParticipantStrip.tsx`: colour dots + names from awareness (owner devices and members) in the DocumentPage header.
- `components/share/GuestApp.tsx` + `link/guest.ts`: the `/d` guest container. `main.tsx` renders `GuestApp` when `isGuest()` (pathname `/d`). Parses the fragment, uses the browser's existing device static (`getIdentity()`, IndexedDB), asks for a name once (stored in IndexedDB `identity/guest-name`), connects `LinkTransport` to the relay, on open calls `doc_join(doc, HMAC(…), name)`, then runs `HarnessProvider` exactly as a device does with the awareness user pinned to the joined name/colour. Viewer → `editable: false` + "read only". `Closed{demoted}` → toast and reopen read-only; `Closed{revoked}`/`deleted`/link errors/`CLOSE 3` → terminal screen ("Your access was removed." / "This link no longer works."). Footer "Get theuth". `Editor.tsx` untouched.
- `lib/doc/provider.ts`: optional `guest` argument; status `readonly` for viewers; `link/transport.ts` exposes `handshakeHash` and `onClose(reason)`.
- `scripts/doc-guest-smoke.ts`: protocol smoke from bun (no browser): bad proof, method gate, 5 s `CLOSE 3`, join, role enforcement, rate limit.

## 2. Mode-2 state machine (as implemented)

```
msg1 (IK) ──known static──▶ mode 0/1 (unchanged)
   │ unknown static, IK ok
   ▼
bucket.take() ── empty ──▶ CLOSE 3 (no msg2)
   │ token
   ▼
msg2 sent, Guest{joined=false} ── 5 s, !joined ──▶ CLOSE 3
   │ any request
   ▼
guest_gate: method ∉ GUEST_METHODS ──▶ -32601 (conn stays)
   │ dispatch to rpc → document::admit / Join
   ├─ ok ──▶ joined=true (deadline disarmed)
   └─ not_member | viewer_readonly | sharing_disabled | Join error ──▶ refusals++ ; 3 ──▶ CLOSE 3 after the response
doc_member_revoke ──▶ DropGuest{key} cancels the conn; Closed{revoked} to its subscriptions
```
No scope set on the conn: `document` checks membership on every call, so a member of several documents of one harness needs one conn. A returning static needs no join — its first `doc_open` succeeds and marks it joined (§8.3's `Membership{pub}` lookup is not built). A revoked static gets `CLOSE 3` ≤5 s after IK (after its first refused call, or the deadline), not before message 2.

## 3. Deviations
1. **Proof.** `proof = HMAC-SHA256(key = sha256(secret), handshake_hash ‖ guest_pub)`; the harness stores only `sha256(secret)` (b64url in meta.json) and never the secret. The brief allowed this rewording of §8.2/8.3; the security argument is unchanged (the hash binds the proof to this handshake and static; the secret itself only ever lives in the fragment).
2. **`doc_join(doc, proof, name?)` carries no link id** — the §8.2 URL has none. The proof is checked against every link of the document (constant-time per link); with ≤ a handful of links that is free.
3. **No conn scope / no `Membership` message** (see §2): design B — the link crate knows nothing about documents; membership lives only in `document`. Consequence: a revoked static is refused on its first call and closed within 5 s rather than before message 2.
4. **`doc_members` returns `{members, links}`** (the owner dialog needs both); `DocEvent::Members{members, links}` goes to every subscriber, members included (links have no secrets on the wire).
5. **Join is idempotent** for an existing member and ignores the link then; the guest page therefore always calls `doc_join` on connect. Name precedence: link preset > given name > "Guest". Colour = `PALETTE[len % 12]`.
6. **Removing a member does not blacklist the static**: while its link is live, reloading re-joins as a fresh member (seen in the smoke). Revoke the link to stop that — matches §8.4 ("revoking a link stops new joins; members admitted through it stay until revoked individually").
7. **Rate limit lives in `link.guest_handshakes_per_minute`** (the handshake is what costs the DH), not in `documents`.
8. **No `routes/d.tsx`**: `main.tsx` branches on `isGuest()` before the router mounts, like `isBrowserContainer()`. Guest identity = the browser's device static (`link/keys.ts`); no separate key.
9. **Name prompt is always shown**, even for preset-name links: the client cannot know the preset (URL has no name); the server applies the preset anyway.
10. **`doc_read` attribution `[member <name>]` not done**: no Rust attribution map exists (the helper's `markdown` has an `authors` slot nobody feeds). The store now records the author static per update (tag 4) so step 8 can build it.
11. `Editor.tsx` untouched: the awareness `user` is pinned by a `PinnedUserAwareness` subclass in the provider; viewer read-only rides on provider status `readonly`.
12. Files outside the allow-list: `crates/desktop/ui/src/rpc/api.ts` (METHODS + Api types; in ae20039), `crates/desktop/ui/scripts/doc-guest-smoke.ts` (test script), `Cargo.toml` (`hmac = "0.12"` workspace dep).
13. Step 6's MobileToolbar is not in this tree: coarse-pointer guests get the plain editor.
14. After a demote the old awareness state lingers up to the awareness timeout (~30 s), so the strip briefly shows the guest twice.

## 4. Checks

| Check | Result |
|---|---|
| `cargo test -p document` | 18 unit + 4 integration green (`sharing_links_join_roles_demote_revoke`: join, bad proof, expired, 65th refused, viewer update refused, demote → `Closed{demoted}`, revoke → membership empty) |
| `cargo test -p link` / `-p rpc` / `-p relay`; `cargo check --workspace`; helper `bun test` | 16 / 17 / 7 green; workspace check clean (3m22s); 77 helper tests green |
| UI `tsc -b`, vitest, `vite build` | green; 38 files / 209 tests (incl. `link/guest.test.ts`); build 35 s |
| Smoke env | `theuth-relay --listen 127.0.0.1:8461 --web dist`, `t --headless` (rpc 7833, link on, relay ws://127.0.0.1:8461), Vite 1461, omp headless Chromium; screenshots `/tmp/theuth-59-smoke7/01–09*.png` |
| Owner creates editor link (dialog) | 145 ms to URL + QR (`01-owner-share-dialog.png`) |
| Guest A: fresh browser context opens URL on the relay | prompt → name → `doc_join` → editor: 64 ms from Continue to joined; role `editor` (`02`) |
| A types → owner sees text, A's colour and cursor label, participant strip "Chrome (you) · Guest A" | 177 ms incl. typing (`03`, `04`) |
| Guest B via viewer link (preset name) in a second fresh context | joined 53 ms, role `viewer`, `contenteditable=false`, sees A's typing live in 88 ms (`05`) |
| Script: wrong proof / `doc_list` from a guest / unjoined 5 s | `bad_proof`; `-32601 Method not found`; `CLOSE 3` at 5.5 s |
| Script: viewer `doc_update` / `doc_set_name` | `viewer_readonly` both; joined conn still open at 11 s |
| Script: 2 + 11 guest handshakes inside a minute | 9 accepted, 2 `CLOSE 3` with no message 2 (bucket refills 10/min); harness log "guest handshake over the per-minute budget" |
| Owner demotes A (role select) | A's page `data-guest-role=viewer`, editor read-only, still live (owner's text visible in 149 ms) (`06`) |
| Owner removes A | "Your access was removed." after 32 ms; owner's member list without A within the same render (`07`, `08`) |
| A reloads with link still live | re-joins as a new member (deviation 6) |
| Link revoked + A removed, A reloads | "This link no longer works." after 60 ms; conn closed by the page (`09`) |
| Relay learns | stdout has only the startup line; no per-request log exists in `relay/src`; `/d` request line has no fragment (browsers never send it) |
| Return visit | A reloads while still a (demoted) member: no prompt (name in IndexedDB), back to the read-only live view within the 7 s wait window, member list unchanged |

## 5. Left
- Step 8: untrusted-text labelling for member text (`[member <name>]` in `doc_read`; the tag-4 author records are the input), member attribution in the outline.
- `Membership{pub}` return-visit lookup / pre-message-2 refusal of revoked statics (deviation 3) if the 5 s window matters.
- Client-side hint of the preset name (URL v2 or a `doc_link_info` call) to skip the prompt.
- Physical-device check of `/d` (phone browser, QR scan) — Sky's.
