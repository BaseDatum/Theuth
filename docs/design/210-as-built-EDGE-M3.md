# 210 as built — EDGE-M3 phase (`share-edge`: slot queue, admission limits, metering)

Design: `docs/design/210-public-share.md` rev 8 §6.5, §9.2–§9.6, §8.1 + appendix §8.1; contract
`210-as-built-CONTRACT2.md` (share-proto at f2410c11f, unchanged here: `LimitScope`, `LimitKind`,
`ErrorBody::{limited, edge_full}`, `StatsEvent`, `ThrottleEvent`, `Welcome.throttled`,
`WelcomeLimits.{tunnel_mbit, throttle_mbit}`, `queue_eta_s`, `QueuePos`, `CertPending.queue_pos`). Builds on EDGE1,
EDGE2, FIX-EDGE, EDGE-K8S, EDGE-M2(-GREEN).

## Files

| path | change |
|---|---|
| `crates/share-edge/src/queue.rs` | new: §6.5 order, per-CA/class buckets, spill, position/ETA, `try_slot`, `names_max` + eviction + full state, `queue` events, housekeeping |
| `crates/share-edge/src/limits.rs` | new: network tiers, `AsnDb`, period helpers, `new_mesh`, `tunnels_refusal`, `new_name` (+ `Taken` refund), `cert_request`, `count_slot` |
| `crates/share-edge/src/meter.rs` | new: `Shaper`, `HostStats`, `Meter` (fold, budgets, pace/throttle, flush, load), `stats`, `run` loop |
| `crates/share-edge/src/store/limits.rs` | new: durable counters/buckets (all-or-nothing), slot reads, `evict`, `set_first_live`, `usage`, `conn_log` purge; tests |
| `crates/share-edge/src/certs.rs` | slot queue replaces the single `le_new` bucket; intake limits on queued CSRs; `tick`/`process` take `&Arc<Edge>`; `queue` events on change |
| `crates/share-edge/src/sni.rs` | per-tunnel new-conn rate; `Metered` now shapes, meters live into tunnel/host, write-stall reset, `first_live_ms` hook; tests |
| `crates/share-edge/src/state.rs` | `Tunnel.{folded, invited, shaper, conn_rate}` (+`Tunnel::new(.., &Limits)`), `Route.stats`, `Edge.{asn, meter, full}`, `full()/set_full()`, fold at `detach` |
| `crates/share-edge/src/control.rs` | hello: new-mesh/relayed limits, `tunnels_per_net`, shaping rate, `welcome.{full, throttled, names[].queue, limits.tunnel_mbit/throttle_mbit}`; info `full`; claim limits; routes byte-budget refusal, route stats carry-over; scope fields on `engines_quota`, routes `quota` |
| `crates/share-edge/src/config.rs` | new keys (below), `bytes` serde (`20GiB`), `Cidr::range` |
| `crates/share-edge/src/lib.rs` | `limits`/`meter`/`queue` modules; `Meter::load` at start, `meter::run` task, fold+flush at `shutdown` |
| `deploy/share-edge/config.example.yaml` | every new key documented; `db_journal: wal|delete` added |

No new dependency (`time`, `governor`, `dashmap` were already deps). `Cargo.lock` not changed by this phase (the
pre-existing one-line worktree change is not mine; left uncommitted).

## New config keys

`acme.invite_share` (0.5), `acme.queue_max` (2000); `limits.`: `creates_per_net_hour` (30), `creates_burst` (10),
`new_meshes_per_net_day` (5), `certs_per_net_week` (2), `certs_per_mesh_week` (2), `queued_per_net` (2),
`reserved_per_net` (6), `tunnels_per_net` (10), `wide_net_factor` (4), `v6_32_factor` (16), `asn_factor` (64),
`asn_db` (unset = off), `open_new_names_day` (500), `relayed_new_meshes_day` (20), `relayed_every` (1h),
`relayed_burst` (4), `names_max` (1200), `allocated_ttl` (1h), `evict_unpublished_after` (7d),
`bytes_per_mesh_day` (20GiB), `bytes_per_net_day` (50GiB), `tunnel_mbit` (50), `throttle_mbit` (2), `egress_month`
(10TiB), `new_conns_per_tunnel_sec` (50), `new_conns_burst` (200), `write_stall` (5s), `conn_log_retention` (30d).
Byte keys accept a count or `B/KiB/MiB/GiB/TiB/KB/MB/GB/TB`.

