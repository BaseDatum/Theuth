# 120 — Inline sandboxed pages (visual replies)

Status: design · 2026-10-07 · extends 35 (artifacts: `show`, kinds, store, viewer; amends §9 and
§14.3), 30 §6 (prompting) and 70; follows the T3 Code recon of 2026-10-06 and the design-120 spikes
of 2026-10-07. Every theuth `file:line` was read on `main` `ebac3603` (main moved to `4a44a8d6`/`78f192f9` while this was
written — only `justfile` shifted, and its cites are at `4a44a8d6`); T3 Code lines were read in the
clone `/tmp/recon-t3code` at `c9b21b28`; Tauri/wry/jsonrpsee lines in
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` (`tauri-2.11.5`, `tauri-utils-2.9.3`,
`wry-0.55.1`, `infer-0.19.0`, `jsonrpsee-server-0.26.0` — the versions in `Cargo.lock:20952-20953`,
`:21374-21375`, `:25656-25657`). Nothing in this document has been executed; §15 lists the probes
still owed and §13 makes the security probe a gate. Where this doc disagrees with 35 about what an
`.html` artifact does, this doc wins.

## 1. Context and problem

**What T3 shipped (2026-10-05, PR #15968 `677d1527`; Mermaid fences `5e35272f` 10-03; scroll-trap fix
`64275ae3` 10-06; browser-on-environment-server `ac8e9453` 10-06).** Two MCP tools. `html_render(html
≤ 512 000 chars, title ≤ 200, height 80–2000)` publishes ONE self-contained HTML page — inline
`<style>`/`<script>`, CDN libraries allowed — as a thread attachment shown inline above the final reply
(`apps/server/src/mcp/toolkits/html/tools.ts:18-23`, `:74-94`; `apps/web/src/session-logic.ts:701-715`).
The page is served as a real `text/html` document under `Content-Security-Policy: sandbox allow-scripts
allow-forms allow-popups` (`apps/server/src/http.ts:56-61`) into `<iframe sandbox="allow-scripts
allow-forms">`, never `allow-same-origin` (`apps/web/src/components/files/BrowserDocumentFrame.tsx:122-129`).
A bootstrap injected at the start of `<head>` hands the page 40 CSS variables of the app theme and speaks
an MCP-Apps-shaped JSON-RPC over `postMessage`: host→page `ui/notifications/host-context-changed`,
page→host `ui/notifications/size-changed` and `ui/open-link` (`packages/shared/src/htmlRender.ts:269-274`,
`:335-344`, `:390-413`). The frame is as tall as the page reports, clamped 80–2000, capped by the
agent's `height` (`:110-132`). `html_preview(html, width 240–1600, appearance)` renders the page in a
pinned Chrome-for-Testing headless shell and returns a PNG, `contentHeight` and the console
(`tools.ts:25-57`; 20 messages × 500 chars, `apps/server/src/htmlRender/headlessChrome.ts:36-40`).
Local images named by absolute path are inlined as data URIs at publish, ≤ 10 MiB each, ≤ 25 MiB page,
magic-byte verified (`apps/server/src/htmlRender/HtmlRender.ts:38-40`, `:129-149`, `:171-185`).
Nothing goes from the page back to the agent or the thread.

**What theuth has.** More *structured* surface than T3 and nothing where T3 built: 38 typed json-render
components (`crates/surface/src/spec.rs:39-116`) validated by the engine with up to `correct_rounds`
`<surface_error>` corrections (`crates/agent/src/loop_.rs:2480`, `crates/agent/src/lib.rs:2418`, `:2426`);
openui fences and the live `app` tool; boards; mermaid 12 with export; Slack Block Kit, TUI and
Telegram/Signal degradation (110 §8, 35 §8). The one escape hatch — `show page.html` — stores the file
as `Kind::Html` (`crates/artifact/src/kind.rs:30`) unchanged (`crates/artifact/src/store.rs:733-736`) and
the desktop renders it only in the artifact panel as `<iframe sandbox="" srcDoc={CSP + text}>` with a
`<meta>` CSP of `default-src 'none'; style-src 'unsafe-inline'; img-src data: https: http:`
(`crates/desktop/ui/src/components/artifacts/Viewer.tsx:37-38`, `:337`; 35 §9 `35-artifacts.md:667-680`):
no scripts, no fonts, no fetch, no forms. The chat card never previews HTML text
(`crates/desktop/ui/src/components/chat/ArtifactCard.tsx:315`, `:407-411`); `.html` was never exercised
(`35-artifacts.md:980-986`). Nothing lets the agent see any of its own renderings except the PDF/SVG
preview PNGs `show` returns (`crates/artifact/src/store.rs:779-788`, `crates/artifact/src/tools.rs:84`).

**The gap.** The 20 % the catalog cannot draw — a chart library's chart, a map, 3D, a mockup, a collage,
an animation — has no inline home, and an agent that writes a page cannot check it. A `srcdoc`/`blob:`
document inherits the Tauri window's CSP (`crates/desktop/tauri.conf.json:48`: `script-src 'self'
'wasm-unsafe-eval' blob: ipc: http://ipc.localhost`, no `https:`), so CDN scripts could never load in the
current viewer even if the sandbox were loosened; the page must be its own document from its own origin,
as T3 does.

**Sky's rulings (2026-10-06 23:40; 2026-10-07 00:14).** Spikes first (done — knowledge
`facts/theuth-design-120-spikes-asset-exposure-page-origin-engine-route-2026-10-07`), then this design
covering B1 (inline sandboxed page via `show`), B2 (theme bridge), B3 (image inlining at publish),
B5 (prompt rules). The preview loop (B4) assumes the connected desktop WebView as its first backend —
a hidden sandboxed frame reporting height and console over RPC — no Chrome-for-Testing download, no
obscura rendering; both are named only as later options (§11). The S0 exposure found by the spikes is
not probed separately: its mitigations are a required section here (§9) and the probe is a gate (§13).

**Decisions taken (written as decided; sub-decisions in the sections named).**

| # | decision | where |
|---|---|---|
| D1 | Entry point stays `show`; an `.html`/`.htm` artifact is a live page inline (card) and full-size (panel); optional `height` caps the inline frame (80–2000), taller content scrolls inside; no new tool | §3, §6 |
| D2 | Origin: a dedicated Tauri scheme `theuth-page` serving the page as its own document (`text/html`, `CSP: sandbox allow-scripts allow-forms`, `nosniff`, `no-store`) into `<iframe sandbox="allow-scripts allow-forms">`; never `allow-same-origin`, `srcdoc`/`blob:` for scripted pages or the `asset:` scope; `srcdoc` only in a plain browser; the engine `GET /page` route deferred | §4 |
| D3 | Bootstrap injected at render time by the UI (artifact stays pristine); T3's message names and variable names; theuth adds `console` | §4.4, §5 |
| D4 | Theme bridge `useHtmlPageTheme()`: theuth tokens → T3's 40 variables; first paint from a URL fragment, updates by `postMessage` | §7 |
| D5 | Image inlining at store time for `Kind::Html`: absolute local paths → data URIs, ≤ 10 MiB / ≤ 25 MiB, magic-verified, `show` fails listing missing files | §8 |
| D6 | Prompt: pick order catalog → openui/app → page; page triggers and rules; "actions back are an `app`" | §10 |
| D7 | Hardening required here: ingest sniffing (demote to `File`), documents via typed `blob:` never `asset:`, `asset:` out of `frame-src`, asset scope narrowed to five roots, the probe as a gate | §9, §13.3 |
| D8 | Preview hook `page_preview` → connected desktop renders hidden, returns `contentHeight` + console; no screenshot; "no renderer connected" fast; PNG backends later | §11 |
| D9 | Channels unchanged (file upload / fenced source); the receipt names the audiences; iOS = same bundle, same scheme | §3, §12.4 |
| D10 | Lifecycle unchanged: ordinary artifacts, versioned by `(session, name)` (`crates/artifact/src/store.rs:812-815`), deleted with the session (`crates/artifact/src/lib.rs:637-651`), fetched from peers over `theuth/artifacts/1` (`crates/artifact/src/mesh.rs:1-10`) | — |

## 2. Goals, and what is not one

Goals:

1. `show page.html` renders the page **live** — scripts, fonts, fetch, CDN libraries — inline in the
   chat at reply-column width and full-size in the artifact panel, in the desktop, web and phone
   clients, themed like the app, sized to its content, inside an opaque origin that can reach nothing
   of theuth's (§3–§7).
2. A page written for T3's `html_render` renders in theuth unchanged: same variable names, same
   bridge messages (§5, §7).
3. Local images the agent names by absolute path travel inside the page (§8).
4. The exposure chain the spikes inferred (`show{path}` of HTML under a `.pdf` name → scripted
   document at the shared `asset://localhost` origin → `<data_dir>/token`) is closed and provably stays
   closed (§9, §13).
5. The model is told when a page is the right surface and how to write one (§10).
6. The agent can check a page's height and console in the connected desktop before publishing (§11).

Not goals, with the reason:

- **A page→agent channel.** T3 chose not to (no message is forwarded to server, composer or tool
  result); theuth already has `@ToAssistant` and `Mutation` in apps (`crates/app/src/tool.rs:97`).
  The prompt says so (§10).
- **Replacing json-render/openui with HTML.** The typed catalog is what gives validation, correction
  rounds, Slack blocks and TUI text; the page is the escape hatch, picked last (§10).
- **Multi-file bundles.** One document; images inlined (§8); `.css`/`.js` siblings stay unreferenced.
- **Persistent page state.** Opaque origin: no storage, no cookies, no `localStorage`; every mount is
  a fresh page.
- **Chrome-for-Testing download, obscura rendering, a user-installed browser for preview.** §11 names
  them as later options only (the ruling).
- **Mermaid changes.** theuth is ahead (strict, `htmlLabels: false`, DiagramFrame export); T3's
  DOMPurify pass is B6, a later note.
