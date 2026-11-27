# 228 — Private relay bridge

Status: proposal, not built (2026-11-27). Records an owner-confirmed topology; not a build contract until the owner accepts it.

## 0. The picture

One owner mesh spans a private network and the public internet through a single dual-homed engine (the **bridge**). Two separate `iroh-relay` servers exist — one public (`https://iroh.relay.theuth.io`), one private inside the private network (distinct hostname, e.g. `https://iroh.lan.internal`). The two relays **do not peer** and **must never share a hostname** (`deploy/do/README.md:187-193`).

Participants and what each joins:

| Participant | `mesh.relay` (iroh) | `mesh.private_relay` (iroh) | `link.relay` (WebSocket) | Role |
|---|---|---|---|---|
| **LAN-only engine** | Private `iroh-relay` URL | empty | none (never hub) | Runs sessions on the private network; `mesh.discovery: false` |
| **Bridge engine** | Public `iroh-relay` URL | Private `iroh-relay` URL | Public `theuth-relay` (when hub) | Dual-homed on both iroh relays; relays data and one-hop interactive traffic between sides |
| **Cloud / public engine** | Public `iroh-relay` URL | empty | Public `theuth-relay` (when hub) | Public-only engine; competes with the bridge for the single public hub lease |
| **iPad (or phone / web)** | none (not an iroh peer) | none | Public `theuth-relay` (`wss://relay.theuth.io`) | Talks JSON-RPC to whichever public-homed engine holds `/h/<mesh_device_id>` |

```text
  [iPad] ──(WSS link.relay: wss://relay.theuth.io)──► [Public Hub: Cloud or Bridge]
                                                               │
                 ┌──────────── Public iroh-relay ──────────────┤ (QUIC mesh)
                 │             (iroh.relay.theuth.io)          │
                 │                                             ▼
            [Cloud engine] ◄──────────────────────────► [Bridge engine]
                                                               │
                                   Private iroh-relay ─────────┤ (QUIC mesh, distinct host)
                                   (iroh.lan.internal)         │
                                                               ▼
                                                        [LAN-only engine]
```

What the iPad sees while the bridge is up:

- Sessions owned by the **LAN-only engine**, the **bridge engine**, and the **cloud engine** all appear in the sidebar.
- The owner can open and actively use any of them — send a message, watch live streaming output and approval/ask prompts, fetch artifacts/attachments, open a PTY terminal, or open a remote desktop tunnel — and can start "New chat on" the LAN-only engine.
- If the bridge drops, the private side becomes an island: public clients lose live access to LAN-only sessions (the last replicated transcript may remain readable offline), LAN-only keeps working locally, and the public mesh does not split.

## 1. What already works

Several pieces of the bridge path are already built and verified in the codebase:

- **Transitive session and record replication.** `sessionsync` already replicates every origin it holds across every connected peer without checking whether the remote peer can dial the origin directly. `Node::held_here` offers the engine's own log head plus every applied foreign cursor (`crates/sessionsync/src/sync.rs:264-279`), `catch_up` snapshots relayed origins (`crates/sessionsync/src/sync.rs:1191-1212`), and the live `relay.recv()` loop ships foreign `Msg::Log` rows (or bootstraps with `Msg::Snapshot`) to every peer except the origin itself (`crates/sessionsync/src/sync.rs:931-964`). `list_sessions` reads local `sessions.sqlite` with no reachability filter (`crates/agent/src/rpc_sessions.rs:130-132`, `crates/agent/src/store.rs:3478-3499`).
- **Non-home relay dialing.** `Net::dial_alpn` already dials a peer's `EndpointAddr` using a relay URL outside the local `RelayMap` as a non-home relay (`crates/mesh/src/net.rs:818-850`).
- **Multi-entry relay insertion loop.** `Net::bind` builds the iroh `Endpoint` with `RelayMode::Custom(RelayMap::empty())` (`crates/mesh/src/net.rs:549-563`) and `Net::insert_relays` iterates every entry in `self.relays.relays::<Vec<_>>()` calling `self.endpoint.insert_relay(...)` (`crates/mesh/src/net.rs:809-816`).
- **iPad path through the hub on the link relay.** `theuth-relay` (`crates/relay/src/relay.rs:1-6, 100-113`) is a stateless WebSocket frame relay holding an in-memory uplink table, not a roster. The iPad connects to `link.relay` (default `wss://relay.theuth.io`, `crates/link/src/types.rs:15-38`) and reaches whichever engine currently holds the mesh lease and registers `/h/<mesh_device_id>?role=harness` (`crates/link/src/follow.rs:1-15, 53-100`, `crates/link/src/mesh_uplink.rs:1-14, 57-93`). `iroh-gossip` carries only `Heartbeat` and `LeaseRelease`, not addresses or sessions (`crates/mesh/src/lib.rs:624-633`).

## 2. The gap

Joining bridge, LAN-only, and cloud into one mesh today gets LAN-only session rows replicated onto a public hub, but breaks in four places:

