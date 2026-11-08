MeshMsgRecon: send_message / discover_peers across mesh engines. main 60c86602 (v2.18.12). Read-only. No shell here, so code, docs, config and logs only; nothing run or built.
Paths: bare names are crates/agent/src/ (mesh.rs = communication/mesh.rs); api.rs, forward.rs = crates/rpc/src/; ledger.rs = crates/tasks/src/; other crates as written; docs = docs/design/; config.yaml = ~/.theuth/config.yaml.

VERDICTS
- Same machine, session to session: WORKS (isolated-scope roots excepted, gap 6). Both ends enabled; same tree or both roots scope=owner (communication.rs:1134-1155). Desktop and desk roots are owner (supervisor.rs:6627; roster/src/lib.rs:865-874).
- Across the mesh to a session owned by the other engine: PARTIAL. Built as "69 P" on Sky's 2026-09-25 ruling (docs/design/69-as-built-P.md:3), unit-tested (supervisor.rs:15441,15522,15573),
  smoke-proven once on one Mac (69-smoke.md:95-96: await_reply "42" in 3.3 s; dead owner gives failed in 32.3 s). NOT seen live on A<->xeus: this laptop is not in that mesh.
- Across the mesh to an agent/desk: PARTIAL, weaker. A desk is an owner-scope root, so `session:<desk id>` delivers like the case above. No name or agent address exists, desks list as 8-char ids, a cold or paused desk may not act (gaps 4, 5).
  By-name reach exists only through task comments and @mentions, worked by the task's home engine (tasks/ledger.rs:1916-1919, 371-376).

HOW IT WORKS NOW (Q1, Q2, Q4)
- Resolve (supervisor.rs:1833-1879): `session:<uuid>` must be a stored row; a replicated row counts.
  (a) local: Target::Local, Policy::route (5657), accept, enqueue_peer (5754-5777).
  (b) owned elsewhere: same resolve; after accept, remote_owner (5531-5534, 5749-5753) calls forward_peer (5552-5601), then mesh::forward (communication/mesh.rs:89-149): one JSON-RPC `peer_deliver` line on theuth/rpc/1, paired peers only (mesh/src/net.rs:1223-1248).
  Owner side: rpc/forward.rs:360-383, api.rs:3876-3891 (Forwarded-only), Agents::peer_deliver (5488-5526). It refuses a row it does not own (never a take, never a second hop), then runs ITS OWN Policy::route.
  (c) row not in this store (not replicated yet, a Private session which never replicates (site mesh.mdx:53), or unpaired): `unknown peer` (supervisor.rs:1834,1851); nothing is sent.
- Replies: sender mints the message id, owner keeps it (store/communication.rs:597-611), so reply_to matches. The reply is itself a forwarded Deliver (5749-5753). The await_reply waiter lives on the sender engine (5580-5583, 5709-5746).
  Receipts are engine-local (store/capture.rs:3-5): sender sees accepted, then replied; consumed/suppressed never come back (69-as-built-P.md:45).
- discover_peers (6089-6237): every row in this engine's store, replicated included, each passing Policy::route. Owned-elsewhere rows: state `remote`, `engine=<mesh name>`, no activity, dropped by active_only (6166-6201, 6295-6298).
  Relations: parent, child, sibling, ancestor, descendant, root (communication.rs:265-280); remote-parent row of a node child (6232-6234); A2A `peer:` rows (6235). Other roots show an alias or 8-char id and no title (6142-6151, 6181).
- Other paths: node child to parent = ACP `_theuth/peer_message` notification, fire-and-forget, no await_reply (acp/src/peer.rs:13; rpc/src/acp.rs:859-880; acp_child.rs:712-741; supervisor.rs:1854-1858, 5430-5464). NOT the only path:
  child results to a parent owned elsewhere use forward_peer (2455-2471); job/reminder/child notices use the durable harness outbox on theuth/sessions/1, 7-day TTL (1621-1675; sessionsync/src/sync.rs:367-452; config.rs:16-19);
  human typing into a remote user session TAKES it, desks are forwarded (rpc/forward.rs:1-14); tasks and wakes replicate, only the home engine works them (tasks/ledger.rs:371-376, 414-425, 2045-2050).

GATES (Q3)
- Sender engine: its own `agent.communication: true` (live here, config.yaml:28); no `/communication off` on the session, an ancestor or its root (inherited, memory-only; communication.rs:1088-1109, 1254-1270); both roots owner or same tree;
  send_message is Mutating so it asks (peers.rs:392) unless tools.approvals=false (live false, config.yaml:41); owner connected now (mesh.rs:99-101).
- Receiver engine: YES, checked. peer_deliver runs deliver, which runs the owner's route (supervisor.rs:5511-5524, 5657): ITS `agent.communication` (own config.yaml, never mirrored, replica/src/files.rs:564-575; missing = off, design 50:231-235),
  recipient opt-out, scopes from replicated sessions.scope (store/communication.rs:880-917), pause (supervisor.rs:2069-2074), chain, quota. Owner-global-off returns `denied: ...communication_disabled...` (mesh.rs:68-70) naming the SENDER's session, not the engine (supervisor.rs:1903-1905); recipient opt-out or scope reads `unknown peer` (1909-1911).
