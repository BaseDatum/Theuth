//! Fetch adapter and the engine ladder `fetch → issue → island → render`
//! (38 §0.3, §3; 122 D1 scrape side, D8 render tier, D9, D11 wiring).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use kameo::actor::ActorRef;
use reqwest::header::HeaderMap;
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};
use url::Url;
use web::excerpt::{is_noise, prose_chars};
use web::fetch::client::ClientKind;
use web::fetch::issue::{IssueKind, Next, PageIssue};
use web::fetch::meta::PageMeta;
use web::fetch::{FetchError, Fetcher};
use web::sources::ApiSource;
use web::tools::Source;

use crate::ScrapeConfig;

/// Prose the island rung must recover before it replaces the render tier (122 D9).
const ISLAND_MIN_PROSE: usize = 800;

/// `web::fetch::issue::detect`'s shape; a test seam injects a constructed
/// [`PageIssue`] because the contract stub answers `None`.
pub type Detector = dyn Fn(u16, &HeaderMap, &Url, &str, usize) -> Option<PageIssue> + Send + Sync;

/// `firecrawl_native::extract_islands`'s shape; the test seam injects island
/// prose because the contract stub answers `None`.
pub type Islands = dyn Fn(&str) -> Option<String> + Send + Sync;

/// Body cap for crawl/scrape so tool-sized truncation does not apply.
pub const PAGE_CHARS: usize = 500_000;

/// Bytes of a body inspected by the HTML / SPA sniffers.
const HEAD_BYTES: usize = 4096;

/// Base of the render-tier session ids: slot `i` is `RENDER_BASE + i`,
/// distinct from `browser`'s `read_url` session.
const RENDER_BASE: u128 = 0x7363_7261_7065_5f72_656e_6465_725f_0038;

/// Browser sessions for the render tier. Each slot is one obscura isolate;
/// `cfg.concurrency` of them let `search_web`'s parallel reads and a crawl
/// render concurrently instead of serialising on one isolate. Idle isolates
/// are reclaimed by the browser's idle sweeper and recreated on next use.
struct RenderSlots {
    permits: tokio::sync::Semaphore,
    free: Mutex<Vec<SessionId>>,
}

impl RenderSlots {
    fn new(n: usize) -> Self {
        Self {
            permits: tokio::sync::Semaphore::new(n),
            free: Mutex::new(
                (0..n)
                    .map(|i| SessionId(uuid::Uuid::from_u128(RENDER_BASE + i as u128)))
                    .collect(),
            ),
        }
    }

    /// Wait for a free slot; the session returns to the pool when the guard drops.
    async fn acquire(&self) -> anyhow::Result<RenderSlot<'_>> {
        let permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("render slots closed"))?;
        let session = self
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .ok_or_else(|| anyhow::anyhow!("render slot pool empty under permit"))?;
        Ok(RenderSlot {
            slots: self,
            session,
            _permit: permit,
        })
    }
}

/// One checked-out render session; `Drop` pushes it back before the permit frees.
struct RenderSlot<'a> {
    slots: &'a RenderSlots,
    session: SessionId,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl Drop for RenderSlot<'_> {
    fn drop(&mut self) {
        self.slots
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.session);
    }
}

/// Shared scrape handle: SSRF-guarded fetch, optional obscura, config.
pub struct Scrape {
    /// HTTP fetcher with a large body cap.
    pub(crate) fetcher: Arc<Fetcher>,
    /// `scrape:` section.
    pub(crate) cfg: ScrapeConfig,
    slots: RenderSlots,
    /// Publishes [`crate::ScrapeProgress`]; `None` in unit tests.
    pub(crate) rt: Option<Runtime>,
    /// Issue detector run on 2xx bodies and rendered markdown (122 D1).
    detect: Arc<Detector>,
    /// Data-island extractor for the D9 rung.
    islands: Arc<Islands>,
}

impl Scrape {
    /// Wrap a fetcher. The browser actor is looked up at render time.
    #[must_use]
    pub fn new(fetcher: Fetcher, cfg: ScrapeConfig) -> Self {
        let slots = RenderSlots::new(cfg.concurrency.max(1));
        Self {
            fetcher: Arc::new(fetcher),
            cfg,
            slots,
            rt: None,
            detect: Arc::new(web::fetch::issue::detect),
            islands: Arc::new(firecrawl_native::extract_islands),
        }
    }

