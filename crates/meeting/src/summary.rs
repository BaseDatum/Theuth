//! The guided summary (04 §6, 53 decision 7, 59 §7.4): one normal turn in the
//! meeting's session — the form as a `SendInput` — then `summary.md`, the
//! title, `knowledge_harvest`, and a compaction with the summary as head so the
//! follow-up conversation starts from it and reaches the record through the
//! tools. A meeting with a note (`Manifest::note`) has the turn *edit the note*
//! with the `doc_*` tools and reply with the title only; `summary.md` is then
//! the note's derived export. Older meetings keep the reply-is-the-summary path.

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};

use crate::store::{Manifest, SUMMARY, Store};
use crate::templates::{self, Template};
use crate::{MeetingEvent, MeetingState, note};

/// The form's marker: [`finished_turn`] finds the summary turn in the history by it.
pub const SUMMARY_HEAD: &str = "[meeting summary]";

/// The pre-158 fixed form for meetings without a note; kept as the fixture text tests send
/// (`holds_summary` keys on [`SUMMARY_HEAD`]). The turn itself sends [`form`].
pub const SUMMARY_FORM: &str = "[meeting summary] The meeting has ended and the transcript was \
reviewed. Use `meeting_read` (all of it, in pieces) and `meeting_speakers`, then write the summary \
as markdown: a `#` title line (at most 8 words), then **Topics** (what was discussed, in order, one \
line each), **Decisions** (with who), **Action items** (owner — item — due if said), **Open \
questions**, **Numbers and names said aloud**. Improvise sections if the meeting calls for it; keep \
it under 400 words. Reply with the markdown only.";

/// The form for a meeting without a note (recorded before 59 step 5), compiled from its
/// template (158 §11.1) with the rules block (§11.2).
pub fn form(t: &Template, duration_ms: Option<u64>, preferred: &[String]) -> String {
    format!(
        "{SUMMARY_HEAD} The meeting has ended and the transcript was reviewed. Use `meeting_read` \
         (all of it, in pieces) and `meeting_speakers`, then write the summary as markdown: a `#` \
         title line (at most 8 words), then these sections, each as a `## ` heading followed by its \
         content: {}. Reply with the markdown only.\n\n{}",
        note::sections(t),
        note::rules(t, duration_ms, preferred)
    )
}

/// The form for `m`: its template (`Manifest.template`, the file under
/// `<data_dir>/meetings/templates/` else the shipped body) compiled into the note contract when
/// it has a note, else [`form`]; both carry the dictionary's preferred spellings.
pub fn form_for(m: &Manifest) -> String {
    let t = templates::load(templates::dir().as_deref(), &m.template);
    let preferred = note::preferred_names(templates::data_dir());
    match &m.note {
        Some(doc) => note::summary_form(doc, &t, m.duration_ms, &preferred),
        None => form(&t, m.duration_ms, &preferred),
    }
}

/// The title the note turn replied with: its first non-empty line when it looks like one (≤ 120 chars,
/// a `#` allowed), else `None` — the caller falls back to `title_of(derived)`.
fn reply_title(reply: &str) -> Option<String> {
    let line = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .trim_start_matches('#')
        .trim();
    (!line.is_empty() && line.chars().count() <= 120 && !line.contains("`"))
        .then(|| line.to_owned())
}

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

