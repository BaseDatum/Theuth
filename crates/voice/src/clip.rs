//! Outbound clip container (45 §0.4): MP3, mono, 48 kbit/s via LAME — the one
//! format Signal (Android, iOS, Desktop) and Slack's inline player all play.

use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality, max_required_buffer_size};

use crate::Clip;
use crate::speak::{SAMPLE_RATE, Synthesizer};

/// Synthesize `spoken` (already sentences) to one [`Clip`]. Blocking; no
/// cancellation — a block is short and nobody is listening yet.
pub(crate) fn render(synth: &dyn Synthesizer, voice: &str, spoken: &str) -> anyhow::Result<Clip> {
    let stream = synth.stream(spoken, voice)?;
    let mut pcm: Vec<f32> = Vec::new();
    for chunk in stream {
        pcm.extend_from_slice(&chunk?);
    }
    Ok(Clip {
        bytes: mp3(&pcm, SAMPLE_RATE)?,
        mime: "audio/mpeg",
        duration_ms: pcm.len() as u64 * 1000 / SAMPLE_RATE as u64,
    })
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
}
