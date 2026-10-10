# 158 as-built — W1E (engine: meeting detection, defaults, finalise-on-start)

Branch `feat/158-w1e`, cut from the C158 PIN 715e22d3; `feat/158-contract` ccc48db5 merged at 11a1f2ca (main not merged).
Design: `docs/design/158-meetings-around.md` §4, §5.4, §6, D13, §15, A2.

## Built

- `crates/meeting/src/detect/sensor.rs` — `mic_holders()` per OS, never opens a device.
  - macOS: `kAudioHardwarePropertyProcessObjectList` → per object `PID` + `kAudioProcessPropertyIsRunningInput`;
    names/bundle ids by joining the holding pids against `capture::system::processes()` (same list, read once over
    there); own pid and `com.apple.CoreSpeech` skipped; `active = true`.
  - Windows (`cfg(target_os = "windows")`, not compiled here): default capture endpoint for `eConsole` **and**
    `eCommunications`, `IAudioSessionManager2` sessions in `AudioSessionStateActive`, de-duplicated by pid, name via
    `QueryFullProcessImageNameW` stem. Orchestrator runs build-B `cargo-check`.
  - Linux (`cfg(target_os = "linux")`): PipeWire registry nodes `media.class = Stream/Input/Audio`, 500 ms sync guard.
- `crates/meeting/src/detect/rules.rs` — pure `Engine` (§4.4): `tick(now, holders, quiet)`, `dismiss(now, id, never)`,
  `starting(source_app)`, `recording(now, meeting, process)`, `ended(now)` → `Vec<Output>` (`Detected` / `Cleared` /
  `AppReleased`). 15 s hold (quiet pauses, drop-out resets), per-app 10 min cooldown, one prompt per overlap with
  `also`, ignore list case-insensitive, `AppReleased` once per release episode at 30 s, `Cleared{Released}` at 30 s,
  `Cleared{Started}` on any accepted start. `app_id()`: bundle id / `application.name`, Windows `<stem>.exe` lowercase.
  Calls `calendar::matcher::best(now, app_id, &[])` to fill `Detection.event` (W2 fills the candidates).
- `crates/meeting/src/detect/mod.rs` — `Detector` (engine + dictation/talk flags + poll task) lives in the `Meeting`
  actor; `spawn_poll` = `tokio::time::interval(poll_secs)` with `Skip`, `spawn_blocking(mic_holders)`, `tell(Holders)`.
  Messages `Holders`, `CurrentDetection`, `Dismiss`, `Configure`; `VoiceEvent::DictationStarted/Finished` and
  `TalkEvent::TalkStarted/Finished` subscriptions set quiet; RPC delegate bodies `rpc_detection`, `rpc_detection_dismiss`
  (Never: config write first, then the engine; a failed write changes nothing).
