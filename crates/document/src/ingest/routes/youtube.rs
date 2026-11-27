// 144-W1 owns this file.
//! YouTube (§4.1 row 8, §4.1.1): captions through the stealth fetcher (two fetches), else audio
//! through `yt-dlp` when it is on PATH (or `notebooks.youtube_ytdlp`) and the audio route.

use std::path::PathBuf;
#[cfg(not(target_os = "ios"))]
use std::time::Duration;

use crate::ingest::captions;
use crate::ingest::{Ctx, Extracted, NO_ROUTE, YoutubeRouteCfg, timed_blocks};
use crate::notebook::{SourceOrigin, YoutubeRoute};

pub use captions::video_id;

/// Caption blocks span about a minute (§4.1).
pub const BLOCK_MS: u64 = 60_000;
/// A `yt-dlp` download.
#[cfg(not(target_os = "ios"))]
const YTDLP_DEADLINE: Duration = Duration::from_secs(15 * 60);

/// `notebooks.youtube_ytdlp`, else `yt-dlp` on PATH; `None` → no audio route.
pub fn ytdlp(ctx: &Ctx) -> Option<PathBuf> {
    #[cfg(target_os = "ios")]
    {
        let _ = ctx;
        None
    }
    #[cfg(not(target_os = "ios"))]
    {
        if let Some(p) = &ctx.cfg.youtube_ytdlp {
            let p = PathBuf::from(p);
            return p.is_file().then_some(p);
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join("yt-dlp"))
            .find(|p| p.is_file())
    }
}

pub async fn preflight(
    ctx: &Ctx,
    video: String,
    url: &str,
) -> anyhow::Result<(SourceOrigin, Option<String>)> {
    let has_ytdlp = ytdlp(ctx).is_some();
    let (route, refusal) = match ctx.cfg.youtube_route {
        YoutubeRouteCfg::Captions => (YoutubeRoute::Captions, None),
        YoutubeRouteCfg::Auto => (YoutubeRoute::Captions, None),
        YoutubeRouteCfg::Audio if has_ytdlp => (YoutubeRoute::Audio, None),
        YoutubeRouteCfg::Audio => (
            YoutubeRoute::Audio,
            Some(format!(
                "{NO_ROUTE}: notebooks.youtube_route is audio and yt-dlp is not on PATH (notebooks.youtube_ytdlp)"
            )),
        ),
    };
    Ok((
        SourceOrigin::Youtube {
            video,
            url: url.trim().to_owned(),
            route,
        },
        refusal,
    ))
}

pub async fn extract(
    ctx: &Ctx,
    video: &str,
    url: &str,
    progress: super::Progress,
) -> anyhow::Result<Extracted> {
    anyhow::ensure!(captions::valid_id(video), "bad video id {video:?}");
    let watch = format!("https://www.youtube.com/watch?v={video}");
    let caption_err = match ctx.cfg.youtube_route {
        YoutubeRouteCfg::Audio => None,
        _ => match captions_route(ctx, video, &watch).await {
            Ok(ex) => return Ok(ex),
            Err(e) => Some(e),
        },
    };
    if ctx.cfg.youtube_route == YoutubeRouteCfg::Captions {
        return Err(caption_err.expect("captions ran"));
    }
    let Some(bin) = ytdlp(ctx) else {
        return Err(match caption_err {
            Some(e) => anyhow::anyhow!(
                "{NO_ROUTE}: captions failed ({e:#}) and yt-dlp is not on PATH (notebooks.youtube_ytdlp)"
            ),
            None => anyhow::anyhow!("{NO_ROUTE}: yt-dlp is not on PATH (notebooks.youtube_ytdlp)"),
        });
    };
    #[cfg(target_os = "ios")]
    {
        let _ = (bin, url, progress);
        unreachable!("ytdlp() always returns None on iOS")
    }
    #[cfg(not(target_os = "ios"))]
    {
        let out = ctx
            .data_dir
            .join("attachments")
            .join(format!("{video}.m4a"));
        std::fs::create_dir_all(ctx.data_dir.join("attachments"))?;
        if !out.is_file() {
            download(&bin, &watch, &out).await?;
        }
        let mut ex = super::audio::transcribe(&out, progress).await?;
        let details = reqwest_free_details(ctx, &watch).await;
        ex.title = details.title.or(ex.title);
        ex.meta = serde_json::json!({ "url": url, "video": video, "route": "audio", "channel": details.channel, "captions_error": caption_err.map(|e| format!("{e:#}")) });
        ex.blob = Some(out.to_string_lossy().into_owned());
        ex.origin = Some(SourceOrigin::Youtube {
            video: video.to_owned(),
            url: url.to_owned(),
            route: YoutubeRoute::Audio,
        });
        Ok(ex)
    }
}