1. **Single home relay in config, binding, and status.**
   - `MeshConfig.relay` is a single `String` defaulting to `https://iroh.relay.theuth.io` (`crates/mesh/src/config.rs:11-14, 62`); `Net::bind` parses either `RELAY_PUBLIC` or one `RelayUrl` (`crates/mesh/src/net.rs:542-548`).
   - `Net::relay_state` reads only `.first()` from `self.relays` and `.first()` from `self.endpoint.home_relay_status().get()` (`crates/mesh/src/net.rs:1102-1120`), and `MeshStatus.relay` is a single `Option<RelayState>` (`crates/mesh/src/types.rs:135-144, 351-353`).
   - `Net::a2a_reach` overwrites a single `relay = Some(u.to_string())` (`crates/mesh/src/net.rs:1017-1028`). `Payload.relay` (`crates/mesh/src/pairing.rs:64`) and `InviteTicket.relay` (`crates/a2a/src/contacts.rs:354`) are single strings.
   - `iroh` is pinned at `1.4.1` (`Cargo.toml:64`, `Cargo.lock:10831-10835`). Whether one `Endpoint` keeps two home relays connected concurrently is unverified at runtime — and reading iroh 1.4.1's `RelayActor` indicates single-home-relay behavior (see §7).

2. **Reachability and every interactive ALPN are strictly one direct QUIC hop.**
   - `Net::peer_of` and `Transport::peers` read only direct `presence` connections (`crates/mesh/src/net.rs:1066-1072, 1557-1582, 2107-2113`). A public hub that is not the bridge marks the LAN-only engine `reachable: false, path: PeerPath::Offline`.
   - `rpc_forward` checks `t.peers().contains(&owner)` in `classify` and `to_peer_via`, returning `OWNER_UNREACHABLE` (`-32010`) immediately if the owner is not directly connected, and refuses any second hop whenever `Forwarded` is in the request extensions (`crates/agent/src/rpc_forward.rs:95, 176-177, 263-283, 381-399, 486-508, 572-616`). `engine_call` in `rpc_machines` rides `to_peer_via` (`crates/agent/src/rpc_machines.rs:1-8`).
   - `EventRelay` (`theuth/sessions/1`) dials `t.open(owner, ALPN_SESSIONS)` directly (`crates/sessionsync/src/events.rs:821-913`), and `EventRelay`'s `Serve` handler closes any stream for a session not owned locally (`crates/sessionsync/src/events.rs:480-524`, `crates/sessionsync/src/lib.rs:198-225`).
   - Artifact fetch (`theuth/artifacts/1`) queries only directly connected `t.peers()` (`crates/artifact/src/mesh.rs:302-350`) and `answer` serves only local files (`crates/artifact/src/mesh.rs:170-207`).
   - PTY streaming (`theuth/pty/1`) checks `t.peers().contains(peer)` and attaches only local jobs (`crates/agent/src/rpc_engine.rs:516-595`).
   - Desktop tunneling (`theuth/desktop/1`) opens `t.open(&to, ALPN_DESKTOP)` directly and `serve_desktop` pipes only to local `127.0.0.1:<port>` websockify (`crates/agent/src/rpc_engine.rs:645-775`).
   - The desktop/iPad UI treats `!p.reachable` as offline in Settings › Network (`crates/desktop/ui/src/components/settings/Network.tsx:158-168`), "New chat on" (`crates/desktop/ui/src/components/chat/NewChatPane.tsx:48-85, 485-509`), Settings by machine (`crates/desktop/ui/src/lib/mesh-machines.ts:118-135`), and the mesh node counter (`crates/desktop/ui/src/link/engines.ts:157-159`).

3. **Hub election can split across the bridge.**
   - `Ctx::recompute` builds election candidates from `self` plus `self.net.connected()` (`crates/mesh/src/lib.rs:783-833`), and `lease::elect` filters only `c.hub_pref != HubPref::Never` (`crates/mesh/src/lease.rs:130-159`).
   - If a public cloud node outranks the bridge and wins the hub lease, the LAN-only engine (which cannot connect to the cloud node) sees only `{lan_only, bridge}` with no cloud candidate; when the bridge is down, the LAN-only engine sees `{lan_only}` and elects itself hub, running hub duties on an island or attempting to claim `/h/<mesh_device_id>` if it has outbound reachability.

4. **Address and pkarr leak.**
   - `MeshConfig.discovery` defaults to `true` (`crates/mesh/src/config.rs:17, 63`), registering `PkarrPublisher::n0_dns`, `PkarrResolver::n0_dns`, and `DnsAddressLookup::n0_dns` on `join_network` (`crates/mesh/src/net.rs:772-799`), which publishes every `EndpointAddr` (direct IPs + relay URL) to public n0 DNS.
   - `Net::me()` puts all `self.endpoint.addr().addrs` into `PeerEntry.addrs` (`crates/mesh/src/net.rs:1149-1158`), `pairing::merge` unconditionally unions `addrs` (`crates/mesh/src/pairing.rs:216-244`), and both `Msg::Peers` (`crates/mesh/src/net.rs:1535-1554, 1596-1601`) and `replica::attach` on the `peers` table (`crates/mesh/src/lib.rs:1610-1613`) replicate every private relay URL and LAN IP to every public engine.
   - `Net::pairing_string`, `Net::remote_ticket`, and `Net::a2a_reach` copy raw `self.endpoint.addr().addrs` without filtering out a private relay URL or private IPs (`crates/mesh/src/net.rs:1017-1028, 1177-1227`), whereas A2A `ticket_target` already rejects private/link-local/loopback relays (`crates/a2a/src/contacts/ceremony.rs:369-400`).

