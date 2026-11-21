//! Real-model check, `#[ignore]`d: needs the mlx-community/parakeet-tdt-0.6b-v3 files and a clip.
//!
//! PARAKEET_MLX_DIR=/tmp/parakeet-tdt-mlx PARAKEET_MLX_CLIP=clip.pcm16 \
//!   cargo test -p voice-mlx --test real_clip -- --ignored --nocapture
//!
//! PARAKEET_MLX_CLIP: raw 16 kHz mono s16le (the voice history `.pcm16` format); several paths may be joined
//! with `:`. PARAKEET_MLX_EXPECT (optional): the expected transcript of a single clip (e.g. parakeet-mlx's).
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::Path;
use std::time::Instant;

#[test]
#[ignore = "needs PARAKEET_MLX_DIR and PARAKEET_MLX_CLIP"]
fn transcribes_real_clip() {
    let dir = std::env::var("PARAKEET_MLX_DIR").expect("PARAKEET_MLX_DIR");
    let clips = std::env::var("PARAKEET_MLX_CLIP").expect("PARAKEET_MLX_CLIP");

    let t0 = Instant::now();
    let mut model = voice_mlx::ParakeetMlx::load(Path::new(&dir)).unwrap();
    println!("load: {} ms", t0.elapsed().as_millis());
    let t0 = Instant::now();
    model.warm().unwrap();
    println!("warm: {} ms", t0.elapsed().as_millis());

    for clip in clips.split(':') {
        let bytes = std::fs::read(clip).unwrap();
        let pcm: Vec<f32> =
            bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let t0 = Instant::now();
        let t = model.transcribe(&pcm).unwrap();
        println!(
            "clip {:.1} s: {} ms, {} words\n  {}",
            pcm.len() as f32 / 16_000.0,
            t0.elapsed().as_millis(),
            t.words.len(),
            t.text
        );
        if let Some(w) = t.words.first() {
            println!("  first word {:?} {:.2}–{:.2} s", w.text, w.start_s, w.end_s);
        }
        assert!(!t.text.is_empty());
        if let Ok(expect) = std::env::var("PARAKEET_MLX_EXPECT") {
            assert_eq!(t.text, expect);
        }
    }
}
