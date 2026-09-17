# 44 — Routes: learn a site's API in the browser, call it without one

Status: as-built v0 · 2026-09-17 (draft v0 same day) · extends `23-browser.md` §3.3 (the fork) and
§4 (tool surface), `16-web.md` §5 (fetcher), `38-scrape-vendored-crawler.md`
§0.3 (two tiers), `28-untrusted-content.md` §3 (labels), `00-architecture.md`
§4 (tools), §6 (risk), §10 (crates). Measured against
`~/Develop/unbrowse` (514 source files, 144,119 lines, today), obscura at
`skymoore/obscura` `main` (`~/.cargo/git/checkouts/obscura-9d787776d7b5fca7/002c5c6`)
and this tree today. §12 is the as-built status and what review changed.

## 0. Decisions in one screen

1. **Distil the idea, not the product.** Unbrowse's claim is one sentence
   (`~/Develop/unbrowse/README.md:7`): *if a site already exposes a
   first-party route behind its UI, an agent should reuse that route instead
   of rediscovering it through a browser on every call.* The mechanism is
   three steps — load the page in a real JS engine and watch its XHR/fetch
   traffic; keep the responses that are collections of records; store them as
   URL templates and replay them over plain HTTP. Everything else in that
   tree — wallet, x402, marketplace, publish sanitiser, receipts, telemetry,
   covenants, the closed `/v1/reveng` backend — serves a hosted route
   market we do not want. Alternative beaten: port the client (`src/` is
   144k lines against a backend we cannot run). We write ≈1,300 lines.
2. **Capture is the browser we already have.** The agent browses with the
   `browser_*` tools it already knows (23 §4); the isolate records every
   request/response it makes; `site_learn` reads that record. No sidecar
   process, no CDP, no second engine: unbrowse's own capture engine is a
   250-line Rust binary on `obscura::Page::on_response`
   (`~/Develop/unbrowse/native/obscura-capture/src/main.rs:151-186`), and
   `crates/browser` links the same engine in-process (`Cargo.toml:153`).
   Alternative beaten: unbrowse's `browser_network_requests` route —
   obscura-mcp's tool prints `[status] METHOD url (NB)` lines only
   (`obscura-mcp/src/lib.rs:1188-1201`), the underlying `NetworkEvent`
   records **no request headers** (`obscura-browser/src/page.rs:4264,3968`)
   and its body buffer silently drops anything over 2 MiB (`page.rs:4275,
   8021-8025`). `on_response` gives `RequestInfo{url, method, headers,
   resource_type}` and `Response{status, headers, body}`
   (`obscura-net/src/client.rs:82-88,120-125`) — headers are how a route's
   auth is learned.
3. **One more fork commit, one more obscura-mcp tool.** `BrowserState.tabs`
   and `page_mut()` are private (`obscura-mcp/src/lib.rs:63-66,99`), so
   `crates/browser` cannot reach a `Page`. The fork (23 §3.3: `v0.2.2` + one
   commit) gains a second commit: attach `on_response` on every page the
   state creates, buffer exchanges bounded by entries and bytes, and expose
   them as tool `browser_network_capture` (§3.1). `crates/browser` needs
   **no change**: `Handle::call` already runs any obscura-mcp tool
   (`crates/browser/src/session.rs:38-49`). Alternative beaten: a
   `Cmd::Capture` in `session.rs` owning a `Page` — duplicates page lifecycle
   that obscura-mcp already owns (tabs, navigation, suspend).
4. **A route is a data endpoint iff its response is a collection of
   like-shaped records.** Unbrowse's local inference rule, verbatim in spirit
   (`~/Develop/unbrowse/src/capture/reveng-local.ts:12-30`): the body decodes
   (after an XSSI prefix strip) to JSON containing an array of ≥ 2 objects
   sharing keys. No host list, no `/api/` token match, no content-type trust
   beyond "textual". HTML, CSS, fonts, sourcemaps (arrays of strings),
   beacons (204 / `{"ok":true}`) all fail the predicate by construction.
   Alternative beaten: their 64 KB intent matcher (`src/intent-match.ts`) —
   the caller is a model; hand it the ranked route table and let it pick.
