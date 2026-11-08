SESSIONSTREAMRECON: Q1–Q4, read-only, main 60c86602 (v2.18.12). I changed nothing and ran nothing. Everything below comes from reading the code. Items marked "inferred" are not confirmed live. Paths are relative to crates/ unless they start with docs/.

HEADLINE
- The live relay of a remote owner's events is already built (sessionsync/src/events.rs). Nothing ever turns it on. The desktop UI opens one global event stream, and the relay only starts on a per-session stream.
- A phone paired to the mesh has the same problem. It talks to the hub engine, and a send on a session owned by another engine takes the session to the hub.

Q1. WHY A B-OWNED SESSION DOES NOT UPDATE LIVE ON A

(a) What replicates, and when
- Whole rows, not metadata only. Each committed write transaction becomes one session_log row per session. That row is a SQLite-session changeset of the row diffs (agent/src/store.rs:190-283). The 20 attached tables include sessions, messages, epochs, turn_steps, tool_calls, jobs and pty_screens (agent/src/store/capture.rs:19-40).
- Granularity is per commit (user row, assistant row, tool row, journal step). It is not per turn.
- Token deltas, tool progress and the running flag are bus events. They are never persisted.
- The one mid-stream trace is the assistant_partial row. It is upserted into turn_steps every partial_checkpoint_ms = 5000 (agent/src/durability/mod.rs:33,54; store/durability.rs:555-590; loop_.rs:5132-5171).
- Channel: a dedicated ALPN, theuth/sessions/1 (replica/src/lib.rs:46; mesh/src/net.rs:344-347). The lower id dials (sessionsync/src/ship.rs:56-62). It is not the record replica and not mesh RPC.
- Latency: no debounce and no interval. The commit wakes a watch (store.rs:279-281), and sync.rs:333-336 ships at once. Catch-up batches are at most 5000 rows or 32 MiB (sync.rs:33-35). A gap waits 10 s (sync.rs:39). Redial backoff is 1→60 s (ship.rs:20-21,72-84). The 68 smoke measured +127 ms per append on a LAN (docs/design/68-sessions.md:160).
- Not replicated:
  - the python heap, a local file at <data_dir>/runtime/<session>.monty (agent/src/repl.rs:6-7,1100-1106);
  - the per-session approvals bypass (tools/src/registry.rs:786-789);
  - the communication opt-out, "memory-only" (agent/src/communication.rs:1254-1270);
  - the cwd seat (tools/src/seat.rs:6-11);
  - running jobs and PTYs, which stay on the origin (their last screen does replicate).

(b) Viewing side
- A's UI subscribes once: App.tsx:257 `client.subscribe(null, null, …)`. That is the only call site (ui/src/rpc/client.ts:126-132). set_event_focus (App.tsx:560-579; rpc/src/api.rs:9478-9500) only filters deltas.
- A has no actor for B's session, so replication is all it emits:
  - SessionOwnerChanged when the owner row first appears or changes (sessionsync/src/sync.rs:185-195,645-651,724-731).
  - ContextChanged{reason:"replicated"} per applied batch (sync.rs:645-662).
- The UI re-queries on those, but only partly:
  - SessionOwnerChanged patches the cached rows and does a debounced list refetch (lib/rpc.ts:233-238).
  - ContextChanged refreshes search only (rpc.ts:216), not the sidebar.
  - For the open chat, the reducer marks the slice stale and settle() re-reads load_session(settled_only) (store/reduce.ts:890-902; routes/c.$session.tsx:255-287,322-324). Chat updates at commit granularity.
  - The reducer drops a ContextChanged whose revision is unchanged (reduce.ts:891). Partial upserts don't bump context_revision (store/durability.rs:563-588 against store.rs:3623-3625). So the 5 s partials replicate but never redraw (inferred).
- On a replica, load_session reports running:false, settled:true, editable:true (agent/src/supervisor.rs:7429-7439,7539-7546). The only hint is the open turn's "Running on <owner>" banner (supervisor/drain.rs:297-376; ResumeBanner.tsx:33-40).
- Bus: rt.publish (sync.rs:181) feeds the RPC bridge (rpc/src/bridge.rs:216-254), the event ring, then subscribe_events.
- A live relay of B's events exists. The owner serves Msg::Events streams (events.rs:249-334). The viewer's follower() re-publishes them on A's bus (events.rs:346-412).
- It starts only at rpc/src/api.rs:9351, `session.and_then(sessionsync::events::follow)`. `session` is None for the desktop's stream, and follow() has no other caller.

