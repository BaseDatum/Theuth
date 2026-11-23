# 210 as built — E2E1 (first real publish against the live edge, P0 numbers)

Branch `feat/210-public-share`. Live edge: k8s ns `theuth`, Deployment `theuth-share-edge` (image
`share-edge-a0c7a86e8`, later rolled twice by the orchestrator for pod DNS, see Findings 1), endpoint
`72f4d775e18884ea9e349619f3500927641d63db67b2b8a0ee0929cb69d8e038`, home relay `https://iroh.relay.theuth.io`,
domain `tun.zip`, Let's Encrypt STAGING. Run on 2026-11-23, 16:37–17:20 (WET = UTC). Scripts and logs: `/tmp/e2e210/`.

**Result: end to end works.** Engine attached via the relay, the edge allocated a random name, wrote the CNAMEs and
DNS-01 TXTs at Porkbun, ordered a staging wildcard, the engine installed it, `https://app.<name>.tun.zip/` and
`https://<name>.tun.zip/hello.txt` served the local server through HAProxy passthrough with the visitor's real IP;
Stop all removed the routes; release removed records and the per-name Ingress. The first cert took 32 min only
because the edge pod could not resolve DNS reliably (Finding 1); with public resolvers the order took ~2.5 min.

## What changed in the repo

| path | change |
|---|---|
| `crates/share/src/config.rs` | `DEFAULT_EDGE_TICKETS[0]` = `theuth-share-edge ticket` output of the live edge (`theuthedgeabzpjv3v4…`); the standby slot stays a placeholder (`TODO(210-M4)`). The unit test now pins: one default edge parses, endpoint id `72f4d775…`. Commit 8183327d3 |
| `docs/design/210-as-built-E2E1.md` | this file |

No public API change. No Cargo.lock change.

## Procedure as run

1. Ticket: `kubectl -n theuth exec deploy/theuth-share-edge -- /theuth-share-edge ticket` (the deployed `ticket`
   has no `--standby`; one key).
2. Dev engine: `t` from this branch, `--data-dir /tmp/theuth-210-e2e --headless`, `THEUTH_RPC__LISTEN` unset,
   config `rpc.listen 127.0.0.1:17210`, `secrets.key_store: file`, `link.enabled: false`, `mesh.discovery: false`,
   default `mesh.relay` (= iroh.relay.theuth.io). No model needed. Local site: Python on `127.0.0.1:18210`.
3. Admission: the deployed edge has no invite/allowlist command (`admin` has status/list/ban/unban/admission/pause/
   resume; `control.rs:340` `TODO(210-P2)` allowlist, `admin.rs:207` `TODO(210-M4)` invite). I set
   `admin --ref "E2E1 test engine admit" admission open` at 16:40:55, attached once (the mesh row now exists, so
   later hellos skip admission, `control.rs:325`), and set it back to `closed` at 16:44:13. Nothing else attached
   in that window (`admin list tunnels` showed only my engine).
4. RPC (owner bearer token; each gated call raised a human-only row that `/tmp/e2e210/rpc.py --approve` resolved
   with `resolve_approval` `allow` — the documented RPC-token residual):
   `share_settings_set {enabled: true}` → `share_publish` (refused: terms) → `share_terms_accept {edge: <id>,
   terms_rev: "2026-11-23"}` → `share_publish {source: {port: 18210}, label: "app", expiry: "1h"}` →
   (after the cert) `share_publish {source: {dir: /tmp/e2e210/www}, name: "nnsgqr", expiry: "1h"}` →
   `share_stop {id: "all"}` → `share_names_release {name: "nnsgqr"}`.

## P0 numbers

