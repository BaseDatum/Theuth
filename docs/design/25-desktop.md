# 25 — Desktop: a macOS menubar app over RPC

Status: as-built v0 · 2026-09-15 (draft 2026-09-14) · extends `00-architecture.md` §2 (daemon
launch), §7 (RPC), §8 (one command list), §10, §12 row 4; `18-app-launch.md`
(the launch mechanism, unchanged, made concrete for packaging); `10-knowledge.md`
§8 (two RPC additions); `19-subagent-board.md` §5 (the GUI tree). Where 18
and this doc disagree on launch, 18 wins; §5 here only adds packaging.
§12 is the as-built record. Recon: TUI inventory, RPC inventory, knowledge
inventory and the measured ChatGPT tokens are recorded in §2.

## 0. Decisions in one screen

1. **Not a plugin. An RPC client of `rpc::Api`** (`docs/plugins.md:17`).
   The app never links the harness crates; it talks `ws://127.0.0.1:<port>`
   with `<data_dir>/token` like any other frontend. A command exists when it
   is an `Api` method (00 §8); §7 lists the handful this doc adds.
2. **Tauri 2, one window, tray icon, `ActivationPolicy::Accessory`.** No
   Dock icon, no app menu bar of our own; a monochrome template tray icon
   toggles a normal resizable window. Rejected: an `NSPanel` popover
   (`tauri-nspanel`, private API, hides on blur - wrong for a transcript you
   read for minutes) and a Dock app (the user asked for a menubar app).
3. **The daemon is the bundled `wth` sidecar, started exactly as 18 says**:
   `portable-pty` + `$SHELL -l -i -c 'exec <Contents/MacOS/wth> --headless
   --data-dir ~/.wth'`, `WTH_RPC__LISTEN=127.0.0.1:<free port>`,
   `WTH_LAUNCH=1`. Before spawning, probe the configured port with the token
   and reuse a live daemon (18 §3). Same `~/.wth` as the terminal: sessions,
   knowledge, skills and logins are shared.
4. **The webview owns the WebSocket.** The Rust side does launch, probe,
   quit, attachment writes and OS integration only; it never proxies RPC.
   Rejected: RPC through Tauri IPC - a second serialization of every
   `TextDelta`, for nothing.
5. **React 19 + TypeScript + Vite; TanStack Router/Query/Virtual; shadcn/ui
   on Tailwind v4; Zustand for event-fed state; `streamdown` for markdown.**
   Each is the boring default for its job (§4.1 names the alternative each
   beat). No component library of our own; every widget is a shadcn
   primitive restyled with the ChatGPT tokens.
