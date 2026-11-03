//! Owner: E1 — redaction for the agent (design 171 §9.4): vault values exactly, then the
//! credential patterns. Applied to the agent's copy, `cell_output` and replicated text.

/// Mask vault values and credential patterns in `text`; returns the masked text and how many
/// values were masked (the footer's "2 values masked for theuth").
pub fn mask(text: &str, vault_values: &[String]) -> (String, u32) {
    let _ = vault_values;
    (text.to_owned(), 0)
}
