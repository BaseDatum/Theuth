//! `scrape_url`, `crawl`, `map` (design doc 38 §6). Risk `ReadOnly`.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};
use url::Url;

use crate::ProgressCx;
use crate::crawl::CrawlOpts;
use crate::engine::Scrape;

/// Plugin prompt block: when to reach for scrape vs `read_url`. Runtime
/// functions (08 §0.4): not direct tool definitions.
pub const PROMPT: &str = "scrape_url, crawl and map are runtime functions \
(pages = await crawl(url=\"…\")). scrape_url reads one page to markdown (JS via rendered: true). \
crawl walks internal links with limit/max_depth/ignore_paths. \
map lists URLs (sitemap first). Prefer these over read_url when you need a site, not one page.";

/// Register the three tools and the prompt block.
pub async fn register(reg: &ActorRef<ToolRegistry>, scrape: Arc<Scrape>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "scrape",
            text: PROMPT.to_owned(),
        })
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(ScrapeUrlTool(Arc::clone(&scrape)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CrawlTool(Arc::clone(&scrape)))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(MapTool(scrape)))).await;
}

fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

fn parse_url(s: &str) -> anyhow::Result<Url> {
    let u = Url::parse(s).map_err(|e| anyhow::anyhow!("invalid url: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        anyhow::bail!("only http(s) URLs");
    }
    Ok(u)
}

/// Output format for [`ScrapeUrlTool`].
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PageFormat {
    /// Post-processed markdown (default).
    #[default]
    Markdown,
    /// Raw HTML from the fetch tier.
    Html,
    /// Outbound hrefs.
    Links,
}

/// Arguments for [`ScrapeUrlTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScrapeUrlArgs {
    /// Absolute http(s) URL.
    pub url: String,
    /// `markdown` (default), `html`, and/or `links`.
    #[serde(default)]
    pub formats: Option<Vec<PageFormat>>,
    /// Drop chrome via `transform_html` before converting.
    #[serde(default)]
    pub only_main_content: Option<bool>,
    /// Markdown cap in characters (default 100000).
    #[serde(default)]
    pub max_chars: Option<u32>,
    /// Force the obscura render tier.
    #[serde(default)]
    pub rendered: Option<bool>,
}

/// `scrape_url`: one page, fetch then optional render.
pub struct ScrapeUrlTool(pub Arc<Scrape>);

#[async_trait::async_trait]
impl Tool for ScrapeUrlTool {
    const NAME: &'static str = "scrape_url";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = ScrapeUrlArgs;

    fn description(&self) -> String {
        "Fetch a URL to markdown (and optionally html/links). rendered: true uses the embedded browser for JS-only pages. Prefer crawl to walk a site.".into()
    }

    async fn call(&self, a: ScrapeUrlArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = parse_url(&a.url)?;
        let prog = ProgressCx {
            session: cx.session,
            call_id: cx.call_id.clone(),
        };
        let page = self
            .0
            .scrape_url(
                &url,
                a.rendered.unwrap_or(false),
                a.only_main_content.unwrap_or(false),
                Some(&prog),
            )
            .await?;
        let formats = a.formats.unwrap_or_else(|| vec![PageFormat::Markdown]);
        let max_chars = a.max_chars.unwrap_or(100_000) as usize;
        let mut text = String::new();
        let mut data = serde_json::json!({
            "url": page.url,
            "title": page.title,
            "tier": page.tier,
        });
        for f in formats {
            match f {
                PageFormat::Markdown => {
                    let md = clip(&page.markdown, max_chars);
                    data["markdown"] = md.clone().into();
                    if text.is_empty() {
                        text = md;
                    }
                }
                PageFormat::Html => {
                    data["html"] = clip(&page.html, max_chars).into();
                }
                PageFormat::Links => {
                    data["links"] = serde_json::to_value(&page.links)?;
                }
            }
        }
        if text.is_empty() {
            text = page.url;
        }
        Ok(structured(text, data))
    }
}

