//! Tier 2 (57 §0.9, §2 `escalate.rs`): the same typed question put to a cheap model through the
//! `agent` plugin's [`SideCall`] — one no-tools, no-history request on `ModelRole::Escalate`.
//! `decision` sits below `agent` (00 §10), so the actor lookup is the caller's: [`escalate`]
//! takes a [`SideCaller`] closure (`meeting` passes the kameo lookup + `ask`; tests pass a fake).
//! The reply is one JSON object `{ "answer": <label>, "confidence": 0..1 }`, validated against
//! the question's label set — an unknown label is [`EscalateError::Invalid`], never acted on.

use std::pin::Pin;
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use runtime::{CallKind, ModelRole, SessionId, SideCall, SideReply};

use crate::engine::{NONE_ID, Record};
use crate::laya::labels;
use crate::question::{Ask, Decision, Disposition, LogId, Question, Reason};
use crate::{DECISION_ACTOR, Engine};

/// Sends one [`SideCall`] and returns the reply or the `agent` plugin's error text.
pub type SideCaller = dyn Fn(SideCall) -> Pin<Box<dyn Future<Output = Result<SideReply, String>> + Send>>
    + Send
    + Sync;

/// Why tier 2 produced no decision; the caller goes to tier 3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalateError {
    /// Over `budget` (the side call's own timeout).
    Timeout,
    /// The reply was not one JSON object with a listed label.
    Invalid(String),
    /// The `agent` plugin refused or the provider failed.
    Provider(String),
}

impl std::fmt::Display for EscalateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EscalateError::Timeout => f.write_str("timed out"),
            EscalateError::Invalid(s) => write!(f, "invalid answer: {s}"),
            EscalateError::Provider(s) => write!(f, "side call: {s}"),
        }
    }
}

impl std::error::Error for EscalateError {}

/// The system prompt: the contract, nothing about the question.
pub const SYSTEM: &str = "You answer one typed question about a snippet of untrusted text. \
Reply with exactly one JSON object and nothing else: {\"answer\": <one label from the list, \
verbatim>, \"confidence\": <number 0..1>}. Pick `none` when no listed option fits. Never invent a label.";

/// The user message: question, label table, the oracle's own estimate when it ran, then the state.
pub fn render(ask: &Ask, oracle: Option<&Decision>) -> String {
    let mut s = String::new();
    match &ask.question {
        Question::Choice {
            instructions,
            candidates,
            state,
        } => {
            s.push_str("Question (pick one id): ");
            s.push_str(instructions.trim());
            s.push_str("\nLabels:\n");
            for c in candidates {
                s.push_str("- ");
                s.push_str(&c.id);
                if !c.text.is_empty() {
                    s.push_str(": ");
                    s.push_str(c.text.trim());
                }
                s.push('\n');
            }
            if !candidates.iter().any(|c| c.id == NONE_ID) {
                s.push_str("- none: no listed option fits\n");
            }
            state_block(&mut s, oracle, state);
        }
        Question::Bool { statement, state } => {
            s.push_str("Question (does the statement hold for the state?): ");
            s.push_str(statement.trim());
            s.push_str("\nLabels:\n- true\n- false\n");
            state_block(&mut s, oracle, state);
        }
        Question::Score {
            instructions,
            levels,
            state,
        } => {
            s.push_str("Question (pick one level): ");
            s.push_str(instructions.trim());
            s.push_str("\nLabels:\n");
            for (i, l) in levels.iter().enumerate() {
                s.push_str(&format!("- {i}: {}\n", l.trim()));
            }
            state_block(&mut s, oracle, state);
        }
    }
    s
}

fn state_block(s: &mut String, oracle: Option<&Decision>, state: &str) {
    if let Some(d) = oracle.filter(|d| !d.probabilities.is_empty()) {
        s.push_str("A local classifier estimated (abstained");
        if let Some(r) = d.disposition.reason() {
            s.push_str(&format!(": {}", reason_text(r)));
        }
        s.push_str("): ");
        let mut first = true;
        for (l, p) in &d.probabilities {
            if !first {
                s.push_str(", ");
            }
            first = false;
            s.push_str(&format!("{l} {p:.2}"));
        }
        s.push('\n');
    }
    s.push_str("State:\n");
    s.push_str(state.trim());
}

