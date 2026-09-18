//! BFS crawl: visited set, page budget, hop depth, buffered concurrency,
//! per-host politeness, run deadline, robots via `build_robot` (38 §3).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use firecrawl_native::{FilterLinksCall, build_robot, filter_links, process_sitemap};
use futures::StreamExt;
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;

use crate::ProgressCx;
use crate::engine::Scrape;
use web::progress::report;

/// Token our requests present to `robots.txt` (`User-agent: wth`).
const ROBOTS_UA: &str = "wth";
/// Body cap for one sitemap document.
const SITEMAP_CHARS: usize = 8_000_000;
/// Raw sitemap URLs gathered before scoping; bounds memory on huge indexes.
const SITEMAP_URL_CAP: usize = 5_000;
/// `sitemapindex` nesting we follow.
const SITEMAP_DEPTH: u32 = 3;
/// `Crawl-delay` ceiling.
const MAX_CRAWL_DELAY: Duration = Duration::from_secs(10);

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

/// How sitemaps take part in a crawl.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SitemapMode {
    /// Seed the walk with sitemap URLs, then follow links (default).
    #[default]
    Include,
    /// Follow links only.
    Skip,
    /// Sitemap URLs only; no link walk.
    Only,
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
    /// Sitemap participation.
    pub sitemap: SitemapMode,
    /// Follow links outside the seed's directory (`allowBackwardCrawling`).
    pub crawl_entire_domain: bool,
    /// Follow links to subdomains of the seed host.
    pub allow_subdomains: bool,
    /// Match include/exclude regexes on the full URL instead of the path.
    pub regex_on_full_url: bool,
    /// Treat URLs that differ only by query string as one page.
    pub ignore_query_parameters: bool,
    /// Per-host gap between requests; robots `Crawl-delay` wins when larger.
    pub delay_ms: u64,
    /// Honour robots.txt.
    pub robots: bool,
    /// Force the obscura tier.
    pub rendered: bool,
    /// Run `transform_html` `only_main_content`.
    pub only_main_content: bool,
    /// Hard stop for the whole run; shared across a `map` run's phases.
    pub deadline: Instant,
}

impl CrawlOpts {
    /// Fill from config, then tool overlays. The deadline starts now.
    #[must_use]
    pub fn from_cfg(cfg: &crate::ScrapeConfig) -> Self {
        Self {
            limit: cfg.max_pages.max(1),
            max_depth: cfg.max_depth,
            exclude_paths: Vec::new(),
            include_paths: Vec::new(),
            same_domain: cfg.same_domain,
            sitemap: SitemapMode::Include,
            crawl_entire_domain: false,
            allow_subdomains: false,
            regex_on_full_url: false,
            ignore_query_parameters: false,
            delay_ms: cfg.delay_ms,
            robots: cfg.robots,
            rendered: false,
            only_main_content: true,
            deadline: Instant::now() + Duration::from_millis(cfg.run_timeout_ms.max(1)),
        }
    }