## 3. Design

### 3.1 One mesh, not two

The private network is an extension of the existing owner mesh, not a second mesh. Two separate meshes cannot replicate sessions across a bridge today because `replica`, `sessionsync`, and the mesh keyring are scoped to one paired peer set and one shared vault key. The bridge keeps **one** Ed25519 identity (`EngineId`), **one** `mesh.sqlite`, **one** `sessions.sqlite`, and the existing shared vault.

### 3.2 Dual-home config and endpoint binding

Extend `MeshConfig` (`crates/mesh/src/config.rs:11-57`) with one optional string field (default empty, back-compatible with every existing config where a lone `mesh.relay` string means one relay):

```yaml
mesh:
  relay: https://iroh.relay.theuth.io
  private_relay: ""   # optional second home; LAN-only sets relay to the private URL and leaves this empty
```

Configuration by role:

- **Bridge:** `mesh.relay` = public iroh relay URL; `mesh.private_relay` = private iroh relay URL (distinct hostname).
- **LAN-only engine:** `mesh.relay` = private iroh relay URL; `mesh.private_relay` = `""`; `mesh.discovery: false`; `mesh.hub: never` (also enforced automatically, §3.4).
- **Cloud / public-only engine:** `mesh.relay` = public iroh relay URL; `mesh.private_relay` = `""`.

Validation and binding in `BindOpts` / `Net::bind` (`crates/mesh/src/net.rs:214-233, 533-568`):

- Reject startup config if `private_relay` is non-empty and either (a) `relay` is empty/`none`/`off` (a single-relay private engine must put the private URL in `mesh.relay`), or (b) `private_relay` parses to the same `RelayUrl` or hostname as `relay`.
- Store `public_relay: Option<RelayUrl>` and `private_relay: Option<RelayUrl>` separately on `Net` alongside `relays: RelayMap` so advertisement, status, and filtering never guess which URL is private.
- Primary design: insert both URLs into the single `Endpoint` via `Net::insert_relays` (`crates/mesh/src/net.rs:809-816`).
- Contingent fallback (triggered if the §7 probe fails): keep a second `iroh::Endpoint` inside the same `Net` struct using the **same** `SecretKey` (same `EndpointId`), bound only to `private_relay` with `discovery: false`, serving `ALPN_MESH`, `ALPN_GOSSIP`, and `STREAM_ALPNS` (never `ALPN_A2A`). See §7 for full fallback wiring.
- Status reporting: keep `MeshStatus.relay: Option<RelayState>` for the primary `mesh.relay` (back-compat), and add an optional `MeshStatus.private_relay: Option<RelayState>` (`crates/mesh/src/types.rs:331-369`) populated by looking up `self.private_relay` in `home_relay_status().get()` (or on the private endpoint under the fallback) rather than taking `.first()`.

### 3.3 Advertisement, tickets, pkarr, and `peers` privacy

Private relay URLs and private LAN sockets must never leave the private network:

1. **Pairing tickets and A2A invites publish the public relay only.**
   - `Net::pairing_string` and `Net::remote_ticket` (`crates/mesh/src/net.rs:1177-1227`) always emit `Payload.relay = self.relay` (public) and filter `Payload.addrs` to drop any `TransportAddr::Relay` matching `self.private_relay` and any private/link-local/loopback `TransportAddr::Ip`.
   - Pairing a LAN-only engine to the bridge uses either: (a) `t mesh pair` on the LAN-only node (which is on the private network and emits its private relay/LAN address) followed by `t mesh add` on the bridge, or (b) Nearby mDNS pairing on the LAN, or (c) an explicit local-only `t mesh pair --private` flag on the bridge that is never used for cloud/remote tickets.
   - `Net::a2a_reach` (`crates/mesh/src/net.rs:1017-1028`) returns only the public relay URL (skipping `self.private_relay`) and drops private/link-local/loopback IPs, so A2A `InviteTicket`s built on the bridge always pass `ticket_target` (`crates/a2a/src/contacts/ceremony.rs:369-400`).

2. **pkarr / n0 DNS never receives private addresses.**
   - LAN-only engines run with `mesh.discovery: false`, preventing `Net::join_network` (`crates/mesh/src/net.rs:777-799`) from attaching `PkarrPublisher::n0_dns`, `PkarrResolver::n0_dns`, or `DnsAddressLookup::n0_dns`.
   - On the bridge, pkarr publication must exclude `self.private_relay` and private-only addresses: under the single-endpoint path, wrap/filter the published `EndpointInfo` before `PkarrPublisher` publishes it; under the §7 second-endpoint fallback, the private `Endpoint` has no `AddressLookup` / pkarr publisher attached at all.

