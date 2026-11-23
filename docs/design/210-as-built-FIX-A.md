# 210 as built — FIX-A (branch review fixes in `crates/share` engine/RPC/tools/edge/store, `crates/tools` approval)

Design: `docs/design/210-public-share.md` rev 8. Seams: CLIENT1, CONSENT1, WIRE1 as-built notes.

## Files

| path | what |
|---|---|
| `crates/tools/src/approval.rs`, `lib.rs` | `ask_human_only_on(&ActorRef<Approval>, RequestApproval)`, `ask_human_only(RequestApproval)` → `anyhow::Result<Approved>`; test |
| `crates/share/src/engine.rs` | `Consent`, `StopAllOutcome`, `name_host_of`; publish/stop/Stop all/cert/restore changes; tests |
| `crates/share/src/rpc_share.rs` | approval rows on the RPC path; `Stopped.edge_pending`; `share_password` |
| `crates/share/src/tools.rs` | `share_publish` refuses without a row; `share_stop all` reports a pending edge stop |
| `crates/share/src/edge.rs`, `edge/tests.rs` | bounded, line-decoded `SseParser`; `Incompatible`, `compat()`; tests |
| `crates/share/src/store.rs` | `delete_secret`, `put_request`/`request_expiry`/`delete_request`, `stop_all_at`/`stop_all_ms`/`stop_all_pending`/`stop_all_sent` |
| `crates/share/migrations/0002_share_requests.sql` | local `share_requests (share, expiry_ms)` |

## Public API changes

```rust
// crates/tools
pub async fn ask_human_only_on(approval: &ActorRef<Approval>, req: RequestApproval) -> anyhow::Result<Approved>;
pub async fn ask_human_only(req: RequestApproval) -> anyhow::Result<Approved>; // on APPROVAL_ACTOR

// crates/share::engine
pub struct Consent(/* private ApprovedBy */);
impl Consent { pub fn of_call() -> Option<Self>; pub async fn ask(what: &str, args: Value) -> anyhow::Result<Self>; pub fn approved_by(&self) -> &ApprovedBy }
pub enum CallOrigin { Call { session, call_id }, Settings }            // `approved` field removed
Engine::publish(plan, origin, consent: Consent)
Engine::enable(&Consent) / Engine::disable()                            // was set_enabled(bool)
Engine::set_edge(&EdgeChoice, &Consent); Engine::accept_terms(edge, rev, &Consent)
Engine::stop_all() -> anyhow::Result<StopAllOutcome { stopped, edge_pending: Option<String> }>
Engine::password(id) -> Option<String>
pub fn name_host_of(host, domain) -> String

// crates/share::edge
pub enum Incompatible { Protocol { edge, ours }, ClientTooOld { min } }
pub fn compat(v, min_client, me) -> Result<(), Incompatible>
pub const SSE_MAX_LINE: usize; pub const SSE_MAX_DATA: usize;

// RPC (owner audience)
Stopped { stopped, edge_pending: Option<String> }                      // new field, serde default
share_password { id } -> Option<String>                                // new method
```

## Findings

