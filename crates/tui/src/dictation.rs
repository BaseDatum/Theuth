//! Live dictation into the composer (design doc 54 §5–6): the region
//! splice, the per-dictation state and the header level meter. Pure; the
//! keys and the render live in `ui.rs`, the events in `model.rs`.

use std::collections::VecDeque;
use std::time::Instant;

/// One utterance as the daemon reports it: `text` so far, `is_final` once
/// the VAD closed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utt {
    /// Text so far (append-only per utterance).
    pub text: String,
    /// The utterance is closed: rendered in full colour.
    pub is_final: bool,
}

/// Where a dictation is in its life (54 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// `dictation_start` answered; waiting for `DictationStarted`.
    Starting,
    /// The mic is open.
    Dictating,
    /// `dictation_stop` sent; still rendering until `DictationFinished`.
    Stopping,
}

/// One dictation in progress, anchored at the caret it started from.
#[derive(Clone, Debug)]
pub struct Dictating {
    /// The daemon's id; events for other ids are ignored.
    pub id: voice::DictationId,
    /// Editor text up to the caret at start. Never modified.
    pub before: String,
    /// Editor text from the caret at start. Never modified.
    pub after: String,
    /// Utterances by index (`DictationText.utterance`).
    pub utts: Vec<Utt>,
    /// Last 24 mic levels (0–1), newest last.
    pub levels: VecDeque<f32>,
    /// When it started, for the header's clock.
    pub started: Instant,
    /// Lifecycle.
    pub phase: Phase,
    /// Esc was pressed: `Finished` must not splice (the editor is restored already).
    pub discard: bool,
}

/// Meter width in the header.
pub const METER_CELLS: usize = 24;

impl Dictating {
    /// Fresh state for `id`, anchored at `before | after`.
    pub fn new(id: voice::DictationId, before: String, after: String) -> Self {
        Self {
            id,
            before,
            after,
            utts: Vec::new(),
            levels: VecDeque::with_capacity(METER_CELLS),
            started: Instant::now(),
            phase: Phase::Starting,
            discard: false,
        }
    }

    /// `lead` (54 §5.1): a space unless `before` is empty or ends in whitespace.
    pub fn lead(&self) -> &'static str {
        lead(&self.before)
    }

    /// `trail` (54 §5.1): a space unless `after` is empty or starts with whitespace or punctuation.
    pub fn trail(&self) -> &'static str {
        trail(&self.after)
    }

    /// Fold `levels` in, keeping the newest [`METER_CELLS`].
    pub fn push_levels(&mut self, levels: &[f32]) {
        self.levels.extend(levels);
        while self.levels.len() > METER_CELLS {
            self.levels.pop_front();
        }
    }

    /// Set utterance `n`, growing the list.
    pub fn set_utt(&mut self, n: usize, text: String, is_final: bool) {
        if self.utts.len() <= n {
            self.utts.resize(n + 1, Utt::default());
        }
        self.utts[n] = Utt { text, is_final };
    }

    /// The composer value and caret after a stop: every utterance rendered,
    /// final or not, caret at the end of the region.
    pub fn splice(&self) -> (String, usize) {
        splice(&self.before, &self.after, &self.utts)
    }

    /// Whole seconds since start, as `mm:ss`.
    pub fn clock(&self) -> String {
        let s = self.started.elapsed().as_secs();
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn lead(before: &str) -> &'static str {
    if before.is_empty() || before.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    }
}

fn trail(after: &str) -> &'static str {
    if after.is_empty() || after.starts_with(|c: char| c.is_whitespace() || ".,;:!?".contains(c)) {
        ""
    } else {
        " "
    }
}

/// The region body: non-empty utterances joined by one space.
pub fn body(utts: &[Utt]) -> String {
    let mut out = String::new();
    for u in utts.iter().filter(|u| !u.text.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&u.text);
    }
    out
}

/// `before + region + after` and the caret after the last dictated word —
/// before the trailing separator, so typing continues the sentence (54 §5.1).
/// An empty body leaves the value and caret unchanged.
pub fn splice(before: &str, after: &str, utts: &[Utt]) -> (String, usize) {
    let body = body(utts);
    let mut value = String::with_capacity(before.len() + body.len() + after.len() + 2);
    value.push_str(before);
    let mut caret = value.len();
    if !body.is_empty() {
        value.push_str(lead(before));
        value.push_str(&body);
        caret = value.len();
        value.push_str(trail(after));
    }
    value.push_str(after);
    (value, caret)
}

