# 210 as built: FOLLOW-A (dir pin after restart, engine data dir, B2 test, double skips human-only, wire fixture)

Branch `feat/210-public-share`. Files: `crates/share/migrations/0003_share_pins.sql` (new),
`crates/share/src/{engine.rs,routes.rs,sources.rs,store.rs,edge.rs,edge/tests.rs}`, `crates/double/src/actor.rs`,
`crates/theuth/tests/fixtures/wire/openrpc-v1.json` (via `just wire-schema-accept`).

## What was built

| item | how |
|---|---|
| M1 after restart | New local table `share_pins (share PK, canon, dev, ino)` (migration 0003; not in `store::REPLICATED`). `Engine::publish` now calls `sources::pin_root` (was `check_root`) against the engine's `data_dir` and `$HOME`. `publish_admitted` stores the pin with the share row; if storing fails, the pin is deleted along with the secret and request. `stop` deletes the pin. Every mount is built by the new `Engine::mount(&Share)`: a `port` share mounts as before. A `dir` share loads its stored pin and re-runs `check_root` on the pinned canonical path. That path must still canonicalise to itself, and `DirPin::check` must still see the same `(dev, ino)`. Otherwise the share is **stopped with `StoppedBy::SourceGone`** and a warn log with the reason (`not the folder approved at publish (...)`, `the path now leads to ...`, the `DirRefused` text, or `no pinned folder was stored at publish`). It is never re-pinned. A store read error skips the mount and leaves the share as it is. Used by `restore` (start and every `welcome`), the pending→live promotion in `after_cert`, and the offline→live step in `welcome`. |
| M1 data dir | `routes::Mount::of(&Share, Option<DirPin>)` no longer pins or checks anything and does not read `platform::env::data_dir()`. All pinning and protected-path checks run in the engine against `Engine.data_dir`, the directory `rt.data_dir` the plugin runtime gives it (`lib.rs`). |
| B2 test | `engine::tests::cert_event_replaces_the_installed_chain`: the fake edge connection (`edge::tests::FakeConn`, now `pub(crate)` with `FakeConn::new(Router)`) serves `GET /v1/names/sky/cert` from an axum router. Fake `NameKeyStore` (`MemKeys`). `welcome` with `sky` `valid` installs chain 1. Then the route serves chain 2 (same key, other serial), and an `Event::Cert{name:"sky"}` must replace the leaf that `Certs::lookup("app.sky.tun.zip")` returns. |
| double skips human-only | `impl Message<ApprovalRequested> for Double`: returns at once when `m.human_only`, so there is no card, no example, no prediction and no label. Test `actor::tests::human_only_cards_are_never_seen`: a `RequestHumanOnly` is raised, then a normal approval. Only the normal one is predicted. |
| wire fixture | `just ts-desktop` then `just wire-schema-accept`; see Verification for the diff check. |

## Public API changes

- `routes::Mount::of(s: &Share, dir: Option<DirPin>) -> Mount` (was `of(&Share)`). All callers are in `engine.rs`.
- `sources::DirPin::{from_parts(PathBuf, u64, u64), dev(), ino()}`.
- `store::Store::{put_pin(&str, &DirPin), pin(&str) -> Option<DirPin>, delete_pin(&str)}`.
- `edge::tests` is `pub(crate)` (test cfg only); `FakeConn::new`.

## Deviations

- A swapped or now-refused root stops the share as `source_gone`, the design's "source left its allowed set". It is
  not a new `StoppedBy` variant, so the wire enum is unchanged.
- A live `dir` share stored before this migration has no pin. It is stopped at the next restore instead of being
  re-pinned. Branch-only data, so no migration backfill.

## Not built

- `TODO(210-M1)` (unchanged, `sources.rs`): Linux `openat2(RESOLVE_NO_SYMLINKS)` for intermediate components.
- The pin is local to the hosting engine. Moving a share to another engine is M2 and would need a new pin there.

## Doubts

- `recheck` compares `check_root(canon)` with `canon`. On macOS, `/var` → `/private/var` is already resolved in the
  stored canonical path, so this holds. A filesystem that canonicalises case differently would stop the share
  (fails closed).

## Verification

Each command ran under its own hold of `/tmp/theuth-build.lock`, after the last code edit:

- `CARGO_BUILD_JOBS=6 cargo test -p share` → `test result: ok. 34 passed; 0 failed`. New tests:
  `engine::tests::{restore_mounts_dirs_from_the_stored_pin_and_stops_swapped_roots, cert_event_replaces_the_installed_chain}`.
- `CARGO_BUILD_JOBS=6 cargo clippy -p share -p double --all-targets --no-deps -- -D warnings` → `Finished`, no
  diagnostics. The only warning is cargo's future-incompat notice for the external crate `block v0.1.6`.
- `CARGO_BUILD_JOBS=6 cargo test -p double` → `test result: ok. 82 passed; 0 failed`. New test:
  `actor::tests::human_only_cards_are_never_seen`. An earlier run failed 11 actor tests at the harness's
  `approval.register(..).unwrap()` and at the `override demotes one rung` timing assertion. That run had the machine
  under heavy parallel load, and the clean rerun on the same code passed all 82. Load-sensitive, not caused by this change.
- Wire (item 5): **not done.** At 14:56 an engine restart killed my first `just ts-desktop`. The rerun (own hold,
  ~15:15) failed in `wire-schema` (`cargo test -p theuth --bin t export_bindings`) with 5 compile errors in another
  builder's uncommitted M2 work in `crates/share`: `family.rs:39` `MeshStatus::map_err`, and `rpc_share.rs:292,306,315,320`
  `?` type mismatches. None of those edits are mine. `openrpc-v1.json` is unchanged: `share_password` and
  `Stopped.edge_pending` are still missing. To do once `crates/share` compiles: `just ts-desktop && just wire-schema-accept`,
  check that `git diff crates/theuth/tests/fixtures/wire/openrpc-v1.json` only adds `share_*` entries, commit it, then run
  `cargo test -p theuth --test wire_schema_snapshot`.

## Edits for the share-proto contract change (CONTRACT2)

CONTRACT2's share-proto fields broke `crates/share`. At the orchestrator's request, FOLLOW-A owns these compile fixes
(commit `e2a765f55`):

- `edge::Control::put_routes` sends `RoutesPut { hosts, published_ms: {} }`, with
  `TODO(210-M2)` to send each host's `live_ms` so the edge can run its `stopped_all` check. Until then the edge judges
  hosts by the `hello` time, as before.
- `Engine::event` ignores `Event::Throttle` (next to `Queue`/`Stats`, `TODO(210-M3)` throttled badge).
- Test literals: `Welcome.throttled: false`, `NameInfo.{transfer_to, transfer_until_ms}: None`.
