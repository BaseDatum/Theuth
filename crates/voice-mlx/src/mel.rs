//! Log-mel front end, ported from parakeet-mlx 0.7.0 `audio.get_logmel` with the v3 preprocessor config
//! (16 kHz, 128 mels, n_fft 512, win 400 = 0.025 s, hop 160 = 0.01 s, periodic Hann, per-feature normalisation,
//! preemphasis 0.97, magnitude power 2, no dither at inference).
//!
//! Quirks kept on purpose so features match the reference bit-for-bit-ish:
//! - the 400-sample window is zero-padded at the END to 512 (not centred);
//! - the frame count is `(len + 2·256 − 400 + 160) / 160` (win_length, not n_fft), so the last frames read past
//!   the padded signal; those samples meet the zero tail of the window, here they are zeros;
//! - the "magnitude" is `|re| + |im|` (parakeet-mlx views the complex spectrum as interleaved floats), squared.

use anyhow::{Result, ensure};
use mlx_rs::{Array, fft, ops};

pub(crate) const SAMPLE_RATE: usize = 16_000;
pub(crate) const N_MELS: usize = 128;
pub(crate) const N_FFT: usize = 512;
pub(crate) const WIN: usize = 400;
pub(crate) const HOP: usize = 160;
const PREEMPH: f32 = 0.97;
const N_BINS: usize = N_FFT / 2 + 1;

/// librosa `filters.mel(sr, n_fft, n_mels, fmin=0, fmax=sr/2, norm="slaney", htk=False)`, row-major
/// `[N_MELS, N_BINS]`, float32 like librosa's default dtype.
pub(crate) fn filterbank() -> Vec<f32> {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4f64.ln() / 27.0;
    let hz_to_mel = |f: f64| {
        if f >= MIN_LOG_HZ {
            min_log_mel + (f / MIN_LOG_HZ).ln() / logstep
        } else {
            f / F_SP
        }
    };
    let mel_to_hz = |m: f64| {
        if m >= min_log_mel {
            MIN_LOG_HZ * (logstep * (m - min_log_mel)).exp()
        } else {
            F_SP * m
        }
    };

    let sr = SAMPLE_RATE as f64;
    let fftfreqs: Vec<f64> = (0..N_BINS)
        .map(|i| i as f64 * (sr / 2.0) / (N_BINS - 1) as f64)
        .collect();
    let (mel_lo, mel_hi) = (hz_to_mel(0.0), hz_to_mel(sr / 2.0));
    let mel_f: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(mel_lo + (mel_hi - mel_lo) * i as f64 / (N_MELS + 1) as f64))
        .collect();
    let mut w = vec![0f32; N_MELS * N_BINS];
    for m in 0..N_MELS {
        let enorm = 2.0 / (mel_f[m + 2] - mel_f[m]);
        for (k, &f) in fftfreqs.iter().enumerate() {
            let lower = -(mel_f[m] - f) / (mel_f[m + 1] - mel_f[m]);
            let upper = (mel_f[m + 2] - f) / (mel_f[m + 2] - mel_f[m + 1]);
            let v = lower.min(upper).max(0.0) as f32;
            w[m * N_BINS + k] = (v as f64 * enorm) as f32;
        }
    }
    w
}

/// Windowed frames `[t, N_FFT]` (preemphasis, reflect padding by N_FFT/2, periodic Hann zero-padded at the end).
fn frames(pcm: &[f32]) -> (Vec<f32>, usize) {
    let n = pcm.len();
    let mut x = Vec::with_capacity(n);
    x.push(pcm[0]);
    x.extend(pcm.windows(2).map(|w| w[1] - PREEMPH * w[0]));

    let pad = N_FFT / 2;
    let mut padded = Vec::with_capacity(n + 2 * pad);
    padded.extend((1..=pad).rev().map(|i| x[i.min(n - 1)]));
    padded.extend_from_slice(&x);
    padded.extend((0..pad).map(|i| x[(n as isize - 2 - i as isize).max(0) as usize]));

    let window: Vec<f32> = (0..N_FFT)
        .map(|i| {
            if i < WIN {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / WIN as f64).cos()) as f32
            } else {
                0.0
            }
        })
        .collect();
    let t = (padded.len() - WIN + HOP) / HOP;
    let mut out = vec![0f32; t * N_FFT];
    for f in 0..t {
        let base = f * HOP;
        for k in 0..N_FFT {
            let s = padded.get(base + k).copied().unwrap_or(0.0);
            out[f * N_FFT + k] = s * window[k];
        }
    }
    (out, t)
}

