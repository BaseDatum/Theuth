//! The engine ladder `fetch → issue → island → render` (design doc 122 D1
//! scrape side, D9) against wiremock. The contract stubs for `detect` and
//! `extract_islands` answer `None`, so each test injects what the real
//! functions will return through the engine's test seams; nothing here
//! passes *because* a stub is empty. No browser actor is registered, so the
//! render rung is the "not registered → surface/keep" branch.

use std::time::Duration;

use scrape::{CrawlOpts, Scrape, ScrapeConfig, SitemapMode};
use url::Url;
use web::fetch::Fetcher;
use web::fetch::issue::{IssueKind, Next, PageIssue};
use web::tools::Source;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scrape(server: &MockServer) -> Scrape {
    let fetcher = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()]);
    Scrape::new(
        fetcher,
        ScrapeConfig {
            concurrency: 2,
            run_timeout_ms: 10_000,
            ..ScrapeConfig::default()
        },
    )
}

async fn mount(server: &MockServer, at: &str, status: u16, body: String) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(status).set_body_raw(body, "text/html"))
        .mount(server)
        .await;
}

fn challenge() -> PageIssue {
    PageIssue {
        kind: IssueKind::AccessChallenge,
        detail: "HTTP 403 · cloudflare".into(),
        retry_after: None,
        next: vec![Next::Rendered, Next::ImportCookies, Next::Handoff],
    }
}

fn prose(n: usize) -> String {
    "The reviewer measured input lag across every picture mode and found it stable. "
        .repeat(n.div_ceil(80))
}

#[tokio::test]
async fn access_challenge_is_surfaced_when_no_browser_is_registered() {
    let server = MockServer::start().await;
    // `cf-mitigated` is conclusive at any status (D1 row 1); the real fetcher
    // sets `issue` from it, the stub does not — the seam supplies the same value.
    Mock::given(method("GET"))
        .and(path("/wall"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("cf-mitigated", "challenge")
                .set_body_raw(
                    "<html><head><title>Just a moment…</title></head><body>\
                     <p>Enable JavaScript and cookies to continue.</p></body></html>",
                    "text/html",
                ),
        )
        .mount(&server)
        .await;
    let s = scrape(&server).with_detector(|_status, _headers, url, _body, _prose| {
        (url.path() == "/wall").then(challenge)
    });
    let url = Url::parse(&format!("{}/wall", server.uri())).unwrap();
    let page = s
        .fetch_page(&url, false, true)
        .await
        .expect("surfaced, not an error");
    assert_eq!(page.tier, "fetch");
    let issue = page.issue.expect("issue carried from the fetch");
    assert_eq!(issue.kind, IssueKind::AccessChallenge);
    assert_eq!(issue.detail, "HTTP 403 · cloudflare");
    assert!(issue.next.contains(&Next::Rendered));
    assert!(
        page.markdown.contains("Enable JavaScript"),
        "{}",
        page.markdown
    );

    // Through `scrape_url`: the Page carries the same issue.
    let page = s
        .scrape_url(&url, false, true, Source::Auto, None)
        .await
        .expect("scrape_url");
    assert_eq!(page.issue.map(|i| i.kind), Some(IssueKind::AccessChallenge));
}

#[tokio::test]
async fn login_required_is_surfaced_with_its_next() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/login",
        200,
        "<html><head><title>Sign in</title></head><body><form class=\"login-form\">\
         <p>Email</p><p>Password</p></form></body></html>"
            .into(),
    )
    .await;
    let s = scrape(&server).with_detector(|_, _, _, _, _| {
        Some(PageIssue {
            kind: IssueKind::LoginRequired,
            detail: "login".into(),
            retry_after: None,
            next: vec![Next::ImportCookies, Next::Handoff],
        })
    });
    let url = Url::parse(&format!("{}/login", server.uri())).unwrap();
    let page = s.fetch_page(&url, false, true).await.expect("surfaced");
    let issue = page.issue.expect("issue");
    assert_eq!(issue.kind, IssueKind::LoginRequired);
    assert_eq!(issue.next, vec![Next::ImportCookies, Next::Handoff]);
    assert_eq!(page.tier, "fetch");
}

