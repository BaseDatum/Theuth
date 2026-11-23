# 210 as built — EDGE-FIX2 (two fixes from the live end-to-end test)

Design: `docs/design/210-public-share.md` rev 8 (§6.4 ledger/reconcile, §6.5 job backoff, §8.5 admin). Code commit
`1b0ebd7e7`.

## 1. Reconcile never deletes a held name's records

`certs::reconcile` (`crates/share-edge/src/certs.rs`) kept a ledger CNAME only when its name was held **and**
`names.records != 0`. A job that died between `dns_add`/zone write and `set_records(true)` (records == 0) had its
CNAMEs deleted at the next start as "orphans". Now the keep test is `cert_row(..).is_some_and(|r| r.held())`:
every name not `released`/`banned` (allocated, queued, ordering, valid) keeps its CNAMEs; only names no longer held
lose them. The next attempt re-runs `ensure_records` (ledger `ON CONFLICT DO NOTHING`, `dns::ensure` idempotent)
and sets the flag. Challenge TXTs are still always removed.

Test: `tests/certs.rs::reconciliation_keeps_a_held_names_half_written_records` (queued name with ledger+zone
CNAMEs and records 0 survives reconcile; a tick then issues, `valid`, records 1, still 2 ledger rows). The existing
`reconciliation_removes_orphans_and_keeps_live_records` (released name's CNAMEs and a stale TXT go) still passes.

## 2. `admin cert retry <name>`

- Wire (`share-proto`, additive, approved by the owner session): `AdminCommand::CertRetry { name }`
  (`{"cmd":"cert-retry","args":{"name":"sky"}}`), mutating (audited with `--ref`); typed answer
  `CertRetryResult { name, job_id, kind: JobKind, attempts_before: u32, last_error: Option<String>, next_at_ms }`.
- CLI: `theuth-share-edge admin cert retry <name>` (`AdminCmd::Cert { cmd: CertCmd::Retry }`, nested like `invite`).
- Store: `Store::retry_job(id, now_ms) -> bool`: `UPDATE cert_jobs SET next_at_ms = now, attempts = 0 WHERE id = ?
  AND state = 'queued'`. Resetting `attempts` matters: `backoff(attempts)` and the `acme.max_attempts` give-up both
  read it, so the retried job starts at the 60 s rung with a full attempt budget.
- Handler `admin::cert_retry`: no cert backends → `invalid`; no queued/ordering job → `not_found`; job `ordering`
  (leased) → `invalid` (untouched); else retry and `certs.wake.notify_one()` so the runner leases it immediately.
- Scope: only the open job of the name (`open_job`). Failed jobs (past `max_attempts`) are not resurrected; the
  engine's next CSR submit queues a new one.

Test: `tests/admin.rs::cert_retry_makes_a_backed_off_job_due_and_resets_attempts` (2 failed attempts, next_at 1 h
out → retry answers kind/attempts_before/last_error, row is queued/attempts 0/next_at now, leasable at now; while
ordering → invalid; unknown name → not_found). Proto: `m4_commands_and_typed_results` round-trips the command, the
kebab wire line and `CertRetryResult`.

## Deviations

- §8.5's command list in the design doc does not name `cert retry`; the design doc is not edited here.
  TODO(210-M4): add `cert retry <name>` to the §8.5 command list.

## Not built

Nothing in scope left open.

## Doubts

- Wire name is `cert-retry` (flat, like `revoke-cert`) while the CLI is nested `cert retry`, matching how
  `invite mint` maps to `invite-new`.

## Verification (each under its own `/tmp/theuth-build.lock` hold)

- `cargo test -p share-edge -p share-proto --all-features`: all passed (share-edge lib 38, tests 8/15/4/2;
  share-proto 33), 0 failed.
- `cargo clippy -p share-edge -p share-proto --all-targets --all-features -- -D warnings`: clean.
- `cargo clippy -p share-edge -p share-proto --all-targets -- -D warnings`: clean.
- `cargo tree -p share-edge -e normal`: ok; direct deps unchanged (share-proto, platform + external only).
- `Cargo.lock`: no new packages from this phase (it was already modified in the tree by another builder).
