# 23 — Browser: an embedded headless browser the model can drive

Status: as-built v0 · 2026-09-14 (draft same day) · extends `00-architecture.md` §1 (single
binary), §4 (tools), §6; `16-web.md` §5 (rendered fetch); `17-images.md`
(images now flow out of tools, not only in from users). §13 records what
shipped and what was not exercised.

## 0. Decisions in one screen

1. **Obscura, embedded.** `crates/browser` links the
   [obscura](https://github.com/h4ckf0r0day/obscura) engine (V8 via
   `deno_core`, own DOM/layout/paint, Rust, Apache-2.0) into the theuth binary.
   No Chrome, no sidecar process, no download at runtime. 00 §1's single
   binary is not negotiable; the alternatives (§10) all put a browser
   somewhere else.
2. **Reuse obscura's agent tool layer, not its facade.** The `obscura`
   crate's public API is `goto/evaluate/query_selector/click`. The 2.4k
   lines that make a browser usable by a model — element refs, fill, forms,
   tabs, wait, scroll, readable snapshot, screenshot — live in `obscura-mcp`
   behind `pub(crate)`. We depend on `obscura-mcp` directly and call its
   tool dispatch in-process through a 20-line fork (§3.3).
3. **One isolate per session, on its own thread.** `Page`/`BrowserState`
   are `!Send` (RefCell + V8). Each theuth session that browses gets an OS
   thread owning a `current_thread` runtime and one `BrowserState`; commands
   arrive over a channel. Subagents are sessions, so they are isolated for
   free (cookie jar, tabs, refs). Measured: two isolates on two threads load
   pages concurrently (§2).
4. **Stealth on by default**, `render` on always. The `stealth` cargo
   feature (wreq/BoringSSL TLS fingerprint, tracker blocklist) is compiled
   in; `browser.stealth: true` is the default and `false` is a runtime
   switch. `render` is what makes `browser_screenshot` exist.
5. **Sixteen curated tools with our names and descriptions**, mapped onto
   obscura-mcp's 37 (§4). Descriptions are static (cache-safe). Output is
   tidied (blank-line collapse, trim, `max_chars`) before the model sees it.
6. **Screenshots reach the model as image parts of the tool result.**
   `ToolOutput` gains `files: Vec<PathBuf>`; the loop turns them into
   `genai::Binary` on the `ToolResponse` (a genai fork patch, §5); Anthropic
   gets `tool_result.content: [text, image]`, OpenAI-compatible gets a
   following user turn with the image, Gemini gets `inline_data` parts.
   Text-only models got `[screenshot omitted]` via the `Frame.vision` strip
   (17 §3.3); since 52 (2026-09-19) they get a stable marker and, with
   `agent.vision_model` set, a fallback model's answer to the main model's
   question (17 §12).
7. **Browser tools are `Risk::ReadOnly`, except cookie import.** The profile
   is ephemeral: fresh cookie jar per session, nothing outlives it, no user
   credentials — *unless* `browser.import_cookies` is on, which adds two agent
   tools: `browser_list_profiles` (`ReadOnly`, discover local profiles) and
   `browser_import_cookies` (`Mutating`), which copies the user's real browser
   cookies (via `unjar`) into a session so the agent acts as the signed-in
   user (§7). Off by default; not registered when off. Broader persistent
   storage (`storage_state`) is still v1.
8. **Private networks stay blocked by default**
   (`browser.allow_private_network: false`), flipped per installation. This
   deviates from 16 §5's "not configurable off" on purpose: verifying a dev
   server's UI on `localhost` is the coding-agent use case; obscura's own
   guard is the default and the flip is explicit config, not a tool
   argument.
9. **`read_url rendered: true` works keyless**: `browser` registers a
   rendering `Provider` with `web` over the bus (16 §3.1), so the Firecrawl
   dependency for rendered fetches is gone.

## 1. Why

theuth can search and fetch (16) but cannot *use* a page: no JavaScript, no
forms, no clicking through, no reading a SPA, no looking at what a dev
server actually renders. Every coding agent verifies UI in a browser; oh-my-pi
does it with Chromium via CDP and a relay to the user's Chrome. theuth's
constraint is one binary with everything included, so the browser has to
be a library.

## 2. Facts relied on (measured 2026-09-14, macOS arm64)