#[tokio::test]
async fn island_rung_runs_before_render_and_clears_empty_shell() {
    let server = MockServer::start().await;
    let island = prose(1_500);
    let shell = format!(
        "<html><head><title>App</title></head><body><div id=\"__next\"></div>\
         <script id=\"__NEXT_DATA__\" type=\"application/json\">{{\"props\":{{\"body\":\"{island}\"}}}}</script>\
         </body></html>"
    );
    mount(&server, "/app", 200, shell).await;
    let island_text = island.clone();
    let s = scrape(&server)
        .with_detector(|_, _, _, _, _| {
            Some(PageIssue {
                kind: IssueKind::EmptyShell,
                detail: "empty-shell".into(),
                retry_after: None,
                next: vec![Next::Rendered],
            })
        })
        .with_islands(move |html| html.contains("__NEXT_DATA__").then(|| island_text.clone()));
    let url = Url::parse(&format!("{}/app", server.uri())).unwrap();
    let page = s.fetch_page(&url, false, true).await.expect("island");
    assert_eq!(page.tier, "island");
    assert_eq!(
        page.issue, None,
        "EmptyShell is cleared once the island reads"
    );
    assert!(
        page.markdown.contains("measured input lag"),
        "{}",
        page.markdown
    );
    assert!(!page.html.is_empty(), "the island tier keeps the HTML");

    // Island too thin (< 800 prose chars): not used; no browser → the shell
    // is kept with its issue.
    let s = scrape(&server)
        .with_detector(|_, _, _, _, _| {
            Some(PageIssue {
                kind: IssueKind::EmptyShell,
                detail: "empty-shell".into(),
                retry_after: None,
                next: vec![Next::Rendered],
            })
        })
        .with_islands(|_| Some(prose(200)));
    let page = s.fetch_page(&url, false, true).await.expect("kept");
    assert_eq!(page.tier, "fetch");
    assert_eq!(page.issue.map(|i| i.kind), Some(IssueKind::EmptyShell));
}

#[tokio::test]
async fn thin_page_without_issue_tries_the_island_too() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/thin",
        200,
        "<html><body><div id=\"root\"></div><script>window.__DATA__ = 1</script></body></html>"
            .into(),
    )
    .await;
    let s = scrape(&server).with_islands(|_| Some(prose(1_000)));
    let url = Url::parse(&format!("{}/thin", server.uri())).unwrap();
    let page = s.fetch_page(&url, false, true).await.expect("island");
    assert_eq!(page.tier, "island");
    assert_eq!(page.issue, None);
}

#[tokio::test]
async fn full_page_skips_island_and_render() {
    let server = MockServer::start().await;
    let body = format!(
        "<html><head><title>Full</title></head><body><main><p>{}</p></main></body></html>",
        prose(3_000)
    );
    mount(&server, "/full", 200, body).await;
    let s = scrape(&server).with_islands(|_| panic!("island rung must not run on a full page"));
    let url = Url::parse(&format!("{}/full", server.uri())).unwrap();
    let page = s.fetch_page(&url, false, true).await.expect("fetch");
    assert_eq!(page.tier, "fetch");
    assert_eq!(page.issue, None);
    assert_eq!(page.title.as_deref(), Some("Full"));
    assert_eq!(page.meta.title.as_deref(), Some("Full"));
}

#[tokio::test]
async fn fetch_error_without_browser_keeps_the_http_error() {
    let server = MockServer::start().await;
    mount(&server, "/gone", 500, String::new()).await;
    let s = scrape(&server);
    let url = Url::parse(&format!("{}/gone", server.uri())).unwrap();
    let err = s
        .fetch_page(&url, false, true)
        .await
        .expect_err("no browser");
    assert!(err.to_string().contains("HTTP 500"), "{err}");
}

#[tokio::test]
async fn crawl_rows_carry_issue_and_client() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/",
        200,
        format!(
            "<html><body><a href=\"{u}/wall\">w</a><p>{}</p></body></html>",
            prose(1_000),
            u = server.uri()
        ),
    )
    .await;
    mount(
        &server,
        "/wall",
        200,
        "<html><body><p>Just a moment…</p></body></html>".into(),
    )
    .await;
    let s =
        scrape(&server).with_detector(|_, _, url, _, _| (url.path() == "/wall").then(challenge));
    let seed = Url::parse(&server.uri()).unwrap();
    let opts = CrawlOpts {
        sitemap: SitemapMode::Skip,
        ..CrawlOpts::from_cfg(&ScrapeConfig::default())
    };
    let c = tokio::time::timeout(Duration::from_secs(20), s.crawl(seed, opts, None))
        .await
        .expect("in time")
        .expect("crawl");
    assert_eq!(c.pages.len(), 2);
    let wall = c
        .pages
        .iter()
        .find(|p| p.url.ends_with("/wall"))
        .expect("wall row");
    assert_eq!(
        wall.issue.as_ref().map(|i| i.kind),
        Some(IssueKind::AccessChallenge)
    );
    assert_eq!(wall.client, web::fetch::client::ClientKind::Reqwest);
    assert!(!c.incomplete);
}
