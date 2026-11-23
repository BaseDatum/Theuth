# 210 as built — FIX-EDGE (branch review fixes in `share-edge`)

Design: `docs/design/210-public-share.md` rev 8 and its appendix. Builds on `210-as-built-EDGE1.md` and
`210-as-built-EDGE2.md`. Scope: the findings of the 2026-11-23 branch review in `crates/share-edge/**`.
No `share-proto` change.

## Findings

| id | status | how |
|---|---|---|
| M5 | fixed | `Store::claim(n, reserved_max) -> Claimed` runs the whole §5.2 decision in one `BEGIN IMMEDIATE` transaction: row read (ban mark, owner, state, cooldown), the mesh's held names for the quota, then an upsert whose `DO UPDATE … WHERE names.state = 'released' AND names.banned IS NULL AND (reclaim_after_ms IS NULL OR reclaim_after_ms <= now OR owner = claimer)`; `rows_affected() == 1` is checked. `control::claim` maps `Claimed::{Granted, AlreadyHeld, Banned, Taken, Cooldown(t), Quota}` to the wire codes. `Store::release(domain, name, owner, now, cooldown) -> Option<reclaim_after_ms>` is conditional on the owner and a held state, computes `max(now + cooldown, not_after_ms)` in SQL, and in the same transaction moves the chain to the new `revocations` table, so a re-claim can never wipe an unrevoked chain. |
| M6 | fixed | `Store::store_chain` now takes `owner` and the CSR SPKI hash and updates only `WHERE owner = ? AND csr_spki = ? AND banned IS NULL AND state IN (held)`; it returns whether a row changed. `certs::process` queues the orphan leaf for revocation (`cessationOfOperation`) and finishes the job `dropped` when nothing was stored (also when the issued key differs from the CSR's). Revocation is durable: migration `0002_revocations.sql` adds `revocations` (chain, reason, state `queued/done/failed`, attempts, `next_at_ms`, `last_error`); release and ban move the chain there in a transaction (`move_chain_to_revocations`, only for a released/banned row), a superseded chain after a rotation is queued too. `certs::revoke_due` (one pass at a time, under a mutex) runs at every `tick` and right after `retire`; an error retries with the job backoff (1 min doubling, jitter) until `acme.max_attempts`, then `failed` (logged as an error). `retire` no longer returns before deleting records when the CA fails. `retire` leaves jobs and records alone when the name is held again (re-claimed by its previous owner before `retire` ran). `submit` ignores an open job of a previous holder (its result is refused by the guard above). |
| M7 | fixed (edge half) | `Session::hello_ms` (edge time of the `hello`). `PUT /v1/routes` answers `403 hello_required` ("Stop all at …: reconnect, then publish again") when the mesh's `stop_all_ms >= hello_ms`, checked before applying and again after inserting (then the tunnel's routes are dropped): `mesh_stop` writes the store before dropping routes, so a racing put is either seen or swept. The per-share rule (`live_ms <= stop_all_ms`) stays the client's: `RoutesPut` carries only hosts. A per-host publish time in `RoutesPut` would let the edge enforce it too; that is a `share-proto` change, not made. |
| M10 | fixed | `control::serve`: at delegation expiry one `kill {delegation_expired}` and `break` (the loop then detaches once). Before, the elapsed timer re-armed at `now`, so every pass spawned a close task and sent a `Kill`. No test: the test would need a delegation that expires while it runs. Checked by reading the code only. |
| M11 | fixed (edge half) | `hello` refuses `Hello.v != PROTOCOL_MAJOR` with `426 client_too_old` (message names the protocol rule; share-proto has no separate version code) before anything else, then `client_ok` against `min_client` (already enforced; now with a message). |
| m1 | fixed | Admin socket lines are read with `read_until` through `take(MAX_LINE + 1)`, `MAX_LINE = share_proto::limits::MAX_JSON_BODY` (64 KiB); an over-long line is answered `too_large` and the connection closed. |
| m2 | fixed | `admin::bind` binds inside a fresh `0700` staging directory next to the socket path, sets `0600`, renames into place, removes the directory. The socket is never reachable with umask permissions; no `unsafe` umask call. |
| m9 | fixed | (a) `dns::TxtProbe` trait + `dns::AuthoritativeTxt` (hickory): NS set of the DNS zone from the system resolver, one uncached, non-recursive resolver per NS address, polled every 2 s until every server answers the value or `acme.propagation` passes (a hard cap of `propagation + 15 s` around the whole probe). A value still missing at the deadline is logged and the CA asked anyway, as before. `certs::Backends` gained `probe`; `start` uses `AuthoritativeTxt`, the test fakes use `FakeZone`. (b) `GET /v1/info` is rate limited per client network (`Tunnel::net`: IPv4, IPv6 /64, or `relay:<url>`) with a keyed `governor` limiter, `limits.info_per_ip_min` (default 30, the appendix value): over it, `429 rate_limited`. |
| TODO(210-M3) attach | fixed | `renewal_scan` counts a mesh with a live tunnel (`Edge::mesh_tunnels`) as attached and stamps its held names' `last_attach_ms` (`Store::touch_attached`), so a tunnel up for more than 90 days without a new `hello` keeps renewing and the future inactivity expiry sees a true time. No test (a live tunnel needs an iroh connection inside the certificate rig). |