3. **What is stored in `peers` instead of raw private addresses.**
   - Extend `PeerEntry` (`crates/mesh/src/pairing.rs:168-184`) and the replicated `peers` table with an additive flag `#[serde(default)] pub private_only: bool` (older engines default it to `false`).
   - Add a **local-only** table `peer_private_addrs (id TEXT PRIMARY KEY, addrs_json TEXT NOT NULL)` in `crates/mesh/src/store.rs`, created with `CREATE TABLE IF NOT EXISTS` (following `peer_docs_seen`, `crates/mesh/src/store.rs:64-78`) and **not** listed in `replica::attach` (`crates/mesh/src/lib.rs:1610-1613`).
   - When the bridge learns a `private_only` peer's `TransportAddr::Relay(<private_relay>)` or RFC1918/ULA/link-local `TransportAddr::Ip` addresses (during pairing or over the private-side `ALPN_MESH` control connection), it persists those addresses only in local `peer_private_addrs`.
   - Before writing any `private_only` row to the replicated `peers` table (`Store::put_peer`) or sending `Msg::Peers` (`Net::push_peers`, `drive`, `PairHello`, `PairAck`) to a public peer, the bridge strips `self.private_relay` and private/link-local/loopback IPs from `PeerEntry.addrs`. On public engines, a LAN-only peer's replicated `peers` row therefore carries `id`, `name`, `hub_pref: Never`, `private_only: true`, and `addrs: []`.
   - `Net::addr_for` (`crates/mesh/src/net.rs:1161-1172`) reads `peer_private_addrs` first on the bridge (and on any private-only engine) when dialing private-side peers. On public-only engines, `Net::run_connector` (`crates/mesh/src/net.rs:1940-2036`) skips dialing any `private_only` peer whose `addrs` list is empty instead of spinning in a backoff loop.
   - **Live bridge reachability advertisement (`Msg::BridgePeers`).** Append a new variant to `Msg` on `ALPN_MESH` (`crates/mesh/src/net.rs:132-211`; older engines log and ignore unknown variants per `net.rs:129-130`):

     ```rust
     BridgePeers { peers: Vec<EngineId> }
     ```

     The bridge sends `Msg::BridgePeers` containing the paired `private_only` engines it currently has a **direct** QUIC connection to (`self.presence`), on every public control connect (beside `KeyEpoch`/`DocsSchema`) and whenever that set changes. Each receiving public engine keeps an in-memory map `via: Mutex<HashMap<EndpointId, BTreeSet<EndpointId>>>` (target → connected bridges advertising it), never written to SQLite. When a bridge's control connection drops (`presence_drop`), all entries contributed by that bridge are purged immediately and a `PeerEvent` / `MeshEvent::Peer` notification fires.

### 3.4 Hub election ignores private-only engines

A private-only engine is never eligible for the hub lease and never claims `/h/<mesh_device_id>` on `link.relay`:

- **Private-only identification.** An engine is `private_only` when it has no public relay — specifically when `mesh.private_relay` is empty and `mesh.relay` is a non-public private relay (configured explicitly, e.g. `mesh.hub: never` plus `mesh.discovery: false`, or an explicit `mesh.private_only: true` / derived whenever `mesh.relay` is neither `public` nor the public relay and `mesh.hub` is clamped). To avoid heuristic DNS guessing on arbitrary custom public relays, define `private_only` cleanly from config: `mesh.hub: never` is required for LAN-only engines, and add a `public_homed: bool` field to `lease::Candidate` (`crates/mesh/src/lease.rs:61-77`) and `engine_stats` (`crates/mesh/src/store.rs:346-412`, default `true` for pre-228 rows).
- **Concrete rule on a LAN-only engine.** When an engine is configured as LAN-only (`mesh.discovery: false` with a private `mesh.relay`, or explicit `mesh.private_only: true`):
  - Effective `hub_pref` is forced to `HubPref::Never` in `Ctx::recompute` and `write_stats` (`crates/mesh/src/lib.rs:764-808`); `SetHubPref` rejects `Eligible`/`Prefer` with an error.
  - `public_homed` is written as `false` in `engine_stats`.
  - It never sets `is_hub: true` in `GossipMsg::Heartbeat`.
- **Election filter.** `lease::elect` (`crates/mesh/src/lease.rs:130-159`) filters:

  ```rust
  candidates.iter().filter(|c| c.hub_pref != HubPref::Never && c.public_homed)
  ```

- **Result.**
  - The **bridge** has `public_homed: true` (it is homed on the public `mesh.relay`) and participates in the single public election alongside cloud and other public-homed engines.
  - Public engines elect solely among public-homed nodes.
  - The LAN-only engine never takes the lease even if the bridge drops: alone, its candidate set reduces to `{self}` with `hub_pref: Never` and `public_homed: false`, so `lease::elect` returns `None` (`crates/mesh/src/lease.rs:400-403`). `link`'s `Follower` (`crates/link/src/follow.rs:53-100`) stays `holding: false` and never opens `MeshUplink` at `/h/<mesh_device_id>`. No split lease is possible.

### 3.5 One extra hop via the bridge + explicit via-bridge peer state

#### Transport route lookup

Extend `replica::Transport` (`crates/replica/src/types.rs:133-142`) with a defaulted method so existing test `Loop` transports continue compiling unchanged:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerRoute {
    /// Directly connected over QUIC.
    Direct,
    /// Not directly connected; reachable in one extra hop through this connected bridge.
    Via(PeerId),
}

