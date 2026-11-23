# 210 — Public web sharing: appendix

Companion to `docs/design/210-public-share.md` (rev 8, 2026-11-23). Background, history and dispositions that an
implementer does not need to build P0 or P1. Section numbers match the main doc's references: "§11.9" in the main
doc means §11.9 here. Where anything here disagrees with the main doc, the main doc wins.

Contents: revision history · §4.6 building blocks · §6.1 zone-update source notes · §6.6 Public Suffix List · §6.8
`.zip` · §8.1 complete edge config · §8.4 deploy detail · §11 alternatives · §14 review dispositions (first review,
rev 7 decisions, final review).

## Revision history

Rev 7's status line, verbatim:

> design, 2026-11-22 · revised 2026-11-22: ACME and open admission in P1; domain configurable (tun.zip) · rev 3 2026-11-22: one name per user, wildcard per name via DNS-01, Caddy and TLS-ALPN-01 rejected · rev 4 2026-11-22: zone-update for DNS; building-blocks audit · rev 5 2026-11-22: edge is a standalone service (theuth-share-edge), not an engine role · rev 6 2026-11-23: Fable 5.2 review folded in (B1–B2, M1–M11, m1–m12, N1–N5) · rev 7 2026-11-23: names belong to the mesh (delegated engines, replicated name key); §12 Q1–Q4 and Q6 answered (both validation paths in P1, open launch, owner-only approval, Porkbun)

Rev 7's §1 revision paragraphs, verbatim:

Revision of the same day (owner, binding): automatic certificates (ACME DNS-01, Let's Encrypt) and admission for
*anyone who has theuth* ship in P1, with no account; the tunnel domain is a variable, never hard-coded, with `tun.zip`
as the worked example; reserved subdomains are in P1. §6.5–§6.8, §9.4, §10 and §13 carry the consequences.

Revision 3 of the same day (owner, binding): the edge stays blind (Caddy considered and rejected, §11.8); one name
per user with every share under it replaces the earlier two-level `<random16>.<ns>.D` namespaces and the separate
flat reserved `<name>.D` (D5); certificates are one wildcard per name over DNS-01 with the operator's DNS token (D7,
D17); per-hostname TLS-ALPN-01 and the edge as authoritative DNS are rejected (§11.9, §11.10).

Revision 5 of the same day (owner, binding): the edge is an independent service, `theuth-share-edge`, built, imaged,
deployed and upgraded apart from theuth, exactly as `theuth-relay` is, and theuth has only a client of it, as
`crates/link` is the relay's client (D2, D14, D18, §4.3, §8). Rev 1–4's engine-hosted edge is §11.12.

Revision 6 (2026-11-23) folds in the review `docs/design/210-review-fable.md` (§14): human-only approval becomes
owner-only on every surface, with its residual stated (D10, §7.2); `open` admission gets lazy allocation, a names
cap, byte budgets and an invite-first launch (D19, §9.4, §9.5); Google Trust Services' `dns-persist-01` is designed
as Path P beside rev 5's DNS-01 path, now Path Z (§6.0); names can be handed over (D20); P1 is cut down (§10).

Revision 7 (2026-11-23, owner, binding): "Name per mesh for sure". A name belongs to the mesh, not to one engine: the
edge binds it to the mesh share key, each engine attaches with its own key and a delegation signed by that key, the
name's TLS key is replicated through the mesh vault, and several machines serve sub-labels of one name (D21, §5.4,
§5.7); per-engine names are §11.15. The same day the owner answered §12 Q1–Q4 and Q6: both validation paths ship in
P1, GTS primary and Let's Encrypt over Porkbun as overflow (D6, D7, §6); the public edge launches `open` with
`invite` as the brake (D19, §9.5); approval stays owner-only (D10). The decisions still open are §12 Q5 and Q8–Q18.

**Rev 8 (2026-11-23)** folds in the final review `docs/design/210-review-fable-final.md` (F-M1…F-M9, F-m1…F-m10,
F-N1…F-N5; dispositions in §14.3) and the owner's answers of the same day:

1. **Let's Encrypt is the P1 primary and only active path** ("I want to test with LE; having [GTS dns-persist] built
   for later is fine"). Path Z (LE `dns-01`, zone-update, Porkbun) ships in P1; Path P (GTS `dns-persist-01`) stays
   designed behind the same `Ca`/`Validator`/`DnsZone` traits and is enabled in P2 with `cas.gts.enabled: true` once
   the operator has a GTS EAB. Per-name records are P1 again for every name (D17); CAA carries the LE lines only in
   P1; the slot accounting is LE-only in P1; F-M7's GTS accounting is kept as P2 text (main §6.0). Rev 7 had GTS
   primary and LE overflow, both in P1.
2. **A lost rekey race is the operator's judgement**, with no recovery code (main §5.7; rejected alternative §11.16).
3. **Porkbun: the current account**, with an API key scoped to `tun.zip`, the `dns` permission and the edge's source
   IPs; the conditional "if registered at Porkbun / elsewhere" text and the dedicated-account recommendation are
   removed (main §6.4; optional note §11.17). The Porkbun sandbox (`pk1_sb_` keys) is used for the first Path Z test.
4. **The hub dials for rekey duties even with its own sharing switch off**, with disclosure (D22, main §5.7).

Readability (F-N5): D-rows cut to two sentences; lazy allocation (main §5.2), Porkbun key scoping (main §6.4) and
`421` (main §5.4) each stated once; rev narration removed from normative text; §4.6, §6.1's source notes, §6.6,
§6.8, §8.1's full config, §8.4's detail, §11 and §14 moved here with their numbers. New in the main doc: `GET /v1/info`
and the clock rule, the rekey state machine, `share.name_key.ensure` and `csr_mismatch`, `share_edges` (D22), both
sqlite DDLs (§5.8), the client connection state machine (§5.9), the slot-queue order and ETA (§6.5), the admin
socket protocol and `acme register` (§8.5), byte metering and shaping (§9.6), the numbered P0 checks and the P1
milestones (§10). `Delegation.serial` is dropped (F-m1).

## 4.6 Building blocks: no reinvented wheels

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins.

Doc 00 §1 rule 2 (`docs/design/00-architecture.md#L17-L18`): "Off-the-shelf library first; we write glue. A custom
implementation needs a written reason in this doc." Every mechanism of this design, checked against crates.io and the
workspace `Cargo.lock` on 2026-11-22. "Direct" means a workspace crate already depends on it; "transitive" that the
version is in `Cargo.lock` through another dependency; "no" that it is new. "Used by" assigns it to the rev 5 crates
(D14): the edge binary `share-edge`, the contract `share-proto`, the client plugin `share`.