    /// Attach the bus so crawl/scrape emit [`crate::ScrapeProgress`].
    #[must_use]
    pub fn with_runtime(mut self, rt: Runtime) -> Self {
        self.rt = Some(rt);
        self
    }

    /// Test seam: replace the issue detector (the contract stub says `None`).
    #[doc(hidden)]
    #[must_use]
    pub fn with_detector(
        mut self,
        detect: impl Fn(u16, &HeaderMap, &Url, &str, usize) -> Option<PageIssue> + Send + Sync + 'static,
    ) -> Self {
        self.detect = Arc::new(detect);
        self
    }

    /// Test seam: replace the island extractor (the contract stub says `None`).
    #[doc(hidden)]
    #[must_use]
    pub fn with_islands(
        mut self,
        islands: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.islands = Arc::new(islands);
        self
    }

    /// [`Self::fetch_page`] behind the D11 source switch: a known host's JSON
    /// API under `auto`/`api` (never when the browser is forced), the page
    /// otherwise. An API failure under `auto` falls back to the page with a note.
    pub async fn fetch_source(
        &self,
        url: &Url,
        force_render: bool,
        only_main_content: bool,
        source: Source,
    ) -> anyhow::Result<Fetched> {
        let api = match source {
            Source::Page => None,
            Source::Auto if force_render => None,
            Source::Auto | Source::Api => web::sources::api_source(url),
        };
        let Some(api) = api else {
            if source == Source::Api {
                anyhow::bail!("no API source matches {url}");
            }
            return self.fetch_page(url, force_render, only_main_content).await;
        };
        match self.fetch_api(url, &api).await {
            Ok(page) => Ok(page),
            Err(e) if source == Source::Api => Err(e),
            Err(e) => {
                let mut page = self
                    .fetch_page(url, force_render, only_main_content)
                    .await?;
                page.notes.push(format!(
                    "api source {} failed: {e}; read the page",
                    api.kind
                ));
                Ok(page)
            }
        }
    }

    /// `tier: "api"`: the JSON endpoint (plus `sources::follow_up`'s, Stack
    /// Overflow answers) through the same fetcher, rendered by `web::sources::render`.
    async fn fetch_api(&self, page_url: &Url, api: &ApiSource) -> anyhow::Result<Fetched> {
        let mut resp = self
            .fetcher
            .fetch_html(api.fetch_url.as_str(), PAGE_CHARS)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(more) = web::sources::follow_up(api) {
            match self.fetcher.fetch_html(more.as_str(), PAGE_CHARS).await {
                Ok(extra) => {
                    resp.content.push('\n');
                    resp.content.push_str(&extra.content);
                }
                Err(e) => resp.notes.push(format!("{more}: {e}")),
            }
        }
        let markdown = web::sources::render(api.kind, &resp.content)
            .ok_or_else(|| anyhow::anyhow!("body did not parse"))?;
        let title = first_heading(&markdown);
        Ok(Fetched {
            url: page_url.to_string(),
            meta: PageMeta {
                title: title.clone(),
                ..PageMeta::default()
            },
            title,
            html: String::new(),
            links: links_from_markdown(&markdown),
            markdown,
            tier: "api",
            issue: None,
            charset: resp.charset,
            client: resp.client,
            source_kind: Some(api.kind),
            via: Some(api.fetch_url.to_string()),
            notes: resp.notes,
        })
    }

    /// Fetch `url` up the ladder: HTTP, then — by the diagnosed issue or a
    /// thin body — the data island, then obscura when `force_render`, the
    /// issue's `next` says `rendered`, or the page is thin.
    pub async fn fetch_page(
        &self,
        url: &Url,
        force_render: bool,
        only_main_content: bool,
    ) -> anyhow::Result<Fetched> {
        if force_render {
            return self.render(url, only_main_content).await;
        }
        let page = match self.fetch_http(url, only_main_content).await {
            Ok(page) => page,
            Err(e) => {
                let Some(actor) = browser_actor() else {
                    return Err(e);
                };
                tracing::debug!(url = %url, error = %e, "fetch failed; trying obscura");
                return self
                    .render_with(&actor, url, only_main_content)
                    .await
                    .map_err(|re| anyhow::anyhow!("{e}; render: {re}"));
            }
        };
        self.ladder(url, page, only_main_content).await
    }

