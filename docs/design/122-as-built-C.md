# 122 as-built — C (contract wave)

Branch `feat/122-web-reading`, worktree `~/Develop/theuth-122-web`, base `c73421d5`. Seams only: every
frozen signature of 122 §7.2 compiles with a stub body; no behaviour from D1–D13.

## What was built

| file | lines | what |
|---|---|---|
| `crates/web/src/fetch/issue.rs` | new | `IssueKind` (snake_case), `Next` (snake_case), `PageIssue` (`retry_after` serialised as integer seconds via a private `duration_secs` serde module; schema `Option<u64>`), `detect → None`; one test pinning the seconds wire form |
| `crates/web/src/fetch/charset.rs` | new | `decode → (from_utf8_lossy, "utf-8")` |
| `crates/web/src/fetch/meta.rs` | new | `PageMeta` (derives `Default`), `extract → PageMeta::default()` |
| `crates/web/src/fetch/text.rs` | 160–191, tests 215–221 | `Cut`, `cut` (hard_truncate semantics from `offset`; see Decisions) |
| `crates/web/src/fetch/mod.rs` | 9–13, 24, 42–125, 506, 535 | `pub mod charset; pub mod issue; pub mod meta;`, `pub use text::{Cut, cut, …}`, `FetchResponse` fields + `impl Default`, both construction sites use `..FetchResponse::default()` |
| `crates/web/src/tools.rs` | 67–103, 556–557 | `Source { Auto, Page, Api }` (lowercase, `#[default] Auto`), `FetchArgs.offset/compact/source` with §4.1 doc comments; the rendered-path `FetchResponse` sets `tier: "render"` |
| `crates/web/src/render.rs` | 403 | test construction uses `..FetchResponse::default()` |
| `crates/web/src/lib.rs` | 12, 24, 82–85, 110–111 | `pub mod compact; pub mod sources;`, `WebConfig.accept_language` (`"en-US,en;q=0.9"`), `header_ladder` (`true`) |
| `crates/web/src/compact.rs` | new | `CompactOptions`, `Compacted`, `compact → identity, removed_chars 0` |
| `crates/web/src/sources.rs` | new | `ApiSource`, `api_source → None`, `render → None` |
| `crates/web/Cargo.toml`, `Cargo.toml:236`, `Cargo.lock` | | `encoding_rs = "0.8"` workspace dep (resolves to the already-present 0.8.41) + `encoding_rs = { workspace = true }` in web |
| `crates/firecrawl-native/src/extract.rs` | new | `Method` (lowercase, `Serialize`), `SelectOptions`, `Selection`, `select_main → { html: input, scoped: false, method: Body }` |
| `crates/firecrawl-native/src/noise.rs` | new | `may_strip → true`, `is_noise → false` |
| `crates/firecrawl-native/src/islands.rs` | new | `extract_islands → None` |
| `crates/firecrawl-native/src/lib.rs` | 15–26 | `pub mod extract; pub mod islands; pub mod noise;` + `pub use` of every §7.2 name |
| `crates/firecrawl-native/src/crawler.rs` | 486–500, test 833–844 | `compile_path_patterns` (complete) + `compile_path_patterns_reports_invalid_and_keeps_rest` |
| `crates/firecrawl-native/Cargo.toml` | dev-dependencies | `html-to-markdown-rs = { workspace = true }` for `tests/corpus.rs` (see Decisions) |
| `crates/firecrawl-native/tests/corpus.rs` | new | loads every `tests/fixtures/*.html`, runs `transform_html` (only_main_content) → html-to-markdown (engine options) → `post_process_markdown`, asserts `*.expected.md` (`UPDATE_GOLDEN=1` rewrites) and `expect.json` `must_contain`/`must_not_contain`/`min_words` |
| `crates/firecrawl-native/tests/fixtures/docs-page.{html,expected.md,expect.json}` | new | §6.1 row 1: header nav, sidebar nav, `<main>` with five `h2`, `language-bash` + `language-rust` fences, footer; golden generated with `UPDATE_GOLDEN=1` then re-run green |
| `crates/scrape/src/tools.rs` | 94–102, 105–210, 321–323 | `ScrapeUrlArgs.offset/compact/source`, `ScrapeResponse`, `Skipped`, `CrawlPage`, `CrawlResult`, `CrawlArgs.max_chars_per_page` |
| `crates/scrape/src/lib.rs` | 21 | `pub use tools::{CrawlPage, CrawlResult, ScrapeResponse, Skipped};` |
| `crates/scrape/src/crawl.rs` | 43–66, 599–603, 608–621 | `Page.issue/meta/charset/truncated/next_offset`, `MapHit.source`; `push_hit` sets `"page"`, `page_from` fills defaults |
| `crates/web/tests/request.rs` | 235 | reformat only (`cargo fmt -p web`) |

