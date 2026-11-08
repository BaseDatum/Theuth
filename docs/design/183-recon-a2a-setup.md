A2ASetupRecon, read-only, main 60c86602. Nothing was run, and the mDNS service name comes from the crate source, not a live check.

**VERDICT:** What Sky describes (paired engines, mDNS on LAN plus relay, agents message each other) is the MESH, not the A2A plugin, and it is already built.
- Transport is in mesh/net.rs. Messaging is `peer_deliver` (design 69 P).
- It needs `agent.communication` on BOTH engines and both roots in `owner` scope.
- The A2A plugin is for other people's or vendors' agents over HTTPS. It has no mDNS, no relay and no pairing, and it is only loopback-verified (50 §12.5, :1454-1477).
- The "Network (A2A)" card Sky sees (Communication.tsx:250-371) belongs to the plugin.
- Missing for his goal: LAN browse, a pairing ceremony, a tier for other people, per-peer limits, setup UI. Cross-mesh messaging detail (item 2) is MeshMsgRecon's.
- Sky's live config.yaml has `agent.communication` true, `tools.approvals` false, `mesh{hub: prefer}`, `link.enabled` true, and no a2a section.

**CURRENT STATE**

DISCOVERY AND TRANSPORT (crates/mesh; iroh 1.2 QUIC, dial by Ed25519 endpoint id; Cargo.toml:53-55)
- **LAN:**
  - `iroh_mdns_address_lookup` is added at net.rs:500, but it only resolves ids already paired.
  - Nothing calls `subscribe()` or browses for "nearby" engines (grep).
  - The service name is the crate default "irohv1" (crate src lib.rs:83).
  - Internet lookup uses pkarr+DNS (net.rs:488-499).
- **Relay:**
  - It is the stock iroh-relay at https://iroh.relay.theuth.io (config.rs:55; deploy/relay/README.md:8,24, access="everyone").
  - `mesh.relay` takes a URL, `public` or `none` (net.rs:366-377).
  - Other `mesh.*` keys: discovery, name, bind, hub prefer|eligible|never, class, workspaces, lease secs (config.rs:11-50).
  - There is no tailscale code.
- **Lazy join:**
  - A lone engine has no relay, mDNS or DNS. `join_network` runs on first pairing or when peers are stored (net.rs:455-457,468-510).
  - So an unpaired engine is invisible on the LAN by design (67 single-engine rule).
  - LAN pairing needs no relay, because the string carries direct addresses (net.rs:472-474).
- **Second relay:** crates/relay is theuth-relay at wss://relay.theuth.io (link/types.rs:33). It carries Noise frames for phones, browsers, share-link guests and `nodes` (31/32/59). A2A has no relay.

PAIRING AND TRUST
- **What "paired" means:**
  - It is a mesh.sqlite `peers` row with `removed_ms` NULL (migrations/0001_mesh.sql:5-11, 0002:5), loaded into `Net.paired` (net.rs:391-396,797-812).
  - It gates every ALPN (net.rs:864,1189,1237,1262).
  - Unpair is a replicated tombstone (net.rs:774-795; pairing.rs:94-135). I found no key rotation (no rotate/rekey in mesh or secrets).
- **The string:**
  - Format is `1.<base32 json{id,name,relay,addrs,token,vault_key}>`, valid 5 minutes, single use (pairing.rs:19-39,76-92; net.rs:591-616).
  - Use `t mesh pair|add` or UI paste/QR.
  - The acceptor auto-accepts any valid token, with no Allow prompt and no code (net.rs:946-1024).
  - The merge is mutual and transitive (net.rs:739-771).
  - The vault key travels, and the bigger mesh's key wins (pairing.rs:204-222; net.rs:629-648,721-733).
- **What the connection carries** (replica/src/lib.rs:41-58):
  - records (vault rows secrets/store.rs:17, tasks, roster)
  - sessions log and takes (sessionsync)
  - knowledge, artifacts, docs relay, double
  - acp remote children (acp/src/mesh.rs:15-41)
  - lease gossip (mesh/lib.rs:513-659)
  - rpc
