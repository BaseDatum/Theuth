# 224 — Hosted nodes: a theuth engine on the user's own cloud account

Owner request 2026-11-26: "one-click hosted theuth node". A user with an existing cloud account (or
cluster) supplies credentials — or theuth finds the provider CLI's — and theuth starts a remote
container running a headless engine that joins the owner's mesh, keeps its state on a persistent
volume, updates itself on new releases and stays up. Built first as the plugin `cloud-nodes`
(design 216) so it can be tried before it moves into the app (§13). Kubernetes added the same day
at the same priority as DigitalOcean.

Branch `feat/cloud-node`. Nothing here has created a cloud resource yet; §14 lists what was run.

## 1. Shape

| Piece | Where | What |
|---|---|---|
| Install script | `deploy/computer/theuth-install.sh` | one script for both kinds of node: `install` (newest signed `t` into `/data/bin`), `config`, `join` (the ticket loop) (§5) |
| VM install | `deploy/computer/vm-cloud-init.sh` | native node on a VM: user `engine`, `/data` volume, runtime libs, systemd units (§9) |
| Engine image | `deploy/computer/Dockerfile`, `entrypoint.sh` | the published `ghcr.io/basedatum/theuth/engine` for container providers; its entrypoint runs `theuth-install.sh` (§5) |
| Remote pairing | `t mesh pair --remote`, `scripts/cloud-node-ticket.sh` | a one-time ticket without the vault key (§4) |
| Kubernetes | `deploy/computer/k8s/`, `crates/sandbox/src/k8s.rs`, `crates/theuth/src/k8s_cli.rs` | `t mesh add --k8s …`, `t mesh k8s list\|status\|destroy\|storage-classes` (§7) |
| Plugin | `plugins/cloud-nodes/` | tools `providers`, `create`, `status`, `list`, `destroy` over DigitalOcean, Fly.io, Kubernetes (via `t`), AWS (dry run) |
| Test script | `scripts/cloud-node-do-test.sh` | DigitalOcean end to end without the plugin (§9) |

Every resource carries `theuth-managed` plus `theuth-node:<name>` (DO tags), the metadata
`theuth-managed`/`theuth-node` (Fly), or the labels `theuth.io/managed=true`/`theuth.io/node=<name>`
(Kubernetes). `list` finds nodes by them and nothing else.

## 2. Providers, and why these