5. **Structure persists; credentials do not.** Routes — method, URL
   template, holes, record keys, collection pointer, which header *names*
   were present — live in `<data_dir>/routes/<registrable-domain>.json`
   for ever. Header **values** captured alongside (`Cookie`,
   `Authorization`, CSRF tokens, anything not in a fixed allow-list of
   content-negotiation headers) are held in the actor's memory keyed by
   `(session, domain)`, used for replay, and dropped when the session's
   browser closes or the isolate is swept (23 §3.2). This is 23 §7's rule —
   *nothing the model logs into survives the session* — applied to the
   route layer. The model never sees a value: outputs carry names only, and
   replay errors carry status + body excerpt, never the request. Alternative
   beaten: persist them as `{{secret:route.<domain>.<header>}}` in the vault
   (37) — correct and the named upgrade path, but it turns a read-only tool
   into one that writes credentials to disk; not before someone needs it.
   Alternative beaten: replay inside the isolate with `browser_eval` +
   `fetch()` — cookies ride for free but bearer tokens do not, and the call
   loses `web`'s SSRF guard.
6. **Replay is `web::Fetcher`, extended by one method.** `Fetcher` is GET-only
   (`crates/web/src/fetch/mod.rs:184-189`); it gains `request(method, url,
   headers, body)` with the same per-hop `ssrf::check_static` and
   `GuardedResolver` (`:123,182`). Safe methods only (RFC 9110 §9.2.1), plus
   POST when the captured body is a GraphQL `query` operation — that is what
   makes `site_call` an honest `Risk::ReadOnly`. Alternative beaten: obscura's
   stealth client for replay — its TLS fingerprint helps against some walls,
   but the isolate is `!Send` on its own thread and a replay is one HTTP call;
   when a route only answers to the fingerprint, the fallback is the browser
   tier that already has it.
7. **Per-domain JSON, not sqlite.** Tens of routes per domain, always read
   whole, human-inspectable, deletable to forget a site. Matches
   `nodes.yaml` / `devices.yaml` / `mcp/<name>.json` (plugins.md §1);
   `knowledge`, `artifact`, `schedule` use `sqlx` for append-only stores in
   the thousands, which this is not.