    /// The rungs after a successful HTTP fetch (122 D1 "scrape engine ladder").
    async fn ladder(
        &self,
        url: &Url,
        mut page: Fetched,
        only_main_content: bool,
    ) -> anyhow::Result<Fetched> {
        let thin = is_thin(&page.html, &page.markdown);
        let kind = page.issue.as_ref().map(|i| i.kind);
        let to_render = match kind {
            None if !thin => {
                tracing::debug!(url = %url, tier = "fetch", "scrape engine");
                return Ok(page);
            }
            // Shell or interstitial over content: the island rung first.
            None | Some(IssueKind::EmptyShell | IssueKind::ConsentWall) => {
                if let Some(text) = (self.islands)(&page.html)
                    && prose_chars(&text) >= ISLAND_MIN_PROSE
                {
                    tracing::debug!(url = %url, tier = "island", "scrape engine");
                    page.markdown = firecrawl_native::post_process_markdown(text);
                    page.tier = "island";
                    if kind == Some(IssueKind::EmptyShell) {
                        page.issue = None;
                    }
                    return Ok(page);
                }
                true
            }
            Some(_) => page
                .issue
                .as_ref()
                .is_some_and(|i| i.next.contains(&Next::Rendered)),
        };
        if !to_render {
            return Ok(page);
        }
        let Some(actor) = browser_actor() else {
            return Ok(page);
        };
        tracing::debug!(url = %url, tier = "render", issue = ?kind, "trying obscura");
        match self.render_with(&actor, url, only_main_content).await {
            Ok(rendered) => Ok(rendered),
            Err(e) => {
                tracing::debug!(error = %e, "render failed; keeping fetch");
                Ok(page)
            }
        }
    }

    /// Plain HTTP: HTML (or already-converted document markdown), then
    /// `transform_html` → `html-to-markdown-rs` → `post_process_markdown`;
    /// metadata mapped (122 D5) and the issue detector run on the body (D1).
    pub async fn fetch_http(&self, url: &Url, only_main_content: bool) -> anyhow::Result<Fetched> {
        let resp = self
            .fetcher
            .fetch_html(url.as_str(), PAGE_CHARS)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let html = resp.content;
        let final_url = Url::parse(&resp.url).unwrap_or_else(|_| url.clone());
        if !html_looks_like_html(&html) {
            let markdown = firecrawl_native::post_process_markdown(html);
            return Ok(Fetched {
                url: resp.url,
                meta: PageMeta {
                    title: resp.meta.title.clone().or_else(|| resp.title.clone()),
                    ..resp.meta
                },
                title: resp.title,
                links: links_from_markdown(&markdown),
                html: String::new(),
                markdown,
                tier: "fetch",
                issue: resp.issue,
                charset: resp.charset,
                client: resp.client,
                source_kind: None,
                via: None,
                notes: resp.notes,
            });
        }
        let markdown = to_markdown(&html, url.as_str(), only_main_content);
        let meta = page_meta(&html, &final_url, resp.meta);
        let issue = resp.issue.or_else(|| {
            (self.detect)(
                200,
                &HeaderMap::new(),
                &final_url,
                &html,
                prose_chars(&markdown),
            )
        });
        Ok(Fetched {
            url: resp.url,
            title: resp.title.or_else(|| meta.title.clone()),
            links: firecrawl_native::extract_links(Some(html.clone())).unwrap_or_default(),
            html,
            markdown,
            tier: "fetch",
            issue,
            meta,
            charset: resp.charset,
            client: resp.client,
            source_kind: None,
            via: None,
            notes: resp.notes,
        })
    }

