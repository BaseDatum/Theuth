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

## Standby key (launch procedure, §5.5, §13)

A release pins two edge keys: the live `iroh.key` and a cold standby. The standby secret is generated and kept
offline; the edge only knows its public half.

```sh
# On an offline machine (no config needed): writes the secret (0600, never overwrites), prints the public key
theuth-share-edge keygen --out /media/offline/standby.key
# → line 1: hex public key (what standby_public takes); line 2: z32 form
```

Put the hex key into the config as `standby_public: <hex>`, restart, and pin both tickets in the release:

```sh
theuth-share-edge --config <path> ticket             # live
theuth-share-edge --config <path> ticket --standby   # standby_public with the live relay and addresses
```

With `standby_public` set the edge never creates `iroh.standby.key`. A leftover `iroh.standby.key` on the volume
is a start warning when it is the same key (delete it) and refuses start when it is a different one.

**Rotation (live key lost or compromised).** Stop the edge, take a copy of the data dir (or work on the volume),
then, with the offline standby secret at hand:

```sh
theuth-share-edge retire --standby-key /media/offline/standby.key --edge-key <data_dir copy>/iroh.key
# or, on the host: theuth-share-edge --config <path> retire --standby-key /media/offline/standby.key
```

This signs `Retire {edge: standby, retired: live}` with the standby secret and writes beside the edge key:
`retire.json`, the standby secret as the new `iroh.key` (0600) and the old key as `iroh.key.retired`. Copy those
back into the data dir, `keygen` a new standby, set `standby_public` to it, start the edge: every `welcome` now
carries the retire notice, and clients move their pin. Without a config file present, the `--edge-key` form does
not check `standby_public` or whether the edge is running.

Edges without `standby_public` keep the pre-launch behaviour: `iroh.standby.key` is created on the volume at
first start, and `retire` (no flags) promotes it.

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
