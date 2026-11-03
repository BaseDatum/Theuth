//! 173 Part 1 repro: does a process tap keep delivering audio when the tapped
//! process re-creates its output stream or renders to another device? A child
//! (this binary, `child` arg) plays a 440 Hz tone at amplitude 1e-4 (−80 dBFS,
//! inaudible) through cpal; the parent taps the child's pid with flexaudio
//! `ProcessLoopback` exactly as `capture::system::spawn` does, prints every
//! event and, per second, chunks / exact-zero chunk fraction / peak dBFS / the
//! child's `IsRunningOutput`, while it scripts the child:
//! baseline → recreate (same device) → stop output → start → `device <name>` →
//! back to default. Never changes a device, rate or volume.
//! `cargo run -p meeting --example tap_repro -- [virtual output name, e.g. ZoomAudioDevice]`.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use flexaudio::{OutputFormat, SourceKind, StreamConfig};

/// −80 dBFS: below any speaker or headset noise floor.
const AMPLITUDE: f32 = 1e-4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("child") {
        return child();
    }
    parent(args.first().cloned());
}

/// Play the tone on the default output; stdin lines `recreate`, `stop`,
/// `start`, `device <name>` (`default` for the default output), `quit`.
fn child() {
    let host = cpal::default_host();
    let open = |name: &str| -> Result<cpal::Stream, String> {
        let dev = if name == "default" {
            host.default_output_device()
        } else {
            host.output_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.name().is_ok_and(|n| n == name))
        }
        .ok_or_else(|| format!("no output device {name:?}"))?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        if cfg.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("{name}: {:?}, want f32", cfg.sample_format()));
        }
        let ch = usize::from(cfg.channels());
        let step = 440.0 * std::f32::consts::TAU / cfg.sample_rate().0 as f32;
        let mut phase = 0f32;
        let stream = dev
            .build_output_stream(
                &cfg.config(),
                move |data: &mut [f32], _| {
                    for frame in data.chunks_mut(ch) {
                        frame.fill(AMPLITUDE * phase.sin());
                        phase = (phase + step) % std::f32::consts::TAU;
                    }
                },
                |e| eprintln!("child: stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        eprintln!(
            "child: playing on {:?} ({} Hz, {ch} ch)",
            dev.name().unwrap_or_default(),
            cfg.sample_rate().0
        );
        Ok(stream)
    };
    let mut device = "default".to_owned();
    let mut stream = Some(open(&device).expect("child: open default output"));
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap_or_default();
        let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        match cmd {
            "recreate" => {
                drop(stream.take());
                stream = open(&device).map_err(|e| eprintln!("child: {e}")).ok();
            }
            "stop" => drop(stream.take()),
            "start" => stream = open(&device).map_err(|e| eprintln!("child: {e}")).ok(),
            "device" => {
                drop(stream.take());
                device = arg.to_owned();
                stream = open(&device).map_err(|e| eprintln!("child: {e}")).ok();
            }
            _ => return,
        }
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
