//! 173 Part 1 repro: does a process tap keep delivering audio when the tapped
//! process re-creates its output stream or renders to another device? A child
//! (this binary, `child` arg) plays a 440 Hz tone at amplitude 1e-4 (−80 dBFS,
//! inaudible) through `voice::out::Player` (cpal); the parent taps the child's pid with flexaudio
//! `ProcessLoopback` exactly as `capture::system::spawn` does, prints every
//! event and, per second, chunks / exact-zero chunk fraction / peak dBFS / the
//! child's `IsRunningOutput`, while it scripts the child:
//! baseline → recreate (same device) → stop output → start → `device <name>` →
//! back to default. Never changes a device, rate or volume.
//! `cargo run -p meeting --example tap_repro -- [virtual output name, e.g. ZoomAudioDevice]`.
//!
//! `tap_repro watchdog <scenario>`: the 173 watchdog end to end — the real
//! `capture::system::spawn` loop on child A, printing every watchdog report:
//! - `moved` (≈ 200 s): a second child B plays the −80 dBFS tone throughout; at
//!   5 s A stops rendering (the incident: the tapped object idle, sound elsewhere).
//!   Expect probe audible → reopens → fallback → B heard; A plays again at 130 s →
//!   the next return probe goes back to A's tap. The probe floor is set to −90
//!   dBFS (`THEUTH_MEETING_PROBE_DBFS`) unless given: −80 dBFS is below the
//!   default −70 floor by design.
//! - `silent` (≈ 80 s): A keeps its output running but renders exact zeros and
//!   nothing else of ours plays; default floor. Expect probe silent → no episode,
//!   no warning (anything else playing on the Mac at ≥ −70 dBFS counts as audible).

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use flexaudio::{OutputFormat, SourceKind, StreamConfig};

/// −80 dBFS: below any speaker or headset noise floor.
const AMPLITUDE: f32 = 1e-4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("child") => child(),
        Some("watchdog") => watchdog(args.get(1).map_or("moved", String::as_str)),
        _ => parent(args.first().cloned()),
    }
}

/// Play the tone on the default output; stdin lines `recreate`, `stop`,
/// `start`, `device <name>` (`default` for the default output), `mute` /
/// `unmute` (exact 0.0 samples, the output keeps running), `quit`.
fn child() {
    let open = |name: &str| -> Option<voice::out::Player> {
        let p = voice::out::Player::open((name != "default").then_some(name), 100)
            .map_err(|e| eprintln!("child: {e:#}"))
            .ok()?;
        eprintln!(
            "child: playing on {:?} ({} Hz, {} ch)",
            p.device, p.rate, p.channels
        );
        Some(p)
    };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if tx.send(line.unwrap_or_default()).is_err() {
                return;
            }
        }
        // The parent is gone: stop playing.
        let _ = tx.send("quit".into());
    });
    let id = voice::SpeechId::new();
    let rate = voice::out::RATE as usize;
    let step = 440.0 * std::f32::consts::TAU / rate as f32;
    let (mut phase, mut muted) = (0f32, false);
    let mut device = "default".to_owned();
    let mut player = open(&device);
    loop {
        while let Ok(line) = rx.try_recv() {
            let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            match cmd {
                "recreate" => {
                    drop(player.take());
                    player = open(&device);
                }
                "stop" => drop(player.take()),
                "start" => player = open(&device),
                "device" => {
                    drop(player.take());
                    device = arg.to_owned();
                    player = open(&device);
                }
                "mute" => muted = true,
                "unmute" => muted = false,
                _ => return,
            }
        }
        let Some(p) = &player else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        // 20 ms per write; `write` blocks while the 100 ms ring is full, which paces the loop.
        // Exact 0.0 when muted (not `0.0 * sin`, which is −0.0 half the time).
        let pcm: Vec<f32> = (0..rate / 50)
            .map(|_| {
                let s = if muted { 0.0 } else { AMPLITUDE * phase.sin() };
                phase = (phase + step) % std::f32::consts::TAU;
                s
            })
            .collect();
        p.write(id, &pcm);
    }
}

/// A child player: its handle and stdin.
fn spawn_child() -> (std::process::Child, std::process::ChildStdin) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut child = Command::new(exe)
        .arg("child")
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn child");
    let stdin = child.stdin.take().expect("child stdin");
    (child, stdin)
}

