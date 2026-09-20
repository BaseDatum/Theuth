//! The guided summary (04 §6, 53 decision 7): one normal turn in the meeting's
//! session — the form below as a `SendInput` — then the assistant's reply is
//! `summary.md`, its `#` line the title, `knowledge_harvest` runs, and the
//! session is compacted with the summary as head so the follow-up conversation
//! starts from it and reaches the record through the tools.

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};

use crate::store::{Manifest, SUMMARY, Store};
use crate::{MeetingEvent, MeetingState};

/// Sent as one user input after `Reviewed`.
pub const SUMMARY_FORM: &str = "[meeting summary] The meeting has ended and the transcript was \
reviewed. Use `meeting_read` (all of it, in pieces) and `meeting_speakers`, then write the summary \
as markdown: a `#` title line (at most 8 words), then **Topics** (what was discussed, in order, one \
line each), **Decisions** (with who), **Action items** (owner — item — due if said), **Open \
questions**, **Numbers and names said aloud**. Improvise sections if the meeting calls for it; keep \
it under 400 words. Reply with the markdown only.";

/// The title: the first `#` heading, else the first non-empty line; ≤ 200 chars.
pub fn title_of(markdown: &str) -> String {
    let lines = markdown.lines().map(str::trim).filter(|l| !l.is_empty());
    let t = lines
        .clone()
        .find(|l| l.starts_with('#'))
        .or_else(|| lines.clone().next())
        .unwrap_or("Meeting summary")
        .trim_start_matches('#')
        .trim();
    t.chars().take(200).collect()
}

fn agents() -> Result<ActorRef<agent::Agents>, String> {
    ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "agent is not running".to_owned())
}

/// The last assistant message of `session`.
async fn last_reply(session: SessionId) -> Result<String, String> {
    let loaded = agents()?
        .ask(agent::LoadSession {
            session,
            before: None,
            limit: Some(8),
            settled_only: false,
        })
        .await
        .map_err(|e| e.to_string())?;
    loaded
        .history
        .iter()
        .rev()
        .find_map(|h| match h {
            agent::HistoryEntry::Assistant { text, .. } if !text.trim().is_empty() => {
                Some(text.trim().to_owned())
            }
            _ => None,
        })
        .ok_or_else(|| "the summary turn produced no text".to_owned())
}

/// After the summary turn finished: file, title, harvest, compaction, `Done`.
async fn finish(store: &Store, m: &mut Manifest) -> Result<String, String> {
    let session = m.session;
    let summary = last_reply(session).await?;
    let title = title_of(&summary);
    let path = store.meeting_dir(m.id).join(SUMMARY);
    std::fs::write(&path, &summary).map_err(|e| format!("write {}: {e}", path.display()))?;
    if let Ok(mut t) = store.transcript(m.id) {
        t.summarized_at = Some(chrono::Local::now().to_rfc3339());
        if let Err(e) = store.save_transcript(&t).await {
            tracing::warn!(meeting = %m.id, "summarized_at: {e:#}");
        }
    }
    let agents = agents()?;
    if let Err(e) = agents
        .ask(agent::RenameSession {
            session,
            title: title.clone(),
        })
        .await
    {
        tracing::warn!(meeting = %m.id, "rename session: {e}");
    }
    // 10 §5: harvest the meeting's frame. A turn still settling → one retry.
    let mut harvest = agents.ask(agent::Harvest { session }).await;
    if harvest.as_ref().is_err_and(|e| e.to_string().contains("turn in flight")) {
        tokio::time::sleep(Duration::from_secs(3)).await;
        harvest = agents.ask(agent::Harvest { session }).await;
    }
    match harvest {
        Ok(r) => tracing::info!(meeting = %m.id, "harvest: {r:?}"),
        Err(e) => tracing::warn!(meeting = %m.id, "harvest: {e}"),
    }
    // 04 §0.6: the summary is the head of the follow-up conversation, verbatim.
    if let Err(e) = agents
        .ask(agent::Compact {
            session,
            style: Some(agent::CompactStyle::Summary),
            head: Some(summary),
        })
        .await
    {
        tracing::warn!(meeting = %m.id, "compact with summary head: {e}");
    }
    if m.title.is_none() {
        m.title = Some(title.clone());
    }
    Ok(title)
}

/// Run [`finish`] off the mailbox and settle the manifest: `Done` + `Summarized`,
/// or back to `Review` with `failure` set so the user can retry.
pub fn spawn(rt: Runtime, store: Arc<Store>, mut m: Manifest, error: Option<String>) {
    tokio::spawn(async move {
        let r = match error {
            Some(e) => Err(e),
            None => finish(&store, &mut m).await,
        };
        let title = match r {
            Ok(title) => {
                m.state = MeetingState::Done;
                m.failure = None;
                Some(title)
            }
            Err(e) => {
                tracing::warn!(meeting = %m.id, "summary: {e}");
                m.state = MeetingState::Review;
                m.failure = Some(format!("summary: {e}"));
                None
            }
        };
        if let Err(e) = store.save(&m).await {
            tracing::warn!(meeting = %m.id, "manifest: {e:#}");
        }
        rt.publish(MeetingEvent::State {
            meeting: m.id,
            state: m.state,
            degraded: m.degraded.clone(),
        })
        .await;
        if let Some(title) = title {
            rt.publish(MeetingEvent::Summarized {
                meeting: m.id,
                title,
            })
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_the_heading_or_the_first_line() {
        assert_eq!(title_of("\n## Weekly sync  \n\n**Topics**"), "Weekly sync");
        assert_eq!(title_of("Just prose.\nMore."), "Just prose.");
        assert_eq!(title_of("   "), "Meeting summary");
    }
}
