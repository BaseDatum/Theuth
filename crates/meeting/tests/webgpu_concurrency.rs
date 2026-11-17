//! The live-meeting race on the shared WebGPU context (149 ENGINE-DIAR): the mic STT, the
//! system STT and the live diariser each run their ORT session on their own thread. WebGPU
//! plugin EP ≤ 0.4.0 shares one command encoder between all sessions: without
//! `models::onnx`'s run lock this returns "WebGPU validation failed. [CommandEncoder] is
//! already finished" within seconds, or kills the test process in
//! `onnxruntime_providers_webgpu.dll`. Plugin ≥ 0.5.0 on an eligible runtime (we ship 0.6.0 on
//! ORT 1.32) runs them lock-free (`models::onnx::gpu_lock_free`). Either way every run must be
//! `Ok` for the whole 20 s; the output names the mode.
//!
//! Needs the real models and a discrete GPU, so `#[ignore]`:
//!
//! ```text
//! cargo test -p meeting --test webgpu_concurrency -- --ignored --nocapture
//! ```
//!
//! Models and the runtime come from `$THEUTH_DATA_DIR`, else `~/.theuth` (the engine's
//! default data dir): `models/nemotron-streaming-en`, `models/nemotron-diarization`,
//! `models/pyannote`, `models/onnxruntime/plugins/`. Skips (passes, with a message) when any
//! of that, or a discrete GPU, is missing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use meeting::diar;
use meeting::post::diarize::{self, RATE, Streamer};
use voice::{STREAMING_MODEL_SUBDIR, Stream, WINDOW};

/// How long the three threads hammer the GPU.
const RUN: Duration = Duration::from_secs(20);
/// When the diariser thread drops its session and opens a new one.
const DROP_AT: Duration = Duration::from_secs(10);
/// Mel frames per step at the live geometry (`chunk × SUBSAMPLING`).
const SUBSAMPLING: usize = 8;

fn data_dir() -> PathBuf {
    std::env::var_os("THEUTH_DATA_DIR")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|p| {
            if p.join("models").is_dir() {
                p
            } else {
                p.join(".theuth")
            }
        })
        .unwrap_or_default()
}

/// Deterministic noise at `[-0.3, 0.3]`: enough energy for every kernel to run.
fn noise(samples: usize) -> Vec<f32> {
    let mut x: u32 = 0x9E37_79B9;
    (0..samples)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32 - 0.5) * 0.6
        })
        .collect()
}

/// Step the diariser over `pcm` until `until`, wrapping at the end of the buffer.
fn step_until(s: &mut Streamer, pcm: &[f32], until: Instant) -> anyhow::Result<usize> {
    let hop = s.geometry().chunk * SUBSAMPLING;
    let (mut m0, mut n) = (0, 0);
    while Instant::now() < until {
        if s.step(pcm, m0)?.is_empty() {
            m0 = 0;
            continue;
        }
        m0 += hop;
        n += 1;
    }
    Ok(n)
}

