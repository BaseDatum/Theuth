# qwen-audio-agent vs theuth talk — what to borrow (2026-10-08)

Recon of QwenAudio/qwen-audio-agent (HEAD 84490d46, v2.0.1, Apache-2.0, 2,860★) against theuth's talk feature (main a109c4fc → cb944b75 during the pass). Three read-only workers, 14 minutes: their repo (686 lines, 44/44 cites), our stack (675 lines, 25/25), a web survey (27 sources). The orchestrator re-opened 27/28 load-bearing citations in both trees. Techniques only — nothing is copied.

## 1. The two bets

| | qwen-audio-agent | theuth talk (design 76) |
|---|---|---|
| Who talks | A cloud speech-to-speech model (Qwen Audio 3.0 Realtime, GPT-Live, Gemini Live…) behind a fixed 93-line prompt and 6 core tools; may not pick sessions, tools or execution mode | The session's own model, with every tool, exactly as if typed |
| Who works | One ACP coding agent (Qwen Code, OpenCode, Claude Code…) in a fixed per-owner session | The same model, inline; subagents/jobs when it chooses to delegate |
| Audio | The provider's VAD/STT/TTS; audio leaves the machine; no AEC beyond the browser | Local TEN-VAD + Nemotron + Pocket TTS + VPIO AEC; daemon owns mic and speaker |
| While work runs | Conversation continues; results re-enter at a safe window | One narration sentence, then silence; speech cancels the turn |
| Interruption | Cuts speech only; accepted work never cancelled | Cuts speech and the turn (children survive via `CancelScope::Root`) |
| Instrumented | No latency logs at all; paper measures task latency | First-sentence / first-audio logs; 1.9–3.3 s budget |

The paper's own number says why not to copy the split: delegation costs ~2.8 s even on short tasks (4.27 s vs 1.49 s); mixed routing wins (91.04 % vs 72.39 % direct / 80.60 % all-delegated) precisely because short things stay inline. theuth already runs mixed routing with one model — what it lacks is their runtime etiquette around the long things.

## 2. Axis by axis

| Axis | Theirs | Ours (built) | Live plan |
|---|---|---|---|
| Turn-taking | Generation counter; interrupt = advance boundary, never a task cancel | Two-stage barge-in, five cuts, heard-prefix history; speech while Thinking cancels + resubmits | Smart Turn slot exists, model not built |
| Background work | `spawn_thinking` returns at once; task record; status reads the record | Everything is one blocking turn; `shell()` alone promotes to a job | 77 fillers B/C documented, not built |
| Result delivery | Window `userSpeaking‖turnPending‖audio queued`; ≤8 items/6000 chars/120 ms batches; delivered on `playback.started`; 60 s claims | Job/reminder/child results start a turn whose reply is spoken ungated; receipts exist on both sinks | nothing |
| Progress | Visual only; spoken progress only from explicit messages ≥60 s apart | `Working…` phase line; nothing spoken | 77 B: neutral filler 700 ms after submit |
| Permissions | `respond_permission`: short id, bare yes/no when one pending, invalid never falls back, grants die with task | Card only; phone with screen off hears silence | nothing |
| Cancellation | Confirmed (`cancelling` state) | `Cancel{Root}`; tool bodies cannot be stopped (`ToolCx` has no token) | OPEN since 76 §12 |
| Recovery | Journal + replay; queued/running tasks fail on restart, delegated reattach | Actor state in memory; talk ends on restart | nothing |
| Clients | One active client per owner; claims protect reconnects | Desktop, TUI, phone via relay from one contract; one talk per daemon | multi-device not designed |
| Mic | Ref-counted TTL holds (15 s default) | Utterer owned by one feature at a time | talk-in-meeting lease OPEN |
| Wake/sleep | Local sherpa-onnx wake word; sleep predicate never cancels work | none | none |
| Evals | Deterministic boundary suite in CI without a model | ~20 pure policy tests | no barge-in/EOU/WER corpus (76 §13) |

## 3. What users notice (survey)
- Their sharpest complaint is "task accepted instead of an answer" → issue #88 proposes a 15 s read-only quick lane, a half-sentence bridge, promotion with an explicit "this is taking longer".
- The field converged on three delivery policies: Gemini Live `SILENT / WHEN_IDLE / INTERRUPT`; LiveKit `ctx.update()` / `with_filler(delay=5)` (plays only after N s of idle, "so they don't talk over the user") / `foreground()`.
- Barge-in is the #1 failure surface everywhere: too sensitive (LiveKit #3427), no recovery from a false interrupt (#1229), hearing input it should not (#824), hard cut-off sounds robotic (pipecat #1305), short "yes/no/stop" unheard (pipecat #984). Interrupting Gemini "lost a 20–30 minute thread — straight up data loss".
- Numbers: ≤1,500 ms voice-to-voice is accepted, LLM TTFT ≤600 ms, no spikes when tools are added; Smart Turn v3 ≈ 8 M params / 65 ms CPU; GPT-Live $3/h; Moshi 200 ms.
- Push/hold-to-talk stays valued by users; no user evidence at all on spoken permission prompts.

