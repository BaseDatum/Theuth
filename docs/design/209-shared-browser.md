# 209 — One browser profile for the user's web view and the agent's browser

Status: design 2026-11-22, approved to build (three builders per phase in one worktree, as 205); this is the build contract · extends 205 (web view tile, incl. §9 iOS shell on `feat/205-ios-web-view`), 23
(obscura browser), 122 §5 (cookie rule), 67 §5.1 (replica), 37/67 §5.4 (vault), 68/183 (front door, event relay)

Evidence: two reports, both read in full. Code recon, session `01a2007d-94f1-7532-add0-2b2aab26d032` (cited
"recon §n"); web research, session `01a2007d-94f4-7378-aca8-91538bf9e4e4` (cited "web §n" with its URLs). Every
`path:line` below without "(recon)" was opened for this document on `main` or on the worktree
`theuth-ios-webview` (written `wt:`); §12 lists what was not.

## 1. Context and problem

Two browsers live in theuth and share nothing. The agent's browser is obscura, one isolate per session on its own
thread; "the isolate *is* the profile: `BrowserState` holds the cookie jar and storage in memory and obscura writes
nothing to disk" (`crates/browser/src/session.rs:9-11`, `:121`). The user's browser is the web view tile of 205: a
native `WKWebView` / WebView2 on the user's device with its own persistent data store
(`crates/desktop/src/webview.rs:66-71`, `:319-327`; iOS `wt:crates/desktop/src/ios_webview.rs:655-659`). The prompt
says so to the model: "The two share nothing … a sign-in the user completes in the tile does not sign your browser
in" (`crates/artifact/src/tools.rs:60`).

The owner's requirements (2026-11-21):

- R1. Cookies shared between the tile and obscura.
- R2. The jar synced across the nodes of a mesh: the user signs into a site in the tile on an iPad, and an agent
  on any engine continues that session in obscura.
- R3. The agent has full control of the tile too, whether the tile is on a client far from the engine or in the
  desktop app on the engine's own machine.
- R4. On iOS the user moves between the page and the chat easily.

## 2. Goals, and what is not one

- G1. A site signed into in the tile is signed in for every agent browser session on every engine of the mesh,
  within seconds, without the user doing anything else.
- G2. A session the agent establishes in obscura works in the tile (P2).
- G3. The jar is a credential: sealed at rest, never in `sessions.sqlite`, logs, transcripts, tool results or events.
- G4. The `browser_*` tools can drive a tile through the client that holds it (P3).
- G5. On a phone the page is one tap from the chat and back, with its state intact (P4).
- G6. The agent can lift one form out of a page it drives into the chat; the user fills it there and the values go
  to that page's real form and nowhere else (P5).
- Not a goal: the user driving obscura (the live view stays watch-only, D10); syncing Safari, Chrome or any browser
  other than theuth's own tile; passkeys, device-bound sessions, service workers, HTTP cache (D7); the web client
  (it has no native tile, 205 D7, and takes no part).

## 3. Decisions