- **An engine HTTP route for pages** (`GET /page/<call_id>.<mac>` on the hyper loop, spike S4). Documented
  as a deferred option in §4.6; not built — it cannot reach the relay-served web/iOS client at all.

## 3. User-visible behaviour

**The agent's call.** `show{content, name: "chart.html"}` or `show{path: "/tmp/report.html"}` as
today (`crates/artifact/src/tools.rs:16-30`), plus an optional `height` (CSS px, clamped 80–2000):
the cap on the inline frame. The receipt (`crates/artifact/src/kind.rs:122-154`) gains, for
`Kind::Html`, the page facts and the audience line (§3.9).

**In the chat.** The `show` tool card keeps its position in the transcript — the deliverable card
that never folds into a tool run (decision 2026-10-06, `crates/desktop/ui/src/components/chat/ToolRunCard.tsx:20`, `:32`) — but for an
`.html` artifact the card *is the page*: a borderless frame on the transcript background, as wide as the
reply column (the same width as a json-render block), its left edge on the reply text, no card chrome,
no banner title. Above it the card header row stays (`⚙ show`) so the tool trace reads as today; below
it one caption line in `text-xs text-fg-secondary`: `title · HTML · 12 KB · v2`. The existing controls —
Reveal, Open in panel, Pop out, More (`ArtifactCard.tsx:441-457`) — move into a hover/focus toolbar at
the frame's top-right, the pattern `OpenUIFence` already uses (70 "hover/focus toolbar"), always
visible under a coarse pointer. Clicking the frame does nothing (the page owns its clicks); the caption
and the toolbar open the panel (`ArtifactCard.tsx:413-414`).

**Height.** The frame is reserved at `height ?? 240` px before the page loads, then follows the page's
`size-changed` posts, clamped to `[80, height ?? 2000]`; content taller than the cap scrolls inside the
frame with the page's own scrollbar hidden (§6). No animation on resize.

**Expand to panel.** Open in panel → the artifact panel (`components/chat/ArtifactPanel.tsx:183`) shows
the same page at the panel's full size through `ArtifactView` (`ArtifactPanel.tsx:169`,
`Viewer.tsx:281-287`), uncapped; the panel's version switcher and Reveal/Pop out/More stay
(`ArtifactPanel.tsx:147-149`). A pop-out window shows the page the same way.

**Show source.** The panel gains a two-state toggle `Page | Source` for `html`; Source renders the
stored bytes the way `text` artifacts render today (`Viewer.tsx:326-333`, fenced `html`). The card menu
keeps `Copy` (the source, `ArtifactCard.tsx:166-177`), `Save as…`/`Download`, `Open` (local only),
`Open in explorer`, `Attach…`, `Copy path` (`:178-208`). The stored artifact is pristine (§4.4): Save,
Reveal, Copy and every channel upload hand over exactly the bytes the agent wrote.

**Light/dark.** The page paints in the app theme from its first frame (fragment, §4.3/§7) and follows a
theme flip live (`postMessage`, §5); the frame itself is transparent, so a page that sets no background
shows the transcript's.

**Phone (~360 px).** Same component, same scheme (iOS is the same Tauri bundle,
`crates/desktop/tauri.ios.conf.json`); the frame is the chat column's width; the bootstrap adds
`<meta name="viewport" content="width=device-width, initial-scale=1">` when the page has none
(T3 `htmlRender.ts:354-356`). The page scrolls inside only when capped.

**Slack, Telegram, Signal, TUI.** Unchanged: the file is uploaded with the title as comment/caption
(`crates/slack/src/channel.rs:2203-2240`, `crates/telegram/src/channel.rs:2549-2596`,
`crates/signal/src/channel.rs:1612-1622`); the TUI shows the source fenced
(`crates/tui/src/model.rs:10167-10175`). A PNG of the page for those channels arrives with whichever
later design yields one (§11.6).

**Failure UI.** The page fails to load (handler 404 after eviction, scheme error, a page that throws
before first paint) → the frame's box stays the same size and shows `Unable to load <title>` in
`text-fg-secondary` with the toolbar still offering Open in panel and Show source (T3
`HtmlRenderFrame.tsx:132-136` wording). Console: `error`-level messages and uncaught exceptions the page
posts (§5.4) are counted in a small chip in the toolbar (`2 errors`) that expands to the list; `log`,
`info` and `warning` never reach the reader. The same messages are what the preview hook returns to the
agent (§11). While loading: the `bg-layer-2` pulse the PDF frame uses (`Viewer.tsx:273`).

**The receipt (§3.9).** `shown chart.html (v1 · 212 lines · 48 KB · 2 images inlined · height 600)` +
`live page in the theuth app (desktop, web, phone); Slack/Telegram/Signal get the file; TUI the source.
Check it with page_preview when it has scripts.` — one line of harness text, no claim about which client
is connected (the engine does not know).

## 4. Architecture

### 4.1 Data flow

```
engine                                   desktop UI (webview)                     Tauri (Rust)
──────                                   ────────────────────                     ─────────────
show{content|path, name:*.html, height}
  store: sniff kinds (§9a) → inline
  images (§8) → blob <sha16>.html →
  artifacts.sqlite row → ArtifactShown ──▶ ArtifactCard sees kind=html
                                           readText(api, a.path)        ──get_attachment──▶ (RPC; local or relay)
                                           injectBootstrap(text, theme)
                                           invoke page_put(call_id, html) ─────────────────▶ Pages state: token → html
                                                                          ◀── url ──────────  "theuth-page://localhost/<token>"
                                           <iframe sandbox="allow-scripts allow-forms"
                                                   src=url#theuth-theme=…>
                                                                                             theuth-page handler:
                                                                                               200 text/html; charset=utf-8
                                                                                               Content-Security-Policy: sandbox allow-scripts allow-forms
                                                                                               X-Content-Type-Options: nosniff
                                                                                               Cache-Control: no-store
                                           page ⇄ host postMessage (§5): theme · size-changed · open-link · console
plain browser (no __TAURI_INTERNALS__):    <iframe sandbox="allow-scripts allow-forms" srcdoc={bootstrapped html}>
```

The engine side is today's `show` path (`crates/artifact/src/store.rs:647-681` bytes in, `:690` kind by
name, `:701-757` per-kind arm, `:767-772` blob by `sha16`, `:812-815` version) with two additions in the
`Kind::Html` arm (§8 inliner, §9a sniffing). The UI already reads artifact bytes over RPC
`get_attachment` (`crates/desktop/ui/src/lib/files.ts:91-105`; `crates/rpc/src/api.rs:5701-5764`,
allowed roots `:5754`), which works for the local daemon and through the relay (the relay speaks
JSON-RPC records into `raw_json_request`, `crates/link/src/conn.rs:471`, `:790`). The one new seam is the
Tauri command + scheme.

### 4.2 The `theuth-page` scheme (D2)

`crates/desktop/src/lib.rs` registers `theuth-page` on the builder with
`register_uri_scheme_protocol` (tauri 2.11.5 `src/app.rs:2130-2151`), beside the existing plugins
(`crates/desktop/src/lib.rs:32-41`, `:243-288`), on desktop **and** mobile builders. A managed
`Pages` state (`Mutex<HashMap<token, Arc<[u8]>>>` with an LRU of 64 entries / 128 MiB) is filled by a
command `page_put(call_id, html) -> url` and drained by `page_drop(call_id)`; the handler answers:

| request | response |
|---|---|
| `GET theuth-page://localhost/<token>` (Windows/Android: `GET http://theuth-page.localhost/<token>`, wry's host form, `wry-0.55.1/src/custom_protocol_workaround.rs:11-43`; tauri `src/app.rs:2126-2127`, `src/manager/webview.rs:247-252`) with a known token | `200`, body = the bootstrapped page, headers `Content-Type: text/html; charset=utf-8` · `Content-Security-Policy: sandbox allow-scripts allow-forms` · `X-Content-Type-Options: nosniff` · `Cache-Control: no-store` · `Referrer-Policy: no-referrer` |
| unknown token | `404`, `text/plain`, no body |
| any other method | `405` |

Why each header: **`Content-Type`** must be set by the handler — wry copies the response headers
verbatim and defaults to nothing (`wry-0.55.1/src/wkwebview/class/url_scheme_handler.rs:218-245`), so a
missing type would be sniffed. **`CSP: sandbox …`** gives the *document* an opaque origin even when it is
loaded top-level (a pop-out, a future deep link) and is the second half of T3's double sandbox
(`http.ts:56-61`); the iframe attribute is the first and the intersection wins — no `allow-popups`:
wry's UI delegate returns no webview for `window.open` when the app sets no new-window handler, and theuth sets none (`wry-0.55.1/src/wkwebview/class/wry_web_view_ui_delegate.rs:148`, `:257-259`). The `sandbox` directive is header-only (a `<meta>`
is ignored by spec), hence a served document and not `srcdoc`. **`nosniff`** stops the content-type
re-sniffing that makes the asset protocol dangerous (§9). **`no-store`** because the body is per-token
and evicted. **No `Access-Control-Allow-Origin`**: the document is navigated to, never fetched, and an
opaque-origin frame must not be able to read sibling pages. The token is a fresh 128-bit random value per
`page_put`, never the `call_id`: a sandboxed page may navigate *itself* to another URL (visual only, same
sandbox), so page URLs must be unguessable.

Why a custom scheme and not what exists:

- **Custom-protocol documents never receive `security.csp`.** Tauri injects the configured CSP only into
  embedded frontend assets (`tauri-2.11.5/src/protocol/tauri.rs:177-184`, computed in
  `manager::get_asset`); user protocols pass straight through (`src/manager/webview.rs:230-242`). The
  handler owns its headers — exactly the isolation pattern Tauri itself uses: the isolation protocol sets
  `Content-Security-Policy` on a custom-scheme response with a hand-written per-platform `frame-src`
  (`src/protocol/isolation.rs:29-34`, `:59-62`) and loads it in `iframe.sandbox.add('allow-scripts')`
  (`tauri-2.11.5/scripts/isolation.js:11-15`) talking `postMessage` to the parent
  (`tauri-utils-2.9.3/src/pattern/isolation.js:16-18`, `:108-112`).
