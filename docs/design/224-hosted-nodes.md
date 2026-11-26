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
| Engine image | `deploy/computer/Dockerfile`, `entrypoint.sh` | the published `ghcr.io/basedatum/theuth/engine`; on every start installs the newest signed `t` into `/data/bin` and runs that one (§5) |
| Remote pairing | `t mesh pair --remote`, `scripts/cloud-node-ticket.sh` | a one-time ticket without the vault key (§4) |
| Kubernetes | `deploy/computer/k8s/`, `crates/sandbox/src/k8s.rs`, `crates/theuth/src/k8s_cli.rs` | `t mesh add --k8s …`, `t mesh k8s list\|status\|destroy\|storage-classes` (§7) |
| Plugin | `plugins/cloud-nodes/` | tools `providers`, `create`, `status`, `list`, `destroy` over DigitalOcean, Fly.io, Kubernetes (via `t`), AWS (dry run) |
| Test script | `scripts/cloud-node-do-test.sh` | DigitalOcean end to end without the plugin (§9) |

Every resource carries `theuth-managed` plus `theuth-node:<name>` (DO tags), the metadata
`theuth-managed`/`theuth-node` (Fly), or the labels `theuth.io/managed=true`/`theuth.io/node=<name>`
(Kubernetes). `list` finds nodes by them and nothing else.

## 2. Providers, and why these

- **DigitalOcean: a Droplet running the container.** `s-1vcpu-1gb` Ubuntu 24.04, cloud-init
  installs Docker and runs the image with `--restart always --init`; `/data` and `/workspace` on an
  attached Block Storage Volume; a Cloud Firewall with no inbound rule (optional tcp/22 from one
  CIDR). **Not App Platform**: it has no persistent volumes (an App's filesystem is ephemeral and
  lost on every deploy or restart), so the vault, sessions and `/data/bin/t` would be lost, and it
  expects an HTTP service with a health check, which an outbound-only engine is not.
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
  result). Lightsail is flat-priced, one API, and takes the DigitalOcean cloud-init unchanged.

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

**Delivery** (the image reads, in order: `/data/pair-ticket`, `$THEUTH_PAIR_TICKET_FILE` (default
`/run/theuth/pair/ticket`), `$THEUTH_PAIR_TICKET`):

| Provider | How | Visible to |
|---|---|---|
| DigitalOcean | cloud-init writes `/mnt/theuth/data/pair-ticket` (0600, uid 1000) | the Droplet's metadata service (user-data stays there), the DO account |
| Fly.io | Machine `files:` entry at `/run/theuth/pair/ticket` | the Machine config, the Fly org |
| Kubernetes | Secret `theuth-<name>-pair` mounted at `/run/theuth/pair` (0440, fsGroup 1000, `optional`); `t mesh add --k8s` deletes it once the node joined or the wait ended | the namespace's Secret readers |

**Joining.** `entrypoint.sh` starts a helper (double-forked, so PID 1 reaps it) that, until
`/data/.theuth-paired` exists, runs `t --data-dir /data mesh add <ticket>` every 10 s. Success:
the marker is written and `/data/pair-ticket` deleted. "invalid or expired" / "refused": that
ticket is not retried, a log line says to write a fresh one to `/data/pair-ticket`, and the loop
keeps watching the file — so a node that missed its 5 minutes is re-paired by dropping a new
ticket in (`cloud-node-do-test.sh ticket` over SSH; `kubectl cp`/`exec` on Kubernetes). A node
re-created on a kept volume has the marker and does not try again.

**The 5-minute window** is the main operational risk: a Droplet that has to install Docker and
build the image takes ~3–4 min from mint to join [INFERENCE: not measured on DO]. Mint in the same
command line as the create, so the clock starts at the create; the published image (pull instead of
build) and a Docker 1-click Droplet image shorten it. A longer TTL needs a parameter on
`mesh_pairing_string` (a wire change); not done here because the live test runs against the
installed `t`, which would not have it.

## 5. The image and updates

