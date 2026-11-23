# 210 as built — CONTRACT phase (`share-proto` + empty homes)

Design: `docs/design/210-public-share.md` rev 8. This phase builds the wire contract every later builder codes
against, plus empty crates for the edge and the client plugin.

## Files

| path | what |
|---|---|
| `Cargo.toml` (root) | workspace entries: `share-proto`, `share`, `iroh-base` (`key`), `iroh-tickets` 1.0.0, `postcard` 1.1.3 (`use-std`), `x509-parser` 0.18.1 (`verify-aws`), `rcgen` 0.14.10 (`aws_lc_rs`, `pem`), and for later phases `instant-acme` 0.8.5 (`aws-lc-rs`, `hyper-rustls`, `time`, `x509-parser`), `zone-update` =0.12.3 (`async`, `porkbun`), `ppp` 2.3.0, `governor` 0.10.4 |
| `Cargo.lock` | +13 packages (`iroh-tickets`, `x509-parser` and its asn1 stack, `rcgen`, `pem`, `yasna`) |
| `crates/share-proto/` | the contract (below) |
| `crates/share-edge/Cargo.toml`, `src/main.rs` | bin `theuth-share-edge`, own `version = "0.1.0"`; clap `--version`/`--help` only |
| `crates/share/Cargo.toml`, `src/lib.rs` | plugin crate, doc comment only |

`share-proto` dependencies: `serde`, `serde_json`, `postcard`, `iroh-base`, `iroh-tickets`, `x509-parser`,
`data-encoding`, `sha2`, `thiserror`; dev: `rcgen`. No workspace crate.

Verification: `cargo test -p share-proto` 24 passed; `cargo clippy -p share-proto -p share-edge -p share
--all-targets -- -D warnings` clean (it also covers `cargo check` of all three); `cargo tree -p share-proto -e
normal` lists no workspace crate but itself.

## Public API of `share-proto`

Root re-exports: `api::*`, `ErrorBody`, `ErrorCode`, `Event`, `KillReason`, `CsrError`, `CsrInfo`, `SpkiHash`,
`check_csr`, `expected_sans`, `HostParts`, `NameError`, `NameKind`, `Delegation`, `DelegationError`,
`EdgeIdentity`, `Rekey`, `RekeyError`, `Retire`, `Signed`, `SignedRekey`, `Invite`, `ShareEdgeTicket`, `ALPN`,
`ALPNS`, `PROTOCOL_MAJOR`, `Version`, and `iroh_base::{EndpointAddr, PublicKey, SecretKey}`.

### `version`

```rust
pub const ALPN: &[u8] = b"theuth/share/1";
pub const ALPNS: &[&[u8]] = &[ALPN];            // newest first; edge registers all, client offers all
pub const PROTOCOL_MAJOR: u32 = 1;
pub const EDGE_AGENT_PREFIX: &str = "theuth-share-edge/";
pub const CLIENT_AGENT_PREFIX: &str = "theuth/";
pub fn edge_agent(version: &str) -> String;     // "theuth-share-edge/<ver>"
pub fn client_agent(version: &str) -> String;   // "theuth/<ver>"
pub struct Version { pub major: u64, pub minor: u64, pub patch: u64 }   // FromStr, Display, Ord; ignores -pre/+build
pub struct VersionError(pub String);
pub fn parse_agent(agent: &str) -> Result<Version, VersionError>;      // "theuth/x.y.z" or bare
pub fn client_ok(client: &str, min_client: &str) -> bool;              // false → 426 client_too_old
```

### `paths` (in `lib.rs`)

```rust
pub const INFO = "/v1/info"; HELLO = "/v1/hello"; NAMES = "/v1/names"; NAME = "/v1/names/{name}";
NAME_TRANSFER = "/v1/names/{name}/transfer"; NAME_CSR = "/v1/names/{name}/csr"; NAME_CERT = "/v1/names/{name}/cert";
ROUTES = "/v1/routes"; EVENTS = "/v1/events"; MESH = "/v1/mesh"; MESH_STOP = "/v1/mesh/stop";
MESH_REVOKE = "/v1/mesh/revoke"; MESH_REKEY = "/v1/mesh/rekey";
pub fn name(name: &str) -> String; pub fn name_transfer(name: &str) -> String;
pub fn name_csr(name: &str, rotate: bool) -> String;   // adds ?rotate=true
pub fn name_cert(name: &str) -> String;
```

