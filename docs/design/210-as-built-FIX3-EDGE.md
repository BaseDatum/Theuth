# 210 as built — FIX3-EDGE (E2E1 findings 3 and 5)

Branch `feat/210-public-share`, commit `c7a4f625e`. Scope: `crates/share-edge/**` only.

## What was built

### Finding 3: DNS-01 TXT probe works without IPv6 (`crates/share-edge/src/dns.rs`)

- New trait `dns::NsResolver` (injectable): `servers(zone)` → NS names, each with all its A **and** AAAA addresses
  (`type NameServer = (String, Vec<IpAddr>)`); `global_v6(probe: Ipv6Addr) -> bool`; `txt(server, fqdn)` →
  TXT strings (empty for NXDOMAIN / no records = "answered"; `Err` = no answer: timeout/unreachable).
- `dns::HickoryNs` is the production impl: system resolver for NS, then separate `A` and `AAAA` lookups per NS
  name (the old code used `lookup_ip`, which in the pod yielded only the AAAA set). One uncached, non-recursive
  resolver per query, 3 s timeout, 1 attempt. `global_v6` binds `[::]:0` and UDP-`connect`s to the first NS AAAA
  address (no packet sent) and checks the chosen source address is global (not loopback, unspecified,
  `fe80::/10`, `fc00::/7`, v4-mapped).
- `dns::AuthoritativeTxt { resolver: Arc<dyn NsResolver>, interval }`, `AuthoritativeTxt::new(resolver, interval)`,
  `Default` = `HickoryNs` + 2 s (used by `lib::start`). Algorithm:
  - per NS, addresses ordered IPv4 first when the host has no global IPv6, IPv6 first when it has (stable sort;
    the other family stays as fallback, so an AAAA-only NS is still tried);
  - each round queries all not-yet-satisfied NS concurrently; per NS the first address that answers decides and
    moves to the front (sticky);
  - done as soon as every NS that has **ever answered** returns the value and at least one does; NS that never
    answered are logged (`warn`) and not waited for;
  - `info!(fqdn, ns, addr, "challenge TXT visible")` names the answering address;
  - at the deadline: error listing NS that answered without the value and NS that never answered (the caller in
    `certs.rs` already logs and proceeds, unchanged).
- TXT match uses `same(Rtype::Txt, ..)` (quote/trailing-dot tolerant) on the concatenated character-strings
  (`TXT::txt_data`) instead of `TXT::to_string()`.

### Finding 5: wildcard CNAME first (`dns.rs`, `certs.rs`)

- `dns::name_hosts` now returns `[*.<n>, <n>]` (write order). `ensure_records` iterates it unchanged, so `*.<n>`
  is written before `<n>`.
- `remove_records` iterates it reversed (`<n>` deleted before `*.<n>`).
- `reconcile` sorts ledger rows so `*.`-hosts are removed after the others (the ledger is ordered by
  `created_ms`, which now puts `*.<n>` first).

## Public API changes (crate `share_edge`)

- `dns::AuthoritativeTxt` is no longer a unit struct: use `AuthoritativeTxt::default()` or `::new(resolver, interval)`.
- New: `dns::NsResolver`, `dns::HickoryNs`, `dns::NameServer`.
- `dns::name_hosts` element order flipped (only caller: `certs.rs`).
- Test helper `tests/common::FakeZone` gained `ops` (create/delete log) and `ops_of(op, rtype)`.

## Tests

- `dns::tests::without_global_v6_each_ns_is_asked_over_ipv4` — NS with AAAA listed first, no global v6: only the
  IPv4 addresses are queried.
- `dns::tests::the_other_family_answers_when_the_preferred_one_does_not` — global v6: AAAA first, times out, A
  answers; AAAA-only NS without v6 still tried.
- `dns::tests::waits_for_every_answering_ns_and_skips_silent_ones` — exact query sequence: satisfied NS not
  re-queried, NS without value polled until it has it, silent NS not waited for.
- `dns::tests::an_ns_that_answers_without_the_value_fails_at_the_deadline` — and "nobody answers" is an error.
- `dns::tests::global_v6_excludes_local_scopes`, `hosts_are_relative_to_the_zone` (new order).
- `tests/certs.rs`: issuance asserts CNAME creates `["*.sky", "sky"]`; release asserts deletes `["sky", "*.sky"]`;
  reconciliation (ledger seeded `*.old` first) asserts deletes `["old", "*.old"]`.

## Deviations / doubts

- "Reachable" = answered at least once during this wait. An NS silent on every round is skipped (warned), so a
  Porkbun NS that is down does not block issuance; if every NS is silent the probe errors at the deadline (the
  caller still proceeds, as before).
- The v6 route check is a local UDP connect; on hosts with a global v6 address but broken upstream v6 the AAAA
  query times out (3 s) and the IPv4 address answers, then sticks.
- Not verified against live Porkbun (no network calls allowed here). `[INFERENCE]` from E2E1: Porkbun NS answer
  over IPv4 from the pod, so the probe should finish in seconds instead of 30 s.
- Deadline is checked between rounds; a round can overrun it by one query timeout (≤ ~6 s with UDP→TCP), within
  the caller's `propagation + 15 s` budget.

## Not built

- Nothing outstanding for findings 3/5. Other E2E1 findings are not in this scope.

## Verification (each under its own `/tmp/theuth-build.lock` hold, after the last edit)

- `CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features` — all pass (lib 43, admin 8, certs 15, mesh 4,
  visitor 2, plus empty bins).
- `cargo clippy -p share-edge --all-targets -- -D warnings` — clean.
- `cargo clippy -p share-edge --all-targets --all-features -- -D warnings` — clean.
- `cargo tree -p share-edge -e normal` — exit 0, no new direct dependencies (hickory-resolver was already there).
- Cargo.lock: not changed by this work.