## Public API changes

```rust
store::Claimed { Granted, AlreadyHeld, Banned, Taken, Cooldown(i64), Quota }
store::RevocationRow { id, name, chain_pem, reason, state, attempts, last_error }
Store::claim(&self, n: NewName<'_>, reserved_max: u32) -> Result<Claimed>                 // was -> Result<()>
Store::release(&self, domain, name, owner, now_ms, cooldown_ms) -> Result<Option<u64>>    // was (domain, name, now, reclaim_after)
Store::store_chain(&self, domain, name, owner, spki, ca, chain_pem, not_after_ms, ari) -> Result<bool>
Store::{retire_chain, queue_revocation, due_revocations, revocations, revocation_attempt, touch_attached}
Store::clear_chain                                                                         // removed (retire_chain)
certs::revoke_due(edge, certs, now)
certs::Backends { zone, ca, caa, probe: Arc<dyn dns::TxtProbe> }                          // + probe
dns::{TxtProbe, AuthoritativeTxt}
acme::Reason::{as_str, parse}
admin::MAX_LINE
config::Limits::info_per_ip_min                                                            // default 30
state::Session::hello_ms; Edge::info_rate
```

## Deviations

- M11: one code (`client_too_old`, 426) for both a protocol-major mismatch and `min_client`, since share-proto
  has no `version` code; the `message` distinguishes them.
- M7: the edge's rule is per session (a `hello` after the last Stop all), not per share; see the table.
- m9: on the probe's deadline the order goes ahead (old behaviour), it does not fail the attempt.
- `acme.propagation` now means "poll deadline", not "fixed sleep"; the default (30 s) is unchanged.

## Not built / left open

- TODO(210-M3): `/v1/info` limiter keys never expire (`governor` keyed state grows with distinct networks
  until restart; the visitor `ip_rate` limiter has the same property). A periodic `retain_recent` belongs with
  the M3 limits work.
- `deploy/share-edge/config.example.yaml` and its README still describe `propagation` as a fixed wait and do not
  list `info_per_ip_min` as enforced (not an owned path).

## Doubts

- `AuthoritativeTxt` was never compiled (see Verification) and never run against real DNS (no network in tests). The NS lookup uses the
  system resolver; a zone whose NS answers come from a parent delegation only is not handled specially.
- Relayed engines share one `relay:<url>` key for the `/v1/info` limit (30/min per relay); a busy relay could
  starve newcomers. That follows §9.4's per-network rule as written.
- `BEGIN IMMEDIATE` serializes claims and releases on the sqlite write lock (busy timeout: sqlx default 5 s).

