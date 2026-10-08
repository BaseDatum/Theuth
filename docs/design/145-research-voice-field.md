# Web survey — realtime voice-agent runtimes, the Qwen-Audio-Agent paper, and user evidence (2026-10-08)

Method: read-only web. `search_web` was degraded (brave/google/qwant/firecrawl all failed; duckduckgo only) so most material came via `read_url` on official docs, the GitHub REST API/HTML issue lists, HN Algolia, and arXiv HTML. Reddit blocks fetches ("You've been blocked by network security"), so Reddit evidence is thread titles only. Every claim carries its URL; "not found" is stated where it applies.

## 1. The paper — arXiv 2609.25195, "Qwen-Audio-Agent Technical Report" (Alibaba Token Foundry, 21 Sep 2026)

Source: https://arxiv.org/abs/2609.25195 and https://arxiv.org/html/2609.25195 (PDF not needed; HTML complete). Authors Deng, Ji, Kong, Li, Li, Zhang, Zhu, Zhuo. Code: https://github.com/QwenAudio/qwen-audio-agent.

**Architecture claimed (§2.1).** A *harness*, not a model: a Frontend Agent (full-duplex dialogue, chooses between direct tool calls and delegation), a Backend Agent (own context, tools, workspace), an Orchestration Runtime (task state, requests for input/authorization, scheduling result return), and Clients. Adapters: frontend adapters translate provider streaming events/tool calls; backend adapters speak ACP, A2A, or custom, and declare optional controls (cancel, request-user-input).

**Asynchronous task coordination (§2.2) — the load-bearing design rules:**
- Delegation is one tool, `spawn_thinking`, taking a *self-contained objective*; frontend dialogue history and memory are **not** transferred automatically. Runtime acks immediately with a task id.
- Task record = objective, status (queued/running → completed/failed/cancelled), progress, outputs, pending requests; the frontend can query it in later turns; follow-ups are new tasks referencing earlier results.
- **"Speech interruption and task cancellation are controlled independently. Interrupting a response stops spoken output while preserving the task's execution state."**
- **"Result delivery is scheduled separately from execution completion. The runtime retains completed results while the user is speaking or a frontend response is in progress. Results that arrive within the same collection window are combined into one response request."** Separate execution and delivery records let a failed delivery be retried without redoing work.
- Authorization: preview-and-commit — the tool prepares the change, the backend task pauses, the frontend obtains the decision, the task resumes (§3.3).

**Environment events (§2.3).** Unified event protocol; each event type has schema, retention rule and *response policy*: handle directly, update context silently, schedule a response, or interrupt the current response. Example: user changes destination on the cockpit UI → context updated with no spoken response.

**Memory (§2.4).** Bounded memory snapshot per turn; async consolidation; "current user requests take precedence over stored preferences, explicit over inferred"; revision checks stop delayed background updates overwriting newer corrections; rejected preferences are never re-added automatically.

**Table 1 (capability comparison, their claim):** only Qwen-Audio-Agent has cross-provider frontend support, full backend task lifecycle management, extensible environmental events, persistent memory and an open-source runtime; gpt-realtime, GPT-Live and Gemini Live are marked △ "requires application integration" for lifecycle/events/memory.

**Evaluation (§4).** In-house cockpit benchmark: 134 cases (86 short, 48 multi-step), 159 user turns, 41 business tools. Frontend = Qwen Audio 3.0 Realtime Plus, backend = qwen3.8-max. Metrics: adapted FDB V3 (Tool F1, argument accuracy, reply quality) + execution-based task success.

| Config | Tool F1 | Arg acc | Task success | Reply quality |
|---|---|---|---|---|
| Audio frontend, direct tools | 0.947 | 93.6 | 72.39 | 95.5 |
| Audio frontend, all delegated | 0.908 | 89.8 | 80.60 | 89.6 |
| Text backend, direct tools | 0.963 | 94.1 | 90.30 | 100.0 |
| Audio frontend, mixed | 0.962 | 95.2 | **91.04** | 97.8 |

Direct succeeds on 83/86 short but 14/48 multi-step; all-delegated 71/86 and 37/48; mixed 84/86 and 38/48.

Latency (80 turns from 71 cases that all configs completed; measured from end of spoken request to task completion, *excluding reply generation and playback*):