- **rpc trust:** forward.rs:360-383 answers the FULL JSON-RPC API as `RpcPeer::internal`. So paired means owner-level trust.

A2A PLUGIN (crates/a2a; official SDK; A2A 1.0 JSON-RPC+SSE over HTTPS; client.rs:223-255)
- **Gating:**
  - It is off by default (lib.rs:591-594) and needs a restart to enable.
  - The listener is loopback only, 127.0.0.1:7780 (config.rs:21,60-91). Exposure needs your own TLS proxy plus `a2a.public_url` (50 §6.1 :770-775).
  - Auth is a static bearer per peer, compared in constant time (server.rs:213-218,232).
  - `peer:<name>` needs BOTH `a2a.enabled` and `agent.communication` (communication.rs:1159-1169; a2a/lib.rs:441-442).
- **Peer config:**
  - Peers and exports live in `<data_dir>/a2a/peers.yaml`, hand-edited.
  - Fields: card_url, outbound/inbound secret (vault NAMES), local root UUIDs, exports, `allow_private` (config.rs:19,107-133).
  - `/a2a reload` applies edits (lib.rs:269-427).
  - There is no `a2a.peers` key.
- **peer:<name> routing:**
  - It maps to `PeerAddress::Remote` (agent/communication.rs:733-754), then to a `PeerSender` per alias (network.rs:200-205,279-307). That sender is an HTTP client only.
  - card_url must be https unless loopback (config.rs:264-288). Private or LAN IPs need `allow_private` (:290-323).
  - It discovers nothing and never uses the mesh.
- **Inbound:**
  - Flow: bearer → binding → export → ONE exact native session, as a peer exchange (server.rs:252-303; network.rs:868-896,899-1071).
  - Limits: 10 req/s, 8 concurrent, 64 KiB (server.rs:49-57).
  - There is no first-contact approval.
- **Approvals:**
  - Tool calls ride normal approvals (50 §0.5).
  - But `approvals:false` turns every ask into allow (tools/registry.rs:56,988).
  - `SetCap` (registry.rs:492-502) is never applied to exports. Only subagents, roster and schedule use it.

OVERLAPS: three layers, no link between them
- **(a) Local bus:**
  - `send_message` and `discover_peers`.
  - Ceiling `agent.communication`, default false (theuth/main.rs:879), plus a per-session opt-out and root scope owner|isolated (communication.rs:100-114,1134-1155).
- **(b) Mesh:**
  - A session owned by another engine gets `send_message` forwarded as `peer_deliver` on theuth/rpc/1 (agent/communication/mesh.rs:25-141; supervisor.rs:5488-5601; rpc/api.rs:3876-3891).
  - Both engines must allow it (mesh.rs:56-58).
- **(c) A2A:** per-peer sessions, exports and rate only.
- **Grants and limits:**
  - "Pairing grants communication" does not exist as a notion. Pairing never touches the switches.
  - The mesh has no per-peer permission (types.rs:94-108).
  - No per-peer risk cap exists anywhere.

UI (crates/desktop/ui/src; tabs SettingsDialog.tsx:68-81,118-131)
- **Without editing config.yaml:**
  - pair/unpair with string, QR or paste (components/settings/Mesh.tsx:137-203)
  - hub pref (:114)
  - SSH add-node via nodes (:214-266)
  - VMs
  - flip communication and root scope (Communication.tsx:110-241)
  - link relay and phone QR (Link.tsx:55-157; PairScreen.tsx:22-70)
- **Config or file only:**
  - mesh.relay/discovery/name/bind
  - a2a.enabled/listen/public_url
  - peers.yaml, vault secrets, TLS proxy
- **A2A card:** read-only plus Reload, and its text says "edit the file" (Communication.tsx:302,314).
- **Status gaps:** `MeshStatus` (types.rs:230-246) has no relay state, LAN/relay path or nearby list. `PeerInfo.engine` exists (communication.rs:401) but no UI renders it.