**Rule (owner, 2026-11-26): VM providers install natively; container providers use the image.**
DigitalOcean, Hetzner, EC2/Lightsail instances get `t` under systemd with no Docker (one less
daemon, no image registry in the path, the updater's file is just a file); Fly.io, Kubernetes and
RunPod run `ghcr.io/basedatum/theuth/engine`. Both paths share `theuth-install.sh`.

- **DigitalOcean: a Droplet with a native install.** `s-1vcpu-1gb` Ubuntu 24.04; cloud-init
  (`vm-cloud-init.sh`) creates user `engine` (uid 1000), mounts the Block Storage Volume at
  `/data`, installs the libraries `t` links, writes the release key and `theuth-install.sh`, and
  starts `theuth-engine.service` (`User=engine`, `Restart=always`, `RestartSec=5`, `ExecStartPre=
  theuth-install.sh install`) and `theuth-join.service`; a Cloud Firewall with no inbound rule
  (optional tcp/22 from one CIDR). **Not App Platform**: it has no persistent volumes (an App's
  filesystem is ephemeral and lost on every deploy or restart), so the vault, sessions and
  `/data/bin/t` would be lost, and it expects an HTTP service with a health check, which an
  outbound-only engine is not.
- **Fly.io: one App + one Volume + one Machine** (Machines API). A real container with a
  volume at `/data`, restart policy `always`. Fly allows one volume per Machine, so `/workspace` is
  the image's directory (scratch, lost when the Machine is replaced); repos meant to last go under
  `/data`.
- **Kubernetes: a one-replica Deployment** (Recreate) with two PVCs, in any context and namespace
  the user's kubeconfig reaches (§7).
- **AWS: Lightsail instance + Lightsail disk** (planned, §10). Rejected: Lightsail *container
  services* (no persistent storage), ECS/Fargate + EFS (task definition, service, cluster, VPC
  subnets, security group, IAM execution role, EFS filesystem + mount targets + access point: nine
  kinds of resource for one container, and SQLite on NFS is a known way to corrupt a database),
  plain EC2 + EBS (VPC/subnet/security group/AMI lookups; workable but more surface for the same
  result). Lightsail is flat-priced, one API, and takes `vm-cloud-init.sh` unchanged but for the
  disk's device path.

## 3. Credentials: discovery and storage

First match wins, per provider; values are never returned, logged, or put on a command line
(curl gets a 0600 header file, Python a request header).

| Provider | 1. theuth vault (via `plugin.yaml` `env:`) | 2. the provider CLI's file |
|---|---|---|
| DigitalOcean | `DIGITALOCEAN_TOKEN: "{{secret:digitalocean.token}}"` | `~/.config/doctl/config.yaml`, `~/Library/Application Support/doctl/config.yaml` (`access-token`) |
| Fly.io | `FLY_API_TOKEN: "{{secret:fly.token}}"` | `~/.fly/config.yml` (`access_token`); org from `FLY_ORG` (default `personal`) |
| Kubernetes | — | kubectl and its kubeconfig (every auth plugin works because kubectl runs it) |
| AWS | `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` | `~/.aws/credentials`, `~/.aws/config` profiles (SSO included) |

The shipped `plugin.yaml` declares no secret: a `{{secret:X}}` whose row is missing stops the
plugin (216 §4.3), and nobody has all four. The comment at its top gives the line to add per
provider; adding it widens the grant and the install card asks again (216 §10.2), and from then on
the plugin holds a code hash, so an edit to its code needs a fresh press. `providers()` says where
each credential came from and the `t --secret-set` line that moves it into the vault.

## 4. Pairing a remote engine

**Today** (`t mesh add --docker|--ssh|--vm`, 64 §4.4, `crates/sandbox/src/bringup.rs`) the owner's
engine execs `t mesh pair` *inside* the new engine, reads its pairing string, and calls
`mesh::AddPeer` itself. A cloud node has no exec channel (no inbound at all), so the direction
flips: the owner mints the string and the new engine dials (`t mesh add <string>`), exactly as a
laptop joins a mesh by pasting one (67 §4.1).

**The ticket** is `t mesh pair`'s string (`1.` + base32 of `{id, name, relay, addrs, token,
vault_key?, vault_key_id?}`, `crates/mesh/src/pairing.rs`) **without `vault_key` and
`vault_key_id`**:

- `token`: 16 random bytes, live 5 minutes (`pairing::TTL_MS`), consumed by the first `PairHello`
  that proves it (`Net::on_stranger`: `*pending = None`). One pending token per engine; a second
  `t mesh pair` inside the window returns the same one.
- The vault key is not needed: a 183+ acceptor sends its whole keyring in `PairAck` over the
  token-checked QUIC connection, and the fresh side adopts it (`keys::adopt_pairing` uses the
  string's key only when the keyring is empty, i.e. a pre-183 owner). Verified locally (§14):
  the joiner logged `mesh: adopted the vault keyring from …` from a ticket with no key.
- Nothing long-lived of the owner's is in it: the owner's endpoint id and addresses are public
  routing data, and the iroh secret key and the vault key never leave the owner.

What the ticket still is: **an admission credential for 5 minutes**. Whoever presents it first
becomes a mesh peer and receives the vault keyring. So it travels only through channels the owner
already trusts with the node itself (the provider's metadata service, a Kubernetes Secret in the
user's namespace), and it is dead after the first use or 5 minutes, whichever comes first.

**Minting it.** `t mesh pair --remote` (new, CLI-only: it calls the existing `mesh_pairing_string`
RPC and strips the key with `mesh::pairing::without_vault_key`; no wire change). For a `t` without
the flag (≤ 2.31), `scripts/cloud-node-ticket.sh` does the same decode/re-encode in Python:

```sh
t mesh pair --remote                        # a t from this branch
scripts/cloud-node-ticket.sh                # any t: uses --remote when present, else strips itself
THEUTH_T="docker exec engine t" scripts/cloud-node-ticket.sh   # an engine in a container
```

**Delivery** (`theuth-install.sh join` reads, in order: `/data/pair-ticket`,
`$THEUTH_PAIR_TICKET_FILE` (default `/run/theuth/pair/ticket`), `$THEUTH_PAIR_TICKET`):

| Provider | How | Visible to |
|---|---|---|
| DigitalOcean | cloud-init writes `/data/pair-ticket` (0600, user `engine`) | the Droplet's metadata service (user-data stays there), the DO account |
| Fly.io | Machine `files:` entry at `/run/theuth/pair/ticket` | the Machine config, the Fly org |
| Kubernetes | Secret `theuth-<name>-pair` mounted at `/run/theuth/pair` (0440, fsGroup 1000, `optional`); `t mesh add --k8s` deletes it once the node joined or the wait ended | the namespace's Secret readers |

**Joining.** `theuth-install.sh join` — `theuth-join.service` on a VM (`ConditionPathExists=
!/data/.theuth-paired`), a double-forked helper of the image's entrypoint (so PID 1 reaps it) —
runs `t --data-dir /data mesh add <ticket>` every 10 s until `/data/.theuth-paired` exists.
Success: the marker is written and `/data/pair-ticket` deleted. "invalid or expired" / "refused":
that ticket is not retried, a log line says to write a fresh one to `/data/pair-ticket`, and the
loop keeps watching the file — so a node that missed its 5 minutes is re-paired by dropping a new
ticket in (`cloud-node-do-test.sh ticket` over SSH; `kubectl cp`/`exec` on Kubernetes). A node
re-created on a kept volume has the marker and does not try again.

**What runs where (release compatibility).** Reading `/data/pair-ticket` is the shell script's job,
not `t`'s; `t` only runs `mesh add <string>`, which every 67+ release has. So the **released
2.31.2** both joins (node) and accepts (owner) a key-less ticket: the owner's `PairAck` carries the
keyring since 183 (2026-11-09), and 2.31.2 is from 2026-11-26. Verified with release binaries on
both sides (§14). Only `t mesh pair --remote` and `t mesh add --k8s` need a `t` from this branch;
`scripts/cloud-node-ticket.sh` stands in for the former with any `t`. An owner older than 183
still pairs, but sends no keyring, so the node's replicated secrets would not open.

**The 5-minute window** is the main operational risk: a VM node has to boot, install packages
and download a ~450 MB tarball before `t` dials [INFERENCE: ~2–3 min on a DO Droplet, not
measured]. Mint in the same command line as the create, so the clock starts at the create; a late
node is re-paired with a fresh ticket (above). A longer TTL needs a parameter on
`mesh_pairing_string` (a wire change); not done here because the live test runs against the
installed release, which would not have it.

## 5. Install, image and updates

Problem: the release binary was baked into root-owned `/usr/local/bin`, so the engine's own
updater (crates/update) found its directory unwritable (`Outcome::Available`, "`sudo t
--update` installs it"), and a recreated container reverted to the baked version.

**`deploy/computer/theuth-install.sh`** (POSIX sh; the one place that downloads and verifies):

- `install`: `$THEUTH_BAKED_DIR` (the image's build-time-verified release; unset on a VM) when it
  is newer than `/data/bin/t` (or nothing is there) → copied in (helpers first, `t` last, renames
  on one filesystem — the order crates/update uses). Then, unless `THEUTH_BOOT_UPDATE=0`, read
  `$THEUTH_MANIFEST` (default `latest.json`; release.yml passes `latest-prerelease.json` for
  prerelease images): if it names a newer version (of the same major, when one is installed) for
  `cli-linux-<arch>`, download the tarball, verify the manifest's signature with minisign against
  `$THEUTH_PUBKEY_FILE` (the release key, `/usr/local/share/theuth/pubkey.txt`), require the
  trusted comment to name that file (`file:theuth_<v>_linux-<arch>.tar.gz`) and the unpacked
  `t --version` to equal `<v>`, then install it. Any failure leaves `/data/bin` as it was; exit 1
  only when no `t` is installed afterwards. Never a downgrade.
- `config NAME KIND`: first start only — `config.yaml` gets `mesh: {name, class: KIND, nearby:
  false}` (`container` in the image, `server` on a VM); the engine's first run appends its own
  defaults below.
- `join`: §4.

**The image** (`deploy/computer/Dockerfile`): verifies the release tarball with minisign as before
and unpacks it into `/usr/local/lib/theuth` (+ `/usr/local/bin/t` symlink); keeps the public key
at `/usr/local/share/theuth/pubkey.txt`; adds `tini`, `util-linux` (`setpriv`) and the shared
libraries the Linux `t` links (`libgbm1 libwayland-client0 libxcb1 libasound2t64
libpipewire-0.3-0t64` — without them 2.31.2 exits 127 before `main`, which the old Dockerfile
did not install either); `theuth-install.sh` at `/usr/local/lib/`; `PATH=/data/bin:…`;
`ENTRYPOINT ["theuth-entrypoint"]`, extra arguments passed on to `t --headless --data-dir /data`.
`t mesh add --docker` builds the same file and now sends `entrypoint.sh` and `theuth-install.sh`
in its build context (`crates/sandbox/src/docker.rs`). `entrypoint.sh`, every start:

1. **root** (a Fly volume is root-owned): chown `/data` and `/workspace` to 1000 if needed, then
   `setpriv` to uid/gid 1000 with no capabilities. Docker and Kubernetes start it as 1000 already.
2. **PID 1** (Kubernetes, `docker run` without `--init`): re-exec under `tini`, so `t`'s parent is
   PID 1 (the updater's restart depends on it).
3. `theuth-install.sh install` with `THEUTH_BAKED_DIR=/usr/local/lib/theuth`.
4. `theuth-install.sh config $THEUTH_NODE_NAME container`.
5. `theuth-install.sh join`, double-forked, until paired.
6. `exec /data/bin/t --headless --data-dir /data`.

**A VM** (§9) runs the same `install` as `ExecStartPre` of `theuth-engine.service` (`User=engine`),
`config … server` once from cloud-init, and `join` as `theuth-join.service`.
**The updater needs no change.** Checked in `crates/update/src/lib.rs`: the target is
`std::env::current_exe()` canonicalized → `/data/bin/t`; step 4 tests that its directory is
writable (it is, uid 1000 owns `/data`) and installs in place with the same signature check; the
tick loop runs in release builds that are not an app sidecar (first check 60 s after start, then
every 6 h ± 5 min). It exits for a restart (`EXIT_RESTART = 75`) only when all of:
`platform::service::managed()` (parent PID is 1, or systemd's `INVOCATION_ID`), no external wire
client attached, and 10 min without a turn. On a VM systemd sets `INVOCATION_ID`; in the image the
parent is PID 1 everywhere: `docker run --init` (docker-init), Kubernetes (tini via step 2), Fly
(its init). The restart (`Restart=always`; Kubernetes `restartPolicy: Always`; Fly
`restart.policy: always`) runs `install` again, which finds `/data/bin/t` already newest, and the
new version runs. A recreated VM or container on the same volume keeps the updated binary.

Known noise (pre-existing, not changed): the Linux `t` logs a PipeWire `client.conf` warning every
5 s on a host without PipeWire (journald rate-limits it on a VM).

## 6. Lifecycle

| | create | status | list | destroy (default keeps the volume) |
|---|---|---|---|---|
| DigitalOcean | tags → firewall on the node tag (before the Droplet exists, so it is covered from its first packet) → volume (reused if `theuth-<name>-data` exists) → Droplet | Droplet, volume, firewall; `mesh_peer` from `t mesh status` | Droplets tagged `theuth-managed` + volumes kept | Droplet, firewall, node tag; volume after it detaches unless kept |
| Fly.io | app `theuth-<name>-<6 hex of org>` → volume `theuth_data` → Machine | Machines and volumes of the app | apps with the prefix and the org suffix | Machines (`force`); with `keep_volume` the app and volume stay |
| Kubernetes | `t mesh add --k8s` | `t mesh k8s status` | `t mesh k8s list` | `t mesh k8s destroy [--delete-volume]` |

Destroying never unpairs: the node stays in the peer set until `t mesh remove <id>`; `destroy`
reports whether it is still a peer and the command. (A node destroyed with its volume kept and
re-created comes back as the same engine — same identity on `/data` — and needs no ticket.)

## 7. Kubernetes

`t mesh add --k8s [--name N] [--context C] [--namespace NS [--create-namespace]]
[--storage-class S] [--volume-size 10Gi] [--image REF] [--deny-cluster-network [--deny-cidr
CIDR]…] [--dry-run]`.

- **Base** (`deploy/computer/k8s/base`, embedded with `include_str!`): ServiceAccount with
  `automountServiceAccountToken: false` and no Role/RoleBinding; PVCs `data` and `workspace`
  (RWO, 10Gi, no class → the cluster default); Deployment `engine`: 1 replica, `Recreate`,
  `restartPolicy: Always`, `runAsUser/runAsGroup/fsGroup 1000`, `runAsNonRoot`,
  `seccompProfile: RuntimeDefault`, `allowPrivilegeEscalation: false`, all capabilities dropped,
  `enableServiceLinks: false`, requests 100m/512Mi, limit 2Gi; the ticket Secret mounted
  `optional`. No Service: the engine only dials out.
- **Overlay** (written by `sandbox::k8s::kustomization`, rendered with `kubectl kustomize`, applied
  with `kubectl apply -f -` from stdin; the ticket is a 0600 file in a 0700 temp dir that is
  removed on drop, never an argument): `namespace`, `namePrefix: theuth-<name>-`, labels
  `theuth.io/node` (also in selectors) and `theuth.io/managed`, `images:` (default
  `ghcr.io/basedatum/theuth/engine:<this t's version>`), the PVC size/class patch, `THEUTH_NODE_NAME`,
  `secretGenerator` `pair` (no hash suffix, so it can be deleted by name).