/// Step 1: the watch page, `captionTracks`, the track as `json3`, `## [mm:ss]` blocks.
async fn captions_route(ctx: &Ctx, video: &str, watch: &str) -> anyhow::Result<Extracted> {
    let f = super::url::fetcher(ctx)?;
    let page = f.fetch_html(watch, 6_000_000).await?;
    let tracks = captions::tracks(&page.content);
    if tracks.is_empty() {
        let why = match &page.issue {
            Some(i) => format!("{:?}", i.kind),
            None if page.content.contains("captionTracks") => "captions unreadable".to_owned(),
            None => "no captions".to_owned(),
        };
        anyhow::bail!("{why}");
    }
    let track = captions::pick(&tracks, &ctx.cfg.youtube_langs).expect("non-empty");
    let sep = if track.base_url.contains('?') {
        '&'
    } else {
        '?'
    };
    let json = f
        .request(
            "GET",
            &format!("{}{sep}fmt=json3", track.base_url),
            &[],
            None,
        )
        .await?;
    anyhow::ensure!(json.status < 400, "caption track answered {}", json.status);
    let words = captions::json3_words(&String::from_utf8_lossy(&json.body))?;
    anyhow::ensure!(!words.is_empty(), "the caption track is empty");
    let markdown = timed_blocks(words.iter().cloned(), BLOCK_MS);
    let d = captions::details(&page.content);
    let duration_ms = d.length_ms.or_else(|| words.last().map(|(t, _)| *t));
    Ok(Extracted {
        bytes: Some(json.body.len() as u64),
        markdown,
        title: d.title.or(page.title),
        meta: serde_json::json!({ "url": watch, "video": video, "route": "captions", "lang": track.lang, "kind": track.kind, "channel": d.channel }),
        pages: None,
        duration_ms,
        blob: None,
        origin: Some(SourceOrigin::Youtube {
            video: video.to_owned(),
            url: watch.to_owned(),
            route: YoutubeRoute::Captions,
        }),
    })
}

/// Title and channel for the audio route (one fetch; a failure just leaves them empty).
#[cfg(not(target_os = "ios"))]
async fn reqwest_free_details(ctx: &Ctx, watch: &str) -> captions::Details {
    match super::url::fetcher(ctx) {
        Ok(f) => match f.fetch_html(watch, 2_000_000).await {
            Ok(p) => {
                let mut d = captions::details(&p.content);
                d.title = d.title.or(p.title);
                d
            }
            Err(_) => captions::Details::default(),
        },
        Err(_) => captions::Details::default(),
    }
}

/// `yt-dlp -x --audio-format m4a -o <out> <url>`: fixed argv, the id validated above (§15).
#[cfg(not(target_os = "ios"))]
async fn download(bin: &std::path::Path, watch: &str, out: &std::path::Path) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["-x", "--audio-format", "m4a", "--no-playlist", "-o"])
        .arg(out)
        .arg(watch)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    platform::process::hide_console(cmd.as_std_mut());
    let o = tokio::time::timeout(YTDLP_DEADLINE, cmd.output())
        .await
        .map_err(|_| anyhow::anyhow!("yt-dlp timed out after {} s", YTDLP_DEADLINE.as_secs()))??;
    anyhow::ensure!(
        o.status.success() && out.is_file(),
        "yt-dlp failed ({}): {}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
            .lines()
            .last()
            .unwrap_or("")
            .trim()
    );
    Ok(())
}
