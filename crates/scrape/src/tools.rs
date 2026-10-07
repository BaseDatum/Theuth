//! `scrape_url`, `crawl`, `map_site` (design doc 38 §6). Risk `ReadOnly`.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};
use url::Url;

use crate::ProgressCx;
use crate::crawl::{CrawlOpts, SitemapMode};
use crate::engine::Scrape;

/// Plugin prompt block: when to reach for scrape vs `read_url`. Runtime
/// functions (08 §0.4): not direct tool definitions.
pub const PROMPT: &str = "`scrape_url()`, `crawl()` and `map_site()` are runtime functions (python: \
`pages = await crawl(url=\"…\")`). `scrape_url()` reads one page to markdown (rendered: true for JS-only pages). \
`crawl()` walks a site: limit, max_depth, include_paths/ignore_paths, sitemap (include|skip|only). \
`map_site()` lists a site's URLs without page bodies (sitemap + links; search=\"…\" ranks URLs by a query). \
Prefer these over `read_url()` when you need a site, not one page.";

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

fn progress(cx: &ToolCx) -> ProgressCx {
    ProgressCx {
        session: cx.session,
        call_id: cx.call_id.clone(),
    }
}

/// Page output format for [`ScrapeUrlTool`] and [`CrawlTool`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// Strip nav, footer and ads before converting (default true).
    #[serde(default)]
    pub only_main_content: Option<bool>,
    /// Markdown cap in characters (default 100000).
    #[serde(default)]
    pub max_chars: Option<u32>,
    /// Force the embedded browser (JS-only pages). Default false: plain
    /// fetch, browser only when the fetch comes back thin.
    #[serde(default)]
    pub rendered: Option<bool>,
    /// Start at this character offset (default 0); use next_offset from a truncated result.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Remove repeated blocks, image runs, class leaks and invisible characters before the cap (default true).
    #[serde(default)]
    pub compact: Option<bool>,
    /// auto (default): known hosts (HN, Reddit, npm, PyPI, crates.io, arXiv, Wikipedia, Stack Overflow) are read from their JSON API; page: always the HTML; api: fail if no API source matches.
    #[serde(default)]
    pub source: Option<web::tools::Source>,
}

/// `scrape_url`'s structured result (design doc 122 §4.3). Every field added
/// by 122 (`issue` onward) has a serde default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScrapeResponse {
    /// Final URL.
    pub url: String,
    /// Title when known.
    pub title: Option<String>,
    /// `fetch`, `island`, `render` or `api`.
    pub tier: String,
    /// Markdown when asked for (default format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Raw HTML when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Outbound links when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
    /// Why the page did not read as content, with the next rung.
    #[serde(default)]
    pub issue: Option<web::fetch::issue::PageIssue>,
    /// title, description, author, published (RFC 3339), lang, canonical, site_name.
    #[serde(default)]
    pub meta: web::fetch::meta::PageMeta,
    /// Encoding label used to decode the body.
    #[serde(default = "default_charset")]
    pub charset: String,
    /// Characters removed by compaction before the cap.
    #[serde(default)]
    pub compacted_chars: usize,
    /// Whether the markdown (or html) was cut at `max_chars`.
    #[serde(default)]
    pub truncated: bool,
    /// Pass as `offset` to continue; null when nothing was omitted.
    #[serde(default)]
    pub next_offset: Option<usize>,
    /// Headings of the omitted tail (≤ 12).
    #[serde(default)]
    pub omitted_headings: Vec<String>,
    /// The API source used (`hn-item`, `npm`, …).
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Non-fatal remarks.
    #[serde(default)]
    pub notes: Vec<String>,
}

fn default_charset() -> String {
    "utf-8".to_owned()
}

/// A page `crawl` could not read (design doc 122 D10).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Skipped {
    /// The URL that was attempted.
    pub url: String,
    /// The fetch error text or `"timed out after Ns"`.
    pub error: String,
}

/// One row of [`CrawlResult::pages`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CrawlPage {
    /// Final URL.
    pub url: String,
    /// Title when known.
    pub title: Option<String>,
    /// Post-processed markdown, cut at `max_chars_per_page`.
    pub markdown: String,
    /// `fetch`, `island`, `render` or `api`.
    pub tier: String,
    /// Why the page did not read as content, with the next rung.
    #[serde(default)]
    pub issue: Option<web::fetch::issue::PageIssue>,
    /// Page metadata.
    #[serde(default)]
    pub meta: web::fetch::meta::PageMeta,
    /// Whether `markdown` was cut.
    #[serde(default)]
    pub truncated: bool,
    /// Pass as `offset` to `scrape_url` to continue; null when nothing was omitted.
    #[serde(default)]
    pub next_offset: Option<usize>,
    /// Raw HTML when `format: html`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

