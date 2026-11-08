# 183 — Remote sessions, mesh messaging and A2A contacts

Status: DESIGN — rulings D1–D5 from Sky 2026-11-08 11:56; not built. Supersedes design 68 ruling 5 (user sessions follow you) for writes from a non-owner engine.

Inputs: `183-rulings.md` (fixed), `183-recon-mesh-messaging.md`, `183-recon-a2a-setup.md`,
`183-recon-session-stream.md`, `183-research-a2a-standards.md`, mock
`assets/183-network-and-remote-sessions.html` (panels 1–5). Base: origin/main `60c86602` (v2.18.12). Every
`path:line` below was re-opened in this worktree at that commit; recon lines that moved are corrected in §20.

## 0. Decisions in one screen

| Ruling | Mechanism | Section |
|---|---|---|
| D1 Run on the owner | `forward::classify` returns `Forward(owner)` for a user session whose owner is connected; the front door never takes on write. Only Continue here (`session_take user`), Move to (`session_move`) and the OwnerOfflineCard's "Continue here and resume" move a session | §4.1–§4.3 |
| D2 Owner offline → ask | Owner not connected → `-32010 OwnerUnreachable` at once (no `takeover_secs` wait, no `TakeReason::Write`); the existing `OwnerOfflineCard` offers Continue here; nothing runs until accepted; reads stay on the replica | §4.4 |
| D3 Same mesh ⇒ agents message by default; pairing ceremony is for separate harnesses, over an A2A standard | `agent.communication` defaults on for an engine with paired peers; a per-machine "Agents can message" switch; the forward leg leaves the Agents mailbox. Separate harnesses become **contacts**, paired by the A2A-standard ceremony of §7 | §6, §7 |
| D4 Settings go with the session | `sessions.approvals` and `sessions.communication` columns on the replicated row; the owner's registry and policy load them; the moved notice reports them | §4.5, §9.1 |
| D5 Design contacts now | Contact tier: local-only rows, no vault, no replication, A2A only, per-contact risk cap, first-contact Attention, fresh capped inbox session | §7, §8 |

Writer's calls:

- **W-1** Cells forward run/stop/input/ask to the owner through the existing `route_scope`; live PTY bytes (`cell_attach`) stay owner-local, the cell shows "live output on cloud-1" until the run settles. Reuses `FORWARDED` run scopes; no streaming proxy.
- **W-2** Terminals (`subscribe_pty`) and job PTYs stay owner-local in all phases: the unary `theuth/rpc/1` cannot carry a stream, and a generic stream proxy is design 183's largest risk for its smallest gain.
- **W-3** "Connected" means `owner ∈ transport.peers()` at call time; a connected owner that then times out (30 s) is also `-32010`. No retry inside the front door: the composer already retries once.
- **W-4** `sessions.approvals` is tri-state (`NULL` inherit, `'on'`, `'off'`), because today a session can only opt *out* of approvals (`registry.rs:987-991`); ask 7 ("turn on approvals there") is impossible on a node whose `tools.approvals` is false without a session pin.
- **W-5** Readers that depend on the owner's engine-local defaults (`get_approvals`, `get_communication`, `list_models`) forward to the owner; the session row alone answers when the owner is offline, with `global: null`.
- **W-6** The owner's `approval_timeout_secs` (300) is unchanged by relaying; the card shows the owner's deadline. Nobody answering still denies.
- **W-7** The sidebar running glyph for remote sessions is **rows-only**: `list_sessions` derives `running` for a row owned elsewhere from the replicated open turn, and the UI refetches the list on replicated `ContextChanged` (debounced). No summary event.
- **W-8** The moved notice is a typed user row (`HistoryEntry::Moved`, the advisory pattern) written by the taker after `set_owner`. Idle: in context. A turn open at the take: written outside context (UI only) and the model reads the extended `<engine_restart/>` instead, so no user row lands between a tool call and its result.
- **W-9** A cwd that does not exist on the taker never blocks a move: the seat falls back to the taker's home, the notice says so (`cwd_missing="true"`), and `sessions.cwd` keeps the original path so a move back restores it.
- **W-10** New chat on a machine = `CreateSessionOptions.engine`; the attached engine forwards the whole `create_session` to that engine with `forward::to_peer`. The target owns the session from epoch 1.
- **W-11** Machine facts ride the existing replicated `engine_stats` record: `+os`, `+os_name`, `+arch`, `+host`. `MeshStatus` gains `stats: Vec<EngineStats>`. No `engine_info` RPC.
- **W-12** `path_complete` and `list_models` gain a trailing `engine: Option<String>` (jsonrpsee treats a missing trailing optional as `None`, so old clients are unchanged).
- **W-13** Mesh messaging default: an *absent* `agent.communication` means on when the engine has ≥1 paired peer (source `mesh`); the first-run template stops writing `communication: false`; an untouched template line is commented out once on upgrade. An explicit value always wins.
- **W-14** The per-machine "Agents can message" switch is a **local** deny list `mesh.messaging_off: [engine id]` on the engine where it is flipped: it blocks peer messages both ways between this engine and that one. Local, so one machine cannot silence another's policy.
- **W-15** Agent messages to an offline owner keep failing fast; a retry with the same `request_id` re-forwards a `failed{OwnerUnreachable}` exchange. No queue (§6.4 gives the reasoning against D2).
- **W-16** Desk addressing: the roster sets the desk root's alias to `<agent>@<engine>`; `send_message` resolves aliases as today; no new address scheme.
- **W-17** Family pairing also gets Nearby (mDNS browse) and a 6-digit check code (mock panels 1, 2). Panel 2's "Let agents on both machines message each other" checkbox is dropped: D3 makes messaging the default.
- **W-18** The widened relay uses two new `theuth/sessions/1` frames (`Watch`, `Bus`). An old owner rejects `Watch` and the viewer falls back to `Events` (AgentEvent only); an old viewer never sees `Bus`.
- **W-19** The relay's pending prompts survive the viewer's snapshot: the viewer's `EventRelay` keeps the relayed `*Requested` set per followed session, and `PendingPrompts::collect` merges it.
- **W-20** to **W-27** are the Phase 3 calls, listed in §7.0.