8. **Three runtime functions, `Risk::ReadOnly`, `DIRECT: false`** (08 §0.4,
   38 §0.8): `site_learn`, `site_routes`, `site_call`. Output lands in a
   variable (`rows = await site_call(...)`). Labels per 28 §3: all three
   `UNTRUSTED` (templates, keys and rows are the web's strings); `site_learn`
   and `site_call` `SINK` (a navigation or a parameter carries bytes out).

## 1. Why

wth reads the web three ways today: `read_url` (16), `browser_*` (23),
`scrape_url`/`crawl` (38). All three hand the model a **page** — markdown or
a snapshot — and the model extracts structure from prose. For a
client-rendered site the page is a shell; obscura renders it, at ~20 s for a
heavy SPA (38 §11), and the answer the model wanted was already on the wire
as JSON, in one XHR, a few hundred milliseconds in.

Unbrowse measured this across 94 domains: a 3.6× mean / 5.4× median speedup
when a learned route replaces a browser run, with lower token use because
the model receives records instead of a page dump
(`~/Develop/unbrowse/README.md:11`). Its own honest ceiling is 12/24 on an
adversarial anti-bot corpus (`:15`); the misses are Cloudflare/DataDome
challenge gates we also do not solve (§8).

What we want is narrower than the product: an agent that has opened
`crates.io/crates/tokio` once should be able to call
`GET /api/v1/crates/{crate}` next time — for any crate — without a browser,
and be told when the route has drifted.

## 2. Facts relied on

- **`Page::on_response(cb) -> u64` / `off_response(id)`**
  (`obscura-browser/src/page.rs:4489,4501`; re-exported `obscura/src/page.rs:148,162`).
  Fires for navigation and JS `fetch()`/XHR
  (`obscura-js/src/ops.rs:2519-2523`), page-scoped, dies with the page
  (`page.rs:328-331`). `ResponseCallback = Arc<dyn Fn(&RequestInfo, &Response)>`.
  `Response::text()` decodes by content-type/BOM (`obscura-net/src/client.rs:96`).
  Documented as "the main path for capturing the JSON an SPA loads
  asynchronously" (`docs/Use-as-a-Rust-library.md:72`).
- **Request bodies are not on this path** — unbrowse's note
  (`src/capture/obscura-capture.ts:16-21`); `enable_interception()` would add
  them. So a POST is replayable only when everything it needs is in the URL
  — GraphQL with `?operationName=` or a persisted-query `extensions=` hash —
  and any other POST is stored `replay: false`: shown, not callable (§3.3.6).
- **Bounded buffers exist as env knobs**: `OBSCURA_NETWORK_BODY_BUFFER_ENTRIES`
  (128) / `_BYTES` (2 MiB) (`page.rs:8014-8025`); `browser::warmup` already
  exports `OBSCURA_*` from config on the main thread (`crates/browser/src/lib.rs:85-121`).
  The capture buffer reuses the same two names for its own limits.
- **`browser_storage_state` / `browser_set_storage_state`** exist
  (`obscura-mcp/src/lib.rs:681,686`); `browser_import_cookies` is the latter
  fed by `unjar`, gated by `browser.import_cookies` (`crates/browser/src/tools.rs:788-794,875-878`).
- **`web::Fetcher`**: `redirect::Policy::none()`, guarded resolver, manual
  redirect loop with `check_static` per hop (`fetch/mod.rs:118-124,179-215`);
  `client.get(...)` is the only verb (`:184-189`).
- **`psl::domain_str`** already in the build (`crates/firecrawl-native/src/crawler.rs:242`)
  for the registrable domain; `web::url::canonical_key` (`crates/web/src/url.rs:88`)
  for URL normalisation.
- **Tool contract**: `Risk::{ReadOnly, Mutating, Destructive}`
  (`crates/tools/src/lib.rs:62-69`), `DIRECT` (`:173`), `UNTRUSTED`/`SINK`
  (28 §3), `secrets::redact_output` on every result (37 §3.4) — which only
  knows vault values, hence §0.5's "never echo the request".
- **Plugin pattern** to copy: `scrape`'s `start` — read section, build a
  `Fetcher` from `WebConfig::default()` constants, `lookup_when::<ToolRegistry>`
  then register (`crates/scrape/src/lib.rs:75-101`); browser actor looked up
  at call time, never at start (`crates/scrape/src/engine.rs` `browser_actor`,
  38 §0.3).

## 3. Shape

```
crates/routes/
  Cargo.toml          routes → {runtime, tools, web, browser, kameo, tokio, serde*, schemars, psl, url, reqwest, anyhow, tracing}
  src/lib.rs          RoutesConfig, Routes actor (ROUTES_ACTOR), messages, Plugin impl          ≈150
  src/capture.rs      Exchange (the fork's JSON row) → Vec<Exchange> via browser::Call            ≈80
  src/classify.rs     Exchange → Option<Route>: predicate, XSSI strip, templating, ranking        ≈400
  src/store.rs        <data_dir>/routes/<domain>.json load/merge/save; Route, Hole, Schema        ≈150
  src/replay.rs       Route + params + session creds → Fetcher::request → rows | Drift            ≈200
  src/tools.rs        site_learn / site_routes / site_call, PROMPT, register()                    ≈300
```

`browser` already depends on `web`; `routes → browser` is the same direction
`scrape` takes (38 §0.2). Nothing in `agent` changes.

### 3.1 The fork commit (obscura-mcp)

```rust
// obscura-mcp/src/lib.rs — added to BrowserState
captured: Rc<RefCell<VecDeque<Exchange>>>,           // shared with every page's callback
// on every Page creation (page_mut's lazy tab, tool_tab_new): attach once
page.on_response(Arc::new(move |req, resp| captured.push(Exchange::from(req, resp, max_bytes))));
```

Tool `browser_network_capture { clear?: bool (true) }` → text content: a JSON
array of

```json
{ "url": "…", "method": "GET", "resource_type": "Fetch|XHR|Document|Script|…",
  "status": 200, "request_headers": {…}, "response_headers": {…},
  "content_type": "application/json", "body": "…" | null, "body_len": 12345,
  "truncated": false, "ts": 1758067200123 }
```

`body` is set only for textual content types (`text/*`, `application/json`,
`+json`, `application/xml`, `+xml`, `application/javascript`) and cut at
`OBSCURA_NETWORK_BODY_BUFFER_BYTES`; the deque drops oldest past
`_ENTRIES`. `clear` empties it after the read so `site_learn` twice in one
session does not re-learn the first page's traffic. Upstreamed as a PR like
the first commit; the fork stays the build source (00 §1.3). 23 §4's "not
exposed" list keeps `network_requests`; this is a different tool with bodies.

### 3.2 Capture (`capture.rs`)

`Routes` asks `browser::Call { session, name: "browser_network_capture",
args: {} }` on `BROWSER_ACTOR`, parses the text block, and returns
`Vec<Exchange>`. With `url` given, `site_learn` first runs `browser_navigate
{url, waitUntil: "networkidle0"}` through the same actor and then
`browser_wait_for`-less `settle` by sleeping `routes.settle_ms` — obscura's
loop is pumped between commands (`session.rs:100-118`), so timers and late
fetches land before the read. No page interaction is scripted here: clicking
pagination, opening filters, scrolling for infinite lists is what
`browser_click` / `browser_scroll` already do, and the model drives them
when a first `site_learn` shows fewer routes than the page evidently has
(the prompt block says so, §6).

### 3.3 Classify (`classify.rs`)

Pure functions, unit-tested on fixtures; no I/O.

1. **Admit**: `status` 2xx, `body` present, `resource_type ∈ {Fetch, XHR,
   Document}` (a `Document` is admitted for the SSR-embedded case below).
2. **Decode**: strip XSSI prefixes (`)]}'`, `for(;;);`, `while(1);`); parse
   JSON; for `text/html` bodies parse each `<script type="application/json">`
   / `application/ld+json` block instead (Next/Nuxt data islands — the one
   structural way SSR pages ship records).
3. **Predicate**: bounded tree walk (depth ≤ 8, nodes ≤ 50k) for the first
   array of ≥ 2 objects whose key sets overlap ≥ 50 % pairwise; record its
   JSON pointer (`/data/items`), the union of keys with JSON types, and the
   count. No collection → not a route.
4. **Template the URL**: split the path; a segment becomes `{id}` when it is
   all digits, a UUID, ≥ 16 chars of `[A-Za-z0-9_-]`, or — after merging with
   the store — differs between two exchanges that share every other segment.
   Query keys become holes with the observed value as `example`. Tracking
   params (`web::url`'s list) are dropped. Fragment dropped.
5. **Auth**: request header names outside the fixed allow-list
   (`accept`, `accept-language`, `content-type`, `user-agent`, `referer`,
   `origin`, `x-requested-with`, `sec-*`) are recorded as `auth_headers:
   [names]`; their values go to the session credential map (§3.5), never to
   the file. A `cookie` header or a jar hit for the domain sets `cookies: true`.
6. **Replayable**: safe method → yes. POST → yes only when the URL carries
   `operationName=` or a persisted-query `extensions=` (GraphQL GET-in-POST
   shape) — the body cannot be captured (§2), so otherwise `replay: false`.
7. **Rank**: `body_len / (url.len() + Σ header lens)` (information gain —
   a chatty beacon that echoes records still sorts under the payload), then
   record count. GraphQL operation name (from the URL) is the route's `name`;
   otherwise the last non-hole path segment.

Same rule, no site tables: unbrowse's worked examples
(`reveng-local.ts:21-29`: `data.europa.eu/api/hub/search/search` admitted for
its 80 like-shaped records; `careers.un.org/ds/filter-search-proto/` admitted
with no `/api/` anywhere; `main.js.map` rejected despite `/api/` in the path)
are the fixtures.

### 3.4 Store (`store.rs`)

`<data_dir>/routes/<registrable-domain>.json`, written whole on every merge
(`tokio::fs::write` to `.tmp` then rename):

```json
{ "domain": "crates.io", "learned_at": "2026-09-17T18:02:11Z", "routes": [
  { "id": "crates.io:GET:/api/v1/crates/{id}", "name": "crates", "method": "GET",
    "template": "https://crates.io/api/v1/crates/{id}",
    "holes": [ { "name": "id", "in": "path", "example": "tokio" } ],
    "collection": "/versions", "keys": { "num": "string", "yanked": "bool", "downloads": "number", "…": "…" },
    "records_seen": 214, "auth_headers": [], "cookies": false, "replay": true,
    "seen": 3, "last_ok": "2026-09-17T18:02:11Z", "stale": false, "example_url": "https://crates.io/api/v1/crates/tokio" } ] }
```

Merge key is the route `id` — `{domain}:{METHOD}:{path}`, one row per
path shape (§12.9). Query holes are the **union** across captures: a key
present in every capture keeps its `example` as the default (`?include=
versions`), a key seen in only some is `optional: true` and is sent only
when passed. A re-learn bumps `seen`, refreshes keys and `example_url`,
unions holes, clears `stale`. Delete the file to forget the site. The
directory is rebuildable by browsing again; nothing else references it.

### 3.5 Replay (`replay.rs`)

`site_call { route, params }`:

1. Load the route; refuse `replay: false` with the reason.
2. Fill holes: every path hole must be given; required query holes default
   to `example` when omitted (a route learned with `?page=1` is callable as
   `site_call(route, {page: 2})`); optional ones are omitted unless passed.
3. Headers: content-negotiation headers from the capture (`accept`,
   `accept-language`, `x-requested-with`, `referer` = page origin), then the
   session credential map for `(session, domain)` — the captured values for
   `auth_headers` and `cookie`. No entry and `auth_headers` non-empty → the
   call still goes out (many "auth" headers are optional) but the result
   text notes `sent without <names>; open the site in this session and
   site_learn again to arm them`.
4. `Fetcher::request(method, url, headers, body: None)` with
   `routes.call_timeout_ms`; body cap `routes.max_body_bytes`.
5. **Drift check**: decode as in §3.3; the collection must be found at the
   stored pointer with ≥ 1 record and ≥ 50 % key overlap. Pass → return the
   records (structured `list[dict]`) plus `{status, count, pointer}`;
   miss → set `stale: true`, return `ToolOutput::err("route drifted (status
   403, collection not at /data/items); re-learn with site_learn")`. A 401/403
   with `auth_headers` non-empty and no credentials armed says so explicitly.

### 3.6 Credentials in the actor

```rust
struct Creds { headers: HashMap<String, Zeroizing<String>>, cookie: Option<Zeroizing<String>> }
creds: HashMap<(SessionId, String /*domain*/), Creds>
```

Filled by `site_learn` from the exchanges' request headers (the `cookie`
header as sent by the page, or `browser_storage_state` for the domain when
the exchange had none). Dropped on `browser`'s `Close` for the session and
on a periodic sweep mirroring the isolate's `idle_secs`; `rt.shutdown` drops
all. Never serialised; never in a `ToolOutput`; never in an event.
`Zeroizing` from `zeroize` (already a dep through `secrets`).

## 4. Tools

All three `Risk::ReadOnly`, `DIRECT: false`, `UNTRUSTED: true`; http(s) only.

- `site_learn { url?: str }` → `dict` `{domain, routes, captured, top_urls,
  hint}` — the whole reply, not the rows alone (§12.9). `routes` are the
  §3.4 rows for the page's domain. With `url`: navigate in this session's
  isolate, settle, capture, classify, merge. Without: capture what this
  session's page has done so far — after the model clicked a filter — for
  the session's last domain. `SINK: true`. `hint` names the empty case:
  *no traffic captured*; *HTML page + beacons only* (a full navigation or a
  server-rendered list — click a control that updates the list in place);
  *N requests, no record collections* (with `top_urls`); or *nothing new
  captured; N stored routes*.
- `site_routes { url: str }` → the stored rows for the URL's registrable
  domain, or `no routes learned for <domain>; site_learn first`. `SINK: false`.
- `site_call { route: str, params?: dict }` → `list[dict]` records; text
  summary `GET https://…/api/v1/crates/tokio → 200, 214 records at /versions
  (auth: none)`. `SINK: true`.

Prompt block (registered as the `routes` plugin's `RegisterPrompt`):

> `site_learn`, `site_routes`, `site_call` are runtime functions. When a
> page is an app (search, tables, feeds, dashboards), `site_learn(url=…)`
> once: it opens the page in your browser session and returns the JSON
> routes the page itself called, as URL templates with holes. Then
> `site_call(route=…, params={…})` returns records directly — no page, no
> rendering — for any hole value. Click pagination or filters with
> `browser_click` first and call `site_learn()` with no url to learn what
> those fired. Prefer this over `scrape_url {rendered: true}` when you need
> data, not prose; a route that says *drifted* needs `site_learn` again.

## 5. Config and disk

```yaml
routes:                     # design doc 44
  enabled: true             # false: no tools, no prompt block, no directory touched
  settle_ms: 4000           # after networkidle0, for late fetches
  call_timeout_ms: 15000    # one site_call
  max_body_bytes: 2097152   # replayed body cap

browser:                    # 23 §6, two rows added
  capture_entries: 256      # exchanges the isolate keeps per page  → OBSCURA_NETWORK_BODY_BUFFER_ENTRIES
  capture_body_bytes: 2097152  # per captured body                 → OBSCURA_NETWORK_BODY_BUFFER_BYTES
```

The capture limits live under `browser:`, not `routes:`: they are env
knobs obscura reads at isolate build, and only `browser::warmup` may set
env — main thread, before plugins (23 §3.1). `routes` never reads
`browser`'s section (plugins.md §3.6); its replay cap is its own key with
the same default.

Disk: `<data_dir>/routes/*.json` (§3.4). Nothing else.

## 6. Events and commands

No bus events in v0 — a `site_call` is one HTTP request; the `ScrapeProgress`
card (38 §6) is for multi-page runs. Commands on `ROUTES_ACTOR = "routes"`:

```rust
/// `ask` → `Vec<Route>`: capture this session's page traffic and merge.
pub struct Learn { pub session: SessionId, pub url: Option<Url> }
/// `ask` → `Vec<Route>` for the registrable domain.
pub struct List { pub domain: String }
/// `ask` → `Result<Rows, Drift>`.
pub struct Call { pub session: SessionId, pub route: String, pub params: Map<String, Value> }
/// `tell`: drop the session's credentials (from browser Close / sweep).
pub struct Forget { pub session: SessionId }
```

## 7. Security

In 28's letters: the model has B (a signed-in isolate when
`browser.import_cookies` is on) and C (egress via `site_call`'s URL).

- **SSRF**: replay goes through `web::Fetcher` — guarded resolver, per-hop
  static check, redirects capped (16 §10). A learned template whose host
  resolves private is rejected at call time like any `read_url`.
- **Credentials**: values never reach the file, the transcript, an event or
  a `ToolOutput` (§0.5, §3.6). The file holds header *names*. A prompt
  injection that reads `<data_dir>/routes/*.json` through `read_file` learns
  which sites the user browsed and which APIs need a cookie — the same
  information `knowledge` or the session store already leaks to that
  attacker; not new B.
- **Write safety**: `site_call` sends safe methods and GraphQL query POSTs
  only (§0.6). A captured `POST /cart/add` is stored `replay: false` and
  shown as such; there is no flag to enable it. Mutating replay is a later
  doc with `Risk::Mutating` and the approval card.
- **Taint**: all three tools `UNTRUSTED`; a session that called them is
  tainted for the gate (28 §2). `site_learn`/`site_call` `SINK`.
- **Politeness**: one request per `site_call`; no fan-out. robots.txt is not
  consulted for a route the page itself called — the site served it to this
  client seconds ago. A `crawl`-style loop over `site_call` is the model's
  Python and bounded by the cell.

## 8. What we knowingly give up

- **Shared routes.** Unbrowse's marketplace means the *first* visit to a
  popular site is already warm. Ours is cold once per domain per machine.
- **Request bodies** (§2): GraphQL-over-POST with the query in the body is
  visible but not replayable until `enable_interception()` is wired
  (one more fork commit; named as the upgrade in `classify.rs`).
- **Per-request signatures** (X, Instagram, anything HMAC-ing the URL) and
  short-lived bearer tokens: replay fails the drift check, the route goes
  `stale`, the browser tier still works. Unbrowse carries ~60 KB of
  `token-resolver`/`replay-tokens`/`*-challenge.ts` for this; not built.
- **JS-challenge walls** (Cloudflare, DataDome): unbrowse's rungs 3–4 are a
  real Firefox/Chrome (`src/capture/DESIGN_NOTES.md:33-38`); obscura's
  `stealth` is our only rung, same as 23/38.
- **Cross-session credentials**: `ponytail:` session-scoped map; upgrade
  path is the vault (37) as `route.<domain>.<header>` when a user asks for
  it. Route structure already persists.

## 9. Rejected

- **Unbrowse as an MCP server** (`npx unbrowse mcp`, zero Rust): a Node
  process, a backend account, telemetry egress unless `UNBROWSE_LOCAL_ONLY`,
  and the local inference path is explicitly the fallback to a server we
  do not run (`reveng-local.ts:1-10`). Against 00 §1 single binary.
- **Vendor `native/obscura-capture`** as 38 vendored firecrawl: it is a
  `main.rs` that spawns its own `Browser` — the isolate-per-session model
  (23 §3.2) already owns the browser; what we want is 40 lines of it inside
  obscura-mcp (§3.1), not a second engine instance per learn.
- **`browser_network_requests` + `get_response_body` join** (§0.2).
- **Intent matching / embeddings** (§0.4).
- **sqlite store** (§0.7).
- **Robots for replay** (§7).

## 10. Effort

Fork commit ≈120 lines (deque, callback attach at two page-creation sites,
one tool, its `tools()` row). `web::Fetcher::request` ≈60 lines. `routes`
≈1,300 lines including tests. The expensive half, as in 38, is not typing:
it is the classifier's fixtures against real captures (crates.io,
hn.algolia.com, a Next.js data-island page, a sourcemap, a beacon) and the
templating rule's false positives (a slug that looks like an id).

## 11. Checks

Per step, runnable:

1. **Fork**: `cargo test -p obscura-mcp` gains one test — navigate an echo
   server page whose script `fetch()`es `/api`, call
   `browser_network_capture`, assert the `/api` row has `request_headers`
   non-empty and `body` = the JSON; `clear` empties it.
2. **`Fetcher::request`**: wiremock — method honoured, headers sent,
   loopback rejected without `allowing`, redirect hop re-checked.
3. **`classify`**: fixtures — nested collection admitted with pointer and
   keys; XSSI-prefixed body admitted; `<script type="application/json">`
   island admitted from a `Document`; sourcemap, HTML shell, `{"ok":true}`,
   204 rejected; `/users/123` + `/users/456` → `/users/{id}`; tracking
   params dropped; `authorization` recorded as a name, never a value in the
   serialised `Route`.
4. **`store`**: merge bumps `seen`, clears `stale`, preserves unknown fields;
   tmp+rename.
5. **`replay`**: wiremock — hole fill, query default from `example`, drift
   → `stale: true` + error text; `replay: false` refused; credentials sent
   from the map and absent from the `ToolOutput` on a 500 that echoes them.
6. **Live smoke** (throwaway `cargo run --example`, deleted after):
   `site_learn https://crates.io/crates/tokio` → expect
   `GET /api/v1/crates/{id}` (`/versions`) and `GET /api/v1/crates/{id}/downloads`;
   `site_call(route, {id: "serde"})` → records without a browser, < 1 s.
   `site_learn https://hn.algolia.com/?q=rust` → `GET /api/v1/search?query=…`
   with `query` as a query hole; `site_call(route, {query: "tokio"})`.
   `site_learn https://news.ycombinator.com` → *no traffic captured* text
   (server-rendered), pointing at `scrape_url`.
7. **Headless RPC**: `list_tools` shows the three as `direct: false, risk:
   read_only`; `get_prompt` carries the block; `routes.enabled: false`
   removes all of it.

## 12. Status (as-built, 2026-09-17)

Shipped on branch `routes`: `crates/routes` (1,985 lines incl. tests:
`classify.rs` 863, `lib.rs` 473, `tools.rs` 273, `store.rs` 166,
`replay.rs` 121, `capture.rs` 89), `web::Fetcher::request` + `RawResponse`
(`crates/web/src/fetch/mod.rs`, byte-cap loop factored into `read_capped`
shared with `read_body`), `browser.capture_entries` / `capture_body_bytes`
(`crates/browser/src/lib.rs`), `RoutesPlugin` in both plugin vectors
(`crates/wth/src/main.rs`). Fork branch `skymoore/obscura` `network-capture`
= `002c5c6` + two commits (`95f87e0`, `accc198`; 186 lines): the tool, the
capture buffer, and a two-line `obscura-js/src/ops.rs` fix — `op_fetch`
fired `on_response` with the *response* headers in `RequestInfo.headers`,
so request-header (auth) capture was impossible before it. Root
`Cargo.toml` points all four obscura rows at that branch; flip to `main`
when it merges.

Verified by: `cargo clippy --workspace --all-targets -D warnings` clean;
`cargo test -p routes -p web -p browser -p scrape` green (routes 18 unit +
2 tokio: classifier fixtures of §11.3 plus the page-path rule below, store
merge/mark, `absorb` grouping with secrets kept off disk and out of
`LearnReply`, cred sweep; `web/tests/request.rs` 7 wiremock cases of §11.2);
`cargo test -p obscura-mcp` 12/12 incl.
`network_capture_reports_request_headers_and_bodies_then_drains`;
obscura-js 382/382 with the pre-existing `heap_limit` V8-flag abort skipped
(fails identically on `002c5c6`). Live smoke (throwaway example, deleted;
debug build, author's network):

- `learn https://crates.io/crates/tokio` → 22 exchanges captured, 6.9 s,
  three routes: `GET /api/v1/crates/{id}/downloads?include={include}`
  (367 records at `/version_downloads`), `GET /api/v1/crates/{id}?include=`
  (`/keywords`), `GET /api/v1/crates/{id}/owners` (`/users`). Then
  `call(…downloads, {id: "serde"})` → `https://crates.io/api/v1/crates/serde/downloads?include=versions`,
  200, **422 records in 0.66 s**, no browser. The first run learned
  `…/crates/tokio/…` with no hole and `{id: serde}` fell through as a query
  param — two fixes below (§12.3, §12.4).
- `learn https://news.ycombinator.com/` → 1 exchange, no routes, `top_urls`
  = the page: the *no XHR, use `scrape_url`* case as designed.
- `learn https://www.npmjs.com/package/react` → Cloudflare Turnstile
  (`/cdn-cgi/challenge-platform/…`), no routes — the §8 anti-bot class.
- `learn https://hn.algolia.com/?q=rust` → obscura fails the 2.6 MB bundle
  (`Dynamic script fetch error: HTTP 0`); `learn https://data.europa.eu/data/datasets`
  → `ReferenceError: XPathEvaluator is not defined` in the app bundle.
  Both engine gaps, not ours; §11.6's Algolia expectation is withdrawn.
- Not exercised: `site_*` from a real model turn (scratch data dir has no
  provider key); §11.7 headless `list_tools` (binary builds; not driven);
  an authenticated site (`browser.import_cookies` path → cookie header →
  replay); `replay: false` refusal and drift against a live site.

Revisions at build time:

1. **No kameo actor.** Nothing outside the crate asks `routes`; the §6
   messages are methods on `Arc<Routes>` held by the three tools, exactly
   `scrape`'s `Arc<Scrape>` shape (38). `Forget` is gone: credentials expire
   by a 30 s sweep at `CREDS_IDLE` = 300 s (mirrors `browser.idle_secs`).
2. **Capture buffers documents and fetch/XHR only** (`accc198`). The first
   crates.io run hit the 256-entry cap exactly: ~250 script/image/font
   exchanges evicted the `Document` navigation. Scripts, styles, images and
   fonts are never routes; they no longer enter the deque.
3. **Page-path resource/id pairs are holes.** One visit gives no sibling and
   `tokio` is not id-shaped, so the first template was `/crates/tokio/…`.
   Now a segment whose predecessor *and* itself appear in a captured
   `Document` path (`/crates/tokio` ↔ `/api/v1/crates/tokio/downloads`)
   becomes `{id}` (`classify.rs`, `page_path_resource_id_pair_becomes_hole`).
   `ponytail:` pairs only — `/users/alice/posts` also holes `posts`.
4. **Sibling merge has two guards**: the differing segment is never the
   first, and both replies must share ≥ 50 % record keys (`same_shape`, the
   one overlap rule, also used by `drift`). Without them `/graphql` and
   `/gql` merged into `/{id}`.
5. **Cookie header comes from the jar.** obscura never puts `cookie` in
   `RequestInfo.headers`; `learn` calls `browser_storage_state` per learned
   domain, joins matching cookies (domain equal or subdomain, leading dot
   stripped), arms `cookie` in the session map and marks the routes
   `cookies: true`. Domains with nothing to arm get no map entry.
6. **`browser_navigate` takes `waitUntil`**, not `wait` (obscura-mcp schema);
   §3.2's spelling was wrong and would have silently defaulted to `load`.
7. `web::url::is_tracking_param` made `pub` so `routes` reuses `web`'s list
   instead of a second one (§3.3.4).
8. Labels of 28 §3 are recorded in that doc's table; 28's `UNTRUSTED`/`SINK`
   consts are not implemented in `tools` yet, so nothing is set in code.
9. **Hugging Face field report** (2026-09-17, an agent using the tools
   against huggingface.co listings). What worked: filter/sort/search clicks
   fire `GET /models-json?…`, `/datasets-json?…`,
   `/api/spaces/semantic-search?…`; all learned and replayed with swapped
   hole values. What looked broken, and the fixes: (a) `site_learn`'s
   structured output was `routes` alone, so the runtime saw `[]` for three
   different empty cases — now the whole reply with a `hint` (§4). (b)
   `site_learn()` with no url and an empty capture set `domain = ""` —
   now the session's last domain, so a quiet page returns its stored rows.
   (c) Two `/models-json` query shapes shared one id; the store kept both
   and replay took the first, so `{search: bert}` silently kept
   `pipeline_tag=text-generation` — now one route per id with unioned
   query holes, `optional` ones omitted unless passed (§3.4, §3.5).
   Alternative beaten: id = path + hash of query names — two ids for what
   the model thinks of as one route, and `search` unreachable from the
   route it was handed. (d) "Files / Discussions captured zero": verified
   false — a click-navigation captures its `Document` plus beacons (HF
   gpt2 → Files: 7 exchanges); it was (a) hiding `captured`. The hint now
   says *HTML page + beacons only*. (e) Prompt rewritten to lead with the
   SSR first-load reality. Not changed: single-object responses (a model
   card's `/api/models/{id}`) still do not classify — §0.4's rule is
   record collections; `read_url` covers one object.

## 13. Edits to other docs

- **00 §10**: row `routes` — plugin, doc 44, "learn a site's JSON routes from
  the browser session; replay them over `web::Fetcher`".
- **00 §6**: after the browser sentence: "Routes (44): three `site_*`
  runtime functions learn a page's XHR routes from the isolate and call them
  without one; `ReadOnly`, safe methods only."
- **23 §3.3**: fork is `v0.2.2` + two commits; list `browser_network_capture`.
  **23 §4**: `network_requests` stays unexposed; `browser_network_capture`
  is consumed by `routes`, not registered as a `browser_*` tool. **23 §6**:
  `capture_body_bytes` / `capture_entries` under `browser:` (§5).
- **16 §5**: `Fetcher::request` beside `fetch`/`fetch_html`.
- **28 §3**: three rows — `site_learn` ReadOnly ✓ ✓; `site_routes` ReadOnly
  ✓ —; `site_call` ReadOnly ✓ ✓.
- **38 §0.3**: the two-tier pick gains a note: a domain with learned routes
  is the model's third option, chosen by the prompt block, not by the engine.