`scrape_url`/`crawl`/`map_site` still return what they did (`json!` values); the new types are
declared and re-exported, not yet filled (W3).

## Definitions as landed (verbatim)

`crates/web/src/fetch/mod.rs`
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchResponse {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    pub truncated: bool,
    pub took_ms: u64,
    #[serde(default)]
    pub issue: Option<issue::PageIssue>,
    #[serde(default)]
    pub meta: meta::PageMeta,
    #[serde(default = "default_attempts")]   // 1
    pub attempts: u8,
    #[serde(default)]
    pub ladder: bool,
    #[serde(default = "default_charset")]    // "utf-8"
    pub charset: String,
    #[serde(default)]
    pub compacted_chars: usize,
    #[serde(default)]
    pub next_offset: Option<usize>,
    #[serde(default)]
    pub omitted_headings: Vec<String>,
    #[serde(default = "default_tier")]       // "fetch"
    pub tier: String,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]                        // ClientKind::Reqwest (addendum)
    pub client: client::ClientKind,
}
impl Default for FetchResponse { /* the serde defaults above; everything else empty/false/0 */ }
```

`crates/web/src/fetch/issue.rs`
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind { AccessChallenge, RateLimited, LoginRequired, ConsentWall, ErrorPage, EmptyShell, Unsupported }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Next { Retry, Rendered, ImportCookies, Handoff }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageIssue {
    pub kind: IssueKind,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "duration_secs")]
    #[schemars(with = "Option<u64>")]
    pub retry_after: Option<std::time::Duration>,
    pub next: Vec<Next>,
}
pub fn detect(status: u16, headers: &reqwest::header::HeaderMap, final_url: &url::Url, body: &str, prose_chars: usize) -> Option<PageIssue>
```

`crates/web/src/fetch/charset.rs`
```rust
pub fn decode<'a>(bytes: &'a [u8], content_type: Option<&str>) -> (Cow<'a, str>, &'static str)
```

`crates/web/src/fetch/text.rs` (re-exported as `web::fetch::{Cut, cut}`)
```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cut { pub text: String, pub truncated: bool, pub next_offset: Option<usize>, pub omitted_headings: Vec<String> }
pub fn cut(s: &str, offset: usize, max_chars: usize) -> Cut
```

`crates/web/src/tools.rs`
```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Source { #[default] Auto, Page, Api }
pub struct FetchArgs { pub url: String, pub max_chars: Option<u32>, pub format: Option<FetchFormat>, pub rendered: Option<bool>,
    pub offset: Option<u32>, pub compact: Option<bool>, pub source: Option<Source> }   // new three: #[serde(default)]
```

