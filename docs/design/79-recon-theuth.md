<!-- Recon fact sheet for design 79, produced by a read-only subagent on 2026-09-29; cited by 79-sandbox-vm.md §2. Reported facts, not decisions. -->

# 79 recon: theuth as built on 2026-09-29, for the VM-engine design

I read the tree and changed nothing. I did not run cargo. Paths are relative to `/Users/Sky.Moore/Develop/theuth`. `crates/X/src/` is shortened to `X/` where it is unambiguous. Code wins over docs.

**The short version:**
- **Sandbox crate:** it exists and works from the CLI only (docker and ssh). It has no RPC, no tools, no UI, no config section and no VM backend.
- **Mesh:** a new engine joins by pairing. Once paired it gets the whole vault by replication.
- **What an engine advertises:** only `class`, `harnesses`, `repos`, `display` and `gpu`. There is no os, arch, hostname or "vm" field.

## A. Mesh

**Crate files**
- `crates/mesh/src/{lib,net,pairing,identity,config,lease,stats,store,tools,types}.rs`, plus `migrations/0001–0004`.
- Plugin order is mesh → replica → sessionsync → roster → tasks → sandbox (`theuth/src/main.rs:1192-1199`).

**Identity and data dir**
- `<data_dir>/mesh/identity` holds a 32-byte Ed25519 secret, mode 0600, created on first run. The engine id is the iroh public key as text (`mesh/identity.rs:1-3, 22-41, 53-56`).
- `first_seen_ms` is the identity file's creation time; it drives probation (`identity.rs:42-46`).
- `DIR = "mesh"`, `DB = "mesh.sqlite"` (`mesh/lib.rs:52-59`).
- The peer set is the `peers` table in `mesh.sqlite`, not a separate file (`migrations/0001_mesh.sql:5-11`). Removal is a tombstone column, `removed_ms` (`0002_mesh_peer_removed.sql:1-5`).

**iroh endpoint and relay**
- Built with `Endpoint::builder(presets::Minimal).secret_key(..).alpns(..).relay_mode(..)` (`mesh/net.rs:315-323`).
- `mesh.relay` values: `""`, `none` or `off` means no relay; `public` means n0's relays; anything else is a URL (`net.rs:300-311`). Default is `https://iroh.relay.theuth.io` (`mesh/config.rs:51`).
- The network join is lazy. Relay and discovery start only once a peer exists (`net.rs:400-442`; 67 §20 row, `docs/design/67-agents.md:518`).

**Discovery**
- pkarr publisher and resolver, DNS lookup and `iroh_mdns_address_lookup::MdnsAddressLookup` (`net.rs:414-435`).
- Gated by `mesh.discovery` (`config.rs:15-17`).

**ALPNs**
- Control: `ALPN_MESH = "theuth/mesh/1"` (`net.rs:37-39`).
- Streams: `theuth/{records,acp,sessions,knowledge,rpc,artifacts,docs}/1` (`net.rs:278-287`; constants at `replica/src/lib.rs:41-55`).
- Gossip: `theuth/gossip/1`.

**Connections**
- The lower id dials. The higher id waits `DIAL_GRACE` (3 s), then dials itself (`net.rs:6-8, 51-52`).
- Backoff is 1–60 s; dial timeout is 30 s (`net.rs:47-50`).

**Pairing string**
- Format: `1.<BASE32_NOPAD(json)>` with `{ id, name, relay, addrs, token, vault_key? }` (`mesh/pairing.rs:4-7, 21-39, 76-92`).
- TTL is 300 000 ms. The token is 16 random bytes in base32 (`pairing.rs:18-19, 61-68`).
- `pairing_string()` joins the network, waits up to 3 s to be online, and adds `vault_key` from `secrets::GetMeshKey` (`net.rs:522-559`).

**Pairing handshake**
- `add_peer`: dial `ALPN_MESH`, then `PairHello{token, me, peers}`, answered by `PairAck{…}` or `PairReject{reason}`. Both sides then merge each other's peer sets, which makes pairing mutual and transitive (`net.rs:56-75, 561-627`).
- The joiner adopts the inviter's vault key with `secrets::AdoptMeshKey` (`net.rs:602-615`).
- Peer-set merge is last-writer-wins on the entry's stamp; on a tie, a removal wins (`pairing.rs:94-135`).

**CLI: `t mesh …`** (`theuth/src/main.rs:170-231`, dispatch at `:307-368`)

| Command | RPC method |
|---|---|
| `status` | `mesh_status` |
| `pair` | `mesh_pairing_string` |
| `add <pairing>` | `mesh_add` |
| `remove <id>` | `mesh_remove` |
| `hub prefer\|eligible\|never` | `mesh_set_hub` |
| `devices list\|add [name]\|revoke <id>\|rename <id> <name>` | `devices_list` / `device_pair_start` / `device_revoke` / `device_rename` |

- `t mesh add --ssh TARGET [--key NAME | --system-ssh] | --docker [--name N] [--image I | --local-t PATH]` goes to `sandbox::make_engine` (`main.rs:176-215, 310-338`).
- The doc comment at `main.rs:141-142` still says every subcommand "answers `not built`". That comment is stale.
- The TUI `/mesh` command only handles `devices …` (`tui/src/model.rs:1504-1508, 9005-9033`).

**Peer record: `Peer`** (`mesh/types.rs:87-106`)
- Fields: `id: EngineId, name: String, addrs: Vec<String>, reachable: bool, last_seen_ms: u64, hub_pref: HubPref`.
- `EngineId(pub String)` is `serde(transparent)` (`types.rs:10-26`).
- `HubPref { Prefer, #[default] Eligible, Never }` (`types.rs:46-70`).

**What an engine advertises: `EngineStats`** (`types.rs:108-129`)
- Fields: `id, uptime_since_ms, class: EngineClass, availability_30d: f32, harnesses: Vec<String>, repos: Vec<String>, display: bool, gpu: bool`.
- `EngineClass { Server, Desktop, Laptop, Container }` (`types.rs:72-85`).
- There is **no** `os`, `arch`, `hostname`, `platform`, `browsers_signed_in`, `load` or VM field. 67 §8 designed `{ os, arch, harnesses, repos, display, browsers_signed_in, gpu, load }` (`67-agents.md:397`).
- The `engine_stats` table matches the struct: `class TEXT -- server | desktop | laptop | container` (`migrations/0001_mesh.sql:13-23`). Migration 0003 adds `hub_pref` and `first_seen_ms` (`0003_replicable.sql:13-14`).

