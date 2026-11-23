# 210 as built: FIX-B (branch-review fixes, visitor side)

Branch `feat/210-public-share`. Files: `crates/share/src/{sources.rs,proxy.rs,gate.rs,visitor.rs,routes.rs,wiring.rs}`,
`crates/share/Cargo.toml` (one dependency line). Fixes the 2026-11-23 Sonnet branch review findings M1, M2, m1 (visitor
half), m3, m4, m5.

## Findings

| id | status | how |
|---|---|---|
| M1 dir root swappable after approval | fixed | `sources::DirPin` (canonical root + `dev`/`ino`), built by `sources::pin_root` = `check_root` + `lstat`. `check_root` now applies the dot rule to the canonical path too (a typed path that is a link into `~/.ssh`/`~/.theuth` is refused) and the data-dir/`$HOME` rules already ran on the canonical path. `Mount::of` pins `Source::Dir` roots (re-running the checks against `platform::env::data_dir()` and `$HOME`); `Mount.dir: Option<DirPin>`, `None` → 404. Per request `DirPin::open`: open the canonical root `O_DIRECTORY` + `O_NOFOLLOW_ANY` (macOS) / `O_NOFOLLOW` (Linux), require the pinned dev/ino, `resolve` below the pinned path, `open_nofollow`, check the root again before serving. `proxy::serve_dir` no longer re-canonicalises the typed root. |
| M2 port deny-list only covers the RPC listener | fixed | `sources::ProcessTree: ProcessCheck` over the `listeners` crate (socket table: `proc_pidfdinfo` on macOS, `/proc` on Linux): a TCP socket in `LISTEN` on the port whose owner pid is this process or has it as an ancestor (parent walk via `proc_pidinfo(PROC_PIDTBSDINFO)` on macOS, `/proc/<pid>/stat` elsewhere, depth ≤ 64). `wiring::engine_hooks` installs it instead of `NoProcessCheck`. Checked at publish (engine already calls `PortGuard::connect`) and on every upstream connect; `PortGuard::connect` now runs the check in `spawn_blocking`. |
| m1 (visitor half) slowloris | fixed for HTTP/1.1, partial for h2 | `visitor::http_server()`: h1 `header_read_timeout(10 s)` with `TokioTimer`, `max_buf_size(64 KiB)` (head above it → 431); h2 `max_header_list_size(64 KiB)`. `proxy::forward`: port checks + TCP connect + h1 handshake under `UPSTREAM_CONNECT_TIMEOUT` (10 s), response head under `UPSTREAM_HEAD_TIMEOUT` (60 s, the design's idle-upstream figure); timeouts answer 504. |
| m3 cookie carries argon2 tail, name not MAC-bound | fixed | `Gate.passwords: share → (phc, token)`; the token is 256 bits (two v4 UUIDs, base32) minted when a new or changed hash is installed, kept when `restore` re-installs the same hash. Password cookie value = `<share id>.<token>`; interstitial cookie value = `<share id>` (was `"1"`). A cookie renamed to another share's name fails the value check whether or not the jar's MAC covers the name. |
| m4 limiter per exact IPv6 | fixed | `gate::limiter_key`: IPv6 → its /64, IPv4-mapped IPv6 → the IPv4 address, IPv4 → itself. |
| m5 SNI never compared with Host | fixed | `Proxy::handle(req, visitor, sni)`; `visitor::serve` passes the TLS session's SNI (`ServerConnection::server_name`, falling back to the PROXY `PP2_TYPE_AUTHORITY`). After routing, `421` unless `Mount::name_of(sni) == Mount::name_of(host)` (`<name>.D` from the mount's new `domain` field). Same-name sub-labels on one connection (h2 coalescing inside a name the cert covers) pass, routing stays by `Host` (§5.4). Empty SNI → 421. |

## Public API changes

- `sources::{DirPin, pin_root, ProcessTree}`; `DirPin::{check, open}`, `DirPin.canon`.
- `routes::Mount` gains `domain: String`, `dir: Option<DirPin>`, `Mount::name_of(&str)`.
- `proxy::Proxy::handle` takes `sni: &str`; `proxy::{UPSTREAM_CONNECT_TIMEOUT, UPSTREAM_HEAD_TIMEOUT}`.
- `visitor::{http_server, HEADER_READ_TIMEOUT, MAX_HEADER_BYTES}`.
- New dependency `listeners = "0.6.1"` (MIT, released 2026-08, deps on macOS: `byteorder`; Linux: `rustix`).
  `Cargo.lock` changes are left uncommitted.

## Deviations

- The pin is taken in `Mount::of`, not stored in the share row: `model.rs`/`store.rs`/`engine.rs` belong to
  FIX-SHARE-A. A mount rebuilt after a restart or reconnect re-pins whatever the path names then (the protected and dot
  checks still apply to it). `TODO(210-M1)` in `routes.rs`. Ask for FIX-SHARE-A: persist `(canon, dev, ino)` at
  publish and build the mount from it.
- `Mount::of` uses `platform::env::data_dir()` for the protected data-dir check, not the engine's `data_dir` (not
  reachable from `routes.rs`); `engine::publish` still runs `check_root` with its own `data_dir`.
- m3: tokens live in memory; an engine restart signs password visitors out.
- M2 fails open when the socket table cannot be read (`None`), as the `ProcessCheck` contract already said.

## Not built

- `TODO(210-M2)` HTTP/2 has no request-head deadline in hyper: an h2 connection that opens no stream lives until the
  edge's idle timeout (`visitor.rs`).
- `TODO(210-M1)` Linux `openat2(RESOLVE_NO_SYMLINKS)` (Linux follows links on intermediate components; the post-open
  root check narrows, not closes, that window).
- A daemon that double-forked away from theuth's tree is not caught by `ProcessTree` (registry only).

## Doubts

- `ProcessTree` enumerates every process's sockets per upstream connect (`listeners::get_all`); cost unmeasured
  (no benchmarks allowed). If it shows up, cache per port for a second or two.
- The h2 coalescing rule accepts any host of the same name on a connection; the cert's `*.<name>.D` covers one label,
  and routing only holds hosts this engine registered, so a deeper host 421s at routing.

## Verification

Run after the last edit (2026-11-23), each under its own hold of `/tmp/theuth-build.lock`:

- `CARGO_BUILD_JOBS=6 cargo test -p share` → `test result: ok. 32 passed; 0 failed` (lib), doc-tests 0.
  New tests (confirmed in `--list`): `sources::tests::{swapped_root_is_refused, process_tree_owns_own_and_child_listeners}`,
  `visitor::tests::{slow_head_is_dropped, oversized_head_is_refused}`, `gate::tests::limiter_buckets_ipv6_by_64`,
  `routes::tests::name_of_host`. Extended: `gate::tests::interstitial_then_password` (m3) and
  `proxy::tests::misdirected_and_dir_and_upstream` (m5, pinned dir). No regression test for the upstream timeouts
  (10 s / 60 s constants; not worth a slow test).
- `CARGO_BUILD_JOBS=6 cargo clippy -p share --all-targets -- -D warnings` → clean.

An earlier run of `interstitial_then_password` (FIX-SHARE-A's run) hung. The cause was my new test code: it held
`g.passwords.lock()` as a temporary while calling `set_password`, which locks the same mutex. That deadlock existed
only in the test, and binding the value first fixed it. The gate code holds no lock across an `.await`.