### `limits` (in `lib.rs`)

```rust
pub const MAX_JSON_BODY: usize = 64 * 1024;
pub const VISITOR_STREAMS_PER_TUNNEL: u32 = 256;
pub const OPEN_BI_TIMEOUT_MS: u64 = 5_000;
pub const DELEGATION_TTL_MS: u64 = 24 h;  DELEGATION_MAX_TTL_MS: u64 = 25 h;
pub const DELEGATION_SKEW_MS: u64 = 5 min; DELEGATION_RENEW_MS: u64 = 12 h; CLOCK_WARN_MS: u64 = 5 min;
```

### `api` (bodies of §5.3; field names exactly the design's)

```rust
pub enum Admission { Open, Invite, Closed }                                   // "open" | "invite" | "closed"
pub struct Info { v: u32, edge: String, domain: String, edge_id: PublicKey, now_ms: u64, min_client: String,
                  admission: Admission, terms_url: String, terms_rev: String, abuse_url: String, full: bool }
pub struct Hello { v: u32, delegation: Signed, invite: Option<Invite>, client: String, terms_rev: Option<String> }
pub enum NameState { Allocated, Queued, Slotted, Valid, Released, Banned }
pub struct QueuePos { position: u32, eta_s: u64 }
pub struct NameInfo { name: String, kind: NameKind, state: NameState, ca: Option<String>,
                      spki: Option<SpkiHash>, not_after: Option<u64>, queue: Option<QueuePos> }
pub struct RouteEntry { host: String, engine: PublicKey }
pub enum InterstitialRule { Optional /*default*/, Required }
pub struct WelcomeLimits { interstitial: InterstitialRule, random_idle_ms: u64, reserved_idle_ms: u64,
                           expiry_warn_ms: Vec<u64>, reclaim_cooldown_ms: u64, reserved_per_mesh: u32,
                           engines_per_mesh: u32, routes_per_tunnel: u32, conns_per_tunnel: u32,
                           delegation_max_ttl_ms: u64 }                       // #[serde(default)], Default = design defaults
pub struct Welcome { v: u32, now_ms: u64, min_client: String, mesh: PublicKey, names: Vec<NameInfo>,
                     routes: Vec<RouteEntry>, revoked: Vec<PublicKey>, stop_all_ms: Option<u64>, domain: String,
                     limits: WelcomeLimits, terms_url: String, abuse_url: String, terms_rev: String,
                     terms_ok: bool, full: bool, retire: Option<Signed> }
pub struct NameAvailability { available: bool, reason: Option<String> }        // GET /v1/names/{name}
pub struct NameRequest { name: String }                                        // POST /v1/names
pub struct NameGranted { name: String }                                        // 201 POST, 200 PUT
pub struct NameReleased { name: String, reclaim_after_ms: u64 }                // 200 DELETE
pub struct TransferRequest { to_mesh: PublicKey }
pub struct TransferAccepted { transfer_until_ms: u64 }
pub struct RoutesPut { hosts: Vec<String> }                                    // 204
pub struct CsrQuery { rotate: bool }                                           // ?rotate=true; body = CSR DER
pub struct CertChain { chain_pem: String, not_after: u64 }                     // 200 csr, 200 cert
pub struct CertPending { eta_s: u64, queue_pos: Option<u32> }                  // 202 csr
pub struct MeshEngine { engine: PublicKey, attached: bool, last_ms: u64, revoked_ms: Option<u64> }
pub struct MeshStatus { mesh: PublicKey, engines: Vec<MeshEngine>, routes: Vec<RouteEntry>, stop_all_ms: Option<u64> }
pub struct StopAll { stop_all_ms: u64 }
pub struct RevokeRequest { engines: Vec<PublicKey> }
pub struct RevokeResult { revoked: Vec<PublicKey> }
pub struct RekeyAccepted { mesh: PublicKey }                                   // body: SignedRekey
```

