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
    browser: Option<ActorRef<browser::Browsers>>,
    render_lock: tokio::sync::Mutex<()>,
    /// Publishes [`crate::ScrapeProgress`]; `None` in unit tests.
    pub(crate) rt: Option<Runtime>,
}

impl Scrape {
    /// Wrap a fetcher. `browser` is `None` when the browser plugin is off.
    #[must_use]
    pub fn new(
        fetcher: Fetcher,
        cfg: ScrapeConfig,
        browser: Option<ActorRef<browser::Browsers>>,
    ) -> Self {
        Self {
            fetcher: Arc::new(fetcher),
            cfg,
            browser,
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
                tracing::debug!(url = %url, tier = "render", "fetch thin; trying obscura");
                match self.render(url).await {
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
    /// `html-to-markdown-rs` + `post_process_markdown`.
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
        let looks_html = html_looks_like_html(&html);
        let markdown = if looks_html {
            to_markdown(&html, url.as_str(), only_main_content)
        } else {
            firecrawl_native::post_process_markdown(html.clone())
        };
        let links = if looks_html {
            firecrawl_native::extract_links(Some(html.clone())).unwrap_or_default()
        } else {
            links_from_markdown(&markdown)
        };
        Ok(Fetched {
            url: resp.url,
            title: resp.title.or_else(|| title_from_meta(&html)),
            html: if looks_html { html } else { String::new() },
            markdown,
            links,
            tier: "fetch",
        })
    }

    /// `browser_navigate` → `browser_markdown` → `browser_close` on the scrape session.
    pub async fn render(&self, url: &url::Url) -> anyhow::Result<Fetched> {
        let actor = self
            .browser
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("browser not available for rendered fetch"))?;
        let _guard = self.render_lock.lock().await;
        let navigated = call(actor, "browser_navigate", json!({ "url": url.as_str() })).await?;
        let markdown = call(
            actor,
            "browser_markdown",
            json!({ "max_chars": PAGE_CHARS }),
        )
        .await;
        let _ = call(actor, "browser_close", json!({})).await;
        let markdown = markdown?;
        let markdown = firecrawl_native::post_process_markdown(markdown);
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

    /// Body of `{origin}/robots.txt`, or empty if missing/rejected.
    pub async fn robots_txt(&self, page: &url::Url) -> String {
        let mut robots = page.clone();
        robots.set_path("/robots.txt");
        robots.set_query(None);
        robots.set_fragment(None);
        match self
            .fetcher
            .fetch(robots.as_str(), web::fetch::FetchFormat::Text, 64_000)
            .await
        {
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

    /// Run deadline from now.
    #[must_use]
    pub fn deadline(&self) -> std::time::Instant {
        std::time::Instant::now() + Duration::from_millis(self.cfg.run_timeout_ms.max(1))
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

fn html_looks_like_html(body: &str) -> bool {
    let t = body.trim_start();
    t.len() > 15 && (t.as_bytes().starts_with(b"<") || t.to_ascii_lowercase().contains("<html"))
}

fn is_thin(html: &str, markdown: &str) -> bool {
    let text = markdown.trim();
    if text.len() < 200 {
        return true;
    }
    let h = html.to_ascii_lowercase();
    let spa = [
        "id=\"root\"",
        "id='root'",
        "id=\"app\"",
        "id='app'",
        "__next",
    ]
    .iter()
    .any(|m| h.contains(m));
    spa && text.len() < 800
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

fn to_markdown(html: &str, url: &str, only_main_content: bool) -> String {
    let html = if only_main_content {
        let opts = firecrawl_native::TransformHtmlOptions {
            html: html.to_owned(),
            url: url.to_owned(),
            include_tags: Vec::new(),
            exclude_tags: Vec::new(),
            only_main_content: true,
            omce_signatures: None,
        };
        firecrawl_native::transform_html(opts).unwrap_or_else(|_| html.to_owned())
    } else {
        html.to_owned()
    };
    firecrawl_native::post_process_markdown(html_to_markdown(&html))
}

fn html_to_markdown(html: &str) -> String {
    use html_to_markdown_rs::{
        ConversionOptions, PreprocessingOptions, PreprocessingPreset, TierStrategy, convert,
    };
    let options = ConversionOptions {
        extract_metadata: false,
        skip_images: true,
        exclude_selectors: [
            "script", "style", "noscript", "template", "svg", "iframe", "canvas", "object",
            "embed", "form", "nav", "footer", "aside",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
        preprocessing: PreprocessingOptions {
            preset: PreprocessingPreset::Aggressive,
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

fn links_from_markdown(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = md.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let start = i + 2;
            if let Some(rel) = md[start..].find(')') {
                let href = md[start..start + rel].trim();
                if !href.is_empty() && !href.starts_with('#') {
                    out.push(href.to_owned());
                }
                i = start + rel;
                continue;
            }
        }
        i += 1;
    }
    out
}