Release binary `obscura 0.2.2` driven over MCP stdio, and a scratch crate
depending on `obscura` + `obscura-mcp` at tag `v0.2.2`:

- **Build**: 57 s cold without stealth, +70 s for stealth (BoringSSL via
  `wreq 6.0.0-rc.29`), +43 s for `render` (`taffy`, `cosmic-text`,
  `obscura-render`). V8 is a **prebuilt static lib download** (`v8 137.3.0`
  through `deno_core 0.350`), not a source compile; the README's "5 min" is
  stale. The JS bootstrap snapshot is built by `obscura-js`'s build script.
- **Binary**: 60 MiB plain, 65 MiB stealth, 86 MiB stealth+render (release,
  unstripped). theuth grows by about that.
- **Isolates**: `obscura-js/src/runtime.rs:50-60` — isolate *construction*
  is serialised by a process lock, execution is per-thread and parallel;
  `obscura-cdp` is thread-per-connection. Two isolates on two threads:
  HN loaded in 1.1 s while example.com loaded in 61 ms alongside. The
  AGENTS.md line "single V8 isolate per process" describes their test
  harness, not the engine.
- **First isolate on the main thread**: `obscura-cdp/src/server.rs:363-369`
  builds and drops one `ObscuraJsRuntime` on the main thread before any
  connection thread exists, because the first isolate built off-main once
  segfaulted in `InitializeBuiltinJSDispatchTable`. Our scratch run built
  both isolates off-main without incident; we still do the warmup (§3.1).
- **MCP layer**: 37 tools, none annotated; `browser_snapshot` = `URL /
  Title / body text / "N interactive elements registered"`;
  `browser_interactive_elements` = `ref=e2 input[text] "q" name="q"`; refs
  are `data-obscura-ref` attributes stamped into the DOM and resolved to
  selectors. `fill ref=e2` + `click ref=e3` performed a real POST + redirect.
  `press_key Enter` on an input did **not** submit the form. Body text is
  whitespace-noisy (3–5 blank lines between HN rows). `browser_screenshot`
  returns an `image/png` block (HN: 200 KB base64; a correct, legible
  render). `http://127.0.0.1:…` → `Access to private/internal IP address
  127.0.0.1 is not allowed` unless `--allow-private-network`.
- **Vendored patches**: obscura's root `[patch.crates-io]` points `taffy`
  and `cosmic-text` at `vendor/`. Patches do not propagate through a git
  dependency; declaring them in **our** root manifest as
  `{ git = <obscura fork>, branch = "main" }` resolves to the vendored
  copies (`taffy 0.12.1`, `cosmic-text 0.14.2` compiled from the obscura
  source). Verified by build.
- **Build prerequisites**: `cmake` and `clang`/`libclang` for BoringSSL;
  network on first build for the V8 archive (`RUSTY_V8_ARCHIVE` for
  offline). `panic = "unwind"` (the default, which theuth keeps) is required
  for obscura's `catch_unwind` around DOM ops.
- **Runtime knobs are environment variables** read by the library:
  `OBSCURA_ALLOW_PRIVATE_NETWORK`, `OBSCURA_TIMEZONE` (default
  `Europe/Berlin`), `OBSCURA_PROFILE`, `OBSCURA_ROTATE_PROFILE`,
  `OBSCURA_GEOLOCATION`, `OBSCURA_SCRIPT_DEADLINE_MS`.
- **Stealth does not handle**: Cloudflare interactive challenges, Datadome,
  Akamai, CAPTCHAs (DDG served a duck captcha to a stealth form post), IP
  rate limits. It handles TLS/UA fingerprint checks and tracker reachability.

## 3. Shape

### 3.1 Plugin and warmup

`crates/browser` is a `runtime::Plugin` like `web` (`crates/web/src/lib.rs:81`):
reads `browser:`, spawns the `Browsers` kameo actor registered as
`BROWSER_ACTOR = "browser"`, registers the tools once the registry is up,
and registers the render provider once `web` is up.

`browser::warmup(&BrowserConfig)` runs **on the main thread** from
`theuth/src/main.rs::run` right after the config is loaded and before plugins
start: it exports the environment knobs from config (`std::env::set_var`
while the process is still effectively single-threaded — the only place
this is safe), then builds and drops one `obscura_js::runtime::ObscuraJsRuntime`
exactly as `obscura-cdp` does. Skipped when `browser.enabled: false`.
Cost: tens of milliseconds, paid once per launch.