- **kubectl, not the `kube` crate**: kube-rs is in the workspace only behind share-edge's `k8s`
  feature; linking it into `t` adds a client stack for one subcommand, and kubeconfig `exec`/OIDC
  plugins work through kubectl exactly as for the user.
- **Flow**: `mesh_status` before → `mesh_pairing_string` → strip → apply → poll `mesh_status`
  every 5 s for a *new* peer id with the node's name, printing the pod phase (`ContainerCreating`,
  `ImagePullBackOff`, …) as it changes, for the ticket's TTL plus a minute → delete the Secret
  either way. No `kubectl exec`: the user needs no `pods/exec` right.
- Without `--storage-class` in a cluster that has classes but no default one, it refuses and lists
  them (a PVC there would stay Pending). `t mesh k8s storage-classes` lists them.
- **`--deny-cluster-network`** (off by default) adds `components/deny-cluster-network`: a
  NetworkPolicy on the node's pod with no ingress rule and egress to (a) port 53 TCP/UDP anywhere
  (kube-dns or node-local-dns, wherever it is) and (b) `0.0.0.0/0` except `169.254.169.254/32`
  (cloud metadata), the pod CIDRs from `node.spec.podCIDRs` and the service CIDRs from
  `ServiceCIDR` objects (Kubernetes 1.33+), plus `--deny-cidr`. If the user cannot read nodes and
  gives no `--deny-cidr`, it refuses rather than apply an open policy. IPv4 only (the policy has no
  `::/0` rule, so IPv6 egress is denied). It only works where the CNI enforces NetworkPolicy.
