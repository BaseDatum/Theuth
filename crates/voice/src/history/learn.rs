//! Correction-learning policy (153 §6.2, pure): which dictated → sent edits
//! are vocabulary, and when a pair is due for a suggestion card.
//!
//! 153-C stub — A3 fills.

use super::Correction;

/// §6.3 step 1: the single-word edits between `heard` (delivered text) and `typed` (sent text).
pub fn candidates(heard: &str, typed: &str) -> Vec<Correction> {
    // 153-C stub — A3 fills (SINGLE_WORD, INFLECTIONS).
    let _ = (heard, typed);
    Vec::new()
}

/// §6.3 step 3: `SUGGEST_AFTER` holds for the pair, no cooldown/never row, session ignores < 3.
pub fn due(heard: &str, typed: &str, history: &[Correction], now_ms: u64) -> bool {
    // 153-C stub — A3 fills.
    let _ = (heard, typed, history, now_ms);
    false
}