(c) What Sky saw at pairing
- It was backfill, not streaming. A new peer has no cursor, so it sends Hello (sync.rs:244-266). The owner sends a snapshot (sync.rs:272-285, 557-574). receive_snapshot then emits SessionOwnerChanged for every session (sync.rs:724-731), and the UI refetches list_sessions (lib/rpc.ts:233-238). The sidebar fills in one burst.
- The same event fires once when a new session appears on the other engine (sync.rs:645-651).
- Steady state is per-commit rows with no list refresh, no running glyph and no deltas.

(d) Exact reason
Replication ships only committed rows (sync.rs:333-336). B's in-flight turn exists only on B's bus: deltas, tool progress, running state and pending approvals. The engine feature built to carry that bus across the mesh (events.rs, 68 §5) is wired to a per-session subscribe_events (api.rs:9351). The desktop and phone open one global subscribe_events(null,null) (App.tsx:257). So no Events stream is ever opened. The owner side returns early at events.rs:271-277, and the viewer follower (events.rs:389-398) never runs. A gets only row-level ContextChanged (sync.rs:652). That reloads the open chat per commit, ignores 5 s partials, and never refreshes the sidebar or the running state.
On Sky's 2.18.8 pair there was a second problem. Before b6a1cfc4 (after 2.18.9), merely opening the chat on a non-owner took it. The cell reads ran as writes, so a stable viewer could not exist.

Design options
1. Follow-on-focus (recommended, about 20 lines). In set_event_focus (api.rs:9478), hold events::follow(s) for each focused session owned elsewhere. Keep the guard in the per-connection state and drop it on unfocus or close. The follower already handles owner moves and reconnects (events.rs:254-270,363-378).
   - Widen the relay. It subscribes to AgentEvent only (events.rs:144). Approvals, asks, JobEvent and CellEvent are separate bus types (bridge.rs:216-254) and need adding.
   - Trade-offs:
     - Bandwidth is only for focused sessions. ToolCallProgress is already coalesced to one per 250 ms (agent/src/lib.rs:3140-3143).
     - The stream is live-only, with no replay buffer (events.rs:21-23). Reconnect and offline catch-up come from the replicated rows plus settle(), and the rows stay the truth.
     - Event seq is per process and rows are ordered by log seq, so the UI must settle on TurnFinished or ContextChanged.
     - It fits take and release. SessionOwnerChanged ends or restarts the follower, and a take just makes the stream local.
2. Rows-only liveness. Redraw on every replicated batch, and render the 5 s partial as streaming text. There is no new transport and offline catch-up works. But it moves in 5 s steps with no tool progress, running state or approvals. It also re-renders the whole page per tick (supervisor.rs:7440).
3. Viewer attaches straight to the owner's engine. See Q4.
Recommend 1, with the redraw fix from 2 as the offline fallback.

(e) Prior intent
- docs/design/68-sessions.md:78 says reads, including subscribe_events, are "served from the local replica".
- 68-sessions.md:81 says forwarded desk events are streamed "back onto its own bus, so a device paired here sees the turn live".
- 68-sessions.md:82 says "Events: the owner publishes session events for sessions with remote subscribers over theuth/sessions/1 (Events{session, from_seq} stream…) … the stream exists only for liveness of the UI."
- docs/design/68-as-built-F.md:37 says "subscribe_events holds a sessionsync::events::follow(session) guard … (remote owner's events flow onto this bus while a client watches)".
- The UI was written expecting it: store/reduce.ts:746, store/reduce.test.ts:1223 and docs/design/139-as-built-W4.md:30 ("a relayed TurnResumed").

Q2. START A SESSION ON A CHOSEN NODE

(f) What exists
- No UI for it. New chat has no node choice (ui/src/components/chat/NewChatPane.tsx, routes/new.tsx, ⇧⌘O at lib/shortcuts.ts:44). CreateSessionOptions has no engine field (agent/src/lifecycle.rs:23-70). A session is created on the engine the client is attached to (docs/design/68-sessions.md:56-57).
- The engine switcher (components/link/EngineSwitcher.tsx:62-137, hidden with only the local daemon at :72) lists local, link-engine rows, or one mesh row whose hub answers (link/engines.ts:12-29).
  - Members are only counted ("2 of 3 nodes online", :50-54), never listed.
  - One engine is active per window, and events attach for it only (App.tsx:329).
  - Settings › Mesh (settings/Mesh.tsx) manages peers and hub, with no start-session action. The search palette (⌘K) has no node entry either.
- Node pickers exist for tasks only:
  - NewTaskSheet.tsx:131-137 ("Where", built from nodesOf(mesh_status) at tasks/helpers.ts:16-17, passed as task_create where);
  - Board.tsx:208;
  - tasks/MoveHome.tsx (design 182).