- **What the agent on that engine may do inside the cluster is the user's decision.** It has no
  Kubernetes API credentials (no token mounted, no RBAC); without the policy it can open
  connections to any pod or Service the namespace's network allows, like any workload there; with
  it, nothing in the cluster but DNS. Giving it more (a kubeconfig in the vault, an RBAC binding)
  is the user's explicit act.

## 8. The plugin

`plugins/cloud-nodes/`, scaffolded as `t plugin new cloud-nodes --lang py` lays a plugin out
(`plugin.yaml`, `main.py`, the vendored `theuth_plugin.py`); the logic is `cloud_nodes.py`
(stdlib only: urllib, subprocess). Install and approve as any plugin: `t plugin install
plugins/cloud-nodes`, then the owner presses the card (`t plugin approve cloud-nodes`).

- `providers()` — §3.
- `create(provider, name, region?, size?, volume_gb?, image?, ssh_cidr?, ssh_key?, context?,
  namespace?, storage_class?, deny_cluster_network?, dry_run = true)`: the dry run returns the plan
  and `estimated_monthly_usd` and touches nothing; `dry_run: false` mints the ticket (`t mesh pair
  --remote`, or strips it itself for an older `t`; `--data-dir` is derived from
  `THEUTH_PLUGIN_DATA`) and creates. DigitalOcean installs natively and refuses `image`.
  Kubernetes runs `t mesh add --k8s`. AWS answers with the §10 plan and a cost, and refuses
  `dry_run: false`.
