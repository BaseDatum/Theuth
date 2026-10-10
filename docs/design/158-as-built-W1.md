# 158 as-built — W1 (detection + island prompt + page banner + meeting defaults + D13)

Closer: W1E, worktree `theuth-158-w1e`, branch `feat/158-w1e`. Design `docs/design/158-meetings-around.md` §4–§6, D13, §15, §16 W1, A2.
Per-worker detail: `158-as-built-W1E.md`, `-W1M.md`, `-W1W.md`, `-W1U.md`; contract `158-as-built-CONTRACT.md`.

## Workers and shas

| Worker | Area | Branch head | Merged at |
|---|---|---|---|
| C158 | contract stubs + gate fixes | `ccc48db5` (main 539e65aa) | `11a1f2ca` |
| W1E | engine: sensor, rules, actor wiring, settings, summary model, D13 | this branch (`254ff280` pre-close) | — |
| W1M | macOS island prompt (Swift) | `045e531d` | `6a060291` |
| W1W | Windows island prompt (C#) | `81752213` | `835b64b5` |
| W1U | desktop UI: banner, Settings › Meetings, R3 sentence, §6.4 offer | `d43077db` | `ae518452` (orchestrator) |

Close commits: `dca17626` (capture/system.rs de-dup), `d283a18e` (phase-1 gates), `af801b5c` (bindings, phase-2 gates, this file).
Fold: `origin/main` 4b49e238 (W5 landed) merged at `77fc503f`, no conflicts (Cargo.lock took main's + mbx re-resolved `talk`);
re-exported bindings (8 passed, 871 files); on the folded tree `mbx test -p meeting -p rpc` meeting 92 / rpc 56 passed,
`tsc --noEmit` 0, wave vitest 9 files / 32 passed, `BUILD_SLOTS=1 mbx check --workspace --tests` exit 0. W2 still absent from main.

## What landed

- **Engine** (`crates/meeting`): `detect/sensor.rs` `mic_holders()` per OS as thin glue over `capture::system` (now
  `pub(crate) process_objects(running)` / `sessions_on(flow, roles)` / `nodes(class)`; `processes()` unchanged one-liners);
  `detect/rules.rs` pure `Engine` (15 s hold, quiet pauses, 10 min cooldown, one prompt per overlap + `also`, `AppReleased`
  once per release episode, `DetectionCleared{NotNow|Never|Released|Started}`, case-insensitive ignore, `app_id` per OS);
  `detect/mod.rs` `Detector` in the `Meeting` actor (poll task, `Holders`/`CurrentDetection`/`Dismiss`/`Configure`,
  VoiceEvent + TalkEvent quiet, RPC bodies); `Status.detection`; `StartMeeting.source` handling; `session_model.or(cfg.model)`;
  `summary_model` switch/restore; `MeetingSettings` persist (all-or-nothing) + `GetSettings`/`SetSettings`; D13
  `finalise_interrupted()`; `DEFAULT_IGNORE` verified per OS; `talk` dep added to `meeting`.
- **macOS island** (W1M): `MeetingSlice.prompt` (`.detected` / `.stop`), `IslandModel.visible || prompt`, `PromptView` (A/B) +
  `StopRow` (D), keys ⏎/⎋/N, `Store+Meeting` start/dismiss/keep, `--probe-island prompt` (12/12).
- **Windows island** (W1W): `MeetingReducer`/`MeetingWire` prompt events (+ `Summarized` parity gap closed), `IslandModel.HasPrompt`,
  `RenderDetected`/`RenderStopPrompt`, `IslandController` start/dismiss/keep, `--island-prompt(-auto)` fixture, settings caption fix.
- **Desktop UI** (W1U): `lib/detection.ts`, `DetectionBanner.tsx` (§5.5), `StartForm` on the R3 sentence (A2), Settings › Meetings
  Defaults/Detection/Live tools bound to `meeting_settings_get/set`, §6.4 one-time model offer.
- **Bindings**: exported at close (`export_bindings` 8 passed, 870 files copied; gitignored).

## Gates (merged tree, head of this branch)

- Rust: `build-lock mbx test -p meeting -p rpc --no-run` → 0; `mbx test -p meeting -p rpc` → meeting 92 / rpc 56 passed, 0 failed;
  `build-lock mbx clippy -p meeting -p rpc` → 0; `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → exit 0 (only the
  pre-existing `block v0.1.6` future-incompat note).
- Windows: build-B `cargo-check` `Cargo.toml -p meeting` @ dca17626 (job 20261010T143528-325) → `Checking meeting … Finished`, 0
  errors/warnings. Note: `crates/desktop/Cargo.toml` pulls only `platform`, so the `cfg(windows)` sensor needs `-p meeting`.
- Swift: `build-lock bash crates/desktop/companion/test.sh` → 268 tests / 44 suites passed.
- C#: build-B `test` `Theuth.Companion.slnx` Debug @ dca17626 (job 20261010T144621-327) → Core 496/496, Rpc 114/114.
- UI: `build-lock bun install` (ui + packages/apps); `build-lock bunx tsc --noEmit -p .` → 0; `build-lock bunx vitest run`
  DetectionBanner, MeetingList, StartForm, settings/Meetings, lib/detection, lib/meeting-model-offer, lib/meeting-settings,
  rpc/api, store/meeting → 9 files, 32 tests passed.

## Live proof

- Engine sensor + rules on this Mac (W1E): `ffmpeg -nostdin -f avfoundation -i ":0" -t 30 -f null -` held the mic; the real
  `mic_holders()` reported `ffmpeg` from t+0 and the engine emitted `Detected{app_id:"ffmpeg"}` at the t+16 s tick, prompt held
  after. Sensor ≈ 3 ms/tick over 26 process objects.
- Island probes: W1M `--probe-island prompt` 12/12 with PNGs; W1W `--island-prompt-auto` on build-B (prompt → Start → stop prompt →
  Keep → Stop → Done, PNGs per state; see `158-as-built-W1W.md`).
- **Scratch engine + banner screenshot: not run.** Free + speculative memory was 1.5–2.5 GB all session (rule: > 4 GB) with load
  20–47 from the other waves' builds; building `t` here (~15 min) plus an engine would break the machine-sharing rule. The
  `meeting_detection` → banner path is covered by: the live engine proof above, `tests::fake_holders_reach_the_bus_as_detected`
  (Holders → `Detected` on the bus → `meeting_status().detection` → `Dismiss` → `DetectionCleared{NotNow}`), and the banner's
  vitest against a mocked `Api` (Start / Not now / Never / Stop / Keep all call the RPCs). Open item below.

## Deviations (wave level; per-worker lists in their files)

1. `AppReleased` is emitted **once per release episode**, not re-sent every 30 s (§4.5): there is no keep-recording RPC, Keep is
   local in both islands and the banner (orchestrator ruling 14:50). Islands still tolerate a re-send (ordering by `released_for_s`).
2. `MeetingSettings` derives `kameo::Reply`; `rules::Output::Detected` is boxed; D13 also marks a `Recording`/`Paused` manifest with
   no audio file `Failed`; Not now cools the `also` apps too; `ended()` cools the recorded app.
3. `EventSwitcher` still rendered through W1U's cast in `DetectionBanner.tsx` (W2's props have not landed); the calendar seam is the
   single `matcher::best(..)` call in `detect/rules.rs` (comment marks `match_for` + `clear_choice()` for whichever lands second).
4. Windows "Never for" is a third button, not a ⋮ menu (WinUI `MenuFlyout` in a tool window fail-fasts; W1W).
5. Default project in Settings is a `Select`, not a combobox; `DEFAULT_LIVE_TOOLS` duplicated in the UI (W1U).
6. `DEFAULT_IGNORE` Linux entries `theuth-desktop`, `OBS Studio` and Windows `Cap.exe` / `Wispr Flow*.exe` are documented
   identifiers, not observed here.

## Open items

- Banner screenshot against a real engine with a prompt (needs a quiet machine; recipe in the briefs). A `?sim=` fixture for the
  banner would make this engine-free — not added (not requested).
- Linux sensor compiled nowhere yet (behind `cfg(target_os = "linux")`).
- W2 lands second: switch the seam, drop the `EventSwitcher` cast, re-run meeting tests + tsc + banner vitest (one-line each).
- W1M: re-run `--probe-island clicks` with nothing of Sky's frontmost (7 keyboard steps were withheld, not failed).
- A companion connecting mid-release gets no `AppReleased` (`Status` carries no release state; shape frozen).