- **`srcdoc`/`blob:` inherit the parent's CSP** (CSP3 policy container), i.e. `tauri.conf.json:48`
  — no `https:` in `script-src`, so no CDN script ever loads; and both run in the *same* process
  policy as the app. Today's `sandbox=""` viewer is safe for that reason and useless for scripts.
- **The `asset:` protocol is the wrong origin.** Its scope is the whole data dir
  (`crates/desktop/src/lib.rs:322-325`, `allow_directory(launch::data_dir(), true)`); it answers with a
  fixed `Access-Control-Allow-Origin: <window origin>` on every response including errors
  (`tauri-2.11.5/src/protocol/asset.rs:13-27`, `:39`) and no CSP (the file has none, 238 lines); and its
  `Content-Type` is sniffed from the first 8 KiB by `infer` (`asset.rs:74-88`;
  `tauri-utils-2.9.3/src/mime_type.rs:80-98`) with `text/html` as the fallback for unknown extensions
  (`:71-72`, `:81`), where `infer` calls `<!DOCTYPE HTML`, `<HTML`, `<SCRIPT`, `<DIV`, `<P`, `<!--` … HTML
  (`infer-0.19.0/src/matchers/text.rs:5-32`). Anything served from it is same-origin with
  `secrets.vault`, `sessions.sqlite` and `token`. §9 removes documents from it altogether.
- **iOS needs nothing extra.** wry registers custom schemes through the same `WKURLSchemeHandler`
  path on macOS and iOS (`wry-0.55.1/src/wkwebview/mod.rs:167-171`); `asset:`/`ipc:`/`tauri:` already
  work on the phone with no `Info.plist` entry (`crates/desktop/ios/Info.ios.plist` declares none; 23
  lines). Linux (WebKitGTK) uses the `theuth-page://localhost` form too and marks the scheme secure
  (`wry-0.55.1/src/webkitgtk/web_context.rs:134-144`).
- **Windows/Android** rewrite the scheme to `http://theuth-page.localhost/<token>`
  (`custom_protocol_workaround.rs:21-27`); the UI never builds URLs — `page_put` returns the one to use.

### 4.3 The iframe

`<iframe sandbox="allow-scripts allow-forms" src={url + themeFragment} title={a.title}
className="block w-full border-0 bg-transparent" style={{height}} loading="lazy" />`. Never
`allow-same-origin` (opaque origin: no cookies, storage, or app session); never `allow-popups`,
`allow-top-navigation`, `allow-modals`. The theme fragment `#theuth-theme=<urlencoded JSON>` is read by
the bootstrap before first paint and stripped with `history.replaceState` (T3 `htmlRender.ts:306-311`;
the bootstrap does the same); a fragment is never sent to a server, so the handler never sees it — the
handler keys on the path alone and must not parse the query either.

`tauri.conf.json:48` `frame-src` becomes `'self' theuth-page: http://theuth-page.localhost blob:`.
Both forms by hand: tauri parses and serialises the CSP verbatim with no platform rewriting
(`tauri-utils-2.9.3/src/config.rs:2453-2491`), `'self'` never matches a custom scheme, and `blob:` stays
for the typed PDF frames of §9b. `asset:` and `http://asset.localhost` leave `frame-src` (§9c) and stay
in `img-src`.

### 4.4 Bootstrap at render time (D3)

The UI injects the bootstrap (§5) into the text it fetched, just before `page_put`, in a shared module
`crates/desktop/ui/src/lib/page-bootstrap.ts`. The stored artifact stays pristine: Save/Reveal/Copy,
Slack/Telegram/Signal uploads and the TUI all see the agent's bytes. The cost is that a page opened
outside theuth has no theme variables and no base stylesheet unless the page carries its own fallbacks
(§7.4); the prompt says to write them (§10). The preview hook (§11) injects the same bootstrap, so what the
agent checks is what the reader gets.

### 4.5 Plain browser fallback

Without `__TAURI_INTERNALS__` (`crates/desktop/ui/src/lib/tauri.ts:18`; the dev browser pointed at a
daemon, `:27-36`, or a relay-served web build) there is no scheme handler. The same component renders
`<iframe sandbox="allow-scripts allow-forms" srcdoc={bootstrapped html}>`. The parent CSP there is the
dev server's or the relay's, not Tauri's; CDN scripts load if that policy allows them. Same bridge, same
sizing, same opaque origin.

### 4.6 Deferred: the engine `GET /page` route (S4)

A tower wrapper outside `Auth` on the hyper accept loop (`crates/rpc/src/lib.rs:171-203`;
`Auth` `:241-262`) could serve `GET /page/<call_id>.<mac>` as `text/html` + the same headers:
jsonrpsee's `TowerService` already answers HTTP (`jsonrpsee-server-0.26.0/src/server.rs:1042-1044`
upgrade, else `transport/http.rs:88-112` POST-only), adds no CORS, and `hmac`/`sha2` are workspace
deps (`Cargo.toml:175-176`). It would serve the dev browser only: the relay-served web/iOS client has no
HTTP path to the engine (JSON-RPC records only, `crates/link/src/conn.rs:471`, `:790`). The Tauri scheme
fed by `get_attachment` bytes is therefore the primary mechanism and this route is not built. If it is,
decisions then: key by `call_id`; token-derived vs per-process HMAC key; liveness via `State::get`
(`crates/artifact/src/store.rs:1405-1414`); never an `Access-Control-Allow-Origin`.

## 5. The bridge protocol

JSON-RPC 2.0 over `postMessage`, T3's MCP-Apps-shaped names (`htmlRender.ts:269-274`) plus one theuth
notification. The host accepts a message only when `event.source === frame.contentWindow`; the page
accepts a message only when `data.jsonrpc === "2.0"` and the method matches (the bootstrap has no origin
to check: the host is `tauri://localhost` on macOS and `http://tauri.localhost` on Windows).

| direction | method | params | notes |
|---|---|---|---|
| host → page | `ui/notifications/host-context-changed` | `{ theme: "dark" \| "light", styles: { variables: { "--background": "#…", … } } }` | sent on mount (`onLoad`), on every `<html class>` flip (`lib/theme.ts:32-36` observer), and whenever the variable set changes; the page rewrites its `<style id="theuth-theme">` (T3 `htmlRender.ts:335-344`) |
| page → host | `ui/notifications/size-changed` | `{ height: number }` | `Math.ceil` of `documentElement.scrollHeight` when it exceeds `clientHeight`, else the root's `getBoundingClientRect().height`; from a `ResizeObserver` on `<html>` and `<body>`, on `DOMContentLoaded` and `load`; posted only when the value changed (T3 `:344`); host accepts finite `> 0` (T3 `:277-286`) |
| page → host | `ui/open-link` (request, `id: "theuth-link-<n>"`) | `{ url: "https://…" }` | the bootstrap intercepts trusted clicks on `a[href]`, resolves against `document.baseURI`, lets same-document hashes through, and asks for `http(s)` only (T3 `:344`); the host opens it (§5.3) and answers `{ jsonrpc, id, result: {} }` (T3 `:302-304`) |
| page → host | `theuth/notifications/console` | `{ level: "log" \| "info" \| "warning" \| "error", text: string }` | theuth's addition (§5.4); the inline frame counts `error`s; the preview hook returns them |

### 5.1 Injection rules

The bootstrap goes at the start of `<head>`, before the page's own styles and scripts, so first paint is
themed and a page's later `:root` rules still win (T3 `htmlRender.ts:335-339`). Position is found on a
scan copy where comments, raw-text elements (`script`, `style`, `textarea`, `title`, `xmp`, `iframe`,
`noembed`, `noframes`, `noscript`, `plaintext`) and `<template>` contents are blanked to the same length
(`:362-388`) so an inert `<head>` inside a comment or a string can never receive it; the splice is made
in the original text. Order of attempts: after `<head …>`; else after `<html …>` wrapped in
`<head>…</head>`; else after the doctype; else prepended with `<!doctype html>` (`:394-413`). The
markup is: `<meta charset="utf-8">` unless the first 4 KiB already declare a charset, the viewport meta
unless present, `<style id="theuth-theme">` with the §7.4 default stylesheet, `<script>` with the
bootstrap (`:346-360`). Variable names are whitelisted `^--[a-z0-9-]+$` and values stripped of `;{}<>`
before they are written into the stylesheet (`:344`), so a hostile theme payload cannot close the style
element. The bootstrap never touches `document.body`.

### 5.2 Sizing messages

See §6. The host clamps; the page never knows the cap.

### 5.3 Links

A page cannot open windows: wry answers `window.open` with no webview when the app registers no new-window handler (`wry_web_view_ui_delegate.rs:148`, `:257-259`), and the frame has no `allow-popups`. The host
opens an `open-link` URL with `window.open(url, "_blank", "noopener,noreferrer")` — in the Tauri shell
`window.open` is already routed to the system browser through `tauri-plugin-opener`
(`crates/desktop/ui/src/main.tsx:15-19`; `opener:default` in `crates/desktop/capabilities/default.json`)
— **only when** `event.source` is this frame's window, `document.activeElement === frame`, and
`navigator.userActivation.isActive !== false` (T3 `BrowserDocumentFrame.tsx:84-107`). A page can take
focus by script, so the gate stops opens on load, not a page that waits for the reader's next click; it
is the same bar the `app` host sets for `@OpenUrl` (`crates/desktop/ui/src/components/apps/AppHost.tsx:420-429`).
`javascript:`, `file:`, `theuth-page:`, `asset:` never pass (`^https?:` test on both sides).

