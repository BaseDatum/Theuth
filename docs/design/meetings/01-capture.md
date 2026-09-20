# Meetings phase 1 — capture, encode, persist

Status: phase design v0 · 2026-09-19 · implements 53 §0 decisions 1, 4, 12,
13, 14 and §3.1/§4.1/§4.2; fills 53's missing §5 (storage). Contract types:
`README.md` §3. macOS 15 only; the crate compiles on other targets with
`start` returning `Ok(())` after a log line.

Goal of the phase: press start, talk, stop, and have three valid Opus files,
a manifest, an index row and an (empty) agent session — with no STT, no
model, no island. Recording outranks intelligence (decision 12); this phase
is the part that must never lose audio.

## 0. Decisions local to this phase

1. **Session is created in this phase**, empty. `MeetingEvent::Started.session`
   is non-optional from day one so phases 2–4 never migrate an `Option`. The
   session gets `project: Some("meetings")` (`SetSessionProject`, exists) and
   title `Meeting <yyyy-mm-dd hh:mm>` (`RenameSession`, exists). 53 §4.1's
   "`kind` metadata from 51" does not exist in `SessionSummary`
   (`crates/agent/src/lib.rs:907-940`); the sidebar filters on `project`.
   Rejected: a `kind` column (a migration for an icon).
2. **Mic AEC via `coreaudio-rs` VoiceProcessingIO; fallback to `flexaudio`
   `Mic`.** Per decision 4. The fallback flips `Degradation::MicNoAec` and
   nothing else changes downstream — both paths fill the same rings.
3. **Encoder thread is the only writer of the three files.** Mic and system
   threads push into `rtrb` rings; the encoder drains at 20 ms cadence. One
   writer → no file locking, page boundaries are ours, and a crash in STT
   (phase 2) cannot touch a file handle.
4. **`meetings.sqlite` is derived, rebuildable by scanning `<data_dir>/meetings/*/manifest.json`.**
   `Store::open` runs the rebuild when the db is missing. Truth is the
   directory. Rejected: manifest data only in sqlite (decision 13 forbids
   blobs; a corrupt db must not lose the list).
5. **Manifest rewritten atomically (`.tmp` + rename) on every state change.**
   It is small; a partial write on crash must not exist.
6. **Permissions asked on first start, reported by `meeting_permissions`.**
   Copies `crates/computer/src/lib.rs`'s `computer_permissions` pattern
   (report status, never pre-prompt). Mic: `AVCaptureDevice`-free — opening
   the VPIO unit triggers the TCC prompt; audio capture: the first
   `flexaudio::open(Process)` triggers it. A denial is `Starting → Failed`
   with `PermissionDenied(_)` in `State.degraded`.

## 1. Crate

```
crates/meeting/
  Cargo.toml
  src/lib.rs          consts, MeetingConfig, contract types (README §3), Meeting actor,
                      messages, MeetingPlugin
  src/state.rs        state machine + Recording (the live meeting's handles)
  src/store.rs        directory, Manifest, meetings.sqlite index
  src/capture/mod.rs  Capture::start(process, cfg) -> Capture { rings, threads, stop }
  src/capture/mic.rs  VPIO unit (coreaudio-rs); fallback flexaudio Mic
  src/capture/system.rs  flexaudio Process tap thread; processes()
  src/capture/encode.rs  encoder thread: resample, mix, opus, ogg
  src/ogg.rs          OpusHead/OpusTags + PacketWriter helpers (RFC 7845)
```