/// Normalised log-mel features `[1, t, N_MELS]`, float32.
pub(crate) fn log_mel(pcm: &[f32], fb: &Array) -> Result<Array> {
    // parakeet-mlx needs ≥ HOP samples, but its reflect padding silently shortens below N_FFT/2 + 1; refuse that.
    ensure!(
        pcm.len() > N_FFT / 2 + 1,
        "audio must hold more than {} samples, got {}",
        N_FFT / 2 + 1,
        pcm.len()
    );
    let (fr, t) = frames(pcm);
    let spec = fft::rfft(
        Array::from_slice(&fr, &[t as i32, N_FFT as i32]),
        N_FFT as i32,
        -1,
    )?;
    let mag = ops::abs(spec.real()?)?.add(ops::abs(spec.imag()?)?)?;
    let power = mag.power(Array::from_f32(2.0))?;
    let mel = ops::matmul(fb, power.t())?; // [N_MELS, t]
    let mel = mel.add(Array::from_f32(1e-5))?.log()?;
    let mean = mel.mean_axis(1, true)?;
    let std = mel.var_axis(1, true, 0)?.sqrt()?;
    let norm = mel
        .subtract(&mean)?
        .divide(std.add(Array::from_f32(1e-5))?)?;
    Ok(ops::expand_dims(norm.t(), 0)?)
}

pub(crate) fn filterbank_array() -> Array {
    Array::from_slice(&filterbank(), &[N_MELS as i32, N_BINS as i32])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oracle: parakeet-mlx 0.7.0 `get_logmel` (float32) on
    /// `0.5·sin(2π·440·n/16000) + 0.25·sin(2π·1234.5·n/16000)`, n = 0..8000.
    #[test]
    fn log_mel_matches_parakeet_mlx() {
        let pcm: Vec<f32> = (0..8000)
            .map(|n| {
                let n = n as f64;
                (0.5 * (2.0 * std::f64::consts::PI * 440.0 * n / 16000.0).sin()
                    + 0.25 * (2.0 * std::f64::consts::PI * 1234.5 * n / 16000.0).sin())
                    as f32
            })
            .collect();
        let mel = log_mel(&pcm, &filterbank_array())
            .unwrap()
            .contiguous()
            .unwrap();
        assert_eq!(mel.shape(), &[1, 51, 128]);
        let v: &[f32] = mel.as_slice();
        let at = |t: usize, f: usize| v[t * 128 + f];
        let oracle: &[((usize, usize), f32)] = &[
            ((0, 0), 4.787_711),
            ((0, 40), 2.427_402),
            ((0, 127), 5.288_674),
            ((1, 5), 3.853_853),
            ((1, 100), 4.445_015),
            ((10, 0), -0.255_293_7),
            ((25, 40), -0.187_235_25),
            ((50, 0), 2.809_578),
            ((50, 40), 6.238_788),
            ((50, 127), 0.476_270_9),
        ];
        for &((t, f), want) in oracle {
            assert!(
                (at(t, f) - want).abs() < 2e-3,
                "mel[{t},{f}] = {} want {want}",
                at(t, f)
            );
        }
        let mean_abs = v.iter().map(|x| x.abs()).sum::<f32>() / v.len() as f32;
        assert!(
            (mean_abs - 0.470_609_4).abs() < 1e-3,
            "mean |mel| {mean_abs}"
        );
    }
}