## Public API (`share_edge`)

```rust
queue::{order(&[JobRow], class) -> Vec<&JobRow>, bucket(edge, ca, class) -> BucketSpec, ClassView { ids, tokens,
        refill_s, next_token_s, spill } + pos(id) -> Option<QueuePos>, view(..), Slot { Slotted, Wait { pos, retry_ms } },
        try_slot(&Arc<Edge>, ca, &JobRow, now), has_room, evict(&Arc<Edge>, name, only_unpublished, now), position,
        mesh_positions, announce, housekeeping, SPILL_AFTER_MS}
limits::{DAY_MS, WEEK_MS, day_start, week_start, month_bounds, Tier { scope, key, factor }, is_relayed, wide_key,
         tiers(net, &Limits, &AsnDb), in_tier, AsnDb { load, parse, lookup }, open_names_bucket, new_mesh, tunnels_refusal,
         Taken { refund }, new_name, cert_request, count_slot}
meter::{MAX_BURST, mbit_bytes, Shaper { new, rate, set_rate, charge(n, Instant) -> Duration }, HostStats { conns,
        bytes_in, bytes_out, open, visitors, live, take }, Meter { load, fold_tunnel, over_budget, rate_for, mesh_day,
        net_day, over, month_bytes, throttled }, tick, flush, stats, run}
store::{CounterCheck, BucketSpec}; Store::{counters_check(&[..], bump) -> Result<(), usize>, counters_add, counter,
        buckets_take(&[..], now) -> Result<(), (usize, wait_ms)>, bucket_tokens, bucket_refund, mark, marked, slot_jobs,
        slotted_count, unpublished_names, stale_allocated, evict, set_first_live, first_live, open_reserved_nets,
        usage_add, usage_day, purge_conn_log, conn_log_count}
state::Tunnel::new(conn, pre_hello, &Limits)        // was (conn, pre_hello)
state::Route { .., stats: Arc<HostStats> }; Edge::{full(), set_full()}, Edge.{asn, meter}
certs::tick(&Arc<Edge>, now)                         // was &Edge
certs::NEW_CERT_BUCKET                               // removed (per-CA class buckets `<ca>_invited` / `<ca>_open`)
```

## Behaviour

- **Slot queue (§6.5).** Only a `new` job whose name is `allocated`/`queued` draws on it; renewals and rotations
  never do (unchanged runner path). Buckets per CA id and class: `<ca>_invited` = ⌈week × invite_share⌉ (23 of 45),
  `<ca>_open` = the rest (22), each refilled continuously over 7 days. Order per class: first names, then
  additional names; inside each, round-robin across wide network keys (IPv4 /24, IPv6 /48, a relay URL as its own
  key) in order of each key's oldest job; FIFO within a key. A leased job gets a slot only when its position
  `p ≤ t` (whole tokens), so a later job never overtakes the head. ETA = `share_proto::queue_eta_s(p, t, r,
  next_token)`. A waiting job is requeued at the next token (+ refills for the shortfall), the name goes `queued`,
  a `queue {name, position, eta_s}` event goes to the mesh; after each runner pass every waiting job whose position
  changed gets one too. `202 CertPending {queue_pos, eta_s}` and `welcome.names[].queue` carry the same numbers.