`crates/scrape/src/tools.rs` (re-exported from `scrape`)
```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScrapeResponse {
    pub url: String,
    pub title: Option<String>,
    pub tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub links: Option<Vec<String>>,
    #[serde(default)] pub issue: Option<web::fetch::issue::PageIssue>,
    #[serde(default)] pub meta: web::fetch::meta::PageMeta,
    #[serde(default = "default_charset")] pub charset: String,   // "utf-8"
    #[serde(default)] pub compacted_chars: usize,
    #[serde(default)] pub truncated: bool,
    #[serde(default)] pub next_offset: Option<usize>,
    #[serde(default)] pub omitted_headings: Vec<String>,
    #[serde(default)] pub source_kind: Option<String>,
    #[serde(default)] pub notes: Vec<String>,
    #[serde(default)] pub client: web::fetch::client::ClientKind,   // addendum
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Skipped { pub url: String, pub error: String }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CrawlPage {
    pub url: String,
    pub title: Option<String>,
    pub markdown: String,
    pub tier: String,
    #[serde(default)] pub issue: Option<web::fetch::issue::PageIssue>,
    #[serde(default)] pub meta: web::fetch::meta::PageMeta,
    #[serde(default)] pub truncated: bool,
    #[serde(default)] pub next_offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub html: Option<String>,
    #[serde(default)] pub client: web::fetch::client::ClientKind,   // addendum
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CrawlResult {
    pub pages: Vec<CrawlPage>,
    pub incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")] pub reason: Option<&'static str>,
    #[serde(default)] pub skipped: Vec<Skipped>,
    #[serde(default)] pub invalid_patterns: Vec<String>,
}
// ScrapeUrlArgs: offset: Option<u32>, compact: Option<bool>, source: Option<web::tools::Source>  (#[serde(default)])
// CrawlArgs:     max_chars_per_page: Option<u32>                                                   (#[serde(default)])
```

`crates/scrape/src/crawl.rs`
```rust
#[derive(Clone, Debug)]
pub struct MapHit { pub url: String, pub title: Option<String>, pub source: &'static str }
#[derive(Clone, Debug)]
pub struct Page { pub url: String, pub title: Option<String>, pub markdown: String, pub html: String, pub links: Vec<String>,
    pub tier: &'static str, pub issue: Option<web::fetch::issue::PageIssue>, pub meta: web::fetch::meta::PageMeta,
    pub charset: String, pub truncated: bool, pub next_offset: Option<usize> }
```

`crates/firecrawl-native` — `Method`/`SelectOptions`/`Selection`/`select_main`, `may_strip`/`is_noise`,
`extract_islands`, `compile_path_patterns(&[String]) -> (Vec<regex::bytes::Regex>, Vec<String>)` exactly
as §7.2; all reachable at the crate root.

## Decisions taken beyond the design (deviations from §7.2)

1. **`charset::decode` names its lifetime**: `decode<'a>(bytes: &'a [u8], content_type: Option<&str>) -> (Cow<'a, str>, &'static str)`.
   The elided form in §7.2 is a compile error (E0106: two borrowed arguments, no rule picks one). Every
   call site reads identically; the `Cow` borrows `bytes`, which is what D3 needs.
