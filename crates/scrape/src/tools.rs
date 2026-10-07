//! `scrape_url`, `crawl`, `map_site` (design doc 38 §6). Risk `ReadOnly`.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};
use url::Url;

use crate::ProgressCx;
use crate::crawl::{COMPACT, CrawlOpts, Crawled, MapHit, PAGE_CAP, Page, SitemapMode};
use crate::engine::Scrape;
use web::fetch::meta::PageMeta;

/// Plugin prompt block: when to reach for scrape vs `read_url`. Runtime
/// functions (08 §0.4): not direct tool definitions.
pub const PROMPT: &str = "`scrape_url()`, `crawl()` and `map_site()` are runtime functions (python: \
`result = await crawl(url=\"…\")`; pages are `result[\"pages\"]`). `scrape_url()` reads one page to markdown \
(rendered: true for JS-only pages; a result that starts with `ISSUE:` is a wall, not content; a cut result \
carries next_offset). `crawl()` walks a site: limit, max_depth, include_paths/ignore_paths, sitemap \
(include|skip|only); it says `incomplete` and why. `map_site()` lists a site's URLs without page bodies \
(sitemap + links + the unfetched frontier; search=\"…\" ranks URLs by a query). \
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
    /// Markdown cap in characters (default 100000); a cut result carries next_offset and the omitted headings.
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
    /// Which HTTP client answered (`reqwest` or `wreq_chrome`).
    #[serde(default)]
    pub client: web::fetch::client::ClientKind,
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
    /// Which HTTP client answered (`reqwest` or `wreq_chrome`).
    #[serde(default)]
    pub client: web::fetch::client::ClientKind,
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
        "Fetch one page to markdown. Returns {url, title, tier, markdown} plus html/links when asked in formats, with issue (bot wall / login / consent and the next rung), meta (author, published, canonical) and a truthful cut (next_offset). only_main_content drops chrome on the fetch tier and trims link-only blocks on the render tier; rendered: true forces the embedded browser; tier says fetch, island, render or api. Use crawl for a site, map_site for its URLs.".into()
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
                a.source.unwrap_or_default(),
                Some(&prog),
            )
            .await?;
        let formats = a.formats.unwrap_or_else(|| vec![PageFormat::Markdown]);
        let via = page.via.clone();
        let resp = scrape_response(
            page,
            &formats,
            a.max_chars.unwrap_or(100_000) as usize,
            a.offset.unwrap_or(0) as usize,
            a.compact.unwrap_or(true),
        );
        let text = scrape_text(&resp, via.as_deref());
        Ok(structured(text, resp))
    }
}

/// `Page` → [`ScrapeResponse`] (122 §4.3): `compact` (D7) then `cut` (D6) on
/// the markdown, `cut` from the same `offset` on the html, links verbatim.
fn scrape_response(
    page: Page,
    formats: &[PageFormat],
    max_chars: usize,
    offset: usize,
    compact: bool,
) -> ScrapeResponse {
    let mut r = ScrapeResponse {
        url: page.url,
        title: page.title,
        tier: page.tier.to_owned(),
        issue: page.issue,
        meta: page.meta,
        charset: page.charset,
        source_kind: page.source_kind.map(str::to_owned),
        notes: page.notes,
        client: page.client,
        ..ScrapeResponse::default()
    };
    if formats.contains(&PageFormat::Markdown) {
        let md = if compact {
            let c = web::compact::compact(&page.markdown, &COMPACT);
            r.compacted_chars = c.removed_chars;
            c.text
        } else {
            page.markdown
        };
        let cut = web::fetch::cut(&md, offset, max_chars);
        r.markdown = Some(cut.text);
        r.truncated = cut.truncated;
        r.next_offset = cut.next_offset;
        r.omitted_headings = cut.omitted_headings;
    }
    if formats.contains(&PageFormat::Html) {
        let cut = web::fetch::cut(&page.html, offset, max_chars);
        r.html = Some(cut.text);
        r.truncated |= cut.truncated;
        if r.next_offset.is_none() {
            r.next_offset = cut.next_offset;
        }
    }
    if formats.contains(&PageFormat::Links) {
        r.links = Some(page.links);
    }
    r
}