**How the stats are probed** (`mesh/stats.rs:11-38`); nothing is declared
- Class is `Container` if `/.dockerenv` or `/run/.containerenv` exists, or `/proc/1/cgroup` names docker, containerd, kubepods, lxc or podman (`stats.rs:40-52`).
- Otherwise `Laptop` if there is a battery: `pmset` on macOS, `/sys/class/power_supply/BAT*` on Linux (`stats.rs:54-77`).
- Otherwise `Server` if there is no display, else `Desktop` (`stats.rs:17-27`).
- Display on Linux means `DISPLAY` or `WAYLAND_DISPLAY` is set in `t`'s environment. On macOS and Windows it means not in a container (`stats.rs:79-87`).
- GPU: always true on Apple Silicon; on Linux, `/dev/nvidia0` or `/dev/dri/renderD*` (`stats.rs:89-102`).
- Harnesses: `native` plus the keys of `acp/agents.yaml` (`stats.rs:104-113`). Repos: git roots up to two levels under `mesh.workspaces` (`stats.rs:115-130`).
- Stats are re-probed every `capabilities_secs` (minimum 30 s) and written only when they change (`mesh/lib.rs:221-241, 784-795`).
- There is **no config key to declare a class or capability**; `MeshConfig` has none (`config.rs:9-46`).

**Hub lease**
- The election is a pure function, `lease::elect(candidates, incumbent, now_ms, probation_ms, handover_ms)` (`mesh/lease.rs:126-153`).
- Ranking order: `hub: prefer` first, then availability bucket (≥99.9 / 99 / 95 %), then class (`Server = Container > Desktop > Laptop`), then older uptime, then id ascending. `Never` is filtered out (`lease.rs:79-119, 133-136`).
- Guards: probation (default 21600 s) and handover (default 600 s) (`lease.rs:121-151`).
- Recompute runs over the connected set; the live QUIC connection is the liveness signal (`lib.rs:259-347`, comment at `:289-293`).
- Gossip topic is `sha256("theuth/mesh/lease/1")` (`lib.rs:369`). Messages are `GossipMsg::{Heartbeat{id, is_hub, stats_hlc}, LeaseRelease{id}}` (`lib.rs:130-143`).
- Defaults: `heartbeat_secs` 10, `lapse_secs` 30 (`config.rs:57-59`). The lease is released on stop (`lib.rs:504-513`).
- Hub duties read `mesh::is_hub()` / `set_hub` and `oauth::set_hub` (`lib.rs:61-75, 210-216`). `lease::Follower` is how link, slack, signal and roster track the lease (`lease.rs:1-4, 11-57`).

**`mesh_peers` tool** (`mesh/tools.rs:24-104`)
- Risk `ReadOnly`; one argument, `reachable: bool`.
- One line per engine: `"{name} id={id} reachable|unreachable class={class} harnesses={a,b} repos={..|-} display={bool} gpu={bool}[ hub]"` (`tools.rs:91-97`). The local engine's row is named `"this engine"` (`tools.rs:58-65`).

**Mesh actor**
- Messages: `Status, AddPeer(String), RemovePeer(EngineId), SetHubPref(HubPref), PairingString, ListStats, Subscribe` (`lib.rs:97-128`).
- `SetHubPref` writes `mesh.hub` into config.yaml (`lib.rs:567-585`).
- **`MeshEvent` is not forwarded to RPC**: it is missing from the bridge list (`rpc/src/bridge.rs:92-124`). The UI just queries `mesh_status` (`crates/desktop/ui/src/components/tasks/queries.ts:59-62`).

**Replicated records**

| Crate | What replicates | Where |
|---|---|---|
| mesh | `peers`, `engine_stats`, `availability`, `mesh_meta` | `mesh/lib.rs:751-758` |
| secrets | `vault`, `vault_current` | `secrets/store.rs:17`, `secrets/lib.rs:566` |
| link | `devices`, `pending` | `link/src/store.rs:23` |
| document | `docs`, plus the `doc_files` mirror | `document/src/store.rs:44`, `document/src/lib.rs:300-305` |
| roster | `agents`, `budget` | `roster/src/lib.rs:2008` |
| tasks | `tasks`, `wakes`, `answered` | `tasks/src/lib.rs:961` |
| agent | `projects`, `cost_owners` | `agent/src/store.rs:691` |
| files as rows | skills, schedules, mcp (`mcp/repos/` excluded) | `agent/src/supervisor.rs:7430-7437`, `schedule/src/lib.rs:2295-2301`, `tools/src/mcp.rs:1691-1697` |
| OAuth | grants are vault rows `oauth/<ns>` | `secrets/vault.rs:113-114` |
| sessions | transcripts | `crates/sessionsync` (design 68) |

- Never replicated: `signal/presage.db` and each engine's own iroh identity (`67-agents.md:251`).

**How a session is opened on another engine**
- `spawn_subagent{agent}` runs `canonical_agent` and `parse_agent` (`agent/src/peers.rs:216-241`). `parse_agent` splits on the last `@`, so `"omp@build-A"` gives `(Some("omp"), "build-A")` (`peers.rs:120-132`).
- `resolve_node` matches by name, or by a unique engine-id prefix (`peers.rs:134-150`). `omp@<this engine>` becomes the local `omp` (`peers.rs:152-167`).
- The supervisor's `SpawnChild` handling is at `agent/src/supervisor.rs:3168-3237`. `acp_session` finds a node in `self.nodes` and uses `acp::Transport::Dial(dialer)` with `SessionMeta{model, effort, tools, parent, parent_engine, harness}` (`supervisor.rs:767-830`).
- **Every mesh peer is registered as a node** by `rpc::acp::serve_mesh`, which sends `RegisterNode{name, describe, dialer: MeshDialer}` (`rpc/src/acp.rs:166-269`). The description is `"mesh · <class> · harnesses: …"` or `"mesh · offline"`. Name collisions get an id suffix (`acp.rs:271-285`). It is started from the RPC plugin (`rpc/src/lib.rs:108`).
- `MeshDialer` opens `Transport::open(peer, ALPN_ACP)` and frames newline-delimited JSON-RPC (`acp/src/mesh.rs:15-80`).
- On the receiving engine, each inbound `theuth/acp/1` stream is served as one ACP connection (`rpc/src/acp.rs:189-214`). `_meta.theuth` is parsed at `acp.rs:436-446`; its schema is in `acp/src/meta.rs:13-40`.
- `pub trait Dialer { async fn dial(&self) -> Result<LineDuplex,String>; fn caller_id(&self)->Option<String>; fn engine_id(&self)->Option<&str> }` (`acp/src/harness.rs:108-124`).
- `enum Transport { Process(AcpAgentConfig), Dial(Arc<dyn Dialer>) }` (`harness.rs:126-133`).
- Placement uses the advertised stats: words like browser, screen, display or screenshot in a task keep only engines with `stats.display` (`tasks/src/place.rs:75-76, 147-154`).

**`mesh:` config keys** (`mesh/config.rs:48-66`)

