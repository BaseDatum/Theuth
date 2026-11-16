# 195 — Phone calls: theuth dials, the talk pipeline carries the conversation, Telnyx the line

Status: design v0 · 2026-11-15 · base HEAD `0423e7d33` (v2.20.2). Extends `76-talk.md` (the
conversation, the policy, the ledger, barge-in), `43-voice.md` (`Sink`, `Speak`, `markdown::spoken`),
`53-meetings.md` §6 (a directory under `data_dir` and its sqlite index), `47-todos.md` (the operation
ledger and the `<job_result>` wake), `37-secrets.md`, `31` (the relay: the precedent for a reachable
listener), `00-architecture.md` §4. macOS and Linux.

Scope. Sky, 2026-11-15 16:22: "the ability to make phone calls"; 16:51: "It'll definitely be telnix and
it should be based on the normal talk feature that we have on the app. But if we can learn something
from dograh and improve it or make it better for phone calls then we should do that." So: theuth dials a
number the user named, the callee answers, and the pipeline that holds a `[spoken]` talk holds the call
— same `Utterer`, same TTS, same policy, same ledger. v1 is outbound and two-way, one call at a time,
Telnyx as carrier. Not a phone system: no inbound numbers, transfers, conferencing or voicemail box of
our own.

## 0. Decisions in one screen

1. **A call is a `talk` whose remote leg is the telephone, and that leg already exists.** `StartTalk {
   sink: Sink::Client, sample_rate }` (`crates/talk/src/lib.rs:209`) takes any rate that is a multiple
   of 50 from 8000 up; `PushAudio` (`:223`) takes mono i16 PCM plus the highest `SpeechAudio.seq` the
   far end finished playing; the reply returns as `VoiceEvent::SpeechAudio` Opus packets on
   `voice.events` (76 §5.3). The far end pushes PCM, we hand it audio, it says what it played — a
   telephone interface before there is a telephone, and v1 uses it unchanged. Rejected: a third
   `Sink::Call` in `voice` — one transcode saved, and a second ledger path, a second `SpeechAudio`
   filter in every frontend and a variant only one caller names in exchange (§3.5 keeps the seam).
   Rejected: `crates/telnyx` driving `voice::Utterer` with its own loop — the second audio stack this
   doc exists to prevent, duplicating the classifier, the ledger, the backchannel list and the policy
   until the copies drift.

2. **One new plugin, crate `telephony`, owns the carrier and the record.** Call Control, the media
   socket, the conversions, the store, five tools, its RPC. Rejected: growing `voice` (43 §0.1: voice is
   "text in, speech out, per message"; a carrier session with a websocket and a billing record is
   another owner). Rejected: growing `talk` (talk owns the conversation policy and must stay free of a
   vendor).

3. **Telnyx; PCMU both ways; `stream_bidirectional_mode: rtp`; `stream_track: inbound_track`.** The leg
   is G.711 8 kHz whatever we choose, so the only question is where the conversion happens; ours is
   measurable and keeps the carrier from transcoding twice. Rejected: `L16` 16 kHz, which Telnyx
   recommends for AI platforms — it removes our 8→16 kHz stage inbound and adds a transcode each way
   inside Telnyx; our stage is 0.3 ms per 20 ms chunk, a carrier transcode is a hop we cannot see.
   Rejected: the mp3 media path — "media payloads can only be submitted once per second".

4. **The plugin owns its listener, and that is a deployment step, not a code step.** Telnyx opens the
   media websocket *to us* and delivers call state as webhooks, so a public HTTPS endpoint is mandatory;
   the engine has none today (`rpc.listen` is "loopback only by design", `crates/rpc/src/lib.rs:44-50`).
   `telephony.listen` binds axum + axum-server TLS inside the plugin — the shape `theuth-relay` already
   uses (`crates/relay/src/main.rs`) — and both routes verify Telnyx's Ed25519 signature against the
   account's published key. Rejected: routes in `theuth-relay`, which is stateless by design ("copies
   opaque encrypted frames", 31 §5) and media is not opaque. Rejected: polling the Call Control API,
   which cannot give the hangup cause at the moment of the report while the socket we hold already sends
   `stop`.

