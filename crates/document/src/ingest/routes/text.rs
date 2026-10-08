// 144-W1 owns this file.
//! Pasted text (§4.1 row 4): no extraction. A paste has no origin to go back to, so `source_add`
//! saves it in the attachment store (`blob`, `<sha16>.md`) and the job reads it from there, as a
//! file; a refresh re-reads the same bytes.

use crate::ingest::{Ctx, Extracted};
use crate::notebook::SourceOrigin;
use crate::store::SourceRow;

/// `text:<sha256>`; the title is the caller's or the first line (80 chars, `#` stripped).
pub fn origin(text: &str, title: Option<&str>) -> SourceOrigin {
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| first_line(text));
    SourceOrigin::Text {
        title,
        sha256: crate::store::sha256(text.as_bytes()),
    }
}

/// The first non-empty line without heading marks, cut at 80 chars.
pub fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("Pasted text");
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

pub async fn extract(_ctx: &Ctx, row: &SourceRow) -> anyhow::Result<Extracted> {
    let blob = row
        .blob
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("the pasted text is not on this engine (no blob)"))?;
    let text = tokio::fs::read_to_string(blob).await?;
    Ok(Extracted {
        bytes: Some(text.len() as u64),
        markdown: text,
        title: None,
        meta: serde_json::Value::Null,
        pages: None,
        duration_ms: None,
        blob: Some(blob.to_owned()),
        origin: None,
    })
}
