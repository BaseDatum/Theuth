//! Owner: E1 — the run's end (design 171 §7.5–7.6): expect checks on the cleaned text, the
//! summary, `after` arming.

use crate::types::{Armed, CellRun, Expect, ExpectResult};

/// §7.5: each item met or missed (plain = substring, `/re/` = multi-line regex).
pub fn check_expect(text: &str, items: &[Expect]) -> Vec<ExpectResult> {
    let _ = (text, items);
    Vec::new()
}

/// §7.5: plan summary (`~3 update · 0 replace · 0 delete`), else the met expect lines, else
/// the last non-empty line; ≤ 80 chars.
pub fn summary(text: &str, expect: &[ExpectResult]) -> Option<String> {
    let _ = (text, expect);
    None
}

/// §7.6: `Some` when `gate` (the named cell's latest run) succeeded, ran `current_code_sha`,
/// and ended within `max_age_mins` of `now_ms`.
pub fn armed(
    gate: &CellRun,
    current_code_sha: &str,
    now_ms: i64,
    max_age_mins: u64,
) -> Option<Armed> {
    let _ = (gate, current_code_sha, now_ms, max_age_mins);
    None
}
