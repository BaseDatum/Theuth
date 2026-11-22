# 205 — Web view tile

Status: built 2026-11-21 on `feat/205-web-view` (§8 says what differs from the design) · extends 120 (inline pages), 25 §tiles (`lib/panes.ts`), 70 (pop-out windows), 79 (why not an http iframe)

## 1. Context and problem

An agent that starts a dev server, a dashboard or a docs site can only tell the user the address. `show` hands
over files; `browser_show` shows PNG frames of the agent's own headless browser. Nothing puts a live page the
user can click beside the chat.

The owner's request (2026-11-21): a tool, in the `show` family, that inserts a web view frame as a tile in the
sessions view and shows a web page or something running on localhost; the frame pops out into its own window
with the same button style as everywhere else.

## 2. Goals, and what is not one

- G1. One tool call opens a URL in a tile next to the chat that called it.
- G2. Any site renders, not only those that allow framing. About a third of the top million domains send
  `X-Frame-Options` or CSP `frame-ancestors` (SANS ISC diary 33068, 2026: 29.7 %; top 100k: 37.4 %), and those
  are the ones with a login. An `<iframe>` cannot be the mechanism.
- G3. The framed page is a stranger: no Tauri IPC, no app origin, no app schemes, its own cookie store.
- G4. Pop-out into a `popout-*` window with the app's toolbar; the same buttons as every other pop-out.
- G5. The transcript keeps a card that reopens the tile (tiles are in memory only, `lib/panes.ts:1-2`).
- Not a goal: the agent reading or driving the page (that is `browser_*` and `computer_*`); tunnelling the
  engine's localhost to a phone or a remote client; tabs, history UI, downloads, devtools.

## 3. Decisions

| # | Decision |
|---|---|
| D1 | A new direct tool `show_web {url, title?}` in `crates/artifact`, risk `read_only`, sibling of `show`. Not a `url` argument on `show`: every existing consumer of a `show` call expects an artifact. |
| D2 | The desktop frame is a **native child webview** (Tauri `unstable` multiwebview: `Window::add_child`), positioned over a placeholder `<div>` the UI measures. Not an iframe: `frame-src` stays as 120 §9c left it, without `http:`/`https:`. |
| D3 | The framed webview has label `web-<id>`, matches no capability, loads only `http:`/`https:` (an `on_navigation` allowlist), and uses its own data store, shared by all web views and separate from the app's. |
| D4 | Frame identity is `id` = first 16 hex of `sha256(session ‖ "\n" ‖ origin)`: a second `show_web` for the same origin in the same session navigates the existing tile instead of opening another. |
| D5 | The tile is a new leaf kind in the pane tree, the string `web:<id>` (like `draft:<uuid>`), with its payload in a separate store. It is placed to the right of the calling session's tile, only when that session is visible in the main window and the device tiles (not a phone). |
| D6 | Pop-out is an app route `/web/$id` in a `popout-web-<id>` window opened by the existing `open_window`; the route renders the same frame component, which hosts its own child webview in that window. The tile closes when it pops out; the live page moves with it (`reparent`), it does not reload. |
| D7 | Where there is no native layer (web client, iOS) the tile and the card show the address and **Open in browser**; no iframe. |
| D8 | A loopback or private address needs the call's LAN grant (`tools::egress::lan()`), like `browser_*`. `169.254.0.0/16` and credentials in the URL are always refused. |

## 4. The contract

### 4.1 Tool (crates/artifact)

```
show_web { url: String, title?: String }      // DIRECT, Risk::ReadOnly, RETURNS "dict"
```

- `url`: absolute `http://` or `https://`; a bare `localhost:3000` or `127.0.0.1:3000/x` gets `http://` prepended.
  No userinfo. Host must be present.
- `title`: tile and card title; default the URL's `host[:port]`.
- Receipt (content): `web view opened · <title> · <url> · tile web:<id>` and, when no client is attached
  (`preview::feed_live()` is false), `\nnote: no app is connected; the card in the chat opens it later`. It ends
  with `\nThe user sees and clicks this page; you do not. Read a page with browser_* or read_url.`
- `structured`: `{ "id", "url", "title", "session" }`.
- Errors are `ToolOutput::err("show_web: …")`: not http(s), credentials in the URL, no host, link-local host,
  local network not allowed for this call.

### 4.2 Event (crates/artifact, mirrored to clients)