/// The text half of `scrape_url` (122 §4.1 shape): the ISSUE line when there
/// is one, `# title`, the URL (`· via <api url>` for `tier: api`), the meta
/// line, then the markdown (its cut marker included).
fn scrape_text(r: &ScrapeResponse, via: Option<&str>) -> String {
    let mut text = String::new();
    if let Some(issue) = &r.issue {
        text.push_str(&web::render::issue_line(issue, r.client));
        text.push_str("\n\n");
    }
    if let Some(t) = &r.title {
        text.push_str("# ");
        text.push_str(t);
        text.push('\n');
    }
    text.push_str(&r.url);
    if let Some(v) = via {
        text.push_str(" · via ");
        text.push_str(v);
    }
    text.push('\n');
    if let Some(line) = meta_line(&r.meta) {
        text.push_str(&line);
        text.push('\n');
    }
    if let Some(md) = &r.markdown {
        text.push('\n');
        text.push_str(md);
    }
    text
}

/// `published … · by … · lang …` (122 D5); `None` when every field is empty.
fn meta_line(m: &PageMeta) -> Option<String> {
    let parts: Vec<String> = [
        m.published.as_ref().map(|p| format!("published {p}")),
        m.author.as_ref().map(|a| format!("by {a}")),
        m.lang.as_ref().map(|l| format!("lang {l}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
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
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = CrawlArgs;

    fn description(&self) -> String {
        "Crawl a site from a seed URL. Returns {pages: [{url, title, markdown, tier, issue, truncated}], incomplete, reason (deadline | limit), skipped: [{url, error}], invalid_patterns}. Bounded by limit, max_depth and max_chars_per_page; include_paths/ignore_paths scope the walk (a pattern that does not compile is reported, not ignored); sitemap include|skip|only; honours robots.txt. Stays under the seed directory unless crawl_entire_domain.".into()
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
        opts.max_chars_per_page = a.max_chars_per_page.map_or(PAGE_CAP, |n| n as usize).max(1);
        let prog = progress(&cx);
        let crawled = self.0.crawl(url, opts, Some(&prog)).await?;
        let format = a.format.unwrap_or_default();
        let result = crawl_result(crawled, format);
        let text = crawl_text(&result, format);
        Ok(structured(text, result))
    }
}

/// `Crawled` → [`CrawlResult`] (122 D10); rows are already compacted and
/// cut at `max_chars_per_page`. `format: links` leaves the markdown out.
fn crawl_result(c: Crawled, format: PageFormat) -> CrawlResult {
    let pages = c
        .pages
        .into_iter()
        .map(|p| CrawlPage {
            url: p.url,
            title: p.title,
            markdown: if format == PageFormat::Links {
                String::new()
            } else {
                p.markdown
            },
            tier: p.tier.to_owned(),
            issue: p.issue,
            meta: p.meta,
            truncated: p.truncated,
            next_offset: p.next_offset,
            html: (format == PageFormat::Html).then_some(p.html),
            client: p.client,
        })
        .collect();
    CrawlResult {
        pages,
        incomplete: c.incomplete,
        reason: c.reason,
        skipped: c.skipped,
        invalid_patterns: c.invalid_patterns,
    }
}

/// Per-page `# url` / title / markdown as before, then one line naming what
/// the crawl missed when anything did.
fn crawl_text(r: &CrawlResult, format: PageFormat) -> String {
    let mut text = String::new();
    for p in &r.pages {
        if format == PageFormat::Links {
            text.push_str(&p.url);
            text.push('\n');
            continue;
        }
        text.push_str("# ");
        text.push_str(&p.url);
        text.push('\n');
        if let Some(t) = &p.title {
            text.push_str(t);
            text.push('\n');
        }
        if let Some(issue) = &p.issue {
            text.push_str(&web::render::issue_line(issue, p.client));
            text.push('\n');
        }
        text.push_str(&p.markdown);
        text.push_str("\n\n");
    }
    if text.is_empty() {
        text.push_str("no pages\n");
    }
    let mut tail: Vec<String> = Vec::new();
    if r.incomplete {
        tail.push(format!("incomplete: {}", r.reason.unwrap_or("cut")));
    }
    if !r.skipped.is_empty() {
        tail.push(format!("skipped {}", r.skipped.len()));
    }
    if !r.invalid_patterns.is_empty() {
        tail.push(format!("invalid patterns {}", r.invalid_patterns.len()));
    }
    if !tail.is_empty() {
        text.push_str(&tail.join(" · "));
        text.push('\n');
    }
    text
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
        "List a site's URLs without fetching bodies: sitemap, then links from a short walk, then the walk's unfetched frontier; search=\"…\" puts URLs relevant to a query first. Returns one {url, title, source} per hit (title only where a page was fetched).".into()
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
        let mut rows = Vec::with_capacity(hits.len());
        for h in &hits {
            text.push_str(&map_line(h));
            text.push('\n');
            rows.push(serde_json::json!({"url": h.url, "title": h.title, "source": h.source}));
        }
        if text.is_empty() {
            text.push_str("no urls");
        }
        Ok(structured(text, rows))
    }
}

/// `url  title  [source]` (122 D10); the title column is absent when unknown.
fn map_line(h: &MapHit) -> String {
    match &h.title {
        Some(t) => format!("{}  {t}  [{}]", h.url, h.source),
        None => format!("{}  [{}]", h.url, h.source),
    }
}

const _: () = {
    assert!(!<ScrapeUrlTool as Tool>::DIRECT);
    assert!(!<CrawlTool as Tool>::DIRECT);
    assert!(!<MapTool as Tool>::DIRECT);
};

#[cfg(test)]
mod tests {
    use super::*;
    use web::fetch::client::ClientKind;
    use web::fetch::issue::{IssueKind, Next, PageIssue};

    fn page(markdown: &str) -> Page {
        Page {
            url: "https://e.com/a".into(),
            title: Some("Title".into()),
            markdown: markdown.into(),
            html: "<html><body><p>hi</p></body></html>".into(),
            links: vec!["https://e.com/b".into()],
            tier: "fetch",
            issue: None,
            meta: PageMeta::default(),
            charset: "utf-8".into(),
            truncated: false,
            next_offset: None,
            client: ClientKind::Reqwest,
            source_kind: None,
            via: None,
            notes: Vec::new(),
        }
    }

    /// Thirty sections whose paragraphs all differ (compaction would drop
    /// repeated blocks and the window would never need a cut).
    fn long_markdown() -> String {
        (0..30)
            .map(|i| {
                let words = format!("Words of prose number {i} follow. ").repeat(4);
                format!("## Section {i}\n\nParagraph {i} opens here. {words}\n\n")
            })
            .collect()
    }

    #[test]
    fn scrape_response_cuts_with_offset_and_chains() {
        let md = long_markdown();
        let total = md.chars().count();
        let r = scrape_response(page(&md), &[PageFormat::Markdown], 400, 0, true);
        assert!(r.truncated);
        let next = r.next_offset.expect("next_offset after a cut");
        assert!(next > 0 && next <= 400, "{next}");
        let text = r.markdown.as_deref().unwrap();
        assert!(text.starts_with("## Section 0"), "{text}");
        assert_eq!(r.tier, "fetch");
        assert_eq!(r.charset, "utf-8");
        assert_eq!(r.html, None);
        assert_eq!(r.links, None);

        // The next window starts where the first said.
        let r2 = scrape_response(page(&md), &[PageFormat::Markdown], 1_000_000, next, true);
        assert!(!r2.truncated);
        assert_eq!(r2.next_offset, None);
        let expect: String = md.chars().skip(next).collect();
        assert!(
            r2.markdown.as_deref().unwrap().trim_start().starts_with(
                expect
                    .trim_start()
                    .chars()
                    .take(20)
                    .collect::<String>()
                    .as_str()
            ),
            "window starts at next_offset"
        );

        // Past the end: nothing omitted.
        let r3 = scrape_response(page(&md), &[PageFormat::Markdown], 400, total + 5, true);
        assert!(!r3.truncated);
        assert_eq!(r3.next_offset, None);

        // Fits: untouched.
        let r4 = scrape_response(page("short"), &[PageFormat::Markdown], 400, 0, false);
        assert_eq!(r4.markdown.as_deref(), Some("short"));
        assert!(!r4.truncated);
        assert_eq!(r4.compacted_chars, 0);
    }

    #[test]
    fn scrape_response_html_and_links_formats() {
        let r = scrape_response(
            page("md"),
            &[PageFormat::Html, PageFormat::Links],
            10,
            0,
            true,
        );
        assert_eq!(r.markdown, None);
        assert!(r.truncated, "html cut at 10 chars");
        assert!(r.next_offset.is_some());
        assert_eq!(
            r.links.as_deref(),
            Some(&["https://e.com/b".to_owned()][..])
        );
    }

    #[test]
    fn scrape_text_has_issue_header_url_meta_then_body() {
        let mut p = page("Body text.");
        p.issue = Some(PageIssue {
            kind: IssueKind::AccessChallenge,
            detail: "HTTP 403 · cloudflare".into(),
            retry_after: None,
            next: vec![Next::Rendered, Next::ImportCookies, Next::Handoff],
        });
        p.meta = PageMeta {
            published: Some("2026-09-30T08:00:00Z".into()),
            author: Some("Jane Doe".into()),
            lang: Some("en".into()),
            ..PageMeta::default()
        };
        p.client = ClientKind::WreqChrome;
        let r = scrape_response(p, &[PageFormat::Markdown], 1000, 0, true);
        let text = scrape_text(&r, Some("https://api.example/x"));
        assert_eq!(
            text,
            "ISSUE: access_challenge (HTTP 403 · cloudflare) — next: rendered: true | browser_import_cookies | desktop_handoff · client: wreq_chrome\n\n\
             # Title\nhttps://e.com/a · via https://api.example/x\npublished 2026-09-30T08:00:00Z · by Jane Doe · lang en\n\nBody text."
        );

        let plain = scrape_response(page("Body."), &[PageFormat::Links], 1000, 0, true);
        assert_eq!(scrape_text(&plain, None), "# Title\nhttps://e.com/a\n");
    }

    #[test]
    fn crawl_result_rows_and_tail_line() {
        let mut p = page("Body.");
        p.truncated = true;
        p.next_offset = Some(5);
        let c = Crawled {
            pages: vec![p],
            incomplete: true,
            reason: Some("limit"),
            skipped: vec![Skipped {
                url: "https://e.com/x".into(),
                error: "HTTP 500".into(),
            }],
            invalid_patterns: vec!["(x: error".into()],
        };
        let r = crawl_result(c.clone(), PageFormat::Markdown);
        assert_eq!(r.pages[0].markdown, "Body.");
        assert!(r.pages[0].truncated);
        assert_eq!(r.pages[0].next_offset, Some(5));
        assert_eq!(r.pages[0].html, None);
        let text = crawl_text(&r, PageFormat::Markdown);
        assert!(
            text.starts_with("# https://e.com/a\nTitle\nBody.\n\n"),
            "{text}"
        );
        assert!(
            text.ends_with("incomplete: limit · skipped 1 · invalid patterns 1\n"),
            "{text}"
        );

        let r = crawl_result(c, PageFormat::Html);
        assert!(r.pages[0].html.is_some());

        let complete = CrawlResult::default();
        assert_eq!(crawl_text(&complete, PageFormat::Markdown), "no pages\n");
        let json = serde_json::to_value(&complete).unwrap();
        assert!(json.get("reason").is_none(), "reason skipped when None");
    }

    #[test]
    fn map_line_prints_source() {
        let hit = MapHit {
            url: "https://e.com/".into(),
            title: Some("Home".into()),
            source: "sitemap",
        };
        assert_eq!(map_line(&hit), "https://e.com/  Home  [sitemap]");
        let bare = MapHit {
            title: None,
            source: "frontier",
            ..hit
        };
        assert_eq!(map_line(&bare), "https://e.com/  [frontier]");
    }
}