What ships: **Phase 1** lets any machine (and the phone through the hub) operate, watch and configure a session
on another machine without moving it, tells the model plainly when a session does move, and starts new chats
on a chosen machine. **Phase 2** makes agents in one mesh message each other by default, off the Agents
mailbox, and replaces Settings › Mesh with Settings › Network (paths, latency, Nearby, code-checked pairing).
**Phase 3** adds contacts: separate harnesses found over mDNS or reached over the relay, paired with the A2A
ceremony, limited to exported agents under a per-contact risk cap.

## 1. Sky's asks and what this design answers

| # | Ask (short quote) | Answer | Sections |
|---|---|---|---|
| 1 | "a better UI for setting up network A2A … with mdns … as well as through the relay, agents can communicate if they're paired" | Inside one mesh this is already the transport; Phase 2 makes it visible and default (Settings › Network). Between separate harnesses: contacts (Phase 3) | §6, §7, §12.6–§12.7 |
| 2 | "if two nodes are in the same mesh, can those sessions communicate freely … I believe that was the intention" | Built as 69 P but behind three off-by-default switches and on the Agents mailbox. Phase 2: on by default (D3), off the mailbox, named desks, a refusal that names the engine | §2.1, §6 |
| 3 | "why doesn't the state of that session stream to other reachable nodes … and update live" | The 68 relay exists but never starts (`subscribe_events(null,null)`). Phase 1b: follow on focus, widened to approvals, asks, jobs and cells | §2.3, §5.1 |
| 4 | "do I have UI to start a session on a specific node" | No. Phase 1b: New chat on [machine ▾] with that machine's folders and models | §5.4, §12.5 |
| 5 | "a harness message to the agent/model that says, your session has moved to node: x, os: linux … custom UI … not a blob of xml" | Today nothing on an idle move; `<engine_restart/>` shows as raw XML. Phase 1b: `<session_moved/>` typed row + MovedCard; `<engine_restart/>` extended and carded; prompt header names the machine | §5.3, §11 |
| 6 | "sending a message from another machine … should not cause the session to move … just like … a paired pure UI device" | D1: forward to the owner; the phone gets the same fix because it rides the hub's front door | §4 |
| 7 | "control approvals from the three dots for a remote session … turn on approvals there without moving" | ⋯ menu: Approvals, Agent messaging, Model, Effort, forwarded; persisted on the row (D4); relayed approval cards | §4.5, §4.6, §12.2 |

## 2. As built today (v2.18.12)

### 2.1 Messaging across engines