// On trait Transport:
fn route_for(&self, peer: &PeerId) -> Option<PeerRoute> {
    self.peers().contains(peer).then_some(PeerRoute::Direct)
}
```

`Net`'s implementation (`crates/mesh/src/net.rs:2107-2135`) returns `Some(PeerRoute::Direct)` whenever `peer` is in `self.presence`, else `Some(PeerRoute::Via(bridge))` picking the lowest `EndpointId` among currently connected bridges in `self.via` advertising `peer`, else `None`. Direct QUIC always wins over via-bridge.

#### Maximum two mesh hops (one extra hop), enforced on every interactive ALPN

When the hub IS the bridge, `route_for(lan_only)` is `Direct` — existing 1-hop code runs with zero overhead. When the hub is a public-only node (e.g. cloud), traffic runs `Hub -> Bridge -> LAN-only` (2 mesh hops). **No third hop is ever permitted:** a bridge only forwards to a target in its own **direct** `transport.peers()` set, never to another `PeerRoute::Via`, and every protocol strips or marks the bridge hop so the target refuses further relaying.

1. **JSON-RPC (`theuth/rpc/1`, `crates/agent/src/rpc_forward.rs`).**
   - Extend the `Forwarded` extension guard (`crates/agent/src/rpc_forward.rs:176-177`) from a tuple struct to record the via-peer:

     ```rust
     #[derive(Clone, Debug)]
     pub struct Forwarded {
         /// Originating front-door peer (or direct peer on a 1-hop call).
         pub origin: PeerId,
         /// Authenticated bridge peer that relayed this call (`None` on a direct hop).
         pub via: Option<PeerId>,
     }
     ```

   - `classify` (`rpc_forward.rs:263-283`) and `to_peer_via` (`rpc_forward.rs:486-508`) consult `transport.route_for(&owner)`:
     - `Direct`: unchanged — sends the raw JSON-RPC request line on `ALPN_RPC` to `owner`.
     - `Via(bridge)`: opens `ALPN_RPC` to `bridge` and writes a single-line envelope `{"_bridge_to":"<owner>","req":<raw JSON-RPC>}\n`.
     - `None`: returns `OWNER_UNREACHABLE` (`-32010`) immediately.
   - On the **bridge** (`serve` / `answer`, `rpc_forward.rs:572-616`): when a line parses with `"_bridge_to"`:
     - Refuse immediately if `req` itself contains `"_bridge_to"` or `"_bridged_by"` (no second via).
     - Refuse if `owner == peer` (no bounce) or `!transport.peers().contains(&owner)` (must be **directly** connected to this bridge; else return `OWNER_UNREACHABLE`).
     - Open `ALPN_RPC` directly to `owner`, write `{"_bridged_by":"<self_id>","_origin":"<peer>","req":<raw JSON-RPC>}\n`, and return the single JSON-RPC response line back to `peer`.
   - On the **owner** (`serve` / `answer`): when a line parses with `"_bridged_by"`:
     - Verify `_bridged_by` equals the authenticated QUIC `peer` that opened the stream (ignore any unauthenticated claim).
     - Refuse if `req` contains `"_bridge_to"` or `"_bridged_by"`.
     - Insert `Forwarded { origin, via: Some(peer) }` into `methods.extensions_mut()`.
     - The existing guards at `rpc_forward.rs:381-383` (`route` → `admit_forwarded` → `NOT_OWNER`), `422-425` (`engine_peer`), and `494-496` (`to_peer_via`) see `Forwarded` and refuse any further hop.
   - Because `route`, `to_creator`, `call_peer`, `to_engine`, and `rpc_machines::engine_call` (`crates/agent/src/rpc_machines.rs:1-8`) all funnel through `rpc_forward`, session writes (`send_input`, `steer`, `cancel`, prompt answers), `create_session{engine}`, `engine_logs`, `engine_desktop`, `engine_terminal`, `vault_*`, and per-machine Settings calls all work across the bridge automatically.
   - Attachments (`resolve_files_via`, `rpc_forward.rs:228-243`): prefer `forwarded.via.as_ref().unwrap_or(&forwarded.origin)` so the LAN-only owner fetches uploaded `attachments/<name>` markers from the bridge (which fetches from `origin` if not cached locally).

2. **Live event & prompt watch (`theuth/sessions/1`, `crates/sessionsync/src/events.rs`, `lib.rs`, `types.rs`).**
   - Append `Msg::WatchVia { owner: PeerId, session: String, from_seq: u64 }` to `sessionsync::types::Msg` (`crates/sessionsync/src/types.rs:43-120`) with a new tag byte.
   - In `events::follower` / `events::stream` (`crates/sessionsync/src/events.rs:821-913`): if `t.route_for(&owner)` is `Direct`, send `Msg::Watch` directly as today; if `Via(bridge)`, open `ALPN_SESSIONS` to `bridge` and send `Msg::WatchVia { owner, session, from_seq }`. Wake the backoff loop on `BridgePeers` updates advertising `owner` in addition to `PeerEvent::Connected(owner)`.
   - On the **bridge** (`SessionSync::serve`, `crates/sessionsync/src/lib.rs:198-225`): handle `Msg::WatchVia` by checking `transport.peers().contains(&owner)` (direct only — never sends another `WatchVia`), opening `ALPN_SESSIONS` directly to `owner` with plain `Msg::Watch { session, from_seq }`, and forwarding frames from `owner` back to the subscriber until either side closes. The owner sees a normal `Msg::Watch`, so a second hop is impossible.

3. **Artifact fetch (`theuth/artifacts/1`, `crates/artifact/src/mesh.rs`).**
   - Append `Req::Relay { target: PeerId, inner: Box<Req> }` to `artifact::mesh::Req`.
   - In `ask_peers` (`crates/artifact/src/mesh.rs:312-328`): try directly connected peers first (with `prefer` first if direct); if not found and `prefer` (or a known via-bridge peer) resolves to `PeerRoute::Via(bridge)`, open `ALPN_ARTIFACTS` to `bridge` sending `Req::Relay { target, inner }`.
   - On the **bridge** (`answer`, `crates/artifact/src/mesh.rs:170-207`): reject any `Req::Relay` whose `inner` is itself `Req::Relay`; verify `transport.peers().contains(&target)` (direct only); serve from local disk if already cached, else open one direct `ALPN_ARTIFACTS` stream to `target` with `inner` and stream `Reply` frames (`Chunk`… `End`, `Record`, or `Missing`) back.

4. **PTY streaming (`theuth/pty/1`, `crates/agent/src/rpc_engine.rs:516-595`).**
   - Add an optional `#[serde(default, skip_serializing_if = "Option::is_none")] pub target: Option<PeerId>` to `PtyOpen`.
   - In `open` (`rpc_engine.rs:516-532`): resolve `t.route_for(peer)`. On `Direct`, send `PtyOpen` with `target: None` directly. On `Via(bridge)`, open `ALPN_PTY` to `bridge` with `target: Some(peer.clone())`.
   - On the **bridge** (`serve`, `rpc_engine.rs:572-595`): if `open.target` is `Some(target)`, verify `transport.peers().contains(&target)` (direct only; else reply `PtyAck::refused(&OWNER_UNREACHABLE)`), clear `open.target = None`, open `ALPN_PTY` directly to `target`, write the stripped `PtyOpen` line, and `tokio::io::copy_bidirectional` the remaining stream. Because `target` is stripped, a third hop is impossible.

