# 210 as built — EDGE-M4 phase (`share-edge`: invites, admin commands, tickets/standby/retire, metrics, brake, M3 leftovers, drain)

Design: `docs/design/210-public-share.md` rev 8 §5.5, §7.6, §8.1, §8.4–§8.5, §9.3–§9.5 + appendix §8.1/§8.4.
Contract: share-proto at f2410c11f (unchanged here; typed admin answers of `share_proto::admin`). Builds on EDGE1,
EDGE2, FIX-EDGE, EDGE-K8S, EDGE-M2, EDGE-M3. The live cluster edge (`share-edge-a0c7a86e8`) is untouched; nothing
here is deployed until the orchestrator rebuilds the image.

## Files

| path | change |
|---|---|
| `crates/share-edge/src/admin.rs` | every `AdminCommand` answered with its typed result: `status` (`StatusResult` incl. names/queue/egress/standby), `queue`, `mesh`, `release`, `evict-unpublished`, `recover`, `invite-new/list/revoke`, `caa-check`, `revoke-cert`, `reload`, `brake-status`; `pub fn reload`, `pub async fn {status, queue, caa_check}` |
| `crates/share-edge/src/ops.rs` | new: `Ops` counters (refusals by code/scope/limit, cert failures, splice bytes, minute windows, per-name failures), `brake_status`, `brake_tick` (auto brake), `run_brake`, `render` (`/metrics`) |
| `crates/share-edge/src/store/ops.rs` | new: invites (mint/list/revoke by prefix), `mesh_detail`, `engines_first_seen_since`, `engine_known`, `rehome_relayed`, `unattached_names`, `recover`, `revoke_name_chain`, `hex_lower`; unit test |
| `crates/share-edge/src/state.rs` | `Live` (reloadable config) behind `Edge::live()/set_live()`; `Edge.{ops, standby, retire, draining, config_path}`; `Session.terms_rev`; `Edge.brands` moved into `Live` |
| `crates/share-edge/src/control.rs` | refusal counting via a response extension; `terms()` check on claims **and CSRs** (`403 terms_required`, message = terms URL, against the live revision); hello refused while draining; new-mesh counter; relayed re-count; `engine_keys_per_mesh_day`; `reserved_min_mesh_age`; `welcome.retire`; `name_info` `pub(crate)` |
| `crates/share-edge/src/certs.rs` | `Certs.caa`; `orders_per_3h` bucket before each order; per-name failure cool-down; cert-failure counter |
| `crates/share-edge/src/queue.rs` | housekeeping: 90-day unattached expiry |
| `crates/share-edge/src/sni.rs` | splice bytes into `Ops.bytes` (in/out) |
| `crates/share-edge/src/lib.rs` | standby key created at start, `standby_ticket`, `retire`, `retire.json` loaded into `welcome.retire`, `drain`, `run(cfg, secrets, config_path)` drains on SIGTERM, brake task, daily CAA task, `/metrics` via `ops::render` |
| `crates/share-edge/src/main.rs` | CLI: `admin queue|mesh|release|evict-unpublished|recover|invite mint/list/revoke|caa-check|revoke-cert|reload|brake-status`, `ticket [--standby]`, `retire` |
| `crates/share-edge/src/config.rs` | keys below; `standby_key_path`, `retire_path` |
| `crates/share-edge/tests/admin.rs` | new: 7 tests (below) |
| `deploy/share-edge/config.example.yaml` | new keys documented |

## New config keys

Top level: `drain_secs` (30), `brake: {auto: false, refusals_per_min: 0, new_meshes_per_min: 0, queue_eta_hours: 72}`.
`limits.`: `engine_keys_per_mesh_day` (8), `reserved_min_mesh_age` (0s), `unattached_expiry` (90d), `name_fail_max` (2),
`name_fail_cooldown` (1h). `acme.cas.letsencrypt.orders_per_3h` (200).

## Public API (`share_edge`)