/// Arguments for [`CrawlTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CrawlArgs {
    /// Seed URL; directory-seed scope applies when `same_domain` is true.
    pub url: String,
    /// Page budget (default `scrape.max_pages`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Hop-depth cap (default `scrape.max_depth`).
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// Path regexes to skip (`filter_links` excludePaths).
    #[serde(default)]
    pub ignore_paths: Option<Vec<String>>,
    /// Path regexes that must match.
    #[serde(default)]
    pub include_paths: Option<Vec<String>>,
    /// Stay on the seed host (default `scrape.same_domain`).
    #[serde(default)]
    pub same_domain: Option<bool>,
    /// Use only sitemap URLs; no HTML link walk.
    #[serde(default)]
    pub only_sitemap: Option<bool>,
    /// `markdown` (default) or `links`.
    #[serde(default)]
    pub format: Option<String>,
}

/// `crawl`: bounded BFS over in-scope links.
pub struct CrawlTool(pub Arc<Scrape>);

#[async_trait::async_trait]
impl Tool for CrawlTool {
    const NAME: &'static str = "crawl";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = CrawlArgs;

    fn description(&self) -> String {
        "Crawl a site from a seed URL. Honours limit, max_depth, ignore_paths/include_paths, same_domain, robots.txt. Directory seeds stay under that directory.".into()
    }

    async fn call(&self, a: CrawlArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = parse_url(&a.url)?;
        let mut opts = CrawlOpts::from_cfg(&self.0.cfg);
        if let Some(n) = a.limit {
            opts.limit = (n as usize).max(1);
        }
        if let Some(d) = a.max_depth {
            opts.max_depth = d;
        }
        if let Some(p) = a.ignore_paths {
            opts.exclude_paths = p;
        }
        if let Some(p) = a.include_paths {
            opts.include_paths = p;
        }
        if let Some(s) = a.same_domain {
            opts.same_domain = s;
        }
        opts.only_sitemap = a.only_sitemap.unwrap_or(false);
        let prog = ProgressCx {
            session: cx.session,
            call_id: cx.call_id.clone(),
        };
        let pages = self.0.crawl(url, opts, Some(&prog)).await?;
        let links_only = a.format.as_deref() == Some("links");
        let mut text = String::new();
        let mut rows = Vec::new();
        for p in &pages {
            if links_only {
                text.push_str(&p.url);
                text.push('\n');
                rows.push(serde_json::json!({"url": p.url, "title": p.title}));
            } else {
                text.push_str("# ");
                text.push_str(&p.url);
                text.push('\n');
                if let Some(t) = &p.title {
                    text.push_str(t);
                    text.push('\n');
                }
                text.push_str(&p.markdown);
                text.push_str("\n\n");
                rows.push(serde_json::json!({
                    "url": p.url,
                    "title": p.title,
                    "markdown": p.markdown,
                    "tier": p.tier,
                }));
            }
        }
        if text.is_empty() {
            text.push_str("no pages");
        }
        Ok(structured(text, rows))
    }
}

/// Arguments for [`MapTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MapArgs {
    /// Seed URL.
    pub url: String,
    /// Max URLs (default `scrape.max_pages`).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `map`: sitemap-first URL discovery.
pub struct MapTool(pub Arc<Scrape>);

#[async_trait::async_trait]
impl Tool for MapTool {
    const NAME: &'static str = "map";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = MapArgs;

    fn description(&self) -> String {
        "List URLs on a site: sitemap first, then a link walk. Returns urls and titles, not page bodies.".into()
    }

    async fn call(&self, a: MapArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = parse_url(&a.url)?;
        let limit = a
            .limit
            .map(|n| n as usize)
            .unwrap_or(self.0.cfg.max_pages)
            .max(1);
        let prog = ProgressCx {
            session: cx.session,
            call_id: cx.call_id.clone(),
        };
        let hits = self.0.map(url, limit, Some(&prog)).await?;
        let mut text = String::new();
        let mut rows = Vec::new();
        for h in &hits {
            match &h.title {
                Some(t) => text.push_str(&format!("{}  {t}\n", h.url)),
                None => text.push_str(&format!("{}\n", h.url)),
            }
            rows.push(serde_json::json!({"url": h.url, "title": h.title}));
        }
        if text.is_empty() {
            text.push_str("no urls");
        }
        Ok(structured(text, rows))
    }
}

fn clip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    s.chars().take(max_chars).collect()
}

const _: () = {
    assert!(!<ScrapeUrlTool as Tool>::DIRECT);
    assert!(!<CrawlTool as Tool>::DIRECT);
    assert!(!<MapTool as Tool>::DIRECT);
};