    /// Time left before [`Self::deadline`].
    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Keep the absolute `links` that are in scope for a crawl from `seed`:
    /// same origin (or subdomain), under the seed directory unless
    /// `crawl_entire_domain`, include/exclude regexes, robots. Returns the
    /// surviving inputs verbatim, in order, at most `limit`.
    fn scope_links(
        &self,
        seed: &Url,
        robots_txt: &str,
        links: Vec<String>,
        limit: usize,
    ) -> Vec<String> {
        let call = FilterLinksCall {
            links,
            limit: Some(i64::try_from(limit).unwrap_or(i64::MAX)),
            max_depth: 100,
            base_url: seed.origin().ascii_serialization(),
            initial_url: seed.to_string(),
            regex_on_full_url: self.regex_on_full_url,
            excludes: self.exclude_paths.clone(),
            includes: self.include_paths.clone(),
            allow_backward_crawling: self.crawl_entire_domain,
            ignore_robots_txt: !self.robots,
            robots_txt: robots_txt.to_owned(),
            robots_user_agent: Some(ROBOTS_UA.to_owned()),
            allow_external_content_links: !self.same_domain,
            allow_subdomains: self.allow_subdomains,
        };
        match filter_links(call) {
            Ok(r) => r.links,
            Err(e) => {
                tracing::debug!(error = %e, "filter_links failed");
                Vec::new()
            }
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
        let seed = strip_fragment(seed);
        let robots_txt = if opts.robots || opts.sitemap != SitemapMode::Skip {
            self.robots_txt(&seed).await
        } else {
            String::new()
        };
        self.walk(&seed, &opts, &robots_txt, cx, Vec::new(), &mut |_| true)
            .await
    }

    /// The walk behind [`Self::crawl`] and [`Self::map`]. `done` pre-seeds
    /// the progress event; `on_page` sees every page and returns `false` to
    /// stop early.
    async fn walk(
        &self,
        seed: &Url,
        opts: &CrawlOpts,
        robots_txt: &str,
        cx: Option<&ProgressCx>,
        mut done: Vec<String>,
        on_page: &mut (dyn FnMut(&Page) -> bool + Send),
    ) -> anyhow::Result<Vec<Page>> {
        let rt = self.rt.as_ref();
        let robot = build_robot(false, robots_txt, Some(ROBOTS_UA));
        let crawl_delay = robot.as_ref().filter(|_| opts.robots).and_then(|r| r.delay);
        let host_gate = HostGate::new(host_delay(crawl_delay, opts.delay_ms));
        let only = opts.sitemap == SitemapMode::Only;

        let mut frontier: VecDeque<(Url, u32)> = VecDeque::new();
        if opts.sitemap != SitemapMode::Skip {
            let sources = robot.map(|r| r.sitemaps).unwrap_or_default();
            let found: Vec<String> = self
                .sitemap_urls(seed, &sources, opts.deadline)
                .await
                .iter()
                .map(Url::to_string)
                .collect();
            let depth = if only { 0 } else { 1 };
            frontier.extend(
                opts.scope_links(seed, robots_txt, found, opts.limit)
                    .iter()
                    .filter_map(|l| Url::parse(l).ok())
                    .map(|u| (strip_fragment(u), depth)),
            );
        }
        if !only || frontier.is_empty() {
            frontier.push_front((seed.clone(), 0));
        }

        let mut pages: Vec<Page> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let concurrency = self.cfg.concurrency.max(1);
        let scrape = self;

        while !frontier.is_empty() && pages.len() < opts.limit && Instant::now() < opts.deadline {
            let mut batch = Vec::new();
            while batch.len() < concurrency
                && let Some((url, depth)) = frontier.pop_front()
            {
                if depth > opts.max_depth
                    || !visited.insert(normalize(&url, opts.ignore_query_parameters))
                {
                    continue;
                }
                batch.push((url, depth));
            }
            if batch.is_empty() {
                continue;
            }

            let mut inflight: Vec<String> = batch.iter().map(|(u, _)| u.to_string()).collect();
            report(rt, cx, &inflight, &done, opts.limit).await;

            let mut stream = futures::stream::iter(batch)
                .map(|(url, depth)| {
                    let host_gate = &host_gate;
                    async move {
                        if let Some(host) = url.host_str() {
                            host_gate.wait(host).await;
                        }
                        let budget = scrape.page_timeout().min(opts.remaining());
                        let page = tokio::time::timeout(
                            budget,
                            scrape.fetch_page(&url, opts.rendered, opts.only_main_content),
                        )
                        .await;
                        (url, depth, page)
                    }
                })
                .buffered(concurrency);

            while let Some((url, depth, outcome)) = stream.next().await {
                inflight.retain(|u| u.as_str() != url.as_str());
                let fetched = match outcome {
                    Ok(Ok(p)) => p,
                    Ok(Err(e)) => {
                        tracing::debug!(url = %url, error = %e, "page skipped");
                        report(rt, cx, &inflight, &done, opts.limit).await;
                        continue;
                    }
                    Err(_) => {
                        tracing::debug!(url = %url, "page timed out");
                        report(rt, cx, &inflight, &done, opts.limit).await;
                        continue;
                    }
                };
                if pages.len() >= opts.limit {
                    break;
                }
                let page = page_from(fetched);
                let base = Url::parse(&page.url).unwrap_or(url);
                let raw_links = if only || depth >= opts.max_depth {
                    Vec::new()
                } else {
                    absolutize_all(&base, &page.links)
                };
                let go_on = on_page(&page);
                done.push(page.url.clone());
                pages.push(page);
                report(rt, cx, &inflight, &done, opts.limit).await;
                if !go_on {
                    return Ok(pages);
                }
                if raw_links.is_empty() || pages.len() >= opts.limit {
                    continue;
                }
                // Dedupe (and drop visited) before scoping: `filter_links` counts
                // every survivor against `limit`, so three links back to the
                // seed would otherwise eat three of the remaining slots.
                let mut seen_here = HashSet::new();
                let fresh: Vec<String> = raw_links
                    .into_iter()
                    .filter(|h| {
                        Url::parse(h).is_ok_and(|u| {
                            let key = normalize(&u, opts.ignore_query_parameters);
                            !visited.contains(&key) && seen_here.insert(key)
                        })
                    })
                    .collect();
                let room = opts.limit - pages.len();
                for href in opts.scope_links(seed, robots_txt, fresh, room) {
                    if let Ok(next) = Url::parse(&href) {
                        frontier.push_back((strip_fragment(next), depth + 1));
                    }
                }
            }
        }
        Ok(pages)
    }

    /// URL discovery: `search` hits on the site first, then sitemap URLs, then
    /// a link walk whose pages and outbound links all count. Titles only where
    /// a page was fetched. `opts.allow_subdomains` / `ignore_query_parameters`
    /// scope and collapse the result.
    pub async fn map(
        &self,
        seed: Url,
        mut opts: CrawlOpts,
        search: Option<&str>,
        cx: Option<&ProgressCx>,
    ) -> anyhow::Result<Vec<MapHit>> {
        let seed = strip_fragment(seed);
        let limit = opts.limit.max(1);
        let strip_query = opts.ignore_query_parameters;
        let robots_txt = self.robots_txt(&seed).await;
        let robot = build_robot(false, &robots_txt, Some(ROBOTS_UA));

        let mut found: Vec<(String, Option<String>)> = Vec::new();
        if let Some(q) = search {
            found.extend(self.site_search(&seed, q, limit, opts.remaining()).await);
        }
        let sources = robot.map(|r| r.sitemaps).unwrap_or_default();
        found.extend(
            self.sitemap_urls(&seed, &sources, opts.deadline)
                .await
                .into_iter()
                .map(|u| (u.to_string(), None)),
        );
        let mut titles: HashMap<String, String> = found
            .iter()
            .filter_map(|(u, t)| Some((u.clone(), t.clone()?)))
            .collect();
        let urls = found.into_iter().map(|(u, _)| u).collect();

        let mut hits: Vec<MapHit> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for href in opts.scope_links(&seed, &robots_txt, urls, usize::MAX) {
            if hits.len() >= limit {
                break;
            }
            if let Ok(u) = Url::parse(&href) {
                push_hit(&mut hits, &mut seen, strip_query, u, titles.remove(&href));
            }
        }
        let done: Vec<String> = hits.iter().map(|h| h.url.clone()).collect();
        report(self.rt.as_ref(), cx, &[], &done, limit).await;

        if hits.len() < limit && Instant::now() < opts.deadline {
            opts.sitemap = SitemapMode::Skip;
            let opts = &opts;
            let mut on_page = |p: &Page| {
                let Ok(page_url) = Url::parse(&p.url) else {
                    return true;
                };
                let links = absolutize_all(&page_url, &p.links);
                push_hit(&mut hits, &mut seen, strip_query, page_url, p.title.clone());
                let room = limit.saturating_sub(hits.len());
                for href in opts.scope_links(&seed, &robots_txt, links, room) {
                    if let Ok(u) = Url::parse(&href) {
                        push_hit(&mut hits, &mut seen, strip_query, u, None);
                    }
                }
                hits.len() < limit
            };
            self.walk(&seed, opts, &robots_txt, cx, done, &mut on_page)
                .await?;
        }
        hits.truncate(limit);
        let done: Vec<String> = hits.iter().map(|h| h.url.clone()).collect();
        report(self.rt.as_ref(), cx, &[], &done, limit).await;
        Ok(hits)
    }

    /// `text site:<seed host>` through the `web` actor; `(url, title)` pairs,
    /// empty when `web` is not running or the search fails.
    async fn site_search(
        &self,
        seed: &Url,
        text: &str,
        limit: usize,
        budget: Duration,
    ) -> Vec<(String, Option<String>)> {
        let Some(host) = seed.host_str() else {
            return Vec::new();
        };
        let Ok(Some(web)) = ActorRef::<web::WebActor>::lookup(web::WEB_ACTOR) else {
            tracing::debug!("map search skipped: web actor not running");
            return Vec::new();
        };
        let query = web::Query {
            text: text.to_owned(),
            category: web::Category::General,
            limit: limit.clamp(1, 20),
            site: Some(host.trim_start_matches("www.").to_owned()),
            recent: None,
        };
        let ask = web.ask(tools::Tracked::new(web::actor::Search {
            query,
            providers: None,
        }));
        let Ok(Ok(resp)) = tokio::time::timeout(budget, ask).await else {
            tracing::debug!("map search failed or timed out");
            return Vec::new();
        };
        resp.results
            .into_iter()
            .filter_map(|r| {
                let url = strip_fragment(Url::parse(&r.url).ok()?).to_string();
                Some((url, Some(r.title).filter(|t| !t.is_empty())))
            })
            .collect()
    }

    /// Page URLs from every sitemap we can find: the seed itself when it is
    /// one, `Sitemap:` lines from robots, `{seed dir}/sitemap.xml`,
    /// `/sitemap.xml`, `/sitemap_index.xml`; `sitemapindex` recursed
    /// [`SITEMAP_DEPTH`] deep. Fetched raw (gzip handled by the fetcher).
    async fn sitemap_urls(
        &self,
        seed: &Url,
        from_robots: &[String],
        deadline: Instant,
    ) -> Vec<Url> {
        let mut candidates: Vec<Url> = Vec::new();
        if is_sitemap_path(seed.path()) {
            candidates.push(seed.clone());
        }
        candidates.extend(from_robots.iter().filter_map(|s| Url::parse(s).ok()));
        if seed.path() != "/" {
            candidates.extend(seed.join("sitemap.xml"));
        }
        candidates.extend(seed.join("/sitemap.xml"));
        candidates.extend(seed.join("/sitemap_index.xml"));

        let mut queue: VecDeque<(Url, u32)> = candidates.into_iter().map(|u| (u, 0)).collect();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        while let Some((url, depth)) = queue.pop_front() {
            if Instant::now() >= deadline || out.len() >= SITEMAP_URL_CAP {
                break;
            }
            if depth > SITEMAP_DEPTH || !seen.insert(normalize(&url, false)) {
                continue;
            }
            let body = match self.fetcher.fetch_html(url.as_str(), SITEMAP_CHARS).await {
                Ok(r) => r.content,
                Err(e) => {
                    tracing::debug!(url = %url, error = %e, "sitemap skipped");
                    continue;
                }
            };
            for (u, recurse) in sitemap_entries(&body) {
                if recurse {
                    queue.push_back((u, depth + 1));
                } else {
                    out.push(u);
                }
            }
        }
        out
    }
}

/// `(url, is_nested_sitemap)` for every `<loc>` in `xml`; a body the parser
/// rejects (truncated, odd namespaces) still yields what a plain scan finds.
fn sitemap_entries(xml: &str) -> Vec<(Url, bool)> {
    match process_sitemap(xml) {
        Ok(parsed) => parsed
            .instructions
            .into_iter()
            .flat_map(|inst| {
                let recurse = inst.action == "recurse";
                inst.urls
                    .into_iter()
                    .filter_map(|u| Url::parse(&u).ok())
                    .map(move |u| (strip_fragment(u), recurse))
            })
            .collect(),
        Err(_) => {
            let mut out = Vec::new();
            let mut rest = xml;
            while let Some(i) = rest.find("<loc>") {
                rest = &rest[i + "<loc>".len()..];
                let Some(j) = rest.find("</loc>") else { break };
                if let Ok(u) = Url::parse(rest[..j].trim()) {
                    let recurse = is_sitemap_path(u.path());
                    out.push((strip_fragment(u), recurse));
                }
                rest = &rest[j + "</loc>".len()..];
            }
            out
        }
    }
}

fn is_sitemap_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".xml") || p.ends_with(".xml.gz")
}