2. **`cut` stub = `hard_truncate` from `offset`** (brief over §7.1's `next_offset: None`): window is exactly
   `max_chars` with `…` as its last char when cut; `next_offset = Some(offset + max_chars − 1)` (the first
   char not shown, so chained windows have no gap); `offset ≥ len` → empty text, not truncated (the
   `[offset M is past the end]` text is D6/W1). No marker, no headings. Not yet called by `read_body`
   (which still calls `hard_truncate`) — W1 wires it.
3. **`CrawlResult` derives `Serialize` only** (plus `Clone, Debug, Default, PartialEq, Eq, JsonSchema`): its
   `reason: Option<&'static str>` cannot derive `Deserialize`. Matches the design's own derive list (D10).
4. **`html-to-markdown-rs` as a dev-dependency of `firecrawl-native`** (already a workspace dependency; no new
   crate in `Cargo.lock`). The crate exposes no markdown converter, and `corpus.rs` must produce
   `*.expected.md`; the test copies the engine's `ConversionOptions` (`engine.rs:392-420`) verbatim as D13
   says W2 should.
5. **`scrape` re-exports** `CrawlPage, CrawlResult, ScrapeResponse, Skipped` from `lib.rs` so the private
   `tools` module's new types are reachable (no dead-code warnings without a test that constructs them).
6. **`FetchResponse` on the `rendered: true` path says `tier: "render"`** (one word in `tools.rs:556`); every
   other new field is the default on every path.
7. **`MapHit.source` is `"page"` for every hit** `push_hit` creates (search, sitemap and link hits alike) until W3
   distinguishes them — the design's stated default.

## Tests

- `web`: `fetch::issue::tests::retry_after_serialises_as_seconds`; `fetch::text::tests::title_scan_and_truncate`
  (extended with `cut` cases). Existing config tests unchanged and green.
- `firecrawl-native`: `crawler::tests::compile_path_patterns_reports_invalid_and_keeps_rest`;
  `tests/corpus.rs::corpus_goldens_and_expectations` (docs-page golden + 10 `must_contain`, 4 `must_not_contain`,
  `min_words: 350`).
- `scrape`: no new tests (types only).

## Gate

```
$ cargo fmt -p web -p scrape -p firecrawl-native
(exit 0)
$ cargo test -p web -p scrape -p firecrawl-native --no-fail-fast
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (firecrawl-native lib)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s    (corpus)
test result: ok. 17 passed; …  test result: ok. 86 passed; …  (web lib)  … every binary ok, TEST_EXIT=0
$ cargo clippy -p web -p scrape -p firecrawl-native --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 02s   (exit 0)
$ cargo check --workspace --tests
error: couldn't read `crates/nodes/src/../../../bindings/methods.json`: No such file or directory (os error 2)
   --> crates/nodes/src/client.rs:123:34
error: could not compile `nodes` (lib test) due to 1 previous error
```

### Pre-existing

`nodes`'s lib test `include_str!`s `bindings/methods.json`, a `just ts` artefact not present in this
worktree (`~/Develop/theuth-122-web/bindings` does not exist). Unrelated to C. `cargo check --workspace
--tests --keep-going` reports no other error (see below).

```
$ cargo check --workspace --tests --keep-going
error: couldn't read `crates/nodes/src/../../../bindings/methods.json`: No such file or directory (os error 2)
error: could not compile `nodes` (lib test) due to 1 previous error
(every other crate, including `theuth`, `tui`, `rpc`, finishes `Checking`; no other error)
```

## Hand-offs

- **W1** — `FetchResponse` is constructed in exactly three places: `crates/web/src/fetch/mod.rs:500` (document
  path) and `:529` (HTML/text path) in `read_body`, and `crates/web/src/tools.rs:550` (the `rendered: true`
  branch of `FetchTool::call`); `render.rs:397` is a test. All use `..FetchResponse::default()`; set the real
  `issue`/`meta`/`attempts`/`ladder`/`charset`/`compacted_chars`/`next_offset`/`omitted_headings`/`tier`/
  `source_kind`/`notes` there. `read_body` still calls `hard_truncate` (`:499`, `:527`) and
  `String::from_utf8_lossy` (`:508`); `cut` and `charset::decode` are waiting beside them. `FetchArgs.offset/
  compact/source` are parsed but unread. `WebConfig.accept_language`/`header_ladder` are unread.
- **W3** — `ScrapeResponse`, `CrawlPage`, `CrawlResult`, `Skipped` are declared and re-exported; `ScrapeUrlTool`/
  `CrawlTool`/`MapTool` still build `serde_json::Value`s (`tools.rs:241-246`, `crawl` rows, `map` rows) and `clip`
  (`:467`) is still there. `Page`'s new fields are filled with defaults in `crawl.rs::page_from` (`:608`); `Fetched`
  (`engine.rs:252`) has no `issue`/`meta`/`charset` yet. `MapHit.source` is `"page"` everywhere (`crawl.rs:602`).
  `ScrapeUrlArgs.offset/compact/source`, `CrawlArgs.max_chars_per_page` are parsed but unread.
- **W2** — `select_main`/`may_strip`/`is_noise`/`extract_islands` are not called from `transform_html`; the hook
  after `html.rs:584` is yours. `corpus.rs` ignores `issue`/`tier_hint`/`method` in `expect.json`; extend it when the
  `Method` is observable. The `docs-page` golden is pre-W2 output (no code-language normalisation needed: the fixture
  uses `language-bash`/`language-rust`, which h2m already honours).
- **W4** — `compact.rs` and `sources.rs` are untouched by anyone else; `CompactOptions` derives `Default` so
  `compact(md, &CompactOptions::default())` is the call shape W1/W3 will use.
- **Closer** — `charset::decode`'s explicit lifetime (Decision 1) should be reflected in §7.2 when the design is
  updated; `nodes` pre-existing red above.

## Addendum: stealth client seam (design 122 revision 1)

Commit `631db50f` + the gate/as-built commit. Sky's call: page fetches should be stealth like the browser;
C lands the client seam and the dependency pins so W5 can fill `WreqChrome` without touching any
`Cargo.toml`.

### What was built

| file | what |
|---|---|
| `crates/web/src/fetch/client.rs` | new: `ClientKind`, `Impersonate`, `ClientConfig`, `ClientError`, `ClientResponse`, `HttpClient`, `build`; `ReqwestClient` (real: today's client construction) ; `reqwest_client(cfg)` shared builder; one unit test |
| `crates/web/src/fetch/mod.rs` | `pub mod client;` `Fetcher { http: Arc<dyn HttpClient>, client: reqwest::Client, … }` — `fetch`/`fetch_html` go through `http.get()`; `request` keeps the plain reqwest client (never stealth, §5). `read_capped(content_length, &mut BoxStream)` drains the seam's stream; `read_body(ClientResponse, …)`. `FetchResponse.client: ClientKind` (`#[serde(default)]` → `Reqwest`), set from `self.http.kind()` at both construction sites. `FetchError::from_reqwest` delegates to `ClientError::from_reqwest`; `impl From<ClientError> for FetchError` |
| `crates/web/src/lib.rs` | `WebConfig.impersonate: Impersonate` (default `Always`) after `header_ladder` |
| `crates/scrape/src/tools.rs` | `ScrapeResponse.client`, `CrawlPage.client` (`#[serde(default)]`) |
| `crates/web/Cargo.toml` | `futures`, `bytes` (workspace entries, already in the graph), `wreq`, `wreq-util` |
| `Cargo.toml` | `wreq = { version = "=6.0.0-rc.29", default-features = false, features = ["webpki-roots", "tokio-rt", "stream", "gzip", "brotli", "deflate", "zstd"] }`, `wreq-util = { version = "=3.0.0-rc.12", default-features = false, features = ["emulation", "emulation-compression"] }` — obscura-net's lists minus `socks`; `cookies` off |
| `Cargo.lock` | four new edges on `web` (`bytes`, `futures`, `wreq`, `wreq-util`); no new package |

```
$ cargo tree --workspace -i wreq --depth 1
wreq v6.0.0-rc.29
├── obscura-net v0.1.0 (https://github.com/skymoore/obscura?branch=main#d5aed2b4)
├── web v2.9.2 (/Users/Sky.Moore/Develop/theuth-122-web/crates/web)
└── wreq-util v3.0.0-rc.12
$ cargo tree -p web -i wreq-util --depth 1
wreq-util v3.0.0-rc.12
└── web v2.9.2 (/Users/Sky.Moore/Develop/theuth-122-web/crates/web)
```

### The seam as landed (verbatim)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind { Reqwest, WreqChrome }
impl Default for ClientKind { fn default() -> Self { Self::Reqwest } }   // added beside: the response structs derive Default

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Impersonate { Never, OnChallenge, #[default] Always }

#[derive(Debug, Clone)]
pub struct ClientConfig { pub user_agent: String, pub accept_language: String, pub connect_timeout: Duration, pub read_timeout: Duration, pub max_body: usize }

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("stealth client not built")] NotBuilt,
    #[error("{0}")] Transport(String),
    #[error("{0}")] Build(String),
}
impl ClientError {
    pub fn rejected(detail: &str) -> Self;          // Transport("rejected: <detail>") — a guard refusal
    pub fn as_rejected(&self) -> Option<&str>;      // the Fetcher maps it back to FetchError::Rejected
}

pub struct ClientResponse { pub status: u16, pub headers: HeaderMap, pub final_url: Url, pub body: BoxStream<'static, Result<Bytes, ClientError>> }

#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    fn kind(&self) -> ClientKind;
    /// One request, redirects NOT followed (the Fetcher's loop re-checks SSRF per hop), extra headers appended after the client's defaults.
    async fn get(&self, url: &Url, headers: &[(HeaderName, HeaderValue)]) -> Result<ClientResponse, ClientError>;
}

pub fn build(kind: ClientKind, cfg: &ClientConfig) -> Result<Arc<dyn HttpClient>, ClientError>;   // WreqChrome → Err(NotBuilt)
pub struct ReqwestClient { .. }  impl ReqwestClient { pub fn new(cfg: &ClientConfig) -> Result<Self, ClientError> }
```

`BoxStream` is `futures::stream::BoxStream` — the same type as `futures_core::stream::BoxStream` (the
`futures` facade re-exports it); `web` already needed no `futures-core` line of its own.

### Decisions

1. **`ClientError` has no `Rejected` variant** (frozen), yet a guard refusal from the resolver must stay
   `FetchError::Rejected`. Convention: `ClientError::rejected(detail)` = `Transport("rejected: <detail>")`,
   `as_rejected()` reads it back; `ReqwestClient::get` applies it by walking the reqwest error's source chain
   (what `FetchError::from_reqwest` did). **W5 MUST use `ClientError::rejected` for refusals from its resolver.**
2. **Two clients in `Fetcher`**: page GETs through `Arc<dyn HttpClient>`; `request()` (route replay / `http_request`,
   arbitrary method + body, the trait has `get` only) through a plain `reqwest::Client` from the same
   `reqwest_client(cfg)` builder. §5 says those never impersonate, so this is the right split, at the cost of two
   connection pools.
3. **`ClientConfig.read_timeout` is the whole-request timeout** (`.timeout()`), reproducing today's 8 s exactly;
   `accept_language` and `max_body` are carried, not yet used by the `Reqwest` kind (W1 sends the header; the
   fetcher caps bodies itself). `Fetcher::new_with_timeouts` passes `accept_language: String::new()` until W1
   threads `WebConfig.accept_language` in.
4. **`ClientKind: Default`** added as a manual impl beside the frozen derive list (`ScrapeResponse`/`CrawlPage` derive
   `Default`).
5. **`FetchResponse.client` is set from `http.kind()`** at both construction sites, so once W5's client is
   selected the field is already truthful.

### Hand-offs (addendum)

- **W5** — fill `ClientKind::WreqChrome` in `client.rs::build` (an `HttpClient` over `wreq` + `wreq_util`
  emulation): redirects off, a resolver that applies `ssrf::resolve_checked` (`ssrf.rs:85`, `pub(super)` — widen
  as needed inside `fetch`), `connect_timeout`/`read_timeout`, streamed body, no cookie store;
  `ClientError::rejected` for refusals. `Fetcher::new_with_timeouts` (`fetch/mod.rs:229-245`) is where the kind is
  chosen — today hard-wired `ClientKind::Reqwest`; `WebConfig.impersonate` is unread.
- **W1** — `Fetcher::new*` do not yet take `accept_language`/`impersonate`; `HttpClient::get`'s `headers`
  argument is where the D4 ladder headers go (the Fetcher passes `&[]` today, `fetch/mod.rs:298`).
- **Closer** — `WebConfig.impersonate` default `Always` means the wreq path is on by default once W5 lands;
  document in 16 §6 with `accept_language`/`header_ladder`.

### Gate (addendum)

```
$ cargo fmt -p web -p scrape -p firecrawl-native                       (exit 0)
$ cargo test -p web -p scrape -p firecrawl-native --no-fail-fast
test result: ok. 23 passed … (firecrawl-native lib)   test result: ok. 1 passed … (corpus)
test result: ok. 87 passed … (web lib, +1: client::tests::rejected_round_trips_and_wreq_is_not_built)
test result: ok. 17 / 22 / 7 / 7 / 10 / 7 passed … (web integration: fetch_ssrf, request, http_request, … all green)
TEST_EXIT=0
$ cargo clippy -p web -p scrape -p firecrawl-native --tests -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 57s   (exit 0)
$ cargo check --workspace --tests --keep-going
error: couldn't read `crates/nodes/src/../../../bindings/methods.json`: No such file or directory (os error 2)
error: could not compile `nodes` (lib test) due to 1 previous error
(pre-existing, see above; every other crate finishes `Checking`)
```

## Open items

None.
