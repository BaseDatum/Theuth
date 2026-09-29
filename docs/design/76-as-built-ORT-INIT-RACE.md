# 76 as-built — ORT-INIT-RACE (2026-09-29)

Brief `/tmp/talk76/omp-ORT-INIT-RACE.md`. Branch `fix/ort-init-race` on `main` @ 0c87f563.
Symptom: release `t` on a scratch data dir, run-to-run: `/talk` works; or
`dictation model load task panicked: "OrtGetApiBase must be present in ONNX Runtime dylib:
DlSym { source: "dlsym(0x0, OrtGetApiBase): invalid handle" }"`; or `"Mutex poisoned"` at
`ort-2.0.0-rc.13/src/util/mutex_std.rs:15`; earlier the same day
`dictation_status.model = failed "load <data_dir>/models/onnxruntime/libonnxruntime.dylib:
failed to load from …: dlopen failed"`.

## 1. The hypothesis, checked against the as-built

Brief: a status RPC (`voice_status` → `models::onnx::ep()`) races the dictation prewarm and
makes `ort` dlopen its default `libonnxruntime` name before `ort_lib` has run `init_from`.

**Refuted for `ep()`.** `ep()` → `webgpu_ready()` → `WEBGPU_STATE == 0` →
`webgpu_wanted(accelerator::get(), plugin.is_some()) || !ort_ready()` returns `false` before
any `ort::` call: `WEBGPU_PLUGIN` is only filled by `ort_lib`, so `auto` sees no plugin, and
explicit `webgpu` stops at `!ort_ready()`. `directml_wanted()` reads atomics only. Neither
`voice::Status` (`voice/src/lib.rs:786`) nor `meeting::Status` (`meeting/src/lib.rs:2304`)
can reach `ort::api()` before a model load. Measured (§4): 5/5 headless launches with
`voice_status` + `meeting_status` + `dictation_prewarm` fired in one batch at connect, all
`accelerator=CPUExecutionProvider` and `model=ready`; the daemon log shows the first `ort`
line is `ort_lib`'s own `Loaded ONNX Runtime dylib from <data_dir>/…` every time.

**Confirmed for `devices()`** (`t --bench-models` only): `Environment::current()` →
`ort::api()` → `setup_api()` before `ort_lib` dlopens `libonnxruntime.dylib` by leaf name
and panics `Failed to load ONNX Runtime dylib` — the new unit test reproduced exactly that
against the old code (dyld searched `target/debug/`, `target/debug/deps/`, `~/lib`,
`/usr/local/lib`, …). No production status path calls it.

Audit of every `ort::` use outside `onnx.rs` (`grep -rn '\bort::' crates`): `voice/src/stt.rs`
(`Engine::load`, `Stream::open`: `ort_lib` then `from_pretrained`), `voice/src/vad.rs`
(`TenVad::new` after `Stream::open` in `live.rs:65-66`), `meeting/src/post/diarize.rs`
`session()` and `meeting/examples/live_diar_bench.rs` (`ort_lib` then `Session::builder`),
`theuth/src/bench_models.rs` (`ort_lib` then `ep`/`devices`). `decision` has no `ort` dep
(candle). The only `ort` dependents in `Cargo.lock` are `models`, `voice`, `meeting`,
`parakeet-rs`, `wavekat-vad`; neither library calls `ort::init*` itself. No ordering fix was
needed at any call site.

## 2. The chain that produces all three messages (ort rc.13 internals)

`ort::init_from(path)` → `load_dynamic::init` → `G_ORT_LIB.get_or_try_init(dlopen…)`.
`ort`'s `OnceLock` (`util/once_lock_std.rs try_init_inner`) uses `Once::call_once_force`
and **completes even when the initialiser returns `Err`**: the cell is then "initialised"
over a zeroed `MaybeUninit<Library>` (a static, so handle `0x0`). From there, in one process:

1. First `ort_lib`: `init_from` returns `Err(Dlopen)` → our `ort_lib` returned
   `load <path>: failed to load from …` → `dictation_status.model = failed "load …"` (the
   "earlier in the day" shape). Old `ort_lib` did **not** cache this ("the downloader may land
   the file").
2. Next `ort_lib` (the `Lease` at `/talk`, or the next `Prewarm`): `init_from` → `get()` is
   `Some(garbage)` → `Ok(false)` → `.commit()` → `ORT = Some(path)`. Then
   `Nemotron::from_pretrained` → `Session::builder()` → `Environment::current()` (holding
   `G_ENV`) → `api()` → `setup_api()` → `G_ORT_LIB.get()` → `dlsym(0x0, "OrtGetApiBase")` →
   `expect("OrtGetApiBase must be present in ONNX Runtime dylib")` — launch 2's panic, inside
   `spawn_blocking` → `dictation model load task panicked`.
3. That panic unwound through `G_ENV.lock()` → every later `Environment::current()` →
   `Mutex poisoned` at `mutex_std.rs:15` — launch 3's panic.

So launches 2 and 3 are the second and third `ort_lib` in a process whose first load failed;
the visible panic is never the first failure. What made the first `dlopen` of a valid file
fail is **not reproduced** (0/10 launches here, TUI and headless), and the message could not
say: `ort` rc.13 pulls `libloading` 0.9.0, whose `Display` for `DlOpen` is the bare
`dlopen failed` with dyld's reason only in `source()` (checked: a garbage `.dylib` gives the
same `dlopen failed` through `ort`, while `ctypes.CDLL` on it says `slice is not valid
mach-o file`). No in-tree writer touches the file (the desktop downloader renames a `.part`,
and only for missing files). Whatever the trigger, the fix below turns it from a
process-poisoning panic into one clear, fully worded error per call.

