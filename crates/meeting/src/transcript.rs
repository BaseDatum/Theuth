//! The transcript of record (04 §3): words with times from the post pipeline,
//! grouped into speaker segments, plus the review edits — one op enum, one
//! `apply`, one rewrite of `transcript.json`. Pure; unit-tested.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::speakers::{ProfileId, Profiles};
use crate::{MeetingId, Track};

/// File name inside a meeting's directory.
pub const FILE: &str = "transcript.json";
/// Per-label mean voice embeddings, beside the transcript; what `Enroll` reads.
pub const EMBEDDINGS: &str = "embeddings.json";
/// The mic track's label.
pub const ME: &str = "me";

/// One word with its times on the recording clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Word {
    /// The word (punctuation attached).
    pub text: String,
    /// Start.
    pub t0_ms: u64,
    /// End.
    pub t1_ms: u64,
}

/// Consecutive words of one speaker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Segment {
    /// Stable within the transcript; `Split` mints new ones.
    pub id: u32,
    /// Which file the words came from.
    pub track: Track,
    /// A [`SpeakerLabel::id`].
    pub speaker: String,
    /// Start.
    pub t0_ms: u64,
    /// End.
    pub t1_ms: u64,
    /// The text; `FixText` replaces it while the words keep their times.
    pub text: String,
    /// The words as transcribed.
    pub words: Vec<Word>,
}

/// A speaker as the review pane shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SpeakerLabel {
    /// `me` for the mic track, `s1`, `s2`, … for the system clusters.
    pub id: String,
    /// Display name: `Me`, `Speaker 1`, or what the user renamed/enrolled it to.
    pub name: String,
    /// Enrolled profile this label is linked to.
    pub profile: Option<ProfileId>,
    /// Best profile match ≥ 0.6 (04 §0.3); proposed, never applied without the user.
    pub proposed: Option<Proposed>,
}

/// A profile match the user may accept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Proposed {
    /// The profile.
    pub profile: ProfileId,
    /// Its name at match time.
    pub name: String,
    /// Cosine similarity.
    pub score: f32,
}

/// `transcript.json`: derived until `reviewed_at` is set, truth after (01 §5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
// `MeetingTranscript` on the wire: `Transcript.ts` is the voice note's reply.
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "MeetingTranscript")
)]
pub struct Transcript {
    /// The meeting.
    pub meeting: MeetingId,
    /// Labels; `me` first.
    pub speakers: Vec<SpeakerLabel>,
    /// Sorted by `t0_ms` across both tracks.
    pub segments: Vec<Segment>,
    /// RFC 3339, set by the `Reviewed` op.
    pub reviewed_at: Option<String>,
    /// RFC 3339, set when the summary turn finished.
    pub summarized_at: Option<String>,
    /// Diarization ran; `false` → every system segment is `s1` and `note` says why.
    pub diarized: bool,
    /// Why diarization was skipped, for the review pane.
    #[serde(default)]
    pub note: Option<String>,
}

/// One review edit (04 §0.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "op")]
pub enum EditOp {
    /// Move segments to an existing label.
    Relabel {
        /// Segment ids.
        segments: Vec<u32>,
        /// Existing label id.
        speaker: String,
    },
    /// `Speaker 2` → `John`.
    Rename {
        /// Label id.
        speaker: String,
        /// New display name.
        name: String,
    },
    /// Join adjacent segments (in transcript order) into the first; its speaker wins.
    Merge {
        /// Segment ids, at least two, consecutive in the transcript.
        segments: Vec<u32>,
    },
    /// Cut a segment before word `at_word` (`0 < at_word < words.len()`).
    Split {
        /// Segment id.
        segment: u32,
        /// Word index the second half starts at.
        at_word: u32,
    },
    /// Replace a segment's text; the words keep their times.
    FixText {
        /// Segment id.
        segment: u32,
        /// New text.
        text: String,
    },
    /// Make (or update) a profile from this label's voice and link the label to it.
    Enroll {
        /// Label id.
        speaker: String,
        /// Profile name; an existing profile of that name takes the running mean.
        name: String,
        /// Knowledge entity id, if the user picked one.
        entity: Option<String>,
    },
    /// The user is done: sets `reviewed_at`.
    Reviewed,
}