### 3.2 One thread per session

```rust
// crates/browser/src/session.rs
pub struct Handle { tx: mpsc::Sender<Cmd> }              // Clone
enum Cmd { Call { name: String, args: Value, reply: oneshot::Sender<Result<Value, String>> } }
pub fn spawn(cfg: &BrowserConfig) -> Handle;             // std::thread + current_thread runtime
impl Handle { pub async fn call(&self, name: &str, args: Value) -> Result<Value, String>; }
```

The thread owns `obscura_mcp::BrowserState::new(proxy, user_agent, stealth)`
and loops exactly like `obscura_mcp::run` (`lib.rs:248-299`): `select!`
between the next `Cmd` and `state.advance_active_page_tasks()` while the
active page has pending JS work, so timers and fetches keep running between
tool calls. Dropping every `Handle` closes the channel; the loop ends, the
state drops, the isolate dies with the thread.

`Browsers` (the actor) maps `SessionId → (Handle, last_used)`. `Call {
session, name, args }` creates the handle on first use. A sweeper every 30 s
drops handles idle longer than `browser.idle_secs` (default 300); `Close {
session }` drops one now; `rt.shutdown` drops all. Not a job (14): invisible
in `/jobs`, no log file — a browser is a capability, not a process the user
started.

`Screenshot { session } -> Option<String>` returns the current page as a
base64 PNG for a live viewer; `None` when the session has no isolate or no
open page. It looks the handle up without spawning and does not refresh
`last_used`, so a viewer never keeps an isolate alive. It queues on the
session's thread like any command, so a frame waits behind an in-flight
`browser_wait`. This is the in-process equivalent of obscura's
`Page.captureScreenshot` polling guide
(https://docs.obscura.sh/guides/watch-agent-sessions-live): the embedded
engine has no CDP socket.

### 3.3 The fork

`obscura-mcp` keeps its dispatch `pub(crate)` and its only public entry
point, `run()`, is wired to the process's stdin/stdout. Fork
`github.com/skymoore/obscura` (`main` = tag `v0.2.2` + two commits) exposes:

```rust
pub fn tools() -> Vec<serde_json::Value>;               // the tools/list array
pub async fn call_tool(state: &mut BrowserState, name: &str, args: &serde_json::Value)
    -> Result<serde_json::Value, String>;                // the tools/call `result` object
pub fn has_active_page_runtime(&self) -> bool;           // on BrowserState
pub async fn advance_active_page_tasks(&mut self) -> Result<bool, String>;
// second commit (44 §3.1): tool `browser_network_capture { clear?: bool }` — on_response
// exchanges with request headers and textual bodies; obscura-js `op_fetch` fixed so
// `RequestInfo.headers` carries the *request* headers
```

Result shape is MCP's: `{ "content": [ {type:"text", text} | {type:"image",
data, mimeType} ], "isError": bool }`. Upstreamed as a PR; the fork is the
build source until it lands (00 §1.3). The dep is
`obscura-mcp = { git = "https://github.com/skymoore/obscura", branch = "main",
features = ["stealth", "render"] }` with the reason comment, plus the two
`[patch.crates-io]` rows from §2.

## 4. Tool surface

Names are ours; each maps to one obscura-mcp tool and passes its arguments
through unchanged unless the row says otherwise. Descriptions are static
strings written for a model that has never seen obscura: say what the
refs are, that refs go stale after navigation, and that **forms are
submitted by clicking their submit button** (Enter does not submit, §2).

| theuth tool | obscura-mcp | args | notes |
|---|---|---|---|
| `browser_open` | `browser_navigate` | `url`, `wait?: load\|domcontentloaded\|networkidle0` | reply ends with the tidied snapshot so one call reads the page |
| `browser_read` | `browser_snapshot` / `browser_markdown` | `format?: text\|markdown` (text), `max_chars?` | text = readable body + ref count; markdown keeps links |
| `browser_elements` | `browser_interactive_elements` | — | `ref=eN kind "label"` rows |
| `browser_click` | `browser_click` | `ref?`, `selector?` | reply ends with the snapshot |
| `browser_fill` | `browser_fill` | `ref?`, `selector?`, `value` | sets value, fires input/change |
| `browser_type` | `browser_type` | `ref?`, `selector?`, `text` | appends keystrokes |
| `browser_press` | `browser_press_key` | `key`, `selector?` | "to submit a form click its button" |
| `browser_select` | `browser_select_option` | `ref?`, `selector?`, `value` | by value or visible text |
| `browser_scroll` | `browser_scroll` | `direction?`, `amount?`, `ref?`, `selector?` | |
| `browser_wait` | `browser_wait_for` / `browser_wait_for_text` | `selector?` xor `text?`, `timeout?` (s, ≤ 60) | |
| `browser_eval` | `browser_evaluate` | `expression` | result as JSON; multi-statement snippets must be an IIFE |
| `browser_back` | `browser_back` | — | |
| `browser_screenshot` | `browser_screenshot` | `width?`, `height?` (CSS px) | PNG written to `<data_dir>/browser/<session>/shot-<n>.png`; `ToolOutput.files = [path]`; text `screenshot <w>×<h> attached` |
| `browser_tabs` | `browser_tab_list` / `_new` / `_switch` / `_close` | `action: list\|new\|switch\|close`, `tab_id?`, `url?` | one tool, four actions |
| `browser_close` | `browser_close` | — | closes every tab; the isolate stays until idle |
| `browser_list_profiles` | — (`unjar`) | — | `ReadOnly`; registered only when `browser.import_cookies`. Lists local browser profiles (`browser  id  path`) so the agent picks one to import |
| `browser_import_cookies` | `browser_set_storage_state` | `browser?`, `domains?`, `profile?` | `Mutating`; registered only when `browser.import_cookies`. Reads local profile cookies via `unjar`, maps them (session cookie `expires 0` → no-expiry, not delete), applies in one call |
| `browser_show` | — (bus) | — | `ReadOnly`; publishes `BrowserEvent::ShowLive { session }`; the desktop opens the live view (25 §7). Other frontends ignore it; the tool cannot tell whether a desktop is attached |

Every tool is `Risk::ReadOnly` (§0.7) except `browser_import_cookies`
(`Mutating`). Not exposed: cookie export (`browser_storage_state`), per-cookie
`get`/`set`/`clear`, PDF, `network_requests`, `console_messages`, `links`,
`extract`, `detect_forms`, `fill_form`, `get_attribute`, `count`, `search`,
`forward`, `reload`. They are one table row each when wanted. The fork's
`browser_network_capture` is consumed by the `routes` plugin (44 §3.2), not
registered as a `browser_*` tool.

`BrowserEvent` is the crate's one wire event (`#[serde(tag = "type")]`, one
variant `BrowserShowLive { session }`), published on the bus like
`tools::JobEvent`.

