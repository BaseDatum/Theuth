// 144-W1 owns this file.
//! Video files (§4.1 row 10) = their audio track: symphonia demuxes the container (`isomp4`, `mkv`,
//! `ogg`), the audio route runs, no frame is read.

use crate::ingest::{Ctx, Extracted};
use crate::store::SourceRow;

pub async fn extract(
    ctx: &Ctx,
    row: &SourceRow,
    progress: super::Progress,
) -> anyhow::Result<Extracted> {
    super::audio::extract(ctx, row, progress).await
}