### 5.4 Console (theuth's addition)

The bootstrap wraps `console.log/info/warn/error` (`warn` → `warning`), `window.onerror`,
`unhandledrejection` and posts each as `theuth/notifications/console` with `text` = the arguments joined
by a space (`String()` of each; objects via `JSON.stringify` with a 500-char cut), stack traces kept.
Caps: 20 messages per page load, 500 chars each (T3's numbers, `headlessChrome.ts:36-37`); past 20 the
page sends one final `{ level: "warning", text: "… N more console messages omitted" }`. The wrapper
calls the original console too, so devtools still work.

## 6. Sizing

Frame height, per mount, in order of authority:

1. **The page's own report.** The latest `size-changed` value, clamped to `[80, cap]` where
   `cap = show{height} ?? 2000`. T3's `64275ae3` made the page-reported height authoritative because the
   server's measurement was computed with its fonts, the client wrapped text taller, the frame came up
   short and scrolled inside the thread, stealing the reader's scroll (`HtmlRenderFrame.tsx:47-50`;
   `htmlRenderFrameHeight`, `htmlRender.ts:110-132`, prefers `contentHeight` whenever the client has
   one). The client's measurement is the only one that is right for the client's fonts and width —
   so theuth measures nowhere else.
2. **Before the first post.** The frame is reserved at `show{height} ?? 240` px so the transcript below
   does not move for a well-sized page; the first post then sets the real height. A page that never posts
   (a page that throws before the bootstrap runs; a frozen page) keeps the
   reserved height and still renders; the preview hook (§11) tells the agent the real `contentHeight`
   to pass as `height`.
3. **Taller than the cap.** The frame stays at the cap and the page scrolls inside it. The base
   stylesheet hides the page's scrollbar (`scrollbar-width: none`, `html::-webkit-scrollbar{display:none}`,
   T3 `htmlRender.ts:322-326`): a scrollbar inside a reply reads as a box within the thread. The rule
   only ever applies when the agent asked for a cap or the page exceeds 2000.
4. **Width.** The frame is `width: 100%` of the reply column; the page's layout rules (§10: fluid width,
   no outer padding, fixed pixel chart heights, no `100vh`) are what keep a width change from growing the
   page again and again. On a column resize the page's `ResizeObserver` re-posts and the frame follows.

No server-side width measurement: T3 measures nine widths 320…1144 only when its Chrome shell is already
installed (`htmlRender.ts:35`, `:101-108`) and uses them only before the page reports; theuth has no
renderer at store time by ruling, and the page report wins anyway. `show{height}` and
`page_preview.contentHeight` fill the gap.

The engine clamps `height` to 80–2000 at `show` time and notes a clamp in the receipt
(`height 3000 → 2000`); the UI clamps again. `height` is a `show` argument, not an artifact column:
tool arguments are persisted with the call (`HistoryEntry::Tool { args }`,
`crates/agent/src/lib.rs:1618-1624`) and the card already has them (`store/sessions.ts:103-107`
`args: unknown`); the panel and the explorer, which have only the record, show the page uncapped by
design. No migration, no `Artifact` field.

## 7. Theme bridge (D4)

### 7.1 Variables

`useHtmlPageTheme()` (new, `crates/desktop/ui/src/lib/page-theme.ts`) resolves the variables below from
`getComputedStyle(document.documentElement)` the way `diagramTokens()` does
(`crates/desktop/ui/src/lib/diagram-theme.ts:20-36`), re-resolves on the `<html class>` flip through
`subscribeDark` (`lib/theme.ts:32-36`), and returns `{ appearance: "dark" | "light", variables }`. Names
are T3's 40 (`htmlRender.ts:209-244`, agent guide `:250-259`) so a T3 page renders unchanged. T3's
`--accent` is the *brand* accent, not shadcn's neutral hover surface (`:195-199`); theuth's
`--accent` is the neutral (`styles.css:113`), so the bridge maps from the ochre token instead.

| T3 variable | theuth source (`crates/desktop/ui/src/styles.css`) |
|---|---|
| `--background` | `--background` (:101 → `--bg-page`, the transcript background) |
| `--foreground` | `--foreground` (:102 → `--fg-primary`) |
| `--muted` / `--muted-foreground` | `--muted` / `--muted-foreground` (:111-112) |
| `--card` / `--card-foreground` | `--card` / `--card-foreground` (:103-104 → `--bg-elevated`) |
| `--popover` / `--popover-foreground` | :105-106 |
| `--secondary` / `--secondary-foreground` | :109-110 |
| `--border` / `--input` / `--ring` | :116 / :117 / :118 |
| `--primary` / `--primary-foreground` | :107 / :108 |
| `--accent` / `--accent-foreground` | `--fg-accent` (:22, :68) / `--fg-inverse` (:21, :67) |
| `--accent-surface` / `--accent-surface-foreground` | `--status-warning-bg` (ochre at 12 %, :36, :82) / `--fg-accent-hover` (:23, :69) |
| `--destructive` / `--destructive-foreground` / `--destructive-surface` | `--status-danger-fg` / `--fg-inverse` / `--status-danger-bg` (:27-29, :73-75) |
| `--warning` / `--warning-foreground` / `--warning-surface` | `--status-warning-fg` / `--fg-inverse` / `--status-warning-bg` (:36-38, :82-84) |
| `--success` / `--success-foreground` | `--status-success-fg` / `--fg-inverse` (:35, :81) |
| `--info` / `--info-foreground` | `--status-info-fg` / `--fg-inverse` (:32, :78) |
| `--code-background` / `--code-foreground` | `--bg-layer-3` (:16 "code well", :62) / `--fg-primary` |
| `--chart-1` … `--chart-6` | `--fg-accent`, `--project-faience`, `--project-malachite`, `--project-lapis`, `--project-carnelian`, `--project-plum` (:41-50 light, :86-95 dark) |
| `--radius` | `--radius` (:100, `0.75rem`) |
| `--font-sans` | `--font-sans` (:191) with the same literal fallback `diagram-theme.ts:34` uses |
| `--font-mono` | no theuth token; the literal `"SF Mono", "SFMono-Regular", Menlo, Consolas, "Liberation Mono", monospace` (T3 `htmlRender.ts:157`: concrete names, since some engines alias `ui-monospace` to the UI font) |

Every value is a resolved colour string (the `var()` chains are resolved by `getComputedStyle`), so the
page never depends on theuth's token names.

### 7.2 Propagation

First paint: `#theuth-theme=<json>` on the scheme URL (or inside the `srcdoc` bootstrap call in the
fallback). Updates: `ui/notifications/host-context-changed` on `onLoad` (covers a flip that landed while
loading, T3 `BrowserDocumentFrame.tsx:130-134`) and on every observer callback. The bootstrap rewrites
one `<style id="theuth-theme">` element, never inline properties, so the page's own `:root` rules keep
winning by cascade order (§5.1).

### 7.3 Fonts

System fonts only from the bridge; a page may `@import` or `<link>` web fonts (its own document has no
`font-src` restriction — the sandbox directive constrains nothing but the sandbox flags). The theme bridge
does not ship theuth's bundled Newsreader (`styles.css:197-211`): the page is an opaque origin and
cannot read `/public/fonts`.

### 7.4 Outside theuth

The bootstrap's default stylesheet is the dark `:root` block, a `@media (prefers-color-scheme: light)`
light block, then the base CSS (`html{background:var(--background);color:var(--foreground);
font-family:var(--font-sans);font-size:14px;line-height:1.5;…}` `body{margin:0}` `code,kbd,pre,samp
{font-family:var(--font-mono)}`, T3 `htmlRender.ts:322-326`, `:346-351`) with theuth's two palettes
baked in — so the hidden preview frame without a fragment, a pop-out and a direct load follow the OS
appearance. But the *stored* file has no bootstrap (§4.4): opened from Save as… or a Slack upload it is
unstyled wherever it relied on the variables. The prompt therefore tells the model to give every
`var()` a fallback (`var(--foreground, #1a1612)`) or to set its own `:root` defaults (§10); a page that
follows that rule looks the same everywhere.

## 8. Image inlining at store time (D5)

In the `Kind::Html` arm (`crates/artifact/src/store.rs:733-736`), before the blob is written, a pure
function `inline_images(html, read) -> Result<Inlined, InlineError>` in a new
`crates/artifact/src/inline.rs`:

**Grammar** (T3 `HtmlRender.ts:141-149`, one regex, byte offsets kept). A match is either
(a) a whole quoted string — `"…"`, `'…'`, `` `…` `` — in an attribute or a JS literal whose content is an
absolute path ending in an allowed image extension, or (b) an unquoted CSS `url( … )` with such a path.
Absolute = POSIX `/…` but not protocol-relative `//…`, or Windows `[A-Za-z]:\…` / `[A-Za-z]:/…`
(inside a JS string a Windows path's backslashes are doubled and are un-escaped before the read,
`:157-159`). Extensions: `png jpg jpeg gif webp avif svg bmp ico` (`:129-139`). Path ≤ 2048 chars, no
newline. URLs (`https://…`), `data:`, `blob:`, `file:` and relative paths never match by construction.