```rust
ticket(cfg, id) -> ShareEdgeTicket            // unchanged; invites added by `admin invite new`
standby_ticket(cfg) -> ShareEdgeTicket        // creates <data_dir>/iroh.standby.key if missing
retire(cfg, now_ms) -> share_proto::Signed    // signs Retire{edge: standby, retired: live}, writes retire.json, promotes
drain(&Edge, secs) -> usize                   // tunnels left at the deadline
run(cfg, secrets, config_path)                // was run(cfg, secrets)
admin::{status, queue, caa_check, reload(&Edge, &Config) -> ReloadResult}
ops::{Ops { refused, new_mesh, refusals_per_min, meshes_per_min, name_failed, .. }, ByteTotals,
      is_limit_refusal, brake_status, brake_tick, pulled, run_brake, render}
state::{Live { limits, brands, reserved_words, terms_url, terms_rev, abuse_url, queue_max, invite_share, brake,
               admission } + from_config}, Edge::{live, set_live}, Edge.{ops, standby, retire, draining, config_path}
state::Session.terms_rev
store::{InviteRecord, InviteRevoke, MeshDetail, hex_lower}; Store::{mint_invite, live_invites, revoke_invite,
        mesh_detail, engines_first_seen_since, engine_known, rehome_relayed, unattached_names, recover,
        revoke_name_chain}
certs::Certs.caa
config::{BrakeConfig, Config.{drain_secs, brake}, Config::{standby_key_path, retire_path}}
```

## Behaviour

- **Invites.** `admin invite mint [--uses N] [--expires 7d] [--note …]` (alias `new`) stores only the hash
  (`invites`), answers `InviteMinted {ticket, hash, uses, expires_ms}`; the CLI prints the ticket-with-invite on
  its own first line, then the YAML. `invite list` = live invites (hash, uses left, expiry, note, never the
  secret). `invite revoke <hash|≥8-hex prefix>` sets uses to 0 (`not_found` / ambiguous → `invalid`). `hello`
  consumes one use under `admission: invite` (EDGE1) and an invited mesh skips every per-network count (M3).
- **Admin answers** are the share-proto typed results. `queue`: new-name jobs waiting for a slot, invited class
  first, §6.5 order, position/ETA as the client sees them (`ordering` = position 0), tokens and next token.
  `mesh <id>`: `MeshResult` incl. names, engines, revoke list, rekey history, ban. `release <name>`: as the
  holder's DELETE (cooldown, revocation, records, routes, k8s). `evict-unpublished`: names never live
  `evict_unpublished_after` after allocation. `recover <name|mesh> --to-mesh <id> --ref T`: audited (every
  mutating command is), moves one name (routes dropped) or every name of a mesh (its tunnels get `kill {revoked}`),
  creates `to`'s `meshes` row if unseen, keeps chains and jobs with the names. `revoke-cert <name> [reason]`:
  chain to `revocations` now, revoked at once, a `valid` name goes back to `slotted`. `caa-check`: resolves `D`'s
  CAA set against the primary CA's lines; under `require_caa` a failure flips admission to `closed`; the same check
  runs every 24 h. `reload`: re-reads the config file (`--config` of `run`) and applies `limits`, brand file,
  `reserved_words`, terms/abuse URLs, `acme.queue_max/invite_share`, `brake` and a changed `admission`; keys sized at
  start (`domain`, `data_dir`, `listen`, `iroh`, `admin`, `dns`, `k8s`, `acme.cas`, `limits.{global_conns,
  pre_hello_conns, new_conns_per_ip_sec, info_per_ip_min, asn_db}`) are listed in `restart_required`.
  `brake-status`: `BrakeStatus` with every condition and its evidence.
- **Tickets and keys (§5.5).** At first start the edge creates `<data_dir>/iroh.standby.key` (0600) beside
  `iroh.key`; `status.standby` shows it; `ticket` prints the live ticket, `ticket --standby` the standby key's
  ticket (same relay and `public_addrs`). `retire` (edge stopped; refused while the admin socket answers) signs
  `Retire {v: 1, edge: standby, retired: live, issued_ms}` with the standby key (share-proto `Retire::sign`),
  writes `retire.json`, renames `iroh.key` → `iroh.key.retired` and the standby key → `iroh.key`. At start a
  `retire.json` that verifies against the running key goes into every `welcome.retire`. Tested against
  `EdgePins::accept_retire`.
