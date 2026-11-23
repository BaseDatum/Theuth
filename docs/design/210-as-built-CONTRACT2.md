# 210 as built — CONTRACT2 phase (`share-proto` for M2, M3, M4)

Design: `docs/design/210-public-share.md` rev 8 + appendix. Extends the contract of
`210-as-built-CONTRACT.md` so M2 (mesh identity), M3 (limits) and M4 (operations) can be built in parallel. Every
wire change is additive: new fields are `#[serde(default)]` (and skipped when empty where the design shows no
field), new enum values sit next to the existing `#[serde(other)] Unknown`, new SSE events are unknown names to an
old client (`Event::parse` → `None`). Old JSON without the new fields is tested to decode for every extended
answer.

**Rust-source compatibility is not kept** (owner decision "break ok", 14:27): structs that `share`/`share-edge`
build with exhaustive literals gained fields, and `Event` gained a variant. The `share-edge` fixes were applied
mechanically here (listed below); the `crates/share` fixes are FOLLOW-A's (orchestrator, 14:41) and committed with
its own work, so the branch may be briefly inconsistent between the two commits.

## Files

| path | change |
|---|---|
| `crates/share-proto/src/{api,error,events,admin,signed,lib,version,ticket}.rs` | the additions below, tests |
| `crates/share-edge/src/control.rs`, `crates/share-edge/tests/visitor.rs` | mechanical literal fixes |
| `crates/share/**` | not edited here: FOLLOW-A owns the matching fixes (see "Downstream edits") |
| `docs/design/210-as-built-CONTRACT2.md` | this file |

No new dependency; `Cargo.lock` unchanged by this phase.

## New public API, by milestone

### M2 — mesh identity, rekey, names, routes

```rust
// api
pub struct NameInfo { …, transfer_to: Option<PublicKey>, transfer_until_ms: Option<u64> }   // pending transfer (D20)
pub struct NameAvailability { …, reclaim_after_ms: Option<u64> }                            // with reason "cooldown"
pub struct RoutesPut { hosts: Vec<String>, published_ms: BTreeMap<String, u64> }            // per-host live_ms
impl RoutesPut { pub fn stopped_hosts(&self, stop_all_ms: u64) -> Vec<String> }             // published_ms <= stop_all
pub struct CsrQuery { rotate: bool, compromised: bool }      // ?rotate=true&compromised=true → keyCompromise (§6.3)
pub struct MeshStatus { …, revoked: Vec<PublicKey>, rekeys: Vec<RekeyEntry> }               // GET /v1/mesh
pub struct RekeyEntry { old: PublicKey, new: PublicKey, ms: u64 }                           // applied rekey, oldest first

// paths (lib.rs)
pub fn paths::name_csr_query(name: &str, q: &CsrQuery) -> String;

// error
ErrorCode::StoppedAll          // "stopped_all", 409: RoutesPut hosts published at/before stop_all; nothing applied
ErrorCode::ProtocolMismatch    // "protocol_mismatch", 426: Hello.v not served (client_too_old stays min_client)
pub struct ErrorBody { …, supported: Vec<u32>, stop_all_ms: Option<u64>, hosts: Vec<String> }
impl ErrorBody { pub fn stopped_all(stop_all_ms: u64, hosts: Vec<String>) -> Self;
                 pub fn protocol_mismatch(supported: Vec<u32>) -> Self }

// version
pub const SUPPORTED_MAJORS: &[u32] = &[PROTOCOL_MAJOR];   // parallel to ALPNS, newest first
pub fn version::protocol_ok(v: u32) -> bool;

// signed
impl Rekey { pub fn new(domain, edge, old, new, revoke: Vec<PublicKey>, issued_ms: u64) -> Self;
             pub fn chain(&self, to: PublicKey) -> Rekey }        // heal rule: old = to, same new/revoke/issued
pub enum RekeyConflict { Confirming, Chain { from: PublicKey }, Contested }
pub fn rekey_conflict(to: &PublicKey, settled: &PublicKey, stash: &[PublicKey]) -> RekeyConflict;  // §5.7 `sending`

// limits
REKEY_SETTLE_SECS = 60; REKEY_HEAL_SECS = 3_600; REKEY_BACKOFF_MIN_MS = 60_000; REKEY_BACKOFF_MAX_MS = 3_600_000;
TRANSFER_TTL_MS = 7 d; ALLOCATED_TTL_MS = 1 h
```

