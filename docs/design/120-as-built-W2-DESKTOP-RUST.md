# 120 as-built — W2 DESKTOP-RUST (`theuth-page` scheme, `page_put`/`page_drop`, `frame-src`, asset scope)

Branch `feat/120-desktop` off `fd193e7e`; worktree `~/Develop/theuth-120-desktop`. Design §4.2, §4.3, §9c,
§9d, §12.2. Commits: `9a803605` scheme + commands + LRU · `32253356` asset scope · `70e78fde` CSP ·
`84ea6efd` rustfmt drift · this note.

## What landed, where

| file | what |
|---|---|
| `crates/desktop/src/pages.rs` (new, 333 lines) | `SCHEME = "theuth-page"` (:18); `Pages(Mutex<Inner>)` managed state (:24-34): `by_token: HashMap<String, Arc<[u8]>>`, `by_call: HashMap<call_id, token>`, `order: VecDeque` (LRU, front = oldest), `bytes`; `MAX_ENTRIES = 64`, `MAX_BYTES = 128 MiB` (:19-20). `Pages::put` (:53-70): token = `format!("{:032x}", rand::random::<u128>())`, a re-put for the same `call_id` replaces its page, eviction loop pops the LRU front while count > 64 or bytes > 128 MiB (the page just put is never evicted, even if alone over budget). `Pages::drop_call` (:72-77). `Pages::get` refreshes recency (:80-88). `page_url` (:92-98) follows tauri's own host rule (`cfg!(any(windows, target_os = "android"))` → `http://theuth-page.localhost/<token>`, else `theuth-page://localhost/<token>`). Commands `page_put` (:101-104) / `page_drop` (:106-110), both `#[tauri::command(rename_all = "snake_case")]` with the frozen signatures. `handler` (:113-118) is the `register_uri_scheme_protocol` callback and delegates to the pure `respond(&Pages, &Request)` (:121-141): non-GET → 405; `uri().path()` minus the leading `/` is the token, nothing else is read; unknown → 404 `text/plain` empty body; known → 200 with exactly `Content-Type: text/html; charset=utf-8` · `Content-Security-Policy: sandbox allow-scripts allow-forms` · `X-Content-Type-Options: nosniff` · `Cache-Control: no-store` · `Referrer-Policy: no-referrer`. No `Access-Control-Allow-Origin` on any arm. |
| `crates/desktop/src/lib.rs` | `mod pages;` (:20, unconditional — both containers). Shared builder: `.register_uri_scheme_protocol(pages::SCHEME, pages::handler).manage(pages::Pages::default())` (:36-38) before `desktop::configure`/`mobile::configure`. `page_put`/`page_drop` in the mobile `invoke_handler` (:84-88) and the desktop one (:296-297). Asset scope (:330-340): five `allow_directory(data_dir.join(root), true)` for `attachments`, `browser`, `computer`, `artifacts`, `spill` replacing `allow_directory(data_dir, true)`. |
| `crates/desktop/Cargo.toml:27-28` | `rand = { workspace = true }` moved from the desktop-only target table to `[dependencies]` so iOS/Android compile the token source. `Cargo.lock` unchanged (`desktop` already resolved `rand 0.9.5`; lock entries carry no target cfg). |
| `crates/desktop/tauri.conf.json:48` | `frame-src 'self' theuth-page: http://theuth-page.localhost blob:`. `asset:`/`http://asset.localhost` left `frame-src`, stayed in `img-src`; nothing else in the string changed. `python3 -m json.tool` → valid. |
| `crates/desktop/capabilities/*.json` | Unchanged. Verified: `default.json`/`mobile.json` list core/plugin permissions only; `commands::connection`, `open_window`, `island_*`, `point_*` have no entry and are invoked today, so app-defined commands need none. |