DESIGNS (docs/design)
- **31 link, 32 nodes, 59 documents, 62 broadcast:** promised and built. 59's guest allow-list (link/conn.rs:535-580) is a precedent for limited trust.
- **50:** built, default off. It rejects an mDNS scan (:662-664), a relay mode (:774-775) and PKI (:760). HTTPS is unverified (:1454-1477).
- **67 mesh:** promised "engines on one LAN find each other" (:24,:118). Built as lazy pair-then-resolve. Security is at :462-467.
- **Built (v2.18.12):** 68 sessions, 69 devices plus 69 P peer messages (Sky ruling 2026-09-25), 79 VMs, 139, 180, 181 move-to-node, 182 task-home.
- **Not covered:** no design covers cross-owner trust, nearby discovery or a pairing UI.

**GAPS vs goal (LAN mDNS + relay, paired => talk, good UI)**
- **G1:** mDNS only resolves known ids. There is no browse, advertise or Nearby list, and an engine is invisible until first pairing.
- **G2:** Pairing is string/QR only, with no Allow and no code. The string holds the vault key. One pairing means whole mesh plus full RPC, so it is unusable for other people.
- **G3:** Relay and path are config-only. The UI cannot show relay state, LAN vs relay, or errors.
- **G4:** Same-mesh talk sits behind three invisible switches (global, session, scope), default off. Nothing happens at pair time and there is no per-peer toggle.
- **G5:**
  - Sky's laptop mesh vs the A+xeus mesh (knowledge 2026-11-07): pairing them would merge vaults, tasks and sessions. The laptop adopts the bigger mesh's key and its secrets replicate.
  - The only alternative today is HTTP A2A: TLS proxy, hand-made bearers, YAML, UUIDs, no LAN or relay.
- **G6:** There is no per-peer risk cap or first-contact approval. With approvals off an export is unguarded, and unpair does not revoke the key.
- **G7:** Three pairing strings (mesh, node key, device QR) and two relays.

**DESIGN OUTLINE "network A2A over the mesh" (proposal)**

Two tiers on one iroh identity:
- **FAMILY:** today's mesh, unchanged (full trust, `peer_deliver`).
- **CONTACTS (new):** other meshes or people.
  - Local-only rows (not in replicated `peers`, not gossiped).
  - No vault key.
  - Only a new ALPN `theuth/a2a/1` (add at net.rs:344-353 and replica/lib.rs).
  - Dialed on demand over LAN or the iroh relay.

Wire:
- A2A 1.0 requests, one bi-stream per call.
- Reuse the a2a `Handler` (server.rs:440-474) with principal = iroh remote id → binding (no bearer), plus a mesh `PeerSender` (network.rs:200-205). That keeps `peer:<name>`, receipts and grant epochs as built.
- HTTP A2A stays for non-theuth vendors.

**Rulings needed from Sky:**
- **R1 Same-mesh engines:** keep `peer_deliver` (recommended), or wrap it in A2A for uniformity.
- **R2 Does pairing grant messaging?**
  - (a) as today
  - (b) the pair dialog asks "let agents on both engines message each other" and writes `agent.communication` (recommended for family)
  - (c) per-peer local toggles
  - Contacts stay off until an export is granted.
- **R3 Do other people's engines count as paired?** Contact tier only (recommended), never mesh membership (vault key, replication, full RPC).
- **R4 Inbound:**
  - A contact's export is a fresh capped inbox session, not an existing chat.
  - A per-contact `SetCap` of read_only or mutating holds even with approvals off.
  - The first message raises an Attention allow/deny (once/always/block).
- **R5 Discovery:**
  - Opt-in pairing mode (custom mDNS service, name and class in TXT), only while Settings › Mesh Add is open.
  - mDNS is unauthenticated, so require Allow plus a 6-digit code on both sides.
  - Never always-advertise.
- **R6 Overturn design 50's no-mDNS / no-relay-mode / no-PKI?** This needs Sky's explicit yes. The relay would be the existing iroh relay (open access) or self-hosted.

**UI:**
- Settings › Mesh becomes Network, with four sections:
  - This engine (relay chip, LAN switch).
  - Nearby (Pair…).
  - Engines (LAN/relay chip, message toggle, engine name in Peers.tsx).
  - Contacts (QR/string intro, export picker, risk cap, block).
