// 144-W1 owns this file.
//! One ingest job (§4.2): `running` → extract → cap → `ImportMarkdown` as `Principal::Helper`
//! (`Replace` first, `Diff` on refresh so unchanged blocks keep their ids) → rename when the origin
//! knows its title → snapshot → row (`fetched_ms`, `content_sha`, sizes, `changed_since`) →
//! `completed` → index hook → insight hook. A refresh whose markdown hashes the same touches the
//! row only. Any error is `failed` with the reason; transient ones say so.

use std::sync::Arc;
use std::time::Instant;

use crate::actor::{ImportMarkdown, Rename, Snapshot, with_heading};
use crate::helper::ImportMode;
use crate::ingest::{
    Ctx, cap_chars, insights, permanent, project, provisional_title, routes, summaries, word_count,
};
use crate::meta::{DocId, DocKind, DocumentChanged, Principal};
use crate::notebook::{SourceEvent, SourceStatus};
use crate::store::{SourceRow, now_ms, sha256};

/// Run the row's route end to end; never panics out, never returns before the row says what happened.
pub async fn run(ctx: &Ctx, id: &DocId, refresh: bool) {
    let row = match ctx.store.source_row(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(source = %id, "ingest: row unreadable: {e:#}");
            return;
        }
    };
    let attempts = row.attempts + 1;
    set(ctx, id, SourceStatus::Running, None, attempts).await;
    let t = Instant::now();
    let progress: routes::Progress = {
        let (rt, id) = (ctx.rt.clone(), id.clone());
        Arc::new(move |percent: u8| {
            let (rt, id) = (rt.clone(), id.clone());
            tokio::spawn(async move {
                rt.publish(SourceEvent::Progress {
                    source: id,
                    percent: percent.min(100),
                })
                .await;
            });
        })
    };
    match step(ctx, &row, attempts, refresh, progress).await {
        Ok(out) => {
            let title = out.title.clone();
            if let Err(e) = ctx.store.source_upsert(&out.row).await {
                tracing::warn!(source = %id, "ingest: row not written: {e:#}");
                set(
                    ctx,
                    id,
                    SourceStatus::Failed,
                    Some(&format!("row not written: {e:#}")),
                    attempts,
                )
                .await;
                return;
            }
            tracing::info!(source = %id, origin = %out.row.origin.key(), words = ?out.row.words, "ingest: completed in {:?}", t.elapsed());
            ctx.rt
                .publish(SourceEvent::Changed {
                    source: id.clone(),
                    status: SourceStatus::Completed,
                })
                .await;
            let ev = DocumentChanged {
                doc: id.clone(),
                kind: DocKind::Source,
                title: title.clone(),
                updated_ms: out.row.fetched_ms.unwrap_or_else(now_ms),
                deleted: false,
            };
            crate::index::on_document_changed(&ev);
            if let Ok(mut docs) = summaries(&ctx.store).await
                && let Some(doc) = docs.remove(id)
            {
                insights::after_ingest(&project(out.row, doc, None));
            }
        }
        Err(e) => {
            let mut msg = format!("{e:#}");
            if !permanent(&msg) {
                msg.push_str(" (transient; refresh to retry)");
            }
            tracing::warn!(source = %id, "ingest: failed after {:?}: {msg}", t.elapsed());
            set(ctx, id, SourceStatus::Failed, Some(&msg), attempts).await;
        }
    }
}

struct Outcome {
    row: SourceRow,
    title: String,
}

async fn step(
    ctx: &Ctx,
    row: &SourceRow,
    attempts: u32,
    refresh: bool,
    progress: routes::Progress,
) -> anyhow::Result<Outcome> {
    let mut ex = routes::extract(ctx, row, progress).await?;
    ex.markdown = cap_chars(ex.markdown, ctx.cfg.max_source_chars);
    let sha = sha256(ex.markdown.as_bytes());
    let now = now_ms();
    let current = summaries(&ctx.store)
        .await?
        .remove(&row.id)
        .ok_or_else(|| anyhow::anyhow!("source document {} is gone", row.id))?
        .title;
    let title = match &ex.title {
        Some(t) if !t.trim().is_empty() && current == provisional_title(&row.origin) => {
            t.trim().to_owned()
        }
        _ => current.clone(),
    };
    let mut out = row.clone();
    out.attempts = attempts;
    if !(refresh && row.content_sha.as_deref() == Some(sha.as_str())) {
        let mode = if refresh && row.content_sha.is_some() {
            ImportMode::Diff
        } else {
            ImportMode::Replace
        };
        ctx.docs
            .ask(ImportMarkdown {
                doc: row.id.clone(),
                markdown: with_heading(&title, &ex.markdown),
                by: Principal::Helper,
                mode,
            })
            .await
            .map_err(|e| anyhow::anyhow!("import: {e}"))?
            .map_err(anyhow::Error::msg)?;
        if title != current {
            ctx.docs
                .ask(Rename {
                    doc: row.id.clone(),
                    title: title.clone(),
                })
                .await
                .map_err(|e| anyhow::anyhow!("rename: {e}"))?
                .map_err(anyhow::Error::msg)?;
        }
        ctx.docs
            .ask(Snapshot {
                doc: row.id.clone(),
                label: format!("ingest {attempts}"),
            })
            .await
            .map_err(|e| anyhow::anyhow!("snapshot: {e}"))?
            .map_err(anyhow::Error::msg)?;
        if refresh && row.content_sha.is_some() {
            out.changed_since = row.fetched_ms;
        }
    }
    out.status = SourceStatus::Completed;
    out.error = None;
    out.fetched_ms = Some(now);
    out.content_sha = Some(sha);
    out.bytes = ex.bytes.or(row.bytes);
    out.words = Some(word_count(&ex.markdown));
    out.pages = ex.pages.or(row.pages);
    out.duration_ms = ex.duration_ms.or(row.duration_ms);
    out.blob = ex.blob.or_else(|| row.blob.clone());
    if !ex.meta.is_null() {
        out.meta = ex.meta;
    }
    if let Some(o) = ex.origin {
        out.origin = o;
    }
    Ok(Outcome { row: out, title })
}

async fn set(ctx: &Ctx, id: &DocId, status: SourceStatus, error: Option<&str>, attempts: u32) {
    if let Err(e) = ctx
        .store
        .source_set_status(id, status, error, attempts)
        .await
    {
        tracing::warn!(source = %id, "ingest: status not written: {e:#}");
    }
    ctx.rt
        .publish(SourceEvent::Changed {
            source: id.clone(),
            status,
        })
        .await;
}