/// Restart recovery (04 §2): whether `session` is idle and its history already
/// holds a summary turn — a user row starting with [`SUMMARY_HEAD`] followed by a
/// non-empty assistant reply — so [`spawn`] can settle on it without a new turn.
pub async fn finished_turn(session: SessionId) -> Result<bool, String> {
    let loaded = agents()?
        .ask(agent::LoadSession {
            session,
            before: None,
            limit: Some(8),
            settled_only: false,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(!loaded.running && holds_summary(&loaded.history))
}

/// The tail of a history holds a finished summary turn: the last user row starting with
/// [`SUMMARY_HEAD`] has a non-empty assistant reply after it.
fn holds_summary(history: &[agent::HistoryEntry]) -> bool {
    let form = history.iter().rposition(
        |h| matches!(h, agent::HistoryEntry::User { text, .. } if text.starts_with(SUMMARY_HEAD)),
    );
    form.is_some_and(|at| {
        history[at..].iter().any(
            |h| matches!(h, agent::HistoryEntry::Assistant { text, .. } if !text.trim().is_empty()),
        )
    })
}

/// After the summary turn finished: file, title, harvest, compaction, `Done`.
async fn finish(store: &Store, m: &mut Manifest) -> Result<String, String> {
    let session = m.session;
    let reply = last_reply(session).await?;
    let (summary, title) = match &m.note {
        // 59 §7.4: the note holds the summary; snapshot it, move its export to `summary.md`.
        Some(doc) => {
            note::snapshot(doc, note::SUMMARY).await?;
            let markdown = note::markdown(doc).await?;
            note::set_summary_path(doc, m.id).await?;
            let title = reply_title(&reply).unwrap_or_else(|| title_of(&markdown));
            (markdown, title)
        }
        None => {
            let title = title_of(&reply);
            let path = store.meeting_dir(m.id).join(SUMMARY);
            std::fs::write(&path, &reply).map_err(|e| format!("write {}: {e}", path.display()))?;
            (reply, title)
        }
    };
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
    if harvest
        .as_ref()
        .is_err_and(|e| e.to_string().contains("turn in flight"))
    {
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
        assert!(SUMMARY_FORM.starts_with(SUMMARY_HEAD));
    }

    /// §11.4: the no-note form compiles too — head, template sections, rules; `form_for` picks
    /// by `Manifest.note` and falls back to the shipped `meeting` body without a data dir.
    #[test]
    fn no_note_form_compiles_the_template() {
        let t = templates::load(None, "standup");
        let f = form(&t, Some(5 * 60_000), &["GIDA".into()]);
        assert!(f.starts_with(SUMMARY_HEAD));
        assert!(f.contains("`## Done since last time` (one line per person, by name or label)"));
        assert!(f.contains("\n7. ") && f.contains("at most 120 words"));
        assert!(f.contains("Preferred spellings: GIDA"));
        let mut m = Manifest::new(
            crate::MeetingId::new(),
            crate::ProcessInfo {
                pid: 1,
                name: "zoom".into(),
                bundle_id: None,
                active: false,
            },
            "2026-10-10T10:00:00Z".into(),
        );
        m.duration_ms = Some(90 * 60_000);
        let f = form_for(&m);
        assert!(f.contains("`## Numbers and names said aloud`") && f.contains("at most 400 words"));
        assert!(!f.contains("doc_insert"));
        m.note = Some(document::DocId::from_uuid(uuid::Uuid::nil()));
        m.template = "one-on-one".into();
        let f = form_for(&m);
        assert!(f.contains("doc_insert") && f.contains("`## Follow-ups for next time`"));
    }

    #[test]
    fn reply_title_is_one_plain_line_or_nothing() {
        assert_eq!(reply_title("# Weekly sync\n"), Some("Weekly sync".into()));
        assert_eq!(
            reply_title("\n  Budget review  \n"),
            Some("Budget review".into())
        );
        assert_eq!(reply_title("I ran `doc_insert` and it failed"), None);
        assert_eq!(reply_title(&"x".repeat(121)), None);
        assert_eq!(reply_title("  \n"), None);
    }

    #[test]
    fn history_holds_a_summary_only_with_a_reply_after_the_form() {
        let user = |text: &str| agent::HistoryEntry::User {
            text: text.into(),
            turn: None,
            context: None,
            peer: None,
            vision: None,
        };
        let reply = |text: &str| agent::HistoryEntry::Assistant {
            text: text.into(),
            turn: None,
            context: None,
        };
        assert!(!holds_summary(&[]));
        assert!(!holds_summary(&[
            user("[meeting batch] hi"),
            reply("a brief")
        ]));
        assert!(
            !holds_summary(&[reply("earlier"), user(SUMMARY_FORM)]),
            "form sent, no reply yet"
        );
        assert!(
            !holds_summary(&[user(SUMMARY_FORM), reply("   ")]),
            "an empty reply is none"
        );
        assert!(holds_summary(&[
            reply("earlier"),
            user(SUMMARY_FORM),
            reply("# Sync\n**Topics**")
        ]));
    }
}
