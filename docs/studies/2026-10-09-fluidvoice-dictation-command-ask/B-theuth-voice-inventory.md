# OURS — theuth voice inventory (dictation · command mode · ask-anywhere voice)

Analyst: fv-ours (read-only). Tree `/Users/Sky.Moore/Develop/theuth`. **Note on the base:** main moved from `4bd11af3`
to `cfe7eb13` (Merge fix/149w-spoken) while this ran; `git diff --stat 4bd11af3 HEAD` touches only
`crates/desktop/companion-windows/**` + `docs/design/149-as-built-spoken-replies-windows.md`, so every Rust/Swift/TS cite
below is byte-identical on `4bd11af3`. Nothing built or run.

## Executive summary — the ten biggest gaps vs the axes

1. **No transcript shaping at all.** Text goes model → UI verbatim; the only edits are dropping a stray leading
   punctuation mark and SentencePiece spacing (`crates/voice/src/live.rs#L218-L233`). No spoken punctuation, no ITN, no
   "new line", no LLM cleanup, no dictionary. (A3)
2. **No vocabulary biasing / hotwords.** `Stream::feed` takes a window and returns text; nothing else goes in
   (`crates/voice/src/stt.rs#L237-L266`); repo-wide grep for hotword/vocabulary/biasing finds nothing in voice. (A2/A3)
3. **No dictation into other apps.** Every surface splices into its own field: desktop `splice()` (`crates/desktop/ui/src/lib/dictation.ts#L42-L53`),
   companion `HoldToTalk.onText` (`crates/desktop/companion/Sources/Companion/Voice/HoldToTalk.swift#L5-L8`). The only
   cross-app typing is the agent's `computer_type` tool (`crates/computer/src/tools.rs#L332-L350`), never a dictation sink. (A4)
4. **No write/rewrite mode** (select text anywhere → spoken instruction → replaced inline). Not found; the desktop field
   only *replaces a selection with dictated words* (`crates/desktop/ui/src/lib/dictation.ts#L211-L215`). (A4)
5. **No command mode outside the agent turn.** The voice key always becomes a desk turn (`send_input`/`steer`)
   (`crates/desktop/companion/Sources/Companion/PointAndSay/PointController.swift#L288-L304`); no local intent parser,
   no AppleScript/Shortcuts tool (grep `osascript|AppleScript|NSAppleScript` → only the updater, `crates/desktop/src/update.rs#L78`). (A5)
6. **No echo cancellation / noise suppression on dictation.** Dictation opens `Capture::open()` = `Aec::None`
   (`crates/voice/src/dictation.rs#L249`, `crates/voice/src/mic.rs#L59-L61`); VPIO is used only by talk and meetings
   (`crates/talk/src/lib.rs#L860-L863`, `crates/meeting/src/capture/mic.rs#L39`). The companion's own capture is a plain
   `AVAudioEngine` tap (`crates/desktop/companion/Sources/Companion/Voice/VoiceRecorder.swift#L36-L45`). (A1)
7. **No input-device selection.** The daemon opens "the default input" (`crates/voice/src/mic.rs#L54-L59`); config has
   an output device but no input device (`crates/voice/src/lib.rs#L443-L447`). Device loss ends the dictation
   (`crates/voice/src/mic.rs#L163`). (A1)
8. **No dictation history, stats or feedback loop.** Dictation leaves no audio or transcript record; a finished
   dictation is just the field value (`crates/desktop/ui/src/lib/dictation.ts#L245-L251`). Not found: raw-vs-edited
   diff, correction capture, usage stats. (A7)