**Output tidy** (`tools::tidy`): trim every line's trailing whitespace,
collapse runs of ≥ 2 blank lines to one, trim ends, then cut at `max_chars`
(default `browser.max_chars`, 4000) with `…(truncated, N more chars)`. The
registry's spill (07 §6) still bounds the whole result.

**Errors** come back as `ToolOutput::err(text)` when obscura reports
`isError`; a dead browser thread (panic inside the isolate) is reported once
as `browser restarted: <reason>` and the next call spawns a fresh one.

## 5. Images out of tools

17 brought images *into* the conversation from users. Screenshots are the
first tool that hands one *back*. Three layers, each one field:

1. **`tools::ToolOutput.files: Vec<PathBuf>`** (`#[serde(default)]`). Paths,
   not bytes, like `files` on inputs (17 §2). `browser_screenshot` filled
   it first; `computer_screenshot` (27) is the second tool filling
   `ToolOutput.files`.
2. **`genai::chat::ToolResponse.images: Vec<Binary>`** (fork
   `github.com/skymoore/rust-genai`, `main` tracks upstream with the tool-image
   and Bedrock profile-isolation patches; `[patch.crates-io]`, commit pinned in `Cargo.lock`), with
   `ToolResponse::with_images`. Adapters:
   - Anthropic (`adapter_shared.rs` user and tool branches):
     `tool_result.content` becomes `[{type:text}, {type:image, source:
     base64}…]` when images are present; URL-sourced images are skipped
     with the existing warning.
   - OpenAI-compatible: the `tool` message stays text; images follow as one
     `{"role":"user","content":[{type:image_url, url:"data:<mime>;base64,…"}…]}`
     right after that message's tool responses — the shape OpenAI accepts,
     since tool messages cannot carry images.
   - Gemini: `functionResponse` parts then `inline_data` parts in the same
     user turn.
   Rejected alternative: emit a separate user message from theuth after the
   tool message — genai's Anthropic adapter already accepts `Binary` parts
   in a user message, so it would work there, but OpenAI's would drop the
   `ToolResponse` parts of a user-role message, and a user turn that is not
   a user turn confuses history readers and compaction.
