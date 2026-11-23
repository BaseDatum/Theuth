# 210 as built — EDGE2 phase (`share-edge` certificates, Path Z)

Design: `docs/design/210-public-share.md` rev 8 §6.0–§6.5, §8.1–§8.5, D7, D17; appendix §6.1. Builds on EDGE1
(`docs/design/210-as-built-EDGE1.md`). Let's Encrypt over `dns-01`, records through `zone-update`'s Porkbun
client. No network was used by any test (fake zone, fake CA, fake CAA).

## Files

| path | what |
|---|---|
| `crates/share-edge/Cargo.toml` | + `instant-acme`, `zone-update` (workspace pins), `backon` 1, `x509-parser`, `time`, `uuid`, `hickory-resolver` 0.26; dev + `rcgen` (`x509-parser`, for the fake CA) |
| `src/dns.rs` | `DnsZone` trait, `PorkbunZone`, checked `ensure`/`remove`, `contain` (panic boundary), host helpers |
| `src/acme.rs` | `Ca`, `Validator`, `CaaLookup` traits; `AcmeCa` (instant-acme); account file; `register`; CAA lines and check; `SystemCaa` (hickory) |
| `src/certs.rs` | `Backends`, `Certs`, `Dns01` validator, CSR intake, `cert_jobs` runner, renewal scan, `retire`, `reconcile` |
| `src/store/certs.rs` | `CertRow`, `JobRow`; job lease/requeue/finish, chain/ARI columns, `dns_records` ledger, `buckets` token bucket |
| `src/config.rs` | `acme.{profile, renew_check, job_lease, max_attempts, propagation, auth_gap}`, `acme.cas.letsencrypt.{directory, new_certs_per_week}`, `acme.cas.gts.{directory, validation}`; `dns_zone()`, `dns_target()`, `account_path()`; refuses `require_caa: false` with `admission: open` |
| `src/state.rs` | `Edge::certs: OnceLock<Certs>` |
| `src/control.rs` | `POST …/csr`, `GET …/cert` filled; release spawns `certs::retire` |
| `src/admin.rs` | `ban name` spawns `certs::retire` |
| `src/lib.rs` | `start` builds the production backends; new `start_with(cfg, secrets, Backends)`; CAA gate; runner task |
| `src/main.rs` | `acme register [--ca letsencrypt|gts]` |
| `tests/common/mod.rs` | `FakeZone`, `FakeCa` (drives the validator, signs the CSR with rcgen), `FakeCaa` |
| `tests/certs.rs` | 7 tests (below) |
| `tests/visitor.rs` | now `start_with` the fakes (the CAA gate would otherwise resolve DNS) |
| `deploy/share-edge/README.md`, `config.example.yaml` | certificate section; timing keys |

No new migration: EDGE1's `0001_init.sql` already has `cert_jobs`, `buckets`, `dns_records` and the `names`
certificate columns.

## Public API added