| Fact | Where |
|---|---|
| `send_message` to `session:<uuid>` owned elsewhere: after `accept`, `remote_owner` → `forward_peer` → `communication::mesh::forward`, one `peer_deliver` JSON-RPC line on `theuth/rpc/1` | `crates/agent/src/supervisor.rs:5531-5534`, `:5552-5601`, `:5749-5753`; `crates/agent/src/communication/mesh.rs:89-149` |
| Owner intake: `peer_deliver` requires the `Forwarded` marker, then `Agents::peer_deliver` refuses a row not `here`, mirrors lineage and runs its own `deliver` | `crates/rpc/src/api.rs:3876-3891`; `crates/agent/src/supervisor.rs:5488-5526` |
| The owner never checks that the forwarding peer owns `envelope.from`: any paired engine may send as any session | `crates/rpc/src/api.rs:3883-3890` |
| Gates: sender and receiver each evaluate `Policy::route` (global ceiling, per-session opt-out, scope) | `crates/agent/src/communication.rs:1134-1155`; `crates/agent/src/supervisor.rs:5657` |
| `agent.communication` defaults off: code default and the first-run template | `crates/agent/src/lib.rs:3769`; `crates/theuth/src/main.rs:879` |
| Owner-side global off is reported as `denied: send_message: communication_disabled: communication for <from> is disabled globally…` — it names the sender's session, never the engine | `crates/agent/src/communication/mesh.rs:68-70`; `crates/agent/src/supervisor.rs:1903-1905`; `crates/agent/src/communication.rs:835` |
| Fail fast: owner not connected → `Failure::Unreachable` at once; a silent owner after 30 s | `crates/agent/src/communication/mesh.rs:32`, `:99-101`, `:129-138` |
| A retry with the same `request_id` returns the stale `failed` receipt, nothing re-sent | `crates/agent/src/store/communication.rs:622-635`; `crates/agent/src/supervisor.rs:5699-5705` |
| The mailbox stall: `impl Message<Deliver>` awaits the whole network leg inline, and the failure path asks `mesh::Status` inline | `crates/agent/src/supervisor.rs:5467-5473`, `:5571-5577`, `:5586-5588`; `crates/agent/src/communication/mesh.rs:169-183` |
| Off-mailbox precedent (176 E1): `DelegatedReply` + `ctx.spawn` | `crates/agent/src/supervisor.rs:5974-5991` |
| `PeerEnvelope{id, from, text, reply_to, request_id}`: no `chain` | `crates/agent/src/communication.rs:578-591` |
| Chain limit 256 is inherited only from a local sender | `crates/agent/src/communication.rs:96`; `crates/agent/src/supervisor.rs:2060-2068` |
| `discover_peers` rows owned elsewhere: `state: remote`, `engine`, no activity, no reachable flag; `query` matches name and address only | `crates/agent/src/supervisor.rs:6166-6201`, `:6019-6021` |
| Desks get no alias; other roots list as 8-char ids | `crates/roster/src/lib.rs:862-900`; `crates/agent/src/supervisor.rs:6142-6151` |

### 2.2 Discovery and pairing

| Fact | Where |
|---|---|
| Transport is iroh 1.2 QUIC, dial by Ed25519 endpoint id | `Cargo.toml:53-55` |
| mDNS is lookup only (`MdnsAddressLookup` resolves ids already paired); nothing browses or advertises | `crates/mesh/src/net.rs:500` |
| A lone engine joins no network: `join_network` runs only with stored peers or at first pairing | `crates/mesh/src/net.rs:455-457`, `:475` |
| Relay default `https://iroh.relay.theuth.io`; `mesh.relay` = URL, `public` or `none` | `crates/mesh/src/config.rs:55`; `crates/mesh/src/net.rs:366-377` |
| The pairing string is `1.<base32 json Payload{id, name, relay, addrs, token, vault_key}>`, token TTL 5 min | `crates/mesh/src/pairing.rs:19-39`, `:76-92` |
| The acceptor auto-accepts any live token: no prompt, no code | `crates/mesh/src/net.rs:946-1024` |
| The vault key travels; the bigger mesh's key wins | `crates/mesh/src/pairing.rs:204-222`; `crates/mesh/src/net.rs:629-648` |
| "Paired" = a `peers` row with `removed_ms` NULL, loaded into `Net.paired`; every ALPN is gated on it | `crates/mesh/migrations/0001_mesh.sql:5-11`; `crates/mesh/src/net.rs:391-396`, `:1189`, `:1237` |
| Unpair is a replicated tombstone; no key rotation exists in mesh or secrets | `crates/mesh/src/net.rs:774-795` |
| `theuth/rpc/1` answers the full JSON-RPC API as `RpcPeer::internal` | `crates/rpc/src/forward.rs:360-361` |
| `Peer{id, name, addrs, reachable, last_seen_ms, hub_pref}`; `MeshStatus` has no relay state, path or latency | `crates/mesh/src/types.rs:94-108`, `:230-246` |
| Mesh RPCs: `mesh_status`, `mesh_pairing_string`, `mesh_add`, `mesh_remove`, `mesh_set_hub` | `crates/rpc/src/api.rs:2543-2556` |