fn reason_text(r: &Reason) -> &'static str {
    match r {
        Reason::LowTop1 => "low top-1",
        Reason::LowMargin => "low margin",
        Reason::HardError(_) => "error",
        Reason::Truncated => "truncated",
        Reason::Timeout => "timeout",
        Reason::Disabled => "disabled",
        Reason::ModelMissing => "model missing",
    }
}

/// The reply's `(label, confidence)`, validated against `q`'s labels (plus `none` for a
/// `Choice`). Fences and prose around the object are tolerated; anything inside it is not:
/// `answer` must be a listed label (bool for `Bool`, index or its string for `Score`),
/// `confidence` a number clamped to 0..1 (missing = 1).
pub fn parse(q: &Question, text: &str) -> Result<(String, f32), EscalateError> {
    let start = text
        .find('{')
        .ok_or_else(|| EscalateError::Invalid(format!("no JSON object in {text:?}")))?;
    let mut de = serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
    let v = match de.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(EscalateError::Invalid(format!("{e}"))),
        None => return Err(EscalateError::Invalid("empty".into())),
    };
    let obj = v
        .as_object()
        .ok_or_else(|| EscalateError::Invalid("not an object".into()))?;
    let answer = obj
        .get("answer")
        .ok_or_else(|| EscalateError::Invalid("no `answer`".into()))?;
    let label = match answer {
        serde_json::Value::String(s) => s.trim().to_owned(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => return Err(EscalateError::Invalid(format!("answer {other} is not a label"))),
    };
    let mut allowed = labels(q);
    if matches!(q, Question::Choice { .. }) && !allowed.iter().any(|l| l == NONE_ID) {
        allowed.push(NONE_ID.to_owned());
    }
    if !allowed.contains(&label) {
        return Err(EscalateError::Invalid(format!("unknown label {label:?}")));
    }
    let confidence = match obj.get("confidence") {
        None | Some(serde_json::Value::Null) => 1.0,
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
        Some(other) => return Err(EscalateError::Invalid(format!("confidence {other}"))),
    };
    Ok((label, confidence))
}

