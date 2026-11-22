//! Design doc 155 §7 (D6): the engine decides *whether* an attention item
//! should alert Sky; clients decide *how*. Pure per-kind classing
//! ([`class_of`], [`why_of`]) and the per-episode bookkeeping ([`Episodes`])
//! that gives every alert a stable id and applies `roster.alert_delay_ms`
//! (the item must still be there when the delay has passed). Focus
//! suppression and the per-connection finished-seen set are rpc's (§7.3,
//! §7.5): nothing here knows a client.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::attention::{AttentionItem, AttentionKind};

/// Why a client should notify (155 §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AlertClass {
    /// Only Sky can move it: an approval, a question, a review, …
    NeedsYou,
    /// A session finished while unseen (done ≠ idle, per connection).
    Finished,
}

/// The alert on one [`AttentionItem`] (155 §7.1): `id` is stable for one
/// episode (`<item id>@<episode start ms>`); clients post one notification
/// per id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Alert {
    /// `<item id>@<at_ms>`.
    pub id: String,
    /// What it is.
    pub class: AlertClass,
    /// What triggered it, one short sentence.
    pub why: String,
    /// ms since epoch: when the episode began.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
}

/// The §7.2 table: the class an item of this kind alerts with, or none.
/// `resume:boot:*` is a banner, not a need. 207 §6: home items are
/// `node:<engine>` / `misplaced:<task>`; a study's items (`autoresearch:*`)
/// alert as review / blocked.
#[must_use]
pub fn class_of(item: &AttentionItem) -> Option<AlertClass> {
    use AttentionKind as K;
    let own = |prefix: &str| item.id.starts_with(prefix);
    let needs_you = match item.kind {
        K::Approval => own("approval:"),
        K::Decision => own("decision:"),
        K::Hire => own("hire:"),
        // 207 E3: a decision left to the agent is a quiet notice, not a need.
        K::Blocked => {
            (own("blocked:") && !crate::score::left_decision(item)) || own("autoresearch:")
        }
        K::Review => own("review:") || own("autoresearch:"),
        K::Handoff => own("handoff:"),
        K::Secret => own("secret:"),
        K::Ask => own("ask:"),
        K::Home => own("node:") || own("misplaced:"),
        K::ContactFirst => own("contact_first:"),
        K::Resume => own("resume:") && !own("resume:boot:"),
        K::Budget => {
            own("budget:")
                && item
                    .detail
                    .as_deref()
                    .is_some_and(|d| d.starts_with("paused"))
        }
        K::Finished => return own("finished:").then_some(AlertClass::Finished),
        K::Node | K::Show | K::WebView | K::Double => false,
    };
    needs_you.then_some(AlertClass::NeedsYou)
}