- The Communication › A2A card becomes editable (RPC writes peers.yaml and generates secrets).
- An Attention card handles first contact.

**SIZE:** about 4-6k LOC.
- **~20 Rust files:**
  - mesh: net, pairing, store plus migration, types, lib, config
  - a2a: new mesh.rs plus server, client, config, lib
  - agent: communication.rs, network.rs, store plus migration
  - rpc: api.rs, ~8 methods
  - replica: one ALPN const
  - theuth main.rs
  - ts bindings
- **~12 UI files:** Mesh, Communication, Peers, CommunicationNotice, slash, queries, rpc keys, 3-4 new components, attention.
- **Phase 1** (family polish: Nearby, Allow plus code, relay/path status, pair-time switch): about 1 worker-day.
- **Phase 2** (contacts): about 2-3 worker-days, plus review and a two-scratch-engine proof (docs/design/180-notes.md).

Line numbers are at 60c86602.
A2ASetupRecon final (plain text, supersedes my earlier long reply). Read-only, main 60c86602. VERDICT: what Sky describes (paired engines, mDNS on LAN + relay, agents message each other) is the MESH, not the A2A plugin, and it is already built: transport in mesh/net.rs, messaging as peer_deliver (69 P). It needs agent.communication on BOTH engines and both roots in `owner` scope.
The A2A plugin is for other people's/vendors' agents over HTTPS: no mDNS, no relay, no pairing, loopback-verified only (50 s12.5 :1454-1477). The "Network (A2A)" card Sky sees (Communication.tsx:250-371) is the plugin's. Missing for his goal: LAN browse, pairing ceremony, a tier for other people, per-peer limits, setup UI. (Item 2, cross-mesh messaging detail: MeshMsgRecon.)
Sky's live config.yaml: agent.communication true, tools.approvals false, mesh{hub: prefer}, link.enabled true, no a2a section.

