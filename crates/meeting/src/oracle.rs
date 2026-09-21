//! The decision-oracle pre-filter on transcript batches (57 §3): before a flushed batch
//! spends a meeting-session model turn, the local oracle answers `Bool("worth a brief?")`
//! (family `meeting.topic`) and, when the brief cache has FTS rows, `Choice` over the top-8
//! cached briefs + `none` (`meeting.brief`). Dispositions per §3.2 — `topic=false` → no turn,
//! `brief=<id>` → the cached brief published like an alias hit, anything else → tier 2 (the
//! cheap model through `decision::escalate`) then tier 3 (today's turn, with one evidence line).
//!
//! Sky's switch (§3.4) is the `meeting` prefix's mode: **off** = never asked, **shadow** = asked
//! and shown, today's path still runs and the turn's outcome is reported back, **active** = the
//! gate stands. Everything here runs off the actor's mailbox ([`run`] is a spawned task that
//! `tell`s a [`Verdict`] back); the pure parts ([`settle`], [`evidence`], [`outcome`]) are what
//! the tests cover.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use decision::{Ask, Decision, Disposition, LogId, Mode, Question, Reason};
use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, CachedBrief};
use crate::{BriefId, MeetingEvent, MeetingId, Track, Utterance, session};

/// The config prefix Settings → Meetings switches: `decision.families.meeting.mode`.
pub const PREFIX: &str = "meeting";
/// Family of the `Bool` question.
pub const TOPIC: &str = "meeting.topic";
/// Family of the `Choice` question.
pub const BRIEF: &str = "meeting.brief";
/// Most cached briefs offered as candidates.
pub const CANDIDATES: usize = 8;
/// The `Bool` statement (57 §3.1).
pub const TOPIC_STATEMENT: &str = "The speakers mention a specific ticket, system, person, project, number or term the listener may need to look up";
/// The `Choice` instructions (57 §3.1).
pub const BRIEF_INSTRUCTIONS: &str = "which cached brief is the subject being discussed";

/// Where a batch went (57 §3.2), as the island and the session rows show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OracleDisposition {
    /// `topic=false`: no model turn; the batch rides along as context in the next delivered one.
    Skip,
    /// `brief=<id>`: the cached brief is (would be, in shadow) published directly.
    Brief {
        /// The cache row.
        brief: BriefId,
        /// Its title, for the row.
        title: String,
    },
    /// Tier 3 — today's model turn — with why the oracle did not settle it.
    Model {
        /// `low margin`, `topic true, no cached brief`, `tier 2: timed out`, …
        reason: String,
    },
}

/// What the meeting session's turn did with a shadowed (or escalated) batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OracleOutcome {
    /// The model did what the oracle's answers said (nothing, or the same brief).
    pub agreed: bool,
    /// The title the model published, if it published one.
    pub published: Option<String>,
}

/// One oracle-handled batch on the meeting event stream (57 §3.4 data path). Emitted once when
/// the oracle answers; again with `outcome` after the model turn in shadow (and after a tier-3
/// turn in active).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OracleDecision {
    /// `hh:mm:ss` of the batch's first utterance — the `[meeting hh:mm:ss]` line's stamp, so the
    /// session view attaches the row to its batch card.
    pub batch_id: String,
    /// The family whose answer settled it (`meeting.topic` / `meeting.brief`).
    pub family: String,
    /// The switch position when it ran.
    pub mode: Mode,
    /// Where the batch went.
    pub disposition: OracleDisposition,
    /// Display labels with probabilities: `topic` = P(worth a brief), then every candidate
    /// brief and `none`, oracle order.
    pub top: Vec<(String, f32)>,
    /// Wall clock from the first ask to the verdict (tier 2 included).
    pub latency_ms: u64,
    /// 1 oracle, 2 cheap model, 3 main model turn.
    pub tier: u8,
    /// Filled after the model turn.
    pub outcome: Option<OracleOutcome>,
}

/// What tier 1 (or 1+2) settled for a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    /// `topic=false` accepted.
    Skip,
    /// `brief=<id>` accepted (the candidate id string).
    Brief(String),
    /// Needs the model: `topic=true` with no cached brief.
    Model(String),
    /// Nothing accepted yet: which questions are still open, and the first escalation reason.
    Open {
        /// The topic question escalated.
        topic: bool,
        /// The brief question escalated (never true without candidates).
        brief: bool,
        /// Why (`low margin`, `timeout`, …).
        reason: String,
    },
}