| Key | Default |
|---|---|
| `relay` | `"https://iroh.relay.theuth.io"` |
| `discovery` | `true` |
| `name` | `""` (hostname) |
| `bind` | `None` |
| `hub` | `eligible` |
| `probation_secs` | 21600 |
| `handover_secs` | 600 |
| `heartbeat_secs` | 10 |
| `lapse_secs` | 30 |
| `capabilities_secs` | 300 |
| `workspaces` | `[]` |
| `availability_flush_secs` | 600 |
| `storm_frames_per_sec` | 50 |

- There is no `enabled` key: a grep for `enabled` in `crates/mesh` returns nothing.
- The plugin logs "mesh unavailable" and keeps running if identity, store or bind fails (`lib.rs:641-679`).

## B. Nodes (design 32): what is left

**Still built and running**
- `NodesPlugin` is in the plugin vector (`theuth/src/main.rs:1190`). Files: `nodes/{lib,conn,dialer,client,store,types}.rs`.
- Config is `NodesConfig { enabled: bool /* true */ }` (`nodes/types.rs:12-24`).
- `Node` fields: id, name, relay, platform, version, paired_at_ms, last_seen_ms, online, revoked, children (`types.rs:26-50`).
- Events and messages: `NodeEvent`, and `Status, Add, Remove, Rename, Invite` (`types.rs:52-118`).
- Data: `<data_dir>/nodes.yaml` (`nodes/store.rs:1-3, 84-86`).

**Control connection method list**
- `pub const METHODS: [&str; 4] = ["info", "link_pair", "link_revoke_device", "link_status"];` (`nodes/client.rs:15-17`). A test checks each name against `bindings/methods.json` (`client.rs:118-131`).
- **None of `set_secret`, `add_mcp_server`, `add_skill` or `oauth_import` is on the control connection.**
  - The first three do exist as ordinary RPC methods (`rpc/src/api.rs:723, 978, 1640-1646`), but the nodes plugin never calls them.
  - `oauth_import` does not exist anywhere; a grep over `crates/` and `bindings/` returns nothing.
  - Design 64 had planned to grow this list (`64-bots.md:578-585`); that was never built.

**Dialer**
- `NodeDialer` opens one link-relay mode-1 connection per child with a Noise handshake (`nodes/dialer.rs:1-83`).
- Nodes are registered through the same `RegisterNode` message (`nodes/lib.rs:129-140`), described as `"<platform> · theuth <version>"` (`lib.rs:94-101`).
- There is no `AcpChild` type in nodes; the node child is just `acp::Transport::Dial`. The mirror `AcpChild` from 64 was withdrawn (`67-agents.md:508`).

**RPC and TUI**
- RPC: `list_nodes, add_node, remove_node, rename_node, invite_node` (`rpc/src/api.rs:1123-1140`).
- TUI: `/nodes [add <key>|rm <n>|name <n> <name>|invite <n>]` (`tui/src/model.rs:1509-1513`).

**`t --install-service`**
- Writes `link.enabled: true`, installs a launchd / systemd-user / Task Scheduler unit and waits for the RPC port (`theuth/src/service.rs:28-53`).
- Then it prints the *node* pairing hint "/nodes add <key>" and "Settings › Nodes › Add node" (`service.rs:71-74`). `--no-pair` skips that (`main.rs:74-91`).
- The Settings › Nodes tab no longer exists; the add-node field now lives inside Settings › Mesh (`Mesh.tsx:207-263`).
- Sandbox bring-up uses `--install-service --no-pair` followed by `t mesh pair` (`sandbox/bringup.rs:130-157`).

**Which path `spawn_subagent{agent:"<name>"}` takes today**
- Both paths feed one map, `Agents.nodes`. `RegisterNode` does a map insert, so a later registration under the same name replaces the earlier one (`supervisor.rs:6629-6637`).
  - A mesh peer gets a `MeshDialer`: iroh, `ALPN_ACP`.
  - A design-32 node gets a `NodeDialer`: the 31 relay.
- 69's note: the per-engine link identity's only remaining user is `nodes`; retiring it is "the natural follow-up" (`69-devices.md:111`).

## C. Sandbox (`crates/sandbox`)

**It exists as code:**
- `src/lib.rs` (475 lines), `bringup.rs` (450), `docker.rs` (457), `ssh.rs` (386), `release.rs` (136), `record.rs` (103)
- `tests/{ssh,docker}.rs` (both `#[ignore]`)
- `Cargo.toml`

**The trait, verbatim** (`sandbox/lib.rs:176-202`):
```rust
#[async_trait::async_trait]
pub trait Sandbox: Send + Sync {
    fn kind(&self) -> &'static str;                       // "docker" | "ssh"
    async fn create(&self, out: &Progress) -> anyhow::Result<Handle>;   // idempotent
    async fn exec(&self, h: &Handle, cmd: Cmd) -> anyhow::Result<ExecOutput>;
    async fn put(&self, h: &Handle, path: &str, bytes: Bytes, mode: u32) -> anyhow::Result<()>;
    fn durable_roots(&self) -> Vec<String>;
    fn daemon(&self) -> DaemonKind;
    async fn capabilities(&self, h: &Handle) -> anyhow::Result<Capabilities> { /* detect() + constants */ }
    async fn destroy(&self, h: &Handle, preserve_data: bool) -> anyhow::Result<()>;
}
```
- There is no `status`, `start`, `stop`, `recreate`, `snapshot` or `restore`. Those were in 64 §4.1 (`64-bots.md:365-387`) but not built (`67-as-built-S.md:41-42`).

**Types**
- `Handle { id: String }` (`lib.rs:76-82`)
- `Cmd { line: String, stdin: Option<Bytes>, timeout: Duration }` (`lib.rs:84-111`)
- `ExecOutput { status: i32, stdout: Vec<u8>, stderr: Vec<u8> }` (`lib.rs:113-148`)
- `DaemonKind { Entrypoint, Service }` (`lib.rs:150-158`)
- **`Capabilities { os: String, arch: String, durable_roots: Vec<String>, daemon: DaemonKind }`** (`lib.rs:160-171`). No screen, display, resources or mounts.
- `Progress(Arc<dyn Fn(&str,&str)+Send+Sync>)` with only stdout, log and silent sinks (`lib.rs:45-74`)
- `detect()`: `uname -sm`, with a PowerShell fallback (`lib.rs:204-221`)

**Targets and actor**
- `enum Target { Ssh{target, key: Option<String>, system: bool, name}, Docker{name, image: Option<String>, local_t: Option<PathBuf>} }` (`lib.rs:248-272`).
- `SANDBOX_ACTOR = "sandbox"` (`lib.rs:36-37`). Messages:
  - `MakeEngine(Target)` → `Result<mesh::Peer,String>`
  - `Engines` → `Result<Vec<Computer>,String>` (`lib.rs:274-325`)
