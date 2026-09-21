//! `decision lifecycle` (57 §2.1 item 3): process memory before load, after the first inference
//! and after the session is dropped, per checkpoint and device — the number the "resident only
//! while in use" rule stands on. Two columns per sample: **footprint** (macOS `phys_footprint`,
//! Activity Monitor's Memory — what the OS charges the process) and **rss** (`resident_size`,
//! which on macOS keeps counting `free`d pages until the kernel reclaims them).
//!
//! `cargo run --release -p decision --example lifecycle -- [--device metal|cpu] [--repeat N] [--settle MS] [DIR…]`
//! DIRs default to `<data>/models/decision` and `<data>/models/decision-browser`
//! (`$THEUTH_DATA_DIR`, else `~/.theuth`); a missing one is skipped. `--repeat` loads and drops
//! each checkpoint N times in a row (default 2) so a one-off runtime baseline (Metal's own
//! allocations on first use) separates from a per-load leak; `--settle` re-samples that many ms
//! after the drop (default 3000) — the Metal driver returns its pages asynchronously.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use candle_core::DType;
use decision::candle::Loaded;
use decision::{Device, ProcessMemory, Question, process_memory};

fn cell(m: ProcessMemory) -> String {
    format!("{:.0} / {:.0}", m.footprint as f64 / 1e6, m.resident as f64 / 1e6)
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let mut args = std::env::args().skip(1);
    let mut metal = Device::Auto.metal();
    let mut repeat = 2usize;
    let mut settle = 3000u64;
    let mut dirs: Vec<PathBuf> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--device" => metal = args.next().as_deref() == Some("metal"),
            "--repeat" => repeat = args.next().and_then(|n| n.parse().ok()).unwrap_or(2),
            "--settle" => settle = args.next().and_then(|n| n.parse().ok()).unwrap_or(3000),
            _ => dirs.push(PathBuf::from(a)),
        }
    }
    if dirs.is_empty() {
        let data = std::env::var_os("THEUTH_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".theuth"));
        dirs.push(data.join("models/decision"));
        dirs.push(data.join("models/decision-browser"));
    }
    let dtype = if metal { DType::F16 } else { DType::F32 };
    println!(
        "device {} {dtype:?} · MB, footprint / rss (proc_pid_rusage phys_footprint / resident_size)",
        if metal { "metal" } else { "cpu" }
    );
    println!(
        "| checkpoint | run | before | loaded (after 1st inference) | released | released +{settle} ms | settled − before (footprint) | load | infer |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for dir in dirs {
        if !dir.join("model.safetensors").is_file() {
            println!("| {} | – | skipped: no checkpoint | | | | | | |", dir.display());
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        for run in 1..=repeat {
            let before = process_memory();
            let t = Instant::now();
            let m = Loaded::open(&dir, metal, dtype).expect("open");
            let load = t.elapsed();
            let q = Question::Bool {
                statement: "The speakers are only greeting, scheduling or saying goodbye".into(),
                state: "them: hi all, can we push the standup to three? me: sure, works for me".into(),
            };
            let t = Instant::now();
            let seq = m.build(&q).unwrap();
            m.run(std::slice::from_ref(&seq)).expect("run");
            let infer = t.elapsed();
            let loaded = process_memory();
            drop(m);
            let released = process_memory();
            std::thread::sleep(Duration::from_millis(settle));
            let settled = process_memory();
            println!(
                "| {name} | {run} | {} | {} | {} | {} | {:+.0} | {:.0} ms | {:.0} ms |",
                cell(before),
                cell(loaded),
                cell(released),
                cell(settled),
                (settled.footprint as f64 - before.footprint as f64) / 1e6,
                load.as_secs_f64() * 1e3,
                infer.as_secs_f64() * 1e3,
            );
        }
    }
}
