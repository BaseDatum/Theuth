//! Speaker-name suggestions (158 §10): `meeting_suggest_speakers` as a read-only tool and RPC.
//! Candidates = the event's attendees minus `is_me` (else enrolled profiles + intro cues);
//! sources = voiceprint (≥ `speakers::MATCH`, margin `speakers::MARGIN`, mutual),
//! self-introductions, vocatives, elimination, 1:1, plus a merged-speaker warning. Never applies
//! anything; the Review page's Accept does. `me` is the user and never in the output (#245).
//!
//! Also the enrollment gates (§10.4) the automatic "Accept & enroll" path runs before
//! embedding: [`gate_level`], [`gate_agreement`], [`embed_spans_gated`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::calendar::EventRef;
use crate::speakers::{Profiles, cosine};
use crate::transcript::{ME, Transcript, mmss};
use crate::{MeetingId, ProfileId, Track};

use super::{diarize, transcribe};

/// Where a suggestion came from (§10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "SpeakerSuggestionSource")
)]
pub enum SuggestionSource {
    /// Cosine against an enrolled profile.
    Voiceprint,
    /// "I'm <Name>" on the system track in the first 10 minutes.
    Intro,
    /// "<Name>, …" / "thanks <Name>" naming the next speaker.
    Vocative,
    /// One unassigned label, one unassigned candidate.
    Elimination,
    /// One system label, one remote attendee.
    OneOnOne,
    /// Diarization may have merged people; no name attached.
    Warning,
}

/// One proposed name for one speaker label (§10.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "SpeakerSuggestion")
)]
pub struct Suggestion {
    /// Transcript label (`s1`…), never `me`.
    pub label: String,
    /// Proposed name.
    pub name: String,
    /// Attendee mail when known.
    #[serde(default)]
    pub email: Option<String>,
    /// Enrolled profile to link on accept, when the source was a voiceprint.
    #[serde(default)]
    pub profile: Option<ProfileId>,
    /// 0–1.
    pub confidence: f32,
    /// Source.
    pub source: SuggestionSource,
    /// One line the chip shows (`voiceprint 0.82`, `also matched s3 at 0.61`, …).
    pub evidence: String,
}

/// Self-introductions count only this far into the meeting.
pub const INTRO_WINDOW_MS: u64 = 10 * 60_000;
/// A vocative names the next label to speak within this long.
pub const VOCATIVE_GAP_MS: u64 = 5_000;
/// Suggestions under this are dropped (§10.2 step 6).
pub const MIN_CONFIDENCE: f32 = 0.5;
/// §10.2 step 7: one label over this share of system speech with ≥ [`WARN_ATTENDEES`] remote attendees.
pub const WARN_SHARE: f32 = 0.8;
/// See [`WARN_SHARE`].
pub const WARN_ATTENDEES: usize = 4;
/// §10.2 step 7's chip text.
pub const WARNING: &str =
    "one speaker label covers most of the meeting; diarization may have merged people";

const INTRO: f32 = 0.8;
const VOCATIVE: f32 = 0.5;
const ELIMINATION: f32 = 0.6;
const ONE_ON_ONE: f32 = 0.9;

