//! 67 §6.4: monthly budgets. The pure parts: the month key, the 80 % / 100 %
//! thresholds, and the per-session cost delta tracker. The actor feeds them
//! from `agent::SessionCostQuery` and writes the `budget` table.

use std::collections::HashMap;

use runtime::SessionId;

use crate::types::Budget;

/// `YYYY-MM` (UTC) of `now_ms`.
pub fn month(now_ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(now_ms as i64)
        .map(|t| t.format("%Y-%m").to_string())
        .unwrap_or_default()
}

/// Which line the month's spend just crossed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threshold {
    /// 80 % of the cap: a task for the manager.
    Warn,
    /// The cap: the agent is paused until the manager overrides.
    Cap,
}

/// The threshold crossed by moving the month's spend from `before` to
/// `after`; `Cap` wins when both are crossed at once. `Budget::None` never.
pub fn crossed(budget: &Budget, before: f64, after: f64) -> Option<Threshold> {
    let Budget::Monthly { amount, .. } = budget else {
        return None;
    };
    let warn = amount * 0.8;
    if after >= *amount && before < *amount {
        Some(Threshold::Cap)
    } else if after >= warn && before < warn {
        Some(Threshold::Warn)
    } else {
        None
    }
}

/// Session → the total last seen, so each `ModelUsageChanged` adds only the
/// new spend. Totals only grow; a smaller one (a reset) adds nothing.
#[derive(Debug, Default)]
pub struct Tracker {
    last: HashMap<SessionId, f64>,
}

impl Tracker {
    /// `max(0, total_now - last)`; remembers `total_now`.
    pub fn delta(&mut self, session: SessionId, total_now: f64) -> f64 {
        let last = self.last.insert(session, total_now).unwrap_or(0.0);
        (total_now - last).max(0.0)
    }

    /// The session is gone.
    pub fn forget(&mut self, session: SessionId) {
        self.last.remove(&session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monthly(amount: f64) -> Budget {
        Budget::Monthly {
            amount,
            currency: "USD".into(),
        }
    }

    #[test]
    fn warn_once_cap_once_none_never() {
        let b = monthly(10.0);
        assert_eq!(crossed(&b, 0.0, 7.9), None);
        assert_eq!(crossed(&b, 7.9, 8.0), Some(Threshold::Warn));
        assert_eq!(crossed(&b, 8.0, 9.9), None, "warned already");
        assert_eq!(crossed(&b, 9.9, 10.0), Some(Threshold::Cap));
        assert_eq!(crossed(&b, 10.0, 12.0), None, "capped already");
        assert_eq!(crossed(&b, 0.0, 11.0), Some(Threshold::Cap), "one jump past both: cap");
        assert_eq!(crossed(&Budget::None, 0.0, 1e9), None);
    }

    #[test]
    fn tracker_is_monotone() {
        let mut t = Tracker::default();
        let s = SessionId::new();
        assert_eq!(t.delta(s, 1.5), 1.5);
        assert_eq!(t.delta(s, 2.0), 0.5);
        assert_eq!(t.delta(s, 2.0), 0.0);
        assert_eq!(t.delta(s, 1.0), 0.0, "a smaller total adds nothing");
        assert_eq!(t.delta(s, 1.5), 0.5, "…and becomes the new base");
        assert_eq!(t.delta(SessionId::new(), 3.0), 3.0);
    }

    #[test]
    fn month_is_utc_year_dash_month() {
        assert_eq!(month(0), "1970-01");
        assert_eq!(month(1_704_067_200_000), "2024-01"); // 2024-01-01T00:00:00Z
        assert_eq!(month(1_704_067_199_999), "2023-12");
    }
}
