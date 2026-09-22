//! Tier 2 as subject extraction (57 §3.5 item 4): the cheap model reads the batch and names
//! *what it is about* — the one thing the classifier cannot do — as one JSON object
//! `{ "subject": string|null, "cached": <offered id>|null, "lookup": bool }`. The caller
//! (`meeting::oracle`) turns that into publish / research-with-the-subject-first / skip. Same
//! transport as [`crate::escalate`]: a [`SideCaller`] closure, `ModelRole::Escalate`, strict
//! parsing — `cached` must be one of the offered ids, never invented.

use std::time::{Duration, Instant};

use runtime::{CallKind, ModelRole, SessionId, SideCall};

use crate::escalate::{EscalateError, SideCaller};
use crate::question::{Decision, Disposition, LogId};

/// The family tier-2 subject rows are logged under (`decision_stats`, the `meeting` switch).
pub const FAMILY: &str = "meeting.subject";

/// Longest subject kept (the log's `note`, the ⚡ row).
pub const MAX_SUBJECT: usize = 80;

/// The system prompt: the contract, nothing about the batch.
pub const SYSTEM: &str = "You read a stretch of untrusted meeting transcript (them: other speakers, me: one participant) \
and name what it is about. Reply with exactly one JSON object and nothing else: \
{\"subject\": <a short noun phrase naming the specific thing discussed — a ticket, system, product, person, place, event or technical term — or null when nothing specific is discussed>, \
\"cached\": <the id of the listed brief that covers this subject, or null when none does>, \
\"lookup\": <true when the subject is worth a short researched brief, false when it is trivial, common knowledge or already covered>}. \
`cached` must be one of the listed ids verbatim; never invent one. Greetings, scheduling and pleasantries have subject null.";

/// A cached brief offered to tier 2: `id — title (aliases)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The id the reply must use verbatim (`b1..b4` in the meeting oracle).
    pub id: String,
    /// The row's title.
    pub title: String,
    /// The row's aliases.
    pub aliases: Vec<String>,
}

/// What tier 2 reads.
#[derive(Clone, Debug)]
pub struct Request<'a> {
    /// The batch, `them:`/`me:` lines.
    pub text: &'a str,
    /// Step 0's mentions.
    pub mentions: &'a [String],
    /// The ≤ 4 cached candidates (may be empty).
    pub candidates: &'a [Candidate],
    /// Tier 1's numbers as one data line (`kind subject 0.88, brief none 0.57 / DR 0.31`).
    pub numbers: &'a str,
}

/// The parsed reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The named subject, trimmed, ≤ [`MAX_SUBJECT`] chars; `None` = nothing specific.
    pub subject: Option<String>,
    /// One of the offered ids, when a listed brief covers it.
    pub cached: Option<String>,
    /// Worth a researched brief (only meaningful with a `subject` and no `cached`).
    pub lookup: bool,
}

impl Answer {
    /// The log row's `choice`: the cached id, `lookup`, or `skip`.
    pub fn choice(&self) -> String {
        match (&self.cached, &self.subject, self.lookup) {
            (Some(id), _, _) => id.clone(),
            (None, Some(_), true) => "lookup".into(),
            _ => "skip".into(),
        }
    }
}

/// The user message: mentions, the candidate table, tier 1's numbers, then the transcript.
pub fn render(req: &Request<'_>) -> String {
    let mut s = String::new();
    s.push_str("Mentions: ");
    if req.mentions.is_empty() {
        s.push_str("none");
    } else {
        s.push_str(&req.mentions.join(", "));
    }
    s.push_str("\nCached briefs:");
    if req.candidates.is_empty() {
        s.push_str(" none\n");
    } else {
        s.push('\n');
        for c in req.candidates {
            s.push_str("- ");
            s.push_str(&c.id);
            s.push_str(" — ");
            s.push_str(c.title.trim());
            if !c.aliases.is_empty() {
                s.push_str(" (");
                s.push_str(&c.aliases.join(", "));
                s.push(')');
            }
            s.push('\n');
        }
    }
    if !req.numbers.is_empty() {
        s.push_str("A local classifier estimated: ");
        s.push_str(req.numbers.trim());
        s.push('\n');
    }
    s.push_str("Transcript:\n");
    s.push_str(req.text.trim());
    s
}