/// `cells` glyphs of `▁▂▃▄▅▆▇█` from `levels` (0–1, newest last), newest at
/// the right, log-scaled so speech fills the bar, padded with `▁` on the left.
pub fn meter(levels: &VecDeque<f32>, cells: usize) -> String {
    const GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let shown = levels.len().min(cells);
    let mut out = String::with_capacity(cells * 3);
    out.extend(std::iter::repeat_n('▁', cells - shown));
    out.extend(levels.iter().skip(levels.len() - shown).map(|&l| {
        let v = (1.0 + 9.0 * l.clamp(0.0, 1.0)).log10().clamp(0.0, 1.0);
        // 7.999 keeps 1.0 on the last glyph.
        GLYPHS[(v * 7.999) as usize]
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utt(text: &str, is_final: bool) -> Utt {
        Utt {
            text: text.into(),
            is_final,
        }
    }

    #[test]
    fn splice_empty_before_and_after() {
        assert_eq!(splice("", "", &[utt("hi", true)]), ("hi".into(), 2));
        assert_eq!(splice("", "x", &[utt("hi", true)]), ("hi x".into(), 2));
        assert_eq!(splice("x", "", &[utt("hi", true)]), ("x hi".into(), 4));
    }

    #[test]
    fn splice_no_double_space_after_trailing_space() {
        assert_eq!(
            splice("hello ", "", &[utt("there", true)]),
            ("hello there".into(), 11)
        );
        assert_eq!(splice("a\n", "b", &[utt("c", false)]), ("a\nc b".into(), 3));
    }

    #[test]
    fn splice_no_space_before_punctuation() {
        assert_eq!(
            splice("hello", ".", &[utt("there", true)]),
            ("hello there.".into(), 11)
        );
        assert_eq!(splice("a", " b", &[utt("c", true)]), ("a c b".into(), 3));
    }

    #[test]
    fn splice_mid_word_caret_never_merges() {
        assert_eq!(
            splice("hel", "lo", &[utt("there", true)]),
            ("hel there lo".into(), 9)
        );
    }

    #[test]
    fn splice_multibyte_before_caret() {
        let (v, c) = splice("héllo→", "wörld", &[utt("x", true)]);
        assert_eq!(v, "héllo→ x wörld");
        assert_eq!(c, "héllo→ x".len());
        assert!(v.is_char_boundary(c));
    }

    #[test]
    fn splice_joins_utterances_and_drops_empty_ones() {
        let utts = [utt("one.", true), utt("", true), utt("two", false)];
        assert_eq!(splice("a", "b", &utts), ("a one. two b".into(), 10));
    }

    #[test]
    fn splice_all_empty_is_unchanged() {
        assert_eq!(
            splice("hel", "lo", &[utt("", true), utt("", false)]),
            ("hello".into(), 3)
        );
        assert_eq!(splice("hel", "lo", &[]), ("hello".into(), 3));
    }

    #[test]
    fn meter_pads_left_and_scales() {
        let mut levels = VecDeque::new();
        assert_eq!(meter(&levels, 4), "▁▁▁▁");
        levels.extend([0.0, 1.0]);
        assert_eq!(meter(&levels, 4), "▁▁▁█");
        // log10(1 + 9·0.1) ≈ 0.28 → the third glyph.
        levels.push_back(0.1);
        assert_eq!(meter(&levels, 4), "▁▁█▃");
        // More levels than cells: the newest win.
        levels.extend([0.5, 0.5, 0.5]);
        assert_eq!(meter(&levels, 2), "▆▆");
    }

    #[test]
    fn set_utt_grows_and_levels_cap() {
        let mut d = Dictating::new(
            voice::DictationId(uuid::Uuid::nil()),
            "a".into(),
            "b".into(),
        );
        d.set_utt(1, "two".into(), false);
        assert_eq!(d.utts.len(), 2);
        assert_eq!(d.splice(), ("a two b".into(), 5));
        d.push_levels(&[0.5; 30]);
        assert_eq!(d.levels.len(), METER_CELLS);
    }
}
