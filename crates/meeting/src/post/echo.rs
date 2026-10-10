//! 158 §13 (W6, W-o): the echo counter. Outside macOS the mic has no AEC, so the far end
//! leaks back in as a *mic* segment whose text duplicates a *system* segment a moment
//! earlier or later. This pass only counts those (`Manifest.stats`); nothing is dropped —
//! the gate (median ratio ≥ 5 % over ≥ 5 speaker meetings) decides whether AEC3 or the bleed
//! check ever ships.

use std::collections::HashSet;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;

use crate::store::Stats;
use crate::transcript::Segment;
use crate::{MEETING_ACTOR, Manifest, Meeting, MeetingId, Track};

/// A mic segment is an echo candidate when it overlaps a system segment widened by this much.
pub const WINDOW_MS: u64 = 2_000;
/// Jaccard over normalised word sets at or above this is "the same text".
pub const THRESHOLD: f32 = 0.8;

/// Count echo duplicates over a meeting's segments (both tracks, any order). `speakers` is
/// left `None`: Review asks the user.
pub fn count(segments: &[Segment]) -> Stats {
    let lowered: Vec<String> = segments.iter().map(|s| s.text.to_lowercase()).collect();
    let words: Vec<HashSet<&str>> = lowered.iter().map(|t| tokens(t)).collect();
    let mut stats = Stats {
        echo_dupes: 0,
        mic_segments: 0,
        speakers: None,
    };
    // ponytail: n² over the cheap time check; a meeting is a few hundred segments.
    for (i, s) in segments.iter().enumerate() {
        if s.track != Track::Mic {
            continue;
        }
        stats.mic_segments += 1;
        let echo = segments.iter().enumerate().any(|(j, o)| {
            o.track == Track::System
                && o.t0_ms <= s.t1_ms + WINDOW_MS
                && s.t0_ms <= o.t1_ms + WINDOW_MS
                && jaccard(&words[i], &words[j]) >= THRESHOLD
        });
        if echo {
            stats.echo_dupes += 1;
        }
    }
    stats
}

/// Alphanumeric words (case as given; [`count`] and [`similar`] lower-case first).
pub fn tokens(text: &str) -> HashSet<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// |a ∩ b| / |a ∪ b|; 0 when both are empty.
pub fn jaccard(a: &HashSet<&str>, b: &HashSet<&str>) -> f32 {
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

/// Jaccard of two texts' lower-cased word sets.
pub fn similar(a: &str, b: &str) -> f32 {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    jaccard(&tokens(&a), &tokens(&b))
}

// ---- Review's "Did you use speakers?" answer (Windows/Linux only in the UI).

/// `ask` → the manifest after `meeting_set_speakers`.
pub(crate) struct SetSpeakers {
    pub meeting: MeetingId,
    pub speakers: bool,
}

impl Message<SetSpeakers> for Meeting {
    type Reply = Result<Manifest, String>;

    async fn handle(&mut self, m: SetSpeakers, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let live = self
            .current
            .as_ref()
            .is_some_and(|r| r.manifest.id == m.meeting);
        let mut manifest = match self.current.as_ref().filter(|_| live) {
            Some(r) => r.manifest.clone(),
            None => self
                .store
                .manifest(m.meeting)
                .map_err(|e| format!("{e:#}"))?,
        };
        let stats = manifest.stats.get_or_insert(Stats {
            echo_dupes: 0,
            mic_segments: 0,
            speakers: None,
        });
        stats.speakers = Some(m.speakers);
        if let Some(rec) = self.current.as_mut().filter(|r| r.manifest.id == m.meeting) {
            rec.manifest = manifest.clone();
        }
        self.save(&manifest).await;
        Ok(manifest)
    }
}

/// `meeting_set_speakers(id, speakers) -> Manifest` (§13, W6): records the one-per-meeting
/// answer the echo gate needs; a manifest without `stats` yet gets zeros + the answer.
pub async fn rpc_set_speakers(
    _rt: &Runtime,
    id: MeetingId,
    speakers: bool,
) -> anyhow::Result<Manifest> {
    let actor = ActorRef::<Meeting>::lookup(MEETING_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("meeting is not running"))?;
    actor
        .ask(SetSpeakers {
            meeting: id,
            speakers,
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: u32, track: Track, t0_ms: u64, t1_ms: u64, text: &str) -> Segment {
        Segment {
            id,
            track,
            speaker: if track == Track::Mic { "me" } else { "s1" }.into(),
            t0_ms,
            t1_ms,
            text: text.into(),
            words: Vec::new(),
        }
    }
    const FAR: &str = "Let's move the launch to Thursday and tell the team.";

    #[test]
    fn exact_duplicate_within_two_seconds_counts() {
        let segs = [
            seg(1, Track::System, 10_000, 13_000, FAR),
            seg(
                2,
                Track::Mic,
                14_500,
                17_000,
                "LET'S MOVE THE LAUNCH TO THURSDAY AND TELL THE TEAM",
            ),
            seg(3, Track::Mic, 30_000, 31_000, "Sounds good to me."),
        ];
        let s = count(&segs);
        assert_eq!((s.echo_dupes, s.mic_segments, s.speakers), (1, 2, None));
    }

    #[test]
    fn outside_the_window_does_not_count() {
        let segs = [
            seg(1, Track::System, 10_000, 13_000, FAR),
            seg(2, Track::Mic, 15_001, 17_000, FAR),
            seg(3, Track::Mic, 5_000, 7_999, FAR),
        ];
        assert_eq!(count(&segs).echo_dupes, 0);
        // Right on the edge still counts.
        let edge = [
            seg(1, Track::System, 10_000, 13_000, FAR),
            seg(2, Track::Mic, 15_000, 17_000, FAR),
        ];
        assert_eq!(count(&edge).echo_dupes, 1);
    }

    #[test]
    fn below_the_threshold_does_not_count() {
        // 7 shared of 15 distinct words ≈ 0.47.
        let segs = [
            seg(1, Track::System, 10_000, 13_000, FAR),
            seg(
                2,
                Track::Mic,
                11_000,
                13_000,
                "Let's move the launch to Friday after we tell everyone else.",
            ),
        ];
        assert_eq!(count(&segs).echo_dupes, 0);
        assert!(similar(FAR, FAR) >= THRESHOLD);
        assert!(similar(FAR, "Let's move the launch to Thursday") < THRESHOLD);
    }

    #[test]
    fn system_segments_and_empty_text_never_count() {
        let segs = [
            seg(1, Track::System, 0, 1_000, FAR),
            seg(2, Track::System, 500, 1_500, FAR),
            seg(3, Track::Mic, 600, 900, ""),
            seg(4, Track::System, 2_000, 3_000, ""),
        ];
        let s = count(&segs);
        assert_eq!((s.echo_dupes, s.mic_segments), (0, 1));
    }
}
