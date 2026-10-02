//! 02 §4.1: the meeting session's compaction head — the meeting's state,
//! rendered deterministically by the plugin and pushed through
//! `agent::SetCompaction{head}` whenever it changes. The transcript of record
//! is the meeting store (`meeting_read`); the session keeps this head plus the
//! last minutes verbatim, and compacts at no model cost.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};

use crate::state::{Prompted, Recording};
use crate::store::Manifest;
use crate::{MeetingId, session};

/// Briefs listed in full; older ones collapse to `… and N earlier`.
const BRIEF_CAP: usize = 20;

/// A live topic as the head prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTopic {
    /// The label as the batch line prints it (`<label> (cached <age>)` for a cached one).
    pub label: String,
    /// First seen, recording clock.
    pub at_ms: u64,
}

/// A brief given this meeting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefLine {
    /// Shown at, recording clock.
    pub at_ms: u64,
    /// The topic's label while it is live, else how the brief came about (`request`, `deeper`, …).
    pub label: String,
    /// The brief's title.
    pub title: String,
}

/// Everything the head is rendered from. Plain data: the same input is the same head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadInput {
    /// For the `meeting_read` pointer.
    pub meeting: MeetingId,
    /// The user's title, else the session's.
    pub title: String,
    /// RFC 3339 local stamp (`Manifest::started_at`); printed as `HH:MM`.
    pub started_at: String,
    /// Recording clock now — the only "now" in the head.
    pub elapsed_ms: u64,
    /// Speakers heard so far, in order of first utterance (`me`, `Speaker N`, or `them`).
    pub speakers: Vec<String>,
    /// Live topics, oldest first.
    pub live: Vec<LiveTopic>,
    /// Dismissed labels, in order.
    pub dismissed: Vec<String>,
    /// Briefs given, in order.
    pub briefs: Vec<BriefLine>,
    /// Unanswered steers (`Live::outstanding`), oldest first.
    pub outstanding: Vec<String>,
}

/// The head at `start_task`: nothing heard yet.
pub fn initial(manifest: &Manifest, session_title: &str) -> HeadInput {
    HeadInput {
        meeting: manifest.id,
        title: manifest
            .title
            .clone()
            .unwrap_or_else(|| session_title.to_owned()),
        started_at: manifest.started_at.clone(),
        elapsed_ms: 0,
        speakers: Vec::new(),
        live: Vec::new(),
        dismissed: Vec::new(),
        briefs: Vec::new(),
        outstanding: Vec::new(),
    }
}

/// The head of a running meeting; `now` ages the cached labels as the batch line does.
pub fn of(rec: &Recording, now: DateTime<Utc>) -> HeadInput {
    let topics = &rec.live.topics;
    let live = topics
        .batch_labels(now)
        .into_iter()
        .zip(topics.live())
        .map(|(label, t)| LiveTopic {
            label,
            at_ms: t.at_ms,
        })
        .collect();
    let briefs = topics
        .briefs()
        .iter()
        .map(|b| BriefLine {
            at_ms: b.at_ms,
            label: b.topic.and_then(|t| topics.topic(t)).map_or_else(
                || format!("{:?}", b.source).to_lowercase(),
                |t| t.label.clone(),
            ),
            title: b.title.clone(),
        })
        .collect();
    let outstanding = rec
        .live
        .outstanding
        .iter()
        .map(|o| match o.prompt {
            Prompted::Tap(_) => format!("tapped \"{}\"", o.text),
            Prompted::Deeper(_) => format!("deeper on \"{}\"", o.text),
            Prompted::Request { spoken: true } => format!("spoken request: {}", o.text),
            Prompted::Request { spoken: false } => format!("typed request: {}", o.text),
        })
        .collect();
    HeadInput {
        meeting: rec.manifest.id,
        title: rec
            .manifest
            .title
            .clone()
            .unwrap_or_else(|| format!("Meeting {}", started_hm(&rec.manifest.started_at))),
        started_at: rec.manifest.started_at.clone(),
        elapsed_ms: rec.clock.now_ms(),
        speakers: rec.live.speakers.clone(),
        live,
        dismissed: topics.dismissed_labels().to_vec(),
        briefs,
        outstanding,
    }
}

/// `HH:MM` of an RFC 3339 stamp in its own offset; unparseable → as given.
fn started_hm(stamp: &str) -> String {
    DateTime::parse_from_rfc3339(stamp)
        .map(|d| d.format("%H:%M").to_string())
        .unwrap_or_else(|_| stamp.to_owned())
}

/// `a, b` or `none`.
fn list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