5. **Desktop tunnel (`theuth/desktop/1`, `crates/agent/src/rpc_engine.rs:645-775`).**
   - `desktop_via`'s initial `engine_desktop` RPC already crosses the bridge via `rpc_forward::call_peer`.
   - In `tunnel_port` (`rpc_engine.rs:672-703`): resolve `t.route_for(&to)`. On `Direct`, pipe raw websockify bytes on `ALPN_DESKTOP` as today. On `Via(bridge)`, open `ALPN_DESKTOP` to `bridge`, write a one-line JSON preamble `{"bridge_to":"<to>"}\n`, then pipe websockify bytes.
   - On the **bridge** (`serve_desktop`, `rpc_engine.rs:750-775`): peek the first byte of an accepted `ALPN_DESKTOP` stream. Normal websockify traffic begins with an HTTP request (`GET ...`), never `{`. If the first byte is `{`, read one bounded header line, parse `{"bridge_to":"<target>"}`, verify `transport.peers().contains(&target)` (direct only), open raw `ALPN_DESKTOP` directly to `target` **without** forwarding any `{...}` preamble (back-compat with `answer_desktop` and forbids a third hop), and `tokio::io::copy_bidirectional`.

#### Explicit via-bridge peer state (do not fake `Peer.reachable`)

`Peer.reachable` (`crates/mesh/src/types.rs:102`) remains true **only** when this engine holds a direct QUIC connection to that peer. Add an explicit path variant and bridge pointer instead:

- Add `PeerPath::Bridge` (`#[serde(rename = "bridge")]`) to `PeerPath` (`crates/mesh/src/types.rs:122-130`).
- Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub via: Option<EngineId>` to `Peer` (`crates/mesh/src/types.rs:94-114`).
- In `Net::peer_of` (`crates/mesh/src/net.rs:1557-1582`):
  - Direct connection up → `reachable: true, path: Lan | Relay, via: None`.
  - No direct connection, advertised by a connected bridge `B` → `reachable: false, path: Bridge, via: Some(B), rtt_ms: None`.
  - Neither → `reachable: false, path: Offline, via: None, rtt_ms: None`.
- UI treats `p.reachable || p.path === 'bridge'` as online-and-usable:
  - Settings › Network (`crates/desktop/ui/src/components/settings/Network.tsx:158-168`) shows `via <bridge name>` instead of `offline`.
  - "New chat on" (`crates/desktop/ui/src/components/chat/NewChatPane.tsx:48-85, 485-509`) enables the LAN-only row with status `via <bridge name>` while the bridge is up, and disables it as `offline` when the bridge drops.
  - Settings by machine (`crates/desktop/ui/src/lib/mesh-machines.ts:118-135`) and the switcher count (`crates/desktop/ui/src/link/engines.ts:157-159`) include `path === 'bridge'` peers as online.

## 4. Failure

- **Bridge down.** The public mesh is unchanged: public-homed engines stay connected via the public `iroh-relay` and keep their single hub election. When the bridge's `ALPN_MESH` control connection closes on public engines, `presence_drop` immediately clears that bridge's `via` entries, flipping the LAN-only peer from `path: bridge` to `path: offline`. LAN-only sessions stop replicating to the public side and stop accepting input, live watches, artifacts, PTY, or desktop streams from public clients (`OWNER_UNREACHABLE`); the last replicated transcript in the hub's `sessions.sqlite` remains readable offline. The LAN-only engine continues running local work normally and elects `holder: None` (no split lease). **Do not** fail over by having the LAN-only engine join the public relay.
- **Private relay down.** The bridge and the public mesh are unchanged; the bridge remains on the public `iroh-relay` and stays in the public election. The LAN-only engine becomes unreachable from the bridge (unless a direct LAN UDP path is already up); once its control connection drops, the bridge sends an updated `Msg::BridgePeers` removing it, and public clients see it transition to `offline`.
- **Public `iroh-relay` down.** Existing single-relay failure mode; this design does not change it.
- **Link relay independence.** `theuth-relay` (`crates/relay`, default `wss://relay.theuth.io`, which carries the iPad's WebSocket link to the hub) is a separate process and protocol from `iroh-relay` (`https://iroh.relay.theuth.io`, which carries QUIC mesh traffic between engines), so an outage of one does not imply an outage of the other.