/// `crawl`'s structured result (design doc 122 D10): the pages plus what was missed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CrawlResult {
    /// Pages read, in crawl order.
    pub pages: Vec<CrawlPage>,
    /// `true` when the frontier was non-empty when the loop ended.
    pub incomplete: bool,
    /// `"deadline"` | `"limit"` | `"cancelled"` when `incomplete`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// Pages that failed or timed out (≤ 200 rows, then `… and N more`).
    #[serde(default)]
    pub skipped: Vec<Skipped>,
    /// `include_paths`/`ignore_paths` entries that did not compile, as `"<pattern>: <error>"`.
    #[serde(default)]
    pub invalid_patterns: Vec<String>,
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
        "Fetch one page to markdown. Returns {url, title, tier, markdown} plus html/links when asked in formats. only_main_content drops chrome; rendered: true uses the embedded browser for JS-only pages. Use crawl for a site, map_site for its URLs.".into()
    }

    async fn call(&self, a: ScrapeUrlArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = parse_url(&a.url)?;
        let prog = progress(&cx);
        let page = self
            .0
            .scrape_url(
                &url,
                a.rendered.unwrap_or(false),
                a.only_main_content.unwrap_or(true),
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
    /// Seed URL. The walk stays under the seed's directory unless
    /// `crawl_entire_domain`.
    pub url: String,
    /// Page budget (default `scrape.max_pages`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Hop-depth cap from the seed; 0 = seed only (default `scrape.max_depth`).
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// Path regexes to skip (`excludePaths`).
    #[serde(default)]
    pub ignore_paths: Option<Vec<String>>,
    /// Path regexes a URL must match (`includePaths`).
    #[serde(default)]
    pub include_paths: Option<Vec<String>>,
    /// Stay on the seed host (default `scrape.same_domain`).
    #[serde(default)]
    pub same_domain: Option<bool>,
    /// `include` (default): seed the walk with sitemap URLs, then follow
    /// links; `skip`: links only; `only`: sitemap URLs only, no link walk.
    #[serde(default)]
    pub sitemap: Option<SitemapMode>,
    /// Follow links outside the seed's directory, anywhere on the host
    /// (default false).
    #[serde(default)]
    pub crawl_entire_domain: Option<bool>,
    /// Follow links to subdomains of the seed host (default false).
    #[serde(default)]
    pub allow_subdomains: Option<bool>,
    /// Match `include_paths`/`ignore_paths` against the full URL instead of
    /// the path (default false).
    #[serde(default)]
    pub regex_on_full_url: Option<bool>,
    /// Treat URLs that differ only by query string as one page (default false).
    #[serde(default)]
    pub ignore_query_parameters: Option<bool>,
    /// Minimum gap in ms between requests to one host (default
    /// `scrape.delay_ms`; a larger robots `Crawl-delay` wins).
    #[serde(default)]
    pub delay_ms: Option<u64>,
    /// Strip nav, footer and ads before converting (default true).
    #[serde(default)]
    pub only_main_content: Option<bool>,
    /// `markdown` (default): url, title and markdown per page; `links`: URLs
    /// only; `html`: markdown plus raw html in the structured rows.
    #[serde(default)]
    pub format: Option<PageFormat>,
    /// Per-page markdown cap (default 20000); a cut page carries next_offset for scrape_url.
    #[serde(default)]
    pub max_chars_per_page: Option<u32>,
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
        "Crawl a site from a seed URL. Returns one {url, title, markdown, tier} per page (url/title only with format: links). Bounded by limit and max_depth; include_paths/ignore_paths scope the walk; sitemap include|skip|only; honours robots.txt. Stays under the seed directory unless crawl_entire_domain.".into()
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
        if let Some(m) = a.sitemap {
            opts.sitemap = m;
        }
        if let Some(d) = a.delay_ms {
            opts.delay_ms = d;
        }
        opts.crawl_entire_domain = a.crawl_entire_domain.unwrap_or(false);
        opts.allow_subdomains = a.allow_subdomains.unwrap_or(false);
        opts.regex_on_full_url = a.regex_on_full_url.unwrap_or(false);
        opts.ignore_query_parameters = a.ignore_query_parameters.unwrap_or(false);
        opts.only_main_content = a.only_main_content.unwrap_or(true);
        let prog = progress(&cx);
        let pages = self.0.crawl(url, opts, Some(&prog)).await?;
        let format = a.format.unwrap_or_default();
        let mut text = String::new();
        let mut rows = Vec::new();
        for p in &pages {
            if format == PageFormat::Links {
                text.push_str(&p.url);
                text.push('\n');
                rows.push(serde_json::json!({"url": p.url, "title": p.title}));
                continue;
            }
            text.push_str("# ");
            text.push_str(&p.url);
            text.push('\n');
            if let Some(t) = &p.title {
                text.push_str(t);
                text.push('\n');
            }
            text.push_str(&p.markdown);
            text.push_str("\n\n");
            let mut row = serde_json::json!({
                "url": p.url,
                "title": p.title,
                "markdown": p.markdown,
                "tier": p.tier,
            });
            if format == PageFormat::Html {
                row["html"] = p.html.clone().into();
            }
            rows.push(row);
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
    /// Seed URL; results stay under its directory.
    pub url: String,
    /// Max URLs (default `scrape.max_pages`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rank URLs by relevance to this query: a site-restricted web search
    /// runs first and its hits lead the list.
    #[serde(default)]
    pub search: Option<String>,
    /// Include subdomains of the seed host (default true).
    #[serde(default)]
    pub include_subdomains: Option<bool>,
    /// Collapse URLs that differ only by query string (default true).
    #[serde(default)]
    pub ignore_query_parameters: Option<bool>,
}

/// `map_site`: URL discovery; sitemap and links, optional search ranking.
pub struct MapTool(pub Arc<Scrape>);

#[async_trait::async_trait]
impl Tool for MapTool {
    const NAME: &'static str = "map_site";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = MapArgs;

    fn description(&self) -> String {
        "List a site's URLs without fetching bodies: sitemap, then links from a short walk; search=\"…\" puts URLs relevant to a query first. Returns one {url, title} per hit (title only where a page was fetched).".into()
    }

    async fn call(&self, a: MapArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = parse_url(&a.url)?;
        let mut opts = CrawlOpts::from_cfg(&self.0.cfg);
        opts.limit = a.limit.map_or(self.0.cfg.max_pages, |n| n as usize).max(1);
        opts.allow_subdomains = a.include_subdomains.unwrap_or(true);
        opts.ignore_query_parameters = a.ignore_query_parameters.unwrap_or(true);
        let prog = progress(&cx);
        let hits = self
            .0
            .map(url, opts, a.search.as_deref(), Some(&prog))
            .await?;
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
