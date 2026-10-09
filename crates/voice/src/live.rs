//! Audio → utterances (54 §4.5, `meetings/02-live.md` §0.7): one [`Utterer`]
//! owns a streaming Nemotron session, a VAD, the utterance [`Gate`] and the
//! resampler, and turns any-length PCM chunks into punctuated text pieces.
//! The dictation task and the meeting's two track tasks both drive it; it
//! knows nothing about microphones, clocks or actors.

use std::path::Path;

use rubato::Resampler as _;

use crate::stt::{Stream, WINDOW};
use crate::vad::{Edge, FRAME, Gate, Vad};

/// The models' input rate.
pub const MODEL_RATE: u32 = 16_000;
/// 16 kHz samples kept behind the newest VAD frame while no utterance is open:
/// the pre-roll, the onset run, one frame of slack.
const HISTORY: usize = (Gate::PREROLL + Gate::ONSET + 1) * FRAME;

/// A piece of an utterance's text: rolling while it is open, `is_final` once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// Index of the utterance (0-based, per `Utterer`; `reset` does not restart it).
    pub utterance: u32,
    /// 16 kHz sample the utterance starts at (pre-roll included), counted from
    /// the first sample this `Utterer` saw — compare with [`Utterer::position`].
    pub start: usize,
    /// Everything said so far in this utterance, trimmed.
    pub text: String,
    /// The utterance closed; no more text will follow for it.
    pub is_final: bool,
}

/// Streaming STT + VAD + gate + resampler. `Send`, not `Sync`: one owner.
pub struct Utterer {
    stream: Stream,
    vad: Vad,
    gate: Gate,
    /// Source rate; `resampler` is `None` at [`MODEL_RATE`].
    rate: u32,
    /// FIR state must survive chunk boundaries; needs whole `rate / 50` blocks.
    resampler: Option<rubato::FftFixedIn<f32>>,
    /// Source samples waiting for a full block.
    inbox: Vec<f32>,
    /// 16 kHz samples from absolute sample `base` on.
    buf: Vec<f32>,
    base: usize,
    /// 16 kHz samples seen so far.
    total: usize,
    /// `Some(fed)` while an utterance is open: samples before `fed` went to the model.
    open: Option<usize>,
    /// Start of the open utterance.
    start: usize,
    /// Index of the open (or next) utterance.
    utterance: u32,
    /// Text of the open utterance.
    text: String,
}

impl Utterer {
    /// Load the streaming model from `dir`, then the VAD (the `ort` init order,
    /// 02-live §0.1c); `data_dir` locates the runtime. Source rate starts at
    /// [`MODEL_RATE`]; see [`Utterer::set_rate`].
    pub fn open(dir: &Path, data_dir: &Path) -> anyhow::Result<Utterer> {
        let stream = Stream::open(dir, data_dir)?;
        let vad = Vad::new()?;
        Ok(Utterer {
            stream,
            vad,
            gate: Gate::new(),
            rate: MODEL_RATE,
            resampler: None,
            inbox: Vec::new(),
            buf: Vec::new(),
            base: 0,
            total: 0,
            open: None,
            start: 0,
            utterance: 0,
            text: String::new(),
        })
    }

    /// Source sample rate of what [`Utterer::chunk`] receives; a multiple of 50.
    pub fn set_rate(&mut self, rate: u32) -> anyhow::Result<()> {
        anyhow::ensure!(
            rate.is_multiple_of(50) && (8_000..=192_000).contains(&rate),
            "sample rate {rate} is not a multiple of 50 in 8–192 kHz"
        );
        self.resampler = if rate == MODEL_RATE {
            None
        } else {
            Some(rubato::FftFixedIn::<f32>::new(
                rate as usize,
                MODEL_RATE as usize,
                rate as usize / 50,
                2,
                1,
            )?)
        };
        self.rate = rate;
        self.inbox.clear();
        Ok(())
    }

    /// 16 kHz samples consumed so far (the clock [`Text::start`] is on).
    pub fn position(&self) -> usize {
        self.total
    }

    /// The gate's speech threshold (see [`Gate::set_threshold`]); [`Gate::THRESHOLD`]
    /// by default and untouched by [`Utterer::reset`].
    pub fn set_vad_threshold(&mut self, threshold: f32) {
        self.gate.set_threshold(threshold);
    }