#[test]
#[ignore = "needs the real models and a discrete GPU"]
fn three_webgpu_sessions_run_concurrently_and_one_drops_mid_run() -> anyhow::Result<()> {
    let data_dir = data_dir();
    let models_dir = data_dir.join("models");
    let stt_dir = models_dir.join(STREAMING_MODEL_SUBDIR);
    let Some(models) = diarize::models(&models_dir, &data_dir) else {
        eprintln!(
            "skip: diarization models missing under {}",
            models_dir.display()
        );
        return Ok(());
    };
    if !stt_dir.is_dir() {
        eprintln!("skip: streaming STT model missing at {}", stt_dir.display());
        return Ok(());
    }

    let t = Instant::now();
    // Progress on stderr (unbuffered): a native crash during load then shows which step died.
    eprintln!("data dir {}; opening the diariser", data_dir.display());
    let mut diariser = diar::open(&models)?;
    eprintln!(
        "diariser open on {} (ONNX Runtime {}) in {:?}",
        models::onnx::ep(),
        models::onnx::runtime_version().unwrap_or("of unknown version"),
        t.elapsed()
    );
    let Some(gpu) = models::onnx::webgpu_device() else {
        eprintln!("skip: no WebGPU device; sessions are on the CPU, which cannot race");
        return Ok(());
    };
    let mut mic = Stream::open(&stt_dir, &data_dir)?;
    let mut system = Stream::open(&stt_dir, &data_dir)?;
    eprintln!("sessions on {gpu} loaded in {:?}", t.elapsed());
    eprintln!(
        "run lock {}",
        if models::onnx::gpu_lock_free() {
            "off: concurrent WebGPU sessions"
        } else {
            "on: serialised WebGPU sessions"
        }
    );

    let pcm = Arc::new(noise(30 * RATE as usize));
    let window: [f32; WINDOW] = pcm[..WINDOW].try_into().unwrap();
    // Warm up one at a time: a session's first run compiles its shaders (seconds). The wait
    // stats below are then the steady state the meeting runs in, not the compile queue.
    let t = Instant::now();
    diariser.step(&pcm, 0)?;
    mic.feed(&window)?;
    system.feed(&window)?;
    let (waits0, total0, _) = models::onnx::gpu_wait_stats();
    eprintln!("warm-up (one run each, sequential) {:?}", t.elapsed());
    let start = Barrier::new(3);
    let failed = AtomicBool::new(false);
    let t0 = Instant::now();

    std::thread::scope(|s| {
        let stt = |name: &'static str, mut stream: Stream| {
            let (start, failed) = (&start, &failed);
            s.spawn(move || -> anyhow::Result<usize> {
                start.wait();
                let mut n = 0;
                while t0.elapsed() < RUN && !failed.load(Ordering::Relaxed) {
                    if let Err(e) = stream.feed(&window) {
                        failed.store(true, Ordering::Relaxed);
                        anyhow::bail!("{name} STT feed #{n}: {e:#}");
                    }
                    n += 1;
                }
                eprintln!("{name} STT: {n} windows");
                Ok(n)
            })
        };
        let mic = stt("mic", mic);
        let system = stt("system", system);
        let diar = {
            let (start, failed, pcm) = (&start, &failed, pcm.clone());
            s.spawn(move || -> anyhow::Result<usize> {
                start.wait();
                // First session until the drop point, dropped while the STT threads run.
                let mut n = step_until(&mut diariser, &pcm, t0 + DROP_AT).map_err(|e| {
                    failed.store(true, Ordering::Relaxed);
                    e.context("diariser step (first session)")
                })?;
                drop(diariser);
                eprintln!("diariser: dropped after {n} steps at {:?}", t0.elapsed());
                let mut second = diar::open(&models).map_err(|e| {
                    failed.store(true, Ordering::Relaxed);
                    e.context("diariser reopen")
                })?;
                n += step_until(&mut second, &pcm, t0 + RUN).map_err(|e| {
                    failed.store(true, Ordering::Relaxed);
                    e.context("diariser step (second session)")
                })?;
                eprintln!("diariser: {n} steps");
                Ok(n)
            })
        };
        let results = [
            ("mic", mic.join()),
            ("system", system.join()),
            ("diariser", diar.join()),
        ];
        let mut errors = Vec::new();
        for (name, r) in results {
            match r {
                Ok(Ok(n)) => assert!(n > 0, "{name} never ran"),
                Ok(Err(e)) => errors.push(format!("{name}: {e:#}")),
                Err(_) => errors.push(format!("{name}: thread panicked")),
            }
        }
        let (waits, total, max) = models::onnx::gpu_wait_stats();
        let (waits, total) = (waits - waits0, total - total0);
        eprintln!(
            "run lock: {waits} acquisitions in {:?}, waited {:?} total, mean {:?}, max {:?} (max includes the reopened diariser's first run)",
            t0.elapsed(),
            total,
            total.checked_div(waits.max(1) as u32).unwrap_or_default(),
            max
        );
        assert!(
            errors.is_empty(),
            "WebGPU runs failed:\n{}",
            errors.join("\n")
        );
    });
    // The plugin's DLL teardown at process exit fail-fasts (0xC0000409) after the test has
    // passed and every session is gone; the engine never unloads it. End the process before
    // that, so the verdict is the runs above.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        eprintln!("test passed; terminating before the plugin's exit-time teardown");
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    Ok(())
}
