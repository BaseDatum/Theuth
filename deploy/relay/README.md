# Relay deployment

Two processes on `*.theuth.io`, one certificate:

| Process | Host | Serves | Ports (container) |
|---|---|---|---|
| `theuth-relay` (`crates/relay`, design doc 31) | `relay.theuth.io`, `app.theuth.io`, `theuth.io` | device ↔ engine link over WebSocket, browser UI, site | 8787/tcp, 8788/tcp |
| `iroh-relay` (upstream, design doc 67 §4.2) | `iroh.relay.theuth.io` | engine ↔ engine mesh fallback and QUIC hole-punch probe | 8443/tcp, 7842/udp, 8080/tcp, 9090/tcp |

- `compose.yaml` — single host: `theuth-relay` behind Caddy (automatic TLS), `iroh-relay` beside it.
- `k8s/base` + `k8s/rwx-dev` — kustomize; HAProxy ingress, cert-manager certificate `theuth-io-cert-tls`.
- `Dockerfile` builds `theuth-relay`; `Dockerfile.iroh-relay` builds the upstream `iroh-relay` binary.

## theuth relay

See `crates/relay` and design doc 31 §5. Stateless per connection except the in-memory uplink table, so the ingress hashes on URI (`/h/<id>`).

## iroh relay

Engines default to `mesh.relay: https://iroh.relay.theuth.io` (`crates/mesh/src/config.rs`); `mesh.relay: public` uses n0's relays instead, `mesh.relay: ""` disables relaying (LAN only).

**What it is.** The stock `iroh-relay` server from the iroh project, version-pinned to the `iroh` line in the workspace `Cargo.toml` (1.2.x). No theuth code. Locally: `cargo install iroh-relay --version 1.2.0 --features server`, then `iroh-relay --config-path deploy/relay/iroh-relay.toml`.

**Config** is `iroh-relay.toml`: relay + QUIC address discovery on, metrics on, `access = "everyone"` (the mesh is pairwise authenticated by key, 67 §15; switch to `access.allowlist = [...]` with engine ids if abused).

**Ports.** Three, and why each exists:

| Port | Why |
|---|---|
| 8443/tcp (public 443) | the relay itself: HTTPS + WebSocket. TLS is terminated **in the process**, not at the proxy — see below |
| 7842/udp | QUIC address discovery. Engines send a probe and learn their public address for hole punching. UDP cannot ride an HTTP ingress, which is why the mesh relay is a *subdomain* and not a path under `relay.theuth.io` |
| 8080/tcp | plain-HTTP captive portal / probe target; not exposed publicly |
| 9090/tcp | Prometheus metrics; cluster-internal |

**Certificate.** The QUIC listener needs the private key in-process, so `iroh-relay` serves TLS itself with the same certificate the ingress uses for `relay.theuth.io`: the cert-manager secret `theuth-io-cert-tls` is mounted at `/certs` (`cert_mode = "Reloading"` picks up renewals). `k8s/rwx-dev/ingress.yaml` lists `iroh.relay.theuth.io` under `tls.hosts` so cert-manager adds the SAN (the deployed certificate is a SAN list, not a wildcard — the design doc's "same wildcard certificate" is that list). `k8s/rwx-dev/ingress-iroh.yaml` passes 443 through by SNI (`haproxy.org/ssl-passthrough`) to the pod's 8443; `Service iroh-relay-quic` (type LoadBalancer) exposes 7842/udp. DNS: `iroh.relay.theuth.io` → the ingress address (443/tcp) and the UDP LoadBalancer address (7842/udp) — one IP if the cluster can give both to one Service, else two A records are fine (iroh dials the relay URL over HTTPS and probes UDP on the same name).

With compose, Caddy already owns 443, so `iroh-relay` publishes 8443 and 7842/udp directly and reads `./certs/tls.crt|tls.key` (copy Caddy's certificate for the host there on renewal, or give the mesh relay its own IP and publish `443:8443`). Set `mesh.relay: https://iroh.relay.example.com:8443` on engines if you keep the non-default port.

**Health.** `curl -sk https://iroh.relay.theuth.io/` returns the relay's landing page; metrics on `:9090/metrics`. An engine reports its relay status in `t mesh status` (peers' `addrs` show `https://iroh.relay.theuth.io./` once connected).