**Limits.** Per image ≤ 10 MiB; the inlined page ≤ 25 MiB computed *before* reading (base64 is
`ceil(n/3)·4` + the data-URI prefix − the path's own bytes, `:254-265`) and again after; each read stops
one byte past the per-image limit (`:266-291`). The artifact cap stays `MAX_BYTES` 32 MiB
(`crates/artifact/src/lib.rs:46`) and is checked on the result.

**Verification.** Bytes are an image whatever the name: PNG `\x89PNG`, JPEG `\xff\xd8\xff`, GIF
`GIF8`, ICO `\0\0\x01\0`, BMP `BM` + reserved zeros, WebP `RIFF…WEBP`, AVIF/HEIF `ftyp(avif|avis|mif1)`
at offset 4, else an XML whose root element is `<svg` after any prolog, comments and doctype in a single
forward pass (`:171-185`, `:198-209`) — "so a symlink or renamed file cannot carry other data, such as a
secret, into a page" (`:167-170`). The MIME in the data URI follows the *extension* (T3 `:161-162`); a
`.png` that is a JPEG still inlines (browsers sniff images).

**Failure.** Any referenced file that is missing, unreadable, a directory or not an image makes `show`
fail with the T3 wording (`:43-49`): `show: chart.html: these local images could not be read:
/tmp/a.png, /Users/…/b.svg — use absolute paths to existing image files, or remove them`; an oversize
image: `…: /tmp/a.png is 12.3 MiB; images are limited to 10 MiB`; an oversize page: `…: inlined page
would be 27.1 MiB; the limit is 25 MiB`. No partial publish: the blob is written only after inlining
succeeds (today's order, `store.rs:767-772`).

**Why store time, unlike the bootstrap.** The page must outlive the files it names: `/tmp` screenshots
are gone tomorrow, a phone through the relay never had them, and a Slack upload has no second request.
The bootstrap is a function of the *reader's* theme and client; the images are a function of the
*author's* disk at publish — the two have different owners, hence two different times. The receipt counts
them (`· 2 images inlined`). This retires the `sips -Z` + base64-into-SVG trick orchestrators use to look
at PNGs; `.md` artifacts can get the same inliner later (open, §15).

**Taint.** The inlined bytes are files the model could already read; the page is shown to the same
audience as the reply. What is new is that a scripted page with network access can send its own content
anywhere — see §9f.

## 9. Hardening (D7) — the exposure chain and its closure

**The chain, inferred from source, not executed (knowledge entry 4, S0).** (1) `show{path: "/tmp/x.pdf"}`
stores the bytes as they are: kind by extension (`crates/artifact/src/kind.rs:20-40`), no content check
(`store.rs:701-757` has arms for compiled PDFs and SVGs only; a `path` PDF falls to `_ => bytes`,
`:756`), blob written `:767-772`. (2) On a mouse-driven desktop the panel renders a PDF as
`<iframe src={useFileUrl(a.path)}>` with **no `sandbox`** (`Viewer.tsx:270-275`, chosen at `:278`); the
chat attachment lightbox does the same (`components/chat/Attachments.tsx:107-114`); `useFileUrl` →
`resolveFile` → `convertFileSrc(path)` whenever the engine is local (`lib/files.ts:68-69`,
`Attachments.tsx:14-21`) = `asset://localhost/<data_dir>/artifacts/<sha16>.pdf`. (3) The asset protocol
sniffs the type from the first 8 KiB with `infer` (`tauri-2.11.5/src/protocol/asset.rs:74-88`;
`tauri-utils-2.9.3/src/mime_type.rs:80-98`), where `<!DOCTYPE HTML`, `<HTML`, `<SCRIPT`, `<DIV`, `<P`,
`<!--` … mean `text/html` (`infer-0.19.0/src/matchers/text.rs:5-32`); it sets a fixed
`Access-Control-Allow-Origin` and no CSP (`asset.rs:13-27`, `:39`). (4) So an HTML file under a `.pdf`
name renders as a *scripted document at the shared origin `asset://localhost`*, same-origin with the
whole data dir (scope `crates/desktop/src/lib.rs:322-325`), where `<data_dir>/token` is the RPC bearer
accepted as `Authorization: Bearer` or `?token=` with no origin check (`crates/rpc/src/lib.rs:248-262`) —
one `fetch("asset://localhost/<data_dir>/token")` and a WebSocket later the page is the user. Windows:
the same at `http://asset.localhost`. Trigger: an agent — or a prompt-injected one — shows an HTML file
under a `.pdf` name and the user clicks the card. Opaque-origin pages (this design's) cannot do any of
this: WebKit refuses a cross-origin `fetch` to a non-HTTP scheme before dispatch, Chromium sees an ACAO
mismatch (S1), and `allow-scripts allow-forms` has no `allow-same-origin`.

Required in this design, cheapest first:

- **(a) Ingest-time content checks** (`crates/artifact/src/sniff.rs`, called from `store.rs` right after
  `kind::of`, `:690`): `Kind::Pdf` requires `%PDF-` at offset 0; `Kind::Image` requires one of the §8
  image magics (PNG/JPEG/GIF/WebP — any of the four formats under any of the five names `kind.rs:27`; the mime
  recorded follows the bytes); `Kind::Svg` requires the §8 SVG-root test *before* `svg::rasterize` (`store.rs:727-732`,
  which already fails on a non-SVG, but with a parser message). A mismatch **demotes the kind to
  `File`** and the receipt says why: `note: bytes are not a PDF (no %PDF- header); shown as a downloadable
  file` — the existing `File` note path (`tools.rs:79-81`) with the reason. Demotion over failure: a
  `File` card is a download chip on every client, so a mis-named file is still handed over (a user's
  dropped file the model did not write), the record never lies about what the viewer will do, and the
  model reads the reason and re-shows under the right name when it was its own bytes. The known
  false negative (a PDF with a PJL preamble before `%PDF-`) becomes a download; acceptable. `Kind::Html`
  is not sniffed — it is *meant* to be scripted — and `Kind::Text`/`Markdown`/`Csv` render as text.
- **(b) Documents never load from `asset:`.** `PdfFrame` (`Viewer.tsx:270-275`) and the lightbox
  `Original` (`Attachments.tsx:107-114`) read the bytes over RPC (`readBytes`, `lib/files.ts:91-92`) and
  build a typed `blob:` URL (`new Blob([bytes], { type: "application/pdf" })`) on the local path too —
  the remote path already does exactly this (`files.ts:76-78`); `convertFileSrc` stays for `<img>`
  only. A blob document of type `application/pdf` runs no script; a blob of mis-typed bytes shows
  WebKit's "failed to load" instead of a page. `resolveFile` gains a `document: true` mode or a sibling
  `resolveDocument` so the two frames cannot regress to `asset:` by default.
- **(c) `frame-src` loses `asset: http://asset.localhost`** (`tauri.conf.json:48`), keeps `'self'` and
  `blob:`, gains `theuth-page: http://theuth-page.localhost` (§4.3). `img-src` keeps `asset:`.
- **(d) The asset scope narrows** from `allow_directory(data_dir, true)` (`crates/desktop/src/lib.rs:323-325`)
  to the roots the UI loads as images and `get_attachment` already serves (`crates/rpc/src/api.rs:5754`):
  `attachments/`, `browser/`, `computer/`, `artifacts/`, `spill/` under the data dir, recursive. `token`,
  `secrets.vault`, `sessions.sqlite`, `config.yaml`, `knowledge/`, `mcp/` become unaddressable from the
  webview. Any later `convertFileSrc` of a path outside those roots fails loudly (403, `asset.rs:46-49`)
  instead of silently serving.
- **(e) The probe is a gate** (§13.3): a scratch data dir with a canary file, an HTML file stored under a
  `.pdf` name, opened in the panel and in the lightbox, with the assertion that no same-origin read of
  the canary or of `token` happens — run before (expected: the read succeeds today, confirming S0) and
  after (a)–(d) (expected: the artifact is a `File`; and with (a) bypassed in a test build, the frame is
  a `blob:` PDF viewer that renders nothing).
- **(f) Taint.** 35 §9 gave `show` `sink: false` because its audience equals the reply's
  (`35-artifacts.md:657-663`). A *scripted* page with network access changes that for one kind: it can
  send its own content — the model's text and the §8 images — to any host, the way a `<img src=https:…>`
  already can carry request metadata (`35-artifacts.md:676-677`), now with the whole page as payload.
  28's taint gate is a later phase (`28-untrusted-content.md:358`, §7); when it lands, a `show` of
  `Kind::Html` whose bytes contain `<script` or an external `src`/`href` is sink-class — at least Ask in a
  tainted session — while every other kind keeps `sink: false`. Until then the exposure is documented
  here and in the prompt's "one self-contained document" rule, not gated.

## 10. Prompt and skill changes (D6)

Pick order everywhere: **catalog → openui/app → page**. The page is what you reach for when the catalog
has no component for the shape and an `app` would be wrong because nothing comes back.

### 10.1 `surface::prompt_block` — `BLOCK_TAIL` (`crates/surface/src/prompt.rs:182-236`)

After the three fence lines (`:183-184`) and before `Pick by the shape of the answer:` (`:186`), add:

> - A **page** (`show{content, name: "x.html"}`): one self-contained HTML document with its own
>   `<style>`/`<script>`, CDN libraries allowed, rendered live inline in the theuth app in a sandboxed
>   frame themed like the chat. Only for what no catalog component draws: a chart library's chart
>   (Chart.js, ECharts, Plotly, D3), a map, 3D, a mockup, an image collage, an animation, a layout you
>   must control pixel by pixel. Nothing comes back from a page — a button that should reach you is an
>   `app`, not a page. Rules: fluid width, no horizontal padding on the outermost element, no outer
>   card, border or banner title (the page is part of your reply); charts at fixed pixel heights; let
>   content set the height, never `100vh` or `height:100%` on `html`/`body`; use the theme variables
>   (`--background --foreground --muted --card --border --primary --accent --chart-1..6 --radius
>   --font-sans --font-mono`, the same names as T3 Code) with fallbacks, e.g. `var(--foreground,
>   #1a1612)`; local images by absolute path are inlined for you; pass `height` only to cap a long
>   page. Check a scripted page with `page_preview` before you show it. The reader sees the page where
>   the card is — do not announce or restate it; add only what it does not say.

And one row in the pick-by-shape table (`:186-210`), after the `Image` row (`:208`):

> | a chart from a chart library, a map, 3D, a mockup, a collage, an animation | a page: `show{content, name: "x.html"}` (check with `page_preview`) — never for what a component above draws |

### 10.2 The `show` block (`crates/artifact/src/tools.rs:33-34`)

Append to `PROMPT`:

> `show{content, name: "x.html", height?}` renders a live sandboxed page inline in the theuth app
> (desktop, web, phone; Slack/Telegram/Signal get the file). One self-contained document; absolute
> local image paths are inlined; `height` caps the frame (80–2000) and the rest scrolls inside. Preview
> a scripted page with `page_preview` first.

And in the tool description (`tools.rs:53-61`): `html` names render as live pages (scripts, CDN
libraries) inline; the optional `height` caps the inline frame.

### 10.3 The visualize skill (`crates/surface/visualize.md`, `## Choose the block` `:15-43`)

A row before the board row (`:43`): `| a chart library's chart, a map, 3D, a mockup, a collage, an
animation | a page — show{content, name: "x.html"} after page_preview | a Code block of HTML, or an app
that calls nothing back |`, and a new section `## Pages` after `## json-render rules that avoid errors`
(`:87`) carrying the §10.1 rules in full plus the theme-variable list of §7.1 and a 12-line example (a
Chart.js bar chart from the CDN, `<canvas height="320">`, `:root` fallbacks, no outer card). The skill is
regenerated at start (`crates/theuth/src/main.rs:1176-1191`, `surface::visualize_skill()`
`crates/surface/src/prompt.rs:274-294`), so the file is the only place to edit.

### 10.4 `page_preview` description (§11)

> Load an HTML page hidden in the connected theuth desktop and get back `contentHeight` (the height
> the page needs at `width`) and its console output: log, info, warning, error and uncaught exceptions.
> `console.log` is a fine way to report your own checks. Use it before `show` when the page has
> scripts. No screenshot. Fails fast with "no renderer connected" when no desktop or web client is
> open; then pass a sensible `height` to `show` and go on.

## 11. Preview hook (D8, its own wave; may ship after the others)

### 11.1 Tool

`page_preview { html: string (1..512 000 chars), width?: 240..1600 (default 728), appearance?: "dark"
| "light" (default: the client's current) }` → `{ width, contentHeight, console: [{level, text}], loaded:
bool, client: "desktop" | "web" | "phone" }`. `Risk::ReadOnly`, `RETURNS: "dict"`, `DIRECT: true`,
registered by the artifact plugin next to `ShowTool` (`crates/artifact/src/lib.rs:812-820`). The name:
the argument is a page and the result is page metrics; `show` is the publish verb and `show_preview`
would read as a dry-run of every `show` (markdown, PDF, CSV), which it is not. A later design that
returns PNGs for fences and apps can add parameters under the same noun or a sibling tool.

### 11.2 RPC shape (the `ask_user` pattern)

Mirrors `AskRequested` → `answer_ask` (`crates/tools/src/ask.rs:80-91`, `:107-115`;
`crates/rpc/src/api.rs:575-578`):

```rust
/// Event, every UI subscribes (bridge `forward!`, crates/rpc/src/bridge.rs:95-130).
#[serde(tag = "type")] pub struct PagePreviewRequested {
    pub id: PreviewId, pub session: SessionId,
    pub html: String,            // already bootstrapped by the UI; the engine sends the agent's bytes
    pub width: u32, pub appearance: Option<Appearance>,
}
#[serde(tag = "type")] pub struct PagePreviewResolved { pub id: PreviewId }
#[serde(tag = "kind", rename_all = "snake_case")] pub enum PagePreviewAnswer {
    Started { client: Client },                              // "I am rendering it"
    Done { content_height: u32, console: Vec<ConsoleLine>, loaded: bool, client: Client },
    Failed { reason: String },
}
// RPC method: true if the preview was still pending (first Done/Failed wins; Started extends the wait).
#[method(name = "answer_page_preview")]
async fn answer_page_preview(&self, id: PreviewId, answer: PagePreviewAnswer) -> RpcResult<bool>;
```

Not in `PendingPrompts` (`api.rs:1856-1872`): a preview that nobody rendered in time is dead, not
pending. The TUI, Slack, Telegram and Signal ignore the event. Every connected desktop/web client that
receives it renders; the first `Done` wins, `PagePreviewResolved` tells the others to tear down.

### 11.3 Client

On `PagePreviewRequested`: `answer_page_preview(id, Started)`; inject the bootstrap (§5) with the
requested appearance's variables from `useHtmlPageTheme` (resolved for `light` by the
`lightTokens()` trick, `diagram-theme.ts:42-51`); `page_put` (or `srcdoc`); mount
`<iframe sandbox="allow-scripts allow-forms" style="position:fixed;left:-10000px;top:0;width:<width>px;
height:800px;visibility:hidden">` (off-screen and invisible, never `display:none`, which stops layout);
collect `size-changed` and `console` posts; after `load`, wait for fonts (`document.fonts.ready` is the
page's — the bootstrap's post already follows a `ResizeObserver`) and settle 300 ms after the last post,
at most 6 s after `load` (T3's measure budget); answer `Done` with the last height (clamped 80–2000),
the console lines (≤ 20 × 500), `loaded: true`. No `load` within 10 s → `Done { loaded: false, … }` with
whatever arrived. Then `page_drop`, unmount.

### 11.4 Timeouts and the "no renderer" result

Engine: no `Started` within **2 s** → the tool returns `error: no renderer connected — open the theuth
desktop or web app, or pass height to show and skip the preview`; a `Started` extends the deadline to
**15 s** total; no `Done` by then → `error: preview timed out after 15 s (the page did not finish
loading); console so far: …`. The bridge has `receiver_count()` on its broadcast sender
(`crates/rpc/src/bridge.rs:142`); zero subscribers short-circuits to the same "no renderer" error at
once. Concurrency: one in-flight preview per session; a second call while one is pending returns
`error: a preview is already running`.

### 11.5 Prompt text

In §10.1/§10.2 already: "Check a scripted page with `page_preview` before you show it." The tool
description (§10.4) carries the rest. The result's `message` line: `Previewed at 728 px: contentHeight
612, 0 errors. Pass height: 612 to show, or less to make it scroll.`

### 11.6 Later options, named only

A PNG needs a renderer: (a) T3's pinned Chrome-for-Testing headless shell (~120 MB opt-in download,
public-only egress proxy, fake origin, 20 s capture) or (b) obscura (`crates/browser`, feature `render`,
`Cargo.toml:277-278`) if canvas-2D libraries render and console can be captured. Whichever yields a PNG
also becomes the Slack/Telegram/Signal rendering of pages **and** of Graph/Sequence/mermaid fences — the
open items in `30-surfaces.md:796-813` (§14 "Mermaid PNG to Signal/Slack") and
`110-slack-cognitive-load.md:598-613` (§16 "PNG for Graph / Sequence / mermaid", `:611`). Not this design.

## 12. Changes by place

### 12.1 Engine (W1)

| file | change |
|---|---|
| `crates/artifact/src/tools.rs:16-30` | `ShowArgs.height: Option<u32>` (doc: "Cap on the inline frame of an `.html` page, CSS px, 80–2000; content taller scrolls inside"); description `:53-61` names live pages; `PROMPT` `:33-34` gains §10.2 |
| `crates/artifact/src/inline.rs` (new) | §8: `find_local_images`, `is_image_bytes`, `has_svg_root`, `inline_images`; pure over a `read: Fn(&Path) -> io::Result<Vec<u8>>` for tests |
| `crates/artifact/src/sniff.rs` (new) | §9a: `fn demote(kind, &bytes) -> (Kind, Option<&'static str>)` with the reason text |
| `crates/artifact/src/store.rs:690` | call `sniff::demote` after `kind::of`; `:733-736` Html arm runs the inliner, fails `show` on missing/oversize, sets `receipt.images = Some(n)`; `height` clamped and echoed |
| `crates/artifact/src/kind.rs:97-108`, `:122-154` | `Receipt.images: Option<usize>`, `Receipt.height: Option<(u32, u32)>` (asked, used); receipt line for `Kind::Html` appends the §3.9 audience sentence; the `File` demotion note carries the sniff reason |
| `crates/artifact/src/preview.rs` (new, W4) | `page_preview` tool (§11.1), `PagePreviewRequested/Resolved`, `PagePreviewAnswer`, the pending map with the 2 s / 15 s timers |
| `crates/artifact/src/lib.rs:812-820` | register `PagePreviewTool` (W4); `ts` exports for the new types |
| `crates/surface/src/prompt.rs:182-236` | §10.1 paragraph + table row; test `skill_is_generated_and_valid` (`:438`) still passes |
| `crates/surface/visualize.md` | §10.3 row + `## Pages` section |
| `crates/rpc/src/bridge.rs:95-130` | `forward!(… PagePreviewRequested, PagePreviewResolved)` (W4) |
| `crates/rpc/src/api.rs` | `answer_page_preview` beside `answer_ask` (`:575-578`) (W4); `just ts` regenerates bindings |

Nothing in `crates/rpc` for W1–W3: `get_attachment` already serves artifact blobs (`api.rs:5701-5764`).

### 12.2 Desktop Rust (W2)

| file | change |
|---|---|
| `crates/desktop/src/pages.rs` (new) | `Pages` managed state (token → bytes, LRU 64 / 128 MiB), `#[tauri::command] page_put(call_id: String, html: String) -> Result<String, String>` returning the platform URL (`theuth-page://localhost/<token>` or `http://theuth-page.localhost/<token>`), `page_drop(call_id)`, the scheme handler (§4.2 table) |
| `crates/desktop/src/lib.rs:32-41` | `.register_uri_scheme_protocol("theuth-page", pages::handler)` + `.manage(Pages::default())` on the shared builder (before `desktop::configure`/`mobile::configure`, so both get it); `page_put`/`page_drop` in both `invoke_handler`s (`:80`, `:267-288`) |
| `crates/desktop/src/lib.rs:322-325` | asset scope → the five roots (§9d), each `allow_directory(data_dir.join(root), true)`, missing roots ignored |
| `crates/desktop/tauri.conf.json:48` | `frame-src 'self' theuth-page: http://theuth-page.localhost blob:` |
| `crates/desktop/capabilities/*.json` | nothing: theuth's own commands (`commands::connection`, `point_*`, `island_*`) are invoked today with no entry in either capability file (`default.json`/`mobile.json` list core and plugin permissions only), so an app-defined command needs none |

The 25 MiB upper bound travels through `invoke` as a JSON string; if that proves slow on the phone,
Tauri 2's raw-body `invoke` (`tauri::ipc::Request`) is the escape — unverified here, probe in §15.

### 12.3 UI (W3)

| file | change |
|---|---|
| `crates/desktop/ui/src/lib/page-bootstrap.ts` (new) | `injectBootstrap(html, theme): string` (§5.1 rules, the script of §5 with §5.4), `BASE_CSS`, `themeFragment(theme)`, `readSizeChanged`, `readOpenLink`, `readConsole`, `result(id)` — pure, vitest-covered |
| `crates/desktop/ui/src/lib/page-theme.ts` (new) | `useHtmlPageTheme()` (§7.1 table), `pageThemeFor(appearance)` using `lightTokens()`'s class-lift trick (`diagram-theme.ts:42-51`) |
| `crates/desktop/ui/src/lib/pages.ts` (new) | `pagePut/pageDrop` wrappers over `invoke` (pattern `lib/tauri.ts`), `isTauri()` switch for the `srcdoc` fallback |
| `crates/desktop/ui/src/components/chat/HtmlPage.tsx` (new) | the frame: reserve/clamp (§6), bridge listeners (§5), link gate (§5.3), console chip, `Unable to load`, hover toolbar slot, `cap` prop; used by the card (inline, `cap = height ?? 2000`) and the viewer (`cap = none`) |
| `components/chat/ArtifactCard.tsx:421-460` | `artifact.kind === "html"` → `<HtmlPage>` + caption instead of the `Frame` body; `height` read from `tool.args` (`store/sessions.ts:103-107`) |
| `components/artifacts/Viewer.tsx:337` | `default` arm → `kind === "html"` ? `<HtmlPage cap={none}>` with the `Page \| Source` toggle : today's inert iframe (unreachable now, kept for `file`-like fallbacks) |
| `components/artifacts/Viewer.tsx:270-275`, `components/chat/Attachments.tsx:107-114` | PDF frames load typed `blob:` URLs on the local path too (§9b) |
| `lib/files.ts:68-88` | `resolveDocument(api, path)`: always RPC → typed blob; `resolveFile` unchanged for images |
| `crates/desktop/ui/src/lib/page-preview.ts` + a mount in `App.tsx` (W4) | §11.3 hidden-frame runner subscribed to `PagePreviewRequested` |

### 12.4 Channels and TUI

Receipt text only (§3.9), produced by the engine; `crates/slack`, `crates/telegram`, `crates/signal`,
`crates/tui` unchanged — their artifact handlers already upload the file / show the source
(`crates/slack/src/channel.rs:2203-2240`, `crates/telegram/src/channel.rs:2549-2596`,
`crates/signal/src/channel.rs:1612-1622`, `crates/tui/src/model.rs:10141-10150`, `:10167-10175`).

## 13. Tests and gates

### 13.1 Rust (`cargo test -p artifact`, `-p surface`, `-p rpc --features ts`)

`artifact::inline`: `absolute_paths_only` (URLs, `data:`, `blob:`, `//host/x.png`, `./x.png` never
match; `/a/b.png` in `src=""`, `'…'`, backticks and `url()` do); `windows_paths_unescape_in_js_strings`;
`extension_allow_list`; `svg_root_detected_after_prolog_comment_doctype`; `renamed_secret_is_not_an_image`
(a text file named `.png` → listed as unreadable); `oversize_image_fails_before_read`;
`page_limit_counts_base64_growth`; `missing_file_lists_every_path_once`; `offsets_splice_in_original_text`
(bootstrap-free text in, same text out except the data URIs). `artifact::sniff`:
`pdf_requires_magic_at_zero`, `image_kinds_accept_any_image_magic_and_record_mime`,
`svg_requires_root`, `html_is_never_sniffed`, `mismatch_demotes_to_file_with_reason`.
`artifact::store`: `html_show_inlines_and_counts_images`, `html_show_fails_listing_missing_images`,
`height_is_clamped_and_echoed`, `pdf_named_html_bytes_is_a_file` (the S0 ingest half). `surface::prompt`:
`skill_is_generated_and_valid` (exists, `prompt.rs:438`) plus `block_names_pages_after_the_fences`.
`rpc` (W4): `answer_page_preview_first_done_wins`, `started_extends_the_deadline`,
`no_started_in_two_seconds_is_no_renderer`.

### 13.2 vitest (`cd crates/desktop/ui && bun run test`)

`lib/page-bootstrap.test.ts`: `injects_at_start_of_head`, `wraps_when_no_head`, `ignores_head_in_comment_
script_and_template`, `keeps_existing_charset_and_viewport`, `whitelists_variable_names_and_strips_
delimiters`, `size_changed_requires_finite_positive`, `open_link_rejects_javascript_file_and_custom_
schemes`, `console_caps_at_twenty_by_five_hundred`. `components/chat/HtmlPage.test.ts`:
`reserves_height_or_240_before_first_post`, `clamps_to_cap_and_2000`, `ignores_messages_from_other_
windows`, `link_opens_only_with_focus_and_activation`, `error_chip_counts_errors_only`,
`shows_unable_to_load_on_failure`. `lib/page-theme.test.ts`: `maps_all_forty_names` (the T3 list
pinned verbatim), `accent_is_the_brand_token`. `lib/files.test.ts`: `documents_never_use_asset_urls`.

### 13.3 The exposure probe (gate for W2+W3 integration; run by the closer)

Scratch data dir (`t --data-dir /tmp/t120 --headless`, config/vault copied, the knowledge-recorded
scratch-engine recipe): write `canary.txt` into the data dir root; `show{path}` an HTML file under a
`.pdf` name whose script does `fetch("asset://localhost/<data_dir>/canary.txt")` and, on success, sets
`document.title = "LEAKED"`; open the card in the panel and in the lightbox on macOS. **Before** this
design (main `ebac3603`): the title flips — S0 confirmed and recorded. **After**: the artifact is a
`File` (13.1 covers the sniff); with the sniff bypassed in a test build the frame is a `blob:`
`application/pdf` viewer that renders nothing and the canary is never requested (verified by the
engine log and a `sandbox` iframe `console` capture). A second page served through `theuth-page:`
attempts the same fetch plus `fetch("theuth-page://localhost/<another token>")`, `location.href =
"asset://…"`, `window.open`, `localStorage`, `document.cookie`: every one fails or is inert; the
`location.href` result on WebKit is the §15 probe's answer.

### 13.4 Manual smoke (recorded in the PR)

Desktop light → dark flip while a Chart.js page is on screen (recolours without reload); `height: 300`
on a 900-px page scrolls inside with no visible scrollbar; a page without `height` grows once to its
content; phone (iOS 17.4+ device or simulator, `tauri.ios.conf.json:10`): the same page at ~360 px, a
`<form method="get">` submit (expect: blocked by `frame-src`, or navigates within the sandbox — §15
records which), a link opens Safari only after a tap; Windows: `http://theuth-page.localhost` form
loads; `Save as…` yields the agent's bytes; Slack and Telegram threads receive the `.html` file; TUI
shows fenced source; `page_preview` from a session with the desktop open returns height + console in
< 3 s, and returns "no renderer connected" within 2 s with every client closed.

### 13.5 Gates

Per `justfile:23-38` (at `4a44a8d6`, which prefixed the gate recipes with `CARGO_INCREMENTAL`): `cargo check -p
artifact -p surface -p rpc --all-targets`, `cargo test -p artifact`, `cargo test -p surface`, `cargo test -p rpc
--features ts --lib` (bindings, `justfile:90-91`),
`cargo clippy -p artifact -p surface -p rpc -p desktop --all-targets -- -D warnings`, `cargo fmt -p
<crate> -- --check` (never `--all`); desktop crate `cargo check -p desktop` on the host OS only (the
Tauri CLI is never run by a gate); UI `just ts-desktop` (`:94-97`) then `just desktop-check`
(`:223-224` = `bun run check && bun run test && bun run build`, `crates/desktop/ui/package.json:8-11`).
No worker runs the workspace suite; the closer does, once.

## 14. Waves and seams

Four workers off a docs-only main carrying this file; strict seam ownership; each writes
`docs/design/120-as-built-<W>.md`. Contract stubs frozen before the cut (names and signatures below are
binding; a change is a message to the sibling, not an edit):

```
Tauri commands (W2 owns, W3 calls):   page_put(call_id: string, html: string) -> Promise<string /* url */>
                                      page_drop(call_id: string) -> Promise<void>
Scheme:                               theuth-page   (frame-src: theuth-page: http://theuth-page.localhost)
Bridge (W3 owns, W4 reuses):          ui/notifications/host-context-changed · ui/notifications/size-changed ·
                                      ui/open-link · theuth/notifications/console   (payloads §5)
show (W1 owns, W3 reads):             ShowArgs.height?: u32 (80–2000)  — read by the card from tool.args.height
RPC (W4 owns, both halves):           event PagePreviewRequested { id, session, html, width, appearance }
                                      event PagePreviewResolved { id }
                                      method answer_page_preview(id, answer: Started|Done|Failed) -> bool
```

| worker | owns (files) | owns (sections) | does not touch |
|---|---|---|---|
| **W1 ENGINE** | `crates/artifact/src/{tools,store,kind}.rs`, new `inline.rs`, `sniff.rs`; `crates/surface/src/prompt.rs`, `crates/surface/visualize.md`; their tests | §8, §9a, §10, §13.1 (non-W4 tests) | `crates/rpc`, `crates/desktop`, anything under `ui/` |
| **W2 DESKTOP-RUST** | `crates/desktop/src/{lib.rs,pages.rs}`, `crates/desktop/tauri.conf.json`; `cargo check -p desktop` | §4 (scheme, headers, CSP edits, asset scope), §9c, §9d | `crates/desktop/ui/**`, the engine crates |
| **W3 UI** | `crates/desktop/ui/src/lib/{page-bootstrap,page-theme,pages}.ts`, `components/chat/HtmlPage.tsx`, edits to `ArtifactCard.tsx`, `Viewer.tsx`, `ArtifactPanel.tsx`, `Attachments.tsx`, `lib/files.ts`; their vitest files | §3, §5, §6, §7, §9b, §13.2 | Rust anywhere; `rpc/bindings` (hand-written stubs for `height` only if needed — none: it is a tool arg) |
| **W4 PREVIEW** (may start after W1–W3 land) | `crates/artifact/src/preview.rs` + its registration line in `lib.rs`, `crates/rpc/src/{bridge,api}.rs` additions, `ui/src/lib/page-preview.ts` + the `App.tsx` mount; `just ts` run | §11, §12's W4 rows, §13.1 rpc tests | `HtmlPage.tsx` internals (imports `page-bootstrap.ts` as-is), `store.rs`, `tauri.conf.json` |
| **closer** | integration branch, fold of main, the probe | §9e/§13.3, §13.4, §13.5 | — |

Cross-seam facts each brief must carry: W3 cannot run without W2's command in the Tauri shell but can
develop and test in the dev browser (`srcdoc` path) first; W2 has no UI to exercise the handler and
tests it with a unit test over the handler function (request in, response headers out); W1's receipt text
and W3's caption are independent (different readers); W4's hidden frame must reuse `HtmlPage`'s message
parsers, never duplicate them (109's lesson: one owner per function).

## 15. Open questions and probes still owed

| question | settles it |
|---|---|
| **iOS 17.4 subframe + forms + postMessage.** Does WKWebView on iOS load a `sandbox="allow-scripts allow-forms"` iframe from `theuth-page://localhost/<token>`, run its script, deliver `postMessage` both ways, and what does a `<form method="get">` submit do? | §13.4 phone smoke on a device; record in the as-built. Fallback if the custom-scheme subframe fails on iOS only: the `srcdoc` path of §4.5 inside the Tauri webview (parent CSP = Tauri's, so CDN scripts would need `https:` in `script-src` — a trade the as-built must name) |
| **WebKit `frame-src` on a sandboxed child's self-navigation.** A sandboxed frame may navigate itself; does WebKit apply the parent's `frame-src` to `location.href = "asset://localhost/…"` from inside the child? (Chromium does.) | §13.3's second page: `location.href` to an `asset:` and an `https:` URL; expected blocked by `frame-src` — if not, the content is still opaque-origin and visual only, but §9d's scope narrowing is then load-bearing, not belt-and-braces |
| **`sandbox` CSP directive support** in WKWebView / WebKitGTK / WebView2 for custom-scheme documents. | A top-level load of a page URL in a pop-out window (`open_window`, `crates/desktop/src/commands.rs:20-21`) with a script that reads `window.origin` (expect `"null"`) and `localStorage` (expect a `SecurityError`) |
| **`invoke` with a 25 MiB string** on the phone. | Time `page_put` with a 25 MiB page on iOS; if > 1 s, switch to Tauri's raw-body `invoke` (`tauri::ipc::Request`) — "unverified — probe: measure first" |
| **Window-label scoping.** Should the handler refuse requests from webviews other than `main`/pop-outs (`UriSchemeContext::webview_label`)? | Decide when the island or overlay webviews ever host a transcript; today they do not |
| **`.md` artifacts with local images.** The §8 inliner over markdown image syntax, so a report can carry screenshots. | A later note once pages have shipped; the regex is a different grammar |
| **Mermaid hardening (B6).** DOMPurify over mermaid's SVG as T3 does (`apps/web/src/components/chat/MermaidDiagram.tsx`). | Later; strict + `htmlLabels: false` already covers most |
| **Taint gate (§9f).** When 28 §7 lands, the sink rule for scripted pages. | 28's own wave |

## 16. References

**T3 Code** (`pingdotgg/t3code`, clone `/tmp/recon-t3code` at `c9b21b28`; commits `677d1527` visual
replies, `5e35272f` mermaid fences, `64275ae3` scroll-trap fix, `ac8e9453` preview browser on the
environment server): `apps/server/src/mcp/toolkits/html/tools.ts`; `packages/shared/src/htmlRender.ts`
and `htmlRender.test.ts`; `apps/server/src/http.ts`; `apps/server/src/htmlRender/{HtmlRender,
headlessChrome}.ts` and `HtmlRender.test.ts`; `apps/web/src/components/files/BrowserDocumentFrame.tsx`;
`apps/web/src/components/chat/{HtmlRenderFrame,MermaidDiagram}.tsx`;
`apps/server/src/provider/T3OrchestrationInstructions.ts:33-35`; `apps/web/src/session-logic.ts:701-715`.

**theuth** (`main` `ebac3603`): `crates/artifact/src/{kind,store,tools,lib,mesh}.rs`,
`crates/artifact/migrations/0001_artifacts.sql`; `crates/surface/src/{spec,prompt}.rs`,
`crates/surface/visualize.md`; `crates/agent/src/{loop_,lib,frame}.rs`; `crates/tools/src/ask.rs`;
`crates/rpc/src/{lib,api,bridge}.rs`; `crates/link/src/conn.rs`; `crates/desktop/src/{lib,commands}.rs`,
`crates/desktop/tauri.conf.json`, `crates/desktop/tauri.ios.conf.json`, `crates/desktop/ios/Info.ios.plist`,
`crates/desktop/capabilities/{default,mobile}.json`; `crates/desktop/ui/src/components/artifacts/Viewer.tsx`,
`components/chat/{ArtifactCard,ArtifactPanel,Attachments,MessageAssistant}.tsx`,
`components/apps/AppHost.tsx`, `lib/{files,theme,diagram-theme,tauri}.ts`, `main.tsx`, `styles.css`,
`rpc/extra.ts`, `store/sessions.ts`; `crates/slack/src/channel.rs`, `crates/telegram/src/channel.rs`,
`crates/signal/src/channel.rs`, `crates/tui/src/model.rs`; `justfile`; `Cargo.toml`, `Cargo.lock`;
`docs/design/35-artifacts.md`, `30-surfaces.md`, `70-as-built-SURFACES.md`, `110-slack-cognitive-load.md`,
`28-untrusted-content.md`.

**Tauri / wry / jsonrpsee** (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`):
`tauri-2.11.5/src/app.rs`, `src/protocol/{asset,isolation,tauri}.rs`, `src/manager/webview.rs`,
`scripts/isolation.js`; `tauri-utils-2.9.3/src/{mime_type,config}.rs`, `src/pattern/isolation.js`;
`infer-0.19.0/src/matchers/text.rs`; `wry-0.55.1/src/wkwebview/{mod,navigation}.rs`,
`src/wkwebview/class/url_scheme_handler.rs`, `src/custom_protocol_workaround.rs`,
`src/webkitgtk/web_context.rs`; `jsonrpsee-server-0.26.0/src/server.rs`, `src/transport/http.rs`.

**Knowledge**: `facts/t3code-visual-replies-html-render-recon-2026-10-06`;
`facts/theuth-agent-visual-surfaces-inventory-2026-10-06`;
`facts/theuth-borrow-list-from-t3code-visual-replies-2026-10-06`;
`facts/theuth-design-120-spikes-asset-exposure-page-origin-engine-route-2026-10-07`;
`decisions/theuth-t3-visual-replies-next-steps-rulings-2026-10-06`. Brief and gap analysis:
`~/Develop/theuth-120-briefs/{brief-design-120,gap-analysis}.md`.


## Revision 1 (2026-10-07 09:47) — interfaces are mocked as pages

Sky: "make sure that we have prompt in place in the skill or wherever that lets the agent know that when
we're building interfaces, html mocks is a great way to visualize that in the session."

§10 as built listed "a mockup" as one of the exceptional page uses behind "the escape hatch, picked last";
nothing told the model to reach for a mock *when the work is an interface*. This revision makes that the
standing case, in the three places §10 names:

- `surface::prompt_block` (`crates/surface/src/prompt.rs`, `BLOCK_TAIL`): the page paragraph gains
  "Interfaces are the standing case: when the work is designing or changing a screen, dialog, form,
  dashboard or component — in theuth or in any product — mock it as a page (`show{content, name:
  "mock.html"}`) before or while you build it, so the reader sees the proposed screen in the chat and
  reacts to a picture, not a description: fake data, real layout, the theme variables, states or variants
  side by side. A clickable flow with live data is an `app`; the faithful look of a screen is a page."
  The pick table gains the row "an interface you are designing or changing — a screen, dialog, form,
  dashboard, component → an HTML mock as a page" directly after `Image`; the test
  `block_names_pages_after_the_fences` asserts both.
- The visualize skill (`crates/surface/visualize.md`): the same row before the chart-library row in
  "Choose the block", and an "Interfaces are the standing case" paragraph under `## Pages` (states and
  variants side by side — empty · loading · filled; phone · desktop — iterate on the mock, then build).
- The `show` prompt block (`crates/artifact/src/tools.rs` `PROMPT`): "Designing or changing an interface
  — a screen, dialog, form, dashboard, component? Mock it as a page (`mock.html`, fake data, real layout)
  so the user sees the screen in the chat before you build it."

Routing rule restated: a *clickable flow with live data* is an `app` (OpenUI, §2 of 46); the *faithful look*
of a screen — including theuth's own screens, which OpenUI cannot draw pixel-faithfully — is a page. No
engine or UI code changes; the running engine shows the new text after its next rebuild.