5. **A call is an operation with a receipt and a settlement, not a blocking tool call.** `call_dial`
   admits `Kind::Command("call")` (`crates/tools/src/activity.rs:97`), answers one line — "ringing Alice
   on +15550100" — and the turn ends; the settlement at hangup reaches the model as `<job_result>` on
   the path a job's result takes (47 §4.2). The conversation does not wait on the tool: it arrives as
   `[call …]` inputs while the operation is open, as a meeting's batches do (53 §4.4). Rejected: holding
   the tool open for the length of the call — four minutes of frozen turn and every approval behind it.

6. **The 800 ms is the fill, not the answer.** 76 §8 measures 1.9–3.3 s from speech end to the agent
   audible on a *local* talk, dominated by the provider's time-to-first-sentence; no provider we run
   answers in 800 ms. The call commits to the far end hearing *something* within 800 ms — a filler
   pre-synthesised at dial time — and reports the answer's own first syllable as a measured number, per
   turn, in the record (§7).

7. **The record is a directory and an index, not a row with a blob.** `<data_dir>/telephony/<id>/` holds
   `record.json`, `transcript.md` and the carrier's dual-channel recording; `calls.sqlite` indexes it
   (53 §6's shape). The session keeps the `[call]` turns as ordinary messages. The outcome is one side
   call over the transcript (`CallKind::CallOutcome`, a new variant), written into `record.json` and
   quoted in the settlement.

8. **Barge-in is three acts with one wire message.** `voice::Stop { id: None }` and the queue drop are
   local; the far end's buffer is cleared with `{"event":"clear"}`; the marks Telnyx returns when it
   clears (§2.2) are what close the heard-prefix ledger, so the session keeps exactly what the callee
   heard.

9. **Never dial blind.** The number is an E.164 the user said in this conversation, a name in
   `telephony/contacts.json`, or `telephony.default_number`. Nothing resolves → one line, nothing
   dialled. The receipt always names the number.

10. **Spoken-only is the renderer, not the prompt's good manners.** Synthesis goes through
`markdown::spoken` (`crates/markdown/src/lib.rs:152`), which drops code blocks, reads tables cell by
cell and says "link" for a bare URL. The call's prompt block is style, and style is the only thing left
to get wrong.

## 1. Why

The harness already holds spoken conversations: `talk` owns a live, interruptible one with a barge-in
classifier, a turn policy, a heard-prefix ledger and a prompt that says write for the ear. All of that
is what a phone call needs and none of it is specific to a microphone in the room. What is missing is a
carrier — so the honest answer is not "build a voice agent" but "give `talk` a different pair of ears
and mouth". A call is a conversation with someone who is not the user, over a link that is 8 kHz, lossy,
sometimes busy and always billed, which changes three things and nothing else: the marker on the
session's inputs, the patience of the far end, and what must be written down afterwards.

## 2. Facts relied on (recon 2026-11-15, HEAD `0423e7d33`)

### 2.1 In the tree

- `voice`: `Sink::{Client, Device}` (`voice/src/lib.rs:232`), `Speak` (`:246`), `Stop` (`:264`);
  `Utterer` with `MODEL_RATE = 16_000` and `set_rate` taking 8 000–192 000 in multiples of 50
  (`live.rs:15`, `:85`); TTS at 24 kHz mono, 1 920 samples per 80 ms frame, Opus 32 kbps ≈ 320 B
  (`speak.rs:17`, `:19`); the device path captures at 48 kHz with VoiceProcessingIO AEC (`mic.rs:22`) —
  a call never opens it.
- `talk`: `StartTalk`/`PushAudio` (`lib.rs:209`, `:223`); one talk per daemon (`:867`) and an exclusive
  `Utterer` lease; the barge-in classifier (`policy.rs:748`: noise | backchannel | echo | real) and the
  `[spoken, N s]` marker at `:597`, `:669`; the prompt block is `tools::RegisterPrompt { plugin, text }`
  (`lib.rs:435`) — static text, no per-call slot.
- `tools`: `Kind::Command(&'static str)` is "a harness-managed command with a completion the model is
  owed" (`activity.rs:97`); `JobKind::{Shell, Pty, Promoted}` (`journal.rs:291`) — a call spawns no
  process. `runtime::side_call::CallKind` (`side_call.rs:19`) already owns the word "call" for model
  side calls, so the telephone's id is `telephony::CallId`.
- `secrets`: `Handle` replies with `Arc<Vault>` (`lib.rs:227`); `Vault::get(name, back)` returns
  `Zeroizing<String>` (`vault.rs:588`). Placeholders expand in *tool arguments*, which a plugin's HTTP
  client never passes through — so a plugin reads the vault by name.
- `rpc.listen` defaults to `127.0.0.1:7777`, loopback only by design (`crates/rpc/src/lib.rs:44-50`);
  `crates/relay/src/main.rs` is the in-tree precedent for a TLS listener the internet reaches. The
  workspace already carries `axum 0.8` (ws), `axum-server 0.8`, `tokio-tungstenite 0.30`, `rubato 5`,
  `opus 0.4`, `reqwest 0.13`.

### 2.2 The carrier (Telnyx, read 2026-11-15)

- `POST /v2/calls` dials with `connection_id`, `to`, `from`, `webhook_url`, `timeout_secs`, `record*`,
  `stream_url`, `stream_track`, `stream_bidirectional_mode` (`rtp` | `mp3`),
  `stream_bidirectional_codec`, `answering_machine_detection`; `call.hangup` carries the cause and
  reports `no_answer`/`timeout` for a ring that timed out.
- Telnyx **opens** the media websocket to `stream_url`, signs the upgrade with the account's Ed25519 key
  (the key that signs webhooks; two headers), and sends `connected`, `media` (base64 RTP payload),
  `mark`, `dtmf`, `error`, `stop`. Bidirectional RTP codecs: PCMU 8 kHz (default), PCMA, G722, OPUS,
  AMR-WB, L16 16 kHz; one bidirectional stream and one fork per call; chunks 20 ms to 30 s; "when the
  audio is sent using a different encoding than on the call, it will be transcoded".
- `{"event":"clear"}` — "immediately stop the media playing on the stream and clear the media queue".
  `{"event":"mark","mark":{"name":…}}` returns when the media before it finishes, and "the `clear`
  message will also result in all queued marks being sent back". That pair is the far end's playhead.
- AMD: standard (`call.machine.detection.ended` → human | machine) and premium (`human_residence`,
  `human_business`, `machine`, `silence`, `fax_detected`), plus `call.machine.greeting.ended` with
  `beep_detected`.

### 2.3 What is missing

A public listener (§0.4). A marker for the callee's speech (§3.5). A turn model on disk:
`talk.turn_model: smart-turn` parses (`talk/src/lib.rs:139`) but is refused until
`<data_dir>/models/smart-turn` exists, so the end-of-turn decision is a timer today. And a decision
about who the callee thinks they are talking to (§12).

## 3. Shape

### 3.1 The boundary

| Owner | Owns | Never does |
|---|---|---|
| `telephony` (new) | Telnyx credentials, the listener, the media socket, PCMU/RTP, the pacer, the call actor, `calls.sqlite` and the call directory, five tools, `CallEvent` | synthesise, transcribe, decide a turn, cut a barge-in |
| `talk` (unchanged but §3.5) | the conversation policy: gate, end-of-turn, barge-in classification, the ledger, what is spoken | know a carrier exists |
| `voice` (unchanged) | `Utterer`, `speak`, `Sink`, `SpeechId`, `Stop` | know a telephone exists |
| `agent` (unchanged) | the session, the turn, the tool loop, `SetTalk` | know a call is not a talk |

Audio never leaves the pipeline: inbound G.711 → `telephony::audio` → `PushAudio` → `Utterer` → session;
session → policy → `voice::Speak { sink: Client }` → `SpeechAudio` → `telephony::audio` → G.711 →
socket. The plugin is a client of `talk` in the sense `dictation.ts` is a client of the mic.

### 3.2 Audio path and every conversion

| Step | Where | Rate / format | Added latency |
|---|---|---|---|
| μ-law → i16 | `telephony/src/audio.rs` (256-entry table) | 8 kHz G.711 → i16 | none |
| 8 → 16 kHz | `Utterer::set_rate(8000)` (rubato FFT, exists) | 8 → 16 kHz | none; ~0.3 ms CPU per 20 ms |
| VAD, gate, streaming STT | `Utterer::chunk` (exists) | 16 kHz | ~130 ms to a final (54, 76 §8) |
| TTS | `voice::speak` (exists) | text → 24 kHz f32 | provider-bound |
| Opus encode / decode | `voice::speak` / `telephony::audio` (`opus` 0.4) | 24 kHz, 80 ms, ~320 B | ~0.2 ms CPU per frame |
| 24 → 8 kHz | `telephony::audio::Decimate3`: 48-tap anti-alias FIR, then every third sample | 24 → 8 kHz | none; 1 920 in → 640 out per packet |
| i16 → μ-law | `audio.rs` (same table, inverted) | i16 → G.711 | none |
| RTP framing | `provider/telnyx/rtp.rs`: 12-byte header, sequence, timestamp, PT 0 | 160 B per 20 ms | 20 ms of packing |
| base64 + JSON | `provider/telnyx/media.rs` | — | ~0.05 ms per frame |

Two conversion decisions worth their cost. **The decimator is a FIR, not `pcm[::3]`**: naive decimation
folds 4–12 kHz into 0–4 kHz, and a voice that should sound like a phone sounds like a fax; §10.1 fails a
naive one by construction. **The pacer never stalls**: one 20 ms frame per 20 ms tick out of a 100 ms
pre-buffer, and on starvation the μ-law silence byte, because a stalled RTP stream is indistinguishable
from a dropped call.

### 3.3 The call actor and its events

`Call` is a kameo actor per call, addressed by the plugin: the `call_control_id`, the `TalkId`, the
pacer, the turn count, the deadlines and the marks that came back. `CallEvent` (`#[serde(tag =
"type")]`, `ts` export) joins the bridge list and the TUI's `UiEvent`: `Ringing`, `Answered { amd }`,
`Heard`, `Said`, `Marked { heard }`, `Cleared`, `Ended { outcome, reason, duration_ms, cost }`, `Failed
{ line }`. The call card reads that and the session and does **not** play the call's `SpeechAudio`
locally by default — the phone in the user's hand already is; a "listen in" toggle turns local playback
back on.

### 3.4 Files and modules

```
crates/telephony/src/lib.rs         plugin start: config, listener, tools, prompt block, events
src/config.rs                       the `telephony:` section (§9)
src/call.rs                         the Call actor, pacing, deadlines, the turn cap
src/audio.rs                        μ-law tables, Decimate3, i16 framing, silence
src/provider/mod.rs                 the Provider trait (§3.6)
src/provider/telnyx/{mod,webhooks,media,rtp}.rs   Call Control, signature, socket, framing
src/store.rs                        calls.sqlite, <data_dir>/telephony/<id>/, retention
src/outcome.rs                      the hangup side call
src/tools.rs                        call_dial, call_hangup, call_status, call_list, call_get
src/rpc.rs, src/events.rs           RPC + typed client, CallEvent
```

### 3.5 The changes outside the plugin

Four, each small, named so the worker invents no fifth.

1. `talk/src/lib.rs:209` — `StartTalk` gains `#[serde(default)] mode: TalkMode` (`User` | `Call`).
   `Call` makes the three marker formats (`policy.rs:597`, `:667`, `:669`) read `[call, N s]` and
   registers the call's prompt block instead of the `[spoken]` one. Wire-additive: an old client sends
   no `mode`.
2. `runtime/src/side_call.rs:19` — `CallKind::CallOutcome` (`"call_outcome"`), so the hangup summary is
   charged and visible like any other side call.
3. `crates/rpc/src/bridge.rs` — `CallEvent` in the bridge list; `crates/tui` gets `UiEvent::Call`.
4. Nothing in `tools`: `Kind::Command("call")` is already the right seat and `JobKind` gains no variant.

Rejected: `Sink::Call` (§0.1). If the Opus round-trip ever showed in a profile — it will not at ~2 % of
a core — the seam is `speak.rs`'s device arm: a `CallWriter` registry beside `out::Player`, one arm in
`speak`, `Sink::Call` on the wire.

### 3.6 The provider seam

`Provider` is the trait the call actor holds: `dial`, `hangup`, `record`, `dr` and the socket accept,
with Telnyx the only implementation in v1. It exists because a second carrier is a week's work when the
media and the record are carrier-shaped and a rewrite when they are not. Rejected: an abstraction over
the *media* format too — `audio.rs` already is that.

## 4. The call, step by step

**Dial.** `call_dial { to, purpose, max_turns? }` resolves the number (§8), reads the vault, POSTs the
dial with `stream_url`, `stream_bidirectional_mode: rtp`, `stream_bidirectional_codec: PCMU`,
`stream_track: inbound_track`, `timeout_secs`, `record`, `webhook_url` and
`answering_machine_detection`, admits the operation and answers one line: "ringing Alice on +15550100".
The turn ends.

**Answer.** The media socket opens; `call.answered` and the AMD verdict may arrive in either order. The
plugin starts the talk (`StartTalk { sink: Client, sample_rate: 8000, mode: Call }`) and sends the
purpose as the session's first input: `[call 00:00] You are calling Alice about the invoice. Greet her
and say why you called, in one sentence.` That reply is the opening line, spoken by the policy out of
the socket; if no first sentence exists 600 ms after answer, the pre-rendered "hello" filler plays
instead, because a phone that answers with silence sounds broken.

**Turns.** The callee's speech is a `[call, N s]` input, answered as any talk turn. Turn count and
duration are checked before each new turn; at the cap the last input carries "last turn: wrap up" and
the reply ends the call.

**Hangup.** Any of: `call_hangup`, the user pressing hang up on the card or saying it, the cap, the
callee hanging up, a carrier failure, or `give_up_ms` with no audio either way. After the last sentence
the plugin waits 400 ms and POSTs `…/actions/hangup` (a closing line first when the hangup is ours).

**What each ending reports.** One line, in the settlement and the card, never a stack trace:

| Ending | Line |
|---|---|
| answered, ended | `call to +15550100 ended: 3 turns, 1:42, answered` |
| no answer | `call to +15550100 ended: no answer after 30 s (0:31)` |
| busy | `call to +15550100 ended: busy (0:04)` |
| rejected | `call to +15550100 ended: rejected by the callee (0:06)` |
| voicemail | `call to +15550100 ended: voicemail, message left (0:38)` |
| machine, no beep | `call to +15550100 ended: answering machine, message left at the tone (0:41)` |
| fax | `call to +15550100 ended: fax tone, nothing said (0:12)` |
| our failure | `call to +15550100 failed: <one clause> (no audio)` |

Voicemail is the carrier's verdict, not a guess: on `call.machine.detection.ended: machine` (or premium
`machine`) the agent switches to the voicemail script — one sentence, no questions — and hangs up after
`call.machine.greeting.ended` or 8 s. Rejected: detecting a machine from the transcript, which works
until someone answers "Alice speaking" with a cough while the carrier has already told us.

## 5. The record

`<data_dir>/telephony/<id>/` holds `record.json` (the row, the numbers, the outcome), `transcript.md`
(the session's `[call]` turns rendered, so the record outlives the session) and `recording.wav`
(Telnyx's dual-channel recording, fetched at `call.recording.saved`, written 0600). `calls.sqlite`
indexes `id, session, to_e164, name, purpose, dialled_ms, answered_ms, ended_ms, duration_ms, turns,
outcome, hangup_cause, amd, cost, currency, recording, bytes`.

Retention: `telephony.retention_days` (default 30) deletes the directory and the row at start and at
04:00 local; the session's copy follows the session's own retention (51). Deleting a call deletes its
recording — no orphan files, the meeting store's rule too (53 §6).

The user reads it three ways: `call_get <id>` (record, outcome, latency numbers), `call_list` (the last
20, one line each), and the call card left in the transcript where the call happened. Cost is Telnyx's
Detail Record, fetched at hangup and again at the sweep; when the DR has not landed the row says `cost:
unknown until <time>`, never a zero.

The outcome side call (`CallKind::CallOutcome`) runs once at hangup on the session's model unless
`telephony.outcome_model` names another: transcript in, `{outcome, summary, commitments[],
follow_ups[]}` out, one JSON object, no tools. Its line is quoted in the settlement and filed in
`record.json`. Rejected: extracting per turn — a commitment is only one once the call has ended.

## 6. Barge-in over the wire

Local barge-in is 76 §4.5 and does not change: `Stop { id: None }` clears the queue, the turn is
cancelled with what was heard kept (`Cancel { keep }` / `TrimAssistant`, 76 §7), and `[call, N s,
interrupted]` tells the model it was cut.

What is new is the far end, and the order matters:

1. The classifier says `Real` — not noise, not a backchannel, not echo (`policy.rs:748`; echo matters
   more here than on the device, because VoiceProcessingIO is not in the path and the callee's
   speakerphone is).
2. `{"event":"clear"}` goes out first, before anything else on that socket: Telnyx stops the media,
   clears the queue and returns every queued mark.
3. The marks that come back name the `SpeechId:seq` pairs that never played; the plugin subtracts them
   from what it sent and pushes the remainder as `PushAudio { played_seq }`. That is the ledger's
   position for a call, and it is the carrier's answer, not our estimate.
4. `voice::Stop { id: None }` and the `Cancel`/`TrimAssistant` follow, as on the device.

Rejected: a mark per frame — returns are one per mark, and one per utterance's last frame is enough to
know where a cut landed. Rejected: trusting the socket's write order — the socket says what we sent,
only the marks say what was played.

## 7. Latency budget

Two paths, because they are two promises. The fill path is the one we commit to; the answer path is the
one we measure and report.

| Hop | Fill path | Answer path | Where it is measured |
|---|---|---|---|
| Callee stops → VAD close | 300 ms (76 §8) | 300 ms | `talk` state log |
| Close → turn decision | 200 ms (smart-turn; 400 ms timer fallback) | same | `talk` timers |
| Final text of the last utterance | not waited for | ~130 ms after the 560 ms window pad (54, 76 §8) | `Utterer` |
| Filler select → first frame | 5 ms (pre-rendered, in memory) | — | plugin |
| Model time-to-first-sentence | — | 250–1 500 ms, provider-bound | `talk` "first audio after N ms" |
| First sentence → first TTS frame | — | ~200 ms + 80 ms frame (43 §0.5) | `voice` |
| 24→8 kHz, μ-law, 20 ms packing | 20 ms | 20 ms | plugin |
| Pacer, socket, Telnyx edge → PSTN | 80 ms (estimate) | 80 ms | mark round-trip |
| **Total from the callee stopping** | **≈ 605 ms** (195 ms headroom) | **≈ 1.4–2.9 s** | per turn, into the record |

The fill path is met by asking the model nothing: fillers are synthesised at dial time — three clips and
an opening "hello", ~1 s of TTS while the phone rings — and held as 24 kHz PCM in the call actor. Rules:
one filler per turn, one per 8 s, never two in a row, never while the policy believes the callee is
still talking. A mid-call tool round gets the same filler at `tool_hold_ms` (6 s) and a second class
("still working on it") at `give_up_ms`/2, after which the agent speaks or the call closes.

Where it is measured: the plugin timestamps the last inbound frame, the gate close, the first outbound
frame and the mark return, and writes per-turn `first_audio_ms` into `record.json`; `t call get <id>`
prints p50, p95 and the dominant hop. A p95 over 1 200 ms is a defect, not variance.

## 8. The constraints, each answered

**A ringing phone must not block a turn.** The dial is an admitted `Kind::Command("call")` operation
with a one-line receipt; the turn ends after the receipt; the result is a settlement on the
`<job_result>` path (47 §4.2); the conversation arrives as `[call]` inputs while the operation is open.
Nothing waits: not the turn, not an approval, not the next tool call.

**Never dial blind.** Resolution order: an E.164 in the tool's `to`; a name in `telephony/contacts.json`
(name → E.164, plain JSON the user edits or the agent writes when the user says "call Alice on +1…"); a
number the user typed in this turn; `telephony.default_number`. Nothing resolves → "no number: say who,
or set telephony.default_number", and nothing is dialled. The receipt names the number and the name, and
the number is the first line of the record. Rejected for v1: the macOS address book — a permission
prompt and a second identity model for the three people this is for.

**Spoken-only replies.** The renderer is the enforcement (§0.10). The call's prompt block adds style
only: two sentences a turn, one question at a time, no lists, numbers as words when a word will do.

**A mid-call tool call gets a filler and a cap.** `tool_hold_ms` (6 s) → the "one moment" filler;
`give_up_ms` (30 s) → the agent speaks or the call closes. The policy already speaks prose before a tool
batch (76 §4.8); the filler is for the batch that arrives without prose.

**A failed, busy or unanswered call reports one line.** §4's table; the reason is the carrier's
`hangup_cause` and the AMD verdict, and the line never carries a JSON body.

**Secrets.** The Telnyx key lives in the vault under `telephony.api_key_secret` (default
`telnyx.api_key`). The plugin reads it with `secrets::Handle` → `Vault::get` — a plugin is not a tool
call, so nothing expands a placeholder for it — and holds it as `Zeroizing<String>`. Every string the
plugin publishes or stores passes the vault's redactor first (the same `Secrets::redact` tool output
goes through), so a carrier error that echoes the key comes back as the placeholder text instead of the
value. The key never enters `record.json`, a log line, a `CallEvent`, or the receipt.

## 9. Config

```yaml
telephony:
  enabled: false
  listen: "0.0.0.0:8443"          # behind your ingress; Telnyx requires TLS
  tls: { cert: "", key: "" }      # or terminate TLS in the ingress
  api_key_secret: telnyx.api_key  # a vault name, never a value
  connection_id: ""               # the Call Control application
  from_number: ""                 # a number on that connection
  default_number: ""              # the fallback of §8
  ring_timeout_secs: 30
  amd: standard                   # standard | premium | off
  record: dual                    # dual | single | off
  max_turns: 24
  max_cents: 50                   # against cents_per_minute × elapsed
  retention_days: 30
  tool_hold_ms: 6000              # filler deadline 600 ms, gap 8 s (§7)
  give_up_ms: 30000
  outcome_model: ""               # empty = the session's model
```

`enabled: false` by default: a listener that is not configured should not be open, and `call_dial` says
so in one line ("telephony is not listening: set telephony.listen").

## 10. Checks

1. **Codec.** 440 Hz at 24 kHz → `Decimate3` → μ-law → decode → 440 Hz within 1 Hz, no image above −40
   dBFS at 3× and 5×; a naive `[::3]` fails it. `cargo test -p telephony audio`.
2. **Signature.** A captured webhook and media upgrade replay against the verifier: good passes, one
   flipped byte fails, a stale timestamp fails.
3. **A real call**, 60 s, to the user's own mobile: transcript in the session, record row complete,
   recording fetched and playable, cost from the DR, `call_get` printing the outcome. v1's gate is this
   call plus the next four.
4. **Barge-in.** A 20 s reply interrupted at 4 s: `clear` on the wire, the returned marks naming the
   unplayed tail, `played_seq` matching, the assistant message trimmed to what was heard, the next input
   carrying `[call, N s, interrupted]`.
5. **Latency and failure lines.** p50/p95 of `first_audio_ms` over 20 turns in three calls; and six
   one-line answers (listener off, no number, bad key, busy, no answer, a carrier 4xx) with no stack
   trace and no JSON.

## 11. Rejected (beyond §0)

A `Sink::Call` (§0.1, §3.5). A second conversation loop in the carrier crate. The mp3 media path. Our
own recording muxer. An inbound number, a voicemail box, transfers, conferencing — each a product, not a
feature; the seams are the `Provider` trait and the record. AMD off. Polling instead of webhooks.

## 12. Open questions

1. **Who does the callee think they are talking to?** The agent speaks in the user's name but is not the
   user: the opening line can say "this is Sky's assistant" (honest, slower, some people hang up) or say
   nothing (faster, occasionally awkward). Lean: name it unless `telephony.introduce_as` is set —
   config, not a doc's opinion.
2. **Recording consent.** California and others need notice: one clause in the opening line ("I'll
   record this, alright?") by default, or off and the user is responsible. Lean: on by default, off for
   numbers in `telephony.no_record` — a recording you cannot lawfully keep is worse than a call you
   cannot replay.
3. **The turn model.** `smart-turn` parses but is refused until `<data_dir>/models/smart-turn` exists;
   without it the fallback timer is 400 ms, which puts the fill path at ~780 ms — inside 800 with no
   headroom for a bad line. Lean: install it and re-measure; if it stays uninstalled, raise the
   commitment to 900 ms in §7 rather than miss it quietly.
4. **The 12-byte RTP header.** Telnyx's page says the payload is "a base64-encoded RTP payload (no
   headers)"; its own demos and Pipecat's serializer write the 12-byte header. v1 writes the header
   because two working implementations do; being wrong costs one function in `rtp.rs`.
5. **Two calls at once.** `talk` allows one ("already talking"). Lean: refuse a second dial with
   "already on a call with Alice" — a second conversation on one `Utterer` is a second engine.

## 13. Status

Design v0, 2026-11-15. Nothing built. The order is §10's list: the codec and the signature verifier need
no carrier account; the first real call gates everything after it.