- The one stub-like marker: `// ponytail: bring-up runs inside the handler (minutes) so one engine is made at a time; spawn + progress events if the UI ever drives this.` (`lib.rs:292-293`). A grep for TODO, FIXME, `unimplemented!` or `todo!` finds nothing else.
- `make_engine(data_dir, target, secret, join, out)` is the single entry point (`lib.rs:351-412`). `SandboxPlugin::start` reads no config (`lib.rs:414-428`).

**Bring-up** (`sandbox/bringup.rs`)
- `pub trait MeshJoin { async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer>; async fn status(&self) -> anyhow::Result<mesh::MeshStatus>; }` (`bringup.rs:14-22`)
- `Spec { name, local_t, pair_tries: 6, verify_tries: 3, verify_every: 10 s }` (`bringup.rs:24-50`). `BIN_DIR = "$HOME/.local/bin"` (`:61-62`).
- `run()` steps (`bringup.rs:73-182`):
  1. create
  2. detect; Windows is refused (`:87-91`)
  3. binary: "in the image" for docker; for ssh, the `--local-t` file, a present `t`, or `release::fetch`, then `put` with mode 0755 (`:93-117`)
  4. first_run: `t --secret-list` (`:118-123`)
  5. daemon: `t --install-service --no-pair --name` (`:127-144`)
  6. pair: `t mesh pair`, retried, then `MeshJoin::add_peer` (`:146-163`)
  7. verify: poll status until the peer is reachable (`:165-181`)

**Docker backend (bollard)**
- Connects via `DOCKER_HOST`, else `~/.docker/run/docker.sock`, else the platform default (`docker.rs:59-107`).
- Container `theuth-engine-<name>`; volumes `theuth-<name>-{data,workspace}`; labels `io.theuth.{engine,image,schema}` (`docker.rs:31-38, 109-119`).
- Both Dockerfiles are compiled in with `include_str!` (`:40-41`). The image is pulled, built from the release Dockerfile, or built from the dev Dockerfile (`:131-200`).
- `create` adopts a matching container or recreates a mismatched one. It sets env `THEUTH_DATA_DIR=/data`, binds **only the two named volumes**, uses restart `unless-stopped`, and publishes no ports. **No CPU or memory limits, no devices, no host mounts** (`docker.rs:260-313`).
- `exec` = `create_exec /bin/sh -lc` (`:315-370`). `put` = `upload_to_container` with a one-file tar owned by uid 1000 (`:372-389`).
- `durable_roots` are `/data` and `/workspace`; daemon kind is `Entrypoint`; `destroy` removes the container and optionally the volumes (`:391-430`).

**SSH backend**
- Built-in transport is `russh` with a key from the vault and hosts checked against `~/.ssh/known_hosts`; the system transport runs `ssh` with `BatchMode=yes` (`ssh.rs:1-12`).
- `create` probes with `echo ok` (`:276-296`). `put` = `mkdir -p && cat > && chmod` (`:312-333`).
- `durable_roots` is `$HOME/.theuth`; daemon kind is `Service`; `destroy` runs `t --uninstall-service` and optionally `rm -rf` (`:335-364`).

**Release fetch and record**
- `release.rs` reads `latest.json`, picks `platforms["cli-<os>-<arch>"]`, verifies with minisign, and returns the `t` bytes (`release.rs:1-6, 37-60`).
- `Computer { name, backend, handle, engine: mesh::EngineId, created_ms }` is written to `<data_dir>/computers/<name>.yaml` (`record.rs:1-3, 11-24`).

**Surface that does not exist**
- **No RPC methods:** grep `sandbox` in `crates/rpc` returns nothing.
- **No tools, no desktop UI:** grep `sandbox|computers/|make_engine` in `ui/src` returns only unrelated hits.
- **No `sandbox:` config section:** timeouts are constants (`STEP_TIMEOUT` 600 s, `lib.rs:42-43`; `67-as-built-S.md:36`).
- The only way in is the CLI. It runs bring-up **in the CLI process** and joins through the running engine's `mesh_add` / `mesh_status` via `RpcJoin` (`main.rs:423-475`; `67-as-built-S.md:35`).
- `t mesh remove` does not destroy the compute (`67-as-built-S.md:42`).

**Dependencies**
- Workspace: `members = ["crates/*"]` (`Cargo.toml:3`); `russh = "0.63"`, `bollard = "0.21"` (`Cargo.toml:133-135`); `iroh 1.2` (`tls-aws-lc-rs`), `iroh-gossip 0.101`, `iroh-mdns-address-lookup 0.5`, `data-encoding 2` (`Cargo.toml:48-55`).
- The sandbox crate's own dependencies: `sandbox/Cargo.toml:8-33`.
- **No VM-related dependency anywhere.** A grep for `microsandbox|libkrun|krun|firecracker|microvm|vfkit|qemu|lima|utm|Virtualization.framework|cloud-hypervisor|kvm|hvf` over `crates/` and `Cargo.lock` returns nothing.

**The image** (`deploy/computer/Dockerfile:1-42`)
- `ubuntu:24.04` plus `ca-certificates curl git python3 ripgrep tar zstd minisign`.
- The release tarball is minisign-verified; user `engine`, uid 1000; volumes `/data` and `/workspace`; `ENTRYPOINT ["t","--headless","--data-dir","/data"]`.
- The file's own header says "no display, no published ports". `Dockerfile.dev:1-25` is the same image with a copied-in `t`.

## D. Secrets across engines

**Storage**
- Rows live in `<data_dir>/secrets.sqlite`, one per `(name, version)`, each sealed with XChaCha20-Poly1305 under a **mesh key** that every paired engine shares. The mesh key itself sits in the local `meta` table, sealed under this engine's own key (`secrets/vault.rs:1-11`; `store.rs:1-3, 13-17`).
- The legacy `secrets.vault` file is migrated on first start (`vault.rs:42-45`).

**Key store**
- `KeyStore { Auto (default), Keychain, File }` (`vault.rs:51-62`).
- Keychain item: service `theuth`, account `secrets:<data_dir>`. File: `secrets.key`, base64, mode 0600. `Auto` falls back to the file with a warning (`secrets/key.rs:1-4, 24-40`).
- Config: `SecretsConfig { key_store, tools: true, request_timeout_secs: 1800 }` (`secrets/lib.rs:46-67`; `README.md:189-192`). There is deliberately no `enabled` key (`37-secrets.md:721-725`).