`Cargo.toml` per `docs/plugins.md` §3.1: `runtime`, `tools` (for `Risk`
later; not needed in this phase — omit until phase 2), `agent` (session
messages), `voice` (`voice::mic::Capture`, the AEC-less fallback), `kameo`,
`tokio`, `async-trait`, `anyhow`, `serde`, `serde_json`, `schemars`,
`tracing`, `uuid`, `chrono`, `sqlx` (sqlite, as `artifact`), `rtrb = "0.3"`,
`opus` (workspace), `rubato` (workspace), `ogg = "0.9"`, `flexaudio`
(workspace entry exists since 54; 0.2.0 is the only release), and under
`[target.'cfg(target_os = "macos")'.dependencies]`: `coreaudio-rs = "0.13"`
(already in `Cargo.lock` via `cpal`; not 0.12 — two versions would duplicate
`objc2-audio-toolbox`) and `objc2-core-audio = "0.3"` (already in the lock
at 0.3.2 through `flexaudio-os-macos`; for the process list, §4.1). `ts-rs`
optional. Feature `ts = ["dep:ts-rs", "runtime/ts", "agent/ts"]`. Root
`[workspace.dependencies]`: `meeting = { path = "crates/meeting" }`, plus
`rtrb`, `ogg`, `coreaudio-rs`, `objc2-core-audio` with a `# 53` comment.
`crates/wth/Cargo.toml`
gains `meeting`; `crates/wth/src/main.rs:469-495` default plugin vector gains
`Box::new(meeting::MeetingPlugin)` after `voice::VoicePlugin` (L482); the
`--acp` branch omits it like `voice`. `DEFAULT_CONFIG` (main.rs:126-262) gains
the commented `meeting:` section from README §3.

## 2. Config

`MeetingConfig` (`#[serde(default)]`): `enabled: bool = true`,
`max_minutes: u32 = 180`, plus the phase-2/4 keys declared with their
defaults now (README §3) so the struct is written once. `rt.section("meeting")`
with the `unwrap_or_else(Default)` fallback (`crates/voice/src/lib.rs:563-566`
pattern). `enabled: false` → plugin starts nothing; RPC methods answer
`NOT_RUNNING` remapped to "meetings are disabled" (as `voice()` does at
`crates/rpc/src/api.rs:996-1001`).

## 3. Actor and state machine

`Meeting` (`#[derive(kameo::Actor)]`, registered `MEETING_ACTOR`), fields:
`rt: Runtime`, `cfg: MeetingConfig`, `store: Arc<Store>`, `current:
Option<Recording>`. Spawned with `runtime::subscriber_mailbox()` (phase 2
subscribes it to `AgentEvent`).

```
Idle ──StartMeeting──▶ Starting ──ok──▶ Recording ◀──Pause/Resume──▶ Paused
                          │ fail                  │ StopMeeting / max_minutes
                          ▼                       ▼
                        Failed                Stopping ──▶ Post(_) ──▶ Review ──▶ Summarizing ──▶ Done
                                               (phase 1 goes Stopping → Done directly)
```

Exactly one `Recording` per process (one mic). `StartMeeting` while
`current.is_some()` → `Err("a meeting is already recording")`. Every
transition publishes `MeetingEvent::State` and rewrites the manifest.

`Recording { id, session, started: Instant, clock: RecordingClock, capture:
Capture, level_task: JoinHandle, degraded: Vec<Degradation>, gaps: Vec<Gap> }`.
`RecordingClock` = wall `Instant` minus accumulated pause duration; `now_ms()`
is the timeline every later phase stamps with. `Pause` stops the encoder's
drain loop and both capture threads' pushes (rings are drained and discarded),
keeps devices open, records `Gap { at_ms, started: Instant }`; `Resume`
closes the gap (`dur_ms`) and restarts pushes. Max length: a tokio timer
publishes `NearMaxLength` at `max_minutes - 10` and sends `StopMeeting` at
`max_minutes`.

Messages (`ask` unless noted; all `Result<_, String>`):

| message | reply | notes |
|---|---|---|
| `ListProcesses` | `Vec<ProcessInfo>` | `capture::system::processes()` (§4.1), active first, then by name |
| `StartMeeting { process: Option<u32>, title: Option<String>, context: Option<String>, consent: bool, session_model: Option<String> }` | `Started { meeting, session }` | `consent == false` → `Err`; `process: None` → `Err` in v1 (a meeting is a meeting app) |
| `PauseMeeting` / `ResumeMeeting` / `StopMeeting` | `()` | wrong state → `Err(state name)` |
| `MeetingStatus` | `Status { state, meeting: Option<MeetingId>, session: Option<SessionId>, elapsed_ms, degraded }` | for the UI on reconnect |
| `ListMeetings { limit: Option<u32> }` | `Vec<MeetingSummary>` | from the index, newest first |
| `GetMeeting { meeting }` | `Manifest` | |
| `DeleteMeeting { meeting }` | `()` | `Err` if it is the current recording; else directory, index row, `agent::DeleteSession` — in that order, so a half-failure leaves a session pointing at nothing rather than files without a session |
| `Permissions` | `Permissions { microphone: PermissionState, audio_capture: PermissionState }` | `Granted | Denied | NotDetermined` (as `computer`) |