Problem: the release binary was baked into root-owned `/usr/local/bin`, so the engine's own
updater (crates/update) found its directory unwritable (`Outcome::Available`, "`sudo t
--update` installs it"), and a recreated container reverted to the baked version.

`deploy/computer/Dockerfile` now: verifies the release tarball with minisign as before and unpacks
it into `/usr/local/lib/theuth` (+ `/usr/local/bin/t` symlink); keeps the public key at
`/usr/local/share/theuth/pubkey.txt`; adds `tini`, `util-linux` (`setpriv`) and the shared
libraries the Linux `t` links (`libgbm1 libwayland-client0 libxcb1 libasound2t64
libpipewire-0.3-0t64` — without them 2.31.2 exits 127 before `main`, which the old Dockerfile
did not install either); `PATH=/data/bin:…`; `ENTRYPOINT ["theuth-entrypoint"]`, extra arguments
passed on to `t --headless --data-dir /data`. `t mesh add --docker` builds the same file and now
sends `entrypoint.sh` in its build context (`crates/sandbox/src/docker.rs`).

`entrypoint.sh`, every start:

1. **root** (a Fly volume is root-owned): chown `/data` and `/workspace` to 1000 if needed, then
   `setpriv` to uid/gid 1000 with no capabilities. Docker and Kubernetes start it as 1000 already.
2. **PID 1** (Kubernetes, `docker run` without `--init`): re-exec under `tini`, so `t`'s parent is
   PID 1 (step 6 depends on it).
3. **The binary.** Baked version newer than `/data/bin/t` (or none there) → copy it in (helpers
   first, `t` last, renames on one filesystem — the order crates/update uses). Then, unless
   `THEUTH_BOOT_UPDATE=0`, read `$THEUTH_MANIFEST` (default `latest.json`; release.yml passes
   `latest-prerelease.json` for prerelease images): if it names a newer version of the same major
   for `cli-linux-<arch>`, download the tarball, verify the manifest's signature with minisign
   against the baked key, require the trusted comment to name that file (`file:theuth_<v>_linux-<arch>.tar.gz`)
   and the unpacked `t --version` to equal `<v>`, then install it. Any failure leaves `/data/bin`
   as it was and the engine starts on what is there; an unreachable manifest is a log line.
   Never a downgrade: only strictly newer versions are installed.
4. First start with `THEUTH_NODE_NAME`: `config.yaml` gets `mesh: {name, class: container,
   nearby: false}` (the engine's first run appends its own defaults below).
5. The join helper (§4).
6. `exec /data/bin/t --headless --data-dir /data`.

**The updater needs no change.** Checked in `crates/update/src/lib.rs`: the target is
`std::env::current_exe()` canonicalized → `/data/bin/t`; step 4 tests that its directory is
writable (it is, uid 1000 owns `/data`) and installs in place with the same signature check; the
tick loop runs in release builds that are not an app sidecar (first check 60 s after start, then
every 6 h ± 5 min). It exits for a restart (`EXIT_RESTART = 75`) only when all of:
`platform::service::managed()` (parent PID is 1, or systemd's `INVOCATION_ID`), no external wire
client attached, and 10 min without a turn. In the image the parent is PID 1 everywhere:
`docker run --init` (docker-init), Kubernetes (tini via step 2), Fly (its init). The restart policy
(`always`; Kubernetes `restartPolicy: Always`; Fly `restart.policy: always`) starts the container
again, step 3 finds `/data/bin/t` already newest, and the new version runs. A recreated container
on the same volume keeps the updated binary.

Known noise (pre-existing, not changed): the Linux `t` logs a PipeWire `client.conf` warning every
5 s in a container without PipeWire; the DO run caps Docker logs at 3 × 10 MB.

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
  `THEUTH_PLUGIN_DATA`) and creates. DigitalOcean also takes `image: build` (builds
  `deploy/computer` on the Droplet; needs the repo or `CLOUD_NODES_ENGINE_CONTEXT`). Kubernetes runs
  `t mesh add --k8s`. AWS answers with the §10 plan and a cost, and refuses `dry_run: false`.
- `status(name, provider)`, `list()` (every configured provider; one failing provider is reported
  in `errors`, not fatal), `destroy(name, provider, keep_volume = true)`.
- `create` and `destroy` carry `destructiveHint` (they spend or delete), so `tools.policy` asks.
- Checks: `plugins/cloud-nodes/check.sh` (py_compile, ruff, 15 unit tests with a fake HTTP transport
  and a fake `t`: request order and bodies, firewall before Droplet, no inbound rule, the vault key
  never sent, the ticket only in user-data, user-data valid bash in both image modes, destroy
  keeping/deleting the volume, Fly config, the `t` argv for Kubernetes, the SDK `tools/call` path
  and `--theuth-manifest` against `plugin.yaml`).

The DigitalOcean cloud-init exists twice — `cloud_nodes.user_data` and
`scripts/cloud-node-do-test.sh` — because the script must run without the plugin; the plugin's is
canonical and the step 13 move deletes both for the Rust one.

## 9. DigitalOcean in detail, and the live test

cloud-init (bash, no `set -x`): `PasswordAuthentication no`; 1 GiB swap (a 1 GiB Droplet and a
1.2 GB binary); wait for `/dev/disk/by-id/scsi-0DO_Volume_theuth-<name>-data`, `mkfs.ext4` only
when `blkid` finds no filesystem, mount at `/mnt/theuth` (fstab `nofail`), `data/` and
`workspace/` owned 1000; the ticket file; `docker.io` unless Docker is there; build or pull; `docker
run -d --name theuth-engine --restart always --init --hostname <name> -e THEUTH_NODE_NAME=<name>
-v /mnt/theuth/data:/data -v /mnt/theuth/workspace:/workspace --log-opt max-size=10m
--log-opt max-file=3`. Its log: `/var/log/theuth-node.log`.

`ENGINE_IMAGE=build` is the script's default: `ghcr.io/basedatum/theuth/engine` is not
anonymously pullable today (`docker manifest inspect` → `manifest unknown`, token endpoint 403),
and no published tag has this entrypoint yet. Build bakes `latest.json`'s version.

**Live test (orchestrator, on the Mac whose engine the node joins):**

```sh
cd ../theuth-wt/cloud-node
export DIGITALOCEAN_TOKEN=…                           # never echoed by the script
t mesh status >/dev/null                              # the engine is up
# optional, recommended for the first run: SSH for logs and ticket re-delivery
export SSH_KEY=<DO key id or fingerprint> SSH_CIDR="$(curl -fsS https://api.ipify.org)/32"
THEUTH_PAIR_TICKET="$(scripts/cloud-node-ticket.sh)" scripts/cloud-node-do-test.sh create
#   → plan + ≈$7/month, firewall, volume, droplet active at <ip>, then waits ≤10 min for the
#     peer 'theuth-test' in `t mesh status`
t mesh status                                         # peer theuth-test, reachable: true
scripts/cloud-node-do-test.sh status                  # Droplet/volume/firewall (+ node log with SSH)
# missed the 5 minutes (SSH mode):
THEUTH_PAIR_TICKET="$(scripts/cloud-node-ticket.sh)" scripts/cloud-node-do-test.sh ticket
scripts/cloud-node-do-test.sh destroy                 # everything; --keep-volume keeps the $1/mo disk
t mesh status   # then: t mesh remove <id of theuth-test>
```

Persistence check (optional, before destroy): `destroy --keep-volume`, `create` again (no ticket
needed — the marker is on the volume; `THEUTH_PAIR_TICKET` can be any placeholder only if
`SSH_CIDR` is set, otherwise mint one), and the same peer id reappears in `t mesh status`.

## 10. AWS plan (not implemented)

Lightsail, region default `eu-west-2`, bundle `micro_3_0` (2 vCPU burst, 1 GiB, $7/mo):
`CreateDisk theuth-<name>-data` (tags `theuth-managed`, `theuth-node:<name>`) →
`CreateInstances` (blueprint `ubuntu_24_04`, user-data = the DigitalOcean cloud-init with the
device `/dev/nvme1n1` instead of the DO by-id path, same tags) → `AttachDisk` →
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
  `/data/pair-ticket` gone, marker present, the helper reaped.
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