    /// An utterance is open (audio is being fed to the model).
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Any-length source PCM in; the text pieces it produced out.
    pub fn chunk(&mut self, pcm: &[f32]) -> anyhow::Result<Vec<Text>> {
        let mut out = Vec::new();
        let Some(block) = self.resampler.as_ref().map(|r| r.input_frames_next()) else {
            self.pcm16(pcm, &mut out)?;
            return Ok(out);
        };
        self.inbox.extend_from_slice(pcm);
        let mut at = 0;
        while self.inbox.len() - at >= block {
            let resampled = self
                .resampler
                .as_mut()
                .expect("checked")
                .process(&[&self.inbox[at..at + block]], None)?;
            at += block;
            self.pcm16(&resampled[0], &mut out)?;
        }
        self.inbox.drain(..at);
        Ok(out)
    }

    /// 16 kHz samples: VAD, gate edges, feed.
    fn pcm16(&mut self, new: &[f32], out: &mut Vec<Text>) -> anyhow::Result<()> {
        self.buf.extend_from_slice(new);
        self.total += new.len();
        let probs = self.vad.process(new);
        for (k, &p) in probs.iter().enumerate() {
            // The last frame ends at (within a frame of) `total`; earlier ones FRAME apart.
            let end = self.total - (probs.len() - 1 - k) * FRAME;
            match self.gate.push(p) {
                Some(Edge::Onset) if self.open.is_none() => {
                    let start = end.saturating_sub((Gate::ONSET + Gate::PREROLL) * FRAME);
                    self.start = start.max(self.base);
                    self.open = Some(self.start);
                }
                Some(Edge::Close) => self.finish(out)?,
                Some(Edge::Turn) => {
                    self.stream.reset();
                    self.vad.reset();
                }
                _ => {}
            }
        }
        self.feed_full(out)?;
        let keep = self
            .open
            .unwrap_or_else(|| self.total.saturating_sub(HISTORY))
            .max(self.base);
        self.buf.drain(..keep - self.base);
        self.base = keep;
        Ok(())
    }

    /// Feed every full window past `fed`; collect new text.
    fn feed_full(&mut self, out: &mut Vec<Text>) -> anyhow::Result<()> {
        while let Some(fed) = self.open
            && self.total - fed >= WINDOW
        {
            let at = fed - self.base;
            let window: &[f32; WINDOW] = self.buf[at..at + WINDOW].try_into()?;
            let t = self.stream.feed(window)?;
            self.open = Some(fed + WINDOW);
            if self.append(&t) {
                let p = self.piece(false);
                if !p.text.is_empty() {
                    out.push(p);
                }
            }
        }
        Ok(())
    }

    /// Close the open utterance: the partial window zero-padded, then one zero
    /// window to pull the tail out of the decoder's lookahead; the `final` piece
    /// (its text may be empty — the caller decides what an empty utterance is).
    /// Nothing when no utterance is open.
    fn finish(&mut self, out: &mut Vec<Text>) -> anyhow::Result<()> {
        self.feed_full(out)?;
        let Some(fed) = self.open.take() else {
            return Ok(());
        };
        let mut window = [0f32; WINDOW];
        let rest = &self.buf[fed - self.base..];
        window[..rest.len()].copy_from_slice(rest);
        let t = self.stream.feed(&window)?;
        self.append(&t);
        window.fill(0.0);
        let t = self.stream.feed(&window)?;
        self.append(&t);
        out.push(self.piece(true));
        self.utterance += 1;
        self.text.clear();
        Ok(())
    }

    /// The text so far. A close's zero window does not always pull the sentence's
    /// last punctuation mark out of the decoder; it then arrives with the next
    /// utterance's first window as `". The …"` — that stray lead-in is dropped.
    fn piece(&self, is_final: bool) -> Text {
        let text = self
            .text
            .trim()
            .trim_start_matches(|c: char| ".,?!;:".contains(c))
            .trim_start();
        Text {
            utterance: self.utterance,
            start: self.start,
            text: text.to_owned(),
            is_final,
        }
    }

    /// Append new model text verbatim. SentencePiece marks word starts with a
    /// leading space (`▁`), so a window boundary inside a word yields `"t"` then
    /// `"ags"` — inserting a space here would split it. `false` when blank.
    fn append(&mut self, t: &str) -> bool {
        if t.trim().is_empty() {
            return false;
        }
        self.text.push_str(t);
        true
    }

