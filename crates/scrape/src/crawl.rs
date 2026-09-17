//! BFS crawl: visited set, page budget, hop depth, buffered concurrency,
//! per-host politeness, run deadline, robots via `build_robot` (38 §3).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use firecrawl_native::{FilterLinksCall, filter_links, process_sitemap};
use futures::StreamExt;
use runtime::Runtime;
use tokio::sync::Mutex;
use url::Url;

use crate::engine::Scrape;
use crate::{ProgressCx, ScrapeProgress};

/// One crawled page.
#[derive(Clone, Debug)]
pub struct Page {
    /// Final URL.
    pub url: String,
    /// Title when known.
    pub title: Option<String>,
    /// Post-processed markdown.
    pub markdown: String,
    /// Raw HTML when the fetch tier answered.
    pub html: String,
    /// Outbound hrefs (absolute).
    pub links: Vec<String>,
    /// `fetch` or `render`.
    pub tier: &'static str,
}

/// Discovery hit for [`Scrape::map`](crate::Scrape::map).
#[derive(Clone, Debug)]
pub struct MapHit {
    /// Absolute URL.
    pub url: String,
    /// Title when a page was fetched.
    pub title: Option<String>,
}

/// Options for one crawl/map run. Tool args overlay [`crate::ScrapeConfig`].
#[derive(Clone, Debug)]
pub struct CrawlOpts {
    /// Page budget.
    pub limit: usize,
    /// Hop-depth cap from the seed (0 = seed only).
    pub max_depth: u32,
    /// `excludePaths` for `filter_links`.
    pub exclude_paths: Vec<String>,
    /// `includePaths` for `filter_links`.
    pub include_paths: Vec<String>,
    /// Stay on the seed host.
    pub same_domain: bool,
    /// Sitemap only; no HTML link walk.
    pub only_sitemap: bool,
    /// Honour robots.txt.
    pub robots: bool,
    /// Force the obscura tier.
    pub rendered: bool,
    /// Run `transform_html` `only_main_content`.
    pub only_main_content: bool,
}

impl CrawlOpts {
    /// Fill from config, then tool overlays.
    #[must_use]
    pub fn from_cfg(cfg: &crate::ScrapeConfig) -> Self {
        Self {
            limit: cfg.max_pages.max(1),
            max_depth: cfg.max_depth,
            exclude_paths: Vec::new(),
            include_paths: Vec::new(),
            same_domain: cfg.same_domain,
            only_sitemap: false,
            robots: cfg.robots,
            rendered: false,
            only_main_content: false,
        }
    }
}

impl Scrape {
    /// Scrape one URL (no walk).
    pub async fn scrape_url(
        &self,
        url: &Url,
        rendered: bool,
        only_main_content: bool,
        cx: Option<&ProgressCx>,
    ) -> anyhow::Result<Page> {
        let inflight = vec![url.to_string()];
        report(self.rt.as_ref(), cx, &inflight, &[], 1).await;
        let fetched = tokio::time::timeout(
            self.page_timeout(),
            self.fetch_page(url, rendered, only_main_content),
        )
        .await
        .map_err(|_| anyhow::anyhow!("page timed out"))??;
        let page = page_from(fetched);
        report(
            self.rt.as_ref(),
            cx,
            &[],
            std::slice::from_ref(&page.url),
            1,
        )
        .await;
        Ok(page)
    }