3. **The loop** (`loop_.rs::round`): `ToolResponse::new(call_id, content)
   .with_images(files → Binary::from_file)`; an unreadable file becomes a
   text note. `frame::strip_images` learns the Tool role: drop the images,
   append `[screenshot omitted: model is text-only]`. `compact::est_content`
   gets a `ToolResponse` arm — `chars(content)/4 + images × IMAGE_TOKENS` —
   instead of falling into the serialise-everything arm, which would count
   200 KB of base64 as 50k tokens. `text_of` and `snap.rs` render `[image:
   shot-3.png]`. History rows carry the base64 inline as they do for user
   images; ~200 KB per screenshot is accepted.

TUI/Slack/Signal show the text line; rendering the PNG in a frontend is
not v0 (Slack upload would be `files.uploadV2`, one call, when asked).

Since 52 (built 2026-09-19) a `browser_screenshot` called from inside a
`python` cell also reaches the model: the inner result's file is snapshotted
into `<spill>/<session>` when it arrives and rides on the cell's
`ToolOutput.files` (08 §4.1), so the same `ToolResponse.images` path applies;
two screenshots overwriting `shot-<n>.png` stay two distinct images. Before
this the Python value conversion dropped the file. Not exercised live in the
52 run; the repl has a unit test for the overwrite case.

## 6. Config

```yaml
browser:                        # design doc 23
  enabled: true
  stealth: true                 # wreq TLS fingerprint + tracker blocklist (§0.4)
  proxy: null                   # http://user:pass@host:port | socks5://host:port
  user_agent: null              # default: obscura's current-Chrome UA for the platform
  timezone: null                # IANA zone; null = the host's zone (not obscura's Europe/Berlin)
  geolocation: null             # "lat,lon"
  allow_private_network: false  # §0.8
  import_cookies: false         # add browser_import_cookies: load the user's real cookies (§7)
  idle_secs: 300                # isolate teardown after inactivity
  max_chars: 4000               # default cap of browser_read / browser_open replies
  script_deadline_ms: 30000     # OBSCURA_SCRIPT_DEADLINE_MS for heavy SPAs
  capture_entries: 256          # OBSCURA_NETWORK_BODY_BUFFER_ENTRIES: exchanges kept per page (44 §5)
  capture_body_bytes: 2097152   # OBSCURA_NETWORK_BODY_BUFFER_BYTES: per captured body (44 §5)
```

No credentials. `timezone` defaults to the host zone via `iana-time-zone`
(already in the tree through `chrono`) so `Date` matches the exit IP of a
home user; a proxy user sets it to the exit region per obscura's guide.

## 7. Security

- SSRF: obscura's gate (loopback, RFC1918, link-local, DNS-resolution-time
  re-check, applied to redirects, JS `fetch()` and `Continue` rewrites) is on
  unless `allow_private_network`. theuth's own `read_url` guard is unchanged.
- The isolate is the sandbox for page JavaScript; `browser_eval` runs the
  model's JS inside the page, not in theuth. It can read the page, not the
  machine.
- No persistent profile: nothing the model logs into survives the session.
  With `import_cookies` off (default) no user cookies are ever loaded, which
  is what makes `ReadOnly` honest. Turning it on is an explicit, per-install
  decision to hand the agent the user's real sessions: the one tool it adds is
  `Mutating` (approval-gated), reads cookies through `unjar` from the local
  profile (Chrome/Edge/Brave decryption may prompt the OS keychain), and loads
  them into an ephemeral jar that still dies with the session.
- Heap and script deadlines are obscura's (`OBSCURA_SCRIPT_DEADLINE_MS`,
  watchdog termination); a runaway page kills its isolate, not theuth
  (`catch_unwind` on ops, `panic = "unwind"` kept).