### 2.3 Liveness of a session owned elsewhere

| Fact | Where |
|---|---|
| The relay exists: owner `serve`, viewer `follower` re-publishing on the local bus | `crates/sessionsync/src/events.rs:309-334`, `:348-412` |
| It starts only from a per-session `subscribe_events`; `follow` has no other production caller | `crates/rpc/src/api.rs:9351`; `crates/sessionsync/src/events.rs:88` |
| The desktop opens one global `subscribe(null, null)`; `set_event_focus` only filters deltas and never follows | `crates/desktop/ui/src/App.tsx:257`; `crates/rpc/src/api.rs:9478-9500` |
| The relay subscribes to `AgentEvent` only; approvals, asks, jobs and cells are separate bus types | `crates/sessionsync/src/events.rs:144`; `crates/rpc/src/bridge.rs:216`, `:222-227`, `:266` |
| `ApprovalResolved{id, decision, timed_out}` carries no session | `crates/tools/src/approval.rs:61-68` |
| The pending-prompt snapshot is built from this engine's actors only | `crates/rpc/src/api.rs:2932-2960` |
| Replication is per commit: `ContextChanged{reason: "replicated"}` per applied batch | `crates/sessionsync/src/sync.rs:652-661` |
| The reducer drops a same-revision `ContextChanged`; partial upserts never bump `context_revision`, so 5 s partials replicate but never redraw | `crates/desktop/ui/src/store/reduce.ts:891`; `crates/agent/src/store/durability.rs:554-590`; `crates/agent/src/store.rs:3623-3625` |
| A replica's `load_session` says `running: false` | `crates/agent/src/supervisor.rs:7429-7439` |
| `ContextChanged` refreshes search only, not the sidebar | `crates/desktop/ui/src/lib/rpc.ts:216` |

### 2.4 Take on write

| Fact | Where |
|---|---|
| `classify`: `Here` / `Take` (user session) / `Forward(owner)` (desk) | `crates/rpc/src/forward.rs:86-93`, `:182-195` |
| `Take` sends `TakeSession{Write, by_user: true}`; an unreachable owner waits `takeover_secs` (300) | `crates/rpc/src/forward.rs:221-247`; `crates/sessionsync/src/owner.rs:150-152`; `crates/sessionsync/src/config.rs:25` |
| 37 `forward::route` call sites in `api.rs` (list in §4.2) plus cells' `route_scope` | `crates/rpc/src/api.rs:3812`…`:8945`; `crates/rpc/src/cells.rs:82-107` |
| Cell reads ask the owner and never take | `crates/rpc/src/cells.rs:109-147` |
| Approval and ask answers forward, never take | `crates/rpc/src/api.rs:3205-3221`, `:5000-5036` |
| `path_complete` already forwards | `crates/rpc/src/api.rs:4622-4642` |
| The phone talks to the hub; a send on a non-hub session takes it to the hub | `docs/design/139-mesh-durability.md:636-654` |
| The offline card exists and does D2's Continue here | `crates/desktop/ui/src/components/chat/OwnerOfflineCard.tsx:9-61`; `crates/desktop/ui/src/components/chat/Composer.tsx:1161` |

### 2.5 The moved notice

| Fact | Where |
|---|---|
| Idle take or move: `resume_on_take` returns `NoOpenTurn`; no row, no input | `crates/sessionsync/src/owner.rs:202-208` |
| Mid-turn resume: `<engine_restart at after kind from replayed interrupted/>`, `from` = engine id | `crates/agent/src/loop_.rs:8568-8585` |
| No UI renders `engine_restart`: it shows as raw XML on a tool card | `crates/desktop/ui/src/lib/injected.ts:22-57` (no `engine_restart` kind) |
| The wrapper parity test finds only tags with a closing `</name>`; `<engine_restart/>` and `<time/>` are self-closing | `crates/desktop/ui/src/lib/injected-wrappers.test.ts:41-54` |
| The TUI twin is `injected()` in the model, not a module | `crates/tui/src/model.rs:1175`, `:1516` |
| Typed-row precedent: `HistoryEntry::Advisory`, a user row with typed metadata that never opens a turn | `crates/agent/src/lib.rs:3018-3029`; `crates/agent/migrations/0034_advisor.sql:16` |
| The prompt header has persona, working directory and date; no machine or OS | `crates/agent/src/frame.rs:451-457` |

