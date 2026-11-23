//! Wall-clock Unix time, once.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock Unix milliseconds; `0` if the clock is before the epoch,
/// saturating at `u64::MAX`.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// [`now_ms`] as `i64`, saturating at `i64::MAX`.
pub fn now_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_variants_are_after_2020_and_agree() {
        let (a, b) = (now_ms(), now_ms_i64());
        assert!(a > 1_577_836_800_000);
        assert!((b - a as i64).abs() < 5_000);
    }
}