## 8. Module layout and contracts

```
crates/browser/
  Cargo.toml        obscura-mcp (fork, stealth+render), tools, runtime, web, kameo, tokio,
                    serde, serde_json, schemars, anyhow, tracing, base64, async-trait, reqwest
  src/lib.rs        BrowserConfig, BrowserPlugin, Browsers actor (Call/Close/Screenshot/sweep), BrowserEvent, warmup(), BROWSER_ACTOR
  src/session.rs    Handle, spawn(): thread + current_thread runtime + BrowserState + pump loop
  src/tools.rs      the 16 Tool impls, tidy(), screenshot file writing; `pub fn all(actor, cfg, rt) -> Vec<DynTool>`
  src/render.rs     ObscuraRender: web::Provider { renders: true }; `pub async fn register(rt, actor)`
```

`Browsers::Call { session, name, args } -> Result<Value, String>` is the
one internal contract; `tools.rs` and `render.rs` never touch a `Handle`.
The render provider uses a fixed private `SessionId` so it has its own
isolate: `browser_navigate` → `browser_markdown { max_chars: 1_000_000 }`
→ `browser_close`, returned as `web::Rendered { markdown, tier: "render", .. }`.
Since 2026-09-17 (16 §13.17) `web::Provider` has two entry points — `read`
(cheapest real content) and `render` (always the browser) — and this provider
implements only `render`, so it is the browser for both `read_url {rendered:
true}` and a `search_web` read when `scrape` is disabled; with `scrape` on,
`web` prefers `scrape` by name and this one is the fallback (38 §5).

## 9. Build and dependencies

Root manifest additions:

```toml
# obscura-mcp: the agent tool layer of the obscura headless browser (23 §3.3).
# Fork `main` = tag v0.2.2 + two commits: `tools()`/`call_tool()`, then `browser_network_capture` (44 §3.1).
obscura-mcp = { git = "https://github.com/skymoore/obscura", branch = "main", features = ["stealth", "render"] }

[patch.crates-io]
# obscura vendors corrected taffy/cosmic-text; a git dependency's patches do not
# propagate, so we point at the same vendored copies (23 §2).
taffy = { git = "https://github.com/skymoore/obscura", branch = "main" }
cosmic-text = { git = "https://github.com/skymoore/obscura", branch = "main" }
# genai: `ToolResponse.images` so screenshots ride inside tool results (23 §5).
genai = { git = "https://github.com/skymoore/rust-genai", branch = "main" }
# btls-sys: wreq's BoringSSL renamed `links = "btls"` and symbol-prefixed on macOS too (§9.1).
btls-sys = { git = "https://github.com/skymoore/btls", branch = "main" }
```

Prerequisites documented in README: `cmake`, `clang` + `libclang`,
`llvm-objcopy` + `llvm-nm` (`brew install cmake llvm` / `apt install cmake
clang libclang-dev llvm`), network on first build. `justfile` unchanged.
CI installs them.

### 9.1 Two BoringSSLs in one binary (found at first `cargo check`)