    /// BFS from `seed` until limit, depth, or run deadline.
    pub async fn crawl(
        &self,
        seed: Url,
        opts: CrawlOpts,
        cx: Option<&ProgressCx>,
    ) -> anyhow::Result<Vec<Page>> {
        let deadline = self.deadline();
        let robots = if opts.robots {
            self.robots_txt(&seed).await
        } else {
            String::new()
        };
        let mut pages = Vec::new();
        let mut done: Vec<String> = Vec::new();
        let rt = self.rt.clone();
        if opts.only_sitemap {
            let urls = self.sitemap_urls(&seed, opts.limit, deadline).await;
            for u in urls {
                if Instant::now() >= deadline || pages.len() >= opts.limit {
                    break;
                }
                report(rt.as_ref(), cx, &[u.to_string()], &done, opts.limit).await;
                match tokio::time::timeout(
                    self.page_timeout(),
                    self.fetch_page(&u, opts.rendered, opts.only_main_content),
                )
                .await
                {
                    Ok(Ok(p)) => {
                        let page = page_from(p);
                        done.push(page.url.clone());
                        pages.push(page);
                    }
                    Ok(Err(e)) => tracing::debug!(url = %u, error = %e, "sitemap page skipped"),
                    Err(_) => tracing::debug!(url = %u, "sitemap page timed out"),
                }
                report(rt.as_ref(), cx, &[], &done, opts.limit).await;
            }
            return Ok(pages);
        }

        let mut visited = HashSet::new();
        let mut frontier: VecDeque<(Url, u32)> = VecDeque::new();
        frontier.push_back((seed.clone(), 0));
        let host_gate = HostGate::new(Duration::from_millis(100));
        let concurrency = self.cfg.concurrency.max(1);

        while !frontier.is_empty() && pages.len() < opts.limit && Instant::now() < deadline {
            let mut batch = Vec::new();
            while batch.len() < concurrency && !frontier.is_empty() {
                let (url, depth) = frontier.pop_front().expect("just checked");
                if depth > opts.max_depth {
                    continue;
                }
                let key = normalize(&url);
                if !visited.insert(key) {
                    continue;
                }
                batch.push((url, depth));
            }
            if batch.is_empty() {
                continue;
            }
            let remaining = Instant::now()
                .checked_duration_since(deadline)
                .map(|_| Duration::ZERO)
                .unwrap_or_else(|| deadline.saturating_duration_since(Instant::now()));
            if remaining.is_zero() {
                break;
            }

            let mut inflight: Vec<String> = batch.iter().map(|(u, _)| u.to_string()).collect();
            report(rt.as_ref(), cx, &inflight, &done, opts.limit).await;

            let scrape = self;
            let mut stream = futures::stream::iter(batch)
                .map(|(url, depth)| {
                    let host_gate = &host_gate;
                    async move {
                        if let Some(host) = url.host_str() {
                            host_gate.wait(host).await;
                        }
                        let page = tokio::time::timeout(
                            scrape.page_timeout(),
                            scrape.fetch_page(&url, opts.rendered, opts.only_main_content),
                        )
                        .await;
                        (url, depth, page)
                    }
                })
                .buffered(concurrency);

            while let Some((url, depth, outcome)) = stream.next().await {
                let key = url.to_string();
                inflight.retain(|u| u != &key);
                if pages.len() >= opts.limit || Instant::now() >= deadline {
                    report(rt.as_ref(), cx, &inflight, &done, opts.limit).await;
                    break;
                }
                let fetched = match outcome {
                    Ok(Ok(p)) => p,
                    Ok(Err(e)) => {
                        tracing::debug!(url = %url, error = %e, "page skipped");
                        report(rt.as_ref(), cx, &inflight, &done, opts.limit).await;
                        continue;
                    }
                    Err(_) => {
                        tracing::debug!(url = %url, "page timed out");
                        report(rt.as_ref(), cx, &inflight, &done, opts.limit).await;
                        continue;
                    }
                };
                let page = page_from(fetched);
                let raw_links = page.links.clone();
                done.push(page.url.clone());
                pages.push(page);
                report(rt.as_ref(), cx, &inflight, &done, opts.limit).await;
                if depth >= opts.max_depth || pages.len() >= opts.limit {
                    continue;
                }
                let absolute = absolutize_all(&url, &raw_links);
                let filtered = match filter_links(FilterLinksCall {
                    links: absolute,
                    limit: Some(
                        i64::try_from(opts.limit.saturating_sub(pages.len())).unwrap_or(i64::MAX),
                    ),
                    max_depth: 100,
                    base_url: url.to_string(),
                    initial_url: seed.to_string(),
                    regex_on_full_url: false,
                    excludes: opts.exclude_paths.clone(),
                    includes: opts.include_paths.clone(),
                    allow_backward_crawling: false,
                    ignore_robots_txt: !opts.robots,
                    robots_txt: robots.clone(),
                    robots_user_agent: None,
                    allow_external_content_links: !opts.same_domain,
                    allow_subdomains: false,
                }) {
                    Ok(r) => r.links,
                    Err(e) => {
                        tracing::debug!(error = %e, "filter_links failed");
                        continue;
                    }
                };
                for href in filtered {
                    if let Ok(next) = Url::parse(&href) {
                        let next = strip_fragment(next);
                        if !visited.contains(&normalize(&next)) {
                            frontier.push_back((next, depth + 1));
                        }
                    }
                }
            }
        }
        Ok(pages)
    }

