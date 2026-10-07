# 122 W5 — as-built: STEALTH (the `WreqChrome` arm of the fetch client)

Branch `feat/122-w5`, base `529eec18`. Design: `122-web-reading.md` Revision 1, R1-D1.
Contract: `122-as-built-C.md` "stealth client seam" addendum.

## What was built

| file | lines | what |
|---|---|---|
| `crates/web/src/fetch/client.rs` | 1-6 | module doc: `WreqChrome` is built |
| | 140-149 | `build`: `ClientKind::WreqChrome => Ok(Arc::new(WreqChromeClient::new(cfg)?))` (the `Reqwest` arm untouched) |
| | 212-249 | `WreqChromeClient` + `new(cfg)`: `wreq::Client` with `wreq_util::Emulation { profile: Chrome148, platform: MacOS }` (HTTP/2 and the profile headers on), `.timeout(read_timeout)` (whole request, as the `Reqwest` arm), `.connect_timeout`, `redirect::Policy::none()`, `.dns_resolver(GuardedWreqResolver)`, no cookie store (the `cookies` feature is off in the workspace pin; nothing to disable), `Accept-Language` from `cfg` applied through `default_headers` **after** `emulation(..)` so it replaces the profile's `en-US,en;q=0.9`; an empty `accept_language` keeps the profile's |
| | 251-267 | `GuardedWreqResolver`: `wreq::dns::Resolve` over the crate's `ssrf::GuardedResolver` (the `reqwest::dns::Resolve` impl), route A |
| | 269-281 | `ClientError::from_wreq`: walks the `wreq::Error` source chain for `FetchError::Rejected` → `ClientError::rejected`, else `Transport("HTTP request failed: …")` — same shape as `from_reqwest` |
| | 283-311 | `impl HttpClient for WreqChromeClient`: `kind()`; `get` mirrors `ReqwestClient::get` (extra headers appended after the client's defaults, `final_url` = request URL, `bytes_stream()` boxed, no pre-cap: the `Fetcher`'s `read_capped` cuts at `max_body` as for `Reqwest`) |
| | 317-343 | unit test renamed `rejected_round_trips_and_both_kinds_build`: `build(WreqChrome)` now asserted `Ok` with `kind() == WreqChrome` (it asserted `Err(NotBuilt)` before — the behaviour the brief changes) |
| `crates/web/tests/client_wreq.rs` | all | the seven integration tests below (wiremock on loopback, no network) |

**Emulation profile:** `wreq_util::Profile::Chrome148` with `wreq_util::Platform::MacOS` — the newest
Chrome entry in wreq-util 3.0.0-rc.12 (`src/emulate.rs:22-61`, `Chrome100`…`Chrome148`). UA sent:
`Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36`;
`sec-ch-ua: "Chromium";v="148", "Google Chrome";v="148", "Not/A)Brand";v="99"`, `sec-ch-ua-platform: "macOS"`,
`sec-ch-ua-mobile: ?0`, `sec-fetch-*`, Chrome's `Accept`, `Accept-Encoding: gzip, deflate, br, zstd`, `priority`.
TLS/HTTP-2 settings are `v132::build_emulation` (shared by Chrome 132–148 in wreq-util).

**Resolver route:** **A** — `wreq::ClientBuilder::dns_resolver` accepts any `wreq::dns::Resolve`
(`wreq/src/dns/resolve.rs:57`), whose `Name`/`Addrs`/`Resolving` types are shape-identical to
reqwest's. `GuardedWreqResolver` parses the name into `reqwest::dns::Name` and calls the crate's
`GuardedResolver` (`ssrf.rs:102`), so both kinds share one guard implementation; `ssrf::resolve_checked`
stays private (no edit to `ssrf.rs` was needed). IP literals bypass both resolvers (wreq
`SocketAddrs::try_parse`, as hyper-util in reqwest); the `Fetcher`'s `check_static` per hop covers them,
identically for both kinds.

## Decisions beyond the design

1. **`read_timeout` is the whole-request `timeout()`**, not wreq's per-read `read_timeout()` — mirrors
   the `Reqwest` arm (C decision 3) and the design's "`read_timeout` as the whole-request timeout".