6. **Visual clone of chatgpt.com, measured, not eyeballed** (§2.4): 260 px
   sidebar, 768 px column, 22 px user bubble at 70 %, `#0d0d0d`/`#fff` text,
   the exact greys, radii, paddings and the system font stack. Icons are
   lucide (shadcn's default) - the one thing not cloned. No logo, no product
   name anywhere in the UI; the window title is the session title.
7. **Performance is a design input, not a tuning pass** (§6): one
   `subscribe_events(null)` per connection, per-frame coalescing of deltas,
   per-session store slices so hidden sessions cost nothing to render,
   virtualized transcript and sidebar, memoized completed markdown blocks.
   Budget: 50 concurrent streaming sessions with no dropped frame on the
   visible one; the check in §9 measures it.
8. **Feature parity with the TUI is a table, not a feeling** (§3): every
   slash command, menu, key and event rendering of `crates/tui` has a named
   desktop surface. The slash popup survives (same `COMMANDS` list) so the
   TUI muscle memory works; menus become ChatGPT's dropdowns and the
   settings dialog.
9. **Knowledge explorer** (§8): tree from `knowledge_list`, rendered
   document with working `[t](/dir/id.md)` links resolved client-side by the
   okf rules, CodeMirror raw editor saving through a new `knowledge_write`,
   backlinks, history/restore, search. Rename is not in v0 (no RPC; put+delete
   would break backlinks); said in the UI, not hidden.
10. **Packaging is designed now** (§5): `externalBin` sidecar, `mainBinaryName`
    distinct from the sidecar, signing/notarization through Tauri's own
    env-driven path, DMG target, login item via `tauri-plugin-autostart`,
    `just desktop` / `just desktop-bundle`. The mechanism 18 §6 verified is
    the only thing between the `.app` and a working daemon.

## 1. Why

The TUI is complete and the RPC surface is one list; nothing has consumed it
from outside the process except smoke scripts. A desktop client is the
second frontend 00 promised, and the first that can show images, a tree of
agents beside a transcript, and a knowledge base as documents rather than
cards. The user wants it to look like the tool they already use all day
(ChatGPT) so there is nothing to learn, and to run as a menubar app so it is
one keystroke away. Feature parity matters because two frontends with
different capabilities means the TUI stays the real one.

## 2. Facts relied on (recon 2026-09-14)

### 2.1 RPC (`crates/rpc`)

- jsonrpsee 0.26, JSON-RPC 2.0 over WebSocket on `rpc.listen`
  (default `127.0.0.1:7777`, env `WTH_RPC__LISTEN`); token 64-hex at
  `<data_dir>/token`, presented as `Authorization: Bearer` **or** `?token=`
  on the upgrade (`lib.rs:120-141`). Loopback only, no TLS.
- 77 methods on `trait Api` (`api.rs:52-341`), every one a thin `ask`.
  Errors: `-32000` handler string, `-32001` actor not running.
- One subscription `subscribe_events(session?, descendants?)`; items are
  `{ "type": "<TypeName>", ... }`. `session: null` = everything
  (`api.rs:889-912`). Broadcast capacity 1024; overflow yields
  `{"type":"Lagged","count":n}` and the gap is unrecoverable
  (`bridge.rs:24`, `api.rs:866`). Nothing is replayed to a late subscriber.
- Bridged event types (`bridge.rs:61-71`): `AgentEvent` (20 variants),
  `ApprovalRequested/Resolved`, `McpProgress`, `LoginFinished`, `JobEvent`
  (`JobStarted|JobOutput|JobScreen|JobFinished`), `SignalEvent`,
  `SlackEvent`, `KnowledgeChanged`.
- `load_session` returns the whole transcript, unpaged today
  (`Loaded.history: HistoryEntry[]`, roles `user|assistant|tool|compacted`;
  no timestamps, no reasoning, no attachments). §7 pages it.
- Attachments are absolute paths on the daemon host (`send_input.files`),
  validated `image/* ≤ 5 MB`, `application/pdf ≤ 32 MB`
  (`supervisor.rs:2067-2105`). Convention for client-written files:
  `<data_dir>/attachments/<sha256[..16]>.<ext>` (`rpc/acp.rs:262-281`).
- `just ts` (`cargo test -p rpc --features ts export_bindings`) writes
  `bindings/*.ts` (gitignored). The export list omits `LoginInfo`,
  `ProviderInfo`, `AgentInfo`, `AgentState`, `LoginFinished`, and nothing
  exports method signatures; `bindings/` on disk predates Slack.
- No RPC: session rename/delete/fork, default-model or approvals getter,
  `data_dir`, title-change event, bytes upload, images in
  `ToolCallFinished`. §7 adds the ones the clone needs.

### 2.2 TUI (`crates/tui`)

- Pure `Model` fed by bus events and `Api` replies; 60 Hz drain of all
  pending events under one write per frame; idle frames do nothing
  (`ui.rs:123-152`). Typewriter reveals `max(1, backlog/16)` chars per frame.
  Only a 2-view window of entries is laid out per frame (00 §8: 27 ms frames
  at 150 turns without it).
- Commands: 30-odd slash commands (`model.rs:696-774`), each an `Api` call;
  menus: models, efforts (from the bundled catalog, no RPC), sessions,
  agents, MCP (per-tool switch + gate cycling), ACP, skills, logins, slash
  popup. Approval modal takes precedence over any menu.
- Out-of-band data the TUI reads in-process: `agent.model` and
  `tools.approvals` from config, `models::Catalog::closest()` for context
  limits / effort levels / vision, `data_dir` for clipboard PNGs.
- No session rename/delete/unread/fork, no `@mentions`, no input history,
  no help screen, no ask-user tool (a child's `send_message{await_reply}`
  shows as `awaiting` on the board).

### 2.3 Knowledge (`crates/knowledge`, okf 0.1.0-alpha.1)

- Bundle `<data_dir>/knowledge/`, ids = path minus `.md`, ≤3 segments,
  lowercased on write, reserved `index`/`log`. YAML-subset frontmatter
  (`type,title,description,tags,scope,source,timestamp`; unknown keys
  preserved by okf, **dropped by `knowledge_put`** which rebuilds it).
- Links are plain markdown: `[t](/projects/wth.md)` (bundle-absolute) or
  `[t](./wth.md)` (relative to the source's directory); `#anchor` stripped;
  `.md` stripped from the last segment; external schemes ignored; fenced and
  inline code skipped (`okf links.rs:38-146`). Resolution does **not**
  lowercase. Broken links are legal; `Concept.links: [id, exists][]`,
  `Concept.cited_by: id[]`.
- RPC: `knowledge_search/get/list/put/delete/history/restore/harvest`;
  event `KnowledgeChanged{id, op, source}`. External file edits are picked up
  by `sync()` at daemon open and every turn start; no watcher. `list` default
  limit 50, no paging; `search` drops terms < 3 chars and stopwords.
- Store has `write_concept(id, text)` (used by `restore`) and public
  `sync()`; neither is on RPC yet.

### 2.4 ChatGPT UI (chatgpt.com, unauthenticated shell, Chromium 1280×860, 2026-09-14)

Measured with computed styles; the full dumps are `assets/25-chatgpt-tokens.md`
(all 90 `:root` custom properties, dark and light) and
`assets/25-chatgpt-geometry.json` (element rects and computed styles). The authenticated
app is the same shell with a populated sidebar; the relay into the user's
Firefox tab did not attach, so §4.4's build step re-verifies against it.

| element | value |
|---|---|
| font | `-apple-system-body, ui-sans-serif, -apple-system, system-ui, "Segoe UI", Helvetica, …`; body 16/24; composer 17px |
| page bg / text | light `#fff` / `#0d0d0d`; dark `#000` / `#fff` |
| layers | light `#fff` `#f3f3f3` `#e8e8e8`, elevated `#fff`; dark `#212121` `#303030` `#414141`, elevated `#303030` |
| content colours | primary `#0d0d0d`/`#fff`, secondary `#5d5d5d`/`#cdcdcd`, tertiary `#8f8f8f`/`#afafaf`, accent `oklch(62.6% .205 255)` / `oklch(71.9% .154 248)` |
| borders | subtle `#0000000d`/`#ffffff1a`, default `#0000001a`/`#ffffff26`, strong `#0003`/`#fff3` |
| status | danger/info/success/warning bg+border+content triples (both themes) in the token dump |
| sidebar | 260 px; header row 52 px; items 36 px high, radius 10, padding 6×10, margin 0 6; sections: New chat, Search chats, …, bottom: Settings, Help |
| column | `<section max-width: 768px>` centred; messages 640 px content inside |
| user message | right-aligned button-like bubble, `max-width: 70%`, radius 22, padding 10×16, bg `#e8e8e8` / `#303030` |
| assistant | unboxed markdown at column width; `h1` 24/32 600 margin 0 0 8; `p` 16/26 margin 4 0; `li` padding-top 12; links underline offset 2px, text colour |
| code block | `pre` radius 16, padding 52 16 16 (header band with language + copy), bg `#f7f7f7` (light); 44 px icon buttons |
| tables | header row bold, 1 px subtle borders, copy button at top right |
| composer | form 768 wide, radius 28, padding 7×8, 1 px `rgba(0,0,0,.05)` border, elevated bg; leading `+` (attach), trailing send circle 34×30 area; multi-line grows |
| header | 52 px; model dropdown left (`ChatGPT ▾`), actions right |
| action bar | 44 px icon buttons under an answer (copy, share, …) |
| empty state | centred "Where should we begin?" 28 px above the composer |
| shortcuts (observed build) | ⇧⌘O new chat, ⌘K search chats, ⇧⌘S toggle sidebar, ⇧Esc focus composer, ⇧⌘C copy last response, ⇧⌘; copy last code block, ⌘/ shortcuts |

## 3. Parity: TUI feature → desktop surface

Every row is an `Api` call already present unless marked **new** (§7).

| TUI | Desktop |
|---|---|
| transcript, windowed | virtualized column (§6.3); same card kinds: user bubble, assistant markdown, reasoning (collapsible "Thinking…", dim), tool call row (`name  described args`, ✓/✗, click expands output in a code block), job tail (last 10 lines live, PTY screen rows), `⟵ name` peer message card, `↳ name` subagent spawn/cancel/error lines, `── context compacted (epoch · style) ──` divider with expandable summary, red error line |
| `describe()` of args (`$ cmd` for shell, `k: v` else, long strings collapsed) | `describe.ts`, same rules, unit-tested against the same fixtures |
| `◆ you` echo rules (skip own send, injected, harness wrappers) | same `injected()` classifier in TS; `<skill name>` → skill chip + words |
| header `model · effort · ctx used/limit (pct) · cache % · approvals off` | model dropdown (ChatGPT's header slot) shows model; right side: context meter pill (colour steps 70/90 %), cache %, `approvals off` badge |
| status line spinner states (`thinking…`, `running tool…`, `streaming…`, `compacting…`, `harvesting…`) | the ChatGPT pulsing dot under the last message + the same words in the composer placeholder area; window title suffix `·` while any session streams |
| `/model`, `/model <id>` | header dropdown (cmdk list from `list_models`, typed filter) → `set_model` |
| `/effort`, `/effort <lvl>` | second column in the same dropdown, from `ModelInfo.efforts` (**not** the catalog - `list_models` already carries `efforts`, `context`, `image`) → `set_effort` |
| `/title [model|off|clear]` | Settings › General › "Title model" |
| `/approvals [on|off] [always]` | Settings › Approvals (global + this session toggles) → `set_approvals`; state read from **new** `info()` |
| approval modal `y/n/o` | inline card at the tool row: `approval: tool (risk)`, described args, **Allow** / **Deny** buttons; also a sheet listing all pending approvals across sessions (badge on tray title); `o` = the card's "open session" link; macOS notification when the window is hidden |
| `/mcp` menu (switch, scope prompt, gate cycle, expand, `l`, `s`) | Settings › Tools: server rows with connect state, `On / Off here / Off always` segmented control (→ `set_mcp_hidden` / `set_mcp_gate`), tool sub-rows with risk select (→ `set_mcp_gate`), Login/Logout, Sync, Add (JSON or git URL), Remove (**exposes `remove_mcp_server`, which the TUI never did**); refreshed on `McpProgress` |
| `/acp` (`p` probe, add, remove) | Settings › Harnesses: installed + registry, Probe, Add, Remove |
| `/skills`, `/skills add|rm|sync`, `/<skill> args`, `s` show | Settings › Skills (list, body viewer, add repo, remove, sync); composer `/` popup runs `use_skill` for skill names as the TUI does |
| `/login`, `/logout`, `LoginFinished` | Settings › Connections: providers + OAuth MCP servers, Login opens the URL via the opener plugin, row flips on `LoginFinished` |
| `/provider`, `/provider add`, `/provider rm` | Settings › Connections › Gateways: table + add form (name, base URL, key; key never echoed) |
| `/knowledge …` | Knowledge explorer (§8); `harvest` is a button on the current session's menu |
| `/prompt` | session menu › "Show prompt" → sheet with the text in a code block |
| `/jobs`, `/jobs kill` | session menu › Jobs sheet (`list_jobs` table, kill) |
| `/sessions`, picker row `● id8 date model title` | sidebar (ChatGPT chat list): title (or first user line), grouped Today / Yesterday / Previous 7 days / Older by `updated_at`; active row highlighted; **new** `TitleChanged` keeps it live |
| `/resume <uuid>` | ⌘K search (titles + ids, client-side over `list_sessions`) |
| `/new` (lazy create) | New chat → route `/`, session created by the first `send_input(null, …)` exactly like the TUI; `SessionCreated` inferred from the reply |
| `/agents` picker + board (≤6 rows) + `/parent` | sidebar: the active session expands into its `list_agents` subtree (glyph · name · state · age), live from the subtree's events; clicking a child opens it with `↳ parent` breadcrumb in the header (19 §5) |
| `/clear`, `/compact [style]`, `/compact style …`, `/undo`, `/redo`, `/cancel` | session menu (compact with style submenu, clear, undo, redo); Esc / stop button in the composer = `cancel`; compact style default in Settings › General |
| steer while streaming + `◇ queued` chips + ⌥↑ withdraw | typing while streaming sends `steer(session, text, client id)`; chips above the composer with an × (`withdraw`) and click-to-edit (withdraw into the composer) |
| Shift+Enter newline, Enter send | same |
| paste → `[⎘ paste N]` token over 5 lines / 400 chars | same tokens, same expansion at submit (`pastes.ts`) |
| Ctrl+V image → PNG in `<data_dir>/attachments`, dropped paths | paste/drop of image or PDF → **Tauri command** `save_attachment(bytes, ext)` (same sha16 path convention; `data_dir` from `info()`); native drag-drop paths are passed straight through; thumbnails inline in the bubble; text-only model warning from `ModelInfo.image` |
| Ctrl+O toggle newest card | click any collapsible; ⌘⇧O is New chat here (ChatGPT binding) |
| PgUp/PgDn, ↑/↓ scroll, anchored scrolling during streaming | native scroll; stick-to-bottom with ChatGPT's "scroll to bottom" arrow when the user has scrolled up |
| `/quit` | tray › Quit, ⌘Q: RPC `shutdown` if we launched the daemon, then close the pty (18 §7 step 3); a reused daemon is left running |
| line printer / `-p` | n/a |
| `Lagged` | reload the visible session (`load_session`) and re-list sessions; toast "events dropped, reloaded" |

ChatGPT affordances with no TUI counterpart:

| affordance | decision |
|---|---|
| rename chat | **add** `rename_session` (§7, 5 lines over `store.set_title`) - the sidebar item menu without it looks broken |
| delete / archive chat | not in v0: no RPC, running sessions and subagent trees make it non-trivial; the item menu says so. Add `delete_session` when asked. |
| share, temporary chat, voice, projects, GPTs, plans | not shown |
| search chat contents | titles and ids only (no content-search RPC); the ⌘K dialog says "titles" |
| light / dark / system theme | Settings › General, default system (`prefers-color-scheme`), tokens from §2.4 |
| shortcuts dialog `⌘/` | yes, the table in §2.4 plus ours (⌥Space toggle window) |

## 4. Shape

### 4.1 Stack, with the alternative each choice beat

| need | choice | beat |
|---|---|---|
| shell | Tauri 2 (`tauri`, `tauri-build`), macOS-only build for v0 | Electron: 150 MB runtime beside an 86 MB sidecar, and 18's pty launch is Rust anyway; native SwiftUI: no shadcn/TanStack, a second UI codebase |
| UI | React 19 + TypeScript, Vite | Svelte/Solid: shadcn's canonical build is React; the user named the stack |
| routing | `@tanstack/react-router` (file-based, typed params) | react-router: untyped params; hand-rolled state: the settings modal and knowledge doc need real URLs for back/forward |
| server state | `@tanstack/react-query` for every request/response method (`list_sessions`, `load_session`, `list_models`, `knowledge_*`, settings lists), invalidated by events | putting stream state in the query cache: a cache write per token |
| event-fed state | `zustand` store: `sessions: Map<id, SessionSlice>`, `approvals`, `mcp`, `connection`; per-session selectors | Redux: ceremony; React context: every consumer re-renders per delta |
| lists | `@tanstack/react-virtual` (dynamic `measureElement`) for transcript and sidebar | react-window: no dynamic heights without measurement hacks |
| markdown | `streamdown` + `@streamdown/code` (shiki, lazy languages, block memoization, incomplete-markdown repair, GFM tables, math) | react-markdown + own block splitting = re-implementing 03 in TS; `crates/markdown` over RPC = `Style` spans on the wire (03 §2 already allows either) |
| components | shadcn/ui (radix): Sidebar, Dialog, Sheet, DropdownMenu, Command (cmdk), Popover, Tooltip, ScrollArea, Textarea, Button, Badge, Separator, Collapsible, Tabs, Switch, Select, Sonner toasts | headless-only (radix bare): we would write shadcn; MUI/Mantine: their look fights the clone |
| styling | Tailwind v4, tokens as CSS variables mapped onto shadcn's (`--background`, `--foreground`, `--muted`, `--border`, `--radius`, …) | CSS modules: the shadcn components expect Tailwind classes |
| icons | `lucide-react` | cloning OpenAI's glyphs (explicitly excluded) |
| editor | `@uiw/react-codemirror` + `@codemirror/lang-markdown` + `@codemirror/lang-yaml` | textarea: no highlighting/undo tree; Monaco: 5 MB for a markdown box |
| RPC client | 80 lines over the webview's `WebSocket`: id counter, pending map, `subscribe_events` notification demux, reconnect with backoff | jsonrpsee has no TS client; generic JSON-RPC libs add a dependency for less than a screen of code |
| package manager | bun (`bun install`, `bun run`), lockfile committed | npm: slower, nothing else uses it here |
| Rust deps | `portable-pty` 0.9 (already a workspace dep), `tauri-plugin-{opener,notification,global-shortcut,autostart,positioner,window-state}` | writing NSStatusItem/LaunchServices glue by hand |

### 4.2 Layout

```
crates/desktop/                 Tauri crate (workspace member by glob; kind: app, not a plugin)
  Cargo.toml                    tauri, tauri-build, portable-pty, tauri-plugin-*
  tauri.conf.json               productName "wth", mainBinaryName "wth-desktop", externalBin ["binaries/wth"]
  capabilities/default.json     core:default, opener, notification, global-shortcut, autostart, positioner, window-state, dialog(open)
  binaries/                     wth-aarch64-apple-darwin (copied by `just desktop-bundle`; gitignored)
  icons/                        template tray icon (monochrome), app icon (plain glyph, no wordmark)
  src/main.rs                   Builder: plugins, tray, activation policy, window show/hide, quit sequence
  src/launch.rs                 18 §7 step 1: probe → spawn on pty → drain transcript → wait for port
  src/commands.rs               #[tauri::command]: connection(), launch_log(), save_attachment(), relaunch()
  ui/                           Vite app (bun)
    package.json, vite.config.ts, tsconfig.json, components.json (shadcn), tailwind.css (tokens)
    src/rpc/client.ts           WebSocket JSON-RPC client + subscription demux
    src/rpc/api.ts              typed `Api` interface (77 methods + §7), hand-written against bindings/
    src/rpc/events.ts           `WireEvent` union = AgentEvent | ApprovalRequested | … | Lagged
    src/rpc/bindings/           copied by `just ts-desktop` from ../../bindings (gitignored)
    src/store/                  zustand: sessions.ts (slices, reducer per event), approvals.ts, connection.ts
    src/store/reduce.ts         pure `apply(slice, event)` mirroring tui::model::apply_agent (unit-tested)
    src/lib/describe.ts, injected.ts, pastes.ts, resolveLink.ts, hotkeys.ts
    src/routes/                 __root.tsx (shell: sidebar + outlet), index.tsx (empty state), c.$session.tsx,
                                knowledge.tsx, knowledge.$id.tsx, settings.$tab.tsx (dialog route)
    src/components/chat/        Transcript, Message{User,Assistant,Reasoning,Tool,Job,Peer,Divider,Error},
                                Composer, QueuedChips, ApprovalCard, ScrollToBottom, ModelMenu, ContextMeter
    src/components/sidebar/     SessionList, SessionItem (menu: rename, copy id, harvest, prompt, jobs), AgentTree, SearchDialog
    src/components/settings/    General, Approvals, Tools (MCP), Skills, Harnesses, Connections, Channels
    src/components/knowledge/   Tree, Document, Editor, Backlinks, History, SearchBox
    src/components/launch/      StartupScreen (pty transcript, 18 §4 timeout message, Relaunch)
```

`crates/desktop` is excluded from `cargo check --workspace` in CI only if the
Tauri build deps prove slow; default is in, like every other crate.

### 4.3 Connection lifecycle

```mermaid
sequenceDiagram
  participant App as Tauri (Rust)
  participant Sh as $SHELL -l -i (pty)
  participant D as wth --headless
  participant W as webview
  App->>App: read ~/.wth/config.yaml rpc.listen, ~/.wth/token
  App->>D: probe ws://127.0.0.1:7777/?token=… (500 ms)
  alt live daemon (terminal `wth --serve`, another app instance)
    App->>W: connection{url, token, owned:false}
  else none
    App->>Sh: spawn on pty, env += TERM, WTH_LAUNCH=1, WTH_RPC__LISTEN=127.0.0.1:<free>
    Sh->>D: exec <bundle>/wth --headless --data-dir ~/.wth
    App-->>W: launch_log events (pty transcript) until port accepts (≤15 s) or timeout screen
    App->>W: connection{url, token, owned:true}
  end
  W->>D: WebSocket, subscribe_events(null), info(), list_sessions()
  W->>D: on close: backoff reconnect; StartupScreen shows pty tail if owned and pty EOF
  Note over App,D: ⌘Q / tray Quit: W calls shutdown() if owned → App closes pty master → exit
```

- Readiness is the port, never the log (18 §2.5). Timeout surfaces the
  transcript with "your shell startup is waiting for input" (18 §4).
- Reconnect: the store marks `connection.state = reconnecting`; on
  reconnect it re-subscribes, re-fetches `list_sessions` and reloads the
  visible session (everything between is lost by design, §2.1).
- Dev (`just desktop`): `WTH_BIN` env or `target/debug/wth` replaces the
  bundled path; `--data-dir` may be overridden with `WTH_DATA_DIR` so the
  smoke data dir (`/tmp/wth-title`) can be used without touching `~/.wth`.

### 4.4 Visual spec

- `tailwind.css` defines the §2.4 tokens as CSS variables under `:root` and
  `.dark`, and aliases shadcn's variables to them (`--background:
  var(--bg-page)`, `--card: var(--bg-elevated)`, `--muted:
  var(--bg-layer-2)`, `--border: var(--border-default)`, `--radius: 0.75rem`).
  No hard-coded colour in a component.
- Shell: sidebar 260 px (collapsible to 0 with ⇧⌘S, ChatGPT's animation),
  traffic lights inset into the sidebar header (`titleBarStyle: Overlay`,
  `hiddenTitle: true`, `data-tauri-drag-region` on the header bars).
- Column: `max-w-[768px] mx-auto`; user bubble `ml-auto max-w-[70%]
  rounded-[22px] px-4 py-2.5 bg-layer-3`; assistant `prose`-less: the
  streamdown element classes carry the measured type scale.
- Composer: `rounded-[28px] p-[7px_8px] bg-elevated border border-subtle`,
  `+` popover (attach file, browse…), trailing send/stop circle; grows to 8
  lines then scrolls.
- Build step: side-by-side screenshots of the authenticated chatgpt.com
  thread (the user's Firefox) and the app at the same viewport, for the
  empty state, a thread with the markdown sample of §2.4, the settings
  dialog and the model dropdown; differences fixed until they are icon-only.
  This is the acceptance check for decision 6.

### 4.5 Window, tray, shortcuts

- `ActivationPolicy::Accessory` in `setup`; the default app menu is kept so
  ⌘C/⌘V/⌘A/⌘Q reach the webview (macOS routes edit shortcuts through the
  menu even when the menu bar is hidden).
- Tray: template icon; left click toggles the window (positioned under the
  tray on first show via `tauri-plugin-positioner` `TrayCenter`, then
  wherever the user left it via `window-state`); right click menu: Open,
  New chat, Knowledge, Settings, separator, Quit. Tray title shows `●N` while
  approvals are pending.
- Window close hides; ⌘Q quits (§4.3). Global shortcut ⌥Space toggles the
  window (`tauri-plugin-global-shortcut`), rebindable in Settings › General.
- Notifications (`tauri-plugin-notification`) only when the window is
  hidden: an approval request, or a turn finished in a session the user is
  not looking at. The desktop is not a `message_user` sink (§7).
- Login item: Settings › General › "Open at login" → `tauri-plugin-autostart`
  (a LaunchAgent for the *app*; the daemon still starts through 18's pty
  shell, so 18 §5's rejection of a daemon LaunchAgent stands).

## 5. Packaging (18, made concrete)

18 decided how a `.app` starts the daemon and verified the mechanism with a
Python stand-in. This section is the bundle around it.

- **Sidecar.** `bundle.externalBin: ["binaries/wth"]`; `just desktop-bundle`
  runs `cargo build --release -p wth` and copies `target/release/wth` to
  `crates/desktop/binaries/wth-aarch64-apple-darwin`. Tauri places it at
  `Contents/MacOS/wth`; `launch.rs` resolves it as a sibling of
  `current_exe()`. The exec line in 18 §2 becomes
  `exec '<that path>' --headless --data-dir <dir>`; nothing else changes.
- **Names.** `productName: "wth"` (the `.app` name, 00 §1.1: the only
  product name is the binary's), `mainBinaryName: "wth-desktop"` so the
  Tauri executable and the sidecar do not both want `Contents/MacOS/wth`.
  Identifier `wth.desktop` (Tauri requires reverse-DNS-ish; no domain
  implied).
- **Version lock.** The sidecar is built from the same commit as the UI;
  the terminal's `~/.cargo/bin/wth` from `just install` may differ. Both
  read `~/.wth`; `sessions.sqlite` is WAL and the schema is additive so far.
  Settings › General shows the daemon version from `info()` (version string
  only; no name).
- **Signing / notarization.** Tauri signs the bundle including
  `externalBin` when `APPLE_SIGNING_IDENTITY` is set and notarizes with
  `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` (or an API key). Unsigned local
  builds run ad-hoc signed. The sidecar's V8 archive and stealth TLS
  (23 §9) are plain Mach-O code, nothing that needs entitlements beyond the
  default. Hardened runtime is on by default; the daemon spawns the user's
  shell and tools, which hardened runtime permits (no library validation
  issue - no plugins are dlopen'd).
- **Targets.** `bundle.targets: ["app", "dmg"]`, `macOS.minimumSystemVersion`
  the Tauri default; arm64 only in v0 (`x86_64-apple-darwin` needs the
  sidecar built for it; the recipe accepts a target triple when wanted).
- **Data dir.** `~/.wth` by default (18: same as the terminal). The app
  never writes outside it except its own window state
  (`~/Library/Application Support/wth.desktop/`, Tauri's default).
- **Updater.** Not in v0 (no channel to serve from). `tauri-plugin-updater`
  is the one-line add when there is one.
- **just recipes.**
  ```
  ts-desktop:        just ts && cp bindings/*.ts crates/desktop/ui/src/rpc/bindings/
  desktop:           just ts-desktop && cd crates/desktop/ui && bun install && cd .. && cargo tauri dev
  desktop-bundle:    cargo build --release -p wth && mkdir -p crates/desktop/binaries && cp target/release/wth crates/desktop/binaries/wth-aarch64-apple-darwin && just ts-desktop && cd crates/desktop/ui && bun install && bun run build && cd .. && cargo tauri build
  ```
  `cargo-tauri` is a `just setup` addition (`cargo install tauri-cli --locked`).
- **What 18 left unexercised** and this phase must: the sidecar bundling
  itself, `bash`/`fish` as `$SHELL`, the blocking-rc timeout path.

## 6. Performance

The budget: 50 sessions streaming concurrently (a parent fan-out of
subagents is the realistic worst case), the visible one renders at 60 Hz,
input latency under 16 ms, memory linear in transcript size.

### 6.1 Event path

1. One `subscribe_events(null)` per connection. Filtering server-side per
   session would need one subscription per open session and still miss
   sidebar liveness. Descendants are routed client-side by `session`.
2. Notifications are pushed into a plain array; a `requestAnimationFrame`
   drains it into **one** `store.set` per frame (the TUI's 60 Hz drain,
   `ui.rs:123-152`). Idle frames schedule nothing.
3. `reduce.ts` applies events to the target `SessionSlice` only. Slices are
   separate objects; Zustand selectors are per session, so a delta for a
   hidden session invalidates no mounted component. Hidden sessions store
   text only (no markdown parse, no layout).
4. `TextDelta` appends to `slice.entries[last].text` when the last entry is
   an open assistant entry; any other event for that session closes it (the
   TUI rule, `model.rs:2145-2153`), so text before and after a tool call are
   separate entries and streamdown's memoized blocks stay stable.
5. `JobOutput` keeps a 10-line tail per job, `\r` rewind and ANSI stripped,
   like the TUI; `JobScreen` replaces rows.
6. `Lagged` → reload (§3). No attempt at replay.

### 6.2 Rendering

- Transcript: `useVirtualizer` over `slice.entries` with `measureElement`,
  overscan 4, bottom-anchored. Only the last entry is ever in a streaming
  state; every earlier `<Message>` is `React.memo` keyed by entry id with
  stable props, so a delta re-renders exactly one component.
- Markdown: `streamdown` with `isAnimating` on the open entry; completed
  blocks are memoized by the library; code highlighting is per block,
  lazy-loaded languages, token cache. Static mode (`mode="static"`) for
  history entries.
- Typewriter: none. ChatGPT reveals tokens as they arrive; the TUI's
  `backlog/16` reveal exists for terminal redraw cost, which a webview
  does not have. If a provider bursts (Anthropic sends sentences), the
  per-frame drain already smooths it.
- Sidebar: virtualized list; `list_sessions` refetched (debounced 500 ms) on
  `TurnFinished`, `SubagentSpawned`, `TitleChanged`, `SessionCreated`-by-reply.
- History load is paged: `load_session(session, before?, limit?)` returns
  the newest `limit` entries (default 60 in the app) plus `first_seq` and
  `more`; scrolling to the top fetches the previous page with
  `before: first_seq` and prepends without moving the viewport (the
  virtualizer's scroll offset is corrected by the prepended height). The
  TUI keeps calling it with `limit: null` and gets the old behaviour. A
  3000-entry session opens in one small payload; the whole transcript is
  never in the wire or the parser unless the user reads all of it.
- Images: `<img>` from `asset://` (Tauri asset protocol scoped to
  `<data_dir>/attachments`), thumbnails `max-h-64`, click opens a lightbox
  Dialog.

### 6.3 Measured acceptance (the §9 check)

Fixture: a bun script drives the daemon with 50 concurrent sessions each
told to write a 2000-word essay (`-p` style prompts against a cheap model or
a mock provider gateway added with `add_provider` pointing at a local echo
server that streams 40 tokens/s). The app has one visible. Chrome DevTools
performance trace via `tauri dev` devtools: no frame > 16 ms on the main
thread for 60 s, `store.set` count ≈ frames not events, heap growth ≈ text
size. If the visible session's markdown parse dominates, the next rung is
moving `streamdown`'s parse off-thread (it does not support workers today) -
recorded, not built.

## 7. RPC additions (server side, one commit, `api.rs` + owners)

Each is small and each is a real gap the clone hits; none changes existing
behaviour.

| method / event | shape | why |
|---|---|---|
| `info() → Info { version, data_dir, default_model, approvals: ApprovalsState, compact_style }` | new struct in `rpc` (the one place that knows all of them) | the TUI reads these in-process (`tui/lib.rs:63-70`); §3 header, attachments path, settings state |
| `rename_session(session, title: String) → ()` | `agent::RenameSession` → `store.set_title` + publish `TitleChanged` | sidebar item menu; the store fn exists (`loop_.rs:1204`) |
| `AgentEvent::TitleChanged { session, title }` | published where the titler writes (`loop_.rs:1204`, `title.rs`) and by `rename_session` | live sidebar titles without polling |
| `ToolCallFinished.files: Vec<PathBuf>` | from `ToolOutput.files` (already on the struct, `tools/lib.rs:107`) | screenshots and generated images inline, like ChatGPT; TUI ignores the field |
| `load_session(session, before?: i64, limit?: u32)`; `Loaded.first_seq: i64?`, `Loaded.more: bool` | `LoadSession` gains the two optional fields; the handler renders all rows (tool call/result pairs collapse across rows, so the slice is on rendered entries, keyed by `seq`; compaction dividers keep their `at_seq` position) and returns entries with `seq < before`, the last `limit` | §6.2: open a long session in one small payload; `ponytail:` the sqlite read is still all rows per page - a `seq`-ranged query is the upgrade if a measured session makes it slow |
| `knowledge_write(id, text) → Meta` | `KnowledgeActor::Write` → `Knowledge::write_concept` (exists, used by `restore`) with `source: user`, timestamp untouched | the raw editor; `knowledge_put` rebuilds frontmatter and drops unknown keys |
| `knowledge_sync() → Sync` | exposes `Knowledge::sync()` | explorer calls it on focus so files edited in another editor appear without waiting for a turn |
| `export.rs` list | add `LoginInfo`, `ProviderInfo`, `AgentInfo`, `AgentState`, `LoginFinished`, `Info`, `TitleChanged` (via `AgentEvent`) | bindings were incomplete |
| `bindings/methods.json` | `export_bindings` also writes `Server::into_rpc().method_names()` sorted | a vitest asserts `api.ts` keys == this list, both directions; the only drift check we need (no method-signature codegen) |

Not added, with the reason: `delete_session` (§3 table), bytes upload
(same host; `save_attachment` is a Tauri command),
`SessionCreated` event (the `send_input` reply carries the id; a foreign
client's new session shows up on its first `TurnStarted`), a desktop
`UserSink` for `message_user` (24 targets out-of-band channels; the desktop
is in-band - the transcript is where the agent talks to the user).

`docs/plugins.md` §"Commands" procedure applies: trait + impl, export
list, `methods.json`. `TitleChanged` is an `AgentEvent` variant, so the
bridge needs nothing. `knowledge_write` is not a tool; the model keeps
`knowledge_put`, so `harness.md` is unchanged.

## 8. Knowledge explorer

Route `/knowledge` (tree + search, no document) and `/knowledge/$id`.

- **Tree.** `knowledge_list(limit: 100000)` once (React Query, key
  `['knowledge','list']`), invalidated on `KnowledgeChanged`. Directories are
  derived from id segments; a node shows `title` (falls back to the last
  segment), a `type` badge, and the directory's count. Sorted by id. The
  tree is a shadcn Collapsible list, virtualized only if the bundle grows
  past ~2000 ids (`ponytail:` note; today it is 32).
- **Document.** `knowledge_get(id)` → header card (title, type, tags,
  description, scope, source, timestamp from `meta`), body = `text` with the
  frontmatter stripped (first `---…---` block), rendered by streamdown in
  static mode. Right rail: **Links** (`links` with `exists`; missing ones in
  the danger colour with "create"), **Cited by** (`cited_by`), **History**
  (`knowledge_history(id)`: commits, latest diff in a code block, Restore
  per commit → `knowledge_restore` after a confirm Dialog).
- **Working links.** A click handler on the rendered body intercepts `<a>`:
  `resolveLink(sourceId, href)` in `resolveLink.ts` mirrors okf
  (`links.rs:38-146`): external schemes → opener plugin; `#anchor` →
  in-page; `/abs/path.md` → from bundle root; `rel.md` → from the source's
  directory, `..` pops, `.md` stripped from the last segment, no
  lowercasing. Result → `navigate({ to: '/knowledge/$id' })`; if the id is
  not in the list, the "create" affordance opens the editor with
  frontmatter scaffold (`type` from the directory name, `title` from the
  link text). `index.md`/`log.md` are not concepts and not on RPC: the tree
  does not show them and links to them render inert with a tooltip. Unit
  test: the okf cases (absolute, relative, `..`, anchor, external, trailing
  `/`, `.md` strip).
- **Editor.** Edit button swaps the body for CodeMirror (markdown + yaml
  frontmatter highlighting, `⌘S` saves, Esc asks if dirty). Save →
  `knowledge_write(id, text)`; a `-32000` (okf conformance: non-empty
  `type`) shows inline under the editor. New document: `+` in the tree
  header → id prompt (validated by the id rules of §2.3: ≤3 segments,
  `[A-Za-z0-9_][A-Za-z0-9_.-]*`, not `index`/`log`; lowercased with a note)
  → editor with scaffold → `knowledge_write`. Delete: item menu, confirm
  Dialog listing `cited_by` count → `knowledge_delete`.
- **Search.** Box above the tree → `knowledge_search(q, 50)` results with
  snippets, Enter opens. Terms under 3 chars show the "3+ characters" hint
  (§2.3) instead of an empty list.
- **Freshness.** `knowledge_sync()` on window focus and on entering the
  explorer; `KnowledgeChanged` invalidates the list and the open document.
  Harvest of the current session from the session menu; `Harvesting`/
  `Harvested` toasts.
- **Not in v0.** Rename/move (no RPC; backlinks would break; the item menu
  states it), graph view, tag facets, multi-select, drag-drop into
  directories, `index.md` viewing.

## 9. Checks

Each phase ends with a runnable check.

| step | deliverable | check |
|---|---|---|
| 1 | §7 server additions + bindings + `methods.json` | `cargo test -p rpc --features ts`; `bindings/methods.json` lists `info`, `rename_session`, `knowledge_write`, `knowledge_sync`; a headless `TitleChanged` observed over ws after `rename_session` |
| 2 | `crates/desktop` shell: launch.rs, tray, Accessory, StartupScreen | `cargo test -p desktop` spawns `$SHELL -l -i -c 'exec env'` on a pty and finds `TERM`, `WTH_LAUNCH` (18 §7.1); `just desktop` from Finder-like env (`env -i HOME=… PATH=/usr/bin:/bin`) shows the transcript then connects; `ps eww` on the daemon shows the rc-file `PATH` |
| 3 | RPC client + store + reducer | vitest: `api.ts` ⟷ `methods.json`; `reduce.ts` fixtures (TextDelta append/close, ToolCallPending→Started by call_id, InputQueued dedupe, Compacted divider, SubagentSpawned adds child, Lagged flag); frame demux |
| 4 | chat shell: sidebar, transcript, composer, model menu, approvals, steering, attachments | smoke against `/tmp/wth-title`: send, stream, tool call with approval allowed from the card, steer while streaming, withdraw, paste an image, `⌘K` open another session, child opened from the tree, undo/redo |
| 5 | settings dialog (all tabs) | every §3 settings row round-trips: toggle an MCP server off-here and see the tool list shrink on the next turn; add a provider; login flow opens the browser and the row flips |
| 6 | knowledge explorer | open a concept, follow an absolute and a relative link, edit and save (frontmatter unknown key survives), create from a broken link, restore a previous commit, external edit + focus shows the change |
| 7 | visual clone | §4.4 side-by-side screenshots, four screens, light and dark |
| 8 | performance | §6.3 trace: no frame > 16 ms with 50 streaming sessions |
| 9 | packaging | `just desktop-bundle` produces `wth.app` with `Contents/MacOS/wth` and `wth-desktop`; launched from Finder with a `.zshrc` that exports a marker, the daemon's environment has it; quit leaves no `wth` process |

Unit tests earn their place only where a plausible bug fails them:
`reduce.ts`, `resolveLink.ts`, `describe.ts`, `pastes.ts`, `client.ts`
demux, the `methods.json` consistency test. No component snapshot tests.

## 10. Rejected

- **A web app served by the daemon** (15 §7) - 15 is shelved; the desktop
  needs 18's launch and OS integration regardless, and a login page is
  scope we do not have.
- **Reusing `crates/markdown` over RPC** (03 §2 allowed it): puts render
  work on the daemon per client and `Style` spans on the wire per token;
  streamdown does it in the webview with block memoization.
- **Proxying RPC through Tauri IPC**: double serialization per delta and a
  Rust client we would have to write (`jsonrpsee` client feature + types).
- **One `subscribe_events` per open session**: misses sidebar liveness;
  N subscriptions for N sessions; the server filter saves nothing the client
  does not do in one `Map` lookup.
- **Putting stream state in React Query**: a cache write per token,
  `structuralSharing` on a growing string.
- **NSPanel / popover window**: transient UI for a long-form transcript;
  private-API dependency.
- **Dock app with a menu bar**: the ask was a menubar app; Accessory policy
  costs nothing and the tray gives the toggle.
- **Electron**: §4.1.
- **Bundling the daemon as a LaunchAgent**: 18 §5.
- **`Command.sidecar` (shell plugin) to start the daemon**: no pty, so 18's
  environment argument fails; portable-pty is already in the workspace.
- **Typewriter reveal**: §6.2.
- **Method-signature codegen** (OpenRPC / proc-macro emitting TS): 77 lines
  of hand-typed interface plus a name-set test is smaller and the wire types
  are already generated.
- **Session delete/archive in v0**: §3.
- **Desktop as a `UserSink`**: §7.

## 11. Amendments to 00 (same commit)

- §2 "How a GUI starts the daemon" already points at 18; add "packaging in
  25 §5".
- §10 tree: `desktop/  app: Tauri menubar frontend, RPC client of Api - not a plugin (25)`.
- §12 row 4 status: designed 2026-09-14 (25); not started.
- §7: the §7 additions above land as `Api` methods; `info()` is the one
  getter the TUI may also switch to later so it stops reading config
  in-process (not required by this doc).

## 12. Status (as-built v0, 2026-09-15)

Shipped in four commits: `9931c14` (server §7), `3bc520d` (`crates/desktop`
shell), `e44ac60` (UI), this doc. `crates/desktop/` holds the Tauri crate and
`ui/` the Vite app; `just desktop`, `just desktop-bundle`, `just
desktop-check`, `just ts-desktop` are the recipes.

Deviations from the sections above:

- **`externalBin` lives in `tauri.bundle.conf.json`**, not `tauri.conf.json`:
  tauri-build fails `cargo check --workspace` when the sidecar file is
  absent. `just desktop-bundle` passes `--config tauri.bundle.conf.json`.
- **`knowledge_write` stamps `source: user` and a fresh `timestamp`** (§7 said
  timestamp untouched); it mirrors `knowledge_put` and preserves every other
  frontmatter key, which was the point.
- **`Loaded.first_seq` is typed `number | null` in TS** via `ts(type = …)`;
  ts-rs maps `i64` to `bigint`, which JSON never carries.
- **`load_session` paging also drops in-flight tool entries** (`result:
  None`) from older pages so the live tail is never repeated.
- **⌥Space is fixed**, not rebindable (§4.5): add the setting when asked.
- **The slash popup is a hand-rolled list** styled like cmdk, not a `Command`
  (forwarding synthetic key events from the textarea into cmdk was worse).
- **`api.ts` has 72 request methods + the 4 additions** (the recon's 77
  counted subscription plumbing); `methods.json` is the source of truth and
  the vitest compares against it.
- **Vite dev**: `optimizeDeps.entries` scans every module so code-split routes
  do not trigger a mid-session full reload (dev only; the bundle is static).

Verified (2026-09-15, browser-mode UI at `localhost:1420` against
`/tmp/wth-title`, then the real Tauri binary):

- Gates: `cargo clippy --workspace --all-targets -D warnings`, `cargo test
  --workspace` (incl. `desktop`'s pty env test), `tsc -b`, vitest 47/47
  (reducer, resolveLink, describe, injected, pastes, jobTail, mcpEntry,
  frontmatter, `api.ts` ⟷ `methods.json`).
- Chat: history session opens paged (`limit 60`); a new chat streams a turn
  with a `shell` tool call, a GFM table and a highlighted Rust block; the
  sidebar title updates live from `TitleChanged`; the context meter and cache
  % fill from `TurnFinished`.
- Knowledge: tree from two seeded docs; absolute `/projects/desktop.md` and
  relative `../facts/smoke.md` links navigate; the missing link renders in
  the danger colour with `→ missing` in the rail; Edit → CodeMirror → ⌘S
  writes through `knowledge_write` — `extra: keep` survives, `source: user`
  stamped, `log.md` gains `put projects/desktop`.
- Settings: all eight tabs render against the live daemon (MCP empty, four
  provider accounts, Signal link URL, ACP registry); Escape closes back to
  the previous route.
- **Performance (§6.3)**: `crates/desktop/perf_fixture.ts` drove 50 sessions
  streaming at once (~11 000 events in 18 s, peak ≈ 700 `TextDelta`/s) with
  one open in the UI: 959 frames in 16 s = **59.9 fps, worst frame gap 25
  ms, zero long tasks, heap 81 MB**. The budget in §6 holds with margin.
- **Launch (18)**: `wth-desktop` started under `env -i HOME USER SHELL TMPDIR
  PATH=/usr/bin:/bin:/usr/sbin:/sbin` spawned `wth --headless` on a pty via
  `zsh -l -i -c exec …`; a `shell` tool call inside that daemon printed
  `WTH_DESKTOP_MARK=rc-was-sourced` (a marker exported from `~/.zshrc`),
  `WTH_LAUNCH=1`, `TERM=xterm-256color` and the rc-file `PATH` (rbenv, atuin,
  homebrew). The webview connected (two established sockets). `SIGTERM` to
  the app left no daemon behind.

Not exercised: the `.app` bundle itself (`just desktop-bundle`, signing,
DMG); tray click / ⌥Space / notifications / autostart in the native window
(the webview was verified in Chromium, the Tauri window only through its
daemon); approvals inline card with a real `ApprovalRequested` (approvals
are off in the smoke config); Signal/Slack tabs beyond rendering status;
the §4.4 side-by-side screenshot against the authenticated chatgpt.com
(sidebar `#f9f9f9`/`#171717` still the design's guess); bash/fish as
`$SHELL`; the blocking-rc timeout screen.