### 2.6 Per-session settings

| Fact | Where |
|---|---|
| Approvals bypass is a memory `HashSet` in the registry | `crates/tools/src/registry.rs:60`, `:776-803` |
| A session can only opt out: `!self.approvals || bypassed(s)` → Allow | `crates/tools/src/registry.rs:987-991` |
| The cap check runs first and denies regardless of approvals | `crates/tools/src/registry.rs:980-982` |
| Communication opt-out is a memory `HashSet` in `Policy` | `crates/agent/src/communication.rs:942`, `:1258-1270` |
| Model and effort persist on the row (replicated), resolved against the owner's catalog | `crates/agent/src/supervisor.rs:8682`, `:8713`, `:8751` |
| No per-session approvals getter (RPC or UI) | `crates/desktop/ui/src/components/settings/Approvals.tsx:13-18` |
| Approval timeout 300 s, then deny | `crates/tools/src/lib.rs:495`, `:531` |

### 2.7 New chat

| Fact | Where |
|---|---|
| `CreateSessionOptions` has no engine field | `crates/agent/src/lifecycle.rs:23-70` |
| `EngineStats{id, uptime_since_ms, class, availability_30d, harnesses, repos, display, gpu, accelerator}`: no os, arch or host | `crates/mesh/src/types.rs:113-138` |
| `engine_stats` is a replicated `mesh.sqlite` record; only `rpc/src/acp.rs` reads it | `crates/mesh/migrations/0001_mesh.sql:13-23`; `crates/rpc/src/acp.rs:281` |
| The only node picker is for tasks | `crates/desktop/ui/src/components/tasks/NewTaskSheet.tsx:131-137` |

## 3. Goals, non-goals, terms

### 3.1 Goals

| Ask | Goal (observable) |
|---|---|
| 6 | Typing, approving, steering, cancelling or changing a setting in a chat owned by B, from A or from the phone, runs on B; `sessions.owner` and `epoch` are unchanged |
| 6 | B offline: the OwnerOfflineCard appears; nothing runs until Continue here |
| 3 | A focused remote chat streams deltas, tool progress, approvals and asks within one relay hop; the sidebar shows remote sessions running |
| 7 | ⋯ menu on a remote chat toggles Approvals and Agent messaging and picks Model and Effort, on the owner; the value survives restart and move |
| 5 | Every move or take leaves one MovedCard in the transcript and one `<session_moved/>` the model reads; `<engine_restart/>` renders as a card |
| 4 | New chat on [machine ▾] creates the session owned by that machine, in a folder that exists there |
| 2 | Two sessions on two machines of one mesh, both left at defaults, exchange `send_message … await_reply` |
| 1 | Settings › Network shows each machine's path (LAN/relay/offline) and latency; Nearby pairs with a code |
| 1, D5 | Two separate harnesses pair as contacts over LAN or relay; a contact reaches only what is exported, under a cap |

### 3.2 Non-goals

- **Remote terminals and live PTY bytes** (W-2). `subscribe_pty` and `cell_attach` are jsonrpsee subscriptions
  (`crates/rpc/src/api.rs:317`, `:2328`); `theuth/rpc/1` is one request line and one response line
  (`crates/rpc/src/forward.rs:298-355`). Carrying streams needs a second stream proxy with its own back-pressure
  and auth; the replicated `pty_screens` already show the last screen. Revisit with design 68 option 3.
- **Merging two meshes across owners.** A contact never joins `peers`, never gets the vault key, never replicates.
- **A second forwarder.** Phase 1 and 2 reuse `forward::to_peer` and `communication::mesh::forward`.
- **Queueing agent messages for an offline owner** (W-15).
- **Vault key rotation on unpair.** Named as a gap in §8.4, not built here.
- **Replacing HTTP A2A.** It stays for non-theuth agents.

### 3.3 Terms

| Term | Meaning |
|---|---|
| owner | The engine in `sessions.owner`; the only writer and the only runner of the session's actor |
| viewer | Any other engine, or a device on the hub, showing the session from its replica |
| family (mesh) | Engines in one `peers` set: full trust, shared vault, replication, full RPC |
| contact | A separate harness paired by the §7 ceremony: local row, A2A only, capped |
| path | How a peer is reached now: `lan` (direct), `relay` (iroh relay), `offline` |
| forward | One unary JSON-RPC call over `theuth/rpc/1` to the owner, one hop, marked `Forwarded` |
| take / move | An ownership change: `session_take(user)` (Continue here) or `session_move` (Move to) |
