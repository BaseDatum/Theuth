//! Spoken end commands on partials (153 §7.1): a state machine the shaper
//! feeds per piece — `Idle → Armed` on an exact phrase at a partial's tail,
//! sticky across re-decodes, fired at the final (near miss within
//! `max(1, len / 5)` edits for phrases ≥ 5 chars), disarmed by "literal".
//! The actor owns one parser per utterance (`reset` between utterances) and
//! runs the §7.3 timers off [`SETTLE_MS`] / [`SILENCE_MS`].

use super::{EndCommand, EndPhrases};

/// §7.3: "sending in 1.5 s" after `Send` arms on a partial.
pub const SETTLE_MS: u64 = 1500;
/// §7.3: silence after the settle that stops the dictation.
pub const SILENCE_MS: u64 = 350;
/// §7.1: shorter phrases get no near-miss tolerance.
pub const NEAR_MISS_MIN_LEN: usize = 5;

/// The §7.1 state machine for one utterance.
#[derive(Debug, Default)]
pub struct EndParser {
    send: Vec<Vec<String>>,
    scratch: Vec<Vec<String>>,
    literal: String,
    armed: Option<EndCommand>,
    consumed: usize,
}

fn words_of(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// §7.1: `max(1, len / 5)` edits, none under [`NEAR_MISS_MIN_LEN`] chars.
fn tolerance(phrase_len: usize) -> Option<usize> {
    (phrase_len >= NEAR_MISS_MIN_LEN).then_some((phrase_len / 5).max(1))
}

impl EndParser {
    /// A fresh parser for `phrases`.
    pub fn new(phrases: &EndPhrases) -> Self {
        let split = |v: &[String]| {
            v.iter()
                .map(|p| words_of(p))
                .filter(|w| !w.is_empty())
                .collect()
        };
        Self {
            send: split(&phrases.send),
            scratch: split(&phrases.scratch),
            literal: words_of(&phrases.literal).join(" "),
            armed: None,
            consumed: 0,
        }
    }

    /// Feed the utterance's text so far; `Some` when a command is armed (partial) or fires (final).
    pub fn feed(&mut self, raw: &str, is_final: bool) -> Option<EndCommand> {
        let words = words_of(raw);
        self.consumed = 0;
        // Exact phrase at the tail (Idle); once armed, up to two re-decoded words may follow it.
        let slack = if self.armed.is_some() { 2 } else { 0 };
        let exact = self.commands().into_iter().find_map(|(cmd, phrases)| {
            phrases.iter().find_map(|p| {
                let k = p.len();
                (0..=slack).find_map(|extra| {
                    let start = words.len().checked_sub(k + extra)?;
                    (words[start..start + k] == p[..]).then_some((cmd, start))
                })
            })
        });
        if let Some((cmd, start)) = exact {
            if start > 0 && !self.literal.is_empty() && words[start - 1] == self.literal {
                self.armed = None;
                return None;
            }
            self.armed = Some(cmd);
            self.consumed = words.len() - start;
            return Some(cmd);
        }
        let armed = self.armed?;
        if !is_final {
            return Some(armed); // sticky across re-decodes
        }
        // Final without the exact phrase: a near miss on the armed command's phrases.
        let phrases = match armed {
            EndCommand::Send { .. } => &self.send,
            EndCommand::ScratchThat => &self.scratch,
        };
        let near = phrases.iter().find_map(|p| {
            let start = words.len().checked_sub(p.len())?;
            let phrase = p.join(" ");
            let tol = tolerance(phrase.len())?;
            (strsim::levenshtein(&words[start..].join(" "), &phrase) <= tol).then_some(p.len())
        });
        self.armed = None;
        let k = near?;
        self.consumed = k;
        Some(match armed {
            EndCommand::Send { .. } => EndCommand::Send { near_miss: true },
            EndCommand::ScratchThat => EndCommand::ScratchThat,
        })
    }

    /// Words at the tail the last `feed` matched (phrase + anything after it); 0 when nothing to remove.
    pub fn consumed(&self) -> usize {
        self.consumed
    }

    /// Back to `Idle` (a new utterance).
    pub fn reset(&mut self) {
        self.armed = None;
        self.consumed = 0;
    }

    fn commands(&self) -> [(EndCommand, &[Vec<String>]); 2] {
        [
            (EndCommand::Send { near_miss: false }, &self.send),
            (EndCommand::ScratchThat, &self.scratch),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEND: EndCommand = EndCommand::Send { near_miss: false };
    const NEAR: EndCommand = EndCommand::Send { near_miss: true };

    fn parser() -> EndParser {
        EndParser::new(&EndPhrases::default())
    }

    #[test]
    fn arms_on_partial() {
        let mut p = parser();
        assert_eq!(p.feed("ship it tonight", false), None);
        assert_eq!(p.feed("ship it tonight send it", false), Some(SEND));
        assert_eq!(p.consumed(), 2);
        assert_eq!(p.feed("ship it tonight send it.", true), Some(SEND));
    }

    #[test]
    fn sticky_across_redecode() {
        let mut p = parser();
        assert_eq!(p.feed("ship it tonight send it", false), Some(SEND));
        assert_eq!(p.feed("ship it tonight sent", false), Some(SEND));
        assert_eq!(p.consumed(), 0);
        assert_eq!(p.feed("ship it tonight sent it", true), Some(NEAR));
        assert_eq!(p.consumed(), 2);
    }

    #[test]
    fn near_miss_only_when_armed_and_len_ge_5() {
        let mut p = parser();
        assert_eq!(p.feed("ship it tonight sent it", true), None);
        let short = EndPhrases {
            send: vec!["send".into()],
            ..EndPhrases::default()
        };
        let mut p = EndParser::new(&short);
        assert_eq!(p.feed("ship it send", false), Some(SEND));
        assert_eq!(p.feed("ship it end", true), None);
        assert_eq!(tolerance(4), None);
        assert_eq!(tolerance(7), Some(1));
        assert_eq!(tolerance(12), Some(2));
    }

    #[test]
    fn end_is_not_send() {
        let mut p = parser();
        assert_eq!(p.feed("the end", false), None);
        assert_eq!(p.feed("the end", true), None);
        assert_eq!(p.feed("send it", false), Some(SEND));
        assert_eq!(p.feed("the end", true), None);
    }

    #[test]
    fn literal_disarms() {
        let mut p = parser();
        assert_eq!(p.feed("ship it send it", false), Some(SEND));
        assert_eq!(p.feed("ship it literal send it", false), None);
        assert_eq!(p.feed("ship it literal send it", true), None);
    }

    #[test]
    fn scratch_arms_then_fires_on_final() {
        let mut p = parser();
        assert_eq!(p.feed("scratch that", false), Some(EndCommand::ScratchThat));
        assert_eq!(p.feed("scratch that", true), Some(EndCommand::ScratchThat));
        p.reset();
        assert_eq!(p.feed("scratch this", true), None);
    }
}