- `status(name, provider)`, `list()` (every configured provider; one failing provider is reported
  in `errors`, not fatal), `destroy(name, provider, keep_volume = true)`.
- `create` and `destroy` carry `destructiveHint` (they spend or delete), so `tools.policy` asks.
- `vm/` holds copies of `deploy/computer/vm-cloud-init.sh`, `deploy/computer/theuth-install.sh` and
  `crates/update/pubkey.txt` (an installed plugin has no repo beside it); `watch:` lists them so the
  code hash covers them, and a test fails when a copy drifts from the repo.
- Checks: `plugins/cloud-nodes/check.sh` (py_compile, ruff, 16 unit tests with a fake HTTP transport
  and a fake `t`: request order and bodies, firewall before Droplet, no inbound rule, the vault key
  never sent, the ticket only in user-data, user-data valid bash with no Docker and the install
  script embedded byte for byte, injection refused, the vendored copies, destroy keeping/deleting
  the volume, Fly config, the `t` argv for Kubernetes, the SDK `tools/call` path and
  `--theuth-manifest` against `plugin.yaml`).

The VM template is one file (`deploy/computer/vm-cloud-init.sh`); the script and the plugin only
fill its placeholders.

## 9. DigitalOcean in detail, and the live test

`vm-cloud-init.sh` (bash, no `set -x`, log `/var/log/theuth-node.log`): `PasswordAuthentication
no`; 1 GiB swap when the host allows it (a 1 GiB Droplet and a 1.2 GB binary); user `engine`
(uid 1000); wait for `/dev/disk/by-id/scsi-0DO_Volume_theuth-<name>-data`, `mkfs.ext4` only when
`blkid` finds no filesystem, mount at `/data` (fstab `nofail`), owned by `engine`; `apt-get install`
`ca-certificates curl git python3 ripgrep tar zstd minisign` and the five libraries `t` links;
`/usr/local/share/theuth/pubkey.txt` and `/usr/local/lib/theuth-install.sh` from the template;
`/data/bin` on login `PATH`; the ticket at `/data/pair-ticket` (0600); `config.yaml` (`class:
server`); then two units, enabled and started without blocking cloud-init:

- `theuth-engine.service`: `User=engine`, `Environment=THEUTH_DATA_DIR=/data THEUTH_MANIFEST=…`,
  `ExecStartPre=theuth-install.sh install` (first start: download + minisign; later starts: a
  manifest check), `ExecStart=/data/bin/t --headless --data-dir /data`, `Restart=always`,
  `RestartSec=5`, `TimeoutStartSec=1200`, `StartLimitIntervalSec=0`, `RequiresMountsFor=/data`.
- `theuth-join.service`: `User=engine`, `ExecStart=theuth-install.sh join`,
  `ConditionPathExists=!/data/.theuth-paired`, `Restart=on-failure`.

The node downloads whatever `latest.json` names; `MANIFEST=` points it elsewhere. No image, no
registry: the private ghcr package does not matter here.

**Which binary runs where for the live test: release binaries only.** The Droplet installs the
released `t` 2.31.2 from `latest.json`; this Mac's engine is the app's 2.31.2 sidecar
(`~/.theuth/engine.json`: `version 2.31.2`); the ticket is minted by `scripts/cloud-node-ticket.sh`
through that engine's `mesh_pairing_string` RPC. No branch build is involved (§4).

**Live test (orchestrator, on this Mac):**

```sh
cd /Users/sky/Develop/skymoore/theuth-wt/cloud-node
export DIGITALOCEAN_TOKEN=…                                    # never echoed by the script
export THEUTH_T=/Applications/theuth.app/Contents/MacOS/t      # the 2.31.2 that matches the engine
"$THEUTH_T" mesh status >/dev/null                             # the engine is up
# recommended for the first run: SSH for logs and ticket re-delivery
export SSH_KEY=<id or fingerprint from `doctl compute ssh-key list`> SSH_CIDR="$(curl -fsS https://api.ipify.org)/32"
THEUTH_PAIR_TICKET="$(scripts/cloud-node-ticket.sh)" scripts/cloud-node-do-test.sh create
#   → plan + ≈$7/month, firewall, volume, Droplet active at <ip>, then waits ≤10 min for the
#     peer 'theuth-test' in `t mesh status`
"$THEUTH_T" mesh status                                        # peer theuth-test, reachable: true
scripts/cloud-node-do-test.sh status     # Droplet/volume/firewall; with SSH: units, t --version, journal
# missed the 5 minutes (SSH mode):
THEUTH_PAIR_TICKET="$(scripts/cloud-node-ticket.sh)" scripts/cloud-node-do-test.sh ticket
scripts/cloud-node-do-test.sh destroy    # everything; --keep-volume keeps the $1/mo disk
"$THEUTH_T" mesh status                  # then: "$THEUTH_T" mesh remove <id of theuth-test>
```

