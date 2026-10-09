//! Live dictation into the composer (design doc 54 §5–6): the region
//! splice, the per-dictation state and the header level meter. Pure; the
//! keys and the render live in `ui.rs`, the events in `model.rs`.

use std::collections::VecDeque;
use std::time::Instant;

/// One utterance as the daemon reports it: `text` so far (the engine's
/// `shaped`, 153 §4; see [`shown`]), `is_final` once the VAD closed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utt {
    /// Text so far (append-only per utterance). Empty = dropped from the
    /// region ("scratch that" re-sends an earlier utterance empty, 153 §7.4).
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
    /// "send it" is on the latest piece (153 §7.3): `Finished` splices and submits.
    pub send: bool,
    /// The mic's last `DictationDevice` (153 §8.2), for the header.
    pub mic: Option<String>,
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
            send: false,
            mic: None,
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

    /// Fold a `DictationText` in: the shown text per [`shown`], and whether
    /// "send it" is armed/fired on this piece.
    pub fn text_event(
        &mut self,
        n: usize,
        text: String,
        shaped: String,
        command: Option<voice::EndCommand>,
        is_final: bool,
    ) {
        self.send = matches!(command, Some(voice::EndCommand::Send { .. }));
        self.set_utt(n, shown(text, shaped, command.is_some()), is_final);
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

/// What the region shows for one piece: `shaped` (153 §4.4). An engine
/// that sends no `shaped` (serde default `""`) falls back to `text` — but
/// only when no end command explains the emptiness: "send it" / "scratch
/// that" alone shape to `""` on purpose (§7.1), and so does the scratched
/// utterance's empty re-send.
pub fn shown(text: String, shaped: String, has_command: bool) -> String {
    if shaped.is_empty() && !has_command {
        text
    } else {
        shaped
    }
}

/// A dictionary suggestion (153 §6.4) shown as one line under the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Answered with `dictionary_suggestion_answer`.
    pub id: voice::SuggestionId,
    /// What the model heard.
    pub heard: String,
    /// What was typed instead.
    pub typed: String,
    /// How often the pair was seen.
    pub occurrences: u32,
    /// Unix ms when it was shown; [`SUGGESTION_MS`] later it dismisses as "not now".
    pub shown_ms: u64,
}

/// Auto-dismiss (153 §6.4): counts as "not now" without the engine's cooldown.
pub const SUGGESTION_MS: u64 = 20_000;

impl Suggestion {
    /// The prompt line.
    pub fn line(&self) -> String {
        format!(
            "dictionary: you changed “{}” → “{}” after dictating ({}×). add it?  [y] add · [n] not now · [N] never · [p] play (desktop only)",
            self.heard, self.typed, self.occurrences
        )
    }

    /// Past its 20 s.
    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.shown_ms) >= SUGGESTION_MS
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

    /// 153 §4.4 / §7.4: the region renders `shaped`, falls back to `text`
    /// only for an engine that sends none, empties a scratched utterance
    /// and tracks "send it" per piece. Each row: events `(utt, text,
    /// shaped, command, final)` on an editor `before | after`, then the
    /// spliced value and whether "send it" is on the latest piece.
    #[test]
    fn splice_over_shaped_table() {
        use voice::EndCommand::{ScratchThat, Send};
        let send = Some(Send { near_miss: false });
        type Ev = (
            usize,
            &'static str,
            &'static str,
            Option<voice::EndCommand>,
            bool,
        );
        let rows: &[(&str, &str, &str, &[Ev], &str, bool)] = &[
            (
                "shaped wins over text",
                "",
                "",
                &[(0, "hello comma world", "hello, world", None, true)],
                "hello, world",
                false,
            ),
            (
                "no shaped (older engine): text",
                "",
                "",
                &[(0, "hi there", "", None, false)],
                "hi there",
                false,
            ),
            (
                "a partial re-shapes at the final",
                "",
                "",
                &[
                    (0, "parakeet dot", "parakeet dot", None, false),
                    (0, "parakeet dot rs", "parakeet.rs", None, true),
                ],
                "parakeet.rs",
                false,
            ),
            (
                "send it alone shapes to nothing: editor unchanged",
                "hel",
                "lo",
                &[(0, "send it", "", send, true)],
                "hello",
                true,
            ),
            (
                "phrase removed, send armed",
                "",
                "",
                &[(0, "ship it send it", "ship it", send, false)],
                "ship it",
                true,
            ),
            (
                "scratch that empties the earlier utterance; the scratch itself shows nothing",
                "",
                "",
                &[
                    (0, "one", "one", None, true),
                    (1, "two", "two", None, true),
                    (1, "", "", None, true),
                    (2, "scratch that", "", Some(ScratchThat), true),
                ],
                "one",
                false,
            ),
            (
                "dictation continues after a scratch",
                "a",
                "b",
                &[
                    (0, "one", "one", None, true),
                    (0, "", "", None, true),
                    (1, "scratch that", "", Some(ScratchThat), true),
                    (2, "three", "three", None, false),
                ],
                "a three b",
                false,
            ),
            (
                "scratching the only utterance restores the editor",
                "x",
                "",
                &[
                    (0, "oops", "oops", None, true),
                    (0, "", "", None, true),
                    (1, "scratch that", "", Some(ScratchThat), true),
                ],
                "x",
                false,
            ),
            (
                "a new utterance without the phrase disarms",
                "",
                "",
                &[
                    (0, "x send it", "x", send, true),
                    (1, "and more", "and more", None, false),
                ],
                "x and more",
                false,
            ),
            (
                "newline inside shaped lands verbatim",
                "",
                ".",
                &[(0, "done new line next", "done.\nnext", None, true)],
                "done.\nnext.",
                false,
            ),
        ];
        for (name, before, after, events, value, send) in rows {
            let mut d = Dictating::new(
                voice::DictationId(uuid::Uuid::nil()),
                (*before).to_owned(),
                (*after).to_owned(),
            );
            for (n, text, shaped, command, is_final) in events.iter().copied() {
                d.text_event(n, text.to_owned(), shaped.to_owned(), command, is_final);
            }
            assert_eq!(d.splice().0, *value, "{name}");
            assert_eq!(d.send, *send, "{name}: send");
        }
    }
}