```rust
share_edge::start_with(cfg: Config, secrets: Secrets, backends: certs::Backends) -> Result<Running>

dns::{Rtype { A, Cname, Txt }, DnsZone, PorkbunZone, contain, ensure, remove, relative, name_hosts,
      challenge_host, panic_text}
trait DnsZone: Send + Sync + Debug {
    fn get(&self, Rtype, host: String) -> BoxFuture<Result<Option<String>>>;   // error on >1 record
    fn create(&self, Rtype, host: String, value: String) -> BoxFuture<Result<()>>;
    fn delete_all(&self, Rtype, host: String) -> BoxFuture<Result<()>>;
}
dns::ensure(zone, t, host, value)   // get; clear a different/multiple value; create; read back
dns::remove(zone, t, host)          // delete_all; read back none
dns::contain(fut)                   // catch_unwind → "DNS provider panicked: …"

acme::{Ca, Validator, CaaLookup, AcmeCa, SystemCaa, Reason, Window, Caa, LeafInfo,
       leaf_der, leaf_info, caa_lines, caa_ok, parse_caa_text, require_caa, register, load_cas,
       issuer_domain, validation_method}
trait Ca { fn id(&self) -> &str; fn account_uri(&self) -> &str;
           fn issue(&self, names: &[String; 2], csr: &[u8], replaces: Option<&str>, v: &dyn Validator)
               -> BoxFuture<Result<String /* PEM chain */>>;
           fn renewal_window(&self, chain_pem: &str) -> BoxFuture<Option<Window>>;
           fn revoke(&self, chain_pem: &str, reason: Reason) -> BoxFuture<Result<()>>; }
trait Validator { fn challenge(&self) -> &'static str; fn parallel(&self) -> bool;
                  fn prepare(&self, ident: &str, value: &str) -> BoxFuture<Result<()>>;
                  fn cleanup(&self, ident: &str) -> BoxFuture<Result<()>>; }
trait CaaLookup { fn caa(&self, domain: &str) -> BoxFuture<Result<Vec<Caa>>>; }

certs::{Backends { zone, ca, caa }, Certs, Dns01, Submitted { Chain(CertChain), Pending(CertPending) },
        submit, chain, retire, reconcile, tick, run, NEW_CERT_BUCKET, NO_RENEW_AFTER_MS}
store::{CertRow, JobRow} + Store::{cert_row, valid_names, set_csr, set_name_state, set_records, store_chain,
        set_ari, clear_chain, open_job, jobs, enqueue_job, lease_job, finish_job, requeue_job, drop_jobs,
        reset_leases, queue_position, dns_add, dns_del, dns_all, bucket_take}
```

The traits return `BoxFuture` instead of the design's `async fn` so they are object-safe (`Arc<dyn Ca>`); the
methods are the design's (`issue`, `renewal_window`, `revoke`; `challenge`, `parallel`, `prepare`, `cleanup`),
plus `Ca::id`/`account_uri` for the CAA check. `DnsZone` is generic get/create/delete over A/CNAME/TXT; the
design's `ensure_cname`/`delete_name`/`set_txt`/`clear_txt` are `dns::ensure`/`dns::remove` calls in
`certs.rs`.

## Behaviour

- **CSR intake** (`POST /v1/names/{name}/csr[?rotate=true]`, body DER): session; name exists, not banned
  (`banned`), held by the session's mesh (`not_owner`); `share_proto::check_csr` (`invalid` / `csr_names` with
  `reason`); then: same CSR as stored → `200 {chain_pem, not_after}` if a chain exists and no job is open, else
  `202`; a stored different CSR without `rotate`, or with `rotate` while a job is `ordering` →
  `409 csr_mismatch {spki}` (the stored SPKI); with `rotate` → CSR replaced, job `rotate` (or `new` if never
  issued); no stored CSR → stored, name `queued`, job `new`. `202 {eta_s, queue_pos}`: `queue_pos` only for a
  `new` job without a slot, `eta_s = position × 180`.
- **`GET /v1/names/{name}/cert`**: held by the mesh → `200 CertChain`, no chain → `404 not_found`.
- **Welcome**: unchanged code; it already reads `state`, `ca`, `csr_spki`, `not_after_ms`, which the runner now
  fills (`valid` once issued).
- **Runner** (`certs::run`, one task): `reconcile` once, then `tick(now)` on every wake (a CSR) and every 60 s.
  `tick` runs the renewal scan every `acme.renew_check`, then leases due jobs one by one (`queued` and due, or
  `ordering` with a lapsed lease; never while another job of the same name holds a live lease). Per job: name
  still held by the job's mesh (else `dropped`); CSR present and passing `check_csr` again (else
  `dropped`/`failed`); a `new` job of an unslotted name takes a token from bucket `le_new` (capacity and weekly
  refill = `new_certs_per_week`), or is requeued at the next token time without counting an attempt, the name
  `queued`, a `queue {name, position, eta_s}` event to the mesh's tunnels; with a token the name is `slotted`;
  then the two D17 CNAMEs if `names.records = 0` (ledger row first, `ensure` with read-back, `records = 1`);
  then `Ca::issue` inside `contain`; the leaf's SPKI must equal the CSR's; ARI window fetched; chain, `ca`,
  `not_after_ms`, ARI stored, name `valid`, job `done`, `cert {name}` to every attached tunnel of the mesh; a
  `rotate` revokes the previous chain (`superseded`). An error (including a contained panic) deletes the
  challenge TXT and requeues with backon's exponential delay (1 min doubling to 24 h, jitter); after
  `acme.max_attempts` the job is `failed`.
