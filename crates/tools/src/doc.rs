//! Documents (PDF, Word, PowerPoint, Excel, ODF, RTF, EPUB) as GitHub-Flavored
//! Markdown via `anydoc` (design doc 20 §2.1). The conversion is synchronous
//! and occasionally slow, so it runs on the blocking pool under a deadline.
//! Used by `read_file` and by `web`'s `fetch`.

use std::time::Duration;

pub use anydoc::Format;

/// Largest input handed to the converter; above it the caller says so instead.
pub const MAX_BYTES: usize = 25 * 1024 * 1024;

/// Wall-clock cap on one conversion.
const DEADLINE: Duration = Duration::from_secs(30);

/// The document format `bytes` carry, if `anydoc` recognizes one from the
/// content (PDF header, RTF group, OLE stream names, ZIP package mimetype).
/// `None` for text, CSV and unknown binaries.
#[must_use]
pub fn format(bytes: &[u8]) -> Option<Format> {
    Format::from_bytes(bytes)
}

/// A conversion that outlived [`to_markdown`]'s deadline. The blocking task
/// cannot be interrupted, so it keeps burning a thread until anydoc returns;
/// a caller that serialises conversions (design doc 49 §4.3) checks
/// [`Pending::is_finished`] before starting the next one.
pub struct Pending(tokio::task::JoinHandle<Result<String, anydoc::ConvertError>>);

impl Pending {
    /// Whether the abandoned conversion has actually ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

/// `bytes` as Markdown. The error names the reason in words the model can
/// act on: `pages 3, 4 of 12 are scanned (need OCR) …`, `encrypted`, …
pub async fn to_markdown(bytes: Vec<u8>, format: Format) -> anyhow::Result<String> {
    convert(bytes, format).await.0
}

/// [`to_markdown`], plus the still-running task when it timed out (`None`
/// otherwise) for callers that must not overlap conversions.
pub async fn convert(bytes: Vec<u8>, format: Format) -> (anyhow::Result<String>, Option<Pending>) {
    if bytes.len() > MAX_BYTES {
        let err = anyhow::anyhow!(
            "{:.1} MB exceeds the {} MB conversion cap",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_BYTES >> 20
        );
        return (Err(err), None);
    }
    let mut work =
        tokio::task::spawn_blocking(move || anydoc::to_markdown_bytes(&bytes, Some(format)));
    match tokio::time::timeout(DEADLINE, &mut work).await {
        Ok(Ok(Ok(md))) => (Ok(md), None),
        Ok(Ok(Err(e))) => (Err(describe(e)), None),
        Ok(Err(join)) => (Err(anyhow::anyhow!("converter panicked: {join}")), None),
        Err(_) => (
            Err(anyhow::anyhow!(
                "conversion timed out after {}s",
                DEADLINE.as_secs()
            )),
            Some(Pending(work)),
        ),
    }
}

/// Scanned pages are the one failure with a next step (design doc 20 §2.5);
/// everything else is anydoc's own message.
fn describe(e: anydoc::ConvertError) -> anyhow::Error {
    match e {
        anydoc::ConvertError::NeedsOcr { pages, page_count } => anyhow::anyhow!(
            "pages {} of {page_count} are scanned (need OCR); ask for the file as an attachment — a vision model reads the pages itself",
            pages
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => anyhow::anyhow!("{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn text_is_not_a_document_and_a_bad_pdf_names_why() {
        assert_eq!(format(b"plain text\n"), None);
        assert_eq!(format(b"a,b\n1,2\n"), None);
        assert_eq!(format(b"%PDF-1.7\n%garbage"), Some(Format::Pdf));
        let err = to_markdown(b"%PDF-1.7\n%garbage".to_vec(), Format::Pdf)
            .await
            .unwrap_err()
            .to_string();
        assert!(!err.is_empty(), "{err}");
    }
}