    /// Sitemap first, then a link walk; URLs + titles, no markdown bodies.
    pub async fn map(
        &self,
        seed: Url,
        limit: usize,
        cx: Option<&ProgressCx>,
    ) -> anyhow::Result<Vec<MapHit>> {
        let deadline = self.deadline();
        let limit = limit.max(1);
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for u in self.sitemap_urls(&seed, limit, deadline).await {
            if seen.insert(normalize(&u)) {
                hits.push(MapHit {
                    url: u.to_string(),
                    title: None,
                });
            }
            if hits.len() >= limit {
                return Ok(hits);
            }
        }
        let mut opts = CrawlOpts::from_cfg(&self.cfg);
        opts.limit = limit;
        opts.rendered = false;
        let pages = self.crawl(seed, opts, cx).await?;
        for p in pages {
            if seen.insert(p.url.clone()) {
                hits.push(MapHit {
                    url: p.url,
                    title: p.title,
                });
            }
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    async fn sitemap_urls(&self, seed: &Url, limit: usize, deadline: Instant) -> Vec<Url> {
        let mut out = Vec::new();
        let mut queue = VecDeque::new();
        if let Some(origin) = seed.host_str()
            && let Ok(u) = Url::parse(&format!("{}://{origin}/sitemap.xml", seed.scheme()))
        {
            queue.push_back((u, 0u32));
        }
        let mut seen = HashSet::new();
        while let Some((url, depth)) = queue.pop_front() {
            if Instant::now() >= deadline || out.len() >= limit || depth > 3 {
                break;
            }
            if !seen.insert(normalize(&url)) {
                continue;
            }
            let xml = match self
                .fetcher
                .fetch(url.as_str(), web::fetch::FetchFormat::Text, 1_000_000)
                .await
            {
                Ok(r) => r.content,
                Err(_) => continue,
            };
            let parsed = match process_sitemap(&xml) {
                Ok(p) => p,
                Err(_) => continue,
            };
            for inst in parsed.instructions {
                for u in inst.urls {
                    let Ok(parsed_u) = Url::parse(&u) else {
                        continue;
                    };
                    if inst.action == "recurse" {
                        queue.push_back((parsed_u, depth + 1));
                    } else if out.len() < limit {
                        out.push(parsed_u);
                    }
                }
            }
        }
        out
    }
}

fn page_from(f: crate::engine::Fetched) -> Page {
    Page {
        url: f.url,
        title: f.title,
        markdown: f.markdown,
        html: f.html,
        links: f.links,
        tier: f.tier,
    }
}

fn normalize(url: &Url) -> String {
    strip_fragment(url.clone()).to_string()
}

fn strip_fragment(mut url: Url) -> Url {
    url.set_fragment(None);
    url
}

fn absolutize_all(base: &Url, hrefs: &[String]) -> Vec<String> {
    hrefs
        .iter()
        .filter_map(|h| {
            let mut u = base.join(h).ok()?;
            u.set_fragment(None);
            if u.scheme() != "http" && u.scheme() != "https" {
                return None;
            }
            Some(u.to_string())
        })
        .collect()
}

struct HostGate {
    last: Mutex<HashMap<String, Instant>>,
    delay: Duration,
}

impl HostGate {
    fn new(delay: Duration) -> Self {
        Self {
            last: Mutex::new(HashMap::new()),
            delay,
        }
    }

    async fn wait(&self, host: &str) {
        let sleep_for = {
            let mut map = self.last.lock().await;
            let now = Instant::now();
            let wait = map
                .get(host)
                .and_then(|t| self.delay.checked_sub(now.saturating_duration_since(*t)))
                .filter(|d| !d.is_zero());
            map.insert(host.to_owned(), now);
            wait
        };
        if let Some(d) = sleep_for {
            tokio::time::sleep(d).await;
            let mut map = self.last.lock().await;
            map.insert(host.to_owned(), Instant::now());
        }
    }
}

async fn report(
    rt: Option<&Runtime>,
    cx: Option<&ProgressCx>,
    inflight: &[String],
    done: &[String],
    limit: usize,
) {
    let (Some(rt), Some(cx)) = (rt, cx) else {
        return;
    };
    rt.publish(ScrapeProgress {
        session: cx.session,
        call_id: cx.call_id.clone(),
        inflight: inflight.to_vec(),
        done: done.to_vec(),
        limit,
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScrapeConfig;
    use web::fetch::Fetcher;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn page(body: &str) -> String {
        format!("<!doctype html><html><head><title>t</title></head><body>{body}</body></html>")
    }

    async fn scrape(server: &MockServer) -> Scrape {
        let fetcher = Fetcher::new("wth-test", 2_097_152, 5)
            .expect("client")
            .allowing(vec![*server.address()]);
        let mut cfg = ScrapeConfig::default();
        cfg.robots = false;
        cfg.concurrency = 2;
        cfg.run_timeout_ms = 10_000;
        Scrape::new(fetcher, cfg, None)
    }

    #[tokio::test]
    async fn crawl_honours_limit_and_depth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                page(&format!(
                    "<a href=\"{}/a\">a</a><a href=\"{}/b\">b</a><a href=\"{}/c\">c</a>",
                    server.uri(),
                    server.uri(),
                    server.uri()
                )),
                "text/html",
            ))
            .mount(&server)
            .await;
        for p in ["/a", "/b", "/c"] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_raw(page(p), "text/html"))
                .mount(&server)
                .await;
        }
        let s = scrape(&server).await;
        let seed = Url::parse(&server.uri()).unwrap();
        let mut opts = CrawlOpts::from_cfg(&s.cfg);
        opts.limit = 2;
        opts.robots = false;
        let pages = s.crawl(seed.clone(), opts, None).await.expect("crawl");
        assert_eq!(pages.len(), 2, "limit");