| # | Decision |
|---|---|
| D1 | One **browser profile** per owner mesh. There is no cross-node user id: mesh identity is per engine (recon §7d, `crates/mesh/src/identity.rs:1-3`); the owner is the paired mesh plus its shared vault keyring. The profile holds, per site (registrable domain, eTLD+1), every cookie with all attributes — `name, value, domain, host_only, path, expires, secure, http_only, same_site, partitioned` — and an optional localStorage snapshot per origin. `partitioned` is kept although `NSHTTPCookie` cannot carry the partition key (web §1). |
| D2 | Storage: a new replicated table `web_sites` in a new plugin crate `crates/webprofile`, one row per site, the jar of the site sealed with `Vault::seal_with_mesh_key` (`crates/secrets/src/vault.rs:482-487`), the pattern link uses for `pending.psk_sealed` (`crates/link/migrations/0001_link.sql:8-11`, `crates/link/src/devices.rs:286`). Replicated by `replica::attach` (`crates/replica/src/lib.rs:104-108`). The row key is a salted hash of the site, not the site. Caps in §4.1. |
| D3 | Tile → profile: the client app reads its own cookie store natively and pushes **deltas** (set / removed cookies) to its engine over the RPC `web_profile_push`, debounced 2 s. iOS and macOS use WebKit directly on the named store (`WKWebsiteDataStore dataStoreForIdentifier:` → `httpCookieStore`, `getAllCookies`, `WKHTTPCookieStoreObserver`); Windows and Linux use Tauri `Webview::cookies()` off the UI thread. **Changed from the brief** for macOS: Tauri's API needs a live webview and has blocked the main thread before (web §2); WebKit's needs neither. localStorage of the top-level origin is captured in an isolated content world on load-finished and before close. |
| D4 | Profile → obscura: loaded into a session's isolate by `Ctx` (`crates/browser/src/tools.rs:26-64`) before the first `browser_*` call of that session, not in `session::spawn` / `BrowserState::new` (**changed**: the `Browsers` actor also serves isolates keyed by private ids for the render provider and scrape slots, `crates/browser/src/lib.rs:219-225`, which must stay cookie-less under 122 §5). Read back and written as deltas on `browser_close` and before every isolate drop (`drop_session`, `lib.rs:207-211`: `Close`, `Sweep`, `CloseAll`, `CloseTree`). A profile change for a site is applied before the session's next `browser_open` to that site. localStorage is applied with the cookies, per origin, at load (**changed**: obscura's `browser_set_storage_state` already stores localStorage per origin independent of the current page, `obscura-mcp/src/lib.rs:2406-2412`, so "on first navigation" buys nothing). Requires a small obscura change (§4.3): today the round trip drops `host_only`. |
| D5 | Profile → tile (P2): on attach and on `WebProfileChanged`, the client sets the changed cookies into the tile's store. iOS/macOS can do this with no view open; Windows/Linux only while a web view exists, so they apply before the first load. |
| D6 | The whole tile store syncs; it is theuth's own browser, not Safari. Settings lists signed-in sites with **Forget this site** and **Sign out everywhere**. Config `browser.profile: shared \| off` (default `shared`; `off` stops push, load and save, and leaves rows as they are until cleared). Alternative "ask once per site" in §8. |
| D7 | Honest limits, in this document and in the text the model reads (§4.8): passkeys, device-bound sessions (DBSC), service workers and Cloudflare clearance do not travel; sites binding a session to IP, user agent or fingerprint may reject the moved session; the fallback is D8. |
| D8 | Agent control of the tile: the `browser_*` tools get a second backend behind a seam introduced at `Ctx::raw` (`tools.rs:45-64`). A new tool `browser_attach { view? }` switches the calling session's target between obscura (default) and a tile (§4.6 says why not a per-call argument). The tile backend publishes `WebViewCallRequested` and awaits the RPC `answer_web_view_call`, copying `PagePreviewRequested` / `answer_page_preview` (`crates/artifact/src/preview.rs:229-289`, `crates/artifact/src/rpc_artifacts.rs:22-23`). The client runs the call in an isolated script world. Synthetic input on Apple platforms is `isTrusted: false`. |
| D9 | Tile → engine state: the client reports each tile's URL, title and loading state with the RPC `web_view_state`; the read-only tool `web_views {}` answers "where is the user". |
| D10 | The agent's own browser shown to the user stays what `browser_show` does today: a 500 ms screenshot poll, watch-only (`crates/desktop/ui/src/components/chat/BrowserLive.tsx:14`; tool text `tools.rs:691`). Out of scope. |
| D11 | iOS: iPad keeps the tile beside the chat (205 §9) and collapses the sidebar when the tile would be narrower than 420 px. iPhone gets a full-screen page with a toolbar and a minimise control that leaves a pill above the composer (§7). |
| D12 | The prompt's "The two share nothing" sentence is replaced when P1 lands (§4.9). |
| D13 | `browser_show_element { ref?, selector?, title? }` (crates/browser, either backend of D8) lifts one element of the page — typically a sign-in form — into a transcript card rendered in the sandboxed inline-page frame of 120 (`crates/desktop/src/pages.rs:1-8`, `components/chat/HtmlPage.tsx:1-6`). The app draws the origin, the source browser and Submit/Cancel around the frame. Submit sends the values with the RPC `answer_element_form`; the engine fills the real controls by field index and submits the real form; the agent gets a receipt without values (§4.10). |
| D14 | Acting `browser_*` calls are raised to `Mutating` for the approval gate when they act on a site that has profile cookies, and always on a tile. Tool risk is a per-type constant today (`crates/tools/src/lib.rs:356-357`, every browser tool `Risk::ReadOnly` at `tools.rs:226`) and the registry's only per-call input is the secret-placeholder flag (`crates/tools/src/registry.rs:1484-1485`), so this needs a new per-call hook (§9). |

## 4. The contract

### 4.1 Table and row (crates/webprofile)

`<data_dir>/webprofile.sqlite`, migration `migrations/0001_web_sites.sql`, attached with
`replica::attach(&pool, &["web_sites"])` (single-column primary key and `hlc TEXT`, `replica/src/lib.rs:105-107`).
**One row per cookie and one per origin's localStorage snapshot** (changed after the P1 review, finding 2): replica
merges last-writer-wins under an HLC (`replica/src/lib.rs:1-4`), so per-cookie rows make the merge per cookie, two
engines writing different cookies of one site both keep theirs, and a change replicates as itself.

```sql
CREATE TABLE web_sites (
  id       TEXT PRIMARY KEY,  -- first 32 hex of HMAC-SHA256(id key, JSON ["cookie", site, name, domain, path])
                              --   or ["storage", site, origin]; "epoch" for the epoch row
  site_id  TEXT,              -- first 32 hex of HMAC-SHA256(id key, JSON ["site", site]); NULL on the epoch row
  sealed   TEXT,              -- base64url(nonce ‖ XChaCha20-Poly1305(mesh key, aad = "web_sites\n" ‖ id, entry JSON));
                              --   NULL = removed (a tombstone)
  hlc      TEXT
);
```

- **Id key** (changed after the P1 review, finding 1; there is no salt secret): `HMAC-SHA256(current mesh key,
  "theuth/webprofile/id/v1")`. Every engine holding the mesh keyring (`Vault::mesh_key`, distributed by pairing and
  the key frames, `crates/mesh/src/keys.rs`) computes the same ids with nothing created per engine, so there is no
  creation race; the file at rest names no site, and `site_id` groups, lists and forgets a site without opening a row.
  `site` is the lower-case registrable domain of §4.2.
- **Mesh key rotation** (183 §6.9): the ids change with the key. The first store operation that sees a new current
  key, or the plugin's one-minute maintenance tick, moves every row to its new id — re-sealed under the new key, the
  old id tombstoned — well inside the vault's 10-minute retire grace (`RETIRE_GRACE_MS`; the vault's retire counts only
  its own rows). Every later full read (a session's first load, the Settings list) also folds rows a lagging engine
  still wrote under the old ids, while the old key is retained. Every engine computes the same new ids, so concurrent
  moves converge; a newer tombstone at the new id keeps a removal. A row sealed under a key already retired is lost.
- **Entry** (the sealed plaintext, JSON) — the site, the epoch, the writer and the time travel inside, and the aad binds
  the row id, so a row is one atomic value:

```json
{ "v": 1, "site": "example.com", "epoch": 0, "writer": "tile:iPad", "updated_ms": 1795000000000,
  "item": { "cookie": { "name": "__Host-sid", "value": "…", "domain": "app.example.com", "host_only": true,
                        "path": "/", "expires": 1795000000, "secure": true, "http_only": true,
                        "same_site": "Lax", "partitioned": false } } }
```

  `item` is `{ "cookie": … }`, `{ "storage": { "origin", "local_storage": [[k, v]], "captured_ms" } }` or, on the
  epoch row, `{ "epoch": {} }`. `expires` is Unix seconds or `null` (a session cookie, kept until the site goes 30 days
  without a write; §11 Q1). `same_site` is `"Strict" | "Lax" | "None" | ""`. A row of another `v` is skipped (and
  counted in a log line), never misread. `SiteJar { v, site, cookies, origins }` remains the per-site shape a load
  answers, gathered from the site's rows.
- **Epoch**: the row `id = "epoch"` seals `{ "epoch": n, "item": { "epoch": {} } }`. "Sign out everywhere" writes
  `n+1` and tombstones every row; an entry written under an older epoch is dead.
- **Removal** is a column write (`sealed = NULL`), never a `DELETE`: a delete loses to a newer column write
  (`0001_link.sql:5-7`). A remove writes its tombstone even for a row this engine has not seen, so it wins over an older
  set elsewhere. Tombstones are deleted at open 30 days after their write (their `hlc`).
- **Caps**: per site at most 300 cookies and 128 KiB of cookie and snapshot JSON (over it the oldest snapshots go
  first, then the site's push is refused); one origin's snapshot at most 96 KiB, over that dropped whole, never cut;
  at most 500 live sites and 8 MiB per profile, refusing new sites with `profile full`. The profile caps read a local,
  non-replicated table `web_site_stats` (live rows and sealed bytes per `site_id`, kept by triggers on every write,
  own or replicated), never the sealed rows. Per push, before any work: at most 2000 changes and 64 snapshots, the
  device name cut to 64 characters; every cookie needs a name, a path from `/`, a value of at most 4096 bytes and a
  domain that is not a bare public suffix; its domain is stored lower case, and a leading `.` becomes
  `host_only: false`. Whether the replica feed has its own record limit was not found (§12).

### 4.2 Merge: deltas, not snapshots

Writers never upload a site snapshot; they upload changes against the last state they saw, so a stale browser
cannot delete what another wrote:

- The client keeps, in memory, the last cookie list it pushed or applied; on a store change it diffs
  `getAllCookies` against it, keyed `(name, domain, path)` (obscura's identity too, `obscura-net/src/cookies.rs:27-33`).
- `Ctx` keeps, per session, the cookies it loaded (per site and the newest row's `updated_ms`); at save it diffs the
  isolate's export against that.
- The webprofile actor groups changes by site (the registrable domain by the public suffix list, its private section
  included, so `user.github.io` is its own site; an IP address, a single-label host such as `localhost` and a bare
  suffix are their own site) and writes each change as its own row: set seals the cookie, remove or an `expires` in
  the past writes a tombstone, a set equal to the stored cookie writes nothing. A localStorage capture newer than the
  stored one replaces it; an emptied localStorage is a capture too.
- An agent session only updates sites the profile already holds (sites its load gave it, or sites the engine knows
  the profile holds): cookies of any other site stay in that session. New sites enter the profile through the user's
  web view (P1 rule, review finding 20).
- A cookie imported by `browser_import_cookies` (`tools.rs:835-841`) is remembered by `Ctx` and never saved back:
  that tool grants one session a local browser's cookies, not the mesh (§11 Q2). Its value is scrubbed from replies
  like a profile cookie's.

### 4.3 obscura change (skymoore/obscura, branch `theuth/sync-2026-11-02`, `Cargo.toml:315-316`)

`browser_storage_state` exports `CookieInfo`, which has no `host_only` (`obscura-mcp/src/lib.rs:2323-2332`), and
`browser_set_storage_state` imports through `set_cookies_from_cdp` with `host_only = false`
(`lib.rs:2423`, `cookies.rs:284-286`). Every cookie comes back domain-scoped, and `cookie_prefix_allows` then drops
any `__Host-` cookie (`cookies.rs:328`) — the prefix most session cookies of careful sites use. The jar is private
(`BrowserState.context` is not `pub`, `obscura-mcp/src/lib.rs:73`), so theuth cannot reach it except through tools.
Change: export `host_only` from the entry, read an optional `host_only` on import and call
`set_cookies_from_cdp_with_scope` (`cookies.rs:291-296`). Backward compatible: absent means `false`, as today.
`partitioned` is carried by theuth and ignored by obscura, which has no partition model.

### 4.4 RPC methods (client → engine) and events

| Method | Params | Result | Notes |
|---|---|---|---|
| `web_profile_push` | `{ device, epoch, changes: [{op:"set", cookie} \| {op:"remove", name, domain, path}], storage: [{origin, local_storage, captured_ms}] }` | `{ applied, sites: [String], epoch, profile: "shared" \| "off" }` | Refused with error `stale-epoch` and data `{ epoch }` (the current one) when `epoch` is older; the client then clears its store (sign out everywhere happened elsewhere). Refused with `unknown-epoch` and the same data when `epoch` is newer than the current one. Refused before any work with `invalid push: …` beyond 2000 changes or 64 snapshots or for a malformed cookie (§4.1 caps; the device name is cut to 64 characters), and with `profile full`. With `browser.profile: off` nothing is written and the result says `profile: "off"`; an error then means only "starting/unavailable" or a refusal. Not forwarded: any engine writes the replicated table. |
| `web_profile_pull` | `{ sites?: [String] }` | `{ epoch, cookies: [Cookie], storage: [...] }` | P2, for D5. The only method that returns values; the app only. |
| `web_profile_sites` | `{}` | `[{ site, cookies, origins, updated_ms, writer }]` | Settings list. Counts, never values; `origins` counts non-empty snapshots; `updated_ms` and `writer` are the site's newest row's. `[]` with `browser.profile: off`. |
| `web_profile_forget` | `{ site }` | `{}` | P2. Writes `sealed = NULL`, publishes `WebProfileChanged`. |
| `web_profile_clear` | `{}` | `{ epoch }` | P2. Bumps the epoch and nulls every row. |
| `web_view_state` | `{ session, view, url, title, loading }` | `{}` | D9. Forwarded to the session's owner by `rpc_forward::route` (`crates/agent/src/rpc_forward.rs:1-10`). |
| `answer_web_view_call` | `(id, answer)` | `bool` (still pending) | D8. Forwarded to the owner like `web_view_state`. |
| `answer_element_form` | `(id, answer)`; `answer` = `{ submit: { values: { "<index>": String } } }` \| `{ cancel: {} }` | `bool` (still pending) | D13, P5. Forwarded to the owner. Values handled as in §4.10. |
| `element_forms_pending` | `{}` | `ElementFormRequested[]` | D13, P5. Pending cards for a client that attached late (like `secret_requests_list`, `crates/secrets/src/rpc.rs:41-45`). |

Every method is declared with `audience = "owner"` as the credentials API is (`crates/secrets/src/rpc.rs:8`).

Events (`wire::WireEvent`, registered in `declare_wire`, mirrored like 205 §4.2):

- `WebProfileChanged { sites: [String], epoch }`, topic `browser.web-profile-changed`. Site names, never values;
  not session-scoped, so it is not stored with any session. Published for a local write and for rows replicated from
  another engine (the sites read from those rows; a tombstone names its site through a live row of the same
  `site_id`; the epoch row alone gives `sites: []`), so every engine's D14 gate learns a sign-in made elsewhere.
- `WebViewCallRequested { id: Uuid, session, view, op, args }`, topic `artifact.web-view-call-requested`, and
  `WebViewCallResolved { id }`, topic `artifact.web-view-call-resolved`.
- `WebViewCallAnswer` = `Claimed { client }` | `Done { result: Value, png?: String }` | `Failed { reason }`.
- `ElementFormRequested { id: Uuid, session, call_id, origin, url, source, title, html, width, height, fields: [{ index, kind, name, label, masked }] }`,
  topic `browser.element-form-requested`, and `ElementFormResolved { id, outcome: "submitted" | "cancelled" | "stale" | "timeout" }`,
  topic `browser.element-form-resolved`. `source` is `"obscura:<engine>"` or `"web:<id>"`. No field values, ever.

A session owned by another engine reaches a front-door client only through the event relay, which forwards
`AgentEvent`s and, on a `Watch` stream, `ApprovalRequested`, `ApprovalResolved`, `AskRequested`, `AskResolved`,
`JobEvent` and `CellEvent` (`crates/sessionsync/src/events.rs:1-34`, `:90`). `WebViewCallRequested`,
`WebViewCallResolved`, `ElementFormRequested` and `ElementFormResolved` join that list. As read, `WebViewOpened` (205) and `PagePreviewRequested` are not on it,
so `show_web` from a remote-owned session does not open a tile on a front-door client today (§11 Q4).

### 4.5 Client side: shell commands and events (crates/desktop/src)

The shell reads and writes the store and runs scripts; the UI diffs, pushes and answers. Every command below exists
on iOS (`wt:ios_webview.rs`) and the desktop (`webview.rs`) with the same name, arguments and errors.

| Tauri command | Arguments | Returns / does |
|---|---|---|
| `web_profile_read` | — | `Cookie[]` of the whole tile store (§4.1 shape, `partitioned` from `NSHTTPCookie` properties, else `false`). |
| `web_profile_write` | `set: Cookie[]`, `remove: CookieKey[]` (`{name, domain, path}`) | Writes into the store; iOS/macOS with no view open, Windows/Linux only while a web view exists (else error `no web view`). P2. |
| `web_profile_clear` | — | Removes every cookie of the tile store and its localStorage, sessionStorage and IndexedDB data (`WKWebsiteDataStore removeDataOfTypes:`). Ships in P1: the UI calls it on `stale-epoch`. |
| `web_storage_capture` | `label` | `{ origin, local_storage: [[k, v]] }` of the top-level origin, run in the isolated world. A capture over the snapshot cap (96 KiB) is an error starting `storage too large`, never an empty or cut snapshot. |
| `web_view_call` | `label`, `op`, `args` | Runs one tile op (§4.7); returns its JSON, or `{ png }` (base64) for `snapshot`. P3. |

Tauri events: `web-profile://changed {}` — the store changed, debounced 2 s in the shell (iOS/macOS: a
`WKHTTPCookieStoreObserver` object, `define_class!` like the delegate at `wt:ios_webview.rs:356-389`, on the
`httpCookieStore` of the `DATA_STORE` store at `:657-659`; Windows/Linux: after `on_page_load` finished,
`webview.rs:302-309`). The `DATA_STORE` identifier is shared by iOS and macOS (`crates/desktop/src/web_rules.rs`, wt),
so each device has one store for every tile. macOS uses WebKit directly (`objc2-web-kit` is already used by the iOS
module, `wt:ios_webview.rs:50-54`); Windows/Linux use Tauri `Webview::cookies()` / `set_cookie()` on a blocking task.

UI (crates/desktop/ui/src): `lib/web-profile.ts` exports `startWebProfileSync()`: on `web-profile://changed` it
calls `web_profile_read`, diffs against the last pushed list (key `(name, domain, path)`), calls
`web_storage_capture` for each live tile (also on each `web-view://state` with `loading: false` and before
`web_view_close`), and `web_profile_push`; on `stale-epoch` it calls `web_profile_clear`. It
also forwards each `web-view://state {label, url, title, loading}` (already emitted, `webview.rs:55`, `:302-318`;
`wt:ios_webview.rs:435-450`) to `web_view_state` with the session that opened the tile, at most one per 500 ms per view.

As built in P1 (the shell and UI review fixes):

- **Platforms.** Every profile command answers `unsupported: the browser profile needs macOS 14 or iOS 17 and later`
  on Windows and Linux (the Tauri `Webview::cookies()` route is not built) and on macOS < 14 / iOS < 17, where the
  tiles use the app's default store; nothing is observed there.
- **Callers.** Every profile command refuses a caller that is a tile (`web-*`) with an error starting `no access to
  the browser profile from`; on iOS it also refuses anything but `main`. `web-profile://changed` is emitted to the `main`
  window only.
- **Host-only** is derived by `web_rules::profile::scope` from `NSHTTPCookie.domain` and its `properties`, first
  rule that matches: (1) a `__Host-` name is host-only; (2) a domain that is an IP address is host-only; (3) a
  leading `.` on `domain` or on the `Domain` property is domain-wide (WebKit's mark of `Domain=`); (4) no dot, but
  the `OriginURL` property's host is a subdomain of the domain: domain-wide; (5) otherwise host-only. The domain is
  stored lower case without the dot. A `tracing::debug!` line `web profile cookie scope` names the rule, never a
  value.
- **UI sync.** At start and after an engine switch, the first `web_profile_read` seeds the baseline and is not
  pushed (cookies already in the store reach the profile when they next change). A cookie change pushes the cookie
  diff only. A `web-view://state` with `loading: false` also re-reads the store and pushes its diff (added by the P1
  simulator proof: there the `WKHTTPCookieStoreObserver` never fired, so a cookie set without a page load, by
  script or `fetch`, reaches the profile only with a tile's next finished load). Storage is no longer captured on
  cookie changes, but on `web-view://state` with `loading: false` and
  before `web_view_close`, pushed 2 s after the last capture, and what is still captured is pushed best-effort on
  `pagehide` / `beforeunload`. One push in flight; a failed read or push keeps its delta and retries after 10 s,
  doubling to at most 60 s. `profile: "off"` stops pushing to that connection, re-probed by the next change after
  60 s. On `stale-epoch` the UI calls `web_profile_clear`; a client whose clear fails keeps its old epoch and stops
  pushing on that connection, so its cookies never sign the profile back in.
- **One data store instance.** The shell keeps the `DATA_STORE` `WKWebsiteDataStore` it first gets for the life of
  the process: `dataStoreForIdentifier:` answers a new instance once the last one is released, and the cookie store
  refers to its data store only weakly, so the observer had been on a store the tile did not use.

### 4.6 Seam and target (crates/browser)

`Ctx::raw` is the one place every tool's obscura call passes (`tools.rs:45-64`); tools are a name plus JSON
passed to `obscura_mcp::call_tool` (`session.rs:24-31`, `:147`). There is no `Page` or driver trait (recon §3).

- New `enum Target { Obscura, Tile { view: String } }` in `Ctx`, keyed by `SessionId` (a `parking_lot::Mutex<HashMap>`
  like `pending`, `tools.rs:35-36`). `raw` dispatches on it: `Obscura` sends the `Call` as now; `Tile` hands the
  obscura tool name and arguments to `tile::call(rt, session, view, name, args)`, which maps them to a tile op.
- `browser_attach { view?: String }` (risk `ReadOnly`): `view` is a `web:<id>` from `show_web`'s receipt or
  `web_views`; without `view` it returns to obscura. It fails with `no client holds tile web:<id>` when no client
  claims within 2 s. **Why a tool, not a `view` argument on 16 tools:** refs (`e3`) are per page and go stale on any
  navigation (`tools.rs:279`); a per-call target lets one batch mix refs of two pages. One switch keeps the 16 schemas,
  the read-then-act loop and the System-1 shadow ledger (`tools.rs:33-36`) per session as they are. Its cost, a hidden
  mode, is paid by prefixing every reply on a tile with `[tile web:<id> · <host>]`.
- `browser_run` (System-1), `browser_import_cookies` and `browser_list_profiles` stay obscura-only.
- D14 timing: the gate decides before a call, on the site of the page the session starts from (`browser_open`: the
  requested URL's site; `browser_eval`: any page once the session holds profile cookies, since a script can reach any
  signed-in origin). After a call that may navigate, `Ctx` records the page it reached (`browser_navigate`'s reply names
  it; other calls read `location.href`), so the next acting call is gated. The navigation that first reaches a
  signed-in page (a redirect, a click, `browser_back`) is therefore not gated itself; it is logged.

### 4.7 Tile operations

The engine owns the agent script (`crates/browser/src/tile/agent.js`, one source, also used on obscura through
`browser_eval` for D13); the client is a generic executor. `op` is one of `script`, `navigate`, `back`, `forward`,
`reload`, `snapshot`. `script` args: `{ world: "isolated" | "page", source, arguments, timeout_ms }`; `isolated` is
`WKContentWorld` named `theuth-agent` with `callAsyncJavaScript` (own JS globals, same DOM; CSP `script-src` does not
block it, web §3), where refs live in a `Map` of `WeakRef`s. Desktop reaches the native view through
`Webview::with_webview`; Windows uses `CallDevToolsProtocolMethod` (`Page.createIsolatedWorld`, `Runtime.evaluate`,
`Input.dispatchMouseEvent`), the one platform with trusted input (web §3). `snapshot` is `takeSnapshot` /
`Page.captureScreenshot`. Timeouts: 2 s to `Claimed`, then 30 s, or the tool's own wait plus 5 s. `lib/web-view-agent.ts`
exports `startWebViewAgent()`: it claims a `WebViewCallRequested` only when this window holds `web-<view>`, calls
`web_view_call`, and answers with `answer_web_view_call`.

| Tool | On a tile | How, and the limit |
|---|---|---|
| `browser_open` | supported | navigate the tile (same path as `web_view_navigate`), wait for finish, then `browser_read`. The user sees it. |
| `browser_read` | supported, limits | text and refs from the main frame; cross-origin frames unread (no frame list API, web §3). Output format is ours, close to obscura's. |
| `browser_elements` | supported, limits | same as read. |
| `browser_click` | supported, limits | `scrollIntoView`, pointer and mouse events, `click()`. `isTrusted: false` everywhere except Windows, where CDP input is trusted; a site gating on it ignores it. |
| `browser_fill` | supported, limits | native value setter plus `input`/`change` (works with React-controlled inputs); file inputs only from base64 via `DataTransfer`. |
| `browser_type` | supported, limits | synthetic key events per character; no IME. |
| `browser_press` | supported, limits | synthetic `KeyboardEvent`; Enter in a form falls back to `requestSubmit()`. |
| `browser_select` | supported | set `selected`, fire `change`. |
| `browser_scroll` | supported | `scrollBy` / `scrollIntoView`. |
| `browser_wait` | supported | a promise in the content world, bounded. |
| `browser_eval` | supported, limits | runs in the **page** world (`WKContentWorld.page`) so page globals are visible; the page can see and tamper with it. |
| `browser_back` | supported | `goBack` (as `web_view_history` does, wt 205 §9). |
| `browser_screenshot` | supported, limits | `takeSnapshot` (Apple), `Page.captureScreenshot` (Windows); viewport only, no `full_page`. |
| `browser_tabs` | unsupported | a tile is one page; new windows go to the system browser (205 §8). Error names `web_views`. |
| `browser_close` | changed | detaches (back to obscura); never closes the user's tile. |
| `browser_show` | unsupported | "the user is already looking at this page". |

There is no public API for trusted input into a `WKWebView`, and the Web Inspector protocol has no Input domain
(web §3). This holds for the iPad and for the desktop app on the engine's machine alike: both are clients and the
code path is the same. An engine with no client app has no tile and the agent uses obscura.

### 4.8 Tool text

`browser_open` gains: "When the user has signed into a site in their web view (on any of their devices), this
browser is signed in too. Passkeys, device-bound sessions, service workers and Cloudflare clearance do not carry
over, and a site that ties a login to the device, network or browser may ask to sign in again: then use
`browser_attach` with the user's tile and act there." `browser_attach` carries the trusted-input limit verbatim.

### 4.9 Prompt (D12)

In `crates/artifact/src/tools.rs:60`, replace from "The two share nothing:" to "(watch only)." with:

> The two share one sign-in: when the user signs into a site in a `show_web` tile on any of their devices, your
> browser is signed in to that site too, and a site you sign into works in their tile. They share nothing else: you
> do not see a `show_web` page unless you `browser_attach` to it, and the user does not see your browser unless you
> call `browser_show` (watch only). A site may still refuse a login that moved; then attach to the user's tile and
> act there, where the session is theirs. When a step needs the user's own input on a page you drive (a
> sign-in, a code), call `browser_show_element` on the form: they fill it in the chat and you never see the values.

The clause "and a site you sign into works in their tile" lands with P2, `browser_attach` with P3, the last sentence
with P5; each phase ships the text its tools support.

### 4.10 `browser_show_element` (D13)

```
browser_show_element { ref?: String, selector?: String, title?: String }   // DIRECT, Risk::Mutating, APPROVED_BY_CARD, not promotable
```

- Exactly one of `ref` (from `browser_read` / `browser_elements`) and `selector`. Works on obscura (the agent script
  through `browser_eval`) and on an attached tile (`web_view_call` op `script`, world `isolated`).
- **Serialise** (`agent.js` `serializeElement`): clone the subtree; inline `getComputedStyle` of every node as a
  `style` attribute, properties that differ from the element's default only; same-origin `<img>` as `data:` URIs,
  cross-origin images replaced by an empty box of the same size; remove `script`, `noscript`, `style`, `link`, `iframe`,
  `object`, `embed`, `svg <use>` with external refs, every `on*` attribute, `href`, `action`, `formaction`, `src` that
  is not `data:`, and `url()` in styles that is not `data:image`. Never copy a control's current value; keep
  `checked` and the selected option. Hidden inputs are dropped from the clone and stay in the real form. Each
  visible control and each submit button gets `data-theuth-field="<n>"` in the clone and in the live page, and the
  root gets `data-theuth-form="<id>"` in the live page. Returned with the root's rendered `width` and `height`, the
  page `origin` (`location.origin`) and `url`.
- **Refuse** with `ToolOutput::err`: the subtree contains any `iframe` — "the form contains a frame from <origin> (a
  CAPTCHA or a sign-in provider); it can only be used in the page itself: show it to the user with show_web or
  browser_attach to their tile"; the clone is over 256 KiB — "element too large to show (N KiB > 256 KiB); pick a
  smaller element"; no visible control; a capped contact turn (as every browser tool, `tools.rs:237-239`).
- The engine re-sanitises the HTML with an allowlist (tags of forms and text, attributes `style`, `type`, `name`,
  `placeholder`, `autocomplete`, `checked`, `selected`, `disabled`, `for`, `id`, `class`, `data-theuth-field`) before
  publishing `ElementFormRequested`; the client trusts nothing in it.
- **Card** (`components/chat/ElementFormCard.tsx`, UI): app chrome above the frame shows a lock glyph, the origin
  as `scheme://host[:port]` in ASCII (punycode, never Unicode, so a homograph shows as `xn--`), and the source ("your
  web view" for `web:<id>`, "agent browser on <engine>" for obscura); below it **Submit** and **Cancel**. The frame is
  `HtmlPage`'s (`sandbox="allow-scripts allow-forms"`, `HtmlPage.tsx:171`; `theuth-page` header CSP `sandbox …`,
  `pages.rs:132-135`). That header has no `default-src`, so the document the UI builds starts with `<meta
  http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline';
  script-src 'unsafe-inline'; form-action 'none'; base-uri 'none'">`, then the app's own bootstrap (`lib/element-form.ts`
  `formBootstrap()`), then the snapshot. The bootstrap answers `theuth/collect` with `{ "<index>": value }` by
  `postMessage`, turns a submit inside the frame (Enter) into `theuth/submit-request`, and reports size. Password
  inputs stay `type=password` and render masked.
- **Submit**: `answerElementForm(id, values)` calls `answer_element_form`. The engine (in the owning session's
  `Ctx`) checks the browser is still on `url` (fragment ignored) and `[data-theuth-form="<id>"]` exists with the same
  field count; else `ElementFormResolved { outcome: "stale" }` and the tool returns "the page changed since the form
  was shown; nothing was typed". Then, per index: native value setter plus `input`/`change` (as `browser_fill`),
  `checked` for checkbox and radio, `value` for select; then the form's first submit button `click()`, else
  `form.requestSubmit()`, else the clone's submit-button index. It waits for a navigation or a 10 s settle.
- **Receipt** (the only thing the model sees): `submitted · <origin> · filled 0 email, 1 password · now <url> — <title>`
  or `… · the page did not change within 10 s`; `cancelled`; `no answer` after `tools.ask_timeout_secs`
  (`crates/tools/src/lib.rs:562`). Field labels and names, never values.
- **Values** are handled like `answer_secret`'s (`crates/secrets/src/rpc.rs:29-40`): they live only in the RPC
  params, in `Zeroizing<String>` while filling, and in the page. Not in the transcript, `ToolOutput`, events, `tracing`
  fields or `sessions.sqlite`; the scrub of §9 adds them for the rest of the session.
- **What "exactly like the site" means.** A static clone with computed styles and no site JavaScript: client-side
  validation, show-password toggles, dependent fields, autocomplete widgets and web fonts do not run or load. A
  multi-step sign-in (username, then password, then a code) is one call per step; the receipt says the page changed.

## 5. The owner's scenario, end to end (P1)

1. iPad app, attached through the relay to the engine holding the hub lease (recon §7c). The user opens a tile and
   signs into `example.com`. WebKit stores `__Host-sid` (HttpOnly, Secure, Lax) in the `DATA_STORE` store.
2. The observer fires; 2 s later the app diffs and calls `web_profile_push` with one `set`.
3. The hub engine seals the `example.com` row; replica ships it over `theuth/records/1` to every engine.
4. On engine B an agent session calls `browser_open https://app.example.com/orders`. `Ctx` sees no profile loaded
   for the session, loads every live row with `browser_set_storage_state` (with `host_only`), then navigates. The
   page is signed in.
5. The agent's actions on `example.com` are acting calls on a profile site: the gate treats them as `Mutating` (D14).
6. On `browser_close` or the isolate's idle drop, `Ctx` / the actor export the jar and push the diff (a refreshed
   session cookie, say) to the profile.

## 6. Settings and consent (P2)

Settings › Browser: the switch `browser.profile` and the list from `web_profile_sites` (site, cookie count, last
changed, by which device or engine). **Forget this site**: `web_profile_forget`; every client deletes that site's
cookies from its store and every engine's `Ctx` deletes them from live isolates before their next call.
**Sign out everywhere**: `web_profile_clear`; every client clears its store (`removeDataOfTypes` on the named store)
and every engine drops all profile-loaded isolates. A client offline at the time learns it from `stale-epoch` on
its next push and clears then.

## 7. iOS navigation (P4 for iPhone)

- iPad: unchanged from 205 §9. When a web tile is placed and the tile would be narrower than 420 px with the sidebar
  inline, the sidebar collapses (`useSidebar().setOpen(false)`); it is not reopened automatically.
- iPhone: one pane at a time below 768 px (`crates/desktop/ui/src/hooks/use-mobile.ts:3-5`,
  `components/chat/Panes.tsx:109-119`); 205 §9 gives the phone only an address card (wt doc line 194). New:
  `components/chat/PhoneWebView.tsx`, a full-screen overlay holding a 52 px DOM toolbar — back, forward, reload,
  address, open in Safari, minimise, close — over the placeholder the native view frames to. **Minimise** calls
  `web_view_visible(false)` and sets `minimised` in `lib/web-views.ts`; `components/chat/WebPill.tsx` sits above the
  composer with the site title and favicon; tapping it restores the view, which was never destroyed, so scroll
  position, form input and history survive. A downward swipe of at least 60 px on the toolbar minimises. The view's
  own edge swipe is history: `ios_webview.rs` sets `allowsBackForwardNavigationGestures` (not set today, wt
  `:655-666`). A `show_web` from the agent while the composer has focus or a draft arrives as the pill, never over
  the screen. Several tiles: the pill shows the latest and a count; tapping the count lists them.
- Native glass chrome (design 198) is not on main (recon §6); the pill is DOM and can move to native chrome later.

## 8. Trade-offs and alternatives

- **Ask once per site** instead of syncing the whole store: a sheet on the first sign-in to a site ("Let your agents
  use your example.com sign-in?"). Narrower blast radius, one more prompt per site, and a sign-in the user did
  for the agent's sake is not usable until answered. Rejected for P1 because the tile is theuth's own browser that
  the user opens for this purpose; the Settings list and D14 carry the control instead.
- **Agent in the user's own browser** (Claude in Chrome, Playwright MCP `--extension`, web §4): nothing to sync, but
  desktop-only, and the agent acts in the real profile. D8 is the same idea limited to the tile.
- **Persisting the whole user-data-dir** (Browserbase contexts, web §4): not possible; WebKit exposes cookies and
  script-reachable storage only (web §1).
- **The vault itself** (`Vault::set`, one secret per site, `vault.rs:637-645`, recon §7b) instead of a table: rows
  are versioned per `(name, version)` and every write keeps the old version, which for a cookie jar written every
  few seconds grows without bound, and the jar would show in `secret_list`. Rejected for its own table.

## 9. Security

- **The profile is a credential.** A session id is "temporarily equivalent to the strongest authentication method"
  of the site (OWASP Session Management Cheat Sheet,
  https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html, web §5). A copy of
  `web_sites` plus the mesh key is the user's sign-in to every site in it. Sealed like a vault row; the mesh key
  sits sealed under the engine's own key (`vault.rs:1-11`).
- **Reach.** Any agent session on any engine of the mesh can act as the user on a signed-in site, including
  sessions started by routines and channels. A capped contact turn gets no browser at all (`tools.rs:211-239`), so no
  contact or A2A peer reaches the profile.
- **Prompt injection.** A page read while signed in can carry instructions (a mail, a ticket, a review) aimed at an
  agent holding that sign-in. Page text is data; the tool text says so; D14 puts a person in front of acting calls
  under the default policy.
- **D14, the gate.** New `Tool::credentialed(&self, session: SessionId, args: &Self::Args) -> bool` (default
  `false`), forwarded by `ErasedTool::credentialed(&self, session, args: &Value)`. At `registry.rs:1484`, `:1616`
  and `:2208` the registry computes `risk = if credentialed { spec.risk.max(Risk::Mutating) } else { spec.risk }`
  and passes that `risk` to `effective()` (`registry.rs:1369-1391`), so both the cap check (`:1376`) and the policy
  see `Mutating`; passing a bool like `mentions` would only raise to `Ask` and skip the cap. The acting browser
  tools (open, click, fill, type, press, select, eval, back, and any call on a tile) answer from a synchronous map
  `Ctx` keeps: session → current site and whether that site had profile cookies loaded. A session capped
  `read_only` is refused acting calls on signed-in sites and on tiles (`denied: subagent is capped at read_only`,
  `registry.rs:1397`); reads stay allowed. Under the default policy (`mutating: Ask`, `crates/tools/src/lib.rs:589-596`)
  each such call prompts unless the user turned prompts off.
- **`Tool::APPROVED_BY_CARD`** (new const, default `false`): for a tool whose own card is the user's approval. The
  cap check uses the tool's risk; the policy lookup then uses `Risk::ReadOnly`. Only `browser_show_element` sets it
  (Mutating, so a `read_only` cap hides it; no second prompt before the card, since pressing Submit is the approval).
- **Never echo values.** No theuth tool returns cookies (the 16 tools have no cookie read, `tools.rs:273-694`).
  `browser_eval` and `browser_read` replies on a profile site are scrubbed of every loaded cookie value of 8 or more
  characters, replaced by `[cookie <name>]`. The jar never enters `tracing` fields, events, `ToolOutput`, or
  `sessions.sqlite`; `web_profile_pull` is the only wire path that carries values, to the app.
- **`site_learn`.** It copies the session's cookie header for a learned domain into the routes store
  (`crates/routes/src/capture.rs:67-92`, `crates/routes/src/lib.rs:290-333`). With the profile loaded that is the
  user's sign-in; it must only learn on a profile site after an acting-call approval, and where the routes store keeps
  that header was not checked (§12).
- **Transport.** Pushes travel client → relay → hub engine; the relay is stateless (recon §7c). Whether it can read
  payloads was not checked (§12); the values ride the app's authenticated RPC connection either way.
- **Tile control** is the user's live session on the user's screen: always credentialed (D14), every reply prefixed
  with the tile, and the tile shows a thin "agent is acting" bar while a call is pending (`Claimed` to `Done`).
- **D13, phishing and injection.** A page the agent reads (or an injected instruction) may ask it to "show the login
  form" of another site, or a hostile page may draw a fake sign-in. The origin in the card is the app's, read from
  the browser at serialise time, not from the snapshot; values go only to the element stamped `data-theuth-form` on
  the page that was serialised, re-checked at submit, and nowhere else. The snapshot cannot draw outside the frame,
  run script, load anything or submit (CSP `default-src 'none'; form-action 'none'`, sandbox without
  `allow-same-origin` or `allow-top-navigation`). The user's Submit is the approval; a `read_only` subagent cannot
  see the tool. What the user cannot be protected from here is a real page of the right origin that is itself
  hostile: the card shows the origin, the user decides.
- **Off switch and forget** (§6). `browser.profile: off` takes effect at once for new loads; isolates already
  loaded keep their cookies until dropped.

## 10. Phases

Every phase is built by three builders in one worktree on disjoint paths (205 §8). Fixed for all phases:

- **Engine** owns `crates/browser`, `crates/artifact`, `crates/webprofile` (new), `crates/tools`, `crates/sessionsync`,
  `crates/agent/src/rpc_forward.rs`, `crates/rpc-compose`, `crates/theuth/tests/fixtures/wire/`, root `Cargo.toml`
  and `Cargo.lock`, the obscura fork, and the generated bindings `crates/desktop/ui/src/rpc/bindings/**` (it runs
  the bindings generator, `just ts-desktop`, as an early step).
- **Shell** owns `crates/desktop/src/**/*.rs` (incl. `ios_webview.rs`, `webview.rs`, `web_rules.rs`, `lib.rs`) and
  `crates/desktop/Cargo.toml`. A new workspace dependency is requested from the engine builder, never added to the root.
- **UI** owns `crates/desktop/ui/src/**` except `rpc/bindings/**`.
- Testing apart: the engine drives its RPCs and events in `rpc-compose` API tests with a fake client that answers
  (as `answer_page_preview_first_done_wins`, `crates/rpc-compose/src/api/tests.rs:1382`); the shell keeps conversions
  and rules in pure functions with unit tests and checks with `cargo check -p desktop` for the macOS and iOS targets;
  the UI tests with vitest against mocked `invoke` and a mocked RPC client fed JSON fixtures of §4's shapes (as
  `lib/web-views.test.ts`) until the bindings land.

### P1 — the owner's scenario on iPad and macOS (D1–D4, D7, D9 minimal, D12, D14)

| Engine | Shell | UI |
|---|---|---|
| obscura `obscura-mcp/src/lib.rs` (§4.3); `crates/webprofile/{Cargo.toml, migrations/0001_web_sites.sql, src/lib.rs, src/store.rs, src/merge.rs, src/rpc.rs}`; `crates/browser/src/{tools.rs, lib.rs, profile.rs}`; `crates/tools/src/{lib.rs, registry.rs}`; `crates/artifact/src/{tools.rs, webview.rs}`; `crates/agent/src/rpc_forward.rs`; rpc-compose mirror and golden; wire fixture; bindings | `ios_webview.rs`, `webview.rs`: `web_profile_read`, `web_storage_capture`, event `web-profile://changed` | `lib/web-profile.ts` (`startWebProfileSync`), `lib/web-views.ts` (state forwarding), `App.tsx` (start the sync) |

- New names: table `web_sites`; secret `webprofile/salt`; actor `webprofile`; RPCs `web_profile_push`,
  `web_profile_sites`, `web_view_state`; event `WebProfileChanged`; tool `web_views {}`; config `browser.profile`;
  `Tool::credentialed`.
- Tests: seal/open and wrong-aad refusal; delta merge with two writers; caps; `stale-epoch`; two engines replicate a
  row (as `crates/document/tests/replicate.rs`); a `__Host-` HttpOnly cookie survives export → profile → import; a
  private-id isolate never gets the profile; save on `Close` and `Sweep`; imported cookies not saved; `credentialed`
  raises the risk and a `read_only` cap denies; scrub; wire golden and schema snapshot; UI diff and debounce.
- iPad simulator, engine on 333 plus a second engine: sign in to a local HTTPS test site in the tile;
  `web_profile_sites` lists it on both within 5 s; an agent on the second engine opens the protected page signed in;
  the test site logs one session id from both browsers.

#### P1 as built (after the engine review)

What the P1 engine differs in from the text above, decided by the orchestrator after the review; §4.1, §4.2, §4.4
and §4.6 already say it:

- No secret `webprofile/salt` (nor the interim `mesh/webprofile-salt`): row ids are keyed by an id key derived from
  the mesh key (§4.1), so D2's "salted hash of the site" is a keyed hash of each cookie's identity, and the P1 "new
  names" lose the secret. A mesh key rotation moves the rows to the new ids.
- One row per cookie and per origin's snapshot, not per site (D2, §4.1): the table is `id, site_id, sealed, hlc`
  plus a local `web_site_stats`; epoch, writer and time are inside the sealed entry. The migration was rewritten in
  place (never shipped); a development database created by the first P1 build fails its checksum and needs
  `webprofile.sqlite` removed.
- `web_profile_push`'s result has `profile: "shared" | "off"`; `unknown-epoch` and `invalid push: …` join
  `stale-epoch` and `profile full`; `browser.profile: off` answers empty instead of an error.
- An agent session never adds a site to the profile (§4.2); `browser_open`'s tool text says so.
- `WebProfileChanged` is also published for replicated rows (§4.4). The browser's D14 gate answers from the sites
  the profile is known to hold and from the session's own loaded cookies; a failed load is retried with a backoff
  per session; a save refused as `stale-epoch` clears the isolate's profile cookies and loads again; the last save of
  a dropped isolate waits at most 2 s for its export.
- `Tool::CREDENTIALED` (a const, default `false`) gates `Tool::credentialed`, so no other tool's arguments are
  deserialised for it (§9).

### P2 — profile into the tile, settings (D5, D6)

| Engine | Shell | UI |
|---|---|---|
| `crates/webprofile/src/{rpc.rs, store.rs}`; `crates/browser/src/profile.rs` (forget/clear on live isolates); fixtures; bindings | `web_profile_write`, `web_profile_clear` in `ios_webview.rs`, `webview.rs` | `lib/web-profile.ts` (apply on `WebProfileChanged`), `components/settings/BrowserProfile.tsx` |

- New names: RPCs `web_profile_pull`, `web_profile_forget`, `web_profile_clear`.
- Tests: forget on one engine clears an isolate on another before its next call; clear bumps the epoch; a stale
  push is refused; pull answers only an owner-audience connection.
- Simulator: the agent signs in to the test site in obscura; the user opens it in the tile signed in; forget removes
  it from the store (`web_profile_read`).

### P3 — agent control of the tile (D8, D9 full)

| Engine | Shell | UI |
|---|---|---|
| `crates/browser/src/{tools.rs, tile.rs, tile/agent.js}` (`browser_attach`, `Target`); `crates/artifact/src/{webview.rs, rpc_artifacts.rs}` (event pair, `answer_web_view_call`); `crates/sessionsync/src/events.rs`; `crates/agent/src/rpc_forward.rs`; fixtures; bindings | `web_view_call` (ops of §4.7, isolated world, `takeSnapshot`, CDP on Windows) in `ios_webview.rs`, `webview.rs` | `lib/web-view-agent.ts` (`startWebViewAgent`), the "agent is acting" bar in `components/chat/WebFrame.tsx` |

- New names: tool `browser_attach { view? }`; events `WebViewCallRequested`, `WebViewCallResolved`; RPC
  `answer_web_view_call`; command `web_view_call`; world `theuth-agent`; `crates/browser/src/tile/agent.js`.
- Tests: the round trip against the real RPC; no claim → `no client holds tile web:<id>`; only the holder claims; each
  op of §4.7 against a fixture page in the fake client (engine) and in a desktop test webview (shell).
- Simulator: from a session on the second engine, attach to the iPad tile, read, fill and submit a form, screenshot.

### P4 — iPhone surface (D11)

| Engine | Shell | UI |
|---|---|---|
| — | `ios_webview.rs`: `allowsBackForwardNavigationGestures` | `components/chat/{PhoneWebView.tsx, WebPill.tsx}`, `Panes.tsx`, `lib/web-views.ts` (`minimised`), `App.tsx` (`show_web` as pill), iPad sidebar collapse |

- Tests: minimise/restore keeps the view (no `web_view_close`); a `show_web` while the composer has focus becomes the
  pill. iPhone simulator: minimise, send a message, restore: same scroll position and history.

### P5 — a form of the page in the chat (D13)

| Engine | Shell | UI |
|---|---|---|
| `crates/browser/src/{show_element.rs, tile/agent.js}` (tool, serialise, sanitise, fill, receipt); `crates/tools/src/{lib.rs, registry.rs}` (`APPROVED_BY_CARD`); `crates/sessionsync/src/events.rs`; `crates/agent/src/rpc_forward.rs`; fixtures; bindings | none (runs through `web_view_call` op `script`) | `components/chat/ElementFormCard.tsx`, `lib/element-form.ts` (`formBootstrap`, `answerElementForm`), card registration with the transcript, `element_forms_pending` on attach |

- New names: tool `browser_show_element { ref?, selector?, title? }`; events `ElementFormRequested`,
  `ElementFormResolved`; RPCs `answer_element_form`, `element_forms_pending`; `Tool::APPROVED_BY_CARD`;
  attributes `data-theuth-form`, `data-theuth-field`; messages `theuth/collect`, `theuth/submit-request`.
- Tests: serialise a fixture sign-in form (no scripts, no `on*`, no values, password stays `type=password`); refuse an
  iframe and an oversize element; sanitiser allowlist; stale page → nothing typed; receipt carries no value (assert
  the test password appears in no event, log line or `ToolOutput`); a `read_only` cap cannot see the tool; the card's
  CSP blocks a network image (UI, jsdom); collect by index.
- iPad simulator: a login form served from the Mac (HTTP test server on the LAN); the agent, attached to the tile and
  separately on obscura, calls `browser_show_element`; the user types in the card and presses Submit; the server logs
  the posted fields and the receipt names the post-login page.

## 11. Open questions

- Q1. Session cookies (no `expires`) in the profile outlive the browser that set them, the way "continue where you
  left off" does. Keep them until the site row goes 30 days without a write, or drop them at save?
- Q2. Should cookies brought in by `browser_import_cookies` be promotable into the profile with an explicit flag?
- Q3. Obscura sends its own user agent (stealth, `BrowserConfig.user_agent`, `crates/browser/src/lib.rs:46-50`) while the
  session was minted by WebKit. Should a profile-loaded isolate present the tile's user agent for that site?
- Q4. `show_web` from a session owned by another engine does not reach a front-door client as read (§4.4). Fix it in
  205 (relay `WebViewOpened`) before P3, or in P3?
- Q5. 420 px as the iPad collapse width is a guess; measure the address field at the narrowest tile.

- Q6. The engine-side sanitiser for D13: a maintained allowlist crate (for example `ammonia`) or a small own
  allowlist over `html5ever`? Neither dependency was checked in the workspace.
- Q7. `APPROVED_BY_CARD` is a new permission shape (a tool's own card standing in for the policy's `Ask`). Accept it,
  or keep `browser_show_element` `Mutating` with a second press under the default policy?

Decided by the owner's "all good" on 2026-11-22 (orchestrator's proposals): Q1 keep session cookies until the site row
goes 30 days without a write. Q2 no: imported cookies stay out of the profile. Q3 obscura keeps its own user agent.
Q4 fix in P3 (relay `WebViewOpened` with the new events). Q5 measure on the simulator in P2. Q6 the P5 engine builder
checks the workspace and picks; report the licence. Q7 accept `APPROVED_BY_CARD`. Additive wire changes for 209 are
accepted with the build order ("build it and get it merged"); the orchestrator runs `just wire-schema-accept` at each
phase's integration, builders do not. The obscura change of §4.3 is made by a separate worker on the fork branch
`theuth/209-host-only` (cut from ee30f89); the P1 engine builder only repoints `Cargo.toml` at that branch once it is
pushed and must not edit obscura sources.

## 12. Unverified

Taken from the reports and not checked in source or at the URL for this document:

- WebKit: `WKHTTPCookieStore` methods and observer, `dataStoreForIdentifier:` (iOS 17 / macOS 14), HttpOnly and
  session cookies returned, partition key lost through `NSHTTPCookie`, reload needed before `document.cookie`
  reflects a set, `fetchDataRecords` without contents (web §1, WebKit headers on GitHub).
- `WKContentWorld` and `callAsyncJavaScript` semantics, CSP not applying to them, `takeSnapshot`, no trusted input,
  `isInspectable` with no Input domain (web §3).
- Tauri 2.4 `cookies()`, 2.8 `set_cookie()`; wry 0.47/0.53 APIs, the 0.50.3 and 0.54.3 fixes; Android returning an
  empty list; WebView2 `with_profile_name`, `ICoreWebView2CookieManager`, `CallDevToolsProtocolMethod` and trusted
  `Input.dispatchMouseEvent`; WebKitGTK cookie and world APIs (web §2–3).
- Cloudflare `cf_clearance` set Partitioned and its 30-minute default passage; DBSC in Chrome 145; Browser Use's
  warning; Browserbase, Steel, Hyperbrowser, Operator, Claude in Chrome descriptions; Telegram's pill (web §4–6).
- The OWASP quotation (web §5).
- Mesh key distribution (`crates/mesh/src/keys.rs:1-5,85-120`), the relay being stateless
  (`crates/relay/src/main.rs:1`), the hub lease (`crates/link/src/follow.rs:1-15`), the link `devices` table as the
  owner set (recon §7).
- `App.tsx:572,578` handling of web view and live view events, `ArtifactPanel.tsx`, `Header.tsx:274-280`,
  `popout.ts:147` (recon §4, §6).
- obscura is V8 plus its own DOM, not Chromium, with an unused CDP server (recon §2).
- Whether replica imposes a size limit on one record or batch (not found in `crates/replica/src/engine.rs`).
- Where the routes store keeps a learned `cookie` header, and whether it is sealed.
- Whether the relay can read RPC payloads.
- That a native `WKWebView` on macOS reached through `Webview::with_webview` accepts `callAsyncJavaScript` in a
  named world while Tauri also drives it (205 §8 says nobody has run the desktop native layer).
- obscura's `getComputedStyle` completeness for D13 (it exists, `obscura-js/js/bootstrap.js:4064`, `obscura-render/src/dom.rs:322`;
  how many properties it resolves was not checked), and whether `browser_eval` can return a 256 KiB string.
- That the RPC layer keeps `answer_secret`'s params out of logs; D13 copies that behaviour and it was not traced.
- That `just ts-desktop` writes only `crates/desktop/ui/src/rpc/bindings/**` (from the 205 build notes, not checked).