All fields `pub`; all types `Serialize + Deserialize + Debug + Clone + PartialEq + Eq`. Option fields are
`#[serde(default, skip_serializing_if = "Option::is_none")]`. Unknown JSON fields are ignored (serde default).
`PublicKey` serialises as iroh's string form in JSON, 32 raw bytes in postcard. Times are Unix ms.

### `error`

```rust
pub enum ErrorCode {   // snake_case on the wire; #[serde(other)] Unknown
    Banned, NetworkQuota, AdmissionClosed, InviteRequired, BadDelegation, Revoked, EnginesQuota,
    DelegationNotYetValid, DelegationExpired, MeshRekeyed, ClientTooOld, NameTaken, Cooldown, Quota,
    TermsRequired, Paced, EdgeFull, Invalid, NotOwner, RouteConflict, CsrMismatch, CsrNames, BadSignature,
    HelloRequired, NotFound, RateLimited, TooLarge, Internal, Unknown }
impl ErrorCode { pub fn status(self) -> u16; pub fn as_str(self) -> &'static str }   // + Display
pub struct ErrorBody { code: ErrorCode, now_ms: Option<u64>, to: Option<PublicKey>, retry_after_s: Option<u64>,
                       reason: Option<String>, conflicts: Vec<RouteEntry>, spki: Option<SpkiHash>,
                       reclaim_after_ms: Option<u64>, message: Option<String> }
impl ErrorBody { pub fn new(code) -> Self; with_now(code, now_ms); invalid(reason); rekeyed(to: PublicKey);
                 paced(retry_after_s); route_conflict(Vec<RouteEntry>); csr_mismatch(SpkiHash); status(&self) -> u16 }
```

Status map: 400 `invalid`; 403 `banned network_quota admission_closed invite_required bad_delegation revoked
engines_quota delegation_not_yet_valid delegation_expired quota terms_required not_owner csr_names bad_signature
hello_required`; 404 `not_found`; 409 `mesh_rekeyed name_taken cooldown route_conflict csr_mismatch`; 413
`too_large`; 426 `client_too_old`; 429 `paced rate_limited`; 500 `internal unknown`; 503 `edge_full`.

### `events` (SSE of `GET /v1/events`)

```rust
pub struct QueueEvent { name: String, position: u32, eta_s: u64 }
pub struct CertEvent { name: String }
pub struct RoutesEvent { host: String, engine: Option<PublicKey> }
pub struct StatsEvent { host: String, conns: u64, bytes: u64 }
pub struct DrainEvent { secs: u64 }
pub enum KillReason { EdgeBan, Budget, StopAll, Revoked, MeshRekeyed, DelegationExpired, Unknown /*serde other*/ }
pub struct KillEvent { host: Option<String>, reason: KillReason }
pub enum Event { Queue(QueueEvent), Cert(CertEvent), Routes(RoutesEvent), Stats(StatsEvent), Drain(DrainEvent), Kill(KillEvent) }
impl Event {
    pub fn name(&self) -> &'static str;     // SSE `event:` — "queue" | "cert" | "routes" | "stats" | "drain" | "kill"
    pub fn data(&self) -> String;           // SSE `data:` JSON
    pub fn parse(name: &str, data: &str) -> Result<Option<Event>, serde_json::Error>;   // None = unknown name
}
```

### `signed` (§5.7)

