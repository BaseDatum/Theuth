# As-built — the process-wide WebGPU run lock (149 GPULOCK)

Branch `fix/webgpu-run-lock` off `origin/main 82433e4b`, 2026-10-09. Fixes the engine's death during live
meetings on PCs with a discrete GPU (diagnosis: `~/.theuth/briefs/2026-10-09/149-windows/reports/ENGINE-DIAR.md`).
Sky's go, 17:33: "implement like a mutex … so that each thread can only write to the GPU independently in its
own time slot … a small latency cost but it will continue to work for now until we get that merged PR build".

## The cause, in one paragraph

A live meeting runs ONNX Runtime `Session::run` on three threads at once — mic STT, system STT
(`crates/meeting/src/lib.rs` `stt::spawn` ×2) and the live diariser (`diar::spawn`). With `models.accelerator:
auto` on a discrete GPU, `models::onnx::configure` puts every session on the WebGPU plugin EP 0.4.0 with one
device. In that plugin all sessions share ONE `WebGpuContext` with ONE unlocked command encoder
(`webgpu_context.h:229-260`, `.cc:1047-1052` at tag `plugin-ep-webgpu/v0.4.0`): a run ending on one thread
`Finish()`es the encoder another thread is still recording into. Symptom: `WARN meeting::diar: meeting: live
diarization failed: WebGPU validation failed. [CommandEncoder (unlabeled)] is already finished.`, then a native
fault in `onnxruntime_providers_webgpu.dll`; `t.exe` dies with no panic line. Upstream fixed it in
microsoft/onnxruntime #32587 (merged 2026-09-28) and #32874 (2026-10-03); no released plugin (NuGet
`Microsoft.ML.OnnxRuntime.EP.WebGpu` 0.1.0–0.4.0) contains the fix.

## What changed

| File | Change |
|---|---|
| `crates/models/src/onnx.rs` | `static WEBGPU_RUN: Mutex<()>` with the why/remove-when comment; `gpu_guard(on_gpu) -> Option<MutexGuard>` (poison recovered like `WEBGPU_CHOICE`); `gpu_serial(on_gpu, f)`; `gpu_wait_stats()` (acquisitions, total and max wait); `Guarded<T>` (a model + its WebGPU flag: `Deref`/`DerefMut`, `guard()`, and `Drop` under the lock via `ManuallyDrop`). `configure` now returns `(SessionBuilder, bool)` — the flag is `true` only when `with_devices(WebGPU)` succeeded for THAT builder, never a global guess. Three unit tests. |
| `crates/meeting/src/post/diarize.rs` | `session()` returns `Guarded<Session>`; `Streamer.sess` and `Embedder.0` are `Guarded`; `Streamer::step` and `Embedder::embed_frames` hold `guard()` across `run` only (dropped before outputs are read). Drops of both go through `Guarded::drop`. |
| `crates/voice/src/stt.rs` | `exec_config(on_gpu: Arc<AtomicBool>)` sets the flag from `configure`'s return for each of parakeet-rs's two sessions per model; `guarded(load)` wraps the model. `Engine(Mutex<Guarded<ParakeetTDT>>)`, `Stream(Guarded<Nemotron>)`; `transcribe` and `feed` run under `gpu_serial`. |
| `crates/voice/src/lib.rs` | exports `Stream` and `WINDOW` (the GPU test drives the streaming model directly, without the VAD gate). |
| `crates/meeting/examples/live_diar_bench.rs` | same `Guarded<Session>` treatment as `diarize::session` (it also runs a `Utterer` on a second thread). |
| `crates/meeting/tests/webgpu_concurrency.rs` | new, `#[ignore]`: the GPU concurrency test (below). |
| `crates/theuth/src/main.rs` | Fix B: WARN+ written synchronously (below). |

