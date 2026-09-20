//! The meeting session's inputs (02 §3, 53 §4.4): the line formats the model
//! is told about in the prompt block, and delivery — `Steer` while a turn
//! runs, `SendInput` otherwise. Every line starts with `[meeting`.

use kameo::actor::ActorRef;
use runtime::{InputId, SessionId};

use crate::{Track, Utterance};

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

/// One transcript batch: the header with the live and dismissed label sets,
/// then one `them:`/`me:` line per utterance. `batch` is non-empty.
pub fn batch_line(batch: &[Utterance], live: &[String], dismissed: &[String]) -> String {
    let mut s = format!(
        "[meeting {}] live: {} · dismissed: {}",
        hms(batch.first().map_or(0, |u| u.t0_ms)),
        labels(live),
        labels(dismissed)
    );
    for u in batch {
        let who = match u.track {
            Track::Mic => "me",
            Track::System => "them",
        };
        s.push('\n');
        s.push_str(who);
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

/// A push-to-talk (`spoken`) or control-center (`typed`) request.
pub fn request_line(spoken: bool, text: &str) -> String {
    let how = if spoken { "spoken" } else { "typed" };
    format!("[meeting request, {how}] {}", text.trim())
}

/// The user tapped a topic button.
pub fn tapped_line(label: &str) -> String {
    format!("[meeting tapped \"{label}\"]")
}

/// The user dismissed a topic button.
pub fn dismissed_line(label: &str) -> String {
    format!("[meeting: user dismissed \"{label}\" — not useful]")
}

/// The user asked for more on a brief.
pub fn deeper_line(title: &str, bullets: &[String]) -> String {
    let bullets: Vec<&str> = bullets.iter().map(|b| b.trim()).collect();
    format!("[meeting deeper on \"{title}\"] - {}", bullets.join("\n- "))
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
            })
            .await
            .map_err(|e| e.to_string())
    } else {
        agents
            .ask(agent::SendInput {
                session: Some(session),
                text,
                files: Vec::new(),
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
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

    fn u(track: Track, t0_ms: u64, text: &str) -> Utterance {
        Utterance {
            track,
            t0_ms,
            t1_ms: t0_ms + 1000,
            text: text.into(),
            request: false,
        }
    }

    /// §3's example verbatim: the header time is the first utterance's `t0`,
    /// labels render as sets or `none`, speakers as `them:`/`me:`.
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
        assert_eq!(
            batch_line(&batch[1..], &[], &[]),
            "[meeting 00:14:35] live: none · dismissed: none\nme: right, I think the app patches those"
        );
        assert_eq!(hms(3_600_000 * 2 + 59_999), "02:00:59");
    }

    #[test]
    fn other_lines() {
        assert_eq!(
            started_line(Some("  weekly DR sync ")),
            "[meeting 00:00:00] started. context: weekly DR sync"
        );
        assert_eq!(started_line(Some("  ")), "[meeting 00:00:00] started. context: none");
        assert_eq!(
            request_line(true, "what is the DR cutover date"),
            "[meeting request, spoken] what is the DR cutover date"
        );
        assert_eq!(request_line(false, "who owns KEDA"), "[meeting request, typed] who owns KEDA");
        assert_eq!(tapped_line("SRE-2386"), "[meeting tapped \"SRE-2386\"]");
        assert_eq!(
            dismissed_line("KEDA"),
            "[meeting: user dismissed \"KEDA\" — not useful]"
        );
        assert_eq!(
            deeper_line("DR cutover", &["b1".into(), "b2 ".into()]),
            "[meeting deeper on \"DR cutover\"] - b1\n- b2"
        );
    }
}