Stealth means `wreq`, whose TLS is `btls-sys` — a BoringSSL build with
`links = "boringssl"`. The Signal channel (09) already links libsignal's
`boring-sys`, also `links = "boringssl"`. Cargo refuses two packages with
one `links` value, and even past that the two static archives would collide
on every `SSL_*` symbol at link time. Three ways out, in the order they were
rejected: drop stealth (the user's requirement), a cargo feature swapping
Signal for stealth (§10: two binaries by another name), and the one taken:

- **Fork `btls-sys`** (`github.com/skymoore/btls`, `main` = 0.5.6 + one
  commit): `links = "btls"`; the existing `prefix-symbols` feature (rename
  every export to `btls_sys_*` with `nm` + `objcopy --redefine-syms`, bindgen
  `link_name` overrides to match) now runs on macOS/iOS via `llvm-nm` /
  `llvm-objcopy` (Mach-O carries a leading `_`, and bindgen's override is
  emitted pre-mangled, so the callback prefixes `_btls_sys_` there). Upstream
  skipped Apple targets with a warning; nothing in the repo read
  `DEP_BORINGSSL_*`.
- **Obscura fork commit 2**: `obscura-net` enables wreq's `prefix-symbols`
  on every platform (upstream: Linux/Android only, issue #39 — the very
  Apple gap the btls fork closes).

Proof: a scratch binary linking both `btls-sys` (prefixed) and Signal's
`boring-sys` (`signal-v5.0.2`, the rev theuth locks) built with no `links`
conflict and no duplicate symbols; `nm` shows `_btls_sys_SSL_CTX_new` and
`_SSL_CTX_new` once each; the stealth path fetched example.com and the
boring path created an `SSL_CTX`. Then the whole workspace built.

## 10. Rejected

- **`obscura mcp` as a configured MCP server** (zero code): one shared
  `BrowserState` for every session and subagent; unannotated tools all
  `Mutating` → `Ask` on every snapshot; screenshots JSON-stringified into
  context; a 100 MB binary the user installs by hand — and not one binary.
- **Spawning the obscura binary** (per-session process over MCP or CDP):
  fixes isolation and risk, still two binaries. 00 §1 wins.
- **Headless Chrome via CDP** (`chromiumoxide`): 200+ MB, 2 s start, needs
  Chrome installed. Attaching to the user's real logged-in Chrome (oh-my-pi's
  relay) is the one thing it offers that obscura cannot; nobody asked for
  it, and it is a separate design if they do.
- **The `obscura` facade crate**: too thin (§0.2); we would rewrite
  `obscura-mcp`.
- **Our own tool layer over `obscura_browser::Page`**: same rewrite, ~1.5k
  lines of JS-in-Rust we would maintain instead of upstream.
- **A stealth cargo feature on theuth**: two binaries by another name.
- **Gating clicks as `Mutating`**: an approval treadmill for actions with
  no persistent state behind them; revisit with persistent profiles (v1).
- **Separate user message for screenshots** instead of the genai patch: §5.
- **`ToolOutput.images: Vec<Binary>` (bytes)**: `ToolOutput` crosses the
  bus and RPC; paths keep it small and match `files` on inputs.

## 11. Tests and verification

Unit (`browser`): `tidy` (blank-line collapse, trailing whitespace, cut
marker), the name→obscura mapping and `browser_tabs` action dispatch,
`browser_wait` selector-xor-text validation, PNG dimension read from the
IHDR chunk. Unit (`agent`): `strip_images` on a Tool-role message; `est` of
a `ToolResponse` with one image is `≤ IMAGE_TOKENS + chars`. Unit (genai
fork): Anthropic `tool_result.content` array, OpenAI trailing user turn.

Integration (`browser/tests/live.rs`, one test, in-process, no network):
serve a fixture page from a `std::net::TcpListener` on loopback with
`allow_private_network` set for the test; `browser_open` → snapshot contains
the fixture text; `browser_fill` + `browser_click` on its form → the second
page shows the posted value; `browser_screenshot` → `files[0]` starts with
`\x89PNG` and the reported size matches the IHDR.

Smoke (headless, real model): "open news.ycombinator.com and tell me the
top story" → `browser_open` only, answer names the story; "start
`python3 -m http.server 8123` and screenshot the listing" with
`allow_private_network: true` → `start` (22) then `browser_open` +
`browser_screenshot`, the model describes the directory listing from the
image; `read_url rendered: true` on a JS-only page returns content without
Firecrawl.

## 12. Edits to other docs

- **00 §6.** After the PTY sentence: "Browser (23): sixteen `browser_*`
  tools drive an embedded obscura isolate per session; screenshots return
  as image parts of the tool result."
- **00 §10 deps.** Rows: `obscura-mcp | git v0.2.2 (fork, 23 §3.3) |
  embedded headless browser (stealth + render)`, `taffy / cosmic-text |
  patched to obscura's vendored copies`, `genai | fork of upstream main (patched, 23
  §5; revision in Cargo.lock)`. Build prerequisites paragraph: cmake, clang/libclang, first-build
  network.
- **00 §13 phases.** Row 14: "browser (23) | `browser_open` reads HN; a
  screenshot of a local dev server is described from the image; `read_url
  rendered` keyless".
- **16 §5.** `rendered: true` → "the first registered rendering provider;
  `browser` (23) registers one at start, so this works keyless; Firecrawl /
  OpenSERP remain as alternatives." Note the private-network deviation.