9. **The ask-anywhere answer is thin.** The Ask bar receipt shows only "the answer's first two lines"
   (`crates/desktop/companion/Sources/CompanionCore/AskRouter.swift#L108-L118`); no in-place follow-up thread, one
   fixed target (theuth's desk, "No agent picker", `AskRouter.swift#L34`). (A5/A6, Q3)
10. **No published latency numbers and no dictation eval.** 54 has no "latency" passage (grep); first text is
    structurally ≥ onset 160 ms + one 560 ms window (`crates/voice/src/vad.rs#L106-L118`, `crates/voice/src/stt.rs#L32-L34`)
    [INFERENCE]; no WER/latency harness for dictation found (A8).

Strongest things we already have (for contrast): one engine-owned streaming STT for every surface
(`crates/rpc/src/api.rs#L6267-L6326`); append-only partials rendered muted→final (54 §0.3,
`docs/design/54-dictation.md#L57-L68`); a real point-and-say screen context (`[screen …]` line with pointer trail + AX
element, `crates/computer/src/context.rs#L585`); a full agent with local action tools behind the voice key
(`crates/computer/src/tools.rs#L143-L432`); spoken replies through client-sink TTS (`crates/desktop/companion/Sources/Companion/PointAndSay/SpeechPlayer.swift#L4-L5`).

## A1 Capture front-end

### (a) What exists
- **Who owns the mic.** Design 54 decision 2: "The daemon owns the microphone, natively, for every frontend"
  (`docs/design/54-dictation.md#L35-L52`). `voice::mic` opens the default input through `flexaudio` at 48 kHz mono,
  20 ms chunks of 960 samples (`crates/voice/src/mic.rs#L21-L29`), and the dictation actor picks it for
  `AudioSource::Mic` (`crates/voice/src/dictation.rs#L56-L68`, `#L249`).
- **Remote sources.** `AudioSource::Remote { sample_rate }` takes mono i16 PCM pushed by `dictation_audio`, any multiple
  of 50 Hz in 8–192 kHz (`crates/voice/src/dictation.rs#L60-L67`, `#L79-L88`), rechunked to 20 ms
  (`crates/voice/src/dictation.rs#L123-L130`). Used by:
  - the desktop/mobile webview when the engine is relay-linked: browser mic → tap → "100 ms i16 batches →
    `dictation_audio`" (`crates/desktop/ui/src/lib/dictation.ts#L277-L278`); the daemon mic refuses relay peers
    (`crates/rpc/src/api.rs#L8847-L8864`);
  - the macOS companion: `VoiceRecorder` taps `AVAudioEngine` input and converts to 16 kHz Int16
    (`crates/desktop/companion/Sources/Companion/Voice/VoiceRecorder.swift#L8`, `#L36-L45`), pushed by `HoldToTalk`
    after `dictationStart()` (`crates/desktop/companion/Sources/Companion/Voice/HoldToTalk.swift#L128-L145`).
  - the Windows companion has the twin (`crates/desktop/companion-windows/src/Theuth.Companion.Core/AskAudio.cs`,
    `AskVoice.cs`, `DictationLease.cs` — file list from grep, not read line by line).
- **One mic at a time.** `DictationState.active` "one per daemon: one mic" (`crates/voice/src/dictation.rs#L103-L110`);
  a talk leases the same `Utterer` (`crates/voice/src/dictation.rs#L38-L48`); refusals surface as "dictating elsewhere"
  / "Meeting running — use the island to ask" (`crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L114-L116`, `#L148-L152`).
- **Audio processing.** `Aec` enum: `Vpio` (Apple VoiceProcessingIO) or `None` (`crates/voice/src/mic.rs#L31-L43`);
  VPIO sets ducking of other audio (`crates/voice/src/mic.rs#L234-L239`). Dictation uses `None`
  (`crates/voice/src/mic.rs#L59-L61`, `crates/voice/src/dictation.rs#L249`). Talk prefers VPIO
  (`crates/talk/src/lib.rs#L860-L863`); meetings force it (`crates/meeting/src/capture/mic.rs#L39`).
- **VAD / end of utterance.** TEN-VAD via wavekat (54 decision 4, `docs/design/54-dictation.md#L69-L73`); gate constants:
  pre-roll 20 frames (320 ms), onset 10 frames (160 ms), close 19 frames (≈300 ms), max utterance 15 s, turn boundary 2 s,
  threshold 0.5 (`crates/voice/src/vad.rs#L106-L118`). 256-sample frames at 16 kHz (`crates/voice/src/vad.rs#L16-L19`).
  Pipeline: resample → VAD per frame → gate edges → onset opens with pre-roll, close finishes, turn resets stream+VAD
  (`crates/voice/src/live.rs#L144-L174`).
- **Stop conditions.** User stop; `dictation_max_secs` deadline (default 300 s; `crates/voice/src/lib.rs#L442-L443`,
  `#L459`; checked in the task loop `crates/voice/src/dictation.rs#L566-L573`); device/model error. Companion caps a
  hold at 60 s (`crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L37-L38`).
- **Readiness / warm-up.** `Prewarm` loads model+VAD without opening the mic (`crates/voice/src/dictation.rs#L33-L36`);
  `StartDictation` waits for an in-flight load rather than failing (`crates/voice/src/dictation.rs#L70-L77`).
- **Idle/unload.** The streaming model (≈0.9 GB int8) "is never idle-unloaded" (`crates/voice/src/dictation.rs#L9-L11`);
  the file-STT TDT model unloads after `stt_idle_secs` = 600 (`crates/voice/src/lib.rs#L438`, `#L457`).
- **Metering.** RMS per 20 ms chunk, sent as `DictationLevel` every 8 chunks = 160 ms (`crates/voice/src/dictation.rs#L28-L29`,
  `#L139-L140`, RMS at `#L614`); desktop keeps the last 24 (`crates/desktop/ui/src/lib/dictation.ts#L26-L27`, `#L38`).

### (b) Missing vs the axis
- No AEC/NS/AGC on dictation (VPIO exists in-crate but is not used for it). No WebRTC APM.
- No input device picker, no device-change follow (DeviceLost = end, `crates/voice/src/mic.rs#L163`).
- No measured press→first-sample→first-word numbers in code or 54 (grep "latency" in `docs/design/54-dictation.md`: no match).
  Structural floor [INFERENCE]: onset confirm 160 ms + first full 560 ms window ⇒ first partial ≈ 0.7 s after speech start,
  plus RPC/event hop; companion adds the 250 ms hold threshold before the mic even opens
  (`crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L35-L36`, `#L52-L54`).
- No spectrum display (RMS only).

### (c) Designs
54 (built, decisions 2/4), 76 §0.5 AEC for talk (built), 145 talk presence (BUILT 2026-10-08, `docs/design/145-talk-presence.md#L3`).
No design promises AEC or device selection for dictation.

## A2 STT engines and streaming

### (a) What exists
- **Streaming (dictation, talk, live meeting):** `nemotron-speech-streaming-en-0.6b` int8 via `parakeet_rs::Nemotron`,
  wrapped as `voice::stt::Stream` (`docs/design/54-dictation.md#L57-L68`, `crates/voice/src/stt.rs#L237-L253`).
  Window = 8 960 samples = 560 ms at 16 kHz ("56 mel frames × 160 hop … 35 VAD frames") (`crates/voice/src/stt.rs#L32-L34`).
  Model dir `nemotron-streaming-en` (`crates/voice/src/lib.rs#L54`); fetched from `hf:lokkju/nemotron-speech-streaming-en-0.6b-int8`
  (`scripts/fetch-models.sh#L51-L53`, `#L76`).
- **Partial vs final.** The model emits only new, punctuated, cased text, append-only (54 decision 3). `feed_full`
  pushes a non-final piece after every window with new text (`crates/voice/src/live.rs#L176-L193`); `finish` zero-pads
  the tail, feeds one extra zero window to drain lookahead and emits the `final` piece (`crates/voice/src/live.rs#L195-L216`).
- **Rendering.** Desktop: `Utt { text, final }`, "`final: false` renders muted" (`crates/desktop/ui/src/lib/dictation.ts#L16-L17`);
  reducer over the five `Dictation*` events (`crates/desktop/ui/src/lib/dictation.ts#L55-L84`). Companion: `liveText` =
  finals + open partial (`crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L77-L79`, `#L135-L136`); the island
  voice view draws finals primary, partial tertiary (`crates/desktop/companion/Sources/Companion/Island/VoiceModeView.swift#L5-L16`).
  TUI: own splice + header meter (`crates/tui/src/dictation.rs#L1-L2`).
- **File / voice-note STT:** Parakeet TDT 0.6B (v3 int8 smoothquant) `Engine::transcribe`, pieces of 180 s
  (`crates/voice/src/stt.rs#L27-L29`, `#L170-L187`); origin "hub id not recorded" (`scripts/fetch-models.sh#L75`).
- **Catalog / download / verify:** `scripts/fetch-models.sh` REQUIRED list + origins (`scripts/fetch-models.sh#L28-L53`).
  Missing model → distinct error so the UI says `Missing` (`crates/voice/src/stt.rs#L240-L246`).
- **Language:** English only (`-en` model). No language selector.

### (b) Missing vs the axis
- No vocabulary biasing / hotwords / context boosting (the `Stream::feed` contract has no such input,
  `crates/voice/src/stt.rs#L255-L266`).
- No separate EOU model (Parakeet realtime EOU) — EOU = VAD silence 300 ms.
- No fallback engine (Apple Speech explicitly rejected, `docs/design/54-dictation.md#L65-L68`).
- No in-app model catalog/updater for STT beyond the fetch script (not found in voice).
- No multilingual dictation.

### (c) Designs
54 (built), 45 voice notes (built), meetings 02-live (built). Field note (knowledge, not code): a 1 120 ms Nemotron export
and Parakeet realtime EOU 120M are the only alternative tiers known (`facts/theuth-stt-model-tiers-recon-2026-09-29`).

## A3 Transcript shaping (between STT and the target)

### (a) What exists
- Model-side punctuation + casing only (54 decision 3, `docs/design/54-dictation.md#L60-L62`).
- `piece()` drops a stray leading `.,?!;:` that leaks across a close into the next utterance
  (`crates/voice/src/live.rs#L218-L233`); text appended verbatim (SentencePiece spacing), `crates/voice/src/live.rs#L235-L237`.
- Splice rules: join utterances with spaces, add a leading space if needed, a trailing space unless the next char is
  whitespace/punctuation (`crates/desktop/ui/src/lib/dictation.ts#L42-L53`).
- Companion trims and refuses empty: "Nothing recognized" (`crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L64-L67`).
- **Agent-side tolerance instead of shaping:** voice-note / spoken markers tell the model "read through recognition
  slips" (`crates/voice/src/lib.rs#L170-L180`); the voice key prefixes `[voice note, N s]`
  (`crates/desktop/companion/Sources/Companion/PointAndSay/PointController.swift#L311-L314`). This is the only "AI
  post-processing" — the agent interprets, nothing rewrites the text.

### (b) Missing vs the axis
- Spoken punctuation / literal formatting ("comma", "new line", numbers, emails): **not found**.
- AI post-processing layer (prompts, gates, local vs cloud, per-app prompts): **not found**.
- Custom dictionary, automatic pronunciation learning, replacement protection: **not found** (grep).
- Spoken "send": **not found** as a parsed word. Closest: the companion Ask bar auto-commits through a 3 s undo hold after
  dictation ("the same 3 s hold; typing or ⎋ keeps editing", `crates/desktop/companion/Sources/Companion/AskBar/AskBarPanel.swift#L213`,
  `crates/desktop/companion/Sources/Companion/AskBar/Store+Ask.swift#L51-L55`); the voice key sends on release.
- Undo: desktop Esc discards the region and restores `before + after` exactly (`crates/desktop/ui/src/lib/dictation.ts#L306-L311`);
  companion ⎋ undoes the held send (`crates/desktop/companion/Sources/Companion/AskBar/AskBarPanel.swift#L116`).
- Quality feedback: none.

### (c) Designs
None of 54/70/94/89/96/130/149 promises shaping, a dictionary or LLM cleanup. Nothing in flight.

## A4 Delivery into the target

### (a) What exists
- **Desktop/mobile (Tauri webview):** caret anchor `before`/`after` taken at start; a selection is replaced
  (`crates/desktop/ui/src/lib/dictation.ts#L211-L215`); while dictating the field shows the live splice read-only and the
  composed value is handed back on `DictationFinished` (`crates/desktop/ui/src/lib/dictation.ts#L217-L229`, `#L245-L251`).
  The mic lives inside every shadcn `Input`/`Textarea` (`docs/design/70-as-built-DICTATION.md#L7`) via `useBox`
  (`crates/desktop/ui/src/lib/dictation.ts#L160-L208`). Composer: Ctrl+D / Mic button toggle
  (`crates/desktop/ui/src/components/chat/Composer.tsx#L418-L437`); text-on-top, voice bar below (94,
  `docs/design/94-as-built-DICTATION-LAYOUT.md#L13-L19`).
- **TUI:** in-process, same splice semantics (`crates/tui/src/dictation.rs#L1-L2`).
- **Companion:** `HoldToTalk` drops text into the surface's field via `onText` — Ask bar, card reply (R), agent row
  (`crates/desktop/companion/Sources/Companion/Voice/HoldToTalk.swift#L5-L8`, `#L98-L103`); key rows `talk` V /
  `agentsTalk` V (`crates/desktop/companion/Sources/CompanionCore/KeyBindings.swift#L223`, `#L233`). A user edit after stop
  wins over late partials (`typed`, `crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L105-L107`, `#L170-L172`).
- **Cross-app typing exists only as agent tools:** `computer_type`/`computer_key` over one enigo thread
  (`crates/computer/src/tools.rs#L332-L350`, `crates/computer/src/input.rs#L1-L9`).

### (b) Missing vs the axis
- Dictation into other apps (paste at cursor, AX `kAXSelectedText` insert, typing): **not found** (grep for
  `kAXSelectedText|AXUIElementSetAttributeValue|NSPasteboard|CGEventPost` in companion/desktop/computer returns nothing
  relevant).
- Paste verification, pasteboard preservation, secure-field / terminal policy: n/a (no delivery path).
- Write/Rewrite mode: **not found**.

### (c) Designs
54 §0.9 kept dictation inside theuth's own inputs; 96 decision 1 left Ctrl+D "untouched" in-app
(`docs/design/96-point-and-say.md#L29-L36`). No design proposes system-wide dictation.

## A5 Command mode

### (a) What exists — the voice key (point-and-say) is our command mode, via the agent
- **Trigger:** ⌃S anywhere on macOS, Ctrl+Alt+Space on Windows/Linux defaults
  (`crates/desktop/companion/Sources/CompanionCore/KeyBindings.swift#L218`, `#L251-L257`); Tauri shell fallback uses the
  same Rust machine when no companion runs (`crates/desktop/src/point/machine.rs#L6-L18`; 149 "Tauri … stand down while a
  native companion is alive", `docs/design/149-companion-native.md#L41-L43`).
- **What the message carries:** dictated text as `[voice note, N s] …` (`PointController.swift#L311-L314`) plus, when
  "Include the screen" is on (default true, `PointController.swift#L545-L548`), `screen_context`: screenshots per visited
  display as attachments and one `[screen i/n "App" · "Title" · url · pointer: circled (x,y)–(x,y): link "…" → url …;
  ended on … · accessibility: on]` line (`crates/computer/src/context.rs#L585`, example `#L735`), from the pointer trail
  sampled at 60 Hz (`crates/desktop/companion/Sources/CompanionCore/PointMachine.swift#L18`) and the AX element under it
  (toggle "Read what the pointer touched", `crates/desktop/companion/Sources/Companion/PointAndSay/VoiceKeyTab.swift#L21-L22`).
  Request type: `ScreenContextReq { samples, color, sinceMs, accessibility }` (`PointController.swift#L327-L334`,
  engine side `crates/computer/src/lib.rs#L137-L159`). Agent is told how to read it (`crates/computer/src/context.rs#L39`).
- **Which session answers:** theuth's desk, resolved by `desk_open` on the hub (or a replicated desk row off-hub);
  `steer` if the desk is mid-turn, else `send_input` (`PointController.swift#L288-L304`, `crates/desktop/companion/Sources/CompanionCore/AskRouter.swift#L51-L57`).
- **Local action tools the desk can use on the Mac:** `computer_screenshot` (read-only), `computer_move`, `computer_click`,
  `computer_drag`, `computer_scroll`, `computer_type`, `computer_key`, `computer_open` (mutating) (`crates/computer/src/tools.rs#L143-L385`),
  plus every other agent tool (shell, browser_*, MCP, ...) under `tools.policy`. No AppleScript/Shortcuts tool
  (grep: only `crates/desktop/src/update.rs#L78`).
- **How the result shows:** `AnswerCard` — "the desk's streamed reply … 'theuth · working…', tool names as they start, the
  final text, 'Open O · ⎋ dismiss'" (`crates/desktop/companion/Sources/Companion/PointAndSay/AnswerCard.swift#L6-L15`),
  shown near the pointer (`PointController.swift#L305-L307`).
- **Spoken:** `speak` RPC (`crates/rpc/src/api.rs#L1557`) with a client sink: Opus packets as `SpeechAudio` events played by
  the companion (`crates/desktop/companion/Sources/Companion/PointAndSay/SpeechPlayer.swift#L4-L5`, `#L27-L31`); macOS
  setting key `voiceKeySpeak` (`PointController.swift#L545-L546`).

### (b) Missing vs the axis
- No command mode that bypasses the LLM turn (no intent grammar, no app search/launch shortcut layer, no "open X" fast path).
- No model picker for voice requests; no per-request agent choice ("No agent picker", `AskRouter.swift#L34`).
- No confirmation UX specific to voice actions beyond the normal approval policy.
- No command history/sessions view of voice requests apart from the desk session itself (`askOpenSession` ⌘O opens it,
  `KeyBindings.swift#L213`).
- No local HTTP/API surface for voice (RPC only).

### (c) Designs
96 point-and-say (built; gesture decision `docs/design/96-point-and-say.md#L49-L61`), 130 companion (built), 149 native
voice key (built; Windows twin closed), 149-as-built-voice-key-fix(-windows) (built). In flight: `fix/149-spoken-replies`
(fc1fbf42 "spoken replies as two opt-in settings (voiceKeySpeak default false, askSpeak new)") — macOS twin of the Windows
change already on main (cfe7eb13).

## A6 Overlay, hotkey and gesture UX

### (a) What exists
- **Hotkeys (macOS companion defaults):** panel ⌃⇧Space "Open the panel · hold to talk"; Ask bar unbound by default;
  voice key ⌃S; meeting PTT ⌥`; meeting eye ⌥⇧E; panel-local V = hold to talk into the reply, P = attach the screen
  (`crates/desktop/companion/Sources/CompanionCore/KeyBindings.swift#L193-L245`). Rebindable via the recorder
  (`crates/desktop/companion/Sources/Companion/Settings/ShortcutRecorder.swift`, not read). Desktop app: `dictate` and
  `voice-key` rows in Settings › Shortcuts (`crates/desktop/ui/src/components/settings/Shortcuts.tsx#L10`).
- **Gestures.**
  - Voice key: hold ≥ 200 ms → send on release; < 200 ms opens a 300 ms double-tap window to lock; notes < 800 ms
    discarded with "Hold to talk · double-tap to lock"; done state 6 s; lost key-up guard 2.5 s
    (`crates/desktop/companion/Sources/CompanionCore/PointMachine.swift#L12-L22`, `#L113`; same in Rust
    `crates/desktop/src/point/machine.rs#L6-L18`; design `docs/design/96-point-and-say.md#L49-L61`). Esc registered only
    between press and send (`KeyBindings.swift#L243-L244`).
  - Ask bar / field hold-to-talk: press < 250 ms is a tap (toggle), ≥ 250 ms records, release stops, max 60 s
    (`crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L35-L74`). No double-tap lock here.
  - Desktop Composer: Ctrl+D toggle (`Composer.tsx#L418-L437`), Esc discards (`dictation.ts#L306-L311`).
- **Overlays.** Voice-mode view on the island: rec dot, live words, "· ⌃S held · theuth's desk", lock, hint; phases
  listening/locked/sending/hint (`crates/desktop/companion/Sources/Companion/Island/VoiceModeView.swift#L5-L16`) — the
  96 §0.3 "reuse the island" rule is honoured in code. Pointer trail overlay windows
  (`crates/desktop/companion/Sources/Companion/PointAndSay/OverlayWindows.swift`, not read). Answer card near the pointer.
- **State per surface (what the user sees):**
  - Voice key: idle → listening (island voice view with live words + level) → locked (lock glyph) → sending → AnswerCard
    "working…" / "working · ‹tool›" → "replied" / "couldn't reply — …" (`AnswerCard.swift#L10-L12`); failure before send →
    pill hint (`PointController.swift#L316-L321`).
  - Ask bar hold: status "listening…" → "finishing…" → text in field → 3 s hold → receipt "theuth · …"
    (`HoldToTalk.swift#L84-L103`, `AskBarPanel.swift#L213`, `AskRouter.swift#L108-L118`); refusals as status text
    (`HoldToTalk.swift#L14`).
  - Desktop: wave bar below full-width text, Square stop glyph, tooltip "Stop dictation (Ctrl+D)" (`docs/design/94-as-built-DICTATION-LAYOUT.md#L15`).
- **Sounds:** none found for dictation start/stop (not grepped exhaustively — flag as unverified).

### (b) Missing vs the axis
- No "enhancing" state (no shaping step to show).
- No dictation overlay for typing into other apps (no such path).
- Overlay screen choice: island remembers its place per display (149 summary, `docs/design/149-companion-native.md#L38-L41`);
  per-pointer display for the voice view not verified.
- Fn / modifier-only chords explicitly rejected (event tap needed) (`docs/design/96-point-and-say.md#L45-L48`).
- Onboarding tryout: not found.

### (c) Designs
89 VOICE-HOLD (built; composer voice notes), 96 (built), 130/134 (built), 149 + as-builts (built). In flight on the seam:
`fix/149-island` (ede34e08, island clicks/stop/open-no-hover probe), `fix/149w-island` (17d5cc03 Windows twin),
`fix/149-spoken-replies` (fc1fbf42/8b8ea7fd). Rulings: island open while a meeting is live, no hover collapse, spoken
replies opt-in, panel key = panel (`decisions/theuth-companion-island-spoken-replies-panel-key-rulings-2026-10-09`).
`docs/design/151*` is not on main; 151 (worktrees `theuth-151-*`) is the autoresearch draft panel ("draft_edit accepts
leaf include_dirty …", `git log` of `integrate/151`) — **not voice**.

## A7 History, stats and the feedback loop

### (a) What exists
- Voice notes become chat messages `[voice note, N s] transcript` (`crates/voice/src/lib.rs#L157-L168`) — persisted with
  the session like any message. Point-and-say requests likewise land in the desk session.
- Diagnostics logs: companion `PointLog`/`AskLog` (`crates/desktop/companion/Sources/Companion/AskBar/Store+Ask.swift#L190-L192`,
  `PointController.swift#L304`, `#L318`); engine `tracing` load timing (`crates/voice/src/stt.rs#L248-L251`).

### (b) Missing vs the axis
- No audio history, no budget; no raw-vs-edited diff; no correction capture; no usage stats; no per-user improvement loop.
  (Not found in `crates/voice`, `crates/desktop/ui/src/lib/dictation.ts`, companion sources.)

### (c) Designs
None promise it. (The `decide`/double machinery learns from labels elsewhere in theuth, but nothing feeds it from dictation.)

## A8 Engineering quality and traction

### (a) What exists
- Unit tests in `crates/voice/src` (~26 `#[test]`/`#[tokio::test]` hits by grep); pure reducers with tests on every
  client: `lib/dictation.test.ts` (desktop), `AskVoiceMachine`/`AskDictation`/`PointMachine` pure Swift types designed for
  tests (`AskVoice.swift#L3-L6`), Rust `point::machine` (96 decision 2). Windows: `SpokenRepliesTests.cs` +8 tests
  (cfe7eb13 diff stat).
- Synthetic probes: `--synthetic-dictation` pushes a 440 Hz tone through `dictation_start/audio/stop`
  (`crates/desktop/companion/Sources/Companion/AskBar/AskBarInstall.swift#L61`); `--probe-point` stops before `desk_open`
  (`PointController.swift#L75`, `#L287`).
- RPC tests that the daemon mic refuses relay peers and status reaches them (`crates/rpc/src/api.rs#L8847-L8864`).
- Model freshness check script (`scripts/fetch-models.sh#L28-L32`).

### (b) Missing vs the axis
- No WER / latency eval harness for dictation; no recorded latency numbers.
- Repo health/stars n/a (private). License: no top-level LICENSE file on main (glob finds only vendored ones) — borrowing
  from GPL-3 FluidVoice must stay technique-only.

### (c) Designs
54 §10 checks (manual); 149 as-builts record probe-based acceptance.

## Map to the three questions

### Q1 Dictation today — exact pipeline per surface
- **Desktop Composer (Ctrl+D, local engine):** `dictation.start(api, before, after, mic=false)`
  (`Composer.tsx#L433-L437`) → RPC `dictation_start` (`crates/rpc/src/api.rs#L6272`) → actor opens daemon mic
  (`dictation.rs#L249`) → 48 kHz 20 ms chunks → resample 16 kHz → TEN-VAD gate → Nemotron 560 ms windows → `DictationText`
  partial/final + `DictationLevel` → reducer → field shows muted partial / full-colour final → Ctrl+D again or Esc →
  `dictation_stop` → open utterance flushed (`dictation.rs#L90-L96`) → `DictationFinished` → spliced value
  (`dictation.ts#L217-L229`).
- **Desktop/mobile on a relay-linked engine:** same, but `mic=true`: browser mic → AudioWorklet tap → 100 ms i16 base64
  batches → `dictation_audio` (`dictation.ts#L277-L278`; 54 decision 2 `docs/design/54-dictation.md#L41-L51`).
- **TUI:** in-process, daemon mic, same events (`crates/tui/src/dictation.rs#L1-L2`).
- **Companion Ask bar / card reply / agent row:** hold ≥ 250 ms → `VoiceRecorder` 16 kHz Int16 → `dictation_start`
  (Remote) + `dictation_audio` → live words in field → release → `dictation_stop` → text in field → (Ask bar) 3 s hold →
  `send_input` to the desk (`AskVoice.swift#L35-L74`, `HoldToTalk.swift#L128-L145`, `Store+Ask.swift#L51-L58`).
- **Companion voice key:** same capture, but release sends `[voice note] + [screen …] + screenshots` to the desk (A5).
- **Numbers:** model Nemotron 0.6B int8 streaming (yes); chunk 560 ms (yes, `stt.rs#L32-L34`); muted-while-open /
  full-colour-when-closed (yes, `dictation.ts#L16-L17`); VAD onset 160 ms / close 300 ms / 15 s / 2 s turn
  (`vad.rs#L106-L118`); max 300 s engine, 60 s companion; device = default input only; AEC/NS **none** on dictation;
  latency numbers **none** recorded.
- **Post-processing:** none beyond model punctuation/casing and a leading-punctuation trim (`live.rs#L218-L233`). No
  custom vocabulary path into parakeet-rs. No spoken send or spoken punctuation. **No dictation into other apps**; no
  write/rewrite.

### Q2 Command mode
- No path outside the agent turn. The voice key = desk turn with `[voice note]` text + `[screen …]` lines (pointer
  trail summary, AX element/link/url) + per-display screenshots; desk session answers via `send_input`/`steer`
  (`PointController.swift#L272-L308`).
- The desk can act with `computer_*` (8 tools, `crates/computer/src/tools.rs#L143-L385`), shell, browser and MCP; no
  AppleScript/Shortcuts.
- Reply: AnswerCard with streamed text + tool names (`AnswerCard.swift#L6-L15`); spoken via client-sink `speak`
  (`SpeechPlayer.swift#L4-L5`), now opt-in (macOS fix in flight on `fix/149-spoken-replies`; Windows on main).
- Gap for FluidVoice-style command mode: fast local intents, visible action log, confirmation, a dedicated command
  window/history.

### Q3 Ask-anywhere voice mode
- **Triggers:** macOS panel ⌃⇧Space (held = talk), Ask bar key (unbound by default), voice key ⌃S; Windows voice key
  Ctrl+Alt+Space, panel key "held ≥ 250 ms it talks" (cfe7eb13 message) (`KeyBindings.swift#L193-L257`).
- **Gesture:** voice key hold 200 / double-tap 300 lock / 800 min / Esc; Ask bar hold 250 / max 60 s, no lock.
- **Audio:** companion PCM 16 kHz → `dictation_start` + `dictation_audio` (yes).
- **Routing:** `AskRouter` → always theuth's desk, resolved once per session; off-hub needs a replicated desk row or
  refuses naming the hub (`AskRouter.swift#L34-L57`). No agent choice.
- **Display:** Ask bar receipt = first two lines of the streamed answer (`AskRouter.swift#L108-L118`); voice key =
  AnswerCard with full streamed text, "Open O · ⎋ dismiss" (`AnswerCard.swift#L6-L7`). Follow-up = open the desk
  session (⌘O) or ask again; no in-card thread. Markdown rendering in the card: not verified.
- **Spoken reply:** `speak` client sink → companion Opus player; opt-in settings (Windows on main; macOS branch in flight).
- **States:** idle → listening (island voice view / field status) → finishing → sending/"working…" → answer/"replied" →
  failed ("couldn't reply — …", or pill hint).
- **Island & voice mode:** shared — the voice view is hosted by the island (`VoiceModeView.swift#L5-L7`), as 96 intended;
  meeting briefs stay on the island; a running meeting blocks dictation ("Meeting running — use the island to ask",
  `AskVoice.swift#L150-L151`). Island click/lifecycle fixes in flight (`fix/149-island`, `fix/149w-island`).
- `98-as-built-COMMANDS.md` is Telegram bot native commands (`docs/design/98-as-built-COMMANDS.md#L1-L4`) — unrelated.

## Cites self-check

Re-opened with `read_file` at the cited lines after drafting:
- `crates/voice/src/mic.rs#L59-L61` — held (`open()` → `open_with(Aec::None)`)
- `crates/voice/src/dictation.rs#L249` — held (`AudioSource::Mic => Capture::open()`)
- `crates/voice/src/dictation.rs#L566-L573` — held (deadline loop)
- `crates/voice/src/vad.rs#L108` — held (`PREROLL = 20`)
- `crates/voice/src/stt.rs#L34` — held (`WINDOW = 8_960`)
- `crates/voice/src/lib.rs#L443-L447` — held (max secs, player_device; no input device)
- `crates/voice/src/live.rs#L225` — held (leading punctuation trim)
- `crates/desktop/ui/src/lib/dictation.ts#L278` — held (`dictation_audio` batches)
- `crates/desktop/companion/Sources/CompanionCore/AskVoice.swift#L36-L38` — held (0.25 s / 60 s)
- `crates/desktop/companion/Sources/CompanionCore/KeyBindings.swift#L218` — held (⌃S voice key)
- `crates/desktop/companion/Sources/CompanionCore/AskRouter.swift#L34` — held ("No agent picker")
- `crates/desktop/companion/Sources/Companion/PointAndSay/PointController.swift#L313` — held (`[voice note, N s]`)
- `crates/desktop/companion/Sources/Companion/Voice/VoiceRecorder.swift#L8` — held (16 kHz)
- `crates/computer/src/tools.rs#L332` — held (`computer_type`)
- `crates/talk/src/lib.rs#L860-L863` — held (VPIO in talk)
- `crates/desktop/src/point/machine.rs#L6` — held (`HOLD_MS = 200`)
- `crates/desktop/companion/Sources/Companion/AskBar/AskBarPanel.swift#L213` — held (3 s hold)
- `crates/rpc/src/api.rs#L1557` — held (`speak` method)
- `scripts/fetch-models.sh#L76` — held (HF origin)
- `crates/voice/src/lib.rs#L170-L180` — held (voice-notes prompt)
Unverified/soft (flagged inline): sounds, AnswerCard markdown, Windows .cs line numbers, ShortcutRecorder/OverlayWindows
(named, not read). 20/20 re-opened held, 0 corrected.