Already on the branch and reused, not duplicated: reserve = `PUT paths::NAME` → `NameGranted`; release =
`DELETE paths::NAME` → `NameReleased`; rotate = `POST paths::NAME_CSR?rotate=true`; transfer =
`POST paths::NAME_TRANSFER` `TransferRequest` → `TransferAccepted`, claimed by the receiving mesh with the plain
`PUT`; `POST paths::MESH_REVOKE` `RevokeRequest` → `RevokeResult`; `POST paths::MESH_REKEY` `SignedRekey` →
`RekeyAccepted` / `409 mesh_rekeyed {to}`; `csr_mismatch {spki}` (`ErrorBody::csr_mismatch`); multi-engine routes
(`welcome.routes` / `MeshStatus.routes` are `RouteEntry {host, engine}`, the owner engine per host;
`route_conflict.conflicts` names the holder engine per host).

Rekey semantics the edge must apply (doc comments on `Rekey`/`MeshStatus`): `revoke` **replaces** the mesh's
revoke list; everything of `old` moves to `new`; `old` gets `rekeyed_to`; a `RekeyEntry` is appended. The
client's `settling` waits `synced()` + `REKEY_SETTLE_SECS`; `confirming` polls `GET /v1/mesh` until `mesh ==` the
settled key.

### M3 — slot queue, limits, metering

```rust
// error
pub enum LimitScope { Mesh, Engine, Tunnel, Net, NetWide, Net32, Asn, Relayed, Global, Unknown }
pub enum LimitKind  { Names, Reserved, Claims, Certs, Queued, Meshes, Engines, Tunnels, Routes, Bytes,
                      NamesMax, QueueMax, Unknown }
pub struct ErrorBody { …, scope: Option<LimitScope>, limit: Option<LimitKind> }
impl ErrorBody { pub fn limited(code: ErrorCode, scope: LimitScope, limit: LimitKind) -> Self;
                 pub fn edge_full(limit: LimitKind) -> Self }     // edge_full, scope global, 503

// events
pub struct StatsEvent { host, conns, bytes, bytes_in: u64, bytes_out: u64, visitors: u64, open: u64,
                        period_s: u64 }                            // new fields serde(default); derives Default
pub struct ThrottleEvent { throttled: bool, mbit: u32 }            // SSE "throttle"
Event::Throttle(ThrottleEvent)
KillReason::Paused, KillReason::Flagged                            // admin pause; Safe Browsing / Search Console hit

// api
pub struct WelcomeLimits { …, tunnel_mbit: u32 /*50*/, throttle_mbit: u32 /*2*/ }
pub struct Welcome { …, throttled: bool }                          // egress throttle at hello

// lib.rs
pub fn queue_eta_s(position: u32, tokens: u32, refill_s: u64, next_token_s: u64) -> u64;   // §6.5
limits::STATS_PERIOD_S = 30; limits::ORDER_ETA_S = 180
```

Already present and reused: `QueuePos {position, eta_s}` in `NameInfo.queue`; `CertPending {eta_s, queue_pos}`
(202 of the CSR post); `QueueEvent {name, position, eta_s}` (SSE `queue`); `ErrorCode::{EdgeFull, NetworkQuota,
Quota, EnginesQuota, Paced}`; `Info.full`/`Welcome.full`.

### M4 — admission, operations, pinned keys