/// See the module doc: the real system capture loop on child A in `scenario`.
fn watchdog(scenario: &str) {
    let moved = scenario == "moved";
    if moved && std::env::var_os("THEUTH_MEETING_PROBE_DBFS").is_none() {
        // SAFETY: single-threaded here; no thread reads the environment yet.
        unsafe { std::env::set_var("THEUTH_MEETING_PROBE_DBFS", "-90") };
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("meeting=info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let (a, mut to_a) = spawn_child();
    let b = moved.then(spawn_child);
    if !moved {
        let _ = writeln!(to_a, "mute");
    }
    std::thread::sleep(Duration::from_millis(1500));
    let pid = a.id();
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (feed, mut enc, _stt) = meeting::capture::rings(&paused);
    let (reports, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let thread = match meeting::capture::system::spawn(
        pid,
        None,
        feed,
        stop.clone(),
        reports,
        tracing::Span::none(),
    ) {
        Ok(t) => t,
        Err(e) => {
            println!("tap on child pid {pid}: {e:#}");
            for (mut c, _) in [Some((a, to_a)), b].into_iter().flatten() {
                let _ = c.kill();
                let _ = c.wait();
            }
            return;
        }
    };
    let (secs, script): (u64, &[(u64, &str)]) = if moved {
        (200, &[(5, "stop"), (130, "start")])
    } else {
        (80, &[])
    };
    println!(
        "watchdog `{scenario}` on child A pid {pid}{} for {secs}s",
        b.as_ref().map_or(String::new(), |(c, _)| format!(
            ", child B pid {} playing",
            c.id()
        ))
    );
    let t0 = Instant::now();
    let mut step = 0;
    while t0.elapsed() < Duration::from_secs(secs) {
        while enc.pop().is_ok() {}
        while let Ok(r) = rx.try_recv() {
            println!("{:6.2}s report {r:?}", t0.elapsed().as_secs_f32());
        }
        if let Some((at, cmd)) = script.get(step)
            && t0.elapsed() >= Duration::from_secs(*at)
        {
            step += 1;
            println!("{:6.2}s >>> child A: {cmd}", t0.elapsed().as_secs_f32());
            let _ = writeln!(to_a, "{cmd}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::SeqCst);
    let _ = thread.join();
    while let Ok(r) = rx.try_recv() {
        println!("{:6.2}s report {r:?}", t0.elapsed().as_secs_f32());
    }
    for (mut c, mut to) in [Some((a, to_a)), b].into_iter().flatten() {
        let _ = writeln!(to, "quit");
        let _ = c.wait();
    }
}

fn parent(virtual_out: Option<String>) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut child = Command::new(exe)
        .arg("child")
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn child");
    let pid = child.id();
    let mut to_child = child.stdin.take().expect("child stdin");
    std::thread::sleep(Duration::from_millis(1500));
    let cfg = StreamConfig {
        kind: SourceKind::ProcessLoopback,
        target_pid: Some(pid),
        output: OutputFormat {
            sample_rate: meeting::capture::RATE,
            channels: 1,
        },
        ..Default::default()
    };
    let mut stream = match flexaudio::open(cfg).and_then(|mut s| s.start().map(|()| s)) {
        Ok(s) => s,
        Err(e) => {
            println!("tap on child pid {pid}: open failed: {e}");
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
    };
    println!("tapping child pid {pid}");
    let mut script: Vec<(u64, String)> = vec![
        (5, "recreate".into()),
        (10, "stop".into()),
        (15, "start".into()),
    ];
    let mut end = 20;
    if let Some(name) = virtual_out {
        script.push((20, format!("device {name}")));
        script.push((27, "device default".into()));
        end = 33;
    }
    let t0 = Instant::now();
    let mut next = 1u64;
    let (mut chunks, mut zero_chunks, mut peak) = (0u32, 0u32, 0f32);
    loop {
        let secs = t0.elapsed().as_secs();
        while let Some(ev) = stream.poll_event() {
            println!("{:6.2}s event {ev:?}", t0.elapsed().as_secs_f32());
        }
        if let Some(chunk) = stream.poll_chunk() {
            chunks += 1;
            let p = chunk.data.iter().fold(0f32, |m, s| m.max(s.abs()));
            zero_chunks += u32::from(p == 0.0);
            peak = peak.max(p);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
        if secs >= next {
            let running = meeting::capture::system::processes()
                .ok()
                .and_then(|ps| ps.into_iter().find(|p| p.pid == pid))
                .map(|p| p.active);
            println!(
                "{secs:4}s chunks {chunks:3} exact-zero {:5.1}% peak {:7.1} dBFS running_output {running:?}",
                100.0 * f64::from(zero_chunks) / f64::from(chunks.max(1)),
                20.0 * peak.max(1e-10).log10(),
            );
            (chunks, zero_chunks, peak) = (0, 0, 0.0);
            next += 1;
            if let Some(i) = script.iter().position(|(at, _)| *at == secs) {
                let (_, cmd) = script.remove(i);
                println!("{secs:4}s >>> child: {cmd}");
                let _ = writeln!(to_child, "{cmd}");
            }
            if secs >= end {
                break;
            }
        }
    }
    let _ = writeln!(to_child, "quit");
    drop(stream);
    let _ = child.wait();
}