        let mut opts = CrawlOpts::from_cfg(&s.cfg);
        opts.limit = 10;
        opts.max_depth = 0;
        opts.robots = false;
        let pages = s.crawl(seed, opts, None).await.expect("depth");
        assert_eq!(pages.len(), 1, "seed only at depth 0");
        assert_eq!(
            pages[0].url.trim_end_matches('/'),
            server.uri().trim_end_matches('/')
        );
    }

    #[tokio::test]
    async fn crawl_exclude_paths_skips_date_archive() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                page(&format!(
                    "<a href=\"{}/blog/2020-01-01/\">d</a><a href=\"{}/blog/hello\">h</a>",
                    server.uri(),
                    server.uri()
                )),
                "text/html",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/blog/2020-01-01/"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page("date"), "text/html"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/blog/hello"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page("hello"), "text/html"))
            .mount(&server)
            .await;
        let s = scrape(&server).await;
        let seed = Url::parse(&server.uri()).unwrap();
        let mut opts = CrawlOpts::from_cfg(&s.cfg);
        opts.robots = false;
        opts.exclude_paths = vec!["/blog/[0-9]{4}-[0-9]{2}-[0-9]{2}".into()];
        let pages = s.crawl(seed, opts, None).await.expect("crawl");
        let urls: Vec<_> = pages.iter().map(|p| p.url.clone()).collect();
        assert!(
            urls.iter().all(|u| !u.contains("2020-01-01")),
            "date archive excluded: {urls:?}"
        );
        assert!(
            urls.iter().any(|u| u.contains("/blog/hello")),
            "named post kept: {urls:?}"
        );
    }

    #[tokio::test]
    async fn deep_seed_stays_under_directory() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/docs/guide.md"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                page(&format!(
                    "<a href=\"{}/docs/other\">o</a><a href=\"{}/blog/x\">b</a>",
                    server.uri(),
                    server.uri()
                )),
                "text/html",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/docs/other"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page("other"), "text/html"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/blog/x"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page("blog"), "text/html"))
            .mount(&server)
            .await;
        let s = scrape(&server).await;
        let seed = Url::parse(&format!("{}/docs/guide.md", server.uri())).unwrap();
        let mut opts = CrawlOpts::from_cfg(&s.cfg);
        opts.robots = false;
        let pages = s.crawl(seed, opts, None).await.expect("crawl");
        assert!(
            pages.iter().all(|p| p.url.contains("/docs/")),
            "scope: {:?}",
            pages.iter().map(|p| &p.url).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn crawl_honours_robots_disallow() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("User-agent: *\nDisallow: /secret", "text/plain"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                page(&format!(
                    "<a href=\"{}/secret\">s</a><a href=\"{}/ok\">o</a>",
                    server.uri(),
                    server.uri()
                )),
                "text/html",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/secret"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page("secret"), "text/html"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/ok"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page("ok"), "text/html"))
            .mount(&server)
            .await;
        let fetcher = Fetcher::new("wth-test", 2_097_152, 5)
            .expect("client")
            .allowing(vec![*server.address()]);
        let mut cfg = ScrapeConfig::default();
        cfg.robots = true;
        cfg.concurrency = 2;
        let s = Scrape::new(fetcher, cfg, None);
        let seed = Url::parse(&server.uri()).unwrap();
        let mut opts = CrawlOpts::from_cfg(&s.cfg);
        opts.robots = true;
        let pages = s.crawl(seed, opts, None).await.expect("crawl");
        let urls: Vec<_> = pages.iter().map(|p| p.url.clone()).collect();
        assert!(
            urls.iter().all(|u| !u.contains("/secret")),
            "robots disallow: {urls:?}"
        );
        assert!(
            urls.iter().any(|u| u.contains("/ok")),
            "allowed page: {urls:?}"
        );
    }
}
