# 224 — Hosted nodes: a theuth engine on the user's own cloud account

Owner request 2026-11-26: "one-click hosted theuth node". A user with an existing cloud account (or
cluster) supplies credentials — or theuth finds the provider CLI's — and theuth starts a remote
container running a headless engine that joins the owner's mesh, keeps its state on a persistent
volume, updates itself on new releases and stays up. Built first as the plugin `cloud-nodes`
(design 216) so it could be tried; the app feature is the crate `cloudnodes` (§16) and the plugin
stays a prototype the app does not use. Kubernetes added the same day at the same priority as
DigitalOcean.

Branch `feat/cloud-node`. The DigitalOcean path (`scripts/cloud-node-do-test.sh`) was run live on
2026-11-26: Droplet + volume + firewall, native install, joined the mesh in 232 s. The app path
(§16) has not created a cloud resource; §14 and §16.9 list what was run.

**Owner decisions for the app (2026-11-26, final):** 2 GB RAM is the minimum and the default
everywhere (DO `s-1vcpu-2gb`, Fly `shared-cpu-1x` 2048 MB, Kubernetes request 2Gi), 4 and 8 GB
offered; the monthly cost (server + disk) is in the create button's label and that press is the
approval; **no SSH and no inbound port** on any node (no SSH key uploaded or attached; the
public IPv4 stays, so peers can connect directly instead of through the relay — iroh encrypts
end to end either way); the `engine` user on a VM node has passwordless sudo (agents on a node
are meant to have root); the escape hatch is a terminal over the mesh (§16.6).

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