## 5. Security

- **Relay membership is not mesh membership.** Reaching the private `iroh-relay` grants no access to the mesh. `iroh-relay` forwards opaque QUIC packets only; Ed25519 endpoint authentication, one-time pairing tokens, and `Net::is_paired` gating on `ALPN_MESH`, `ALPN_GOSSIP`, and `STREAM_ALPNS` (`crates/mesh/src/net.rs:1590, 2117`) are unchanged. A stranger on the private network who can talk to the private `iroh-relay` still cannot pair or open any stream without a valid pairing token.
- **No new capability boundary.** A public client (including the iPad via the public hub) that opens a LAN-only session through the bridge receives the same owner-mesh access any mesh peer has today — session RPCs, tools, artifacts, PTY terminal, and remote desktop. That is accepted because all participants belong to one owner mesh; per-zone capability policies are intentionally not part of this design.
- **Address leak closed.** By §3.3, the private relay URL and private/LAN IPs are omitted from default pairing tickets, remote cloud tickets, A2A invite tickets, pkarr/n0 DNS publications, and the replicated `peers.addrs_json` column stored on public engines. Public engines learn only the authenticated engine id, display name, and live "reachable via `<bridge>`" status.
- **Split-hostname rule.** The public and private `iroh-relay` servers **must** have distinct hostnames and must never share a DNS name or round-robin A records. As documented in `deploy/do/README.md:187-193`, two `iroh-relay` servers behind one hostname split the mesh because each engine homes on one server and holds a long-lived connection without re-resolving. `Net::bind` enforces distinct hostnames when `mesh.private_relay` is set (§3.2).

## 6. Seams

No new crates. All changes fit in existing crates and UI modules:

- **Config:**
  - `crates/mesh/src/config.rs` (`MeshConfig.private_relay`, optional `private_only` flag/validation)
  - `crates/mesh/src/lib.rs` (`BindOpts` wiring at `lib.rs:1452-1466`, `SetHubPref` refusal on private-only nodes)
  - `crates/theuth/src/main.rs` (default config template comment documenting `mesh.private_relay`)
- **Endpoint:**
  - `crates/mesh/src/net.rs` (`BindOpts`, `Net::bind`, `insert_relays`, `relay_state` for both relays, `Msg::BridgePeers`, in-memory `via` map & `presence_drop` cleanup, `peer_of`, `run_connector` skip for address-less `private_only` peers, `Transport::route_for`; plus contingent second `Endpoint` if §7 fails)
  - `crates/mesh/src/types.rs` (`PeerPath::Bridge`, `Peer.via`, `MeshStatus.private_relay`)
  - `crates/replica/src/types.rs` (`PeerRoute` enum and defaulted `Transport::route_for`)
- **Advertisement:**
  - `crates/mesh/src/pairing.rs` (`PeerEntry.private_only`)
  - `crates/mesh/src/store.rs` (local-only `peer_private_addrs` table, stripping private addrs in `put_peer` / `peer_entries`)
  - `crates/mesh/src/net.rs` (`me`, `pairing_string`, `remote_ticket`, `a2a_reach`, `merge_peers`, `push_peers`, pkarr filtering on bridge)
  - `crates/a2a/src/contacts/ceremony.rs` (no change expected; existing `ticket_target` SSRF check at `ceremony.rs:369-400` serves as regression backstop)
- **Lease:**
  - `crates/mesh/src/lease.rs` (`Candidate.public_homed`, `elect` filter)
  - `crates/mesh/src/store.rs` (`engine_stats` `public_homed` column/default, `put_stats`, `candidate`)
  - `crates/mesh/src/lib.rs` (`Ctx::recompute`, `write_stats`, `SetHubPref`)
  - `crates/link/src/follow.rs` (verify `holder: None` keeps `MeshUplink` stopped on private-only nodes)
- **Forwarding:**
  - `crates/agent/src/rpc_forward.rs` (`Forwarded { origin, via }`, `classify` / `route` / `to_peer_via` using `route_for`, `_bridge_to` / `_bridged_by` single-hop envelope in `serve` / `answer`, `resolve_files_via` preferring `via`)
  - `crates/sessionsync/src/types.rs` (`Msg::WatchVia`)
  - `crates/sessionsync/src/events.rs` (`follower` / `stream` dialing `Via(bridge)` and waking on bridge updates)
  - `crates/sessionsync/src/lib.rs` (`SessionSync::serve` splicing `Msg::WatchVia` to direct `Msg::Watch`)
  - `crates/artifact/src/mesh.rs` (`Req::Relay`, `ask_peers`, `answer` single-hop relay)
  - `crates/agent/src/rpc_engine.rs` (`PtyOpen.target` single-hop splice on `ALPN_PTY`; `{bridge_to}` preamble peek & splice on `ALPN_DESKTOP`)