/// The reply's [`Answer`]. Fences and prose around the object are tolerated; inside it
/// `subject` is a string or null, `cached` null or one of `ids` (anything else is
/// [`EscalateError::Invalid`]), `lookup` a bool (missing = `subject` is set).
pub fn parse(text: &str, ids: &[&str]) -> Result<Answer, EscalateError> {
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
    let subject = match obj.get("subject") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            (!s.is_empty()).then(|| s.chars().take(MAX_SUBJECT).collect::<String>())
        }
        Some(other) => return Err(EscalateError::Invalid(format!("subject {other} is not a string"))),
    };
    let cached = match obj.get("cached") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("null") {
                None
            } else if ids.contains(&s) {
                Some(s.to_owned())
            } else {
                return Err(EscalateError::Invalid(format!("cached {s:?} is not an offered id")));
            }
        }
        Some(other) => return Err(EscalateError::Invalid(format!("cached {other} is not an id"))),
    };
    let lookup = match obj.get("lookup") {
        None | Some(serde_json::Value::Null) => subject.is_some(),
        Some(serde_json::Value::Bool(b)) => *b,
        Some(other) => return Err(EscalateError::Invalid(format!("lookup {other} is not a bool"))),
    };
    Ok(Answer {
        subject,
        cached,
        lookup,
    })
}