CURRENT STATE
DISCOVERY/TRANSPORT (crates/mesh; iroh 1.2 QUIC, dial by Ed25519 endpoint id; Cargo.toml:53-55)
- LAN: iroh_mdns_address_lookup added at net.rs:500; it only resolves ids already paired. No subscribe()/browse/"nearby" anywhere (grep). Service name = crate default "irohv1" (crate src lib.rs:83; not run live). Internet: pkarr+DNS lookup net.rs:488-499.
- Relay: stock iroh-relay at https://iroh.relay.theuth.io (config.rs:55; deploy/relay/README.md:8,24 access="everyone"). mesh.relay = URL|public|none (net.rs:366-377). Other mesh.* keys: discovery, name, bind, hub prefer|eligible|never, class, workspaces, lease secs (config.rs:11-50). No tailscale code.
- Lazy: a lone engine has no relay/mDNS/DNS; join_network runs on first pairing or stored peers (net.rs:455-457,468-510), so an unpaired engine is invisible on the LAN by design (67 single-engine rule). LAN pairing needs no relay: the string carries direct addrs (net.rs:472-474).
- Second relay: crates/relay = theuth-relay wss://relay.theuth.io (link/types.rs:33), Noise frames for phones, browsers, share-link guests, `nodes` (31/32/59). A2A has no relay.
PAIRING/TRUST
- "paired" = a mesh.sqlite `peers` row with removed_ms NULL (migrations/0001_mesh.sql:5-11, 0002:5) loaded into Net.paired (net.rs:391-396,797-812); it gates every ALPN (net.rs:864,1189,1237,1262). Unpair = replicated tombstone (net.rs:774-795; pairing.rs:94-135); no key rotation (no rotate/rekey in mesh or secrets).
- String `1.<base32 json{id,name,relay,addrs,token,vault_key}>`, 5 min, single use (pairing.rs:19-39,76-92; net.rs:591-616). `t mesh pair|add` or UI paste/QR. Acceptor auto-accepts any valid token: no Allow prompt, no code (net.rs:946-1024). Merge is mutual+transitive (net.rs:739-771). Vault key travels; bigger mesh's key wins (pairing.rs:204-222; net.rs:629-648,721-733).
- Carried (replica/src/lib.rs:41-58): records (vault rows secrets/store.rs:17, tasks, roster), sessions log + takes (sessionsync), knowledge, artifacts, docs relay, double, acp remote children (acp/src/mesh.rs:15-41), lease gossip (mesh/lib.rs:513-659), rpc.
- rpc: forward.rs:360-383 answers the FULL JSON-RPC API as RpcPeer::internal. So paired = owner-level trust.
A2A PLUGIN (crates/a2a; official SDK; A2A 1.0 JSON-RPC+SSE over HTTPS; client.rs:223-255)
- Off by default (lib.rs:591-594; restart to enable); listener loopback only 127.0.0.1:7780 (config.rs:21,60-91); exposure = your TLS proxy + a2a.public_url (50 s6.1 :770-775). Auth = static bearer per peer, constant-time (server.rs:213-218,232). peer:<name> needs BOTH a2a.enabled and agent.communication (communication.rs:1159-1169; a2a/lib.rs:441-442).
- Peers/exports are in <data_dir>/a2a/peers.yaml, hand-edited: card_url, outbound/inbound secret = vault NAMES, local root UUIDs, exports, allow_private (config.rs:19,107-133); `/a2a reload` (lib.rs:269-427). There is no a2a.peers key.
- peer:<name> -> PeerAddress::Remote (agent/communication.rs:733-754) -> PeerSender per alias (network.rs:200-205,279-307) = HTTP client only. card_url must be https unless loopback (config.rs:264-288); private/LAN IPs need allow_private (:290-323). Discovers nothing, never uses the mesh.
- Inbound: bearer -> binding -> export -> ONE exact native session as a peer exchange (server.rs:252-303; network.rs:868-896,899-1071); 10 req/s, 8 concurrent, 64 KiB (server.rs:49-57). No first-contact approval.
- Tool calls ride normal approvals (50 s0.5), but approvals:false turns every ask into allow (tools/registry.rs:56,988) and SetCap (registry.rs:492-502) is never applied to exports (only subagents, roster, schedule use it).
OVERLAPS: three layers, no link between them
- (a) local bus: send_message/discover_peers; ceiling agent.communication default false (theuth/main.rs:879) + per-session opt-out + root scope owner|isolated (communication.rs:100-114,1134-1155).
- (b) mesh: a session owned by another engine gets send_message forwarded as peer_deliver on theuth/rpc/1 (agent/communication/mesh.rs:25-141; supervisor.rs:5488-5601; rpc/api.rs:3876-3891); both engines must allow (mesh.rs:56-58). (c) A2A: per-peer sessions/exports/rate only.
- "Pairing grants communication": no such notion; pairing never touches the switches. No per-peer permission in mesh (types.rs:94-108). No per-peer risk cap anywhere.
UI (crates/desktop/ui/src; tabs SettingsDialog.tsx:68-81,118-131)
- Without config.yaml: pair/unpair + string/QR/paste (components/settings/Mesh.tsx:137-203), hub pref (:114), SSH add-node via nodes (:214-266), VMs, flip communication + root scope (Communication.tsx:110-241), link relay + phone QR (Link.tsx:55-157; PairScreen.tsx:22-70).
- Config/file only: mesh.relay/discovery/name/bind, a2a.enabled/listen/public_url, peers.yaml, vault secrets, TLS proxy. A2A card is read-only + Reload, text says "edit the file" (Communication.tsx:302,314). MeshStatus (types.rs:230-246) has no relay state, LAN/relay path or nearby list; PeerInfo.engine exists (communication.rs:401) but no UI renders it.
DESIGNS (docs/design)
- 31 link, 32 nodes, 59 documents (guest allow-list link/conn.rs:535-580 = limited-trust precedent), 62 broadcast: promised and built. 50: built, default off; rejects mDNS scan (:662-664), a relay mode (:774-775), PKI (:760); HTTPS unverified (:1454-1477).
- 67 mesh: promised "engines on one LAN find each other" (:24,:118), built as lazy pair-then-resolve; security :462-467. 68 sessions, 69 devices + 69 P peer messages (Sky ruling 2026-09-25), 79 VMs, 139, 180, 181 move-to-node, 182 task-home: built (v2.18.12). No design covers cross-owner trust, nearby discovery or a pairing UI.