/// The accepted choice of a decision, looking through a shadow wrapper.
pub fn accepted(d: &Decision) -> Option<&str> {
    match &d.disposition {
        Disposition::Accept => Some(&d.choice),
        Disposition::Shadow(inner) => accepted(inner),
        Disposition::Escalate(_) => None,
    }
}

fn reason_text(r: &Reason) -> String {
    match r {
        Reason::LowTop1 => "low top-1".into(),
        Reason::LowMargin => "low margin".into(),
        Reason::HardError(e) => format!("error: {e}"),
        Reason::Truncated => "truncated".into(),
        Reason::Timeout => "timeout".into(),
        Reason::Disabled => "disabled".into(),
        Reason::ModelMissing => "model missing".into(),
    }
}

/// §3.2, mode-independent (shadow is looked through): a brief pick wins, then `topic=false`,
/// then `topic=true` without a brief needs research; anything unaccepted is `Open`.
pub fn settle(topic: &Decision, brief: Option<&Decision>) -> Settled {
    let pick = brief.and_then(accepted);
    if let Some(id) = pick.filter(|id| *id != decision::engine::NONE_ID) {
        return Settled::Brief(id.to_owned());
    }
    match accepted(topic) {
        Some("false") => Settled::Skip,
        Some(_) if brief.is_none() || pick.is_some() => {
            Settled::Model("topic true, no cached brief".into())
        }
        t => Settled::Open {
            topic: t.is_none(),
            brief: brief.is_some() && pick.is_none(),
            reason: topic
                .disposition
                .reason()
                .or_else(|| brief.and_then(|b| b.disposition.reason()))
                .map_or_else(|| "undecided".into(), reason_text),
        },
    }
}

/// P(true) of the topic decision, if it ran.
fn p_true(topic: &Decision) -> Option<f32> {
    topic
        .probabilities
        .iter()
        .find(|(l, _)| l == "true")
        .map(|(_, p)| *p)
}

/// The display labels + probabilities the event carries: `topic` first, then the brief
/// candidates under their titles.
pub fn top(topic: &Decision, brief: Option<&Decision>, titles: &HashMap<String, String>) -> Vec<(String, f32)> {
    let mut v = Vec::new();
    if let Some(p) = p_true(topic) {
        v.push(("topic".to_owned(), p));
    }
    if let Some(b) = brief {
        for (id, p) in &b.probabilities {
            v.push((titles.get(id).cloned().unwrap_or_else(|| id.clone()), *p));
        }
    }
    v
}

/// The tier-3 evidence line (57 §3.2): `oracle: topic 0.62, brief none 0.55 / SRE-2386 0.31
/// (escalated: low margin)`. Data for the model, never an instruction.
pub fn evidence(topic: &Decision, brief: Option<&Decision>, titles: &HashMap<String, String>, why: &str) -> String {
    let mut s = String::from("oracle:");
    match p_true(topic) {
        Some(p) => s.push_str(&format!(" topic {p:.2}")),
        None => s.push_str(" topic ?"),
    }
    if let Some(b) = brief {
        let mut ranked: Vec<&(String, f32)> = b.probabilities.iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let two: Vec<String> = ranked
            .iter()
            .take(2)
            .map(|(id, p)| format!("{} {p:.2}", titles.get(id).map_or(id.as_str(), String::as_str)))
            .collect();
        s.push_str(&format!(", brief {}", two.join(" / ")));
    }
    s.push_str(&format!(" (escalated: {why})"));
    s
}

/// The batch as the oracle reads it: one `them:`/`me:` line per utterance.
pub fn state(batch: &[Utterance]) -> String {
    let mut s = String::new();
    for u in batch {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(match u.track {
            Track::Mic => "me: ",
            Track::System => "them: ",
        });
        s.push_str(u.text.trim());
    }
    s
}

/// The batch's id: its first utterance's `hh:mm:ss`, the `[meeting …]` line's stamp.
pub fn batch_id(batch: &[Utterance]) -> String {
    session::hms(batch.first().map_or(0, |u| u.t0_ms))
}

/// What the oracle expects the model turn to do, for the shadow outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// Log ids to `Report` against: `(family log id, expected label)`. The topic's label is
    /// `true`/`false`; the brief's a candidate id or `none`.
    pub rows: Vec<(LogId, String, String)>,
}