```rust
// admin: new commands (tag "cmd", kebab-case)
AdminCommand::InviteNew { uses, expires_s, note: Option<String> }   // `note` new
AdminCommand::InviteList                                            // read-only
AdminCommand::InviteRevoke { hash: String }                         // hex of Invite::hash, or ≥ 8-char prefix
AdminCommand::RevokeCert { name, reason: Option<RevokeReason> }     // `reason` new
pub enum RevokeReason { Unspecified /*default*/, KeyCompromise, Superseded, CessationOfOperation }
impl AdminResponse { pub fn ok_typed<T: Serialize>(id: u64, result: &T) -> Self;
                     pub fn result_as<T: DeserializeOwned>(&self) -> Option<Result<T, serde_json::Error>> }

// admin: typed results, one per command
ClosedTunnels { closed_tunnels: u64 }                                   // ban, pause
AdmissionResult { admission }                                           // admission
StatusResult { version, domain, edge_id, admission, paused, tunnels, routes, visitors,     // = M1 edge's json
               full, names_slotted, names_max, queued, queue_max, throttled,               // serde(default)
               egress_month_bytes, egress_pace_bytes, standby: Option<PublicKey> }
QueueResult { invited_tokens: u32, open_tokens: u32, next_token_s: u64, jobs: Vec<QueueRow> }
QueueRow { name, mesh, class: QueueClass, kind: JobKind, position: u32, eta_s, first_of_mesh, net, enqueued_ms, attempts: u32 }
QueueClass { Invited, Open }; JobKind { New, Renew, Rotate }
MeshResult { mesh, invited, net, asn: Option<u32>, created_ms, names: Vec<NameInfo>, engines: Vec<MeshEngine>,
             revoked, rekeys: Vec<RekeyEntry>, rekeyed_to, stop_all_ms, banned: Option<BanInfo> }
BanInfo { reason, ms, by }
ReleaseResult { name, reclaim_after_ms, closed_routes }
EvictResult { evicted: Vec<String> }
RecoverResult { from_mesh, to_mesh, names: Vec<String>, closed_tunnels }
InviteMinted { ticket: ShareEdgeTicket, hash: String, uses: u32, expires_ms }
InviteRow { hash, uses_left: u32, expires_ms, created_ms, note }; InviteListResult { invites }; InviteRevoked { hash }
CaaCheckResult { ok, found: Vec<String>, missing: Vec<String>, admission, checked_ms }
RevokeCertResult { name, reason: RevokeReason }
ReloadResult { changed: Vec<String>, restart_required: Vec<String> }
BrakeStatus { admission, pull: bool, conditions: Vec<BrakeCondition>, clear_days: Option<u32> }
BrakeCondition { kind: BrakeKind, holds: bool, detail: Option<String> }
BrakeKind { EdgeFull, QueueEta, BucketEmpty, SearchConsole, SafeBrowsing, AbuseSpike, Unknown }

// ticket
impl Invite { pub fn hash_hex(&self) -> String }                    // the operator-facing invite id

// signed: the two pinned default-edge keys (§5.5, D15)
pub struct EdgePins { live: PublicKey, standby: Option<PublicKey>, retired: Vec<PublicKey> }   // serde
impl EdgePins { pub fn new(live, standby: Option<PublicKey>) -> Self;
                pub fn dial_order(&self) -> Vec<PublicKey>;          // live, standby; never retired
                pub fn accept_retire(&mut self, signed: &Signed, connected: &PublicKey) -> Result<Retire, RetireError> }
RetireError::NotPinned                                               // verified, but not standby retiring live
limits::STANDBY_AFTER_MS = 10 min

// lib.rs re-exports added
admin::{AdminCommand, AdminRequest, AdminResponse, RevokeReason}, error::{LimitKind, LimitScope},
events::ThrottleEvent, signed::{EdgePins, RekeyConflict, RetireError, rekey_conflict}, version::SUPPORTED_MAJORS
```

`terms_rev` in `hello`: verified complete from CONTRACT — `Info.terms_rev`, `Hello.terms_rev: Option<String>`,
`Welcome.{terms_rev, terms_ok}`, `ErrorCode::TermsRequired`. Nothing added.

## Downstream edits (mechanical, behaviour unchanged)

| file | edit |
|---|---|
| `crates/share-edge/src/control.rs` `welcome` | `throttled: false` + `TODO(210-M3)` |
| `crates/share-edge/src/control.rs` `name_info` | `transfer_to: None, transfer_until_ms: None` + `TODO(210-M2)` |
| `crates/share-edge/src/control.rs` `availability` (4 literals) | `reclaim_after_ms: None` |
| `crates/share-edge/src/control.rs` `mesh` | `revoked: Vec::new(), rekeys: Vec::new()` + `TODO(210-M2)` |
| `crates/share-edge/tests/visitor.rs` (3 `RoutesPut`) | `..Default::default()` |
| `crates/share/**` (FOLLOW-A, not this phase) | `RoutesPut` with empty `published_ms` + `TODO(210-M2)`; `Event::Throttle` ignored; test `Welcome`/`NameInfo` literals at defaults |

## Deviations from the design, and why

- **`stopped_all` (409)** instead of FIX-EDGE's `403 hello_required` for a route published before Stop all: a
  dedicated code lets the client stop just the named shares (`hosts`) instead of re-hello. The edge's existing
  session-level `hello_ms` rule stays; `published_ms` adds the per-share check.