```rust
pub const DELEGATION_TAG: &[u8] = b"theuth/share/delegation/1\0";
pub const REKEY_TAG: &[u8] = b"theuth/share/rekey/1\0";
pub const RETIRE_TAG: &[u8] = b"theuth/share/retire/1\0";
pub struct Signed { body: Vec<u8> /* postcard */, sig: [u8; 64] }        // JSON {body, sig}, both base64url-nopad
pub struct EdgeIdentity { edge: PublicKey, domain: String }

pub struct Delegation { v: u8, domain: String, edge: PublicKey, mesh: PublicKey, engine: PublicKey,
                        issued_ms: u64, expires_ms: u64 }
impl Delegation {
    pub const VERSION: u8 = 1;
    pub fn new(domain, edge, mesh, engine, holder_now_ms: u64, edge_now_ms: u64) -> Self;  // issued = min, +24 h
    pub fn sign(&self, mesh_key: &SecretKey) -> Result<Signed, DelegationError>;          // KeyMismatch if not `mesh`
    pub fn open(signed: &Signed) -> Result<Delegation, DelegationError>;                  // decode + sig under body.mesh
    pub fn verify(signed: &Signed, peer: &PublicKey, edge: &EdgeIdentity, now_ms: u64, max_ttl_ms: u64)
        -> Result<Delegation, DelegationError>;   // sig, engine == peer, edge, domain, ttl ≤ max, issued−5min ≤ now < expires
    pub fn clock_skew_ms(holder_now_ms: u64, edge_now_ms: u64) -> i64;
    pub fn clock_off(holder_now_ms: u64, edge_now_ms: u64) -> bool;                       // |Δ| > 5 min
}
pub enum DelegationError { Decode, Version(u8), KeyMismatch, BadSignature, EngineNotPeer, WrongEdge, WrongDomain,
                           Lifetime, NotYetValid { now_ms }, Expired { now_ms } }
impl DelegationError { pub fn code(&self) -> ErrorCode; pub fn now_ms(&self) -> Option<u64> }

pub struct Rekey { v: u8, domain: String, edge: PublicKey, old: PublicKey, new: PublicKey,
                   revoke: Vec<PublicKey>, issued_ms: u64 }
pub struct SignedRekey { body: Vec<u8>, sig_old: [u8; 64], sig_new: [u8; 64] }        // JSON base64url
impl Rekey {
    pub const VERSION: u8 = 1;
    pub fn sign(&self, old: &SecretKey, new: &SecretKey) -> Result<SignedRekey, RekeyError>;
    pub fn verify(signed: &SignedRekey, edge: &EdgeIdentity) -> Result<Rekey, RekeyError>;   // stateless checks only
}
pub enum RekeyError { Decode, Version(u8), KeyMismatch, SameKey, WrongEdge, WrongDomain, BadOldSignature, BadNewSignature }
impl RekeyError { pub fn code(&self) -> ErrorCode }       // signatures → bad_signature, else invalid

pub struct Retire { v: u8, edge: PublicKey /* signing standby */, retired: PublicKey, issued_ms: u64 }
impl Retire {
    pub const VERSION: u8 = 1;
    pub fn sign(&self, standby: &SecretKey) -> Result<Signed, RetireError>;
    pub fn verify(signed: &Signed, connected: &PublicKey) -> Result<Retire, RetireError>;
}
pub enum RetireError { Decode, KeyMismatch, BadSignature }
```

Signature = Ed25519 (`iroh_base::SecretKey::sign`) over `TAG ‖ body`; `body` = postcard of the record. The
verifier never re-encodes.

### `ticket` (§5.5)

```rust
pub struct Invite(pub [u8; 16]);   // JSON base64url, postcard raw; Debug redacts
impl Invite { pub fn hash(&self) -> [u8; 32] }           // sha256, the edge's invites.hash
pub struct ShareEdgeTicket { pub addr: EndpointAddr, pub invite: Option<Invite> }
impl iroh_tickets::Ticket for ShareEdgeTicket { const KIND = "theuthedge"; .. }  // "theuthedge" + lowercase base32(postcard)
impl ShareEdgeTicket { pub fn edge_id(&self) -> PublicKey }
// Display / FromStr (trims) / Serialize / Deserialize as the ticket string
```

### `names` (§5.2, §5.4, §5.1)