**How a secret reaches another engine: by replication**
- `vault` and `vault_current` attach to the replica once it is up (`secrets/lib.rs:551-580`).
- The mesh key travels as `vault_key` inside the pairing string (`pairing.rs:34-38`; `net.rs:539-558`). The joiner calls `AdoptMeshKey`, and "local rows are re-sealed under it" (`net.rs:602-615`; `secrets/lib.rs:154-157, 393-404`). `GetMeshKey` is never exposed over RPC (`lib.rs:149-152`).
- 67 §20 row: "vault rows + a mesh data key carried in the pairing string, sealed locally per engine" (`67-agents.md:532`).
- **Consequence: any engine that pairs receives the whole vault.** There is no allowlist; 64's bundle allowlist (`64-bots.md:670-677`) was not built.
- `set_secret` over the node control connection: not used (see B).

**Where ``{{secret:…}}` (the 37 placeholder, NAME elided here)` is substituted**
- Regex: `\{\{secret:([A-Za-z0-9_.\-]+)(?::-([1-4]))?\}\}` (`vault.rs:39-40`).
- The hook is the `tools::secrets::Secrets` trait with `expand` and `redact` (`tools/src/secrets.rs:1-66`).
- In the tool registry's `invoke`, placeholders are expanded **after** the approval gate and before the tool body (`tools/src/registry.rs:1543-1550`). Output is redacted on every path (`:1581-1582`).
- A mentioned placeholder raises the call to Ask (`registry.rs:1453-1458`), unless the tool sets `CONSUMES_SECRETS = false` (`tools/src/lib.rs:315-325`). MCP expansion is at `tools/src/mcp.rs:1300`.

**Remote child**
- A native child on a node or mesh peer is a session in *that* engine's registry, so placeholders resolve against that engine's vault (`37-secrets.md:623-625`; `67-agents.md:251`: "resolves on the executing engine").
- An external ACP harness sees the literal text (`37-secrets.md:620-622`).

**`secret_request` and `secret_capture`**
- `secret_request`: the actor publishes `SecretsEvent::Requested{id, session, name, note, current}` on the **local** bus, waits, and stores the answer as `source: user` (`secrets/lib.rs:159-192, 266-342`). It is bridged to RPC (`bridge.rs:114, 162`) and answered with `answer_secret` (`api.rs:1653-1657`).
- **These events are not relayed across engines.** `sessionsync::EventRelay` relays `AgentEvent` only (`sessionsync/src/events.rs:8-13, 29`), and `rpc/src/acp.rs` has no secret forwarding (grep → nothing).
- `secret_capture` runs one tool and stores its output. It is `Risk::ReadOnly`; the inner tool takes its own approval (`secrets/tools.rs:182-240, 213-214`). All four `secret_*` tools are ReadOnly (`tools.rs:76-77, 138-139, 213-214, 325-326`).
- `mesh/*` rows (for example `mesh/device_key`) are reserved and never expandable (`vault.rs:116-119`; `69-devices.md:9`).

**OAuth**
- Grants are `oauth/<ns>` vault rows. Only the hub refreshes, via a process-global HUB flag (`oauth/src/lib.rs:7, 53-57, 491-493`; `67-agents.md:166`).

**The documented rule for a non-hub engine** (67 §5.4, `67-agents.md:250-251`)
- The vault replicates. ``{{secret:…}}` (the 37 placeholder, NAME elided here)` resolves on the executing engine, "which now always has it".
- The hub is the only OAuth refresher. Log in once, anywhere.
- `secret_request` prompts "via the hub's channels".
- This supersedes 37 §7's "Nothing crosses the link" (`37-secrets.md:623-625`) and 64 §5.1's three routes (`64-bots.md:665-696`).

**CLI**
- `t --secret-set NAME` (`main.rs:92-103`). Sandbox reads the SSH key with `Vault::open` (`main.rs:444-456`).

## E. Computer control and display

**Tools**
- Eight tools: `computer_screenshot` (ReadOnly), and `computer_{move,click,drag,scroll,type,key,open}` (Mutating) (`computer/tools.rs:143, 207, 238, 272, 305, 332, 350, 385`).
- Config: `computer: { enabled: true, max_edge: 1568 }` (`computer/lib.rs:30-49`).

**Backends**
- Capture: macOS shells out to `/usr/sbin/screencapture`; everything else uses xcap's `capture_image`. xcap enumerates displays on every OS (`computer/screen.rs:1-5, 155-212`).
- Input: enigo on one thread, `Enigo::new(&Settings::default())` (`computer/input.rs:1-2, 153`).
- There is no Linux-specific code in theuth itself.

**Linux is covered through the libraries**
- xcap 0.9.8 depends on `xcb`, `libwayshot-xcap`, `pipewire` and `zbus` (`Cargo.lock:24262-24290`).
- enigo 0.6.1 depends on `x11rb` and `xkbcommon` (`Cargo.lock:5087-5105`).
- 58 decision 11: computer control ships on Windows and X11; Wayland is a gated portal backend (`58-cross-platform.md:121-129`).
- **`DISPLAY` is not read in `crates/computer`** (grep → nothing). The libraries use the process environment. Only `mesh/stats.rs:79-87` and `desktop/src/island/linux.rs:29-33` read `DISPLAY`.

**No seam for a remote display**
- A grep for `vnc|rfb|novnc|websockify|xvfb|x11vnc|tigervnc|xfce` in `crates/` finds nothing. The only near-hit is an "xdotool-style" comment in `cursor/proto/agent.proto:1263`.
- There is no noVNC package in `crates/desktop/ui/package.json:13-67`.
- Tools run in the engine that owns the session, so a `computer_*` call on a remote child drives that engine's own local display.

**Browser live view**
- The browser is an embedded obscura V8 headless browser (`browser/lib.rs:1-7`).
- `browser_show` publishes `BrowserEvent::ShowLive`, wire name `"BrowserShowLive"` (`browser/lib.rs:262-273`; `browser/tools.rs:678-680`).
- `Screenshot{session}` returns a base64 PNG (`browser/lib.rs:252-260`), served over RPC as `browser_screenshot`.
- The desktop's `BrowserLive.tsx:1-27` is a `Dialog` with `<img src="data:image/png;base64,…">`, polled every 500 ms by react-query and opened by `App.tsx:328`. There is no iframe or webview for it.

