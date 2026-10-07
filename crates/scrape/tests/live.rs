//! Design doc 122 §6.3 live smoke for `scrape_url` / `crawl` / `map_site`
//! (network). Ignored by default; runs when `THEUTH_LIVE=1`:
//!
//! ```sh
//! THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=always cargo test -p scrape --test live -- --ignored --nocapture
//! THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=never  cargo test -p scrape --test live -- --ignored --nocapture
//! ```
//!
//! `THEUTH_LIVE_IMPERSONATE` is the test-local switch for `web.impersonate`.
//! No browser actor is registered, so the render rung surfaces the issue
//! instead of rendering. Rows assert shape only and print
//! `url · client · tier · issue · chars · compacted · took_ms`.

use std::time::{Duration, Instant};

use scrape::{CrawlOpts, Page, Scrape, ScrapeConfig, SitemapMode};
use url::Url;
use web::fetch::Fetcher;
use web::fetch::client::Impersonate;
use web::tools::Source;

const URLS: &[&str] = &[
    "https://example.com",
    "https://docs.rs/reqwest/latest/reqwest/",
    "https://news.ycombinator.com/item?id=1",
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://httpbin.org/status/403",
    "https://httpbin.org/status/429",
    "https://www.jalan.net/",
    "http://abehiroshi.la.coocan.jp/",
    "https://nowsecure.nl",
    "https://www.g2.com/",
    "https://stackoverflow.com/questions/26960537/how-do-i-print-a-rust-string",
];

fn live() -> bool {
    std::env::var("THEUTH_LIVE").is_ok_and(|v| v == "1")
}

fn impersonate() -> Impersonate {
    match std::env::var("THEUTH_LIVE_IMPERSONATE").as_deref() {
        Ok("never") => Impersonate::Never,
        Ok("on_challenge") => Impersonate::OnChallenge,
        _ => Impersonate::Always,
    }
}

fn scrape() -> Scrape {
    let web_cfg = web::WebConfig::default();
    let cfg = ScrapeConfig::default();
    let fetcher = Fetcher::new_with_timeouts(
        &web_cfg.user_agent,
        web_cfg.max_fetch_bytes.max(8 * 1024 * 1024),
        web_cfg.max_redirects,
        Duration::from_millis(cfg.page_timeout_ms),
        web::fetch::CONNECT_TIMEOUT,
    )
    .expect("client")
    .with_policy(
        impersonate(),
        web_cfg.header_ladder,
        Some(&web_cfg.accept_language),
    );
    Scrape::new(fetcher, cfg)
}

fn row(url: &str, page: &anyhow::Result<Page>, took: u128) -> String {
    match page {
        Err(e) => format!("{url} · ERROR · - · {e} · 0 · 0 · {took}"),
        Ok(p) => {
            let c = web::compact::compact(
                &p.markdown,
                &web::compact::CompactOptions {
                    fold_links: false,
                    keep_images: true,
                },
            );
            let issue = p.issue.as_ref().map_or_else(
                || "-".to_owned(),
                |i| format!("{:?} ({})", i.kind, i.detail),
            );
            format!(
                "{url} · {:?} · {} · {issue} · {} · {} · {took} · charset={} source_kind={:?} notes={:?}",
                p.client,
                p.tier,
                p.markdown.chars().count(),
                c.removed_chars,
                p.charset,
                p.source_kind,
                p.notes,
            )
        }
    }
}

#[tokio::test]
#[ignore = "network; THEUTH_LIVE=1"]
async fn scrape_url_matrix() {
    if !live() {
        eprintln!("THEUTH_LIVE unset; skipping");
        return;
    }
    let s = scrape();
    println!("impersonate={:?}", impersonate());
    for url in URLS {
        let u = Url::parse(url).unwrap();
        let t = Instant::now();
        let page = s.scrape_url(&u, false, true, Source::Auto, None).await;
        println!("{}", row(url, &page, t.elapsed().as_millis()));
        if let Ok(p) = &page {
            assert!(
                matches!(p.tier, "fetch" | "island" | "render" | "api"),
                "{url}"
            );
            assert!(!p.charset.is_empty(), "{url}");
        }
    }
}

#[tokio::test]
#[ignore = "network; THEUTH_LIVE=1"]
async fn crawl_with_limit_two_is_incomplete_and_map_names_sources() {
    if !live() {
        return;
    }
    let s = scrape();
    let seed = Url::parse("https://docs.rs/reqwest/latest/reqwest/").unwrap();
    let opts = CrawlOpts {
        limit: 2,
        max_depth: 1,
        sitemap: SitemapMode::Skip,
        ..CrawlOpts::from_cfg(&ScrapeConfig::default())
    };
    let t = Instant::now();
    let c = s.crawl(seed.clone(), opts, None).await.expect("crawl");
    println!(
        "crawl {seed}: pages={} incomplete={} reason={:?} skipped={} invalid_patterns={:?} took_ms={}",
        c.pages.len(),
        c.incomplete,
        c.reason,
        c.skipped.len(),
        c.invalid_patterns,
        t.elapsed().as_millis()
    );
    for p in &c.pages {
        println!(
            "  {} · {:?} · {} · truncated={} next_offset={:?} chars={}",
            p.url,
            p.client,
            p.tier,
            p.truncated,
            p.next_offset,
            p.markdown.chars().count()
        );
    }
    assert!(c.pages.len() <= 2);
    if c.pages.len() == 2 {
        assert!(c.incomplete, "two of many pages read: incomplete");
        assert_eq!(c.reason, Some("limit"));
    }

    let opts = CrawlOpts {
        limit: 10,
        max_depth: 1,
        sitemap: SitemapMode::Include,
        ..CrawlOpts::from_cfg(&ScrapeConfig::default())
    };
    let t = Instant::now();
    let hits = s.map(seed, opts, None, None).await.expect("map");
    let mut by_source: std::collections::BTreeMap<&str, usize> = Default::default();
    for h in &hits {
        *by_source.entry(h.source).or_default() += 1;
    }
    println!(
        "map: hits={} by_source={by_source:?} took_ms={}",
        hits.len(),
        t.elapsed().as_millis()
    );
    assert!(
        hits.iter()
            .all(|h| matches!(h.source, "search" | "sitemap" | "page" | "frontier"))
    );
}