- **Monitoring (`/metrics`).** `share_edge_{tunnels, routes, conns}`, `share_edge_bytes_total{dir=in|out}`,
  `egress_month_bytes`, `throttle_engaged`, `throttled_tunnels`, `edge_full`, `names_slotted`, `names_max`,
  `queue_max`, `queue_depth{class}`, `slots_left{ca,class}`, `cert_job_failures_total`,
  `refusals_total{code,scope,limit}` (limit refusals: `network_quota`, `paced`, `quota`, `engines_quota`,
  `edge_full`), `refusals_per_min`, `new_meshes_per_min`, `admission{mode}`, `brake_pulled`, `k8s_errors_total`.
- **Brake (§9.5).** Conditions: `edge_full` (full now or an `edge_full` answer in the last 24 h), `queue_eta`
  (longest ETA > `queue_eta_hours`), `bucket_empty` (a class bucket below one token for 7 days, tracked from the
  first brake pass that saw it empty), `abuse_spike` (`refusals_per_min` or `new_meshes_per_min` reached), and
  `search_console`/`safe_browsing` always `holds: false` with the detail "watched outside the edge". Every 10 s:
  with `brake.auto` and `admission: open`, a holding condition flips admission to `invite`, audited as `brake`
  (`ref: auto`), logged; never back to `open` automatically. `clear_days` = days since a condition last held.
- **M3 leftovers.** `orders_per_3h`: a per-CA bucket (200 per 3 h, continuous refill) taken right before each order
  (any job kind); empty → requeued at the next token without spending an attempt. `engine_keys_per_mesh_day`: an
  engine key the mesh never had, when the mesh already saw that many new keys today → `403 engines_quota {scope:
  mesh, limit: engines}`. `reserved_min_mesh_age`: a reserved claim by a younger mesh → `quota {scope: mesh, limit:
  reserved, retry_after_s}`. Per-name failure cool-down: `name_fail_max` failed attempts in a row (reset on
  success) → the job waits at least `name_fail_cooldown`. Unattached expiry: every minute, held names whose mesh
  last attached more than `unattached_expiry` ago (and has no live tunnel) are evicted (row removed, chain revoked
  `cessationOfOperation`, records and routes removed, `k8s::gone` delete hook). Relayed re-count: a non-invited
  mesh first seen relayed whose `hello` comes over a direct path gets `meshes.net` set to that network and is
  counted in its `new_meshes_per_net_day` tiers (an exhausted count is logged, not answered).
- **Terms (§7.6).** `/v1/info` and `welcome` carry `terms_url`/`terms_rev` (live, reloadable). Claims and CSRs need
  the `hello`'s `terms_rev` to equal the live revision, else `403 terms_required` with the terms URL as `message`.
- **Drain.** `run` on SIGTERM/SIGINT: `draining` (new `hello` → `403 admission_closed`), `drain {secs}` to every
  tunnel, wait until all left or `drain_secs`, then the existing shutdown (fold, flush, close).
- **pkarr.** Not built, by the design: §5.5 and P0-3 say the tickets carry the relay URL and public addresses so
  clients dial without discovery and without pkarr; the `iroh.pkarr` key stays inert (documented).

## Deviations

- Terms are enforced at claim/CSR time (design §7.6: "refuses name and CSR requests … `403 terms_required`"), not at
  `hello`: a `hello` without terms must stay possible (§7.6 step 1: Settings attaches before the first-run card).
  share-proto has no "terms" code beyond `terms_required`; the URL rides in `message`.
- `revoke-cert` reason `unspecified` is sent as `cessationOfOperation` (the `revocations.reason` CHECK has no
  `unspecified`); the answer names the reason actually sent.
- The standby key is generated by the edge at first start, as this phase's brief asks; the design's §13 says
  "generate the standby key offline". The operator should copy `iroh.standby.key` off the PVC and remove it if the
  standby is to stay cold (the edge only needs it for `ticket --standby` and `retire`).
- `retire` promotes the key files itself; the next `run` serves on the former standby key and creates a fresh
  standby key (pinned by nobody until a release ships it).