/// After the model turn: `published` = the cached rows (and titles) the turn registered with
/// `meeting_topic`. Per family: `Confirmed` when the label matches what happened, else
/// `Corrected { actual }`; the event's `agreed` is every family confirmed.
pub fn outcome(expected: &Expected, published: &[(Option<BriefId>, String)]) -> (OracleOutcome, Vec<(LogId, decision::Outcome)>) {
    let mut reports = Vec::new();
    let mut agreed = true;
    for (id, family, label) in &expected.rows {
        let actual = if family == TOPIC {
            (!published.is_empty()).to_string()
        } else {
            published
                .iter()
                .find_map(|(row, _)| row.map(|r| r.to_string()))
                .unwrap_or_else(|| decision::engine::NONE_ID.to_owned())
        };
        let o = if *label == actual {
            decision::Outcome::Confirmed
        } else {
            agreed = false;
            decision::Outcome::Corrected { actual }
        };
        reports.push((*id, o));
    }
    (
        OracleOutcome {
            agreed,
            published: published.first().map(|(_, t)| t.clone()),
        },
        reports,
    )
}

/// What the actor does with a batch once the oracle (and tier 2) spoke.
#[derive(Debug)]
pub enum Action {
    /// No model turn; carry the batch into the next delivered one.
    Skip,
    /// Publish the cached brief as an `Oracle` topic; carry the batch.
    Publish {
        /// The row.
        row: Box<CachedBrief>,
        /// The label (its first alias).
        label: String,
    },
    /// Today's path: deliver the batch, with the evidence line when tier 1/2 ran and escalated.
    Model {
        /// Appended to the batch line.
        evidence: Option<String>,
    },
}

/// `tell` from [`run`]: the oracle's verdict on one batch.
#[derive(Debug)]
pub struct Verdict {
    /// The batch itself, to deliver or carry.
    pub batch: Vec<Utterance>,
    /// What to do.
    pub action: Action,
    /// The event already published (`None` when the oracle was off); kept for the outcome update.
    pub event: Option<OracleDecision>,
    /// What to compare the model turn against (shadow, or a tier-3 turn).
    pub expected: Option<Expected>,
}

/// Sends a `SideCall` to the `agent` plugin through the registry (57 §2 / 3b's snippet).
fn side_caller() -> Box<decision::SideCaller> {
    Box::new(|call| {
        Box::pin(async move {
            let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                .ok()
                .flatten()
                .ok_or_else(|| "agent is not running".to_owned())?;
            agents.ask(call).await.map_err(|e| e.to_string())
        })
    })
}

/// One ask on the decision actor; `None` when it is not running.
async fn ask(engine: &ActorRef<decision::Engine>, ask: Ask) -> Option<Decision> {
    engine
        .ask(ask)
        .await
        .map_err(|e| tracing::debug!("meeting: decision ask: {e}"))
        .ok()
}

