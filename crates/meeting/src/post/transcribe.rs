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

/// libopus decodes every packet at this rate (RFC 7845 §5.1), whatever the input rate was.
pub const OPUS_RATE: u32 = 48_000;

/// 156 D10: the samples of `ranges` (ms, in time order) from the Ogg Opus track at `path`,
/// concatenated, at [`OPUS_RATE`]. The same samples `voice::decode` + slicing gives — the
/// stream is read and decoded from its start so libopus sees the same packet history — but
/// decoding stops at the last range's end and nothing outside the ranges is kept, so memory is
/// bounded by the ranges, not the track.
// ponytail: decode from packet 0 (~1 s per hour of track); page seeking via granule positions if a click ever feels slow.
pub fn opus_ranges(path: &Path, ranges: &[(u64, u64)]) -> anyhow::Result<Vec<f32>> {
    let at = |ms: u64| (ms * u64::from(OPUS_RATE) / 1000) as usize;
    let want: Vec<(usize, usize)> = ranges.iter().map(|&(a, b)| (at(a), at(b))).collect();
    let end = want.iter().map(|r| r.1).max().unwrap_or(0);
    let mut out = Vec::with_capacity(want.iter().map(|(a, b)| b.saturating_sub(*a)).sum());
    let mut reader = ::ogg::PacketReader::new(std::io::BufReader::new(std::fs::File::open(path)?));
    let mut dec = opus::Decoder::new(OPUS_RATE, opus::Channels::Mono)?;
    let mut buf = vec![0f32; 5760];
    // `OpusHead` and `OpusTags` first (RFC 7845 §3); the demuxer `voice::decode` uses skips them too.
    let mut headers = 2;
    let mut pos = 0usize;
    while pos < end {
        // A truncated or damaged tail ends the stream, as `voice::decode`'s `while let Ok` does.
        let Ok(Some(p)) = reader.read_packet() else {
            break;
        };
        if headers > 0 {
            headers -= 1;
            continue;
        }
        let n = dec.decode_float(&p.data, &mut buf, false)?;
        let next = pos + n;
        for &(a, b) in &want {
            let (lo, hi) = (a.max(pos), b.min(next));
            if lo < hi {
                out.extend_from_slice(&buf[lo - pos..hi - pos]);
            }
        }
        pos = next;
    }
    Ok(out)
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
    for (i, frame) in pcm.as_chunks::<FRAME>().0.iter().enumerate() {
        let end = (i + 1) * FRAME;
        let Some(&p) = vad.process(frame).first() else {
            continue;
        };
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
        let spans = [
            (0, 5_000),
            (4_800, 12_000),
            (11_000, 11_500),
            (20_000, 21_000),
        ];
        assert_eq!(
            disjoint(&spans),
            [(0, 5_000), (5_000, 12_000), (20_000, 21_000)]
        );
        assert!(disjoint(&[]).is_empty());
    }

    /// 156 D10: a 3 s synthetic track through the crate's own encoder and writer; the bounded
    /// decode of a range is exactly the full decode sliced, and the whole-track range is the
    /// full decode.
    #[test]
    fn opus_ranges_match_the_full_decode_sliced() {
        use crate::ogg::{FRAME, RATE as ENC_RATE, Writer};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mic.opus");
        let mut enc =
            opus::Encoder::new(ENC_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut w = Writer::new(std::fs::File::create(&path).unwrap(), 7, 0, &[]).unwrap();
        let frames = 150; // 3 s of 20 ms frames
        let mut buf = vec![0u8; 4000];
        for f in 0..frames {
            let pcm: Vec<f32> = (0..FRAME)
                .map(|i| {
                    let t = (f * FRAME + i) as f32 / ENC_RATE as f32;
                    (t * 440.0 * std::f32::consts::TAU).sin() * 0.5
                })
                .collect();
            let n = enc.encode_float(&pcm, &mut buf).unwrap();
            w.packet(buf[..n].to_vec(), f + 1 == frames).unwrap();
        }
        drop(w);
        let (full, rate) = voice::decode(std::fs::read(&path).unwrap(), "opus").unwrap();
        assert_eq!(rate, OPUS_RATE);
        assert_eq!(full.len(), frames * FRAME * 2, "3 s at 48 kHz");
        let at = |ms: u64| (ms * u64::from(OPUS_RATE) / 1000) as usize;
        let cut = opus_ranges(&path, &[(1_234, 2_345)]).unwrap();
        assert_eq!(cut, full[at(1_234)..at(2_345)]);
        // Two ranges concatenate in order; the end past the track clamps; a range past the end is empty.
        let two = opus_ranges(&path, &[(100, 200), (2_900, 9_000)]).unwrap();
        assert_eq!(two, [&full[at(100)..at(200)], &full[at(2_900)..]].concat());
        assert!(opus_ranges(&path, &[(5_000, 6_000)]).unwrap().is_empty());
        assert_eq!(opus_ranges(&path, &[(0, 60_000)]).unwrap(), full);
    }
}