- Engine side: spawn_subagent{agent:"<harness>@<engine>"|"<engine>"} makes a child session owned by that node (agent/src/peers.rs:140-168; docs/design/67-agents.md:405-407). Only the model can call it. cells.rs:496-519 cell_interpreters(engine) is the one engine-addressed RPC.
- Today the choices are:
  - create locally, then ⋯ Move to ▸ (SessionMenu.tsx:262-269,355-371);
  - ask the agent to spawn_subagent on the node;
  - open that node's own UI or engine row;
  - give a task Where=node.

(g) What a "New chat on ▸ node" picker needs
- RPC: create_session and send_input(None) take an engine and go to the owner via forward::to_peer (rpc/src/forward.rs:261-296). The owner is B from the start. The row then replicates back (sync.rs:645-651).
- Picker data: mesh_status gives name and reachable only (mesh/src/types.rs:225-246). EngineStats carries class, harnesses, repos, display and gpu (types.rs:110-138). It has no os or arch, although docs/design/67-agents.md:405 promised them. No RPC exposes it (only rpc/src/acp.rs:281 reads it). Add peer stats to an RPC.
- cwd and project: paths are machine-local, so offer B's repos (EngineStats.repos) and validate them on B. Project labels replicate.
- Model and harness: set_model resolves against the engine's own catalog (supervisor.rs:8682), and provider keys are per engine. Take the lists from B (a forwarded list_models, and EngineStats.harnesses).
- UI: an "on <node> ▾" chip in the NewChatPane header (default this node, offline nodes disabled, like the Move to rows), a "New chat on ▸" submenu on the sidebar button, and a palette entry. The phone uses the same React app.

Q3. TELLING THE MODEL ABOUT A MOVE

(h) What the model sees after a take or move
- Idle session (Continue here, /take, Move to ▸): nothing.
  - Take::take (sessionsync/src/owner.rs:111-190) does set_owner, publishes SessionOwnerChanged{moved_from}, rearms reminders and calls resume_on_take. That returns NoOpenTurn when no turn is open (owner.rs:202-208).
  - There is no transcript row and no injected input. The UI shows only the chip and the banner (ResumeBanner.tsx:12-17).
- System prompt: the header is persona, "Working directory: {cwd}" and "Date" (agent/src/frame.rs:451-457). It has no OS, hostname or engine name. It is rebuilt each turn from the taker's own prompt.md, skills dir, plugin blocks and tool list (frame.rs:464-492; loop_.rs:4841-4869). Tools and skills can differ silently.
- Mid-turn move that resumes: the model gets <engine_restart at= after= kind="transfer|takeover" from="<engine id>" replayed= interrupted=/>, prepended to the first tool response (loop_.rs:8564-8585; run_batch 6625-6640).
  - Cut calls return "ERROR: interrupted: the session moved to this engine at HH:MM:SS while this call was running…" (loop_/resume_calls.rs:23-44).
  - Jobs get "the session moved here from <engine>…" plus the origin's last PTY screen (resume_calls.rs:116-200; durability/jobs.rs:274).
  - With resume set to ask, a local, unreplicated System note says "taken from X at HH:MM while a turn was running; press Resume…" (owner.rs:229-238; store.rs:970-978).
- UI: engine_restart has no renderer (grep of ui/src finds none), so it shows as raw XML at the top of a tool card. The wrapper-parity test cannot catch it. It only finds tags that have a closing tag (lib/injected-wrappers.test.ts:41-54), and <engine_restart/> and <time/> are self-closing.

(i) What changes under the model
- cwd: the seat is a per-process map. An unseated plain chat uses the taker's process cwd (tools/src/seat.rs:84-89; loop_.rs:3668). The transcript keeps the origin's paths, with no check and no notice.
- OS, tools, harnesses, MCP servers, skills and models are all engine-local.
- The python heap is a local file, so the taker starts with an empty runtime and no <runtime> note (repl.rs:6-7,1100-1106).
- Approvals bypass and communication opt-out reset to the taker's defaults (registry.rs:786-789; communication.rs:1254-1270).
- Jobs and PTYs stay on the origin and are settled as lost on the taker (durability/jobs.rs:94-130).
- Child results are forwarded to the new owner with "(produced on X after the session moved to Y)" (supervisor.rs:1637-1664,2455-2471).
- Secrets and the vault are per engine (not verified).

