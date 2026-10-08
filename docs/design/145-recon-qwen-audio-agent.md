# recon-theirs — QwenAudio/qwen-audio-agent (HEAD 84490d46, v2.0.1, Apache-2.0)

Read-only recon, 2026-10-08, ~25 min budget. All paths relative to `/tmp/qwen-audio-agent`
(shallow clone, 1 commit, so git history comes from the GitHub API, not the tree).
Techniques only; excerpts ≤ 6 lines. Load-bearing docs: `docs/architecture/deep-dive.md`
(576 lines, "defines the product boundary"), `docs/gateway-protocol.md` (708 lines),
`docs/contract.md` (316 lines). Load-bearing code: `server/src/voice/**` (8.5k LOC),
`server/src/task/**` (2.5k), `server/src/orchestration/**`, `server/src/backend/**` (8.1k).

Headline in one paragraph: the product is a **cloud speech-to-speech ("Realtime") model that
talks**, a Node "Gateway" that owns every lifecycle (turns, playback, tasks, permissions,
delivery), and an **ACP coding agent that works** in the background. The voice model may not
pick sessions, execution mode or tools-of-the-backend; it gets six core tools and a 93-line
fixed prompt. The machinery worth studying is the Gateway's: a monotonic turn-generation
boundary, a server-side response slot, a "safe insertion window" for background results,
playback receipts (not `response.done`) as the proof the user heard something, renewable
claims so two clients never speak the same result, a reserved-envelope guard that catches a
model forging Gateway events, TTL-bounded microphone arbitration, and a sleep controller whose
`canSleep` is a conjunction of every "busy" predicate.

## 1. Architecture bet and layering