**How the desktop can show other content**
- Pop-out windows: `open_window(label, url)` uses `WebviewWindowBuilder` with `WebviewUrl::App(url)`, **app routes only**, and the label must start with `popout-` (`desktop/src/commands.rs:12-48`).
- **No child webviews or multi-webview:** a grep for `WebviewBuilder|add_child` finds nothing.
- iframes are used only for artifacts (PDF blob `Viewer.tsx:269-273`; HTML `srcDoc` with `sandbox=""` `Viewer.tsx:336`) and attachments (`Attachments.tsx:113`).
- **The CSP blocks an http iframe:** `frame-src 'self' asset: http://asset.localhost blob:`. It does allow WebSockets: `connect-src … ws://127.0.0.1:* ws://localhost:* wss: ws:`, and `img-src … https: http:` (`crates/desktop/tauri.conf.json:47-52`). So a noVNC page at `http://localhost:6080` could not be framed today.
- External URLs open in the system browser via `tauri-plugin-opener`'s `openUrl` (for example `Viewer.tsx:3, 50`; MCP login in `library/Tools.tsx:107`). Capability: `opener:default` plus `allow-open-url` for `x-apple.systempreferences:*` (`capabilities/default.json:22-30`).

## F. Desktop app

**Settings screens** (`components/settings/SettingsDialog.tsx:43-59`)
- Tabs: `general, chat, loop, approvals, communication, computer, meetings, search, labs, agents ("Harnesses"), mesh, link, connections, channels, shortcuts`.
- Labs shows only while `decision.enabled` (`SettingsDialog.tsx:107-109`).
- There is **no Nodes, Engines, Devices or Sandbox tab**. Pages live in `crates/desktop/ui/src/components/settings/*.tsx`; the route is `routes/settings.$tab.tsx`.

**Settings › Mesh** (`settings/Mesh.tsx`)
- This node: name, id, mesh name, hub preference (Select → `mesh_set_hub`), `sessions_status`, replication log (`Mesh.tsx:61-134`).
- Nodes in this mesh, each with a remove button (`mesh_remove`) (`:136-161`).
- **"Add a node"**: show the pairing string as a QR (`qrcode.react`) with copy, or paste one (→ `mesh_add`) (`:163-202`).
- "On another computer over SSH": the `add_node` path, shown only if the engine answers `list_nodes` (`:207-263`).
- Meshes this app reaches as a device (`:51-59`).

**Other "Add …" flows to model a one-click "Add sandbox" on**
- Settings › Link: "Pair a device" (`device_pair_start` → QR) (`settings/Link.tsx:165-194`).
- Library Tools page: an Add popover that calls `add_mcp_server` or `add_mcp_repo` (`library/Tools.tsx:116-123, 150`).
- Convention: `useAction(fn, {keys, onSuccess})` wraps `useMutation`, invalidates the given query keys and shows a toast on error (`settings/shared.tsx:105-122`).

**Engine switcher**
- Component `components/link/EngineSwitcher.tsx`; store `link/engines.ts:8-72` (zustand).
- `Engine { id, name, relayHost, secure, pairedAt, lastSeen, order, parent?, kind: 'engine'|'mesh', meshName? }`; the local engine is `LOCAL = 'local'`.
- Rows are link-relay pairings (`/h/<id>`). Individual mesh peers are not rows; a `mesh` row reaches whichever engine is the hub.

**Router and store**
- TanStack file routes in `ui/src/routes/*.tsx`.
- Event subscription: `on(type, cb)` (`store/sessions.ts:620`). Events are reduced in `store/reduce.ts`. Reads go through react-query.

**Adding an RPC method**
- `#[rpc(server, client)] pub trait Api` (`rpc/src/api.rs:147-151`).
- Events need a `forward!` entry and a `rt.subscribe` line in `rpc/src/bridge.rs:92-124, 141-171`.
- `export.rs` writes `bindings/*.ts` and `bindings/methods.json` (`rpc/src/export.rs:1-2, 360-369`).
- `just ts` = `cargo test -p rpc --features ts --quiet export_bindings`; `just ts-desktop` copies `bindings/` into `ui/src/rpc/bindings` (`justfile:67-75`).
- The TS client `ui/src/rpc/api.ts` is hand-written: the interface (for example `:214`) plus a method list (`:742`) that `rpc/api.test.ts:7` checks against `methods.json`.
- The event union is `rpc/events.ts:55-96`. The recipe is in `docs/plugins.md` §4 step 2 (`plugins.md:245`).

**How long work streams progress to the UI today** (nothing streams provisioning)
- `ScrapeProgress{session, call_id, inflight, done, limit}` (`web/src/progress.rs:18-54`) is bridged (`bridge.rs:115`) and joined to its tool card by call id (`store/reduce.ts:637`).
- `McpProgress{connecting, connected, failed}` is a status snapshot (`tools/src/mcp.rs:609-624`); the UI invalidates its `['mcp']` query (`lib/rpc.ts:186`).
- Jobs publish `JobEvent`; the shell card shows a tail (`lib/jobTail.ts:1-30`).
- **The one step-log UI:** the daemon launch transcript. Tauri command `launch_log` plus event `launch-log` (`desktop/src/commands.rs:67-72`; `lib/tauri.ts:40-54`) feed a scrolling `<pre>` in `components/launch/StartupScreen.tsx:93-129`.
- Sandbox progress goes only to stdout or tracing (`sandbox/lib.rs:45-74`).

**Directory picker**
- `tauri-plugin-dialog` is registered (`desktop/src/lib.rs:244`; `Cargo.toml:290`) with capabilities `dialog:allow-open` and `dialog:allow-save` (`capabilities/default.json:42-43`).
- It is used as a file picker, `open({multiple, filters})` (`chat/Composer.tsx:1024-1032`), and for `save` (`ArtifactCard.tsx:1`, `notes/ExportMenu.tsx:1`).
- **`directory: true` is used nowhere** (grep → nothing).

## G. Conventions

**1. Design-doc format**
- Title is `# NN — Title`, then a status line with version and date, the docs it extends or amends, and where it wins. Recurring sections:
  - `## 0. Decisions in one screen`: numbered decisions, each naming a "Rejected:" alternative.
  - `## 2. Facts relied on (recon <date>, main @ <sha>)`
  - `## 3. Shape`
  - Contract sections, then Rejected / Alternatives, Open, and Status / As built.