| concern | crate | version · license | already in tree? | used by | what theuth writes itself |
|---|---|---|---|---|---|
| Transport, peer identity | `iroh` | 1.4.1 · MIT OR Apache-2.0 | direct (`mesh`, `a2a`) | `share-edge` (its own `Endpoint` and `Router`); `share` (the `Connection` from `mesh`'s `Net::dial_alpn`) | the edge's `theuth/share/1` handler and admission; `Net::dial_alpn` with `connect_with_opts` (§5.3) |
| Edge ticket | `iroh-tickets` (`Ticket` trait, as `EndpointTicket` uses it) | 1.0.0 · MIT OR Apache-2.0 | no (needs `iroh-base` ^1; 1.4.1 is in tree) | `share-proto` | `ShareEdgeTicket { addr, invite }` implementing `Ticket` (§5.5) |
| Mesh delegation signatures | `iroh-base` `SecretKey::sign` / `PublicKey::verify` (Ed25519), `postcard` | 1.4.1 · MIT OR Apache-2.0; 1.1.3 · MIT OR Apache-2.0 | transitive (both in `Cargo.lock`) | `share-proto` (types, signed bytes, verify), `share` (signs on the lease holder), `share-edge` (verifies) | `Delegation`, `Rekey`, `Handover` and their domain-separation tags (§5.7) |
| Name key replication | the vault (`secrets`, workspace) | — | in tree | `share` | the `share/` reserved prefix (§8.3) |
| Control plane over a QUIC stream | `axum` (edge router), `hyper` + `hyper-util` (server on the stream, host client) | 0.8.9 · MIT; 1.12.0 · MIT; 0.1.21 · MIT | direct (`a2a`, `relay`) | `share-edge` (axum server), `share` (hyper client); bodies are `share-proto` types | the endpoints of §5.3; no framing |
| Server push on the control plane | `axum::response::sse` | 0.8.9 · MIT | direct | `share-edge`; event names in `share-proto` | the event names of §5.3 |
| Visitor preamble (address + SNI) | `ppp` (PROXY v2 with TLVs) | 2.3.0 · Apache-2.0 | no | `share-edge` (writes), `share` (parses); TLV constants in `share-proto` | the TLV choice (§5.3) |
| Storage | `sqlx` (sqlite, `migrate`) | 0.9.0 · MIT OR Apache-2.0 | direct, workspace | `share-edge` (`share-edge.sqlite`), `share` (`share.sqlite`) | two schemas |
| ACME orders, EAB, ARI, revocation | `instant-acme` (`aws-lc-rs`, `time`, `x509-parser`) | 0.8.5 · Apache-2.0 | no | `share-edge` | the `Ca`/`Validator` traits, `Unknown("dns-persist-01")` handling (§6.0), the job runner, budgets and slot queue (§6.2, §6.5) |
| DNS records (Path Z only) | `zone-update` (one provider feature + `async`) | 0.12.3 · Apache-2.0 OR MIT | no (its `ureq` 3.5.0 and `blocking` 1.7.0 are transitive) | `share-edge` | the `DnsZone` trait and one impl over the concrete provider type, inside the worker's panic boundary (§6.1) |
| Key and CSR (hosting engine) | `rcgen` (`aws_lc_rs`) | 0.14.10 · MIT OR Apache-2.0 | no | `share` | — |
| CSR check (edge) | `x509-parser` (`verify`) | 0.18.1 · MIT OR Apache-2.0 | no (`instant-acme`'s ARI feature pulls it anyway) | `share-edge`; the expected SAN set comes from `share-proto` | the rule "SANs equal `{<name>.D, *.<name>.D}` of a held name" |
| SNI peek without terminating | `rustls::server::Acceptor` | 0.23.46 · Apache-2.0 OR ISC OR MIT | direct (`relay`) | `share-edge` | the read loop that keeps the raw bytes to replay (§4.2) |
| TLS termination (hosting engine) | `tokio-rustls` | 0.26.6 · MIT OR Apache-2.0 | direct (`agent`) | `share` | a ~20-line `ResolvesServerCert`: host → its name's `CertifiedKey` (one label stripped) |
| Certificate hot swap | `arc-swap` | 1.9.3 · MIT OR Apache-2.0 | transitive | `share` | — |
| HTTP/1.1 + h2 serving (hosting engine) | `hyper-util` `server::conn::auto` | 0.1.21 · MIT | direct (feature to add) | `share` | — |
| Reverse proxy | `hyper-util` `client::legacy::Client` | 0.1.21 · MIT | direct (feature to add) | `share` | header policy (strip and set `X-Forwarded-*`, `Host` rewrite) and path-prefix routing (§5.6) |
| WebSocket and other upgrades | `hyper::upgrade::on` + `tokio::io::copy_bidirectional` | 1.12.0 · MIT; tokio 1.54.1 · MIT | direct | `share` (upgrades); `share-edge` (`copy_bidirectional` for the splice) | the 101 hand-off; frames are never parsed, so no `tokio-tungstenite` |
| Static files | `tower-http` `ServeDir` | 0.7.1 · MIT | direct, workspace | `share`; `share-edge` uses only its request-body limit | dot-segment and root denials (§5.6) |
| Rate limits in memory | `governor` keyed limiters | 0.10.4 · MIT | no | `share-edge` (visitor, network, key limits), `share` (password attempts) | the keys (IPv4, IPv6 /64 and /48); day and week counts that must survive a restart (§9.4) are sqlite rows |
| Password hashing | `argon2` (argon2id) | 0.6.0 · MIT OR Apache-2.0 | transitive (0.5.3 and 0.6.0) | `share` | — |
| Gate and interstitial cookies | `cookie` (`signed` jar) | 0.18.2 · MIT OR Apache-2.0 | transitive, without `signed` | `share` | cookie names and scope (§7.5) |
| Job retry delays | `backon` | 1.6.0 · Apache-2.0 | transitive | `share-edge` | the `cert_jobs` table (below) |
| Random names, invites | `rand` + `data-encoding` | 0.10.3 · MIT OR Apache-2.0; 2.11.1 · MIT | direct | `share-edge` | 6 draws from `[a-z2-7]`, the a2a ceremony pattern (`crates/a2a/src/contacts/ceremony.rs#L35`) |
| Record ids | `uuid` v7 | 1.29.0 · Apache-2.0 OR MIT | direct, workspace | `share` (share ids), `share-edge` (job ids) | — (no ULID: the workspace has no ulid crate, `crates/dreaming/Cargo.toml#L40`) |
| Edge route table | `dashmap` | 6.2.1 · MIT | direct, workspace | `share-edge` | — |
| Edge config file + env overrides | `figment` (`yaml`, `env`) | 0.10.19 · MIT OR Apache-2.0 | direct, workspace; theuth's own loader (`crates/theuth/src/main.rs#L1599-L1600`) | `share-edge` | the config struct (§8.1) |
| Edge CLI | `clap` | 4.6.7 · MIT OR Apache-2.0 | direct (`relay`) | `share-edge` | `run`, `ticket`, `admin …` (§8.4) |
| Stop signal, owner-only key files | `platform` (workspace crate, OS primitives only) | — | in tree; `theuth-relay` uses it (`crates/relay/Cargo.toml#L14`) | `share-edge`, `share` | the edge's 32-byte `iroh.key` load-or-create, as the engine's `crates/mesh/src/identity.rs#L22-L40` (reimplemented, not imported); the client's 0600 cookie key file via `platform::files::create_private` (§8.3) |

Candidates rejected, and what is built here:

- **SNI peek: not `tls-parser`, not `LazyConfigAcceptor`.** `tls-parser` 0.12.2 (MIT/Apache-2.0) was last released
  2024-09-09 and adds `nom`; rustls's `Acceptor`, already in tree, parses the same ClientHello with the parser the
  hosting engine terminates with, so whatever the edge can route the host can serve. `tokio_rustls::LazyConfigAcceptor`
  turns the socket into a TLS session, but the edge must forward the untouched bytes, so it feeds `Acceptor` a copy and
  replays the buffer. opentunnel hand-writes this parser (§11.2); this design does not.
- **Reverse proxy: not `hyper-reverse-proxy`.** 0.5.1, last released 2022-03-12, on hyper 0.14; theuth is on hyper 1.
  The proxy is the `hyper-util` client plus a header policy theuth must own anyway (§5.6), about 150 lines
  [INFERENCE: estimate].
- **Durable certificate jobs: one table, not a framework.** Nothing in the tree is a retrying job queue:
  `crates/schedule` fires markdown schedules into sessions, `crates/tasks` is the agents' work ledger, and the replica
  lease is the hub lease (`crates/replica/src/lib.rs#L49`). `apalis-sqlite` 1.2.0 (MIT, on sqlx 0.9) would give
  durable retries but not what this queue is: product state, FIFO slot positions shown to users, per-network fairness
  and the global budgets (§6.5), for one worker; it also brings `ulid` and a worker framework. So `cert_jobs` is one
  sqlx table (`name`, `state`, `ca`, `lease_until`, `attempts`, `next_at`) and `backon` computes the delay.
- **Cookies: not the relay's code.** `crates/relay/src/cookie.rs#L1-L4` hand-rolls HMAC-SHA256 because "the workspace
  has no `hmac` crate"; it now has one (`hmac` 0.13), and `cookie`'s signed jar also covers parsing and attributes.
- **Built here on purpose:** the edge's accept loop and byte copy (an accept, a parse call, `copy_bidirectional`), the
  name table and its rules (D5), the budgets and the slot queue (§6.5). That is the product, not plumbing.


## 6.1 Stack and zone-update source notes

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins.

Porkbun facts re-read 2026-11-23 ([full reference](https://porkbun.com/llms-full.txt), [getting started](https://porkbun.com/llms/guides/getting-started)): a sandbox mode with `pk1_sb_`/`sk1_sb_` keys runs the whole API against an isolated datastore where "DNS … and DNSSEC are simulated" (so it tests calls, not real resolution); API keys can be restricted to domains, source IPs/CIDRs and scopes (`dns` = DNS records and URL forwarding; `domains` = nameservers, DNSSEC, contacts, transfers); per-domain API opt-in, or "Opt In All Domains"; automatic zone restore points before the first write each hour (`/dns/history`, `/dns/restore`); DS records and an ANS-only `/ans/enableDnssec` exist in the API, so rev 7's "no API call turns the toggle on" was outdated (F-N4).

- **ACME:** `instant-acme` 0.8.5 (Apache-2.0; features `aws-lc-rs`, `time`, `x509-parser`), the only maintained Rust
  client with DNS-01, EAB (`ExternalAccountKey`), ARI (`Account::renewal_info`, `NewOrder::replaces`) and revocation
  (`Account::revoke`); orders finalize with the hosting engine's CSR (`Order::finalize_csr`). theuth writes no ACME
  protocol code; it lives behind the `share_edge::acme::Ca` trait of §6.0, one impl per configured CA.
- **DNS (Path Z):** `zone-update` 0.12.3 (Apache-2.0 OR MIT, released 2026-10-21, ~2.2k downloads,
  [repo](https://github.com/tarka/zone-update); the owner's choice, 2026-11-22): "CRUD-like operations on DNS records"
  for nine providers, each a cargo feature: Bunny, Cloudflare, deSEC, DigitalOcean, Dnsimple, DnsMadeEasy, Gandi,
  Linode, Porkbun ([matrix](https://github.com/tarka/zone-update/blob/main/docs/PROVIDERS.md)). The provider is
  **Porkbun** (owner, 2026-11-23), feature `porkbun`, which the matrix marks "Tested against personal account"
  (`docs/PROVIDERS.md` L15). Its `DnsProvider` trait has `create_record`/`get_record`/`update_record`/
  `delete_record`/`delete_all_records` over `RecordType` (`A`, `AAAA`, `CAA`, `CNAME`, `MX`, `NS`, `PTR`, `SRV`,
  `TXT`, `SVCB`, `HTTPS`) plus TXT and `A` helpers (`create_txt_record`, `delete_txt_record`, `get_txt_record`,
  `create_a_record`, …); `Config { domain, dry_run }` names the zone, and hosts are relative to it (`sky`, `*.sky`,
  `_acme-challenge.sky`). The `Provider` enum deserialises with serde, tagged by `name` (`src/lib.rs` L54; variant
  `PorkBun(porkbun::Auth)`, L73–L74), so the edge config's `dns.provider: porkbun` plus the `{key, secret}`
  credential from env or file become a `Provider` (§8.1, §8.3). Dependency: `zone-update = { version = "=0.12.3",
  default-features = false, features = ["async", "porkbun"] }`.
- **What the source says** (the published 0.12.3 crate: `src/lib.rs`, `src/porkbun/mod.rs`, `src/http.rs`,
  `src/async_impl/porkbun.rs`; read 2026-11-23), and what the design does about it:
  - The generic methods carry `where Self: Sized`, so the `Box<dyn DnsProvider>` that `Provider::blocking_impl` and
    `async_impl` return exposes only the TXT and `A` helpers and the deletes: no `CNAME` through it. theuth's
    `DnsZone` impl matches the deserialised `Provider` once and keeps the concrete `porkbun::Porkbun`, on which
    `create_record(RecordType::CNAME, "*.sky", &target)` works.
  - It calls `https://api.porkbun.com/api/json/v3/dns` (`porkbun/mod.rs` L18): `create/{domain}` (L126),
    `retrieveByNameType/{domain}/{type}/{host}` for reads (L50, L110–L120), and `edit/{domain}/{id}` and
    `delete/{domain}/{id}` after a lookup (L162, L187–L201); `delete_all_records` deletes each id it finds
    (L203–L211). Names go into the URL path unencoded (L50).
  - `async` is `blocking::unblock` around the blocking `ureq` client (`async_impl/porkbun.rs` L4–L14), a thread pool
    rather than tokio I/O; fine for a few calls per order. `ureq` 3.5.0 and `blocking` 1.7.0 are already in
    `Cargo.lock`.
  - **Errors are read from the HTTP status only.** Reads take 200 as records, 404 as none and anything else as an
    error (`http.rs` L69–L87); creates and edits fail on non-2xx (`porkbun/mod.rs` L145, L182; `http.rs`
    L89–L100); **deletes never check the status** (`porkbun/mod.rs` L99–L101); Porkbun's `status: "ERROR"` body is
    never read. Porkbun answers refusals with HTTP 400, 403 or 429
    ([API docs](https://porkbun.com/api/json/v3/documentation)), so most failures surface, but a failed delete does
    not. `DnsZone` therefore reads back every write and every delete, and the DNS-01 step already waits for the
    authoritative answer (§6.2).
  - **TTL is fixed at 300 s** (`porkbun/mod.rs` L134, L171), while Porkbun's minimum is an account setting,
    "typically 600" (same docs, `/dns/create`). Whether Porkbun rejects or raises a 300 s TTL is not documented; P0
    checks it, and if Porkbun rejects it the provider is patched in-repo to send 600 (00 §1 rule 3) and the patch
    offered upstream. The plan uses 600 s (§6.4).
- **Two TXT values at one name: serialised.** An order for `{<name>.D, *.<name>.D}` has two DNS-01 authorizations,
  and both TXT values live at `_acme-challenge.<name>.D`. Porkbun itself appears to accept two TXT records at one
  name (it refuses only an exact duplicate, `DUPLICATE_RECORD` [INFERENCE from its error table; not stated]), but
  zone-update's porkbun `get_record`, `update_record` and `delete_record` fail with `UnexpectedRecord` when a name
  holds more than one record of the type (`porkbun/mod.rs` L77–L80, L190–L192), the shape of its Cloudflare client
  too; only `delete_all_records` copes. The edge answers one authorization at a time: write the TXT for the first,
  wait for the authoritative servers, ask the CA to validate, wait for `valid`, clear it (`delete_all_records(TXT,
  …)`, read back), then the second. At most one TXT ever exists at the name; the cost is a second propagation wait
  per Let's Encrypt order (new names and renewals). A multi-value TXT call contributed upstream would remove the
  wait; not needed for P1.
  Cached first value: Let's Encrypt's recursors cap TTLs at 60 s (a Let's Encrypt engineer, "subject to change",
  [community thread](https://community.letsencrypt.org/t/dns01-validation-timeouts/116844)), so the worker waits 60 s
  after clearing the first TXT before writing the second, whatever TTL the provider writes; P0 confirms it on
  staging (§10). GTS's resolver cache is not documented, which matters only if GTS is ever used over `dns-01`.
- **Risks of `zone-update`.** Small user base and one person as author and maintainer of every provider (the matrix
  lists `tarka` in both columns). Answers: pin the exact version; enable only the chosen provider's feature; keep
  the edge's code behind a thin `share_edge::dns::DnsZone` trait (`ensure_cname`, `delete_name`, `set_txt`,
  `clear_txt`) so a provider or the whole crate can be swapped, or patched in-repo under 00 §1 rule 3. No Route 53,
  Google Cloud DNS, Hetzner or RFC 2136/TSIG: the operator's provider must be one of the nine (§13), and a missing one
  is contributed upstream, which its README asks for ("the most useful contributions would be to add additional DNS
  provider APIs").
- **Its Cloudflare client can panic** on an empty zone lookup (`zones.remove(0)`,
  [source](https://raw.githubusercontent.com/tarka/zone-update/main/src/cloudflare/mod.rs)); irrelevant to Porkbun,
  but the job worker still runs every `DnsZone` call in `tokio::task::spawn_blocking` (its `async` feature is a
  thread-pool wrapper anyway) and maps a `JoinError` that `is_panic()` to a failed attempt with back-off and an
  operator alert, never a worker crash.
- **Names are not URL-encoded**, so `*.sky` travels as a raw `*` in the request path (legal in a URL path; whether
  `ureq` passes it through untouched is not verified), and Porkbun documents `*` as a wildcard name but not `*.sky`
  below a subdomain. P0 measures create, read and delete of `*.<name>` on Porkbun (§10), and `DnsZone` reads every
  write back (above).
- **Hosting engine key and CSR:** `rcgen` 0.14.10 (MIT OR Apache-2.0, feature `aws_lc_rs`), P-256. **CSR check on the
  edge:** `x509-parser` 0.18.1 (`verify`): signature, then SANs (§6.2).
- The crypto provider is aws-lc-rs, installed as the process default before any `ServerConfig` is built, as the relay
  does (`crates/relay/src/main.rs#L88-L90`). The full building-block audit is §4.6.


## 6.6 Public Suffix List

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins. Rev 8: Path P is P2, so "P0 check 2" is now P2 entry check 5 of main §6.0, answered from GTS support or the Baseline Requirements, not by a spike.

With `D` in the PSL private section, each `<name>.D` is a registrable domain: browsers reject a tenant's cookie for
`Domain=D`, and Let's Encrypt counts the 50-per-week limit per name. Requirements from the
[guidelines](https://github.com/publicsuffix/list/wiki/Guidelines) (page of 2025-05-23, read 2026-11-22):

- A pull request to [publicsuffix/list](https://github.com/publicsuffix/list) adding `tun.zip` to the PRIVATE section,
  sorted by organisation name, headed `// <Organisation> : <url>` and `// Submitted by <name> <email>` (an address of
  the requesting entity), with the PR template filled in (why, example names and expected results) and `make test`
  passing.
- **Authentication:** a TXT record `_psl.tun.zip` containing the PR URL, left in place after the merge ("missing _psl
  records will be a flag used in determination of domains to remove"). Private entries must come from the domain
  owner or an authorized representative: "We will not accept patches submitted by third party users of the service".
  The owner, as registrant, opens it.
- **Registration term:** more than 2 years left at submission, a statement saying so, and a commitment to keep more
  than a year left.
- **Rationale:** subdomains issued to mutually untrusting parties (true here). Declined: entries whose sole purpose is
  rate limits; small, temporary or exploratory projects ("projects not serving more then thousands of users are quite
  likely to be declined").
- **Review time:** "NO SERVICE LEVEL AGREEMENTS ON TIME", and after a merge there is no way to expedite propagation
  into browsers.

So the entry may be declined while the edge is young, and the PSL's own bar ("projects not serving more then
thousands of users are quite likely to be declined") means it may never be accepted: **never listed is the planning
case**, and nothing in P1 depends on a listing. Under Path P the submission also waits for P0 check 2 (§6.0). Until it
is listed, every tenant is the same site to browsers:

- **Cookie tossing.** An app on `a.D` can set `Domain=D` cookies that apps on `b.D` receive. The engine's own gates use
  `__Host-` cookies, which cannot carry a `Domain` attribute and are safe; tenant apps' own cookies are not.
- **Cross-tenant CSRF.** `SameSite=Lax` and `Strict` compare sites, not origins, so a page on `a.D` can make a
  visitor's browser send `b.D` requests with `b.D`'s SameSite cookies. Tenant apps need CSRF tokens or
  `Sec-Fetch-Site: same-origin` checks; the engine's password form checks it (§7.5).
- **Password managers** that match on the registrable domain offer logins saved for one tenant on another tenant's
  page.

Settings and the terms say all three (§9.1). Once listed, the apex `D` is itself a public suffix: the Baseline Requirements forbid a wildcard
directly under a public suffix unless the applicant controls it, and a CA may refuse the apex [not verified per CA],
so the edge serves nothing on `D:443` (opentunnel uses a Cloudflare Origin CA cert for its apex for this reason), and
the abuse and terms pages live at an operator URL outside `D`. Shares under one name still share cookies with each
other — same owner, acceptable. A private edge (`invite` with known users) does not need a PSL entry.


## 6.8 The `.zip` TLD as `D`

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins.

`tun.zip` is the owner's choice and stays a variable (D16). Facts:

- `.zip` is a Google Registry TLD, open since May 2023, on the browsers' HSTS preload list as a whole TLD; the registry
  makes registrars warn that HTTPS is required ([policy](https://www.registry.google/policies/registration/zip/),
  [Netcraft](https://www.netcraft.com/blog/phishing-attacks-already-using-the-zip-tld)). Harmless here: the edge serves
  only HTTPS, and it removes TCP 80 (§4.1).
- **Reputation.** Netcraft found phishing on `.zip` within two weeks of launch and many brand and "installer/update"
  squats (same post); URLs mixing `@` and look-alike slashes with a `.zip` host read as file paths
  ([Rauch](https://medium.com/@bobbyrsec/the-dangers-of-googles-zip-tld-5e1e675e59a5)); SANS ISC advised blocking the
  TLD "until it is clear if it will be useful" ([diary](https://isc.sans.edu/diary/The+zip+gTLD+Risks+and+Opportunities/29838)),
  and vendors gave the same advice ([TechTarget](https://www.techtarget.com/news/The-potential-danger-of-the-new-Google-zip-top-level-domain/837760/)).
  Expect some corporate secure web gateways, DNS filters and home blocklists to block or down-rank the whole TLD
  [prevalence not measured]: visitors behind them cannot open a share at all.
- **Filename confusion.** SANS notes software may auto-link ZIP file names in text (same diary), so a chat message
  mentioning `report.zip` becomes a link; theuth always shows full `https://` URLs, so this affects visitors'
  perception, not routing.
- **Shared-domain blocklisting** [INFERENCE]: until the PSL entry, a blocklist may treat all of `tun.zip` as one site,
  so one tenant's phishing page can get every tenant blocked. Fast bans (§9.3) and the abuse inbox are the defence;
  this holds for any `D`, and `.zip`'s reputation raises the odds.

Tradeoff: a short, memorable, HTTPS-only name against a TLD that some networks block. The card and Settings always
show the edge's domain, so a user whose visitors cannot reach it can choose another edge; a second domain on the
same edge is P2 (D16).


## 8.1 Complete edge config (`config.example.yaml`)

The main doc states each default where it acts (§6.5, §9.4, §9.6, §4.2, §5.3); this is the whole file as P1 ships
it, for `deploy/share-edge/config.example.yaml`.

```yaml
domain: tun.zip               # D, sent in /v1/info and welcome
data_dir: /var/lib/theuth-share-edge   # share-edge.sqlite, iroh.key, acme/<ca>.json; persistent, backed up
listen: "[::]:443"            # "[::]:8443" in k8s behind Service port 443
listen_http: ""               # off; only for a D outside an HSTS-preloaded TLD
proxy_protocol_in: false
iroh: {listen: "[::]:7850", relay: https://iroh.relay.theuth.io, public_addrs: [], pkarr: true}
admin: {socket: /var/lib/theuth-share-edge/admin.sock, http: "[::]:9090"}   # §8.5; /metrics, /healthz
admission: open               # open | invite | closed; live via admin
reserved_per_mesh: 1
engines_per_mesh: 8
delegation_max_ttl: 25h       # top-level, not under limits
random_name_len: 6
reclaim_cooldown: 30d
reserved_words: []
brand_blocklist_file: /etc/theuth-share-edge/brands.txt   # §9.4; reloadable
terms_url: https://example.com/share-terms     # outside D
terms_rev: "2026-11-23"
abuse_url: https://example.com/share-abuse     # outside D
abuse_contact: abuse@example.com
require_caa: true             # cannot be false with admission open (§8.2)
acme:
  contact: mailto:ops@example.com
  profile: classic
  primary: letsencrypt        # P1
  invite_share: 0.5
  queue_max: 2000
  cas:
    letsencrypt:
      directory: https://acme-staging-v02.api.letsencrypt.org/directory   # prod: https://acme-v02.api.letsencrypt.org/directory
      validation: dns-01
      orders_per_3h: 200
      new_certs_per_week: 45
    gts:                      # P2
      enabled: false
      directory: https://dv.acme-v02.api.pki.goog/directory   # staging: https://dv.acme-v02.test-api.pki.goog/directory
      validation: dns-persist-01
      eab_file: /run/secrets/theuth-share-edge/acme-gts-eab.json
      orders_per_hour: 80
dns:
  provider: porkbun
  zone: tun.zip
  credential_file: /run/secrets/theuth-share-edge/dns.json
  target: edge.tun.zip
limits:                       # §9.4; "net" = IPv4 address or IPv6 /64
  creates_per_net_hour: 30    # burst 10
  new_meshes_per_net_day: 5
  certs_per_net_week: 2       # open meshes only; invited meshes are not counted per network
  queued_per_net: 2
  reserved_per_net: 6
  reserved_min_mesh_age: 0s
  tunnels_per_net: 10
  wide_net_factor: 4          # per IPv4 /24 and IPv6 /48
  v6_32_factor: 16            # per IPv6 /32
  asn_factor: 64              # per origin ASN
  asn_db: /var/lib/theuth-share-edge/ip2asn.tsv   # optional; absent = ASN tier off
  open_new_names_day: 500     # global, paced, open meshes only
  relayed_new_meshes_day: 20  # paced 1/h, burst 4
  names_max: 1200             # P1: Porkbun record bound (§6.4); counted at slot time
  allocated_ttl: 1h           # an allocated name without a CSR is evicted
  evict_unpublished_after: 7d
  bytes_per_mesh_day: 20GiB
  bytes_per_net_day: 50GiB
  engine_keys_per_mesh_day: 8
  tunnel_mbit: 50             # burst 64 MiB
  throttle_mbit: 2            # non-invited tunnels while egress is over pace (§9.6)
  egress_month: 10TiB         # the host's allowance; pace = allowance × fraction of month elapsed
  new_conns_per_tunnel_sec: 50
  conns_per_tunnel: 256
  open_bi_timeout_secs: 5
  conns_per_ip: 64
  new_conns_per_ip_sec: 20
  global_conns: 20000         # refuses to start with RLIMIT_NOFILE under 65536
  pre_hello_conns: 256
  hello_timeout_secs: 10
  info_per_ip_min: 30
  routes_per_tunnel: 64
  idle_secs: 900
```

## 8.4 Deploy

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins.

The edge ships like `theuth-relay`: its own image `ghcr.io/skymoore/theuth/share-edge` (static musl, rustls, `FROM
scratch`, as `deploy/relay/Dockerfile`), pushed by `just share-edge-docker-push [tag]` (modelled on
`relay-docker-push`, `justfile#L359`), versioned and released apart from theuth: `crates/share-edge/Cargo.toml`
carries its own `version` and tags `share-edge-v*`, not `version.workspace` as `crates/relay/Cargo.toml#L3` does.
It stays in the workspace and builds against its `Cargo.lock`, so a dependency bump in theuth reaches the edge only
at the edge's next release. Upgrading the edge never needs a
theuth release and the reverse, within §5.3's compatibility rule. New `deploy/share-edge/`, laid out like
`deploy/relay/`: `README.md`, `Dockerfile`, `compose.yaml`, `theuth-share-edge.service`, `config.example.yaml`,
`k8s/base/`, `k8s/<cluster>/`.

**Kubernetes (the owner's `tun.zip` edge).** The repo's cluster runs the HAProxy Kubernetes Ingress Controller of
HAProxy Technologies (`haproxytech/kubernetes-ingress`): `ingressClassName: haproxy` and `haproxy.org/*` annotations
(`deploy/relay/k8s/rwx-dev/ingress-iroh.yaml#L10-L14`), not the community `haproxy-ingress`, whose annotations are
`haproxy-ingress.github.io/*`. An Ingress need not decrypt: `haproxy.org/ssl-passthrough: "true"` routes TCP by SNI
without terminating, and `iroh.relay.theuth.io` already runs that way (same file). But the Ingress spec's wildcard
host matches exactly one DNS label: `*.foo.com` matches `bar.foo.com`, not `baz.bar.foo.com`
([hostname wildcards](https://kubernetes.io/docs/concepts/services-networking/ingress/#hostname-wildcards)). A rule
for `*.tun.zip` carries `sky.tun.zip` but not `app.sky.tun.zip`, and one rule per name cannot be written for names
claimed at runtime. Whether haproxytech's passthrough SNI map matches wildcards deeper than the spec is not checked;
relying on it would break on a controller change. Options:

| option | how | verdict |
|---|---|---|
| (a) Service `type: LoadBalancer` with its own IP | TCP 443 → pod 8443 and UDP 7850 on an IP only the edge uses; `edge.tun.zip` A/AAAA → that IP; `externalTrafficPolicy: Local` so the pod sees visitor addresses for per-IP limits and the PROXY preamble | **recommended**: no coupling to the ingress, visitor IPs intact, UDP beside TCP |
| (b) HAProxy TCP services | a `--configmap-tcp-services` ConfigMap entry `"<port>": "theuth/theuth-share-edge:443"`, or the TCP CRD (controller 3.0+): a whole frontend port to the Service | 443 on the controller's IP is already its HTTPS and passthrough frontend (`theuth.io`, `iroh.relay.theuth.io`), so this needs a second controller IP or a port other than 443, which visitors do not use; UDP still needs (a)'s Service; visitor IPs need `send-proxy-v2` and `proxy_protocol_in: true` |
| (c) HAProxy passthrough with a custom SNI rule | a frontend snippet in the passthrough frontend: `use_backend` on `req.ssl_sni -m end .tun.zip` to the edge's Service, with `send-proxy-v2` | works on one shared public IP; depends on controller snippets and its per-version frontend layout [not verified]; fallback only when no second IP exists; UDP still needs its own Service |

Sketch of (a), kustomize as `deploy/relay/k8s` (namespace `theuth`), abridged:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata: {name: theuth-share-edge, labels: {app.kubernetes.io/name: theuth-share-edge, app.kubernetes.io/part-of: theuth}}
spec:
  replicas: 1                 # one writer of share-edge.sqlite and one holder of iroh.key; no HA in v1 (§2)
  strategy: {type: Recreate}  # RWO volume; two pods with one endpoint key would split clients
  selector: {matchLabels: {app.kubernetes.io/name: theuth-share-edge}}
  template:
    metadata: {labels: {app.kubernetes.io/name: theuth-share-edge}}
    spec:
      automountServiceAccountToken: false
      securityContext: {runAsNonRoot: true, runAsUser: 65532, runAsGroup: 65532, fsGroup: 65532, seccompProfile: {type: RuntimeDefault}}
      containers:
        - name: edge
          image: ghcr.io/skymoore/theuth/share-edge:<tag>
          args: ["run", "--config", "/etc/theuth-share-edge/config.yaml"]
          ports:
            - {name: tls, containerPort: 8443, protocol: TCP}     # unprivileged; the Service maps 443
            - {name: iroh, containerPort: 7850, protocol: UDP}
            - {name: admin, containerPort: 9090, protocol: TCP}   # /metrics, /healthz
          readinessProbe: {httpGet: {path: /healthz, port: admin}}
          securityContext: {allowPrivilegeEscalation: false, readOnlyRootFilesystem: true, capabilities: {drop: ["ALL"]}}
          volumeMounts:
            - {name: data, mountPath: /var/lib/theuth-share-edge}
            - {name: config, mountPath: /etc/theuth-share-edge, readOnly: true}
            - {name: secrets, mountPath: /run/secrets/theuth-share-edge, readOnly: true}
      volumes:
        - {name: data, persistentVolumeClaim: {claimName: theuth-share-edge-data}}
        - {name: config, configMap: {name: theuth-share-edge}}
        - {name: secrets, secret: {secretName: theuth-share-edge, defaultMode: 0440}}   # group-readable via fsGroup
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata: {name: theuth-share-edge-data}
spec: {accessModes: [ReadWriteOnce], resources: {requests: {storage: 1Gi}}}
---
apiVersion: v1
kind: Service
metadata: {name: theuth-share-edge}
spec:
  type: LoadBalancer
  externalTrafficPolicy: Local
  selector: {app.kubernetes.io/name: theuth-share-edge}
  ports:
    - {name: tls, port: 443, targetPort: tls, protocol: TCP}
    - {name: iroh, port: 7850, targetPort: iroh, protocol: UDP}
---
apiVersion: v1
kind: ConfigMap
metadata: {name: theuth-share-edge}
data:
  config.yaml: |
    domain: tun.zip
    data_dir: /var/lib/theuth-share-edge
    listen: "[::]:8443"
    iroh: {listen: "[::]:7850", public_addrs: ["<LB IPv4>:7850", "[<LB IPv6>]:7850"]}
    admin: {socket: /var/lib/theuth-share-edge/admin.sock, http: "[::]:9090"}
    acme: {primary: letsencrypt, cas: {gts: {enabled: false}}}   # P1; GTS keys in P2
    dns: {provider: porkbun, zone: tun.zip, credential_file: /run/secrets/theuth-share-edge/dns.json, target: edge.tun.zip}
    # … the rest of §8.1
---
# Created out of band (kubectl create secret generic, or sealed-secrets), never committed;
# deploy/relay/k8s/rwx-dev/secret.yaml commits its HMAC, which is not a pattern to copy for a DNS token.
apiVersion: v1
kind: Secret
metadata: {name: theuth-share-edge}
type: Opaque
stringData:
  acme-gts-eab.json: '{"kid": "…", "hmac_key": "…"}'   # P2 only
  dns.json: '{"key": "pk1_…", "secret": "sk1_…"}'   # Porkbun key: tun.zip only, dns permission only (§6.4)
```

- Mixed TCP and UDP on one LoadBalancer Service is GA since Kubernetes 1.26, but some cloud load balancers refuse
  it; then two Services on one IP (provider-specific, e.g. MetalLB's `metallb.universe.tf/allow-shared-ip`), the
  split the repo already uses for iroh-relay's UDP (`deploy/relay/k8s/base/iroh-relay-service-quic.yaml#L1-L19`). If
  UDP ends on another IP, list that address in `iroh.public_addrs`.
- Pod egress is SNATed to the node, so the address iroh learns for itself is not the LB's; `iroh.public_addrs` puts
  the LB address into the ticket, and clients dial it. The edge never dials, so it should not need that address
  advertised for hole punching [INFERENCE: verify in P0 with a client behind NAT].
- The PVC holds `iroh.key` (pinned in shipped clients, D15), the name registry and the ACME accounts: back it up
  (VolumeSnapshot, or a nightly sqlite `.backup` plus the key and `acme/`). An empty new PVC is a key rotation.
- Upgrade: bump the image tag; `Recreate` drops tunnels after the edge sends `drain {secs}` (§5.3) on SIGTERM, and
  clients reconnect.
- Operator commands: `kubectl -n theuth exec deploy/theuth-share-edge -- theuth-share-edge admin …`.

**Any host.** The same binary runs as a plain process, a systemd unit (`AmbientCapabilities=CAP_NET_BIND_SERVICE` for
443, `StateDirectory=theuth-share-edge`, `LoadCredential=` for the Porkbun key, §8.3) or a compose service (`ports:
["443:8443/tcp", "7850:7850/udp"]`, a named volume for the data dir, the Porkbun key through `env_file` or a file secret),
on any box with a public IP, including a user's always-on machine beside its engine. Not on the same IP as
`deploy/relay/compose.yaml`: its Caddy already owns 443. No TCP 80 (§4.1).

- Health: the admin port serves `/healthz` and `/metrics` with `share_edge_{tunnels,routes,conns,bytes,cert_jobs,bans}`.
  opencode's [#65521](https://github.com/anomalyco/opencode/issues/65521) is a healthy control plane with dead visitor
  paths: probe the data path, not only the control plane. A canary hosting engine (any theuth install, e.g. a headless
  engine on another machine) keeps a static share on an operator-reserved name live, and an external check fetches it
  through 443.
- Operator commands (`theuth-share-edge admin …` over the admin socket): `list`, `queue`, `mesh <id>` (its names,
  engines, revoke list and rekey history), `ban <name|mesh|key|net> --reason`, `unban`, `release <name>` (starts the
  cooldown), `evict-unpublished`, `recover <name> --to-mesh <id> --ref <ticket>` (lost mesh key or a lost rekey race,
  §5.2, §5.7), `admission <invite|open|closed>`, `invite new [--uses N --expires
  7d]`, `caa-check`, `revoke-cert <name>`, `reload`, `pause`, `resume`. `theuth-share-edge ticket` prints the edge's
  ticket (endpoint id, `iroh.public_addrs`, home relay) for `share.edge`; `ticket --standby <pubkey>` prints the
  two-key form the release pins (§5.5).


## 11. Alternatives considered

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins.

### 11.1 Terminating edge (ngrok-style)

The edge holds certificates and keys, sees HTTP, and can show its own interstitial, rate-limit by path, log
requests, scan content, and offer OAuth gates. Rejected for v1: the operator — for the owner's public `tun.zip` edge, a third party to
every user of another mesh — reads every request and response of every shared app, which inverts doc 31's
position for a feature the user experiences as "my app on my machine"; and keys at the edge make a single breach
expose every tenant. What it buys (an interstitial that cannot be skipped, moderation) is an operator's need, not the
user's. Deferred, not foreclosed: a `mode: terminate` in the edge config could be added for operators who must moderate,
disclosed as such in the card. The owner confirmed blind for v1 on 2026-11-22. The same objection rejects a terminating *fallback* for engines
waiting for a certificate slot (§6.5 (d)).

### 11.2 opentunnel as a dependency

Its server and client are Rust and MIT. Rejected as a dependency: bearer tokens instead of engine keys, MySQL, a
Cloudflare/ZeroSSL-specific issuance path and a separate WebSocket transport beside the mesh (rev 1–4 also counted
"a separate service to deploy"; since rev 5 theuth's edge is one too, D2). Borrowed: the blind-SNI design, client-side keys, CSR reuse on renewal, the pre-order name check, every
limit in §6.5 and §8.1, PROXY protocol, the loopback warning, `drain`.

Re-evaluated 2026-11-22 for parts to borrow even without its transport (repo at `b1a27e3`, MIT). Only the client
crate `opentunnel` 0.6.2 is on crates.io (1,519 downloads); `opentunnel-server` is `publish = false`, so server code
could only be copied with its notice. Against §4.6: its `sni.rs` (a hand-written ClientHello parser, 308 lines) and
`acme.rs` (its own 959-line RFC 8555 client on `ring`) are what rustls and instant-acme give us; `dns.rs` is
Cloudflare- and hook-specific, replaced by zone-update; `csr.rs` (x509-parser `verify_signature`, then the SANs, 222
lines) and the client's `provision.rs` (rcgen key and CSR, 24 lines) make the same library calls we make and serve as
reference, too small to be a dependency. Worth copying as test data, with the MIT notice: the test helper that builds a
ClientHello for a given SNI and ALPN (`sni.rs`, `client_hello`) and the label vectors in `spec/vectors/names.json`.
Its other vectors (`frames.json`, `control.json`, …) describe its WebSocket protocol and do not apply.

### 11.3 Extending `theuth-relay` and the `/h/{id}` uplink

Reuses a deployed service, but adds state (names, cert jobs), a DNS credential and HTTP semantics to the one
component 31 keeps stateless and E2E; frames are 64 KiB with 256-deep queues and no per-stream flow control; the
relay has no iroh endpoint; and the uplink is per engine id with last-registrant-wins. Rejected. The edge copies the
relay's *shape* instead (its own binary, image and deploy dir, a client in the engine, §4.3), not its process.

### 11.4 Flat per-share subdomains, and two-level namespaces

`<random>.D` per share, HTTP-01 or TLS-ALPN-01 per name: no DNS credential at all, but one order per share, 30–120 s
before a share is live, and the 50-per-week limit for `D` (or per share with a PSL entry, which makes each share its
own cookie site — fine — but the order count still meets 300 per 3 h). The previous revision's `<random16>.<ns>.D`
(a random namespace per engine, a 16-character label per share) beside a separate flat reserved `<name>.D` gave two
kinds of names and URLs too long to read aloud; one name per user covers both. A per-name wildcard makes new shares
instant.

### 11.5 yamux over TLS/WebSocket as the primary transport

The research's recommendation for a standalone edge. For theuth it duplicates iroh's job, needs its own auth, and
brings head-of-line blocking across visitors on one TCP connection. Kept as the P3 fallback carrier only.

### 11.6 Third-party edges (Tailscale Funnel, Cloudflare Tunnel, ngrok SDK)

Funnel is blind too and its consent model (off by default, explicit approve, visible per-port state) is borrowed;
all three need the user's account with a vendor, and ngrok's SDK is not self-hostable. They could be alternative
"edges" behind the same tools later; not v1.

### 11.7 Building on pingora, rpxy or sozu

pingora's dynamic certificates need OpenSSL or BoringSSL and its process model fights embedding; rpxy is not on
crates.io; sozu-lib is AGPL. The blind edge needs none of them: it is an accept loop, a ClientHello parse and a copy.

### 11.8 Caddy with on-demand TLS as the edge

Caddy's [on-demand TLS](https://caddyserver.com/docs/automatic-https#on-demand-tls) obtains a certificate during the
first TLS handshake for a name, gated by an `ask` endpoint the edge would serve from its name table, and its admin API
adds routes live; it would work. Rejected: Caddy issues and holds the private keys, so the edge would terminate and
decrypt all traffic (D1, §11.1), and issuance at handshake time solves a problem this design does not have, because
every name is known at claim time and every share at publish time. Its `ask` pattern survives in the P3 custom-domain
check (§6.7).

### 11.9 Per-hostname certificates over TLS-ALPN-01 (certificate option A)

Each share hostname gets its own certificate, validated with TLS-ALPN-01 ([RFC 8737](https://www.rfc-editor.org/rfc/rfc8737))
answered by the edge itself: the CA's validation connection carries ALPN `acme-tls/1` in its ClientHello, which the
SNI router sees and diverts to a challenge responder instead of the tunnel. It needs no DNS token and only one `*.D`
record (with no challenge TXT, no empty non-terminal arises, §6.4). Rejected: every new sub-label costs a certificate
slot from the 50-per-week budget (§6.5) and a 10–30 s wait for an order before the share is live, which defeats
"share anything under your name". Kept for custom domains (§6.7), where the edge holds no token for the customer's zone.

### 11.10 The edge as authoritative DNS for `D` (certificate option C)

The edge answers for `D` itself with `hickory-server` behind an NS delegation: records synthesized from the name
table, challenge TXT served from memory (no propagation wait, no DNS API token, no record quota). Costs: inbound
UDP/TCP 53 and a DNS DoS surface on the edge; a second authoritative server that also sees the dynamic TXT
(registries expect at least two NS [INFERENCE: usual registry rule, not checked for `.zip`]), so either both run edge
code or a hidden primary feeds secondaries by NOTIFY/IXFR; DNSSEC signing if wanted; and the edge becomes the zone's
availability. Rejected for P1; kept as a P2 option if the provider's record quota or the token's scope becomes a
problem.

### 11.11 `dns-update` instead of `zone-update`

`dns-update` 0.6.2 (Stalwart, Apache-2.0 OR MIT, ~117k downloads) was rev 3's choice and supports more providers,
including Route 53, Google Cloud DNS and RFC 2136 with TSIG; the owner chose `zone-update` (2026-11-22), and the
`DnsZone` trait (§6.1) keeps a later switch contained.

### 11.12 The edge as a theuth engine role (rev 1–4)

Rev 1–4 made the edge a role of any engine: a `share_edge:` section in `config.yaml`, an `EngineStats.share_edge`
advertisement, a `ShareGate` with an installed `ShareEdgeAlpn` in the mesh router, the DNS token in a non-replicated
`local/` vault prefix, and `theuth --headless` as the dedicated deploy. It reused the engine's endpoint, sqlite, vault
and RPC. Rejected by the owner on 2026-11-22 for the standalone service (D2): it put the whole engine (plugins, vault,
models, and on a family node the family's replicated data) behind the public 443 listener; it tied edge upgrades to
theuth releases and theuth's cadence to a public service; a headless engine is a poor Kubernetes workload, its config,
data dir and plugin set being built for a user's machine; and it made the mesh carry a server role only operators
need. What it would have shared (iroh, sqlx, axum) are libraries; the standalone binary links them directly.

### 11.13 Owner plus the bound thread user may approve a publish

Slack and Telegram already let "owner or the card's thread user" answer an approval
(`crates/slack/src/channel.rs#L1077-L1092`, `crates/telegram/src/channel.rs#L2103-L2108`), and keeping that rule for
`share_publish` would let a teammate in an owner-allowed thread publish from it without waiting for the owner.
Rejected (owner, 2026-11-23): the thread user would be approving exposure of the owner's machine to the internet, which
no thread binding was meant to grant; it would need its own per-user "may publish" grant and its own audit line.
The owner-only predicate is one line per channel, so adding the grant later is additive.

### 11.14 A dedicated iroh endpoint for share

A second `Endpoint` with its own key would isolate the share transport from the mesh (its own stream limits,
windows and relay connection). Rejected: the edge identifies an engine by the key its delegation names (§5.7), so a
second key needs a persisted file, its own delegation and a second relay connection and hole punching. The isolation
it buys is available per connection: iroh 1.4.1's `Endpoint::connect_with_opts` takes
`ConnectOptions::with_transport_config`, which replaces the endpoint's transport config for that one connection (iroh
`src/endpoint.rs` L1146–L1149 and L1798–L1813), so the mesh's `presets::Minimal` endpoint
(`crates/mesh/src/net.rs#L546-L554`) stays as it is (D18).

### 11.15 Names per engine (rev 1–6)

Rev 1–6 bound each name to one engine's key, with its TLS key a 0600 file that never left that machine. Rejected by
the owner on 2026-11-23 ("Name per mesh for sure", D21). Against it: a user with three machines got three names and
three certificate slots, and `sky.tun.zip` could carry only the shares of the one machine that held it, so `app.sky`
on the laptop and `docs.sky` on the desktop was impossible; a new or reinstalled machine needed a signed handover
(rev 6's M7 answer) and a reissue; and the per-key quotas let one owner with several machines look like several
users. What it had over per-mesh, and what rev 7 pays for instead: a name key on one machine only, so one compromised
machine exposed one name, not all of them (§9.2); no delegation format or revocation (§5.7); no route conflicts
between machines and no HTTP/2 coalescing across engines (§5.4); and the edge did not learn which engines belong to
one owner (§9.1).


### 11.16 An owner-held recovery code for lost rekey races (rejected, owner 2026-11-23)

The final review (F-M6) proposed storing `recovery_pub` at the edge when the mesh first registers: the hash of a code
shown once to the owner and never written to the vault, which `admin recover` would then require. It would give the
operator a cryptographic tiebreak between a thief and the owner, both of whom hold the old mesh share key. Rejected
by the owner in favour of operator judgement (main §5.7): the code must be kept by the owner outside every machine
for years and is lost about as often as it is needed; a mesh registered on several edges needs it at each; and the
case it serves (a thief who rekeys before the owner unpairs) is narrowed by prompt unpairing and the immediate
revoke. Additive later: an optional `recovery_pub` field in a P2 `hello` under §5.3's compatibility rule.

### 11.17 A dedicated Porkbun account for `tun.zip` (optional, not required)

Rev 7 recommended moving `tun.zip` to a Porkbun account that holds only it, so that an account compromise could not
reach the owner's other domains and the account's one API key was the edge's. The owner keeps the current account
(2026-11-23). The edge's exposure is the same either way: its key is restricted to `tun.zip`, the `dns` scope and the
edge's source IPs (main §6.4), and cannot touch nameservers, DNSSEC, contacts or transfers (the `domains` scope). A
dedicated account remains an option if the owner's account ever holds keys or users that should not sit beside the
tunnel zone; moving means a Porkbun account change for the domain, not a registrar transfer [INFERENCE: Porkbun
offers account-to-account moves; not checked].

### 11.18 GTS `dns-persist-01` as the P1 primary (deferred to P2)

Rev 7 shipped Path P as the primary in P1, with Let's Encrypt as overflow. Its advantages are real: no DNS write per
order, no per-name records for GTS names (so the Porkbun 2,500-record cap stops bounding the edge), and no
per-registered-domain weekly cap (100 `newOrder` per hour per project). The owner moved it to P2 to test with Let's
Encrypt first and to avoid a Google Cloud project and EAB before launch. Costs that remain for P2: Google holds an
account that can issue any `<name>.D` with no DNS write; the method is a draft whose record format may change; the
accounting of main §6.0 (renewal priority, overflow only on failure or shorter ETA, LE names moved back at renewal,
GTS-over-`dns-01` as first fallback). Let's Encrypt itself had `dns-persist-01` in staging (draft-00) with a
production date that slipped from Q2 2026
([community thread](https://community.letsencrypt.org/t/dns-persist-01-deployment-status-and-timeline/246468)),
and Boulder reportedly does not let a parent record validate subdomains
([CertKit](https://www.certkit.io/blog/whats-taking-dns-persist-01-so-long)), which this design needs.

## 14. Review dispositions

### 14.1 First review (rev 6)

> Moved from rev 7 of the main doc with its wording; code anchors re-checked on 2026-11-23. Where it disagrees with the main doc (rev 8), the main doc wins.

Findings of `docs/design/210-review-fable.md` (Fable 5.2, on rev 5) and where rev 6 answers them.

| finding | disposition |
|---|---|
| B1 human-only bypassable | Folded in. Owner-only on every channel, refusal in non-owner threads, computer-use and script tools refuse theuth's windows while a human-only card is pending, the share RPC table, Settings actions through the same approval row, the RPC-token residual and its P2 fix: G3, D10, §5.1, §7.1, §7.2, §9.2; rejected alternative §11.13; confirmed by the owner in rev 7 (former §12 Q3). Signal was already owner-only (`crates/signal/src/commands.rs#L136-L139`). |
| B2 `open` unbounded | Folded in. Lazy allocation (`POST /v1/names`), global names cap with "edge full" and eviction of never-published names, an invite share of the weekly slots, per-key and per-network byte, bandwidth and connection-rate budgets, a paced relayed-newcomer bucket, the provider record ceiling, invite-first launch with measured gates: D4, D19, §5.2, §5.3, §6.4, §6.5, §8.1, §9.4, §9.5. Rev 7: the bounds stay, counted per mesh, and the owner chose an `open` launch with `invite` as the brake (§9.5). |
| M1 `dns-persist-01` | Folded in as Path P beside Path Z, with costs, P0 verifications and the trait shape: §6.0, D6, D7, D17, §8.2, §10. Rev 7: both paths ship in P1, Path P primary, Path Z over Porkbun as overflow and fallback. |
| M2 tenant-driven issuance | Folded in. The edge resets ClientHellos offering `acme-tls/1`; CAA with `accounturi` and `validationmethods` for every CA; the edge refuses `open` without CAA: §4.2, §6.7, §8.2, §9.2. |
| M3 stolen DNS token | Folded in. Claim corrected, exact token permissions, zone-update panics contained in the job worker: §6.1, §8.2, §9.2, §13. |
| M4 stream limit on the wrong side | Folded in. Per-connection transport config through iroh's `connect_with_opts` (D18 kept, the mesh endpoint's config untouched), an edge-side stream counter and an `open_bi` timeout: D18, §4.2, §5.3; dedicated endpoint rejected in §11.14. |
| M5 `join_network` side effects | Folded in: `Net::dial_alpn` dials the ticket's addresses without `join_network`, so nothing is published: D18, §5.3, §5.5. |
| M6 source safety | Folded in. Per-request canonicalisation and containment for `dir`, per-connect port and process-tree check, `Location` rewrite: D9, §5.6, §9.2. |
| M7 no handover | Folded in, then mostly superseded by rev 7: a new machine of the same mesh gets a delegation and the name key from the vault, so no handover is needed (D20, D21, §5.7); transfer remains for moving a name to another mesh, online in P1 and signed offline in P2; lost-mesh-key recovery is operator-assisted (§5.2, §5.3, §8.4; §12 Q5). |
| M8 G4 unmet | Folded in. TUI status line and `/share stop`, channel pinned message and `/share stop`, hidden-window badge and OS notifications in P1: D12, §7.4, §10. |
| M9 pre-PSL isolation | Folded in. Same-site CSRF and password-manager offers in §6.6, §9.2 and the disclosure; "never listed" is the planning case. |
| M10 first run | Folded in. §7.6 flow; explicit `terms_rev`, accepted revision in `hello`: §5.3, §7.3, §8.1. |
| M11 P1 too large | Folded in. Hidden work added to §10; cuts listed in §10 "Moved out of P1". |
| m1 TTL risk | Folded in: wait 60 s between authorizations under Path Z (§6.1). |
| m2 `mesh` → `share-proto` | Folded in: generic `Net::dial_alpn`; the default ticket lives in `crates/share` (D14, D15, §5.3). |
| m3 `vault_local` | Folded in: dropped; 0600 files and a non-replicated table (D8, §5.1, §8.3). Rev 7 moves the name keys into replicated `share/` vault rows, because every family machine must serve the name (D8, §6.2). |
| m4 single default key | Folded in: two pinned keys (D15, §5.5, §13). |
| m5 SOA negative TTL | Folded in as unverified: plan for up to 3600 s, measured in P0 (§6.4). The 1800 s figure was not confirmed. |
| m6 env/config flips the switch | Folded in: `enabled` and `edge` live only in the plugin's store (D11, §8.1). |
| m7 missing numbers | Folded in: global connection and fd caps, pre-`hello` cap and timeout, argon2 concurrency (§8.1, §9.4, §7.5). |
| m8 zone-update lookups | Folded in: P0 measures wildcard-name create, read and delete (§6.1, §10). |
| m9 expiry and attach presence | Folded in: warnings before expiry; attach only while needed (§5.2, §5.5). |
| m10 h2 coalescing | Folded in: no longer moot under rev 7, where several engines serve one name; every hosting engine answers `421` for hosts it does not route, h2 kept, with an HTTP/1.1-only fallback per name if P0's browser check fails (§4.2, §5.4). |
| m11 workspace version | Folded in: own version (D14, §8.4). |
| m12 DNSSEC | Folded in: §8.2, §13. |
| N1 "through the relay" | Folded in: marked as the owner's words (§1). |
| N2 random-name lifetime | Folded in: D5 matches §5.2. |
| N3 `KIND` and env prefix | Folded in: relay precedent named (§5.5, §8.1). |
| N4 two stop entry points | Folded in: §7.1, §7.4. |
| N5 transport-config wording | Folded in with M4 (§5.3). |

Review claims not taken as written: Signal is already owner-only (above); `iroh` 1.4.1 already runs on `noq`, not
quinn (its default is also 100 incoming bidirectional streams), and iroh offers a per-connection transport config,
so M4 needs neither a mesh-wide change nor a second endpoint (§4.4, D18).

### 14.2 Rev 7 owner decisions (not review findings)

**Rev 7 (2026-11-23).** "Name per mesh for sure": D21 is new; D4, D5, D7,
D8, D12, D14, D18 and D20 change; §5.7 is new (mesh share key, delegation, rekey and revoke list); §4, §5.1–§5.4,
§6.2, §6.3, §7, §8, §9, §10, §11.14 and §12 follow; per-engine names are §11.15. The same day's answers to §12 Q1,
Q2, Q3, Q4 and Q6 put both validation paths in P1 with Porkbun as Path Z's DNS (D6, D7, §6, §8, §13), launch the
public edge `open` with `invite` as the brake (D4, D19, §9.5), and confirm owner-only approval (D10).

### 14.3 Final review (`docs/design/210-review-fable-final.md`, on rev 7)

| id | disposition (main doc sections unless noted) |
|---|---|
| F-M1 | Fixed: `GET /v1/info` before `hello` (§5.3), `Delegation.domain` from it, `403 {code: delegation_not_yet_valid \| delegation_expired, now_ms}`, holder signs `issued_ms = min(own, edge now)`, "clock is off" in Settings and on the card (§5.7, §5.9, §7.6); P0-7. |
| F-M2 | Fixed: every tombstone after `mesh/share_key` exists triggers a rekey at every edge in `share_edges`; `GET /v1/mesh` only supplies the revoke list (§5.7). |
| F-M3 | Fixed: rekey state machine (`revoking` → `waiting_vault` gated on `KeyInfo.members` → `writing` → `settling` with `synced()` and re-read → `sending` the settled value → `confirming`; `contested`), local 0600 stash, replicated `share_rekeys` row with retry, chained `Rekey` to heal (§5.7, §5.8). Dual-holder guard credited to the lease predicate, not the CAS (§5.7). P0-6. |
| F-M4 | Fixed: name keys created through the lease holder (`share.name_key.ensure`), `welcome.names[].spki`, `409 {code: csr_mismatch, spki}`, heal rotation after `csr_heal_secs` (§6.2). P0-6. |
| F-M5 | Fixed per owner answer 4: D22, replicated `share_edges`, holder duties regardless of the local switch, only on removal events, Settings disclosure (§5.7, §9.1). |
| F-M6 | Owner answer 2: operator judgement, no recovery code; evidence the operator may weigh, indistinguishability, mitigations (§5.7); recovery code rejected (appendix §11.16). |
| F-M7 | Re-scoped by owner answer 1: P1 is LE-only, so the two-CA accounting applies to P2; kept concisely in §6.0 (GTS loss, overflow only on failure or shorter ETA, renewals first in the GTS bucket, LE names back to GTS); fallback order is §12 Q19. |
| F-M8 | Fixed: global paced bucket for open new names, IPv6 /32 and ASN tiers, names counted at slot time with 1 h eviction of allocated names without a CSR, `first_live_ms` on first visitor bytes, egress throttle for non-invited meshes, brand blocklist, Safe Browsing and Search Console as launch prerequisites and brake triggers (§9.4, §9.5, §9.6). Relayed-bucket residual stated (§9.4). |
| F-M9 | Fixed: P0-1…P0-9 with pass lines, decisions and owner inputs; P1 milestones M1–M4 with exit criteria; both DDLs (§5.8), slot-queue order and ETA (§6.5), metering and shaping (§9.6), reconnect state machine (§5.9), admin-socket protocol (§8.5). Milestone order follows the owner's brief (M1 single machine + Path Z + consent UI), not the review's Path P-first order. |
| F-m1 | Fixed: `Delegation.serial` dropped (§5.7). |
| F-m2 | Fixed: election runs over connected peers, so a partitioned machine elects itself and self-issues; no 12 h cliff for an honest machine; the rekey is the boundary (§5.7). |
| F-m3 | Fixed: `theuth-share-edge acme register` prints account URI and CAA lines before the first `run` (§8.5); P0-9. |
| F-m4 | Fixed: the PSL check is no longer a spike; it is P2 entry check 5, answered from GTS support or the Baseline Requirements (§6.0, §12 Q10). |
| F-m5 | Fixed per owner answer 3: conditional registrar text removed; one Porkbun statement (§6.4); dedicated account optional (appendix §11.17). |
| F-m6 | Fixed: budgets say mesh (§4.2, §9.4); "ban by name, mesh, engine key or network" (§9.2); `delegation_max_ttl` top level (§5.7, §8.1); invite-first is history only (revision history, §14.1); §6.5 no longer argues GTS then reverses; the PSL request is listed with Q10's hold (§13). |
| F-m7 | Fixed: `browser_*` acting calls refuse the engine's own web-client origins while a human-only card is pending (§7.2, §9.2). |
| F-m8 | Fixed: `dir` opens the canonical path with no symlink following and an inode check; `ServeDir` not used for `dir` (§5.6). |
| F-m9 | Fixed: per-network certificate cap applies to open meshes only (`certs_per_net_week: 2`); invited meshes leave every per-network count (§6.5, §9.4). |
| F-m10 | Answered: Pebble merged draft `dns-persist-01` on 2026-02-17 (PR #536); P0-8 checks it, else Path P is tested only through the fake `Ca` (§6.0, §10). |
| F-N1 | Fixed: vault anchors corrected to `vault.rs#L458-L481`, `#L704-L718`, `#L249-L256`, `#L258-L268`, `#L618-L627`, `#L48`, `#L453-L457`; the `write_row` comment is `#L894-L896`; applied in the main doc and in text moved here. |
| F-N2 | Fixed: `replica::attach` is `crates/secrets/src/lib.rs#L900`; also `platform::files::create_private` is `files.rs#L60-L63`. |
| F-N3 | Fixed: `share-proto` also needs a base64 encoder (`data-encoding`) and a serde helper for `[u8; 64]` (§5.7). |
| F-N4 | Fixed: Porkbun sandbox used for P0-1 (DNS simulated, so P0-2 uses the scoped live key); restore points used in the runbook; the DNSSEC claim corrected (§6.4, §8.2, appendix §6.1). |
| F-N5 | Done in part: D-rows ≤ 2 sentences, single statements of lazy allocation, Porkbun scoping and `421`, narration stripped, background moved here. The main doc is about 1,450 lines, above the ~1,100 target, because F-M9's DDLs and state machines were added. |
