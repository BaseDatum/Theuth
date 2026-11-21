//! 207 §3: the flat Attention list's score — pure. Every item gets a base by
//! its class plus live / blocks / deadline / risk / task / age points, each
//! listed as an [`AttentionReason`]; the list is ordered by the sum
//! ([`order`]). Weights come from `roster.attention.weights`
//! ([`ScoreWeights`]).

use std::cmp::Ordering;

use crate::attention::{
    AttentionItem, AttentionKind, AttentionReason, AttentionSignal, StopOutcome,
};
use crate::config::ScoreWeights;

/// 207 §3.2: the base class of an item (R2, with D2's id-prefix overrides).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseClass {
    /// The item stops an agent until the user acts.
    StopsAgent,
    /// A mesh notice (node, home).
    Mesh,
    /// Yours to do (review, hire, blocked, a paused budget).
    Yours,
    /// An artifact shown to you.
    Shown,
    /// A notice (budget at 80 %, the restart notice).
    Notice,
    /// A session stopped failed or timed out.
    StoppedFailed,
    /// A session stopped otherwise.
    Stopped,
    /// Answered by your double.
    Double,
}

impl BaseClass {
    /// This class's base points.
    #[must_use]
    pub fn points(self, w: &ScoreWeights) -> u32 {
        match self {
            Self::StopsAgent => w.stops_agent,
            Self::Mesh => w.mesh,
            Self::Yours => w.yours,
            Self::Shown => w.shown,
            Self::Notice => w.notice,
            Self::StoppedFailed => w.stopped_failed,
            Self::Stopped => w.stopped,
            Self::Double => w.double,
        }
    }
}

/// 207 §3.2: the class of `item`.
#[must_use]
pub fn base_class(item: &AttentionItem) -> BaseClass {
    use AttentionKind as K;
    use BaseClass::*;
    let id = item.id.as_str();
    match item.kind {
        K::Approval | K::Ask | K::Secret | K::Handoff | K::Decision | K::ContactFirst => StopsAgent,
        K::Resume => {
            if id.starts_with("resume:boot:") {
                Notice
            } else {
                StopsAgent
            }
        }
        K::Node | K::Home => Mesh,
        K::Blocked => {
            if id.starts_with("autoresearch:launch:") {
                StopsAgent
            } else {
                Yours
            }
        }
        K::Review | K::Hire => Yours,
        K::Budget => {
            if paused_budget(item) {
                Yours
            } else {
                Notice
            }
        }
        K::Show => Shown,
        K::Finished => match item.stop.as_ref().map(|s| s.outcome) {
            Some(StopOutcome::Failed | StopOutcome::TimedOut) => StoppedFailed,
            _ => Stopped,
        },
        K::Double => Double,
    }
}

fn paused_budget(item: &AttentionItem) -> bool {
    item.detail
        .as_deref()
        .is_some_and(|d| d.starts_with("paused"))
}

/// 207 §3.2: the `base` reason's text; the first matching row wins.
#[must_use]
pub fn base_text(item: &AttentionItem) -> &'static str {
    use AttentionKind as K;
    let id = item.id.as_str();
    if item.kind == K::Finished {
        return match item.stop.as_ref().map(|s| s.outcome) {
            None | Some(StopOutcome::Finished) => "finished",
            Some(StopOutcome::Failed) => "failed",
            Some(StopOutcome::Cancelled) => "cancelled",
            Some(StopOutcome::TimedOut) => "timed out",
        };
    }
    if id.starts_with("autoresearch:launch:") {
        return "launch waits for your press";
    }
    if id.starts_with("resume:boot:") {
        return "restart notice";
    }
    if id.starts_with("misplaced:") {
        return "agent elsewhere";
    }
    if id.starts_with("contact_roster:") {
        return "mesh alert";
    }
    match item.kind {
        K::Home => "machine offline",
        K::Budget if paused_budget(item) => "budget paused",
        K::Budget => "budget at 80 %",
        K::Approval => "approval",
        K::Ask => "question",
        K::Secret => "secret request",
        K::Handoff => "hand-off",
        K::Decision => "decision",
        K::Resume => "interrupted turn",
        K::ContactFirst => "first contact",
        K::Node => "engine unreachable",
        K::Review => "review",
        K::Hire => "hire",
        K::Blocked => "blocked task",
        K::Show => "shown to you",
        K::Double => "answered by your double",
        K::Finished => "finished",
    }
}

