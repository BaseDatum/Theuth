//! Headless playback smoke for live diarization (07 §7 step 9): a recorded system track
//! is pushed through the production live path without the actor — 48 kHz 20 ms chunks
//! into a capture ring → `diar::run` (rubato → `Streamer` in `LIVE` → `Labeller` → the
//! stop flush) on one thread, the live STT `Utterer` on another for the system
//! utterances — then the hold-back is replayed against the `Labels` the task emitted,
//! and the live labels are reconciled with the offline pass (`diarize::run` + `align::
//! reconcile`) on the same audio. Prints speakers, hold-back latency p50/p95 (audio time
//! between an utterance's end and the step that labelled it), coverage, and agreement.
//!
//! ```text
//! cargo run --release -p meeting --example live_diar_smoke -- <system.opus> [--secs N] [--data-dir ~/.theuth]
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use meeting::capture::{RATE as CAPTURE_RATE, Sample};
use meeting::diar::{self, Flags, Labels};
use meeting::post::align::{Span, reconcile};
use meeting::post::diarize::{self, RATE};
use meeting::post::transcribe::pcm16k;
use meeting::{Track, Utterance};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut audio: Option<PathBuf> = None;
    let mut secs: Option<usize> = None;
    let mut data_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".theuth");
    while let Some(a) = args.next() {
        match a.as_str() {
            "--secs" => secs = args.next().and_then(|s| s.parse().ok()),
            "--data-dir" => data_dir = args.next().map(PathBuf::from).unwrap_or(data_dir),
            _ => audio = Some(PathBuf::from(a)),
        }
    }
    let audio =
        audio.ok_or_else(|| anyhow::anyhow!("usage: live_diar_smoke <system.opus> [--secs N]"))?;
    let models_dir = data_dir.join("models");
    let models = diarize::models(&models_dir, &data_dir)
        .ok_or_else(|| anyhow::anyhow!("diarization models missing"))?;

    // The track at both rates: 48 kHz for the ring (what the capture delivers), 16 kHz for STT/offline.
    let (mut pcm48, rate) = voice::decode(std::fs::read(&audio)?, "opus")?;
    if rate != CAPTURE_RATE {
        pcm48 = voice::resample(pcm48, rate, CAPTURE_RATE)?;
    }
    let mut pcm16 = pcm16k(&audio)?;
    if let Some(s) = secs {
        pcm48.truncate(s * CAPTURE_RATE as usize);
        pcm16.truncate(s * RATE as usize);
    }
    let audio_s = pcm16.len() as f64 / f64::from(RATE);
    println!("audio: {} · {audio_s:.1} s", audio.display());

    // 1. Live diariser task over a capture ring, producer paced by back-pressure (never drops).
    let t = Instant::now();
    let streamer = diar::open(&models)?;
    println!(
        "live session loaded in {:?} ({:?})",
        t.elapsed(),
        streamer.geometry()
    );
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (mut producer, ring) = rtrb::RingBuffer::<Sample>::new(250);
    let (tx, rx) = std::sync::mpsc::channel::<Labels>();
    let flags = Flags {
        stop: stop.clone(),
        paused,
    };
    let diar_thread = std::thread::spawn(move || {
        let t = Instant::now();
        let r = diar::run(ring, streamer, flags, |l| tx.send(l).is_ok());
        (r, t.elapsed())
    });
    let chunk = CAPTURE_RATE as usize / 50;
    let t = Instant::now();
    for c in pcm48.chunks(chunk) {
        // `Feed::push` drops on a full ring; wait for room instead so the replay is lossless.
        while producer.slots() == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        producer
            .push((Instant::now(), c.to_vec()))
            .expect("slot checked");
    }
    while producer.slots() < 250 {
        std::thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::SeqCst);
    let (final_view, diar_wall) = diar_thread.join().expect("diariser thread");
    let final_view = final_view?;
    let mut views: Vec<Labels> = rx.try_iter().collect();
    let push_wall = t.elapsed();
    println!(
        "diariser: {} steps · wall {:.1} s (RTF {:.3}, producer-paced) · final horizon {:.1} s of {:.1} s",
        views.len(),
        diar_wall.as_secs_f64(),
        diar_wall.as_secs_f64() / audio_s,
        final_view.scored as f64 / f64::from(RATE),
        final_view.position as f64 / f64::from(RATE)
    );
    let _ = push_wall;
    views.push(final_view.clone());

    // 2. Live STT over the same audio: the system utterances the actor would hold back.
    let stt_dir = models_dir.join(voice::STREAMING_MODEL_SUBDIR);
    let t = Instant::now();
    let mut live = voice::live::Utterer::open(&stt_dir, &data_dir)?;
    let mut utterances: Vec<Utterance> = Vec::new();
    let mut close = |texts: Vec<voice::live::Text>, position: usize| {
        for x in texts
            .into_iter()
            .filter(|x| x.is_final && x.text.chars().any(char::is_alphanumeric))
        {
            utterances.push(Utterance {
                track: Track::System,
                t0_ms: (x.start as u64 * 1000) / u64::from(RATE),
                t1_ms: (position as u64 * 1000) / u64::from(RATE),
                text: x.text,
                request: false,
                speaker: None,
            });
        }
    };
    for c in pcm16.chunks(RATE as usize / 50) {
        let texts = live.chunk(c)?;
        let position = live.position();
        close(texts, position);
    }
    let texts = live.close()?;
    let position = live.position();
    close(texts, position);
    println!(
        "live STT: {} system utterances · wall {:.1} s",
        utterances.len(),
        t.elapsed().as_secs_f64()
    );

    // 3. Hold-back replay: an utterance is labelled by the first view whose horizon passes
    // its end; at stop the actor labels the rest against the final view (07 §3).
    let ms = |s: usize| (s as u64 * 1000) / u64::from(RATE);
    let mut latencies: Vec<u64> = Vec::new();
    let mut at_stop = 0usize;
    for u in &mut utterances {
        let v = match views.iter().find(|v| ms(v.scored) >= u.t1_ms) {
            Some(v) => v,
            None => {
                at_stop += 1;
                &final_view
            }
        };
        // The view's `position` is the audio time the step ran at; the label is that far after `t1`.
        latencies.push(ms(v.position).saturating_sub(u.t1_ms));
        let spans = v.view(ms(v.position)).spans;
        u.speaker = diar::speaker_of(u.t0_ms, u.t1_ms, &spans);
    }
    latencies.sort_unstable();
    let pct = |p: f64| {
        latencies
            .get(((latencies.len().max(1) - 1) as f64 * p) as usize)
            .copied()
            .unwrap_or(0)
    };
    let labelled = utterances.iter().filter(|u| u.speaker.is_some()).count();
    let mut live_ids: Vec<&str> = utterances
        .iter()
        .filter_map(|u| u.speaker.as_deref())
        .collect();
    live_ids.sort_unstable();
    live_ids.dedup();
    println!(
        "hold-back: label latency after utterance end p50 {:.2} s · p95 {:.2} s · max {:.2} s · {} of {} utterances labelled ({:.1} %), {} labelled by the stop flush",
        pct(0.5) as f64 / 1000.0,
        pct(0.95) as f64 / 1000.0,
        latencies.last().copied().unwrap_or(0) as f64 / 1000.0,
        labelled,
        utterances.len(),
        labelled as f64 * 100.0 / utterances.len().max(1) as f64,
        at_stop
    );
    println!("live speakers: {} {live_ids:?}", live_ids.len());

    // 4. Offline pass on the same audio, reconciled to the live labels.
    let t = Instant::now();
    let probs = diarize::probabilities(&pcm16, &models.diarization, &models.data_dir, |_, _| {})?;
    let mut spans: Vec<Span> = diarize::turns(&probs);
    let mut offline_ids: Vec<&str> = spans.iter().map(|s| s.speaker.as_str()).collect();
    offline_ids.sort_unstable();
    offline_ids.dedup();
    let offline_count = offline_ids.len();
    let renamed = reconcile(&utterances, &spans);
    let mut map: Vec<(&String, &String)> = renamed.iter().collect();
    map.sort();
    println!(
        "offline: {} speakers · {:.1} s · reconcile offline→live: {}",
        offline_count,
        t.elapsed().as_secs_f64(),
        map.iter()
            .map(|(a, b)| format!("{a}→{b}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    for s in &mut spans {
        if let Some(id) = renamed.get(&s.speaker) {
            s.speaker.clone_from(id);
        }
    }
    let (mut same, mut both, mut offline_none) = (0usize, 0usize, 0usize);
    for u in &utterances {
        let offline = diar::speaker_of(u.t0_ms, u.t1_ms, &spans);
        match (&u.speaker, &offline) {
            (Some(a), Some(b)) => {
                both += 1;
                same += usize::from(a == b);
            }
            (_, None) => offline_none += 1,
            _ => {}
        }
    }
    println!(
        "agreement live vs offline after reconciliation: {same} of {both} utterances labelled by both ({:.1} %) · offline has no speaker for {offline_none}",
        same as f64 * 100.0 / both.max(1) as f64
    );
    // A few labelled lines, as the batch would read them.
    for u in utterances.iter().filter(|u| u.speaker.is_some()).take(6) {
        let text: String = u.text.chars().take(70).collect();
        println!("  [{:>7.1}s] {}: {text}", u.t0_ms as f64 / 1000.0, u.who());
    }
    Ok(())
}