/// The whole pre-filter for one batch, off the mailbox: mode → the two asks → §3.2 → tier 2
/// when active and open → the [`Verdict`]. `blocked` are the cache rows already shown or
/// dismissed this meeting (never offered again, like the alias scan).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    rt: Runtime,
    cache: Option<Arc<Cache>>,
    blocked: Vec<BriefId>,
    batch: Vec<Utterance>,
    meeting: MeetingId,
    session: SessionId,
) -> Verdict {
    let today = |batch| Verdict {
        batch,
        action: Action::Model { evidence: None },
        event: None,
        expected: None,
    };
    let Ok(Some(engine)) = ActorRef::<decision::Engine>::lookup(decision::DECISION_ACTOR) else {
        return today(batch);
    };
    let Ok(info) = engine
        .ask(decision::GetMode {
            prefix: PREFIX.to_owned(),
        })
        .await
    else {
        return today(batch);
    };
    if info.effective == Mode::Off {
        return today(batch);
    }
    let id = batch_id(&batch);
    rt.publish(MeetingEvent::OracleDeciding {
        meeting,
        batch_id: id.clone(),
    })
    .await;
    let t = Instant::now();
    let text = state(&batch);
    let budget = Duration::from_millis(info.budget_ms);
    let escalate_budget = Duration::from_millis(info.escalate_budget_ms);

    // Candidates: FTS rows not shown/dismissed this meeting, by rank.
    let mut rows: Vec<CachedBrief> = Vec::new();
    if let Some(c) = &cache {
        match c.search(&text, (CANDIDATES * 2) as u32).await {
            Ok(hits) => {
                rows = hits
                    .into_iter()
                    .filter(|r| !blocked.contains(&r.id))
                    .take(CANDIDATES)
                    .collect();
            }
            Err(e) => tracing::debug!("meeting: oracle brief_search: {e:#}"),
        }
    }
    let titles: HashMap<String, String> = rows
        .iter()
        .map(|r| (r.id.to_string(), r.title.clone()))
        .collect();

    let topic_ask = Ask {
        family: TOPIC.into(),
        question: Question::Bool {
            statement: TOPIC_STATEMENT.into(),
            state: text.clone(),
        },
        budget,
    };
    let brief_ask = (!rows.is_empty()).then(|| Ask {
        family: BRIEF.into(),
        question: Question::Choice {
            instructions: BRIEF_INSTRUCTIONS.into(),
            candidates: rows
                .iter()
                .map(|r| decision::Candidate {
                    id: r.id.to_string(),
                    text: format!("{} ({})", r.title, r.aliases.join(", ")),
                })
                .collect(),
            state: text.clone(),
        },
        budget,
    });
    let Some(mut topic) = ask(&engine, topic_ask.clone()).await else {
        return today(batch);
    };
    // The engine's disposition names the mode it actually ran in.
    let mode = match &topic.disposition {
        Disposition::Shadow(_) => Mode::Shadow,
        Disposition::Escalate(Reason::Disabled | Reason::ModelMissing) => return today(batch),
        _ => Mode::Active,
    };
    let mut brief = match &brief_ask {
        Some(a) => ask(&engine, a.clone()).await,
        None => None,
    };

    let mut settled = settle(&topic, brief.as_ref());
    let mut tier = 1;
    let mut why_tier2 = None;
    if mode == Mode::Active
        && let Settled::Open {
            topic: t_open,
            brief: b_open,
            reason,
        } = &settled
    {
        let call = side_caller();
        let mut failed = None;
        if *b_open && let Some(a) = &brief_ask {
            match decision::escalate(a, brief.as_ref(), session, escalate_budget, &*call).await {
                Ok(d) => brief = Some(d),
                Err(e) => failed = Some(e.to_string()),
            }
        }
        if *t_open && failed.is_none() {
            match decision::escalate(&topic_ask, Some(&topic), session, escalate_budget, &*call).await {
                Ok(d) => topic = d,
                Err(e) => failed = Some(e.to_string()),
            }
        }
        tier = 2;
        why_tier2 = Some(match &failed {
            Some(e) => format!("{reason}; tier 2: {e}"),
            None => format!("{reason}; tier 2 answered"),
        });
        settled = settle(&topic, brief.as_ref());
    }

    let (family, disposition, action) = match &settled {
        Settled::Skip => (TOPIC, OracleDisposition::Skip, Action::Skip),
        Settled::Brief(id) => {
            let row = rows.iter().find(|r| r.id.to_string() == *id).cloned();
            match row {
                Some(row) => (
                    BRIEF,
                    OracleDisposition::Brief {
                        brief: row.id,
                        title: row.title.clone(),
                    },
                    Action::Publish {
                        label: row.aliases.first().cloned().unwrap_or_else(|| row.title.clone()),
                        row: Box::new(row),
                    },
                ),
                // The id came from our own table; a miss is a bug, not a pick.
                None => (
                    BRIEF,
                    OracleDisposition::Model {
                        reason: "unknown brief id".into(),
                    },
                    Action::Model { evidence: None },
                ),
            }
        }
        Settled::Model(reason) => {
            let why = why_tier2.clone().map_or_else(|| reason.clone(), |w| format!("{reason}; {w}"));
            (
                TOPIC,
                OracleDisposition::Model { reason: why.clone() },
                Action::Model {
                    evidence: Some(evidence(&topic, brief.as_ref(), &titles, &why)),
                },
            )
        }
        Settled::Open { topic: t_open, reason, .. } => {
            let why = why_tier2.clone().unwrap_or_else(|| reason.clone());
            (
                if *t_open { TOPIC } else { BRIEF },
                OracleDisposition::Model { reason: why.clone() },
                Action::Model {
                    evidence: Some(evidence(&topic, brief.as_ref(), &titles, &why)),
                },
            )
        }
    };
    let expected = Expected {
        rows: std::iter::once((topic.log_id, TOPIC.to_owned(), inner_choice(&topic)))
            .chain(brief.iter().map(|b| (b.log_id, BRIEF.to_owned(), inner_choice(b))))
            .collect(),
    };
    let event = OracleDecision {
        batch_id: id,
        family: family.to_owned(),
        mode,
        disposition,
        top: top(&topic, brief.as_ref(), &titles),
        latency_ms: t.elapsed().as_millis() as u64,
        tier: if matches!(action, Action::Model { .. }) { 3 } else { tier },
        outcome: None,
    };
    rt.publish(MeetingEvent::OracleDecision {
        meeting,
        decision: event.clone(),
    })
    .await;
    // Shadow: today's path exactly — no evidence line, so the outcome is an independent label.
    let action = match (mode, action) {
        (Mode::Shadow, _) => Action::Model { evidence: None },
        (_, a) => a,
    };
    let expected = matches!(action, Action::Model { .. }).then_some(expected);
    Verdict {
        batch,
        action,
        event: Some(event),
        expected,
    }
}