/// `Speaker N` / `Me` display name for a fresh label.
pub fn default_name(id: &str) -> String {
    match id.strip_prefix('s').and_then(|n| n.parse::<u32>().ok()) {
        Some(n) => format!("Speaker {n}"),
        None => "Me".to_owned(),
    }
}

/// `mm:ss` (hours roll into minutes: `61:05`).
pub fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl Transcript {
    /// Display name of a label id (the id itself when unknown).
    pub fn name<'a>(&'a self, speaker: &'a str) -> &'a str {
        self.speakers
            .iter()
            .find(|s| s.id == speaker)
            .map_or(speaker, |s| s.name.as_str())
    }

    /// `[mm:ss] Name: text` lines, optionally filtered (04 §3).
    pub fn lines(
        &self,
        from_ms: Option<u64>,
        to_ms: Option<u64>,
        speaker: Option<&str>,
    ) -> impl Iterator<Item = String> + '_ {
        let speaker = speaker.map(str::to_lowercase);
        self.segments
            .iter()
            .filter(move |s| from_ms.is_none_or(|f| s.t1_ms >= f))
            .filter(move |s| to_ms.is_none_or(|t| s.t0_ms <= t))
            .filter(move |s| {
                speaker.as_deref().is_none_or(|sp| {
                    s.speaker.eq_ignore_ascii_case(sp)
                        || self.name(&s.speaker).eq_ignore_ascii_case(sp)
                })
            })
            .map(|s| format!("[{}] {}: {}", mmss(s.t0_ms), self.name(&s.speaker), s.text))
    }

    fn index(&self, id: u32) -> Result<usize, String> {
        self.segments
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| format!("no segment {id}"))
    }

    fn label_mut(&mut self, id: &str) -> Result<&mut SpeakerLabel, String> {
        self.speakers
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("unknown speaker label {id:?}"))
    }
}

