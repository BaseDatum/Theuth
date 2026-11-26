# theuth public services on one DigitalOcean droplet

Moves `theuth-relay` (theuth.io site + device link relay), `iroh-relay` (engine mesh) and `theuth-share-edge`
(`*.tun.zip`) off the home cluster onto one droplet with a reserved IP. Nothing here is applied automatically.

| Path | What |
|---|---|
| `terraform/` | droplet (Ubuntu 24.04, monitoring), reserved IP, 10 GiB volume at `/srv/theuth`, firewall, tags, cloud-init (no secrets) |
| `dns/` | separate root module: the static A records at Porkbun, **imported** and edited in place |
| `host/` | compose project shipped to `/srv/theuth/app`: HAProxy SNI router, Caddy (DNS-01 via Porkbun), relay, iroh-relay, share edge |
| `scripts/deploy.sh` | rsync `host/`, write `.env` + DNS credential from the caller's env, `docker compose up -d` |
| `scripts/migrate-edge-data.sh` | cluster edge → 0, copy its PVC read-only to `/srv/theuth/edge`, start the droplet edge |
| `scripts/porkbun-import-ids.sh` | read-only Porkbun lookup of the record ids for `dns/` imports |
| `scripts/smoke.sh` | probe every name against one IP with `curl --resolve` / `openssl -servername` |

## What is served

