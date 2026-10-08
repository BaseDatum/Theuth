// 144-W1 owns this file.
//! The nine routes of §4.1: [`preflight`] names the origin (and a permanent refusal when the origin
//! cannot work), [`extract`] runs the route for a job.

use std::path::Path;
use std::time::Duration;

use kameo::actor::ActorRef;

use crate::ingest::{Ctx, Extracted, SourceAddArgs, TOO_LARGE, UNSUPPORTED};
use crate::notebook::{McpLabel, SourceOrigin};
use crate::store::SourceRow;

pub mod audio;
pub mod file;
pub mod mcp;
pub mod meeting;
pub mod session;
pub mod text;
pub mod url;
pub mod video;
pub mod youtube;

/// Extensions the audio route takes (§4.1; `webm` is audio first, video is its audio track anyway).
pub const AUDIO_EXTS: &[&str] = &["m4a", "mp3", "wav", "ogg", "opus", "webm", "flac", "aac"];
/// Extensions the video route takes.
pub const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "mov", "m4v"];
/// Taken verbatim, no converter.
pub const TEXT_EXTS: &[&str] = &["txt", "md", "markdown", "text", "csv"];

/// Progress sink a route reports 0–100 to.
pub type Progress = std::sync::Arc<dyn Fn(u8) + Send + Sync>;

/// Resolve the arguments to an origin; `Some(reason)` is a permanent refusal written on the card at
/// once (`415 unsupported`, `too large`, `refused: private address`, `no transcript route`).
pub async fn preflight(
    ctx: &Ctx,
    a: &SourceAddArgs,
) -> anyhow::Result<(SourceOrigin, Option<String>)> {
    let given = [
        a.path.is_some(),
        a.url.is_some(),
        a.text.is_some(),
        a.meeting.is_some(),
        a.session.is_some(),
        a.mcp.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    anyhow::ensure!(
        given == 1,
        "source_add takes exactly one of path | url | text | meeting | session | mcp"
    );
    if let Some(p) = &a.path {
        return file::preflight(ctx, Path::new(p)).await;
    }
    if let Some(u) = &a.url {
        return match youtube::video_id(u) {
            Some(video) => youtube::preflight(ctx, video, u).await,
            None => url::preflight(u).await,
        };
    }
    if let Some(t) = &a.text {
        return Ok((text::origin(t, a.title.as_deref()), None));
    }
    if let Some(m) = &a.meeting {
        return meeting::preflight(ctx, m).map(|o| (o, None));
    }
    if let Some(s) = &a.session {
        anyhow::ensure!(!s.trim().is_empty(), "session id is empty");
        return Ok((
            SourceOrigin::Session {
                session: s.trim().to_owned(),
            },
            None,
        ));
    }
    let m = a.mcp.as_ref().expect("one of six");
    let label = match &ctx.cfg.mcp {
        t if t.confluence_tool.as_deref() == Some(m.tool.as_str()) => McpLabel::Confluence,
        t if t.jira_tool.as_deref() == Some(m.tool.as_str()) => McpLabel::Jira,
        _ => McpLabel::Other,
    };
    Ok((
        SourceOrigin::Mcp {
            server: m.server.clone(),
            tool: m.tool.clone(),
            args: m.args.clone(),
            label,
        },
        None,
    ))
}

/// Run the row's route. Errors are the `error` column; [`crate::ingest::permanent`] sorts them.
pub async fn extract(ctx: &Ctx, row: &SourceRow, progress: Progress) -> anyhow::Result<Extracted> {
    match &row.origin {
        SourceOrigin::File { .. } => file::extract(ctx, row).await,
        SourceOrigin::Url { url, .. } => url::extract(ctx, url).await,
        SourceOrigin::Meeting { meeting } => meeting::extract(ctx, meeting),
        SourceOrigin::Text { .. } => text::extract(ctx, row),
        SourceOrigin::Session { session } => session::extract(ctx, session).await,
        SourceOrigin::Mcp {
            server,
            tool,
            args,
            label,
        } => mcp::extract(ctx, server, tool, args, *label).await,
        SourceOrigin::Youtube { video, url, .. } => {
            youtube::extract(ctx, video, url, progress).await
        }
        SourceOrigin::Audio { .. } => audio::extract(ctx, row, progress).await,
        SourceOrigin::Video { .. } => video::extract(ctx, row, progress).await,
    }
}

/// `too large: <n> MB over <cap> MB` (§4.1 end).
pub fn too_large(bytes: u64, cap: u64) -> String {
    format!(
        "{TOO_LARGE}: {:.1} MB over the {:.0} MB cap (notebooks.max_source_bytes)",
        mb(bytes),
        mb(cap)
    )
}

/// `415 unsupported: <ext>`.
pub fn unsupported(ext: &str) -> String {
    format!(
        "{UNSUPPORTED}: {}",
        if ext.is_empty() {
            "(no extension)"
        } else {
            ext
        }
    )
}

/// The tool registry when this engine runs one (the plugin's `lookup_when` has landed by the time a
/// job runs; tests have none).
pub fn registry() -> Option<ActorRef<tools::ToolRegistry>> {
    ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
        .ok()
        .flatten()
}

/// Call a registered tool (`scrape_url`, `session_read`, an MCP tool) the way a plugin does: a quiet
/// call under a fresh session; a tool error is an `Err` here.
pub async fn invoke(
    reg: &ActorRef<tools::ToolRegistry>,
    name: &str,
    args: serde_json::Value,
    deadline: Duration,
) -> anyhow::Result<tools::ToolOutput> {
    let call = tools::ToolCall {
        id: format!("ingest-{}", uuid::Uuid::now_v7()),
        name: name.to_owned(),
        args,
        session: runtime::SessionId::new(),
        quiet: true,
    };
    let out = tokio::time::timeout(deadline, reg.ask(tools::Invoke(call)))
        .await
        .map_err(|_| anyhow::anyhow!("{name} timed out after {} s", deadline.as_secs()))?
        .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    anyhow::ensure!(!out.is_error, "{name}: {}", out.content.trim());
    Ok(out)
}

fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}