- **`AcmeCa::issue`**: `NewOrder` for both identifiers with `profile` and, for renew/rotate, `replaces` (the old
  leaf's `CertificateIdentifier`); authorizations strictly one at a time: pick the first `pending`, `prepare`
  (TXT), `set_ready`, poll that authorization (re-fetched through `Order::refresh`) to `valid` (3 min cap),
  `cleanup`; then `poll_ready`, `finalize_csr` with the engine's CSR, `poll_certificate`.
- **`Dns01`**: `prepare` waits until `auth_gap` after the previous `cleanup`, writes the ledger row and the TXT at
  `_acme-challenge.<n>` (read back), waits `propagation`; `cleanup` deletes it (read back) and the ledger row.
  `parallel() = false`.
- **Renewal scan**: for `valid` names with a chain and an owner, attached within 90 days, no open job: refresh the
  ARI window (`Account::renewal_info`), store it; due when `now ≥ window start`, else (no window) at
  `not_before + ⅔ lifetime`; enqueue `renew` (no bucket token).
- **Retire** (release, `admin ban name`, spawned): drop open jobs, revoke the chain
  (`cessationOfOperation`), clear it, delete both CNAMEs and the challenge TXT (read back), ledger rows,
  `records = 0`. Errors are logged.
- **Reconcile** (start): `ordering` jobs back to `queued`; every ledger row is deleted from the zone (and the
  ledger) unless it is a CNAME of a held name with `records = 1`; challenge TXTs always go.
- **Panic containment**: `dns::contain` wraps every `DnsZone` call and the whole `Ca::issue` future in
  `catch_unwind`; `PorkbunZone` additionally runs each zone-update call in `spawn_blocking` and maps a panicking
  `JoinError` to an error.
- **Porkbun credential**: re-read from the file/env on every call; JSON `{key, secret}` or
  `{apikey, secretapikey}`; `PorkbunZone::new` fails early if unreadable.
- **`acme register`**: loads an existing `acme/<ca>.json` (never overwrites) or creates the account
  (`terms_of_service_agreed`, `acme.contact`; EAB from the GTS secret `{kid, hmac_key}` for `--ca gts`), writes
  the credentials 0600 (`platform::files::create_private`, dir `private_dir`), prints the account URI and
  `D. CAA 0 issue "letsencrypt.org; accounturi=<uri>; validationmethods=dns-01"` plus the `issuewild` line.
- **`run`**: refuses without the DNS credential, with a provider other than `porkbun`, or without the account file;
  with `require_caa` refuses until `D`'s CAA set has `issue` and `issuewild` for the issuer with this
  `accounturi` and the method (`SystemCaa`: hickory's system resolver). Config refuses `require_caa: false` with
  `admission: open` (appendix §8.1 comment).

## Deviations from the design, and why

- **Records are written when a job gets its slot** (design §6.5 "A job that gets a slot … writes its records";
  §6.4 "before its first order"), not when the name is claimed: an allocated name without a CSR never costs
  Porkbun records. If the brief's "claim time" meant `PUT/POST /v1/names`, that is the difference.
- **No authoritative-server poll** after writing the TXT: a fixed `acme.propagation` wait (30 s) instead
  (`TODO(210-M1)`: query Porkbun's NS for the value; P0-2 measures it).
- **CAA `require_caa` without DNS at `run`** is impossible; tests and `tests/visitor.rs` use `start_with` with a
  fake resolver. `require_caa: false` with `admission: open` is now a config error (it was accepted in EDGE1).
- **`start` now requires the DNS credential and an ACME account** (EDGE1 started without them). An edge without
  certificates has no use in P1; a `Certs`-less `Edge` (built by hand) still answers `501` on the CSR endpoint.
- **TTL**: zone-update sends 300 s; the design plans 600 s (`TODO(210-M1)` in `dns.rs`, P0-1 decides).
- **Ledger instead of a zone listing**: zone-update cannot list records, so reconciliation covers what the
  `dns_records` ledger names (written before every create). A record created by hand or by another tool is never
  touched.
- **`Validator::parallel`** is reported but `AcmeCa` always serializes (Path P is P2).
- **Rotation passes `replaces`** too (same identifier set). Let's Encrypt may refuse `replaces` for a certificate
  already replaced; then the job retries and fails. [INFERENCE: not observed; no CA was contacted.]
- **ETA** is `position × 180 s`; the §6.5 formula with token refill and class order is M3.

## Not built

- `TODO(210-M3)`: the invited/open bucket split (`invite_share`), per-mesh and per-network certificate budgets,
  `orders_per_3h`, the class order and round-robin, the 24 h spill, `queue_max`/`edge_full`, the failure
  cool-down per name, `welcome.names[].queue`, inactivity expiry (release + revoke after 90 days unattached;
  today such names simply stop renewing).
- `TODO(210-M4)`: daily CAA re-check flipping admission to `closed`; `admin queue`, `caa-check`, `revoke-cert`;
  `keyCompromise` on an abuse report.
- `TODO(210-M2)`: `keyCompromise` for rotations after a compromised removal; transfer's revocation.
- `TODO(210-P2)`: GTS as a second `Ca` with a `dns-persist-01` validator (`load_cas` loads only
  `acme.primary`); `register --ca gts` already creates an EAB account.
- Mesh bans do not revoke the mesh's names' certificates (only `ban name` does).

## Doubts

- `AcmeCa` is compiled but never run against a CA (no Pebble here): the per-authorization poll relies on
  `Order::refresh` dropping the cached authorization states (it deserializes `Authorization { state: None }`,
  read in instant-acme 0.8.5 source). First check in P0-2 or against Pebble.
- `SystemCaa` formats hickory's `CAA` rdata with `Display` and re-parses `flags tag "value"`; not exercised
  against a live answer.
- `retire` runs in a spawned task: a crash between release and the revoke loses the revocation (the records are
  still reconciled at the next start). A durable `revoke` job kind would fix it.
- A previous owner re-claiming a name while its `retire` still runs could have fresh records deleted.
- `instant-acme` with `hyper-rustls` installs its own rustls config; `main` does not install a process-wide
  aws-lc-rs provider (EDGE1 did not either).

## Verification

Commands, each under the machine-wide mkdir lock:

```sh
CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets -- -D warnings
CARGO_BUILD_JOBS=6 cargo test -p share-edge
cargo tree -p share-edge -e normal | grep -E '\(/'      # path crates only
```

- `cargo clippy -p share-edge --all-targets -- -D warnings`: **clean** (second run, after replacing
  `format!`-built SQL with `concat!` literals — sqlx rejects non-`'static` SQL — and mapping the
  `CertificateIdentifier` error). This compiles the lib, the bin and both integration tests.
- `cargo test -p share-edge`: **unverified: lock** (queued > 25 min behind other sessions' workspace test runs;
  cancelled per the house rule). The tests compile (clippy `--all-targets` above) but were not run.
- `cargo tree -p share-edge -e normal`: **unverified: lock** (not run). The manifest adds only crates.io
  dependencies; the path crates stay `share-proto`, `platform` (and the patched `vendor/libsqlite3-sys` via
  `sqlx`, as in EDGE1). [INFERENCE from `Cargo.toml`]

Tests written (`tests/certs.rs`, fake zone + fake CA + fake CAA, `propagation: 0`, `auth_gap: 0`):
`issuance_writes_records_serializes_txt_and_serves_the_chain` (CNAMEs, ledger, no TXT left, never two TXTs,
leaf SPKI = CSR SPKI, same CSR → chain, `GET cert`), `foreign_names_and_foreign_meshes_are_refused`
(`csr_names` for a foreign SAN set and for a bare-only CSR, `not_owner` for another mesh, no job, no record),
`a_different_csr_needs_rotate_and_rotation_revokes_the_old_chain` (`409 csr_mismatch {spki}`, rotate →
`replaces` old, old revoked `superseded`), `renewal_follows_ari_then_falls_back_to_two_thirds` (ARI window →
renewal with `replaces`, no token needed at 1/week, nothing revoked; no window → not at day 50, renewed at day
61), `release_revokes_and_deletes_records`, `reconciliation_removes_orphans_and_keeps_live_records`,
`a_provider_panic_is_a_failed_attempt_with_backoff` (panic in `create` → job `queued`, attempt 1,
`last_error` "panicked", next attempt ≥ 30 s later; succeeds once fixed), `new_names_wait_for_a_token`
(second new name at 1/week: `queue_pos 1`, stays `queued` without records or an attempt, issued a week later).
Unit tests: `dns` host helpers and both credential spellings; `acme` CAA lines round-trip through `caa_ok`.
`tests/visitor.rs` now starts with the fakes (CAA gate satisfied by `FakeCaa::registered`).

## For the orchestrator

- `Cargo.lock` changes (not committed; +154 lines): new packages `instant-acme`, `zone-update`,
  `hickory-resolver`, `hickory-net`, `resolv-conf`, `prefix-trie`, `moka`, `tagptr`; `backon` 1.6.0 (already
  locked) becomes a direct dependency of `share-edge`.
- No `share-proto` change needed.

## Fix 1 — renewal skipped for freshly claimed names

**Symptom (gate).** `cargo test -p share-edge --test certs` → `renewal_follows_ari_then_falls_back_to_two_thirds`
panicked at `tests/certs.rs:163` (`issued` 1, expected 2): the renewal inside the ARI window issued nothing.

**Root cause.** Not the new-certificate bucket: `certs::process` draws on `le_new` only for `kind == "new"`;
`renew` and `rotate` jobs never touch it. The real cause: `Store::claim` inserted the name with
`last_attach_ms` NULL (and reset it to NULL on re-claim). `Store::attach` stamps only the names the mesh
already holds, and a `hello` always comes before a claim, so a name claimed over a live tunnel had no attach
time until the next `hello`. `renewal_scan` reads NULL as 0, so `now - 0 > NO_RENEW_AFTER_MS` held and the
name was skipped as "unattached for 90 days" (§6.3). The test was correct: its claim helper calls
`attach` and then `claim`, which is the production order (`control.rs` hello → claim). In production, an engine
that stays connected after claiming would have lost its certificate at expiry.

**Fix.** `crates/share-edge/src/store.rs` `Store::claim`: `last_attach_ms` is set to the claim time on insert
and on re-claim. The claim arrives over a live tunnel, so it counts as an attach.

**Test added.** `tests/certs.rs` `renewal_runs_while_the_new_cert_bucket_is_empty`: `new_certs_per_week: 1`,
`sky` issued (takes the only token), `fog` submitted and left queued for the next token. Inside an ARI
window, `sky` renews with `replaces` while `fog` stays `queued` with its job still about a week out. No
`hello` after the claims.

**Doubt (not fixed, not in scope).** `last_attach_ms` moves only on `hello` and on claim. A mesh whose
tunnel stays up for more than 90 days without reconnecting would still be read as unattached by the renewal
scan and by the future inactivity expiry. TODO(210-M3): count a live tunnel as attached (refresh on
heartbeat or check `Edge::mesh_tunnels` in the scan).

**Verification (after the last code edit, each under its own lock hold):**
- `cargo test -p share-edge`: all pass. Unit tests 12/12, `certs` 9/9 (including both renewal tests),
  `visitor` 1/1, doc-tests 0.
- `cargo clippy -p share-edge --all-targets -- -D warnings`: clean (exit 0).
- `cargo tree -p share-edge -e normal`: the only path crates are `share-edge`, `share-proto`, `platform` and
  `vendor/libsqlite3-sys`.
- No `Cargo.lock` change.
