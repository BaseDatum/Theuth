//! The pure matcher (153 §5.4): candidates by exact spoken form and Double
//! Metaphone, scored with normalised Levenshtein, accepted against the W-k
//! thresholds in the parent module.
//!
//! 153-C stub — A2 fills.

/// §5.4 step 2: `1 − lev(norm(a), norm(b)) / max(len)`, plus 0.1 for a phonetic hit within `EDIT_MAX`.
pub fn score(a: &str, b: &str) -> f32 {
    // 153-C stub — A2 fills.
    let _ = (a, b);
    0.0
}