GAPS vs goal (LAN mDNS + relay, paired => talk, good UI)
G1 mDNS only resolves known ids; no browse/advertise/Nearby; invisible until first pairing. G2 Pairing = string/QR only, no Allow, no code; string holds the vault key; one pairing = whole mesh + full RPC, unusable for other people.
G3 Relay/path is config-only; UI cannot show relay state, LAN vs relay, errors. G4 Same-mesh talk sits behind three invisible switches (global, session, scope), default off; nothing at pair time; no per-peer toggle.
G5 Sky's laptop mesh vs A+xeus mesh (knowledge 2026-11-07): pairing them would merge vaults/tasks/sessions (laptop adopts the bigger mesh's key and its secrets replicate); the only alternative is HTTP A2A: TLS proxy, hand bearers, YAML, UUIDs, no LAN/relay.
G6 No per-peer risk cap or first-contact approval; with approvals off an export is unguarded; unpair does not revoke the key. G7 Three pairing strings (mesh, node key, device QR), two relays.

DESIGN OUTLINE "network A2A over the mesh" (proposal)
Two tiers on one iroh identity. FAMILY = today's mesh, unchanged (full trust, peer_deliver). CONTACTS = new: other meshes/people; local-only rows (not in replicated peers, not gossiped); no vault key; only new ALPN theuth/a2a/1 (add at net.rs:344-353 + replica/lib.rs); dialed on demand over LAN or iroh relay.
Wire = A2A 1.0 requests, one bi-stream per call; reuse a2a Handler (server.rs:440-474) with principal = iroh remote id -> binding (no bearer) and a mesh PeerSender (network.rs:200-205), so peer:<name>, receipts and grant epochs stay as built. HTTP A2A stays for non-theuth vendors.
Rulings needed from Sky:
R1 Same-mesh engines: keep peer_deliver (recommended) or wrap in A2A for uniformity.
R2 Does pairing grant messaging? (a) as today; (b) pair dialog asks "let agents on both engines message each other", writes agent.communication (recommended for family); (c) per-peer local toggles. Contacts: off until an export is granted.
R3 Do other people's engines count as paired? Contact tier only (recommended), never mesh membership (vault key, replication, full RPC).
R4 Inbound: a contact's export = fresh capped inbox session, not an existing chat; per-contact SetCap read_only|mutating (holds with approvals off); first message = Attention allow/deny (once/always/block).
R5 Discovery: opt-in pairing mode (custom mDNS service, name+class in TXT) only while Settings>Mesh Add is open; mDNS is unauthenticated, so Allow + 6-digit code on both sides; never always-advertise.
R6 Overturn design 50's no-mDNS/no-relay-mode/no-PKI? Needs Sky's explicit yes. Relay = existing iroh.relay (open access) or self-hosted.
UI: Settings>Mesh becomes Network: This engine (relay chip, LAN switch), Nearby (Pair...), Engines (LAN/relay chip, message toggle, engine name in Peers.tsx), Contacts (QR/string intro, export picker, risk cap, block). Communication>A2A card becomes editable (RPC writes peers.yaml, generates secrets). Attention card for first contact.
SIZE (estimate) ~4-6k LOC: ~20 Rust files (mesh net/pairing/store+migration/types/lib/config; a2a new mesh.rs + server/client/config/lib; agent communication.rs, network.rs, store+migration; rpc api.rs ~8 methods; replica const; theuth main.rs; ts bindings) + ~12 UI files (Mesh, Communication, Peers, CommunicationNotice, slash, queries, rpc keys, 3-4 new components, attention).
Phase 1 family polish (Nearby, Allow+code, relay/path status, pair-time switch) ~1 worker-day; phase 2 contacts ~2-3 worker-days + review + two-scratch-engine proof (docs/design/180-notes.md).
Caveats: nothing was run; mDNS name is from crate source; line numbers are at 60c86602.