```rust
pub const RESERVED_MIN_LEN = 3; RESERVED_MAX_LEN = 32; RANDOM_LEN = 6; LABEL_MAX_LEN = 63; PATH_MAX_LEN = 64;
pub const RANDOM_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
pub const BUILTIN_RESERVED_WORDS: &[&str];   // www api admin login mail abuse status support secure account theuth edge ns1 ns2
pub enum NameKind { Random, Reserved }
pub enum NameError { Length, Charset, Hyphen, Idna, Reserved, Brand, Path }
impl NameError { pub fn reason(self) -> &'static str }   // the `reason` of 400 invalid
pub fn check_label(label: &str) -> Result<(), NameError>;                          // [a-z0-9-]{1,63}, LDH, no xn--
pub fn check_reserved<S: AsRef<str>>(name: &str, operator_words: &[S]) -> Result<(), NameError>;
pub fn check_random(name: &str) -> Result<(), NameError>;
pub fn random_name(bytes: [u8; RANDOM_LEN]) -> String;                             // caller supplies randomness
pub fn check_brand<S: AsRef<str>>(name: &str, brands: &[S]) -> Result<(), NameError>;
pub fn check_path(path: &str) -> Result<(), NameError>;
pub struct HostParts { pub name: String, pub label: Option<String> }
pub enum HostError { NotUnderDomain, Apex, TooDeep, Label(NameError) }
pub fn parse_host(host: &str, domain: &str) -> Result<HostParts, HostError>;     // case-insensitive, trailing dot ok
pub fn host(name: &str, label: Option<&str>, domain: &str) -> String;
pub fn share_url(host: &str, path: &str) -> String;                                // https://{host}{path}/
```

### `csr` (§6.2)

```rust
pub struct SpkiHash(pub [u8; 32]);       // sha256(DER SPKI); JSON base64url
impl SpkiHash { pub fn of(spki_der: &[u8]) -> Self }
pub struct CsrInfo { pub spki: SpkiHash }
pub enum CsrError { Parse, Signature, NonDnsSan, Names { found: Vec<String>, expected: [String; 2] } }
impl CsrError { pub fn code(&self) -> ErrorCode }   // Parse/Signature → invalid; names → csr_names
pub fn expected_sans(name: &str, domain: &str) -> [String; 2];       // [<name>.D, *.<name>.D]
pub fn check_csr(der: &[u8], name: &str, domain: &str) -> Result<CsrInfo, CsrError>;
```

### `proxy` (§5.3, §4.2)

```rust
pub const TLV_AUTHORITY: u8 = 0x02;   // PP2_TYPE_AUTHORITY = SNI
pub const TLV_CONN_ID: u8 = 0xE0;     // edge connection id, 8 bytes BE
pub const CONN_ID_LEN: usize = 8;
pub fn conn_id_bytes(id: u64) -> [u8; 8]; pub fn conn_id_from(value: &[u8]) -> Option<u64>;
pub const CLIENT_HELLO_MAX: usize = 16 KiB; CLIENT_HELLO_TIMEOUT_MS: u64 = 5_000;
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";
pub const VISITOR_ALPNS: &[&[u8]] = &[b"h2", b"http/1.1"];
```

### `admin` (§8.5)

```rust
pub enum BanKind { Name, Mesh, Key, Net }
pub enum ListWhat { Names, Meshes, Tunnels }
pub enum AdminCommand {   // #[serde(tag = "cmd", content = "args", rename_all = "kebab-case")]
    Status, List { what: Option<ListWhat> }, Queue, Mesh { id: PublicKey },
    Ban { kind: BanKind, value: String, reason: String }, Unban { kind: BanKind, value: String },
    Release { name: String }, EvictUnpublished, Recover { target: String, to_mesh: PublicKey },
    Admission { mode: Admission }, InviteNew { uses: Option<u32>, expires_s: Option<u64> }, CaaCheck,
    RevokeCert { name: String }, Reload, Pause, Resume, BrakeStatus }
impl AdminCommand { pub fn is_mutating(&self) -> bool }
pub struct AdminRequest { pub id: u64, #[serde(flatten)] pub command: AdminCommand,
                          #[serde(rename = "ref")] pub reference: Option<String> }
pub struct AdminError { pub code: ErrorCode, pub message: String }
pub struct AdminResponse { pub id: u64, pub ok: bool, pub result: Option<serde_json::Value>, pub error: Option<AdminError> }
impl AdminResponse { pub fn ok(id, result: Value) -> Self; pub fn err(id, code: ErrorCode, message) -> Self }
```

