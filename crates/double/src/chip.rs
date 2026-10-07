//! The chip and the answer line as text (126 §10), shared by the TUI and the
//! Slack / Signal / Telegram cards so every surface says the same thing.

use crate::events::{DoubleAnswered, DoublePredicted};

/// Whole seconds until `will_act_at` (rounded up, floored at 0); `None` below L2.
pub fn grace_left_s(p: &DoublePredicted, now_ms: u64) -> Option<u64> {
    let at = chrono::DateTime::parse_from_rfc3339(p.will_act_at.as_deref()?).ok()?;
    let at = u64::try_from(at.timestamp_millis()).unwrap_or(0);
    Some(at.saturating_sub(now_ms).div_ceil(1000))
}

/// The channel line: `double would: Allow (91 %)`, plus
/// ` · answers in 30 s unless you do` while an L2 grace runs.
pub fn would(p: &DoublePredicted, now_ms: u64) -> String {
    let mut s = format!("double would: {} ({:.0} %)", p.choice, p.p * 100.0);
    if let Some(left) = grace_left_s(p, now_ms) {
        s.push_str(&format!(" · answers in {left} s unless you do"));
    }
    s
}

/// The line posted when the double answered: `double answered Allow (approval:…)`.
pub fn answered(a: &DoubleAnswered) -> String {
    format!("double answered {} ({})", a.choice, a.class)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(will_act_at: Option<&str>) -> DoublePredicted {
        DoublePredicted {
            question_id: "q".into(),
            session: None,
            kind: crate::CardKind::Approval,
            class: "approval:shell:mutating:git push:root".into(),
            choice: "Allow".into(),
            index: None,
            p: 0.912,
            probs: vec![],
            rung: if will_act_at.is_some() { 2 } else { 1 },
            will_act_at: will_act_at.map(str::to_owned),
            version: "baseline".into(),
            explain: Default::default(),
        }
    }

    #[test]
    fn lines() {
        let now = u64::try_from(
            chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:12.400Z")
                .unwrap()
                .timestamp_millis(),
        )
        .unwrap();
        assert_eq!(would(&p(None), now), "double would: Allow (91 %)");
        assert_eq!(grace_left_s(&p(None), now), None);
        let l2 = p(Some("2030-01-01T00:00:30Z"));
        assert_eq!(grace_left_s(&l2, now), Some(18));
        assert_eq!(
            would(&l2, now),
            "double would: Allow (91 %) · answers in 18 s unless you do"
        );
        // Past the grace end: 0, never negative; an unparsable stamp is no countdown.
        assert_eq!(grace_left_s(&l2, now + 60_000), Some(0));
        assert_eq!(grace_left_s(&p(Some("soon")), now), None);
        assert_eq!(
            answered(&DoubleAnswered {
                question_id: "q".into(),
                session: None,
                class: "ask:freeform:root".into(),
                choice: "Start now".into(),
                version: "v2".into(),
                undo_until: None,
            }),
            "double answered Start now (ask:freeform:root)"
        );
    }
}