(j) How notices reach the model and the UI, and what a "moved" notice needs
- Model side: wrapper text is built at the call site and injected by Agents::inject_as (supervisor.rs:1637-1664). Cross-engine delivery rides harness_outbox (sync.rs:313-322). A <time now=…/> stamp is prepended to inputs.
- Engine notes (todo_continue, rounds, context_pressure, recovery, continue, steer, surface_error, todo, knowledge, vision, nudge, journal, budget) are listed in lib/injected.ts:4-19.
- A typed precedent exists. Advisory and vision rows carry typed metadata beside the model text and never open a turn (agent/src/lib.rs:3018-3025).
- UI side:
  - lib/injected.ts classify() (:252) regex-parses the wrapper into card kinds (Injected union, :22-60).
  - lib/time.ts:48-57 strips <time/>.
  - Message.tsx:50-62,277 switches on the kind to components/chat/HarnessCards.tsx (DeskCard, WakeCard, ResumeCard :130, ChildLostCard, ChildStateCard, AutoresearchCard).
  - store/reduce.ts:935-941 keeps wrapped rows out of the "you" bubbles.
  - tui::model::injected mirrors it.
- A "Session moved to <node> · <os> · <cwd>" notice needs the following.
  - Engine: a typed replicated user-role row, written by the taker right after set_owner (owner.rs:164-190). It must never wake or open a turn. Attributes: to, from, os, arch, cwd, at, reason (user|moved|write|orphan), jobs_left, runtime=reset, approvals=default. The model text is a matching <session_moved …/>.
  - Mid-turn resume: add to=, os= and cwd= to <engine_restart> rather than writing a second note.
  - UI: a new Injected/Entry kind "moved" in injected.ts and Message.tsx, plus a MovedCard in HarnessCards.tsx. Also render engine_restart as a card. Extend the parity test to self-closing tags, and add the TUI twin. Keep ResumeBanner as the ephemeral line.
  - Cases:
    - idle take or move writes the row;
    - mid-turn auto-resume writes the row plus the restart note;
    - policy-ask takeover writes a row with pending=true, replacing the local note (owner.rs:229-238);
    - the old owner keeps its local orphan note (owner.rs:517-536).

Q4. REMOTE OPERATION WITHOUT A MOVE

(k) Take-on-send today
- Every session-mutating RPC calls forward::route. There are 37 call sites in rpc/src/api.rs: send_input 3812, steer 3849, withdraw, cancel, compact, undo/redo, endless_* 4269-4377, remove_context, regenerate, edit_turn, rename/pin/archive/delete/set_project 4834-4942, set_model 5096, set_effort 5126, harness options, mcp_hidden 5307, set_approvals 5330, set_communication 5354, set_peer_scope, answer_peer_request, use_skill 5508, knowledge_harvest 6585, resume_turn 8945. Cell run/stop/input take too (rpc/src/cells.rs:79-107).
- forward::classify returns Take for an owner elsewhere and a user session. That sends TakeSession{Write, by_user:true} (forward.rs:182-195,221-247). Desk sessions are forwarded instead. This is 68 ruling 5, "user sessions follow you" (docs/design/68-sessions.md:13).
- Does not take:
  - approval and ask answers, which are forward-only (api.rs:3202-3220,5000-5036);
  - reads: load_session, list_*, and cell_state/cell_runs since b6a1cfc4 (cells.rs:109-147);
  - path_complete (api.rs:4633), answer_secret (api.rs:9160), and fork_session.
- The approval card itself is not visible on A. PendingPrompts::collect is local (api.rs:2932-2960) and the relay carries AgentEvent only. Answering from A is plumbed, but there is nothing to click (inferred).

(l) How pure UI clients work
- iPhone, iPad and browser run the same React app: RpcClient (ui/src/rpc/client.ts) over LinkTransport (link/transport.ts). The path is a relay websocket at /h/<id> with a Noise IK handshake, then the same Api trait and the same subscribe_events(null,null). Pairing is a QR string with a PSK that expires in 5 min (link/src/lib.rs:1-20). Device rows replicate across the mesh (link/src/devices.rs:1-8).
- Devices pair to the mesh, not to an engine. Only the hub registers the mesh identity at the relay and serves them (docs/design/69-devices.md:9-15; link/src/mesh_uplink.rs:1-14).
- A phone's send on a session owned by a non-hub engine takes it to the hub (docs/design/139-mesh-durability.md:636-654; forward.rs:221-247). So today a phone is no better than A's UI. Sky's premise holds only for hub-owned sessions, or for a device paired directly to that engine.
- Desktop: the local daemon is a loopback websocket plus a bearer token (rpc/src/lib.rs:62; rpc/src/client.rs:196). Other engines go through link rows. There is one active engine per window (App.tsx:275-334).
- Mesh peers do not expose their link ids or relay. B's RPC is loopback by default, so there is no direct LAN path.
- A cannot proxy generic RPC to B. Only the routed methods ride theuth/rpc/1, and that path is unary (forward.rs:256-296; api.rs:3471).

