//! Voice activity detection for dictation (54 §0.4, `meetings/02-live.md` §0.1c):
//! TEN-VAD scores 16 ms frames; [`Gate`] turns the score stream into utterance
//! edges. The gate is pure over probabilities so the rules are unit-tested
//! without audio. No `wavekat_vad` type leaves here.
//!
//! The TEN-VAD ONNX model is `include_bytes!`-embedded by `wavekat-vad`'s build
//! script, which downloads it unless `TEN_VAD_MODEL_PATH` names a local
//! `ten-vad.onnx` (set it in offline/release builds). Its `ort` session shares
//! the process-wide runtime, so [`Vad::new`] must run after [`crate::stt::Stream::open`]
//! (or any `models::onnx::ort_lib` call) has loaded the dylib — the caller orders that.

use wavekat_vad::FrameAdapter;
use wavekat_vad::backends::ten_vad::TenVad;

/// Samples per VAD frame at 16 kHz (16 ms). 35 frames = one [`crate::stt::WINDOW`].
pub const FRAME: usize = 256;

/// The VAD's only rate.
const RATE: u32 = 16_000;

/// TEN-VAD behind a `FrameAdapter`: any chunk length in, one probability per
/// completed [`FRAME`] out. Stateful; `reset` at turn boundaries with the stream.
pub struct Vad {
    inner: FrameAdapter,
    /// f32 → i16 scratch, reused so the 20 ms path does not allocate.
    pcm: Vec<i16>,
}

impl Vad {
    /// Build the TEN-VAD session (needs `ort` initialised first, see the module doc).
    pub fn new() -> anyhow::Result<Vad> {
        let vad = TenVad::new().map_err(|e| anyhow::anyhow!("TEN-VAD: {e}"))?;
        Ok(Vad {
            inner: FrameAdapter::new(Box::new(vad)),
            pcm: Vec::new(),
        })
    }

    /// Score `pcm16k` (mono f32, any length). Returns one probability in `0.0..=1.0`
    /// per frame completed by this call; a trailing partial frame is carried over.
    pub fn process(&mut self, pcm16k: &[f32]) -> Vec<f32> {
        self.pcm.clear();
        self.pcm.extend(
            pcm16k
                .iter()
                .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        );
        let mut out = Vec::with_capacity((self.pcm.len() + FRAME) / FRAME);
        if let Err(e) = self.inner.process_each(&self.pcm, RATE, |p| out.push(p)) {
            // ponytail: an ONNX failure mid-stream drops the rest of this chunk; the
            // gate just sees fewer frames. Surface as an Err if it ever happens in practice.
            tracing::warn!("voice: VAD frame failed: {e}");
        }
        out
    }

    /// Forget hidden state and the carried partial frame.
    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

/// An utterance edge from [`Gate::push`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Speech confirmed: the utterance starts [`Gate::PREROLL`] frames before the first of
    /// the [`Gate::ONSET`] frames that confirmed it, i.e. at `this_frame - ONSET + 1 - PREROLL`.
    Onset,
    /// The utterance ended (silence, or the [`Gate::MAX_OPEN`] cap). Flush and finalize.
    Close,
    /// [`Gate::TURN`] frames of silence since speech ended: reset the stream and the VAD.
    Turn,
}

/// Utterance state machine over per-frame speech probabilities (threshold 0.5).
/// Pure: no audio, no clock — one frame per `push`.
#[derive(Debug, Default)]
pub struct Gate {
    /// Consecutive frames > 0.5.
    loud: usize,
    /// Consecutive frames ≤ 0.5.
    quiet: usize,
    /// Frames since the utterance opened (counts the onset run).
    open_for: usize,
    open: bool,
    /// A `Close` happened and its `Turn` has not fired yet.
    closed: bool,
}

