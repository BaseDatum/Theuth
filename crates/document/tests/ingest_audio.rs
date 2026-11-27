//! The audio route's pieces on the 3 s wav fixture (144 §4.1 row 9), no network: decode → 16 kHz
//! → duration, and — when `THEUTH_STT_MODEL_DIR` names a Parakeet TDT checkpoint (its parent's
//! parent is the data dir holding `models/onnxruntime`) — word-timed TDT through `voice::Engine`,
//! rendered as `## [mm:ss]` blocks. Without the model the inference half is skipped loudly.

#![cfg(not(target_os = "ios"))]

use std::path::Path;
use std::time::Instant;

const WAV: &[u8] = include_bytes!("fixtures/ingest/tone-3s.wav");

#[test]
fn the_three_second_wav_decodes_to_three_seconds_and_transcribes_with_word_times() {
    let (pcm, rate) = voice::decode(WAV.to_vec(), "wav").unwrap();
    assert_eq!(rate, 16_000);
    let pcm = voice::resample(pcm, rate, 16_000).unwrap();
    assert_eq!(pcm.len() * 1000 / 16_000, 3_000);

    let Some(dir) = std::env::var_os("THEUTH_STT_MODEL_DIR") else {
        eprintln!("SKIPPED inference: set THEUTH_STT_MODEL_DIR=<data_dir>/models/parakeet-tdt");
        return;
    };
    let dir = Path::new(&dir);
    let data_dir = dir.parent().and_then(Path::parent).unwrap();
    let t = Instant::now();
    let engine = match voice::Engine::load(dir, data_dir) {
        Ok(e) => e,
        Err(e) if format!("{e:#}").contains("libonnxruntime") => {
            eprintln!("SKIPPED inference: {e:#}");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    let loaded = t.elapsed();
    let t = Instant::now();
    let piece = engine.transcribe(&pcm, true).unwrap();
    let inferred = t.elapsed();
    let md = document::ingest::timed_blocks(
        piece.words.iter().map(|w| (w.t0_ms, w.text.clone())),
        60_000,
    );
    eprintln!(
        "TDT load {loaded:?}, 3 s wav with word times {inferred:?}: {} words → {md:?}",
        piece.words.len()
    );
    assert!(
        piece.words.iter().all(|w| w.t1_ms <= 3_500),
        "{:?}",
        piece.words
    );
}