- **Three logical components, one process.** Frontend Agent (realtime model + instructions +
  tools), Orchestration Runtime (tasks, permissions, sessions, delivery — explicitly "without
  adding a separate reasoning Agent"), Backend Agent (one configured action agent). The Gateway
  "is not a fourth core component"; clients "own I/O and presentation" and "are not the
  Frontend Agent". `docs/architecture/deep-dive.md#L11-L23`.
- **Who talks / who works.** Final ASR → either answered directly by Realtime speech, or
  `spawn_thinking(objective)` → Task accepted → response returns immediately → owner FIFO →
  BackendPort → final presentation "waits for a safe duplex insertion window" → Realtime
  speaks it. `deep-dive.md#L33-L60`. `spawn_thinking` never waits; one Task per owner is in
  the backend at a time. `deep-dive.md#L58-L60`.
- **What the voice model may NOT decide**: selecting/creating/continuing/cancelling backend
  Sessions; sync vs async / foreground vs background; backend execution strategy; tools, agents
  or subagents. `deep-dive.md#L141-L146`. The only exception is relaying an explicit
  current-turn permission decision. `deep-dive.md#L148-L150`.
- **Backend is opaque.** The backend may use tools/skills/subagents/sessions internally — "those
  are backend-private implementation details and do not create additional layers".
  `deep-dive.md#L25-L31`. The backend receives one self-contained natural instruction and
  current-turn native attachments — no frontend persona, memory or chat history.
  `deep-dive.md#L173-L180`, `#L219-L222`.
- **Dependency direction.** Frontend → `spawn_thinking` → runtime → `BackendPort` → adapter;
  realtime vendor events "must not enter generic task logic"; transport projects runtime results
  into client events. `deep-dive.md#L370-L382`. Directory ownership by feature (`memory/`,
  `knowledge/`, `frontend/`, `voice/`, `orchestration/`, `backend/`, `conversation/`+`session/`),
  `app/` as composition root; dependency tests forbid a runtime importing its own provider
  implementation. `deep-dive.md#L384-L406`. Memory and knowledge are removable by deleting the
  directory and two composition entries; tests physically remove them. `deep-dive.md#L408-L415`.
- **Process ownership.** Backends are `owned` (Gateway starts/stops the ACP child) or `external`
  (connect to published address). `deep-dive.md#L496-L503`. Backend children receive only
  portable OS env vars plus the plugin's declared credential namespace; realtime/memory secrets
  never cross. `deep-dive.md#L516-L520`.
- **One state authority.** `TaskOperations` unifies submission/lookup/cancel/permission/input;
  `TaskManager` is the single state authority; `BackendWorkRuntime` only translates into
  BackendPort calls. `deep-dive.md#L417-L426`. Per connection a `SessionTaskCoordinator`
  observes the owner's tasks, coordinates pending permissions and claims final-result
  notifications; it "neither runs a model nor interprets protocol frames".
  `deep-dive.md#L428-L433`. Transport (`transport/gateway-client-transport.mjs`) does auth,
  capability negotiation, heartbeats, encoding; it hands each client to an independent
  `createRealtimeSessionRuntime` which owns model session, tool calls, audio turns, playback,
  recovery — "it does not own a socket, credentials or a protocol handshake".
  `deep-dive.md#L442-L449`.
- **Review checklist** the maintainers run before merge (can Realtime still converse while
  work runs; is completion spoken only from a final backend result; can interruption postpone
  speech without cancelling a Task). `deep-dive.md#L564-L575`.
- Clients (Desktop/TUI/WebUI/mobile) are "replaceable Gateway clients"; closing a UI cannot
  affect queued work or the fixed backend Session. `deep-dive.md#L549-L556`.

## 2. Voice I/O pipeline

- **No STT/TTS of their own.** Every provider is a speech-to-speech realtime socket:
  DashScope Qwen-Omni/Qwen-Audio, GPT-Live, Google Live, StepFun, Doubao Seeduplex, MiniCPM-o,
  and a generic "speech-to-speech" (Hugging Face s2s service, local-hostable).
  `server/src/voice/providers/` (15 files, listed via `ls`); `s2s.mjs#L42-L54`,
  `#L84-L88`.
- **Provider abstraction = a capability table**, not an interface hierarchy. Defaults in
  `server/src/voice/realtime-provider.mjs#L75-L100`: `acknowledgesSessionUpdate`,
  `singleResponseSlot`, `responseMetadataCorrelation`, `perResponseInstructions`,
  `sessionOutputVoice`, `conversationItemIdEcho`, `acknowledgesConversationItems`,
  `restoreConversationContext`, `conversationItems`. Each provider overrides what it cannot
  do (e.g. s2s has one response slot and does not ack `session.update`
  `providers/s2s.mjs#L42-L54`; several providers set `conversationItemIdEcho: false`
  `providers/google-live.mjs#L66`, `providers/stepfun.mjs#L31`, `providers/doubao-seeduplex.mjs#L37`).
  Without echoed ids "only one client-created item may await a receipt"
  `realtime-provider.mjs#L528-L529`.
- **VAD is the provider's.** `turn_detection: { type: 'server_vad', interrupt_response: true }`
  `providers/s2s.mjs#L86`; WebRTC clients may not change `turn_detection` and "manual audio
  commit is unsupported" `server/src/transport/webrtc/protocol.mjs#L82-L87`, `#L110`.
- **Echo handling is client-side only**: browser `getUserMedia({ echoCancellation: true,
  noiseSuppression: true, channelCount: 1 })` `shared/gateway/webrtc-browser.mjs#L153-L156`.
  No server-side AEC or playback-reference subtraction found (grep `echo` across `server/src`
  matches only "echo"-as-in-id-echo).
- **Audio format/framing.** PCM16 mono; input 16 kHz / output 24 kHz for DashScope, Google,
  MiniCPM-o, Doubao; GPT-Live 24/24. `providers/dashscope.mjs#L97-L98`,
  `providers/gpt-live.mjs#L40-L41`, `providers/google-live.mjs#L59-L60`,
  `providers/minicpm-o.mjs#L59-L60`, `providers/doubao-seeduplex.mjs#L30-L31`. Audio arrives from
  clients as base64 `audio.append` events; the Gateway forwards and also fans out to observers
  with the provider's `inputSampleRate` (default 16 000). `realtime-session-runtime.mjs#L1012-L1028`.
- **Fallbacks.** None across providers at runtime — one configured provider per Gateway; the
  Gateway "is the single owner of its active Realtime model" and model mismatches from a remote
  Gateway are rejected. `deep-dive.md#L549-L553`. Provider errors `content_safety` trigger a
  playback clear and a recovery path. `realtime-session-runtime.mjs#L637-L658`.
- **Timeline priming trick**: for providers that need audio before video, the Gateway sends
  20 ms of PCM16 silence so a camera frame need not open the microphone.
  `realtime-provider.mjs#L384-L392`, `docs/voice-frontends/custom-provider.md#L37`.

## 3. Turn-taking and duplex

- **Turn state machine = a monotonic generation counter.** `RealtimeTurnState` owns
  `turnGeneration`/`turnSequence`, `committedTurnGeneration`, `userSpeaking`, and
  `manualInputGeneration`; "Provider events, manual input and client interruption all advance
  the same generation boundary … prevents late provider events from reclaiming a newer turn".
  `server/src/voice/realtime-turn-state.mjs#L7-L14`. `beginVoice(itemId)` allocates a generation
  (or re-resolves a known item) `#L46-L66`; `beginManual()` supersedes an in-flight voice turn and
  commits immediately `#L72-L83`; `isStale()`/`commit()` refuse anything older than the committed
  generation `#L112-L127`; `advanceBoundary()` bumps and commits in one move `#L145-L149`.
- **Interruption** (client `interrupt` event): record activity, `turns.advanceBoundary()`,
  `announcementWindow.interrupt()`, `announcements.dismissActive()`,
  `realtimeSession.cancelResponse()`. `realtime-session-runtime.mjs#L1064-L1069`. Interrupting
  never cancels tasks: "Mute, voice interruption, sleep and frontend disconnection do not cancel
  accepted backend work". `deep-dive.md#L456-L460`.
- **Response slot.** `RealtimeResponseSlot` phases idle→pending→cancelling; "A server-reported
  response can occupy the slot before response.created. Never treat a local timeout as proof
  that the remote slot was released." `realtime-response-slot.mjs#L1-L3`. `occupy()` arms a
  `waitMs` watchdog; `recover()` sends cancel and, if not released within `cancelMs`, disconnects
  the provider socket. `#L18-L38`. CHANGELOG confirms: "reply occupancy waits for the server to
  release; after timeout cancel and confirm; if unconfirmable rebuild the connection".
  `CHANGELOG.md#L48-L49`.
- **"Safe insertion window"** is `AnnouncementWindow.isBlocked()` = `userSpeaking ||
  turnPending || audioResponses.size > 0`. `voice/announcement/announcement-window.mjs#L78-L84`.
  `turnPending` clears only when the *active* turn's response finishes without audio and without
  a pending tool follow-up, or when its audio playback finishes `#L20-L35`, `#L51-L64`;
  announcement-origin responses never clear it `#L28-L33`.
- **Delivery gate** beyond the window: `isDeliveryBlocked = closed || !session.ready ||
  (mode !== 'context' && (sleeping || waking || !outputEnabled))` — context-only injections pass
  while sleeping, spoken ones do not. `realtime-session-runtime.mjs#L128-L135`.
- **Input arbitration with an external host** (an IME or platform app that owns the mic):
  reference-counted per holder, idempotent re-suspend refreshes the TTL, every hold expires
  (default 15 s, max 300 s). `server/src/voice/input-arbitration.mjs#L1-L18`, `#L69-L97`,
  `#L110-L121`. Audio frames are dropped at the Gateway while suspended, as defence in depth
  `realtime-session-runtime.mjs#L1012-L1022`. Protocol: `input.capture.suspend/resume` with
  TTL; local mute needs no Gateway event. `docs/gateway-protocol.md#L508-L519`.
- **Stale-response guard**: a correction is applied only if the response's turn equals the
  committed turn and the user is not speaking. `voice/response-guards/index.mjs#L29-L46`.
- Eval invariant: "Audio and transcripts arriving after a user interruption are suppressed,
  while the interruption event is projected once." `docs/reference/frontend-evaluations.md#L11`.

## 4. Nonblocking work and background tasks

- **Request → Task.** `spawn_thinking(objective)` where `objective` is "a conservative
  interpretation of the user's request, not an execution plan", references resolved into one
  self-contained instruction, verbatim ASR not appended. `deep-dive.md#L173-L180`. The prompt
  forbids speaking before the call and says `accepted` only means accepted.
  `config/frontend-agent/PROMPT.md#L60-L64`.
- **States**: `queued → running → completed`, `running → delegated → finalizing → completed`,
  `→ cancelling → cancelled | failed`. `deep-dive.md#L229-L234`; constants in
  `server/src/task/task-state.mjs#L7-L14`. Public fields deliberately exclude execution mode,
  delivery mode, subagent state, backend permission ids. `deep-dive.md#L236-L240`. UI shows
  queued and running as one "processing" state. `deep-dive.md#L242-L244`.
- **Scheduler**: `maxConcurrent = 4`, `maxConcurrentPerOwner = 2`, plus optional
  `laneKey/laneLimit` (default lane limit 1). `server/src/task/task-scheduler.mjs#L6-L14`,
  `#L23-L33`. (Docs say one per owner into the backend `deep-dive.md#L58-L60`; the per-owner
  cap in code is 2 with the backend adapter serialising writes `deep-dive.md#L216-L217` — see
  "Documented vs code".)
- **Delegation** frees the lane: when the backend calls `session_start`/`session_send` through
  the injected coordinator MCP tools and the adapter validates the returned ids, the Task moves
  to `delegated` and "releases both the backend Agent serialization lock and the Task scheduler
  lane". `deep-dive.md#L295-L303`. Only the correlated target completion can advance the Task;
  busy target, empty result, unrelated update or older result cannot. `deep-dive.md#L310-L315`.
- **No wall-clock deadline** on agent turns; only init/control RPCs have timeouts.
  `deep-dive.md#L317-L321`.
- **Cancellation is confirmed, not optimistic.** queued → cancelled locally; running/finalizing
  → abort active request; delegated → adapter cancels target via recorded correlation; Task stays
  `cancelling` until the result arrives; a cancellation fact is injected once into the next safe
  coordinator turn. `deep-dive.md#L323-L331`.
- **Status queries** read the Task record directly, never create a Task or enter the
  announcement queue. `deep-dive.md#L135-L139`. Prompt: get facts from
  `get_agent_task_status`, do not infer from history. `PROMPT.md#L74-L75`.
- **Follow-ups**: a backend question is presented naturally; the user's answer goes back as
  another `spawn_thinking` "as a continuation of the same work". `deep-dive.md#L182-L186`.
- **Fixed backend session** per owner: `qwen-audio-agent:<owner>:backend`, resumed with
  `session/resume`; voice session ids never change it. `deep-dive.md#L201-L214`.
- **Restart**: queued/running fail with an explicit restart reason; delegated may reattach if the
  adapter can verify the persisted native session. `deep-dive.md#L246-L249`; code:
  `TaskRecoveryAction.REATTACH|FAIL` `server/src/task/task-recovery.mjs#L11-L13`, `#L40-L42`;
  restored status/error assembled in `server/src/task/task-manager.mjs#L213-L228`.

## 5. Result delivery back into the conversation

- **Three separations** the design insists on: execution completion ≠ result delivery,
  generation ≠ heard, interruption ≠ cancellation. `deep-dive.md#L286-L293`;
  `docs/gateway-protocol.md#L506` ("`response.done` means generation finished, not that the
  user heard the response").
- **AnnouncementManager** defaults: `maxBatchItems 8`, `batchWindowMs 120`, retry backoff
  1 s → 10 s with `maxRetryAttempts 8`, `acknowledgementTimeoutMs 120 000`,
  `leaseRenewIntervalMs 20 000`, `resultContextMaxChars 6000`.
  `voice/announcement/announcement-manager.mjs#L66-L85`.
- **Batching**: pending items sorted by sequence; a batch stops at 8 items or when the
  formatted text exceeds 6000 chars; reminder batches and work-result batches never mix; "Only
  events actually represented in this bounded model input may be acknowledged when playback
  ends. Remaining events stay pending." `announcement-manager.mjs#L312-L345`.
- **Delivery loop**: if blocked, re-arm a delayed self-wakeup ("a throttled renderer or a lost
  playback receipt must not leave a completed task parked in 'replying' forever"); if the
  frontend is not ready, hold the batch, renew the lease and retry. `#L351-L366`. On
  `completed` from the model, mark `responseCompleted` and start the acknowledgement timer —
  "Delivery is confirmed only when the client reports that playback has actually started."
  `#L396-L409`. The ack timer itself defers while delivery is blocked and never creates a second
  spoken response while the first is queued/playing. `#L244-L265`.
- **Claims across clients**: TaskManager `notificationClaimTtlMs = 60 000`
  `server/src/task/task-manager.mjs#L70`; `claimNotifications / markNotificationsDelivered /
  renewNotificationClaims / releaseNotificationClaims / reclaimExpiredNotificationClaims`
  `#L1003-L1032`; the manager renews every 20 s while a batch is active
  `announcement-manager.mjs#L231-L242`. Docs: "A renewable claim prevents two live frontends
  from presenting the same result." `deep-dive.md#L286-L289`.
- **Playback receipt acceptance**: `acceptsPlaybackReceipt = outputEnabled && active &&
  responseKnown` — a receipt for an unknown or foreign response id is ignored.
  `voice/realtime-presentation-runtime.mjs#L39-L45`; handled at
  `realtime-session-runtime.mjs#L1070-L1075`. Protocol receipts: `playback.started/ended/
  cancelled`. `docs/gateway-protocol.md#L495-L506`.
- **Context injection vs speaking** are separate: `RealtimeAgentDeliveryRuntime.deliver()`
  returns `{completed:false, blocked:true}` when gated, `{unavailable:true}` if the frontend has
  no injection primitive; `injectContext` is passed per call and tracked per batch so context is
  injected once even if speaking retries. `voice/realtime-agent-delivery-runtime.mjs#L19-L27`,
  `announcement-manager.mjs#L385-L400`.
- **No replay after interruption**: "Once playback starts, interruption does not cause repeated
  replay." `deep-dive.md#L290-L292`; `dismissActive()` on interrupt
  `realtime-session-runtime.mjs#L1068`.
- **Result origin**: the backend's ACP turn ends with `PromptResponse`; only
  `stopReason=end_turn` counts as success; text and non-text content are projected into
  `content`/`artifacts`, never flattened into a model-reconstructed envelope.
  `deep-dive.md#L275-L284`.
- **On (re)activation** of a client: announce pending permissions, pending inputs, claim pending
  notifications, flush. `realtime-session-runtime.mjs#L1004-L1010`.
- **Composition seam**: `taskAnnouncementFactory` passed to `createGatewayApplication` replaces
  the final-result and progress managers together — "dependency injection for product code,
  not a user setting". `deep-dive.md#L484-L489`.

## 6. Presence, progress and animation

- **Progress is observability, not control.** ACP `session/update` is projected into generic
  activity: tool name + bounded detail + state, plan progress, "a generic thinking signal
  without thought content", bounded title/mode. Session ids, subagent ids, raw permission
  payloads and raw thoughts are never shown. "Activity never produces spoken status updates and
  never affects the queue." `deep-dive.md#L253-L271`.
- **Spoken progress** exists but is rare and from a different source: `ProgressAnnouncementManager`
  coalesces backend MESSAGE events into low-frequency updates (`DEFAULT_INTERVAL_MS 60 000`,
  `QUIET_MS 800`, `RETRY_MS 1000`); "it never polls a backend or derives narration from tool
  activity". `voice/announcement/progress-announcement-manager.mjs#L6-L8`, `#L32-L39`. The
  payload is a Gateway envelope `<background_work_progress> task_id … 进展 …`. `#L23-L30`.
  Progress is cleared on sleep and interruption. `realtime-session-runtime.mjs#L338`, `#L748`.
- **Desktop pet states**: `idle/connecting/occupied → idle`, `speaking → waving`, `listening →
  waiting`, `working/starting → running`, `processing → review` (foreground turn, play once),
  `waking/success → jumping`, `error/failure → failed`. Sustained states vs one-shot events are
  arbitrated separately; a pending authorization "does not select an Agent animation".
  `docs/reference/desktop-animations.md#L7-L30`. All backend work incl. thinking shares
  `working`; `processing` is reserved for the foreground Realtime turn. `deep-dive.md#L263-L265`.
- Skins are static JSON + WebP, validated on import, fall back to built-in orb.
  `desktop-animations.md#L27-L30`.

## 7. Latency

- **No in-tree latency measurement.** grep `latency|first_audio|ttfb` over `server/src` hits only
  two comments (`server/src/core/websocket-send.mjs#L28` — send buffer budget "bounds both
  memory and stale-speech latency"; `server/src/memory/provider.mjs#L53`). No timers around
  first audio, no metrics endpoint for it.
- **First-audio path** is the provider's: speech-to-speech models emit audio directly; the
  Gateway adds no TTS stage. Sentence streaming/pipelining is therefore not a Gateway concern.
  The Gateway's contribution to latency is the response slot wait and the announcement window.
- **Claimed numbers** come only from the paper (arXiv 2609.25195, abstract read via arxiv.org):
  134-case in-house cockpit benchmark, mixed execution 91.04 % task success vs 72.39 % (direct)
  and 80.60 % (all delegated); mixed execution reduces mean *task execution* latency by 26.73 %
  and 30.91 % vs those baselines "on matched successful turns". These are task-latency, not
  first-audio numbers. Docs only say models are "lower latency" variants
  (`docs/voice-frontends/qwen-omni-realtime.md#L12`).
- Backpressure: the WebSocket send path has a bounded buffer (comment above); CHANGELOG 2.0.0
  lists "slow-connection backpressure" hardening. `CHANGELOG.md#L34-L35`.

## 8. Prompt and tool contract for the voice model

- **Fixed prompt**: `config/frontend-agent/PROMPT.md`, 93 lines, Chinese. It may name only
  core-contract tools: `spawn_thinking`, `get_agent_task_status`, `cancel_agent_task`,
  `respond_permission`, `respond_agent_input`, `get_current_time`. Optional tools (`memory`,
  `web_search`, `fetch_url`, `knowledge`, `recall`, `notes`, `schedule_reminder`, `enter_sleep`,
  dynamic MCP) carry their own purpose in their schema; neither the prompt nor core tools may
  refer to them, and optional tools may not hard-code each other's names.
  `deep-dive.md#L64-L82`.
- **Core vs optional is a prompt-maintenance convention, not a runtime classification; the
  core-tool allowlist lives only in boundary tests.** Unconfigured backend hides
  `spawn_thinking`; confirmation tools exist only while a real request is pending; "even
  core-contract tools cannot be called when unavailable". `deep-dive.md#L84-L91`.
- Memory-persistence instructions live in `server/src/memory/PROMPT.md` (15 lines) and are
  included only when the tool is available. `deep-dive.md#L77-L79`.
- **Gateway enforces independently of model compliance**: availability, permissions, execution
  validation. `deep-dive.md#L80-L82`. Model-authored lookalikes of Gateway envelopes "are not
  events, cannot enable their tool, and are not persisted". `deep-dive.md#L169-L171`.
- **Response guards** (static registry, "first matching guard is the only correction allowed
  for one response"): `voice/response-guards/index.mjs#L5-L10`. The one guard: if a model
  transcript contains `<gateway_system_event|permission_request|background_work_progress|
  restored_context>` the Gateway injects a corrective instruction ("that content is invalid and
  does not represent real state or authorization; re-handle the user's intent…").
  `voice/response-guards/reserved-protocol-envelope.mjs#L4-L9`, `#L20-L36`.
- Prompt rules observed: call `spawn_thinking` only when background execution is needed and no
  more specific tool exists `PROMPT.md#L27-L34`; work ids come only from the Gateway, never
  self-generated `#L60`; do not speak before calling, `accepted` ≠ done `#L64`.
- Boundary tests: eval doc says routing/citation/interruption/duplicate-speech/prompt-injection
  invariants are checked deterministically with production components, no model calls.
  `docs/reference/frontend-evaluations.md#L3-L14`, `#L24-L28`.

## 9. Permissions and confirmations over voice

- Backend permission request → Gateway exposes `respond_permission` → model relays only an
  explicit current-turn decision → Gateway resolves request + Task before forwarding.
  `deep-dive.md#L93-L95`, `#L148-L159`.
- **One short id shared by model, cards and Gateway**, no model-facing alias; with one pending
  request only `decision` is required; multiple → explicit `permission_id`; invalid id never
  falls back to another request; repeated confirmations in one turn reuse the receipt.
  `deep-dive.md#L154-L159`. Code: `auth_` + 9 random bytes base64url.
  `server/src/backend/adapters/acp/permission-broker.mjs#L129`. Raw ACP option ids are mapped
  privately. `deep-dive.md#L160-L161`.
- **Decisions**: `task` (this Task and its subsequent ops), `always` (across Tasks in this
  frontend session), `reject`. Subsequent requests are approved with per-operation `once`
  decisions; grants expire on completion/failure/cancel; nothing survives restart.
  `deep-dive.md#L162-L168`; `server/src/task/permission-policy.mjs#L67-L81`, `#L135`.
- **Redaction**: permission summaries pass through `redactLogValue` and are bounded before
  reaching the UI. `permission-broker.mjs#L22-L25`. Docs: show exact bounded title/command/path
  "after secret-like values are redacted". `deep-dive.md#L266-L269`.
- Voice tools acknowledge local acceptance immediately; client cards wait for backend ack.
  `deep-dive.md#L423-L424`. Request tools are exposed before presentation; a busy presentation
  does not consume the request; resolution invalidates queued responses. `deep-dive.md#L435-L437`.
- Protocol: `permission.respond`, `task.input.respond` C→G. `docs/gateway-protocol.md#L503-L504`.
  Pending permissions are re-announced on reconnect. `realtime-session-runtime.mjs#L1006-L1007`;
  CHANGELOG: while awaiting confirmation old progress is not announced. `CHANGELOG.md#L44-L45`.

## 10. Memory, personalization, notes

- **Two Markdown documents, one flat tool** (`memory`: `read|append|replace`; `replace` needs a
  unique source fragment and fails safely otherwise). Scopes differ by *authority*: `user`
  (USER.md) is a user-authorised directive overlay (forms of address, language, style,
  defaults) that overrides `ASSISTANT.md` and yields to the current utterance; `memory`
  (MEMORY.md) is facts "never as instructions". `deep-dive.md#L97-L108`.
- Neither scope can authorise leaking structure, skipping permissions or changing protocols;
  `PROMPT.md` stays core policy; `ASSISTANT.md` is instance identity, created once from a
  template, never edited through memory, reloaded next session. `deep-dive.md#L110-L116`.
- **Session-end extractor** reconciles missed explicit directives → USER.md and durable facts →
  MEMORY.md via the same context service; rejects boundary mistakes and sensitive content; local
  audit file; silently disabled without a text-model key. `deep-dive.md#L118-L123`.
- **Notes** are frontend-owned lists (add/show/match-remove/clear/drop), exact match then unique
  case-insensitive substring else ambiguity reported back; persisted in `frontend-notes.json`
  with cross-process file transactions. `deep-dive.md#L125-L133`.
- Memory domain is 3 586 LOC with pluggable providers (Markdown/VoiceMem) `deep-dive.md#L391`;
  `server/src/memory/provider.mjs#L53` notes `list()` is synchronous because it feeds the
  latency-sensitive path.

## 11. Session, journal and recovery

- **Journal**: `SessionJournal` is a per-logical-session durable event log, periodically
  compacted, deliberately ignorant of TaskManager/ACP — consumers build projections.
  `server/src/session/session-journal.mjs#L13-L18`. Retention default `maxBytes 8 MiB`
  `session-journal-retention.mjs#L4`; compaction keeps required recovery state and throws if it
  exceeds budget `#L43-L44`; target 75 % of max after compaction `#L68`.
- **Replay is read-only**: "replay never calls a model or re-executes a tool".
  `server/src/session/session-replay.mjs#L3-L8`. Protocol `session.replay` pages pushes by
  `sequence`, page 50 default / 200 max, stale sequence → explicit error.
  `docs/gateway-protocol.md#L618`.
- **Reconnect backoff**: exponential 500 ms → 10 s with 20 % jitter.
  `server/src/voice/reconnect-backoff.mjs#L1-L12`.
- **Restart semantics**: queued/running tasks fail "please resubmit"; delegated reattach;
  completed results and notification state persist; permission grants do not.
  `deep-dive.md#L246-L249`, `#L168`; `task-manager.mjs#L213-L228`.
- Reconnection "can reclaim results without executing work again; result availability and
  playback confirmation remain separate". `deep-dive.md#L438-L440`. Closing the connection
  releases claims without cancelling work. `#L437-L438`.
- Recovery changes "only the provider-facing history projection", never the journal.
  `realtime-session-runtime.mjs#L139-L140`.

## 12. Clients and transport

- **Gateway Client Protocol invariants**: one owner = one active client connection; one
  WebSocket for business traffic; raw audio on a media fast path, only committed semantic inputs
  enter routing; Events describe, Actions request; client events cannot spoof Gateway/Task/
  permission events; provider and backend wire objects never cross. `docs/gateway-protocol.md#L22-L34`.
- Planes: user input & media, client information events, client-owned tools, runtime commands,
  Gateway state/presentation, receipts & decisions, mute/capture ownership.
  `gateway-protocol.md#L240-L520` (section headings at `#L242`, `#L296`, `#L344`, `#L399`,
  `#L454`, `#L495`, `#L508`).
- **Transports**: WebSocket (primary) and WebRTC (`packages/webrtc`, `server/src/transport/webrtc/`,
  `docs/gateway-webrtc-client.md` 280 lines). WebRTC session fields limited to voice/modalities/
  turn_detection; "instructions and tools belong to the Gateway". `transport/webrtc/protocol.mjs#L82-L83`.
- **Clients**: Electron desktop (6 897 LOC incl. wake word), WebUI (3 561), TUI (4 576), mobile
  dev client (451, Android APK / iOS simulator only per `CHANGELOG.md#L28-L29`), CLI (7 280).
  Desktop serves its own immutable renderer from a random loopback path and proxies only Gateway
  HTTP + WS. `deep-dive.md#L558-L562`. Desktop may configure/restart only its local Gateway;
  WebUI/TUI read-only. `#L549-L553`.
- **Remote access**: mobile pairing over LAN/Tailscale with per-device revocable credentials.
  `CHANGELOG.md#L26-L29`; roadmap still has physical-phone validation unchecked
  `docs/roadmap/gateway-remote-access.md#L139`, `#L181`.
- Error codes never expose credentials, backend objects, stack traces or local paths.
  `gateway-protocol.md#L620-L642`.

## 13. Wake word, sleep and always-on

- **Wake word is client-side (desktop), local**: `sherpa-onnx` keyword spotter with
  encoder/decoder/joiner/tokens/keywords files managed by `model-manager.mjs`.
  `desktop/src/wake-word/sherpa-detector.mjs#L19-L34` (294 LOC total in `desktop/src/wake-word/`).
  A client can declare `wakeWordOnly`, which puts the Gateway session straight to sleep after
  activation. `realtime-session-runtime.mjs#L967-L969`.
- **SleepController**: idle timeout, `canSleep` predicate re-polled every `retryMs` (1 s) when
  busy; `onSleep` failure retries. `server/src/voice/sleep-controller.mjs#L51-L65`. The predicate:
  `inputEnabled && voiceAccess.isActive() && session.ready && !turns.userSpeaking &&
  !announcementWindow.isBlocked() && !connecting && !waking`. `realtime-session-runtime.mjs#L786-L796`.
- Only `speech_started/stopped` provider events count as sleep activity. `#L41-L44`.
- **Sleep is a client presence transition**: Gateway mutes input and hides nothing itself;
  "only the client knows about visible work, permission prompts and playback". `#L755-L759`.
  Protocol: user-requested sleep = model calls client-declared `enter_sleep`; automatic sleep =
  client idle timer → `client.presence.update`; both publish a context-only cause text, deduped
  and resent on reconnect. `docs/gateway-protocol.md#L577-L614`. Sleep "neither cancels backend
  work, discards pending results, nor deliberately disconnects Realtime". `#L611-L612`.
- Cost of always-on: the realtime provider socket stays connected while sleeping (sleep is
  gating, not disconnect); audio/image/text events are dropped while sleeping.
  `realtime-session-runtime.mjs#L1013`, `#L1030-L1035`, `#L1055-L1061`.

## 14. Vision / multimodal input

- Clients send `image.append` (JPEG base64) → `VisualInputBuffer` → `realtimeSession.appendImage`.
  `realtime-session-runtime.mjs#L281-L282`, `#L1029-L1047`. Bounds: `VISUAL_INPUT_MAX_BASE64_BYTES
  = 256 KiB` `server/src/voice/visual-input-buffer.mjs#L2`; JPEG magic bytes checked `#L18-L24`;
  out-of-order frames dropped `#L66-L70`; rate-limited by `intervalMs` `#L72-L78`; dropped-frame
  counter kept.
- Current-turn attachments go to the backend as native protocol parts; only explicit dependence
  on an earlier image uses `input_refs`. `deep-dive.md#L188-L191`.
- Providers that need audio before video get 20 ms silence (axis 2). WebUI can toggle camera
  and mic independently for video models. `CHANGELOG.md#L22-L23`.

## 15. Reminders, scheduling and proactive speech

- `ReminderScheduler`: no polling, one `setTimeout` for the next due task re-armed after each
  fire; on restart overdue reminders are **staggered** (30 s steps), not replayed in a burst.
  `server/src/task/reminder-scheduler.mjs#L14-L19`, `#L24-L34`, `#L65-L76`. Recurrence logic in
  `server/src/task/recurrence.mjs` (202 LOC).
- Due reminders enter the same announcement queue as work results but batch separately and are
  formatted as a `GatewaySystemEvent.REMINDER_DUE` envelope with content/scheduledAt/recurrence/
  timeZone. `announcement-manager.mjs#L9-L32`, `#L40-L56`, `#L317-L319`.
- Proactive speech otherwise = final results (axis 5) and ≤ 1/min progress (axis 6). Scheduled
  backend work reuses `TaskOperations` execution and permission paths; "System jobs are not folded
  into user work." `deep-dive.md#L420-L422`.

## 16. Testing and evaluation

- 378 `*.test.*` files in the tree; `server/test/` 175 entries, `test/` 48; 46 213 LOC of test
  code under `server/test` + `test` (vs 40 136 LOC in `server/src`).
- Deterministic eval suite: `server/eval/frontend-runtime-evaluations.mjs` +
  `run-frontend-evaluations.mjs`; `npm run eval:frontend [-- --json]`; also runs inside the
  server test suite, `release:check` and CI; drives production components, no network, no LLM;
  explicitly does not measure semantic tool-selection quality.
  `docs/reference/frontend-evaluations.md#L16-L34`.
- Dependency-direction tests, prompt boundary tests (core-tool allowlist lives only there),
  physical-removal tests for memory/knowledge. `deep-dive.md#L84-L85`, `#L403-L406`, `#L414-L415`.
- #477 increments tested "through the production task operations, coordinator and frontend
  runtime, with fake model/backend boundaries and no network". `deep-dive.md#L462-L465`.
- Repo-level `TODO|FIXME|XXX|HACK` count in source: **0**.

## 17. Numbers

| Item | Value | Source |
|---|---|---|
| Stars / forks / watchers / open issues | 2 860 / 288 / 19 / 19 | GitHub API 2026-10-08 |
| Created | 2026-07-27 | GitHub API |
| Releases | v1.10.0 08-13, v1.10.1 08-15, v1.11.0 08-20, v2.0.0 09-23, v2.0.1 09-26 | GitHub releases API |
| CHANGELOG sections | 34 (`## Unreleased` … `## 0.2.0`) | `CHANGELOG.md#L3`, `#L466` |
| Files | 1 424 (brief); `server/src` 233 files | `find` |
| LOC server/src | 40 136 | `find \| wc` |
| LOC by domain | voice 8 468 · backend 8 127 · frontend 5 383 · memory 3 586 · task 2 491 · transport 1 964 · session 671 · orchestration 447 · delivery 198 | `wc` |
| LOC clients | shared 8 678 · cli 7 280 · desktop 6 897 · tui 4 576 · web 3 561 · mobile 451 | `wc` |
| Tests | 378 test files; 46 213 test LOC | `find`, `wc` |
| Fixed prompt | 93 lines | `config/frontend-agent/PROMPT.md` |
| Realtime providers | 7 (dashscope, gpt-live, google-live, stepfun, doubao-seeduplex, minicpm-o, s2s) | `server/src/voice/providers/` |
| Audio | PCM16, 16 kHz in / 24 kHz out (GPT-Live 24/24) | axis 2 cites |
| Timers | window batch 120 ms; ack 120 s; retries 8 (1→10 s); claim TTL 60 s; lease renew 20 s; progress ≥ 60 s; sleep retry 1 s; input hold 15 s / 300 s max; reconnect 0.5→10 s ±20 % | axes 3,5,6,11,13 |
| Paper | 134 cases; 91.04 % vs 72.39 % / 80.60 %; latency −26.73 % / −30.91 % | arXiv 2609.25195 abstract |
| Contributors | not determinable from shallow clone (1 commit, author "Marc Liang") | `git log` |

Model sizes: none in tree (all cloud models; MiniCPM-o and s2s point at external services).

## 18. Techniques worth borrowing

Ranked for a single-model theuth "talk" mode with local STT/TTS (no speech-to-speech model, no
separate gateway process).

1. **Monotonic turn generation with commit/stale checks** — `realtime-turn-state.mjs#L7-L14`,
   `#L112-L127`, `#L145-L149`. Every late event (STT final, TTS chunk, tool result, background
   result) carries the generation it belongs to and is dropped if older than the committed
   boundary; interruption = `advanceBoundary()`. Why: theuth's talk mode has exactly the race
   this solves (late TTS audio after barge-in, a tool result from an interrupted turn). Cost:
   small — one counter object threaded through STT/TTS/tool callbacks. Risk: low.
2. **Safe insertion window as a pure predicate** — `announcement-window.mjs#L78-L84` plus the
   delivery gate `realtime-session-runtime.mjs#L128-L135`. Background results (subagent
   finishes, job results, reminders) speak only when `!userSpeaking && !turnPending &&
   nothing queued`; context-only injections pass even while sleeping. Why: theuth already has
   `<job_result>`/`<message>` arrivals that would otherwise interrupt speech. Cost: a predicate +
   a queue. Risk: starvation if the predicate never clears → they add a self-wakeup (item 4).
3. **Playback receipt ≠ generation done** — `gateway-protocol.md#L495-L506`,
   `announcement-manager.mjs#L396-L409`, `acceptsPlaybackReceipt` `realtime-presentation-runtime.mjs#L39-L45`.
   Mark a result "delivered" only when the client reports audio actually started, and ignore
   receipts for unknown response ids. Why: with local TTS theuth owns both ends, so this is
   cheap and it fixes "spoken while the window was hidden". Cost: one client→engine event.
4. **Bounded announcement batching with retry/ack/lease timers** — `announcement-manager.mjs#L66-L85`,
   `#L312-L345`, `#L244-L265`, `#L351-L366`. 120 ms coalescing window, 8 items / 6000 chars per
   spoken batch, only batched items get acknowledged, exponential retry capped at 8, ack timeout
   that defers while blocked, self-wakeup when a receipt is lost. Why: several desks/children
   finishing together should become one sentence. Cost: ~300 lines. Risk: tuning.
5. **Renewable claims for multi-client delivery** — `task-manager.mjs#L70`, `#L1003-L1032`,
   `announcement-manager.mjs#L231-L242`. 60 s claim, renewed every 20 s while speaking, released
   on disconnect, reclaimed when expired. Why: theuth has desktop + web + phone clients on one
   owner; prevents the same result spoken twice. Cost: a small claims table. Risk: low.
6. **Reserved-envelope response guard** — `reserved-protocol-envelope.mjs#L4-L36`,
   `response-guards/index.mjs#L5-L46`. If the model's transcript contains a tag only the
   harness may emit, inject a one-shot correction and treat the output as invalid; apply only if
   the response is still the committed turn. Why: theuth injects `<job_result>`, `<message>`,
   `<reminder>` and a talk model could parrot them. Cost: a regex + one instruction. Risk: low.
7. **Three separations as design rules** (completion ≠ delivery, generation ≠ heard,
   interruption ≠ cancellation) and the review checklist — `deep-dive.md#L286-L293`,
   `#L456-L460`, `#L564-L575`. Why: cheap to adopt as a written invariant for the talk design.
8. **TTL-bounded, ref-counted, idempotent microphone arbitration** — `input-arbitration.mjs#L1-L18`,
   `#L69-L97`. Why: theuth's dictation, voice notes, meeting capture and talk all want the mic;
   a holder that crashes must not silence talk forever. Cost: ~150 lines. Risk: low.
9. **Sleep as a conjunction predicate with retry** — `sleep-controller.mjs#L51-L65`,
   `realtime-session-runtime.mjs#L786-L796`; sleep never cancels work or disconnects
   `gateway-protocol.md#L611-L612`. Why: theuth talk's idle/exit logic can adopt the exact
   predicate list. Cost: trivial.
10. **Progress narration policy**: activity → visual only; spoken progress only from explicit
    agent messages, ≤ 1/min, 800 ms quiet — `progress-announcement-manager.mjs#L6-L8`, `#L32-L39`;
    `deep-dive.md#L253-L271`. Why: directly answers "what should the orb say while children run".
11. **Confirmed (not optimistic) cancellation with `cancelling` state** — `deep-dive.md#L323-L331`.
    Why: theuth's `cancel_job`/child cancel could show "cancelling" until the child actually
    stops. Cost: one more state.
12. **Permission relay rules**: short shared id, `decision` alone when one pending, invalid id
    never falls back, receipt reuse within a turn, grants die with the task —
    `deep-dive.md#L154-L168`, `permission-broker.mjs#L129`. Why: theuth approvals over voice need
    exactly these guards. Cost: policy code only.
13. **Staggered overdue reminders on restart** — `reminder-scheduler.mjs#L14-L19`, `#L65-L76`.
    Why: theuth routines after a restart could otherwise fire as a burst. Cost: trivial.
14. **Bounded visual buffer** (size cap, JPEG magic, out-of-order drop, min interval) —
    `visual-input-buffer.mjs#L2`, `#L18-L24`, `#L66-L78`. Why: if talk gets camera/screen frames.
15. **Deterministic boundary evals that run in CI without a model** — `frontend-evaluations.md#L3-L28`.
    Why: theuth could pin interruption/duplicate-speech/injection invariants the same way.

## 19. Do-not-borrow

- **Speech-to-speech provider plumbing**: the capability table and all seven protocol files
  (`realtime-provider.mjs#L75-L100`, `providers/*`) exist because the model owns VAD, STT, TTS
  and turn detection and each vendor differs in acks/echoes/slots. With local STT+TTS theuth has
  none of these problems; the turn-generation idea transfers, the provider dialect handling does not.
- **Response slot recovery by socket disconnect** (`realtime-response-slot.mjs#L25-L38`) — a
  workaround for a remote single-slot service; a local TTS queue is simply cancelled.
- **Server VAD / `interrupt_response`** (`s2s.mjs#L86`, `webrtc/protocol.mjs#L82-L87`) — theuth
  must do its own VAD; nothing here to copy.
- **Gateway topology**: separate Node Gateway, Electron desktop proxying to it, one-owner-one-
  connection WebSocket protocol with capability negotiation (`gateway-protocol.md#L22-L34`,
  `deep-dive.md#L549-L562`). theuth's engine already is the gateway; adding GCP would be a
  second protocol.
- **ACP coordinator session + injected MCP coordination tools** (`deep-dive.md#L201-L217`,
  `#L341-L360`): built for a coding agent that delegates to project sessions. theuth's own
  subagents/desks already have `spawn_subagent`/`send_message`; the "fixed backend session per
  owner" idea conflicts with theuth's per-task children.
- **Frontend-agent prompt as the only persona** (`PROMPT.md`, Chinese, 93 lines) and the
  USER.md/MEMORY.md/ASSISTANT.md triad as files — theuth has knowledge + prompt.md; borrow the
  *authority split* idea at most, not the file layout.
- **Desktop pet skins / animation protocol** (`desktop-animations.md`) — theuth has the orb;
  only the state list is informative.
- **Mobile dev client / Tailscale pairing** — theuth already has phone + mesh.

## 20. Weaknesses

Admitted in-tree:
- Queued/running tasks cannot survive a Gateway restart — they fail with "please resubmit";
  only delegated tasks reattach. `deep-dive.md#L246-L249`; `task-manager.mjs#L221-L222`.
- Permission grants (`task`/`always`) never survive restart and never create a persistent rule.
  `deep-dive.md#L168`.
- Roadmap items still unchecked: "Enforce dependency direction", "Add characterization tests for
  current critical behavior", merge roadmap with RFC. `docs/roadmap/frontend-chatbot-runtime.md#L185-L187`.
  Remote access: persistent WS + long audio on a physical phone, DERP fallback and Wi-Fi/cellular
  transitions untested. `docs/roadmap/gateway-remote-access.md#L139`, `#L181`. iOS is a simulator
  build only. `CHANGELOG.md#L28-L29`.
- Eval suite explicitly does not measure semantic tool-selection quality; live-model evals are
  outside the gate. `frontend-evaluations.md#L29-L34`.
- Compaction throws when required recovery state exceeds the 8 MiB journal budget.
  `session-journal-retention.mjs#L43-L44`.
- The acknowledgement path tolerates a 120 s window and up to 8 retries before abandoning a
  batch — a lost receipt can keep a result "replying" for minutes. `announcement-manager.mjs#L77-L78`,
  `#L294-L299`.
- Fixed per-owner backend session means a new voice conversation continues old backend context
  by design. `deep-dive.md#L213-L214`.

Observed:
- **Entirely dependent on cloud speech-to-speech models** (DashScope/OpenAI/Google/StepFun/
  Doubao); the only self-hostable paths are MiniCPM-o and the HF speech-to-speech service.
  No local STT/TTS, no VAD of its own, no echo cancellation beyond browser `getUserMedia`.
- **No latency instrumentation** anywhere in `server/src` (axis 7); the paper's numbers are
  task-execution latency on an in-house benchmark, not first-audio.
- Prompt and many runtime strings are Chinese-only (`PROMPT.md`, error at
  `realtime-session-runtime.mjs#L1058`, guard text `reserved-protocol-envelope.mjs#L22-L25`),
  so the design is bilingual in docs but monolingual at the model boundary.
- One active client connection per owner is a protocol invariant (`gateway-protocol.md#L24`),
  so "claims across clients" mainly protect reconnects and sequential clients, not true
  simultaneous multi-device presence.
- Shallow clone: cannot assess contributor count or cadence beyond 5 releases in 6 weeks
  (Aug 13 → Sep 26) and a push on 2026-10-08; `open_issues_count` 19.
- `TaskScheduler` allows 2 concurrent per owner while docs say one Task per owner enters the
  backend at a time — the serialisation actually comes from the adapter lock, so the scheduler
  numbers are not the invariant the docs describe (see below).

## Documented vs code

Documented, not found in code (or only partially):
- "For each owner, only one Task item is sent into the configured BackendPort at a time"
  (`deep-dive.md#L58-L60`) — the scheduler's default is `maxConcurrentPerOwner = 2`
  (`task-scheduler.mjs#L8-L9`); serialisation is enforced by the ACP adapter's write lock
  (`deep-dive.md#L216-L217`), not by the scheduler. Not contradicted, but the invariant lives
  elsewhere than the diagram implies.
- "Safe duplex insertion window" is never named in code; it is `AnnouncementWindow.isBlocked()`
  + `isDeliveryBlocked` (axis 3). Found, under another name.
- Latency claims (paper) — nothing measured in the tree (axis 7).
- "Progressively OpenTelemetry-aligned traces" (`roadmap/frontend-chatbot-runtime.md#L164`) —
  not searched exhaustively; no tracing seen in `voice/` or `task/`.

In code, undocumented (or thinly documented):
- `RealtimeResponseSlot` disconnect-on-unconfirmed-cancel (`realtime-response-slot.mjs#L25-L38`)
  — only a CHANGELOG line (`CHANGELOG.md#L48-L49`).
- Response guards registry (`response-guards/index.mjs`) — the deep-dive mentions lookalike
  envelopes being ignored (`#L169-L171`) but not the corrective re-prompt.
- Exact timer constants (batch 120 ms, ack 120 s, claim 60 s, lease 20 s, progress 60 s,
  input hold 15/300 s, visual 256 KiB) are code-only.
- `wakeWordOnly` client flag → immediate sleep (`realtime-session-runtime.mjs#L967-L969`).
- 20 ms silence priming for video-first providers is documented only in the custom-provider
  guide (`custom-provider.md#L37`).

## Cites checked

Re-opened with `read_file` in the final pass and keyword-matched; 44 of 44 ✓.

- ✓ `docs/architecture/deep-dive.md#L11-L23` — "not a fourth core component"
- ✓ `docs/architecture/deep-dive.md#L58-L60` — "never waits"
- ✓ `docs/architecture/deep-dive.md#L141-L146` — "subagents"
- ✓ `docs/architecture/deep-dive.md#L229-L234` — "finalizing"
- ✓ `docs/architecture/deep-dive.md#L246-L249` — "restart"
- ✓ `docs/architecture/deep-dive.md#L286-L293` — "renewable claim"
- ✓ `docs/architecture/deep-dive.md#L323-L331` — "confirmed rather than optimistic"
- ✓ `docs/architecture/deep-dive.md#L456-L460` — "do not cancel"
- ✓ `server/src/voice/realtime-turn-state.mjs#L7-L14` — "generation boundary"
- ✓ `server/src/voice/realtime-turn-state.mjs#L112-L127` — "isStale"
- ✓ `server/src/voice/realtime-response-slot.mjs#L1-L3` — "Never treat a local timeout"
- ✓ `server/src/voice/input-arbitration.mjs#L17-L18` — "15_000"
- ✓ `server/src/voice/announcement/announcement-window.mjs#L78-L84` — "turnPending"
- ✓ `server/src/voice/announcement/announcement-manager.mjs#L66-L85` — "acknowledgementTimeoutMs = 120000"
- ✓ `server/src/voice/announcement/announcement-manager.mjs#L396-L409` — "playback has actually started"
- ✓ `server/src/voice/realtime-session-runtime.mjs#L128-L135` — "isDeliveryBlocked"
- ✓ `server/src/voice/realtime-session-runtime.mjs#L786-L796` — "announcementWindow.isBlocked"
- ✓ `server/src/voice/realtime-session-runtime.mjs#L1064-L1069` — "INTERRUPT"
- ✓ `server/src/voice/realtime-presentation-runtime.mjs#L39-L45` — "responseKnown"
- ✓ `server/src/voice/response-guards/reserved-protocol-envelope.mjs#L4-L9` — "background_work_progress"
- ✓ `server/src/task/task-scheduler.mjs#L6-L14` — "maxConcurrentPerOwner = 2"
- ✓ `server/src/task/task-manager.mjs#L70` — "notificationClaimTtlMs = 60_000"
- ✓ `server/src/voice/announcement/progress-announcement-manager.mjs#L6-L8` — "60_000"
- ✓ `server/src/voice/sleep-controller.mjs#L51-L65` — "canSleep"
- ✓ `server/src/voice/reconnect-backoff.mjs#L1-L12` — "maxMs = 10_000"
- ✓ `server/src/voice/visual-input-buffer.mjs#L2` — "256 * 1024"
- ✓ `server/src/task/reminder-scheduler.mjs#L14-L19` — "stagger"
- ✓ `server/src/backend/adapters/acp/permission-broker.mjs#L129` — "randomBytes(9)"
- ✓ `docs/gateway-protocol.md#L22-L34` — "only one active Client connection"
- ✓ `docs/gateway-protocol.md#L495-L506` — "response.done"
- ✓ `docs/gateway-protocol.md#L611-L612` — "neither cancels backend work"
- ✓ `desktop/src/wake-word/sherpa-detector.mjs#L19-L34` — "sherpa-onnx"
- ✓ `shared/gateway/webrtc-browser.mjs#L153-L156` — "echoCancellation: true"
- ✓ `server/src/voice/providers/s2s.mjs#L86` — "server_vad"
- ✓ `server/src/voice/realtime-provider.mjs#L384-L392` — "20 ms"
- ✓ `server/src/session/session-replay.mjs#L3-L8` — "never calls a model"
- ✓ `server/src/session/session-journal-retention.mjs#L4` — "8 * 1024 * 1024"
- ✓ `docs/roadmap/frontend-chatbot-runtime.md#L185-L187` — "Enforce dependency direction"
- ✓ `config/frontend-agent/PROMPT.md#L64` — "accepted"
- ✓ `docs/architecture/deep-dive.md#L154-L159` — "permission_id"
- ✓ `server/src/task/permission-policy.mjs#L75-L76` — "auto_allow"
- ✓ `server/src/voice/realtime-session-runtime.mjs#L967-L969` — "wakeWordOnly"
- ✓ `docs/reference/frontend-evaluations.md#L11` — "interruption"
- ✓ `CHANGELOG.md#L48-L49` — "占用"

Not re-opened (read once during the pass, cited as seen): the remaining `deep-dive.md` and `gateway-protocol.md` ranges, provider sample-rate lines, `visual-input-buffer.mjs#L66-L78`, `task-recovery.mjs`, `permission-policy.mjs#L67-L81`, `desktop-animations.md`, LOC and GitHub-API numbers.