- **Spill.** The open class may take an invited token when no invited job waits, the open bucket is empty and
  no invited token was taken for 24 h (clock starts at the edge's first look, so a fresh edge does not spill).
- **`names_max` at slot time (F-M8).** Before slotting: `slotted+valid < names_max`, else evict the oldest
  never-live name allocated more than `evict_unpublished_after` ago (row deleted, no cooldown, chain revoked
  `cessationOfOperation`, records and routes removed, k8s objects gone), else `Edge::set_full(true)` and the job
  retries in 10 min. Full → `POST /v1/names`/`PUT` answer `503 edge_full {scope: global, limit: names_max}`,
  `/v1/info.full` and `welcome.full` true; cleared when a slot frees.
- **Eviction of uncertified names**: every 60 s `allocated` names without a CSR older than `allocated_ttl` are
  evicted the same way (housekeeping in the meter loop).
- **Limits** (refusals carry `scope` + `limit`; time limits carry `retry_after_s`):
  - hello of an unknown, non-invited mesh: `new_meshes_per_net_day` × factor per tier → `429 network_quota
    {scope: net|net_wide|net32|asn, limit: meshes}`; a relay path (`relay:<url>`) instead draws on the paced
    relayed bucket (1/`relayed_every`, burst 4) and its day cap → `429 paced {scope: relayed, limit: meshes}`.
  - hello: `tunnels_per_net` × factor over live tunnels → `network_quota {limit: tunnels}`;
    `engines_quota {scope: mesh, limit: engines}`.
  - claim/allocation (not a re-claim of a held name): queue at `queue_max` → `503 edge_full {queue_max}`; open
    meshes: `reserved_per_net` × factor for reserved claims → `network_quota {limit: reserved}`; then, all or
    nothing, per-tier claim buckets (`creates_burst` × f, `creates_per_net_hour` × f per hour) → `network_quota
    {limit: claims}` and the global open-names bucket (`open_new_names_day`, burst 1/24) → `429 paced {scope:
    global, limit: names}`. A claim that is not `Granted` refunds the global token.
  - first CSR that will queue: `queue_max`; `certs_per_mesh_week` → `429 quota {scope: mesh, limit: certs}`; open
    meshes: `certs_per_net_week` × factor → `network_quota {limit: certs}`, `queued_per_net` × factor queued open
    jobs → `network_quota {limit: queued}`. Week counters are bumped when the job gets its slot.
  - Invited meshes skip every per-network onboarding count and the open-names bucket.
- **Tiers**: v4 address (×1), /24 (×`wide_net_factor`); v6 /64 (×1), /48 (×wide), /32 (×`v6_32_factor`); ASN
  (×`asn_factor`) when `asn_db` resolves the address (iptoasn TSV `start end asn …` or `cidr asn`; ASN 0 skipped).
- **Durable counters**: day (UTC), fixed 7-day windows (epoch aligned) and month rows in `counters`; buckets and
  the spill marker in `buckets`; byte usage in `usage` — all survive a restart (tested).
- **Metering (§9.6)**: the splice counts both directions live into `Tunnel.bytes` and the route's `HostStats`;
  every second the meter folds each tunnel's delta into per-mesh and per-tier day totals and the month total
  (also at `detach` and `shutdown`). Over `bytes_per_mesh_day` / `bytes_per_net_day` × factor: `kill {budget}` on
  every tunnel of the mesh / network (once a day) and `PUT /v1/routes` → `429 quota {scope: mesh|net, limit:
  bytes}` until the UTC day rolls. Every 60 s: `usage` rows and the `egress/month` counter. `Meter::load` restores
  today's totals and the month at start.
- **Shaping**: per-tunnel byte bucket at `tunnel_mbit` (burst min(64 MiB, 10 s of rate)), waited out in the
  splice's `poll_read`/`poll_write`; unshaped before `hello`. While the month's bytes exceed `egress_month ×
  elapsed fraction`, every non-invited tunnel is re-rated to `throttle_mbit` and gets `throttle {throttled: true,
  mbit}`; back under pace → `tunnel_mbit` and `throttled: false`. `welcome.throttled` gives the state at hello.
- **`stats`**: every 30 s per route host with activity or open connections: `conns, bytes, bytes_in, bytes_out,
  visitors (distinct /64), open, period_s: 30`. Counters survive a re-`PUT` of the same host by the same tunnel.
- **Per-tunnel new connections**: `governor` direct limiter, 50/s burst 200 → TCP reset.
- **`first_live_ms`** is stamped (once) when a visitor stream from outside every network of the mesh's tunnels (same
  /64-or-v4, or same /24 / /48) has received 1 KiB from the engine — mid-stream, not at route time.
- **Write stall**: a write to the visitor pending for `write_stall` (5 s) fails the splice → stream reset + TCP RST.
- **Brand blocklist**: already enforced on reserved claims (`check_brand`, EDGE1); unchanged. Reload is M4.
- **`conn_log`** rows older than `conn_log_retention` are purged every 60 s.

## Deviations

