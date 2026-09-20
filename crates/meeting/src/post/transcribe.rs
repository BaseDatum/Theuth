//! One Opus track → words with times on the recording clock (04 §2 stage 1):
//! `voice::decode` (48 kHz) → `voice::resample` (16 kHz) → the live path's VAD
//! finds the utterances ([`speech`], ≤ 15 s each by the gate's cap) →
//! `Engine::transcribe(words)` per utterance with [`PAD_MS`] of context, word
//! times shifted by the offset and clamped to the utterance. Silence is never
//! handed to the model: a whole track with silences made the TDT drop an
//! utterance and stretch word ends across the gaps (04 §10 deviation 13).

use std::path::Path;

use voice::Engine;
use voice::vad::{Edge, FRAME, Gate, Vad};

use crate::transcript::Word;

/// The model's rate.
pub const RATE: u32 = 16_000;
/// Samples of the previous piece repeated at the start of the next (1 s).
pub const OVERLAP: usize = RATE as usize;

/// Decode a track file to 16 kHz mono.
pub fn pcm16k(path: &Path) -> anyhow::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("opus");
    // ponytail: whole-file decode; a 3 h track is ~2 s. Page seeking if that registers.
    let (pcm, rate) = voice::decode(bytes, ext)?;
    voice::resample(pcm, rate, RATE)
}

/// Utterance spans `(t0_ms, t1_ms)` of `pcm` (16 kHz) per the live gate
/// (`voice::live`): TEN-VAD frames → [`Gate`] edges, onset backdated by the
/// gate's onset run and pre-roll. Needs `ort` loaded ([`Engine::load`] first).
pub fn speech(pcm: &[f32]) -> anyhow::Result<Vec<(u64, u64)>> {
    let ms = |samples: usize| samples as u64 * 1000 / u64::from(RATE);
    let mut vad = Vad::new()?;
    let mut gate = Gate::new();
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    // Frame by frame so a `Turn` resets the VAD's hidden state exactly where the live path does.
    for (i, frame) in pcm.chunks_exact(FRAME).enumerate() {
        let end = (i + 1) * FRAME;
        let Some(&p) = vad.process(frame).first() else { continue };
        match gate.push(p) {
            Some(Edge::Onset) => {
                open = Some(end.saturating_sub((Gate::ONSET + Gate::PREROLL) * FRAME));
            }
            Some(Edge::Close) => {
                if let Some(s) = open.take() {
                    out.push((ms(s), ms(end)));
                }
            }
            Some(Edge::Turn) => vad.reset(),
            None => {}
        }
    }
    if let Some(s) = open {
        out.push((ms(s), ms(pcm.len())));
    }
    Ok(out)
}

/// Context kept on each side of an utterance.
const PAD_MS: u64 = 200;

/// Make sorted spans disjoint (onset pre-rolls overlap the previous close).
pub fn disjoint(spans: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &(a, b) in spans {
        let a = out.last().map_or(a, |c| a.max(c.1));
        if a < b {
            out.push((a, b));
        }
    }
    out
}

/// Transcribe the speech in `pcm` (16 kHz); `progress(done, total)` after each utterance.
pub fn words(
    engine: &Engine,
    pcm: &[f32],
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<Vec<Word>> {
    let spans = disjoint(&speech(pcm)?);
    let samples = |ms: u64| (ms * u64::from(RATE) / 1000) as usize;
    let mut out: Vec<Word> = Vec::new();
    for (i, &(a, b)) in spans.iter().enumerate() {
        let start = samples(a.saturating_sub(PAD_MS)).min(pcm.len());
        let end = samples(b + PAD_MS).min(pcm.len());
        let offset = start as u64 * 1000 / u64::from(RATE);
        let piece = engine.transcribe(&pcm[start..end], true)?;
        for w in piece.words {
            let t0 = (w.t0_ms + offset).clamp(a, b);
            out.push(Word {
                text: w.text,
                t0_ms: t0,
                t1_ms: (w.t1_ms + offset).clamp(t0, b),
            });
        }
        progress(i + 1, spans.len());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_trims_overlapping_prerolls_and_drops_swallowed_spans() {
        let spans = [(0, 5_000), (4_800, 12_000), (11_000, 11_500), (20_000, 21_000)];
        assert_eq!(disjoint(&spans), [(0, 5_000), (5_000, 12_000), (20_000, 21_000)]);
        assert!(disjoint(&[]).is_empty());
    }
}