- **B1 fixed.** `share_publish`, `share_settings_set` (when `enabled: true` or `edge` is given) and
  `share_terms_accept` call `Consent::ask`, which raises `RequestHumanOnly` on the approval actor
  (`tools::ask_human_only`) and waits for the owner's `Allow`; deny/timeout/dismiss/no actor is an RPC error and
  nothing happens. `Engine::publish`, `enable`, `set_edge`, `accept_terms` take a `Consent`, whose only public
  constructors are `of_call()` (the registry's task-local `Approved`) and `ask()`; `from_approved` refuses a nil id
  or a non-`owner` surface, and `publish` re-checks a non-empty `approval_id`. Skipping the ask no longer compiles.
  Switch-off (`disable`) and stop stay ungated; `enabled: false` is applied before any ask.
  Tests: `tools::approval::tests::ask_human_only_waits_for_the_owner`, `engine::tests::consent_needs_an_owner_row`.
- **B2 fixed.** `ensure_cert(link, name, renew)`: `Event::Cert` → `after_cert(name, renew = true)` fetches the chain
  and installs over the old one. The engine records the welcome's `NameInfo` (`not_after`, `spki`) per installed
  `<name>.D`; at a welcome a differing `not_after`/SPKI re-fetches. `Certs::remove` runs when the welcome lists the
  name `released`/`banned` and when the last non-stopped share under `<name>.D` stops. No unit test (needs a fake
  edge `Control`; see doubts).
- **M7 fixed.** `stop_all` records `stop_all_ms` (and a pending mark) in `settings` first, stops every non-stopped
  local share created before it (pending ones too), continues past per-share errors (first error returned after the
  rest), then `POST /v1/mesh/stop` with 3 tries (0 / 0.5 s / 2 s). Detached or refused → `edge_pending: Some(why)`
  in RPC `Stopped` and in the tool text; the mark is sent again at the next `welcome`. `after_cert` never promotes
  a share created at or before the local Stop all (stops it). The welcome's `stop_all_ms` stops shares by
  `created_ms` (was `live_ms`, which skipped pending ones). Test: `stop_all_stops_pending_and_reports_the_edge_pending`.
- **M8 fixed.** The approved lifetime of a pending share is stored in the new local table `share_requests`
  (migration 0002) and used when it goes live (`go_live`); deleted on promotion and stop. Test:
  `pending_share_keeps_the_approved_expiry`.
- **M9 fixed (engine half).** `restore()` runs at `start()` and at every `welcome`: live/offline shares of this
  engine are mounted again and password shares get their hash from `Store::secret`; one whose hash is gone is
  stopped. A pending password share gets its hash at promotion. The generated password is kept in memory per run
  and served to the owner by RPC `share_password {id}` (and still in the RPC `Published` result); the tool output
  says only that the user sees it in Settings › Sharing. Test:
  `restore_remounts_live_shares_and_stops_hashless_password_shares`.
- **M11 (client half) fixed.** `edge::compat` checks `v == PROTOCOL_MAJOR` and `min_client` for both `info` and
  `welcome`; a mismatch is `Next::Blocked(Incompatible::to_string())`, i.e. `share.status` `blocked { reason:
  "protocol_mismatch: …" | "client_too_old: …" }`. Test: `protocol_versions_are_checked`.
- **m1 (client half) fixed.** `SseParser` buffers bytes, splits on `\n` before UTF-8 decoding (a split multibyte
  char survives), drops a line over 64 KiB whole and an event whose `data` exceeds `limits::MAX_JSON_BODY`.
  Test: `sse_parser_keeps_split_multibyte_and_caps_sizes`.
- **m6 fixed.** The password hash is computed first but stored only after `ensure_cert`, immediately before the
  share row; a failed row write deletes it. `max_live` is admitted under `admit` (tokio mutex) counting stored
  shares + in-flight reservations; a reservation is released (under the same mutex) only after the row is stored.
  Stop also deletes the secret. Test: `max_live_reservations_are_counted`.
- **m7 fixed.** `SharePublishTool::call` refuses when `Consent::of_call()` is `None`.

## Deviations

- **Requested expiry is not a `shares` column** but a local table `share_requests`: `shares` is replicated and
  only the publishing engine ever promotes its pending share, so a replicated column (and a replica schema change)
  buys nothing.
- **Password to the owner** goes through a new owner-audience RPC `share_password` (memory only, lost on restart),
  not through the approval card (the card is built before the password exists) and not through an event.
- **RPC-raised rows use session `00000000-…` (nil)**: there is no session for a Settings/card action.

## Not built / for other owners

- `TODO(210-M1)` UI: desktop/web must call `share_password` after a tool-path password publish, and render
  `Stopped.edge_pending`. Wire: `share_password` and `Stopped.edge_pending` change the OpenRPC snapshot
  (`crates/theuth/tests/fixtures/wire/openrpc-v1.json`) — needs `just wire-schema-accept` by its owner.
- Nothing needed in FIX-SHARE-B's files (`gate::set_password`, `routes::Table::insert`, `visitor::Certs::remove`
  were already public).

## Doubts

- B2 renewal and the welcome-driven re-fetch are not unit-tested: `Control` needs the fake edge of `edge/tests.rs`
  plus a `NameKeyStore`; time did not allow a full fake.
- RPC handlers now block until the owner answers (approval timeout); clients must not use a short RPC timeout.
- The RPC-token residual of CONSENT1 still applies: a holder of the token can also call `resolve_approval`.
- `share_password` is readable by anything holding the owner RPC token (same residual).

## Verification

(see below)

Each command under its own build-lock hold, macOS, after the last code edit:

- `cargo test -p share`: 31 of 32 passed; `gate::tests::interstitial_then_password` (FIX-SHARE-B's `gate.rs`, edited
  in the tree at the same time, uncommitted) hung at 0 % CPU for over 13 min and was killed. Rerun
  `cargo test -p share -- --skip gate::tests::interstitial_then_password`: **31 passed, 0 failed** (all FIX-A tests
  included). The hang is not in code this phase touched; FIX-SHARE-B should look at it.
- `cargo clippy -p share --all-targets -- -D warnings`: clean.
- `cargo test -p tools --lib -- --skip git::`: **298 passed**, 0 failed (incl. `ask_human_only_waits_for_the_owner`).
- `cargo clippy -p tools --all-targets -- -D warnings`: clean.
- Not run: `rpc-compose`/`theuth` wire golden and OpenRPC snapshot (expected to need regenerating, see above).
- `Cargo.lock`: no change from this phase (no new packages).
