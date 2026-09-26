# 67 — as built, worker M (`crates/mesh`)

*Branch `feature/67-agents-mesh`. Names from `67-contract.md` §3 unchanged; additions listed under Deviations.*

## What was built, per commit

### 1 — `feat(mesh): iroh endpoint, identity, mutual pairing`
- `identity.rs`: `Identity::load_or_create(<data_dir>/mesh)` — 32-byte iroh `SecretKey` at `mesh/identity`, mode 0600 via `platform::files::create_private`; `EngineId` = public key hex (`PublicKey::to_string`); `first_seen_ms` = identity file ctime (the probation clock).
- `net.rs::Net::bind`: `Endpoint::builder(presets::Minimal)` with ALPNs `replica::{ALPN_RECORDS, ALPN_ACP, ALPN_SESSIONS, ALPN_KNOWLEDGE, ALPN_GOSSIP}` + `mesh::ALPN_MESH`; relay from `mesh.relay` (`public` → `RelayMode::Default` = n0 presets; empty/`none`/`off` → disabled; else custom URL). Crypto provider is aws-lc-rs only (`iroh` feature `tls-aws-lc-rs`, no `tls-ring`) to match `theuth::main`'s installed provider.
- `pairing.rs`: pairing string `1.<base32-nopad(json {id,name,relay,addrs,token})>` (31's `version.payload` convention; base32 so it is typeable, case-insensitive on parse); `Pending` one-time token, 5 min TTL, consumed on first use. `PeerEntry` + `merge` (union, LWW on `max(added_ms, removed_ms)`, removal wins a tie).
- Pairing protocol (`ALPN_MESH`, `u32 LE len ‖ JSON`): dialer `PairHello{token, me, peers}` → inviter checks token, replies `PairAck{token, me, peers}`; both write the other into `peers` and merge the other's full set (transitive). Inviter refuses with `PairReject`. `RemovePeer` writes a tombstone (`peers.removed_ms`, migration `0002`), closes connections, pushes the set to connected peers.
- Name = `platform::process::hostname()` unless `mesh.name`.
- Tests: `pairing::tests::{pairing_string_round_trip, merge_is_union_and_removal_wins_when_newer}`, `identity::tests::create_then_load_is_stable`.

### 2 — `feat(mesh): discovery, connectivity, replica transport`
- Discovery: `PkarrPublisher/PkarrResolver/DnsAddressLookup::n0_dns()` (DNS/pkarr) + `iroh_mdns_address_lookup::MdnsAddressLookup` added after bind (LAN direct); relay fallback per `mesh.relay`. Dial hints: addresses remembered in `peers.addrs_json`.
- Connected set: one control connection per paired peer on `ALPN_MESH` (`run_connector` → per-peer `connect_loop`; the lower id dials, the higher waits 3 s then dials; backoff 1 s → 60 s ×2 with jitter, reset on success). Presence = live control connections; `PeerEvent::Connected/Disconnected` on 0↔1; the lease loop wraps them as `MeshEvent::Peer`.
- `impl replica::Transport for Net`: `peers()`, `open(peer, alpn)` (cached QUIC connection per (peer, alpn), `open_bi` → `tokio::io::join(recv, send)` boxed as `ByteStream`), `subscribe()`, and the new `accept(alpn)` (inbound bi streams queued per ALPN by the `Inbound` handler; paired peers only). Registered with `replica::RegisterTransport` via `lookup_when::<replica::Replica>`.
- Test `net::tests::pair_connect_and_round_trip_records` (two endpoints on `127.0.0.1:0`, relay disabled, no discovery): pair, `Connected`, `open(ALPN_RECORDS)` ↔ `accept(ALPN_RECORDS)` echo, token single-use, remove → tombstone. Not `#[ignore]`d — loopback only.

### 3 — `feat(mesh): engine stats, peer-observed availability, hub lease`
- `stats.rs::probe`: class (container: `/.dockerenv`, `/run/.containerenv`, `/proc/1/cgroup`; laptop: battery via `pmset -g batt` / `/sys/class/power_supply/BAT*`; server: no display and no battery; else desktop), `harnesses` = `native` + keys of `<data_dir>/acp/agents.yaml`, `repos` = git roots under `mesh.workspaces` (two levels), `display`, `gpu` (Apple Silicon → true; Linux DRM/NVIDIA nodes; else false). Refreshed every `capabilities_secs` (min 30) into `engine_stats`.
- `replica::attach(pool, &["peers","engine_stats","availability"])` at start; `Err` → "local only", continue.
- Availability ledger (`store.rs`): on Connected/Disconnected `availability_open(observer=self, subject, reachable)`; the open interval's `to_ms` is extended every heartbeat tick (`availability_touch`) so a crash loses ≤ one tick. `availability_30d(subject)` = reachable ÷ observed over 30 days across all observers' rows; `lease::bucket` → 3/2/1/0 for ≥ 99.9 / 99 / 95 / less.
- `lease.rs::elect`: pure; filters `Never`; sort key (Prefer, bucket desc, class rank Server=Container > Desktop > Laptop, uptime desc, id asc); probation (`first_seen < probation_secs` ago cannot outrank the incumbent); handover grace (`connected_since ≥ handover_secs`); both only while the incumbent is still a candidate. Tests: server beats laptop regardless of id, Prefer beats everything, probation blocks newcomer, grace delays handover, lone laptop is hub (+ buckets).
- `lib.rs::run_lease`: `iroh-gossip` on `ALPN_GOSSIP` (`Gossip::builder().alpn(ALPN_GOSSIP)`), topic `sha256("theuth/mesh/lease/1")`, bootstrap = paired ids, `join_peers` on peer-set changes. Heartbeat every `heartbeat_secs`: `{id, is_hub, stats_hlc, stats, hub_pref, first_seen_ms}`. Candidates = self + connected peers whose last heartbeat is within `lapse_secs`; recompute on tick, heartbeat, disconnect, release, pair/remove/hub-pref change. Change → `lease_log` row + `MeshEvent::Lease` (Subscribe channel and `rt.publish`). `LeaseRelease` broadcast when the loop ends (shutdown token) while holding; SIGTERM (`platform::process::stop_signal`) cancels the shutdown token so the release goes out before exit.

### 4 — `feat(mesh): status RPC, CLI, relay deployment`
- Handlers: `Status` (peers from the store with live reachability, lease), `PairingString` (waits ≤ 3 s for the home relay so the string carries a relay address), `AddPeer` (DelegatedReply, off the mailbox; pushes the merged set, recomputes), `RemovePeer`, `SetHubPref` (`runtime::write_config_value(config.yaml, "mesh", "hub", …)` + in-memory + advertised), `Subscribe`, new `ListStats`. `mesh_peers` tool body: one line per engine (self first) with class/harnesses/repos/display/gpu and `hub` marker; `reachable: true` filter.
- `deploy/relay`: `iroh-relay.toml`, `Dockerfile.iroh-relay` (`cargo install iroh-relay --features server`, pinned 1.2.0), `compose.yaml` service, `k8s/base/iroh-relay-{deployment,service,service-quic}.yaml` (Deployment, ClusterIP, UDP LoadBalancer for 7842 — one object per file like the rest of the manifests), `k8s/rwx-dev/ingress-iroh.yaml` (SNI passthrough to 8443), `ingress.yaml` SAN `iroh.relay.theuth.io`, kustomizations, `README.md` with section "iroh relay" (ports, why UDP forces a subdomain, certificate handling).

## Deviations from the brief / contract (all additive)
1. **`mesh::ALPN_MESH = "theuth/mesh/1"`** carries the pairing handshake and presence instead of `ALPN_GOSSIP`: iroh-gossip's protocol owns its ALPN end to end (the router routes one handler per ALPN), so pairing streams could not share it.
2. **`replica::Transport::accept(&self, alpn) -> anyhow::Result<(PeerId, Box<dyn ByteStream>)>`** added (one method, `crates/replica/src/types.rs`). "Next inbound stream; call in a loop" rather than a `Stream` type, to keep `replica` free of a `futures` dependency.
3. **Heartbeat carries `stats`, `hub_pref`, `first_seen_ms`** beyond `{id, is_hub, stats_hlc}`: until `engine_stats` replicates, the election needs class/uptime/preference of peers; receivers cache them into `engine_stats` (`hlc` column = sender's `stats_hlc`).
4. **Availability with no observations = 1.0** (top bucket): a never-observed engine is not penalised; probation covers the newcomer case. Otherwise, pre-replication, every engine would rank every peer above itself (it has observations of others, none of itself) and elections would disagree.
5. **Probation clock = the engine's own identity-file creation time**, advertised in heartbeats — the same value on every observer, so elections agree; a restart is not a "new join".
6. **New message `mesh::ListStats`** → `Result<Vec<EngineStats>, String>` (for `mesh_peers`); **`MeshConfig.name`** added (display name, empty = hostname; documented in the brief as `mesh.name`).
7. **`peers.removed_ms`** (migration `0002_mesh_peer_removed.sql`): removal is a tombstone so it replicates and outranks a stale re-add; `Status`/`mesh_peers` filter tombstones.
8. **Workspace deps** beyond `iroh`/`iroh-gossip`: `iroh-mdns-address-lookup` (mDNS moved out of iroh in 1.0) and `data-encoding` (base32; already in the graph under iroh). `crates/mesh/Cargo.toml` also pulls existing workspace crates `platform`, `tokio-util`, `futures`, `serde_yaml`, `bytes`, `rand`, `sha2`; dev-dep `tempfile`.
9. **SIGTERM** is now observed by the process (previously default-killed): the mesh's handler cancels `rt.shutdown`, so `main.rs` exits through its normal path after the lease release. `crates/platform` has no sleep/`willSleep`/battery hook; the battery probe lives in `stats.rs`, sleep release is SIGTERM-only.
10. **Connection direction rule** (lower id dials, higher waits 3 s) is mesh-local policy, not in the design.
11. **Deploy**: the repo has no wildcard certificate — cert-manager issues a SAN list; `iroh.relay.theuth.io` is added to it. Because the QUIC probe needs the key in-process, the ingress passes 443 through by SNI (`haproxy.org/ssl-passthrough`) and UDP 7842 gets a LoadBalancer Service. Compose: Caddy keeps 443, iroh-relay publishes 8443/7842-udp and reads `./certs`. `iroh-relay` is installed from crates.io in the Dockerfile, not vendored.
12. `Peer.last_seen_ms`: now while reachable; else the last connection start or `added_ms`.
13. Election candidates are limited to peers that have **heartbeated within `lapse_secs`**; a freshly connected peer joins the running after its first heartbeat (≤ `heartbeat_secs`).

## Not built
- `t mesh add --ssh|--docker` (sandbox worker), macOS `willSleep` release (no platform hook), self-reported vs replicated availability reconciliation (replica worker), bridging `MeshEvent` into `crates/rpc/src/bridge.rs` for the desktop (UI worker; events are on the bus and `Subscribe`).

## Tests written (not run, per the brief)
`identity::tests::create_then_load_is_stable`; `pairing::tests::{pairing_string_round_trip, merge_is_union_and_removal_wins_when_newer}`; `lease::tests::{server_beats_laptop_regardless_of_id, prefer_beats_everything, probation_blocks_a_newcomer, grace_delays_handover, lone_laptop_is_hub, buckets}`; `stats::tests::{repos_finds_git_roots_two_levels_down, harnesses_always_has_native}`; `net::tests::pair_connect_and_round_trip_records` (loopback, two endpoints).

## `cargo check -p mesh --tests`
Run once per commit (`--tests`, so the unit and loopback tests compile too). Last run, after commit 3's changes:

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5m 16s
```

No errors, no warnings. Commit 1's check surfaced 13 errors (reserved `gen` identifier, `AcceptError::from_err` needing `std::error::Error`, `Eq` on an `f32` field, kameo `ask` flattening `Result` replies) — all fixed before the commit; commits 2 and 3 checked clean.

Also touched outside `crates/mesh` (disclosed): `crates/replica/src/types.rs` (+`Transport::accept`), `crates/theuth/src/main.rs` (+`name` line in the `mesh:` template), workspace `Cargo.toml` (four dependency lines), `Cargo.lock`.

DONE M

## Fix round M2 — mesh tables attach to replication

Z found (`67-as-built-Z.md` §3.4) that `replica::attach(pool, &["peers","engine_stats","availability"])` could never succeed: `peers` had no `hlc TEXT`, `availability` had a composite primary key, and `engine_stats` had two writers (gossip heartbeats *and* the replicated row).

- **Migration `0003_replicable.sql`**: `peers.hlc TEXT`; `availability` rebuilt with `id TEXT PRIMARY KEY` = `observer|subject|from_ms` (the three columns kept, two indexes), `hlc TEXT`; `engine_stats` gains `hub_pref` and `first_seen_ms` (see below). Existing rows are copied.
- **HLC stamps**: `Store::hlc(now_ms)` — the process clock the `tasks` store uses, but emitted in `replica::Hlc::to_text` form with this engine's id as origin, so the replica *observes* our clock instead of re-stamping (its capture falls back to its own tick on an unparseable value). Every `peers`/`engine_stats`/`availability` write stamps it (`put_peer`, `put_stats`, `availability_open`, `availability_touch`). `Store::open(path, me)` now takes the engine id.
- **Self row in `peers`**: once `peers` replicates, what a peer wrote about *us* arrives as a row with our own id. `peer_entries()` filters `id != me`, so `Status`, `paired_ids`, the availability loop and `push_peers` never treat this engine as its own peer.
- **`engine_stats` single path**: the replicated row is the whole advertisement — `class`, `uptime_since_ms`, harnesses/repos/display/gpu, and now `hub_pref` and `first_seen_ms` (which rode on the heartbeat). Only this engine writes its row (`put_stats` returns the `hlc` written); remote rows arrive by replication only — `on_heartbeat` no longer writes anything. `SetHubPref` rewrites the row. Z's "write only on change" is kept (`refresh_stats` compares the probe to `self_stats` first).
- **Heartbeat = `{ id, is_hub, stats_hlc }`** (67 §4.5), `stats_hlc` the text `hlc` of the sender's last `engine_stats` write. `Seen` keeps `{ last_hb_ms, stats_hlc }`. Election candidates are built by `Store::candidate(id, connected_since, now)` from the replicated row plus our ledger; a connected, heartbeating peer whose row has not arrived yet (its `stats_hlc` is newer than anything we hold) is simply **not a candidate this round** — nothing is requested, the change feed delivers it and the next recompute picks it up (one `debug!` line names the wait). The `EngineStats` contract type is unchanged.
- **Test** `store::tests::tables_attach_to_replication`: a `Store` on a temp file, `replica::engine::Engine::attach` with the three tables → `Ok`; writes to `availability` and `engine_stats` appear on the change feed with origin `me`; `candidate()` reads `hub_pref`/`first_seen_ms`/`class` back and is `None` for an unknown id.

`cargo test -p mesh -p replica`: mesh 13 passed, replica 10 + 4 + 2 passed, 0 failed, no warnings.
