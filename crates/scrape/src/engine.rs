//! Fetch adapter and two-tier engine pick (38 §0.3, §3).

use std::sync::Arc;
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

/// One isolate for scrape's render tier, distinct from `browser`'s
/// `read_url` session.
static RENDER_SESSION: std::sync::LazyLock<SessionId> = std::sync::LazyLock::new(|| {
    SessionId(uuid::Uuid::from_u128(
        0x7363_7261_7065_5f72_656e_6465_725f_0038,
    ))
});

/// Shared scrape handle: SSRF-guarded fetch, optional obscura, config.
pub struct Scrape {
    /// HTTP fetcher with a large body cap.
    pub(crate) fetcher: Arc<Fetcher>,
    /// `scrape:` section.
    pub(crate) cfg: ScrapeConfig,
    render_lock: tokio::sync::Mutex<()>,
    /// Publishes [`crate::ScrapeProgress`]; `None` in unit tests.
    pub(crate) rt: Option<Runtime>,
}

impl Scrape {
    /// Wrap a fetcher. The browser actor is looked up at render time.
    #[must_use]
    pub fn new(fetcher: Fetcher, cfg: ScrapeConfig) -> Self {
        Self {
            fetcher: Arc::new(fetcher),
            cfg,
            render_lock: tokio::sync::Mutex::new(()),
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

    /// `browser_navigate` → `browser_markdown` → `browser_close` on the scrape session.
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
        let _guard = self.render_lock.lock().await;
        let navigated = call(actor, "browser_navigate", json!({ "url": url.as_str() })).await?;
        let markdown = call(
            actor,
            "browser_markdown",
            json!({ "max_chars": PAGE_CHARS }),
        )
        .await;
        let _ = call(actor, "browser_close", json!({})).await;
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
    name: &str,
    args: Value,
) -> anyhow::Result<String> {
    let result = actor
        .ask(browser::Call {
            session: *RENDER_SESSION,
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

// ponytail: thin = <200 chars of markdown, or <800 chars alongside an SPA
// root marker in the first 4 KiB of HTML. Upgrade path is upstream's
// engine-picker (text/HTML ratio, status codes) if this misfires on real
// sites.
fn is_thin(html: &str, markdown: &str) -> bool {
    let text = markdown.trim();
    if text.len() < 200 {
        return true;
    }
    if text.len() >= 800 {
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

    #[test]
    fn thin_sniff_is_case_insensitive_and_bounded() {
        let long = "x".repeat(300);
        assert!(is_thin("<div ID=\"ROOT\"></div>", &long));
        assert!(!is_thin("<div id=\"content\"></div>", &long));
        assert!(!is_thin("<div id=\"root\"></div>", &"x".repeat(900)));
        assert!(is_thin("<html>", "short"));
        assert!(html_looks_like_html("  <!DOCTYPE html><html></html>"));
        assert!(html_looks_like_html("garbage prefix here <HTML>"));
        assert!(!html_looks_like_html("# markdown\n\nplain text"));
    }
}