impl Gate {
    /// Frames of audio kept before the onset run so the first syllable is not clipped (320 ms).
    pub const PREROLL: usize = 20;
    /// Consecutive speech frames that confirm an onset (160 ms).
    pub const ONSET: usize = 10;
    /// Consecutive silent frames that close an open utterance (≈300 ms).
    pub const CLOSE: usize = 19;
    /// Longest utterance in frames (15 s); closed mid-speech and reopened by the next onset.
    pub const MAX_OPEN: usize = 15_000 / 16;
    /// Silent frames since speech ended that make a turn boundary (2 s).
    pub const TURN: usize = 2_000 / 16;

    /// A closed gate.
    pub fn new() -> Gate {
        Gate::default()
    }

    /// Whether an utterance is open (frames should be fed to the stream).
    pub fn open(&self) -> bool {
        self.open
    }

    /// Back to the initial state.
    pub fn reset(&mut self) {
        *self = Gate::default();
    }

    /// One frame's speech probability in; the edge it caused, if any.
    pub fn push(&mut self, prob: f32) -> Option<Edge> {
        if prob > 0.5 {
            self.loud += 1;
            self.quiet = 0;
        } else {
            self.quiet += 1;
            self.loud = 0;
        }
        if self.open {
            self.open_for += 1;
            if self.quiet >= Self::CLOSE || self.open_for >= Self::MAX_OPEN {
                self.open = false;
                self.closed = true;
                // A cap close lands mid-speech: let the next ONSET frames reopen at once.
                self.loud = 0;
                return Some(Edge::Close);
            }
            return None;
        }
        if self.loud == Self::ONSET {
            self.open = true;
            self.open_for = Self::ONSET;
            self.closed = false;
            return Some(Edge::Onset);
        }
        if self.closed && self.quiet == Self::TURN {
            self.closed = false;
            return Some(Edge::Turn);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `probs`; return `(frame_index, edge)` for every edge.
    fn edges(probs: impl IntoIterator<Item = f32>) -> Vec<(usize, Edge)> {
        let mut g = Gate::new();
        probs
            .into_iter()
            .enumerate()
            .filter_map(|(i, p)| g.push(p).map(|e| (i, e)))
            .collect()
    }

    fn run(n: usize, p: f32) -> impl Iterator<Item = f32> {
        std::iter::repeat_n(p, n)
    }

    #[test]
    fn onset_close_turn() {
        let probs = run(30, 0.1)
            .chain(run(5, 0.9)) // too short: no onset
            .chain(run(30, 0.1))
            .chain(run(12, 0.9)) // onset on the 10th
            .chain(run(20, 0.1)) // close on the 19th
            .chain(run(130, 0.1)); // turn when silence reaches TURN
        let speech = 30 + 5 + 30;
        let silence = speech + 12;
        assert_eq!(
            edges(probs),
            vec![
                (speech + Gate::ONSET - 1, Edge::Onset),
                (silence + Gate::CLOSE - 1, Edge::Close),
                (silence + Gate::TURN - 1, Edge::Turn),
            ]
        );
    }

    #[test]
    fn onset_slices_preroll_from_first_loud_frame() {
        let mut g = Gate::new();
        let mut onset_at = None;
        for (i, p) in run(50, 0.0).chain(run(10, 1.0)).enumerate() {
            if g.push(p) == Some(Edge::Onset) {
                onset_at = Some(i);
            }
        }
        let i = onset_at.unwrap();
        assert!(g.open());
        assert_eq!(i - Gate::ONSET + 1, 50, "first loud frame");
        assert_eq!(i - Gate::ONSET + 1 - Gate::PREROLL, 30, "utterance start");
    }

    #[test]
    fn max_open_caps_and_reopens() {
        let n = Gate::MAX_OPEN + Gate::ONSET;
        let e = edges(run(n, 0.9));
        assert_eq!(
            e,
            vec![
                (Gate::ONSET - 1, Edge::Onset),
                (Gate::MAX_OPEN - 1, Edge::Close),
                (Gate::MAX_OPEN + Gate::ONSET - 1, Edge::Onset),
            ]
        );
    }

    #[test]
    fn no_turn_without_a_close() {
        assert!(edges(run(3 * Gate::TURN, 0.0)).is_empty());
    }
}