/// `I'm Dana`, `this is Dana`, `my name is Dana`, `it's Dana` — the token after the cue.
static INTRO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:i'?m|this is|my name is|it'?s)\s+([\p{L}][\p{L}'’-]*)").unwrap()
});
/// `Dana, …` at the start; `…, Dana.` / `… thanks Dana.` at the end.
static VOCATIVE_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([\p{L}][\p{L}'’-]*)\s*,").unwrap());
static VOCATIVE_END_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:,|\bthanks?(?:\s+you)?,?)\s+([\p{L}][\p{L}'’-]*)\s*[.!?…]*\s*$").unwrap()
});

/// Capitalised words after an intro cue that are not names (no-event mode only; with an event
/// the attendee list is the filter — Minutes' "refuse common words", E `:55`).
const NOT_NAMES: &[&str] = &[
    "not",
    "so",
    "ok",
    "okay",
    "just",
    "going",
    "good",
    "fine",
    "here",
    "there",
    "sure",
    "yes",
    "no",
    "sorry",
    "done",
    "all",
    "still",
    "also",
    "back",
    "on",
    "in",
    "at",
    "the",
    "a",
    "about",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "today",
    "tomorrow",
    "time",
    "late",
    "early",
    "happy",
    "glad",
    "afraid",
    "right",
    "wrong",
];

/// Someone the labels may be: an attendee, an enrolled profile or a name heard in an intro.
#[derive(Clone, Debug)]
struct Candidate {
    name: String,
    email: Option<String>,
}

impl Candidate {
    fn first(&self) -> String {
        self.name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase()
    }
}

/// `Dana` ↔ `Dana Whitfield`, `dana whitfield` ↔ `Dana Whitfield`; never `Dana` ↔ `Dana Smith`
/// when both are full names that differ.
fn same_person(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    if a == b {
        return true;
    }
    let (fa, fb) = (a.split_whitespace().next(), b.split_whitespace().next());
    fa.is_some() && fa == fb && (!a.contains(' ') || !b.contains(' '))
}

/// Compute the suggestions for the post transcript `t` (live labels never reach here: 07 §5
/// renumbers them). `embeddings` is `embeddings.json`; `event` the matched calendar event.
pub fn suggest(
    t: &Transcript,
    embeddings: &HashMap<String, Vec<f32>>,
    profiles: &Profiles,
    event: Option<&EventRef>,
) -> Vec<Suggestion> {
    // 1–2. Owner rule and the candidate set.
    let labels: Vec<&str> = t
        .speakers
        .iter()
        .map(|s| s.id.as_str())
        .filter(|id| *id != ME)
        .collect();
    if labels.is_empty() {
        return Vec::new();
    }
    let attendees: Vec<Candidate> = event
        .map(|e| {
            e.attendees
                .iter()
                .filter(|a| !a.is_me && !a.name.trim().is_empty())
                .map(|a| Candidate {
                    name: a.name.trim().to_owned(),
                    email: a.email.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let with_event = event.is_some();
    let mut candidates: Vec<Candidate> = if with_event {
        attendees.clone()
    } else {
        profiles
            .list()
            .into_iter()
            .map(|p| Candidate {
                name: p.name,
                email: None,
            })
            .collect()
    };
    let mut out: Vec<Suggestion> = Vec::new();

    // 7. Warning: one label carrying most of the system speech in a crowded call.
    let speech = |label: &str| -> u64 {
        t.segments
            .iter()
            .filter(|g| g.track == Track::System && g.speaker == label)
            .map(|g| g.t1_ms.saturating_sub(g.t0_ms))
            .sum()
    };
    let total: u64 = labels.iter().map(|l| speech(l)).sum();
    let mut warned: Option<&str> = None;
    if attendees.len() >= WARN_ATTENDEES && total > 0 {
        warned = labels
            .iter()
            .copied()
            .find(|l| speech(l) as f32 / total as f32 > WARN_SHARE);
        if let Some(l) = warned {
            out.push(Suggestion {
                label: l.to_owned(),
                name: String::new(),
                email: None,
                profile: None,
                confidence: 1.0,
                source: SuggestionSource::Warning,
                evidence: WARNING.to_owned(),
            });
        }
    }

    // 4. Voiceprints: labels with an embedding, profiles that are a candidate (with an event).
    let allowed: Option<Vec<ProfileId>> = with_event.then(|| {
        profiles
            .list()
            .into_iter()
            .filter(|p| attendees.iter().any(|a| same_person(&a.name, &p.name)))
            .map(|p| p.id)
            .collect()
    });
    let with_emb: Vec<(&str, &[f32])> = labels
        .iter()
        .copied()
        .filter(|l| warned != Some(*l))
        .filter_map(|l| embeddings.get(l).map(|e| (l, e.as_slice())))
        .collect();
    let mut proposals: Vec<Suggestion> = profiles
        .assign(&with_emb, allowed.as_deref())
        .into_iter()
        .map(|a| {
            let mut evidence = format!("voiceprint {:.2}", a.score);
            if let Some((other, s)) = &a.also {
                evidence.push_str(&format!(" · also matched {other} at {s:.2}"));
            }
            let email = attendees
                .iter()
                .find(|c| same_person(&c.name, &a.profile.name))
                .and_then(|c| c.email.clone());
            Suggestion {
                label: a.label,
                name: a.profile.name,
                email,
                profile: Some(a.profile.id),
                confidence: a.score,
                source: SuggestionSource::Voiceprint,
                evidence,
            }
        })
        .collect();

    // 3a. Self-introductions on the system track in the first ten minutes.
    for g in t
        .segments
        .iter()
        .filter(|g| g.track == Track::System && g.speaker != ME && g.t0_ms < INTRO_WINDOW_MS)
    {
        for cap in INTRO_RE.captures_iter(&g.text) {
            let heard = cap[1].trim_matches(|c: char| !c.is_alphanumeric());
            let heard_lc = heard.to_lowercase();
            let found = candidates.iter().find(|c| c.first() == heard_lc).cloned();
            let candidate = match found {
                Some(c) => c,
                None if with_event => continue,
                None => {
                    // No attendee list: a capitalised, uncommon word after the cue is a name.
                    if !heard.chars().next().is_some_and(char::is_uppercase)
                        || heard.chars().count() < 2
                        || NOT_NAMES.contains(&heard_lc.as_str())
                    {
                        continue;
                    }
                    let c = Candidate {
                        name: heard.to_owned(),
                        email: None,
                    };
                    candidates.push(c.clone());
                    c
                }
            };
            proposals.push(Suggestion {
                label: g.speaker.clone(),
                name: candidate.name.clone(),
                email: candidate.email.clone(),
                profile: None,
                confidence: INTRO,
                source: SuggestionSource::Intro,
                evidence: format!("\"{}\" at {}", cap[0].trim(), mmss(g.t0_ms)),
            });
        }
    }

    // 3b. Vocatives: a name at the edge of one segment names the next label to speak.
    for (i, g) in t.segments.iter().enumerate() {
        let heard = VOCATIVE_START_RE
            .captures(&g.text)
            .or_else(|| VOCATIVE_END_RE.captures(&g.text))
            .map(|c| c[1].to_lowercase());
        let Some(heard) = heard else { continue };
        let Some(c) = candidates.iter().find(|c| c.first() == heard).cloned() else {
            continue;
        };
        let Some(next) = t.segments[i + 1..]
            .iter()
            .take_while(|n| n.t0_ms <= g.t1_ms + VOCATIVE_GAP_MS)
            .find(|n| n.speaker != g.speaker && n.speaker != ME && n.track == Track::System)
        else {
            continue;
        };
        proposals.push(Suggestion {
            label: next.speaker.clone(),
            name: c.name,
            email: c.email,
            profile: None,
            confidence: VOCATIVE,
            source: SuggestionSource::Vocative,
            evidence: format!(
                "\"{}\" at {}, answered by {}",
                g.text.trim(),
                mmss(g.t0_ms),
                next.speaker
            ),
        });
    }

    // 5. 1:1 and elimination (attendee candidates only).
    if with_event && attendees.len() == 1 && labels.len() == 1 {
        let a = &attendees[0];
        proposals.push(Suggestion {
            label: labels[0].to_owned(),
            name: a.name.clone(),
            email: a.email.clone(),
            profile: None,
            confidence: ONE_ON_ONE,
            source: SuggestionSource::OneOnOne,
            evidence: "the only other attendee, the only other voice".to_owned(),
        });
    } else if with_event {
        let open_labels: Vec<&str> = labels
            .iter()
            .copied()
            .filter(|l| !proposals.iter().any(|p| p.label == *l))
            .collect();
        let open_names: Vec<&Candidate> = attendees
            .iter()
            .filter(|a| !proposals.iter().any(|p| same_person(&p.name, &a.name)))
            .collect();
        if let ([l], [a]) = (open_labels.as_slice(), open_names.as_slice()) {
            proposals.push(Suggestion {
                label: (*l).to_owned(),
                name: a.name.clone(),
                email: a.email.clone(),
                profile: None,
                confidence: ELIMINATION,
                source: SuggestionSource::Elimination,
                evidence: "the one attendee left for the one voice left".to_owned(),
            });
        }
    }

    // 6. Conflicts: one per label (highest wins), one label per name (highest wins), ≥ 0.5.
    let mut best: Vec<Suggestion> = Vec::new();
    for l in &labels {
        if let Some(p) = proposals
            .iter()
            .filter(|p| p.label == *l && p.confidence >= MIN_CONFIDENCE)
            .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
        {
            best.push(p.clone());
        }
    }
    let mut kept: Vec<Suggestion> = Vec::new();
    for s in best {
        match kept.iter_mut().find(|k| same_person(&k.name, &s.name)) {
            None => kept.push(s),
            Some(k) if k.confidence >= s.confidence => {
                k.evidence.push_str(&format!(
                    " · also matched {} at {:.2}",
                    s.label, s.confidence
                ));
            }
            Some(k) => {
                let mut s = s;
                s.evidence.push_str(&format!(
                    " · also matched {} at {:.2}",
                    k.label, k.confidence
                ));
                *k = s;
            }
        }
    }
    kept.sort_by_key(|s| labels.iter().position(|l| *l == s.label));
    out.extend(kept);
    out
}

/// `meeting_suggest_speakers(id) -> Vec<Suggestion>` (§15.2, W4): the transcript of record,
/// `embeddings.json`, the enrolled profiles and the manifest's event through [`suggest`].
pub async fn rpc_suggest(_rt: &Runtime, id: MeetingId) -> anyhow::Result<Vec<Suggestion>> {
    let (store, _) = crate::calendar::handles().await?;
    suggest_for(&store, id)
}

/// [`suggest`] over what the store holds for `id` (the tool and the RPC share it).
pub fn suggest_for(store: &crate::store::Store, id: MeetingId) -> anyhow::Result<Vec<Suggestion>> {
    let t = store.transcript(id)?;
    let manifest = store.manifest(id)?;
    let embeddings: HashMap<String, Vec<f32>> =
        std::fs::read(store.meeting_dir(id).join(crate::transcript::EMBEDDINGS))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    let profiles = Profiles::load(&store.speakers_dir())?;
    Ok(suggest(&t, &embeddings, &profiles, manifest.event.as_ref()))
}

/// One suggestion as the tool prints it.
pub fn line(s: &Suggestion) -> String {
    match s.source {
        SuggestionSource::Warning => format!("{} — warning: {}", s.label, s.evidence),
        _ => format!(
            "{} — {}? ({:?} {:.2}) — {}",
            s.label, s.name, s.source, s.confidence, s.evidence
        ),
    }
}

// ---- §10.4 enrollment gates for the automatic path.

/// Least speech the automatic path enrolls from (manual keeps `ENROLL_MIN_MS`).
pub const GATE_MIN_MS: u64 = 5_000;
/// Min pairwise cosine between the span's 3 s windows.
pub const GATE_AGREEMENT: f32 = 0.70;
/// Span RMS over the quietest 10 % of 20 ms frames.
pub const GATE_SNR_DB: f32 = 8.0;
/// Most samples at |x| ≥ 0.99.
pub const GATE_CLIP: f32 = 0.05;
/// Window the agreement gate embeds.
pub const GATE_WINDOW_MS: u64 = 3_000;

/// Duration, clipping and SNR of the 16 kHz speech about to be embedded; `Err` is the tooltip
/// behind "Accept name only".
pub fn gate_level(speech: &[f32], rate: u32) -> Result<(), String> {
    let ms = speech.len() as u64 * 1000 / u64::from(rate.max(1));
    if ms < GATE_MIN_MS {
        return Err(format!(
            "{:.1} s of speech: {} s needed to enroll a voice",
            ms as f32 / 1000.0,
            GATE_MIN_MS / 1000
        ));
    }
    let clipped = speech.iter().filter(|x| x.abs() >= 0.99).count() as f32 / speech.len() as f32;
    if clipped > GATE_CLIP {
        return Err(format!(
            "clipped: {:.0}% of the samples are at full scale",
            clipped * 100.0
        ));
    }
    let frame = (rate as usize / 50).max(1);
    let mut powers: Vec<f32> = speech
        .chunks(frame)
        .map(|c| c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32)
        .collect();
    powers.sort_by(f32::total_cmp);
    let quiet = (powers.len() / 10).max(1);
    let noise = powers[..quiet].iter().sum::<f32>() / quiet as f32;
    let signal = powers.iter().sum::<f32>() / powers.len() as f32;
    if noise > 0.0 {
        let snr = 10.0 * (signal / noise).log10();
        if snr < GATE_SNR_DB {
            return Err(format!(
                "too noisy: {snr:.1} dB above the quietest part ({GATE_SNR_DB:.0} needed)"
            ));
        }
    }
    Ok(())
}

/// Min pairwise cosine between the windows' embeddings ≥ [`GATE_AGREEMENT`]; fewer than two
/// windows pass.
pub fn gate_agreement(windows: &[Vec<f32>]) -> Result<(), String> {
    let mut min = f32::MAX;
    for (i, a) in windows.iter().enumerate() {
        for b in &windows[i + 1..] {
            min = min.min(cosine(a, b));
        }
    }
    if windows.len() >= 2 && min < GATE_AGREEMENT {
        return Err(format!(
            "the voice is not consistent across the span (agreement {min:.2} < {GATE_AGREEMENT:.2})"
        ));
    }
    Ok(())
}

/// [`super::embed_spans`] behind the §10.4 gates: level gates on the decoded speech, then the
/// 3 s windows through the model for agreement, then the whole span. Blocking (seconds).
pub fn embed_spans_gated(
    path: &Path,
    spans: &[(u64, u64)],
    models: &diarize::Models,
) -> anyhow::Result<Vec<f32>> {
    let cut = transcribe::opus_ranges(path, spans)?;
    let speech = voice::resample(cut, transcribe::OPUS_RATE, transcribe::RATE)?;
    gate_level(&speech, transcribe::RATE).map_err(anyhow::Error::msg)?;
    let mut e = diarize::Embedder::open(models)?;
    let window = (GATE_WINDOW_MS * u64::from(transcribe::RATE) / 1000) as usize;
    let windows = speech
        .chunks(window)
        .filter(|c| c.len() >= window / 2)
        .map(|c| e.compute(&diarize::to_i16(c)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    gate_agreement(&windows).map_err(anyhow::Error::msg)?;
    e.compute(&diarize::to_i16(&speech))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::{Attendee, EventId};
    use crate::transcript::{Segment, SpeakerLabel, Word};

    fn attendee(name: &str, is_me: bool) -> Attendee {
        Attendee {
            name: name.into(),
            email: Some(format!(
                "{}@example.com",
                name.split_whitespace().next().unwrap().to_lowercase()
            )),
            status: None,
            is_me,
        }
    }

    fn event(names: &[&str]) -> EventRef {
        EventRef {
            id: EventId("ics:1:".into()),
            uid: "1".into(),
            title: "Sync".into(),
            start: "2026-10-10T10:00:00Z".into(),
            end: "2026-10-10T11:00:00Z".into(),
            join_url: None,
            join_host: None,
            organizer: None,
            attendees: std::iter::once(attendee("Sky Moore", true))
                .chain(names.iter().map(|n| attendee(n, false)))
                .collect(),
            description: None,
        }
    }

    /// `(label, t0_ms, t1_ms, text)` → a transcript; `me` lines are on the mic track.
    fn transcript(lines: &[(&str, u64, u64, &str)]) -> Transcript {
        let mut speakers = vec![SpeakerLabel {
            id: ME.into(),
            name: "Me".into(),
            profile: None,
            proposed: None,
        }];
        let mut segments = Vec::new();
        for (i, (label, t0, t1, text)) in lines.iter().enumerate() {
            if *label != ME && !speakers.iter().any(|s| s.id == *label) {
                speakers.push(SpeakerLabel {
                    id: (*label).into(),
                    name: crate::transcript::default_name(label),
                    profile: None,
                    proposed: None,
                });
            }
            segments.push(Segment {
                id: i as u32,
                track: if *label == ME {
                    Track::Mic
                } else {
                    Track::System
                },
                speaker: (*label).into(),
                t0_ms: *t0,
                t1_ms: *t1,
                text: (*text).into(),
                words: vec![Word {
                    text: (*text).into(),
                    t0_ms: *t0,
                    t1_ms: *t1,
                }],
            });
        }
        segments.sort_by_key(|s| s.t0_ms);
        Transcript {
            meeting: MeetingId::new(),
            speakers,
            segments,
            reviewed_at: None,
            summarized_at: None,
            diarized: true,
            note: None,
        }
    }

    fn names(s: &[Suggestion]) -> Vec<(&str, &str, SuggestionSource)> {
        s.iter()
            .map(|s| (s.label.as_str(), s.name.as_str(), s.source))
            .collect()
    }

    fn empty() -> Profiles {
        Profiles::load(tempfile::tempdir().unwrap().path()).unwrap()
    }

    /// §10.5 #245: `me` saying "I'm Dana" is a quote; `s1` saying it with Dana attending is an intro.
    #[test]
    fn owner_is_never_suggested_and_intros_name_the_system_speaker() {
        let t = transcript(&[
            (ME, 0, 2_000, "Hi everyone, I'm Dana."),
            ("s1", 3_000, 5_000, "Hey, I'm Dana, from the platform team."),
            ("s2", 6_000, 8_000, "Hello."),
        ]);
        let e = event(&["Dana Whitfield", "Bob Lee", "Cy Young"]);
        let got = suggest(&t, &HashMap::new(), &empty(), Some(&e));
        assert_eq!(
            names(&got),
            [("s1", "Dana Whitfield", SuggestionSource::Intro)]
        );
        assert_eq!(got[0].email.as_deref(), Some("dana@example.com"));
        assert!(
            got[0].evidence.starts_with("\"I'm Dana\" at 00:03"),
            "{}",
            got[0].evidence
        );
        assert!((got[0].confidence - 0.8).abs() < 1e-6);
        // Past the ten-minute window the cue is ignored.
        let late = transcript(&[("s1", 11 * 60_000, 11 * 60_000 + 2_000, "I'm Dana.")]);
        assert!(suggest(&late, &HashMap::new(), &empty(), Some(&e)).is_empty());
    }

    /// §10.5: with an event only attendees are candidates; without, an intro may name anyone.
    #[test]
    fn candidates_are_attendees_only_when_an_event_matched() {
        let t = transcript(&[
            ("s1", 1_000, 3_000, "This is Bob speaking."),
            (
                "s2",
                4_000,
                6_000,
                "Right. It's Monday, so I'm going to be quick.",
            ),
            ("s3", 7_000, 9_000, "ok"),
        ]);
        let e = event(&["Dana Whitfield", "Cy Young", "Ann Li"]);
        assert!(
            suggest(&t, &HashMap::new(), &empty(), Some(&e)).is_empty(),
            "Bob is not invited; no elimination with three open attendees"
        );
        let got = suggest(&t, &HashMap::new(), &empty(), None);
        assert_eq!(names(&got), [("s1", "Bob", SuggestionSource::Intro)]);
    }

    /// §10.5: 1:1 → OneOnOne 0.9 for the one system label; two labels → none (the second gets
    /// elimination only when exactly one attendee is left, which a 1:1 never has).
    #[test]
    fn one_on_one_needs_one_system_label() {
        let t = transcript(&[(ME, 0, 1_000, "hi"), ("s1", 2_000, 4_000, "hello there")]);
        let e = event(&["Dana Whitfield"]);
        let got = suggest(&t, &HashMap::new(), &empty(), Some(&e));
        assert_eq!(
            names(&got),
            [("s1", "Dana Whitfield", SuggestionSource::OneOnOne)]
        );
        assert!((got[0].confidence - 0.9).abs() < 1e-6);
        let two = transcript(&[
            (ME, 0, 1_000, "hi"),
            ("s1", 2_000, 4_000, "hello there"),
            ("s2", 5_000, 6_000, "hey"),
        ]);
        assert!(suggest(&two, &HashMap::new(), &empty(), Some(&e)).is_empty());
    }

    /// Elimination: the one unnamed label gets the one unclaimed attendee; a vocative names
    /// the next speaker.
    #[test]
    fn elimination_and_vocatives() {
        let t = transcript(&[
            ("s1", 1_000, 3_000, "I'm Dana, let's start."),
            ("s2", 4_000, 6_000, "Sounds good."),
        ]);
        let e = event(&["Dana Whitfield", "Bob Lee"]);
        let got = suggest(&t, &HashMap::new(), &empty(), Some(&e));
        assert_eq!(
            names(&got),
            [
                ("s1", "Dana Whitfield", SuggestionSource::Intro),
                ("s2", "Bob Lee", SuggestionSource::Elimination)
            ]
        );
        assert!((got[1].confidence - 0.6).abs() < 1e-6);
        // Vocative: "Bob, what do you think?" → the next system voice within 5 s is Bob.
        let t = transcript(&[
            ("s1", 1_000, 3_000, "Bob, what do you think?"),
            ("s2", 4_000, 6_000, "I like it."),
            ("s3", 7_000, 9_000, "Same."),
            ("s3", 20_000, 21_000, "Thanks Dana."),
            ("s2", 30_000, 31_000, "Too late to count."),
        ]);
        let e = event(&["Dana Whitfield", "Bob Lee", "Cy Young"]);
        let got = suggest(&t, &HashMap::new(), &empty(), Some(&e));
        assert_eq!(names(&got), [("s2", "Bob Lee", SuggestionSource::Vocative)]);
        assert!((got[0].confidence - 0.5).abs() < 1e-6);
        assert!(got[0].evidence.contains("answered by s2"));
    }

    fn at(c: f32) -> Vec<f32> {
        vec![c, (1.0 - c * c).sqrt()]
    }

    /// §10.5: voiceprints need margin (0.71/0.69 → neither, 0.71/0.55 → one) and mutuality;
    /// the chip carries the profile id and the attendee's mail.
    #[test]
    fn voiceprints_follow_margin_and_mutuality() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Profiles::load(tmp.path()).unwrap();
        let dana = p.create("Dana", None, &[1.0, 0.0]).unwrap();
        let t = transcript(&[("s1", 0, 2_000, "a"), ("s2", 3_000, 5_000, "b")]);
        let e = event(&["Dana Whitfield", "Bob Lee", "Cy Young"]);
        let emb =
            |a: f32, b: f32| HashMap::from([("s1".to_owned(), at(a)), ("s2".to_owned(), at(b))]);
        assert!(suggest(&t, &emb(0.71, 0.69), &p, Some(&e)).is_empty());
        let got = suggest(&t, &emb(0.71, 0.55), &p, Some(&e));
        assert_eq!(names(&got), [("s1", "Dana", SuggestionSource::Voiceprint)]);
        assert_eq!(got[0].profile, Some(dana.id));
        assert_eq!(got[0].email.as_deref(), Some("dana@example.com"));
        assert!(got[0].evidence.starts_with("voiceprint 0.71"));
        // Mutuality: Dana best for s2, but s2's best is John → neither.
        p.create("John", None, &[0.0, 1.0]).unwrap();
        let emb = HashMap::from([
            ("s1".to_owned(), vec![0.5, 0.866]),
            ("s2".to_owned(), vec![0.6, 0.8]),
        ]);
        assert!(suggest(&t, &emb, &p, None).is_empty());
        // With an event, a profile that is not an attendee is not a candidate.
        let emb = HashMap::from([("s1".to_owned(), vec![0.0, 1.0])]);
        assert!(
            suggest(&t, &emb, &p, Some(&e)).is_empty(),
            "John is enrolled but not invited"
        );
        assert_eq!(
            names(&suggest(&t, &emb, &p, None)),
            [("s1", "John", SuggestionSource::Voiceprint)]
        );
    }

    /// §10.5 (Hyprnote): one label with 85 % of the speech and five attendees → a warning and no
    /// voiceprint for it; the others still get theirs.
    #[test]
    fn a_dominant_label_in_a_crowd_is_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Profiles::load(tmp.path()).unwrap();
        p.create("Dana", None, &[1.0, 0.0]).unwrap();
        p.create("Bob", None, &[0.0, 1.0]).unwrap();
        let t = transcript(&[("s1", 0, 85_000, "lots"), ("s2", 85_000, 100_000, "a bit")]);
        let e = event(&["Dana Whitfield", "Bob Lee", "Cy Young", "Ann Li", "Eve Fox"]);
        let emb = HashMap::from([
            ("s1".to_owned(), vec![1.0, 0.0]),
            ("s2".to_owned(), vec![0.0, 1.0]),
        ]);
        let got = suggest(&t, &emb, &p, Some(&e));
        assert_eq!(
            names(&got),
            [
                ("s1", "", SuggestionSource::Warning),
                ("s2", "Bob", SuggestionSource::Voiceprint)
            ]
        );
        assert_eq!(got[0].evidence, WARNING);
        // Four attendees but a 60/40 split → no warning, both voiceprints.
        let t = transcript(&[("s1", 0, 60_000, "x"), ("s2", 60_000, 100_000, "y")]);
        assert_eq!(suggest(&t, &emb, &p, Some(&e)).len(), 2);
    }

    /// §10.2 step 6: one name claimed by two labels keeps the higher with the other noted;
    /// live labels never enter — only the post transcript's ids come out.
    #[test]
    fn conflicts_keep_the_higher_and_labels_are_the_post_transcripts() {
        let t = transcript(&[
            ("s1", 1_000, 3_000, "I'm Dana."),
            ("s2", 4_000, 6_000, "Dana, go ahead."),
            ("s3", 7_000, 8_000, "ok"),
        ]);
        let e = event(&["Dana Whitfield", "Bob Lee", "Cy Young"]);
        let got = suggest(&t, &HashMap::new(), &empty(), Some(&e));
        // s1 intro 0.8 beats s3's vocative 0.5 for Dana; s3 is dropped with a note on s1.
        assert_eq!(
            names(&got),
            [("s1", "Dana Whitfield", SuggestionSource::Intro)]
        );
        assert!(
            got[0].evidence.ends_with("also matched s3 at 0.50"),
            "{}",
            got[0].evidence
        );
        let live_labels = ["Speaker 1", "Speaker 2", "them"];
        assert!(
            got.iter()
                .all(|s| t.speakers.iter().any(|l| l.id == s.label))
        );
        assert!(got.iter().all(|s| !live_labels.contains(&s.label.as_str())));
    }

    /// The RPC/tool path: a meeting on disk (manifest with event, `transcript.json`,
    /// `embeddings.json`, an enrolled profile) → suggestions through [`suggest_for`].
    #[tokio::test]
    async fn fixture_on_disk_reaches_suggestions() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let mut t = transcript(&[
            (ME, 0, 2_000, "hi all"),
            ("s1", 3_000, 6_000, "hello, this is Cy."),
            ("s2", 7_000, 9_000, "hey"),
        ]);
        let mut m = crate::Manifest::new(
            t.meeting,
            crate::ProcessInfo {
                pid: 1,
                name: "zoom".into(),
                bundle_id: None,
                active: false,
            },
            "2026-10-10T10:00:00Z".into(),
        );
        m.state = crate::MeetingState::Review;
        m.event = Some(event(&["Dana Whitfield", "Cy Young"]));
        store.create(&m).await.unwrap();
        t.meeting = m.id;
        store.save_transcript(&t).await.unwrap();
        std::fs::write(
            store.meeting_dir(m.id).join(crate::transcript::EMBEDDINGS),
            serde_json::to_vec(&HashMap::from([("s2".to_owned(), vec![1.0f32, 0.0])])).unwrap(),
        )
        .unwrap();
        let mut p = Profiles::load(&store.speakers_dir()).unwrap();
        let dana = p.create("Dana", None, &[1.0, 0.0]).unwrap();
        let got = suggest_for(&store, m.id).unwrap();
        assert_eq!(
            names(&got),
            [
                ("s1", "Cy Young", SuggestionSource::Intro),
                ("s2", "Dana", SuggestionSource::Voiceprint)
            ]
        );
        assert_eq!(got[1].profile, Some(dana.id));
        assert!(line(&got[1]).starts_with("s2 — Dana? (Voiceprint 1.00)"));
        assert!(
            suggest_for(&store, MeetingId::new()).is_err(),
            "no such meeting"
        );
    }

    /// §10.5 gates: a 4 s span and a clipped span fail with the tooltip text; a clean 6 s span
    /// passes; windows that disagree fail.
    #[test]
    fn enrollment_gates() {
        let rate = transcribe::RATE;
        let tone = |secs: f32, amp: f32| -> Vec<f32> {
            (0..(secs * rate as f32) as usize)
                .map(|i| (i as f32 / rate as f32 * 220.0 * std::f32::consts::TAU).sin() * amp)
                .collect()
        };
        assert_eq!(
            gate_level(&tone(4.0, 0.5), rate).unwrap_err(),
            "4.0 s of speech: 5 s needed to enroll a voice"
        );
        let mut clipped = tone(6.0, 0.5);
        let tenth = clipped.len() / 10;
        clipped[..tenth].fill(1.0);
        assert!(
            gate_level(&clipped, rate)
                .unwrap_err()
                .starts_with("clipped: 10%")
        );
        // A pure tone has no quiet frames (SNR ≈ 0 dB): too noisy. Speech-like: tone + silence.
        assert!(
            gate_level(&tone(6.0, 0.5), rate)
                .unwrap_err()
                .starts_with("too noisy")
        );
        let mut speech = tone(6.0, 0.5);
        for x in speech.iter_mut().step_by(7) {
            *x = 0.0;
        }
        let quiet = speech.len() / 5;
        for x in &mut speech[..quiet] {
            *x *= 0.01;
        }
        assert_eq!(gate_level(&speech, rate), Ok(()));
        assert_eq!(gate_agreement(&[at(0.9), at(0.8), at(0.85)]), Ok(()));
        assert!(gate_agreement(&[vec![1.0, 0.0], vec![0.0, 1.0]]).is_err());
        assert_eq!(
            gate_agreement(&[vec![1.0, 0.0]]),
            Ok(()),
            "one window passes"
        );
    }
}
