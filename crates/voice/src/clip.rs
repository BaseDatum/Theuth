//! Outbound clip container (45 §0.4): MP3, mono, 48 kbit/s via LAME — the one
//! format Signal (Android, iOS, Desktop) and Slack's inline player all play.
//! 144 §10: a clip may be a dialogue — one synthesis per speaker turn, each in
//! its own voice, the PCM joined with [`GAP_MS`] of silence, encoded once.

use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality, max_required_buffer_size};

use crate::Clip;
use crate::speak::{SAMPLE_RATE, Synthesizer};

/// Silence between two speaker turns of a dialogue.
pub const GAP_MS: u32 = 350;

/// Synthesize `turns` (`(voice, spoken)`, already sentences) to one [`Clip`]:
/// every turn in its voice, [`GAP_MS`] of silence between turns, one MP3.
/// Blocking; no cancellation — a block is short and nobody is listening yet.
pub(crate) fn render(synth: &dyn Synthesizer, turns: &[(String, String)]) -> anyhow::Result<Clip> {
    let pcm = dialogue(synth, turns)?;
    Ok(Clip {
        bytes: mp3(&pcm, SAMPLE_RATE)?,
        mime: "audio/mpeg",
        duration_ms: pcm.len() as u64 * 1000 / SAMPLE_RATE as u64,
    })
}

/// The joined PCM of `turns`: each turn's samples in order, [`GAP_MS`] of
/// zeros between consecutive turns (none before the first or after the last).
pub(crate) fn dialogue(
    synth: &dyn Synthesizer,
    turns: &[(String, String)],
) -> anyhow::Result<Vec<f32>> {
    let gap = (SAMPLE_RATE * GAP_MS / 1000) as usize;
    let mut pcm: Vec<f32> = Vec::new();
    for (i, (voice, spoken)) in turns.iter().enumerate() {
        if i > 0 {
            pcm.resize(pcm.len() + gap, 0.0);
        }
        for chunk in synth.stream(spoken, voice)? {
            pcm.extend_from_slice(&chunk?);
        }
    }
    Ok(pcm)
}

/// Encode mono f32 PCM at `rate` Hz.
pub(crate) fn mp3(pcm: &[f32], rate: u32) -> anyhow::Result<Vec<u8>> {
    let mut b = Builder::new().ok_or_else(|| anyhow::anyhow!("lame init"))?;
    b.set_num_channels(1)
        .map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    b.set_sample_rate(rate)
        .map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    b.set_brate(Bitrate::Kbps48)
        .map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    b.set_quality(Quality::Good)
        .map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    let mut enc = b.build().map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    let mut out = Vec::with_capacity(max_required_buffer_size(pcm.len()));
    let n = enc
        .encode(MonoPcm(pcm), out.spare_capacity_mut())
        .map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    // SAFETY: `encode` initialised the first `n` bytes of the spare capacity.
    unsafe { out.set_len(n) };
    let n2 = enc
        .flush::<FlushNoGap>(out.spare_capacity_mut())
        .map_err(|e| anyhow::anyhow!("lame: {e:?}"))?;
    // SAFETY: `flush` initialised the next `n2` bytes.
    unsafe { out.set_len(n + n2) };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;
    use crate::speak::Pcm;

    #[test]
    fn encodes_a_second_of_tone() {
        let pcm: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 24_000.0).sin() * 0.5)
            .collect();
        let bytes = super::mp3(&pcm, 24_000).unwrap();
        // 48 kbit/s ≈ 6 KB/s; allow for headers and the encoder's discretion.
        assert!((4_000..9_000).contains(&bytes.len()), "{}", bytes.len());
        assert_eq!(bytes[0], 0xFF, "no MP3 sync word");
    }

    /// One frame of `1.0` per call; records the voice of every call.
    struct Stub(Mutex<Vec<String>>);

    impl Synthesizer for Stub {
        fn stream(&self, _spoken: &str, voice: &str) -> anyhow::Result<Pcm> {
            self.0.lock().push(voice.to_owned());
            Ok(Box::new(std::iter::once(Ok(vec![1.0; 1920]))))
        }
    }

    #[test]
    fn dialogue_switches_voice_per_turn_and_joins_with_350_ms_silences() {
        let stub = Stub(Mutex::new(Vec::new()));
        let turns = [
            ("alba".to_owned(), "Hello.".to_owned()),
            ("peter_yearsley".to_owned(), "Hi.".to_owned()),
            ("alba".to_owned(), "Bye.".to_owned()),
        ];
        let pcm = dialogue(&stub, &turns).unwrap();
        let gap = 24_000 * 350 / 1000;
        assert_eq!(pcm.len(), 3 * 1920 + 2 * gap);
        // Speech, silence, speech, silence, speech — in that order.
        assert!(pcm[..1920].iter().all(|s| *s == 1.0));
        assert!(pcm[1920..1920 + gap].iter().all(|s| *s == 0.0));
        assert!(pcm[1920 + gap..2 * 1920 + gap].iter().all(|s| *s == 1.0));
        assert!(pcm[pcm.len() - 1920..].iter().all(|s| *s == 1.0));
        assert_eq!(
            *stub.0.lock(),
            ["alba", "peter_yearsley", "alba"],
            "one synthesis per turn, in the turn's voice"
        );
        // One turn: no silence at all.
        assert_eq!(dialogue(&stub, &turns[..1]).unwrap().len(), 1920);
    }
}
