//! Token → text joining, as parakeet-mlx 0.7.0 does it (`tokenizer.decode` per emitted token, then the
//! `AlignedResult` concatenation and `strip()`), plus the word grouping behind [`crate::Word`].

use crate::{Transcript, Word};

/// One emitted TDT token: vocabulary id, start frame and duration (encoder frames).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Token {
    pub id: usize,
    pub start_frame: usize,
    pub duration_frames: usize,
}

/// `tokenizer.decode([id], vocabulary)`: `▁` becomes a space, then a whitespace directly before `.`, `!` or `?`
/// is dropped (NeMo's `(\s)(\.|!|\?)` → `\2`).
pub(crate) fn decode_piece(piece: &str) -> String {
    let spaced: Vec<char> = piece.replace('▁', " ").chars().collect();
    let mut out = String::with_capacity(spaced.len());
    for (i, c) in spaced.iter().enumerate() {
        let next_is_punct = matches!(spaced.get(i + 1), Some('.' | '!' | '?'));
        if c.is_whitespace() && next_is_punct {
            continue;
        }
        out.push(*c);
    }
    out
}

/// Assemble the transcript: text = concatenated token texts, stripped (Python `str.strip`); a word starts at
/// every token whose text begins with whitespace (or the first token) and runs to the next such token.
pub(crate) fn assemble(
    tokens: &[Token],
    vocabulary: &[String],
    seconds_per_frame: f32,
) -> Transcript {
    let mut text = String::new();
    let mut words: Vec<Word> = Vec::new();
    for t in tokens {
        let piece = decode_piece(&vocabulary[t.id]);
        let start_s = t.start_frame as f32 * seconds_per_frame;
        let end_s = start_s + t.duration_frames as f32 * seconds_per_frame;
        text.push_str(&piece);
        match words.last_mut() {
            Some(w) if !piece.starts_with(char::is_whitespace) => {
                w.text.push_str(&piece);
                w.end_s = end_s;
            }
            _ => words.push(Word {
                text: piece.trim_start().to_owned(),
                start_s,
                end_s,
            }),
        }
    }
    words.retain(|w| !w.text.is_empty());
    Transcript {
        text: text.trim().to_owned(),
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(pieces: &[&str]) -> Vec<String> {
        pieces.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn piece_drops_space_before_terminal_punctuation() {
        assert_eq!(decode_piece("▁Hello"), " Hello");
        assert_eq!(decode_piece("▁."), ".");
        assert_eq!(decode_piece("▁?"), "?");
        assert_eq!(decode_piece("▁,"), " ,");
        assert_eq!(decode_piece("s"), "s");
    }

    /// Oracle: parakeet-mlx 0.7.0 `decode([869, 2148, 7870, 7883, 7931, 7956, 8020], vocabulary)` on the v3 config
    /// returned `" it oks.'?!"`; the pieces are those ids' vocabulary entries.
    #[test]
    fn joins_like_parakeet_mlx() {
        let v = vocab(&["▁it", "▁ok", "s", ".", "'", "?", "!"]);
        let toks: Vec<Token> = (0..v.len())
            .map(|id| Token {
                id,
                start_frame: id * 2,
                duration_frames: 1,
            })
            .collect();
        let t = assemble(&toks, &v, 0.08);
        assert_eq!(t.text, "it oks.'?!");
        let words: Vec<_> = t.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(words, ["it", "oks.'?!"]);
        assert!((t.words[1].start_s - 0.16).abs() < 1e-6);
        assert!((t.words[1].end_s - (12.0 * 0.08 + 0.08)).abs() < 1e-6);
    }
}
