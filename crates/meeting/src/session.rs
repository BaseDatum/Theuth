//! The meeting session's inputs (02 §3, 53 §4.4): the line formats the model
//! is told about in the prompt block, and delivery — `Steer` while a turn
//! runs, `SendInput` otherwise. Every line starts with `[meeting`.

use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use runtime::{InputId, SessionId};

use crate::state::Prompted;
use crate::{BriefId, Utterance};

/// `hh:mm:ss` on the recording clock.
pub fn hms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// `label, label` or `none`.
fn labels(labels: &[String]) -> String {
    if labels.is_empty() {
        "none".to_owned()
    } else {
        labels.join(", ")
    }
}

/// How long ago `at` was, as `Nd` / `Nh` / `Nm` (05 §0.7: staleness is visible, not automatic).
pub fn age(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let mins = now.signed_duration_since(at).num_minutes().max(0);
    match mins {
        m if m >= 24 * 60 => format!("{}d", m / (24 * 60)),
        m if m >= 60 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// `age` of an RFC 3339 stamp; unparseable → `?`.
pub fn age_of(stamp: &str, now: DateTime<Utc>) -> String {
    DateTime::parse_from_rfc3339(stamp)
        .map(|d| age(d.with_timezone(&Utc), now))
        .unwrap_or_else(|_| "?".into())
}

/// A cached live label in the batch header: `<label> (cached <age>)`.
pub fn cached_label(label: &str, age: &str) -> String {
    format!("{label} (cached {age})")
}

/// One transcript batch: the header with the live and dismissed label sets,
/// then one `Speaker N:`/`them:`/`me:` line per utterance (07 §4). `batch` is non-empty.
pub fn batch_line(batch: &[Utterance], live: &[String], dismissed: &[String]) -> String {
    batch_line_with(batch, live, dismissed, &[], None)
}

/// [`batch_line`] plus the oracle's additions (57 §3.2, §3.5): `carried` are earlier batches the
/// oracle kept from the model (`(stamp, utterances)`), listed in the header as
/// `earlier (oracle skipped): 00:13:10, 00:13:40` and written before this batch's rows so the
/// turn still has them as context; `evidence` is the `oracle: …` line, written **first** — right
/// after the header — so a research turn starts from the oracle's subject, not the transcript.
pub fn batch_line_with(
    batch: &[Utterance],
    live: &[String],
    dismissed: &[String],
    carried: &[(String, Vec<Utterance>)],
    evidence: Option<&str>,
) -> String {
    let mut s = format!(
        "[meeting {}] live: {} · dismissed: {}",
        hms(batch.first().map_or(0, |u| u.t0_ms)),
        labels(live),
        labels(dismissed)
    );
    if !carried.is_empty() {
        s.push_str(" · earlier (oracle skipped): ");
        s.push_str(
            &carried
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some(e) = evidence {
        s.push('\n');
        s.push_str(e);
    }
    for u in carried.iter().flat_map(|(_, b)| b.iter()).chain(batch) {
        s.push('\n');
        s.push_str(&u.who());
        s.push_str(": ");
        s.push_str(u.text.trim());
    }
    s
}

/// The first input of the meeting.
pub fn started_line(context: Option<&str>) -> String {
    let context = context.map(str::trim).filter(|c| !c.is_empty());
    format!(
        "[meeting 00:00:00] started. context: {}",
        context.unwrap_or("none")
    )
}

/// A push-to-talk (`spoken`) or control-center (`typed`) request. `cached` is the title of
/// the cached brief the actor already showed for it (05 §3): the model adds only what it lacks.
pub fn request_line(spoken: bool, text: &str, cached: Option<&str>) -> String {
    let how = if spoken { "spoken" } else { "typed" };
    let mut s = format!("[meeting request, {how}] {}", text.trim());
    if let Some(title) = cached {
        s.push_str(&format!(
            " (cached brief \"{title}\" already shown — add only what it lacks)"
        ));
    }
    s
}

/// The user tapped a topic button.
pub fn tapped_line(label: &str) -> String {
    format!("[meeting tapped \"{label}\"]")
}

/// The user dismissed a topic button.
pub fn dismissed_line(label: &str) -> String {
    format!("[meeting: user dismissed \"{label}\" — not useful]")
}

/// The user asked for more on a brief. The id rides along so the model can set
/// `meeting_brief.parent` (a `BriefId`; the title alone parsed as a UUID and failed — 03 §9).
pub fn deeper_line(id: BriefId, title: &str, bullets: &[String]) -> String {
    let bullets: Vec<&str> = bullets.iter().map(|b| b.trim()).collect();
    format!(
        "[meeting deeper on \"{title}\" — parent: {}] - {}",
        id.0,
        bullets.join("\n- ")
    )
}

/// 03 §9: a turn ended without a `meeting_brief` for a request or deeper the user made; the text
/// reply, if any, was never shown. Sent once per prompt.
pub fn nudge_line(prompt: Prompted, text: &str) -> String {
    let what = match prompt {
        Prompted::Deeper(id) => format!("deeper on \"{text}\" (parent: {})", id.0),
        _ => format!("the request \"{text}\""),
    };
    format!(
        "[meeting: no brief was produced for {what} — the user sees only briefs, never text. Call \
         `meeting_brief` now with two or three short factual bullets answering it.]"
    )
}

/// Deliver `text` to `session`: `Steer` when a turn runs (it lands after the
/// current tool batch, 53 §0.2), `SendInput` otherwise. `Err` is the text of
/// the supervisor's refusal or "agent is not running".
pub async fn deliver(session: SessionId, text: String, turn_running: bool) -> Result<(), String> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or("agent is not running")?;
    if turn_running {
        agents
            .ask(agent::Steer {
                session,
                id: InputId::new(),
                text,
                files: Vec::new(),
                delivery: agent::Delivery::default(),
            })
            .await
            .map_err(|e| e.to_string())
    } else {
        agents
            .ask(agent::SendInput {
                session: Some(session),
                text,
                files: Vec::new(),
                delivery: agent::Delivery::default(),
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Deliver `text` to `session` as its own turn: now when idle, else after the running
/// turn ends (`Delivery::NextTurn`, 62 §0.3). `id` is echoed by `AgentEvent::InputInjected`
/// with the turn it opened — the caller binds to that, not to the turn's text.
pub async fn queue_turn(session: SessionId, id: InputId, text: String) -> Result<(), String> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or("agent is not running")?;
    agents
        .ask(agent::Steer {
            session,
            id,
            text,
            files: Vec::new(),
            delivery: agent::Delivery::NextTurn,
        })
        .await
        .map_err(|e| e.to_string())
}

/// Steers waiting on `session`; `0` when the agent is unreachable.
pub async fn queued(session: SessionId) -> usize {
    match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(agents)) => agents
            .ask(agent::ListQueued { session })
            .await
            .map_or(0, |q| q.len()),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Track;

    fn u(track: Track, t0_ms: u64, text: &str) -> Utterance {
        Utterance {
            track,
            t0_ms,
            t1_ms: t0_ms + 1000,
            text: text.into(),
            request: false,
            speaker: None,
        }
    }

    /// §3's example verbatim: the header time is the first utterance's `t0`,
    /// labels render as sets or `none`, speakers as `them:`/`me:` — `Speaker N:` once the
    /// live diariser labelled a system line (07 §4).
    #[test]
    fn batch_line_matches_the_design() {
        let batch = [
            u(
                Track::System,
                872_000,
                "so the DR sync for two-three-eight-six is blocked on the image tags",
            ),
            u(Track::Mic, 875_500, " right, I think the app patches those "),
        ];
        let live = ["SRE-2386".to_owned(), "DR cutover".to_owned()];
        let dismissed = ["KEDA".to_owned()];
        assert_eq!(
            batch_line(&batch, &live, &dismissed),
            "[meeting 00:14:32] live: SRE-2386, DR cutover · dismissed: KEDA\n\
             them: so the DR sync for two-three-eight-six is blocked on the image tags\n\
             me: right, I think the app patches those"
        );
        let labelled = [Utterance { speaker: Some("s2".into()), ..batch[0].clone() }];
        assert_eq!(
            batch_line(&labelled, &[], &[]),
            "[meeting 00:14:32] live: none · dismissed: none\n\
             Speaker 2: so the DR sync for two-three-eight-six is blocked on the image tags"
        );
        assert_eq!(
            batch_line(&batch[1..], &[], &[]),
            "[meeting 00:14:35] live: none · dismissed: none\nme: right, I think the app patches those"
        );
        assert_eq!(hms(3_600_000 * 2 + 59_999), "02:00:59");
        // 05 §3: a cached live label carries its age.
        let now: DateTime<Utc> = "2026-09-20T12:00:00Z".parse().unwrap();
        assert_eq!(age_of("2026-09-17T11:59:00Z", now), "3d");
        assert_eq!(age_of("2026-09-20T09:01:00Z", now), "2h");
        assert_eq!(age_of("2026-09-20T11:48:30Z", now), "11m");
        assert_eq!(age_of("2026-09-21T00:00:00Z", now), "0m", "a future stamp is not negative");
        assert_eq!(age_of("garbage", now), "?");
        let live = [cached_label("SRE-2386", "3d"), "DR cutover".to_owned()];
        assert!(batch_line(&batch, &live, &[]).starts_with("[meeting 00:14:32] live: SRE-2386 (cached 3d), DR cutover · dismissed: none\n"));
    }

    #[test]
    fn other_lines() {
        assert_eq!(
            started_line(Some("  weekly DR sync ")),
            "[meeting 00:00:00] started. context: weekly DR sync"
        );
        assert_eq!(started_line(Some("  ")), "[meeting 00:00:00] started. context: none");
        assert_eq!(
            request_line(true, "what is the DR cutover date", None),
            "[meeting request, spoken] what is the DR cutover date"
        );
        assert_eq!(request_line(false, "who owns KEDA", None), "[meeting request, typed] who owns KEDA");
        assert_eq!(
            request_line(false, "what's the KEDA limit", Some("KEDA scaling")),
            "[meeting request, typed] what's the KEDA limit (cached brief \"KEDA scaling\" already shown — add only what it lacks)"
        );
        assert_eq!(tapped_line("SRE-2386"), "[meeting tapped \"SRE-2386\"]");
        assert_eq!(
            dismissed_line("KEDA"),
            "[meeting: user dismissed \"KEDA\" — not useful]"
        );
        let id = BriefId(uuid::Uuid::nil());
        assert_eq!(
            deeper_line(id, "DR cutover", &["b1".into(), "b2 ".into()]),
            format!("[meeting deeper on \"DR cutover\" — parent: {}] - b1\n- b2", id.0)
        );
        assert!(nudge_line(Prompted::Request { spoken: true }, "cutover date")
            .starts_with("[meeting: no brief was produced for the request \"cutover date\""));
        assert!(nudge_line(Prompted::Deeper(id), "DR cutover").contains(&format!("parent: {}", id.0)));
    }
}