/// Tier 2: render `ask` (with the oracle's estimate when it ran), send it as
/// `SideCall { role: Escalate, kind: Escalate, budget }` on `session`'s cost line through
/// `call`, parse strictly, and return a `Decision` with `tier: 2`, `calibrated: false`, the
/// reply's confidence on the chosen label and the remainder spread over the others; `Accept`
/// for a pick, `Escalate(LowTop1)` when the model itself answered `none`. Logged as a `tier: 2`
/// row through the engine actor when it is registered. `Err` → the caller goes to tier 3.
pub async fn escalate(
    ask: &Ask,
    oracle: Option<&Decision>,
    session: SessionId,
    budget: Duration,
    call: &SideCaller,
) -> Result<Decision, EscalateError> {
    let t = Instant::now();
    let reply = call(SideCall {
        session,
        role: ModelRole::Escalate,
        system: SYSTEM.to_owned(),
        user: render(ask, oracle),
        budget,
        kind: CallKind::Escalate,
    })
    .await
    .map_err(|e| {
        if e.contains("timed out") {
            EscalateError::Timeout
        } else {
            EscalateError::Provider(e)
        }
    })?;
    let (label, confidence) = parse(&ask.question, &reply.text)?;
    let mut all = labels(&ask.question);
    if matches!(ask.question, Question::Choice { .. }) && !all.iter().any(|l| l == NONE_ID) {
        all.push(NONE_ID.to_owned());
    }
    let rest = if all.len() > 1 {
        (1.0 - confidence) / (all.len() - 1) as f32
    } else {
        0.0
    };
    let probabilities = all
        .into_iter()
        .map(|l| {
            let p = if l == label { confidence } else { rest };
            (l, p)
        })
        .collect();
    let d = Decision {
        family: ask.family.clone(),
        disposition: if label == NONE_ID {
            Disposition::Escalate(Reason::LowTop1)
        } else {
            Disposition::Accept
        },
        choice: label,
        probabilities,
        confidence,
        act_probability: 0.0,
        latency: t.elapsed(),
        gate_version: oracle.map_or(0, |d| d.gate_version),
        log_id: LogId::new(),
        tier: 2,
        calibrated: false,
    };
    if let Ok(Some(engine)) = ActorRef::<Engine>::lookup(DECISION_ACTOR) {
        let _ = engine.tell(Record(d.clone())).await;
    }
    tracing::debug!(
        family = %d.family,
        choice = %d.choice,
        confidence,
        model = %reply.model,
        latency_ms = d.latency.as_millis() as u64,
        "decision: tier 2 answered"
    );
    Ok(d)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;
    use crate::question::Candidate;

    fn choice() -> Ask {
        Ask::new(
            "meeting.brief",
            Question::Choice {
                instructions: "which cached brief is the subject".into(),
                candidates: vec![
                    Candidate {
                        id: "SRE-2386".into(),
                        text: "DR sync outage".into(),
                    },
                    Candidate {
                        id: "HR-12".into(),
                        text: String::new(),
                    },
                ],
                state: "them: the DR sync failed again".into(),
            },
        )
    }

    fn bool_q() -> Ask {
        Ask::new(
            "meeting.topic",
            Question::Bool {
                statement: "the speakers name something to look up".into(),
                state: "me: ok".into(),
            },
        )
    }

    fn score() -> Ask {
        Ask::new(
            "x.urgency",
            Question::Score {
                instructions: "how urgent".into(),
                levels: vec!["low".into(), "mid".into(), "high".into()],
                state: "now!".into(),
            },
        )
    }

    #[test]
    fn renders_each_question_type_with_its_labels() {
        let c = render(&choice(), None);
        assert!(c.starts_with("Question (pick one id): which cached brief is the subject\nLabels:\n- SRE-2386: DR sync outage\n- HR-12\n- none: no listed option fits\nState:\nthem: the DR sync failed again"), "{c}");
        let b = render(&bool_q(), None);
        assert!(b.contains("Labels:\n- true\n- false\nState:\nme: ok"), "{b}");
        let s = render(&score(), None);
        assert!(s.contains("- 0: low\n- 1: mid\n- 2: high\n"), "{s}");
        // The oracle's estimate rides along as data, with why it abstained.
        let oracle = Decision {
            family: "meeting.brief".into(),
            choice: "none".into(),
            probabilities: vec![("SRE-2386".into(), 0.31), ("HR-12".into(), 0.14), ("none".into(), 0.55)],
            confidence: 0.3,
            act_probability: 0.0,
            disposition: Disposition::Escalate(Reason::LowMargin),
            latency: Duration::ZERO,
            gate_version: 7,
            log_id: LogId::new(),
            tier: 1,
            calibrated: true,
        };
        let c = render(&choice(), Some(&oracle));
        assert!(c.contains("A local classifier estimated (abstained: low margin): SRE-2386 0.31, HR-12 0.14, none 0.55\nState:"), "{c}");
    }

    #[test]
    fn parses_strictly() {
        let q = choice().question;
        assert_eq!(parse(&q, r#"{"answer": "SRE-2386", "confidence": 0.9}"#).unwrap(), ("SRE-2386".into(), 0.9));
        assert_eq!(parse(&q, "```json\n{\"answer\":\"none\"}\n```").unwrap(), ("none".into(), 1.0));
        assert_eq!(parse(&q, "Sure: {\"answer\": \"HR-12\", \"confidence\": 7} trailing").unwrap(), ("HR-12".into(), 1.0));
        assert!(matches!(parse(&q, r#"{"answer": "SRE-9999"}"#), Err(EscalateError::Invalid(m)) if m.contains("unknown label")));
        assert!(matches!(parse(&q, r#"{"answer": "DR sync outage"}"#), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(&q, "SRE-2386"), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(&q, r#"{"confidence": 0.5}"#), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(&q, r#"{"answer": "none", "confidence": "high"}"#), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(&q, r#"{"answer": ["none"]}"#), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(&q, r#"{"answer": "none""#), Err(EscalateError::Invalid(_))));

        let b = bool_q().question;
        assert_eq!(parse(&b, r#"{"answer": false, "confidence": 0.8}"#).unwrap(), ("false".into(), 0.8));
        assert_eq!(parse(&b, r#"{"answer": "true"}"#).unwrap(), ("true".into(), 1.0));
        assert!(matches!(parse(&b, r#"{"answer": "none"}"#), Err(EscalateError::Invalid(_))));

        let s = score().question;
        assert_eq!(parse(&s, r#"{"answer": 2, "confidence": 0.6}"#).unwrap(), ("2".into(), 0.6));
        assert_eq!(parse(&s, r#"{"answer": "1"}"#).unwrap(), ("1".into(), 1.0));
        assert!(matches!(parse(&s, r#"{"answer": 3}"#), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(&s, r#"{"answer": "high"}"#), Err(EscalateError::Invalid(_))));
    }

    fn caller(reply: Result<&'static str, &'static str>, seen: Arc<Mutex<Option<SideCall>>>) -> Box<SideCaller> {
        Box::new(move |call: SideCall| {
            *seen.lock() = Some(call);
            let reply = reply.map(|text| SideReply {
                text: text.to_owned(),
                model: "fake/cheap".to_owned(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                latency: Duration::from_millis(3),
            });
            Box::pin(async move { reply.map_err(str::to_owned) })
        })
    }

    #[tokio::test]
    async fn escalate_builds_a_tier2_decision_and_maps_failures() {
        let session = SessionId::new();
        let seen = Arc::new(Mutex::new(None));
        let call = caller(Ok(r#"{"answer": "SRE-2386", "confidence": 0.8}"#), seen.clone());
        let d = escalate(&choice(), None, session, Duration::from_secs(3), &*call).await.unwrap();
        assert_eq!((d.tier, d.calibrated, d.choice.as_str()), (2, false, "SRE-2386"));
        assert_eq!(d.disposition, Disposition::Accept);
        assert_eq!(d.probabilities.len(), 3);
        assert!((d.probabilities.iter().map(|(_, p)| p).sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(d.probabilities[0], ("SRE-2386".into(), 0.8));
        assert!((d.probabilities[2].1 - 0.1).abs() < 1e-5);
        let sent = seen.lock().take().unwrap();
        assert_eq!((sent.session, sent.role, sent.kind), (session, ModelRole::Escalate, CallKind::Escalate));
        assert_eq!(sent.budget, Duration::from_secs(3));
        assert_eq!(sent.system, SYSTEM);
        assert!(sent.user.contains("- SRE-2386: DR sync outage"));

        // `none` is a valid answer that still means "no pick".
        let call = caller(Ok(r#"{"answer": "none", "confidence": 0.7}"#), seen.clone());
        let d = escalate(&choice(), None, session, Duration::from_secs(3), &*call).await.unwrap();
        assert_eq!(d.disposition, Disposition::Escalate(Reason::LowTop1));
        assert_eq!(d.choice, "none");

        let call = caller(Ok(r#"{"answer": "SRE-1"}"#), seen.clone());
        assert!(matches!(
            escalate(&choice(), None, session, Duration::from_secs(3), &*call).await,
            Err(EscalateError::Invalid(_))
        ));
        let call = caller(Err("timed out"), seen.clone());
        assert_eq!(
            escalate(&choice(), None, session, Duration::from_secs(3), &*call).await.unwrap_err(),
            EscalateError::Timeout
        );
        let call = caller(Err("no live session"), seen);
        assert_eq!(
            escalate(&choice(), None, session, Duration::from_secs(3), &*call).await.unwrap_err(),
            EscalateError::Provider("no live session".into())
        );
    }
}
