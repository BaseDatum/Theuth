# 58 step 10 — meeting capture on Windows and Linux, as built, 2026-09-23

Branch `feature/meeting-capture-xplat`. Scope: 58 §6.1 through the existing
`capture` seam; 53's feed contract, encoder, STT and everything above are
untouched. macOS behaviour is unchanged.

## What changed

| Was | Now |
|---|---|
| `capture::Capture::start`, `capture::system::spawn` under `cfg(target_os = "macos")`, "meetings need macOS 15" stubs elsewhere | Un-gated. `flexaudio 0.2` (already the dependency) implements `SourceKind::SystemLoopback` and `ProcessLoopback{target_pid}` on WASAPI (incl. process loopback) and PipeWire as well as Core Audio; `spawn` used only that API, so the body is byte-identical |
| `capture::system::processes()`: Core Audio process objects on macOS, `Ok(vec![])` elsewhere | Per OS. Windows: `IMMDeviceEnumerator` → default render endpoint → `IAudioSessionManager2::GetSessionEnumerator` → `IAudioSessionControl2::GetProcessId`, name from `QueryFullProcessImageNameW` (file stem), `active` = `AudioSessionStateActive`, de-duplicated by pid, pid 0 (system sounds) and our own pid skipped; `Err` when there is no render device. Linux: PipeWire registry listener, nodes with `media.class = Stream/Output/Audio`, pid from `application.process.id`, name from `application.name` (else `node.name`), `bundle_id` carries `application.name`; the list ends at the `core.sync` `done`, a 500 ms timer guards a wedged daemon; `Err` when no daemon answers (`connect_rc` fails) |
| — | `capture::system::supported() -> Support`: `Full` on macOS (no probe); elsewhere `Partial("no echo cancellation; headphones recommended")` when `processes()` succeeds, `None(why)` when it fails |
| `meeting::Status` | `+ supported: Support` (58 §3.3; `#[serde(tag = "kind", content = "reason")]`, ts-rs exported like its neighbours). Probed on every `meeting_status` via `spawn_blocking` — no cache, so plugging in a device or starting PipeWire is seen on the next status call |
| Mic: VPIO on macOS, else `voice::mic::Capture` (cpal) with `Degradation::MicNoAec` | Same code; the fallback branch is now the Windows/Linux normal case (58 §6.1). `permission()` returns `Granted` off macOS (no TCC) — already so |
| `MeetingPlugin::start`: `if !cfg!(target_os = "macos") { return }` | Gone. `crates/theuth`: `meeting` is an unconditional dependency and `engine_plugins` pushes it on every OS |
| — | `crates/meeting/examples/capture_probe.rs [secs] [pid]`: prints OS, `supported()`, mic permission, the process list (with the System-audio pseudo-entry the UI adds), opens the loopback on `pid` (default `SYSTEM_AUDIO`), one line per second with chunks / samples / RMS / peak, then opens the mic for 1 s and prints its degradations. Exit 2 when the loopback does not open, 1 when no frames arrived |

New target-gated deps in `crates/meeting`: `windows = "=0.54.0"` (Foundation,
System_Com, System_Threading, Media_Audio — the exact version and a subset of
the features `flexaudio-os-windows` already pulls, so no new copy in the
graph) and `pipewire = "0.10"` (the crate `flexaudio-os-linux` uses; needs
`libpipewire-0.3-dev` at build time). `Cargo.lock` gained no new packages.

## Judgement calls

- `processes()` does **not** include the `SYSTEM_AUDIO` pseudo-entry on any
  OS: macOS never did, and `StartForm.tsx` adds the "System audio" row itself
  (`process: null` → `system_audio()`). The probe prints it for parity with
  what the picker shows.
- Linux `active` is always `true`: every `Stream/Output/Audio` node is an app
  with a live output stream; the registry has no cheap "producing samples
  now" bit and a per-node param round-trip is not worth the picker's sort.
- PipeWire enumeration returns `Err` (not an empty list) when no daemon
  answers, so `meeting_processes` reports why and `supported()` becomes
  `None(reason)`; the UI's picker still shows the System-audio row. The
  Ubuntu LXC with no sound server is exactly this state.
- `disk_low` (`statvfs`) stays Unix-only and returns `false` on Windows, as
  before this step.
- Not done (58 §6.1 lists them, the brief did not ask): the Windows 11 22H2
  AEC audio effect on the mic, PipeWire `module-echo-cancel` detection.
  Both would flip `MicNoAec` off when present.

## Verification

Mac: `cargo check --workspace --all-targets`, `cargo test -p meeting`,
`cargo build -p theuth`, `cargo run -p meeting --example capture_probe -- 3`
(captures on macOS as before). Cross: `cargo check -p meeting --target
x86_64-pc-windows-msvc` for the Windows code paths. Windows 11 and Ubuntu
24.04 runs of the probe are the orchestrator's.