| Name | Port | Path on the droplet |
|---|---|---|
| `theuth.io` | 443/tcp | HAProxy → Caddy (TLS) → relay `:8788` (site) |
| `www.theuth.io` | 443/tcp | HAProxy → Caddy → 301 `https://theuth.io` (new; the cluster never served it) |
| `relay.theuth.io`, `app.theuth.io` | 443/tcp | HAProxy → Caddy (TLS) → relay `:8787` (UI + WebSocket link) |
| `iroh.relay.theuth.io` | 443/tcp | HAProxy → iroh-relay `:8443` (TLS in-process, cert from Caddy's store) |
| `iroh.relay.theuth.io` | 7842/udp | iroh-relay QUIC address discovery (iroh-relay 1.x has no STUN) |
| `*.tun.zip` (any depth, incl. `edge.`/`iroh.`/`relay.tun.zip`) | 443/tcp | HAProxy → share edge `:8443`, PROXY v2, TLS passed through untouched |
| edge iroh endpoint | 7850/udp | share edge; `iroh.public_addrs: ["<reserved-ip>:7850"]` (the cluster had none) |
| all of the above | 80/tcp | HAProxy → Caddy → 301 to https |
| anything else on 443 | | closed (no certificate) |

Inventory source: the live Ingresses `theuth/theuth` (theuth.io, relay, app; cert SANs + iroh.relay),
`theuth/theuth-iroh-relay` (iroh.relay.theuth.io), `theuth-share/share-edge-static` (`*.tun.zip`) and the
edge-created `share-edge-name-*` (`*.<name>.tun.zip`). `iroh.theuth.io` and `www.theuth.io` resolve (via `*.theuth.io`)
but the cluster serves neither (HAProxy's default `argocd.rwx.dev` certificate); the droplet redirects `www` and
closes `iroh.theuth.io`, as today (engines use `iroh.relay.theuth.io`).

Internal: metrics (iroh-relay `:9090`, edge `:9090`) stay on the compose network. Compose IPs are fixed in
`172.30.0.0/24`; HAProxy is `.10`, the only source Caddy and the edge accept PROXY headers from.

Certificates, all DNS-01, so everything is issued before DNS points here:
- theuth.io names and `iroh.relay.theuth.io`: Caddy, Let's Encrypt production, `caddy-dns/porkbun`. iroh-relay reads
  `/srv/theuth/caddy/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/iroh.relay.theuth.io/*` read-only
  and reloads every 24 h (Caddy renews ~30 days early). Until Caddy has issued it, iroh-relay restart-loops: expected.
- `*.tun.zip`: the edge's own ACME (migrated production account `acct/4035598822`, which the tun.zip CAA records pin).
  The edge never serves a certificate on 443; engines hold theirs.

## Images

| Service | Image (`host/.env.example`) |
|---|---|
| relay | `ghcr.io/basedatum/theuth/relay:e35c6cc9e@sha256:7e01cee2…` (live) |
| iroh-relay | `ghcr.io/skymoore/theuth/iroh-relay:latest@sha256:a1e1c6e8…` (live) |
| share-edge | `ghcr.io/skymoore/theuth/share-edge:share-edge-5a4c67d52@sha256:a31601d0…` (live, `FEATURES=k8s`) |
| haproxy | `haproxy:3.2.27-alpine` |
| caddy | built on the host from `host/caddy/Dockerfile`: `caddy:2.11.9` + `caddy-dns/porkbun v0.3.1` |

No workflow change: the `FEATURES=k8s` edge binary only builds a Kubernetes client when `k8s.enabled: true`
(`crates/share-edge/src/lib.rs`), and `host/share-edge/config.yaml` leaves it off. All three GHCR packages are private
(anonymous token requests are denied), so the droplet logs in with `GHCR_PULL_USER`/`GHCR_PULL_TOKEN`.

## Secrets (names only; all from the operator's environment)

| Variable | Used by | Source |
|---|---|---|
| `DIGITALOCEAN_TOKEN` | `terraform/` | DO API token |
| `PORKBUN_API_KEY`, `PORKBUN_SECRET_KEY` | `dns/` (provider `jianyuan/porkbun`) | same key as below: `export PORKBUN_API_KEY=$PORKBUN_TUNZIP_APIKEY PORKBUN_SECRET_KEY=$PORKBUN_TUNZIP_SECRET` |
| `PORKBUN_TUNZIP_APIKEY`, `PORKBUN_TUNZIP_SECRET` | Caddy DNS-01 (theuth.io), edge DNS (tun.zip), `porkbun-import-ids.sh` | vault `porkbun.tunzip.*`; the key must be allowed on **both** zones |
| `THEUTH_RELAY_COOKIE_SECRET` | relay | cluster Secret `theuth/theuth-relay` key `cookie-secret` (same value, or every `__Host-link` cookie is dropped) |
| `GHCR_PULL_USER`, `GHCR_PULL_TOKEN` | `docker login ghcr.io` on the droplet | PAT with `read:packages` on both `basedatum` and `skymoore` packages |

On the droplet they end up in `/srv/theuth/app/.env` (0600), `/srv/theuth/secrets/tunzip-dns.json` (0400, uid 65532)
and `/root/.docker/config.json`. Nothing secret is in cloud-init, Terraform state, or git.

## Cutover runbook

One switch, no dual A records: the droplet is fully prepared and tested against its IP first, then every static A
record flips at once. The edge's own move is the only real outage and is kept to a few minutes.

```sh
cd deploy/do
export DIGITALOCEAN_TOKEN=…  PORKBUN_TUNZIP_APIKEY=…  PORKBUN_TUNZIP_SECRET=…  GHCR_PULL_USER=…  GHCR_PULL_TOKEN=…
export PORKBUN_API_KEY=$PORKBUN_TUNZIP_APIKEY PORKBUN_SECRET_KEY=$PORKBUN_TUNZIP_SECRET
export THEUTH_RELAY_COOKIE_SECRET="$(kubectl -n theuth get secret theuth-relay -o jsonpath='{.data.cookie-secret}' | base64 -d)"
```

### 0. Day before: import DNS, lower TTL

```sh
scripts/porkbun-import-ids.sh            # lists every A record, fails on duplicates, writes dns/import-ids.auto.tfvars.json
terraform -chdir=dns init
terraform -chdir=dns plan                # MUST read: N to import, 0 to add, 0 to destroy; changes only ttl (or none)
terraform -chdir=dns apply               # target stays 149.90.131.89; only imports (and sets dns_ttl)
```

Records not in the list (the edge's `<n>.tun.zip` CNAMEs, `_acme-challenge` TXTs, CAA, MX, …) are never touched.
A static name without its own A record (covered by `*.theuth.io`) is reported and left unmanaged. Any warning about
an unmanaged A record on the old IP: point it by hand at step 6.

TTL: Porkbun's minimum is normally **600 s**; the API refuses or clamps lower values. Leave `dns_ttl = 600` (the
default) unless the account allows less (`-var dns_ttl=60`). Apply it at least one old TTL before step 6.

### 1. Droplet

```sh
cp terraform/terraform.tfvars.example terraform/terraform.tfvars   # set admin_cidrs
terraform -chdir=terraform init
terraform -chdir=terraform apply
RIP=$(terraform -chdir=terraform output -raw reserved_ip)
ssh root@$RIP 'cloud-init status --wait; findmnt /srv/theuth; curl -4s https://icanhazip.com'   # must print $RIP
```

The last line proves egress leaves from the reserved IP (cloud-init's `theuth-reserved-ip-egress` routes the
default via the anchor gateway once DO reports the reserved IP active). If it prints the droplet IP, run
`systemctl restart theuth-reserved-ip-egress` and check `journalctl -u theuth-reserved-ip-egress`.

### 2. Services except the edge

```sh
scripts/deploy.sh $RIP
ssh root@$RIP 'cd /srv/theuth/app && docker compose logs -f caddy'   # wait for 5 "certificate obtained successfully"
```

iroh-relay restarts until the `iroh.relay.theuth.io` certificate exists, then stays up.

### 3. Smoke test against the new IP (DNS still points home)

```sh
scripts/smoke.sh $RIP
```

Every theuth.io and iroh-relay line must be `ok` (the `*.tun.zip` line is expected to be weak/failing until step 4).
Mesh relay end to end: on a test machine add `$RIP iroh.relay.theuth.io` to `/etc/hosts`, restart its engine, and
check `t mesh status` shows the relay connected (`https://iroh.relay.theuth.io./`); remove the hosts line after.
Device link: add `$RIP relay.theuth.io app.theuth.io` likewise and open `https://app.theuth.io`.

### 4. Edge move (minutes; the only planned outage)

```sh
scripts/migrate-edge-data.sh $RIP
scripts/smoke.sh $RIP sky.tun.zip     # a name with a live tunnel: real end-to-end check through the droplet edge
```

The script scales the cluster edge to 0 and waits for the pod to exit **before** copying (the edge's iroh secret key
must never be live in two places, and SQLite must be copied closed), copies the PVC through a read-only busybox pod
(the edge image is `FROM scratch`), checks every file's sha256 on both sides, then starts the droplet edge. It
refuses if `/srv/theuth/edge` is not empty (`FORCE=1` moves it aside). `deploy.sh --with-edge` likewise refuses
without `iroh.key` and the ACME account, so no deploy can mint a new edge key or re-register ACME.

Check `docker compose --profile edge exec -T share-edge /theuth-share-edge --config /etc/theuth-share-edge/config.yaml admin status`:
same endpoint id as before (`kubectl -n theuth-share logs` history, or the shipped ticket), CAA check passed (it
refuses to start otherwise), tunnels re-attaching.

Rollback before step 6: `docker compose --profile edge stop share-edge` on the droplet, then
`kubectl -n theuth-share scale deploy/theuth-share-edge --replicas=1`. The cluster PVC was mounted read-only and is
unchanged.

The relay and iroh-relay carry no state to migrate (in-memory uplink table; certificate only). Caddy starts empty.

### 5. Last check

`scripts/smoke.sh $RIP sky.tun.zip` all `ok`.

### 6. Switch DNS (immediately after step 4)

```sh
terraform -chdir=dns apply -var dns_target_ip=$RIP     # plan: N to change (content), 0 add, 0 destroy
for h in theuth.io relay.theuth.io iroh.relay.theuth.io edge.tun.zip x.tun.zip; do dig +short $h @curitiba.ns.porkbun.com; done
```

Persist the switch: set `dns_target_ip = "<RIP>"` in `dns/terraform.tfvars` (gitignored) or commit it as a default,
so a later plain `apply` does not flip back.

Once the old TTL has passed: `dig +short theuth.io edge.tun.zip` from a normal resolver returns `$RIP`, and
`scripts/smoke.sh $RIP sky.tun.zip` plus plain `curl -sI https://theuth.io` pass. Watch `docker compose logs -f`
on the droplet and the cluster pods' logs fading out.

### 7. Retire the cluster side (~1 hour later)

```sh
kubectl -n theuth scale deploy/theuth-relay deploy/iroh-relay --replicas=0
# theuth-share edge is already 0. Keep the PVC and Secrets until the droplet has run a week.
```

See the next section for why the old relays should not outlive the TTL by much. Rollback after this point is the
reverse: scale the cluster up, `terraform -chdir=dns apply -var dns_target_ip=149.90.131.89`, and (for the edge)
copy `/srv/theuth/edge` back into the PVC with the cluster edge at 0.

## Risk: two relays or two edges behind one name

- **iroh-relay.** Each engine homes on one relay server and is reachable *through that server only*. Two live
  iroh-relays under `iroh.relay.theuth.io` are two disjoint meshes: an engine homed on the old one cannot reach a
  peer homed on the new one by relay (direct UDP still works when hole punching succeeds). A relay connection is
  long-lived and is not re-resolved while it stays up, so a client that resolved the old IP during the TTL **stays**
  on the old relay for as long as it runs, not just for the TTL.
- **theuth-relay.** Same shape: the uplink table is in memory, so a device and its engine must be on the same relay
  process (`deploy/relay/k8s/base/deployment.yaml` runs one replica for exactly this reason).
- **share edge.** A share's tunnel lives on exactly one edge (the engine's iroh connection), and the edge's iroh key
  is one identity. Two edges would split tunnels, and two processes with one key fight over the relay home. The
  runbook therefore never runs two edges: the cluster edge is at 0 before the droplet edge starts.

Recommendation: round-robin DNS across both hosts is never safe for these services; keep the single flip. Retire
the old theuth-relay and iroh-relay as soon as the TTL after step 6 has passed (≈10–15 min with TTL 600) rather
than an hour later: until they stop, clients that resolved the old IP stay split from everyone on the droplet.
Stopping them makes those clients reconnect and land on the droplet. Even better, right after step 6 point the home
HAProxy VM's TCP 443 passthrough for all these SNI names at `$RIP:443` (TCP mode, no PROXY header) and scale the
old relays to 0 at once: stale resolvers then also reach the droplet, and the split window is zero. UDP 7842/7850
are not forwarded that way; that only costs those clients QUIC address discovery/direct paths for one TTL.

## Operating

```sh
ssh root@$RIP
cd /srv/theuth/app
docker compose --profile edge ps
docker compose logs -f haproxy caddy relay iroh-relay
docker compose --profile edge logs -f share-edge
docker compose --profile edge exec -T share-edge /theuth-share-edge --config /etc/theuth-share-edge/config.yaml admin status
```

- Upgrades: change the pin in `host/.env.example` (or export `RELAY_IMAGE=…` etc.), rerun
  `scripts/deploy.sh $RIP --with-edge`. Without `--with-edge` the running edge is left as is.
- Backups: `/srv/theuth/edge` is the one thing that cannot be recreated (edge key, ACME account, names). Take DO
  volume snapshots (`doctl compute volume snapshot <volume_id>`) on a schedule; the volume has `prevent_destroy`.
- Reboots: unattended-upgrades installs security updates but never reboots; reboot by hand when
  `/var/run/reboot-required` exists. Compose restarts everything (`restart: unless-stopped`).
- SSH: after the egress switch use the reserved IP; the droplet's own IP may stop answering
  ([INFERENCE] replies leave via the anchor gateway).
- The edge reaches this host's iroh-relay through `extra_hosts: iroh.relay.theuth.io:host-gateway` (published 443 and
  7842/udp via Docker's userland proxy), so it homes here even while public DNS still says home.
  [INFERENCE: not exercised on a droplet yet; if the edge logs relay errors, drop the `extra_hosts` line after step 6.]
