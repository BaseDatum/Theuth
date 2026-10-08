// 144-W1 owns this file.
//! The meeting route (§4.1 row 3): `<data_dir>/meetings/<id>/transcript.json` segments → one
//! `## [mm:ss] <speaker>` block each. The `meeting` crate depends on `document`, so the two files
//! are read through a serde mirror of the fields used (01 §5 is the on-disk contract). The
//! meeting's `MeetingNote` is a note item the caller adds — never copied here.

use std::path::PathBuf;

use anyhow::Context as _;

use crate::ingest::{Ctx, Extracted, mmss};
use crate::notebook::SourceOrigin;

#[derive(serde::Deserialize)]
struct Manifest {
    title: Option<String>,
    #[serde(default)]
    started_at: String,
    duration_ms: Option<u64>,
}

#[derive(serde::Deserialize)]
struct Speaker {
    id: String,
    name: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct Segment {
    pub speaker: String,
    pub t0_ms: u64,
    pub t1_ms: u64,
    pub text: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct Transcript {
    #[serde(default)]
    speakers: Vec<Speaker>,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

fn dir(ctx: &Ctx, meeting: &str) -> PathBuf {
    ctx.data_dir.join("meetings").join(meeting)
}

/// The meeting must exist on this engine.
pub fn preflight(ctx: &Ctx, meeting: &str) -> anyhow::Result<SourceOrigin> {
    let meeting = meeting.trim();
    anyhow::ensure!(
        !meeting.is_empty() && !meeting.contains(['/', '\\', '.']),
        "bad meeting id: {meeting:?}"
    );
    anyhow::ensure!(
        dir(ctx, meeting).join("manifest.json").is_file(),
        "no such meeting: {meeting}"
    );
    Ok(SourceOrigin::Meeting {
        meeting: meeting.to_owned(),
    })
}

pub fn extract(ctx: &Ctx, meeting: &str) -> anyhow::Result<Extracted> {
    let d = dir(ctx, meeting);
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(d.join("manifest.json")).with_context(|| format!("read {}", d.display()))?,
    )?;
    let path = d.join("transcript.json");
    let transcript: Transcript = serde_json::from_slice(
        &std::fs::read(&path)
            .with_context(|| "the meeting has no transcript yet (post-processing pending)")?,
    )?;
    let (markdown, segments) = render(&transcript);
    anyhow::ensure!(!segments.is_empty(), "the transcript has no segments");
    let title = manifest
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            (!manifest.started_at.is_empty()).then(|| {
                format!(
                    "Meeting {}",
                    &manifest.started_at[..manifest.started_at.len().min(16)]
                )
            })
        });
    let duration_ms = manifest
        .duration_ms
        .or_else(|| transcript.segments.iter().map(|s| s.t1_ms).max());
    Ok(Extracted {
        bytes: Some(markdown.len() as u64),
        markdown,
        title,
        meta: serde_json::json!({ "meeting": meeting, "started_at": manifest.started_at, "segments": segments }),
        pages: None,
        duration_ms,
        blob: None,
        origin: None,
    })
}

/// Blocks plus `meta.segments = [{t0_ms, t1_ms, speaker}]`, index-aligned with the headings (the
/// block ids are the helper's; `@mm:ss` resolves by heading text).
pub(crate) fn render(t: &Transcript) -> (String, Vec<serde_json::Value>) {
    let name = |id: &str| {
        t.speakers
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.as_str())
            .unwrap_or(id)
            .to_owned()
    };
    let mut md = String::new();
    let mut segs = Vec::with_capacity(t.segments.len());
    for s in &t.segments {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        let who = name(&s.speaker);
        md.push_str(&format!("## [{}] {who}\n\n{text}\n\n", mmss(s.t0_ms)));
        segs.push(serde_json::json!({ "t0_ms": s.t0_ms, "t1_ms": s.t1_ms, "speaker": who }));
    }
    (md, segs)
}
