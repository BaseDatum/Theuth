# Share edge deployment

`theuth-share-edge` (`crates/share-edge`, design doc 210) is the public edge of web sharing: visitors reach it on
TCP 443, it reads only the ClientHello's SNI and splices the bytes to the hosting engine over iroh
(`theuth/share/1`). It never decrypts and never presents a certificate on 443. Its own version and
`share-edge-v*` tags; image `ghcr.io/skymoore/theuth/share-edge`.

| File | What |
|---|---|
| `Dockerfile` | static musl binary, `FROM scratch`, built from the repo root |
| `compose.yaml` | one host with a public IP: `443:8443/tcp`, `7850:7850/udp`, a named data volume, the DNS key as a file secret |
| `theuth-share-edge.service` | systemd unit for a plain host |
| `config.example.yaml` | every key with its default (appendix §8.1) |
| `k8s/base` | kustomize: Deployment (1 replica, `Recreate`), 1 Gi PVC, LoadBalancer Service (TCP 443 → 8443, UDP 7850, `externalTrafficPolicy: Local`), ConfigMap; the Secret is created out of band |

## Ports

| Port | Why |
|---|---|
| 8443/tcp (public 443) | visitors; TLS passes through untouched |
| 7850/udp | the edge's iroh endpoint, which engines dial |
| 9090/tcp | `/healthz`, `/metrics`; cluster-internal |

No TCP 80: DNS validation needs no inbound port.

## Data dir

`/var/lib/theuth-share-edge` holds `iroh.key` (the endpoint key clients pin; created 0600 on first start),
`share-edge.sqlite` (names, meshes, bans) and `admin.sock`. Back it up; an empty data dir is a new edge key.

## Secrets

Never in the config file. The Porkbun key `{"key": "pk1_…", "secret": "sk1_…"}` comes from exactly one of
`dns.credential_file` or the env var `THEUTH_SHARE_EDGE_DNS__CREDENTIAL`; both, or a `dns.credential` key in the
YAML, refuse to start. Kubernetes:

```sh
kubectl -n theuth create secret generic theuth-share-edge --from-file=dns.json=./dns.json
kubectl apply -k deploy/share-edge/k8s/base
```

## Operating

```sh
theuth-share-edge --config <path> ticket                 # the ticket for `share.edge`
theuth-share-edge --config <path> admin status
theuth-share-edge --config <path> admin list tunnels
theuth-share-edge --config <path> admin ban name evil --reason "phishing report 123" --ref 123
theuth-share-edge --config <path> admin admission invite # the brake (§9.5)
kubectl -n theuth exec deploy/theuth-share-edge -- /theuth-share-edge admin status
```

`acme register` (account and CAA lines, §8.5) arrives with the ACME worker.
