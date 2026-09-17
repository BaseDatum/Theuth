//! Fetch adapter and two-tier engine pick (38 §0.3, §3).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};
use web::fetch::{FetchError, Fetcher};

use crate::ScrapeConfig;

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
        }
    }

    /// Attach the bus so crawl/scrape emit [`crate::ScrapeProgress`].
    #[must_use]
    pub fn with_runtime(mut self, rt: Runtime) -> Self {
        self.rt = Some(rt);
        self
    }

    /// Fetch `url`: HTTP first, obscura when `force_render` or the fetch is thin.
    pub async fn fetch_page(
        &self,
        url: &url::Url,
        force_render: bool,
        only_main_content: bool,
    ) -> anyhow::Result<Fetched> {
        if force_render {
            return self.render(url).await;
        }
        match self.fetch_http(url, only_main_content).await {
            Ok(page) if !is_thin(&page.html, &page.markdown) => {
                tracing::debug!(url = %url, tier = "fetch", "scrape engine");
                Ok(page)
            }
            Ok(thin) => {
                let Some(actor) = browser_actor() else {
                    return Ok(thin);
                };
                tracing::debug!(url = %url, tier = "render", "fetch thin; trying obscura");
                match self.render_with(&actor, url).await {
                    Ok(page) => Ok(page),
                    Err(e) => {
                        tracing::debug!(error = %e, "render failed; keeping thin fetch");
                        Ok(thin)
                    }
                }
            }
            Err(e) => {
                tracing::debug!(url = %url, error = %e, "fetch failed; trying obscura");
                self.render(url).await
            }
        }
    }

    /// Plain HTTP: HTML (or already-converted document markdown), then
    /// `transform_html` → `html-to-markdown-rs` → `post_process_markdown`.
    pub async fn fetch_http(
        &self,
        url: &url::Url,
        only_main_content: bool,
    ) -> anyhow::Result<Fetched> {
        let resp = self
            .fetcher
            .fetch_html(url.as_str(), PAGE_CHARS)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let html = resp.content;
        if !html_looks_like_html(&html) {
            let markdown = firecrawl_native::post_process_markdown(html);
            return Ok(Fetched {
                url: resp.url,
                title: resp.title,
                links: links_from_markdown(&markdown),
                html: String::new(),
                markdown,
                tier: "fetch",
            });
        }
        let markdown = to_markdown(&html, url.as_str(), only_main_content);
        Ok(Fetched {
            url: resp.url,
            title: resp.title.or_else(|| title_from_meta(&html)),
            links: firecrawl_native::extract_links(Some(html.clone())).unwrap_or_default(),
            html,
            markdown,
            tier: "fetch",
        })
    }

    /// `browser_navigate` → `browser_markdown` → `browser_close` on a free render slot.
    pub async fn render(&self, url: &url::Url) -> anyhow::Result<Fetched> {
        let actor = browser_actor()
            .ok_or_else(|| anyhow::anyhow!("browser not available for rendered fetch"))?;
        self.render_with(&actor, url).await
    }

    async fn render_with(
        &self,
        actor: &ActorRef<browser::Browsers>,
        url: &url::Url,
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
        let markdown = firecrawl_native::post_process_markdown(strip_base64_images(markdown?));
        let (final_url, title) = parse_navigated(&navigated).unwrap_or((url.as_str(), None));
        let links = links_from_markdown(&markdown);
        tracing::debug!(url = %url, tier = "render", "scrape engine");
        Ok(Fetched {
            url: final_url.to_owned(),
            title: title.map(str::to_owned),
            html: String::new(),
            markdown,
            links,
            tier: "render",
        })
    }

    /// Raw body of `{origin}/robots.txt`, or empty if missing/rejected.
    pub async fn robots_txt(&self, page: &url::Url) -> String {
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
    /// Raw HTML on the fetch path; empty on render.
    pub html: String,
    /// Post-processed markdown.
    pub markdown: String,
    /// Hrefs as they appeared (absolutized by the crawler).
    pub links: Vec<String>,
    /// `fetch` or `render`.
    pub tier: &'static str,
}

fn browser_actor() -> Option<ActorRef<browser::Browsers>> {
    ActorRef::<browser::Browsers>::lookup(browser::BROWSER_ACTOR)
        .ok()
        .flatten()
}

async fn call(
    actor: &ActorRef<browser::Browsers>,
    session: SessionId,
    name: &str,
    args: Value,
) -> anyhow::Result<String> {
    let result = actor
        .ask(browser::Call {
            session,
            name: name.to_owned(),
            args,
        })
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

fn title_from_meta(html: &str) -> Option<String> {
    firecrawl_native::extract_metadata(html)
        .ok()
        .and_then(|m| {
            m.get("title")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|s| !s.is_empty())
}

/// `transform_html` (head/script/style off, hrefs absolutized, chrome off
/// when `only_main_content`) → markdown → base64 images stripped →
/// `post_process_markdown`. An empty main-content result falls back to the
/// whole page, as upstream does.
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
    let md = firecrawl_native::post_process_markdown(strip_base64_images(html_to_markdown(
        &transformed,
        only_main_content,
    )));
    if only_main_content && md.trim().is_empty() && !html.trim().is_empty() {
        return to_markdown(html, url, false);
    }
    md
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
