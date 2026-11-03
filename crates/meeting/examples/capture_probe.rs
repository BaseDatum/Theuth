//! What the meeting capture sees on this host (58 §6.1): `supported()`, the
//! process picker, then `SYSTEM_AUDIO` loopback for N seconds with frames /
//! RMS / degradations. Non-zero exit when the loopback does not open or no
//! frames arrive. `cargo run -p meeting --example capture_probe -- [secs] [pid]`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use meeting::capture::{Feed, rings};
use meeting::{SYSTEM_AUDIO, capture, system_audio};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug")),
        )
        .with_writer(std::io::stderr)
        .init();
    let mut args = std::env::args().skip(1);
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let pid: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SYSTEM_AUDIO);

    println!("os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("supported: {:?}", capture::system::supported());
    println!("mic permission: {:?}", capture::mic::permission());

    let t = Instant::now();
    match capture::system::processes() {
        Ok(list) => {
            println!("processes ({} in {:?}):", list.len() + 1, t.elapsed());
            for p in std::iter::once(system_audio()).chain(list) {
                println!(
                    "  {:>7}  {}{}  {}",
                    p.pid,
                    if p.active { "*" } else { " " },
                    p.name,
                    p.bundle_id.as_deref().unwrap_or("")
                );
            }
        }
        Err(e) => println!("processes: ERROR {e:#}"),
    }
    // 158 §4.7: the mic sensor beside the output picker.
    let t = Instant::now();
    match meeting::detect::sensor::mic_holders() {
        Ok(list) => {
            println!("mic holders ({} in {:?}):", list.len(), t.elapsed());
            for p in list {
                println!(
                    "  {:>7}  {}  {}  [{}]",
                    p.pid,
                    p.name,
                    p.bundle_id.as_deref().unwrap_or(""),
                    meeting::detect::rules::app_id(&p)
                );
            }
        }
        Err(e) => println!("mic holders: ERROR {e:#}"),
    }

    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (feed, mut enc, _stt): (Feed, _, _) = rings(&paused);
    let t = Instant::now();
    // 173: the watchdog's reports print as they arrive; nothing drains them otherwise.
    let (reports, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let thread = match capture::system::spawn(
        pid,
        None,
        feed,
        stop.clone(),
        reports,
        tracing::Span::none(),
    ) {
        Ok(t) => t,
        Err(e) => {
            println!("loopback pid {pid}: ERROR {e:#}");
            std::process::exit(2);
        }
    };
    println!(
        "loopback pid {pid}: open in {:?}; capturing {secs}s",
        t.elapsed()
    );

    let (mut chunks, mut samples, mut sumsq, mut peak) = (0u64, 0u64, 0f64, 0f32);
    let start = Instant::now();
    let end = start + Duration::from_secs(secs);
    let mut next = start + Duration::from_secs(1);
    while Instant::now() < end {
        while let Ok((_, pcm)) = enc.pop() {
            chunks += 1;
            samples += pcm.len() as u64;
            for &s in &pcm {
                sumsq += f64::from(s * s);
                peak = peak.max(s.abs());
            }
        }
        while let Ok(r) = rx.try_recv() {
            println!("  watchdog: {r:?}");
        }
        if Instant::now() >= next {
            next += Duration::from_secs(1);
            println!(
                "  t+{:>2}s chunks={chunks} samples={samples} rms={:.4} peak={peak:.4}",
                start.elapsed().as_secs(),
                (sumsq / samples.max(1) as f64).sqrt()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    let _ = thread.join();

    let expected = capture::RATE as u64 * secs;
    println!(
        "done: chunks={chunks} samples={samples} ({:.0}% of {expected}) rms={:.4} peak={peak:.4}",
        100.0 * samples as f64 / expected as f64,
        (sumsq / samples.max(1) as f64).sqrt()
    );
    if samples == 0 {
        // A tap/loopback delivers nothing while no app plays (macOS idle
        // behaviour); play something during the probe to see frames.
        println!("WARN: no frames — nothing was playing, or the backend is silent while idle");
    }
    match mic_probe() {
        Ok(d) => println!("mic degradations: {d:?}"),
        Err(e) => {
            println!("mic: ERROR {e}");
            std::process::exit(3);
        }
    }
}

/// Open the mic path for one second; the degradations it reports.
fn mic_probe() -> Result<Vec<meeting::Degradation>, String> {
    let paused = Arc::new(AtomicBool::new(false));
    let (feed, mut enc, _stt) = rings(&paused);
    let stop = Arc::new(AtomicBool::new(false));
    let (reports, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (mic, degraded) = capture::mic::open(feed, stop.clone(), reports, tracing::Span::none())
        .map_err(|e| format!("{e:#}"))?;
    std::thread::sleep(Duration::from_secs(1));
    let mut n = 0usize;
    while enc.pop().is_ok() {
        n += 1;
    }
    stop.store(true, Ordering::SeqCst);
    drop(mic);
    println!("mic: {n} chunks in 1s");
    Ok(degraded)
}