/// The head body (markdown). Stable ordering, no wall-clock "now": the same
/// input renders the same text, so an unchanged state pushes nothing.
pub fn render(input: &HeadInput) -> String {
    let mut out = String::with_capacity(1024);
    let id = input.meeting;
    let _ = writeln!(out, "# Meeting: {}", input.title);
    let _ = writeln!(
        out,
        "Started {} local · elapsed {} · id {id}",
        started_hm(&input.started_at),
        session::hms(input.elapsed_ms)
    );
    let _ = writeln!(out, "Speakers heard: {}", list(&input.speakers));
    out.push_str("\nLive topics:");
    if input.live.is_empty() {
        out.push_str(" none\n");
    } else {
        out.push('\n');
        for t in &input.live {
            let _ = writeln!(out, "- {} — since {}", t.label, session::hms(t.at_ms));
        }
    }
    let _ = writeln!(out, "Dismissed: {}", list(&input.dismissed));
    let _ = write!(out, "\nBriefs given ({}):", input.briefs.len());
    if input.briefs.is_empty() {
        out.push_str(" none\n");
    } else {
        out.push('\n');
        let earlier = input.briefs.len().saturating_sub(BRIEF_CAP);
        if earlier > 0 {
            let _ = writeln!(out, "… and {earlier} earlier");
        }
        for b in &input.briefs[earlier..] {
            let _ = writeln!(out, "- {} {}: {}", session::hms(b.at_ms), b.label, b.title);
        }
    }
    out.push_str("\nOutstanding:");
    if input.outstanding.is_empty() {
        out.push_str(" none\n");
    } else {
        out.push('\n');
        for o in &input.outstanding {
            let _ = writeln!(out, "- {o}");
        }
    }
    let _ = write!(
        out,
        "\nEarlier transcript (before the lines that follow): read it with \
         meeting_read(meeting={id}, to_ms={}) when a question needs it.\n\
         Do not re-offer a live or dismissed topic; reply with no text unless asked.",
        input.elapsed_ms
    );
    out
}

/// Push only when the head differs from the last one pushed.
pub fn changed(last: Option<&str>, next: &str) -> bool {
    last != Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> HeadInput {
        HeadInput {
            meeting: MeetingId(uuid::Uuid::nil()),
            title: "Roadmap sync".into(),
            started_at: "2026-10-02T09:30:12+02:00".into(),
            elapsed_ms: 754_000,
            speakers: ["me", "Speaker 1", "Speaker 2"].map(String::from).to_vec(),
            live: vec![
                LiveTopic {
                    label: "Q4 roadmap".into(),
                    at_ms: 61_000,
                },
                LiveTopic {
                    label: "ACME-1234 (cached 2h)".into(),
                    at_ms: 300_000,
                },
            ],
            dismissed: vec!["lunch".into()],
            briefs: vec![
                BriefLine {
                    at_ms: 61_000,
                    label: "Q4 roadmap".into(),
                    title: "Q4 roadmap: three themes".into(),
                },
                BriefLine {
                    at_ms: 400_000,
                    label: "request".into(),
                    title: "Headcount plan".into(),
                },
            ],
            outstanding: vec!["spoken request: what did we promise sales".into()],
        }
    }

    #[test]
    fn golden() {
        let expected = "\
# Meeting: Roadmap sync
Started 09:30 local · elapsed 00:12:34 · id 00000000-0000-0000-0000-000000000000
Speakers heard: me, Speaker 1, Speaker 2

Live topics:
- Q4 roadmap — since 00:01:01
- ACME-1234 (cached 2h) — since 00:05:00
Dismissed: lunch

Briefs given (2):
- 00:01:01 Q4 roadmap: Q4 roadmap: three themes
- 00:06:40 request: Headcount plan

Outstanding:
- spoken request: what did we promise sales

Earlier transcript (before the lines that follow): read it with meeting_read(meeting=00000000-0000-0000-0000-000000000000, to_ms=754000) when a question needs it.
Do not re-offer a live or dismissed topic; reply with no text unless asked.";
        let head = render(&input());
        assert_eq!(head, expected);
        assert!(head.len() < 1_500, "{}", head.len());
    }

    #[test]
    fn empty_state_prints_none_and_the_fixed_tail() {
        let head = render(&HeadInput {
            speakers: Vec::new(),
            live: Vec::new(),
            dismissed: Vec::new(),
            briefs: Vec::new(),
            outstanding: Vec::new(),
            ..input()
        });
        assert!(head.contains("Speakers heard: none\n"));
        assert!(head.contains("Live topics: none\n"));
        assert!(head.contains("Briefs given (0): none\n"));
        assert!(head.contains("Outstanding: none\n"));
        assert!(head.ends_with("reply with no text unless asked."));
    }

    #[test]
    fn briefs_cap_at_the_last_twenty() {
        let mut i = input();
        i.briefs = (0..23u64)
            .map(|n| BriefLine {
                at_ms: n * 1_000,
                label: "t".into(),
                title: format!("brief {n}"),
            })
            .collect();
        let head = render(&i);
        assert!(head.contains("Briefs given (23):\n… and 3 earlier\n- 00:00:03 t: brief 3\n"));
        assert!(!head.contains("brief 2\n"));
        assert!(head.contains("- 00:00:22 t: brief 22\n"));
    }

    /// A topic change pushes a new head; an unchanged state does not.
    #[test]
    fn only_a_changed_state_pushes() {
        let a = render(&input());
        assert_eq!(a, render(&input()));
        assert!(!changed(Some(&a), &a));
        assert!(changed(None, &a));
        let mut i = input();
        i.live.push(LiveTopic {
            label: "budget".into(),
            at_ms: 700_000,
        });
        assert!(changed(Some(&a), &render(&i)));
        let mut i = input();
        i.dismissed.push("budget".into());
        assert!(changed(Some(&a), &render(&i)));
    }
}
