//! The daemon speaker on this host (76 §10 step 1): 3 s of a 440 Hz tone
//! through `voice::out::Player`, `flush()` at 1.5 s, then device rate,
//! callback period, reported output latency, and `position()` against the
//! samples written at flush time. `cargo run -p voice --example audio_out -- [device name]`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use voice::SpeechId;
use voice::out::{Player, RATE};

/// One 80 ms Mimi frame, what the speak task writes per call.
const FRAME: usize = 1920;

fn main() -> anyhow::Result<()> {
    let device = std::env::args().nth(1);
    let t0 = Instant::now();
    let player = Arc::new(Player::open(device.as_deref(), 200)?);
    println!(
        "device: {:?} rate: {} channels: {} opened in {:?}",
        player.device,
        player.rate,
        player.channels,
        t0.elapsed()
    );
    let ring = player.ring();
    println!(
        "ring: {} device samples ({} ms)",
        ring.capacity(),
        ring.capacity() as u64 * 1000 / u64::from(player.rate)
    );

    let id = SpeechId::new();
    let tone: Vec<f32> = (0..RATE as usize * 3)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.2)
        .collect();
    let writer = std::thread::spawn({
        let player = player.clone();
        move || {
            let mut written = 0usize;
            for frame in tone.chunks(FRAME) {
                if !player.write(id, frame) {
                    break;
                }
                written += frame.len();
            }
            written
        }
    });
    std::thread::sleep(Duration::from_millis(1_500));
    let written_before = ring.written() * u64::from(RATE) / u64::from(player.rate);
    let pos_before = player.position();
    let cut = player.flush();
    let t_cut = Instant::now();
    let written = writer.join().expect("writer");
    let quiet_after = t_cut.elapsed();
    let frames = ring.callback_frames();
    println!(
        "callback: {frames} frames = {:.1} ms",
        f64::from(frames) * 1000.0 / f64::from(player.rate)
    );
    println!(
        "reported output latency: {}",
        ring.latency()
            .map_or("none (40 ms assumed)".to_owned(), |l| format!("{l:?}"))
    );
    println!(
        "at flush (t=1.5 s): written {written_before} samples ({} ms), position {} samples ({} ms), id {:?} seq {}",
        written_before * 1000 / u64::from(RATE),
        cut.samples,
        cut.samples * 1000 / u64::from(RATE),
        cut.id.map(|_| "tone"),
        cut.seq
    );
    println!(
        "position() before flush: {} samples; run-ahead (written - position): {} ms",
        pos_before.samples,
        (written_before.saturating_sub(cut.samples)) * 1000 / u64::from(RATE)
    );
    println!(
        "writer returned {quiet_after:?} after flush having written {written} of {} samples",
        RATE as usize * 3
    );
    std::thread::sleep(Duration::from_millis(300));
    let after = player.position();
    println!(
        "300 ms after flush: position {} samples, id {:?}, ledger {:?}",
        after.samples,
        after.id.map(|_| "tone"),
        ring.ledger()
            .iter()
            .map(|e| (e.first, e.last, e.dropped))
            .collect::<Vec<_>>()
    );
    drop(player);
    println!("closed at {:?}", t0.elapsed());
    Ok(())
}
