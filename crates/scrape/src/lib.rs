//! Local scrape, crawl and map (design doc 38). `web::Fetch` first, obscura
//! when the fetch comes back thin or `rendered: true`; markdown always runs
//! through `firecrawl_native::post_process_markdown`.

use std::sync::Arc;
use std::time::Duration;

use ::tools::{REGISTRY_ACTOR, ToolRegistry};
use runtime::Runtime;
use serde::{Deserialize, Serialize};
use web::fetch::Fetcher;
pub(crate) use web::progress::ProgressCx;

mod crawl;
mod engine;
mod provider;
mod tools;

pub use crawl::{CrawlOpts, Crawled, MapHit, Page, SitemapMode};
pub use engine::Scrape;
pub use tools::{CrawlPage, CrawlResult, ScrapeResponse, Skipped};

/// `scrape:` config section (38 §6). Every field has a default; the section
/// may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrapeConfig {
    /// `false`: no tools, no render provider.
    pub enabled: bool,
    /// Concurrent page fetches in one crawl.
    pub concurrency: usize,
    /// Default page budget for `crawl` / `map`.
    pub max_pages: usize,
    /// Default hop-depth cap for `crawl`.
    pub max_depth: u32,
    /// Stay on the seed's registrable domain.
    pub same_domain: bool,
    /// Per-page HTTP timeout.
    pub page_timeout_ms: u64,
    /// Hard deadline for one crawl/map run.
    pub run_timeout_ms: u64,
    /// Honour `robots.txt` (`build_robot`).
    pub robots: bool,
    /// Per-host politeness floor between requests in one run; a larger
    /// robots `Crawl-delay` wins (capped at 10 s).
    pub delay_ms: u64,
}

impl Default for ScrapeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            concurrency: 5,
            max_pages: 50,
            max_depth: 3,
            same_domain: true,
            page_timeout_ms: 8000,
            run_timeout_ms: 60000,
            robots: true,
            delay_ms: 100,
        }
    }
}

/// The plugin: reads `scrape`, builds the fetcher, registers `scrape_url` /
/// `crawl` / `map_site` once the tool registry is up, and a `renders: true`
/// provider once `web` is up.
pub struct ScrapePlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ScrapePlugin {
    fn name(&self) -> &'static str {
        "scrape"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: ScrapeConfig = rt.section("scrape").unwrap_or_else(|e| {
            tracing::debug!("no [scrape] config section, using defaults: {e}");
            ScrapeConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("scrape disabled by config");
            return Ok(());
        }
        // Page policy (122 R1-D1/D4) is the `web` section's: same client
        // choice, escalation and `Accept-Language` as `read_url`.
        let web_cfg: web::WebConfig = rt.section("web").unwrap_or_default();
        let fetcher = Fetcher::new_with_timeouts(
            &web_cfg.user_agent,
            web_cfg.max_fetch_bytes.max(8 * 1024 * 1024),
            web_cfg.max_redirects,
            Duration::from_millis(cfg.page_timeout_ms.max(1)),
            web::fetch::CONNECT_TIMEOUT,
        )?
        .with_policy(
            web_cfg.impersonate,
            web_cfg.header_ladder,
            Some(&web_cfg.accept_language),
        )
        .with_private_network(web_cfg.allow_private_network)?;
        let scrape = Arc::new(Scrape::new(fetcher, cfg).with_runtime(rt.clone()));
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                tools::register(&reg, Arc::clone(&scrape)).await;
            }
            provider::register(rt, scrape).await;
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design() {
        let d = ScrapeConfig::default();
        assert!(d.enabled);
        assert_eq!(d.concurrency, 5);
        assert_eq!(d.max_pages, 50);
        assert_eq!(d.max_depth, 3);
        assert!(d.same_domain);
        assert_eq!(d.page_timeout_ms, 8000);
        assert_eq!(d.run_timeout_ms, 60_000);
        assert!(d.robots);
        assert_eq!(d.delay_ms, 100);
    }
}
