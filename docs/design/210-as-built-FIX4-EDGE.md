# 210 as built — FIX4-EDGE

Branch `feat/210-public-share`. Code commit `189011e75` (`crates/share-edge/**`), deploy commit `e420fc478`
(`deploy/share-edge/**`). Scope: `crates/share-edge/**`, `deploy/share-edge/**` (files only, nothing applied),
corrections to the EDGE2 and DEPLOY1 as-built notes. No `share-proto` change. No `Cargo.lock` change.

## What was built, per finding

| # | fix | where | test |
|---|---|---|---|
| 2 | Banned names are not held. `NameRow` gained `banned`; `NameRow::held` = `CertRow::held` (state not `released`/`banned` **and** `banned IS NULL`). `held_names` and the claim quota count filter `banned IS NULL`, so `put_routes` (ownership via `held_names`) refuses a banned name with `not_owner`, `welcome.names` and `admin mesh` omit it. CSR was already refused (`submit` checks `row.banned`). `admin ban name` already dropped live routes (`apply_ban`); the test now pins that. | `store.rs` | `tests/mesh.rs::a_banned_name_is_not_held_and_a_stale_route_is_evicted`; `store/ops.rs::bans_unhold_…` |
| 4 | `put_routes`: before the conflict check, routes on the wanted hosts whose mesh is not this mesh (which holds the name, read just before) are evicted, not answered as `route_conflict`. After the inserts, `held_names` is read again and this tunnel's routes on names no longer held are dropped, answered `not_owner` (same write-store-then-drop argument as `after_stop_all`). | `control.rs` | `tests/mesh.rs` (stale route of another mesh evicted) |
| 5 | `dns::ensure(zone, t, host, value, ours)` and `dns::remove(zone, t, host, ours)` take the ledger's values for the host. `ensure`: read error → step fails, nothing deleted; foreign different value → step fails; a different value in `ours` → replaced. `remove`: nothing → ok; value in `ours` → delete + read back; foreign → left (warn), ok; read error → fails, nothing deleted. `certs::remove_host` skips the zone entirely when the ledger has no row for the host. `retire` skips the record removal when no `names` row exists (ban of an unclaimed label). `admin ban name <value>` is validated with `share_proto::names::check_label` (`invalid` otherwise). New `Store::dns_values`. | `dns.rs`, `certs.rs`, `admin.rs`, `store/certs.rs` | `dns.rs::only_ledger_values_are_replaced_or_deleted`; ban label check in the `mesh.rs` test |
| 7 | `Store::revoke(mesh, engines, cap, now)` updates only existing `mesh_engines` rows (no more upsert of made-up keys) and returns `None` (nothing written) when the mesh would exceed `cap` revoked rows. `mesh_revoke`: request > `REVOKED_PER_MESH` (256) → `400 invalid engines`; over the cap → `429 quota {scope: mesh, limit: engines}`; `RevokeResult.revoked` lists the engines actually revoked. Per-tunnel control-request limiter `Tunnel::control_rate` (`CONTROL_PER_SEC` 20/s, burst `CONTROL_BURST` 100), checked for every bi-stream request in `serve_stream`; over it → `429 rate_limited`. | `store.rs`, `control.rs`, `state.rs` | `store/ops.rs::bans_unhold_…` (unknown engine skipped, cap) |
| 8 | `Edge::sweep_limiters`: `retain_recent()` + `shrink_to_fit()` on `ip_rate` and `info_rate`; `state::sweep_limiters` task every 60 s, spawned in `start_with`. | `state.rs`, `lib.rs` | none (governor calls; no observable contract to test) |
| 9 | `state::ConnLog`: bounded mpsc (`CONN_LOG_QUEUE` 4096) on `Edge::conn_log`; `push` is `try_send`, a full queue drops the row and bumps `dropped` (`share_edge_conn_log_dropped_total` in `/metrics`); `ConnLog::run` (one task, spawned in `start_with`) inserts batches of up to 256 rows per transaction (`Store::conn_log_batch`, replaces `Store::conn_log`). The visitor's global, per-IP, per-tunnel and per-host slots are dropped before the row is pushed. | `state.rs`, `sni.rs`, `store.rs`, `ops.rs`, `lib.rs` | `store/limits.rs` retention test now goes through `conn_log_batch` |
| 14 | `ensure_records(edge, certs, name, mesh)` re-reads the name before each CNAME write and fails the attempt when the job's mesh no longer holds it (released/banned/transferred). `K8s::delete_name(name, still_gone)` re-checks under the k8s lock; `k8s::gone` passes "no row or not held", so a name claimed again before the delete keeps its Ingress. DNS reconciliation split: `reconcile` (start: `reset_leases` + `reconcile_dns`) and `reconcile_dns`, which the runner also runs every hour (`DNS_RECONCILE_EVERY`) between ticks. | `certs.rs`, `k8s.rs` | `k8s.rs::reconcile_applies_and_deletes_and_lifecycle_ops` (delete with `still_gone = false` keeps objects) |
| 18 | `admin pause`/`resume` persist in the new `edge_state` table (`migrations/0004_edge_state.sql`, key `paused`); `start_with` restores `Edge::paused` (logged). `Store::paused`/`set_paused`. | `store.rs`, `admin.rs`, `lib.rs` | `store/ops.rs::bans_unhold_…` (survives reopen) |
| 20 | `hello` with an invite checks it read-only (`Store::invite_live`) and consumes it inside the attach transaction (`Store::attach_invited`, `BEGIN IMMEDIATE`): a refusal between the check and the attach spends no use; a use lost to a concurrent `hello` → `invite_required`, nothing written. | `store.rs`, `control.rs` | `store/ops.rs::bans_unhold_…` |
| 22 | Stale `TODO(210-M3): the 30-day purge` removed (the purge runs in `queue.rs`'s sweep). Name-ban and unban `UPDATE names` filter on `domain` (`Store::ban`/`unban` take it). DEPLOY1 updated for the delete journal. | `store.rs`, DEPLOY1 | `store/ops.rs::bans_unhold_…` (other domain untouched) |
| 13/21 | `rbac.yaml`: ingress verbs `get, list, create, patch, delete` (`watch`, `update` dropped: `k8s/client.rs` calls only SSA `patch`, `list`, `delete`; `create` kept because a server-side apply that creates needs it; `get` kept read-only). New `networkpolicy.yaml` (in the overlay's resources): TCP 8443 only from namespace `haproxy-controller`, TCP 9090 only from namespace `monitoring`, UDP 7850 open. `listen_proxy_protocol.trusted` left at `10.244.0.0/16` with a comment: HAProxy pod IPs are not determinable from the repo's config; the NetworkPolicy is what bounds who can send PROXY headers. Image tag unchanged. | `deploy/share-edge/k8s/rwx-dev/` | `kubectl kustomize` renders |

## Public API changes

```rust
store::NameRow { …, banned: Option<String> }                // held() now also requires banned == None
store::ConnLogRow { ms, sni, visitor, mesh, engine, up, down }
Store::attach_invited(mesh, engine, net, invite: &[u8], now) -> Result<bool>
Store::invite_live(hash, now) -> Result<bool>
Store::revoke(mesh, engines, cap: u64, now) -> Result<Option<Vec<String>>>   // was -> Result<()>
Store::ban(domain, kind, value, reason, now)                // + domain
Store::unban(domain, kind, value) -> Result<bool>           // + domain
Store::conn_log_batch(&[ConnLogRow])                        // replaces Store::conn_log
Store::paused() / set_paused(bool)
Store::dns_values(host, rtype) -> Result<Vec<String>>
dns::ensure(zone, rtype, host, value, ours: &[String])      // + ours
dns::remove(zone, rtype, host, ours: &[String])             // + ours
certs::reconcile_dns(edge)                                  // reconcile = reset_leases + reconcile_dns
K8s::delete_name(name, still_gone: impl Future<Output = bool>)
state::{ConnLog, sweep_limiters, CONTROL_PER_SEC, CONTROL_BURST, REVOKED_PER_MESH, CONN_LOG_QUEUE}
Edge::{conn_log, sweep_limiters()}, Tunnel::control_rate
```

Migration `0004_edge_state.sql`: `CREATE TABLE edge_state (key TEXT PRIMARY KEY, value TEXT NOT NULL)`.

## Deviations from the brief

- **#2 "check ban in CSR"**: `certs::submit` already refused a row with `banned` set (`403 banned`); no code change,
  covered by the new test. A ban on a label with no row cannot reach CSR (no row → `not_owner`).
- **#4 post-insert drop** answers `403 not_owner` and drops only the lost names' routes of this tunnel, not the
  whole set; routes on names still held stay.
- **#5 `remove` on a foreign value** returns `Ok` (record left, warning) rather than failing, so a retire or
  reconcile still clears the ledger row and moves on; `ensure` on a foreign value fails the attempt (backoff), as
  asked.
- **#7 limits are constants** in `state.rs`, not config keys (no `share-proto`/config contract change). The
  control limiter counts `GET /v1/events` too.
- **#9** `push` never waits; rows in the queue at shutdown are lost (no drain on `Running::shutdown`).
- **#14 hourly reconcile** runs in the cert runner between ticks, so a challenge TXT of this runner's own order is
  never out while it runs; a `retire` spawned concurrently may race it on the same host, both deleting (harmless).

## Not built

- TODO(210-M4): config keys for `CONTROL_PER_SEC`/`CONTROL_BURST`/`REVOKED_PER_MESH`/`CONN_LOG_QUEUE`
  (`admin reload`-able).
- TODO(210-M3): drain the `conn_log` queue on shutdown.
- TODO(210-M1): DNS records written by a provider value the ledger never saw (e.g. a CNAME the operator pointed
  elsewhere) block the name's issuance until removed by hand; there is no admin command to adopt or clear them.

## Doubts

- `networkpolicy.yaml` assumes namespaces `haproxy-controller` and `monitoring` and a CNI that enforces
  NetworkPolicy; kubelet probes to 9090 come from the node and are not matched by the `monitoring` rule
  [INFERENCE: most CNIs exempt host-originated probe traffic; if probes fail after apply, add an `ipBlock` for the
  node CIDR]. Not applied.
- `retain_recent` keeps keys whose state is not yet back to "full"; with the 1 s / 1 min quotas that is at most a
  minute of keys.
- The control limit (20/s, burst 100) is a guess; no test exceeds it today.

## Verification (after the last code edit, each under its own lock hold)

```sh
CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
cargo tree -p share-edge -e normal | grep -E '\(/'
kubectl kustomize deploy/share-edge/k8s/rwx-dev >/dev/null
```

- `cargo test -p share-edge --all-features`: all pass — lib 45, admin 8, certs 15, mesh 5, two more suites 2 and 0,
  doc-tests 0.
- `cargo clippy … --all-features -- -D warnings`: clean (exit 0). An earlier run found one `collapsible_match` in
  `Store::revoke`, fixed before the gates.
- `cargo clippy … -- -D warnings` (default features): clean (exit 0).
- `cargo tree -p share-edge -e normal`: path crates `share-edge`, `platform`, `share-proto`,
  `vendor/libsqlite3-sys` only.
- `kubectl kustomize deploy/share-edge/k8s/rwx-dev`: exit 0.
- `Cargo.lock`: unchanged.