## Tests added

- `tests/certs.rs` `concurrent_claims_have_one_winner_and_keep_the_quota` (M5): two meshes claim one name from
  two tasks → exactly `Granted` + `Taken`; one mesh claims two random names at once → `Granted` + `Quota`, one
  held; a release is owner-conditional (second release `None`), others get `Cooldown`, the previous owner
  re-claims (`Granted`, then `AlreadyHeld`).
- `tests/certs.rs` `a_chain_ordered_before_a_release_never_lands_on_the_next_owner` (M6 + m9): the fake CA is
  held after validation; mesh `a` releases, `b` claims and submits; the released order completes → `a`'s leaf
  revoked (`cessationOfOperation`), `a`'s job `dropped`, `b` gets its own chain (leaf SPKI = `b`'s CSR); every
  challenge TXT was probed as `_acme-challenge.sky.tun.zip.`.
- `tests/certs.rs` `a_failed_revocation_is_retried_and_the_records_still_go` (M6): a failing `revoke` leaves the
  zone clean, the chain off the row, a `queued` revocation with attempt 1 and the error; not retried before the
  backoff, `done` after it.
- `tests/visitor.rs` `control_gates` (m9, M11, M7, m1, m2): `/v1/info` 200, 200, 429 `rate_limited` at
  `info_per_ip_min: 2`; `hello` with `v: 2` and with a client below `min_client` → 426 `client_too_old`; after a
  Stop all written to the store, `PUT /v1/routes` → 403 `hello_required`, after a new `hello` → 204; admin socket
  mode 0600 with no staging directory left; a 64 KiB + 1 line → `too_large`.
- Existing tests updated to the new `claim`/`release`/`store_chain` signatures.

## Verification

- **`cargo clippy -p share-edge --all-targets -- -D warnings`: unverified (lock).** Queued under the mkdir lock
  from 13:11 to 13:36 WET; other worktrees' `cargo test` runs held it the whole time. Cancelled after 25 min, as
  the house rule says.
- **`cargo test -p share-edge`: unverified (lock).** Not started, same reason.
- **`cargo tree -p share-edge -e normal`: unverified (lock).** Not run. No dependency was added: hickory,
  governor and rand were already direct dependencies, so the path crates are still `share-edge`, `share-proto`,
  `platform` and `vendor/libsqlite3-sys`. [INFERENCE from the unchanged `Cargo.toml`]
- What did run: `rustfmt --edition 2024 --check` on every changed `.rs` file in the crate. All of them parse.
  This does not type-check or borrow-check anything.
- The hickory 0.26 / sqlx 0.9 / tokio API uses were checked by hand against the vendored sources in
  `~/.cargo/registry` (`Pool::begin_with`, `ResolverConfig::add_name_server`,
  `NameServerConfig::udp_and_tcp`, `ResolverOpts::{cache_size, recursion_desired}`, `RData::NS(NS(pub Name))`,
  `Record::data`, `hickory_resolver::net::runtime::TokioRuntimeProvider`).
- No `Cargo.lock` change.

Commands to run once the lock is free (one hold each):

```sh
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
CARGO_BUILD_JOBS=6 cargo test -p share-edge
cargo tree -p share-edge -e normal | grep -E '\(/'
```

## For the orchestrator

- Proposed `share-proto` change (not made), for the per-share half of M7: `RoutesPut { hosts, published_ms:
  BTreeMap<String, u64> }` (additive, `#[serde(default)]`); the edge would refuse hosts with
  `published_ms <= stop_all_ms`. A dedicated `ErrorCode::VersionMismatch` would also let M11 tell a protocol-major
  mismatch apart from `client_too_old` without the message.
- `deploy/share-edge/config.example.yaml` and its README: `propagation` is now a poll deadline, and
  `limits.info_per_ip_min` is enforced.
- The migration `0002_revocations.sql` is new; existing edge databases migrate on start.