    /// `browser_navigate` → `browser_markdown` → `browser_close` on a free render slot.
    pub async fn render(&self, url: &Url, only_main_content: bool) -> anyhow::Result<Fetched> {
        let actor = browser_actor()
            .ok_or_else(|| anyhow::anyhow!("browser not available for rendered fetch"))?;
        self.render_with(&actor, url, only_main_content).await
    }

    /// The render tier has no DOM: `only_main_content` is the markdown-level
    /// trim of leading/trailing link-only blocks (122 D8 render tier); the
    /// issue is recomputed from the rendered markdown (status 200, no headers).
    async fn render_with(
        &self,
        actor: &ActorRef<browser::Browsers>,
        url: &Url,
        only_main_content: bool,
    ) -> anyhow::Result<Fetched> {
        let slot = self.slots.acquire().await?;
        let session = slot.session;
        let navigated = call(
            actor,
            session,
            "browser_navigate",
            json!({ "url": url.as_str() }),
        )
        .await?;
        let markdown = call(
            actor,
            session,
            "browser_markdown",
            json!({ "max_chars": PAGE_CHARS }),
        )
        .await;
        let _ = call(actor, session, "browser_close", json!({})).await;
        drop(slot);
        let mut markdown = firecrawl_native::post_process_markdown(strip_base64_images(markdown?));
        if only_main_content {
            markdown = trim_link_blocks(&markdown);
        }
        let (final_url, title) = parse_navigated(&navigated).unwrap_or((url.as_str(), None));
        let final_url_parsed = Url::parse(final_url).unwrap_or_else(|_| url.clone());
        let issue = (self.detect)(
            200,
            &HeaderMap::new(),
            &final_url_parsed,
            &markdown,
            prose_chars(&markdown),
        );
        let links = links_from_markdown(&markdown);
        let title = title.map(str::to_owned);
        tracing::debug!(url = %url, tier = "render", "scrape engine");
        Ok(Fetched {
            url: final_url.to_owned(),
            meta: PageMeta {
                title: title.clone(),
                ..PageMeta::default()
            },
            title,
            html: String::new(),
            markdown,
            links,
            tier: "render",
            issue,
            charset: "utf-8".to_owned(),
            // The browser fetched this page, not an HTTP client; the enum has no third value.
            client: ClientKind::default(),
            source_kind: None,
            via: None,
            notes: Vec::new(),
        })
    }

    /// Raw body of `{origin}/robots.txt`, or empty if missing/rejected.
    pub async fn robots_txt(&self, page: &Url) -> String {
        let mut robots = page.clone();
        robots.set_path("/robots.txt");
        robots.set_query(None);
        robots.set_fragment(None);
        match self.fetcher.fetch_html(robots.as_str(), 64_000).await {
            Ok(r) => r.content,
            Err(FetchError::Rejected(_)) => String::new(),
            Err(e) => {
                tracing::debug!(url = %robots, error = %e, "robots.txt skipped");
                String::new()
            }
        }
    }

    /// Per-page timeout from config.
    #[must_use]
    pub fn page_timeout(&self) -> Duration {
        Duration::from_millis(self.cfg.page_timeout_ms.max(1))
    }
}

/// One fetched page before crawl bookkeeping.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// Final URL after redirects.
    pub url: String,
    /// Title when the converter found one.
    pub title: Option<String>,
    /// Raw HTML on the fetch path; empty on render, island keeps it.
    pub html: String,
    /// Post-processed markdown.
    pub markdown: String,
    /// Hrefs as they appeared (absolutized by the crawler).
    pub links: Vec<String>,
    /// `fetch`, `island`, `render` or `api`.
    pub tier: &'static str,
    /// Why the page did not read as content, with the next rung (122 D1).
    pub issue: Option<PageIssue>,
    /// Page metadata (122 D5); `title` only on the render and api tiers.
    pub meta: PageMeta,
    /// Encoding label the fetcher used (122 D3).
    pub charset: String,
    /// Which HTTP client answered (122 R1).
    pub client: ClientKind,
    /// The API source used (`tier: "api"`, 122 D11).
    pub source_kind: Option<&'static str>,
    /// The API URL actually fetched (`· via …` on the text's URL line).
    pub via: Option<String>,
    /// Non-fatal remarks (an API source that failed).
    pub notes: Vec<String>,
}