| what | observed |
|---|---|
| Attach path | **relay**: `admin list tunnels` → `net: relay:https://iroh.relay.theuth.io/` (UDP 7850 not forwarded) |
| Attach time | ≤ 0.4 s: `share_publish` at 16:42:13.517 already had the edge's `info` (terms refusal at 13.922) |
| Name | random `nnsgqr` (`kind: random`), Ingress `share-edge-name-nnsgqr` (`*.nnsgqr.tun.zip`) created at claim, ~16:42:21 |
| `share_publish` RPC latency (cold) | 60.2 s (16:42:20.748 → 16:43:20.960), answered `state: pending`. Approval resolved at +0.17 s; where the rest goes is not traced [INFERENCE: waits on the edge/claim with a timeout] |
| Cert job | first attempt 16:43:24 (≈ 63 s after claim, the runner's 60 s tick); attempts 1–4 failed on DNS (Finding 1); attempt 5 started 17:12:29 and the share was `live` at 17:14:55.046 |
| Cert issuance with working DNS | **≈ 2 min 26 s** (17:12:29 → 17:14:55): 2 serialised authorizations, each TXT create → probe 30 s (timed out, Finding 3) → validate → delete; 61 s gap between them (the 60 s cached-value wait) |
| Publish → first successful visit (cold) | **32 min 36 s** (16:42:20.75 → 17:14:56.99, `curl -k --resolve … 149.90.131.89` probe every 3 s). Dominated by Finding 1 and backoff, not the design |
| Live → first visit | 1.9 s (probe granularity 3 s) |
| Publish → first visit (warm: name already has a valid cert) | **385 ms** (apex dir share: call 17:16:36.405, approval 36.730, `live` answer 36.734, first 200 at 36.790) |
| Cert | issuer `C=US, O=Let's Encrypt, CN=(STAGING) Artificial Amaranth YE1`, subject `CN=nnsgqr.tun.zip`, SANs `DNS:*.nnsgqr.tun.zip, DNS:nnsgqr.tun.zip`, notAfter 2027-01-26 |
| DNS records at Porkbun | `nnsgqr.tun.zip. 300 IN CNAME edge.tun.zip.` and `*.nnsgqr.tun.zip` CNAME `edge.tun.zip.` (read back from curitiba/fortaleza.ns.porkbun.com); TXT `_acme-challenge.nnsgqr.tun.zip` seen at Porkbun's NS 30 s after creation. **Porkbun accepted TTL 300** (P0-1 question). SOA `tun.zip` minimum (negative TTL) **1800 s** |
| DNS propagation | public resolvers (1.1.1.1, 8.8.8.8) answered `nnsgqr.tun.zip → edge.tun.zip → 149.90.131.89` minutes after the CNAME write; `x.nnsgqr.tun.zip` was NXDOMAIN there until the wildcard CNAME existed (Finding 5) |
| Visitor IP at the origin | `X-Forwarded-For: ::ffff:188.251.216.205` = this host's public IP (api.ipify.org) → **HAProxy PROXY v2 → edge → engine preamble works** (v4-mapped form, Finding 6). `Host` rewritten to `localhost:18210`, `X-Forwarded-Proto: https` |
| Visitor latency (warm, this host → 149.90.131.89) | `curl` total 53 ms, TCP connect 7 ms, TLS done 42 ms, for a directory listing |
| Stop all | `{stopped: 2}`; both hosts then fail with curl exit 35 (TLS reset by the edge); Ingress `share-edge-name-nnsgqr` stayed |
| Release | first call failed (Finding 4); retry 14 s later OK (`reclaim_after_ms` = +30 d). Within ~7 s: both CNAMEs deleted (read back empty on two Porkbun NS), no TXT left, Ingress gone, `admin list names` → `state: released` |

## Findings (not my code)

1. **Edge pod DNS (deploy, fixed live by the orchestrator).** `certificate job: porkbun: io: failed to lookup address
   information: Try again` (attempts 1–3) and `newOrder: client error (Connect): dns error: … Try again` (attempt 4,
   after a first roll to `ndots:1, timeout:2, attempts:3`). Static musl `getaddrinfo` in the scratch image behind
   CoreDNS (`forward . /etc/resolv.conf`); attempt 4 failed instantly, so musl got SERVFAIL rather than timing out;
   CoreDNS logged nothing. The second roll (`dnsPolicy: None`, nameservers 1.1.1.1/9.9.9.9, ndots 1) fixed it:
   attempt 5 ran clean. The deploy overlay must carry that dnsConfig, or the edge needs a resolver that does not
   depend on the node's upstream.
2. **`certs::reconcile` deletes a held name's CNAMEs** (`crates/share-edge/src/certs.rs:434-438`): `keep` requires
   `records != 0`, and `ensure_records` sets it only after both CNAMEs. After a mid-way failure (attempt 2 wrote
   `nnsgqr` only) the restart removed `nnsgqr.tun.zip` as "orphan" (16:51:27); the next attempt re-created it.
   Churn plus a window where the name does not resolve. The orchestrator reports a fix in the next image.
3. **TXT propagation probe is IPv6-only from the pod**: both authorizations log `TXT not yet on
   [2400:cb00:2049:1::adf5:3a25, …] after 30s` (Porkbun's NS AAAA addresses) and then proceed; the TXT was visible
   over IPv4 at that time. The probe is a blind 30 s wait here; it should try the NS A records too (or prefer v4
   when the pod has no v6 route).
4. **Release right after Stop all fails once**: `share_names_release` 3 s after `share_stop all` →
   `edge transport: closed by peer: stop_all (code 1)` (the edge closes the tunnel on stop_all; the client reused the
   closed connection). A retry 14 s later succeeded. The engine should redial instead of failing the call.
5. **Wildcard gap**: before a name's CNAMEs exist, `*.tun.zip` answers for `x.<name>.tun.zip`; once `<name>` has a
   CNAME and before `*.<name>` does, `x.<name>.tun.zip` is NXDOMAIN, cacheable for the SOA minimum (1800 s). A
   visitor (or the owner) hitting `app.<name>` in that gap may see NXDOMAIN for up to 30 min. Writing `*.<name>`
   first would close it.
6. **`X-Forwarded-For` carries a v4-mapped address** (`::ffff:188.251.216.205`), not `188.251.216.205`; apps that
   parse/allowlist IPv4 will not match. Normalising `to_canonical()` before writing the header fixes it.
7. **Abandoned RPC leaves its approval row**: my first `share_settings_set` client timed out at 40 s; its approval
   stayed pending, and allowing it later was accepted (`true`) but did nothing (the call was gone). The row should be
   withdrawn when the RPC future drops.
8. **Terms are checked after the approval**: the first `share_publish` raised and got the owner's approval, then
   failed with "accept the terms … first". The owner approves for nothing; check terms before asking.
9. **Cold `share_publish` blocks ~60 s** and answers `pending` (above).
10. **A `dir` share at `/` without `index.html` answers `not found` for `/`** (`/hello.txt` was 200). Probably by
    design (no listings); noting it because the brief expected a listing.

## Not built / not done

- `TODO(210-M4)`: the cold standby ticket (second slot of `DEFAULT_EDGE_TICKETS`); the deployed `ticket` command
  has no `--standby`.
- Invite/allowlist admission does not exist in the deployed edge; admission was opened for ~3 min instead.
- The edge's `conn_log` was not read directly (SQLite on the PVC, scratch image, no admin command); the visitor
  IP was proven through `X-Forwarded-For` at the origin, which the engine fills from the edge's PROXY v2 preamble.
- Revocation of the released name's staging cert: no `revocation` warning/error in the edge log within 3 min after
  release; success itself is not logged, so not confirmed.

## Doubts

- Cold publish→visit (32 min) is not a P0 number for the design; the meaningful ones are issuance ≈ 2.5 min (under
  P0-2's 5 min) and warm publish→visit 385 ms.
- The cert's `notBefore` (16:16:20) is ~1 h before issuance; LE staging backdating, not checked further.

## Verification (commands as run)

```
# ticket
kubectl -n theuth exec deploy/theuth-share-edge -- /theuth-share-edge ticket
# build + test, one lock hold each
until mkdir /tmp/theuth-build.lock …; CARGO_BUILD_JOBS=6 cargo build -p theuth --bin t        # ok
until mkdir /tmp/theuth-build.lock …; CARGO_BUILD_JOBS=6 cargo test -p share --lib config::  # 1 passed
# engine
env -u THEUTH_RPC__LISTEN target/debug/t --data-dir /tmp/theuth-210-e2e --headless
/tmp/e2e210/rpc.py share_settings_set '{"enabled":true,"edge":null}'      # with rpc.py --approve running
/tmp/e2e210/rpc.py share_terms_accept '{"edge":"72f4…e038","terms_rev":"2026-11-23"}'
/tmp/e2e210/rpc.py share_publish '{"args":{"source":{"port":18210},"label":"app","expiry":"1h"}}'
# observe
kubectl -n theuth exec deploy/theuth-share-edge -- /theuth-share-edge admin list tunnels   # net relay:…
dig +short CNAME nnsgqr.tun.zip @curitiba.ns.porkbun.com                                  # edge.tun.zip.
kubectl -n theuth get ingress share-edge-name-nnsgqr
curl -sk https://app.nnsgqr.tun.zip/                     # 200, directory listing
curl -sk https://nnsgqr.tun.zip/hello.txt                # 200 (after the apex dir share)
openssl s_client -connect 149.90.131.89:443 -servername app.nnsgqr.tun.zip | openssl x509 -noout -issuer -ext subjectAltName
/tmp/e2e210/rpc.py share_stop '{"id":"all"}'             # stopped 2; curl exit 35 on both; Ingress still there
/tmp/e2e210/rpc.py share_names_release '{"name":"nnsgqr"}'   # 1st: transport closed; 2nd: ok
kubectl -n theuth get ingress | grep share               # only share-edge-static
dig +short CNAME {,*.}nnsgqr.tun.zip @{curitiba,fortaleza}.ns.porkbun.com   # empty
```

Cleanup done: dev engine and Python server stopped, `/tmp/theuth-210-e2e` deleted (engine log kept as
`/tmp/e2e210/engine-theuth.log`), name released, records and Ingress gone, edge admission `closed`,
`admin status` tunnels 0.
