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
- **Remove the lock** when the WebGPU plugin ships #32587 (+ #32874); it may also need explicit per-session
  streams (`OrtEp CreateSyncStreamForDevice`). The comment on `WEBGPU_RUN` says the same.
- **Session construction is not serialised.** Meeting sessions are built before the live tasks start, but a
  dictation/talk `Stream` can be built while a meeting runs (and vice versa). The GPU test built a second
  diariser while two STT threads ran and every run stayed `Ok` (one observation on one GPU) — no evidence yet
  that `commit_from_file` needs the lock.
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