/// The choice through a shadow wrapper (the outer clone carries the same `choice`).
fn inner_choice(d: &Decision) -> String {
    d.choice.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(family: &str, choice: &str, probs: &[(&str, f32)], disp: Disposition) -> Decision {
        Decision {
            family: family.into(),
            choice: choice.into(),
            probabilities: probs.iter().map(|(l, p)| ((*l).to_owned(), *p)).collect(),
            confidence: 0.5,
            act_probability: 0.0,
            disposition: disp,
            latency: Duration::from_millis(80),
            gate_version: 1,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
        }
    }

    fn topic(p_true: f32, disp: Disposition) -> Decision {
        let choice = if p_true >= 0.5 { "true" } else { "false" };
        d(TOPIC, choice, &[("false", 1.0 - p_true), ("true", p_true)], disp)
    }

    #[test]
    fn settle_follows_the_disposition_table() {
        let accept = Disposition::Accept;
        let low = Disposition::Escalate(Reason::LowMargin);
        // topic=false accepted → skip, whatever the brief says.
        assert_eq!(settle(&topic(0.12, accept.clone()), None), Settled::Skip);
        let b_none = d(BRIEF, "none", &[("a", 0.1), ("none", 0.9)], accept.clone());
        assert_eq!(settle(&topic(0.12, accept.clone()), Some(&b_none)), Settled::Skip);
        // brief=<id> accepted → publish, even when the topic question is unsure.
        let b_a = d(BRIEF, "a", &[("a", 0.91), ("none", 0.09)], accept.clone());
        assert_eq!(settle(&topic(0.6, low.clone()), Some(&b_a)), Settled::Brief("a".into()));
        // topic=true accepted, no candidates or brief=none accepted → research (tier 3).
        assert!(matches!(settle(&topic(0.95, accept.clone()), None), Settled::Model(_)));
        assert!(matches!(settle(&topic(0.95, accept.clone()), Some(&b_none)), Settled::Model(_)));
        // Anything escalated → open, naming what is open and why.
        let b_low = d(BRIEF, "a", &[("a", 0.55), ("none", 0.31)], low.clone());
        assert_eq!(
            settle(&topic(0.62, Disposition::Escalate(Reason::LowTop1)), Some(&b_low)),
            Settled::Open {
                topic: true,
                brief: true,
                reason: "low top-1".into()
            }
        );
        assert_eq!(
            settle(&topic(0.95, accept.clone()), Some(&b_low)),
            Settled::Open {
                topic: false,
                brief: true,
                reason: "low margin".into()
            }
        );
        assert_eq!(
            settle(&topic(0.7, Disposition::Escalate(Reason::Timeout)), None),
            Settled::Open {
                topic: true,
                brief: false,
                reason: "timeout".into()
            }
        );
        // Shadow wrappers are looked through.
        assert_eq!(settle(&topic(0.1, accept.clone()).shadowed(), None), Settled::Skip);
        assert_eq!(settle(&topic(0.6, low).shadowed(), Some(&b_a.shadowed())), Settled::Brief("a".into()));
    }

    #[test]
    fn tier2_answers_resettle_and_failures_stay_open() {
        // A tier-2 `Accept` on the brief turns an open batch into a publish; a tier-2 `none`
        // (Escalate) leaves it open; the topic answered false settles it as a skip.
        let low = Disposition::Escalate(Reason::LowMargin);
        let b_low = d(BRIEF, "a", &[("a", 0.55), ("none", 0.31)], low.clone());
        let t_low = topic(0.62, low);
        assert!(matches!(settle(&t_low, Some(&b_low)), Settled::Open { .. }));
        let mut b2 = d(BRIEF, "a", &[("a", 0.8), ("none", 0.2)], Disposition::Accept);
        b2.tier = 2;
        assert_eq!(settle(&t_low, Some(&b2)), Settled::Brief("a".into()));
        let b_none = d(BRIEF, "none", &[("a", 0.15), ("none", 0.7)], Disposition::Escalate(Reason::LowTop1));
        assert!(matches!(settle(&t_low, Some(&b_none)), Settled::Open { topic: true, brief: true, .. }));
        let t2 = topic(0.2, Disposition::Accept);
        assert_eq!(settle(&t2, Some(&b_none)), Settled::Skip);
        let t2 = topic(0.9, Disposition::Accept);
        assert!(matches!(settle(&t2, Some(&b_none)), Settled::Open { topic: false, brief: true, .. }));
    }

    #[test]
    fn evidence_line_and_top_labels() {
        let titles: HashMap<String, String> = [("a".to_owned(), "SRE-2386".to_owned())].into();
        let t = topic(0.62, Disposition::Escalate(Reason::LowTop1));
        let b = d(BRIEF, "none", &[("a", 0.31), ("none", 0.55)], Disposition::Escalate(Reason::LowMargin));
        assert_eq!(
            evidence(&t, Some(&b), &titles, "low margin"),
            "oracle: topic 0.62, brief none 0.55 / SRE-2386 0.31 (escalated: low margin)"
        );
        assert_eq!(evidence(&t, None, &titles, "timeout"), "oracle: topic 0.62 (escalated: timeout)");
        assert_eq!(
            top(&t, Some(&b), &titles),
            vec![("topic".to_owned(), 0.62), ("SRE-2386".to_owned(), 0.31), ("none".to_owned(), 0.55)]
        );
    }

    #[test]
    fn outcome_compares_each_family_with_what_the_turn_did() {
        let a = BriefId::new();
        let b = BriefId::new();
        let (t_id, b_id) = (LogId::new(), LogId::new());
        let exp = Expected {
            rows: vec![(t_id, TOPIC.into(), "false".into()), (b_id, BRIEF.into(), "none".into())],
        };
        let (o, r) = outcome(&exp, &[]);
        assert_eq!(o, OracleOutcome { agreed: true, published: None });
        assert_eq!(r, vec![(t_id, decision::Outcome::Confirmed), (b_id, decision::Outcome::Confirmed)]);
        let (o, r) = outcome(&exp, &[(Some(a), "HR-12".into())]);
        assert_eq!(o, OracleOutcome { agreed: false, published: Some("HR-12".into()) });
        assert_eq!(r[0].1, decision::Outcome::Corrected { actual: "true".into() });
        assert_eq!(r[1].1, decision::Outcome::Corrected { actual: a.to_string() });
        // The oracle said brief=a; the model published a → agreed; published b → corrected.
        let exp = Expected {
            rows: vec![(t_id, TOPIC.into(), "true".into()), (b_id, BRIEF.into(), a.to_string())],
        };
        assert!(outcome(&exp, &[(Some(a), "x".into())]).0.agreed);
        let (o, r) = outcome(&exp, &[(Some(b), "y".into())]);
        assert!(!o.agreed);
        assert_eq!(r, vec![(t_id, decision::Outcome::Confirmed), (b_id, decision::Outcome::Corrected { actual: b.to_string() })]);
        // A new (uncached) brief counts as "something" for the topic and as no cached pick.
        let (o, _) = outcome(&exp, &[(None, "new".into())]);
        assert_eq!(o.published.as_deref(), Some("new"));
        assert!(!o.agreed);
    }

    #[test]
    fn state_and_batch_id() {
        let batch = vec![
            Utterance { track: Track::System, t0_ms: 872_000, t1_ms: 873_000, text: " so the DR sync ".into(), request: false },
            Utterance { track: Track::Mic, t0_ms: 874_000, t1_ms: 875_000, text: "right".into(), request: false },
        ];
        assert_eq!(state(&batch), "them: so the DR sync\nme: right");
        assert_eq!(batch_id(&batch), "00:14:32");
    }
}