/// 207 §3.3: `1:42`, `12:05`, `1:02:03` for a span in ms (floored to seconds).
#[must_use]
pub fn countdown(ms: u64) -> String {
    let s = ms / 1000;
    let h = s / 3600;
    if h > 0 {
        format!("{h}:{:02}:{:02}", s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// 207 §3.3: deadline points — 0 while at least half the span is left,
/// rising to `deadline_max` at the deadline; `deadline_max` when due or when
/// the span is empty. Rounded half up.
#[must_use]
pub fn deadline_points(start_ms: u64, deadline_ms: u64, now_ms: u64, max: u32) -> u32 {
    if deadline_ms <= start_ms || now_ms >= deadline_ms {
        return max;
    }
    let share = (deadline_ms - now_ms) as f64 / (deadline_ms - start_ms) as f64;
    if share >= 0.5 {
        return 0;
    }
    (f64::from(max) * (1.0 - 2.0 * share) + 0.5).floor() as u32
}

/// 207 §3: score and reasons of `item` at `now_ms`. Pure. Integer arithmetic except the deadline share.
#[must_use]
pub fn score(item: &AttentionItem, w: &ScoreWeights, now_ms: u64) -> (u32, Vec<AttentionReason>) {
    use AttentionKind as K;
    let reason = |signal, points, text: String| AttentionReason {
        signal,
        points,
        text,
    };
    let class = base_class(item);
    let mut reasons = vec![reason(
        AttentionSignal::Base,
        class.points(w),
        base_text(item).to_owned(),
    )];
    // D3: a double's session approval is its own item; a stopped session is not live.
    if item.session_live && !matches!(item.kind, K::Double | K::Finished) && w.live > 0 {
        let text = if class == BaseClass::StopsAgent {
            "agent waiting on you"
        } else {
            "session running"
        };
        reasons.push(reason(AttentionSignal::Live, w.live, text.to_owned()));
    }
    if item.blocks > 0 {
        let points = item.blocks.saturating_mul(w.blocks_each).min(w.blocks_max);
        let text = if item.blocks == 1 {
            "blocks 1 agent".to_owned()
        } else {
            format!("blocks {} agents", item.blocks)
        };
        if points > 0 {
            reasons.push(reason(AttentionSignal::Blocks, points, text));
        }
    }
    if let Some(d) = item.deadline_ms {
        let start = item.waiting_since_ms.unwrap_or(item.at_ms);
        let points = deadline_points(start, d, now_ms, w.deadline_max);
        if points > 0 {
            let text = if now_ms >= d {
                "due now".to_owned()
            } else {
                let verb = match item.kind {
                    K::Approval => "denies itself in",
                    K::Double => "double answers in",
                    _ => "times out in",
                };
                format!("{verb} {}", countdown(d - now_ms))
            };
            reasons.push(reason(AttentionSignal::Deadline, points, text));
        }
    }
    match item.risk {
        Some(tools::Risk::Destructive) if w.destructive > 0 => {
            reasons.push(reason(
                AttentionSignal::Risk,
                w.destructive,
                "destructive".into(),
            ));
        }
        Some(tools::Risk::Mutating) if w.mutating > 0 => {
            reasons.push(reason(AttentionSignal::Risk, w.mutating, "mutating".into()));
        }
        _ => {}
    }
    // D4: only items with a task priority; `rank` = project step × 100 + weight.
    if let Some(p) = item.priority {
        let ranked = item.rank >= 100;
        let points = w.task_max.min(
            (item.rank % 100) * w.task_per_weight + if ranked { w.task_ranked_project } else { 0 },
        );
        if points > 0 {
            let mut text = format!("{} priority", p.as_str());
            if ranked {
                text.push_str(" \u{b7} ranked project");
            }
            reasons.push(reason(AttentionSignal::Task, points, text));
        }
    }
    let since = item.waiting_since_ms.unwrap_or(item.at_ms);
    let mins = now_ms.saturating_sub(since) / 60_000;
    let points = u32::try_from(mins.saturating_mul(u64::from(w.age_per_min)))
        .unwrap_or(u32::MAX)
        .min(w.age_max);
    if points > 0 {
        let span = if mins >= 60 {
            format!("{} h", mins / 60)
        } else {
            format!("{mins} min")
        };
        let text = if item.waiting_since_ms.is_some() {
            format!("waiting {span}")
        } else if item.kind == K::Finished {
            format!("stopped {span} ago")
        } else {
            format!("{span} ago")
        };
        reasons.push(reason(AttentionSignal::Age, points, text));
    }
    // Points desc, ties in signal declaration order (the push order above is that order).
    reasons.sort_by(|a, b| {
        b.points
            .cmp(&a.points)
            .then((a.signal as u8).cmp(&(b.signal as u8)))
    });
    let total = reasons.iter().map(|r| r.points).sum();
    (total, reasons)
}

/// 207 §3.4: `score` desc → `waiting_since_ms` asc (`None` last) → `at_ms` desc → `id` asc.
pub fn order(items: &mut [AttentionItem]) {
    items.sort_by(cmp);
}

/// 207 §3.4: the list's total order.
#[must_use]
pub fn cmp(a: &AttentionItem, b: &AttentionItem) -> Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| match (a.waiting_since_ms, b.waiting_since_ms) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| b.at_ms.cmp(&a.at_ms))
        .then_with(|| a.id.as_bytes().cmp(b.id.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attention::AttentionStop;
    use AttentionKind as K;
    use AttentionSignal as S;

    /// Any instant; the fixture is relative to it.
    const NOW: u64 = 1_795_269_600_000;

    fn w() -> ScoreWeights {
        ScoreWeights::default()
    }

    fn it(id: &str, kind: AttentionKind, ago_ms: u64) -> AttentionItem {
        AttentionItem::new(id.into(), kind, "t".into(), None, NOW - ago_ms)
    }

    fn signals(r: &[AttentionReason]) -> Vec<(AttentionSignal, u32)> {
        r.iter().map(|r| (r.signal, r.points)).collect()
    }

    #[test]
    fn base_only_for_a_fresh_item() {
        let (s, r) = score(&it("approval:1", K::Approval, 0), &w(), NOW);
        assert_eq!(s, 600);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].signal, r[0].text.as_str()), (S::Base, "approval"));
    }

    #[test]
    fn each_signal_and_its_text() {
        let mut i = it("approval:1", K::Approval, 180_000);
        i.session_live = true;
        i.blocks = 2;
        i.deadline_ms = Some(NOW + 120_000);
        i.risk = Some(tools::Risk::Destructive);
        i.priority = Some(tasks::Priority::High);
        i.rank = 107;
        let (s, r) = score(&i, &w(), NOW);
        assert_eq!(s, 1213);
        let texts: Vec<(AttentionSignal, u32, &str)> = r
            .iter()
            .map(|r| (r.signal, r.points, r.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                (S::Base, 600, "approval"),
                (S::Live, 300, "agent waiting on you"),
                (S::Risk, 100, "destructive"),
                (S::Blocks, 80, "blocks 2 agents"),
                (S::Task, 80, "high priority \u{b7} ranked project"),
                (S::Deadline, 50, "denies itself in 2:00"),
                (S::Age, 3, "waiting 3 min"),
            ]
        );
        // A mutating risk, one blocker, a show's live text.
        let mut m = it("show:c", K::Show, 0);
        m.session_live = true;
        m.blocks = 1;
        m.risk = Some(tools::Risk::Mutating);
        let (_, r) = score(&m, &w(), NOW);
        let texts: Vec<&str> = r.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "session running",
                "shown to you",
                "blocks 1 agent",
                "mutating"
            ]
        );
        // Read-only risk adds nothing.
        let mut ro = it("approval:2", K::Approval, 0);
        ro.risk = Some(tools::Risk::ReadOnly);
        assert_eq!(score(&ro, &w(), NOW).0, 600);
    }

    #[test]
    fn caps() {
        let mut i = it("blocked:t", K::Blocked, 90 * 60_000);
        i.blocks = 6;
        i.priority = Some(tasks::Priority::Urgent);
        i.rank = 209;
        let (_, r) = score(&i, &w(), NOW);
        assert_eq!(
            signals(&r),
            [
                (S::Base, 300),
                (S::Blocks, 200),
                (S::Task, 100),
                (S::Age, 60)
            ]
        );
        assert_eq!(r[3].text, "waiting 1 h");
    }

    #[test]
    fn deadline_rounding() {
        let max = 250;
        // share 0.5 → 0; 0.4 → 50; 0.0 (due) → 250; passed → 250; zero span → 250.
        assert_eq!(deadline_points(0, 1000, 500, max), 0);
        assert_eq!(deadline_points(0, 1000, 600, max), 50);
        assert_eq!(deadline_points(0, 1000, 1000, max), 250);
        assert_eq!(deadline_points(0, 1000, 2000, max), 250);
        assert_eq!(deadline_points(1000, 1000, 10, max), 250);
        assert_eq!(deadline_points(2000, 1000, 10, max), 250);
        // Half up: 250 × (1 − 2 × 7/15) = 16.67 → 17; 250 × 0.002 = 0.5 → 1.
        assert_eq!(deadline_points(0, 30_000, 16_000, max), 17);
        assert_eq!(deadline_points(0, 1000, 501, max), 1);
        let mut i = it("ask:1", K::Ask, 60_000);
        i.deadline_ms = Some(NOW - 1);
        let (_, r) = score(&i, &w(), NOW);
        assert_eq!(
            r.iter().find(|r| r.signal == S::Deadline).unwrap().text,
            "due now"
        );
        assert_eq!(countdown(3_723_000), "1:02:03");
        assert_eq!(countdown(725_999), "12:05");
    }

    #[test]
    fn d2_overrides() {
        assert_eq!(
            base_class(&it("autoresearch:launch:x:1", K::Blocked, 0)),
            BaseClass::StopsAgent
        );
        assert_eq!(
            base_class(&it("autoresearch:paused:x:b", K::Blocked, 0)),
            BaseClass::Yours
        );
        assert_eq!(
            base_class(&it("resume:boot:5", K::Resume, 0)),
            BaseClass::Notice
        );
        assert_eq!(
            base_class(&it("resume:s:t", K::Resume, 0)),
            BaseClass::StopsAgent
        );
        let mut b = it("budget:a", K::Budget, 0);
        b.detail = Some("at 80 %: the cap pauses it".into());
        assert_eq!(
            (base_class(&b), base_text(&b)),
            (BaseClass::Notice, "budget at 80 %")
        );
        b.detail = Some("paused at the cap".into());
        assert_eq!(
            (base_class(&b), base_text(&b)),
            (BaseClass::Yours, "budget paused")
        );
        assert_eq!(base_text(&it("misplaced:t", K::Home, 0)), "agent elsewhere");
        assert_eq!(base_text(&it("node:e", K::Home, 0)), "machine offline");
        assert_eq!(base_text(&it("contact_roster:r", K::Node, 0)), "mesh alert");
    }

    #[test]
    fn d3_double_and_finished_take_no_live_points() {
        let mut d = it("double:1", K::Double, 0);
        d.session_live = true;
        d.waiting_since_ms = None;
        assert_eq!(score(&d, &w(), NOW).0, 20);
        let mut f = it("finished:s:t", K::Finished, 0);
        f.session_live = true;
        f.waiting_since_ms = None;
        f.stop = Some(AttentionStop {
            outcome: StopOutcome::TimedOut,
            reason: "timed_out".into(),
            error: None,
        });
        let (s, r) = score(&f, &w(), NOW);
        assert_eq!((s, r[0].text.as_str()), (150, "timed out"));
        f.stop = None;
        assert_eq!(
            score(&f, &w(), NOW),
            (
                80,
                vec![AttentionReason {
                    signal: S::Base,
                    points: 80,
                    text: "finished".into(),
                }]
            )
        );
    }

    #[test]
    fn d4_no_priority_no_task_points() {
        let mut i = it("show:c", K::Show, 0);
        i.rank = 209;
        assert_eq!(score(&i, &w(), NOW).0, 120);
    }

    #[test]
    fn weights_come_from_config() {
        let weights = ScoreWeights {
            stops_agent: 1,
            age_max: 5,
            ..ScoreWeights::default()
        };
        assert_eq!(score(&it("ask:1", K::Ask, 3_600_000), &weights, NOW).0, 6);
    }

    /// One fixture row with its expected score and `(signal, points)` reasons.
    type Expected = (Row, u32, Vec<(AttentionSignal, u32)>);

    /// 207 contract §9: one fixture row — the fields `score` reads.
    struct Row {
        id: String,
        kind: AttentionKind,
        at: u64,
        waiting: bool,
        detail: Option<&'static str>,
        deadline: Option<i64>,
        priority: Option<tasks::Priority>,
        rank: u32,
        risk: Option<tools::Risk>,
        blocks: u32,
        live: bool,
        stop: Option<StopOutcome>,
    }

    fn row(id: impl Into<String>, kind: AttentionKind, at: u64) -> Row {
        Row {
            id: id.into(),
            kind,
            at,
            waiting: true,
            detail: None,
            deadline: None,
            priority: None,
            rank: 5,
            risk: None,
            blocks: 0,
            live: false,
            stop: None,
        }
    }

    impl Row {
        fn item(&self) -> AttentionItem {
            let mut i = AttentionItem::new(
                self.id.clone(),
                self.kind,
                "t".into(),
                self.detail.map(str::to_owned),
                NOW - self.at,
            );
            if !self.waiting {
                i.waiting_since_ms = None;
            }
            i.deadline_ms = self.deadline.map(|d| (NOW as i64 + d) as u64);
            i.priority = self.priority;
            i.rank = self.rank;
            i.risk = self.risk;
            i.blocks = self.blocks;
            i.session_live = self.live;
            i.stop = self.stop.map(|outcome| AttentionStop {
                outcome,
                reason: String::new(),
                error: None,
            });
            i
        }
    }

    /// 207 contract §9.2: the 34-item fixture's scores, reasons and order.
    #[test]
    fn fixture_scores_and_order() {
        use tasks::Priority as P;
        use tools::Risk;
        let talk = "019a2070-0000-7000-8000-000000000004";
        let migrate = "019a2070-0000-7000-8000-000000000008";
        let changelog = "019a2070-0000-7000-8000-000000000010";
        let scratch = "019a2070-0000-7000-8000-000000000009";
        let digest = "019a2070-0000-7000-8000-000000000007";
        let live = |mut r: Row| {
            r.live = true;
            r
        };
        #[rustfmt::skip]
        let table: Vec<Expected> = vec![
            (Row { risk: Some(Risk::Destructive), deadline: Some(120_000), priority: Some(P::High), rank: 107, blocks: 2, ..live(row("approval:019a2072-0000-7000-8000-000000000001", K::Approval, 180_000)) },
             1213, vec![(S::Base, 600), (S::Live, 300), (S::Risk, 100), (S::Blocks, 80), (S::Task, 80), (S::Deadline, 50), (S::Age, 3)]),
            (Row { deadline: Some(300_000), priority: Some(P::High), rank: 107, blocks: 1, ..live(row("ask:019a2073-0000-7000-8000-000000000002", K::Ask, 1_500_000)) },
             1212, vec![(S::Base, 600), (S::Live, 300), (S::Deadline, 167), (S::Task, 80), (S::Blocks, 40), (S::Age, 25)]),
            (Row { priority: Some(P::Urgent), rank: 209, blocks: 3, ..live(row("decision:019a2071-0000-7000-8000-000000000001:decision:019a2074-0000-7000-8000-000000000001", K::Decision, 60_000)) },
             1121, vec![(S::Base, 600), (S::Live, 300), (S::Blocks, 120), (S::Task, 100), (S::Age, 1)]),
            (Row { risk: Some(Risk::Mutating), deadline: Some(60_000), ..live(row("approval:019a2072-0000-7000-8000-000000000004", K::Approval, 240_000)) },
             1084, vec![(S::Base, 600), (S::Live, 300), (S::Deadline, 150), (S::Risk, 30), (S::Age, 4)]),
            (Row { risk: Some(Risk::Mutating), deadline: Some(180_000), ..live(row("approval:019a2072-0000-7000-8000-000000000003", K::Approval, 120_000)) },
             932, vec![(S::Base, 600), (S::Live, 300), (S::Risk, 30), (S::Age, 2)]),
            (Row { risk: Some(Risk::Mutating), deadline: Some(240_000), ..live(row("approval:019a2072-0000-7000-8000-000000000002", K::Approval, 60_000)) },
             931, vec![(S::Base, 600), (S::Live, 300), (S::Risk, 30), (S::Age, 1)]),
            (Row { risk: Some(Risk::Mutating), deadline: Some(270_000), ..live(row("approval:019a2072-0000-7000-8000-000000000005", K::Approval, 30_000)) },
             930, vec![(S::Base, 600), (S::Live, 300), (S::Risk, 30)]),
            (Row { deadline: Some(1_200_000), ..live(row("ask:019a2073-0000-7000-8000-000000000001", K::Ask, 600_000)) },
             910, vec![(S::Base, 600), (S::Live, 300), (S::Age, 10)]),
            (Row { deadline: Some(1_440_000), ..live(row("handoff:019a2076-0000-7000-8000-000000000001", K::Handoff, 360_000)) },
             906, vec![(S::Base, 600), (S::Live, 300), (S::Age, 6)]),
            (Row { deadline: Some(1_680_000), ..live(row("secret:019a2075-0000-7000-8000-000000000001", K::Secret, 120_000)) },
             902, vec![(S::Base, 600), (S::Live, 300), (S::Age, 2)]),
            (Row { deadline: Some(120_000), ..row("secret:019a2075-0000-7000-8000-000000000002", K::Secret, 1_680_000) },
             845, vec![(S::Base, 600), (S::Deadline, 217), (S::Age, 28)]),
            (Row { priority: Some(P::Medium), ..row("decision:019a2071-0000-7000-8000-000000000002:decision:019a2074-0000-7000-8000-000000000002", K::Decision, 2_700_000) },
             695, vec![(S::Base, 600), (S::Task, 50), (S::Age, 45)]),
            (row(format!("resume:{talk}:019a2077-0000-7000-8000-000000000001"), K::Resume, 1_200_000),
             620, vec![(S::Base, 600), (S::Age, 20)]),
            (row("autoresearch:launch:blurb-tone:3", K::Blocked, 720_000),
             612, vec![(S::Base, 600), (S::Age, 12)]),
            (row("contact_first:maria:calendar", K::ContactFirst, 420_000),
             607, vec![(S::Base, 600), (S::Age, 7)]),
            (row("node:engine-studio-0002", K::Node, 10_800_000),
             510, vec![(S::Base, 450), (S::Age, 60)]),
            (Row { priority: Some(P::Medium), ..row("misplaced:019a2071-0000-7000-8000-000000000003", K::Home, 300_000) },
             505, vec![(S::Base, 450), (S::Task, 50), (S::Age, 5)]),
            (Row { priority: Some(P::High), rank: 107, ..live(row("show:toolu_01AZQL17aTdoAMD49A3uZkDa", K::Show, 240_000)) },
             504, vec![(S::Live, 300), (S::Base, 120), (S::Task, 80), (S::Age, 4)]),
            (row("node:engine-lab-0001", K::Home, 1_800_000),
             480, vec![(S::Base, 450), (S::Age, 30)]),
            (row("contact_roster:rec-7f3a", K::Node, 120_000),
             452, vec![(S::Base, 450), (S::Age, 2)]),
            (Row { priority: Some(P::Medium), ..row("blocked:019a2071-0000-7000-8000-000000000005", K::Blocked, 5_400_000) },
             410, vec![(S::Base, 300), (S::Age, 60), (S::Task, 50)]),
            (Row { priority: Some(P::High), rank: 207, ..row("review:019a2071-0000-7000-8000-000000000004", K::Review, 1_800_000) },
             410, vec![(S::Base, 300), (S::Task, 80), (S::Age, 30)]),
            (row("hire:summarizer", K::Hire, 7_200_000),
             360, vec![(S::Base, 300), (S::Age, 60)]),
            (Row { detail: Some("paused at the cap; un-pause to override for the month"), ..row("budget:research", K::Budget, 900_000) },
             315, vec![(S::Base, 300), (S::Age, 15)]),
            (row("autoresearch:paused:blurb-tone:budget", K::Blocked, 480_000),
             308, vec![(S::Base, 300), (S::Age, 8)]),
            (Row { waiting: false, priority: Some(P::Urgent), rank: 209, stop: Some(StopOutcome::Failed), ..row(format!("finished:{migrate}:019a2078-0000-7000-8000-000000000002"), K::Finished, 300_000) },
             255, vec![(S::Base, 150), (S::Task, 100), (S::Age, 5)]),
            (Row { waiting: false, stop: Some(StopOutcome::TimedOut), ..row(format!("finished:{changelog}:019a2078-0000-7000-8000-000000000004"), K::Finished, 2_400_000) },
             190, vec![(S::Base, 150), (S::Age, 40)]),
            (Row { detail: Some("at 80 %: the cap pauses it"), ..row("budget:analyst", K::Budget, 12_000_000) },
             180, vec![(S::Base, 120), (S::Age, 60)]),
            (row("show:toolu_01AZQL17aTdoAMD49A3uZkDb", K::Show, 2_400_000),
             160, vec![(S::Base, 120), (S::Age, 40)]),
            (row(format!("resume:boot:{}", NOW - 180_000), K::Resume, 180_000),
             123, vec![(S::Base, 120), (S::Age, 3)]),
            (Row { waiting: false, stop: Some(StopOutcome::Cancelled), ..row(format!("finished:{scratch}:019a2078-0000-7000-8000-000000000003"), K::Finished, 1_200_000) },
             100, vec![(S::Base, 80), (S::Age, 20)]),
            (Row { waiting: false, stop: Some(StopOutcome::Finished), ..row(format!("finished:{digest}:019a2078-0000-7000-8000-000000000001"), K::Finished, 600_000) },
             90, vec![(S::Base, 80), (S::Age, 10)]),
            (Row { waiting: false, ..row("double:019a2079-0000-7000-8000-000000000002", K::Double, 3_000_000) },
             70, vec![(S::Age, 50), (S::Base, 20)]),
            (Row { waiting: false, deadline: Some(14_000), ..live(row("double:019a2079-0000-7000-8000-000000000001", K::Double, 16_000)) },
             37, vec![(S::Base, 20), (S::Deadline, 17)]),
        ];
        assert_eq!(table.len(), 34);
        let mut items = Vec::new();
        for (r, want_score, want) in &table {
            let mut i = r.item();
            let (s, reasons) = score(&i, &w(), NOW);
            assert_eq!(
                (s, signals(&reasons)),
                (*want_score, want.clone()),
                "{}",
                r.id
            );
            i.score = s;
            i.reasons = reasons;
            items.push(i);
        }
        let expected: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
        items.reverse();
        order(&mut items);
        let got: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
        assert_eq!(got, expected);
        // A few texts of the fixture.
        let text = |id: &str, sig: AttentionSignal| {
            items
                .iter()
                .find(|i| i.id == id)
                .and_then(|i| i.reasons.iter().find(|r| r.signal == sig))
                .map(|r| r.text.clone())
                .unwrap()
        };
        assert_eq!(
            text("ask:019a2073-0000-7000-8000-000000000002", S::Deadline),
            "times out in 5:00"
        );
        assert_eq!(text("node:engine-studio-0002", S::Age), "waiting 3 h");
        assert_eq!(
            text("double:019a2079-0000-7000-8000-000000000002", S::Age),
            "50 min ago"
        );
        assert_eq!(
            text("double:019a2079-0000-7000-8000-000000000001", S::Deadline),
            "double answers in 0:14"
        );
        assert_eq!(
            text(
                &format!("finished:{migrate}:019a2078-0000-7000-8000-000000000002"),
                S::Age
            ),
            "stopped 5 min ago"
        );
        assert_eq!(
            text(
                &format!("finished:{migrate}:019a2078-0000-7000-8000-000000000002"),
                S::Base
            ),
            "failed"
        );
    }
}