```rust
#[serde(tag = "type")]
pub struct WebViewOpened { pub session: SessionId, pub call_id: String, pub id: String, pub url: String, pub title: String }
impl wire::WireEvent for WebViewOpened { const TOPIC: &'static str = "artifact.web-view-opened"; }
```

Wire shape `{ "type": "WebViewOpened", "session", "call_id", "id", "url", "title" }`, client topic
`artifact.web-view-opened@1`. Registered in `declare_wire`, republished by the client mirror.

### 4.3 Shell commands (crates/desktop/src/webview.rs)

All take `label: String` that must match `^web-[0-9a-f]{16}$`; anything else is an error.

| Command | Arguments | Does |
|---|---|---|
| `web_view_attach` | `window: tauri::Window` (the caller's), `label`, `url`, `x`, `y`, `width`, `height` (logical px, f64) | Creates the child webview in the caller's window at the rectangle, or, when the label exists, reparents it to the caller's window if needed, moves it there and shows it. An existing webview is **not** navigated. |
| `web_view_bounds` | `label`, `x`, `y`, `width`, `height` | Moves and sizes it. |
| `web_view_visible` | `label`, `visible: bool` | Shows or hides it without destroying it. |
| `web_view_navigate` | `label`, `url` | Loads the URL (http/https only). |
| `web_view_reload` | `label` | Reloads. |
| `web_view_history` | `label`, `delta: i32` (-1 or 1) | Back or forward. |
| `web_view_close` | `label` | Destroys it. Closing an unknown label is `Ok`. |

Event to every window: `web-view://state` with payload `{ label, url, title, loading }` on navigation start,
page load finished and title change.

Rules: `on_navigation` returns `true` only for `http:`/`https:`; a new-window request (`target=_blank`,
`window.open`) opens in the system browser; the label is in no capability and no `remote` block exists; child
webviews are closed when their parent window closes. As built (§8): Tauri attaches its IPC handler to every
webview, so isolation rests on the origin allowlist, and one constant `initialization_script` keeps
`target=_blank` links alive.

### 4.4 UI (crates/desktop/ui/src)

- `lib/web-views.ts`: zustand store `{ views: Record<id, { id, session, url, title, current, loading }> }` with
  `open(view)`, `close(id)`, `patch(id, …)`; `webLeaf(id)` = `"web:" + id`, `isWeb(leaf)`, `webId(leaf)`.
- `lib/panes.ts`: a leaf may be `web:<id>`; `normal` and every `leaves()` consumer that expects session ids
  skips web leaves (`App.tsx:483`, `:611`, `SessionList.tsx:295`).
- `components/chat/WebFrame.tsx`: toolbar + placeholder. Toolbar, 52 px like `Header.tsx`: back, forward,
  reload, address field (Enter navigates), open in browser (`openExternal`), pop out (`AppWindow`, the classes of
  `Header.tsx:369-379`, hidden when `!canPopout()`), close (`X`, `Header.tsx:381-386`). Lucide icons, shadcn
  `Button`, `Tooltip`; no emoji.
- The placeholder owns the native layer: on mount `web_view_attach`; `ResizeObserver` + window resize/scroll →
  `web_view_bounds` (one call per animation frame); on unmount `web_view_visible(false)`; store `close` →
  `web_view_close`. While a pane drag is in progress, or any dialog, sheet, popover or menu is open over the
  frame, the native layer is hidden and the placeholder shows the title and address (a native webview draws
  above all DOM).
- `components/chat/WebViewCard.tsx`: the transcript card of a `show_web` call (title, address, Open, pop out,
  open in browser); `show_web` joins `DELIVERABLE_TOOLS`.
- `lib/popout.ts`: target `{ kind: 'web', id, url, title }` ↔ `/web/$id?url=…&title=…`, label `popout-web-<id>`;
  `routes/web.$id.tsx` renders `WebFrame` full-window.
- `App.tsx`: `on("artifact.web-view-opened@1", 'WebViewOpened', …)` → store `open`; if a pop-out for the id
  exists, navigate it; else if the tile exists, navigate it; else place the tile right of the session's leaf
  when that session is visible and the device tiles.
- Remote engine: when the engine is not this machine and the host is loopback, the frame shows "This address
  is on <engine>" with **Load anyway** instead of loading.

## 5. Security notes (G3)

- The page never shares a process-level bridge with the app: no capability names `web-*`, remote origins get
  no IPC without a `remote` capability block, and none exists. A test asserts no capability file matches
  `web-*` or contains `remote`.
- `on_navigation` stops `tauri:`, `asset:`, `theuth-page:`, `file:`, `data:` and `javascript:` top-level loads.
- The tool's check is not a network guard: the page can redirect anywhere. That is acceptable because the
  viewer is the user's own browser engine, sandboxed like Safari, with no access to the app. Taint (120 §9f):
  `show_web` exfiltrates only what the model puts in the URL, like a markdown link; it is link-class, not sink-class.
- `tools::egress::lan()` false (a capped contact turn) refuses loopback, RFC 1918, CGNAT, ULA and `.local`/`localhost` hosts.

## 6. Changes by place

- Engine: `crates/artifact/src/{webview.rs (new), tools.rs (PROMPT paragraph), lib.rs}`, `crates/rpc-compose`
  (mirror, golden fixture), `crates/theuth/tests/fixtures/wire/openrpc-v1.json`.
- Shell: root `Cargo.toml` (`tauri` feature `unstable`), `crates/desktop/src/{webview.rs (new), lib.rs}`.
- UI: the files of §4.4 and their tests.

## 7. Tests and gates

`cargo test -p artifact`, `cargo test -p rpc-compose --test wire_golden`, `cargo test -p theuth --test wire_schema_snapshot`,
`cargo check -p desktop`, and in `crates/desktop/ui`: `bun run check`, `bun run test`.

## 8. As built (2026-11-21)

Built by three workers in one worktree against §4, reviewed once (two readers: Rust and security, UI
lifecycle), one fix round. What differs from §1–§7, and what was learned:

**Isolation is by origin, not by "no IPC" (§4.3, §5).** Tauri 2.12.5 attaches its IPC handler and every plugin's
init script to every webview (`manager/webview.rs:555`, `:159-204`), and a child webview takes the capability of
its *window* (`ipc/authority.rs:476-478`). A remote origin can still invoke nothing, neither plugin commands nor
this app's own (`webview/mod.rs:2080-2108`; no capability has `remote`). A *local* origin would get the main
window's rights, so `allowed()` in `crates/desktop/src/webview.rs` refuses `tauri.localhost`, `ipc.localhost`,
`asset.localhost`, `theuth-page.localhost` and, in dev builds, the `devUrl` origin (a dev build cannot frame
`http://localhost:1420`). `on_navigation` uses `navigation_allowed()`: the same, plus `about:blank`,
`about:srcdoc`, `data:` and `blob:` (inner origin checked), because wry asks for every frame on macOS and Linux.
On Windows WebView2 asks for top-level navigations only (`wry webview2/mod.rs:754-772`): a hostile page could
frame the app's origin there (clickjacking at worst). Open.

**The `asset:` scheme is the app's own handler now.** Tauri registers `asset` on every webview and answers
with `Access-Control-Allow-Origin` = that webview's origin, so a framed page could read the asset scope.
`crates/desktop/src/asset.rs` is a copy of `tauri-2.12.5/src/protocol/asset.rs` registered app-wide (Tauri then
skips its own), whose only intended difference is 403 for `web-*` labels. Every image of the main UI loads
through it: re-check it on each Tauri upgrade.

**Links.** tauri-plugin-opener's injected click handler swallows `target=_blank` clicks and its invoke is
refused for a remote origin; one constant init script turns those clicks into `window.open`, which
`on_new_window` sends to the system browser (http/https only, at most one per second per view).

**Lifecycle contracts added in the fix round.**
- `web_view_attach` refuses to take a view out of another `popout-*` window: error `in-popout: <window>`.
- `web-view://closed {label}` to every window when a view is destroyed (`web_view_close`, or its window closed).
- Commands on a missing label fail with `no web view …`.
- The UI queues every native call per label on one promise chain and re-applies the wanted visibility and
  rectangle after attach (`lib/web-view-native.ts`); Tauri runs async commands unordered.
- Bounds follow the placeholder by one animation-frame loop that sends only on change.
- The main window keeps one listener pair for state and closed events; a pop-out closes itself when its view goes.

**Tool.** `show_web` normalises a bare address by scheme prefix, flattens the title to one line of at most 120
characters, refuses URLs over 2048 bytes, and treats single-label hosts and `.lan`, `.home.arpa`, `.internal`,
`.localdomain`, `.intranet`, `.corp`, `.home` as local network. The card reads id, address and title from the
receipt's first line (the transcript store keeps no structured result), so that line is a contract
(`WebViewCard.tsx`, pinned by a test in `crates/artifact/src/webview.rs`).

**Panes.** A web leaf behaves like a draft in `normal`, never takes focus and never names the URL. A chat that
is the only chat pane beside a web tile has no close button. A left-most web tile carries the sidebar toggle,
the traffic-light inset and the drag region.

**Not verified.** Nobody has run the native layer: coordinates under the overlay title bar, stacking above the
main webview, `hide`/`show`, `reparent` keeping the page, `on_new_window` and title events for child webviews,
and the copied `asset:` handler are read from source only. The separate data store needs macOS 14 or later;
older systems share the default store. Deleting the chat named in the URL while a web tile sits beside it is
untested.

## 9. iOS shell (2026-11-21)

Replaces D7 for the iOS shell on iPad: the tile hosts a native page there too. The web client keeps the
address card, and so does the phone, which does not tile (D5): its card offers **Open in browser** only.

**What was built.** `crates/desktop/src/ios_webview.rs` implements the seven §4.3 commands with the same
names, arguments, error strings and events. Tauri cannot add a child webview on iOS (`Window::add_child` is
desktop-only, wry's `build_as_child` is unsupported there), so each `web-<id>` is a raw `WKWebView` built
through UIKit (objc2): a sibling of Tauri's `WKWebView`, inserted directly above it in the same superview, framed
at the placeholder's rectangle converted through Tauri's scroll view (`zoomScale`, content offset and insets),
clipped to it. All UIKit work runs on the main thread (`Webview::with_webview`); views and their delegates live in a
main-thread registry. The pure rules (label, allowlist, throttle, error prefixes, data store identifier) moved to
`crates/desktop/src/web_rules.rs`, shared with the desktop's `webview.rs`. `nativeWebViews()` is true in the iOS
container (`lib/web-view-native.ts`); the pop-out button was already absent there (`canPopout()`).

**Isolation.** Each view has its own `WKWebViewConfiguration` with the persistent data store of the desktop's
identifier (`dataStoreForIdentifier:`, iOS 17+; the app's minimum is 17.4) and nothing else: no script message
handler, no URL scheme handler, no user script. It is not a Tauri webview, so it has no IPC, no `tauri:`/`asset:`/
`theuth-page:` scheme and no plugin init scripts; the desktop's IPC fence, `asset` check and links script have no
counterpart and are not needed.

**Differences from the desktop.**
- Navigation policy (`decidePolicyForNavigationAction`): the main frame passes `allowed()` only (http/https,
  never an app origin); subframes pass `navigation_allowed()` (adds `about:blank`/`srcdoc`, `data:`, `blob:`).
  The desktop applies `navigation_allowed()` to every frame.
- New-window requests have one owner, `createWebViewWithConfiguration…`: it opens the URL with
  `UIApplication openURL:options:completionHandler:` (http/https, one per second per view) and returns nil. The
  navigation policy allows a navigation without a target frame (`target=_blank`) so that WebKit reaches it, and
  opens nothing itself. `javaScriptCanOpenWindowsAutomatically` is set (iOS defaults to NO, which drops a
  `window.open` outside a user gesture without any delegate call). No links script is needed: no opener plugin
  script swallows the click.
- A main-frame response WebKit cannot show (`canShowMIMEType` false, or `Content-Disposition: attachment`) is
  cancelled in `decidePolicyForNavigationResponse` and opened in the system browser under the same throttle.
- JavaScript `alert`, `confirm` and `prompt` are `UIAlertController`s titled with the asking frame's host,
  presented from the top-most view controller of the view's window. Each completion handler is called exactly
  once: by the tapped action, or with the default (`alert` returns, `confirm` false, `prompt` nil) when the alert
  cannot be presented or goes away unanswered.
- `webViewWebContentProcessDidTerminate` reloads the page and publishes state (`loading: true`).
- `web_view_history` calls `goBack`/`goForward` instead of evaluating `history.back()`; a missing label is
  `no web view` before the delta is checked, as on the desktop.
- State events: the policy's allowing a main-frame navigation publishes its target URL; navigation start and
  commit (`loading: true`), finish and failure (`loading: false`), and KVO on `title` and `URL` (title changes and
  `pushState` without a navigation). Until the navigation commits, payloads carry that target URL; after a
  provisional failure they keep it (the address field shows what failed, as on the desktop) until the view's own
  URL changes outside a load. Otherwise URL and title are read from the view at that moment.
- Commands wait at most 5 s for the main thread (`the main thread did not run the web view call within 5 s`).
- One window: no reparenting, `in-popout` is unreachable, no `web-view://closed` from a window closing.
- The view is not inside the DOM, so the occlusion rules of §4.4 (hide under dialogs, sheets, menus) apply
  unchanged; the iPad sidebar is inline at tile widths, and as a sheet it is a `role="dialog"` overlay.

**Review round.** `objc2-web-kit` 0.3.2 declares the delegate protocols' methods for macOS only, so the
compiler checks none of the delegate selectors or signatures on iOS; a debug assertion at creation checks that
the delegate answers `respondsToSelector:` for each of the twelve it implements. The KVO registrations carry a
context pointer of their own and the observer ignores any other. A view replaced under the same label is
destroyed (observers removed, delegates cleared, off screen) rather than dropped. Not done: nothing re-derives a
view's frame when the app's geometry changes without the placeholder's CSS rectangle changing (Stage Manager
resize, rotation changing the safe-area insets); the frame follows only the UI's `web_view_bounds` calls.

**Not verified.** Proven on the iPad simulator only. After the review round, seen there: the address field shows
a slow page's target while it loads; `alert`, `confirm` (OK → `true`) and `prompt` (OK → the field's text,
Cancel → `null`) as native alerts over the tile; a `window.open` two seconds after a page load with no tap, a
`target=_blank` link and a `Content-Disposition: attachment` response each open Safari once, the tile keeping its
page. Untested: the content-process reload (no way to kill the process on demand), the 5 s timeout, the
replaced-view path (not reachable from the UI), rotation, Stage Manager window resizing, the on-screen keyboard
shifting Tauri's scroll view while a tile shows, memory with several views, and a real device.

## 10. In the inbox (2026-11-22)

Every `show_web` page is also an Inbox item (207's list), kind `web_view`, label "web page".

- **Source.** `artifact::webview` keeps, per tile, `opened_ms` (wall clock of the latest `show_web` for it; a
  client's `web_view_state` report keeps it) and publishes nothing new: a process-wide `watch` (`watch_tiles`) is
  bumped when a tile opens, closes, is forgotten (session deleted, tree closed) or reports another address or title.
  `open_tiles()` lists the tiles a `show_web` opened (a tile only a client reported — the engine restarted under it —
  makes no item). The Attention actor watches that channel (one recheck per 250 ms burst) and `gather` reads
  `open_tiles()`.
- **Item.** Id `web_view:<tile id>`: the tile id is `sha256(session ‖ origin)` (D4), so the same origin in the same
  chat stays one item. Title: the page's current title, else its `host[:port]`; `detail`: the current address;
  `session` the opener (lineage, project, path as every session item); `waiting_since_ms` none; `at_ms` = `opened_ms`.
- **Score.** Base class Shown (120), no live bonus (the page waits on nobody), task and age points as usual — always
  below an approval or a question. No alert.
- **Lifetime.** Dismiss and snooze are 207's hidden kinds (versioned): a later `show_web` for the same origin moves
  `opened_ms` and the item is back. It leaves when the tile closes (`web_view_state{closed}`), its session is
  deleted, or the engine restarts (tiles are memory-only on both sides).
- **Wire.** `AttentionKind` gains `web_view` (a new `oneOf` member in `attention_list`, `roster.attention@1` and the
  bulk filter's `kinds`: the classifier says `ReviewRequired`, "union extension is not a proven tagged variant", the
  only finding besides doc text; accepted, because every consumer has a fallback — the desktop UI renders an unknown
  kind with the plain screen, the companions decode it to `.other`). No new field: the address rides in `detail`.
- **Detail pane.** `components/attention/WebViewScreen.tsx`: Source, title and address, then the page — the frame's
  own native view `web-<id>` laid over the pane through `useNativeLayer` (the tile's machinery), so leaving the item
  or the page hides it and the chat's tile takes it back when shown — and a footer: Open in chat (the session's
  route, then the tile beside it), Pop out, Open in browser, Copy URL, Reload, Snooze, Dismiss. A pop-out window
  holding the page leaves a "This page is open in its own window" face. Without a native layer (a browser, the dev
  fixture) the pane shows `FrameFace`: the address with **Open in browser** (D7), the size the page would be.
- **Not verified in the Tauri app**: the native view over the inbox pane (position under the graph region and the
  header, hide on item change, the chat tile taking it back) is read from the tile's code, not seen.