### `b64`

`encode(&[u8]) -> String`, `decode(&str) -> Result<Vec<u8>, DecodeError>` (base64url, no padding);
`#[serde(with = "b64::bytes")]` for `Vec<u8>`, `#[serde(with = "b64::array")]` for `[u8; N]`.

## Deviations from the design, and why

- **Error codes added** (not in §5.3's tables): `hello_required` (403, a request before a valid `hello`; the design
  says such requests are refused but names no code), `not_found` (404 of `GET …/cert`; also the admin example's
  code), `rate_limited` (429, the `/v1/info` and name-check rate limits), `too_large` (413, the 64 KiB cap),
  `internal` (500), and `Unknown` as the `#[serde(other)]` catch-all the additive rule requires.
- **`ErrorBody.reclaim_after_ms`** optional on `cooldown`: additive; the design names the field only on `DELETE`.
- **`welcome.limits` fields**: the design names only `interstitial` and "the expiry windows". Chosen:
  `random_idle_ms`, `reserved_idle_ms`, `expiry_warn_ms`, `reclaim_cooldown_ms`, `reserved_per_mesh`,
  `engines_per_mesh`, `routes_per_tunnel`, `conns_per_tunnel`, `delegation_max_ttl_ms`; the whole struct is
  `#[serde(default)]` with the design's defaults.
- **`welcome.retire`** is an optional `Signed` of a new `Retire` record with tag `theuth/share/retire/1\0`: §5.5
  says "`retire {key}` in `welcome`, signed by the standby key" without a field or tag.
- **Time units**: `not_after` (welcome, cert answers) is Unix ms, matching `not_after_ms` in the edge DDL.
- **`SpkiHash`** and **`Invite`** travel as base64url in JSON (the design fixes no encoding; base64url matches
  `body`/`sig`).
- **Labels** also refuse a leading/trailing `-` and `xn--` (DNS LDH; the design's sub-label regex is only
  `[a-z0-9-]{1,63}`). `check_random` also refuses built-in reserved words (`status`, `secure`, `theuth` are
  drawable from `[a-z2-7]{6}`).
- **`random_name` takes 6 caller-supplied bytes** so `share-proto` needs no RNG dependency.
- **`share` plugin version**: the brief says "workspace version like other plugins"; every plugin crate carries a
  frozen literal (`version = "2.22.2"  # frozen: …`), so `share` and `share-proto` follow that convention at the
  current workspace value `2.25.5`.
- **Extra deps**: `serde_json` (SSE data, admin `result`), `sha2` (SPKI and invite hashes), `thiserror` (errors).
  All already workspace dependencies.
- **Root `Cargo.toml`** also carries `instant-acme`, `zone-update`, `ppp`, `governor` entries for later phases,
  since this phase owns that file; unused entries do not enter the lock.

## Not built

- Golden JSON fixtures of every request/answer (§5.3 "tested in both `share-edge` and `share`"; M4). The unit
  tests here round-trip each shape but there is no fixtures directory yet.
- `Handover` (P2) and its tag.
- `share.delegate` / `share.name_key.ensure` RPC types: those are theuth RPC inside `crates/share`, not the edge wire.
- The default-edge two-pinned-keys type: the design places it in `crates/share`'s config (D15, appendix m2).
- Edge config struct, share-edge and share functionality (later phases).

## Open doubts

- `ErrorCode::status()` gives one status per code; the design uses `quota`, `banned`, `terms_required` always with
  403 and `invalid` with 400, so one-to-one holds today. If a later code needs two statuses, callers must override.
- `parse_host` lowercases; the edge should route on the returned lowercase parts, not the raw SNI.
- `Delegation::verify` does not check `mesh != engine` or the edge's ban/rekey/revoke/engine-count state (stateful,
  the edge's job, as the doc comment says).
- `Rekey.issued_ms` has no freshness rule in the design; `verify` does not check it.
- `AdminRequest` uses `#[serde(flatten)]` over an adjacently tagged enum: works for every command (tested for
  `ban`, `status`, `evict-unpublished`, `pause`), but flatten makes unknown-field errors impossible to enable later.