Unchanged on purpose: the `auto` policy, the adapter choice, `configure`'s device/options (`with_devices(one
device, None)`), session *construction* (not serialised — all meeting sessions are built before the live tasks
start; see follow-ups).

## Every ORT session site

| Site | Built through | Lock | Why |
|---|---|---|---|
| `voice::stt::Stream` (Nemotron streaming: encoder + decoder-joint) — live STT ×2 in meetings, dictation, talk | `exec_config` → `configure` | **run + drop** | WebGPU-possible |
| `voice::stt::Engine` (Parakeet TDT: encoder + decoder-joint) — `Transcriber` actor (idle-unloaded: that drop is locked too), post pass, `--bench-models` | `exec_config` → `configure` | **run + drop** | WebGPU-possible |
| `meeting::post::diarize::Streamer` — live diariser (`diar::open`) and offline `probabilities` | `diarize::session` → `configure` | **run + drop** | WebGPU-possible |
| `meeting::post::diarize::Embedder` (wespeaker) — post pass, speaker profile match | `diarize::session` → `configure` | **run + drop** | WebGPU-possible |
| `examples/live_diar_bench.rs` `session()` | `configure` | **run + drop** | WebGPU-possible (runs beside a `Utterer`) |
| `voice::vad::Vad` (TEN-VAD, wavekat-vad `session_from_memory`) — every `Utterer`, `transcribe::speech` | bare `Session::builder()`, no EP | none | CPU-only: wavekat-vad never calls `configure` |
| `decision::gliner::onnx::Loaded` (feature `gliner-onnx`) | bare builder, optional CoreML | none | CPU/CoreML only, never WebGPU |

`configure` has no other callers (grep `onnx::configure(` over `crates/`). parakeet-rs's `with_custom_configure`
closure is the only way a parakeet-rs session gets an EP; both of its sessions per model go through it.

## Lock scope and cost

The guard is held for exactly one `Session::run` (diariser, embedder) or one parakeet-rs call
(`transcribe_chunk` = one encoder run + the decoder loop; `transcribe_samples` = the whole TDT pass), and for the
model drop. Output extraction runs unlocked. The cost is queueing only; `gpu_wait_stats()` measures it (the
build-B numbers are in the GPU test output below).

## Fix B — WARN+ survive a native crash

`crates/theuth/src/main.rs`: the engine log keeps its non-blocking appender for INFO and below; a second
`RollingFileAppender` on the same `theuth.log`, filtered to `WARN` and above (`MakeWriterExt::with_max_level`),
is `and`-ed in and writes synchronously. Both appenders open the file `O_APPEND` and the fmt layer writes one
line per call, so lines never interleave. Measured with a scratch binary (200 000 INFO events, release,
this Mac): non-blocking only 911/1029/1073 ns per event, tee 899/959/1154 ns per event — same within noise. In
the tee run the WARN landed at line 60 505 of 188 499 while the non-blocking worker was still draining; no torn
lines. (The non-blocking appender is lossy at 128 k queued lines in both configurations: 188 499 of 200 000
arrived. Pre-existing, unchanged.)

## Tests

1. `crates/models/src/onnx.rs` unit tests: `cpu_sessions_never_take_the_run_lock` (lock held, `gpu_guard(false)`
   is `None` and `gpu_serial(false, …)` returns), `gpu_runs_never_overlap_and_a_drop_waits_its_turn` (4 threads ×
   50 runs/drops with a 200 µs critical section; an in-flight counter never exceeds 1),
   `a_panic_under_the_lock_does_not_poison_later_runs`.
2. `crates/meeting/tests/webgpu_concurrency.rs`, `#[ignore]`: builds the live `Streamer` and two `Stream`s on
   WebGPU, runs them on three threads for 20 s on synthetic noise, drops the diariser at 10 s and opens a second
   one; every call must be `Ok`. Data dir from `$THEUTH_DATA_DIR`, else `%USERPROFILE%`/`$HOME` `/.theuth` (the
   engine's default). Skips with a message when the models or a discrete GPU (`webgpu_device()` is `None`
   after the first session) are missing. Warms each session up once, sequentially, then reports the lock
   wait stats of the concurrent phase. On Windows it ends with `TerminateProcess(…, 0)` after the verdict:
   the plugin's exit-time teardown fail-fasts (below).

## The GPU test on build-B (Quadro M2000M, D3D12, plugin 0.4.0, debug build — the service ignores `config` for cargo)

Data dir found at `C:\Users\user\.theuth` (the engine's default, `%USERPROFILE%\.theuth`), plugin at
`models\onnxruntime\plugins\onnxruntime_providers_webgpu.dll`. The three sessions load in ~18–20 s.

**Before (0f0e19fb, test only, no lock) — job `20261009T170702-1`:**
```
running 1 test
sessions on GPU NVIDIA NVIDIA Quadro M2000M … 0.4.0 …onnxruntime_providers_webgpu.dll loaded in 20.4924681s
error: test failed, to rerun pass `-p meeting --test webgpu_concurrency`
Caused by:
  process didn't exit successfully: `…\webgpu_concurrency-5e1dc29e1c24597b.exe --ignored --nocapture`
  (exit code: 0xc0000005, STATUS_ACCESS_VIOLATION)
```
The test process died natively inside the first seconds of concurrent runs, with no Rust error — the
production symptom (`exit: -1073741819`, 873 s including a 14 min cold compile). A first attempt at the same
sha lost the HTTP connection after 595 s and the service answered again a minute later; no log id came back.

**After (a1e6e122, lock in) — job `20261009T172206-5`:**
```
mic STT: 6 windows
system STT: 10 windows
diariser: dropped after 1 steps at 23.828561s
diariser: 1 steps
run lock: 21 acquisitions, waited 45.920726s total, mean 2.186701238s, max 20.9272352s
test three_webgpu_sessions_run_concurrently_and_one_drops_mid_run ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 42.41s
error: test failed … (exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN)
```
Every run `Ok`, the mid-run drop and reopen fine. The `0xC0000409` fail-fast came *after* `test result: ok`,
with every session already dropped — process exit (the plugin DLL's teardown), not the race.

**After, final test (79fb799f: warm-up first, steady-state stats, clean exit) — job `20261009T173528-6`, exit 0:**
```
sessions on GPU NVIDIA NVIDIA Quadro M2000M … loaded in 17.8307222s
warm-up (one run each, sequential) 4.9556948s
system STT: 14 windows
mic STT: 11 windows
diariser: dropped after 1 steps at 21.318868s
diariser: 1 steps
run lock: 30 acquisitions in 22.5738903s, waited 41.5794366s total, mean 1.38598122s, max 19.4367739s (max includes the reopened diariser's first run)
test passed; terminating before the plugin's exit-time teardown
```

**The lock's measured cost (build-B, debug build).** Mean wait 1.39 s, max 19.4 s over 30 acquisitions in
22.6 s. The distribution is bimodal: the diariser's first step with a non-empty speaker cache (a new input shape,
so [INFERENCE] a fresh WGSL→DXIL compile of the shape-specialised kernels) held the lock for ~19 s, and both
STT threads queued behind it (2 × 19.4 s ≈ 38.8 s of the 41.6 s total). Outside that, 28 acquisitions
waited ≈ 2.8 s together — **≈ 0.1 s mean**, i.e. one neighbour's STT window in a debug build; the release
engine's windows are shorter. 25 STT windows fit in the ~3 s the diariser was not holding the lock. The
reopened diariser's first step took ~1.3 s (program cache hits). What this means for a meeting: the first
diariser step after the cache fills (and any later new shape) now stalls both live STT tracks for as long as
that compile takes, where before they ran through it in parallel (and crashed). Steady state costs the
queueing of one run.

## Gates

- Mac, `BUILD_LABEL=gpulock build-lock mbx test -p models -p meeting -p voice -p theuth --no-run`, then
  `mbx test -p models -p meeting -p voice -p theuth`: meeting lib 69 + `tests/note` 1 + `tests/oracle` 1 +
  `tests/webgpu_concurrency` 0 (1 ignored); models lib 25 (the three new lock tests included) +
  `tests/ort_load_failure` 1; theuth 3; voice lib 27 (first full run: 26 + `live::tests::
  utterer_returns_the_spoken_sentence_punctuated` red on the final `.` — the load-sensitive flake recorded in
  76-as-built-VOICE-OUT/TALK-CORE and 144-as-built-W1; green on the filtered rerun and on the second full run).
- `cargo fmt -p models -p meeting -p voice -p theuth`: clean.
- Windows build-B, `cargo-check -p models -p meeting -p voice -p theuth --tests` at 79fb799f (the last code
  commit; the as-built commit after it is docs only): job `20261009T175613-8`, exit 0, no warnings. A first
  run with `--examples` added (job `20261009T173733-7`) failed only in `examples/live_diar_bench.rs`
  `cpu_time()`'s `libc::getrusage`, Unix-only code that predates this branch (main `0e0c60f6`); the example
  has never built on Windows and is not part of the gate.
- Windows build-B GPU test: above (before `0xC0000005`, after exit 0).

## Follow-ups (out of scope here)

- **Windows crash dumps**: WER LocalDumps for `t.exe` (`HKLM\SOFTWARE\Microsoft\Windows\Windows Error
  Reporting\LocalDumps\t.exe`, `DumpType=1`) is an installer/registry change.
- **Engine respawn by the desktop shell**: never designed (ENGINE-DIAR Q3); Sky rules on the design.
- **Remove the lock** when the WebGPU plugin ships #32587 (+ #32874): plugin 0.5.0 shipped them with #33082. The
  capability gate is in, but it stays off until a GPU run passes. See "The lock made conditional" below.
- ~~**Session construction is not serialised.**~~ — where the lock stays, WebGPU session creation now takes
  it too (below): the plugin's serialized-compatibility contract requires it.
- **Exit-time fail-fast.** With the WebGPU plugin loaded the test process dies with `0xC0000409` at process exit,
  after every session is gone. `t.exe` loads the same plugin and quits through the same path, so the engine's
  exit code on quit may read as a crash to a future supervisor (ENGINE-DIAR Fix C) — check before treating
  a non-zero exit as a reason to respawn. Not investigated here.
- **First-step stalls.** The diariser's shape-change compiles (~19 s on the M2000M, debug build) now block the
  STT tracks. If that shows in meetings, the cheapest relief is the diagnosis's option (a): the live diariser
  on the CPU (it was tuned for a 4-thread CPU cap, RTF 0.035) while STT keeps the GPU — a `configure` policy
  change, so Sky's call; not made here.
- The macOS built-in WebGPU EP (explicit `models.accelerator: webgpu`) and Linux/Vulkan take the same lock
  (same `configure` branch); not run.

## The lock made conditional — held until GPU-verified (ORT 1.32.0 + plugin 0.6.0, branch `chore/ort-1.32-webgpu-0.6`, 2026-11-17)

**Status: the lock is still unconditional.** The gating below is implemented, but `WEBGPU_CONCURRENT_VERIFIED` is
`false`. On build-B the pinned pair crashed natively while loading, before any concurrent run (below), so per the
briefing's safety rule the run was not retried. Flip the const when `ort-webgpu-test.ps1` passes on a discrete GPU.

Pins (`scripts/ort-version.env`): ORT 1.27.0 → **1.32.0** (all four archives re-hashed), `EP.WebGpu` 0.4.0 →
**0.6.0** ("matched with ONNX Runtime 1.32.0": same contrib-op schemas, bump both together). `ort` stays
`=2.0.0-rc.13` with `api-22`. 0.6.0's `runtimes/win-x64/native/` has no `dxil.dll` and adds an optional preview
D3D12 Agility SDK under `D3D12/` (activates only in Developer Mode); `release-models.sh` now publishes
`onnxruntime_providers_webgpu.dll` + `dxcompiler.dll` and not the SDK.

**The rule** (plugin 0.5.0 release notes, "Runtime Compatibility", restated in 0.6.0): concurrent independent
Sessions need plugin ≥ 0.5.0 (#32587, #32874, #33082) on ORT ≥ 1.31, 1.30.1+ in 1.30 or 1.28.3+ in 1.28; anything
else (1.29.x included) is "serialized compatibility", where the caller must serialise every WebGPU operation on the
device, Session creation/destruction included. `models::onnx::webgpu_concurrent(core, plugin)` is that rule;
unknown or unparseable versions are `false`.

**Detection**, once, in `register_webgpu`:
- core: `runtime_version()` — `OrtGetApiBase()->GetVersionString()` of the library `ort_lib` loaded (rc.13 keeps its
  handle private, so `read_runtime_version` reopens the same file with `libloading`); replaces the old `VERSION`-file
  read, so bundled runtimes (`.app`, Windows package) report too. Also gates `WEBGPU_MIN_ORT`.
- plugin: the chosen device's EP metadata `version` key (`kOrtEpDevice_EpMetadataKey_Version`, "available for all
  plugin EPs"; `ort` rc.13 has no wrapper, so raw `EpDevice_EpMetadata` as for adapter classification).
- the built-in EP (a runtime that already offers a `WebGpuExecutionProvider` device: macOS) is never concurrent. Its
  metadata `version` is the core's (`GPU Apple 1.32.0` on this Mac), which would pass a naive plugin check. The
  1.31.0 core notes name #32587 for the in-tree WebGPU EP and 1.32.0 "synchronized per-session command
  recording", but neither names #32874 or #33082, so the macOS EP keeps the lock.

`WEBGPU_CONCURRENT` (= eligible `&& WEBGPU_CONCURRENT_VERIFIED`) is stored before `WEBGPU_STATE` turns 1, so no
session reaches the GPU before the mode is fixed. `gpu_guard(on_gpu)` takes `WEBGPU_RUN` only when
`on_gpu && !gpu_lock_free()`; `gpu_wait_stats` counts only real acquisitions (0 in concurrent mode). One INFO line:
`onnxruntime: WebGPU sessions run concurrently, no run lock (ONNX Runtime 1.32.0, WebGPU plugin 0.6.0)`, `…
serialised under one run lock (…): eligible for concurrent sessions, held until verified on a GPU` (today, with the
pinned pair), or `… serialised under one run lock (ONNX Runtime …, …); concurrent needs …`.

**Session creation under the lock (serialised mode).** `diarize::session` and `live_diar_bench::session` wrap
`commit_from_file` in `gpu_serial(on_gpu, …)`. parakeet-rs creates its two sessions inside `from_pretrained`, so
`stt::guarded` holds `gpu_serial(onnx::ep() == WEBGPU_EP, …)` across the whole load (`configure` inside it takes no
lock, so no re-entry).

**Same-Session overlap (concurrent mode's remaining rule).** No caller uses I/O binding (no `IoBinding` /
`create_binding` in `crates/`). Each Session is driven by one thread at a time, which the types enforce:
`ort` rc.13's `Session::run` takes `&mut self`; `Streamer::step`, `Embedder::embed_frames` and `Stream::feed` take
`&mut self` (`Stream` is `Send`, not `Sync`, one owner task); `Engine` keeps its model behind a `Mutex`.

### Mac (M3 Max, CPU, debug) against Microsoft's `onnxruntime-osx-arm64-1.32.0.tgz`

Scratch data dir `/tmp/ort132/home/.theuth` (the 1.32.0 dylib as `models/onnxruntime/libonnxruntime.dylib`, model
dirs symlinked read-only from `~/.theuth/models`); test binaries run with `HOME` and `THEUTH_DATA_DIR` pointing
there. A throwaway test (not committed) asserted `runtime_version() == Some("1.32.0")`, created a session with
`ort::ep::CoreML` `error_on_failure()` (pyannote segmentation), then with explicit `webgpu` registered the
built-in EP (`GPU Apple 1.32.0`), `gpu_lock_free() == false`, and a WebGPU session created under the lock (1
acquisition). Suites: models lib 28 + `ort_load_failure` 1; voice lib 74 (3 ignored), then `stt:: live:: vad::
transcribe` with `--include-ignored` 10/11 — `live::tests::utterer_returns_the_spoken_sentence_punctuated` drops the
final `.` and gives the identical text on the production 1.27.0 runtime (the known flake above); meeting lib 173
(2 ignored) + `enroll_segments_on_a_real_meeting_copy` (wespeaker on a copied meeting, cosine 0.976) + `note` 1 +
`oracle` 1; `webgpu_concurrency --ignored` loads the diariser on the CPU and skips (no discrete GPU). `calendar`'s
`fastmail_floating_time_is_local_and_unknown_hosts_are_not_links` fails with the real `HOME` too (no ORT).

### build-B (Quadro M2000M, D3D12, Windows 11 25H2 26200.9974, Developer Mode on, debug) — crashed while loading

`crates/desktop/companion-windows/ort-webgpu-test.ps1` (kept as the dev tool for the next pin bump): downloads
Microsoft's win-x64 zip and the nupkg into `%TEMP%\theuth-ort-verify\`, checks them against `ort-version.env`, lays
out a scratch data dir with exactly what `release-models.sh` ships (`onnxruntime.dll`, `VERSION`,
`plugins\{onnxruntime_providers_webgpu.dll, dxcompiler.dll}`) plus junctions to the real model dirs, sets
`THEUTH_DATA_DIR` and runs the test once; `-NoPlugin` leaves `plugins\` out (CPU control, the plugin never loads).

- `20261117T101056-1` cargo-test `-p meeting --test webgpu_concurrency --no-run` at `60853a7ff`: exit 0, 1110 s.
- `20261117T102934-2` the script at `60853a7ff`: both sha256 ok; it compiled again (16 m 44 s), then
  `running 1 test` → `process didn't exit successfully: …webgpu_concurrency-1e8930828f464f38.exe --ignored
  --nocapture (exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN)`, exit -1073740791. The test ran ≈ 1 s (1005.3 s
  total less the compile) and printed nothing, i.e. it died in `diar::open` — registration, device choice or the first
  session on WebGPU — before `sessions on … loaded`, so before any concurrent run. With 1.27.0/0.4.0 that load took
  ≈ 18 s and succeeded. Not retried (safety rule).
- `20261117T104725-3` read-only facts: `System32\D3D12Core.dll` present, Developer Mode = 1 (so 0.6.0's preview
  Agility SDK path is live, and we ship no `D3D12\` folder), NVIDIA driver 32.0.15.8241; no Application Error / WER
  record for the crash was found.
- `20261117T104858-4` the script with `-NoPlugin` at `bed237f8e` (the test now prints load progress): `diariser
  open on CPUExecutionProvider (ONNX Runtime 1.32.0) in 256.9734ms`, skip, exit 0. So rc.13 loads 1.32.0 on
  Windows, `read_runtime_version` works there, and a 1.32.0 CPU session creates: the crash is in the WebGPU plugin
  path.

[INFERENCE] 0xC0000409 with no Rust panic text is a native fail-fast (`__fastfail` or `abort()` after an uncaught
C++ exception) inside `onnxruntime_providers_webgpu.dll` or Dawn. Candidates: the missing `D3D12\` Agility SDK
folder on a Developer Mode box, the missing `dxil.dll` (0.6.0 does not ship one), or the old Maxwell GPU or driver.
Next runs, each needing Sky's go: the same script with the `D3D12\` folder copied from the nupkg; then one with
`dxil.dll` from 0.4.0. **Do not publish these pins to the bucket before one of them passes:** Windows users with a
discrete GPU would get the same crash at their first ONNX session.