Tests (`pages.rs:152-307`, 9): `known_token_gets_the_frozen_headers_and_body` (exact header list in order,
no ACAO, body bytes, Windows host form hits the same path), `unknown_token_is_404_text_plain_without_body`
(also the bare `/`), `other_methods_are_405` (POST/HEAD/PUT/OPTIONS), `query_and_fragment_are_ignored`
(`?x=1`, `#theuth-theme=…`, `?q#frag` all 200; `/<token>/extra` 404), `evicts_by_count_least_recently_used_first`,
`evicts_by_bytes_least_recently_used_first` (4 × 32 MiB fit; a 1-byte put evicts the LRU one, not the
one just read; a single 128 MiB + 1 page lands alone), `page_drop_evicts_that_call_only`,
`re_put_for_a_call_replaces_its_page`, `tokens_are_unique_random_and_not_the_call_id` (two puts for one
`call_id` differ, 32 hex chars, `call_id` not a substring).

## Gates (macOS host, `CARGO_TARGET_DIR=~/Develop/theuth-120-desktop-target`, `CARGO_INCREMENTAL=0`)

```
cargo check -p desktop --all-targets          Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 16s   EXIT 0
cargo test -p desktop --lib                   test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.51s   EXIT 0
cargo clippy -p desktop --all-targets -- -D warnings   Finished `dev` profile [unoptimized + debuginfo] target(s) in 26.05s   EXIT 0
cargo fmt -p desktop -- --check               EXIT 0
python3 -m json.tool crates/desktop/tauri.conf.json > /dev/null   OK
```

The only compiler output is the pre-existing `block v0.1.6` future-incompat notice (tauri-nspanel's dep).

## Deviations from the design / brief, with reasons

- **Asset-scope "missing root is ignored"** — implemented by registering all five roots unconditionally,
  not by skipping roots that do not exist yet. `Scope::allow_directory` (`tauri-2.11.5/src/scope/fs.rs:351-365`,
  `push_pattern` `:92-104`) only inserts glob patterns and never stats the path, so a root created after
  launch (`artifacts/` on the first `show` of a fresh data dir) is covered; an `is_dir()` skip would have
  left it unaddressable until restart. A root that never appears matches nothing. Same outcome as the
  brief's intent, no error either way.
- **Response body is copied once per GET** (`Arc<[u8]>` → `Cow::Owned(Vec)`): tauri's handler returns
  `Into<Cow<'static, [u8]>>`, which cannot borrow from the map. The lock is released before the copy.
  ≤ 25 MiB memcpy per document load; the async responder would not avoid it.
- **`HEAD` is 405** like every non-GET (brief: "any other method → 405"). WebKit does not issue HEAD for
  frame navigations.
- **`style(desktop): rustfmt ios_windowing.rs`** — a file outside my "You own" list, format-only. It was
  not rustfmt-clean at base, so `cargo fmt -p desktop -- --check` was red before any W2 change; fixing the
  three reflows was the only way to make the gate green. `lib.rs` also absorbed rustfmt's pending reflow
  (mod order, `on_event` match arms) in `9a803605`. No semantic change in either.

## Contract notes

None. `page_put(call_id: String, html: String) -> Result<String, String>` and
`page_drop(call_id: String) -> Result<(), String>` as frozen; both always `Ok` today (a poisoned mutex is
recovered with `PoisonError::into_inner` — the state is a plain cache). The returned URL is exactly
`theuth-page://localhost/<32 hex>` on macOS/iOS/Linux, `http://theuth-page.localhost/<32 hex>` on
Windows/Android.

## Open items

- Not exercised in a running webview (no UI on this branch, Tauri CLI never run by a gate): the handler
  is verified by the unit tests over `respond`; §13.3's exposure probe and §13.4's smoke belong to the closer.
- §15 unchanged: window-label scoping (`UriSchemeContext::webview_label` is available in `handler` and
  unused by decision), 25 MiB `invoke` on the phone, `sandbox` CSP support on WKWebView/WebKitGTK/WebView2.
- Page URLs are per mount and `no-store`: a reload of the same iframe after eviction is a 404 → the UI's
  `Unable to load` path (§3). With 64 entries the inline transcript would need > 64 simultaneously
  mounted pages (or > 128 MiB) before that happens.