- **17 §0/§3.** Images also flow out of tools (23 §5): `ToolOutput.files`,
  `ToolResponse.images`, `strip_images` covers the Tool role.
- **README.** Tools list + `browser:` block + build prerequisites.
- **`harness.md`.** When to use `browser_*` vs `read_url` (JS, forms,
  visual check); refs go stale; click the submit button.

## 13. Status (as-built, 2026-09-14)

Shipped: `crates/browser` (`lib.rs` 282 lines: config, `Browsers` actor with
`DelegatedReply` so one slow page never blocks another session, sweeper,
`warmup`; `session.rs` 128; `tools.rs` 762: sixteen tools via a
`browser_tool!` macro over a shared `Ctx`, `tidy`, screenshot files;
`render.rs` 163: `ObscuraRender` behind a mutex on one fixed render isolate);
`tests/live.rs` (§11's fixture test, 0.24 s after warmup); the two obscura
fork commits, the genai fork commit, the btls fork commit (§9.1); agent
loop changes (§5 layer 3, with `binary_tag` shared by `text_of`, `snap.rs`
and the history reader); `tools::ToolOutput.files`; `theuth/src/main.rs`
warmup on the main thread (measured 11 ms). Docs per §12.

Deviations from the sections above (the code wins):

- `browser_select` maps `ref` to `[data-obscura-ref="eN"]` locally: obscura's
  `browser_select_option` takes only `selector` + `value`.
- Every pass-through argument object is built with `null` members stripped:
  obscura's `browser_screenshot` rejects unknown keys and nulls.
- `Browsers` also carries `Sweep`/`CloseAll` internal messages; `Call` runs
  off the mailbox (`tokio::spawn` + `DelegatedReply`), like `web`'s `Search`.
- Gemini also emits `file_data` for URL-sourced images rather than dropping
  them; the fork's merge of consecutive functionResponse turns accepts image
  parts so multi-tool turns still merge.
- `text_of` now renders a `ToolResponse`'s content even without images
  (previously skipped); only the store's undo/redo path reads Tool-role text.
- `warmup` ran on a libtest thread in `tests/live.rs` without incident,
  consistent with §2; main-thread placement in `theuth` is kept regardless.

Smoke (headless, `claude-sonnet-4-5`, `/tmp/theuth-title`,
`browser.allow_private_network: true`): "open news.ycombinator.com and give
me the top story" → exactly one `browser_open`, the reply named the top
story. "Serve /tmp with `python3 -m http.server`, open a fixture page,
screenshot it, answer from the image only: background colour and the colour
of the rounded rectangle" → `shell` (job), `browser_open`,
`browser_screenshot` (`screenshot 1280×720 attached`), `cancel_job`; reply
"dark navy blue … bright green" for `#1a1a2e` / `#2ecc71` — neither colour
appears in the page text, so the model read the PNG. The persisted tool
message carries `images: [{content_type: image/png, source: Base64 …}]`.
`read_url { rendered: true }` on example.com returned the markdown with no
Firecrawl configured. No orphan processes after `cancel_job`.

Gates: fmt, clippy `-D warnings` (two `result_large_err` hits fixed by
shrinking two helpers' error type to `&'static str`), workspace tests, debug
and release builds of `theuth` (release binary 227 MiB unstripped, up from the pre-browser build).

Not exercised: Linux build (the ELF prefixing path is upstream's, unchanged
in behaviour except the prefix string `btls_sys_` instead of
`build_script_main_`), Windows (btls skips prefixing there; the clash would
return), Gemini/OpenAI image paths live (fork unit tests only), the idle
sweeper and `Close` on a real session end, a proxy, `browser_tabs`,
`browser_scroll`, `browser_select`, `browser_type`, `browser_press` on a
real site (unit-tested mappings only), obscura's isolate panic → "browser
restarted" path.

2026-09-15: live view shipped — `Screenshot` query (§3.2), `browser_show` +
`BrowserEvent` (§4), `browser_screenshot` RPC, desktop globe + dialog polling
at 500 ms (25 §7). Icon semantics = "session has used a browser tool",
decided client-side from tool events/history; it lingers after
`browser_close`/the idle sweep, the view then reports no page. Ceiling:
history pagination — browser use only in unloaded older pages does not flag.