/// Robots `Crawl-delay` (seconds) when it beats the configured floor, capped
/// at [`MAX_CRAWL_DELAY`].
fn host_delay(crawl_delay: Option<f32>, floor_ms: u64) -> Duration {
    let floor = Duration::from_millis(floor_ms);
    match crawl_delay {
        Some(d) if d.is_finite() && d > 0.0 => {
            Duration::from_secs_f32(d).min(MAX_CRAWL_DELAY).max(floor)
        }
        _ => floor,
    }
}

/// Record `url` once; a later sighting with a title fills a title-less hit.
fn push_hit(
    hits: &mut Vec<MapHit>,
    seen: &mut HashMap<String, usize>,
    strip_query: bool,
    mut url: Url,
    title: Option<String>,
) {
    if strip_query {
        url.set_query(None);
    }
    match seen.entry(normalize(&url, strip_query)) {
        std::collections::hash_map::Entry::Occupied(e) => {
            let hit = &mut hits[*e.get()];
            if hit.title.is_none() {
                hit.title = title;
            }
        }
        std::collections::hash_map::Entry::Vacant(e) => {
            e.insert(hits.len());
            hits.push(MapHit {
                url: url.to_string(),
                title,
            });
        }
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

/// Dedupe key: no fragment, lowercase host sans `www.`, no `/index.html`,
/// no trailing slash (root aside), tracking params dropped; the whole query
/// when `ignore_query`.
fn normalize(url: &Url, ignore_query: bool) -> String {
    let mut u = url.clone();
    u.set_fragment(None);
    let bare = u
        .host_str()
        .and_then(|h| h.strip_prefix("www."))
        .map(str::to_owned);
    if let Some(host) = bare {
        let _ = u.set_host(Some(&host));
    }
    if ignore_query {
        u.set_query(None);
    }
    let dir = u
        .path()
        .strip_suffix("/index.html")
        .map(|d| format!("{d}/"));
    if let Some(dir) = dir {
        u.set_path(&dir);
    }
    web::url::canonical_key(u.as_str())
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

/// Per-host request spacing. Each waiter reserves the next free slot so N
/// concurrent requests to one host go out `delay` apart, not together.
struct HostGate {
    next: Mutex<HashMap<String, Instant>>,
    delay: Duration,
}

impl HostGate {
    fn new(delay: Duration) -> Self {
        Self {
            next: Mutex::new(HashMap::new()),
            delay,
        }
    }

    async fn wait(&self, host: &str) {
        if self.delay.is_zero() {
            return;
        }
        let slot = {
            let mut map = self.next.lock().await;
            let now = Instant::now();
            let slot = map
                .get(host)
                .map_or(now, |last| (*last + self.delay).max(now));
            map.insert(host.to_owned(), slot);
            slot
        };
        tokio::time::sleep_until(slot.into()).await;
    }
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

    async fn mount(server: &MockServer, at: &str, body: String, ct: &str) {
        Mock::given(method("GET"))
            .and(path(at))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, ct))
            .mount(server)
            .await;
    }

    fn scrape_with(server: &MockServer, robots: bool) -> Scrape {
        let fetcher = Fetcher::new("wth-test", 2_097_152, 5)
            .expect("client")
            .allowing(vec![*server.address()]);
        let cfg = ScrapeConfig {
            robots,
            concurrency: 2,
            run_timeout_ms: 10_000,
            ..Default::default()
        };
        Scrape::new(fetcher, cfg)
    }

    fn scrape(server: &MockServer) -> Scrape {
        scrape_with(server, false)
    }

    #[tokio::test]
    async fn crawl_honours_limit_and_depth() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/",
            page(&format!(
                "<a href=\"{u}/a\">a</a><a href=\"{u}/b\">b</a><a href=\"{u}/c\">c</a>"
            )),
            "text/html",
        )
        .await;
        for p in ["/a", "/b", "/c"] {
            mount(&server, p, page(p), "text/html").await;
        }
        let s = scrape(&server);
        let seed = Url::parse(&server.uri()).unwrap();
        let opts = CrawlOpts {
            limit: 2,
            ..CrawlOpts::from_cfg(&s.cfg)
        };
        let pages = s.crawl(seed.clone(), opts, None).await.expect("crawl");
        assert_eq!(pages.len(), 2, "limit");

        let opts = CrawlOpts {
            limit: 10,
            max_depth: 0,
            ..CrawlOpts::from_cfg(&s.cfg)
        };
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
        let u = server.uri();
        mount(
            &server,
            "/",
            page(&format!(
                "<a href=\"{u}/blog/2020-01-01/\">d</a><a href=\"{u}/blog/hello\">h</a>"
            )),
            "text/html",
        )
        .await;
        mount(&server, "/blog/2020-01-01/", page("date"), "text/html").await;
        mount(&server, "/blog/hello", page("hello"), "text/html").await;
        let s = scrape(&server);
        let seed = Url::parse(&server.uri()).unwrap();
        let opts = CrawlOpts {
            exclude_paths: vec!["/blog/[0-9]{4}-[0-9]{2}-[0-9]{2}".into()],
            ..CrawlOpts::from_cfg(&s.cfg)
        };
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
        let u = server.uri();
        mount(
            &server,
            "/docs/guide.md",
            page(&format!(
                "<a href=\"{u}/docs/other\">o</a><a href=\"{u}/blog/x\">b</a>"
            )),
            "text/html",
        )
        .await;
        mount(&server, "/docs/other", page("other"), "text/html").await;
        mount(&server, "/blog/x", page("blog"), "text/html").await;
        let s = scrape(&server);
        let seed = Url::parse(&format!("{}/docs/guide.md", server.uri())).unwrap();
        let pages = s
            .crawl(seed, CrawlOpts::from_cfg(&s.cfg), None)
            .await
            .expect("crawl");
        assert!(
            pages.iter().all(|p| p.url.contains("/docs/")),
            "scope: {:?}",
            pages.iter().map(|p| &p.url).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn crawl_honours_robots_disallow() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/robots.txt",
            "User-agent: *\nDisallow: /secret".into(),
            "text/plain",
        )
        .await;
        mount(
            &server,
            "/",
            page(&format!(
                "<a href=\"{u}/secret\">s</a><a href=\"{u}/ok\">o</a>"
            )),
            "text/html",
        )
        .await;
        mount(&server, "/secret", page("secret"), "text/html").await;
        mount(&server, "/ok", page("ok"), "text/html").await;
        let s = scrape_with(&server, true);
        let seed = Url::parse(&server.uri()).unwrap();
        let pages = s
            .crawl(seed, CrawlOpts::from_cfg(&s.cfg), None)
            .await
            .expect("crawl");
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

    #[tokio::test]
    async fn sitemap_only_follows_index_and_keeps_scope() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/sitemap.xml",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\
                 <sitemap><loc>{u}/pages.xml</loc></sitemap></sitemapindex>"
            ),
            "application/xml",
        )
        .await;
        mount(
            &server,
            "/pages.xml",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\
                 <url><loc>{u}/docs/a</loc></url><url><loc>{u}/blog/b</loc></url></urlset>"
            ),
            "application/xml",
        )
        .await;
        mount(&server, "/docs/a", page("a"), "text/html").await;
        mount(&server, "/blog/b", page("b"), "text/html").await;
        mount(&server, "/docs/", page("index"), "text/html").await;
        let s = scrape(&server);
        let seed = Url::parse(&format!("{u}/docs/")).unwrap();
        let opts = CrawlOpts {
            sitemap: SitemapMode::Only,
            ..CrawlOpts::from_cfg(&s.cfg)
        };
        let pages = s.crawl(seed, opts, None).await.expect("crawl");
        let urls: Vec<_> = pages.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![format!("{u}/docs/a")],
            "in-scope sitemap URL only"
        );
    }

    #[tokio::test]
    async fn ignore_query_parameters_collapses_variants() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/",
            page(&format!(
                "<a href=\"{u}/a?x=1\">1</a><a href=\"{u}/a?x=2\">2</a>"
            )),
            "text/html",
        )
        .await;
        mount(&server, "/a", page("a"), "text/html").await;
        let s = scrape(&server);
        let seed = Url::parse(&u).unwrap();
        let opts = CrawlOpts {
            ignore_query_parameters: true,
            sitemap: SitemapMode::Skip,
            ..CrawlOpts::from_cfg(&s.cfg)
        };
        let pages = s.crawl(seed, opts, None).await.expect("crawl");
        assert_eq!(
            pages.len(),
            2,
            "seed plus one /a: {:?}",
            pages.iter().map(|p| &p.url).collect::<Vec<_>>()
        );
    }

    #[test]
    fn normalize_collapses_shape_variants() {
        let a = Url::parse("https://www.Example.com/docs/index.html#top").unwrap();
        let b = Url::parse("https://example.com/docs/").unwrap();
        let c = Url::parse("https://example.com/docs?x=1").unwrap();
        assert_eq!(normalize(&a, false), normalize(&b, false));
        assert_ne!(normalize(&b, false), normalize(&c, false));
        assert_eq!(normalize(&b, true), normalize(&c, true));
    }

    #[test]
    fn sitemap_entries_salvage_truncated_xml() {
        let cut = "<urlset><url><loc>https://a.example/x</loc></url><url><loc>https://a.example/nested.xml</loc></url><url><loc>https://a.exa";
        let got = sitemap_entries(cut);
        assert_eq!(
            got.iter()
                .map(|(u, r)| (u.as_str(), *r))
                .collect::<Vec<_>>(),
            vec![
                ("https://a.example/x", false),
                ("https://a.example/nested.xml", true)
            ]
        );
    }
}
