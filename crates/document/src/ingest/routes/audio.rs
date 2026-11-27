// 144-W1 owns this file.
//! Audio files (§4.1 row 9): the voice plugin's `Transcriber` with word times
//! (`voice::TranscribeWords`: symphonia → 16 kHz → Parakeet TDT in 180 s pieces) → `## [mm:ss]`
//! blocks per ~60 s. Progress is the pieces done.

use std::path::Path;
#[cfg(not(target_os = "ios"))]
use std::time::Duration;

#[cfg(not(target_os = "ios"))]
use kameo::actor::ActorRef;

use crate::ingest::{Ctx, Extracted, timed_blocks};
#[cfg(not(target_os = "ios"))]
use crate::notebook::SourceOrigin;
use crate::store::SourceRow;

/// Blocks span about a minute.
pub const BLOCK_MS: u64 = 60_000;
/// A 41-minute talk is minutes of CPU; a 3-hour recording is the ceiling this allows.
#[cfg(not(target_os = "ios"))]
const DEADLINE: Duration = Duration::from_secs(60 * 60);

pub async fn extract(
    ctx: &Ctx,
    row: &SourceRow,
    progress: super::Progress,
) -> anyhow::Result<Extracted> {
    #[cfg(target_os = "ios")]
    {
        let _ = (ctx, row, progress);
        anyhow::bail!("audio/video transcription is not supported on iOS");
    }
    #[cfg(not(target_os = "ios"))]
    {
        let (SourceOrigin::Audio { name, ext, .. } | SourceOrigin::Video { name, ext, .. }) =
            &row.origin
        else {
            anyhow::bail!("audio route on a {} origin", row.origin.key());
        };
        let (bytes, blob) = super::file::load(ctx, row).await?;
        let mut ex = transcribe(Path::new(&blob), progress).await?;
        ex.title = Some(name.clone());
        ex.meta = serde_json::json!({ "name": name, "ext": ext });
        ex.bytes = Some(bytes.len() as u64);
        ex.blob = Some(blob);
        Ok(ex)
    }
}

/// Word-timed transcript of a media file on this machine, as blocks.
pub async fn transcribe(path: &Path, progress: super::Progress) -> anyhow::Result<Extracted> {
    #[cfg(target_os = "ios")]
    {
        let _ = (path, progress);
        anyhow::bail!("audio/video transcription is not supported on iOS");
    }
    #[cfg(not(target_os = "ios"))]
    {
        let stt = ActorRef::<voice::Transcriber>::lookup(voice::TRANSCRIBER_ACTOR)
            .ok()
            .flatten()
            .ok_or_else(|| {
                anyhow::anyhow!("speech-to-text is not running (voice plugin disabled)")
            })?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let relay = tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                progress(p);
            }
        });
        let r = tokio::time::timeout(
            DEADLINE,
            stt.ask(voice::TranscribeWords {
                path: path.to_path_buf(),
                progress: Some(tx),
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("transcription timed out after {} s", DEADLINE.as_secs()))?
        .map_err(|e| anyhow::anyhow!("transcriber: {}", crate::ingest::flat_err(e)))?;
        relay.abort();
        Ok(from_words(
            r.words.into_iter().map(|w| (w.t0_ms, w.text)),
            r.duration_ms,
        ))
    }
}

/// `## [mm:ss]` blocks from `(start ms, word)`; empty speech is one notice block.
pub fn from_words(words: impl IntoIterator<Item = (u64, String)>, duration_ms: u64) -> Extracted {
    let mut markdown = timed_blocks(words, BLOCK_MS);
    if markdown.trim().is_empty() {
        markdown = "## [00:00]\n\n*(no speech recognized)*\n".into();
    }
    Extracted {
        markdown,
        duration_ms: Some(duration_ms),
        ..Default::default()
    }
}
