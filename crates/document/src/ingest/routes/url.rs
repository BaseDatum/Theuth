// 144-W1 owns this file.
//! The URL route (§4.1 row 2): the `scrape_url` ladder with `only_main_content` through the tool
//! registry (fetch → data island → render, 122); the plain `web` fetcher when no registry runs. PDF
//! and Office bodies are converted inside the fetcher (`read_body`), so they arrive as markdown.

use std::time::Duration;

use crate::ingest::{Ctx, Extracted, REFUSED};
use crate::notebook::SourceOrigin;

/// A fetch of one page, ladder included.
pub const DEADLINE: Duration = Duration::from_secs(120);

/// SSRF guard (122 §5) at pre-flight: a blocked address is a permanent refusal. The local network
/// passes only when `lan` (the adder's `tools::egress` grant: an owner, never a contact or an
/// unknown caller) and `web.allow_private_network` both say so (16 §5, amended 2026-11-11).
pub async fn preflight(
    rt: &runtime::Runtime,
    url: &str,
    lan: bool,
) -> anyhow::Result<(SourceOrigin, Option<String>)> {
    let url = url.trim().to_owned();
    let canonical = web::url::canonicalize(&url).map(|c| c.key);
    let web_cfg: web::WebConfig = rt.section("web").unwrap_or_default();
    let private = lan && web_cfg.allow_private_network;
    let refusal = match web::fetch::check_url(&url, private).await {
        Ok(_) => None,
        Err(web::fetch::FetchError::Rejected(why)) => Some(format!("{REFUSED} ({why})")),
        Err(web::fetch::FetchError::Failed(why)) => anyhow::bail!("{why}"),
    };
    Ok((SourceOrigin::Url { url, canonical }, refusal))
}

/// The mirror of `scrape::ScrapeResponse` this route reads (the crate itself is not importable).
#[derive(Debug, Default, serde::Deserialize)]
struct Page {
    url: String,
    title: Option<String>,
    #[serde(default)]
    tier: String,
    markdown: Option<String>,
    issue: Option<serde_json::Value>,
    #[serde(default)]
    meta: web::fetch::meta::PageMeta,
    #[serde(default)]
    truncated: bool,
    source_kind: Option<String>,
}

pub async fn extract(ctx: &Ctx, url: &str) -> anyhow::Result<Extracted> {
    let max = ctx.cfg.max_source_chars.min(u32::MAX as usize) as u32;
    let page = match super::registry() {
        Some(reg) => {
            let out = super::invoke(
                &reg,
                "scrape_url",
                serde_json::json!({ "url": url, "only_main_content": true, "formats": ["markdown"], "max_chars": max }),
                DEADLINE,
            )
            .await?;
            match out.structured {
                Some(v) => serde_json::from_value::<Page>(v)?,
                None => Page {
                    url: url.to_owned(),
                    markdown: Some(out.content),
                    ..Default::default()
                },
            }
        }
        None => fetch_direct(ctx, url, max as usize).await?,
    };
    let markdown = page.markdown.unwrap_or_default();
    anyhow::ensure!(
        !markdown.trim().is_empty(),
        "the page has no readable text{}",
        issue_note(&page.issue)
    );
    let title = page.title.clone().or_else(|| page.meta.title.clone());
    Ok(Extracted {
        bytes: Some(markdown.len() as u64),
        markdown,
        title,
        meta: serde_json::json!({
            "url": page.url, "tier": page.tier, "meta": page.meta, "issue": page.issue,
            "truncated": page.truncated, "source_kind": page.source_kind,
        }),
        pages: None,
        duration_ms: None,
        blob: None,
        origin: None,
    })
}

fn issue_note(issue: &Option<serde_json::Value>) -> String {
    issue
        .as_ref()
        .map(|i| {
            format!(
                " ({})",
                i.get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("page issue")
            )
        })
        .unwrap_or_default()
}

/// The fetch rung alone (no registry: tests, a stripped engine).
async fn fetch_direct(ctx: &Ctx, url: &str, max_chars: usize) -> anyhow::Result<Page> {
    let f = fetcher(ctx)?;
    let r = f
        .fetch(url, web::fetch::FetchFormat::Markdown, max_chars)
        .await?;
    Ok(Page {
        url: r.url,
        title: r.title,
        tier: "fetch".into(),
        markdown: Some(r.content),
        issue: r.issue.map(|i| serde_json::to_value(i).unwrap_or_default()),
        meta: r.meta,
        truncated: r.truncated,
        source_kind: r.source_kind,
    })
}

/// The stealth fetcher built as `scrape` builds it from the `web` section (122 R1-D1), its
/// local-network lane included (used only inside the job's grant, 16 §5).
pub fn fetcher(ctx: &Ctx) -> anyhow::Result<web::fetch::Fetcher> {
    let web_cfg: web::WebConfig = ctx.rt.section("web").unwrap_or_default();
    web::fetch::Fetcher::new_with_timeouts(
        &web_cfg.user_agent,
        web_cfg
            .max_fetch_bytes
            .max(ctx.cfg.max_source_bytes as usize),
        web_cfg.max_redirects,
        Duration::from_secs(20),
        web::fetch::CONNECT_TIMEOUT,
    )?
    .with_policy(
        web_cfg.impersonate,
        web_cfg.header_ladder,
        Some(&web_cfg.accept_language),
    )
    .with_private_network(web_cfg.allow_private_network)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAN_URL: &str = "http://192.168.8.123:28499/v1/models";

    /// The refusal [`preflight`] writes on the card for `url` under config `yaml` and grant `lan`.
    async fn refusal(yaml: &str, url: &str, lan: bool) -> Option<String> {
        use figment::providers::{Format, Yaml};
        let rt = runtime::Runtime::new(
            figment::Figment::from(Yaml::string(yaml)),
            std::env::temp_dir(),
        );
        preflight(&rt, url, lan).await.unwrap().1
    }

    /// 16 §5 (2026-11-11): an owner's (granted) LAN URL passes while `web.allow_private_network`
    /// is on, absent included; the metadata endpoint stays refused even then.
    #[tokio::test]
    async fn an_owners_lan_url_passes_with_the_setting_on() {
        for yaml in ["", "web:\n  allow_private_network: true\n"] {
            assert_eq!(refusal(yaml, LAN_URL, true).await, None, "{yaml:?}");
        }
        let metadata = refusal("", "http://169.254.169.254/latest/meta-data", true).await;
        assert!(metadata.is_some_and(|r| r.starts_with(REFUSED)));
    }

    /// Without the grant (a contact's call, an unknown caller) or with the setting off, the same
    /// URL is refused as private.
    #[tokio::test]
    async fn a_lan_url_without_the_grant_or_the_setting_is_refused() {
        for (yaml, lan) in [
            ("", false),
            ("web:\n  allow_private_network: true\n", false),
            ("web:\n  allow_private_network: false\n", true),
        ] {
            let r = refusal(yaml, LAN_URL, lan).await;
            assert!(
                r.as_deref()
                    .is_some_and(|r| r.starts_with(REFUSED) && r.contains("192.168.8.123")),
                "{yaml:?} lan={lan}: {r:?}"
            );
        }
    }
}