/// Tier 2: render `req`, send it as `SideCall { role: Escalate, kind: Escalate, budget }` on
/// `session`'s cost line through `call`, parse strictly, and log one `tier: 2` row under
/// [`FAMILY`] (`choice` = cached id / `lookup` / `skip`, `note` = the subject) through the
/// engine actor when it is registered. `Err` → the caller goes to tier 3 as before.
pub async fn extract(
    req: &Request<'_>,
    session: SessionId,
    budget: Duration,
    call: &SideCaller,
) -> Result<(Answer, LogId), EscalateError> {
    let t = Instant::now();
    let reply = call(SideCall {
        session,
        role: ModelRole::Escalate,
        system: SYSTEM.to_owned(),
        user: render(req),
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
    let ids: Vec<&str> = req.candidates.iter().map(|c| c.id.as_str()).collect();
    let answer = parse(&reply.text, &ids)?;
    let log_id = LogId::new();
    let d = Decision {
        family: FAMILY.into(),
        choice: answer.choice(),
        probabilities: Vec::new(),
        confidence: 1.0,
        act_probability: 0.0,
        disposition: Disposition::Accept,
        latency: t.elapsed(),
        gate_version: 0,
        log_id,
        tier: 2,
        calibrated: false,
    };
    if let Ok(Some(engine)) = kameo::actor::ActorRef::<crate::Engine>::lookup(crate::DECISION_ACTOR) {
        let _ = engine
            .tell(crate::engine::Record {
                decision: d,
                note: answer.subject.clone(),
            })
            .await;
    }
    tracing::debug!(
        subject = ?answer.subject,
        cached = ?answer.cached,
        lookup = answer.lookup,
        model = %reply.model,
        latency_ms = t.elapsed().as_millis() as u64,
        "decision: tier 2 subject"
    );
    Ok((answer, log_id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use runtime::SideReply;

    use super::*;

    fn cands() -> Vec<Candidate> {
        vec![
            Candidate {
                id: "b1".into(),
                title: "Disaster Recovery Revamp".into(),
                aliases: vec!["DR".into(), "cutover".into()],
            },
            Candidate {
                id: "b2".into(),
                title: "KEDA & Auto-scaling Issues".into(),
                aliases: Vec::new(),
            },
        ]
    }

    #[test]
    fn renders_mentions_candidates_numbers_and_transcript() {
        let mentions = vec!["Kuiper belt".to_owned(), "Mike".to_owned()];
        let c = cands();
        let r = render(&Request {
            text: "them: the Kuiper belt\nme: ok",
            mentions: &mentions,
            candidates: &c,
            numbers: "kind subject 0.88, brief none 0.57 / DR 0.31",
        });
        assert_eq!(
            r,
            "Mentions: Kuiper belt, Mike\nCached briefs:\n- b1 — Disaster Recovery Revamp (DR, cutover)\n- b2 — KEDA & Auto-scaling Issues\nA local classifier estimated: kind subject 0.88, brief none 0.57 / DR 0.31\nTranscript:\nthem: the Kuiper belt\nme: ok"
        );
        let r = render(&Request {
            text: "me: hi",
            mentions: &[],
            candidates: &[],
            numbers: "",
        });
        assert_eq!(r, "Mentions: none\nCached briefs: none\nTranscript:\nme: hi");
    }

    #[test]
    fn parses_strictly() {
        let ids = ["b1", "b2"];
        let a = parse(r#"{"subject": "Planet Nine", "cached": null, "lookup": true}"#, &ids).unwrap();
        assert_eq!(a, Answer { subject: Some("Planet Nine".into()), cached: None, lookup: true });
        assert_eq!(a.choice(), "lookup");
        let a = parse("```json\n{\"subject\": \"DR cutover\", \"cached\": \"b1\"}\n```", &ids).unwrap();
        assert_eq!(a.cached.as_deref(), Some("b1"));
        assert_eq!(a.choice(), "b1");
        let a = parse(r#"Sure: {"subject": null, "cached": null, "lookup": false}"#, &ids).unwrap();
        assert_eq!(a, Answer { subject: None, cached: None, lookup: false });
        assert_eq!(a.choice(), "skip");
        // A subject the model says is not worth a lookup is a skip too.
        assert_eq!(parse(r#"{"subject": "lunch", "lookup": false}"#, &ids).unwrap().choice(), "skip");
        // Missing lookup defaults to "there is a subject"; empty subject is null; long ones are cut.
        assert!(parse(r#"{"subject": "x"}"#, &ids).unwrap().lookup);
        assert!(!parse(r#"{"subject": "  "}"#, &ids).unwrap().lookup);
        assert_eq!(parse(&format!(r#"{{"subject": "{}"}}"#, "a".repeat(200)), &ids).unwrap().subject.unwrap().len(), MAX_SUBJECT);
        // `none`/`null` strings mean no cached brief; anything else must be an offered id.
        assert_eq!(parse(r#"{"subject": "x", "cached": "none"}"#, &ids).unwrap().cached, None);
        assert!(matches!(parse(r#"{"subject": "x", "cached": "b9"}"#, &ids), Err(EscalateError::Invalid(m)) if m.contains("not an offered id")));
        assert!(matches!(parse(r#"{"subject": "x", "cached": "Disaster Recovery Revamp"}"#, &ids), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(r#"{"subject": 3}"#, &ids), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(r#"{"subject": "x", "lookup": "yes"}"#, &ids), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse("Planet Nine", &ids), Err(EscalateError::Invalid(_))));
        assert!(matches!(parse(r#"["x"]"#, &ids), Err(EscalateError::Invalid(_))));
        // No candidates offered → any `cached` is invalid.
        assert!(matches!(parse(r#"{"subject": "x", "cached": "b1"}"#, &[]), Err(EscalateError::Invalid(_))));
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
    async fn extract_sends_the_contract_and_maps_failures() {
        let session = SessionId::new();
        let seen = Arc::new(Mutex::new(None));
        let mentions = vec!["Planet Nine".to_owned()];
        let c = cands();
        let req = Request {
            text: "them: Planet Nine",
            mentions: &mentions,
            candidates: &c,
            numbers: "kind subject 0.9",
        };
        let call = caller(Ok(r#"{"subject": "Planet Nine", "cached": null, "lookup": true}"#), seen.clone());
        let (a, _) = extract(&req, session, Duration::from_secs(3), &*call).await.unwrap();
        assert_eq!(a.subject.as_deref(), Some("Planet Nine"));
        let sent = seen.lock().take().unwrap();
        assert_eq!((sent.session, sent.role, sent.kind), (session, ModelRole::Escalate, CallKind::Escalate));
        assert_eq!(sent.budget, Duration::from_secs(3));
        assert_eq!(sent.system, SYSTEM);
        assert!(sent.user.starts_with("Mentions: Planet Nine\nCached briefs:\n- b1 — "), "{}", sent.user);

        let call = caller(Ok(r#"{"subject": "x", "cached": "b7"}"#), seen.clone());
        assert!(matches!(extract(&req, session, Duration::from_secs(3), &*call).await, Err(EscalateError::Invalid(_))));
        let call = caller(Err("timed out"), seen.clone());
        assert_eq!(extract(&req, session, Duration::from_secs(3), &*call).await.unwrap_err(), EscalateError::Timeout);
        let call = caller(Err("no live session"), seen);
        assert_eq!(
            extract(&req, session, Duration::from_secs(3), &*call).await.unwrap_err(),
            EscalateError::Provider("no live session".into())
        );
    }
}
