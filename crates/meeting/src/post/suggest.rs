//! 158 W4 owns this file (stub from C158).
//!
//! Speaker-name suggestions (158 §10): `meeting_suggest_speakers` as a read-only tool and RPC.
//! Candidates = the event's attendees minus `is_me` (else enrolled profiles + intro cues);
//! sources = voiceprint (≥ `speakers::MATCH`, margin 0.08, mutual), self-introductions,
//! vocatives, elimination, 1:1, plus a merged-speaker warning. Never applies anything; the
//! Review page's Accept does. The stub suggests nothing.

use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::{MeetingId, ProfileId};

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

/// `meeting_suggest_speakers(id) -> Vec<Suggestion>` (§15.2, W4). Stub: none.
pub async fn rpc_suggest(_rt: &Runtime, _id: MeetingId) -> anyhow::Result<Vec<Suggestion>> {
    Ok(Vec::new())
}