2. **Empty `accept_language` keeps the profile's header** instead of sending an empty value: C's
   `Fetcher::new_with_timeouts` still passes `String::new()` until W1 threads `WebConfig.accept_language`.
3. **Private-IP test uses a blocked hostname (`localhost:<wiremock port>`)**, not an IP literal: an IP
   literal never reaches a resolver in either client (the `Fetcher`'s `check_static` refuses it before
   `get`), so the hostname is the only way to prove the guard runs *inside* the wreq client. wiremock's
   `.expect(0)` proves no connection was made.
4. **No proxy/cert-store overrides**: the `Reqwest` arm sets none either; same defaults.
5. **`ClientError::from_wreq` is private** (`from_reqwest` is `pub(super)` because `FetchError` delegates
   to it); nothing outside `client.rs` sees a `wreq::Error`.

## Tests

`crates/web/tests/client_wreq.rs`:
- `sends_chrome_profile_headers_and_configured_accept_language` — wiremock-received UA (Macintosh, `Chrome/148.`, ≠ `cfg.user_agent`), `sec-ch-ua`, `sec-ch-ua-platform "macOS"`, `sec-ch-ua-mobile ?0`, `sec-fetch-mode`, `accept-language` = configured, Chrome `accept`, the extra header passed to `get`, no `cookie`
- `empty_accept_language_keeps_the_profiles`
- `returns_302_without_following` — status 302, `Location`, `final_url` = request URL, `/b` `.expect(0)`
- `three_mib_body_streams_and_caps_like_the_fetcher` — 3 MiB drained whole under a 4 MiB cap; cut to `max_body` (1 MiB) with the flag using the `read_capped` loop shape
- `blocked_hostname_is_refused_by_the_guard_before_connecting` — `as_rejected()` is `Some`, names `localhost`, wiremock `.expect(0)`
- `read_timeout_fires_against_a_sleeping_mock` — 300 ms budget vs a 5 s delay; `Transport`, not a refusal
- `connect_timeout_fires` — 1 ns connect budget; `Transport`, wiremock `.expect(0)`

`crates/web/src/fetch/client.rs`: `client::tests::rejected_round_trips_and_both_kinds_build`.

### Gate

```
$ cargo fmt -p web
$ build-lock cargo test -p web --no-fail-fast
GATE_TEST_PLACEHOLDER
$ build-lock cargo clippy -p web --tests -- -D warnings
GATE_CLIPPY_PLACEHOLDER
$ cargo tree -p web -i wreq --depth 1
wreq v6.0.0-rc.29
├── web v2.9.2 (/Users/Sky.Moore/Develop/theuth-122-w5/crates/web)
└── wreq-util v3.0.0-rc.12
$ cargo tree -p web -i wreq-util --depth 1
wreq-util v3.0.0-rc.12
└── web v2.9.2 (/Users/Sky.Moore/Develop/theuth-122-w5/crates/web)
```

## Hand-offs

- **Closer / W1** — `Fetcher::new_with_timeouts` (`crates/web/src/fetch/mod.rs:232`) still hard-wires
  `client::build(ClientKind::Reqwest, …)`; `WebConfig.impersonate` is unread. `build(WreqChrome, &cfg)`
  is `Ok` now, so W1's `NotBuilt` fallback note never fires once wired (R1-D3 closer row).
- **W1** — `ClientConfig.accept_language` is sent by this arm; `Fetcher::new_with_timeouts` passes
  `String::new()` (`fetch/mod.rs:228`), which here means "the profile's `en-US,en;q=0.9`". Thread
  `WebConfig.accept_language` through and it replaces it.
- **Closer** — `crates/scrape` builds its own `ClientConfig` (8 MiB, `page_timeout_ms`); same `build` call.
- No sibling stub is called from `client.rs` or `client_wreq.rs` (only `ssrf::GuardedResolver`, C's code).

## Pre-existing

PREEXISTING_PLACEHOLDER

## Open items

- Connect-timeout coverage uses a 1 ns budget against loopback (the connect is pending on its first
  poll, so tokio's timer wins on the second). It cannot exercise a genuinely unreachable host without
  network; if it ever flakes, drop it — the `read_timeout` test covers the timer plumbing.
- `cargo tree -p web -i wreq` shows one `wreq`; `btls-sys` symbol prefixing (C's R1.0 note) unchanged.