`StartMeeting` handler (leaves the mailbox — `ctx.reply_sender()` +
`tokio::spawn`, `docs/plugins.md` §3.2): 1 `store.create(id, manifest)`
(directory + first manifest); 2 `agent::CreateSession` (`ask` on
`AGENT_ACTOR`, `CreateSessionOptions { retention: durable, project:
Some("meetings"), ..Default }`, peer `None`) → `SessionOpen.session`; then
`RenameSession`, and `SetSessionProject` if `CreateSession` did not persist
it; `session_model` is forwarded to phase 2 (stored in the manifest; unused
here — see `02-live.md` §0.3 for why not `SetModel`); 3
`Capture::start(pid, &cfg, dir)` (blocking — `spawn_blocking`); 4 `State
{Recording}` + `Started`; 5 `Level` task. Any failure → `Failed`, manifest
rewritten with the reason, directory kept (the user can delete it).

`StopMeeting`: `Stopping` → `capture.stop()` joins the three threads (the
encoder flushes and writes the final Ogg pages with the EOS flag) → manifest
`ended_at`, `duration_ms` → index `upsert` → `Done`. Since phase 4 the last
arrow is `Post(Transcribing)`: `Stopping` is saved with `ended_at` and the
post pipeline task takes the manifest from there (`04-post.md` §2, §10
deviation 7) — `Done` is reached only through `Review` → `Summarizing`.

## 4. Capture

### 4.1 System thread (`capture/system.rs`)

Facts verified against `flexaudio` 0.2.0 / `flexaudio-core` 0.2.0 sources
(2026-09-20; 53 §2.2's `Process(pid)` / `processes()` / `host_time` were
guesses): `flexaudio::open(StreamConfig { kind: SourceKind::ProcessLoopback,
target_pid: Some(pid), output: OutputFormat { sample_rate: 48_000, channels: 1 },
chunk_ms: 20, ..Default::default() })` → `Stream::start()` →
`poll_chunk() -> Option<AudioChunk { data: Vec<f32>, frames, pts_ns: i64
/* normalized monotonic ns of the first sample */, seq, flags, dropped_before,
peak, rms }>`; the crate has no process enumeration (`pub fn`s are `devices`,
`watch_devices`, `open`). Run it on a dedicated `std::thread` (the tap's ObjC
objects are `!Send`). Loop: `poll_chunk()` (sleep 5 ms when `None`, as
`voice::mic` does) → push `(pts_ns, data)` into `ring_enc` and (phase 2)
`ring_stt`. `ring_stt` exists
in this phase (constructed, never drained by anything; phase 2 attaches a
consumer) — the producer side `try_push`es and drops on full so an absent
consumer costs nothing.