- `crates/meeting/src/lib.rs` — `Meeting.detect`; hooks: `StartMeeting` → `detect_starting(source)` + the prompt's
  `ProcessInfo` stands in when `processes()` misses the pid; `Begun(Ok)` → `detect_recording`; `Begun(Err)`/`StopMeeting`
  → `detect_ended`; `Status.detection`; plugin start subscribes voice/talk events and tells `Configure(cfg.detect)`.
  §6.1: `manifest.session_model = m.session_model.or(cfg.model)`; `summary_model` → `SetModel{session, manual:false}`
  before `queue_turn`, prior model restored when the slot settles (`Summarizing.restore`; a session that followed the
  default is pinned to `GetDefaultModel`'s value after). §6.2: `MeetingSettings::persist` writes every key with
  `write_config_value` / `write_nested_config_value` on a copy of `config.yaml` and renames it in (all or nothing);
  `GetSettings` / `SetSettings` messages; `rpc_settings_get` asks the actor (file when meetings are disabled).
  D13: `finalise_interrupted()` before the resume filter — `Recording`/`Paused` with a track file on disk → `Stopping`
  + `ended_at` (the existing filter then starts post); without any audio → `Failed("interrupted before any audio…")`.
  Filter predicate extracted as `resumable()`.
- `crates/meeting/examples/capture_probe.rs` prints `mic holders` beside `processes()` (§4.7 manual proof).
- `crates/meeting/Cargo.toml`: `talk` dependency (for `TalkEvent`; talk does not depend on meeting — checked with
  `cargo tree`).

## Tests (crates/meeting)

- `detect::rules::tests` (12): 14 s nothing / 15 s one prompt; Not now → quiet 10 min then prompt; two holders → one
  event with `also`; ignored alone / ignored + eligible; Wispr Flow (Minutes #244) never; live meeting quiet; dictation
  pauses the clock without reset; release 28 s nothing / 30 s `AppReleased{30}` / 60 s again, back resets; Never grows
  the list and silences; unanswered prompt withdraws at 30 s; manual start clears, stop does not re-nag; app ids per OS.
- `detect::sensor::tests::mic_holders_returns` (host probe, `Err` tolerated).
- `tests::fake_holders_reach_the_bus_as_detected` (integration: `Holders` → `Detected` on the bus, `Status.detection`,
  `meeting_detection`, `Dismiss` → `DetectionCleared{NotNow}`), `tests::never_persists_to_the_ignore_list`,
  `tests::settings_set_persists_and_applies` (figment round-trip, other sections kept, live cfg follows),
  `tests::interrupted_recordings_finalise_on_start` (D13 fixture: Recording + `mic.opus` → Stopping/resumable; Paused
  without audio → Failed; Done untouched).

## Gates

- `build-lock mbx test -p meeting --no-run` → ok; `mbx test -p meeting` → 92 passed, 1 ignored (+ note 1, oracle 1).
- `build-lock mbx clippy -p meeting --tests --examples` → 0 errors, 0 warnings.
- `cargo fmt -p meeting` clean.
- Live (this Mac): `ffmpeg -nostdin -f avfoundation -i ":0" -t 30 -f null -` held the mic; a throwaway example ticking
  `Engine` on the real `mic_holders()` every 2 s printed `holders=["<pid>:ffmpeg[ffmpeg]"]` from t+0 and
  `Detected(Detection{ app: ffmpeg, app_id: "ffmpeg", … })` at the t+16 s tick (first tick ≥ 15 s), then
  `prompt=Some("ffmpeg")` held. Sensor cost ≈ 3 ms/tick (26 process objects). Scratch engine **skipped**: free +
  speculative memory was 1.5 GB (< 4 GB rule). Windows/Linux sensors uncompiled here.

## Deviations

- `MeetingSettings` gained `#[derive(kameo::Reply)]` (a kameo `ask` reply; same as `Status`). No serde/TS change.
- Not now also cools down the apps in `also` (one answer per overlap, §4.4 rule 4); `ended()` cools down the recorded
  app and restarts every hold, so stopping mid-call does not re-prompt 15 s later. Both engine-local.
- D13 also marks a `Recording`/`Paused` manifest with no track file `Failed` (would otherwise stay "Recording" forever).
- `Output::Detected` is boxed (clippy `large_enum_variant`); `MeetingEvent::Detected(Detection)` untouched.
- `rpc_detection*` / `rpc_settings_*` take `&Runtime` but look the actor up by `MEETING_ACTOR` (frozen signatures kept).

## Hand-offs

- `capture/system.rs`: `image_name`, the PipeWire walk and the `get` property helper are duplicated in `sensor.rs`
  (capture is read-only for W1). Making them `pub(crate)` / parametrising `sessions(flow, role)` and `nodes(class)`
  would delete ~120 lines — closer or a later wave.
- Windows: a mic-only process is absent from `processes()` (render sessions), so `StartMeeting{process}` falls back to
  the prompt's `ProcessInfo`; `capture::system::spawn(pid)` still taps that pid's loopback as before.
- C158 hand-off done — `DEFAULT_IGNORE` verified per OS (compared case-insensitively with `rules::app_id`):
  theuth: `io.theuth.mac.companion` (`scripts/bundle-companion.sh:12`), `io.theuth.mac` (`tauri.conf.json:6`),
  `Theuth.Companion.exe` (`Theuth.Companion.csproj` `AssemblyName`), `theuth-desktop.exe` / `theuth-desktop`
  (`tauri.conf.json` `mainBinaryName`; Linux `application.name` [INFERENCE]). macOS bundle ids checked against
  vendor MDM docs / app catalogs (Wispr Flow MDM page lists `com.electron.wispr-flow`, `com.superwhisper.macOS`,
  `com.goodsnooze.MacWhisper`; Screen Studio / VoiceInk / Cap from their catalogs and docs); `com.apple.CoreSpeech`
  seen live in this Mac's process list. Windows added: `Wispr Flow.exe`, `Wispr Flow Helper.exe` (Wispr support
  docs), `Cap.exe` (Cap docs / issue 2019). Linux added: `OBS Studio` [INFERENCE: OBS's PipeWire client name].
  Windows-only dictation (Voice Access, voice typing) is not addable by image name — they run inside system hosts.

## Libraries

- Core Audio / WASAPI / PipeWire: the bindings already in the crate (`objc2-core-audio`, `windows 0.54`, `pipewire 0.10`);
  no new FFI. State machine, cooldowns: std/tokio time only (no scheduler crate — ~150 lines, needs the quiet-pause rule).
- YAML writes: `runtime::write_config_value` / `write_nested_config_value` (existing); values via `serde_json`
  scalars/arrays, which YAML parses (no `serde_yaml` round-trip, which would drop comments).
- New dependency in `meeting`: `talk` (workspace crate), for `TalkEvent`.

## Open

- `Engine::tick` accrues `dt` since the previous tick, so a sensor error skips time rather than counting it.
- `AppReleased` is emitted once per release episode (orchestrator ruling 14:50 over §4.5's "re-sent every 30 s":
  there is no keep-recording RPC). A companion that connects mid-release does not get it; `Status` carries no
  release state (shape frozen) — hand-off if a reconnecting island must also ask.
