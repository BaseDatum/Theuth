// 144-W1 owns this file.
//! The file route (§4.1 row 1): `tools::doc::convert` (anydoc → GFM) for documents, verbatim for
//! txt/md; the original lands in the attachment store as `blob`. Audio/video files are classified
//! here too and run their own routes.

use std::path::Path;

use anyhow::Context as _;
use sha2::Digest as _;
use tools::doc::Format;

use super::{AUDIO_EXTS, TEXT_EXTS, VIDEO_EXTS, too_large, unsupported};
use crate::ingest::{Ctx, Extracted, save_blob};
use crate::notebook::SourceOrigin;
use crate::store::SourceRow;

/// Name, lowercase extension and sha of a file on disk; `Audio | Video | File` by extension.
pub async fn preflight(ctx: &Ctx, path: &Path) -> anyhow::Result<(SourceOrigin, Option<String>)> {
    let md = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    anyhow::ensure!(md.is_file(), "{} is not a file", path.display());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_owned();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let p = path.to_path_buf();
    let sha256 = tokio::task::spawn_blocking(move || sha256_file(&p)).await??;
    let origin = if AUDIO_EXTS.contains(&ext.as_str()) {
        SourceOrigin::Audio {
            name,
            sha256,
            ext: ext.clone(),
        }
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        SourceOrigin::Video {
            name,
            sha256,
            ext: ext.clone(),
        }
    } else {
        SourceOrigin::File {
            name,
            sha256,
            ext: ext.clone(),
        }
    };
    let refusal = if md.len() > ctx.cfg.max_source_bytes {
        Some(too_large(md.len(), ctx.cfg.max_source_bytes))
    } else if matches!(origin, SourceOrigin::File { .. })
        && !TEXT_EXTS.contains(&ext.as_str())
        && Format::from_extension(&ext).is_none()
    {
        Some(unsupported(&ext))
    } else {
        None
    };
    Ok((origin, refusal))
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

/// Read the original (`blob`, else the path it was added from), copy it into the attachment store
/// when it is not there yet, convert.
pub async fn extract(ctx: &Ctx, row: &SourceRow) -> anyhow::Result<Extracted> {
    let SourceOrigin::File { name, ext, .. } = &row.origin else {
        anyhow::bail!("file route on a {} origin", row.origin.key());
    };
    let (bytes, blob) = load(ctx, row).await?;
    let fmt = Format::from_bytes(&bytes).or_else(|| Format::from_extension(ext));
    let markdown = match fmt {
        Some(f) => tools::doc::to_markdown(bytes.clone(), f).await?,
        None if TEXT_EXTS.contains(&ext.as_str()) => String::from_utf8_lossy(&bytes).into_owned(),
        None => anyhow::bail!("{}", unsupported(ext)),
    };
    let pages = matches!(fmt, Some(Format::Pdf))
        .then(|| pdf_pages(&bytes))
        .flatten();
    Ok(Extracted {
        markdown,
        title: Some(name.clone()),
        meta: serde_json::json!({ "name": name, "ext": ext }),
        bytes: Some(bytes.len() as u64),
        pages,
        duration_ms: None,
        blob: Some(blob),
        origin: None,
    })
}

/// The bytes of a file source and the attachment path they live at (copied in on first use).
pub async fn load(ctx: &Ctx, row: &SourceRow) -> anyhow::Result<(Vec<u8>, String)> {
    let (name, ext) = match &row.origin {
        SourceOrigin::File { name, ext, .. }
        | SourceOrigin::Audio { name, ext, .. }
        | SourceOrigin::Video { name, ext, .. } => (name, ext),
        o => anyhow::bail!("not a file origin: {}", o.key()),
    };
    let from = row
        .blob
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{name}: the original is not on this engine (no blob)"))?;
    let bytes = tokio::fs::read(from)
        .await
        .with_context(|| format!("read {from}"))?;
    anyhow::ensure!(
        bytes.len() as u64 <= ctx.cfg.max_source_bytes,
        "{}",
        too_large(bytes.len() as u64, ctx.cfg.max_source_bytes)
    );
    let attachments = ctx.data_dir.join("attachments");
    let blob = if Path::new(from).starts_with(&attachments) {
        from.to_owned()
    } else {
        let (b, e, d) = (bytes.clone(), ext.clone(), ctx.data_dir.clone());
        tokio::task::spawn_blocking(move || save_blob(&b, &e, &d))
            .await??
            .to_string_lossy()
            .into_owned()
    };
    Ok((bytes, blob))
}

/// `ponytail:` page count by scanning for page objects — the converter reports none on success.
/// Good for every PDF a writer produces; a stream-compressed object tree hides them (→ `None`).
pub fn pdf_pages(bytes: &[u8]) -> Option<u32> {
    let hay = String::from_utf8_lossy(bytes);
    let n = hay
        .match_indices("/Type")
        .filter(|(i, _)| {
            let rest = hay[*i + 5..].trim_start();
            rest.starts_with("/Page") && !rest.starts_with("/Pages")
        })
        .count() as u32;
    (n > 0).then_some(n)
}