## 3. Changes (`crates/models/src/onnx.rs`)

| Change | Why |
|---|---|
| `static ORT: Mutex<Option<Result<PathBuf, String>>>`. `ort_lib` caches a **failed load** (`init_from` `Err`) and returns that error to every later caller, logged once at `error!`, text suffixed `(ONNX Runtime stays unavailable until restart)`. A failed **search** (`find_ort_lib`) is still not cached. | After one failed `init_from` no later one can succeed in `ort` rc.13 (§2); committing over the dead cell is what panics. |
| `load_error_text()`: the `LoadDynamicError` plus its `source()` chain — `… dlopen failed: dlopen(<path>, 0x0005): tried: '<path>' (slice is not valid mach-o file), …`. | The dyld/ld.so reason is the one line that says why an existing file did not load; `libloading` 0.9 hides it behind `dlopen failed`. |
| `ort_ready()` = `matches!(.., Some(Ok(_)))`; `runtime_version()` reads through the `Result`. | |
| `webgpu_ready()` checks `ort_ready()` first, before locking `WEBGPU_PLUGIN` / reading the accelerator. | Explicit ordering for every `ep()`/`configure()` entry, independent of `webgpu_wanted`'s short-circuit. |
| `devices()` returns empty when `!ort_ready()`, without `Environment::current()`. | The one pre-init `ort::api()` path that existed. |
| `configure()` logs `warn!` when `!ort_ready()`. | A builder can only exist if `ort` found a runtime by itself (`$ORT_DYLIB_PATH`, loader path) — name it when it happens. Not given a data dir (brief step 1 option): no caller reaches `configure` before `ort_lib`, and a second global for the data dir would be unused. |

Call sites (`voice`, `meeting`, `rpc::voice_status`, `bench_models`): unchanged — all already
`ort_lib` first (§1).

## 4. Tests and counts

- `crates/models/src/onnx.rs` `status_before_ort_lib_answers_without_loading_ort`: `ep()`,
  `devices()`, `webgpu_device()` before any `ort_lib`; `!ort_ready()` before and after. No
  test in the `models` lib binary calls `ort_lib`. Fails on old code with `Failed to load
  ONNX Runtime dylib: … dlopen(libonnxruntime.dylib, 0x0005)` from `devices()`.
- `crates/models/tests/ort_load_failure.rs` (its own binary — `ort` state is process-wide)
  `a_failed_load_is_final_for_the_process`: a garbage `libonnxruntime.dylib` in a scratch
  data dir → `ort_lib` `Err(load …)`; the same and another data dir afterwards get the same
  `Err`, `!ort_ready()`, `ep() == CPU_EP`, `devices()` empty. Skips (`skipped:`) when a
  global runtime (`$ORT_DYLIB_PATH`, Homebrew) would load for real. Fails on old code at the
  second `ort_lib(&bad)`, which returned `Ok(path)` — the launch-2 precondition.
- Launch counts, release `t --headless`, `THEUTH_DATA_DIR=/tmp/ort-fix-data` (`models` →
  `~/.theuth/models`, `secrets.key_store: file`, `rpc.listen: 127.0.0.1:7791`,
  `THEUTH_RPC__LISTEN`/`THEUTH_LAUNCH` unset), per launch `voice_status` + `meeting_status`
  + `dictation_prewarm` in one batch at connect, then `dictation_status` polled to a settled
  model (harness `/tmp/ort-fix-data/launch.ts`, Bun):
  - before (`main` @ 0c87f563): **0/5 failures**, 5× `model=ready`.
  - after: **0/5 failures**, 5× `model=ready`.
  - TUI `t` (pty) before: prewarm `dictation model loaded in 840 ms`, `/talk` started, 0 panics.
  - Release `t --headless` on a data dir whose `libonnxruntime.dylib` is 12 bytes of text
    (`/tmp/ort-fix-data/badlib.ts`): `dictation_prewarm` twice → both settle on
    `failed "load …: dlopen failed (ONNX Runtime stays unavailable until restart)"`,
    `voice_status.accelerator = CPUExecutionProvider`, no panic and no `Mutex poisoned` in the
    log — the launch-2/3 shape with the same input now ends as two identical errors. (Run
    before `load_error_text` landed; the debug test binary prints the dyld reason.)
  The nondeterministic first failure did not occur on this machine; the counts show no
  regression, not a fix of the trigger. The two regression tests are the evidence for the
  panic chain being cut.
- `cargo test -p models -p voice`, `cargo clippy -p models -p voice --all-targets` (0
  warnings), `cargo check --workspace --tests`: green.

## 5. Left open

- The trigger for the first `dlopen` failure. When it recurs, the daemon log carries one
  `ERROR models::onnx: onnxruntime: load <path>: failed to load from `<path>`: dlopen
  failed: dlopen(<path>, 0x0005): tried: … (<dyld reason>)` line, and every later
  STT/dictation/diarisation load fails with the same text instead of panicking. Upstream:
  `ort`'s `OnceLock::try_init_inner` completing on `Err` (rc.13) is the bug that makes one
  failure permanent.
