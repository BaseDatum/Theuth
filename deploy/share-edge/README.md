# Share edge deployment

`theuth-share-edge` (`crates/share-edge`, design doc 210) is the public edge of web sharing: visitors reach it on
TCP 443, it reads only the ClientHello's SNI and splices the bytes to the hosting engine over iroh
(`theuth/share/1`). It never decrypts and never presents a certificate on 443. Its own version and
`share-edge-v*` tags; image `ghcr.io/skymoore/theuth/share-edge`.

| File | What |
|---|---|
| `Dockerfile` | static musl binary, `FROM scratch`, built from the repo root; cross-compiled (cargo-zigbuild), `--build-arg FEATURES=k8s` for edge-managed Kubernetes objects |
| `compose.yaml` | one host with a public IP: `443:8443/tcp`, `7850:7850/udp`, a named data volume, the DNS key as a file secret |
| `theuth-share-edge.service` | systemd unit for a plain host |
| `config.example.yaml` | every key with its default (appendix §8.1) |
| `k8s/base` | kustomize: Deployment (1 replica, `Recreate`), 1 Gi PVC, LoadBalancer Service (TCP 443 → 8443, UDP 7850, `externalTrafficPolicy: Local`), ConfigMap; the Secret is created out of band |
| `k8s/rwx-dev` | the owner's cluster: ClusterIP 8443 behind the HAProxy ingress (SNI passthrough, PROXY v2), Ingresses created by the edge from `k8s:` templates (image with `FEATURES=k8s`), ServiceAccount + Role on `ingresses`; admission closed, Let's Encrypt staging. See `docs/design/210-as-built-DEPLOY1.md` |

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

## Certificates (ACME over DNS-01, Porkbun)

Order of first start (§8.5):

```sh
theuth-share-edge --config <path> acme register          # creates acme/letsencrypt.json (0600), prints 2 CAA lines
# put both CAA lines (issue + issuewild, accounturi, validationmethods=dns-01) into the zone at Porkbun
theuth-share-edge --config <path> run                    # refuses to start until the CAA lines resolve
```

`run` also refuses without the DNS credential or the account file. The Porkbun key may be written as
`{"key", "secret"}` (zone-update's names) or `{"apikey", "secretapikey"}` (Porkbun's); the file is re-read on
every DNS call, so a rotated key needs no restart.

Per name the edge writes `<n>.D` and `*.<n>.D` CNAME `edge.D` when the name's first order gets a slot, one
`_acme-challenge.<n>.D` TXT at a time during an order, and deletes them on release or a name ban (and revokes
the certificate). Every record it may have written is in the `dns_records` table first; at start every row no
held name needs (and every challenge TXT) is deleted from the zone. Renewals follow ARI (6 h scan), else at two
thirds of the lifetime. New-name certificates draw from one bucket (`acme.cas.letsencrypt.new_certs_per_week`,
default 45); renewals and rotations do not.

Timing keys (`acme:`): `propagation` (30s, wait after writing the TXT), `auth_gap` (60s, between the two
authorizations), `renew_check` (6h), `job_lease` (15m), `max_attempts` (12; retries back off from 1 min to
24 h).