| Config | Short (74 turns) | Multi-step (6 turns) | Overall |
|---|---|---|---|
| Direct | 1.488 s | 67.705 s | 6.455 s |
| All delegated | 4.266 s | 38.653 s | 6.845 s |
| Text backend | 3.149 s | 35.449 s | 5.572 s |
| Mixed | 1.591 s | 43.430 s | **4.729 s** |

Mixed = −26.73 % vs direct, −30.91 % vs all-delegated. Note the ~2.8 s delegation overhead on short tasks (4.27 vs 1.49 s) — the cost of routing a quick question through the backend.

**Limitations admitted / visible.** In-house benchmark, one domain, 6 multi-step latency turns; no measurement of interruption handling, barge-in accuracy, time-to-first-audio or how results are announced; the all-delegated and text baselines are *faster* on multi-step turns; "Future work will examine conversational behavior during task execution, recovery over longer sessions, and personalization" (§5). No ablation of the result-holding/batching policy.

## 2. Their users — QwenAudio/qwen-audio-agent issues, releases

Repo facts (https://api.github.com/repos/QwenAudio/qwen-audio-agent): created 2026-07-27, 2,860 stars, 288 forks, 19 open issues, 47 issues total, pushed 2026-10-08. No license field returned. Description: "A realtime voice runtime that keeps Agents talking, working, and present." Discussions: not found (page returned nothing). Caveat: the repo is young and most issues are filed by the maintainer (x-lixu) as roadmap/plan items; reaction counts are tiny (max 2).

**Sorted by reactions** (https://github.com/QwenAudio/qwen-audio-agent/issues?q=is%3Aissue+sort%3Areactions-%2B1-desc):
- #92 (+2, closed) "自动长期记忆：会话后提取与统计确认（无感记忆）" — unattended long-term memory; "the frontend is only a voice orb, no memory-confirmation UI… memory must be fully automatic, low-disturbance"; cites Mem0's lesson "garbage extraction is the biggest failure mode — be conservative." https://github.com/QwenAudio/qwen-audio-agent/issues/92
- #117 (bug, closed) "长文本回复（400+ 中文字符）语音回复约 200～300 字范围内提前结束" — spoken reply stops after ~200 chars (local s2s) / ~300 chars (cloud) on long answers. https://github.com/QwenAudio/qwen-audio-agent/issues/117
- #431 (open, 3 comments) support Cursor CLI as an ACP backend. https://github.com/QwenAudio/qwen-audio-agent/issues/431
- #71 desktop auto-sleep after inactivity + global hotkey wake; must "handle speaking, announcing, awaiting-permission and background-task-running edge cases — never sleep at an unsafe moment". https://github.com/QwenAudio/qwen-audio-agent/issues/71
- #499 (bug) smart-cockpit with Doubao model: "when the MCP tool fails, the frontend is silent, no announcement". https://github.com/QwenAudio/qwen-audio-agent/issues/499
- #292 recover mic capture after audio input device changes. https://github.com/QwenAudio/qwen-audio-agent/issues/292

**Sorted by comments** (https://github.com/QwenAudio/qwen-audio-agent/issues?q=is%3Aissue+sort%3Acomments-desc):
- #185 (5 comments) Roadmap "Realtime Voice Chatbot Runtime + Backend Action Agent": frontend tools carry a policy `mode: 'inline' | 'background' | 'control'`, with `spawn_thinking` the only background tool; "the frontend may run a bounded short tool loop… the user's interjection immediately terminates the current frontend loop"; R5 lists evaluations for "routing, citation, interruption, repeated announcement and prompt injection". https://github.com/QwenAudio/qwen-audio-agent/issues/185
- #88 (4 comments, closed) quick-query lane: "a read-only, answerable-in-seconds question… degrades into filing a ticket — the user hears 'task accepted' instead of an answer." Proposal: slot behind the running coordinator turn, 15 s cap then promote to a normal task "and the user is told it's taking longer", "the frontend bridges with a natural half-sentence ('let me check that')", "if the user interrupts… the dangling query is dropped (no orphaned announcements)". https://github.com/QwenAudio/qwen-audio-agent/issues/88
- #80 reminders/timers: "AnnouncementManager already provides the full safe-window insertion + retry + spoken-confirmation infrastructure"; "Duplex conversation is never blocked (roadmap §5 invariant 3)"; reminders missed while the gateway was down are delivered as catch-up, "never silently lost". https://github.com/QwenAudio/qwen-audio-agent/issues/80
- #78 / #77 Linux and Windows desktop builds (3 and 1 comments). https://github.com/QwenAudio/qwen-audio-agent/issues/78
- #130 support Qwen3.5 Omni Plus Realtime as frontend. https://github.com/QwenAudio/qwen-audio-agent/issues/130
- #38 "Qwen Audio 3.0 Realtime 价格太贵，免费额度又不够" — too expensive, free quota insufficient; #21 cannot find the free quota. https://github.com/QwenAudio/qwen-audio-agent/issues/38
- #20 / #449 asks for a demo video (3 and 2 comments). https://github.com/QwenAudio/qwen-audio-agent/issues/20
- #165 "Dictation Agent": hotkey takes over any text field, live transcript, voice edits, Esc cancels. https://github.com/QwenAudio/qwen-audio-agent/issues/165
- #436 desktop Settings cannot select generic ACP and silently reverts `AGENT_PROTOCOL=acp`. https://github.com/QwenAudio/qwen-audio-agent/issues/436
- #333 WS upgrade with `Origin: null` crashes the gateway. https://github.com/QwenAudio/qwen-audio-agent/issues/333

**Releases** (https://github.com/QwenAudio/qwen-audio-agent/releases): v1.8.x–v1.11.0 through Aug 2026 (DeepSeek backend v1.10.0 13 Aug; injectable realtime providers v1.11.0); **v2.0.0 23 Sep 2026** "rebuilt the orchestration runtime with a unified client protocol and ACP/A2A backend integration… desktop conversation panel and remote mobile access… customer service, smart cockpit, digital human examples… technical report"; v2.0.1 26 Sep (macOS codesign fixes, backend discovery/installation fixes, 3 reactions).

Praise: not found as explicit issues; the only positive signals are the demo-video requests and 2.8 k stars in ten weeks.

## 3. The field — how the runtimes handle the five behaviours

**OpenAI Realtime (gpt-realtime-2.x)**
- Turn detection: `server_vad` (threshold, `prefix_padding_ms` 300, `silence_duration_ms` 500 defaults) or `semantic_vad` with `eagerness: low|medium|high|auto` — "a way to control how eager the model is to interrupt the user, tuning the maximum wait timeout"; `low` to "let the user speak uninterrupted". https://platform.openai.com/docs/guides/realtime-vad
- Barge-in: `interrupt_response` and `create_response` can be set false to keep VAD but decide manually when to reply (https://platform.openai.com/docs/guides/realtime-conversations). Truncation to what was heard is `conversation.item.truncate` with `audio_end_ms`; LiveKit found the API rejects it when the interrupt lands before the first audio frame (`audio_end_ms=0` → "Only model output audio messages can be truncated") https://github.com/livekit/agents/issues/6157.
- Async tool results: "the Realtime API allows clients to continue a session while a function call is pending… good for UX… but the model sometimes hallucinates the content of a nonexistent function response" — async FC is GA-model only and recommended with MCP. https://developers.openai.com/blog/realtime-api

**OpenAI GPT-Live (gpt-live-1)** — the direct analogue of the paper's frontend/backend split.
- "GPT-Live handles a spoken conversation while a backend agent looks up information… It can listen while speaking (full duplex)… Backend work can continue when the caller interrupts the assistant; your application decides whether to finish or cancel it." https://developers.openai.com/api/docs/guides/live
- Prompt template has fixed headings `Backchannel policy` ("moderate backchannels… without competing with the main response"), `Interruption policy` ("Stop speaking when the user interrupts. Listen"), `Delegation policy`; "'Stop talking' asks the assistant to yield; 'Cancel my booking' asks the backend to take an action" — the same speech-vs-task separation as the paper. https://developers.openai.com/api/docs/guides/live-prompting
- "GPT-Live can continue speaking while your application reviews a backend result. If your application must control when the user hears audio, add playback controls." Client delegation lets the app "validate, redact, combine, or discard results before they reach GPT-Live." https://developers.openai.com/api/docs/guides/live-delegation

**Gemini Live**
- Async function calling: "some function calls can block the model's execution, which causes the audio stream to go quiet and the user to be left waiting in silence"; all calls non-blocking; the function *response* carries `scheduling`: `SILENT` (added to context, no response, no interruption), `WHEN_IDLE` ("only when there is no active user interaction… waits for it to complete"), `INTERRUPT` (respond immediately). https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/live-api/asynchronous-function-calling — NON_BLOCKING is the default on Gemini 3.8 Live (search snippet, https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live; page itself unreachable — redirect loop).
- Barge-in: `realtimeInputConfig.activityHandling` = `START_OF_ACTIVITY_INTERRUPTS` (default) or `NO_INTERRUPTION` ("finishes the sentence it is on"); automatic activity detection has start/end sensitivity, prefix padding, silence duration (reference https://ai.google.dev/api/live, quoted via https://github.com/hkjarral/AVA-AI-Voice-Agent-for-Asterisk/issues/637 — ai.google.dev pages redirect-loop from this host).
- Known rough edge: a forum thread "scheduling SILENT in non-blocking function response not preventing duplicate audio generation" (title only; body not fetched) https://discuss.ai.google.dev/t/scheduling-silent-in-non-blocking-function-response-not-preventing-duplicate-audio-generation/114361

**LiveKit Agents** (most complete open implementation of all five behaviours)
- Barge-in: "When interrupted, the agent stops speaking and automatically truncates its conversation history to include only the portion of the speech that the user heard." `allow_interruptions=False` per `say()`, `disallow_interruptions()` inside a tool for non-rollbackable actions; `session.interrupt()` stops queued speech "at the first uninterruptible one"; `mode: "adaptive"` gates VAD "shortly after the agent starts speaking, then re-enables it once the adaptive detector identifies a genuine interruption rather than a backchannel"; `min_duration`/`min_words`; false interruptions (empty transcript) → "the agent resumes speaking from where it left off" (`resume_false_interruption`, `false_interruption_timeout`). https://docs.livekit.io/agents/build/turns/
- Turn detection: audio EOT model default, 14 languages, requires VAD `min_silence_duration ≥ 250 ms` (Silero default 0.55 s); dynamic endpointing adapts `min_delay..max_delay` "based on session pause statistics". Deprecated text model: Qwen2.5-0.5B base, 396 MB, 50–160 ms per turn, TPR 99.3 %, TNR 85–96 %. https://docs.livekit.io/agents/logic/turns/turn-detector/ Benchmarks via https://github.com/livekit/eot-bench.
- Tools: "A tool continues running in the background until it returns; interrupting the agent doesn't cancel the work"; a tool finishing after an interrupt keeps call+result in history "so the model doesn't call it again… Recording the result doesn't prompt a reply, so the agent doesn't speak." Short lookups: `wait_if_not_interrupted` then return `None` if interrupted. "For work that takes more than a few seconds, don't block the turn at all." https://docs.livekit.io/agents/logic/tools/definition/
- Async tools: "The agent stops talking, the user hears silence, and a regular tool can't send progress updates, be cancelled, or stop the LLM from calling the same tool twice." `ctx.update(msg)` → "the LLM reads it, voices something natural"; `ctx.with_filler("Still searching, hang on a sec.", delay=5)` → "the filler plays once the session has been continuously idle for delay seconds. Fillers only play during quiet pauses, so they don't talk over the user or pile up"; `ctx.foreground()` "first plays any reply the tool has already queued, waits for the session to be idle, then prevents other agent speech for the duration" for mid-task questions. https://docs.livekit.io/agents/logic/tools/async/
- "Thinking sounds" (keyboard/ambient on a separate track) tied to the thinking state. https://docs.livekit.io/agents/multimodality/audio/background-audio/

**pipecat / Smart Turn** — Smart Turn v3: Whisper-Tiny base + linear head, "approximately 8M parameters", int8/fp32, 16 kHz, ≤ 8 s of the user's turn, "inference completing in around 65 ms on a standard 1x instance"; run after Silero VAD detects silence. https://github.com/pipecat-ai/smart-turn ; bundled as `LocalSmartTurnAnalyzerV3` with a separate `stop_secs` from the VAD. https://docs.pipecat.ai/api-reference/server/utilities/turn-detection/smart-turn-overview . Gemini SILENT/WHEN_IDLE/INTERRUPT exposed (issue https://github.com/pipecat-ai/pipecat/issues/4118); open #4657 argues `cancel_on_interruption` wrongly couples "cancel the in-flight call" with "render as async tool (running placeholder + finished result)". https://github.com/pipecat-ai/pipecat/issues/4657

**huggingface/speech-to-speech** — VAD → STT → LLM → TTS cascade behind "the core OpenAI Realtime GA event set"; Silero VAD v5 for boundaries; MLX path for Apple Silicon; `--local_audio_block_mic_during_playback` "pauses microphone capture during playback, so you cannot interrupt the assistant while it speaks" — i.e. no echo cancellation, barge-in traded away. https://github.com/huggingface/speech-to-speech

**Moshi / Kyutai** — single full-duplex model: user and model audio as parallel streams, "removal of explicit speaker turns, and the modeling of arbitrary conversational dynamics", "theoretical latency of 160 ms, 200 ms in practice". https://arxiv.org/abs/2410.00037 . Unmute wraps any text LLM in Kyutai STT/TTS; the STT "detects that the user has stopped speaking and it's time to generate a response" (semantic VAD in the STT). https://github.com/kyutai-labs/unmute

**Sesame CSM** — speech generation, not turn-taking: 1B/3B/8B backbones with 100–300 M decoders; time-to-first-audio is attacked by splitting at the zeroth RVQ codebook because delay-pattern RVQ "requires N backbone steps before decoding the first audio chunk". No barge-in or tool mechanics described. https://www.sesame.com/research/crossing_the_uncanny_valley_of_voice

**Vocode** — hosted dials: `interrupt_sensitivity` low (default, "ignore backchannels e.g. 'sure', 'uh-huh'") / high ("stop at any word"); `endpointing_sensitivity` sensitive/relaxed/auto; `conversation_speed`; open-source adjusts a `speed_coefficient` from the user's measured WPM. https://docs.vocode.dev/conversational-dials

**Telnyx** (industry, via HN) — "separates triggering a backend request from delivering the result… pushes the result back into the live call… the assistant receives that context mid-call". https://telnyx.com/release-notes/async-webhook-tools-ai-assistants

"Do not interrupt the user to announce a result": explicit in Gemini `WHEN_IDLE`/`SILENT`, LiveKit filler idle gating and `foreground()`, and the paper's hold-while-speaking rule; OpenAI leaves it to the app ("playback controls"). Not found: any runtime with a *user-configurable* announce policy beyond these three enum values.

## 4. User evidence on voice-agent UX

- End-of-utterance is the hard part: "I decided to make him PTT by default as the problems of the model accurately guessing the end of utterance are just too great… different humans have different ways of speaking." https://news.ycombinator.com/item?id=47029721
- "latency of 500ms to 1s… good enough to have real conversations… the friction point right now tends to be either interrupting too often or waiting too long to respond." https://news.ycombinator.com/item?id=46382256
- Interrupting loses work: on Gemini's voice mode, "it spontaneously lost an entire 20-30 minutes long thread… It basically does it any time you interrupt it in any way. It's straight up data loss… keeps asking annoying questions which you can't make it stop." https://news.ycombinator.com/item?id=46236896
- Silence while working: "In most voice agents today… the entire conversation pauses until that request finishes. Even a few seconds of silence feels broken on a call." https://news.ycombinator.com/item?id=46760355
- Being cut off — Reddit threads (titles; bodies blocked): "Voice chat cuts me off constantly" https://www.reddit.com/r/ChatGPT/comments/1cfvx91/ ; "ChatGPT voice cutting you off" https://www.reddit.com/r/ChatGPT/comments/183oz3c/ ; "Voice constantly interrupting" https://www.reddit.com/r/ChatGPT/comments/17kesq4/ ; "Stop ChatGPT voice from interrupting" https://www.reddit.com/r/ChatGPTPro/comments/1772ywp/ . HN workaround: "hold down the circular icon while you're speaking… and it won't interrupt you." https://news.ycombinator.com/item?id=45362956
- Explicit turn control is accepted: Claude's voice "tap anywhere… to indicate that you're finished… If you are silent for several seconds, it goes ahead… you can tap on the screen to interrupt; that is actually quite useful, as it can be a bit long-winded." https://news.ycombinator.com/item?id=44169472
- Verbosity: "controlling the length of response is an inherently difficult problem in voice conversations… some kind of response length parameter/slider would be helpful." https://news.ycombinator.com/item?id=49277853
- Repeating oneself ends the call: plumber's phone agent "does not understand my home address and keeps asking me over and over to repeat myself… no option to speak to a person, so I hung up." https://news.ycombinator.com/item?id=46570170
- Short confirmations unheard: pipecat #984 (+11) "short utterances like 'OK', 'Yes', 'No' aren't 'heard' by the bot… start_secs of 0.2 seconds"; lowering it risks "interruptions triggering unexpectedly". https://github.com/pipecat-ai/pipecat/issues/984
- Agent interrupting the user: LiveKit #3427 "interruption logic is shared across the agent's thinking and speaking states and can't be tuned independently… not possible to make the agent that is hard to interrupt but still friendly to slow speakers." https://github.com/livekit/agents/issues/3427 ; #3903 agent's first frames collide with the user's second sentence https://github.com/livekit/agents/issues/3903 ; #1229 a cough before the reply → agent goes silent until "hey are you still there?" https://github.com/livekit/agents/issues/1229
- Hearing things it should not: #824 "When the agent is speaking with allow_interruption=False, we should not be processing any user input, instead of queuing up another response… the user would not be expecting that the agent heard it." https://github.com/livekit/agents/issues/824
- Robotic cut-off: pipecat #1305 "The hard cut-off of actively speaking TTS makes it sound very robotic… should be tapered off." https://github.com/pipecat-ai/pipecat/issues/1305 ; agent idling until the user asks "are you still there" #1605 (19 comments) https://github.com/pipecat-ai/pipecat/issues/1605
- Realtime console: "stop" transcribed as empty → "cause the assistant to start over, not stop"; fix was filling every `turn_detection` field because defaults were "too sensitive". https://github.com/openai/openai-realtime-console/issues/35 ; request "Add silence/turn-taking detection so Realtime Chat Agent knows when not to speak" https://github.com/openai/openai-realtime-console/issues/559
- Leaking internals into speech: Gemini realtime agent "saying words like 'tools_output' during function-call". https://github.com/livekit/agents/issues/2174
- Trust in permission prompts spoken aloud: **not found** in user threads. Nearest primary evidence is design-side: the paper's preview-and-commit pause (§3.3), GPT-Live "your application checks permissions, obtains required confirmations" (https://developers.openai.com/api/docs/guides/live), and qwen #71 "never sleep while awaiting permission".

## 5. Numbers worth remembering

| Quantity | Value | Source |
|---|---|---|
| Human conversational response gap | ~500 ms "typical" | https://voiceaiandvoiceagents.com/ |
| Voice-to-voice target that users accept | ≤ 1,500 ms; "critical not to have latency spikes… as you add tool calling" | same |
| Typical cloud cascade breakdown | STT+endpointing 300, LLM TTFB 650, TTS TTFB 120, total ≈ 1,293 ms; 500 ms achievable co-located | same |
| LLM TTFT rule of thumb | ≤ 600 ms | same |
| Practitioner-observed | 500 ms–1 s "good enough" | HN 46382256 |
| Moshi full-duplex latency | 160 ms theoretical / 200 ms practice | https://arxiv.org/abs/2410.00037 |
| OpenAI server VAD defaults | prefix 300 ms, silence 500 ms | https://platform.openai.com/docs/guides/realtime-vad |
| LiveKit EOT prerequisites | VAD min_silence ≥ 250 ms; Silero default 550 ms | https://docs.livekit.io/agents/logic/turns/turn-detector/ |
| LiveKit text EOT model | Qwen2.5-0.5B, 396 MB, 50–160 ms CPU, TPR 99.3 %, TNR 85–96 % | same |
| Smart Turn v3 | ~8 M params, ~65 ms CPU, ≤ 8 s window | https://github.com/pipecat-ai/smart-turn |
| pipecat VAD start threshold | 0.2 s default; 0.1–0.15 s misses fewer "OK"s | https://github.com/pipecat-ai/pipecat/issues/984 |
| qwen quick-lane cap (proposed) | 15 s, then promote + tell the user | https://github.com/QwenAudio/qwen-audio-agent/issues/88 |
| qwen paper delegation overhead | 4.27 s vs 1.49 s on short tasks | https://arxiv.org/html/2609.25195 |
| qwen paper multi-step | 43 s (mixed) vs 68 s (direct) | same |
| Sesame CSM sizes | 1B/100M, 3B/250M, 8B/300M | https://www.sesame.com/research/crossing_the_uncanny_valley_of_voice |
| GPT-Live voice | $0.05/min billed per second → $3.00/hour, backend extra | https://developers.openai.com/api/docs/models/gpt-live-1 |
| gpt-realtime-2.1 | audio $32 in / $64 out per 1 M tokens; mini $10 / $20 | https://developers.openai.com/api/docs/pricing |
| gpt-live-transcribe | $0.017/min ≈ $1.02/hour | same |
| Gemini 3.8 Live | audio in $3.00, audio out $12.00, text in $0.75, text out $4.50 per 1 M | https://cloud.google.com/gemini-enterprise-agent-platform/generative-ai/pricing |
| Not found | Qwen Audio 3.0 Realtime list price (only the complaint #38); Deepgram Flux/EOT numbers |  |

## 6. Implications for a single-model local talk mode

1. **Separate the four states explicitly** — speech-interrupt vs task-cancel, and task-done vs result-spoken — as runtime state, not prompt hints; the paper (§2.2), GPT-Live ("Stop talking" vs "Cancel my booking") and LiveKit ("interrupting the agent doesn't cancel the work") all converge here.
2. **Hold results while the user speaks or a reply plays, then batch** what arrived in the same window into one utterance (paper §2.2; Gemini `WHEN_IDLE`). Never `INTERRUPT`-announce by default.
3. **Give every background result a delivery policy** of exactly three values — silent / when idle / interrupt — chosen by the tool or the orchestrator (Gemini scheduling; pipecat #4118). Silent is needed for "the environment changed" events (paper §2.3).
4. **Fillers only when idle, with a delay** — LiveKit's `with_filler(delay=5)` plays only after N s of continuous silence so it never talks over the user; the qwen quick-lane bridge is "a natural half-sentence". A filler is not a substitute for the result: "the user hears 'task accepted' instead of an answer" (#88).
5. **Add a fast read-only lane with a hard cap and promotion** — answer sub-15 s lookups inline, promote to a task on timeout and *say so* (#88); the paper shows delegation costs ~2.8 s even on short tasks.
6. **Make interruptions cheap to recover from**: truncate history to what was heard (LiveKit), taper audio (pipecat #1305), drop dangling announcements on interrupt (#88), and never lose the thread — the Gemini-mode complaint is "straight up data loss" (HN 46236896).
7. **Distinguish backchannels from barge-in** (Vocode low/high; LiveKit adaptive; GPT-Live "a brief listening sound is different from taking over the user's turn") and give **thinking and speaking separate sensitivity** (LiveKit #3427).
8. **Resume after false interruptions** (empty transcript → continue from where it stopped, LiveKit) and don't go deaf after a cough (LiveKit #1229, #5674's "~600 ms deaf window").
9. **While speaking uninterruptibly, do not ingest user audio as a new turn** (LiveKit #824) — the user did not expect to be heard.
10. **Turn detection: pair a VAD with a tiny semantic EOT model**, run on the whole turn after silence (Smart Turn v3 ≈ 8 M params, 65 ms; LiveKit EOT needs ≥ 250 ms VAD silence). Expose an eagerness knob (OpenAI `eagerness`, Vocode `endpointing_sensitivity`) and consider adapting to the speaker's pace (Vocode speed coefficient, LiveKit dynamic endpointing) — practitioners say individual speech patterns are the unsolved part (HN 47029721).
11. **Keep push/hold-to-talk and tap-to-finish as first-class options**, not fallbacks — users praise them (HN 44169472, 45362956) and one builder made PTT the default.
12. **Short confirmations must be heard**: tune VAD start below 0.2 s or use a word-level path for "yes/no/stop" (pipecat #984; realtime-console #35 where "stop" restarted the answer).
13. **Budget latency as a whole** and watch it when tools are added: ≤ 1,500 ms voice-to-voice, TTFT ≤ 600 ms, no spikes (Daily guide); the local advantage is the 650 ms LLM TTFB and 300 ms STT stages, not the network.
14. **Keep spoken replies short and bounded**, and say when the full answer is on screen — long spoken answers were cut at 200–300 chars for qwen users (#117) and HN users want a length control (49277853).
15. **Avoid** blocking the mic during playback as a substitute for echo cancellation (HF s2s flag kills barge-in), speaking internal tokens (LiveKit #2174), and single-model hallucinated tool results when a call is still pending (OpenAI dev notes) — in a single-model design, the pending-call placeholder must be explicit in context (pipecat #4657's "running"/"finished" rendering).
16. **Permission prompts spoken aloud**: no user evidence found; the only patterns are preview-and-commit with the task paused and the decision routed back to that task (paper §3.3; GPT-Live app-owned confirmations). Treat as an open question to test, not a settled rule.
All six sections are complete and verified against their sources; the report in my previous message is the deliverable (≤30-line summary first, then the full ~340-line survey with 27 distinct sources, every claim linked, "not found" marked where applicable). Nothing remains blocked.