(m) Design
- The one trust anchor: the owner dispatches forwarded calls as the internal peer, one hop only (forward.rs:360-376). Any paired engine acts as owner console. That is acceptable inside one user's mesh (68 decision 6).
- Option 1, recommended: A's engine proxies.
  - Change classify so a user session with a connected owner returns Forward(owner), for every routed method except explicit take/move.
  - Add follow-on-focus and the widened relay from Q1.
  - Forward reads of owner-memory state (approvals, communication, jobs) like cells' read_scope.
  - Attachments already travel by name and the owner fetches them (forward.rs:16-20,101-165).
  - Approvals: the owner's policy and bypass apply. Cards reach A through the widened relay. Answers already forward (api.rs:3202).
  - Streaming RPCs (subscribe_pty, cell_attach) cannot ride the unary forward. Terminals and cells stay owner-local: "runs on <node> — open there / Continue here".
  - B offline: -32010 triggers OwnerOfflineCard, "Continue here now?" (components/chat/OwnerOfflineCard.tsx:9-61). Reads keep working from the replica.
  - Stays deliberate: Continue here, Move to ▸, Resume-here, orphan takeover.
  - This reverses 68 ruling 5, so it needs Sky's OK.
- Option 2: the desktop UI opens a second RPC connection to B.
  - It is the literal "pure client" and covers PTYs and cells.
  - It needs B's link identity and relay row per member (not discoverable), the desktop registered as B's device, and per-session api and event routing (today one engine per window). It is relay-only and a large UI refactor.
- Option 3: A tunnels the whole JSON-RPC socket to B over iroh (new ALPN, UI picks the engine per session).
  - It gives streams, new-chat-on-node and settings. It needs a generic proxy and a per-connection peer identity on B. It is bigger than option 1.
- Recommend option 1 now. Revisit option 3 only if remote terminals are wanted.

(n) Per-session settings in the ⋯ menu
- The ⋯ menu (SessionMenu.tsx:334-401) holds rename, Continue here / Move to ▸, compact, clear, shake, undo/redo, harvest, show prompt, jobs, and "Communication & peers…" (opens Settings).
- Approvals, model and effort are not in it. They live in Settings › Approvals (settings/Approvals.tsx:18), /approvals (slash.ts:451) and the header ModelMenu (ModelMenu.tsx:175,185).
- Approvals: stored in memory in the owner's registry bypass set (tools/src/registry.rs:776-803). Communication is memory-only too (communication.rs:1254-1270). Neither persists, replicates, or survives a restart or a move.
- Model and effort: persisted on the session row, which replicates (supervisor.rs:8713,8751). They are applied to the live actor and resolved against the owner's catalog (:8682).
- Applier and policy: the executing owner applies the settings, under its own tools.policy and tools.approvals (tools/src/lib.rs:488-497). The owner's approval actor raises the cards, and its approval_timeout_secs denies if nobody answers (:494). So a cloud session with approvals on and nobody on that node gets denials.
- Toggled on a non-owner today: it takes the session (api.rs:5096,5126,5307,5330,5354,5369 route to forward.rs:221-247). It is neither refused nor replicated.

(o) Under the recommended design
- Forward each setter to the owner (Forward instead of Take). Also forward the matching readers (get_approvals, get_communication, list_models), because a replica cannot know memory-only flags.
- Persist the approvals bypass and the communication opt-out on the session row, so they survive moves and restarts.
- Show the restored value in the moved notice.

OPEN QUESTIONS FOR SKY
1. May sends, approvals and settings from another node forward to a reachable owner instead of taking it, which reverses 68 ruling 5? Take would stay for Continue here, Move to ▸ and offline-confirm.
2. If the owner is offline, keep the explicit "Continue here" card, or queue the message to the owner (the harness_outbox pattern)?
3. Should per-session approvals and communication persist on the session row? On a move, keep the origin's setting or reset to the taker's?
4. Live scope: only focused sessions, or also running glyphs and titles for every remote session?
5. Should the "moved" notice be a replicated card with model text including os and cwd, or UI-only?
6. For a new chat on a node: pick cwd from the node's advertised repos only, or allow a free path? Default model from the node?

Not verified live: the reducer dropping same-revision ContextChanged, A having no way to show B's approval card, vault and secrets behaviour on a move, and which engine is currently the hub on Sky's pair.