/// Apply `ops` in order; the first failure returns `Err` and leaves `t`
/// partially edited — callers apply to a copy and keep it only on `Ok`.
/// `embeddings` are the per-label means from the pipeline (`Enroll` needs one).
pub fn apply(
    t: &mut Transcript,
    ops: &[EditOp],
    embeddings: &HashMap<String, Vec<f32>>,
    profiles: &mut Profiles,
) -> Result<(), String> {
    for op in ops {
        match op {
            EditOp::Relabel { segments, speaker } => {
                t.label_mut(speaker)?;
                for id in segments {
                    let i = t.index(*id)?;
                    t.segments[i].speaker = speaker.clone();
                }
            }
            EditOp::Rename { speaker, name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err("empty name".into());
                }
                t.label_mut(speaker)?.name = name.to_owned();
            }
            EditOp::Merge { segments } => {
                if segments.len() < 2 {
                    return Err("merge needs at least two segments".into());
                }
                let mut idx: Vec<usize> = segments
                    .iter()
                    .map(|id| t.index(*id))
                    .collect::<Result<_, _>>()?;
                idx.sort_unstable();
                if idx.windows(2).any(|w| w[1] != w[0] + 1) {
                    return Err("segments to merge must be adjacent".into());
                }
                let first = idx[0];
                let rest: Vec<Segment> = t.segments.drain(first + 1..=idx[idx.len() - 1]).collect();
                let head = &mut t.segments[first];
                for s in rest {
                    if s.track != head.track {
                        return Err("segments to merge must be on one track".into());
                    }
                    head.t1_ms = head.t1_ms.max(s.t1_ms);
                    if !s.text.is_empty() {
                        if !head.text.is_empty() {
                            head.text.push(' ');
                        }
                        head.text.push_str(&s.text);
                    }
                    head.words.extend(s.words);
                }
            }
            EditOp::Split { segment, at_word } => {
                let i = t.index(*segment)?;
                let at = *at_word as usize;
                let n = t.segments[i].words.len();
                if at == 0 || at >= n {
                    return Err(format!("split index {at} out of range 1..{n}"));
                }
                let next_id = t.segments.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                let s = &mut t.segments[i];
                let tail: Vec<Word> = s.words.split_off(at);
                let join = |ws: &[Word]| {
                    ws.iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                s.text = join(&s.words);
                s.t1_ms = s.words.last().map_or(s.t1_ms, |w| w.t1_ms);
                let second = Segment {
                    id: next_id,
                    track: s.track,
                    speaker: s.speaker.clone(),
                    t0_ms: tail[0].t0_ms,
                    t1_ms: tail.last().map_or(tail[0].t1_ms, |w| w.t1_ms),
                    text: join(&tail),
                    words: tail,
                };
                t.segments.insert(i + 1, second);
            }
            EditOp::FixText { segment, text } => {
                let text = text.trim();
                if text.is_empty() {
                    return Err("empty text".into());
                }
                let i = t.index(*segment)?;
                t.segments[i].text = text.to_owned();
            }
            EditOp::Enroll {
                speaker,
                name,
                entity,
            } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err("empty name".into());
                }
                let emb = embeddings.get(speaker).ok_or_else(|| {
                    format!("no voice embedding for {speaker:?} (diarization did not run)")
                })?;
                let id = profiles.enroll(name, entity.clone(), emb)?;
                let label = t.label_mut(speaker)?;
                label.name = name.to_owned();
                label.profile = Some(id);
                label.proposed = None;
            }
            EditOp::Reviewed => t.reviewed_at = Some(chrono::Local::now().to_rfc3339()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, t0: u64, t1: u64) -> Word {
        Word {
            text: text.into(),
            t0_ms: t0,
            t1_ms: t1,
        }
    }

    fn seg(id: u32, track: Track, speaker: &str, words: &[Word]) -> Segment {
        Segment {
            id,
            track,
            speaker: speaker.into(),
            t0_ms: words[0].t0_ms,
            t1_ms: words[words.len() - 1].t1_ms,
            text: words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            words: words.to_vec(),
        }
    }

    fn label(id: &str) -> SpeakerLabel {
        SpeakerLabel {
            id: id.into(),
            name: default_name(id),
            profile: None,
            proposed: None,
        }
    }

    fn transcript() -> Transcript {
        Transcript {
            meeting: MeetingId::new(),
            speakers: vec![label(ME), label("s1"), label("s2")],
            segments: vec![
                seg(
                    0,
                    Track::System,
                    "s1",
                    &[w("Hello", 0, 400), w("there.", 450, 900)],
                ),
                seg(1, Track::Mic, ME, &[w("Hi.", 1000, 1300)]),
                seg(
                    2,
                    Track::System,
                    "s2",
                    &[
                        w("The", 2000, 2100),
                        w("tags", 2150, 2500),
                        w("broke.", 2600, 3000),
                    ],
                ),
                seg(3, Track::System, "s2", &[w("Again.", 3200, 3600)]),
            ],
            reviewed_at: None,
            summarized_at: None,
            diarized: true,
            note: None,
        }
    }

    fn profiles() -> (tempfile::TempDir, Profiles) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Profiles::load(tmp.path()).unwrap();
        (tmp, p)
    }

    fn run(t: &mut Transcript, ops: &[EditOp]) -> Result<(), String> {
        let (_tmp, mut p) = profiles();
        apply(t, ops, &HashMap::new(), &mut p)
    }

    #[test]
    fn relabel_rename_fix_and_reviewed() {
        let mut t = transcript();
        run(
            &mut t,
            &[
                EditOp::Relabel {
                    segments: vec![2, 3],
                    speaker: "s1".into(),
                },
                EditOp::Rename {
                    speaker: "s1".into(),
                    name: "Dana".into(),
                },
                EditOp::FixText {
                    segment: 2,
                    text: "The image tags broke.".into(),
                },
                EditOp::Reviewed,
            ],
        )
        .unwrap();
        assert!(t.segments.iter().filter(|s| s.speaker == "s1").count() == 3);
        assert_eq!(t.name("s1"), "Dana");
        assert_eq!(t.segments[2].text, "The image tags broke.");
        assert_eq!(t.segments[2].words.len(), 3, "words keep their times");
        assert!(t.reviewed_at.is_some());
        let lines: Vec<String> = t.lines(None, None, Some("dana")).collect();
        assert_eq!(lines[0], "[00:00] Dana: Hello there.");
        assert_eq!(lines.len(), 3);
        assert_eq!(t.lines(Some(2000), Some(2100), None).count(), 1);
    }

    #[test]
    fn merge_and_split() {
        let mut t = transcript();
        run(
            &mut t,
            &[EditOp::Merge {
                segments: vec![3, 2],
            }],
        )
        .unwrap();
        assert_eq!(t.segments.len(), 3);
        let m = &t.segments[2];
        assert_eq!((m.id, m.t0_ms, m.t1_ms), (2, 2000, 3600));
        assert_eq!(m.text, "The tags broke. Again.");
        assert_eq!(m.words.len(), 4);
        run(
            &mut t,
            &[EditOp::Split {
                segment: 2,
                at_word: 3,
            }],
        )
        .unwrap();
        assert_eq!(t.segments.len(), 4);
        assert_eq!(t.segments[2].text, "The tags broke.");
        assert_eq!(t.segments[2].t1_ms, 3000);
        let s = &t.segments[3];
        assert_eq!(
            (s.id, s.t0_ms, s.t1_ms, s.speaker.as_str()),
            (3, 3200, 3600, "s2")
        );
        assert_eq!(s.text, "Again.");
    }

    #[test]
    fn rejects() {
        let t = transcript();
        let bad = [
            EditOp::Merge {
                segments: vec![0, 2],
            },
            EditOp::Merge { segments: vec![0] },
            EditOp::Merge {
                segments: vec![0, 1],
            },
            EditOp::Relabel {
                segments: vec![0],
                speaker: "s9".into(),
            },
            EditOp::Relabel {
                segments: vec![42],
                speaker: "s1".into(),
            },
            EditOp::Split {
                segment: 2,
                at_word: 0,
            },
            EditOp::Split {
                segment: 2,
                at_word: 3,
            },
            EditOp::FixText {
                segment: 0,
                text: "  ".into(),
            },
            EditOp::Rename {
                speaker: "s1".into(),
                name: "".into(),
            },
            EditOp::Enroll {
                speaker: "s1".into(),
                name: "John".into(),
                entity: None,
            },
        ];
        for op in bad {
            let mut c = t.clone();
            assert!(run(&mut c, std::slice::from_ref(&op)).is_err(), "{op:?}");
        }
    }

    #[test]
    fn enroll_links_the_label_and_updates_a_named_profile() {
        let (_tmp, mut p) = profiles();
        let mut emb = HashMap::new();
        emb.insert("s2".to_owned(), vec![1.0, 0.0, 0.0]);
        emb.insert("s1".to_owned(), vec![0.0, 1.0, 0.0]);
        let mut t = transcript();
        t.speakers[2].proposed = Some(Proposed {
            profile: ProfileId::new(),
            name: "x".into(),
            score: 0.7,
        });
        apply(
            &mut t,
            &[EditOp::Enroll {
                speaker: "s2".into(),
                name: "John".into(),
                entity: Some("people/john".into()),
            }],
            &emb,
            &mut p,
        )
        .unwrap();
        let john = t.speakers[2].profile.expect("linked");
        assert_eq!(t.name("s2"), "John");
        assert!(t.speakers[2].proposed.is_none());
        assert_eq!(p.list().len(), 1);
        // Same name from another label: the running mean, not a second John.
        apply(
            &mut t,
            &[EditOp::Enroll {
                speaker: "s1".into(),
                name: "john".into(),
                entity: None,
            }],
            &emb,
            &mut p,
        )
        .unwrap();
        assert_eq!(t.speakers[1].profile, Some(john));
        assert_eq!(p.list().len(), 1);
        assert_eq!(p.list()[0].samples, 2);
        assert_eq!(
            p.list()[0].entity.as_deref(),
            Some("people/john"),
            "entity kept"
        );
    }
}