fn browser_actor() -> Option<ActorRef<browser::Browsers>> {
    ActorRef::<browser::Browsers>::lookup(browser::BROWSER_ACTOR)
        .ok()
        .flatten()
}

/// One obscura-mcp command on render slot `session`. Sent as a
/// [`tools::Tracked`] so the invocation that owns this fetch stays admitted
/// until the browser finishes the command, even after a page timeout dropped
/// this future (47 §4.2 rule 5); `session` is the slot key, never the owner.
async fn call(
    actor: &ActorRef<browser::Browsers>,
    session: SessionId,
    name: &str,
    args: Value,
) -> anyhow::Result<String> {
    let result = actor
        .ask(tools::Tracked::new(browser::Call {
            session,
            name: name.to_owned(),
            args,
        }))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if result["isError"].as_bool() == Some(true) {
        anyhow::bail!(text.strip_prefix("Error: ").unwrap_or(&text).to_owned());
    }
    Ok(text)
}

fn parse_navigated(reply: &str) -> Option<(&str, Option<&str>)> {
    let rest = reply.trim().strip_prefix("Navigated to ")?;
    match rest.rsplit_once(" — \"") {
        Some((url, title)) => {
            let title = title.strip_suffix('"').unwrap_or(title);
            Some((url, (!title.is_empty()).then_some(title)))
        }
        None => Some((rest, None)),
    }
}

/// First [`HEAD_BYTES`] of `body`, ASCII-lowercased, cut on a char boundary.
fn head(body: &str) -> String {
    let mut end = body.len().min(HEAD_BYTES);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_ascii_lowercase()
}

fn html_looks_like_html(body: &str) -> bool {
    let t = body.trim_start();
    t.len() > 15 && (t.as_bytes().starts_with(b"<") || head(t).contains("<html"))
}

/// Thin = the browser tier should run. Gate on prose, not markdown length:
/// rtings.com's plain fetch yields thousands of chars of navigation links
/// with the article client-rendered, so a raw-length gate let it through as
/// a nav shell. Thin when the scored prose is under 800 chars, or under
/// 2500 alongside an SPA root marker in the first 4 KiB of HTML.
fn is_thin(html: &str, markdown: &str) -> bool {
    let prose = web::excerpt::prose_chars(markdown);
    if prose < 800 {
        return true;
    }
    if prose >= 2500 {
        return false;
    }
    let h = head(html);
    [
        "id=\"root\"",
        "id='root'",
        "id=\"app\"",
        "id='app'",
        "__next",
    ]
    .iter()
    .any(|m| h.contains(m))
}