    /// Force the open utterance closed (stop, push-to-talk release, pause):
    /// its `final` piece, if it said anything. The gate restarts so the next
    /// speech frames open a new utterance at once.
    pub fn close(&mut self) -> anyhow::Result<Vec<Text>> {
        let mut out = Vec::new();
        self.finish(&mut out)?;
        self.gate.reset();
        Ok(out)
    }

    /// A turn boundary: model cache, VAD state and gate back to the start;
    /// buffered audio (and an open utterance's text) dropped. The sample
    /// counter and the utterance index keep counting.
    pub fn reset(&mut self) {
        self.stream.reset();
        self.vad.reset();
        self.gate.reset();
        self.inbox.clear();
        self.buf.clear();
        self.base = self.total;
        self.open = None;
        self.text.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's data dir and the dictation checkpoint under it; `None` skips the test.
    fn model_dir() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let data_dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join(".theuth");
        let dir = data_dir.join("models").join(crate::STREAMING_MODEL_SUBDIR);
        dir.is_dir().then_some((data_dir, dir))
    }

    /// `say` renders `text` as 16-bit mono 48 kHz PCM; `None` off macOS or without `say`.
    fn speech(text: &str) -> Option<Vec<f32>> {
        let path = std::env::temp_dir().join(format!("theuth-live-{}.wav", uuid::Uuid::now_v7()));
        let ok = std::process::Command::new("say")
            .args(["-o"])
            .arg(&path)
            .args(["--data-format=LEI16@48000", text])
            .status()
            .ok()?
            .success();
        if !ok {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        std::fs::remove_file(&path).ok();
        let (pcm, rate) = crate::stt::decode(bytes, "wav").ok()?;
        assert_eq!(rate, 48_000);
        Some(pcm)
    }

    /// The smoke sentence, spoken by `say`, fed as 20 ms chunks with a second of
    /// silence around it: one utterance, rolling pieces then a final one that is
    /// the sentence back, cased and punctuated; the start lands inside the leading silence.
    #[test]
    #[ignore = "loads the Nemotron streaming model (~0.9 GB) and runs `say`; run with --ignored"]
    fn utterer_returns_the_spoken_sentence_punctuated() {
        let (Some((data_dir, dir)), Some(pcm)) = (
            model_dir(),
            speech("The deploy is blocked on the image tags."),
        ) else {
            eprintln!("skipped: no streaming model or no `say`");
            return;
        };
        // `models::onnx::ort_lib` needs the dylib in `<data_dir>/models/onnxruntime/` or `$ORT_DYLIB_PATH`.
        let mut u = match Utterer::open(&dir, &data_dir) {
            Ok(u) => u,
            Err(e) if format!("{e:#}").contains("libonnxruntime") => {
                eprintln!("skipped: {e:#}");
                return;
            }
            Err(e) => panic!("{e:#}"),
        };
        u.set_rate(48_000).unwrap();
        let silence = vec![0f32; 48_000];
        let mut pieces = Vec::new();
        for pcm in [&silence, &pcm, &silence] {
            for chunk in pcm.chunks(960) {
                pieces.extend(u.chunk(chunk).unwrap());
            }
        }
        pieces.extend(u.close().unwrap());
        let finals: Vec<&Text> = pieces.iter().filter(|p| p.is_final).collect();
        assert_eq!(finals.len(), 1, "{pieces:?}");
        let f = finals[0];
        let words: String = f
            .text
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .to_lowercase();
        assert_eq!(words, "the deploy is blocked on the image tags", "{f:?}");
        assert!(f.text.ends_with('.') && f.text.starts_with('T'), "{f:?}");
        assert!(
            pieces.iter().any(|p| !p.is_final),
            "no rolling piece: {pieces:?}"
        );
        // Pre-roll: the utterance opens before the speech, inside the leading second.
        assert!(f.start < 16_000, "{}", f.start);
        assert!(f.start >= 16_000 - (Gate::PREROLL + Gate::ONSET) * FRAME - 8 * FRAME);
        // A reset drops the open state; the sample counter keeps counting (within
        // the resampler's block rounding and filter delay).
        u.reset();
        assert!(!u.is_open());
        let fed = 48_000 * 2 + pcm.len() / 960 * 960;
        assert!(
            (u.position() as i64 - (fed / 3) as i64).abs() < 1_600,
            "{}",
            u.position()
        );
    }
}