`processes()`: `AudioObjectGetPropertyData(kAudioObjectSystemObject,
kAudioHardwarePropertyProcessObjectList)` → per object
`kAudioProcessPropertyPID`, `kAudioProcessPropertyBundleID` (CFString, may
be absent), `kAudioProcessPropertyIsRunningOutput` (→ `active`); constants
from `objc2_core_audio`. `name`: `libc::proc_name(pid)` (falls back to the
bundle id's last component). Skip our own pid. Sort `active` first, then
`name`. If the process exits mid-meeting, `poll_chunk` errors →
publish `Degradation`-free `State` unchanged, keep the encoder inserting
silence (the meeting app may relaunch; the user stops when done).

### 4.2 Mic thread (`capture/mic.rs`)

`coreaudio::audio_unit::AudioUnit::new(IOType::VoiceProcessingIO)`
(`coreaudio-rs` 0.13: `IOType::VoiceProcessingIO`, `set_input_callback`,
`set_property`); input scope enabled on bus 1, output scope disabled on bus
0; stream format `f32`, 48 kHz, mono; `set_property(
kAUVoiceIOProperty_BypassVoiceProcessing /* objc2_audio_toolbox, re-exported
by coreaudio-rs's sys */, Global, 0, &0u32)` (AEC + AGC + NS on). Input
callback pushes into `ring_enc`/`ring_stt`. VPIO open failure
(aggregate/odd device) → `voice::mic::Capture::open()` (54 §4.5; exists:
`crates/voice/src/mic.rs`, `open() -> (Capture, Receiver<Result<Vec<f32>,
String>>)`, 960-sample 48 kHz mono chunks, dropping `Capture` closes the
device) pumped by the mic thread into the same rings, plus
`Degradation::MicNoAec`. The VPIO unit's echo reference is whatever the
default output device is; no code chooses it (decision 4).

**One clock for alignment**: every ring entry is `(Instant, Vec<f32>)`,
stamped by the producing thread on receipt. The three sources expose three
different timestamps (VPIO `mHostTime` in mach ticks, the tap's normalized
`pts_ns`, the fallback mic none), and arrival is within one 20 ms chunk of
capture for all of them, which is well inside the 60 ms stall threshold
below. Rejected: converting mach ticks and `pts_ns` to a common base — two
conversions for sub-chunk precision nobody consumes.

### 4.3 Encoder thread (`capture/encode.rs`)

Every 20 ms: drain both rings into per-track 48 kHz buffers; clock-align by
the receipt `Instant` — a source whose newest chunk is older than 60 ms
behind the other gets zeros for the missing span (53 §3.1, "insert silence
when a source stalls"); resample each track to 24 kHz once (`rubato::FftFixedIn`, as
`crates/voice/src/stt.rs` `resample`); mix = `mic + system` then a soft
limiter (`x / (1 + |x|)` is enough: same size as clamp, no clicks); three
`opus::Encoder::new(24_000, Mono, Application::Voip)` at 32 kbps, 480-sample
frames; each packet → `ogg::PacketWriter::write_packet(.., granule)` with
granule = 48 kHz-based sample count per RFC 7845 (pre-skip from
`encoder.lookahead()`), page flushed every 1 s (`PacketWriteEndInfo::EndPage`)
so a dead daemon leaves playable files. `OpusHead` (version 1, channels 1,
pre-skip, input rate 48000) and `OpusTags` (vendor `wth`, `TITLE`, `TRACK`)
written at open. RMS per track computed here and sent to the actor's
`Level` task over a `watch` channel at 10 Hz.

`Capture::stop()` sets an `AtomicBool`, joins the three threads; the encoder
writes the last page with EOS. `Capture::pause()/resume()` flip a second
flag read by the encoder (stop draining; discard ring contents on resume so
the gap is clean).

### 4.4 Disk

Before start and every 60 s: `statvfs` on the data dir; < 500 MB →
`Degradation::DiskLow` (no automatic stop — decision 12 says recording
outranks; the user decides).

## 5. Data on disk (fills 53 §5)

```
<data_dir>/meetings/
  meetings.sqlite              derived index (rebuildable)
  <meeting-id>/
    manifest.json              truth
    mic.opus  system.opus  mix.opus   truth; Ogg Opus 24 kHz mono 32 kbps
    live.jsonl                 phase 2 (truth for the live text)
    briefs.jsonl               phase 2
    transcript.json            phase 4 (derived until reviewed_at is set, then truth)
    summary.md                 phase 4
  speakers/<uuid>.json         phase 4 (truth)
```

`Manifest` (serde, `#[serde(default)]` on every later-phase field):

```rust
pub struct Manifest {
  pub id: MeetingId, pub session: SessionId,
  pub title: Option<String>, pub context: Option<String>,
  pub started_at: String, pub ended_at: Option<String>,          // RFC 3339
  pub duration_ms: Option<u64>,                                  // recording clock
  pub process: ProcessInfo,
  pub consent_at: String,                                        // decision 14
  pub gaps: Vec<Gap>,                                            // { at_ms, dur_ms }
  pub degraded: Vec<Degradation>,                                // union over the meeting
  pub state: MeetingState,
  pub failure: Option<String>,
  pub session_model: Option<String>,                             // for phase 2
  pub tracks: Vec<TrackFile>,                                    // { track: Track|Mix, file, sample_rate: 24000 }
  // phase 4 appends: reviewed_at, summarized_at
}
```

`meetings.sqlite` (sqlx, `sqlx::migrate!("./migrations")` as `artifact`):

```sql
CREATE TABLE meetings (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, ended_at TEXT,
  title TEXT, session TEXT NOT NULL, process_name TEXT NOT NULL, duration_ms INTEGER,
  state TEXT NOT NULL);
-- phase 4 adds: CREATE VIRTUAL TABLE segments USING fts5(meeting UNINDEXED, t0 UNINDEXED,
--   t1 UNINDEXED, speaker, text);
```

`MeetingSummary { id, started_at, ended_at, title, session, process_name,
duration_ms, state }` is the row.

## 6. RPC (`crates/rpc/src/api.rs`)

Pattern: `#[method(name = "…")]` in `trait Api` (L122+), impl via
`ask::<meeting::Meeting, _>(meeting::MEETING_ACTOR, msg)` through a
`fn meeting() -> RpcResult<ActorRef<meeting::Meeting>>` beside `voice()`
(L996) that remaps `NOT_RUNNING` to "meetings are disabled".

| method | params | returns |
|---|---|---|
| `meeting_processes` | — | `Vec<ProcessInfo>` |
| `meeting_start` | `StartMeeting` | `Started` |
| `meeting_pause` / `meeting_resume` / `meeting_stop` | — | `()` |
| `meeting_status` | — | `Status` |
| `meeting_list` | `limit: Option<u32>` | `Vec<MeetingSummary>` |
| `meeting_get` | `MeetingId` | `Manifest` |
| `meeting_delete` | `MeetingId` | `()` |
| `meeting_permissions` | — | `Permissions` |

Events: `MeetingEvent` into `crates/rpc/src/bridge.rs` `forward!` list
(L76-105) and `rt.subscribe::<Bridge, MeetingEvent>` in `start` (L130-154).
`crates/rpc/Cargo.toml` `features.ts` gains `"meeting/ts"`; `export.rs`
gains `MeetingEvent, MeetingId, MeetingState, Degradation, ProcessInfo,
StartMeeting, Started, Status, MeetingSummary, Manifest, Permissions, Topic,
Brief, BriefSource, TopicId, BriefId, Track` (declare the phase-2 types now
so bindings are stable). `just ts` → `bindings/`.

`rpc` depends on `meeting` (it names its types); `meeting` depends on
`agent`, `voice` (the fallback mic) and `runtime` in this phase. The chain
in 00 §4 gains `meeting` after `voice`.

## 7. Not in this phase

STT, VAD, batcher, tools, prompt (02). Island, start form, tray entry,
control center (03) — this phase is checked headless over RPC. Post pipeline
(04). TUI: none (53 is desktop-only; a `/meeting` slash command is not
designed).

## 8. Check

Headless on a scratch data dir (`docs/plugins.md` §5), a real Mac with a
notch or not, QuickTime Player or Music playing a known clip as the
"meeting app", the built-in mic:

1. `cargo check/clippy -D warnings/test -p meeting`, `cargo check -p rpc
   --features ts`; `just ts` emits `MeetingEvent.ts`.
2. `meeting_processes` lists Music with `active: true` first.
3. `meeting_start { process, consent: true }` → `Started` with a `session`
   that `list_sessions` shows under project `meetings`; `State{Recording}`;
   `Level` events at ~10 Hz with `system_rms` following the clip and
   `mic_rms` following speech; files `mic.opus`/`system.opus`/`mix.opus`
   exist and grow within 2 s.
4. Speak over the clip with speakers on (no headphones) for 20 s; pause
   (`meeting_pause`) 5 s; resume; 10 s more; `meeting_stop`.
5. `ffprobe` on each file: Ogg, Opus, 24 kHz, mono, durations within 1 s of
   each other and of `duration_ms`; `mix.opus` plays both voices; the
   far-end clip is inaudible (or clearly attenuated) in `mic.opus`. Manifest
   has one gap ≈ 5 000 ms and `state: Done`.
6. Kill the daemon (`kill -9`) mid-recording; the three files play up to
   the last second; restart; `meeting_list` shows the row (index rebuilt
   from the manifest; `state` is what the manifest last said).
7. `consent: false` → error; `process: None` → error; second `meeting_start`
   while recording → error.
8. Force the fallback (set an aggregate output device in Audio MIDI Setup)
   → `State.degraded` contains `MicNoAec`, recording proceeds.
9. `meeting_delete` → directory gone, `meeting_list` empty, session gone.
10. `meeting.enabled: false`, restart → `meeting_start` answers "meetings
    are disabled".
11. Deny the audio-capture TCC prompt on a fresh TCC state (`tccutil reset
    All wth.rwx.dev` is not enough for audio capture on 15; use a fresh
    user or accept "not exercised") → `Failed` with `PermissionDenied`.

## 9. Status

Implemented 2026-09-20 (`crates/meeting`, ~2 700 lines incl. tests; rpc/wth
wiring +184 lines). Verified headless over RPC on `/tmp/wth-meet`, macOS 15,
QuickTime Player playing a 100 s `say`-generated speech clip, an external mic
on a dock with the laptop lid closed, built-in speakers.

### Exercised

1. `cargo check --workspace --all-targets` clean; `cargo test -p meeting` 6/6
   (OpusHead/OpusTags bytes, granule/page/EOS layout read back with
   `ogg::PacketReader`, clock gaps, encoder stall/backlog, manifest round trip
   + index rebuild + delete); `cargo check -p rpc --features ts`; `just ts`
   emits `MeetingEvent.ts` (+ 23 more) with `final: boolean` from `r#final`.
   `cargo clippy -p meeting -p rpc -p wth --all-targets --no-deps -D warnings`
   clean. **The workspace-wide clippy run fails in `crates/tools`** (three
   pre-existing lints — `obfuscated_if_else` in `pty.rs:621`, `type_complexity`
   in `registry.rs:91`, `too_many_arguments` in `registry.rs:1251` — raised by
   the installed 1.96 toolchain on code this phase does not touch); not fixed
   here.
2. `meeting_processes`: QuickTime Player `active: true` first, then inactive
   ones by name; pids `proc_name` refuses show as their number.
3. `meeting_start` → `Started` in ~950 ms; `list_sessions` shows the session
   titled `Meeting 2026-09-20 02:07`, `project: "meetings"`;
   `State{Starting}`, `State{Recording}`; `Level` at 10.1 Hz, `system_rms`
   following the clip, `mic_rms` following speech (max 0.055 while talking,
   0.005 when not); the three files existed at start and grew ~1.7–3.3 KB/s.
4. 20 s of speech over the clip (speakers), `meeting_pause`, ~10 s (the 5 s
   plus round trips), `meeting_resume`, 10 s, `meeting_stop` (249 ms).
5. `ffprobe`: `ogg`/`opus`/mono on all three, durations identical (51.06 s),
   `duration_ms` 49 893; manifest `state: Done`, one gap `{at_ms: 32531,
   dur_ms: 10227}` = the pause actually taken. Listening (user): `mix.opus`
   carries both voices; **the far-end clip is clearly audible in
   `mic.opus`**. Measured separately with nobody talking (plain
   `voice::mic` vs the VPIO unit in one process, clip at 55 %): RMS
   3.2e-2 → 3.9e-3, i.e. the canceller removes ~18 dB but not the far end.
   VPIO is configured (§4.2 below), AEC is active; its residual on this setup
   (external dock mic, built-in speakers, external-clock aggregate) is what
   decision 4 accepted, not a bug this phase can fix. Note `ffprobe` reports
   `sample_rate=48000` for every Opus stream by definition; the tracks are
   encoded at 24 kHz (`OpusHead` input rate 24000, `TrackFile.sample_rate`).
   After the paused-start fix (deviation 5) a re-run gave `duration_ms` 8 238
   vs files 7.96 s.
6. `kill -9` at ~22 s of recording: all three files play to 22.00 s (1 s
   pages), manifest says `Recording`; `meetings.sqlite*` deleted; restart →
   `meeting_list` shows both rows, the crashed one as `Recording` with
   `duration_ms: null` (rebuilt from the manifests).
7. `consent: false` → "consent is required"; `process: None` → "a meeting
   needs a process to record"; second start → "a meeting is already
   recording"; `meeting_stop` when idle → `Idle`.
9. `meeting_delete` of the crashed meeting → directory gone, row gone,
   session gone from `list_sessions`, `Deleted` event; refused for the
   current recording.
10. `WTH_MEETING__ENABLED=false` → log "meeting disabled by config";
    `meeting_start`/`meeting_list` answer `-32001 "meetings are disabled"`.

### Not exercised

- Check 8 as written: with an aggregate device set as **output**, the VPIO
  unit still opens (no `MicNoAec`; the mic then carried the clip at full
  level — the aggregate has no echo reference). Chromium documents the
  refusal for aggregate *input* devices only. The `voice::mic` fallback path
  ran only in a throwaway probe (rings filled, RMS sane), never inside the
  daemon; `Degradation::MicNoAec` was never observed live.
- Check 11 (audio-capture TCC denial → `Failed` + `PermissionDenied`). The
  denial mapping is string-based on the flexaudio error ("permission
  denied"); untested.
- `NearMaxLength`/max-minutes stop (would need 170 min), `DiskLow`
  (`statvfs` path compiled, never under 500 MB), the 60 ms stall/silence
  insertion on a real device drop-out (unit-tested only), a process that
  exits mid-meeting.
- Non-macOS build (`Capture::start` and `processes()` stubs) was not
  compiled on another target.
- First run of the day had both mic paths returning literal zeros: the
  external mic was plugged into another machine and the lid was closed
  (macOS disables the built-in mic). Not a TCC problem; `meeting_permissions`
  reported `Granted` correctly throughout.

### Deviations from the design above

1. **VPIO configuration (§4.2).** `kAudioOutputUnitProperty_EnableIO` is not
   writable on the voice-processing unit (Chromium
   `audio_low_latency_input.cc`: "IO is always enabled"), so nothing is
   enabled/disabled; instead bus 0 gets a render callback that outputs
   silence with `OUTPUT_IS_SILENCE`, both bus formats (mono f32 48 kHz) are
   set, and everything happens between `uninitialize()` (coreaudio-rs's
   `new` initializes eagerly) and `initialize()`. `coreaudio-rs` does not
   re-export `objc2_audio_toolbox`; `kAUVoiceIOProperty_BypassVoiceProcessing`
   is the literal `2100` with the header named. The VPIO callback delivers
   over an mpsc (as `voice::mic` does) and one pump thread stamps into the
   rings for both paths — the callback closure cannot be recovered on a
   failed start, so it must not own the producers.
2. **Session before directory (§3 step order).** `Manifest::session` is
   non-optional (§0.1), so `CreateSession` runs first and `store.create`
   writes the first manifest with it; a `CreateSession` failure leaves no
   directory (nothing to keep). `SetSessionProject` is not sent:
   `CreateSession` persists `project` itself (`supervisor.rs`
   `Message<CreateSession>`).
3. **Index row from the first write, not from stop.** `Store::save` rewrites
   the manifest and upserts the row on every state change; otherwise check 6
   (index present, daemon killed) could not list the crashed meeting.
4. **Encoder timeline is clock-driven, not per-chunk receipt placement.**
   Frames due = recording clock − 60 ms; a source with nothing buffered gets
   zeros; a backlog over 200 ms is dropped from the front. Receipt
   `Instant`s stamp every ring entry as specified and drive the clock, but no
   per-chunk offset is computed — that is the "rejected" branch of §4.2 and
   stays rejected. Consequence: file duration == recording clock (±0.3 s)
   even when both sources die.
5. **Capture starts paused.** Devices open one after another (tap first, ~1 s
   for the VPIO unit); `Capture::start` flips `paused` to `false` only when
   all are open, so the file timeline begins with `Recording::clock`. Before
   this the files ran 1.2 s longer than `duration_ms`.
6. **`Permissions` message is `CheckPermissions`**; the reply is the wire type
   `Permissions`. `microphone` comes from
   `AVCaptureDevice.authorizationStatusForMediaType` (objc2-av-foundation,
   already in the lock via xcap); `audio_capture` has no query API on macOS,
   so it is what the last tap open taught the actor (`NotDetermined` until a
   start; observed flipping to `Granted`).
7. **Extra macOS deps**: `objc2-core-foundation` (bundle-id `CFString`) and
   `objc2-av-foundation` (above); `libc` for `proc_name`/`statvfs`. `ts`
   feature forwards to `runtime/ts` only (no `agent` type is on the wire).
8. `TrackFile.track` is a separate `Stem { Mic, System, Mix }` rather than
   "`Track|Mix`" — `Track` stays the two-sided contract type.
9. `gaps`/`degraded` live only in `Recording::manifest`, not duplicated on
   `Recording`; `Recording.tasks` (level, max-length, disk) are aborted by
   `StopMeeting`.
10. `ListMeetings.limit` defaults to 100; `meeting_get`/`meeting_delete` take
    the id as `id`.