## 4. Borrows, ranked, by where they land

**Policy + prompt (`crates/talk`, wave A, ~1 day)**
1. **Interruption ≠ cancellation.** Speech while a tool batch or child runs → `agent::Delivery::Steer` (already in `crates/agent/src/lib.rs#L153-L160`, never used by talk) instead of `Cancel + NextTurn` (`policy.rs#L743-L762`). Speech is still cut and history trimmed; the work continues; "stop that" stays an explicit cancel. Needs a Working state distinct from Thinking.
2. **Progress policy.** Activity stays visual; spoken progress only from explicit progress notes, ≤1/min; 77's filler B gated on idle silence, not a fixed 700 ms.
3. **Envelope guard.** Never speak `<job_result>`, `<reminder>`, `<message>` or tool tokens; one-shot correction (their `reserved-protocol-envelope.mjs#L4-L36`).
4. **Boundary invariants as tests**: late audio after interrupt suppressed, one `Interrupted`, no duplicate speech, no re-speak of a result after interrupt, envelope never spoken.
5. **Spoken-length rule**: one-sentence version aloud, "the rest is on screen" (their #117 cut-offs).

**Engine (`crates/agent`, `crates/tools`, wave B, ~2 days)**
6. **Safe insertion window + delivery policy.** New policy inputs for job results, reminders, child messages and schedule runs; speak only when `!user_speaking && !turn_pending && queue empty` (their `announcement-window.mjs#L78-L84`); each item `silent | when_idle | interrupt`, default `when_idle`; coalesce arrivals within ~120 ms into one turn; delivered = first `SpeechStarted` / `PlayedSeq` (already built).
7. **Talk doctrine: answer or delegate; promote past N seconds and say so.** Generalise `shell()`'s few-seconds promotion to every tool in talk (`talk.promote_after_ms`): the call detaches into a job, the turn ends with "I'll tell you when it's done". This is the home for the `ToolCx` cancel token (76 §12).
8. **Confirmed cancellation** — a `cancelling` state for jobs/children, shown in the phase line.

**Design first (wave C)**
9. **Spoken approvals**: bounded title/command aloud (secrets redacted), bare yes/no when exactly one is pending, short id otherwise, invalid id never falls back, grants die with the turn; `double` answers first where allowed. Untested territory — no user evidence.
10. **Mic arbitration as ref-counted TTL holds** — the shape for the open talk-in-meeting Utterer lease.

**Later**
11. Sleep predicate + local wake word (sherpa-onnx KWS; theuth already runs ONNX; sleep must never cancel work). 12. Turn generation counter only if 6's queue needs a stale check.

## 5. Do not borrow
Speech-to-speech provider layer and server VAD (76 §0.1 rejected hosted audio; still right). Response-slot recovery by socket disconnect. A separate Gateway process and client protocol (the engine is the gateway). Fixed per-owner backend session (conflicts with per-task children). USER.md/MEMORY.md/ASSISTANT.md file triad (knowledge + prompt.md already; the authority split is the only idea). Pet skins, mobile pairing.

## 6. Their weaknesses
Cloud-only audio (price complaints #38); zero latency instrumentation; restart loses queued/running tasks and all grants; 120 s / 8-retry ack path can park a result for minutes; Chinese-only prompt boundary; one client per owner so "claims" mostly protect reconnects; docs say one task per owner, scheduler allows two; benchmark is in-house, 6 multi-step latency turns, and "conversational behavior during task execution" is admitted future work — the very thing the tagline promises.

## 7. Open questions
1. Steer-not-cancel for speech during Working (item 1) — or keep 76 §0.9's cancel and only add the announcement path?
2. Default delivery policy for child results and job results: `when_idle` spoken, or `silent` into context with a chime and the sidebar glyph?
3. Spoken approvals: design it now (phone-with-screen-off case) or wait for a real complaint?
4. Promotion threshold for inline tools in talk (their users asked for 15 s; `shell()` uses a few seconds).

## Sources
`~/.theuth/briefs/2026-10-08/qwen-audio-agent/` — recon-theirs.md, recon-ours.md, survey-voice.md, AXES.md, the three briefs. Knowledge: facts/qwen-audio-agent-recon-2026-10-08, facts/theuth-talk-substrate-vs-voice-runtime-model-2026-10-08, facts/theuth-borrow-list-from-qwen-audio-agent-2026-10-08. Paper: arXiv 2609.25195. Clone: /tmp/qwen-audio-agent @ 84490d46.