Persistence check (optional, before destroy): `destroy --keep-volume`, then `create` again with a
fresh ticket (the script requires one without `SSH_CIDR`; the node ignores it: the marker on the
volume says it is paired), and the same peer id reappears in `t mesh status`.

## 10. AWS plan (not implemented)

Lightsail, region default `eu-west-2`, bundle `micro_3_0` (2 vCPU burst, 1 GiB, $7/mo):
`CreateDisk theuth-<name>-data` (tags `theuth-managed`, `theuth-node:<name>`) →
`CreateInstances` (blueprint `ubuntu_24_04`, user-data = `vm-cloud-init.sh` with `/dev/nvme1n1`
as the data device, same tags) → `AttachDisk` →
`PutInstancePublicPorts []` (no inbound; tcp/22 from one CIDR if asked). Credentials from the
discovered profile (SigV4 via botocore if present, else the `aws` CLI). `destroy`:
`DeleteInstance`, `DetachDisk`/`DeleteDisk` unless kept. `providers()` and the dry run (with cost)
exist; `create(dry_run: false)` refuses.

## 11. Costs (list prices, USD/month, 2026; billed hourly)

| Provider | Default | Instance | 10 GiB volume | Total |
|---|---|---|---|---|
| DigitalOcean | `s-1vcpu-1gb` lon1 | $6.00 | $1.00 | **$7.00** |
| Fly.io | `shared-cpu-1x` 1 GB lhr | $1.94 + 0.75 GB × $5 = $5.69 | $1.50 | **$7.19** (no dedicated IPv4) |
| AWS Lightsail | `micro_3_0` eu-west-2 | $7.00 | $1.00 | **$8.00** |
| Kubernetes | — | the cluster's | the class's | — |

Egress is within the providers' included transfer for a mesh node. `create`'s dry run prints the
figure; DigitalOcean's 2 GiB size (`s-1vcpu-2gb`, $12) is the step up if the engine's memory
needs it (the 1 GiB default has 1 GiB swap).

## 12. Fly.io notes

Fields used: `POST /v1/apps {app_name, org_slug}`, `POST /v1/apps/{app}/volumes {name, region,
size_gb, encrypted}`, `POST /v1/apps/{app}/machines {name, region, config: {image, guest, env,
mounts, restart, processes: [{name, user: root}], files: [{guest_path, raw_value, mode}],
metadata}}`. `processes[].user = root` is what lets step 1 own the root-formatted volume; it and
`files[].mode` are from the Machines API reference and are not exercised against Fly here. Fly
needs a pullable image (no `build`).

## 13. From plugin into the app

1. **Records and RPC.** A `sandbox::Target::Cloud { provider, region, size, volume_gb }` beside
   Docker/SSH/VM, with a `Computer` record per node (the record store 79 already has), so
   `sandboxes_list` and Settings › Network › Machines show hosted nodes with the others. RPC
   `node_create(spec, dry_run) -> Plan | Created`, `node_destroy(name, keep_volume)`,
   `node_status(name)`; progress as the existing bring-up `Progress` lines.
2. **Providers in Rust** (`crates/sandbox/src/cloud/{digitalocean,fly}.rs`, reqwest, the same calls
   as `cloud_nodes.py`); Kubernetes is already Rust. The cloud-init becomes one `include_str!`
   template; the plugin and the script are deleted.
3. **Credentials** become vault rows (`cloud/digitalocean`, `cloud/fly`; 221 for external
   stores), set from the UI; discovery of CLI files stays as the "import" button.
4. **The ticket is minted in-process** (no CLI round-trip) and `mesh_pairing_string` gains an
   optional TTL so a slow provider is not racing 5 minutes (one wire-schema accept).
5. **UI**: Machines › Add › "Hosted node": provider, region, size with the monthly cost, Create;
   the row turns green when the peer appears.
6. **Teardown** unpairs as part of destroy (`mesh_remove`) once the record knows the peer id.

## 14. Verification (this branch)

- `docker build deploy/computer` (THEUTH_VERSION 2.31.2, arm64): builds; `ldd` clean; `t 2.31.2`.
- Boot install from the manifest: the image's `/usr/local/lib/theuth` replaced by a fake `t`
  reporting 2.31.1 → the entrypoint installed it, then downloaded 2.31.2 from `latest.json`,
  verified signature + trusted comment + version, installed into `/data/bin`; `t` ran from there
  with parent PID 1 (tini), `config.yaml` carried the mesh name.