/// `firecrawl_native::extract_metadata` mapped into [`PageMeta`] (122 D5);
/// `web::fetch::meta::extract` (W1's scanner: author, canonical, JSON-LD)
/// wins per field, then the fetcher's `meta`, then this mapping.
fn page_meta(html: &str, base: &Url, from_fetch: PageMeta) -> PageMeta {
    let fc = firecrawl_native::extract_metadata(html).unwrap_or_default();
    let s = |k: &str| {
        fc.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let mapped = PageMeta {
        title: s("ogTitle").or_else(|| s("title")),
        description: s("description").or_else(|| s("ogDescription")),
        author: s("author").or_else(|| s("article:author")),
        published: s("publishedTime")
            .or_else(|| s("date"))
            .and_then(|raw| web::date::normalize(&raw, chrono::Utc::now())),
        lang: s("language").map(|l| l.to_lowercase()),
        canonical: None,
        site_name: s("ogSiteName"),
    };
    fill_meta(
        web::fetch::meta::extract(html, base),
        fill_meta(from_fetch, mapped),
    )
}

/// `a` with its empty fields taken from `b`.
fn fill_meta(a: PageMeta, b: PageMeta) -> PageMeta {
    PageMeta {
        title: a.title.or(b.title),
        description: a.description.or(b.description),
        author: a.author.or(b.author),
        published: a.published.or(b.published),
        lang: a.lang.or(b.lang),
        canonical: a.canonical.or(b.canonical),
        site_name: a.site_name.or(b.site_name),
    }
}

/// `# Title` of the first ATX heading.
fn first_heading(md: &str) -> Option<String> {
    md.lines().find_map(|l| {
        let t = l.trim_start_matches('#');
        (t.len() < l.len() && t.starts_with(' ')).then(|| t.trim().to_owned())
    })
}

/// Drop leading and trailing noise blocks (nav, footer, "related" rows,
/// image rows) from rendered markdown — `excerpt`'s link-noise rule, the
/// honest minimum for `only_main_content` on a tier without a DOM (122 D8).
fn trim_link_blocks(md: &str) -> String {
    let blocks: Vec<&str> = md.split("\n\n").collect();
    let start = blocks
        .iter()
        .position(|b| !is_noise(b))
        .unwrap_or(blocks.len());
    let end = blocks
        .iter()
        .rposition(|b| !is_noise(b))
        .map_or(start, |i| i + 1);
    blocks[start..end.max(start)].join("\n\n")
}

/// `transform_html` (head/script/style off, hrefs absolutized; under
/// `only_main_content` the chrome strip and D8's `select_main`, whose two
/// invariants make an empty real `<main>` stay empty and a thin scored
/// result retry with `<body>`) → markdown → base64 images stripped →
/// `post_process_markdown`.
fn to_markdown(html: &str, url: &str, only_main_content: bool) -> String {
    let opts = firecrawl_native::TransformHtmlOptions {
        html: html.to_owned(),
        url: url.to_owned(),
        include_tags: Vec::new(),
        exclude_tags: Vec::new(),
        only_main_content,
        omce_signatures: None,
    };
    let transformed = firecrawl_native::transform_html(opts).unwrap_or_else(|_| html.to_owned());
    firecrawl_native::post_process_markdown(strip_base64_images(html_to_markdown(
        &transformed,
        only_main_content,
    )))
}

/// `strip_chrome` also lets the converter drop elements it recognises as
/// navigation by `role`/`aria-label`/class tokens — a second net under
/// `transform_html`'s fixed selector list (upstream has only the list).
fn html_to_markdown(html: &str, strip_chrome: bool) -> String {
    use html_to_markdown_rs::{ConversionOptions, PreprocessingOptions, TierStrategy, convert};
    let options = ConversionOptions {
        extract_metadata: false,
        skip_images: false,
        exclude_selectors: [
            "script", "style", "noscript", "template", "svg", "iframe", "canvas", "object", "embed",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
        // With `strip_chrome` off the converter must not drop nav/footer/forms
        // on its own or `only_main_content: false` lies.
        preprocessing: PreprocessingOptions {
            remove_navigation: strip_chrome,
            remove_forms: strip_chrome,
            ..PreprocessingOptions::default()
        },
        tier_strategy: TierStrategy::Tier2,
        ..ConversionOptions::default()
    };
    match convert(html, options) {
        Ok(result) => result.content.unwrap_or_default(),
        Err(e) => {
            tracing::debug!(error = %e, "html conversion failed");
            String::new()
        }
    }
}

/// `](data:image/…;base64,…)` → `](<base64-image-removed>)` (upstream
/// `removeBase64Images`).
fn strip_base64_images(md: String) -> String {
    const NEEDLE: &str = "](data:image/";
    if !md.contains(NEEDLE) {
        return md;
    }
    let mut out = String::with_capacity(md.len());
    let mut rest = md.as_str();
    while let Some(i) = rest.find(NEEDLE) {
        let dest = &rest[i + 2..];
        match dest.find(')') {
            Some(end) if dest[..end].contains(";base64,") => {
                out.push_str(&rest[..i + 2]);
                out.push_str("<base64-image-removed>)");
                rest = &dest[end + 1..];
            }
            _ => {
                out.push_str(&rest[..i + NEEDLE.len()]);
                rest = &rest[i + NEEDLE.len()..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Link destinations from markdown, skipping images and fenced code.
fn links_from_markdown(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b']' && bytes[i + 1] == b'(' {
                let start = i + 2;
                if let Some(rel) = line[start..].find(')') {
                    let href = line[start..start + rel].trim();
                    if !href.is_empty() && !href.starts_with('#') && !is_image_label(bytes, i) {
                        out.push(href.to_owned());
                    }
                    i = start + rel;
                    continue;
                }
            }
            i += 1;
        }
    }
    out
}

/// Whether the `]` at `close` closes a `![...]` label.
fn is_image_label(bytes: &[u8], close: usize) -> bool {
    let mut depth = 0usize;
    let mut j = close;
    while j > 0 {
        j -= 1;
        match bytes[j] {
            b']' => depth += 1,
            b'[' if depth == 0 => return j > 0 && bytes[j - 1] == b'!',
            b'[' => depth -= 1,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_images_are_stripped() {
        let md = "a ![logo](data:image/png;base64,iVBOR/+=) b ![k](data:image/svg+xml,plain) \
                  [x](https://e.com/data:image/) ![y](data:image/gif;base64,AAA)"
            .to_owned();
        assert_eq!(
            strip_base64_images(md),
            "a ![logo](<base64-image-removed>) b ![k](data:image/svg+xml,plain) \
             [x](https://e.com/data:image/) ![y](<base64-image-removed>)"
        );
        let plain = "no images".to_owned();
        assert_eq!(strip_base64_images(plain.clone()), plain);
    }

    #[test]
    fn markdown_links_skip_images_and_fences() {
        let md = "[a](https://a.com) ![img](https://a.com/i.png) [![badge](https://b.png)](https://b.com)\n\
                  ```\n[c](https://c.com)\n```\n[d](https://d.com) [e](#frag)\n\
                  ~~~\n[f](https://f.com)\n~~~\n";
        assert_eq!(
            links_from_markdown(md),
            ["https://a.com", "https://b.com", "https://d.com"]
        );
    }

    #[test]
    fn navigated_reply_parses() {
        assert_eq!(
            parse_navigated("Navigated to https://e.com/x — \"Title — with dash\"\n"),
            Some(("https://e.com/x", Some("Title — with dash")))
        );
        assert_eq!(
            parse_navigated("Navigated to https://e.com/x — \"\""),
            Some(("https://e.com/x", None))
        );
        assert_eq!(
            parse_navigated("Navigated to https://e.com/x"),
            Some(("https://e.com/x", None))
        );
        assert_eq!(parse_navigated("Error: nope"), None);
    }

    #[test]
    fn only_main_content_drops_chrome_and_absolutizes() {
        let html = "<!doctype html><html><head><title>T</title><style>p{}</style></head><body>\
                    <nav><a href=\"/menu\">Menu link</a></nav>\
                    <main><h1>Hello</h1><p>Body <a href=\"/rel\">rel</a></p>\
                    <img src=\"/i.png\" alt=\"pic\"></main>\
                    <footer>Footer text</footer></body></html>";
        let main = to_markdown(html, "https://e.com/a/b", true);
        assert!(main.contains("Hello"), "{main}");
        assert!(main.contains("https://e.com/rel"), "{main}");
        assert!(main.contains("https://e.com/i.png"), "{main}");
        assert!(!main.contains("Menu link"), "{main}");
        assert!(!main.contains("Footer text"), "{main}");
        assert!(!main.contains("p{}"), "{main}");

        let full = to_markdown(html, "https://e.com/a/b", false);
        assert!(full.contains("Menu link"), "{full}");
        assert!(full.contains("Footer text"), "{full}");
        assert!(full.contains("https://e.com/menu"), "{full}");
    }

    #[test]
    fn empty_main_content_falls_back_to_whole_page() {
        let html =
            "<html><body><nav><p>Only chrome here, long enough to matter.</p></nav></body></html>";
        let md = to_markdown(html, "https://e.com/", true);
        assert!(md.contains("Only chrome here"), "{md}");
    }

    /// Blank-line separated 240-char paragraphs totalling at least `n` prose
    /// chars (each under `excerpt`'s 1200-char passage cap, so none is cut).
    fn prose(n: usize) -> String {
        let para =
            "The reviewer measured input lag across every picture mode and found it stable. "
                .repeat(3);
        let mut out = String::new();
        while out.len() < n {
            out.push_str(para.trim_end());
            out.push_str("\n\n");
        }
        out
    }

    #[test]
    fn thin_is_prose_based_not_markdown_length() {
        let mut nav_shell = String::new();
        for i in 0..300 {
            nav_shell.push_str(&format!(
                "- [Menu item number {i}](https://e.com/section/{i})\n"
            ));
        }
        nav_shell.push_str("\nLoading the review.\n");
        assert!(nav_shell.len() > 10_000);
        assert!(is_thin("<html><body>", &nav_shell));

        let article = prose(1000);
        assert!(!is_thin("<div id=\"content\"></div>", &article));
        assert!(is_thin("<div ID=\"ROOT\"></div>", &prose(1500)));
        assert!(!is_thin("<div id=\"root\"></div>", &prose(3000)));
        assert!(is_thin("<html>", "short"));
    }

    #[test]
    fn html_sniff_is_bounded() {
        assert!(html_looks_like_html("  <!DOCTYPE html><html></html>"));
        assert!(html_looks_like_html("garbage prefix here <HTML>"));
        assert!(!html_looks_like_html("# markdown\n\nplain text"));
    }

    #[test]
    fn render_trim_drops_leading_and_trailing_link_blocks() {
        let md = "- [Home](https://e.com/)\n- [Docs](https://e.com/docs)\n\n\
                  # Article\n\nA real paragraph that ends with a sentence.\n\n\
                  [See also](https://e.com/a) [More](https://e.com/b)\n\n\
                  [Privacy](https://e.com/p)\n[Terms](https://e.com/t)";
        assert_eq!(
            trim_link_blocks(md),
            "# Article\n\nA real paragraph that ends with a sentence."
        );
        // A link list with a sentence is content, not chrome.
        let kept = "- [Read the guide](https://e.com/g). It explains everything.\n\nBody.";
        assert_eq!(trim_link_blocks(kept), kept);
        assert_eq!(trim_link_blocks("[a](https://e.com)"), "");
        assert_eq!(trim_link_blocks(""), "");
    }

    #[test]
    fn page_meta_maps_firecrawl_fields() {
        let html = "<html lang=\"EN\"><head><title>Plain</title>\
                    <meta property=\"og:title\" content=\"Og Title\">\
                    <meta name=\"description\" content=\"Desc\">\
                    <meta name=\"author\" content=\"Jane Doe\">\
                    <meta property=\"article:published_time\" content=\"2026-09-30T08:00:00Z\">\
                    <meta property=\"og:site_name\" content=\"Example\">\
                    </head><body><p>x</p></body></html>";
        let base = Url::parse("https://e.com/a").unwrap();
        let m = page_meta(html, &base, PageMeta::default());
        assert_eq!(m.title.as_deref(), Some("Og Title"));
        assert_eq!(m.description.as_deref(), Some("Desc"));
        assert_eq!(m.author.as_deref(), Some("Jane Doe"));
        assert_eq!(m.published.as_deref(), Some("2026-09-30T08:00:00Z"));
        assert_eq!(m.lang.as_deref(), Some("en"));
        assert_eq!(m.site_name.as_deref(), Some("Example"));
        // The fetcher's meta wins over the mapping.
        let from_fetch = PageMeta {
            author: Some("Fetcher Author".into()),
            ..PageMeta::default()
        };
        let m = page_meta(html, &base, from_fetch);
        assert_eq!(m.author.as_deref(), Some("Fetcher Author"));
        assert_eq!(m.title.as_deref(), Some("Og Title"));
    }

    #[test]
    fn first_heading_reads_atx_only() {
        assert_eq!(first_heading("x\n## Two\n# One").as_deref(), Some("Two"));
        assert_eq!(first_heading("#hashtag\ntext"), None);
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;

    #[tokio::test]
    async fn slots_hand_out_distinct_sessions_and_recycle() {
        let slots = RenderSlots::new(2);
        let a = slots.acquire().await.unwrap();
        let b = slots.acquire().await.unwrap();
        assert_ne!(a.session, b.session);
        assert_eq!(slots.permits.available_permits(), 0);
        let first = a.session;
        drop(a);
        let c = slots.acquire().await.unwrap();
        assert_eq!(c.session, first);
        drop(b);
        drop(c);
        assert_eq!(slots.permits.available_permits(), 2);
    }
}