- **UI & wire schema:**
  - `crates/desktop/ui/src/components/settings/Network.tsx` (`pathChip` for `bridge` / `via <bridge>`, `relayChip` rendering optional `private_relay`)
  - `crates/desktop/ui/src/components/chat/NewChatPane.tsx` (`machineRows` treating `path === 'bridge'` as online for "New chat on")
  - `crates/desktop/ui/src/lib/mesh-machines.ts` (`meshMachines` treating `path === 'bridge'` as online)
  - `crates/desktop/ui/src/link/engines.ts` (`applyMeshStatus` counting `path === 'bridge'` peers in online total)
  - Wire/TS bindings regeneration (`just ts-desktop`, `just wire-schema-accept` — additive fields/variants only)

## 7. Probe before building

**Status of the probe: not yet run.** Do not assume single-endpoint dual-homing works on `iroh 1.4.1` without running the live probe below.

Why the probe is mandatory: from theuth's call site, `Endpoint::home_relay_status().get()` returns a `Vec<RelayStatus>` that theuth indexes with `.first()` (`crates/mesh/src/net.rs:1105`). Inspecting the pinned `iroh 1.4.1` crate source in the cargo registry (`iroh-1.4.1/src/endpoint.rs:1406`, `src/socket.rs:505`, `src/socket/transports/relay/actor.rs:524, 699, 849, 1071-1073, 1232-1272`) shows that `HomeRelayWatch` wraps a single `Watchable<Option<RelayStatus>>`, `RelayActor::on_network_change` tracks one `report.preferred_relay: Option<RelayUrl>`, and `RelayActor::set_home_relay` sends `ActiveRelayMessage::SetHomeRelay(url == home_url_ref)` to every active relay — demoting every other `ActiveRelayActor` so `is_home_relay` becomes `false` and its `inactive_timeout` closes the connection. That static inspection strongly suggests one `Endpoint` in `iroh 1.4.1` will not keep two home relays alive, **but the live behavior has not been probed yet**.

### What the probe must run and observe

1. Start two independent `iroh-relay` instances — **Relay A** and **Relay B** — on distinct hostnames/ports, with no peering between them.
2. Start three iroh `1.4.1` endpoints with direct UDP / hole-punching disabled (relay-only):
   - **Bridge**: `RelayMode::Custom(RelayMap::empty())`, then `insert_relay(Relay A)` and `insert_relay(Relay B)`.
   - **Peer A**: homed only on Relay A; knows Bridge's `EndpointId` + Relay A URL (does **not** know Relay B).
   - **Peer B**: homed only on Relay B; knows Bridge's `EndpointId` + Relay B URL (does **not** know Relay A).
3. Wait past iroh's relay idle/inactivity timeout, then have **Peer A** (via Relay A) and **Peer B** (via Relay B) both initiate QUIC handshakes (`connect`) to **Bridge** concurrently, exchanging a message on each stream.
4. **Pass criterion:** both inbound handshakes succeed concurrently and repeatedly after idle periods, with neither peer knowing the other's relay URL and with no direct UDP path.

### Contingent fallback if the probe fails

If the probe fails (one relay is demoted or refuses inbound dials while the other is home), implement dual-homing inside `crates/mesh/src/net.rs` using a **second `iroh::Endpoint` in the same process**:

- Construct `private_endpoint: Option<Endpoint>` inside `Net::bind` only when `mesh.private_relay` is non-empty, passing the **same** `SecretKey` (so both endpoints share the exact same `EndpointId` and mesh identity).
- Bind `private_endpoint` with `RelayMode::Custom(RelayMap::from(private_relay))` and **no** `AddressLookup` / pkarr / n0 DNS / mDNS (which also eliminates any bridge pkarr leak of the private relay by construction).
- Attach a second `Router` on `private_endpoint` accepting `ALPN_MESH`, `ALPN_GOSSIP` (fed into the same gossip or bridged control/gossip path; note LAN-only never competes in hub election anyway), and `STREAM_ALPNS` — **not** `ALPN_A2A` — delivering into the exact same `Net::presence`, `Net::inbound`, and `Net::drive` handlers.
- Route outbound dials in `Net::run_connector` and `Net::connection`: peers whose addresses live in local `peer_private_addrs` dial through `private_endpoint`; all other peers dial through `self.endpoint`.
- No second process, no second `EngineId`, no second `mesh.sqlite`, and no changes to §3.1, §3.3, §3.4, §3.5, §4, or §5.

## 8. Not in this design

- **iOS / iPad as an iroh peer.** The iPad remains a `link.relay` WebSocket client talking to the public hub.
- **A private `theuth-relay`.** The private relay in this design is `iroh-relay` (`mesh.relay`), not a second `crates/relay` instance.
- **Relay federation / peering.** The public and private `iroh-relay` servers never peer with each other.
- **N-hop routing.** Traffic traverses at most one bridge hop (two mesh hops total: front door → bridge → owner). Chained bridges are refused.
- **Per-zone capabilities or ACLs.** One owner mesh means uniform owner access across bridged sessions; no zone policy is added here.
- **Two relays behind one DNS name.** Public and private `iroh-relay` instances must always have distinct hostnames; round-robin or split-horizon aliasing is forbidden.