- Pairing: engine A in one container minted a pairing string; `scripts/cloud-node-ticket.sh`
  (`THEUTH_T="docker exec cn1 t"`) stripped it (payload keys `addrs, id, name, relay, token`);
  container B started with it as `THEUTH_PAIR_TICKET` and logged `joined the mesh` within ~15 s;
  A's `t mesh status` listed `cn-two`, reachable; B logged `adopted the vault keyring from cn-test`;
  `/data/pair-ticket` gone, marker present, the helper reaped. (These two ran the entrypoint
  before `theuth-install.sh` was split out; after the split the image was rebuilt and an engine
  started from it: baked `t` installed into `/data/bin`, `config.yaml` written, `t` under tini,
  the join helper reparented to PID 1.)
- Native VM path, emulated: the rendered `vm-cloud-init.sh` (from `cloud-node-do-test.sh
  user-data`, the device swapped for a 4 GiB file) ran in a privileged `ubuntu:24.04` container
  with a stub `systemctl`: exit 0; user `engine` uid 1000; the file formatted and mounted at
  `/data` owned by `engine`; packages installed; `config.yaml` with `class: server`;
  `/data/pair-ticket` 0600; both unit files written; swap refused by the container and skipped.
  Then the units' commands by hand as `engine`: `theuth-install.sh install` downloaded 2.31.2 from
  `latest.json`, verified it and installed `/data/bin/t`; `t --headless` (with `INVOCATION_ID`)
  and `theuth-install.sh join` → `joined the mesh`, `adopted the vault keyring from owner`, the
  owner engine (the rebuilt image, release 2.31.2) listed `vm-node` reachable. Release binaries
  on both sides. Not covered: real systemd, a DO volume device, cloud-init itself.
- Kubernetes: the overlay rendered with `kubectl kustomize` and `kubectl apply --dry-run=client`
  against the local cluster (context `k8s.rwx.dev`): ServiceAccount, Secret, 2 PVCs, Deployment,
  NetworkPolicy created (dry run). Pod CIDRs (`10.244.x.0/24` per node) and the ServiceCIDR
  (`10.96.0.0/12`) detected; that cluster has no default StorageClass (`nfs-csi` only), which is
  why the refusal in §7 exists.
- `cargo check -p mesh -p sandbox --tests`; `cargo clippy -p mesh -p sandbox --lib --tests` and
  `cargo clippy -p theuth --bin t` clean (the latter with `-A clippy::manual_contains` for a
  pre-existing finding in `crates/tasks/src/ledger.rs:1344` that is not this branch's); unit tests
  `sandbox::k8s`, `sandbox::docker`, `mesh::pairing`.
- This branch's debug `t`: `t mesh pair --help` shows `--remote`; `t mesh k8s storage-classes`
  and `list` against the local cluster; `t mesh add --k8s --dry-run` without `--storage-class`
  refuses (no default class); with `--namespace theuth-test --storage-class nfs-csi
  --deny-cluster-network` it renders, the `except` list holds the metadata address, every node's
  pod CIDR and the ServiceCIDR, and `kubectl apply --dry-run=client` accepts it; `--k8s --docker`
  and `--deny-cidr` without `--k8s` are refused by clap.
- `bash -n` + shellcheck (koalaman/shellcheck:stable) on both scripts, the entrypoint and the
  rendered user-data: clean. `plugins/cloud-nodes/check.sh`: ruff clean, 15/15 tests.
  `plugin.yaml` validates against `sdk/manifest.schema.json`.
- Not run: any cloud API call, Fly, AWS, a real `kubectl apply`, `t mesh pair --remote` against a
  running engine (its stripping is `without_vault_key`, unit-tested; the Python twin was run
  end to end above), `t plugin install`/`approve` (owner only).

## 15. Open points

- The 5-minute ticket vs. a Droplet that builds its image (§4); the TTL parameter (§13.4).
- The engine image is private on ghcr; publishing it (package visibility) makes `image:` refs
  work for Fly and Kubernetes without registry credentials.
- A 1.2 GB `t` (2.31.2): every boot-time update downloads ~450 MB; `/data` holds two copies
  during an install (10 GiB default volume is ample).