- Fixed 7-day windows (epoch aligned) for "per 7 days" counters instead of sliding windows (a counter row per
  window; at most 2× the limit across a window boundary).
- Spill rule: "unused for 24 h" = no invited take for 24 h and no invited job waiting (no per-token ages).
- Each class bucket holds at least one token (`new_certs_per_week` < 2 is treated as 2), so `invite_share: 0`
  still leaves the invited class one token a week.
- Egress counts both directions at the splice (the host's egress to visitors and to engines).
- Shaping burst is min(64 MiB, 10 s of rate) so the 2 Mbit throttle is not a 64 MiB free burst; implemented as a
  small own token bucket rather than `governor` (it must be re-rated live).
- `edge_full` at `queue_max` is answered at claim time as well as CSR time (a claim would queue).
- Claim-rate tokens are taken for every non-idempotent claim attempt (including ones refused as `name_taken`);
  only the global open-names token is refunded.

## Not built

- `TODO(210-M4)`: re-counting a relayed newcomer against its network once a direct path appears; per-network rate
  limit of `GET /v1/names/{name}`; `orders_per_3h`; `engine_keys_per_mesh_day`; `reserved_min_mesh_age`; inactivity
  expiry of names unattached 90 days; brand file / limits reload (`admin reload`); queue/limits in
  `admin status`/`admin queue` and `/metrics`; the failure cool-down per name (2 failed authorizations → 1 h).
- GTS bucket (P2) — the per-CA bucket names are keyed by `Ca::id()`, ready for a second CA.

## Doubts

- `limits::new_name` reads `slot_jobs` (a join over `cert_jobs`) on every claim; fine at `queue_max` 2000, worth an
  index-backed count if claims get hot.
- Budget kill closes the tunnel; a client that reconnects at once can attach but cannot publish routes until UTC
  midnight. Nothing refuses its `hello`.
- `first_live_ms` "outside every network" compares the visitor's /64-or-v4 and /24-or-/48 with the mesh's tunnels'
  networks at stream start only.
- The global open-names bucket is taken before `Store::claim` (refunded on refusal); two racing claims may both see
  a token that only one keeps — harmless.

## Verification

Commands, each under its own `/tmp/theuth-build.lock` hold, after the last code edit:

```sh
CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features --no-fail-fast
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
cargo tree -p share-edge -e normal | grep -E '\(/'
```

Results: see "Verification results" below.

Tests added (time injected; nothing sleeps over 1 s of wall time — paused tokio clocks):
`queue::tests::{first_names_then_round_robin_by_network_then_fifo, eta_counts_tokens_refill_and_jobs_ahead}`,
`limits::tests::{tiers_cover_v4_v6_and_asn, month_bounds_are_utc_months}`,
`meter::tests::{shaper_paces_after_the_burst_and_re_rates, host_stats_reset_each_period}`,
`store::limits::tests::{counters_and_buckets_are_all_or_nothing_and_survive_a_restart,
evictions_first_live_and_conn_log_retention}`,
`sni::tests::{a_visitor_that_stops_reading_is_reset_after_the_write_stall, a_reading_visitor_never_trips_the_stall}`.
Existing `tests/certs.rs` `new_names_wait_for_a_token` and `renewal_runs_while_the_new_cert_bucket_is_empty` cover
the slot queue through the runner (position 1, waiting is not an attempt, renewals bypass it).

Not covered by an automated test: the hello/claim limit paths end to end over iroh (the store and tier logic they
call are unit-tested), the throttle switch and budget kill over live tunnels, the `stats` SSE over a live tunnel.

## Verification results (all after the last code edit, each its own lock hold)

- `cargo test -p share-edge --all-features --no-fail-fast`: exit 0 — unit 36/36 (10 new), `certs` 14/14, `mesh`
  4/4, `visitor` 2/2.
- `cargo clippy -p share-edge --all-targets --all-features -- -D warnings`: exit 0 (one `result_large_err` fixed by
  `limits::tunnels_refusal` returning `Option<ErrorBody>`).
- `cargo clippy -p share-edge --all-targets -- -D warnings`: exit 0.
- `cargo tree -p share-edge -e normal` path crates: `share-edge`, `share-proto`, `platform`, `vendor/libsqlite3-sys`.
  No new packages; `Cargo.lock` untouched by this phase.