/// The §7.2 table: the one-sentence `why` for an item.
#[must_use]
pub fn why_of(item: &AttentionItem) -> String {
    use AttentionKind as K;
    if item.id.starts_with("autoresearch:") {
        return "a study waits for you".into();
    }
    let first_line = |s: Option<&str>| {
        s.and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    match item.kind {
        K::Approval => {
            let tool = item
                .title
                .rsplit_once(" wants to run ")
                .map_or(item.title.as_str(), |(_, t)| t);
            format!("waiting for allow / deny on `{tool}`")
        }
        K::Decision => "a task is blocked on your decision".into(),
        K::Hire => "a proposed agent waits for hire / decline".into(),
        K::Blocked => "a task is blocked on you".into(),
        K::Review => "a task waits for your review".into(),
        K::Handoff => "an agent in a VM needs you at its desktop".into(),
        K::Secret => "a session asked for a secret".into(),
        K::Ask => "a session asked you a question".into(),
        K::Resume => "an interrupted turn needs the Resume button".into(),
        K::Home => "a machine is offline with tasks waiting on it".into(),
        K::ContactFirst => "a contact waits for you to allow it".into(),
        K::Budget => "an agent is paused at its budget cap".into(),
        K::Finished => {
            use crate::attention::StopOutcome as O;
            let stop = item.stop.as_ref();
            match stop.map(|s| s.outcome) {
                Some(O::Failed) => {
                    match first_line(
                        stop.and_then(|s| s.error.as_deref())
                            .or(item.summary.as_deref()),
                    ) {
                        e if e.is_empty() => "turn failed".into(),
                        e => format!("turn failed: {e}"),
                    }
                }
                Some(O::Cancelled) => "turn cancelled".into(),
                Some(O::TimedOut) => "turn timed out".into(),
                Some(O::Finished) | None => match first_line(item.summary.as_deref()) {
                    s if s.is_empty() => "turn ended".into(),
                    s => format!("turn ended: {s}"),
                },
            }
        }
        K::Node | K::Show | K::WebView | K::Double => String::new(),
    }
}

/// Item id → when its current episode began (155 §7.4). Lives on the
/// `Attention` actor; `retain` after every derive.
#[derive(Debug, Default)]
pub struct Episodes {
    start_ms: HashMap<String, u64>,
}

impl Episodes {
    /// The alert for `item` now: `None` for kinds that never alert and
    /// while the episode is younger than `delay_ms`.
    pub fn alert(&mut self, item: &AttentionItem, now_ms: u64, delay_ms: u64) -> Option<Alert> {
        let class = class_of(item)?;
        let start = *self.start_ms.entry(item.id.clone()).or_insert(now_ms);
        (now_ms >= start.saturating_add(delay_ms)).then(|| Alert {
            id: format!("{}@{start}", item.id),
            class,
            why: why_of(item),
            at_ms: start,
        })
    }

    /// Forget episodes of items no longer in the list.
    pub fn retain<'a>(&mut self, live: impl Iterator<Item = &'a str>) {
        let live: std::collections::HashSet<&str> = live.collect();
        self.start_ms.retain(|id, _| live.contains(id.as_str()));
    }

    /// The earliest `start + delay_ms` still in the future: when to recheck.
    #[must_use]
    pub fn next_due(&self, now_ms: u64, delay_ms: u64) -> Option<u64> {
        self.start_ms
            .values()
            .map(|s| s + delay_ms)
            .filter(|due| *due > now_ms)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: AttentionKind) -> AttentionItem {
        AttentionItem {
            waiting_since_ms: None,
            ..AttentionItem::new(id.into(), kind, String::new(), None, 0)
        }
    }

    /// §7.2: every kind's class and `why`; external ids, `resume:boot:*`
    /// and an unpaused budget never alert.
    #[test]
    fn class_and_why_for_every_kind() {
        use AlertClass::{Finished, NeedsYou};
        use AttentionKind as K;
        let rows: [(&str, K, Option<AlertClass>, &str); 16] = [
            (
                "approval:1",
                K::Approval,
                Some(NeedsYou),
                "waiting for allow / deny on `shell`",
            ),
            (
                "decision:t:d",
                K::Decision,
                Some(NeedsYou),
                "a task is blocked on your decision",
            ),
            (
                "hire:amy",
                K::Hire,
                Some(NeedsYou),
                "a proposed agent waits for hire / decline",
            ),
            (
                "blocked:t",
                K::Blocked,
                Some(NeedsYou),
                "a task is blocked on you",
            ),
            (
                "review:t",
                K::Review,
                Some(NeedsYou),
                "a task waits for your review",
            ),
            (
                "handoff:h",
                K::Handoff,
                Some(NeedsYou),
                "an agent in a VM needs you at its desktop",
            ),
            (
                "secret:s",
                K::Secret,
                Some(NeedsYou),
                "a session asked for a secret",
            ),
            (
                "ask:a",
                K::Ask,
                Some(NeedsYou),
                "a session asked you a question",
            ),
            (
                "resume:s:t",
                K::Resume,
                Some(NeedsYou),
                "an interrupted turn needs the Resume button",
            ),
            (
                "budget:ops",
                K::Budget,
                Some(NeedsYou),
                "an agent is paused at its budget cap",
            ),
            (
                "finished:s:t",
                K::Finished,
                Some(Finished),
                "turn ended: all green",
            ),
            ("node:n", K::Node, None, ""),
            (
                "node:e",
                K::Home,
                Some(NeedsYou),
                "a machine is offline with tasks waiting on it",
            ),
            (
                "misplaced:t",
                K::Home,
                Some(NeedsYou),
                "a machine is offline with tasks waiting on it",
            ),
            ("show:c", K::Show, None, ""),
            ("double:q", K::Double, None, ""),
        ];
        for (id, kind, class, why) in rows {
            let mut it = item(id, kind);
            it.title = "s wants to run shell".into();
            it.detail = Some("paused at the cap; un-pause to override".into());
            it.summary = Some("\nall green\nsecond line".into());
            assert_eq!(class_of(&it), class, "{kind:?}");
            assert_eq!(why_of(&it), why, "{kind:?}");
        }
        let mut budget = item("budget:ops", K::Budget);
        budget.detail = Some("at 80 %: the cap pauses it".into());
        assert_eq!(class_of(&budget), None, "a budget warning is not a need");
        assert_eq!(class_of(&item("resume:boot:5", K::Resume)), None);
        // 207 §6: a study's items alert as review / blocked, with their own why.
        for (id, kind) in [
            ("autoresearch:finished:x", K::Review),
            ("autoresearch:launch:x:3", K::Blocked),
        ] {
            let it = item(id, kind);
            assert_eq!(class_of(&it), Some(NeedsYou), "{id}");
            assert_eq!(why_of(&it), "a study waits for you");
        }
        let mut finished = item("finished:s:t", K::Finished);
        finished.summary = None;
        assert_eq!(why_of(&finished), "turn ended");
        // 207 §6: the why follows the stop's outcome.
        let stop = |outcome, error: Option<&str>| crate::attention::AttentionStop {
            outcome,
            reason: String::new(),
            error: error.map(str::to_owned),
        };
        use crate::attention::StopOutcome as O;
        finished.stop = Some(stop(O::Failed, Some("rate limited\nretry later")));
        assert_eq!(why_of(&finished), "turn failed: rate limited");
        finished.stop = Some(stop(O::Failed, None));
        assert_eq!(why_of(&finished), "turn failed");
        finished.summary = Some("half done".into());
        assert_eq!(why_of(&finished), "turn failed: half done");
        finished.stop = Some(stop(O::Cancelled, None));
        assert_eq!(why_of(&finished), "turn cancelled");
        finished.stop = Some(stop(O::TimedOut, Some("deadline")));
        assert_eq!(why_of(&finished), "turn timed out");
        finished.stop = Some(stop(O::Finished, None));
        assert_eq!(why_of(&finished), "turn ended: half done");
        assert_eq!(class_of(&finished), Some(Finished));
    }

    /// §7.4: `None` inside the delay, a stable id after it, a new id once the
    /// item left and came back; `next_due` is the earliest open window.
    #[test]
    fn episodes_delay_then_stable_id_then_new_episode() {
        let mut ep = Episodes::default();
        let ask = item("ask:a", AttentionKind::Ask);
        let show = item("show:c", AttentionKind::Show);
        assert_eq!(ep.alert(&ask, 1000, 2000), None);
        assert_eq!(ep.alert(&show, 1000, 2000), None);
        assert_eq!(ep.alert(&ask, 2999, 2000), None, "still inside the delay");
        assert_eq!(ep.next_due(1500, 2000), Some(3000));
        let first = ep.alert(&ask, 3000, 2000).expect("due");
        assert_eq!(first.id, "ask:a@1000");
        assert_eq!((first.class, first.at_ms), (AlertClass::NeedsYou, 1000));
        assert_eq!(first.why, "a session asked you a question");
        assert_eq!(
            ep.alert(&ask, 9000, 2000).unwrap().id,
            "ask:a@1000",
            "stable"
        );
        assert_eq!(ep.next_due(9000, 2000), None, "nothing waits");

        ep.retain(std::iter::empty());
        assert_eq!(
            ep.alert(&ask, 10_000, 2000),
            None,
            "a new episode re-arms the delay"
        );
        assert_eq!(ep.alert(&ask, 12_000, 2000).unwrap().id, "ask:a@10000");

        let later = item("ask:b", AttentionKind::Ask);
        assert_eq!(ep.alert(&later, 12_500, 2000), None);
        assert_eq!(ep.next_due(12_600, 2000), Some(14_500));
        ep.retain(["ask:a"].into_iter());
        assert_eq!(
            ep.next_due(12_600, 2000),
            None,
            "retain dropped the newcomer"
        );
        assert_eq!(
            ep.alert(&ask, 13_000, 0),
            Some(Alert {
                id: "ask:a@10000".into(),
                class: AlertClass::NeedsYou,
                why: why_of(&ask),
                at_ms: 10_000
            })
        );
    }
}