- Examples:
  - 76-talk: status line `76-talk.md:3-10`; §0 at `:22` (Rejected lines at `:28-36`); Facts at `:245`; Shape at `:376`; Rejected at `:832`; Open at `:851`; Status plus "As built 2026-09-28" at `:871-900`.
  - 69: `**v0.4 — BUILT**`, "Supersedes 31 §6" (`69-devices.md:3`); §0 is a Decisions table (# | Decision | Why) (`:5-17`); Shape at `:19`; `## 8. As built` with an invariant/evidence table, accepted deviations and "Found, not fixed" (`:86-111`).
  - 67: status v0.6, "Supersedes 64", `[orch]` marks (`67-agents.md:3`); §0 at `:5`; Facts at `:54`; Shape at `:69`; Open questions at `:497`; Relation to 64 at `:507`; `## 20. As built — stage 1 … deviations` table (Section | As designed | As built | Why) at `:509-538`.
  - 64: "extends 32 (… amends §0.7) … this doc wins; §16 lists the sentences" (`64-bots.md:3-15`); §0 at `:47`; `## 2. Facts relied on (recon 2026-09-23; path:line in this repo)` at `:257`; Shape at `:307`; Alternatives at `:1175`; Amendments at `:1230`; Build steps, each with its check, at `:1292`; Where this doc wins at `:1311`; Open at `:1317`; Status at `:1352`.
  - 74 is the short form, with no §0: "Status: built 2026-09-28. Amends 07 §2 rule 4 …; Code: …" (`74-time.md:3`; sections at `:5-83`).
  - Per-worker as-built docs look like `67-as-built-S.md:1-63`: built per commit, deviations, not built, tests, check results, edits outside the crate.
- **Numbering:** the highest number in use is **78** (`78-aec-spike.md`). 74-time, 75-acp-deep, 76-talk (plus thirteen `76-as-built-*`), 77-talk-followups and 78-aec-spike exist. There is no 79.

**2. Adding a plugin** (`docs/plugins.md`)
- The plugin trait: `pub trait Plugin: Send { fn name(&self) -> &'static str; async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()>; }` (`runtime/src/lib.rs:210-218`).
- Steps:
  1. §1: decide plugin vs tool vs library (`plugins.md:7-21`).
  2. §2: write the doc; nine required items (`:23-37`).
  3. §3: crate anatomy (`:39-47`).
  4. §3.3 events: `serde(tag="type")`, `ts` export, `subscriber_mailbox` (`:175-181`).
  5. §3.4 commands: `FOO_ACTOR`, `kameo::Reply` (`:183-188`).
  6. §3.5 tools: `Risk` is the only permission input; name `foo_verb`; `CONSUMES_SECRETS` (`:190-216`).
  7. §3.5.1 prompt block via `RegisterPrompt` (`:218-230`).
  8. §3.6 config: `rt.section`, `THEUTH_FOO__ENABLED`, `write_config_value`, a `DEFAULT_CONFIG` entry and a README bullet (`:232-238`).
  9. §4 integrate: one line in the `main.rs` plugin vector; the Api method plus `forward!`/subscribe and `export.rs`; the TUI `Command`/`UiEvent`/`COMMANDS`/execute/render; Signal if needed; docs (00 §10 line, README) (`:240-252`).
  10. §5 verify and §6 checklist (`:254-298`).
- There is **no desktop-page step** in plugins.md; the desktop is just "Tauri over the socket" (`:245`).

**3. Config conventions**
- Plugins have `enabled:` (for example `computer/lib.rs:35`, `browser/lib.rs:43`, `nodes/types.rs:16`). **mesh and sandbox have none.**
- Env overrides: `Env::prefixed("THEUTH_").split("__")` (`theuth/src/main.rs:863`). `DEFAULT_CONFIG` opens with `# Env overrides: THEUTH_<SECTION>__<KEY>` (`main.rs:477-478`); README says the same at `README.md:160`.
- README config block (`README.md:157-280`):
  ```yaml
  nodes:                          # design doc 32; `/nodes add <key>` pairs, the node ran `t --install-service`
    enabled: true                 # false: no dials, nodes invisible to the model and the UI; nodes.yaml kept
  ```
  (`README.md:248-249`). **There is no `mesh:` entry in the README** (grep → nothing).
- `DEFAULT_CONFIG` has `nodes:` at `main.rs:590-591` and a commented `mesh:` block at `main.rs:597-608`. That block omits `availability_flush_secs` and `storm_frames_per_sec`.

**4. Tool risk and long work**
- `pub enum Risk { ReadOnly /* Observes only */, Mutating /* Changes state that can be undone or is scoped */, Destructive /* Irreversible or unbounded */ }` (`tools/src/lib.rs:95-107`).
- `Tool` trait constants: `NAME`, `RISK`, `Args`, `RETURNS`, `DIRECT`, `CONSUMES_SECRETS` (`tools/src/lib.rs:299-332`). `tools.policy` maps risk to allow, ask or deny (`plugins.md:211`).
- Long work: the `Jobs` actor makes every process a background job, logging to `jobs/<session>/<id>.log`, publishing `JobEvent`, and delivering `<job_result>` when the activity settles (`tools/src/jobs.rs:1-12`). Knobs: `job_grace_secs: 3`, `job_timeout_secs: 1800` (`README.md:185-186`).
- `ToolCx { session, call_id, quiet, resources }` (`registry.rs:1551-1556`); `call_id` is what joins a `ScrapeProgress` to its card.
- `AgentEvent::ToolCallProgress` is argument streaming, not execution progress (`agent/src/lib.rs:1658-1671`).

**5. Data dir layout**
- README lists (`README.md:157-296`): `config.yaml`, `secrets.vault` (stale; the vault is now `secrets.sqlite`), `providers.yaml`, `models.json`, `oauth/`, `prompt.md`, `mcp/*.json`, `mcp/config.yaml`, `acp/agents.yaml`, `a2a/peers.yaml`, `skills/`, `knowledge/`, `documents/`, `artifacts/`.
- The README **omits** these, which the code creates:

| Path | Constant |
|---|---|
| `mesh/identity`, `mesh.sqlite` | `mesh/lib.rs:52-59` |
| `replica.sqlite` | `replica/src/lib.rs:38` |
| `secrets.sqlite` | `secrets/store.rs:14` |
| `link.sqlite` | `link/src/store.rs:20` |
| `nodes.yaml` | `nodes/store.rs:1` |
| `computers/` | `sandbox/lib.rs:40` |
| `agents/`, `roster.sqlite` | `roster/src/file.rs:12`, `roster/src/lib.rs:45` |
| `tasks.sqlite` | `tasks/src/lib.rs:48` |
| `schedules/`, `schedule.sqlite` | `schedule/src/lib.rs:41-44` |

- 00 §10 is a crate list, not a data-dir list (`00-architecture.md:759-805`). It **omits mesh, replica, sessionsync, roster, tasks and sandbox** (grep → nothing).

**6. Naming, libraries, dependency chain**
- 00 §1.1: "No branding. Crates, modules, types, config keys are named by function. The only product name is the binary `theuth`." (`00-architecture.md:15-16`)
- 00 §1.2: "No reinvented wheels. Off-the-shelf library first; we write glue. A custom implementation needs a written reason in this doc." (`:17-18`)
- 00 §4: "Plugins depend on each other's crates for those types along the `runtime ← tools ← secrets ← knowledge ← agent ← link ← {signal, slack, nodes, schedule, artifact, update, voice} ← rpc ← tui ← theuth`" (`:178-180`). The §10 version adds decision, meeting, browser, codeindex and a2a (`:813-814`).

**7. `deploy/` contents**
- `computer/{Dockerfile, Dockerfile.dev}`
- `relay/{Dockerfile, Dockerfile.iroh-relay, iroh-relay.toml, compose.yaml, README.md, k8s/base/*, k8s/rwx-dev/*}`
- `update/{Dockerfile, Dockerfile.dockerignore}`
- `deploy/computer/` exists and has no display packages.

## H. Prior VM and sandbox thinking

- **No hits in `crates/`** for microsandbox, libkrun, firecracker, microvm, "full linux", vfkit, lima, utm or qemu. The knowledge base has no VM concept either.

**Hits in `docs/`**

| Where | Context |
|---|---|
| `15-server-config.md:188` | "gVisor / Firecracker — VM-class isolation; if that is needed, that is the `docker` executor with a different runtime, not a host option." |
| `39-update.md:558, 772`; `58-cross-platform.md:157, 220` | QEMU for builds only: "never under QEMU". The same rule is in `justfile:104`, `scripts/setup-ubuntu-build.sh:4`, `scripts/stage-linux.sh:4`, `deploy/update/Dockerfile:16` and `deploy/relay/Dockerfile:8`. |
| `32-nodes.md:84` | "a cloud VM" listed as compute to add as a node. |
| `58-cross-platform.md:23` | Termux / the Android AVF Debian VM rejected as the product route. |
| `64-bots.md:41-42` | Not in v1: "cloud provisioning of the VM itself (Hetzner/EC2 backends are further `Sandbox` impls that end in the SSH installer)". |
| `64-bots.md:1182-1184` | A cloud-provider backend first: rejected; "the seam is `impl Sandbox`". |
| `64-bots.md:1302` | Build-step check: "a fresh Ubuntu 24.04 VM with only sshd + curl". |
| `cross-platform/linux-desktop-as-built.md:23` | A VM without GPU passthrough prints a libEGL warning. |

**Design 15 (shelved worker sandbox)**
- Status: "preliminary · **shelved** 2026-09-12 … Nothing here is decided; §9 lists what must be true before any of it starts" (`15-server-config.md:3-6`).
- Scope: multi-user, with a per-user worker run under a host sandbox (Landlock or Seatbelt) or a docker/k8s executor (`15:101-121`).
- Why shelved (`15:226-232`): no named second user who would use it weekly; owner plus auth must come first; it needs a Linux box for the worker. It is also the 00 §13 row at `00-architecture.md:1019`.
- 64 kept it shelved: "this doc provisions *another machine*; it does not sandbox a worker on this one" (`64-bots.md:12-14`).

**64 §4, "Not in v1" and the display route**
- "a display in the Docker computer for `computer_*` tools (`Capabilities.screen`; the image gains Xvfb + a VNC route when someone needs it)" (`64-bots.md:32-36`).
- Designed but unbuilt: `Capabilities.screen: bool // (false in v1 images)` (`64-bots.md:394`); "No display, no browser binary: `browser_*` is pure Rust (23) and works; `computer_*` reports `screen: false`" (`:474-475`).
- Also unbuilt: docker `cpus`/`memory_mb` config (`64-bots.md:606-627`), the `computer_*` RPC (`:572-577`) and the `computers` / `computer_create` tools (`:587-599`).

**Grok Bot recon** (`scratch-always-on-recon-grok.md`)
- The box connector returns `vncUrl` and `forkVncBaseUrl`; `GatewayConnection.vncProxy{primaryUrl, forkBaseUrl, networkToken}` (`:106-121, 128`).
- The local docker computer publishes loopback ports including 6080 and 6081 (`:138`). Each Bot gets its own screen/window lease (`:31, 184-186`).
- Lessons (§H): a narrow `SandboxAdapter` that resolves `{gateway, auth, display endpoints, capabilities}` (`:192`); negotiate shell/files/browser/screen capabilities up front (`:193`); per-agent display leases (`:199`).

## Gaps: what the VM design has to add

1. **VM backend:** no dependency or code for microsandbox, libkrun or any other VM runtime.
2. **Trait lifecycle:** `Sandbox` has no `start`, `stop`, `status`, `recreate`, `snapshot` or `restore`.
3. **Capabilities:** `Capabilities` has no display/screen, VNC endpoint, resource or mount fields (`sandbox/lib.rs:160-171`).
4. **Resources:** no CPU, memory or disk settings anywhere. Docker `create` sets none (`docker.rs:285-305`) and there is no `sandbox:` config section.
5. **Host directory sharing:** docker binds only two named volumes (`docker.rs:293-297`); there is no mount API.
6. **Control surface:** the `SandboxActor` exists (`MakeEngine`, `Engines`), but there is no RPC method, no tool and no desktop "Add sandbox". The only entry is the CLI (`main.rs:423-475`).
7. **Progress:** `Progress` is a stdout/log closure. No `SandboxEvent` exists in the bridge, and bring-up runs for minutes inside the actor handler, one engine at a time (`sandbox/lib.rs:292-293`).
8. **Destroy and records:** no lifecycle UI. `t mesh remove` does not destroy the compute. The `computers/<name>.yaml` record has no state, resources or display info.
9. **Advertisement:** there is no `Vm` class, no os/arch/hostname/platform field and no config override. `display` is inferred from `DISPLAY` in `t`'s environment, and `Server`/`Container` rank highest for the hub lease (`lease.rs:93-99`). Unless it sets `hub: never`, a VM engine can win the hub lease.
10. **Remote display:** no VNC/RFB client, no noVNC, no websockify. The `computer_*` tools drive only the local display (xcap/enigo).
11. **Embedding a VNC page:** the desktop CSP `frame-src` excludes http origins (`tauri.conf.json:48`), there is no multi-webview, and `open_window` takes app routes only (`commands.rs:17-48`).
12. **Folder picker:** the dialog plugin and `dialog:allow-open` are present, but `directory: true` has never been used.
13. **Secret scope and prompts:**
    - A joining engine gets the entire vault through `vault_key`; there is no per-engine allowlist.
    - `SecretRequested` is not relayed across engines.
14. **Live mesh UI:** `MeshEvent` is not on the RPC bridge.
15. **Name clash:** mesh peers and design-32 nodes share one `Agents.nodes` namespace, and a later registration replaces an earlier one (`supervisor.rs:6632-6637`).
16. **Image:** the `deploy/computer` image has no desktop, X server, VNC, Node or `uv`.
17. **Windows:** targets are refused at bring-up (`bringup.rs:87-91`).
18. **Doc drift to fix:**
    - README has no `mesh:` config block; it omits the mesh and sandbox data files and still names `secrets.vault`.
    - 00 §10 omits the mesh, replica, sessionsync, roster, tasks and sandbox crates.
    - The `main.rs:141-142` "not built" comment is stale.
    - `t --install-service` still prints the Settings › Nodes hint.