- **`protocol_mismatch` (426)**, a separate code for `Hello.v` (FIX-EDGE M11 used `client_too_old` for both).
- **ETA formula**: §6.5 writes `max(0, p − t) × r + next_token`, which counts the next token twice when
  `p > t` and adds it when `p ≤ t`; `queue_eta_s` uses `(p − t − 1) × r + next_token` when short, `0` otherwise,
  plus 3 min per job ahead.
- **Primary name** has no wire type: the design keeps it in the replicated `share_names.is_primary` (§5.2), never
  at the edge.
- **Throttle state** travels in `Welcome.throttled` + `WelcomeLimits.{tunnel_mbit, throttle_mbit}` and live as
  the new `throttle` event, not in `GET /v1/mesh`. `StatusResult.throttled` is the operator view.
- **`MeshStatus.rekeys`** is the "pending rekey" view: the edge applies a rekey atomically, so there is no pending
  state at the edge; the client's `confirming` compares `mesh` with its settled key.
- **`EdgePins`** lives in `share-proto` (CONTRACT placed the two-pinned-keys type in `crates/share` config): it
  holds the retire-acceptance rule, which must match `Retire::verify`.
- **`KillReason::{Paused, Flagged}`** are additions not in §5.3's list (§9.3 pause, §9.5 Safe Browsing kill).
- **Invite id** = lowercase hex of `Invite::hash` (the `invites.hash` column).

## Not built

- `TODO(210-M2)`: edge — fill `NameInfo.transfer_*`, `MeshStatus.{revoked, rekeys}`, apply `published_ms` /
  `stopped_all`, answer `protocol_mismatch`; client — send `published_ms`, use `rekey_conflict` and
  `Rekey::chain`, `compromised` on post-removal rotations.
- `TODO(210-M3)`: edge — `ErrorBody::limited` scopes, `queue_eta_s`, the new `StatsEvent` fields, `throttle`
  events, `Welcome.throttled`; client — `throttle`/`stats`/`queue` UI.
- `TODO(210-M4)`: edge admin — the typed results for queue, mesh, release, evict-unpublished, recover, invite
  new/list/revoke, caa-check, revoke-cert, reload, brake-status (the dispatcher still answers `invalid … not
  available` for them); CLI mapping of the new commands; client — `EdgePins` from the shipped default tickets.
- Golden JSON fixture files (§5.3) are still not built; the round-trip tests are inline.
- `Handover` (P2).

## Doubts

- `ErrorBody` is now a wide bag of optional fields; per-code helpers keep construction honest but nothing stops a
  body with fields of two codes.
- `EdgePins::accept_retire` re-accepts an already-applied notice (idempotent) but refuses a retire of a key that
  is neither live nor already retired; a standby that retires a third key is `NotPinned`.
- `StatsEvent.bytes` stays the sum for old clients; an M3 edge must keep `bytes = bytes_in + bytes_out`.
- `CsrQuery.compromised` without `rotate` is ignored by `name_csr_query`; the edge should do the same.

## Verification

Each under its own `/tmp/theuth-build.lock` hold, after the last edit:

- `cargo test -p share-proto`: 33 passed, 0 failed (new or extended: `error::new_codes_and_fields`,
  `events::events_round_trip` extended (old `stats` JSON, `throttle`, `paused`/`flagged`), `admin::m4_commands_and_typed_results`
  (M1 `status` JSON decodes as `StatusResult`), `signed::rekey_chain_heal_and_conflict_rule` (chain sign/verify,
  forged and tampered body), `signed::edge_pins_accept_only_the_standby_retiring_live` (stranger, wrong key,
  tampered sig, idempotent re-delivery, old JSON), `api::{routes_put_published_ms_and_old_json,
  mesh_status_rekeys_and_old_json, welcome_and_names_old_json, csr_query_and_paths, queue_eta_rule}`).
- `cargo clippy -p share-proto --all-targets -- -D warnings`: clean.
- `cargo check -p share-edge --all-targets`: clean (with the share-edge fixes above).
- `cargo check -p share`: clean, against the working tree that contains FOLLOW-A's uncommitted `crates/share`
  fixes (`edge.rs`, `edge/tests.rs`, `engine.rs`); without them `crates/share` does not compile until FOLLOW-A
  commits.