- `recover` of a mesh kills its tunnels with `kill {revoked}` (no dedicated kill reason in share-proto).
- Brake `edge_full` holds for 24 h after the last `edge_full` answer (the design says "an `edge_full`"); the
  "abuse spike" is the refusal/new-mesh rate thresholds (the design names no measure).
- Per-name failure cool-down counts every failed attempt of a name's job (not only failed authorizations), in
  memory (reset at restart).
- `reload` reports `limits` as changed when only a start-sized limit changed (that one also appears in
  `restart_required` and is not applied).
- Drain happens in `run` only; `Running::shutdown` (tests) does not drain.

## Not built

- `TODO(210-M4)`: Safe Browsing / Search Console lookups (brake conditions report `holds: false`; watched outside
  the edge per §9.5 prerequisites).
- `TODO(210-M4)`: per-network rate limit of `GET /v1/names/{name}` (M3 list).
- share-proto needs (not changed here): `BrakeStatus` has no field for "pulled automatically at/on"
  (`ops::pulled` has it; shown in logs, audit and `share_edge_brake_pulled`); `KillReason` has no `recovered`;
  `RevokeReason::Unspecified` has no store value.
- deploy: no ServiceMonitor added (no Prometheus operator CRDs known in rwx-dev; the `/metrics` port is already in
  the Deployment).

## Doubts

- Relayed re-count: `meshes.net` is rewritten to the first direct network seen; a mesh that only ever returns
  relayed is never re-counted (by design of the rule).
- Unattached expiry uses `names.last_attach_ms` (stamped at every `hello` and by the renewal scan for live
  tunnels); a mesh attached continuously without re-`hello` for 90 days is protected by the live-tunnel check.
- The brake's `bucket_empty` clock lives in memory: a restart restarts the 7-day count.
- `queue` lists new-name jobs only (renewals/rotations never wait for a slot).

## Verification

Each command under its own `/tmp/theuth-build.lock` hold, after the last code edit:

```sh
CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features --no-fail-fast
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
cargo tree -p share-edge -e normal | grep -E '\(/'
```

Tests added (time injected where it matters): `tests/admin.rs`
`invites_mint_list_revoke_and_admit_once`, `queue_mesh_release_recover_and_revoke_cert`,
`caa_check_status_and_reload`, `the_brake_pulls_invite_on_a_refusal_spike_and_says_why` (injected ms: refusals
window, auto flip, clear days, `/metrics` lines), `edge_full_holds_the_brake_without_auto`,
`standby_ticket_and_retire_promote_the_standby_key` (verified with `EdgePins::accept_retire`),
`drain_refuses_hello_and_returns_at_once_without_tunnels`; unit `ops::tests::windows_slide_and_limit_refusals_only_count`,
`store::ops::tests::invites_recover_and_unattached` (relayed re-home, engine-key day count, recover, unattached).

Not covered by an automated test: the hello paths over iroh for `engine_keys_per_mesh_day`, the relayed re-count,
`terms_required` on CSR, `reserved_min_mesh_age`, drain with live tunnels; `orders_per_3h` and the failure
cool-down through the runner; the daily CAA task.

Results: see below.

## Verification results (all after the last code edit, each its own lock hold, 16:58–17:01)

- `cargo test -p share-edge --all-features --no-fail-fast`: exit 0 — unit 38/38, `admin` 7/7 (new), `certs` 14/14,
  `mesh` 4/4, `visitor` 2/2. One real bug found by `queue_mesh_release_recover_and_revoke_cert` on the first run and
  fixed before this run: `revoke-cert` used `retire_chain`, which only moves chains of released/banned names (new
  `Store::revoke_name_chain`).
- `cargo clippy -p share-edge --all-targets --all-features -- -D warnings`: exit 0, no warnings.
- `cargo clippy -p share-edge --all-targets -- -D warnings`: exit 0, no warnings.
- `cargo tree -p share-edge -e normal` path crates: `share-edge`, `platform`, `share-proto`, `vendor/libsqlite3-sys`.
  No dependency added (`Cargo.toml` unchanged). `Cargo.lock` shows the pre-existing one-line worktree change
  (noted since EDGE-M2), not from this phase; left uncommitted.
