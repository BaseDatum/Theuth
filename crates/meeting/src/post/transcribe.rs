//! One Opus track → words with times on the recording clock (04 §2 stage 1):
//! `voice::decode` (48 kHz) → `voice::resample` (16 kHz) → [`voice::PIECE`]-long
//! pieces with [`OVERLAP`] of context on each side → `Engine::transcribe(words)`,
//! word times shifted by the piece offset, words in the overlap deduplicated by time.
//! The mic track is additionally masked by the live path's VAD ([`speech`]): the
//! TDT hears the far-end residual the AEC leaves and transcribes it as `me`,
//! which bridged the user's own utterances into one row (04 §10 deviation 13).

use std::path::Path;

use voice::vad::{Edge, FRAME, Gate, Vad};
use voice::{Engine, PIECE};

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

/// Drop words whose midpoint lies outside every span of `spans` (sorted, disjoint).
pub fn mask(words: &mut Vec<Word>, spans: &[(u64, u64)]) {
    words.retain(|w| {
        let mid = w.t0_ms.midpoint(w.t1_ms);
        spans.iter().any(|&(a, b)| a <= mid && mid < b)
    });
}

/// Piece starts: `0`, then `PIECE - OVERLAP` further while the previous piece
/// does not already reach the end.
fn starts(len: usize) -> impl Iterator<Item = usize> {
    let step = PIECE - OVERLAP;
    std::iter::successors(Some(0), move |&s| (s + PIECE < len).then_some(s + step))
}

/// Transcribe `pcm` (16 kHz) in pieces; `progress(done, total)` after each.
pub fn words(
    engine: &Engine,
    pcm: &[f32],
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<Vec<Word>> {
    if pcm.is_empty() {
        return Ok(Vec::new());
    }
    let total = starts(pcm.len()).count();
    let mut out: Vec<Word> = Vec::new();
    for (i, start) in starts(pcm.len()).enumerate() {
        let end = (start + PIECE).min(pcm.len());
        let offset = start as u64 * 1000 / u64::from(RATE);
        let piece = engine.transcribe(&pcm[start..end], true)?;
        // A word starting before the previous piece's last word ended is the
        // overlap heard twice: the earlier piece wins, so times stay monotonic.
        let keep_from = out.last().map_or(0, |w| w.t1_ms);
        for w in piece.words {
            let t0 = w.t0_ms + offset;
            if start > 0 && t0 < keep_from {
                continue;
            }
            out.push(Word {
                text: w.text,
                t0_ms: t0,
                t1_ms: (w.t1_ms + offset).max(t0),
            });
        }
        progress(i + 1, total);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_starts_overlap_by_one_second_and_cover_the_tail() {
        assert_eq!(starts(0).collect::<Vec<_>>(), [0]);
        assert_eq!(starts(PIECE).collect::<Vec<_>>(), [0]);
        let step = PIECE - OVERLAP;
        assert_eq!(starts(PIECE + 1).collect::<Vec<_>>(), [0, step]);
        // The piece at 2·step reaches 2·step + PIECE = 3·step + OVERLAP: the tail is covered.
        assert_eq!(starts(3 * step + 5).collect::<Vec<_>>(), [0, step, 2 * step]);
        assert_eq!(starts(3 * step + OVERLAP + 1).collect::<Vec<_>>(), [0, step, 2 * step, 3 * step]);
    }
}