- **DigitalOcean: a Droplet with a native install.** `s-1vcpu-2gb` (the 2 GB minimum) Ubuntu 24.04; cloud-init
  (`vm-cloud-init.sh`) creates user `engine` (uid 1000, passwordless sudo), mounts the Block Storage Volume at
  `/data`, installs the libraries `t` links, writes the release key and `theuth-install.sh`, and
  starts `theuth-engine.service` (`User=engine`, `Restart=always`, `RestartSec=5`, `ExecStartPre=
  theuth-install.sh install`) and `theuth-join.service`; a Cloud Firewall with no inbound rule
  and no SSH key on the Droplet (the test script keeps an opt-in tcp/22 for debugging; the app never offers it). **Not App Platform**: it has no persistent volumes (an App's
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

**The 5-minute window** was the main operational risk: a VM node has to boot, install packages
and download a ~450 MB tarball before `t` dials. Measured on DigitalOcean (live test, 2026-11-26):
**232 s** from create to joined — inside 5 minutes, but with ~70 s to spare, and a slow apt
mirror or a busy region would miss it. For the CLI paths above the ticket stays the 5-minute
one; the app's cloud nodes use a **separate 15-minute single-use ticket** (§16.3), because a node
without SSH has no way to receive a second ticket after boot.

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
no`; 1 GiB swap when the host allows it; user `engine` (uid 1000) with **passwordless sudo** —
`engine ALL=(ALL) NOPASSWD:ALL` in `/etc/sudoers.d/90-theuth-engine`, mode 0440, written to a temp
file, checked with `visudo -cf` and only then moved into place, and `visudo -c` over the whole
configuration afterwards (a bad drop-in is removed, never left to break sudo); wait for `/dev/disk/by-id/scsi-0DO_Volume_theuth-<name>-data`, `mkfs.ext4` only when
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
| DigitalOcean | `s-1vcpu-2gb` lon1 | $12.00 | $1.00 | **$13.00** |
| Fly.io | `shared-cpu-1x` 2 GB lhr | $1.94 + 1.75 GB × $5 = $10.69 | $1.50 | **$12.19** (no dedicated IPv4) |
| AWS Lightsail (coming soon) | `small_3_0` eu-west-2 | $12.00 | $1.00 | **$13.00** |
| Kubernetes | 2Gi request | the cluster's | the class's | — |

2 GB is the owner's minimum (§ top); 4 GB (DO `s-2vcpu-4gb` $24, Fly `shared-cpu-2x` 4096 MB
$21.38) and 8 GB (DO `s-4vcpu-8gb` $48, Fly `shared-cpu-4x` 8192 MB $42.76) are the steps up.
Egress is within the providers' included transfer for a mesh node. The numbers live in one place,
`crates/cloudnodes/src/providers/catalog.rs`; the UI shows `cloud_quote`'s total in the create button.

## 12. Fly.io notes

Fields used: `POST /v1/apps {app_name, org_slug}`, `POST /v1/apps/{app}/volumes {name, region,
size_gb, encrypted}`, `POST /v1/apps/{app}/machines {name, region, config: {image, guest, env,
mounts, restart, processes: [{name, user: root}], files: [{guest_path, raw_value, mode}],
metadata}}`. `processes[].user = root` is what lets step 1 own the root-formatted volume; it and
`files[].mode` are from the Machines API reference and are not exercised against Fly here. Fly
needs a pullable image (no `build`).

## 13. From plugin into the app

Done on this branch as the crate `cloudnodes` (§16) rather than a `sandbox::Target::Cloud`: a
cloud node's lifecycle (provider API, OAuth, quotes, a join measured in minutes) shares nothing
with the sandbox actor's exec/put bring-up, and the sandbox crate links Docker, SSH and VM stacks
an iOS engine cannot carry. `cloudnodes` depends on `sandbox` only behind its `kubernetes`
feature. The plugin and `scripts/cloud-node-do-test.sh` stay as the prototype and the live-test
tool; nothing in the app calls them.

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

- A cloud node whose 15-minute ticket expired before it dialled (§16.3) is marked failed;
  recovery is destroy (keep the disk) + create, which mints a fresh ticket. No in-place re-pair
  without SSH.
- The engine image is private on ghcr; publishing it (package visibility) makes `image:` refs
  work for Fly and Kubernetes without registry credentials.
- A 1.2 GB `t` (2.31.2): every boot-time update downloads ~450 MB; `/data` holds two copies
  during an install (10 GiB default volume is ample).

## 16. Cloud nodes in the app (crate `cloudnodes`)

### 16.1 Shape

| Piece | Where |
|---|---|
| Wire types, topic `cloudnodes.events` | `crates/cloudnodes/src/types.rs` |
| Provider clients behind `providers::Provider` (DO, Fly, Kubernetes), catalog (sizes, regions, prices), discovery, VM user-data | `crates/cloudnodes/src/providers/` |
| Actor, node records (`<data_dir>/cloudnodes/<name>.json`, no secrets), the create job, credentials, DO OAuth client, RPC | `crates/cloudnodes/src/` |
| Single-use remote ticket with its own TTL | `crates/mesh` (`RemoteTicket`) |
| Terminal and engine log on another engine | `crates/agent` (`engine_terminal`, `engine_logs`, PTY relay) |
| DigitalOcean code exchange | `crates/relay` (`/oauth/digitalocean/*`) |
| UI | `crates/desktop/ui/src/components/cloud/`, fixture `src/dev/cloud-fixture.html`, screenshots `docs/design/224-assets/` |

Providers in this build: **DigitalOcean** (full), **Kubernetes** (desktop engines only: it needs
kubectl and a kubeconfig; `cloudnodes` feature `kubernetes`), **Fly.io** (every call implemented and
unit-tested against recorded shapes, never run against Fly: the UI marks it *beta*), **AWS
Lightsail** (listed with its price, *coming soon*, not creatable).

### 16.2 RPC (owner `cloudnodes`, owner audience)

`cloud_providers`, `cloud_import_credential(provider, id)`, `cloud_set_token(provider, token)`,
`cloud_forget_credential(provider)`, `cloud_oauth_start(provider)`, `cloud_oauth_cancel(provider)`,
`cloud_quote(spec)`, `cloud_k8s_info(context?)`, `cloud_create(spec)` (returns at once, state
`creating`), `cloud_list` (`{nodes, errors}`), `cloud_status(name)`, `cloud_restart(name)`,
`cloud_resize(name, memory_gb)`, `cloud_update(name)`, `cloud_destroy(name, keep_volume?)`,
`cloud_terminal(name)`, `cloud_logs(name, tail?)`. Events: `CloudNodeChanged{node}`,
`CloudNodeProgress{name, stage, line, pct, eta_secs}`, `CloudNodeRemoved{name}`,
`CloudCredentialChanged{provider, connected, error}` — none carries a credential.

The wizard calls these on **whatever engine the app is connected to**: the iPhone's own engine
on a phone-only install, or a desktop/hub engine. A fresh engine already has a mesh identity
(`<data_dir>/mesh/identity` is created on first start), so minting the ticket and having the node
join *is* forming the mesh: the phone becomes its first member with nothing to copy or paste.

### 16.3 The ticket: 15 minutes, single use, separate slot

The engine mints the ticket itself right before it creates the server (`mesh::RemoteTicket
{ttl_ms: 15 min}`), key-less like `t mesh pair --remote`. It is **not** the Settings pairing
string: that slot holds one 5-minute token shared by every `mesh_pairing_string` call, and the
user pairing a laptop while a node boots would otherwise consume the node's ticket (or the other
way round). Remote tickets live in their own list, are removed on first use and pruned on expiry,
and minting one opens no nearby window.

Why 15 minutes and not re-minting: re-delivery after boot needs a channel into the node, and the
node has none — no SSH, no inbound port, and user-data/Fly `files`/the Kubernetes Secret are
read once at creation. A longer single-use ticket is the only sound option. Exposure: whoever
presents it first becomes a peer and gets the vault keyring, so its lifetime matters; it travels
only through the provider's own metadata channel, which already holds the node (an attacker with
the user's provider account owns the node and its disk anyway), and it dies at first use. The
measured join is 232 s (§4); 15 minutes covers a slow mirror, a busy region and Fly/Kubernetes
image pulls of ~450 MB. If it expires first, the job marks the node failed and says so; destroy
(keeping the disk) and create again mints a new one.

### 16.4 The create job

A detached task in the `cloudnodes` actor, persisted in the node record before anything is made,
so it survives the UI closing and an engine restart (a `creating` record resumes watching the mesh
until its ticket's deadline). Stages (`types::Stage`), each a `CloudNodeProgress` event and a
record update:

1. **Server** — mint the ticket; the provider makes firewall (DO: zero inbound rules), volume and
   server, tagged `theuth-managed` + `theuth-node:<name>`; waits until the server runs.
2. **Storage** — the `/data` volume is attached.
3. **Install** — the node downloads and verifies the newest signed release (minisign, §5); the
   engine cannot observe this directly, so the stage ends when the node dials the mesh.
4. **Join** — a new peer named `<name>` appeared in `mesh_status`; waits until it is reachable.
5. **Ready**.

The percentage and "about N minutes left" are estimates from the measured 232 s.

### 16.5 Credentials

Vault rows `cloud/digitalocean` and `cloud/fly` (a backend family like `oauth/`: never listed, not
expandable into prompts), JSON `{access_token, refresh_token?, expires_at_ms?, source}`. Never
logged, never in an event or URL. Sources, in the order the UI offers them:

- **Sign in with DigitalOcean** (primary, phone and desktop) — §16.7.
- **Discovered on this machine** (desktop): doctl's `config.yaml` (each auth context), flyctl's
  `~/.fly/config.yml`, kubeconfig contexts. Discovery reads files and returns labels only; the token
  is copied into the vault when the user presses *Use it* (`cloud_import_credential`).
- **Paste a token** (secondary link): checked against the provider (`GET /v2/account`) before it is
  stored.

Vault rows replicate across the mesh, so a node holds the provider token too (it already holds
the whole keyring, §4).

### 16.6 Terminal and logs over the mesh (the escape hatch)

No SSH: *Open terminal* in a node's ⋯ menu calls `cloud_terminal(name)`, which forwards
`engine_terminal` to the node's peer id; the node opens a login-shell PTY job in an owner root
session and answers `{session, job}`. The UI attaches the existing `Terminal` component; PTY
input/resize/screen calls and the `subscribe_pty` stream are relayed to the engine that owns the
job (details in §16.9 as built). *View logs* calls `cloud_logs`, which forwards `engine_logs`: the
tail of the node's `logs/theuth.log.<date>`, redacted on the node.

### 16.7 Sign in with DigitalOcean

DigitalOcean's authorization-code flow needs the client secret, which must not ship in the app, so
the code exchange runs on theuth-relay (`relay.theuth.io`):

1. The engine makes `state` and `verifier` (32 random bytes each, base64url) and
   `challenge = base64url(sha256(verifier))`; `cloud_oauth_start` returns
   `https://relay.theuth.io/oauth/digitalocean/start?state=…&challenge=…`, which the app opens in
   the system browser (desktop) / Safari (iOS) with `openExternal`, the same helper every other
   external link uses.
2. The relay remembers `state → challenge` (10 min) and redirects to DigitalOcean's authorize page
   (`client_id` from its env, `scope=read write`, redirect
   `https://relay.theuth.io/oauth/digitalocean/callback`).
3. The callback exchanges the code with the client secret and keeps the token response under
   `state`, one-time; the page says to return to theuth. No token ever goes into a URL.
4. The engine polls `POST /oauth/digitalocean/redeem {state, verifier}` every 2 s (≤ 10 min):
   `202` pending, `200` the tokens once (the relay checks `sha256(verifier)` against the
   challenge, so a party that saw the state in the browser cannot redeem), then the row is gone.
5. Refresh: `POST /oauth/digitalocean/refresh {refresh_token}` when the access token is expired.

Relay env: `THEUTH_DO_OAUTH_CLIENT_ID`, `THEUTH_DO_OAUTH_CLIENT_SECRET` (the routes answer 503 when
unset). DigitalOcean app registration: name *theuth*, homepage `https://theuth.io`, callback URL
`https://relay.theuth.io/oauth/digitalocean/callback`.

### 16.8 UI flows

Mockup approved by the owner; screenshots of the fixture in `docs/design/224-assets/`.

- **Phone, no mesh (first run):** ① *Where should theuth run?* — an always-on cloud node (why:
  keeps working when the phone sleeps, runs long tasks and routines overnight, full tools, one
  memory across devices; from $12/mo, your own account) vs. *Just this iPhone*. ② Provider: DO
  (*Sign in with DigitalOcean*), Fly.io (paste a token, beta), AWS Lightsail (coming soon); prices
  for 2 GB; "the bill is from your own provider". ③ Name, region (nearest preselected from the
  time zone), the **watchable desktop** toggle, size cards, cost, *No open ports · updates itself ·
  watchable desktop*, **Create node · $25/mo** (4 GB + disk) — the press is the approval. ④ Real
  stages, "you can leave this screen". ⑤ *<name> is ready* → *Connect a model* (the
  model-provider sign-in), *Add my laptop too* (the existing pairing flow).
- **Size cards** (owner, supersedes the earlier 2-GB default): each card shows RAM/CPU, the monthly
  price (server + disk) and what it is for — 2 GB *minimum*: "Chat, coding, schedules and routines.
  Light browsing."; 4 GB, *Recommended* whenever the desktop is on: "Heavy computer use and
  browsing."; 8 GB: "Several agents at once, big repos, local tools." Default 4 GB with the desktop
  on, 2 GB with it off; 2 GB with the desktop on shows an inline note (not a blocker): "We recommend
  4 GB for computer use and browsing." Kubernetes offers the same three as memory request = limit;
  Fly the matching machine presets. Size and desktop changes update the create button's price live.
- **Desktop wizard dialog:** step rail Provider / Configure / Create with the "why" note; provider
  cards show discovered credentials; Configure has name/region, the desktop toggle, the size cards
  and a collapsed *Advanced* (disk 10 GB kept on delete, SSH access *Off (no inbound ports)*,
  updates follow stable). The Kubernetes variant picks context, namespace (+ create), storage class
  (flags a cluster with no default class) and offers *Block cluster network*, with a warning when
  the CNI is flannel (detected from kube-system), which does not enforce NetworkPolicies.
- **Existing mesh:** Settings › Machines lists cloud nodes with provider chip, region,
  direct/relay, version + auto-update, monthly cost and a ⋯ menu (Restart, Resize, Update now,
  View desktop, Open terminal, View logs, Destroy — the confirmation names the node and the monthly
  saving, and offers to keep the disk); *＋ Add cloud node* opens the wizard without the "why"
  screen. Phone: an *Add a cloud node — always on, joins this mesh* entry for the engine switcher.

### 16.10 The watchable desktop

Owner addition: every node gets a desktop agents can use (headed browser, `computer_*` tools) and
the owner can watch and take over, on by default and chosen at creation ("Watchable desktop — lets
agents use a browser and apps you can watch and take over"; off = a terminal-only node).

- **VM path** (`vm-cloud-init.sh`, placeholder `__DESKTOP__`): `Xtigervnc :0` at 1440x900x24 — a
  virtual X display that is also the VNC server, the same server the sandbox VM image uses
  (`deploy/vm`), so no separate Xvfb + x11vnc — listening for RFB on `localhost:5900` only;
  `openbox` (lighter than the sandbox image's XFCE; no panel, no session manager); `websockify
  127.0.0.1:6080 localhost:5900`. Three units (`theuth-xvnc`, `theuth-wm`, `theuth-websockify`),
  and a drop-in gives `theuth-engine.service` `DISPLAY=:0` and orders it after the window manager.
  The engine only sets `DISPLAY`; which subsystems use a display is decided by the presence of one
  (feat/node-slim), not by `--headless`. The VNC password is 8 random characters (VncAuth's
  limit), in `/data/desktop.json` (0600, `engine`) for the engine to hand to the owner, and in
  `vncpasswd` form for the server; it survives re-creation on a kept disk.
- **Streaming: the sandbox VM desktop's protocol, carried over the mesh.** The app's viewer is the
  same noVNC client the sandbox VM pop-out uses (WebSocket to websockify, VncAuth). Nothing listens
  on a public address and the firewall has no inbound rule, so `engine_desktop(peer)` on the
  owner's engine asks the node for `desktop.json` (forwarded RPC), opens a loopback listener on
  the owner's engine and pipes every connection over a mesh stream (`theuth/desktop/1`) to the
  node, which connects only to `127.0.0.1:<its websockify port>`. `cloud_desktop(name)` returns
  `{ws_url: ws://127.0.0.1:<local>/websockify, password, geometry, engine}`. Like the sandbox VM
  view, the URL is local to the engine the app is connected to: the desktop app and an iPhone
  with its own engine view it; a phone remoting into a desktop engine does not (same limit as
  sandbox VMs).
- **Take over**: input off while watching; *Take over* turns it on (red frame), *Hand back* off.
  Unlike sandbox VMs it does not steer the node's sessions (no takeover protocol on a peer yet).
- **Container path (Fly, Kubernetes): follow-up.** The engine image has no X stack; adding
  Xtigervnc + openbox + websockify to `deploy/computer/Dockerfile` and starting them from the
  entrypoint is straightforward, but grows the image and was not built or run here, so the toggle
  is shown disabled for those providers.
- **Enabling/disabling after creation: follow-up.** Disabling is three `systemctl disable --now`
  on the node; enabling needs the packages installed (apt on the node). Both need a small
  node-side RPC (the engine has passwordless sudo); not built. Today: destroy keeping the disk and
  create again with the other choice (the node comes back as the same engine).