- NOT needed: `a2a.enabled`. It gates only `peer:<name>` (communication.rs:1159-1169; supervisor.rs:5626-5653; default false, a2a/src/config.rs:40); that is why `peer:<agent>` failed on 2026-10-05.
- Trust and limits: pairing is the only gate; any paired engine may send as ANY `from` (api.rs:3883-3890 drops the Forwarded peer id). Chain 256 per session since last external input (communication.rs:96; continuation.rs:381-392) is inherited only from a LOCAL sender (supervisor.rs:2060-2068); the forward path skips it (5749-5753).
  Also 64 pending per recipient, 1024 per engine (store/communication.rs:668-692), 32 waits per session (supervisor.rs:5664-5669), 32 KiB, 16 recipients per call, 30 s forward timeout (mesh.rs:32), 120 s reply wait.

INTENT (Q5) AND LIVE STATE (Q6)
- Promised and built: "agents on different engines coordinate by send_message ... 'sessions everywhere' also means 'peers everywhere'" (69-as-built-P.md:3); invariant 11 "peers follow the owner" (69-devices.md:102); "both engines' communication policy must allow" (69-devices.md:105; 50-agent-communication.md:200-202).
  Never promised: queueing, naming, or skipping the owner-scope rule (50:253-256). Public docs (site mesh.mdx:53-62, delegation.mdx) never mention cross-engine send_message.
- Live: laptop config has communication true (config.yaml:28), no a2a/replica/sessions sections, mesh hub prefer (:70-71). mesh_peers() lists only this engine (c2d88a8b). Logs Sep-Nov: no pairing, peer_deliver or "peer message not forwarded" lines. A and xeus (paired 2026-11-07) unreadable from here; mesh.sqlite not queried.

GAPS AGAINST SKY'S INTENT
1. Owner offline: `failed {"OwnerUnreachable"}` at once, or after 30 s if connected but silent. Nothing queued or retried; the same request_id returns the stale failed receipt (store/communication.rs:622-635; supervisor.rs:5699-5705). The harness outbox (1621-1675) shows the queue pattern exists.
2. Deliver runs on the Agents mailbox: the cross-engine leg (up to 30 s, mesh.rs:129-138) stalls the whole supervisor, then the failure path asks Mesh Status inline (5586-5588; mesh.rs:169-183), whose `COUNT(*) FROM outbox` took 1-4 s in this laptop's log (theuth.log.2026-11-08:580).
   Two engines sending to each other at once could hold each other 30 s. By code reading, not reproduced (supervisor.rs:5467-5473, 5571-5577; 176-live-under-load.md:18,179).
3. The other engine's flag is per engine, default off, and the refusal does not name the engine (above).
4. Names: desks get no alias (roster/src/lib.rs:862-900) so 8-char ids (supervisor.rs:6142-6151); `query` matches name and address only, not tags or title (6019-6021); no reachable flag on remote rows (6166-6201).
5. Cold or paused desks: input can be held (supervisor.rs:2069-2115) or wait for the next turn (knowledge facts/theuth-cold-desk-peer-message-queued-until-turn-2026-10-06); an idle native session does start a turn (loop_.rs:1673-1714). Unverified which wins today.
6. Isolated-scope roots (guest Slack/Telegram, t --acp, some forks) cannot talk across roots, on one machine too (communication.rs:1145-1150).
7. A move or take between send and reply orphans the exchange, as peer_* tables are engine-local ("unknown message", supervisor.rs:5671-5681). By reading, Policy ignores replicated scope or alias changes (apply_lineage 1118-1149 returns early for known roots; only local set_scope 5957 updates).

SMALLEST CHANGE (the forwarder exists; do not build a second one)
1. Forwarder: keep forward_peer + mesh::forward on theuth/rpc/1. Move the network leg off the mailbox: register the waiter first (5580-5583 relies on mailbox order), spawn, post the outcome back, Deliver replies via DelegatedReply as 176 E1 did, and stop asking Mesh Status inline for last_seen (cache it from peer events).
2. Resolution: keep `session:<uuid>`. Alias = agent name on desk roots (roster/src/lib.rs:891-899, SetPeerScope); add `reachable` to remote rows from the mesh::Status ask discover_peers already makes (mesh.rs:185-201; mesh/src/types.rs:102); `query` matches tags and engine; owner-side refusal says "communication is off on <engine>".
3. Replies: no change. Optionally ship consumed/suppressed back on the same rpc stream.
4. Owner offline: keep fail-fast; make a same-request_id retry re-forward a failed OwnerUnreachable row. Durable retry on reconnect (harness-outbox shape and TTL, excluding await_reply) only with Sky's ruling.
5. Guards: add `chain` to PeerEnvelope (communication.rs:580-591), honor it in enqueue_peer; require Forwarded peer == owner of envelope.from (api.rs:3883-3890); keep one hop, 32 KiB, 64 pending, 32 waits, 16 recipients, 30 s; refresh Policy on replicated scope change; document in site mesh.mdx.
6. Live check for Sky on A: discover_peers shows `remote ... engine=xeus`; send_message(session:<xeus id>, await_reply). `denied ...communication_disabled` = xeus flag off; `unknown peer` = scope or replication; `OwnerUnreachable` = xeus offline.

OPEN QUESTIONS FOR SKY
1. Owner offline: fail fast (today) or queue with a TTL? Should await_reply ever queue? 2. Should agent.communication be mesh-wide (replicated) or per engine? 3. May any paired engine send as any session (today) or only sessions it owns?
4. Desk addressing: alias on session rows, or a new `desk:<agent>@<engine>` address? 5. After takeover_secs offline, may a model message take the session like a human write (TakeReason::Write), or never (today)? 6. Isolated roots stay unreachable across the mesh?